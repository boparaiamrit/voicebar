//! Two-window floating GPUI Kit UI for Voicebar:
//! 1. `render_pill`: ultra-clean single-surface Dynamic Island capsule (fixed 548x68 px, never stretched).
//! 2. `render_settings_popover`: dedicated floating Settings & Model card (376x380 px) anchored below.

use crate::audio::WAVEFORM_BARS;
use crate::config::{ModelKind, is_model_ready};
use crate::{
    Phase, VoiceState, cancel_dictation_from_window, close_settings_from_window,
    dismiss_overlay_from_window, select_language_from_settings, select_model_from_settings,
    toggle_auto_paste_from_ui, toggle_autostart_from_ui, toggle_gpu_from_ui,
    toggle_settings_popover,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

pub const PILL_WIDTH: f32 = 548.0;
pub const PILL_HEIGHT: f32 = 68.0;
pub const SETTINGS_WIDTH: f32 = 382.0;
pub const SETTINGS_HEIGHT: f32 = 384.0;

fn c(x: u32) -> Hsla {
    rgb(x).into()
}
fn ca(x: u32) -> Hsla {
    rgba(x).into()
}

pub fn ui_font() -> &'static str {
    static F: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    F.get_or_init(|| {
        if cfg!(windows) {
            let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
            if std::path::Path::new(&windir)
                .join("Fonts")
                .join("SegUIVar.ttf")
                .exists()
            {
                "Segoe UI Variable Text"
            } else {
                "Segoe UI"
            }
        } else if cfg!(target_os = "macos") {
            ".SystemUIFont"
        } else {
            "Inter"
        }
    })
}

fn icon(name: &str, sz: f32, color: Hsla) -> Svg {
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .size(px(sz))
        .flex_none()
        .text_color(color)
}

fn visible_tail(text: &str, max_chars: usize) -> (bool, String) {
    let total = text.chars().count();
    if total <= max_chars {
        return (false, text.to_string());
    }
    let skip = total - max_chars;
    let tail: String = text.chars().skip(skip).collect();
    if let Some(pos) = tail.find(' ') {
        if pos < 20 {
            return (true, tail[pos + 1..].to_string());
        }
    }
    (true, tail)
}

// =========================================================================
// 1. Floating Dynamic Island Dictation Capsule (fixed 548 x 68 px)
// =========================================================================

