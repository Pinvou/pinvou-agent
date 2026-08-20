use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalState {
    Completed,
    Interrupted,
    Failed,
    Missing,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ScenarioEvidence {
    pub name: String,
    pub terminal_state: TerminalState,
    pub turn_completed: bool,
    pub first_delta_seen: bool,
    pub r1_sufficient: bool,
    pub approval_seen: bool,
    pub interrupt_response_seen: bool,
    #[serde(default)]
    pub transport_retry_count: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EventSizeDistribution {
    pub samples: u64,
    pub min_bytes: u64,
    pub p50_bytes: u64,
    pub p95_bytes: u64,
    pub max_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PerformanceEvidence {
    pub real_content: bool,
    pub peak_events_per_second: f64,
    pub peak_megabytes_per_second: f64,
    pub event_sizes: EventSizeDistribution,
    pub merge_window_ms: u64,
    pub merge_input_events: u64,
    pub merge_output_events: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct S2Evidence {
    pub scenarios: Vec<ScenarioEvidence>,
    pub auth_errors: u64,
    pub quota_errors: u64,
    pub protocol_errors: u64,
    pub performance: Option<PerformanceEvidence>,
    pub candidate_percentiles: Option<Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GateResult {
    pub passed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct S2Report {
    pub valid: bool,
    pub f1: GateResult,
    pub f2: GateResult,
    pub f3: GateResult,
    pub reasons: Vec<String>,
    pub pass_percentiles: Option<Value>,
    pub baseline_update_allowed: bool,
}

pub fn validate(evidence: S2Evidence) -> S2Report {
    let mut f1_reasons = Vec::new();
    let mut f2_reasons = Vec::new();
    let mut f3_reasons = Vec::new();

    for name in ["A", "B", "C", "D"] {
        let count = evidence
            .scenarios
            .iter()
            .filter(|item| item.name == name)
            .count();
        if count != 1 {
            f1_reasons.push(format!(
                "F1: evidence requires exactly one scenario {name}, found {count}"
            ));
        }
    }
    let scenario = |name: &str| {
        let mut matches = evidence.scenarios.iter().filter(|item| item.name == name);
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    };
    for name in ["A", "B", "C"] {
        match scenario(name) {
            Some(item)
                if item.turn_completed && item.terminal_state == TerminalState::Completed => {}
            _ => f1_reasons.push(format!(
                "F1: scenario {name} requires successful turn/completed"
            )),
        }
    }
    match scenario("D") {
        Some(item) if item.terminal_state == TerminalState::Interrupted => {}
        _ => f1_reasons.push("F1: scenario D requires interrupted terminal state".to_owned()),
    }
    if evidence.auth_errors != 0 || evidence.quota_errors != 0 || evidence.protocol_errors != 0 {
        f1_reasons.push("F1: auth/quota/protocol error count must be zero".to_owned());
    }

    for name in ["A", "B"] {
        match scenario(name) {
            Some(item) if item.r1_sufficient && item.first_delta_seen => {}
            _ => f2_reasons.push(format!(
                "F2: scenario {name} requires sufficient R1 content and first delta"
            )),
        }
    }
    if !scenario("C").is_some_and(|item| item.approval_seen) {
        f2_reasons.push("F2: scenario C requires approval_seen=true".to_owned());
    }
    if !scenario("D").is_some_and(|item| {
        item.interrupt_response_seen && item.terminal_state == TerminalState::Interrupted
    }) {
        f2_reasons.push(
            "F2: scenario D requires interrupt response and interrupted terminal state".to_owned(),
        );
    }

    match evidence.performance.as_ref() {
        None => f3_reasons.push("F3: performance evidence is missing".to_owned()),
        Some(performance) => {
            if !performance.real_content {
                f3_reasons.push("F3: measurements must use real content".to_owned());
            }
            if !(performance.peak_events_per_second.is_finite()
                && performance.peak_events_per_second > 0.0
                && performance.peak_megabytes_per_second.is_finite()
                && performance.peak_megabytes_per_second > 0.0)
            {
                f3_reasons
                    .push("F3: positive finite peak events/s and MB/s are required".to_owned());
            }
            let sizes = &performance.event_sizes;
            if sizes.samples == 0
                || sizes.min_bytes == 0
                || !(sizes.min_bytes <= sizes.p50_bytes
                    && sizes.p50_bytes <= sizes.p95_bytes
                    && sizes.p95_bytes <= sizes.max_bytes)
            {
                f3_reasons.push("F3: valid event-size distribution is required".to_owned());
            }
            if performance.merge_window_ms != 50
                || performance.merge_input_events == 0
                || performance.merge_output_events == 0
                || performance.merge_output_events > performance.merge_input_events
            {
                f3_reasons.push("F3: valid 50ms merge-rate inputs are required".to_owned());
            }
        }
    }

    let f1 = GateResult {
        passed: f1_reasons.is_empty(),
    };
    let f2 = GateResult {
        passed: f2_reasons.is_empty(),
    };
    let f3 = GateResult {
        passed: f3_reasons.is_empty(),
    };
    let valid = f1.passed && f2.passed && f3.passed;
    let mut reasons = f1_reasons;
    reasons.extend(f2_reasons);
    reasons.extend(f3_reasons);

    S2Report {
        valid,
        f1,
        f2,
        f3,
        reasons,
        pass_percentiles: valid.then_some(evidence.candidate_percentiles).flatten(),
        baseline_update_allowed: valid,
    }
}
