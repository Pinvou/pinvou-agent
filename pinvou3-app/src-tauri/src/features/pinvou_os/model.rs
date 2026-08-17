use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SCHEMA_VERSION: u32 = 1;
pub const PINVOU_IDENTITY_ID: &str = "pinvou";
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
    ResourceObserved {
        observation: ResourceObservation,
        pressure: ResourcePressure,
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
    pub claims: BTreeMap<String, WorldClaim>,
    pub directives: BTreeMap<String, ControlDirective>,
    pub resources: ResourceState,
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
            claims: BTreeMap::new(),
            directives: BTreeMap::new(),
            resources: ResourceState::default(),
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
