use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::super::domain::{
    MemoryApplicability, MemoryCandidateIntent, MemoryEvidence, MemoryEvidenceOrigin,
    MemoryEvidencePolarity, OrganizedMemoryKind, OrganizedMemoryStatus,
};
use super::*;

fn applicability(valid_from_ms: i64, valid_until_ms: Option<i64>) -> MemoryApplicability {
    MemoryApplicability {
        space_id: "personal".to_string(),
        environment: BTreeMap::new(),
        valid_from_ms,
        valid_until_ms,
    }
}

fn evidence(event_id: &str, observed_at_ms: i64) -> MemoryEvidence {
    MemoryEvidence {
        event_id: event_id.to_string(),
        source_actor_id: "actor:user".to_string(),
        origin: MemoryEvidenceOrigin::UserExplicit,
        polarity: MemoryEvidencePolarity::Supports,
        observed_at_ms,
        recorded_at_ms: observed_at_ms + 1,
        reliability: 0.95,
        mission_id: Some(format!("mission:{event_id}")),
        run_id: Some(format!("run:{event_id}")),
    }
}

fn candidate(
    candidate_id: &str,
    predicate: &str,
    value: Value,
    observed_at_ms: i64,
) -> MemoryCandidate {
    MemoryCandidate {
        candidate_id: candidate_id.to_string(),
        kind: OrganizedMemoryKind::Preference,
        subject: "user".to_string(),
        predicate: predicate.to_string(),
        value,
        applicability: applicability(observed_at_ms, None),
        importance: 0.8,
        confidence: 0.9,
        intent: MemoryCandidateIntent::Assert,
        target_memory_id: None,
        evidence: vec![evidence(&format!("event:{candidate_id}"), observed_at_ms)],
    }
}

fn memory_id(candidate_id: &str) -> String {
    format!("memory:organized:{candidate_id}")
}

fn take_decision<T>(outcome: OrganizedMemoryDecisionOutcome<T>) -> OrganizedMemoryDecisionBatch {
    outcome.decision.expect("write must emit a decision")
}

fn conflict_resolution_request() -> ResolveMemoryDisputeRequest {
    ResolveMemoryDisputeRequest {
        operation_id: "resolution:response-style".to_string(),
        winner_memory_id: memory_id("style-b"),
        losing_memory_ids: vec![memory_id("style-a")],
        reason: "the user explicitly clarified the current preference".to_string(),
        resolved_at_ms: 31,
        evidence: vec![evidence("event:style-resolution", 30)],
    }
}

#[test]
fn decisions_are_fine_grained_and_replay_without_rerunning_organizer_policy() {
    let mut engine = OrganizedMemoryDecisionEngine::new();
    let mut decisions = Vec::new();

    decisions.push(take_decision(
        engine
            .organize(candidate("language-a", "language", json!("Chinese"), 10))
            .unwrap(),
    ));
    decisions.push(take_decision(
        engine
            .organize(candidate("language-b", "language", json!("Chinese"), 20))
            .unwrap(),
    ));
    decisions.push(take_decision(engine.maintain(25).unwrap()));

    assert_eq!(decisions[0].delta.record_upserts.len(), 1);
    assert_eq!(decisions[1].delta.record_upserts.len(), 1);
    assert_eq!(decisions[2].delta.record_upserts.len(), 0);
    assert_eq!(decisions[0].delta.processed_candidate_upserts.len(), 1);
    assert_eq!(decisions[2].delta.last_maintenance_at_ms, Some(25));

    let encoded = serde_json::to_value(&decisions[0]).unwrap();
    assert!(encoded.get("state").is_none());
    assert!(encoded.get("records").is_none());
    assert!(encoded["delta"].get("recordUpserts").is_some());
    let text = serde_json::to_string(&decisions[0]).unwrap();
    assert!(!text.to_ascii_lowercase().contains("session"));

    let replayed = OrganizedMemoryDecisionEngine::replay(decisions).unwrap();
    assert_eq!(
        replayed.organizer().export_state(),
        engine.organizer().export_state()
    );
    assert_eq!(replayed.last_sequence(), engine.last_sequence());
    assert_eq!(replayed.last_decision_hash(), engine.last_decision_hash());
}

