//! Voicebar: a floating, always-on-top real-time GPU dictation capsule for Windows, macOS & Linux.
//!
//! Architecture:
//! - Pill window (`OverlayWindow`, fixed 548x68 px): ultra-clean single-surface floating bar.
//! - Settings popover (`SettingsWindow`, fixed 382x384 px): dedicated popup for models & toggles.
//! Never resizes the pill window, and always closes windows directly when called inside click handlers
//! so no window can ever get orphaned on screen.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod audio;
mod config;
mod download;
mod engine;
mod history_import;
mod hotkey;
mod overlay;
mod personalization;
mod profile_setup;
mod rewrite;
mod tray;

extern crate gpui_kit as gpui;

use crate::audio::WAVEFORM_BARS;
use crate::config::{
    AppConfig, ModelKind, RewriteModel, is_autostart_enabled, is_model_ready, models_root,
    open_folder, set_autostart,
};
use crate::engine::{EngineCommand, EngineEvent, EngineHandle};
use crate::overlay::{PILL_HEIGHT, PILL_WIDTH, SETTINGS_HEIGHT, SETTINGS_WIDTH};
use crate::tray::TrayMood;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

const INSTANCE_PORT: u16 = 7981;

#[derive(Clone, Debug, Default)]
pub enum Phase {
    #[default]
    Idle,
    Downloading {
        file_name: String,
        downloaded_mb: f64,
        total_mb: f64,
        fraction: f32,
        mb_per_sec: f32,
    },
    Loading {
        stage: String,
    },
    Listening {
        mic_name: String,
    },
    Finalizing,
    Copied {
        text: String,
        pasted: bool,
    },
    Error(String),
}

pub struct VoiceState {
    pub config: AppConfig,
    pub phase: Phase,
    pub live_text: String,
    pub last_transcript: String,
    pub rewrite_fell_back: bool,
    pending_rewrite: Option<PendingRewrite>,
    dictation_config: Option<AppConfig>,
    pub target_hwnd: isize,
    pub waveform: [f32; WAVEFORM_BARS],
    pub auto_hide_at: Option<Instant>,
    pub backend_label: String,
    pub model_loaded: bool,
    pub settings_open: bool,
    pub autostart: bool,
    pub tick: u64,
}

impl Global for VoiceState {}

struct PendingRewrite {
    result: Receiver<Option<String>>,
    deadline: Instant,
}

impl VoiceState {
    pub fn is_rewriting(&self) -> bool {
        self.pending_rewrite.is_some()
    }
}

struct EngineGlobal(EngineHandle);
impl Global for EngineGlobal {}

struct TrayGlobal(Option<tray::Tray>);
impl Global for TrayGlobal {}

struct OverlayWindow(Option<AnyWindowHandle>);
impl Global for OverlayWindow {}

struct SettingsWindow(Option<AnyWindowHandle>);
impl Global for SettingsWindow {}

struct OverlayView;

impl Render for OverlayView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        overlay::render_pill(cx)
    }
}

struct SettingsView;

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        overlay::render_settings_popover(cx)
    }
}

// =========================================================================
// Window management (two independent fixed-size frameless popup windows)
// =========================================================================

fn ensure_overlay_open(cx: &mut App) {
    if let Some(h) = cx.global::<OverlayWindow>().0 {
        if h.update(cx, |_, _, _| {}).is_ok() {
            cx.refresh_windows();
            return;
        }
        // Stale handle (window was already closed externally)
        cx.global_mut::<OverlayWindow>().0 = None;
    }

    let pill_w = px(PILL_WIDTH);
    let pill_h = px(PILL_HEIGHT);
    let bounds = match cx.primary_display() {
        Some(display) => {
            let vb = display.visible_bounds();
            let x = vb.origin.x + (vb.size.width - pill_w) / 2.0;
            let y = vb.origin.y + px(18.);
            Bounds::new(point(x, y), size(pill_w, pill_h))
        }
        None => Bounds::centered(None, size(pill_w, pill_h), cx),
    };

    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: true,
        is_resizable: false,
        is_minimizable: false,
        app_id: Some("dev.voicebar.app".into()),
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };

    match cx.open_window(options, |_window, cx| cx.new(|_| OverlayView)) {
        Ok(handle) => {
            hotkey::strip_dwm_outer_ring();
            cx.global_mut::<OverlayWindow>().0 = Some(handle.into());
        }
        Err(e) => eprintln!("voicebar: failed to open pill window: {e}"),
    }
}

