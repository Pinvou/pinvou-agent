//! macOS 后端：xcap（CGWindowListCreateImage）/ScreenCaptureKit 截屏 + 鼠标
//! 事件原生 CGEvent 直发、键盘/滚轮经 enigo + ApplicationServices AXUIElement
//! 无障碍树。
//!
//! 坐标约定（types.rs 契约的 macOS 具体化）：
//! - 输入坐标空间 = CGEvent 全局**点**（左上角原点，多显示器可为负）。
//! - `Capture.origin_x/y` 直接用 xcap 报告的显示器原点（macOS 上
//!   CGDisplayBounds 返回点）；`Capture.input_scale_x/y` 从返回图像反推
//!   （点宽 / 实际像素宽，Retina 2x → 0.5），不信任 scale_factor 估计值；
//!   截图为物理像素。
//! - `cursor_position` 按 trait 契约返回全局**设备物理像素**（点 × 光标所在
//!   显示器的 scale），由 ScaleMap::device_to_input 乘 input_scale 还原为点。
//! - AX 返回的元素位置/尺寸同为屏幕点坐标，与输入坐标空间一致。
//!
//! 截屏实现：macOS 15.2+ 走 ScreenCaptureKit（[`super::screen_capture_kit`]，
//! `SCScreenshotManager::captureImageInRect`——该类方法 15.2 才可用）；14.x 及
//! 更早回退 xcap 的 CGWindowListCreateImage 路径（该 API 在 macOS 15 SDK 已
//! obsoleted，且在 Sequoia+ 会触发周期性"继续允许录屏"系统确认，但对旧系统
//! 仍是唯一选择）。应用最低支持 macOS 11：ScreenCaptureKit.framework 为弱
//! 链接（见 build.rs），12.3 之前的系统由 dyld 跳过并走回退。
//!
//! 权限（TCC，两条独立授权，授权后通常都需要重启本应用才生效）：
//! - Screen Recording：截屏前用 CGPreflightScreenCaptureAccess 预检，缺失返回
//!   显式 `screen_recording_denied` 错误；工具路径**不**主动触发系统弹窗。
//! - Accessibility：输入注入与 AX 树前用 AXIsProcessTrusted 预检，缺失返回显式
//!   `accessibility_denied` 错误——CGEventPost 未授权时被 WindowServer **静默
//!   丢弃**（无错误返回），不预检会出现"点击成功但什么都没发生"。
//! 授权引导由 [`request_permissions`] 提供，供集成层在设置页/引导流程调用。
//!
//! 线程约定：AXUIElement 是 CFType（!Send/!Sync）。本后端不跨调用持有任何 AX
//! 对象——所有 AXUIElement/AXValue 都在单次方法调用内于 backend worker 线程
//! 创建、使用、释放（backend.rs 保证 worker 线程亲和，对象永不离线程），因此
//! 结构体只靠自动 Send（Enigo 内部已 unsafe impl Send），无需手写 unsafe impl。
//!
//! Autorelease-pool invariant: the backend worker thread has no Obj-C
//! runloop and no autorelease pool. Every ObjC/CF call on the input/capture
//! paths below (and in [`super::screen_capture_kit`]) must therefore return
//! +1/CF-typed objects or plain primitives — no autoreleased returns. This
//! holds today, but one innocent future edit returning an autoreleased object
//! would start leaking silently; if that ever changes, wrap the call site in
//! `objc2::rc::autoreleasepool`.
//!
//! CF 层说明：AX 属性名常量（kAXRoleAttribute 等）未被
//! objc2-application-services 0.3.2 的 header-translator 生成，且
//! CFDictionary/CFArray 的泛型默认参数是 crate 私有类型，外部无法构造——
//! 因此属性名用 NSString（toll-free bridged 即 CFString）、数组/字典/布尔/
//! 释放用少量 CoreFoundation extern "C"（同 detach.rs 的 FFI 先例）。
//! AX「Copy 规则」返回的对象经 GetTypeID 前置校验后一律转入类型化智能指针
//! （`CFRetained<AXUIElement>` / `CFRetained<AXValue>` / `Retained<NSString>`）
//! 使用，本模块不对裸指针做解引用（CodeQL invalid-pointer-dereference 的
//! 根治：裸 `&*(ptr as *const T)` 桥接转型已全部移除）。

use std::ffi::c_void;
use std::fmt::Write as _;
use std::ptr::NonNull;
use std::thread::sleep;
use std::time::{Duration, Instant};

use enigo::{Axis, Direction, Enigo, Keyboard, Mouse, Settings};
use objc2::rc::Retained;
use objc2_application_services::{AXError, AXIsProcessTrusted, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};
use objc2_foundation::{NSPoint, NSSize, NSString};
use xcap::Monitor;

use super::super::backend::ComputerUseBackend;
use super::super::types::{
    Capabilities, Capture, ComputerUseError, ElementInfo, Key, MouseButton, ScrollDirection,
    UiTreeOptions,
};

/// 点击前等待先前 move 落位（目标进程消费鼠标移动事件）。
const CLICK_SETTLE_MS: u64 = 30;
/// 双击/三击中相邻两次点击的间隔（远小于系统双击时限；enigo 内部按
/// NSEvent.doubleClickInterval 维护 clickState，双/三击由它识别）。
const MULTI_CLICK_INTERVAL_MS: u64 = 40;
/// 拖拽按下后、开始移动前的停顿，给目标窗口进入拖拽识别状态的时间。
const DRAG_PRESS_SETTLE_MS: u64 = 60;
/// 拖拽路径插值步数（部分应用只有收到足够多 motion 事件才识别拖拽）。
const DRAG_STEPS: usize = 8;
/// 拖拽相邻两点的间隔。
const DRAG_STEP_DELAY_MS: u64 = 12;
/// 按下全部键与逆序释放之间的停顿，提高修饰键和弦（cmd+Tab 等）命中率。
const CHORD_HOLD_MS: u64 = 20;

/// `ui_tree` 默认/上限：深度与节点数（防止巨型树拖垮 worker 线程与文本预算）。
const DEFAULT_TREE_MAX_DEPTH: u32 = 8;
const DEFAULT_TREE_MAX_NODES: u32 = 400;
const MAX_TREE_DEPTH: u32 = 24;
const MAX_TREE_NODES: u32 = 2000;
/// 单行 name 的字符上限。
const MAX_NODE_NAME_CHARS: usize = 80;
/// AX 同步跨进程 IPC 的全局消息超时（秒）。默认约 6s，重型应用
/// （浏览器/Electron）属性读可能挂住，收紧到 2s 保护 worker 线程。
const AX_MESSAGING_TIMEOUT_SECS: f32 = 2.0;
/// `ui_tree` 整体遍历时限。单次 AX 调用有 [`AX_MESSAGING_TIMEOUT_SECS`] 兜底，
/// 但整棵树（默认 400 节点 × 每节点最多 7 次属性读）的最坏总耗时没有上界
/// （可把 worker 线程钉死约 1.5 小时）；到点即中止遍历并置 truncated，
/// 已生成的部分树仍然有效。
const AX_TREE_TIME_LIMIT: Duration = Duration::from_secs(10);
/// 连续 `AXError::CannotComplete`（目标进程无响应/挂死）的中止阈值。正常树里
/// 该错误不应连续出现；连续出现说明目标已挂死，继续遍历只会每属性白耗 2s。
const AX_CANNOT_COMPLETE_LIMIT: u32 = 3;
/// 点击落点防护的容差（点）：光标实际位置与最近一次合成移动目标的偏差超过
/// 该值时，点击/按下前先重新移动到目标。
const CLICK_POSITION_TOLERANCE_PT: i32 = 2;

/// Screen Recording 拒绝时的显式错误（授权后需重启应用才生效，错误里说明）。
const SCREEN_RECORDING_DENIED: &str = "screen_recording_denied: enable in System Settings → Privacy & Security → Screen Recording, then restart the app";
/// Accessibility 拒绝时的显式错误。
const ACCESSIBILITY_DENIED: &str = "accessibility_denied: enable in System Settings → Privacy & Security → Accessibility, then restart the app";

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreate(source: *const c_void) -> *const c_void;
    fn CGEventGetLocation(event: *const c_void) -> NSPoint;
    /// CGEventRef CGEventCreateMouseEvent(CGEventSourceRef, CGEventType,
    /// CGPoint, CGMouseButton)。类型按 SDK 原型自行声明：枚举是 u32。
    fn CGEventCreateMouseEvent(
        source: *const c_void,
        mouse_type: u32,
        at: NSPoint,
        button: u32,
    ) -> *const c_void;
    /// void CGEventSetIntegerValueField(CGEventRef, CGEventField, int64_t)。
    fn CGEventSetIntegerValueField(event: *const c_void, field: u32, value: i64);
    /// void CGEventPost(CGEventTapLocation, CGEventRef)。
    fn CGEventPost(tap: u32, event: *const c_void);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
    fn CFGetTypeID(cf: *const c_void) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFBooleanGetTypeID() -> usize;
    /// C 原型返回 `Boolean`（unsigned char），按逐类型一致原则声明为 u8
    /// 再判 `!= 0`（Rust `bool` 的 ABI 虽然实践中兼容，但不依赖它）。
    fn CFBooleanGetValue(boolean: *const c_void) -> u8;
    fn CFArrayGetTypeID() -> usize;
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
    static kCFBooleanTrue: *const c_void;
    fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        num_values: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> *const c_void;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    /// 带选项的 Accessibility 查询（kAXTrustedCheckOptionPrompt 可触发系统授权
    /// 弹窗）。签名自行声明为 *const c_void：crate 里的版本参数是
    /// Option<&CFDictionary>，而 CFDictionary 的泛型默认参数是 crate 私有类型，
    /// 外部构造不出该引用。C 原型返回 `Boolean`（unsigned char）→ u8。
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
    /// objc2-application-services 只经 crate 私有的 ConcreteType trait 暴露
    /// type_id；此处直接声明 C 符号用于 AX 对象的类型校验。
    fn AXUIElementGetTypeID() -> usize;
    fn AXValueGetTypeID() -> usize;
}

