//! Wayland 输入注入:xdg-desktop-portal RemoteDesktop。
//!
//! enigo 没有 Wayland 后端,XTest-through-XWayland 只能触达 X11 客户端,故
//! Wayland 输入合成的规范路径是 RemoteDesktop portal(GNOME/KDE 均有实现;
//! `AvailableDeviceTypes` 不含键盘/指针的合成器在探测阶段即显式不可用,不会
//! 静默退化)。portal 的系统授权对话框是独立于应用内同意流之外的第二层用户
//! 同意。
//!
//! 会话流程(懒启动,首次输入动作才触发,全程 `handle_token` 驱动
//! Request/Response 信号往返):
//! 1. `CreateSession` → Response 结果取 `session_handle`(返回值本身是
//!    Request 对象,历史包袱);
//! 2. `RemoteDesktop.SelectDevices`(types = KEYBOARD|POINTER);
//! 3. `ScreenCast.SelectSources`(monitor,multiple=false,cursor_mode=hidden)
//!    ——绝对移动的坐标系是绑定 stream 的逻辑空间,没有 stream 就没有绝对
//!    移动(mutter 对未知 stream 直接报错);
//! 4. `RemoteDesktop.Start`(弹系统授权对话框,用户决定后 Response 携带
//!    `devices` 与 `streams`);
//! 5. 输入全部走 `Notify*`;`Session.Close` 在 backend 析构时尽力调用。
//!
//! 语义以 mutter `meta-remote-desktop-session.c` 为准:
//! - `NotifyPointerMotionAbsolute` 的 x/y(oa{sv}udd 的 d)在 stream 的逻辑
//!   坐标空间(`meta_screen_cast_stream_transform_position`),与 AT-SPI
//!   `CoordType::Screen` 同为全局逻辑空间。故 linux.rs 在 Wayland 下把
//!   `Capture.input_scale` 设为 `1/scale`、origin 保持逻辑坐标,ScaleMap 的
//!   输出即本后端的输入坐标。
//! - `NotifyPointerAxisDiscrete`(oa{sv}ui):axis 0=垂直 1=水平;steps 正=
//!   下/右、负=上/左(`discrete_steps_to_scroll_direction`),一次可带多格。
//! - 按键用 `NotifyKeyboardKeysym`(X keysym;U+0000..=U+00FF 用码点,其余
//!   Unicode 为 `0x01000000 | 码点`,同 libxkbcommon `xkb_keysym_from_utf32`)。
//! - 指针按钮为 evdev 按钮码;按下/释放 state=1/0。
//!
//! 线程约定:对象在 computer_use 专用 worker 线程构造/使用;对外是同步方法,
//! 内部经自持的 current-thread runtime `block_on` 驱动 async zbus(与
//! linux.rs 对 AT-SPI 的处理同一模式,无嵌套运行时)。

use std::collections::HashMap;
use std::future::poll_fn;
use std::pin::Pin;
use std::time::Duration;

use zbus::export::futures_core;
use zbus::message::Type as MessageType;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use super::super::types::{ComputerUseError, Key, MouseButton, ScrollDirection};

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

/// evdev 按钮码(portal 规范:Linux evdev button codes)。
const EVDEV_BTN_LEFT: i32 = 0x110;
const EVDEV_BTN_RIGHT: i32 = 0x111;
const EVDEV_BTN_MIDDLE: i32 = 0x112;

/// types 的归一化按键 → X keysym(NotifyKeyboardKeysym 的编码空间)。
pub(super) fn map_keysym(key: Key) -> Result<i32, ComputerUseError> {
    let keysym: u32 = match key {
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
            1..=12 => 0xffbe + u32::from(n) - 1, // F1..F12 连续段
            _ => {
                return Err(ComputerUseError::unsupported(
                    "input",
                    format!("function key F{n} is out of the mappable range F1-F12"),
                ));
            }
        },
        Key::Char(c) => keysym_for_char(c).ok_or_else(|| {
            ComputerUseError::unsupported(
                "input",
                format!("character {c:?} (U+{:04X}) has no X keysym", c as u32),
            )
        })?,
    };
    i32::try_from(keysym)
        .map_err(|_| ComputerUseError::failed("keysym does not fit the portal i32 argument"))
}

