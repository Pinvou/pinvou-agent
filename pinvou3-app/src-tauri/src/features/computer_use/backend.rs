//! The backend trait and the dedicated worker thread.
//!
//! xcap/enigo/platform a11y objects are all synchronous with thread affinity (and contain
//! unsafe FFI internally), so all backend work runs on **one dedicated worker thread per
//! session**: the backend object is constructed, used, and dropped on that thread, never
//! moving across threads. `BackendHandle` is a cloneable channel wrapper; the tool layer
//! (async) calls its synchronous methods via `spawn_blocking`.
//!
//! When all `BackendHandle` clones are dropped, the thread's Drop sends `Shutdown` and joins
//! the exit.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::Mutex;

/// Wait ceiling for a single backend request. It must cover the worst legal
/// SUM of one lazy-start path, not any single phase: every portal round-trip
/// has THREE independently bounded phases (AddMatch subscription + method
/// call + response wait). Worst legal lazy-start request:
/// - CreateSession + SelectDevices + SelectSources: 3 x (30s subscribe +
///   30s call + 30s response) = 270s
/// - Start: 30s subscribe + 30s call + 120s response = 180s
/// - OpenPipeWireRemote: +30s (on the same ensure_started chain whenever
///   the request auto-captures; hold_key always does, T3 screens it)
/// - the request itself: hold_key 4 keys 4x(10s press+release) + 30s hold
///   = 110s, drag motion+press+12 waypoints+release = 150s
/// = 632s worst legal lazy-start request (recycle Session.Close included);
/// 700s leaves margin. (The previous value, 600s, under-counted the drag's
/// 12 per-waypoint notify ceilings and the poisoned-recycle close; 530s
/// before that ignored hold_key chords; 400s under-counted the per-round-trip
/// AddMatch subscription phase and the press/release notify bounds; 340s
/// before that under-counted OpenPipeWireRemote.) Below the true sum the
/// lazy-start slow path false-times-out: the caller reports failure and
/// retries while the already-dequeued zombie request still executes — the
/// double injection this constant exists to prevent. The historical findings
/// still hold: an unbounded wait lets one wedged XTEST/CGEvent/portal call
/// queue every later request of the session forever. The timeout only frees
/// the caller; on timeout/disconnect it sets the request's cancel flag and
/// the worker checks it after dequeue, before execution, and — for the
/// multi-event request kinds (type) — again between injected events, so an
/// abandoned request stops early instead of running to completion. (scroll
/// is a bounded ≤100-click loop, seconds at worst, and is not checked
/// between clicks — documented residual.) Residual
/// gap (inherent platform limitation, recorded honestly): a request already
/// inside a single OS call (XTEST/CGEvent/portal notify) cannot be
/// interrupted; while the worker stays wedged in an OS call, later requests
/// are rejected by the in-flight flag instead of queueing onto the thread.
const BACKEND_CALL_TIMEOUT: Duration = Duration::from_secs(700);

use super::types::{
    Capabilities, Capture, ComputerUseError, ElementInfo, Key, MouseButton, ScrollDirection,
    UiTreeOptions,
};

