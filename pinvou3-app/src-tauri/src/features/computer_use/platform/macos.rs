//! macOS 后端：xcap（CGWindowListCreateImage）截屏 + enigo（CGEvent）输入注入
//! + ApplicationServices AXUIElement 无障碍树。
//!
//! 坐标约定（types.rs 契约的 macOS 具体化）：
//! - 输入坐标空间 = CGEvent 全局**点**（左上角原点，多显示器可为负）。
//! - `Capture.origin_x/y` 直接用 xcap 报告的显示器原点（macOS 上
//!   CGDisplayBounds 返回点），`Capture.input_scale_x/y = 1 / scale_factor`
//!   （xcap `scale_factor()` = 设备像素/点，Retina 2x → 0.5）；截图为物理像素。
//! - `cursor_position` 按 trait 契约返回全局**设备物理像素**（点 × 光标所在
//!   显示器的 scale），由 ScaleMap::device_to_input 乘 input_scale 还原为点。
//! - AX 返回的元素位置/尺寸同为屏幕点坐标，与输入坐标空间一致。
//!
//! 截屏 deprecation 说明：xcap 0.9.8 的 macOS 静态截图仍走
//! CGWindowListCreateImage——该 API 在 macOS 15 SDK 已 obsoleted（"Please use
//! ScreenCaptureKit instead"），且基于它的截屏在 Sequoia+ 会触发周期性
//! "继续允许录屏"系统确认。迁移 ScreenCaptureKit 是已记录的后续工作项。
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
//! CF 层说明：AX 属性名常量（kAXRoleAttribute 等）未被
//! objc2-application-services 0.3.2 的 header-translator 生成，且
//! CFDictionary/CFArray 的泛型默认参数是 crate 私有类型，外部无法构造——
//! 因此属性名用 NSString（toll-free bridged 即 CFString）、数组/字典/布尔/
//! 释放用少量 CoreFoundation extern "C"（同 detach.rs 的 FFI 先例），
//! AX 对象本体全部走 objc2-application-services 的类型化 API。

use std::ffi::c_void;
use std::fmt::Write as _;
use std::ptr::NonNull;
use std::thread::sleep;
use std::time::Duration;

use enigo::{Axis, Button, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};
use objc2::rc::Retained;
use objc2_application_services::{AXError, AXIsProcessTrusted, AXUIElement, AXValue, AXValueType};
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

/// Screen Recording 拒绝时的显式错误（授权后需重启应用才生效，错误里说明）。
const SCREEN_RECORDING_DENIED: &str = "screen_recording_denied: enable in System Settings → Privacy & Security → Screen Recording, then restart the app";
/// Accessibility 拒绝时的显式错误。
const ACCESSIBILITY_DENIED: &str = "accessibility_denied: enable in System Settings → Privacy & Security → Accessibility, then restart the app";

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreate(source: *const c_void) -> *const c_void;
    fn CGEventGetLocation(event: *const c_void) -> NSPoint;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
    fn CFGetTypeID(cf: *const c_void) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(boolean: *const c_void) -> bool;
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
    /// 外部构造不出该引用。
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
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
    let keys = [Retained::as_ptr(&key) as *const c_void];
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
        let _ = AXIsProcessTrustedWithOptions(options);
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
/// - `Char` 走 `Unicode`：enigo macOS 经当前键盘布局反查 keycode（ASCII
///   字母/数字/标点正确；布局外字符退化为 keycode 0 + 事件标志，打不出字，
///   但和弦键都是 ASCII，可接受）。
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
        Key::Char(c) => EK::Unicode(c),
    };
    Ok(mapped)
}

