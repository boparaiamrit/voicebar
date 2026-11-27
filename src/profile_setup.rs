//! Dark-glass Personal Rewrite Studio for optional local dictation rephrasing & translation.
//! Clean 3-zone app shell (Fixed Header + Overflow-Safe Scroll Body + Sticky Footer):
//! - Uses Voicebar's built-in `llama.cpp` (Vulkan / Metal / CPU) engine — zero Ollama required.
//! - Ships Sarvam Translate 4B (default, 22 Indian languages + English & code-mixing) alongside
//!   ultra-tiny 378 MB Qwen 3 0.6B through Sarvam M 24B directly from Hugging Face GGUFs.
//! - Clean 2-column history picker, zero text overflow, responsive first-click controls, and sticky footer CTAs.

use crate::config::{RewriteModel, is_rewrite_model_downloaded};
use crate::history_import::{self, Source};
use crate::llama_engine::{self, RuntimeReadiness};
use crate::overlay::icon;
use crate::personalization::{
    PREVIEW_COUNT, ProfilePreview, RewritePair, SetupDraft, count_paste_entries,
};
use crate::{VoiceState, hotkey, rewrite};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct SetupWindow(Option<AnyWindowHandle>);
impl Global for SetupWindow {}

pub fn init(cx: &mut App) {
    cx.set_global(SetupWindow(None));
}

pub fn open(cx: &mut App) {
    if let Some(handle) = cx.global::<SetupWindow>().0 {
        if handle
            .update(cx, |_, window, _| {
                hotkey::restore_minimized_windows();
                window.activate_window();
            })
            .is_ok()
        {
            cx.activate(true);
            return;
        }
        cx.global_mut::<SetupWindow>().0 = None;
    }
    let bounds = Bounds::centered(None, size(px(920.), px(700.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(700.), px(540.))),
        titlebar: Some(TitlebarOptions {
            title: Some("Voicebar · Personal Rewrite Studio".into()),
            ..Default::default()
        }),
        app_id: Some("dev.voicebar.studio".into()),
        ..Default::default()
    };
    match gpui_kit::open_window(options, cx, |window, cx| {
        crate::apply_dark_theme(cx);
        window
            .observe_window_appearance(|_window, cx| crate::apply_dark_theme(cx))
            .detach();
        cx.new(|cx| Setup::new(window, cx))
    }) {
        Ok((handle, _)) => {
            cx.global_mut::<SetupWindow>().0 = Some(handle);
            cx.activate(true);
        }
        Err(_) => eprintln!("Voicebar could not open the Personal Rewrite Studio."),
    }
}

fn c(x: u32) -> Hsla {
    rgb(x).into()
}
fn ca(x: u32) -> Hsla {
    rgba(x).into()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Start,
    Paste,
    Sources,
    Summary,
    Review,
    Fresh,
    Compare,
    Done,
}

