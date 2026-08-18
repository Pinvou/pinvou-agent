//! PinvouOS Runtime 与确定性 Memory Organizer 之间的可信边界。
//!
//! 这里不读取文件、不启动线程，也不依赖其他 Agent。Runtime 先从统一事件账本
//! 验证证据，再把不可伪造的 envelope 元数据交给本适配器；候选提取器提供的
//! actor/origin/time/reliability 会被完全覆盖。

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::decision::OrganizedMemoryDecisionEngine;
use super::domain::{
    MemoryApplicability, MemoryCandidate, MemoryCandidateIntent, MemoryEvidence,
    MemoryEvidenceOrigin, MemoryEvidencePolarity, MemoryOrganizationAction,
    MemoryOrganizationReceipt, OrganizedMemory, OrganizedMemoryKind, OrganizedMemoryQuery,
    ResolveMemoryDisputeRequest, RetractOrganizedMemoryRequest,
};
use super::{
    CompileMemoryContextRequest, MemoryContextItem, MemoryContextProjection, MemoryContextStatus,
    MemoryMutationReceipt, MemoryProjectionState, MemoryRecordStatus, MemoryTier,
    RememberMemoryRequest, RetractMemoryRequest,
};

const DEFAULT_MEMORY_SPACE_ID: &str = "personal";
const LEGACY_IMPORT_RELIABILITY: f32 = 0.45;
const DEFAULT_IMPORTANCE: f32 = 0.5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustedMemoryEvidence {
    pub event_id: String,
    pub source_actor_id: String,
    pub origin: MemoryEvidenceOrigin,
    pub observed_at_ms: i64,
    pub recorded_at_ms: i64,
    pub reliability: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// 只有 Runtime 能从结构化事件 payload 派生这个绑定。候选不能只引用一个
    /// “确实存在”的无关事件来冒充内容证据。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding: Option<TrustedMemoryEvidenceBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TrustedMemoryEvidenceBinding {
    Assertion {
        claim_id: String,
        subject: String,
        predicate: String,
        value: Value,
    },
    Retraction {
        claim_id: String,
        subject: String,
        predicate: String,
        value: Value,
    },
}

impl TrustedMemoryEvidence {
    fn attest(&self, polarity: MemoryEvidencePolarity) -> MemoryEvidence {
        MemoryEvidence {
            event_id: self.event_id.clone(),
            source_actor_id: self.source_actor_id.clone(),
            origin: self.origin,
            polarity,
            observed_at_ms: self.observed_at_ms,
            recorded_at_ms: self.recorded_at_ms,
            reliability: self.reliability,
            mission_id: self.mission_id.clone(),
            run_id: self.run_id.clone(),
        }
    }

