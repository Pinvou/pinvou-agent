use pinvou_agent_adapter_codex::{CodexAdapter, CodexAdapterConfig};
use pinvou_protocol::{
    HelloClient, HelloServer, IpcMessage, IpcMessageKind, RuntimeEventEnvelope, RuntimeEventKind,
};
use pinvou_runtime_api::{
    AgentRuntimeAdapter, RuntimeCommand, RuntimeEventSubscription, RuntimeOperation, RuntimeSession,
};
use serde_json::json;
use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use crate::NodeError;

pub type NodeRuntimeEventStream =
    Box<dyn Iterator<Item = Result<RuntimeEventEnvelope, NodeError>> + Send>;

pub trait NodeRuntimeHost: Send + Sync + std::fmt::Debug {
    fn start_turn(
        &self,
        node_id: &str,
        prompt: &str,
        seq: u64,
    ) -> Result<NodeRuntimeEventStream, NodeError>;

    fn cleanup_after_delivery_failure(
        &self,
        stream: NodeRuntimeEventStream,
        _turn_id: Option<&str>,
    ) {
        drop(stream);
    }

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
    inner: Arc<Mutex<AdapterRuntimeState>>,
    retired: Arc<AtomicBool>,
    cleanup_timeout: std::time::Duration,
}

struct AdapterRuntimeState {
    adapter: Box<dyn AgentRuntimeAdapter>,
    probed: bool,
    session: Option<RuntimeSession>,
    subscription: Option<RuntimeEventSubscription>,
    closed: bool,
}

impl AdapterRuntimeHost {
    pub fn new(adapter: Box<dyn AgentRuntimeAdapter>) -> Self {
        Self::with_cleanup_timeout(adapter, std::time::Duration::from_secs(5))
    }

    pub fn with_cleanup_timeout(
        adapter: Box<dyn AgentRuntimeAdapter>,
        cleanup_timeout: std::time::Duration,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(AdapterRuntimeState {
                adapter,
                probed: false,
                session: None,
                subscription: None,
                closed: false,
            })),
            retired: Arc::new(AtomicBool::new(false)),
            cleanup_timeout,
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
        self.retired.store(true, Ordering::Release);
        if let Ok(mut state) = self.inner.try_lock() {
            close_adapter_state(&mut state);
        } else {
            close_adapter_state_async(Arc::clone(&self.inner));
        }
    }
}