pub fn render_pill(cx: &mut App) -> AnyElement {
    let state = cx.global::<VoiceState>();
    let phase = state.phase.clone();
    let cfg = state.config.clone();
    let transcript = state.live_text.clone();
    let bars = state.waveform;
    let settings_open = state.settings_open;
    let tick = state.tick;
    let pulse_on = (tick / 12) % 2 == 0;

    let surface = ca(0x101016f7);
    let text_hi = c(0xf4f4f6);
    let text_mid = c(0xa1a1aa);
    let text_dim = c(0x71717a);
    let indigo = c(0x818cf8);
    let violet = c(0xa78bfa);
    let crimson = c(0xf87171);
    let emerald = c(0x4ade80);
    let amber = c(0xfbbf24);

    let border = match &phase {
        Phase::Listening { .. } => ca(0x818cf855),
        Phase::Loading { .. } | Phase::Downloading { .. } => ca(0x818cf842),
        Phase::Copied { .. } => ca(0x4ade8066),
        Phase::Error(_) => ca(0xf8717166),
        _ => ca(0xffffff1c),
    };

    // Optional razor-thin progress/shimmer bar along the bottom edge (loading / downloading only).
    let bottom_bar: Option<Div> = match &phase {
        Phase::Loading { .. } => {
            let track_w = PILL_WIDTH - 48.0;
            let bar_w = 132.0f32;
            let t = ((tick % 52) as f32) / 52.0;
            let eased = 0.5 - 0.5 * (t * std::f32::consts::TAU).cos();
            let offset = (eased * (track_w - bar_w)).round();
            Some(
                div()
                    .mx(px(24.))
                    .mb(px(3.))
                    .h(px(2.5))
                    .rounded(px(999.))
                    .bg(ca(0xffffff10))
                    .overflow_hidden()
                    .child(
                        div()
                            .ml(px(offset))
                            .w(px(bar_w))
                            .h_full()
                            .rounded(px(999.))
                            .bg(indigo),
                    ),
            )
        }
        Phase::Downloading { fraction, .. } => {
            let track_w = PILL_WIDTH - 48.0;
            let bar_w = (track_w * fraction.clamp(0.02, 1.0)).round();
            Some(
                div()
                    .mx(px(24.))
                    .mb(px(3.))
                    .h(px(2.5))
                    .rounded(px(999.))
                    .bg(ca(0xffffff10))
                    .overflow_hidden()
                    .child(div().w(px(bar_w)).h_full().rounded(px(999.)).bg(indigo)),
            )
        }
        _ => None,
    };

    // Left live indicator / waveform cluster (acts as drag handle too).
    let left_cluster: AnyElement = match &phase {
        Phase::Listening { .. } => div()
            .id("pill-drag-left")
            .h_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .window_control_area(WindowControlArea::Drag)
            .child(
                div()
                    .size(px(8.))
                    .rounded_full()
                    .bg(crimson)
                    .when(!pulse_on, |d| d.opacity(0.4)),
            )
            .child(live_waveform(bars, indigo, violet))
            .into_any_element(),
        Phase::Loading { .. } | Phase::Finalizing => div()
            .id("pill-drag-load")
            .h_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .window_control_area(WindowControlArea::Drag)
            .child(
                div()
                    .size(px(8.))
                    .rounded_full()
                    .bg(if matches!(phase, Phase::Finalizing) {
                        amber
                    } else {
                        violet
                    })
                    .when(!pulse_on, |d| d.opacity(0.45)),
            )
            .child(breathing_wave(tick, indigo, violet))
            .into_any_element(),
        Phase::Downloading { .. } => div()
            .id("pill-drag-dl")
            .h_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .window_control_area(WindowControlArea::Drag)
            .child(div().size(px(8.)).rounded_full().bg(indigo))
            .into_any_element(),
        Phase::Copied { .. } => div()
            .size(px(26.))
            .rounded_full()
            .bg(ca(0x4ade8024))
            .flex()
            .items_center()
            .justify_center()
            .child(icon("check", 14., emerald))
            .into_any_element(),
        Phase::Error(_) => div()
            .size(px(26.))
            .rounded_full()
            .bg(ca(0xf8717124))
            .flex()
            .items_center()
            .justify_center()
            .child(icon("triangle-alert", 14., crimson))
            .into_any_element(),
        Phase::Idle => div()
            .size(px(8.))
            .rounded_full()
            .bg(indigo)
            .into_any_element(),
    };

    // Center live text surface (single clean plane — no nested inner box, draggable).
    let center: AnyElement = match &phase {
        Phase::Listening { .. } => {
            if transcript.trim().is_empty() {
                div()
                    .id("pill-drag-mid")
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .window_control_area(WindowControlArea::Drag)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(text_mid)
                            .child("Listening…"),
                    )
                    .child(
                        div()
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(5.))
                            .bg(ca(0xffffff0e))
                            .border_1()
                            .border_color(ca(0xffffff18))
                            .text_size(px(11.))
                            .text_color(text_dim)
                            .child(if cfg.auto_paste {
                                "F1 to paste"
                            } else {
                                "F1 to copy"
                            }),
                    )
                    .into_any_element()
            } else {
                let (clipped, shown) = visible_tail(&transcript, 118);
                let display = if clipped {
                    format!("… {shown}")
                } else {
                    shown
                };
                div()
                    .id("pill-drag-text")
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .overflow_hidden()
                    .window_control_area(WindowControlArea::Drag)
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .w_full()
                            .text_size(px(14.5))
                            .line_height(px(20.))
                            .text_color(text_hi)
                            .child(format!("{display}{}", if pulse_on { " ▍" } else { "" })),
                    )
                    .into_any_element()
            }
        }
        Phase::Loading { stage } => div()
            .id("pill-drag-stage")
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .window_control_area(WindowControlArea::Drag)
            .flex()
            .flex_col()
            .justify_center()
            .child(
                div()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(text_hi)
                    .truncate()
                    .child(stage.clone()),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(text_dim)
                    .truncate()
                    .child("First load only · instant after GPU warmup"),
            )
            .into_any_element(),
        Phase::Downloading {
            file_name,
            downloaded_mb,
            total_mb,
            fraction,
            mb_per_sec,
        } => {
            let pct = (fraction * 100.0).round() as u32;
            div()
                .id("pill-drag-dl-txt")
                .flex_1()
                .min_w(px(0.))
                .h_full()
                .window_control_area(WindowControlArea::Drag)
                .flex()
                .flex_col()
                .justify_center()
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(text_hi)
                        .truncate()
                        .child(format!("Downloading {} ({pct}%)", cfg.model.short_badge())),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(text_dim)
                        .truncate()
                        .child(format!(
                            "{file_name} · {downloaded_mb:.0}/{total_mb:.0} MB · {mb_per_sec:.1} MB/s"
                        )),
                )
                .into_any_element()
        }
        Phase::Finalizing => div()
            .id("pill-drag-fin")
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .window_control_area(WindowControlArea::Drag)
            .flex()
            .items_center()
            .child(
                div()
                    .text_size(px(14.))
                    .text_color(text_mid)
                    .child("Finalizing…"),
            )
            .into_any_element(),
        Phase::Copied { text, pasted } => {
            let label = if *pasted { "Pasted" } else { "Copied" };
            let preview = if text.is_empty() {
                "No speech detected".to_string()
            } else {
                let (_, tail) = visible_tail(text, 78);
                tail
            };
            div()
                .id("pill-drag-ok")
                .flex_1()
                .min_w(px(0.))
                .h_full()
                .window_control_area(WindowControlArea::Drag)
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(emerald)
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_size(px(13.5))
                        .text_color(text_hi)
                        .truncate()
                        .child(preview),
                )
                .into_any_element()
        }
        Phase::Error(msg) => div()
            .id("pill-drag-err")
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .window_control_area(WindowControlArea::Drag)
            .flex()
            .items_center()
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(text_hi)
                    .truncate()
                    .child(msg.clone()),
            )
            .into_any_element(),
        Phase::Idle => div()
            .id("pill-drag-idle")
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .window_control_area(WindowControlArea::Drag)
            .flex()
            .items_center()
            .child(
                div()
                    .text_size(px(13.5))
                    .text_color(text_mid)
                    .child("Press F1 anywhere to dictate"),
            )
            .into_any_element(),
    };

    // Right controls: minimal 2-icon cluster (Settings/Models popover + guaranteed instant Close).
    let right_controls = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.))
        .child(
            div()
                .id("pill-settings-btn")
                .size(px(28.))
                .rounded_full()
                .bg(if settings_open {
                    ca(0x6366f138)
                } else {
                    ca(0xffffff0a)
                })
                .border_1()
                .border_color(if settings_open {
                    ca(0x818cf866)
                } else {
                    ca(0xffffff12)
                })
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(ca(0xffffff1c)))
                .child(icon(
                    "settings",
                    13.5,
                    if settings_open { indigo } else { text_mid },
                ))
                .on_click(|_, _, cx| toggle_settings_popover(cx)),
        )
        .child(
            div()
                .id("pill-close-btn")
                .size(px(28.))
                .rounded_full()
                .bg(ca(0xffffff08))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(ca(0xf871712a)))
                .child(icon("x", 13.5, text_mid))
                .on_click(|_, window, cx| {
                    let is_active = matches!(
                        cx.global::<VoiceState>().phase,
                        Phase::Listening { .. } | Phase::Loading { .. } | Phase::Downloading { .. }
                    );
                    if is_active {
                        cancel_dictation_from_window(window, cx);
                    } else {
                        dismiss_overlay_from_window(window, cx);
                    }
                }),
        );

    div()
        .size_full()
        .font_family(ui_font())
        .rounded(px(22.))
        .bg(surface)
        .border_1()
        .border_color(border)
        .flex()
        .flex_col()
        .child(
            div()
                .flex_1()
                .min_h(px(0.))
                .px(px(16.))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.))
                .child(left_cluster)
                .child(center)
                .child(right_controls),
        )
        .when_some(bottom_bar, |d, b| d.child(b))
        .into_any_element()
}