#[test]
fn absorbed_duplicate_updates_only_idempotency_sidecar() {
    let mut engine = OrganizedMemoryDecisionEngine::new();
    let first_candidate = candidate("source-a", "language", json!("Chinese"), 10);
    let first = take_decision(engine.organize(first_candidate.clone()).unwrap());
    let mut duplicate_evidence = candidate("source-b", "language", json!("Chinese"), 20);
    duplicate_evidence.evidence = first_candidate.evidence;
    let second = take_decision(engine.organize(duplicate_evidence).unwrap());

    assert!(second.delta.record_upserts.is_empty());
    assert_eq!(second.delta.processed_candidate_upserts.len(), 1);
    let OrganizedMemoryDecisionOperation::CandidateOrganized { receipt } = &second.operation else {
        panic!("expected candidate decision");
    };
    assert_eq!(receipt.action, MemoryOrganizationAction::IgnoredDuplicate);
    assert_eq!(receipt.affected_memory_ids, vec![memory_id("source-a")]);

    let replayed = OrganizedMemoryDecisionEngine::replay(vec![first, second]).unwrap();
    assert_eq!(
        replayed.organizer().export_state(),
        engine.organizer().export_state()
    );
}

#[test]
fn exact_command_replays_do_not_emit_or_advance_decisions() {
    let mut candidate_engine = OrganizedMemoryDecisionEngine::new();
    let candidate_request = candidate("candidate-once", "language", json!("Chinese"), 10);
    assert!(candidate_engine
        .organize(candidate_request.clone())
        .unwrap()
        .decision
        .is_some());
    let sequence = candidate_engine.last_sequence();
    assert!(candidate_engine
        .organize(candidate_request)
        .unwrap()
        .decision
        .is_none());
    assert_eq!(candidate_engine.last_sequence(), sequence);

    let retraction_request = RetractOrganizedMemoryRequest {
        operation_id: "retraction:once".to_string(),
        memory_id: memory_id("candidate-once"),
        reason: "the user withdrew this preference".to_string(),
        retracted_at_ms: 21,
        evidence: vec![evidence("event:retraction-once", 20)],
    };
    assert!(candidate_engine
        .retract(retraction_request.clone())
        .unwrap()
        .decision
        .is_some());
    let sequence = candidate_engine.last_sequence();
    assert!(candidate_engine
        .retract(retraction_request)
        .unwrap()
        .decision
        .is_none());
    assert_eq!(candidate_engine.last_sequence(), sequence);

    assert!(candidate_engine.maintain(30).unwrap().decision.is_some());
    let sequence = candidate_engine.last_sequence();
    assert!(candidate_engine.maintain(30).unwrap().decision.is_none());
    assert_eq!(candidate_engine.last_sequence(), sequence);

    let mut resolution_engine = OrganizedMemoryDecisionEngine::new();
    resolution_engine
        .organize(candidate("style-a", "response_style", json!("brief"), 10))
        .unwrap();
    resolution_engine
        .organize(candidate(
            "style-b",
            "response_style",
            json!("detailed"),
            20,
        ))
        .unwrap();
    let request = conflict_resolution_request();
    assert!(resolution_engine
        .resolve_dispute(request.clone())
        .unwrap()
        .decision
        .is_some());
    let sequence = resolution_engine.last_sequence();
    assert!(resolution_engine
        .resolve_dispute(request)
        .unwrap()
        .decision
        .is_none());
    assert_eq!(resolution_engine.last_sequence(), sequence);
}

#[test]
fn dispute_resolution_captures_every_changed_record_in_one_atomic_batch() {
    let mut engine = OrganizedMemoryDecisionEngine::new();
    let mut decisions = vec![
        take_decision(
            engine
                .organize(candidate("style-a", "response_style", json!("brief"), 10))
                .unwrap(),
        ),
        take_decision(
            engine
                .organize(candidate(
                    "style-b",
                    "response_style",
                    json!("detailed"),
                    20,
                ))
                .unwrap(),
        ),
    ];
    let resolution = take_decision(
        engine
            .resolve_dispute(conflict_resolution_request())
            .unwrap(),
    );
    let changed_ids = resolution
        .delta
        .record_upserts
        .iter()
        .map(|record| record.memory_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        changed_ids,
        vec![memory_id("style-a"), memory_id("style-b")]
    );
    assert_eq!(resolution.delta.dispute_resolution_upserts.len(), 1);
    assert_eq!(
        resolution.delta.record_upserts[0].status,
        OrganizedMemoryStatus::Superseded
    );
    assert_eq!(
        resolution.delta.record_upserts[1].status,
        OrganizedMemoryStatus::Confirmed
    );
    decisions.push(resolution);

    let replayed = OrganizedMemoryDecisionEngine::replay(decisions).unwrap();
    assert_eq!(
        replayed.organizer().export_state(),
        engine.organizer().export_state()
    );
}

