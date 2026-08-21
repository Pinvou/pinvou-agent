use std::sync::{Arc, Mutex};

use pinvou_node::{
    AdapterRuntimeHost, NodeError, NodeInstanceLock, NodeRuntimeHost, NodeSession,
    NodeTransportPolicy,
};
use pinvou_protocol::{
    HelloClient, IpcMessage, IpcMessageKind, RuntimeEventEnvelope, StableExitCode,
};
use pinvou_runtime_api::{
    AdapterError, AgentRuntimeAdapter, AuthStatus, RuntimeCapabilities, RuntimeCommand,
    RuntimeEventSubscription, RuntimeOperation, RuntimeSession,
};

#[test]
fn node_has_local_only_transport_and_no_tcp_surface() {
    let policy = NodeTransportPolicy::stage_one();
    assert!(policy.local_ipc());
    assert!(!policy.tcp());
    assert!(!policy.discovery());
    assert!(!policy.has_port());
}

#[test]
fn node_hello_health_and_echo_are_instance_bound() {
    let session = NodeSession::new("node-instance").unwrap();
    let hello = session
        .accept_hello(HelloClient::new(serde_json::json!({"controller": "test"})).unwrap())
        .unwrap();
    assert_eq!(hello.instance_id(), "node-instance");
    let health = IpcMessage::request(
        serde_json::json!(1),
        "health",
        serde_json::json!({"instance_id": "node-instance"}),
    )
    .unwrap();
    assert_eq!(session.handle(health).unwrap().payload()["status"], "ok");
    let echo = IpcMessage::request(
        serde_json::json!(2),
        "runtime.echo",
        serde_json::json!({"instance_id": "node-instance", "text": "M1"}),
    )
    .unwrap();
    let event = session.handle(echo).unwrap();
    assert_eq!(event.kind(), IpcMessageKind::Evt);
    assert_eq!(event.topic(), Some("runtime.event"));
    let envelope = RuntimeEventEnvelope::from_value(event.payload().clone()).unwrap();
    assert_eq!(envelope.kind(), "text.delta");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(envelope.payload().get()).unwrap()["content"],
        "M1"
    );
}

#[test]
fn node_runtime_control_lists_detects_and_switches_runtime_profiles() {
    let session = NodeSession::new("node-instance").unwrap();
    let list = IpcMessage::request(
        serde_json::json!(10),
        "runtime.list",
        serde_json::json!({"instance_id": "node-instance"}),
    )
    .unwrap();
    let response = session.handle(list).unwrap();
    assert_eq!(response.payload()["current"], "echo");
    assert_eq!(response.payload()["runtimes"][0]["id"], "echo");
    assert_eq!(response.payload()["runtimes"][0]["available"], true);

    let switch = IpcMessage::request(
        serde_json::json!(11),
        "runtime.switch",
        serde_json::json!({"instance_id": "node-instance", "runtime": "echo"}),
    )
    .unwrap();
    let response = session.handle(switch).unwrap();
    assert_eq!(response.payload()["status"], "ok");
    assert_eq!(response.payload()["runtime"], "echo");

    let detect = IpcMessage::request(
        serde_json::json!(12),
        "runtime.detect",
        serde_json::json!({"instance_id": "node-instance"}),
    )
    .unwrap();
    let response = session.handle(detect).unwrap();
    assert_eq!(response.payload()["status"], "available");
    assert_eq!(response.payload()["runtime"], "echo");

    let unknown = IpcMessage::request(
        serde_json::json!(13),
        "runtime.switch",
        serde_json::json!({"instance_id": "node-instance", "runtime": "missing"}),
    )
    .unwrap();
    assert!(matches!(
        session.handle(unknown),
        Err(NodeError::UnsupportedRequest)
    ));
}

#[test]
fn node_runtime_switch_replaces_the_active_runtime_host() {
    let session = NodeSession::with_runtime("node-instance", Arc::new(PrefixRuntime)).unwrap();
    let before = IpcMessage::request(
        serde_json::json!(14),
        "runtime.echo",
        serde_json::json!({"instance_id":"node-instance", "text":"before"}),
    )
    .unwrap();
    let event = session.handle(before).unwrap();
    let envelope = RuntimeEventEnvelope::from_value(event.payload().clone()).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(envelope.payload().get()).unwrap()["content"],
        "runtime:before"
    );

    let switch = IpcMessage::request(
        serde_json::json!(15),
        "runtime.switch",
        serde_json::json!({"instance_id":"node-instance", "runtime":"echo"}),
    )
    .unwrap();
    assert_eq!(session.handle(switch).unwrap().payload()["runtime"], "echo");

    let after = IpcMessage::request(
        serde_json::json!(16),
        "runtime.echo",
        serde_json::json!({"instance_id":"node-instance", "text":"after"}),
    )
    .unwrap();
    let event = session.handle(after).unwrap();
    let envelope = RuntimeEventEnvelope::from_value(event.payload().clone()).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(envelope.payload().get()).unwrap()["content"],
        "after"
    );
}

