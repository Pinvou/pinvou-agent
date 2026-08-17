use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;

use super::*;

struct TempRuntime {
    root: PathBuf,
    ledger: PathBuf,
}

impl TempRuntime {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!(
            "pinvou-os-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let ledger = root.join("events.v1.jsonl");
        std::fs::create_dir_all(&root).unwrap();
        Self { root, ledger }
    }

    fn boot(&self) -> PinvouOsRuntime {
        PinvouOsRuntime::boot(self.ledger.clone()).unwrap()
    }
}

impl Drop for TempRuntime {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn capability(id: &str, resource_class: ResourceClass) -> CapabilityContract {
    CapabilityContract {
        capability_id: id.to_string(),
        version: 1,
        summary: format!("execute {id}"),
        input_schema: json!({ "type": "object" }),
        output_schema: json!({ "type": "object" }),
        preconditions: Vec::new(),
        permissions: Vec::new(),
        side_effects: Vec::new(),
        resource_class,
        interruptibility: Interruptibility::Checkpoint,
        idempotent: false,
    }
}

fn mission_agent(
    runtime: &PinvouOsRuntime,
    capability_id: &str,
    priority: u8,
    interruptibility: Interruptibility,
) -> AgentManifest {
    let started = runtime
        .open_mission(OpenMissionRequest {
            objective: format!("mission for {capability_id}"),
            priority,
            deadline_at_ms: None,
        })
        .unwrap();
    runtime
        .register_mission_agent(RegisterMissionAgentRequest {
            display_name: capability_id.to_string(),
            role: "test executor".to_string(),
            capabilities: vec![capability(capability_id, ResourceClass::Heavy)],
            priority,
            interruptibility,
            mission_id: started.mission.mission_id,
            run_id: started.run.run_id,
        })
        .unwrap()
}

fn resources(temperature_c: f64, memory_used_pct: f64) -> ResourceObservation {
    ResourceObservation {
        sampled_at_ms: chrono::Utc::now().timestamp_millis(),
        cpu_usage_pct: Some(20.0),
        memory_used_pct: Some(memory_used_pct),
        gpu_usage_pct: Some(30.0),
        temperature_c: Some(temperature_c),
        power_w: Some(25.0),
    }
}

#[test]
fn boot_declares_one_continuous_identity_and_resident_agents() {
    let temp = TempRuntime::new("boot");
    let runtime = temp.boot();
    let snapshot = runtime.snapshot();

    assert_eq!(
        snapshot
            .identity
            .as_ref()
            .map(|value| value.identity_id.as_str()),
        Some(PINVOU_IDENTITY_ID)
    );
    assert_eq!(
        snapshot.identity.as_ref().unwrap().continuity,
        IdentityContinuity::Continuous
    );
    for agent_id in [
        "agent:front",
        "agent:surface",
        RESOURCE_AGENT_ID,
        "agent:device",
        "agent:memory",
        "agent:policy",
        "agent:attention",
    ] {
        assert!(snapshot.agents.contains_key(agent_id), "missing {agent_id}");
    }

    // 领域快照不得把旧执行底座的 Session 概念重新包装进来。
    let encoded = serde_json::to_string(&snapshot)
        .unwrap()
        .to_ascii_lowercase();
    assert!(!encoded.contains("session"));
}

#[test]
fn capability_catalog_tells_available_temporary_and_unsupported_apart() {
    let temp = TempRuntime::new("capabilities");
    let runtime = temp.boot();

    let front = runtime.explain_capability("user.interact");
    assert_eq!(front.state, CapabilityAvailabilityState::Available);
    let surface = runtime.explain_capability("surface.observe");
    assert_eq!(
        surface.state,
        CapabilityAvailabilityState::TemporarilyUnavailable
    );
    assert_eq!(
        runtime.explain_capability("teleport.execute").state,
        CapabilityAvailabilityState::Unsupported
    );

    let agent = mission_agent(&runtime, "render.video", 40, Interruptibility::Checkpoint);
    assert_eq!(
        runtime.explain_capability("render.video").state,
        CapabilityAvailabilityState::Available
    );
    assert_eq!(agent.capabilities.len(), 1);
}

#[test]
fn hot_claim_pauses_only_interruptible_low_priority_mission_agents() {
    let temp = TempRuntime::new("hot-governor");
    let runtime = temp.boot();
    let low = mission_agent(
        &runtime,
        "background.index",
        40,
        Interruptibility::Checkpoint,
    );
    let high = mission_agent(&runtime, "urgent.reply", 95, Interruptibility::Immediate);
    let atomic = mission_agent(&runtime, "atomic.commit", 20, Interruptibility::Atomic);

    let decision = runtime.observe_resources(resources(89.0, 60.0)).unwrap();
    assert_eq!(decision.pressure, ResourcePressure::Hot);
    assert!(decision.pressure_claim_id.is_some());
    assert_eq!(decision.directives.len(), 1);
    assert_eq!(decision.directives[0].target_agent_id, low.agent_id);
    assert_eq!(decision.directives[0].action, DirectiveAction::Pause);
    assert!(!decision.directives[0].hard);

    let snapshot = runtime.snapshot();
    let low_projected = snapshot.agents.get(&low.agent_id).unwrap();
    assert_eq!(low_projected.desired_state, AgentState::Paused);
    assert_eq!(low_projected.observed_state, AgentState::Running);
    assert_eq!(
        snapshot.agents.get(&high.agent_id).unwrap().desired_state,
        AgentState::Running
    );
    assert_eq!(
        snapshot.agents.get(&atomic.agent_id).unwrap().desired_state,
        AgentState::Running
    );

    let ack = runtime
        .acknowledge_directive(
            &decision.directives[0].directive_id,
            true,
            "paused".to_string(),
        )
        .unwrap();
    assert_eq!(ack.status, DirectiveStatus::Applied);
    assert_eq!(
        runtime
            .snapshot()
            .agents
            .get(&low.agent_id)
            .unwrap()
            .observed_state,
        AgentState::Paused
    );
}

#[test]
fn normal_pressure_resumes_agents_only_after_adapter_acknowledges() {
    let temp = TempRuntime::new("resume-governor");
    let runtime = temp.boot();
    let agent = mission_agent(&runtime, "background.sync", 30, Interruptibility::Immediate);

    let hot = runtime.observe_resources(resources(90.0, 60.0)).unwrap();
    runtime
        .acknowledge_directive(&hot.directives[0].directive_id, true, "paused".to_string())
        .unwrap();
    let normal = runtime.observe_resources(resources(60.0, 50.0)).unwrap();
    assert_eq!(normal.pressure, ResourcePressure::Normal);
    assert_eq!(normal.directives.len(), 1);
    assert_eq!(normal.directives[0].action, DirectiveAction::Resume);
    let projected = runtime.snapshot();
    assert_eq!(
        projected.agents.get(&agent.agent_id).unwrap().desired_state,
        AgentState::Running
    );
    assert_eq!(
        projected
            .agents
            .get(&agent.agent_id)
            .unwrap()
            .observed_state,
        AgentState::Paused
    );

    runtime
        .acknowledge_directive(
            &normal.directives[0].directive_id,
            true,
            "resumed".to_string(),
        )
        .unwrap();
    assert_eq!(
        runtime
            .snapshot()
            .agents
            .get(&agent.agent_id)
            .unwrap()
            .observed_state,
        AgentState::Running
    );
}

#[test]
fn critical_pressure_issues_hard_stop_even_for_atomic_agents() {
    let temp = TempRuntime::new("critical-governor");
    let runtime = temp.boot();
    let agent = mission_agent(&runtime, "atomic.work", 100, Interruptibility::Atomic);
    let decision = runtime.observe_resources(resources(96.0, 50.0)).unwrap();
    let directive = decision
        .directives
        .iter()
        .find(|directive| directive.target_agent_id == agent.agent_id)
        .unwrap();
    assert_eq!(directive.action, DirectiveAction::Stop);
    assert!(directive.hard);
}

#[test]
fn replay_restores_projection_and_keeps_entity_events_causal() {
    let temp = TempRuntime::new("replay");
    let before = {
        let runtime = temp.boot();
        let started = runtime
            .open_mission(OpenMissionRequest {
                objective: "keep continuity across restart".to_string(),
                priority: 50,
                deadline_at_ms: None,
            })
            .unwrap();
        let events = runtime.list_events(None, 1_000).unwrap();
        let run_event = events
            .iter()
            .find(|event| {
                matches!(
                    &event.event,
                    RuntimeEvent::RunStarted { run } if run.run_id == started.run.run_id
                )
            })
            .unwrap();
        assert!(run_event.causation_id.is_some());
        runtime.snapshot()
    };

    let rebooted = temp.boot();
    let after = rebooted.snapshot();
    assert_eq!(after.identity, before.identity);
    assert_eq!(after.missions, before.missions);
    assert_eq!(after.runs, before.runs);
    assert!(after.last_sequence > before.last_sequence); // 每次 boot 都记录 RuntimeStarted。
}

#[test]
fn malformed_trailing_record_does_not_destroy_prior_runtime_truth() {
    let temp = TempRuntime::new("malformed-tail");
    {
        let runtime = temp.boot();
        runtime
            .open_mission(OpenMissionRequest {
                objective: "survive a torn final write".to_string(),
                priority: 50,
                deadline_at_ms: None,
            })
            .unwrap();
    }
    use std::io::Write as _;
    let mut ledger = std::fs::OpenOptions::new()
        .append(true)
        .open(&temp.ledger)
        .unwrap();
    write!(ledger, "{{\"torn\":").unwrap();
    drop(ledger);

    let runtime = temp.boot();
    assert_eq!(runtime.snapshot().missions.len(), 1);
}
