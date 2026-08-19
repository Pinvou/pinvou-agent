use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{BufReader, Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _, Result};
use fs2::FileExt as _;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::asr_context_agent::{asr_context_compile_contract, ASR_CONTEXT_AGENT_ID};
use super::attention_agent::attention_allocate_contract;
use super::capability_agent::{
    capability_agent_capabilities, CapabilityAgent, CapabilityReportRequest,
};
use super::connectivity_agent::{connectivity_observe_contract, CONNECTIVITY_AGENT_ID};
use super::governor::{classify_pressure, directive_for_agent, ResourceGovernorPolicy};
use super::inference_agent::{inference_observe_contract, INFERENCE_AGENT_ID};
use super::memory_agent::{
    attest_memory_candidate, attest_memory_resolution, attest_memory_retraction,
    legacy_context_projection, legacy_organization_receipt, legacy_remember_candidate,
    legacy_retraction_receipt, legacy_retraction_request, memory_capabilities,
    migrate_legacy_memory_projection, CompileMemoryContextRequest, LegacyMemoryMigrationReport,
    MemoryAgent, MemoryCandidate, MemoryContextProjection, MemoryDisputeResolutionReceipt,
    MemoryEvidence, MemoryEvidenceOrigin, MemoryMaintenanceReport, MemoryMutationReceipt,
    MemoryOrganizationReceipt, MemoryProjectionState, OrganizedMemory,
    OrganizedMemoryDecisionBatch, OrganizedMemoryDecisionCheckpoint, OrganizedMemoryDecisionEngine,
    OrganizedMemoryProjection, OrganizedMemoryQuery, RememberMemoryRequest,
    ResolveMemoryDisputeRequest, RetractMemoryRequest, RetractOrganizedMemoryRequest,
    TrustedMemoryEvidence, TrustedMemoryEvidenceBinding, MEMORY_AGENT_ID,
};
use super::model::*;
use super::policy_agent::policy_authorize_contract;
use super::screen_observer_agent::{
    canonical_screen_observer_agent_id, canonical_screen_observer_capability_id,
    screen_observe_contract, LEGACY_SURFACE_AGENT_ID, LEGACY_SURFACE_OBSERVE_CAPABILITY_ID,
    SCREEN_OBSERVER_AGENT_ID, SCREEN_OBSERVER_IDENTITY_SCHEMA_VERSION,
};

type RuntimeEventSink = dyn Fn(EventEnvelope) + Send + Sync + 'static;
pub type AgentControlAdapter =
    Arc<dyn Fn(ControlDirective) -> std::result::Result<String, String> + Send + Sync + 'static>;

#[derive(Clone)]
pub struct PinvouOsRuntime {
    inner: Arc<RuntimeInner>,
}