#[test]
fn retraction_delta_includes_conflict_detach_and_reconciliation_side_effects() {
    let mut engine = OrganizedMemoryDecisionEngine::new();
    let mut decisions = vec![
        take_decision(
            engine
                .organize(candidate("style-a", "response_style", json!("brief"), 10))
                .unwrap(),
        ),
        take_decision(
            engine
                .organize(candidate(
                    "style-b",
                    "response_style",
                    json!("detailed"),
                    20,
                ))
                .unwrap(),
        ),
    ];
    let retraction = take_decision(
        engine
            .retract(RetractOrganizedMemoryRequest {
                operation_id: "retraction:style-b".to_string(),
                memory_id: memory_id("style-b"),
                reason: "the user withdrew the second claim".to_string(),
                retracted_at_ms: 31,
                evidence: vec![evidence("event:retract-style-b", 30)],
            })
            .unwrap(),
    );
    let changed_ids = retraction
        .delta
        .record_upserts
        .iter()
        .map(|record| record.memory_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        changed_ids,
        vec![memory_id("style-a"), memory_id("style-b")]
    );
    assert_eq!(
        engine
            .organizer()
            .record(&memory_id("style-a"))
            .unwrap()
            .status,
        OrganizedMemoryStatus::Confirmed
    );
    decisions.push(retraction);

    let replayed = OrganizedMemoryDecisionEngine::replay(decisions).unwrap();
    assert_eq!(
        replayed.organizer().export_state(),
        engine.organizer().export_state()
    );
}

#[test]
fn replay_rejects_missing_reordered_tampered_and_incoherent_batches() {
    let mut engine = OrganizedMemoryDecisionEngine::new();
    let first = take_decision(
        engine
            .organize(candidate("first", "language", json!("Chinese"), 10))
            .unwrap(),
    );
    let second = take_decision(
        engine
            .organize(candidate("second", "timezone", json!("Asia/Shanghai"), 20))
            .unwrap(),
    );

    let error = OrganizedMemoryDecisionEngine::replay(vec![second.clone()]).unwrap_err();
    assert_eq!(
        error.code(),
        OrganizedMemoryDecisionErrorCode::SequenceMismatch
    );
    let error =
        OrganizedMemoryDecisionEngine::replay(vec![second.clone(), first.clone()]).unwrap_err();
    assert_eq!(
        error.code(),
        OrganizedMemoryDecisionErrorCode::SequenceMismatch
    );

    let mut payload_tampered = first.clone();
    payload_tampered.command_id = "candidate:tampered".to_string();
    let error = OrganizedMemoryDecisionEngine::replay(vec![payload_tampered]).unwrap_err();
    assert_eq!(error.code(), OrganizedMemoryDecisionErrorCode::HashMismatch);

    let mut revision_tampered = first.clone();
    revision_tampered.delta.revision = 8;
    revision_tampered.decision_hash = decision_hash(&revision_tampered);
    let error = OrganizedMemoryDecisionEngine::replay(vec![revision_tampered]).unwrap_err();
    assert_eq!(
        error.code(),
        OrganizedMemoryDecisionErrorCode::RevisionMismatch
    );

    let mut receipt_tampered = first;
    let OrganizedMemoryDecisionOperation::CandidateOrganized { receipt } =
        &mut receipt_tampered.operation
    else {
        panic!("expected candidate decision");
    };
    receipt.affected_memory_ids.clear();
    receipt_tampered.decision_hash = decision_hash(&receipt_tampered);
    let error = OrganizedMemoryDecisionEngine::replay(vec![receipt_tampered]).unwrap_err();
    assert_eq!(error.code(), OrganizedMemoryDecisionErrorCode::InvalidDelta);
}

