//! Persistent user settings, portable mode support, Windows startup toggle,
//! and Hugging Face model manifests.

use crate::personalization::RewriteProfile;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

const CURRENT_CONFIG_VERSION: u32 = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ModelKind {
    /// NVIDIA Nemotron 3.5 ASR Streaming 0.6B: clean punctuation, capitalization, removes um/uh,
    /// 40 languages (locked to English en-US by default so Indian accents never drift to Hindi).
    #[default]
    Nemotron35,
    /// NVIDIA Nemotron Speech Streaming EN 0.6B: dedicated English-only specialist with native
    /// punctuation & capitalization (physically incapable of outputting non-English scripts).
    NemotronEn,
    /// NVIDIA Parakeet Unified EN 0.6B: NVIDIA's newest unified English streaming model with
    /// native punctuation & capitalization.
    ParakeetUnifiedEn,
    /// NVIDIA Parakeet Realtime EOU 120M: instant 0.8s cold start, ultra-fast 160 ms chunks,
    /// lightweight (~460 MB), English-only.
    ParakeetEou,
}

pub struct ModelFileSpec {
    pub remote_path: &'static str,
    pub local_name: &'static str,
    pub approx_bytes: u64,
}

impl ModelKind {
    pub const ALL: [ModelKind; 4] = [
        ModelKind::Nemotron35,
        ModelKind::NemotronEn,
        ModelKind::ParakeetUnifiedEn,
        ModelKind::ParakeetEou,
    ];

