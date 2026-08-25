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
    Backend, BackendError, BackendErrorKind, EventEmitter, ModelCandidate, ModelList,
    PermissionControlStrength, PermissionMode, PermissionStatus, ResumeResult, RuntimeList,
    RuntimeStatus, SessionCandidate, SessionList,
};
use serde_json::{Value, json};

use super::{ControllerWire, DistributedError, ensure_controller};

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

trait ControllerBootstrap: Send + Sync {
    fn ensure(&self) -> Result<(), DistributedError>;
}

struct ProductionBootstrap;

impl ControllerBootstrap for ProductionBootstrap {
    fn ensure(&self) -> Result<(), DistributedError> {
        ensure_controller().map(|_| ())
    }
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
    streams: Mutex<HashMap<u64, LeaseEntry>>,
    controls: Mutex<HashMap<u64, LeaseEntry>>,
}

struct LeaseEntry {
    connection_id: u64,
    cancelled: bool,
    cancel: Option<Arc<dyn CancelHandle>>,
}

#[derive(Clone, Copy)]
enum RegistrationKind {
    Stream,
    Control,
}

struct RegistrationGuard {
    in_flight: Arc<InFlight>,
    kind: RegistrationKind,
    operation_token: u64,
    connection_id: u64,
}

struct ControlOperationGuard {
    in_flight: Arc<InFlight>,
    operation_token: u64,
    connection_id: u64,
}

impl Drop for ControlOperationGuard {
    fn drop(&mut self) {
        if let Ok(mut controls) = self.in_flight.controls.lock()
            && controls
                .get(&self.operation_token)
                .is_some_and(|entry| entry.connection_id == self.connection_id)
        {
            controls.remove(&self.operation_token);
        }
    }
}

