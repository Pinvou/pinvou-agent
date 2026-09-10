//! Linux 后端:X11 全功能;Wayland 截屏走 portal RemoteDesktop 会话绑定的
//! ScreenCast 流(PipeWire,见 [`super::wayland_portal`]/[`super::wayland_capture`]),
//! 输入走 xdg-desktop-portal RemoteDesktop。
//!
//! 会话探测决定能力面(`detect_session` 为纯函数,便于单测):
//! - 主信号 `XDG_SESSION_TYPE`(`x11`/`wayland`/`tty`),`WAYLAND_DISPLAY` +
//!   `XDG_RUNTIME_DIR` 佐证;`DISPLAY` 绝不单独作为 X11 判据(XWayland 下也会
//!   设置)。两者都不可达 → 构造时显式 `unsupported`。
//! - X11:截屏 `xcap::Monitor`(xcb/XGetImage,根窗口物理像素);输入 `enigo`
//!   (x11rb XTEST,坐标同为根窗口像素,`Capture.input_scale_x/y = 1.0`)。
//!   注意 xcap 在 X11 下把 RandR 几何除以 Xft.dpi/96 报告(逻辑坐标),而
//!   `capture_image` 按根窗口像素截图、XTEST 也注入根窗口像素,故
//!   `origin_x/y` 需把 xcap 的逻辑原点乘回 `scale_factor`,输入倍率恒为 1.0。
//! - Wayland:截屏与输入共用 portal RemoteDesktop 会话(懒启动,首次截屏或
//!   输入动作弹一次系统授权对话框):截屏取 `OpenPipewireRemote` 的 PipeWire
//!   流帧(输入坐标 = 流本地像素,origin (0,0);KDE 的输入单位是流逻辑像素,
//!   按合成器报告的流尺寸折算倍率)。会话内的截屏流不可用时回退 xcap 的
//!   GNOME-Shell/portal/wlroots 链(构造时探测,portal 路径可能每次弹授权
//!   对话框,`capabilities().notes` 注明);两条路都失败则显式 `unsupported`。
//!   输入合成走 portal RemoteDesktop;探测不到 portal 时输入显式不可用。
//! - 无障碍树:`atspi`(AT-SPI over D-Bus,独立 a11y 总线,X11/Wayland 均可)。
//!   trait 为同步而 atspi 为 async:backend 持有一个专用 current-thread tokio
//!   runtime,在 worker 线程(普通 std::thread,不含引擎主 runtime)上
//!   `block_on`,无嵌套运行时死锁风险。Wayland 下 Component extents 为
//!   尽力而为(合成器/工具包相关),ui_tree 输出头部注明。
//! - 线程约定同其他平台:对象于 worker 线程构造,不得跨线程移动。

use std::thread::sleep;
use std::time::{Duration, Instant};

use atspi::proxy::accessible::{AccessibleProxy, ObjectRefExt};
use atspi::proxy::component::ComponentProxy;
use atspi::{AccessibilityConnection, CoordType, Role, State, StateSet};
use enigo::{Button, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};
use xcap::Monitor;

use super::super::backend::ComputerUseBackend;
use super::super::types::{
    Capabilities, Capture, ComputerUseError, ElementInfo, Key, MouseButton, ScrollDirection,
    UiTreeOptions,
};
use super::wayland_portal::{self, PortalInput};

/// 移动后点击/按下前的静置时间(XTEST 注入与合成器处理间的竞态缓冲)。
const SETTLE_MS: u64 = 40;
/// 多次点击(双击/三击)之间的间隔。
const CLICK_GAP_MS: u64 = 40;
/// 拖拽插值步数与每步间隔(过快的瞬时移动会被部分应用识别为非拖拽)。
const DRAG_STEPS: u32 = 12;
const DRAG_STEP_MS: u64 = 10;
/// 滚动每格之间的间隔。
const SCROLL_GAP_MS: u64 = 15;
/// ui_tree 默认抓取上限。
const DEFAULT_MAX_DEPTH: u32 = 8;
const DEFAULT_MAX_NODES: usize = 200;
/// 单节点名称最长保留字符数(防止超大文本撑爆工具结果)。
const MAX_NAME_CHARS: usize = 80;

/// 会话类型(探测结果)。纯数据,供 `detect_session` 单测断言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionKind {
    X11,
    Wayland,
    /// tty/无任何图形会话信号。
    NoDisplay,
}

/// 会话探测结果(含诊断信息,用于错误与 capabilities 文案)。
#[derive(Debug, Clone)]
struct SessionInfo {
    kind: SessionKind,
    /// 原始 `XDG_SESSION_TYPE`(小写化),未设置/无法识别时为 None。
    session_type: Option<String>,
    /// `XDG_CURRENT_DESKTOP`(诊断用,如 "GNOME"/"KDE"/"sway")。
    desktop: Option<String>,
    has_wayland_display: bool,
    has_display: bool,
}

impl SessionInfo {
    fn desktop_label(&self) -> &str {
        self.desktop.as_deref().unwrap_or("unknown")
    }
}

/// 纯函数会话探测:从环境变量映射判定 X11 / Wayland / 无显示。
///
/// 规则:主信号 `XDG_SESSION_TYPE`;未设置或无法识别时回退到
/// `WAYLAND_DISPLAY`(+`XDG_RUNTIME_DIR` 佐证);`DISPLAY` 只在没有任何
/// Wayland 信号时才算 X11 判据(它在 XWayland 会话里同样存在)。
fn detect_session(env: &dyn Fn(&str) -> Option<String>) -> SessionInfo {
    let nonempty = |key: &str| env(key).filter(|value| !value.trim().is_empty());
    let session_type = nonempty("XDG_SESSION_TYPE").map(|value| value.trim().to_ascii_lowercase());
    let wayland_display = nonempty("WAYLAND_DISPLAY");
    let runtime_dir = nonempty("XDG_RUNTIME_DIR");
    let display = nonempty("DISPLAY");
    let desktop = nonempty("XDG_CURRENT_DESKTOP");

    let kind = match session_type.as_deref() {
        Some("wayland") => SessionKind::Wayland,
        Some("x11") => SessionKind::X11,
        Some("tty") => SessionKind::NoDisplay,
        _ => {
            if wayland_display.is_some() && runtime_dir.is_some() {
                // 强佐证:Wayland socket 名 + runtime 目录都在。
                SessionKind::Wayland
            } else if wayland_display.is_some() {
                // 弱佐证(缺 XDG_RUNTIME_DIR)仍按 Wayland 处理:
                // 截屏探测与显式 unsupported 路径会兜底。
                SessionKind::Wayland
            } else if display.is_some() {
                SessionKind::X11
            } else {
                SessionKind::NoDisplay
            }
        }
    };

    SessionInfo {
        kind,
        session_type,
        desktop,
        has_wayland_display: wayland_display.is_some(),
        has_display: display.is_some(),
    }
}

