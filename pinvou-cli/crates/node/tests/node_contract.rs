use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pinvou_node::{
    AdapterRuntimeHost, NodeError, NodeInstanceLock, NodeRuntimeEventStream, NodeRuntimeHost,
    NodeSession, NodeTransportPolicy,
};
use pinvou_protocol::{
    HelloClient, IpcMessage, IpcMessageKind, RuntimeEventEnvelope, StableExitCode,
};
use pinvou_runtime_api::{
    AdapterError, AgentRuntimeAdapter, AuthStatus, RuntimeCapabilities, RuntimeCommand,
    RuntimeEventSubscription, RuntimeOperation, RuntimeSession,
};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn scripted_runtime_events() -> Vec<RuntimeEventEnvelope> {
    [
        (
            "turn.started",
            serde_json::json!({"user_input_ref":"prompt"}),
        ),
        (
            "text.delta",
            serde_json::json!({"role":"assistant","content":"one","merged_count":1}),
        ),
        (
            "text.delta",
            serde_json::json!({"role":"assistant","content":"two","merged_count":1}),
        ),
        (
            "approval.requested",
            serde_json::json!({"approval_id":"approval-1","tool":"shell","summary":"run","options":["approved","denied"],"timeout_ms":30000}),
        ),
        (
            "input.requested",
            serde_json::json!({"input_id":"input-1","prompt":"choose","schema":{"type":"string"}}),
        ),
        (
            "tool.call.started",
            serde_json::json!({"tool_id":"call-1","name":"read","args_json":{}}),
        ),
        (
            "tool.call.completed",
            serde_json::json!({"tool_id":"call-1","result":{},"is_error":false,"exit_code":0}),
        ),
        (
            "turn.ended",
            serde_json::json!({"end_reason":"completed","error":null}),
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (kind, payload))| match kind {
        "turn.started" | "turn.ended" | "approval.requested" | "input.requested" => {
            runtime_event_with_seq(kind, "R0", "control", payload, index as u64 + 1)
        }
        _ => runtime_event_with_seq(kind, "R1", "main", payload, index as u64 + 1),
    })
    .collect()
}

#[derive(Debug)]
struct ScriptedRuntime {
    events: Mutex<Option<Vec<RuntimeEventEnvelope>>>,
}

impl NodeRuntimeHost for ScriptedRuntime {
    fn start_turn(&self, _: &str, _: &str, _: u64) -> Result<NodeRuntimeEventStream, NodeError> {
        Ok(Box::new(
            self.events
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .into_iter()
                .map(Ok),
        ))
    }
}

