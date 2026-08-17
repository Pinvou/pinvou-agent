//! Deterministic and judge-assisted analysis types for evaluation runs.

pub(crate) mod judge;
mod product_score;
pub(crate) mod rules;
#[cfg(test)]
pub(crate) use judge::validate_judge_identity;
pub(crate) use judge::{EvalModelSelection, EvalSuiteModelSnapshot, ModelIdentity};
#[cfg(test)]
pub(crate) use product_score::PRODUCT_SCORE_VERSION;
pub(crate) use product_score::{
    calculate_product_score, summarize_product_problems, ProductDiagnosis, ProductGrade,
    ProductProblemArea, ProductScore, ProductScoreConfidence, ProductScoreDimension,
};

pub(crate) fn score_product_run(
    records: &[anyhow::Result<super::EvalRecord>],
    rule_findings: &[EvalFinding],
    trusted_suite_case_ids: &[String],
) -> ProductScore {
    calculate_product_score(records, rule_findings, trusted_suite_case_ids)
}

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSource {
    Rule,
    Judge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    P0,
    P1,
    P2,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalFinding {
    pub id: String,
    pub source: FindingSource,
    pub severity: FindingSeverity,
    pub case_id: Option<String>,
    pub category: String,
    pub title: String,
    pub evidence: String,
    pub impact: String,
    pub recommendation: String,
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleAnalysis {
    pub findings: Vec<EvalFinding>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JudgeStatus {
    Completed,
    NotConfigured,
    SkippedSameModel { reason: String },
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JudgeDimensionScore {
    pub dimension: String,
    pub score: u8,
    pub confidence: f32,
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JudgeReport {
    pub status: JudgeStatus,
    pub dimensions: Vec<JudgeDimensionScore>,
    pub findings: Vec<EvalFinding>,
}

pub(super) fn severity_rank(severity: FindingSeverity) -> u8 {
    match severity {
        FindingSeverity::P0 => 0,
        FindingSeverity::P1 => 1,
        FindingSeverity::P2 => 2,
    }
}

const REQUIRED_JUDGE_DIMENSIONS: [&str; 6] = [
    "task_completion",
    "correctness",
    "tool_choice",
    "efficiency",
    "safety_boundaries",
    "overall_quality",
];

pub(crate) fn judge_report_is_usable(judge: &JudgeReport) -> bool {
    if !matches!(judge.status, JudgeStatus::Completed) || judge.dimensions.len() != 6 {
        return false;
    }
    let mut seen = HashSet::new();
    judge.dimensions.iter().all(|dimension| {
        REQUIRED_JUDGE_DIMENSIONS.contains(&dimension.dimension.as_str())
            && seen.insert(dimension.dimension.as_str())
            && dimension.score <= 100
            && dimension.confidence.is_finite()
            && (0.0..=1.0).contains(&dimension.confidence)
            && !dimension.evidence.trim().is_empty()
    })
}

pub(crate) fn contains_sensitive_identifier(value: &str) -> bool {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("glpat-")
        || lower.starts_with("xoxb-")
        || trimmed.starts_with("AKIA")
        || lower.starts_with("ghp_")
        || lower.starts_with("github_pat_")
        || lower.starts_with("sk-")
        || lower.starts_with("sk_")
        || lower.contains("bearer")
}

/// Sort findings by severity, case ID, then finding ID.
pub(crate) fn sort_findings(findings: &mut [EvalFinding]) {
    findings.sort_by(|left, right| {
        severity_rank(left.severity)
            .cmp(&severity_rank(right.severity))
            .then_with(|| left.case_id.cmp(&right.case_id))
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn truncate_diagnostic(value: &mut String) {
    const MAX_CHARS: usize = 300;
    if value.chars().count() <= MAX_CHARS {
        return;
    }
    *value = value.chars().take(MAX_CHARS - 1).collect::<String>();
    value.push('…');
}

pub(crate) fn enforce_finding_safety(finding: &mut EvalFinding) {
    truncate_diagnostic(&mut finding.title);
    truncate_diagnostic(&mut finding.evidence);
    truncate_diagnostic(&mut finding.impact);
    truncate_diagnostic(&mut finding.recommendation);
}

fn normalized_title(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn duplicate_key(finding: &EvalFinding) -> (Option<String>, String, String, String) {
    (
        finding.case_id.clone(),
        finding.category.clone(),
        normalized_title(&finding.title),
        finding.evidence.clone(),
    )
}

pub(crate) fn deduplicate_findings(findings: &mut Vec<EvalFinding>) {
    let mut seen = HashSet::new();
    findings.retain(|finding| seen.insert(duplicate_key(finding)));
}

/// Merge judge findings after rule findings without allowing judge output to replace a rule.
/// Findings are duplicates only when case, category, normalized title, and evidence all match.
pub(crate) fn merge_findings(
    rule_analysis: RuleAnalysis,
    mut judge_findings: Vec<EvalFinding>,
) -> Vec<EvalFinding> {
    let mut merged = rule_analysis.findings;
    for finding in &mut merged {
        enforce_finding_safety(finding);
    }
    deduplicate_findings(&mut merged);
    let mut seen = merged.iter().map(duplicate_key).collect::<HashSet<_>>();

    for finding in &mut judge_findings {
        enforce_finding_safety(finding);
    }
    for finding in judge_findings {
        let key = duplicate_key(&finding);
        if seen.insert(key) {
            merged.push(finding);
        }
    }
    sort_findings(&mut merged);
    merged
}