/// types 的归一化按键 → enigo 按键。`Char` 走 Unicode(enigo 在 X11 上
/// 用临时 keycode 重映射注入,中文等任意字符可用)。F13-F20 可映射,
/// 超出 F20 显式报错。
fn map_enigo_key(key: Key) -> Result<enigo::Key, ComputerUseError> {
    let mapped = match key {
        Key::Control => enigo::Key::Control,
        Key::Alt => enigo::Key::Alt,
        Key::Shift => enigo::Key::Shift,
        Key::Meta => enigo::Key::Meta,
        Key::Enter => enigo::Key::Return,
        Key::Escape => enigo::Key::Escape,
        Key::Tab => enigo::Key::Tab,
        Key::Space => enigo::Key::Space,
        Key::Backspace => enigo::Key::Backspace,
        Key::Delete => enigo::Key::Delete,
        Key::Insert => enigo::Key::Insert,
        Key::Up => enigo::Key::UpArrow,
        Key::Down => enigo::Key::DownArrow,
        Key::Left => enigo::Key::LeftArrow,
        Key::Right => enigo::Key::RightArrow,
        Key::Home => enigo::Key::Home,
        Key::End => enigo::Key::End,
        Key::PageUp => enigo::Key::PageUp,
        Key::PageDown => enigo::Key::PageDown,
        Key::Function(n) => match n {
            1 => enigo::Key::F1,
            2 => enigo::Key::F2,
            3 => enigo::Key::F3,
            4 => enigo::Key::F4,
            5 => enigo::Key::F5,
            6 => enigo::Key::F6,
            7 => enigo::Key::F7,
            8 => enigo::Key::F8,
            9 => enigo::Key::F9,
            10 => enigo::Key::F10,
            11 => enigo::Key::F11,
            12 => enigo::Key::F12,
            13 => enigo::Key::F13,
            14 => enigo::Key::F14,
            15 => enigo::Key::F15,
            16 => enigo::Key::F16,
            17 => enigo::Key::F17,
            18 => enigo::Key::F18,
            19 => enigo::Key::F19,
            20 => enigo::Key::F20,
            _ => {
                return Err(ComputerUseError::unsupported(
                    "input",
                    format!("function key F{n} is out of the mappable range F1-F20"),
                ));
            }
        },
        Key::Char(c) => enigo::Key::Unicode(c),
    };
    Ok(mapped)
}

fn map_enigo_button(button: MouseButton) -> Button {
    match button {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
    }
}

fn input_failed(context: &str, error: impl std::fmt::Display) -> ComputerUseError {
    ComputerUseError::failed(format!("{context}: {error}"))
}

fn settle() {
    sleep(Duration::from_millis(SETTLE_MS));
}

/// 名称清洗:去引号/换行并截断,保证单行输出。
fn sanitize_name(raw: &str) -> String {
    raw.chars()
        .take(MAX_NAME_CHARS)
        .map(|c| match c {
            '"' => '\'',
            '\n' | '\r' => ' ',
            c => c,
        })
        .collect()
}

/// 节点标志位(紧凑单行输出用)。PasswordText 即安全输入框(T3 确认信号)。
fn state_flags(role: Role, state: Option<StateSet>) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if role == Role::PasswordText {
        flags.push("secure");
    }
    if let Some(set) = state {
        if set.contains(State::Active) {
            flags.push("active");
        }
        if set.contains(State::Focused) {
            flags.push("focused");
        }
        if set.contains(State::Editable) {
            flags.push("editable");
        }
        if set.contains(State::Modal) {
            flags.push("modal");
        }
        if !set.contains(State::Enabled) {
            flags.push("disabled");
        }
    }
    flags
}

/// 由 AccessibleProxy 盲建同对象的 ComponentProxy(不对每个节点先查
/// GetInterfaces,避免一次额外 D-Bus 往返;不支持 Component 的对象在
/// get_extents 时报错,按"无 bounds"处理)。
/// 注意:crate 未直接依赖 zbus,`zbus::Connection` 类型一律经
/// `AccessibilityConnection::connection()` 以内联临时值传入,不出现在签名里。
async fn component_of<'c>(
    conn: &'c AccessibilityConnection,
    proxy: &AccessibleProxy<'_>,
) -> Option<ComponentProxy<'c>> {
    ComponentProxy::builder(conn.connection())
        .destination(proxy.inner().destination().as_str().to_string())
        .ok()?
        .path(proxy.inner().path().as_str().to_string())
        .ok()?
        .build()
        .await
        .ok()
}

async fn screen_extents(
    conn: &AccessibilityConnection,
    proxy: &AccessibleProxy<'_>,
) -> Option<(i32, i32, i32, i32)> {
    component_of(conn, proxy)
        .await?
        .get_extents(CoordType::Screen)
        .await
        .ok()
}

/// 注册表根的全部应用顶层窗口(不区分活动与否)。
async fn app_windows<'c>(
    conn: &'c AccessibilityConnection,
    root: &AccessibleProxy<'_>,
) -> Vec<AccessibleProxy<'c>> {
    let mut windows = Vec::new();
    let Ok(apps) = root.get_children().await else {
        return windows;
    };
    for app_ref in apps {
        if app_ref.is_null() {
            continue;
        }
        let Ok(app) = app_ref.into_accessible_proxy(conn.connection()).await else {
            continue;
        };
        let Ok(children) = app.get_children().await else {
            continue;
        };
        for child_ref in children {
            if child_ref.is_null() {
                continue;
            }
            if let Ok(window) = child_ref.into_accessible_proxy(conn.connection()).await {
                windows.push(window);
            }
        }
    }
    windows
}

/// 把带 `State::Active` 的窗口排到最前(命中测试优先活动窗口)。
async fn active_first(windows: &mut [AccessibleProxy<'_>]) {
    for (index, window) in windows.iter().enumerate() {
        if let Ok(state) = window.get_state().await {
            if state.contains(State::Active) {
                windows.swap(0, index);
                return;
            }
        }
    }
}

/// 紧凑文本树写出器:`[i] role "name" (x,y,w,h) flags`,深度/节点数双上限。
struct TreeWriter<'c> {
    conn: &'c AccessibilityConnection,
    out: String,
    next_index: usize,
    max_nodes: usize,
    max_depth: u32,
}

impl TreeWriter<'_> {
    fn is_full(&self) -> bool {
        self.next_index >= self.max_nodes
    }

    async fn write_node(&mut self, proxy: &AccessibleProxy<'_>, depth: u32) {
        if self.is_full() {
            return;
        }
        let index = self.next_index;
        self.next_index += 1;

        let role = proxy.get_role().await.unwrap_or(Role::Unknown);
        let name = sanitize_name(&proxy.name().await.unwrap_or_default());
        let state = proxy.get_state().await.ok();
        let extents = screen_extents(self.conn, proxy).await;

        let indent = "  ".repeat(depth as usize);
        let mut line = format!("{indent}[{index}] {} \"{name}\"", role.name());
        if let Some((x, y, width, height)) = extents {
            line.push_str(&format!(" ({x},{y},{width},{height})"));
        }
        let flags = state_flags(role, state);
        if !flags.is_empty() {
            line.push(' ');
            line.push_str(&flags.join(" "));
        }
        self.out.push_str(&line);
        self.out.push('\n');

        if depth >= self.max_depth {
            return;
        }
        let Ok(children) = proxy.get_children().await else {
            return;
        };
        for child_ref in children {
            if self.is_full() {
                return;
            }
            if child_ref.is_null() {
                continue;
            }
            if let Ok(child) = child_ref
                .into_accessible_proxy(self.conn.connection())
                .await
            {
                Box::pin(self.write_node(&child, depth + 1)).await;
            }
        }
    }
}