#[test]
fn node_runtime_registry_exposes_and_selects_the_codex_profile() {
    let session = NodeSession::new("node-instance").unwrap();
    let list = IpcMessage::request(
        serde_json::json!(17),
        "runtime.list",
        serde_json::json!({"instance_id": "node-instance"}),
    )
    .unwrap();
    let response = session.handle(list).unwrap();
    let runtimes = response.payload()["runtimes"].as_array().unwrap();
    assert!(runtimes.iter().any(|runtime| runtime["id"] == "codex"));

    let switch = IpcMessage::request(
        serde_json::json!(18),
        "runtime.switch",
        serde_json::json!({"instance_id":"node-instance", "runtime":"codex"}),
    )
    .unwrap();
    let response = session.handle(switch).unwrap();
    assert_eq!(response.payload()["status"], "ok");
    assert_eq!(response.payload()["runtime"], "codex");

    let detect = IpcMessage::request(
        serde_json::json!(19),
        "runtime.detect",
        serde_json::json!({"instance_id":"node-instance"}),
    )
    .unwrap();
    let response = session.handle(detect).unwrap();
    assert_eq!(response.payload()["runtime"], "codex");
}

#[test]
fn node_control_surface_is_instance_bound_and_stably_unsupported_until_runtime_attachment() {
    let session = NodeSession::new("node-instance").unwrap();
    for (id, method, payload, echoed_field) in [
        (
            3,
            "approval.resolve",
            serde_json::json!({"instance_id":"node-instance", "approval_id":"approval-1", "accepted":true}),
            "approval_id",
        ),
        (
            4,
            "input.resolve",
            serde_json::json!({"instance_id":"node-instance", "input_id":"input-1", "value":"answer"}),
            "input_id",
        ),
        (
            5,
            "turn.interrupt",
            serde_json::json!({"instance_id":"node-instance", "turn_id":"turn-1"}),
            "turn_id",
        ),
    ] {
        let request = IpcMessage::request(serde_json::json!(id), method, payload).unwrap();
        let response = session.handle(request).unwrap();
        assert_eq!(response.id(), Some(&serde_json::json!(id)));
        assert_eq!(response.payload()["status"], "unsupported");
        assert_eq!(response.payload()["method"], method);
        assert!(response.payload()[echoed_field].is_string());
    }

    let malformed = IpcMessage::request(
        serde_json::json!(6),
        "approval.resolve",
        serde_json::json!({"instance_id":"node-instance", "approval_id":"approval-1"}),
    )
    .unwrap();
    assert!(matches!(
        session.handle(malformed),
        Err(NodeError::InvalidMessage)
    ));
}

#[derive(Debug)]
struct PrefixRuntime;

impl NodeRuntimeHost for PrefixRuntime {
    fn echo(&self, node_id: &str, text: &str, seq: u64) -> Result<RuntimeEventEnvelope, NodeError> {
        RuntimeEventEnvelope::from_value(serde_json::json!({
            "protocol_version":pinvou_protocol::IPC_VERSION,
            "schema_version":1,
            "node_id":node_id,
            "logical_session_id":"runtime-session",
            "attachment_id":"runtime-attachment",
            "work_id":null,
            "collaborative_run_id":null,
            "stream_id":"main",
            "turn_id":"runtime-turn",
            "seq":seq,
            "source_span":{"start":seq,"end":seq},
            "timestamp":"2026-08-21T00:00:00.000Z",
            "rate_class":"R1",
            "kind":"text.delta",
            "payload":{"role":"assistant","content":format!("runtime:{text}"),"merged_count":1}
        }))
        .map_err(|_| NodeError::InvalidMessage)
    }
}

#[test]
fn node_echo_uses_the_injected_runtime_host_seam() {
    let session = NodeSession::with_runtime("node-instance", Arc::new(PrefixRuntime)).unwrap();
    let echo = IpcMessage::request(
        serde_json::json!(7),
        "runtime.echo",
        serde_json::json!({"instance_id":"node-instance", "text":"M2"}),
    )
    .unwrap();
    let event = session.handle(echo).unwrap();
    let envelope = RuntimeEventEnvelope::from_value(event.payload().clone()).unwrap();
    assert_eq!(envelope.node_id(), "node-instance");
    assert_eq!(envelope.seq(), 1);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(envelope.payload().get()).unwrap()["content"],
        "runtime:M2"
    );
}

#[derive(Debug, Default)]
struct RecordingRuntime {
    calls: Mutex<Vec<String>>,
}

