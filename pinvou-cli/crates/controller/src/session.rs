use pinvou_protocol::{HelloClient, HelloServer, IpcMessage, IpcMessageKind, RuntimeEventEnvelope};
use serde_json::json;

use crate::{ControllerError, LocalNodeClient};

#[derive(Clone, Debug)]
pub struct ControllerSession {
    instance_id: String,
    local_node: Option<LocalNodeRoute>,
}

#[derive(Clone, Debug)]
struct LocalNodeRoute {
    endpoint: String,
    instance_id: String,
}

impl ControllerSession {
    pub fn new(instance_id: impl Into<String>) -> Result<Self, ControllerError> {
        let instance_id = instance_id.into();
        if instance_id.is_empty() {
            return Err(ControllerError::InvalidMessage);
        }
        Ok(Self {
            instance_id,
            local_node: None,
        })
    }

    pub fn with_local_node(
        instance_id: impl Into<String>,
        endpoint: impl Into<String>,
        node_instance_id: impl Into<String>,
    ) -> Result<Self, ControllerError> {
        let mut session = Self::new(instance_id)?;
        let endpoint = endpoint.into();
        let node_instance_id = node_instance_id.into();
        if endpoint.is_empty() || node_instance_id.is_empty() {
            return Err(ControllerError::InvalidMessage);
        }
        session.local_node = Some(LocalNodeRoute {
            endpoint,
            instance_id: node_instance_id,
        });
        Ok(session)
    }

    pub fn accept_hello(&self, hello: HelloClient) -> Result<HelloServer, ControllerError> {
        if hello.protocol_version() != pinvou_protocol::IPC_VERSION {
            return Err(ControllerError::ProtocolMismatch);
        }
        HelloServer::new(self.instance_id.clone()).map_err(|_| ControllerError::InvalidMessage)
    }

    pub fn handle(&self, request: IpcMessage) -> Result<IpcMessage, ControllerError> {
        let mut responses = self.handle_many(request)?;
        if responses.len() != 1 {
            return Err(ControllerError::UnsupportedRequest);
        }
        Ok(responses.remove(0))
    }

    pub fn handle_many(&self, request: IpcMessage) -> Result<Vec<IpcMessage>, ControllerError> {
        if request.kind() != IpcMessageKind::Req {
            return Err(ControllerError::UnsupportedRequest);
        }
        let id = request
            .id()
            .cloned()
            .ok_or(ControllerError::InvalidMessage)?;
        match request.method() {
            Some("health") => Ok(vec![
                IpcMessage::response(
                    id,
                    json!({
                        "status": "ok",
                        "instance_id": self.instance_id,
                        "protocol_version": pinvou_protocol::IPC_VERSION
                    }),
                )
                .map_err(|_| ControllerError::InvalidMessage)?,
            ]),
            Some("runtime.echo") | Some("chat.start") => {
                let is_chat_start = request.method() == Some("chat.start");
                let route = self
                    .local_node
                    .as_ref()
                    .ok_or(ControllerError::UnsupportedRequest)?;
                let text_field = if is_chat_start { "prompt" } else { "text" };
                let text = request
                    .payload()
                    .get(text_field)
                    .and_then(|v| v.as_str())
                    .filter(|value| !value.is_empty())
                    .ok_or(ControllerError::InvalidMessage)?;
                let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
                let envelope = client.echo(text)?;
                let mut responses = vec![
                    IpcMessage::event(
                        "runtime.event",
                        serde_json::to_value(&envelope)
                            .map_err(|_| ControllerError::InvalidMessage)?,
                    )
                    .map_err(|_| ControllerError::InvalidMessage)?,
                ];
                if is_chat_start {
                    let terminal = turn_ended_after(&envelope)?;
                    responses.push(
                        IpcMessage::event(
                            "runtime.event",
                            serde_json::to_value(terminal)
                                .map_err(|_| ControllerError::InvalidMessage)?,
                        )
                        .map_err(|_| ControllerError::InvalidMessage)?,
                    );
                }
                Ok(responses)
            }
            Some("runtime.detect") => Ok(vec![IpcMessage::response(
                id,
                json!({
                    "status": if self.local_node.is_some() { "available" } else { "unavailable" },
                    "runtime": "local-node",
                    "protocol_version": pinvou_protocol::IPC_VERSION
                }),
            )
            .map_err(|_| ControllerError::InvalidMessage)?]),
            _ => Err(ControllerError::UnsupportedRequest),
        }
    }

    pub fn handle_bound(&self, request: IpcMessage) -> Result<IpcMessage, ControllerError> {
        let mut responses = self.handle_bound_many(request)?;
        if responses.len() != 1 {
            return Err(ControllerError::UnsupportedRequest);
        }
        Ok(responses.remove(0))
    }

    pub fn handle_bound_many(
        &self,
        request: IpcMessage,
    ) -> Result<Vec<IpcMessage>, ControllerError> {
        if request
            .payload()
            .get("instance_id")
            .and_then(|value| value.as_str())
            != Some(&self.instance_id)
        {
            return Err(ControllerError::ProtocolMismatch);
        }
        self.handle_many(request)
    }
}

fn turn_ended_after(
    envelope: &RuntimeEventEnvelope,
) -> Result<RuntimeEventEnvelope, ControllerError> {
    let value = json!({
        "protocol_version": envelope.protocol_version(),
        "schema_version": envelope.schema_version(),
        "node_id": envelope.node_id(),
        "logical_session_id": envelope.logical_session_id(),
        "attachment_id": envelope.attachment_id(),
        "work_id": null,
        "collaborative_run_id": null,
        "stream_id": "control",
        "turn_id": envelope.turn_id(),
        "seq": 1,
        "source_span": null,
        "timestamp": envelope.timestamp(),
        "rate_class": "R0",
        "kind": "turn.ended",
        "payload": {"end_reason": "completed", "error": null}
    });
    RuntimeEventEnvelope::from_value(value).map_err(|_| ControllerError::InvalidMessage)
}