async fn ui_tree_async(
    conn: &AccessibilityConnection,
    opts: &UiTreeOptions,
    wayland: bool,
    desktop: &str,
) -> Result<String, ComputerUseError> {
    let root = conn
        .root_accessible_on_registry()
        .await
        .map_err(|error| ComputerUseError::unavailable(format!("AT-SPI registry root: {error}")))?;

    let session = if wayland { "wayland" } else { "x11" };
    // Wayland 没有全局坐标系,AT-SPI Component extents 是尽力而为
    // (Qt 已知有偏差;X11/XWayland 下是可靠的全局根窗口像素)。
    let extents_note = if wayland {
        "best-effort (wayland: no global coordinate space)"
    } else {
        "screen pixels"
    };
    let mut writer = TreeWriter {
        conn,
        out: format!("# atspi tree session={session} desktop={desktop} extents={extents_note}\n"),
        next_index: 0,
        max_nodes: opts.max_nodes.map_or(DEFAULT_MAX_NODES, |n| n as usize),
        max_depth: opts.max_depth.unwrap_or(DEFAULT_MAX_DEPTH),
    };

    let mut windows = app_windows(conn, &root).await;
    active_first(&mut windows).await;
    if let Some(active) = windows.first() {
        if let Ok(state) = active.get_state().await {
            if state.contains(State::Active) {
                // 常规路径:只序列化活动窗口子树。
                writer.write_node(active, 0).await;
                if writer.is_full() {
                    writer.out.push_str("# truncated: node cap reached\n");
                }
                return Ok(writer.out);
            }
        }
    }
    // 无活动窗口(全屏锁屏/空桌面等):退化为注册表根的浅层树(应用列表)。
    writer.write_node(&root, 0).await;
    if writer.is_full() {
        writer.out.push_str("# truncated: node cap reached\n");
    }
    Ok(writer.out)
}

async fn element_at_point_async(
    conn: &AccessibilityConnection,
    x: i32,
    y: i32,
) -> Result<Option<ElementInfo>, ComputerUseError> {
    let root = conn
        .root_accessible_on_registry()
        .await
        .map_err(|error| ComputerUseError::unavailable(format!("AT-SPI registry root: {error}")))?;

    let mut windows = app_windows(conn, &root).await;
    active_first(&mut windows).await;
    for window in &windows {
        let Some((wx, wy, ww, wh)) = screen_extents(conn, window).await else {
            continue;
        };
        if x < wx || x >= wx + ww || y < wy || y >= wy + wh {
            continue;
        }
        let Some(component) = component_of(conn, window).await else {
            continue;
        };
        let Ok(target_ref) = component
            .get_accessible_at_point(x, y, CoordType::Screen)
            .await
        else {
            continue;
        };
        if target_ref.is_null() {
            continue;
        }
        let Ok(target) = target_ref.into_accessible_proxy(conn.connection()).await else {
            continue;
        };
        let role = target.get_role().await.unwrap_or(Role::Unknown);
        let name = sanitize_name(&target.name().await.unwrap_or_default());
        let (ex, ey, ew, eh) = screen_extents(conn, &target).await.unwrap_or((x, y, 0, 0));
        return Ok(Some(ElementInfo {
            role: role.name().to_string(),
            name,
            x: ex,
            y: ey,
            width: ew,
            height: eh,
            secure: role == Role::PasswordText,
        }));
    }
    Ok(None)
}

/// AT-SPI 初始化:先打开会话 a11y 开关(Electron/Chromium 只在有 AT 注册后
/// 才构建无障碍树;该调用失败非致命),再连接 a11y 总线。
async fn a11y_init() -> Result<AccessibilityConnection, String> {
    let _ = atspi::connection::set_session_accessibility(true).await;
    AccessibilityConnection::new()
        .await
        .map_err(|error| error.to_string())
}

/// Wayland 截屏探测:枚举显示器并对主屏(兜底首个)实际抓一帧。
/// xcap 的 Wayland 链路(GNOME Shell D-Bus → portal Screenshot → wlroots
/// wayshot)任一可用即成功;portal 路径可能向用户弹授权对话框。
fn probe_wayland_screenshot() -> Result<(), String> {
    let monitors =
        Monitor::all().map_err(|error| format!("monitor enumeration failed: {error}"))?;
    let monitor = monitors
        .iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .or_else(|| monitors.first())
        .ok_or_else(|| "no monitors reported".to_string())?;
    let image = monitor
        .capture_image()
        .map_err(|error| format!("capture failed: {error}"))?;
    if image.width() == 0 || image.height() == 0 {
        return Err("capture returned an empty image".to_string());
    }
    Ok(())
}

/// xcap 的 Wayland 路径内部有 `.expect(...)`(PNG 重编码),包一层
/// catch_unwind 把潜在 panic 转成显式错误,避免炸掉 backend worker 线程。
fn probe_wayland_screenshot_guarded() -> Result<(), String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(probe_wayland_screenshot)) {
        Ok(result) => result,
        Err(_) => Err("capture panicked inside xcap's Wayland fallback chain".to_string()),
    }
}

pub(super) struct LinuxComputerUseBackend {
    session: SessionInfo,
    /// 专用 current-thread runtime:atspi(async/zbus)在同步 trait 内的桥。
    runtime: tokio::runtime::Runtime,
    a11y: Option<AccessibilityConnection>,
    a11y_init_error: Option<String>,
    /// 仅 X11 构造;Wayland 输入走 `wayland_portal`。
    input: Option<Enigo>,
    input_init_error: Option<String>,
    /// 仅 Wayland 构造(portal RemoteDesktop,懒启动会话)。
    wayland_portal: Option<PortalInput>,
    wayland_portal_error: Option<String>,
    wayland_screenshot_ok: bool,
    wayland_screenshot_error: Option<String>,
    /// 同会话截屏流最近一次失败原因(回退 xcap 链后随错误透出)。
    wayland_portal_capture_error: Option<String>,
    /// 最近一次输入动作的开始时刻:同会话截屏流是 damage 驱动的,补拍
    /// 截图要等比它新的帧,才能看到动作后的画面(无视觉变化时沿用现有帧)。
    last_input_at: Option<Instant>,
}

impl LinuxComputerUseBackend {
    fn is_wayland(&self) -> bool {
        self.session.kind == SessionKind::Wayland
    }

    /// 输入动作入口打点(补拍截图等比它新的帧)。
    fn note_input(&mut self) {
        self.last_input_at = Some(Instant::now());
    }

    /// Wayland 首选截屏:portal 同会话 ScreenCast 流(PipeWire)。会话未启动
    /// 则启动(首次截屏弹一次系统授权对话框,与输入共用)。失败返回 None 并
    /// 记录原因(`wayland_portal_capture_error`,回退 xcap 链后随错误透出)。
    fn wayland_portal_capture(&mut self) -> Option<Capture> {
        let portal = self.wayland_portal.as_mut()?;
        let frame = match portal.capture_frame(self.last_input_at) {
            Ok(frame) => frame,
            Err(error) => {
                self.wayland_portal_capture_error = Some(error.to_string());
                return None;
            }
        };
        // 输入坐标 = 流本地像素(mutter 语义;origin 由合成器/portal 层内部
        // 处理)。KDE 的输入单位是流本地逻辑像素而其缓冲是物理像素,按合成器
        // 报告的流尺寸折算;其余合成器(含 scale=1 的 KDE)倍率为 1。
        let input_scale = if self
            .session
            .desktop
            .as_deref()
            .map_or(false, |desktop| desktop.contains("KDE"))
        {
            match portal.stream_logical_size() {
                Some((lw, lh)) if lw > 0 && lh > 0 && frame.width > 0 && frame.height > 0 => (
                    f64::from(lw) / f64::from(frame.width),
                    f64::from(lh) / f64::from(frame.height),
                ),
                _ => (1.0, 1.0),
            }
        } else {
            (1.0, 1.0)
        };
        Some(Capture {
            rgba: frame.rgba,
            width: frame.width,
            height: frame.height,
            origin_x: 0,
            origin_y: 0,
            input_scale_x: input_scale.0,
            input_scale_y: input_scale.1,
        })
    }