impl RecordingRuntime {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl NodeRuntimeHost for RecordingRuntime {
    fn echo(&self, node_id: &str, text: &str, seq: u64) -> Result<RuntimeEventEnvelope, NodeError> {
        PrefixRuntime.echo(node_id, text, seq)
    }

    fn resolve_approval(
        &self,
        approval_id: &str,
        accepted: bool,
    ) -> Result<serde_json::Value, NodeError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("approval:{approval_id}:{accepted}"));
        Ok(serde_json::json!({"status":"ok", "method":"approval.resolve"}))
    }

    fn resolve_input(
        &self,
        input_id: &str,
        value: &serde_json::Value,
    ) -> Result<serde_json::Value, NodeError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("input:{input_id}:{value}"));
        Ok(serde_json::json!({"status":"ok", "method":"input.resolve"}))
    }

    fn interrupt_turn(&self, turn_id: &str) -> Result<serde_json::Value, NodeError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("interrupt:{turn_id}"));
        Ok(serde_json::json!({"status":"ok", "method":"turn.interrupt"}))
    }
}

#[test]
fn node_control_surface_uses_the_injected_runtime_host_seam() {
    let runtime = Arc::new(RecordingRuntime::default());
    let session = NodeSession::with_runtime("node-instance", runtime.clone()).unwrap();

    for (id, method, payload) in [
        (
            8,
            "approval.resolve",
            serde_json::json!({"instance_id":"node-instance", "approval_id":"approval-2", "accepted":false}),
        ),
        (
            9,
            "input.resolve",
            serde_json::json!({"instance_id":"node-instance", "input_id":"input-2", "value":"answer"}),
        ),
        (
            10,
            "turn.interrupt",
            serde_json::json!({"instance_id":"node-instance", "turn_id":"turn-2"}),
        ),
    ] {
        let request = IpcMessage::request(serde_json::json!(id), method, payload).unwrap();
        let response = session.handle(request).unwrap();
        assert_eq!(response.id(), Some(&serde_json::json!(id)));
        assert_eq!(response.payload()["status"], "ok");
        assert_eq!(response.payload()["method"], method);
    }

    assert_eq!(
        runtime.calls(),
        [
            "approval:approval-2:false",
            "input:input-2:\"answer\"",
            "interrupt:turn-2",
        ]
    );
}

#[derive(Debug, Default)]
struct FakeAdapter {
    calls: Arc<Mutex<Vec<String>>>,
}

impl FakeAdapter {
    fn calls(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.calls)
    }
}

impl AgentRuntimeAdapter for FakeAdapter {
    fn probe(&mut self) -> Result<(), AdapterError> {
        self.calls.lock().unwrap().push("probe".into());
        Ok(())
    }

    fn capabilities(&self) -> Result<RuntimeCapabilities, AdapterError> {
        Ok(RuntimeCapabilities {
            interactive_chat: true,
            tool_approval: true,
            elicitation: true,
            ..RuntimeCapabilities::default()
        })
    }

    fn auth_status(&mut self) -> Result<AuthStatus, AdapterError> {
        Ok(AuthStatus::NotRequired)
    }

    fn create(&mut self, operation: RuntimeOperation) -> Result<RuntimeSession, AdapterError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("create:{}", operation.operation_id));
        RuntimeSession::new("adapter-session")
    }

    fn send(
        &mut self,
        session: &RuntimeSession,
        command: RuntimeCommand,
    ) -> Result<(), AdapterError> {
        self.calls.lock().unwrap().push(format!(
            "send:{}:{}",
            session.as_str(),
            command.payload.as_str().unwrap()
        ));
        Ok(())
    }

    fn approve(
        &mut self,
        session: &RuntimeSession,
        operation: RuntimeOperation,
    ) -> Result<(), AdapterError> {
        self.calls.lock().unwrap().push(format!(
            "approve:{}:{}:{}",
            session.as_str(),
            operation.operation_id,
            operation.options["decision"].as_str().unwrap()
        ));
        Ok(())
    }

    fn respond_input(
        &mut self,
        session: &RuntimeSession,
        operation: RuntimeOperation,
    ) -> Result<(), AdapterError> {
        self.calls.lock().unwrap().push(format!(
            "input:{}:{}:{}",
            session.as_str(),
            operation.operation_id,
            operation.options["value"].as_str().unwrap()
        ));
        Ok(())
    }

    fn interrupt(&mut self, session: &RuntimeSession) -> Result<(), AdapterError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("interrupt:{}", session.as_str()));
        Ok(())
    }

    fn subscribe_events(
        &mut self,
        _: &RuntimeSession,
    ) -> Result<RuntimeEventSubscription, AdapterError> {
        self.calls.lock().unwrap().push("subscribe".into());
        let event = RuntimeEventEnvelope::from_value(serde_json::json!({
            "protocol_version":pinvou_protocol::IPC_VERSION,
            "schema_version":1,
            "node_id":"adapter-node",
            "logical_session_id":"adapter-logical",
            "attachment_id":"adapter-attachment",
            "work_id":null,
            "collaborative_run_id":null,
            "stream_id":"main",
            "turn_id":"adapter-turn",
            "seq":1,
            "source_span":null,
            "timestamp":"2026-08-21T00:00:00.000Z",
            "rate_class":"R1",
            "kind":"text.delta",
            "payload":{"role":"assistant","content":"from-adapter","merged_count":1}
        }))
        .unwrap();
        Ok(Box::new(vec![Ok(event)].into_iter()))
    }

    fn close(&mut self, session: &RuntimeSession) -> Result<(), AdapterError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("close:{}", session.as_str()));
        Ok(())
    }
}