fn ensure_settings_open(cx: &mut App) {
    if let Some(h) = cx.global::<SettingsWindow>().0 {
        if h.update(cx, |_, _, _| {}).is_ok() {
            cx.global_mut::<VoiceState>().settings_open = true;
            cx.refresh_windows();
            return;
        }
        cx.global_mut::<SettingsWindow>().0 = None;
    }

    let sw = px(SETTINGS_WIDTH);
    let sh = px(SETTINGS_HEIGHT);
    let bounds = match cx.primary_display() {
        Some(display) => {
            let vb = display.visible_bounds();
            let x = vb.origin.x + (vb.size.width - sw) / 2.0;
            let y = vb.origin.y + px(18. + PILL_HEIGHT + 10.);
            Bounds::new(point(x, y), size(sw, sh))
        }
        None => Bounds::centered(None, size(sw, sh), cx),
    };

    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: true,
        is_resizable: false,
        is_minimizable: false,
        app_id: Some("dev.voicebar.settings".into()),
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };

    match cx.open_window(options, |_window, cx| cx.new(|_| SettingsView)) {
        Ok(handle) => {
            hotkey::strip_dwm_outer_ring();
            cx.global_mut::<SettingsWindow>().0 = Some(handle.into());
            cx.global_mut::<VoiceState>().settings_open = true;
            cx.global_mut::<VoiceState>().auto_hide_at = None;
        }
        Err(e) => eprintln!("voicebar: failed to open settings popover: {e}"),
    }
}

/// Close the settings popover from outside its own click callback.
fn close_settings_external(cx: &mut App) {
    cx.global_mut::<VoiceState>().settings_open = false;
    if let Some(handle) = cx.global::<SettingsWindow>().0 {
        if handle
            .update(cx, |_, window, _| window.remove_window())
            .is_ok()
        {
            cx.global_mut::<SettingsWindow>().0 = None;
        } else {
            // Defer removal so we never lose track of a temporarily leased window.
            cx.defer(|cx| {
                if let Some(h) = cx.global_mut::<SettingsWindow>().0.take() {
                    let _ = h.update(cx, |_, window, _| window.remove_window());
                }
            });
        }
    }
}

/// Close the settings popover directly from inside its own window `.on_click` callback.
pub fn close_settings_from_window(window: &mut Window, cx: &mut App) {
    cx.global_mut::<VoiceState>().settings_open = false;
    cx.global_mut::<SettingsWindow>().0 = None;
    window.remove_window();
    cx.refresh_windows();
}

pub fn toggle_settings_popover(cx: &mut App) {
    if cx.global::<VoiceState>().settings_open {
        close_settings_external(cx);
    } else {
        ensure_settings_open(cx);
    }
    cx.refresh_windows();
}

/// Close the pill (and settings popover if open) from outside the pill's own click callback
/// (e.g. from the auto-hide timer, F1 hotkey, or engine event loop).
pub fn dismiss_overlay(cx: &mut App) {
    close_settings_external(cx);
    {
        let st = cx.global_mut::<VoiceState>();
        st.auto_hide_at = None;
        st.pending_rewrite = None;
        st.dictation_config = None;
        st.phase = Phase::Idle;
    }
    if let Some(handle) = cx.global::<OverlayWindow>().0 {
        if handle
            .update(cx, |_, window, _| window.remove_window())
            .is_ok()
        {
            cx.global_mut::<OverlayWindow>().0 = None;
        } else {
            cx.defer(|cx| {
                if let Some(h) = cx.global_mut::<OverlayWindow>().0.take() {
                    let _ = h.update(cx, |_, window, _| window.remove_window());
                }
            });
        }
    }
    sync_tray(cx);
}

/// Close the pill directly from inside its own `.on_click` callback (never re-entrant!).
pub fn dismiss_overlay_from_window(window: &mut Window, cx: &mut App) {
    close_settings_external(cx);
    {
        let st = cx.global_mut::<VoiceState>();
        st.auto_hide_at = None;
        st.pending_rewrite = None;
        st.dictation_config = None;
        st.phase = Phase::Idle;
    }
    cx.global_mut::<OverlayWindow>().0 = None;
    window.remove_window();
    sync_tray(cx);
}

pub fn cancel_dictation_from_window(window: &mut Window, cx: &mut App) {
    let _ = cx
        .global::<EngineGlobal>()
        .0
        .tx
        .send(EngineCommand::CancelDictation);
    dismiss_overlay_from_window(window, cx);
}

// =========================================================================
// Actions triggered from Settings Popover / Pill / Hotkey / Tray
// =========================================================================

