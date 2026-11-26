//! Streaming Hugging Face model downloader with atomic `.part` writes and progress reporting.

use crate::config::{ModelKind, model_dir};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct DownloadProgress {
    pub file_name: String,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub mb_per_sec: f32,
}

impl DownloadProgress {
    pub fn fraction(&self) -> f32 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.downloaded_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0) as f32
        }
    }
}

pub fn ensure_model_downloaded<F>(kind: ModelKind, mut on_progress: F) -> Result<PathBuf, String>
where
    F: FnMut(DownloadProgress),
{
    let dir = model_dir(kind);
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create {}: {e}", dir.display()))?;

    let specs = kind.files();
    let total_bytes: u64 = specs.iter().map(|s| s.approx_bytes).sum();
    let mut completed_before: u64 = 0;

    let client = reqwest::blocking::Client::builder()
        .timeout(None)
        .connect_timeout(Duration::from_secs(30))
        .user_agent("voicebar/0.1.0")
        .build()
        .map_err(|e| format!("HTTP client init failed: {e}"))?;

    let started = Instant::now();
    let mut session_downloaded: u64 = 0;

    for spec in specs {
        let final_path = dir.join(spec.local_name);
        if let Ok(meta) = fs::metadata(&final_path) {
            if meta.is_file() && meta.len() >= (spec.approx_bytes * 9 / 10).max(1) {
                completed_before += meta.len();
                on_progress(DownloadProgress {
                    file_name: spec.local_name.to_string(),
                    downloaded_bytes: completed_before.min(total_bytes),
                    total_bytes,
                    mb_per_sec: 0.0,
                });
                continue;
            }
        }

        let part_path = dir.join(format!("{}.part", spec.local_name));
        let url = ModelKind::hf_url(spec.remote_path);

        on_progress(DownloadProgress {
            file_name: spec.local_name.to_string(),
            downloaded_bytes: completed_before,
            total_bytes,
            mb_per_sec: 0.0,
        });

        let mut resp = client
            .get(&url)
            .send()
            .map_err(|e| format!("Download failed for {}: {e}", spec.local_name))?;

        if !resp.status().is_success() {
            return Err(format!(
                "HTTP {} downloading {}",
                resp.status(),
                spec.local_name
            ));
        }

        let mut out = File::create(&part_path)
            .map_err(|e| format!("Cannot create {}: {e}", part_path.display()))?;

        let mut buf = vec![0u8; 256 * 1024];
        let mut file_bytes: u64 = 0;
        let mut last_emit = Instant::now();

        loop {
            let n = resp
                .read(&mut buf)
                .map_err(|e| format!("Read error while downloading {}: {e}", spec.local_name))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])
                .map_err(|e| format!("Write error for {}: {e}", spec.local_name))?;
            file_bytes += n as u64;
            session_downloaded += n as u64;

            if last_emit.elapsed() >= Duration::from_millis(80) {
                last_emit = Instant::now();
                let secs = started.elapsed().as_secs_f32().max(0.05);
                let mbps = (session_downloaded as f32 / (1024.0 * 1024.0)) / secs;
                on_progress(DownloadProgress {
                    file_name: spec.local_name.to_string(),
                    downloaded_bytes: (completed_before + file_bytes).min(total_bytes),
                    total_bytes: total_bytes.max(completed_before + file_bytes),
                    mb_per_sec: mbps,
                });
            }
        }

        out.flush()
            .map_err(|e| format!("Flush error for {}: {e}", spec.local_name))?;
        drop(out);

        if final_path.exists() {
            let _ = fs::remove_file(&final_path);
        }
        fs::rename(&part_path, &final_path)
            .map_err(|e| format!("Rename failed for {}: {e}", spec.local_name))?;

        completed_before += file_bytes;
        let secs = started.elapsed().as_secs_f32().max(0.05);
        let mbps = (session_downloaded as f32 / (1024.0 * 1024.0)) / secs;
        on_progress(DownloadProgress {
            file_name: spec.local_name.to_string(),
            downloaded_bytes: completed_before.min(total_bytes),
            total_bytes: total_bytes.max(completed_before),
            mb_per_sec: mbps,
        });
    }

    Ok(dir)
}