/// Platform backend contract. All methods are synchronous and `Result`-driven; when the
/// platform lacks a capability, return `ComputerUseError::unsupported` — never degrade
/// silently or reuse another platform's implementation.
///
/// Coordinate contract:
/// - `capture` returns device physical pixels plus an input scale factor (see [`Capture`]).
/// - `cursor_position` returns global INPUT-space coordinates — the same
///   space `move_to` / `click` / `drag` take (Windows/X11 physical pixels,
///   macOS CGEvent points, Wayland stream-logical pixels). A global
///   device-pixel space is deliberately NOT the contract: on mixed-DPI
///   macOS the per-monitor device rects overlap, so a well-defined global
///   device-pixel space does not exist there.
/// - The coordinate arguments of `move_to` / `click` / `drag` / `scroll` / `element_at_point`
///   are **input coordinate space** (Windows physical pixels; macOS CGEvent points).
pub trait ComputerUseBackend: Send {
    fn capabilities(&self) -> Capabilities;
    fn capture(&mut self) -> Result<Capture, ComputerUseError>;
    fn cursor_position(&mut self) -> Result<(i32, i32), ComputerUseError>;
    fn move_to(&mut self, x: i32, y: i32) -> Result<(), ComputerUseError>;
    /// count: 1 = single click, 2 = double click, 3 = triple click.
    fn click(&mut self, button: MouseButton, count: u8) -> Result<(), ComputerUseError>;
    fn mouse_down(&mut self, button: MouseButton) -> Result<(), ComputerUseError>;
    fn mouse_up(&mut self, button: MouseButton) -> Result<(), ComputerUseError>;
    fn drag(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<(), ComputerUseError>;
    /// clicks: wheel detents (positive; the direction is given by `direction`).
    fn scroll(&mut self, direction: ScrollDirection, clicks: u32) -> Result<(), ComputerUseError>;
    fn type_text(&mut self, text: &str) -> Result<(), ComputerUseError>;
    /// Press all keys then release them in reverse (a chord).
    fn key_chord(&mut self, keys: &[Key]) -> Result<(), ComputerUseError>;
    /// Hold the chord for `ms` milliseconds, then release.
    fn hold_key(&mut self, keys: &[Key], ms: u64) -> Result<(), ComputerUseError>;
    /// Serialized accessibility tree text (format is backend-defined; an indented text tree
    /// is recommended).
    fn ui_tree(&mut self, opts: &UiTreeOptions) -> Result<String, ComputerUseError>;
    fn element_at_point(&mut self, x: i32, y: i32)
    -> Result<Option<ElementInfo>, ComputerUseError>;
    /// Returns the element currently holding keyboard focus (used for the T3 screening of
    /// keyboard actions).
    /// Ok(None) = definitively no focused element; Err = the query failed or the platform
    /// does not support it (the tool layer treats it best-effort: when screening is
    /// unavailable the action proceeds, and only a positive hit on a password/listed role
    /// requires confirmation).
    fn focused_element(&mut self) -> Result<Option<ElementInfo>, ComputerUseError> {
        let _ = &mut *self;
        Err(ComputerUseError::unsupported(
            "focused_element",
            "this backend cannot query the keyboard-focused element",
        ))
    }
    /// Called when the user revokes session authorization / global stop / the master switch
    /// turns off: closes the **persistent OS-level authorization** held by this backend
    /// (the Wayland RemoteDesktop portal session is a system-level input
    /// authorization; if revoke/stop only cleared in-app state it would
    /// live until process exit, contradicting the user-visible "can be
    /// stopped at any time" promise). X11 XTEST,
    /// Windows SendInput, and macOS CGEvent have no persistent grant, so the default is a
    /// no-op. After the call the backend must remain usable: the next action lazily rebuilds
    /// the grant via the existing path (the user will see the system authorization dialog
    /// when needed).
    fn release_os_grant(&mut self) -> Result<(), ComputerUseError> {
        let _ = &mut *self;
        Ok(())
    }
    /// Request-scoped cancel flag (see [`BackendRequest::cancelled`]): the worker sets it
    /// before dispatching each request and clears it afterwards. Implementations of
    /// multi-event requests (type's per-character/per-chunk injection) check it between
    /// events, so a request the caller has already abandoned on timeout stops injecting
    /// immediately — the one-shot check at dequeue cannot stop long requests that
    /// legitimately exceed the call budget: Wayland per-character injection costs
    /// two bounded portal notifications per character; 10k characters on a degraded
    /// bus far exceeds the call budget, and after the caller times out, the zombie
    /// request would double-inject alongside the retry. type checks in chunks on all
    /// three platforms (X11 per-character remapping; Wayland per-character portal
    /// notifications; Windows/macOS 64-character chunks — low-level event hooks
    /// process each event synchronously, so a whole-text batch injection can
    /// legitimately exceed the call budget). The multi-event loops of scroll/drag do
    /// not check (seconds-bounded, see the residual notes in the
    /// BACKEND_CALL_TIMEOUT comment).
    fn set_cancel_flag(&mut self, flag: Option<Arc<AtomicBool>>) {
        let _ = flag;
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
    /// Cancel flag: the caller and the request each hold half of an `Arc`. It is set once the
    /// caller gives up on timeout/channel disconnect; the worker checks it after dequeue and
    /// before execution, and a cancelled request is not executed (without
    /// the skip, a request the caller abandoned on timeout was still
    /// executed as usual — by then the physical input lock was released
    /// and the guard would not re-check, other sessions could be injecting
    /// at the same time, breaking the cross-session full-serialization
    /// guarantee, and the model had already been told "the action failed").
    cancelled: Arc<AtomicBool>,
    /// Control-lane request (emergency-stop button release / OS-grant close / Shutdown):
    /// **exempt from the dequeue skip**. An action request executing late
    /// after cancellation is a double injection and must be blocked; control requests are the
    /// exact opposite — when a wedged worker recovers, a late release is strictly better than
    /// never executing (skipping would strand pressed buttons and portal sessions even though
    /// the worker has recovered). The cancel flag is still set for control requests; it is
    /// simply no longer used to skip them.
    control: bool,
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
        // Check the cancel flag after dequeue and before execution: a request the caller has
        // already abandoned is not executed. Multi-event injection (type) also checks the
        // flag between events (see `set_cancel_flag`). Residual gap (an inherent limitation
        // at the platform API level, recorded honestly): a request already inside a single
        // OS call cannot be interrupted; it can only be intercepted at call boundaries.
        // Control-lane requests (control=true) are exempt: a late emergency-stop release
        // must execute rather than be skipped (see `BackendRequest::control`).
        if !request.control && request.cancelled.load(Ordering::SeqCst) {
            let _ = request.reply.send(Err(ComputerUseError::unavailable(
                "request was cancelled after the caller timed out",
            )));
            continue;
        }
        // Hand the cancel flag to the backend before dispatch: multi-event injection (type)
        // checks it between events so a request the caller has abandoned stops injecting.
        // Cleared after dispatch so a stale flag does not affect later requests.
        backend
            .as_mut()
            .set_cancel_flag(Some(Arc::clone(&request.cancelled)));
        // dispatch as a whole runs inside catch_unwind: previously only some platforms'
        // capture paths had panic protection, and any other panic (enigo/UIA/portal layers)
        // would unwind-kill the worker thread, sticking the whole session at StartFailed
        // (the safe direction is no injection, but disabling the entire session forever
        // overshot). A panic becomes one Failed reply and the worker keeps serving later
        // requests. The panic payload may embed input content — the same reason it is
        // excluded from the reply/audit — so stderr only gets a bounded, truncated
        // summary, never the raw payload in full.
        let dispatched = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dispatch(backend.as_mut(), request.kind)
        }));
        backend.as_mut().set_cancel_flag(None);
        match dispatched {
            Ok(Some(result)) => {
                let _ = request.reply.send(result);
            }
            Ok(None) => return,
            Err(panic) => {
                eprintln!(
                    "[computer_use] backend worker panicked: {}",
                    panic_payload_summary(&panic)
                );
                let _ = request.reply.send(Err(ComputerUseError::failed(
                    "the computer use backend panicked while handling the request; \
                     the action may have partially executed",
                )));
            }
        }
    }
}

