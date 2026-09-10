//! Windows 后端：xcap（GDI BitBlt）截屏 + enigo（SendInput）输入注入
//! + uiautomation（UI Automation）无障碍树。
//!
//! 坐标约定：进程经 tao 置为 Per-Monitor-V2 DPI aware（`new()` 时读取线程
//! DPI 感知核对，见 `capabilities` notes），截图像素、UIA BoundingRectangle、
//! SendInput 绝对坐标同为**设备物理像素**，故 `Capture.input_scale_x/y = 1.0`；
//! 多显示器虚拟桌面原点可为负，原样透传。
//!
//! **绝对鼠标移动不经 enigo**：enigo 0.6.1 的 `Coordinate::Abs` 只按主屏
//! （`SM_CXSCREEN`）归一化且不带 `MOUSEEVENTF_VIRTUALDESK`，光标在副屏时坐标
//! 会被折算到主屏。本模块用 windows-sys 直接调用 `SendInput`，以整个虚拟桌面
//! 归一化（`move_mouse_abs`）；按钮按下/释放与键盘仍走 enigo——其点击事件
//! 相对当前光标位置（dx=dy=0、无 ABSOLUTE 标志），绝对移动先行落位即正确。
//!
//! 线程与 COM：所有对象于 backend worker 线程构造、使用、析构（见
//! `super::super::backend::BackendHandle`），不跨线程移动。`Enigo` 持有在结构内；
//! `uiautomation::UIAutomation` 因 windows-rs COM 接口为 `!Send` 而不入结构——
//! `new()` 在该线程以 MTA 初始化 COM 一次，此后每次 UIA 调用现建现析客户端。
//! 全部 UIA 调用都留在这条无窗口的 worker 线程上，避免与 WebView UI 线程死锁。
//!
//! 已知限制（UIPI / 安全桌面 / UIA 超时）：
//! - 本进程以 medium IL 运行且不带 `uiAccess`，对**以管理员身份运行**的目标窗口，
//!   SendInput 注入会被 UIPI 丢弃、UIA 读取返回 access denied。两类失败都可
//!   探测并映射为显式 `unavailable`：UIA 的拒绝访问（HRESULT 0x80070005）与
//!   enigo 报告的注入数不足（见 `map_input_err`）。
//! - 安全桌面（锁屏 / UAC 同意框）与显示器休眠期间截图为全黑、注入与 UIA 全部
//!   失效：全黑帧会作为显式 `unavailable` 错误返回，唤醒后自动恢复。
//! - UIA 读取没有事务超时：目标应用挂死可阻塞调用直至系统默认 COM 超时
//!   （会话层仍有 `BackendHandle` 的调用超时兜底，worker 线程在 OS 调用里
//!   期间该会话后续请求排队）。
//! - `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` 与 DRM 保护内容在截图中
//!   呈现为黑块，属系统预期行为。

use std::fmt::Write as _;
use std::thread::sleep;
use std::time::Duration;

use enigo::{Axis, Button, Direction, Enigo, Keyboard, Mouse, Settings};
use uiautomation::UIAutomation;
use uiautomation::types::{ControlType, Point, TreeScope, UIProperty};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, HKL, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput, VkKeyScanExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WHEEL_DELTA,
};
use xcap::Monitor;

use super::super::backend::ComputerUseBackend;
use super::super::types::{
    Capabilities, Capture, ComputerUseError, ElementInfo, Key, MouseButton, ScrollDirection,
    UiTreeOptions,
};

/// 点击前等待先前 move 落位（目标进程消费鼠标移动事件）。
const CLICK_SETTLE_MS: u64 = 30;
/// 双击/三击中相邻两次点击的间隔（远小于系统双击时限 500ms）。
const MULTI_CLICK_INTERVAL_MS: u64 = 40;
/// 拖拽按下后、开始移动前的停顿，给目标窗口进入拖拽识别状态的时间。
const DRAG_PRESS_SETTLE_MS: u64 = 60;
/// 拖拽路径插值步数（部分应用只有收到足够多 motion 事件才识别拖拽）。
const DRAG_STEPS: usize = 8;
/// 拖拽相邻两点的间隔。
const DRAG_STEP_DELAY_MS: u64 = 12;
/// 按下全部键与逆序释放之间的停顿，提高修饰键和弦（alt+Tab 等）命中率。
const CHORD_HOLD_MS: u64 = 20;

/// `ui_tree` 默认/上限：深度与节点数（防止巨型树拖垮 worker 线程与文本预算）。
const DEFAULT_TREE_MAX_DEPTH: u32 = 8;
const DEFAULT_TREE_MAX_NODES: u32 = 400;
const MAX_TREE_DEPTH: u32 = 24;
const MAX_TREE_NODES: u32 = 2000;
/// 单行 name 的字符上限。
const MAX_NODE_NAME_CHARS: usize = 80;
/// 焦点元素向上爬升查找所属窗口的防御性上限。
const MAX_ANCESTOR_CLIMB: u32 = 16;

/// 滚轮格数上限：enigo 在 Windows 上将格数乘 `WHEEL_DELTA`(120)（debug 下溢出
/// 即 panic），后端侧再钳一次防上游传大数溢出。
const MAX_SCROLL_CLICKS: u32 = (i32::MAX / WHEEL_DELTA as i32) as u32;

/// `VkKeyScanExW` 返回值高字节的修饰位（Win32 文档）：bit0=Shift，bit1=Ctrl，
/// bit2=Alt（AltGr = Ctrl+Alt）。高字节非零表示产生该字符需要修饰键。
const VKSHIFT_SHIFT: i16 = 0x01;
const VKSHIFT_CTRL: i16 = 0x02;
const VKSHIFT_ALT: i16 = 0x04;

