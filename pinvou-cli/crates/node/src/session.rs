use pinvou_agent_adapter_codex::{CodexAdapter, CodexAdapterConfig};
use pinvou_protocol::{
    HelloClient, HelloServer, IpcMessage, IpcMessageKind, RuntimeEventEnvelope, RuntimeEventKind,
};
use pinvou_runtime_api::{AgentRuntimeAdapter, RuntimeCommand, RuntimeOperation, RuntimeSession};
use serde_json::json;
use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use crate::NodeError;

pub trait NodeRuntimeHost: Send + Sync + std::fmt::Debug {
    fn echo(&self, node_id: &str, text: &str, seq: u64) -> Result<RuntimeEventEnvelope, NodeError>;

    fn detect(&self) -> Result<serde_json::Value, NodeError> {
        Ok(json!({
            "status": "available",
            "auth_status": "not_required",
            "capabilities": {
                "interactive_chat": true,
                "native_resume": false,
                "history_import": false,
                "tool_approval": false,
                "elicitation": false,
                "steering": false,
                "image_input": false,
                "file_reference": false,
                "session_modes": ["interactive"],
                "config_options": [],
                "auth_flows": []
            }
        }))
    }

    fn resolve_approval(
        &self,
        approval_id: &str,
        _accepted: bool,
    ) -> Result<serde_json::Value, NodeError> {
        Ok(json!({"status":"unsupported", "method":"approval.resolve", "approval_id":approval_id}))
    }

    fn resolve_input(
        &self,
        input_id: &str,
        _value: &serde_json::Value,
    ) -> Result<serde_json::Value, NodeError> {
        Ok(json!({"status":"unsupported", "method":"input.resolve", "input_id":input_id}))
    }

    fn interrupt_turn(&self, turn_id: &str) -> Result<serde_json::Value, NodeError> {
        Ok(json!({"status":"unsupported", "method":"turn.interrupt", "turn_id":turn_id}))
    }
}

pub struct AdapterRuntimeHost {
    inner: Mutex<AdapterRuntimeState>,
}

struct AdapterRuntimeState {
    adapter: Box<dyn AgentRuntimeAdapter>,
    probed: bool,
    session: Option<RuntimeSession>,
}

impl AdapterRuntimeHost {
    pub fn new(adapter: Box<dyn AgentRuntimeAdapter>) -> Self {
        Self {
            inner: Mutex::new(AdapterRuntimeState {
                adapter,
                probed: false,
                session: None,
            }),
        }
    }
}

impl fmt::Debug for AdapterRuntimeHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdapterRuntimeHost")
            .finish_non_exhaustive()
    }
}

impl Drop for AdapterRuntimeHost {
    fn drop(&mut self) {
        let Ok(state) = self.inner.get_mut() else {
            return;
        };
        if let Some(session) = state.session.take() {
            let _ = state.adapter.close(&session);
        }
    }
}

impl NodeRuntimeHost for AdapterRuntimeHost {
    fn detect(&self) -> Result<serde_json::Value, NodeError> {
        let mut inner = self.inner.lock().map_err(|_| NodeError::InvalidMessage)?;
        if let Err(error) = ensure_adapter_probe(&mut inner) {
            return Ok(runtime_error_status(&error));
        }
        let capabilities = inner.adapter.capabilities()?;
        let auth_status = inner.adapter.auth_status()?;
        let status = match auth_status {
            pinvou_runtime_api::AuthStatus::Blocked => "blocked_auth",
            _ => "available",
        };
        Ok(json!({
            "status": status,
            "auth_status": auth_status,
            "capabilities": capabilities
        }))
    }

    fn echo(&self, _: &str, text: &str, _: u64) -> Result<RuntimeEventEnvelope, NodeError> {
        let mut inner = self.inner.lock().map_err(|_| NodeError::InvalidMessage)?;
        let session = ensure_adapter_session(&mut inner)?;
        inner.adapter.send(&session, RuntimeCommand::text(text)?)?;
        let mut events = inner.adapter.subscribe_events(&session)?;
        for event in events.by_ref() {
            let event = event?;
            if event.event_kind() == RuntimeEventKind::TextDelta {
                return Ok(event);
            }
        }
        Err(NodeError::InvalidMessage)
    }