#[test]
fn node_stream_bound_emits_the_complete_runtime_event_stream_in_order() {
    let events = scripted_runtime_events();
    let session = NodeSession::with_runtime(
        "node-instance",
        Arc::new(ScriptedRuntime {
            events: Mutex::new(Some(events)),
        }),
    )
    .unwrap();
    let request = IpcMessage::request(
        serde_json::json!(101),
        "chat.start",
        serde_json::json!({"instance_id":"node-instance", "prompt":"hello"}),
    )
    .unwrap();
    let mut emitted = Vec::new();

    session
        .stream_bound(request, |event| {
            emitted.push(RuntimeEventEnvelope::from_value(event.payload().clone()).unwrap());
            Ok(())
        })
        .unwrap();

    assert_eq!(
        emitted
            .iter()
            .map(RuntimeEventEnvelope::kind)
            .collect::<Vec<_>>(),
        [
            "turn.started",
            "text.delta",
            "text.delta",
            "approval.requested",
            "input.requested",
            "tool.call.started",
            "tool.call.completed",
            "turn.ended",
        ]
    );
}

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
fn node_runtime_switch_persists_the_selected_runtime_for_restart() {
    let state_file = temp_state_file("runtime-selection");
    let session = NodeSession::with_state_file("node-instance", state_file.clone()).unwrap();
    let switch = IpcMessage::request(
        serde_json::json!(31),
        "runtime.switch",
        serde_json::json!({"instance_id":"node-instance", "runtime":"echo"}),
    )
    .unwrap();

    assert_eq!(session.handle(switch).unwrap().payload()["runtime"], "echo");
    assert!(
        std::fs::read_to_string(&state_file)
            .unwrap()
            .contains("\"runtime\":\"echo\"")
    );

    let restarted = NodeSession::with_state_file("node-instance", state_file.clone()).unwrap();
    let list = IpcMessage::request(
        serde_json::json!(32),
        "runtime.list",
        serde_json::json!({"instance_id":"node-instance"}),
    )
    .unwrap();
    assert_eq!(restarted.handle(list).unwrap().payload()["current"], "echo");

    std::fs::remove_file(state_file).unwrap();
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
fn node_rejects_runtime_switch_while_a_turn_is_active() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let session = NodeSession::with_runtime(
        "node-instance",
        Arc::new(BlockingRuntime {
            entered: Mutex::new(Some(entered_tx)),
            release: Mutex::new(release_rx),
        }),
    )
    .unwrap();
    let running = session.clone();
    let worker = std::thread::spawn(move || {
        let echo = IpcMessage::request(
            serde_json::json!(41),
            "runtime.echo",
            serde_json::json!({"instance_id":"node-instance", "text":"busy"}),
        )
        .unwrap();
        running.handle(echo).unwrap()
    });
    entered_rx.recv().unwrap();

    let switch_while_busy = IpcMessage::request(
        serde_json::json!(42),
        "runtime.switch",
        serde_json::json!({"instance_id":"node-instance", "runtime":"echo"}),
    )
    .unwrap();
    assert!(matches!(
        session.handle(switch_while_busy),
        Err(NodeError::RuntimeBusy)
    ));

    release_tx.send(()).unwrap();
    let event = worker.join().unwrap();
    assert_eq!(event.kind(), IpcMessageKind::Evt);

    let switch_after_turn = IpcMessage::request(
        serde_json::json!(43),
        "runtime.switch",
        serde_json::json!({"instance_id":"node-instance", "runtime":"echo"}),
    )
    .unwrap();
    assert_eq!(
        session.handle(switch_after_turn).unwrap().payload()["runtime"],
        "echo"
    );
}

#[test]
fn node_runtime_switch_prepare_and_commit_are_token_bound() {
    let session = NodeSession::new("node-instance").unwrap();
    let prepare = IpcMessage::request(
        serde_json::json!(44),
        "runtime.switch.prepare",
        serde_json::json!({"instance_id":"node-instance", "runtime":"echo"}),
    )
    .unwrap();

    let prepared = session.handle(prepare).unwrap();
    assert_eq!(prepared.payload()["status"], "ready");
    assert_eq!(prepared.payload()["runtime"], "echo");
    assert_eq!(prepared.payload()["current_runtime"], "echo");
    assert_eq!(prepared.payload()["requires_compression"], false);
    assert_eq!(prepared.payload()["context"]["strategy"], "none");
    assert_eq!(
        prepared.payload()["tools"]["policy"],
        "portable_or_replay_only"
    );
    let token = prepared.payload()["switch_token"].as_str().unwrap();
    assert!(!token.is_empty());

    let stale_commit = IpcMessage::request(
        serde_json::json!(45),
        "runtime.switch.commit",
        serde_json::json!({
            "instance_id":"node-instance",
            "runtime":"codex",
            "switch_token": token
        }),
    )
    .unwrap();
    assert!(matches!(
        session.handle(stale_commit),
        Err(NodeError::InvalidMessage)
    ));

    let commit = IpcMessage::request(
        serde_json::json!(46),
        "runtime.switch.commit",
        serde_json::json!({
            "instance_id":"node-instance",
            "runtime":"echo",
            "switch_token": token
        }),
    )
    .unwrap();
    let committed = session.handle(commit).unwrap();
    assert_eq!(committed.payload()["status"], "ok");
    assert_eq!(committed.payload()["runtime"], "echo");
    assert_eq!(committed.payload()["switch_token"], token);

    let duplicate_commit = IpcMessage::request(
        serde_json::json!(47),
        "runtime.switch.commit",
        serde_json::json!({
            "instance_id":"node-instance",
            "runtime":"echo",
            "switch_token": token
        }),
    )
    .unwrap();
    assert!(matches!(
        session.handle(duplicate_commit),
        Err(NodeError::InvalidMessage)
    ));
}