fn map_xcap_err(context: &str, err: xcap::XCapError) -> ComputerUseError {
    ComputerUseError::failed(format!("{context}: {err}"))
}

fn map_input_err(context: &str, err: enigo::InputError) -> ComputerUseError {
    ComputerUseError::failed(format!("{context}: {err}"))
}

fn screen_recording_granted() -> bool {
    CGPreflightScreenCaptureAccess()
}

fn accessibility_granted() -> bool {
    // SAFETY: 无参数、纯查询本进程 TCC 授权态，不弹窗，任意线程可调。
    unsafe { AXIsProcessTrusted() }
}

fn screen_recording_error() -> ComputerUseError {
    ComputerUseError::unavailable(SCREEN_RECORDING_DENIED)
}

fn accessibility_error() -> ComputerUseError {
    ComputerUseError::unavailable(ACCESSIBILITY_DENIED)
}

/// 焦点链（focused_element）单级属性读取失败（fail-closed）。
fn focused_chain_error(attribute: &str, err: AXError) -> ComputerUseError {
    ComputerUseError::failed(format!("reading {attribute} failed: AXError({})", err.0))
}

/// 触发两条 TCC 授权的系统弹窗（Screen Recording + Accessibility）。
/// 由 `computer_use_request_permissions` Tauri 命令经 `platform::request_permissions`
/// 调用；**工具路径不得调用**（工具只返回显式错误）。
/// 两个弹窗各自每会话最多出现一次；授权后通常需要重启本应用才生效。
pub fn request_permissions() {
    // 返回值是调用时的授权态；这里只为触发弹窗，忽略之。
    let _ = CGRequestScreenCaptureAccess();
    // AXIsProcessTrustedWithOptions({kAXTrustedCheckOptionPrompt: true})。
    // kAXTrustedCheckOptionPrompt 的字符串值恒为 "AXTrustedCheckOptionPrompt"；
    // 用 toll-free bridged 的 NSString 当键，绕开 crate 未生成该 static 的限制。
    let key = NSString::from_str("AXTrustedCheckOptionPrompt");
    let keys = [Retained::as_ptr(&key).cast::<c_void>()];
    // SAFETY: kCFBooleanTrue 是 CoreFoundation 导出的有效 CFBoolean 单例，
    // 进程生命周期内恒定有效；此处只把它的地址借给临时字典。
    let values = [unsafe { kCFBooleanTrue }];
    // SAFETY: keys/values 指向本函数栈上等长（1）数组，回调传 NULL（字典不
    // retain 键值，键值在本调用期间存活）；返回的 +1 字典在本函数内 CFRelease。
    let options = unsafe {
        CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if options.is_null() {
        return;
    }
    // SAFETY: options 是上面成功创建的有效 CFDictionaryRef；
    // AXIsProcessTrustedWithOptions 只读它并异步弹窗；CFRelease 精确释放一次。
    unsafe {
        let _ = AXIsProcessTrustedWithOptions(options) != 0;
        CFRelease(options);
    }
}

/// CGEvent 全局点坐标读光标位置（左上角原点）。免授权（仅事件合成才需
/// Accessibility），任意线程可调——与 detach.rs 的 macos_mouse 同一做法。
fn cursor_points() -> Result<(i32, i32), ComputerUseError> {
    // SAFETY: CGEventCreate 接受 NULL（默认事件源）；返回事件判空后只读坐标；
    // CFRelease 对成功创建的对象精确释放一次。
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() {
            return Err(ComputerUseError::failed(
                "CGEventCreate returned null while reading the cursor position",
            ));
        }
        let loc = CGEventGetLocation(event);
        CFRelease(event);
        if !loc.x.is_finite() || !loc.y.is_finite() {
            return Err(ComputerUseError::failed(
                "CGEventGetLocation returned non-finite coordinates",
            ));
        }
        Ok((loc.x.round() as i32, loc.y.round() as i32))
    }
}

/// 截屏目标显示器：光标所在屏（macOS 上 from_point 吃点坐标），取不到时
/// 回退主屏，再退化为第一块屏。
fn pick_monitor(cursor: Option<(i32, i32)>) -> Result<Monitor, ComputerUseError> {
    if let Some((x, y)) = cursor {
        if let Ok(monitor) = Monitor::from_point(x, y) {
            return Ok(monitor);
        }
    }
    let mut monitors =
        Monitor::all().map_err(|err| map_xcap_err("cannot enumerate monitors", err))?;
    if let Some(primary) = monitors
        .iter()
        .position(|m| m.is_primary().unwrap_or(false))
    {
        return Ok(monitors.swap_remove(primary));
    }
    monitors
        .drain(..)
        .next()
        .ok_or_else(|| ComputerUseError::unavailable("no monitor available for screen capture"))
}

/// 显示器像素/点倍率（Retina 2x → 2.0）。异常值（非有限/非正）回退 1.0。
fn monitor_scale(monitor: &Monitor) -> Result<f64, ComputerUseError> {
    let scale = f64::from(
        monitor
            .scale_factor()
            .map_err(|err| map_xcap_err("monitor scale factor", err))?,
    );
    if scale.is_finite() && scale > 0.0 {
        Ok(scale)
    } else {
        Ok(1.0)
    }
}

/// 本 crate `Key` → enigo 按键。macOS 差异：
/// - `Char` 走 `Unicode`：enigo macOS 经当前键盘布局反查 keycode。反查不中
///   （布局外字符）时 enigo 返回初始值 keycode 0——即 ANSI 'a'，会**静默
///   注入错误的键**而非报错。因此安全集合（[`is_layout_safe_char`]）之外的
///   字符在此显式失败（fail-closed）；任意文本应改走 `type_text`（Unicode
///   直注，绕过布局反查）。
/// - `Insert`：enigo 的 `Key::Insert` 变体在 macOS 上被 cfg 排除（不存在）；
///   映射到 `Help`——ANSI HELP keycode(0x72) 即 Mac 扩展键盘上 Insert 位置的
///   键，RDP/VNC/虚拟机场景均按此约定透传为 Insert。
fn map_key(key: Key) -> Result<enigo::Key, ComputerUseError> {
    use enigo::Key as EK;
    let mapped = match key {
        Key::Control => EK::Control,
        Key::Alt => EK::Alt,
        Key::Shift => EK::Shift,
        Key::Meta => EK::Meta,
        Key::Enter => EK::Return,
        Key::Escape => EK::Escape,
        Key::Tab => EK::Tab,
        Key::Space => EK::Space,
        Key::Backspace => EK::Backspace,
        Key::Delete => EK::Delete,
        Key::Insert => EK::Help,
        Key::Up => EK::UpArrow,
        Key::Down => EK::DownArrow,
        Key::Left => EK::LeftArrow,
        Key::Right => EK::RightArrow,
        Key::Home => EK::Home,
        Key::End => EK::End,
        Key::PageUp => EK::PageUp,
        Key::PageDown => EK::PageDown,
        Key::Function(n) => match n {
            1 => EK::F1,
            2 => EK::F2,
            3 => EK::F3,
            4 => EK::F4,
            5 => EK::F5,
            6 => EK::F6,
            7 => EK::F7,
            8 => EK::F8,
            9 => EK::F9,
            10 => EK::F10,
            11 => EK::F11,
            12 => EK::F12,
            other => {
                return Err(ComputerUseError::failed(format!(
                    "function key f{other} is outside the supported f1..=f12 range"
                )));
            }
        },
        Key::Char(c) => {
            if !is_layout_safe_char(c) {
                // enigo 对布局外字符返回 keycode 0（= ANSI 'a'）、对 Shift 态
                // 字符只返回基础 keycode——两种情况都会静默注入错误的键，
                // 必须在此显式失败，不能交给 enigo。
                return Err(ComputerUseError::failed(format!(
                    "character {c:?} is not representable in the current keyboard layout; \
                     use type_text for arbitrary text"
                )));
            }
            EK::Unicode(c)
        }
    };
    Ok(mapped)
}

/// enigo macOS 能把 `Key::Unicode(c)` 反查并**原样**注入的安全字符集合：
/// 无需 Shift 即可打出的美式布局字符（小写字母、数字、空格与不加 Shift 的
/// 标点）。集合外一律拒绝，原因有二（见 enigo `get_layoutdependent_keycode`
/// 与 `add_event_flag` 的实现）：
/// 1. 布局外字符（CJK、emoji、控制字符等）反查不中，enigo 返回初始值
///    keycode 0 = ANSI 'a'，静默注入 'a'；
/// 2. Shift 态字符（大写字母、`+`/`!` 等组合标点）即使反查命中，enigo 也只
///    取基础 keycode 而不附带 Shift 事件标志，注入的是未加 Shift 的错字。
fn is_layout_safe_char(c: char) -> bool {
    c.is_ascii_lowercase()
        || c.is_ascii_digit()
        || c == ' '
        || matches!(
            c,
            '`' | '-' | '=' | '[' | ']' | '\\' | ';' | '\'' | ',' | '.' | '/'
        )
}