    fn resolve_approval(
        &self,
        approval_id: &str,
        accepted: bool,
    ) -> Result<serde_json::Value, NodeError> {
        let mut inner = self.inner.lock().map_err(|_| NodeError::InvalidMessage)?;
        let session = ensure_adapter_session(&mut inner)?;
        inner.adapter.approve(
            &session,
            RuntimeOperation::new(
                approval_id,
                json!({"decision": if accepted { "accept" } else { "decline" }}),
            )?,
        )?;
        Ok(json!({"status":"ok", "method":"approval.resolve", "approval_id":approval_id}))
    }

    fn resolve_input(
        &self,
        input_id: &str,
        value: &serde_json::Value,
    ) -> Result<serde_json::Value, NodeError> {
        let mut inner = self.inner.lock().map_err(|_| NodeError::InvalidMessage)?;
        let session = ensure_adapter_session(&mut inner)?;
        inner.adapter.respond_input(
            &session,
            RuntimeOperation::new(input_id, json!({"value":value}))?,
        )?;
        Ok(json!({"status":"ok", "method":"input.resolve", "input_id":input_id}))
    }

    fn interrupt_turn(&self, turn_id: &str) -> Result<serde_json::Value, NodeError> {
        let mut inner = self.inner.lock().map_err(|_| NodeError::InvalidMessage)?;
        let session = ensure_adapter_session(&mut inner)?;
        inner.adapter.interrupt(&session)?;
        Ok(json!({"status":"ok", "method":"turn.interrupt", "turn_id":turn_id}))
    }
}

fn ensure_adapter_session(state: &mut AdapterRuntimeState) -> Result<RuntimeSession, NodeError> {
    ensure_adapter_probe(state)?;
    if let Some(session) = &state.session {
        return Ok(session.clone());
    }
    let session = state
        .adapter
        .create(RuntimeOperation::new("node-runtime", json!({}))?)?;
    state.session = Some(session.clone());
    Ok(session)
}

fn ensure_adapter_probe(state: &mut AdapterRuntimeState) -> Result<(), NodeError> {
    if !state.probed {
        state.adapter.probe()?;
        state.probed = true;
    }
    Ok(())
}

fn runtime_error_status(error: &NodeError) -> serde_json::Value {
    match error {
        NodeError::Runtime(error) => json!({
            "status": runtime_status_for_error(error),
            "error_kind": runtime_error_kind(error),
            "exit_code": error.exit_code().as_i32(),
            "message": error.to_string()
        }),
        _ => json!({
            "status": "unavailable",
            "error_kind": "node_error",
            "exit_code": pinvou_protocol::StableExitCode::RuntimeFailed.as_i32(),
            "message": error.to_string()
        }),
    }
}

fn runtime_status_for_error(error: &pinvou_runtime_api::AdapterError) -> &'static str {
    match error {
        pinvou_runtime_api::AdapterError::BlockedAuth => "blocked_auth",
        pinvou_runtime_api::AdapterError::QuotaExceeded => "quota_exceeded",
        pinvou_runtime_api::AdapterError::HandshakeTimeout => "handshake_timeout",
        _ => "unavailable",
    }
}

fn runtime_error_kind(error: &pinvou_runtime_api::AdapterError) -> &'static str {
    match error {
        pinvou_runtime_api::AdapterError::Unsupported { .. } => "unsupported",
        pinvou_runtime_api::AdapterError::NotProbed => "not_probed",
        pinvou_runtime_api::AdapterError::HandshakeTimeout => "handshake_timeout",
        pinvou_runtime_api::AdapterError::BlockedAuth => "blocked_auth",
        pinvou_runtime_api::AdapterError::QuotaExceeded => "quota_exceeded",
        pinvou_runtime_api::AdapterError::Protocol { .. } => "protocol",
        pinvou_runtime_api::AdapterError::ProcessExit { .. } => "process_exit",
        pinvou_runtime_api::AdapterError::InvalidRequest { .. } => "invalid_request",
        pinvou_runtime_api::AdapterError::Cancelled => "cancelled",
    }
}

#[derive(Clone, Debug)]
pub struct NodeSession {
    instance_id: String,
    next_seq: Arc<AtomicU64>,
    runtime: Arc<Mutex<RuntimeSlot>>,
    active_turn: Arc<Mutex<Option<u64>>>,
}

