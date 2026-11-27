//! System tray icon (3 visual states: warming amber, ready indigo-violet, recording crimson)
//! and full menu for Voicebar.

use crate::config::{AppConfig, ModelKind};
use tray_icon::menu::{
    CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
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
    OpenProfileSetup,
    CopyLast,
    SelectModel(ModelKind),
    SelectLanguage(String),
    TogglePersonalRewrite,
    ToggleAutoPaste,
    ToggleAutostart,
    ToggleGpu,
    OpenModelsFolder,
    Quit,
}

const TRAY_LANGUAGES: &[(&str, &str)] = &[
    ("en-US", "English (en-US)"),
    ("en-GB", "English · India / UK (en-GB)"),
    ("hi-IN", "Hindi (hi-IN)"),
    ("bn-IN", "Bengali (bn-IN)"),
    ("ta-IN", "Tamil (ta-IN)"),
    ("te-IN", "Telugu (te-IN)"),
    ("mr-IN", "Marathi (mr-IN)"),
    ("gu-IN", "Gujarati (gu-IN)"),
    ("kn-IN", "Kannada (kn-IN)"),
    ("ml-IN", "Malayalam (ml-IN)"),
    ("auto", "Auto Detect"),
];

pub struct Tray {
    icon: TrayIcon,
    last_mood: Option<TrayMood>,
    toggle: MenuId,
    open_settings: MenuId,
    open_profile_setup: MenuId,
    copy_last: MenuId,
    model_nemotron35: CheckMenuItem,
    model_nemotron_en: CheckMenuItem,
    model_unified_en: CheckMenuItem,
    model_eou: CheckMenuItem,
    lang_items: Vec<(&'static str, CheckMenuItem)>,
    personal_rewrite: CheckMenuItem,
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
        let open_profile_setup = MenuItem::new("Personal Rewrite Profile…", true, None);
        let copy_last = MenuItem::new("Copy Last Transcript", true, None);

        let model_menu = Submenu::new("Speech Model (GPU)", true);
        let model_nemotron35 = CheckMenuItem::new(
            "Nemotron 3.5 · 0.6B  (EN + Indian Languages · Punctuation)",
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

        let lang_menu = Submenu::new("Spoken Language (Nemotron 3.5)", true);
        let mut lang_items = Vec::with_capacity(TRAY_LANGUAGES.len());
        for &(code, label) in TRAY_LANGUAGES {
            let item = CheckMenuItem::new(label, true, cfg.language == code, None);
            let _ = lang_menu.append(&item);
            lang_items.push((code, item));
        }

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
        let personal_rewrite = CheckMenuItem::new(
            "Optional Rephrasing (Built-in llama.cpp)",
            true,
            cfg.personal_rewrite,
            None,
        );
        let auto_paste = CheckMenuItem::new(paste_label, true, cfg.auto_paste, None);
        let autostart =
            CheckMenuItem::new("Start at Sign-In (Preload GPU)", true, autostart_on, None);
        let use_gpu = CheckMenuItem::new(gpu_label, true, cfg.use_gpu, None);
        let open_models = MenuItem::new("Open Models Folder", true, None);
        let quit = MenuItem::new("Quit Voicebar", true, None);

        let _ = menu.append_items(&[
            &toggle,
            &open_settings,
            &open_profile_setup,
            &copy_last,
            &PredefinedMenuItem::separator(),
            &personal_rewrite,
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
            open_profile_setup: open_profile_setup.id().clone(),
            copy_last: copy_last.id().clone(),
            model_nemotron35,
            model_nemotron_en,
            model_unified_en,
            model_eou,
            lang_items,
            personal_rewrite,
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
        for (code, item) in &self.lang_items {
            item.set_checked(cfg.language == *code);
        }
        self.personal_rewrite.set_checked(cfg.personal_rewrite);
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
            } else if id == self.open_profile_setup {
                out.push(TrayAction::OpenProfileSetup);
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
            } else if let Some((code, _)) = self.lang_items.iter().find(|(_, item)| id == *item.id())
            {
                out.push(TrayAction::SelectLanguage((*code).to_string()));
            } else if id == *self.personal_rewrite.id() {
                out.push(TrayAction::TogglePersonalRewrite);
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
