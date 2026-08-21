use pinvou_agent_adapter_codex::CodexAdapter;
use pinvou_runtime_api::AgentRuntimeAdapter;

/// Manual, zero-turn smoke. It is intentionally ignored in CI and never consumes model quota.
#[test]
#[ignore = "requires a locally installed/authenticated Codex; run manually"]
fn real_codex_zero_turn_probe_and_auth_status() {
    let mut adapter = CodexAdapter::default();
    adapter.probe().unwrap();
    adapter.auth_status().unwrap();
}
