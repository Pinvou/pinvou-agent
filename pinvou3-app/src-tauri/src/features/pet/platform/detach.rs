//! 撕离窗口（tear-off）：把某个左侧菜单项弹成独立 WebviewWindow。
//! 模式照搬 commands::open_artifact_window（label 去重 + 聚焦已存在窗口）。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

/// 同一时刻只允许一个撕离拖拽,防止多个跟随循环并存。
static DRAG_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 全局鼠标状态(坐标 + 左键按下)。三平台轮询封装。
/// - Linux: device_query X11
/// - macOS: NSEvent 全局监听线程写的原子快照(见 macos_mouse 模块)
/// - Windows: GetCursorPos + GetAsyncKeyState 同步读
pub struct GlobalMouse {
    pub x: i32,
    pub y: i32,
    pub left_down: bool,
}

#[cfg(target_os = "linux")]
fn poll_global_mouse(dev: &device_query::DeviceState) -> GlobalMouse {
    use device_query::DeviceQuery;
    let m = dev.get_mouse();
    GlobalMouse {
        x: m.coords.0,
        y: m.coords.1,
        left_down: *m.button_pressed.get(1).unwrap_or(&false),
    }
}

#[cfg(target_os = "macos")]
fn poll_global_mouse(_dev: &()) -> GlobalMouse {
    // CoreGraphics 同步读全局光标(与 Linux device_query / Windows GetCursorPos 同构):
    // 任意线程可调、免授权(仅键盘态/事件合成才需 Accessibility)、无需事件监听器。
    macos_mouse::poll()
}

#[cfg(target_os = "windows")]
fn poll_global_mouse(_dev: &()) -> GlobalMouse {
    use std::mem::MaybeUninit;
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let mut pt = MaybeUninit::<POINT>::uninit();
    let (x, y) = unsafe {
        if GetCursorPos(pt.as_mut_ptr()) == 0 {
            return GlobalMouse {
                x: 0,
                y: 0,
                left_down: false,
            };
        }
        let pt = pt.assume_init();
        (pt.x, pt.y)
    };
    let left_down = unsafe { (GetAsyncKeyState(VK_LBUTTON as i32) as u16) & 0x8000 != 0 };
    GlobalMouse { x, y, left_down }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn poll_global_mouse(_dev: &()) -> GlobalMouse {
    GlobalMouse {
        x: 0,
        y: 0,
        left_down: false,
    }
}

/// macOS 全局鼠标:CoreGraphics 同步读取(与 Linux device_query / Windows GetCursorPos
/// 同构)。撕离拖拽的轮询线程直接调用,无需主线程调度、无事件监听器(因此也不会泄漏)。
///
/// 为什么不用 NSEvent global monitor:addGlobalMonitorForEventsMatchingMask 只收**其它
/// 应用**的事件,而撕离拖拽从 pinvou 自身 WebView 发起,事件派发给本 app,全局监听收不
/// 到 → 拖拽期间坐标/按键态永不更新,松手检测失效。CoreGraphics 读的是硬件级全局光标,
/// 与哪个 app 拥有焦点无关,正符合撕离场景。
///
/// 坐标系:CGEventGetLocation 返回主屏左上原点、Y 向下(与 X11 / Win32 一致),下游
/// main_window_contains / create_detached_at 按 Tauri PhysicalPosition(左上原点)工作,
/// 因此**无需 Y 翻转**(此前 NSEvent monitor 是左下原点才需翻转)。
///
/// 权限:鼠标位置/按键态的只读查询免授权(仅键盘态查询与事件合成才需 Accessibility /
/// Input Monitoring),可在任意线程调用。
#[cfg(target_os = "macos")]
mod macos_mouse {
    use super::GlobalMouse;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    /// CGEventSourceStateID:kCGEventSourceStateHIDSystemState = 1。
    /// 取 HID 硬件状态(而非本 app 会话状态),确保拖拽时光标按下态不被 app 捕获掩盖。
    const HID_SYSTEM_STATE: i32 = 1;
    /// CGMouseButton:kCGMouseButtonLeft = 0。
    const MOUSE_BUTTON_LEFT: u32 = 0;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreate(source: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
        fn CGEventGetLocation(event: *mut std::ffi::c_void) -> CGPoint;
        // CGEventSourceButtonState 第一参数是 CGEventSourceStateID(int32 枚举值,如
        // kCGEventSourceStateHIDSystemState = 1),**不是** CGEventSourceRef 指针。
        // 此前误声明为 *mut c_void 并先 CGEventSourceCreate 再传入,arm64 ABI 下
        // 堆指针低 32 位被当作 stateID 读取(非法枚举值),导致恒返回 false →
        // macOS 撕离拖拽 100% 失效。直接传整数枚举值即可,无需分配 source 对象。
        fn CGEventSourceButtonState(state_id: i32, button: u32) -> bool;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *mut std::ffi::c_void);
    }

    /// 同步读全局鼠标位置 + 左键按下态。任意线程可调,免授权。任一 CG 调用失败(罕见,
    /// 如 window server 异常)返回零值快照,轮询循环下一轮重试,不会崩溃。
    pub(super) fn poll() -> GlobalMouse {
        unsafe {
            let event = CGEventCreate(core::ptr::null_mut());
            if event.is_null() {
                return GlobalMouse {
                    x: 0,
                    y: 0,
                    left_down: false,
                };
            }
            let loc = CGEventGetLocation(event);
            CFRelease(event);

            // CGEventCreate 返回的位置反映当前光标;按键态直接从 HID 源读。
            // CGEventSourceButtonState 吃 CGEventSourceStateID(int32 枚举值),
            // 无需 CGEventSourceCreate 分配/释放 source 对象。
            let left_down = CGEventSourceButtonState(HID_SYSTEM_STATE, MOUSE_BUTTON_LEFT);
            GlobalMouse {
                x: if loc.x.is_finite() {
                    loc.x.round() as i32
                } else {
                    0
                },
                y: if loc.y.is_finite() {
                    loc.y.round() as i32
                } else {
                    0
                },
                left_down,
            }
        }
    }
}

