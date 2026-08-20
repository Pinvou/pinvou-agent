use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

// v5 canonicalizes the legacy observer identity during replay. v6 adds the HostWork control
// ledger without rewriting older frames; replay always projects the current schema.
pub const SCHEMA_VERSION: u32 = 6;
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

/// 由受信 Host Supervisor 的固定 `pinvou_app` Status 回执投影出的 cgroup 事实。
///
/// 这里只保存绝对计数，不在采样器里提前消费 delta。Runtime 将它与最后一条已成功
/// 持久化的同实例观测比较；因此账本写失败不会丢掉一次 memory.events 边沿。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppCgroupResourceObservation {
    pub observed_at_ms: i64,
    /// systemd InvocationID。它只用于识别 cumulative counter 是否仍属于同一实例，
    /// 不会成为任意 unit/PID 控制入口。
    pub instance_generation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_current_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_high_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_max_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_events_high: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_events_oom: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_events_oom_kill: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_pressure_full_avg10: Option<f64>,
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
    /// 可选字段保持 schema v6 旧帧逐字兼容；缺失只表示本轮没有新的可信 Supervisor
    /// 事实，绝不能被解释为 cgroup 已恢复。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_cgroup: Option<AppCgroupResourceObservation>,
}

/// 最近一张可独立证明 Critical 的可信 ResourceObserved。sequence 是账本顺序，
/// 用来判断证据发生在某次 HostWork 尝试之前还是之后；不是新的控制 identity。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceGovernanceEvidence {
    pub event_id: String,
    pub sequence: u64,
    pub sampled_at_ms: i64,
    pub pressure_epoch: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceState {
    pub pressure: ResourcePressure,
    /// 只在权威压力等级变化时递增。HostWork 的有界重试由同一 epoch 内持久化的
    /// work_id + generation + action 的 definitive Rejected 次数决定，不靠易失 latch。
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub pressure_epoch: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_observation: Option<ResourceObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_pressure_claim_id: Option<String>,
    /// cgroup Critical 是粘性的：只有同一实例更新、更低于 memory.high 且三个
    /// memory.events 计数均无新增的可信观测才能解除。
    #[serde(default, skip_serializing_if = "is_false")]
    pub app_cgroup_critical: bool,
    /// 最近一条已持久化的可信绝对计数 baseline。普通整机心跳不会清掉它。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_app_cgroup_observation: Option<AppCgroupResourceObservation>,
    /// 只由 fresh、非倒序且已落账的 Critical 样本更新。若它发生在首张 Directive
    /// 之后，即使当时 Directive 尚 Pending，后续 Rejected 也不会丢掉唯一 retry credit。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fresh_critical_evidence: Option<ResourceGovernanceEvidence>,
}

impl Default for ResourceState {
    fn default() -> Self {
        Self {
            pressure: ResourcePressure::Normal,
            pressure_epoch: 0,
            last_observation: None,
            active_pressure_claim_id: None,
            app_cgroup_critical: false,
            last_app_cgroup_observation: None,
            last_fresh_critical_evidence: None,
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

/// Runtime 可治理的宿主工作不是 Agent，也不携带 PID、unit 或命令。具体执行目标只由
/// 受信 Adapter 私有持有；这个枚举仅描述固定的所有权边界。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum HostWorkKind {
    EngineTurn,
    DetachedSubAgent,
    ScheduledRun,
    KnowledgeJob,
    ConnectorJob,
    ManagedChildProcess,
    AppCgroup,
    AsrCgroup,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum HostWorkAction {
    Pause,
    Stop,
    Resume,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostWorkDesiredState {
    Running,
    Paused,
    Stopped,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostWorkObservedState {
    Unknown,
    Starting,
    Running,
    Paused,
    Stopped,
    Completed,
    Failed,
}

/// Host Adapter 的回执只说明调用结果；`Applied` 仍需一次后验 status reconciliation，
/// 不能直接改变 observed state。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostWorkDirectiveAcknowledgement {
    Applied,
    Rejected,
    OutcomeUnknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostWorkReconciliationOutcome {
    Confirmed,
    NotApplied,
    OutcomeUnknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostWorkDirectiveStatus {
    Pending,
    AwaitingReconciliation,
    OutcomeUnknown,
    Reconciled,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostWork {
    pub work_id: String,
    pub generation: u64,
    pub owner: String,
    pub kind: HostWorkKind,
    pub resource_class: ResourceClass,
    pub priority: u8,
    pub interruptibility: Interruptibility,
    pub essential: bool,
    pub governable: bool,
    pub supported_actions: BTreeSet<HostWorkAction>,
    pub desired_state: HostWorkDesiredState,
    pub observed_state: HostWorkObservedState,
    pub registered_at_ms: i64,
    pub last_observed_at_ms: i64,
    /// 只有同一 Governor 已确认暂停的工作才能在 Normal 压力下自动恢复。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub governor_pause_directive_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostWorkDirective {
    pub directive_id: String,
    pub work_id: String,
    pub generation: u64,
    pub action: HostWorkAction,
    pub reason: String,
    pub policy_revision: String,
    /// Directive 签发时的资源压力周期。确定拒绝/未执行只在同一周期内抑制重签；
    /// 压力发生真实变化后才允许 Governor 使用新的 identity 再评估。
    #[serde(default)]
    pub resource_pressure_epoch: u64,
    /// 仅在 Runtime projection 中由承载该 Directive 的 envelope sequence 回填；
    /// 账本事件本身保持 None，从而兼容既有 schema-v6 字节。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issued_event_sequence: Option<u64>,
    pub issued_at_ms: i64,
    pub status: HostWorkDirectiveStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledgement: Option<HostWorkDirectiveAcknowledgement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledged_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledgement_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconciliation: Option<HostWorkReconciliationOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconciled_observed_state: Option<HostWorkObservedState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconciled_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconciliation_detail: Option<String>,
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
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
    HostWorkRegistered {
        work: HostWork,
    },
    HostWorkObserved {
        work_id: String,
        generation: u64,
        observed_state: HostWorkObservedState,
        observed_at_ms: i64,
        detail: String,
    },
    HostWorkDirectiveIssued {
        directive: HostWorkDirective,
    },
    HostWorkDirectiveAcknowledged {
        directive_id: String,
        work_id: String,
        generation: u64,
        acknowledgement: HostWorkDirectiveAcknowledgement,
        acknowledged_at_ms: i64,
        detail: String,
    },
    HostWorkDirectiveReconciled {
        directive_id: String,
        work_id: String,
        generation: u64,
        outcome: HostWorkReconciliationOutcome,
        #[serde(skip_serializing_if = "Option::is_none")]
        observed_state: Option<HostWorkObservedState>,
        reconciled_at_ms: i64,
        detail: String,
    },
    HostWorkUnregistered {
        work_id: String,
        generation: u64,
        unregistered_at_ms: i64,
        reason: String,
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
    #[serde(default)]
    pub host_works: BTreeMap<String, HostWork>,
    #[serde(default)]
    pub host_work_directives: BTreeMap<String, HostWorkDirective>,
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
            host_works: BTreeMap::new(),
            host_work_directives: BTreeMap::new(),
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
    #[serde(default)]
    pub host_work_directives: Vec<HostWorkDirective>,
}
