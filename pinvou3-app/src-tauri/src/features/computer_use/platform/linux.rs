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
//!   a11y 总线连接由本模块自建(`zbus::connection::Builder` +
//!   `method_timeout`):atspi 的 `AccessibilityConnection` 不允许注入自建
//!   connection(`new`/`from_address` 内部各自 build,无超时设置点),而 zbus
//!   默认超时很宽,树遍历每节点多次调用,一个挂死的 app 曾能永久钉死
//!   worker——故绕开该薄封装,直接用 atspi 的 proxy 类型 + 自管连接,并在
//!   操作层再加一道整体 deadline 兜底。trait 为同步而 atspi 为 async:
//!   backend 持有一个专用 current-thread tokio runtime,在 worker 线程(普通
//!   std::thread,不含引擎主 runtime)上 `block_on`,无嵌套运行时死锁风险。
//!   Wayland 下 Component extents 为尽力而为(合成器/工具包相关),ui_tree
//!   输出头部注明。
//! - 线程约定同其他平台:对象于 worker 线程构造,不得跨线程移动。

use std::future::Future;
use std::thread::sleep;
use std::time::{Duration, Instant};

use atspi::proxy::accessible::{AccessibleProxy, ObjectRefExt};
use atspi::proxy::bus::BusProxy;
use atspi::proxy::component::ComponentProxy;
use atspi::{CoordType, Role, State, StateSet};
use enigo::{Button, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};
use xcap::Monitor;
use zbus::proxy::CacheProperties;

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
/// a11y D-Bus 单次方法调用的超时(zbus connection 级 `method_timeout`)。
/// 评审发现:zbus 默认超时很宽,树遍历每节点 4-5 次调用,一个挂死的 app
/// 即可让 worker 永久 pending。
const A11Y_METHOD_TIMEOUT: Duration = Duration::from_secs(3);
/// 单点 a11y 查询(`element_at_point`/`focused_element`)的操作级 deadline:
/// 方法级 3s 之上的第二层兜底,防"每步都快但总时长失控"。
const A11Y_POINT_DEADLINE: Duration = Duration::from_secs(6);
/// ui_tree 全树遍历的操作级 deadline(节点数有上限但每节点多次调用;20s
/// 覆盖健康桌面,挂死环境快速失败,保持 worker 响应)。
const A11Y_TREE_DEADLINE: Duration = Duration::from_secs(20);
/// focused_element 树搜索的节点预算(与操作级 deadline 双约束)。
const FOCUSED_SEARCH_MAX_NODES: usize = 300;

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

/// 拖拽收尾错误合并（评审发现：移动与释放**双双失败**时，旧实现只向上报
/// 移动错误——调用方永远不知道左键还卡在按下状态）。两败俱伤时显式指出
/// 按键可能未释放。
fn combine_drag_errors(
    move_result: Result<(), ComputerUseError>,
    release_result: Result<(), ComputerUseError>,
) -> Result<(), ComputerUseError> {
    match (move_result, release_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(error)) => Err(error),
        (Err(error), Ok(())) => Err(error),
        (Err(move_error), Err(release_error)) => Err(ComputerUseError::failed(format!(
            "{move_error}; additionally the drag release failed ({release_error}) — \
             the left mouse button may still be pressed"
        ))),
    }
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

/// secure 判定(纯函数):`PasswordText` 直接判定;**角色查询失败**
/// (`role_unknown`)保守兜底为 secure——无法证明不是密码框,宁可让工具层
/// 多要一次确认,也不能把密码框当普通元素放行(评审发现)。
fn is_secure_role(role: Role, role_unknown: bool) -> bool {
    role == Role::PasswordText || role_unknown
}

/// 节点标志位(紧凑单行输出用)。`secure` 由调用方按 [`is_secure_role`]
/// 给出;此时调用方须同时抹除 name(见 `write_node`/`element_info_of`)。
fn state_flags(secure: bool, state: Option<StateSet>) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if secure {
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

/// a11y 注册表根的 AccessibleProxy。复制 atspi
/// `AccessibilityConnection::root_accessible_on_registry` 的构造要点:
/// registry 对 DBus 属性接口实现不完整,属性缓存必须显式关闭。
async fn root_accessible(conn: &zbus::Connection) -> Result<AccessibleProxy<'_>, ComputerUseError> {
    AccessibleProxy::builder(conn)
        .destination("org.a11y.atspi.Registry")
        .map_err(|error| {
            ComputerUseError::unavailable(format!("AT-SPI registry destination: {error}"))
        })?
        .cache_properties(CacheProperties::No)
        .build()
        .await
        .map_err(|error| ComputerUseError::unavailable(format!("AT-SPI registry root: {error}")))
}

