//! Two-window floating GPUI Kit UI for Voicebar:
//! 1. `render_pill`: ultra-clean single-surface Dynamic Island capsule (fixed 548x68 px, never stretched).
//! 2. `render_settings_popover`: dedicated floating Settings & Model card anchored below.

use crate::audio::WAVEFORM_BARS;
use crate::config::{ModelKind, is_model_ready};
use crate::{
    Phase, VoiceState, cancel_dictation_from_window, close_settings_from_window,
    dismiss_overlay_from_window, finish_dictation_from_window, select_language_from_settings,
    select_model_from_settings, select_rewrite_language_from_ui, select_rewrite_model_from_ui,
    toggle_auto_paste_from_ui, toggle_autostart_from_ui, toggle_gpu_from_ui,
    toggle_pause_dictation, toggle_personal_rewrite_from_ui, toggle_settings_popover,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

pub const PILL_WIDTH: f32 = 596.0;
pub const PILL_HEIGHT: f32 = 68.0;
pub const SETTINGS_WIDTH: f32 = 404.0;
pub const SETTINGS_HEIGHT: f32 = 504.0;

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

pub fn icon(name: &str, sz: f32, color: Hsla) -> Svg {
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

fn pill_switch(id: &'static str, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(34.))
        .h(px(19.))
        .p(px(2.))
        .rounded(px(999.))
        .bg(if on { c(0x6366f1) } else { ca(0xffffff1a) })
        .border_1()
        .border_color(if on { ca(0x818cf888) } else { ca(0xffffff20) })
        .flex()
        .items_center()
        .when(on, |d| d.justify_end())
        .when(!on, |d| d.justify_start())
        .cursor_pointer()
        .child(div().size(px(13.)).rounded_full().bg(c(0xffffff)))
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
    let is_rewriting = state.is_rewriting();
    let fell_back = state.rewrite_fell_back;
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
        Phase::Paused { .. } => ca(0xfbbf2470),
        Phase::Loading { .. } | Phase::Downloading { .. } => ca(0x818cf842),
        Phase::Finalizing if is_rewriting => ca(0xa78bfa55),
        Phase::Copied { .. } if fell_back => ca(0xfbbf2466),
        Phase::Copied { .. } => ca(0x4ade8066),
        Phase::Error(_) => ca(0xf8717166),
        _ => ca(0xffffff1c),
    };

    // Optional razor-thin progress/shimmer bar along the bottom edge (loading / downloading / rewriting).
    let bottom_bar: Option<Div> = match &phase {
        Phase::Loading { .. } | Phase::Finalizing if !matches!(phase, Phase::Finalizing) || is_rewriting => {
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
                            .bg(if is_rewriting { violet } else { indigo }),
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
        Phase::Paused { .. } => div()
            .id("pill-drag-paused")
            .h_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .window_control_area(WindowControlArea::Drag)
            .child(div().size(px(8.)).rounded_full().bg(amber))
            .child(paused_waveform(amber))
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
                    .bg(if matches!(phase, Phase::Finalizing) && !is_rewriting {
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
        Phase::Copied { .. } if fell_back => div()
            .size(px(26.))
            .rounded_full()
            .bg(ca(0xfbbf2422))
            .flex()
            .items_center()
            .justify_center()
            .child(icon("check", 14., amber))
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
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(text_hi)
                            .child("Listening…"),
                    )
                    .child(
                        div()
                            .px(px(7.))
                            .py(px(2.))
                            .rounded(px(6.))
                            .bg(if cfg.personal_rewrite {
                                ca(0x6366f124)
                            } else {
                                ca(0xffffff0e)
                            })
                            .border_1()
                            .border_color(if cfg.personal_rewrite {
                                ca(0x818cf848)
                            } else {
                                ca(0xffffff18)
                            })
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.))
                            .when(cfg.personal_rewrite, |d| d.child(icon("sparkles", 10.5, indigo)))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(if cfg.personal_rewrite { indigo } else { text_mid })
                                    .child(if cfg.personal_rewrite {
                                        "F1 · Rewrite"
                                    } else if cfg.auto_paste {
                                        "F1 to paste"
                                    } else {
                                        "F1 to copy"
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(text_dim)
                            .truncate()
                            .child(cfg.model.short_badge()),
                    )
                    .into_any_element()
            } else {
                let (clipped, shown) = visible_tail(&transcript, 108);
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
                            .text_size(px(14.))
                            .line_height(px(20.))
                            .text_color(text_hi)
                            .child(format!("{display}{}", if pulse_on { " ▍" } else { "" })),
                    )
                    .into_any_element()
            }
        }
        Phase::Paused { .. } => {
            let preview = if transcript.trim().is_empty() {
                "Mic paused · click Resume or press F1".to_string()
            } else {
                let (clipped, shown) = visible_tail(&transcript, 74);
                if clipped {
                    format!("… {shown}")
                } else {
                    shown
                }
            };
            div()
                .id("pill-drag-paused-txt")
                .flex_1()
                .min_w(px(0.))
                .h_full()
                .overflow_hidden()
                .window_control_area(WindowControlArea::Drag)
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .flex_none()
                        .px(px(7.))
                        .py(px(2.))
                        .rounded(px(6.))
                        .bg(ca(0xfbbf2424))
                        .border_1()
                        .border_color(ca(0xfbbf2455))
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(amber)
                        .child("Paused"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_size(px(13.))
                        .text_color(if transcript.trim().is_empty() {
                            text_mid
                        } else {
                            text_hi
                        })
                        .truncate()
                        .child(preview),
                )
                .into_any_element()
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
            .flex_row()
            .items_center()
            .gap(px(6.))
            .when(is_rewriting, |d| d.child(icon("sparkles", 13., violet)))
            .child(div().text_size(px(13.5)).text_color(text_mid).child(
                if is_rewriting {
                    format!("Polishing with {}…", cfg.rewrite_model.short_label())
                } else {
                    "Finalizing…".to_string()
                },
            ))
            .into_any_element(),
        Phase::Copied { text, pasted } => {
            let label = if fell_back {
                "Original copied"
            } else if *pasted {
                "Pasted"
            } else {
                "Copied"
            };
            let label_color = if fell_back { amber } else { emerald };
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
                        .text_color(label_color)
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

    // Right controls: Pause/Resume (while dictating), Paste/Done, Settings popover, and Cancel/Close.
    // Uses on_mouse_down + cx.defer so the very first click on an unfocused popup always fires immediately.
    let is_listening = matches!(phase, Phase::Listening { .. });
    let is_paused = matches!(phase, Phase::Paused { .. });

    let mut right_controls = div()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.));

    if is_listening {
        right_controls = right_controls.child(
            div()
                .id("pill-pause-btn")
                .h(px(28.))
                .px(px(10.))
                .rounded_full()
                .bg(ca(0xfbbf241c))
                .border_1()
                .border_color(ca(0xfbbf244a))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.))
                .cursor_pointer()
                .hover(|s| s.bg(ca(0xfbbf2430)))
                .child(icon("pause", 12., amber))
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(c(0xfef3c7))
                        .child("Pause"),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                    cx.defer(toggle_pause_dictation);
                }),
        );
    } else if is_paused {
        right_controls = right_controls
            .child(
                div()
                    .id("pill-resume-btn")
                    .h(px(28.))
                    .px(px(10.))
                    .rounded_full()
                    .bg(ca(0x4ade8024))
                    .border_1()
                    .border_color(ca(0x4ade8055))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.))
                    .cursor_pointer()
                    .hover(|s| s.bg(ca(0x4ade8038)))
                    .child(icon("play", 12., emerald))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(0xdcfce7))
                            .child("Resume"),
                    )
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                        cx.defer(toggle_pause_dictation);
                    }),
            )
            .child(
                div()
                    .id("pill-done-btn")
                    .size(px(28.))
                    .rounded_full()
                    .bg(ca(0x6366f130))
                    .border_1()
                    .border_color(ca(0x818cf866))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(ca(0x6366f14a)))
                    .child(icon("check", 13.5, c(0xe0e7ff)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                        cx.defer(finish_dictation_from_window);
                    }),
            );
    }

    let right_controls = right_controls
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
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                    cx.defer(toggle_settings_popover);
                }),
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
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    cx.stop_propagation();
                    let is_active = matches!(
                        cx.global::<VoiceState>().phase,
                        Phase::Listening { .. }
                            | Phase::Paused { .. }
                            | Phase::Loading { .. }
                            | Phase::Downloading { .. }
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
// 2. Separate Floating Settings & Model Popover (fixed 404 x 496 px)
// =========================================================================

pub fn render_settings_popover(cx: &mut App) -> AnyElement {
    let state = cx.global::<VoiceState>();
    let cfg = state.config.clone();
    let backend = state.backend_label.clone();
    let autostart = state.autostart;
    let has_profile = cfg.has_valid_profile();
    let example_count = cfg
        .rewrite_profile
        .as_ref()
        .map(|p| p.examples().len())
        .unwrap_or(0);

    let surface = ca(0x111119f9);
    let card_bg = ca(0xffffff06);
    let border = ca(0xffffff1a);
    let text_hi = c(0xf4f4f6);
    let text_mid = c(0xa1a1aa);
    let text_dim = c(0x71717a);
    let indigo = c(0x818cf8);
    let indigo_bg = ca(0x6366f128);
    let emerald = c(0x4ade80);
    let emerald_bg = ca(0x4ade8020);

    let header = div()
        .h(px(26.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .size(px(22.))
                .rounded(px(7.))
                .bg(indigo_bg)
                .border_1()
                .border_color(ca(0x818cf848))
                .flex()
                .items_center()
                .justify_center()
                .child(icon("mic", 12., indigo)),
        )
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_hi)
                .child("Voicebar"),
        )
        .child(
            div()
                .px(px(7.))
                .py(px(1.5))
                .rounded(px(999.))
                .bg(ca(0xffffff0c))
                .text_size(px(10.))
                .font_weight(FontWeight::MEDIUM)
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
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    cx.stop_propagation();
                    close_settings_from_window(window, cx);
                }),
        );

    let models_header = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_dim)
                .child("SPEECH MODEL · LOCAL GPU"),
        );

    let models_list = div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .children(ModelKind::ALL.into_iter().enumerate().map(move |(i, kind)| {
            let active = kind == cfg.model;
            let cached = is_model_ready(kind);
            div()
                .id(ElementId::Name(SharedString::from(format!("set-m-{i}"))))
                .px(px(10.))
                .py(px(6.5))
                .rounded(px(10.))
                .bg(if active { indigo_bg } else { card_bg })
                .border_1()
                .border_color(if active {
                    ca(0x818cf860)
                } else {
                    ca(0xffffff0e)
                })
                .cursor_pointer()
                .hover(|s| s.bg(ca(0xffffff14)))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(9.))
                .child(
                    div()
                        .size(px(7.))
                        .rounded_full()
                        .bg(if active { indigo } else { ca(0xffffff26) }),
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
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    cx.stop_propagation();
                    select_model_from_settings(window, cx, kind);
                })
        }));

    // Spoken language row
    let lang_row = if cfg.model.supports_language_choice() {
        let langs: [(&str, &str); 4] = [
            ("en-US", "EN"),
            ("en-GB", "EN-IN"),
            ("hi-IN", "Hindi"),
            ("auto", "Auto"),
        ];
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(text_mid)
                            .child("Spoken"),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(text_dim)
                            .child("(+8 Indic in tray)"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(3.))
                    .p(px(2.))
                    .rounded(px(8.))
                    .bg(ca(0xffffff08))
                    .border_1()
                    .border_color(ca(0xffffff12))
                    .children(langs.into_iter().map(move |(code, label)| {
                        let on = cfg.language == code;
                        div()
                            .id(ElementId::Name(SharedString::from(format!("lang-{code}"))))
                            .px(px(8.))
                            .py(px(2.5))
                            .rounded(px(6.))
                            .bg(if on { indigo_bg } else { ca(0x00000000) })
                            .text_size(px(11.))
                            .font_weight(if on {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .text_color(if on { text_hi } else { text_mid })
                            .cursor_pointer()
                            .hover(|s| s.bg(ca(0xffffff14)))
                            .child(label)
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                cx.stop_propagation();
                                select_language_from_settings(cx, code);
                            })
                    })),
            )
    } else {
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(text_mid)
                    .child("Spoken"),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(text_dim)
                    .child("English-only model (locked)"),
            )
    };

    // Dedicated Optional Rephrasing Card — crystal-clear ON/OFF switch, zero layout cramping!
    let rewrite_on = cfg.personal_rewrite;
    let rewrite_model = cfg.rewrite_model;
    let rewrite_ready =
        crate::rewrite::check_readiness(rewrite_model) == crate::llama_engine::RuntimeReadiness::Ready;
    let rewrite_status = if !rewrite_on {
        ("OFF".to_string(), ca(0xffffff0e), text_dim)
    } else if !rewrite_ready {
        ("Needs download".to_string(), ca(0xfbbf2422), c(0xfbbf24))
    } else if has_profile {
        (
            format!("Profile · {example_count}"),
            emerald_bg,
            emerald,
        )
    } else {
        ("llama.cpp".to_string(), indigo_bg, indigo)
    };

    let rewrite_card = div()
        .p(px(10.))
        .rounded(px(12.))
        .bg(if rewrite_on {
            ca(0x6366f110)
        } else {
            ca(0xffffff05)
        })
        .border_1()
        .border_color(if rewrite_on {
            ca(0x818cf844)
        } else {
            ca(0xffffff12)
        })
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.))
                        .child(icon(
                            "sparkles",
                            13.,
                            if rewrite_on { indigo } else { text_dim },
                        ))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_hi)
                                .child("Optional Rephrasing"),
                        )
                        .child(
                            div()
                                .px(px(6.))
                                .py(px(1.))
                                .rounded(px(999.))
                                .bg(rewrite_status.1)
                                .text_size(px(9.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rewrite_status.2)
                                .child(rewrite_status.0),
                        ),
                )
                .child(
                    pill_switch("tog-rewrite", rewrite_on).on_mouse_down(
                        MouseButton::Left,
                        |_, _, cx| {
                            cx.stop_propagation();
                            toggle_personal_rewrite_from_ui(cx);
                        },
                    ),
                ),
        )
        .when(!rewrite_on, |card| {
            card.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(px(10.5))
                            .text_color(text_dim)
                            .truncate()
                            .child("Off — direct GPU dictation without LLM editing"),
                    )
                    .child(
                        div()
                            .id("open-profile-setup")
                            .px(px(9.))
                            .py(px(4.))
                            .rounded(px(7.))
                            .bg(ca(0xffffff0c))
                            .border_1()
                            .border_color(ca(0xffffff18))
                            .text_size(px(10.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(text_hi)
                            .cursor_pointer()
                            .hover(|s| s.bg(ca(0xffffff18)))
                            .child(if has_profile {
                                "Manage"
                            } else {
                                "Set up…"
                            })
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                cx.stop_propagation();
                                close_settings_from_window(window, cx);
                                cx.defer(crate::profile_setup::open);
                            }),
                    ),
            )
        })
        .when(rewrite_on, |card| {
            let next_model = rewrite_model.next();
            let out_langs: [(&str, &str); 4] = [
                ("", "Same"),
                ("en", "EN"),
                ("hi", "Hindi"),
                ("pa", "Punjabi"),
            ];
            card.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(6.))
                    .child(
                        div()
                            .id("cycle-rewrite-model")
                            .flex_1()
                            .min_w(px(0.))
                            .h(px(26.))
                            .px(px(8.))
                            .rounded(px(7.))
                            .bg(ca(0xffffff0a))
                            .border_1()
                            .border_color(ca(0xffffff16))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(px(6.))
                            .cursor_pointer()
                            .hover(|s| s.bg(ca(0xffffff16)))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(5.))
                                    .min_w(px(0.))
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(text_dim)
                                            .child("LLM:"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(text_hi)
                                            .truncate()
                                            .child(rewrite_model.short_label()),
                                    )
                                    .when(rewrite_model.is_indic_focused(), |d| {
                                        d.child(
                                            div()
                                                .px(px(5.))
                                                .py(px(0.5))
                                                .rounded(px(999.))
                                                .bg(ca(0xa78bfa24))
                                                .text_size(px(9.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(c(0xc4b5fd))
                                                .child("Indic"),
                                        )
                                    }),
                            )
                            .child(icon("chevron-right", 12., text_dim))
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                cx.stop_propagation();
                                select_rewrite_model_from_ui(cx, next_model);
                            }),
                    )
                    .child(
                        div()
                            .id("open-profile-setup")
                            .h(px(26.))
                            .px(px(9.))
                            .rounded(px(7.))
                            .bg(indigo_bg)
                            .border_1()
                            .border_color(ca(0x818cf850))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.))
                            .cursor_pointer()
                            .hover(|s| s.bg(ca(0x6366f140)))
                            .child(icon("sliders-horizontal", 11., indigo))
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(text_hi)
                                    .child(if has_profile { "Profile" } else { "Set up" }),
                            )
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                cx.stop_propagation();
                                close_settings_from_window(window, cx);
                                cx.defer(crate::profile_setup::open);
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(text_mid)
                            .child("Output"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(3.))
                            .p(px(2.))
                            .rounded(px(7.))
                            .bg(ca(0xffffff08))
                            .border_1()
                            .border_color(ca(0xffffff12))
                            .children(out_langs.into_iter().map(move |(code, label)| {
                                let on = cfg.rewrite_language == code;
                                div()
                                    .id(ElementId::Name(SharedString::from(format!(
                                        "rewrite-lang-{code}"
                                    ))))
                                    .px(px(7.5))
                                    .py(px(2.))
                                    .rounded(px(5.))
                                    .bg(if on { indigo_bg } else { ca(0x00000000) })
                                    .text_size(px(10.5))
                                    .font_weight(if on {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .text_color(if on { text_hi } else { text_mid })
                                    .cursor_pointer()
                                    .hover(|s| s.bg(ca(0xffffff14)))
                                    .child(label)
                                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                        cx.stop_propagation();
                                        select_rewrite_language_from_ui(cx, code);
                                    })
                            })),
                    ),
            )
        });

    // Quick bottom toggles
    let toggle_chip = |id: &'static str, label: &'static str, on: bool| {
        div()
            .id(id)
            .flex_1()
            .h(px(30.))
            .px(px(8.))
            .rounded(px(8.))
            .bg(if on { indigo_bg } else { card_bg })
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
        .pt(px(6.))
        .border_t_1()
        .border_color(ca(0xffffff10))
        .flex()
        .flex_row()
        .gap(px(6.))
        .child(
            toggle_chip("tog-paste", "Auto-Paste", cfg.auto_paste).on_mouse_down(
                MouseButton::Left,
                |_, _, cx| {
                    cx.stop_propagation();
                    toggle_auto_paste_from_ui(cx);
                },
            ),
        )
        .child(
            toggle_chip("tog-boot", "At Sign-In", autostart).on_mouse_down(
                MouseButton::Left,
                |_, _, cx| {
                    cx.stop_propagation();
                    toggle_autostart_from_ui(cx);
                },
            ),
        )
        .child(
            toggle_chip("tog-gpu", "GPU", cfg.use_gpu).on_mouse_down(
                MouseButton::Left,
                |_, _, cx| {
                    cx.stop_propagation();
                    toggle_gpu_from_ui(cx);
                },
            ),
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
        .gap(px(8.))
        .child(header)
        .child(models_header)
        .child(models_list)
        .child(lang_row)
        .child(rewrite_card)
        .child(div().flex_1())
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

fn paused_waveform(amber: Hsla) -> Div {
    const HEIGHTS: [f32; 10] = [4., 6., 5., 8., 6., 4., 7., 5., 6., 4.];
    div()
        .h(px(22.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(2.5))
        .children(HEIGHTS.into_iter().map(move |h| {
            div()
                .w(px(3.))
                .h(px(h))
                .rounded(px(999.))
                .bg(amber)
                .opacity(0.45)
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