#[derive(Debug)]
struct RuntimeSlot {
    id: String,
    host: Arc<dyn NodeRuntimeHost>,
    state_file: Option<PathBuf>,
}

impl NodeSession {
    pub fn new(instance_id: impl Into<String>) -> Result<Self, NodeError> {
        Self::with_runtime_id(instance_id, "echo", Arc::new(StageOneEchoRuntime))
    }

    pub fn with_state_file(
        instance_id: impl Into<String>,
        state_file: impl Into<PathBuf>,
    ) -> Result<Self, NodeError> {
        let state_file = state_file.into();
        let runtime_id = load_runtime_selection(&state_file)?;
        let runtime = create_runtime_host(&runtime_id)?;
        Self::with_runtime_id_and_state(instance_id, runtime_id, runtime, Some(state_file))
    }

    pub fn with_runtime(
        instance_id: impl Into<String>,
        runtime: Arc<dyn NodeRuntimeHost>,
    ) -> Result<Self, NodeError> {
        Self::with_runtime_id(instance_id, "custom", runtime)
    }

    fn with_runtime_id(
        instance_id: impl Into<String>,
        runtime_id: impl Into<String>,
        runtime: Arc<dyn NodeRuntimeHost>,
    ) -> Result<Self, NodeError> {
        Self::with_runtime_id_and_state(instance_id, runtime_id, runtime, None)
    }

    fn with_runtime_id_and_state(
        instance_id: impl Into<String>,
        runtime_id: impl Into<String>,
        runtime: Arc<dyn NodeRuntimeHost>,
        state_file: Option<PathBuf>,
    ) -> Result<Self, NodeError> {
        let instance_id = instance_id.into();
        let runtime_id = runtime_id.into();
        if instance_id.is_empty() {
            Err(NodeError::InvalidMessage)
        } else {
            Ok(Self {
                instance_id,
                next_seq: Arc::new(AtomicU64::new(1)),
                active_turn: Arc::new(Mutex::new(None)),
                runtime: Arc::new(Mutex::new(RuntimeSlot {
                    id: runtime_id,
                    host: runtime,
                    state_file,
                })),
            })
        }
    }

    pub fn accept_hello(&self, hello: HelloClient) -> Result<HelloServer, NodeError> {
        if hello.protocol_version() != pinvou_protocol::IPC_VERSION {
            return Err(NodeError::ProtocolMismatch);
        }
        HelloServer::new(self.instance_id.clone()).map_err(|_| NodeError::InvalidMessage)
    }

