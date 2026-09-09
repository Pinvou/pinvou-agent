//! Windows 后端：xcap（GDI BitBlt）截屏 + enigo（SendInput）输入注入
//! + uiautomation（UI Automation）无障碍树。
//!
//! 坐标约定：进程经 tao 置为 Per-Monitor-V2 DPI aware，截图像素、UIA
//! BoundingRectangle、SendInput 绝对坐标同为**设备物理像素**，故
//! `Capture.input_scale_x/y = 1.0`；多显示器虚拟桌面原点可为负，原样透传。
//!
//! 线程与 COM：所有对象于 backend worker 线程构造、使用、析构（见
//! `super::super::backend::BackendHandle`），不跨线程移动。`Enigo` 持有在结构内；
//! `uiautomation::UIAutomation` 因 windows-rs COM 接口为 `!Send` 而不入结构——
//! `new()` 在该线程以 MTA 初始化 COM 一次，此后每次 UIA 调用现建现析客户端。
//! 全部 UIA 调用都留在这条无窗口的 worker 线程上，避免与 WebView UI 线程死锁。
//!
//! 已知限制（UIPI / 安全桌面）：
//! - 本进程以 medium IL 运行且不带 `uiAccess`，对**以管理员身份运行**的目标窗口，
//!   SendInput 注入会被 UIPI 静默丢弃、UIA 读取返回 access denied。能检测的
//!   拒绝访问（HRESULT 0x80070005）会映射为显式 `unavailable` 错误；被静默丢弃的
//!   输入无法直接探测，由上层动作验证（执行后补拍截图）兜底。
//! - 安全桌面（锁屏 / UAC 同意框）期间截图为全黑、注入与 UIA 全部失效：全黑帧
//!   会作为显式 `unavailable` 错误返回。
//! - `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` 与 DRM 保护内容在截图中
//!   呈现为黑块，属系统预期行为。

use std::fmt::Write as _;
use std::thread::sleep;
use std::time::Duration;

use enigo::{Axis, Button, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};
use uiautomation::UIAutomation;
use uiautomation::types::{ControlType, Point, TreeScope, UIProperty};
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

/// Win32 `E_ACCESSDENIED`（0x80070005）：UIA 跨完整性级别读取被拒的典型信号。
const E_ACCESSDENIED: i32 = -2147024891;

fn map_xcap_err(context: &str, err: xcap::XCapError) -> ComputerUseError {
    ComputerUseError::failed(format!("{context}: {err}"))
}