pub fn select_model_from_settings(window: &mut Window, cx: &mut App, kind: ModelKind) {
    let was_active = matches!(
        cx.global::<VoiceState>().phase,
        Phase::Listening { .. } | Phase::Loading { .. } | Phase::Downloading { .. }
    );
    let already_selected = cx.global::<VoiceState>().config.model == kind;
    close_settings_from_window(window, cx);
    if already_selected {
        return;
    }
    let cfg = {
        let st = cx.global_mut::<VoiceState>();
        st.config.model = kind;
        st.config.save();
        st.model_loaded = false;
        st.live_text.clear();
        st.phase = if is_model_ready(kind) {
            Phase::Loading {
                stage: format!("Loading {} onto GPU…", kind.display_name()),
            }
        } else {
            Phase::Downloading {
                file_name: "Connecting to Hugging Face…".into(),
                downloaded_mb: 0.0,
                total_mb: kind.total_approx_bytes() as f64 / 1_048_576.0,
                fraction: 0.0,
                mb_per_sec: 0.0,
            }
        };
        st.config.clone()
    };
    if was_active {
        cx.global_mut::<VoiceState>().dictation_config = Some(cfg.clone());
    }
    ensure_overlay_open(cx);
    sync_tray(cx);
    let tx = &cx.global::<EngineGlobal>().0.tx;
    if was_active {
        let _ = tx.send(EngineCommand::CancelDictation);
        let _ = tx.send(EngineCommand::StartDictation(cfg));
    } else {
        let _ = tx.send(EngineCommand::Prepare(cfg));
    }
}

pub fn select_language_from_settings(cx: &mut App, code: &str) {
    let cfg = {
        let st = cx.global_mut::<VoiceState>();
        if st.config.language == code {
            return;
        }
        st.config.language = code.to_string();
        st.config.save();
        st.config.clone()
    };
    sync_tray(cx);
    let was_listening = matches!(cx.global::<VoiceState>().phase, Phase::Listening { .. });
    if was_listening {
        {
            let st = cx.global_mut::<VoiceState>();
            st.dictation_config = Some(cfg.clone());
            st.phase = Phase::Loading {
                stage: "Switching language…".into(),
            };
        }
        let tx = &cx.global::<EngineGlobal>().0.tx;
        let _ = tx.send(EngineCommand::CancelDictation);
        let _ = tx.send(EngineCommand::StartDictation(cfg));
    } else if cx.global::<VoiceState>().model_loaded {
        let tx = &cx.global::<EngineGlobal>().0.tx;
        let _ = tx.send(EngineCommand::Prepare(cfg));
    }
    cx.refresh_windows();
}

pub fn toggle_auto_paste_from_ui(cx: &mut App) {
    let st = cx.global_mut::<VoiceState>();
    st.config.auto_paste = !st.config.auto_paste;
    st.config.save();
    sync_tray(cx);
    cx.refresh_windows();
}

pub fn toggle_personal_rewrite_from_ui(cx: &mut App) {
    let st = cx.global_mut::<VoiceState>();
    st.config.personal_rewrite = !st.config.personal_rewrite;
    st.config.save();
    if st.pending_rewrite.take().is_some() {
        let original = st.last_transcript.clone();
        deliver_transcript(cx, original, false, true);
    }
    sync_tray(cx);
    cx.refresh_windows();
}

pub fn select_rewrite_model_from_ui(cx: &mut App, model: RewriteModel) {
    let st = cx.global_mut::<VoiceState>();
    if st.config.rewrite_model != model {
        st.config.rewrite_model = model;
        st.config.save();
        sync_tray(cx);
        cx.refresh_windows();
    }
}

pub fn select_rewrite_language_from_ui(cx: &mut App, code: &str) {
    let st = cx.global_mut::<VoiceState>();
    if rewrite::valid_language(code) && st.config.rewrite_language != code {
        st.config.rewrite_language = code.to_owned();
        st.config.save();
        cx.refresh_windows();
    }
}

pub fn activate_rewrite_profile(
    cx: &mut App,
    profile: personalization::RewriteProfile,
) -> Result<(), &'static str> {
    cx.global_mut::<VoiceState>()
        .config
        .activate_profile(profile)
        .map_err(|_| "Voicebar could not save the profile. Check access to its settings folder and try again.")?;
    sync_tray(cx);
    cx.refresh_windows();
    Ok(())
}

pub fn clear_rewrite_profile(cx: &mut App) -> Result<(), &'static str> {
    cx.global_mut::<VoiceState>()
        .config
        .clear_profile()
        .map_err(|_| "Voicebar could not clear the saved profile. Check access to its settings folder.")?;
    sync_tray(cx);
    cx.refresh_windows();
    Ok(())
}

