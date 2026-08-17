//! Stable, local-only rules for evaluation findings.

use super::{
    deduplicate_findings, enforce_finding_safety, sort_findings, EvalFinding, FindingSeverity,
    FindingSource, RuleAnalysis,
};
use crate::features::assistant::eval::{EvalCase, EvalRecord, ToolExpectation};
use anyhow::Result;
use std::collections::{BTreeMap, HashMap};

const SMALL_SAMPLE_LIMITATION: &str =
    "Fewer than 10 cases: a single smoke run is insufficient for trend conclusions.";

pub(crate) fn analyze_rules(cases: &[EvalCase], records: &[Result<EvalRecord>]) -> RuleAnalysis {
    let successful_elapsed = records
        .iter()
        .enumerate()
        .filter_map(|(index, result)| {
            result
                .as_ref()
                .ok()
                .filter(|record| record.status.eq_ignore_ascii_case("completed"))
                .map(|record| (index, record.elapsed_ms))
        })
        .collect::<Vec<_>>();
    let cases_by_id = cases
        .iter()
        .map(|case| (case.case_id.as_str(), case))
        .collect::<HashMap<_, _>>();
    let mut findings = Vec::new();

    for (index, result) in records.iter().enumerate() {
        match result {
            Err(error) => {
                let case_id = cases.get(index).map(|case| case.case_id.clone());
                findings.push(finding(
                    "case_failed",
                    FindingSeverity::P0,
                    case_id,
                    "execution",
                    "Evaluation case failed",
                    format!(
                        "Execution category: {}.",
                        classify_error(&error.to_string())
                    ),
                    "The case produced no trustworthy result.",
                    "Resolve the runner or provider error and rerun the case.",
                ));
            }
            Ok(record) => {
                // Current evaluation suites are smoke-sized; keep the leave-one-out peers explicit
                // so the baseline cannot accidentally include the candidate being classified.
                let peer_elapsed = successful_elapsed
                    .iter()
                    .filter(|(peer_index, _)| *peer_index != index)
                    .map(|(_, elapsed)| *elapsed)
                    .collect::<Vec<_>>();
                analyze_record(
                    record,
                    cases_by_id.get(record.case_id.as_str()).copied(),
                    &peer_elapsed,
                    &mut findings,
                )
            }
        }
    }

    for finding in &mut findings {
        enforce_finding_safety(finding);
    }
    deduplicate_findings(&mut findings);
    sort_findings(&mut findings);
    RuleAnalysis {
        findings,
        limitations: if cases.len() < 10 {
            vec![SMALL_SAMPLE_LIMITATION.to_string()]
        } else {
            Vec::new()
        },
    }
}

