//! Adapter between the synchronous TUI boundary and authenticated Controller IPC.
//!
//! Every operation owns its connection. Cancellation handles are registered before
//! authentication starts, so TUI shutdown can wake a read blocked in either the
//! handshake or a request. Detach operations only cancel local I/O; they never
//! send a Controller method and therefore cannot interrupt or approve remote work.

use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use pinvou_controller::{ControllerPaths, LocalEndpoint};
use pinvou_protocol::{
    HelloClient, HelloServer, IpcMessage, IpcMessageKind, RuntimeEventEnvelope, RuntimeEventKind,
    StableExitCode, encode_frame, read_frame,
};
use pinvou_tui::backend::{
    Backend, BackendError, BackendErrorKind, EventEmitter, RuntimeList, RuntimeStatus,
};
use serde_json::{Value, json};

use super::{ControllerWire, DistributedError};

trait ReadWrite: Read + Write + Send {}
impl<T: Read + Write + Send> ReadWrite for T {}
type TuiWire = ControllerWire<Box<dyn ReadWrite>>;
type OpenedConnection = (TuiWire, Arc<dyn CancelHandle>, RegistrationGuard);

struct Connected {
    io: Box<dyn ReadWrite>,
    cancel: Arc<dyn CancelHandle>,
}

trait CancelHandle: Send + Sync {
    fn cancel(&self);
    fn is_cancelled(&self) -> bool;
}

trait ConnectionFactory: Send + Sync {
    fn connect(&self) -> Result<Connected, DistributedError>;
}

struct LocalConnectionFactory {
    endpoint: LocalEndpoint,
}

impl ConnectionFactory for LocalConnectionFactory {
    fn connect(&self) -> Result<Connected, DistributedError> {
        connect_local(&self.endpoint)
    }
}

#[derive(Default)]
struct InFlight {
    next_connection_id: AtomicU64,
    streams: Mutex<HashMap<u64, RegisteredCancel>>,
    controls: Mutex<HashMap<u64, Arc<dyn CancelHandle>>>,
}

struct RegisteredCancel {
    connection_id: u64,
    cancel: Arc<dyn CancelHandle>,
}

enum RegistrationKind {
    Stream { operation_token: u64 },
    Control,
}

struct RegistrationGuard {
    in_flight: Arc<InFlight>,
    kind: RegistrationKind,
    connection_id: u64,
}

impl Drop for RegistrationGuard {
    fn drop(&mut self) {
        match self.kind {
            RegistrationKind::Stream { operation_token } => {
                if let Ok(mut streams) = self.in_flight.streams.lock()
                    && streams
                        .get(&operation_token)
                        .is_some_and(|entry| entry.connection_id == self.connection_id)
                {
                    streams.remove(&operation_token);
                }
            }
            RegistrationKind::Control => {
                if let Ok(mut controls) = self.in_flight.controls.lock() {
                    controls.remove(&self.connection_id);
                }
            }
        }
    }
}

/// Production TUI backend. It is safe to share across the TUI worker threads.
pub struct ControllerTuiBackend {
    workspace: PathBuf,
    connector: Arc<dyn ConnectionFactory>,
    in_flight: Arc<InFlight>,
}

impl ControllerTuiBackend {
    pub fn discover(workspace: impl AsRef<Path>) -> Result<Self, BackendError> {
        let paths = ControllerPaths::discover().map_err(|error| {
            BackendError::new(BackendErrorKind::ControllerUnavailable, error.to_string())
                .with_exit_code(error.exit_code())
        })?;
        Self::with_connector(
            workspace.as_ref().to_path_buf(),
            Arc::new(LocalConnectionFactory {
                endpoint: paths.endpoint().clone(),
            }),
        )
    }

    fn with_connector(
        workspace: PathBuf,
        connector: Arc<dyn ConnectionFactory>,
    ) -> Result<Self, BackendError> {
        let workspace = workspace.canonicalize().map_err(|_| {
            BackendError::new(BackendErrorKind::Operation, "workspace path is unavailable")
        })?;
        Ok(Self {
            workspace,
            connector,
            in_flight: Arc::new(InFlight::default()),
        })
    }

    fn open_stream(&self, operation_token: u64) -> Result<OpenedConnection, BackendError> {
        let connected = self.connector.connect().map_err(map_distributed)?;
        let connection_id = self
            .in_flight
            .next_connection_id
            .fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::clone(&connected.cancel);
        self.in_flight.streams.lock().map_err(lock_error)?.insert(
            operation_token,
            RegisteredCancel {
                connection_id,
                cancel: Arc::clone(&cancel),
            },
        );
        let guard = RegistrationGuard {
            in_flight: Arc::clone(&self.in_flight),
            kind: RegistrationKind::Stream { operation_token },
            connection_id,
        };
        let wire = authenticate(connected.io).map_err(|error| map_with_cancel(error, &*cancel))?;
        Ok((wire, cancel, guard))
    }

    fn open_control(&self) -> Result<OpenedConnection, BackendError> {
        let connected = self.connector.connect().map_err(map_distributed)?;
        let connection_id = self
            .in_flight
            .next_connection_id
            .fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::clone(&connected.cancel);
        self.in_flight
            .controls
            .lock()
            .map_err(lock_error)?
            .insert(connection_id, Arc::clone(&cancel));
        let guard = RegistrationGuard {
            in_flight: Arc::clone(&self.in_flight),
            kind: RegistrationKind::Control,
            connection_id,
        };
        let wire = authenticate(connected.io).map_err(|error| map_with_cancel(error, &*cancel))?;
        Ok((wire, cancel, guard))
    }