pub fn toggle_autostart_from_ui(cx: &mut App) {
    let target = !cx.global::<VoiceState>().autostart;
    match set_autostart(target) {
        Ok(()) => {
            cx.global_mut::<VoiceState>().autostart = target;
            sync_tray(cx);
        }
        Err(e) => {
            let st = cx.global_mut::<VoiceState>();
            st.phase = Phase::Error(e);
            st.auto_hide_at = Some(Instant::now() + Duration::from_secs(5));
            ensure_overlay_open(cx);
        }
    }
    cx.refresh_windows();
}

pub fn toggle_gpu_from_ui(cx: &mut App) {
    let cfg = {
        let st = cx.global_mut::<VoiceState>();
        st.config.use_gpu = !st.config.use_gpu;
        st.config.save();
        st.model_loaded = false;
        st.config.clone()
    };
    sync_tray(cx);
    if is_model_ready(cfg.model) {
        let _ = cx
            .global::<EngineGlobal>()
            .0
            .tx
            .send(EngineCommand::Prepare(cfg));
    }
    cx.refresh_windows();
}

fn copy_to_clipboard(cx: &App, text: &str) {
    if text.is_empty() {
        return;
    }
    cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_text(text.to_string());
    }
}

pub fn toggle_dictation(cx: &mut App) {
    close_settings_external(cx);
    let phase = cx.global::<VoiceState>().phase.clone();
    match phase {
        Phase::Listening { .. } => {
            cx.global_mut::<VoiceState>().phase = Phase::Finalizing;
            let _ = cx
                .global::<EngineGlobal>()
                .0
                .tx
                .send(EngineCommand::StopDictation);
            cx.refresh_windows();
        }
        Phase::Downloading { .. } | Phase::Loading { .. } => {
            let _ = cx
                .global::<EngineGlobal>()
                .0
                .tx
                .send(EngineCommand::CancelDictation);
            dismiss_overlay(cx);
        }
        Phase::Finalizing => {
            if cx.global::<VoiceState>().is_rewriting() {
                dismiss_overlay(cx);
            }
        }
        Phase::Idle | Phase::Copied { .. } | Phase::Error(_) => {
            let cfg = cx.global::<VoiceState>().config.clone();
            let fg_hwnd = hotkey::capture_foreground_hwnd();
            {
                let st = cx.global_mut::<VoiceState>();
                st.live_text.clear();
                st.dictation_config = Some(cfg.clone());
                st.target_hwnd = fg_hwnd;
                st.auto_hide_at = None;
                st.waveform = [0.08; WAVEFORM_BARS];
                st.phase = if st.model_loaded {
                    Phase::Loading {
                        stage: "Opening microphone…".into(),
                    }
                } else if is_model_ready(cfg.model) {
                    Phase::Loading {
                        stage: format!("Warming {} on GPU…", cfg.model.short_badge()),
                    }
                } else {
                    Phase::Downloading {
                        file_name: "Connecting to Hugging Face…".into(),
                        downloaded_mb: 0.0,
                        total_mb: cfg.model.total_approx_bytes() as f64 / 1_048_576.0,
                        fraction: 0.0,
                        mb_per_sec: 0.0,
                    }
                };
            }
            ensure_overlay_open(cx);
            let _ = cx
                .global::<EngineGlobal>()
                .0
                .tx
                .send(EngineCommand::StartDictation(cfg));
            sync_tray(cx);
            cx.refresh_windows();
        }
    }
}

fn sync_tray(cx: &mut App) {
    let (cfg, autostart, mood, tip) = {
        let st = cx.global::<VoiceState>();
        let mood = if matches!(st.phase, Phase::Listening { .. }) {
            TrayMood::Recording
        } else if !st.model_loaded {
            TrayMood::Warming
        } else {
            TrayMood::Ready
        };
        let tip = match mood {
            TrayMood::Recording if st.config.personal_rewrite => {
                "Voicebar · Listening (F1 to rewrite & copy)".to_string()
            }
            TrayMood::Recording => "Voicebar · Listening (F1 to stop & paste)".to_string(),
            TrayMood::Warming => format!(
                "Voicebar · Preloading {} on GPU…",
                st.config.model.short_badge()
            ),
            TrayMood::Ready => format!(
                "Voicebar · {} ({}) · Ready · Press F1",
                st.config.model.short_badge(),
                st.backend_label
            ),
        };
        (st.config.clone(), st.autostart, mood, tip)
    };
    if let Some(tray) = cx.global_mut::<TrayGlobal>().0.as_mut() {
        tray.sync(&cfg, autostart, mood, &tip);
    }
}