fn temp_state_file(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "pinvou-node-{name}-{}-{unique}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
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
    fn start_turn(
        &self,
        node_id: &str,
        prompt: &str,
        seq: u64,
    ) -> Result<NodeRuntimeEventStream, NodeError> {
        let event = RuntimeEventEnvelope::from_value(serde_json::json!({
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
            "payload":{"role":"assistant","content":format!("runtime:{prompt}"),"merged_count":1}
        }))
        .map_err(|_| NodeError::InvalidMessage)?;
        Ok(Box::new(std::iter::once(Ok(event))))
    }
}

#[derive(Debug)]
struct BlockingRuntime {
    entered: Mutex<Option<mpsc::Sender<()>>>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl NodeRuntimeHost for BlockingRuntime {
    fn start_turn(
        &self,
        node_id: &str,
        prompt: &str,
        seq: u64,
    ) -> Result<NodeRuntimeEventStream, NodeError> {
        if let Some(sender) = self.entered.lock().unwrap().take() {
            sender.send(()).unwrap();
        }
        self.release.lock().unwrap().recv().unwrap();
        PrefixRuntime.start_turn(node_id, prompt, seq)
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
    fn start_turn(
        &self,
        node_id: &str,
        prompt: &str,
        seq: u64,
    ) -> Result<NodeRuntimeEventStream, NodeError> {
        PrefixRuntime.start_turn(node_id, prompt, seq)
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
    events: Vec<RuntimeEventEnvelope>,
    probe_error: Option<AdapterError>,
}

impl FakeAdapter {
    fn calls(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.calls)
    }

    fn with_events(events: Vec<RuntimeEventEnvelope>) -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            events,
            probe_error: None,
        }
    }

    fn with_probe_error(probe_error: AdapterError) -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            events: Vec::new(),
            probe_error: Some(probe_error),
        }
    }
}

impl AgentRuntimeAdapter for FakeAdapter {
    fn probe(&mut self) -> Result<(), AdapterError> {
        self.calls.lock().unwrap().push("probe".into());
        if let Some(error) = self.probe_error.clone() {
            return Err(error);
        }
        Ok(())
    }

    fn capabilities(&self) -> Result<RuntimeCapabilities, AdapterError> {
        self.calls.lock().unwrap().push("capabilities".into());
        Ok(RuntimeCapabilities {
            interactive_chat: true,
            tool_approval: true,
            elicitation: true,
            ..RuntimeCapabilities::default()
        })
    }

    fn auth_status(&mut self) -> Result<AuthStatus, AdapterError> {
        self.calls.lock().unwrap().push("auth_status".into());
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
        let events = if self.events.is_empty() {
            vec![runtime_event(
                "text.delta",
                "R1",
                "main",
                serde_json::json!({"role":"assistant","content":"from-adapter","merged_count":1}),
            )]
        } else {
            std::mem::take(&mut self.events)
        };
        Ok(Box::new(events.into_iter().map(Ok)))
    }

    fn close(&mut self, session: &RuntimeSession) -> Result<(), AdapterError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("close:{}", session.as_str()));
        Ok(())
    }
}

