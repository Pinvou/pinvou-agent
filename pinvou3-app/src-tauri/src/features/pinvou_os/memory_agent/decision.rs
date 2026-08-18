//! Memory Agent 的细粒度 decision stream 与纯回放器。
//!
//! 每个批次只保存本次改变记录的 post-image 和少量幂等状态，不复制完整记忆投影。
//! 回放只机械应用 delta，不重新执行 Organizer 的业务判断，因此未来策略升级不会改变
//! 已经发生过的历史决策。本模块不负责文件 I/O，也不接 Runtime 或其他 Agent。

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::io::{self, Write};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::domain::{
    MemoryCandidate, MemoryDisputeResolutionReceipt, MemoryMaintenanceReport,
    MemoryOrganizationAction, MemoryOrganizationReceipt, MemoryOrganizerState,
    MemoryRetractionReceipt, OrganizedMemory, ProcessedMemoryCandidate,
    ResolveMemoryDisputeRequest, RetractOrganizedMemoryRequest,
    MAX_MEMORY_CANDIDATE_FINGERPRINT_SAMPLES, MAX_MEMORY_CONFLICTS_PER_RECORD,
    MAX_MEMORY_EVIDENCE_PER_CANDIDATE, MAX_MEMORY_EVIDENCE_PER_RECORD, MAX_MEMORY_IDS_PER_RECEIPT,
    MAX_MEMORY_SUPERSEDED_RECORDS_PER_RECORD, MAX_ORGANIZED_MEMORY_RECORDS,
    MAX_PROCESSED_MEMORY_CANDIDATES, MEMORY_ORGANIZER_SCHEMA_VERSION,
};
use super::organizer::{
    all_record_evidence, fingerprint_bytes, fingerprint_evidence_metadata,
    validate_incremental_projection_state, validate_record, EvidenceDigest, MemoryOrganizer,
    MemoryOrganizerError,
};

