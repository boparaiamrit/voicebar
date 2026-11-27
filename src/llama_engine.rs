//! Self-contained built-in `llama.cpp` (`llama-server`) runtime & Hugging Face GGUF downloader.
//! Zero Ollama required: VoiceBar manages its own Vulkan/Metal `llama-server` binary and local
//! GGUF weights, starts the server on-demand on a loopback port, and shuts it down cleanly on
//! toggle-off or app exit.

use crate::config::{
    RewriteModel, is_rewrite_model_downloaded, llama_runtime_dir, llm_model_path, llm_models_dir,
};
use reqwest::blocking::Client;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const LLAMA_CPP_TAG: &str = "b12814";

#[derive(Clone, Debug)]
pub struct DownloadStatus {
    pub status: String,
    pub completed: u64,
    pub total: u64,
}

impl DownloadStatus {
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.completed as f64 / self.total as f64).clamp(0.0, 1.0) as f32
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeReadiness {
    /// Both `llama-server` runtime AND the selected GGUF weight file are present locally.
    Ready,
    /// `llama-server` is installed, only the selected GGUF model needs downloading.
    ModelMissing,
    /// Neither the built-in `llama-server` runtime nor the model is downloaded yet (1-click auto-download).
    EngineMissing,
}

/// Official `ggml-org/llama.cpp` release archive filename for this OS & architecture.
/// Uses native Vulkan on Windows/Linux (works on NVIDIA, AMD, Intel + automatic CPU fallback)
/// and native Metal on Apple Silicon macOS.
pub fn llama_archive_name() -> &'static str {
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        "llama-b12814-bin-win-vulkan-arm64.zip"
    }
    #[cfg(all(target_os = "windows", not(target_arch = "aarch64")))]
    {
        "llama-b12814-bin-win-vulkan-x64.zip"
    }
    #[cfg(target_os = "macos")]
    {
        "llama-b12814-bin-macos-arm64.tar.gz"
    }
    #[cfg(all(unix, not(target_os = "macos"), target_arch = "aarch64"))]
    {
        "llama-b12814-bin-ubuntu-vulkan-arm64.tar.gz"
    }
    #[cfg(all(unix, not(target_os = "macos"), not(target_arch = "aarch64")))]
    {
        "llama-b12814-bin-ubuntu-vulkan-x64.tar.gz"
    }
}

pub fn llama_archive_url() -> String {
    format!(
        "https://github.com/ggml-org/llama.cpp/releases/download/{LLAMA_CPP_TAG}/{}",
        llama_archive_name()
    )
}

fn server_exe_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

fn find_file_recursive(root: &Path, target_name: &str, depth: usize) -> Option<PathBuf> {
    if depth == 0 || !root.is_dir() {
        return None;
    }
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file()
            && p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case(target_name))
        {
            return Some(p);
        }
        if p.is_dir() {
            if let Some(found) = find_file_recursive(&p, target_name, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

/// Locate VoiceBar's managed `llama-server` executable (or an existing `llama-server` on PATH).
pub fn find_llama_server() -> Option<PathBuf> {
    let exe = server_exe_name();
    let versioned = llama_runtime_dir().join(LLAMA_CPP_TAG);
    if let Some(found) = find_file_recursive(&versioned, exe, 4) {
        return Some(found);
    }
    if let Some(found) = find_file_recursive(&llama_runtime_dir(), exe, 4) {
        return Some(found);
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join(exe);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn check_readiness(model: RewriteModel) -> RuntimeReadiness {
    let has_engine = find_llama_server().is_some();
    let has_model = is_rewrite_model_downloaded(model);
    match (has_engine, has_model) {
        (true, true) => RuntimeReadiness::Ready,
        (true, false) => RuntimeReadiness::ModelMissing,
        (false, _) => RuntimeReadiness::EngineMissing,
    }
}

fn http_client() -> Result<Client, &'static str> {
    Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(3600))
        .user_agent("Voicebar/0.3 (llama.cpp downloader)")
        .build()
        .map_err(|_| "Voicebar could not initialize its download client.")
}

fn download_file_streaming(
    url: &str,
    dest: &Path,
    label: &str,
    approx_bytes: u64,
    cancel: &AtomicBool,
    on_progress: &mut impl FnMut(DownloadStatus),
) -> Result<(), &'static str> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|_| "Could not create Voicebar models folder.")?;
    }
    let part = dest.with_extension("part");
    let client = http_client()?;
    let mut response = client
        .get(url)
        .send()
        .map_err(|_| "Could not connect to download server. Check your internet connection.")?;
    if !response.status().is_success() {
        return Err("Download server returned an error for this file. Please try again.");
    }
    let total = response.content_length().unwrap_or(approx_bytes);
    let mut file =
        File::create(&part).map_err(|_| "Could not write downloaded file to disk.")?;

    let mut buf = vec![0u8; 256 * 1024];
    let mut downloaded = 0u64;
    let mut last_emit = Instant::now() - Duration::from_secs(1);

    loop {
        if cancel.load(Ordering::Acquire) {
            drop(file);
            let _ = fs::remove_file(&part);
            return Err("Download cancelled.");
        }
        let n = response
            .read(&mut buf)
            .map_err(|_| "Download interrupted. Please retry.")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|_| "Disk write failed while downloading model. Check free disk space.")?;
        downloaded += n as u64;
        if last_emit.elapsed() >= Duration::from_millis(120) {
            on_progress(DownloadStatus {
                status: label.to_string(),
                completed: downloaded,
                total,
            });
            last_emit = Instant::now();
        }
    }

    file.flush()
        .map_err(|_| "Could not finalize downloaded file.")?;
    drop(file);

    if cancel.load(Ordering::Acquire) {
        let _ = fs::remove_file(&part);
        return Err("Download cancelled.");
    }

    let _ = fs::remove_file(dest);
    fs::rename(&part, dest).map_err(|_| "Could not move completed download into place.")?;
    on_progress(DownloadStatus {
        status: format!("{label} · complete"),
        completed: total.max(downloaded),
        total: total.max(downloaded),
    });
    Ok(())
}