#[derive(Debug)]
struct LongLivedAdapter {
    receiver: Option<mpsc::Receiver<Result<RuntimeEventEnvelope, AdapterError>>>,
    keepalive: Arc<Mutex<Option<mpsc::Sender<Result<RuntimeEventEnvelope, AdapterError>>>>>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl AgentRuntimeAdapter for LongLivedAdapter {
    fn probe(&mut self) -> Result<(), AdapterError> {
        Ok(())
    }

    fn capabilities(&self) -> Result<RuntimeCapabilities, AdapterError> {
        Ok(RuntimeCapabilities::default())
    }

    fn auth_status(&mut self) -> Result<AuthStatus, AdapterError> {
        Ok(AuthStatus::NotRequired)
    }

    fn create(&mut self, _: RuntimeOperation) -> Result<RuntimeSession, AdapterError> {
        RuntimeSession::new("long-lived-session")
    }

    fn send(&mut self, _: &RuntimeSession, command: RuntimeCommand) -> Result<(), AdapterError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("send:{}", command.payload.as_str().unwrap()));
        Ok(())
    }

    fn subscribe_events(
        &mut self,
        _: &RuntimeSession,
    ) -> Result<RuntimeEventSubscription, AdapterError> {
        self.calls.lock().unwrap().push("subscribe".into());
        Ok(Box::new(self.receiver.take().unwrap().into_iter()))
    }

    fn close(&mut self, _: &RuntimeSession) -> Result<(), AdapterError> {
        self.keepalive.lock().unwrap().take();
        Ok(())
    }
}

#[test]
fn adapter_stream_stops_at_turn_end_and_reuses_the_long_lived_subscription() {
    let (event_tx, event_rx) = mpsc::channel();
    for event in [
        runtime_event_for_turn(
            "turn.started",
            "R0",
            "control",
            serde_json::json!({"user_input_ref":"one"}),
            "turn-one",
            1,
        ),
        runtime_event_for_turn(
            "approval.requested",
            "R0",
            "control",
            serde_json::json!({"approval_id":"approval-1","tool":"shell","summary":"run","options":["approved","denied"],"timeout_ms":30000}),
            "turn-one",
            2,
        ),
        runtime_event_for_turn(
            "input.requested",
            "R0",
            "control",
            serde_json::json!({"input_id":"input-1","prompt":"choose","schema":{"type":"string"}}),
            "turn-one",
            3,
        ),
        runtime_event_for_turn(
            "turn.ended",
            "R0",
            "control",
            serde_json::json!({"end_reason":"completed","error":null}),
            "other-turn",
            4,
        ),
        runtime_event_for_turn(
            "turn.ended",
            "R0",
            "control",
            serde_json::json!({"end_reason":"completed","error":null}),
            "turn-one",
            5,
        ),
        runtime_event_for_turn(
            "turn.started",
            "R0",
            "control",
            serde_json::json!({"user_input_ref":"two"}),
            "turn-two",
            6,
        ),
        runtime_event_for_turn(
            "text.delta",
            "R1",
            "main",
            serde_json::json!({"role":"assistant","content":"second","merged_count":1}),
            "turn-two",
            7,
        ),
        runtime_event_for_turn(
            "turn.ended",
            "R0",
            "control",
            serde_json::json!({"end_reason":"completed","error":null}),
            "turn-two",
            8,
        ),
    ] {
        event_tx.send(Ok(event)).unwrap();
    }
    let keepalive = Arc::new(Mutex::new(Some(event_tx)));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let session = NodeSession::with_runtime(
        "node-instance",
        Arc::new(AdapterRuntimeHost::new(Box::new(LongLivedAdapter {
            receiver: Some(event_rx),
            keepalive: keepalive.clone(),
            calls: calls.clone(),
        }))),
    )
    .unwrap();

    let first_session = session.clone();
    let (first_tx, first_rx) = mpsc::channel();
    let first_worker = std::thread::spawn(move || {
        let request = IpcMessage::request(
            serde_json::json!(110),
            "chat.start",
            serde_json::json!({"instance_id":"node-instance","prompt":"one"}),
        )
        .unwrap();
        let mut kinds = Vec::new();
        let result = first_session.stream_bound(request, |message| {
            kinds.push(
                RuntimeEventEnvelope::from_value(message.payload().clone())
                    .unwrap()
                    .kind()
                    .to_owned(),
            );
            Ok(())
        });
        first_tx.send((result, kinds)).unwrap();
    });
    let first = first_rx.recv_timeout(Duration::from_secs(2));
    if first.is_err() {
        keepalive.lock().unwrap().take();
    }
    first_worker.join().unwrap();
    let (first_result, first_kinds) = first.expect("first turn waited for subscription EOF");
    first_result.unwrap();
    assert_eq!(
        first_kinds,
        [
            "turn.started",
            "approval.requested",
            "input.requested",
            "turn.ended",
            "turn.ended"
        ]
    );

    let second = IpcMessage::request(
        serde_json::json!(111),
        "chat.start",
        serde_json::json!({"instance_id":"node-instance","prompt":"two"}),
    )
    .unwrap();
    let mut second_kinds = Vec::new();
    session
        .stream_bound(second, |message| {
            second_kinds.push(
                RuntimeEventEnvelope::from_value(message.payload().clone())
                    .unwrap()
                    .kind()
                    .to_owned(),
            );
            Ok(())
        })
        .unwrap();
    keepalive.lock().unwrap().take();
    assert_eq!(second_kinds, ["turn.started", "text.delta", "turn.ended"]);
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        ["send:one", "subscribe", "send:two"]
    );
}