#[test]
fn checkpoint_plus_tail_matches_genesis_replay_and_rejects_corruption() {
    let mut engine = OrganizedMemoryDecisionEngine::new();
    let mut all_decisions = Vec::new();
    all_decisions.push(take_decision(
        engine
            .organize(candidate("checkpoint-a", "language", json!("Chinese"), 10))
            .unwrap(),
    ));
    all_decisions.push(take_decision(
        engine
            .organize(candidate(
                "checkpoint-b",
                "timezone",
                json!("Asia/Shanghai"),
                20,
            ))
            .unwrap(),
    ));
    let checkpoint = engine.checkpoint();

    let tail = vec![
        take_decision(engine.maintain(30).unwrap()),
        take_decision(
            engine
                .organize(candidate("tail", "language", json!("Chinese"), 40))
                .unwrap(),
        ),
    ];
    all_decisions.extend(tail.clone());

    let restored =
        OrganizedMemoryDecisionEngine::from_checkpoint(checkpoint.clone(), tail).unwrap();
    let replayed = OrganizedMemoryDecisionEngine::replay(all_decisions.clone()).unwrap();
    assert_eq!(
        restored.organizer().export_state(),
        engine.organizer().export_state()
    );
    assert_eq!(
        replayed.organizer().export_state(),
        engine.organizer().export_state()
    );
    assert_eq!(restored.last_sequence(), engine.last_sequence());
    assert_eq!(restored.last_decision_hash(), engine.last_decision_hash());

    let mut corrupted = checkpoint;
    corrupted.state.revision = corrupted.state.revision.saturating_add(1);
    let error = OrganizedMemoryDecisionEngine::from_checkpoint(corrupted, Vec::new()).unwrap_err();
    assert_eq!(
        error.code(),
        OrganizedMemoryDecisionErrorCode::CheckpointMismatch
    );

    let mut sequence_mismatch = engine.checkpoint();
    sequence_mismatch.last_sequence = sequence_mismatch.last_sequence.saturating_sub(1);
    sequence_mismatch.checkpoint_hash = checkpoint_hash(&sequence_mismatch);
    let error =
        OrganizedMemoryDecisionEngine::from_checkpoint(sequence_mismatch, Vec::new()).unwrap_err();
    assert_eq!(
        error.code(),
        OrganizedMemoryDecisionErrorCode::CheckpointMismatch
    );

    let mut other_engine = OrganizedMemoryDecisionEngine::new();
    other_engine
        .organize(candidate(
            "other-checkpoint",
            "timezone",
            json!("Europe/Paris"),
            10,
        ))
        .unwrap();
    let other_checkpoint = other_engine.checkpoint();
    let mut mixed_checkpoint = OrganizedMemoryDecisionEngine::replay(
        all_decisions[..other_checkpoint.last_sequence as usize].to_vec(),
    )
    .unwrap()
    .checkpoint();
    assert_eq!(
        mixed_checkpoint.last_sequence,
        other_checkpoint.last_sequence
    );
    mixed_checkpoint.last_decision_hash = other_checkpoint.last_decision_hash;
    let error =
        OrganizedMemoryDecisionEngine::from_checkpoint(mixed_checkpoint, Vec::new()).unwrap_err();
    assert_eq!(
        error.code(),
        OrganizedMemoryDecisionErrorCode::CheckpointMismatch
    );
}

#[test]
fn decision_json_schema_is_stable_and_round_trips() {
    let mut engine = OrganizedMemoryDecisionEngine::new();
    let decision = take_decision(
        engine
            .organize(candidate("json", "language", json!("Chinese"), 10))
            .unwrap(),
    );
    let encoded = serde_json::to_value(&decision).unwrap();
    assert_eq!(encoded["schemaVersion"], json!(1));
    assert_eq!(encoded["organizerSchemaVersion"], json!(1));
    assert_eq!(encoded["policyVersion"], json!(1));
    assert_eq!(encoded["sequence"], json!(1));
    assert_eq!(
        encoded["previousDecisionHash"],
        json!(ORGANIZED_MEMORY_DECISION_GENESIS_HASH)
    );
    assert_eq!(encoded["operation"]["kind"], json!("candidate_organized"));
    assert!(encoded["delta"].get("recordUpserts").is_some());

    let decoded: OrganizedMemoryDecisionBatch = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(decoded, decision);

    let mut with_unknown_field = encoded;
    with_unknown_field
        .as_object_mut()
        .unwrap()
        .insert("unexpected".to_string(), json!(true));
    assert!(serde_json::from_value::<OrganizedMemoryDecisionBatch>(with_unknown_field).is_err());
}