fn extract_archive(archive: &Path, dest_dir: &Path) -> Result<(), &'static str> {
    fs::create_dir_all(dest_dir).map_err(|_| "Could not create llama.cpp runtime folder.")?;

    let tar_cmd = if cfg!(windows) { "tar.exe" } else { "tar" };
    let mut cmd = Command::new(tar_cmd);
    cmd.arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dest_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    if cmd.status().is_ok_and(|s| s.success()) {
        return Ok(());
    }

    // Windows PowerShell fallback if bsdtar is ever unavailable
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let ps = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!(
                    "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
                    archive.display(),
                    dest_dir.display()
                ),
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if ps.is_ok_and(|s| s.success()) {
            return Ok(());
        }
    }

    Err("Could not extract the built-in llama.cpp runtime archive.")
}

/// Ensure BOTH VoiceBar's built-in `llama-server` (~16–38 MB) AND the selected `.gguf` file
/// are downloaded and ready locally — zero Ollama needed.
pub fn ensure_runtime_and_model(
    model: RewriteModel,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(DownloadStatus),
) -> Result<PathBuf, &'static str> {
    if find_llama_server().is_none() {
        let rt_root = llama_runtime_dir();
        let target_dir = rt_root.join(LLAMA_CPP_TAG);
        let archive_path = rt_root.join(llama_archive_name());
        download_file_streaming(
            &llama_archive_url(),
            &archive_path,
            "Downloading built-in llama.cpp engine (Vulkan/Metal)",
            38_000_000,
            cancel,
            &mut on_progress,
        )?;
        on_progress(DownloadStatus {
            status: "Extracting built-in llama.cpp engine…".into(),
            completed: 1,
            total: 1,
        });
        extract_archive(&archive_path, &target_dir)?;
        let _ = fs::remove_file(&archive_path);

        #[cfg(unix)]
        if let Some(bin) = find_llama_server() {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(&bin) {
                let mut perm = meta.permissions();
                perm.set_mode(0o755);
                let _ = fs::set_permissions(&bin, perm);
            }
        }

        if find_llama_server().is_none() {
            return Err("Built-in llama.cpp extracted, but llama-server executable was not found.");
        }
    }

    let model_path = llm_model_path(model);
    if !is_rewrite_model_downloaded(model) {
        let _ = fs::create_dir_all(llm_models_dir());
        download_file_streaming(
            model.hf_download_url(),
            &model_path,
            &format!("Downloading {}", model.display_name()),
            model.approx_bytes(),
            cancel,
            &mut on_progress,
        )?;
    }

    Ok(model_path)
}

// =========================================================================
// Managed background `llama-server` daemon (127.0.0.1 ephemeral loopback)
// =========================================================================

