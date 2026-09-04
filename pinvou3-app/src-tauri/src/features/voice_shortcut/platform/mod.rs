#[cfg(target_os = "windows")]
use super::{
    VoiceShortcutDecision, VoiceShortcutEvent, VoiceShortcutKey, VoiceShortcutState,
    emit_shortcut_event, handle_voice_shortcut_key, is_voice_shortcut_router_window,
    recording_label, resolve_trigger_target,
};
#[cfg(target_os = "windows")]
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
#[cfg(target_os = "windows")]
use std::time::Duration;
use tauri::AppHandle;
#[cfg(target_os = "windows")]
use tauri::Manager;
#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::{GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, SendInput, VIRTUAL_KEY, VK_ESCAPE, VK_LMENU,
    VK_MENU, VK_RMENU, VK_SPACE,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GA_ROOT, GA_ROOTOWNER, GetAncestor, GetForegroundWindow, GetMessageW,
    GetWindowThreadProcessId, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, SetWindowsHookExW,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

#[cfg(target_os = "windows")]
static INSTALLED: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "windows")]
static WORKERS_STARTED: OnceLock<()> = OnceLock::new();
#[cfg(target_os = "windows")]
static SHORTCUT_STATE: OnceLock<Mutex<VoiceShortcutState>> = OnceLock::new();
#[cfg(target_os = "windows")]
static EVENT_SENDER: OnceLock<
    Mutex<Option<mpsc::Sender<(VoiceShortcutEvent, String, &'static str)>>>,
> = OnceLock::new();
#[cfg(target_os = "windows")]
static SHORTCUT_ENABLED: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "windows")]
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();
/// HWND cache of the router windows (main + detached), queried by the hook
/// thread without blocking. Tauri's window getters require a main-thread
/// message round-trip and cannot be called from a low-level hook callback, so
/// a dedicated thread refreshes the cache at a low frequency while the switch
/// is on.
#[cfg(target_os = "windows")]
static ROUTER_WINDOWS: Mutex<Vec<(isize, String)>> = Mutex::new(Vec::new());

