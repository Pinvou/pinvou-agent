//! PinvouOS Capability Agent 的确定性原子能力。
//!
//! 该 Agent 不向模型自我吹嘘能力，而是只读取已注册 `CapabilityContract`、Agent
//! 实际/期望状态与 Resource Governor 投影，生成“能做 / 暂时不能 / 不支持”报告。
//! 每个结论都携带结构化条件和证据，未知的权限或前置条件明确标成待验证。

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::model::{
    AgentManifest, AgentState, CapabilityAvailabilityState, CapabilityContract, Interruptibility,
    ResourceClass, ResourcePressure, RuntimeSnapshot,
};
use super::screen_observer_agent::canonical_screen_observer_capability_id;

pub const CAPABILITY_AGENT_ID: &str = "agent:capability";
pub const CAPABILITY_REPORT_CAPABILITY_ID: &str = "capability.explain";
pub const MAX_REQUESTED_CAPABILITIES: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityReportRequest {
    /// 明确询问的 capability id；未注册的 id 会进入 unsupported。
    #[serde(default)]
    pub requested_capability_ids: Vec<String>,
    /// 同时报告所有已注册能力，用于回答“我现在能做什么”。
    #[serde(default = "default_include_registered")]
    pub include_registered: bool,
}

fn default_include_registered() -> bool {
    true
}