struct RunningServer {
    model: RewriteModel,
    requested_use_gpu: bool,
    port: u16,
    child: Child,
}

impl RunningServer {
    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn server_slot() -> &'static Mutex<Option<RunningServer>> {
    static SLOT: OnceLock<Mutex<Option<RunningServer>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn pick_free_port() -> u16 {
    if let Ok(l) = TcpListener::bind(("127.0.0.1", 17982)) {
        drop(l);
        return 17982;
    }
    TcpListener::bind(("127.0.0.1", 0))
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
        .unwrap_or(17982)
}

fn health_ok(port: u16) -> bool {
    let Ok(client) = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_millis(500))
        .timeout(Duration::from_millis(800))
        .build()
    else {
        return false;
    };
    client
        .get(format!("http://127.0.0.1:{port}/health"))
        .send()
        .is_ok_and(|r| r.status().is_success())
}

fn spawn_llama_server(
    bin: &Path,
    model_path: &Path,
    port: u16,
    gpu_layers: u32,
) -> Result<Child, &'static str> {
    let mut cmd = Command::new(bin);
    if let Some(dir) = bin.parent() {
        cmd.current_dir(dir);
    }
    cmd.args([
        "--model",
        &model_path.to_string_lossy(),
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--ctx-size",
        "8192",
        "--n-gpu-layers",
        &gpu_layers.to_string(),
        "--reasoning",
        "off",
        "--no-webui",
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    cmd.spawn()
        .map_err(|_| "Voicebar could not start its built-in llama.cpp server.")
}

fn wait_until_healthy(child: &mut Child, port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            return false;
        }
        if health_ok(port) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

/// Ensure VoiceBar's built-in `llama-server` is running for `model` and return its local base URL.
/// Automatically falls back from Vulkan/Metal GPU offload to CPU if GPU allocation fails.
pub fn ensure_server_running(model: RewriteModel, use_gpu: bool) -> Result<String, &'static str> {
    let mut guard = server_slot()
        .lock()
        .map_err(|_| "Built-in rewrite engine lock error.")?;

    if let Some(running) = guard.as_mut() {
        if running.model == model
            && running.requested_use_gpu == use_gpu
            && running.is_alive()
            && health_ok(running.port)
        {
            return Ok(format!("http://127.0.0.1:{}", running.port));
        }
        running.stop();
        *guard = None;
    }

    let bin = find_llama_server()
        .ok_or("Built-in llama.cpp engine is not downloaded yet. Click Download in Personal Rewrite Studio.")?;
    let model_path = llm_model_path(model);
    if !is_rewrite_model_downloaded(model) {
        return Err("Selected local GGUF model is not downloaded yet. Click Download first.");
    }

    let port = pick_free_port();
    let layers = if use_gpu { 99 } else { 0 };
    let mut child = spawn_llama_server(&bin, &model_path, port, layers)?;
    if wait_until_healthy(&mut child, port, Duration::from_secs(45)) {
        *guard = Some(RunningServer {
            model,
            requested_use_gpu: use_gpu,
            port,
            child,
        });
        return Ok(format!("http://127.0.0.1:{port}"));
    }

    // GPU offload failed (e.g., tight VRAM alongside Nemotron) — fall back cleanly to CPU!
    let _ = child.kill();
    let _ = child.wait();
    if use_gpu {
        let mut cpu_child = spawn_llama_server(&bin, &model_path, port, 0)?;
        if wait_until_healthy(&mut cpu_child, port, Duration::from_secs(45)) {
            *guard = Some(RunningServer {
                model,
                requested_use_gpu: use_gpu,
                port,
                child: cpu_child,
            });
            return Ok(format!("http://127.0.0.1:{port}"));
        }
        let _ = cpu_child.kill();
        let _ = cpu_child.wait();
    }

    Err("Built-in llama.cpp server could not load this model. Try a smaller model like Qwen 3 · 0.6B.")
}

/// Immediately shut down the background `llama-server` process and free VRAM/RAM.
pub fn shutdown_server() {
    if let Ok(mut guard) = server_slot().lock() {
        if let Some(mut running) = guard.take() {
            running.stop();
        }
    }
}

pub fn is_server_running_for(model: RewriteModel) -> bool {
    if let Ok(mut guard) = server_slot().lock() {
        if let Some(running) = guard.as_mut() {
            return running.model == model && running.is_alive();
        }
    }
    false
}
