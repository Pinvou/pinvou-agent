use std::io::Write;

use pinvou_protocol::{
    HelloClient, HelloServer, IpcMessage, IpcMessageKind, RuntimeEventEnvelope, encode_frame,
    read_frame,
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

    pub fn stream_chat<F>(&mut self, prompt: &str, mut emit: F) -> Result<(), ControllerError>
    where
        F: FnMut(IpcMessage) -> Result<(), ControllerError>,
    {
        if prompt.is_empty() {
            return Err(ControllerError::InvalidMessage);
        }
        self.send_request("chat.start", serde_json::json!({"prompt":prompt}))?;
        let mut active_turn_id = None;
        loop {
            let message: IpcMessage =
                read_frame(&mut self.stream).map_err(|_| ControllerError::InvalidMessage)?;
            if message.kind() != IpcMessageKind::Evt || message.topic() != Some("runtime.event") {
                return Err(ControllerError::InvalidMessage);
            }
            let envelope = RuntimeEventEnvelope::from_value(message.payload().clone())
                .map_err(|_| ControllerError::InvalidMessage)?;
            if envelope.kind() == "turn.started" {
                let turn_id = envelope.turn_id().ok_or(ControllerError::InvalidMessage)?;
                match &active_turn_id {
                    Some(active) if active != turn_id => {
                        return Err(ControllerError::InvalidMessage);
                    }
                    None => active_turn_id = Some(turn_id.to_owned()),
                    _ => {}
                }
            }
            let terminal = envelope.kind() == "turn.ended";
            if terminal {
                let terminal_turn_id = envelope.turn_id().ok_or(ControllerError::InvalidMessage)?;
                if active_turn_id.as_deref() != Some(terminal_turn_id) {
                    return Err(ControllerError::InvalidMessage);
                }
            }
            emit(message)?;
            if terminal {
                return Ok(());
            }
        }
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

    pub fn runtime_switch_prepare(&mut self, runtime: &str) -> Result<IpcMessage, ControllerError> {
        if runtime.is_empty() {
            return Err(ControllerError::InvalidMessage);
        }
        self.request(
            "runtime.switch.prepare",
            serde_json::json!({"runtime":runtime}),
        )
    }

    pub fn runtime_switch_commit(
        &mut self,
        runtime: &str,
        switch_token: &str,
    ) -> Result<IpcMessage, ControllerError> {
        if runtime.is_empty() || switch_token.is_empty() {
            return Err(ControllerError::InvalidMessage);
        }
        self.request(
            "runtime.switch.commit",
            serde_json::json!({"runtime":runtime, "switch_token":switch_token}),
        )
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
        payload: serde_json::Value,
    ) -> Result<IpcMessage, ControllerError> {
        self.send_request(method, payload)?;
        read_frame(&mut self.stream).map_err(|_| ControllerError::InvalidMessage)
    }

    fn send_request(
        &mut self,
        method: &str,
        mut payload: serde_json::Value,
    ) -> Result<(), ControllerError> {
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
        Ok(())
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

    #[test]
    fn stream_chat_rejects_a_terminal_event_for_a_different_turn() {
        let messages = [
            runtime_message("turn.started", "turn-1", 1),
            runtime_message("turn.ended", "turn-2", 2),
        ];
        let inbound = messages
            .iter()
            .flat_map(|message| encode_frame(message).unwrap())
            .collect();
        let outbound = Arc::new(Mutex::new(Vec::new()));
        let stream = FakeStream {
            inbound: std::io::Cursor::new(inbound),
            outbound,
        };
        let mut client = LocalNodeClient {
            stream: Box::new(stream),
            instance_id: "node-instance".into(),
            next_id: 1,
        };

        assert!(matches!(
            client.stream_chat("hello", |_| Ok(())),
            Err(ControllerError::InvalidMessage)
        ));
    }

    #[test]
    fn stream_chat_rejects_error_wrong_topic_bad_envelope_and_early_eof() {
        let valid_payload = runtime_message("turn.started", "turn-1", 1)
            .payload()
            .clone();
        let cases = [
            Some(IpcMessage::error(None, serde_json::json!({"error":"failed"})).unwrap()),
            Some(IpcMessage::event("other.topic", valid_payload).unwrap()),
            Some(IpcMessage::event("runtime.event", serde_json::json!({})).unwrap()),
            None,
        ];

        for message in cases {
            let inbound = message
                .as_ref()
                .map(|message| encode_frame(message).unwrap())
                .unwrap_or_default();
            let outbound = Arc::new(Mutex::new(Vec::new()));
            let stream = FakeStream {
                inbound: std::io::Cursor::new(inbound),
                outbound,
            };
            let mut client = LocalNodeClient {
                stream: Box::new(stream),
                instance_id: "node-instance".into(),
                next_id: 1,
            };
            let mut emitted = 0;

            assert!(matches!(
                client.stream_chat("hello", |_| {
                    emitted += 1;
                    Ok(())
                }),
                Err(ControllerError::InvalidMessage)
            ));
            assert_eq!(emitted, 0);
        }
    }

    fn runtime_message(kind: &str, turn_id: &str, seq: u64) -> IpcMessage {
        let (rate_class, stream_id, payload) = match kind {
            "turn.started" => (
                "R0",
                "control",
                serde_json::json!({"user_input_ref":"prompt"}),
            ),
            "turn.ended" => (
                "R0",
                "control",
                serde_json::json!({"end_reason":"completed","error":null}),
            ),
            _ => unreachable!(),
        };
        IpcMessage::event(
            "runtime.event",
            serde_json::json!({
                "protocol_version": 1,
                "schema_version": 1,
                "node_id": "node-test",
                "logical_session_id": "session-test",
                "attachment_id": "attachment-test",
                "work_id": null,
                "collaborative_run_id": null,
                "stream_id": stream_id,
                "turn_id": turn_id,
                "seq": seq,
                "source_span": null,
                "timestamp": "2026-08-24T00:00:00.000Z",
                "rate_class": rate_class,
                "kind": kind,
                "payload": payload
            }),
        )
        .unwrap()
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