    fn require_enigo(&mut self) -> Result<&mut Enigo, ComputerUseError> {
        if self.is_wayland() {
            return Err(ComputerUseError::unsupported(
                "input",
                "XTEST input is not used on Wayland sessions (XWayland would only reach \
                 X11 clients); input goes through the portal RemoteDesktop backend",
            ));
        }
        match self.input.as_mut() {
            Some(enigo) => Ok(enigo),
            None => {
                let detail = self
                    .input_init_error
                    .as_deref()
                    .unwrap_or("unknown error")
                    .to_string();
                Err(ComputerUseError::unavailable(format!(
                    "X11 input connection (XTEST) failed at backend init: {detail}"
                )))
            }
        }
    }

    /// Wayland portal 输入后端(探测失败的粘性错误在 `wayland_portal_error`)。
    /// Wayland 截屏探测失败后在下一次 capture 时重试（评审发现：旧实现一次
    /// 探测失败就粘死整个 backend 生命周期——portal 对话框被用户误关、合成器
    /// 短暂抖动都会永久失去截屏，且无任何重试入口）。
    fn ensure_wayland_capture(&mut self) -> Result<(), ComputerUseError> {
        if !self.is_wayland() || self.wayland_screenshot_ok {
            return Ok(());
        }
        match probe_wayland_screenshot_guarded() {
            Ok(()) => {
                self.wayland_screenshot_ok = true;
                self.wayland_screenshot_error = None;
            }
            Err(error) => self.wayland_screenshot_error = Some(error),
        }
        if !self.wayland_screenshot_ok {
            let portal_note = self
                .wayland_portal_capture_error
                .as_deref()
                .map(|error| format!("; same-session stream capture failed: {error}"))
                .unwrap_or_default();
            return Err(ComputerUseError::unsupported(
                "screenshot",
                format!(
                    "screenshot on Wayland compositor {}: {}{portal_note}",
                    self.session.desktop_label(),
                    self.wayland_screenshot_error
                        .as_deref()
                        .unwrap_or("probe failed")
                ),
            ));
        }
        Ok(())
    }

    fn require_portal(&mut self) -> Result<&mut PortalInput, ComputerUseError> {
        match self.wayland_portal.as_mut() {
            Some(portal) => Ok(portal),
            None => {
                let detail = self
                    .wayland_portal_error
                    .as_deref()
                    .unwrap_or("unknown error");
                Err(ComputerUseError::unavailable(format!(
                    "Wayland input via xdg-desktop-portal RemoteDesktop is not available: \
                     {detail}"
                )))
            }
        }
    }

    fn require_a11y(&self) -> Result<&AccessibilityConnection, ComputerUseError> {
        match self.a11y.as_ref() {
            Some(conn) => Ok(conn),
            None => {
                let detail = self.a11y_init_error.as_deref().unwrap_or("unknown error");
                Err(ComputerUseError::unavailable(format!(
                    "AT-SPI bus connection failed at backend init: {detail}"
                )))
            }
        }
    }

    fn press_chord(enigo: &mut Enigo, keys: &[enigo::Key]) -> Result<(), ComputerUseError> {
        for (index, key) in keys.iter().enumerate() {
            if let Err(error) = enigo.key(*key, Direction::Press) {
                // 错误路径上释放已按下的键,避免修饰键卡死。
                for held in keys[..index].iter().rev() {
                    let _ = enigo.key(*held, Direction::Release);
                }
                return Err(input_failed("key press", error));
            }
        }
        Ok(())
    }

    fn release_chord(enigo: &mut Enigo, keys: &[enigo::Key]) -> Result<(), ComputerUseError> {
        for key in keys.iter().rev() {
            enigo
                .key(*key, Direction::Release)
                .map_err(|error| input_failed("key release", error))?;
        }
        Ok(())
    }
}

impl ComputerUseBackend for LinuxComputerUseBackend {
    fn capabilities(&self) -> Capabilities {
        let ui_tree = self.a11y.is_some();
        if self.is_wayland() {
            let portal_ok = self.wayland_portal.is_some();
            let screenshot_note = if portal_ok {
                "screenshot via the same-session ScreenCast stream (PipeWire; the first \
                 screenshot or input action opens one system authorization dialog)"
                    .to_string()
            } else if self.wayland_screenshot_ok {
                "screenshot via the xcap GNOME-Shell/portal/wlroots fallback chain \
                 (the portal path may show a compositor permission dialog per capture)"
                    .to_string()
            } else {
                format!(
                    "screenshot unavailable: {}",
                    self.wayland_screenshot_error
                        .as_deref()
                        .unwrap_or("probe failed")
                )
            };
            let input_note = match (&self.wayland_portal, &self.wayland_portal_error) {
                (Some(_), _) => "input via xdg-desktop-portal RemoteDesktop (the first \
                                 screenshot or input action opens a system authorization \
                                 dialog)"
                    .to_string(),
                (None, Some(error)) => format!("input unavailable: {error}"),
                (None, None) => "input unavailable".to_string(),
            };
            Capabilities {
                screenshot: portal_ok || self.wayland_screenshot_ok,
                input: self.wayland_portal.is_some(),
                ui_tree,
                notes: format!(
                    "Wayland session ({}): {input_note}; {screenshot_note}; \
                     AT-SPI bounds best-effort on Wayland",
                    self.session.desktop_label()
                ),
            }
        } else {
            let input_note = if self.input.is_some() {
                "XTEST input ready".to_string()
            } else {
                format!(
                    "input unavailable: {}",
                    self.input_init_error.as_deref().unwrap_or("unknown error")
                )
            };
            Capabilities {
                screenshot: true,
                input: self.input.is_some(),
                ui_tree,
                notes: format!(
                    "X11 session ({}): full support (xcap capture, {input_note}, AT-SPI tree{})",
                    self.session.desktop_label(),
                    if ui_tree { "" } else { " unavailable" },
                ),
            }
        }
    }