/// 由 AccessibleProxy 盲建同对象的 ComponentProxy(不对每个节点先查
/// GetInterfaces,避免一次额外 D-Bus 往返;不支持 Component 的对象在
/// get_extents 时报错,按"无 bounds"处理)。
async fn component_of<'c>(
    conn: &'c zbus::Connection,
    proxy: &AccessibleProxy<'_>,
) -> Option<ComponentProxy<'c>> {
    ComponentProxy::builder(conn)
        .destination(proxy.inner().destination().as_str().to_string())
        .ok()?
        .path(proxy.inner().path().as_str().to_string())
        .ok()?
        .build()
        .await
        .ok()
}

async fn screen_extents(
    conn: &zbus::Connection,
    proxy: &AccessibleProxy<'_>,
) -> Option<(i32, i32, i32, i32)> {
    component_of(conn, proxy)
        .await?
        .get_extents(CoordType::Screen)
        .await
        .ok()
}

/// 严格版 extents:命中测试要靠窗口 extents 判定"点是否在此窗口内",
/// 查询失败必须上抛(工具层 T3 筛查按 Unscreenable 失败关闭),不能
/// continue 成"窗口不覆盖该点"(评审发现的 fail-open)。
async fn screen_extents_strict(
    conn: &zbus::Connection,
    proxy: &AccessibleProxy<'_>,
) -> Result<(i32, i32, i32, i32), ComputerUseError> {
    let component = component_of(conn, proxy).await.ok_or_else(|| {
        ComputerUseError::unavailable("AT-SPI Component proxy unavailable for window")
    })?;
    let extents = component
        .get_extents(CoordType::Screen)
        .await
        .map_err(|error| {
            ComputerUseError::unavailable(format!("AT-SPI window extents: {error}"))
        })?;
    // 「成功但零尺寸」无法判定覆盖关系:Wayland 上的 AT-SPI 普遍把 extents
    // 报告为全 0(见模块底部 e2e 注释),若放行会让**每个**窗口都被判
    // 「不覆盖该点」→ Ok(None) → 工具层 Clear,指针类 T3 筛查整体空转
    // (第二轮评审发现的 fail-open)。按查询失败上抛 → Unscreenable →
    // 要求确认,方向 fail-closed。
    if extents.2 <= 0 || extents.3 <= 0 {
        return Err(ComputerUseError::unavailable(format!(
            "AT-SPI window extents are empty ({},{},{},{}); the window exposes no usable \
             coordinate space (common on Wayland), so hit-testing cannot be trusted",
            extents.0, extents.1, extents.2, extents.3
        )));
    }
    Ok(extents)
}

/// 点是否落在窗口 extents 内(右/下边开区间)。纯函数,便于单元测试。
fn extents_contain(extents: (i32, i32, i32, i32), x: i32, y: i32) -> bool {
    let (wx, wy, ww, wh) = extents;
    x >= wx && x < wx + ww && y >= wy && y < wy + wh
}

