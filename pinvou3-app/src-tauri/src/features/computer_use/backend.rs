//! 后端 trait 与专用 worker 线程。
//!
//! xcap/enigo/平台 a11y 对象全部同步且有线程亲和性（且内部含 unsafe FFI），
//! 因此所有后端工作跑在**每个会话一条**的专用 worker 线程上：backend 对象在该
//! 线程上构造、使用、析构，永不跨线程移动。`BackendHandle` 是可克隆的通道封装，
//! 工具层（async）用 `spawn_blocking` 调它的同步方法。
//!
//! 线程通过 `BackendHandle` 全部析构时的 Drop 发送 `Shutdown` 并 join 退出。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::Mutex;

/// 单次 backend 请求的等待上限。必须覆盖**同一条**懒启动路径的最坏合法
/// 组合（评审第三轮发现：常数论证要按求和做，不能只看单项）：portal 探测
/// 建连 5s + Start 方法调用 30s + 授权对话框 120s + `hold_key` 30s = 185s，
/// 取 190s 留余量。旧值 150s 恰好等于「120+30」的字面拼凑，同帧懒启动的
/// hold_key 会假超时——调用方误判失败并重试，而已出队不可中断的僵尸请求
/// 仍会执行，产生双重注入。此前的历史评审发现仍然成立：`recv()` 无上限会让
/// 一个挂死的 XTEST/CGEvent/portal 调用让该会话后续所有请求永远排队。
/// 超时只释放调用方；超时/断连后调用方置位请求的取消旗标，worker 在出队
/// 后、执行前检查，已取消的请求不再执行。剩余缺口（固有限制，如实记录）：
/// 已进入 OS 调用（XTEST/CGEvent/portal）的请求无法中断，只能在请求边界
/// 拦截；worker 线程若仍卡在 OS 调用里，后续请求会被 in-flight 旗标拒绝
/// 而不是排队占线程。
const BACKEND_CALL_TIMEOUT: Duration = Duration::from_secs(190);

use super::types::{
    Capabilities, Capture, ComputerUseError, ElementInfo, Key, MouseButton, ScrollDirection,
    UiTreeOptions,
};