// =========================================================================
// 2. Separate Floating Settings & Model Popover (fixed 382 x 384 px)
// =========================================================================

pub fn render_settings_popover(cx: &mut App) -> AnyElement {
    let state = cx.global::<VoiceState>();
    let cfg = state.config.clone();
    let backend = state.backend_label.clone();
    let autostart = state.autostart;

    let surface = ca(0x13131af8);
    let border = ca(0xffffff1e);
    let text_hi = c(0xf4f4f6);
    let text_mid = c(0xa1a1aa);
    let text_dim = c(0x71717a);
    let indigo = c(0x818cf8);
    let indigo_bg = ca(0x6366f12a);
    let emerald = c(0x4ade80);
    let emerald_bg = ca(0x4ade8022);

    let header = div()
        .h(px(28.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_hi)
                .child("Models & Settings"),
        )
        .child(
            div()
                .px(px(7.))
                .py(px(1.))
                .rounded(px(999.))
                .bg(ca(0xffffff0c))
                .text_size(px(10.5))
                .text_color(text_mid)
                .child(backend),
        )
        .child(
            div()
                .id("settings-drag")
                .flex_1()
                .h_full()
                .window_control_area(WindowControlArea::Drag),
        )
        .child(
            div()
                .id("settings-close")
                .size(px(24.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(ca(0xffffff18)))
                .child(icon("x", 13., text_mid))
                .on_click(|_, window, cx| close_settings_from_window(window, cx)),
        );

    let models_list = div()
        .flex()
        .flex_col()
        .gap(px(5.))
        .children(ModelKind::ALL.into_iter().enumerate().map(move |(i, kind)| {
            let active = kind == cfg.model;
            let cached = is_model_ready(kind);
            div()
                .id(ElementId::Name(SharedString::from(format!("set-m-{i}"))))
                .px(px(10.))
                .py(px(7.))
                .rounded(px(10.))
                .bg(if active { indigo_bg } else { ca(0xffffff07) })
                .border_1()
                .border_color(if active {
                    ca(0x818cf860)
                } else {
                    ca(0xffffff0f)
                })
                .cursor_pointer()
                .hover(|s| s.bg(ca(0xffffff16)))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(9.))
                .child(
                    div()
                        .size(px(7.))
                        .rounded_full()
                        .bg(if active { indigo } else { ca(0xffffff28) }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_hi)
                                .child(kind.display_name()),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(text_mid)
                                .truncate()
                                .child(kind.subtitle()),
                        ),
                )
                .child(
                    div()
                        .px(px(7.))
                        .py(px(2.))
                        .rounded(px(999.))
                        .bg(if cached { emerald_bg } else { ca(0xffffff0e) })
                        .text_size(px(10.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if cached { emerald } else { text_mid })
                        .child(if cached {
                            "Ready".to_string()
                        } else {
                            format!("↓ {}", kind.size_label())
                        }),
                )
                .on_click(move |_, window, cx| select_model_from_settings(window, cx, kind))
        }));

    // Language row
    let lang_row = if cfg.model.supports_language_choice() {
        let langs: [(&str, &str); 3] = [("en-US", "English"), ("hi-IN", "Hindi"), ("auto", "Auto")];
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .pt(px(4.))
            .child(
                div()
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(text_mid)
                    .child("Language"),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(4.))
                    .p(px(2.))
                    .rounded(px(8.))
                    .bg(ca(0xffffff08))
                    .border_1()
                    .border_color(ca(0xffffff12))
                    .children(langs.into_iter().map(move |(code, label)| {
                        let on = cfg.language == code;
                        div()
                            .id(ElementId::Name(SharedString::from(format!("lang-{code}"))))
                            .px(px(10.))
                            .py(px(3.))
                            .rounded(px(6.))
                            .bg(if on { indigo_bg } else { ca(0x00000000) })
                            .text_size(px(11.5))
                            .font_weight(if on {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .text_color(if on { text_hi } else { text_mid })
                            .cursor_pointer()
                            .hover(|s| s.bg(ca(0xffffff14)))
                            .child(label)
                            .on_click(move |_, _, cx| select_language_from_settings(cx, code))
                    })),
            )
    } else {
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .pt(px(4.))
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(text_mid)
                    .child("Language"),
            )
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(text_dim)
                    .child("English-only model (locked)"),
            )
    };

    // Quick bottom toggles
    let toggle_chip = |id: &'static str, label: &'static str, on: bool| {
        div()
            .id(id)
            .flex_1()
            .h(px(30.))
            .px(px(8.))
            .rounded(px(8.))
            .bg(if on { indigo_bg } else { ca(0xffffff07) })
            .border_1()
            .border_color(if on { ca(0x818cf855) } else { ca(0xffffff12) })
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap(px(5.))
            .cursor_pointer()
            .hover(|s| s.bg(ca(0xffffff16)))
            .child(
                div()
                    .size(px(6.))
                    .rounded_full()
                    .bg(if on { emerald } else { text_dim }),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if on { text_hi } else { text_mid })
                    .child(label),
            )
    };

    let toggles_row = div()
        .pt(px(4.))
        .border_t_1()
        .border_color(ca(0xffffff12))
        .flex()
        .flex_row()
        .gap(px(6.))
        .child(
            toggle_chip("tog-paste", "Auto-Paste", cfg.auto_paste)
                .on_click(|_, _, cx| toggle_auto_paste_from_ui(cx)),
        )
        .child(
            toggle_chip("tog-boot", "At Sign-In", autostart)
                .on_click(|_, _, cx| toggle_autostart_from_ui(cx)),
        )
        .child(
            toggle_chip("tog-gpu", "GPU", cfg.use_gpu)
                .on_click(|_, _, cx| toggle_gpu_from_ui(cx)),
        );

    div()
        .size_full()
        .font_family(ui_font())
        .p(px(14.))
        .rounded(px(16.))
        .bg(surface)
        .border_1()
        .border_color(border)
        .flex()
        .flex_col()
        .gap(px(9.))
        .child(header)
        .child(models_list)
        .child(lang_row)
        .child(toggles_row)
        .into_any_element()
}