impl NodeRuntimeHost for AdapterRuntimeHost {
    fn detect(&self) -> Result<serde_json::Value, NodeError> {
        ensure_adapter_available(&self.retired)?;
        let mut inner = self.inner.lock().map_err(|_| NodeError::InvalidMessage)?;
        ensure_adapter_available(&self.retired)?;
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

    fn start_turn(
        &self,
        _: &str,
        prompt: &str,
        _: u64,
    ) -> Result<NodeRuntimeEventStream, NodeError> {
        ensure_adapter_available(&self.retired)?;
        let mut inner = self.inner.lock().map_err(|_| NodeError::InvalidMessage)?;
        ensure_adapter_available(&self.retired)?;
        let session = ensure_adapter_session(&mut inner)?;
        inner
            .adapter
            .send(&session, RuntimeCommand::text(prompt)?)?;
        let events = match inner.subscription.take() {
            Some(events) => events,
            None => inner.adapter.subscribe_events(&session)?,
        };
        drop(inner);
        Ok(Box::new(AdapterTurnStream {
            inner: Arc::clone(&self.inner),
            retired: Arc::clone(&self.retired),
            subscription: Some(events),
            turn_id: None,
            finished: false,
        }))
    }

    fn cleanup_after_delivery_failure(
        &self,
        mut stream: NodeRuntimeEventStream,
        turn_id: Option<&str>,
    ) {
        let deadline = std::time::Instant::now() + self.cleanup_timeout;
        let inner = Arc::clone(&self.inner);
        let retired = Arc::clone(&self.retired);
        let turn_id = turn_id.map(str::to_owned);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("pinvou-node-runtime-drain".into())
            .spawn(move || {
                let interrupt = match turn_id {
                    Some(_) => interrupt_adapter_state(&inner, &retired),
                    None => Ok(()),
                };
                let drained = interrupt.and_then(|_| drain_runtime_stream(&mut stream));
                if drained.is_err() || retired.load(Ordering::Acquire) {
                    retire_adapter_state_blocking(&inner, &retired);
                }
                let _ = done_tx.send(drained);
            });
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match done_rx.recv_timeout(remaining) {
            Ok(Ok(())) => {}
            Ok(Err(_)) | Err(_) => retire_adapter_state(&self.inner, &self.retired),
        }
    }

    fn resolve_approval(
        &self,
        approval_id: &str,
        accepted: bool,
    ) -> Result<serde_json::Value, NodeError> {
        ensure_adapter_available(&self.retired)?;
        let mut inner = self.inner.lock().map_err(|_| NodeError::InvalidMessage)?;
        ensure_adapter_available(&self.retired)?;
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
        ensure_adapter_available(&self.retired)?;
        let mut inner = self.inner.lock().map_err(|_| NodeError::InvalidMessage)?;
        ensure_adapter_available(&self.retired)?;
        let session = ensure_adapter_session(&mut inner)?;
        inner.adapter.respond_input(
            &session,
            RuntimeOperation::new(input_id, json!({"value":value}))?,
        )?;
        Ok(json!({"status":"ok", "method":"input.resolve", "input_id":input_id}))
    }

    fn interrupt_turn(&self, turn_id: &str) -> Result<serde_json::Value, NodeError> {
        ensure_adapter_available(&self.retired)?;
        let mut inner = self.inner.lock().map_err(|_| NodeError::InvalidMessage)?;
        ensure_adapter_available(&self.retired)?;
        let session = ensure_adapter_session(&mut inner)?;
        inner.adapter.interrupt(&session)?;
        Ok(json!({"status":"ok", "method":"turn.interrupt", "turn_id":turn_id}))
    }
}

struct AdapterTurnStream {
    inner: Arc<Mutex<AdapterRuntimeState>>,
    retired: Arc<AtomicBool>,
    subscription: Option<RuntimeEventSubscription>,
    turn_id: Option<String>,
    finished: bool,
}

impl AdapterTurnStream {
    fn return_subscription(&mut self) {
        if self.retired.load(Ordering::Acquire) {
            self.subscription.take();
            return;
        }
        let Some(subscription) = self.subscription.take() else {
            return;
        };
        if let Ok(mut inner) = self.inner.lock()
            && !self.retired.load(Ordering::Acquire)
            && inner.subscription.is_none()
        {
            inner.subscription = Some(subscription);
        }
    }
}

impl Iterator for AdapterTurnStream {
    type Item = Result<RuntimeEventEnvelope, NodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        let next = self.subscription.as_mut()?.next();
        match next {
            Some(Ok(event)) => {
                if self.turn_id.is_none() && event.event_kind() == RuntimeEventKind::TurnStarted {
                    self.turn_id = event.turn_id().map(str::to_owned);
                }
                let terminal = event.event_kind() == RuntimeEventKind::TurnEnded
                    && self
                        .turn_id
                        .as_deref()
                        .is_some_and(|turn_id| event.turn_id() == Some(turn_id));
                if terminal {
                    self.finished = true;
                    self.return_subscription();
                }
                Some(Ok(event))
            }
            Some(Err(error)) => {
                self.finished = true;
                self.subscription.take();
                retire_adapter_state(&self.inner, &self.retired);
                Some(Err(NodeError::from(error)))
            }
            None => {
                self.finished = true;
                self.subscription.take();
                retire_adapter_state(&self.inner, &self.retired);
                Some(Err(NodeError::InvalidMessage))
            }
        }
    }
}

fn retire_adapter_state(inner: &Arc<Mutex<AdapterRuntimeState>>, retired: &Arc<AtomicBool>) {
    retired.store(true, Ordering::Release);
    close_adapter_state_async(Arc::clone(inner));
}

fn retire_adapter_state_blocking(
    inner: &Arc<Mutex<AdapterRuntimeState>>,
    retired: &Arc<AtomicBool>,
) {
    retired.store(true, Ordering::Release);
    if let Ok(mut state) = inner.lock() {
        close_adapter_state(&mut state);
    }
}

fn close_adapter_state_async(inner: Arc<Mutex<AdapterRuntimeState>>) {
    let _ = std::thread::Builder::new()
        .name("pinvou-node-runtime-close".into())
        .spawn(move || {
            if let Ok(mut state) = inner.lock() {
                close_adapter_state(&mut state);
            }
        });
}

fn close_adapter_state(state: &mut AdapterRuntimeState) {
    if state.closed {
        return;
    }
    state.closed = true;
    state.subscription.take();
    if let Some(session) = state.session.take() {
        let _ = state.adapter.close(&session);
    }
}

fn interrupt_adapter_state(
    inner: &Arc<Mutex<AdapterRuntimeState>>,
    retired: &Arc<AtomicBool>,
) -> Result<(), NodeError> {
    ensure_adapter_available(retired)?;
    let mut state = inner.lock().map_err(|_| NodeError::InvalidMessage)?;
    ensure_adapter_available(retired)?;
    let session = ensure_adapter_session(&mut state)?;
    state.adapter.interrupt(&session)?;
    Ok(())
}