fn map_mouse_button(button: MouseButton) -> Button {
    match button {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
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
    cleaned.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Accessibility（AX）层
// ---------------------------------------------------------------------------

/// AX「Copy 规则」返回的 +1 CF 对象的 RAII 释放（构造时已判非空）。
struct CfObject(*const c_void);

impl CfObject {
    fn type_id(&self) -> usize {
        // SAFETY: self.0 非空且指向存活的 CF 对象（本 guard 持有 +1）；
        // CFGetTypeID 是纯类型查询。
        unsafe { CFGetTypeID(self.0) }
    }
}

impl Drop for CfObject {
    fn drop(&mut self) {
        // SAFETY: self.0 是 AX Copy 规则返回的 +1 对象（构造时判过非空），
        // 每个 CfObject 恰好释放一次，释放后不再使用。
        unsafe { CFRelease(self.0) };
    }
}

/// AX 属性名集合。kAX*Attribute 字符串常量未被 objc2-application-services
/// 0.3.2 生成，用 NSString 字面量（值稳定且文档化；toll-free bridged 即
/// CFString，可直接传给 AX API）。每次 ui_tree/element_at_point 调用构造一次，
/// 遍历内所有节点复用。
struct AxNames {
    role: Retained<NSString>,
    title: Retained<NSString>,
    subrole: Retained<NSString>,
    position: Retained<NSString>,
    size: Retained<NSString>,
    enabled: Retained<NSString>,
    focused: Retained<NSString>,
    children: Retained<NSString>,
    focused_application: Retained<NSString>,
    focused_window: Retained<NSString>,
}

impl AxNames {
    fn new() -> Self {
        Self {
            role: NSString::from_str("AXRole"),
            title: NSString::from_str("AXTitle"),
            subrole: NSString::from_str("AXSubrole"),
            position: NSString::from_str("AXPosition"),
            size: NSString::from_str("AXSize"),
            enabled: NSString::from_str("AXEnabled"),
            focused: NSString::from_str("AXFocused"),
            children: NSString::from_str("AXChildren"),
            focused_application: NSString::from_str("AXFocusedApplication"),
            focused_window: NSString::from_str("AXFocusedWindow"),
        }
    }
}

/// 读单个 AX 属性（Copy 规则 +1）。任何错误（属性不支持/无值/IPC 失败）一律
/// 返回 None——树遍历里单个属性缺失不拖垮整次抓取。
fn copy_attr(element: &AXUIElement, attribute: &NSString) -> Option<CfObject> {
    let mut raw = std::ptr::null();
    let Some(out) = NonNull::new(&mut raw) else {
        return None; // &mut 局部变量地址永不 null，此处仅避免 unwrap
    };
    // SAFETY: `out` 指向本函数栈上有效的 out 指针；attribute 是 NSString，
    // toll-free bridged 即 CFString；仅当返回 Success 且输出非空才使用输出值。
    let err = unsafe { element.copy_attribute_value(attribute.as_ref(), out) };
    if err != AXError::Success || raw.is_null() {
        return None;
    }
    Some(CfObject(raw as *const c_void))
}

fn ax_string(element: &AXUIElement, attribute: &NSString) -> Option<String> {
    let obj = copy_attr(element, attribute)?;
    // SAFETY: CFStringGetTypeID 是纯类型查询。
    if obj.type_id() != unsafe { CFStringGetTypeID() } {
        return None;
    }
    // SAFETY: 上面已校验对象是 CFString；NSString 与 CFString toll-free
    // bridged、布局相同；guard 持有 +1，借用不超过 guard 生命周期。
    let text = unsafe { &*(obj.0 as *const NSString) };
    Some(text.to_string())
}

fn ax_bool(element: &AXUIElement, attribute: &NSString) -> Option<bool> {
    let obj = copy_attr(element, attribute)?;
    // SAFETY: CFBooleanGetTypeID 是纯类型查询。
    if obj.type_id() != unsafe { CFBooleanGetTypeID() } {
        return None;
    }
    // SAFETY: 已校验 CFBoolean；纯读取。
    Some(unsafe { CFBooleanGetValue(obj.0) })
}

/// 读一个 AXUIElement 类型属性（Copy 规则 +1 guard）。
fn ax_ui_element(element: &AXUIElement, attribute: &NSString) -> Option<CfObject> {
    let obj = copy_attr(element, attribute)?;
    // SAFETY: AXUIElementGetTypeID 是纯类型查询。
    if obj.type_id() != unsafe { AXUIElementGetTypeID() } {
        return None;
    }
    Some(obj)
}

/// guard → &AXUIElement。调用方须先经 ax_ui_element/copy_element_at_position
/// 保证对象确为 AXUIElement。
fn as_ui_element(obj: &CfObject) -> &AXUIElement {
    // SAFETY: 调用方已校验对象类型为 AXUIElement；guard 持有 +1，
    // 借用不超过 guard 生命周期。
    unsafe { &*(obj.0 as *const AXUIElement) }
}

fn ax_point(element: &AXUIElement, attribute: &NSString) -> Option<NSPoint> {
    let obj = copy_attr(element, attribute)?;
    // SAFETY: AXValueGetTypeID 是纯类型查询。
    if obj.type_id() != unsafe { AXValueGetTypeID() } {
        return None;
    }
    // SAFETY: 已校验 AXValue；guard 持有 +1，借用不超过 guard 生命周期。
    let value = unsafe { &*(obj.0 as *const AXValue) };
    // SAFETY: point 是栈上有效 out buffer；value() 仅在类型匹配时写它并返回
    // true，此时读取才有效。
    unsafe {
        if value.r#type() != AXValueType::CGPoint {
            return None;
        }
        let mut point = NSPoint::default();
        if value.value(AXValueType::CGPoint, NonNull::from(&mut point).cast()) {
            Some(point)
        } else {
            None
        }
    }
}

fn ax_size(element: &AXUIElement, attribute: &NSString) -> Option<NSSize> {
    let obj = copy_attr(element, attribute)?;
    // SAFETY: AXValueGetTypeID 是纯类型查询。
    if obj.type_id() != unsafe { AXValueGetTypeID() } {
        return None;
    }
    // SAFETY: 已校验 AXValue；guard 持有 +1，借用不超过 guard 生命周期。
    let value = unsafe { &*(obj.0 as *const AXValue) };
    // SAFETY: size 是栈上有效 out buffer；value() 仅在类型匹配时写它并返回
    // true，此时读取才有效。
    unsafe {
        if value.r#type() != AXValueType::CGSize {
            return None;
        }
        let mut size = NSSize::default();
        if value.value(AXValueType::CGSize, NonNull::from(&mut size).cast()) {
            Some(size)
        } else {
            None
        }
    }
}

