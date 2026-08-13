#[cfg(target_os = "windows")]
use super::{
    emit_shortcut_event, handle_voice_shortcut_key, VoiceShortcutEvent, VoiceShortcutKey,
    VoiceShortcutState,
};
#[cfg(target_os = "windows")]
use std::sync::{mpsc, Mutex, OnceLock};
use tauri::AppHandle;
#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::{
    HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VIRTUAL_KEY, VK_ESCAPE, VK_LMENU, VK_MENU, VK_RMENU, VK_SPACE,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetAncestor, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId,
    SetWindowsHookExW, GA_ROOT, GA_ROOTOWNER, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

#[cfg(target_os = "windows")]
static INSTALLED: OnceLock<()> = OnceLock::new();
#[cfg(target_os = "windows")]
static SHORTCUT_STATE: OnceLock<Mutex<VoiceShortcutState>> = OnceLock::new();
#[cfg(target_os = "windows")]
static EVENT_SENDER: OnceLock<Mutex<Option<mpsc::Sender<VoiceShortcutEvent>>>> = OnceLock::new();

#[cfg(target_os = "windows")]
pub(super) fn install(app: AppHandle) {
    if INSTALLED.set(()).is_err() {
        return;
    }

    let (tx, rx) = mpsc::channel::<VoiceShortcutEvent>();
    let _ = EVENT_SENDER.set(Mutex::new(Some(tx)));
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            emit_shortcut_event(&app, event);
        }
    });

    std::thread::spawn(|| unsafe {
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), 0 as HINSTANCE, 0);
        if hook.is_null() {
            eprintln!("[pinvou3-app] failed to install voice shortcut keyboard hook");
            return;
        }
        eprintln!("[pinvou3-app] voice shortcut keyboard hook installed");
        let mut msg = std::mem::zeroed::<MSG>();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {}
    });
}

#[cfg(not(target_os = "windows"))]
pub(super) fn install(_app: AppHandle) {}

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

    let info = unsafe { &*(l_param as *const KBDLLHOOKSTRUCT) };
    let key = voice_shortcut_key(info.vkCode as VIRTUAL_KEY);
    let foreground = foreground_is_current_app();
    let alt_pressed = alt_is_physically_pressed();
    let decision = {
        let mutex = SHORTCUT_STATE.get_or_init(|| Mutex::new(VoiceShortcutState::default()));
        let mut state = match mutex.lock() {
            Ok(guard) => guard,
            Err(_) => return call_next_hook(code, w_param, l_param),
        };
        let decision = handle_voice_shortcut_key(&mut state, key, key_down, foreground);
        if key == VoiceShortcutKey::Space && key_down && foreground && alt_pressed {
            state.alt_pending = false;
        }
        if key == VoiceShortcutKey::Space && key_up && !alt_pressed {
            state.alt_down = false;
            state.alt_pending = false;
        }
        decision
    };

    log_shortcut_decision(
        key,
        key_down,
        key_up,
        foreground,
        alt_pressed,
        decision.event,
        decision.suppress,
    );
    if let Some(event) = decision.event {
        send_event(event);
    }
    if decision.suppress {
        return 1;
    }
    call_next_hook(code, w_param, l_param)
}

#[cfg(target_os = "windows")]
fn call_next_hook(code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, w_param, l_param) }
}

#[cfg(target_os = "windows")]
fn voice_shortcut_key(vk: VIRTUAL_KEY) -> VoiceShortcutKey {
    match vk {
        VK_MENU | VK_LMENU | VK_RMENU => VoiceShortcutKey::Alt,
        VK_SPACE => VoiceShortcutKey::Space,
        VK_ESCAPE => VoiceShortcutKey::Escape,
        _ => VoiceShortcutKey::Other,
    }
}

#[cfg(target_os = "windows")]
fn foreground_is_current_app() -> bool {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return false;
    }
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
fn alt_is_physically_pressed() -> bool {
    unsafe {
        (GetAsyncKeyState(VK_MENU as i32) & 0x8000u16 as i16) != 0
            || (GetAsyncKeyState(VK_LMENU as i32) & 0x8000u16 as i16) != 0
            || (GetAsyncKeyState(VK_RMENU as i32) & 0x8000u16 as i16) != 0
    }
}

#[cfg(target_os = "windows")]
fn log_shortcut_decision(
    key: VoiceShortcutKey,
    key_down: bool,
    key_up: bool,
    foreground: bool,
    alt_pressed: bool,
    event: Option<VoiceShortcutEvent>,
    suppress: bool,
) {
    if matches!(
        key,
        VoiceShortcutKey::Alt | VoiceShortcutKey::Space | VoiceShortcutKey::Escape
    ) {
        eprintln!(
            "[pinvou3-app] voice shortcut key={:?} down={} up={} foreground={} alt_pressed={} event={:?} suppress={}",
            key, key_down, key_up, foreground, alt_pressed, event, suppress
        );
    }
}

#[cfg(target_os = "windows")]
fn send_event(event: VoiceShortcutEvent) {
    let Some(mutex) = EVENT_SENDER.get() else {
        return;
    };
    let Ok(guard) = mutex.lock() else {
        return;
    };
    if let Some(sender) = guard.as_ref() {
        match sender.send(event) {
            Ok(()) => {
                eprintln!("[pinvou3-app] voice shortcut queued event={:?}", event);
            }
            Err(error) => {
                eprintln!("[pinvou3-app] voice shortcut queue failed: {}", error);
            }
        }
    }
}
