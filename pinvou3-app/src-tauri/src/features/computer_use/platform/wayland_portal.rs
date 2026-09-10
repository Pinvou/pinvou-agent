//! Wayland 输入注入与同会话截屏:xdg-desktop-portal RemoteDesktop(+ScreenCast)。
//!
//! enigo 没有 Wayland 后端,XTest-through-XWayland 只能触达 X11 客户端,故
//! Wayland 输入合成的规范路径是 RemoteDesktop portal(GNOME/KDE 均有实现;
//! `AvailableDeviceTypes` 不含键盘/指针的合成器在探测阶段即显式不可用,不会
//! 静默退化)。portal 的系统授权对话框是独立于应用内同意流之外的第二层用户
//! 同意。
//!
//! 会话流程(懒启动,首次输入动作或截屏才触发,全程 `handle_token` 驱动
//! Request/Response 信号往返):
//! 1. `CreateSession` → Response 结果取 `session_handle`(返回值本身是
//!    Request 对象,历史包袱);
//! 2. `RemoteDesktop.SelectDevices`(types = KEYBOARD|POINTER);
//! 3. `ScreenCast.SelectSources`(monitor,multiple=false,cursor_mode=hidden)
//!    ——绝对移动的坐标系是绑定 stream 的逻辑空间,没有 stream 就没有绝对
//!    移动(mutter 对未知 stream 直接报错);
//! 4. `RemoteDesktop.Start`(弹系统授权对话框,用户决定后 Response 携带
//!    `devices` 与 `streams`);
//! 5. `ScreenCast.OpenPipeWireRemote`:用同一会话换取截屏流的 PipeWire fd,
//!    交给 [`super::wayland_capture`] 建帧接收器——截屏与输入共用这一个
//!    会话/授权,不再依赖 xcap 的旧截屏链(见 `linux.rs` 的回退说明);
//! 6. 输入全部走 `Notify*`;`Session.Close` 在 backend 析构时尽力调用——
//!    CreateSession 成功后的任何失败路径(请求错误/超时/用户取消/授权不含
//!    设备/无 stream)同样尽力关闭,不让半授权会话在合成器侧泄漏
//!    (见 [`PortalInner::abandon`])。
//!
//! 语义以 mutter `meta-remote-desktop-session.c` 为准:
//! - `NotifyPointerMotionAbsolute` 的 x/y(oa{sv}udd 的 d)是**流本地像素**
//!   (mutter `transform_position`:全局逻辑 = 显示器布局原点 + x/scale;
//!   KDE 由 portal 层补流的逻辑原点,scale=1 时两者一致)。流本地像素即
//!   PipeWire 协商的缓冲像素,故截屏(`linux.rs` 用协商尺寸构造 Capture)
//!   与输入共用同一坐标空间,origin 恒 (0,0)。
//! - `NotifyPointerAxisDiscrete`(oa{sv}ui):axis 0=垂直 1=水平;steps 正=
//!   下/右、负=上/左(`discrete_steps_to_scroll_direction`),一次可带多格;
//!   steps=0 会被 mutter 报 Invalid 并触发会话重置,调用方须先行挡下
//!   (见 `linux.rs` scroll 的 clicks=0 no-op)。
//! - 按键用 `NotifyKeyboardKeysym`(X keysym;仅命名键与 Latin-1 区——其余
//!   Unicode 虽有 `0x01000000 | 码点` 编码,但 mutter 对**不在当前 keymap
//!   内**的 keysym 静默丢弃,注入"成功"却无输入,故 [`char_keysym`] 对超出
//!   Latin-1 的字符显式报错,fail-closed)。
//! - 指针按钮为 evdev 按钮码;按下/释放 state=1/0。
//!
//! 线程约定:对象在 computer_use 专用 worker 线程构造/使用;对外是同步方法,
//! 内部经自持的 current-thread runtime `block_on` 驱动 async zbus(与
//! linux.rs 对 AT-SPI 的处理同一模式,无嵌套运行时)。

use std::collections::HashMap;
use std::future::poll_fn;
use std::os::fd::{AsFd as _, OwnedFd};
use std::pin::Pin;
use std::time::{Duration, Instant};

use zbus::export::futures_core;
use zbus::message::Type as MessageType;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use super::super::types::{ComputerUseError, Key, MouseButton, ScrollDirection};
use super::wayland_capture::{PortalFrame, PwCapture};

const PORTAL_DEST: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const REMOTE_DESKTOP_IFACE: &str = "org.freedesktop.portal.RemoteDesktop";
const SCREEN_CAST_IFACE: &str = "org.freedesktop.portal.ScreenCast";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";
const SESSION_IFACE: &str = "org.freedesktop.portal.Session";

/// 设备类型位(SelectDevices `types`、Start 响应 `devices` 共用)。
const DEVICE_KEYBOARD: u32 = 1;
const DEVICE_POINTER: u32 = 2;
/// ScreenCast 源类型:monitor。
const SOURCE_MONITOR: u32 = 1;
/// ScreenCast cursor_mode:hidden(输入合成不需要把光标编进流)。
const CURSOR_MODE_HIDDEN: u32 = 1;
/// 按下/释放的 portal state 值(指针按钮与键盘共用)。
const STATE_RELEASED: u32 = 0;
const STATE_PRESSED: u32 = 1;