struct RuntimeInner {
    ledger_path: PathBuf,
    /// 当前 Runtime 实例的单写者租约。旧二进制不认识这个锁，因此每次 append 仍会
    /// 在账本文件锁内做 durable-head CAS；发现外部写入就 fail-stop。
    _writer_lease: fs::File,
    append_lock: Mutex<()>,
    durable_head: RwLock<Option<LedgerHead>>,
    write_failure: RwLock<Option<String>>,
    snapshot: RwLock<RuntimeSnapshot>,
    next_sequence: AtomicU64,
    event_sink: RwLock<Option<Arc<RuntimeEventSink>>>,
    governor_policy: ResourceGovernorPolicy,
    memory_engine: RwLock<MemoryEngineState>,
    /// 从统一账本 envelope 派生的最小可信元数据索引；不是第二真相源，重启可重建。
    memory_evidence_index: RwLock<BTreeMap<String, TrustedMemoryEvidence>>,
    control_adapters: RwLock<BTreeMap<String, AgentControlAdapter>>,
    /// 本进程已经交给外部 Adapter 的 Directive。claim 在进程生命周期内不释放：
    /// Adapter 返回后由同一 owner 落 ACK；若进程在两者之间崩溃，重启会携带相同
    /// directive_id 重试，Adapter 必须用该 identity 实现外部副作用幂等。
    dispatched_directive_ids: Mutex<BTreeSet<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectiveAcknowledgementOwner {
    External,
    RegisteredAdapter,
}

#[derive(Debug)]
enum MemoryEngineState {
    Ready(OrganizedMemoryDecisionEngine),
    /// 一次 tentative 决策未能确认耐久化后，禁止继续读写该实例。重启会只从
    /// 已落账 checkpoint + tail 重建，避免把下一条 hash 接到不确定的头上。
    Poisoned {
        reason: String,
    },
}

struct ReplayedMemoryEngine {
    engine: OrganizedMemoryDecisionEngine,
    pending_legacy_migration: Option<PendingLegacyMemoryMigration>,
    pending_screen_observer_identity_migration: bool,
}

struct PendingLegacyMemoryMigration {
    source_event_id: String,
    report: LegacyMemoryMigrationReport,
}

const MAX_RUNTIME_EVENT_FRAME_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct LedgerHead {
    sequence: u64,
    event_id: String,
    /// 已提交 JSON frame 的原始字节摘要（不含换行 commit marker）。不能对反序列化
    /// 后的 typed envelope 再序列化取摘要：serde_json 的浮点文本并不保证字节级回环。
    frame_digest: [u8; 32],
    /// 最后一个已提交换行符之后的物理长度。仅比较最后一条 envelope 无法发现
    /// 旧 writer 重复追加同一 frame，或在本进程写入前后插入又被新 head 遮住。
    committed_len: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpenMissionRequest {
    pub objective: String,
    #[serde(default = "default_mission_priority")]
    pub priority: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_at_ms: Option<i64>,
}

fn default_mission_priority() -> u8 {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RegisterMissionAgentRequest {
    pub display_name: String,
    pub role: String,
    pub capabilities: Vec<CapabilityContract>,
    #[serde(default = "default_mission_priority")]
    pub priority: u8,
    pub interruptibility: Interruptibility,
    pub mission_id: String,
    pub run_id: String,
}

/// Execution adapter 提交给 Runtime 的一次 Front 交互。原文只在调用栈中短暂
/// 存在；账本只保存 SHA-256 与字符数，避免把完整对话复制成第二份持久真相源。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpenInteractionRunRequest {
    pub content: String,
    pub modality: InteractionModality,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_interaction_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_interrupt_id: Option<String>,
}

#[derive(Default)]
struct EventContext {
    source_actor_id: String,
    mission_id: Option<String>,
    run_id: Option<String>,
    interaction_scope_id: Option<String>,
    interaction_run_id: Option<String>,
    causation_id: Option<String>,
    correlation_id: Option<String>,
}

impl EventContext {
    fn kernel() -> Self {
        Self {
            source_actor_id: KERNEL_ACTOR_ID.to_string(),
            ..Self::default()
        }
    }
}

impl PinvouOsRuntime {
    pub fn boot(ledger_path: PathBuf) -> Result<Self> {
        if let Some(parent) = ledger_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create PinvouOS runtime dir {}", parent.display()))?;
            super::platform::harden_private_runtime_dir(parent)
                .with_context(|| format!("protect PinvouOS runtime dir {}", parent.display()))?;
        }
        let writer_lease = acquire_runtime_writer_lease(&ledger_path)?;
        let (events, committed_len, durable_head) = read_ledger_events(&ledger_path)?;
        validate_ledger_event_chain(&events)?;
        let snapshot = replay_ledger_events(&events);
        let replayed_memory = replay_memory_engine(&events)?;
        let memory_evidence_index = build_trusted_memory_evidence_index(&events, &snapshot);
        if durable_head.is_none() && committed_len != 0 {
            bail!("PinvouOS ledger contains committed bytes but no event envelope");
        }
        let next_sequence = snapshot.last_sequence.saturating_add(1).max(1);
        if let Some(migration) = replayed_memory.pending_legacy_migration.as_ref() {
            preflight_legacy_memory_migration(&replayed_memory.engine, migration)?;
        } else if replayed_memory.pending_screen_observer_identity_migration {
            preflight_screen_observer_memory_identity_migration(&replayed_memory.engine)?;
        }
        let pending_legacy_migration = replayed_memory.pending_legacy_migration;
        let pending_screen_observer_identity_migration =
            replayed_memory.pending_screen_observer_identity_migration;
        let runtime = Self {
            inner: Arc::new(RuntimeInner {
                ledger_path,
                _writer_lease: writer_lease,
                append_lock: Mutex::new(()),
                durable_head: RwLock::new(durable_head),
                write_failure: RwLock::new(None),
                snapshot: RwLock::new(snapshot),
                next_sequence: AtomicU64::new(next_sequence),
                event_sink: RwLock::new(None),
                governor_policy: ResourceGovernorPolicy::default(),
                memory_engine: RwLock::new(MemoryEngineState::Ready(replayed_memory.engine)),
                memory_evidence_index: RwLock::new(memory_evidence_index),
                control_adapters: RwLock::new(BTreeMap::new()),
                dispatched_directive_ids: Mutex::new(BTreeSet::new()),
            }),
        };
        // 先写一个当前 schema 的 RuntimeStarted，形成升级栅栏；旧 Runtime 会因
        // schema 过新而拒绝启动，不会在新事件之后继续写旧格式快照。
        runtime.bootstrap()?;
        if let Some(migration) = pending_legacy_migration {
            runtime.persist_legacy_memory_migration(migration)?;
        } else if pending_screen_observer_identity_migration {
            runtime.persist_screen_observer_memory_identity_migration()?;
        }
        runtime.reconcile_interrupted_process_interactions()?;
        Ok(runtime)
    }

    pub fn ledger_path(&self) -> &Path {
        &self.inner.ledger_path
    }

    pub fn set_event_sink(&self, sink: impl Fn(EventEnvelope) + Send + Sync + 'static) {
        *self.inner.event_sink.write() = Some(Arc::new(sink));
    }

    #[cfg(test)]
    pub(super) fn record_test_user_claim(
        &self,
        subject: &str,
        predicate: &str,
        value: serde_json::Value,
    ) -> Result<EventEnvelope> {
        let asserted_at_ms = now_ms();
        let claim = WorldClaim {
            claim_id: new_entity_id("test-user-claim"),
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            value,
            confidence: 1.0,
            asserted_by_actor_id: "actor:user".to_string(),
            evidence_event_ids: Vec::new(),
            asserted_at_ms,
            active: true,
            retracted_at_ms: None,
            retraction_reason: None,
        };
        self.append(
            EventContext {
                source_actor_id: "actor:user".to_string(),
                ..EventContext::default()
            },
            RuntimeEvent::ClaimAsserted { claim },
        )
    }

    #[cfg(test)]
    pub(super) fn retract_test_user_claim(&self, claim_id: &str) -> Result<EventEnvelope> {
        self.append(
            EventContext {
                source_actor_id: "actor:user".to_string(),
                ..EventContext::default()
            },
            RuntimeEvent::ClaimRetracted {
                claim_id: claim_id.to_string(),
                retracted_at_ms: now_ms(),
                reason: "test user correction".to_string(),
            },
        )
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.inner.snapshot.read().clone()
    }

    fn reconcile_interrupted_process_interactions(&self) -> Result<()> {
        let unfinished = self
            .inner
            .snapshot
            .read()
            .interaction_runs
            .values()
            .filter(|interaction| {
                matches!(
                    interaction.status,
                    InteractionRunStatus::Submitted | InteractionRunStatus::Running
                )
            })
            .map(|interaction| (interaction.interaction_run_id.clone(), interaction.status))
            .collect::<Vec<_>>();
        for (interaction_run_id, status) in unfinished {
            if status == InteractionRunStatus::Submitted {
                self.start_interaction_run(&interaction_run_id)?;
            }
            self.finish_interaction_run(
                &interaction_run_id,
                InteractionRunOutcome::Error {
                    error_code: "runtime_restarted".to_string(),
                },
            )?;
        }
        Ok(())
    }

    pub fn open_interaction_run(
        &self,
        request: OpenInteractionRunRequest,
    ) -> Result<InteractionRun> {
        let content = request.content.trim();
        if content.is_empty() {
            bail!("interaction content must not be empty");
        }
        let input_char_count = u32::try_from(content.chars().count())
            .map_err(|_| anyhow!("interaction content is too long"))?;
        if input_char_count > 16_384 {
            bail!("interaction content is too long");
        }
        if request.parent_interaction_run_id.is_some() != request.resume_interrupt_id.is_some() {
            bail!("interaction resume requires both parent run and interrupt id");
        }

        let submitted_at_ms = now_ms();
        let interaction_run_id = new_entity_id("interaction-run");
        let interaction_run = InteractionRun {
            interaction_run_id: interaction_run_id.clone(),
            interaction_scope_id: PINVOU_INTERACTION_SCOPE_ID.to_string(),
            parent_interaction_run_id: request.parent_interaction_run_id,
            resume_interrupt_id: request.resume_interrupt_id,
            input_digest: sha256_hex(content.as_bytes()),
            input_char_count,
            modality: request.modality,
            status: InteractionRunStatus::Submitted,
            submitted_at_ms,
            started_at_ms: None,
            finished_at_ms: None,
            outcome: None,
        };
        let append_guard = self.inner.append_lock.lock();
        if let (Some(parent_id), Some(interrupt_id)) = (
            interaction_run.parent_interaction_run_id.as_deref(),
            interaction_run.resume_interrupt_id.as_deref(),
        ) {
            let snapshot = self.inner.snapshot.read();
            let parent = snapshot
                .interaction_runs
                .get(parent_id)
                .ok_or_else(|| anyhow!("unknown parent interaction run {parent_id}"))?;
            let resumable = matches!(
                parent.outcome.as_ref(),
                Some(InteractionRunOutcome::Interrupt { interrupts })
                    if interrupts.iter().any(|interrupt| interrupt.interrupt_id == interrupt_id)
            );
            if !resumable {
                bail!("interrupt {interrupt_id} is not resumable from {parent_id}");
            }
            if snapshot.interaction_runs.values().any(|interaction| {
                interaction.parent_interaction_run_id.as_deref() == Some(parent_id)
                    && interaction.resume_interrupt_id.as_deref() == Some(interrupt_id)
            }) {
                bail!("interrupt {interrupt_id} was already resumed from {parent_id}");
            }
        }
        let publication = self.append_locked(
            interaction_event_context(&interaction_run_id),
            RuntimeEvent::InteractionRunOpened {
                interaction_run: interaction_run.clone(),
            },
        )?;
        drop(append_guard);
        deliver_persisted_event(Some(publication));
        Ok(interaction_run)
    }

    pub fn start_interaction_run(&self, interaction_run_id: &str) -> Result<EventEnvelope> {
        self.append_interaction_if_status(
            interaction_run_id,
            InteractionRunStatus::Submitted,
            RuntimeEvent::InteractionRunStarted {
                interaction_run_id: interaction_run_id.to_string(),
                started_at_ms: now_ms(),
            },
        )
    }

    pub fn record_interaction_tool_started(
        &self,
        interaction_run_id: &str,
        tool_call_id: &str,
        tool_name: &str,
    ) -> Result<EventEnvelope> {
        let tool_call_id = required_identifier(tool_call_id, "tool call id")?;
        let tool_name = required_text(tool_name, "tool name")?;
        self.append_interaction_if_status(
            interaction_run_id,
            InteractionRunStatus::Running,
            RuntimeEvent::InteractionToolStarted {
                interaction_run_id: interaction_run_id.to_string(),
                tool_call_id,
                tool_name,
                started_at_ms: now_ms(),
            },
        )
    }

    pub fn record_interaction_tool_finished(
        &self,
        interaction_run_id: &str,
        tool_call_id: &str,
        tool_name: &str,
        success: bool,
    ) -> Result<EventEnvelope> {
        let tool_call_id = required_identifier(tool_call_id, "tool call id")?;
        let tool_name = required_text(tool_name, "tool name")?;
        self.append_interaction_if_status(
            interaction_run_id,
            InteractionRunStatus::Running,
            RuntimeEvent::InteractionToolFinished {
                interaction_run_id: interaction_run_id.to_string(),
                tool_call_id,
                tool_name,
                success,
                finished_at_ms: now_ms(),
            },
        )
    }

    pub fn record_interaction_assistant_message(
        &self,
        interaction_run_id: &str,
        content: &str,
    ) -> Result<Option<EventEnvelope>> {
        if content.is_empty() {
            return Ok(None);
        }
        let message_char_count = u32::try_from(content.chars().count())
            .map_err(|_| anyhow!("assistant message is too long"))?;
        self.append_interaction_if_status(
            interaction_run_id,
            InteractionRunStatus::Running,
            RuntimeEvent::InteractionAssistantMessageCompleted {
                interaction_run_id: interaction_run_id.to_string(),
                message_digest: sha256_hex(content.as_bytes()),
                message_char_count,
                completed_at_ms: now_ms(),
            },
        )
        .map(Some)
    }

    pub fn finish_interaction_run(
        &self,
        interaction_run_id: &str,
        outcome: InteractionRunOutcome,
    ) -> Result<EventEnvelope> {
        validate_interaction_outcome(&outcome)?;
        self.append_interaction_if_status(
            interaction_run_id,
            InteractionRunStatus::Running,
            RuntimeEvent::InteractionRunFinished {
                interaction_run_id: interaction_run_id.to_string(),
                outcome,
                finished_at_ms: now_ms(),
            },
        )
    }

    /// Interaction 状态检查和落账必须共享 append_lock；否则两个并发终态或两个
    /// 并发 resume 都可能先观察到同一旧快照，再各自形成合法但互相矛盾的事件。
    fn append_interaction_if_status(
        &self,
        interaction_run_id: &str,
        expected: InteractionRunStatus,
        event: RuntimeEvent,
    ) -> Result<EventEnvelope> {
        let interaction_run_id = required_identifier(interaction_run_id, "interaction run id")?;
        let append_guard = self.inner.append_lock.lock();
        let interaction_run = self
            .inner
            .snapshot
            .read()
            .interaction_runs
            .get(&interaction_run_id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown interaction run {interaction_run_id}"))?;
        if interaction_run.status != expected {
            bail!(
                "interaction run {} is {:?}, expected {:?}",
                interaction_run_id,
                interaction_run.status,
                expected
            );
        }
        let publication =
            self.append_locked(interaction_event_context(&interaction_run_id), event)?;
        let envelope = publication.0.clone();
        drop(append_guard);
        deliver_persisted_event(Some(publication));
        Ok(envelope)
    }

    pub fn open_mission(&self, request: OpenMissionRequest) -> Result<MissionStart> {
        let objective = request.objective.trim();
        if objective.is_empty() {
            bail!("mission objective must not be empty");
        }
        if request
            .deadline_at_ms
            .is_some_and(|deadline| deadline <= now_ms())
        {
            bail!("mission deadline must be in the future");
        }

        let created_at_ms = now_ms();
        let mission_id = new_entity_id("mission");
        let run_id = new_entity_id("run");
        let mission = Mission {
            mission_id: mission_id.clone(),
            objective: objective.to_string(),
            priority: request.priority,
            status: MissionStatus::Active,
            created_at_ms,
            deadline_at_ms: request.deadline_at_ms,
        };
        let run = Run {
            run_id: run_id.clone(),
            mission_id: mission_id.clone(),
            attempt: 1,
            status: RunStatus::Running,
            started_at_ms: created_at_ms,
            ended_at_ms: None,
        };

        let opened = self.append(
            EventContext {
                source_actor_id: "actor:user".to_string(),
                mission_id: Some(mission_id.clone()),
                correlation_id: Some(mission_id.clone()),
                ..EventContext::default()
            },
            RuntimeEvent::MissionOpened {
                mission: mission.clone(),
            },
        )?;
        self.append(
            EventContext {
                source_actor_id: KERNEL_ACTOR_ID.to_string(),
                mission_id: Some(mission_id.clone()),
                run_id: Some(run_id.clone()),
                causation_id: Some(opened.event_id),
                correlation_id: Some(mission_id),
                ..EventContext::default()
            },
            RuntimeEvent::RunStarted { run: run.clone() },
        )?;

        Ok(MissionStart { mission, run })
    }

    pub fn register_mission_agent(
        &self,
        mut request: RegisterMissionAgentRequest,
    ) -> Result<AgentManifest> {
        canonicalize_and_validate_capability_contracts(&mut request.capabilities)?;
        let snapshot = self.inner.snapshot.read();
        let mission = snapshot
            .missions
            .get(&request.mission_id)
            .ok_or_else(|| anyhow!("unknown mission {}", request.mission_id))?;
        if mission.status != MissionStatus::Active {
            bail!("mission {} is not active", request.mission_id);
        }
        let run = snapshot
            .runs
            .get(&request.run_id)
            .ok_or_else(|| anyhow!("unknown run {}", request.run_id))?;
        if run.mission_id != request.mission_id || run.status != RunStatus::Running {
            bail!("run {} is not active for mission", request.run_id);
        }
        drop(snapshot);

        let agent = AgentManifest {
            agent_id: new_entity_id("agent"),
            display_name: required_text(&request.display_name, "agent display name")?,
            kind: AgentKind::Mission,
            role: required_text(&request.role, "agent role")?,
            capabilities: request.capabilities,
            priority: request.priority,
            interruptibility: request.interruptibility,
            observed_state: AgentState::Running,
            desired_state: AgentState::Running,
            mission_id: Some(request.mission_id.clone()),
            run_id: Some(request.run_id.clone()),
            created_at_ms: now_ms(),
        };
        self.append(
            EventContext {
                source_actor_id: KERNEL_ACTOR_ID.to_string(),
                mission_id: Some(request.mission_id.clone()),
                run_id: Some(request.run_id),
                correlation_id: Some(request.mission_id),
                ..EventContext::default()
            },
            RuntimeEvent::AgentRegistered {
                agent: agent.clone(),
            },
        )?;
        Ok(agent)
    }

    pub fn explain_capability(&self, capability_id: &str) -> CapabilityAvailability {
        let capability_id = capability_id.trim();
        let fallback = || CapabilityAvailability {
            capability_id: capability_id.to_string(),
            state: CapabilityAvailabilityState::Unsupported,
            candidate_agent_ids: Vec::new(),
            reason_codes: vec!["invalid_or_unregistered_capability".to_string()],
        };
        if capability_id.is_empty() {
            return fallback();
        }
        if matches!(
            capability_id,
            super::memory_agent::MEMORY_REMEMBER_CAPABILITY_ID
                | super::memory_agent::MEMORY_CONTEXT_CAPABILITY_ID
                | super::memory_agent::MEMORY_RETRACT_CAPABILITY_ID
        ) && matches!(
            &*self.inner.memory_engine.read(),
            MemoryEngineState::Poisoned { .. }
        ) {
            return CapabilityAvailability {
                capability_id: capability_id.to_string(),
                state: CapabilityAvailabilityState::TemporarilyUnavailable,
                candidate_agent_ids: vec![MEMORY_AGENT_ID.to_string()],
                reason_codes: vec!["memory_persistence_isolated".to_string()],
            };
        }
        let snapshot = self.snapshot();
        let Ok(report) = CapabilityAgent::report(
            &snapshot,
            CapabilityReportRequest {
                requested_capability_ids: vec![capability_id.to_string()],
                include_registered: false,
            },
            now_ms(),
        ) else {
            return fallback();
        };
        report
            .can_do
            .into_iter()
            .chain(report.temporarily_cannot)
            .chain(report.unsupported)
            .next()
            .map(|assessment| CapabilityAvailability {
                capability_id: assessment.capability_id,
                state: assessment.state,
                candidate_agent_ids: assessment.candidate_agent_ids,
                reason_codes: assessment.reason_codes,
            })
            .unwrap_or_else(fallback)
    }

    pub fn capability_report(
        &self,
        request: CapabilityReportRequest,
    ) -> Result<super::capability_agent::CapabilityReport> {
        CapabilityAgent::report(&self.snapshot(), request, now_ms())
            .map_err(|error| anyhow!(error.to_string()))
    }

    /// 内核 Executor Registry 为实际 Run 注册控制句柄。注册后会立即续接账本里该
    /// Agent 尚未完成的 Directive，因此 Runtime 重启不会把 Pending 控制静默丢掉。
    pub fn register_agent_control_adapter(
        &self,
        agent_id: &str,
        adapter: AgentControlAdapter,
    ) -> Result<usize> {
        let agent_id = required_text(agent_id, "control adapter agent id")?;
        let agent = self
            .inner
            .snapshot
            .read()
            .agents
            .get(&agent_id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown control adapter agent {agent_id}"))?;
        if agent.kind != AgentKind::Mission {
            bail!("control adapters can only own mission agents");
        }
        self.inner
            .control_adapters
            .write()
            .insert(agent_id.clone(), adapter);

        // Adapter 必须先可见，再从最新投影补偿 Pending。与新 Directive 并发时，
        // 要么新派发看到 Adapter，要么这里看到 Pending；两边都看到则由 dispatch
        // claim 合并成一次，不能留下“双方都错过”的窗口。
        let mut pending = self
            .inner
            .snapshot
            .read()
            .directives
            .values()
            .filter(|directive| {
                directive.target_agent_id == agent_id
                    && directive.status == DirectiveStatus::Pending
            })
            .cloned()
            .collect::<Vec<_>>();
        pending.sort_by_key(|directive| directive.issued_at_ms);
        let count = pending.len();
        for directive in pending {
            self.dispatch_control_directive(&directive)?;
        }
        Ok(count)
    }

    pub fn unregister_agent_control_adapter(&self, agent_id: &str) -> bool {
        self.inner
            .control_adapters
            .write()
            .remove(agent_id)
            .is_some()
    }

    /// 新 Memory Agent 的原生写入口。当前只接受与结构化 ClaimAsserted 的
    /// subject/predicate/value 精确一致的 ContextualFact；证据极性、权威、时间、
    /// 身份空间与环境范围均由 Runtime 派生，不能由模型自报。
    pub fn organize_memory(&self, candidate: MemoryCandidate) -> Result<MemoryOrganizationReceipt> {
        let evidence_event_ids = candidate
            .evidence
            .iter()
            .map(|evidence| evidence.event_id.clone())
            .collect::<Vec<_>>();
        self.commit_attested_memory_operation(
            &evidence_event_ids,
            move |trusted, _evidence_index, engine| {
                let candidate = attest_memory_candidate(candidate, trusted)
                    .map_err(|error| anyhow!(error.to_string()))?;
                let context = memory_event_context(&candidate.candidate_id, &candidate.evidence);
                let outcome = engine
                    .organize_with_evidence_actor_alias(
                        candidate,
                        SCREEN_OBSERVER_AGENT_ID,
                        LEGACY_SURFACE_AGENT_ID,
                    )
                    .map_err(|error| anyhow!(error.to_string()))?;
                Ok((context, outcome))
            },
        )
    }

    pub fn retract_organized_memory(
        &self,
        request: RetractOrganizedMemoryRequest,
    ) -> Result<super::memory_agent::MemoryRetractionReceipt> {
        let evidence_event_ids = request
            .evidence
            .iter()
            .map(|evidence| evidence.event_id.clone())
            .collect::<Vec<_>>();
        self.commit_attested_memory_operation(
            &evidence_event_ids,
            move |trusted, evidence_index, engine| {
                let target = engine
                    .organizer()
                    .state()
                    .records
                    .get(&request.memory_id)
                    .ok_or_else(|| anyhow!("unknown memory {}", request.memory_id))?;
                let request = attest_memory_retraction(request, trusted, evidence_index, target)
                    .map_err(|error| anyhow!(error.to_string()))?;
                let context = memory_event_context(&request.operation_id, &request.evidence);
                let outcome = engine
                    .retract(request)
                    .map_err(|error| anyhow!(error.to_string()))?;
                Ok((context, outcome))
            },
        )
    }

    pub fn resolve_memory_dispute(
        &self,
        request: ResolveMemoryDisputeRequest,
    ) -> Result<MemoryDisputeResolutionReceipt> {
        let evidence_event_ids = request
            .evidence
            .iter()
            .map(|evidence| evidence.event_id.clone())
            .collect::<Vec<_>>();
        self.commit_attested_memory_operation(
            &evidence_event_ids,
            move |trusted, _evidence_index, engine| {
                let request = attest_memory_resolution(request, trusted)
                    .map_err(|error| anyhow!(error.to_string()))?;
                let context = memory_event_context(&request.operation_id, &request.evidence);
                let outcome = engine
                    .resolve_dispute_with_evidence_actor_alias(
                        request,
                        SCREEN_OBSERVER_AGENT_ID,
                        LEGACY_SURFACE_AGENT_ID,
                    )
                    .map_err(|error| anyhow!(error.to_string()))?;
                Ok((context, outcome))
            },
        )
    }

    pub fn maintain_organized_memory(
        &self,
        _requested_at_ms: i64,
    ) -> Result<MemoryMaintenanceReport> {
        // 维护游标会影响所有记忆，绝不能由 Agent/插件把它推进到任意未来时间。
        let maintained_at_ms = now_ms();
        self.commit_memory_operation(
            EventContext {
                source_actor_id: MEMORY_AGENT_ID.to_string(),
                correlation_id: Some(format!("memory-maintenance:{maintained_at_ms}")),
                ..EventContext::default()
            },
            move |engine| {
                engine
                    .maintain(maintained_at_ms)
                    .map_err(|error| anyhow!(error.to_string()))
            },
        )
    }

    pub fn project_organized_memory(
        &self,
        query: OrganizedMemoryQuery,
    ) -> Result<OrganizedMemoryProjection> {
        let state = self.inner.memory_engine.read();
        let MemoryEngineState::Ready(engine) = &*state else {
            let MemoryEngineState::Poisoned { reason } = &*state else {
                unreachable!()
            };
            bail!("Memory Agent is isolated after a persistence failure: {reason}");
        };
        engine
            .organizer()
            .project(query)
            .map_err(|error| anyhow!(error.to_string()))
    }

    /// 迁移期兼容入口：不再运行旧 MemoryAgent，也不会写完整 projection。
    pub fn remember_memory(&self, request: RememberMemoryRequest) -> Result<MemoryMutationReceipt> {
        let evidence_event_ids = request.evidence_event_ids.clone();
        let receipt = self.commit_attested_memory_operation(
            &evidence_event_ids,
            move |trusted, _evidence_index, engine| {
                let candidate = legacy_remember_candidate(&request, trusted)
                    .map_err(|error| anyhow!(error.to_string()))?;
                let context = memory_event_context(&candidate.candidate_id, &candidate.evidence);
                let outcome = engine
                    .organize_with_evidence_actor_alias(
                        candidate,
                        SCREEN_OBSERVER_AGENT_ID,
                        LEGACY_SURFACE_AGENT_ID,
                    )
                    .map_err(|error| anyhow!(error.to_string()))?;
                Ok((context, outcome))
            },
        )?;
        Ok(legacy_organization_receipt(&receipt, evidence_event_ids))
    }

    pub fn retract_memory(&self, request: RetractMemoryRequest) -> Result<MemoryMutationReceipt> {
        let evidence_event_ids = request.evidence_event_ids.clone();
        let outcome = self.commit_attested_memory_operation(
            &evidence_event_ids,
            move |trusted, evidence_index, engine| {
                let request = legacy_retraction_request(&request, engine, trusted, evidence_index)
                    .map_err(|error| anyhow!(error.to_string()))?;
                let context = memory_event_context(&request.operation_id, &request.evidence);
                let outcome = engine
                    .retract(request)
                    .map_err(|error| anyhow!(error.to_string()))?;
                Ok((context, outcome))
            },
        )?;
        Ok(legacy_retraction_receipt(
            outcome.memory_id,
            outcome.revision,
            evidence_event_ids,
        ))
    }

    pub fn compile_memory_context(
        &self,
        request: CompileMemoryContextRequest,
    ) -> Result<MemoryContextProjection> {
        let state = self.inner.memory_engine.read();
        let engine = ready_memory_engine(&state)?;
        legacy_context_projection(engine, request, now_ms())
            .map_err(|error| anyhow!(error.to_string()))
    }

    pub fn observe_resources(&self, observation: ResourceObservation) -> Result<ResourceDecision> {
        validate_resource_observation(&observation)?;
        let before = self.snapshot();
        let previous_pressure = before.resources.pressure;
        let classified_pressure = classify_pressure(&observation, self.inner.governor_policy);
        // 降压必须证明导致当前压力的传感器已经恢复。缺温度/内存、陈旧或倒序的
        // 样本可以作为遥测留下，但不能把 Hot/Critical 错判成 Normal 并恢复任务。
        let pressure = if classified_pressure < previous_pressure
            && !resource_relief_is_authoritative(&before, &observation)
        {
            previous_pressure
        } else {
            classified_pressure
        };
        let observed = self.append(
            EventContext {
                source_actor_id: RESOURCE_AGENT_ID.to_string(),
                correlation_id: Some("resource-pressure".to_string()),
                ..EventContext::default()
            },
            RuntimeEvent::ResourceObserved {
                observation: observation.clone(),
                pressure,
            },
        )?;

        let pressure_changed = pressure != previous_pressure;
        let mut pressure_claim_id = before.resources.active_pressure_claim_id.clone();
        let mut directive_cause = observed.event_id.clone();
        if pressure_changed {
            if let Some(claim_id) = before.resources.active_pressure_claim_id {
                self.append(
                    EventContext {
                        source_actor_id: RESOURCE_AGENT_ID.to_string(),
                        causation_id: Some(observed.event_id.clone()),
                        correlation_id: Some("resource-pressure".to_string()),
                        ..EventContext::default()
                    },
                    RuntimeEvent::ClaimRetracted {
                        claim_id,
                        retracted_at_ms: now_ms(),
                        reason: format!("resource pressure changed to {pressure:?}"),
                    },
                )?;
                pressure_claim_id = None;
            }
        }
        if pressure != ResourcePressure::Normal && (pressure_changed || pressure_claim_id.is_none())
        {
            let claim_id = new_entity_id("claim");
            let claim = WorldClaim {
                claim_id: claim_id.clone(),
                subject: "device.resources".to_string(),
                predicate: "pressure_level".to_string(),
                value: json!({
                    "level": pressure,
                    "temperatureC": observation.temperature_c,
                    "memoryUsedPct": observation.memory_used_pct,
                    "cpuUsagePct": observation.cpu_usage_pct,
                }),
                confidence: 1.0,
                asserted_by_actor_id: RESOURCE_AGENT_ID.to_string(),
                evidence_event_ids: vec![observed.event_id.clone()],
                asserted_at_ms: now_ms(),
                active: true,
                retracted_at_ms: None,
                retraction_reason: None,
            };
            let asserted = self.append(
                EventContext {
                    source_actor_id: RESOURCE_AGENT_ID.to_string(),
                    causation_id: Some(observed.event_id.clone()),
                    correlation_id: Some("resource-pressure".to_string()),
                    ..EventContext::default()
                },
                RuntimeEvent::ClaimAsserted { claim },
            )?;
            directive_cause = asserted.event_id;
            pressure_claim_id = Some(claim_id);
        }

        let mut directives = Vec::new();
        // 每次权威观测都做 reconciliation：这既覆盖 Hot 期间新注册的 Agent，也能在
        // Hot→Warm→Normal 或进程重启后补发尚未完成的恢复控制，而不会依赖单次边沿。
        for agent in before.agents.values() {
            let Some((action, hard)) = directive_for_agent(
                agent,
                previous_pressure,
                pressure,
                self.inner.governor_policy,
            ) else {
                continue;
            };
            let directive = ControlDirective {
                directive_id: new_entity_id("directive"),
                target_agent_id: agent.agent_id.clone(),
                action,
                reason: format!(
                    "resource pressure reconciliation {previous_pressure:?} -> {pressure:?}"
                ),
                hard,
                issued_at_ms: now_ms(),
                status: DirectiveStatus::Pending,
                acknowledged_at_ms: None,
                acknowledgement_detail: None,
            };
            self.append(
                EventContext {
                    source_actor_id: GOVERNOR_ACTOR_ID.to_string(),
                    mission_id: agent.mission_id.clone(),
                    run_id: agent.run_id.clone(),
                    causation_id: Some(directive_cause.clone()),
                    correlation_id: Some("resource-pressure".to_string()),
                    ..EventContext::default()
                },
                RuntimeEvent::DirectiveIssued {
                    directive: directive.clone(),
                },
            )?;
            directives.push(self.dispatch_control_directive(&directive)?);
        }

        Ok(ResourceDecision {
            pressure,
            observation_event_id: observed.event_id,
            pressure_claim_id,
            directives,
        })
    }

    pub fn observe_connectivity(
        &self,
        observation: ConnectivityObservation,
    ) -> Result<EventEnvelope> {
        validate_observation_timestamp(observation.checked_at_ms, "connectivity")?;
        validate_reason_code(observation.reason_code.as_deref())?;
        self.append(
            EventContext {
                source_actor_id: CONNECTIVITY_AGENT_ID.to_string(),
                correlation_id: Some("connectivity-health".to_string()),
                ..EventContext::default()
            },
            RuntimeEvent::ConnectivityObserved { observation },
        )
    }

    pub fn observe_inference_health(
        &self,
        observation: InferenceHealthObservation,
    ) -> Result<EventEnvelope> {
        validate_observation_timestamp(observation.checked_at_ms, "inference health")?;
        validate_reason_code(observation.reason_code.as_deref())?;
        if observation
            .model
            .as_deref()
            .is_some_and(|model| model.trim().is_empty() || model.chars().count() > 256)
        {
            bail!("inference model name is invalid");
        }
        self.append(
            EventContext {
                source_actor_id: INFERENCE_AGENT_ID.to_string(),
                correlation_id: Some("inference-health".to_string()),
                ..EventContext::default()
            },
            RuntimeEvent::InferenceHealthObserved { observation },
        )
    }

    pub fn record_inference_completion(
        &self,
        observation: InferenceCompletionObservation,
    ) -> Result<EventEnvelope> {
        validate_observation_timestamp(observation.completed_at_ms, "inference completion")?;
        required_text(&observation.model, "inference completion model")?;
        self.append(
            EventContext {
                source_actor_id: INFERENCE_AGENT_ID.to_string(),
                correlation_id: Some("inference-completion".to_string()),
                ..EventContext::default()
            },
            RuntimeEvent::InferenceCompleted { observation },
        )
    }

    pub fn acknowledge_directive(
        &self,
        directive_id: &str,
        applied: bool,
        detail: String,
    ) -> Result<ControlDirective> {
        self.commit_directive_acknowledgement(
            directive_id,
            applied,
            detail,
            DirectiveAcknowledgementOwner::External,
        )
    }

    fn acknowledge_dispatched_directive(
        &self,
        directive_id: &str,
        applied: bool,
        detail: String,
    ) -> Result<ControlDirective> {
        self.commit_directive_acknowledgement(
            directive_id,
            applied,
            detail,
            DirectiveAcknowledgementOwner::RegisteredAdapter,
        )
    }

    fn commit_directive_acknowledgement(
        &self,
        directive_id: &str,
        applied: bool,
        detail: String,
        owner: DirectiveAcknowledgementOwner,
    ) -> Result<ControlDirective> {
        // Pending 检查与 ACK 落账共享 append_lock。否则两个并发确认都可能先读取
        // Pending，再各自追加一个合法但互相冲突的 DirectiveAcknowledged。
        let append_guard = self.inner.append_lock.lock();
        let (directive, agent) = {
            let snapshot = self.inner.snapshot.read();
            let directive = snapshot
                .directives
                .get(directive_id)
                .cloned()
                .ok_or_else(|| anyhow!("unknown directive {directive_id}"))?;
            if directive.status != DirectiveStatus::Pending {
                bail!("directive {directive_id} was already acknowledged");
            }
            let adapter_owns_ack = self
                .inner
                .dispatched_directive_ids
                .lock()
                .contains(directive_id);
            match (owner, adapter_owns_ack) {
                (DirectiveAcknowledgementOwner::External, true) => {
                    bail!(
                        "directive {directive_id} acknowledgement is owned by its control adapter"
                    )
                }
                (DirectiveAcknowledgementOwner::RegisteredAdapter, false) => {
                    bail!("directive {directive_id} has no adapter dispatch claim")
                }
                _ => {}
            }
            let agent = snapshot
                .agents
                .get(&directive.target_agent_id)
                .cloned()
                .ok_or_else(|| anyhow!("directive target no longer exists"))?;
            (directive, agent)
        };

        let status = if applied {
            DirectiveStatus::Applied
        } else {
            DirectiveStatus::Rejected
        };
        let resulting_state = if applied {
            match directive.action {
                DirectiveAction::Pause => AgentState::Paused,
                DirectiveAction::Resume => AgentState::Running,
                DirectiveAction::Stop => AgentState::Stopped,
            }
        } else {
            agent.observed_state
        };
        let publication = self.append_locked(
            EventContext {
                source_actor_id: format!("adapter:{}", directive.target_agent_id),
                mission_id: agent.mission_id,
                run_id: agent.run_id,
                causation_id: Some(directive.directive_id.clone()),
                correlation_id: Some("resource-pressure".to_string()),
                ..EventContext::default()
            },
            RuntimeEvent::DirectiveAcknowledged {
                directive_id: directive.directive_id.clone(),
                target_agent_id: directive.target_agent_id.clone(),
                status,
                resulting_state,
                acknowledged_at_ms: now_ms(),
                detail,
            },
        )?;
        let acknowledged = self
            .inner
            .snapshot
            .read()
            .directives
            .get(directive_id)
            .cloned()
            .ok_or_else(|| anyhow!("directive projection disappeared"))?;
        drop(append_guard);
        deliver_persisted_event(Some(publication));
        Ok(acknowledged)
    }

    fn dispatch_control_directive(&self, directive: &ControlDirective) -> Result<ControlDirective> {
        let (directive, adapter) = {
            // 与 ACK 共用 append_lock：一个已确认的 Directive 不能在状态检查后又被
            // 外部执行；dispatch claim 则合并新派发与注册时补偿派发的并发竞争。
            let _append_guard = self.inner.append_lock.lock();
            let current = self
                .inner
                .snapshot
                .read()
                .directives
                .get(&directive.directive_id)
                .cloned()
                .ok_or_else(|| anyhow!("unknown directive {}", directive.directive_id))?;
            if current.status != DirectiveStatus::Pending {
                return Ok(current);
            }
            let adapter = self
                .inner
                .control_adapters
                .read()
                .get(&current.target_agent_id)
                .cloned();
            let Some(adapter) = adapter else {
                return Ok(current);
            };
            if !self
                .inner
                .dispatched_directive_ids
                .lock()
                .insert(current.directive_id.clone())
            {
                return Ok(current);
            }
            (current, adapter)
        };
        match adapter(directive.clone()) {
            Ok(detail) => {
                self.acknowledge_dispatched_directive(&directive.directive_id, true, detail)
            }
            Err(detail) => {
                self.acknowledge_dispatched_directive(&directive.directive_id, false, detail)
            }
        }
    }

    pub fn list_events(
        &self,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Vec<EventEnvelope>> {
        let limit = limit.clamp(1, 1_000);
        let after_sequence = after_sequence.unwrap_or(0);
        // Renderer 只能看到本 Runtime 已完成 sync 和后验校验的提交。否则轮询可能
        // 抢在 append 的 suffix 验证前读到一条最终被 fail-stop 的可见事件。
        let append_guard = self.inner.append_lock.lock();
        ensure_runtime_writable(&self.inner)?;
        let (events, _committed_len, actual_head) = read_ledger_events(&self.inner.ledger_path)?;
        validate_ledger_event_chain(&events)?;
        if actual_head != *self.inner.durable_head.read() {
            bail!("PinvouOS ledger changed outside this Runtime; renderer read refused");
        }
        drop(append_guard);
        Ok(events
            .into_iter()
            .filter(|event| event_is_renderer_visible(&event.event))
            .filter(|event| event.sequence > after_sequence)
            .take(limit)
            .collect())
    }

    fn commit_memory_operation<T>(
        &self,
        context: EventContext,
        operation: impl FnOnce(
            &mut OrganizedMemoryDecisionEngine,
        )
            -> Result<super::memory_agent::OrganizedMemoryDecisionOutcome<T>>,
    ) -> Result<T> {
        let mut state = self.inner.memory_engine.write();
        let append_guard = self.inner.append_lock.lock();
        ensure_runtime_writable(&self.inner)?;
        let outcome = {
            let engine = ready_memory_engine_mut(&mut state)?;
            operation(engine)?
        };
        let super::memory_agent::OrganizedMemoryDecisionOutcome { receipt, decision } = outcome;
        let publication = if let Some(decision) = decision {
            match self.append_locked(
                context,
                RuntimeEvent::OrganizedMemoryDecisionRecorded { decision },
            ) {
                Ok(publication) => Some(publication),
                Err(error) => {
                    let reason = error.to_string();
                    *state = MemoryEngineState::Poisoned {
                        reason: reason.clone(),
                    };
                    return Err(error).context(format!(
                        "persist organized memory decision; Memory Agent isolated: {reason}"
                    ));
                }
            }
        } else {
            None
        };
        drop(state);
        drop(append_guard);
        deliver_persisted_event(publication);
        Ok(receipt)
    }

    /// 在统一账本追加锁内重新验证证据并生成 decision。这样 ClaimRetracted 与
    /// Memory decision 之间只有一种可重放的先后顺序，不会把已经失效的声明用于
    /// 新记忆。锁序固定为 memory -> append -> evidence/snapshot；长查询只会推迟
    /// Memory 写者取得 append 锁，不会阻塞 Resource/Governor 等关键账本写入。
    fn commit_attested_memory_operation<T>(
        &self,
        evidence_event_ids: &[String],
        operation: impl FnOnce(
            &BTreeMap<String, TrustedMemoryEvidence>,
            &BTreeMap<String, TrustedMemoryEvidence>,
            &mut OrganizedMemoryDecisionEngine,
        ) -> Result<(
            EventContext,
            super::memory_agent::OrganizedMemoryDecisionOutcome<T>,
        )>,
    ) -> Result<T> {
        validate_memory_evidence_request(evidence_event_ids)?;
        let mut state = self.inner.memory_engine.write();
        let append_guard = self.inner.append_lock.lock();
        ensure_runtime_writable(&self.inner)?;
        let (context, outcome) = {
            let evidence_index = self.inner.memory_evidence_index.read();
            let snapshot = self.inner.snapshot.read();
            let trusted =
                collect_current_memory_evidence(evidence_event_ids, &evidence_index, &snapshot)?;
            let engine = ready_memory_engine_mut(&mut state)?;
            operation(&trusted, &evidence_index, engine)?
        };
        let super::memory_agent::OrganizedMemoryDecisionOutcome { receipt, decision } = outcome;
        let publication = if let Some(decision) = decision {
            match self.append_locked(
                context,
                RuntimeEvent::OrganizedMemoryDecisionRecorded { decision },
            ) {
                Ok(publication) => Some(publication),
                Err(error) => {
                    let reason = error.to_string();
                    *state = MemoryEngineState::Poisoned {
                        reason: reason.clone(),
                    };
                    return Err(error).context(format!(
                        "persist organized memory decision; Memory Agent isolated: {reason}"
                    ));
                }
            }
        } else {
            None
        };
        drop(state);
        drop(append_guard);
        deliver_persisted_event(publication);
        Ok(receipt)
    }

    fn persist_legacy_memory_migration(
        &self,
        migration: PendingLegacyMemoryMigration,
    ) -> Result<()> {
        let mut state = self.inner.memory_engine.write();
        let append_guard = self.inner.append_lock.lock();
        ensure_runtime_writable(&self.inner)?;
        let checkpoint = ready_memory_engine_mut(&mut state)?.checkpoint();
        let result = self.append_locked(
            EventContext {
                source_actor_id: MEMORY_AGENT_ID.to_string(),
                causation_id: Some(migration.source_event_id.clone()),
                correlation_id: Some("memory-legacy-migration".to_string()),
                ..EventContext::default()
            },
            RuntimeEvent::OrganizedMemoryCheckpointRecorded {
                checkpoint,
                legacy_source_event_id: Some(migration.source_event_id),
                legacy_migration: Some(migration.report),
            },
        );
        let publication = match result {
            Ok(publication) => Some(publication),
            Err(error) => {
                let reason = error.to_string();
                *state = MemoryEngineState::Poisoned {
                    reason: reason.clone(),
                };
                return Err(error).context(format!(
                    "persist legacy memory migration checkpoint; Memory Agent isolated: {reason}"
                ));
            }
        };
        drop(state);
        drop(append_guard);
        deliver_persisted_event(publication);
        Ok(())
    }

    fn persist_screen_observer_memory_identity_migration(&self) -> Result<()> {
        let mut state = self.inner.memory_engine.write();
        let append_guard = self.inner.append_lock.lock();
        ensure_runtime_writable(&self.inner)?;
        let checkpoint = ready_memory_engine_mut(&mut state)?.checkpoint();
        let result = self.append_locked(
            EventContext {
                source_actor_id: MEMORY_AGENT_ID.to_string(),
                correlation_id: Some("memory-screen-observer-identity-migration".to_string()),
                ..EventContext::default()
            },
            RuntimeEvent::OrganizedMemoryCheckpointRecorded {
                checkpoint,
                legacy_source_event_id: None,
                legacy_migration: None,
            },
        );
        let publication = match result {
            Ok(publication) => Some(publication),
            Err(error) => {
                let reason = error.to_string();
                *state = MemoryEngineState::Poisoned {
                    reason: reason.clone(),
                };
                return Err(error).context(format!(
                    "persist Screen Observer memory identity checkpoint; Memory Agent isolated: {reason}"
                ));
            }
        };
        drop(state);
        drop(append_guard);
        deliver_persisted_event(publication);
        Ok(())
    }

    fn bootstrap(&self) -> Result<()> {
        self.append(
            EventContext::kernel(),
            RuntimeEvent::RuntimeStarted {
                process_id: std::process::id(),
            },
        )?;
        if self.inner.snapshot.read().identity.is_none() {
            self.append(
                EventContext::kernel(),
                RuntimeEvent::IdentityDeclared {
                    identity: PinvouIdentity::default(),
                },
            )?;
        }
        for mut agent in builtin_system_agents(now_ms()) {
            let existing = self
                .inner
                .snapshot
                .read()
                .agents
                .get(&agent.agent_id)
                .cloned();
            let should_register = match existing {
                None => true,
                Some(existing) if builtin_manifest_needs_refresh(&existing, &agent) => {
                    // 升级内建 Agent 的契约时保留其运行态与首次创建时间。账本追加一条
                    // 同 ID 的 AgentRegistered 即可重放出最新 manifest，不另建影子真相。
                    // 若当前实现已经有真实处理器（expected=Running），允许旧账本中的
                    // Starting 一次性晋级；其余动态运行态仍由账本而非 bootstrap 决定。
                    if !builtin_became_operational(&existing, &agent) {
                        agent.observed_state = existing.observed_state;
                    }
                    agent.desired_state = existing.desired_state;
                    agent.created_at_ms = existing.created_at_ms;
                    true
                }
                Some(_) => false,
            };
            if should_register {
                self.append(
                    EventContext::kernel(),
                    RuntimeEvent::AgentRegistered { agent },
                )?;
            }
        }
        Ok(())
    }

    fn append(&self, context: EventContext, event: RuntimeEvent) -> Result<EventEnvelope> {
        let append_guard = self.inner.append_lock.lock();
        let publication = self.append_locked(context, event)?;
        let envelope = publication.0.clone();
        drop(append_guard);
        deliver_persisted_event(Some(publication));
        Ok(envelope)
    }

    /// 调用方必须持有 `append_lock`。返回的 renderer 回调必须等所有 Runtime 锁
    /// 释放后再执行，避免外部回调重入造成锁反转。
    fn append_locked(
        &self,
        context: EventContext,
        event: RuntimeEvent,
    ) -> Result<(EventEnvelope, Option<Arc<RuntimeEventSink>>)> {
        ensure_runtime_writable(&self.inner)?;
        let sequence = self.inner.next_sequence.load(Ordering::Relaxed);
        let occurred_at_ms = now_ms();
        let envelope = EventEnvelope {
            schema_version: SCHEMA_VERSION,
            sequence,
            event_id: format!("event-{sequence:016x}"),
            occurred_at_ms,
            source_actor_id: context.source_actor_id,
            mission_id: context.mission_id,
            run_id: context.run_id,
            interaction_scope_id: context.interaction_scope_id,
            interaction_run_id: context.interaction_run_id,
            causation_id: context.causation_id,
            correlation_id: context.correlation_id,
            event,
        };
        let expected_head = self.inner.durable_head.read().clone();
        let new_head = match append_envelope(&self.inner.ledger_path, &envelope, &expected_head) {
            Ok(head) => head,
            Err(error) => {
                *self.inner.write_failure.write() = Some(error.to_string());
                return Err(error).context("append PinvouOS event; Runtime write side isolated");
            }
        };
        let evidence = {
            let mut snapshot = self.inner.snapshot.write();
            apply_event(&mut snapshot, &envelope);
            trusted_memory_evidence_from_envelope(&envelope, &snapshot)
        };
        if let Some(evidence) = evidence {
            self.inner
                .memory_evidence_index
                .write()
                .insert(evidence.event_id.clone(), evidence);
        }
        *self.inner.durable_head.write() = Some(new_head);
        self.inner
            .next_sequence
            .store(sequence.saturating_add(1), Ordering::Relaxed);
        let sink = event_is_renderer_visible(&envelope.event)
            .then(|| self.inner.event_sink.read().clone())
            .flatten();
        Ok((envelope, sink))
    }
}

fn ensure_runtime_writable(inner: &RuntimeInner) -> Result<()> {
    if let Some(reason) = inner.write_failure.read().as_deref() {
        bail!("PinvouOS Runtime write side is isolated until reboot: {reason}");
    }
    Ok(())
}

fn deliver_persisted_event(publication: Option<(EventEnvelope, Option<Arc<RuntimeEventSink>>)>) {
    if let Some((envelope, Some(sink))) = publication {
        sink(envelope);
    }
}

fn acquire_runtime_writer_lease(ledger_path: &Path) -> Result<fs::File> {
    let mut lock_name = ledger_path.as_os_str().to_os_string();
    lock_name.push(".writer.lock");
    let lock_path = PathBuf::from(lock_name);
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    super::platform::configure_private_ledger(&mut options);
    let file = options
        .open(&lock_path)
        .with_context(|| format!("open PinvouOS writer lease {}", lock_path.display()))?;
    super::platform::harden_private_ledger(&file)
        .with_context(|| format!("protect PinvouOS writer lease {}", lock_path.display()))?;
    fs2::FileExt::try_lock_exclusive(&file).with_context(|| {
        format!(
            "another PinvouOS Runtime already owns the writer lease {}",
            lock_path.display()
        )
    })?;
    Ok(file)
}

fn append_envelope(
    path: &Path,
    envelope: &EventEnvelope,
    expected_head: &Option<LedgerHead>,
) -> Result<LedgerHead> {
    let payload = serialize_envelope_frame(envelope)?;
    let mut options = OpenOptions::new();
    options.create(true).append(true).read(true).write(true);
    super::platform::configure_private_ledger(&mut options);
    let mut file = options
        .open(path)
        .with_context(|| format!("open PinvouOS ledger {}", path.display()))?;
    super::platform::harden_private_ledger(&file)
        .with_context(|| format!("protect PinvouOS ledger {}", path.display()))?;
    file.lock_exclusive()
        .with_context(|| format!("lock PinvouOS ledger {}", path.display()))?;
    // 进程在一行中途被强杀时，只能丢弃最后一个未完成 frame。若把残行简单补
    // 换行，下一次重启便无法区分“曾经 torn”与“完整但被篡改”的事件。
    truncate_torn_ledger_tail(&mut file)?;
    let base_len = file
        .metadata()
        .context("read PinvouOS ledger length after torn-frame repair")?
        .len();
    let actual_head = read_last_complete_ledger_head(&mut file)?;
    if &actual_head != expected_head {
        bail!(
            "PinvouOS ledger head changed outside this Runtime; expected {:?}, found {:?}",
            expected_head,
            actual_head
        );
    }
    let expected_final_len = base_len
        .checked_add(payload.len() as u64)
        .ok_or_else(|| anyhow!("PinvouOS ledger length overflow"))?;
    file.write_all(&payload)
        .with_context(|| format!("append PinvouOS ledger {}", path.display()))?;
    file.flush().context("flush PinvouOS ledger")?;
    file.sync_data().context("sync PinvouOS ledger")?;
    let final_len = file
        .metadata()
        .context("read PinvouOS ledger length after append")?
        .len();
    if final_len != expected_final_len {
        bail!(
            "PinvouOS ledger changed during append; expected length {}, found {}",
            expected_final_len,
            final_len
        );
    }
    let mut committed_frame = vec![0_u8; payload.len()];
    file.seek(std::io::SeekFrom::Start(base_len))
        .context("seek newly committed PinvouOS ledger frame")?;
    file.read_exact(&mut committed_frame)
        .context("read newly committed PinvouOS ledger frame")?;
    if committed_frame != payload {
        bail!("PinvouOS ledger frame was interleaved or replaced during append");
    }
    if let Err(error) = fs2::FileExt::unlock(&file) {
        // 数据已经完成 flush + sync；锁也会随 file drop 释放。此时返回 Err 会把
        // 已提交 frame 误报为失败，并诱发调用方重试同一 sequence。
        log::warn!("unlock PinvouOS ledger {}: {}", path.display(), error);
    }
    ledger_head_from_frame(envelope, expected_final_len, &payload[..payload.len() - 1])
}

pub(super) fn serialize_envelope_frame(envelope: &EventEnvelope) -> Result<Vec<u8>> {
    validate_current_schema_screen_observer_identity(envelope)?;
    let mut payload = serde_json::to_vec(envelope).context("serialize PinvouOS event")?;
    payload.push(b'\n');
    if payload.len() > MAX_RUNTIME_EVENT_FRAME_BYTES {
        bail!(
            "PinvouOS event frame exceeds {} bytes",
            MAX_RUNTIME_EVENT_FRAME_BYTES
        );
    }
    Ok(payload)
}

fn preflight_legacy_memory_migration(
    engine: &OrganizedMemoryDecisionEngine,
    migration: &PendingLegacyMemoryMigration,
) -> Result<()> {
    let envelope = EventEnvelope {
        schema_version: SCHEMA_VERSION,
        sequence: u64::MAX,
        event_id: "event-ffffffffffffffff".to_string(),
        occurred_at_ms: i64::MAX,
        source_actor_id: MEMORY_AGENT_ID.to_string(),
        mission_id: None,
        run_id: None,
        interaction_scope_id: None,
        interaction_run_id: None,
        causation_id: Some(migration.source_event_id.clone()),
        correlation_id: Some("memory-legacy-migration".to_string()),
        event: RuntimeEvent::OrganizedMemoryCheckpointRecorded {
            checkpoint: engine.checkpoint(),
            legacy_source_event_id: Some(migration.source_event_id.clone()),
            legacy_migration: Some(migration.report.clone()),
        },
    };
    serialize_envelope_frame(&envelope)
        .map(|_| ())
        .context("legacy memory migration checkpoint does not fit one durable ledger frame")
}

fn preflight_screen_observer_memory_identity_migration(
    engine: &OrganizedMemoryDecisionEngine,
) -> Result<()> {
    let envelope = EventEnvelope {
        schema_version: SCHEMA_VERSION,
        sequence: u64::MAX,
        event_id: "event-ffffffffffffffff".to_string(),
        occurred_at_ms: i64::MAX,
        source_actor_id: MEMORY_AGENT_ID.to_string(),
        mission_id: None,
        run_id: None,
        interaction_scope_id: None,
        interaction_run_id: None,
        causation_id: None,
        correlation_id: Some("memory-screen-observer-identity-migration".to_string()),
        event: RuntimeEvent::OrganizedMemoryCheckpointRecorded {
            checkpoint: engine.checkpoint(),
            legacy_source_event_id: None,
            legacy_migration: None,
        },
    };
    serialize_envelope_frame(&envelope)
        .map(|_| ())
        .context("Screen Observer memory identity checkpoint does not fit one durable ledger frame")
}

fn ledger_head_from_frame(
    envelope: &EventEnvelope,
    committed_len: u64,
    frame: &[u8],
) -> Result<LedgerHead> {
    if frame.is_empty() || frame.len().saturating_add(1) > MAX_RUNTIME_EVENT_FRAME_BYTES {
        bail!("invalid PinvouOS ledger head frame length {}", frame.len());
    }
    Ok(LedgerHead {
        sequence: envelope.sequence,
        event_id: envelope.event_id.clone(),
        frame_digest: Sha256::digest(frame).into(),
        committed_len,
    })
}

fn read_last_complete_ledger_head(file: &mut fs::File) -> Result<Option<LedgerHead>> {
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    if length == 0 {
        return Ok(None);
    }
    file.seek(std::io::SeekFrom::End(-1))
        .context("seek PinvouOS ledger commit marker")?;
    let mut marker = [0_u8; 1];
    file.read_exact(&mut marker)
        .context("read PinvouOS ledger commit marker")?;
    if marker[0] != b'\n' {
        bail!("PinvouOS ledger tail has no commit marker after torn-frame repair");
    }

    const SEARCH_CHUNK_BYTES: usize = 8 * 1024;
    let frame_end = length - 1;
    let mut cursor = frame_end;
    let mut frame_start = 0_u64;
    let mut chunk = vec![0_u8; SEARCH_CHUNK_BYTES];
    while cursor > 0 {
        let chunk_start = cursor.saturating_sub(SEARCH_CHUNK_BYTES as u64);
        let chunk_len = (cursor - chunk_start) as usize;
        file.seek(std::io::SeekFrom::Start(chunk_start))
            .context("seek previous PinvouOS ledger frame")?;
        file.read_exact(&mut chunk[..chunk_len])
            .context("scan previous PinvouOS ledger frame")?;
        if let Some(position) = chunk[..chunk_len].iter().rposition(|byte| *byte == b'\n') {
            frame_start = chunk_start + position as u64 + 1;
            break;
        }
        cursor = chunk_start;
    }
    let frame_len = (frame_end - frame_start) as usize;
    if frame_len == 0 {
        bail!("PinvouOS ledger ends with an empty committed frame");
    }
    if frame_len.saturating_add(1) > MAX_RUNTIME_EVENT_FRAME_BYTES {
        bail!(
            "PinvouOS ledger frame exceeds {} bytes",
            MAX_RUNTIME_EVENT_FRAME_BYTES
        );
    }
    let mut frame = vec![0_u8; frame_len];
    file.seek(std::io::SeekFrom::Start(frame_start))
        .context("seek last PinvouOS ledger frame")?;
    file.read_exact(&mut frame)
        .context("read last PinvouOS ledger frame")?;
    let envelope = serde_json::from_slice::<EventEnvelope>(&frame)
        .context("decode last complete PinvouOS ledger frame")?;
    ledger_head_from_frame(&envelope, length, &frame).map(Some)
}

fn truncate_torn_ledger_tail(file: &mut fs::File) -> Result<()> {
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    if length == 0 {
        return Ok(());
    }
    file.seek(std::io::SeekFrom::End(-1))
        .context("seek PinvouOS ledger tail")?;
    let mut tail = [0_u8; 1];
    file.read_exact(&mut tail)
        .context("read PinvouOS ledger tail")?;
    if tail[0] == b'\n' {
        return Ok(());
    }

    const SEARCH_CHUNK_BYTES: usize = 8 * 1024;
    let mut cursor = length;
    let mut chunk = vec![0_u8; SEARCH_CHUNK_BYTES];
    while cursor > 0 {
        let chunk_start = cursor.saturating_sub(SEARCH_CHUNK_BYTES as u64);
        let chunk_len = (cursor - chunk_start) as usize;
        file.seek(std::io::SeekFrom::Start(chunk_start))
            .context("seek torn PinvouOS ledger frame")?;
        file.read_exact(&mut chunk[..chunk_len])
            .context("scan torn PinvouOS ledger frame")?;
        if let Some(position) = chunk[..chunk_len].iter().rposition(|byte| *byte == b'\n') {
            file.set_len(chunk_start + position as u64 + 1)
                .context("truncate torn PinvouOS ledger frame")?;
            return Ok(());
        }
        cursor = chunk_start;
    }
    file.set_len(0)
        .context("truncate sole torn PinvouOS ledger frame")?;
    Ok(())
}

fn replay_ledger_events(events: &[EventEnvelope]) -> RuntimeSnapshot {
    let mut snapshot = RuntimeSnapshot::default();
    for event in events {
        apply_event(&mut snapshot, &event);
    }
    snapshot
}

fn validate_ledger_event_chain(events: &[EventEnvelope]) -> Result<()> {
    let mut expected_sequence = 1_u64;
    let mut event_ids = BTreeSet::new();
    let mut highest_schema = 0_u32;
    for envelope in events {
        if envelope.schema_version == 0 || envelope.schema_version > SCHEMA_VERSION {
            bail!(
                "unsupported PinvouOS ledger schema v{} at sequence {}",
                envelope.schema_version,
                envelope.sequence
            );
        }
        validate_current_schema_screen_observer_identity(envelope)?;
        if envelope.sequence != expected_sequence {
            bail!(
                "PinvouOS ledger sequence gap or duplicate: expected {}, found {}",
                expected_sequence,
                envelope.sequence
            );
        }
        if !event_ids.insert(envelope.event_id.clone()) {
            bail!("duplicate PinvouOS event id {}", envelope.event_id);
        }
        if envelope.schema_version < highest_schema {
            bail!(
                "PinvouOS ledger schema downgraded to v{} after v{} at sequence {}",
                envelope.schema_version,
                highest_schema,
                envelope.sequence
            );
        }
        highest_schema = highest_schema.max(envelope.schema_version);
        if envelope.schema_version >= 3 {
            if envelope.event_id != format!("event-{:016x}", envelope.sequence) {
                bail!(
                    "non-canonical v{} PinvouOS event id {} at sequence {}",
                    envelope.schema_version,
                    envelope.event_id,
                    envelope.sequence
                );
            }
        }
        expected_sequence = expected_sequence
            .checked_add(1)
            .ok_or_else(|| anyhow!("PinvouOS ledger sequence overflow"))?;
    }
    Ok(())
}

/// schema v5 之后，旧观察者标识只能存在于不可变的历史事件。这里仅检查带身份
/// 语义的 typed 字段；Claim value、Memory value、说明文本等普通内容即使恰好包含
/// 同样字符串，也不能被误判为协议身份。
pub(super) fn validate_current_schema_screen_observer_identity(
    envelope: &EventEnvelope,
) -> Result<()> {
    if envelope.schema_version < SCREEN_OBSERVER_IDENTITY_SCHEMA_VERSION {
        return Ok(());
    }
    if envelope.source_actor_id == LEGACY_SURFACE_AGENT_ID {
        bail!(
            "schema-v{} event {} uses the legacy Screen Observer source actor",
            envelope.schema_version,
            envelope.event_id
        );
    }
    match &envelope.event {
        RuntimeEvent::AgentRegistered { agent } => {
            if agent.agent_id == LEGACY_SURFACE_AGENT_ID {
                bail!(
                    "schema-v{} agent registration {} uses the legacy Screen Observer agent id",
                    envelope.schema_version,
                    envelope.event_id
                );
            }
            if agent
                .capabilities
                .iter()
                .any(|capability| capability.capability_id == LEGACY_SURFACE_OBSERVE_CAPABILITY_ID)
            {
                bail!(
                    "schema-v{} agent registration {} uses the legacy Screen Observer capability id",
                    envelope.schema_version,
                    envelope.event_id
                );
            }
        }
        RuntimeEvent::ClaimAsserted { claim }
            if claim.asserted_by_actor_id == LEGACY_SURFACE_AGENT_ID =>
        {
            bail!(
                "schema-v{} claim {} uses the legacy Screen Observer actor id",
                envelope.schema_version,
                envelope.event_id
            );
        }
        RuntimeEvent::DirectiveIssued { directive }
            if directive.target_agent_id == LEGACY_SURFACE_AGENT_ID =>
        {
            bail!(
                "schema-v{} directive {} uses the legacy Screen Observer target id",
                envelope.schema_version,
                envelope.event_id
            );
        }
        RuntimeEvent::DirectiveAcknowledged {
            target_agent_id, ..
        } if target_agent_id == LEGACY_SURFACE_AGENT_ID => {
            bail!(
                "schema-v{} directive acknowledgement {} uses the legacy Screen Observer target id",
                envelope.schema_version,
                envelope.event_id
            );
        }
        RuntimeEvent::MemoryProjectionUpdated { .. } => {
            bail!(
                "schema-v{} event {} re-emits the retired MemoryProjectionUpdated wire shape",
                envelope.schema_version,
                envelope.event_id
            );
        }
        RuntimeEvent::OrganizedMemoryDecisionRecorded { decision }
            if decision_reemits_legacy_screen_observer_actor(decision) =>
        {
            bail!(
                "schema-v{} memory decision {} re-emits legacy Screen Observer actor",
                envelope.schema_version,
                envelope.event_id
            );
        }
        RuntimeEvent::OrganizedMemoryCheckpointRecorded { checkpoint, .. }
            if memory_state_reemits_legacy_screen_observer_actor(&checkpoint.state) =>
        {
            bail!(
                "schema-v{} memory checkpoint {} re-emits legacy Screen Observer actor",
                envelope.schema_version,
                envelope.event_id
            );
        }
        _ => {}
    }
    Ok(())
}

#[derive(Clone)]
enum MemoryRecoveryEntry {
    Decision(OrganizedMemoryDecisionBatch),
    Checkpoint {
        envelope_schema_version: u32,
        checkpoint: OrganizedMemoryDecisionCheckpoint,
    },
}

fn replay_memory_engine(events: &[EventEnvelope]) -> Result<ReplayedMemoryEngine> {
    let mut latest_legacy = None::<(MemoryProjectionState, TrustedMemoryEvidence)>;
    let mut entries = Vec::<MemoryRecoveryEntry>::new();
    let mut saw_new_memory_stream = false;
    for envelope in events {
        match &envelope.event {
            RuntimeEvent::MemoryProjectionUpdated {
                revision,
                projection,
                ..
            } => {
                if saw_new_memory_stream {
                    bail!(
                        "legacy memory projection {} appears after the organized memory stream; refusing a split-brain ledger",
                        envelope.event_id
                    );
                }
                let projection =
                    serde_json::from_value::<MemoryProjectionState>(projection.clone())
                        .with_context(|| {
                            format!(
                                "decode legacy MemoryAgent projection at {}",
                                envelope.event_id
                            )
                        })?;
                if projection.revision != *revision {
                    bail!(
                        "legacy MemoryAgent projection revision mismatch at {}: event={} payload={}",
                        envelope.event_id,
                        revision,
                        projection.revision
                    );
                }
                // 使用旧实现仅验证旧 wire state；它不再成为 Runtime 的可写真相源。
                MemoryAgent::from_projection(projection.clone())
                    .map_err(|error| anyhow!(error.to_string()))
                    .with_context(|| {
                        format!(
                            "validate legacy MemoryAgent projection at {}",
                            envelope.event_id
                        )
                    })?;
                latest_legacy = Some((
                    projection,
                    TrustedMemoryEvidence {
                        event_id: envelope.event_id.clone(),
                        source_actor_id: envelope.source_actor_id.clone(),
                        origin: MemoryEvidenceOrigin::ModelInference,
                        observed_at_ms: envelope.occurred_at_ms,
                        recorded_at_ms: envelope.occurred_at_ms,
                        reliability: 0.45,
                        mission_id: envelope.mission_id.clone(),
                        run_id: envelope.run_id.clone(),
                        binding: None,
                    },
                ));
            }
            RuntimeEvent::OrganizedMemoryDecisionRecorded { decision } => {
                if envelope.source_actor_id != MEMORY_AGENT_ID {
                    bail!(
                        "organized memory decision {} was not emitted by the Memory Agent",
                        envelope.event_id
                    );
                }
                if envelope.schema_version >= SCREEN_OBSERVER_IDENTITY_SCHEMA_VERSION
                    && decision_reemits_legacy_screen_observer_actor(decision)
                {
                    bail!(
                        "schema-v{} memory decision {} re-emits legacy Screen Observer actor",
                        envelope.schema_version,
                        envelope.event_id
                    );
                }
                if !saw_new_memory_stream && latest_legacy.is_some() {
                    bail!(
                        "organized memory decisions cannot follow a legacy projection without a migration checkpoint"
                    );
                }
                saw_new_memory_stream = true;
                entries.push(MemoryRecoveryEntry::Decision(decision.clone()));
            }
            RuntimeEvent::OrganizedMemoryCheckpointRecorded {
                checkpoint,
                legacy_source_event_id,
                legacy_migration,
            } => {
                if envelope.source_actor_id != MEMORY_AGENT_ID {
                    bail!(
                        "organized memory checkpoint {} was not emitted by the Memory Agent",
                        envelope.event_id
                    );
                }
                if envelope.schema_version >= SCREEN_OBSERVER_IDENTITY_SCHEMA_VERSION
                    && memory_state_reemits_legacy_screen_observer_actor(&checkpoint.state)
                {
                    bail!(
                        "schema-v{} memory checkpoint {} re-emits legacy Screen Observer actor",
                        envelope.schema_version,
                        envelope.event_id
                    );
                }
                if !saw_new_memory_stream {
                    match (&latest_legacy, legacy_source_event_id, legacy_migration) {
                        (Some((projection, source)), Some(marker), Some(report))
                            if marker == &source.event_id =>
                        {
                            let (mut expected_engine, expected_report) =
                                migrate_legacy_memory_projection(projection, source)
                                    .map_err(|error| anyhow!(error.to_string()))?;
                            if envelope.schema_version >= SCREEN_OBSERVER_IDENTITY_SCHEMA_VERSION {
                                canonicalize_screen_observer_memory_evidence(
                                    &mut expected_engine,
                                )?;
                            }
                            let expected = expected_engine.checkpoint();
                            if report != &expected_report
                                || checkpoint.last_sequence != expected.last_sequence
                                || checkpoint.last_decision_hash != expected.last_decision_hash
                                || checkpoint.state != expected.state
                            {
                                bail!(
                                    "organized memory migration checkpoint does not match the deterministic legacy import"
                                );
                            }
                        }
                        (None, None, None) => {}
                        (Some((_, source)), _, _) => bail!(
                            "organized memory migration checkpoint does not match latest legacy projection {}",
                            source.event_id
                        ),
                        (None, Some(_), _) | (None, None, Some(_)) => bail!(
                            "organized memory checkpoint claims a legacy migration without a legacy projection"
                        ),
                    }
                } else if legacy_source_event_id.is_some() || legacy_migration.is_some() {
                    bail!("legacy migration marker may only appear on the first memory checkpoint");
                }
                saw_new_memory_stream = true;
                entries.push(MemoryRecoveryEntry::Checkpoint {
                    envelope_schema_version: envelope.schema_version,
                    checkpoint: checkpoint.clone(),
                });
            }
            _ => {}
        }
    }

    if saw_new_memory_stream {
        let mut engine = recover_organized_memory_entries(&entries)?;
        let pending_screen_observer_identity_migration =
            canonicalize_screen_observer_memory_evidence(&mut engine)?;
        return Ok(ReplayedMemoryEngine {
            engine,
            pending_legacy_migration: None,
            pending_screen_observer_identity_migration,
        });
    }
    if let Some((projection, source)) = latest_legacy {
        let source_event_id = source.event_id.clone();
        let (mut engine, report) = migrate_legacy_memory_projection(&projection, &source)
            .map_err(|error| anyhow!(error.to_string()))?;
        let pending_screen_observer_identity_migration =
            canonicalize_screen_observer_memory_evidence(&mut engine)?;
        return Ok(ReplayedMemoryEngine {
            engine,
            pending_legacy_migration: Some(PendingLegacyMemoryMigration {
                source_event_id,
                report,
            }),
            pending_screen_observer_identity_migration,
        });
    }
    Ok(ReplayedMemoryEngine {
        engine: OrganizedMemoryDecisionEngine::new(),
        pending_legacy_migration: None,
        pending_screen_observer_identity_migration: false,
    })
}

fn canonicalize_screen_observer_memory_evidence(
    engine: &mut OrganizedMemoryDecisionEngine,
) -> Result<bool> {
    engine
        .rewrite_evidence_source_actor_ids(|actor_id| {
            canonical_screen_observer_agent_id(actor_id).to_string()
        })
        .map_err(|error| anyhow!(error.to_string()))
}

fn decision_reemits_legacy_screen_observer_actor(decision: &OrganizedMemoryDecisionBatch) -> bool {
    decision
        .delta
        .record_upserts
        .iter()
        .any(memory_record_reemits_legacy_screen_observer_actor)
}

fn memory_state_reemits_legacy_screen_observer_actor(
    state: &super::memory_agent::MemoryOrganizerState,
) -> bool {
    state
        .records
        .values()
        .any(memory_record_reemits_legacy_screen_observer_actor)
}

fn memory_record_reemits_legacy_screen_observer_actor(record: &OrganizedMemory) -> bool {
    record
        .supporting_evidence
        .iter()
        .chain(record.contradicting_evidence.iter())
        .chain(
            record
                .retraction
                .iter()
                .flat_map(|retraction| retraction.evidence.iter()),
        )
        .any(|evidence| evidence.source_actor_id == LEGACY_SURFACE_AGENT_ID)
}

fn recover_organized_memory_entries(
    entries: &[MemoryRecoveryEntry],
) -> Result<OrganizedMemoryDecisionEngine> {
    let mut base = None::<OrganizedMemoryDecisionCheckpoint>;
    let mut tail = Vec::<OrganizedMemoryDecisionBatch>::new();
    let mut has_prior_history = false;
    for entry in entries {
        match entry {
            MemoryRecoveryEntry::Decision(decision) => {
                tail.push(decision.clone());
                has_prior_history = true;
            }
            MemoryRecoveryEntry::Checkpoint {
                envelope_schema_version,
                checkpoint,
            } => {
                let checkpoint_engine = OrganizedMemoryDecisionEngine::from_checkpoint(
                    checkpoint.clone(),
                    std::iter::empty(),
                )
                .map_err(|error| anyhow!(error.to_string()))?;
                if has_prior_history {
                    let mut prior = recover_memory_segment(base.take(), std::mem::take(&mut tail))?;
                    if *envelope_schema_version >= SCREEN_OBSERVER_IDENTITY_SCHEMA_VERSION {
                        canonicalize_screen_observer_memory_evidence(&mut prior)?;
                    }
                    if prior.last_sequence() != checkpoint_engine.last_sequence()
                        || prior.last_decision_hash() != checkpoint_engine.last_decision_hash()
                        || prior.organizer().state() != checkpoint_engine.organizer().state()
                    {
                        bail!("organized memory checkpoint does not match the preceding decision head");
                    }
                }
                base = Some(checkpoint.clone());
                tail.clear();
                has_prior_history = true;
            }
        }
    }
    recover_memory_segment(base, tail)
}

fn recover_memory_segment(
    base: Option<OrganizedMemoryDecisionCheckpoint>,
    tail: Vec<OrganizedMemoryDecisionBatch>,
) -> Result<OrganizedMemoryDecisionEngine> {
    match base {
        Some(checkpoint) => OrganizedMemoryDecisionEngine::from_checkpoint(checkpoint, tail)
            .map_err(|error| anyhow!(error.to_string())),
        None => {
            OrganizedMemoryDecisionEngine::replay(tail).map_err(|error| anyhow!(error.to_string()))
        }
    }
}

fn read_ledger_events(path: &Path) -> Result<(Vec<EventEnvelope>, u64, Option<LedgerHead>)> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Vec::new(), 0, None))
        }
        Err(error) => {
            return Err(error).with_context(|| format!("read PinvouOS ledger {}", path.display()))
        }
    };
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct EventSchemaProbe {
        schema_version: u32,
    }

    let mut events = Vec::new();
    let mut committed_len = 0_u64;
    let mut durable_head = None;
    let mut reader = BufReader::new(file);
    let mut index = 0usize;
    while let Some((line, complete)) = read_bounded_ledger_line(&mut reader)? {
        index = index.saturating_add(1);
        if !complete {
            log::warn!(
                "ignoring uncommitted final PinvouOS ledger frame {} in {}",
                index,
                path.display()
            );
            break;
        }
        committed_len = committed_len
            .checked_add(line.len() as u64)
            .ok_or_else(|| anyhow!("PinvouOS ledger length overflow"))?;
        if line.iter().all(|byte| byte.is_ascii_whitespace()) {
            bail!(
                "PinvouOS ledger contains an empty committed frame at line {}",
                index
            );
        }
        if let Ok(probe) = serde_json::from_slice::<EventSchemaProbe>(&line) {
            if probe.schema_version > SCHEMA_VERSION {
                bail!(
                    "PinvouOS ledger schema v{} is newer than supported v{}",
                    probe.schema_version,
                    SCHEMA_VERSION
                );
            }
        }
        match serde_json::from_slice::<EventEnvelope>(&line) {
            Ok(event) => {
                let frame = line
                    .strip_suffix(b"\n")
                    .ok_or_else(|| anyhow!("committed PinvouOS frame has no newline marker"))?;
                durable_head = Some(ledger_head_from_frame(&event, committed_len, frame)?);
                events.push(event);
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "decode complete PinvouOS ledger line {} in {}",
                        index,
                        path.display()
                    )
                })
            }
        }
    }
    Ok((events, committed_len, durable_head))
}

