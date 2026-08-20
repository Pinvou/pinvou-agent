use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

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
        app_cgroup: None,
    }
}

fn resources_with_app_cgroup(
    observed_at_ms: i64,
    memory_current_bytes: u64,
    memory_high_bytes: u64,
    events_high: u64,
    events_oom: u64,
    events_oom_kill: u64,
) -> ResourceObservation {
    let mut observation = resources(50.0, 20.0);
    observation.sampled_at_ms = observed_at_ms;
    observation.app_cgroup = Some(AppCgroupResourceObservation {
        observed_at_ms,
        instance_generation: "0123456789abcdef0123456789abcdef".to_string(),
        memory_current_bytes: Some(memory_current_bytes),
        memory_high_bytes: Some(memory_high_bytes),
        memory_max_bytes: Some(memory_high_bytes.saturating_mul(2)),
        memory_events_high: Some(events_high),
        memory_events_oom: Some(events_oom),
        memory_events_oom_kill: Some(events_oom_kill),
        memory_pressure_full_avg10: Some(0.5),
    });
    observation
}

fn host_work_request(
    owner: &str,
    priority: u8,
    interruptibility: Interruptibility,
    essential: bool,
    governable: bool,
    actions: &[HostWorkAction],
) -> RegisterHostWorkRequest {
    RegisterHostWorkRequest {
        owner: owner.to_string(),
        kind: HostWorkKind::ScheduledRun,
        resource_class: ResourceClass::Heavy,
        priority,
        interruptibility,
        essential,
        governable,
        supported_actions: actions.iter().copied().collect::<BTreeSet<_>>(),
        initial_observed_state: HostWorkObservedState::Running,
    }
}

fn user_evidence_event(
    runtime: &PinvouOsRuntime,
    subject: &str,
    predicate: &str,
    value: serde_json::Value,
) -> EventEnvelope {
    runtime
        .record_test_user_claim(subject, predicate, value)
        .unwrap()
}

fn asserted_claim_id(event: &EventEnvelope) -> String {
    let RuntimeEvent::ClaimAsserted { claim } = &event.event else {
        panic!("expected ClaimAsserted test evidence")
    };
    claim.claim_id.clone()
}

#[test]
fn schema_v6_resource_cgroup_fields_are_optional_and_omitted_from_legacy_shapes() {
    let encoded_observation = serde_json::to_value(resources(50.0, 20.0)).unwrap();
    assert!(encoded_observation.get("appCgroup").is_none());
    let legacy_observation: ResourceObservation = serde_json::from_value(json!({
        "sampledAtMs": 1,
        "cpuUsagePct": 10.0,
        "memoryUsedPct": 20.0
    }))
    .unwrap();
    assert!(legacy_observation.app_cgroup.is_none());

    let encoded_state = serde_json::to_value(ResourceState::default()).unwrap();
    assert_eq!(encoded_state, json!({ "pressure": "normal" }));
    let legacy_state: ResourceState =
        serde_json::from_value(json!({ "pressure": "normal" })).unwrap();
    assert!(!legacy_state.app_cgroup_critical);
    assert!(legacy_state.last_app_cgroup_observation.is_none());
    assert!(legacy_state.last_fresh_critical_evidence.is_none());
}

fn proposed_fact(
    candidate_id: &str,
    event: &EventEnvelope,
    value: serde_json::Value,
) -> MemoryCandidate {
    MemoryCandidate {
        candidate_id: candidate_id.to_string(),
        kind: OrganizedMemoryKind::ContextualFact,
        subject: "user".to_string(),
        predicate: candidate_id.to_string(),
        value,
        applicability: MemoryApplicability {
            // Runtime 必须覆盖这个调用方自报的身份空间。
            space_id: "forged-space".to_string(),
            environment: BTreeMap::new(),
            valid_from_ms: event.occurred_at_ms,
            valid_until_ms: None,
        },
        importance: 0.8,
        confidence: 1.0,
        intent: MemoryCandidateIntent::Assert,
        target_memory_id: None,
        evidence: vec![MemoryEvidence {
            event_id: event.event_id.clone(),
            source_actor_id: "actor:user".to_string(),
            origin: MemoryEvidenceOrigin::UserExplicit,
            polarity: MemoryEvidencePolarity::Supports,
            observed_at_ms: 1,
            recorded_at_ms: 1,
            reliability: 1.0,
            mission_id: None,
            run_id: None,
        }],
    }
}

