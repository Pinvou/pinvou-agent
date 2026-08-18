//! PinvouOS Memory Agent：第二代确定性整理核心，以及旧 MemoryAgent 的只读迁移兼容面。
//!
//! 新核心不拥有 Session，也不把模型上下文窗口当作记忆真相；它从统一 Runtime
//! 账本中的结构化证据生成可重放决策和有界热投影。下方旧 Working/DurableFact
//! 类型仅用于兼容调用与一次迁移，不能再成为可写真相源。

#[path = "memory_agent/decision.rs"]
mod decision;
#[path = "memory_agent/domain.rs"]
mod domain;
#[path = "memory_agent/organizer.rs"]
mod organizer;
#[path = "memory_agent/retrieval.rs"]
mod retrieval;
#[path = "memory_agent/runtime_adapter.rs"]
mod runtime_adapter;

#[allow(unused_imports)]
pub use self::decision::*;
#[allow(unused_imports)]
pub use self::domain::*;
#[allow(unused_imports)]
pub use self::organizer::*;
#[allow(unused_imports)]
pub use self::runtime_adapter::*;

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::model::{CapabilityContract, Interruptibility, ResourceClass, PINVOU_IDENTITY_ID};

pub const MEMORY_AGENT_ID: &str = "agent:memory";
pub const MEMORY_REMEMBER_CAPABILITY_ID: &str = "memory.remember";
pub const MEMORY_CONTEXT_CAPABILITY_ID: &str = "memory.context";
pub const MEMORY_RETRACT_CAPABILITY_ID: &str = "memory.retract";
pub const MAX_CONTEXT_ITEMS: usize = 128;
pub const MAX_MEMORY_RECORDS: usize = 10_000;
pub const MAX_MEMORY_VALUE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTier {
    /// 当前 Mission/Run 正在使用的短期事实；没有 Mission 时表示全局工作状态。
    Working,
    /// 经证据支持、可跨 Mission 延续的长期事实。
    DurableFact,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRecordStatus {
    Active,
    Superseded,
    Retracted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecord {
    pub memory_id: String,
    pub tier: MemoryTier,
    pub subject: String,
    pub predicate: String,
    pub value: Value,
    pub confidence: f32,
    pub source_actor_id: String,
    pub evidence_event_ids: Vec<String>,
    pub observed_at_ms: i64,
    pub recorded_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub status: MemoryRecordStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes_memory_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_by_memory_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retraction_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retracted_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retracted_by_actor_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RememberMemoryRequest {
    pub memory_id: String,
    pub tier: MemoryTier,
    pub subject: String,
    pub predicate: String,
    pub value: Value,
    pub confidence: f32,
    pub source_actor_id: String,
    pub evidence_event_ids: Vec<String>,
    pub observed_at_ms: i64,
    pub recorded_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RetractMemoryRequest {
    pub memory_id: String,
    pub reason: String,
    pub source_actor_id: String,
    pub evidence_event_ids: Vec<String>,
    pub retracted_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryMutationReceipt {
    pub memory_id: String,
    pub revision: u64,
    pub status: MemoryRecordStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_memory_id: Option<String>,
    pub evidence_event_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompileMemoryContextRequest {
    /// 当前工作目标的标识；只用于选择工作记忆，不是 Session。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<String>,
    #[serde(default)]
    pub subjects: Vec<String>,
    #[serde(default)]
    pub predicates: Vec<String>,
    #[serde(default = "default_include_working")]
    pub include_working: bool,
    #[serde(default = "default_include_durable_facts")]
    pub include_durable_facts: bool,
    #[serde(default = "default_context_items")]
    pub max_items: usize,
}

fn default_include_working() -> bool {
    true
}

fn default_include_durable_facts() -> bool {
    true
}

fn default_context_items() -> usize {
    32
}

impl Default for CompileMemoryContextRequest {
    fn default() -> Self {
        Self {
            mission_id: None,
            subjects: Vec::new(),
            predicates: Vec::new(),
            include_working: true,
            include_durable_facts: true,
            max_items: default_context_items(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryContextStatus {
    Ready,
    Partial,
    Empty,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryContextItem {
    pub memory_id: String,
    pub tier: MemoryTier,
    pub subject: String,
    pub predicate: String,
    pub value: Value,
    pub confidence: f32,
    pub observed_at_ms: i64,
    pub source_actor_id: String,
    pub evidence_event_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryContextProjection {
    pub identity_id: String,
    pub revision: u64,
    pub compiled_at_ms: i64,
    pub status: MemoryContextStatus,
    pub items: Vec<MemoryContextItem>,
    pub omitted_count: usize,
    /// 输出使用到的全部证据事件，去重并稳定排序，便于调用方继续追溯账本。
    pub evidence_event_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryProjectionState {
    pub identity_id: String,
    pub revision: u64,
    pub records: BTreeMap<String, MemoryRecord>,
}

impl Default for MemoryProjectionState {
    fn default() -> Self {
        Self {
            identity_id: PINVOU_IDENTITY_ID.to_string(),
            revision: 0,
            records: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryAgentError {
    message: String,
}

impl MemoryAgentError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for MemoryAgentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for MemoryAgentError {}

#[derive(Debug, Clone, Default)]
pub struct MemoryAgent {
    projection: MemoryProjectionState,
}

impl MemoryAgent {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_projection(projection: MemoryProjectionState) -> Result<Self, MemoryAgentError> {
        if projection.identity_id != PINVOU_IDENTITY_ID {
            return Err(MemoryAgentError::new("memory projection identity mismatch"));
        }
        for (memory_id, record) in &projection.records {
            if memory_id != &record.memory_id {
                return Err(MemoryAgentError::new(
                    "memory projection key does not match record id",
                ));
            }
            validate_record(record)?;
        }
        Ok(Self { projection })
    }

    pub fn projection(&self) -> MemoryProjectionState {
        self.projection.clone()
    }

    /// 写入一条可追溯记忆。同一作用域、subject、predicate 的活动记录会被 supersede，
    /// 旧记录不删除，从而让连续身份的事实变化可审计、可回放。
    pub fn remember(
        &mut self,
        request: RememberMemoryRequest,
    ) -> Result<MemoryMutationReceipt, MemoryAgentError> {
        let mut record = MemoryRecord {
            memory_id: required_text(request.memory_id, "memory id")?,
            tier: request.tier,
            subject: required_text(request.subject, "memory subject")?,
            predicate: required_text(request.predicate, "memory predicate")?,
            value: request.value,
            confidence: request.confidence,
            source_actor_id: required_text(request.source_actor_id, "source actor id")?,
            evidence_event_ids: normalized_ids(request.evidence_event_ids, "evidence event id")?,
            observed_at_ms: request.observed_at_ms,
            recorded_at_ms: request.recorded_at_ms,
            mission_id: optional_text(request.mission_id, "mission id")?,
            run_id: optional_text(request.run_id, "run id")?,
            status: MemoryRecordStatus::Active,
            supersedes_memory_id: None,
            superseded_by_memory_id: None,
            retraction_reason: None,
            retracted_at_ms: None,
            retracted_by_actor_id: None,
        };
        validate_record(&record)?;
        if self.projection.records.contains_key(&record.memory_id) {
            return Err(MemoryAgentError::new(format!(
                "memory id {} already exists",
                record.memory_id
            )));
        }
        if self.projection.records.len() >= MAX_MEMORY_RECORDS {
            return Err(MemoryAgentError::new(format!(
                "memory record limit {MAX_MEMORY_RECORDS} reached"
            )));
        }

        let superseded_id = self
            .projection
            .records
            .values()
            .filter(|existing| existing.status == MemoryRecordStatus::Active)
            .filter(|existing| same_memory_slot(existing, &record))
            .max_by(|left, right| {
                left.recorded_at_ms
                    .cmp(&right.recorded_at_ms)
                    .then_with(|| left.memory_id.cmp(&right.memory_id))
            })
            .map(|existing| existing.memory_id.clone());

        if let Some(superseded_id) = superseded_id.as_ref() {
            let previous = self
                .projection
                .records
                .get_mut(superseded_id)
                .expect("selected memory record must exist");
            if record.recorded_at_ms < previous.recorded_at_ms {
                return Err(MemoryAgentError::new(
                    "older memory update must not supersede newer projected truth",
                ));
            }
            previous.status = MemoryRecordStatus::Superseded;
            previous.superseded_by_memory_id = Some(record.memory_id.clone());
            record.supersedes_memory_id = Some(superseded_id.clone());
        }

        let memory_id = record.memory_id.clone();
        let evidence_event_ids = record.evidence_event_ids.clone();
        self.projection.records.insert(memory_id.clone(), record);
        self.projection.revision = self.projection.revision.saturating_add(1);
        Ok(MemoryMutationReceipt {
            memory_id,
            revision: self.projection.revision,
            status: MemoryRecordStatus::Active,
            superseded_memory_id: superseded_id,
            evidence_event_ids,
        })
    }

    /// 撤回错误或过期事实。撤回必须再次提供证据，且不会物理删除原记录。
    pub fn retract(
        &mut self,
        request: RetractMemoryRequest,
    ) -> Result<MemoryMutationReceipt, MemoryAgentError> {
        let memory_id = required_text(request.memory_id, "memory id")?;
        let reason = required_text(request.reason, "retraction reason")?;
        let source_actor_id = required_text(request.source_actor_id, "source actor id")?;
        let retraction_evidence = normalized_ids(request.evidence_event_ids, "evidence event id")?;
        if retraction_evidence.is_empty() {
            return Err(MemoryAgentError::new(
                "memory retraction requires at least one evidence event",
            ));
        }
        if request.retracted_at_ms < 0 {
            return Err(MemoryAgentError::new(
                "memory retraction timestamp must be non-negative",
            ));
        }
        let record = self
            .projection
            .records
            .get_mut(&memory_id)
            .ok_or_else(|| MemoryAgentError::new(format!("unknown memory {memory_id}")))?;
        if record.status != MemoryRecordStatus::Active {
            return Err(MemoryAgentError::new(format!(
                "memory {memory_id} is not active"
            )));
        }
        record.status = MemoryRecordStatus::Retracted;
        record.retraction_reason = Some(reason);
        record.retracted_at_ms = Some(request.retracted_at_ms);
        record.retracted_by_actor_id = Some(source_actor_id);
        record.evidence_event_ids.extend(retraction_evidence);
        record.evidence_event_ids.sort();
        record.evidence_event_ids.dedup();
        self.projection.revision = self.projection.revision.saturating_add(1);
        Ok(MemoryMutationReceipt {
            memory_id,
            revision: self.projection.revision,
            status: MemoryRecordStatus::Retracted,
            superseded_memory_id: None,
            evidence_event_ids: record.evidence_event_ids.clone(),
        })
    }

    /// 为一个 Mission 编译最小上下文切片。长期事实跨 Mission 可见；工作记忆只取
    /// 全局记录或当前 Mission 的记录，避免并发任务互相污染。
    pub fn compile_context(
        &self,
        request: CompileMemoryContextRequest,
        compiled_at_ms: i64,
    ) -> Result<MemoryContextProjection, MemoryAgentError> {
        if compiled_at_ms < 0 {
            return Err(MemoryAgentError::new(
                "context compilation timestamp must be non-negative",
            ));
        }
        if request.max_items == 0 || request.max_items > MAX_CONTEXT_ITEMS {
            return Err(MemoryAgentError::new(format!(
                "max items must be between 1 and {MAX_CONTEXT_ITEMS}"
            )));
        }
        let mission_id = optional_text(request.mission_id, "mission id")?;
        let subjects = normalized_filter(request.subjects, "subject")?;
        let predicates = normalized_filter(request.predicates, "predicate")?;

        let mut candidates = self
            .projection
            .records
            .values()
            .filter(|record| record.status == MemoryRecordStatus::Active)
            .filter(|record| match record.tier {
                MemoryTier::Working => {
                    request.include_working
                        && (record.mission_id.is_none() || record.mission_id == mission_id)
                }
                MemoryTier::DurableFact => request.include_durable_facts,
            })
            .filter(|record| subjects.is_empty() || subjects.contains(&record.subject))
            .filter(|record| predicates.is_empty() || predicates.contains(&record.predicate))
            .cloned()
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            context_rank(left.tier)
                .cmp(&context_rank(right.tier))
                .then_with(|| right.observed_at_ms.cmp(&left.observed_at_ms))
                .then_with(|| left.memory_id.cmp(&right.memory_id))
        });

        let omitted_count = candidates.len().saturating_sub(request.max_items);
        candidates.truncate(request.max_items);
        let items = candidates
            .into_iter()
            .map(|record| MemoryContextItem {
                memory_id: record.memory_id,
                tier: record.tier,
                subject: record.subject,
                predicate: record.predicate,
                value: record.value,
                confidence: record.confidence,
                observed_at_ms: record.observed_at_ms,
                source_actor_id: record.source_actor_id,
                evidence_event_ids: record.evidence_event_ids,
                mission_id: record.mission_id,
                run_id: record.run_id,
            })
            .collect::<Vec<_>>();
        let evidence_event_ids = items
            .iter()
            .flat_map(|item| item.evidence_event_ids.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let status = if items.is_empty() {
            MemoryContextStatus::Empty
        } else if omitted_count > 0 {
            MemoryContextStatus::Partial
        } else {
            MemoryContextStatus::Ready
        };
        Ok(MemoryContextProjection {
            identity_id: self.projection.identity_id.clone(),
            revision: self.projection.revision,
            compiled_at_ms,
            status,
            items,
            omitted_count,
            evidence_event_ids,
        })
    }
}

/// Memory Agent 注册时应声明的全部原子能力。Runtime 集成层只需把这些契约挂到
/// 常驻 `agent:memory` manifest；模块本身不修改共享 builtin 注册表。
pub fn memory_capabilities() -> Vec<CapabilityContract> {
    vec![
        CapabilityContract {
            capability_id: MEMORY_REMEMBER_CAPABILITY_ID.to_string(),
            version: 3,
            summary: "把与统一账本结构化 Claim 精确绑定的长期事实交给确定性整理器；任务运行态不进入长期记忆"
                .to_string(),
            input_schema: json!({
                "type": "object",
                "required": ["memoryId", "tier", "subject", "predicate", "value", "confidence", "sourceActorId", "evidenceEventIds", "observedAtMs", "recordedAtMs"]
            }),
            output_schema: json!({
                "type": "object",
                "required": ["memoryId", "revision", "status", "evidenceEventIds"]
            }),
            preconditions: vec![
                "evidence_event_is_trusted_and_eligible".to_string(),
                "candidate_exactly_matches_structured_claim".to_string(),
            ],
            permissions: vec!["private_memory_write".to_string()],
            side_effects: vec!["organized_memory_decision_recorded".to_string()],
            resource_class: ResourceClass::Light,
            interruptibility: Interruptibility::Atomic,
            idempotent: true,
        },
        CapabilityContract {
            capability_id: MEMORY_CONTEXT_CAPABILITY_ID.to_string(),
            version: 3,
            summary: "从最近一次耐久提交的结构索引生成有界、可追溯的相关记忆切片".to_string(),
            input_schema: json!({
                "type": "object",
                "required": ["maxItems"]
            }),
            output_schema: json!({
                "type": "object",
                "required": ["identityId", "revision", "status", "items", "omittedCount", "evidenceEventIds"]
            }),
            preconditions: Vec::new(),
            permissions: vec!["private_memory_read".to_string()],
            side_effects: Vec::new(),
            resource_class: ResourceClass::Light,
            interruptibility: Interruptibility::Immediate,
            idempotent: true,
        },
        CapabilityContract {
            capability_id: MEMORY_RETRACT_CAPABILITY_ID.to_string(),
            version: 3,
            summary: "用与目标内容精确匹配的结构化 ClaimRetracted 事件撤回活动记忆"
                .to_string(),
            input_schema: json!({
                "type": "object",
                "required": ["memoryId", "reason", "sourceActorId", "evidenceEventIds", "retractedAtMs"]
            }),
            output_schema: json!({
                "type": "object",
                "required": ["memoryId", "revision", "status", "evidenceEventIds"]
            }),
            preconditions: vec![
                "memory_record_is_active".to_string(),
                "evidence_event_is_trusted_and_eligible".to_string(),
                "retraction_exactly_matches_target_claim".to_string(),
            ],
            permissions: vec!["private_memory_write".to_string()],
            side_effects: vec!["organized_memory_decision_recorded".to_string()],
            resource_class: ResourceClass::Light,
            interruptibility: Interruptibility::Atomic,
            idempotent: true,
        },
    ]
}

fn validate_record(record: &MemoryRecord) -> Result<(), MemoryAgentError> {
    required_text(record.memory_id.clone(), "memory id")?;
    required_text(record.subject.clone(), "memory subject")?;
    required_text(record.predicate.clone(), "memory predicate")?;
    required_text(record.source_actor_id.clone(), "source actor id")?;
    if !record.confidence.is_finite() || !(0.0..=1.0).contains(&record.confidence) {
        return Err(MemoryAgentError::new(
            "memory confidence must be between 0 and 1",
        ));
    }
    if record.observed_at_ms < 0 || record.recorded_at_ms < 0 {
        return Err(MemoryAgentError::new(
            "memory timestamps must be non-negative",
        ));
    }
    if record.evidence_event_ids.is_empty() {
        return Err(MemoryAgentError::new(
            "memory record requires at least one evidence event",
        ));
    }
    let value_bytes = serde_json::to_vec(&record.value).map_err(|error| {
        MemoryAgentError::new(format!("memory value is not serializable: {error}"))
    })?;
    if value_bytes.len() > MAX_MEMORY_VALUE_BYTES {
        return Err(MemoryAgentError::new(format!(
            "memory value exceeds {MAX_MEMORY_VALUE_BYTES} bytes"
        )));
    }
    if memory_slot_is_credential(&record.subject, &record.predicate)
        && !is_credential_reference(&record.value)
    {
        return Err(MemoryAgentError::new(
            "credential memory must store a keyring reference, never secret material",
        ));
    }
    if record.tier == MemoryTier::DurableFact
        && (record.mission_id.is_some() || record.run_id.is_some())
    {
        return Err(MemoryAgentError::new(
            "durable fact must not be owned by a mission or run",
        ));
    }
    if record.run_id.is_some() && record.mission_id.is_none() {
        return Err(MemoryAgentError::new(
            "working memory with run id requires mission id",
        ));
    }
    Ok(())
}

fn memory_slot_is_credential(subject: &str, predicate: &str) -> bool {
    let slot = format!("{subject}.{predicate}")
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    [
        "password",
        "passwd",
        "secret",
        "token",
        "apikey",
        "credential",
        "privatekey",
        "clientsecret",
        "authorization",
        "bearer",
        "cookie",
        "sessionkey",
    ]
    .iter()
    .any(|marker| slot.contains(marker))
}

fn is_credential_reference(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object
        .get("credentialRef")
        .and_then(Value::as_str)
        .is_some_and(|reference| {
            let reference = reference.trim();
            reference.starts_with("keyring:")
                && reference.chars().count() <= 512
                && !reference.chars().any(char::is_whitespace)
                && !reference.chars().any(char::is_control)
        })
        && object.keys().all(|key| {
            matches!(
                key.as_str(),
                "credentialRef" | "provider" | "accountHint" | "updatedAtMs"
            )
        })
}

fn same_memory_slot(left: &MemoryRecord, right: &MemoryRecord) -> bool {
    left.tier == right.tier
        && left.subject == right.subject
        && left.predicate == right.predicate
        && match left.tier {
            MemoryTier::Working => left.mission_id == right.mission_id,
            MemoryTier::DurableFact => true,
        }
}

fn context_rank(tier: MemoryTier) -> u8 {
    match tier {
        MemoryTier::Working => 0,
        MemoryTier::DurableFact => 1,
    }
}

fn required_text(value: String, label: &str) -> Result<String, MemoryAgentError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(MemoryAgentError::new(format!("{label} must not be empty")));
    }
    if value.chars().count() > 512 {
        return Err(MemoryAgentError::new(format!("{label} is too long")));
    }
    Ok(value.to_string())
}

fn optional_text(value: Option<String>, label: &str) -> Result<Option<String>, MemoryAgentError> {
    value.map(|value| required_text(value, label)).transpose()
}

fn normalized_ids(values: Vec<String>, label: &str) -> Result<Vec<String>, MemoryAgentError> {
    let mut values = values
        .into_iter()
        .map(|value| required_text(value, label))
        .collect::<Result<Vec<_>, _>>()?;
    values.sort();
    values.dedup();
    Ok(values)
}

fn normalized_filter(
    values: Vec<String>,
    label: &str,
) -> Result<BTreeSet<String>, MemoryAgentError> {
    normalized_ids(values, label).map(|values| values.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remember_request(
        memory_id: &str,
        tier: MemoryTier,
        value: Value,
        mission_id: Option<&str>,
        observed_at_ms: i64,
    ) -> RememberMemoryRequest {
        RememberMemoryRequest {
            memory_id: memory_id.to_string(),
            tier,
            subject: "user.preference".to_string(),
            predicate: "response_language".to_string(),
            value,
            confidence: 1.0,
            source_actor_id: "actor:user".to_string(),
            evidence_event_ids: vec![format!("event:{memory_id}")],
            observed_at_ms,
            recorded_at_ms: observed_at_ms,
            mission_id: mission_id.map(str::to_string),
            run_id: mission_id.map(|_| "run:one".to_string()),
        }
    }

    #[test]
    fn durable_fact_supersedes_without_erasing_evidence_history() {
        let mut agent = MemoryAgent::new();
        agent
            .remember(remember_request(
                "memory:old",
                MemoryTier::DurableFact,
                json!("English"),
                None,
                10,
            ))
            .unwrap();
        let receipt = agent
            .remember(remember_request(
                "memory:new",
                MemoryTier::DurableFact,
                json!("中文"),
                None,
                20,
            ))
            .unwrap();

        assert_eq!(receipt.superseded_memory_id.as_deref(), Some("memory:old"));
        let state = agent.projection();
        assert_eq!(
            state.records["memory:old"].status,
            MemoryRecordStatus::Superseded
        );
        assert_eq!(
            state.records["memory:old"]
                .superseded_by_memory_id
                .as_deref(),
            Some("memory:new")
        );
        assert_eq!(
            state.records["memory:new"].supersedes_memory_id.as_deref(),
            Some("memory:old")
        );

        let context = agent
            .compile_context(CompileMemoryContextRequest::default(), 30)
            .unwrap();
        assert_eq!(context.status, MemoryContextStatus::Ready);
        assert_eq!(context.items.len(), 1);
        assert_eq!(context.items[0].memory_id, "memory:new");
        assert_eq!(context.evidence_event_ids, vec!["event:memory:new"]);

        let stale = agent.remember(remember_request(
            "memory:stale",
            MemoryTier::DurableFact,
            json!("stale"),
            None,
            15,
        ));
        assert!(stale.is_err());
        assert_eq!(
            agent
                .compile_context(CompileMemoryContextRequest::default(), 31)
                .unwrap()
                .items[0]
                .memory_id,
            "memory:new"
        );
    }

    #[test]
    fn working_memory_is_mission_scoped_without_creating_sessions() {
        let mut agent = MemoryAgent::new();
        agent
            .remember(remember_request(
                "memory:mission-a",
                MemoryTier::Working,
                json!("A"),
                Some("mission:a"),
                10,
            ))
            .unwrap();
        agent
            .remember(remember_request(
                "memory:mission-b",
                MemoryTier::Working,
                json!("B"),
                Some("mission:b"),
                20,
            ))
            .unwrap();

        let context = agent
            .compile_context(
                CompileMemoryContextRequest {
                    mission_id: Some("mission:a".to_string()),
                    ..CompileMemoryContextRequest::default()
                },
                30,
            )
            .unwrap();
        assert_eq!(context.items.len(), 1);
        assert_eq!(context.items[0].memory_id, "memory:mission-a");
        let encoded = serde_json::to_string(&context)
            .unwrap()
            .to_ascii_lowercase();
        assert!(!encoded.contains("session"));
    }

    #[test]
    fn evidence_and_context_budget_are_hard_requirements() {
        let mut agent = MemoryAgent::new();
        let mut invalid = remember_request(
            "memory:no-evidence",
            MemoryTier::DurableFact,
            json!(true),
            None,
            10,
        );
        invalid.evidence_event_ids.clear();
        assert!(agent.remember(invalid).is_err());

        for index in 0..3 {
            let mut request = remember_request(
                &format!("memory:{index}"),
                MemoryTier::DurableFact,
                json!(index),
                None,
                index,
            );
            request.subject = format!("subject:{index}");
            agent.remember(request).unwrap();
        }
        let context = agent
            .compile_context(
                CompileMemoryContextRequest {
                    max_items: 2,
                    ..CompileMemoryContextRequest::default()
                },
                30,
            )
            .unwrap();
        assert_eq!(context.status, MemoryContextStatus::Partial);
        assert_eq!(context.items.len(), 2);
        assert_eq!(context.omitted_count, 1);
    }

    #[test]
    fn credentials_require_a_keyring_reference_instead_of_secret_material() {
        let mut agent = MemoryAgent::new();
        let mut unsafe_request = remember_request(
            "memory:credential",
            MemoryTier::DurableFact,
            json!("plain-text-secret"),
            None,
            1,
        );
        unsafe_request.predicate = "api_key".to_string();
        assert!(agent.remember(unsafe_request).is_err());

        let mut safe_request = remember_request(
            "memory:credential-ref",
            MemoryTier::DurableFact,
            json!({ "credentialRef": "keyring:model:primary", "provider": "system-keyring" }),
            None,
            2,
        );
        safe_request.predicate = "api_key".to_string();
        assert!(agent.remember(safe_request).is_ok());
    }

    #[test]
    fn retract_keeps_record_but_removes_it_from_context() {
        let mut agent = MemoryAgent::new();
        agent
            .remember(remember_request(
                "memory:wrong",
                MemoryTier::DurableFact,
                json!("wrong"),
                None,
                10,
            ))
            .unwrap();
        let receipt = agent
            .retract(RetractMemoryRequest {
                memory_id: "memory:wrong".to_string(),
                reason: "user corrected it".to_string(),
                source_actor_id: "actor:user".to_string(),
                evidence_event_ids: vec!["event:correction".to_string()],
                retracted_at_ms: 20,
            })
            .unwrap();
        assert_eq!(receipt.status, MemoryRecordStatus::Retracted);
        let context = agent
            .compile_context(CompileMemoryContextRequest::default(), 30)
            .unwrap();
        assert_eq!(context.status, MemoryContextStatus::Empty);
        assert_eq!(agent.projection().records.len(), 1);
    }

    #[test]
    fn contracts_expose_typed_inputs_outputs_permissions_and_side_effects() {
        let capabilities = memory_capabilities();
        assert_eq!(capabilities.len(), 3);
        assert!(capabilities
            .iter()
            .all(|capability| capability.input_schema.is_object()));
        assert!(capabilities
            .iter()
            .all(|capability| capability.output_schema.is_object()));
        assert!(
            capabilities
                .iter()
                .find(|capability| capability.capability_id == MEMORY_CONTEXT_CAPABILITY_ID)
                .unwrap()
                .idempotent
        );
    }
}
