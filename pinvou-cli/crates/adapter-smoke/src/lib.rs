use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use agent_backend_api::{
    AgentBackendError, PrivateInputHandle, PrivateInputResolver, ResolvedPrivateInput, SecretText,
};
use benchmark_core::{
    BenchmarkAdapter, BenchmarkDescriptor, BenchmarkError, BenchmarkId, BenchmarkPlan,
    BenchmarkTask, CompletedRun, ExecutionKind, ExecutionRequest, OfficialScoreReport,
    OutputContract, PreparedTask, Result as BenchmarkResult, RunContext, Split, SubmissionArtifact,
    TaskOutcome, TaskSelection, TaskStatus, ToolPolicyId, VerifiedDataset,
};
use serde::{Deserialize, Serialize};

pub const PRODUCT_SCORE_VERSION: &str = "pinvou-product-score/v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolExpectation {
    Forbidden,
    Optional,
    Required,
}

#[derive(Clone, Debug)]
pub struct SmokeCase {
    id: &'static str,
    prompt: &'static str,
    timeout: Duration,
    tool_expectation: ToolExpectation,
}

impl SmokeCase {
    pub fn id(&self) -> &str {
        self.id
    }
    pub fn prompt(&self) -> &str {
        self.prompt
    }
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
    pub fn tool_expectation(&self) -> ToolExpectation {
        self.tool_expectation
    }

    pub fn to_benchmark_task(&self) -> BenchmarkTask {
        BenchmarkTask::new(
            self.id,
            Some("smoke".to_string()),
            None,
            ExecutionRequest::native_turn(
                PrivateInputHandle::new(format!("smoke:{}", self.id)),
                vec![],
                self.timeout,
                ToolPolicyId::new("pinvou-product/v1"),
                OutputContract::new("smoke-private-output/v1"),
            ),
            None,
        )
    }
}

pub fn smoke_cases() -> Vec<SmokeCase> {
    vec![
        SmokeCase {
            id: "plep_smoke_hi",
            prompt: "hi",
            timeout: Duration::from_secs(30),
            tool_expectation: ToolExpectation::Forbidden,
        },
        SmokeCase {
            id: "plep_smoke_weather",
            prompt: "广州今天天气怎么样",
            timeout: Duration::from_secs(60),
            tool_expectation: ToolExpectation::Required,
        },
        SmokeCase {
            id: "plep_smoke_math",
            prompt: "1+1等于几",
            timeout: Duration::from_secs(30),
            tool_expectation: ToolExpectation::Forbidden,
        },
        SmokeCase {
            id: "plep_smoke_poem",
            prompt: "帮我写一首关于春天的诗",
            timeout: Duration::from_secs(60),
            tool_expectation: ToolExpectation::Forbidden,
        },
        SmokeCase {
            id: "plep_smoke_date",
            prompt: "今天星期几",
            timeout: Duration::from_secs(30),
            tool_expectation: ToolExpectation::Optional,
        },
    ]
}

#[derive(Default)]
pub struct SmokePrivateInputs;

impl SmokePrivateInputs {
    pub fn new() -> Self {
        Self
    }

    pub fn resolve_handle(
        &self,
        handle: &PrivateInputHandle,
    ) -> Result<ResolvedPrivateInput, AgentBackendError> {
        let id = handle
            .expose_to_backend()
            .strip_prefix("smoke:")
            .ok_or_else(|| AgentBackendError::Operation("unknown Smoke private input".into()))?;
        let case = smoke_cases()
            .into_iter()
            .find(|case| case.id() == id)
            .ok_or_else(|| AgentBackendError::Operation("unknown Smoke private input".into()))?;
        Ok(ResolvedPrivateInput::new(
            SecretText::new(case.prompt()),
            vec![],
        ))
    }
}

impl fmt::Debug for SmokePrivateInputs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SmokePrivateInputs([redacted])")
    }
}

impl PrivateInputResolver for SmokePrivateInputs {
    fn resolve<'life0, 'life1, 'async_trait>(
        &'life0 self,
        handle: &'life1 PrivateInputHandle,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<ResolvedPrivateInput, AgentBackendError>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { self.resolve_handle(handle) })
    }
}

pub struct SmokeAdapter {
    descriptor: BenchmarkDescriptor,
}

impl SmokeAdapter {
    pub fn new() -> Self {
        Self {
            descriptor: BenchmarkDescriptor::new(
                BenchmarkId::new("smoke"),
                "smoke-adapter/v1",
                "embedded-plep-smoke/v1",
                PRODUCT_SCORE_VERSION,
                vec![Split::new("smoke")],
                ExecutionKind::NativeTurn,
            ),
        }
    }
}