    pub fn id(self) -> &'static str {
        match self {
            ModelKind::Nemotron35 => "nemotron-3.5-asr-streaming-0.6b-onnx",
            ModelKind::NemotronEn => "nemotron-speech-streaming-en-0.6b",
            ModelKind::ParakeetUnifiedEn => "parakeet-unified-en-0.6b",
            ModelKind::ParakeetEou => "realtime_eou_120m-v1-onnx",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ModelKind::Nemotron35 => "Nemotron 3.5 · 0.6B",
            ModelKind::NemotronEn => "Nemotron EN · 0.6B",
            ModelKind::ParakeetUnifiedEn => "Parakeet Unified · 0.6B",
            ModelKind::ParakeetEou => "Parakeet EOU · 120M",
        }
    }

    pub fn short_badge(self) -> &'static str {
        match self {
            ModelKind::Nemotron35 => "Nemotron 3.5",
            ModelKind::NemotronEn => "Nemotron EN",
            ModelKind::ParakeetUnifiedEn => "Parakeet Unified",
            ModelKind::ParakeetEou => "Parakeet 120M",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            ModelKind::Nemotron35 => "Punctuation · removes um/uh · EN locked (40 langs) · 560ms",
            ModelKind::NemotronEn => "100% English-only · punctuation · zero Hindi drift · 560ms",
            ModelKind::ParakeetUnifiedEn => {
                "Newest unified English · punctuation · high accuracy · 560ms"
            }
            ModelKind::ParakeetEou => "Instant 0.8s startup · ultra-fast 160ms chunks · English",
        }
    }

    pub fn size_label(self) -> &'static str {
        match self {
            ModelKind::Nemotron35 => "2.5 GB",
            ModelKind::NemotronEn => "2.4 GB",
            ModelKind::ParakeetUnifiedEn => "2.4 GB",
            ModelKind::ParakeetEou => "460 MB",
        }
    }

    pub fn supports_language_choice(self) -> bool {
        matches!(self, ModelKind::Nemotron35)
    }

    pub fn files(self) -> &'static [ModelFileSpec] {
        match self {
            ModelKind::Nemotron35 => &[
                ModelFileSpec {
                    remote_path: "nemotron-3.5-asr-streaming-0.6b-onnx/config.json",
                    local_name: "config.json",
                    approx_bytes: 2_995,
                },
                ModelFileSpec {
                    remote_path: "nemotron-3.5-asr-streaming-0.6b-onnx/tokenizer.model",
                    local_name: "tokenizer.model",
                    approx_bytes: 406_554,
                },
                ModelFileSpec {
                    remote_path: "nemotron-3.5-asr-streaming-0.6b-onnx/encoder.onnx",
                    local_name: "encoder.onnx",
                    approx_bytes: 4_161_952,
                },
                ModelFileSpec {
                    remote_path: "nemotron-3.5-asr-streaming-0.6b-onnx/decoder_joint.onnx",
                    local_name: "decoder_joint.onnx",
                    approx_bytes: 97_653_235,
                },
                ModelFileSpec {
                    remote_path: "nemotron-3.5-asr-streaming-0.6b-onnx/encoder.onnx.data",
                    local_name: "encoder.onnx.data",
                    approx_bytes: 2_495_361_024,
                },
            ],
            ModelKind::NemotronEn => &[
                ModelFileSpec {
                    remote_path: "nemotron-speech-streaming-en-0.6b/config.json",
                    local_name: "config.json",
                    approx_bytes: 980,
                },
                ModelFileSpec {
                    remote_path: "nemotron-speech-streaming-en-0.6b/tokenizer.model",
                    local_name: "tokenizer.model",
                    approx_bytes: 251_056,
                },
                ModelFileSpec {
                    remote_path: "nemotron-speech-streaming-en-0.6b/encoder.onnx",
                    local_name: "encoder.onnx",
                    approx_bytes: 4_026_060,
                },
                ModelFileSpec {
                    remote_path: "nemotron-speech-streaming-en-0.6b/decoder_joint.onnx",
                    local_name: "decoder_joint.onnx",
                    approx_bytes: 35_842_420,
                },
                ModelFileSpec {
                    remote_path: "nemotron-speech-streaming-en-0.6b/encoder.onnx.data",
                    local_name: "encoder.onnx.data",
                    approx_bytes: 2_477_522_944,
                },
            ],
            ModelKind::ParakeetUnifiedEn => &[
                ModelFileSpec {
                    remote_path: "parakeet-unified-en-0.6b/vocab.txt",
                    local_name: "vocab.txt",
                    approx_bytes: 4_929,
                },
                ModelFileSpec {
                    remote_path: "parakeet-unified-en-0.6b/tokenizer.model",
                    local_name: "tokenizer.model",
                    approx_bytes: 251_056,
                },
                ModelFileSpec {
                    remote_path: "parakeet-unified-en-0.6b/encoder.onnx",
                    local_name: "encoder.onnx",
                    approx_bytes: 4_137_221,
                },
                ModelFileSpec {
                    remote_path: "parakeet-unified-en-0.6b/decoder_joint.onnx",
                    local_name: "decoder_joint.onnx",
                    approx_bytes: 35_842_420,
                },
                ModelFileSpec {
                    remote_path: "parakeet-unified-en-0.6b/encoder.onnx.data",
                    local_name: "encoder.onnx.data",
                    approx_bytes: 2_478_047_232,
                },
            ],
            ModelKind::ParakeetEou => &[
                ModelFileSpec {
                    remote_path: "realtime_eou_120m-v1-onnx/tokenizer.json",
                    local_name: "tokenizer.json",
                    approx_bytes: 89_339,
                },
                ModelFileSpec {
                    remote_path: "realtime_eou_120m-v1-onnx/decoder_joint.onnx",
                    local_name: "decoder_joint.onnx",
                    approx_bytes: 21_379_651,
                },
                ModelFileSpec {
                    remote_path: "realtime_eou_120m-v1-onnx/encoder.onnx",
                    local_name: "encoder.onnx",
                    approx_bytes: 461_446_924,
                },
            ],
        }
    }

    pub fn total_approx_bytes(self) -> u64 {
        self.files().iter().map(|f| f.approx_bytes).sum()
    }

    pub fn hf_url(remote_path: &str) -> String {
        format!("https://huggingface.co/altunenes/parakeet-rs/resolve/main/{remote_path}")
    }
}