/// 平台后端契约。所有方法同步、`Result` 驱动；平台缺少能力时返回
/// `ComputerUseError::unsupported`，不得静默降级或复用其他平台实现。
///
/// 坐标约定：
/// - `capture` 返回设备物理像素 + 输入倍率（见 [`Capture`]）。
/// - `cursor_position` 返回全局设备物理像素。
/// - `move_to` / `click` / `drag` / `scroll` / `element_at_point` 的坐标参数是
///   **输入坐标空间**（Windows 物理像素；macOS CGEvent 点）。
pub trait ComputerUseBackend: Send {
    fn capabilities(&self) -> Capabilities;
    fn capture(&mut self) -> Result<Capture, ComputerUseError>;
    fn cursor_position(&mut self) -> Result<(i32, i32), ComputerUseError>;
    fn move_to(&mut self, x: i32, y: i32) -> Result<(), ComputerUseError>;
    /// count: 1=单击 2=双击 3=三击。
    fn click(&mut self, button: MouseButton, count: u8) -> Result<(), ComputerUseError>;
    fn mouse_down(&mut self, button: MouseButton) -> Result<(), ComputerUseError>;
    fn mouse_up(&mut self, button: MouseButton) -> Result<(), ComputerUseError>;
    fn drag(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<(), ComputerUseError>;
    /// clicks: 滚轮格数（正数，方向由 direction 给出）。
    fn scroll(&mut self, direction: ScrollDirection, clicks: u32) -> Result<(), ComputerUseError>;
    fn type_text(&mut self, text: &str) -> Result<(), ComputerUseError>;
    /// 按下全部键再逆序释放（和弦）。
    fn key_chord(&mut self, keys: &[Key]) -> Result<(), ComputerUseError>;
    /// 按住和弦 ms 毫秒后释放。
    fn hold_key(&mut self, keys: &[Key], ms: u64) -> Result<(), ComputerUseError>;
    /// 无障碍树序列化文本（格式由后端定，建议缩进文本树）。
    fn ui_tree(&mut self, opts: &UiTreeOptions) -> Result<String, ComputerUseError>;
    fn element_at_point(&mut self, x: i32, y: i32)
    -> Result<Option<ElementInfo>, ComputerUseError>;
    /// 返回当前持有键盘焦点的元素（用于键盘类动作的 T3 筛查）。
    /// Ok(None) = 明确无焦点元素；Err = 查询失败或平台不支持（调用方按
    /// fail-closed 处理）。
    fn focused_element(&mut self) -> Result<Option<ElementInfo>, ComputerUseError> {
        let _ = &mut *self;
        Err(ComputerUseError::unsupported(
            "focused_element",
            "this backend cannot query the keyboard-focused element",
        ))
    }
    /// 用户撤销会话授权 / 全局停止 / 总开关关闭时调用：关闭该后端持有的
    /// **持久性 OS 级授权**（评审发现：Wayland RemoteDesktop 的 portal 会话
    /// 是系统级输入授权，revoke/stop 只清应用内状态会让它活到进程退出，
    /// 与"可随时停止"的用户可见承诺不符）。X11 XTEST、Windows SendInput、
    /// macOS CGEvent 无持久授权，默认 no-op。调用后后端必须保持可用：
    /// 下次动作按既有路径懒重建授权（需要时用户会看到系统授权对话框）。
    fn release_os_grant(&mut self) -> Result<(), ComputerUseError> {
        let _ = &mut *self;
        Ok(())
    }
}

enum BackendRequestKind {
    Capabilities,
    Capture,
    CursorPosition,
    MoveTo {
        x: i32,
        y: i32,
    },
    Click {
        button: MouseButton,
        count: u8,
    },
    MouseDown {
        button: MouseButton,
    },
    MouseUp {
        button: MouseButton,
    },
    Drag {
        from: (i32, i32),
        to: (i32, i32),
    },
    Scroll {
        direction: ScrollDirection,
        clicks: u32,
    },
    TypeText {
        text: String,
    },
    KeyChord {
        keys: Vec<Key>,
    },
    HoldKey {
        keys: Vec<Key>,
        ms: u64,
    },
    UiTree {
        opts: UiTreeOptions,
    },
    ElementAtPoint {
        x: i32,
        y: i32,
    },
    FocusedElement,
    ReleaseOsGrant,
    Shutdown,
}

enum BackendReply {
    Capabilities(Capabilities),
    Capture(Capture),
    CursorPosition((i32, i32)),
    Unit,
    Tree(String),
    Element(Option<ElementInfo>),
}

type BackendResult = Result<BackendReply, ComputerUseError>;

struct BackendRequest {
    kind: BackendRequestKind,
    reply: Sender<BackendResult>,
    /// 取消旗标：caller 与请求各持一半 `Arc`。调用方超时/通道断开放弃后
    /// 置位；worker 在出队后、执行前检查，已取消的请求不再执行（评审发现：
    /// 调用方超时返回后请求仍被照常执行——此刻物理输入锁已释放、guard 不会
    /// 复检，其他会话可能同时在注入，击穿跨会话全程串行化保证，且模型已被
    /// 误导「动作失败」）。
    cancelled: Arc<AtomicBool>,
}

fn dispatch(
    backend: &mut dyn ComputerUseBackend,
    kind: BackendRequestKind,
) -> Option<BackendResult> {
    let result = match kind {
        BackendRequestKind::Capabilities => BackendReply::Capabilities(backend.capabilities()),
        BackendRequestKind::Capture => match backend.capture() {
            Ok(capture) => BackendReply::Capture(capture),
            Err(error) => return Some(Err(error)),
        },
        BackendRequestKind::CursorPosition => match backend.cursor_position() {
            Ok(pos) => BackendReply::CursorPosition(pos),
            Err(error) => return Some(Err(error)),
        },
        BackendRequestKind::MoveTo { x, y } => match backend.move_to(x, y) {
            Ok(()) => BackendReply::Unit,
            Err(error) => return Some(Err(error)),
        },
        BackendRequestKind::Click { button, count } => match backend.click(button, count) {
            Ok(()) => BackendReply::Unit,
            Err(error) => return Some(Err(error)),
        },
        BackendRequestKind::MouseDown { button } => match backend.mouse_down(button) {
            Ok(()) => BackendReply::Unit,
            Err(error) => return Some(Err(error)),
        },
        BackendRequestKind::MouseUp { button } => match backend.mouse_up(button) {
            Ok(()) => BackendReply::Unit,
            Err(error) => return Some(Err(error)),
        },
        BackendRequestKind::Drag { from, to } => match backend.drag(from, to) {
            Ok(()) => BackendReply::Unit,
            Err(error) => return Some(Err(error)),
        },
        BackendRequestKind::Scroll { direction, clicks } => {
            match backend.scroll(direction, clicks) {
                Ok(()) => BackendReply::Unit,
                Err(error) => return Some(Err(error)),
            }
        }
        BackendRequestKind::TypeText { text } => match backend.type_text(&text) {
            Ok(()) => BackendReply::Unit,
            Err(error) => return Some(Err(error)),
        },
        BackendRequestKind::KeyChord { keys } => match backend.key_chord(&keys) {
            Ok(()) => BackendReply::Unit,
            Err(error) => return Some(Err(error)),
        },
        BackendRequestKind::HoldKey { keys, ms } => match backend.hold_key(&keys, ms) {
            Ok(()) => BackendReply::Unit,
            Err(error) => return Some(Err(error)),
        },
        BackendRequestKind::UiTree { opts } => match backend.ui_tree(&opts) {
            Ok(tree) => BackendReply::Tree(tree),
            Err(error) => return Some(Err(error)),
        },
        BackendRequestKind::ElementAtPoint { x, y } => match backend.element_at_point(x, y) {
            Ok(element) => BackendReply::Element(element),
            Err(error) => return Some(Err(error)),
        },
        BackendRequestKind::FocusedElement => match backend.focused_element() {
            Ok(element) => BackendReply::Element(element),
            Err(error) => return Some(Err(error)),
        },
        BackendRequestKind::ReleaseOsGrant => match backend.release_os_grant() {
            Ok(()) => BackendReply::Unit,
            Err(error) => return Some(Err(error)),
        },
        BackendRequestKind::Shutdown => return None,
    };
    Some(Ok(result))
}

fn worker_loop(
    factory: BackendFactory,
    rx: Receiver<BackendRequest>,
    startup: Sender<Result<(), ComputerUseError>>,
) {
    let mut backend = match factory() {
        Ok(backend) => {
            let _ = startup.send(Ok(()));
            backend
        }
        Err(error) => {
            let _ = startup.send(Err(error));
            return;
        }
    };
    while let Ok(request) = rx.recv() {
        // 出队后、执行前检查取消旗标：调用方已放弃的请求不再执行。剩余缺口
        // （平台 API 层面的固有限制，如实记录）：已进入 OS 调用的请求无法
        // 中断，只能在请求边界拦截。
        if request.cancelled.load(Ordering::SeqCst) {
            let _ = request.reply.send(Err(ComputerUseError::unavailable(
                "request was cancelled after the caller timed out",
            )));
            continue;
        }
        match dispatch(backend.as_mut(), request.kind) {
            Some(result) => {
                let _ = request.reply.send(result);
            }
            None => return,
        }
    }
}

type BackendFactory =
    Box<dyn FnOnce() -> Result<Box<dyn ComputerUseBackend>, ComputerUseError> + Send>;

enum WorkerState {
    /// 懒启动：工厂已存，首个请求到来时才 spawn 线程构造 backend。
    Pending(Option<BackendFactory>),
    Running {
        tx: Sender<BackendRequest>,
        thread: JoinHandle<()>,
    },
    /// 启动失败（粘性）：权限未授予等，避免每请求重建线程刷屏。
    StartFailed(String),
    Shutdown,
}

struct BackendInner {
    state: Mutex<WorkerState>,
    /// 在途请求旗标（每 handle 一枚，克隆共享）。请求通道无界，worker 卡死时
    /// 每个新调用都会占一个 blocking 线程等满 190s（评审发现）；compare_exchange
    /// 获取/释放把并发在途请求钉在 1，超出的调用立即失败、不再排队占线程。
    in_flight: AtomicBool,
}

impl BackendInner {
    fn ensure_sender(&self) -> Result<Sender<BackendRequest>, ComputerUseError> {
        let mut state = self.state.lock();
        match &mut *state {
            WorkerState::Pending(factory) => {
                let Some(factory) = factory.take() else {
                    return Err(ComputerUseError::unavailable(
                        "computer use backend factory already consumed",
                    ));
                };
                let (tx, rx) = channel::<BackendRequest>();
                let (startup_tx, startup_rx) = channel::<Result<(), ComputerUseError>>();
                let thread = std::thread::Builder::new()
                    .name("computer-use-backend".to_string())
                    .spawn(move || worker_loop(factory, rx, startup_tx))
                    .map_err(|error| {
                        ComputerUseError::unavailable(format!(
                            "cannot spawn computer use backend thread: {error}"
                        ))
                    })?;
                let startup = startup_rx.recv_timeout(BACKEND_CALL_TIMEOUT);
                // 失败分支暂存 (发送端, 线程)：先在锁内写状态、释放 state 锁，
                // 再在锁外收尾。评审发现：此前超时分支在 state 锁临界区内
                // `thread.join()`，而局部 tx 仍存活、worker 阻塞在 rx.recv()
                // 永不退出 → join 永久挂起且持有 state 锁，全会话卡死。
                let mut cleanup: Option<(Sender<BackendRequest>, JoinHandle<()>)> = None;
                let result = match startup {
                    Ok(Ok(())) => {
                        *state = WorkerState::Running {
                            tx: tx.clone(),
                            thread,
                        };
                        Ok(tx)
                    }
                    Ok(Err(error)) => {
                        let message = error.to_string();
                        *state = WorkerState::StartFailed(message.clone());
                        cleanup = Some((tx, thread));
                        Err(error)
                    }
                    // 超时/启动应答通道断开：工厂 >190s 未返回，或线程已 panic。
                    Err(_) => {
                        *state = WorkerState::StartFailed(
                            "computer use backend thread died during startup".to_string(),
                        );
                        cleanup = Some((tx, thread));
                        Err(ComputerUseError::unavailable(
                            "computer use backend thread died during startup",
                        ))
                    }
                };
                drop(state);
                if let Some((tx, thread)) = cleanup {
                    // 对齐 Drop impl 的做法：先丢弃发送端（worker 的 rx.recv()
                    // 得到断连并退出循环），再在锁外 join 回收线程。
                    drop(tx);
                    let _ = thread.join();
                }
                result
            }
            WorkerState::Running { tx, .. } => Ok(tx.clone()),
            WorkerState::StartFailed(message) => Err(ComputerUseError::unavailable(format!(
                "computer use backend is not available: {message}"
            ))),
            WorkerState::Shutdown => Err(ComputerUseError::unavailable(
                "computer use backend is shut down",
            )),
        }
    }