fn read_bounded_ledger_line(reader: &mut impl std::io::BufRead) -> Result<Option<(Vec<u8>, bool)>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().context("read PinvouOS ledger frame")?;
        if available.is_empty() {
            return Ok((!line.is_empty()).then_some((line, false)));
        }
        let chunk_len = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        if line.len().saturating_add(chunk_len) > MAX_RUNTIME_EVENT_FRAME_BYTES {
            bail!(
                "PinvouOS ledger frame exceeds {} bytes",
                MAX_RUNTIME_EVENT_FRAME_BYTES
            );
        }
        line.extend_from_slice(&available[..chunk_len]);
        let ended = available[chunk_len - 1] == b'\n';
        reader.consume(chunk_len);
        if ended {
            return Ok(Some((line, true)));
        }
    }
}

fn build_trusted_memory_evidence_index(
    events: &[EventEnvelope],
    snapshot: &RuntimeSnapshot,
) -> BTreeMap<String, TrustedMemoryEvidence> {
    events
        .iter()
        .filter_map(|envelope| trusted_memory_evidence_from_envelope(envelope, snapshot))
        .map(|evidence| (evidence.event_id.clone(), evidence))
        .collect()
}

fn validate_memory_evidence_request(evidence_event_ids: &[String]) -> Result<()> {
    if evidence_event_ids.is_empty() {
        bail!("memory mutation requires at least one evidence event");
    }
    if evidence_event_ids.len() > super::memory_agent::MAX_MEMORY_EVIDENCE_PER_CANDIDATE {
        bail!(
            "memory mutation evidence exceeds {} items",
            super::memory_agent::MAX_MEMORY_EVIDENCE_PER_CANDIDATE
        );
    }
    Ok(())
}