/// Optional built-in `llama.cpp` text-only GGUF model used when Optional Rephrasing is ON.
/// Zero Ollama required — VoiceBar downloads the verified GGUF directly from Hugging Face
/// and runs it locally on Vulkan / Metal / CPU via its managed `llama-server`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RewriteModel {
    /// Sarvam Translate 4B (Gemma-3 4B GGUF): 22 Indian languages + English + code-mixing (~2.37 GB).
    #[default]
    SarvamTranslate4b,
    /// Qwen 3 0.6B (text-only GGUF): ultra-tiny (~378 MB), instant on any CPU or low-VRAM GPU.
    Qwen3_0_6b,
    /// Qwen 2.5 1.5B Instruct (text-only GGUF, zero thinking): fast small English & Hinglish (~1.07 GB).
    Qwen25_1_5b,
    /// Qwen 3 1.7B (text-only GGUF): fast low-VRAM cleanup across 119 languages (~1.06 GB).
    Qwen3_1_7b,
    /// Qwen 2.5 3B Instruct (text-only GGUF): snappy Hinglish & Indian English (~2.01 GB).
    Qwen25_3b,
    /// Qwen 3 4B (text-only GGUF): balanced English, Hindi & Hinglish cleanup (~2.38 GB).
    Qwen3_4b,
    /// Sarvam M 24B (Mistral 24B GGUF, Apache 2.0): full Indian languages + English (16 GB+ RAM/VRAM).
    SarvamM24b,
}

impl RewriteModel {
    pub const ALL: [RewriteModel; 7] = [
        RewriteModel::SarvamTranslate4b,
        RewriteModel::Qwen3_0_6b,
        RewriteModel::Qwen25_1_5b,
        RewriteModel::Qwen3_1_7b,
        RewriteModel::Qwen25_3b,
        RewriteModel::Qwen3_4b,
        RewriteModel::SarvamM24b,
    ];