/// 读数组型属性（AXChildren/AXWindows）：返回 (数组 guard, 元素数)。
fn ax_array(element: &AXUIElement, attribute: &NSString) -> Option<(CfObject, usize)> {
    let obj = copy_attr(element, attribute)?;
    // SAFETY: CFArrayGetTypeID 是纯类型查询。
    if obj.type_id() != unsafe { CFArrayGetTypeID() } {
        return None;
    }
    // SAFETY: 已校验 CFArray；纯计数查询。
    let count = unsafe { CFArrayGetCount(obj.0) };
    if count <= 0 {
        return None;
    }
    Some((obj, count as usize))
}

/// 取数组第 index 个元素并借用为 &AXUIElement（Get 规则：不 retain，
/// 生命周期跟随数组 guard，故返回值携带 guard 的生命周期）。
fn array_element_at(array: &CfObject, index: usize) -> Option<&AXUIElement> {
    // SAFETY: array 已经 CFArrayGetTypeID 校验；index < count 由调用方保证
    // （count 来自同一数组的 CFArrayGetCount，数组不可变）；AXChildren/
    // AXWindows 契约元素为 AXUIElement；Get 规则返回的指针在数组存活期间有效。
    let raw = unsafe { CFArrayGetValueAtIndex(array.0, index as isize) };
    if raw.is_null() {
        return None;
    }
    // SAFETY: 见上；借用不超过数组 guard 生命周期。
    Some(unsafe { &*(raw as *const AXUIElement) })
}

/// 树节点的单行信息（位置/尺寸为屏幕点坐标，与输入坐标空间一致）。
struct AxNodeInfo {
    role: String,
    title: String,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    disabled: bool,
    focused: bool,
    secure: bool,
}