fn collect_current_memory_evidence(
    evidence_event_ids: &[String],
    index: &BTreeMap<String, TrustedMemoryEvidence>,
    snapshot: &RuntimeSnapshot,
) -> Result<BTreeMap<String, TrustedMemoryEvidence>> {
    let requested = evidence_event_ids.iter().cloned().collect::<BTreeSet<_>>();
    let trusted = requested
        .iter()
        .filter_map(|event_id| {
            index
                .get(event_id)
                .filter(|evidence| trusted_evidence_is_current(evidence, snapshot))
                .cloned()
                .map(|evidence| (event_id.clone(), evidence))
        })
        .collect::<BTreeMap<_, _>>();
    if trusted.len() != requested.len() {
        bail!("memory mutation references an unknown or ineligible evidence event");
    }
    Ok(trusted)
}

fn trusted_memory_evidence_from_envelope(
    envelope: &EventEnvelope,
    snapshot: &RuntimeSnapshot,
) -> Option<TrustedMemoryEvidence> {
    let (binding, observed_at_ms) = match &envelope.event {
        RuntimeEvent::ClaimAsserted { claim }
            if claim.asserted_by_actor_id == envelope.source_actor_id =>
        {
            (
                TrustedMemoryEvidenceBinding::Assertion {
                    claim_id: claim.claim_id.clone(),
                    subject: claim.subject.clone(),
                    predicate: claim.predicate.clone(),
                    value: claim.value.clone(),
                },
                claim.asserted_at_ms,
            )
        }
        RuntimeEvent::ClaimRetracted {
            claim_id,
            retracted_at_ms,
            ..
        } => {
            let claim = snapshot.claims.get(claim_id)?;
            (
                TrustedMemoryEvidenceBinding::Retraction {
                    claim_id: claim_id.clone(),
                    subject: claim.subject.clone(),
                    predicate: claim.predicate.clone(),
                    value: claim.value.clone(),
                },
                *retracted_at_ms,
            )
        }
        _ => return None,
    };
    let (origin, reliability) = if envelope.source_actor_id == "actor:user" {
        (MemoryEvidenceOrigin::UserExplicit, 1.0)
    } else if matches!(
        envelope.source_actor_id.as_str(),
        RESOURCE_AGENT_ID | CONNECTIVITY_AGENT_ID | INFERENCE_AGENT_ID
    ) {
        (MemoryEvidenceOrigin::ExternalSource, 0.85)
    } else {
        (MemoryEvidenceOrigin::AgentAction, 0.7)
    };
    Some(TrustedMemoryEvidence {
        event_id: envelope.event_id.clone(),
        // Raw v4 envelopes remain immutable audit history, but any new decision derived
        // from that evidence must carry only the schema-v5 canonical actor identity.
        source_actor_id: canonical_screen_observer_agent_id(&envelope.source_actor_id).to_string(),
        origin,
        observed_at_ms,
        recorded_at_ms: envelope.occurred_at_ms,
        reliability,
        mission_id: envelope.mission_id.clone(),
        run_id: envelope.run_id.clone(),
        binding: Some(binding),
    })
}