fn analyze_record(
    record: &EvalRecord,
    case: Option<&EvalCase>,
    peer_elapsed: &[u64],
    findings: &mut Vec<EvalFinding>,
) {
    if !record.status.eq_ignore_ascii_case("completed") {
        let error_category = classify_error(record.error.as_deref().unwrap_or_default());
        findings.push(finding(
            "case_failed",
            FindingSeverity::P0,
            Some(record.case_id.clone()),
            "execution",
            "Evaluation case failed",
            format!(
                "Status `{}`; execution category: {error_category}.",
                safe_status(&record.status)
            ),
            "The case did not complete, so its result is unusable.",
            "Investigate the reported execution failure and rerun the case.",
        ));
    }

    for event in record
        .analysis
        .tool_events
        .iter()
        .filter(|event| event.failed)
    {
        let tool_label = safe_tool_label(&event.name);
        findings.push(finding(
            "tool_event_failed",
            FindingSeverity::P0,
            Some(record.case_id.clone()),
            "execution",
            "Tool execution failed",
            format!("Tool `{tool_label}` reported a failed event."),
            "A failed tool call can invalidate the assistant result.",
            "Inspect the tool failure and make the call path resilient.",
        ));
    }

    if let Some(case) = case {
        match case.tool_expectation {
            ToolExpectation::Forbidden if !record.analysis.tool_events.is_empty() => {
                findings.push(finding(
                    "unexpected_tool_use",
                    FindingSeverity::P1,
                    Some(record.case_id.clone()),
                    "tool_use",
                    "Forbidden tool was used",
                    format!(
                        "Observed {} tool event(s) although tools were forbidden.",
                        record.analysis.tool_events.len()
                    ),
                    "The assistant violated the case's tool-use constraint.",
                    "Prevent tool dispatch when a case forbids tools.",
                ));
            }
            ToolExpectation::Required if record.analysis.tool_events.is_empty() => {
                findings.push(finding(
                    "required_tool_missing",
                    FindingSeverity::P1,
                    Some(record.case_id.clone()),
                    "tool_use",
                    "Required tool was not used",
                    "No tool events were observed although tool use was required.".to_string(),
                    "The assistant may have answered without required external evidence or action.",
                    "Ensure the required tool is selected and executed.",
                ));
            }
            _ => {}
        }
    }

    if let Some(usage) = record.usage {
        if record.elapsed_ms >= 30_000 && usage.input_tokens >= 40_000 {
            findings.push(finding(
                "slow_high_token",
                FindingSeverity::P1,
                Some(record.case_id.clone()),
                "efficiency",
                "Slow high-token case",
                format!(
                    "Elapsed {} ms with {} input tokens.",
                    record.elapsed_ms, usage.input_tokens
                ),
                "High latency and context volume increase cost and user wait time.",
                "Reduce unnecessary context and profile the slow execution path.",
            ));
        }

        let cache_total = usage.cache_hit_tokens as u128 + usage.cache_miss_tokens as u128;
        if usage.input_tokens >= 40_000
            && is_low_cache_hit_ratio(usage.cache_hit_tokens, usage.cache_miss_tokens)
        {
            findings.push(finding(
                "low_cache_hit_ratio",
                FindingSeverity::P1,
                Some(record.case_id.clone()),
                "cache",
                "Low cache hit ratio on large input",
                format!(
                    "Cache hits were {} of {} hit-plus-miss tokens.",
                    usage.cache_hit_tokens, cache_total
                ),
                "Low reuse on a large prompt increases latency and token processing cost.",
                "Stabilize reusable prompt prefixes and verify provider cache configuration.",
            ));
        }
    }

    // Count by raw identity. Redaction is a presentation boundary and must not merge
    // distinct unknown tools into one synthetic repeated-call sequence.
    let mut tool_counts = BTreeMap::<&str, usize>::new();
    for event in &record.analysis.tool_events {
        *tool_counts.entry(event.name.as_str()).or_default() += 1;
    }
    for (raw_name, count) in tool_counts {
        if count >= 3 {
            let tool_label = safe_tool_label(raw_name);
            findings.push(finding(
                "repeated_tool_use",
                FindingSeverity::P2,
                Some(record.case_id.clone()),
                "efficiency",
                "Repeated tool use",
                format!("Tool `{tool_label}` was called {count} times."),
                "Repeated calls may indicate a loop or redundant work.",
                "Check whether results can be reused or the tool loop can terminate earlier.",
            ));
        }
    }

    if record.status.eq_ignore_ascii_case("completed")
        && record.elapsed_ms >= 10_000
        && latency_exceeds_twice_median(record.elapsed_ms, peer_elapsed)
    {
        findings.push(finding(
            "latency_outlier",
            FindingSeverity::P2,
            Some(record.case_id.clone()),
            "latency",
            "Latency outlier",
            format!(
                "Elapsed {} ms versus successful-case median {} ms.",
                record.elapsed_ms,
                display_median(peer_elapsed)
            ),
            "This case is materially slower than the successful batch baseline.",
            "Profile this case and compare its model and tool path with faster cases.",
        ));
    }
}

pub(crate) fn latency_exceeds_twice_median(elapsed: u64, peers: &[u64]) -> bool {
    let Some(twice_median) = twice_median(peers) else {
        return false;
    };
    (elapsed as u128) * 2 > twice_median * 2
}

fn twice_median(values: &[u64]) -> Option<u128> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        Some((sorted[middle] as u128) * 2)
    } else {
        Some(sorted[middle - 1] as u128 + sorted[middle] as u128)
    }
}