impl Page {
    fn step_index(self) -> Option<usize> {
        match self {
            Page::Start => None,
            Page::Paste | Page::Sources | Page::Summary => Some(0),
            Page::Review => Some(1),
            Page::Fresh | Page::Compare => Some(2),
            Page::Done => Some(3),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModelStatus {
    Ready,
    ModelMissing,
    EngineMissing,
}

enum Failure {
    MissingModel,
    Message(&'static str),
}

fn check_model(model: RewriteModel) -> Result<(), Failure> {
    if rewrite::check_readiness(model) == RuntimeReadiness::Ready {
        Ok(())
    } else {
        Err(Failure::MissingModel)
    }
}

fn readiness_to_status(r: RuntimeReadiness) -> ModelStatus {
    match r {
        RuntimeReadiness::Ready => ModelStatus::Ready,
        RuntimeReadiness::ModelMissing => ModelStatus::ModelMissing,
        RuntimeReadiness::EngineMissing => ModelStatus::EngineMissing,
    }
}

struct Setup {
    page: Page,
    paste: Entity<TextareaState>,
    own: Entity<TextareaState>,
    fresh: Entity<TextareaState>,
    sources: Vec<(Source, bool)>,
    source_page: Page,
    summary: String,
    draft: Option<SetupDraft>,
    suggestions: Option<(String, String)>,
    editing: bool,
    preview: Option<ProfilePreview>,
    comparison_index: usize,
    model_status: ModelStatus,
    confirm_clear: bool,
    saved_example_count: usize,
    busy: Option<String>,
    pull_progress: Arc<Mutex<Option<rewrite::PullProgress>>>,
    cancel: Option<Arc<AtomicBool>>,
    problem: Option<Failure>,
}

impl Drop for Setup {
    fn drop(&mut self) {
        self.stop_job();
    }
}

impl Setup {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let active = cx.global::<VoiceState>().config.rewrite_model;
        Self {
            page: Page::Start,
            paste: cx.new(|cx| TextareaState::new(window, cx)),
            own: cx.new(|cx| TextareaState::new(window, cx)),
            fresh: cx.new(|cx| TextareaState::new(window, cx)),
            sources: Vec::new(),
            source_page: Page::Paste,
            summary: String::new(),
            draft: None,
            suggestions: None,
            editing: false,
            preview: None,
            comparison_index: 0,
            model_status: readiness_to_status(rewrite::check_readiness(active)),
            confirm_clear: false,
            saved_example_count: 0,
            busy: None,
            pull_progress: Arc::new(Mutex::new(None)),
            cancel: None,
            problem: None,
        }
    }

    fn active_model(&self, cx: &App) -> RewriteModel {
        cx.global::<VoiceState>().config.rewrite_model
    }

    fn probe_active_model(&mut self, cx: &mut Context<Self>) {
        let model = self.active_model(cx);
        self.model_status = readiness_to_status(rewrite::check_readiness(model));
        cx.notify();
    }

    fn select_model(&mut self, model: RewriteModel, cx: &mut Context<Self>) {
        crate::select_rewrite_model_from_ui(cx, model);
        self.problem = None;
        self.probe_active_model(cx);
        cx.notify();
    }

    fn stop_job(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        if let Ok(mut guard) = self.pull_progress.lock() {
            *guard = None;
        }
        self.busy = None;
    }

    fn show(&mut self, page: Page, cx: &mut Context<Self>) {
        self.stop_job();
        self.problem = None;
        self.confirm_clear = false;
        self.page = page;
        cx.notify();
    }

    fn fail(&mut self, error: &'static str, cx: &mut Context<Self>) {
        self.problem = Some(Failure::Message(error));
        cx.notify();
    }

    fn run<T: Send + 'static>(
        &mut self,
        status: impl Into<String>,
        cx: &mut Context<Self>,
        work: impl FnOnce(&AtomicBool) -> Result<T, Failure> + Send + 'static,
        apply: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        self.stop_job();
        self.problem = None;
        self.busy = Some(status.into());
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let worker_flag = cancel.clone();
        let task = cx.background_executor().spawn(async move {
            if worker_flag.load(Ordering::Acquire) {
                return Err(Failure::Message("Cancelled."));
            }
            work(&worker_flag)
        });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |view, cx| {
                if cancel.load(Ordering::Acquire) {
                    return;
                }
                view.busy = None;
                view.cancel = None;
                if let Ok(mut guard) = view.pull_progress.lock() {
                    *guard = None;
                }
                match result {
                    Ok(value) => apply(view, value, cx),
                    Err(error) => {
                        if matches!(error, Failure::MissingModel) {
                            let active = view.active_model(cx);
                            view.model_status =
                                readiness_to_status(rewrite::check_readiness(active));
                        }
                        view.problem = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn find_sources(&mut self, cx: &mut Context<Self>) {
        self.show(Page::Sources, cx);
        if !self.sources.is_empty() {
            return;
        }
        self.run(
            "Scanning local assistant folders (read-only)…",
            cx,
            |_| Ok(history_import::discover()),
            |view, sources, _| view.sources = sources.into_iter().map(|s| (s, false)).collect(),
        );
    }

    fn import(&mut self, cx: &mut Context<Self>) {
        let selected: Vec<_> = self
            .sources
            .iter()
            .filter(|(_, selected)| *selected)
            .map(|(source, _)| source.clone())
            .collect();
        if selected.is_empty() {
            self.fail("Select at least one history source to import.", cx);
            return;
        }
        self.run(
            format!(
                "Reading newest messages from {} selected source{}…",
                selected.len(),
                if selected.len() == 1 { "" } else { "s" }
            ),
            cx,
            move |cancel| {
                history_import::read_selected_cancellable(&selected, cancel)
                    .ok_or(Failure::Message("Import cancelled."))
            },
            |view, report, cx| {
                view.summary = report.summary;
                match SetupDraft::from_entries(report.entries) {
                    Ok(draft) => view.accept_input(draft, Page::Sources, cx),
                    Err(message) => view.fail(message, cx),
                }
            },
        );
    }

    fn accept_input(&mut self, draft: SetupDraft, source: Page, cx: &mut Context<Self>) {
        self.draft = Some(draft);
        self.source_page = source;
        self.preview = None;
        self.show(Page::Summary, cx);
    }

    fn accept_paste(&mut self, cx: &mut Context<Self>) {
        match SetupDraft::from_paste(&self.paste.read(cx).value()) {
            Ok(draft) => {
                self.summary.clear();
                self.accept_input(draft, Page::Paste, cx);
            }
            Err(message) => self.fail(message, cx),
        }
    }

    fn generate_suggestions(&mut self, cx: &mut Context<Self>) {
        self.suggestions = None;
        let Some(sample) = self
            .draft
            .as_ref()
            .and_then(SetupDraft::current)
            .map(str::to_owned)
        else {
            return;
        };
        let model = self.active_model(cx);
        let use_gpu = cx.global::<VoiceState>().config.use_gpu;
        self.run(
            format!("Generating two local rewrites with {}…", model.short_label()),
            cx,
            move |cancel| {
                check_model(model)?;
                let light = rewrite::rewrite_sample(model, use_gpu, &sample, "", None, true)
                    .map_err(Failure::Message)?;
                if cancel.load(Ordering::Acquire) {
                    return Err(Failure::Message("Cancelled."));
                }
                let natural = rewrite::rewrite_sample(model, use_gpu, &sample, "", None, false)
                    .map_err(Failure::Message)?;
                Ok((light, natural))
            },
            |view, suggestions, _| {
                view.model_status = ModelStatus::Ready;
                view.suggestions = Some(suggestions);
            },
        );
    }

    fn begin_review(&mut self, cx: &mut Context<Self>) {
        self.show(Page::Review, cx);
        self.editing = false;
        self.generate_suggestions(cx);
    }

    fn choose(&mut self, preferred: Option<String>, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        match draft.review(preferred.as_deref()) {
            Ok(()) if draft.current().is_none() => self.show(Page::Fresh, cx),
            Ok(()) => self.begin_review(cx),
            Err(message) => self.fail(message, cx),
        }
    }

    fn edit_own(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.draft.as_ref().and_then(SetupDraft::current) else {
            return;
        };
        self.own
            .update(cx, |area, cx| area.set_value(text.to_owned(), window, cx));
        self.editing = true;
        cx.notify();
    }

    fn compare(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.as_ref() else {
            return;
        };
        let inputs = match draft.preview_inputs(&self.fresh.read(cx).value()) {
            Ok(inputs) => inputs,
            Err(message) => {
                self.fail(message, cx);
                return;
            }
        };
        let model = self.active_model(cx);
        let use_gpu = cx.global::<VoiceState>().config.use_gpu;
        let language = cx.global::<VoiceState>().config.rewrite_language.clone();
        self.run(
            format!("Comparing 3 dictations locally with {}…", model.short_label()),
            cx,
            move |cancel| {
                check_model(model)?;
                let mut pairs = Vec::new();
                for original in &inputs.originals {
                    if cancel.load(Ordering::Acquire) {
                        return Err(Failure::Message("Cancelled."));
                    }
                    let standard =
                        rewrite::rewrite_sample(model, use_gpu, original, &language, None, false)
                            .map_err(Failure::Message)?;
                    if cancel.load(Ordering::Acquire) {
                        return Err(Failure::Message("Cancelled."));
                    }
                    let personalized = rewrite::rewrite_sample(
                        model,
                        use_gpu,
                        original,
                        &language,
                        Some(&inputs.profile),
                        false,
                    )
                    .map_err(Failure::Message)?;
                    pairs.push(RewritePair {
                        standard,
                        personalized,
                    });
                }
                ProfilePreview::complete(inputs, pairs).map_err(Failure::Message)
            },
            |view, preview, cx| {
                view.model_status = ModelStatus::Ready;
                view.preview = Some(preview);
                view.comparison_index = 0;
                view.show(Page::Compare, cx);
            },
        );
    }

    fn download(&mut self, cx: &mut Context<Self>) {
        let model = self.active_model(cx);
        let progress_slot = self.pull_progress.clone();
        let poll_cancel = Arc::new(AtomicBool::new(false));
        let poll_flag = poll_cancel.clone();
        cx.spawn(async move |view, cx| {
            while !poll_flag.load(Ordering::Acquire) {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let keep_going = view
                    .update(cx, |view, cx| {
                        let active = view.busy.is_some();
                        if active {
                            cx.notify();
                        }
                        active
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();

        let status_label = if self.model_status == ModelStatus::EngineMissing {
            format!("Downloading built-in llama.cpp + {}…", model.display_name())
        } else {
            format!("Downloading {} from Hugging Face…", model.display_name())
        };

        self.run(
            status_label,
            cx,
            move |cancel| {
                let res = rewrite::download_model_with_progress(model, cancel, |p| {
                    if let Ok(mut guard) = progress_slot.lock() {
                        *guard = Some(p);
                    }
                })
                .map_err(Failure::Message);
                poll_cancel.store(true, Ordering::Release);
                res
            },
            |view, _, cx| {
                view.probe_active_model(cx);
                match view.page {
                    Page::Review => view.generate_suggestions(cx),
                    Page::Fresh => view.compare(cx),
                    _ => {}
                }
            },
        );
    }

    fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preview) = self.preview.as_ref() else {
            return;
        };
        if self.comparison_index + 1 != preview.rewrites.len() {
            return;
        }
        let saved_count = preview.inputs.profile.examples().len();
        match crate::activate_rewrite_profile(cx, preview.inputs.profile.clone()) {
            Ok(()) => {
                *self = Self::new(window, cx);
                self.saved_example_count = saved_count;
                self.model_status = ModelStatus::Ready;
                self.show(Page::Done, cx);
            }
            Err(message) => self.fail(message, cx),
        }
    }

    fn restart_review(&mut self, cx: &mut Context<Self>) {
        if let Some(draft) = self.draft.as_mut() {
            draft.restart_review();
        }
        self.preview = None;
        self.begin_review(cx);
    }
}

// =========================================================================
// Dark-glass UI building blocks (strictly contrast-safe & overflow-safe)
// =========================================================================

#[derive(Clone, Copy, PartialEq, Eq)]
enum BtnTone {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

fn btn(
    cx: &Context<Setup>,
    id: &'static str,
    label: impl Into<SharedString>,
    tone: BtnTone,
    disabled: bool,
    full_width: bool,
    handle: impl Fn(&mut Setup, &mut Window, &mut Context<Setup>) + 'static,
) -> Stateful<Div> {
    let (bg, bg_hover, border, fg) = match tone {
        BtnTone::Primary => (c(0x6366f1), c(0x818cf8), ca(0x818cf890), c(0xffffff)),
        BtnTone::Secondary => (c(0x1f1f2e), c(0x2b2b40), ca(0xffffff24), c(0xf4f4f6)),
        BtnTone::Ghost => (ca(0x00000000), ca(0xffffff14), ca(0x00000000), c(0xd1d5db)),
        BtnTone::Danger => (ca(0xef444428), ca(0xef444442), ca(0xf8717160), c(0xfca5a5)),
    };

    div()
        .id(id)
        .flex_none()
        .h(px(34.))
        .px(px(14.))
        .rounded(px(9.))
        .bg(bg)
        .border_1()
        .border_color(border)
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .text_size(px(12.5))
        .font_weight(if tone == BtnTone::Primary {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::MEDIUM
        })
        .text_color(fg)
        .when(full_width, |d| d.w_full())
        .when(disabled, |d| d.opacity(0.42).cursor_default())
        .when(!disabled, |d| {
            d.cursor_pointer()
                .hover(move |s| s.bg(bg_hover))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _, window, cx| {
                        cx.stop_propagation();
                        handle(view, window, cx);
                    }),
                )
        })
        .child(label.into())
}

fn action(
    cx: &Context<Setup>,
    id: &'static str,
    label: impl Into<SharedString>,
    handle: impl Fn(&mut Setup, &mut Window, &mut Context<Setup>) + 'static,
) -> Stateful<Div> {
    btn(cx, id, label, BtnTone::Secondary, false, false, handle)
}

fn action_primary(
    cx: &Context<Setup>,
    id: &'static str,
    label: impl Into<SharedString>,
    handle: impl Fn(&mut Setup, &mut Window, &mut Context<Setup>) + 'static,
) -> Stateful<Div> {
    btn(cx, id, label, BtnTone::Primary, false, false, handle)
}

fn action_primary_full(
    cx: &Context<Setup>,
    id: &'static str,
    label: impl Into<SharedString>,
    handle: impl Fn(&mut Setup, &mut Window, &mut Context<Setup>) + 'static,
) -> Stateful<Div> {
    btn(cx, id, label, BtnTone::Primary, false, true, handle)
}

fn action_secondary_full(
    cx: &Context<Setup>,
    id: &'static str,
    label: impl Into<SharedString>,
    handle: impl Fn(&mut Setup, &mut Window, &mut Context<Setup>) + 'static,
) -> Stateful<Div> {
    btn(cx, id, label, BtnTone::Secondary, false, true, handle)
}

fn action_ghost(
    cx: &Context<Setup>,
    id: &'static str,
    label: impl Into<SharedString>,
    handle: impl Fn(&mut Setup, &mut Window, &mut Context<Setup>) + 'static,
) -> Stateful<Div> {
    btn(cx, id, label, BtnTone::Ghost, false, false, handle)
}

fn col() -> Div {
    div().w_full().min_w_0().flex().flex_col().gap(px(12.))
}

fn row() -> Div {
    div().min_w_0().flex().flex_row().items_center().gap(px(8.))
}

fn actions_bar() -> Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(8.))
}

fn subtle(message: impl Into<SharedString>) -> Div {
    div()
        .w_full()
        .min_w_0()
        .text_size(px(13.))
        .line_height(px(19.5))
        .text_color(c(0x9ca3af))
        .child(message.into())
}

fn badge_pill(label: impl Into<SharedString>, bg: Hsla, fg: Hsla) -> Div {
    div()
        .flex_none()
        .px(px(8.))
        .py(px(2.))
        .rounded(px(999.))
        .bg(bg)
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(fg)
        .child(label.into())
}

fn glass_card(
    eyebrow: &'static str,
    value: &str,
    accented: bool,
    icon_name: Option<&'static str>,
) -> Div {
    let indigo = c(0x818cf8);
    let text_hi = c(0xf4f4f6);
    let text_dim = c(0x9ca3af);
    div()
        .flex_1()
        .min_w_0()
        .w_full()
        .p(px(14.))
        .rounded(px(13.))
        .bg(if accented { ca(0x6366f114) } else { c(0x14141f) })
        .border_1()
        .border_color(if accented { ca(0x818cf855) } else { ca(0xffffff16) })
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            row()
                .when_some(icon_name, |d, ic| {
                    d.child(icon(ic, 13., if accented { indigo } else { text_dim }))
                })
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if accented { indigo } else { text_dim })
                        .child(eyebrow),
                ),
        )
        .child(
            div()
                .w_full()
                .min_w_0()
                .text_size(px(14.))
                .line_height(px(21.))
                .text_color(text_hi)
                .child(value.to_owned()),
        )
}

fn stepper(active: usize) -> Div {
    const STEPS: [&str; 4] = ["Samples", "Review", "Compare", "Active"];
    let mut bar = div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.));
    for (i, label) in STEPS.iter().enumerate() {
        let done = i < active;
        let current = i == active;
        let circle = div()
            .flex_none()
            .size(px(22.))
            .rounded_full()
            .bg(if done {
                ca(0x4ade8022)
            } else if current {
                c(0x6366f1)
            } else {
                ca(0xffffff0c)
            })
            .border_1()
            .border_color(if done {
                ca(0x4ade8066)
            } else if current {
                c(0x818cf8)
            } else {
                ca(0xffffff18)
            })
            .flex()
            .items_center()
            .justify_center()
            .child(if done {
                icon("check", 12., c(0x4ade80)).into_any_element()
            } else {
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if current { c(0xffffff) } else { c(0x71717a) })
                    .child(format!("{}", i + 1))
                    .into_any_element()
            });
        bar = bar.child(
            row()
                .flex_none()
                .gap(px(6.))
                .child(circle)
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(if current {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(if current {
                            c(0xf4f4f6)
                        } else if done {
                            c(0x4ade80)
                        } else {
                            c(0x71717a)
                        })
                        .child(*label),
                ),
        );
        if i + 1 < STEPS.len() {
            bar = bar.child(
                div()
                    .flex_1()
                    .min_w(px(12.))
                    .h(px(1.5))
                    .rounded_full()
                    .bg(if done { ca(0x4ade8055) } else { ca(0xffffff14) }),
            );
        }
    }
    bar
}

// =========================================================================
// Pages + Sticky Footers
// =========================================================================

impl Setup {
    fn title(&self) -> String {
        match self.page {
            Page::Start => "Personal Rewrite Studio".into(),
            Page::Paste => "Paste dictation samples".into(),
            Page::Sources => "Import local assistant histories".into(),
            Page::Summary => "Samples ready to review".into(),
            Page::Review => {
                if let Some(draft) = self.draft.as_ref() {
                    let (number, total) = draft.review_position();
                    format!("Choose your preferred wording · {number} of {total}")
                } else {
                    "Choose your preferred wording".into()
                }
            }
            Page::Fresh => "Test on three fresh dictations".into(),
            Page::Compare => format!(
                "Compare before activating · {} of {PREVIEW_COUNT}",
                self.comparison_index + 1
            ),
            Page::Done => "Personal profile active".into(),
        }
    }