    fn capture(&mut self) -> Result<Capture, ComputerUseError> {
        // Wayland 首选:portal 同会话截屏流(与输入共用一次授权,无逐次弹窗)。
        if self.is_wayland() {
            if let Some(capture) = self.wayland_portal_capture() {
                return Ok(capture);
            }
        }
        // 回退:xcap 链(X11 主路径;Wayland 上探测失败可在后续截屏时重试,
        // 评审发现:旧实现一次探测失败就粘死整个 backend 生命周期——portal
        // 对话框被用户误关、合成器短暂抖动都会永久失去截屏,且无任何重试入口)。
        self.ensure_wayland_capture()?;
        let monitors = Monitor::all().map_err(|error| {
            ComputerUseError::unavailable(format!("monitor enumeration: {error}"))
        })?;
        // X11 下 xcap 用 Xft.dpi/96 作 scale 并把 RandR 几何除以它;截图与
        // XTEST 都在根窗口物理像素平面,故输入倍率恒 1.0,origin 乘回 scale。
        let scale = monitors
            .iter()
            .find_map(|m| m.scale_factor().ok())
            .filter(|s| *s > 0.0)
            .unwrap_or(1.0);
        let cursor = self.input.as_mut().and_then(|enigo| enigo.location().ok());
        let contains = |monitor: &Monitor, lx: i32, ly: i32| -> bool {
            match (monitor.x(), monitor.y(), monitor.width(), monitor.height()) {
                (Ok(x), Ok(y), Ok(w), Ok(h)) => {
                    lx >= x && lx < x + w as i32 && ly >= y && ly < y + h as i32
                }
                _ => false,
            }
        };
        let monitor = cursor
            .and_then(|(cx, cy)| {
                // 游标为根窗口像素;换算到 xcap 的逻辑坐标再做包含测试。
                let lx = (cx as f32 / scale) as i32;
                let ly = (cy as f32 / scale) as i32;
                monitors.iter().find(|m| contains(m, lx, ly)).cloned()
            })
            .or_else(|| {
                monitors
                    .iter()
                    .find(|m| m.is_primary().unwrap_or(false))
                    .cloned()
            })
            .or_else(|| monitors.first().cloned())
            .ok_or_else(|| {
                ComputerUseError::unavailable("no monitors reported by the display server")
            })?;
        let image = monitor.capture_image().map_err(|error| {
            if self.is_wayland() {
                ComputerUseError::unsupported(
                    "screenshot",
                    format!(
                        "screenshot on Wayland compositor {}: capture failed: {error}",
                        self.session.desktop_label()
                    ),
                )
            } else {
                ComputerUseError::unavailable(format!("monitor capture failed: {error}"))
            }
        })?;
        let width = image.width();
        let height = image.height();
        // X11:xcap 报告逻辑原点,乘回 scale 得根窗口物理像素(=输入空间,
        // 输入倍率 1.0)。Wayland:portal 输入在 stream 逻辑坐标空间,与 xcap
        // 的逻辑几何同空间,origin 不乘 scale,输入倍率取 1/scale(见
        // wayland_portal 模块文档)。
        let (origin_x, origin_y, input_scale_x, input_scale_y) = if self.is_wayland() {
            let scale = f64::from(scale);
            let origin_x = monitor.x().unwrap_or(0);
            let origin_y = monitor.y().unwrap_or(0);
            (origin_x, origin_y, 1.0 / scale, 1.0 / scale)
        } else {
            let origin_x = monitor
                .x()
                .map(|x| (x as f32 * scale).round() as i32)
                .unwrap_or(0);
            let origin_y = monitor
                .y()
                .map(|y| (y as f32 * scale).round() as i32)
                .unwrap_or(0);
            (origin_x, origin_y, 1.0, 1.0)
        };
        Ok(Capture {
            rgba: image.into_raw(),
            width,
            height,
            origin_x,
            origin_y,
            input_scale_x,
            input_scale_y,
        })
    }

    fn cursor_position(&mut self) -> Result<(i32, i32), ComputerUseError> {
        if self.is_wayland() {
            return match self.require_portal()?.last_pointer() {
                Some(position) => Ok(position),
                None => Err(ComputerUseError::unsupported(
                    "cursor_position",
                    "Wayland exposes no cursor query API; the position becomes known after \
                     the first mouse_move of an authorized portal session",
                )),
            };
        }
        let enigo = self.require_enigo()?;
        enigo
            .location()
            .map_err(|error| input_failed("cursor position query", error))
    }

