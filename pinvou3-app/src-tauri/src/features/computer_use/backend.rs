//! 后端 trait 与专用 worker 线程。
//!
//! xcap/enigo/平台 a11y 对象全部同步且有线程亲和性（且内部含 unsafe FFI），
//! 因此所有后端工作跑在**每个会话一条**的专用 worker 线程上：backend 对象在该
//! 线程上构造、使用、析构，永不跨线程移动。`BackendHandle` 是可克隆的通道封装，
//! 工具层（async）用 `spawn_blocking` 调它的同步方法。
//!
//! 线程通过 `BackendHandle` 全部析构时的 Drop 发送 `Shutdown` 并 join 退出。

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use parking_lot::Mutex;

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
    fn scroll(
        &mut self,
        direction: ScrollDirection,
        clicks: u32,
    ) -> Result<(), ComputerUseError>;
    fn type_text(&mut self, text: &str) -> Result<(), ComputerUseError>;
    /// 按下全部键再逆序释放（和弦）。
    fn key_chord(&mut self, keys: &[Key]) -> Result<(), ComputerUseError>;
    /// 按住和弦 ms 毫秒后释放。
    fn hold_key(&mut self, keys: &[Key], ms: u64) -> Result<(), ComputerUseError>;
    /// 无障碍树序列化文本（格式由后端定，建议缩进文本树）。
    fn ui_tree(&mut self, opts: &UiTreeOptions) -> Result<String, ComputerUseError>;
    fn element_at_point(
        &mut self,
        x: i32,
        y: i32,
    ) -> Result<Option<ElementInfo>, ComputerUseError>;
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
}

fn dispatch(backend: &mut dyn ComputerUseBackend, kind: BackendRequestKind) -> Option<BackendResult> {
    let result = match kind {
        BackendRequestKind::Capabilities => {
            BackendReply::Capabilities(backend.capabilities())
        }
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
        match dispatch(backend.as_mut(), request.kind) {
            Some(result) => {
                let _ = request.reply.send(result);
            }
            None => return,
        }
    }
}

type BackendFactory = Box<dyn FnOnce() -> Result<Box<dyn ComputerUseBackend>, ComputerUseError> + Send>;

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
                match startup_rx.recv() {
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
                        let _ = thread.join();
                        Err(error)
                    }
                    Err(_) => {
                        *state = WorkerState::StartFailed(
                            "computer use backend thread died during startup".to_string(),
                        );
                        let _ = thread.join();
                        Err(ComputerUseError::unavailable(
                            "computer use backend thread died during startup",
                        ))
                    }
                }
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

    fn request(&self, kind: BackendRequestKind) -> Result<BackendReply, ComputerUseError> {
        let tx = self.ensure_sender()?;
        let (reply_tx, reply_rx) = channel::<BackendResult>();
        tx.send(BackendRequest {
            kind,
            reply: reply_tx,
        })
        .map_err(|_| {
            ComputerUseError::unavailable("computer use backend thread is not running")
        })?;
        reply_rx.recv().map_err(|_| {
            ComputerUseError::unavailable("computer use backend thread dropped the request")
        })?
    }
}

impl Drop for BackendInner {
    fn drop(&mut self) {
        let mut state = self.state.lock();
        let previous = std::mem::replace(&mut *state, WorkerState::Shutdown);
        if let WorkerState::Running { tx, thread } = previous {
            let _ = tx.send(BackendRequest {
                kind: BackendRequestKind::Shutdown,
                reply: channel::<BackendResult>().0,
            });
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

    pub fn scroll(
        &self,
        direction: ScrollDirection,
        clicks: u32,
    ) -> Result<(), ComputerUseError> {
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
        match self.inner.request(BackendRequestKind::ElementAtPoint { x, y })? {
            BackendReply::Element(element) => Ok(element),
            _ => Err(ComputerUseError::failed("unexpected backend reply")),
        }
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
        assert!(tree.as_ref().is_err_and(|e| e.to_string().contains("unsupported")));
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
            fn scroll(
                &mut self,
                _d: ScrollDirection,
                _c: u32,
            ) -> Result<(), ComputerUseError> {
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
}