impl Drop for RegistrationGuard {
    fn drop(&mut self) {
        match self.kind {
            RegistrationKind::Stream => {
                if let Ok(mut streams) = self.in_flight.streams.lock()
                    && streams
                        .get(&self.operation_token)
                        .is_some_and(|entry| entry.connection_id == self.connection_id)
                {
                    streams.remove(&self.operation_token);
                }
            }
            RegistrationKind::Control => {
                if let Ok(mut controls) = self.in_flight.controls.lock()
                    && let Some(entry) = controls.get_mut(&self.operation_token)
                    && entry.connection_id == self.connection_id
                {
                    entry.cancel = None;
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
            BackendError::new(
                BackendErrorKind::ControllerUnavailable,
                "Controller paths are unavailable",
            )
            .with_exit_code(error.exit_code())
        })?;
        Self::with_connector_and_bootstrap(
            workspace.as_ref().to_path_buf(),
            Arc::new(LocalConnectionFactory {
                endpoint: paths.endpoint().clone(),
            }),
            Arc::new(ProductionBootstrap),
        )
    }

    fn with_connector_and_bootstrap(
        workspace: PathBuf,
        connector: Arc<dyn ConnectionFactory>,
        bootstrap: Arc<dyn ControllerBootstrap>,
    ) -> Result<Self, BackendError> {
        bootstrap.ensure().map_err(map_distributed)?;
        Self::with_connector(workspace, connector)
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
        let connection_id = self.claim_stream(operation_token)?;
        let guard = RegistrationGuard {
            in_flight: Arc::clone(&self.in_flight),
            kind: RegistrationKind::Stream,
            operation_token,
            connection_id,
        };
        self.ensure_lease_active(operation_token, connection_id, RegistrationKind::Stream)?;
        let connected = self.connector.connect().map_err(map_distributed)?;
        let cancel = Arc::clone(&connected.cancel);
        self.attach_cancel(
            operation_token,
            connection_id,
            &cancel,
            RegistrationKind::Stream,
        )?;
        let wire = authenticate(connected.io).map_err(|error| map_with_cancel(error, &*cancel))?;
        Ok((wire, cancel, guard))
    }

    fn open_control(
        &self,
        operation_token: u64,
        connection_id: u64,
    ) -> Result<OpenedConnection, BackendError> {
        let guard = RegistrationGuard {
            in_flight: Arc::clone(&self.in_flight),
            kind: RegistrationKind::Control,
            operation_token,
            connection_id,
        };
        self.ensure_lease_active(operation_token, connection_id, RegistrationKind::Control)?;
        let connected = self.connector.connect().map_err(map_distributed)?;
        let cancel = Arc::clone(&connected.cancel);
        self.attach_cancel(
            operation_token,
            connection_id,
            &cancel,
            RegistrationKind::Control,
        )?;
        let wire = authenticate(connected.io).map_err(|error| map_with_cancel(error, &*cancel))?;
        Ok((wire, cancel, guard))
    }

    fn control<T>(
        &self,
        operation_token: u64,
        connection_id: u64,
        operation: impl FnOnce(&mut TuiWire) -> Result<T, DistributedError>,
    ) -> Result<T, BackendError> {
        let (mut wire, cancel, _guard) = self.open_control(operation_token, connection_id)?;
        operation(&mut wire).map_err(|error| map_with_cancel(error, &*cancel))
    }

    fn detect(
        &self,
        operation_token: u64,
        connection_id: u64,
        runtime: &str,
    ) -> Result<Value, BackendError> {
        let response = self.control(operation_token, connection_id, |wire| {
            wire.runtime_detect(Some(runtime))
        })?;
        require_response(response, "runtime.detect")
    }

    fn new_connection_id(&self) -> u64 {
        self.in_flight
            .next_connection_id
            .fetch_add(1, Ordering::Relaxed)
    }

    fn begin_lease(&self, operation_token: u64, stream: bool) -> Result<(), BackendError> {
        let connection_id = self.new_connection_id();
        let mut leases = if stream {
            self.in_flight.streams.lock().map_err(lock_error)?
        } else {
            self.in_flight.controls.lock().map_err(lock_error)?
        };
        if leases.contains_key(&operation_token) {
            return Err(BackendError::new(
                BackendErrorKind::Operation,
                "operation token is already active",
            ));
        }
        leases.insert(
            operation_token,
            LeaseEntry {
                connection_id,
                cancelled: false,
                cancel: None,
            },
        );
        Ok(())
    }

    fn claim_stream(&self, operation_token: u64) -> Result<u64, BackendError> {
        claim_lease(&self.in_flight, &self.in_flight.streams, operation_token)
    }

    fn claim_control(&self, operation_token: u64) -> Result<u64, BackendError> {
        claim_lease(&self.in_flight, &self.in_flight.controls, operation_token)
    }

    fn control_operation(
        &self,
        operation_token: u64,
    ) -> Result<ControlOperationGuard, BackendError> {
        let connection_id = self.claim_control(operation_token)?;
        Ok(ControlOperationGuard {
            in_flight: Arc::clone(&self.in_flight),
            operation_token,
            connection_id,
        })
    }

    fn attach_cancel(
        &self,
        operation_token: u64,
        connection_id: u64,
        cancel: &Arc<dyn CancelHandle>,
        kind: RegistrationKind,
    ) -> Result<(), BackendError> {
        let mut leases = match kind {
            RegistrationKind::Stream => self.in_flight.streams.lock().map_err(lock_error)?,
            RegistrationKind::Control => self.in_flight.controls.lock().map_err(lock_error)?,
        };
        let entry = leases
            .get_mut(&operation_token)
            .filter(|entry| entry.connection_id == connection_id)
            .ok_or_else(cancelled_error)?;
        if entry.cancelled {
            cancel.cancel();
            return Err(cancelled_error());
        }
        entry.cancel = Some(Arc::clone(cancel));
        Ok(())
    }

    fn ensure_lease_active(
        &self,
        operation_token: u64,
        connection_id: u64,
        kind: RegistrationKind,
    ) -> Result<(), BackendError> {
        let leases = match kind {
            RegistrationKind::Stream => self.in_flight.streams.lock().map_err(lock_error)?,
            RegistrationKind::Control => self.in_flight.controls.lock().map_err(lock_error)?,
        };
        match leases.get(&operation_token) {
            Some(entry) if entry.connection_id == connection_id && !entry.cancelled => Ok(()),
            _ => Err(cancelled_error()),
        }
    }
}

fn claim_lease(
    in_flight: &InFlight,
    leases: &Mutex<HashMap<u64, LeaseEntry>>,
    operation_token: u64,
) -> Result<u64, BackendError> {
    let mut leases = leases.lock().map_err(lock_error)?;
    let connection_id = match leases.get(&operation_token) {
        Some(entry) if entry.cancelled => {
            leases.remove(&operation_token);
            return Err(cancelled_error());
        }
        Some(entry) => entry.connection_id,
        None => {
            let connection_id = in_flight.next_connection_id.fetch_add(1, Ordering::Relaxed);
            leases.insert(
                operation_token,
                LeaseEntry {
                    connection_id,
                    cancelled: false,
                    cancel: None,
                },
            );
            connection_id
        }
    };
    Ok(connection_id)
}

fn cancelled_error() -> BackendError {
    BackendError::new(
        BackendErrorKind::Cancelled,
        "local Controller operation was detached",
    )
    .with_exit_code(StableExitCode::Cancelled)
}

fn is_attachment_scoped(kind: RuntimeEventKind) -> bool {
    matches!(
        kind,
        RuntimeEventKind::AttachmentStarted | RuntimeEventKind::AttachmentEnded
    )
}

impl Backend for ControllerTuiBackend {
    fn workspace(&self) -> Result<PathBuf, BackendError> {
        Ok(self.workspace.clone())
    }

    fn begin_stream(&self, operation_token: u64) -> Result<(), BackendError> {
        self.begin_lease(operation_token, true)
    }

    fn begin_control(&self, operation_token: u64) -> Result<(), BackendError> {
        self.begin_lease(operation_token, false)
    }

    fn runtime_list(&self, operation_token: u64) -> Result<RuntimeList, BackendError> {
        let operation = self.control_operation(operation_token)?;
        let response = self.control(
            operation_token,
            operation.connection_id,
            ControllerWire::runtime_list,
        )?;
        let payload = require_response(response, "runtime.list")?;
        map_runtime_list(&payload)
    }

    fn session_list(
        &self,
        operation_token: u64,
        query: Option<String>,
    ) -> Result<SessionList, BackendError> {
        let operation = self.control_operation(operation_token)?;
        let response = self.control(operation_token, operation.connection_id, |wire| {
            wire.session_list(query.as_deref())
        })?;
        map_session_list(&require_response(response, "session.list")?)
    }

    fn resume_session(
        &self,
        operation_token: u64,
        session_id: String,
    ) -> Result<ResumeResult, BackendError> {
        let operation = self.control_operation(operation_token)?;
        let connection_id = operation.connection_id;
        let prepared = self
            .control(operation_token, connection_id, |wire| {
                wire.session_resume_prepare(&session_id)
            })
            .and_then(|message| require_response(message, "session.resume.prepare"))?;
        if prepared.get("status").and_then(Value::as_str) != Some("ready")
            || prepared.get("session_id").and_then(Value::as_str) != Some(session_id.as_str())
        {
            return Err(protocol_error("session resume prepare response is invalid"));
        }
        let token = required_token(&prepared, "resume_token")?;
        let committed = self
            .control(operation_token, connection_id, |wire| {
                wire.session_resume_commit(&token)
            })
            .and_then(|message| require_response(message, "session.resume.commit"))?;
        if committed.get("status").and_then(Value::as_str) != Some("ok")
            || committed.get("session_id").and_then(Value::as_str) != Some(session_id.as_str())
        {
            return Err(protocol_error(
                "session resume commit response does not match prepare",
            ));
        }
        map_resume_result(&committed)
    }