    pub fn handle(&self, request: IpcMessage) -> Result<IpcMessage, NodeError> {
        if request.kind() != IpcMessageKind::Req {
            return Err(NodeError::InvalidMessage);
        }
        if request
            .payload()
            .get("instance_id")
            .and_then(|value| value.as_str())
            != Some(&self.instance_id)
        {
            return Err(NodeError::ProtocolMismatch);
        }
        let id = request.id().cloned().ok_or(NodeError::InvalidMessage)?;
        let payload = match request.method() {
            Some("health") => {
                json!({"status":"ok", "instance_id":self.instance_id, "protocol_version":pinvou_protocol::IPC_VERSION})
            }
            Some("runtime.list") => {
                let runtime = self.runtime.lock().map_err(|_| NodeError::InvalidMessage)?;
                json!({
                    "current": runtime.id,
                    "runtimes": [
                        {"id":"echo", "label":"Stage 1 Echo", "available":true},
                        {"id":"codex", "label":"Codex App Server", "available":true}
                    ]
                })
            }
            Some("runtime.detect") => {
                let (runtime_id, host) = if let Some(runtime) = request
                    .payload()
                    .get("runtime")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                {
                    (runtime.to_owned(), create_runtime_host(runtime)?)
                } else {
                    let runtime = self.runtime.lock().map_err(|_| NodeError::InvalidMessage)?;
                    (runtime.id.clone(), runtime.host.clone())
                };
                let mut payload = host.detect()?;
                payload["runtime"] = json!(runtime_id);
                payload["protocol_version"] = json!(pinvou_protocol::IPC_VERSION);
                payload
            }
            Some("runtime.switch") => {
                if self
                    .active_turn
                    .lock()
                    .map_err(|_| NodeError::InvalidMessage)?
                    .is_some()
                {
                    return Err(NodeError::RuntimeBusy);
                }
                let runtime = request
                    .payload()
                    .get("runtime")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .ok_or(NodeError::InvalidMessage)?;
                let mut active = self.runtime.lock().map_err(|_| NodeError::InvalidMessage)?;
                let host = create_runtime_host(runtime)?;
                if let Some(state_file) = &active.state_file {
                    persist_runtime_selection(state_file, runtime)?;
                }
                active.id = runtime.into();
                active.host = host;
                json!({"status":"ok", "runtime":runtime})
            }
            Some("runtime.echo") => {
                let text = request
                    .payload()
                    .get("text")
                    .and_then(|value| value.as_str())
                    .ok_or(NodeError::InvalidMessage)?;
                let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
                let runtime = self.current_runtime_host()?;
                let _active = ActiveTurnGuard::enter(Arc::clone(&self.active_turn), seq)?;
                let envelope = runtime.echo(&self.instance_id, text, seq)?;
                return IpcMessage::event(
                    "runtime.event",
                    serde_json::to_value(envelope).map_err(|_| NodeError::InvalidMessage)?,
                )
                .map_err(|_| NodeError::InvalidMessage);
            }
            Some("approval.resolve") => {
                let approval_id = request
                    .payload()
                    .get("approval_id")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .ok_or(NodeError::InvalidMessage)?;
                if request
                    .payload()
                    .get("accepted")
                    .and_then(|value| value.as_bool())
                    .is_none()
                {
                    return Err(NodeError::InvalidMessage);
                }
                self.current_runtime_host()?.resolve_approval(
                    approval_id,
                    request
                        .payload()
                        .get("accepted")
                        .and_then(|value| value.as_bool())
                        .ok_or(NodeError::InvalidMessage)?,
                )?
            }
            Some("input.resolve") => {
                let input_id = request
                    .payload()
                    .get("input_id")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .ok_or(NodeError::InvalidMessage)?;
                if !request
                    .payload()
                    .as_object()
                    .is_some_and(|payload| payload.contains_key("value"))
                {
                    return Err(NodeError::InvalidMessage);
                }
                self.current_runtime_host()?.resolve_input(
                    input_id,
                    request
                        .payload()
                        .get("value")
                        .ok_or(NodeError::InvalidMessage)?,
                )?
            }
            Some("turn.interrupt") => {
                let turn_id = request
                    .payload()
                    .get("turn_id")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .ok_or(NodeError::InvalidMessage)?;
                self.current_runtime_host()?.interrupt_turn(turn_id)?
            }
            _ => return Err(NodeError::UnsupportedRequest),
        };
        IpcMessage::response(id, payload).map_err(|_| NodeError::InvalidMessage)
    }

    fn current_runtime_host(&self) -> Result<Arc<dyn NodeRuntimeHost>, NodeError> {
        Ok(self
            .runtime
            .lock()
            .map_err(|_| NodeError::InvalidMessage)?
            .host
            .clone())
    }
}

struct ActiveTurnGuard {
    active_turn: Arc<Mutex<Option<u64>>>,
    seq: u64,
}

impl ActiveTurnGuard {
    fn enter(active_turn: Arc<Mutex<Option<u64>>>, seq: u64) -> Result<Self, NodeError> {
        let mut active = active_turn.lock().map_err(|_| NodeError::InvalidMessage)?;
        if active.is_some() {
            return Err(NodeError::RuntimeBusy);
        }
        *active = Some(seq);
        drop(active);
        Ok(Self { active_turn, seq })
    }
}

impl Drop for ActiveTurnGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = self.active_turn.lock()
            && *active == Some(self.seq)
        {
            *active = None;
        }
    }
}