fn raw_ledger_events(temp: &TempRuntime) -> Vec<EventEnvelope> {
    std::fs::read_to_string(&temp.ledger)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn append_test_envelope(temp: &TempRuntime, envelope: &EventEnvelope) {
    use std::io::Write as _;
    let mut ledger = std::fs::OpenOptions::new()
        .append(true)
        .open(&temp.ledger)
        .unwrap();
    serde_json::to_writer(&mut ledger, envelope).unwrap();
    ledger.write_all(b"\n").unwrap();
    ledger.sync_data().unwrap();
}

fn screen_observer_identity_schema_test_envelope(event: RuntimeEvent) -> EventEnvelope {
    EventEnvelope {
        schema_version: SCREEN_OBSERVER_IDENTITY_SCHEMA_VERSION,
        sequence: 1,
        event_id: "event-0000000000000001".to_string(),
        occurred_at_ms: 1,
        source_actor_id: KERNEL_ACTOR_ID.to_string(),
        mission_id: None,
        run_id: None,
        interaction_scope_id: None,
        interaction_run_id: None,
        causation_id: None,
        correlation_id: None,
        event,
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
    assert_eq!(snapshot.agents.len(), 12);
    for agent_id in [
        "agent:front",
        "agent:orchestrator",
        SCREEN_OBSERVER_AGENT_ID,
        RESOURCE_AGENT_ID,
        CONNECTIVITY_AGENT_ID,
        INFERENCE_AGENT_ID,
        "agent:device",
        "agent:capability",
        "agent:memory",
        "agent:policy",
        "agent:attention",
        ASR_CONTEXT_AGENT_ID,
    ] {
        assert!(snapshot.agents.contains_key(agent_id), "missing {agent_id}");
    }
    assert_eq!(
        snapshot
            .agents
            .values()
            .filter(|agent| matches!(agent.observed_state, AgentState::Running | AgentState::Idle))
            .count(),
        9
    );
    for agent_id in [
        "agent:front",
        "agent:orchestrator",
        RESOURCE_AGENT_ID,
        CONNECTIVITY_AGENT_ID,
        INFERENCE_AGENT_ID,
        "agent:capability",
        "agent:memory",
        "agent:attention",
        ASR_CONTEXT_AGENT_ID,
    ] {
        assert_eq!(
            snapshot.agents[agent_id].observed_state,
            AgentState::Running,
            "{agent_id} has an in-process handler and must not remain Starting"
        );
    }
    // 领域快照不得把旧执行底座的 Session 概念重新包装进来。
    let encoded = serde_json::to_string(&snapshot)
        .unwrap()
        .to_ascii_lowercase();
    assert!(!encoded.contains("session"));
}

#[test]
fn schema_v5_rejects_legacy_screen_observer_ids_only_in_typed_identity_fields() {
    let canonical_agent = AgentManifest {
        agent_id: SCREEN_OBSERVER_AGENT_ID.to_string(),
        display_name: "Screen Observer Agent".to_string(),
        kind: AgentKind::System,
        role: "observe accessible UI facts".to_string(),
        capabilities: vec![capability(
            SCREEN_OBSERVE_CAPABILITY_ID,
            ResourceClass::Light,
        )],
        priority: 85,
        interruptibility: Interruptibility::Immediate,
        observed_state: AgentState::Starting,
        desired_state: AgentState::Starting,
        mission_id: None,
        run_id: None,
        created_at_ms: 1,
    };
    let canonical_claim = WorldClaim {
        claim_id: "claim-screen".to_string(),
        subject: "screen".to_string(),
        predicate: "focused_window".to_string(),
        value: json!("pinvou"),
        confidence: 1.0,
        asserted_by_actor_id: SCREEN_OBSERVER_AGENT_ID.to_string(),
        evidence_event_ids: Vec::new(),
        asserted_at_ms: 1,
        active: true,
        retracted_at_ms: None,
        retraction_reason: None,
    };
    let canonical_directive = ControlDirective {
        directive_id: "directive-screen".to_string(),
        target_agent_id: SCREEN_OBSERVER_AGENT_ID.to_string(),
        action: DirectiveAction::Pause,
        reason: "test".to_string(),
        hard: false,
        issued_at_ms: 1,
        status: DirectiveStatus::Pending,
        acknowledged_at_ms: None,
        acknowledgement_detail: None,
    };

    let mut legacy_source =
        screen_observer_identity_schema_test_envelope(RuntimeEvent::RuntimeStarted {
            process_id: 7,
        });
    legacy_source.source_actor_id = LEGACY_SURFACE_AGENT_ID.to_string();

    let mut legacy_agent = canonical_agent.clone();
    legacy_agent.agent_id = LEGACY_SURFACE_AGENT_ID.to_string();
    let legacy_agent =
        screen_observer_identity_schema_test_envelope(RuntimeEvent::AgentRegistered {
            agent: legacy_agent,
        });

    let mut legacy_capability_agent = canonical_agent;
    legacy_capability_agent.capabilities[0].capability_id =
        LEGACY_SURFACE_OBSERVE_CAPABILITY_ID.to_string();
    let legacy_capability =
        screen_observer_identity_schema_test_envelope(RuntimeEvent::AgentRegistered {
            agent: legacy_capability_agent,
        });

    let mut legacy_claim = canonical_claim.clone();
    legacy_claim.asserted_by_actor_id = LEGACY_SURFACE_AGENT_ID.to_string();
    let legacy_claim = screen_observer_identity_schema_test_envelope(RuntimeEvent::ClaimAsserted {
        claim: legacy_claim,
    });

    let mut legacy_directive = canonical_directive.clone();
    legacy_directive.target_agent_id = LEGACY_SURFACE_AGENT_ID.to_string();
    let legacy_directive =
        screen_observer_identity_schema_test_envelope(RuntimeEvent::DirectiveIssued {
            directive: legacy_directive,
        });

    let legacy_ack =
        screen_observer_identity_schema_test_envelope(RuntimeEvent::DirectiveAcknowledged {
            directive_id: canonical_directive.directive_id,
            target_agent_id: LEGACY_SURFACE_AGENT_ID.to_string(),
            status: DirectiveStatus::Applied,
            resulting_state: AgentState::Paused,
            acknowledged_at_ms: 1,
            detail: "test".to_string(),
        });
    let retired_projection =
        screen_observer_identity_schema_test_envelope(RuntimeEvent::MemoryProjectionUpdated {
            revision: 1,
            operation: "legacy projection".to_string(),
            memory_id: "legacy-memory".to_string(),
            projection: json!({}),
        });

    for (label, envelope) in [
        ("source", legacy_source),
        ("agent", legacy_agent),
        ("capability", legacy_capability),
        ("claim", legacy_claim),
        ("directive", legacy_directive),
        ("acknowledgement", legacy_ack),
        ("retired-memory-projection", retired_projection),
    ] {
        assert!(
            super::runtime::validate_current_schema_screen_observer_identity(&envelope).is_err(),
            "{label} typed legacy identity must fail validation"
        );
        assert!(
            super::runtime::serialize_envelope_frame(&envelope).is_err(),
            "{label} typed legacy identity must fail before append"
        );

        let temp = TempRuntime::new(&format!("v5-legacy-{label}"));
        std::fs::write(&temp.ledger, "").unwrap();
        append_test_envelope(&temp, &envelope);
        assert!(
            PinvouOsRuntime::boot(temp.ledger.clone()).is_err(),
            "{label} typed legacy identity must fail during replay"
        );
    }

    let mut literal_claim = canonical_claim;
    literal_claim.value = json!(LEGACY_SURFACE_AGENT_ID);
    let literal = screen_observer_identity_schema_test_envelope(RuntimeEvent::ClaimAsserted {
        claim: literal_claim,
    });
    super::runtime::validate_current_schema_screen_observer_identity(&literal)
        .expect("ordinary claim values are content, not actor identities");
    super::runtime::serialize_envelope_frame(&literal)
        .expect("ordinary claim values must not be rejected as legacy identities");
}

#[test]
fn legacy_surface_identity_replays_as_one_canonical_screen_observer() {
    let temp = TempRuntime::new("screen-observer-v4-upcast");
    std::fs::write(&temp.ledger, "").unwrap();

    let legacy_agent = AgentManifest {
        agent_id: LEGACY_SURFACE_AGENT_ID.to_string(),
        display_name: "Surface Agent".to_string(),
        kind: AgentKind::System,
        role: "legacy screen observer".to_string(),
        capabilities: vec![capability(
            LEGACY_SURFACE_OBSERVE_CAPABILITY_ID,
            ResourceClass::Light,
        )],
        priority: 85,
        interruptibility: Interruptibility::Immediate,
        observed_state: AgentState::Starting,
        desired_state: AgentState::Starting,
        mission_id: None,
        run_id: None,
        created_at_ms: 123,
    };
    let legacy_claim = WorldClaim {
        claim_id: "claim-legacy-screen".to_string(),
        subject: "screen".to_string(),
        predicate: "focused_window".to_string(),
        value: json!("pinvou"),
        confidence: 1.0,
        asserted_by_actor_id: LEGACY_SURFACE_AGENT_ID.to_string(),
        evidence_event_ids: vec!["event-0000000000000002".to_string()],
        asserted_at_ms: 3,
        active: true,
        retracted_at_ms: None,
        retraction_reason: None,
    };
    let legacy_directive = ControlDirective {
        directive_id: "directive-legacy-screen".to_string(),
        target_agent_id: LEGACY_SURFACE_AGENT_ID.to_string(),
        action: DirectiveAction::Pause,
        reason: "legacy compatibility fixture".to_string(),
        hard: false,
        issued_at_ms: 4,
        status: DirectiveStatus::Pending,
        acknowledged_at_ms: None,
        acknowledgement_detail: None,
    };
    let envelopes = [
        EventEnvelope {
            schema_version: 4,
            sequence: 1,
            event_id: "event-0000000000000001".to_string(),
            occurred_at_ms: 1,
            source_actor_id: KERNEL_ACTOR_ID.to_string(),
            mission_id: None,
            run_id: None,
            interaction_scope_id: None,
            interaction_run_id: None,
            causation_id: None,
            correlation_id: None,
            event: RuntimeEvent::RuntimeStarted { process_id: 7 },
        },
        EventEnvelope {
            schema_version: 4,
            sequence: 2,
            event_id: "event-0000000000000002".to_string(),
            occurred_at_ms: 2,
            source_actor_id: KERNEL_ACTOR_ID.to_string(),
            mission_id: None,
            run_id: None,
            interaction_scope_id: None,
            interaction_run_id: None,
            causation_id: None,
            correlation_id: None,
            event: RuntimeEvent::AgentRegistered {
                agent: legacy_agent,
            },
        },
        EventEnvelope {
            schema_version: 4,
            sequence: 3,
            event_id: "event-0000000000000003".to_string(),
            occurred_at_ms: 3,
            source_actor_id: LEGACY_SURFACE_AGENT_ID.to_string(),
            mission_id: None,
            run_id: None,
            interaction_scope_id: None,
            interaction_run_id: None,
            causation_id: Some("event-0000000000000002".to_string()),
            correlation_id: None,
            event: RuntimeEvent::ClaimAsserted {
                claim: legacy_claim,
            },
        },
        EventEnvelope {
            schema_version: 4,
            sequence: 4,
            event_id: "event-0000000000000004".to_string(),
            occurred_at_ms: 4,
            source_actor_id: KERNEL_ACTOR_ID.to_string(),
            mission_id: None,
            run_id: None,
            interaction_scope_id: None,
            interaction_run_id: None,
            causation_id: None,
            correlation_id: None,
            event: RuntimeEvent::DirectiveIssued {
                directive: legacy_directive,
            },
        },
        EventEnvelope {
            schema_version: 4,
            sequence: 5,
            event_id: "event-0000000000000005".to_string(),
            occurred_at_ms: 5,
            source_actor_id: KERNEL_ACTOR_ID.to_string(),
            mission_id: None,
            run_id: None,
            interaction_scope_id: None,
            interaction_run_id: None,
            causation_id: Some("event-0000000000000004".to_string()),
            correlation_id: None,
            event: RuntimeEvent::DirectiveAcknowledged {
                directive_id: "directive-legacy-screen".to_string(),
                target_agent_id: LEGACY_SURFACE_AGENT_ID.to_string(),
                status: DirectiveStatus::Applied,
                resulting_state: AgentState::Paused,
                acknowledged_at_ms: 5,
                detail: "legacy adapter paused".to_string(),
            },
        },
    ];
    for envelope in &envelopes {
        append_test_envelope(&temp, envelope);
    }
    let legacy_ledger_prefix = std::fs::read(&temp.ledger).unwrap();

    let runtime = temp.boot();
    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.agents.len(), 12);
    assert!(!snapshot.agents.contains_key(LEGACY_SURFACE_AGENT_ID));
    let screen_observer = &snapshot.agents[SCREEN_OBSERVER_AGENT_ID];
    assert_eq!(screen_observer.display_name, "Screen Observer Agent");
    assert_eq!(screen_observer.created_at_ms, 123);
    assert_eq!(screen_observer.observed_state, AgentState::Paused);
    assert_eq!(screen_observer.desired_state, AgentState::Paused);
    assert_eq!(
        screen_observer.capabilities[0].capability_id,
        SCREEN_OBSERVE_CAPABILITY_ID
    );
    assert_eq!(
        snapshot.claims["claim-legacy-screen"].asserted_by_actor_id,
        SCREEN_OBSERVER_AGENT_ID
    );
    assert_eq!(
        snapshot.directives["directive-legacy-screen"].target_agent_id,
        SCREEN_OBSERVER_AGENT_ID
    );

    let canonical = runtime.explain_capability(SCREEN_OBSERVE_CAPABILITY_ID);
    let legacy_alias = runtime.explain_capability(LEGACY_SURFACE_OBSERVE_CAPABILITY_ID);
    assert_eq!(
        canonical.state,
        CapabilityAvailabilityState::TemporarilyUnavailable
    );
    assert_eq!(legacy_alias.capability_id, SCREEN_OBSERVE_CAPABILITY_ID);
    assert_eq!(legacy_alias.state, canonical.state);
    assert_eq!(
        legacy_alias.candidate_agent_ids,
        vec![SCREEN_OBSERVER_AGENT_ID]
    );

    runtime
        .organize_memory(MemoryCandidate {
            candidate_id: "legacy-screen-observation".to_string(),
            kind: OrganizedMemoryKind::ContextualFact,
            subject: "screen".to_string(),
            predicate: "focused_window".to_string(),
            value: json!("pinvou"),
            applicability: MemoryApplicability {
                space_id: "forged-space".to_string(),
                environment: BTreeMap::new(),
                valid_from_ms: 0,
                valid_until_ms: None,
            },
            importance: 0.5,
            confidence: 1.0,
            intent: MemoryCandidateIntent::Assert,
            target_memory_id: None,
            evidence: vec![MemoryEvidence {
                event_id: "event-0000000000000003".to_string(),
                source_actor_id: LEGACY_SURFACE_AGENT_ID.to_string(),
                origin: MemoryEvidenceOrigin::AgentAction,
                polarity: MemoryEvidencePolarity::Supports,
                observed_at_ms: 0,
                recorded_at_ms: 0,
                reliability: 0.0,
                mission_id: None,
                run_id: None,
            }],
        })
        .unwrap();
    let new_memory_decisions = raw_ledger_events(&temp)
        .into_iter()
        .filter(|envelope| {
            envelope.schema_version == SCHEMA_VERSION
                && matches!(
                    envelope.event,
                    RuntimeEvent::OrganizedMemoryDecisionRecorded { .. }
                )
        })
        .collect::<Vec<_>>();
    assert_eq!(new_memory_decisions.len(), 1);
    let encoded_decision = serde_json::to_string(&new_memory_decisions[0]).unwrap();
    assert!(encoded_decision.contains(SCREEN_OBSERVER_AGENT_ID));
    assert!(!encoded_decision.contains(LEGACY_SURFACE_AGENT_ID));
    assert!(
        std::fs::read(&temp.ledger)
            .unwrap()
            .starts_with(&legacy_ledger_prefix),
        "schema-v5 replay must append canonical events without rewriting v4 audit bytes"
    );

    drop(runtime);
    let replayed = temp.boot();
    let replayed_snapshot = replayed.snapshot();
    assert_eq!(replayed_snapshot.agents.len(), 12);
    assert!(!replayed_snapshot
        .agents
        .contains_key(LEGACY_SURFACE_AGENT_ID));
    let canonical_registrations = raw_ledger_events(&temp)
        .into_iter()
        .filter(|envelope| {
            matches!(
                &envelope.event,
                RuntimeEvent::AgentRegistered { agent }
                    if agent.agent_id == SCREEN_OBSERVER_AGENT_ID
            )
        })
        .count();
    assert_eq!(canonical_registrations, 1);
}

#[test]
fn legacy_memory_actor_migrates_through_one_canonical_v5_checkpoint() {
    let temp = TempRuntime::new("screen-observer-memory-v4-upcast");
    std::fs::write(&temp.ledger, "").unwrap();

    let claim_event = EventEnvelope {
        schema_version: 4,
        sequence: 2,
        event_id: "event-0000000000000002".to_string(),
        occurred_at_ms: 2,
        source_actor_id: LEGACY_SURFACE_AGENT_ID.to_string(),
        mission_id: None,
        run_id: None,
        interaction_scope_id: None,
        interaction_run_id: None,
        causation_id: None,
        correlation_id: None,
        event: RuntimeEvent::ClaimAsserted {
            claim: WorldClaim {
                claim_id: "claim-legacy-screen-memory".to_string(),
                subject: "user".to_string(),
                predicate: "legacy-screen-memory".to_string(),
                value: json!("dark"),
                confidence: 1.0,
                asserted_by_actor_id: LEGACY_SURFACE_AGENT_ID.to_string(),
                evidence_event_ids: Vec::new(),
                asserted_at_ms: 2,
                active: true,
                retracted_at_ms: None,
                retraction_reason: None,
            },
        },
    };
    let legacy_candidate = MemoryCandidate {
        candidate_id: "legacy-screen-memory-v4".to_string(),
        kind: OrganizedMemoryKind::ContextualFact,
        subject: "user".to_string(),
        predicate: "legacy-screen-memory".to_string(),
        value: json!("dark"),
        applicability: MemoryApplicability {
            space_id: "personal".to_string(),
            environment: BTreeMap::new(),
            valid_from_ms: 2,
            valid_until_ms: None,
        },
        importance: 0.8,
        confidence: 1.0,
        intent: MemoryCandidateIntent::Assert,
        target_memory_id: None,
        evidence: vec![MemoryEvidence {
            event_id: claim_event.event_id.clone(),
            source_actor_id: LEGACY_SURFACE_AGENT_ID.to_string(),
            origin: MemoryEvidenceOrigin::AgentAction,
            polarity: MemoryEvidencePolarity::Supports,
            observed_at_ms: 2,
            recorded_at_ms: 2,
            reliability: 0.7,
            mission_id: None,
            run_id: None,
        }],
    };
    let mut legacy_engine = OrganizedMemoryDecisionEngine::new();
    let legacy_decision = legacy_engine
        .organize(legacy_candidate.clone())
        .unwrap()
        .decision
        .unwrap();
    let legacy_checkpoint = legacy_engine.checkpoint();
    let followup_claim_event = EventEnvelope {
        schema_version: 4,
        sequence: 5,
        event_id: "event-0000000000000005".to_string(),
        occurred_at_ms: 5,
        source_actor_id: LEGACY_SURFACE_AGENT_ID.to_string(),
        mission_id: None,
        run_id: None,
        interaction_scope_id: None,
        interaction_run_id: None,
        causation_id: None,
        correlation_id: None,
        event: RuntimeEvent::ClaimAsserted {
            claim: WorldClaim {
                claim_id: "claim-legacy-screen-memory-followup".to_string(),
                subject: "user".to_string(),
                predicate: "legacy-screen-memory-followup".to_string(),
                value: json!("light"),
                confidence: 1.0,
                asserted_by_actor_id: LEGACY_SURFACE_AGENT_ID.to_string(),
                evidence_event_ids: Vec::new(),
                asserted_at_ms: 5,
                active: true,
                retracted_at_ms: None,
                retraction_reason: None,
            },
        },
    };
    let envelopes = [
        EventEnvelope {
            schema_version: 4,
            sequence: 1,
            event_id: "event-0000000000000001".to_string(),
            occurred_at_ms: 1,
            source_actor_id: KERNEL_ACTOR_ID.to_string(),
            mission_id: None,
            run_id: None,
            interaction_scope_id: None,
            interaction_run_id: None,
            causation_id: None,
            correlation_id: None,
            event: RuntimeEvent::RuntimeStarted { process_id: 7 },
        },
        claim_event.clone(),
        EventEnvelope {
            schema_version: 4,
            sequence: 3,
            event_id: "event-0000000000000003".to_string(),
            occurred_at_ms: 3,
            source_actor_id: MEMORY_AGENT_ID.to_string(),
            mission_id: None,
            run_id: None,
            interaction_scope_id: None,
            interaction_run_id: None,
            causation_id: Some(claim_event.event_id.clone()),
            correlation_id: Some("legacy-screen-memory-v4".to_string()),
            event: RuntimeEvent::OrganizedMemoryDecisionRecorded {
                decision: legacy_decision,
            },
        },
        EventEnvelope {
            schema_version: 4,
            sequence: 4,
            event_id: "event-0000000000000004".to_string(),
            occurred_at_ms: 4,
            source_actor_id: MEMORY_AGENT_ID.to_string(),
            mission_id: None,
            run_id: None,
            interaction_scope_id: None,
            interaction_run_id: None,
            causation_id: Some("event-0000000000000003".to_string()),
            correlation_id: Some("legacy-screen-memory-v4-checkpoint".to_string()),
            event: RuntimeEvent::OrganizedMemoryCheckpointRecorded {
                checkpoint: legacy_checkpoint,
                legacy_source_event_id: None,
                legacy_migration: None,
            },
        },
        followup_claim_event.clone(),
    ];
    for envelope in &envelopes {
        append_test_envelope(&temp, envelope);
    }
    let legacy_ledger_prefix = std::fs::read(&temp.ledger).unwrap();

    let runtime = temp.boot();
    let events_after_boot = raw_ledger_events(&temp);
    let identity_checkpoints = events_after_boot
        .iter()
        .filter(|envelope| {
            envelope.schema_version == SCHEMA_VERSION
                && envelope.correlation_id.as_deref()
                    == Some("memory-screen-observer-identity-migration")
        })
        .collect::<Vec<_>>();
    assert_eq!(identity_checkpoints.len(), 1);
    let encoded_checkpoint = serde_json::to_string(identity_checkpoints[0]).unwrap();
    assert!(encoded_checkpoint.contains(SCREEN_OBSERVER_AGENT_ID));
    assert!(!encoded_checkpoint.contains(LEGACY_SURFACE_AGENT_ID));
    assert!(
        std::fs::read(&temp.ledger)
            .unwrap()
            .starts_with(&legacy_ledger_prefix),
        "v5 memory identity migration must append without rewriting the validated v4 chain"
    );

    let duplicate = runtime.organize_memory(legacy_candidate.clone()).unwrap();
    assert_eq!(
        duplicate.action,
        MemoryOrganizationAction::IgnoredDuplicate,
        "canonical evidence must match the old idempotency hash only through the exact alias"
    );
    let mut altered_retry = legacy_candidate;
    altered_retry.importance = 0.81;
    assert!(
        runtime.organize_memory(altered_retry).is_err(),
        "the legacy alias may forgive only the renamed actor, not any other content change"
    );
    let decisions_before_followup = raw_ledger_events(&temp)
        .iter()
        .filter(|envelope| {
            matches!(
                envelope.event,
                RuntimeEvent::OrganizedMemoryDecisionRecorded { .. }
            )
        })
        .count();
    assert_eq!(decisions_before_followup, 1);

    let followup = proposed_fact(
        "legacy-screen-memory-followup",
        &followup_claim_event,
        json!("light"),
    );
    runtime.organize_memory(followup).unwrap();
    let newest_decision = raw_ledger_events(&temp)
        .into_iter()
        .rev()
        .find(|envelope| {
            matches!(
                envelope.event,
                RuntimeEvent::OrganizedMemoryDecisionRecorded { .. }
            )
        })
        .unwrap();
    assert_eq!(newest_decision.schema_version, SCHEMA_VERSION);
    let encoded_decision = serde_json::to_string(&newest_decision).unwrap();
    assert!(encoded_decision.contains(SCREEN_OBSERVER_AGENT_ID));
    assert!(!encoded_decision.contains(LEGACY_SURFACE_AGENT_ID));

    drop(runtime);
    let replayed = temp.boot();
    assert_eq!(replayed.snapshot().agents.len(), 12);
    assert_eq!(
        raw_ledger_events(&temp)
            .iter()
            .filter(|envelope| {
                envelope.correlation_id.as_deref()
                    == Some("memory-screen-observer-identity-migration")
            })
            .count(),
        1,
        "the canonical bridge checkpoint must be durable and idempotent across reboot"
    );
    assert!(
        std::fs::read(&temp.ledger)
            .unwrap()
            .starts_with(&legacy_ledger_prefix),
        "reboot must preserve the original v4 audit prefix byte-for-byte"
    );
}

#[test]
fn canonical_v5_memory_checkpoint_remains_a_permanent_replay_boundary() {
    let temp = TempRuntime::new("screen-observer-memory-v5-boundary");
    std::fs::write(&temp.ledger, "").unwrap();

    let mut legacy_engine = OrganizedMemoryDecisionEngine::new();
    let legacy_decision = legacy_engine
        .organize(MemoryCandidate {
            candidate_id: "v5-boundary".to_string(),
            kind: OrganizedMemoryKind::ContextualFact,
            subject: "user".to_string(),
            predicate: "v5-boundary".to_string(),
            value: json!("dark"),
            applicability: MemoryApplicability {
                space_id: "personal".to_string(),
                environment: BTreeMap::new(),
                valid_from_ms: 1,
                valid_until_ms: None,
            },
            importance: 0.8,
            confidence: 1.0,
            intent: MemoryCandidateIntent::Assert,
            target_memory_id: None,
            evidence: vec![MemoryEvidence {
                event_id: "event-v5-boundary-source".to_string(),
                source_actor_id: LEGACY_SURFACE_AGENT_ID.to_string(),
                origin: MemoryEvidenceOrigin::AgentAction,
                polarity: MemoryEvidencePolarity::Supports,
                observed_at_ms: 1,
                recorded_at_ms: 1,
                reliability: 0.7,
                mission_id: None,
                run_id: None,
            }],
        })
        .unwrap()
        .decision
        .unwrap();
    let mut canonical_engine =
        OrganizedMemoryDecisionEngine::replay([legacy_decision.clone()]).unwrap();
    assert!(canonical_engine
        .rewrite_evidence_source_actor_ids(|actor_id| {
            canonical_screen_observer_agent_id(actor_id).to_string()
        })
        .unwrap());
    let canonical_v5_checkpoint = canonical_engine.checkpoint();

    for envelope in [
        EventEnvelope {
            schema_version: 4,
            sequence: 1,
            event_id: "event-0000000000000001".to_string(),
            occurred_at_ms: 1,
            source_actor_id: KERNEL_ACTOR_ID.to_string(),
            mission_id: None,
            run_id: None,
            interaction_scope_id: None,
            interaction_run_id: None,
            causation_id: None,
            correlation_id: None,
            event: RuntimeEvent::RuntimeStarted { process_id: 7 },
        },
        EventEnvelope {
            schema_version: 4,
            sequence: 2,
            event_id: "event-0000000000000002".to_string(),
            occurred_at_ms: 2,
            source_actor_id: MEMORY_AGENT_ID.to_string(),
            mission_id: None,
            run_id: None,
            interaction_scope_id: None,
            interaction_run_id: None,
            causation_id: None,
            correlation_id: Some("v4-raw-memory".to_string()),
            event: RuntimeEvent::OrganizedMemoryDecisionRecorded {
                decision: legacy_decision,
            },
        },
        EventEnvelope {
            schema_version: SCREEN_OBSERVER_IDENTITY_SCHEMA_VERSION,
            sequence: 3,
            event_id: "event-0000000000000003".to_string(),
            occurred_at_ms: 3,
            source_actor_id: MEMORY_AGENT_ID.to_string(),
            mission_id: None,
            run_id: None,
            interaction_scope_id: None,
            interaction_run_id: None,
            causation_id: Some("event-0000000000000002".to_string()),
            correlation_id: Some("fixture-canonical-v5-boundary".to_string()),
            event: RuntimeEvent::OrganizedMemoryCheckpointRecorded {
                checkpoint: canonical_v5_checkpoint,
                legacy_source_event_id: None,
                legacy_migration: None,
            },
        },
    ] {
        append_test_envelope(&temp, &envelope);
    }

    let runtime = temp.boot();
    assert_eq!(runtime.snapshot().agents.len(), 12);
    assert_eq!(
        raw_ledger_events(&temp)
            .iter()
            .filter(|envelope| {
                envelope.correlation_id.as_deref()
                    == Some("memory-screen-observer-identity-migration")
            })
            .count(),
        0,
        "an existing canonical v5 checkpoint must remain the migration boundary"
    );
    drop(runtime);
    temp.boot();
}

#[test]
fn v5_memory_rejects_legacy_evidence_actor_without_rejecting_literal_history_text() {
    let decision = |label: &str, evidence_actor: &str, value: serde_json::Value| {
        let mut engine = OrganizedMemoryDecisionEngine::new();
        engine
            .organize(MemoryCandidate {
                candidate_id: label.to_string(),
                kind: OrganizedMemoryKind::ContextualFact,
                subject: "audit".to_string(),
                predicate: label.to_string(),
                value,
                applicability: MemoryApplicability {
                    space_id: "personal".to_string(),
                    environment: BTreeMap::new(),
                    valid_from_ms: 1,
                    valid_until_ms: None,
                },
                importance: 0.5,
                confidence: 1.0,
                intent: MemoryCandidateIntent::Assert,
                target_memory_id: None,
                evidence: vec![MemoryEvidence {
                    event_id: format!("event:{label}"),
                    source_actor_id: evidence_actor.to_string(),
                    origin: MemoryEvidenceOrigin::AgentAction,
                    polarity: MemoryEvidencePolarity::Supports,
                    observed_at_ms: 1,
                    recorded_at_ms: 1,
                    reliability: 0.7,
                    mission_id: None,
                    run_id: None,
                }],
            })
            .unwrap()
            .decision
            .unwrap()
    };
    let write_current_ledger = |temp: &TempRuntime, decision| {
        std::fs::write(&temp.ledger, "").unwrap();
        append_test_envelope(
            temp,
            &EventEnvelope {
                schema_version: SCHEMA_VERSION,
                sequence: 1,
                event_id: "event-0000000000000001".to_string(),
                occurred_at_ms: 1,
                source_actor_id: KERNEL_ACTOR_ID.to_string(),
                mission_id: None,
                run_id: None,
                interaction_scope_id: None,
                interaction_run_id: None,
                causation_id: None,
                correlation_id: None,
                event: RuntimeEvent::RuntimeStarted { process_id: 7 },
            },
        );
        append_test_envelope(
            temp,
            &EventEnvelope {
                schema_version: SCHEMA_VERSION,
                sequence: 2,
                event_id: "event-0000000000000002".to_string(),
                occurred_at_ms: 2,
                source_actor_id: MEMORY_AGENT_ID.to_string(),
                mission_id: None,
                run_id: None,
                interaction_scope_id: None,
                interaction_run_id: None,
                causation_id: None,
                correlation_id: None,
                event: RuntimeEvent::OrganizedMemoryDecisionRecorded { decision },
            },
        );
    };

    let bad = TempRuntime::new("v5-memory-legacy-actor-rejected");
    write_current_ledger(
        &bad,
        decision(
            "legacy-actor",
            LEGACY_SURFACE_AGENT_ID,
            json!("ordinary value"),
        ),
    );
    assert!(PinvouOsRuntime::boot(bad.ledger.clone()).is_err());

    let literal = TempRuntime::new("v5-memory-legacy-literal-allowed");
    write_current_ledger(
        &literal,
        decision("legacy-literal", "actor:user", json!("agent:surface")),
    );
    assert!(PinvouOsRuntime::boot(literal.ledger.clone()).is_ok());
}

#[test]
fn future_runtime_schema_is_rejected_by_the_v5_downgrade_fence() {
    let temp = TempRuntime::new("future-schema-rejected");
    std::fs::write(&temp.ledger, "").unwrap();
    append_test_envelope(
        &temp,
        &EventEnvelope {
            schema_version: SCHEMA_VERSION + 1,
            sequence: 1,
            event_id: "event-0000000000000001".to_string(),
            occurred_at_ms: 1,
            source_actor_id: KERNEL_ACTOR_ID.to_string(),
            mission_id: None,
            run_id: None,
            interaction_scope_id: None,
            interaction_run_id: None,
            causation_id: None,
            correlation_id: None,
            event: RuntimeEvent::RuntimeStarted { process_id: 7 },
        },
    );
    let Err(error) = PinvouOsRuntime::boot(temp.ledger.clone()) else {
        panic!("future schema must not boot on the v5 writer")
    };
    assert!(error.to_string().contains("newer than supported"));
}

#[test]
fn capability_catalog_tells_available_temporary_and_unsupported_apart() {
    let temp = TempRuntime::new("capabilities");
    let runtime = temp.boot();

    let front = runtime.explain_capability("user.interact");
    assert_eq!(front.state, CapabilityAvailabilityState::Available);
    let catalog = runtime.explain_capability("capability.explain");
    assert_eq!(catalog.state, CapabilityAvailabilityState::Available);
    let asr_context = runtime.explain_capability(ASR_CONTEXT_CAPABILITY_ID);
    assert_eq!(asr_context.state, CapabilityAvailabilityState::Available);
    let orchestrator = runtime.explain_capability("mission.orchestrate");
    assert_eq!(orchestrator.state, CapabilityAvailabilityState::Available);
    assert_eq!(
        runtime.explain_capability("network.observe").state,
        CapabilityAvailabilityState::Available
    );
    assert_eq!(
        runtime.explain_capability("inference.observe").state,
        CapabilityAvailabilityState::Available
    );
    let screen = runtime.explain_capability(SCREEN_OBSERVE_CAPABILITY_ID);
    assert_eq!(
        screen.state,
        CapabilityAvailabilityState::TemporarilyUnavailable
    );
    let legacy_screen_alias = runtime.explain_capability(LEGACY_SURFACE_OBSERVE_CAPABILITY_ID);
    assert_eq!(
        legacy_screen_alias.capability_id,
        SCREEN_OBSERVE_CAPABILITY_ID
    );
    assert_eq!(legacy_screen_alias.state, screen.state);
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
fn mission_registration_canonicalizes_legacy_screen_capability_before_persistence() {
    let temp = TempRuntime::new("mission-screen-observer-alias");
    let runtime = temp.boot();
    let started = runtime
        .open_mission(OpenMissionRequest {
            objective: "observe a legacy caller request".to_string(),
            priority: 50,
            deadline_at_ms: None,
        })
        .unwrap();
    let request = RegisterMissionAgentRequest {
        display_name: "Legacy caller executor".to_string(),
        role: "observe the visible UI".to_string(),
        capabilities: vec![capability(
            LEGACY_SURFACE_OBSERVE_CAPABILITY_ID,
            ResourceClass::Light,
        )],
        priority: 50,
        interruptibility: Interruptibility::Checkpoint,
        mission_id: started.mission.mission_id.clone(),
        run_id: started.run.run_id.clone(),
    };
    let registered = runtime.register_mission_agent(request.clone()).unwrap();
    assert_eq!(
        registered.capabilities[0].capability_id,
        SCREEN_OBSERVE_CAPABILITY_ID
    );
    assert_eq!(
        runtime.snapshot().agents[&registered.agent_id].capabilities[0].capability_id,
        SCREEN_OBSERVE_CAPABILITY_ID
    );
    let persisted = raw_ledger_events(&temp)
        .into_iter()
        .find_map(|envelope| match envelope.event {
            RuntimeEvent::AgentRegistered { agent } if agent.agent_id == registered.agent_id => {
                Some(agent)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(
        persisted.capabilities[0].capability_id,
        SCREEN_OBSERVE_CAPABILITY_ID
    );

    let mut duplicate_request = request;
    duplicate_request.display_name = "Duplicate legacy caller".to_string();
    duplicate_request.capabilities.push(capability(
        SCREEN_OBSERVE_CAPABILITY_ID,
        ResourceClass::Light,
    ));
    let error = runtime
        .register_mission_agent(duplicate_request)
        .unwrap_err()
        .to_string();
    assert!(error.contains("duplicate capability screen.observe"));
}

#[test]
fn connectivity_and_inference_agents_project_live_health_and_last_success() {
    let temp = TempRuntime::new("connectivity-inference");
    let runtime = temp.boot();
    let now = chrono::Utc::now().timestamp_millis();

    runtime
        .observe_connectivity(ConnectivityObservation {
            checked_at_ms: now,
            status: ConnectivityStatus::Online,
            latency_ms: Some(42),
            reason_code: None,
        })
        .unwrap();
    runtime
        .observe_inference_health(InferenceHealthObservation {
            checked_at_ms: now,
            status: InferenceStatus::Ready,
            model: Some("glm-5.2".to_string()),
            provider: Some("glm".to_string()),
            probe_latency_ms: Some(67),
            reason_code: None,
        })
        .unwrap();
    runtime
        .record_inference_completion(InferenceCompletionObservation {
            completed_at_ms: now,
            model: "glm-5.2".to_string(),
            latency_ms: 1_234,
        })
        .unwrap();

    let projected = runtime.snapshot();
    assert_eq!(projected.connectivity.status, ConnectivityStatus::Online);
    assert_eq!(projected.connectivity.latency_ms, Some(42));
    assert_eq!(projected.inference.status, InferenceStatus::Ready);
    assert_eq!(projected.inference.model.as_deref(), Some("glm-5.2"));
    assert_eq!(projected.inference.last_success_at_ms, Some(now));
    assert_eq!(projected.inference.last_success_latency_ms, Some(1_234));

    drop(runtime);
    let replayed = temp.boot().snapshot();
    assert_eq!(replayed.connectivity.status, ConnectivityStatus::Online);
    assert_eq!(replayed.inference.last_success_at_ms, Some(now));
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
fn hot_to_warm_to_normal_still_reconciles_a_resume() {
    let temp = TempRuntime::new("stepped-resume-governor");
    let runtime = temp.boot();
    let agent = mission_agent(
        &runtime,
        "background.index",
        20,
        Interruptibility::Immediate,
    );

    let hot = runtime.observe_resources(resources(90.0, 60.0)).unwrap();
    runtime
        .acknowledge_directive(&hot.directives[0].directive_id, true, "paused".to_string())
        .unwrap();
    let warm = runtime.observe_resources(resources(82.0, 60.0)).unwrap();
    assert_eq!(warm.pressure, ResourcePressure::Warm);
    assert!(warm.directives.is_empty());
    let normal = runtime.observe_resources(resources(60.0, 50.0)).unwrap();
    assert_eq!(normal.directives.len(), 1);
    assert_eq!(normal.directives[0].target_agent_id, agent.agent_id);
    assert_eq!(normal.directives[0].action, DirectiveAction::Resume);
}

#[test]
fn missing_sensor_cannot_clear_a_hot_claim_or_resume_work() {
    let temp = TempRuntime::new("missing-sensor-relief");
    let runtime = temp.boot();
    let agent = mission_agent(
        &runtime,
        "background.index",
        20,
        Interruptibility::Immediate,
    );
    let hot = runtime.observe_resources(resources(90.0, 60.0)).unwrap();
    runtime
        .acknowledge_directive(&hot.directives[0].directive_id, true, "paused".to_string())
        .unwrap();

    let mut incomplete = resources(60.0, 50.0);
    incomplete.temperature_c = None;
    let decision = runtime.observe_resources(incomplete).unwrap();
    assert_eq!(decision.pressure, ResourcePressure::Hot);
    assert!(decision.directives.is_empty());
    let projected = runtime.snapshot();
    assert_eq!(
        projected.agents[&agent.agent_id].observed_state,
        AgentState::Paused
    );
    assert!(projected.resources.active_pressure_claim_id.is_some());
}

#[test]
fn hot_reconciliation_catches_agents_registered_after_the_edge() {
    let temp = TempRuntime::new("hot-late-agent");
    let runtime = temp.boot();
    runtime.observe_resources(resources(90.0, 60.0)).unwrap();
    let late = mission_agent(&runtime, "late.background", 20, Interruptibility::Immediate);

    let decision = runtime.observe_resources(resources(90.0, 60.0)).unwrap();
    assert_eq!(decision.pressure, ResourcePressure::Hot);
    assert_eq!(decision.directives.len(), 1);
    assert_eq!(decision.directives[0].target_agent_id, late.agent_id);
    assert_eq!(decision.directives[0].action, DirectiveAction::Pause);
}

#[test]
fn rejected_adapter_directive_restores_the_observed_desire() {
    let temp = TempRuntime::new("rejected-directive");
    let runtime = temp.boot();
    let agent = mission_agent(
        &runtime,
        "background.index",
        20,
        Interruptibility::Immediate,
    );
    let hot = runtime.observe_resources(resources(90.0, 60.0)).unwrap();

    runtime
        .acknowledge_directive(
            &hot.directives[0].directive_id,
            false,
            "adapter unavailable".to_string(),
        )
        .unwrap();
    let projected = runtime.snapshot();
    assert_eq!(
        projected.agents[&agent.agent_id].observed_state,
        AgentState::Running
    );
    assert_eq!(
        projected.agents[&agent.agent_id].desired_state,
        AgentState::Running
    );
}

#[test]
fn concurrent_acknowledgements_commit_exactly_once() {
    let temp = TempRuntime::new("directive-concurrent-ack");
    let runtime = temp.boot();
    mission_agent(
        &runtime,
        "background.concurrent-ack",
        20,
        Interruptibility::Immediate,
    );
    let decision = runtime.observe_resources(resources(90.0, 60.0)).unwrap();
    let directive_id = decision.directives[0].directive_id.clone();

    let start = Arc::new(Barrier::new(3));
    let acknowledgements = ["worker-a", "worker-b"].map(|detail| {
        let worker_runtime = runtime.clone();
        let worker_directive_id = directive_id.clone();
        let worker_start = start.clone();
        std::thread::spawn(move || {
            worker_start.wait();
            worker_runtime.acknowledge_directive(&worker_directive_id, true, detail.to_string())
        })
    });
    start.wait();
    let results = acknowledgements.map(|worker| worker.join().unwrap());

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
    assert_eq!(
        runtime.snapshot().directives[&directive_id].status,
        DirectiveStatus::Applied
    );
    assert_eq!(
        raw_ledger_events(&temp)
            .iter()
            .filter(|envelope| matches!(
                &envelope.event,
                RuntimeEvent::DirectiveAcknowledged {
                    directive_id: acknowledged_id,
                    ..
                } if acknowledged_id == &directive_id
            ))
            .count(),
        1
    );
}

#[test]
fn mission_governor_only_persists_pending_until_a_trusted_async_owner_acknowledges() {
    let temp = TempRuntime::new("mission-directive-pending");
    let runtime = temp.boot();
    let agent = mission_agent(
        &runtime,
        "background.index",
        20,
        Interruptibility::Immediate,
    );

    let decision = runtime.observe_resources(resources(90.0, 60.0)).unwrap();
    let directive = decision.directives.first().expect("pause directive");
    assert_eq!(directive.target_agent_id, agent.agent_id);
    assert_eq!(directive.action, DirectiveAction::Pause);
    assert_eq!(directive.status, DirectiveStatus::Pending);
    assert_eq!(
        runtime.snapshot().agents[&agent.agent_id].observed_state,
        AgentState::Running,
        "desired control must not impersonate an adapter acknowledgement"
    );

    let acknowledged = runtime
        .acknowledge_directive(
            &directive.directive_id,
            true,
            "trusted async owner confirmed pause".to_string(),
        )
        .unwrap();
    assert_eq!(acknowledged.status, DirectiveStatus::Applied);
    assert_eq!(
        runtime.snapshot().agents[&agent.agent_id].observed_state,
        AgentState::Paused
    );
}

#[test]
fn pending_mission_directive_survives_reboot_without_inline_side_effect_replay() {
    let temp = TempRuntime::new("mission-directive-reboot");
    let directive_id = {
        let runtime = temp.boot();
        mission_agent(
            &runtime,
            "background.index",
            20,
            Interruptibility::Immediate,
        );
        runtime
            .observe_resources(resources(90.0, 60.0))
            .unwrap()
            .directives[0]
            .directive_id
            .clone()
    };

    let rebooted = temp.boot();
    assert_eq!(
        rebooted.snapshot().directives[&directive_id].status,
        DirectiveStatus::Pending
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
fn schema_v5_ledger_upcasts_to_v6_without_rewriting_old_bytes() {
    let temp = TempRuntime::new("host-work-v5-upcast");
    std::fs::write(&temp.ledger, "").unwrap();
    append_test_envelope(
        &temp,
        &EventEnvelope {
            schema_version: 5,
            sequence: 1,
            event_id: "event-0000000000000001".to_string(),
            occurred_at_ms: 1,
            source_actor_id: KERNEL_ACTOR_ID.to_string(),
            mission_id: None,
            run_id: None,
            interaction_scope_id: None,
            interaction_run_id: None,
            causation_id: None,
            correlation_id: None,
            event: RuntimeEvent::RuntimeStarted { process_id: 7 },
        },
    );
    let legacy_prefix = std::fs::read(&temp.ledger).unwrap();

    let runtime = temp.boot();
    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.schema_version, 6);
    assert!(snapshot.host_works.is_empty());
    assert!(snapshot.host_work_directives.is_empty());
    let upgraded_bytes = std::fs::read(&temp.ledger).unwrap();
    assert_eq!(&upgraded_bytes[..legacy_prefix.len()], legacy_prefix);
    assert!(raw_ledger_events(&temp)
        .iter()
        .skip(1)
        .all(|event| event.schema_version == 6));
}

#[test]
fn host_work_control_requires_ack_and_reconciliation_before_observed_state_changes() {
    let temp = TempRuntime::new("host-work-control-lifecycle");
    let runtime = temp.boot();
    let (handle, work) = runtime
        .register_host_work(host_work_request(
            "feature:scheduled:lifecycle",
            20,
            Interruptibility::Checkpoint,
            false,
            true,
            &[
                HostWorkAction::Pause,
                HostWorkAction::Resume,
                HostWorkAction::Stop,
            ],
        ))
        .unwrap();
    assert!(work.work_id.starts_with("host-work-"));
    assert_eq!(work.generation, 1);

    let pause_request =
        HostWorkDirectiveRequest::new(HostWorkAction::Pause, "hot pressure", "test-policy:v1");
    let pause_id = pause_request.directive_id().to_string();
    runtime
        .issue_host_work_directive(&handle, pause_request.clone())
        .unwrap();
    let after_issue = runtime.snapshot();
    assert_eq!(
        after_issue.host_works[handle.work_id()].desired_state,
        HostWorkDesiredState::Paused
    );
    assert_eq!(
        after_issue.host_works[handle.work_id()].observed_state,
        HostWorkObservedState::Running
    );
    assert_eq!(
        runtime.pending_host_work_directives(&handle).unwrap().len(),
        1
    );

    let events_before_issue_retry = raw_ledger_events(&temp).len();
    runtime
        .issue_host_work_directive(&handle, pause_request)
        .unwrap();
    assert_eq!(raw_ledger_events(&temp).len(), events_before_issue_retry);

    runtime
        .acknowledge_host_work_directive(
            &handle,
            &pause_id,
            HostWorkDirectiveAcknowledgement::Applied,
            "pause request accepted".to_string(),
        )
        .unwrap();
    assert_eq!(
        runtime.snapshot().host_works[handle.work_id()].observed_state,
        HostWorkObservedState::Running
    );
    assert_eq!(
        runtime.snapshot().host_work_directives[&pause_id].status,
        HostWorkDirectiveStatus::AwaitingReconciliation
    );
    runtime
        .reconcile_host_work_directive(
            &handle,
            &pause_id,
            ReconcileHostWorkDirectiveRequest {
                outcome: HostWorkReconciliationOutcome::Confirmed,
                observed_state: Some(HostWorkObservedState::Paused),
                detail: "status confirms paused".to_string(),
            },
        )
        .unwrap();
    let paused = runtime.snapshot();
    assert_eq!(
        paused.host_works[handle.work_id()].observed_state,
        HostWorkObservedState::Paused
    );
    assert_eq!(
        paused.host_works[handle.work_id()]
            .governor_pause_directive_id
            .as_deref(),
        Some(pause_id.as_str())
    );

    let resume_request =
        HostWorkDirectiveRequest::new(HostWorkAction::Resume, "normal pressure", "test-policy:v1");
    let resume_id = resume_request.directive_id().to_string();
    runtime
        .issue_host_work_directive(&handle, resume_request)
        .unwrap();
    runtime
        .acknowledge_host_work_directive(
            &handle,
            &resume_id,
            HostWorkDirectiveAcknowledgement::OutcomeUnknown,
            "transport closed before receipt".to_string(),
        )
        .unwrap();
    assert!(runtime
        .pending_host_work_directives(&handle)
        .unwrap()
        .is_empty());
    assert_eq!(
        runtime
            .host_work_directives_requiring_reconciliation(&handle)
            .unwrap()
            .iter()
            .map(|directive| directive.directive_id.as_str())
            .collect::<Vec<_>>(),
        vec![resume_id.as_str()]
    );

    let unknown = ReconcileHostWorkDirectiveRequest {
        outcome: HostWorkReconciliationOutcome::OutcomeUnknown,
        observed_state: None,
        detail: "status endpoint unavailable".to_string(),
    };
    runtime
        .reconcile_host_work_directive(&handle, &resume_id, unknown.clone())
        .unwrap();
    let events_before_unknown_retry = raw_ledger_events(&temp).len();
    runtime
        .reconcile_host_work_directive(&handle, &resume_id, unknown)
        .unwrap();
    assert_eq!(raw_ledger_events(&temp).len(), events_before_unknown_retry);
    assert_eq!(
        runtime.snapshot().host_works[handle.work_id()].observed_state,
        HostWorkObservedState::Paused
    );

    runtime
        .reconcile_host_work_directive(
            &handle,
            &resume_id,
            ReconcileHostWorkDirectiveRequest {
                outcome: HostWorkReconciliationOutcome::Confirmed,
                observed_state: Some(HostWorkObservedState::Running),
                detail: "same request id status confirms running".to_string(),
            },
        )
        .unwrap();
    let resumed = runtime.snapshot();
    assert_eq!(
        resumed.host_works[handle.work_id()].observed_state,
        HostWorkObservedState::Running
    );
    assert_eq!(
        resumed.host_work_directives[&resume_id].status,
        HostWorkDirectiveStatus::Reconciled
    );
    assert_eq!(resumed.host_work_directives.len(), 2);
}

#[test]
fn host_work_generation_rejects_old_handles_and_old_directives() {
    let temp = TempRuntime::new("host-work-generation");
    let runtime = temp.boot();
    let (first_handle, first) = runtime
        .register_host_work(host_work_request(
            "feature:connector:generation",
            40,
            Interruptibility::Immediate,
            false,
            true,
            &[HostWorkAction::Stop],
        ))
        .unwrap();
    let stop =
        HostWorkDirectiveRequest::new(HostWorkAction::Stop, "generation test", "test-policy:v1");
    let stop_id = stop.directive_id().to_string();
    runtime
        .issue_host_work_directive(&first_handle, stop)
        .unwrap();
    runtime
        .acknowledge_host_work_directive(
            &first_handle,
            &stop_id,
            HostWorkDirectiveAcknowledgement::Rejected,
            "work remained running".to_string(),
        )
        .unwrap();

    assert!(runtime
        .renew_host_work_registration(&first_handle, HostWorkObservedState::Running)
        .is_err());
    runtime
        .observe_host_work(
            &first_handle,
            HostWorkObservedState::Stopped,
            "previous physical generation stopped".to_string(),
        )
        .unwrap();

    let (second_handle, second) = runtime
        .renew_host_work_registration(&first_handle, HostWorkObservedState::Running)
        .unwrap();
    assert_eq!(second.work_id, first.work_id);
    assert_eq!(second.generation, first.generation + 1);
    assert!(runtime
        .observe_host_work(
            &first_handle,
            HostWorkObservedState::Running,
            "stale handle".to_string(),
        )
        .is_err());
    assert!(runtime
        .acknowledge_host_work_directive(
            &second_handle,
            &stop_id,
            HostWorkDirectiveAcknowledgement::Rejected,
            "old directive".to_string(),
        )
        .is_err());
}

#[test]
fn host_work_directives_are_single_flight_and_retry_only_after_pressure_changes() {
    let temp = TempRuntime::new("host-work-single-flight");
    let runtime = temp.boot();
    let (handle, _) = runtime
        .register_host_work(host_work_request(
            "feature:scheduled:single-flight",
            20,
            Interruptibility::Checkpoint,
            false,
            true,
            &[HostWorkAction::Pause, HostWorkAction::Stop],
        ))
        .unwrap();

    let pause =
        HostWorkDirectiveRequest::new(HostWorkAction::Pause, "first pause", "test-policy:v1");
    let pause_id = pause.directive_id().to_string();
    runtime.issue_host_work_directive(&handle, pause).unwrap();
    assert!(runtime
        .issue_host_work_directive(
            &handle,
            HostWorkDirectiveRequest::new(
                HostWorkAction::Stop,
                "must not supersede",
                "test-policy:v1",
            ),
        )
        .is_err());

    runtime
        .acknowledge_host_work_directive(
            &handle,
            &pause_id,
            HostWorkDirectiveAcknowledgement::Rejected,
            "adapter refused pause".to_string(),
        )
        .unwrap();
    assert!(runtime
        .reconcile_host_work_directive(
            &handle,
            &pause_id,
            ReconcileHostWorkDirectiveRequest {
                outcome: HostWorkReconciliationOutcome::Confirmed,
                observed_state: Some(HostWorkObservedState::Paused),
                detail: "rejected is already definitive".to_string(),
            },
        )
        .is_err());
    assert!(runtime
        .issue_host_work_directive(
            &handle,
            HostWorkDirectiveRequest::new(
                HostWorkAction::Pause,
                "same pressure heartbeat",
                "test-policy:v1",
            ),
        )
        .is_err());

    let warm = runtime.observe_resources(resources(81.0, 60.0)).unwrap();
    assert_eq!(warm.pressure, ResourcePressure::Warm);
    assert!(warm.host_work_directives.is_empty());
    let retried = runtime
        .issue_host_work_directive(
            &handle,
            HostWorkDirectiveRequest::new(
                HostWorkAction::Pause,
                "new pressure epoch",
                "test-policy:v1",
            ),
        )
        .unwrap();
    assert_eq!(retried.resource_pressure_epoch, 1);
}

#[test]
fn host_work_terminal_state_precedes_renewal_unregistration_and_identity_retirement() {
    let temp = TempRuntime::new("host-work-terminal-generation");
    let runtime = temp.boot();
    let (first_handle, first) = runtime
        .register_host_work(host_work_request(
            "feature:connector:terminal-generation",
            30,
            Interruptibility::Immediate,
            false,
            true,
            &[HostWorkAction::Stop],
        ))
        .unwrap();

    assert!(runtime
        .renew_host_work_registration(&first_handle, HostWorkObservedState::Running)
        .is_err());
    assert!(runtime
        .unregister_host_work(&first_handle, "still running".to_string())
        .is_err());
    runtime
        .observe_host_work(
            &first_handle,
            HostWorkObservedState::Completed,
            "first generation completed".to_string(),
        )
        .unwrap();
    assert!(runtime
        .observe_host_work(
            &first_handle,
            HostWorkObservedState::Running,
            "terminal state cannot revive".to_string(),
        )
        .is_err());

    let (second_handle, second) = runtime
        .renew_host_work_registration(&first_handle, HostWorkObservedState::Running)
        .unwrap();
    assert_eq!(second.work_id, first.work_id);
    assert_eq!(second.generation, 2);
    runtime
        .observe_host_work(
            &second_handle,
            HostWorkObservedState::Stopped,
            "second generation stopped".to_string(),
        )
        .unwrap();
    runtime
        .unregister_host_work(&second_handle, "descriptor retired".to_string())
        .unwrap();

    let mut events = raw_ledger_events(&temp);
    let reused = events
        .iter()
        .find(|envelope| {
            matches!(
                &envelope.event,
                RuntimeEvent::HostWorkRegistered { work } if work.work_id == first.work_id
            )
        })
        .cloned()
        .unwrap();
    events.push(reused);
    assert!(super::runtime::validate_host_work_events_for_test(&events).is_err());
}

#[test]
fn host_work_governor_pause_ownership_is_cleared_by_external_resume() {
    let temp = TempRuntime::new("host-work-pause-ownership");
    let runtime = temp.boot();
    let (handle, _) = runtime
        .register_host_work(host_work_request(
            "feature:knowledge:pause-ownership",
            20,
            Interruptibility::Checkpoint,
            false,
            true,
            &[HostWorkAction::Pause, HostWorkAction::Resume],
        ))
        .unwrap();
    let pause = HostWorkDirectiveRequest::new(HostWorkAction::Pause, "pause", "test-policy:v1");
    let pause_id = pause.directive_id().to_string();
    runtime.issue_host_work_directive(&handle, pause).unwrap();
    runtime
        .acknowledge_host_work_directive(
            &handle,
            &pause_id,
            HostWorkDirectiveAcknowledgement::Applied,
            "accepted".to_string(),
        )
        .unwrap();
    runtime
        .reconcile_host_work_directive(
            &handle,
            &pause_id,
            ReconcileHostWorkDirectiveRequest {
                outcome: HostWorkReconciliationOutcome::Confirmed,
                observed_state: Some(HostWorkObservedState::Paused),
                detail: "paused".to_string(),
            },
        )
        .unwrap();
    assert!(runtime.snapshot().host_works[handle.work_id()]
        .governor_pause_directive_id
        .is_some());

    runtime
        .observe_host_work(
            &handle,
            HostWorkObservedState::Running,
            "resumed outside Governor".to_string(),
        )
        .unwrap();
    assert!(runtime.snapshot().host_works[handle.work_id()]
        .governor_pause_directive_id
        .is_none());
    let normal = runtime.observe_resources(resources(40.0, 20.0)).unwrap();
    assert!(normal.host_work_directives.is_empty());
}

#[test]
fn newly_registered_host_work_reconciles_immediately_against_current_pressure() {
    let temp = TempRuntime::new("host-work-registration-pressure");
    let runtime = temp.boot();
    let critical = runtime.observe_resources(resources(96.0, 50.0)).unwrap();
    assert_eq!(critical.pressure, ResourcePressure::Critical);
    assert!(critical.host_work_directives.is_empty());

    let (handle, _) = runtime
        .register_host_work(host_work_request(
            "feature:scheduled:late-critical",
            20,
            Interruptibility::Immediate,
            false,
            true,
            &[HostWorkAction::Stop],
        ))
        .unwrap();
    let directive = runtime
        .reconcile_host_work_governance(&handle)
        .unwrap()
        .expect("critical pressure must immediately govern the new work");
    assert_eq!(directive.action, HostWorkAction::Stop);
    assert_eq!(directive.resource_pressure_epoch, 1);
    assert!(runtime
        .reconcile_host_work_governance(&handle)
        .unwrap()
        .is_none());
}

#[test]
fn host_work_registered_during_resource_commit_is_governed_without_heartbeat_gap() {
    use std::sync::{Arc, Barrier};

    let temp = TempRuntime::new("host-work-registration-pressure-race");
    let runtime = temp.boot();
    let observation_entered = Arc::new(Barrier::new(2));
    let allow_observation_commit = Arc::new(Barrier::new(2));
    let entered = Arc::clone(&observation_entered);
    let release = Arc::clone(&allow_observation_commit);
    runtime.set_resource_observation_before_commit_hook(Some(Arc::new(move || {
        entered.wait();
        release.wait();
    })));

    let observing = runtime.clone();
    let observation_thread = std::thread::spawn(move || {
        observing
            .observe_resources(resources(96.0, 50.0))
            .expect("critical observation")
    });
    observation_entered.wait();

    let (handle, _) = runtime
        .register_host_work(host_work_request(
            "feature:scheduled:during-critical-commit",
            20,
            Interruptibility::Immediate,
            false,
            true,
            &[HostWorkAction::Stop],
        ))
        .unwrap();
    assert!(runtime
        .reconcile_host_work_governance(&handle)
        .unwrap()
        .is_none());

    allow_observation_commit.wait();
    let decision = observation_thread.join().expect("observation thread");
    runtime.set_resource_observation_before_commit_hook(None);

    assert_eq!(decision.pressure, ResourcePressure::Critical);
    assert_eq!(decision.host_work_directives.len(), 1);
    let directive = &decision.host_work_directives[0];
    assert_eq!(directive.work_id, handle.work_id());
    assert_eq!(directive.generation, handle.generation());
    assert_eq!(directive.action, HostWorkAction::Stop);
    assert_eq!(directive.resource_pressure_epoch, 1);
    assert!(runtime
        .reconcile_host_work_governance(&handle)
        .unwrap()
        .is_none());
}

#[test]
fn resource_observation_and_registration_reconcile_race_issue_exactly_one_directive() {
    let temp = TempRuntime::new("host-work-governance-issue-race");
    let runtime = temp.boot();
    let (handle, _) = runtime
        .register_host_work(host_work_request(
            "feature:scheduled:governance-issue-race",
            20,
            Interruptibility::Immediate,
            false,
            true,
            &[HostWorkAction::Stop],
        ))
        .unwrap();
    let governance_snapshot_taken = Arc::new(Barrier::new(2));
    let allow_resource_governance = Arc::new(Barrier::new(2));
    let entered = Arc::clone(&governance_snapshot_taken);
    let release = Arc::clone(&allow_resource_governance);
    runtime.set_resource_observation_before_governance_issue_hook(Some(Arc::new(move || {
        entered.wait();
        release.wait();
    })));

    let observing = runtime.clone();
    let observation_thread = std::thread::spawn(move || {
        observing
            .observe_resources(resources(96.0, 50.0))
            .expect("critical observation must not fail when reconcile wins the issue race")
    });
    governance_snapshot_taken.wait();
    let reconciled = runtime
        .reconcile_host_work_governance(&handle)
        .unwrap()
        .expect("registration reconcile should atomically issue the directive");
    allow_resource_governance.wait();
    let decision = observation_thread.join().unwrap();
    runtime.set_resource_observation_before_governance_issue_hook(None);

    assert!(decision.host_work_directives.is_empty());
    assert_eq!(runtime.snapshot().host_work_directives.len(), 1);
    assert_eq!(reconciled.work_id, handle.work_id());
    assert_eq!(reconciled.resource_pressure_epoch, 1);
}

#[test]
fn reboot_rebinds_exact_generation_and_recovers_original_pending_directive() {
    let temp = TempRuntime::new("host-work-rebind");
    let (work_id, generation, directive_id) = {
        let runtime = temp.boot();
        let (handle, work) = runtime
            .register_host_work(host_work_request(
                "feature:knowledge:rebind",
                25,
                Interruptibility::Checkpoint,
                false,
                true,
                &[HostWorkAction::Pause, HostWorkAction::Resume],
            ))
            .unwrap();
        let request =
            HostWorkDirectiveRequest::new(HostWorkAction::Pause, "hot pressure", "test-policy:v1");
        let directive_id = request.directive_id().to_string();
        runtime.issue_host_work_directive(&handle, request).unwrap();
        (work.work_id, work.generation, directive_id)
    };

    let rebooted = temp.boot();
    assert!(rebooted
        .rebind_host_work(
            "feature:knowledge:rebind",
            HostWorkKind::ScheduledRun,
            generation + 1,
        )
        .is_err());
    let (handle, rebound) = rebooted
        .rebind_host_work(
            "feature:knowledge:rebind",
            HostWorkKind::ScheduledRun,
            generation,
        )
        .unwrap();
    assert_eq!(rebound.work_id, work_id);
    assert_eq!(rebound.generation, generation);
    let pending = rebooted.pending_host_work_directives(&handle).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].directive_id, directive_id);
    assert!(rebooted
        .register_host_work(host_work_request(
            "feature:knowledge:rebind",
            25,
            Interruptibility::Checkpoint,
            false,
            true,
            &[HostWorkAction::Pause, HostWorkAction::Resume],
        ))
        .is_err());
}

#[test]
fn host_work_governor_filters_candidates_and_does_not_duplicate_heartbeats() {
    let temp = TempRuntime::new("host-work-governor-hot");
    let runtime = temp.boot();
    let (low_handle, low) = runtime
        .register_host_work(host_work_request(
            "feature:scheduled:low",
            20,
            Interruptibility::Checkpoint,
            false,
            true,
            &[
                HostWorkAction::Pause,
                HostWorkAction::Resume,
                HostWorkAction::Stop,
            ],
        ))
        .unwrap();
    runtime
        .register_host_work(host_work_request(
            "feature:scheduled:high",
            95,
            Interruptibility::Immediate,
            false,
            true,
            &[
                HostWorkAction::Pause,
                HostWorkAction::Resume,
                HostWorkAction::Stop,
            ],
        ))
        .unwrap();
    runtime
        .register_host_work(host_work_request(
            "feature:scheduled:atomic",
            10,
            Interruptibility::Atomic,
            false,
            true,
            &[HostWorkAction::Stop],
        ))
        .unwrap();
    runtime
        .register_host_work(host_work_request(
            "feature:scheduled:essential",
            10,
            Interruptibility::Checkpoint,
            true,
            true,
            &[
                HostWorkAction::Pause,
                HostWorkAction::Resume,
                HostWorkAction::Stop,
            ],
        ))
        .unwrap();
    runtime
        .register_host_work(host_work_request(
            "feature:scheduled:observe-only",
            10,
            Interruptibility::Checkpoint,
            false,
            false,
            &[
                HostWorkAction::Pause,
                HostWorkAction::Resume,
                HostWorkAction::Stop,
            ],
        ))
        .unwrap();

    let hot = runtime.observe_resources(resources(89.0, 60.0)).unwrap();
    assert_eq!(hot.host_work_directives.len(), 1);
    assert_eq!(hot.host_work_directives[0].work_id, low.work_id);
    assert_eq!(hot.host_work_directives[0].action, HostWorkAction::Pause);
    let repeated = runtime.observe_resources(resources(89.0, 60.0)).unwrap();
    assert!(repeated.host_work_directives.is_empty());
    assert_eq!(
        runtime
            .pending_host_work_directives(&low_handle)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn critical_host_work_governor_stops_only_nonessential_governable_work() {
    let temp = TempRuntime::new("host-work-governor-critical");
    let runtime = temp.boot();
    for (owner, priority, interruptibility, essential, governable) in [
        (
            "feature:critical:low",
            10,
            Interruptibility::Checkpoint,
            false,
            true,
        ),
        (
            "feature:critical:high",
            100,
            Interruptibility::Immediate,
            false,
            true,
        ),
        (
            "feature:critical:atomic",
            100,
            Interruptibility::Atomic,
            false,
            true,
        ),
        (
            "feature:critical:essential",
            10,
            Interruptibility::Checkpoint,
            true,
            true,
        ),
        (
            "feature:critical:observe-only",
            10,
            Interruptibility::Checkpoint,
            false,
            false,
        ),
    ] {
        runtime
            .register_host_work(host_work_request(
                owner,
                priority,
                interruptibility,
                essential,
                governable,
                &[HostWorkAction::Stop],
            ))
            .unwrap();
    }
    let critical = runtime.observe_resources(resources(96.0, 50.0)).unwrap();
    assert_eq!(critical.pressure, ResourcePressure::Critical);
    assert_eq!(critical.host_work_directives.len(), 3);
    assert!(critical
        .host_work_directives
        .iter()
        .all(|directive| directive.action == HostWorkAction::Stop));
    let repeated = runtime.observe_resources(resources(96.0, 50.0)).unwrap();
    assert!(repeated.host_work_directives.is_empty());
}

#[test]
fn app_cgroup_edge_stops_nonessential_host_work_while_system_memory_is_low() {
    let temp = TempRuntime::new("app-cgroup-low-system-stop");
    let runtime = temp.boot();
    runtime
        .register_host_work(host_work_request(
            "feature:cgroup:background",
            20,
            Interruptibility::Immediate,
            false,
            true,
            &[HostWorkAction::Stop],
        ))
        .unwrap();
    runtime
        .register_host_work(host_work_request(
            "feature:cgroup:essential",
            20,
            Interruptibility::Immediate,
            true,
            true,
            &[HostWorkAction::Stop],
        ))
        .unwrap();
    let base = chrono::Utc::now().timestamp_millis();

    let baseline = runtime
        .observe_resources(resources_with_app_cgroup(base, 2_000, 4_000, 10, 1, 0))
        .unwrap();
    assert_eq!(baseline.pressure, ResourcePressure::Normal);
    assert!(baseline.host_work_directives.is_empty());

    let critical = runtime
        .observe_resources(resources_with_app_cgroup(base + 1, 2_100, 4_000, 11, 1, 0))
        .unwrap();
    assert_eq!(critical.pressure, ResourcePressure::Critical);
    assert_eq!(critical.host_work_directives.len(), 1);
    assert_eq!(
        critical.host_work_directives[0].action,
        HostWorkAction::Stop
    );
    assert!(critical.host_work_directives[0]
        .reason
        .contains("app_cgroup_memory_high_event"));

    let snapshot = runtime.snapshot();
    assert!(snapshot.resources.app_cgroup_critical);
    let claim = snapshot
        .claims
        .get(
            snapshot
                .resources
                .active_pressure_claim_id
                .as_deref()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(claim.value["memoryUsedPct"], 20.0);
    assert_eq!(
        claim.value["appCgroup"]["instanceGeneration"],
        "0123456789abcdef0123456789abcdef"
    );
    assert_eq!(claim.value["appCgroup"]["memoryEventDeltas"]["high"], 1);
}

#[test]
fn app_cgroup_critical_holds_on_missing_and_relaxes_only_on_trusted_same_instance_relief() {
    let temp = TempRuntime::new("app-cgroup-sticky-relief");
    let runtime = temp.boot();
    let base = chrono::Utc::now().timestamp_millis();
    runtime
        .observe_resources(resources_with_app_cgroup(base, 2_000, 4_000, 10, 1, 0))
        .unwrap();
    runtime
        .observe_resources(resources_with_app_cgroup(base + 1, 4_100, 4_000, 10, 1, 0))
        .unwrap();

    let mut missing_observation = resources(50.0, 20.0);
    missing_observation.sampled_at_ms = base + 2;
    let missing = runtime.observe_resources(missing_observation).unwrap();
    assert_eq!(missing.pressure, ResourcePressure::Critical);
    assert!(runtime.snapshot().resources.app_cgroup_critical);

    let relieved = runtime
        .observe_resources(resources_with_app_cgroup(base + 3, 3_000, 4_000, 10, 1, 0))
        .unwrap();
    assert_eq!(relieved.pressure, ResourcePressure::Normal);
    assert!(!runtime.snapshot().resources.app_cgroup_critical);
}

#[test]
fn fresh_cgroup_evidence_allows_one_rejected_stop_retry_and_replays_the_bound() {
    let temp = TempRuntime::new("app-cgroup-actionable-epoch");
    let runtime = temp.boot();
    let (handle, _) = runtime
        .register_host_work(host_work_request(
            "feature:cgroup:retry",
            20,
            Interruptibility::Immediate,
            false,
            true,
            &[HostWorkAction::Stop],
        ))
        .unwrap();
    let base = chrono::Utc::now().timestamp_millis();
    runtime
        .observe_resources(resources_with_app_cgroup(base, 2_000, 4_000, 10, 1, 0))
        .unwrap();
    let first = runtime
        .observe_resources(resources_with_app_cgroup(base + 1, 4_100, 4_000, 10, 1, 0))
        .unwrap();
    assert_eq!(first.host_work_directives.len(), 1);
    assert_eq!(first.host_work_directives[0].resource_pressure_epoch, 1);
    runtime
        .acknowledge_host_work_directive(
            &handle,
            &first.host_work_directives[0].directive_id,
            HostWorkDirectiveAcknowledgement::Rejected,
            "fixed target refused stop".to_string(),
        )
        .unwrap();

    let sustained = runtime
        .observe_resources(resources_with_app_cgroup(base + 2, 4_200, 4_000, 10, 1, 0))
        .unwrap();
    assert_eq!(runtime.snapshot().resources.pressure_epoch, 1);
    assert_eq!(sustained.host_work_directives.len(), 1);
    assert_eq!(sustained.host_work_directives[0].resource_pressure_epoch, 1);
    runtime
        .acknowledge_host_work_directive(
            &handle,
            &sustained.host_work_directives[0].directive_id,
            HostWorkDirectiveAcknowledgement::Rejected,
            "retry also rejected".to_string(),
        )
        .unwrap();

    for index in 0..100_u64 {
        let repeated_edge = runtime
            .observe_resources(resources_with_app_cgroup(
                base + 4 + i64::try_from(index).unwrap(),
                4_300,
                4_000,
                11 + index,
                1,
                0,
            ))
            .unwrap();
        assert!(repeated_edge.host_work_directives.is_empty());
    }
    assert_eq!(runtime.snapshot().resources.pressure_epoch, 1);
    assert_eq!(runtime.snapshot().host_work_directives.len(), 2);
    drop(runtime);

    let replayed = temp.boot();
    assert_eq!(replayed.snapshot().resources.pressure_epoch, 1);
    let (rebound, _) = replayed
        .rebind_host_work("feature:cgroup:retry", HostWorkKind::ScheduledRun, 1)
        .unwrap();
    let after_replay = replayed
        .observe_resources(resources_with_app_cgroup(
            base + 105,
            4_300,
            4_000,
            200,
            1,
            0,
        ))
        .unwrap();
    assert!(after_replay.host_work_directives.is_empty());
    assert_eq!(replayed.snapshot().resources.pressure_epoch, 1);
    assert_eq!(replayed.snapshot().host_work_directives.len(), 2);
    assert_eq!(rebound.generation(), 1);
}

#[test]
fn first_above_high_cgroup_baseline_authorizes_retry_without_changing_pressure_epoch() {
    let temp = TempRuntime::new("app-cgroup-baseline-existing-critical");
    let runtime = temp.boot();
    let (handle, _) = runtime
        .register_host_work(host_work_request(
            "feature:cgroup:existing-critical-retry",
            20,
            Interruptibility::Immediate,
            false,
            true,
            &[HostWorkAction::Stop],
        ))
        .unwrap();
    let system = runtime.observe_resources(resources(96.0, 50.0)).unwrap();
    assert_eq!(system.pressure, ResourcePressure::Critical);
    assert_eq!(runtime.snapshot().resources.pressure_epoch, 1);
    runtime
        .acknowledge_host_work_directive(
            &handle,
            &system.host_work_directives[0].directive_id,
            HostWorkDirectiveAcknowledgement::Rejected,
            "initial system-pressure stop rejected".to_string(),
        )
        .unwrap();
    let base = chrono::Utc::now().timestamp_millis();

    let retry = runtime
        .observe_resources(resources_with_app_cgroup(base, 4_100, 4_000, 10, 1, 0))
        .unwrap();
    assert_eq!(runtime.snapshot().resources.pressure_epoch, 1);
    assert_eq!(retry.host_work_directives.len(), 1);
    assert_eq!(retry.host_work_directives[0].resource_pressure_epoch, 1);
    runtime
        .acknowledge_host_work_directive(
            &handle,
            &retry.host_work_directives[0].directive_id,
            HostWorkDirectiveAcknowledgement::Rejected,
            "baseline-authorized retry rejected".to_string(),
        )
        .unwrap();
    let repeated = runtime
        .observe_resources(resources_with_app_cgroup(base + 1, 4_200, 4_000, 10, 1, 0))
        .unwrap();
    assert!(repeated.host_work_directives.is_empty());
    assert_eq!(runtime.snapshot().resources.pressure_epoch, 1);
}

#[test]
fn outcome_unknown_host_work_does_not_authorize_cgroup_edge_identity_churn() {
    let temp = TempRuntime::new("app-cgroup-outcome-unknown-epoch");
    let runtime = temp.boot();
    let (handle, _) = runtime
        .register_host_work(host_work_request(
            "feature:cgroup:outcome-unknown",
            20,
            Interruptibility::Immediate,
            false,
            true,
            &[HostWorkAction::Stop],
        ))
        .unwrap();
    let base = chrono::Utc::now().timestamp_millis();
    runtime
        .observe_resources(resources_with_app_cgroup(base, 2_000, 4_000, 10, 1, 0))
        .unwrap();
    let first = runtime
        .observe_resources(resources_with_app_cgroup(base + 1, 4_100, 4_000, 10, 1, 0))
        .unwrap();
    runtime
        .acknowledge_host_work_directive(
            &handle,
            &first.host_work_directives[0].directive_id,
            HostWorkDirectiveAcknowledgement::OutcomeUnknown,
            "transport closed after dispatch".to_string(),
        )
        .unwrap();

    let later_edge = runtime
        .observe_resources(resources_with_app_cgroup(base + 2, 4_200, 4_000, 11, 1, 0))
        .unwrap();
    assert_eq!(runtime.snapshot().resources.pressure_epoch, 1);
    assert!(later_edge.host_work_directives.is_empty());
    assert_eq!(runtime.snapshot().host_work_directives.len(), 1);
}

#[test]
fn manual_policy_and_stale_generation_rejections_do_not_consume_governor_retry_budget() {
    let temp = TempRuntime::new("app-cgroup-retry-authority");
    let runtime = temp.boot();
    runtime.observe_resources(resources(96.0, 50.0)).unwrap();
    let (manual_handle, _) = runtime
        .register_host_work(host_work_request(
            "feature:cgroup:manual-policy",
            20,
            Interruptibility::Immediate,
            false,
            true,
            &[HostWorkAction::Stop],
        ))
        .unwrap();
    let manual = HostWorkDirectiveRequest::new(
        HostWorkAction::Stop,
        "trusted manual test directive",
        "manual-policy:v1",
    );
    let manual_id = manual.directive_id().to_string();
    runtime
        .issue_host_work_directive(&manual_handle, manual)
        .unwrap();
    runtime
        .acknowledge_host_work_directive(
            &manual_handle,
            &manual_id,
            HostWorkDirectiveAcknowledgement::Rejected,
            "manual action rejected".to_string(),
        )
        .unwrap();
    let base = chrono::Utc::now().timestamp_millis();
    let initial_governor = runtime
        .observe_resources(resources_with_app_cgroup(base, 4_100, 4_000, 10, 1, 0))
        .unwrap();
    assert_eq!(runtime.snapshot().resources.pressure_epoch, 1);
    assert_eq!(initial_governor.host_work_directives.len(), 1);
    assert_eq!(
        initial_governor.host_work_directives[0].work_id,
        manual_handle.work_id()
    );

    let (old_handle, _) = runtime
        .register_host_work(host_work_request(
            "feature:cgroup:stale-generation",
            20,
            Interruptibility::Immediate,
            false,
            true,
            &[HostWorkAction::Stop],
        ))
        .unwrap();
    let old = HostWorkDirectiveRequest::new(
        HostWorkAction::Stop,
        "old generation governor-shaped directive",
        super::governor::RESOURCE_GOVERNOR_POLICY_REVISION,
    );
    let old_id = old.directive_id().to_string();
    runtime.issue_host_work_directive(&old_handle, old).unwrap();
    runtime
        .acknowledge_host_work_directive(
            &old_handle,
            &old_id,
            HostWorkDirectiveAcknowledgement::Rejected,
            "old generation rejected".to_string(),
        )
        .unwrap();
    runtime
        .observe_host_work(
            &old_handle,
            HostWorkObservedState::Stopped,
            "old physical generation stopped".to_string(),
        )
        .unwrap();
    let new_generation = runtime
        .renew_host_work_registration(&old_handle, HostWorkObservedState::Running)
        .unwrap()
        .0;
    let renewed_governance = runtime
        .observe_resources(resources_with_app_cgroup(base + 1, 4_200, 4_000, 11, 1, 0))
        .unwrap();
    assert_eq!(runtime.snapshot().resources.pressure_epoch, 1);
    assert_eq!(renewed_governance.host_work_directives.len(), 1);
    assert_eq!(
        renewed_governance.host_work_directives[0].work_id,
        new_generation.work_id()
    );
    assert_eq!(renewed_governance.host_work_directives[0].generation, 2);
}

#[test]
fn rejected_ack_during_resource_commit_authorizes_exactly_one_edge_retry() {
    let temp = TempRuntime::new("app-cgroup-ack-resource-race");
    let runtime = temp.boot();
    let (handle, _) = runtime
        .register_host_work(host_work_request(
            "feature:cgroup:ack-race",
            20,
            Interruptibility::Immediate,
            false,
            true,
            &[HostWorkAction::Stop],
        ))
        .unwrap();
    let base = chrono::Utc::now().timestamp_millis();
    runtime
        .observe_resources(resources_with_app_cgroup(base, 2_000, 4_000, 10, 1, 0))
        .unwrap();
    let initial = runtime
        .observe_resources(resources_with_app_cgroup(base + 1, 4_100, 4_000, 10, 1, 0))
        .unwrap();

    let observation_entered = Arc::new(Barrier::new(2));
    let allow_observation_commit = Arc::new(Barrier::new(2));
    let entered = Arc::clone(&observation_entered);
    let release = Arc::clone(&allow_observation_commit);
    runtime.set_resource_observation_before_commit_hook(Some(Arc::new(move || {
        entered.wait();
        release.wait();
    })));
    let observing = runtime.clone();
    let edge_thread = std::thread::spawn(move || {
        observing
            .observe_resources(resources_with_app_cgroup(base + 2, 4_200, 4_000, 11, 1, 0))
            .unwrap()
    });
    observation_entered.wait();
    runtime
        .acknowledge_host_work_directive(
            &handle,
            &initial.host_work_directives[0].directive_id,
            HostWorkDirectiveAcknowledgement::Rejected,
            "rejected while ResourceObserved awaited append".to_string(),
        )
        .unwrap();
    allow_observation_commit.wait();
    let decision = edge_thread.join().unwrap();
    runtime.set_resource_observation_before_commit_hook(None);

    assert_eq!(runtime.snapshot().resources.pressure_epoch, 1);
    assert_eq!(decision.host_work_directives.len(), 1);
    assert_eq!(decision.host_work_directives[0].resource_pressure_epoch, 1);
    let claim = runtime
        .snapshot()
        .claims
        .get(decision.pressure_claim_id.as_deref().unwrap())
        .cloned()
        .unwrap();
    assert_eq!(claim.value["appCgroup"]["memoryEventDeltas"]["high"], 1);
}

#[test]
fn resource_commit_before_rejected_ack_preserves_durable_retry_credit() {
    let temp = TempRuntime::new("app-cgroup-resource-before-ack");
    let runtime = temp.boot();
    let (_handle, _) = runtime
        .register_host_work(host_work_request(
            "feature:cgroup:resource-before-ack",
            20,
            Interruptibility::Immediate,
            false,
            true,
            &[HostWorkAction::Stop],
        ))
        .unwrap();
    let base = chrono::Utc::now().timestamp_millis();
    runtime
        .observe_resources(resources_with_app_cgroup(base, 2_000, 4_000, 10, 1, 0))
        .unwrap();
    let initial = runtime
        .observe_resources(resources_with_app_cgroup(base + 1, 4_100, 4_000, 10, 1, 0))
        .unwrap();

    let committed_while_pending = runtime
        .observe_resources(resources_with_app_cgroup(base + 2, 4_200, 4_000, 11, 1, 0))
        .unwrap();
    assert!(committed_while_pending.host_work_directives.is_empty());
    let initial_directive_id = initial.host_work_directives[0].directive_id.clone();
    drop(runtime);

    let runtime = temp.boot();
    let (handle, _) = runtime
        .rebind_host_work(
            "feature:cgroup:resource-before-ack",
            HostWorkKind::ScheduledRun,
            1,
        )
        .unwrap();
    runtime
        .acknowledge_host_work_directive(
            &handle,
            &initial_directive_id,
            HostWorkDirectiveAcknowledgement::Rejected,
            "rejected after the cgroup edge was committed".to_string(),
        )
        .unwrap();
    let credited_retry = runtime
        .pending_host_work_directives(&handle)
        .unwrap()
        .into_iter()
        .find(|directive| directive.directive_id != initial_directive_id)
        .expect("fresh evidence committed while Pending must survive until Rejected ACK");
    assert_eq!(credited_retry.resource_pressure_epoch, 1);

    let no_third_identity = runtime
        .observe_resources(resources_with_app_cgroup(base + 3, 3_000, 4_000, 11, 1, 0))
        .unwrap();
    assert!(no_third_identity.host_work_directives.is_empty());
    assert_eq!(runtime.snapshot().host_work_directives.len(), 2);
    drop(runtime);
    assert_eq!(temp.boot().snapshot().host_work_directives.len(), 2);
}

#[test]
fn stale_or_nonadvancing_critical_samples_cannot_authorize_retry() {
    let temp = TempRuntime::new("resource-retry-freshness");
    let runtime = temp.boot();
    let (handle, _) = runtime
        .register_host_work(host_work_request(
            "feature:critical:retry-freshness",
            20,
            Interruptibility::Immediate,
            false,
            true,
            &[HostWorkAction::Stop],
        ))
        .unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let old_base = now - 30_000;
    let mut initial_observation = resources(96.0, 50.0);
    initial_observation.sampled_at_ms = old_base;
    let initial = runtime.observe_resources(initial_observation).unwrap();
    runtime
        .acknowledge_host_work_directive(
            &handle,
            &initial.host_work_directives[0].directive_id,
            HostWorkDirectiveAcknowledgement::Rejected,
            "stale episode initial stop rejected".to_string(),
        )
        .unwrap();

    let mut stale_advancing = resources(96.0, 50.0);
    stale_advancing.sampled_at_ms = old_base + 1;
    assert!(runtime
        .observe_resources(stale_advancing)
        .unwrap()
        .host_work_directives
        .is_empty());
    let mut duplicate = resources(96.0, 50.0);
    duplicate.sampled_at_ms = old_base + 1;
    assert!(runtime
        .observe_resources(duplicate)
        .unwrap()
        .host_work_directives
        .is_empty());

    let mut fresh = resources(96.0, 50.0);
    fresh.sampled_at_ms = now + 1;
    let retry = runtime.observe_resources(fresh).unwrap();
    assert_eq!(retry.host_work_directives.len(), 1);
    assert_eq!(retry.host_work_directives[0].resource_pressure_epoch, 1);
}

#[test]
fn out_of_order_cgroup_sample_cannot_rewind_counter_baseline_or_replay_delta() {
    let temp = TempRuntime::new("app-cgroup-baseline-monotonic");
    let runtime = temp.boot();
    let base = chrono::Utc::now().timestamp_millis();
    runtime
        .observe_resources(resources_with_app_cgroup(base, 2_000, 4_000, 10, 1, 0))
        .unwrap();
    runtime
        .observe_resources(resources_with_app_cgroup(base + 2, 2_100, 4_000, 11, 1, 0))
        .unwrap();
    let out_of_order = runtime
        .observe_resources(resources_with_app_cgroup(base + 1, 2_200, 4_000, 10, 1, 0))
        .unwrap();
    assert_eq!(out_of_order.pressure, ResourcePressure::Critical);
    assert_eq!(
        runtime
            .snapshot()
            .resources
            .last_app_cgroup_observation
            .as_ref()
            .unwrap()
            .observed_at_ms,
        base + 2
    );

    let relief = runtime
        .observe_resources(resources_with_app_cgroup(base + 3, 2_200, 4_000, 11, 1, 0))
        .unwrap();
    assert_eq!(relief.pressure, ResourcePressure::Normal);
}

#[test]
fn rejected_governor_action_gets_one_fresh_retry_then_waits_for_pressure_change() {
    let temp = TempRuntime::new("host-work-governor-retry-epoch");
    let runtime = temp.boot();
    let (handle, _) = runtime
        .register_host_work(host_work_request(
            "feature:critical:retry-epoch",
            10,
            Interruptibility::Immediate,
            false,
            true,
            &[HostWorkAction::Stop],
        ))
        .unwrap();
    let base = chrono::Utc::now().timestamp_millis();
    let mut first_observation = resources(96.0, 50.0);
    first_observation.sampled_at_ms = base;
    let first = runtime.observe_resources(first_observation).unwrap();
    let first_id = first.host_work_directives[0].directive_id.clone();
    runtime
        .acknowledge_host_work_directive(
            &handle,
            &first_id,
            HostWorkDirectiveAcknowledgement::Rejected,
            "fixed target refused stop".to_string(),
        )
        .unwrap();

    let mut retry_observation = resources(96.0, 50.0);
    retry_observation.sampled_at_ms = base + 1;
    let retry = runtime.observe_resources(retry_observation).unwrap();
    assert_eq!(retry.host_work_directives.len(), 1);
    assert_eq!(retry.host_work_directives[0].resource_pressure_epoch, 1);
    runtime
        .acknowledge_host_work_directive(
            &handle,
            &retry.host_work_directives[0].directive_id,
            HostWorkDirectiveAcknowledgement::Rejected,
            "fresh retry also rejected".to_string(),
        )
        .unwrap();
    let mut exhausted_observation = resources(96.0, 50.0);
    exhausted_observation.sampled_at_ms = base + 2;
    let exhausted = runtime.observe_resources(exhausted_observation).unwrap();
    assert!(exhausted.host_work_directives.is_empty());
    assert_eq!(runtime.snapshot().host_work_directives.len(), 2);

    let mut warm_observation = resources(81.0, 50.0);
    warm_observation.sampled_at_ms = base + 3;
    let warm = runtime.observe_resources(warm_observation).unwrap();
    assert_eq!(warm.pressure, ResourcePressure::Warm);
    let mut next_critical_observation = resources(96.0, 50.0);
    next_critical_observation.sampled_at_ms = base + 4;
    let next_epoch = runtime
        .observe_resources(next_critical_observation)
        .unwrap();
    assert_eq!(next_epoch.host_work_directives.len(), 1);
    assert_ne!(next_epoch.host_work_directives[0].directive_id, first_id);
    assert_eq!(
        next_epoch.host_work_directives[0].resource_pressure_epoch,
        3
    );
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
    drop(runtime);
    assert!(!std::fs::read_to_string(&temp.ledger)
        .unwrap()
        .contains("\"torn\""));
}

#[test]
fn complete_malformed_ledger_frame_fails_recovery_closed() {
    let temp = TempRuntime::new("malformed-complete-frame");
    drop(temp.boot());
    use std::io::Write as _;
    let mut ledger = std::fs::OpenOptions::new()
        .append(true)
        .open(&temp.ledger)
        .unwrap();
    writeln!(ledger, "{{\"corrupt\":true}}").unwrap();
    ledger.sync_data().unwrap();
    drop(ledger);
    assert!(PinvouOsRuntime::boot(temp.ledger.clone()).is_err());
}

#[test]
fn complete_json_without_commit_newline_is_not_replayed() {
    let temp = TempRuntime::new("unterminated-complete-memory-frame");
    let runtime = temp.boot();
    let evidence = user_evidence_event(&runtime, "user", "uncommitted", json!(true));
    runtime
        .remember_memory(RememberMemoryRequest {
            memory_id: "memory:uncommitted".to_string(),
            tier: MemoryTier::DurableFact,
            subject: "user".to_string(),
            predicate: "uncommitted".to_string(),
            value: json!(true),
            confidence: 1.0,
            source_actor_id: "actor:user".to_string(),
            evidence_event_ids: vec![evidence.event_id],
            observed_at_ms: evidence.occurred_at_ms,
            recorded_at_ms: evidence.occurred_at_ms,
            mission_id: None,
            run_id: None,
        })
        .unwrap();
    drop(runtime);

    let mut bytes = std::fs::read(&temp.ledger).unwrap();
    assert_eq!(bytes.pop(), Some(b'\n'));
    std::fs::write(&temp.ledger, bytes).unwrap();

    let rebooted = temp.boot();
    assert!(rebooted
        .project_organized_memory(OrganizedMemoryQuery {
            current_at_ms: chrono::Utc::now().timestamp_millis(),
            include_provisional: true,
            ..OrganizedMemoryQuery::default()
        })
        .unwrap()
        .items
        .is_empty());
    let events = raw_ledger_events(&temp);
    assert!(events
        .iter()
        .enumerate()
        .all(|(index, event)| event.sequence == index as u64 + 1));
}

#[test]
fn only_one_live_runtime_can_own_the_writer_lease() {
    let temp = TempRuntime::new("single-writer-lease");
    let runtime = temp.boot();
    assert!(PinvouOsRuntime::boot(temp.ledger.clone()).is_err());
    drop(runtime);
    assert!(PinvouOsRuntime::boot(temp.ledger.clone()).is_ok());
}

#[test]
fn duplicate_external_frame_is_detected_by_committed_ledger_length() {
    let temp = TempRuntime::new("external-duplicate-frame");
    let runtime = temp.boot();
    let duplicated = raw_ledger_events(&temp).last().unwrap().clone();
    append_test_envelope(&temp, &duplicated);

    assert!(runtime.list_events(None, 1_000).is_err());
    assert!(runtime
        .open_mission(OpenMissionRequest {
            objective: "must not hide an external duplicate".to_string(),
            priority: 50,
            deadline_at_ms: None,
        })
        .is_err());
    assert!(runtime.snapshot().missions.is_empty());
    drop(runtime);
    assert!(PinvouOsRuntime::boot(temp.ledger.clone()).is_err());
}

#[test]
fn resource_float_round_trip_does_not_isolate_the_live_writer() {
    let temp = TempRuntime::new("resource-float-ledger-head");
    let runtime = temp.boot();
    runtime
        .observe_resources(ResourceObservation {
            sampled_at_ms: chrono::Utc::now().timestamp_millis(),
            cpu_usage_pct: Some(0.3756574004507889),
            memory_used_pct: Some(26.030540826721566),
            gpu_usage_pct: None,
            temperature_c: Some(35.0),
            power_w: None,
            app_cgroup: None,
        })
        .unwrap();
    let before_reboot = runtime.snapshot().last_sequence;
    assert_eq!(
        runtime
            .list_events(None, 1_000)
            .unwrap()
            .last()
            .unwrap()
            .sequence,
        before_reboot
    );

    drop(runtime);
    let runtime = temp.boot();
    assert!(runtime.snapshot().last_sequence > before_reboot);

    let mission = runtime
        .open_mission(OpenMissionRequest {
            objective: "the writer remains healthy after float telemetry".to_string(),
            priority: 50,
            deadline_at_ms: None,
        })
        .unwrap();
    assert!(runtime
        .list_events(None, 1_000)
        .unwrap()
        .iter()
        .any(|event| event.run_id.as_deref() == Some(mission.run.run_id.as_str())));
}

#[test]
fn same_length_raw_frame_replacement_still_isolates_the_live_writer() {
    let temp = TempRuntime::new("same-length-frame-replacement");
    let runtime = temp.boot();
    runtime
        .observe_resources(ResourceObservation {
            sampled_at_ms: chrono::Utc::now().timestamp_millis(),
            cpu_usage_pct: Some(0.3756574004507889),
            memory_used_pct: Some(26.030540826721566),
            gpu_usage_pct: None,
            temperature_c: Some(35.0),
            power_w: None,
            app_cgroup: None,
        })
        .unwrap();

    let original = std::fs::read_to_string(&temp.ledger).unwrap();
    let replaced = original.replacen("26.030540826721566", "26.030540826721567", 1);
    assert_ne!(replaced, original);
    assert_eq!(replaced.len(), original.len());
    std::fs::write(&temp.ledger, replaced).unwrap();

    assert!(runtime.list_events(None, 1_000).is_err());
    assert!(runtime
        .open_mission(OpenMissionRequest {
            objective: "tampered bytes must not be accepted".to_string(),
            priority: 50,
            deadline_at_ms: None,
        })
        .is_err());
}

#[test]
fn legacy_schema_cannot_resume_after_the_current_writer_barrier() {
    let temp = TempRuntime::new("ledger-schema-downgrade");
    let runtime = temp.boot();
    let sequence = runtime.snapshot().last_sequence.saturating_add(1);
    drop(runtime);
    append_test_envelope(
        &temp,
        &EventEnvelope {
            schema_version: 2,
            sequence,
            event_id: format!("event-{sequence:016x}"),
            occurred_at_ms: chrono::Utc::now().timestamp_millis(),
            source_actor_id: KERNEL_ACTOR_ID.to_string(),
            mission_id: None,
            run_id: None,
            interaction_scope_id: None,
            interaction_run_id: None,
            causation_id: None,
            correlation_id: None,
            event: RuntimeEvent::RuntimeStarted { process_id: 42 },
        },
    );
    assert!(PinvouOsRuntime::boot(temp.ledger.clone()).is_err());
}

#[test]
fn interaction_run_has_one_terminal_outcome_and_does_not_persist_transcript_text() {
    let temp = TempRuntime::new("interaction-lifecycle");
    let runtime = temp.boot();
    let private_input = "这段原始输入不能复制进 Runtime 账本";
    let private_output = "这段完整回答也不能复制进 Runtime 账本";
    let interaction = runtime
        .open_interaction_run(OpenInteractionRunRequest {
            content: private_input.to_string(),
            modality: InteractionModality::Voice,
            parent_interaction_run_id: None,
            resume_interrupt_id: None,
        })
        .unwrap();
    runtime
        .start_interaction_run(&interaction.interaction_run_id)
        .unwrap();
    runtime
        .record_interaction_tool_started(&interaction.interaction_run_id, "tool-1", "search")
        .unwrap();
    runtime
        .record_interaction_tool_finished(&interaction.interaction_run_id, "tool-1", "search", true)
        .unwrap();
    runtime
        .record_interaction_assistant_message(&interaction.interaction_run_id, private_output)
        .unwrap();
    runtime
        .finish_interaction_run(
            &interaction.interaction_run_id,
            InteractionRunOutcome::Success,
        )
        .unwrap();

    let projected = runtime
        .snapshot()
        .interaction_runs
        .get(&interaction.interaction_run_id)
        .cloned()
        .unwrap();
    assert_eq!(projected.status, InteractionRunStatus::Completed);
    assert_eq!(projected.outcome, Some(InteractionRunOutcome::Success));
    assert_eq!(projected.input_digest.len(), 64);
    assert!(runtime
        .finish_interaction_run(
            &interaction.interaction_run_id,
            InteractionRunOutcome::Cancelled,
        )
        .is_err());
    let ledger = std::fs::read_to_string(&temp.ledger).unwrap();
    assert!(!ledger.contains(private_input));
    assert!(!ledger.contains(private_output));
}

#[test]
fn concurrent_interaction_terminals_and_resumes_commit_only_once() {
    let temp = TempRuntime::new("interaction-concurrency");
    let runtime = temp.boot();
    let interaction = runtime
        .open_interaction_run(OpenInteractionRunRequest {
            content: "并发终态".to_string(),
            modality: InteractionModality::Text,
            parent_interaction_run_id: None,
            resume_interrupt_id: None,
        })
        .unwrap();
    runtime
        .start_interaction_run(&interaction.interaction_run_id)
        .unwrap();

    let terminal_barrier = Arc::new(Barrier::new(3));
    let terminal_attempts = [
        InteractionRunOutcome::Success,
        InteractionRunOutcome::Cancelled,
    ]
    .into_iter()
    .map(|outcome| {
        let runtime = runtime.clone();
        let interaction_run_id = interaction.interaction_run_id.clone();
        let barrier = Arc::clone(&terminal_barrier);
        std::thread::spawn(move || {
            barrier.wait();
            runtime.finish_interaction_run(&interaction_run_id, outcome)
        })
    })
    .collect::<Vec<_>>();
    terminal_barrier.wait();
    let terminal_results = terminal_attempts
        .into_iter()
        .map(|attempt| attempt.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        terminal_results
            .iter()
            .filter(|result| result.is_ok())
            .count(),
        1
    );

    let parent = runtime
        .open_interaction_run(OpenInteractionRunRequest {
            content: "并发恢复".to_string(),
            modality: InteractionModality::Text,
            parent_interaction_run_id: None,
            resume_interrupt_id: None,
        })
        .unwrap();
    runtime
        .start_interaction_run(&parent.interaction_run_id)
        .unwrap();
    runtime
        .finish_interaction_run(
            &parent.interaction_run_id,
            InteractionRunOutcome::Interrupt {
                interrupts: vec![InteractionInterrupt {
                    interrupt_id: "interrupt-concurrent".to_string(),
                    reason: "user_input_required".to_string(),
                    question_count: 1,
                    created_at_ms: chrono::Utc::now().timestamp_millis(),
                }],
            },
        )
        .unwrap();

    let resume_barrier = Arc::new(Barrier::new(3));
    let resume_attempts = (0..2)
        .map(|attempt| {
            let runtime = runtime.clone();
            let parent_id = parent.interaction_run_id.clone();
            let barrier = Arc::clone(&resume_barrier);
            std::thread::spawn(move || {
                barrier.wait();
                runtime.open_interaction_run(OpenInteractionRunRequest {
                    content: format!("恢复 {attempt}"),
                    modality: InteractionModality::Text,
                    parent_interaction_run_id: Some(parent_id),
                    resume_interrupt_id: Some("interrupt-concurrent".to_string()),
                })
            })
        })
        .collect::<Vec<_>>();
    resume_barrier.wait();
    let resume_results = resume_attempts
        .into_iter()
        .map(|attempt| attempt.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        resume_results
            .iter()
            .filter(|result| result.is_ok())
            .count(),
        1
    );
    assert_eq!(
        runtime
            .snapshot()
            .interaction_runs
            .values()
            .filter(|run| {
                run.parent_interaction_run_id.as_deref() == Some(parent.interaction_run_id.as_str())
                    && run.resume_interrupt_id.as_deref() == Some("interrupt-concurrent")
            })
            .count(),
        1
    );

    let terminal_events = std::fs::read_to_string(&temp.ledger)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| {
            event.pointer("/event/kind").and_then(|kind| kind.as_str())
                == Some("interaction_run_finished")
        })
        .count();
    assert_eq!(
        terminal_events, 2,
        "one terminal for each of the two parent interactions"
    );
}

#[test]
fn interrupted_interaction_resumes_as_a_new_run_with_exact_binding() {
    let temp = TempRuntime::new("interaction-resume");
    let runtime = temp.boot();
    let parent = runtime
        .open_interaction_run(OpenInteractionRunRequest {
            content: "帮我预订".to_string(),
            modality: InteractionModality::Voice,
            parent_interaction_run_id: None,
            resume_interrupt_id: None,
        })
        .unwrap();
    runtime
        .start_interaction_run(&parent.interaction_run_id)
        .unwrap();
    let interrupt = InteractionInterrupt {
        interrupt_id: "interrupt-1".to_string(),
        reason: "user_input_required".to_string(),
        question_count: 1,
        created_at_ms: chrono::Utc::now().timestamp_millis(),
    };
    runtime
        .finish_interaction_run(
            &parent.interaction_run_id,
            InteractionRunOutcome::Interrupt {
                interrupts: vec![interrupt],
            },
        )
        .unwrap();

    let resumed = runtime
        .open_interaction_run(OpenInteractionRunRequest {
            content: "明天晚上七点".to_string(),
            modality: InteractionModality::Voice,
            parent_interaction_run_id: Some(parent.interaction_run_id.clone()),
            resume_interrupt_id: Some("interrupt-1".to_string()),
        })
        .unwrap();
    assert_ne!(resumed.interaction_run_id, parent.interaction_run_id);
    assert_eq!(
        resumed.parent_interaction_run_id.as_deref(),
        Some(parent.interaction_run_id.as_str())
    );
    assert_eq!(resumed.resume_interrupt_id.as_deref(), Some("interrupt-1"));
    assert!(runtime
        .open_interaction_run(OpenInteractionRunRequest {
            content: "重复恢复".to_string(),
            modality: InteractionModality::Voice,
            parent_interaction_run_id: resumed.parent_interaction_run_id.clone(),
            resume_interrupt_id: resumed.resume_interrupt_id.clone(),
        })
        .is_err());
    assert!(runtime
        .open_interaction_run(OpenInteractionRunRequest {
            content: "伪造恢复".to_string(),
            modality: InteractionModality::Voice,
            parent_interaction_run_id: Some(parent.interaction_run_id),
            resume_interrupt_id: Some("interrupt-other".to_string()),
        })
        .is_err());
}

#[test]
fn reboot_closes_an_unfinished_interaction_instead_of_leaving_it_running() {
    let temp = TempRuntime::new("interaction-reboot");
    let runtime = temp.boot();
    let interaction = runtime
        .open_interaction_run(OpenInteractionRunRequest {
            content: "进程中途退出".to_string(),
            modality: InteractionModality::Voice,
            parent_interaction_run_id: None,
            resume_interrupt_id: None,
        })
        .unwrap();
    runtime
        .start_interaction_run(&interaction.interaction_run_id)
        .unwrap();
    drop(runtime);

    let recovered = temp.boot();
    let projected = recovered
        .snapshot()
        .interaction_runs
        .get(&interaction.interaction_run_id)
        .cloned()
        .unwrap();
    assert_eq!(projected.status, InteractionRunStatus::Failed);
    assert_eq!(
        projected.outcome,
        Some(InteractionRunOutcome::Error {
            error_code: "runtime_restarted".to_string(),
        })
    );
}

#[test]
fn memory_agent_persists_in_the_unified_ledger_across_reboot() {
    let temp = TempRuntime::new("memory-replay");
    let runtime = temp.boot();
    let evidence_event = user_evidence_event(&runtime, "user", "preferred_name", json!("白浪"));
    let evidence_event_id = evidence_event.event_id.clone();
    let receipt = runtime
        .remember_memory(RememberMemoryRequest {
            memory_id: "memory:user-name".to_string(),
            tier: MemoryTier::DurableFact,
            subject: "user".to_string(),
            predicate: "preferred_name".to_string(),
            value: json!("白浪"),
            confidence: 1.0,
            source_actor_id: "actor:user".to_string(),
            evidence_event_ids: vec![evidence_event_id],
            observed_at_ms: chrono::Utc::now().timestamp_millis(),
            recorded_at_ms: chrono::Utc::now().timestamp_millis(),
            mission_id: None,
            run_id: None,
        })
        .unwrap();
    assert_eq!(receipt.revision, 1);
    drop(runtime);

    let rebooted = temp.boot();
    let context = rebooted
        .compile_memory_context(CompileMemoryContextRequest::default())
        .unwrap();
    assert_eq!(context.revision, 1);
    assert_eq!(context.items.len(), 1);
    assert_eq!(context.items[0].value, json!("白浪"));
    let raw_ledger = std::fs::read_to_string(&temp.ledger).unwrap();
    assert!(raw_ledger.contains("organized_memory_decision_recorded"));
    assert!(!raw_ledger.contains("memory_projection_updated"));
    assert!(!rebooted
        .list_events(None, 1_000)
        .unwrap()
        .iter()
        .any(|event| matches!(
            event.event,
            RuntimeEvent::MemoryProjectionUpdated { .. }
                | RuntimeEvent::OrganizedMemoryDecisionRecorded { .. }
                | RuntimeEvent::OrganizedMemoryCheckpointRecorded { .. }
        )));
}

#[test]
fn memory_agent_rejects_untraceable_evidence() {
    let temp = TempRuntime::new("memory-evidence");
    let runtime = temp.boot();
    let result = runtime.remember_memory(RememberMemoryRequest {
        memory_id: "memory:untraceable".to_string(),
        tier: MemoryTier::Working,
        subject: "mission".to_string(),
        predicate: "state".to_string(),
        value: json!("running"),
        confidence: 0.8,
        source_actor_id: "agent:front".to_string(),
        evidence_event_ids: vec!["event:does-not-exist".to_string()],
        observed_at_ms: chrono::Utc::now().timestamp_millis(),
        recorded_at_ms: chrono::Utc::now().timestamp_millis(),
        mission_id: None,
        run_id: None,
    });
    assert!(result.is_err());
}

#[test]
fn memory_runtime_requires_exact_claim_binding_and_keeps_decisions_off_the_renderer_stream() {
    let temp = TempRuntime::new("memory-attestation");
    let runtime = temp.boot();
    let evidence = runtime
        .record_inference_completion(InferenceCompletionObservation {
            completed_at_ms: chrono::Utc::now().timestamp_millis(),
            model: "test-model".to_string(),
            latency_ms: 10,
        })
        .unwrap();
    assert!(runtime
        .organize_memory(proposed_fact(
            "unbound-inference",
            &evidence,
            json!("must be rejected"),
        ))
        .is_err());
    let evidence = user_evidence_event(
        &runtime,
        "user",
        "forged-authority",
        json!("bound user statement"),
    );
    let receipt = runtime
        .organize_memory(proposed_fact(
            "forged-authority",
            &evidence,
            json!("bound user statement"),
        ))
        .unwrap();
    assert_eq!(receipt.revision, 1);

    let hidden_by_default = runtime
        .project_organized_memory(OrganizedMemoryQuery {
            current_at_ms: chrono::Utc::now().timestamp_millis(),
            ..OrganizedMemoryQuery::default()
        })
        .unwrap();
    assert_eq!(hidden_by_default.items.len(), 1);
    assert_eq!(
        hidden_by_default.items[0].status,
        OrganizedMemoryStatus::Confirmed
    );
    assert_eq!(
        hidden_by_default.items[0].applicability.space_id,
        "personal"
    );

    let decision_event = raw_ledger_events(&temp)
        .into_iter()
        .find(|event| {
            matches!(
                event.event,
                RuntimeEvent::OrganizedMemoryDecisionRecorded { .. }
            )
        })
        .unwrap();
    assert_eq!(decision_event.source_actor_id, MEMORY_AGENT_ID);
    assert_eq!(
        decision_event.causation_id.as_deref(),
        Some(evidence.event_id.as_str())
    );
    assert!(!runtime
        .list_events(None, 1_000)
        .unwrap()
        .iter()
        .any(|event| event.event_id == decision_event.event_id));
}

#[test]
fn retracted_claim_cannot_be_used_by_a_later_memory_decision() {
    let temp = TempRuntime::new("memory-stale-claim");
    let runtime = temp.boot();
    let assertion = user_evidence_event(&runtime, "user", "timezone", json!("Asia/Shanghai"));
    runtime
        .retract_test_user_claim(&asserted_claim_id(&assertion))
        .unwrap();

    assert!(runtime
        .remember_memory(RememberMemoryRequest {
            memory_id: "memory:stale-claim".to_string(),
            tier: MemoryTier::DurableFact,
            subject: "user".to_string(),
            predicate: "timezone".to_string(),
            value: json!("Asia/Shanghai"),
            confidence: 1.0,
            source_actor_id: "actor:user".to_string(),
            evidence_event_ids: vec![assertion.event_id.clone()],
            observed_at_ms: 0,
            recorded_at_ms: 0,
            mission_id: None,
            run_id: None,
        })
        .is_err());
    assert!(!raw_ledger_events(&temp).iter().any(|event| matches!(
        event.event,
        RuntimeEvent::OrganizedMemoryDecisionRecorded { .. }
    )));
}

#[test]
fn exact_claim_retraction_uses_runtime_time_and_tombstones_its_memory() {
    let temp = TempRuntime::new("memory-exact-retraction");
    let runtime = temp.boot();
    let assertion = user_evidence_event(&runtime, "user", "locale", json!("zh-CN"));
    runtime
        .remember_memory(RememberMemoryRequest {
            memory_id: "memory:locale".to_string(),
            tier: MemoryTier::DurableFact,
            subject: "user".to_string(),
            predicate: "locale".to_string(),
            value: json!("zh-CN"),
            confidence: 1.0,
            source_actor_id: "actor:user".to_string(),
            evidence_event_ids: vec![assertion.event_id.clone()],
            observed_at_ms: 0,
            recorded_at_ms: 0,
            mission_id: None,
            run_id: None,
        })
        .unwrap();
    let retraction = runtime
        .retract_test_user_claim(&asserted_claim_id(&assertion))
        .unwrap();
    runtime
        .retract_memory(RetractMemoryRequest {
            memory_id: "memory:locale".to_string(),
            reason: "user corrected the claim".to_string(),
            source_actor_id: "forged:caller".to_string(),
            evidence_event_ids: vec![retraction.event_id],
            // Runtime must ignore this stale caller-provided timestamp.
            retracted_at_ms: 0,
        })
        .unwrap();
    assert!(runtime
        .project_organized_memory(OrganizedMemoryQuery {
            current_at_ms: chrono::Utc::now().timestamp_millis(),
            include_provisional: true,
            ..OrganizedMemoryQuery::default()
        })
        .unwrap()
        .items
        .is_empty());
}

#[test]
fn retracting_one_of_two_supporting_claims_does_not_delete_the_merged_memory() {
    let temp = TempRuntime::new("memory-multi-support-retraction");
    let runtime = temp.boot();
    let first = user_evidence_event(&runtime, "user", "editor", json!("Obsidian"));
    let second = user_evidence_event(&runtime, "user", "editor", json!("Obsidian"));
    for (memory_id, assertion) in [
        ("memory:editor:first", &first),
        ("memory:editor:second", &second),
    ] {
        runtime
            .remember_memory(RememberMemoryRequest {
                memory_id: memory_id.to_string(),
                tier: MemoryTier::DurableFact,
                subject: "user".to_string(),
                predicate: "editor".to_string(),
                value: json!("Obsidian"),
                confidence: 1.0,
                source_actor_id: "actor:user".to_string(),
                evidence_event_ids: vec![assertion.event_id.clone()],
                observed_at_ms: 0,
                recorded_at_ms: 0,
                mission_id: None,
                run_id: None,
            })
            .unwrap();
    }
    let retraction = runtime
        .retract_test_user_claim(&asserted_claim_id(&first))
        .unwrap();
    assert!(runtime
        .retract_memory(RetractMemoryRequest {
            memory_id: "memory:editor:first".to_string(),
            reason: "only the first assertion was withdrawn".to_string(),
            source_actor_id: "actor:user".to_string(),
            evidence_event_ids: vec![retraction.event_id],
            retracted_at_ms: 0,
        })
        .is_err());
    assert_eq!(
        runtime
            .project_organized_memory(OrganizedMemoryQuery {
                current_at_ms: chrono::Utc::now().timestamp_millis(),
                ..OrganizedMemoryQuery::default()
            })
            .unwrap()
            .items
            .len(),
        1
    );
}

#[test]
fn memory_idempotent_retry_emits_no_second_runtime_event() {
    let temp = TempRuntime::new("memory-idempotent");
    let runtime = temp.boot();
    let evidence = user_evidence_event(&runtime, "user.preference", "editor", json!("Obsidian"));
    let request = RememberMemoryRequest {
        memory_id: "memory:preferred-editor".to_string(),
        tier: MemoryTier::DurableFact,
        subject: "user.preference".to_string(),
        predicate: "editor".to_string(),
        value: json!("Obsidian"),
        confidence: 1.0,
        source_actor_id: "forged:caller".to_string(),
        evidence_event_ids: vec![evidence.event_id],
        observed_at_ms: evidence.occurred_at_ms,
        recorded_at_ms: evidence.occurred_at_ms,
        mission_id: None,
        run_id: None,
    };
    let first = runtime.remember_memory(request.clone()).unwrap();
    let after_first = runtime.snapshot().last_sequence;
    let second = runtime.remember_memory(request).unwrap();
    assert_eq!(first.revision, second.revision);
    assert_eq!(runtime.snapshot().last_sequence, after_first);
    assert_eq!(
        raw_ledger_events(&temp)
            .iter()
            .filter(|event| matches!(
                event.event,
                RuntimeEvent::OrganizedMemoryDecisionRecorded { .. }
            ))
            .count(),
        1
    );
}

#[test]
fn legacy_projection_migrates_once_as_provisional_then_replays_checkpoint_tail() {
    let temp = TempRuntime::new("memory-legacy-migration");
    let mut legacy = MemoryAgent::new();
    legacy
        .remember(RememberMemoryRequest {
            memory_id: "legacy:durable".to_string(),
            tier: MemoryTier::DurableFact,
            subject: "user".to_string(),
            predicate: "old_preference".to_string(),
            value: json!("legacy value"),
            confidence: 1.0,
            source_actor_id: "actor:user".to_string(),
            evidence_event_ids: vec!["event:unattested-old-source".to_string()],
            observed_at_ms: 10,
            recorded_at_ms: 10,
            mission_id: None,
            run_id: None,
        })
        .unwrap();
    legacy
        .remember(RememberMemoryRequest {
            memory_id: "legacy:working".to_string(),
            tier: MemoryTier::Working,
            subject: "mission".to_string(),
            predicate: "next_step".to_string(),
            value: json!("do not migrate"),
            confidence: 1.0,
            source_actor_id: "agent:orchestrator".to_string(),
            evidence_event_ids: vec!["event:old-task".to_string()],
            observed_at_ms: 10,
            recorded_at_ms: 10,
            mission_id: Some("mission:old".to_string()),
            run_id: Some("run:old".to_string()),
        })
        .unwrap();
    std::fs::write(
        &temp.ledger,
        format!(
            "{}\n",
            serde_json::to_string(&EventEnvelope {
                schema_version: 2,
                sequence: 1,
                event_id: "event-legacy-projection".to_string(),
                occurred_at_ms: 20,
                source_actor_id: MEMORY_AGENT_ID.to_string(),
                mission_id: None,
                run_id: None,
                interaction_scope_id: None,
                interaction_run_id: None,
                causation_id: None,
                correlation_id: Some("legacy-memory-projection".to_string()),
                event: RuntimeEvent::MemoryProjectionUpdated {
                    revision: legacy.projection().revision,
                    operation: "remember".to_string(),
                    memory_id: "legacy:working".to_string(),
                    projection: serde_json::to_value(legacy.projection()).unwrap(),
                },
            })
            .unwrap()
        ),
    )
    .unwrap();

    let runtime = temp.boot();
    let migrated = runtime
        .project_organized_memory(OrganizedMemoryQuery {
            current_at_ms: 30,
            include_provisional: true,
            ..OrganizedMemoryQuery::default()
        })
        .unwrap();
    assert_eq!(migrated.items.len(), 1);
    assert_eq!(migrated.items[0].status, OrganizedMemoryStatus::Provisional);
    let evidence = user_evidence_event(&runtime, "user", "new_preference", json!("trusted value"));
    runtime
        .remember_memory(RememberMemoryRequest {
            memory_id: "new:trusted".to_string(),
            tier: MemoryTier::DurableFact,
            subject: "user".to_string(),
            predicate: "new_preference".to_string(),
            value: json!("trusted value"),
            confidence: 1.0,
            source_actor_id: "actor:user".to_string(),
            evidence_event_ids: vec![evidence.event_id],
            observed_at_ms: evidence.occurred_at_ms,
            recorded_at_ms: evidence.occurred_at_ms,
            mission_id: None,
            run_id: None,
        })
        .unwrap();
    drop(runtime);

    let rebooted = temp.boot();
    assert_eq!(
        rebooted
            .project_organized_memory(OrganizedMemoryQuery {
                current_at_ms: chrono::Utc::now().timestamp_millis(),
                include_provisional: true,
                ..OrganizedMemoryQuery::default()
            })
            .unwrap()
            .items
            .len(),
        2
    );
    let checkpoints = raw_ledger_events(&temp)
        .into_iter()
        .filter_map(|event| match event.event {
            RuntimeEvent::OrganizedMemoryCheckpointRecorded {
                legacy_source_event_id,
                legacy_migration,
                ..
            } => Some((legacy_source_event_id, legacy_migration)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].0.as_deref(), Some("event-legacy-projection"));
    let report = checkpoints[0].1.as_ref().unwrap();
    assert_eq!(report.imported_durable_records, 1);
    assert_eq!(report.skipped_working_records, 1);
}

#[test]
fn memory_append_failure_isolates_tentative_state_until_reboot() {
    let temp = TempRuntime::new("memory-append-failure");
    let runtime = temp.boot();
    let evidence = user_evidence_event(&runtime, "user", "durability_test", json!(true));
    let request = RememberMemoryRequest {
        memory_id: "memory:must-be-durable".to_string(),
        tier: MemoryTier::DurableFact,
        subject: "user".to_string(),
        predicate: "durability_test".to_string(),
        value: json!(true),
        confidence: 1.0,
        source_actor_id: "actor:user".to_string(),
        evidence_event_ids: vec![evidence.event_id],
        observed_at_ms: evidence.occurred_at_ms,
        recorded_at_ms: evidence.occurred_at_ms,
        mission_id: None,
        run_id: None,
    };
    let backup = temp.root.join("events.backup.jsonl");
    std::fs::rename(&temp.ledger, &backup).unwrap();
    std::fs::create_dir(&temp.ledger).unwrap();
    assert!(runtime.remember_memory(request).is_err());
    assert!(runtime
        .compile_memory_context(CompileMemoryContextRequest::default())
        .is_err());
    assert_eq!(
        runtime
            .explain_capability(MEMORY_CONTEXT_CAPABILITY_ID)
            .state,
        CapabilityAvailabilityState::TemporarilyUnavailable
    );
    std::fs::remove_dir(&temp.ledger).unwrap();
    std::fs::rename(&backup, &temp.ledger).unwrap();
    drop(runtime);

    let rebooted = temp.boot();
    assert!(rebooted
        .project_organized_memory(OrganizedMemoryQuery {
            current_at_ms: chrono::Utc::now().timestamp_millis(),
            include_provisional: true,
            ..OrganizedMemoryQuery::default()
        })
        .unwrap()
        .items
        .is_empty());
}

#[test]
fn missing_memory_decision_in_the_middle_fails_recovery_closed() {
    let temp = TempRuntime::new("memory-missing-decision");
    let runtime = temp.boot();
    for index in 0..2 {
        let predicate = format!("sequence_{index}");
        let evidence = user_evidence_event(&runtime, "user", &predicate, json!(index));
        runtime
            .remember_memory(RememberMemoryRequest {
                memory_id: format!("memory:sequence-{index}"),
                tier: MemoryTier::DurableFact,
                subject: "user".to_string(),
                predicate,
                value: json!(index),
                confidence: 1.0,
                source_actor_id: "actor:user".to_string(),
                evidence_event_ids: vec![evidence.event_id.clone()],
                observed_at_ms: evidence.occurred_at_ms,
                recorded_at_ms: evidence.occurred_at_ms,
                mission_id: None,
                run_id: None,
            })
            .unwrap();
    }
    drop(runtime);

    let mut removed = false;
    let retained = std::fs::read_to_string(&temp.ledger)
        .unwrap()
        .lines()
        .filter(|line| {
            let is_decision = serde_json::from_str::<EventEnvelope>(line).is_ok_and(|event| {
                matches!(
                    event.event,
                    RuntimeEvent::OrganizedMemoryDecisionRecorded { .. }
                )
            });
            if !removed && is_decision {
                removed = true;
                false
            } else {
                true
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&temp.ledger, format!("{retained}\n")).unwrap();
    assert!(PinvouOsRuntime::boot(temp.ledger.clone()).is_err());
}

#[test]
fn legacy_snapshot_after_new_decision_is_rejected_as_split_brain() {
    let temp = TempRuntime::new("memory-split-brain");
    let runtime = temp.boot();
    let evidence = user_evidence_event(&runtime, "user", "new_stream", json!(true));
    runtime
        .remember_memory(RememberMemoryRequest {
            memory_id: "memory:new-stream".to_string(),
            tier: MemoryTier::DurableFact,
            subject: "user".to_string(),
            predicate: "new_stream".to_string(),
            value: json!(true),
            confidence: 1.0,
            source_actor_id: "actor:user".to_string(),
            evidence_event_ids: vec![evidence.event_id],
            observed_at_ms: evidence.occurred_at_ms,
            recorded_at_ms: evidence.occurred_at_ms,
            mission_id: None,
            run_id: None,
        })
        .unwrap();
    let sequence = runtime.snapshot().last_sequence.saturating_add(1);
    drop(runtime);
    append_test_envelope(
        &temp,
        &EventEnvelope {
            schema_version: SCHEMA_VERSION,
            sequence,
            event_id: format!("event-{sequence:016x}"),
            occurred_at_ms: chrono::Utc::now().timestamp_millis(),
            source_actor_id: MEMORY_AGENT_ID.to_string(),
            mission_id: None,
            run_id: None,
            interaction_scope_id: None,
            interaction_run_id: None,
            causation_id: None,
            correlation_id: Some("old-binary-write".to_string()),
            event: RuntimeEvent::MemoryProjectionUpdated {
                revision: 0,
                operation: "remember".to_string(),
                memory_id: "legacy:fork".to_string(),
                projection: serde_json::to_value(MemoryProjectionState::default()).unwrap(),
            },
        },
    );
    assert!(PinvouOsRuntime::boot(temp.ledger.clone()).is_err());
}

#[test]
fn concurrent_memory_writes_have_one_continuous_decision_head() {
    let temp = TempRuntime::new("memory-concurrent-writes");
    let runtime = temp.boot();
    let evidence = (0..16)
        .map(|index| {
            user_evidence_event(
                &runtime,
                "user",
                &format!("concurrent_{index}"),
                json!(index),
            )
        })
        .collect::<Vec<_>>();
    let handles = evidence
        .into_iter()
        .enumerate()
        .map(|(index, evidence)| {
            let runtime = runtime.clone();
            std::thread::spawn(move || {
                runtime.remember_memory(RememberMemoryRequest {
                    memory_id: format!("memory:concurrent-{index}"),
                    tier: MemoryTier::DurableFact,
                    subject: "user".to_string(),
                    predicate: format!("concurrent_{index}"),
                    value: json!(index),
                    confidence: 1.0,
                    source_actor_id: "actor:user".to_string(),
                    evidence_event_ids: vec![evidence.event_id],
                    observed_at_ms: evidence.occurred_at_ms,
                    recorded_at_ms: evidence.occurred_at_ms,
                    mission_id: None,
                    run_id: None,
                })
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().unwrap().unwrap();
    }
    let decision_sequences = raw_ledger_events(&temp)
        .into_iter()
        .filter_map(|event| match event.event {
            RuntimeEvent::OrganizedMemoryDecisionRecorded { decision } => Some(decision.sequence),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(decision_sequences, (1..=16).collect::<Vec<_>>());
    drop(runtime);

    let rebooted = temp.boot();
    assert_eq!(
        rebooted
            .project_organized_memory(OrganizedMemoryQuery {
                current_at_ms: chrono::Utc::now().timestamp_millis(),
                max_items: 32,
                ..OrganizedMemoryQuery::default()
            })
            .unwrap()
            .items
            .len(),
        16
    );
}