    fn model_list(&self, operation_token: u64) -> Result<ModelList, BackendError> {
        let operation = self.control_operation(operation_token)?;
        let response = self.control(operation_token, operation.connection_id, |wire| {
            wire.model_list()
        })?;
        map_model_list(&require_response(response, "model.list")?)
    }

    fn switch_model(&self, operation_token: u64, model_id: String) -> Result<(), BackendError> {
        let operation = self.control_operation(operation_token)?;
        let connection_id = operation.connection_id;
        let prepared = self
            .control(operation_token, connection_id, |wire| {
                wire.model_switch_prepare(&model_id)
            })
            .and_then(|message| require_response(message, "model.switch.prepare"))?;
        if prepared.get("status").and_then(Value::as_str) != Some("ready")
            || prepared.get("model_id").and_then(Value::as_str) != Some(model_id.as_str())
        {
            return Err(protocol_error("model switch prepare response is invalid"));
        }
        let token = required_token(&prepared, "switch_token")?;
        let committed = self
            .control(operation_token, connection_id, |wire| {
                wire.model_switch_commit(&token)
            })
            .and_then(|message| require_response(message, "model.switch.commit"))?;
        if committed.get("status").and_then(Value::as_str) != Some("ok")
            || committed.get("model_id").and_then(Value::as_str) != Some(model_id.as_str())
        {
            return Err(protocol_error(
                "model switch commit response does not match prepare",
            ));
        }
        Ok(())
    }

    fn permissions(&self, operation_token: u64) -> Result<PermissionStatus, BackendError> {
        let operation = self.control_operation(operation_token)?;
        let response = self.control(operation_token, operation.connection_id, |wire| {
            wire.permissions_inspect()
        })?;
        map_permission_status(&require_response(response, "permissions.inspect")?)
    }

    fn switch_permissions(
        &self,
        operation_token: u64,
        profile: PermissionMode,
        full_access_confirmed: bool,
    ) -> Result<(), BackendError> {
        let operation = self.control_operation(operation_token)?;
        let connection_id = operation.connection_id;
        let prepared = self
            .control(operation_token, connection_id, |wire| {
                wire.permissions_switch_prepare(profile.as_str(), full_access_confirmed)
            })
            .and_then(|message| require_response(message, "permissions.switch.prepare"))?;
        if prepared.get("status").and_then(Value::as_str) != Some("ready")
            || prepared.get("profile").and_then(Value::as_str) != Some(profile.as_str())
        {
            return Err(protocol_error(
                "permission switch prepare response is invalid",
            ));
        }
        let token = required_token(&prepared, "switch_token")?;
        let committed = self
            .control(operation_token, connection_id, |wire| {
                wire.permissions_switch_commit(&token)
            })
            .and_then(|message| require_response(message, "permissions.switch.commit"))?;
        if committed.get("status").and_then(Value::as_str) != Some("ok")
            || committed.get("profile").and_then(Value::as_str) != Some(profile.as_str())
        {
            return Err(protocol_error(
                "permission switch commit response does not match prepare",
            ));
        }
        Ok(())
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
            match (active_turn.as_deref(), event.turn_id()) {
                (None, Some(turn_id)) if event.event_kind() == RuntimeEventKind::TurnStarted => {
                    active_turn = Some(turn_id.to_owned());
                }
                (None, None) if is_attachment_scoped(event.event_kind()) => {}
                (Some(expected), Some(actual)) if expected == actual => {
                    if event.event_kind() == RuntimeEventKind::TurnStarted {
                        return Err(protocol_error(
                            "controller started another turn on the same stream",
                        ));
                    }
                }
                (Some(_), None) if is_attachment_scoped(event.event_kind()) => {}
                (None, _) => {
                    return Err(protocol_error(
                        "controller returned a turn event before turn.started",
                    ));
                }
                (Some(_), _) => {
                    return Err(protocol_error(
                        "controller returned an event for another turn",
                    ));
                }
            }
            let terminal = event.event_kind() == RuntimeEventKind::TurnEnded;
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
        let mut streams = self.in_flight.streams.lock().map_err(lock_error)?;
        let entry = streams
            .entry(operation_token)
            .or_insert_with(|| LeaseEntry {
                connection_id: self.new_connection_id(),
                cancelled: true,
                cancel: None,
            });
        entry.cancelled = true;
        let cancel = entry.cancel.clone();
        drop(streams);
        if let Some(cancel) = cancel {
            cancel.cancel();
        }
        Ok(())
    }

    fn detach_controls(&self) -> Result<(), BackendError> {
        let mut controls = self.in_flight.controls.lock().map_err(lock_error)?;
        let cancellations = controls
            .values_mut()
            .filter_map(|entry| {
                entry.cancelled = true;
                entry.cancel.clone()
            })
            .collect::<Vec<_>>();
        drop(controls);
        for cancel in cancellations {
            cancel.cancel();
        }
        Ok(())
    }

