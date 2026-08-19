use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};

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
fn registered_control_adapter_applies_governor_directive_immediately() {
    let temp = TempRuntime::new("control-adapter");
    let runtime = temp.boot();
    let agent = mission_agent(
        &runtime,
        "background.index",
        20,
        Interruptibility::Immediate,
    );
    let actions = Arc::new(Mutex::new(Vec::new()));
    let captured = actions.clone();
    runtime
        .register_agent_control_adapter(
            &agent.agent_id,
            Arc::new(move |directive| {
                captured.lock().unwrap().push(directive.action);
                Ok("run handle reached checkpoint".to_string())
            }),
        )
        .unwrap();

    let decision = runtime.observe_resources(resources(90.0, 60.0)).unwrap();
    assert_eq!(decision.directives[0].status, DirectiveStatus::Applied);
    assert_eq!(*actions.lock().unwrap(), vec![DirectiveAction::Pause]);
    assert_eq!(
        runtime.snapshot().agents[&agent.agent_id].observed_state,
        AgentState::Paused
    );
}

#[test]
fn concurrent_compensation_and_fresh_dispatch_call_adapter_once() {
    let temp = TempRuntime::new("control-adapter-concurrent-dispatch");
    let runtime = temp.boot();
    let agent = mission_agent(
        &runtime,
        "background.concurrent",
        20,
        Interruptibility::Immediate,
    );

    // Freeze the issuing path after DirectiveIssued is durable but before its fresh dispatch.
    // Registration can now find the same Pending directive through its compensation path.
    let directive_issued = Arc::new(Barrier::new(2));
    let release_fresh_dispatch = Arc::new(Barrier::new(2));
    let sink_issued = directive_issued.clone();
    let sink_release = release_fresh_dispatch.clone();
    runtime.set_event_sink(move |envelope| {
        if matches!(envelope.event, RuntimeEvent::DirectiveIssued { .. }) {
            sink_issued.wait();
            sink_release.wait();
        }
    });

    let observing_runtime = runtime.clone();
    let observing =
        std::thread::spawn(move || observing_runtime.observe_resources(resources(90.0, 60.0)));
    directive_issued.wait();

    // Keep the compensation invocation in flight. A broken implementation calls the same
    // Adapter again from the newly-issued path; the second call intentionally does not block,
    // so this interleaving is deterministic rather than scheduler-probabilistic.
    let adapter_calls = Arc::new(AtomicUsize::new(0));
    let delivered_ids = Arc::new(Mutex::new(Vec::new()));
    let first_adapter_entered = Arc::new(Barrier::new(2));
    let release_first_adapter = Arc::new(Barrier::new(2));
    let registering_runtime = runtime.clone();
    let registering_agent_id = agent.agent_id.clone();
    let captured_calls = adapter_calls.clone();
    let captured_ids = delivered_ids.clone();
    let adapter_entered = first_adapter_entered.clone();
    let adapter_release = release_first_adapter.clone();
    let registering = std::thread::spawn(move || {
        registering_runtime.register_agent_control_adapter(
            &registering_agent_id,
            Arc::new(move |directive| {
                let call_index = captured_calls.fetch_add(1, Ordering::SeqCst);
                captured_ids
                    .lock()
                    .unwrap()
                    .push(directive.directive_id.clone());
                if call_index == 0 {
                    adapter_entered.wait();
                    adapter_release.wait();
                }
                Ok(format!("applied {}", directive.directive_id))
            }),
        )
    });
    first_adapter_entered.wait();

    release_fresh_dispatch.wait();
    let decision = observing.join().unwrap().unwrap();
    let calls_while_first_was_in_flight = adapter_calls.load(Ordering::SeqCst);

    release_first_adapter.wait();
    let reconciled = registering.join().unwrap().unwrap();
    let directive_id = decision.directives[0].directive_id.clone();

    assert_eq!(reconciled, 1);
    assert_eq!(calls_while_first_was_in_flight, 1);
    assert_eq!(adapter_calls.load(Ordering::SeqCst), 1);
    assert_eq!(*delivered_ids.lock().unwrap(), vec![directive_id.clone()]);
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
fn control_adapter_registration_reconciles_pending_directive_after_reboot() {
    let temp = TempRuntime::new("control-adapter-reboot");
    let (agent_id, directive_id) = {
        let runtime = temp.boot();
        let agent = mission_agent(
            &runtime,
            "background.index",
            20,
            Interruptibility::Immediate,
        );
        let decision = runtime.observe_resources(resources(90.0, 60.0)).unwrap();
        (agent.agent_id, decision.directives[0].directive_id.clone())
    };

    let rebooted = temp.boot();
    let reconciled = rebooted
        .register_agent_control_adapter(
            &agent_id,
            Arc::new(|_| Ok("recovered run handle paused".to_string())),
        )
        .unwrap();
    assert_eq!(reconciled, 1);
    let snapshot = rebooted.snapshot();
    assert_eq!(
        snapshot.directives[&directive_id].status,
        DirectiveStatus::Applied
    );
    assert_eq!(
        snapshot.agents[&agent_id].observed_state,
        AgentState::Paused
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