    /// worker 线程已退出/解 unwind（发送端全部失效）但状态机停留 Running：
    /// 迁移为粘性 StartFailed（评审发现），与工厂失败同语义——否则会话剩余
    /// 生命周期里每个请求都先成功入队、再报误导性的 "did not respond within
    /// 150s"。已在 Running 之外的状态（并发迁移/Drop 抢先）不覆盖。
    fn mark_thread_dead(&self) {
        let mut state = self.state.lock();
        if matches!(&*state, WorkerState::Running { .. }) {
            *state = WorkerState::StartFailed("computer use backend thread died".to_string());
        }
    }

    fn request(&self, kind: BackendRequestKind) -> Result<BackendReply, ComputerUseError> {
        if self
            .in_flight
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(ComputerUseError::unavailable(
                "another computer use request is still in flight",
            ));
        }
        let result = self.request_inner(kind);
        self.in_flight.store(false, Ordering::SeqCst);
        result
    }

    fn request_inner(&self, kind: BackendRequestKind) -> Result<BackendReply, ComputerUseError> {
        let tx = self.ensure_sender()?;
        let (reply_tx, reply_rx) = channel::<BackendResult>();
        let cancelled = Arc::new(AtomicBool::new(false));
        tx.send(BackendRequest {
            kind,
            reply: reply_tx,
            cancelled: Arc::clone(&cancelled),
        })
        .map_err(|_| {
            // 全部发送端失效即 worker 线程已死：状态机从 Running 迁移为粘性
            // StartFailed，本请求与后续请求都得到明确错误（评审发现）。
            self.mark_thread_dead();
            ComputerUseError::unavailable("computer use backend thread died")
        })?;
        match reply_rx.recv_timeout(BACKEND_CALL_TIMEOUT) {
            Ok(result) => result,
            // 超时：置位取消旗标，worker 出队后不再执行该请求（已进入 OS
            // 调用的请求无法中断，见 worker_loop 处注释）。
            Err(RecvTimeoutError::Timeout) => {
                cancelled.store(true, Ordering::SeqCst);
                Err(ComputerUseError::unavailable(format!(
                    "computer use backend did not respond within {BACKEND_CALL_TIMEOUT:?}"
                )))
            }
            // 断连 = worker 已退出，与超时是不同故障，分开报错（评审发现：
            // 此前混报 "did not respond within 150s"）。置位取消旗标仅为防御
            // （worker 已死不会再消费请求），并让状态机同步落地。
            Err(RecvTimeoutError::Disconnected) => {
                cancelled.store(true, Ordering::SeqCst);
                self.mark_thread_dead();
                Err(ComputerUseError::unavailable(
                    "computer use backend thread is not running",
                ))
            }
        }
    }