    fn control<T>(
        &self,
        operation: impl FnOnce(&mut TuiWire) -> Result<T, DistributedError>,
    ) -> Result<T, BackendError> {
        let (mut wire, cancel, _guard) = self.open_control()?;
        operation(&mut wire).map_err(|error| map_with_cancel(error, &*cancel))
    }

    fn detect(&self, runtime: &str) -> Result<Value, BackendError> {
        let response = self.control(|wire| wire.runtime_detect(Some(runtime)))?;
        require_response(response, "runtime.detect")
    }
}

impl Backend for ControllerTuiBackend {
    fn workspace(&self) -> Result<PathBuf, BackendError> {
        Ok(self.workspace.clone())
    }

    fn runtime_list(&self) -> Result<RuntimeList, BackendError> {
        let response = self.control(ControllerWire::runtime_list)?;
        let payload = require_response(response, "runtime.list")?;
        let listed = map_runtime_list(&payload)?;
        let mut runtimes = Vec::with_capacity(listed.runtimes.len());
        for candidate in listed.runtimes {
            let detected = map_runtime_status(&self.detect(&candidate.id)?, Some(&candidate.id))?;
            let mut status =
                RuntimeStatus::new(detected.id, candidate.display_name, detected.available);
            status.capability_summary =
                detected.capability_summary.or(candidate.capability_summary);
            runtimes.push(status);
        }
        Ok(RuntimeList::new(listed.active_runtime, runtimes))
    }

    fn stream_turn(
        &self,
        operation_token: u64,
        prompt: String,
        mut emit: EventEmitter,
    ) -> Result<(), BackendError> {
        let (mut wire, cancel, _guard) = self.open_stream(operation_token)?;
        let mut message = wire
            .chat_start(&prompt)
            .map_err(|error| map_with_cancel(error, &*cancel))?;
        let mut active_turn = None::<String>;
        loop {
            if message.kind() != IpcMessageKind::Evt || message.topic() != Some("runtime.event") {
                return Err(protocol_error(
                    "controller returned an invalid runtime event",
                ));
            }
            let event = RuntimeEventEnvelope::from_value(message.payload().clone())
                .map_err(|_| protocol_error("controller returned a malformed runtime event"))?;
            if event.event_kind() == RuntimeEventKind::TurnStarted {
                let turn_id = event
                    .turn_id()
                    .ok_or_else(|| protocol_error("turn.started has no turn id"))?;
                if active_turn.replace(turn_id.to_owned()).is_some() {
                    return Err(protocol_error(
                        "controller started another turn on the same stream",
                    ));
                }
            }
            let terminal = event.event_kind() == RuntimeEventKind::TurnEnded;
            if terminal
                && active_turn
                    .as_deref()
                    .is_none_or(|turn_id| event.turn_id().is_none_or(|ended| ended != turn_id))
            {
                return Err(protocol_error(
                    "controller returned turn.ended for another turn",
                ));
            }
            if let Err(error) = emit(event) {
                cancel.cancel();
                return Err(error);
            }
            if terminal {
                return Ok(());
            }
            message = wire
                .read_next()
                .map_err(|error| map_with_cancel_or_protocol(error, &*cancel))?;
        }
    }

    fn detach_stream(&self, operation_token: u64) -> Result<(), BackendError> {
        let cancel = self
            .in_flight
            .streams
            .lock()
            .map_err(lock_error)?
            .get(&operation_token)
            .map(|entry| Arc::clone(&entry.cancel));
        if let Some(cancel) = cancel {
            cancel.cancel();
        }
        Ok(())
    }

    fn detach_controls(&self) -> Result<(), BackendError> {
        let cancellations = self
            .in_flight
            .controls
            .lock()
            .map_err(lock_error)?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for cancel in cancellations {
            cancel.cancel();
        }
        Ok(())
    }

    fn resolve_approval(&self, approval_id: String, accepted: bool) -> Result<(), BackendError> {
        self.control(|wire| wire.resolve_approval(&approval_id, accepted))
            .and_then(|message| require_response(message, "approval.resolve"))?;
        Ok(())
    }

    fn resolve_input(&self, input_id: String, value: String) -> Result<(), BackendError> {
        self.control(|wire| wire.resolve_input(&input_id, &value))
            .and_then(|message| require_response(message, "input.resolve"))?;
        Ok(())
    }

    fn interrupt(&self, turn_id: String) -> Result<(), BackendError> {
        self.control(|wire| wire.interrupt_turn(&turn_id))
            .and_then(|message| require_response(message, "turn.interrupt"))?;
        Ok(())
    }