#[cfg(target_os = "windows")]
pub(super) fn install(app: AppHandle) {
    // Reinstall is allowed: an abnormal pump-thread exit (GetMessageW error /
    // WM_QUIT) unhooks and resets INSTALLED.
    if INSTALLED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let _ = APP_HANDLE.set(app.clone());

    if WORKERS_STARTED.set(()).is_ok() {
        let (tx, rx) = mpsc::channel::<(VoiceShortcutEvent, String, &'static str)>();
        let _ = EVENT_SENDER.set(Mutex::new(Some(tx)));
        let emit_app = app.clone();
        std::thread::spawn(move || {
            while let Ok((event, window_label, route)) = rx.recv() {
                emit_shortcut_event(&emit_app, event, &window_label, route);
            }
        });

        std::thread::spawn(move || {
            loop {
                if shortcut_enabled() {
                    refresh_router_window_hwnds(&app);
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
    }

    std::thread::spawn(|| unsafe {
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), 0 as HINSTANCE, 0);
        if hook.is_null() {
            log::warn!("failed to install voice shortcut keyboard hook");
            INSTALLED.store(false, Ordering::SeqCst);
            return;
        }
        log::debug!("voice shortcut keyboard hook installed");
        let mut msg = std::mem::zeroed::<MSG>();
        loop {
            let result = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
            if result > 0 {
                continue;
            }
            if result < 0 {
                log::warn!(
                    "voice shortcut hook message pump failed: error={}",
                    GetLastError()
                );
            }
            break;
        }
        UnhookWindowsHookEx(hook);
        INSTALLED.store(false, Ordering::SeqCst);
        log::warn!("voice shortcut keyboard hook pump exited; hook uninstalled");
        schedule_hook_reinstall();
    });
}

/// Bounded auto-reinstall after the pump thread dies: a GetMessageW failure/
/// exit permanently removes the low-level hook, which previously meant silent
/// breakage until restart. After a 1s delay the hook is reinstalled via
/// APP_HANDLE, at most 3 times per app lifetime; once exhausted, an error is
/// logged (the shortcut stays broken until restart, visible in the log).
#[cfg(target_os = "windows")]
static PUMP_RESTARTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(target_os = "windows")]
const PUMP_RESTART_LIMIT: usize = 3;

#[cfg(target_os = "windows")]
fn schedule_hook_reinstall() {
    let attempts = PUMP_RESTARTS.fetch_add(1, Ordering::SeqCst);
    if attempts >= PUMP_RESTART_LIMIT {
        log::error!(
            "voice shortcut keyboard hook kept dying ({} restarts); shortcut stays disabled until app restart",
            attempts + 1
        );
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let Some(app) = APP_HANDLE.get() else {
            return;
        };
        if INSTALLED.load(Ordering::SeqCst) {
            return;
        }
        log::info!(
            "voice shortcut keyboard hook reinstalling after pump death (attempt {}/{})",
            attempts + 1,
            PUMP_RESTART_LIMIT
        );
        install(app.clone());
    });
}

#[cfg(target_os = "windows")]
pub(super) fn set_enabled(enabled: bool) {
    SHORTCUT_ENABLED.store(enabled, Ordering::SeqCst);
    if !enabled {
        let mutex = SHORTCUT_STATE.get_or_init(|| Mutex::new(VoiceShortcutState::default()));
        if let Ok(mut state) = mutex.lock() {
            *state = VoiceShortcutState::default();
        }
    } else if let Some(app) = APP_HANDLE.get() {
        // Refresh the HWND cache immediately on enable so the shortcut is not
        // unresponsive during the first 200ms refresh window.
        let app = app.clone();
        std::thread::spawn(move || refresh_router_window_hwnds(&app));
    }
    log::debug!("voice shortcut enabled={}", enabled);
}

#[cfg(target_os = "windows")]
fn shortcut_enabled() -> bool {
    SHORTCUT_ENABLED.load(Ordering::SeqCst)
}

#[cfg(not(target_os = "windows"))]
pub(super) fn install(_app: AppHandle) {
    log::debug!("voice shortcut keyboard hook is unsupported on this platform");
}

#[cfg(not(target_os = "windows"))]
pub(super) fn set_enabled(enabled: bool) {
    log::debug!(
        "voice shortcut enabled={} ignored because keyboard hook is unsupported on this platform",
        enabled
    );
}

#[cfg(target_os = "windows")]
fn refresh_router_window_hwnds(app: &AppHandle) {
    let mut windows = Vec::new();
    for (label, window) in app.webview_windows() {
        if !is_voice_shortcut_router_window(&label) {
            continue;
        }
        if let Ok(hwnd) = window.hwnd() {
            windows.push((hwnd.0 as isize, label));
        }
    }
    if let Ok(mut guard) = ROUTER_WINDOWS.lock() {
        *guard = windows;
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn keyboard_hook_proc(
    code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if code < 0 {
        return call_next_hook(code, w_param, l_param);
    }

    let message = w_param as u32;
    let key_down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
    let key_up = message == WM_KEYUP || message == WM_SYSKEYUP;
    if !key_down && !key_up {
        return call_next_hook(code, w_param, l_param);
    }

    // Gate ordering: the switch takes precedence over any Win32 query; while
    // disabled, all keystrokes pass straight through.
    if !shortcut_enabled() {
        return call_next_hook(code, w_param, l_param);
    }

    let info = unsafe { &*(l_param as *const KBDLLHOOKSTRUCT) };
    // Injected keys (SendInput synthetics, including the Alt down this module
    // replays for combos) never take part in gesture detection.
    if info.flags & LLKHF_INJECTED != 0 {
        return call_next_hook(code, w_param, l_param);
    }

    let key = voice_shortcut_key(info.vkCode as VIRTUAL_KEY);
    let foreground = unsafe { GetForegroundWindow() };
    let target = hook_target_label(foreground);
    let decision = {
        let mutex = SHORTCUT_STATE.get_or_init(|| Mutex::new(VoiceShortcutState::default()));
        let mut state = match mutex.lock() {
            Ok(guard) => guard,
            Err(_) => return call_next_hook(code, w_param, l_param),
        };
        let decision = handle_voice_shortcut_key(
            &mut state,
            key,
            key_down,
            target.is_some(),
            foreground as isize,
            info.time,
        );
        if decision.inject_alt_down {
            // The combo down was swallowed: replay [Alt↓, combo↓] in order
            // with a single SendInput, so the system/WebView sees the modifier
            // order of a real press and Alt+Tab/Alt+F4 keep working. Only a
            // successful replay confirms alt_forwarded (the real Alt up is let
            // through to pair with it); on failure it stays unforwarded, the
            // combo is lost, the real Alt up is wrapped up along the
            // unforwarded path, and no state is left behind.
            if replay_combo_with_alt(info.vkCode as VIRTUAL_KEY) {
                state.alt_forwarded = true;
            }
        }
        decision
    };

    log_shortcut_decision(
        key,
        key_down,
        target.as_ref().map(|(label, _)| label.as_str()),
        decision,
    );
    if let (Some(event), Some((window_label, route))) = (decision.event, target) {
        send_event(event, window_label, route);
    }
    if decision.suppress {
        return 1;
    }
    call_next_hook(code, w_param, l_param)
}

/// Whether this keystroke is taken over by the shortcut gesture, and the
/// target window on trigger: only a foreground window of this process is taken
/// over; the recording window wins (cross-window mutual exclusion: targeted to
/// the recording window to stop it, never a second session), otherwise the
/// focused window must be in the Router whitelist (main/detached); with
/// neither, no swallowing and no emit.
#[cfg(target_os = "windows")]
fn hook_target_label(foreground: HWND) -> Option<(String, &'static str)> {
    if foreground.is_null() || !foreground_is_current_app(foreground) {
        return None;
    }
    let focused = focused_router_label(foreground);
    resolve_trigger_target(recording_label().as_deref(), focused.as_deref())
}

#[cfg(target_os = "windows")]
fn focused_router_label(foreground: HWND) -> Option<String> {
    let hwnd = foreground as isize;
    let guard = ROUTER_WINDOWS.lock().ok()?;
    guard
        .iter()
        .find(|(cached, _)| *cached == hwnd)
        .map(|(_, label)| label.clone())
}

/// tap-hold compensation: the combo down was swallowed, so inject the two-entry
/// [Alt↓, combo↓] sequence with a single SendInput, replayed in order, so the
/// system and WebView see the same modifier order as a real press (Alt first,
/// then the combo; the old implementation let the combo through first and
/// injected Alt afterwards, which broke Alt+Tab/Alt+F4 due to the reversed
/// order). The combo up and the real Alt up are let through as they arrive,
/// completing the sequence.
/// Injected keys carry LLKHF_INJECTED and are let straight through at the hook
/// entry, so they cannot recursively re-trigger gesture logic.
/// Returns whether all entries were injected successfully.
#[cfg(target_os = "windows")]
fn replay_combo_with_alt(combo_vk: VIRTUAL_KEY) -> bool {
    let inputs = [
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_LMENU,
                    wScan: 0,
                    dwFlags: 0,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: combo_vk,
                    wScan: 0,
                    dwFlags: 0,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        },
    ];
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    if sent != inputs.len() as u32 {
        log::warn!("voice shortcut failed to replay combo key with ordered Alt down");
        return false;
    }
    true
}

#[cfg(target_os = "windows")]
fn call_next_hook(code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, w_param, l_param) }
}

