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

/// Wayland 和弦按压失败时的回退(供 `key_chord`/`hold_key` 共用):顺序按下
/// 全部 keysym;某一次按压失败时,逆序尽力释放已按下的键(释放错误吞掉),
/// 再上抛原始错误——避免中途失败把修饰键卡在按下状态(与 X11 的
/// `press_chord` 同款回退)。对注入端泛型,单测可用录制替身驱动。
fn press_keysyms_unwind(
    keysyms: &[i32],
    mut event: impl FnMut(i32, bool) -> Result<(), ComputerUseError>,
) -> Result<(), ComputerUseError> {
    for (index, keysym) in keysyms.iter().enumerate() {
        if let Err(error) = event(*keysym, true) {
            // Release the already-pressed keysyms in reverse, best-effort
            // (release errors swallowed): the caller's original press error
            // is what matters, but stranded modifiers would corrupt every
            // subsequent input action.
            for held in keysyms[..index].iter().rev() {
                let _ = event(*held, false);
            }
            return Err(error);
        }
    }
    Ok(())
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
/// - `Ok(None)`:**无元素**——枚举到的窗口都不覆盖该点、覆盖窗口的 AT-SPI
///   命中为空(null ObjectRef),**或可达树为空**(注册表应答但没有任何
///   应用注册——目标应用的 toolkit a11y 未启用时的常态)。空树与「元素
///   不在该点」在此不可区分,同按主流 None-策略放行(无元素 → 不强制
///   确认);这只是策略声明,不是筛查证明。
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
    // The bootstrap session-bus connection needs a deadline too: zbus 5
    // defaults method_timeout to None, so a wedged session bus would hang
    // `create_backend` on the worker thread forever. Bound it with the same
    // 3s method timeout as the self-built a11y bus connection below.
    let session = zbus::connection::Builder::session()
        .map_err(|error| format!("session bus builder: {error}"))?
        .method_timeout(A11Y_METHOD_TIMEOUT)
        .build()
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

/// Wayland 探测时限。xcap 的 portal 应答是**无界** D-Bus 等待（内部
/// `receiver.recv()??`，KDE 还可能每次弹交互式对话框）——一旦在等人，
/// 探测永远不返回，而 catch_unwind 挡不住挂起：backend worker 会被永久
/// pin 死，190s 调用方超时后 in-flight 门与控制通道（紧急抬起、授权释放）
/// 全部堵死（round-6 评审）。超时后放弃并遗弃探测线程（纯捕获、不碰共享
/// 状态；多次超时至多多遗弃几个线程，好过 worker 卡死）。下一次截屏仍会
/// 重试探测——粘死探测状态会退回「一次失败永久失去截屏」的旧缺陷。
const WAYLAND_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// xcap 的 Wayland 路径内部有 `.expect(...)`(PNG 重编码),包一层
/// catch_unwind 把潜在 panic 转成显式错误,避免炸掉 backend worker 线程;
/// 整个探测限时运行（见 [`WAYLAND_PROBE_TIMEOUT`]）。
fn probe_wayland_screenshot_guarded() -> Result<(), String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("computer-use-wayland-probe".to_string())
        .spawn(move || {
            let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                probe_wayland_screenshot,
            )) {
                Ok(inner) => inner,
                Err(_) => Err("capture panicked inside xcap's Wayland fallback chain".to_string()),
            };
            let _ = sender.send(result);
        })
        .map_err(|error| format!("capture probe thread spawn failed: {error}"))?;
    match receiver.recv_timeout(WAYLAND_PROBE_TIMEOUT) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "the Wayland capture probe did not answer within {}s (the portal screenshot \
             request may be waiting on an interactive dialog)",
            WAYLAND_PROBE_TIMEOUT.as_secs()
        )),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err("capture probe thread terminated unexpectedly".to_string())
        }
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
        // Keep the stored input scale in sync with the capture actually
        // returned: portal and xcap-fallback captures live in different
        // input coordinate spaces, and cursor_position undoes the scale of
        // the LAST successful capture path (a stale portal scale would
        // misreport cursor_position once captures alternate portal stream →
        // xcap fallback, e.g. under KDE fractional scaling).
        if self.is_wayland() {
            self.wayland_input_scale = (input_scale_x, input_scale_y);
        }
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
            let mut last_reached = (from.0, from.1);
            for step in 1..=DRAG_STEPS {
                let t = f64::from(step) / f64::from(DRAG_STEPS);
                let x = f64::from(from.0) + f64::from(to.0 - from.0) * t;
                let y = f64::from(from.1) + f64::from(to.1 - from.1) * t;
                if let Err(error) = portal.motion_absolute(x.round() as i32, y.round() as i32) {
                    result = Err(error);
                    break;
                }
                last_reached = (x.round() as i32, y.round() as i32);
                sleep(Duration::from_millis(DRAG_STEP_MS));
            }
            let release = portal.button(wayland_portal::map_button(MouseButton::Left), false);
            // Record the destination only when the interpolated move fully
            // succeeded; on failure keep the furthest waypoint the pointer
            // verifiably reached (or the drag start) so cursor_position
            // never reports a spot the pointer never touched.
            if result.is_ok() {
                portal.track_pointer(to.0, to.1);
            } else {
                portal.track_pointer(last_reached.0, last_reached.1);
            }
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
            // A failed press aborts (nothing landed for that char); a failed
            // release must not strand the loop since the press already
            // landed: remember the first error, keep typing the remaining
            // chars, and report the first error at the end (same semantics
            // as the X11 `release_chord` helper).
            let mut first_err = None;
            for keysym in keysyms {
                portal.keysym_event(keysym, true)?;
                if let Err(error) = portal.keysym_event(keysym, false) {
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
            press_keysyms_unwind(&mapped, |keysym, pressed| {
                portal.keysym_event(keysym, pressed)
            })?;
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
            press_keysyms_unwind(&mapped, |keysym, pressed| {
                portal.keysym_event(keysym, pressed)
            })?;
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

    #[test]
    fn wayland_chord_press_unwinds_held_keysyms_on_failure() {
        // Recording stand-in: pressing the second keysym fails.
        let mut events: Vec<(i32, bool)> = Vec::new();
        let result = press_keysyms_unwind(&[0xffe3, 0x63, 0x64], |keysym, pressed| {
            let failed = pressed && keysym == 0x63;
            events.push((keysym, pressed));
            if failed {
                Err(ComputerUseError::failed("portal injection failed"))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        // The modifier pressed before the failure is released in reverse;
        // keysyms after the failing one are never attempted.
        assert_eq!(events, vec![(0xffe3, true), (0x63, true), (0xffe3, false)]);
    }

    #[test]
    fn wayland_chord_press_success_does_not_release_anything() {
        let mut events: Vec<(i32, bool)> = Vec::new();
        let result = press_keysyms_unwind(&[0xffe3, 0x63], |keysym, pressed| {
            events.push((keysym, pressed));
            Ok(())
        });
        assert!(result.is_ok());
        assert_eq!(events, vec![(0xffe3, true), (0x63, true)]);
    }

    #[test]
    fn wayland_chord_press_unwind_release_errors_are_swallowed() {
        // Release on the unwind failing too must not mask the press error.
        let mut releases = 0;
        let result = press_keysyms_unwind(&[0xffe3, 0x63], |keysym, pressed| {
            if pressed {
                if keysym == 0x63 {
                    return Err(ComputerUseError::failed("press failed"));
                }
            } else {
                releases += 1;
                return Err(ComputerUseError::failed("release failed"));
            }
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "failed: press failed");
        assert_eq!(releases, 1);
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
        wait_for_big_change(&mut backend, &shot1, 40, shot_h / 2)
            .expect("clicking the clock must open the calendar dropdown");

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

// ---------------------------------------------------------------------------
// X11 live E2E: the full consent pipeline against a real X server (Xvfb).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod x11_live_tests {
    //! Live X11 E2E for the real backend plus the full consent pipeline
    //! (settings toggle → session grant → fail-closed T3 screening →
    //! user confirmation token → execute → redacted audit).
    //!
    //! WARNING: each test takes over the X server named by `$DISPLAY` and
    //! injects real XTEST input into it — run them ONLY against a sandboxed
    //! Xvfb, never against a desktop someone is using:
    //!
    //! ```text
    //! Xvfb :99 -screen 0 1280x800x24 &
    //! cd pinvou3-app/src-tauri
    //! DISPLAY=:99 cargo test --lib computer_use -- --ignored --test-threads=1
    //! ```
    //!
    //! Every injection is verified EXTERNALLY: the X server itself reports the
    //! pointer position via `xdotool getmouselocation`, so the assertions
    //! cannot be fooled by anything inside the process.
    //!
    //! The AT-SPI (a11y) stack is intentionally absent under Xvfb: the test
    //! points the D-Bus session bus at a nonexistent socket, so the backend's
    //! AT-SPI connection fails at init and EVERY input target is unscreenable.
    //! That exercises the fail-closed screening path end to end: even with a
    //! session grant, a screened input action must stop and demand a user
    //! confirmation token against a real X server.
    //!
    //! Tests are `#[ignore]` so CI never runs them; each additionally skips
    //! cleanly (early return) unless `$DISPLAY` answers
    //! `xdotool getdisplaygeometry`.

    #![allow(clippy::await_holding_lock)]

    use super::*;
    use crate::features::computer_use::backend::BackendHandle;
    use crate::features::computer_use::guard::ComputerUseShared;
    use crate::features::computer_use::tool::{ComputerUseEventSink, ComputerUseTool};
    use crate::features::computer_use::types::{EVENT_CONFIRM_REQUIRED, EVENT_GRANT_REQUIRED};
    use deepseek_tui::tools::spec::{ToolContext, ToolSpec};
    use serde_json::{Value, json};
    use std::process::Command;
    use std::sync::{Arc, Mutex as StdMutex};
    use xcap::image;

    /// Session id used by every test (drives the audit file name too).
    const SESSION: &str = "s-x11";

    // ---- external X server verification helpers ---------------------------

    /// Run xdotool against `display`; `None` when xdotool is missing or the
    /// display does not answer (the caller then skips the test).
    fn xdotool(display: &str, args: &[&str]) -> Option<String> {
        let output = Command::new("xdotool")
            .env("DISPLAY", display)
            .args(args)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// The pointer position as reported by the X server itself (external
    /// ground truth, not anything the backend claims).
    fn pointer_at(display: &str) -> Option<(i32, i32)> {
        let out = xdotool(display, &["getmouselocation"])?;
        let mut x = None;
        let mut y = None;
        for token in out.split_whitespace() {
            if let Some(value) = token.strip_prefix("x:") {
                x = value.parse().ok();
            }
            if let Some(value) = token.strip_prefix("y:") {
                y = value.parse().ok();
            }
        }
        Some((x?, y?))
    }

    /// The live-display gate: `Some((width, height, display))` when `$DISPLAY`
    /// names an X server xdotool can reach, `None` otherwise (tests skip).
    fn live_display() -> Option<(u32, u32, String)> {
        let display = std::env::var("DISPLAY").ok()?;
        let geometry = xdotool(&display, &["getdisplaygeometry"])?;
        let mut parts = geometry.split_whitespace();
        let width = parts.next()?.parse().ok()?;
        let height = parts.next()?.parse().ok()?;
        Some((width, height, display))
    }

    fn assert_pointer_unchanged(display: &str, before: (i32, i32), what: &str) {
        let after = pointer_at(display).expect("xdotool can read the pointer");
        assert_eq!(
            after, before,
            "{what}: the X server pointer must not move (before {before:?}, after {after:?})"
        );
    }

    /// Expected PNG dimensions for a `screen` of `size` under the tool's
    /// scaling cap ([`crate::features::computer_use::scaling::MAX_LONG_EDGE`];
    /// smaller screens are never upscaled).
    fn expected_png_size(size: (u32, u32)) -> (u32, u32) {
        use crate::features::computer_use::scaling::MAX_LONG_EDGE;
        let long = size.0.max(size.1);
        if long <= MAX_LONG_EDGE {
            return size;
        }
        let factor = f64::from(MAX_LONG_EDGE) / f64::from(long);
        (
            (f64::from(size.0) * factor).round() as u32,
            (f64::from(size.1) * factor).round() as u32,
        )
    }

    // ---- fixture (isolated PINVOU3_HOME + real X11 backend) ---------------

    /// Records the Tauri events the tool emits (grant/confirm prompts) so the
    /// tests can read `confirm_id`s exactly like the frontend does.
    #[derive(Clone)]
    struct EventRecorder(Arc<StdMutex<Vec<(String, Value)>>>);

    impl EventRecorder {
        fn new() -> Self {
            Self(Arc::new(StdMutex::new(Vec::new())))
        }

        fn latest_confirm_id(&self) -> String {
            self.0
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|(name, _)| name == EVENT_CONFIRM_REQUIRED)
                .map(|(_, payload)| {
                    payload["confirm_id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .expect("a computer_use:confirm_required event must have been emitted")
        }

        fn emitted(&self, event: &str) -> bool {
            self.0.lock().unwrap().iter().any(|(name, _)| name == event)
        }
    }

    impl ComputerUseEventSink for EventRecorder {
        fn emit(&self, event: &str, payload: Value) {
            if let Ok(mut events) = self.0.lock() {
                events.push((event.to_string(), payload));
            }
        }
    }

    /// Process-env takeover for one test: `$DISPLAY` → the live X server,
    /// session type forced to X11, the D-Bus session bus pointed at a
    /// nonexistent socket (the AT-SPI stack is intentionally absent under
    /// Xvfb — this is what makes the screening genuinely unscreenable),
    /// `PINVOU3_HOME` → an isolated temp root so the audit JSONL and
    /// screenshots never touch the developer's real data. Everything is
    /// restored on drop; the platform env lock is held for the whole test so
    /// env writes stay serialized in-process.
    struct LiveEnv {
        _env_lock: std::sync::MutexGuard<'static, ()>,
        previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
        home: std::path::PathBuf,
    }

    impl LiveEnv {
        fn take(display: &str) -> Self {
            let env_lock = crate::platform::paths::tests::ENV_LOCK
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let mut previous: Vec<(&'static str, Option<std::ffi::OsString>)> = Vec::new();
            let mut set = |key: &'static str, value: Option<std::ffi::OsString>| {
                previous.push((key, std::env::var_os(key)));
                // SAFETY: ENV_LOCK is held; env writes are serialized in-process.
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            };
            let home = std::env::temp_dir().join(format!(
                "pinvou3-cu-x11-live-{}-{}",
                std::process::id(),
                crate::platform::paths::tests::unique_suffix()
            ));
            set("PINVOU3_HOME", Some(home.clone().into_os_string()));
            set("DISPLAY", Some(display.into()));
            // Force X11 detection even when the host session is Wayland.
            set("XDG_SESSION_TYPE", Some("x11".into()));
            set("WAYLAND_DISPLAY", None);
            // No a11y bus in the sandbox: the backend's AT-SPI connect must
            // fail at init so every target is unscreenable (fail closed).
            set(
                "DBUS_SESSION_BUS_ADDRESS",
                Some("unix:path=/tmp/pinvou3-cu-x11-live-no-a11y-bus".into()),
            );
            Self {
                _env_lock: env_lock,
                previous,
                home,
            }
        }

        fn audit_path(&self) -> std::path::PathBuf {
            self.home
                .join("computer-use")
                .join(format!("audit-{SESSION}.jsonl"))
        }
    }

    impl Drop for LiveEnv {
        fn drop(&mut self) {
            for (key, value) in self.previous.drain(..) {
                // SAFETY: ENV_LOCK is still held by self._env_lock.
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }

    struct Fixture {
        tool: ComputerUseTool,
        shared: Arc<ComputerUseShared>,
        events: EventRecorder,
        env: LiveEnv,
        workspace: std::path::PathBuf,
        display: String,
    }

    /// Build the real stack: `platform::create_backend` (xcap capture + enigo
    /// XTEST on `$DISPLAY`, AT-SPI absent) behind the same lazy BackendHandle
    /// the production tool uses, with an isolated audit home and a recording
    /// event sink. Declared field order matters on drop: the tool (which
    /// triggers the backend emergency release) drops before the env is
    /// restored.
    fn fixture(display: String) -> Fixture {
        let env = LiveEnv::take(&display);
        let workspace = env.home.join("sessions").join(SESSION).join("workspace");
        let _ = std::fs::create_dir_all(&workspace);
        let shared = Arc::new(ComputerUseShared::new());
        // The settings toggle (computer_use_set_enabled) mirrors here.
        shared.set_enabled(true);
        let events = EventRecorder::new();
        let backend = BackendHandle::lazy(move || create_backend());
        let tool = ComputerUseTool::with_parts(
            SESSION.to_string(),
            Arc::clone(&shared),
            backend,
            Arc::new(events.clone()),
        );
        Fixture {
            tool,
            shared,
            events,
            env,
            workspace,
            display,
        }
    }

    impl Fixture {
        async fn execute_raw(
            &self,
            input: Value,
        ) -> Result<deepseek_tui::tools::spec::ToolResult, deepseek_tui::tools::spec::ToolError>
        {
            self.tool
                .execute(input, &ToolContext::new(self.workspace.as_path()))
                .await
        }

        /// Run a tool call and flatten to (success, model-visible text).
        async fn execute(&self, input: Value) -> (bool, String) {
            match self.execute_raw(input).await {
                Ok(result) => (result.success, result.content),
                Err(error) => (false, error.to_string()),
            }
        }
    }

    #[tokio::test]
    #[ignore = "drives the X server named by $DISPLAY (run against a sandboxed Xvfb)"]
    async fn x11_live_capabilities_are_reported() {
        let Some((_, _, display)) = live_display() else {
            eprintln!("SKIP x11_live_capabilities_are_reported: $DISPLAY does not answer xdotool");
            return;
        };
        let fx = fixture(display);

        // The real backend, asked directly: X11 must report full input and
        // capture support, and must honestly report the absent AT-SPI stack.
        let probe = BackendHandle::lazy(create_backend);
        let caps = probe
            .capabilities()
            .expect("the real X11 backend must construct on the live display");
        println!(
            "x11_live capabilities: screenshot={} input={} ui_tree={}\n  notes: {}",
            caps.screenshot, caps.input, caps.ui_tree, caps.notes
        );
        assert!(caps.input, "X11 XTEST input must be reported available");
        assert!(
            caps.screenshot,
            "X11 xcap capture must be reported available"
        );
        assert!(
            !caps.ui_tree,
            "AT-SPI is absent in the sandbox; ui_tree must be reported unavailable: {}",
            caps.notes
        );

        // The tool-level ui_tree action must fail with the documented
        // unavailable error instead of silently returning a tree.
        let (success, text) = fx.execute(json!({"action": "ui_tree"})).await;
        println!("x11_live ui_tree action: success={success} text={text}");
        assert!(!success, "ui_tree must not succeed without AT-SPI: {text}");
        assert!(
            text.contains("accessibility tree is unsupported"),
            "ui_tree must fail with the documented unavailable error: {text}"
        );
        assert!(
            !text.contains("[0]"),
            "ui_tree must not return any serialized tree: {text}"
        );
    }

    #[tokio::test]
    #[ignore = "drives the X server named by $DISPLAY (run against a sandboxed Xvfb)"]
    async fn x11_live_capture_returns_screen_image() {
        let Some((width, height, display)) = live_display() else {
            eprintln!(
                "SKIP x11_live_capture_returns_screen_image: $DISPLAY does not answer xdotool"
            );
            return;
        };
        let fx = fixture(display);

        let result = fx
            .execute_raw(json!({"action": "screenshot"}))
            .await
            .expect("screenshot must not return a tool error");
        assert!(result.success, "{}", result.content);

        // PNG decodes and matches the externally verified geometry, modulo
        // the tool's long-edge scaling cap (small screens are not upscaled).
        let abs_path = result
            .metadata
            .as_ref()
            .and_then(|m| m.get("images"))
            .and_then(Value::as_array)
            .and_then(|images| images.first())
            .and_then(Value::as_str)
            .map(std::path::PathBuf::from)
            .expect("screenshot must attach an image path");
        let png = std::fs::read(&abs_path).expect("screenshot file must exist");
        let decoded = image::load_from_memory(&png).expect("the attachment must be a valid PNG");
        let (expected_w, expected_h) = expected_png_size((width, height));
        println!(
            "x11_live capture: xdotool geometry {width}x{height}, png {}x{} (expected {expected_w}x{expected_h})",
            decoded.width(),
            decoded.height()
        );
        assert_eq!(
            (decoded.width(), decoded.height()),
            (expected_w, expected_h),
            "capture must match the X server geometry modulo the long-edge cap"
        );
        assert!(
            result
                .content
                .contains(&format!("{expected_w}x{expected_h} px")),
            "the model-visible text must report the real geometry: {}",
            result.content
        );

        // Privacy: the screenshot (may contain on-screen secrets) lands 0600
        // inside a 0700 directory, under the isolated home.
        use std::os::unix::fs::PermissionsExt;
        let file_mode = std::fs::metadata(&abs_path)
            .expect("screenshot metadata")
            .permissions()
            .mode();
        assert_eq!(file_mode & 0o7777, 0o600, "screenshot file must be 0600");
        let dir_mode = std::fs::metadata(abs_path.parent().expect("screenshot has a parent"))
            .expect("screenshot dir metadata")
            .permissions()
            .mode();
        assert_eq!(
            dir_mode & 0o7777,
            0o700,
            "screenshot directory must be 0700"
        );
    }

    /// The core consent E2E against a real X server: no grant → nothing
    /// injected; grant → hover moves execute (deliberately unscreened), but a
    /// pointer CLICK is T3-screened and, with AT-SPI absent, fails closed to
    /// a confirmation; mint → execute lands on the X server; replay → spent.
    #[tokio::test]
    #[ignore = "drives the X server named by $DISPLAY (run against a sandboxed Xvfb)"]
    async fn x11_live_input_requires_grant_then_confirmation_then_executes() {
        let Some((_, _, display)) = live_display() else {
            eprintln!("SKIP x11_live_input_requires_grant…: $DISPLAY does not answer xdotool");
            return;
        };
        let fx = fixture(display);

        // (1) No session grant: the gate rejects before any injection, the
        // grant_required event fires, and the X server pointer stays put.
        let before = pointer_at(&fx.display).expect("xdotool can read the pointer");
        let move_to = json!({"action": "mouse_move", "x": 600, "y": 400});
        let (success, text) = fx.execute(move_to.clone()).await;
        assert!(!success, "an ungranted input action must fail: {text}");
        assert!(text.contains("has not granted control"), "{text}");
        assert!(
            fx.events.emitted(EVENT_GRANT_REQUIRED),
            "the grant_required event must fire for the frontend prompt"
        );
        assert_pointer_unchanged(&fx.display, before, "mouse_move without a grant");

        // (2) Grant (the same guard call the computer_use_grant command
        // makes). A plain mouse_move is a hover — deliberately NOT
        // T3-screened — so it executes; the X server must report the target.
        fx.shared.grant_session(SESSION);
        let (success, text) = fx.execute(move_to).await;
        assert!(success, "a granted hover move must execute: {text}");
        let at = pointer_at(&fx.display).expect("xdotool");
        assert!(
            (at.0 - 600).abs() <= 2 && (at.1 - 400).abs() <= 2,
            "the granted hover must land at (600, 400); xdotool reports {at:?}"
        );

        // (3) A pointer click is a screened action: with the a11y stack
        // absent the target is unscreenable, so the action must fail CLOSED —
        // "NOT executed" + a pending confirmation in the guard + a pointer
        // that provably did not move.
        let click = json!({"action": "left_click", "x": 200, "y": 300});
        let before = pointer_at(&fx.display).expect("xdotool");
        let (success, text) = fx.execute(click.clone()).await;
        assert!(!success, "an unscreenable click must not execute: {text}");
        assert!(text.contains("NOT executed"), "{text}");
        assert!(text.contains("unverifiable"), "{text}");
        assert!(text.contains("confirm_id"), "{text}");
        let confirm_id = fx.events.latest_confirm_id();
        assert!(
            fx.shared.pending_confirmation(&confirm_id).is_some(),
            "the pending confirmation must exist in the guard"
        );
        assert_pointer_unchanged(&fx.display, before, "confirmation-required click");

        // (4) Mint the token (the same guard call computer_use_confirm makes)
        // and retry the same action WITH the confirm_id: it executes and the
        // X server reports the injected coordinates.
        assert!(
            fx.shared.mint_confirmation(&confirm_id),
            "minting must succeed while the pending exists"
        );
        let confirmed_click =
            json!({"action": "left_click", "x": 200, "y": 300, "confirm_id": confirm_id});
        let (success, text) = fx.execute(confirmed_click.clone()).await;
        assert!(success, "the confirmed click must execute: {text}");
        assert!(!text.contains("NOT executed"), "{text}");
        let at = pointer_at(&fx.display).expect("xdotool");
        assert!(
            (at.0 - 200).abs() <= 2 && (at.1 - 300).abs() <= 2,
            "the confirmed click must land at (200, 300); xdotool reports {at:?}"
        );

        // (5) The token is single-use: replaying the same confirm_id fails as
        // invalid/used and injects nothing.
        let before = pointer_at(&fx.display).expect("xdotool");
        let (success, text) = fx.execute(confirmed_click).await;
        assert!(!success, "a spent token must not execute: {text}");
        assert!(
            text.contains("invalid, expired, or was already used"),
            "{text}"
        );
        assert_pointer_unchanged(&fx.display, before, "replayed confirm_id");
    }

    /// Denial has no memory: denying just closes the dialog — the denied id
    /// reads as invalid/spent on retry, mints nothing, and injects nothing;
    /// a fresh blocked request mints a NEW confirm_id.
    #[tokio::test]
    #[ignore = "drives the X server named by $DISPLAY (run against a sandboxed Xvfb)"]
    async fn x11_live_deny_blocks_action_without_memory() {
        let Some((_, _, display)) = live_display() else {
            eprintln!(
                "SKIP x11_live_deny_blocks_action_without_memory: $DISPLAY does not answer xdotool"
            );
            return;
        };
        let fx = fixture(display);
        fx.shared.grant_session(SESSION);

        let click = json!({"action": "left_click", "x": 500, "y": 450});
        let before = pointer_at(&fx.display).expect("xdotool");
        let (success, text) = fx.execute(click.clone()).await;
        assert!(!success, "{text}");
        assert!(text.contains("NOT executed"), "{text}");
        let confirm_id = fx.events.latest_confirm_id();

        // Deny (the same guard call the computer_use_deny command makes).
        assert!(
            fx.shared.deny_confirmation(&confirm_id),
            "denying must consume the pending confirmation"
        );
        assert!(
            fx.shared.pending_confirmation(&confirm_id).is_none(),
            "the pending must be gone after the denial"
        );

        // Retrying with the denied id: the token is simply invalid (there is
        // no remembered denial), it cannot mint again, nothing is injected.
        let denied_retry = json!({
            "action": "left_click",
            "x": 500,
            "y": 450,
            "confirm_id": confirm_id.clone()
        });
        let (success, text) = fx.execute(denied_retry).await;
        assert!(!success, "a denied action must not execute: {text}");
        assert!(
            text.contains("invalid, expired, or was already used"),
            "{text}"
        );
        assert_pointer_unchanged(&fx.display, before, "denied click retry");
        assert!(
            !fx.shared.mint_confirmation(&confirm_id),
            "a denied confirmation must not mint"
        );

        // A fresh attempt without a token goes through the confirmation flow
        // again (the denial must not leak into the no-token path).
        let (success, text) = fx.execute(click).await;
        assert!(!success, "{text}");
        assert!(text.contains("NOT executed"), "{text}");
        let new_id = fx.events.latest_confirm_id();
        assert_ne!(new_id, confirm_id, "a NEW pending must be requested");
        assert!(fx.shared.pending_confirmation(&new_id).is_some());
        assert_pointer_unchanged(&fx.display, before, "fresh blocked click");
    }

    /// B2 regression, live: typed text (and typing-form chords) must reach
    /// the X server but never the audit JSONL in plaintext — the audit
    /// stores redacted targets only (`typed N characters` / `pressed 1
    /// key`), and the audit file itself must be 0600.
    #[tokio::test]
    #[ignore = "drives the X server named by $DISPLAY (run against a sandboxed Xvfb)"]
    async fn x11_live_type_audit_stays_redacted() {
        let Some((_, _, display)) = live_display() else {
            eprintln!("SKIP x11_live_type_audit_stays_redacted: $DISPLAY does not answer xdotool");
            return;
        };
        let fx = fixture(display);
        fx.shared.grant_session(SESSION);

        let secret = "p@ssw0rd-shift-test";
        let type_call = json!({"action": "type", "text": secret});
        // Typing target is unverifiable (no AT-SPI): fail closed first.
        let (success, text) = fx.execute(type_call.clone()).await;
        assert!(!success, "{text}");
        assert!(text.contains("NOT executed"), "{text}");
        let type_id = fx.events.latest_confirm_id();
        assert!(fx.shared.mint_confirmation(&type_id));
        let confirmed_type = json!({"action": "type", "text": secret, "confirm_id": type_id});
        let (success, text) = fx.execute(confirmed_type).await;
        assert!(success, "the confirmed typing must execute: {text}");

        // A duplicate-modifier typing-form chord goes through the same
        // pipeline; accepted or rejected at parse, either outcome is reported
        // (the current parser accepts shift+shift+h and audits it as typed
        // text, exactly like a bare single character).
        let chord = "shift+shift+h";
        let key_call = json!({"action": "key", "text": chord});
        let (success, text) = fx.execute(key_call.clone()).await;
        let chord_needed_confirmation = !success;
        if chord_needed_confirmation {
            assert!(text.contains("confirm_id"), "{text}");
            let key_id = fx.events.latest_confirm_id();
            assert!(fx.shared.mint_confirmation(&key_id));
            let confirmed_key = json!({"action": "key", "text": chord, "confirm_id": key_id});
            let (success, text) = fx.execute(confirmed_key).await;
            assert!(success, "the confirmed chord must execute: {text}");
        }
        println!("x11_live chord {chord:?}: needed confirmation = {chord_needed_confirmation}");

        // Audit privacy: the plaintexts must appear nowhere in the JSONL;
        // every record carries a redacted target only.
        let audit_path = fx.env.audit_path();
        let raw = std::fs::read_to_string(&audit_path).expect("the audit JSONL must exist");
        assert!(
            !raw.contains(secret),
            "typed plaintext leaked into the audit log"
        );
        assert!(
            !raw.contains(chord),
            "chord plaintext leaked into the audit log"
        );
        assert!(
            !raw.contains("keys: shift+shift+h"),
            "a typing-form chord must not be audited as a plaintext keys: shortcut"
        );
        let records: Vec<Value> = raw
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_str(line).expect("valid JSONL line"))
            .collect();
        let typed: Vec<&Value> = records
            .iter()
            .filter(|r| r["action"] == "type" && r["target"] == "typed 19 characters")
            .collect();
        assert_eq!(
            typed.len(),
            2,
            "blocked + executed type calls must both audit the redacted count: {records:?}"
        );
        let chords: Vec<&Value> = records
            .iter()
            .filter(|r| r["action"] == "key" && r["target"] == "pressed 1 key")
            .collect();
        assert_eq!(
            chords.len(),
            2,
            "blocked + executed chord calls must both audit the key count: {records:?}"
        );
        // The audit is a plain log: no crypto or begin/end phase fields.
        for absent in ["salt", "text_hmac", "text_len", "phase"] {
            assert!(!raw.contains(absent), "{absent} in {raw}");
        }

        // The audit file itself is private.
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&audit_path)
            .expect("audit metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o7777, 0o600, "the audit JSONL must be 0600");
    }

    /// The computer_use_stop path (guard stop_all + registry
    /// emergency_release_all) wipes grants and consent state on a live
    /// backend: after stop + reopen, input is grant-required again.
    #[tokio::test]
    #[ignore = "drives the X server named by $DISPLAY (run against a sandboxed Xvfb)"]
    async fn x11_live_stop_releases_and_wipes() {
        let Some((_, _, display)) = live_display() else {
            eprintln!("SKIP x11_live_stop_releases_and_wipes: $DISPLAY does not answer xdotool");
            return;
        };
        let fx = fixture(display);
        fx.shared.grant_session(SESSION);

        let click = json!({"action": "left_click", "x": 300, "y": 200});
        let (success, text) = fx.execute(click.clone()).await;
        assert!(!success, "{text}");
        assert!(text.contains("NOT executed"), "{text}");
        let confirm_id = fx.events.latest_confirm_id();
        assert!(fx.shared.pending_confirmation(&confirm_id).is_some());

        // The computer_use_stop command path, verbatim.
        fx.shared.stop_all();
        fx.shared.backends.emergency_release_all();

        assert!(
            !fx.shared.has_active_grant(SESSION),
            "stop must wipe the session grant"
        );
        assert!(
            fx.shared.pending_confirmation(&confirm_id).is_none(),
            "stop must wipe the pending confirmation"
        );
        assert!(
            !fx.shared.mint_confirmation(&confirm_id),
            "minting on a wiped pending must fail"
        );
        assert!(fx.shared.is_stopped(), "stop must raise the stop flag");

        // Reopen (computer_use_set_enabled(true) resets the stop flag): the
        // grant must still be gone — input is grant-required again.
        fx.shared.reset_stop();
        let before = pointer_at(&fx.display).expect("xdotool");
        let (success, text) = fx.execute(click).await;
        assert!(!success, "{text}");
        assert!(text.contains("has not granted control"), "{text}");
        assert_pointer_unchanged(&fx.display, before, "click after stop + reopen");

        // Best-effort check that no XTEST button is left pressed: xdotool has
        // no button-state query, so this is reported rather than asserted —
        // the emergency release path (physical button up first, then the OS
        // grant close) was requested above for every registered backend.
        eprintln!(
            "x11_live_stop: XTEST button state is not verifiable via xdotool; \
             the emergency release (mouse-up + OS grant close) was requested"
        );
    }

    /// Boundary rejections through the real stack: overlong/empty/unknown
    /// chords are rejected at parse, and an out-of-bounds pointer move is
    /// clamped — the X server must never report an out-of-screen pointer.
    #[tokio::test]
    #[ignore = "drives the X server named by $DISPLAY (run against a sandboxed Xvfb)"]
    async fn x11_live_boundary_rejections() {
        let Some((width, height, display)) = live_display() else {
            eprintln!("SKIP x11_live_boundary_rejections: $DISPLAY does not answer xdotool");
            return;
        };
        let fx = fixture(display);
        fx.shared.grant_session(SESSION);

        // (1) More than 4 chord tokens → rejected at parse (use type instead).
        let (success, text) = fx
            .execute(json!({"action": "key", "text": "a+b+c+d+e"}))
            .await;
        assert!(!success, "{text}");
        assert!(
            text.contains("invalid key chord") && text.contains("more than 4"),
            "overlong chord must be rejected at parse: {text}"
        );

        // (2) Empty chord → rejected.
        let (success, text) = fx.execute(json!({"action": "key", "text": ""})).await;
        assert!(!success, "{text}");
        assert!(
            text.contains("non-empty"),
            "an empty chord must be rejected: {text}"
        );

        // (3) Unknown key name → rejected.
        let (success, text) = fx
            .execute(json!({"action": "key", "text": "ctrl+nosuchkey"}))
            .await;
        assert!(!success, "{text}");
        assert!(
            text.contains("unknown key"),
            "an unknown key name must be rejected: {text}"
        );

        // (4) Out-of-bounds pointer move: clamped with a warning (a hover is
        // not T3-screened), and in NO case may the pointer land outside the
        // screen — verified by the X server itself.
        let (success, text) = fx
            .execute(json!({"action": "mouse_move", "x": 99999, "y": 99999}))
            .await;
        println!("x11_live out-of-bounds move: success={success} text={text}");
        assert!(
            success,
            "an out-of-bounds move is clamped, not rejected: {text}"
        );
        assert!(
            text.contains("clamped"),
            "the clamping must be reported as a warning: {text}"
        );
        let at = pointer_at(&fx.display).expect("xdotool");
        println!("x11_live out-of-bounds move landed at {at:?}");
        assert!(
            at.0 >= 0 && at.0 <= width as i32 - 1 && at.1 >= 0 && at.1 <= height as i32 - 1,
            "the pointer must stay on screen ({width}x{height}); xdotool reports {at:?}"
        );
        assert!(
            (at.0 - width as i32 + 1).abs() <= 2 && (at.1 - height as i32 + 1).abs() <= 2,
            "the move must be clamped to the bottom-right corner ({},{}); xdotool reports {at:?}",
            width - 1,
            height - 1
        );
    }
}
