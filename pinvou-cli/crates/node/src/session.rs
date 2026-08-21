use pinvou_protocol::{HelloClient, HelloServer, IpcMessage, IpcMessageKind};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use crate::NodeError;

#[derive(Clone, Debug)]
pub struct NodeSession {
    instance_id: String,
    next_seq: Arc<AtomicU64>,
}

impl NodeSession {
    pub fn new(instance_id: impl Into<String>) -> Result<Self, NodeError> {
        let instance_id = instance_id.into();
        if instance_id.is_empty() {
            Err(NodeError::InvalidMessage)
        } else {
            Ok(Self {
                instance_id,
                next_seq: Arc::new(AtomicU64::new(1)),
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
                let envelope = pinvou_protocol::RuntimeEventEnvelope::from_value(json!({
                    "protocol_version":pinvou_protocol::IPC_VERSION,"schema_version":1,"node_id":self.instance_id,
                    "logical_session_id":"m1-session","attachment_id":"m1-attachment",
                    "work_id":null,"collaborative_run_id":null,"stream_id":"main",
                    "turn_id":"m1-turn","seq":seq,"source_span":{"start":seq,"end":seq},
                    "timestamp":utc_timestamp_now(),"rate_class":"R1","kind":"text.delta",
                    "payload":{"role":"assistant","content":text,"merged_count":1}
                }))
                .map_err(|_| NodeError::InvalidMessage)?;
                return IpcMessage::event(
                    "runtime.event",
                    serde_json::to_value(envelope).map_err(|_| NodeError::InvalidMessage)?,
                )
                .map_err(|_| NodeError::InvalidMessage);
            }
            _ => return Err(NodeError::UnsupportedRequest),
        };
        IpcMessage::response(id, payload).map_err(|_| NodeError::InvalidMessage)
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