fn trusted_evidence_is_current(
    evidence: &TrustedMemoryEvidence,
    snapshot: &RuntimeSnapshot,
) -> bool {
    match &evidence.binding {
        Some(TrustedMemoryEvidenceBinding::Assertion { claim_id, .. }) => snapshot
            .claims
            .get(claim_id)
            .is_some_and(|claim| claim.active),
        Some(TrustedMemoryEvidenceBinding::Retraction { claim_id, .. }) => snapshot
            .claims
            .get(claim_id)
            .is_some_and(|claim| !claim.active),
        None => false,
    }
}

fn memory_event_context(command_id: &str, evidence: &[MemoryEvidence]) -> EventContext {
    let first = evidence.first();
    let common_mission = first
        .and_then(|item| item.mission_id.clone())
        .filter(|mission_id| {
            evidence
                .iter()
                .all(|item| item.mission_id.as_ref() == Some(mission_id))
        });
    let common_run = first.and_then(|item| item.run_id.clone()).filter(|run_id| {
        evidence
            .iter()
            .all(|item| item.run_id.as_ref() == Some(run_id))
    });
    EventContext {
        source_actor_id: MEMORY_AGENT_ID.to_string(),
        mission_id: common_mission,
        run_id: common_run,
        interaction_scope_id: None,
        interaction_run_id: None,
        causation_id: first.map(|item| item.event_id.clone()),
        correlation_id: Some(command_id.to_string()),
    }
}