/// CGEventType 中的鼠标事件类型（`CGEventType` 枚举的原始值）。
const CG_EVENT_LEFT_MOUSE_DOWN: u32 = 1;
const CG_EVENT_LEFT_MOUSE_UP: u32 = 2;
const CG_EVENT_RIGHT_MOUSE_DOWN: u32 = 3;
const CG_EVENT_RIGHT_MOUSE_UP: u32 = 4;
const CG_EVENT_MOUSE_MOVED: u32 = 5;
const CG_EVENT_LEFT_MOUSE_DRAGGED: u32 = 6;
const CG_EVENT_RIGHT_MOUSE_DRAGGED: u32 = 7;
const CG_EVENT_OTHER_MOUSE_DOWN: u32 = 21;
const CG_EVENT_OTHER_MOUSE_UP: u32 = 22;
const CG_EVENT_OTHER_MOUSE_DRAGGED: u32 = 25;
/// CGMouseButton 枚举原始值。
const CG_MOUSE_BUTTON_LEFT: u32 = 0;
const CG_MOUSE_BUTTON_RIGHT: u32 = 1;
const CG_MOUSE_BUTTON_CENTER: u32 = 2;
/// kCGHIDEventTap：事件注入的 tap 位置（与 enigo 的 CGEventTapLocation::HID
/// 一致）。
const CG_EVENT_TAP_HID: u32 = 0;
/// kCGMouseEventClickState：单击计数事件字段（双击/三击识别依据）。
const CG_EVENT_FIELD_CLICK_STATE: u32 = 1;

fn cg_mouse_button(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => CG_MOUSE_BUTTON_LEFT,
        MouseButton::Right => CG_MOUSE_BUTTON_RIGHT,
        MouseButton::Middle => CG_MOUSE_BUTTON_CENTER,
    }
}

/// 该鼠标键对应的 (down, up, dragged) CGEventType。
fn cg_mouse_event_types(button: MouseButton) -> (u32, u32, u32) {
    match button {
        MouseButton::Left => (
            CG_EVENT_LEFT_MOUSE_DOWN,
            CG_EVENT_LEFT_MOUSE_UP,
            CG_EVENT_LEFT_MOUSE_DRAGGED,
        ),
        MouseButton::Right => (
            CG_EVENT_RIGHT_MOUSE_DOWN,
            CG_EVENT_RIGHT_MOUSE_UP,
            CG_EVENT_RIGHT_MOUSE_DRAGGED,
        ),
        MouseButton::Middle => (
            CG_EVENT_OTHER_MOUSE_DOWN,
            CG_EVENT_OTHER_MOUSE_UP,
            CG_EVENT_OTHER_MOUSE_DRAGGED,
        ),
    }
}

/// 滚轮方向 → enigo（轴, 带符号格数）。enigo 约定：Vertical 正值向下/负值向上，
/// Horizontal 正值向右/负值向左（macOS 内部再换算为 CG 滚轮方向）。
fn map_scroll(direction: ScrollDirection, clicks: u32) -> (Axis, i32) {
    let clicks = i32::try_from(clicks).unwrap_or(i32::MAX);
    match direction {
        ScrollDirection::Up => (Axis::Vertical, -clicks),
        ScrollDirection::Down => (Axis::Vertical, clicks),
        ScrollDirection::Left => (Axis::Horizontal, -clicks),
        ScrollDirection::Right => (Axis::Horizontal, clicks),
    }
}

/// 拖拽路径的线性插值点（含终点，不含起点），保证目标收到足够 motion 事件。
fn drag_waypoints(from: (i32, i32), to: (i32, i32), steps: usize) -> Vec<(i32, i32)> {
    let steps = steps.max(1);
    (1..=steps)
        .map(|i| {
            let t = i as f64 / steps as f64;
            let x = f64::from(from.0) + f64::from(to.0 - from.0) * t;
            let y = f64::from(from.1) + f64::from(to.1 - from.1) * t;
            (x.round() as i32, y.round() as i32)
        })
        .collect()
}

/// 密码框判定：权威信号是 subrole AXSecureTextField（AppKit/Safari/Chrome 均
/// 暴露）；少数应用把安全字段直接报为 role AXSecureTextField。
fn is_secure_text(role: &str, subrole: &str) -> bool {
    subrole == "AXSecureTextField" || role == "AXSecureTextField"
}

/// 逆序释放已按下的键；即使中途出错也尽力释放全部，返回首个错误。
fn release_reverse(pressed: &[enigo::Key], enigo: &mut Enigo) -> Result<(), enigo::InputError> {
    let mut first_err = None;
    for key in pressed.iter().rev() {
        if let Err(err) = enigo.key(*key, Direction::Release) {
            if first_err.is_none() {
                first_err = Some(err);
            }
        }
    }
    match first_err {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '"' => '\'',
            '\n' | '\r' | '\t' => ' ',
            other => other,
        })
        .take(MAX_NODE_NAME_CHARS)
        .collect();
    cleaned.trim().to_string()
}

// ---------------------------------------------------------------------------
// Accessibility（AX）层
// ---------------------------------------------------------------------------

/// AX「Copy 规则」返回的 +1 CF 对象的 RAII 释放（构造时已判非空）。类型经
/// [`CfObject::type_id`] 前置校验后可用 `into_*` 转为 objc2/CF 的类型化智能
/// 指针——本模块不保留任何裸指针解引用（CodeQL invalid-pointer-dereference
/// 的根治手段，同时把类型校验收敛到单一位置）。
struct CfObject(NonNull<c_void>);

impl CfObject {
    /// 底层指针（供 CFArray/CFBoolean 的 extern 只读调用使用，不解引用）。
    fn inner(&self) -> NonNull<c_void> {
        self.0
    }

    fn type_id(&self) -> usize {
        // SAFETY: self.0 非空且指向存活的 CF 对象（本 guard 持有 +1）；
        // CFGetTypeID 是纯类型查询。
        unsafe { CFGetTypeID(self.0.as_ptr()) }
    }

    /// 校验为 CFString 后转为 +1 `Retained<NSString>`（toll-free bridged）。
    /// 类型不符返回 None（guard 照常 Drop 释放）。
    fn into_ns_string(self) -> Option<Retained<NSString>> {
        // SAFETY: CFStringGetTypeID 是纯类型查询。
        if self.type_id() != unsafe { CFStringGetTypeID() } {
            return None;
        }
        // SAFETY: self.0 指向存活的 CFString（本 guard 持有 +1，所有权随
        // from_raw 移入 Retained）；CFString 与 NSString toll-free bridged、
        // 为同一 Obj-C 对象，Retained 析构的 objc_release 与 CFRelease 等价。
        let text = unsafe { Retained::from_raw(self.0.as_ptr().cast::<NSString>()) };
        // +1 已移交 Retained：抑制 CfObject::drop，避免双重释放。
        std::mem::forget(self);
        text
    }

    /// 校验为 AXUIElement 后转为 +1 `CFRetained<AXUIElement>`。
    /// 类型不符返回 None（guard 照常 Drop 释放）。
    fn into_ui_element(self) -> Option<CFRetained<AXUIElement>> {
        // SAFETY: AXUIElementGetTypeID 是纯类型查询。
        if self.type_id() != unsafe { AXUIElementGetTypeID() } {
            return None;
        }
        // SAFETY: 已校验 AXUIElement；+1 所有权随 from_raw 移入 CFRetained
        // （析构 CFRelease，与原先的手动释放等价）。
        let element = unsafe { CFRetained::from_raw(self.0.cast::<AXUIElement>()) };
        // +1 已移交 CFRetained：抑制 CfObject::drop，避免双重释放。
        std::mem::forget(self);
        Some(element)
    }

    /// 校验为 AXValue 后转为 +1 `CFRetained<AXValue>`。
    /// 类型不符返回 None（guard 照常 Drop 释放）。
    fn into_ax_value(self) -> Option<CFRetained<AXValue>> {
        // SAFETY: AXValueGetTypeID 是纯类型查询。
        if self.type_id() != unsafe { AXValueGetTypeID() } {
            return None;
        }
        // SAFETY: 已校验 AXValue；+1 所有权随 from_raw 移入 CFRetained。
        let value = unsafe { CFRetained::from_raw(self.0.cast::<AXValue>()) };
        // +1 已移交 CFRetained：抑制 CfObject::drop，避免双重释放。
        std::mem::forget(self);
        Some(value)
    }
}

impl Drop for CfObject {
    fn drop(&mut self) {
        // SAFETY: self.0 是 AX Copy 规则返回的 +1 对象（构造时判过非空）；
        // 未经 into_* 移交所有权时在此恰好释放一次，释放后不再使用。
        unsafe { CFRelease(self.0.as_ptr()) };
    }
}

/// AX 属性名集合。kAX*Attribute 字符串常量未被 objc2-application-services
/// 0.3.2 生成，用 NSString 字面量（值稳定且文档化；toll-free bridged 即
/// CFString，可直接传给 AX API）。每次 ui_tree/element_at_point/
/// focused_element 调用构造一次，遍历内所有节点复用。
struct AxNames {
    role: Retained<NSString>,
    title: Retained<NSString>,
    /// 描述：浏览器/Electron 内 Web 内容的可访问名通常只挂在 AXDescription
    /// （AXTitle 为空），T3 词表匹配依赖它（评审缺陷 T3-Web）。
    description: Retained<NSString>,
    subrole: Retained<NSString>,
    position: Retained<NSString>,
    size: Retained<NSString>,
    enabled: Retained<NSString>,
    focused: Retained<NSString>,
    children: Retained<NSString>,
    focused_application: Retained<NSString>,
    focused_window: Retained<NSString>,
    focused_ui_element: Retained<NSString>,
}

impl AxNames {
    fn new() -> Self {
        Self {
            role: NSString::from_str("AXRole"),
            title: NSString::from_str("AXTitle"),
            description: NSString::from_str("AXDescription"),
            subrole: NSString::from_str("AXSubrole"),
            position: NSString::from_str("AXPosition"),
            size: NSString::from_str("AXSize"),
            enabled: NSString::from_str("AXEnabled"),
            focused: NSString::from_str("AXFocused"),
            children: NSString::from_str("AXChildren"),
            focused_application: NSString::from_str("AXFocusedApplication"),
            focused_window: NSString::from_str("AXFocusedWindow"),
            focused_ui_element: NSString::from_str("AXFocusedUIElement"),
        }
    }
}