pub const ORGANIZED_MEMORY_DECISION_SCHEMA_VERSION: u32 = 1;
/// 记录“由哪一版整理规则作出决定”，与状态形状和 decision 协议版本分开演进。
/// 回放应用的是 post-image，因此接受任意非零 policy 版本；不兼容的 delta 形状必须
/// 提升 decision schema，而不能借 policy 版本静默改变 wire format。
pub const ORGANIZED_MEMORY_POLICY_VERSION: u32 = 1;
pub const ORGANIZED_MEMORY_DECISION_GENESIS_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";
const MAX_DECISION_COMMAND_ID_BYTES: usize = 4_096;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessedMemoryCandidateUpsert {
    pub candidate_id: String,
    pub value: ProcessedMemoryCandidate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrganizedMemoryProjectionDelta {
    pub base_revision: u64,
    pub revision: u64,
    pub record_upserts: Vec<OrganizedMemory>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub processed_candidate_upserts: Vec<ProcessedMemoryCandidateUpsert>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dispute_resolution_upserts: Vec<MemoryDisputeResolutionReceipt>,
    /// `Some` 表示本批次推进维护游标；普通写入保持 `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_maintenance_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum OrganizedMemoryDecisionOperation {
    CandidateOrganized {
        receipt: MemoryOrganizationReceipt,
    },
    MemoryRetracted {
        operation_id: String,
        receipt: MemoryRetractionReceipt,
        affected_memory_ids: Vec<String>,
    },
    DisputeResolved {
        receipt: MemoryDisputeResolutionReceipt,
        affected_memory_ids: Vec<String>,
    },
    MaintenanceAdvanced {
        maintained_at_ms: i64,
        report: MemoryMaintenanceReport,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrganizedMemoryDecisionBatch {
    pub schema_version: u32,
    pub organizer_schema_version: u32,
    pub policy_version: u32,
    pub sequence: u64,
    pub previous_decision_hash: String,
    pub command_id: String,
    pub command_fingerprint: String,
    pub operation: OrganizedMemoryDecisionOperation,
    pub delta: OrganizedMemoryProjectionDelta,
    pub decision_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrganizedMemoryDecisionCheckpoint {
    pub schema_version: u32,
    pub organizer_schema_version: u32,
    pub policy_version: u32,
    pub last_sequence: u64,
    pub last_decision_hash: String,
    pub state: MemoryOrganizerState,
    pub state_hash: String,
    /// 同时绑定 schema、policy、sequence、日志头和 state，防止同 revision 快照串链。
    /// 哈希链需要由可信 checkpoint/head 作为根；它检测损坏，不替代签名或来源认证。
    pub checkpoint_hash: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrganizedMemoryDecisionErrorCode {
    OrganizerRejected,
    UnsupportedSchema,
    SequenceMismatch,
    RevisionMismatch,
    HashMismatch,
    InvalidDelta,
    DuplicateCommand,
    CounterExhausted,
    CheckpointMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrganizedMemoryDecisionError {
    code: OrganizedMemoryDecisionErrorCode,
    message: String,
}

impl OrganizedMemoryDecisionError {
    fn new(code: OrganizedMemoryDecisionErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> OrganizedMemoryDecisionErrorCode {
        self.code
    }
}

impl fmt::Display for OrganizedMemoryDecisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for OrganizedMemoryDecisionError {}

impl From<MemoryOrganizerError> for OrganizedMemoryDecisionError {
    fn from(error: MemoryOrganizerError) -> Self {
        Self::new(
            OrganizedMemoryDecisionErrorCode::OrganizerRejected,
            error.to_string(),
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrganizedMemoryDecisionOutcome<T> {
    pub receipt: T,
    /// 精确幂等回放不产生新 decision，也不推进 sequence。
    pub decision: Option<OrganizedMemoryDecisionBatch>,
}

#[derive(Debug, Clone, Default)]
struct DecisionReplayGuard {
    evidence_metadata: BTreeMap<EvidenceDigest, EvidenceDigest>,
    retraction_operations: BTreeMap<EvidenceDigest, EvidenceDigest>,
}

#[derive(Debug, Default)]
struct DecisionReplayGuardUpdates {
    evidence_metadata: BTreeMap<EvidenceDigest, EvidenceDigest>,
    retraction_operations: BTreeMap<EvidenceDigest, EvidenceDigest>,
}

impl DecisionReplayGuard {
    fn from_state(state: &MemoryOrganizerState) -> Result<Self, OrganizedMemoryDecisionError> {
        let mut guard = Self::default();
        for record in state.records.values() {
            for evidence in all_record_evidence(record) {
                guard.insert_evidence(evidence)?;
            }
            if let Some(retraction) = &record.retraction {
                let operation = fingerprint_bytes(retraction.operation_id.as_bytes());
                let owner = fingerprint_bytes(record.memory_id.as_bytes());
                if guard
                    .retraction_operations
                    .insert(operation, owner)
                    .is_some()
                {
                    return Err(invalid_delta(
                        "retraction operation id is duplicated in replay state",
                    ));
                }
            }
        }
        Ok(guard)
    }

    fn prepare_updates(
        &self,
        records: &[OrganizedMemory],
    ) -> Result<DecisionReplayGuardUpdates, OrganizedMemoryDecisionError> {
        let mut updates = DecisionReplayGuardUpdates::default();
        for record in records {
            for evidence in all_record_evidence(record) {
                let event = fingerprint_bytes(evidence.event_id.as_bytes());
                let metadata = fingerprint_evidence_metadata(evidence);
                if self
                    .evidence_metadata
                    .get(&event)
                    .or_else(|| updates.evidence_metadata.get(&event))
                    .is_some_and(|existing| existing != &metadata)
                {
                    return Err(invalid_delta(
                        "evidence metadata changed during decision replay",
                    ));
                }
                updates.evidence_metadata.entry(event).or_insert(metadata);
            }
            if let Some(retraction) = &record.retraction {
                let operation = fingerprint_bytes(retraction.operation_id.as_bytes());
                let owner = fingerprint_bytes(record.memory_id.as_bytes());
                if self
                    .retraction_operations
                    .get(&operation)
                    .or_else(|| updates.retraction_operations.get(&operation))
                    .is_some_and(|existing| existing != &owner)
                {
                    return Err(invalid_delta(
                        "retraction operation id changed owner during decision replay",
                    ));
                }
                updates
                    .retraction_operations
                    .entry(operation)
                    .or_insert(owner);
            }
        }
        Ok(updates)
    }

    fn apply(&mut self, updates: DecisionReplayGuardUpdates) {
        self.evidence_metadata.extend(updates.evidence_metadata);
        self.retraction_operations
            .extend(updates.retraction_operations);
    }

    fn insert_evidence(
        &mut self,
        evidence: &super::domain::MemoryEvidence,
    ) -> Result<(), OrganizedMemoryDecisionError> {
        let event = fingerprint_bytes(evidence.event_id.as_bytes());
        let metadata = fingerprint_evidence_metadata(evidence);
        if self
            .evidence_metadata
            .insert(event, metadata)
            .is_some_and(|existing| existing != metadata)
        {
            return Err(invalid_delta(
                "evidence metadata is inconsistent in replay state",
            ));
        }
        Ok(())
    }
}

/// 单写者语义核心；这里不自行访问磁盘。
///
/// 接入持久化时，适配器必须在同一写锁内完成“生成 batch → 原子追加并同步”，成功前
/// 不得暴露 tentative 状态。若追加失败，应丢弃/隔离本实例并从最后耐久 checkpoint +
/// tail 重建，不能让下一条 decision 接在未落盘的 hash 后面。
#[derive(Debug, Clone)]
pub struct OrganizedMemoryDecisionEngine {
    organizer: MemoryOrganizer,
    last_sequence: u64,
    last_decision_hash: String,
}

impl Default for OrganizedMemoryDecisionEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl OrganizedMemoryDecisionEngine {
    pub fn new() -> Self {
        Self {
            organizer: MemoryOrganizer::new(),
            last_sequence: 0,
            last_decision_hash: ORGANIZED_MEMORY_DECISION_GENESIS_HASH.to_string(),
        }
    }

    pub fn organizer(&self) -> &MemoryOrganizer {
        &self.organizer
    }

    pub fn last_sequence(&self) -> u64 {
        self.last_sequence
    }

    pub fn last_decision_hash(&self) -> &str {
        &self.last_decision_hash
    }

    pub fn organize(
        &mut self,
        candidate: MemoryCandidate,
    ) -> Result<
        OrganizedMemoryDecisionOutcome<MemoryOrganizationReceipt>,
        OrganizedMemoryDecisionError,
    > {
        self.ensure_write_capacity()?;
        let base_revision = self.organizer.state().revision;
        let receipt = self.organizer.organize(candidate)?;
        if receipt.revision == base_revision {
            debug_assert!(self.organizer.last_changed_record_ids().is_empty());
            return Ok(OrganizedMemoryDecisionOutcome {
                receipt,
                decision: None,
            });
        }
        let processed = self
            .organizer
            .state()
            .processed_candidates
            .get(&receipt.candidate_id)
            .expect("successful candidate decision must persist idempotency metadata")
            .clone();
        let command_id = format!("candidate:{}", receipt.candidate_id);
        let command_fingerprint = processed.candidate_fingerprint.clone();
        let delta = self.capture_delta(
            base_revision,
            vec![ProcessedMemoryCandidateUpsert {
                candidate_id: receipt.candidate_id.clone(),
                value: processed,
            }],
            Vec::new(),
            None,
        );
        let decision = self.commit_batch(
            command_id,
            command_fingerprint,
            OrganizedMemoryDecisionOperation::CandidateOrganized {
                receipt: receipt.clone(),
            },
            delta,
        );
        Ok(OrganizedMemoryDecisionOutcome {
            receipt,
            decision: Some(decision),
        })
    }

    pub fn retract(
        &mut self,
        request: RetractOrganizedMemoryRequest,
    ) -> Result<OrganizedMemoryDecisionOutcome<MemoryRetractionReceipt>, OrganizedMemoryDecisionError>
    {
        self.ensure_write_capacity()?;
        let operation_id = request.operation_id.clone();
        let base_revision = self.organizer.state().revision;
        let receipt = self.organizer.retract(request)?;
        if receipt.revision == base_revision {
            debug_assert!(!receipt.changed);
            return Ok(OrganizedMemoryDecisionOutcome {
                receipt,
                decision: None,
            });
        }
        let retraction = self
            .organizer
            .record(&receipt.memory_id)
            .and_then(|record| record.retraction.as_ref())
            .expect("successful retraction must persist its normalized operation");
        let affected_memory_ids = self.organizer.last_changed_record_ids().to_vec();
        let delta = self.capture_delta(base_revision, Vec::new(), Vec::new(), None);
        let decision = self.commit_batch(
            format!("retraction:{operation_id}"),
            fingerprint_serializable(retraction),
            OrganizedMemoryDecisionOperation::MemoryRetracted {
                operation_id,
                receipt: receipt.clone(),
                affected_memory_ids,
            },
            delta,
        );
        Ok(OrganizedMemoryDecisionOutcome {
            receipt,
            decision: Some(decision),
        })
    }

    pub fn resolve_dispute(
        &mut self,
        request: ResolveMemoryDisputeRequest,
    ) -> Result<
        OrganizedMemoryDecisionOutcome<MemoryDisputeResolutionReceipt>,
        OrganizedMemoryDecisionError,
    > {
        self.ensure_write_capacity()?;
        let base_revision = self.organizer.state().revision;
        let receipt = self.organizer.resolve_dispute(request)?;
        if receipt.revision == base_revision {
            debug_assert!(!receipt.changed);
            return Ok(OrganizedMemoryDecisionOutcome {
                receipt,
                decision: None,
            });
        }
        let affected_memory_ids = self.organizer.last_changed_record_ids().to_vec();
        let delta = self.capture_delta(base_revision, Vec::new(), vec![receipt.clone()], None);
        let decision = self.commit_batch(
            format!("resolution:{}", receipt.operation_id),
            receipt.request_fingerprint.clone(),
            OrganizedMemoryDecisionOperation::DisputeResolved {
                receipt: receipt.clone(),
                affected_memory_ids,
            },
            delta,
        );
        Ok(OrganizedMemoryDecisionOutcome {
            receipt,
            decision: Some(decision),
        })
    }

    pub fn maintain(
        &mut self,
        now_ms: i64,
    ) -> Result<OrganizedMemoryDecisionOutcome<MemoryMaintenanceReport>, OrganizedMemoryDecisionError>
    {
        self.ensure_write_capacity()?;
        let base_revision = self.organizer.state().revision;
        let report = self.organizer.maintain(now_ms)?;
        if report.revision == base_revision {
            return Ok(OrganizedMemoryDecisionOutcome {
                receipt: report,
                decision: None,
            });
        }
        let delta = self.capture_delta(base_revision, Vec::new(), Vec::new(), Some(now_ms));
        let decision = self.commit_batch(
            format!("maintenance:{now_ms}"),
            fingerprint_serializable(&now_ms),
            OrganizedMemoryDecisionOperation::MaintenanceAdvanced {
                maintained_at_ms: now_ms,
                report: report.clone(),
            },
            delta,
        );
        Ok(OrganizedMemoryDecisionOutcome {
            receipt: report,
            decision: Some(decision),
        })
    }

    pub fn checkpoint(&self) -> OrganizedMemoryDecisionCheckpoint {
        let state = self.organizer.export_state();
        let mut checkpoint = OrganizedMemoryDecisionCheckpoint {
            schema_version: ORGANIZED_MEMORY_DECISION_SCHEMA_VERSION,
            organizer_schema_version: MEMORY_ORGANIZER_SCHEMA_VERSION,
            policy_version: ORGANIZED_MEMORY_POLICY_VERSION,
            last_sequence: self.last_sequence,
            last_decision_hash: self.last_decision_hash.clone(),
            state_hash: fingerprint_serializable(&state),
            state,
            checkpoint_hash: String::new(),
        };
        checkpoint.checkpoint_hash = checkpoint_hash(&checkpoint);
        checkpoint
    }

    pub fn replay(
        decisions: impl IntoIterator<Item = OrganizedMemoryDecisionBatch>,
    ) -> Result<Self, OrganizedMemoryDecisionError> {
        Self::replay_from_state(
            MemoryOrganizerState::default(),
            0,
            ORGANIZED_MEMORY_DECISION_GENESIS_HASH.to_string(),
            decisions,
        )
    }

    pub fn from_checkpoint(
        checkpoint: OrganizedMemoryDecisionCheckpoint,
        tail: impl IntoIterator<Item = OrganizedMemoryDecisionBatch>,
    ) -> Result<Self, OrganizedMemoryDecisionError> {
        if checkpoint.schema_version != ORGANIZED_MEMORY_DECISION_SCHEMA_VERSION
            || checkpoint.organizer_schema_version != MEMORY_ORGANIZER_SCHEMA_VERSION
            || checkpoint.policy_version == 0
        {
            return Err(OrganizedMemoryDecisionError::new(
                OrganizedMemoryDecisionErrorCode::UnsupportedSchema,
                "unsupported organized memory checkpoint schema",
            ));
        }
        if !is_digest(&checkpoint.last_decision_hash)
            || !is_digest(&checkpoint.checkpoint_hash)
            || checkpoint.state_hash != fingerprint_serializable(&checkpoint.state)
            || checkpoint.checkpoint_hash != checkpoint_hash(&checkpoint)
            || checkpoint.last_sequence != checkpoint.state.revision
            || (checkpoint.last_sequence == 0
                && checkpoint.last_decision_hash != ORGANIZED_MEMORY_DECISION_GENESIS_HASH)
            || (checkpoint.last_sequence > 0
                && checkpoint.last_decision_hash == ORGANIZED_MEMORY_DECISION_GENESIS_HASH)
        {
            return Err(OrganizedMemoryDecisionError::new(
                OrganizedMemoryDecisionErrorCode::CheckpointMismatch,
                "organized memory checkpoint integrity check failed",
            ));
        }
        // 先完整验证一次 checkpoint；tail 只应用有界 delta，最后再验证一次结果。
        let validated = MemoryOrganizer::from_state(checkpoint.state)?;
        let mut tail = tail.into_iter();
        let Some(first) = tail.next() else {
            return Ok(Self {
                organizer: validated,
                last_sequence: checkpoint.last_sequence,
                last_decision_hash: checkpoint.last_decision_hash,
            });
        };
        Self::replay_from_state(
            validated.into_state(),
            checkpoint.last_sequence,
            checkpoint.last_decision_hash,
            std::iter::once(first).chain(tail),
        )
    }

    fn replay_from_state(
        mut state: MemoryOrganizerState,
        mut last_sequence: u64,
        mut last_decision_hash: String,
        decisions: impl IntoIterator<Item = OrganizedMemoryDecisionBatch>,
    ) -> Result<Self, OrganizedMemoryDecisionError> {
        if state.revision != last_sequence {
            return Err(OrganizedMemoryDecisionError::new(
                OrganizedMemoryDecisionErrorCode::CheckpointMismatch,
                "organized memory state revision does not match its decision sequence",
            ));
        }
        let mut replay_guard = DecisionReplayGuard::from_state(&state)?;
        for decision in decisions {
            apply_decision_batch(
                &mut state,
                &mut replay_guard,
                &decision,
                last_sequence,
                &last_decision_hash,
            )?;
            last_sequence = decision.sequence;
            last_decision_hash = decision.decision_hash;
        }
        let organizer = MemoryOrganizer::from_state(state)?;
        Ok(Self {
            organizer,
            last_sequence,
            last_decision_hash,
        })
    }

    fn ensure_write_capacity(&self) -> Result<(), OrganizedMemoryDecisionError> {
        if self.last_sequence == u64::MAX || self.organizer.state().revision == u64::MAX {
            return Err(OrganizedMemoryDecisionError::new(
                OrganizedMemoryDecisionErrorCode::CounterExhausted,
                "organized memory decision counter is exhausted",
            ));
        }
        Ok(())
    }

    fn capture_delta(
        &self,
        base_revision: u64,
        mut processed_candidate_upserts: Vec<ProcessedMemoryCandidateUpsert>,
        mut dispute_resolution_upserts: Vec<MemoryDisputeResolutionReceipt>,
        last_maintenance_at_ms: Option<i64>,
    ) -> OrganizedMemoryProjectionDelta {
        let mut record_upserts = self
            .organizer
            .last_changed_record_ids()
            .iter()
            .map(|memory_id| {
                self.organizer
                    .record(memory_id)
                    .expect("changed memory id must exist after an organizer write")
                    .clone()
            })
            .collect::<Vec<_>>();
        record_upserts.sort_by(|left, right| left.memory_id.cmp(&right.memory_id));
        processed_candidate_upserts
            .sort_by(|left, right| left.candidate_id.cmp(&right.candidate_id));
        dispute_resolution_upserts
            .sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
        debug_assert!(record_upserts.len() <= MAX_MEMORY_IDS_PER_RECEIPT);
        OrganizedMemoryProjectionDelta {
            base_revision,
            revision: self.organizer.state().revision,
            record_upserts,
            processed_candidate_upserts,
            dispute_resolution_upserts,
            last_maintenance_at_ms,
        }
    }

    fn commit_batch(
        &mut self,
        command_id: String,
        command_fingerprint: String,
        operation: OrganizedMemoryDecisionOperation,
        delta: OrganizedMemoryProjectionDelta,
    ) -> OrganizedMemoryDecisionBatch {
        let sequence = self
            .last_sequence
            .checked_add(1)
            .expect("write capacity was checked before organizer mutation");
        let mut decision = OrganizedMemoryDecisionBatch {
            schema_version: ORGANIZED_MEMORY_DECISION_SCHEMA_VERSION,
            organizer_schema_version: MEMORY_ORGANIZER_SCHEMA_VERSION,
            policy_version: ORGANIZED_MEMORY_POLICY_VERSION,
            sequence,
            previous_decision_hash: self.last_decision_hash.clone(),
            command_id,
            command_fingerprint,
            operation,
            delta,
            decision_hash: String::new(),
        };
        decision.decision_hash = decision_hash(&decision);
        self.last_sequence = sequence;
        self.last_decision_hash = decision.decision_hash.clone();
        decision
    }
}

fn apply_decision_batch(
    state: &mut MemoryOrganizerState,
    replay_guard: &mut DecisionReplayGuard,
    decision: &OrganizedMemoryDecisionBatch,
    last_sequence: u64,
    last_decision_hash: &str,
) -> Result<(), OrganizedMemoryDecisionError> {
    if decision.schema_version != ORGANIZED_MEMORY_DECISION_SCHEMA_VERSION
        || decision.organizer_schema_version != MEMORY_ORGANIZER_SCHEMA_VERSION
        || decision.policy_version == 0
    {
        return Err(OrganizedMemoryDecisionError::new(
            OrganizedMemoryDecisionErrorCode::UnsupportedSchema,
            "unsupported organized memory decision schema",
        ));
    }
    preflight_decision_envelope(decision)?;
    if last_sequence.checked_add(1) != Some(decision.sequence) {
        return Err(OrganizedMemoryDecisionError::new(
            OrganizedMemoryDecisionErrorCode::SequenceMismatch,
            "organized memory decision sequence is not continuous",
        ));
    }
    if decision.previous_decision_hash != last_decision_hash
        || !is_digest(&decision.decision_hash)
        || decision.decision_hash != decision_hash(decision)
    {
        return Err(OrganizedMemoryDecisionError::new(
            OrganizedMemoryDecisionErrorCode::HashMismatch,
            "organized memory decision hash chain is invalid",
        ));
    }
    let expected_revision = state.revision.checked_add(1).ok_or_else(|| {
        OrganizedMemoryDecisionError::new(
            OrganizedMemoryDecisionErrorCode::CounterExhausted,
            "organized memory revision is exhausted",
        )
    })?;
    if decision.delta.base_revision != state.revision
        || decision.delta.revision != expected_revision
    {
        return Err(OrganizedMemoryDecisionError::new(
            OrganizedMemoryDecisionErrorCode::RevisionMismatch,
            "organized memory decision revision is not continuous",
        ));
    }
    validate_delta(decision)?;
    ensure_command_is_new(state, replay_guard, decision)?;
    ensure_projection_capacity(state, decision)?;
    let impacted_record_ids = impacted_record_ids(state, &decision.delta.record_upserts);
    let guard_updates = replay_guard.prepare_updates(&decision.delta.record_upserts)?;
    for record in &decision.delta.record_upserts {
        state
            .records
            .insert(record.memory_id.clone(), record.clone());
    }
    for upsert in &decision.delta.processed_candidate_upserts {
        state
            .processed_candidates
            .insert(upsert.candidate_id.clone(), upsert.value.clone());
    }
    for receipt in &decision.delta.dispute_resolution_upserts {
        state
            .dispute_resolutions
            .insert(receipt.operation_id.clone(), receipt.clone());
    }
    if let Some(last_maintenance_at_ms) = decision.delta.last_maintenance_at_ms {
        state.last_maintenance_at_ms = Some(last_maintenance_at_ms);
    }
    state.revision = decision.delta.revision;
    let processed_candidate_ids = decision
        .delta
        .processed_candidate_upserts
        .iter()
        .map(|upsert| upsert.candidate_id.clone())
        .collect::<Vec<_>>();
    let resolution_operation_ids = decision
        .delta
        .dispute_resolution_upserts
        .iter()
        .map(|receipt| receipt.operation_id.clone())
        .collect::<Vec<_>>();
    validate_incremental_projection_state(
        state,
        &impacted_record_ids,
        &processed_candidate_ids,
        &resolution_operation_ids,
    )?;
    replay_guard.apply(guard_updates);
    Ok(())
}

fn preflight_decision_envelope(
    decision: &OrganizedMemoryDecisionBatch,
) -> Result<(), OrganizedMemoryDecisionError> {
    if decision.command_id.is_empty()
        || decision.command_id.len() > MAX_DECISION_COMMAND_ID_BYTES
        || !is_digest(&decision.previous_decision_hash)
        || !is_digest(&decision.command_fingerprint)
        || !is_digest(&decision.decision_hash)
        || decision.delta.record_upserts.len() > MAX_MEMORY_IDS_PER_RECEIPT
        || decision.delta.processed_candidate_upserts.len() > 1
        || decision.delta.dispute_resolution_upserts.len() > 1
    {
        return Err(invalid_delta(
            "organized memory decision exceeds its envelope limits",
        ));
    }
    for record in &decision.delta.record_upserts {
        let evidence_count = record
            .supporting_evidence
            .len()
            .saturating_add(record.contradicting_evidence.len());
        if evidence_count > MAX_MEMORY_EVIDENCE_PER_RECORD
            || record.retraction.as_ref().is_some_and(|retraction| {
                retraction.evidence.len() > MAX_MEMORY_EVIDENCE_PER_CANDIDATE
            })
            || record.conflicts_with_memory_ids.len() > MAX_MEMORY_CONFLICTS_PER_RECORD
            || record.supersedes_memory_ids.len() > MAX_MEMORY_SUPERSEDED_RECORDS_PER_RECORD
            || record.candidate_fingerprint_samples.len() > MAX_MEMORY_CANDIDATE_FINGERPRINT_SAMPLES
        {
            return Err(invalid_delta(
                "organized memory record exceeds a decision envelope limit",
            ));
        }
        validate_record(record)?;
    }
    match &decision.operation {
        OrganizedMemoryDecisionOperation::CandidateOrganized { receipt } => {
            if receipt.affected_memory_ids.len() > MAX_MEMORY_IDS_PER_RECEIPT {
                return Err(invalid_delta("candidate receipt is too large"));
            }
        }
        OrganizedMemoryDecisionOperation::MemoryRetracted {
            affected_memory_ids,
            ..
        } => {
            if affected_memory_ids.len() > MAX_MEMORY_IDS_PER_RECEIPT {
                return Err(invalid_delta("retraction receipt is too large"));
            }
        }
        OrganizedMemoryDecisionOperation::DisputeResolved {
            receipt,
            affected_memory_ids,
        } => {
            if affected_memory_ids.len() > MAX_MEMORY_IDS_PER_RECEIPT
                || receipt.superseded_memory_ids.len() > MAX_MEMORY_CONFLICTS_PER_RECORD
            {
                return Err(invalid_delta("dispute receipt is too large"));
            }
        }
        OrganizedMemoryDecisionOperation::MaintenanceAdvanced { report, .. } => {
            if report.expired_memory_ids.len() > MAX_ORGANIZED_MEMORY_RECORDS {
                return Err(invalid_delta("maintenance receipt is too large"));
            }
        }
    }
    Ok(())
}

fn ensure_command_is_new(
    state: &MemoryOrganizerState,
    replay_guard: &DecisionReplayGuard,
    decision: &OrganizedMemoryDecisionBatch,
) -> Result<(), OrganizedMemoryDecisionError> {
    let already_applied = match &decision.operation {
        OrganizedMemoryDecisionOperation::CandidateOrganized { receipt } => state
            .processed_candidates
            .contains_key(&receipt.candidate_id),
        OrganizedMemoryDecisionOperation::MemoryRetracted { operation_id, .. } => replay_guard
            .retraction_operations
            .contains_key(&fingerprint_bytes(operation_id.as_bytes())),
        OrganizedMemoryDecisionOperation::DisputeResolved { receipt, .. } => state
            .dispute_resolutions
            .contains_key(&receipt.operation_id),
        OrganizedMemoryDecisionOperation::MaintenanceAdvanced {
            maintained_at_ms, ..
        } => state
            .last_maintenance_at_ms
            .is_some_and(|previous| *maintained_at_ms <= previous),
    };
    if already_applied {
        return Err(OrganizedMemoryDecisionError::new(
            OrganizedMemoryDecisionErrorCode::DuplicateCommand,
            "organized memory decision repeats an already applied command",
        ));
    }
    Ok(())
}

fn ensure_projection_capacity(
    state: &MemoryOrganizerState,
    decision: &OrganizedMemoryDecisionBatch,
) -> Result<(), OrganizedMemoryDecisionError> {
    let new_record_count = decision
        .delta
        .record_upserts
        .iter()
        .filter(|record| !state.records.contains_key(&record.memory_id))
        .count();
    let new_candidate_count = decision
        .delta
        .processed_candidate_upserts
        .iter()
        .filter(|upsert| {
            !state
                .processed_candidates
                .contains_key(&upsert.candidate_id)
        })
        .count();
    let new_resolution_count = decision
        .delta
        .dispute_resolution_upserts
        .iter()
        .filter(|receipt| {
            !state
                .dispute_resolutions
                .contains_key(&receipt.operation_id)
        })
        .count();
    if state.records.len().saturating_add(new_record_count) > MAX_ORGANIZED_MEMORY_RECORDS
        || state
            .processed_candidates
            .len()
            .saturating_add(new_candidate_count)
            > MAX_PROCESSED_MEMORY_CANDIDATES
        || state
            .dispute_resolutions
            .len()
            .saturating_add(new_resolution_count)
            > MAX_PROCESSED_MEMORY_CANDIDATES
    {
        return Err(invalid_delta(
            "organized memory decision would exceed projection capacity",
        ));
    }
    Ok(())
}

fn impacted_record_ids(
    state: &MemoryOrganizerState,
    record_upserts: &[OrganizedMemory],
) -> BTreeSet<String> {
    let mut impacted = BTreeSet::new();
    for post_image in record_upserts {
        impacted.insert(post_image.memory_id.clone());
        if let Some(previous) = state.records.get(&post_image.memory_id) {
            extend_with_record_links(&mut impacted, previous);
        }
        extend_with_record_links(&mut impacted, post_image);
    }
    impacted
}

fn extend_with_record_links(memory_ids: &mut BTreeSet<String>, record: &OrganizedMemory) {
    memory_ids.extend(record.conflicts_with_memory_ids.iter().cloned());
    memory_ids.extend(record.supersedes_memory_ids.iter().cloned());
    memory_ids.extend(record.superseded_by_memory_id.iter().cloned());
}

fn validate_delta(
    decision: &OrganizedMemoryDecisionBatch,
) -> Result<(), OrganizedMemoryDecisionError> {
    if decision.command_id.trim().is_empty()
        || !is_digest(&decision.command_fingerprint)
        || decision.delta.record_upserts.len() > MAX_MEMORY_IDS_PER_RECEIPT
        || !has_unique_record_ids(&decision.delta.record_upserts)
        || !has_unique_processed_ids(&decision.delta.processed_candidate_upserts)
        || !has_unique_resolution_ids(&decision.delta.dispute_resolution_upserts)
    {
        return Err(invalid_delta(
            "organized memory decision has malformed delta metadata",
        ));
    }
    let record_ids = decision
        .delta
        .record_upserts
        .iter()
        .map(|record| record.memory_id.clone())
        .collect::<Vec<_>>();
    match &decision.operation {
        OrganizedMemoryDecisionOperation::CandidateOrganized { receipt } => {
            let Some(processed) = decision.delta.processed_candidate_upserts.first() else {
                return Err(invalid_delta(
                    "candidate decision is missing idempotency state",
                ));
            };
            if receipt.revision != decision.delta.revision
                || decision.command_id != format!("candidate:{}", receipt.candidate_id)
                || decision.delta.processed_candidate_upserts.len() != 1
                || processed.candidate_id != receipt.candidate_id
                || processed.value.applied_revision != receipt.revision
                || processed.value.candidate_fingerprint != decision.command_fingerprint
                || processed.value.memory_ids != receipt.affected_memory_ids
                || (receipt.action == MemoryOrganizationAction::IgnoredDuplicate
                    && !record_ids.is_empty())
                || (receipt.action != MemoryOrganizationAction::IgnoredDuplicate
                    && record_ids != receipt.affected_memory_ids)
                || !decision.delta.dispute_resolution_upserts.is_empty()
                || decision.delta.last_maintenance_at_ms.is_some()
            {
                return Err(invalid_delta(
                    "candidate decision delta does not match its receipt",
                ));
            }
        }
        OrganizedMemoryDecisionOperation::MemoryRetracted {
            operation_id,
            receipt,
            affected_memory_ids,
        } => {
            let retraction_fingerprint = decision
                .delta
                .record_upserts
                .iter()
                .find(|record| record.memory_id == receipt.memory_id)
                .and_then(|record| record.retraction.as_ref())
                .map(fingerprint_serializable);
            let retracted_record = decision
                .delta
                .record_upserts
                .iter()
                .find(|record| record.memory_id == receipt.memory_id);
            if !receipt.changed
                || receipt.revision != decision.delta.revision
                || decision.command_id != format!("retraction:{operation_id}")
                || record_ids != *affected_memory_ids
                || retraction_fingerprint.as_deref() != Some(&decision.command_fingerprint)
                || retracted_record.is_none_or(|record| {
                    record.status != super::domain::OrganizedMemoryStatus::Retracted
                        || record
                            .retraction
                            .as_ref()
                            .is_none_or(|retraction| retraction.operation_id != *operation_id)
                })
                || !decision.delta.processed_candidate_upserts.is_empty()
                || !decision.delta.dispute_resolution_upserts.is_empty()
                || decision.delta.last_maintenance_at_ms.is_some()
            {
                return Err(invalid_delta("retraction decision delta is inconsistent"));
            }
        }
        OrganizedMemoryDecisionOperation::DisputeResolved {
            receipt,
            affected_memory_ids,
        } => {
            let winner = decision
                .delta
                .record_upserts
                .iter()
                .find(|record| record.memory_id == receipt.winner_memory_id);
            let every_loser_is_reflected = receipt.superseded_memory_ids.iter().all(|memory_id| {
                decision
                    .delta
                    .record_upserts
                    .iter()
                    .find(|record| record.memory_id == *memory_id)
                    .is_some_and(|record| {
                        record.status == super::domain::OrganizedMemoryStatus::Superseded
                            && record.superseded_by_memory_id.as_deref()
                                == Some(receipt.winner_memory_id.as_str())
                    })
            });
            if !receipt.changed
                || receipt.revision != decision.delta.revision
                || decision.command_id != format!("resolution:{}", receipt.operation_id)
                || decision.command_fingerprint != receipt.request_fingerprint
                || record_ids != *affected_memory_ids
                || winner.is_none_or(|record| {
                    record.status != super::domain::OrganizedMemoryStatus::Confirmed
                        || receipt
                            .superseded_memory_ids
                            .iter()
                            .any(|memory_id| !record.supersedes_memory_ids.contains(memory_id))
                })
                || !every_loser_is_reflected
                || decision.delta.dispute_resolution_upserts.as_slice()
                    != std::slice::from_ref(receipt)
                || !decision.delta.processed_candidate_upserts.is_empty()
                || decision.delta.last_maintenance_at_ms.is_some()
            {
                return Err(invalid_delta("dispute decision delta is inconsistent"));
            }
        }
        OrganizedMemoryDecisionOperation::MaintenanceAdvanced {
            maintained_at_ms,
            report,
        } => {
            if report.revision != decision.delta.revision
                || decision.command_id != format!("maintenance:{maintained_at_ms}")
                || decision.command_fingerprint != fingerprint_serializable(maintained_at_ms)
                || !decision.delta.record_upserts.is_empty()
                || !decision.delta.processed_candidate_upserts.is_empty()
                || !decision.delta.dispute_resolution_upserts.is_empty()
                || decision.delta.last_maintenance_at_ms != Some(*maintained_at_ms)
            {
                return Err(invalid_delta("maintenance decision delta is inconsistent"));
            }
        }
    }
    Ok(())
}

fn has_unique_record_ids(records: &[OrganizedMemory]) -> bool {
    records
        .iter()
        .map(|record| record.memory_id.as_str())
        .collect::<BTreeSet<_>>()
        .len()
        == records.len()
}

fn has_unique_processed_ids(upserts: &[ProcessedMemoryCandidateUpsert]) -> bool {
    upserts
        .iter()
        .map(|upsert| upsert.candidate_id.as_str())
        .collect::<BTreeSet<_>>()
        .len()
        == upserts.len()
}

fn has_unique_resolution_ids(receipts: &[MemoryDisputeResolutionReceipt]) -> bool {
    receipts
        .iter()
        .map(|receipt| receipt.operation_id.as_str())
        .collect::<BTreeSet<_>>()
        .len()
        == receipts.len()
}

fn invalid_delta(message: &str) -> OrganizedMemoryDecisionError {
    OrganizedMemoryDecisionError::new(OrganizedMemoryDecisionErrorCode::InvalidDelta, message)
}

fn decision_hash(decision: &OrganizedMemoryDecisionBatch) -> String {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct HashInput<'a> {
        domain: &'static str,
        schema_version: u32,
        organizer_schema_version: u32,
        policy_version: u32,
        sequence: u64,
        previous_decision_hash: &'a str,
        command_id: &'a str,
        command_fingerprint: &'a str,
        operation: &'a OrganizedMemoryDecisionOperation,
        delta: &'a OrganizedMemoryProjectionDelta,
    }

    fingerprint_serializable(&HashInput {
        domain: "pinvou.memory.decision.v1",
        schema_version: decision.schema_version,
        organizer_schema_version: decision.organizer_schema_version,
        policy_version: decision.policy_version,
        sequence: decision.sequence,
        previous_decision_hash: &decision.previous_decision_hash,
        command_id: &decision.command_id,
        command_fingerprint: &decision.command_fingerprint,
        operation: &decision.operation,
        delta: &decision.delta,
    })
}

fn checkpoint_hash(checkpoint: &OrganizedMemoryDecisionCheckpoint) -> String {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct HashInput<'a> {
        domain: &'static str,
        schema_version: u32,
        organizer_schema_version: u32,
        policy_version: u32,
        last_sequence: u64,
        last_decision_hash: &'a str,
        state: &'a MemoryOrganizerState,
        state_hash: &'a str,
    }

    fingerprint_serializable(&HashInput {
        domain: "pinvou.memory.checkpoint.v1",
        schema_version: checkpoint.schema_version,
        organizer_schema_version: checkpoint.organizer_schema_version,
        policy_version: checkpoint.policy_version,
        last_sequence: checkpoint.last_sequence,
        last_decision_hash: &checkpoint.last_decision_hash,
        state: &checkpoint.state,
        state_hash: &checkpoint.state_hash,
    })
}

fn fingerprint_serializable(value: &impl Serialize) -> String {
    struct DigestWriter(Sha256);

    impl Write for DigestWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let mut writer = DigestWriter(Sha256::new());
    serde_json::to_writer(&mut writer, value)
        .expect("validated memory protocol must serialize into its digest");
    let digest = writer.0.finalize();
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[cfg(test)]
#[path = "decision_tests.rs"]
mod tests;