/// 常规 portal 请求往返超时(授权对话框在 Start,不在这些步骤上)。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Start 会弹系统授权对话框,用户可能离开座位;超时后关闭会话、下次重试。
const START_TIMEOUT: Duration = Duration::from_secs(120);
/// Notify* 事件注入超时(会话被合成器撤销等异常时不能挂死 worker)。
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(10);
/// `Session.Close` 的尽力超时(析构/失败清理路径,不阻塞太久)。
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
/// portal 探测/建连的整体超时。zbus 对 session bus 的默认 method_timeout
/// 很宽,挂死的 portal 服务不得把 backend 构造无期挂起(评审发现:无界的
/// 属性查询会钉死 worker)。
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// evdev 按钮码(portal 规范:Linux evdev button codes)。
const EVDEV_BTN_LEFT: i32 = 0x110;
const EVDEV_BTN_RIGHT: i32 = 0x111;
const EVDEV_BTN_MIDDLE: i32 = 0x112;

/// types 的归一化按键 → X keysym(NotifyKeyboardKeysym 的编码空间)。
pub(super) fn map_keysym(key: Key) -> Result<i32, ComputerUseError> {
    let keysym: i32 = match key {
        Key::Control => 0xffe3, // Control_L
        Key::Alt => 0xffe9,     // Alt_L
        Key::Shift => 0xffe1,   // Shift_L
        // macOS Cmd / Windows Win / Linux Super。
        Key::Meta => 0xffeb,
        Key::Enter => 0xff0d,
        Key::Escape => 0xff1b,
        Key::Tab => 0xff09,
        Key::Space => 0x0020,
        Key::Backspace => 0xff08,
        Key::Delete => 0xffff,
        Key::Insert => 0xff63,
        Key::Up => 0xff52,
        Key::Down => 0xff54,
        Key::Left => 0xff51,
        Key::Right => 0xff53,
        Key::Home => 0xff50,
        Key::End => 0xff57,
        Key::PageUp => 0xff55,
        Key::PageDown => 0xff56,
        Key::Function(n) => match n {
            1..=12 => (0xffbe + u32::from(n) - 1) as i32, // F1..F12 连续段
            _ => {
                return Err(ComputerUseError::unsupported(
                    "input",
                    format!("function key F{n} is out of the mappable range F1-F12"),
                ));
            }
        },
        Key::Char(c) => char_keysym(c)?,
    };
    Ok(keysym)
}

/// type_text 逐字符注入与 `Key::Char` 共用的单字符 keysym(\n→Return、
/// \t→Tab;Latin-1 区 keysym 与码点一致)。
///
/// 超出 Latin-1 的字符**显式报错**(fail-closed):它们在 X keysym 里有
/// `0x01000000 | 码点` 编码,但 mutter 对**不在当前 keymap 内**的 keysym
/// 静默丢弃——逐字符调用全部"成功"、中文一个都进不去(评审发现的静默
/// 丢失)。宁可显式失败,也不假成功。
pub(super) fn char_keysym(c: char) -> Result<i32, ComputerUseError> {
    let code = u32::from(c);
    let keysym = keysym_for_char(c).ok_or_else(|| {
        if code > 0xff {
            ComputerUseError::unsupported(
                "input",
                format!(
                    "cannot type {c:?} (U+{code:04X}) via the Wayland portal: mutter \
                     silently drops keysyms outside the active keymap, so non-Latin-1 \
                     text (CJK etc.) would be lost; type ASCII/Latin-1 text or switch to \
                     a keymap that contains the character"
                ),
            )
        } else {
            ComputerUseError::unsupported(
                "input",
                format!("character {c:?} (U+{code:04X}) has no X keysym"),
            )
        }
    })?;
    i32::try_from(keysym)
        .map_err(|_| ComputerUseError::failed("keysym does not fit the portal i32 argument"))
}

/// 单字符 → keysym 的纯映射。非 Latin-1 返回 None(不编码为
/// `0x01000000 | 码点`:mutter 会静默丢弃 keymap 外的 keysym,注入等于假
/// 成功;理由与错误文案见 [`char_keysym`])。
fn keysym_for_char(c: char) -> Option<u32> {
    let code = u32::from(c);
    match code {
        0x0a => Some(0xff0d), // type_text 的换行注入为 Return
        0x09 => Some(0xff09),
        0x00 | 0x7f => None,
        0x01..=0xff => Some(code), // Latin-1 区 keysym 与码点一致
        _ => None,
    }
}

pub(super) fn map_button(button: MouseButton) -> i32 {
    match button {
        MouseButton::Left => EVDEV_BTN_LEFT,
        MouseButton::Right => EVDEV_BTN_RIGHT,
        MouseButton::Middle => EVDEV_BTN_MIDDLE,
    }
}

/// 滚动方向 → (portal axis, steps)。符号与 mutter 的
/// `discrete_steps_to_scroll_direction` 一致:正=下/右。
pub(super) fn map_discrete_scroll(direction: ScrollDirection, clicks: u32) -> (u32, i32) {
    let steps = i32::try_from(clicks).unwrap_or(i32::MAX);
    match direction {
        ScrollDirection::Up => (0, -steps),
        ScrollDirection::Down => (0, steps),
        ScrollDirection::Left => (1, -steps),
        ScrollDirection::Right => (1, steps),
    }
}

/// portal request 对象路径:`/org/freedesktop/portal/desktop/request/
/// <发送端唯一名去冒号、点变下划线>/<handle_token>`。
fn request_object_path(
    unique_name: &str,
    token: &str,
) -> Result<OwnedObjectPath, ComputerUseError> {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    let path = ObjectPath::try_from(format!(
        "/org/freedesktop/portal/desktop/request/{sender}/{token}"
    ))
    .map_err(|error| ComputerUseError::failed(format!("portal request path: {error}")))?;
    Ok(OwnedObjectPath::from(path))
}