#[cfg(target_os = "windows")]
fn voice_shortcut_key(vk: VIRTUAL_KEY) -> VoiceShortcutKey {
    match vk {
        VK_MENU | VK_LMENU => VoiceShortcutKey::Alt,
        // Right Alt / AltGr does not trigger the voice shortcut; it stays with
        // the input method and combos.
        VK_RMENU => VoiceShortcutKey::Other,
        VK_SPACE => VoiceShortcutKey::Space,
        VK_ESCAPE => VoiceShortcutKey::Escape,
        _ => VoiceShortcutKey::Other,
    }
}

#[cfg(target_os = "windows")]
fn foreground_is_current_app(hwnd: HWND) -> bool {
    let current_pid = std::process::id();
    window_belongs_to_process(hwnd, current_pid)
        || window_belongs_to_process(unsafe { GetAncestor(hwnd, GA_ROOT) }, current_pid)
        || window_belongs_to_process(unsafe { GetAncestor(hwnd, GA_ROOTOWNER) }, current_pid)
}

#[cfg(target_os = "windows")]
fn window_belongs_to_process(hwnd: HWND, current_pid: u32) -> bool {
    if hwnd.is_null() {
        return false;
    }
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
    }
    pid == current_pid
}

#[cfg(target_os = "windows")]
fn log_shortcut_decision(
    key: VoiceShortcutKey,
    key_down: bool,
    target: Option<&str>,
    decision: VoiceShortcutDecision,
) {
    if matches!(
        key,
        VoiceShortcutKey::Alt | VoiceShortcutKey::Space | VoiceShortcutKey::Escape
    ) {
        log::debug!(
            "voice shortcut key={:?} down={} target={:?} event={:?} suppress={} inject_alt_down={}",
            key,
            key_down,
            target,
            decision.event,
            decision.suppress,
            decision.inject_alt_down
        );
    }
}

#[cfg(target_os = "windows")]
fn send_event(event: VoiceShortcutEvent, window_label: String, route: &'static str) {
    let Some(mutex) = EVENT_SENDER.get() else {
        return;
    };
    let Ok(guard) = mutex.lock() else {
        return;
    };
    if let Some(sender) = guard.as_ref() {
        match sender.send((event, window_label, route)) {
            Ok(()) => {
                log::debug!("voice shortcut queued event={:?}", event);
            }
            Err(error) => {
                log::warn!("voice shortcut queue failed: {}", error);
            }
        }
    }
}