    fn switch_runtime(&self, runtime: String) -> Result<RuntimeStatus, BackendError> {
        let initial = self.detect(&runtime)?;
        let initial_status = map_runtime_status(&initial, Some(&runtime))?;
        if !initial_status.available {
            return Err(map_status_error(&initial));
        }

        let prepared = self
            .control(|wire| wire.runtime_switch_prepare(&runtime))
            .and_then(|message| require_response(message, "runtime.switch.prepare"))?;
        validate_prepare(&prepared, &runtime)?;
        let token = prepared["switch_token"].as_str().unwrap().to_owned();
        let committed = self
            .control(|wire| wire.runtime_switch_commit(&runtime, &token))
            .and_then(|message| require_response(message, "runtime.switch.commit"))?;
        if committed.get("runtime").and_then(Value::as_str) != Some(runtime.as_str())
            || committed.get("switch_token").and_then(Value::as_str) != Some(token.as_str())
        {
            return Err(protocol_error(
                "runtime switch commit response does not match prepare",
            ));
        }
        let final_status = map_runtime_status(&self.detect(&runtime)?, Some(&runtime))?;
        if final_status.id != runtime {
            return Err(protocol_error(
                "runtime switch verification returned another runtime",
            ));
        }
        Ok(final_status)
    }
}

fn authenticate(mut io: Box<dyn ReadWrite>) -> Result<TuiWire, DistributedError> {
    let hello = HelloClient::new(json!({"name":"pinvou-tui","pid":std::process::id()}))
        .map_err(|_| DistributedError::controller("controller hello is invalid"))?;
    io.write_all(
        &encode_frame(&hello)
            .map_err(|_| DistributedError::controller("controller hello is too large"))?,
    )
    .map_err(|_| DistributedError::controller("controller hello write failed"))?;
    io.flush()
        .map_err(|_| DistributedError::controller("controller hello flush failed"))?;
    let answer: HelloServer = read_frame(&mut io)
        .map_err(|_| DistributedError::controller("controller IPC version mismatch"))?;
    Ok(ControllerWire::from_authenticated(io, answer.instance_id()))
}

fn require_response(message: IpcMessage, operation: &str) -> Result<Value, BackendError> {
    if message.kind() == IpcMessageKind::Rsp {
        Ok(message.payload().clone())
    } else {
        Err(protocol_error(format!(
            "controller returned a non-response for {operation}"
        )))
    }
}

fn map_runtime_list(payload: &Value) -> Result<RuntimeList, BackendError> {
    let active = payload
        .get("current")
        .and_then(Value::as_str)
        .filter(|value| *value != "none")
        .map(str::to_owned);
    let runtimes = payload
        .get("runtimes")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol_error("runtime.list has no runtimes array"))?
        .iter()
        .map(|value| map_runtime_status(value, None))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(RuntimeList::new(active, runtimes))
}

fn map_runtime_status(
    value: &Value,
    expected_id: Option<&str>,
) -> Result<RuntimeStatus, BackendError> {
    let id = value
        .get("id")
        .or_else(|| value.get("runtime"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| protocol_error("runtime status has no id"))?;
    if expected_id.is_some_and(|expected| expected != id) {
        return Err(protocol_error("runtime status id does not match request"));
    }
    let label = value
        .get("label")
        .or_else(|| value.get("display_name"))
        .and_then(Value::as_str)
        .unwrap_or(id);
    let available = value
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| value.get("status").and_then(Value::as_str) == Some("available"));
    let mut status = RuntimeStatus::new(id, label, available);
    if let Some(capabilities) = value.get("capabilities").and_then(Value::as_object) {
        let mut names = capabilities
            .iter()
            .filter_map(|(name, enabled)| {
                enabled
                    .as_bool()
                    .filter(|enabled| *enabled)
                    .map(|_| name.clone())
            })
            .collect::<Vec<_>>();
        names.sort();
        if !names.is_empty() {
            status = status.with_capability_summary(names.join(", "));
        }
    }
    Ok(status)
}

