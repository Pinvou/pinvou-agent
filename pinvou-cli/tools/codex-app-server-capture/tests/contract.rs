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
