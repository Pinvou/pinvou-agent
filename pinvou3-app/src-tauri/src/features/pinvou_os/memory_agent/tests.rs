use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use super::*;

fn applicability(
    environment: &[(&str, &str)],
    valid_from_ms: i64,
    valid_until_ms: Option<i64>,
) -> MemoryApplicability {
    MemoryApplicability {
        space_id: "personal".to_string(),
        environment: environment
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        valid_from_ms,
        valid_until_ms,
    }
}

fn evidence(event_id: &str, origin: MemoryEvidenceOrigin, observed_at_ms: i64) -> MemoryEvidence {
    MemoryEvidence {
        event_id: event_id.to_string(),
        source_actor_id: match origin {
            MemoryEvidenceOrigin::UserExplicit => "actor:user",
            MemoryEvidenceOrigin::ObservedBehavior => "agent:observer",
            MemoryEvidenceOrigin::AgentAction => "agent:worker",
            MemoryEvidenceOrigin::VerifiedTaskOutcome => "kernel:task-outcome-verifier",
            MemoryEvidenceOrigin::ExternalSource => "source:external",
            MemoryEvidenceOrigin::ModelInference => "agent:extractor",
        }
        .to_string(),
        origin,
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
    kind: OrganizedMemoryKind,
    value: serde_json::Value,
    origin: MemoryEvidenceOrigin,
    observed_at_ms: i64,
) -> MemoryCandidate {
    MemoryCandidate {
        candidate_id: candidate_id.to_string(),
        kind,
        subject: "user".to_string(),
        predicate: "response_style".to_string(),
        value,
        applicability: applicability(&[], observed_at_ms, None),
        importance: 0.8,
        confidence: 0.9,
        intent: MemoryCandidateIntent::Assert,
        target_memory_id: None,
        evidence: vec![evidence(
            &format!("event:{candidate_id}"),
            origin,
            observed_at_ms,
        )],
    }
}

fn memory_id(candidate_id: &str) -> String {
    format!("memory:organized:{candidate_id}")
}

fn assert_restorable(organizer: &MemoryOrganizer) {
    let state = organizer.export_state();
    assert_eq!(
        organizer.retrieval_index.record_count(),
        state.records.len()
    );
    let serialized = serde_json::to_vec(&state).unwrap();
    let decoded: MemoryOrganizerState = serde_json::from_slice(&serialized).unwrap();
    let restored = MemoryOrganizer::from_state(decoded).unwrap();
    assert_eq!(
        restored.retrieval_index.record_count(),
        restored.state.records.len()
    );
}

/// 保留优化前的全扫描算法作为差分测试基准，防止派生索引改变查询语义。
fn reference_project(
    organizer: &MemoryOrganizer,
    query: OrganizedMemoryQuery,
) -> OrganizedMemoryProjection {
    let query = normalize_query(query).unwrap();
    let kinds = query.kinds.iter().copied().collect::<BTreeSet<_>>();
    let subjects = query.subjects.iter().cloned().collect::<BTreeSet<_>>();
    let predicates = query.predicates.iter().cloned().collect::<BTreeSet<_>>();
    let eligible = organizer
        .state
        .records
        .values()
        .filter(|record| record_is_visible(record, &query))
        .filter(|record| kinds.is_empty() || kinds.contains(&record.kind))
        .filter(|record| subjects.is_empty() || subjects.contains(&record.subject))
        .filter(|record| predicates.is_empty() || predicates.contains(&record.predicate))
        .collect::<Vec<_>>();
    let maximum_specificity = eligible.iter().fold(
        BTreeMap::<MemorySlotKey, usize>::new(),
        |mut maximum, record| {
            maximum
                .entry(MemorySlotKey::from_record(record))
                .and_modify(|value| *value = (*value).max(record.applicability.environment.len()))
                .or_insert(record.applicability.environment.len());
            maximum
        },
    );
    let mut ranked = eligible
        .into_iter()
        .filter(|record| {
            maximum_specificity
                .get(&MemorySlotKey::from_record(record))
                .is_none_or(|maximum| record.applicability.environment.len() == *maximum)
        })
        .map(|record| (selection_score(record, &query), record))
        .collect::<Vec<_>>();
    ranked.sort_by(compare_ranked);
    let omitted_count = ranked.len().saturating_sub(query.max_items);
    ranked.truncate(query.max_items);
    let score_sum = ranked.iter().map(|(score, _)| *score).sum::<f32>();
    let fallback_weight = if ranked.is_empty() {
        0.0
    } else {
        1.0 / ranked.len() as f32
    };
    let items = ranked
        .into_iter()
        .map(|(score, record)| OrganizedMemoryContextItem {
            memory_id: record.memory_id.clone(),
            kind: record.kind,
            subject: record.subject.clone(),
            predicate: record.predicate.clone(),
            value: record.value.clone(),
            applicability: record.applicability.clone(),
            status: record.status,
            importance: record.importance,
            confidence: record.confidence,
            selection_score: score,
            selection_weight: if score_sum > 0.0 {
                score / score_sum
            } else {
                fallback_weight
            },
            evidence_event_ids: all_evidence(record)
                .map(|evidence| evidence.event_id.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        })
        .collect::<Vec<_>>();
    OrganizedMemoryProjection {
        revision: organizer.state.revision,
        generated_at_ms: query.current_at_ms,
        omitted_count,
        evidence_event_ids: items
            .iter()
            .flat_map(|item| item.evidence_event_ids.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        items,
    }
}

#[test]
fn explicit_user_preference_is_confirmed_but_model_inference_is_not() {
    let mut organizer = MemoryOrganizer::new();
    organizer
        .organize(candidate(
            "explicit",
            OrganizedMemoryKind::Preference,
            json!("concise"),
            MemoryEvidenceOrigin::UserExplicit,
            10,
        ))
        .unwrap();

    let mut inferred = candidate(
        "inferred",
        OrganizedMemoryKind::Preference,
        json!("formal"),
        MemoryEvidenceOrigin::ModelInference,
        20,
    );
    inferred.predicate = "tone".to_string();
    inferred.confidence = 1.0;
    inferred.evidence[0].reliability = 1.0;
    organizer.organize(inferred).unwrap();

    assert_eq!(
        organizer.record(&memory_id("explicit")).unwrap().status,
        OrganizedMemoryStatus::Confirmed
    );
    assert_eq!(
        organizer.record(&memory_id("inferred")).unwrap().status,
        OrganizedMemoryStatus::Provisional
    );
}

#[test]
fn same_value_merges_distinct_evidence_and_candidate_replay_is_idempotent() {
    let mut organizer = MemoryOrganizer::new();
    let first = candidate(
        "first",
        OrganizedMemoryKind::Preference,
        json!("中文"),
        MemoryEvidenceOrigin::UserExplicit,
        10,
    );
    organizer.organize(first.clone()).unwrap();

    let second = candidate(
        "second",
        OrganizedMemoryKind::Preference,
        json!("中文"),
        MemoryEvidenceOrigin::ObservedBehavior,
        20,
    );
    let merged = organizer.organize(second.clone()).unwrap();
    assert_eq!(merged.action, MemoryOrganizationAction::Merged);
    assert_eq!(organizer.state().records.len(), 1);
    assert_eq!(
        organizer
            .record(&memory_id("first"))
            .unwrap()
            .supporting_evidence
            .len(),
        2
    );

    let revision = organizer.state().revision;
    let replayed = organizer.organize(second).unwrap();
    assert_eq!(replayed.action, MemoryOrganizationAction::IgnoredDuplicate);
    assert_eq!(organizer.state().revision, revision);
    assert_eq!(organizer.state().records.len(), 1);
}

#[test]
fn repeated_evidence_does_not_inflate_confidence() {
    let mut organizer = MemoryOrganizer::new();
    let first = candidate(
        "same-evidence-first",
        OrganizedMemoryKind::Preference,
        json!("concise"),
        MemoryEvidenceOrigin::ObservedBehavior,
        10,
    );
    let shared_evidence = first.evidence[0].clone();
    organizer.organize(first).unwrap();
    let before = organizer
        .record(&memory_id("same-evidence-first"))
        .unwrap()
        .clone();

    let mut second = candidate(
        "same-evidence-second",
        OrganizedMemoryKind::Preference,
        json!("concise"),
        MemoryEvidenceOrigin::ObservedBehavior,
        20,
    );
    second.evidence = vec![shared_evidence];
    second.importance = 1.0;
    second.applicability.valid_from_ms = 0;
    let receipt = organizer.organize(second).unwrap();

    assert_eq!(receipt.action, MemoryOrganizationAction::IgnoredDuplicate);
    assert_eq!(
        organizer.record(&memory_id("same-evidence-first")).unwrap(),
        &before
    );
    assert_restorable(&organizer);
}

#[test]
fn explicit_new_preference_supersedes_old_version_without_erasing_it() {
    let mut organizer = MemoryOrganizer::new();
    organizer
        .organize(candidate(
            "old-style",
            OrganizedMemoryKind::Preference,
            json!("short"),
            MemoryEvidenceOrigin::UserExplicit,
            10,
        ))
        .unwrap();

    let mut replacement = candidate(
        "new-style",
        OrganizedMemoryKind::Preference,
        json!("detailed"),
        MemoryEvidenceOrigin::UserExplicit,
        20,
    );
    replacement.intent = MemoryCandidateIntent::Replace;
    replacement.target_memory_id = Some(memory_id("old-style"));
    let receipt = organizer.organize(replacement).unwrap();

    assert_eq!(receipt.action, MemoryOrganizationAction::Superseded);
    let old = organizer.record(&memory_id("old-style")).unwrap();
    let new = organizer.record(&memory_id("new-style")).unwrap();
    assert_eq!(old.status, OrganizedMemoryStatus::Superseded);
    assert_eq!(old.applicability.valid_until_ms, Some(20));
    assert_eq!(
        old.superseded_by_memory_id.as_deref(),
        Some(memory_id("new-style").as_str())
    );
    assert_eq!(new.status, OrganizedMemoryStatus::Confirmed);
    assert_eq!(
        new.supersedes_memory_ids,
        BTreeSet::from([memory_id("old-style")])
    );
}

#[test]
fn model_inference_cannot_replace_confirmed_user_statement() {
    let mut organizer = MemoryOrganizer::new();
    organizer
        .organize(candidate(
            "explicit",
            OrganizedMemoryKind::Preference,
            json!("quiet"),
            MemoryEvidenceOrigin::UserExplicit,
            10,
        ))
        .unwrap();
    let mut inferred = candidate(
        "guess",
        OrganizedMemoryKind::Preference,
        json!("verbose"),
        MemoryEvidenceOrigin::ModelInference,
        20,
    );
    inferred.intent = MemoryCandidateIntent::Replace;
    inferred.target_memory_id = Some(memory_id("explicit"));
    inferred.confidence = 1.0;
    inferred.evidence[0].reliability = 1.0;

    assert!(organizer.organize(inferred).is_err());
    assert_eq!(organizer.state().revision, 1);
    assert_eq!(
        organizer.record(&memory_id("explicit")).unwrap().status,
        OrganizedMemoryStatus::Confirmed
    );
}

#[test]
fn equally_authoritative_overlapping_claims_become_disputed() {
    let mut organizer = MemoryOrganizer::new();
    organizer
        .organize(candidate(
            "claim-a",
            OrganizedMemoryKind::ContextualFact,
            json!("Beijing"),
            MemoryEvidenceOrigin::UserExplicit,
            10,
        ))
        .unwrap();
    let receipt = organizer
        .organize(candidate(
            "claim-b",
            OrganizedMemoryKind::ContextualFact,
            json!("Shanghai"),
            MemoryEvidenceOrigin::UserExplicit,
            20,
        ))
        .unwrap();

    assert_eq!(receipt.action, MemoryOrganizationAction::Disputed);
    let left = organizer.record(&memory_id("claim-a")).unwrap();
    let right = organizer.record(&memory_id("claim-b")).unwrap();
    assert_eq!(left.status, OrganizedMemoryStatus::Disputed);
    assert_eq!(right.status, OrganizedMemoryStatus::Disputed);
    assert!(left.conflicts_with_memory_ids.contains(&right.memory_id));
    assert!(right.conflicts_with_memory_ids.contains(&left.memory_id));
    assert!(left.contradicting_evidence.is_empty());
    assert!(right.contradicting_evidence.is_empty());
}

#[test]
fn different_environments_do_not_conflict() {
    let mut organizer = MemoryOrganizer::new();
    let mut tablet = candidate(
        "tablet",
        OrganizedMemoryKind::Preference,
        json!("voice"),
        MemoryEvidenceOrigin::UserExplicit,
        10,
    );
    tablet.applicability = applicability(&[("device", "tablet")], 10, None);
    let mut desktop = candidate(
        "desktop",
        OrganizedMemoryKind::Preference,
        json!("keyboard"),
        MemoryEvidenceOrigin::UserExplicit,
        20,
    );
    desktop.applicability = applicability(&[("device", "desktop")], 20, None);

    organizer.organize(tablet).unwrap();
    organizer.organize(desktop).unwrap();
    assert_eq!(organizer.state().records.len(), 2);
    assert!(organizer
        .state()
        .records
        .values()
        .all(|record| record.status == OrganizedMemoryStatus::Confirmed));
}

#[test]
fn habit_requires_three_independent_behavior_events() {
    let mut organizer = MemoryOrganizer::new();
    for index in 1..=3 {
        organizer
            .organize(candidate(
                &format!("habit-{index}"),
                OrganizedMemoryKind::Habit,
                json!("reviews_before_sending"),
                MemoryEvidenceOrigin::ObservedBehavior,
                index * 10,
            ))
            .unwrap();
        let expected = if index < 3 {
            OrganizedMemoryStatus::Provisional
        } else {
            OrganizedMemoryStatus::Confirmed
        };
        assert_eq!(
            organizer.record(&memory_id("habit-1")).unwrap().status,
            expected
        );
    }
    assert_eq!(organizer.state().records.len(), 1);
}

#[test]
fn action_experiences_are_independent_episodes_and_lessons_need_repetition() {
    let mut organizer = MemoryOrganizer::new();
    for index in 1..=2 {
        organizer
            .organize(candidate(
                &format!("experience-{index}"),
                OrganizedMemoryKind::ActionExperience,
                json!({"action": "verify", "outcome": "success", "episode": index}),
                MemoryEvidenceOrigin::VerifiedTaskOutcome,
                index * 10,
            ))
            .unwrap();
    }
    assert_eq!(organizer.state().records.len(), 2);

    organizer
        .organize(candidate(
            "lesson-1",
            OrganizedMemoryKind::Lesson,
            json!("verify_production_path"),
            MemoryEvidenceOrigin::VerifiedTaskOutcome,
            30,
        ))
        .unwrap();
    assert_eq!(
        organizer.record(&memory_id("lesson-1")).unwrap().status,
        OrganizedMemoryStatus::Provisional
    );
    organizer
        .organize(candidate(
            "lesson-2",
            OrganizedMemoryKind::Lesson,
            json!("verify_production_path"),
            MemoryEvidenceOrigin::VerifiedTaskOutcome,
            40,
        ))
        .unwrap();
    assert_eq!(
        organizer.record(&memory_id("lesson-1")).unwrap().status,
        OrganizedMemoryStatus::Confirmed
    );
}

#[test]
fn projection_respects_scope_time_status_and_normalizes_selection_weights() {
    let mut organizer = MemoryOrganizer::new();
    let mut global = candidate(
        "global",
        OrganizedMemoryKind::Preference,
        json!("Chinese"),
        MemoryEvidenceOrigin::UserExplicit,
        10,
    );
    global.importance = 0.9;
    organizer.organize(global).unwrap();

    let mut tablet = candidate(
        "tablet",
        OrganizedMemoryKind::Preference,
        json!("voice"),
        MemoryEvidenceOrigin::UserExplicit,
        20,
    );
    tablet.predicate = "input_mode".to_string();
    tablet.applicability = applicability(&[("device", "tablet")], 20, Some(100));
    organizer.organize(tablet).unwrap();

    let mut provisional = candidate(
        "guess",
        OrganizedMemoryKind::Preference,
        json!("formal"),
        MemoryEvidenceOrigin::ModelInference,
        30,
    );
    provisional.predicate = "tone".to_string();
    organizer.organize(provisional).unwrap();

    let projection = organizer
        .project(OrganizedMemoryQuery {
            current_at_ms: 50,
            space_id: "personal".to_string(),
            environment: BTreeMap::from([("device".to_string(), "tablet".to_string())]),
            focus_terms: vec!["input_mode".to_string()],
            max_items: 8,
            ..OrganizedMemoryQuery::default()
        })
        .unwrap();
    assert_eq!(projection.items.len(), 2);
    assert_eq!(projection.items[0].memory_id, memory_id("tablet"));
    let weight_sum = projection
        .items
        .iter()
        .map(|item| item.selection_weight)
        .sum::<f32>();
    assert!((weight_sum - 1.0).abs() < 0.0001);

    let after_expiry = organizer
        .project(OrganizedMemoryQuery {
            current_at_ms: 100,
            space_id: "personal".to_string(),
            environment: BTreeMap::from([("device".to_string(), "tablet".to_string())]),
            max_items: 8,
            ..OrganizedMemoryQuery::default()
        })
        .unwrap();
    assert_eq!(after_expiry.items.len(), 1);
    assert_eq!(after_expiry.items[0].memory_id, memory_id("global"));
}

#[test]
fn maintain_expires_records_and_retraction_is_idempotent() {
    let mut organizer = MemoryOrganizer::new();
    let mut expiring = candidate(
        "expiring",
        OrganizedMemoryKind::ContextualFact,
        json!("temporary"),
        MemoryEvidenceOrigin::UserExplicit,
        10,
    );
    expiring.applicability.valid_until_ms = Some(20);
    organizer.organize(expiring).unwrap();
    let report = organizer.maintain(20).unwrap();
    assert_eq!(report.expired_memory_ids, vec![memory_id("expiring")]);
    assert_eq!(
        organizer.record(&memory_id("expiring")).unwrap().status,
        OrganizedMemoryStatus::Confirmed
    );

    organizer
        .organize(candidate(
            "retractable",
            OrganizedMemoryKind::ContextualFact,
            json!("wrong"),
            MemoryEvidenceOrigin::UserExplicit,
            30,
        ))
        .unwrap();
    let request = RetractOrganizedMemoryRequest {
        operation_id: "retract:one".to_string(),
        memory_id: memory_id("retractable"),
        reason: "user corrected it".to_string(),
        retracted_at_ms: 40,
        evidence: vec![evidence(
            "event:retract",
            MemoryEvidenceOrigin::UserExplicit,
            39,
        )],
    };
    assert!(organizer.retract(request.clone()).unwrap().changed);
    let revision = organizer.state().revision;
    assert!(!organizer.retract(request.clone()).unwrap().changed);
    assert_eq!(organizer.state().revision, revision);
    assert_eq!(
        organizer.record(&memory_id("retractable")).unwrap().status,
        OrganizedMemoryStatus::Retracted
    );
    let mut altered_replay = request;
    altered_replay.reason = "same operation id, changed reason".to_string();
    assert!(organizer.retract(altered_replay).is_err());
    assert_eq!(organizer.state().revision, revision);
    MemoryOrganizer::from_state(organizer.export_state()).unwrap();
}

#[test]
fn serialized_state_restores_indexes_and_has_no_session_partition() {
    let mut organizer = MemoryOrganizer::new();
    organizer
        .organize(candidate(
            "persisted",
            OrganizedMemoryKind::Preference,
            json!("Chinese"),
            MemoryEvidenceOrigin::UserExplicit,
            10,
        ))
        .unwrap();
    let raw = serde_json::to_string(&organizer.export_state()).unwrap();
    assert!(!raw.to_ascii_lowercase().contains("session"));
    let state: MemoryOrganizerState = serde_json::from_str(&raw).unwrap();
    let mut restored = MemoryOrganizer::from_state(state).unwrap();

    let receipt = restored
        .organize(candidate(
            "after-restore",
            OrganizedMemoryKind::Preference,
            json!("Chinese"),
            MemoryEvidenceOrigin::ObservedBehavior,
            20,
        ))
        .unwrap();
    assert_eq!(receipt.action, MemoryOrganizationAction::Merged);
    assert_eq!(restored.state().records.len(), 1);
}

#[test]
fn raw_credentials_and_live_task_progress_are_rejected() {
    let mut organizer = MemoryOrganizer::new();
    let mut credential = candidate(
        "secret",
        OrganizedMemoryKind::ContextualFact,
        json!("plaintext-token"),
        MemoryEvidenceOrigin::UserExplicit,
        10,
    );
    credential.subject = "account".to_string();
    credential.predicate = "api_token".to_string();
    assert!(organizer.organize(credential).is_err());

    let mut task = candidate(
        "task-progress",
        OrganizedMemoryKind::ContextualFact,
        json!("80%"),
        MemoryEvidenceOrigin::AgentAction,
        20,
    );
    task.subject = "mission:quarterly-report".to_string();
    task.predicate = "progress".to_string();
    assert!(organizer.organize(task).is_err());
    assert_eq!(organizer.state().revision, 0);
}

#[test]
fn keyring_reference_is_allowed_and_batch_is_bounded() {
    let mut organizer = MemoryOrganizer::new();
    let mut credential = candidate(
        "credential-ref",
        OrganizedMemoryKind::ContextualFact,
        json!({"credentialRef": "keyring://pinvou/account"}),
        MemoryEvidenceOrigin::UserExplicit,
        10,
    );
    credential.subject = "account".to_string();
    credential.predicate = "api_token".to_string();
    organizer.organize(credential).unwrap();

    let oversized = (0..=MAX_MEMORY_CANDIDATES_PER_BATCH)
        .map(|index| {
            candidate(
                &format!("batch-{index}"),
                OrganizedMemoryKind::ActionExperience,
                json!(index),
                MemoryEvidenceOrigin::AgentAction,
                index as i64 + 100,
            )
        })
        .collect();
    assert!(organizer.organize_batch(oversized).is_err());
}

#[test]
fn same_candidate_id_with_different_content_is_rejected() {
    let mut organizer = MemoryOrganizer::new();
    let original = candidate(
        "stable-id",
        OrganizedMemoryKind::Preference,
        json!("concise"),
        MemoryEvidenceOrigin::UserExplicit,
        10,
    );
    organizer.organize(original.clone()).unwrap();
    let revision = organizer.state().revision;

    let mut altered = original;
    altered.value = json!("verbose");
    assert!(organizer.organize(altered).is_err());
    assert_eq!(organizer.state().revision, revision);
    MemoryOrganizer::from_state(organizer.export_state()).unwrap();
}

#[test]
fn replacement_with_same_start_time_is_atomic_and_rejected() {
    let mut organizer = MemoryOrganizer::new();
    organizer
        .organize(candidate(
            "original",
            OrganizedMemoryKind::Preference,
            json!("old"),
            MemoryEvidenceOrigin::UserExplicit,
            10,
        ))
        .unwrap();
    let before = organizer.export_state();
    let mut replacement = candidate(
        "same-start",
        OrganizedMemoryKind::Preference,
        json!("new"),
        MemoryEvidenceOrigin::UserExplicit,
        10,
    );
    replacement.intent = MemoryCandidateIntent::Replace;
    replacement.target_memory_id = Some(memory_id("original"));

    assert!(organizer.organize(replacement).is_err());
    assert_eq!(organizer.export_state(), before);
    MemoryOrganizer::from_state(before).unwrap();
}

#[test]
fn same_value_replacement_cannot_move_a_fact_backwards() {
    let mut organizer = MemoryOrganizer::new();
    organizer
        .organize(candidate(
            "same-value-target",
            OrganizedMemoryKind::Preference,
            json!("concise"),
            MemoryEvidenceOrigin::UserExplicit,
            10,
        ))
        .unwrap();
    let before = organizer.export_state();
    let mut replacement = candidate(
        "same-value-backdated",
        OrganizedMemoryKind::Preference,
        json!("concise"),
        MemoryEvidenceOrigin::UserExplicit,
        20,
    );
    replacement.intent = MemoryCandidateIntent::Replace;
    replacement.target_memory_id = Some(memory_id("same-value-target"));
    replacement.applicability.valid_from_ms = 5;

    assert!(organizer.organize(replacement).is_err());
    assert_eq!(organizer.export_state(), before);
    assert_restorable(&organizer);
}

#[test]
fn same_value_replacement_after_a_time_gap_keeps_two_versions() {
    let mut organizer = MemoryOrganizer::new();
    let mut original = candidate(
        "same-value-old-period",
        OrganizedMemoryKind::Preference,
        json!("concise"),
        MemoryEvidenceOrigin::UserExplicit,
        10,
    );
    original.applicability.valid_until_ms = Some(20);
    organizer.organize(original).unwrap();

    let mut later = candidate(
        "same-value-new-period",
        OrganizedMemoryKind::Preference,
        json!("concise"),
        MemoryEvidenceOrigin::UserExplicit,
        30,
    );
    later.intent = MemoryCandidateIntent::Replace;
    later.target_memory_id = Some(memory_id("same-value-old-period"));
    organizer.organize(later).unwrap();

    assert_eq!(organizer.state().records.len(), 2);
    assert_eq!(
        organizer
            .record(&memory_id("same-value-old-period"))
            .unwrap()
            .applicability
            .valid_until_ms,
        Some(20)
    );
    MemoryOrganizer::from_state(organizer.export_state()).unwrap();
}

#[test]
fn bridge_across_multiple_same_value_periods_is_rejected_atomically() {
    let mut organizer = MemoryOrganizer::new();
    let mut first = candidate(
        "period-one",
        OrganizedMemoryKind::Preference,
        json!("concise"),
        MemoryEvidenceOrigin::UserExplicit,
        10,
    );
    first.applicability = applicability(&[], 0, Some(10));
    organizer.organize(first).unwrap();
    let mut second = candidate(
        "period-two",
        OrganizedMemoryKind::Preference,
        json!("concise"),
        MemoryEvidenceOrigin::UserExplicit,
        20,
    );
    second.applicability = applicability(&[], 20, Some(30));
    organizer.organize(second).unwrap();
    let before = organizer.export_state();

    let mut bridge = candidate(
        "period-bridge",
        OrganizedMemoryKind::Preference,
        json!("concise"),
        MemoryEvidenceOrigin::UserExplicit,
        40,
    );
    bridge.applicability = applicability(&[], 5, Some(25));
    assert!(organizer.organize(bridge).is_err());
    assert_eq!(organizer.export_state(), before);
    assert_restorable(&organizer);
}

#[test]
fn extending_a_time_range_reconciles_newly_overlapping_conflicts() {
    let mut organizer = MemoryOrganizer::new();
    let mut first = candidate(
        "range-a",
        OrganizedMemoryKind::ContextualFact,
        json!("A"),
        MemoryEvidenceOrigin::UserExplicit,
        10,
    );
    first.applicability = applicability(&[], 0, Some(10));
    organizer.organize(first).unwrap();
    let mut second = candidate(
        "range-b",
        OrganizedMemoryKind::ContextualFact,
        json!("B"),
        MemoryEvidenceOrigin::UserExplicit,
        20,
    );
    second.applicability = applicability(&[], 20, Some(30));
    organizer.organize(second).unwrap();

    let mut extension = candidate(
        "range-a-extension",
        OrganizedMemoryKind::ContextualFact,
        json!("A"),
        MemoryEvidenceOrigin::UserExplicit,
        40,
    );
    extension.applicability = applicability(&[], 5, Some(25));
    organizer.organize(extension).unwrap();

    assert_eq!(
        organizer.record(&memory_id("range-a")).unwrap().status,
        OrganizedMemoryStatus::Disputed
    );
    assert_eq!(
        organizer.record(&memory_id("range-b")).unwrap().status,
        OrganizedMemoryStatus::Disputed
    );
    assert_restorable(&organizer);
}

#[test]
fn scoped_exception_overrides_global_value_without_creating_a_false_conflict() {
    let mut organizer = MemoryOrganizer::new();
    organizer
        .organize(candidate(
            "global-mode",
            OrganizedMemoryKind::Preference,
            json!("keyboard"),
            MemoryEvidenceOrigin::UserExplicit,
            10,
        ))
        .unwrap();
    let mut tablet = candidate(
        "tablet-mode",
        OrganizedMemoryKind::Preference,
        json!("voice"),
        MemoryEvidenceOrigin::UserExplicit,
        20,
    );
    tablet.applicability = applicability(&[("device", "tablet")], 20, None);
    organizer.organize(tablet).unwrap();

    assert!(organizer
        .state()
        .records
        .values()
        .all(|record| record.status == OrganizedMemoryStatus::Confirmed));
    let tablet_projection = organizer
        .project(OrganizedMemoryQuery {
            current_at_ms: 30,
            space_id: "personal".to_string(),
            environment: BTreeMap::from([("device".to_string(), "tablet".to_string())]),
            max_items: 8,
            ..OrganizedMemoryQuery::default()
        })
        .unwrap();
    assert_eq!(tablet_projection.items.len(), 1);
    assert_eq!(
        tablet_projection.items[0].memory_id,
        memory_id("tablet-mode")
    );

    let desktop_projection = organizer
        .project(OrganizedMemoryQuery {
            current_at_ms: 30,
            space_id: "personal".to_string(),
            environment: BTreeMap::from([("device".to_string(), "desktop".to_string())]),
            max_items: 8,
            ..OrganizedMemoryQuery::default()
        })
        .unwrap();
    assert_eq!(desktop_projection.items.len(), 1);
    assert_eq!(
        desktop_projection.items[0].memory_id,
        memory_id("global-mode")
    );
}

#[test]
fn partially_overlapping_environment_scopes_are_disputed() {
    let mut organizer = MemoryOrganizer::new();
    let mut device = candidate(
        "device-rule",
        OrganizedMemoryKind::Preference,
        json!("voice"),
        MemoryEvidenceOrigin::UserExplicit,
        10,
    );
    device.applicability = applicability(&[("device", "tablet")], 10, None);
    let mut network = candidate(
        "network-rule",
        OrganizedMemoryKind::Preference,
        json!("keyboard"),
        MemoryEvidenceOrigin::UserExplicit,
        20,
    );
    network.applicability = applicability(&[("network", "office")], 20, None);
    organizer.organize(device).unwrap();
    organizer.organize(network).unwrap();

    assert_eq!(
        organizer.record(&memory_id("device-rule")).unwrap().status,
        OrganizedMemoryStatus::Disputed
    );
    assert_eq!(
        organizer.record(&memory_id("network-rule")).unwrap().status,
        OrganizedMemoryStatus::Disputed
    );
    MemoryOrganizer::from_state(organizer.export_state()).unwrap();
}

#[test]
fn dispute_resolution_closes_every_active_alternative_and_is_idempotent() {
    let mut organizer = MemoryOrganizer::new();
    organizer
        .organize(candidate(
            "answer-a",
            OrganizedMemoryKind::ContextualFact,
            json!("A"),
            MemoryEvidenceOrigin::UserExplicit,
            10,
        ))
        .unwrap();
    organizer
        .organize(candidate(
            "answer-b",
            OrganizedMemoryKind::ContextualFact,
            json!("B"),
            MemoryEvidenceOrigin::UserExplicit,
            20,
        ))
        .unwrap();
    let request = ResolveMemoryDisputeRequest {
        operation_id: "resolve:answer".to_string(),
        winner_memory_id: memory_id("answer-b"),
        losing_memory_ids: vec![memory_id("answer-a")],
        reason: "user explicitly clarified the current value".to_string(),
        resolved_at_ms: 30,
        evidence: vec![evidence(
            "event:resolution",
            MemoryEvidenceOrigin::UserExplicit,
            29,
        )],
    };
    let receipt = organizer.resolve_dispute(request.clone()).unwrap();
    assert!(receipt.changed);
    assert_eq!(
        organizer.record(&memory_id("answer-b")).unwrap().status,
        OrganizedMemoryStatus::Confirmed
    );
    assert_eq!(
        organizer.record(&memory_id("answer-a")).unwrap().status,
        OrganizedMemoryStatus::Superseded
    );
    assert!(organizer
        .record(&memory_id("answer-b"))
        .unwrap()
        .supersedes_memory_ids
        .contains(&memory_id("answer-a")));
    assert!(organizer
        .record(&memory_id("answer-a"))
        .unwrap()
        .conflicts_with_memory_ids
        .is_empty());
    assert!(organizer
        .record(&memory_id("answer-b"))
        .unwrap()
        .conflicts_with_memory_ids
        .is_empty());
    let revision = organizer.state().revision;
    assert!(!organizer.resolve_dispute(request.clone()).unwrap().changed);
    assert_eq!(organizer.state().revision, revision);

    let mut changed_request = request;
    changed_request.reason = "different replay".to_string();
    assert!(organizer.resolve_dispute(changed_request).is_err());
    MemoryOrganizer::from_state(organizer.export_state()).unwrap();
}

#[test]
fn dispute_resolution_requires_a_confirmed_winner_and_monotonic_time() {
    let mut provisional = MemoryOrganizer::new();
    provisional
        .organize(candidate(
            "agent-claim-a",
            OrganizedMemoryKind::ContextualFact,
            json!("A"),
            MemoryEvidenceOrigin::AgentAction,
            10,
        ))
        .unwrap();
    provisional
        .organize(candidate(
            "agent-claim-b",
            OrganizedMemoryKind::ContextualFact,
            json!("B"),
            MemoryEvidenceOrigin::AgentAction,
            20,
        ))
        .unwrap();
    let before = provisional.export_state();
    assert!(provisional
        .resolve_dispute(ResolveMemoryDisputeRequest {
            operation_id: "resolve:provisional".to_string(),
            winner_memory_id: memory_id("agent-claim-b"),
            losing_memory_ids: vec![memory_id("agent-claim-a")],
            reason: "an executing agent asserted a winner".to_string(),
            resolved_at_ms: 30,
            evidence: vec![evidence(
                "event:agent-resolution",
                MemoryEvidenceOrigin::AgentAction,
                29,
            )],
        })
        .is_err());
    assert_eq!(provisional.export_state(), before);

    let mut confirmed = MemoryOrganizer::new();
    confirmed
        .organize(candidate(
            "timed-a",
            OrganizedMemoryKind::ContextualFact,
            json!("A"),
            MemoryEvidenceOrigin::UserExplicit,
            10,
        ))
        .unwrap();
    confirmed
        .organize(candidate(
            "timed-b",
            OrganizedMemoryKind::ContextualFact,
            json!("B"),
            MemoryEvidenceOrigin::UserExplicit,
            20,
        ))
        .unwrap();
    let before = confirmed.export_state();
    assert!(confirmed
        .resolve_dispute(ResolveMemoryDisputeRequest {
            operation_id: "resolve:backdated".to_string(),
            winner_memory_id: memory_id("timed-b"),
            losing_memory_ids: vec![memory_id("timed-a")],
            reason: "backdated correction".to_string(),
            resolved_at_ms: 15,
            evidence: vec![evidence(
                "event:backdated-resolution",
                MemoryEvidenceOrigin::UserExplicit,
                14,
            )],
        })
        .is_err());
    assert_eq!(confirmed.export_state(), before);
    assert_restorable(&confirmed);
}

#[test]
fn terminal_transitions_release_active_conflict_edges() {
    let mut organizer = MemoryOrganizer::new();
    organizer
        .organize(candidate(
            "edge-a",
            OrganizedMemoryKind::Preference,
            json!("A"),
            MemoryEvidenceOrigin::UserExplicit,
            10,
        ))
        .unwrap();
    organizer
        .organize(candidate(
            "edge-b",
            OrganizedMemoryKind::Preference,
            json!("B"),
            MemoryEvidenceOrigin::UserExplicit,
            20,
        ))
        .unwrap();
    let mut replacement = candidate(
        "edge-a-v2",
        OrganizedMemoryKind::Preference,
        json!("A2"),
        MemoryEvidenceOrigin::UserExplicit,
        30,
    );
    replacement.intent = MemoryCandidateIntent::Replace;
    replacement.target_memory_id = Some(memory_id("edge-a"));
    organizer.organize(replacement).unwrap();

    assert!(organizer
        .record(&memory_id("edge-a"))
        .unwrap()
        .conflicts_with_memory_ids
        .is_empty());
    assert_eq!(
        organizer
            .record(&memory_id("edge-b"))
            .unwrap()
            .conflicts_with_memory_ids,
        BTreeSet::from([memory_id("edge-a-v2")])
    );
    assert_restorable(&organizer);
}

#[test]
fn weak_model_counterclaim_does_not_lower_explicit_fact_confidence() {
    let mut organizer = MemoryOrganizer::new();
    organizer
        .organize(candidate(
            "explicit-fact",
            OrganizedMemoryKind::ContextualFact,
            json!("true"),
            MemoryEvidenceOrigin::UserExplicit,
            10,
        ))
        .unwrap();
    let confidence = organizer
        .record(&memory_id("explicit-fact"))
        .unwrap()
        .confidence;
    organizer
        .organize(candidate(
            "model-counterclaim",
            OrganizedMemoryKind::ContextualFact,
            json!("false"),
            MemoryEvidenceOrigin::ModelInference,
            20,
        ))
        .unwrap();

    let explicit = organizer.record(&memory_id("explicit-fact")).unwrap();
    assert_eq!(explicit.status, OrganizedMemoryStatus::Confirmed);
    assert_eq!(explicit.confidence, confidence);
    assert_eq!(
        organizer
            .record(&memory_id("model-counterclaim"))
            .unwrap()
            .status,
        OrganizedMemoryStatus::Provisional
    );
}

#[test]
fn self_reported_agent_action_cannot_confirm_an_experience() {
    let mut organizer = MemoryOrganizer::new();
    organizer
        .organize(candidate(
            "self-report",
            OrganizedMemoryKind::ActionExperience,
            json!({"outcome": "success"}),
            MemoryEvidenceOrigin::AgentAction,
            10,
        ))
        .unwrap();
    organizer
        .organize(candidate(
            "verified-outcome",
            OrganizedMemoryKind::ActionExperience,
            json!({"outcome": "success"}),
            MemoryEvidenceOrigin::VerifiedTaskOutcome,
            20,
        ))
        .unwrap();

    assert_eq!(
        organizer.record(&memory_id("self-report")).unwrap().status,
        OrganizedMemoryStatus::Provisional
    );
    assert_eq!(
        organizer
            .record(&memory_id("verified-outcome"))
            .unwrap()
            .status,
        OrganizedMemoryStatus::Confirmed
    );
}

#[test]
fn model_inference_cannot_retract_a_user_fact() {
    let mut organizer = MemoryOrganizer::new();
    organizer
        .organize(candidate(
            "user-fact",
            OrganizedMemoryKind::ContextualFact,
            json!("kept"),
            MemoryEvidenceOrigin::UserExplicit,
            10,
        ))
        .unwrap();
    let result = organizer.retract(RetractOrganizedMemoryRequest {
        operation_id: "model-retract".to_string(),
        memory_id: memory_id("user-fact"),
        reason: "model changed its mind".to_string(),
        retracted_at_ms: 20,
        evidence: vec![evidence(
            "event:model-retract",
            MemoryEvidenceOrigin::ModelInference,
            19,
        )],
    });
    assert!(result.is_err());
    assert_eq!(
        organizer.record(&memory_id("user-fact")).unwrap().status,
        OrganizedMemoryStatus::Confirmed
    );
}

#[test]
fn retraction_cannot_go_back_in_time_and_its_evidence_is_indexed() {
    let mut organizer = MemoryOrganizer::new();
    organizer
        .organize(candidate(
            "retraction-time",
            OrganizedMemoryKind::ContextualFact,
            json!("old"),
            MemoryEvidenceOrigin::UserExplicit,
            10,
        ))
        .unwrap();
    let before = organizer.export_state();
    assert!(organizer
        .retract(RetractOrganizedMemoryRequest {
            operation_id: "retract:too-early".to_string(),
            memory_id: memory_id("retraction-time"),
            reason: "time travel".to_string(),
            retracted_at_ms: 10,
            evidence: vec![evidence(
                "event:too-early-retraction",
                MemoryEvidenceOrigin::UserExplicit,
                9,
            )],
        })
        .is_err());
    assert_eq!(organizer.export_state(), before);

    let retraction_evidence = evidence(
        "event:indexed-retraction",
        MemoryEvidenceOrigin::UserExplicit,
        19,
    );
    organizer
        .retract(RetractOrganizedMemoryRequest {
            operation_id: "retract:valid".to_string(),
            memory_id: memory_id("retraction-time"),
            reason: "valid correction".to_string(),
            retracted_at_ms: 20,
            evidence: vec![retraction_evidence.clone()],
        })
        .unwrap();
    let before_reuse = organizer.export_state();
    let mut reuse = candidate(
        "reuse-retraction-evidence",
        OrganizedMemoryKind::ContextualFact,
        json!("new"),
        MemoryEvidenceOrigin::UserExplicit,
        30,
    );
    reuse.predicate = "other_fact".to_string();
    reuse.evidence = vec![retraction_evidence];
    reuse.evidence[0].reliability = 0.5;
    assert!(organizer.organize(reuse).is_err());
    assert_eq!(organizer.export_state(), before_reuse);
    assert_restorable(&organizer);
}

#[test]
fn non_overlapping_history_does_not_exhaust_the_hot_slot_limit() {
    let mut organizer = MemoryOrganizer::new();
    for index in 0..=MAX_MEMORY_RECORDS_PER_BASE_SLOT {
        let mut item = candidate(
            &format!("historical-{index}"),
            OrganizedMemoryKind::ContextualFact,
            json!(index),
            MemoryEvidenceOrigin::UserExplicit,
            index as i64 * 10 + 1,
        );
        item.applicability = applicability(&[], index as i64 * 10, Some(index as i64 * 10 + 5));
        organizer.organize(item).unwrap();
    }
    assert_eq!(
        organizer.state().records.len(),
        MAX_MEMORY_RECORDS_PER_BASE_SLOT + 1
    );
    assert_restorable(&organizer);
}

#[test]
fn batch_rejects_one_bad_candidate_without_blocking_valid_neighbors() {
    let mut organizer = MemoryOrganizer::new();
    let first = candidate(
        "batch-valid-1",
        OrganizedMemoryKind::ActionExperience,
        json!(1),
        MemoryEvidenceOrigin::VerifiedTaskOutcome,
        10,
    );
    let mut invalid = candidate(
        "batch-invalid",
        OrganizedMemoryKind::Preference,
        json!(2),
        MemoryEvidenceOrigin::UserExplicit,
        20,
    );
    invalid.subject.clear();
    let second = candidate(
        "batch-valid-2",
        OrganizedMemoryKind::ActionExperience,
        json!(3),
        MemoryEvidenceOrigin::VerifiedTaskOutcome,
        30,
    );

    let outcome = organizer
        .organize_batch(vec![first, invalid, second])
        .unwrap();
    assert_eq!(outcome.accepted.len(), 2);
    assert_eq!(outcome.rejected.len(), 1);
    assert_eq!(organizer.state().revision, 2);
    assert_eq!(organizer.state().records.len(), 2);
}

#[test]
fn evidence_event_metadata_is_globally_immutable() {
    let mut organizer = MemoryOrganizer::new();
    let first = candidate(
        "evidence-first",
        OrganizedMemoryKind::ContextualFact,
        json!("one"),
        MemoryEvidenceOrigin::UserExplicit,
        10,
    );
    let shared_event = first.evidence[0].clone();
    organizer.organize(first).unwrap();

    let mut second = candidate(
        "evidence-second",
        OrganizedMemoryKind::ContextualFact,
        json!("two"),
        MemoryEvidenceOrigin::UserExplicit,
        20,
    );
    second.evidence[0] = shared_event.clone();
    organizer.organize(second).unwrap();
    MemoryOrganizer::from_state(organizer.export_state()).unwrap();

    let mut inconsistent = candidate(
        "evidence-inconsistent",
        OrganizedMemoryKind::ContextualFact,
        json!("three"),
        MemoryEvidenceOrigin::UserExplicit,
        30,
    );
    inconsistent.evidence[0] = shared_event;
    inconsistent.evidence[0].reliability = 0.5;
    assert!(organizer.organize(inconsistent).is_err());
}

#[test]
fn three_way_conflict_is_order_independent() {
    fn build(order: [&str; 3]) -> BTreeMap<String, (OrganizedMemoryStatus, BTreeSet<String>)> {
        let mut organizer = MemoryOrganizer::new();
        for (index, id) in order.into_iter().enumerate() {
            organizer
                .organize(candidate(
                    id,
                    OrganizedMemoryKind::ContextualFact,
                    json!(id),
                    MemoryEvidenceOrigin::UserExplicit,
                    index as i64 * 10 + 10,
                ))
                .unwrap();
        }
        MemoryOrganizer::from_state(organizer.export_state()).unwrap();
        organizer
            .state()
            .records
            .iter()
            .map(|(id, record)| {
                (
                    id.clone(),
                    (record.status, record.conflicts_with_memory_ids.clone()),
                )
            })
            .collect()
    }

    let first = build(["claim-a", "claim-b", "claim-c"]);
    let second = build(["claim-c", "claim-a", "claim-b"]);
    assert_eq!(first, second);
    assert!(first.values().all(
        |(status, conflicts)| *status == OrganizedMemoryStatus::Disputed && conflicts.len() == 2
    ));
}

#[test]
fn malformed_confirmed_model_record_and_fake_credential_reference_fail_closed() {
    let mut organizer = MemoryOrganizer::new();
    organizer
        .organize(candidate(
            "model-only",
            OrganizedMemoryKind::ContextualFact,
            json!("guess"),
            MemoryEvidenceOrigin::ModelInference,
            10,
        ))
        .unwrap();
    let mut corrupted = organizer.export_state();
    corrupted
        .records
        .get_mut(&memory_id("model-only"))
        .unwrap()
        .status = OrganizedMemoryStatus::Confirmed;
    assert!(MemoryOrganizer::from_state(corrupted).is_err());

    let mut fake_reference = candidate(
        "fake-keyring",
        OrganizedMemoryKind::ContextualFact,
        json!({"credentialRef": "plaintext-token"}),
        MemoryEvidenceOrigin::UserExplicit,
        20,
    );
    fake_reference.subject = "account".to_string();
    fake_reference.predicate = "private_key".to_string();
    assert!(organizer.organize(fake_reference).is_err());

    let mut retracted = MemoryOrganizer::new();
    retracted
        .organize(candidate(
            "corrupt-retraction",
            OrganizedMemoryKind::ContextualFact,
            json!("wrong"),
            MemoryEvidenceOrigin::UserExplicit,
            30,
        ))
        .unwrap();
    retracted
        .retract(RetractOrganizedMemoryRequest {
            operation_id: "retract:corrupt".to_string(),
            memory_id: memory_id("corrupt-retraction"),
            reason: "user correction".to_string(),
            retracted_at_ms: 40,
            evidence: vec![evidence(
                "event:corrupt-retraction-operation",
                MemoryEvidenceOrigin::UserExplicit,
                39,
            )],
        })
        .unwrap();
    let mut corrupt_retraction = retracted.export_state();
    corrupt_retraction
        .records
        .get_mut(&memory_id("corrupt-retraction"))
        .unwrap()
        .retraction
        .as_mut()
        .unwrap()
        .reason = "  not canonical  ".to_string();
    assert!(MemoryOrganizer::from_state(corrupt_retraction).is_err());
}

#[test]
fn generated_memory_id_is_bounded_and_always_restorable() {
    let prefix_chars = "memory:organized:".chars().count();
    let mut organizer = MemoryOrganizer::new();
    let longest_valid_id = "x".repeat(512 - prefix_chars);
    let mut valid = candidate(
        &longest_valid_id,
        OrganizedMemoryKind::ContextualFact,
        json!("bounded"),
        MemoryEvidenceOrigin::UserExplicit,
        10,
    );
    valid.evidence = vec![evidence(
        "event:bounded-memory-id",
        MemoryEvidenceOrigin::UserExplicit,
        10,
    )];
    organizer.organize(valid).unwrap();
    assert_eq!(memory_id(&longest_valid_id).chars().count(), 512);
    assert_restorable(&organizer);

    let before = organizer.export_state();
    let too_long_id = "y".repeat(513 - prefix_chars);
    let mut too_long = candidate(
        &too_long_id,
        OrganizedMemoryKind::ContextualFact,
        json!("too long"),
        MemoryEvidenceOrigin::UserExplicit,
        20,
    );
    too_long.evidence = vec![evidence(
        "event:oversized-memory-id",
        MemoryEvidenceOrigin::UserExplicit,
        20,
    )];
    assert!(organizer.organize(too_long).is_err());
    assert_eq!(organizer.export_state(), before);
}

#[test]
fn historical_resolution_receipt_survives_later_memory_transitions() {
    let mut organizer = MemoryOrganizer::new();
    organizer
        .organize(candidate(
            "historical-resolution-a",
            OrganizedMemoryKind::ContextualFact,
            json!("A"),
            MemoryEvidenceOrigin::UserExplicit,
            10,
        ))
        .unwrap();
    organizer
        .organize(candidate(
            "historical-resolution-b",
            OrganizedMemoryKind::ContextualFact,
            json!("B"),
            MemoryEvidenceOrigin::UserExplicit,
            20,
        ))
        .unwrap();
    organizer
        .resolve_dispute(ResolveMemoryDisputeRequest {
            operation_id: "resolve:historical".to_string(),
            winner_memory_id: memory_id("historical-resolution-b"),
            losing_memory_ids: vec![memory_id("historical-resolution-a")],
            reason: "the user chose B".to_string(),
            resolved_at_ms: 30,
            evidence: vec![evidence(
                "event:historical-resolution",
                MemoryEvidenceOrigin::UserExplicit,
                29,
            )],
        })
        .unwrap();

    let mut replacement = candidate(
        "historical-resolution-c",
        OrganizedMemoryKind::ContextualFact,
        json!("C"),
        MemoryEvidenceOrigin::UserExplicit,
        40,
    );
    replacement.intent = MemoryCandidateIntent::Replace;
    replacement.target_memory_id = Some(memory_id("historical-resolution-b"));
    organizer.organize(replacement).unwrap();
    organizer
        .retract(RetractOrganizedMemoryRequest {
            operation_id: "retract:historical-loser".to_string(),
            memory_id: memory_id("historical-resolution-a"),
            reason: "remove the obsolete claim".to_string(),
            retracted_at_ms: 51,
            evidence: vec![evidence(
                "event:historical-loser-retraction",
                MemoryEvidenceOrigin::UserExplicit,
                49,
            )],
        })
        .unwrap();

    assert_restorable(&organizer);
}

#[test]
fn resolution_cannot_reuse_opposite_polarity_evidence_in_the_winner() {
    let mut organizer = MemoryOrganizer::new();
    organizer
        .organize(candidate(
            "polarity-a",
            OrganizedMemoryKind::ContextualFact,
            json!("A"),
            MemoryEvidenceOrigin::UserExplicit,
            10,
        ))
        .unwrap();
    let mut claim_b = candidate(
        "polarity-b",
        OrganizedMemoryKind::ContextualFact,
        json!("B"),
        MemoryEvidenceOrigin::UserExplicit,
        20,
    );
    let mut shared_event = evidence(
        "event:opposite-polarity",
        MemoryEvidenceOrigin::UserExplicit,
        21,
    );
    shared_event.polarity = MemoryEvidencePolarity::Contradicts;
    shared_event.reliability = 0.5;
    claim_b.evidence.push(shared_event.clone());
    organizer.organize(claim_b).unwrap();

    let before = organizer.export_state();
    shared_event.polarity = MemoryEvidencePolarity::Supports;
    assert!(organizer
        .resolve_dispute(ResolveMemoryDisputeRequest {
            operation_id: "resolve:opposite-polarity".to_string(),
            winner_memory_id: memory_id("polarity-b"),
            losing_memory_ids: vec![memory_id("polarity-a")],
            reason: "invalid evidence reuse".to_string(),
            resolved_at_ms: 30,
            evidence: vec![shared_event],
        })
        .is_err());
    assert_eq!(organizer.export_state(), before);
    assert_restorable(&organizer);
}

#[test]
fn indexed_projection_matches_full_scan_across_filters_and_state_restore() {
    let mut organizer = MemoryOrganizer::new();
    for index in 0..48 {
        let mut item = candidate(
            &format!("indexed-{index:02}"),
            if index % 2 == 0 {
                OrganizedMemoryKind::Preference
            } else {
                OrganizedMemoryKind::ContextualFact
            },
            json!({ "value": index }),
            if index % 7 == 0 {
                MemoryEvidenceOrigin::ModelInference
            } else {
                MemoryEvidenceOrigin::UserExplicit
            },
            index + 1,
        );
        item.subject = format!("subject:{}", index % 12);
        item.predicate = format!("predicate:{}", index % 5);
        item.applicability.space_id = if index % 3 == 0 {
            "work".to_string()
        } else {
            "personal".to_string()
        };
        if index % 4 == 0 {
            item.applicability
                .environment
                .insert("device".to_string(), "tablet".to_string());
        }
        if index % 5 == 0 {
            item.applicability.valid_until_ms = Some(80);
        }
        item.importance = (index % 10) as f32 / 10.0;
        organizer.organize(item).unwrap();
    }

    let queries = vec![
        OrganizedMemoryQuery {
            current_at_ms: 60,
            space_id: "personal".to_string(),
            environment: BTreeMap::from([("device".to_string(), "tablet".to_string())]),
            include_provisional: true,
            include_disputed: true,
            max_items: 7,
            ..OrganizedMemoryQuery::default()
        },
        OrganizedMemoryQuery {
            current_at_ms: 90,
            space_id: "personal".to_string(),
            subjects: vec!["subject:1".to_string(), "subject:5".to_string()],
            predicates: vec!["predicate:1".to_string(), "predicate:3".to_string()],
            focus_terms: vec!["value".to_string(), "17".to_string()],
            include_provisional: true,
            include_disputed: true,
            max_items: 3,
            ..OrganizedMemoryQuery::default()
        },
        OrganizedMemoryQuery {
            current_at_ms: 60,
            space_id: "work".to_string(),
            kinds: vec![OrganizedMemoryKind::ContextualFact],
            environment: BTreeMap::from([("device".to_string(), "tablet".to_string())]),
            include_disputed: true,
            max_items: 11,
            ..OrganizedMemoryQuery::default()
        },
    ];

    for query in &queries {
        assert_eq!(
            organizer.project(query.clone()).unwrap(),
            reference_project(&organizer, query.clone())
        );
    }

    let restored = MemoryOrganizer::from_state(organizer.export_state()).unwrap();
    assert_eq!(
        restored.retrieval_index.record_count(),
        restored.state.records.len()
    );
    for query in queries {
        assert_eq!(
            restored.project(query.clone()).unwrap(),
            reference_project(&restored, query)
        );
    }
}

#[test]
fn structured_filters_start_from_the_narrowest_index_posting() {
    let mut organizer = MemoryOrganizer::new();
    for index in 0..320 {
        let mut item = candidate(
            &format!("posting-{index:03}"),
            OrganizedMemoryKind::Preference,
            json!(index),
            MemoryEvidenceOrigin::UserExplicit,
            index + 1,
        );
        item.subject = format!("indexed-subject:{index}");
        item.predicate = format!("indexed-predicate:{}", index % 8);
        if index >= 256 {
            item.applicability.space_id = "work".to_string();
        }
        organizer.organize(item).unwrap();
    }

    let (_, stats) = organizer
        .project_with_stats(OrganizedMemoryQuery {
            current_at_ms: 1_000,
            space_id: "personal".to_string(),
            subjects: vec!["indexed-subject:137".to_string()],
            predicates: vec!["indexed-predicate:1".to_string()],
            max_items: 8,
            ..OrganizedMemoryQuery::default()
        })
        .unwrap();
    assert_eq!(stats.index_seed_posting_count, 1);
    assert!(stats.index_membership_check_count <= 3);
    assert_eq!(stats.indexed_candidate_count, 1);
    assert_eq!(stats.visible_candidate_count, 1);
    assert_eq!(stats.ranked_candidate_count, 1);
    assert_eq!(stats.retained_candidate_count, 1);

    let restored = MemoryOrganizer::from_state(organizer.export_state()).unwrap();
    let (_, restored_stats) = restored
        .project_with_stats(OrganizedMemoryQuery {
            current_at_ms: 1_000,
            space_id: "personal".to_string(),
            subjects: vec!["indexed-subject:137".to_string()],
            predicates: vec!["indexed-predicate:1".to_string()],
            max_items: 8,
            ..OrganizedMemoryQuery::default()
        })
        .unwrap();
    assert_eq!(restored_stats, stats);
}

#[test]
fn ten_thousand_record_projection_keeps_only_bounded_top_k() {
    let mut organizer = MemoryOrganizer::new();
    for index in 0..MAX_ORGANIZED_MEMORY_RECORDS {
        let mut item = candidate(
            &format!("scale-{index:05}"),
            OrganizedMemoryKind::ContextualFact,
            json!(index),
            MemoryEvidenceOrigin::UserExplicit,
            index as i64 + 1,
        );
        item.subject = format!("scale-subject:{index:05}");
        item.predicate = "scale-value".to_string();
        organizer.organize(item).unwrap();
    }

    let (projection, stats) = organizer
        .project_with_stats(OrganizedMemoryQuery {
            current_at_ms: 20_000,
            space_id: "personal".to_string(),
            max_items: 8,
            ..OrganizedMemoryQuery::default()
        })
        .unwrap();
    assert_eq!(organizer.retrieval_index.record_count(), 10_000);
    assert_eq!(stats.indexed_candidate_count, 10_000);
    assert_eq!(stats.visible_candidate_count, 10_000);
    assert_eq!(stats.ranked_candidate_count, 10_000);
    assert_eq!(stats.retained_candidate_count, 8);
    assert_eq!(projection.items.len(), 8);
    assert_eq!(projection.omitted_count, 9_992);
    assert_eq!(projection.items[0].memory_id, memory_id("scale-09999"));
}