/// `GetAwarenessFromDpiAwarenessContext` 的返回值：Per-Monitor 感知。
/// （windows-sys `Win32_UI_HiDpi` 的 `DPI_AWARENESS_PER_MONITOR_AWARE`。）
const DPI_AWARENESS_PER_MONITOR_AWARE: i32 = 2;
/// `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2` 伪句柄（-4）。
const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;

// windows-sys 0.61 的这三条 user32 入口位于 `Win32_UI_HiDpi` feature 之后，而
// Cargo.toml 未启用该 feature；为不新增依赖 feature 在此手声明（签名与
// windows-sys HiDpi 模块逐字一致）。最低要求 Windows 10 1607，在 Tauri 2 /
// WebView2 的 Win10 基线之上。
#[allow(non_snake_case)]
unsafe extern "system" {
    fn GetThreadDpiAwarenessContext() -> *mut core::ffi::c_void;
    fn GetAwarenessFromDpiAwarenessContext(value: *mut core::ffi::c_void) -> i32;
    fn AreDpiAwarenessContextsEqual(a: *mut core::ffi::c_void, b: *mut core::ffi::c_void) -> i32;
}

/// Win32 `E_ACCESSDENIED`（0x80070005）：UIA 跨完整性级别读取被拒的典型信号。
const E_ACCESSDENIED: i32 = -2147024891;

/// 线程 DPI 感知是否为 Per-Monitor-V2：感知级别为 PER_MONITOR_AWARE(2) 且
/// 线程上下文确为 PMv2 伪句柄（Per-Monitor v1 满足前者不满足后者）。纯函数，
/// 便于单测；`context_is_pmv2` 即 `AreDpiAwarenessContextsEqual(ctx, PMv2)`。
fn thread_is_pmv2(awareness: i32, context_is_pmv2: bool) -> bool {
    awareness == DPI_AWARENESS_PER_MONITOR_AWARE && context_is_pmv2
}

/// 读取当前线程的 DPI 感知是否为 PMv2（见 `thread_is_pmv2`）。
fn thread_dpi_is_pmv2() -> bool {
    unsafe {
        let ctx = GetThreadDpiAwarenessContext();
        if ctx.is_null() {
            return false;
        }
        let awareness = GetAwarenessFromDpiAwarenessContext(ctx);
        let is_pmv2_context = AreDpiAwarenessContextsEqual(
            ctx,
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 as *mut core::ffi::c_void,
        ) != 0;
        thread_is_pmv2(awareness, is_pmv2_context)
    }
}

/// 单字符键的注入计划（据 `VkKeyScanExW` 布局查询结果）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharInjection {
    /// 布局直发：无需修饰键（纯 ASCII 字母/数字/无需 Shift 的标点），走既有
    /// enigo 虚拟键路径（shift state 高字节为 0，enigo 的 VK 换算正确）。
    Direct,
    /// 需要修饰键（Shift/AltGr 等）才能产生该字符。enigo 会把 `VkKeyScanExW`
    /// 的 shift state 高字节一并当作 VK 发送（SendInput 成功但目标收不到该
    /// 字符），故不能再走 VK 和弦。
    NeedsModifier,
    /// 布局无此字符（如英文布局下的 CJK）；enigo 对映射失败自动回退
    /// KEYEVENTF_UNICODE 事件，维持现路径即可。
    NotInLayout,
}

/// `VkKeyScanExW` 返回值 → 注入计划（纯函数，便于单测）。低字节为 VK，
/// 高字节为修饰位（`VKSHIFT_*`）；返回负值表示布局无法产生该字符。
fn char_injection_from_scan(scan: i16) -> CharInjection {
    if scan < 0 {
        return CharInjection::NotInLayout;
    }
    let shift_state = (scan >> 8) & (VKSHIFT_SHIFT | VKSHIFT_CTRL | VKSHIFT_ALT);
    if shift_state != 0 {
        CharInjection::NeedsModifier
    } else {
        CharInjection::Direct
    }
}

/// 查询字符在目标线程键盘布局中的注入计划。布局取前台窗口线程的（与 enigo
/// 的换算目标一致），取不到前台窗口时回退当前线程布局。
fn char_injection(c: char) -> CharInjection {
    let mut utf16 = [0u16; 2];
    if c.encode_utf16(&mut utf16).len() != 1 {
        // BMP 外字符（代理对）无法映射为单个 VK。
        return CharInjection::NotInLayout;
    }
    let layout: HKL = unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            GetKeyboardLayout(0)
        } else {
            GetKeyboardLayout(GetWindowThreadProcessId(hwnd, std::ptr::null_mut()))
        }
    };
    char_injection_from_scan(unsafe { VkKeyScanExW(utf16[0], layout) })
}

/// 屏幕/虚拟桌面单轴坐标 → SendInput 绝对归一化坐标（0..=65535）。
/// `origin`/`extent` 为归一化域的原点与尺寸（虚拟桌面原点可为负）。
/// 纯函数，便于单测。
fn normalize_abs_axis(value: i32, origin: i32, extent: i32) -> i32 {
    if extent <= 1 {
        // 退化（单像素/无效）：0 与 65535 等价，取 0。
        return 0;
    }
    let delta = i64::from(value - origin);
    let den = i64::from(extent - 1);
    // 四舍五入（半值向上）；负值（域外）随后被钳掉。
    let scaled = (delta * i64::from(65535) + den / 2) / den;
    scaled.clamp(0, i64::from(65535)) as i32
}