/// 注册表根的全部应用顶层窗口(不区分活动与否)。
///
/// `strict = true`(element_at_point / focused_element 等筛查路径)时任何
/// 一层查询失败都向上报,绝不静默吞成"没有窗口";`strict = false`(ui_tree
/// 观察路径)保持尽力而为:单个 app 挂了就跳过,不拖垮整棵树。
async fn app_windows<'a>(
    conn: &'a zbus::Connection,
    root: &AccessibleProxy<'_>,
    strict: bool,
) -> Result<Vec<AccessibleProxy<'a>>, ComputerUseError> {
    let mut windows = Vec::new();
    let apps = match root.get_children().await {
        Ok(apps) => apps,
        Err(error) => {
            if strict {
                return Err(ComputerUseError::unavailable(format!(
                    "AT-SPI registry root children: {error}"
                )));
            }
            // 尽力而为路径:根 children 失败 → 空窗口表(树退化为浅层根)。
            return Ok(windows);
        }
    };
    for app_ref in apps {
        if app_ref.is_null() {
            continue;
        }
        let app = match app_ref.into_accessible_proxy(conn).await {
            Ok(app) => app,
            Err(error) => {
                if strict {
                    return Err(ComputerUseError::unavailable(format!(
                        "AT-SPI application proxy: {error}"
                    )));
                }
                continue;
            }
        };
        let children = match app.get_children().await {
            Ok(children) => children,
            Err(error) => {
                if strict {
                    return Err(ComputerUseError::unavailable(format!(
                        "AT-SPI application children: {error}"
                    )));
                }
                continue;
            }
        };
        for child_ref in children {
            if child_ref.is_null() {
                continue;
            }
            match child_ref.into_accessible_proxy(conn).await {
                Ok(window) => windows.push(window),
                Err(error) => {
                    if strict {
                        return Err(ComputerUseError::unavailable(format!(
                            "AT-SPI window proxy: {error}"
                        )));
                    }
                }
            }
        }
    }
    Ok(windows)
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

/// 由 AccessibleProxy 构造 [`ElementInfo`]。
///
/// secure 判定:PasswordText 直接判定;**角色查询失败**时保守兜底为 secure
/// ——无法证明不是密码框,宁可让工具层多要一次确认,也不能把密码框当普通
/// 元素放行;此时 name 一并抹除,避免密码内容经 ElementInfo 泄露。
/// `fallback_bounds`:extents 查询失败时的兜底 bounds(命中路径传命中点,
/// 搜索路径传 0;bounds 仅展示用,命中已完成)。
async fn element_info_of(
    conn: &zbus::Connection,
    proxy: &AccessibleProxy<'_>,
    fallback_bounds: (i32, i32, i32, i32),
) -> ElementInfo {
    let (role, role_unknown) = match proxy.get_role().await {
        Ok(role) => (role, false),
        Err(_) => (Role::Unknown, true),
    };
    let secure = is_secure_role(role, role_unknown);
    let name = if secure {
        String::new()
    } else {
        sanitize_name(&proxy.name().await.unwrap_or_default())
    };
    let (x, y, width, height) = screen_extents(conn, proxy).await.unwrap_or(fallback_bounds);
    ElementInfo {
        role: role.name().to_string(),
        name,
        x,
        y,
        width,
        height,
        secure,
    }
}

/// 紧凑文本树写出器:`[i] role "name" (x,y,w,h) flags`,深度/节点数双上限。
struct TreeWriter<'c> {
    conn: &'c zbus::Connection,
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

        // 角色查询失败按 Unknown 保守处理:secure 兜底 + name 抹除(评审
        // 发现:Unknown 角色无法证明不是密码框)。
        let (role, role_unknown) = match proxy.get_role().await {
            Ok(role) => (role, false),
            Err(_) => (Role::Unknown, true),
        };
        let secure = is_secure_role(role, role_unknown);
        // 密码框与角色不明的节点不在树里输出 name(评审发现:name 可能
        // 就是密码内容本身);顺带省一次 D-Bus 属性查询。
        let name = if secure {
            String::new()
        } else {
            sanitize_name(&proxy.name().await.unwrap_or_default())
        };
        let state = proxy.get_state().await.ok();
        let extents = screen_extents(self.conn, proxy).await;

        let indent = "  ".repeat(depth as usize);
        let mut line = format!("{indent}[{index}] {} \"{name}\"", role.name());
        if let Some((x, y, width, height)) = extents {
            line.push_str(&format!(" ({x},{y},{width},{height})"));
        }
        let flags = state_flags(secure, state);
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
            if let Ok(child) = child_ref.into_accessible_proxy(self.conn).await {
                Box::pin(self.write_node(&child, depth + 1)).await;
            }
        }
    }
}

