use codex_app_server_capture::protocol::{CaptureChannel, CaptureRecord, inspect_fixture};
use codex_app_server_capture::s2::{
    EventSizeDistribution, PerformanceEvidence, S2Evidence, ScenarioEvidence, TerminalState,
    validate,
};

#[test]
fn zero_cost_fixture_establishes_protocol_contract() {
    let fixture = include_str!("fixtures/zero-cost-handshake.jsonl");
    let facts = inspect_fixture(fixture).expect("fixture must be valid JSONL");

    assert!(facts.timestamps_are_monotonic);
    assert!(facts.has_initialize_request);
    assert!(facts.has_initialize_response_without_jsonrpc);
    assert!(facts.has_notification_interleaving);
    assert!(facts.has_unknown_notification_noise);
    assert!(facts.has_separate_stderr);

    let records = fixture
        .lines()
        .map(|line| serde_json::from_str::<CaptureRecord>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records[0].channel, CaptureChannel::ClientToServer);
    assert!(records.iter().all(|record| !record.line.contains('\n')));
}

fn successful_evidence() -> S2Evidence {
    let completed = |name: &str| ScenarioEvidence {
        name: name.to_owned(),
        terminal_state: TerminalState::Completed,
        turn_completed: true,
        first_delta_seen: true,
        r1_sufficient: true,
        approval_seen: name == "C",
        interrupt_response_seen: false,
    };

    S2Evidence {
        scenarios: vec![
            completed("A"),
            completed("B"),
            completed("C"),
            ScenarioEvidence {
                name: "D".to_owned(),
                terminal_state: TerminalState::Interrupted,
                turn_completed: false,
                first_delta_seen: true,
                r1_sufficient: false,
                approval_seen: false,
                interrupt_response_seen: true,
            },
        ],
        auth_errors: 0,
        quota_errors: 0,
        protocol_errors: 0,
        performance: Some(PerformanceEvidence {
            real_content: true,
            peak_events_per_second: 42.0,
            peak_megabytes_per_second: 1.25,
            event_sizes: EventSizeDistribution {
                samples: 20,
                min_bytes: 8,
                p50_bytes: 32,
                p95_bytes: 128,
                max_bytes: 256,
            },
            merge_window_ms: 50,
            merge_input_events: 20,
            merge_output_events: 7,
        }),
        candidate_percentiles: Some(serde_json::json!({"p95_ms": 12.0})),
    }
}

#[test]
fn s2_validation_passes_only_with_complete_f1_f2_f3_evidence() {
    let report = validate(successful_evidence());

    assert!(report.valid);
    assert!(report.f1.passed);
    assert!(report.f2.passed);
    assert!(report.f3.passed);
    assert_eq!(
        report.pass_percentiles,
        Some(serde_json::json!({"p95_ms": 12.0}))
    );
    assert!(report.baseline_update_allowed);
}

#[test]
fn s2_validation_is_fail_closed_and_suppresses_pass_outputs() {
    let mut evidence = successful_evidence();
    evidence
        .scenarios
        .iter_mut()
        .find(|item| item.name == "C")
        .unwrap()
        .approval_seen = false;
    evidence.performance.as_mut().unwrap().merge_window_ms = 49;

    let report = validate(evidence);

    assert!(!report.valid);
    assert!(!report.f2.passed);
    assert!(!report.f3.passed);
    assert!(report.pass_percentiles.is_none());
    assert!(!report.baseline_update_allowed);
    assert!(
        report
            .reasons
            .iter()
            .any(|reason| reason.contains("approval_seen"))
    );
    assert!(report.reasons.iter().any(|reason| reason.contains("50ms")));
}

#[test]
fn s2_rejects_each_error_counter_including_u64_max_without_overflow() {
    for (auth, quota, protocol) in [
        (1, 0, 0),
        (0, 1, 0),
        (0, 0, 1),
        (u64::MAX, 0, 0),
        (u64::MAX, 1, 0),
    ] {
        let mut evidence = successful_evidence();
        evidence.auth_errors = auth;
        evidence.quota_errors = quota;
        evidence.protocol_errors = protocol;

        let report = validate(evidence);

        assert!(
            !report.valid,
            "counters {auth}/{quota}/{protocol} must invalidate F1"
        );
        assert!(!report.f1.passed);
    }
}

#[test]
fn s2_requires_exactly_one_of_each_required_scenario() {
    let mut identical_duplicate = successful_evidence();
    identical_duplicate
        .scenarios
        .push(identical_duplicate.scenarios[0].clone());
    let identical_report = validate(identical_duplicate);
    assert!(!identical_report.valid);
    assert!(
        identical_report
            .reasons
            .iter()
            .any(|reason| reason.contains("exactly one scenario A"))
    );

    let mut duplicate = successful_evidence();
    let mut contradictory_a = duplicate.scenarios[0].clone();
    contradictory_a.terminal_state = TerminalState::Failed;
    contradictory_a.turn_completed = false;
    duplicate.scenarios.push(contradictory_a);
    let duplicate_report = validate(duplicate);
    assert!(!duplicate_report.valid);
    assert!(
        duplicate_report
            .reasons
            .iter()
            .any(|reason| reason.contains("exactly one scenario A"))
    );

    let mut missing = successful_evidence();
    missing.scenarios.retain(|scenario| scenario.name != "B");
    let missing_report = validate(missing);
    assert!(!missing_report.valid);
    assert!(
        missing_report
            .reasons
            .iter()
            .any(|reason| reason.contains("exactly one scenario B"))
    );
}

#[test]
fn s2_rejects_zero_or_inconsistent_event_size_distributions() {
    let invalid_sizes = [
        (0, 32, 128, 256),
        (8, 0, 128, 256),
        (8, 32, 0, 256),
        (8, 32, 128, 0),
        (8, 129, 128, 256),
    ];

    for (min_bytes, p50_bytes, p95_bytes, max_bytes) in invalid_sizes {
        let mut evidence = successful_evidence();
        evidence.performance.as_mut().unwrap().event_sizes = EventSizeDistribution {
            samples: 20,
            min_bytes,
            p50_bytes,
            p95_bytes,
            max_bytes,
        };

        let report = validate(evidence);

        assert!(!report.valid);
        assert!(!report.f3.passed);
    }
}