    fn move_to(&mut self, x: i32, y: i32) -> Result<(), ComputerUseError> {
        if self.is_wayland() {
            self.note_input();
            let portal = self.require_portal()?;
            portal.ensure_started()?;
            portal.motion_absolute(x, y)?;
            portal.track_pointer(x, y);
            return Ok(());
        }
        let enigo = self.require_enigo()?;
        enigo
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|error| input_failed("mouse move", error))
    }

    fn click(&mut self, button: MouseButton, count: u8) -> Result<(), ComputerUseError> {
        let count = count.max(1);
        if self.is_wayland() {
            self.note_input();
            let portal = self.require_portal()?;
            portal.ensure_started()?;
            let evdev = wayland_portal::map_button(button);
            // 移动与点击之间留静置窗口(与 XTEST 同样的注入竞态缓冲)。
            settle();
            for i in 0..count {
                portal.button(evdev, true)?;
                portal.button(evdev, false)?;
                if i + 1 < count {
                    sleep(Duration::from_millis(CLICK_GAP_MS));
                }
            }
            return Ok(());
        }
        let enigo = self.require_enigo()?;
        let button = map_enigo_button(button);
        // 移动与点击之间留静置窗口,降低 XTEST 注入竞态。
        settle();
        for i in 0..count {
            enigo
                .button(button, Direction::Click)
                .map_err(|error| input_failed("mouse click", error))?;
            if i + 1 < count {
                sleep(Duration::from_millis(CLICK_GAP_MS));
            }
        }
        Ok(())
    }

    fn mouse_down(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        if self.is_wayland() {
            self.note_input();
            let portal = self.require_portal()?;
            portal.ensure_started()?;
            return portal.button(wayland_portal::map_button(button), true);
        }
        let enigo = self.require_enigo()?;
        enigo
            .button(map_enigo_button(button), Direction::Press)
            .map_err(|error| input_failed("mouse down", error))
    }

    fn mouse_up(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
        if self.is_wayland() {
            self.note_input();
            let portal = self.require_portal()?;
            portal.ensure_started()?;
            return portal.button(wayland_portal::map_button(button), false);
        }
        let enigo = self.require_enigo()?;
        enigo
            .button(map_enigo_button(button), Direction::Release)
            .map_err(|error| input_failed("mouse up", error))
    }

    fn drag(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<(), ComputerUseError> {
        if self.is_wayland() {
            self.note_input();
            let portal = self.require_portal()?;
            portal.ensure_started()?;
            portal.motion_absolute(from.0, from.1)?;
            settle();
            portal.button(wayland_portal::map_button(MouseButton::Left), true)?;
            // 插值移动;无论中途成败,最后都必须释放按键。
            let mut result = Ok(());
            for step in 1..=DRAG_STEPS {
                let t = f64::from(step) / f64::from(DRAG_STEPS);
                let x = f64::from(from.0) + f64::from(to.0 - from.0) * t;
                let y = f64::from(from.1) + f64::from(to.1 - from.1) * t;
                if let Err(error) = portal.motion_absolute(x.round() as i32, y.round() as i32) {
                    result = Err(error);
                    break;
                }
                sleep(Duration::from_millis(DRAG_STEP_MS));
            }
            let release = portal.button(wayland_portal::map_button(MouseButton::Left), false);
            portal.track_pointer(to.0, to.1);
            result.and(release)?;
            return Ok(());
        }
        let enigo = self.require_enigo()?;
        enigo
            .move_mouse(from.0, from.1, Coordinate::Abs)
            .map_err(|error| input_failed("drag: move to start", error))?;
        settle();
        enigo
            .button(Button::Left, Direction::Press)
            .map_err(|error| input_failed("drag: button press", error))?;
        // 插值移动;无论中途成败,最后都必须释放按键。
        let mut result = Ok(());
        for step in 1..=DRAG_STEPS {
            let t = f64::from(step) / f64::from(DRAG_STEPS);
            let x = f64::from(from.0) + f64::from(to.0 - from.0) * t;
            let y = f64::from(from.1) + f64::from(to.1 - from.1) * t;
            if let Err(error) =
                enigo.move_mouse(x.round() as i32, y.round() as i32, Coordinate::Abs)
            {
                result = Err(input_failed("drag: interpolated move", error));
                break;
            }
            sleep(Duration::from_millis(DRAG_STEP_MS));
        }
        let release = enigo
            .button(Button::Left, Direction::Release)
            .map_err(|error| input_failed("drag: button release", error));
        result.and(release)
    }

    fn scroll(&mut self, direction: ScrollDirection, clicks: u32) -> Result<(), ComputerUseError> {
        if self.is_wayland() {
            self.note_input();
            let portal = self.require_portal()?;
            portal.ensure_started()?;
            // 一次离散滚轮事件可携带多格(合成器内部逐格注入)。
            let (axis, steps) = wayland_portal::map_discrete_scroll(direction, clicks);
            return portal.axis_discrete(axis, steps);
        }
        let enigo = self.require_enigo()?;
        // 显式用滚轮按钮而非 Mouse::scroll():后者符号约定因平台而异
        // (x11rb 正数=向下),按钮循环语义无歧义。
        let button = match direction {
            ScrollDirection::Up => Button::ScrollUp,
            ScrollDirection::Down => Button::ScrollDown,
            ScrollDirection::Left => Button::ScrollLeft,
            ScrollDirection::Right => Button::ScrollRight,
        };
        for i in 0..clicks {
            enigo
                .button(button, Direction::Click)
                .map_err(|error| input_failed("scroll", error))?;
            if i + 1 < clicks {
                sleep(Duration::from_millis(SCROLL_GAP_MS));
            }
        }
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<(), ComputerUseError> {
        if self.is_wayland() {
            self.note_input();
            let portal = self.require_portal()?;
            portal.ensure_started()?;
            // 逐字符 keysym 注入(\n→Return、\t→Tab;不可映射字符显式报错)。
            let keysyms = text
                .chars()
                .map(wayland_portal::char_keysym)
                .collect::<Result<Vec<_>, _>>()?;
            for keysym in keysyms {
                portal.keysym_event(keysym, true)?;
                portal.keysym_event(keysym, false)?;
            }
            return Ok(());
        }
        let enigo = self.require_enigo()?;
        // enigo text() 走 Unicode 注入(X11 临时 keycode 重映射,xdotool 同款
        // 技巧),中文等字符直接进入焦点字段,不经 IME 合成。
        enigo
            .text(text)
            .map_err(|error| input_failed("type text", error))
    }

    fn key_chord(&mut self, keys: &[Key]) -> Result<(), ComputerUseError> {
        if self.is_wayland() {
            self.note_input();
            let portal = self.require_portal()?;
            let mapped = keys
                .iter()
                .map(|key| wayland_portal::map_keysym(*key))
                .collect::<Result<Vec<_>, _>>()?;
            portal.ensure_started()?;
            for (index, keysym) in mapped.iter().enumerate() {
                if let Err(error) = portal.keysym_event(*keysym, true) {
                    // 错误路径上释放已按下的键,避免修饰键卡死。
                    for held in mapped[..index].iter().rev() {
                        let _ = portal.keysym_event(*held, false);
                    }
                    return Err(error);
                }
            }
            for keysym in mapped.iter().rev() {
                portal.keysym_event(*keysym, false)?;
            }
            return Ok(());
        }
        let mapped = keys
            .iter()
            .map(|key| map_enigo_key(*key))
            .collect::<Result<Vec<_>, _>>()?;
        let enigo = self.require_enigo()?;
        Self::press_chord(enigo, &mapped)?;
        Self::release_chord(enigo, &mapped)
    }

    fn hold_key(&mut self, keys: &[Key], ms: u64) -> Result<(), ComputerUseError> {
        if self.is_wayland() {
            self.note_input();
            let portal = self.require_portal()?;
            let mapped = keys
                .iter()
                .map(|key| wayland_portal::map_keysym(*key))
                .collect::<Result<Vec<_>, _>>()?;
            portal.ensure_started()?;
            for keysym in &mapped {
                portal.keysym_event(*keysym, true)?;
            }
            sleep(Duration::from_millis(ms));
            for keysym in mapped.iter().rev() {
                portal.keysym_event(*keysym, false)?;
            }
            return Ok(());
        }
        let mapped = keys
            .iter()
            .map(|key| map_enigo_key(*key))
            .collect::<Result<Vec<_>, _>>()?;
        let enigo = self.require_enigo()?;
        Self::press_chord(enigo, &mapped)?;
        sleep(Duration::from_millis(ms));
        Self::release_chord(enigo, &mapped)
    }

    fn ui_tree(&mut self, opts: &UiTreeOptions) -> Result<String, ComputerUseError> {
        let a11y = self.require_a11y()?;
        let wayland = self.is_wayland();
        let desktop = self.session.desktop_label();
        self.runtime
            .block_on(ui_tree_async(a11y, opts, wayland, desktop))
    }

    fn element_at_point(
        &mut self,
        x: i32,
        y: i32,
    ) -> Result<Option<ElementInfo>, ComputerUseError> {
        let a11y = self.require_a11y()?;
        self.runtime.block_on(element_at_point_async(a11y, x, y))
    }
}

pub(super) fn create_backend() -> Result<Box<dyn ComputerUseBackend>, ComputerUseError> {
    let session = detect_session(&|key| std::env::var(key).ok());
    if session.kind == SessionKind::NoDisplay {
        return Err(ComputerUseError::unsupported(
            "computer_use",
            format!(
                "no graphical session reachable (XDG_SESSION_TYPE={:?}, WAYLAND_DISPLAY set: {}, \
                 DISPLAY set: {}); computer use needs an X11 or Wayland desktop session",
                session.session_type, session.has_wayland_display, session.has_display
            ),
        ));
    }
    let wayland = session.kind == SessionKind::Wayland;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            ComputerUseError::unavailable(format!(
                "cannot create tokio runtime for AT-SPI: {error}"
            ))
        })?;
    let (a11y, a11y_init_error) = match runtime.block_on(a11y_init()) {
        Ok(conn) => (Some(conn), None),
        Err(error) => (None, Some(error)),
    };

    // Wayland 不构造 enigo:XTEST 经 XWayland 只能触达 X11 客户端,且
    // enigo 的 wayland/libei 后端均为实验性——输入走 portal RemoteDesktop。
    let (input, input_init_error) = if wayland {
        (None, None)
    } else {
        match Enigo::new(&Settings::default()) {
            Ok(enigo) => (Some(enigo), None),
            Err(error) => (None, Some(error.to_string())),
        }
    };

    // Wayland 输入:探测 portal 的 RemoteDesktop 支持(纯属性查询,不弹窗);
    // 探测失败记为粘性错误,输入动作显式不可用。
    let (wayland_portal, wayland_portal_error) = if wayland {
        match PortalInput::probe() {
            Ok(()) => match PortalInput::new() {
                Ok(portal) => (Some(portal), None),
                Err(error) => (None, Some(error.to_string())),
            },
            Err(detail) => (None, Some(detail)),
        }
    } else {
        (None, None)
    };

    let (wayland_screenshot_ok, wayland_screenshot_error) = if wayland {
        match probe_wayland_screenshot_guarded() {
            Ok(()) => (true, None),
            Err(error) => (false, Some(error)),
        }
    } else {
        (true, None)
    };

    Ok(Box::new(LinuxComputerUseBackend {
        session,
        runtime,
        a11y,
        a11y_init_error,
        input,
        input_init_error,
        wayland_portal,
        wayland_portal_error,
        wayland_screenshot_ok,
        wayland_screenshot_error,
        wayland_portal_capture_error: None,
        last_input_at: None,
    }))
}