/// 读单个 AX 属性（Copy 规则 +1）。返回：
/// - `Ok(Some)`：属性有值，+1 对象由 [`CfObject`] 托管释放；
/// - `Ok(None)`：调用成功但属性无值（NoValue）或返回空对象；
/// - `Err`：AX 调用失败——`AttributeUnsupported`（属性不支持）、
///   `CannotComplete`（目标进程无响应/挂死）等。树遍历按此记账中止（见
///   [`AttrErrors`]），单元素路径据此 fail-closed。
fn copy_attr(element: &AXUIElement, attribute: &NSString) -> Result<Option<CfObject>, AXError> {
    let mut raw = std::ptr::null();
    let Some(out) = NonNull::new(&mut raw) else {
        return Ok(None); // &mut 局部变量地址永不 null，此处仅避免 unwrap
    };
    // SAFETY: `out` 指向本函数栈上有效的 out 指针（NonNull<*const CFType>）；
    // attribute 是 NSString，toll-free bridged 即 CFString；仅当返回 Success
    // 且输出非空才使用输出值。
    let err = unsafe { element.copy_attribute_value(attribute.as_ref(), out) };
    match err {
        // Success 但输出空指针：按无值处理。
        AXError::Success => Ok(NonNull::new(raw.cast_mut()).map(|ptr| CfObject(ptr.cast()))),
        other => Err(other),
    }
}

fn ax_string(element: &AXUIElement, attribute: &NSString) -> Result<Option<String>, AXError> {
    let Some(obj) = copy_attr(element, attribute)? else {
        return Ok(None);
    };
    // 类型不符（非 CFString）按缺失处理：树遍历不容错中断，单元素路径由
    // ensure_readable 的全空判定兜底。
    Ok(obj.into_ns_string().map(|text| text.to_string()))
}

fn ax_bool(element: &AXUIElement, attribute: &NSString) -> Result<Option<bool>, AXError> {
    let Some(obj) = copy_attr(element, attribute)? else {
        return Ok(None);
    };
    // SAFETY: CFBooleanGetTypeID 是纯类型查询。
    if obj.type_id() != unsafe { CFBooleanGetTypeID() } {
        return Ok(None);
    }
    // SAFETY: 已校验 CFBoolean；纯读取，指针只作参数传入不解引用。
    Ok(Some(
        unsafe { CFBooleanGetValue(obj.inner().as_ptr()) } != 0,
    ))
}

/// 读一个 AXUIElement 类型属性（Copy 规则 +1，CFRetained 托管）。
fn ax_ui_element(
    element: &AXUIElement,
    attribute: &NSString,
) -> Result<Option<CFRetained<AXUIElement>>, AXError> {
    let Some(obj) = copy_attr(element, attribute)? else {
        return Ok(None);
    };
    Ok(obj.into_ui_element())
}

fn ax_point(element: &AXUIElement, attribute: &NSString) -> Result<Option<NSPoint>, AXError> {
    let Some(obj) = copy_attr(element, attribute)? else {
        return Ok(None);
    };
    let Some(value) = obj.into_ax_value() else {
        return Ok(None);
    };
    // SAFETY: point 是栈上有效 out buffer；value() 仅在类型匹配时写它并返回
    // true，此时读取才有效。
    unsafe {
        if value.r#type() != AXValueType::CGPoint {
            return Ok(None);
        }
        let mut point = NSPoint::default();
        if value.value(AXValueType::CGPoint, NonNull::from(&mut point).cast()) {
            Ok(Some(point))
        } else {
            Ok(None)
        }
    }
}

fn ax_size(element: &AXUIElement, attribute: &NSString) -> Result<Option<NSSize>, AXError> {
    let Some(obj) = copy_attr(element, attribute)? else {
        return Ok(None);
    };
    let Some(value) = obj.into_ax_value() else {
        return Ok(None);
    };
    // SAFETY: size 是栈上有效 out buffer；value() 仅在类型匹配时写它并返回
    // true，此时读取才有效。
    unsafe {
        if value.r#type() != AXValueType::CGSize {
            return Ok(None);
        }
        let mut size = NSSize::default();
        if value.value(AXValueType::CGSize, NonNull::from(&mut size).cast()) {
            Ok(Some(size))
        } else {
            Ok(None)
        }
    }
}

/// 读数组型属性（AXChildren/AXWindows）：返回 (数组 guard, 元素数)。
fn ax_array(
    element: &AXUIElement,
    attribute: &NSString,
) -> Result<Option<(CfObject, usize)>, AXError> {
    let Some(obj) = copy_attr(element, attribute)? else {
        return Ok(None);
    };
    // SAFETY: CFArrayGetTypeID 是纯类型查询。
    if obj.type_id() != unsafe { CFArrayGetTypeID() } {
        return Ok(None);
    }
    // SAFETY: 已校验 CFArray；纯计数查询。
    let count = unsafe { CFArrayGetCount(obj.inner().as_ptr()) };
    if count <= 0 {
        return Ok(None);
    }
    Ok(Some((obj, count as usize)))
}

/// 取数组第 index 个元素并 retain 为 `CFRetained`（Get 规则指针由数组持有；
/// 额外 CFRetain 让元素可越过数组 guard 的生命周期独立使用，取代原先借用
/// 数组内部的裸解引用）。
fn array_element_at(array: &CfObject, index: usize) -> Option<CFRetained<AXUIElement>> {
    // SAFETY: array 已经 CFArrayGetTypeID 校验；index < count 由调用方保证
    // （count 来自同一数组的 CFArrayGetCount，数组不可变）；AXChildren/
    // AXWindows 契约元素为 AXUIElement；Get 规则返回的指针在数组存活期间有效。
    let raw = unsafe { CFArrayGetValueAtIndex(array.inner().as_ptr(), index as isize) };
    let ptr = NonNull::new(raw.cast::<AXUIElement>().cast_mut())?;
    // SAFETY: 元素由数组 guard 持有存活；AXUIElement toll-free bridged，
    // CFRetain/CFRelease 与其 Obj-C retain/release 等价；额外的 +1 由返回的
    // CFRetained 在 drop 时释放。
    Some(unsafe { CFRetained::retain(ptr) })
}

/// 树节点的单行信息（位置/尺寸为屏幕点坐标，与输入坐标空间一致）。
#[derive(Clone)]
struct AxNodeInfo {
    role: String,
    title: String,
    /// 子角色：AXSecureTextField 判定信号；保留原文以支持「不可读」判定。
    subrole: String,
    /// 描述：Web 内容的可访问名通常在此（title 为空时的显示回退）。
    description: String,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    disabled: bool,
    focused: bool,
    secure: bool,
}

/// AX 属性读错误记账：连续 [`AX_CANNOT_COMPLETE_LIMIT`] 次
/// `AXError::CannotComplete`（目标进程无响应/挂死）置 poisoned，请求中止
/// 整棵树。任何其他读取结果（成功、属性缺失或其他错误码）都重置计数——
/// 正常树里 CannotComplete 不应连续出现，普通应用不支持某属性很常见。
struct AttrErrors {
    cannot_complete_streak: u32,
    poisoned: bool,
}

impl AttrErrors {
    fn new() -> Self {
        Self {
            cannot_complete_streak: 0,
            poisoned: false,
        }
    }

    fn record(&mut self, err: AXError) {
        if err == AXError::CannotComplete {
            self.cannot_complete_streak += 1;
            if self.cannot_complete_streak >= AX_CANNOT_COMPLETE_LIMIT {
                self.poisoned = true;
            }
        } else {
            self.cannot_complete_streak = 0;
        }
    }

    fn record_ok(&mut self) {
        self.cannot_complete_streak = 0;
    }
}

/// 树遍历的单属性读取：错误记入记账并把值降级为 None（树不因单个属性
/// 失败中断），成功则重置连续 CannotComplete 计数。
fn tree_attr<T>(
    errors: &mut AttrErrors,
    read: impl FnOnce() -> Result<Option<T>, AXError>,
) -> Option<T> {
    match read() {
        Ok(value) => {
            errors.record_ok();
            value
        }
        Err(err) => {
            errors.record(err);
            None
        }
    }
}

/// 读一个节点的展示字段。属性逐条读取：批读 API
/// AXUIElementCopyMultipleAttributeValues 需要构造 CFArray<CFString>，其泛型
/// 默认参数为 crate 私有类型，外部无法构造，故绑定层面不支持批读（性能代价由
/// 2s 消息超时 + 聚合时限 + 深度/节点预算收敛）。读取错误经 `errors` 记账后
/// 按默认值降级——树遍历不因单个属性失败中断（连续 CannotComplete 除外）。
///
/// `with_geometry` 为 false 时跳过位置/尺寸/启用/焦点四次读取（用于深度已
/// 达上限、不再下钻的节点：这些字段只影响该行展示，行内坐标显示为 0，
/// 省下 4 次跨进程 IPC）。
fn read_node_info(
    element: &AXUIElement,
    names: &AxNames,
    errors: &mut AttrErrors,
    with_geometry: bool,
) -> AxNodeInfo {
    let role = tree_attr(errors, || ax_string(element, &names.role)).unwrap_or_default();
    let title = tree_attr(errors, || ax_string(element, &names.title)).unwrap_or_default();
    let subrole = tree_attr(errors, || ax_string(element, &names.subrole)).unwrap_or_default();
    let description =
        tree_attr(errors, || ax_string(element, &names.description)).unwrap_or_default();
    let (point, size, enabled, focused) = if with_geometry {
        (
            tree_attr(errors, || ax_point(element, &names.position)),
            tree_attr(errors, || ax_size(element, &names.size)),
            tree_attr(errors, || ax_bool(element, &names.enabled)),
            tree_attr(errors, || ax_bool(element, &names.focused)),
        )
    } else {
        (None, None, None, None)
    };
    AxNodeInfo {
        secure: is_secure_text(&role, &subrole),
        role,
        title,
        subrole,
        description,
        x: point.map_or(0, |p| p.x.round() as i32),
        y: point.map_or(0, |p| p.y.round() as i32),
        width: size.map_or(0, |s| s.width.round().max(0.0) as i32),
        height: size.map_or(0, |s| s.height.round().max(0.0) as i32),
        disabled: enabled == Some(false),
        focused: focused == Some(true),
    }
}