/// 授权对话框被取消(1)或异常结束(其他)时的错误。
fn response_error(code: u32) -> ComputerUseError {
    match code {
        1 => ComputerUseError::unavailable(
            "the system authorization dialog was dismissed; the action was not granted",
        ),
        code => ComputerUseError::unavailable(format!(
            "the system authorization dialog ended unexpectedly (portal response code {code})"
        )),
    }
}

/// 从 Value 取 u32(OwnedValue Deref 到 Value,调用点直接享受强制解引用)。
fn value_u32(value: &Value<'_>) -> Option<u32> {
    match value {
        Value::U32(value) => Some(*value),
        _ => None,
    }
}

/// MessageStream 只实现 `futures_core::Stream`(经 `zbus::export` 再导出),
/// 不引新依赖,用 `poll_fn` 手写 `next`。
async fn next_message(stream: &mut zbus::MessageStream) -> Option<zbus::Result<zbus::Message>> {
    poll_fn(|cx| futures_core::stream::Stream::poll_next(Pin::new(stream), cx)).await
}

/// 剥掉任意层 variant 包裹(zvariant 对 a{sv} 的值可能存成 Value::Value)。
fn unwrap_variant<'a>(value: &'a Value<'a>) -> &'a Value<'a> {
    let mut value = value;
    while let Value::Value(inner) = value {
        value = inner;
    }
    value
}

/// a{sv} 里取 (i32, i32)(结构或两元素数组都接受)。
fn dict_i32_pair(dict: &zbus::zvariant::Dict<'_, '_>, key: &str) -> Option<(i32, i32)> {
    let entry = dict
        .iter()
        .find(|(dict_key, _)| {
            matches!(unwrap_variant(dict_key), Value::Str(name) if name.as_str() == key)
        })
        .map(|(_, value)| value)?;
    // 字典值还可能是 variant 包裹,统一剥掉。
    let value = unwrap_variant(entry);
    let ints = |fields: &[Value<'_>]| -> Option<(i32, i32)> {
        match fields {
            [Value::I32(a), Value::I32(b)] => Some((*a, *b)),
            _ => None,
        }
    };
    match value {
        Value::Structure(structure) => ints(structure.fields()),
        Value::Array(array) => {
            let fields: Vec<Value<'_>> = array.iter().cloned().collect();
            ints(&fields)
        }
        _ => None,
    }
}

/// Start 响应的 streams (a(ua{sv})):取首个 stream 的 PipeWire node id 与
/// 合成器报告的流逻辑尺寸(`size`,KDE 输入倍率换算用;合成器不填时为
/// None,输入倍率按 1.0 处理)。vardict 的值都可能是 variant 包裹的。
fn first_stream_info(results: &HashMap<String, OwnedValue>) -> Option<(u32, Option<(i32, i32)>)> {
    let Value::Array(array) = unwrap_variant(&**results.get("streams")?) else {
        return None;
    };
    let Value::Structure(structure) = array.iter().next()? else {
        return None;
    };
    let node = value_u32(unwrap_variant(structure.fields().first()?))?;
    let logical = structure.fields().get(1).and_then(|entry| {
        let Value::Dict(dict) = unwrap_variant(entry) else {
            return None;
        };
        dict_i32_pair(dict, "size")
    });
    Some((node, logical))
}

/// 已启动的 portal 会话状态。
struct PortalSession {
    path: OwnedObjectPath,
    /// 用户在授权对话框里实际授予的设备位掩码。
    devices: u32,
    /// 绑定的 ScreenCast stream(PipeWire node id),绝对移动的坐标参照。
    stream: u32,
    /// 合成器报告的流逻辑尺寸(诊断与 KDE 输入倍率换算用;可缺省)。
    stream_logical: Option<(i32, i32)>,
    /// 同会话截屏的 PipeWire 帧接收器(OpenPipeWireRemote 失败时为 None,
    /// 截屏回退 xcap 链,输入不受影响)。
    capture: Option<PwCapture>,
}

/// Wayland portal 输入后端(同步外观)。懒启动:首次输入动作或截屏才走
/// 会话建立与系统授权对话框;之后会话复用,失效自动重建。
pub(super) struct PortalInput {
    /// 专用 current-thread runtime:同步方法内驱动 async zbus(同 AT-SPI)。
    runtime: tokio::runtime::Runtime,
    inner: PortalInner,
}