fn validate_prepare(value: &Value, runtime: &str) -> Result<(), BackendError> {
    let compression = value.get("requires_compression").and_then(Value::as_bool);
    let context = value.get("context");
    let tools = value.get("tools");
    if value.get("runtime").and_then(Value::as_str) != Some(runtime)
        || value.get("status").and_then(Value::as_str) != Some("ready")
        || value
            .get("switch_token")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        || compression.is_none()
        || context
            .and_then(|value| value.get("strategy"))
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        || context
            .and_then(|value| value.get("reason"))
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        || context
            .and_then(|value| value.get("portable_checkpoint"))
            .and_then(Value::as_bool)
            .is_none()
        || tools
            .and_then(|value| value.get("policy"))
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        || tools
            .and_then(|value| value.get("active_tool_calls"))
            .and_then(Value::as_u64)
            != Some(0)
        || tools
            .and_then(|value| value.get("blocking_missing_tools"))
            .and_then(Value::as_array)
            .is_none_or(|missing| !missing.is_empty())
        || (compression == Some(false)
            && context
                .and_then(|value| value.get("strategy"))
                .and_then(Value::as_str)
                != Some("none"))
    {
        return Err(protocol_error("runtime switch prepare response is invalid"));
    }
    Ok(())
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> BackendError {
    BackendError::new(
        BackendErrorKind::Operation,
        "TUI backend state is unavailable",
    )
}

fn protocol_error(message: impl Into<String>) -> BackendError {
    BackendError::new(BackendErrorKind::Protocol, message)
        .with_exit_code(StableExitCode::RuntimeFailed)
}

fn map_with_cancel(error: DistributedError, cancel: &dyn CancelHandle) -> BackendError {
    if cancel.is_cancelled() {
        BackendError::new(
            BackendErrorKind::Cancelled,
            "local Controller request was detached",
        )
        .with_exit_code(StableExitCode::Cancelled)
    } else {
        map_distributed(error)
    }
}

fn map_with_cancel_or_protocol(error: DistributedError, cancel: &dyn CancelHandle) -> BackendError {
    if cancel.is_cancelled() {
        map_with_cancel(error, cancel)
    } else {
        protocol_error("controller runtime stream ended before turn.ended")
    }
}

fn map_distributed(error: DistributedError) -> BackendError {
    let code = error.exit_code();
    let kind = match code {
        StableExitCode::ControllerUnavailable => BackendErrorKind::ControllerUnavailable,
        StableExitCode::BlockedAuth => BackendErrorKind::AuthBlocked,
        StableExitCode::Cancelled => BackendErrorKind::Cancelled,
        StableExitCode::Usage | StableExitCode::DataCorruption | StableExitCode::Internal => {
            BackendErrorKind::Protocol
        }
        StableExitCode::Success
        | StableExitCode::RuntimeFailed
        | StableExitCode::ResourceExhausted => BackendErrorKind::Operation,
    };
    let raw = error.to_string();
    BackendError::new(kind, sanitize_message(raw)).with_exit_code(code)
}

fn map_status_error(value: &Value) -> BackendError {
    let code = match value.get("exit_code").and_then(Value::as_i64) {
        Some(3) => StableExitCode::ControllerUnavailable,
        Some(4) => StableExitCode::BlockedAuth,
        Some(6) => StableExitCode::Cancelled,
        Some(7) => StableExitCode::ResourceExhausted,
        Some(8) => StableExitCode::DataCorruption,
        _ => StableExitCode::RuntimeFailed,
    };
    let kind = match code {
        StableExitCode::ControllerUnavailable => BackendErrorKind::ControllerUnavailable,
        StableExitCode::BlockedAuth => BackendErrorKind::AuthBlocked,
        StableExitCode::Cancelled => BackendErrorKind::Cancelled,
        StableExitCode::Internal | StableExitCode::Usage | StableExitCode::DataCorruption => {
            BackendErrorKind::Protocol
        }
        _ => BackendErrorKind::Operation,
    };
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("runtime is not available");
    BackendError::new(kind, sanitize_message(message.to_owned())).with_exit_code(code)
}

fn sanitize_message(raw: String) -> String {
    let lower = raw.to_ascii_lowercase();
    if ["token", "secret", "password", "bearer"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        "controller request failed".to_owned()
    } else {
        raw
    }
}

#[cfg(windows)]
fn connect_local(endpoint: &LocalEndpoint) -> Result<Connected, DistributedError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE},
        System::{
            IO::{CancelIoEx, CancelSynchronousIo},
            Threading::{GetCurrentProcess, GetCurrentThread},
        },
    };

    let LocalEndpoint::WindowsPipe(name) = endpoint else {
        return Err(DistributedError::controller(
            "controller endpoint is invalid",
        ));
    };
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(name)
        .map_err(|_| DistributedError::controller("controller is not reachable"))?;
    struct SharedFile {
        file: Arc<std::fs::File>,
        cancelled: Arc<AtomicBool>,
    }
    impl Read for SharedFile {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.cancelled.load(Ordering::Acquire) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "local IPC detached",
                ));
            }
            (&*self.file).read(buffer)
        }
    }
    impl Write for SharedFile {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            if self.cancelled.load(Ordering::Acquire) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "local IPC detached",
                ));
            }
            (&*self.file).write(buffer)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            (&*self.file).flush()
        }
    }
    struct WindowsCancel {
        file: Arc<std::fs::File>,
        /// Owned HANDLE duplicated from the worker thread that performs all
        /// synchronous I/O for this connection. Stored as an integer because
        /// windows-sys models HANDLE as a non-Send raw pointer.
        worker_thread: usize,
        cancelled: Arc<AtomicBool>,
    }
    impl Drop for WindowsCancel {
        fn drop(&mut self) {
            // SAFETY: `worker_thread` is an owned duplicated HANDLE and this
            // drop is its only close path.
            unsafe { CloseHandle(self.worker_thread as HANDLE) };
        }
    }
    impl CancelHandle for WindowsCancel {
        fn cancel(&self) {
            self.cancelled.store(true, Ordering::Release);
            // SAFETY: `file` keeps this exact HANDLE alive for the duration of
            // CancelIoEx. Passing null cancels every outstanding operation on
            // this one client connection and has no remote runtime semantics.
            unsafe {
                CancelIoEx(self.file.as_raw_handle(), std::ptr::null_mut());
            }
            // The named pipe File uses synchronous ReadFile. There is a very
            // small interval between the pre-read cancellation check and the
            // kernel operation becoming pending. Retry briefly so cancellation
            // covers that interval while keeping detach well below its 100 ms
            // budget. New work cannot start on this worker until its guard drops.
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(20);
            loop {
                let cancelled = unsafe { CancelSynchronousIo(self.worker_thread as HANDLE) };
                if cancelled != 0 || std::time::Instant::now() >= deadline {
                    break;
                }
                std::thread::yield_now();
            }
        }
        fn is_cancelled(&self) -> bool {
            self.cancelled.load(Ordering::Acquire)
        }
    }
    let file = Arc::new(file);
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut worker_thread: HANDLE = std::ptr::null_mut();
    // SAFETY: all source/target handles are valid pseudo-handles for the
    // current process/thread and `worker_thread` is valid writable storage.
    let duplicated = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            GetCurrentThread(),
            GetCurrentProcess(),
            &mut worker_thread,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if duplicated == 0 {
        return Err(DistributedError::controller(
            "controller cancellation handle is unavailable",
        ));
    }
    Ok(Connected {
        io: Box::new(SharedFile {
            file: Arc::clone(&file),
            cancelled: Arc::clone(&cancelled),
        }),
        cancel: Arc::new(WindowsCancel {
            file,
            worker_thread: worker_thread as usize,
            cancelled,
        }),
    })
}