impl Default for SmokeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl BenchmarkAdapter for SmokeAdapter {
    fn descriptor(&self) -> &BenchmarkDescriptor {
        &self.descriptor
    }

    fn verify_dataset(&self, dataset_root: &std::path::Path) -> BenchmarkResult<VerifiedDataset> {
        Ok(VerifiedDataset::new("embedded-plep-smoke/v1", dataset_root))
    }

    fn plan(
        &self,
        _dataset: &VerifiedDataset,
        selection: &TaskSelection,
    ) -> BenchmarkResult<BenchmarkPlan> {
        let selected = selection
            .task_ids()
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let tasks = smoke_cases()
            .into_iter()
            .filter(|case| selected.is_empty() || selected.contains(case.id()))
            .map(|case| case.to_benchmark_task())
            .collect();
        Ok(BenchmarkPlan::new(tasks))
    }

    fn prepare_task(
        &self,
        task: &BenchmarkTask,
        _run: &RunContext,
    ) -> BenchmarkResult<PreparedTask> {
        Ok(PreparedTask::new(task.clone()))
    }

    fn score(&self, _run: &CompletedRun) -> BenchmarkResult<OfficialScoreReport> {
        Err(BenchmarkError::Contract(
            "Smoke Health is an internal diagnostic and is not an official benchmark score".into(),
        ))
    }