#[test]
fn nested_protocol_objects_reject_unknown_fields() {
    let mut engine = OrganizedMemoryDecisionEngine::new();
    let decision = take_decision(
        engine
            .organize(candidate("strict-json", "language", json!("Chinese"), 10))
            .unwrap(),
    );

    let mut record_unknown = serde_json::to_value(&decision).unwrap();
    record_unknown["delta"]["recordUpserts"][0]
        .as_object_mut()
        .unwrap()
        .insert("unexpected".to_string(), json!(true));
    assert!(serde_json::from_value::<OrganizedMemoryDecisionBatch>(record_unknown).is_err());

    let mut evidence_unknown = serde_json::to_value(&decision).unwrap();
    evidence_unknown["delta"]["recordUpserts"][0]["supportingEvidence"][0]
        .as_object_mut()
        .unwrap()
        .insert("unexpected".to_string(), json!(true));
    assert!(serde_json::from_value::<OrganizedMemoryDecisionBatch>(evidence_unknown).is_err());

    let mut receipt_unknown = serde_json::to_value(&decision).unwrap();
    receipt_unknown["operation"]["data"]["receipt"]
        .as_object_mut()
        .unwrap()
        .insert("unexpected".to_string(), json!(true));
    assert!(serde_json::from_value::<OrganizedMemoryDecisionBatch>(receipt_unknown).is_err());

    let checkpoint = engine.checkpoint();
    let mut checkpoint_state_unknown = serde_json::to_value(checkpoint).unwrap();
    checkpoint_state_unknown["state"]
        .as_object_mut()
        .unwrap()
        .insert("unexpected".to_string(), json!(true));
    assert!(
        serde_json::from_value::<OrganizedMemoryDecisionCheckpoint>(checkpoint_state_unknown)
            .is_err()
    );
}

#[test]
fn policy_versions_are_audit_metadata_and_can_mix_within_one_history() {
    let mut engine = OrganizedMemoryDecisionEngine::new();
    let first = take_decision(
        engine
            .organize(candidate("policy-v1", "language", json!("Chinese"), 10))
            .unwrap(),
    );
    let checkpoint_v1 = engine.checkpoint();
    let mut second = take_decision(
        engine
            .organize(candidate(
                "policy-v2",
                "timezone",
                json!("Asia/Shanghai"),
                20,
            ))
            .unwrap(),
    );
    second.policy_version = 2;
    second.decision_hash = decision_hash(&second);

    let replayed =
        OrganizedMemoryDecisionEngine::replay(vec![first.clone(), second.clone()]).unwrap();
    let restored =
        OrganizedMemoryDecisionEngine::from_checkpoint(checkpoint_v1, vec![second]).unwrap();
    assert_eq!(
        replayed.organizer().export_state(),
        restored.organizer().export_state()
    );
    assert_eq!(replayed.last_sequence(), 2);

    let mut future_checkpoint = replayed.checkpoint();
    future_checkpoint.policy_version = 2;
    future_checkpoint.checkpoint_hash = checkpoint_hash(&future_checkpoint);
    assert!(OrganizedMemoryDecisionEngine::from_checkpoint(future_checkpoint, Vec::new()).is_ok());
}