#[test]
fn adapter_stream_reports_eof_before_turn_end() {
    let adapter = FakeAdapter::with_events(vec![runtime_event(
        "turn.started",
        "R0",
        "control",
        serde_json::json!({"user_input_ref":"truncated"}),
    )]);
    let session = NodeSession::with_runtime(
        "node-instance",
        Arc::new(AdapterRuntimeHost::new(Box::new(adapter))),
    )
    .unwrap();
    let request = IpcMessage::request(
        serde_json::json!(112),
        "chat.start",
        serde_json::json!({"instance_id":"node-instance","prompt":"truncated"}),
    )
    .unwrap();

    assert!(matches!(
        session.stream_bound(request, |_| Ok(())),
        Err(NodeError::InvalidMessage)
    ));
}

#[derive(Debug)]
struct ApprovalBlockingAdapter {
    approval_seen: Mutex<Option<mpsc::Sender<()>>>,
    release_stream: Mutex<Option<mpsc::Receiver<()>>>,
}

impl AgentRuntimeAdapter for ApprovalBlockingAdapter {
    fn probe(&mut self) -> Result<(), AdapterError> {
        Ok(())
    }

    fn capabilities(&self) -> Result<RuntimeCapabilities, AdapterError> {
        Ok(RuntimeCapabilities::default())
    }

    fn auth_status(&mut self) -> Result<AuthStatus, AdapterError> {
        Ok(AuthStatus::NotRequired)
    }

    fn create(&mut self, _: RuntimeOperation) -> Result<RuntimeSession, AdapterError> {
        RuntimeSession::new("approval-session")
    }

    fn send(&mut self, _: &RuntimeSession, _: RuntimeCommand) -> Result<(), AdapterError> {
        Ok(())
    }

    fn approve(&mut self, _: &RuntimeSession, _: RuntimeOperation) -> Result<(), AdapterError> {
        Ok(())
    }

    fn subscribe_events(
        &mut self,
        _: &RuntimeSession,
    ) -> Result<RuntimeEventSubscription, AdapterError> {
        let approval_seen = self.approval_seen.lock().unwrap().take().unwrap();
        let release_stream = self.release_stream.lock().unwrap().take().unwrap();
        let mut step = 0;
        Ok(Box::new(std::iter::from_fn(move || {
            let event = match step {
                0 => runtime_event(
                    "turn.started",
                    "R0",
                    "control",
                    serde_json::json!({"user_input_ref":"approval"}),
                ),
                1 => {
                    approval_seen.send(()).unwrap();
                    runtime_event(
                        "approval.requested",
                        "R0",
                        "control",
                        serde_json::json!({
                            "approval_id":"approval-1",
                            "tool":"shell",
                            "summary":"run command",
                            "options":["approved","denied"],
                            "timeout_ms":30_000
                        }),
                    )
                }
                2 => {
                    release_stream.recv().unwrap();
                    runtime_event(
                        "turn.ended",
                        "R0",
                        "control",
                        serde_json::json!({"end_reason":"completed","error":null}),
                    )
                }
                _ => return None,
            };
            step += 1;
            Some(Ok(event))
        })))
    }

    fn close(&mut self, _: &RuntimeSession) -> Result<(), AdapterError> {
        Ok(())
    }
}