/// 命中/焦点元素的 role、subrole、description 全部读不出时按「节点不可读」
/// 处理（fail-closed）：正常无障碍节点至少暴露 role；把属性完全读不出的
/// 元素当普通元素放行，会让 T3 词表与安全判定基于空信息放行（评审缺陷 3：
/// ax_string 失败曾被 unwrap_or_default 静默抹平）。
fn ensure_readable(info: &AxNodeInfo) -> Result<(), ComputerUseError> {
    if info.role.is_empty() && info.subrole.is_empty() && info.description.is_empty() {
        return Err(ComputerUseError::failed(
            "element attributes unreadable (role, subrole and description all missing)",
        ));
    }
    Ok(())
}

/// `AxNodeInfo` → `ElementInfo`。name 取 title，为空时回退 description——
/// 浏览器 Web 内容的可访问名通常只挂在 AXDescription 上。
fn element_info_from(info: AxNodeInfo) -> ElementInfo {
    let name = if info.title.is_empty() {
        info.description
    } else {
        info.title
    };
    ElementInfo {
        role: info.role,
        name: sanitize_name(&name),
        x: info.x,
        y: info.y,
        width: info.width,
        height: info.height,
        secure: info.secure,
    }
}

/// 树序列化的节点预算、整体时限与错误记账。
struct TreeWriter {
    next_index: u32,
    remaining: u32,
    truncated: bool,
    /// 整体遍历时限（绝对时点）。单次 AX 调用的消息超时只约束单次 IPC，
    /// 数百节点的总耗时必须另有上界，否则可把 worker 线程钉死极久。
    deadline: Instant,
    /// AX 属性读错误记账（连续 CannotComplete 中止）。
    errors: AttrErrors,
}

impl TreeWriter {
    /// 分配下一个节点序号；节点预算耗尽时置 truncated 并返回 None。
    fn take_node(&mut self) -> Option<u32> {
        if self.remaining == 0 {
            self.truncated = true;
            return None;
        }
        self.remaining -= 1;
        let index = self.next_index;
        self.next_index += 1;
        Some(index)
    }

    /// 整体时限是否已到；到点置 truncated（中止信号由 truncated 传递）。
    fn past_deadline(&mut self) -> bool {
        let past = Instant::now() >= self.deadline;
        if past {
            self.truncated = true;
        }
        past
    }
}

/// 单行格式：`[i] role "title" (x,y,w,h) flags`，flags 省略时为无。
fn format_tree_line(index: u32, depth: u32, info: &AxNodeInfo) -> String {
    let mut flags = String::new();
    if info.disabled {
        flags.push_str(" disabled");
    }
    if info.focused {
        flags.push_str(" focused");
    }
    if info.secure {
        flags.push_str(" password");
    }
    // Web 内容（浏览器/Electron）的可访问名通常挂在 AXDescription：title 为
    // 空时显示 description，行格式保持不变（评审缺陷 T3-Web）。
    let display_title = if info.title.is_empty() {
        &info.description
    } else {
        &info.title
    };
    let mut line = String::new();
    let _ = write!(
        line,
        "{indent}[{index}] {role} \"{title}\" ({x},{y},{w},{h}){flags}",
        indent = "  ".repeat(depth as usize),
        role = info.role,
        title = sanitize_name(display_title),
        x = info.x,
        y = info.y,
        w = info.width,
        h = info.height,
    );
    line
}

fn write_tree_node(
    element: &AXUIElement,
    depth: u32,
    max_depth: u32,
    names: &AxNames,
    writer: &mut TreeWriter,
    out: &mut String,
) {
    // 聚合时限：到点即中止（past_deadline 已置 truncated），已输出的子树
    // 保持有效。
    if writer.past_deadline() {
        return;
    }
    let Some(index) = writer.take_node() else {
        return;
    };
    // 深度已达上限的节点不再下钻，跳过位置/尺寸/启用/焦点四次读取。
    let info = read_node_info(element, names, &mut writer.errors, depth < max_depth);
    let _ = writeln!(out, "{}", format_tree_line(index, depth, &info));
    // 连续 CannotComplete 判定目标进程挂死：中止整棵树并置 truncated，
    // 避免在无响应应用上每属性白耗一个消息超时。
    if writer.errors.poisoned {
        writer.truncated = true;
        return;
    }
    if depth >= max_depth {
        return;
    }
    match ax_array(element, &names.children) {
        Ok(Some((children, count))) => {
            for i in 0..count {
                let Some(child) = array_element_at(&children, i) else {
                    continue;
                };
                // 元素可能在遍历中途销毁或属性读取失败：由 truncated 统一
                // 传递中止信号（预算耗尽/超时/连续 CannotComplete）。
                write_tree_node(&child, depth + 1, max_depth, names, writer, out);
                if writer.truncated {
                    break;
                }
            }
        }
        // 无子节点（叶子或属性缺失）：按「该分支结束」处理；children 读取
        // 连续 CannotComplete 达到阈值时立即置 truncated 中止整棵树。
        Ok(None) => {}
        Err(err) => {
            writer.errors.record(err);
            if writer.errors.poisoned {
                writer.truncated = true;
            }
        }
    }
}

/// 树的根：焦点应用的焦点窗口；应用无焦点窗口（隐藏/仅菜单栏）或读取失败
/// 时退化为应用根（含菜单栏等，由深度/节点预算收敛）。
fn focused_window_root(system: &AXUIElement, names: &AxNames) -> Option<CFRetained<AXUIElement>> {
    let app = match ax_ui_element(system, &names.focused_application) {
        Ok(Some(app)) => app,
        // 无焦点应用：树不可用。
        _ => return None,
    };
    match ax_ui_element(&app, &names.focused_window) {
        Ok(Some(window)) => Some(window),
        _ => Some(app),
    }
}

pub(super) struct MacosComputerUseBackend {
    /// 懒构造：Accessibility 未授权时 Enigo::new 会直接失败，延迟到首个键盘/
    /// 滚轮操作再构造——截屏/光标等免授权能力在权限缺失时仍可用，也避免权限
    /// 后补授权后整个 backend 粘性失败（backend 工厂失败是粘性的）。
    /// **只承载键盘（key/text）与滚轮**：鼠标事件（移动/点击/拖拽）全部经
    /// [`Self::post_mouse_event`] 直发 CGEvent——enigo 0.6.1 的
    /// `button()`/`move_mouse()` 依赖 `location()`，而后者把 NSEvent 的
    /// **点**坐标用 CGDisplay 的**物理像素**高翻转，Retina/多显示器上结果
    /// 落在屏幕外（点错位置），crates.io 最新发布版（0.6.1）无修复。
    enigo: Option<Enigo>,
    /// 最近一次合成移动落定的目标点（点坐标），作为 click/mouse_down 的落点
    /// 与落点防护的基准。点击落在系统当前鼠标位置：用户物理移动鼠标与
    /// 我们的合成移动存在竞态（评审缺陷 6），点击前据此校验并重定位。
    pending_move_target: Option<(i32, i32)>,
}

impl MacosComputerUseBackend {
    fn new() -> Self {
        Self {
            enigo: None,
            pending_move_target: None,
        }
    }

    /// 取输入注入器。Accessibility 未授权时返回显式错误——CGEventPost 未授权
    /// 时被 WindowServer 静默丢弃，预检是正确性的必要部分。
    fn enigo(&mut self) -> Result<&mut Enigo, ComputerUseError> {
        if !accessibility_granted() {
            return Err(accessibility_error());
        }
        if self.enigo.is_none() {
            // open_prompt_to_get_permissions=false：权限引导统一走
            // request_permissions()，工具路径只返回显式错误，不弹系统窗。
            let settings = Settings {
                open_prompt_to_get_permissions: false,
                ..Settings::default()
            };
            self.enigo = Some(Enigo::new(&settings).map_err(|err| {
                ComputerUseError::unavailable(format!(
                    "cannot initialize CGEvent input backend: {err}"
                ))
            })?);
        }
        match self.enigo.as_mut() {
            Some(enigo) => Ok(enigo),
            None => Err(ComputerUseError::unavailable(
                "CGEvent input backend is not initialized",
            )),
        }
    }

    /// 点击/按下前的落点防护：读系统光标实际位置，与最近一次合成移动目标
    /// 偏差超过 [`CLICK_POSITION_TOLERANCE_PT`] 时先重新移动到目标再继续——
    /// 点击落在系统当前鼠标位置，用户物理移动鼠标会与之竞态。没有
    /// 可信目标（尚未 move_to）时维持现状；光标位置读不出时显式失败
    /// （fail-closed：不知道落点的点击不可放行）。
    fn guard_click_position(&mut self) -> Result<(), ComputerUseError> {
        let Some(target) = self.pending_move_target else {
            return Ok(());
        };
        let (cx, cy) = cursor_points()?;
        if (cx - target.0).abs() <= CLICK_POSITION_TOLERANCE_PT
            && (cy - target.1).abs() <= CLICK_POSITION_TOLERANCE_PT
        {
            return Ok(());
        }
        self.post_move(target)?;
        sleep(Duration::from_millis(CLICK_SETTLE_MS));
        Ok(())
    }

    /// 无坐标点击/按下/释放的落点：优先最近一次合成移动目标（可信），否则
    /// 用系统光标实际位置（免授权可读）。
    fn click_destination(&self) -> Result<(i32, i32), ComputerUseError> {
        match self.pending_move_target {
            Some(target) => Ok(target),
            None => cursor_points(),
        }
    }