    fn resolve_approval(
        &self,
        operation_token: u64,
        approval_id: String,
        accepted: bool,
    ) -> Result<(), BackendError> {
        let operation = self.control_operation(operation_token)?;
        self.control(operation_token, operation.connection_id, |wire| {
            wire.resolve_approval(&approval_id, accepted)
        })
        .and_then(|message| require_response(message, "approval.resolve"))?;
        Ok(())
    }

    fn resolve_input(
        &self,
        operation_token: u64,
        input_id: String,
        value: String,
    ) -> Result<(), BackendError> {
        let operation = self.control_operation(operation_token)?;
        self.control(operation_token, operation.connection_id, |wire| {
            wire.resolve_input(&input_id, &value)
        })
        .and_then(|message| require_response(message, "input.resolve"))?;
        Ok(())
    }

    fn interrupt(&self, operation_token: u64, turn_id: String) -> Result<(), BackendError> {
        let operation = self.control_operation(operation_token)?;
        self.control(operation_token, operation.connection_id, |wire| {
            wire.interrupt_turn(&turn_id)
        })
        .and_then(|message| require_response(message, "turn.interrupt"))?;
        Ok(())
    }

    fn switch_runtime(
        &self,
        operation_token: u64,
        runtime: String,
    ) -> Result<RuntimeStatus, BackendError> {
        let operation = self.control_operation(operation_token)?;
        let connection_id = operation.connection_id;
        let initial = self.detect(operation_token, connection_id, &runtime)?;
        let initial_status = map_runtime_status(&initial, Some(&runtime))?;
        if !initial_status.available {
            return Err(map_status_error(&initial));
        }

        let prepared = self
            .control(operation_token, connection_id, |wire| {
                wire.runtime_switch_prepare(&runtime)
            })
            .and_then(|message| require_response(message, "runtime.switch.prepare"))?;
        validate_prepare(&prepared, &runtime)?;
        let token = prepared["switch_token"].as_str().unwrap().to_owned();
        let committed = self
            .control(operation_token, connection_id, |wire| {
                wire.runtime_switch_commit(&runtime, &token)
            })
            .and_then(|message| require_response(message, "runtime.switch.commit"))?;
        if committed.get("status").and_then(Value::as_str) != Some("ok")
            || committed.get("runtime").and_then(Value::as_str) != Some(runtime.as_str())
            || committed.get("switch_token").and_then(Value::as_str) != Some(token.as_str())
        {
            return Err(protocol_error(
                "runtime switch commit response does not match prepare",
            ));
        }
        let final_value = self.detect(operation_token, connection_id, &runtime)?;
        let final_status = map_runtime_status(&final_value, Some(&runtime))?;
        if final_status.id != runtime {
            return Err(protocol_error(
                "runtime switch verification returned another runtime",
            ));
        }
        if !final_status.available {
            return Err(map_status_error(&final_value));
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

fn map_session_list(payload: &Value) -> Result<SessionList, BackendError> {
    let sessions = payload
        .get("sessions")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol_error("session.list has no sessions array"))?
        .iter()
        .map(|session| {
            Ok(SessionCandidate {
                id: required_string(session, "id", "session has no id")?,
                title: required_string(session, "title", "session has no title")?,
                last_active_at: required_string(
                    session,
                    "last_active_at",
                    "session has no activity timestamp",
                )?,
                runtime_id: required_string(session, "runtime_id", "session has no runtime")?,
                model_id: optional_string(session, "model_id")?,
                status: required_string(session, "status", "session has no status")?,
            })
        })
        .collect::<Result<Vec<_>, BackendError>>()?;
    Ok(SessionList { sessions })
}

fn map_resume_result(payload: &Value) -> Result<ResumeResult, BackendError> {
    let snapshot = payload
        .get("snapshot")
        .ok_or_else(|| protocol_error("session resume has no snapshot"))?;
    let events = snapshot
        .get("normalized_events")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol_error("session snapshot has no normalized events"))?
        .iter()
        .cloned()
        .map(|event| {
            RuntimeEventEnvelope::from_value(event)
                .map_err(|_| protocol_error("session snapshot contains a malformed event"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ResumeResult {
        session_id: required_string(payload, "session_id", "session resume has no id")?,
        runtime_id: required_string(payload, "runtime", "session resume has no runtime")?,
        model_id: optional_string(payload, "model_id")?,
        permission_profile: parse_permission_mode(
            payload.get("approval_profile").and_then(Value::as_str),
        )?,
        attachment_epoch: payload
            .get("attachment_epoch")
            .and_then(Value::as_u64)
            .ok_or_else(|| protocol_error("session resume has no attachment epoch"))?,
        cursor: snapshot
            .get("cursor")
            .and_then(Value::as_u64)
            .ok_or_else(|| protocol_error("session snapshot has no cursor"))?,
        events,
    })
}

fn map_model_list(payload: &Value) -> Result<ModelList, BackendError> {
    let catalog = payload
        .get("catalog")
        .ok_or_else(|| protocol_error("model.list has no catalog"))?;
    let models = catalog
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol_error("model catalog has no models array"))?
        .iter()
        .map(|model| {
            Ok(ModelCandidate {
                id: required_string(model, "id", "model has no id")?,
                display_name: required_string(model, "display_name", "model has no display name")?,
                is_default: model
                    .get("is_default")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| protocol_error("model has no default flag"))?,
                available: model
                    .get("available")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| protocol_error("model has no availability flag"))?,
            })
        })
        .collect::<Result<Vec<_>, BackendError>>()?;
    Ok(ModelList {
        runtime_id: required_string(catalog, "runtime_id", "model catalog has no runtime")?,
        current_model: optional_string(catalog, "current_model")?,
        models,
    })
}

fn map_permission_status(payload: &Value) -> Result<PermissionStatus, BackendError> {
    let permissions = payload
        .get("permissions")
        .ok_or_else(|| protocol_error("permissions.inspect has no capability"))?;
    let supported_profiles = permissions
        .get("supported_profiles")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol_error("permission capability has no profiles"))?
        .iter()
        .map(|profile| parse_permission_mode(profile.as_str()))
        .collect::<Result<Vec<_>, _>>()?;
    let control_strength = match permissions.get("control_strength").and_then(Value::as_str) {
        Some("enforced") => PermissionControlStrength::Enforced,
        Some("partial") => PermissionControlStrength::Partial,
        Some("unsupported") => PermissionControlStrength::Unsupported,
        _ => return Err(protocol_error("permission control strength is invalid")),
    };
    Ok(PermissionStatus {
        current_profile: parse_permission_mode(
            payload.get("current_profile").and_then(Value::as_str),
        )?,
        supported_profiles,
        control_strength,
        native_mode: optional_string(permissions, "native_mode")?,
        sandbox: optional_string(permissions, "sandbox")?,
        residual_guards: permissions
            .get("residual_guards")
            .and_then(Value::as_array)
            .ok_or_else(|| protocol_error("permission capability has no residual guards"))?
            .iter()
            .map(|guard| {
                guard
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| protocol_error("permission residual guard is invalid"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        evidence_version: required_string(
            permissions,
            "evidence_version",
            "permission capability has no evidence version",
        )?,
    })
}

fn required_token(payload: &Value, field: &str) -> Result<String, BackendError> {
    required_string(payload, field, "controller prepare response has no token")
}

fn required_string(payload: &Value, field: &str, message: &str) -> Result<String, BackendError> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| protocol_error(message))
}

fn optional_string(payload: &Value, field: &str) -> Result<Option<String>, BackendError> {
    match payload.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.is_empty() => Ok(Some(value.clone())),
        _ => Err(protocol_error(format!("{field} is invalid"))),
    }
}

fn parse_permission_mode(value: Option<&str>) -> Result<PermissionMode, BackendError> {
    match value {
        Some("request") => Ok(PermissionMode::Request),
        Some("assisted") => Ok(PermissionMode::Assisted),
        Some("full_access") => Ok(PermissionMode::FullAccess),
        _ => Err(protocol_error("permission profile is invalid")),
    }
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
    let message = safe_remote_message(kind);
    BackendError::new(kind, message).with_exit_code(code)
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
    BackendError::new(kind, safe_remote_message(kind)).with_exit_code(code)
}

fn safe_remote_message(kind: BackendErrorKind) -> &'static str {
    match kind {
        BackendErrorKind::ControllerUnavailable => "Controller is unavailable",
        BackendErrorKind::AuthBlocked => "Runtime authentication is required",
        BackendErrorKind::Cancelled => "Operation was cancelled",
        BackendErrorKind::Protocol => "Controller protocol error",
        BackendErrorKind::Operation => "Runtime operation failed",
        BackendErrorKind::WorkerPanic | BackendErrorKind::Timeout => "Backend operation failed",
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
                std::thread::sleep(std::time::Duration::from_millis(1));
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

        fn wait_until_connection_count(&self, expected: usize) {
            let deadline = Instant::now() + Duration::from_secs(1);
            while self.requests.lock().unwrap().len() < expected {
                assert!(Instant::now() < deadline, "fake connection was not opened");
                std::thread::yield_now();
            }
        }

        fn wait_until_connected(&self) {
            self.wait_until_connection_count(1);
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
            "turn.started" | "approval.requested" | "input.requested" | "turn.ended" => {
                ("control", "R0")
            }
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
    fn stream_rejects_every_turn_scoped_event_for_another_turn_before_emit() {
        let cases = [
            (
                "text.delta",
                json!({"role":"assistant","content":"x","merged_count":1}),
            ),
            (
                "tool.call.started",
                json!({"tool_id":"t","name":"shell","args_json":{}}),
            ),
            (
                "approval.requested",
                json!({"approval_id":"a","tool":"shell","summary":"run","options":["allow","deny"]}),
            ),
            ("input.requested", json!({"input_id":"i","prompt":"value?"})),
        ];
        for (kind, payload) in cases {
            let subject = backend(FakeConnector::with([FakePlan::Frames(vec![
                event(1, "turn.started", json!({"user_input_ref":"prompt"})),
                event_for_turn(2, "turn-b", kind, payload),
            ])]));
            let emitted = Arc::new(Mutex::new(Vec::new()));
            let seen = Arc::clone(&emitted);
            let error = subject
                .stream_turn(
                    8,
                    "hello".into(),
                    Box::new(move |event| {
                        seen.lock().unwrap().push(event.kind().to_owned());
                        Ok(())
                    }),
                )
                .unwrap_err();
            assert_eq!(error.kind(), BackendErrorKind::Protocol);
            assert_eq!(*emitted.lock().unwrap(), ["turn.started"]);
        }
    }

    #[test]
    fn every_control_uses_an_independent_authenticated_connection() {
        let connector = FakeConnector::with([
            FakePlan::Frames(vec![response(1, json!({"status":"ok"}))]),
            FakePlan::Frames(vec![response(1, json!({"status":"ok"}))]),
            FakePlan::Frames(vec![response(1, json!({"status":"ok"}))]),
        ]);
        let subject = backend(connector.clone());
        subject
            .resolve_approval(1, "approval-a".into(), true)
            .unwrap();
        subject
            .resolve_input(2, "input-a".into(), "answer".into())
            .unwrap();
        subject.interrupt(3, "turn-a".into()).unwrap();
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
        let status = subject.switch_runtime(4, "codex".into()).unwrap();
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
        let connector = FakeConnector::with([FakePlan::Blocked(Arc::clone(&blocked))]);
        let subject = Arc::new(backend(connector.clone()));
        let worker = {
            let subject = Arc::clone(&subject);
            std::thread::spawn(move || {
                subject.stream_turn(91, "hello".into(), Box::new(|_| Ok(())))
            })
        };
        connector.wait_until_connected();
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
        let subject = Arc::new(backend(connector.clone()));
        let worker = {
            let subject = Arc::clone(&subject);
            std::thread::spawn(move || subject.resolve_input(5, "input-a".into(), "answer".into()))
        };
        connector.wait_until_connected();
        subject.detach_controls().unwrap();
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind(),
            BackendErrorKind::Cancelled
        );
        subject.interrupt(6, "turn-b".into()).unwrap();
    }

    #[test]
    fn runtime_list_maps_controller_payload_without_hard_coding_runtime_ids() {
        let subject = backend(FakeConnector::with([FakePlan::Frames(vec![response(
            1,
            json!({
                "current":"custom-agent",
                "runtimes":[{
                    "id":"custom-agent",
                    "label":"Custom Agent",
                    "available":true,
                    "capabilities":{"interactive_chat":true,"tool_approval":false}
                }]
            }),
        )])]));
        let list = subject.runtime_list(7).unwrap();
        assert_eq!(list.active_runtime.as_deref(), Some("custom-agent"));
        assert_eq!(list.runtimes[0].id, "custom-agent");
        assert_eq!(list.runtimes[0].display_name, "Custom Agent");
        assert_eq!(
            list.runtimes[0].capability_summary.as_deref(),
            Some("interactive_chat")
        );
    }

    #[test]
    fn session_model_and_permission_operations_are_single_tui_semantic_calls() {
        let connector = FakeConnector::with([
            FakePlan::Frames(vec![response(
                1,
                json!({"sessions":[{
                    "id":"logical-1",
                    "title":"Saved task",
                    "last_active_at":"2026-08-25T10:00:00Z",
                    "runtime_id":"codex",
                    "model_id":"gpt-5.6",
                    "status":"completed",
                    "native_session_id":"thread-1"
                }]}),
            )]),
            FakePlan::Frames(vec![response(
                1,
                json!({"status":"ready","session_id":"logical-1","attachment_epoch":3,"resume_token":"controller:1"}),
            )]),
            FakePlan::Frames(vec![response(
                1,
                json!({
                    "status":"ok",
                    "session_id":"logical-1",
                    "snapshot":{
                        "descriptor":{
                            "id":"logical-1",
                            "title":"Saved task",
                            "last_active_at":"2026-08-25T10:00:00Z",
                            "runtime_id":"codex",
                            "model_id":"gpt-5.6",
                            "status":"completed",
                            "native_session_id":"thread-1"
                        },
                        "cursor":0,
                        "normalized_events":[]
                    },
                    "runtime":"codex",
                    "model_id":"gpt-5.6",
                    "approval_profile":"request",
                    "attachment_epoch":4
                }),
            )]),
            FakePlan::Frames(vec![response(
                1,
                json!({"catalog":{
                    "runtime_id":"codex",
                    "current_model":"gpt-5.6",
                    "models":[{"id":"gpt-5.6","display_name":"GPT-5.6","is_default":true,"available":true}]
                }}),
            )]),
            FakePlan::Frames(vec![response(
                1,
                json!({"status":"ready","model_id":"gpt-5.5","switch_token":"controller:2"}),
            )]),
            FakePlan::Frames(vec![response(
                1,
                json!({"status":"ok","model_id":"gpt-5.5","attachment_epoch":5}),
            )]),
            FakePlan::Frames(vec![response(
                1,
                json!({
                    "permissions":{
                        "supported_profiles":["request","assisted","full_access"],
                        "control_strength":"partial",
                        "native_mode":"on-request",
                        "sandbox":"workspace-write",
                        "residual_guards":["os-policy"],
                        "evidence_version":"codex-0.139"
                    },
                    "current_profile":"request"
                }),
            )]),
            FakePlan::Frames(vec![response(
                1,
                json!({"status":"ready","profile":"assisted","switch_token":"controller:3"}),
            )]),
            FakePlan::Frames(vec![response(
                1,
                json!({"status":"ok","profile":"assisted","attachment_epoch":6}),
            )]),
        ]);
        let subject = backend(connector.clone());

        let sessions = subject.session_list(20, Some("saved".into())).unwrap();
        assert_eq!(sessions.sessions[0].id, "logical-1");
        let resumed = subject.resume_session(21, "logical-1".into()).unwrap();
        assert_eq!(resumed.runtime_id, "codex");
        let models = subject.model_list(22).unwrap();
        assert_eq!(models.current_model.as_deref(), Some("gpt-5.6"));
        subject.switch_model(23, "gpt-5.5".into()).unwrap();
        let permissions = subject.permissions(24).unwrap();
        assert_eq!(permissions.current_profile, PermissionMode::Request);
        subject
            .switch_permissions(25, PermissionMode::Assisted, false)
            .unwrap();

        assert_eq!(
            connector.methods(),
            [
                "session.list",
                "session.resume.prepare",
                "session.resume.commit",
                "model.list",
                "model.switch.prepare",
                "model.switch.commit",
                "permissions.inspect",
                "permissions.switch.prepare",
                "permissions.switch.commit"
            ]
        );
    }

    #[test]
    fn model_switch_rejects_a_commit_that_does_not_match_prepare() {
        let connector = FakeConnector::with([
            FakePlan::Frames(vec![response(
                1,
                json!({"status":"ready","model_id":"gpt-5.5","switch_token":"controller:9"}),
            )]),
            FakePlan::Frames(vec![response(
                1,
                json!({"status":"ok","model_id":"another-model","attachment_epoch":7}),
            )]),
        ]);
        let subject = backend(connector.clone());

        let error = subject.switch_model(26, "gpt-5.5".into()).unwrap_err();

        assert_eq!(error.kind(), BackendErrorKind::Protocol);
        assert_eq!(
            connector.methods(),
            ["model.switch.prepare", "model.switch.commit"]
        );
    }

    #[test]
    fn runtime_list_uses_controller_snapshot_without_deep_detection() {
        let connector = FakeConnector::with([FakePlan::Frames(vec![response(
            1,
            json!({
                "current":"codex",
                "runtimes":[
                    {"id":"echo","label":"Stage 1 Echo","available":true},
                    {"id":"codex","label":"Codex App Server","available":true}
                ]
            }),
        )])]);
        let subject = backend(connector.clone());

        let list = subject.runtime_list(75).unwrap();

        assert_eq!(list.active_runtime.as_deref(), Some("codex"));
        assert_eq!(list.runtimes.len(), 2);
        assert_eq!(connector.methods(), ["runtime.list"]);
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
            let connector = FakeConnector::with([FakePlan::Blocked(blocked)]);
            let subject = Arc::new(backend(connector.clone()));
            let worker = {
                let subject = Arc::clone(&subject);
                std::thread::spawn(move || match kind {
                    BlockedControl::Approval => subject.resolve_approval(8, "a".into(), true),
                    BlockedControl::Input => subject.resolve_input(8, "i".into(), "v".into()),
                    BlockedControl::Interrupt => subject.interrupt(8, "t".into()),
                    BlockedControl::RuntimeList => subject.runtime_list(8).map(|_| ()),
                    BlockedControl::RuntimeSwitch => {
                        subject.switch_runtime(8, "codex".into()).map(|_| ())
                    }
                })
            };
            connector.wait_until_connected();
            subject.detach_controls().unwrap();
            assert_eq!(
                worker.join().unwrap().unwrap_err().kind(),
                BackendErrorKind::Cancelled
            );
        }
    }