fn display_median(values: &[u64]) -> String {
    twice_median(values)
        .map(|twice| {
            if twice % 2 == 0 {
                (twice / 2).to_string()
            } else {
                format!("{}.5", twice / 2)
            }
        })
        .unwrap_or_else(|| "unavailable".to_string())
}

pub(crate) fn classify_error(error: &str) -> &'static str {
    let lower = error.trim().to_ascii_lowercase();
    let explicit_timeout = lower == "timeout"
        || lower.starts_with("timeout:")
        || lower.starts_with("timeout ")
        || lower == "timed out"
        || lower.starts_with("timed out:")
        || lower.starts_with("timed out ")
        || lower.starts_with("request timed out")
        || lower.ends_with(" timed out");
    let explicit_auth_status = has_http_status(&lower, "401") || has_http_status(&lower, "403");
    let explicit_rate_status = has_http_status(&lower, "429");
    if explicit_timeout {
        "request timed out"
    } else if lower.starts_with("auth ")
        || lower.starts_with("auth:")
        || lower.starts_with("authentication ")
        || lower.starts_with("authentication:")
        || lower.starts_with("unauthorized")
        || lower.starts_with("forbidden")
        || explicit_auth_status
    {
        "authentication or permission failed"
    } else if lower.starts_with("rate limit") || explicit_rate_status {
        "rate limited"
    } else if lower.starts_with("provider ") || lower.starts_with("provider:") {
        "provider failed"
    } else if lower.starts_with("runner ") || lower.starts_with("runner:") {
        "runner failed"
    } else {
        "error details redacted"
    }
}

fn safe_status(status: &str) -> &'static str {
    if status.eq_ignore_ascii_case("completed") {
        "completed"
    } else if status.eq_ignore_ascii_case("timeout") {
        "timeout"
    } else if status.eq_ignore_ascii_case("runner_error") {
        "runner_error"
    } else if status.eq_ignore_ascii_case("error") {
        "error"
    } else {
        "non-completed"
    }
}

pub(crate) fn canonical_tool_label(name: &str) -> Option<&str> {
    match name {
        "web_search"
        | "fetch_url"
        | "exec_shell"
        | "read_file"
        | "write_file"
        | "append_file"
        | "edit_file"
        | "mcp_pinvou3_present_artifact"
        | "kb_search"
        | "kb_open_source" => Some(name),
        _ => None,
    }
}

fn safe_tool_label(name: &str) -> &str {
    canonical_tool_label(name).unwrap_or("[redacted-tool]")
}

pub(crate) fn has_http_status(error: &str, code: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    ["http", "status"].iter().any(|marker| {
        lower.match_indices(marker).any(|(index, _)| {
            let has_left_boundary =
                index == 0 || !lower.as_bytes()[index - 1].is_ascii_alphanumeric();
            if !has_left_boundary {
                return false;
            }
            let suffix = &lower[index + marker.len()..];
            let has_token_separator = suffix.chars().next().is_some_and(|character| {
                character.is_ascii_whitespace() || matches!(character, ':' | '=')
            });
            if !has_token_separator {
                return false;
            }
            let candidate = suffix.trim_start_matches(|character: char| {
                character.is_ascii_whitespace() || matches!(character, ':' | '=')
            });
            candidate.starts_with(code)
                && candidate
                    .as_bytes()
                    .get(code.len())
                    .is_none_or(|next| !next.is_ascii_digit())
        })
    })
}

pub(crate) fn is_low_cache_hit_ratio(hit: u64, miss: u64) -> bool {
    let total = hit as u128 + miss as u128;
    total > 0 && (hit as u128) * 4 < total
}

fn finding(
    id: &str,
    severity: FindingSeverity,
    case_id: Option<String>,
    category: &str,
    title: &str,
    evidence: String,
    impact: &str,
    recommendation: &str,
) -> EvalFinding {
    EvalFinding {
        id: id.to_string(),
        source: FindingSource::Rule,
        severity,
        case_id,
        category: category.to_string(),
        title: title.to_string(),
        evidence,
        impact: impact.to_string(),
        recommendation: recommendation.to_string(),
        confidence: None,
    }
}