    /// 合成一个鼠标事件（CGEventPost at kCGHIDEventTap，与 enigo 的注入
    /// 位置一致）。`click_state > 0` 时附带 kCGMouseEventClickState——双击/
    /// 三击识别的依据。Accessibility 缺失时 CGEventPost 被静默丢弃，先预检。
    fn post_mouse_event(
        &mut self,
        event_type: u32,
        cg_button: u32,
        at: (i32, i32),
        click_state: i64,
    ) -> Result<(), ComputerUseError> {
        if !accessibility_granted() {
            return Err(accessibility_error());
        }
        // SAFETY: CGEventCreateMouseEvent 接受 NULL source（默认事件源）；
        // 返回 +1 事件判空后使用，CGEventPost 只消费不持有，CFRelease 精确
        // 释放一次；at 是纯值参数。
        let event = unsafe {
            CGEventCreateMouseEvent(
                std::ptr::null(),
                event_type,
                NSPoint {
                    x: f64::from(at.0),
                    y: f64::from(at.1),
                },
                cg_button,
            )
        };
        if event.is_null() {
            return Err(ComputerUseError::failed(format!(
                "CGEventCreateMouseEvent returned null for event type {event_type}"
            )));
        }
        // SAFETY: event 是上面成功创建的有效 CGEventRef；click_state 字段按
        // CG 文档写入（多击计数）。
        unsafe {
            if click_state > 0 {
                CGEventSetIntegerValueField(event, CG_EVENT_FIELD_CLICK_STATE, click_state);
            }
            CGEventPost(CG_EVENT_TAP_HID, event);
            CFRelease(event);
        }
        Ok(())
    }

    /// 合成一次鼠标移动（MouseMoved，不带 clickState）。
    fn post_move(&mut self, at: (i32, i32)) -> Result<(), ComputerUseError> {
        self.post_mouse_event(CG_EVENT_MOUSE_MOVED, CG_MOUSE_BUTTON_LEFT, at, 0)
    }

    /// 按下全部键（出错时回滚已按下的），停顿，再逆序释放。
    fn chord(&mut self, keys: &[Key], hold: Duration) -> Result<(), ComputerUseError> {
        let enigo = self.enigo()?;
        let mut pressed: Vec<enigo::Key> = Vec::with_capacity(keys.len());
        for key in keys {
            let mapped = map_key(*key)?;
            if let Err(err) = enigo.key(mapped, Direction::Press) {
                let _ = release_reverse(&pressed, enigo);
                return Err(map_input_err("key press", err));
            }
            pressed.push(mapped);
        }
        sleep(hold);
        release_reverse(&pressed, enigo).map_err(|err| map_input_err("key release", err))
    }

    /// ScreenCaptureKit 截屏（macOS 15.2+）。captureImageInRect 按显示器原生
    /// 倍率返回物理像素，实际倍率从返回图像反推（点 / 实际像素宽）——不信任
    /// scale_factor 估计值，对旋转屏/非整数倍率也成立。
    fn capture_via_screen_capture_kit(
        &mut self,
        origin_x: i32,
        origin_y: i32,
        width_points: f64,
        height_points: f64,
    ) -> Result<Capture, ComputerUseError> {
        let shot = super::screen_capture_kit::capture_region(
            origin_x,
            origin_y,
            width_points.round() as i32,
            height_points.round() as i32,
        )?;
        let mut rgba = shot.rgba;
        // ScreenCaptureKit 输出预乘 alpha；单趟置不透明，避免下游 PNG 编码
        // 透出无意义 alpha（与 xcap 回退路径同一约定）。
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        let actual_scale = width_points / shot.width.max(1) as f64;
        Ok(Capture {
            rgba,
            width: shot.width as u32,
            height: shot.height as u32,
            origin_x,
            origin_y,
            input_scale_x: actual_scale,
            input_scale_y: actual_scale,
        })
    }
}