#[test]
fn replay_rejects_a_forged_second_application_of_the_same_command() {
    let mut engine = OrganizedMemoryDecisionEngine::new();
    let first = take_decision(
        engine
            .organize(candidate("only-once", "language", json!("Chinese"), 10))
            .unwrap(),
    );
    let mut duplicate = first.clone();
    duplicate.sequence = 2;
    duplicate.previous_decision_hash = first.decision_hash.clone();
    duplicate.delta.base_revision = 1;
    duplicate.delta.revision = 2;
    let OrganizedMemoryDecisionOperation::CandidateOrganized { receipt } = &mut duplicate.operation
    else {
        panic!("expected candidate decision");
    };
    receipt.revision = 2;
    duplicate.delta.processed_candidate_upserts[0]
        .value
        .applied_revision = 2;
    duplicate.decision_hash = decision_hash(&duplicate);

    let error = OrganizedMemoryDecisionEngine::replay(vec![first, duplicate]).unwrap_err();
    assert_eq!(
        error.code(),
        OrganizedMemoryDecisionErrorCode::DuplicateCommand
    );
}

#[test]
fn replay_rejects_an_invalid_intermediate_post_image_even_if_later_repaired() {
    let mut engine = OrganizedMemoryDecisionEngine::new();
    let mut first = take_decision(
        engine
            .organize(candidate(
                "intermediate-a",
                "language",
                json!("Chinese"),
                10,
            ))
            .unwrap(),
    );
    let mut second = take_decision(
        engine
            .organize(candidate(
                "intermediate-b",
                "language",
                json!("Chinese"),
                20,
            ))
            .unwrap(),
    );
    first.delta.record_upserts[0].subject.clear();
    first.decision_hash = decision_hash(&first);
    second.previous_decision_hash = first.decision_hash.clone();
    second.decision_hash = decision_hash(&second);

    let error = OrganizedMemoryDecisionEngine::replay(vec![first, second]).unwrap_err();
    assert_eq!(
        error.code(),
        OrganizedMemoryDecisionErrorCode::OrganizerRejected
    );
}

#[test]
fn checkpoint_rejects_noncanonical_uppercase_fingerprints() {
    let candidate_request = candidate("canonical-digest", "language", json!("Chinese"), 10);
    let mut engine = OrganizedMemoryDecisionEngine::new();
    engine.organize(candidate_request).unwrap();
    let mut checkpoint = engine.checkpoint();
    checkpoint
        .state
        .processed_candidates
        .get_mut("canonical-digest")
        .unwrap()
        .candidate_fingerprint
        .make_ascii_uppercase();
    checkpoint.state_hash = fingerprint_serializable(&checkpoint.state);
    checkpoint.checkpoint_hash = checkpoint_hash(&checkpoint);

    let error = OrganizedMemoryDecisionEngine::from_checkpoint(checkpoint, Vec::new()).unwrap_err();
    assert_eq!(
        error.code(),
        OrganizedMemoryDecisionErrorCode::OrganizerRejected
    );
}

#[test]
fn every_complex_history_prefix_is_independently_restorable() {
    let mut engine = OrganizedMemoryDecisionEngine::new();
    let mut decisions = Vec::new();
    let mut expected_states = Vec::new();

    decisions.push(take_decision(
        engine
            .organize(candidate("style-a", "response_style", json!("brief"), 10))
            .unwrap(),
    ));
    expected_states.push(engine.organizer().export_state());
    decisions.push(take_decision(
        engine
            .organize(candidate(
                "style-b",
                "response_style",
                json!("detailed"),
                20,
            ))
            .unwrap(),
    ));
    expected_states.push(engine.organizer().export_state());
    decisions.push(take_decision(
        engine
            .resolve_dispute(conflict_resolution_request())
            .unwrap(),
    ));
    expected_states.push(engine.organizer().export_state());

    let mut replacement = candidate("style-c", "response_style", json!("structured"), 40);
    replacement.intent = MemoryCandidateIntent::Replace;
    replacement.target_memory_id = Some(memory_id("style-b"));
    decisions.push(take_decision(engine.organize(replacement).unwrap()));
    expected_states.push(engine.organizer().export_state());

    decisions.push(take_decision(
        engine
            .retract(RetractOrganizedMemoryRequest {
                operation_id: "retraction:style-c".to_string(),
                memory_id: memory_id("style-c"),
                reason: "the user withdrew the replacement".to_string(),
                retracted_at_ms: 51,
                evidence: vec![evidence("event:retract-style-c", 50)],
            })
            .unwrap(),
    ));
    expected_states.push(engine.organizer().export_state());

    for prefix_len in 1..=decisions.len() {
        let restored =
            OrganizedMemoryDecisionEngine::replay(decisions[..prefix_len].to_vec()).unwrap();
        assert_eq!(
            restored.organizer().export_state(),
            expected_states[prefix_len - 1],
            "history prefix {prefix_len} did not restore exactly"
        );
        assert_eq!(restored.last_sequence(), prefix_len as u64);
    }
}

