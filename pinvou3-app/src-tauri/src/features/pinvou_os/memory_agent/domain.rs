//! Memory Agent 整理内核的领域对象。
//!
//! 这些对象刻意不包含 Session。Mission / Run 只允许作为证据出处，不能成为
//! 记忆的生命周期容器；任务进度仍由 Task Kernel 持有。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MEMORY_ORGANIZER_SCHEMA_VERSION: u32 = 1;
pub const MAX_MEMORY_CANDIDATES_PER_BATCH: usize = 128;
pub const MAX_MEMORY_EVIDENCE_PER_CANDIDATE: usize = 32;
pub const MAX_MEMORY_EVIDENCE_PER_RECORD: usize = 128;
pub const MAX_MEMORY_CANDIDATE_FINGERPRINT_SAMPLES: usize = 16;
pub const MAX_MEMORY_CONFLICTS_PER_RECORD: usize = 32;
pub const MAX_MEMORY_SUPERSEDED_RECORDS_PER_RECORD: usize = 64;
pub const MAX_MEMORY_RECORDS_PER_BASE_SLOT: usize = 64;
pub const MAX_MEMORY_IDS_PER_RECEIPT: usize = 128;
pub const MAX_ORGANIZED_MEMORY_RECORDS: usize = 10_000;
pub const MAX_PROCESSED_MEMORY_CANDIDATES: usize = 50_000;
pub const MAX_ORGANIZED_CONTEXT_ITEMS: usize = 128;
pub const MAX_ORGANIZED_MEMORY_VALUE_BYTES: usize = 16 * 1024;

/// 记忆描述的是哪一种长期信息，而不是它存在哪个“会话”中。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum OrganizedMemoryKind {
    /// 在特定时间、空间或环境下成立的事实。
    ContextualFact,
    /// 用户明确表达，或从多次行为中得到充分支持的偏好。
    Preference,
    /// 需要跨多个独立事件才可确认的稳定行为模式。
    Habit,
    /// Pinvou 做过什么、结果怎样的一次完整经历。
    ActionExperience,
    /// 从多次经历中提炼出的可复用规律。
    Lesson,
}

/// 证据是怎样产生的。来源类型决定“能不能晋升”，而不只是调整一个浮点分数。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEvidenceOrigin {
    UserExplicit,
    ObservedBehavior,
    AgentAction,
    /// 由 Task Kernel 的终态回执或独立结果验证器签发，不是执行 Agent 自报。
    VerifiedTaskOutcome,
    ExternalSource,
    ModelInference,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEvidencePolarity {
    Supports,
    Contradicts,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum OrganizedMemoryStatus {
    /// 证据仍不足；默认不会注入交互上下文。
    Provisional,
    /// 已满足该记忆类型的确定性晋升规则。
    Confirmed,
    /// 存在权威程度相近、时间和环境重叠的反向主张。
    Disputed,
    /// 被有明确关系的新版本替代，旧版本仍保留供审计。
    Superseded,
    /// 被用户或受信来源撤回，不物理删除。
    Retracted,
    /// 已超出有效时间，不进入当前投影。
    Expired,
}

/// 候选与既有记忆的关系必须由提取阶段明确表达，整理器不会凭文本相似度
/// 武断地把一个新说法当成“纠正”或“更新”。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCandidateIntent {
    Assert,
    Replace,
    Contradict,
}

/// 事实或规则在哪些条件下适用。`environment` 是确定性的结构化约束，例如
/// `{ "device": "tablet", "network": "office" }`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryApplicability {
    /// personal / family / work 等身份空间，必须显式提供，防止跨空间污染。
    pub space_id: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub environment: BTreeMap<String, String>,
    pub valid_from_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_until_ms: Option<i64>,
}

/// 原始事实仍在统一事件账本中；这里仅保存可追溯引用和判断所需的最小元数据。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryEvidence {
    pub event_id: String,
    pub source_actor_id: String,
    pub origin: MemoryEvidenceOrigin,
    pub polarity: MemoryEvidencePolarity,
    /// 事情发生或被观察到的时间。
    pub observed_at_ms: i64,
    /// Pinvou 获知并记录这条证据的时间。
    pub recorded_at_ms: i64,
    pub reliability: f32,
    /// 只记录证据来自哪个任务，不把记忆塞进任务容器。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

