//! System tray icon (3 visual states: warming amber, ready indigo-violet, recording crimson)
//! and full menu for Voicebar.

use crate::config::{AppConfig, ModelKind};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TrayMood {
    Warming,
    Ready,
    Recording,
}

pub enum TrayAction {
    ToggleDictation,
    OpenSettings,
    CopyLast,
    SelectModel(ModelKind),
    SelectLanguage(String),
    ToggleAutoPaste,
    ToggleAutostart,
    ToggleGpu,
    OpenModelsFolder,
    Quit,
}

pub struct Tray {
    icon: TrayIcon,
    last_mood: Option<TrayMood>,
    toggle: MenuId,
    open_settings: MenuId,
    copy_last: MenuId,
    model_nemotron35: CheckMenuItem,
    model_nemotron_en: CheckMenuItem,
    model_unified_en: CheckMenuItem,
    model_eou: CheckMenuItem,
    lang_en: CheckMenuItem,
    lang_hi: CheckMenuItem,
    lang_auto: CheckMenuItem,
    auto_paste: CheckMenuItem,
    autostart: CheckMenuItem,
    use_gpu: CheckMenuItem,
    open_models: MenuId,
    quit: MenuId,
}

/// Generate a crisp 32x32 RGBA tray icon matching assets/voicebar.ico.
fn make_icon(mood: TrayMood) -> Icon {
    const N: usize = 32;
    let mut rgba = vec![0u8; N * N * 4];

    for y in 0..N {
        for x in 0..N {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let qx = (fx - 16.0).abs() - 7.0;
            let qy = (fy - 16.0).abs() - 7.0;
            let d = (qx.max(0.0).hypot(qy.max(0.0))) + qx.max(qy).min(0.0) - 7.0;
            let alpha = (0.5 - d).clamp(0.0, 1.0);
            if alpha <= 0.0 {
                continue;
            }

            let t = (fx * 0.4 + fy * 0.6) / N as f32;
            let (bg_r, bg_g, bg_b) = match mood {
                TrayMood::Recording => (220u8, 38u8, 38u8),
                TrayMood::Warming => (217u8, 119u8, 6u8),
                TrayMood::Ready => (
                    (88.0 + (124.0 - 88.0) * t) as u8,
                    (101.0 + (58.0 - 101.0) * t) as u8,
                    (242.0 + (237.0 - 242.0) * t) as u8,
                ),
            };

            let bar = |cx: f32, half_h: f32| -> bool {
                (fx - cx).abs() <= 1.45 && (fy - 16.0).abs() <= half_h
            };
            let is_glyph = bar(8.5, 4.0)
                || bar(12.2, 7.0)
                || bar(16.0, 9.2)
                || bar(19.8, 7.5)
                || bar(23.5, 4.5);

            let idx = (y * N + x) * 4;
            if is_glyph {
                rgba[idx] = 255;
                rgba[idx + 1] = 255;
                rgba[idx + 2] = 255;
                rgba[idx + 3] = (alpha * 255.0) as u8;
            } else {
                rgba[idx] = bg_r;
                rgba[idx + 1] = bg_g;
                rgba[idx + 2] = bg_b;
                rgba[idx + 3] = (alpha * 255.0) as u8;
            }
        }
    }

    Icon::from_rgba(rgba, N as u32, N as u32).expect("tray icon rgba")
}