impl Default for CapabilityReportRequest {
    fn default() -> Self {
        Self {
            requested_capability_ids: Vec::new(),
            include_registered: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityReportStatus {
    Complete,
    Empty,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RequirementKind {
    ExecutorRunnable,
    ResourceFeasible,
    Preconditions,
    Permission,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RequirementStatus {
    Satisfied,
    Unsatisfied,
    MustVerify,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityRequirement {
    pub kind: RequirementKind,
    pub requirement_id: String,
    pub status: RequirementStatus,
    pub reason_code: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityEvidenceKind {
    RegisteredContract,
    AgentRuntimeState,
    ResourceObservation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityEvidence {
    pub evidence_id: String,
    pub kind: CapabilityEvidenceKind,
    pub source_id: String,
    pub fact: String,
    pub value: Value,
    pub projection_sequence: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityExecutorAssessment {
    pub agent_id: String,
    pub capability_version: u32,
    pub summary: String,
    pub observed_state: AgentState,
    pub desired_state: AgentState,
    pub resource_class: ResourceClass,
    pub runnable_now: bool,
    pub requirements: Vec<CapabilityRequirement>,
    pub evidence: Vec<CapabilityEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityAssessment {
    pub capability_id: String,
    pub state: CapabilityAvailabilityState,
    pub reason_codes: Vec<String>,
    pub candidate_agent_ids: Vec<String>,
    pub executors: Vec<CapabilityExecutorAssessment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityReport {
    pub generated_at_ms: i64,
    pub projection_sequence: u64,
    pub resource_pressure: ResourcePressure,
    pub status: CapabilityReportStatus,
    pub can_do: Vec<CapabilityAssessment>,
    pub temporarily_cannot: Vec<CapabilityAssessment>,
    pub unsupported: Vec<CapabilityAssessment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityAgentError {
    message: String,
}

impl CapabilityAgentError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CapabilityAgentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for CapabilityAgentError {}

#[derive(Debug, Clone, Copy, Default)]
pub struct CapabilityAgent;

impl CapabilityAgent {
    /// 从同一个 RuntimeSnapshot 生成一次原子报告。报告不改变运行时状态，因此可重试。
    pub fn report(
        snapshot: &RuntimeSnapshot,
        request: CapabilityReportRequest,
        generated_at_ms: i64,
    ) -> Result<CapabilityReport, CapabilityAgentError> {
        if generated_at_ms < 0 {
            return Err(CapabilityAgentError::new(
                "capability report timestamp must be non-negative",
            ));
        }
        if request.requested_capability_ids.len() > MAX_REQUESTED_CAPABILITIES {
            return Err(CapabilityAgentError::new(format!(
                "requested capabilities exceed limit {MAX_REQUESTED_CAPABILITIES}"
            )));
        }
        let requested = normalize_capability_ids(request.requested_capability_ids)?;
        let mut ids = requested;
        if request.include_registered {
            ids.extend(snapshot.agents.values().flat_map(|agent| {
                agent
                    .capabilities
                    .iter()
                    .map(|capability| capability.capability_id.clone())
            }));
        }

        let mut can_do = Vec::new();
        let mut temporarily_cannot = Vec::new();
        let mut unsupported = Vec::new();
        for capability_id in ids {
            let assessment = assess_capability(snapshot, &capability_id);
            match assessment.state {
                CapabilityAvailabilityState::Available => can_do.push(assessment),
                CapabilityAvailabilityState::TemporarilyUnavailable => {
                    temporarily_cannot.push(assessment)
                }
                CapabilityAvailabilityState::Unsupported => unsupported.push(assessment),
            }
        }
        let status = if can_do.is_empty() && temporarily_cannot.is_empty() && unsupported.is_empty()
        {
            CapabilityReportStatus::Empty
        } else {
            CapabilityReportStatus::Complete
        };
        Ok(CapabilityReport {
            generated_at_ms,
            projection_sequence: snapshot.last_sequence,
            resource_pressure: snapshot.resources.pressure,
            status,
            can_do,
            temporarily_cannot,
            unsupported,
        })
    }
}

/// Capability Agent 注册时应声明的原子能力契约。
pub fn capability_agent_capabilities() -> Vec<CapabilityContract> {
    vec![CapabilityContract {
        capability_id: CAPABILITY_REPORT_CAPABILITY_ID.to_string(),
        version: 1,
        summary: "基于已注册能力、Agent 状态和资源压力解释当前能做、暂时不能做与不支持的事项"
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "requestedCapabilityIds": { "type": "array", "items": { "type": "string" }, "maxItems": MAX_REQUESTED_CAPABILITIES },
                "includeRegistered": { "type": "boolean" }
            }
        }),
        output_schema: json!({
            "type": "object",
            "required": ["generatedAtMs", "projectionSequence", "resourcePressure", "status", "canDo", "temporarilyCannot", "unsupported"]
        }),
        preconditions: Vec::new(),
        permissions: Vec::new(),
        side_effects: Vec::new(),
        resource_class: ResourceClass::Light,
        interruptibility: Interruptibility::Immediate,
        idempotent: true,
    }]
}

fn assess_capability(snapshot: &RuntimeSnapshot, capability_id: &str) -> CapabilityAssessment {
    let mut candidates = snapshot
        .agents
        .values()
        .filter_map(|agent| {
            agent
                .capabilities
                .iter()
                .find(|capability| capability.capability_id == capability_id)
                .map(|capability| assess_executor(snapshot, agent, capability))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
    let candidate_agent_ids = candidates
        .iter()
        .map(|candidate| candidate.agent_id.clone())
        .collect::<Vec<_>>();

    if candidates.is_empty() {
        return CapabilityAssessment {
            capability_id: capability_id.to_string(),
            state: CapabilityAvailabilityState::Unsupported,
            reason_codes: vec!["no_registered_executor".to_string()],
            candidate_agent_ids,
            executors: candidates,
        };
    }

    let available = candidates.iter().any(|candidate| candidate.runnable_now);
    let state = if available {
        CapabilityAvailabilityState::Available
    } else {
        CapabilityAvailabilityState::TemporarilyUnavailable
    };
    let mut reason_codes = BTreeSet::new();
    if available
        && candidates
            .iter()
            .filter(|candidate| candidate.runnable_now)
            .all(|candidate| {
                candidate
                    .requirements
                    .iter()
                    .any(|requirement| requirement.status == RequirementStatus::MustVerify)
            })
    {
        reason_codes.insert("runtime_requirements_must_be_verified".to_string());
    }
    if !available {
        for candidate in &candidates {
            for requirement in &candidate.requirements {
                if requirement.status != RequirementStatus::Satisfied {
                    reason_codes.insert(requirement.reason_code.clone());
                }
            }
        }
    }
    CapabilityAssessment {
        capability_id: capability_id.to_string(),
        state,
        reason_codes: reason_codes.into_iter().collect(),
        candidate_agent_ids,
        executors: candidates,
    }
}

fn assess_executor(
    snapshot: &RuntimeSnapshot,
    agent: &AgentManifest,
    capability: &CapabilityContract,
) -> CapabilityExecutorAssessment {
    let state_runnable = matches!(agent.observed_state, AgentState::Idle | AgentState::Running)
        && matches!(agent.desired_state, AgentState::Idle | AgentState::Running);
    let resource_feasible = capability.resource_class != ResourceClass::Heavy
        || snapshot.resources.pressure < ResourcePressure::Hot;
    let mut requirements = vec![
        CapabilityRequirement {
            kind: RequirementKind::ExecutorRunnable,
            requirement_id: agent.agent_id.clone(),
            status: if state_runnable {
                RequirementStatus::Satisfied
            } else {
                RequirementStatus::Unsatisfied
            },
            reason_code: if state_runnable {
                "executor_runnable".to_string()
            } else {
                "executor_not_runnable".to_string()
            },
        },
        CapabilityRequirement {
            kind: RequirementKind::ResourceFeasible,
            requirement_id: format!("resource_class:{:?}", capability.resource_class)
                .to_ascii_lowercase(),
            status: if resource_feasible {
                RequirementStatus::Satisfied
            } else {
                RequirementStatus::Unsatisfied
            },
            reason_code: if resource_feasible {
                "resource_budget_feasible".to_string()
            } else {
                "blocked_by_resource_governor".to_string()
            },
        },
    ];
    requirements.extend(capability.preconditions.iter().map(|precondition| {
        CapabilityRequirement {
            kind: RequirementKind::Preconditions,
            requirement_id: precondition.clone(),
            status: RequirementStatus::MustVerify,
            reason_code: "precondition_must_be_verified".to_string(),
        }
    }));
    requirements.extend(
        capability
            .permissions
            .iter()
            .map(|permission| CapabilityRequirement {
                kind: RequirementKind::Permission,
                requirement_id: permission.clone(),
                status: RequirementStatus::MustVerify,
                reason_code: "permission_must_be_authorized".to_string(),
            }),
    );
    // 权限与前置条件尚未由 Policy / Device / Screen Observer（界面感知）
    // 的可信事实验证前，只能报告为暂时不可用，不能把“有一个注册合同”误说成“现在就能做”。
    let runtime_requirements_verified = requirements
        .iter()
        .all(|requirement| requirement.status == RequirementStatus::Satisfied);
    let runnable_now = state_runnable && resource_feasible && runtime_requirements_verified;

    let observed_at_ms = snapshot
        .resources
        .last_observation
        .as_ref()
        .map(|observation| observation.sampled_at_ms);
    let evidence = vec![
        CapabilityEvidence {
            evidence_id: format!(
                "capability-contract:{}:{}:v{}",
                agent.agent_id, capability.capability_id, capability.version
            ),
            kind: CapabilityEvidenceKind::RegisteredContract,
            source_id: agent.agent_id.clone(),
            fact: "capability_contract".to_string(),
            value: json!({
                "capabilityId": capability.capability_id,
                "version": capability.version,
                "resourceClass": capability.resource_class,
                "preconditions": capability.preconditions,
                "permissions": capability.permissions,
            }),
            projection_sequence: snapshot.last_sequence,
            observed_at_ms: None,
        },
        CapabilityEvidence {
            evidence_id: format!("agent-state:{}:{}", snapshot.last_sequence, agent.agent_id),
            kind: CapabilityEvidenceKind::AgentRuntimeState,
            source_id: agent.agent_id.clone(),
            fact: "agent_runtime_state".to_string(),
            value: json!({
                "observedState": agent.observed_state,
                "desiredState": agent.desired_state,
            }),
            projection_sequence: snapshot.last_sequence,
            observed_at_ms: None,
        },
        CapabilityEvidence {
            evidence_id: format!("resource-pressure:{}", snapshot.last_sequence),
            kind: CapabilityEvidenceKind::ResourceObservation,
            source_id: "device.resources".to_string(),
            fact: "resource_pressure".to_string(),
            value: json!({ "pressure": snapshot.resources.pressure }),
            projection_sequence: snapshot.last_sequence,
            observed_at_ms,
        },
    ];
    CapabilityExecutorAssessment {
        agent_id: agent.agent_id.clone(),
        capability_version: capability.version,
        summary: capability.summary.clone(),
        observed_state: agent.observed_state,
        desired_state: agent.desired_state,
        resource_class: capability.resource_class,
        runnable_now,
        requirements,
        evidence,
    }
}

fn normalize_capability_ids(values: Vec<String>) -> Result<BTreeSet<String>, CapabilityAgentError> {
    values
        .into_iter()
        .map(|value| {
            let value = value.trim();
            if value.is_empty() {
                return Err(CapabilityAgentError::new("capability id must not be empty"));
            }
            if value.chars().count() > 512 {
                return Err(CapabilityAgentError::new("capability id is too long"));
            }
            Ok(canonical_screen_observer_capability_id(value).to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::model::{AgentKind, ResourceObservation, ResourceState};
    use super::*;

    fn capability(id: &str, resource_class: ResourceClass) -> CapabilityContract {
        CapabilityContract {
            capability_id: id.to_string(),
            version: 1,
            summary: format!("execute {id}"),
            input_schema: json!({ "type": "object" }),
            output_schema: json!({ "type": "object" }),
            preconditions: Vec::new(),
            permissions: Vec::new(),
            side_effects: Vec::new(),
            resource_class,
            interruptibility: Interruptibility::Immediate,
            idempotent: true,
        }
    }

    fn agent(id: &str, state: AgentState, capability: CapabilityContract) -> AgentManifest {
        AgentManifest {
            agent_id: id.to_string(),
            display_name: id.to_string(),
            kind: AgentKind::System,
            role: "test".to_string(),
            capabilities: vec![capability],
            priority: 50,
            interruptibility: Interruptibility::Immediate,
            observed_state: state,
            desired_state: AgentState::Running,
            mission_id: None,
            run_id: None,
            created_at_ms: 1,
        }
    }

    fn snapshot(pressure: ResourcePressure) -> RuntimeSnapshot {
        RuntimeSnapshot {
            last_sequence: 42,
            resources: ResourceState {
                pressure,
                last_observation: Some(ResourceObservation {
                    sampled_at_ms: 40,
                    cpu_usage_pct: Some(10.0),
                    memory_used_pct: Some(20.0),
                    gpu_usage_pct: Some(30.0),
                    temperature_c: Some(50.0),
                    power_w: Some(10.0),
                }),
                active_pressure_claim_id: None,
            },
            ..RuntimeSnapshot::default()
        }
    }

    #[test]
    fn report_separates_can_temporary_and_unsupported_with_evidence() {
        let mut snapshot = snapshot(ResourcePressure::Hot);
        snapshot.agents.insert(
            "agent:ready".to_string(),
            agent(
                "agent:ready",
                AgentState::Running,
                capability("text.reply", ResourceClass::Light),
            ),
        );
        snapshot.agents.insert(
            "agent:starting".to_string(),
            agent(
                "agent:starting",
                AgentState::Starting,
                capability("screen.observe", ResourceClass::Light),
            ),
        );
        snapshot.agents.insert(
            "agent:heavy".to_string(),
            agent(
                "agent:heavy",
                AgentState::Running,
                capability("video.render", ResourceClass::Heavy),
            ),
        );

        let report = CapabilityAgent::report(
            &snapshot,
            CapabilityReportRequest {
                requested_capability_ids: vec!["teleport.execute".to_string()],
                include_registered: true,
            },
            50,
        )
        .unwrap();

        assert_eq!(report.status, CapabilityReportStatus::Complete);
        assert_eq!(report.can_do[0].capability_id, "text.reply");
        assert_eq!(
            report
                .temporarily_cannot
                .iter()
                .map(|item| item.capability_id.as_str())
                .collect::<Vec<_>>(),
            vec!["screen.observe", "video.render"]
        );
        assert!(report.temporarily_cannot[0]
            .reason_codes
            .contains(&"executor_not_runnable".to_string()));
        assert!(report.temporarily_cannot[1]
            .reason_codes
            .contains(&"blocked_by_resource_governor".to_string()));
        assert_eq!(report.unsupported[0].capability_id, "teleport.execute");
        assert_eq!(
            report.unsupported[0].reason_codes,
            vec!["no_registered_executor"]
        );
        assert_eq!(report.can_do[0].executors[0].evidence.len(), 3);
    }

    #[test]
    fn one_runnable_executor_makes_capability_available() {
        let mut snapshot = snapshot(ResourcePressure::Normal);
        let mut blocked_contract = capability("search", ResourceClass::Light);
        blocked_contract.preconditions = vec!["index_ready".to_string()];
        snapshot.agents.insert(
            "agent:a".to_string(),
            agent("agent:a", AgentState::Stopped, blocked_contract),
        );
        snapshot.agents.insert(
            "agent:b".to_string(),
            agent(
                "agent:b",
                AgentState::Idle,
                capability("search", ResourceClass::Light),
            ),
        );
        let report =
            CapabilityAgent::report(&snapshot, CapabilityReportRequest::default(), 50).unwrap();
        assert_eq!(report.can_do.len(), 1);
        assert_eq!(
            report.can_do[0].candidate_agent_ids,
            vec!["agent:a", "agent:b"]
        );
        assert!(report.can_do[0].reason_codes.is_empty());
    }

    #[test]
    fn unknown_preconditions_and_permissions_are_never_claimed_satisfied() {
        let mut snapshot = snapshot(ResourcePressure::Normal);
        let mut contract = capability("camera.capture", ResourceClass::Moderate);
        contract.preconditions = vec!["camera_present".to_string()];
        contract.permissions = vec!["camera_read".to_string()];
        snapshot.agents.insert(
            "agent:camera".to_string(),
            agent("agent:camera", AgentState::Running, contract),
        );
        let report =
            CapabilityAgent::report(&snapshot, CapabilityReportRequest::default(), 50).unwrap();
        assert!(report.can_do.is_empty());
        let assessment = &report.temporarily_cannot[0];
        assert_eq!(
            assessment.reason_codes,
            vec![
                "permission_must_be_authorized",
                "precondition_must_be_verified"
            ]
        );
        assert_eq!(
            assessment.executors[0]
                .requirements
                .iter()
                .filter(|requirement| requirement.status == RequirementStatus::MustVerify)
                .count(),
            2
        );
    }

    #[test]
    fn report_is_deterministic_and_has_no_session_projection() {
        let mut snapshot = snapshot(ResourcePressure::Normal);
        for id in ["z.last", "a.first"] {
            snapshot.agents.insert(
                format!("agent:{id}"),
                agent(
                    &format!("agent:{id}"),
                    AgentState::Running,
                    capability(id, ResourceClass::Light),
                ),
            );
        }
        let report =
            CapabilityAgent::report(&snapshot, CapabilityReportRequest::default(), 50).unwrap();
        assert_eq!(
            report
                .can_do
                .iter()
                .map(|item| item.capability_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a.first", "z.last"]
        );
        let encoded = serde_json::to_string(&report).unwrap().to_ascii_lowercase();
        assert!(!encoded.contains("session"));
    }

    #[test]
    fn contract_is_side_effect_free_and_idempotent() {
        let contract = capability_agent_capabilities().remove(0);
        assert_eq!(contract.capability_id, CAPABILITY_REPORT_CAPABILITY_ID);
        assert!(contract.side_effects.is_empty());
        assert!(contract.idempotent);
    }
}