#[test]
fn adapter_stream_releases_mutex_while_waiting_for_approval_resolution() {
    let (approval_seen_tx, approval_seen_rx) = mpsc::channel();
    let (release_stream_tx, release_stream_rx) = mpsc::channel();
    let adapter = ApprovalBlockingAdapter {
        approval_seen: Mutex::new(Some(approval_seen_tx)),
        release_stream: Mutex::new(Some(release_stream_rx)),
    };
    let session = NodeSession::with_runtime(
        "node-instance",
        Arc::new(AdapterRuntimeHost::new(Box::new(adapter))),
    )
    .unwrap();
    let streaming = session.clone();
    let stream_worker = std::thread::spawn(move || {
        let request = IpcMessage::request(
            serde_json::json!(102),
            "chat.start",
            serde_json::json!({"instance_id":"node-instance", "prompt":"approve"}),
        )
        .unwrap();
        streaming.stream_bound(request, |_| Ok(())).unwrap();
    });
    approval_seen_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("runtime stream did not reach approval.requested");

    let resolving = session.clone();
    let (resolved_tx, resolved_rx) = mpsc::channel();
    let resolve_worker = std::thread::spawn(move || {
        let request = IpcMessage::request(
            serde_json::json!(103),
            "approval.resolve",
            serde_json::json!({
                "instance_id":"node-instance",
                "approval_id":"approval-1",
                "accepted":true
            }),
        )
        .unwrap();
        resolved_tx.send(resolving.handle(request)).unwrap();
    });
    let resolved_while_streaming = resolved_rx.recv_timeout(Duration::from_secs(2));

    release_stream_tx.send(()).unwrap();
    stream_worker.join().unwrap();
    resolve_worker.join().unwrap();
    resolved_while_streaming
        .expect("approval.resolve was blocked by the adapter stream mutex")
        .unwrap();
}

fn runtime_event(
    kind: &str,
    rate_class: &str,
    stream_id: &str,
    payload: serde_json::Value,
) -> RuntimeEventEnvelope {
    runtime_event_with_seq(kind, rate_class, stream_id, payload, 1)
}

fn runtime_event_with_seq(
    kind: &str,
    rate_class: &str,
    stream_id: &str,
    payload: serde_json::Value,
    seq: u64,
) -> RuntimeEventEnvelope {
    runtime_event_for_turn(kind, rate_class, stream_id, payload, "adapter-turn", seq)
}

fn runtime_event_for_turn(
    kind: &str,
    rate_class: &str,
    stream_id: &str,
    payload: serde_json::Value,
    turn_id: &str,
    seq: u64,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope::from_value(serde_json::json!({
        "protocol_version":pinvou_protocol::IPC_VERSION,
        "schema_version":1,
        "node_id":"adapter-node",
        "logical_session_id":"adapter-logical",
        "attachment_id":"adapter-attachment",
        "work_id":null,
        "collaborative_run_id":null,
        "stream_id":stream_id,
        "turn_id":turn_id,
        "seq":seq,
        "source_span":null,
        "timestamp":"2026-08-21T00:00:00.000Z",
        "rate_class":rate_class,
        "kind":kind,
        "payload":payload
    }))
    .unwrap()
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
fn runtime_detect_probes_adapter_without_creating_a_chat_session() {
    let adapter = FakeAdapter::default();
    let calls = adapter.calls();
    let session = NodeSession::with_runtime(
        "node-instance",
        Arc::new(AdapterRuntimeHost::new(Box::new(adapter))),
    )
    .unwrap();
    let detect = IpcMessage::request(
        serde_json::json!(15),
        "runtime.detect",
        serde_json::json!({"instance_id":"node-instance"}),
    )
    .unwrap();

    let response = session.handle(detect).unwrap();
    assert_eq!(response.payload()["status"], "available");
    assert_eq!(response.payload()["runtime"], "custom");
    assert_eq!(response.payload()["auth_status"], "not_required");
    assert_eq!(response.payload()["capabilities"]["interactive_chat"], true);
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        ["probe", "capabilities", "auth_status"]
    );
}