fn ensure_adapter_available(retired: &AtomicBool) -> Result<(), NodeError> {
    if retired.load(Ordering::Acquire) {
        return Err(NodeError::Runtime(
            pinvou_runtime_api::AdapterError::InvalidRequest {
                details: "adapter runtime is retired".into(),
            },
        ));
    }
    Ok(())
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
    coordinator: Arc<Mutex<RuntimeCoordinator>>,
    pending_switch: Arc<Mutex<Option<PreparedRuntimeSwitch>>>,
}

#[derive(Debug)]
struct RuntimeCoordinator {
    runtime: RuntimeSlot,
    active_turn: Option<u64>,
}

#[derive(Debug)]
struct RuntimeSlot {
    id: String,
    host: Arc<dyn NodeRuntimeHost>,
    state_file: Option<PathBuf>,
}

#[derive(Debug)]
struct PreparedRuntimeSwitch {
    runtime: String,
    token: String,
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
                pending_switch: Arc::new(Mutex::new(None)),
                coordinator: Arc::new(Mutex::new(RuntimeCoordinator {
                    active_turn: None,
                    runtime: RuntimeSlot {
                        id: runtime_id,
                        host: runtime,
                        state_file,
                    },
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
                let coordinator = self
                    .coordinator
                    .lock()
                    .map_err(|_| NodeError::InvalidMessage)?;
                json!({
                    "current": coordinator.runtime.id,
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
                    let coordinator = self
                        .coordinator
                        .lock()
                        .map_err(|_| NodeError::InvalidMessage)?;
                    (
                        coordinator.runtime.id.clone(),
                        coordinator.runtime.host.clone(),
                    )
                };
                let mut payload = host.detect()?;
                payload["runtime"] = json!(runtime_id);
                payload["protocol_version"] = json!(pinvou_protocol::IPC_VERSION);
                payload
            }
            Some("runtime.switch") => {
                let runtime = request
                    .payload()
                    .get("runtime")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .ok_or(NodeError::InvalidMessage)?;
                self.commit_runtime_switch(runtime)?;
                json!({"status":"ok", "runtime":runtime})
            }
            Some("runtime.switch.prepare") => {
                let runtime = request
                    .payload()
                    .get("runtime")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .ok_or(NodeError::InvalidMessage)?;
                let _host = create_runtime_host(runtime)?;
                let coordinator = self
                    .coordinator
                    .lock()
                    .map_err(|_| NodeError::InvalidMessage)?;
                if coordinator.active_turn.is_some() {
                    return Err(NodeError::RuntimeBusy);
                }
                let current_runtime = coordinator.runtime.id.clone();
                drop(coordinator);
                let switch_token = format!(
                    "runtime-switch-{}",
                    self.next_seq.fetch_add(1, Ordering::Relaxed)
                );
                *self
                    .pending_switch
                    .lock()
                    .map_err(|_| NodeError::InvalidMessage)? = Some(PreparedRuntimeSwitch {
                    runtime: runtime.to_owned(),
                    token: switch_token.clone(),
                });
                json!({
                    "status": "ready",
                    "runtime": runtime,
                    "current_runtime": current_runtime,
                    "switch_token": switch_token,
                    "requires_compression": false,
                    "context": {
                        "strategy": "none",
                        "reason": "turn_boundary_clean",
                        "portable_checkpoint": false
                    },
                    "tools": {
                        "policy": "portable_or_replay_only",
                        "active_tool_calls": 0,
                        "blocking_missing_tools": []
                    }
                })
            }
            Some("runtime.switch.commit") => {
                let runtime = request
                    .payload()
                    .get("runtime")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .ok_or(NodeError::InvalidMessage)?;
                let switch_token = request
                    .payload()
                    .get("switch_token")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .ok_or(NodeError::InvalidMessage)?;
                {
                    let pending = self
                        .pending_switch
                        .lock()
                        .map_err(|_| NodeError::InvalidMessage)?;
                    let Some(prepared) = pending.as_ref() else {
                        return Err(NodeError::InvalidMessage);
                    };
                    if prepared.runtime != runtime || prepared.token != switch_token {
                        return Err(NodeError::InvalidMessage);
                    }
                }
                self.commit_runtime_switch(runtime)?;
                let mut pending = self
                    .pending_switch
                    .lock()
                    .map_err(|_| NodeError::InvalidMessage)?;
                if pending
                    .as_ref()
                    .is_some_and(|prepared| prepared.token == switch_token)
                {
                    *pending = None;
                }
                json!({"status":"ok", "runtime":runtime, "switch_token":switch_token})
            }
            Some("runtime.echo") => {
                let text = request
                    .payload()
                    .get("text")
                    .and_then(|value| value.as_str())
                    .ok_or(NodeError::InvalidMessage)?;
                let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
                let (runtime, _active) = self.begin_turn(seq)?;
                let mut stream = runtime.start_turn(&self.instance_id, text, seq)?;
                let mut first_delta = None;
                for event in stream.by_ref() {
                    let event = event?;
                    if first_delta.is_none() && event.event_kind() == RuntimeEventKind::TextDelta {
                        first_delta = Some(event);
                    }
                }
                let envelope = first_delta.ok_or(NodeError::InvalidMessage)?;
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

    pub fn stream_bound<F>(&self, request: IpcMessage, mut emit: F) -> Result<(), NodeError>
    where
        F: FnMut(IpcMessage) -> Result<(), NodeError>,
    {
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
        if request.method() != Some("chat.start") {
            return Err(NodeError::UnsupportedRequest);
        }
        let prompt = request
            .payload()
            .get("prompt")
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .ok_or(NodeError::InvalidMessage)?;
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let (runtime, _active) = self.begin_turn(seq)?;
        let mut stream = runtime.start_turn(&self.instance_id, prompt, seq)?;
        let mut turn_id = None;
        while let Some(envelope) = stream.next() {
            let envelope = envelope?;
            if turn_id.is_none() {
                turn_id = envelope.turn_id().map(str::to_owned);
            }
            let delivery = serde_json::to_value(envelope)
                .map_err(|_| NodeError::InvalidMessage)
                .and_then(|payload| {
                    IpcMessage::event("runtime.event", payload)
                        .map_err(|_| NodeError::InvalidMessage)
                })
                .and_then(&mut emit);
            if let Err(delivery_error) = delivery {
                runtime.cleanup_after_delivery_failure(stream, turn_id.as_deref());
                return Err(delivery_error);
            }
        }
        Ok(())
    }

    fn current_runtime_host(&self) -> Result<Arc<dyn NodeRuntimeHost>, NodeError> {
        Ok(self
            .coordinator
            .lock()
            .map_err(|_| NodeError::InvalidMessage)?
            .runtime
            .host
            .clone())
    }

    fn begin_turn(
        &self,
        seq: u64,
    ) -> Result<(Arc<dyn NodeRuntimeHost>, ActiveTurnGuard), NodeError> {
        let mut coordinator = self
            .coordinator
            .lock()
            .map_err(|_| NodeError::InvalidMessage)?;
        if coordinator.active_turn.is_some() {
            return Err(NodeError::RuntimeBusy);
        }
        coordinator.active_turn = Some(seq);
        let runtime = coordinator.runtime.host.clone();
        drop(coordinator);
        Ok((
            runtime,
            ActiveTurnGuard {
                coordinator: Arc::clone(&self.coordinator),
                seq,
            },
        ))
    }

    fn commit_runtime_switch(&self, runtime: &str) -> Result<(), NodeError> {
        let host = create_runtime_host(runtime)?;
        let mut coordinator = self
            .coordinator
            .lock()
            .map_err(|_| NodeError::InvalidMessage)?;
        if coordinator.active_turn.is_some() {
            return Err(NodeError::RuntimeBusy);
        }
        if let Some(state_file) = &coordinator.runtime.state_file {
            persist_runtime_selection(state_file, runtime)?;
        }
        coordinator.runtime.id = runtime.into();
        coordinator.runtime.host = host;
        Ok(())
    }
}

fn drain_runtime_stream(stream: &mut NodeRuntimeEventStream) -> Result<(), NodeError> {
    for event in stream {
        event?;
    }
    Ok(())
}

struct ActiveTurnGuard {
    coordinator: Arc<Mutex<RuntimeCoordinator>>,
    seq: u64,
}

impl Drop for ActiveTurnGuard {
    fn drop(&mut self) {
        if let Ok(mut coordinator) = self.coordinator.lock()
            && coordinator.active_turn == Some(self.seq)
        {
            coordinator.active_turn = None;
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
    fn start_turn(
        &self,
        node_id: &str,
        prompt: &str,
        seq: u64,
    ) -> Result<NodeRuntimeEventStream, NodeError> {
        let event = pinvou_protocol::RuntimeEventEnvelope::from_value(json!({
            "protocol_version":pinvou_protocol::IPC_VERSION,"schema_version":1,"node_id":node_id,
            "logical_session_id":"m1-session","attachment_id":"m1-attachment",
            "work_id":null,"collaborative_run_id":null,"stream_id":"main",
            "turn_id":"m1-turn","seq":seq,"source_span":{"start":seq,"end":seq},
            "timestamp":utc_timestamp_now(),"rate_class":"R1","kind":"text.delta",
            "payload":{"role":"assistant","content":prompt,"merged_count":1}
        }))
        .map_err(|_| NodeError::InvalidMessage)?;
        Ok(Box::new(std::iter::once(Ok(event))))
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