    pub fn attested_support(&self) -> MemoryEvidence {
        self.attest(MemoryEvidencePolarity::Supports)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyMemoryMigrationReport {
    pub imported_durable_records: usize,
    pub skipped_working_records: usize,
    pub skipped_inactive_records: usize,
    pub skipped_unsafe_or_invalid_records: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRuntimeAdapterError {
    message: String,
}

impl MemoryRuntimeAdapterError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for MemoryRuntimeAdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for MemoryRuntimeAdapterError {}

pub fn attest_memory_candidate(
    mut candidate: MemoryCandidate,
    trusted: &BTreeMap<String, TrustedMemoryEvidence>,
) -> Result<MemoryCandidate, MemoryRuntimeAdapterError> {
    if candidate.kind != OrganizedMemoryKind::ContextualFact {
        return Err(MemoryRuntimeAdapterError::new(
            "Runtime currently accepts only contextual facts with an exact structured claim; verified task outcomes and user preference statements are not connected yet",
        ));
    }
    if candidate.intent != MemoryCandidateIntent::Assert || candidate.target_memory_id.is_some() {
        return Err(MemoryRuntimeAdapterError::new(
            "a structured assertion cannot authorize replacement or contradiction semantics",
        ));
    }
    candidate.evidence = attest_candidate_evidence(&candidate, trusted)?;
    // 第一阶段只有稳定的个人身份空间。空间路由以后必须由受信身份适配器签发，
    // 不能让模型或插件把同一事实写进任意身份空间。
    candidate.applicability.space_id = DEFAULT_MEMORY_SPACE_ID.to_string();
    // 环境和有效期同样属于来源语义，而当前 Claim 协议尚未携带它们。先安全地
    // 投影为“从该声明发生时起、无额外环境约束”，不能沿用调用方自报范围。
    candidate.applicability.environment.clear();
    candidate.applicability.valid_from_ms = candidate
        .evidence
        .iter()
        .map(|evidence| evidence.observed_at_ms)
        .min()
        .unwrap_or(0);
    candidate.applicability.valid_until_ms = None;
    Ok(candidate)
}

pub fn attest_memory_retraction(
    mut request: RetractOrganizedMemoryRequest,
    trusted: &BTreeMap<String, TrustedMemoryEvidence>,
    evidence_index: &BTreeMap<String, TrustedMemoryEvidence>,
    target: &OrganizedMemory,
) -> Result<RetractOrganizedMemoryRequest, MemoryRuntimeAdapterError> {
    request.evidence =
        attest_retraction_evidence(request.evidence, trusted, evidence_index, target)?;
    request.retracted_at_ms = request
        .evidence
        .iter()
        .map(|evidence| evidence.observed_at_ms.max(evidence.recorded_at_ms))
        .max()
        .unwrap_or(0)
        .max(target.updated_at_ms);
    Ok(request)
}

pub fn attest_memory_resolution(
    _request: ResolveMemoryDisputeRequest,
    _trusted: &BTreeMap<String, TrustedMemoryEvidence>,
) -> Result<ResolveMemoryDisputeRequest, MemoryRuntimeAdapterError> {
    Err(MemoryRuntimeAdapterError::new(
        "trusted dispute-resolution events are not connected yet",
    ))
}

fn attest_candidate_evidence(
    candidate: &MemoryCandidate,
    trusted: &BTreeMap<String, TrustedMemoryEvidence>,
) -> Result<Vec<MemoryEvidence>, MemoryRuntimeAdapterError> {
    if candidate.evidence.is_empty() {
        return Err(MemoryRuntimeAdapterError::new(
            "memory mutation requires at least one evidence event",
        ));
    }
    candidate
        .evidence
        .iter()
        .map(|proposed| {
            let attestation = trusted.get(&proposed.event_id).ok_or_else(|| {
                MemoryRuntimeAdapterError::new(format!(
                    "unknown or ineligible memory evidence event {}",
                    proposed.event_id
                ))
            })?;
            let Some(TrustedMemoryEvidenceBinding::Assertion {
                subject,
                predicate,
                value,
                ..
            }) = &attestation.binding
            else {
                return Err(MemoryRuntimeAdapterError::new(format!(
                    "memory evidence event {} is not a structured assertion",
                    proposed.event_id
                )));
            };
            if subject != &candidate.subject
                || predicate != &candidate.predicate
                || value != &candidate.value
            {
                return Err(MemoryRuntimeAdapterError::new(format!(
                    "memory evidence event {} does not exactly support the candidate payload",
                    proposed.event_id
                )));
            }
            // Polarity is part of the trusted semantic binding, not caller-controlled metadata.
            Ok(attestation.attest(MemoryEvidencePolarity::Supports))
        })
        .collect()
}

fn attest_retraction_evidence(
    proposed: Vec<MemoryEvidence>,
    trusted: &BTreeMap<String, TrustedMemoryEvidence>,
    evidence_index: &BTreeMap<String, TrustedMemoryEvidence>,
    target: &OrganizedMemory,
) -> Result<Vec<MemoryEvidence>, MemoryRuntimeAdapterError> {
    if proposed.is_empty() {
        return Err(MemoryRuntimeAdapterError::new(
            "memory mutation requires at least one evidence event",
        ));
    }
    // 当前协议还没有“只撤掉一条 supporting evidence、随后重新计算记录状态”的
    // decision。为避免撤回 claim A 时连带删除仍由 claim B 支撑的合并记忆，这里先
    // fail closed：只有恰好由一条结构化 claim 支撑的记录才能被整条撤回。
    if target.supporting_evidence.len() != 1 {
        return Err(MemoryRuntimeAdapterError::new(
            "a whole-memory retraction requires exactly one supporting structured claim; evidence withdrawal is not connected yet",
        ));
    }
    let supporting_event_id = &target.supporting_evidence[0].event_id;
    let Some(TrustedMemoryEvidenceBinding::Assertion {
        claim_id: supporting_claim_id,
        ..
    }) = evidence_index
        .get(supporting_event_id)
        .and_then(|evidence| evidence.binding.as_ref())
    else {
        return Err(MemoryRuntimeAdapterError::new(
            "the target memory is not backed by one traceable structured claim",
        ));
    };

    proposed
        .into_iter()
        .map(|evidence| {
            let attestation = trusted.get(&evidence.event_id).ok_or_else(|| {
                MemoryRuntimeAdapterError::new(format!(
                    "unknown or ineligible memory evidence event {}",
                    evidence.event_id
                ))
            })?;
            let Some(TrustedMemoryEvidenceBinding::Retraction {
                claim_id,
                subject,
                predicate,
                value,
            }) = &attestation.binding
            else {
                return Err(MemoryRuntimeAdapterError::new(format!(
                    "memory evidence event {} is not a structured claim retraction",
                    evidence.event_id
                )));
            };
            if subject != &target.subject
                || predicate != &target.predicate
                || value != &target.value
                || claim_id != supporting_claim_id
            {
                return Err(MemoryRuntimeAdapterError::new(format!(
                    "memory evidence event {} does not retract the target's sole supporting claim",
                    evidence.event_id
                )));
            }
            Ok(attestation.attest(MemoryEvidencePolarity::Supports))
        })
        .collect()
}

pub fn legacy_remember_candidate(
    request: &RememberMemoryRequest,
    trusted: &BTreeMap<String, TrustedMemoryEvidence>,
) -> Result<MemoryCandidate, MemoryRuntimeAdapterError> {
    if request.tier == MemoryTier::Working {
        return Err(MemoryRuntimeAdapterError::new(
            "working task state belongs to the Task Kernel, not long-term memory",
        ));
    }
    let evidence = request
        .evidence_event_ids
        .iter()
        .map(|event_id| MemoryEvidence {
            event_id: event_id.clone(),
            source_actor_id: String::new(),
            origin: MemoryEvidenceOrigin::ModelInference,
            polarity: MemoryEvidencePolarity::Supports,
            observed_at_ms: 0,
            recorded_at_ms: 0,
            reliability: 0.0,
            mission_id: None,
            run_id: None,
        })
        .collect();
    attest_memory_candidate(
        MemoryCandidate {
            candidate_id: stable_id("compat", &request.memory_id),
            kind: OrganizedMemoryKind::ContextualFact,
            subject: request.subject.clone(),
            predicate: request.predicate.clone(),
            value: request.value.clone(),
            applicability: MemoryApplicability {
                space_id: DEFAULT_MEMORY_SPACE_ID.to_string(),
                environment: BTreeMap::new(),
                valid_from_ms: request.observed_at_ms,
                valid_until_ms: None,
            },
            importance: DEFAULT_IMPORTANCE,
            confidence: request.confidence,
            intent: MemoryCandidateIntent::Assert,
            target_memory_id: None,
            evidence,
        },
        trusted,
    )
}

pub fn legacy_retraction_request(
    request: &RetractMemoryRequest,
    engine: &OrganizedMemoryDecisionEngine,
    trusted: &BTreeMap<String, TrustedMemoryEvidence>,
    evidence_index: &BTreeMap<String, TrustedMemoryEvidence>,
) -> Result<RetractOrganizedMemoryRequest, MemoryRuntimeAdapterError> {
    let memory_id = resolve_legacy_memory_id(engine, &request.memory_id).ok_or_else(|| {
        MemoryRuntimeAdapterError::new(format!("unknown memory {}", request.memory_id))
    })?;
    let evidence = request
        .evidence_event_ids
        .iter()
        .map(|event_id| MemoryEvidence {
            event_id: event_id.clone(),
            source_actor_id: String::new(),
            origin: MemoryEvidenceOrigin::ModelInference,
            polarity: MemoryEvidencePolarity::Supports,
            observed_at_ms: 0,
            recorded_at_ms: 0,
            reliability: 0.0,
            mission_id: None,
            run_id: None,
        })
        .collect::<Vec<_>>();
    let operation_seed = serde_json::to_vec(request)
        .map_err(|error| MemoryRuntimeAdapterError::new(error.to_string()))?;
    let target = engine
        .organizer()
        .state()
        .records
        .get(&memory_id)
        .ok_or_else(|| MemoryRuntimeAdapterError::new(format!("unknown memory {memory_id}")))?;
    attest_memory_retraction(
        RetractOrganizedMemoryRequest {
            operation_id: stable_id_from_bytes("compat-retract", &operation_seed),
            memory_id,
            reason: request.reason.clone(),
            retracted_at_ms: request.retracted_at_ms,
            evidence,
        },
        trusted,
        evidence_index,
        target,
    )
}

pub fn legacy_organization_receipt(
    receipt: &MemoryOrganizationReceipt,
    evidence_event_ids: Vec<String>,
) -> MemoryMutationReceipt {
    let memory_id = format!("memory:organized:{}", receipt.candidate_id);
    MemoryMutationReceipt {
        memory_id,
        revision: receipt.revision,
        status: MemoryRecordStatus::Active,
        superseded_memory_id: (receipt.action == MemoryOrganizationAction::Superseded)
            .then(|| receipt.affected_memory_ids.first().cloned())
            .flatten(),
        evidence_event_ids: sorted_unique(evidence_event_ids),
    }
}

pub fn legacy_retraction_receipt(
    memory_id: String,
    revision: u64,
    evidence_event_ids: Vec<String>,
) -> MemoryMutationReceipt {
    MemoryMutationReceipt {
        memory_id,
        revision,
        status: MemoryRecordStatus::Retracted,
        superseded_memory_id: None,
        evidence_event_ids: sorted_unique(evidence_event_ids),
    }
}

pub fn legacy_context_projection(
    engine: &OrganizedMemoryDecisionEngine,
    request: CompileMemoryContextRequest,
    now_ms: i64,
) -> Result<MemoryContextProjection, MemoryRuntimeAdapterError> {
    if request.max_items == 0 || request.max_items > super::MAX_CONTEXT_ITEMS {
        return Err(MemoryRuntimeAdapterError::new(format!(
            "memory context max_items must be between 1 and {}",
            super::MAX_CONTEXT_ITEMS
        )));
    }
    if !request.include_durable_facts {
        return Ok(MemoryContextProjection {
            identity_id: super::super::model::PINVOU_IDENTITY_ID.to_string(),
            revision: engine.organizer().state().revision,
            compiled_at_ms: now_ms,
            status: MemoryContextStatus::Empty,
            items: Vec::new(),
            omitted_count: 0,
            evidence_event_ids: Vec::new(),
        });
    }
    let projection = engine
        .organizer()
        .project(OrganizedMemoryQuery {
            current_at_ms: now_ms,
            space_id: DEFAULT_MEMORY_SPACE_ID.to_string(),
            subjects: request.subjects,
            predicates: request.predicates,
            max_items: request.max_items,
            ..OrganizedMemoryQuery::default()
        })
        .map_err(|error| MemoryRuntimeAdapterError::new(error.to_string()))?;
    let items = projection
        .items
        .iter()
        .filter_map(|item| {
            let record = engine.organizer().record(&item.memory_id)?;
            let evidence = record
                .supporting_evidence
                .first()
                .or_else(|| record.contradicting_evidence.first());
            Some(MemoryContextItem {
                memory_id: item.memory_id.clone(),
                tier: MemoryTier::DurableFact,
                subject: item.subject.clone(),
                predicate: item.predicate.clone(),
                value: item.value.clone(),
                confidence: item.confidence,
                observed_at_ms: item.applicability.valid_from_ms,
                source_actor_id: evidence
                    .map(|value| value.source_actor_id.clone())
                    .unwrap_or_else(|| super::MEMORY_AGENT_ID.to_string()),
                evidence_event_ids: item.evidence_event_ids.clone(),
                mission_id: evidence.and_then(|value| value.mission_id.clone()),
                run_id: evidence.and_then(|value| value.run_id.clone()),
            })
        })
        .collect::<Vec<_>>();
    let status = if items.is_empty() {
        MemoryContextStatus::Empty
    } else if projection.omitted_count > 0 {
        MemoryContextStatus::Partial
    } else {
        MemoryContextStatus::Ready
    };
    Ok(MemoryContextProjection {
        identity_id: super::super::model::PINVOU_IDENTITY_ID.to_string(),
        revision: projection.revision,
        compiled_at_ms: projection.generated_at_ms,
        status,
        items,
        omitted_count: projection.omitted_count,
        evidence_event_ids: projection.evidence_event_ids,
    })
}

pub fn migrate_legacy_memory_projection(
    projection: &MemoryProjectionState,
    projection_event: &TrustedMemoryEvidence,
) -> Result<(OrganizedMemoryDecisionEngine, LegacyMemoryMigrationReport), MemoryRuntimeAdapterError>
{
    let mut engine = OrganizedMemoryDecisionEngine::new();
    let mut report = LegacyMemoryMigrationReport::default();
    for record in projection.records.values() {
        if record.status != MemoryRecordStatus::Active {
            report.skipped_inactive_records = report.skipped_inactive_records.saturating_add(1);
            continue;
        }
        if record.tier == MemoryTier::Working {
            report.skipped_working_records = report.skipped_working_records.saturating_add(1);
            continue;
        }
        let candidate = MemoryCandidate {
            candidate_id: stable_id("legacy", &record.memory_id),
            kind: OrganizedMemoryKind::ContextualFact,
            subject: record.subject.clone(),
            predicate: record.predicate.clone(),
            value: record.value.clone(),
            applicability: MemoryApplicability {
                space_id: DEFAULT_MEMORY_SPACE_ID.to_string(),
                environment: BTreeMap::new(),
                valid_from_ms: record.observed_at_ms.max(0),
                valid_until_ms: None,
            },
            importance: DEFAULT_IMPORTANCE,
            confidence: record.confidence,
            intent: MemoryCandidateIntent::Assert,
            target_memory_id: None,
            evidence: vec![MemoryEvidence {
                event_id: projection_event.event_id.clone(),
                source_actor_id: projection_event.source_actor_id.clone(),
                origin: MemoryEvidenceOrigin::ModelInference,
                polarity: MemoryEvidencePolarity::Supports,
                observed_at_ms: projection_event.observed_at_ms,
                recorded_at_ms: projection_event.recorded_at_ms,
                reliability: LEGACY_IMPORT_RELIABILITY,
                mission_id: projection_event.mission_id.clone(),
                run_id: projection_event.run_id.clone(),
            }],
        };
        match engine.organize(candidate) {
            Ok(_) => {
                report.imported_durable_records = report.imported_durable_records.saturating_add(1);
            }
            Err(_) => {
                // 旧格式允许任务状态、明文凭据和 64 KiB value。它们不能为了
                // “迁移成功”绕过新内核边界；旧账本仍保留原始审计记录。
                report.skipped_unsafe_or_invalid_records =
                    report.skipped_unsafe_or_invalid_records.saturating_add(1);
            }
        }
    }
    Ok((engine, report))
}

pub fn resolve_legacy_memory_id(
    engine: &OrganizedMemoryDecisionEngine,
    requested: &str,
) -> Option<String> {
    if engine.organizer().record(requested).is_some() {
        return Some(requested.to_string());
    }
    for candidate_id in [
        stable_id("compat", requested),
        stable_id("legacy", requested),
    ] {
        if let Some(processed) = engine
            .organizer()
            .state()
            .processed_candidates
            .get(&candidate_id)
        {
            if let Some(memory_id) = processed
                .memory_ids
                .iter()
                .find(|memory_id| engine.organizer().record(memory_id).is_some())
            {
                return Some(memory_id.clone());
            }
        }
    }
    None
}

fn stable_id(namespace: &str, value: &str) -> String {
    stable_id_from_bytes(namespace, value.as_bytes())
}

fn stable_id_from_bytes(namespace: &str, value: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(value);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    format!("{namespace}-{encoded}")
}

fn sorted_unique(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::domain::OrganizedMemoryStatus;
    use super::*;
    use serde_json::json;

    fn trusted(event_id: &str, origin: MemoryEvidenceOrigin) -> TrustedMemoryEvidence {
        TrustedMemoryEvidence {
            event_id: event_id.to_string(),
            source_actor_id: "actor:user".to_string(),
            origin,
            observed_at_ms: 10,
            recorded_at_ms: 10,
            reliability: 1.0,
            mission_id: None,
            run_id: None,
            binding: Some(TrustedMemoryEvidenceBinding::Assertion {
                claim_id: "claim:one".to_string(),
                subject: "user".to_string(),
                predicate: "name".to_string(),
                value: json!("白浪"),
            }),
        }
    }

    #[test]
    fn attestation_overwrites_forged_authority_and_identity_space() {
        let event = trusted("event:one", MemoryEvidenceOrigin::AgentAction);
        let candidate = MemoryCandidate {
            candidate_id: "candidate:one".to_string(),
            kind: OrganizedMemoryKind::ContextualFact,
            subject: "user".to_string(),
            predicate: "name".to_string(),
            value: json!("白浪"),
            applicability: MemoryApplicability {
                space_id: "someone-else".to_string(),
                environment: BTreeMap::new(),
                valid_from_ms: 10,
                valid_until_ms: None,
            },
            importance: 0.8,
            confidence: 1.0,
            intent: MemoryCandidateIntent::Assert,
            target_memory_id: None,
            evidence: vec![MemoryEvidence {
                event_id: event.event_id.clone(),
                source_actor_id: "actor:user".to_string(),
                origin: MemoryEvidenceOrigin::UserExplicit,
                polarity: MemoryEvidencePolarity::Supports,
                observed_at_ms: 999,
                recorded_at_ms: 999,
                reliability: 1.0,
                mission_id: None,
                run_id: None,
            }],
        };
        let result = attest_memory_candidate(
            candidate,
            &BTreeMap::from([(event.event_id.clone(), event.clone())]),
        )
        .unwrap();
        assert_eq!(result.applicability.space_id, DEFAULT_MEMORY_SPACE_ID);
        assert_eq!(
            result.evidence[0],
            event.attest(MemoryEvidencePolarity::Supports)
        );
    }

    #[test]
    fn legacy_import_is_provisional_and_drops_working_state() {
        let mut projection = MemoryProjectionState::default();
        projection.records.insert(
            "legacy:durable".to_string(),
            super::super::MemoryRecord {
                memory_id: "legacy:durable".to_string(),
                tier: MemoryTier::DurableFact,
                subject: "user".to_string(),
                predicate: "name".to_string(),
                value: json!("白浪"),
                confidence: 1.0,
                source_actor_id: "actor:user".to_string(),
                evidence_event_ids: vec!["event:old".to_string()],
                observed_at_ms: 1,
                recorded_at_ms: 1,
                mission_id: None,
                run_id: None,
                status: MemoryRecordStatus::Active,
                supersedes_memory_id: None,
                superseded_by_memory_id: None,
                retraction_reason: None,
                retracted_at_ms: None,
                retracted_by_actor_id: None,
            },
        );
        let mut working = projection.records["legacy:durable"].clone();
        working.memory_id = "legacy:working".to_string();
        working.tier = MemoryTier::Working;
        projection
            .records
            .insert(working.memory_id.clone(), working);

        let (engine, report) = migrate_legacy_memory_projection(
            &projection,
            &trusted("event:projection", MemoryEvidenceOrigin::AgentAction),
        )
        .unwrap();
        assert_eq!(report.imported_durable_records, 1);
        assert_eq!(report.skipped_working_records, 1);
        assert_eq!(engine.organizer().state().records.len(), 1);
        assert_eq!(
            engine
                .organizer()
                .state()
                .records
                .values()
                .next()
                .unwrap()
                .status,
            OrganizedMemoryStatus::Provisional
        );
    }
}