#[test]
fn runtime_detect_can_probe_a_named_runtime_without_switching_the_active_runtime() {
    let session = NodeSession::with_runtime("node-instance", Arc::new(PrefixRuntime)).unwrap();
    let detect = IpcMessage::request(
        serde_json::json!(34),
        "runtime.detect",
        serde_json::json!({"instance_id":"node-instance", "runtime":"echo"}),
    )
    .unwrap();

    let response = session.handle(detect).unwrap();
    assert_eq!(response.payload()["runtime"], "echo");
    assert_eq!(response.payload()["status"], "available");

    let echo = IpcMessage::request(
        serde_json::json!(35),
        "runtime.echo",
        serde_json::json!({"instance_id":"node-instance", "text":"still-active"}),
    )
    .unwrap();
    let event = session.handle(echo).unwrap();
    let envelope = RuntimeEventEnvelope::from_value(event.payload().clone()).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(envelope.payload().get()).unwrap()["content"],
        "runtime:still-active"
    );
}

#[test]
fn runtime_detect_reports_adapter_probe_failures_as_status_payloads() {
    let adapter = FakeAdapter::with_probe_error(AdapterError::BlockedAuth);
    let calls = adapter.calls();
    let session = NodeSession::with_runtime(
        "node-instance",
        Arc::new(AdapterRuntimeHost::new(Box::new(adapter))),
    )
    .unwrap();
    let detect = IpcMessage::request(
        serde_json::json!(16),
        "runtime.detect",
        serde_json::json!({"instance_id":"node-instance"}),
    )
    .unwrap();

    let response = session.handle(detect).unwrap();
    assert_eq!(response.payload()["status"], "blocked_auth");
    assert_eq!(response.payload()["runtime"], "custom");
    assert_eq!(response.payload()["error_kind"], "blocked_auth");
    assert_eq!(
        response.payload()["exit_code"],
        StableExitCode::BlockedAuth.as_i32()
    );
    assert_eq!(calls.lock().unwrap().as_slice(), ["probe"]);
}

#[test]
fn adapter_runtime_host_skips_lifecycle_events_until_text_delta() {
    let adapter = FakeAdapter::with_events(vec![
        runtime_event(
            "attachment.started",
            "R0",
            "control",
            serde_json::json!({"runtime_id":"codex","agent_kind":"codex","capabilities_snapshot":{}}),
        ),
        runtime_event(
            "turn.started",
            "R0",
            "control",
            serde_json::json!({"user_input_ref":"codex"}),
        ),
        runtime_event(
            "text.delta",
            "R1",
            "main",
            serde_json::json!({"role":"assistant","content":"visible codex text","merged_count":1}),
        ),
    ]);
    let session = NodeSession::with_runtime(
        "node-instance",
        Arc::new(AdapterRuntimeHost::new(Box::new(adapter))),
    )
    .unwrap();

    let echo = IpcMessage::request(
        serde_json::json!(15),
        "runtime.echo",
        serde_json::json!({"instance_id":"node-instance", "text":"hello-codex"}),
    )
    .unwrap();
    let event = session.handle(echo).unwrap();
    let envelope = RuntimeEventEnvelope::from_value(event.payload().clone()).unwrap();
    assert_eq!(envelope.kind(), "text.delta");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(envelope.payload().get()).unwrap()["content"],
        "visible codex text"
    );
}