/// Input-safe summary of a caught panic payload for stderr logging. The
/// payload may embed typed input content (the same reason it is excluded
/// from reply/audit), so only the payload kind plus a truncated head
/// (256 chars, with an explicit truncation marker) is logged.
fn panic_payload_summary(panic: &(dyn std::any::Any + Send)) -> String {
    const PANIC_LOG_LIMIT: usize = 256;
    let (kind, text) = if let Some(text) = panic.downcast_ref::<&str>() {
        ("&str", *text)
    } else if let Some(text) = panic.downcast_ref::<String>() {
        ("String", text.as_str())
    } else {
        return "non-string panic payload".to_string();
    };
    let mut head: String = text.chars().take(PANIC_LOG_LIMIT).collect();
    if text.chars().count() > PANIC_LOG_LIMIT {
        head.push_str("… (truncated)");
    }
    format!("{kind}: {head}")
}

type BackendFactory =
    Box<dyn FnOnce() -> Result<Box<dyn ComputerUseBackend>, ComputerUseError> + Send>;

enum WorkerState {
    /// Lazy start: the factory is stored; the thread spawning the backend only happens when
    /// the first request arrives.
    Pending(Option<BackendFactory>),
    Running {
        tx: Sender<BackendRequest>,
        thread: JoinHandle<()>,
    },
    /// Startup failed (sticky): e.g. permission not granted; avoids rebuilding the thread per
    /// request and spamming.
    StartFailed(String),
    Shutdown,
}