fn interaction_event_context(interaction_run_id: &str) -> EventContext {
    EventContext {
        source_actor_id: super::front_agent::FRONT_AGENT_ID.to_string(),
        interaction_scope_id: Some(PINVOU_INTERACTION_SCOPE_ID.to_string()),
        interaction_run_id: Some(interaction_run_id.to_string()),
        correlation_id: Some(interaction_run_id.to_string()),
        ..EventContext::default()
    }
}

fn ready_memory_engine(state: &MemoryEngineState) -> Result<&OrganizedMemoryDecisionEngine> {
    match state {
        MemoryEngineState::Ready(engine) => Ok(engine),
        MemoryEngineState::Poisoned { reason } => {
            bail!("Memory Agent is isolated after a persistence failure: {reason}")
        }
    }
}

fn ready_memory_engine_mut(
    state: &mut MemoryEngineState,
) -> Result<&mut OrganizedMemoryDecisionEngine> {
    match state {
        MemoryEngineState::Ready(engine) => Ok(engine),
        MemoryEngineState::Poisoned { reason } => {
            bail!("Memory Agent is isolated after a persistence failure: {reason}")
        }
    }
}

fn apply_event(snapshot: &mut RuntimeSnapshot, envelope: &EventEnvelope) {
    snapshot.last_sequence = envelope.sequence;
    match &envelope.event {
        RuntimeEvent::RuntimeStarted { .. } => {}
        RuntimeEvent::IdentityDeclared { identity } => snapshot.identity = Some(identity.clone()),
        RuntimeEvent::AgentRegistered { agent } => {
            let agent = upcast_agent_manifest(agent);
            snapshot.agents.insert(agent.agent_id.clone(), agent);
        }
        RuntimeEvent::MissionOpened { mission } => {
            snapshot
                .missions
                .insert(mission.mission_id.clone(), mission.clone());
        }
        RuntimeEvent::RunStarted { run } => {
            snapshot.runs.insert(run.run_id.clone(), run.clone());
        }
        RuntimeEvent::InteractionRunOpened { interaction_run } => {
            snapshot.interaction_runs.insert(
                interaction_run.interaction_run_id.clone(),
                interaction_run.clone(),
            );
        }
        RuntimeEvent::InteractionRunStarted {
            interaction_run_id,
            started_at_ms,
        } => {
            if let Some(interaction_run) = snapshot.interaction_runs.get_mut(interaction_run_id) {
                interaction_run.status = InteractionRunStatus::Running;
                interaction_run.started_at_ms = Some(*started_at_ms);
            }
        }
        RuntimeEvent::InteractionRunFinished {
            interaction_run_id,
            outcome,
            finished_at_ms,
        } => {
            if let Some(interaction_run) = snapshot.interaction_runs.get_mut(interaction_run_id) {
                interaction_run.status = match outcome {
                    InteractionRunOutcome::Success => InteractionRunStatus::Completed,
                    InteractionRunOutcome::Interrupt { .. } => InteractionRunStatus::Interrupted,
                    InteractionRunOutcome::Error { .. } => InteractionRunStatus::Failed,
                    InteractionRunOutcome::Cancelled => InteractionRunStatus::Cancelled,
                };
                interaction_run.finished_at_ms = Some(*finished_at_ms);
                interaction_run.outcome = Some(outcome.clone());
            }
        }
        RuntimeEvent::InteractionToolStarted { .. }
        | RuntimeEvent::InteractionToolFinished { .. }
        | RuntimeEvent::InteractionAssistantMessageCompleted { .. } => {}
        RuntimeEvent::ResourceObserved {
            observation,
            pressure,
        } => {
            snapshot.resources.pressure = *pressure;
            snapshot.resources.last_observation = Some(observation.clone());
        }
        RuntimeEvent::ConnectivityObserved { observation } => {
            snapshot.connectivity.status = observation.status;
            snapshot.connectivity.checked_at_ms = Some(observation.checked_at_ms);
            snapshot.connectivity.latency_ms = observation.latency_ms;
            snapshot.connectivity.reason_code = observation.reason_code.clone();
        }
        RuntimeEvent::InferenceHealthObserved { observation } => {
            snapshot.inference.status = observation.status;
            snapshot.inference.checked_at_ms = Some(observation.checked_at_ms);
            snapshot.inference.probe_latency_ms = observation.probe_latency_ms;
            if observation.model.is_some() {
                snapshot.inference.model = observation.model.clone();
            }
            if observation.provider.is_some() {
                snapshot.inference.provider = observation.provider.clone();
            }
            snapshot.inference.reason_code = observation.reason_code.clone();
        }
        RuntimeEvent::InferenceCompleted { observation } => {
            snapshot.inference.status = InferenceStatus::Ready;
            snapshot.inference.model = Some(observation.model.clone());
            snapshot.inference.last_success_at_ms = Some(observation.completed_at_ms);
            snapshot.inference.last_success_latency_ms = Some(observation.latency_ms);
            snapshot.inference.reason_code = None;
        }
        RuntimeEvent::ClaimAsserted { claim } => {
            let mut claim = claim.clone();
            claim.asserted_by_actor_id =
                canonical_screen_observer_agent_id(&claim.asserted_by_actor_id).to_string();
            if claim.subject == "device.resources" && claim.predicate == "pressure_level" {
                snapshot.resources.active_pressure_claim_id = Some(claim.claim_id.clone());
            }
            snapshot.claims.insert(claim.claim_id.clone(), claim);
        }
        RuntimeEvent::ClaimRetracted {
            claim_id,
            retracted_at_ms,
            reason,
        } => {
            if let Some(claim) = snapshot.claims.get_mut(claim_id) {
                claim.active = false;
                claim.retracted_at_ms = Some(*retracted_at_ms);
                claim.retraction_reason = Some(reason.clone());
            }
            if snapshot.resources.active_pressure_claim_id.as_deref() == Some(claim_id) {
                snapshot.resources.active_pressure_claim_id = None;
            }
        }
        RuntimeEvent::DirectiveIssued { directive } => {
            let mut directive = directive.clone();
            directive.target_agent_id =
                canonical_screen_observer_agent_id(&directive.target_agent_id).to_string();
            if let Some(agent) = snapshot.agents.get_mut(&directive.target_agent_id) {
                agent.desired_state = match directive.action {
                    DirectiveAction::Pause => AgentState::Paused,
                    DirectiveAction::Resume => AgentState::Running,
                    DirectiveAction::Stop => AgentState::Stopped,
                };
            }
            snapshot
                .directives
                .insert(directive.directive_id.clone(), directive.clone());
        }
        RuntimeEvent::DirectiveAcknowledged {
            directive_id,
            target_agent_id,
            status,
            resulting_state,
            acknowledged_at_ms,
            detail,
        } => {
            if let Some(directive) = snapshot.directives.get_mut(directive_id) {
                directive.status = *status;
                directive.acknowledged_at_ms = Some(*acknowledged_at_ms);
                directive.acknowledgement_detail = Some(detail.clone());
            }
            let target_agent_id = canonical_screen_observer_agent_id(target_agent_id);
            if let Some(agent) = snapshot.agents.get_mut(target_agent_id) {
                if *status == DirectiveStatus::Applied {
                    agent.observed_state = *resulting_state;
                } else {
                    // 签发时先投影 desired_state；Adapter 拒绝后必须撤回该期望，
                    // 否则 Runtime 会把“没有执行”永久误报为已受控。
                    agent.desired_state = agent.observed_state;
                }
            }
        }
        RuntimeEvent::MemoryProjectionUpdated { .. }
        | RuntimeEvent::OrganizedMemoryDecisionRecorded { .. }
        | RuntimeEvent::OrganizedMemoryCheckpointRecorded { .. } => {}
    }
}