    pub fn gguf_filename(self) -> &'static str {
        match self {
            RewriteModel::SarvamTranslate4b => "sarvam-translate.Q4_K_M.gguf",
            RewriteModel::Qwen3_0_6b => "Qwen3-0.6B-Q4_K_M.gguf",
            RewriteModel::Qwen25_1_5b => "qwen2.5-1.5b-instruct-q4_k_m.gguf",
            RewriteModel::Qwen3_1_7b => "Qwen3-1.7B-Q4_K_M.gguf",
            RewriteModel::Qwen25_3b => "qwen2.5-3b-instruct-q4_k_m.gguf",
            RewriteModel::Qwen3_4b => "Qwen3-4B-Q4_K_M.gguf",
            RewriteModel::SarvamM24b => "sarvamai_sarvam-m-Q4_K_M.gguf",
        }
    }

    pub fn hf_download_url(self) -> &'static str {
        match self {
            RewriteModel::SarvamTranslate4b => {
                "https://huggingface.co/mradermacher/sarvam-translate-GGUF/resolve/main/sarvam-translate.Q4_K_M.gguf"
            }
            RewriteModel::Qwen3_0_6b => {
                "https://huggingface.co/unsloth/Qwen3-0.6B-GGUF/resolve/main/Qwen3-0.6B-Q4_K_M.gguf"
            }
            RewriteModel::Qwen25_1_5b => {
                "https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/main/qwen2.5-1.5b-instruct-q4_k_m.gguf"
            }
            RewriteModel::Qwen3_1_7b => {
                "https://huggingface.co/unsloth/Qwen3-1.7B-GGUF/resolve/main/Qwen3-1.7B-Q4_K_M.gguf"
            }
            RewriteModel::Qwen25_3b => {
                "https://huggingface.co/Qwen/Qwen2.5-3B-Instruct-GGUF/resolve/main/qwen2.5-3b-instruct-q4_k_m.gguf"
            }
            RewriteModel::Qwen3_4b => {
                "https://huggingface.co/Qwen/Qwen3-4B-GGUF/resolve/main/Qwen3-4B-Q4_K_M.gguf"
            }
            RewriteModel::SarvamM24b => {
                "https://huggingface.co/bartowski/sarvamai_sarvam-m-GGUF/resolve/main/sarvamai_sarvam-m-Q4_K_M.gguf"
            }
        }
    }

    pub fn approx_bytes(self) -> u64 {
        match self {
            RewriteModel::SarvamTranslate4b => 2_489_800_000,
            RewriteModel::Qwen3_0_6b => 396_700_000,
            RewriteModel::Qwen25_1_5b => 1_117_300_000,
            RewriteModel::Qwen3_1_7b => 1_107_400_000,
            RewriteModel::Qwen25_3b => 2_104_900_000,
            RewriteModel::Qwen3_4b => 2_497_300_000,
            RewriteModel::SarvamM24b => 14_333_900_000,
        }
    }

    pub fn supports_thinking(self) -> bool {
        matches!(
            self,
            RewriteModel::Qwen3_0_6b | RewriteModel::Qwen3_1_7b | RewriteModel::Qwen3_4b
        )
    }

    pub fn display_name(self) -> &'static str {
        match self {
            RewriteModel::SarvamTranslate4b => "Sarvam Translate · 4B",
            RewriteModel::Qwen3_0_6b => "Qwen 3 · 0.6B",
            RewriteModel::Qwen25_1_5b => "Qwen 2.5 · 1.5B",
            RewriteModel::Qwen3_1_7b => "Qwen 3 · 1.7B",
            RewriteModel::Qwen25_3b => "Qwen 2.5 · 3B",
            RewriteModel::Qwen3_4b => "Qwen 3 · 4B",
            RewriteModel::SarvamM24b => "Sarvam M · 24B",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            RewriteModel::SarvamTranslate4b => "Sarvam 4B",
            RewriteModel::Qwen3_0_6b => "Qwen 3 0.6B",
            RewriteModel::Qwen25_1_5b => "Qwen 2.5 1.5B",
            RewriteModel::Qwen3_1_7b => "Qwen 3 1.7B",
            RewriteModel::Qwen25_3b => "Qwen 2.5 3B",
            RewriteModel::Qwen3_4b => "Qwen 3 4B",
            RewriteModel::SarvamM24b => "Sarvam M 24B",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            RewriteModel::SarvamTranslate4b => {
                "Sarvam AI · Gemma-3 4B · 22 Indian languages + English & code-mixing"
            }
            RewriteModel::Qwen3_0_6b => {
                "Ultra-tiny · instant on CPU or low-VRAM GPU · zero Ollama needed"
            }
            RewriteModel::Qwen25_1_5b => {
                "Small & snappy · English & Hinglish cleanup · zero thinking delay"
            }
            RewriteModel::Qwen3_1_7b => "Fast low-VRAM text cleanup · 119 languages",
            RewriteModel::Qwen25_3b => "Instant Hinglish & Indian English · zero thinking",
            RewriteModel::Qwen3_4b => "Balanced English, Hindi & Hinglish cleanup · 8 GB GPU",
            RewriteModel::SarvamM24b => "Sarvam AI · Mistral 24B · Indic & English · 16 GB+ RAM",
        }
    }

    pub fn size_label(self) -> &'static str {
        match self {
            RewriteModel::SarvamTranslate4b => "2.4 GB",
            RewriteModel::Qwen3_0_6b => "378 MB",
            RewriteModel::Qwen25_1_5b => "1.07 GB",
            RewriteModel::Qwen3_1_7b => "1.06 GB",
            RewriteModel::Qwen25_3b => "2.0 GB",
            RewriteModel::Qwen3_4b => "2.4 GB",
            RewriteModel::SarvamM24b => "13.7 GB",
        }
    }

    pub fn is_small_fast(self) -> bool {
        matches!(self, RewriteModel::Qwen3_0_6b | RewriteModel::Qwen25_1_5b)
    }

    pub fn is_indic_focused(self) -> bool {
        matches!(
            self,
            RewriteModel::SarvamTranslate4b
                | RewriteModel::SarvamM24b
                | RewriteModel::Qwen25_3b
                | RewriteModel::Qwen25_1_5b
        )
    }

    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|&m| m == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub config_version: u32,
    #[serde(default)]
    pub model: ModelKind,
    #[serde(default = "default_lang")]
    pub language: String,
    #[serde(default = "default_use_gpu")]
    pub use_gpu: bool,
    #[serde(default = "default_auto_paste")]
    pub auto_paste: bool,
    #[serde(default)]
    pub personal_rewrite: bool,
    #[serde(default)]
    pub rewrite_model: RewriteModel,
    #[serde(default)]
    pub rewrite_language: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rewrite_profile: Option<RewriteProfile>,
    #[serde(default)]
    pub legacy_rewrite: bool,
}