/// 读取虚拟桌面度量：(原点 x, 原点 y, 宽, 高)，覆盖全部显示器（原点可为负）。
fn virtual_desktop_metrics() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

/// SendInput 注入结果校验：注入数与请求数不符（典型原因是目标窗口提权、
/// UIPI 拦截注入）映射为 `unavailable`，而非泛化 failed——上层可据此提示。
fn check_injection(context: &str, requested: u32, sent: u32) -> Result<(), ComputerUseError> {
    if sent == requested {
        return Ok(());
    }
    Err(ComputerUseError::unavailable(format!(
        "{context}: SendInput injected {sent}/{requested} events — the input was likely blocked \
         by UIPI because the target window is elevated (run as administrator); \
         run Pinvou elevated or use an unelevated target window"
    )))
}

fn map_xcap_err(context: &str, err: xcap::XCapError) -> ComputerUseError {
    ComputerUseError::failed(format!("{context}: {err}"))
}

fn map_input_err(context: &str, err: enigo::InputError) -> ComputerUseError {
    // enigo 在 SendInput 注入数不足时返回 Simulate("...blocked by UIPI")——
    // 典型场景是目标窗口以管理员运行、UIPI 静默丢弃注入事件。映射为
    // `unavailable`（可探测、带提示），不再是泛化 failed。
    if let enigo::InputError::Simulate(msg) = &err {
        if msg.contains("UIPI") {
            return ComputerUseError::unavailable(format!(
                "{context}: input was blocked by UIPI — the target window is likely elevated \
                 (run as administrator); run Pinvou elevated or use an unelevated target window"
            ));
        }
    }
    ComputerUseError::failed(format!("{context}: {err}"))
}

fn map_uia_err(context: &str, err: uiautomation::Error) -> ComputerUseError {
    if err.code() == E_ACCESSDENIED {
        ComputerUseError::unavailable(format!(
            "{context}: access denied reading UI Automation data — the target window is likely \
             elevated (run as administrator); UIPI blocks cross-integrity-level access"
        ))
    } else {
        ComputerUseError::failed(format!("{context}: {err}"))
    }
}

/// `map_key` 的结果。普通键给出 enigo 按键；需修饰键的字符（如 '+'）在
/// 单独成和弦时要求改走 Unicode 直注通道。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MappedKey {
    Key(enigo::Key),
    /// 该字符需修饰键才能产生且和弦只有它自己：用 `Keyboard::text` 经
    /// KEYEVENTF_UNICODE 直注（不依赖布局与 IME）。注意该通道立即
    /// down+up，`hold_key` 的按住语义不适用。
    UnicodeOnly(char),
}

/// 据 `Key::Char` 的布局注入计划与和弦长度给出映射结果（纯函数，便于单测）。
/// 需修饰键的字符（如 '+'，enigo 会把 `VkKeyScanExW` 的 shift state 高字节
/// 一并当作 VK 发送、目标收不到该字符）：
/// - 和弦除该字符外还有其它键 → 显式报错；
/// - 和弦仅该字符 → 返回 [`MappedKey::UnicodeOnly`]，由调用方走 Unicode 通道。
fn map_char_with_plan(
    c: char,
    plan: CharInjection,
    chord_len: usize,
) -> Result<MappedKey, ComputerUseError> {
    match plan {
        // Direct：enigo 的 VK 换算正确；NotInLayout：enigo 自动回退 Unicode 事件。
        CharInjection::Direct | CharInjection::NotInLayout => {
            Ok(MappedKey::Key(enigo::Key::Unicode(c)))
        }
        CharInjection::NeedsModifier => {
            if chord_len > 1 {
                Err(ComputerUseError::failed(format!(
                    "character '{c}' requires shift and cannot be combined with modifier chords"
                )))
            } else {
                Ok(MappedKey::UnicodeOnly(c))
            }
        }
    }
}

/// 本 crate `Key` → enigo 按键。`Char` 先经 `VkKeyScanExW` 查布局注入计划
/// （见 [`map_char_with_plan`]）；命名键直映。布局取前台窗口线程的，与
/// enigo 的换算目标一致。
fn map_key(key: Key, chord_len: usize) -> Result<MappedKey, ComputerUseError> {
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
        Key::Insert => EK::Insert,
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
        Key::Char(c) => return map_char_with_plan(c, char_injection(c), chord_len),
    };
    Ok(MappedKey::Key(mapped))
}

fn map_mouse_button(button: MouseButton) -> Button {
    match button {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
    }
}

