//! Global F1 hotkey listener on Windows using a dedicated Win32 message thread.
//! Running RegisterHotKey on its own thread keeps it completely decoupled from GPUI's event loop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;

pub static TOGGLE_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn take_toggle_request() -> bool {
    TOGGLE_REQUESTED.swap(false, Ordering::SeqCst)
}

pub fn trigger_toggle() {
    TOGGLE_REQUESTED.store(true, Ordering::SeqCst);
}

#[cfg(windows)]
pub fn start_global_f1_listener() -> Result<(), String> {
    use std::ptr::null_mut;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey, VK_F1,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

    const HOTKEY_ID: i32 = 0x5642; // "VB"
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();

    thread::Builder::new()
        .name("voicebar-hotkey".into())
        .spawn(move || unsafe {
            let ok = RegisterHotKey(null_mut(), HOTKEY_ID, MOD_NOREPEAT, VK_F1 as u32);
            if ok == 0 {
                let err = std::io::Error::last_os_error();
                let _ = ready_tx.send(Err(format!(
                    "Could not register global F1 hotkey ({err}). Another app may own F1."
                )));
                return;
            }
            let _ = ready_tx.send(Ok(()));

            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                if msg.message == WM_HOTKEY && msg.wParam as i32 == HOTKEY_ID {
                    TOGGLE_REQUESTED.store(true, Ordering::SeqCst);
                }
            }
            UnregisterHotKey(null_mut(), HOTKEY_ID);
        })
        .map_err(|e| format!("Failed to spawn hotkey thread: {e}"))?;

    ready_rx
        .recv()
        .unwrap_or_else(|_| Err("Hotkey thread exited unexpectedly".into()))
}

#[cfg(not(windows))]
pub fn start_global_f1_listener() -> Result<(), String> {
    use global_hotkey::hotkey::{Code, HotKey};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

    let manager = GlobalHotKeyManager::new()
        .map_err(|e| format!("Could not initialize global hotkey manager ({e})"))?;
    let f1 = HotKey::new(None, Code::F1);
    let f1_id = f1.id();
    manager
        .register(f1)
        .map_err(|e| format!("Could not register global F1 hotkey ({e})"))?;

    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.id == f1_id && event.state == HotKeyState::Pressed {
            TOGGLE_REQUESTED.store(true, Ordering::SeqCst);
        }
    }));

    // Keep the manager alive for the lifetime of the application.
    Box::leak(Box::new(manager));
    Ok(())
}

/// Capture the currently focused external window HWND (0 if none or if it belongs to Voicebar).
#[cfg(windows)]
pub fn capture_foreground_hwnd() -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId,
    };
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return 0;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == std::process::id() {
            0
        } else {
            hwnd as isize
        }
    }
}

#[cfg(not(windows))]
pub fn capture_foreground_hwnd() -> isize {
    0
}

/// Synthesize Ctrl+V into the active input field after a brief settle delay.
/// If Voicebar itself currently owns foreground focus (e.g. the user clicked the Done button),
/// restores focus to `fallback_hwnd` first.
#[cfg(windows)]
pub fn paste_ctrl_v_async(fallback_hwnd: isize) {
    use std::time::Duration;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_CONTROL, VK_V,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
    };

    thread::spawn(move || unsafe {
        let fg = GetForegroundWindow();
        let mut pid: u32 = 0;
        if !fg.is_null() {
            GetWindowThreadProcessId(fg, &mut pid);
        }
        if (fg.is_null() || pid == std::process::id()) && fallback_hwnd != 0 {
            SetForegroundWindow(fallback_hwnd as HWND);
        }

        // Brief pause so clipboard contents and target focus settle after F1 release.
        thread::sleep(Duration::from_millis(75));

        let key_event = |vk: u16, flags: u32| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };

        let inputs = [
            key_event(VK_CONTROL, 0),
            key_event(VK_V, 0),
            key_event(VK_V, KEYEVENTF_KEYUP),
            key_event(VK_CONTROL, KEYEVENTF_KEYUP),
        ];

        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    });
}

#[cfg(target_os = "macos")]
pub fn paste_ctrl_v_async(_fallback_hwnd: isize) {
    use std::time::Duration;
    thread::spawn(|| {
        thread::sleep(Duration::from_millis(80));
        let _ = std::process::Command::new("osascript")
            .args([
                "-e",
                r#"tell application "System Events" to keystroke "v" using command down"#,
            ])
            .status();
    });
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn paste_ctrl_v_async(_fallback_hwnd: isize) {
    use std::time::Duration;
    thread::spawn(|| {
        thread::sleep(Duration::from_millis(80));
        let wayland = std::env::var("WAYLAND_DISPLAY")
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        if wayland {
            if std::process::Command::new("wtype")
                .args(["-M", "ctrl", "-k", "v", "-m", "ctrl"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
            {
                return;
            }
        }
        let _ = std::process::Command::new("xdotool")
            .args(["key", "--clearmodifiers", "ctrl+v"])
            .status();
    });
}

/// Strip Windows 11's native DWM outer border ring and OS corner frame from our visible popup HWND.
#[cfg(windows)]
pub fn strip_dwm_outer_ring() {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GWL_STYLE, GetWindowThreadProcessId, IsWindowVisible, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowLongPtrW, SetWindowPos,
    };

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: HWND,
            dw_attribute: u32,
            pv_attribute: *const c_void,
            cb_attribute: u32,
        ) -> i32;
    }

    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWA_BORDER_COLOR: u32 = 34;
    const DWMWCP_DONOTROUND: u32 = 1;
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
    const HWND_TOPMOST: HWND = -1isize as HWND;
    const WS_POPUP_VISIBLE: isize = (0x8000_0000u32 | 0x1000_0000u32) as isize;

    unsafe extern "system" fn callback(hwnd: HWND, target_pid: LPARAM) -> i32 {
        unsafe {
            if IsWindowVisible(hwnd) == 0 {
                return 1;
            }
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == target_pid as u32 {
                // Switch from WS_OVERLAPPED (0) to pure frameless WS_POPUP | WS_VISIBLE.
                SetWindowLongPtrW(hwnd, GWL_STYLE, WS_POPUP_VISIBLE);
                // Explicitly tell Windows 11 DWM: no 1px outer border and no OS corner ring.
                let no_border = DWMWA_COLOR_NONE;
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_BORDER_COLOR,
                    &no_border as *const u32 as *const c_void,
                    std::mem::size_of::<u32>() as u32,
                );
                let no_round = DWMWCP_DONOTROUND;
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_WINDOW_CORNER_PREFERENCE,
                    &no_round as *const u32 as *const c_void,
                    std::mem::size_of::<u32>() as u32,
                );
                // Re-assert HWND_TOPMOST (since GPUI's resize calls SetWindowPos without SWP_NOZORDER).
                SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
            }
            1
        }
    }

    unsafe {
        EnumWindows(Some(callback), std::process::id() as LPARAM);
    }
}

#[cfg(not(windows))]
pub fn strip_dwm_outer_ring() {}