impl PortalInput {
    /// 探测 portal 与 RemoteDesktop 输入支持(纯属性查询,不弹任何对话框)。
    /// 失败时返回人类可读原因,由调用方存为粘性错误。
    ///
    /// 整体受 [`PROBE_TIMEOUT`] 约束:zbus 默认 method_timeout 很宽,挂死的
    /// portal 服务不得把 backend 构造无期挂起(评审发现)。
    pub(super) fn probe() -> Result<(), String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("cannot create probe runtime: {error}"))?;
        runtime.block_on(async {
            match tokio::time::timeout(PROBE_TIMEOUT, Self::probe_async()).await {
                Ok(result) => result,
                Err(_) => Err(format!(
                    "portal probe did not finish within {PROBE_TIMEOUT:?}"
                )),
            }
        })
    }

    async fn probe_async() -> Result<(), String> {
        let conn = zbus::Connection::session()
            .await
            .map_err(|error| format!("cannot connect to the session bus: {error}"))?;
        let proxy = zbus::Proxy::new(&conn, PORTAL_DEST, PORTAL_PATH, REMOTE_DESKTOP_IFACE)
            .await
            .map_err(|error| format!("portal proxy: {error}"))?;
        let version: u32 = proxy
            .get_property("version")
            .await
            .map_err(|error| format!("cannot read portal RemoteDesktop version: {error}"))?;
        if version < 1 {
            return Err(format!(
                "xdg-desktop-portal RemoteDesktop is version {version}; this implementation \
                 needs the session-handle based interface (version >= 1)"
            ));
        }
        let available: u32 = proxy
            .get_property("AvailableDeviceTypes")
            .await
            .map_err(|error| format!("cannot read AvailableDeviceTypes: {error}"))?;
        if available & (DEVICE_KEYBOARD | DEVICE_POINTER) == 0 {
            return Err(
                "xdg-desktop-portal RemoteDesktop advertises no keyboard/pointer support for \
                 this compositor"
                    .to_string(),
            );
        }
        Ok(())
    }

    pub(super) fn new() -> Result<Self, ComputerUseError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                ComputerUseError::unavailable(format!("cannot create portal runtime: {error}"))
            })?;
        // 建连同样受 PROBE_TIMEOUT 约束(session bus 连接握手可能挂死)。
        let inner = runtime.block_on(async {
            match tokio::time::timeout(PROBE_TIMEOUT, PortalInner::new()).await {
                Ok(result) => result,
                Err(_) => Err(ComputerUseError::unavailable(format!(
                    "portal session bus connection did not finish within {PROBE_TIMEOUT:?}"
                ))),
            }
        })?;
        Ok(Self { runtime, inner })
    }

    /// 会话已启动则直接返回;否则走完整建立流程(弹系统授权对话框)。
    pub(super) fn ensure_started(&mut self) -> Result<(), ComputerUseError> {
        self.runtime.block_on(self.inner.ensure_started())
    }

    pub(super) fn motion_absolute(&mut self, x: i32, y: i32) -> Result<(), ComputerUseError> {
        self.runtime.block_on(self.inner.motion_absolute(x, y))
    }

    pub(super) fn button(
        &mut self,
        evdev_button: i32,
        pressed: bool,
    ) -> Result<(), ComputerUseError> {
        self.runtime
            .block_on(self.inner.button(evdev_button, pressed))
    }

    pub(super) fn axis_discrete(&mut self, axis: u32, steps: i32) -> Result<(), ComputerUseError> {
        self.runtime.block_on(self.inner.axis_discrete(axis, steps))
    }

    pub(super) fn keysym_event(
        &mut self,
        keysym: i32,
        pressed: bool,
    ) -> Result<(), ComputerUseError> {
        self.runtime
            .block_on(self.inner.keysym_event(keysym, pressed))
    }

    /// backend 析构时尽力关闭 portal 会话(授权授予的会话不应泄漏)。
    pub(super) fn close(&mut self) {
        self.runtime.block_on(self.inner.close());
    }

    /// 我方注入产生的光标位置(逻辑全局坐标);无 Wayland 查询 API,仅跟踪。
    pub(super) fn last_pointer(&self) -> Option<(i32, i32)> {
        self.inner.last_pointer
    }

    pub(super) fn track_pointer(&mut self, x: i32, y: i32) {
        self.inner.last_pointer = Some((x, y));
    }

    /// 同会话截屏:会话未启动则启动(可能弹系统授权对话框),再从绑定的
    /// ScreenCast 流取最新帧。`not_before` 语义见 [`PwCapture::latest_frame`]。
    pub(super) fn capture_frame(
        &mut self,
        not_before: Option<Instant>,
    ) -> Result<PortalFrame, ComputerUseError> {
        self.runtime.block_on(async {
            self.inner.ensure_started().await?;
            let session = self
                .inner
                .session
                .as_mut()
                .ok_or_else(|| ComputerUseError::unavailable("portal session not started"))?;
            match session.capture.as_mut() {
                Some(capture) => capture.latest_frame(not_before),
                None => Err(ComputerUseError::unavailable(format!(
                    "same-session capture is not available: {}",
                    self.inner
                        .capture_error
                        .as_deref()
                        .unwrap_or("OpenPipeWireRemote failed")
                ))),
            }
        })
    }

    /// 合成器报告的流逻辑尺寸(KDE 输入倍率换算用;可缺省)。
    pub(super) fn stream_logical_size(&self) -> Option<(i32, i32)> {
        self.inner
            .session
            .as_ref()
            .and_then(|session| session.stream_logical)
    }
}

/// portal 会话的 async 主体(状态与 D-Bus 连接;`PortalInput` 的 runtime 与
/// 此处分野,保证 `block_on` 与未来体借用互不相交)。
struct PortalInner {
    conn: zbus::Connection,
    session: Option<PortalSession>,
    /// 当前会话的截屏初始化失败原因(输入不受影响;截屏回退 xcap 链)。
    capture_error: Option<String>,
    /// 我方注入产生的光标位置(逻辑全局坐标)。Wayland 无查询 API,只能跟踪。
    last_pointer: Option<(i32, i32)>,
    request_counter: u32,
    /// CreateSession 的 session_handle_token 计数(路由器用它命名 session)。
    session_counter: u32,
}

impl PortalInner {
    async fn new() -> Result<Self, ComputerUseError> {
        let conn = zbus::Connection::session()
            .await
            .map_err(|error| ComputerUseError::unavailable(format!("session bus: {error}")))?;
        Ok(Self {
            conn,
            session: None,
            capture_error: None,
            last_pointer: None,
            request_counter: 0,
            session_counter: 0,
        })
    }

    fn next_handle_token(&mut self) -> String {
        self.request_counter = self.request_counter.wrapping_add(1);
        format!("pinvou_cu_{}", self.request_counter)
    }