/// 滚轮方向 → enigo（轴, 带符号格数）。enigo 约定：Vertical 正值向下/负值向上，
/// Horizontal 正值向右/负值向左。格数在后端侧再钳一次：enigo 内部会乘
/// `WHEEL_DELTA`(120)，不钳位时大格数会 i32 溢出（debug panic / release 回绕）。
fn map_scroll(direction: ScrollDirection, clicks: u32) -> (Axis, i32) {
    let clicks = i32::try_from(clicks.min(MAX_SCROLL_CLICKS)).unwrap_or(i32::MAX);
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

/// 树序列化的节点预算与序号。
struct TreeWriter {
    next_index: u32,
    remaining: u32,
    truncated: bool,
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

/// 单行格式：`[i] role "name" (x,y,w,h) flags`，flags 省略时为无。
fn format_tree_line(
    index: u32,
    depth: u32,
    element: &uiautomation::UIElement,
) -> Result<String, ComputerUseError> {
    let control_type = element
        .get_cached_control_type()
        .map_err(|e| map_uia_err("ui_tree control type", e))?;
    let name = sanitize_name(
        &element
            .get_cached_name()
            .map_err(|e| map_uia_err("ui_tree name", e))?,
    );
    let rect = element
        .get_cached_bounding_rectangle()
        .map_err(|e| map_uia_err("ui_tree bounds", e))?;
    let mut flags = String::new();
    if !element.is_cached_enabled().unwrap_or(true) {
        flags.push_str(" disabled");
    }
    if element.has_cached_keyboard_focus().unwrap_or(false) {
        flags.push_str(" focused");
    }
    if element.is_cached_offscreen().unwrap_or(false) {
        flags.push_str(" offscreen");
    }
    if element.is_cached_password().unwrap_or(false) {
        flags.push_str(" password");
    }
    let mut line = String::new();
    let _ = write!(
        line,
        "{indent}[{index}] {control_type:?} \"{name}\" ({x},{y},{w},{h}){flags}",
        indent = "  ".repeat(depth as usize),
        x = rect.get_left(),
        y = rect.get_top(),
        w = (rect.get_right() - rect.get_left()).max(0),
        h = (rect.get_bottom() - rect.get_top()).max(0),
    );
    Ok(line)
}

/// 序列化一个子树：逐节点以 Children-scope 缓存请求抓取「该节点 + 直接子级」，
/// 节点预算（`writer.remaining`）约束跨进程往返次数——整棵树的抓取规模由此有界。
/// 元素可能在排队期间销毁：单节点抓取失败按“该分支结束”处理，不拖垮整次抓取。
fn write_tree_node(
    element: &uiautomation::UIElement,
    cache: &uiautomation::core::UICacheRequest,
    depth: u32,
    max_depth: u32,
    writer: &mut TreeWriter,
    out: &mut String,
) -> Result<(), ComputerUseError> {
    if writer.remaining == 0 {
        writer.truncated = true;
        return Ok(());
    }
    // 抓取失败不消耗预算：销毁的元素不应挤占可见节点的名额。
    let Ok(cached) = element.build_updated_cache(cache) else {
        return Ok(());
    };
    writer.remaining -= 1;
    let index = writer.next_index;
    writer.next_index += 1;
    let line = format_tree_line(index, depth, &cached)?;
    let _ = writeln!(out, "{line}");
    if depth >= max_depth {
        return Ok(());
    }
    // 子级已在同一次缓存请求中抓回；枚举失败同样按“该分支结束”处理。
    let Ok(children) = cached.get_cached_children() else {
        return Ok(());
    };
    for child in &children {
        write_tree_node(child, cache, depth + 1, max_depth, writer, out)?;
        if writer.remaining == 0 {
            writer.truncated = true;
            break;
        }
    }
    Ok(())
}

pub(super) struct WindowsComputerUseBackend {
    enigo: Enigo,
    /// `new()` 时读一次的线程 DPI 感知结论：非 Per-Monitor-V2 时在
    /// `capabilities().notes` 声明坐标假设存疑（不做硬失败）。
    dpi_pmv2: bool,
    // 注意：不持有 uiautomation::UIAutomation——windows-rs 的 COM 接口类型是
    // !Send（IUIAutomation 内含 NonNull），而 ComputerUseBackend 要求 Send。
    // 改为在 new() 里于 worker 线程初始化 COM MTA 一次，之后每次 UIA 调用经
    // uia_client() 现建现析客户端（CoCreateInstance 开销对每回合一次的调用可忽略）。
}

impl WindowsComputerUseBackend {
    fn new() -> Result<Self, ComputerUseError> {
        let enigo = Enigo::new(&Settings::default()).map_err(|err| {
            ComputerUseError::unavailable(format!("cannot initialize SendInput backend: {err}"))
        })?;
        // 在 worker 线程上以 MTA 初始化 COM（UIAutomation::new 内含 CoInitializeEx），
        // 随后丢弃客户端；公寓存续至线程结束（该 crate 不 CoUninitialize）。
        let _com_mta_init = UIAutomation::new().map_err(|err| {
            ComputerUseError::unavailable(format!(
                "cannot initialize UI Automation (COM MTA): {err}"
            ))
        })?;
        // 坐标契约的前提是进程 Per-Monitor-V2 DPI aware（tao 置位）。此处读一次
        // 线程感知核对，非 PMv2 时仅在 capabilities notes 声明（不做硬失败）。
        let dpi_pmv2 = thread_dpi_is_pmv2();
        Ok(Self { enigo, dpi_pmv2 })
    }

    /// 现建一个 UIA 客户端。要求 COM 已在当前线程初始化（new() 保证）。
    fn uia_client(&self) -> Result<UIAutomation, ComputerUseError> {
        UIAutomation::new_direct().map_err(|err| {
            ComputerUseError::unavailable(format!("cannot create UI Automation client: {err}"))
        })
    }

    /// 截屏目标显示器：光标所在屏，取不到时回退主屏，再退化为第一块屏。
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

    /// 属性缓存请求：把逐节点多次跨进程 COM 往返合并成一次批量读取。
    /// `scope` 给出缓存抓取的树范围（`ui_tree` 逐节点下潜用 `Children`，
    /// 单元素读取用 `Element`）；`Element` 之外的 scope 会施加控件视图过滤，
    /// 与 UIA 标准查看口径一致。
    fn property_cache(
        uia: &UIAutomation,
        scope: TreeScope,
    ) -> Result<uiautomation::core::UICacheRequest, ComputerUseError> {
        let cache = uia
            .create_cache_request()
            .map_err(|e| map_uia_err("ui_tree cache request", e))?;
        for property in [
            UIProperty::Name,
            UIProperty::ControlType,
            UIProperty::BoundingRectangle,
            UIProperty::IsEnabled,
            UIProperty::HasKeyboardFocus,
            UIProperty::IsOffscreen,
            UIProperty::IsPassword,
        ] {
            cache
                .add_property(property)
                .map_err(|e| map_uia_err("ui_tree cache request", e))?;
        }
        cache
            .set_tree_scope(scope)
            .map_err(|e| map_uia_err("ui_tree cache scope", e))?;
        if scope != TreeScope::Element {
            let control_view = uia
                .get_control_view_condition()
                .map_err(|e| map_uia_err("ui_tree cache filter", e))?;
            cache
                .set_tree_filter(control_view)
                .map_err(|e| map_uia_err("ui_tree cache filter", e))?;
        }
        Ok(cache)
    }

    /// 树的根：焦点元素所属的顶层窗口。焦点取不到时显式失败（fail-closed），
    /// **不回退桌面根**——桌面全树可达数万节点，回退会让逐层预算失去意义。
    fn focused_window_root(
        uia: &UIAutomation,
    ) -> Result<uiautomation::UIElement, ComputerUseError> {
        let focused = uia
            .get_focused_element()
            .map_err(|e| map_uia_err("ui_tree (no focused element)", e))?;
        let walker = match uia.get_raw_view_walker() {
            Ok(walker) => walker,
            Err(_) => return Ok(focused),
        };
        let mut current = focused;
        for _ in 0..MAX_ANCESTOR_CLIMB {
            if current.get_control_type() == Ok(ControlType::Window) {
                return Ok(current);
            }
            match walker.get_parent(&current) {
                Ok(parent) => {
                    // 防御父链自环（桌面根的父是其自身或直接失败）。
                    if uia.compare_elements(&parent, &current).unwrap_or(false) {
                        return Ok(current);
                    }
                    current = parent;
                }
                Err(_) => return Ok(current),
            }
        }
        Ok(current)
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

    /// 按下全部键（出错时回滚已按下的），停顿，再逆序释放。
    fn chord(&mut self, keys: &[Key], hold: Duration) -> Result<(), ComputerUseError> {
        let mut pressed: Vec<enigo::Key> = Vec::with_capacity(keys.len());
        for key in keys {
            let mapped = match map_key(*key, keys.len()) {
                Ok(MappedKey::Key(mapped)) => mapped,
                // 仅当和弦只有这一个字符时才可能出现：改走 Unicode 直注
                // （与 type_text 同通道，KEYEVENTF_UNICODE 不依赖布局）。
                Ok(MappedKey::UnicodeOnly(c)) => {
                    return self
                        .enigo
                        .text(&c.to_string())
                        .map_err(|err| map_input_err("key chord", err));
                }
                Err(err) => {
                    // 和弦解析中途失败：先释放已按下的修饰键，避免卡键。
                    let _ = Self::release_reverse(&pressed, &mut self.enigo);
                    return Err(err);
                }
            };
            if let Err(err) = self.enigo.key(mapped, Direction::Press) {
                let _ = Self::release_reverse(&pressed, &mut self.enigo);
                return Err(map_input_err("key press", err));
            }
            pressed.push(mapped);
        }
        sleep(hold);
        Self::release_reverse(&pressed, &mut self.enigo)
            .map_err(|err| map_input_err("key release", err))
    }

    /// 多显示器正确的绝对移动：windows-sys 直接调 `SendInput`，以整个虚拟桌面
    /// （`MOUSEEVENTF_VIRTUALDESK`，原点可为负）归一化。enigo 0.6.1 的
    /// `Coordinate::Abs` 只按主屏（`SM_CXSCREEN`）归一化，光标在副屏时点击会
    /// 落到主屏，故绝对移动必须自实现；按钮事件相对当前光标位置，先落位即可。
    fn move_mouse_abs(&mut self, x: i32, y: i32) -> Result<(), ComputerUseError> {
        let (vx, vy, vw, vh) = virtual_desktop_metrics();
        // 联合体以字段字面量构造（安全），不必 zeroed 后回填。
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: normalize_abs_axis(x, vx, vw),
                    dy: normalize_abs_axis(y, vy, vh),
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let sent = unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
        check_injection("mouse move", 1, sent)
    }
}

impl ComputerUseBackend for WindowsComputerUseBackend {
    fn capabilities(&self) -> Capabilities {
        let mut notes = "windows: physical-pixel coordinates on the full virtual desktop \
                         (multi-monitor safe); elevated (run as administrator) target windows \
                         are blocked by UIPI — detection is best-effort (injection shortfalls \
                         and UIA access-denied are reported as unavailable), actions should be \
                         verified with a follow-up screenshot; display sleep / secure desktop \
                         (lock screen / UAC) surfaces as black-frame capture errors; UIA reads \
                         have no transaction timeout — a hung target app can stall the call up \
                         to the system default COM timeout (session-level call timeout still \
                         applies)"
            .to_string();
        if !self.dpi_pmv2 {
            notes.push_str(
                "; DPI: thread is not per-monitor-v2 aware — the physical-pixel coordinate \
                 assumption may not hold and coordinates may be virtualized",
            );
        }
        Capabilities {
            screenshot: true,
            input: true,
            ui_tree: true,
            notes,
        }
    }

    fn capture(&mut self) -> Result<Capture, ComputerUseError> {
        let cursor = self.cursor_position().ok();
        let monitor = Self::pick_monitor(cursor)?;
        let origin_x = monitor
            .x()
            .map_err(|err| map_xcap_err("monitor origin", err))?;
        let origin_y = monitor
            .y()
            .map_err(|err| map_xcap_err("monitor origin", err))?;
        let image = monitor
            .capture_image()
            .map_err(|err| map_xcap_err("screen capture", err))?;
        let width = image.width();
        let height = image.height();
        let mut rgba = image.into_raw();
        // xcap 的 GDI 路径在 Win8+ 不修复 alpha（可能整体为 0，下游 PNG 会全透明）；
        // 单趟遍历顺带把 alpha 置为不透明并做全黑帧检测（安全桌面/受保护内容）。
        let mut all_black = true;
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
            if all_black && (pixel[0] != 0 || pixel[1] != 0 || pixel[2] != 0) {
                all_black = false;
            }
        }
        if all_black {
            return Err(ComputerUseError::unavailable(
                "captured frame is entirely black — likely the secure desktop (lock screen or \
                 UAC prompt) or capture-protected content; capture will resume on the normal desktop",
            ));
        }
        Ok(Capture {
            rgba,
            width,
            height,
            origin_x,
            origin_y,
            input_scale_x: 1.0,
            input_scale_y: 1.0,
        })
    }

    fn cursor_position(&mut self) -> Result<(i32, i32), ComputerUseError> {
        self.enigo
            .location()
            .map_err(|err| map_input_err("cursor position", err))
    }

    fn move_to(&mut self, x: i32, y: i32) -> Result<(), ComputerUseError> {
        // 绝对移动走自有 SendInput 路径（全虚拟桌面归一化，多显示器安全）。
        self.move_mouse_abs(x, y)
    }

    fn click(&mut self, button: MouseButton, count: u8) -> Result<(), ComputerUseError> {
        let button = map_mouse_button(button);
        // 让先前的 move 落位再点击，避免点在旧光标位置。
        sleep(Duration::from_millis(CLICK_SETTLE_MS));
        for i in 0..count.max(1) {
            self.enigo
                .button(button, Direction::Click)
                .map_err(|err| map_input_err("mouse click", err))?;
            if i + 1 < count {
                sleep(Duration::from_millis(MULTI_CLICK_INTERVAL_MS));
            }
        }
        Ok(())
    }

    fn mouse_down(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        self.enigo
            .button(map_mouse_button(button), Direction::Press)
            .map_err(|err| map_input_err("mouse down", err))
    }

    fn mouse_up(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        self.enigo
            .button(map_mouse_button(button), Direction::Release)
            .map_err(|err| map_input_err("mouse up", err))
    }

    fn drag(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<(), ComputerUseError> {
        self.move_mouse_abs(from.0, from.1)?;
        sleep(Duration::from_millis(CLICK_SETTLE_MS));
        self.enigo
            .button(Button::Left, Direction::Press)
            .map_err(|err| map_input_err("drag press", err))?;
        sleep(Duration::from_millis(DRAG_PRESS_SETTLE_MS));
        let result = (|| {
            for (x, y) in drag_waypoints(from, to, DRAG_STEPS) {
                self.move_mouse_abs(x, y)?;
                sleep(Duration::from_millis(DRAG_STEP_DELAY_MS));
            }
            Ok(())
        })();
        // 无论路径移动是否出错都必须释放按键，避免鼠标卡在按下状态。
        let release = self
            .enigo
            .button(Button::Left, Direction::Release)
            .map_err(|err| map_input_err("drag release", err));
        result.and(release)
    }

    fn scroll(&mut self, direction: ScrollDirection, clicks: u32) -> Result<(), ComputerUseError> {
        let (axis, length) = map_scroll(direction, clicks);
        self.enigo
            .scroll(length, axis)
            .map_err(|err| map_input_err("scroll", err))
    }

    fn type_text(&mut self, text: &str) -> Result<(), ComputerUseError> {
        // Unicode 直注（KEYEVENTF_UNICODE），绕过 IME——中文直接落进焦点字段。
        self.enigo
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
        let max_depth = opts
            .max_depth
            .unwrap_or(DEFAULT_TREE_MAX_DEPTH)
            .min(MAX_TREE_DEPTH);
        let max_nodes = opts
            .max_nodes
            .unwrap_or(DEFAULT_TREE_MAX_NODES)
            .min(MAX_TREE_NODES);
        let uia = self.uia_client()?;
        let root = Self::focused_window_root(&uia)?;
        // 逐节点下潜：每个节点以 Children-scope 缓存请求抓取「自身 + 直接子级」，
        // 跨进程往返次数由 max_nodes 预算约束。此前是一次 Subtree 抓整树——
        // 抓取规模不受预算约束（桌面级回退可数万节点），MAX_TREE_NODES 只管
        // 序列化。序列化层的 MAX_TREE_NODES/MAX_TREE_DEPTH 语义保持不变。
        let cache = Self::property_cache(&uia, TreeScope::Children)?;
        let mut out = String::new();
        let mut writer = TreeWriter {
            next_index: 0,
            remaining: max_nodes,
            truncated: false,
        };
        write_tree_node(&root, &cache, 0, max_depth, &mut writer, &mut out)?;
        if writer.truncated {
            let _ = writeln!(
                out,
                "... (truncated at {} nodes; pass a larger max_nodes to see more)",
                writer.next_index
            );
        }
        Ok(out)
    }

    fn focused_element(&mut self) -> Result<Option<ElementInfo>, ComputerUseError> {
        let uia = self.uia_client()?;
        let cache = Self::property_cache(&uia, TreeScope::Element)?;
        let element = uia
            .get_focused_element_build_cache(&cache)
            .map_err(|e| map_uia_err("focused_element", e))?;
        // fail-closed：任何属性读取失败都报错，不产出残缺的 ElementInfo。
        let control_type = element
            .get_cached_control_type()
            .map_err(|e| map_uia_err("focused_element control type", e))?;
        let name = element
            .get_cached_name()
            .map_err(|e| map_uia_err("focused_element name", e))?;
        let rect = element
            .get_cached_bounding_rectangle()
            .map_err(|e| map_uia_err("focused_element bounds", e))?;
        // IsPassword 是密码框的权威信号；读取失败同样 fail-closed。
        let secure = element
            .is_cached_password()
            .map_err(|e| map_uia_err("focused_element password", e))?;
        Ok(Some(ElementInfo {
            role: format!("{control_type:?}"),
            name: sanitize_name(&name),
            x: rect.get_left(),
            y: rect.get_top(),
            width: (rect.get_right() - rect.get_left()).max(0),
            height: (rect.get_bottom() - rect.get_top()).max(0),
            secure,
        }))
    }

    fn element_at_point(
        &mut self,
        x: i32,
        y: i32,
    ) -> Result<Option<ElementInfo>, ComputerUseError> {
        let uia = self.uia_client()?;
        let cache = Self::property_cache(&uia, TreeScope::Element)?;
        let element = uia
            .element_from_point_build_cache(Point::new(x, y), &cache)
            .map_err(|e| map_uia_err("element_at_point", e))?;
        let control_type = element
            .get_cached_control_type()
            .map_err(|e| map_uia_err("element_at_point control type", e))?;
        let name = element
            .get_cached_name()
            .map_err(|e| map_uia_err("element_at_point name", e))?;
        let rect = element
            .get_cached_bounding_rectangle()
            .map_err(|e| map_uia_err("element_at_point bounds", e))?;
        // IsPassword 是密码框的权威信号（Edit 控件的 password 变体必置位；
        // 浏览器自绘密码框也会暴露它）。读取失败必须报错（fail-closed）——
        // unwrap_or(false) 会让密码框被当成普通元素静默放行，T3 筛查失效。
        let secure = element
            .is_cached_password()
            .map_err(|e| map_uia_err("element_at_point password", e))?;
        Ok(Some(ElementInfo {
            role: format!("{control_type:?}"),
            name: sanitize_name(&name),
            x: rect.get_left(),
            y: rect.get_top(),
            width: (rect.get_right() - rect.get_left()).max(0),
            height: (rect.get_bottom() - rect.get_top()).max(0),
            secure,
        }))
    }
}

pub(super) fn create_backend() -> Result<Box<dyn ComputerUseBackend>, ComputerUseError> {
    Ok(Box::new(WindowsComputerUseBackend::new()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 解开 [`MappedKey::Key`]（测试辅助）。
    fn unwrap_mapped(plan: Result<MappedKey, ComputerUseError>) -> Option<enigo::Key> {
        match plan {
            Ok(MappedKey::Key(key)) => Some(key),
            Ok(MappedKey::UnicodeOnly(c)) => panic!("unexpected UnicodeOnly({c})"),
            Err(_) => None,
        }
    }

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
            (Key::Insert, EK::Insert),
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
            assert_eq!(
                unwrap_mapped(map_key(*input, 1)),
                Some(*expected),
                "mapping {input:?}"
            );
        }
        // 字符键：无论布局能否映射（Direct 或 NotInLayout→enigo 回退 Unicode），
        // 都走 enigo 虚拟键路径；只有需修饰键的字符才返回 UnicodeOnly。
        assert_eq!(
            unwrap_mapped(map_key(Key::Char('s'), 1)),
            Some(EK::Unicode('s'))
        );
        assert_eq!(
            unwrap_mapped(map_key(Key::Char('中'), 1)),
            Some(EK::Unicode('中'))
        );
        assert!(map_key(Key::Function(0), 1).is_err());
        assert!(map_key(Key::Function(13), 1).is_err());
    }

    #[test]
    fn char_injection_plan_follows_vkkeyscan_shift_bits() {
        // 布局映射失败（返回 -1）：如英文布局下的 CJK → enigo 自动回退 Unicode。
        assert_eq!(char_injection_from_scan(-1), CharInjection::NotInLayout);
        assert_eq!(
            char_injection_from_scan(i16::MIN),
            CharInjection::NotInLayout
        );
        // 无修饰：'a'（VK 0x41，高字节 0）。
        assert_eq!(char_injection_from_scan(0x0041), CharInjection::Direct);
        // 仅 VK、高字节为 0 的 OEM 键。
        assert_eq!(char_injection_from_scan(0x001B), CharInjection::Direct);
        // Shift 位（bit0）：美式键盘的 '+' = Shift + VK_OEM_PLUS(0xBB) → 0x01BB。
        // 这是 enigo 把高字节当 VK 的缺陷场景，必须改道。
        assert_eq!(
            char_injection_from_scan(0x01BB),
            CharInjection::NeedsModifier
        );
        // Ctrl 位（bit1）与 Alt 位（bit2，AltGr）：同样需要改道。
        assert_eq!(
            char_injection_from_scan(0x0241),
            CharInjection::NeedsModifier
        );
        assert_eq!(
            char_injection_from_scan(0x0451),
            CharInjection::NeedsModifier
        );
        // Shift+Ctrl+Alt 组合位。
        assert_eq!(
            char_injection_from_scan(0x0741),
            CharInjection::NeedsModifier
        );
        // 未知的其它高位组合位也按需修饰处理（fail-closed）。
        assert_eq!(
            char_injection_from_scan(0x0841),
            CharInjection::NeedsModifier
        );
    }

    #[test]
    fn shift_char_maps_to_unicode_only_alone_and_errors_in_chords() {
        use enigo::Key as EK;
        // 需修饰字符（如美式键盘的 '+' = Shift+VK_OEM_PLUS）单独成和弦：
        // 改走 Unicode 直注，不经 enigo 的缺陷 VK 换算。
        assert_eq!(
            map_char_with_plan('+', CharInjection::NeedsModifier, 1).ok(),
            Some(MappedKey::UnicodeOnly('+'))
        );
        // 与修饰键组合：显式报错（enigo 会把 shift state 高字节当 VK，目标收不到）。
        let err = map_char_with_plan('+', CharInjection::NeedsModifier, 2).unwrap_err();
        assert!(err.to_string().contains("requires shift"));
        assert!(err.to_string().contains("modifier chords"));
        // Direct / NotInLayout 维持 enigo 路径，与和弦长度无关。
        assert_eq!(
            map_char_with_plan('s', CharInjection::Direct, 2).ok(),
            Some(MappedKey::Key(EK::Unicode('s')))
        );
        assert_eq!(
            map_char_with_plan('中', CharInjection::NotInLayout, 1).ok(),
            Some(MappedKey::Key(EK::Unicode('中')))
        );
    }

    #[test]
    fn thread_pmv2_requires_both_awareness_and_context() {
        // Per-Monitor v1：awareness == 2 但上下文不是 PMv2 伪句柄。
        assert!(!thread_is_pmv2(2, false));
        // 非 PM 感知：unaware(0) / system(1) / invalid(-1) / gdiscaled(3)。
        assert!(!thread_is_pmv2(0, false));
        assert!(!thread_is_pmv2(1, false));
        assert!(!thread_is_pmv2(-1, false));
        assert!(!thread_is_pmv2(3, false));
        // 只有 awareness == 2 且上下文确为 PMv2 才通过。
        assert!(thread_is_pmv2(2, true));
        assert!(!thread_is_pmv2(1, true));
    }

    #[test]
    fn scroll_mapping_matches_enigo_sign_convention() {
        assert_eq!(map_scroll(ScrollDirection::Up, 3), (Axis::Vertical, -3));
        assert_eq!(map_scroll(ScrollDirection::Down, 3), (Axis::Vertical, 3));
        assert_eq!(map_scroll(ScrollDirection::Left, 2), (Axis::Horizontal, -2));
        assert_eq!(map_scroll(ScrollDirection::Right, 2), (Axis::Horizontal, 2));
        // 常数本身必须保证 enigo 乘 WHEEL_DELTA 后不溢出 i32（向下取整除法）。
        assert_eq!(MAX_SCROLL_CLICKS, (i32::MAX / 120) as u32);
        let max_scaled = i64::from(MAX_SCROLL_CLICKS) * i64::from(WHEEL_DELTA);
        assert!(max_scaled <= i64::from(i32::MAX));
        assert!(max_scaled + i64::from(WHEEL_DELTA) > i64::from(i32::MAX));
        // 边界内不钳位。
        assert_eq!(
            map_scroll(ScrollDirection::Down, MAX_SCROLL_CLICKS),
            (Axis::Vertical, MAX_SCROLL_CLICKS as i32)
        );
        // 超界钳位（u32::MAX 原实现会 i32 溢出）。
        assert_eq!(
            map_scroll(ScrollDirection::Down, u32::MAX),
            (Axis::Vertical, MAX_SCROLL_CLICKS as i32)
        );
        assert_eq!(
            map_scroll(ScrollDirection::Up, u32::MAX),
            (Axis::Vertical, -(MAX_SCROLL_CLICKS as i32))
        );
    }

    #[test]
    fn abs_axis_normalization_maps_full_virtual_desktop() {
        // 主屏 (0,0) 1920x1080：左右端点与精确中点（半值向上取整）。
        assert_eq!(normalize_abs_axis(0, 0, 1920), 0);
        assert_eq!(normalize_abs_axis(1919, 0, 1920), 65535);
        assert_eq!(normalize_abs_axis(960, 0, 1921), 32768); // 32767.5 → 32768
        // 域外钳位：超出右缘/低于左缘都压到边界。
        assert_eq!(normalize_abs_axis(1920, 0, 1920), 65535);
        assert_eq!(normalize_abs_axis(-1, 0, 1920), 0);
        assert_eq!(normalize_abs_axis(5000, 0, 1920), 65535);
        // 负原点（左侧副屏 -1920..=-1）：两端正确映射。
        assert_eq!(normalize_abs_axis(-1920, -1920, 1920), 0);
        assert_eq!(normalize_abs_axis(-1, -1920, 1920), 65535);
        // 副屏内的坐标不会折算到主屏（0..65535 域仅覆盖该副屏）。
        let mid = normalize_abs_axis(-960, -1920, 1920);
        assert!((1..65534).contains(&mid), "midpoint {mid} out of range");
        assert_eq!(normalize_abs_axis(0, -1920, 1920), 65535); // 右缘外一点钳位
        // 顶部负原点（副屏在主屏上方）。
        assert_eq!(normalize_abs_axis(-1080, -1080, 1080), 0);
        assert_eq!(normalize_abs_axis(-1, -1080, 1080), 65535);
        // 退化尺寸（单像素/无效）。
        assert_eq!(normalize_abs_axis(100, 0, 1), 0);
        assert_eq!(normalize_abs_axis(100, 0, 0), 0);
        assert_eq!(normalize_abs_axis(100, 0, -5), 0);
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
    fn sanitize_name_strips_quotes_and_caps_length() {
        assert_eq!(sanitize_name("a\"b\nc\td"), "a'b c d");
        let long = "x".repeat(MAX_NODE_NAME_CHARS + 50);
        assert_eq!(sanitize_name(&long).chars().count(), MAX_NODE_NAME_CHARS);
        assert_eq!(sanitize_name("  padded  "), "padded");
    }
}