fn default_lang() -> String {
    "en-US".to_string()
}

fn default_use_gpu() -> bool {
    true
}

fn default_auto_paste() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            config_version: CURRENT_CONFIG_VERSION,
            model: ModelKind::Nemotron35,
            language: default_lang(),
            use_gpu: true,
            auto_paste: true,
            personal_rewrite: false,
            rewrite_model: RewriteModel::SarvamTranslate4b,
            rewrite_language: String::new(),
            rewrite_profile: None,
            legacy_rewrite: false,
        }
    }
}

/// Supports portable mode: if `portable.txt` or `voicebar-data/` exists beside `voicebar`,
/// config and models stay portable next to the binary; otherwise uses the OS app data directory.
pub fn app_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let portable_data = dir.join("voicebar-data");
            if dir.join("portable.txt").is_file() || portable_data.is_dir() {
                return portable_data;
            }
        }
    }
    #[cfg(windows)]
    {
        if let Ok(base) = std::env::var("LOCALAPPDATA") {
            return PathBuf::from(base).join("voicebar");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("voicebar");
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("voicebar");
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".local").join("share").join("voicebar");
        }
    }
    if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        return PathBuf::from(home).join(".voicebar");
    }
    PathBuf::from(".voicebar")
}

pub fn models_root() -> PathBuf {
    app_dir().join("models")
}

pub fn llm_models_dir() -> PathBuf {
    models_root().join("llm")
}

pub fn llm_model_path(model: RewriteModel) -> PathBuf {
    llm_models_dir().join(model.gguf_filename())
}

pub fn is_rewrite_model_downloaded(model: RewriteModel) -> bool {
    let p = llm_model_path(model);
    match std::fs::metadata(&p) {
        Ok(m) => m.is_file() && m.len() >= (model.approx_bytes() / 2).max(1_000_000),
        Err(_) => false,
    }
}

pub fn llama_runtime_dir() -> PathBuf {
    app_dir().join("runtime").join("llama-cpp")
}

pub fn model_dir(kind: ModelKind) -> PathBuf {
    models_root().join(kind.id())
}

pub fn is_model_ready(kind: ModelKind) -> bool {
    let dir = model_dir(kind);
    kind.files().iter().all(|spec| {
        let p = dir.join(spec.local_name);
        match std::fs::metadata(&p) {
            Ok(m) => m.is_file() && m.len() >= (spec.approx_bytes / 2).max(1),
            Err(_) => false,
        }
    })
}

pub fn config_path() -> PathBuf {
    app_dir().join("config.json")
}

impl AppConfig {
    pub fn has_valid_profile(&self) -> bool {
        self.rewrite_profile
            .as_ref()
            .is_some_and(RewriteProfile::is_valid)
    }

    fn decode(raw: &str) -> serde_json::Result<Self> {
        let mut config: Self = serde_json::from_str(raw)?;
        if config.config_version < 2 && config.language == "auto" {
            config.language = "en-US".to_string();
        }
        // Discard invalid or manually corrupted profile data so UI and rewriter always agree.
        if config
            .rewrite_profile
            .as_ref()
            .is_some_and(|profile| !profile.is_valid())
        {
            config.rewrite_profile = None;
        }
        // Keep the old model for installations that had explicitly enabled it.
        // Only the setup flow can replace it with a newly approved profile.
        if config.config_version < 3 && config.personal_rewrite && config.rewrite_profile.is_none()
        {
            config.legacy_rewrite = true;
        }
        config.config_version = CURRENT_CONFIG_VERSION;
        Ok(config)
    }

    pub fn activate_profile(&mut self, profile: RewriteProfile) -> io::Result<()> {
        self.activate_at(profile, &config_path())
    }

    pub fn clear_profile(&mut self) -> io::Result<()> {
        self.clear_at(&config_path())
    }

    fn clear_at(&mut self, path: &Path) -> io::Result<()> {
        let mut cleared = self.clone();
        cleared.rewrite_profile = None;
        cleared.legacy_rewrite = false;
        cleared.personal_rewrite = false;
        cleared.save_to(path)?;
        *self = cleared;
        Ok(())
    }

