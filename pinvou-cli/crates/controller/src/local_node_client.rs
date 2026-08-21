use std::io::Write;

use pinvou_protocol::{
    HelloClient, HelloServer, IpcMessage, RuntimeEventEnvelope, encode_frame, read_frame,
};

use crate::{
    ControllerError,
    local_node_supervisor::{ReadWrite, connect_endpoint},
};

pub struct LocalNodeClient {
    stream: Box<dyn ReadWrite>,
    instance_id: String,
    next_id: u64,
}

impl LocalNodeClient {
    pub fn connect(endpoint: &str, expected_instance_id: &str) -> Result<Self, ControllerError> {
        let mut stream = connect_endpoint(endpoint)?;
        let hello = HelloClient::new(serde_json::json!({"client":"controller-node-client"}))
            .map_err(|_| ControllerError::InvalidMessage)?;
        stream.write_all(&encode_frame(&hello).map_err(|_| ControllerError::InvalidMessage)?)?;
        let answer: HelloServer =
            read_frame(&mut stream).map_err(|_| ControllerError::ProtocolMismatch)?;
        if answer.protocol_version() != pinvou_protocol::IPC_VERSION
            || answer.instance_id() != expected_instance_id
        {
            return Err(ControllerError::ProtocolMismatch);
        }
        Ok(Self {
            stream,
            instance_id: expected_instance_id.into(),
            next_id: 1,
        })
    }

    pub fn health(&mut self) -> Result<(), ControllerError> {
        let response = self.request("health", serde_json::json!({}))?;
        (response.payload()["status"] == "ok")
            .then_some(())
            .ok_or(ControllerError::InvalidMessage)
    }

    pub fn echo(&mut self, text: &str) -> Result<RuntimeEventEnvelope, ControllerError> {
        if text.is_empty() {
            return Err(ControllerError::InvalidMessage);
        }
        let response = self.request("runtime.echo", serde_json::json!({"text":text}))?;
        RuntimeEventEnvelope::from_value(response.payload().clone())
            .map_err(|_| ControllerError::InvalidMessage)
    }

    pub fn runtime_list(&mut self) -> Result<IpcMessage, ControllerError> {
        self.request("runtime.list", serde_json::json!({}))
    }

    pub fn runtime_detect(&mut self) -> Result<IpcMessage, ControllerError> {
        self.request("runtime.detect", serde_json::json!({}))
    }

    pub fn runtime_switch(&mut self, runtime: &str) -> Result<IpcMessage, ControllerError> {
        if runtime.is_empty() {
            return Err(ControllerError::InvalidMessage);
        }
        self.request("runtime.switch", serde_json::json!({"runtime":runtime}))
    }

    pub fn resolve_approval(
        &mut self,
        approval_id: &str,
        accepted: bool,
    ) -> Result<IpcMessage, ControllerError> {
        if approval_id.is_empty() {
            return Err(ControllerError::InvalidMessage);
        }
        self.request(
            "approval.resolve",
            serde_json::json!({"approval_id":approval_id, "accepted":accepted}),
        )
    }

    pub fn resolve_input(
        &mut self,
        input_id: &str,
        value: serde_json::Value,
    ) -> Result<IpcMessage, ControllerError> {
        if input_id.is_empty() {
            return Err(ControllerError::InvalidMessage);
        }
        self.request(
            "input.resolve",
            serde_json::json!({"input_id":input_id, "value":value}),
        )
    }

    pub fn interrupt_turn(&mut self, turn_id: &str) -> Result<IpcMessage, ControllerError> {
        if turn_id.is_empty() {
            return Err(ControllerError::InvalidMessage);
        }
        self.request("turn.interrupt", serde_json::json!({"turn_id":turn_id}))
    }

    fn request(
        &mut self,
        method: &str,
        mut payload: serde_json::Value,
    ) -> Result<IpcMessage, ControllerError> {
        payload
            .as_object_mut()
            .ok_or(ControllerError::InvalidMessage)?
            .insert(
                "instance_id".into(),
                serde_json::Value::String(self.instance_id.clone()),
            );
        let id = self.next_id;
        self.next_id += 1;
        let request = IpcMessage::request(serde_json::json!(id), method, payload)
            .map_err(|_| ControllerError::InvalidMessage)?;
        self.stream
            .write_all(&encode_frame(&request).map_err(|_| ControllerError::InvalidMessage)?)?;
        self.stream.flush()?;
        read_frame(&mut self.stream).map_err(|_| ControllerError::InvalidMessage)
    }
}