/// 读一个节点的全部展示字段。属性逐条读取：批读 API
/// AXUIElementCopyMultipleAttributeValues 需要构造 CFArray<CFString>，其泛型
/// 默认参数为 crate 私有类型，外部无法构造，故绑定层面不支持批读（性能代价由
/// 2s 消息超时 + 深度/节点预算收敛）。
fn read_node_info(element: &AXUIElement, names: &AxNames) -> AxNodeInfo {
    let role = ax_string(element, &names.role).unwrap_or_default();
    let title = ax_string(element, &names.title).unwrap_or_default();
    let subrole = ax_string(element, &names.subrole).unwrap_or_default();
    let point = ax_point(element, &names.position);
    let size = ax_size(element, &names.size);
    AxNodeInfo {
        secure: is_secure_text(&role, &subrole),
        role,
        title,
        x: point.map_or(0, |p| p.x.round() as i32),
        y: point.map_or(0, |p| p.y.round() as i32),
        width: size.map_or(0, |s| s.width.round().max(0.0) as i32),
        height: size.map_or(0, |s| s.height.round().max(0.0) as i32),
        disabled: ax_bool(element, &names.enabled) == Some(false),
        focused: ax_bool(element, &names.focused) == Some(true),
    }
}

/// 树序列化的节点预算与序号。
struct TreeWriter {
    next_index: u32,
    remaining: u32,
    truncated: bool,
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
    let mut line = String::new();
    let _ = write!(
        line,
        "{indent}[{index}] {role} \"{title}\" ({x},{y},{w},{h}){flags}",
        indent = "  ".repeat(depth as usize),
        role = info.role,
        title = sanitize_name(&info.title),
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
    if writer.remaining == 0 {
        writer.truncated = true;
        return;
    }
    writer.remaining -= 1;
    let index = writer.next_index;
    writer.next_index += 1;
    let info = read_node_info(element, names);
    let _ = writeln!(out, "{}", format_tree_line(index, depth, &info));
    if depth >= max_depth {
        return;
    }
    // 元素可能在遍历中途销毁：children 读取失败按"该分支结束"处理。
    let Some((children, count)) = ax_array(element, &names.children) else {
        return;
    };
    for i in 0..count {
        let Some(child) = array_element_at(&children, i) else {
            continue;
        };
        write_tree_node(child, depth + 1, max_depth, names, writer, out);
        if writer.remaining == 0 {
            writer.truncated = true;
            break;
        }
    }
}

/// 树的根：焦点应用的焦点窗口；应用无焦点窗口（隐藏/仅菜单栏）时退化为
/// 应用根（含菜单栏等，由深度/节点预算收敛）。
fn focused_window_root(system: &AXUIElement, names: &AxNames) -> Option<CfObject> {
    let app_guard = ax_ui_element(system, &names.focused_application)?;
    let app = as_ui_element(&app_guard);
    match ax_ui_element(app, &names.focused_window) {
        Some(window) => Some(window),
        None => Some(app_guard),
    }
}

pub(super) struct MacosComputerUseBackend {
    /// 懒构造：Accessibility 未授权时 Enigo::new 会直接失败，延迟到首个输入
    /// 操作再构造——截屏/光标等免授权能力在权限缺失时仍可用，也避免权限后补
    /// 授权后整个 backend 粘性失败（backend 工厂失败是粘性的）。
    enigo: Option<Enigo>,
}

impl MacosComputerUseBackend {
    fn new() -> Self {
        Self { enigo: None }
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
}

impl ComputerUseBackend for MacosComputerUseBackend {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            screenshot: true,
            input: true,
            ui_tree: true,
            notes: "macos: all input coordinates are CGEvent global points (screenshot px / \
                    backing scale factor); Screen Recording + Accessibility TCC grants are \
                    required and usually need an app restart after granting; macOS 26 (Tahoe) \
                    may filter synthetic modifier-key events aimed at global hotkey listeners"
                .to_string(),
        }
    }