    fn activate_at(&mut self, profile: RewriteProfile, path: &Path) -> io::Result<()> {
        if !profile.is_valid() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid rewrite profile",
            ));
        }
        let mut activated = self.clone();
        activated.rewrite_profile = Some(profile);
        activated.personal_rewrite = true;
        activated.legacy_rewrite = false;
        activated.save_to(path)?;
        *self = activated;
        Ok(())
    }

    pub fn load() -> Self {
        std::fs::read_to_string(config_path())
            .ok()
            .and_then(|raw| Self::decode(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = self.save_to(&config_path());
    }

    fn save_to(&self, path: &Path) -> io::Result<()> {
        let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        std::fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(temp.as_file_mut(), self)?;
        temp.as_file().sync_all()?;
        temp.persist(path).map_err(|failure| failure.error)?;
        Ok(())
    }
}

pub fn open_folder(path: &Path) {
    let _ = std::fs::create_dir_all(path);
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}

#[cfg(windows)]
pub fn is_autostart_enabled() -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "Voicebar",
        ])
        .creation_flags(0x0800_0000)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(windows)]
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    let status = if enabled {
        let exe = std::env::current_exe()
            .map_err(|e| format!("Cannot resolve voicebar.exe path: {e}"))?;
        let val = format!("\"{}\"", exe.display());
        std::process::Command::new("reg")
            .args([
                "add", key, "/v", "Voicebar", "/t", "REG_SZ", "/d", &val, "/f",
            ])
            .creation_flags(0x0800_0000)
            .status()
    } else {
        std::process::Command::new("reg")
            .args(["delete", key, "/v", "Voicebar", "/f"])
            .creation_flags(0x0800_0000)
            .status()
    }
    .map_err(|e| format!("Failed to update Windows startup registry: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err("Windows registry update returned non-zero status".into())
    }
}

#[cfg(target_os = "macos")]
fn launch_agent_path() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(|h| {
        PathBuf::from(h)
            .join("Library")
            .join("LaunchAgents")
            .join("dev.voicebar.app.plist")
    })
}

#[cfg(target_os = "macos")]
pub fn is_autostart_enabled() -> bool {
    launch_agent_path().map(|p| p.is_file()).unwrap_or(false)
}

#[cfg(target_os = "macos")]
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let path = launch_agent_path().ok_or_else(|| "HOME is not set".to_string())?;
    if !enabled {
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| format!("Failed to remove LaunchAgent: {e}"))?;
        }
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| format!("Cannot resolve voicebar path: {e}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Cannot create LaunchAgents: {e}"))?;
    }
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>dev.voicebar.app</string>
  <key>ProgramArguments</key><array><string>{}</string></array>
  <key>RunAtLoad</key><true/>
