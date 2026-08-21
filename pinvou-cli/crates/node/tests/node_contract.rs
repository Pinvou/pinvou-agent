use std::sync::{Arc, Mutex};

use pinvou_node::{NodeError, NodeInstanceLock, NodeRuntimeHost, NodeSession, NodeTransportPolicy};
use pinvou_protocol::{
    HelloClient, IpcMessage, IpcMessageKind, RuntimeEventEnvelope, StableExitCode,
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
