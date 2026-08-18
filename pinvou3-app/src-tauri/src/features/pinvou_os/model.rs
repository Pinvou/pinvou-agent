use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SCHEMA_VERSION: u32 = 4;
pub const PINVOU_IDENTITY_ID: &str = "pinvou";
pub const PINVOU_INTERACTION_SCOPE_ID: &str = "pinvou:global";
pub const KERNEL_ACTOR_ID: &str = "kernel:pinvou-os";
pub const GOVERNOR_ACTOR_ID: &str = "kernel:resource-governor";
pub const RESOURCE_AGENT_ID: &str = "agent:resource";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PinvouIdentity {
    pub identity_id: String,
    pub display_name: String,
    pub continuity: IdentityContinuity,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IdentityContinuity {
    Continuous,
}

impl Default for PinvouIdentity {
    fn default() -> Self {
        Self {
            identity_id: PINVOU_IDENTITY_ID.to_string(),
            display_name: "Pinvou".to_string(),
            continuity: IdentityContinuity::Continuous,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    System,
    Mission,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Starting,
    Idle,
    Running,
    Paused,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Interruptibility {
    Immediate,
    Checkpoint,
    Atomic,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ResourceClass {
    Light,
    Moderate,
    Heavy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityContract {
    pub capability_id: String,
    pub version: u32,
    pub summary: String,
    pub input_schema: Value,
    pub output_schema: Value,
    #[serde(default)]
    pub preconditions: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub side_effects: Vec<String>,
    pub resource_class: ResourceClass,
    pub interruptibility: Interruptibility,
    pub idempotent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentManifest {
    pub agent_id: String,
    pub display_name: String,
    pub kind: AgentKind,
    pub role: String,
    pub capabilities: Vec<CapabilityContract>,
    pub priority: u8,
    pub interruptibility: Interruptibility,
    pub observed_state: AgentState,
    pub desired_state: AgentState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissionStatus {
    Active,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Mission {
    pub mission_id: String,
    pub objective: String,
    pub priority: u8,
    pub status: MissionStatus,
    pub created_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Paused,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub run_id: String,
    pub mission_id: String,
    pub attempt: u32,
    pub status: RunStatus,
    pub started_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<i64>,
}

/// 用户交互的输入通道。该值会进入持久化的 InteractionRun，
/// 因此属于运行时领域模型，不属于具体 Front Agent 实现。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InteractionModality {
    Voice,
    Text,
    Touch,
    System,
}

/// Front 的一次用户可感知交互运行。它与 Mission/Run 分离：同一连续 Pinvou
/// 可以在没有 Mission 的情况下完成短问答，也可以在一个交互中观察多个后台 Run。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InteractionRunStatus {
    Submitted,
    Running,
    Interrupted,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InteractionInterrupt {
    pub interrupt_id: String,
    pub reason: String,
    pub question_count: u32,
    pub created_at_ms: i64,
}

/// 与 AG-UI 对齐的唯一终态。需要用户输入不是悬空状态，而是带可恢复句柄的
/// interrupt outcome；恢复时会创建新的 interaction run，并引用该句柄。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InteractionRunOutcome {
    Success,
    Interrupt {
        interrupts: Vec<InteractionInterrupt>,
    },
    Error {
        error_code: String,
    },
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InteractionRun {
    pub interaction_run_id: String,
    pub interaction_scope_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_interaction_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_interrupt_id: Option<String>,
    pub input_digest: String,
    pub input_char_count: u32,
    pub modality: InteractionModality,
    pub status: InteractionRunStatus,
    pub submitted_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<InteractionRunOutcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorldClaim {
    pub claim_id: String,
    pub subject: String,
    pub predicate: String,
    pub value: Value,
    pub confidence: f32,
    pub asserted_by_actor_id: String,
    pub evidence_event_ids: Vec<String>,
    pub asserted_at_ms: i64,
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retracted_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retraction_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ResourcePressure {
    Normal,
    Warm,
    Hot,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceObservation {
    pub sampled_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_usage_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_used_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_usage_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_w: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceState {
    pub pressure: ResourcePressure,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_observation: Option<ResourceObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_pressure_claim_id: Option<String>,
}

impl Default for ResourceState {
    fn default() -> Self {
        Self {
            pressure: ResourcePressure::Normal,
            last_observation: None,
            active_pressure_claim_id: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectivityStatus {
    Unknown,
    Online,
    Degraded,
    Offline,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectivityObservation {
    pub checked_at_ms: i64,
    pub status: ConnectivityStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectivityState {
    pub status: ConnectivityStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

impl Default for ConnectivityState {
    fn default() -> Self {
        Self {
            status: ConnectivityStatus::Unknown,
            checked_at_ms: None,
            latency_ms: None,
            reason_code: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InferenceStatus {
    Unknown,
    Ready,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InferenceHealthObservation {
    pub checked_at_ms: i64,
    pub status: InferenceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe_latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InferenceCompletionObservation {
    pub completed_at_ms: i64,
    pub model: String,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InferenceState {
    pub status: InferenceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe_latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_success_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_success_latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

impl Default for InferenceState {
    fn default() -> Self {
        Self {
            status: InferenceStatus::Unknown,
            model: None,
            provider: None,
            checked_at_ms: None,
            probe_latency_ms: None,
            last_success_at_ms: None,
            last_success_latency_ms: None,
            reason_code: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DirectiveAction {
    Pause,
    Resume,
    Stop,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DirectiveStatus {
    Pending,
    Applied,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ControlDirective {
    pub directive_id: String,
    pub target_agent_id: String,
    pub action: DirectiveAction,
    pub reason: String,
    pub hard: bool,
    pub issued_at_ms: i64,
    pub status: DirectiveStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledged_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledgement_detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum RuntimeEvent {
    RuntimeStarted {
        process_id: u32,
    },
    IdentityDeclared {
        identity: PinvouIdentity,
    },
    AgentRegistered {
        agent: AgentManifest,
    },
    MissionOpened {
        mission: Mission,
    },
    RunStarted {
        run: Run,
    },
    InteractionRunOpened {
        interaction_run: InteractionRun,
    },
    InteractionRunStarted {
        interaction_run_id: String,
        started_at_ms: i64,
    },
    InteractionToolStarted {
        interaction_run_id: String,
        tool_call_id: String,
        tool_name: String,
        started_at_ms: i64,
    },
    InteractionToolFinished {
        interaction_run_id: String,
        tool_call_id: String,
        tool_name: String,
        success: bool,
        finished_at_ms: i64,
    },
    InteractionAssistantMessageCompleted {
        interaction_run_id: String,
        message_digest: String,
        message_char_count: u32,
        completed_at_ms: i64,
    },
    InteractionRunFinished {
        interaction_run_id: String,
        outcome: InteractionRunOutcome,
        finished_at_ms: i64,
    },
    ResourceObserved {
        observation: ResourceObservation,
        pressure: ResourcePressure,
    },
    ConnectivityObserved {
        observation: ConnectivityObservation,
    },
    InferenceHealthObserved {
        observation: InferenceHealthObservation,
    },
    InferenceCompleted {
        observation: InferenceCompletionObservation,
    },
    ClaimAsserted {
        claim: WorldClaim,
    },
    ClaimRetracted {
        claim_id: String,
        retracted_at_ms: i64,
        reason: String,
    },
    DirectiveIssued {
        directive: ControlDirective,
    },
    DirectiveAcknowledged {
        directive_id: String,
        target_agent_id: String,
        status: DirectiveStatus,
        resulting_state: AgentState,
        acknowledged_at_ms: i64,
        detail: String,
    },
    /// v2 及更早版本的完整 MemoryAgent 投影。新 Runtime 只读此事件做一次迁移，
    /// 绝不能继续写入；保留 variant 是为了让同一统一账本可向前恢复。
    MemoryProjectionUpdated {
        revision: u64,
        operation: String,
        memory_id: String,
        projection: Value,
    },
    /// Memory decision stream 是统一 Runtime 账本中的逻辑分区，不是第二真相源。
    OrganizedMemoryDecisionRecorded {
        decision: super::memory_agent::OrganizedMemoryDecisionBatch,
    },
    /// 可重建热投影的受信 checkpoint。旧投影迁移只会写一次带来源 marker 的根。
    OrganizedMemoryCheckpointRecorded {
        checkpoint: super::memory_agent::OrganizedMemoryDecisionCheckpoint,
        #[serde(skip_serializing_if = "Option::is_none")]
        legacy_source_event_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        legacy_migration: Option<super::memory_agent::LegacyMemoryMigrationReport>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EventEnvelope {
    pub schema_version: u32,
    pub sequence: u64,
    pub event_id: String,
    pub occurred_at_ms: i64,
    pub source_actor_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interaction_scope_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interaction_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub causation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    pub event: RuntimeEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSnapshot {
    pub schema_version: u32,
    pub last_sequence: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<PinvouIdentity>,
    pub agents: BTreeMap<String, AgentManifest>,
    pub missions: BTreeMap<String, Mission>,
    pub runs: BTreeMap<String, Run>,
    #[serde(default)]
    pub interaction_runs: BTreeMap<String, InteractionRun>,
    pub claims: BTreeMap<String, WorldClaim>,
    pub directives: BTreeMap<String, ControlDirective>,
    pub resources: ResourceState,
    #[serde(default)]
    pub connectivity: ConnectivityState,
    #[serde(default)]
    pub inference: InferenceState,
}

impl Default for RuntimeSnapshot {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            last_sequence: 0,
            identity: None,
            agents: BTreeMap::new(),
            missions: BTreeMap::new(),
            runs: BTreeMap::new(),
            interaction_runs: BTreeMap::new(),
            claims: BTreeMap::new(),
            directives: BTreeMap::new(),
            resources: ResourceState::default(),
            connectivity: ConnectivityState::default(),
            inference: InferenceState::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityAvailabilityState {
    Available,
    TemporarilyUnavailable,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityAvailability {
    pub capability_id: String,
    pub state: CapabilityAvailabilityState,
    pub candidate_agent_ids: Vec<String>,
    pub reason_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MissionStart {
    pub mission: Mission,
    pub run: Run,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceDecision {
    pub pressure: ResourcePressure,
    pub observation_event_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressure_claim_id: Option<String>,
    pub directives: Vec<ControlDirective>,
}
