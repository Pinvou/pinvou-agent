use pinvou_protocol::{HelloClient, HelloServer, IpcMessage, IpcMessageKind, RuntimeEventEnvelope};
use pinvou_runtime_api::{AgentRuntimeAdapter, RuntimeCommand, RuntimeOperation, RuntimeSession};
use serde_json::json;
use std::{
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use crate::NodeError;

pub trait NodeRuntimeHost: Send + Sync + std::fmt::Debug {
    fn echo(&self, node_id: &str, text: &str, seq: u64) -> Result<RuntimeEventEnvelope, NodeError>;

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
    fn echo(&self, _: &str, text: &str, _: u64) -> Result<RuntimeEventEnvelope, NodeError> {
        let mut inner = self.inner.lock().map_err(|_| NodeError::InvalidMessage)?;
        let session = ensure_adapter_session(&mut inner)?;
        inner.adapter.send(&session, RuntimeCommand::text(text)?)?;
        let mut events = inner.adapter.subscribe_events(&session)?;
        match events.next() {
            Some(Ok(event)) => Ok(event),
            Some(Err(error)) => Err(error.into()),
            None => Err(NodeError::InvalidMessage),
        }
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
    if !state.probed {
        state.adapter.probe()?;
        state.probed = true;
    }
    if let Some(session) = &state.session {
        return Ok(session.clone());
    }
    let session = state
        .adapter
        .create(RuntimeOperation::new("node-runtime", json!({}))?)?;
    state.session = Some(session.clone());
    Ok(session)
}

#[derive(Clone, Debug)]
pub struct NodeSession {
    instance_id: String,
    next_seq: Arc<AtomicU64>,
    runtime: Arc<dyn NodeRuntimeHost>,
}

impl NodeSession {
    pub fn new(instance_id: impl Into<String>) -> Result<Self, NodeError> {
        Self::with_runtime(instance_id, Arc::new(StageOneEchoRuntime))
    }

    pub fn with_runtime(
        instance_id: impl Into<String>,
        runtime: Arc<dyn NodeRuntimeHost>,
    ) -> Result<Self, NodeError> {
        let instance_id = instance_id.into();
        if instance_id.is_empty() {
            Err(NodeError::InvalidMessage)
        } else {
            Ok(Self {
                instance_id,
                next_seq: Arc::new(AtomicU64::new(1)),
                runtime,
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
            Some("runtime.echo") => {
                let text = request
                    .payload()
                    .get("text")
                    .and_then(|value| value.as_str())
                    .ok_or(NodeError::InvalidMessage)?;
                let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
                let envelope = self.runtime.echo(&self.instance_id, text, seq)?;
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
                self.runtime.resolve_approval(
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
                self.runtime.resolve_input(
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
                self.runtime.interrupt_turn(turn_id)?
            }
            _ => return Err(NodeError::UnsupportedRequest),
        };
        IpcMessage::response(id, payload).map_err(|_| NodeError::InvalidMessage)
    }
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