</dict></plist>
"#,
        exe.display()
    );
    std::fs::write(&path, plist).map_err(|e| format!("Failed to write LaunchAgent: {e}"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn xdg_autostart_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(
                PathBuf::from(xdg)
                    .join("autostart")
                    .join("voicebar.desktop"),
            );
        }
    }
    std::env::var("HOME").ok().map(|h| {
        PathBuf::from(h)
            .join(".config")
            .join("autostart")
            .join("voicebar.desktop")
    })
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn is_autostart_enabled() -> bool {
    xdg_autostart_path().map(|p| p.is_file()).unwrap_or(false)
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let path = xdg_autostart_path().ok_or_else(|| "HOME is not set".to_string())?;
    if !enabled {
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("Failed to remove autostart entry: {e}"))?;
        }
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| format!("Cannot resolve voicebar path: {e}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Cannot create autostart dir: {e}"))?;
    }
    let desktop = format!(
        "[Desktop Entry]\nType=Application\nName=Voicebar\nExec={}\nIcon=voicebar\nTerminal=false\n",
        exe.display()
    );
    std::fs::write(&path, desktop).map_err(|e| format!("Failed to write autostart entry: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::personalization::SetupDraft;

    #[test]
    fn default_config_locks_english_and_auto_paste() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.model, ModelKind::Nemotron35);
        assert_eq!(cfg.language, "en-US");
        assert!(cfg.use_gpu);
        assert!(cfg.auto_paste);
        assert!(!cfg.personal_rewrite);
        assert!(cfg.rewrite_language.is_empty());
        assert_eq!(cfg.config_version, CURRENT_CONFIG_VERSION);
    }

    #[test]
    fn older_configs_keep_personal_rewriting_off() {
        let cfg: AppConfig =
            serde_json::from_str(r#"{"language":"hi","auto_paste":false}"#).unwrap();
        assert!(!cfg.personal_rewrite);
        assert!(cfg.rewrite_language.is_empty());
        assert_eq!(cfg.language, "hi");
        assert!(!cfg.auto_paste);
    }

    #[test]
    fn existing_explicit_opt_in_keeps_its_legacy_model_until_activation() {
        let existing =
            AppConfig::decode(r#"{"config_version":2,"personal_rewrite":true}"#).unwrap();
        assert!(existing.legacy_rewrite);
        assert!(existing.rewrite_profile.is_none());
        assert!(!AppConfig::default().legacy_rewrite);
    }

    #[test]
    fn activation_writes_the_reviewed_profile_and_enable_flag_together() {
        let mut draft = SetupDraft::from_paste("Can you uh check?").unwrap();
        draft.review(Some("Can you check?")).unwrap();
        let profile = draft
            .preview_inputs("First\n\nSecond\n\nThird")
            .unwrap()
            .profile;
        let sandbox = tempfile::tempdir().unwrap();
        let blocker = sandbox.path().join("not-a-directory");
        std::fs::write(&blocker, "keep this").unwrap();
        let mut config =
            AppConfig::decode(r#"{"config_version":2,"personal_rewrite":true,"auto_paste":false}"#)
                .unwrap();
        assert!(
            config
                .activate_at(profile.clone(), &blocker.join("config.json"))
                .is_err()
        );
        assert!(config.rewrite_profile.is_none());
        assert_eq!(std::fs::read_to_string(&blocker).unwrap(), "keep this");

        let path = sandbox.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"config_version":2,"personal_rewrite":true,"auto_paste":false}"#,
        )
        .unwrap();
        config.activate_at(profile.clone(), &path).unwrap();
        let saved = AppConfig::decode(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved.rewrite_profile, Some(profile));
        assert!(saved.personal_rewrite);
        assert!(!saved.legacy_rewrite);
        assert!(!saved.auto_paste);

        // Clearing removes the profile and disables personal rewriting cleanly.
        config.clear_at(&path).unwrap();
        let after_clear = AppConfig::decode(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(after_clear.rewrite_profile.is_none());
        assert!(!after_clear.personal_rewrite);
        assert!(!after_clear.legacy_rewrite);
    }

    #[test]
    fn decode_discards_invalid_profile_and_defaults_to_text_only_rewrite_model() {
        let decoded =
            AppConfig::decode(r#"{"config_version":3,"rewrite_profile":{"examples":[]}}"#).unwrap();
        assert!(decoded.rewrite_profile.is_none());
        assert!(!decoded.has_valid_profile());
        assert_eq!(decoded.rewrite_model, RewriteModel::SarvamTranslate4b);
        assert_eq!(RewriteModel::ALL.len(), 7);
        assert!(RewriteModel::Qwen3_0_6b.is_small_fast());
        assert!(RewriteModel::Qwen25_1_5b.is_small_fast());
        for model in RewriteModel::ALL {
            assert!(model.hf_download_url().starts_with("https://huggingface.co/"));
            assert!(model.gguf_filename().ends_with(".gguf"));
            assert!(model.approx_bytes() > 200_000_000);
        }
    }

    #[test]
    fn all_models_have_valid_manifests() {
        assert_eq!(ModelKind::ALL.len(), 4);
        for kind in ModelKind::ALL {
            assert!(!kind.id().is_empty());
            assert!(!kind.files().is_empty());
            assert!(kind.total_approx_bytes() > 100_000_000);
            for file in kind.files() {
                assert!(ModelKind::hf_url(file.remote_path).starts_with("https://huggingface.co/"));
            }
        }
    }
}