    fn capture(&mut self) -> Result<Capture, ComputerUseError> {
        if !screen_recording_granted() {
            return Err(screen_recording_error());
        }
        let cursor = cursor_points().ok();
        let monitor = pick_monitor(cursor)?;
        let scale = monitor_scale(&monitor)?;
        // xcap macOS 显示器原点是 CGDisplayBounds = 点（输入坐标空间）。
        let origin_x = monitor
            .x()
            .map_err(|err| map_xcap_err("monitor origin", err))?;
        let origin_y = monitor
            .y()
            .map_err(|err| map_xcap_err("monitor origin", err))?;
        // 截图为设备物理像素（xcap 已做 BGRA→RGBA 行修复）。
        let image = monitor
            .capture_image()
            .map_err(|err| map_xcap_err("screen capture", err))?;
        let width = image.width();
        let height = image.height();
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
            input_scale_x: 1.0 / scale,
            input_scale_y: 1.0 / scale,
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
        // x/y 是 CGEvent 全局点坐标（工具层已按 input_scale 换算）。
        self.enigo()?
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|err| map_input_err("mouse move", err))
    }

    fn click(&mut self, button: MouseButton, count: u8) -> Result<(), ComputerUseError> {
        let enigo = self.enigo()?;
        let button = map_mouse_button(button);
        // 让先前的 move 落位再点击，避免点在旧光标位置。
        sleep(Duration::from_millis(CLICK_SETTLE_MS));
        for i in 0..count.max(1) {
            enigo
                .button(button, Direction::Click)
                .map_err(|err| map_input_err("mouse click", err))?;
            if i + 1 < count {
                sleep(Duration::from_millis(MULTI_CLICK_INTERVAL_MS));
            }
        }
        Ok(())
    }

    fn mouse_down(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        self.enigo()?
            .button(map_mouse_button(button), Direction::Press)
            .map_err(|err| map_input_err("mouse down", err))
    }

    fn mouse_up(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        self.enigo()?
            .button(map_mouse_button(button), Direction::Release)
            .map_err(|err| map_input_err("mouse up", err))
    }

    fn drag(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<(), ComputerUseError> {
        let enigo = self.enigo()?;
        enigo
            .move_mouse(from.0, from.1, Coordinate::Abs)
            .map_err(|err| map_input_err("drag move to start", err))?;
        sleep(Duration::from_millis(CLICK_SETTLE_MS));
        enigo
            .button(Button::Left, Direction::Press)
            .map_err(|err| map_input_err("drag press", err))?;
        sleep(Duration::from_millis(DRAG_PRESS_SETTLE_MS));
        let result = (|| {
            for (x, y) in drag_waypoints(from, to, DRAG_STEPS) {
                enigo
                    .move_mouse(x, y, Coordinate::Abs)
                    .map_err(|err| map_input_err("drag move", err))?;
                sleep(Duration::from_millis(DRAG_STEP_DELAY_MS));
            }
            Ok(())
        })();
        // 无论路径移动是否出错都必须释放按键，避免鼠标卡在按下状态。
        let release = enigo
            .button(Button::Left, Direction::Release)
            .map_err(|err| map_input_err("drag release", err));
        result.and(release)
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
        let Some(root_guard) = focused_window_root(&system, &names) else {
            return Err(ComputerUseError::unavailable(
                "no focused application is available for ui_tree",
            ));
        };
        let root = as_ui_element(&root_guard);
        let mut out = String::new();
        let mut writer = TreeWriter {
            next_index: 0,
            remaining: max_nodes,
            truncated: false,
        };
        write_tree_node(root, 0, max_depth, &names, &mut writer, &mut out);
        if writer.truncated {
            let _ = writeln!(
                out,
                "... (truncated at {} nodes; pass a larger max_nodes to see more)",
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
        if raw.is_null() {
            return Ok(None);
        }
        let guard = CfObject(raw as *const c_void);
        let info = read_node_info(as_ui_element(&guard), &names);
        Ok(Some(ElementInfo {
            role: info.role,
            name: sanitize_name(&info.title),
            x: info.x,
            y: info.y,
            width: info.width,
            height: info.height,
            secure: info.secure,
        }))
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
        assert_eq!(map_key(Key::Char('中')).ok(), Some(EK::Unicode('中')));
        assert!(map_key(Key::Function(0)).is_err());
        assert!(map_key(Key::Function(13)).is_err());
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
}