impl Tray {
    pub fn new(cfg: &AppConfig, autostart_on: bool) -> Option<Self> {
        let menu = Menu::new();
        let toggle = MenuItem::new("Start / Stop Dictation  (F1)", true, None);
        let open_settings = MenuItem::new("Models & Settings…", true, None);
        let copy_last = MenuItem::new("Copy Last Transcript", true, None);

        let model_menu = Submenu::new("Model", true);
        let model_nemotron35 = CheckMenuItem::new(
            "Nemotron 3.5 · 0.6B  (Punctuation · Clean Fillers · EN/40 langs)",
            true,
            cfg.model == ModelKind::Nemotron35,
            None,
        );
        let model_nemotron_en = CheckMenuItem::new(
            "Nemotron EN · 0.6B  (100% English-Only · Punctuation)",
            true,
            cfg.model == ModelKind::NemotronEn,
            None,
        );
        let model_unified_en = CheckMenuItem::new(
            "Parakeet Unified · 0.6B  (Newest English-Only · Punctuation)",
            true,
            cfg.model == ModelKind::ParakeetUnifiedEn,
            None,
        );
        let model_eou = CheckMenuItem::new(
            "Parakeet EOU · 120M  (0.8s Startup · 160ms Ultra-Fast · EN)",
            true,
            cfg.model == ModelKind::ParakeetEou,
            None,
        );
        let _ = model_menu.append_items(&[
            &model_nemotron35,
            &model_nemotron_en,
            &model_unified_en,
            &model_eou,
        ]);

        let lang_menu = Submenu::new("Language (Nemotron 3.5)", true);
        let lang_en = CheckMenuItem::new("English (en-US)", true, cfg.language == "en-US", None);
        let lang_hi = CheckMenuItem::new("Hindi (hi-IN)", true, cfg.language == "hi-IN", None);
        let lang_auto = CheckMenuItem::new("Auto Detect", true, cfg.language == "auto", None);
        let _ = lang_menu.append_items(&[&lang_en, &lang_hi, &lang_auto]);

        let paste_label = if cfg!(target_os = "macos") {
            "Auto-Paste into Input (⌘V)"
        } else {
            "Auto-Paste into Input (Ctrl+V)"
        };
        let gpu_label = if cfg!(target_os = "macos") {
            "GPU Acceleration (Metal)"
        } else {
            "GPU Acceleration (Vulkan)"
        };
        let auto_paste = CheckMenuItem::new(paste_label, true, cfg.auto_paste, None);
        let autostart = CheckMenuItem::new(
            "Start at Sign-In (Preload GPU)",
            true,
            autostart_on,
            None,
        );
        let use_gpu = CheckMenuItem::new(gpu_label, true, cfg.use_gpu, None);
        let open_models = MenuItem::new("Open Models Folder", true, None);
        let quit = MenuItem::new("Quit Voicebar", true, None);

        let _ = menu.append_items(&[
            &toggle,
            &open_settings,
            &copy_last,
            &PredefinedMenuItem::separator(),
            &auto_paste,
            &autostart,
            &PredefinedMenuItem::separator(),
            &model_menu,
            &lang_menu,
            &use_gpu,
            &PredefinedMenuItem::separator(),
            &open_models,
            &PredefinedMenuItem::separator(),
            &quit,
        ]);

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("Voicebar · Press F1 to dictate")
            .with_icon(make_icon(TrayMood::Warming))
            .build()
            .ok()?;

        Some(Self {
            icon,
            last_mood: Some(TrayMood::Warming),
            toggle: toggle.id().clone(),
            open_settings: open_settings.id().clone(),
            copy_last: copy_last.id().clone(),
            model_nemotron35,
            model_nemotron_en,
            model_unified_en,
            model_eou,
            lang_en,
            lang_hi,
            lang_auto,
            auto_paste,
            autostart,
            use_gpu,
            open_models: open_models.id().clone(),
            quit: quit.id().clone(),
        })
    }

    pub fn sync(&mut self, cfg: &AppConfig, autostart_on: bool, mood: TrayMood, tip: &str) {
        self.model_nemotron35
            .set_checked(cfg.model == ModelKind::Nemotron35);
        self.model_nemotron_en
            .set_checked(cfg.model == ModelKind::NemotronEn);
        self.model_unified_en
            .set_checked(cfg.model == ModelKind::ParakeetUnifiedEn);
        self.model_eou
            .set_checked(cfg.model == ModelKind::ParakeetEou);
        self.lang_en.set_checked(cfg.language == "en-US");
        self.lang_hi.set_checked(cfg.language == "hi-IN");
        self.lang_auto.set_checked(cfg.language == "auto");
        self.auto_paste.set_checked(cfg.auto_paste);
        self.autostart.set_checked(autostart_on);
        self.use_gpu.set_checked(cfg.use_gpu);
        if self.last_mood != Some(mood) {
            let _ = self.icon.set_icon(Some(make_icon(mood)));
            self.last_mood = Some(mood);
        }
        let _ = self.icon.set_tooltip(Some(tip));
    }

    pub fn poll(&self) -> Vec<TrayAction> {
        let mut out = Vec::new();
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                out.push(TrayAction::ToggleDictation);
            }
        }
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            let id = ev.id;
            if id == self.toggle {
                out.push(TrayAction::ToggleDictation);
            } else if id == self.open_settings {
                out.push(TrayAction::OpenSettings);
            } else if id == self.copy_last {
                out.push(TrayAction::CopyLast);
            } else if id == *self.model_nemotron35.id() {
                out.push(TrayAction::SelectModel(ModelKind::Nemotron35));
            } else if id == *self.model_nemotron_en.id() {
                out.push(TrayAction::SelectModel(ModelKind::NemotronEn));
            } else if id == *self.model_unified_en.id() {
                out.push(TrayAction::SelectModel(ModelKind::ParakeetUnifiedEn));
            } else if id == *self.model_eou.id() {
                out.push(TrayAction::SelectModel(ModelKind::ParakeetEou));
            } else if id == *self.lang_en.id() {
                out.push(TrayAction::SelectLanguage("en-US".into()));
            } else if id == *self.lang_hi.id() {
                out.push(TrayAction::SelectLanguage("hi-IN".into()));
            } else if id == *self.lang_auto.id() {
                out.push(TrayAction::SelectLanguage("auto".into()));
            } else if id == *self.auto_paste.id() {
                out.push(TrayAction::ToggleAutoPaste);
            } else if id == *self.autostart.id() {
                out.push(TrayAction::ToggleAutostart);
            } else if id == *self.use_gpu.id() {
                out.push(TrayAction::ToggleGpu);
            } else if id == self.open_models {
                out.push(TrayAction::OpenModelsFolder);
            } else if id == self.quit {
                out.push(TrayAction::Quit);
            }
        }
        out
    }
}