/// 经过可信事件适配器封装后的候选。规则或模型只能产生未验证草案和事件引用；
/// `origin`、`source_actor_id`、时间与可靠度必须由账本适配器填写。只有本整理器能
/// 决定候选最终是临时、确认、争议还是替代关系。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCandidate {
    /// 应由上游基于来源事件稳定生成，保证事件回放幂等。
    pub candidate_id: String,
    pub kind: OrganizedMemoryKind,
    pub subject: String,
    pub predicate: String,
    pub value: Value,
    pub applicability: MemoryApplicability,
    /// 对未来上下文选择的重要程度，不等于事实可信度。
    pub importance: f32,
    /// 提取器对“这段内容表达了该候选”的把握，不赋予模型事实权威。
    pub confidence: f32,
    pub intent: MemoryCandidateIntent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_memory_id: Option<String>,
    pub evidence: Vec<MemoryEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryRetraction {
    pub operation_id: String,
    pub reason: String,
    pub retracted_at_ms: i64,
    pub evidence: Vec<MemoryEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrganizedMemory {
    pub memory_id: String,
    pub kind: OrganizedMemoryKind,
    pub subject: String,
    pub predicate: String,
    pub value: Value,
    pub applicability: MemoryApplicability,
    pub importance: f32,
    /// 仅用于排序和提示，不作为权限判断，也不能让推断越级成为事实。
    pub confidence: f32,
    pub status: OrganizedMemoryStatus,
    pub supporting_evidence: Vec<MemoryEvidence>,
    pub contradicting_evidence: Vec<MemoryEvidence>,
    /// 完整候选谱系属于未来的记忆决策流。热投影只保留计数和少量固定长度指纹样本，
    /// 避免长寿命系统被任意长度的 candidate id 撑大。
    pub absorbed_candidate_count: u64,
    pub candidate_fingerprint_samples: BTreeSet<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub supersedes_memory_ids: BTreeSet<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_by_memory_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub conflicts_with_memory_ids: BTreeSet<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retraction: Option<MemoryRetraction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessedMemoryCandidate {
    pub applied_revision: u64,
    /// 规范化候选的 SHA-256；同 ID 不同内容必须拒绝，不能被当成幂等重放。
    pub candidate_fingerprint: String,
    pub memory_ids: Vec<String>,
}

/// 可单独序列化和回放的整理状态。本轮不把它接入旧的完整投影账本，避免每次
/// 写入都复制全部记忆所造成的平方级磁盘放大。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryOrganizerState {
    pub schema_version: u32,
    pub revision: u64,
    pub records: BTreeMap<String, OrganizedMemory>,
    pub processed_candidates: BTreeMap<String, ProcessedMemoryCandidate>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dispute_resolutions: BTreeMap<String, MemoryDisputeResolutionReceipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_maintenance_at_ms: Option<i64>,
}

impl Default for MemoryOrganizerState {
    fn default() -> Self {
        Self {
            schema_version: MEMORY_ORGANIZER_SCHEMA_VERSION,
            revision: 0,
            records: BTreeMap::new(),
            processed_candidates: BTreeMap::new(),
            dispute_resolutions: BTreeMap::new(),
            last_maintenance_at_ms: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryOrganizationAction {
    Created,
    Merged,
    Superseded,
    Disputed,
    IgnoredDuplicate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryOrganizationReceipt {
    pub candidate_id: String,
    pub revision: u64,
    pub action: MemoryOrganizationAction,
    pub affected_memory_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RejectedMemoryCandidate {
    pub candidate_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryBatchOutcome {
    pub accepted: Vec<MemoryOrganizationReceipt>,
    pub rejected: Vec<RejectedMemoryCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RetractOrganizedMemoryRequest {
    pub operation_id: String,
    pub memory_id: String,
    pub reason: String,
    pub retracted_at_ms: i64,
    pub evidence: Vec<MemoryEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryRetractionReceipt {
    pub memory_id: String,
    pub revision: u64,
    pub changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryMaintenanceReport {
    pub revision: u64,
    pub expired_memory_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolveMemoryDisputeRequest {
    pub operation_id: String,
    pub winner_memory_id: String,
    pub losing_memory_ids: Vec<String>,
    pub reason: String,
    pub resolved_at_ms: i64,
    /// 必须来自用户明确纠正或不低于争议双方权威的可信来源。
    pub evidence: Vec<MemoryEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryDisputeResolutionReceipt {
    pub operation_id: String,
    pub request_fingerprint: String,
    pub winner_memory_id: String,
    pub superseded_memory_ids: Vec<String>,
    pub revision: u64,
    pub changed: bool,
}

/// 这是 Memory Agent 自己的稳定投影请求，不是最终的模型 prompt 组装协议。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OrganizedMemoryQuery {
    /// 当前适用性判断时间，不是历史知识快照查询。历史回放应读取记忆决策账本。
    pub current_at_ms: i64,
    pub space_id: String,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub kinds: Vec<OrganizedMemoryKind>,
    #[serde(default)]
    pub subjects: Vec<String>,
    #[serde(default)]
    pub predicates: Vec<String>,
    /// 第一阶段只做轻量、确定性的子串匹配；向量模型不是硬依赖。
    #[serde(default)]
    pub focus_terms: Vec<String>,
    #[serde(default)]
    pub include_provisional: bool,
    #[serde(default)]
    pub include_disputed: bool,
    pub max_items: usize,
}

impl Default for OrganizedMemoryQuery {
    fn default() -> Self {
        Self {
            current_at_ms: 0,
            space_id: "personal".to_string(),
            environment: BTreeMap::new(),
            kinds: Vec::new(),
            subjects: Vec::new(),
            predicates: Vec::new(),
            focus_terms: Vec::new(),
            include_provisional: false,
            include_disputed: false,
            max_items: 32,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OrganizedMemoryContextItem {
    pub memory_id: String,
    pub kind: OrganizedMemoryKind,
    pub subject: String,
    pub predicate: String,
    pub value: Value,
    pub applicability: MemoryApplicability,
    pub status: OrganizedMemoryStatus,
    pub importance: f32,
    pub confidence: f32,
    /// Memory Agent 对入选顺序的确定性评分。
    pub selection_score: f32,
    /// 入选项之间归一化后的建议占比；它不是 Transformer 内部 attention 权重。
    pub selection_weight: f32,
    pub evidence_event_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OrganizedMemoryProjection {
    pub revision: u64,
    pub generated_at_ms: i64,
    pub items: Vec<OrganizedMemoryContextItem>,
    pub omitted_count: usize,
    pub evidence_event_ids: Vec<String>,
}