/// backend 在 worker 线程上析构(Shutdown):portal 授权授予的会话在这里
/// 尽力关闭,不跨进程泄漏。
impl Drop for LinuxComputerUseBackend {
    fn drop(&mut self) {
        if let Some(portal) = self.wayland_portal.as_mut() {
            portal.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn detect(pairs: &[(&str, &str)]) -> SessionInfo {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        detect_session(&|key| map.get(key).cloned())
    }

    #[test]
    fn session_type_wayland_wins_over_display() {
        // DISPLAY 在 XWayland 下同样设置,绝不能把 Wayland 会话误判成 X11。
        let session = detect(&[
            ("XDG_SESSION_TYPE", "wayland"),
            ("DISPLAY", ":0"),
            ("WAYLAND_DISPLAY", "wayland-0"),
            ("XDG_CURRENT_DESKTOP", "GNOME"),
        ]);
        assert_eq!(session.kind, SessionKind::Wayland);
        assert_eq!(session.desktop_label(), "GNOME");
    }

    #[test]
    fn session_type_x11_selects_x11() {
        let session = detect(&[("XDG_SESSION_TYPE", "x11"), ("DISPLAY", ":0")]);
        assert_eq!(session.kind, SessionKind::X11);
    }

    #[test]
    fn session_type_tty_means_no_display() {
        let session = detect(&[("XDG_SESSION_TYPE", "tty")]);
        assert_eq!(session.kind, SessionKind::NoDisplay);
    }

    #[test]
    fn wayland_display_with_runtime_dir_corroborates_wayland() {
        let session = detect(&[
            ("WAYLAND_DISPLAY", "wayland-0"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("DISPLAY", ":0"),
        ]);
        assert_eq!(session.kind, SessionKind::Wayland);
    }

    #[test]
    fn wayland_display_alone_still_wayland() {
        let session = detect(&[("WAYLAND_DISPLAY", "wayland-1")]);
        assert_eq!(session.kind, SessionKind::Wayland);
    }

    #[test]
    fn display_without_wayland_signals_is_x11() {
        let session = detect(&[("DISPLAY", ":0")]);
        assert_eq!(session.kind, SessionKind::X11);
    }

    #[test]
    fn unknown_session_type_falls_back_to_heuristics() {
        let session = detect(&[("XDG_SESSION_TYPE", "mir"), ("DISPLAY", ":0")]);
        assert_eq!(session.kind, SessionKind::X11);
    }

    #[test]
    fn session_type_is_case_insensitive_and_trimmed() {
        let session = detect(&[("XDG_SESSION_TYPE", " Wayland ")]);
        assert_eq!(session.kind, SessionKind::Wayland);
    }

    #[test]
    fn empty_environment_is_no_display() {
        let session = detect(&[]);
        assert_eq!(session.kind, SessionKind::NoDisplay);
        let blank = detect(&[("XDG_SESSION_TYPE", ""), ("DISPLAY", "  ")]);
        assert_eq!(blank.kind, SessionKind::NoDisplay);
    }

    #[test]
    fn key_mapping_covers_named_keys() {
        assert_eq!(map_enigo_key(Key::Enter).ok(), Some(enigo::Key::Return));
        assert_eq!(map_enigo_key(Key::Control).ok(), Some(enigo::Key::Control));
        assert_eq!(map_enigo_key(Key::Meta).ok(), Some(enigo::Key::Meta));
        assert_eq!(map_enigo_key(Key::Up).ok(), Some(enigo::Key::UpArrow));
        assert_eq!(
            map_enigo_key(Key::PageDown).ok(),
            Some(enigo::Key::PageDown)
        );
        assert_eq!(
            map_enigo_key(Key::Char('s')).ok(),
            Some(enigo::Key::Unicode('s'))
        );
        assert_eq!(
            map_enigo_key(Key::Char('中')).ok(),
            Some(enigo::Key::Unicode('中'))
        );
    }

    #[test]
    fn key_mapping_function_keys_f1_to_f20() {
        assert_eq!(map_enigo_key(Key::Function(1)).ok(), Some(enigo::Key::F1));
        assert_eq!(map_enigo_key(Key::Function(12)).ok(), Some(enigo::Key::F12));
        assert_eq!(map_enigo_key(Key::Function(20)).ok(), Some(enigo::Key::F20));
        assert!(map_enigo_key(Key::Function(21)).is_err());
        assert!(map_enigo_key(Key::Function(0)).is_err());
    }

    #[test]
    fn sanitize_name_strips_quotes_and_truncates() {
        assert_eq!(sanitize_name("say \"hi\"\nnow"), "say 'hi' now");
        let long = "x".repeat(MAX_NAME_CHARS + 20);
        assert_eq!(sanitize_name(&long).chars().count(), MAX_NAME_CHARS);
    }
}

#[cfg(test)]
mod wayland_e2e_tests {
    //! 真机 Wayland E2E(dialog → grant → 同会话截屏 → move/click/type)。
    //! 需要真实 Wayland 会话 + xdg-desktop-portal(RemoteDesktop/ScreenCast),
    //! 且系统授权对话框须被确认——验证环境用 root 的 uinput 脚本模拟用户按
    //! Enter(见 PR 描述的验证章节)。默认 ignored:
    //! `cargo test --lib computer_use::platform::linux::wayland_e2e_tests -- --ignored --nocapture`

    use super::*;
    use atspi::proxy::text::TextProxy;

    /// DFS 收集所有 role=Text 节点的文本(a11y 验证打字结果)。
    async fn read_texts_via_a11y(
        conn: &AccessibilityConnection,
    ) -> Result<Vec<String>, ComputerUseError> {
        let mut texts = Vec::new();
        walk_texts(
            conn,
            conn.root_accessible_on_registry()
                .await
                .map_err(|e| ComputerUseError::unavailable(format!("a11y root: {e}")))?,
            18,
            &mut texts,
        )
        .await?;
        Ok(texts)
    }

    async fn walk_texts(
        conn: &AccessibilityConnection,
        proxy: AccessibleProxy<'_>,
        depth: u32,
        out: &mut Vec<String>,
    ) -> Result<(), ComputerUseError> {
        if depth == 0 || out.len() >= 64 {
            return Ok(());
        }
        let role = proxy.get_role().await.unwrap_or(Role::Unknown);
        if role == Role::Text {
            let text = TextProxy::builder(conn.connection())
                .destination(proxy.inner().destination().as_str().to_string())
                .map_err(|e| ComputerUseError::failed(e.to_string()))?
                .path(proxy.inner().path().as_str().to_string())
                .map_err(|e| ComputerUseError::failed(e.to_string()))?
                .build()
                .await
                .map_err(|e| ComputerUseError::failed(format!("text proxy: {e}")))?;
            let content = text.get_text(0, -1).await.unwrap_or_default();
            if !content.trim().is_empty() {
                out.push(content);
            }
        }
        if let Ok(children) = proxy.get_children().await {
            for child in children {
                if child.is_null() {
                    continue;
                }
                if let Ok(child) = child.into_accessible_proxy(conn.connection()).await {
                    Box::pin(walk_texts(conn, child, depth - 1, out)).await?;
                }
            }
        }
        Ok(())
    }

    /// 临时诊断:把收到的帧存为 PNG(验证环境用)。
    fn save_debug_png(capture: &Capture, index: i32) {
        let path = format!("/tmp/pinvou-wl-e2e/frame-{index}.png");
        let image =
            xcap::image::RgbaImage::from_raw(capture.width, capture.height, capture.rgba.clone());
        if let Some(image) = image {
            let _ = xcap::image::DynamicImage::ImageRgba8(image).save(std::path::Path::new(&path));
        }
    }

    /// 两帧 RGBA 的差分包围盒(无差分返回 None)。步长 4 像素采样,足够定位
    /// 窗口级包围盒且省时。
    fn diff_bounding_box(a: &[u8], b: &[u8]) -> Option<(i32, i32, u32, u32)> {
        assert_eq!(a.len(), b.len());
        let width = ((a.len() / 4) as f64).sqrt() as usize;
        let height = (a.len() / 4) / width;
        let mut x0 = usize::MAX;
        let mut y0 = usize::MAX;
        let mut x1 = 0;
        let mut y1 = 0;
        for y in (0..height).step_by(4) {
            for x in (0..width).step_by(4) {
                let i = (y * width + x) * 4;
                if a[i..i + 3] != b[i..i + 3] {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        if x0 == usize::MAX {
            return None;
        }
        Some((
            x0 as i32,
            y0 as i32,
            (x1 - x0 + 4) as u32,
            (y1 - y0 + 4) as u32,
        ))
    }

    #[test]
    #[ignore = "needs a live Wayland session with xdg-desktop-portal and the system \
                authorization dialog confirmed (uinput Enter in the verification env)"]
    fn e2e_dialog_grant_capture_and_input() {
        let session = detect_session(&|key| std::env::var(key).ok());
        assert_eq!(
            session.kind,
            SessionKind::Wayland,
            "run inside a live Wayland session"
        );

        // backend 构造即建立 AT-SPI 连接(a11y 总线),之后启动的目标应用才
        // 能注册到 a11y 树上。
        let mut backend = create_backend().expect("backend on a live Wayland session");
        let caps = backend.capabilities();
        println!(
            "capabilities: screenshot={} input={} ui_tree={}\n  {}",
            caps.screenshot, caps.input, caps.ui_tree, caps.notes
        );
        assert!(
            caps.screenshot,
            "portal same-session capture must be available"
        );
        assert!(caps.input, "portal input must be available");

        // 1) 首次截屏:建立 portal 会话并弹系统授权对话框(由验证环境的
        //    对话框脚本确认),截屏帧来自同会话 ScreenCast 流。
        let shot1 = backend
            .capture()
            .expect("first capture establishes the portal session (dialog must be granted)");
        println!(
            "capture #1: {}x{} origin=({},{}) input_scale={}",
            shot1.width, shot1.height, shot1.origin_x, shot1.origin_y, shot1.input_scale_x
        );
        assert_eq!((shot1.origin_x, shot1.origin_y), (0, 0));
        assert_eq!(shot1.input_scale_x, 1.0);
        assert_eq!(
            shot1.rgba.len(),
            shot1.width as usize * shot1.height as usize * 4
        );
        assert!(shot1.rgba.iter().any(|byte| *byte != 0), "frame not blank");

        // 2) 输入目标:GNOME Shell 顶栏时钟(位置已知:顶栏中央)。
        //    Wayland 下 AT-SPI extents 不可靠(全 0),computer-use 的正路就是
        //    看截图:点击后用像素差分验证 UI 真的响应了。
        let shot_w = shot1.width;
        let shot_h = shot1.height;
        let clock = (i64::from(shot_w) / 2, 8);
        backend
            .move_to(clock.0 as i32, clock.1)
            .expect("mouse_move");
        assert_eq!(
            backend.cursor_position().ok(),
            Some((clock.0 as i32, clock.1)),
            "cursor_position tracks our injected move"
        );
        backend.click(MouseButton::Left, 1).expect("click");

        // 3) 等待比点击新的帧:日历/通知下拉必须出现在屏幕上半部。
        let dropdown = wait_for_big_change(&mut backend, &shot1, 40, shot_h / 2)
            .expect("clicking the clock must open the calendar dropdown");
        println!("calendar dropdown box: {dropdown:?}");

        // 4) 键盘链路:Escape 关闭下拉,Super 打开概览(搜索框自动聚焦)。
        backend.key_chord(&[Key::Escape]).expect("escape key chord");
        std::thread::sleep(Duration::from_millis(600));
        let baseline = backend.capture().expect("capture before overview");
        backend.key_chord(&[Key::Meta]).expect("super key chord");

        // 5) 打字进 shell 搜索框:概览稳定后输入 "fire",搜索结果(Firefox)
        //    出现 = 按键注入真实到达 shell 搜索框(相对概览基线的第二次
        //    窗口级差分)。
        let typed = "fire";
        let mut typed_shot = None;
        for i in 0..40 {
            std::thread::sleep(Duration::from_millis(400));
            if i == 6 {
                backend.type_text(typed).expect("type_text");
            }
            let Ok(shot) = backend.capture() else {
                continue;
            };
            if i >= 8 {
                if let Some(box_) = diff_bounding_box(&baseline.rgba, &shot.rgba) {
                    if box_.2 > 200 && box_.3 > 200 {
                        typed_shot = Some(box_);
                        break;
                    }
                }
            }
        }
        println!("search results diff box: {typed_shot:?}");
        assert!(
            typed_shot.is_some(),
            "search results must appear after typing into the overview search"
        );

        // 6) a11y 文本读回(嵌套 shell 的 cally 桥在启动时未开,树可能很浅,
        //    仅作诊断输出,不作断言)。
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let conn = runtime.block_on(a11y_init()).expect("a11y connection");
        let texts = runtime
            .block_on(read_texts_via_a11y(&conn))
            .expect("read texts via a11y");
        println!(
            "a11y texts ({} entries, first 3): {:?}",
            texts.len(),
            texts
                .iter()
                .take(3)
                .map(|t| t.chars().take(80).collect::<String>())
                .collect::<Vec<_>>()
        );
    }

    /// 反复截图直到与 `baseline` 出现窗口级差分(忽略 `ignore_prefixes` 里
    /// 以这些前缀开头的固定小变化区域),返回差分包围盒。
    fn wait_for_big_change(
        backend: &mut Box<dyn ComputerUseBackend>,
        baseline: &Capture,
        rounds: usize,
        max_y: u32,
    ) -> Option<(i32, i32, u32, u32)> {
        for _ in 0..rounds {
            std::thread::sleep(Duration::from_millis(400));
            let Ok(shot) = backend.capture() else {
                continue;
            };
            if shot.width != baseline.width || shot.height != baseline.height {
                continue;
            }
            if let Some(box_) = diff_bounding_box(&baseline.rgba, &shot.rgba) {
                if box_.2 > 200 && box_.3 > 200 && (box_.1 as u32) < max_y {
                    return Some(box_);
                }
            }
        }
        None
    }
}