    #[test]
    fn detach_before_worker_start_cancels_announced_stream_without_connecting() {
        let subject = Arc::new(backend(FakeConnector::default()));
        subject.begin_stream(70).unwrap();
        subject.detach_stream(70).unwrap();
        let worker = {
            let subject = Arc::clone(&subject);
            std::thread::spawn(move || {
                subject.stream_turn(70, "hello".into(), Box::new(|_| Ok(())))
            })
        };
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind(),
            BackendErrorKind::Cancelled
        );
    }

    #[test]
    fn detach_before_worker_start_cancels_all_announced_control_kinds_without_connecting() {
        for (index, kind) in [
            BlockedControl::Approval,
            BlockedControl::Input,
            BlockedControl::Interrupt,
            BlockedControl::RuntimeList,
            BlockedControl::RuntimeSwitch,
        ]
        .into_iter()
        .enumerate()
        {
            let token = 80 + index as u64;
            let subject = Arc::new(backend(FakeConnector::default()));
            subject.begin_control(token).unwrap();
            subject.detach_controls().unwrap();
            let worker = {
                let subject = Arc::clone(&subject);
                std::thread::spawn(move || match kind {
                    BlockedControl::Approval => subject.resolve_approval(token, "a".into(), true),
                    BlockedControl::Input => subject.resolve_input(token, "i".into(), "v".into()),
                    BlockedControl::Interrupt => subject.interrupt(token, "t".into()),
                    BlockedControl::RuntimeList => subject.runtime_list(token).map(|_| ()),
                    BlockedControl::RuntimeSwitch => {
                        subject.switch_runtime(token, "codex".into()).map(|_| ())
                    }
                })
            };
            assert_eq!(
                worker.join().unwrap().unwrap_err().kind(),
                BackendErrorKind::Cancelled
            );
        }
    }

    #[test]
    fn detached_old_control_token_does_not_cancel_a_new_token() {
        let subject = backend(FakeConnector::with([FakePlan::Frames(vec![response(
            1,
            json!({"status":"ok"}),
        )])]));
        subject.begin_control(90).unwrap();
        subject.detach_controls().unwrap();
        assert_eq!(
            subject.interrupt(90, "old".into()).unwrap_err().kind(),
            BackendErrorKind::Cancelled
        );
        subject.begin_control(91).unwrap();
        subject.interrupt(91, "new".into()).unwrap();
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
        assert_eq!(auth.safe_message(), "Runtime authentication is required");

        let secret = map_distributed(DistributedError::new(
            StableExitCode::RuntimeFailed,
            "invalid bearer secret-token",
        ));
        assert_eq!(secret.kind(), BackendErrorKind::Operation);
        assert_eq!(secret.safe_message(), "Runtime operation failed");
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
        let error = subject.switch_runtime(9, "codex".into()).unwrap_err();
        assert_eq!(error.kind(), BackendErrorKind::AuthBlocked);
        assert_eq!(error.exit_code(), Some(StableExitCode::BlockedAuth));
        assert_eq!(error.safe_message(), "Runtime authentication is required");
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
        let error = subject.switch_runtime(10, "codex".into()).unwrap_err();
        assert_eq!(error.kind(), BackendErrorKind::Protocol);
        assert_eq!(
            connector.methods(),
            ["runtime.detect", "runtime.switch.prepare"]
        );
    }

    #[test]
    fn runtime_switch_requires_successful_commit_and_available_final_detect() {
        let prepare = || {
            response(
                1,
                json!({
                    "runtime":"codex","status":"ready","switch_token":"switch-1","requires_compression":false,
                    "context":{"strategy":"none","reason":"clean","portable_checkpoint":false},
                    "tools":{"policy":"portable_or_replay_only","active_tool_calls":0,"blocking_missing_tools":[]}
                }),
            )
        };
        let detect = || response(1, json!({"runtime":"codex","status":"available"}));

        let failed_commit = backend(FakeConnector::with([
            FakePlan::Frames(vec![detect()]),
            FakePlan::Frames(vec![prepare()]),
            FakePlan::Frames(vec![response(
                1,
                json!({"runtime":"codex","status":"failed","switch_token":"switch-1"}),
            )]),
        ]));
        assert_eq!(
            failed_commit
                .switch_runtime(11, "codex".into())
                .unwrap_err()
                .kind(),
            BackendErrorKind::Protocol
        );

        let unavailable_final = backend(FakeConnector::with([
            FakePlan::Frames(vec![detect()]),
            FakePlan::Frames(vec![prepare()]),
            FakePlan::Frames(vec![response(
                1,
                json!({"runtime":"codex","status":"ok","switch_token":"switch-1"}),
            )]),
            FakePlan::Frames(vec![response(
                1,
                json!({"runtime":"codex","status":"blocked_auth","exit_code":4}),
            )]),
        ]));
        assert_eq!(
            unavailable_final
                .switch_runtime(12, "codex".into())
                .unwrap_err()
                .kind(),
            BackendErrorKind::AuthBlocked
        );
    }

    #[test]
    fn runtime_list_preserves_controller_candidate_availability() {
        let subject = backend(FakeConnector::with([FakePlan::Frames(vec![response(
            1,
            json!({
                "current":"good",
                "runtimes":[
                    {"id":"bad","label":"Bad","available":false},
                    {"id":"good","label":"Good","available":true}
                ]
            }),
        )])]));
        let list = subject.runtime_list(13).unwrap();
        assert_eq!(list.runtimes.len(), 2);
        assert!(!list.runtimes[0].available);
        assert!(list.runtimes[1].available);
    }

    #[test]
    fn runtime_list_detach_returns_cancelled() {
        let blocked = Arc::new((Mutex::new(false), Condvar::new()));
        let connector = FakeConnector::with([FakePlan::Blocked(blocked)]);
        let subject = Arc::new(backend(connector.clone()));
        let worker = {
            let subject = Arc::clone(&subject);
            std::thread::spawn(move || subject.runtime_list(74))
        };
        connector.wait_until_connection_count(1);
        subject.detach_controls().unwrap();
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind(),
            BackendErrorKind::Cancelled
        );
    }

    #[test]
    fn remote_errors_never_expose_untrusted_message_content() {
        for message in [
            "api_key=abc",
            "Authorization: Bearer abc",
            "cookie=session",
            "credential=abc",
            "private_key=abc",
            "session token abc",
            "value-without-a-label-9f82ac",
        ] {
            let error = map_distributed(DistributedError::new(
                StableExitCode::RuntimeFailed,
                message,
            ));
            assert_eq!(error.safe_message(), "Runtime operation failed");
            assert!(!error.safe_message().contains("abc"));
            assert!(!error.safe_message().contains("9f82ac"));
        }
        assert_eq!(
            protocol_error("local tokenizer state is invalid").safe_message(),
            "local tokenizer state is invalid"
        );
    }

    #[derive(Default)]
    struct FakeBootstrap(AtomicU64);

    impl ControllerBootstrap for FakeBootstrap {
        fn ensure(&self) -> Result<(), DistributedError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    #[test]
    fn construction_bootstraps_controller_before_exposing_backend() {
        let bootstrap = Arc::new(FakeBootstrap::default());
        let subject = ControllerTuiBackend::with_connector_and_bootstrap(
            std::env::current_dir().unwrap(),
            Arc::new(FakeConnector::default()),
            bootstrap.clone(),
        )
        .unwrap();
        assert_eq!(bootstrap.0.load(Ordering::Relaxed), 1);
        assert_eq!(
            subject.workspace().unwrap(),
            std::env::current_dir().unwrap().canonicalize().unwrap()
        );
    }

    struct FailingBootstrap(StableExitCode);

    impl ControllerBootstrap for FailingBootstrap {
        fn ensure(&self) -> Result<(), DistributedError> {
            Err(DistributedError::new(self.0, "untrusted startup detail"))
        }
    }

    #[test]
    fn bootstrap_failures_keep_typed_safe_error_categories() {
        for (code, expected_kind, expected_message) in [
            (
                StableExitCode::ControllerUnavailable,
                BackendErrorKind::ControllerUnavailable,
                "Controller is unavailable",
            ),
            (
                StableExitCode::BlockedAuth,
                BackendErrorKind::AuthBlocked,
                "Runtime authentication is required",
            ),
        ] {
            let error = ControllerTuiBackend::with_connector_and_bootstrap(
                std::env::current_dir().unwrap(),
                Arc::new(FakeConnector::default()),
                Arc::new(FailingBootstrap(code)),
            )
            .err()
            .unwrap();
            assert_eq!(error.kind(), expected_kind);
            assert_eq!(error.exit_code(), Some(code));
            assert_eq!(error.safe_message(), expected_message);
        }
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
            std::thread::spawn(move || subject.runtime_list(14))
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