fn live_waveform(bars: [f32; WAVEFORM_BARS], indigo: Hsla, violet: Hsla) -> Div {
    // Show the 12 newest bars cleanly in the capsule.
    let slice = &bars[WAVEFORM_BARS.saturating_sub(12)..];
    div()
        .h(px(26.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(2.5))
        .children(slice.iter().copied().enumerate().map(move |(i, v)| {
            let h = (4.0 + v.clamp(0.05, 1.0) * 20.0).round();
            div()
                .w(px(3.))
                .h(px(h))
                .rounded(px(999.))
                .bg(if i % 2 == 0 { indigo } else { violet })
                .opacity(0.45 + v.clamp(0.0, 1.0) * 0.55)
        }))
}

fn breathing_wave(tick: u64, indigo: Hsla, violet: Hsla) -> Div {
    const N: usize = 10;
    let phase = tick as f32 * 0.24;
    div()
        .h(px(22.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(2.5))
        .children((0..N).map(move |i| {
            let s = (phase - i as f32 * 0.48).sin() * 0.5 + 0.5;
            let h = (4.0 + s * 14.0).round();
            div()
                .w(px(3.))
                .h(px(h))
                .rounded(px(999.))
                .bg(if i % 2 == 0 { indigo } else { violet })
                .opacity(0.35 + s * 0.65)
        }))
}
