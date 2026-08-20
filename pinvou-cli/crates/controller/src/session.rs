use pinvou_protocol::{HelloClient, HelloServer, IpcMessage, IpcMessageKind};
use serde_json::json;

use crate::ControllerError;

#[derive(Clone, Debug)]
pub struct ControllerSession {
    instance_id: String,
}

impl ControllerSession {
    pub fn new(instance_id: impl Into<String>) -> Result<Self, ControllerError> {
        let instance_id = instance_id.into();
        if instance_id.is_empty() {
            return Err(ControllerError::InvalidMessage);
        }
        Ok(Self { instance_id })
    }

    pub fn accept_hello(&self, hello: HelloClient) -> Result<HelloServer, ControllerError> {
        if hello.protocol_version() != 1 {
            return Err(ControllerError::ProtocolMismatch);
        }
        HelloServer::new(self.instance_id.clone()).map_err(|_| ControllerError::InvalidMessage)
    }

    pub fn handle(&self, request: IpcMessage) -> Result<IpcMessage, ControllerError> {
        if request.kind() != IpcMessageKind::Req || request.method() != Some("health") {
            return Err(ControllerError::UnsupportedRequest);
        }
        let id = request
            .id()
            .cloned()
            .ok_or(ControllerError::InvalidMessage)?;
        IpcMessage::response(
            id,
            json!({
                "status": "ok",
                "instance_id": self.instance_id,
                "protocol_version": 1
            }),
        )
        .map_err(|_| ControllerError::InvalidMessage)
    }

    pub fn handle_bound(&self, request: IpcMessage) -> Result<IpcMessage, ControllerError> {
        if request
            .payload()
            .get("instance_id")
            .and_then(|value| value.as_str())
            != Some(&self.instance_id)
        {
            return Err(ControllerError::ProtocolMismatch);
        }
        self.handle(request)
    }
}