    /// Actionable one-click setup banner whenever the built-in llama.cpp runtime or selected GGUF is not downloaded yet.
    fn engine_action_banner(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.busy.is_some() {
            return None;
        }
        let model = self.active_model(cx);
        let amber = c(0xfbbf24);
        let indigo = c(0x818cf8);
        match self.model_status {
            ModelStatus::EngineMissing => Some(
                div()
                    .w_full()
                    .min_w_0()
                    .p(px(12.))
                    .rounded(px(12.))
                    .bg(ca(0x6366f114))
                    .border_1()
                    .border_color(ca(0x818cf848))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(10.))
                            .child(icon("download", 16., indigo))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(c(0xf4f4f6))
                                            .child("Built-in llama.cpp engine (Zero Ollama required)"),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .min_w_0()
                                            .text_size(px(12.))
                                            .line_height(px(17.))
                                            .text_color(c(0xd1d5db))
                                            .child(format!(
                                                "One-click download fetches Voicebar's Vulkan/Metal llama.cpp runtime (~37 MB) + {} ({}) from GitHub & Hugging Face. Direct GPU dictation works without it.",
                                                model.display_name(),
                                                model.size_label()
                                            )),
                                    ),
                            ),
                    )
                    .child(action_primary(
                        cx,
                        "banner-download-engine-model",
                        "Download Engine + Model",
                        |s, _, cx| s.download(cx),
                    )),
            ),
            ModelStatus::ModelMissing => Some(
                div()
                    .w_full()
                    .min_w_0()
                    .p(px(12.))
                    .rounded(px(12.))
                    .bg(ca(0xfbbf2414))
                    .border_1()
                    .border_color(ca(0xfbbf2444))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(10.))
                            .child(icon("download", 16., amber))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(c(0xf4f4f6))
                                            .child(format!("{} is not downloaded yet", model.display_name())),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .min_w_0()
                                            .text_size(px(12.))
                                            .line_height(px(17.))
                                            .text_color(c(0xd1d5db))
                                            .child(format!(
                                                "Built-in llama.cpp is installed. Download {} ({}) directly from Hugging Face to run 100% locally.",
                                                model.display_name(),
                                                model.size_label()
                                            )),
                                    ),
                            ),
                    )
                    .child(action_primary(
                        cx,
                        "banner-download-model",
                        format!("Download {}", model.size_label()),
                        |s, _, cx| s.download(cx),
                    )),
            ),
            ModelStatus::Ready => None,
        }
    }

    fn start_page(&self, cx: &mut Context<Self>) -> Div {
        let cfg = cx.global::<VoiceState>().config.clone();
        let active_model = cfg.rewrite_model;
        let has_profile = cfg.has_valid_profile();
        let busy = self.busy.is_some();

        let indigo = c(0x818cf8);
        let violet = c(0xa78bfa);
        let emerald = c(0x4ade80);
        let amber = c(0xfbbf24);
        let cyan = c(0x38bdf8);
        let text_hi = c(0xf4f4f6);
        let text_mid = c(0xa1a1aa);
        let text_dim = c(0x71717a);

        let mut body = col().gap(px(14.)).child(subtle(
            "Optional local rephrasing and Indic translation powered by Voicebar's built-in llama.cpp engine (zero Ollama required). Switch Rephrasing OFF anytime to paste raw GPU speech-to-text directly.",
        ));

        // 1. Existing active profile card
        if let Some(profile) = cfg.rewrite_profile.as_ref().filter(|p| p.is_valid()) {
            let count = profile.examples().len();
            let mut pairs_preview = div().w_full().min_w_0().flex().flex_col().gap(px(6.));
            for ex in profile.examples().iter().take(3) {
                pairs_preview = pairs_preview.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .px(px(10.))
                        .py(px(7.))
                        .rounded(px(8.))
                        .bg(ca(0x00000038))
                        .border_1()
                        .border_color(ca(0xffffff0e))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(12.))
                                .text_color(text_mid)
                                .truncate()
                                .child(ex.original.clone()),
                        )
                        .child(icon("arrow-right", 12., indigo))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_hi)
                                .truncate()
                                .child(ex.preferred.clone()),
                        ),
                );
            }
            if count > 3 {
                pairs_preview = pairs_preview.child(
                    div()
                        .text_size(px(11.))
                        .text_color(text_dim)
                        .child(format!("+ {} more approved examples saved locally", count - 3)),
                );
            }

            let clear_label = if self.confirm_clear {
                "Confirm clear"
            } else {
                "Clear profile"
            };
            body = body.child(
                div()
                    .w_full()
                    .min_w_0()
                    .p(px(14.))
                    .rounded(px(13.))
                    .bg(ca(0x4ade800e))
                    .border_1()
                    .border_color(ca(0x4ade803a))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(px(10.))
                            .child(
                                row()
                                    .child(icon("shield-check", 15., emerald))
                                    .child(
                                        div()
                                            .text_size(px(13.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(text_hi)
                                            .child("Active Personal Profile"),
                                    )
                                    .child(badge_pill(
                                        format!(
                                            "{count} example{}",
                                            if count == 1 { "" } else { "s" }
                                        ),
                                        ca(0x4ade8022),
                                        emerald,
                                    ))
                                    .child(badge_pill(
                                        if cfg.personal_rewrite {
                                            "Rephrasing ON"
                                        } else {
                                            "Rephrasing OFF"
                                        },
                                        if cfg.personal_rewrite {
                                            ca(0x6366f130)
                                        } else {
                                            ca(0xffffff12)
                                        },
                                        if cfg.personal_rewrite { indigo } else { text_mid },
                                    )),
                            )
                            .child(btn(
                                cx,
                                "clear-profile",
                                clear_label,
                                if self.confirm_clear {
                                    BtnTone::Danger
                                } else {
                                    BtnTone::Ghost
                                },
                                false,
                                false,
                                |s, _, cx| {
                                    if !s.confirm_clear {
                                        s.confirm_clear = true;
                                        cx.notify();
                                        return;
                                    }
                                    s.confirm_clear = false;
                                    if let Err(err) = crate::clear_rewrite_profile(cx) {
                                        s.fail(err, cx);
                                    }
                                    cx.notify();
                                },
                            )),
                    )
                    .child(pairs_preview),
            );
        }

        // 2. Entry cards RIGHT ABOVE models OR clean compact model selector + entry cards
        let entry_heading = if has_profile {
            "REBUILD PROFILE FROM NEW SAMPLES"
        } else {
            "BUILD YOUR PERSONAL PROFILE"
        };

        body = body.child(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_dim)
                        .child(entry_heading),
                )
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .flex_row()
                        .gap(px(12.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .p(px(14.))
                                .rounded(px(13.))
                                .bg(c(0x14141f))
                                .border_1()
                                .border_color(ca(0xffffff16))
                                .flex()
                                .flex_col()
                                .justify_between()
                                .gap(px(10.))
                                .child(
                                    col()
                                        .gap(px(5.))
                                        .child(
                                            row()
                                                .child(
                                                    div()
                                                        .flex_none()
                                                        .size(px(28.))
                                                        .rounded(px(8.))
                                                        .bg(ca(0x6366f124))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .child(icon("file-text", 14., indigo)),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(14.))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(text_hi)
                                                        .child("Paste transcripts"),
                                                ),
                                        )
                                        .child(subtle(
                                            "Paste 1–50 rough dictations or messages you picked yourself.",
                                        )),
                                )
                                .child(action_primary_full(
                                    cx,
                                    "start-paste",
                                    "Paste samples",
                                    |s, _, cx| s.show(Page::Paste, cx),
                                )),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .p(px(14.))
                                .rounded(px(13.))
                                .bg(c(0x14141f))
                                .border_1()
                                .border_color(ca(0xffffff16))
                                .flex()
                                .flex_col()
                                .justify_between()
                                .gap(px(10.))
                                .child(
                                    col()
                                        .gap(px(5.))
                                        .child(
                                            row()
                                                .child(
                                                    div()
                                                        .flex_none()
                                                        .size(px(28.))
                                                        .rounded(px(8.))
                                                        .bg(ca(0xa78bfa22))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .child(icon("terminal", 14., violet)),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(14.))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(text_hi)
                                                        .child("Import local histories"),
                                                ),
                                        )
                                        .child(subtle(
                                            "Read-only scan of selected local assistants. Assistant replies excluded.",
                                        )),
                                )
                                .child(action_secondary_full(
                                    cx,
                                    "start-history",
                                    "Choose histories",
                                    |s, _, cx| s.find_sources(cx),
                                )),
                        ),
                ),
        );

        // 3. Local Text-Only GGUF Model Selector (built-in llama.cpp — from 378 MB up to Sarvam AI)
        let mut model_cards = div().w_full().min_w_0().flex().flex_col().gap(px(6.));
        for (i, model) in RewriteModel::ALL.into_iter().enumerate() {
            let selected = model == active_model;
            let weights_present = is_rewrite_model_downloaded(model);
            let show_install = selected && self.model_status != ModelStatus::Ready && !busy;
            let status_chip = if selected {
                match self.model_status {
                    ModelStatus::Ready => Some(badge_pill("Ready", ca(0x4ade8022), emerald)),
                    ModelStatus::ModelMissing => {
                        Some(badge_pill("Not downloaded", ca(0xfbbf2422), amber))
                    }
                    ModelStatus::EngineMissing => {
                        Some(badge_pill("Engine + Model", ca(0x6366f128), indigo))
                    }
                }
            } else if weights_present {
                Some(badge_pill("Downloaded", ca(0x4ade801c), emerald))
            } else {
                None
            };

            model_cards = model_cards.child(
                div()
                    .id(ElementId::Name(SharedString::from(format!(
                        "studio-llm-{i}"
                    ))))
                    .w_full()
                    .min_w_0()
                    .px(px(12.))
                    .py(px(9.))
                    .rounded(px(11.))
                    .bg(if selected {
                        ca(0x6366f11c)
                    } else {
                        c(0x14141f)
                    })
                    .border_1()
                    .border_color(if selected {
                        ca(0x818cf866)
                    } else {
                        ca(0xffffff12)
                    })
                    .cursor_pointer()
                    .hover(|s| s.bg(ca(0xffffff10)))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_none()
                            .size(px(8.))
                            .rounded_full()
                            .bg(if selected { indigo } else { ca(0xffffff24) }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                row()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(text_hi)
                                            .child(model.display_name()),
                                    )
                                    .when(model == RewriteModel::SarvamTranslate4b, |d| {
                                        d.child(badge_pill("Default · India", ca(0xa78bfa2c), violet))
                                    })
                                    .when(model.is_small_fast(), |d| {
                                        d.child(badge_pill("Tiny · Fast", ca(0x38bdf822), cyan))
                                    })
                                    .when(model == RewriteModel::Qwen3_4b, |d| {
                                        d.child(badge_pill("Balanced", ca(0x6366f128), indigo))
                                    })
                                    .when(
                                        model.is_indic_focused()
                                            && model != RewriteModel::SarvamTranslate4b,
                                        |d| {
                                            d.child(badge_pill(
                                                "India / Indic",
                                                ca(0xa78bfa24),
                                                violet,
                                            ))
                                        },
                                    ),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .text_size(px(11.5))
                                    .text_color(text_mid)
                                    .truncate()
                                    .child(model.subtitle()),
                            ),
                    )
                    .when_some(status_chip, |d, chip| d.child(chip))
                    .child(badge_pill(model.size_label(), ca(0xffffff0c), text_mid))
                    .when(show_install, |d| {
                        d.child(action_primary(
                            cx,
                            "install-selected-llm",
                            "Download",
                            |s, _, cx| s.download(cx),
                        ))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _, cx| {
                            cx.stop_propagation();
                            view.select_model(model, cx);
                        }),
                    ),
            );
        }

        body.child(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_dim)
                                .child("LOCAL GGUF MODELS · BUILT-IN LLAMA.CPP (378 MB TO 13.7 GB)"),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(text_dim)
                                .child("Zero Ollama · Vulkan / Metal / CPU"),
                        ),
                )
                .child(model_cards),
        )
    }

    fn paste_page(&self, cx: &mut Context<Self>) -> Div {
        let raw = self.paste.read(cx).value();
        let count_pill = match count_paste_entries(&raw) {
            Ok(0) => badge_pill("0 transcripts", ca(0xffffff10), c(0x9ca3af)),
            Ok(n) => badge_pill(
                format!(
                    "{n} distinct transcript{} ready",
                    if n == 1 { "" } else { "s" }
                ),
                ca(0x4ade8022),
                c(0x4ade80),
            ),
            Err(_) => badge_pill("Check size / limit", ca(0xfbbf2422), c(0xfbbf24)),
        };

        col()
            .gap(px(12.))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(13.))
                            .line_height(px(19.))
                            .text_color(c(0x9ca3af))
                            .child("Separate each transcript with a blank line. Keep hesitations, Hinglish, and self-corrections intact."),
                    )
                    .child(count_pill),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .p(px(2.))
                    .rounded(px(12.))
                    .bg(c(0x14141f))
                    .border_1()
                    .border_color(ca(0xffffff18))
                    .child(
                        Textarea::new(&self.paste)
                            .aria_label("Original transcripts")
                            .h(px(280.)),
                    ),
            )
    }

    fn sources_page(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.busy.is_some();
        let selected_count = self.sources.iter().filter(|(_, on)| *on).count();
        let total_sources = self.sources.len();

        let indigo = c(0x818cf8);
        let emerald = c(0x4ade80);
        let text_hi = c(0xf4f4f6);
        let text_mid = c(0xa1a1aa);

        // Overflow-safe green privacy shield banner (.w_full().min_w_0() on both row & text!)
        let mut body = col().gap(px(12.)).child(
            div()
                .w_full()
                .min_w_0()
                .px(px(14.))
                .py(px(10.))
                .rounded(px(11.))
                .bg(ca(0x4ade800e))
                .border_1()
                .border_color(ca(0x4ade8030))
                .flex()
                .flex_row()
                .items_start()
                .gap(px(10.))
                .child(div().pt(px(2.)).flex_none().child(icon("shield-check", 16., emerald)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.5))
                        .line_height(px(18.5))
                        .text_color(c(0xd1d5db))
                        .child("Only sources you check are opened. Assistant replies, tool outputs, and subagents are skipped, and only approved examples are saved on activation."),
                ),
        );

        if !self.sources.is_empty() {
            let all = self.sources.iter().all(|(_, checked)| *checked);
            body = body.child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .id("select-all-row")
                            .px(px(10.))
                            .py(px(6.))
                            .rounded(px(8.))
                            .bg(ca(0xffffff0a))
                            .border_1()
                            .border_color(ca(0xffffff16))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.))
                            .cursor_pointer()
                            .hover(|s| s.bg(ca(0xffffff14)))
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(16.))
                                    .rounded(px(4.))
                                    .bg(if all { c(0x6366f1) } else { ca(0xffffff10) })
                                    .border_1()
                                    .border_color(if all { indigo } else { ca(0xffffff2a) })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(all, |d| d.child(icon("check", 11., c(0xffffff)))),
                            )
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(text_hi)
                                    .child(if all { "Deselect all" } else { "Select all" }),
                            )
                            .when(!busy, |d| {
                                d.on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |view, _, _, cx| {
                                        cx.stop_propagation();
                                        let target = !all;
                                        for (_, selected) in &mut view.sources {
                                            *selected = target;
                                        }
                                        view.summary.clear();
                                        cx.notify();
                                    }),
                                )
                            }),
                    )
                    .child(badge_pill(
                        format!("{selected_count} of {total_sources} selected"),
                        if selected_count > 0 {
                            ca(0x6366f128)
                        } else {
                            ca(0xffffff0e)
                        },
                        if selected_count > 0 { indigo } else { text_mid },
                    )),
            );

            // Clean 2-column grid so 13+ sources fit cleanly with zero cramped scrolling
            let render_source_card = |index: usize, source: &Source, checked: bool| {
                let files = source.file_count();
                let is_voice = source.is_voice();
                div()
                    .id(("history-source-row", index))
                    .flex_1()
                    .min_w_0()
                    .px(px(11.))
                    .py(px(9.))
                    .rounded(px(10.))
                    .bg(if checked {
                        ca(0x6366f11c)
                    } else {
                        c(0x14141f)
                    })
                    .border_1()
                    .border_color(if checked {
                        ca(0x818cf860)
                    } else {
                        ca(0xffffff12)
                    })
                    .cursor_pointer()
                    .hover(|s| s.bg(ca(0xffffff10)))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_none()
                            .size(px(16.))
                            .rounded(px(4.))
                            .bg(if checked { c(0x6366f1) } else { ca(0xffffff10) })
                            .border_1()
                            .border_color(if checked { indigo } else { ca(0xffffff28) })
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(checked, |d| d.child(icon("check", 10.5, c(0xffffff)))),
                    )
                    .child(icon(
                        if is_voice { "mic" } else { "terminal" },
                        13.5,
                        if checked { indigo } else { text_mid },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(text_hi)
                            .truncate()
                            .child(source.label),
                    )
                    .child(badge_pill(
                        if is_voice { "Voice" } else { "Chat" },
                        if is_voice {
                            ca(0xa78bfa24)
                        } else {
                            ca(0xffffff0c)
                        },
                        if is_voice { c(0xc4b5fd) } else { text_mid },
                    ))
                    .child(badge_pill(
                        format!("{files}"),
                        ca(0xffffff0a),
                        text_mid,
                    ))
                    .when(!busy, |d| {
                        d.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, _, _, cx| {
                                cx.stop_propagation();
                                if let Some(entry) = view.sources.get_mut(index) {
                                    entry.1 = !entry.1;
                                }
                                view.summary.clear();
                                cx.notify();
                            }),
                        )
                    })
            };

            let mut grid = div().w_full().min_w_0().flex().flex_col().gap(px(8.));
            let mut idx = 0;
            while idx < self.sources.len() {
                let (s_left, c_left) = &self.sources[idx];
                let mut pair_row = div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .gap(px(8.))
                    .child(render_source_card(idx, s_left, *c_left));
                if idx + 1 < self.sources.len() {
                    let (s_right, c_right) = &self.sources[idx + 1];
                    pair_row = pair_row.child(render_source_card(idx + 1, s_right, *c_right));
                } else {
                    pair_row = pair_row.child(div().flex_1().min_w_0());
                }
                grid = grid.child(pair_row);
                idx += 2;
            }
            body = body.child(grid);
        } else if !busy {
            body = body.child(
                div()
                    .w_full()
                    .min_w_0()
                    .p(px(18.))
                    .rounded(px(13.))
                    .bg(c(0x14141f))
                    .border_1()
                    .border_color(ca(0xffffff14))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(subtle(
                        "No supported local assistant histories were found on this machine. You can paste your own transcripts instead.",
                    ))
                    .child(action_primary(
                        cx,
                        "history-paste",
                        "Paste samples",
                        |s, _, cx| s.show(Page::Paste, cx),
                    )),
            );
        }

        if !self.summary.is_empty() {
            body = body.child(coverage_box(&self.summary));
        }

        body
    }

    fn summary_page(&self, _cx: &mut Context<Self>) -> Div {
        let Some(draft) = self.draft.as_ref() else {
            return col().child(subtle("No samples loaded yet."));
        };
        let total = draft.sample_count();
        let (_, review_count) = draft.review_position();
        let mut body = col().gap(px(12.)).child(
            row()
                .child(badge_pill(
                    format!(
                        "{total} distinct transcript{}",
                        if total == 1 { "" } else { "s" }
                    ),
                    ca(0x4ade8022),
                    c(0x4ade80),
                ))
                .child(badge_pill(
                    format!(
                        "Reviewing {review_count} sample{}",
                        if review_count == 1 { "" } else { "s" }
                    ),
                    ca(0x6366f128),
                    c(0x818cf8),
                )),
        );
        if !self.summary.is_empty() {
            body = body.child(coverage_box(&self.summary));
        }
        if let Some(original) = draft.current() {
            body = body.child(glass_card("FIRST SAMPLE PREVIEW", original, false, Some("mic")));
        }
        body
    }

    fn review_page(&self, cx: &mut Context<Self>) -> Div {
        let Some(draft) = self.draft.as_ref() else {
            return col().child(subtle("No active draft."));
        };
        let original = draft.current().unwrap_or_default();
        let approved = draft.approved_count();
        let used = draft.used_bytes();
        let max_bytes = SetupDraft::max_profile_bytes();
        let model = self.active_model(cx);

        let mut body = col()
            .gap(px(12.))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        row()
                            .child(badge_pill(
                                format!("{approved} approved so far"),
                                ca(0x4ade8020),
                                c(0x4ade80),
                            ))
                            .child(badge_pill(
                                format!("Engine: {}", model.short_label()),
                                ca(0xffffff0c),
                                c(0xa1a1aa),
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(c(0x71717a))
                            .child(format!("Profile budget: {used} / {max_bytes} B")),
                    ),
            )
            .child(glass_card("SPOKEN ORIGINAL", original, false, Some("mic")));

        if let Some((light, natural)) = &self.suggestions {
            body = body.child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .gap(px(12.))
                    .child(
                        glass_card("LIGHT CLEANUP · YOUR WORDS", light, false, Some("wand"))
                            .justify_between()
                            .child(action_secondary_full(
                                cx,
                                "use-light",
                                "Choose light cleanup",
                                |s, _, cx| {
                                    if let Some((text, _)) = &s.suggestions {
                                        s.choose(Some(text.clone()), cx);
                                    }
                                },
                            )),
                    )
                    .child(
                        glass_card(
                            "NATURAL REPHRASE · SMOOTHED",
                            natural,
                            true,
                            Some("sparkles"),
                        )
                        .justify_between()
                        .child(action_primary_full(
                            cx,
                            "use-natural",
                            "Choose natural wording",
                            |s, _, cx| {
                                if let Some((_, text)) = &s.suggestions {
                                    s.choose(Some(text.clone()), cx);
                                }
                            },
                        )),
                    ),
            );
        } else if self.busy.is_some() {
            body = body.child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .gap(px(12.))
                    .child(loading_skeleton(
                        "LIGHT CLEANUP",
                        "Generating locally via llama.cpp…",
                    ))
                    .child(loading_skeleton(
                        "NATURAL REPHRASE",
                        "Preserving certainty & tone…",
                    )),
            );
        } else {
            body = body.child(
                actions_bar().child(action(
                    cx,
                    "retry-suggestions",
                    "Retry suggestions",
                    |s, _, cx| s.generate_suggestions(cx),
                )),
            );
        }

        if self.editing {
            body = body.child(
                div()
                    .w_full()
                    .min_w_0()
                    .p(px(14.))
                    .rounded(px(13.))
                    .bg(c(0x14141f))
                    .border_1()
                    .border_color(ca(0x818cf855))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        row()
                            .child(icon("pencil", 13., c(0x818cf8)))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(c(0xf4f4f6))
                                    .child("Write the exact message you would send"),
                            ),
                    )
                    .child(
                        Textarea::new(&self.own)
                            .aria_label("Your preferred wording")
                            .h(px(110.)),
                    )
                    .child(
                        actions_bar()
                            .child(action_primary(
                                cx,
                                "accept-own",
                                "Use my wording",
                                |s, _, cx| {
                                    let value = s.own.read(cx).value().to_string();
                                    s.choose(Some(value), cx);
                                },
                            ))
                            .child(action_ghost(cx, "cancel-own", "Cancel", |s, _, cx| {
                                s.editing = false;
                                cx.notify();
                            })),
                    ),
            );
        }

        body
    }

    fn fresh_page(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.busy.is_some();
        let raw = self.fresh.read(cx).value();
        let counter = match count_paste_entries(&raw) {
            Ok(n) if n == PREVIEW_COUNT => badge_pill(
                format!("{n} / {PREVIEW_COUNT} ready"),
                ca(0x4ade8022),
                c(0x4ade80),
            ),
            Ok(n) => badge_pill(
                format!("{n} / {PREVIEW_COUNT} dictations"),
                ca(0x6366f124),
                c(0x818cf8),
            ),
            Err(_) => badge_pill("Check format", ca(0xfbbf2422), c(0xfbbf24)),
        };

        col()
            .gap(px(12.))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(13.))
                            .line_height(px(19.))
                            .text_color(c(0x9ca3af))
                            .child("Paste three brand-new dictations separated by a blank line to compare Standard vs. Your Personal Profile side-by-side."),
                    )
                    .child(counter),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .px(px(12.))
                    .py(px(9.))
                    .rounded(px(10.))
                    .bg(ca(0x6366f110))
                    .border_1()
                    .border_color(ca(0x818cf830))
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(8.))
                    .child(div().pt(px(2.)).flex_none().child(icon("mic", 14., c(0x818cf8))))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .line_height(px(17.5))
                            .text_color(c(0xc7d2fe))
                            .child("Tip: Speak with F1, then use Copy Last Transcript in Voicebar's tray menu to paste the raw speech recognition."),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .p(px(2.))
                    .rounded(px(12.))
                    .bg(c(0x14141f))
                    .border_1()
                    .border_color(ca(0xffffff18))
                    .child(
                        Textarea::new(&self.fresh)
                            .aria_label("Three fresh dictations")
                            .h(px(220.))
                            .readonly(busy),
                    ),
            )
    }

    fn compare_page(&self, cx: &mut Context<Self>) -> Div {
        let Some(preview) = self.preview.as_ref() else {
            return col().child(subtle("No comparison generated yet."));
        };
        let index = self
            .comparison_index
            .min(preview.rewrites.len().saturating_sub(1));
        let pair = &preview.rewrites[index];
        let model_name = self.active_model(cx).short_label();

        let tabs = row()
            .gap(px(6.))
            .children((0..preview.rewrites.len()).map(|i| {
                let active = i == index;
                div()
                    .id(("cmp-tab", i))
                    .px(px(11.))
                    .py(px(5.))
                    .rounded(px(8.))
                    .bg(if active { ca(0x6366f12e) } else { c(0x14141f) })
                    .border_1()
                    .border_color(if active {
                        ca(0x818cf870)
                    } else {
                        ca(0xffffff14)
                    })
                    .text_size(px(12.))
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(if active { c(0xf4f4f6) } else { c(0x9ca3af) })
                    .cursor_pointer()
                    .hover(|s| s.bg(ca(0xffffff14)))
                    .child(format!("Test {}", i + 1))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _, cx| {
                            cx.stop_propagation();
                            view.comparison_index = i;
                            cx.notify();
                        }),
                    )
            }));

        col()
            .gap(px(12.))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(tabs)
                    .child(badge_pill(
                        format!("Model: {model_name}"),
                        ca(0xffffff0c),
                        c(0xa1a1aa),
                    )),
            )
            .child(glass_card(
                "YOUR DICTATION",
                &preview.inputs.originals[index],
                false,
                Some("mic"),
            ))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .gap(px(12.))
                    .child(glass_card(
                        "STANDARD REWRITE",
                        &pair.standard,
                        false,
                        Some("wand"),
                    ))
                    .child(glass_card(
                        "YOUR PERSONAL PROFILE",
                        &pair.personalized,
                        true,
                        Some("sparkles"),
                    )),
            )
    }

    fn done_page(&self, cx: &mut Context<Self>) -> Div {
        let model = self.active_model(cx);
        let count = self.saved_example_count.max(
            cx.global::<VoiceState>()
                .config
                .rewrite_profile
                .as_ref()
                .map(|p| p.examples().len())
                .unwrap_or(0),
        );
        div()
            .w_full()
            .min_w_0()
            .p(px(26.))
            .rounded(px(15.))
            .bg(c(0x14141f))
            .border_1()
            .border_color(ca(0x4ade8044))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(14.))
            .child(
                div()
                    .size(px(50.))
                    .rounded_full()
                    .bg(ca(0x4ade8022))
                    .border_1()
                    .border_color(ca(0x4ade8066))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("check", 24., c(0x4ade80))),
            )
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(0xf4f4f6))
                    .child("Your Personal Rewrite Profile is Live"),
            )
            .child(
                row()
                    .child(badge_pill(
                        format!("{count} approved example{}", if count == 1 { "" } else { "s" }),
                        ca(0x4ade8022),
                        c(0x4ade80),
                    ))
                    .child(badge_pill(
                        model.display_name(),
                        ca(0x6366f128),
                        c(0x818cf8),
                    ))
                    .child(badge_pill("100% Local", ca(0xffffff10), c(0xa1a1aa))),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(560.))
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .text_color(c(0x9ca3af))
                    .child("Press F1 anywhere to dictate. Voicebar applies your style locally and copies the result to your clipboard. Switch Optional Rephrasing OFF anytime from Settings or the tray."),
            )
    }

    /// Sticky bottom action footer — ALWAYS pinned and visible on every multi-step page!
    fn footer_bar(&self, cx: &mut Context<Self>) -> Option<Div> {
        let busy = self.busy.is_some();
        let base = || {
            div()
                .flex_none()
                .w_full()
                .px(px(24.))
                .py(px(12.))
                .bg(c(0x101018))
                .border_t_1()
                .border_color(ca(0xffffff16))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(12.))
        };

        match self.page {
            Page::Start => None,
            Page::Paste => Some(
                base()
                    .child(action(cx, "paste-back", "Back", |s, _, cx| {
                        s.show(Page::Start, cx)
                    }))
                    .child(action_primary(
                        cx,
                        "use-paste",
                        "Continue to review",
                        |s, _, cx| s.accept_paste(cx),
                    )),
            ),
            Page::Sources => {
                let selected_count = self.sources.iter().filter(|(_, on)| *on).count();
                let import_label = if selected_count > 0 {
                    format!("Import selected ({selected_count})")
                } else {
                    "Import selected".to_string()
                };
                Some(
                    base()
                        .child(action(cx, "history-back", "Back", |s, _, cx| {
                            s.show(Page::Start, cx)
                        }))
                        .child(btn(
                            cx,
                            "import-selected",
                            import_label,
                            BtnTone::Primary,
                            busy || selected_count == 0,
                            false,
                            |s, _, cx| s.import(cx),
                        )),
                )
            }
            Page::Summary => Some(
                base()
                    .child(action(cx, "summary-back", "Back", |s, _, cx| {
                        s.show(s.source_page, cx)
                    }))
                    .child(action_primary(
                        cx,
                        "begin-review",
                        "Start reviewing",
                        |s, _, cx| s.begin_review(cx),
                    )),
            ),
            Page::Review => {
                let mut left = row()
                    .child(action(cx, "keep-original", "Keep original", |s, _, cx| {
                        let text = s
                            .draft
                            .as_ref()
                            .and_then(SetupDraft::current)
                            .map(str::to_owned);
                        s.choose(text, cx);
                    }));
                if !self.editing {
                    left = left.child(action(cx, "edit-own", "Write my own", |s, window, cx| {
                        s.edit_own(window, cx)
                    }));
                }
                let right = row()
                    .child(action_ghost(cx, "skip-sample", "Skip", |s, _, cx| {
                        s.choose(None, cx)
                    }))
                    .child(action_ghost(
                        cx,
                        "change-input",
                        "Change samples",
                        |s, _, cx| s.show(s.source_page, cx),
                    ));
                Some(base().child(left).child(right))
            }
            Page::Fresh => Some(
                base()
                    .child(action(
                        cx,
                        "redo-examples",
                        "Edit sample choices",
                        |s, _, cx| s.restart_review(cx),
                    ))
                    .child(btn(
                        cx,
                        "generate-preview",
                        "Compare rewrites",
                        BtnTone::Primary,
                        busy,
                        false,
                        |s, _, cx| s.compare(cx),
                    )),
            ),
            Page::Compare => {
                let total = self
                    .preview
                    .as_ref()
                    .map(|p| p.rewrites.len())
                    .unwrap_or(PREVIEW_COUNT);
                let index = self.comparison_index.min(total.saturating_sub(1));
                let mut left = row()
                    .child(action_ghost(
                        cx,
                        "edit-choices",
                        "Edit sample choices",
                        |s, _, cx| s.restart_review(cx),
                    ))
                    .child(action_ghost(
                        cx,
                        "edit-fresh",
                        "Change test dictations",
                        |s, _, cx| s.show(Page::Fresh, cx),
                    ));
                if index > 0 {
                    left = left.child(action(cx, "previous-pair", "Previous", |s, _, cx| {
                        s.comparison_index = s.comparison_index.saturating_sub(1);
                        cx.notify();
                    }));
                }
                let primary_cta = if index + 1 >= total {
                    action_primary(
                        cx,
                        "activate-profile",
                        "Activate personal profile",
                        |s, window, cx| s.activate(window, cx),
                    )
                } else {
                    action_primary(cx, "next-pair", "Next comparison", |s, _, cx| {
                        s.comparison_index = (s.comparison_index + 1).min(PREVIEW_COUNT - 1);
                        cx.notify();
                    })
                };
                Some(base().child(left).child(primary_cta))
            }
            Page::Done => Some(
                base().justify_end().child(action_primary(
                    cx,
                    "close-complete",
                    "Done",
                    |_, window, cx| {
                        cx.global_mut::<SetupWindow>().0 = None;
                        window.remove_window();
                    },
                )),
            ),
        }
    }
}