#[test]
fn adapter_runtime_host_drives_probe_create_send_events_and_control_methods() {
    let adapter = FakeAdapter::default();
    let calls = adapter.calls();
    let host = Arc::new(AdapterRuntimeHost::new(Box::new(adapter)));
    let session = NodeSession::with_runtime("node-instance", host.clone()).unwrap();

    let echo = IpcMessage::request(
        serde_json::json!(11),
        "runtime.echo",
        serde_json::json!({"instance_id":"node-instance", "text":"hello-adapter"}),
    )
    .unwrap();
    let event = session.handle(echo).unwrap();
    let envelope = RuntimeEventEnvelope::from_value(event.payload().clone()).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(envelope.payload().get()).unwrap()["content"],
        "from-adapter"
    );

    for (id, method, payload) in [
        (
            12,
            "approval.resolve",
            serde_json::json!({"instance_id":"node-instance", "approval_id":"approval-a", "accepted":true}),
        ),
        (
            13,
            "input.resolve",
            serde_json::json!({"instance_id":"node-instance", "input_id":"input-a", "value":"typed"}),
        ),
        (
            14,
            "turn.interrupt",
            serde_json::json!({"instance_id":"node-instance", "turn_id":"turn-a"}),
        ),
    ] {
        let request = IpcMessage::request(serde_json::json!(id), method, payload).unwrap();
        let response = session.handle(request).unwrap();
        assert_eq!(response.payload()["status"], "ok");
    }

    drop(session);
    drop(host);

    assert_eq!(
        calls.lock().unwrap().as_slice(),
        [
            "probe",
            "create:node-runtime",
            "send:adapter-session:hello-adapter",
            "subscribe",
            "approve:adapter-session:approval-a:accept",
            "input:adapter-session:input-a:typed",
            "interrupt:adapter-session",
            "close:adapter-session",
        ]
    );
}

#[test]
fn node_home_uses_an_os_lock_handle_and_pid_is_diagnostic_only() {
    let path = std::env::temp_dir().join(format!("pinvou-node-lock-{}", std::process::id()));
    let first = NodeInstanceLock::acquire(&path).unwrap();
    assert_eq!(first.diagnostic_pid(), std::process::id());
    assert!(matches!(
        NodeInstanceLock::acquire(&path),
        Err(NodeError::AlreadyRunning)
    ));
    drop(first);
    assert!(NodeInstanceLock::acquire(&path).is_ok());
    let _ = std::fs::remove_file(path);
}

#[cfg(target_os = "linux")]
#[test]
fn node_lock_is_nofollow_regular_owned_0600_and_requires_a_writable_fd() {
    use std::os::{fd::AsRawFd, unix::fs::PermissionsExt};

    let root =
        std::env::temp_dir().join(format!("pinvou-node-private-lock-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("node.lock");
    std::fs::write(&path, b"stale").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let lock = NodeInstanceLock::acquire(&path).unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    assert!(metadata.is_file());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);

    let read_only = std::fs::File::open(&path).unwrap();
    let mut attempted: libc::flock = unsafe { std::mem::zeroed() };
    attempted.l_type = libc::F_WRLCK as libc::c_short;
    attempted.l_whence = libc::SEEK_SET as libc::c_short;
    assert_eq!(
        unsafe { libc::fcntl(read_only.as_raw_fd(), libc::F_OFD_SETLK, &attempted) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::EBADF)
    );

    let target = root.join("target.lock");
    std::fs::write(&target, b"").unwrap();
    let link = root.join("linked.lock");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(NodeInstanceLock::acquire(&link).is_err());
    drop(lock);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn version_and_instance_mismatch_are_stable_fail_closed_errors() {
    assert_eq!(
        NodeError::ProtocolMismatch.exit_code(),
        StableExitCode::ControllerUnavailable
    );
    let session = NodeSession::new("expected").unwrap();
    let request = IpcMessage::request(
        serde_json::json!(1),
        "health",
        serde_json::json!({"instance_id": "stale"}),
    )
    .unwrap();
    assert!(matches!(
        session.handle(request),
        Err(NodeError::ProtocolMismatch)
    ));
}