/// 撕离窗口 label。Tauri label 仅允许 a-zA-Z0-9-_，故 id 用 16 位 hex 哈希而非原样拼接，
/// 避免 id 里的非法字符 / 冲突。同一 (kind,id) → 同一 label，用于去重 + 聚焦。
pub fn detached_label(kind: &str, id: Option<&str>) -> String {
    let mut h = DefaultHasher::new();
    id.unwrap_or("").hash(&mut h);
    format!("detached-{kind}-{:016x}", h.finish())
}

/// kind → 窗口标题。未知 kind 退化为通用标题。
pub fn view_title(kind: &str) -> &'static str {
    match kind {
        "session" => "对话",
        "codex-session" => "Coding 对话",
        "persona" => "专家",
        "monitor" => "系统监控",
        "toolstore" => "工具商店",
        "cardpool" => "专家卡牌池",
        "localenv" => "本地环境",
        "outputs" => "产出物",
        _ => "PINVOU",
    }
}

/// 点 (px,py) 是否落在矩形 [x, x+w) × [y, y+h) 内(物理像素，全局虚拟桌面坐标)。
/// 撕离落位判定用:松手点在主窗口外接矩形外 → 建窗;在内 → 取消。
pub fn point_in_rect(px: i32, py: i32, x: i32, y: i32, w: i32, h: i32) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
}

/// 极简 URL 编码：只转义 query 里会出问题的字符，足够 kind/id 用。
fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// 建/聚焦撕离窗口的核心。已存在同 (kind,id) 窗口则只聚焦。
/// pos=Some 时建好后把窗口左上角移到全局物理坐标(拖拽松手落位,跨屏)。
/// 撕离窗口加载同一个 index.html，带 ?detached=1&kind=&id=，前端据此只渲染该面板。
pub fn create_detached_at(
    app: &AppHandle,
    kind: &str,
    id: Option<&str>,
    pos: Option<(i32, i32)>,
) -> Result<(), String> {
    let label = detached_label(kind, id);
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.set_focus();
        return Ok(());
    }

    // UI schema 版本戳与主窗口一致，避免撕离窗口命中跨版本旧 HTML。
    // id 做 URL 编码，空 id 省略。
    let mut query = format!(
        "ui={}&detached=1&kind={}",
        crate::platform::ui_cache::UI_CACHE_SCHEMA,
        urlencode(kind)
    );
    if let Some(i) = id {
        query.push_str(&format!("&id={}", urlencode(i)));
    }
    let url = WebviewUrl::App(format!("index.html?{query}").into());

    let win = WebviewWindowBuilder::new(app, &label, url)
        .title(view_title(kind))
        .inner_size(900.0, 720.0)
        .resizable(true)
        .decorations(true)
        .build()
        .map_err(|e| format!("build detached window: {e}"))?;

    // 用 PhysicalPosition 落位:device_query 给的是全局物理像素,绕开 logical/scale 换算。
    // 落位即"全屏":先放到目标屏,再 maximize → 填满该显示器(保留标题栏可关)。
    if let Some((x, y)) = pos {
        let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
        let _ = win.maximize();
    }
    Ok(())
}

/// 建/聚焦某菜单项的撕离窗口(按钮触发,默认位置)。
pub async fn open_detached_window(
    kind: String,
    id: Option<String>,
    app: AppHandle,
) -> Result<(), String> {
    create_detached_at(&app, &kind, id.as_deref(), None)
}

/// 主窗口外接矩形是否包含全局点 (px,py)。拿不到主窗口几何 → 视为不包含(倾向于建窗)。
fn main_window_contains(app: &AppHandle, px: i32, py: i32) -> bool {
    if let Some(w) = app.get_webview_window("main") {
        if let (Ok(pos), Ok(size)) = (w.outer_position(), w.outer_size()) {
            return point_in_rect(px, py, pos.x, pos.y, size.width as i32, size.height as i32);
        }
    }
    false
}