    /// Control lane: identical to [`Self::request`] but it deliberately
    /// bypasses the in-flight gate (no compare_exchange, `in_flight` is
    /// never touched). revoke/stop/emergency cleanup must not be rejected
    /// with "another computer use request is still in flight" while a long
    /// action runs (e.g. the 120s Wayland auth dialog) — the old routing
    /// made a user's Stop click close the in-app grant while the OS-level
    /// portal session survived. This is safe because the worker channel
    /// serializes execution: a control request enqueued while an action is
    /// in flight simply runs right after the current request resolves —
    /// that IS the "queue behind the in-flight action" semantics the old
    /// comment promised but `request` never delivered.
    fn request_control(&self, kind: BackendRequestKind) -> Result<BackendReply, ComputerUseError> {
        self.request_inner(kind)
    }
}

impl Drop for BackendInner {
    fn drop(&mut self) {
        // Transition the state machine to Shutdown under the lock, then
        // RELEASE the guard before joining: the worker may be wedged in an
        // OS call indefinitely, and joining while holding the state mutex
        // would hang the dropping thread (possibly a Tauri/async thread at
        // teardown) forever while every other caller on the handle blocks
        // on that mutex. Mirrors ensure_sender's shape: finish the state
        // write in the lock's critical section, collect what needs
        // teardown, drop the guard, then join outside the lock.
        let previous = {
            let mut state = self.state.lock();
            std::mem::replace(&mut *state, WorkerState::Shutdown)
        };
        if let WorkerState::Running { tx, thread } = previous {
            let _ = tx.send(BackendRequest {
                kind: BackendRequestKind::Shutdown,
                reply: channel::<BackendResult>().0,
                // Shutdown 请求绝不能带取消旗标：worker 会先检查旗标并跳过，
                // 永远走不到 Shutdown 分支退出（评审修正）。
                cancelled: Arc::new(AtomicBool::new(false)),
            });
            // 先丢弃发送端（worker 的 rx.recv() 得到断连并退出循环），再在
            // 锁外 join 回收线程。
            drop(tx);
            let _ = thread.join();
        }
    }
}

/// 可克隆的后端句柄（通道封装）。所有方法同步阻塞；async 调用方应包
/// `tauri::async_runtime::spawn_blocking`。
#[derive(Clone)]
pub struct BackendHandle {
    inner: Arc<BackendInner>,
}