fn coverage_box(summary: &str) -> Div {
    let mut box_el = div()
        .w_full()
        .min_w_0()
        .p(px(12.))
        .rounded(px(11.))
        .bg(c(0x14141f))
        .border_1()
        .border_color(ca(0xffffff14))
        .flex()
        .flex_col()
        .gap(px(5.))
        .child(
            div()
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c(0x71717a))
                .child("IMPORT COVERAGE"),
        );
    for line in summary.lines().filter(|l| !l.trim().is_empty()) {
        box_el = box_el.child(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(7.))
                .child(div().mt(px(6.)).flex_none().size(px(5.)).rounded_full().bg(c(0x818cf8)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .line_height(px(17.))
                        .text_color(c(0xd1d5db))
                        .child(line.to_owned()),
                ),
        );
    }
    box_el
}

fn loading_skeleton(title: &'static str, hint: &'static str) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .p(px(16.))
        .rounded(px(13.))
        .bg(c(0x14141f))
        .border_1()
        .border_color(ca(0x818cf830))
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c(0x818cf8))
                .child(title),
        )
        .child(div().h(px(12.)).w_full().rounded(px(6.)).bg(ca(0xffffff10)))
        .child(div().h(px(12.)).w(px(180.)).rounded(px(6.)).bg(ca(0xffffff0a)))
        .child(div().text_size(px(11.5)).text_color(c(0x71717a)).child(hint))
}