/// v4 及更早账本使用过 `agent:surface` / `surface.observe`。账本保持原始字节不变，
/// replay projector 只把派生快照提升到 canonical Screen Observer 标识。schema v5 的
/// RuntimeStarted 是降级栅栏，旧二进制不会在 canonical 事件之后继续写旧标识。
fn upcast_agent_manifest(agent: &AgentManifest) -> AgentManifest {
    let mut agent = agent.clone();
    agent.agent_id = canonical_screen_observer_agent_id(&agent.agent_id).to_string();
    for capability in &mut agent.capabilities {
        capability.capability_id =
            canonical_screen_observer_capability_id(&capability.capability_id).to_string();
    }
    agent
}

fn required_text(value: &str, label: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        bail!("{label} must not be empty");
    }
    if value.chars().count() > 512 {
        bail!("{label} is too long");
    }
    Ok(value.to_string())
}

fn required_identifier(value: &str, label: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 256 {
        bail!("{label} must contain 1 to 256 characters");
    }
    if !value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | ':' | '_' | '-')
    }) {
        bail!("{label} contains unsupported characters");
    }
    Ok(value.to_string())
}

fn validate_interaction_outcome(outcome: &InteractionRunOutcome) -> Result<()> {
    match outcome {
        InteractionRunOutcome::Interrupt { interrupts } => {
            if interrupts.is_empty() {
                bail!("interrupt outcome must contain at least one interrupt");
            }
            let mut ids = BTreeSet::new();
            for interrupt in interrupts {
                let interrupt_id = required_identifier(&interrupt.interrupt_id, "interrupt id")?;
                if !ids.insert(interrupt_id) {
                    bail!("interrupt outcome contains duplicate interrupt ids");
                }
                required_text(&interrupt.reason, "interrupt reason")?;
                if interrupt.question_count == 0 {
                    bail!("interrupt question count must be positive");
                }
                validate_observation_timestamp(interrupt.created_at_ms, "interrupt")?;
            }
        }
        InteractionRunOutcome::Error { error_code } => {
            required_identifier(error_code, "interaction error code")?;
        }
        InteractionRunOutcome::Success | InteractionRunOutcome::Cancelled => {}
    }
    Ok(())
}