/// type_text 逐字符注入用的 keysym(\n→Return、\t→Tab;不可映射字符显式报错)。
pub(super) fn char_keysym(c: char) -> Result<i32, ComputerUseError> {
    let keysym = keysym_for_char(c).ok_or_else(|| {
        ComputerUseError::unsupported(
            "input",
            format!("character {c:?} (U+{:04X}) has no X keysym", c as u32),
        )
    })?;
    i32::try_from(keysym)
        .map_err(|_| ComputerUseError::failed("keysym does not fit the portal i32 argument"))
}

fn keysym_for_char(c: char) -> Option<u32> {
    let code = u32::from(c);
    match code {
        0x0a => Some(0xff0d), // type_text 的换行注入为 Return
        0x09 => Some(0xff09),
        0x00 | 0x7f => None,
        0x01..=0xff => Some(code), // Latin-1 区 keysym 与码点一致
        _ => Some(0x0100_0000 | code),
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

/// Start 响应的 streams (a(ua{sv})):取首个 stream 的 PipeWire node id。
fn first_stream_node(results: &HashMap<String, OwnedValue>) -> Option<u32> {
    let Value::Array(array) = results.get("streams").map(|owned| &**owned)? else {
        return None;
    };
    let Value::Structure(structure) = array.iter().next()? else {
        return None;
    };
    value_u32(structure.fields().first()?)
}

/// 已启动的 portal 会话状态。
struct PortalSession {
    path: OwnedObjectPath,
    /// 用户在授权对话框里实际授予的设备位掩码。
    devices: u32,
    /// 绑定的 ScreenCast stream(PipeWire node id),绝对移动的坐标参照。
    stream: u32,
}

/// Wayland portal 输入后端(同步外观)。懒启动:首次输入动作才走会话建立
/// 与系统授权对话框;之后会话复用,失效自动重建。
pub(super) struct PortalInput {
    /// 专用 current-thread runtime:同步方法内驱动 async zbus(同 AT-SPI)。
    runtime: tokio::runtime::Runtime,
    inner: PortalInner,
}

impl PortalInput {
    /// 探测 portal 与 RemoteDesktop 输入支持(纯属性查询,不弹任何对话框)。
    /// 失败时返回人类可读原因,由调用方存为粘性错误。
    pub(super) fn probe() -> Result<(), String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("cannot create probe runtime: {error}"))?;
        runtime.block_on(Self::probe_async())
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
        let inner = runtime.block_on(PortalInner::new())?;
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
}

/// portal 会话的 async 主体(状态与 D-Bus 连接;`PortalInput` 的 runtime 与
/// 此处分野,保证 `block_on` 与未来体借用互不相交)。
struct PortalInner {
    conn: zbus::Connection,
    session: Option<PortalSession>,
    /// 我方注入产生的光标位置(逻辑全局坐标)。Wayland 无查询 API,只能跟踪。
    last_pointer: Option<(i32, i32)>,
    request_counter: u32,
}

impl PortalInner {
    async fn new() -> Result<Self, ComputerUseError> {
        let conn = zbus::Connection::session()
            .await
            .map_err(|error| ComputerUseError::unavailable(format!("session bus: {error}")))?;
        Ok(Self {
            conn,
            session: None,
            last_pointer: None,
            request_counter: 0,
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

    /// 完整建立流程(弹系统授权对话框)。
    async fn ensure_started(&mut self) -> Result<(), ComputerUseError> {
        if self.session.is_some() {
            return Ok(());
        }

        // 1) CreateSession。
        let results = self
            .request(
                REMOTE_DESKTOP_IFACE,
                "CreateSession",
                None,
                vec![],
                REQUEST_TIMEOUT,
            )
            .await?;
        let session_path = match results.get("session_handle") {
            Some(owned) => match &**owned {
                Value::ObjectPath(path) => OwnedObjectPath::from(path.clone()),
                _ => {
                    return Err(ComputerUseError::unavailable(
                        "portal CreateSession response has a non-object session_handle",
                    ));
                }
            },
            None => {
                return Err(ComputerUseError::unavailable(
                    "portal CreateSession response has no session_handle",
                ));
            }
        };

        // 2) SelectDevices:keyboard + pointer。
        self.request(
            REMOTE_DESKTOP_IFACE,
            "SelectDevices",
            Some(&session_path),
            vec![("types", OwnedValue::from(DEVICE_KEYBOARD | DEVICE_POINTER))],
            REQUEST_TIMEOUT,
        )
        .await?;

        // 3) ScreenCast.SelectSources:绑定一个显示器流,绝对移动坐标才有参照。
        self.request(
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
        .await?;

        // 4) Start:系统授权对话框(用户可见的第二层同意)。
        let results = self
            .request(
                REMOTE_DESKTOP_IFACE,
                "Start",
                Some(&session_path),
                vec![],
                START_TIMEOUT,
            )
            .await?;
        let devices = results
            .get("devices")
            .and_then(|owned| value_u32(owned))
            .unwrap_or(0);
        if devices & (DEVICE_KEYBOARD | DEVICE_POINTER) == 0 {
            self.reset();
            return Err(ComputerUseError::unavailable(
                "the system authorization dialog granted no keyboard/pointer devices",
            ));
        }
        let Some(stream) = first_stream_node(&results) else {
            self.reset();
            return Err(ComputerUseError::unavailable(
                "portal Start response has no ScreenCast stream; absolute pointer motion has \
                 no coordinate reference",
            ));
        };

        self.session = Some(PortalSession {
            path: session_path,
            devices,
            stream,
        });
        self.last_pointer = None;
        Ok(())
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
        self.session = None;
        self.last_pointer = None;
    }

    /// 尽力关闭 portal 会话。
    async fn close(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        self.last_pointer = None;
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            self.conn.call_method(
                Some(PORTAL_DEST),
                session.path,
                Some(SESSION_IFACE),
                "Close",
                &(),
            ),
        )
        .await;
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
    fn keysym_covers_chars_via_latin1_and_unicode_ranges() {
        assert_eq!(map_keysym(Key::Char('s')).ok(), Some(0x73));
        assert_eq!(map_keysym(Key::Char('S')).ok(), Some(0x53));
        assert_eq!(map_keysym(Key::Char('+')).ok(), Some(0x2b));
        // U+4E2D 中:超出 Latin-1,取 0x01000000 | 码点。
        assert_eq!(map_keysym(Key::Char('中')).ok(), Some(0x0100_0000 | 0x4e2d));
        // 换行/制表映射为命名键;NUL 与 DEL 无 keysym。
        assert_eq!(keysym_for_char('\n'), Some(0xff0d));
        assert_eq!(keysym_for_char('\t'), Some(0xff09));
        assert_eq!(keysym_for_char('\0'), None);
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
    fn stream_node_is_extracted_from_start_results() {
        // 真实 Start 响应的 streams 是 a(ua{sv}):元素为 (node u, a{sv}) 结构。
        // 注意不能经 Value::from(Vec<Value>) 构造——那会把每个元素再包一层
        // variant(Value::Value),形状变成 a(v)。
        let element_signature = zbus::zvariant::Signature::try_from("(ua{sv})").expect("sig");
        let mut array = zbus::zvariant::Array::new(&element_signature);
        array
            .append(Value::Structure(zbus::zvariant::Structure::from((
                7u32,
                HashMap::<String, Value<'static>>::new(),
            ))))
            .expect("append");
        let mut results: HashMap<String, OwnedValue> = HashMap::new();
        results.insert(
            "streams".to_string(),
            Value::Array(array).try_to_owned().expect("owned"),
        );
        assert_eq!(first_stream_node(&results), Some(7));
        assert_eq!(first_stream_node(&HashMap::new()), None);
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
