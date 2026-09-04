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
/// 路由窗口(主窗 + 撕离窗)的 HWND 缓存,供钩子线程零阻塞查询。
/// Tauri 窗口 getter 需要主线程消息回传,不能在低层钩子回调里调用,
/// 故由独立线程在开关开启时低频刷新。
#[cfg(target_os = "windows")]
static ROUTER_WINDOWS: Mutex<Vec<(isize, String)>> = Mutex::new(Vec::new());

#[cfg(target_os = "windows")]
pub(super) fn install(app: AppHandle) {
    // 允许重装:泵线程异常退出(GetMessageW 出错 / WM_QUIT)会卸钩子并复位 INSTALLED。
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

/// pump 线程死亡后的有界自动重装:GetMessageW 失败/退出会永久摘掉低层钩子,
/// 此前只能静默失效到重启。延迟 1s 后经 APP_HANDLE 重装,整个应用生命周期
/// 最多重试 3 次,用尽后记录 error(快捷键失效需重启,日志可见)。
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
        // 开启时立即补一次 HWND 缓存,避免首轮 200ms 刷新窗口内快捷键无响应。
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

    // 门控顺序:开关优先于一切 Win32 查询,disabled 时全局击键直接放行。
    if !shortcut_enabled() {
        return call_next_hook(code, w_param, l_param);
    }

    let info = unsafe { &*(l_param as *const KBDLLHOOKSTRUCT) };
    // 注入键(SendInput 合成,含本模块为组合键补发的 Alt down)不参与手势判定。
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
            // 组合键 down 已被吞:用单次 SendInput 按 [Alt↓, 组合键↓] 保序重放,
            // 系统/WebView 看到的修饰键顺序与真实按下一致,Alt+Tab/Alt+F4 正常。
            // 重放成功才确认 alt_forwarded(真实 Alt up 放行与之配对);失败则保持
            // 未转发,组合键丢失,真实 Alt up 按未转发路径吞掉收尾,不留残态。
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

/// 本次击键是否归快捷键手势接管,以及触发时的目标窗口:
/// 仅本进程前台窗口才接管;录音中的窗口优先(跨窗互斥:定向到录音窗停止,绝不双开),
/// 否则要求聚焦窗口在 Router 白名单(主窗/撕离窗)内;都不是则不吞键、不 emit。
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

/// tap-hold 补偿:组合键 down 已被吞,这里用单次 SendInput 注入两条目
/// [Alt↓, 组合键↓] 保序重放,让系统与 WebView 看到与真实按下一致的修饰键
/// 顺序(先 Alt 后组合键;旧实现先放行组合键再补发 Alt,Alt+Tab/Alt+F4 会因
/// 顺序颠倒而失效)。组合键 up 与真实 Alt up 到达时放行,完成整个序列。
/// 注入键带 LLKHF_INJECTED,钩子入口直接放行,不会递归触发手势逻辑。
/// 返回是否全部注入成功。
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
        // 右 Alt / AltGr 不触发语音快捷键,留给输入法和组合键。
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