impl ComputerUseBackend for MacosComputerUseBackend {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            screenshot: true,
            input: true,
            ui_tree: true,
            notes: "macos: all input coordinates are CGEvent global points (screenshot px / \
                    backing scale factor); mouse events are posted as native CGEvents and \
                    keyboard/wheel go through enigo; Screen Recording + Accessibility TCC \
                    grants are required and usually need an app restart after granting; \
                    macOS 26 (Tahoe) may filter synthetic modifier-key events aimed at global \
                    hotkey listeners; multi-click recognition does not verify that repeated \
                    clicks land on the same point; key-chord characters outside the safe \
                    keyboard-layout set are rejected with an explicit error instead of \
                    injecting a wrong key; T3 target screening relies on the AX tree — \
                    windows that expose no accessibility data (some Electron/web-contents \
                    windows) report no element at the target point, so consequential targets \
                    inside them cannot be recognized and are NOT confirmation-screened"
                .to_string(),
        }
    }

    fn capture(&mut self) -> Result<Capture, ComputerUseError> {
        if !screen_recording_granted() {
            return Err(screen_recording_error());
        }
        let cursor = cursor_points().ok();
        let monitor = pick_monitor(cursor)?;
        // 显示器原点是 CGDisplayBounds = 点（输入坐标空间）；width/height
        // 同为点。
        let origin_x = monitor
            .x()
            .map_err(|err| map_xcap_err("monitor origin", err))?;
        let origin_y = monitor
            .y()
            .map_err(|err| map_xcap_err("monitor origin", err))?;
        let width_points = f64::from(
            monitor
                .width()
                .map_err(|err| map_xcap_err("monitor size", err))?,
        );
        let height_points = f64::from(
            monitor
                .height()
                .map_err(|err| map_xcap_err("monitor size", err))?,
        );
        if super::screen_capture_kit::available() {
            return self.capture_via_screen_capture_kit(
                origin_x,
                origin_y,
                width_points,
                height_points,
            );
        }
        // macOS 15.2 以下回退：xcap 的 CGWindowListCreateImage 路径。截图为
        // 设备物理像素（xcap 已做 BGRA→RGBA 行修复）。实际倍率从返回图像
        // 反推（点 / 实际像素宽）——不信任 scale_factor 估计值，与 SCK 路径
        // 同一策略，对旋转屏/非整数倍率也成立。
        let image = monitor
            .capture_image()
            .map_err(|err| map_xcap_err("screen capture", err))?;
        let width = image.width();
        let height = image.height();
        let actual_scale = width_points / f64::from(width.max(1));
        let mut rgba = image.into_raw();
        // CGWindowListCreateImage 的 alpha 通道依内容而定；单趟置不透明，
        // 避免下游 PNG 编码透出无意义 alpha。
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        Ok(Capture {
            rgba,
            width,
            height,
            origin_x,
            origin_y,
            input_scale_x: actual_scale,
            input_scale_y: actual_scale,
        })
    }

    fn cursor_position(&mut self) -> Result<(i32, i32), ComputerUseError> {
        // trait 契约：全局设备物理像素 = CGEvent 点 × 光标所在显示器的 scale。
        let (px, py) = cursor_points()?;
        let monitor = pick_monitor(Some((px, py)))?;
        let scale = monitor_scale(&monitor)?;
        Ok((
            (f64::from(px) * scale).round() as i32,
            (f64::from(py) * scale).round() as i32,
        ))
    }

    fn move_to(&mut self, x: i32, y: i32) -> Result<(), ComputerUseError> {
        // x/y 是 CGEvent 全局点坐标（工具层已按 input_scale 换算）。原生直发
        // MouseMoved：enigo 的 move_mouse 在 Retina 上以坏掉的 location() 计算
        // delta 字段。
        self.post_move((x, y))?;
        // 记录可信落点，供后续 click/mouse_down 的落点防护与落点选择使用。
        self.pending_move_target = Some((x, y));
        Ok(())
    }

    fn click(&mut self, button: MouseButton, count: u8) -> Result<(), ComputerUseError> {
        // 落点防护先行：用户物理移动鼠标与合成移动竞态时，点击会落在用户
        // 光标处（评审缺陷 6）。
        self.guard_click_position()?;
        let dest = self.click_destination()?;
        // 让先前的 move 落位再点击，避免点在旧光标位置。
        sleep(Duration::from_millis(CLICK_SETTLE_MS));
        let (down, up, _) = cg_mouse_event_types(button);
        let cg_button = cg_mouse_button(button);
        let rounds = u32::from(count.max(1));
        for i in 0..rounds {
            // clickState 从 1 递增：系统/应用据此识别双击与三击。
            let click_state = i64::from(i) + 1;
            self.post_mouse_event(down, cg_button, dest, click_state)?;
            // down 成功、up 失败（TCC 中途吊销、事件分配失败）会让物理按键
            // 卡在按下状态，劫持用户的下一次物理点击（评审发现）——释放
            // 失败重试一次，仍失败则上报"按键可能未释放"而不是静默返回。
            if let Err(error) = self.post_mouse_event(up, cg_button, dest, click_state) {
                sleep(Duration::from_millis(MULTI_CLICK_INTERVAL_MS));
                self.post_mouse_event(up, cg_button, dest, click_state)
                    .map_err(|retry| {
                        ComputerUseError::failed(format!(
                            "click release failed twice ({error}; {retry}); \
                             the mouse button may still be pressed"
                        ))
                    })?;
            }
            if i + 1 < rounds {
                sleep(Duration::from_millis(MULTI_CLICK_INTERVAL_MS));
            }
        }
        Ok(())
    }

    fn mouse_down(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        // 落点防护同 click：按下位置错了，后续拖拽/选择全错。
        self.guard_click_position()?;
        let dest = self.click_destination()?;
        sleep(Duration::from_millis(CLICK_SETTLE_MS));
        let (down, _, _) = cg_mouse_event_types(button);
        self.post_mouse_event(down, cg_mouse_button(button), dest, 1)
    }

    fn mouse_up(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        // 释放不做落点防护/重定位（down→move→up 的 up 必须落在拖拽终点）。
        let dest = self.click_destination()?;
        let (_, up, _) = cg_mouse_event_types(button);
        self.post_mouse_event(up, cg_mouse_button(button), dest, 1)
    }

    fn drag(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<(), ComputerUseError> {
        let (down, up, dragged) = cg_mouse_event_types(MouseButton::Left);
        let left = cg_mouse_button(MouseButton::Left);
        self.post_move(from)?;
        sleep(Duration::from_millis(CLICK_SETTLE_MS));
        self.post_mouse_event(down, left, from, 1)?;
        sleep(Duration::from_millis(DRAG_PRESS_SETTLE_MS));
        let result = (|| {
            for (x, y) in drag_waypoints(from, to, DRAG_STEPS) {
                // 按住期间发 LeftMouseDragged（部分应用只识别 dragged 类型）。
                self.post_mouse_event(dragged, left, (x, y), 1)?;
                sleep(Duration::from_millis(DRAG_STEP_DELAY_MS));
            }
            Ok(())
        })();
        // 无论路径移动是否出错都必须释放按键，避免鼠标卡在按下状态。
        // Inspect both results instead of `Result::and`, which keeps only the
        // first error: when the release fails too, the caller must still
        // learn that the button may be stranded pressed (mirrors click()'s
        // stranded-button wording).
        let release = self.post_mouse_event(up, left, to, 1);
        match (result, release) {
            (Ok(()), Ok(())) => {}
            // Only the path move failed and the release succeeded: report the
            // move error unchanged.
            (Err(move_error), Ok(())) => return Err(move_error),
            (Ok(()), Err(release)) => {
                return Err(ComputerUseError::failed(format!(
                    "drag release failed ({release}); the mouse button may still be pressed"
                )));
            }
            (Err(move_error), Err(release)) => {
                return Err(ComputerUseError::failed(format!(
                    "drag move failed ({move_error}); its release also failed ({release}); \
                     the mouse button may still be pressed"
                )));
            }
        }
        // 拖拽终点即光标落点，刷新可信落点基准（起点 move 不改动）。
        self.pending_move_target = Some(to);
        Ok(())
    }

    fn scroll(&mut self, direction: ScrollDirection, clicks: u32) -> Result<(), ComputerUseError> {
        let (axis, length) = map_scroll(direction, clicks);
        self.enigo()?
            .scroll(length, axis)
            .map_err(|err| map_input_err("scroll", err))
    }

    fn type_text(&mut self, text: &str) -> Result<(), ComputerUseError> {
        // Unicode 直注（CGEventKeyboardSetUnicodeString，enigo 内部按 20 字符
        // 分块），绕过 IME——中文直接落进焦点字段。
        self.enigo()?
            .text(text)
            .map_err(|err| map_input_err("type text", err))
    }

    fn key_chord(&mut self, keys: &[Key]) -> Result<(), ComputerUseError> {
        self.chord(keys, Duration::from_millis(CHORD_HOLD_MS))
    }

    fn hold_key(&mut self, keys: &[Key], ms: u64) -> Result<(), ComputerUseError> {
        self.chord(keys, Duration::from_millis(ms))
    }

    fn ui_tree(&mut self, opts: &UiTreeOptions) -> Result<String, ComputerUseError> {
        if !accessibility_granted() {
            return Err(accessibility_error());
        }
        let max_depth = opts
            .max_depth
            .unwrap_or(DEFAULT_TREE_MAX_DEPTH)
            .min(MAX_TREE_DEPTH);
        let max_nodes = opts
            .max_nodes
            .unwrap_or(DEFAULT_TREE_MAX_NODES)
            .min(MAX_TREE_NODES);
        let names = AxNames::new();
        // SAFETY: AXUIElementCreateSystemWide 任意线程可调；crate 保证返回非
        // null，retain 计数由 CFRetained 管理；对象不离本调用/本线程。
        let system = unsafe { AXUIElement::new_system_wide() };
        // SAFETY: system 是有效的系统级 AXUIElement；设置全局消息超时防止重型
        // 应用的同步 IPC 拖死 worker 线程。失败忽略（退回默认超时）。
        let _ = unsafe { system.set_messaging_timeout(AX_MESSAGING_TIMEOUT_SECS) };
        let Some(root) = focused_window_root(&system, &names) else {
            return Err(ComputerUseError::unavailable(
                "no focused application is available for ui_tree",
            ));
        };
        let mut out = String::new();
        let mut writer = TreeWriter {
            next_index: 0,
            remaining: max_nodes,
            truncated: false,
            // 聚合时限：数百节点 × 多次属性读的最坏总耗时远超单次消息超时。
            deadline: Instant::now() + AX_TREE_TIME_LIMIT,
            errors: AttrErrors::new(),
        };
        write_tree_node(&root, 0, max_depth, &names, &mut writer, &mut out);
        if writer.truncated {
            // 如实标注中止原因：目标挂死（连续 CannotComplete）/ 超时 / 节点
            // 预算耗尽。
            let reason = if writer.errors.poisoned {
                "the focused application stopped responding"
            } else if Instant::now() >= writer.deadline {
                "the traversal time budget was exceeded"
            } else {
                "the node budget was exhausted"
            };
            let _ = writeln!(
                out,
                "... (truncated after {} nodes: {reason}; pass a larger max_nodes to see more)",
                writer.next_index
            );
        }
        Ok(out)
    }

    fn element_at_point(
        &mut self,
        x: i32,
        y: i32,
    ) -> Result<Option<ElementInfo>, ComputerUseError> {
        if !accessibility_granted() {
            return Err(accessibility_error());
        }
        let names = AxNames::new();
        // SAFETY: 同 ui_tree。
        let system = unsafe { AXUIElement::new_system_wide() };
        // SAFETY: 同 ui_tree。
        let _ = unsafe { system.set_messaging_timeout(AX_MESSAGING_TIMEOUT_SECS) };
        let mut raw: *const AXUIElement = std::ptr::null();
        let Some(out) = NonNull::new(&mut raw) else {
            return Err(ComputerUseError::failed(
                "cannot create out pointer for element hit-test",
            ));
        };
        // SAFETY: out 指向本函数栈上有效 out 指针；x/y 是 CGEvent 全局点坐标，
        // 与 AXUIElementCopyElementAtPosition 期望的 top-left 屏幕点坐标一致
        // （该 API 内部按窗口 z-order 命中）；仅当 Success 且输出非空才使用。
        let err = unsafe { system.copy_element_at_position(x as f32, y as f32, out) };
        match err {
            AXError::Success => {}
            // 该点无任何 AX 对象（桌面空隙等）。
            AXError::NoValue => return Ok(None),
            other => {
                return Err(ComputerUseError::failed(format!(
                    "AXUIElementCopyElementAtPosition failed: AXError({})",
                    other.0
                )));
            }
        }
        let ptr = NonNull::new(raw.cast::<c_void>().cast_mut())
            .ok_or_else(|| ComputerUseError::failed("element hit-test returned a null element"))?;
        // 类型校验后转入类型化 CFRetained（不再对裸指针解引用）。
        let element = CfObject(ptr).into_ui_element().ok_or_else(|| {
            ComputerUseError::failed("element hit-test returned an unexpected CF type")
        })?;
        let mut errors = AttrErrors::new();
        let info = read_node_info(&element, &names, &mut errors, true);
        // 命中元素 role/subrole/description 全部读不出时按不可读处理
        // （fail-closed）：不把未知元素当普通元素 Clear 放行。
        ensure_readable(&info)?;
        Ok(Some(element_info_from(info)))
    }

    fn focused_element(&mut self) -> Result<Option<ElementInfo>, ComputerUseError> {
        if !accessibility_granted() {
            return Err(accessibility_error());
        }
        let names = AxNames::new();
        // SAFETY: 同 ui_tree。
        let system = unsafe { AXUIElement::new_system_wide() };
        // SAFETY: 同 ui_tree。
        let _ = unsafe { system.set_messaging_timeout(AX_MESSAGING_TIMEOUT_SECS) };
        // 焦点链逐级下钻：system → AXFocusedApplication → AXFocusedWindow →
        // AXFocusedUIElement。无焦点应用/窗口/元素返回 Ok(None)；任何一级 AX
        // 错误显式失败（fail-closed），绝不猜测一个「大概是焦点」的元素。
        let app = match ax_ui_element(&system, &names.focused_application) {
            Ok(Some(app)) => app,
            Ok(None) => return Ok(None),
            Err(err) => return Err(focused_chain_error("AXFocusedApplication", err)),
        };
        let window = match ax_ui_element(&app, &names.focused_window) {
            Ok(Some(window)) => window,
            Ok(None) => return Ok(None),
            Err(err) => return Err(focused_chain_error("AXFocusedWindow", err)),
        };
        let element = match ax_ui_element(&window, &names.focused_ui_element) {
            Ok(Some(element)) => element,
            Ok(None) => return Ok(None),
            Err(err) => return Err(focused_chain_error("AXFocusedUIElement", err)),
        };
        let mut errors = AttrErrors::new();
        let info = read_node_info(&element, &names, &mut errors, true);
        // 与 element_at_point 同一不可读判定：全空属性不构造 ElementInfo。
        ensure_readable(&info)?;
        Ok(Some(element_info_from(info)))
    }
}