fn map_input_err(context: &str, err: enigo::InputError) -> ComputerUseError {
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

/// 本 crate `Key` → enigo 按键。`Char` 走 `Unicode`：enigo 在 Windows 上先经
/// `VkKeyScanExW` 映射为虚拟键（修饰键和弦正确），映射不了再回退 Unicode 事件。
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
/// Horizontal 正值向右/负值向左。
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

fn write_tree_node(
    element: &uiautomation::UIElement,
    depth: u32,
    max_depth: u32,
    writer: &mut TreeWriter,
    out: &mut String,
) -> Result<(), ComputerUseError> {
    if writer.remaining == 0 {
        writer.truncated = true;
        return Ok(());
    }
    writer.remaining -= 1;
    let index = writer.next_index;
    writer.next_index += 1;
    let line = format_tree_line(index, depth, element)?;
    let _ = writeln!(out, "{line}");
    if depth >= max_depth {
        return Ok(());
    }
    // 元素可能在遍历中途销毁：子树枚举失败按“该分支结束”处理，不拖垮整次抓取。
    let Ok(children) = element.get_cached_children() else {
        return Ok(());
    };
    for child in &children {
        write_tree_node(child, depth + 1, max_depth, writer, out)?;
        if writer.remaining == 0 {
            writer.truncated = true;
            break;
        }
    }
    Ok(())
}

pub(super) struct WindowsComputerUseBackend {
    enigo: Enigo,
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
        Ok(Self { enigo })
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
        if let Some(primary) = monitors.iter().position(|m| m.is_primary().unwrap_or(false)) {
            return Ok(monitors.swap_remove(primary));
        }
        monitors.drain(..).next().ok_or_else(|| {
            ComputerUseError::unavailable("no monitor available for screen capture")
        })
    }

    /// 属性缓存请求：把逐节点多次跨进程 COM 往返合并成一次批量读取。
    /// `subtree` 为 true 时递归缓存整个子树（`build_updated_cache` 用）。
    fn property_cache(
        uia: &UIAutomation,
        subtree: bool,
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
        if subtree {
            cache
                .set_tree_scope(TreeScope::Subtree)
                .map_err(|e| map_uia_err("ui_tree cache scope", e))?;
            let control_view = uia
                .get_control_view_condition()
                .map_err(|e| map_uia_err("ui_tree cache filter", e))?;
            cache
                .set_tree_filter(control_view)
                .map_err(|e| map_uia_err("ui_tree cache filter", e))?;
        }
        Ok(cache)
    }

    /// 树的根：焦点元素所属的顶层窗口；无焦点/爬升失败时回退桌面根。
    fn focused_window_root(
        uia: &UIAutomation,
    ) -> Result<uiautomation::UIElement, ComputerUseError> {
        let desktop = || uia.get_root_element().map_err(|e| map_uia_err("ui_tree root", e));
        let Ok(focused) = uia.get_focused_element() else {
            return desktop();
        };
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
            let mapped = map_key(*key)?;
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
}

impl ComputerUseBackend for WindowsComputerUseBackend {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            screenshot: true,
            input: true,
            ui_tree: true,
            notes: "windows: physical-pixel coordinates; elevated (run as administrator) target \
                    windows are unreachable (UIPI), secure desktop (lock screen / UAC) blocks \
                    capture and input entirely"
                .to_string(),
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
        self.enigo
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|err| map_input_err("mouse move", err))
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
        self.enigo
            .move_mouse(from.0, from.1, Coordinate::Abs)
            .map_err(|err| map_input_err("drag move to start", err))?;
        sleep(Duration::from_millis(CLICK_SETTLE_MS));
        self.enigo
            .button(Button::Left, Direction::Press)
            .map_err(|err| map_input_err("drag press", err))?;
        sleep(Duration::from_millis(DRAG_PRESS_SETTLE_MS));
        let result = (|| {
            for (x, y) in drag_waypoints(from, to, DRAG_STEPS) {
                self.enigo
                    .move_mouse(x, y, Coordinate::Abs)
                    .map_err(|err| map_input_err("drag move", err))?;
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

    fn scroll(
        &mut self,
        direction: ScrollDirection,
        clicks: u32,
    ) -> Result<(), ComputerUseError> {
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
        let cache = Self::property_cache(&uia, true)?;
        // 一次跨进程往返抓回整个子树的缓存属性。
        let cached_root = root
            .build_updated_cache(&cache)
            .map_err(|e| map_uia_err("ui_tree subtree fetch", e))?;
        let mut out = String::new();
        let mut writer = TreeWriter {
            next_index: 0,
            remaining: max_nodes,
            truncated: false,
        };
        write_tree_node(&cached_root, 0, max_depth, &mut writer, &mut out)?;
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
        let uia = self.uia_client()?;
        let cache = Self::property_cache(&uia, false)?;
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
        // 浏览器自绘密码框也会暴露它）。
        let secure = element.is_cached_password().unwrap_or(false);
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
    fn sanitize_name_strips_quotes_and_caps_length() {
        assert_eq!(sanitize_name("a\"b\nc\td"), "a'b c d");
        let long = "x".repeat(MAX_NODE_NAME_CHARS + 50);
        assert_eq!(sanitize_name(&long).chars().count(), MAX_NODE_NAME_CHARS);
        assert_eq!(sanitize_name("  padded  "), "padded");
    }
}