#[cfg(windows)]
#[test]
fn switching_to_codex_uses_the_adapter_runtime_and_projects_text_delta() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let previous = capture_codex_test_env();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let workspace = std::env::temp_dir().join(format!(
        "pinvou-node-fake-codex-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&workspace).unwrap();
    let version_script = "Write-Output 'codex-cli 0.139.0'";
    let app_server_script = r#"
while (($line = [Console]::In.ReadLine()) -ne $null) {
  $frame = $line | ConvertFrom-Json
  if ($frame.method -eq 'initialize') {
    [Console]::Out.WriteLine(('{"id":' + $frame.id + ',"result":{"userAgent":"fake-codex/0.139"}}'))
  } elseif ($frame.method -eq 'initialized') {
  } elseif ($frame.method -eq 'account/read') {
    [Console]::Out.WriteLine(('{"id":' + $frame.id + ',"result":{"requiresOpenaiAuth":false,"account":null}}'))
  } elseif ($frame.method -eq 'model/list') {
    [Console]::Out.WriteLine(('{"id":' + $frame.id + ',"result":{"models":["fake-model"]}}'))
  } elseif ($frame.method -eq 'thread/start') {
    [Console]::Out.WriteLine(('{"id":' + $frame.id + ',"result":{"thread":{"id":"thread-fake"}}}'))
    [Console]::Out.WriteLine('{"method":"thread/started","params":{"thread":{"id":"thread-fake"}}}')
  } elseif ($frame.method -eq 'turn/start') {
    [Console]::Out.WriteLine(('{"id":' + $frame.id + ',"result":{"turn":{"id":"turn-fake"}}}'))
    [Console]::Out.WriteLine('{"method":"turn/started","params":{"threadId":"thread-fake","turn":{"id":"turn-fake","status":"inProgress"}}}')
    [Console]::Out.WriteLine('{"method":"item/agentMessage/delta","params":{"threadId":"thread-fake","turnId":"turn-fake","itemId":"message-fake","delta":"fake codex says hi"}}')
  }
  [Console]::Out.Flush()
}
"#;
    unsafe {
        std::env::set_var("PINVOU_CODEX_EXECUTABLE_FOR_TEST", "powershell.exe");
        std::env::set_var(
            "PINVOU_CODEX_VERSION_ARGS_JSON_FOR_TEST",
            serde_json::to_string(&[
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                version_script,
            ])
            .unwrap(),
        );
        std::env::set_var(
            "PINVOU_CODEX_DOCTOR_ARGS_JSON_FOR_TEST",
            serde_json::to_string(&[
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                version_script,
            ])
            .unwrap(),
        );
        std::env::set_var(
            "PINVOU_CODEX_APP_SERVER_ARGS_JSON_FOR_TEST",
            serde_json::to_string(&[
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                app_server_script,
            ])
            .unwrap(),
        );
        std::env::set_var("PINVOU_CODEX_WORKING_DIRECTORY_FOR_TEST", &workspace);
    }

    let session = NodeSession::new("node-instance").unwrap();
    let switch = IpcMessage::request(
        serde_json::json!(20),
        "runtime.switch",
        serde_json::json!({"instance_id":"node-instance", "runtime":"codex"}),
    )
    .unwrap();
    assert_eq!(
        session.handle(switch).unwrap().payload()["runtime"],
        "codex"
    );

    let echo = IpcMessage::request(
        serde_json::json!(21),
        "runtime.echo",
        serde_json::json!({"instance_id":"node-instance", "text":"hello fake codex"}),
    )
    .unwrap();
    let event = session.handle(echo).unwrap();
    let envelope = RuntimeEventEnvelope::from_value(event.payload().clone()).unwrap();
    assert_eq!(envelope.kind(), "text.delta");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(envelope.payload().get()).unwrap()["content"],
        "fake codex says hi"
    );

    restore_codex_test_env(previous);
    let _ = std::fs::remove_dir_all(workspace);
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

#[cfg(windows)]
fn capture_codex_test_env() -> Vec<(&'static str, Option<std::ffi::OsString>)> {
    [
        "PINVOU_CODEX_EXECUTABLE_FOR_TEST",
        "PINVOU_CODEX_VERSION_ARGS_JSON_FOR_TEST",
        "PINVOU_CODEX_DOCTOR_ARGS_JSON_FOR_TEST",
        "PINVOU_CODEX_APP_SERVER_ARGS_JSON_FOR_TEST",
        "PINVOU_CODEX_WORKING_DIRECTORY_FOR_TEST",
    ]
    .into_iter()
    .map(|name| (name, std::env::var_os(name)))
    .collect()
}

#[cfg(windows)]
fn restore_codex_test_env(previous: Vec<(&'static str, Option<std::ffi::OsString>)>) {
    for (name, value) in previous {
        match value {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
    }
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