impl BackendHandle {
    /// 懒启动：工厂在首个请求时才于 worker 线程上执行（构造 xcap/enigo 对象）。
    pub fn lazy(
        factory: impl FnOnce() -> Result<Box<dyn ComputerUseBackend>, ComputerUseError> + Send + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(BackendInner {
                state: Mutex::new(WorkerState::Pending(Some(Box::new(factory)))),
                in_flight: AtomicBool::new(false),
            }),
        }
    }

    pub fn capabilities(&self) -> Result<Capabilities, ComputerUseError> {
        match self.inner.request(BackendRequestKind::Capabilities)? {
            BackendReply::Capabilities(caps) => Ok(caps),
            _ => Err(ComputerUseError::failed("unexpected backend reply")),
        }
    }

    pub fn capture(&self) -> Result<Capture, ComputerUseError> {
        match self.inner.request(BackendRequestKind::Capture)? {
            BackendReply::Capture(capture) => Ok(capture),
            _ => Err(ComputerUseError::failed("unexpected backend reply")),
        }
    }

    /// 全局设备物理像素。
    pub fn cursor_position(&self) -> Result<(i32, i32), ComputerUseError> {
        match self.inner.request(BackendRequestKind::CursorPosition)? {
            BackendReply::CursorPosition(pos) => Ok(pos),
            _ => Err(ComputerUseError::failed("unexpected backend reply")),
        }
    }

    fn unit(&self, kind: BackendRequestKind) -> Result<(), ComputerUseError> {
        match self.inner.request(kind)? {
            BackendReply::Unit => Ok(()),
            _ => Err(ComputerUseError::failed("unexpected backend reply")),
        }
    }

    /// Like [`Self::unit`] but over the control lane (bypasses the in-flight
    /// gate; see `BackendInner::request_control`).
    fn unit_control(&self, kind: BackendRequestKind) -> Result<(), ComputerUseError> {
        match self.inner.request_control(kind)? {
            BackendReply::Unit => Ok(()),
            _ => Err(ComputerUseError::failed("unexpected backend reply")),
        }
    }

    pub fn move_to(&self, x: i32, y: i32) -> Result<(), ComputerUseError> {
        self.unit(BackendRequestKind::MoveTo { x, y })
    }

    pub fn click(&self, button: MouseButton, count: u8) -> Result<(), ComputerUseError> {
        self.unit(BackendRequestKind::Click { button, count })
    }

    pub fn mouse_down(&self, button: MouseButton) -> Result<(), ComputerUseError> {
        self.unit(BackendRequestKind::MouseDown { button })
    }

    pub fn mouse_up(&self, button: MouseButton) -> Result<(), ComputerUseError> {
        self.unit(BackendRequestKind::MouseUp { button })
    }

    pub fn drag(&self, from: (i32, i32), to: (i32, i32)) -> Result<(), ComputerUseError> {
        self.unit(BackendRequestKind::Drag { from, to })
    }

    pub fn scroll(&self, direction: ScrollDirection, clicks: u32) -> Result<(), ComputerUseError> {
        self.unit(BackendRequestKind::Scroll { direction, clicks })
    }

    pub fn type_text(&self, text: &str) -> Result<(), ComputerUseError> {
        self.unit(BackendRequestKind::TypeText {
            text: text.to_string(),
        })
    }

    pub fn key_chord(&self, keys: &[Key]) -> Result<(), ComputerUseError> {
        self.unit(BackendRequestKind::KeyChord {
            keys: keys.to_vec(),
        })
    }

    pub fn hold_key(&self, keys: &[Key], ms: u64) -> Result<(), ComputerUseError> {
        self.unit(BackendRequestKind::HoldKey {
            keys: keys.to_vec(),
            ms,
        })
    }

    pub fn ui_tree(&self, opts: UiTreeOptions) -> Result<String, ComputerUseError> {
        match self.inner.request(BackendRequestKind::UiTree { opts })? {
            BackendReply::Tree(tree) => Ok(tree),
            _ => Err(ComputerUseError::failed("unexpected backend reply")),
        }
    }

    /// 坐标为输入坐标空间。
    pub fn element_at_point(
        &self,
        x: i32,
        y: i32,
    ) -> Result<Option<ElementInfo>, ComputerUseError> {
        match self
            .inner
            .request(BackendRequestKind::ElementAtPoint { x, y })?
        {
            BackendReply::Element(element) => Ok(element),
            _ => Err(ComputerUseError::failed("unexpected backend reply")),
        }
    }

    /// 当前持有键盘焦点的元素。Err = 后端不支持/查询失败（调用方 fail-closed）。
    pub fn focused_element(&self) -> Result<Option<ElementInfo>, ComputerUseError> {
        match self.inner.request(BackendRequestKind::FocusedElement)? {
            BackendReply::Element(element) => Ok(element),
            _ => Err(ComputerUseError::failed("unexpected backend reply")),
        }
    }

    /// 关闭该会话后端持有的持久 OS 级授权（见 trait 同名方法）。阻塞直到
    /// worker 应答（可能排在在行动作之后）；UI 调用方应经
    /// [`BackendRegistry`] 在 detached 线程里触发，不阻塞事件循环。
    ///
    /// Routed through the control lane (`request_control`), NOT the gated
    /// `request`: revoke/stop can arrive while an action is in flight, and
    /// the old routing rejected the release with "another computer use
    /// request is still in flight", silently leaving the OS-level grant
    /// (Wayland portal session) open. The control request simply queues
    /// behind the in-flight action on the serialized worker channel.
    pub fn release_os_grant(&self) -> Result<(), ComputerUseError> {
        self.unit_control(BackendRequestKind::ReleaseOsGrant)
    }

    /// Best-effort physical left-button release through the control lane
    /// (bypasses the in-flight gate, queues behind any in-flight action).
    /// Emergency cleanup for sessions that may die mid-drag: a model that
    /// pressed the left button and was stopped/revoked/dropped must not
    /// leave the user's machine in button-held drag state.
    pub fn emergency_mouse_up(&self) -> Result<(), ComputerUseError> {
        self.unit_control(BackendRequestKind::MouseUp {
            button: MouseButton::Left,
        })
    }
}

/// 会话 → 后端句柄登记表。工具构造时登记、析构时注销；revoke/stop/总开关
/// 关闭时由命令层触发对应会话的 `release_os_grant`（评审发现：用户"停止
/// 控制"后，OS 级 portal 授权必须随之终止而不是活到进程退出）。
#[derive(Default)]
pub struct BackendRegistry {
    handles: Mutex<HashMap<String, BackendHandle>>,
}

impl BackendRegistry {
    /// 工具构造时登记会话句柄（同会话重复登记以最新为准）。
    pub fn insert(&self, session_id: &str, handle: BackendHandle) {
        self.handles.lock().insert(session_id.to_string(), handle);
    }

    // Unregistration has a single entry point, [`Self::emergency_release`]:
    // it unregisters AND cleans up (physical button + OS grant). A bare
    // remove was removed on purpose — an unregister-only path lets a dying
    // session skip the button/grant cleanup while looking successful.