#[cfg(target_os = "linux")]
fn connect_local(endpoint: &LocalEndpoint) -> Result<Connected, DistributedError> {
    use std::os::unix::net::{Shutdown, UnixStream};
    let LocalEndpoint::UnixSocket(path) = endpoint else {
        return Err(DistributedError::controller(
            "controller endpoint is invalid",
        ));
    };
    struct SharedUnix(Arc<UnixStream>);
    impl Read for SharedUnix {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            (&*self.0).read(buffer)
        }
    }
    impl Write for SharedUnix {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            (&*self.0).write(buffer)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            (&*self.0).flush()
        }
    }
    struct UnixCancel {
        stream: Arc<UnixStream>,
        cancelled: AtomicBool,
    }
    impl CancelHandle for UnixCancel {
        fn cancel(&self) {
            self.cancelled.store(true, Ordering::Release);
            let _ = self.stream.shutdown(Shutdown::Both);
        }
        fn is_cancelled(&self) -> bool {
            self.cancelled.load(Ordering::Acquire)
        }
    }
    let stream = Arc::new(
        UnixStream::connect(path)
            .map_err(|_| DistributedError::controller("controller is not reachable"))?,
    );
    Ok(Connected {
        io: Box::new(SharedUnix(Arc::clone(&stream))),
        cancel: Arc::new(UnixCancel {
            stream,
            cancelled: AtomicBool::new(false),
        }),
    })
}