pub(super) fn create_backend() -> Result<Box<dyn ComputerUseBackend>, ComputerUseError> {
    Ok(Box::new(MacosComputerUseBackend::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_mapping_covers_named_keys_and_functions() {
        use enigo::Key as EK;
        let cases: &[(Key, EK)] = &[
            (Key::Control, EK::Control),
            (Key::Alt, EK::Alt),
            (Key::Shift, EK::Shift),
            (Key::Meta, EK::Meta),
            (Key::Enter, EK::Return),
            (Key::Escape, EK::Escape),
            (Key::Tab, EK::Tab),
            (Key::Space, EK::Space),
            (Key::Backspace, EK::Backspace),
            (Key::Delete, EK::Delete),
            // macOS 无 Insert 变体，映射到 HELP keycode（Insert 位置的键）。
            (Key::Insert, EK::Help),
            (Key::Up, EK::UpArrow),
            (Key::Down, EK::DownArrow),
            (Key::Left, EK::LeftArrow),
            (Key::Right, EK::RightArrow),
            (Key::Home, EK::Home),
            (Key::End, EK::End),
            (Key::PageUp, EK::PageUp),
            (Key::PageDown, EK::PageDown),
            (Key::Function(1), EK::F1),
            (Key::Function(6), EK::F6),
            (Key::Function(12), EK::F12),
        ];
        for (input, expected) in cases {
            assert_eq!(map_key(*input).ok(), Some(*expected), "mapping {input:?}");
        }
        assert_eq!(map_key(Key::Char('s')).ok(), Some(EK::Unicode('s')));
        // 布局外字符显式报错（enigo 会退化为 keycode 0 = ANSI 'a'，注入错键）。
        assert!(map_key(Key::Char('中')).is_err());
        assert!(map_key(Key::Char('\u{1F600}')).is_err());
        // Shift 态字符同样拒绝：enigo 反查后不附带 Shift 标志，注入的是
        // 未加 Shift 的错字（'+' 实际打出 '='）。
        assert!(map_key(Key::Char('+')).is_err());
        assert!(map_key(Key::Char('A')).is_err());
        // 无需 Shift 的美式布局字符仍直接映射。
        assert_eq!(map_key(Key::Char('/')).ok(), Some(EK::Unicode('/')));
        assert_eq!(map_key(Key::Char('-')).ok(), Some(EK::Unicode('-')));
        assert!(map_key(Key::Function(0)).is_err());
        assert!(map_key(Key::Function(13)).is_err());
    }

    #[test]
    fn layout_safe_chars_match_unshifted_us_keys() {
        // 无需 Shift 的美式布局字符全部放行。
        for c in "az09 `-=[]\\;',./".chars() {
            assert!(is_layout_safe_char(c), "{c:?} must be layout-safe");
        }
        assert!(is_layout_safe_char(' '));
        // 大写字母、Shift 组合标点与非 ASCII 一律拒绝。
        for c in [
            'A', 'Z', '!', '@', '#', '$', '%', '^', '&', '*', '(', ')', '_', '+', '{', '}', '|',
            ':', '"', '<', '>', '?', '~', '中', '\n',
        ] {
            assert!(!is_layout_safe_char(c), "{c:?} must be rejected");
        }
    }

    #[test]
    fn consecutive_cannot_complete_poisons_and_other_results_reset() {
        let mut errors = AttrErrors::new();
        errors.record(AXError::CannotComplete);
        errors.record(AXError::CannotComplete);
        assert!(!errors.poisoned);
        // 其他结果（成功/普通错误码）重置连续计数。
        errors.record(AXError::AttributeUnsupported);
        errors.record_ok();
        errors.record(AXError::CannotComplete);
        errors.record(AXError::CannotComplete);
        assert!(!errors.poisoned);
        errors.record(AXError::CannotComplete);
        assert!(
            errors.poisoned,
            "three consecutive CannotComplete must poison the traversal"
        );
    }

    #[test]
    fn tree_writer_flags_truncation_on_budget_and_deadline() {
        let mut writer = TreeWriter {
            next_index: 0,
            remaining: 2,
            truncated: false,
            deadline: Instant::now() + AX_TREE_TIME_LIMIT,
            errors: AttrErrors::new(),
        };
        assert!(!writer.past_deadline());
        assert_eq!(writer.take_node(), Some(0));
        assert_eq!(writer.take_node(), Some(1));
        // 节点预算耗尽：置 truncated 且不再分配序号。
        assert_eq!(writer.take_node(), None);
        assert!(writer.truncated);

        let mut expired = TreeWriter {
            next_index: 0,
            remaining: 10,
            truncated: false,
            // Instant 不支持减法溢出，用 checked_sub 构造过去时点。
            deadline: Instant::now()
                .checked_sub(Duration::from_secs(1))
                .expect("a past instant exists on any platform with a clock"),
            errors: AttrErrors::new(),
        };
        assert!(expired.past_deadline());
        assert!(expired.truncated);
    }

    #[test]
    fn element_info_prefers_title_and_rejects_unreadable_nodes() {
        let base = AxNodeInfo {
            role: "AXButton".to_string(),
            title: String::new(),
            subrole: String::new(),
            // Web 内容：可访问名只在 description。
            description: "Submit search".to_string(),
            x: 1,
            y: 2,
            width: 3,
            height: 4,
            disabled: false,
            focused: false,
            secure: false,
        };
        assert!(ensure_readable(&base).is_ok());
        let info = element_info_from(base.clone());
        assert_eq!(info.name, "Submit search");
        assert_eq!(info.role, "AXButton");
        let titled = AxNodeInfo {
            title: "确定".to_string(),
            ..base.clone()
        };
        assert_eq!(element_info_from(titled).name, "确定");
        // role/subrole/description 全空 → 不可读，fail-closed。
        let unreadable = AxNodeInfo {
            role: String::new(),
            title: String::new(),
            subrole: String::new(),
            description: String::new(),
            ..base
        };
        assert!(ensure_readable(&unreadable).is_err());
    }

    #[test]
    fn tree_line_falls_back_to_description_for_web_nodes() {
        let web_button = AxNodeInfo {
            role: "AXButton".to_string(),
            title: String::new(),
            subrole: String::new(),
            description: "Search the web".to_string(),
            x: 10,
            y: 20,
            width: 100,
            height: 24,
            disabled: false,
            focused: true,
            secure: false,
        };
        let line = format_tree_line(7, 2, &web_button);
        assert!(
            line.contains("[7] AXButton \"Search the web\" (10,20,100,24) focused"),
            "unexpected line: {line}"
        );
        // title 非空时仍显示 title，格式不变。
        let titled = AxNodeInfo {
            title: "Native".to_string(),
            ..web_button
        };
        let line = format_tree_line(8, 0, &titled);
        assert!(
            line.contains("[8] AXButton \"Native\" (10,20,100,24) focused"),
            "unexpected line: {line}"
        );
    }

    #[test]
    fn scroll_mapping_matches_enigo_sign_convention() {
        assert_eq!(map_scroll(ScrollDirection::Up, 3), (Axis::Vertical, -3));
        assert_eq!(map_scroll(ScrollDirection::Down, 3), (Axis::Vertical, 3));
        assert_eq!(map_scroll(ScrollDirection::Left, 2), (Axis::Horizontal, -2));
        assert_eq!(map_scroll(ScrollDirection::Right, 2), (Axis::Horizontal, 2));
        // u32::MAX 不溢出。
        assert_eq!(
            map_scroll(ScrollDirection::Down, u32::MAX),
            (Axis::Vertical, i32::MAX)
        );
    }

    #[test]
    fn cg_mouse_event_mapping_matches_core_graphics_constants() {
        // 原始值对照 CoreGraphics 的 CGEventType/CGMouseButton 枚举（SDK 头）。
        assert_eq!(cg_mouse_button(MouseButton::Left), 0);
        assert_eq!(cg_mouse_button(MouseButton::Right), 1);
        assert_eq!(cg_mouse_button(MouseButton::Middle), 2);
        assert_eq!(cg_mouse_event_types(MouseButton::Left), (1, 2, 6));
        assert_eq!(cg_mouse_event_types(MouseButton::Right), (3, 4, 7));
        assert_eq!(cg_mouse_event_types(MouseButton::Middle), (21, 22, 25));
    }

    #[test]
    fn drag_waypoints_interpolate_and_end_at_target() {
        let points = drag_waypoints((0, 0), (80, 40), 8);
        assert_eq!(points.len(), 8);
        assert_eq!(points.first().copied(), Some((10, 5)));
        assert_eq!(points.last().copied(), Some((80, 40)));
        // 反向拖拽与负坐标（多显示器原点）。
        let back = drag_waypoints((80, 40), (0, 0), 4);
        assert_eq!(back.last().copied(), Some((0, 0)));
        let negative = drag_waypoints((-100, -50), (0, 0), 2);
        assert_eq!(negative.first().copied(), Some((-50, -25)));
        // steps=0 防御：至少一步且落在终点。
        let single = drag_waypoints((1, 2), (3, 4), 0);
        assert_eq!(single, vec![(3, 4)]);
    }

    #[test]
    fn secure_text_detection_covers_role_and_subrole() {
        assert!(is_secure_text("AXTextField", "AXSecureTextField"));
        assert!(is_secure_text("AXSecureTextField", ""));
        assert!(!is_secure_text("AXTextField", ""));
        assert!(!is_secure_text("AXButton", ""));
    }

    #[test]
    fn sanitize_name_strips_quotes_and_caps_length() {
        assert_eq!(sanitize_name("a\"b\nc\td"), "a'b c d");
        let long = "x".repeat(MAX_NODE_NAME_CHARS + 50);
        assert_eq!(sanitize_name(&long).chars().count(), MAX_NODE_NAME_CHARS);
        assert_eq!(sanitize_name("  padded  "), "padded");
    }

    #[test]
    #[ignore = "live probe: needs Accessibility TCC grant and a real focused UI element"]
    fn live_focused_element_returns_element_with_role() {
        if !accessibility_granted() {
            return;
        }
        let mut backend = MacosComputerUseBackend::new();
        let focused = backend
            .focused_element()
            .expect("focused_element must not fail on a granted desktop");
        let info = focused.expect("a real desktop session always has a focused element");
        assert!(
            !info.role.is_empty(),
            "focused element role must not be empty"
        );
    }
}
