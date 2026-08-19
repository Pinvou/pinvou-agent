//! Memory Agent 的轻量、确定性整理内核。
//!
//! 本模块不启动线程、不调用模型、不访问网络或磁盘。未来的异步 Agent 只需要把
//! 事件提取成 [`MemoryCandidate`]，再将本模块产生的细粒度决策写入专用记忆账本。

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::domain::{
    MemoryApplicability, MemoryBatchOutcome, MemoryCandidate, MemoryCandidateIntent,
    MemoryDisputeResolutionReceipt, MemoryEvidence, MemoryEvidenceOrigin, MemoryEvidencePolarity,
    MemoryMaintenanceReport, MemoryOrganizationAction, MemoryOrganizationReceipt,
    MemoryOrganizerState, MemoryRetraction, MemoryRetractionReceipt, OrganizedMemory,
    OrganizedMemoryContextItem, OrganizedMemoryKind, OrganizedMemoryProjection,
    OrganizedMemoryQuery, OrganizedMemoryStatus, ProcessedMemoryCandidate, RejectedMemoryCandidate,
    ResolveMemoryDisputeRequest, RetractOrganizedMemoryRequest, MAX_MEMORY_CANDIDATES_PER_BATCH,
    MAX_MEMORY_CANDIDATE_FINGERPRINT_SAMPLES, MAX_MEMORY_CONFLICTS_PER_RECORD,
    MAX_MEMORY_EVIDENCE_PER_CANDIDATE, MAX_MEMORY_EVIDENCE_PER_RECORD, MAX_MEMORY_IDS_PER_RECEIPT,
    MAX_MEMORY_RECORDS_PER_BASE_SLOT, MAX_MEMORY_SUPERSEDED_RECORDS_PER_RECORD,
    MAX_ORGANIZED_CONTEXT_ITEMS, MAX_ORGANIZED_MEMORY_RECORDS, MAX_ORGANIZED_MEMORY_VALUE_BYTES,
    MAX_PROCESSED_MEMORY_CANDIDATES, MEMORY_ORGANIZER_SCHEMA_VERSION,
};
use super::retrieval::MemoryRetrievalIndex;

const MAX_TEXT_CHARS: usize = 512;
const MAX_ENVIRONMENT_KEYS: usize = 32;
const MAX_QUERY_FILTER_VALUES: usize = 64;
const MAX_QUERY_FOCUS_TERMS: usize = 32;
const MAX_QUERY_FILTER_CHARS: usize = 4_096;
const ORGANIZED_MEMORY_ID_PREFIX: &str = "memory:organized:";
pub(super) type EvidenceDigest = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryOrganizerError {
    message: String,
}

impl MemoryOrganizerError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for MemoryOrganizerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for MemoryOrganizerError {}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct MemorySlotKey {
    kind: OrganizedMemoryKind,
    space_id: String,
    subject: String,
    predicate: String,
}

impl MemorySlotKey {
    fn from_candidate(candidate: &MemoryCandidate) -> Self {
        Self {
            kind: candidate.kind,
            space_id: candidate.applicability.space_id.clone(),
            subject: candidate.subject.clone(),
            predicate: candidate.predicate.clone(),
        }
    }