fn deliver_transcript(cx: &mut App, text: String, auto_paste: bool, rewrite_fell_back: bool) {
    let pasted = auto_paste && !text.is_empty();
    if !text.is_empty() {
        copy_to_clipboard(cx, &text);
        if pasted {
            hotkey::paste_ctrl_v_async(cx.global::<VoiceState>().target_hwnd);
        }
    }
    let st = cx.global_mut::<VoiceState>();
    st.live_text = text.clone();
    st.phase = Phase::Copied { text, pasted };
    st.rewrite_fell_back = rewrite_fell_back;
    let display_ms = if rewrite_fell_back { 2500 } else { 850 };
    st.auto_hide_at = Some(Instant::now() + Duration::from_millis(display_ms));
    sync_tray(cx);
}

fn finish_dictation(cx: &mut App, text: String) {
    let st = cx.global_mut::<VoiceState>();
    if !matches!(st.phase, Phase::Finalizing) || st.is_rewriting() {
        return;
    }
    // A profile activated while the microphone is open applies on the next recording.
    let config = st
        .dictation_config
        .take()
        .unwrap_or_else(|| st.config.clone());
    if !text.is_empty() {
        // "Copy Last Transcript" always recovers what the recognizer heard.
        st.last_transcript = text.clone();
        if config.personal_rewrite {
            if let Some(result) = rewrite::start(
                &text,
                &config.rewrite_language,
                rewrite::RewriteMode::configured(&config),
            ) {
                st.pending_rewrite = Some(PendingRewrite {
                    result,
                    deadline: Instant::now() + rewrite::TIMEOUT,
                });
                return;
            }
            deliver_transcript(cx, text, false, true);
            return;
        }
    }
    let auto_paste = config.auto_paste;
    deliver_transcript(cx, text, auto_paste, false);
}

fn poll_rewrite(cx: &mut App) {
    let st = cx.global_mut::<VoiceState>();
    let Some(pending) = st.pending_rewrite.as_ref() else {
        return;
    };
    if !matches!(st.phase, Phase::Finalizing) {
        st.pending_rewrite = None;
        return;
    }
    let result = match pending.result.try_recv() {
        Ok(text) => text,
        Err(TryRecvError::Empty) if Instant::now() < pending.deadline => return,
        Err(_) => None,
    };
    st.pending_rewrite = None;
    let fell_back = result.is_none();
    let text = result.unwrap_or_else(|| st.last_transcript.clone());
    // The user can change apps while the model runs. Leave pasting to them.
    deliver_transcript(cx, text, false, fell_back);
    cx.refresh_windows();
}

fn handle_engine_event(cx: &mut App, ev: EngineEvent) {
    match ev {
        EngineEvent::Downloading(p) => {
            let fraction = p.fraction();
            let st = cx.global_mut::<VoiceState>();
            st.phase = Phase::Downloading {
                file_name: p.file_name,
                downloaded_mb: p.downloaded_bytes as f64 / 1_048_576.0,
                total_mb: p.total_bytes as f64 / 1_048_576.0,
                fraction,
                mb_per_sec: p.mb_per_sec,
            };
            st.auto_hide_at = None;
            ensure_overlay_open(cx);
        }
        EngineEvent::LoadingModel { gpu, stage } => {
            let has_overlay = cx.global::<OverlayWindow>().0.is_some();
            let st = cx.global_mut::<VoiceState>();
            st.backend_label = if gpu {
                engine::GPU_BACKEND_LABEL.into()
            } else {
                "CPU · Burn".into()
            };
            if has_overlay {
                st.phase = Phase::Loading { stage };
            }
            sync_tray(cx);
        }
        EngineEvent::Ready { backend_label } => {
            let has_overlay = cx.global::<OverlayWindow>().0.is_some();
            {
                let st = cx.global_mut::<VoiceState>();
                st.model_loaded = true;
                st.backend_label = backend_label;
                if has_overlay && matches!(st.phase, Phase::Loading { .. } | Phase::Downloading { .. }) {
                    st.phase = Phase::Idle;
                    st.auto_hide_at = Some(Instant::now() + Duration::from_millis(900));
                }
            }
            sync_tray(cx);
        }
        EngineEvent::Listening { mic_name, gpu } => {
            let st = cx.global_mut::<VoiceState>();
            st.model_loaded = true;
            st.backend_label = if gpu {
                engine::GPU_BACKEND_LABEL.into()
            } else {
                "CPU · Burn".into()
            };
            st.phase = Phase::Listening { mic_name };
            st.auto_hide_at = None;
            ensure_overlay_open(cx);
            sync_tray(cx);
        }
        EngineEvent::TranscriptUpdate { text, .. } => {
            cx.global_mut::<VoiceState>().live_text = text;
        }
        EngineEvent::Finalizing => {
            // Ignore notifications for dictations dismissed before the worker flushed.
            if matches!(cx.global::<VoiceState>().phase, Phase::Listening { .. }) {
                cx.global_mut::<VoiceState>().phase = Phase::Finalizing;
            }
        }
        EngineEvent::Finished { text } => {
            finish_dictation(cx, text);
        }
        EngineEvent::Cancelled => {
            if !matches!(
                cx.global::<VoiceState>().phase,
                Phase::Loading { .. } | Phase::Downloading { .. }
            ) {
                dismiss_overlay(cx);
            }
        }
        EngineEvent::Error(msg) => {
            let st = cx.global_mut::<VoiceState>();
            st.phase = Phase::Error(msg);
            st.auto_hide_at = Some(Instant::now() + Duration::from_secs(5));
            ensure_overlay_open(cx);
            sync_tray(cx);
        }
    }
    cx.refresh_windows();
}

