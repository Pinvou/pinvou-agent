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

    pub fn runtime_detect(&mut self, runtime: Option<&str>) -> Result<IpcMessage, ControllerError> {
        let mut payload = serde_json::json!({});
        if let Some(runtime) = runtime {
            if runtime.is_empty() {
                return Err(ControllerError::InvalidMessage);
            }
            payload["runtime"] = serde_json::json!(runtime);
        }
        self.request("runtime.detect", payload)
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

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};

    use pinvou_protocol::{decode_frame, encode_frame};

    use super::*;

    #[test]
    fn runtime_detect_can_forward_a_named_runtime_without_switching() {
        let response = IpcMessage::response(
            serde_json::json!(1),
            serde_json::json!({"runtime":"codex", "status":"available"}),
        )
        .unwrap();
        let outbound = Arc::new(Mutex::new(Vec::new()));
        let stream = FakeStream {
            inbound: std::io::Cursor::new(encode_frame(&response).unwrap()),
            outbound: Arc::clone(&outbound),
        };
        let mut client = LocalNodeClient {
            stream: Box::new(stream),
            instance_id: "node-instance".into(),
            next_id: 1,
        };

        let response = client.runtime_detect(Some("codex")).unwrap();

        assert_eq!(response.payload()["runtime"], "codex");
        let request: IpcMessage = decode_frame(&outbound.lock().unwrap()).unwrap();
        assert_eq!(request.method(), Some("runtime.detect"));
        assert_eq!(request.payload()["instance_id"], "node-instance");
        assert_eq!(request.payload()["runtime"], "codex");
    }

    struct FakeStream {
        inbound: std::io::Cursor<Vec<u8>>,
        outbound: Arc<Mutex<Vec<u8>>>,
    }

    impl Read for FakeStream {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.inbound.read(buffer)
        }
    }

    impl Write for FakeStream {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.outbound.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