    fn from_record(record: &OrganizedMemory) -> Self {
        Self {
            kind: record.kind,
            space_id: record.applicability.space_id.clone(),
            subject: record.subject.clone(),
            predicate: record.predicate.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct MemoryProjectionStats {
    index_seed_posting_count: usize,
    index_membership_check_count: usize,
    indexed_candidate_count: usize,
    visible_candidate_count: usize,
    ranked_candidate_count: usize,
    retained_candidate_count: usize,
}

/// 整理状态与非序列化索引分离：状态可以回放，索引可以随时廉价重建。
#[derive(Debug, Clone, Default)]
pub struct MemoryOrganizer {
    state: MemoryOrganizerState,
    slot_index: BTreeMap<MemorySlotKey, BTreeSet<String>>,
    /// 当前投影的结构化 posting，只缩小查询候选集，不进入持久状态。
    retrieval_index: MemoryRetrievalIndex,
    /// 最近一次成功写操作实际改变的记录。它只服务于细粒度 decision delta，
    /// 不进入状态快照，也不参与业务语义。
    last_changed_record_ids: Vec<String>,
    /// 非序列化的热索引；键和值都是固定长度指纹，避免再次复制最长 512 字符的
    /// event id。状态恢复时可从证据引用重建。
    evidence_index: BTreeMap<EvidenceDigest, EvidenceDigest>,
}

impl MemoryOrganizer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_state(state: MemoryOrganizerState) -> Result<Self, MemoryOrganizerError> {
        validate_state(&state)?;
        let mut slot_index = BTreeMap::<MemorySlotKey, BTreeSet<String>>::new();
        let mut evidence_index = BTreeMap::<EvidenceDigest, EvidenceDigest>::new();
        for record in state.records.values() {
            slot_index
                .entry(MemorySlotKey::from_record(record))
                .or_default()
                .insert(record.memory_id.clone());
            for evidence in all_record_evidence(record) {
                evidence_index
                    .entry(fingerprint_bytes(evidence.event_id.as_bytes()))
                    .or_insert_with(|| fingerprint_evidence_metadata(evidence));
            }
        }
        let retrieval_index = MemoryRetrievalIndex::from_records(state.records.values());
        Ok(Self {
            state,
            slot_index,
            retrieval_index,
            last_changed_record_ids: Vec::new(),
            evidence_index,
        })
    }

    pub fn state(&self) -> &MemoryOrganizerState {
        &self.state
    }

    pub fn export_state(&self) -> MemoryOrganizerState {
        self.state.clone()
    }

    pub(super) fn into_state(self) -> MemoryOrganizerState {
        self.state
    }

    pub fn record(&self, memory_id: &str) -> Option<&OrganizedMemory> {
        self.state.records.get(memory_id)
    }

    pub(super) fn last_changed_record_ids(&self) -> &[String] {
        &self.last_changed_record_ids
    }

    /// 整理一个候选。相同 `candidate_id` 的事件回放是幂等的，不增加 revision。
    pub fn organize(
        &mut self,
        candidate: MemoryCandidate,
    ) -> Result<MemoryOrganizationReceipt, MemoryOrganizerError> {
        self.organize_with_optional_evidence_actor_alias(candidate, None)
    }

    /// schema identity 迁移后，旧幂等指纹仍然绑定原始命令字节。只有把规范 actor
    /// 精确反向映射成旧 actor 后的完整候选 SHA-256 命中旧指纹，才把它视作同一
    /// 次重试；任何其他字段变化仍按不同内容拒绝。
    pub fn organize_with_evidence_actor_alias(
        &mut self,
        candidate: MemoryCandidate,
        canonical_actor_id: &str,
        legacy_actor_id: &str,
    ) -> Result<MemoryOrganizationReceipt, MemoryOrganizerError> {
        self.organize_with_optional_evidence_actor_alias(
            candidate,
            Some((canonical_actor_id, legacy_actor_id)),
        )
    }

    fn organize_with_optional_evidence_actor_alias(
        &mut self,
        candidate: MemoryCandidate,
        evidence_actor_alias: Option<(&str, &str)>,
    ) -> Result<MemoryOrganizationReceipt, MemoryOrganizerError> {
        self.last_changed_record_ids.clear();
        let candidate = normalize_candidate(candidate)?;
        let candidate_fingerprint = fingerprint_candidate(&candidate);
        if let Some(previous) = self.state.processed_candidates.get(&candidate.candidate_id) {
            let alias_fingerprint_matches =
                evidence_actor_alias.is_some_and(|(canonical_actor_id, legacy_actor_id)| {
                    let legacy_candidate = rewrite_candidate_evidence_actor(
                        candidate.clone(),
                        canonical_actor_id,
                        legacy_actor_id,
                    );
                    legacy_candidate != candidate
                        && previous.candidate_fingerprint
                            == fingerprint_candidate(&legacy_candidate)
                });
            if previous.candidate_fingerprint != candidate_fingerprint && !alias_fingerprint_matches
            {
                return Err(MemoryOrganizerError::new(format!(
                    "candidate id {} was replayed with different content",
                    candidate.candidate_id
                )));
            }
            return Ok(MemoryOrganizationReceipt {
                candidate_id: candidate.candidate_id,
                revision: self.state.revision,
                action: MemoryOrganizationAction::IgnoredDuplicate,
                affected_memory_ids: previous.memory_ids.clone(),
            });
        }
        if self.state.processed_candidates.len() >= MAX_PROCESSED_MEMORY_CANDIDATES {
            return Err(MemoryOrganizerError::new(format!(
                "processed candidate limit {MAX_PROCESSED_MEMORY_CANDIDATES} reached; a durable checkpoint must advance before pruning idempotency state"
            )));
        }
        self.validate_evidence(&candidate.evidence)?;
        let slot = MemorySlotKey::from_candidate(&candidate);
        // 每个 ActionExperience 都是独立 episode，不参与同槽合并或冲突图。
        let slot_records = if candidate.kind == OrganizedMemoryKind::ActionExperience {
            Vec::new()
        } else {
            self.live_slot_records(&slot)
        };
        let target_id = candidate.target_memory_id.clone();
        let target = target_id
            .as_deref()
            .map(|memory_id| {
                self.state.records.get(memory_id).ok_or_else(|| {
                    MemoryOrganizerError::new(format!("unknown target memory {memory_id}"))
                })
            })
            .transpose()?;
        if let Some(target) = target {
            validate_candidate_target(&candidate, target)?;
        }

        // Replace 必须始终走严格版本规则，不能借“值相同”绕过生效时间检查。
        // ActionExperience 则是不可覆盖的 episode，完全不参与同值合并。
        if candidate.intent == MemoryCandidateIntent::Assert
            && candidate.kind != OrganizedMemoryKind::ActionExperience
        {
            let same_value_ids = slot_records
                .iter()
                .filter_map(|memory_id| self.state.records.get(memory_id))
                .filter(|record| {
                    record.value == candidate.value
                        && record.applicability.environment == candidate.applicability.environment
                        && time_ranges_overlap(&record.applicability, &candidate.applicability)
                })
                .map(|record| record.memory_id.clone())
                .collect::<Vec<_>>();
            if same_value_ids.len() > 1 {
                return Err(MemoryOrganizerError::new(
                    "candidate bridges multiple historical periods; explicit consolidation is required",
                ));
            }
            if let Some(memory_id) = same_value_ids.into_iter().next() {
                return self.merge_candidate(memory_id, candidate, candidate_fingerprint);
            }
        }

        match candidate.intent {
            MemoryCandidateIntent::Replace => {
                let target_id = target_id.expect("validated replacement must have a target");
                self.replace_memory(target_id, candidate, candidate_fingerprint)
            }
            MemoryCandidateIntent::Contradict => {
                let target_id = target_id.expect("validated contradiction must have a target");
                self.create_with_conflicts(candidate, vec![target_id], candidate_fingerprint)
            }
            MemoryCandidateIntent::Assert => {
                let conflicts = slot_records
                    .into_iter()
                    .filter(|memory_id| {
                        self.state.records.get(memory_id).is_some_and(|record| {
                            record.value != candidate.value
                                && time_ranges_overlap(
                                    &record.applicability,
                                    &candidate.applicability,
                                )
                                && environment_scopes_conflict(
                                    &record.applicability.environment,
                                    &candidate.applicability.environment,
                                )
                        })
                    })
                    .collect::<Vec<_>>();
                self.create_with_conflicts(candidate, conflicts, candidate_fingerprint)
            }
        }
    }

    /// 有界批处理采用逐条隔离：一个坏候选不会阻断同批其他候选，也不会回滚已完成项。
    pub fn organize_batch(
        &mut self,
        candidates: Vec<MemoryCandidate>,
    ) -> Result<MemoryBatchOutcome, MemoryOrganizerError> {
        if candidates.len() > MAX_MEMORY_CANDIDATES_PER_BATCH {
            return Err(MemoryOrganizerError::new(format!(
                "memory candidate batch exceeds {MAX_MEMORY_CANDIDATES_PER_BATCH} items"
            )));
        }
        let mut outcome = MemoryBatchOutcome::default();
        for (index, candidate) in candidates.into_iter().enumerate() {
            let fallback_id = if candidate.candidate_id.trim().is_empty() {
                format!("invalid-candidate-{index}")
            } else {
                candidate.candidate_id.trim().to_string()
            };
            match self.organize(candidate) {
                Ok(receipt) => outcome.accepted.push(receipt),
                Err(error) => outcome.rejected.push(RejectedMemoryCandidate {
                    candidate_id: fallback_id,
                    reason: error.to_string(),
                }),
            }
        }
        Ok(outcome)
    }

    /// 撤回不会删除旧记录，也不会自动“复活”它曾替代的更旧版本。
    pub fn retract(
        &mut self,
        request: RetractOrganizedMemoryRequest,
    ) -> Result<MemoryRetractionReceipt, MemoryOrganizerError> {
        self.last_changed_record_ids.clear();
        let request = normalize_retraction(request)?;
        self.validate_evidence(&request.evidence)?;
        let proposed_retraction = MemoryRetraction {
            operation_id: request.operation_id.clone(),
            reason: request.reason.clone(),
            retracted_at_ms: request.retracted_at_ms,
            evidence: request.evidence.clone(),
        };
        if let Some((existing_memory_id, existing)) = self
            .state
            .records
            .values()
            .filter_map(|record| {
                record
                    .retraction
                    .as_ref()
                    .map(|retraction| (record.memory_id.as_str(), retraction))
            })
            .find(|(_, retraction)| retraction.operation_id == request.operation_id)
        {
            if existing_memory_id != request.memory_id || existing != &proposed_retraction {
                return Err(MemoryOrganizerError::new(format!(
                    "retraction operation {} was replayed with different content",
                    request.operation_id
                )));
            }
            return Ok(MemoryRetractionReceipt {
                memory_id: request.memory_id,
                revision: self.state.revision,
                changed: false,
            });
        }
        let record = self.state.records.get(&request.memory_id).ok_or_else(|| {
            MemoryOrganizerError::new(format!("unknown memory {}", request.memory_id))
        })?;
        if record.retraction.is_some() {
            return Err(MemoryOrganizerError::new(format!(
                "memory {} is already retracted",
                request.memory_id
            )));
        }
        if evidence_authority(&request.evidence) < record_authority(record) {
            return Err(MemoryOrganizerError::new(format!(
                "weaker evidence cannot retract memory {}",
                request.memory_id
            )));
        }
        let latest_retraction_input = request
            .evidence
            .iter()
            .map(|evidence| evidence.recorded_at_ms)
            .max()
            .unwrap_or(0)
            .max(record.updated_at_ms);
        if request.retracted_at_ms < latest_retraction_input {
            return Err(MemoryOrganizerError::new(
                "memory retraction cannot predate the memory or its evidence",
            ));
        }
        let slot = MemorySlotKey::from_record(record);
        let record = self
            .state
            .records
            .get_mut(&request.memory_id)
            .expect("validated retraction memory must exist");
        record.status = OrganizedMemoryStatus::Retracted;
        record.updated_at_ms = record.updated_at_ms.max(request.retracted_at_ms);
        record.retraction = Some(proposed_retraction);
        let retracted_memory_id = request.memory_id.clone();
        let mut changed_memory_ids = vec![retracted_memory_id.clone()];
        changed_memory_ids.extend(self.detach_conflicts(&[retracted_memory_id]));
        changed_memory_ids.extend(self.reconcile_slot(&slot));
        changed_memory_ids.sort();
        changed_memory_ids.dedup();
        self.last_changed_record_ids = changed_memory_ids;
        self.index_evidence(&request.evidence);
        self.state.revision = self.state.revision.saturating_add(1);
        Ok(MemoryRetractionReceipt {
            memory_id: request.memory_id,
            revision: self.state.revision,
            changed: true,
        })
    }

    /// 用一份权威证据一次性结束同槽的全部活动争议。输家保留为被替代版本，
    /// winner 成为唯一当前结论；遗漏任一活动冲突方都会拒绝，避免半解决状态。
    pub fn resolve_dispute(
        &mut self,
        request: ResolveMemoryDisputeRequest,
    ) -> Result<MemoryDisputeResolutionReceipt, MemoryOrganizerError> {
        self.resolve_dispute_with_optional_evidence_actor_alias(request, None)
    }

    pub fn resolve_dispute_with_evidence_actor_alias(
        &mut self,
        request: ResolveMemoryDisputeRequest,
        canonical_actor_id: &str,
        legacy_actor_id: &str,
    ) -> Result<MemoryDisputeResolutionReceipt, MemoryOrganizerError> {
        self.resolve_dispute_with_optional_evidence_actor_alias(
            request,
            Some((canonical_actor_id, legacy_actor_id)),
        )
    }

    fn resolve_dispute_with_optional_evidence_actor_alias(
        &mut self,
        request: ResolveMemoryDisputeRequest,
        evidence_actor_alias: Option<(&str, &str)>,
    ) -> Result<MemoryDisputeResolutionReceipt, MemoryOrganizerError> {
        self.last_changed_record_ids.clear();
        let request = normalize_resolution(request)?;
        let request_fingerprint = fingerprint_serializable(&request);
        if let Some(previous) = self.state.dispute_resolutions.get(&request.operation_id) {
            let alias_fingerprint_matches =
                evidence_actor_alias.is_some_and(|(canonical_actor_id, legacy_actor_id)| {
                    let legacy_request = rewrite_resolution_evidence_actor(
                        request.clone(),
                        canonical_actor_id,
                        legacy_actor_id,
                    );
                    legacy_request != request
                        && previous.request_fingerprint == fingerprint_serializable(&legacy_request)
                });
            if previous.request_fingerprint != request_fingerprint && !alias_fingerprint_matches {
                return Err(MemoryOrganizerError::new(format!(
                    "resolution operation {} was replayed with different content",
                    request.operation_id
                )));
            }
            let mut previous = previous.clone();
            previous.changed = false;
            previous.revision = self.state.revision;
            return Ok(previous);
        }
        if self.state.dispute_resolutions.len() >= MAX_PROCESSED_MEMORY_CANDIDATES {
            return Err(MemoryOrganizerError::new(format!(
                "dispute resolution limit {MAX_PROCESSED_MEMORY_CANDIDATES} reached; a durable checkpoint must advance before pruning idempotency state"
            )));
        }
        self.validate_evidence(&request.evidence)?;
        let winner = self
            .state
            .records
            .get(&request.winner_memory_id)
            .ok_or_else(|| {
                MemoryOrganizerError::new(format!(
                    "unknown winner memory {}",
                    request.winner_memory_id
                ))
            })?;
        if !is_live_status(winner.status) {
            return Err(MemoryOrganizerError::new("winner memory is not current"));
        }
        let slot = MemorySlotKey::from_record(winner);
        let requested_losers = request
            .losing_memory_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let active_conflicts = winner
            .conflicts_with_memory_ids
            .iter()
            .filter(|memory_id| {
                self.state
                    .records
                    .get(*memory_id)
                    .is_some_and(|record| is_live_status(record.status))
            })
            .cloned()
            .collect::<BTreeSet<_>>();
        if requested_losers != active_conflicts || requested_losers.is_empty() {
            return Err(MemoryOrganizerError::new(
                "resolution must name every active conflict and no unrelated memory",
            ));
        }
        let maximum_existing_authority =
            std::iter::once(record_authority(winner))
                .chain(requested_losers.iter().filter_map(|memory_id| {
                    self.state.records.get(memory_id).map(record_authority)
                }))
                .max()
                .unwrap_or(0);
        if evidence_authority(&request.evidence) < maximum_existing_authority {
            return Err(MemoryOrganizerError::new(
                "dispute resolution evidence is weaker than an existing claim",
            ));
        }
        ensure_evidence_capacity(winner, &request.evidence)?;
        if winner.supersedes_memory_ids.len() + requested_losers.len()
            > MAX_MEMORY_SUPERSEDED_RECORDS_PER_RECORD
        {
            return Err(MemoryOrganizerError::new(format!(
                "winner would supersede more than {MAX_MEMORY_SUPERSEDED_RECORDS_PER_RECORD} records"
            )));
        }
        let latest_resolution_input = request
            .evidence
            .iter()
            .map(|evidence| evidence.recorded_at_ms)
            .max()
            .unwrap_or(0)
            .max(winner.updated_at_ms);
        if request.resolved_at_ms < latest_resolution_input {
            return Err(MemoryOrganizerError::new(
                "dispute resolution cannot predate the winner or its evidence",
            ));
        }
        // 裁决会清除每个 loser 的全部对称冲突边。先按最坏的实际邻接并集做容量
        // 预检，避免 Organizer 已经改完状态后才发现 decision delta 无法承载。
        let mut resolution_affected_ids = BTreeSet::from([request.winner_memory_id.clone()]);
        for losing_id in &requested_losers {
            let losing = self
                .state
                .records
                .get(losing_id)
                .ok_or_else(|| MemoryOrganizerError::new("resolution loser disappeared"))?;
            if MemorySlotKey::from_record(losing) != slot
                || !losing
                    .conflicts_with_memory_ids
                    .contains(&request.winner_memory_id)
            {
                return Err(MemoryOrganizerError::new(
                    "resolution loser is not a symmetric conflict in the winner slot",
                ));
            }
            if request.resolved_at_ms < losing.updated_at_ms {
                return Err(MemoryOrganizerError::new(
                    "dispute resolution cannot predate a losing memory",
                ));
            }
            resolution_affected_ids.insert(losing_id.clone());
            resolution_affected_ids.extend(losing.conflicts_with_memory_ids.iter().cloned());
        }
        if resolution_affected_ids.len() > MAX_MEMORY_IDS_PER_RECEIPT {
            return Err(MemoryOrganizerError::new(format!(
                "dispute resolution would change more than {MAX_MEMORY_IDS_PER_RECEIPT} memories; split or consolidate the conflict graph first"
            )));
        }

        let resolution_confidence = confidence_from_evidence(&request.evidence, 1.0);
        let mut staged_winner = winner.clone();
        append_evidence(&mut staged_winner, request.evidence.clone());
        staged_winner.confidence =
            combine_confidence(staged_winner.confidence, resolution_confidence);
        if status_from_record(&staged_winner) != OrganizedMemoryStatus::Confirmed {
            return Err(MemoryOrganizerError::new(
                "dispute resolution evidence does not establish a confirmed winner",
            ));
        }
        let winner = self
            .state
            .records
            .get_mut(&request.winner_memory_id)
            .expect("validated winner must exist");
        append_evidence(winner, request.evidence.clone());
        winner.confidence = combine_confidence(winner.confidence, resolution_confidence);
        winner.status = OrganizedMemoryStatus::Confirmed;
        winner.updated_at_ms = winner.updated_at_ms.max(request.resolved_at_ms);
        winner
            .supersedes_memory_ids
            .extend(requested_losers.iter().cloned());

        for losing_id in &requested_losers {
            let losing = self
                .state
                .records
                .get_mut(losing_id)
                .expect("validated resolution loser must exist");
            losing.status = OrganizedMemoryStatus::Superseded;
            losing.superseded_by_memory_id = Some(request.winner_memory_id.clone());
            losing.updated_at_ms = losing.updated_at_ms.max(request.resolved_at_ms);
            if request.resolved_at_ms > losing.applicability.valid_from_ms
                && losing
                    .applicability
                    .valid_until_ms
                    .is_none_or(|until| until > request.resolved_at_ms)
            {
                losing.applicability.valid_until_ms = Some(request.resolved_at_ms);
            }
        }
        let losing_ids = requested_losers.iter().cloned().collect::<Vec<_>>();
        let mut changed_memory_ids = vec![request.winner_memory_id.clone()];
        changed_memory_ids.extend(losing_ids.iter().cloned());
        changed_memory_ids.extend(self.detach_conflicts(&losing_ids));
        changed_memory_ids.extend(self.reconcile_slot(&slot));
        changed_memory_ids.sort();
        changed_memory_ids.dedup();
        debug_assert!(changed_memory_ids.len() <= resolution_affected_ids.len());
        self.last_changed_record_ids = changed_memory_ids;
        debug_assert_eq!(
            self.state.records[&request.winner_memory_id].status,
            OrganizedMemoryStatus::Confirmed
        );
        self.index_evidence(&request.evidence);
        self.state.revision = self.state.revision.saturating_add(1);
        let receipt = MemoryDisputeResolutionReceipt {
            operation_id: request.operation_id.clone(),
            request_fingerprint,
            winner_memory_id: request.winner_memory_id,
            superseded_memory_ids: losing_ids,
            revision: self.state.revision,
            changed: true,
        };
        self.state
            .dispute_resolutions
            .insert(request.operation_id, receipt.clone());
        Ok(receipt)
    }

    /// 维护只推进“当前时间已扫描到哪里”的游标。过期是有效时间的派生结果，不
    /// 破坏性改写记录状态，因此未来回放历史时不会丢失当时有效的事实。
    pub fn maintain(
        &mut self,
        now_ms: i64,
    ) -> Result<MemoryMaintenanceReport, MemoryOrganizerError> {
        self.last_changed_record_ids.clear();
        if now_ms < 0 {
            return Err(MemoryOrganizerError::new(
                "memory maintenance timestamp must be non-negative",
            ));
        }
        if self
            .state
            .last_maintenance_at_ms
            .is_some_and(|previous| now_ms < previous)
        {
            return Err(MemoryOrganizerError::new(
                "memory maintenance timestamp must be monotonic",
            ));
        }
        let previous = self.state.last_maintenance_at_ms.unwrap_or(-1);
        let mut expired_memory_ids = self
            .state
            .records
            .values()
            .filter(|record| is_live_status(record.status))
            .filter(|record| {
                record
                    .applicability
                    .valid_until_ms
                    .is_some_and(|until| previous < until && until <= now_ms)
            })
            .map(|record| record.memory_id.clone())
            .collect::<Vec<_>>();
        expired_memory_ids.sort();
        if self.state.last_maintenance_at_ms != Some(now_ms) {
            self.state.last_maintenance_at_ms = Some(now_ms);
            self.state.revision = self.state.revision.saturating_add(1);
        }
        Ok(MemoryMaintenanceReport {
            revision: self.state.revision,
            expired_memory_ids,
        })
    }

    /// 根据结构化范围、当前方向和重要性生成有界投影。这里只给出选择权重；是否
    /// 以及怎样注入模型，由未来 Context 管理器决定。
    pub fn project(
        &self,
        query: OrganizedMemoryQuery,
    ) -> Result<OrganizedMemoryProjection, MemoryOrganizerError> {
        self.project_with_stats(query)
            .map(|(projection, _)| projection)
    }

    fn project_with_stats(
        &self,
        query: OrganizedMemoryQuery,
    ) -> Result<(OrganizedMemoryProjection, MemoryProjectionStats), MemoryOrganizerError> {
        let query = normalize_query(query)?;
        let candidates = self.retrieval_index.candidate_ids(&query);
        let mut stats = MemoryProjectionStats {
            index_seed_posting_count: candidates.seed_posting_count,
            index_membership_check_count: candidates.membership_check_count,
            indexed_candidate_count: candidates.ids.len(),
            ..MemoryProjectionStats::default()
        };
        let candidate_ids = candidates.ids;
        let mut maximum_specificity = BTreeMap::<MemorySlotKey, usize>::new();
        for memory_id in &candidate_ids {
            let Some(record) = self.state.records.get(memory_id) else {
                debug_assert!(false, "retrieval index references an unknown memory");
                continue;
            };
            if record_is_visible(record, &query) {
                stats.visible_candidate_count = stats.visible_candidate_count.saturating_add(1);
                maximum_specificity
                    .entry(MemorySlotKey::from_record(record))
                    .and_modify(|value| {
                        *value = (*value).max(record.applicability.environment.len())
                    })
                    .or_insert(record.applicability.environment.len());
            }
        }

        // 只保留本次需要的 Top-K，查询临时内存不再随全部命中记录线性增长。
        let mut ranked = Vec::<(f32, &OrganizedMemory)>::with_capacity(query.max_items);
        for memory_id in &candidate_ids {
            let Some(record) = self.state.records.get(memory_id) else {
                continue;
            };
            if !record_is_visible(record, &query)
                || maximum_specificity
                    .get(&MemorySlotKey::from_record(record))
                    .is_some_and(|maximum| record.applicability.environment.len() != *maximum)
            {
                continue;
            }
            stats.ranked_candidate_count = stats.ranked_candidate_count.saturating_add(1);
            let candidate = (selection_score(record, &query), record);
            if ranked.len() < query.max_items {
                ranked.push(candidate);
                continue;
            }
            let worst_index = ranked
                .iter()
                .enumerate()
                .max_by(|(_, left), (_, right)| compare_ranked(left, right))
                .map(|(index, _)| index)
                .expect("non-empty bounded ranking must have a worst item");
            if compare_ranked(&candidate, &ranked[worst_index]).is_lt() {
                ranked[worst_index] = candidate;
            }
        }
        ranked.sort_by(compare_ranked);

        stats.retained_candidate_count = ranked.len();
        let omitted_count = stats
            .ranked_candidate_count
            .saturating_sub(stats.retained_candidate_count);
        let score_sum = ranked.iter().map(|(score, _)| *score).sum::<f32>();
        let fallback_weight = if ranked.is_empty() {
            0.0
        } else {
            1.0 / ranked.len() as f32
        };
        let items = ranked
            .into_iter()
            .map(|(score, record)| {
                let evidence_event_ids = all_evidence(record)
                    .map(|evidence| evidence.event_id.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                OrganizedMemoryContextItem {
                    memory_id: record.memory_id.clone(),
                    kind: record.kind,
                    subject: record.subject.clone(),
                    predicate: record.predicate.clone(),
                    value: record.value.clone(),
                    applicability: record.applicability.clone(),
                    status: record.status,
                    importance: record.importance,
                    confidence: record.confidence,
                    selection_score: score,
                    selection_weight: if score_sum > 0.0 {
                        score / score_sum
                    } else {
                        fallback_weight
                    },
                    evidence_event_ids,
                }
            })
            .collect::<Vec<_>>();
        let evidence_event_ids = items
            .iter()
            .flat_map(|item| item.evidence_event_ids.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Ok((
            OrganizedMemoryProjection {
                revision: self.state.revision,
                generated_at_ms: query.current_at_ms,
                items,
                omitted_count,
                evidence_event_ids,
            },
            stats,
        ))
    }

    fn live_slot_records(&self, slot: &MemorySlotKey) -> Vec<String> {
        self.slot_index
            .get(slot)
            .into_iter()
            .flatten()
            .filter(|memory_id| {
                self.state
                    .records
                    .get(*memory_id)
                    .is_some_and(|record| is_live_status(record.status))
            })
            .cloned()
            .collect()
    }

    fn validate_evidence(&self, evidence: &[MemoryEvidence]) -> Result<(), MemoryOrganizerError> {
        for item in evidence {
            let event_id_fingerprint = fingerprint_bytes(item.event_id.as_bytes());
            if let Some(existing_fingerprint) = self.evidence_index.get(&event_id_fingerprint) {
                if existing_fingerprint != &fingerprint_evidence_metadata(item) {
                    return Err(MemoryOrganizerError::new(format!(
                        "evidence event {} was reused with different metadata",
                        item.event_id
                    )));
                }
            }
        }
        Ok(())
    }

    fn index_evidence(&mut self, evidence: &[MemoryEvidence]) {
        for item in evidence {
            self.evidence_index
                .entry(fingerprint_bytes(item.event_id.as_bytes()))
                .or_insert_with(|| fingerprint_evidence_metadata(item));
        }
    }

    /// `conflicts_with_memory_ids` 只表示当前未解决冲突。终态记录的历史关系由未来
    /// memory decision stream 审计，不能继续占用热投影的冲突边预算。
    fn detach_conflicts(&mut self, memory_ids: &[String]) -> Vec<String> {
        let mut affected = BTreeSet::new();
        for memory_id in memory_ids {
            let links = self
                .state
                .records
                .get_mut(memory_id)
                .map(|record| std::mem::take(&mut record.conflicts_with_memory_ids))
                .unwrap_or_default();
            if !links.is_empty() {
                affected.insert(memory_id.clone());
            }
            for linked_id in links {
                if self
                    .state
                    .records
                    .get_mut(&linked_id)
                    .is_some_and(|linked| linked.conflicts_with_memory_ids.remove(memory_id))
                {
                    affected.insert(linked_id);
                }
            }
        }
        affected.into_iter().collect()
    }

    fn merge_candidate(
        &mut self,
        memory_id: String,
        candidate: MemoryCandidate,
        candidate_fingerprint: String,
    ) -> Result<MemoryOrganizationReceipt, MemoryOrganizerError> {
        let record = self
            .state
            .records
            .get(&memory_id)
            .expect("indexed memory must exist");
        ensure_evidence_capacity(record, &candidate.evidence)?;

        let existing_event_ids = all_evidence(record)
            .map(|evidence| evidence.event_id.as_str())
            .collect::<BTreeSet<_>>();
        let new_evidence = candidate
            .evidence
            .iter()
            .filter(|evidence| !existing_event_ids.contains(evidence.event_id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if new_evidence.is_empty() {
            let receipt = self.finish_candidate(
                candidate.candidate_id,
                candidate_fingerprint,
                MemoryOrganizationAction::IgnoredDuplicate,
                vec![memory_id],
            )?;
            // receipt 仍指出候选归属哪条记忆，但该记录本身没有变化；细粒度
            // decision 只需要写 processed-candidate sidecar，不重复写 record post-image。
            self.last_changed_record_ids.clear();
            return Ok(receipt);
        }
        let slot = MemorySlotKey::from_record(record);
        let mut merged_applicability = record.applicability.clone();
        merge_time_range(&mut merged_applicability, &candidate.applicability);
        let new_conflict_ids = self
            .live_slot_records(&slot)
            .into_iter()
            .filter(|other_id| other_id != &memory_id)
            .filter(|other_id| {
                self.state.records.get(other_id).is_some_and(|other| {
                    other.value != record.value
                        && time_ranges_overlap(&other.applicability, &merged_applicability)
                        && environment_scopes_conflict(
                            &other.applicability.environment,
                            &merged_applicability.environment,
                        )
                        && !record.conflicts_with_memory_ids.contains(other_id)
                })
            })
            .collect::<Vec<_>>();
        if record.conflicts_with_memory_ids.len() + new_conflict_ids.len()
            > MAX_MEMORY_CONFLICTS_PER_RECORD
        {
            return Err(MemoryOrganizerError::new(
                "merged memory would exceed the active conflict edge limit",
            ));
        }
        for conflict_id in &new_conflict_ids {
            if self.state.records[conflict_id]
                .conflicts_with_memory_ids
                .len()
                >= MAX_MEMORY_CONFLICTS_PER_RECORD
            {
                return Err(MemoryOrganizerError::new(format!(
                    "memory {conflict_id} has too many conflict edges"
                )));
            }
        }
        let candidate_support_confidence =
            support_confidence_from_evidence(&new_evidence, candidate.confidence);
        let candidate_updated_at = new_evidence
            .iter()
            .map(|evidence| evidence.recorded_at_ms)
            .max()
            .expect("new evidence is not empty");
        let record = self
            .state
            .records
            .get_mut(&memory_id)
            .expect("indexed memory must exist");
        append_evidence(record, new_evidence.clone());
        record.absorbed_candidate_count = record.absorbed_candidate_count.saturating_add(1);
        insert_candidate_fingerprint_sample(record, candidate_fingerprint.clone());
        record.importance = record.importance.max(candidate.importance);
        record.confidence = combine_confidence(record.confidence, candidate_support_confidence);
        record.confidence = penalize_confidence(
            record.confidence,
            new_evidence
                .iter()
                .filter(|evidence| evidence.polarity == MemoryEvidencePolarity::Contradicts),
        );
        record.updated_at_ms = record.updated_at_ms.max(candidate_updated_at);
        record.applicability = merged_applicability;
        record
            .conflicts_with_memory_ids
            .extend(new_conflict_ids.iter().cloned());
        for conflict_id in &new_conflict_ids {
            self.state
                .records
                .get_mut(conflict_id)
                .expect("validated conflict must exist")
                .conflicts_with_memory_ids
                .insert(memory_id.clone());
        }
        let mut affected_memory_ids = vec![memory_id.clone()];
        affected_memory_ids.extend(new_conflict_ids);
        affected_memory_ids.extend(self.reconcile_slot(&slot));
        self.index_evidence(&new_evidence);
        self.finish_candidate(
            candidate.candidate_id,
            candidate_fingerprint,
            MemoryOrganizationAction::Merged,
            affected_memory_ids,
        )
    }

    fn replace_memory(
        &mut self,
        target_id: String,
        candidate: MemoryCandidate,
        candidate_fingerprint: String,
    ) -> Result<MemoryOrganizationReceipt, MemoryOrganizerError> {
        let target = self
            .state
            .records
            .get(&target_id)
            .expect("validated target must exist");
        let new_status = status_from_candidate(&candidate);
        if new_status != OrganizedMemoryStatus::Confirmed {
            return Err(MemoryOrganizerError::new(
                "replacement candidate must be confirmed by non-inference evidence",
            ));
        }
        if candidate.applicability.valid_from_ms <= target.applicability.valid_from_ms {
            return Err(MemoryOrganizerError::new(
                "replacement must begin after the version it replaces",
            ));
        }
        let candidate_authority = candidate_authority(&candidate);
        let target_authority = record_authority(target);
        if target.status == OrganizedMemoryStatus::Confirmed
            && candidate_authority < target_authority
        {
            return Err(MemoryOrganizerError::new(
                "weaker evidence cannot replace confirmed memory",
            ));
        }
        self.ensure_new_record_capacity()?;
        let memory_id = memory_id_for_candidate(&candidate.candidate_id);
        if self.state.records.contains_key(&memory_id) {
            return Err(MemoryOrganizerError::new(format!(
                "memory id collision for candidate {}",
                candidate.candidate_id
            )));
        }
        let mut new_record =
            record_from_candidate(&candidate, new_status, candidate_fingerprint.clone());
        new_record.supersedes_memory_ids.insert(target_id.clone());
        let inherited_conflicts = target
            .conflicts_with_memory_ids
            .iter()
            .filter(|memory_id| {
                self.state.records.get(*memory_id).is_some_and(|record| {
                    is_live_status(record.status)
                        && time_ranges_overlap(&record.applicability, &candidate.applicability)
                        && environment_scopes_conflict(
                            &record.applicability.environment,
                            &candidate.applicability.environment,
                        )
                })
            })
            .cloned()
            .collect::<BTreeSet<_>>();
        if inherited_conflicts.len() > MAX_MEMORY_CONFLICTS_PER_RECORD {
            return Err(MemoryOrganizerError::new(
                "replacement inherits too many unresolved conflicts",
            ));
        }
        for conflict_id in &inherited_conflicts {
            let conflict = self
                .state
                .records
                .get(conflict_id)
                .expect("inherited conflict must exist");
            let conflicts_after_swap =
                conflict
                    .conflicts_with_memory_ids
                    .len()
                    .saturating_sub(usize::from(
                        conflict.conflicts_with_memory_ids.contains(&target_id),
                    ));
            if conflicts_after_swap >= MAX_MEMORY_CONFLICTS_PER_RECORD {
                return Err(MemoryOrganizerError::new(format!(
                    "memory {conflict_id} has too many conflict edges"
                )));
            }
        }
        new_record.conflicts_with_memory_ids = inherited_conflicts.clone();
        let new_updated_at = new_record.updated_at_ms;
        let new_valid_from = new_record.applicability.valid_from_ms;

        let target = self
            .state
            .records
            .get_mut(&target_id)
            .expect("validated target must exist");
        target.status = OrganizedMemoryStatus::Superseded;
        target.superseded_by_memory_id = Some(memory_id.clone());
        target.updated_at_ms = target.updated_at_ms.max(new_updated_at);
        if target
            .applicability
            .valid_until_ms
            .is_none_or(|valid_until_ms| valid_until_ms > new_valid_from)
        {
            target.applicability.valid_until_ms = Some(new_valid_from);
        }

        let slot = MemorySlotKey::from_record(&new_record);
        self.retrieval_index.insert(&new_record);
        self.state.records.insert(memory_id.clone(), new_record);
        self.slot_index
            .entry(slot.clone())
            .or_default()
            .insert(memory_id.clone());
        for conflict_id in &inherited_conflicts {
            self.state
                .records
                .get_mut(conflict_id)
                .expect("inherited conflict must exist")
                .conflicts_with_memory_ids
                .insert(memory_id.clone());
        }
        let mut affected_memory_ids = vec![target_id.clone(), memory_id];
        affected_memory_ids.extend(inherited_conflicts);
        affected_memory_ids.extend(self.detach_conflicts(&[target_id]));
        affected_memory_ids.extend(self.reconcile_slot(&slot));
        self.index_evidence(&candidate.evidence);
        self.finish_candidate(
            candidate.candidate_id,
            candidate_fingerprint,
            MemoryOrganizationAction::Superseded,
            affected_memory_ids,
        )
    }

    fn create_with_conflicts(
        &mut self,
        candidate: MemoryCandidate,
        mut conflict_ids: Vec<String>,
        candidate_fingerprint: String,
    ) -> Result<MemoryOrganizationReceipt, MemoryOrganizerError> {
        self.ensure_new_record_capacity()?;
        let slot = MemorySlotKey::from_candidate(&candidate);
        let overlapping_slot_records = self
            .live_slot_records(&slot)
            .into_iter()
            .filter(|memory_id| {
                self.state.records.get(memory_id).is_some_and(|record| {
                    time_ranges_overlap(&record.applicability, &candidate.applicability)
                        && !environment_scopes_are_disjoint(
                            &record.applicability.environment,
                            &candidate.applicability.environment,
                        )
                })
            })
            .count();
        if candidate.kind != OrganizedMemoryKind::ActionExperience
            && overlapping_slot_records >= MAX_MEMORY_RECORDS_PER_BASE_SLOT
        {
            return Err(MemoryOrganizerError::new(format!(
                "memory base slot exceeds {MAX_MEMORY_RECORDS_PER_BASE_SLOT} live records; consolidation is required"
            )));
        }
        conflict_ids.sort();
        conflict_ids.dedup();
        if conflict_ids.len() > MAX_MEMORY_CONFLICTS_PER_RECORD {
            return Err(MemoryOrganizerError::new(format!(
                "memory conflict set exceeds {MAX_MEMORY_CONFLICTS_PER_RECORD} records"
            )));
        }
        for conflict_id in &conflict_ids {
            let record = self
                .state
                .records
                .get(conflict_id)
                .ok_or_else(|| MemoryOrganizerError::new("conflicting memory disappeared"))?;
            if record.conflicts_with_memory_ids.len() >= MAX_MEMORY_CONFLICTS_PER_RECORD {
                return Err(MemoryOrganizerError::new(format!(
                    "memory {conflict_id} has too many conflict edges"
                )));
            }
        }
        let memory_id = memory_id_for_candidate(&candidate.candidate_id);
        if self.state.records.contains_key(&memory_id) {
            return Err(MemoryOrganizerError::new(format!(
                "memory id collision for candidate {}",
                candidate.candidate_id
            )));
        }

        let mut new_record = record_from_candidate(
            &candidate,
            status_from_candidate(&candidate),
            candidate_fingerprint.clone(),
        );
        for conflict_id in &conflict_ids {
            new_record
                .conflicts_with_memory_ids
                .insert(conflict_id.clone());
        }

        let mut affected_memory_ids = vec![memory_id.clone()];
        for conflict_id in &conflict_ids {
            let existing = self
                .state
                .records
                .get_mut(conflict_id)
                .expect("validated conflict must exist");
            existing.conflicts_with_memory_ids.insert(memory_id.clone());
            affected_memory_ids.push(conflict_id.clone());
        }

        let slot = MemorySlotKey::from_record(&new_record);
        self.retrieval_index.insert(&new_record);
        self.state.records.insert(memory_id.clone(), new_record);
        self.slot_index
            .entry(slot.clone())
            .or_default()
            .insert(memory_id);
        affected_memory_ids.extend(self.reconcile_slot(&slot));
        let action = if self
            .state
            .records
            .get(&memory_id_for_candidate(&candidate.candidate_id))
            .is_some_and(|record| record.status == OrganizedMemoryStatus::Disputed)
        {
            MemoryOrganizationAction::Disputed
        } else {
            MemoryOrganizationAction::Created
        };
        self.index_evidence(&candidate.evidence);
        self.finish_candidate(
            candidate.candidate_id,
            candidate_fingerprint,
            action,
            affected_memory_ids,
        )
    }

    /// 争议是槽位级派生状态，不允许因为输入顺序不同而“粘死”。每次相关写入后，
    /// 都从直接证据、活动冲突边和来源权威重新计算整个基础槽。
    fn reconcile_slot(&mut self, slot: &MemorySlotKey) -> Vec<String> {
        let ids = self.live_slot_records(slot);
        let desired = ids
            .iter()
            .filter_map(|memory_id| {
                self.state
                    .records
                    .get(memory_id)
                    .map(|record| (memory_id.clone(), reconciled_status(&self.state, record)))
            })
            .collect::<Vec<_>>();

        let mut changed = Vec::new();
        for (memory_id, status) in desired {
            let record = self
                .state
                .records
                .get_mut(&memory_id)
                .expect("reconciled memory must exist");
            if record.status != status {
                record.status = status;
                changed.push(memory_id);
            }
        }
        changed
    }

    fn ensure_new_record_capacity(&self) -> Result<(), MemoryOrganizerError> {
        if self.state.records.len() >= MAX_ORGANIZED_MEMORY_RECORDS {
            return Err(MemoryOrganizerError::new(format!(
                "organized memory limit {MAX_ORGANIZED_MEMORY_RECORDS} reached"
            )));
        }
        Ok(())
    }

    fn finish_candidate(
        &mut self,
        candidate_id: String,
        candidate_fingerprint: String,
        action: MemoryOrganizationAction,
        mut affected_memory_ids: Vec<String>,
    ) -> Result<MemoryOrganizationReceipt, MemoryOrganizerError> {
        affected_memory_ids.sort();
        affected_memory_ids.dedup();
        debug_assert!(affected_memory_ids.len() <= MAX_MEMORY_IDS_PER_RECEIPT);
        self.last_changed_record_ids = affected_memory_ids.clone();
        self.state.revision = self.state.revision.saturating_add(1);
        self.state.processed_candidates.insert(
            candidate_id.clone(),
            ProcessedMemoryCandidate {
                applied_revision: self.state.revision,
                candidate_fingerprint,
                memory_ids: affected_memory_ids.clone(),
            },
        );
        Ok(MemoryOrganizationReceipt {
            candidate_id,
            revision: self.state.revision,
            action,
            affected_memory_ids,
        })
    }
}

fn compare_ranked(
    (left_score, left): &(f32, &OrganizedMemory),
    (right_score, right): &(f32, &OrganizedMemory),
) -> Ordering {
    right_score
        .total_cmp(left_score)
        .then_with(|| right.updated_at_ms.cmp(&left.updated_at_ms))
        .then_with(|| left.memory_id.cmp(&right.memory_id))
}

fn normalize_candidate(
    mut candidate: MemoryCandidate,
) -> Result<MemoryCandidate, MemoryOrganizerError> {
    candidate.candidate_id = normalize_text(&candidate.candidate_id, "candidate id")?;
    if ORGANIZED_MEMORY_ID_PREFIX.chars().count() + candidate.candidate_id.chars().count()
        > MAX_TEXT_CHARS
    {
        return Err(MemoryOrganizerError::new(
            "candidate id is too long to form a canonical memory id",
        ));
    }
    candidate.subject = normalize_text(&candidate.subject, "memory subject")?;
    candidate.predicate = normalize_text(&candidate.predicate, "memory predicate")?;
    candidate.applicability = normalize_applicability(candidate.applicability)?;
    candidate.target_memory_id = candidate
        .target_memory_id
        .map(|value| normalize_text(&value, "target memory id"))
        .transpose()?;
    validate_score(candidate.importance, "memory importance")?;
    validate_score(candidate.confidence, "candidate confidence")?;
    let value_bytes = serde_json::to_vec(&candidate.value).map_err(|error| {
        MemoryOrganizerError::new(format!("memory value is not serializable: {error}"))
    })?;
    if value_bytes.len() > MAX_ORGANIZED_MEMORY_VALUE_BYTES {
        return Err(MemoryOrganizerError::new(format!(
            "memory value exceeds {MAX_ORGANIZED_MEMORY_VALUE_BYTES} bytes"
        )));
    }
    if candidate.evidence.is_empty() {
        return Err(MemoryOrganizerError::new(
            "memory candidate requires at least one evidence event",
        ));
    }
    if candidate.evidence.len() > MAX_MEMORY_EVIDENCE_PER_CANDIDATE {
        return Err(MemoryOrganizerError::new(format!(
            "memory candidate evidence exceeds {MAX_MEMORY_EVIDENCE_PER_CANDIDATE} items"
        )));
    }
    candidate.evidence = normalize_evidence(candidate.evidence)?;
    if !candidate
        .evidence
        .iter()
        .any(|evidence| evidence.polarity == MemoryEvidencePolarity::Supports)
    {
        return Err(MemoryOrganizerError::new(
            "memory candidate requires supporting evidence",
        ));
    }
    match candidate.intent {
        MemoryCandidateIntent::Assert if candidate.target_memory_id.is_some() => {
            return Err(MemoryOrganizerError::new(
                "assert candidate must not name a target memory",
            ));
        }
        MemoryCandidateIntent::Replace | MemoryCandidateIntent::Contradict
            if candidate.target_memory_id.is_none() =>
        {
            return Err(MemoryOrganizerError::new(
                "replace or contradict candidate requires a target memory",
            ));
        }
        _ => {}
    }
    if candidate.kind == OrganizedMemoryKind::ActionExperience
        && candidate.intent != MemoryCandidateIntent::Assert
    {
        return Err(MemoryOrganizerError::new(
            "action experience is an immutable episode; retract a bad episode instead",
        ));
    }
    if looks_like_task_runtime_state(&candidate.subject, &candidate.predicate) {
        return Err(MemoryOrganizerError::new(
            "live task status and progress belong to the task system, not long-term memory",
        ));
    }
    if super::memory_slot_is_credential(&candidate.subject, &candidate.predicate)
        && !super::is_credential_reference(&candidate.value)
    {
        return Err(MemoryOrganizerError::new(
            "credential memory must store a keyring reference, never secret material",
        ));
    }
    Ok(candidate)
}

fn normalize_retraction(
    mut request: RetractOrganizedMemoryRequest,
) -> Result<RetractOrganizedMemoryRequest, MemoryOrganizerError> {
    request.operation_id = normalize_text(&request.operation_id, "retraction operation id")?;
    request.memory_id = normalize_text(&request.memory_id, "memory id")?;
    request.reason = normalize_text(&request.reason, "retraction reason")?;
    if request.retracted_at_ms < 0 {
        return Err(MemoryOrganizerError::new(
            "memory retraction timestamp must be non-negative",
        ));
    }
    if request.evidence.is_empty() {
        return Err(MemoryOrganizerError::new(
            "memory retraction requires evidence",
        ));
    }
    if request.evidence.len() > MAX_MEMORY_EVIDENCE_PER_CANDIDATE {
        return Err(MemoryOrganizerError::new(format!(
            "memory retraction evidence exceeds {MAX_MEMORY_EVIDENCE_PER_CANDIDATE} items"
        )));
    }
    request.evidence = normalize_evidence(request.evidence)?;
    if request
        .evidence
        .iter()
        .any(|evidence| evidence.polarity != MemoryEvidencePolarity::Supports)
    {
        return Err(MemoryOrganizerError::new(
            "retraction evidence must support the retraction operation",
        ));
    }
    Ok(request)
}

fn normalize_resolution(
    mut request: ResolveMemoryDisputeRequest,
) -> Result<ResolveMemoryDisputeRequest, MemoryOrganizerError> {
    request.operation_id = normalize_text(&request.operation_id, "resolution operation id")?;
    request.winner_memory_id = normalize_text(&request.winner_memory_id, "winner memory id")?;
    request.losing_memory_ids =
        normalize_string_list(request.losing_memory_ids, "resolution losing memory id")?;
    if request.losing_memory_ids.is_empty()
        || request
            .losing_memory_ids
            .contains(&request.winner_memory_id)
    {
        return Err(MemoryOrganizerError::new(
            "resolution requires at least one distinct losing memory",
        ));
    }
    if request.losing_memory_ids.len() > MAX_MEMORY_CONFLICTS_PER_RECORD {
        return Err(MemoryOrganizerError::new(format!(
            "resolution exceeds {MAX_MEMORY_CONFLICTS_PER_RECORD} losing memories"
        )));
    }
    request.reason = normalize_text(&request.reason, "resolution reason")?;
    if request.resolved_at_ms < 0 {
        return Err(MemoryOrganizerError::new(
            "resolution timestamp must be non-negative",
        ));
    }
    if request.evidence.is_empty() {
        return Err(MemoryOrganizerError::new(
            "dispute resolution requires evidence",
        ));
    }
    if request.evidence.len() > MAX_MEMORY_EVIDENCE_PER_CANDIDATE {
        return Err(MemoryOrganizerError::new(format!(
            "dispute resolution evidence exceeds {MAX_MEMORY_EVIDENCE_PER_CANDIDATE} items"
        )));
    }
    request.evidence = normalize_evidence(request.evidence)?;
    if request
        .evidence
        .iter()
        .any(|evidence| evidence.polarity != MemoryEvidencePolarity::Supports)
        || request
            .evidence
            .iter()
            .all(|evidence| evidence.origin == MemoryEvidenceOrigin::ModelInference)
    {
        return Err(MemoryOrganizerError::new(
            "dispute resolution requires non-inference supporting evidence",
        ));
    }
    Ok(request)
}

fn normalize_query(
    mut query: OrganizedMemoryQuery,
) -> Result<OrganizedMemoryQuery, MemoryOrganizerError> {
    if query.current_at_ms < 0 {
        return Err(MemoryOrganizerError::new(
            "memory projection timestamp must be non-negative",
        ));
    }
    if query.max_items == 0 || query.max_items > MAX_ORGANIZED_CONTEXT_ITEMS {
        return Err(MemoryOrganizerError::new(format!(
            "max items must be between 1 and {MAX_ORGANIZED_CONTEXT_ITEMS}"
        )));
    }
    query.space_id = normalize_text(&query.space_id, "space id")?;
    query.environment = normalize_environment(query.environment)?;
    query.subjects =
        normalize_bounded_string_list(query.subjects, "subject", MAX_QUERY_FILTER_VALUES)?;
    query.predicates =
        normalize_bounded_string_list(query.predicates, "predicate", MAX_QUERY_FILTER_VALUES)?;
    query.focus_terms =
        normalize_bounded_string_list(query.focus_terms, "focus term", MAX_QUERY_FOCUS_TERMS)?
            .into_iter()
            .map(|term| term.to_lowercase())
            .collect();
    query.kinds.sort();
    query.kinds.dedup();
    Ok(query)
}

fn normalize_applicability(
    mut applicability: MemoryApplicability,
) -> Result<MemoryApplicability, MemoryOrganizerError> {
    applicability.space_id = normalize_text(&applicability.space_id, "space id")?;
    applicability.environment = normalize_environment(applicability.environment)?;
    if applicability.valid_from_ms < 0 {
        return Err(MemoryOrganizerError::new(
            "memory valid-from timestamp must be non-negative",
        ));
    }
    if applicability
        .valid_until_ms
        .is_some_and(|valid_until_ms| valid_until_ms <= applicability.valid_from_ms)
    {
        return Err(MemoryOrganizerError::new(
            "memory valid-until timestamp must be after valid-from",
        ));
    }
    Ok(applicability)
}

fn normalize_environment(
    environment: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, MemoryOrganizerError> {
    if environment.len() > MAX_ENVIRONMENT_KEYS {
        return Err(MemoryOrganizerError::new(format!(
            "memory environment exceeds {MAX_ENVIRONMENT_KEYS} keys"
        )));
    }
    environment
        .into_iter()
        .map(|(key, value)| {
            Ok((
                normalize_text(&key, "environment key")?,
                normalize_text(&value, "environment value")?,
            ))
        })
        .collect()
}

fn normalize_evidence(
    evidence: Vec<MemoryEvidence>,
) -> Result<Vec<MemoryEvidence>, MemoryOrganizerError> {
    if evidence.len() > MAX_MEMORY_EVIDENCE_PER_RECORD {
        return Err(MemoryOrganizerError::new(format!(
            "memory evidence exceeds {MAX_MEMORY_EVIDENCE_PER_RECORD} items"
        )));
    }
    let mut by_event = BTreeMap::<String, MemoryEvidence>::new();
    for mut item in evidence {
        item.event_id = normalize_text(&item.event_id, "evidence event id")?;
        item.source_actor_id = normalize_text(&item.source_actor_id, "evidence source actor id")?;
        item.mission_id = item
            .mission_id
            .map(|value| normalize_text(&value, "evidence mission id"))
            .transpose()?;
        item.run_id = item
            .run_id
            .map(|value| normalize_text(&value, "evidence run id"))
            .transpose()?;
        if item.run_id.is_some() && item.mission_id.is_none() {
            return Err(MemoryOrganizerError::new(
                "evidence with run id requires mission id",
            ));
        }
        if item.origin == MemoryEvidenceOrigin::VerifiedTaskOutcome
            && (item.mission_id.is_none()
                || item.run_id.is_none()
                || !(item.source_actor_id.starts_with("kernel:")
                    || item.source_actor_id.starts_with("verifier:")))
        {
            return Err(MemoryOrganizerError::new(
                "verified task outcome requires mission, run, and a kernel/verifier source",
            ));
        }
        if item.observed_at_ms < 0 || item.recorded_at_ms < 0 {
            return Err(MemoryOrganizerError::new(
                "evidence timestamps must be non-negative",
            ));
        }
        if item.recorded_at_ms < item.observed_at_ms {
            return Err(MemoryOrganizerError::new(
                "evidence recorded time must not precede observed time",
            ));
        }
        validate_score(item.reliability, "evidence reliability")?;
        match by_event.get(&item.event_id) {
            Some(existing) if existing != &item => {
                return Err(MemoryOrganizerError::new(format!(
                    "evidence event {} has conflicting metadata",
                    item.event_id
                )));
            }
            Some(_) => {}
            None => {
                by_event.insert(item.event_id.clone(), item);
            }
        }
    }
    Ok(by_event.into_values().collect())
}

fn normalize_string_list(
    values: Vec<String>,
    label: &str,
) -> Result<Vec<String>, MemoryOrganizerError> {
    let mut values = values
        .into_iter()
        .map(|value| normalize_text(&value, label))
        .collect::<Result<Vec<_>, _>>()?;
    values.sort();
    values.dedup();
    Ok(values)
}

fn normalize_bounded_string_list(
    values: Vec<String>,
    label: &str,
    max_items: usize,
) -> Result<Vec<String>, MemoryOrganizerError> {
    if values.len() > max_items {
        return Err(MemoryOrganizerError::new(format!(
            "{label} filter exceeds {max_items} items"
        )));
    }
    let values = normalize_string_list(values, label)?;
    if values
        .iter()
        .map(|value| value.chars().count())
        .sum::<usize>()
        > MAX_QUERY_FILTER_CHARS
    {
        return Err(MemoryOrganizerError::new(format!(
            "{label} filter exceeds {MAX_QUERY_FILTER_CHARS} characters"
        )));
    }
    Ok(values)
}

fn normalize_text(value: &str, label: &str) -> Result<String, MemoryOrganizerError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(MemoryOrganizerError::new(format!(
            "{label} must not be empty"
        )));
    }
    if value.chars().count() > MAX_TEXT_CHARS {
        return Err(MemoryOrganizerError::new(format!("{label} is too long")));
    }
    Ok(value.to_string())
}

fn validate_score(value: f32, label: &str) -> Result<(), MemoryOrganizerError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(MemoryOrganizerError::new(format!(
            "{label} must be between 0 and 1"
        )));
    }
    Ok(())
}

fn validate_candidate_target(
    candidate: &MemoryCandidate,
    target: &OrganizedMemory,
) -> Result<(), MemoryOrganizerError> {
    if !is_live_status(target.status) {
        return Err(MemoryOrganizerError::new(format!(
            "target memory {} is not current",
            target.memory_id
        )));
    }
    if MemorySlotKey::from_candidate(candidate) != MemorySlotKey::from_record(target) {
        return Err(MemoryOrganizerError::new(
            "target memory is in a different kind, subject, predicate, or space",
        ));
    }
    if candidate.intent == MemoryCandidateIntent::Replace
        && candidate.applicability.environment != target.applicability.environment
    {
        return Err(MemoryOrganizerError::new(
            "replacement requires the exact same environment scope; use a scoped exception instead",
        ));
    }
    if candidate.intent == MemoryCandidateIntent::Contradict
        && environment_scopes_are_disjoint(
            &candidate.applicability.environment,
            &target.applicability.environment,
        )
    {
        return Err(MemoryOrganizerError::new(
            "contradiction target has a disjoint environment scope",
        ));
    }
    if candidate.intent == MemoryCandidateIntent::Contradict
        && !time_ranges_overlap(&candidate.applicability, &target.applicability)
    {
        return Err(MemoryOrganizerError::new(
            "contradiction target has a disjoint effective time range",
        ));
    }
    if candidate.intent == MemoryCandidateIntent::Contradict && candidate.value == target.value {
        return Err(MemoryOrganizerError::new(
            "contradiction candidate must assert a different value",
        ));
    }
    Ok(())
}

fn validate_state(state: &MemoryOrganizerState) -> Result<(), MemoryOrganizerError> {
    if state.schema_version != MEMORY_ORGANIZER_SCHEMA_VERSION {
        return Err(MemoryOrganizerError::new(format!(
            "unsupported memory organizer schema version {}",
            state.schema_version
        )));
    }
    if state.records.len() > MAX_ORGANIZED_MEMORY_RECORDS {
        return Err(MemoryOrganizerError::new(
            "organized memory state is too large",
        ));
    }
    if state.processed_candidates.len() > MAX_PROCESSED_MEMORY_CANDIDATES {
        return Err(MemoryOrganizerError::new(
            "processed candidate state is too large",
        ));
    }
    if state
        .last_maintenance_at_ms
        .is_some_and(|timestamp| timestamp < 0)
    {
        return Err(MemoryOrganizerError::new(
            "memory maintenance cursor must be non-negative",
        ));
    }
    for (memory_id, record) in &state.records {
        if memory_id != &record.memory_id {
            return Err(MemoryOrganizerError::new(
                "memory state key does not match record id",
            ));
        }
        validate_record(record)?;
    }
    let mut canonical_evidence = BTreeMap::<String, MemoryEvidence>::new();
    for evidence in state.records.values().flat_map(all_record_evidence) {
        match canonical_evidence.get(&evidence.event_id) {
            Some(existing) if !same_evidence_metadata(existing, evidence) => {
                return Err(MemoryOrganizerError::new(format!(
                    "evidence event {} has inconsistent metadata across memories",
                    evidence.event_id
                )));
            }
            Some(_) => {}
            None => {
                canonical_evidence.insert(evidence.event_id.clone(), evidence.clone());
            }
        }
    }
    for (candidate_id, processed) in &state.processed_candidates {
        if normalize_text(candidate_id, "processed candidate id")? != *candidate_id {
            return Err(MemoryOrganizerError::new(
                "processed candidate id is not canonical",
            ));
        }
        if processed.applied_revision > state.revision
            || !is_canonical_digest(&processed.candidate_fingerprint)
        {
            return Err(MemoryOrganizerError::new(
                "processed candidate metadata is invalid",
            ));
        }
        if processed.memory_ids.is_empty()
            || processed.memory_ids.len() > MAX_MEMORY_IDS_PER_RECEIPT
            || processed
                .memory_ids
                .iter()
                .any(|memory_id| !state.records.contains_key(memory_id))
            || normalize_string_list(processed.memory_ids.clone(), "processed memory id")?
                != processed.memory_ids
        {
            return Err(MemoryOrganizerError::new(
                "processed candidate references unknown memory",
            ));
        }
    }
    if state.dispute_resolutions.len() > MAX_PROCESSED_MEMORY_CANDIDATES {
        return Err(MemoryOrganizerError::new(
            "dispute resolution state is too large",
        ));
    }
    for (operation_id, resolution) in &state.dispute_resolutions {
        if operation_id != &resolution.operation_id
            || resolution.revision > state.revision
            || !is_canonical_digest(&resolution.request_fingerprint)
            || !resolution.changed
            || !state.records.contains_key(&resolution.winner_memory_id)
            || resolution
                .superseded_memory_ids
                .iter()
                .any(|memory_id| !state.records.contains_key(memory_id))
            || resolution.superseded_memory_ids.is_empty()
            || resolution.superseded_memory_ids.len() > MAX_MEMORY_CONFLICTS_PER_RECORD
            || normalize_string_list(
                resolution.superseded_memory_ids.clone(),
                "resolved memory id",
            )? != resolution.superseded_memory_ids
        {
            return Err(MemoryOrganizerError::new(
                "dispute resolution metadata is invalid",
            ));
        }
        // Receipt 是历史幂等记录，不是当前状态断言。解决后 winner 仍可以
        // 被新版本替代或撤回，loser 也可以被撤回；但当初建立的双向替代
        // 关系必须仍可审计。
        let winner = &state.records[&resolution.winner_memory_id];
        if resolution.superseded_memory_ids.iter().any(|memory_id| {
            let loser = &state.records[memory_id];
            loser.superseded_by_memory_id.as_deref() != Some(resolution.winner_memory_id.as_str())
                || !winner.supersedes_memory_ids.contains(memory_id)
        }) {
            return Err(MemoryOrganizerError::new(
                "dispute resolution result is not reflected in memory state",
            ));
        }
    }
    let mut retraction_operations = BTreeSet::new();
    for record in state.records.values() {
        if record.supersedes_memory_ids.len() > MAX_MEMORY_SUPERSEDED_RECORDS_PER_RECORD
            || record.conflicts_with_memory_ids.len() > MAX_MEMORY_CONFLICTS_PER_RECORD
            || record.conflicts_with_memory_ids.contains(&record.memory_id)
            || (!is_live_status(record.status) && !record.conflicts_with_memory_ids.is_empty())
        {
            return Err(MemoryOrganizerError::new(
                "memory conflict links are invalid",
            ));
        }
        for linked_id in record
            .supersedes_memory_ids
            .iter()
            .chain(record.superseded_by_memory_id.iter())
            .chain(record.conflicts_with_memory_ids.iter())
        {
            if !state.records.contains_key(linked_id) {
                return Err(MemoryOrganizerError::new(format!(
                    "memory {} references unknown memory {linked_id}",
                    record.memory_id
                )));
            }
        }
        for conflict_id in &record.conflicts_with_memory_ids {
            let conflict = &state.records[conflict_id];
            if !is_live_status(conflict.status)
                || MemorySlotKey::from_record(conflict) != MemorySlotKey::from_record(record)
                || conflict.value == record.value
                || !time_ranges_overlap(&conflict.applicability, &record.applicability)
                || !environment_scopes_conflict(
                    &conflict.applicability.environment,
                    &record.applicability.environment,
                )
                || !conflict
                    .conflicts_with_memory_ids
                    .contains(&record.memory_id)
            {
                return Err(MemoryOrganizerError::new(
                    "memory conflict link must be symmetric",
                ));
            }
        }
        if is_live_status(record.status) && record.status != reconciled_status(state, record) {
            return Err(MemoryOrganizerError::new(
                "memory live status is not the canonical result of its evidence and conflict graph",
            ));
        }
        for superseded_id in &record.supersedes_memory_ids {
            if state.records[superseded_id]
                .superseded_by_memory_id
                .as_deref()
                != Some(record.memory_id.as_str())
            {
                return Err(MemoryOrganizerError::new(
                    "memory supersession link must be symmetric",
                ));
            }
        }
        if let Some(parent_id) = &record.superseded_by_memory_id {
            if !state.records[parent_id]
                .supersedes_memory_ids
                .contains(&record.memory_id)
            {
                return Err(MemoryOrganizerError::new(
                    "memory supersession parent link must be symmetric",
                ));
            }
        }
        if let Some(retraction) = &record.retraction {
            if !retraction_operations.insert(retraction.operation_id.as_str()) {
                return Err(MemoryOrganizerError::new(
                    "retraction operation id must be globally unique",
                ));
            }
        }
    }
    Ok(())
}

/// Decision replay 的增量门禁。它只检查本批触及的记录和 sidecar，再沿这些记录的
/// 有界关系边检查对称性；完整 checkpoint 仍由 `validate_state` 做一次全量验证。
pub(super) fn validate_incremental_projection_state(
    state: &MemoryOrganizerState,
    record_ids: &BTreeSet<String>,
    processed_candidate_ids: &[String],
    resolution_operation_ids: &[String],
) -> Result<(), MemoryOrganizerError> {
    if state.records.len() > MAX_ORGANIZED_MEMORY_RECORDS
        || state.processed_candidates.len() > MAX_PROCESSED_MEMORY_CANDIDATES
        || state.dispute_resolutions.len() > MAX_PROCESSED_MEMORY_CANDIDATES
    {
        return Err(MemoryOrganizerError::new(
            "incremental memory projection exceeds a state limit",
        ));
    }
    if state
        .last_maintenance_at_ms
        .is_some_and(|timestamp| timestamp < 0)
    {
        return Err(MemoryOrganizerError::new(
            "memory maintenance cursor must be non-negative",
        ));
    }
    for memory_id in record_ids {
        let record = state.records.get(memory_id).ok_or_else(|| {
            MemoryOrganizerError::new(format!(
                "incremental projection references unknown memory {memory_id}"
            ))
        })?;
        if memory_id != &record.memory_id {
            return Err(MemoryOrganizerError::new(
                "memory state key does not match record id",
            ));
        }
        validate_record(record)?;
        validate_record_relationships(state, record)?;
    }
    for candidate_id in processed_candidate_ids {
        let processed = state
            .processed_candidates
            .get(candidate_id)
            .ok_or_else(|| {
                MemoryOrganizerError::new(
                    "incremental projection lost processed candidate metadata",
                )
            })?;
        if normalize_text(candidate_id, "processed candidate id")? != *candidate_id
            || processed.applied_revision > state.revision
            || !is_canonical_digest(&processed.candidate_fingerprint)
            || processed.memory_ids.is_empty()
            || processed.memory_ids.len() > MAX_MEMORY_IDS_PER_RECEIPT
            || processed
                .memory_ids
                .iter()
                .any(|memory_id| !state.records.contains_key(memory_id))
            || normalize_string_list(processed.memory_ids.clone(), "processed memory id")?
                != processed.memory_ids
        {
            return Err(MemoryOrganizerError::new(
                "processed candidate metadata is invalid",
            ));
        }
    }
    for operation_id in resolution_operation_ids {
        let resolution = state.dispute_resolutions.get(operation_id).ok_or_else(|| {
            MemoryOrganizerError::new("incremental projection lost dispute resolution metadata")
        })?;
        if operation_id != &resolution.operation_id
            || resolution.revision > state.revision
            || !is_canonical_digest(&resolution.request_fingerprint)
            || !resolution.changed
            || !state.records.contains_key(&resolution.winner_memory_id)
            || resolution
                .superseded_memory_ids
                .iter()
                .any(|memory_id| !state.records.contains_key(memory_id))
            || resolution.superseded_memory_ids.is_empty()
            || resolution.superseded_memory_ids.len() > MAX_MEMORY_CONFLICTS_PER_RECORD
            || normalize_string_list(
                resolution.superseded_memory_ids.clone(),
                "resolved memory id",
            )? != resolution.superseded_memory_ids
        {
            return Err(MemoryOrganizerError::new(
                "dispute resolution metadata is invalid",
            ));
        }
        let winner = &state.records[&resolution.winner_memory_id];
        if resolution.superseded_memory_ids.iter().any(|memory_id| {
            let loser = &state.records[memory_id];
            loser.superseded_by_memory_id.as_deref() != Some(resolution.winner_memory_id.as_str())
                || !winner.supersedes_memory_ids.contains(memory_id)
        }) {
            return Err(MemoryOrganizerError::new(
                "dispute resolution result is not reflected in memory state",
            ));
        }
    }
    Ok(())
}

fn validate_record_relationships(
    state: &MemoryOrganizerState,
    record: &OrganizedMemory,
) -> Result<(), MemoryOrganizerError> {
    if record.supersedes_memory_ids.len() > MAX_MEMORY_SUPERSEDED_RECORDS_PER_RECORD
        || record.conflicts_with_memory_ids.len() > MAX_MEMORY_CONFLICTS_PER_RECORD
        || record.conflicts_with_memory_ids.contains(&record.memory_id)
        || (!is_live_status(record.status) && !record.conflicts_with_memory_ids.is_empty())
    {
        return Err(MemoryOrganizerError::new(
            "memory conflict links are invalid",
        ));
    }
    for linked_id in record
        .supersedes_memory_ids
        .iter()
        .chain(record.superseded_by_memory_id.iter())
        .chain(record.conflicts_with_memory_ids.iter())
    {
        if !state.records.contains_key(linked_id) {
            return Err(MemoryOrganizerError::new(format!(
                "memory {} references unknown memory {linked_id}",
                record.memory_id
            )));
        }
    }
    for conflict_id in &record.conflicts_with_memory_ids {
        let conflict = &state.records[conflict_id];
        if !is_live_status(conflict.status)
            || MemorySlotKey::from_record(conflict) != MemorySlotKey::from_record(record)
            || conflict.value == record.value
            || !time_ranges_overlap(&conflict.applicability, &record.applicability)
            || !environment_scopes_conflict(
                &conflict.applicability.environment,
                &record.applicability.environment,
            )
            || !conflict
                .conflicts_with_memory_ids
                .contains(&record.memory_id)
        {
            return Err(MemoryOrganizerError::new(
                "memory conflict link must be symmetric",
            ));
        }
    }
    for superseded_id in &record.supersedes_memory_ids {
        if state.records[superseded_id]
            .superseded_by_memory_id
            .as_deref()
            != Some(record.memory_id.as_str())
        {
            return Err(MemoryOrganizerError::new(
                "memory supersession link must be symmetric",
            ));
        }
    }
    if let Some(parent_id) = &record.superseded_by_memory_id {
        if !state.records[parent_id]
            .supersedes_memory_ids
            .contains(&record.memory_id)
        {
            return Err(MemoryOrganizerError::new(
                "memory supersession parent link must be symmetric",
            ));
        }
    }
    if is_live_status(record.status) && record.status != reconciled_status(state, record) {
        return Err(MemoryOrganizerError::new(
            "memory live status is not the canonical result of its evidence and conflict graph",
        ));
    }
    Ok(())
}

fn same_evidence_metadata(left: &MemoryEvidence, right: &MemoryEvidence) -> bool {
    left.event_id == right.event_id
        && left.source_actor_id == right.source_actor_id
        && left.origin == right.origin
        && left.observed_at_ms == right.observed_at_ms
        && left.recorded_at_ms == right.recorded_at_ms
        && left.reliability == right.reliability
        && left.mission_id == right.mission_id
        && left.run_id == right.run_id
}

pub(super) fn validate_record(record: &OrganizedMemory) -> Result<(), MemoryOrganizerError> {
    if normalize_text(&record.memory_id, "memory id")? != record.memory_id
        || normalize_text(&record.subject, "memory subject")? != record.subject
        || normalize_text(&record.predicate, "memory predicate")? != record.predicate
        || normalize_applicability(record.applicability.clone())? != record.applicability
    {
        return Err(MemoryOrganizerError::new(
            "organized memory fields are not canonical",
        ));
    }
    validate_score(record.importance, "memory importance")?;
    validate_score(record.confidence, "memory confidence")?;
    if looks_like_task_runtime_state(&record.subject, &record.predicate) {
        return Err(MemoryOrganizerError::new(
            "organized memory contains live task state",
        ));
    }
    if super::memory_slot_is_credential(&record.subject, &record.predicate)
        && !super::is_credential_reference(&record.value)
    {
        return Err(MemoryOrganizerError::new(
            "organized credential memory contains raw material",
        ));
    }
    let value_bytes = serde_json::to_vec(&record.value).map_err(|error| {
        MemoryOrganizerError::new(format!("memory value is not serializable: {error}"))
    })?;
    if value_bytes.len() > MAX_ORGANIZED_MEMORY_VALUE_BYTES {
        return Err(MemoryOrganizerError::new(
            "organized memory value is too large",
        ));
    }
    if record.absorbed_candidate_count == 0
        || record.candidate_fingerprint_samples.is_empty()
        || record.candidate_fingerprint_samples.len() > MAX_MEMORY_CANDIDATE_FINGERPRINT_SAMPLES
        || record.candidate_fingerprint_samples.len() as u64 > record.absorbed_candidate_count
        || record
            .candidate_fingerprint_samples
            .iter()
            .any(|fingerprint| !is_canonical_digest(fingerprint))
    {
        return Err(MemoryOrganizerError::new(
            "organized memory candidate provenance is invalid",
        ));
    }
    if record.created_at_ms < 0 || record.updated_at_ms < record.created_at_ms {
        return Err(MemoryOrganizerError::new(
            "organized memory timestamps are invalid",
        ));
    }
    let normalized_supporting = normalize_evidence(record.supporting_evidence.clone())?;
    let normalized_contradicting = normalize_evidence(record.contradicting_evidence.clone())?;
    if normalized_supporting != record.supporting_evidence
        || normalized_contradicting != record.contradicting_evidence
    {
        return Err(MemoryOrganizerError::new(
            "organized memory evidence is not canonical",
        ));
    }
    let mut evidence = record.supporting_evidence.clone();
    if record
        .supporting_evidence
        .iter()
        .any(|item| item.polarity != MemoryEvidencePolarity::Supports)
        || record
            .contradicting_evidence
            .iter()
            .any(|item| item.polarity != MemoryEvidencePolarity::Contradicts)
    {
        return Err(MemoryOrganizerError::new(
            "organized evidence polarity is in the wrong collection",
        ));
    }
    evidence.extend(record.contradicting_evidence.clone());
    let evidence = normalize_evidence(evidence)?;
    if !evidence
        .iter()
        .any(|item| item.polarity == MemoryEvidencePolarity::Supports)
    {
        return Err(MemoryOrganizerError::new(
            "organized memory requires supporting evidence",
        ));
    }
    let latest_direct_evidence_at = evidence
        .iter()
        .map(|item| item.recorded_at_ms)
        .max()
        .unwrap_or(record.created_at_ms);
    match (&record.status, &record.retraction) {
        (OrganizedMemoryStatus::Retracted, Some(retraction)) => {
            let latest_retraction_evidence_at = retraction
                .evidence
                .iter()
                .map(|item| item.recorded_at_ms)
                .max()
                .unwrap_or(0);
            if normalize_text(&retraction.operation_id, "retraction operation id")?
                != retraction.operation_id
                || normalize_text(&retraction.reason, "retraction reason")? != retraction.reason
                || retraction.retracted_at_ms < latest_direct_evidence_at
                || retraction.retracted_at_ms < latest_retraction_evidence_at
                || retraction.retracted_at_ms > record.updated_at_ms
                || retraction.evidence.is_empty()
                || retraction.evidence.len() > MAX_MEMORY_EVIDENCE_PER_CANDIDATE
                || retraction
                    .evidence
                    .iter()
                    .any(|item| item.polarity != MemoryEvidencePolarity::Supports)
                || normalize_evidence(retraction.evidence.clone())? != retraction.evidence
                || evidence_authority(&retraction.evidence) < record_authority(record)
            {
                return Err(MemoryOrganizerError::new(
                    "organized memory retraction metadata is invalid",
                ));
            }
        }
        (OrganizedMemoryStatus::Retracted, None) => {
            return Err(MemoryOrganizerError::new(
                "retracted memory requires retraction metadata",
            ));
        }
        (_, Some(_)) => {
            return Err(MemoryOrganizerError::new(
                "only a retracted memory may contain retraction metadata",
            ));
        }
        (_, None) => {}
    }
    if record.status == OrganizedMemoryStatus::Confirmed
        && status_from_record(record) != OrganizedMemoryStatus::Confirmed
    {
        return Err(MemoryOrganizerError::new(
            "confirmed memory does not satisfy its evidence policy",
        ));
    }
    Ok(())
}

fn record_from_candidate(
    candidate: &MemoryCandidate,
    status: OrganizedMemoryStatus,
    candidate_fingerprint: String,
) -> OrganizedMemory {
    let created_at_ms = candidate
        .evidence
        .iter()
        .map(|evidence| evidence.recorded_at_ms)
        .min()
        .expect("validated candidate has evidence");
    let updated_at_ms = candidate
        .evidence
        .iter()
        .map(|evidence| evidence.recorded_at_ms)
        .max()
        .expect("validated candidate has evidence");
    OrganizedMemory {
        memory_id: memory_id_for_candidate(&candidate.candidate_id),
        kind: candidate.kind,
        subject: candidate.subject.clone(),
        predicate: candidate.predicate.clone(),
        value: candidate.value.clone(),
        applicability: candidate.applicability.clone(),
        importance: candidate.importance,
        confidence: confidence_from_candidate(candidate),
        status,
        supporting_evidence: candidate
            .evidence
            .iter()
            .filter(|evidence| evidence.polarity == MemoryEvidencePolarity::Supports)
            .cloned()
            .collect(),
        contradicting_evidence: candidate
            .evidence
            .iter()
            .filter(|evidence| evidence.polarity == MemoryEvidencePolarity::Contradicts)
            .cloned()
            .collect(),
        absorbed_candidate_count: 1,
        candidate_fingerprint_samples: BTreeSet::from([candidate_fingerprint]),
        created_at_ms,
        updated_at_ms,
        supersedes_memory_ids: BTreeSet::new(),
        superseded_by_memory_id: None,
        conflicts_with_memory_ids: BTreeSet::new(),
        retraction: None,
    }
}

fn status_from_candidate(candidate: &MemoryCandidate) -> OrganizedMemoryStatus {
    status_from_evidence(
        candidate.kind,
        &candidate.evidence,
        confidence_from_candidate(candidate),
    )
}

fn status_from_record(record: &OrganizedMemory) -> OrganizedMemoryStatus {
    status_from_evidence(
        record.kind,
        &all_evidence(record).cloned().collect::<Vec<_>>(),
        record.confidence,
    )
}

fn status_from_evidence(
    kind: OrganizedMemoryKind,
    evidence: &[MemoryEvidence],
    confidence_hint: f32,
) -> OrganizedMemoryStatus {
    let supports = evidence
        .iter()
        .filter(|item| item.polarity == MemoryEvidencePolarity::Supports)
        .collect::<Vec<_>>();
    let contradictions = evidence
        .iter()
        .filter(|item| item.polarity == MemoryEvidencePolarity::Contradicts)
        .collect::<Vec<_>>();
    let support_strength = noisy_or(
        supports
            .iter()
            .map(|item| weighted_evidence_reliability(item)),
    );
    let contradiction_strength = noisy_or(
        contradictions
            .iter()
            .map(|item| weighted_evidence_reliability(item)),
    );
    if !contradictions.is_empty()
        && contradiction_strength >= 0.6
        && (support_strength - contradiction_strength).abs() <= 0.25
    {
        return OrganizedMemoryStatus::Disputed;
    }
    let reliable_supports = supports
        .iter()
        .filter(|item| item.reliability >= 0.5)
        .copied()
        .collect::<Vec<_>>();
    let has_user_explicit = reliable_supports
        .iter()
        .any(|item| item.origin == MemoryEvidenceOrigin::UserExplicit);
    let confirmation_rule_met = match kind {
        OrganizedMemoryKind::ContextualFact => {
            has_user_explicit
                || reliable_supports.iter().any(|item| {
                    matches!(
                        item.origin,
                        MemoryEvidenceOrigin::VerifiedTaskOutcome
                            | MemoryEvidenceOrigin::ExternalSource
                    ) && item.reliability >= 0.75
                })
                || reliable_supports
                    .iter()
                    .filter(|item| item.origin == MemoryEvidenceOrigin::ObservedBehavior)
                    .count()
                    >= 2
        }
        OrganizedMemoryKind::Preference => {
            has_user_explicit
                || reliable_supports
                    .iter()
                    .filter(|item| item.origin == MemoryEvidenceOrigin::ObservedBehavior)
                    .count()
                    >= 3
        }
        OrganizedMemoryKind::Habit => {
            reliable_supports
                .iter()
                .filter(|item| item.origin != MemoryEvidenceOrigin::ModelInference)
                .map(|item| item.observed_at_ms)
                .collect::<BTreeSet<_>>()
                .len()
                >= 3
        }
        OrganizedMemoryKind::ActionExperience => reliable_supports.iter().any(|item| {
            item.origin == MemoryEvidenceOrigin::VerifiedTaskOutcome && item.reliability >= 0.7
        }),
        OrganizedMemoryKind::Lesson => {
            has_user_explicit
                || reliable_supports
                    .iter()
                    .filter(|item| item.origin == MemoryEvidenceOrigin::VerifiedTaskOutcome)
                    .filter_map(|item| item.run_id.as_deref())
                    .collect::<BTreeSet<_>>()
                    .len()
                    >= 2
        }
    };
    let model_only = !reliable_supports.is_empty()
        && reliable_supports
            .iter()
            .all(|item| item.origin == MemoryEvidenceOrigin::ModelInference);
    if confirmation_rule_met && !model_only && confidence_hint >= 0.5 {
        OrganizedMemoryStatus::Confirmed
    } else {
        OrganizedMemoryStatus::Provisional
    }
}

fn confidence_from_candidate(candidate: &MemoryCandidate) -> f32 {
    confidence_from_evidence(&candidate.evidence, candidate.confidence)
}

fn support_confidence_from_evidence(
    evidence: &[MemoryEvidence],
    extraction_confidence: f32,
) -> f32 {
    let support_strength = noisy_or(
        evidence
            .iter()
            .filter(|item| item.polarity == MemoryEvidencePolarity::Supports)
            .map(weighted_evidence_reliability),
    );
    (extraction_confidence * support_strength).clamp(0.0, 1.0)
}

fn confidence_from_evidence(evidence: &[MemoryEvidence], extraction_confidence: f32) -> f32 {
    let support = noisy_or(
        evidence
            .iter()
            .filter(|item| item.polarity == MemoryEvidencePolarity::Supports)
            .map(weighted_evidence_reliability),
    );
    let confidence = (extraction_confidence * support).clamp(0.0, 1.0);
    penalize_confidence(
        confidence,
        evidence
            .iter()
            .filter(|item| item.polarity == MemoryEvidencePolarity::Contradicts),
    )
}

fn penalize_confidence<'a>(
    confidence: f32,
    contradictions: impl Iterator<Item = &'a MemoryEvidence>,
) -> f32 {
    let contradiction_strength = noisy_or(contradictions.map(weighted_evidence_reliability));
    (confidence * (1.0 - contradiction_strength * 0.75)).clamp(0.0, 1.0)
}

fn weighted_evidence_reliability(evidence: &MemoryEvidence) -> f32 {
    evidence.reliability * (origin_authority(evidence.origin) as f32 / 5.0)
}

fn combine_confidence(existing: f32, additional: f32) -> f32 {
    (1.0 - (1.0 - existing) * (1.0 - additional)).clamp(0.0, 1.0)
}

fn noisy_or(values: impl Iterator<Item = f32>) -> f32 {
    (1.0 - values.fold(1.0, |remaining, value| remaining * (1.0 - value))).clamp(0.0, 1.0)
}

fn candidate_authority(candidate: &MemoryCandidate) -> u8 {
    candidate
        .evidence
        .iter()
        .filter(|item| item.polarity == MemoryEvidencePolarity::Supports && item.reliability >= 0.5)
        .map(|item| origin_authority(item.origin))
        .max()
        .unwrap_or(0)
}

fn record_authority(record: &OrganizedMemory) -> u8 {
    record
        .supporting_evidence
        .iter()
        .filter(|item| item.reliability >= 0.5)
        .map(|item| origin_authority(item.origin))
        .max()
        .unwrap_or(0)
}

fn evidence_authority(evidence: &[MemoryEvidence]) -> u8 {
    evidence
        .iter()
        .filter(|item| item.polarity == MemoryEvidencePolarity::Supports && item.reliability >= 0.5)
        .map(|item| origin_authority(item.origin))
        .max()
        .unwrap_or(0)
}

fn origin_authority(origin: MemoryEvidenceOrigin) -> u8 {
    match origin {
        MemoryEvidenceOrigin::UserExplicit => 5,
        MemoryEvidenceOrigin::VerifiedTaskOutcome => 5,
        MemoryEvidenceOrigin::AgentAction | MemoryEvidenceOrigin::ExternalSource => 4,
        MemoryEvidenceOrigin::ObservedBehavior => 3,
        MemoryEvidenceOrigin::ModelInference => 1,
    }
}

fn authority_is_comparable(left: u8, right: u8) -> bool {
    left.abs_diff(right) <= 1
}

fn ensure_evidence_capacity(
    record: &OrganizedMemory,
    evidence: &[MemoryEvidence],
) -> Result<(), MemoryOrganizerError> {
    for incoming in evidence {
        if all_evidence(record).any(|existing| {
            existing.event_id == incoming.event_id && existing.polarity != incoming.polarity
        }) {
            return Err(MemoryOrganizerError::new(format!(
                "evidence event {} cannot change polarity within one memory",
                incoming.event_id
            )));
        }
    }
    let unique = all_evidence(record)
        .map(|item| item.event_id.as_str())
        .chain(evidence.iter().map(|item| item.event_id.as_str()))
        .collect::<BTreeSet<_>>()
        .len();
    if unique > MAX_MEMORY_EVIDENCE_PER_RECORD {
        return Err(MemoryOrganizerError::new(format!(
            "memory evidence exceeds {MAX_MEMORY_EVIDENCE_PER_RECORD} items"
        )));
    }
    Ok(())
}

fn append_evidence(record: &mut OrganizedMemory, evidence: Vec<MemoryEvidence>) {
    for item in evidence {
        match item.polarity {
            MemoryEvidencePolarity::Supports => {
                push_unique_evidence(&mut record.supporting_evidence, item)
            }
            MemoryEvidencePolarity::Contradicts => {
                push_unique_evidence(&mut record.contradicting_evidence, item)
            }
        }
    }
}

fn insert_candidate_fingerprint_sample(record: &mut OrganizedMemory, fingerprint: String) {
    if record.candidate_fingerprint_samples.contains(&fingerprint) {
        return;
    }
    if record.candidate_fingerprint_samples.len() >= MAX_MEMORY_CANDIDATE_FINGERPRINT_SAMPLES {
        if let Some(evicted) = record.candidate_fingerprint_samples.iter().next().cloned() {
            record.candidate_fingerprint_samples.remove(&evicted);
        }
    }
    record.candidate_fingerprint_samples.insert(fingerprint);
}

fn push_unique_evidence(target: &mut Vec<MemoryEvidence>, evidence: MemoryEvidence) {
    if !target
        .iter()
        .any(|existing| existing.event_id == evidence.event_id)
    {
        target.push(evidence);
        target.sort_by(|left, right| left.event_id.cmp(&right.event_id));
    }
}

fn all_evidence(record: &OrganizedMemory) -> impl Iterator<Item = &MemoryEvidence> {
    record
        .supporting_evidence
        .iter()
        .chain(record.contradicting_evidence.iter())
}

pub(super) fn all_record_evidence(
    record: &OrganizedMemory,
) -> impl Iterator<Item = &MemoryEvidence> {
    all_evidence(record).chain(
        record
            .retraction
            .iter()
            .flat_map(|retraction| retraction.evidence.iter()),
    )
}

fn time_ranges_overlap(left: &MemoryApplicability, right: &MemoryApplicability) -> bool {
    let left_end = left.valid_until_ms.unwrap_or(i64::MAX);
    let right_end = right.valid_until_ms.unwrap_or(i64::MAX);
    left.valid_from_ms < right_end && right.valid_from_ms < left_end
}

fn environment_scopes_are_disjoint(
    left: &BTreeMap<String, String>,
    right: &BTreeMap<String, String>,
) -> bool {
    left.iter()
        .any(|(key, value)| right.get(key).is_some_and(|other| other != value))
}

/// 相同范围会直接冲突；互不包含但可同时满足的部分重叠范围也会冲突。严格的
/// 父/子范围被视为“默认规则 + 局部例外”，查询时由更具体范围覆盖更宽范围。
fn environment_scopes_conflict(
    left: &BTreeMap<String, String>,
    right: &BTreeMap<String, String>,
) -> bool {
    if environment_scopes_are_disjoint(left, right) {
        return false;
    }
    let left_contains_right = right
        .iter()
        .all(|(key, value)| left.get(key) == Some(value));
    let right_contains_left = left
        .iter()
        .all(|(key, value)| right.get(key) == Some(value));
    left == right || (!left_contains_right && !right_contains_left)
}

fn merge_time_range(target: &mut MemoryApplicability, incoming: &MemoryApplicability) {
    target.valid_from_ms = target.valid_from_ms.min(incoming.valid_from_ms);
    target.valid_until_ms = match (target.valid_until_ms, incoming.valid_until_ms) {
        (Some(left), Some(right)) => Some(left.max(right)),
        _ => None,
    };
}

fn is_live_status(status: OrganizedMemoryStatus) -> bool {
    matches!(
        status,
        OrganizedMemoryStatus::Provisional
            | OrganizedMemoryStatus::Confirmed
            | OrganizedMemoryStatus::Disputed
    )
}

fn reconciled_status(
    state: &MemoryOrganizerState,
    record: &OrganizedMemory,
) -> OrganizedMemoryStatus {
    let mut status = status_from_record(record);
    if status != OrganizedMemoryStatus::Confirmed {
        return status;
    }
    let authority = record_authority(record);
    for conflict_id in &record.conflicts_with_memory_ids {
        let Some(other) = state.records.get(conflict_id) else {
            continue;
        };
        if !is_live_status(other.status)
            || !time_ranges_overlap(&record.applicability, &other.applicability)
            || !environment_scopes_conflict(
                &record.applicability.environment,
                &other.applicability.environment,
            )
            || status_from_record(other) != OrganizedMemoryStatus::Confirmed
        {
            continue;
        }
        let other_authority = record_authority(other);
        if authority_is_comparable(authority, other_authority) {
            return OrganizedMemoryStatus::Disputed;
        }
        if other_authority > authority {
            status = OrganizedMemoryStatus::Provisional;
        }
    }
    status
}

fn memory_id_for_candidate(candidate_id: &str) -> String {
    format!("{ORGANIZED_MEMORY_ID_PREFIX}{candidate_id}")
}

fn fingerprint_candidate(candidate: &MemoryCandidate) -> String {
    fingerprint_serializable(candidate)
}

fn rewrite_candidate_evidence_actor(
    mut candidate: MemoryCandidate,
    from_actor_id: &str,
    to_actor_id: &str,
) -> MemoryCandidate {
    rewrite_evidence_actor_ids(&mut candidate.evidence, from_actor_id, to_actor_id);
    candidate
}

fn rewrite_resolution_evidence_actor(
    mut request: ResolveMemoryDisputeRequest,
    from_actor_id: &str,
    to_actor_id: &str,
) -> ResolveMemoryDisputeRequest {
    rewrite_evidence_actor_ids(&mut request.evidence, from_actor_id, to_actor_id);
    request
}

fn rewrite_evidence_actor_ids(
    evidence: &mut [MemoryEvidence],
    from_actor_id: &str,
    to_actor_id: &str,
) {
    for item in evidence {
        if item.source_actor_id == from_actor_id {
            item.source_actor_id = to_actor_id.to_string();
        }
    }
}

pub(super) fn fingerprint_evidence_metadata(evidence: &MemoryEvidence) -> EvidenceDigest {
    // polarity 是“该事件相对于当前主张”的关系，不是事件自身元数据。
    digest_serializable(&(
        &evidence.event_id,
        &evidence.source_actor_id,
        evidence.origin,
        evidence.observed_at_ms,
        evidence.recorded_at_ms,
        evidence.reliability,
        &evidence.mission_id,
        &evidence.run_id,
    ))
}

fn fingerprint_serializable(value: &impl Serialize) -> String {
    let digest = digest_serializable(value);
    hex_digest(digest)
}

fn digest_serializable(value: &impl Serialize) -> EvidenceDigest {
    let bytes = serde_json::to_vec(value).expect("normalized memory input must serialize");
    fingerprint_bytes(&bytes)
}

pub(super) fn fingerprint_bytes(bytes: &[u8]) -> EvidenceDigest {
    Sha256::digest(bytes).into()
}

fn is_canonical_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn hex_digest(digest: EvidenceDigest) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in digest {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn looks_like_task_runtime_state(subject: &str, predicate: &str) -> bool {
    let subject = subject.to_ascii_lowercase();
    let task_subject = ["mission:", "run:", "task:", "work:", "job:", "step:"]
        .iter()
        .any(|prefix| subject.starts_with(prefix));
    if !task_subject {
        return false;
    }
    let predicate_lower = predicate.to_ascii_lowercase();
    if ["状态", "进度", "下一步", "待处理", "检查点"]
        .iter()
        .any(|marker| predicate_lower.contains(marker))
    {
        return true;
    }
    let predicate = predicate_lower
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    [
        "status",
        "progress",
        "nextstep",
        "pending",
        "checkpoint",
        "currentstep",
    ]
    .iter()
    .any(|marker| predicate.contains(marker))
}

fn record_is_visible(record: &OrganizedMemory, query: &OrganizedMemoryQuery) -> bool {
    let status_visible = match record.status {
        OrganizedMemoryStatus::Confirmed => true,
        OrganizedMemoryStatus::Provisional => query.include_provisional,
        OrganizedMemoryStatus::Disputed => query.include_disputed,
        OrganizedMemoryStatus::Superseded
        | OrganizedMemoryStatus::Retracted
        | OrganizedMemoryStatus::Expired => false,
    };
    status_visible
        && record.applicability.space_id == query.space_id
        && record.applicability.valid_from_ms <= query.current_at_ms
        && record
            .applicability
            .valid_until_ms
            .is_none_or(|valid_until_ms| query.current_at_ms < valid_until_ms)
        && record
            .applicability
            .environment
            .iter()
            .all(|(key, value)| query.environment.get(key) == Some(value))
}

fn selection_score(record: &OrganizedMemory, query: &OrganizedMemoryQuery) -> f32 {
    let age_ms = query
        .current_at_ms
        .saturating_sub(record.updated_at_ms)
        .max(0) as f32;
    let age_days = age_ms / 86_400_000.0;
    let recency = 1.0 / (1.0 + age_days / 30.0);
    let authority = record_authority(record) as f32 / 5.0;
    let focus = if query.focus_terms.is_empty() {
        0.0
    } else {
        let searchable = format!(
            "{} {} {}",
            record.subject,
            record.predicate,
            serde_json::to_string(&record.value).unwrap_or_default()
        )
        .to_lowercase();
        let focus_matches = query
            .focus_terms
            .iter()
            .filter(|term| searchable.contains(term.as_str()))
            .count();
        focus_matches as f32 / query.focus_terms.len() as f32
    };
    let status_bonus = match record.status {
        OrganizedMemoryStatus::Confirmed => 1.0,
        OrganizedMemoryStatus::Provisional => 0.35,
        OrganizedMemoryStatus::Disputed => 0.15,
        _ => 0.0,
    };
    (record.importance * 0.4
        + record.confidence * 0.2
        + focus * 0.2
        + recency * 0.1
        + authority * 0.05
        + status_bonus * 0.05)
        .clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests;