    /// Emergency cleanup for one session (revoke / tool drop): on a detached
    /// thread, first unpress the physical left button (a model that pressed
    /// and died must not leave the machine in button-held drag state), then
    /// close the persistent OS-level grant. Both go through the control
    /// lane, so they are NOT rejected while an action is in flight — they
    /// queue behind it on the serialized worker channel and run once it
    /// resolves. Errors are only logged: the worst case (a leaked grant or
    /// held button) must never panic or block the caller. The handle is
    /// unregistered synchronously, so revoke/drop immediately stops the
    /// registry from tracking the session; the detached thread keeps its own
    /// handle clone alive until the cleanup finishes.
    ///
    /// Known trade-off: a session re-granted after a revoke keeps acting
    /// through its still-alive tool handle, and the rebuilt OS grant is no
    /// longer tracked here — it is only closed again by the tool's Drop.
    pub fn emergency_release(&self, session_id: &str) {
        let handle = self.handles.lock().remove(session_id);
        let Some(handle) = handle else {
            return;
        };
        std::thread::spawn(move || emergency_cleanup(handle));
    }

    /// [`Self::emergency_release`] for every registered session (stop /
    /// master-switch off). Registrations are kept: live tools unregister via
    /// their own Drop, and a session that is re-granted later must still be
    /// reachable for a subsequent global stop.
    pub fn emergency_release_all(&self) {
        let handles: Vec<BackendHandle> = self.handles.lock().values().cloned().collect();
        for handle in handles {
            std::thread::spawn(move || emergency_cleanup(handle));
        }
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, session_id: &str) -> bool {
        self.handles.lock().contains_key(session_id)
    }
}