fn sha256_hex(value: &[u8]) -> String {
    let digest = Sha256::digest(value);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn canonicalize_and_validate_capability_contracts(
    capabilities: &mut [CapabilityContract],
) -> Result<()> {
    if capabilities.is_empty() {
        bail!("mission agent must declare at least one capability contract");
    }
    let mut capability_ids = BTreeSet::new();
    for capability in capabilities {
        let capability_id = required_text(&capability.capability_id, "capability id")?;
        capability.capability_id =
            canonical_screen_observer_capability_id(&capability_id).to_string();
        if !capability_ids.insert(capability.capability_id.clone()) {
            bail!(
                "mission agent declares duplicate capability {}",
                capability.capability_id
            );
        }
        required_text(&capability.summary, "capability summary")?;
        if capability.version == 0 {
            bail!("capability version must be positive");
        }
        if !capability.input_schema.is_object() || !capability.output_schema.is_object() {
            bail!("capability input/output schema must be JSON objects");
        }
    }
    Ok(())
}

fn validate_resource_observation(observation: &ResourceObservation) -> Result<()> {
    if observation.sampled_at_ms <= 0 {
        bail!("resource sample timestamp must be positive");
    }
    if observation.sampled_at_ms > now_ms().saturating_add(5_000) {
        bail!("resource sample timestamp is too far in the future");
    }
    for (label, value) in [
        ("cpu_usage_pct", observation.cpu_usage_pct),
        ("memory_used_pct", observation.memory_used_pct),
        ("gpu_usage_pct", observation.gpu_usage_pct),
    ] {
        if value.is_some_and(|value| !value.is_finite() || !(0.0..=100.0).contains(&value)) {
            bail!("{label} must be between 0 and 100");
        }
    }
    if observation
        .temperature_c
        .is_some_and(|value| !value.is_finite() || !(-50.0..=150.0).contains(&value))
    {
        bail!("temperature_c is outside the supported range");
    }
    if observation
        .power_w
        .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        bail!("power_w must be non-negative");
    }
    Ok(())
}

fn validate_observation_timestamp(timestamp_ms: i64, label: &str) -> Result<()> {
    if timestamp_ms <= 0 {
        bail!("{label} timestamp must be positive");
    }
    if timestamp_ms > now_ms().saturating_add(5_000) {
        bail!("{label} timestamp is too far in the future");
    }
    Ok(())
}

fn validate_reason_code(reason_code: Option<&str>) -> Result<()> {
    if reason_code.is_some_and(|reason| {
        reason.trim().is_empty()
            || reason.chars().count() > 128
            || !reason.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            })
    }) {
        bail!("runtime reason code is invalid");
    }
    Ok(())
}

fn resource_relief_is_authoritative(
    snapshot: &RuntimeSnapshot,
    observation: &ResourceObservation,
) -> bool {
    const MAX_RELIEF_SAMPLE_AGE_MS: i64 = 15_000;
    let now = now_ms();
    if now.saturating_sub(observation.sampled_at_ms) > MAX_RELIEF_SAMPLE_AGE_MS {
        return false;
    }
    if snapshot
        .resources
        .last_observation
        .as_ref()
        .is_some_and(|previous| observation.sampled_at_ms < previous.sampled_at_ms)
    {
        return false;
    }

    let Some(claim_id) = snapshot.resources.active_pressure_claim_id.as_deref() else {
        return false;
    };
    let Some(claim) = snapshot.claims.get(claim_id).filter(|claim| claim.active) else {
        return false;
    };
    let required = [
        ("temperatureC", observation.temperature_c.is_some()),
        ("memoryUsedPct", observation.memory_used_pct.is_some()),
        ("cpuUsagePct", observation.cpu_usage_pct.is_some()),
    ];
    let mut has_pressure_evidence = false;
    for (field, present_now) in required {
        if claim.value.get(field).is_some_and(|value| !value.is_null()) {
            has_pressure_evidence = true;
            if !present_now {
                return false;
            }
        }
    }
    has_pressure_evidence
}

fn event_is_renderer_visible(event: &RuntimeEvent) -> bool {
    !matches!(
        event,
        RuntimeEvent::MemoryProjectionUpdated { .. }
            | RuntimeEvent::OrganizedMemoryDecisionRecorded { .. }
            | RuntimeEvent::OrganizedMemoryCheckpointRecorded { .. }
    )
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn new_entity_id(prefix: &str) -> String {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let serial = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!(
        "{prefix}-{:x}-{:x}-{serial:x}",
        now_ms().max(0),
        std::process::id()
    )
}

fn builtin_system_agents(created_at_ms: i64) -> Vec<AgentManifest> {
    vec![
        builtin_agent(
            "agent:front",
            "Front Agent",
            "与用户保持唯一、连续的 Pinvou 交互面",
            capability(
                "user.interact",
                "接收一次用户意图并形成一个一致的 Pinvou 响应",
                ResourceClass::Moderate,
                Interruptibility::Checkpoint,
                &[],
                &[],
            ),
            AgentState::Running,
            100,
            created_at_ms,
        ),
        builtin_agent(
            "agent:orchestrator",
            "Orchestrator Agent",
            "把持续意图拆成 Mission 与 Run，并调度合适的能力 Agent",
            capability(
                "mission.orchestrate",
                "把一个用户目标分解为可并发、可治理的 Agent 工作图",
                ResourceClass::Moderate,
                Interruptibility::Checkpoint,
                &[],
                &[],
            ),
            AgentState::Running,
            100,
            created_at_ms,
        ),
        builtin_agent(
            SCREEN_OBSERVER_AGENT_ID,
            "Screen Observer Agent",
            "观察并归一当前窗口与可访问性界面场景",
            screen_observe_contract(),
            AgentState::Starting,
            85,
            created_at_ms,
        ),
        builtin_agent(
            RESOURCE_AGENT_ID,
            "Resource Agent",
            "持续观察设备关键资源并提交 Claim",
            capability(
                "resource.observe",
                "采集一次设备资源快照并判定资源压力",
                ResourceClass::Light,
                Interruptibility::Immediate,
                &[],
                &["device_metrics_read"],
            ),
            AgentState::Running,
            100,
            created_at_ms,
        ),
        builtin_agent(
            CONNECTIVITY_AGENT_ID,
            "Connectivity Agent",
            "持续验证联网路径、定位网络故障并为恢复动作提供事实",
            connectivity_observe_contract(),
            AgentState::Running,
            100,
            created_at_ms,
        ),
        builtin_agent(
            INFERENCE_AGENT_ID,
            "Inference Agent",
            "持续验证当前大模型路由、凭据与推理可用性",
            inference_observe_contract(),
            AgentState::Running,
            100,
            created_at_ms,
        ),
        builtin_agent(
            "agent:device",
            "Device Agent",
            "维护设备与外设能力事实",
            capability(
                "device.inspect",
                "查询一次设备或连接能力并返回结构化事实",
                ResourceClass::Light,
                Interruptibility::Immediate,
                &[],
                &["device_metadata_read"],
            ),
            AgentState::Starting,
            90,
            created_at_ms,
        ),
        builtin_agent_with_capabilities(
            "agent:capability",
            "Capability Agent",
            "维护原子能力契约与当前可用性",
            capability_agent_capabilities(),
            AgentState::Running,
            100,
            created_at_ms,
        ),
        builtin_agent_with_capabilities(
            "agent:memory",
            "Memory Agent",
            "整理可追溯长期记忆，并提供有界、稳定的已提交上下文投影",
            memory_capabilities(),
            AgentState::Running,
            95,
            created_at_ms,
        ),
        builtin_agent(
            "agent:policy",
            "Policy Agent",
            "把权限、安全与用户边界投影为可执行约束",
            policy_authorize_contract(),
            AgentState::Starting,
            100,
            created_at_ms,
        ),
        builtin_agent(
            "agent:attention",
            "Attention Agent",
            "在并发目标之间分配注意力与打断预算",
            attention_allocate_contract(),
            AgentState::Running,
            95,
            created_at_ms,
        ),
        builtin_agent(
            ASR_CONTEXT_AGENT_ID,
            "ASR Context Agent",
            "每30分钟把连续上下文编译成有界的语音识别术语快照",
            asr_context_compile_contract(),
            AgentState::Running,
            90,
            created_at_ms,
        ),
    ]
}

fn builtin_agent(
    agent_id: &str,
    display_name: &str,
    role: &str,
    capability: CapabilityContract,
    observed_state: AgentState,
    priority: u8,
    created_at_ms: i64,
) -> AgentManifest {
    builtin_agent_with_capabilities(
        agent_id,
        display_name,
        role,
        vec![capability],
        observed_state,
        priority,
        created_at_ms,
    )
}

fn builtin_agent_with_capabilities(
    agent_id: &str,
    display_name: &str,
    role: &str,
    capabilities: Vec<CapabilityContract>,
    observed_state: AgentState,
    priority: u8,
    created_at_ms: i64,
) -> AgentManifest {
    let interruptibility = capabilities
        .iter()
        .map(|capability| capability.interruptibility)
        .max_by_key(|interruptibility| match interruptibility {
            Interruptibility::Immediate => 0,
            Interruptibility::Checkpoint => 1,
            Interruptibility::Atomic => 2,
        })
        .unwrap_or(Interruptibility::Immediate);
    AgentManifest {
        agent_id: agent_id.to_string(),
        display_name: display_name.to_string(),
        kind: AgentKind::System,
        role: role.to_string(),
        interruptibility,
        capabilities,
        priority,
        observed_state,
        desired_state: AgentState::Running,
        mission_id: None,
        run_id: None,
        created_at_ms,
    }
}

fn builtin_manifest_needs_refresh(existing: &AgentManifest, expected: &AgentManifest) -> bool {
    builtin_contract_changed(existing, expected) || builtin_became_operational(existing, expected)
}

fn builtin_became_operational(existing: &AgentManifest, expected: &AgentManifest) -> bool {
    existing.observed_state == AgentState::Starting
        && expected.observed_state == AgentState::Running
}

fn builtin_contract_changed(existing: &AgentManifest, expected: &AgentManifest) -> bool {
    existing.display_name != expected.display_name
        || existing.kind != AgentKind::System
        || existing.role != expected.role
        || existing.capabilities != expected.capabilities
        || existing.priority != expected.priority
        || existing.interruptibility != expected.interruptibility
}

fn capability(
    capability_id: &str,
    summary: &str,
    resource_class: ResourceClass,
    interruptibility: Interruptibility,
    preconditions: &[&str],
    permissions: &[&str],
) -> CapabilityContract {
    CapabilityContract {
        capability_id: capability_id.to_string(),
        version: 1,
        summary: summary.to_string(),
        input_schema: json!({ "type": "object" }),
        output_schema: json!({ "type": "object" }),
        preconditions: preconditions
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        permissions: permissions
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        side_effects: Vec::new(),
        resource_class,
        interruptibility,
        idempotent: false,
    }
}
