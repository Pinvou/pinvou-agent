//! Deterministic, judge-independent health scoring for Product evaluations.

use super::{
    contains_sensitive_identifier, judge_report_is_usable, severity_rank, EvalFinding,
    FindingSeverity, FindingSource, JudgeReport,
};
use crate::features::assistant::eval::EvalRecord;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

pub(crate) const PRODUCT_SCORE_VERSION: &str = "pinvou-product-score/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProductGrade {
    Excellent,
    Good,
    Fair,
    HighRisk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProductScoreConfidence {
    Unavailable,
    LowSample,
    Standard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProductScoreDimension {
    TaskCompletion,
    ToolReliability,
    ConstraintAdherence,
    PerformanceEfficiency,
    RuntimeStability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProductScoreDimensions {
    pub task_completion: u8,
    pub tool_reliability: u8,
    pub constraint_adherence: u8,
    pub performance_efficiency: u8,
    pub runtime_stability: u8,
}

impl Default for ProductScoreDimensions {
    fn default() -> Self {
        Self {
            task_completion: 100,
            tool_reliability: 100,
            constraint_adherence: 100,
            performance_efficiency: 100,
            runtime_stability: 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProductScoreDeduction {
    pub finding_id: String,
    pub case_id: Option<String>,
    pub evidence: String,
    pub dimension: ProductScoreDimension,
    pub points: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProductScore {
    pub version: String,
    pub total: Option<u8>,
    pub grade: Option<ProductGrade>,
    pub dimensions: ProductScoreDimensions,
    pub deductions: Vec<ProductScoreDeduction>,
    pub confidence: ProductScoreConfidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProductProblemArea {
    TaskCompletion,
    Toolchain,
    Constraints,
    Performance,
    CacheStability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProductDiagnosis {
    pub area: ProductProblemArea,
    pub severity: FindingSeverity,
    pub source: FindingSource,
    pub affected_case_ids: Vec<String>,
    pub affected_case_count: usize,
    pub conclusion: String,
    pub evidence: String,
    pub action: String,
    pub acceptance: String,
}

#[derive(Clone, Copy)]
struct DiagnosisTemplate {
    area: ProductProblemArea,
    conclusion: &'static str,
    action: &'static str,
    acceptance: &'static str,
}

#[derive(Clone, Copy)]
struct FindingPolicy {
    diagnosis: DiagnosisTemplate,
    deduction: Option<(ProductScoreDimension, u8, &'static str)>,
}

fn finding_policy(id: &str) -> Option<FindingPolicy> {
    let (area, deduction) = match id {
        "case_failed" => (
            ProductProblemArea::TaskCompletion,
            Some((
                ProductScoreDimension::TaskCompletion,
                35,
                "Evaluation case did not complete.",
            )),
        ),
        "timeout" | "case_timeout" | "runner_error" | "case_error" => {
            (ProductProblemArea::TaskCompletion, None)
        }
        "tool_event_failed" => (
            ProductProblemArea::Toolchain,
            Some((
                ProductScoreDimension::ToolReliability,
                30,
                "A tool event failed.",
            )),
        ),
        "required_tool_missing" => (
            ProductProblemArea::Toolchain,
            Some((
                ProductScoreDimension::ToolReliability,
                25,
                "A required tool was not used.",
            )),
        ),
        "repeated_tool_use" => (
            ProductProblemArea::Toolchain,
            Some((
                ProductScoreDimension::ToolReliability,
                10,
                "A tool was used repeatedly.",
            )),
        ),
        "unexpected_tool_use" => (
            ProductProblemArea::Constraints,
            Some((
                ProductScoreDimension::ConstraintAdherence,
                25,
                "A tool was used despite the case constraint.",
            )),
        ),
        "forbidden_tool_use" => (ProductProblemArea::Constraints, None),
        "slow_high_token" => (
            ProductProblemArea::Performance,
            Some((
                ProductScoreDimension::PerformanceEfficiency,
                20,
                "A high-token case exceeded the latency threshold.",
            )),
        ),
        "latency_outlier" => (
            ProductProblemArea::Performance,
            Some((
                ProductScoreDimension::PerformanceEfficiency,
                12,
                "A case was a latency outlier.",
            )),
        ),
        "low_cache_hit_ratio" => (
            ProductProblemArea::CacheStability,
            Some((
                ProductScoreDimension::RuntimeStability,
                15,
                "A large-input case had a low cache hit ratio.",
            )),
        ),
        _ => return None,
    };
    let (conclusion, action, acceptance) = area_guidance(area);
    Some(FindingPolicy {
        diagnosis: DiagnosisTemplate {
            area,
            conclusion,
            action,
            acceptance,
        },
        deduction,
    })
}

fn area_guidance(area: ProductProblemArea) -> (&'static str, &'static str, &'static str) {
    match area {
        ProductProblemArea::TaskCompletion => (
            "任务完成链路存在阻断。",
            "优先修复超时与失败状态的根因，并补充失败路径回归测试。",
            "连续 3 次相同用例集评测中，任务完成率达到 100%。",
        ),
        ProductProblemArea::Toolchain => (
            "工具链调用可靠性不足。",
            "修复工具失败与漏调，收敛重复调用，并为必需工具增加确定性校验。",
            "连续 3 次相同用例集评测中，必需工具调用率达到 100%，工具失败率为 0%，且无重复调用告警。",
        ),
        ProductProblemArea::Constraints => (
            "工具使用约束未被稳定遵守。",
            "在调用前校验工具白名单与用例约束，并覆盖禁止调用的回归场景。",
            "连续 3 次相同用例集评测中，禁止工具调用 0 次。",
        ),
        ProductProblemArea::Performance => (
            "响应性能存在异常波动。",
            "定位高耗时阶段并减少高 token 路径上的非必要工作。",
            "连续 3 次相同用例集评测中，用例耗时不超过同组中位数的 2 倍。",
        ),
        ProductProblemArea::CacheStability => (
            "大输入场景的缓存复用不稳定。",
            "检查缓存键与提示词前缀稳定性，避免破坏可复用前缀。",
            "连续 3 次相同用例集评测中，大输入缓存命中率不低于 25%。",
        ),
    }
}

fn display_safe_case_id(case_id: &str) -> Option<&str> {
    let canonical = case_id.trim();
    if canonical.is_empty()
        || canonical.len() > 128
        || !canonical
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return None;
    }
    (!contains_sensitive_identifier(canonical)).then_some(canonical)
}

pub(crate) fn summarize_product_problems(
    rule_findings: &[EvalFinding],
    judge: &JudgeReport,
    trusted_suite_case_ids: &[String],
) -> Vec<ProductDiagnosis> {
    let allowed_case_ids = trusted_suite_case_ids
        .iter()
        .filter_map(|case_id| display_safe_case_id(case_id))
        .collect::<HashSet<_>>();
    let mut by_area = BTreeMap::<ProductProblemArea, Vec<(&EvalFinding, DiagnosisTemplate)>>::new();
    let judge_findings = judge_report_is_usable(judge)
        .then_some(judge.findings.as_slice())
        .unwrap_or_default();
    for finding in rule_findings
        .iter()
        .filter(|finding| finding.source == FindingSource::Rule)
        .chain(
            judge_findings
                .iter()
                .filter(|finding| finding.source == FindingSource::Judge),
        )
    {
        if let Some(template) = finding_policy(&finding.id).map(|policy| policy.diagnosis) {
            by_area
                .entry(template.area)
                .or_default()
                .push((finding, template));
        }
    }

    let mut diagnoses = by_area
        .into_iter()
        .map(|(area, candidates)| {
            let source = if candidates
                .iter()
                .any(|(finding, _)| finding.source == FindingSource::Rule)
            {
                FindingSource::Rule
            } else {
                FindingSource::Judge
            };
            let selected = candidates
                .iter()
                .filter(|(finding, _)| finding.source == source)
                .collect::<Vec<_>>();
            let severity = selected
                .iter()
                .map(|(finding, _)| finding.severity)
                .min_by_key(|severity| severity_rank(*severity))
                .expect("diagnosis area has at least one mapped finding");
            let mut affected_case_ids = selected
                .iter()
                .filter_map(|(finding, _)| finding.case_id.as_deref())
                .filter_map(display_safe_case_id)
                .filter(|case_id| allowed_case_ids.contains(case_id))
                .map(str::to_string)
                .collect::<Vec<_>>();
            affected_case_ids.sort();
            affected_case_ids.dedup();
            let affected_case_count = affected_case_ids.len();
            let template = selected[0].1;
            let inference_prefix = if source == FindingSource::Judge {
                "[AI 推断]"
            } else {
                ""
            };
            let source_label = if source == FindingSource::Rule {
                "规则命中"
            } else {
                "AI 推断命中"
            };
            ProductDiagnosis {
                area,
                severity,
                source,
                affected_case_ids,
                affected_case_count,
                conclusion: format!("{inference_prefix}{}", template.conclusion),
                evidence: format!(
                    "{source_label} {} 次，涉及 {affected_case_count} 个安全用例标识。",
                    selected.len()
                ),
                action: template.action.to_string(),
                acceptance: template.acceptance.to_string(),
            }
        })
        .collect::<Vec<_>>();
    diagnoses.sort_by(|left, right| {
        severity_rank(left.severity)
            .cmp(&severity_rank(right.severity))
            .then_with(|| {
                right
                    .affected_case_ids
                    .len()
                    .cmp(&left.affected_case_ids.len())
            })
            .then_with(|| left.area.cmp(&right.area))
    });
    diagnoses.truncate(5);
    diagnoses
}

#[derive(Default)]
struct DimensionDeductions {
    task_completion: u16,
    tool_reliability: u16,
    constraint_adherence: u16,
    performance_efficiency: u16,
    runtime_stability: u16,
}

impl DimensionDeductions {
    fn add(&mut self, dimension: ProductScoreDimension, points: u8) {
        let target = match dimension {
            ProductScoreDimension::TaskCompletion => &mut self.task_completion,
            ProductScoreDimension::ToolReliability => &mut self.tool_reliability,
            ProductScoreDimension::ConstraintAdherence => &mut self.constraint_adherence,
            ProductScoreDimension::PerformanceEfficiency => &mut self.performance_efficiency,
            ProductScoreDimension::RuntimeStability => &mut self.runtime_stability,
        };
        *target = target.saturating_add(u16::from(points));
    }

    fn scores(&self) -> ProductScoreDimensions {
        ProductScoreDimensions {
            task_completion: remaining_score(self.task_completion),
            tool_reliability: remaining_score(self.tool_reliability),
            constraint_adherence: remaining_score(self.constraint_adherence),
            performance_efficiency: remaining_score(self.performance_efficiency),
            runtime_stability: remaining_score(self.runtime_stability),
        }
    }
}

fn remaining_score(deduction: u16) -> u8 {
    100_u16.saturating_sub(deduction.min(100)) as u8
}

fn deduction_for(id: &str) -> Option<(ProductScoreDimension, u8, &'static str)> {
    finding_policy(id).and_then(|policy| policy.deduction)
}

fn weighted_total(dimensions: ProductScoreDimensions) -> u8 {
    let weighted = u16::from(dimensions.task_completion) * 35
        + u16::from(dimensions.tool_reliability) * 25
        + u16::from(dimensions.constraint_adherence) * 15
        + u16::from(dimensions.performance_efficiency) * 15
        + u16::from(dimensions.runtime_stability) * 10;
    ((weighted + 50) / 100).min(100) as u8
}

fn grade_from_total(total: u8) -> ProductGrade {
    match total {
        90..=100 => ProductGrade::Excellent,
        75..=89 => ProductGrade::Good,
        60..=74 => ProductGrade::Fair,
        _ => ProductGrade::HighRisk,
    }
}

pub(crate) fn calculate_product_score(
    records: &[Result<EvalRecord>],
    findings: &[EvalFinding],
    trusted_suite_case_ids: &[String],
) -> ProductScore {
    let confidence = match records.len() {
        0 => ProductScoreConfidence::Unavailable,
        1..=9 => ProductScoreConfidence::LowSample,
        _ => ProductScoreConfidence::Standard,
    };
    let mut dimension_deductions = DimensionDeductions::default();
    let mut deductions = Vec::new();
    let mut seen = HashSet::new();
    let allowed_case_ids = trusted_suite_case_ids
        .iter()
        .filter_map(|case_id| display_safe_case_id(case_id))
        .collect::<HashSet<_>>();

    let mut failed_record_case_ids = HashSet::new();
    let mut anonymous_record_failures = 0_usize;
    for record in records {
        match record {
            Ok(record) if record.status.trim().eq_ignore_ascii_case("completed") => {}
            Ok(record) => {
                if failed_record_case_ids.insert(record.case_id.as_str()) {
                    add_deduction(
                        &mut dimension_deductions,
                        &mut deductions,
                        "case_failed",
                        safe_trusted_case_id(&record.case_id, &allowed_case_ids),
                    );
                }
            }
            Err(_) => anonymous_record_failures = anonymous_record_failures.saturating_add(1),
        }
    }

    let mut rule_failure_cases = HashSet::new();
    let mut rule_failures = Vec::new();
    for finding in findings
        .iter()
        .filter(|finding| finding.source == FindingSource::Rule && finding.id == "case_failed")
    {
        let key = (
            finding.id.as_str(),
            finding.case_id.as_deref(),
            finding.evidence.as_str(),
        );
        if !seen.insert(key) {
            continue;
        }
        if let Some(case_id) = finding.case_id.as_deref() {
            if failed_record_case_ids.contains(case_id) || !rule_failure_cases.insert(case_id) {
                continue;
            }
        }
        rule_failures.push(
            finding
                .case_id
                .as_deref()
                .and_then(|case_id| safe_trusted_case_id(case_id, &allowed_case_ids)),
        );
    }

    let inferred_failure_count = rule_failures.len().max(anonymous_record_failures);
    for index in 0..inferred_failure_count {
        add_deduction(
            &mut dimension_deductions,
            &mut deductions,
            "case_failed",
            rule_failures.get(index).cloned().flatten(),
        );
    }

    for finding in findings
        .iter()
        .filter(|finding| finding.source == FindingSource::Rule && finding.id != "case_failed")
    {
        let key = (
            finding.id.as_str(),
            finding.case_id.as_deref(),
            finding.evidence.as_str(),
        );
        if !seen.insert(key) {
            continue;
        }
        let Some((dimension, points, evidence)) = deduction_for(&finding.id) else {
            continue;
        };
        dimension_deductions.add(dimension, points);
        deductions.push(ProductScoreDeduction {
            finding_id: finding.id.clone(),
            case_id: finding
                .case_id
                .as_deref()
                .and_then(|case_id| safe_trusted_case_id(case_id, &allowed_case_ids)),
            evidence: evidence.to_string(),
            dimension,
            points,
        });
    }

    let dimensions = dimension_deductions.scores();
    let total = (!records.is_empty()).then(|| weighted_total(dimensions));

    ProductScore {
        version: PRODUCT_SCORE_VERSION.to_string(),
        total,
        grade: total.map(grade_from_total),
        dimensions,
        deductions,
        confidence,
    }
}

fn safe_trusted_case_id(case_id: &str, allowed_case_ids: &HashSet<&str>) -> Option<String> {
    display_safe_case_id(case_id)
        .filter(|case_id| allowed_case_ids.contains(case_id))
        .map(str::to_string)
}

fn add_deduction(
    dimension_deductions: &mut DimensionDeductions,
    deductions: &mut Vec<ProductScoreDeduction>,
    finding_id: &str,
    case_id: Option<String>,
) {
    let (dimension, points, evidence) =
        deduction_for(finding_id).expect("known product score deduction");
    dimension_deductions.add(dimension, points);
    deductions.push(ProductScoreDeduction {
        finding_id: finding_id.to_string(),
        case_id,
        evidence: evidence.to_string(),
        dimension,
        points,
    });
}