fn handle_tray_action(cx: &mut App, action: tray::TrayAction) {
    use tray::TrayAction::*;
    match action {
        ToggleDictation => toggle_dictation(cx),
        OpenSettings => toggle_settings_popover(cx),
        OpenProfileSetup => profile_setup::open(cx),
        CopyLast => {
            let last = cx.global::<VoiceState>().last_transcript.clone();
            if !last.is_empty() {
                copy_to_clipboard(cx, &last);
                let st = cx.global_mut::<VoiceState>();
                st.rewrite_fell_back = false;
                st.phase = Phase::Copied {
                    text: last,
                    pasted: false,
                };
                st.auto_hide_at = Some(Instant::now() + Duration::from_millis(900));
                ensure_overlay_open(cx);
                cx.refresh_windows();
            }
        }
        TogglePersonalRewrite => toggle_personal_rewrite_from_ui(cx),
        ToggleAutoPaste => toggle_auto_paste_from_ui(cx),
        ToggleAutostart => toggle_autostart_from_ui(cx),
        ToggleGpu => toggle_gpu_from_ui(cx),
        SelectModel(kind) => {
            let cfg = {
                let st = cx.global_mut::<VoiceState>();
                if st.config.model == kind {
                    return;
                }
                st.config.model = kind;
                st.config.save();
                st.model_loaded = false;
                st.config.clone()
            };
            sync_tray(cx);
            let _ = cx
                .global::<EngineGlobal>()
                .0
                .tx
                .send(EngineCommand::Prepare(cfg));
        }
        SelectLanguage(lang) => select_language_from_settings(cx, &lang),
        OpenModelsFolder => open_folder(&models_root()),
        Quit => {
            let _ = cx
                .global::<EngineGlobal>()
                .0
                .tx
                .send(EngineCommand::Shutdown);
            cx.quit();
        }
    }
}

fn start_loop(cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            let mut active_overlay = false;
            let _ = cx.update(|cx| {
                cx.global_mut::<VoiceState>().tick =
                    cx.global::<VoiceState>().tick.wrapping_add(1);

                if hotkey::take_toggle_request() {
                    toggle_dictation(cx);
                }

                let tray_actions = cx
                    .global::<TrayGlobal>()
                    .0
                    .as_ref()
                    .map(|t| t.poll())
                    .unwrap_or_default();
                for act in tray_actions {
                    handle_tray_action(cx, act);
                }

                let mut events = Vec::new();
                while let Ok(ev) = cx.global::<EngineGlobal>().0.rx.try_recv() {
                    events.push(ev);
                }
                for ev in events {
                    handle_engine_event(cx, ev);
                }
                poll_rewrite(cx);

                let is_listening =
                    matches!(cx.global::<VoiceState>().phase, Phase::Listening { .. });
                let needs_anim = matches!(
                    cx.global::<VoiceState>().phase,
                    Phase::Listening { .. } | Phase::Loading { .. } | Phase::Finalizing
                );
                if is_listening {
                    let bars = cx.global::<EngineGlobal>().0.audio.waveform();
                    cx.global_mut::<VoiceState>().waveform = bars;
                }
                if needs_anim && cx.global::<OverlayWindow>().0.is_some() {
                    cx.refresh_windows();
                }

                if let Some(deadline) = cx.global::<VoiceState>().auto_hide_at {
                    if Instant::now() >= deadline {
                        dismiss_overlay(cx);
                    }
                }

                active_overlay = cx.global::<OverlayWindow>().0.is_some()
                    || cx.global::<SettingsWindow>().0.is_some();
            });

            let sleep_ms = if active_overlay { 33 } else { 80 };
            cx.background_executor()
                .timer(Duration::from_millis(sleep_ms))
                .await;
        }
    })
    .detach();
}