async fn ui_tree_async(
    conn: &zbus::Connection,
    opts: &UiTreeOptions,
    wayland: bool,
    desktop: &str,
) -> Result<String, ComputerUseError> {
    let root = root_accessible(conn).await?;

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

    // 观察路径:尽力而为枚举(单个 app 挂了就跳过)。
    let mut windows = app_windows(conn, &root, false).await?;
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

/// 命中测试。两种结局语义分明(评审发现的 fail-open 修复):
/// - `Ok(None)`:**确认无元素**——枚举到的每个窗口都明确不覆盖该点,或
///   覆盖的窗口经 AT-SPI 明确返回空命中(null ObjectRef);
/// - `Err`:**查询失败**——根/应用/窗口枚举、extents、命中查询任何一环
///   挂掉都向上报;工具层 T3 筛查按 Unscreenable 失败关闭,绝不当作
///   "无元素"放行。
async fn element_at_point_async(
    conn: &zbus::Connection,
    x: i32,
    y: i32,
) -> Result<Option<ElementInfo>, ComputerUseError> {
    let root = root_accessible(conn).await?;
    let mut windows = app_windows(conn, &root, true).await?;
    active_first(&mut windows).await;
    for window in &windows {
        // extents 失败 → 无法判定覆盖关系 → 查询失败(不 continue)。
        let extents = screen_extents_strict(conn, window).await?;
        if !extents_contain(extents, x, y) {
            continue; // 明确不覆盖该点。
        }
        let component = component_of(conn, window).await.ok_or_else(|| {
            ComputerUseError::unavailable("AT-SPI Component proxy unavailable for window")
        })?;
        let target_ref = component
            .get_accessible_at_point(x, y, CoordType::Screen)
            .await
            .map_err(|error| {
                ComputerUseError::unavailable(format!("AT-SPI hit test at ({x}, {y}): {error}"))
            })?;
        if target_ref.is_null() {
            continue; // AT-SPI 明确空结果:该窗口内确认无元素。
        }
        let target = target_ref
            .into_accessible_proxy(conn)
            .await
            .map_err(|error| {
                ComputerUseError::unavailable(format!("AT-SPI target proxy: {error}"))
            })?;
        let info = element_info_of(conn, &target, (x, y, 0, 0)).await;
        return Ok(Some(info));
    }
    Ok(None)
}

/// 焦点元素。atspi 0.30 的 proxy 层没有 GetFocusedObject 类查询(焦点只能
/// 从事件流异步积累),故退而求其次:**在可达树上找 state 含 FOCUSED 的
/// 节点**——活动窗口优先,DFS,受 [`FOCUSED_SEARCH_MAX_NODES`] 节点预算与
/// 操作级 deadline 双约束(选择此路线而非返回 `Err(unsupported)`:树搜索
/// 在 X11/Wayland 下都可行,不该浪费已有的 AT-SPI 通路)。
///
/// 结局语义:搜完可达树没找到 → `Ok(None)`(确认无焦点元素:部分工具包
/// 不实现 FOCUSED state);预算耗尽仍无定论 → `Err`(fail-closed,结果
/// 不确定时不说"没有")。
async fn focused_element_async(
    conn: &zbus::Connection,
) -> Result<Option<ElementInfo>, ComputerUseError> {
    let root = root_accessible(conn).await?;
    let mut windows = app_windows(conn, &root, true).await?;
    active_first(&mut windows).await;
    let mut budget = FOCUSED_SEARCH_MAX_NODES;
    for window in &windows {
        if let Some(info) = find_focused_in_subtree(conn, window, &mut budget).await? {
            return Ok(Some(info));
        }
        if budget == 0 {
            break;
        }
    }
    if budget == 0 {
        return Err(ComputerUseError::unavailable(
            "focused element search exhausted its node budget without a definitive answer",
        ));
    }
    Ok(None)
}

/// 在子树内 DFS 找 state 含 FOCUSED 的节点;`budget` 限制访问节点数。
async fn find_focused_in_subtree(
    conn: &zbus::Connection,
    proxy: &AccessibleProxy<'_>,
    budget: &mut usize,
) -> Result<Option<ElementInfo>, ComputerUseError> {
    if *budget == 0 {
        return Ok(None);
    }
    *budget -= 1;
    let state = proxy
        .get_state()
        .await
        .map_err(|error| ComputerUseError::unavailable(format!("AT-SPI state query: {error}")))?;
    if state.contains(State::Focused) {
        return Ok(Some(element_info_of(conn, proxy, (0, 0, 0, 0)).await));
    }
    let children = proxy
        .get_children()
        .await
        .map_err(|error| ComputerUseError::unavailable(format!("AT-SPI node children: {error}")))?;
    for child_ref in children {
        if child_ref.is_null() {
            continue;
        }
        let child = child_ref
            .into_accessible_proxy(conn)
            .await
            .map_err(|error| {
                ComputerUseError::unavailable(format!("AT-SPI child proxy: {error}"))
            })?;
        if let Some(found) = Box::pin(find_focused_in_subtree(conn, &child, budget)).await? {
            return Ok(Some(found));
        }
        if *budget == 0 {
            return Ok(None);
        }
    }
    Ok(None)
}

/// AT-SPI 初始化:先打开会话 a11y 开关(Electron/Chromium 只在有 AT 注册后
/// 才构建无障碍树;该调用失败非致命),再**自建** a11y 总线连接。
///
/// 连接用 `zbus::connection::Builder` 构造并设 `method_timeout`(3s)。评审
/// 发现:zbus 默认超时很宽,而 atspi 的 `AccessibilityConnection` 不允许
/// 注入自建 connection——故按 atspi `AccessibilityConnection::new` 同款流程
/// 自行拿总线地址(`org.a11y.Bus.GetAddress`)建连接,直接使用 atspi 的
/// proxy 类型。(zbus 5 的 connection::Builder 只有 method_timeout 一个超时
/// 设置点;操作级整体 deadline 由
/// [`LinuxComputerUseBackend::block_on_a11y`] 兜底。)
async fn a11y_connect() -> Result<zbus::Connection, String> {
    let _ = atspi::connection::set_session_accessibility(true).await;
    let session = zbus::Connection::session()
        .await
        .map_err(|error| format!("session bus: {error}"))?;
    let bus = BusProxy::new(&session)
        .await
        .map_err(|error| format!("a11y bus address proxy: {error}"))?;
    let address: zbus::Address = bus
        .get_address()
        .await
        .map_err(|error| format!("a11y bus address: {error}"))?
        .parse()
        .map_err(|error| format!("a11y bus address parse: {error}"))?;
    zbus::connection::Builder::address(address)
        .map_err(|error| format!("a11y connection builder: {error}"))?
        .method_timeout(A11Y_METHOD_TIMEOUT)
        .build()
        .await
        .map_err(|error| format!("a11y bus connection: {error}"))
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
    /// 自建 a11y 总线连接(带 method_timeout,见 [`a11y_connect`])。
    a11y: Option<zbus::Connection>,
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
    /// 最近一次 portal 截屏折算出的输入倍率(KDE 逻辑像素流 <1,其余 1.0)。
    /// `cursor_position` 记录的是输入坐标,契约要求返回设备物理像素,用
    /// 它做还原;首个截屏之前无从得知,按 1.0 处理(GNOME 恒为 1.0)。
    wayland_input_scale: (f64, f64),
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
        // 记录倍率供 cursor_position 把记录的输入坐标还原为设备物理像素
        // (trait 契约)。
        self.wayland_input_scale = input_scale;
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

    fn require_a11y(&self) -> Result<&zbus::Connection, ComputerUseError> {
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

    /// a11y 异步操作桥:current-thread runtime `block_on` + 操作级整体
    /// deadline(zbus `method_timeout` 之外的第二层兜底;超时区分不了
    /// "无结果"与"失败",一律按 unavailable 上报——fail-closed)。
    fn block_on_a11y<T>(
        &self,
        deadline: Duration,
        what: &str,
        fut: impl Future<Output = Result<T, ComputerUseError>>,
    ) -> Result<T, ComputerUseError> {
        self.runtime.block_on(async move {
            match tokio::time::timeout(deadline, fut).await {
                Ok(result) => result,
                Err(_) => Err(ComputerUseError::unavailable(format!(
                    "AT-SPI operation '{what}' did not finish within {deadline:?}"
                ))),
            }
        })
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
        // 中途失败也要尽力释放全部键,否则修饰键卡死影响后续所有输入;
        // 返回首个错误供上层感知。
        let mut first_err = None;
        for key in keys.iter().rev() {
            if let Err(error) = enigo.key(*key, Direction::Release) {
                if first_err.is_none() {
                    first_err = Some(error);
                }
            }
        }
        match first_err {
            Some(error) => Err(input_failed("key release", error)),
            None => Ok(()),
        }
    }
}

impl ComputerUseBackend for LinuxComputerUseBackend {
    /// 用户撤销授权/全局停止/总开关关闭时由命令层经登记表（BackendRegistry）
    /// `release_os_grant` 请求触发：关闭 portal 会话并保留后端可用（`close`
    /// 后 `session` 为 None，下次输入动作按既有路径懒重建，需要时用户会
    /// 重新看到系统授权对话框）。X11 会话本就无持久授权，`wayland_portal`
    /// 为 None 时此为 no-op。
    fn release_os_grant(&mut self) -> Result<(), ComputerUseError> {
        if let Some(portal) = self.wayland_portal.as_mut() {
            portal.close();
        }
        Ok(())
    }

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
            // 如实披露(评审发现):Wayland 输入整体按 experimental 对待
            // (合成器实现差异);非 Latin-1 文本注入显式拒绝(mutter 对
            // keymap 外 keysym 静默丢弃,见 wayland_portal::char_keysym)。
            let experimental = "Wayland input is experimental (compositor implementations \
                 differ), and typing non-Latin-1 text (CJK etc.) is explicitly rejected: \
                 mutter silently drops keysyms outside the active keymap";
            Capabilities {
                screenshot: portal_ok || self.wayland_screenshot_ok,
                input: self.wayland_portal.is_some(),
                ui_tree,
                notes: format!(
                    "Wayland session ({}): {input_note}; {screenshot_note}; {experimental}; \
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
                Some(position) => {
                    // track_pointer 记录的是输入坐标(工具层 move_to 的入参,
                    // KDE 分支下为流本地逻辑像素);契约要求返回全局设备物理
                    // 像素,按最近一次截屏的输入倍率还原(评审缺陷:GNOME 的
                    // 倍率恒为 1 不受影响,KDE 分数缩放下此前会按错误倍率
                    // 回报并让无坐标 T3 筛查双重缩放)。
                    let (sx, sy) = self.wayland_input_scale;
                    let device = |value: i32, scale: f64| {
                        if scale > 0.0 && scale.is_finite() {
                            (f64::from(value) / scale).round() as i32
                        } else {
                            value
                        }
                    };
                    Ok((device(position.0, sx), device(position.1, sy)))
                }
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
            combine_drag_errors(result, release)?;
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
        combine_drag_errors(result, release)
    }

    fn scroll(&mut self, direction: ScrollDirection, clicks: u32) -> Result<(), ComputerUseError> {
        if self.is_wayland() {
            // amount=0 直接 no-op:mutter 对 axis steps=0 报 Invalid,而
            // notify() 的错误路径会把整个 portal 会话 reset(下次动作重新弹
            // 授权对话框)——不能为一次空滚动付出会话重建的代价。
            if clicks == 0 {
                return Ok(());
            }
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
            // 逐字符 keysym 注入(\n→Return、\t→Tab)。映射先行:非 Latin-1
            // 字符(中文等)显式报错(fail-closed)——mutter 对 keymap 外
            // keysym 静默丢弃,照发就是"成功"却无输入;报错时不应已经弹出
            // 授权对话框。
            let keysyms = text
                .chars()
                .map(wayland_portal::char_keysym)
                .collect::<Result<Vec<_>, _>>()?;
            portal.ensure_started()?;
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
            // 释放阶段中途失败也要尽力释放全部键(避免修饰键卡死),返回
            // 首个错误。
            let mut first_err = None;
            for keysym in mapped.iter().rev() {
                if let Err(error) = portal.keysym_event(*keysym, false) {
                    if first_err.is_none() {
                        first_err = Some(error);
                    }
                }
            }
            match first_err {
                Some(error) => return Err(error),
                None => return Ok(()),
            }
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
            // 释放阶段中途失败也要尽力释放全部键(避免修饰键卡死),返回
            // 首个错误。
            let mut first_err = None;
            for keysym in mapped.iter().rev() {
                if let Err(error) = portal.keysym_event(*keysym, false) {
                    if first_err.is_none() {
                        first_err = Some(error);
                    }
                }
            }
            match first_err {
                Some(error) => return Err(error),
                None => return Ok(()),
            }
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
        let desktop = self.session.desktop_label().to_string();
        self.block_on_a11y(
            A11Y_TREE_DEADLINE,
            "ui_tree",
            ui_tree_async(a11y, opts, wayland, &desktop),
        )
    }

    fn element_at_point(
        &mut self,
        x: i32,
        y: i32,
    ) -> Result<Option<ElementInfo>, ComputerUseError> {
        let a11y = self.require_a11y()?;
        self.block_on_a11y(
            A11Y_POINT_DEADLINE,
            "element_at_point",
            element_at_point_async(a11y, x, y),
        )
    }

    fn focused_element(&mut self) -> Result<Option<ElementInfo>, ComputerUseError> {
        // portal/Wayland 分支同 X11:AT-SPI 是跨会话类型的通路。
        let a11y = self.require_a11y()?;
        self.block_on_a11y(
            A11Y_POINT_DEADLINE,
            "focused_element",
            focused_element_async(a11y),
        )
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
    let (a11y, a11y_init_error) = match runtime.block_on(a11y_connect()) {
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
        wayland_input_scale: (1.0, 1.0),
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

    #[test]
    fn secure_role_decision_is_conservative() {
        // PasswordText 直接判定。
        assert!(is_secure_role(Role::PasswordText, false));
        // 角色查询失败(未知)保守兜底:无法证明不是密码框。
        assert!(is_secure_role(Role::Unknown, true));
        // 对象真实报告 Unknown(非查询失败)不算 secure;普通按钮角色同样不算。
        assert!(!is_secure_role(Role::Unknown, false));
        assert!(!is_secure_role(Role::Button, false));
    }

    #[test]
    fn extents_contain_is_half_open_and_rejects_zero_sized() {
        // 常规包含:原点、内部点;右/下边开区间(恰在边上不算覆盖)。
        let extents = (10, 20, 100, 50);
        assert!(extents_contain(extents, 10, 20));
        assert!(extents_contain(extents, 109, 69));
        assert!(!extents_contain(extents, 110, 69));
        assert!(!extents_contain(extents, 109, 70));
        assert!(!extents_contain(extents, 9, 20));
        assert!(!extents_contain(extents, 10, 19));
        // 负原点(多显示器布局,副屏在左侧/上方)。
        let negative = (-1920, -400, 1920, 1080);
        assert!(extents_contain(negative, -1, -1));
        assert!(!extents_contain(negative, -1921, 0));
        // 零尺寸 extents 不包含任何点(screen_extents_strict 已对它报错,
        // 这里保证即使漏进循环也不会被误判为覆盖)。
        let zero = (0, 0, 0, 0);
        assert!(!extents_contain(zero, 0, 0));
        assert!(!extents_contain(zero, i32::MAX, i32::MAX));
    }

    #[test]
    fn state_flags_mark_secure_and_states() {
        // secure 标志由调用方按 is_secure_role 给出。
        assert!(state_flags(true, None).contains(&"secure"));
        assert!(!state_flags(false, None).contains(&"secure"));
        // 常规状态位不受 secure 判定影响。
        let flags = state_flags(false, Some(StateSet::new(State::Focused)));
        assert!(flags.contains(&"focused"));
        assert!(!flags.contains(&"secure"));
        let flags = state_flags(true, Some(StateSet::new(State::Active | State::Editable)));
        assert!(flags.contains(&"active") && flags.contains(&"editable"));
        // Enabled 缺失 → disabled。
        assert!(state_flags(false, Some(StateSet::empty())).contains(&"disabled"));
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
    async fn read_texts_via_a11y(conn: &zbus::Connection) -> Result<Vec<String>, ComputerUseError> {
        let mut texts = Vec::new();
        walk_texts(conn, root_accessible(conn).await?, 18, &mut texts).await?;
        Ok(texts)
    }

    async fn walk_texts(
        conn: &zbus::Connection,
        proxy: AccessibleProxy<'_>,
        depth: u32,
        out: &mut Vec<String>,
    ) -> Result<(), ComputerUseError> {
        if depth == 0 || out.len() >= 64 {
            return Ok(());
        }
        let role = proxy.get_role().await.unwrap_or(Role::Unknown);
        if role == Role::Text {
            let text = TextProxy::builder(conn)
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
                if let Ok(child) = child.into_accessible_proxy(conn).await {
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
        let conn = runtime.block_on(a11y_connect()).expect("a11y connection");
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