/// 撕离拖拽起手:原生层只负责"读全局光标+左键、判松手落点"。视觉跟随由前端 DOM avatar 完成
/// (在主窗内丝滑跟手,WM 无关、无文字选中)。本函数松手时按全局落点决定建窗(主窗外那一屏
/// 最大化)或取消,并广播 detach:drag-ended 让前端收起 avatar。
pub async fn begin_detach_drag(
    kind: String,
    id: Option<String>,
    app: AppHandle,
) -> Result<(), String> {
    if DRAG_ACTIVE.swap(true, Ordering::SeqCst) {
        return Ok(()); // 已有拖拽进行中,忽略重复起手
    }

    // 硬件状态轮询(独立 OS 线程);窗口操作 marshal 回主线程。
    // macOS:轮询线程直接调 CoreGraphics 同步读全局光标(免主线程、免事件监听),
    // 无需像旧 NSEvent 方案那样在起手时 run_on_main_thread 装监听器。
    // 用 Builder::spawn(返回 Result)而非 thread::spawn(失败直接 panic),
    // 失败时复位 DRAG_ACTIVE + emit drag-ended,避免撕离功能永久卡死。
    let app_for_thread = app.clone();
    let spawn_result = std::thread::Builder::new()
        .name("detach-drag-poll".to_string())
        .spawn(move || {
            // RAII:无论循环正常退出还是 panic,都复位 DRAG_ACTIVE 并广播 drag-ended,
            // 防止撕离功能因线程异常而永久卡死(后续所有起手被当"重复"忽略)
            // 或前端 avatar 永不收起(幽灵光标)。
            struct DragGuard(AppHandle);
            impl Drop for DragGuard {
                fn drop(&mut self) {
                    DRAG_ACTIVE.store(false, Ordering::SeqCst);
                    let _ = self.0.emit("detach:drag-ended", ());
                }
            }
            let _drag_guard = DragGuard(app_for_thread.clone());

            // 平台特定的轮询设备句柄:Linux 是 device_query DeviceState;其它平台不用句柄。
            #[cfg(target_os = "linux")]
            let dev = device_query::DeviceState::new();
            #[cfg(not(target_os = "linux"))]
            let dev = ();

            let mut was_down = false;
            let mut idle_ticks = 0u32;
            loop {
                let m = poll_global_mouse(&dev);
                let (mx, my) = (m.x, m.y);
                let down = m.left_down;

                if down {
                    was_down = true;
                }
                if was_down && !down {
                    // 松手:落点在主窗外那一屏 → 最大化建窗;在内 → 取消。
                    let a2 = app_for_thread.clone();
                    let kind2 = kind.clone();
                    let id2 = id.clone();
                    let _ = app_for_thread.run_on_main_thread(move || {
                        if !main_window_contains(&a2, mx, my) {
                            let _ = create_detached_at(&a2, &kind2, id2.as_deref(), Some((mx, my)));
                        }
                    });
                    break;
                }
                if !was_down {
                    idle_ticks += 1;
                    if idle_ticks > 250 {
                        break; // ~3s 没等到按下(异常起手)→ 放弃
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(12));
            }
            // 拖拽结束(落位/取消/超时任一)→ 广播,让前端收起 avatar。
            // (DRAG_ACTIVE 复位 + drag-ended 广播均由 _drag_guard 的 Drop 保证,panic 安全。)
        });
    // spawn 失败(线程资源耗尽)时 DRAG_ACTIVE 仍为 true(上面已 swap),需复位,
    // 否则撕离功能永久卡死。emit 让前端收起 avatar。
    if spawn_result.is_err() {
        DRAG_ACTIVE.store(false, Ordering::SeqCst);
        let _ = app.emit("detach:drag-ended", ());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_is_sanitized_and_stable() {
        let a = detached_label("session", Some("s-../etc/passwd 你好"));
        assert!(a.starts_with("detached-session-"));
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        // 同输入稳定（去重/聚焦依赖此性质）
        assert_eq!(a, detached_label("session", Some("s-../etc/passwd 你好")));
    }

    #[test]
    fn label_differs_by_id_and_kind() {
        assert_ne!(
            detached_label("session", Some("a")),
            detached_label("session", Some("b"))
        );
        assert_ne!(
            detached_label("session", Some("a")),
            detached_label("persona", Some("a"))
        );
        assert_ne!(
            detached_label("monitor", None),
            detached_label("toolstore", None)
        );
    }

    #[test]
    fn view_title_known_and_fallback() {
        assert_eq!(view_title("persona"), "专家");
        assert_eq!(view_title("codex-session"), "Coding 对话");
        assert_eq!(view_title("outputs"), "产出物");
        assert_eq!(view_title("???"), "PINVOU");
    }

    #[test]
    fn urlencode_escapes_unsafe() {
        assert_eq!(urlencode("a-b_1.~"), "a-b_1.~");
        assert_eq!(urlencode("a b&c=d"), "a%20b%26c%3Dd");
    }

    #[test]
    fn point_in_rect_basic() {
        assert!(point_in_rect(10, 10, 0, 0, 100, 100));
        assert!(!point_in_rect(100, 10, 0, 0, 100, 100)); // 右边界开区间
        assert!(!point_in_rect(-1, 10, 0, 0, 100, 100));
        assert!(point_in_rect(2000, 50, 1920, 0, 1920, 1080)); // 第二屏
    }
}