    fn unique_sender(&self) -> Result<String, ComputerUseError> {
        self.conn
            .unique_name()
            .map(|name| name.to_string())
            .ok_or_else(|| ComputerUseError::unavailable("portal connection has no unique name"))
    }

    /// 发起一次 portal 请求并等待对应 Request 对象上的 Response 信号。
    /// 先订阅再调用,避免 Response 先于订阅到达的竞态。
    ///
    /// `session` 为 `Some` 时方法签名是 `(o session_handle, a{sv} options)`
    /// (CreateSession 之外全是);为 `None` 时是 `(a{sv} options)`。
    async fn request(
        &mut self,
        interface: &str,
        method: &'static str,
        session: Option<&OwnedObjectPath>,
        extra: Vec<(&'static str, OwnedValue)>,
        timeout: Duration,
    ) -> Result<HashMap<String, OwnedValue>, ComputerUseError> {
        self.request_impl(interface, method, session, None, extra, timeout)
            .await
    }

    /// `parent_window` 为 `Some` 时方法签名是 `(o session_handle,
    /// s parent_window, a{sv} options)`:实测 xdg-desktop-portal 1.18 的
    /// Start 是 session 在前、父窗口字符串(空串 = 无父窗)在后,与规范
    /// 文档的顺序相反,以 introspection 为准。缺参数会被 InvalidArgs 拒绝。
    #[allow(clippy::too_many_arguments)]
    async fn request_impl(
        &mut self,
        interface: &str,
        method: &'static str,
        session: Option<&OwnedObjectPath>,
        parent_window: Option<&str>,
        extra: Vec<(&'static str, OwnedValue)>,
        timeout: Duration,
    ) -> Result<HashMap<String, OwnedValue>, ComputerUseError> {
        let token = self.next_handle_token();
        let request_path = request_object_path(&self.unique_sender()?, &token)?;

        let rule = zbus::MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender(PORTAL_DEST)
            .and_then(|builder| builder.path(request_path.clone()))
            .and_then(|builder| builder.interface(REQUEST_IFACE))
            .and_then(|builder| builder.member("Response"))
            .map(|builder| builder.build())
            .map_err(|error| {
                ComputerUseError::unavailable(format!("portal response match rule: {error}"))
            })?;
        let mut stream = zbus::MessageStream::for_match_rule(rule, &self.conn, None)
            .await
            .map_err(|error| {
                ComputerUseError::unavailable(format!("portal response subscription: {error}"))
            })?;

        let mut options: HashMap<&str, OwnedValue> = HashMap::new();
        let token_value = Value::from(token.as_str())
            .try_to_owned()
            .map_err(|error| ComputerUseError::failed(format!("portal handle_token: {error}")))?;
        options.insert("handle_token", token_value);
        for (key, value) in extra {
            options.insert(key, value);
        }

        // 两个分支的 call_method 产出不同 opaque future 类型,各自 await 到
        // 完成后统一为 Result<Message>。
        tokio::time::timeout(timeout, async {
            match session {
                Some(path) if parent_window.is_some() => {
                    self.conn
                        .call_method(
                            Some(PORTAL_DEST),
                            PORTAL_PATH,
                            Some(interface),
                            method,
                            &(path, parent_window.unwrap_or_default(), &options),
                        )
                        .await
                }
                Some(path) => {
                    self.conn
                        .call_method(
                            Some(PORTAL_DEST),
                            PORTAL_PATH,
                            Some(interface),
                            method,
                            &(path, &options),
                        )
                        .await
                }
                None => {
                    self.conn
                        .call_method(
                            Some(PORTAL_DEST),
                            PORTAL_PATH,
                            Some(interface),
                            method,
                            &(&options,),
                        )
                        .await
                }
            }
        })
        .await
        .map_err(|_| {
            ComputerUseError::unavailable(format!("portal {method} timed out after {timeout:?}"))
        })?
        .map_err(|error| ComputerUseError::unavailable(format!("portal {method}: {error}")))?;

        let message = tokio::time::timeout(timeout, next_message(&mut stream))
            .await
            .map_err(|_| {
                ComputerUseError::unavailable(format!(
                    "portal {method} response timed out after {timeout:?}"
                ))
            })?
            .ok_or_else(|| {
                ComputerUseError::unavailable(format!("portal {method} response stream closed"))
            })?
            .map_err(|error| {
                ComputerUseError::unavailable(format!("portal {method} response: {error}"))
            })?;
        let (code, results): (u32, HashMap<String, OwnedValue>) =
            message.body().deserialize().map_err(|error| {
                ComputerUseError::unavailable(format!("portal {method} response body: {error}"))
            })?;
        if code != 0 {
            return Err(response_error(code));
        }
        Ok(results)
    }

    /// 完整建立流程(弹系统授权对话框)。CreateSession 成功后,任何后续
    /// 步骤失败(请求错误/超时/用户取消/授权不含设备/无 stream)都必须
    /// 关闭已创建的会话对象再返回(评审发现的泄漏路径,见 [`PortalInner::abandon`])。
    async fn ensure_started(&mut self) -> Result<(), ComputerUseError> {
        if self.session.is_some() {
            return Ok(());
        }

        // 1) CreateSession。`session_handle_token` 是路由器命名 session 对象
        //    的依据,xdg-desktop-portal ≥1.17 缺失即拒绝("Missing token")。
        self.session_counter = self.session_counter.wrapping_add(1);
        let results = self
            .request(
                REMOTE_DESKTOP_IFACE,
                "CreateSession",
                None,
                vec![(
                    "session_handle_token",
                    Value::from(format!("pinvou_cu_s{}", self.session_counter))
                        .try_to_owned()
                        .map_err(|error| {
                            ComputerUseError::failed(format!("portal session token: {error}"))
                        })?,
                )],
                REQUEST_TIMEOUT,
            )
            .await?;
        // 实测 xdg-desktop-portal 1.18 把 session_handle 作为字符串放进响应
        // (规范写的是 o);两种形式都接受。解析不出有效 handle 时无从关闭,
        // 响应畸形本身即错误(直接抛出)。
        let session_path = match results
            .get("session_handle")
            .map(|owned| unwrap_variant(owned))
        {
            Some(Value::ObjectPath(path)) => OwnedObjectPath::from(path.clone()),
            Some(Value::Str(path)) => {
                let path = ObjectPath::try_from(path.as_str()).map_err(|error| {
                    ComputerUseError::unavailable(format!(
                        "portal CreateSession response has an invalid session_handle: {error}"
                    ))
                })?;
                OwnedObjectPath::from(path)
            }
            _ => {
                return Err(ComputerUseError::unavailable(
                    "portal CreateSession response has a non-object session_handle",
                ));
            }
        };

        // 2) SelectDevices:keyboard + pointer。
        if let Err(error) = self
            .request(
                REMOTE_DESKTOP_IFACE,
                "SelectDevices",
                Some(&session_path),
                vec![("types", OwnedValue::from(DEVICE_KEYBOARD | DEVICE_POINTER))],
                REQUEST_TIMEOUT,
            )
            .await
        {
            self.abandon(session_path).await;
            return Err(error);
        }

        // 3) ScreenCast.SelectSources:绑定一个显示器流,绝对移动坐标才有参照。
        if let Err(error) = self
            .request(
                SCREEN_CAST_IFACE,
                "SelectSources",
                Some(&session_path),
                vec![
                    ("types", OwnedValue::from(SOURCE_MONITOR)),
                    ("multiple", OwnedValue::from(false)),
                    ("cursor_mode", OwnedValue::from(CURSOR_MODE_HIDDEN)),
                ],
                REQUEST_TIMEOUT,
            )
            .await
        {
            self.abandon(session_path).await;
            return Err(error);
        }

        // 4) Start:系统授权对话框(用户可见的第二层同意)。签名含父窗口;
        //    取消/超时同样要关闭会话。
        let results = match self
            .request_impl(
                REMOTE_DESKTOP_IFACE,
                "Start",
                Some(&session_path),
                Some(""),
                vec![],
                START_TIMEOUT,
            )
            .await
        {
            Ok(results) => results,
            Err(error) => {
                self.abandon(session_path).await;
                return Err(error);
            }
        };
        let devices = results
            .get("devices")
            .and_then(|owned| value_u32(unwrap_variant(owned)))
            .unwrap_or(0);
        if devices & (DEVICE_KEYBOARD | DEVICE_POINTER) == 0 {
            self.abandon(session_path).await;
            return Err(ComputerUseError::unavailable(
                "the system authorization dialog granted no keyboard/pointer devices",
            ));
        }
        let Some((stream, stream_logical)) = first_stream_info(&results) else {
            self.abandon(session_path).await;
            return Err(ComputerUseError::unavailable(
                "portal Start response has no ScreenCast stream; absolute pointer motion has \
                 no coordinate reference",
            ));
        };

        // 5) 同会话截屏:OpenPipeWireRemote 换 PipeWire fd 并建帧接收器。
        //    失败不放弃会话:输入仍可用,截屏回退 xcap 链。
        let (capture, capture_error) = match self.open_pipewire_remote(&session_path).await {
            Ok(fd) => match PwCapture::spawn(stream, fd) {
                Ok(capture) => (Some(capture), None),
                Err(error) => (None, Some(error.to_string())),
            },
            Err(error) => (None, Some(error.to_string())),
        };
        self.capture_error = capture_error;

        self.session = Some(PortalSession {
            path: session_path,
            devices,
            stream,
            stream_logical,
            capture,
        });
        self.last_pointer = None;
        Ok(())
    }

    /// `ScreenCast.OpenPipeWireRemote`:用当前会话换取截屏流的 PipeWire
    /// 私有连接 fd(同步快速方法,非 Request 往返)。
    async fn open_pipewire_remote(
        &self,
        session_path: &OwnedObjectPath,
    ) -> Result<OwnedFd, ComputerUseError> {
        let options: HashMap<&str, OwnedValue> = HashMap::new();
        let reply = tokio::time::timeout(
            REQUEST_TIMEOUT,
            self.conn.call_method(
                Some(PORTAL_DEST),
                PORTAL_PATH,
                Some(SCREEN_CAST_IFACE),
                "OpenPipeWireRemote",
                &(&session_path, &options),
            ),
        )
        .await
        .map_err(|_| ComputerUseError::unavailable("portal OpenPipeWireRemote timed out"))?
        .map_err(|error| {
            ComputerUseError::unavailable(format!("portal OpenPipeWireRemote: {error}"))
        })?;
        let fd = reply
            .body()
            .deserialize::<zbus::zvariant::OwnedFd>()
            .map_err(|error| {
                ComputerUseError::unavailable(format!(
                    "portal OpenPipeWireRemote returned no fd: {error}"
                ))
            })?;
        // portal 侧 fd 复制一份给 PipeWire 线程,zvariant 包装随消息释放。
        fd.as_fd()
            .try_clone_to_owned()
            .map_err(|error| ComputerUseError::unavailable(format!("fd clone: {error}")))
    }

    /// 解析并克隆会话路径(借用不能横跨 `notify` 的 `&mut self`:调用体里
    /// 还要再借 self.conn 发起方法调用)。
    fn session_path(&self) -> Result<OwnedObjectPath, ComputerUseError> {
        self.session
            .as_ref()
            .map(|session| session.path.clone())
            .ok_or_else(|| ComputerUseError::unavailable("portal session not started"))
    }

    /// Notify* 事件注入(无 Response 往返,方法返回即送达)。会话中途失效
    /// (合成器撤销、portal 重启)时清空会话状态,下次动作重建并重新授权。
    async fn notify(
        &mut self,
        method: &'static str,
        body: impl serde::ser::Serialize + zbus::zvariant::DynamicType,
    ) -> Result<(), ComputerUseError> {
        let call = self.conn.call_method(
            Some(PORTAL_DEST),
            PORTAL_PATH,
            Some(REMOTE_DESKTOP_IFACE),
            method,
            &body,
        );
        match tokio::time::timeout(NOTIFY_TIMEOUT, call).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => {
                self.reset();
                Err(ComputerUseError::unavailable(format!(
                    "portal {method}: {error} (session reset; the next input action reopens \
                     the authorization dialog)"
                )))
            }
            Err(_) => {
                self.reset();
                Err(ComputerUseError::unavailable(format!(
                    "portal {method} timed out after {NOTIFY_TIMEOUT:?} (session reset)"
                )))
            }
        }
    }

    fn reset(&mut self) {
        // session 置 None 会连带 drop PwCapture(停流并 join 其线程);
        // PortalSession 的 Drop 路径在 close/reset 共用。
        self.session = None;
        self.capture_error = None;
        self.last_pointer = None;
    }

    /// 关闭一个已创建但尚未入册(`self.session`)或正在销毁的 portal 会话:
    /// 尽力 `Session.Close`(短超时,关闭自身的错误被吞——调用方的原始错误
    /// 照常向上抛),并清掉指针跟踪。评审发现:CreateSession 成功后的失败
    /// 路径若只 reset 本地状态,半授权会话会在合成器侧泄漏。
    async fn abandon(&mut self, session_path: OwnedObjectPath) {
        let _ = tokio::time::timeout(
            CLOSE_TIMEOUT,
            self.conn.call_method(
                Some(PORTAL_DEST),
                session_path,
                Some(SESSION_IFACE),
                "Close",
                &(),
            ),
        )
        .await;
        self.last_pointer = None;
    }

    /// 尽力关闭 portal 会话(backend 析构时)。
    async fn close(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        self.abandon(session.path).await;
    }

    async fn motion_absolute(&mut self, x: i32, y: i32) -> Result<(), ComputerUseError> {
        let path = self.session_path()?;
        let options: HashMap<&str, OwnedValue> = HashMap::new();
        let stream = self.session.as_ref().map(|s| s.stream).unwrap_or_default();
        let body = (&path, &options, stream, f64::from(x), f64::from(y));
        self.notify("NotifyPointerMotionAbsolute", body).await
    }

    async fn button(&mut self, evdev_button: i32, pressed: bool) -> Result<(), ComputerUseError> {
        let state = if pressed {
            STATE_PRESSED
        } else {
            STATE_RELEASED
        };
        let path = self.session_path()?;
        let options: HashMap<&str, OwnedValue> = HashMap::new();
        let body = (&path, &options, evdev_button, state);
        self.notify("NotifyPointerButton", body).await
    }

    async fn axis_discrete(&mut self, axis: u32, steps: i32) -> Result<(), ComputerUseError> {
        let path = self.session_path()?;
        let options: HashMap<&str, OwnedValue> = HashMap::new();
        let body = (&path, &options, axis, steps);
        self.notify("NotifyPointerAxisDiscrete", body).await
    }

    async fn keysym_event(&mut self, keysym: i32, pressed: bool) -> Result<(), ComputerUseError> {
        let state = if pressed {
            STATE_PRESSED
        } else {
            STATE_RELEASED
        };
        let path = self.session_path()?;
        let options: HashMap<&str, OwnedValue> = HashMap::new();
        let body = (&path, &options, keysym, state);
        self.notify("NotifyKeyboardKeysym", body).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keysym_covers_named_keys() {
        assert_eq!(map_keysym(Key::Enter).ok(), Some(0xff0d));
        assert_eq!(map_keysym(Key::Control).ok(), Some(0xffe3));
        assert_eq!(map_keysym(Key::Meta).ok(), Some(0xffeb));
        assert_eq!(map_keysym(Key::Up).ok(), Some(0xff52));
        assert_eq!(map_keysym(Key::PageDown).ok(), Some(0xff56));
        assert_eq!(map_keysym(Key::Space).ok(), Some(0x20));
    }

    #[test]
    fn latin1_chars_map_and_non_latin1_fail_closed() {
        // Latin-1 区:keysym 与码点一致,维持注入。
        assert_eq!(map_keysym(Key::Char('s')).ok(), Some(0x73));
        assert_eq!(map_keysym(Key::Char('S')).ok(), Some(0x53));
        assert_eq!(map_keysym(Key::Char('+')).ok(), Some(0x2b));
        // U+4E2D 中:旧实现编码为 0x01000000|码点照发,但 mutter 对 keymap 外
        // keysym 静默丢弃——逐字符"成功"却无输入(评审发现的 CJK 静默丢失)。
        // 现显式报错,错误文案说明 Wayland 限制。
        let error = map_keysym(Key::Char('中')).unwrap_err().to_string();
        assert!(error.contains("Wayland"), "{error}");
        assert!(error.contains("drops keysyms"), "{error}");
        assert!(error.contains("U+4E2D"), "{error}");
        let error = char_keysym('\u{1F600}').unwrap_err().to_string();
        assert!(error.contains("drops keysyms"), "{error}");
        // 换行/制表映射为命名键;NUL 与 DEL 无 keysym。
        assert_eq!(keysym_for_char('\n'), Some(0xff0d));
        assert_eq!(keysym_for_char('\t'), Some(0xff09));
        assert_eq!(keysym_for_char('\0'), None);
        // Latin-1 边界:0xFF(ÿ)可注入,0x100(Ā)报错。
        assert_eq!(char_keysym('\u{FF}').ok(), Some(0xff));
        assert!(char_keysym('\u{100}').is_err());
    }

    #[test]
    fn keysym_function_keys_f1_to_f12() {
        assert_eq!(map_keysym(Key::Function(1)).ok(), Some(0xffbe));
        assert_eq!(map_keysym(Key::Function(12)).ok(), Some(0xffc9));
        assert!(map_keysym(Key::Function(13)).is_err());
        assert!(map_keysym(Key::Function(0)).is_err());
    }

    #[test]
    fn buttons_use_evdev_codes() {
        assert_eq!(map_button(MouseButton::Left), 0x110);
        assert_eq!(map_button(MouseButton::Right), 0x111);
        assert_eq!(map_button(MouseButton::Middle), 0x112);
    }

    #[test]
    fn discrete_scroll_signs_match_mutter() {
        // mutter discrete_steps_to_scroll_direction:正=下/右,负=上/左。
        assert_eq!(map_discrete_scroll(ScrollDirection::Up, 3), (0, -3));
        assert_eq!(map_discrete_scroll(ScrollDirection::Down, 3), (0, 3));
        assert_eq!(map_discrete_scroll(ScrollDirection::Left, 1), (1, -1));
        assert_eq!(map_discrete_scroll(ScrollDirection::Right, 1), (1, 1));
        // amount=0 映射为 0 步:调用方(linux.rs)须在 portal 分支先行 no-op,
        // 否则 mutter 对 steps=0 报 Invalid,notify() 会重置整个会话。
        assert_eq!(map_discrete_scroll(ScrollDirection::Up, 0), (0, 0));
        assert_eq!(map_discrete_scroll(ScrollDirection::Right, 0), (1, 0));
    }

    #[test]
    fn request_path_mangles_sender_and_token() {
        let path = request_object_path(":1.42", "pinvou_cu_1").unwrap();
        assert_eq!(
            path.as_str(),
            "/org/freedesktop/portal/desktop/request/1_42/pinvou_cu_1"
        );
    }

    #[test]
    fn response_codes_map_to_unavailable() {
        for code in [1u32, 2, 7] {
            let error = response_error(code);
            assert!(error.to_string().starts_with("unavailable:"), "{error}");
        }
    }

    #[test]
    fn stream_info_is_extracted_from_start_results() {
        // 真实 Start 响应的 streams 是 a(ua{sv}):元素为 (node u, a{sv}) 结构。
        // 注意不能经 Value::from(Vec<Value>) 构造——那会把每个元素再包一层
        // variant(Value::Value),形状变成 a(v)。
        let element_signature = zbus::zvariant::Signature::try_from("(ua{sv})").expect("sig");
        let mut array = zbus::zvariant::Array::new(&element_signature);
        let mut stream_props: HashMap<String, Value<'static>> = HashMap::new();
        // vardict 值按线上格式是 variant 包裹的:Value::Value((i32,i32))。
        stream_props.insert(
            "size".to_string(),
            Value::Value(Box::new(Value::from((1920, 1080)))),
        );
        array
            .append(Value::Structure(zbus::zvariant::Structure::from((
                7u32,
                stream_props,
            ))))
            .expect("append");
        let mut results: HashMap<String, OwnedValue> = HashMap::new();
        // Response 响应字典的值同样是 variant 包裹:Value::Value(a(ua{sv}))。
        results.insert(
            "streams".to_string(),
            Value::Value(Box::new(Value::Array(array)))
                .try_to_owned()
                .expect("owned"),
        );
        let (node, logical) = first_stream_info(&results).expect("stream info");
        assert_eq!(node, 7);
        assert_eq!(logical, Some((1920, 1080)));
        // 无 size 属性(合成器不填)时 node 仍可用。
        let bare = first_stream_info(&empty_streams(7, true));
        assert_eq!(bare.map(|(node, _)| node), Some(7));
        assert_eq!(first_stream_info(&HashMap::new()), None);
    }

    /// 构造只有 node id、(可选)空属性字典的 streams 响应。
    fn empty_streams(node: u32, with_props: bool) -> HashMap<String, OwnedValue> {
        let element_signature = zbus::zvariant::Signature::try_from("(ua{sv})").expect("sig");
        let mut array = zbus::zvariant::Array::new(&element_signature);
        let props: HashMap<String, Value<'static>> = HashMap::new();
        array
            .append(Value::Structure(zbus::zvariant::Structure::from((
                node,
                if with_props { props } else { HashMap::new() },
            ))))
            .expect("append");
        let mut results: HashMap<String, OwnedValue> = HashMap::new();
        results.insert(
            "streams".to_string(),
            Value::Array(array).try_to_owned().expect("owned"),
        );
        results
    }

    /// 真机验证:portal RemoteDesktop 可达且声明键盘/指针支持(只读属性,
    /// 不建会话不弹窗)。CI 无用户会话总线,仅在本地跑:
    /// `cargo test --lib computer_use::platform::wayland_portal -- --ignored`
    #[test]
    #[ignore = "needs a desktop session bus with xdg-desktop-portal"]
    fn portal_probe_succeeds_on_live_session() {
        PortalInput::probe().expect("portal RemoteDesktop input should be available");
    }
}