    fn write_submission(
        &self,
        _run: &CompletedRun,
        _destination: &std::path::Path,
    ) -> BenchmarkResult<SubmissionArtifact> {
        Err(BenchmarkError::Contract(
            "Smoke does not produce an official submission".into(),
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmokeToolEvent {
    name: String,
    failed: bool,
}

impl SmokeToolEvent {
    pub fn new(name: impl Into<String>, failed: bool) -> Self {
        Self {
            name: name.into(),
            failed,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SmokeUsage {
    input_tokens: u64,
    cache_hit_tokens: u64,
    cache_miss_tokens: u64,
}

impl SmokeUsage {
    pub fn new(input_tokens: u64, cache_hit_tokens: u64, cache_miss_tokens: u64) -> Self {
        Self {
            input_tokens,
            cache_hit_tokens,
            cache_miss_tokens,
        }
    }
}

#[derive(Clone, Default)]
pub struct SmokeAnalysisMaterial {
    tool_names: Vec<String>,
    tool_events: Vec<SmokeToolEvent>,
    usage: Option<SmokeUsage>,
}

impl SmokeAnalysisMaterial {
    pub fn new(tool_names: Vec<String>) -> Self {
        let tool_events = tool_names
            .iter()
            .cloned()
            .map(|name| SmokeToolEvent::new(name, false))
            .collect();
        Self {
            tool_names,
            tool_events,
            usage: None,
        }
    }

    pub fn with_details(tool_events: Vec<SmokeToolEvent>, usage: Option<SmokeUsage>) -> Self {
        let tool_names = tool_events.iter().map(|event| event.name.clone()).collect();
        Self {
            tool_names,
            tool_events,
            usage,
        }
    }

    pub fn tool_names(&self) -> &[String] {
        &self.tool_names
    }

    pub fn tool_events(&self) -> &[SmokeToolEvent] {
        &self.tool_events
    }

    pub fn usage(&self) -> Option<SmokeUsage> {
        self.usage
    }
}

impl fmt::Debug for SmokeAnalysisMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SmokeAnalysisMaterial")
            .field("tool_count", &self.tool_events.len())
            .field("has_usage", &self.usage.is_some())
            .finish()
    }
}

#[derive(Debug)]
pub struct SmokeRecord {
    outcome: TaskOutcome,
    analysis: SmokeAnalysisMaterial,
}

impl SmokeRecord {
    pub fn new(outcome: TaskOutcome, analysis: SmokeAnalysisMaterial) -> Self {
        Self { outcome, analysis }
    }
    pub fn outcome(&self) -> &TaskOutcome {
        &self.outcome
    }
    pub fn analysis(&self) -> &SmokeAnalysisMaterial {
        &self.analysis
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    P0,
    P1,
    P2,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSource {
    #[default]
    Rule,
    Judge,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmokeFinding {
    id: String,
    severity: FindingSeverity,
    case_id: Option<String>,
    title: String,
    recommendation: String,
    #[serde(default)]
    source: FindingSource,
}

impl SmokeFinding {
    fn new(
        id: &str,
        severity: FindingSeverity,
        case_id: Option<&str>,
        title: &str,
        recommendation: &str,
    ) -> Self {
        Self {
            id: id.into(),
            severity,
            case_id: case_id.map(str::to_string),
            title: title.into(),
            recommendation: recommendation.into(),
            source: FindingSource::Rule,
        }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn severity(&self) -> FindingSeverity {
        self.severity
    }
    pub fn case_id(&self) -> Option<&str> {
        self.case_id.as_deref()
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn recommendation(&self) -> &str {
        &self.recommendation
    }
    pub fn source(&self) -> FindingSource {
        self.source
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleAnalysis {
    findings: Vec<SmokeFinding>,
    limitations: Vec<String>,
}

impl RuleAnalysis {
    pub fn findings(&self) -> &[SmokeFinding] {
        &self.findings
    }
    pub fn limitations(&self) -> &[String] {
        &self.limitations
    }
}

pub fn analyze_rules(cases: &[SmokeCase], records: &[SmokeRecord]) -> RuleAnalysis {
    let by_id = cases
        .iter()
        .map(|case| (case.id(), case))
        .collect::<HashMap<_, _>>();
    let mut findings = Vec::new();
    let successful_elapsed = records
        .iter()
        .enumerate()
        .filter(|(_, record)| record.outcome().status() == TaskStatus::Completed)
        .map(|(index, record)| (index, record.outcome().elapsed_ms()))
        .collect::<Vec<_>>();
    for (index, record) in records.iter().enumerate() {
        let outcome = record.outcome();
        if outcome.status() != TaskStatus::Completed {
            findings.push(SmokeFinding::new(
                "case_failed",
                FindingSeverity::P0,
                Some(outcome.task_id()),
                "Case 未完成",
                "检查模型、网络与产品运行时状态",
            ));
        }
        for _event in record
            .analysis()
            .tool_events()
            .iter()
            .filter(|event| event.failed)
        {
            findings.push(SmokeFinding::new(
                "tool_event_failed",
                FindingSeverity::P0,
                Some(outcome.task_id()),
                "工具执行失败",
                "检查工具失败原因并增强调用链路韧性",
            ));
        }
        if let Some(case) = by_id.get(outcome.task_id()) {
            match case.tool_expectation() {
                ToolExpectation::Forbidden if !record.analysis().tool_events().is_empty() => {
                    findings.push(SmokeFinding::new(
                        "unexpected_tool_use",
                        FindingSeverity::P1,
                        Some(outcome.task_id()),
                        "不需要工具的任务调用了工具",
                        "收紧简单任务的工具选择",
                    ))
                }
                ToolExpectation::Required if record.analysis().tool_events().is_empty() => findings
                    .push(SmokeFinding::new(
                        "required_tool_missing",
                        FindingSeverity::P1,
                        Some(outcome.task_id()),
                        "需要工具的任务未调用工具",
                        "检查工具可用性与路由策略",
                    )),
                _ => {}
            }
        }
        if let Some(usage) = record.analysis().usage() {
            if outcome.elapsed_ms() >= 30_000 && usage.input_tokens >= 40_000 {
                findings.push(SmokeFinding::new(
                    "slow_high_token",
                    FindingSeverity::P1,
                    Some(outcome.task_id()),
                    "高 Token 任务耗时过长",
                    "减少无效上下文并分析慢路径",
                ));
            }
            if usage.input_tokens >= 40_000
                && is_low_cache_hit_ratio(usage.cache_hit_tokens, usage.cache_miss_tokens)
            {
                findings.push(SmokeFinding::new(
                    "low_cache_hit_ratio",
                    FindingSeverity::P1,
                    Some(outcome.task_id()),
                    "大输入任务缓存命中率低",
                    "稳定可复用提示前缀并检查缓存配置",
                ));
            }
        }
        let mut tool_counts = HashMap::<&str, usize>::new();
        for event in record.analysis().tool_events() {
            *tool_counts.entry(event.name.as_str()).or_default() += 1;
        }
        for count in tool_counts.into_values() {
            if count >= 3 {
                findings.push(SmokeFinding::new(
                    "repeated_tool_use",
                    FindingSeverity::P2,
                    Some(outcome.task_id()),
                    "工具重复调用",
                    "复用已有结果并确保工具循环及时终止",
                ));
            }
        }
        let peers = successful_elapsed
            .iter()
            .filter(|(peer_index, _)| *peer_index != index)
            .map(|(_, elapsed)| *elapsed)
            .collect::<Vec<_>>();
        if outcome.status() == TaskStatus::Completed
            && outcome.elapsed_ms() >= 10_000
            && latency_exceeds_twice_median(outcome.elapsed_ms(), &peers)
        {
            findings.push(SmokeFinding::new(
                "latency_outlier",
                FindingSeverity::P2,
                Some(outcome.task_id()),
                "任务延迟显著偏高",
                "对比快速任务的模型与工具路径并优化慢路径",
            ));
        }
    }
    let mut seen = HashSet::new();
    findings.retain(|finding| seen.insert((finding.case_id.clone(), finding.id.clone())));
    findings.sort_by_key(|finding| {
        (
            severity_rank(finding.severity),
            finding.case_id.clone(),
            finding.id.clone(),
        )
    });
    RuleAnalysis {
        findings,
        limitations: vec!["Smoke 样本量较小，只用于产品健康检查。".into()],
    }
}

pub fn latency_exceeds_twice_median(elapsed: u64, peers: &[u64]) -> bool {
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

pub fn is_low_cache_hit_ratio(hit: u64, miss: u64) -> bool {
    let total = hit as u128 + miss as u128;
    total > 0 && (hit as u128) * 4 < total
}

fn severity_rank(severity: FindingSeverity) -> u8 {
    match severity {
        FindingSeverity::P0 => 0,
        FindingSeverity::P1 => 1,
        FindingSeverity::P2 => 2,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductGrade {
    Excellent,
    Good,
    Fair,
    HighRisk,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductScoreConfidence {
    Unavailable,
    LowSample,
    Standard,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductScoreDimension {
    TaskCompletion,
    ToolReliability,
    ConstraintAdherence,
    PerformanceEfficiency,
    RuntimeStability,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductScoreDimensions {
    task_completion: u8,
    tool_reliability: u8,
    constraint_adherence: u8,
    performance_efficiency: u8,
    runtime_stability: u8,
}

impl ProductScoreDimensions {
    pub fn task_completion(&self) -> u8 {
        self.task_completion
    }
    pub fn tool_reliability(&self) -> u8 {
        self.tool_reliability
    }
    pub fn constraint_adherence(&self) -> u8 {
        self.constraint_adherence
    }
    pub fn performance_efficiency(&self) -> u8 {
        self.performance_efficiency
    }
    pub fn runtime_stability(&self) -> u8 {
        self.runtime_stability
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductScoreDeduction {
    finding_id: String,
    case_id: Option<String>,
    evidence: String,
    dimension: ProductScoreDimension,
    points: u8,
}

impl ProductScoreDeduction {
    pub fn finding_id(&self) -> &str {
        &self.finding_id
    }
    pub fn case_id(&self) -> Option<&str> {
        self.case_id.as_deref()
    }
    pub fn evidence(&self) -> &str {
        &self.evidence
    }
    pub fn dimension(&self) -> ProductScoreDimension {
        self.dimension
    }
    pub fn points(&self) -> u8 {
        self.points
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductProblemArea {
    TaskCompletion,
    Toolchain,
    Constraints,
    Performance,
    CacheStability,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductDiagnosis {
    area: ProductProblemArea,
    severity: FindingSeverity,
    source: FindingSource,
    affected_case_ids: Vec<String>,
    affected_case_count: usize,
    conclusion: String,
    evidence: String,
    action: String,
    acceptance: String,
}

impl ProductDiagnosis {
    pub fn area(&self) -> ProductProblemArea {
        self.area
    }
    pub fn severity(&self) -> FindingSeverity {
        self.severity
    }
    pub fn source(&self) -> FindingSource {
        self.source
    }
    pub fn affected_case_ids(&self) -> &[String] {
        &self.affected_case_ids
    }
    pub fn affected_case_count(&self) -> usize {
        self.affected_case_count
    }
    pub fn conclusion(&self) -> &str {
        &self.conclusion
    }
    pub fn evidence(&self) -> &str {
        &self.evidence
    }
    pub fn action(&self) -> &str {
        &self.action
    }
    pub fn acceptance(&self) -> &str {
        &self.acceptance
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmokeSafetyError(&'static str);

impl fmt::Display for SmokeSafetyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for SmokeSafetyError {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductScore {
    version: String,
    total: Option<u8>,
    grade: Option<ProductGrade>,
    dimensions: ProductScoreDimensions,
    deductions: Vec<ProductScoreDeduction>,
    confidence: ProductScoreConfidence,
    diagnoses: Vec<ProductDiagnosis>,
}

impl ProductScore {
    pub fn version(&self) -> &str {
        &self.version
    }
    pub fn total(&self) -> Option<u8> {
        self.total
    }
    pub fn grade(&self) -> Option<ProductGrade> {
        self.grade
    }
    pub fn dimensions(&self) -> &ProductScoreDimensions {
        &self.dimensions
    }
    pub fn deductions(&self) -> &[ProductScoreDeduction] {
        &self.deductions
    }
    pub fn confidence(&self) -> ProductScoreConfidence {
        self.confidence
    }
    pub fn diagnoses(&self) -> &[ProductDiagnosis] {
        &self.diagnoses
    }
    pub fn is_official_score(&self) -> bool {
        false
    }
}

pub fn calculate_product_score(
    records: &[SmokeRecord],
    findings: &[SmokeFinding],
) -> Result<ProductScore, SmokeSafetyError> {
    let trusted = records
        .iter()
        .map(|record| record.outcome().task_id())
        .collect::<Vec<_>>();
    calculate_product_score_with_trusted_cases(records, findings, &trusted)
}

pub fn calculate_product_score_with_trusted_cases(
    records: &[SmokeRecord],
    findings: &[SmokeFinding],
    trusted_suite_case_ids: &[&str],
) -> Result<ProductScore, SmokeSafetyError> {
    validate_findings(findings)?;
    if records
        .iter()
        .any(|record| !display_safe_case_id(record.outcome().task_id()))
    {
        return Err(SmokeSafetyError("smoke record identifiers are unsafe"));
    }
    if records.is_empty() {
        return Ok(ProductScore {
            version: PRODUCT_SCORE_VERSION.into(),
            total: None,
            grade: None,
            dimensions: score_dimensions([0; 5]),
            deductions: vec![],
            confidence: ProductScoreConfidence::Unavailable,
            diagnoses: vec![],
        });
    }
    let confidence = if records.len() < 10 {
        ProductScoreConfidence::LowSample
    } else {
        ProductScoreConfidence::Standard
    };
    let mut seen = HashSet::new();
    let mut deductions = Vec::new();
    for record in records
        .iter()
        .filter(|record| record.outcome().status() != TaskStatus::Completed)
    {
        let case_id = record.outcome().task_id();
        if seen.insert((Some(case_id.to_owned()), "case_failed".to_owned())) {
            deductions.push(score_deduction("case_failed", Some(case_id)));
        }
    }
    for finding in findings {
        if !seen.insert((finding.case_id.clone(), finding.id.clone())) {
            continue;
        }
        if score_policy(finding.id()).is_some() {
            deductions.push(score_deduction(finding.id(), finding.case_id()));
        }
    }
    let mut dimension_deductions = [0u32; 5];
    for deduction in &deductions {
        let slot = &mut dimension_deductions[dimension_index(deduction.dimension)];
        *slot = slot.saturating_add(u32::from(deduction.points));
    }
    let dimensions = score_dimensions(dimension_deductions);
    let weighted = u32::from(dimensions.task_completion) * 35
        + u32::from(dimensions.tool_reliability) * 25
        + u32::from(dimensions.constraint_adherence) * 15
        + u32::from(dimensions.performance_efficiency) * 15
        + u32::from(dimensions.runtime_stability) * 10;
    let total = ((weighted + 50) / 100).min(100) as u8;
    let grade = match total {
        90..=100 => ProductGrade::Excellent,
        75..=89 => ProductGrade::Good,
        60..=74 => ProductGrade::Fair,
        _ => ProductGrade::HighRisk,
    };
    let diagnoses = summarize_product_problems(findings, trusted_suite_case_ids);
    Ok(ProductScore {
        version: PRODUCT_SCORE_VERSION.into(),
        total: Some(total),
        grade: Some(grade),
        dimensions,
        deductions,
        confidence,
        diagnoses,
    })
}

fn validate_findings(findings: &[SmokeFinding]) -> Result<(), SmokeSafetyError> {
    if findings.len() > 10_000
        || findings.iter().any(|finding| {
            finding.id.is_empty()
                || finding.id.len() > 64
                || !finding
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                || finding
                    .case_id
                    .as_deref()
                    .is_some_and(|id| !display_safe_case_id(id))
                || !judge_text_is_safe(&finding.title, 300)
                || !judge_text_is_safe(&finding.recommendation, 300)
        })
    {
        return Err(SmokeSafetyError("smoke findings are unsafe"));
    }
    Ok(())
}

fn display_safe_case_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && judge_text_is_safe(value, 128)
}

fn diagnosis_policy(
    id: &str,
) -> Option<(ProductProblemArea, &'static str, &'static str, &'static str)> {
    let area = match id {
        "case_failed" | "timeout" | "case_timeout" | "runner_error" | "case_error" => {
            ProductProblemArea::TaskCompletion
        }
        "tool_event_failed" | "required_tool_missing" | "repeated_tool_use" => {
            ProductProblemArea::Toolchain
        }
        "unexpected_tool_use" | "forbidden_tool_use" => ProductProblemArea::Constraints,
        "slow_high_token" | "latency_outlier" => ProductProblemArea::Performance,
        "low_cache_hit_ratio" => ProductProblemArea::CacheStability,
        _ => return None,
    };
    let guidance = match area {
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
    };
    Some((area, guidance.0, guidance.1, guidance.2))
}

fn summarize_product_problems(
    findings: &[SmokeFinding],
    trusted: &[&str],
) -> Vec<ProductDiagnosis> {
    let allowed = trusted
        .iter()
        .copied()
        .filter(|id| display_safe_case_id(id))
        .collect::<HashSet<_>>();
    let mut grouped = BTreeMap::<ProductProblemArea, Vec<&SmokeFinding>>::new();
    for finding in findings {
        if let Some((area, ..)) = diagnosis_policy(finding.id()) {
            grouped.entry(area).or_default().push(finding);
        }
    }
    let mut diagnoses = grouped
        .into_iter()
        .map(|(area, candidates)| {
            let source = if candidates
                .iter()
                .any(|finding| finding.source() == FindingSource::Rule)
            {
                FindingSource::Rule
            } else {
                FindingSource::Judge
            };
            let selected = candidates
                .iter()
                .copied()
                .filter(|finding| finding.source() == source)
                .collect::<Vec<_>>();
            let severity = selected
                .iter()
                .map(|finding| finding.severity())
                .min_by_key(|severity| severity_rank(*severity))
                .expect("non-empty diagnosis");
            let mut ids = selected
                .iter()
                .filter_map(|finding| finding.case_id())
                .filter(|id| allowed.contains(id))
                .map(str::to_owned)
                .collect::<Vec<_>>();
            ids.sort();
            ids.dedup();
            let (_, conclusion, action, acceptance) =
                diagnosis_policy(selected[0].id()).expect("mapped diagnosis");
            ProductDiagnosis {
                area,
                severity,
                source,
                affected_case_count: ids.len(),
                affected_case_ids: ids,
                conclusion: if source == FindingSource::Judge {
                    format!("[AI 推断]{conclusion}")
                } else {
                    conclusion.to_owned()
                },
                evidence: format!(
                    "{} {} 次，涉及 {} 个安全用例标识。",
                    if source == FindingSource::Rule {
                        "规则命中"
                    } else {
                        "AI 推断命中"
                    },
                    selected.len(),
                    selected
                        .iter()
                        .filter_map(|finding| finding.case_id())
                        .filter(|id| allowed.contains(id))
                        .collect::<HashSet<_>>()
                        .len()
                ),
                action: action.to_owned(),
                acceptance: acceptance.to_owned(),
            }
        })
        .collect::<Vec<_>>();
    diagnoses.sort_by(|left, right| {
        severity_rank(left.severity)
            .cmp(&severity_rank(right.severity))
            .then_with(|| right.affected_case_count.cmp(&left.affected_case_count))
            .then_with(|| left.area.cmp(&right.area))
    });
    diagnoses.truncate(5);
    diagnoses
}

fn score_policy(id: &str) -> Option<(ProductScoreDimension, u8, &'static str)> {
    match id {
        "case_failed" => Some((
            ProductScoreDimension::TaskCompletion,
            35,
            "Evaluation case did not complete.",
        )),
        "tool_event_failed" => Some((
            ProductScoreDimension::ToolReliability,
            30,
            "A tool event failed.",
        )),
        "required_tool_missing" => Some((
            ProductScoreDimension::ToolReliability,
            25,
            "A required tool was not used.",
        )),
        "repeated_tool_use" => Some((
            ProductScoreDimension::ToolReliability,
            10,
            "A tool was used repeatedly.",
        )),
        "unexpected_tool_use" => Some((
            ProductScoreDimension::ConstraintAdherence,
            25,
            "A tool was used despite the case constraint.",
        )),
        "slow_high_token" => Some((
            ProductScoreDimension::PerformanceEfficiency,
            20,
            "A high-token case exceeded the latency threshold.",
        )),
        "latency_outlier" => Some((
            ProductScoreDimension::PerformanceEfficiency,
            12,
            "A case was a latency outlier.",
        )),
        "low_cache_hit_ratio" => Some((
            ProductScoreDimension::RuntimeStability,
            15,
            "A large-input case had a low cache hit ratio.",
        )),
        _ => None,
    }
}

fn score_deduction(id: &str, case_id: Option<&str>) -> ProductScoreDeduction {
    let (dimension, points, evidence) = score_policy(id).expect("known score policy");
    ProductScoreDeduction {
        finding_id: id.to_owned(),
        case_id: case_id.map(str::to_owned),
        evidence: evidence.to_owned(),
        dimension,
        points,
    }
}

fn dimension_index(dimension: ProductScoreDimension) -> usize {
    match dimension {
        ProductScoreDimension::TaskCompletion => 0,
        ProductScoreDimension::ToolReliability => 1,
        ProductScoreDimension::ConstraintAdherence => 2,
        ProductScoreDimension::PerformanceEfficiency => 3,
        ProductScoreDimension::RuntimeStability => 4,
    }
}

fn score_dimensions(deductions: [u32; 5]) -> ProductScoreDimensions {
    let remaining = |deduction: u32| 100u32.saturating_sub(deduction.min(100)) as u8;
    ProductScoreDimensions {
        task_completion: remaining(deductions[0]),
        tool_reliability: remaining(deductions[1]),
        constraint_adherence: remaining(deductions[2]),
        performance_efficiency: remaining(deductions[3]),
        runtime_stability: remaining(deductions[4]),
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

#[derive(Clone, PartialEq)]
pub struct JudgeDimensionScore {
    dimension: String,
    score: u8,
    confidence: f32,
    evidence: String,
}

impl JudgeDimensionScore {
    pub fn new(
        dimension: impl Into<String>,
        score: u8,
        confidence: f32,
        evidence: impl Into<String>,
    ) -> Self {
        Self {
            dimension: dimension.into(),
            score,
            confidence,
            evidence: evidence.into(),
        }
    }
    pub fn dimension(&self) -> &str {
        &self.dimension
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JudgeStatus {
    Completed,
    NotConfigured,
}

#[derive(Clone, PartialEq)]
pub struct JudgeReport {
    status: JudgeStatus,
    dimensions: Vec<JudgeDimensionScore>,
    findings: Vec<SmokeFinding>,
}

impl JudgeReport {
    pub fn status(&self) -> &JudgeStatus {
        &self.status
    }
    pub fn dimensions(&self) -> &[JudgeDimensionScore] {
        &self.dimensions
    }
    pub fn findings(&self) -> &[SmokeFinding] {
        &self.findings
    }
}

#[derive(Clone, PartialEq)]
pub struct JudgeWireResponse {
    dimensions: Vec<JudgeDimensionScore>,
    findings: Vec<SmokeFinding>,
}

impl JudgeWireResponse {
    pub fn new(dimensions: Vec<JudgeDimensionScore>, findings: Vec<SmokeFinding>) -> Self {
        Self {
            dimensions,
            findings,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JudgeParseError(&'static str);

impl fmt::Display for JudgeParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for JudgeParseError {}

pub fn parse_judge_response(response: JudgeWireResponse) -> Result<JudgeReport, JudgeParseError> {
    let mut response = response;
    if response.dimensions.len() != REQUIRED_JUDGE_DIMENSIONS.len() {
        return Err(JudgeParseError("judge must provide exactly six dimensions"));
    }
    let mut seen = HashSet::new();
    for dimension in &response.dimensions {
        if !REQUIRED_JUDGE_DIMENSIONS.contains(&dimension.dimension.as_str())
            || !seen.insert(dimension.dimension.as_str())
        {
            return Err(JudgeParseError("judge dimensions must be known and unique"));
        }
        if dimension.score > 100
            || !dimension.confidence.is_finite()
            || !(0.0..=1.0).contains(&dimension.confidence)
            || dimension.evidence.trim().is_empty()
            || !judge_text_is_safe(&dimension.evidence, 500)
        {
            return Err(JudgeParseError("judge dimension values are invalid"));
        }
    }
    if response.findings.len() > 20
        || response.findings.iter().any(|finding| {
            finding.id.len() > 64
                || !finding
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                || finding
                    .case_id
                    .as_deref()
                    .is_some_and(|case_id| !judge_text_is_safe(case_id, 128))
                || !judge_text_is_safe(&finding.title, 300)
                || !judge_text_is_safe(&finding.recommendation, 300)
        })
    {
        return Err(JudgeParseError("judge findings are invalid"));
    }
    for finding in &mut response.findings {
        finding.source = FindingSource::Judge;
    }
    Ok(JudgeReport {
        status: JudgeStatus::Completed,
        dimensions: response.dimensions,
        findings: response.findings,
    })
}

fn judge_text_is_safe(value: &str, max_chars: usize) -> bool {
    if value.chars().count() > max_chars || value.chars().any(char::is_control) {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    let forbidden = [
        "authorization",
        "bearer ",
        "cookie",
        "api_key",
        "apikey",
        "access_token",
        "auth_token",
        "client_secret",
        "password",
        "passwd",
        "github_pat_",
        "ghp_",
        "glpat-",
        "xoxb-",
        "sk-",
        "sk_",
    ];
    !forbidden.iter().any(|marker| lower.contains(marker)) && !value.contains("AKIA")
}

pub fn not_configured_judge() -> JudgeReport {
    JudgeReport {
        status: JudgeStatus::NotConfigured,
        dimensions: vec![],
        findings: vec![],
    }
}

pub fn render_smoke_markdown(
    records: &[SmokeRecord],
    analysis: &RuleAnalysis,
    score: &ProductScore,
    judge: &JudgeReport,
) -> Result<String, SmokeSafetyError> {
    validate_findings(analysis.findings())?;
    validate_findings(judge.findings())?;
    let completed = records
        .iter()
        .filter(|record| record.outcome().status() == TaskStatus::Completed)
        .count();
    let score_text = score
        .total()
        .map(|value| format!("{value}/100 ({})", score.version()))
        .unwrap_or_else(|| "unavailable".into());
    let grade_text = match score.grade() {
        Some(ProductGrade::Excellent) => "优秀",
        Some(ProductGrade::Good) => "良好",
        Some(ProductGrade::Fair) => "需改进",
        Some(ProductGrade::HighRisk) => "高风险",
        None => "不可用",
    };
    let confidence_text = match score.confidence() {
        ProductScoreConfidence::Unavailable => "Unavailable（不可用）",
        ProductScoreConfidence::LowSample => "LowSample（小样本）",
        ProductScoreConfidence::Standard => "Standard（标准）",
    };
    let judge_text = match judge.status() {
        JudgeStatus::Completed => "completed",
        JudgeStatus::NotConfigured => "not_configured",
    };
    let recommendations = if score.diagnoses().is_empty() {
        "未发现需要优先处理的确定性问题。".to_owned()
    } else {
        score
            .diagnoses()
            .iter()
            .map(|diagnosis| {
                format!(
                    "- [{:?}/{:?}] {}\n  - Evidence: {}\n  - Action: {}\n  - Acceptance: {}\n  - Cases: {}",
                    diagnosis.area(),
                    diagnosis.severity(),
                    diagnosis.conclusion(),
                    diagnosis.evidence(),
                    diagnosis.action(),
                    diagnosis.acceptance(),
                    if diagnosis.affected_case_ids().is_empty() {
                        "无安全标识".to_owned()
                    } else {
                        diagnosis.affected_case_ids().join(", ")
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let dimension_value = |value: u8| {
        score
            .total()
            .map(|_| value.to_string())
            .unwrap_or_else(|| "不可用".to_owned())
    };
    let dimensions = format!(
        "| Dimension | Score |\n|---|---:|\n| Task Completion | {} |\n| Tool Reliability | {} |\n| Constraint Adherence | {} |\n| Performance Efficiency | {} |\n| Runtime Stability | {} |",
        dimension_value(score.dimensions().task_completion()),
        dimension_value(score.dimensions().tool_reliability()),
        dimension_value(score.dimensions().constraint_adherence()),
        dimension_value(score.dimensions().performance_efficiency()),
        dimension_value(score.dimensions().runtime_stability()),
    );
    let deductions = if score.deductions().is_empty() {
        "- 无扣分。".to_owned()
    } else {
        score
            .deductions()
            .iter()
            .map(|deduction| {
                format!(
                    "- {} · Case: {} · {:?} · -{}（{}）",
                    deduction.finding_id(),
                    deduction.case_id().unwrap_or("整体"),
                    deduction.dimension(),
                    deduction.points(),
                    deduction.evidence()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let low_sample_warning = if score.confidence() == ProductScoreConfidence::LowSample {
        "\n- 警告：样本量较小，分数仅用于本次 smoke 诊断。"
    } else {
        ""
    };
    let judge_note = if judge.status() == &JudgeStatus::NotConfigured {
        "\nJudge 未配置；Product Score 不受影响。"
    } else {
        ""
    };
    Ok(format!(
        "# Pinvou Smoke 报告\n\n- Cases: {}\n- Completed: {completed}\n\n## Smoke Health Score\n\n- 总分：{score_text}\n- 等级：{grade_text}\n- 公式版本：{}\n- Confidence: {confidence_text}{low_sample_warning}\n\n{dimensions}\n\n### Deductions / 扣分明细\n\n{deductions}\n\n> 该健康分只用于内部 Smoke 产品诊断，不是官方 benchmark 分数。公开榜单分数：不可用。\n\n## 产品问题与改进方向\n\n发现 {} 项，建议优化如下：\n\n{recommendations}\n\n## 独立 Judge 质量评分\n\n状态：{judge_text}{judge_note}\n",
        records.len(),
        score.version(),
        analysis.findings().len()
    ))
}