struct BackendInner {
    state: Mutex<WorkerState>,
    /// In-flight flag (one per handle, shared by clones). The request
    /// channel is unbounded, so with a wedged worker every new call would
    /// occupy a blocking thread for up to BACKEND_CALL_TIMEOUT; the
    /// compare_exchange acquire/release pins concurrent in-flight requests
    /// to 1, and excess calls fail immediately instead of queueing onto
    /// threads.
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
                let thread = match std::thread::Builder::new()
                    .name("computer-use-backend".to_string())
                    .spawn(move || worker_loop(factory, rx, startup_tx))
                {
                    Ok(thread) => thread,
                    Err(error) => {
                        // Sticky failure, same as the factory-error branch below: the
                        // factory is already consumed, so leaving the state at
                        // Pending(None) would only turn the real spawn error into the
                        // misleading "factory already consumed" on every later request.
                        let message = format!("cannot spawn computer use backend thread: {error}");
                        *state = WorkerState::StartFailed(message.clone());
                        return Err(ComputerUseError::unavailable(message));
                    }
                };
                let startup = startup_rx.recv_timeout(BACKEND_CALL_TIMEOUT);
                // On the failure branches, stash (sender, thread): write the state under the
                // lock first and release the state lock, then finish up outside the lock.
                // The timeout branch previously called `thread.join()` inside the
                // state-lock critical section while the local tx was still alive and the
                // worker blocked in rx.recv() forever → join hung permanently while
                // holding the state lock, deadlocking the whole session.
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
                    // Startup reply timed out (factory did not return within
                    // BACKEND_CALL_TIMEOUT) or the startup channel dropped
                    // (worker thread panicked). The sticky StartFailed state
                    // covers both, but the diagnosis differs: report the
                    // budget overrun as abandoned, not as a dead thread.
                    Err(_) => {
                        *state = WorkerState::StartFailed(
                            "computer use backend startup was abandoned (the factory did not \
                             return within the startup budget, or the thread died)"
                                .to_string(),
                        );
                        cleanup = Some((tx, thread));
                        Err(ComputerUseError::unavailable(
                            "computer use backend startup was abandoned (the factory did not \
                             return within the startup budget, or the thread died)",
                        ))
                    }
                };
                drop(state);
                if let Some((tx, thread)) = cleanup {
                    // Mirror the Drop impl: drop the sender first (the worker's rx.recv()
                    // sees the disconnect and exits the loop), then drop the JoinHandle so
                    // the thread detaches (in the startup-timeout branch the worker may
                    // still be stuck inside the factory call, and join would pin this
                    // caller's spawn_blocking thread forever); once the wedged factory
                    // returns, the worker sees the disconnect and exits on its own, and
                    // detaching only leaves a lingering thread without blocking anyone.
                    drop(tx);
                    drop(thread);
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

    /// The worker thread has exited / unwound (all senders dead) but the state machine is
    /// stuck at Running: migrate to sticky StartFailed, same semantics as a factory
    /// failure — otherwise every request for the rest of the session's lifetime would
    /// first enqueue successfully and then fail with the misleading
    /// "did not respond within <BACKEND_CALL_TIMEOUT>". States outside Running (concurrent
    /// migration / a racing Drop) are not overwritten.
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
        let result = self.request_inner(kind, false);
        self.in_flight.store(false, Ordering::SeqCst);
        result
    }

    fn request_inner(
        &self,
        kind: BackendRequestKind,
        control: bool,
    ) -> Result<BackendReply, ComputerUseError> {
        let tx = self.ensure_sender()?;
        let (reply_tx, reply_rx) = channel::<BackendResult>();
        let cancelled = Arc::new(AtomicBool::new(false));
        tx.send(BackendRequest {
            kind,
            reply: reply_tx,
            cancelled: Arc::clone(&cancelled),
            control,
        })
        .map_err(|_| {
            // All senders dead means the worker thread is dead: migrate the state machine
            // from Running to sticky StartFailed so this request and all later ones get a
            // clear error.
            self.mark_thread_dead();
            ComputerUseError::unavailable("computer use backend thread died")
        })?;
        match reply_rx.recv_timeout(BACKEND_CALL_TIMEOUT) {
            Ok(result) => result,
            // Timeout: set the cancel flag so the worker skips the request after dequeue (a
            // request already inside an OS call cannot be interrupted, see the worker_loop
            // comment). The wording honestly discloses the residual uncertainty: the
            // audit's error record must admit the action may still complete or have
            // partially completed, same as the panic path.
            Err(RecvTimeoutError::Timeout) => {
                cancelled.store(true, Ordering::SeqCst);
                Err(ComputerUseError::unavailable(format!(
                    "computer use backend did not respond within {BACKEND_CALL_TIMEOUT:?}; \
                     the action may still execute or has partially executed"
                )))
            }
            // Disconnect = the worker has exited, a different failure from a timeout, so
            // it is reported separately (both used to be lumped together as "did not
            // respond within <BACKEND_CALL_TIMEOUT>"). Setting the cancel flag here is
            // purely defensive (the dead worker will not consume requests anymore) and
            // lets the state machine settle in sync.
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
    /// comment promised but `request` never delivered. Control requests are
    /// also exempt from the dequeue skip: a cleanup that
    /// timed out behind a wedged worker must run late when the worker
    /// recovers, never be discarded.
    fn request_control(&self, kind: BackendRequestKind) -> Result<BackendReply, ComputerUseError> {
        self.request_inner(kind, true)
    }
}

impl Drop for BackendInner {
    fn drop(&mut self) {
        // Transition the state machine to Shutdown under the lock, then
        // RELEASE the guard before teardown: the worker may be wedged in an
        // OS call indefinitely, and joining while holding the state mutex
        // would hang the dropping thread (possibly a Tauri/async thread at
        // teardown) forever while every other caller on the handle blocks
        // on that mutex.
        // Blocking bound (symmetric to a control request's call-budget
        // wait): `ensure_sender` holds this same mutex while waiting for
        // worker startup, up to BACKEND_CALL_TIMEOUT (700s), so a Drop
        // racing a lazy start can block on the lock for up to 700s.
        let previous = {
            let mut state = self.state.lock();
            std::mem::replace(&mut *state, WorkerState::Shutdown)
        };
        if let WorkerState::Running { tx, thread } = previous {
            let _ = tx.send(BackendRequest {
                kind: BackendRequestKind::Shutdown,
                reply: channel::<BackendResult>().0,
                // The Shutdown request must never carry a set cancel flag: the worker would
                // check the flag and skip first, never reaching the Shutdown branch to exit.
                // control=true as a second line of defense.
                cancelled: Arc::new(AtomicBool::new(false)),
                control: true,
            });
            // Drop the sender first (the worker's rx.recv() sees the disconnect and exits
            // the loop), then drop the JoinHandle so the thread detaches instead of joining
            // (same class as the startup-timeout cleanup in `ensure_sender`): when the
            // worker is wedged inside a single OS call, join would hang the cleanup thread
            // executing Drop forever — after the call returns, the wedged worker still sees
            // the disconnect and exits on its own; detach at worst leaves one lingering
            // thread and never blocks any caller.
            drop(tx);
            drop(thread);
        }
    }
}

/// The cloneable backend handle (a channel wrapper). All methods block synchronously;
/// async callers should wrap in `tauri::async_runtime::spawn_blocking`.
#[derive(Clone)]
pub struct BackendHandle {
    inner: Arc<BackendInner>,
}

impl BackendHandle {
    /// Arc identity comparison: whether the two handles point at the same backend instance.
    pub fn is_same_backend(&self, other: &BackendHandle) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Lazy start: the factory executes on the worker thread only at the first request
    /// (constructing xcap/enigo objects).
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

    /// Global INPUT-space coordinates — the same space `move_to` / `click`
    /// / `drag` take (see the [`ComputerUseBackend`] trait contract).
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

    /// Coordinates are in input coordinate space.
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

    /// The element currently holding keyboard focus. Err = backend unsupported/query failed
    /// (tool layer is best-effort: when screening is unavailable the action proceeds, and
    /// only a positive hit requires confirmation).
    pub fn focused_element(&self) -> Result<Option<ElementInfo>, ComputerUseError> {
        match self.inner.request(BackendRequestKind::FocusedElement)? {
            BackendReply::Element(element) => Ok(element),
            _ => Err(ComputerUseError::failed("unexpected backend reply")),
        }
    }

    /// Close the persistent OS-level authorization held by this session's backend (see the
    /// trait method of the same name). Blocks until the worker answers (it may queue behind
    /// an in-flight action); UI callers should trigger it on a detached thread via
    /// [`BackendRegistry`] and not block the event loop.
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

    /// Best-effort physical button release through the control lane
    /// (bypasses the in-flight gate, queues behind any in-flight action).
    /// Emergency cleanup for sessions that may die mid-drag: a model that
    /// pressed a button and was stopped/revoked/dropped must not leave the
    /// user's machine in button-held state.
    ///
    /// Latency bound: the three button releases are three independent
    /// control-lane requests, each with its own BACKEND_CALL_TIMEOUT (700s)
    /// budget, so when each one queues behind a wedged worker and then
    /// waits out its full budget the worst case is ~35 minutes; with a
    /// healthy worker the whole call is sub-second.
    pub fn emergency_mouse_up(&self) -> Result<(), ComputerUseError> {
        // Release the three buttons one by one (synthetic presses of the
        // right/middle buttons also strand on stop/drop — previously only the
        // left button was released). For unpressed buttons, every backend's
        // harmless no-op (consistent with the established line in the cleanup comment
        // below); better one release too many than a stuck button.
        let mut first_err = None;
        for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
            if let Err(error) = self.unit_control(BackendRequestKind::MouseUp { button }) {
                if first_err.is_none() {
                    first_err = Some(error);
                }
            }
        }
        match first_err {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// Session → backend-handle registry. Registered when a tool is constructed, unregistered on
/// drop; on revoke/stop/master-switch off the command layer triggers `release_os_grant` for
/// the corresponding session (after the user "stops control", the OS-level portal grant must
/// terminate with it instead of living until process exit).
#[derive(Default)]
pub struct BackendRegistry {
    handles: Mutex<HashMap<String, BackendHandle>>,
}

impl BackendRegistry {
    /// Register the session handle at tool construction (a duplicate registration for the
    /// same session keeps the newest).
    pub fn insert(&self, session_id: &str, handle: BackendHandle) {
        self.handles.lock().insert(session_id.to_string(), handle);
    }

    // Both release paths funnel into [`emergency_cleanup`] (physical button
    // up, then OS-grant close). A bare remove with no cleanup was removed on
    // purpose: an unregister-only path would let a dying session skip the
    // button/grant cleanup while still looking successful.

    /// Emergency cleanup for one session (per-session revoke): on a detached
    /// thread, first unpress the physical left button (a model that pressed
    /// and died must not leave the machine in button-held drag state), then
    /// close the persistent OS-level grant. Both go through the control
    /// lane, so they are NOT rejected while an action is in flight — they
    /// queue behind it on the serialized worker channel and run once it
    /// resolves. Errors are only logged: the worst case (a leaked grant or
    /// a held button) must never panic or block the caller.
    ///
    /// Known trade-off (acknowledged rather than gated): the
    /// synthetic mouse-up is machine-global and the control lane does not
    /// hold the physical-input lock, so session B's cleanup releases the
    /// button even while session A (or the user) is mid-drag. Tracking
    /// per-session pressed state to gate this was rejected: the cleanup
    /// exists precisely because state may be inconsistent, and a wrong
    /// "we never pressed" bookkeeping would strand a real held button —
    /// the worse failure. The tool-layer fallback (tool.rs) releases only
    /// what the tool itself pressed; this path is the safety net.
    ///
    /// The registration is KEPT: cleanup is idempotent (the OS-grant close
    /// is take()-based inside the backend, and a mouse-up on an unpressed
    /// button is a no-op), and a session re-granted after this revoke must
    /// stay reachable for a later global stop
    /// ([`Self::emergency_release_all`]). Live tools leave the registry via
    /// [`Self::release_and_unregister`] on Drop.
    pub fn emergency_release(&self, session_id: &str) {
        let Some(handle) = self.handles.lock().get(session_id).cloned() else {
            return;
        };
        std::thread::spawn(move || emergency_cleanup(handle));
    }

    /// [`Self::emergency_release`]'s cleanup plus synchronous
    /// unregistration: the registry entry is removed first, then the same
    /// cleanup runs on a detached thread. Used by tool Drop so a dropped
    /// tool cannot leave its worker pinned in the registry.
    ///
    /// Identity-checked: the entry is removed only when it still holds *this*
    /// tool's handle (Arc identity). Same-session factory re-invocation can
    /// let a new tool register before the old one drops; a blind remove-by-id
    /// would unregister the NEW tool, and its own Drop would then early-return,
    /// skipping the button/portal cleanup entirely.
    pub fn release_and_unregister(&self, session_id: &str, handle: &BackendHandle) {
        // Identity check and remove/reinsert happen under ONE lock
        // acquisition: the previous remove-then-
        // reinsert left a window where the successor tool's own Drop could
        // find the map empty and early-return — skipping its emergency
        // cleanup — while the late reinsert afterwards pinned its handle as
        // a ghost registry entry no Drop would ever remove. BOTH arms
        // schedule an emergency cleanup — the matched entry is this tool's
        // own handle, the mismatched one is the stale clone.
        let cleanup = {
            let mut handles = self.handles.lock();
            match handles.remove(session_id) {
                Some(stored) if Arc::ptr_eq(&stored.inner, &handle.inner) => Some(stored),
                // The entry was already replaced by a new tool of the same session: restore
                // it and clean up only our own handle.
                other => {
                    if let Some(stored) = other {
                        handles.insert(session_id.to_string(), stored);
                    }
                    Some(handle.clone())
                }
            }
        };
        if let Some(target) = cleanup {
            std::thread::spawn(move || emergency_cleanup(target));
        }
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

    /// The currently registered handle for the same session, if any. tool Drop uses it to
    /// decide whether it is still the session's active tool: when a
    /// same-session factory re-enters, a late old tool's Drop must not revoke the session
    /// grant the new tool just obtained — the registry-side identity check must extend to
    /// the guard-side consent revocation.
    pub fn registered_handle(&self, session_id: &str) -> Option<BackendHandle> {
        self.handles.lock().get(session_id).cloned()
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
        ups: usize,
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
            self.ups += 1;
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
            Ok(Box::new(MockBackend { clicks: 0, ups: 0 }))
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
        // The second request does not rebuild the thread (sticky failure).
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
        // After the Drop, the worker thread receives Shutdown and drops the backend object.
        // Under the detach semantics (when the worker is wedged, join would pin the
        // cleanup thread forever) the teardown completes asynchronously —
        // wait with a bound instead of asserting immediately.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !dropped.load(Ordering::SeqCst) {
            assert!(
                std::time::Instant::now() < deadline,
                "worker backend was not dropped after the handle was dropped"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Regression: the worker hands the cancel flag to the backend
    /// before dispatching the request and clears it afterwards — only then can multi-event
    /// injection (type) check the flag between events and stop injecting; the flag must be
    /// reset after dispatch ends so a stale flag does not affect later requests.
    #[test]
    fn cancel_flag_reaches_the_backend_during_dispatch_only() {
        use std::sync::Mutex;
        // Record every set_cancel_flag argument (set before dispatch, cleared after).
        let seen: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
        struct FlagRecordingBackend {
            seen: Arc<Mutex<Vec<bool>>>,
        }
        impl ComputerUseBackend for FlagRecordingBackend {
            fn capabilities(&self) -> Capabilities {
                Capabilities {
                    screenshot: false,
                    input: true,
                    ui_tree: false,
                    notes: "test".to_string(),
                }
            }
            fn set_cancel_flag(&mut self, flag: Option<Arc<AtomicBool>>) {
                self.seen.lock().unwrap().push(flag.is_some());
            }
            fn capture(&mut self) -> Result<Capture, ComputerUseError> {
                Err(ComputerUseError::unsupported("capture", "test"))
            }
            fn cursor_position(&mut self) -> Result<(i32, i32), ComputerUseError> {
                Err(ComputerUseError::unsupported("cursor_position", "test"))
            }
            fn move_to(&mut self, _x: i32, _y: i32) -> Result<(), ComputerUseError> {
                Err(ComputerUseError::unsupported("move_to", "test"))
            }
            fn click(&mut self, _b: MouseButton, _c: u8) -> Result<(), ComputerUseError> {
                Err(ComputerUseError::unsupported("click", "test"))
            }
            fn mouse_down(&mut self, _b: MouseButton) -> Result<(), ComputerUseError> {
                Err(ComputerUseError::unsupported("mouse_down", "test"))
            }
            fn mouse_up(&mut self, _b: MouseButton) -> Result<(), ComputerUseError> {
                Err(ComputerUseError::unsupported("mouse_up", "test"))
            }
            fn drag(&mut self, _from: (i32, i32), _to: (i32, i32)) -> Result<(), ComputerUseError> {
                Err(ComputerUseError::unsupported("drag", "test"))
            }
            fn scroll(&mut self, _d: ScrollDirection, _c: u32) -> Result<(), ComputerUseError> {
                Err(ComputerUseError::unsupported("scroll", "test"))
            }
            fn type_text(&mut self, _t: &str) -> Result<(), ComputerUseError> {
                Ok(())
            }
            fn key_chord(&mut self, _k: &[Key]) -> Result<(), ComputerUseError> {
                Err(ComputerUseError::unsupported("key_chord", "test"))
            }
            fn hold_key(&mut self, _k: &[Key], _ms: u64) -> Result<(), ComputerUseError> {
                Err(ComputerUseError::unsupported("hold_key", "test"))
            }
            fn ui_tree(&mut self, _o: &UiTreeOptions) -> Result<String, ComputerUseError> {
                Err(ComputerUseError::unsupported("ui_tree", "test"))
            }
            fn element_at_point(
                &mut self,
                _x: i32,
                _y: i32,
            ) -> Result<Option<ElementInfo>, ComputerUseError> {
                Err(ComputerUseError::unsupported("element_at_point", "test"))
            }
        }
        let (req_tx, req_rx) = channel::<BackendRequest>();
        let (startup_tx, startup_rx) = channel::<Result<(), ComputerUseError>>();
        let seen_for_worker = Arc::clone(&seen);
        let factory: BackendFactory = Box::new(move || {
            Ok(Box::new(FlagRecordingBackend {
                seen: seen_for_worker,
            }) as Box<dyn ComputerUseBackend>)
        });
        let worker = std::thread::spawn(move || worker_loop(factory, req_rx, startup_tx));
        assert!(startup_rx.recv().expect("startup channel alive").is_ok());
        for _ in 0..2 {
            let (reply_tx, reply_rx) = channel::<BackendResult>();
            req_tx
                .send(BackendRequest {
                    kind: BackendRequestKind::TypeText { text: "hi".into() },
                    reply: reply_tx,
                    cancelled: Arc::new(AtomicBool::new(false)),
                    control: false,
                })
                .expect("worker alive");
            let _ = reply_rx.recv().expect("worker must reply");
        }
        drop(req_tx);
        worker.join().expect("worker exits when channel closes");
        // Exactly one set plus one clear per dispatch (even though the flag is never set
        // between requests).
        assert_eq!(
            *seen.lock().unwrap(),
            vec![true, false, true, false],
            "cancel flag must be handed to the backend before dispatch and cleared after"
        );
    }

    /// Regression: a request the caller has abandoned (flag set on timeout/
    /// disconnect) is skipped after the worker dequeues it and before execution, replying
    /// with a clear cancellation error — executing it as usual would break cross-session
    /// serialization.
    #[test]
    fn worker_skips_requests_cancelled_before_dequeue() {
        let (req_tx, req_rx) = channel::<BackendRequest>();
        let (startup_tx, startup_rx) = channel::<Result<(), ComputerUseError>>();
        let factory: BackendFactory = Box::new(|| {
            Ok(Box::new(MockBackend { clicks: 0, ups: 0 }) as Box<dyn ComputerUseBackend>)
        });
        let worker = std::thread::spawn(move || worker_loop(factory, req_rx, startup_tx));
        assert!(startup_rx.recv().expect("startup channel alive").is_ok());
        let (reply_tx, reply_rx) = channel::<BackendResult>();
        req_tx
            .send(BackendRequest {
                kind: BackendRequestKind::Capabilities,
                reply: reply_tx,
                cancelled: Arc::new(AtomicBool::new(true)),
                control: false,
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

    /// Control-lane requests are EXEMPT from the dequeue
    /// skip — a cleanup that timed out behind a wedged worker must run late
    /// when the worker recovers, never be discarded (a discarded emergency
    /// mouse-up strands the held button; a discarded ReleaseOsGrant keeps
    /// the portal authorization alive past the user's Stop).
    #[test]
    fn worker_executes_cancelled_control_requests_late() {
        let (req_tx, req_rx) = channel::<BackendRequest>();
        let (startup_tx, startup_rx) = channel::<Result<(), ComputerUseError>>();
        let factory: BackendFactory = Box::new(|| {
            Ok(Box::new(MockBackend { clicks: 0, ups: 0 }) as Box<dyn ComputerUseBackend>)
        });
        let worker = std::thread::spawn(move || worker_loop(factory, req_rx, startup_tx));
        assert!(startup_rx.recv().expect("startup channel alive").is_ok());
        let (reply_tx, reply_rx) = channel::<BackendResult>();
        req_tx
            .send(BackendRequest {
                kind: BackendRequestKind::MouseUp {
                    button: MouseButton::Left,
                },
                reply: reply_tx,
                // Simulate: the control request waited out the whole call budget behind a
                // wedged worker, and the caller (emergency cleanup) gave up on timeout one
                // step earlier and set the flag.
                cancelled: Arc::new(AtomicBool::new(true)),
                control: true,
            })
            .expect("worker alive");
        reply_rx
            .recv()
            .expect("worker must answer")
            .expect("a cancelled CONTROL request must still execute, not be skipped");
        drop(req_tx);
        worker.join().expect("worker exits when channel closes");
    }

    /// Regression: the request channel is unbounded, so with a wedged
    /// worker every new call would occupy a blocking thread for up to
    /// BACKEND_CALL_TIMEOUT — the in-flight flag pins concurrent in-flight
    /// requests to 1.
    #[test]
    fn concurrent_requests_are_rejected_while_one_is_in_flight() {
        let handle = BackendHandle::lazy(|| {
            Ok(Box::new(MockBackend { clicks: 0, ups: 0 }) as Box<dyn ComputerUseBackend>)
        });
        // Set the flag directly to simulate "a request already in flight" (the real path
        // acquires/releases it in `request`).
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
        // Bounded wait: if the helper thread dies before setting the flag
        // (expect panic), an unbounded yield spin would turn the fast failure into hanging
        // until the test times out.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !handle.inner.in_flight.load(Ordering::SeqCst) {
            assert!(
                std::time::Instant::now() < deadline,
                "capture never became in-flight; the helper thread likely died"
            );
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
        // Three buttons released one by one (the established contract of emergency_mouse_up,
        // see its docs).
        assert_eq!(
            state.mouse_ups,
            vec![MouseButton::Left, MouseButton::Right, MouseButton::Middle]
        );
    }

    /// Waits until `predicate` observes the emergency cleanup in `state`
    /// (cleanup runs on a detached thread, so it is only observable by
    /// polling); fails the test after ~5s.
    fn wait_for_cleanup(
        state: &Arc<Mutex<ProbeState>>,
        mut predicate: impl FnMut(&ProbeState) -> bool,
    ) {
        for _ in 0..500 {
            if predicate(&state.lock()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("emergency cleanup did not reach the backend in time");
    }

    fn probe_handle(state: &Arc<Mutex<ProbeState>>) -> BackendHandle {
        let state_for_factory = Arc::clone(state);
        BackendHandle::lazy(move || {
            Ok(Box::new(ControlProbeBackend {
                state: state_for_factory,
            }) as Box<dyn ComputerUseBackend>)
        })
    }

    /// Regression (per-session revoke must not orphan the global-stop
    /// teardown): `emergency_release` runs the cleanup (button up + OS-grant
    /// close) but KEEPS the registration, so a later `emergency_release_all`
    /// still reaches a session that was re-granted after the revoke.
    #[test]
    fn emergency_release_keeps_registration_and_cleans_up() {
        let state = Arc::new(Mutex::new(ProbeState::default()));
        let registry = BackendRegistry::default();
        registry.insert("s-revoke", probe_handle(&state));

        registry.emergency_release("s-revoke");
        assert!(
            registry.contains("s-revoke"),
            "per-session revoke must keep the registration reachable for a later global stop"
        );
        wait_for_cleanup(&state, |s| s.releases > 0 && !s.mouse_ups.is_empty());
        {
            let state = state.lock();
            assert_eq!(
                state.mouse_ups,
                vec![MouseButton::Left, MouseButton::Right, MouseButton::Middle]
            );
            assert_eq!(state.releases, 1);
        }

        // Simulate the re-grant path: the session is still registered (same
        // handle keeps acting), a global stop must reach it again.
        registry.emergency_release_all();
        assert!(
            registry.contains("s-revoke"),
            "emergency_release_all keeps registrations (live tools unregister via Drop)"
        );
        wait_for_cleanup(&state, |s| s.releases > 1);
        let state = state.lock();
        assert_eq!(
            state.mouse_ups,
            vec![
                MouseButton::Left,
                MouseButton::Right,
                MouseButton::Middle,
                MouseButton::Left,
                MouseButton::Right,
                MouseButton::Middle
            ]
        );
        assert_eq!(state.releases, 2);
    }

    /// `release_and_unregister` (tool-Drop path) removes the registry entry
    /// synchronously AND still runs the button/grant cleanup on the detached
    /// thread.
    #[test]
    fn release_and_unregister_removes_entry_and_cleans_up() {
        let state = Arc::new(Mutex::new(ProbeState::default()));
        let registry = BackendRegistry::default();
        let handle = probe_handle(&state);
        registry.insert("s-drop", handle.clone());

        registry.release_and_unregister("s-drop", &handle);
        assert!(
            !registry.contains("s-drop"),
            "release_and_unregister must remove the registry entry synchronously"
        );
        wait_for_cleanup(&state, |s| s.releases > 0 && !s.mouse_ups.is_empty());
        let state = state.lock();
        assert_eq!(
            state.mouse_ups,
            vec![MouseButton::Left, MouseButton::Right, MouseButton::Middle]
        );
        assert_eq!(state.releases, 1);
    }

    /// Identity check: a same-session re-registration means the
    /// registry entry belongs to the NEW tool's handle. An old tool's Drop
    /// must not unregister it — the stale entry is restored, and only the
    /// old tool's own backend gets the emergency cleanup.
    #[test]
    fn release_and_unregister_keeps_a_re_registered_successor() {
        let old_state = Arc::new(Mutex::new(ProbeState::default()));
        let new_state = Arc::new(Mutex::new(ProbeState::default()));
        let registry = BackendRegistry::default();
        let old_handle = probe_handle(&old_state);
        let new_handle = probe_handle(&new_state);
        registry.insert("s-rereg", old_handle.clone());
        // The new tool registered before the old tool dropped: the entry is
        // now the new handle.
        registry.insert("s-rereg", new_handle.clone());

        registry.release_and_unregister("s-rereg", &old_handle);
        assert!(
            registry.contains("s-rereg"),
            "the successor's registration must survive a stale tool's Drop"
        );
        wait_for_cleanup(&old_state, |s| s.releases > 0);
        assert_eq!(
            new_state.lock().releases,
            0,
            "the successor's backend must not be cleaned up by the stale Drop"
        );
        // The successor's own Drop removes the entry for real.
        registry.release_and_unregister("s-rereg", &new_handle);
        assert!(
            !registry.contains("s-rereg"),
            "the successor's Drop must remove its own registration"
        );
    }

    /// A global stop must reach a session whose earlier revoke only cleaned
    /// up without unregistering — the exact orphaning regression.
    #[test]
    fn emergency_release_all_reaches_a_revoked_then_re_granted_session() {
        let state = Arc::new(Mutex::new(ProbeState::default()));
        let registry = BackendRegistry::default();
        registry.insert("s-regrant", probe_handle(&state));

        registry.emergency_release("s-regrant");
        wait_for_cleanup(&state, |s| s.releases > 0);
        // Re-grant happened meanwhile; no re-insert: the original entry is
        // still there and must still be reachable.
        registry.emergency_release_all();
        wait_for_cleanup(&state, |s| s.releases > 1);
        let state = state.lock();
        assert_eq!(
            state.releases, 2,
            "global stop must reach the re-granted session"
        );
    }
}