#[cfg(not(any(windows, target_os = "linux")))]
fn connect_local(_: &LocalEndpoint) -> Result<Connected, DistributedError> {
    Err(DistributedError::controller(
        "controller platform is unsupported",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        io::{self, Cursor, Read, Write},
        sync::{Arc, Condvar, Mutex},
        time::{Duration, Instant},
    };

    use pinvou_protocol::{HelloServer, IpcMessage, RuntimeEventEnvelope, encode_frame};
    use pinvou_tui::backend::{Backend, BackendErrorKind};
    use serde_json::json;

    #[derive(Clone, Default)]
    struct FakeConnector {
        plans: Arc<Mutex<VecDeque<FakePlan>>>,
        requests: Arc<Mutex<Vec<Vec<IpcMessage>>>>,
    }

    enum FakePlan {
        Frames(Vec<IpcMessage>),
        Blocked(Arc<(Mutex<bool>, Condvar)>),
    }

    impl FakeConnector {
        fn with(plans: impl IntoIterator<Item = FakePlan>) -> Self {
            Self {
                plans: Arc::new(Mutex::new(plans.into_iter().collect())),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn methods(&self) -> Vec<String> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .flat_map(|requests| requests.iter())
                .filter_map(|request| request.method().map(str::to_owned))
                .collect()
        }
    }

    struct FakeIo {
        input: FakeInput,
        output: Arc<Mutex<Vec<u8>>>,
    }

    enum FakeInput {
        Frames(Cursor<Vec<u8>>),
        Blocked(Arc<(Mutex<bool>, Condvar)>),
    }

    impl Read for FakeIo {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            match &mut self.input {
                FakeInput::Frames(input) => input.read(buffer),
                FakeInput::Blocked(state) => {
                    let (cancelled, wake) = &**state;
                    let mut cancelled = cancelled.lock().unwrap();
                    while !*cancelled {
                        cancelled = wake.wait(cancelled).unwrap();
                    }
                    Err(io::Error::new(io::ErrorKind::BrokenPipe, "detached"))
                }
            }
        }
    }

    impl Write for FakeIo {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.output.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct FakeCancel(Option<Arc<(Mutex<bool>, Condvar)>>);

    impl CancelHandle for FakeCancel {
        fn cancel(&self) {
            if let Some(state) = &self.0 {
                *state.0.lock().unwrap() = true;
                state.1.notify_all();
            }
        }
        fn is_cancelled(&self) -> bool {
            self.0
                .as_ref()
                .is_some_and(|state| *state.0.lock().unwrap())
        }
    }

    impl ConnectionFactory for FakeConnector {
        fn connect(&self) -> Result<Connected, DistributedError> {
            let plan = self.plans.lock().unwrap().pop_front().unwrap();
            let output = Arc::new(Mutex::new(Vec::new()));
            self.requests.lock().unwrap().push(Vec::new());
            let input = match &plan {
                FakePlan::Frames(messages) => {
                    let hello = HelloServer::new("controller-instance").unwrap();
                    let mut bytes = encode_frame(&hello).unwrap();
                    bytes.extend(
                        messages
                            .iter()
                            .flat_map(|message| encode_frame(message).unwrap()),
                    );
                    FakeInput::Frames(Cursor::new(bytes))
                }
                FakePlan::Blocked(state) => FakeInput::Blocked(Arc::clone(state)),
            };
            Ok(Connected {
                io: Box::new(RecordingIo {
                    inner: FakeIo { input, output },
                    sink: Arc::clone(&self.requests),
                }),
                cancel: Arc::new(FakeCancel(match plan {
                    FakePlan::Frames(_) => None,
                    FakePlan::Blocked(state) => Some(state),
                })),
            })
        }
    }

    struct RecordingIo {
        inner: FakeIo,
        sink: Arc<Mutex<Vec<Vec<IpcMessage>>>>,
    }

    impl Read for RecordingIo {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.inner.read(buffer)
        }
    }

    impl Write for RecordingIo {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            let written = self.inner.write(buffer)?;
            let bytes = self.inner.output.lock().unwrap().clone();
            let mut cursor = bytes.as_slice();
            let mut decoded = Vec::new();
            // The first frame is HelloClient, which is deliberately not an IpcMessage.
            if cursor.len() >= 4 {
                let hello_len = u32::from_le_bytes(cursor[..4].try_into().unwrap()) as usize + 4;
                if cursor.len() >= hello_len {
                    cursor = &cursor[hello_len..];
                }
            }
            while cursor.len() >= 4 {
                let len = u32::from_le_bytes(cursor[..4].try_into().unwrap()) as usize + 4;
                if cursor.len() < len {
                    break;
                }
                decoded.push(pinvou_protocol::decode_frame(&cursor[..len]).unwrap());
                cursor = &cursor[len..];
            }
            *self.sink.lock().unwrap().last_mut().unwrap() = decoded;
            Ok(written)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn response(id: u64, payload: serde_json::Value) -> IpcMessage {
        IpcMessage::response(json!(id), payload).unwrap()
    }

    fn event(seq: u64, kind: &str, payload: serde_json::Value) -> IpcMessage {
        event_for_turn(seq, "turn-a", kind, payload)
    }

    fn event_for_turn(
        seq: u64,
        turn_id: &str,
        kind: &str,
        payload: serde_json::Value,
    ) -> IpcMessage {
        let (stream_id, rate_class) = match kind {
            "turn.started" | "approval.requested" | "turn.ended" => ("control", "R0"),
            _ => ("main", "R1"),
        };
        let event = RuntimeEventEnvelope::from_value(json!({
            "protocol_version": pinvou_protocol::IPC_VERSION,
            "schema_version": 1,
            "node_id": "node-a",
            "logical_session_id": "session-a",
            "attachment_id": "attachment-a",
            "work_id": null,
            "collaborative_run_id": null,
            "stream_id": stream_id,
            "turn_id": turn_id,
            "seq": seq,
            "source_span": null,
            "timestamp": "2026-08-24T00:00:00Z",
            "rate_class": rate_class,
            "kind": kind,
            "payload": payload
        }))
        .unwrap();
        IpcMessage::event("runtime.event", serde_json::to_value(event).unwrap()).unwrap()
    }

    fn backend(connector: FakeConnector) -> ControllerTuiBackend {
        ControllerTuiBackend::with_connector(std::env::current_dir().unwrap(), Arc::new(connector))
            .unwrap()
    }

    #[test]
    fn stream_forwards_every_event_through_terminal_event_in_order() {
        let connector = FakeConnector::with([FakePlan::Frames(vec![
            event(1, "turn.started", json!({"user_input_ref":"prompt"})),
            event(
                2,
                "text.delta",
                json!({"role":"assistant","content":"a","merged_count":1}),
            ),
            event(
                3,
                "approval.requested",
                json!({"approval_id":"a","tool":"shell","summary":"ok","options":["allow","deny"]}),
            ),
            event(
                4,
                "tool.call.started",
                json!({"tool_id":"c","name":"shell","args_json":{}}),
            ),
            event(
                5,
                "tool.call.completed",
                json!({"tool_id":"c","result":{},"is_error":false,"exit_code":0}),
            ),
            event(
                6,
                "turn.ended",
                json!({"end_reason":"completed","error":null}),
            ),
        ])]);
        let subject = backend(connector.clone());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&seen);

        subject
            .stream_turn(
                7,
                "hello".into(),
                Box::new(move |event| {
                    output.lock().unwrap().push(event.kind().to_owned());
                    Ok(())
                }),
            )
            .unwrap();

        assert_eq!(
            *seen.lock().unwrap(),
            [
                "turn.started",
                "text.delta",
                "approval.requested",
                "tool.call.started",
                "tool.call.completed",
                "turn.ended"
            ]
        );
        assert_eq!(connector.methods(), ["chat.start"]);
    }

    #[test]
    fn stream_rejects_a_terminal_event_for_another_turn() {
        let subject = backend(FakeConnector::with([FakePlan::Frames(vec![
            event(1, "turn.started", json!({"user_input_ref":"prompt"})),
            event_for_turn(
                2,
                "turn-b",
                "turn.ended",
                json!({"end_reason":"completed","error":null}),
            ),
        ])]));
        let error = subject
            .stream_turn(8, "hello".into(), Box::new(|_| Ok(())))
            .unwrap_err();
        assert_eq!(error.kind(), BackendErrorKind::Protocol);
    }

    #[test]
    fn every_control_uses_an_independent_authenticated_connection() {
        let connector = FakeConnector::with([
            FakePlan::Frames(vec![response(1, json!({"status":"ok"}))]),
            FakePlan::Frames(vec![response(1, json!({"status":"ok"}))]),
            FakePlan::Frames(vec![response(1, json!({"status":"ok"}))]),
        ]);
        let subject = backend(connector.clone());
        subject.resolve_approval("approval-a".into(), true).unwrap();
        subject
            .resolve_input("input-a".into(), "answer".into())
            .unwrap();
        subject.interrupt("turn-a".into()).unwrap();
        assert_eq!(
            connector.methods(),
            ["approval.resolve", "input.resolve", "turn.interrupt"]
        );
        assert_eq!(connector.requests.lock().unwrap().len(), 3);
    }

    #[test]
    fn runtime_switch_is_detect_prepare_commit_detect_without_deprecated_method() {
        let connector = FakeConnector::with([
            FakePlan::Frames(vec![response(
                1,
                json!({"runtime":"codex","status":"available","capabilities":{"interactive_chat":true}}),
            )]),
            FakePlan::Frames(vec![response(
                1,
                json!({
                    "runtime":"codex",
                    "status":"ready",
                    "switch_token":"secret-token",
                    "requires_compression":false,
                    "context":{"strategy":"none","reason":"turn_boundary_clean","portable_checkpoint":false},
                    "tools":{"policy":"portable_or_replay_only","active_tool_calls":0,"blocking_missing_tools":[]}
                }),
            )]),
            FakePlan::Frames(vec![response(
                1,
                json!({"runtime":"codex","status":"ok","switch_token":"secret-token"}),
            )]),
            FakePlan::Frames(vec![response(
                1,
                json!({"runtime":"codex","status":"available","capabilities":{"interactive_chat":true}}),
            )]),
        ]);
        let subject = backend(connector.clone());
        let status = subject.switch_runtime("codex".into()).unwrap();
        assert_eq!(status.id, "codex");
        assert!(status.available);
        assert_eq!(
            connector.methods(),
            [
                "runtime.detect",
                "runtime.switch.prepare",
                "runtime.switch.commit",
                "runtime.detect"
            ]
        );
        assert!(
            !connector
                .methods()
                .iter()
                .any(|method| method == "runtime.switch")
        );
    }

    #[test]
    fn detach_stream_wakes_a_blocked_read_quickly_and_is_idempotent() {
        let blocked = Arc::new((Mutex::new(false), Condvar::new()));
        let subject = Arc::new(backend(FakeConnector::with([FakePlan::Blocked(
            Arc::clone(&blocked),
        )])));
        let worker = {
            let subject = Arc::clone(&subject);
            std::thread::spawn(move || {
                subject.stream_turn(91, "hello".into(), Box::new(|_| Ok(())))
            })
        };
        while subject.in_flight.streams.lock().unwrap().is_empty() {
            std::thread::yield_now();
        }
        let started = Instant::now();
        subject.detach_stream(91).unwrap();
        subject.detach_stream(91).unwrap();
        assert!(started.elapsed() < Duration::from_millis(500));
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind(),
            BackendErrorKind::Cancelled
        );
    }

    #[test]
    fn detach_controls_wakes_blocked_control_and_does_not_cancel_future_connections() {
        let blocked = Arc::new((Mutex::new(false), Condvar::new()));
        let connector = FakeConnector::with([
            FakePlan::Blocked(Arc::clone(&blocked)),
            FakePlan::Frames(vec![response(1, json!({"status":"ok"}))]),
        ]);
        let subject = Arc::new(backend(connector));
        let worker = {
            let subject = Arc::clone(&subject);
            std::thread::spawn(move || subject.resolve_input("input-a".into(), "answer".into()))
        };
        while subject.in_flight.controls.lock().unwrap().is_empty() {
            std::thread::yield_now();
        }
        subject.detach_controls().unwrap();
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind(),
            BackendErrorKind::Cancelled
        );
        subject.interrupt("turn-b".into()).unwrap();
    }

    #[test]
    fn runtime_list_maps_controller_payload_without_hard_coding_runtime_ids() {
        let subject = backend(FakeConnector::with([
            FakePlan::Frames(vec![response(
                1,
                json!({
                    "current":"custom-agent",
                    "runtimes":[{"id":"custom-agent","label":"Custom Agent","available":true}]
                }),
            )]),
            FakePlan::Frames(vec![response(
                1,
                json!({
                    "runtime":"custom-agent",
                    "status":"available",
                    "capabilities":{"interactive_chat":true,"tool_approval":false}
                }),
            )]),
        ]));
        let list = subject.runtime_list().unwrap();
        assert_eq!(list.active_runtime.as_deref(), Some("custom-agent"));
        assert_eq!(list.runtimes[0].id, "custom-agent");
        assert_eq!(list.runtimes[0].display_name, "Custom Agent");
        assert_eq!(
            list.runtimes[0].capability_summary.as_deref(),
            Some("interactive_chat")
        );
    }

    #[derive(Clone, Copy)]
    enum BlockedControl {
        Approval,
        Input,
        Interrupt,
        RuntimeList,
        RuntimeSwitch,
    }

    #[test]
    fn detach_controls_wakes_every_kind_of_blocked_short_request() {
        for kind in [
            BlockedControl::Approval,
            BlockedControl::Input,
            BlockedControl::Interrupt,
            BlockedControl::RuntimeList,
            BlockedControl::RuntimeSwitch,
        ] {
            let blocked = Arc::new((Mutex::new(false), Condvar::new()));
            let subject = Arc::new(backend(FakeConnector::with([FakePlan::Blocked(blocked)])));
            let worker = {
                let subject = Arc::clone(&subject);
                std::thread::spawn(move || match kind {
                    BlockedControl::Approval => subject.resolve_approval("a".into(), true),
                    BlockedControl::Input => subject.resolve_input("i".into(), "v".into()),
                    BlockedControl::Interrupt => subject.interrupt("t".into()),
                    BlockedControl::RuntimeList => subject.runtime_list().map(|_| ()),
                    BlockedControl::RuntimeSwitch => {
                        subject.switch_runtime("codex".into()).map(|_| ())
                    }
                })
            };
            while subject.in_flight.controls.lock().unwrap().is_empty() {
                std::thread::yield_now();
            }
            subject.detach_controls().unwrap();
            assert_eq!(
                worker.join().unwrap().unwrap_err().kind(),
                BackendErrorKind::Cancelled
            );
        }
    }

    #[test]
    fn emitter_failure_cleans_up_the_stream_registration_immediately() {
        let subject = backend(FakeConnector::with([FakePlan::Frames(vec![event(
            1,
            "turn.started",
            json!({"user_input_ref":"prompt"}),
        )])]));
        let error = subject
            .stream_turn(
                12,
                "hello".into(),
                Box::new(|_| {
                    Err(BackendError::new(
                        BackendErrorKind::Operation,
                        "receiver closed",
                    ))
                }),
            )
            .unwrap_err();
        assert_eq!(error.kind(), BackendErrorKind::Operation);
        assert!(subject.in_flight.streams.lock().unwrap().is_empty());
    }

    #[test]
    fn error_mapping_preserves_exit_category_and_redacts_secret_bearing_messages() {
        let auth = map_distributed(DistributedError::new(
            StableExitCode::BlockedAuth,
            "sign in required",
        ));
        assert_eq!(auth.kind(), BackendErrorKind::AuthBlocked);
        assert_eq!(auth.exit_code(), Some(StableExitCode::BlockedAuth));
        assert_eq!(auth.safe_message(), "sign in required");

        let secret = map_distributed(DistributedError::new(
            StableExitCode::RuntimeFailed,
            "invalid bearer secret-token",
        ));
        assert_eq!(secret.kind(), BackendErrorKind::Operation);
        assert_eq!(secret.safe_message(), "controller request failed");
    }

    #[test]
    fn unavailable_switch_target_preserves_controller_status_category() {
        let subject = backend(FakeConnector::with([FakePlan::Frames(vec![response(
            1,
            json!({
                "runtime":"codex",
                "status":"blocked_auth",
                "error_kind":"blocked_auth",
                "exit_code":4,
                "message":"sign in required"
            }),
        )])]));
        let error = subject.switch_runtime("codex".into()).unwrap_err();
        assert_eq!(error.kind(), BackendErrorKind::AuthBlocked);
        assert_eq!(error.exit_code(), Some(StableExitCode::BlockedAuth));
        assert_eq!(error.safe_message(), "sign in required");
    }

    #[test]
    fn runtime_switch_rejects_prepare_without_context_and_tool_handoff_semantics() {
        let connector = FakeConnector::with([
            FakePlan::Frames(vec![response(
                1,
                json!({"runtime":"codex","status":"available"}),
            )]),
            FakePlan::Frames(vec![response(
                1,
                json!({
                    "runtime":"codex",
                    "status":"ready",
                    "switch_token":"secret-token",
                    "requires_compression":false,
                    "context":{"strategy":"none"},
                    "tools":{"active_tool_calls":0}
                }),
            )]),
        ]);
        let subject = backend(connector.clone());
        let error = subject.switch_runtime("codex".into()).unwrap_err();
        assert_eq!(error.kind(), BackendErrorKind::Protocol);
        assert_eq!(
            connector.methods(),
            ["runtime.detect", "runtime.switch.prepare"]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_named_pipe_cancel_wakes_a_real_blocked_controller_read() {
        use std::{ffi::OsStr, os::windows::ffi::OsStrExt, sync::mpsc};
        use windows_sys::Win32::{
            Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE},
            Storage::FileSystem::PIPE_ACCESS_DUPLEX,
            System::Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
            },
        };

        let name = format!(
            r"\\.\pipe\pinvou-tui-cancel-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let wide = OsStr::new(&name)
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let handle = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                4096,
                4096,
                0,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(handle, INVALID_HANDLE_VALUE);
        let handle = handle as usize;
        let (connected_tx, connected_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let handle = handle as windows_sys::Win32::Foundation::HANDLE;
            let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
            assert!(
                connected != 0
                    || std::io::Error::last_os_error().raw_os_error()
                        == Some(ERROR_PIPE_CONNECTED as i32)
            );
            connected_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_secs(2));
            unsafe { CloseHandle(handle) };
        });
        let subject = Arc::new(
            ControllerTuiBackend::with_connector(
                std::env::current_dir().unwrap(),
                Arc::new(LocalConnectionFactory {
                    endpoint: LocalEndpoint::WindowsPipe(name),
                }),
            )
            .unwrap(),
        );
        let worker = {
            let subject = Arc::clone(&subject);
            std::thread::spawn(move || subject.runtime_list())
        };
        connected_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        while subject.in_flight.controls.lock().unwrap().is_empty() {
            std::thread::yield_now();
        }
        std::thread::sleep(Duration::from_millis(50));
        let started = Instant::now();
        subject.detach_controls().unwrap();
        let result = worker.join().unwrap();
        assert!(started.elapsed() < Duration::from_millis(500));
        assert_eq!(result.unwrap_err().kind(), BackendErrorKind::Cancelled);
        server.join().unwrap();
    }
}