#[test]
fn oversized_resolution_is_rejected_before_any_state_changes() {
    let mut seed = MemoryOrganizer::new();
    seed.organize(candidate(
        "topology-seed",
        "response_style",
        json!("seed"),
        10,
    ))
    .unwrap();
    let template = seed.record(&memory_id("topology-seed")).unwrap().clone();
    let winner_id = memory_id("winner");
    let loser_ids = (0..4)
        .map(|index| memory_id(&format!("loser-{index}")))
        .collect::<Vec<_>>();
    let mut records = BTreeMap::new();

    let mut winner = template.clone();
    winner.memory_id = winner_id.clone();
    winner.value = json!("winner");
    winner.status = OrganizedMemoryStatus::Disputed;
    winner.conflicts_with_memory_ids = loser_ids.iter().cloned().collect();
    records.insert(winner.memory_id.clone(), winner);

    for (loser_index, loser_id) in loser_ids.iter().enumerate() {
        let peer_ids = (0..31)
            .map(|peer_index| memory_id(&format!("peer-{loser_index}-{peer_index}")))
            .collect::<Vec<_>>();
        let mut loser = template.clone();
        loser.memory_id = loser_id.clone();
        loser.value = json!(format!("loser-{loser_index}"));
        loser.status = OrganizedMemoryStatus::Disputed;
        loser.conflicts_with_memory_ids = std::iter::once(winner_id.clone())
            .chain(peer_ids.iter().cloned())
            .collect::<BTreeSet<_>>();
        records.insert(loser.memory_id.clone(), loser);

        for (peer_index, peer_id) in peer_ids.into_iter().enumerate() {
            let mut peer = template.clone();
            peer.memory_id = peer_id;
            peer.value = json!(format!("peer-{loser_index}-{peer_index}"));
            peer.status = OrganizedMemoryStatus::Disputed;
            peer.conflicts_with_memory_ids = BTreeSet::from([loser_id.clone()]);
            records.insert(peer.memory_id.clone(), peer);
        }
    }
    assert_eq!(records.len(), MAX_MEMORY_IDS_PER_RECEIPT + 1);

    let mut state = MemoryOrganizerState::default();
    state.records = records;
    let organizer = MemoryOrganizer::from_state(state).unwrap();
    let mut engine = OrganizedMemoryDecisionEngine {
        organizer,
        last_sequence: 0,
        last_decision_hash: ORGANIZED_MEMORY_DECISION_GENESIS_HASH.to_string(),
    };
    let before = engine.organizer().export_state();
    let error = engine
        .resolve_dispute(ResolveMemoryDisputeRequest {
            operation_id: "resolution:oversized-graph".to_string(),
            winner_memory_id: winner_id,
            losing_memory_ids: loser_ids,
            reason: "this graph must be consolidated before one atomic resolution".to_string(),
            resolved_at_ms: 31,
            evidence: vec![evidence("event:oversized-resolution", 30)],
        })
        .unwrap_err();
    assert_eq!(
        error.code(),
        OrganizedMemoryDecisionErrorCode::OrganizerRejected
    );
    assert_eq!(engine.organizer().export_state(), before);
    assert_eq!(engine.last_sequence(), 0);
    assert_eq!(
        engine.last_decision_hash(),
        ORGANIZED_MEMORY_DECISION_GENESIS_HASH
    );
}

#[test]
fn restored_state_rejects_a_noncanonical_live_status() {
    let mut organizer = MemoryOrganizer::new();
    organizer
        .organize(candidate(
            "noncanonical-status",
            "language",
            json!("Chinese"),
            10,
        ))
        .unwrap();
    let mut state = organizer.export_state();
    state
        .records
        .get_mut(&memory_id("noncanonical-status"))
        .unwrap()
        .status = OrganizedMemoryStatus::Provisional;
    assert!(MemoryOrganizer::from_state(state).is_err());
}
