//! Real-time streaming GPU ASR engine powered by `parakeet-rs` (pure-Rust Burn + Vulkan WGPU).
//! Supports Nemotron 3.5 0.6B, Nemotron EN 0.6B, Parakeet Unified EN 0.6B, and Parakeet EOU 120M.

use crate::audio::{AudioShared, MicCapture};
use crate::config::{AppConfig, ModelKind, is_model_ready, model_dir};
use crate::download::{DownloadProgress, ensure_model_downloaded};
use parakeet_rs::{
    ExecutionConfig, ExecutionProvider, Nemotron, NemotronMode, ParakeetEOU, ParakeetEOUHandle,
    ParakeetUnified,
};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

const EOU_CHUNK_SAMPLES: usize = 2_560; // 160 ms at 16 kHz

#[derive(Debug)]
pub enum EngineCommand {
    /// Ensure the configured model is downloaded, loaded on GPU, and warmed up.
    Prepare(AppConfig),
    /// Start capturing microphone audio and streaming live transcription.
    StartDictation(AppConfig),
    /// Pause microphone capture while keeping accumulated transcript & model state intact.
    PauseDictation,
    /// Resume microphone capture into the same dictation session.
    ResumeDictation,
    /// Stop capturing, flush trailing audio through the cache, and emit final transcript.
    StopDictation,
    /// Stop capturing immediately and discard transcript.
    CancelDictation,
    /// Shut down worker thread cleanly.
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum EngineEvent {
    Downloading(DownloadProgress),
    LoadingModel {
        gpu: bool,
        stage: String,
    },
    Ready {
        backend_label: String,
    },
    Listening {
        mic_name: String,
        gpu: bool,
    },
    Paused {
        mic_name: String,
    },
    TranscriptUpdate {
        text: String,
        #[allow(dead_code)]
        last_chunk_ms: u32,
    },
    Finalizing,
    Finished {
        text: String,
    },
    Cancelled,
    Error(String),
}

enum LoadedModel {
    Nemotron {
        model: Nemotron,
        chunk_samples: usize,
    },
    Unified {
        model: ParakeetUnified,
        chunk_samples: usize,
    },
    Eou {
        handle: ParakeetEOUHandle,
        session: ParakeetEOU,
        accumulated: String,
    },
}

struct ActiveModel {
    kind: ModelKind,
    gpu: bool,
    language: String,
    inner: LoadedModel,
}

impl ActiveModel {
    fn chunk_samples(&self) -> usize {
        match &self.inner {
            LoadedModel::Nemotron { chunk_samples, .. } => *chunk_samples,
            LoadedModel::Unified { chunk_samples, .. } => *chunk_samples,
            LoadedModel::Eou { .. } => EOU_CHUNK_SAMPLES,
        }
    }

    fn set_language(&mut self, lang: &str) {
        self.language = lang.to_string();
        if let LoadedModel::Nemotron { model, .. } = &mut self.inner {
            if model.mode() == NemotronMode::Multilingual {
                let _ = model.set_target_lang(lang);
            }
        }
    }

    fn reset(&mut self) {
        match &mut self.inner {
            LoadedModel::Nemotron { model, .. } => {
                model.reset();
                if model.mode() == NemotronMode::Multilingual {
                    let _ = model.set_target_lang(&self.language);
                }
            }
            LoadedModel::Unified { model, .. } => {
                model.reset();
            }
            LoadedModel::Eou {
                handle,
                session,
                accumulated,
            } => {
                *session = ParakeetEOU::from_shared(handle);
                accumulated.clear();
            }
        }
    }

    fn feed_chunk(&mut self, chunk: &[f32]) -> Result<String, String> {
        let need = self.chunk_samples();
        let owned;
        let slice = if chunk.len() < need {
            let mut padded = Vec::with_capacity(need);
            padded.extend_from_slice(chunk);
            padded.resize(need, 0.0);
            owned = padded;
            owned.as_slice()
        } else {
            &chunk[..need]
        };

        match &mut self.inner {
            LoadedModel::Nemotron { model, .. } => {
                model
                    .transcribe_chunk(slice)
                    .map_err(|e| format!("Nemotron GPU chunk failed: {e}"))?;
                Ok(clean_transcript(&model.get_transcript()))
            }
            LoadedModel::Unified { model, .. } => {
                model
                    .transcribe_chunk(slice)
                    .map_err(|e| format!("Parakeet Unified GPU chunk failed: {e}"))?;
                Ok(clean_transcript(&model.get_transcript()))
            }
            LoadedModel::Eou {
                session,
                accumulated,
                ..
            } => {
                let delta = session
                    .transcribe(slice, false)
                    .map_err(|e| format!("Parakeet EOU chunk failed: {e}"))?;
                if !delta.is_empty() {
                    accumulated.push_str(&delta);
                }
                Ok(clean_transcript(accumulated))
            }
        }
    }