struct CliArgs {
    print_version: bool,
    download_only: bool,
    test_wav: Option<PathBuf>,
    model_override: Option<ModelKind>,
    lang_override: Option<String>,
    force_cpu: bool,
    personal_rewrite: bool,
    rewrite_model: Option<RewriteModel>,
    rewrite_language: Option<String>,
    rewrite_stdin: bool,
    show_on_start: bool,
}

fn parse_args() -> CliArgs {
    let mut out = CliArgs {
        print_version: false,
        download_only: false,
        test_wav: None,
        model_override: None,
        lang_override: None,
        force_cpu: false,
        personal_rewrite: false,
        rewrite_model: None,
        rewrite_language: None,
        rewrite_stdin: false,
        show_on_start: false,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--version" | "-V" | "version" => out.print_version = true,
            "--download" => out.download_only = true,
            "--cpu" => out.force_cpu = true,
            "--personal-rewrite" => out.personal_rewrite = true,
            "--rewrite-stdin" => out.rewrite_stdin = true,
            "--rewrite-model" if i + 1 < args.len() => {
                i += 1;
                match args[i].to_lowercase().as_str() {
                    "qwen3" | "qwen3-4b" | "4b" => out.rewrite_model = Some(RewriteModel::Qwen3_4b),
                    "qwen3-1.7b" | "1.7b" | "fast" => {
                        out.rewrite_model = Some(RewriteModel::Qwen3_1_7b)
                    }
                    "qwen2.5" | "qwen2.5-3b" | "3b" => {
                        out.rewrite_model = Some(RewriteModel::Qwen25_3b)
                    }
                    "sarvam-translate" | "sarvam-4b" => {
                        out.rewrite_model = Some(RewriteModel::SarvamTranslate4b)
                    }
                    "sarvam-m" | "sarvam" => out.rewrite_model = Some(RewriteModel::SarvamM24b),
                    _ => {}
                }
            }
            "--rewrite-language" => {
                i += 1;
                let code = args.get(i).filter(|code| rewrite::valid_language(code));
                let Some(code) = code else {
                    eprintln!("--rewrite-language expects a language tag such as hi or en-US.");
                    std::process::exit(2);
                };
                out.rewrite_language = Some(code.clone());
            }
            "--show" => out.show_on_start = true,
            "--model" if i + 1 < args.len() => {
                i += 1;
                match args[i].to_lowercase().as_str() {
                    "eou" | "parakeet" | "120m" => {
                        out.model_override = Some(ModelKind::ParakeetEou)
                    }
                    "nemotron-en" | "en" => out.model_override = Some(ModelKind::NemotronEn),
                    "unified" | "parakeet-unified" => {
                        out.model_override = Some(ModelKind::ParakeetUnifiedEn)
                    }
                    "nemotron" | "nemotron35" | "0.6b" => {
                        out.model_override = Some(ModelKind::Nemotron35)
                    }
                    _ => {}
                }
            }
            "--lang" if i + 1 < args.len() => {
                i += 1;
                out.lang_override = Some(args[i].clone());
            }
            "--test-wav" if i + 1 < args.len() => {
                i += 1;
                out.test_wav = Some(PathBuf::from(&args[i]));
            }
            _ => {}
        }
        i += 1;
    }
    out
}

fn apply_dark_theme(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    let th = cx.global_mut::<Theme>();
    th.font_family = overlay::ui_font().into();
    th.font_size = px(14.);
    th.radius = px(10.);
    let c = &mut th.colors;
    c.background = rgb(0x0d0d14).into();
    c.foreground = rgb(0xf4f4f6).into();
    c.border = rgba(0xffffff1a).into();
    c.input = rgba(0xffffff1e).into();
    c.muted = rgb(0x161622).into();
    c.muted_foreground = rgb(0x9ca3af).into();
    c.secondary = rgb(0x1a1a27).into();
    c.secondary_hover = rgb(0x232334).into();
    c.secondary_active = rgb(0x28283c).into();
    c.secondary_foreground = rgb(0xe4e4eb).into();
    c.primary = rgb(0x6366f1).into();
    c.primary_hover = rgb(0x818cf8).into();
    c.primary_active = rgb(0x4f46e5).into();
    c.primary_foreground = rgb(0xffffff).into();
    c.button = rgb(0x1a1a27).into();
    c.button_hover = rgb(0x242436).into();
    c.button_active = rgb(0x2c2c42).into();
    c.button_foreground = rgb(0xe4e4eb).into();
    c.button_primary = rgb(0x6366f1).into();
    c.button_primary_hover = rgb(0x818cf8).into();
    c.button_primary_active = rgb(0x4f46e5).into();
    c.button_primary_foreground = rgb(0xffffff).into();
    c.ring = rgb(0x818cf8).into();
    c.caret = rgb(0xa5b4fc).into();
    c.selection = rgba(0x6366f144).into();
    c.accent = rgb(0x1e1e2e).into();
    c.accent_foreground = rgb(0xf4f4f6).into();
    c.popover = rgb(0x151520).into();
    c.popover_foreground = rgb(0xf4f4f6).into();
    c.title_bar = rgb(0x101018).into();
    c.title_bar_border = rgba(0xffffff14).into();
}

