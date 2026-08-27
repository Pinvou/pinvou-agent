use pinvou_agent_adapter_codex::CodexAdapter;
use pinvou_protocol::RuntimeEventKind;
use pinvou_runtime_api::{AgentRuntimeAdapter, RuntimeCommand, RuntimeOperation};

/// Manual, zero-turn smoke. It is intentionally ignored in CI and never consumes model quota.
#[test]
#[ignore = "requires a locally installed/authenticated Codex; run manually"]
fn real_codex_zero_turn_probe_and_auth_status() {
    let mut adapter = CodexAdapter::default();
    adapter.probe().unwrap();
    adapter.auth_status().unwrap();
}

#[test]
#[ignore = "requires a locally installed/authenticated Codex and consumes one model turn"]
fn real_codex_dynamic_tool_failure_completes_the_turn() {
    let mut adapter = CodexAdapter::default();
    adapter.probe().unwrap();
    let session = adapter
        .create(RuntimeOperation::new("live-tool-turn", serde_json::json!({})).unwrap())
        .unwrap();
    adapter
        .send(
            &session,
            RuntimeCommand::text(
                "Check the current official Android routes for LiteRT, llama.cpp, and ONNX Runtime. If a web tool is unavailable, continue without it and finish the answer."
            )
            .unwrap(),
        )
        .unwrap();
    let mut events = adapter.subscribe_events(&session).unwrap();
    for _ in 0..4096 {
        let event = events
            .next()
            .expect("runtime event stream remained open")
            .unwrap();
        if event.event_kind() != RuntimeEventKind::TextDelta {
            eprintln!("{} {:?}", event.kind(), event.payload());
        }
        if event.event_kind() == RuntimeEventKind::TurnEnded {
            adapter.close(&session).unwrap();
            return;
        }
    }
    panic!("live turn did not complete within 4096 events")
}