    fn flush(&mut self) -> Result<String, String> {
        match &mut self.inner {
            LoadedModel::Unified { model, .. } => {
                let _ = model.flush();
                Ok(clean_transcript(&model.get_transcript()))
            }
            LoadedModel::Nemotron { .. } | LoadedModel::Eou { .. } => {
                let silence = vec![0.0f32; self.chunk_samples()];
                let mut latest = String::new();
                for _ in 0..3 {
                    latest = self.feed_chunk(&silence)?;
                }
                Ok(latest)
            }
        }
    }
}

fn clean_transcript(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_space = true;
    for ch in raw.chars() {
        // SentencePiece models (like Parakeet EOU) emit U+2581 '▁' for word boundaries.
        let ch = if ch == '\u{2581}' { ' ' } else { ch };
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

pub const GPU_BACKEND_LABEL: &str = if cfg!(target_os = "macos") {
    "GPU · Metal"
} else {
    "GPU · Vulkan"
};

fn build_exec_config(use_gpu: bool) -> (ExecutionConfig, bool, &'static str) {
    if use_gpu {
        (
            ExecutionConfig::new().with_execution_provider(ExecutionProvider::BurnWgpu),
            true,
            GPU_BACKEND_LABEL,
        )
    } else {
        (
            ExecutionConfig::new().with_execution_provider(ExecutionProvider::BurnCpu),
            false,
            "CPU · Burn",
        )
    }
}

fn load_and_warmup<F>(
    kind: ModelKind,
    language: &str,
    use_gpu: bool,
    mut on_stage: F,
) -> Result<(ActiveModel, &'static str), String>
where
    F: FnMut(&str, bool),
{
    let dir = model_dir(kind);
    let (exec_cfg, gpu, backend_label) = build_exec_config(use_gpu);

    on_stage(
        &format!(
            "Loading {} onto {}…",
            kind.display_name(),
            if gpu { "GPU" } else { "CPU" }
        ),
        gpu,
    );

    let load_res = load_model_inner(kind, &dir, language, exec_cfg);
    let (mut active, actual_gpu, actual_label) = match load_res {
        Ok(m) => (m, gpu, backend_label),
        Err(gpu_err) if use_gpu => {
            eprintln!("voicebar: GPU load failed ({gpu_err}), falling back to CPU");
            on_stage("GPU unavailable — falling back to CPU…", false);
            let (cpu_cfg, _, cpu_label) = build_exec_config(false);
            let m = load_model_inner(kind, &dir, language, cpu_cfg).map_err(|cpu_err| {
                format!("GPU load failed ({gpu_err}); CPU fallback failed ({cpu_err})")
            })?;
            (m, false, cpu_label)
        }
        Err(e) => return Err(e),
    };

    // Warm up fixed-size streaming kernels once so the first spoken word is instant.
    on_stage(
        if actual_gpu {
            "Warming Vulkan shaders on GPU…"
        } else {
            "Warming streaming engine…"
        },
        actual_gpu,
    );
    let silence = vec![0.0f32; active.chunk_samples()];
    let _ = active.feed_chunk(&silence);
    active.reset();

    Ok((active, actual_label))
}

fn load_model_inner(
    kind: ModelKind,
    dir: &Path,
    language: &str,
    exec_cfg: ExecutionConfig,
) -> Result<ActiveModel, String> {
    let gpu = exec_cfg.execution_provider == ExecutionProvider::BurnWgpu;
    match kind {
        ModelKind::Nemotron35 | ModelKind::NemotronEn => {
            let mut model = Nemotron::from_pretrained(dir, Some(exec_cfg))
                .map_err(|e| format!("Failed to load {} from {}: {e}", kind.display_name(), dir.display()))?;
            if model.mode() == NemotronMode::Multilingual {
                model
                    .set_target_lang(language)
                    .map_err(|e| format!("Invalid language '{language}': {e}"))?;
            }
            let chunk_samples = model.chunk_samples();
            Ok(ActiveModel {
                kind,
                gpu,
                language: language.to_string(),
                inner: LoadedModel::Nemotron {
                    model,
                    chunk_samples,
                },
            })
        }
        ModelKind::ParakeetUnifiedEn => {
            let model = ParakeetUnified::from_pretrained(dir, Some(exec_cfg))
                .map_err(|e| format!("Failed to load Parakeet Unified from {}: {e}", dir.display()))?;
            let chunk_samples = model.streaming_config().chunk_samples();
            Ok(ActiveModel {
                kind,
                gpu,
                language: "en-US".to_string(),
                inner: LoadedModel::Unified {
                    model,
                    chunk_samples,
                },
            })
        }
        ModelKind::ParakeetEou => {
            let handle = ParakeetEOUHandle::load(dir, Some(exec_cfg))
                .map_err(|e| format!("Failed to load Parakeet EOU from {}: {e}", dir.display()))?;
            let session = ParakeetEOU::from_shared(&handle);
            Ok(ActiveModel {
                kind,
                gpu,
                language: "en-US".to_string(),
                inner: LoadedModel::Eou {
                    handle,
                    session,
                    accumulated: String::new(),
                },
            })
        }
    }
}

pub struct EngineHandle {
    pub tx: Sender<EngineCommand>,
    pub rx: Receiver<EngineEvent>,
    pub audio: AudioShared,
}

impl EngineHandle {
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<EngineCommand>();
        let (ev_tx, ev_rx) = mpsc::channel::<EngineEvent>();
        let audio = AudioShared::new();
        let audio_bg = audio.clone();

        thread::Builder::new()
            .name("voicebar-asr".into())
            .spawn(move || worker_loop(cmd_rx, ev_tx, audio_bg))
            .expect("spawn voicebar-asr thread");

        Self {
            tx: cmd_tx,
            rx: ev_rx,
            audio,
        }
    }
}

fn ensure_loaded(
    loaded: &mut Option<ActiveModel>,
    cfg: &AppConfig,
    ev_tx: &Sender<EngineEvent>,
) -> Result<(), String> {
    let needs_reload = match loaded {
        Some(m) => m.kind != cfg.model || m.gpu != cfg.use_gpu,
        None => true,
    };

    if !needs_reload {
        if let Some(m) = loaded.as_mut() {
            if m.language != cfg.language {
                m.set_language(&cfg.language);
            }
        }
        return Ok(());
    }

    // Drop previous model first to free VRAM before loading a new one.
    *loaded = None;

    if !is_model_ready(cfg.model) {
        ensure_model_downloaded(cfg.model, |progress| {
            let _ = ev_tx.send(EngineEvent::Downloading(progress));
        })?;
    }

    let (active, backend_label) = load_and_warmup(cfg.model, &cfg.language, cfg.use_gpu, |stage, gpu| {
        let _ = ev_tx.send(EngineEvent::LoadingModel {
            gpu,
            stage: stage.to_string(),
        });
    })?;

    *loaded = Some(active);
    let _ = ev_tx.send(EngineEvent::Ready {
        backend_label: backend_label.to_string(),
    });
    Ok(())
}

fn worker_loop(
    cmd_rx: Receiver<EngineCommand>,
    ev_tx: Sender<EngineEvent>,
    audio: AudioShared,
) {
    let mut loaded: Option<ActiveModel> = None;
    #[allow(unused_assignments)]
    let mut _mic: Option<MicCapture> = None;
    let mut dictating = false;
    let mut paused = false;
    let mut last_mic_name = String::new();
    let mut latest_text = String::new();

    loop {
        let poll_timeout = if dictating {
            Duration::from_millis(15)
        } else {
            Duration::from_millis(100)
        };

        match cmd_rx.recv_timeout(poll_timeout) {
            Ok(EngineCommand::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(EngineCommand::Prepare(cfg)) => {
                if dictating || paused {
                    _mic = None;
                    dictating = false;
                    paused = false;
                }
                if let Err(e) = ensure_loaded(&mut loaded, &cfg, &ev_tx) {
                    let _ = ev_tx.send(EngineEvent::Error(e));
                }
            }
            Ok(EngineCommand::StartDictation(cfg)) => {
                if dictating || paused {
                    continue;
                }
                if let Err(e) = ensure_loaded(&mut loaded, &cfg, &ev_tx) {
                    let _ = ev_tx.send(EngineEvent::Error(e));
                    continue;
                }
                // Check if user pressed F1/cancel while the model was downloading or loading.
                let mut aborted = false;
                while let Ok(pending) = cmd_rx.try_recv() {
                    match pending {
                        EngineCommand::StopDictation | EngineCommand::CancelDictation => {
                            aborted = true;
                        }
                        EngineCommand::Shutdown => return,
                        _ => {}
                    }
                }
                if aborted {
                    let _ = ev_tx.send(EngineEvent::Cancelled);
                    continue;
                }

                audio.clear();
                latest_text.clear();
                if let Some(m) = loaded.as_mut() {
                    m.reset();
                }

                match MicCapture::start(audio.clone()) {
                    Ok(capture) => {
                        let mic_name = capture.device_name.clone();
                        last_mic_name = mic_name.clone();
                        _mic = Some(capture);
                        dictating = true;
                        paused = false;
                        let gpu = loaded.as_ref().map(|m| m.gpu).unwrap_or(false);
                        let _ = ev_tx.send(EngineEvent::Listening { mic_name, gpu });
                    }
                    Err(e) => {
                        let _ = ev_tx.send(EngineEvent::Error(e));
                    }
                }
            }
            Ok(EngineCommand::PauseDictation) => {
                if !dictating || paused {
                    continue;
                }
                _mic = None;
                dictating = false;
                paused = true;
                // Process any buffered samples captured just before Pause without resetting the model.
                if let Some(m) = loaded.as_mut() {
                    let tail = audio.drain_all();
                    let step = m.chunk_samples();
                    for piece in tail.chunks(step) {
                        if piece.len() >= 160 {
                            if let Ok(t) = m.feed_chunk(piece) {
                                latest_text = t.clone();
                                let _ = ev_tx.send(EngineEvent::TranscriptUpdate {
                                    text: t,
                                    last_chunk_ms: 0,
                                });
                            }
                        }
                    }
                }
                audio.clear();
                let _ = ev_tx.send(EngineEvent::Paused {
                    mic_name: last_mic_name.clone(),
                });
            }
            Ok(EngineCommand::ResumeDictation) => {
                if !paused || dictating {
                    continue;
                }
                audio.clear();
                match MicCapture::start(audio.clone()) {
                    Ok(capture) => {
                        let mic_name = capture.device_name.clone();
                        last_mic_name = mic_name.clone();
                        _mic = Some(capture);
                        dictating = true;
                        paused = false;
                        let gpu = loaded.as_ref().map(|m| m.gpu).unwrap_or(false);
                        let _ = ev_tx.send(EngineEvent::Listening { mic_name, gpu });
                    }
                    Err(e) => {
                        paused = false;
                        let _ = ev_tx.send(EngineEvent::Error(e));
                    }
                }
            }
            Ok(EngineCommand::CancelDictation) => {
                _mic = None;
                dictating = false;
                paused = false;
                audio.clear();
                latest_text.clear();
                if let Some(m) = loaded.as_mut() {
                    m.reset();
                }
                let _ = ev_tx.send(EngineEvent::Cancelled);
            }
            Ok(EngineCommand::StopDictation) => {
                if !dictating && !paused {
                    continue;
                }
                _mic = None;
                dictating = false;
                paused = false;
                let _ = ev_tx.send(EngineEvent::Finalizing);

                if let Some(m) = loaded.as_mut() {
                    let tail = audio.drain_all();
                    let step = m.chunk_samples();
                    for piece in tail.chunks(step) {
                        if piece.len() >= 160 {
                            if let Ok(t) = m.feed_chunk(piece) {
                                latest_text = t;
                            }
                        }
                    }
                    if let Ok(flushed) = m.flush() {
                        if !flushed.is_empty() {
                            latest_text = flushed;
                        }
                    }
                    m.reset();
                }
                audio.clear();
                let final_text = latest_text.trim().to_string();
                let _ = ev_tx.send(EngineEvent::Finished { text: final_text });
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }

        if dictating {
            if let Some(m) = loaded.as_mut() {
                let need = m.chunk_samples();
                while let Some(chunk) = audio.pop_chunk(need) {
                    let t0 = Instant::now();
                    match m.feed_chunk(&chunk) {
                        Ok(text) => {
                            let ms = t0.elapsed().as_millis() as u32;
                            latest_text = text.clone();
                            let _ = ev_tx.send(EngineEvent::TranscriptUpdate {
                                text,
                                last_chunk_ms: ms,
                            });
                        }
                        Err(e) => {
                            _mic = None;
                            dictating = false;
                            paused = false;
                            let _ = ev_tx.send(EngineEvent::Error(e));
                            break;
                        }
                    }
                    // Yield immediately if the user hit Pause/Stop/Cancel mid-drain.
                    if let Ok(cmd) = cmd_rx.try_recv() {
                        match cmd {
                            EngineCommand::PauseDictation => {
                                _mic = None;
                                dictating = false;
                                paused = true;
                                audio.clear();
                                let _ = ev_tx.send(EngineEvent::Paused {
                                    mic_name: last_mic_name.clone(),
                                });
                                break;
                            }
                            EngineCommand::CancelDictation => {
                                _mic = None;
                                dictating = false;
                                paused = false;
                                audio.clear();
                                latest_text.clear();
                                m.reset();
                                let _ = ev_tx.send(EngineEvent::Cancelled);
                                break;
                            }
                            EngineCommand::StopDictation => {
                                _mic = None;
                                dictating = false;
                                paused = false;
                                let _ = ev_tx.send(EngineEvent::Finalizing);
                                let tail = audio.drain_all();
                                for piece in tail.chunks(need) {
                                    if piece.len() >= 160 {
                                        if let Ok(t) = m.feed_chunk(piece) {
                                            latest_text = t;
                                        }
                                    }
                                }
                                if let Ok(flushed) = m.flush() {
                                    if !flushed.is_empty() {
                                        latest_text = flushed;
                                    }
                                }
                                m.reset();
                                audio.clear();
                                let _ = ev_tx.send(EngineEvent::Finished {
                                    text: latest_text.trim().to_string(),
                                });
                                break;
                            }
                            EngineCommand::Shutdown => return,
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}

/// CLI helper: benchmark streaming GPU inference against a 16 kHz WAV file.
pub fn run_wav_benchmark(cfg: &AppConfig, wav_path: &Path) -> Result<(), String> {
    if !is_model_ready(cfg.model) {
        eprintln!("Downloading {}…", cfg.model.display_name());
        ensure_model_downloaded(cfg.model, |p| {
            eprint!(
                "\r  {}  {:.1} / {:.1} MB ({:.0}%)  {:.1} MB/s   ",
                p.file_name,
                p.downloaded_bytes as f64 / 1_048_576.0,
                p.total_bytes as f64 / 1_048_576.0,
                p.fraction() * 100.0,
                p.mb_per_sec
            );
        })?;
        eprintln!();
    }

    let t_load = Instant::now();
    let (mut active, backend_label) =
        load_and_warmup(cfg.model, &cfg.language, cfg.use_gpu, |stage, _| {
            eprintln!("  {stage}");
        })?;
    eprintln!(
        "Loaded {} on {} in {:.2}s (chunk = {} samples / {} ms)",
        cfg.model.display_name(),
        backend_label,
        t_load.elapsed().as_secs_f32(),
        active.chunk_samples(),
        active.chunk_samples() * 1000 / 16_000
    );

    let mut reader = hound::WavReader::open(wav_path)
        .map_err(|e| format!("Cannot open WAV {}: {e}", wav_path.display()))?;
    let spec = reader.spec();
    let mut audio: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("WAV read error: {e}"))?,
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / 32768.0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("WAV read error: {e}"))?,
    };
    if spec.channels > 1 {
        let ch = spec.channels as usize;
        audio = audio
            .chunks(ch)
            .map(|f| f.iter().sum::<f32>() / f.len() as f32)
            .collect();
    }
    if spec.sample_rate != 16_000 {
        let step = spec.sample_rate as f64 / 16_000.0;
        let out_len = (audio.len() as f64 / step).floor() as usize;
        let mut resampled = Vec::with_capacity(out_len);
        for i in 0..out_len {
            let pos = i as f64 * step;
            let idx = (pos.floor() as usize).min(audio.len() - 1);
            let next = (idx + 1).min(audio.len() - 1);
            let frac = (pos - idx as f64) as f32;
            resampled.push(audio[idx] + (audio[next] - audio[idx]) * frac);
        }
        audio = resampled;
    }

    let audio_secs = audio.len() as f32 / 16_000.0;
    let step = active.chunk_samples();
    let t_run = Instant::now();
    let mut chunks = 0u32;
    let mut last_printed = String::new();

    for piece in audio.chunks(step) {
        let t0 = Instant::now();
        let text = active.feed_chunk(piece)?;
        let ms = t0.elapsed().as_millis();
        chunks += 1;
        if text != last_printed {
            eprintln!("  [chunk {chunks:02} · {ms:3} ms] {text}");
            last_printed = text;
        }
    }
    let final_text = active.flush()?;
    let elapsed = t_run.elapsed().as_secs_f32();
    println!("\nFinal transcript: {final_text}");
    println!(
        "Audio: {audio_secs:.2}s | Compute: {elapsed:.2}s | Speed: {:.1}x real-time ({backend_label})",
        audio_secs / elapsed.max(0.001)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::clean_transcript;

    #[test]
    fn normalizes_sentencepiece_spaces_and_whitespace() {
        assert_eq!(
            clean_transcript("▁hello▁world▁▁from\tvoicebar\n"),
            "hello world from voicebar"
        );
        assert_eq!(
            clean_transcript("  Hello,   this is Voicebar.  "),
            "Hello, this is Voicebar."
        );
    }
}