fn main() {
    let cli = parse_args();
    if cli.print_version {
        println!("voicebar {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let mut cfg = AppConfig::load();
    if let Some(model) = cli.rewrite_model {
        cfg.rewrite_model = model;
    }
    if let Some(language) = cli.rewrite_language {
        cfg.rewrite_language = language;
    }
    if cli.personal_rewrite {
        cfg.personal_rewrite = true;
    }
    if cli.rewrite_stdin {
        if let Err(message) = rewrite::run_stdin(
            &cfg.rewrite_language,
            rewrite::RewriteMode::configured(&cfg),
        ) {
            eprintln!("{message}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(m) = cli.model_override {
        cfg.model = m;
    }
    if let Some(l) = cli.lang_override {
        cfg.language = l;
    }
    if cli.force_cpu {
        cfg.use_gpu = false;
    }

    if cli.download_only {
        eprintln!(
            "Downloading {} to {}…",
            cfg.model.display_name(),
            config::model_dir(cfg.model).display()
        );
        match download::ensure_model_downloaded(cfg.model, |p| {
            eprint!(
                "\r  {}  {:.1}/{:.1} MB ({:.0}%)  {:.1} MB/s   ",
                p.file_name,
                p.downloaded_bytes as f64 / 1_048_576.0,
                p.total_bytes as f64 / 1_048_576.0,
                p.fraction() * 100.0,
                p.mb_per_sec
            );
        }) {
            Ok(dir) => {
                eprintln!("\nDone: {}", dir.display());
                return;
            }
            Err(e) => {
                eprintln!("\nDownload error: {e}");
                std::process::exit(1);
            }
        }
    }

    if let Some(wav) = cli.test_wav {
        if let Err(e) = engine::run_wav_benchmark(&cfg, &wav) {
            eprintln!("Benchmark error: {e}");
            std::process::exit(1);
        }
        return;
    }

    // Single-instance guard: launching a second time toggles the running instance.
    match std::net::TcpListener::bind(("127.0.0.1", INSTANCE_PORT)) {
        Ok(listener) => {
            std::thread::spawn(move || {
                for _ in listener.incoming().flatten() {
                    hotkey::trigger_toggle();
                }
            });
        }
        Err(_) => {
            let _ = std::net::TcpStream::connect(("127.0.0.1", INSTANCE_PORT));
            return;
        }
    }

    let engine = EngineHandle::spawn();

    // Pre-load & warm the cached model silently in the background right on app startup
    // so pressing F1 opens immediately with zero wait.
    if is_model_ready(cfg.model) {
        let _ = engine.tx.send(EngineCommand::Prepare(cfg.clone()));
    }

    let show_on_start = cli.show_on_start;
    let autostart = is_autostart_enabled();
    let initial_backend = if cfg.use_gpu {
        engine::GPU_BACKEND_LABEL.to_string()
    } else {
        "CPU · Burn".to_string()
    };

    application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            apply_dark_theme(cx);
            cx.set_quit_mode(QuitMode::Explicit);

            // Register global F1 hotkey on the main thread (required by macOS Carbon/AppKit).
            let hotkey_err = hotkey::start_global_f1_listener().err();
            let tray = tray::Tray::new(&cfg, autostart);
            cx.set_global(VoiceState {
                config: cfg,
                phase: Phase::Idle,
                live_text: String::new(),
                last_transcript: String::new(),
                rewrite_fell_back: false,
                pending_rewrite: None,
                dictation_config: None,
                target_hwnd: 0,
                waveform: [0.08; WAVEFORM_BARS],
                auto_hide_at: None,
                backend_label: initial_backend,
                model_loaded: false,
                settings_open: false,
                autostart,
                tick: 0,
            });
            cx.set_global(EngineGlobal(engine));
            cx.set_global(TrayGlobal(tray));
            cx.set_global(OverlayWindow(None));
            cx.set_global(SettingsWindow(None));
            profile_setup::init(cx);
            sync_tray(cx);
            start_loop(cx);

            if let Some(err) = hotkey_err {
                let st = cx.global_mut::<VoiceState>();
                st.phase = Phase::Error(err);
                st.auto_hide_at = Some(Instant::now() + Duration::from_secs(7));
                ensure_overlay_open(cx);
            } else if show_on_start {
                ensure_overlay_open(cx);
            }
        });
}