/// Emergency cleanup for one handle, in the fixed order: unpress the
/// physical left button first, then close the persistent OS-level grant.
/// Runs on detached threads (see [`BackendRegistry::emergency_release`]);
/// errors are only logged with the existing `eprintln!` convention.
fn emergency_cleanup(handle: BackendHandle) {
    if let Err(error) = handle.emergency_mouse_up() {
        eprintln!("[computer_use] emergency_mouse_up failed: {error}");
    }
    if let Err(error) = handle.release_os_grant() {
        eprintln!("[computer_use] release_os_grant failed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct MockBackend {
        clicks: usize,
    }

    impl ComputerUseBackend for MockBackend {
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                screenshot: true,
                input: true,
                ui_tree: false,
                notes: "mock".to_string(),
            }
        }

        fn capture(&mut self) -> Result<Capture, ComputerUseError> {
            Ok(Capture {
                rgba: vec![0u8; 4 * 4 * 4],
                width: 4,
                height: 4,
                origin_x: 0,
                origin_y: 0,
                input_scale_x: 1.0,
                input_scale_y: 1.0,
            })
        }

        fn cursor_position(&mut self) -> Result<(i32, i32), ComputerUseError> {
            Ok((10, 20))
        }

        fn move_to(&mut self, _x: i32, _y: i32) -> Result<(), ComputerUseError> {
            Ok(())
        }

        fn click(&mut self, _button: MouseButton, _count: u8) -> Result<(), ComputerUseError> {
            self.clicks += 1;
            Ok(())
        }

        fn mouse_down(&mut self, _button: MouseButton) -> Result<(), ComputerUseError> {
            Ok(())
        }

        fn mouse_up(&mut self, _button: MouseButton) -> Result<(), ComputerUseError> {
            Ok(())
        }

        fn drag(&mut self, _from: (i32, i32), _to: (i32, i32)) -> Result<(), ComputerUseError> {
            Ok(())
        }

        fn scroll(
            &mut self,
            _direction: ScrollDirection,
            _clicks: u32,
        ) -> Result<(), ComputerUseError> {
            Ok(())
        }

        fn type_text(&mut self, _text: &str) -> Result<(), ComputerUseError> {
            Ok(())
        }

        fn key_chord(&mut self, _keys: &[Key]) -> Result<(), ComputerUseError> {
            Ok(())
        }

        fn hold_key(&mut self, _keys: &[Key], _ms: u64) -> Result<(), ComputerUseError> {
            Ok(())
        }

        fn ui_tree(&mut self, _opts: &UiTreeOptions) -> Result<String, ComputerUseError> {
            Err(ComputerUseError::unsupported("ui_tree", "mock has no tree"))
        }

        fn element_at_point(
            &mut self,
            _x: i32,
            _y: i32,
        ) -> Result<Option<ElementInfo>, ComputerUseError> {
            Ok(None)
        }
    }

    #[test]
    fn handle_round_trips_typed_requests_on_worker_thread() {
        let constructed_on_worker = Arc::new(AtomicBool::new(false));
        let main_thread = std::thread::current().id();
        let flag = Arc::clone(&constructed_on_worker);
        let handle = BackendHandle::lazy(move || {
            if std::thread::current().id() != main_thread {
                flag.store(true, Ordering::SeqCst);
            }
            Ok(Box::new(MockBackend { clicks: 0 }))
        });
        let caps = handle.capabilities();
        assert!(caps.as_ref().is_ok_and(|c| c.screenshot));
        assert!(constructed_on_worker.load(Ordering::SeqCst));
        assert_eq!(handle.cursor_position().ok(), Some((10, 20)));
        assert!(handle.click(MouseButton::Left, 1).is_ok());
        let capture = handle.capture();
        assert!(capture.as_ref().is_ok_and(|c| c.width == 4));
        let tree = handle.ui_tree(UiTreeOptions::default());
        assert!(
            tree.as_ref()
                .is_err_and(|e| e.to_string().contains("unsupported"))
        );
    }

    #[test]
    fn factory_failure_is_sticky_and_reports_error() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&attempts);
        let handle = BackendHandle::lazy(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Err::<Box<dyn ComputerUseBackend>, _>(ComputerUseError::unavailable(
                "no screen recording permission",
            ))
        });
        assert!(handle.capture().is_err());
        assert!(handle.capture().is_err());
        // 第二次请求不重建线程（粘性失败）。
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn drop_shuts_worker_thread_down() {
        let dropped = Arc::new(AtomicBool::new(false));
        struct TrackDrop(Arc<AtomicBool>);
        impl Drop for TrackDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        struct DroppingBackend(TrackDrop);
        impl ComputerUseBackend for DroppingBackend {
            fn capabilities(&self) -> Capabilities {
                Capabilities {
                    screenshot: false,
                    input: false,
                    ui_tree: false,
                    notes: String::new(),
                }
            }
            fn capture(&mut self) -> Result<Capture, ComputerUseError> {
                Err(ComputerUseError::unsupported("screenshot", "mock"))
            }
            fn cursor_position(&mut self) -> Result<(i32, i32), ComputerUseError> {
                Err(ComputerUseError::unsupported("cursor_position", "mock"))
            }
            fn move_to(&mut self, _x: i32, _y: i32) -> Result<(), ComputerUseError> {
                Err(ComputerUseError::unsupported("input", "mock"))
            }
            fn click(&mut self, _b: MouseButton, _c: u8) -> Result<(), ComputerUseError> {
                Err(ComputerUseError::unsupported("input", "mock"))
            }
            fn mouse_down(&mut self, _b: MouseButton) -> Result<(), ComputerUseError> {
                Err(ComputerUseError::unsupported("input", "mock"))
            }
            fn mouse_up(&mut self, _b: MouseButton) -> Result<(), ComputerUseError> {
                Err(ComputerUseError::unsupported("input", "mock"))
            }
            fn drag(&mut self, _f: (i32, i32), _t: (i32, i32)) -> Result<(), ComputerUseError> {
                Err(ComputerUseError::unsupported("input", "mock"))
            }
            fn scroll(&mut self, _d: ScrollDirection, _c: u32) -> Result<(), ComputerUseError> {
                Err(ComputerUseError::unsupported("input", "mock"))
            }
            fn type_text(&mut self, _t: &str) -> Result<(), ComputerUseError> {
                Err(ComputerUseError::unsupported("input", "mock"))
            }
            fn key_chord(&mut self, _k: &[Key]) -> Result<(), ComputerUseError> {
                Err(ComputerUseError::unsupported("input", "mock"))
            }
            fn hold_key(&mut self, _k: &[Key], _m: u64) -> Result<(), ComputerUseError> {
                Err(ComputerUseError::unsupported("input", "mock"))
            }
            fn ui_tree(&mut self, _o: &UiTreeOptions) -> Result<String, ComputerUseError> {
                Err(ComputerUseError::unsupported("ui_tree", "mock"))
            }
            fn element_at_point(
                &mut self,
                _x: i32,
                _y: i32,
            ) -> Result<Option<ElementInfo>, ComputerUseError> {
                Err(ComputerUseError::unsupported("ui_tree", "mock"))
            }
        }
        let flag = Arc::clone(&dropped);
        {
            let handle = BackendHandle::lazy(move || {
                Ok(Box::new(DroppingBackend(TrackDrop(flag))) as Box<dyn ComputerUseBackend>)
            });
            assert!(handle.capabilities().is_ok());
        }
        // Drop 后 worker 线程收到 Shutdown 并析构 backend 对象。
        assert!(dropped.load(Ordering::SeqCst));
    }

    /// 评审修复回归：调用方已放弃（超时/断连置位旗标）的请求在 worker 出队
    /// 后、执行前被跳过，并回明确的取消错误——不能照常执行击穿跨会话串行化。
    #[test]
    fn worker_skips_requests_cancelled_before_dequeue() {
        let (req_tx, req_rx) = channel::<BackendRequest>();
        let (startup_tx, startup_rx) = channel::<Result<(), ComputerUseError>>();
        let factory: BackendFactory =
            Box::new(|| Ok(Box::new(MockBackend { clicks: 0 }) as Box<dyn ComputerUseBackend>));
        let worker = std::thread::spawn(move || worker_loop(factory, req_rx, startup_tx));
        assert!(startup_rx.recv().expect("startup channel alive").is_ok());
        let (reply_tx, reply_rx) = channel::<BackendResult>();
        req_tx
            .send(BackendRequest {
                kind: BackendRequestKind::Capabilities,
                reply: reply_tx,
                cancelled: Arc::new(AtomicBool::new(true)),
            })
            .expect("worker alive");
        let error = reply_rx
            .recv()
            .expect("worker must still answer cancelled requests")
            .err()
            .expect("cancelled request must reply with an error");
        assert!(
            error.to_string().contains("cancelled"),
            "unexpected error: {error}"
        );
        drop(req_tx);
        worker.join().expect("worker exits when channel closes");
    }

    /// 评审修复回归：请求通道无界，worker 卡死时每个新调用都会占一个
    /// blocking 线程等满 190s——in-flight 旗标把并发在途请求钉在 1。
    #[test]
    fn concurrent_requests_are_rejected_while_one_is_in_flight() {
        let handle = BackendHandle::lazy(|| {
            Ok(Box::new(MockBackend { clicks: 0 }) as Box<dyn ComputerUseBackend>)
        });
        // 直接置位旗标模拟「已有在途请求」（真实路径由 request 获取/释放）。
        handle.inner.in_flight.store(true, Ordering::SeqCst);
        let error = handle
            .capabilities()
            .err()
            .expect("second request must fail fast, not queue");
        assert!(
            error.to_string().contains("in flight"),
            "unexpected error: {error}"
        );
        handle.inner.in_flight.store(false, Ordering::SeqCst);
        assert!(handle.capabilities().is_ok(), "flag must be released");
    }

    /// Shared probe state for the control-lane tests: records emergency
    /// surfaces and can stall one method to simulate a long in-flight
    /// action (e.g. the 120s Wayland auth dialog).
    #[derive(Default)]
    struct ProbeState {
        stall: Option<Duration>,
        captures: u64,
        mouse_ups: Vec<MouseButton>,
        releases: u64,
    }

    struct ControlProbeBackend {
        state: Arc<Mutex<ProbeState>>,
    }

    impl ComputerUseBackend for ControlProbeBackend {
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                screenshot: true,
                input: true,
                ui_tree: false,
                notes: "probe".to_string(),
            }
        }

        fn capture(&mut self) -> Result<Capture, ComputerUseError> {
            let stall = self.state.lock().stall;
            if let Some(delay) = stall {
                std::thread::sleep(delay);
            }
            self.state.lock().captures += 1;
            Ok(Capture {
                rgba: vec![0u8; 4 * 4 * 4],
                width: 4,
                height: 4,
                origin_x: 0,
                origin_y: 0,
                input_scale_x: 1.0,
                input_scale_y: 1.0,
            })
        }

        fn cursor_position(&mut self) -> Result<(i32, i32), ComputerUseError> {
            Ok((0, 0))
        }

        fn move_to(&mut self, _x: i32, _y: i32) -> Result<(), ComputerUseError> {
            Ok(())
        }

        fn click(&mut self, _button: MouseButton, _count: u8) -> Result<(), ComputerUseError> {
            Ok(())
        }

        fn mouse_down(&mut self, _button: MouseButton) -> Result<(), ComputerUseError> {
            Ok(())
        }

        fn mouse_up(&mut self, button: MouseButton) -> Result<(), ComputerUseError> {
            self.state.lock().mouse_ups.push(button);
            Ok(())
        }

        fn drag(&mut self, _from: (i32, i32), _to: (i32, i32)) -> Result<(), ComputerUseError> {
            Ok(())
        }

        fn scroll(
            &mut self,
            _direction: ScrollDirection,
            _clicks: u32,
        ) -> Result<(), ComputerUseError> {
            Ok(())
        }

        fn type_text(&mut self, _text: &str) -> Result<(), ComputerUseError> {
            Ok(())
        }

        fn key_chord(&mut self, _keys: &[Key]) -> Result<(), ComputerUseError> {
            Ok(())
        }

        fn hold_key(&mut self, _keys: &[Key], _ms: u64) -> Result<(), ComputerUseError> {
            Ok(())
        }

        fn ui_tree(&mut self, _opts: &UiTreeOptions) -> Result<String, ComputerUseError> {
            Err(ComputerUseError::unsupported(
                "ui_tree",
                "probe has no tree",
            ))
        }

        fn element_at_point(
            &mut self,
            _x: i32,
            _y: i32,
        ) -> Result<Option<ElementInfo>, ComputerUseError> {
            Ok(None)
        }

        fn release_os_grant(&mut self) -> Result<(), ComputerUseError> {
            self.state.lock().releases += 1;
            Ok(())
        }
    }

    /// Spins a slow capture on a helper thread and returns once it provably
    /// holds the in-flight flag.
    fn start_in_flight_capture(handle: &BackendHandle) -> std::thread::JoinHandle<()> {
        let worker = {
            let handle = handle.clone();
            std::thread::spawn(move || {
                handle
                    .capture()
                    .expect("slow capture must eventually succeed");
            })
        };
        while !handle.inner.in_flight.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        worker
    }

    /// Regression (revoke/stop during an in-flight request): the OS-grant
    /// release must go through the control lane instead of being rejected
    /// with "another computer use request is still in flight" — the old
    /// `request` routing silently kept the OS-level portal session alive
    /// whenever the user clicked Stop during a long action.
    #[test]
    fn release_os_grant_succeeds_while_another_request_is_in_flight() {
        let state = Arc::new(Mutex::new(ProbeState::default()));
        state.lock().stall = Some(Duration::from_millis(300));
        let state_for_factory = Arc::clone(&state);
        let handle = BackendHandle::lazy(move || {
            Ok(Box::new(ControlProbeBackend {
                state: state_for_factory,
            }) as Box<dyn ComputerUseBackend>)
        });
        let worker = start_in_flight_capture(&handle);
        let released = handle.release_os_grant();
        assert!(
            released.is_ok(),
            "control lane must bypass the in-flight gate: {released:?}"
        );
        worker.join().expect("capture thread");
        let state = state.lock();
        assert_eq!(state.captures, 1);
        assert_eq!(state.releases, 1, "release must reach the backend");
    }

    /// Regression (emergency cleanup during an in-flight request): the
    /// physical left-button release must also ride the control lane and
    /// reach the backend instead of being rejected by the in-flight gate.
    #[test]
    fn emergency_mouse_up_releases_the_left_button_while_in_flight() {
        let state = Arc::new(Mutex::new(ProbeState::default()));
        state.lock().stall = Some(Duration::from_millis(300));
        let state_for_factory = Arc::clone(&state);
        let handle = BackendHandle::lazy(move || {
            Ok(Box::new(ControlProbeBackend {
                state: state_for_factory,
            }) as Box<dyn ComputerUseBackend>)
        });
        let worker = start_in_flight_capture(&handle);
        let result = handle.emergency_mouse_up();
        assert!(
            result.is_ok(),
            "emergency mouse-up must bypass the in-flight gate: {result:?}"
        );
        worker.join().expect("capture thread");
        let state = state.lock();
        assert_eq!(state.captures, 1);
        assert_eq!(state.mouse_ups, vec![MouseButton::Left]);
    }
}