impl Render for Setup {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rewrite_on = cx.global::<VoiceState>().config.personal_rewrite;
        let model = self.active_model(cx);

        let page = match self.page {
            Page::Start => self.start_page(cx),
            Page::Paste => self.paste_page(cx),
            Page::Sources => self.sources_page(cx),
            Page::Summary => self.summary_page(cx),
            Page::Review => self.review_page(cx),
            Page::Fresh => self.fresh_page(cx),
            Page::Compare => self.compare_page(cx),
            Page::Done => self.done_page(cx),
        };

        let mut contents = div()
            .w_full()
            .min_w_0()
            .px(px(24.))
            .py(px(18.))
            .flex()
            .flex_col()
            .gap(px(14.));

        if let Some(step) = self.page.step_index() {
            contents = contents.child(stepper(step));
        }

        contents = contents.child(
            div()
                .w_full()
                .min_w_0()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(20.))
                .text_color(c(0xf4f4f6))
                .child(self.title()),
        );

        // Actionable one-click setup banner whenever built-in llama.cpp runtime or GGUF weights are missing
        if let Some(engine_banner) = self.engine_action_banner(cx) {
            contents = contents.child(engine_banner);
        }

        // Live busy banner (with streaming download progress bar + Cancel)
        if let Some(status) = &self.busy {
            let pull = self.pull_progress.lock().ok().and_then(|g| g.clone());
            let mut banner = div()
                .w_full()
                .min_w_0()
                .p(px(12.))
                .rounded(px(12.))
                .bg(ca(0x6366f116))
                .border_1()
                .border_color(ca(0x818cf848))
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap(px(10.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(8.))
                                .child(icon("loader-circle", 15., c(0x818cf8)))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(c(0xe0e7ff))
                                        .child(status.clone()),
                                ),
                        )
                        .child(action_ghost(cx, "cancel-busy-job", "Cancel", |s, _, cx| {
                            s.stop_job();
                            cx.notify();
                        })),
                );
            if let Some(p) = pull {
                let pct = (p.fraction() * 100.0).round() as u32;
                let detail = if p.total > 0 {
                    format!(
                        "{} · {:.0} / {:.0} MB ({pct}%)",
                        p.status,
                        p.completed as f64 / 1_048_576.0,
                        p.total as f64 / 1_048_576.0
                    )
                } else {
                    p.status.clone()
                };
                banner = banner
                    .child(
                        div()
                            .h(px(5.))
                            .w_full()
                            .rounded(px(999.))
                            .bg(ca(0xffffff14))
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(p.fraction().clamp(0.04, 1.0)))
                                    .rounded(px(999.))
                                    .bg(c(0x818cf8)),
                            ),
                    )
                    .child(div().text_size(px(11.5)).text_color(c(0xa5b4fc)).child(detail));
            }
            contents = contents.child(banner);
        }

        // Crisp dark error / missing-model notice (strictly overflow-safe!)
        if let Some(problem) = &self.problem {
            let (accent, bg, border, message, cta): (Hsla, Hsla, Hsla, String, Option<Stateful<Div>>) =
                match problem {
                    Failure::MissingModel => (
                        c(0xfbbf24),
                        ca(0xfbbf2414),
                        ca(0xfbbf2448),
                        format!(
                            "{} needs Voicebar's built-in llama.cpp runtime and GGUF weights. Download them locally to generate suggestions and comparisons.",
                            model.display_name()
                        ),
                        Some(action_primary(cx, "get-model", "Download now", |s, _, cx| {
                            s.download(cx)
                        })),
                    ),
                    Failure::Message(error) => (
                        c(0xf87171),
                        ca(0xf8717114),
                        ca(0xf8717148),
                        (*error).to_string(),
                        None,
                    ),
                };

            let notice = div()
                .w_full()
                .min_w_0()
                .p(px(12.))
                .rounded(px(12.))
                .bg(bg)
                .border_1()
                .border_color(border)
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_row()
                        .items_start()
                        .gap(px(8.))
                        .child(div().pt(px(2.)).flex_none().child(icon("triangle-alert", 15., accent)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(12.5))
                                .line_height(px(18.))
                                .text_color(c(0xf4f4f6))
                                .child(message),
                        ),
                )
                .when_some(cta, |d, btn_el| d.child(btn_el));
            contents = contents.child(notice);
        }

        // Studio top header bar
        let (dot_color, status_text) = match self.model_status {
            ModelStatus::Ready => {
                if llama_engine::is_server_running_for(model) {
                    (c(0x4ade80), format!("{} · Running", model.short_label()))
                } else {
                    (c(0x4ade80), format!("{} · Ready", model.short_label()))
                }
            }
            ModelStatus::ModelMissing => (
                c(0xfbbf24),
                format!("{} · Not downloaded", model.short_label()),
            ),
            ModelStatus::EngineMissing => (
                c(0x818cf8),
                "Built-in llama.cpp · Download ready".into(),
            ),
        };

        let mut header = div()
            .flex_none()
            .w_full()
            .px(px(20.))
            .py(px(11.))
            .bg(c(0x101018))
            .border_b_1()
            .border_color(ca(0xffffff14))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .child(
                row()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_none()
                            .size(px(26.))
                            .rounded(px(8.))
                            .bg(ca(0x6366f128))
                            .border_1()
                            .border_color(ca(0x818cf850))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon("sparkles", 14., c(0x818cf8))),
                    )
                    .child(
                        div()
                            .text_size(px(13.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(0xf4f4f6))
                            .child("Voicebar · Personal Rewrite"),
                    )
                    .child(
                        div()
                            .id("header-engine-status")
                            .px(px(9.))
                            .py(px(3.5))
                            .rounded(px(999.))
                            .bg(ca(0xffffff0a))
                            .border_1()
                            .border_color(ca(0xffffff16))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .cursor_pointer()
                            .hover(|s| s.bg(ca(0xffffff16)))
                            .child(div().size(px(6.5)).rounded_full().bg(dot_color))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(c(0xd1d5db))
                                    .child(status_text),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|view, _, _, cx| {
                                    cx.stop_propagation();
                                    view.probe_active_model(cx);
                                }),
                            ),
                    ),
            );

        let mut right_bar = row().flex_none().gap(px(8.)).child(
            div()
                .id("studio-optional-toggle")
                .px(px(10.))
                .py(px(5.))
                .rounded(px(999.))
                .bg(if rewrite_on {
                    ca(0x6366f12c)
                } else {
                    ca(0xffffff0c)
                })
                .border_1()
                .border_color(if rewrite_on {
                    ca(0x818cf866)
                } else {
                    ca(0xffffff18)
                })
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(ca(0xffffff1a)))
                .child(
                    div()
                        .size(px(7.))
                        .rounded_full()
                        .bg(if rewrite_on { c(0x4ade80) } else { c(0x71717a) }),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(c(0xf4f4f6))
                        .child(if rewrite_on {
                            "Rephrasing: ON"
                        } else {
                            "Rephrasing: OFF"
                        }),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                    crate::toggle_personal_rewrite_from_ui(cx);
                }),
        );

        if self.page != Page::Done {
            right_bar = right_bar
                .child(action(
                    cx,
                    "continue-later",
                    "Continue later",
                    |_, window, _| {
                        window.minimize_window();
                    },
                ))
                .child(action_ghost(cx, "discard-setup", "Close", |s, window, cx| {
                    s.stop_job();
                    cx.global_mut::<SetupWindow>().0 = None;
                    window.remove_window();
                }));
        }

        header = header.child(right_bar);

        let page_id = (self.page as usize) * 10
            + if self.page == Page::Review {
                self.draft
                    .as_ref()
                    .map(|d| d.review_position().0)
                    .unwrap_or(0)
            } else {
                self.comparison_index
            };

        let footer = self.footer_bar(cx);

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c(0x0d0d14))
            .text_color(c(0xf4f4f6))
            .font_family(crate::overlay::ui_font())
            .child(header)
            .child(
                div()
                    .id(("profile-scroll", page_id))
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .child(contents.child(page)),
            )
            .when_some(footer, |d, f| d.child(f))
    }
}