fn load_runtime_selection(state_file: &Path) -> Result<String, NodeError> {
    match std::fs::read_to_string(state_file) {
        Ok(content) => {
            let value: serde_json::Value =
                serde_json::from_str(&content).map_err(|_| NodeError::InvalidMessage)?;
            let runtime = value
                .get("runtime")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .ok_or(NodeError::InvalidMessage)?;
            Ok(runtime.to_owned())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok("echo".to_owned()),
        Err(error) => Err(error.into()),
    }
}

fn persist_runtime_selection(state_file: &Path, runtime: &str) -> Result<(), NodeError> {
    if let Some(parent) = state_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = state_file.with_extension("json.tmp");
    let bytes = serde_json::to_vec(&json!({ "runtime": runtime }))
        .map_err(|_| NodeError::InvalidMessage)?;
    std::fs::write(&temp, bytes)?;
    std::fs::rename(temp, state_file)?;
    Ok(())
}

fn create_runtime_host(runtime: &str) -> Result<Arc<dyn NodeRuntimeHost>, NodeError> {
    match runtime {
        "echo" => Ok(Arc::new(StageOneEchoRuntime)),
        "codex" => Ok(Arc::new(AdapterRuntimeHost::new(Box::new(
            CodexAdapter::new(codex_adapter_config()?),
        )))),
        _ => Err(NodeError::UnsupportedRequest),
    }
}

fn codex_adapter_config() -> Result<CodexAdapterConfig, NodeError> {
    let mut config = CodexAdapterConfig::default();
    apply_debug_codex_adapter_overrides(&mut config)?;
    Ok(config)
}

#[cfg(debug_assertions)]
fn apply_debug_codex_adapter_overrides(config: &mut CodexAdapterConfig) -> Result<(), NodeError> {
    if let Some(executable) = std::env::var_os("PINVOU_CODEX_EXECUTABLE_FOR_TEST") {
        config.executable = std::path::PathBuf::from(executable);
    }
    if let Some(args) = debug_json_args("PINVOU_CODEX_VERSION_ARGS_JSON_FOR_TEST")? {
        config.version_args = args;
    }
    if let Some(args) = debug_json_args("PINVOU_CODEX_DOCTOR_ARGS_JSON_FOR_TEST")? {
        config.doctor_args = args;
    }
    if let Some(args) = debug_json_args("PINVOU_CODEX_APP_SERVER_ARGS_JSON_FOR_TEST")? {
        config.app_server_args = args;
    }
    if let Some(cwd) = std::env::var_os("PINVOU_CODEX_WORKING_DIRECTORY_FOR_TEST") {
        config.working_directory = Some(std::path::PathBuf::from(cwd));
    }
    Ok(())
}

#[cfg(not(debug_assertions))]
fn apply_debug_codex_adapter_overrides(_: &mut CodexAdapterConfig) -> Result<(), NodeError> {
    Ok(())
}

#[cfg(debug_assertions)]
fn debug_json_args(name: &str) -> Result<Option<Vec<std::ffi::OsString>>, NodeError> {
    let Some(value) = std::env::var_os(name) else {
        return Ok(None);
    };
    let value = value.into_string().map_err(|_| NodeError::InvalidMessage)?;
    let args: Vec<String> = serde_json::from_str(&value).map_err(|_| NodeError::InvalidMessage)?;
    if args.iter().any(|arg| arg.is_empty()) {
        return Err(NodeError::InvalidMessage);
    }
    Ok(Some(
        args.into_iter().map(std::ffi::OsString::from).collect(),
    ))
}

#[derive(Debug)]
struct StageOneEchoRuntime;

impl NodeRuntimeHost for StageOneEchoRuntime {
    fn echo(&self, node_id: &str, text: &str, seq: u64) -> Result<RuntimeEventEnvelope, NodeError> {
        pinvou_protocol::RuntimeEventEnvelope::from_value(json!({
            "protocol_version":pinvou_protocol::IPC_VERSION,"schema_version":1,"node_id":node_id,
            "logical_session_id":"m1-session","attachment_id":"m1-attachment",
            "work_id":null,"collaborative_run_id":null,"stream_id":"main",
            "turn_id":"m1-turn","seq":seq,"source_span":{"start":seq,"end":seq},
            "timestamp":utc_timestamp_now(),"rate_class":"R1","kind":"text.delta",
            "payload":{"role":"assistant","content":text,"merged_count":1}
        }))
        .map_err(|_| NodeError::InvalidMessage)
    }
}

fn utc_timestamp_now() -> String {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let total_seconds = duration.as_secs();
    let days = (total_seconds / 86_400) as i64;
    let seconds = total_seconds % 86_400;
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        seconds / 3_600,
        seconds / 60 % 60,
        seconds % 60,
        duration.subsec_millis()
    )
}
