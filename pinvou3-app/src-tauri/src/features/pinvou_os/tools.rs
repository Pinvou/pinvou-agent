//! Model-visible PinvouOS tools backed by the in-process Runtime projection.
//!
//! These adapters deliberately avoid a second MCP-side cache: every answer is
//! derived from the same Runtime state used by the UI and system Agents.  A
//! later cross-process MCP facade can proxy these contracts without becoming a
//! second authority.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use deepseek_tui::tools::spec::{ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec};

use super::{
    build_mission_work_graph, AsrContextAgent, AttentionAgent, AttentionAllocationInput,
    AttentionGoal, CapabilityNeed, CapabilityReportRequest, MissionPlanningInput, PinvouOsRuntime,
    RuntimeSnapshot,
};

pub const RUNTIME_STATUS_TOOL_NAME: &str = "pinvou_runtime_status";
pub const CAPABILITY_REPORT_TOOL_NAME: &str = "pinvou_capability_report";
pub const ORCHESTRATOR_PLAN_TOOL_NAME: &str = "pinvou_orchestrator_plan";
pub const ATTENTION_PLAN_TOOL_NAME: &str = "pinvou_attention_plan";
pub const ASR_CONTEXT_STATUS_TOOL_NAME: &str = "pinvou_asr_context_status";

/// Tools are installed only on the PinvouOS Front engine. Native Code and
/// scheduled automation engines must not inherit the private OS control plane.
pub fn pinvou_os_runtime_tools(app: AppHandle) -> Vec<Arc<dyn ToolSpec>> {
    vec![
        Arc::new(RuntimeStatusTool::new(app.clone())),
        Arc::new(CapabilityReportTool::new(app.clone())),
        Arc::new(OrchestratorPlanTool::new(app.clone())),
        Arc::new(AttentionPlanTool::new(app.clone())),
        Arc::new(AsrContextStatusTool::new(app)),
    ]
}

fn runtime(app: &AppHandle) -> Result<tauri::State<'_, PinvouOsRuntime>, ToolError> {
    app.try_state::<PinvouOsRuntime>().ok_or_else(|| {
        ToolError::execution_failed("PinvouOS Runtime is not available in this process")
    })
}

fn json_result(value: Value) -> ToolResult {
    ToolResult::success(value.to_string())
}

fn decode<T: for<'de> Deserialize<'de>>(input: Value) -> Result<T, ToolError> {
    serde_json::from_value(input)
        .map_err(|error| ToolError::invalid_input(format!("invalid PinvouOS tool input: {error}")))
}

fn read_only_capabilities() -> Vec<ToolCapability> {
    vec![ToolCapability::ReadOnly]
}

#[derive(Clone)]
struct RuntimeStatusTool {
    app: AppHandle,
}

impl RuntimeStatusTool {
    fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

#[async_trait]
impl ToolSpec for RuntimeStatusTool {
    fn name(&self) -> &str {
        RUNTIME_STATUS_TOOL_NAME
    }

    fn description(&self) -> &str {
        "Read the bounded authoritative PinvouOS projection: Agent states and contracts, active Mission/Run counts, resource pressure, network reachability, model readiness, and pending control directives. Read-only and safe to call in parallel."
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false})
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        read_only_capabilities()
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _input: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let snapshot = runtime(&self.app)?.snapshot();
        Ok(json_result(runtime_status_payload(&snapshot)))
    }
}

#[derive(Clone)]
struct CapabilityReportTool {
    app: AppHandle,
}

impl CapabilityReportTool {
    fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

#[async_trait]
impl ToolSpec for CapabilityReportTool {
    fn name(&self) -> &str {
        CAPABILITY_REPORT_TOOL_NAME
    }

    fn description(&self) -> &str {
        "Ask Capability Agent what PinvouOS can do now, what is temporarily unavailable, and what is unsupported. Results come only from registered contracts, actual Agent state, requirements, and current resource pressure."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "requestedCapabilityIds": {
                    "type": "array",
                    "items": {"type": "string"},
                    "maxItems": 256
                },
                "includeRegistered": {"type": "boolean", "default": true}
            },
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        read_only_capabilities()
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let request: CapabilityReportRequest = decode(input)?;
        let report = runtime(&self.app)?
            .capability_report(request)
            .map_err(|error| ToolError::execution_failed(error.to_string()))?;
        let value = serde_json::to_value(report)
            .map_err(|error| ToolError::execution_failed(error.to_string()))?;
        Ok(json_result(value))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OrchestratorPlanInput {
    objective: String,
    priority: u8,
    needs: Vec<CapabilityNeed>,
    #[serde(default)]
    evidence_event_ids: Vec<String>,
}

#[derive(Clone)]
struct OrchestratorPlanTool {
    app: AppHandle,
}

impl OrchestratorPlanTool {
    fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

#[async_trait]
impl ToolSpec for OrchestratorPlanTool {
    fn name(&self) -> &str {
        ORCHESTRATOR_PLAN_TOOL_NAME
    }

    fn description(&self) -> &str {
        "Build a deterministic minimal work graph for a complex PinvouOS objective. The tool reads current resource pressure and capability availability from Runtime; callers provide only the objective and required atomic capabilities. It plans but does not start work."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["objective", "priority", "needs"],
            "properties": {
                "objective": {"type": "string", "minLength": 1, "maxLength": 4096},
                "priority": {"type": "integer", "minimum": 0, "maximum": 100},
                "needs": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 64,
                    "items": {
                        "type": "object",
                        "required": ["capabilityId", "resourceClass"],
                        "properties": {
                            "capabilityId": {"type": "string"},
                            "resourceClass": {"enum": ["light", "moderate", "heavy"]},
                            "dependsOnCapabilityIds": {
                                "type": "array",
                                "items": {"type": "string"},
                                "maxItems": 64
                            }
                        },
                        "additionalProperties": false
                    }
                },
                "evidenceEventIds": {
                    "type": "array",
                    "items": {"type": "string"},
                    "maxItems": 128
                }
            },
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        read_only_capabilities()
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let request: OrchestratorPlanInput = decode(input)?;
        if request.needs.len() > 64 {
            return Err(ToolError::invalid_input(
                "orchestrator plan accepts at most 64 capability needs",
            ));
        }
        let runtime = runtime(&self.app)?;
        let snapshot = runtime.snapshot();
        let capability_ids = request
            .needs
            .iter()
            .map(|need| need.capability_id.clone())
            .collect::<Vec<_>>();
        let reports = runtime
            .capability_report(CapabilityReportRequest {
                requested_capability_ids: capability_ids,
                include_registered: false,
            })
            .map_err(|error| ToolError::execution_failed(error.to_string()))?;
        let graph = build_mission_work_graph(&MissionPlanningInput {
            objective: request.objective,
            priority: request.priority,
            resource_pressure: snapshot.resources.pressure,
            needs: request.needs,
            capability_reports: reports
                .can_do
                .into_iter()
                .chain(reports.temporarily_cannot)
                .chain(reports.unsupported)
                .map(|assessment| super::CapabilityAvailability {
                    capability_id: assessment.capability_id,
                    state: assessment.state,
                    candidate_agent_ids: assessment.candidate_agent_ids,
                    reason_codes: assessment.reason_codes,
                })
                .collect(),
            evidence_event_ids: request.evidence_event_ids,
        })
        .map_err(|error| ToolError::invalid_input(error.to_string()))?;
        let value = serde_json::to_value(graph)
            .map_err(|error| ToolError::execution_failed(error.to_string()))?;
        Ok(json_result(value))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AttentionPlanInput {
    max_concurrent: usize,
    total_work_budget_ms: u64,
    #[serde(default)]
    goals: Vec<AttentionGoal>,
}

#[derive(Clone)]
struct AttentionPlanTool {
    app: AppHandle,
}

impl AttentionPlanTool {
    fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

#[async_trait]
impl ToolSpec for AttentionPlanTool {
    fn name(&self) -> &str {
        ATTENTION_PLAN_TOOL_NAME
    }

    fn description(&self) -> &str {
        "Rank concurrent PinvouOS Runs and allocate bounded work and interruption budgets. Current time and Resource pressure are taken from Runtime, so a caller cannot forge a cooler device state. Planning is read-only; Scheduler applies the result."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["maxConcurrent", "totalWorkBudgetMs", "goals"],
            "properties": {
                "maxConcurrent": {"type": "integer", "minimum": 1, "maximum": 64},
                "totalWorkBudgetMs": {"type": "integer", "minimum": 0},
                "goals": {"type": "array", "maxItems": 256}
            },
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        read_only_capabilities()
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let request: AttentionPlanInput = decode(input)?;
        if request.max_concurrent > 64 || request.goals.len() > 256 {
            return Err(ToolError::invalid_input(
                "attention plan exceeds bounded concurrency or goal count",
            ));
        }
        let pressure = runtime(&self.app)?.snapshot().resources.pressure;
        let mut agent = AttentionAgent::default();
        let plan = agent
            .allocate(AttentionAllocationInput {
                now_ms: chrono::Utc::now().timestamp_millis(),
                resource_pressure: pressure,
                max_concurrent: request.max_concurrent,
                total_work_budget_ms: request.total_work_budget_ms,
                goals: request.goals,
            })
            .map_err(|error| ToolError::invalid_input(error.to_string()))?;
        let value = serde_json::to_value(plan)
            .map_err(|error| ToolError::execution_failed(error.to_string()))?;
        Ok(json_result(value))
    }
}

#[derive(Clone)]
struct AsrContextStatusTool {
    app: AppHandle,
}

impl AsrContextStatusTool {
    fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

#[async_trait]
impl ToolSpec for AsrContextStatusTool {
    fn name(&self) -> &str {
        ASR_CONTEXT_STATUS_TOOL_NAME
    }

    fn description(&self) -> &str {
        "Read the current bounded Qwen3-ASR vocabulary context, revision, refresh time, term counts, and term sources. It does not return stored utterances and does not use the legacy Memory projection."
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false})
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        read_only_capabilities()
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _input: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let snapshot = self
            .app
            .try_state::<AsrContextAgent>()
            .and_then(|agent| agent.current_snapshot());
        Ok(json_result(json!({
            "available": snapshot.is_some(),
            "snapshot": snapshot,
            "memorySource": "organized_memory_context_projection"
        })))
    }
}

pub(crate) fn runtime_status_payload(snapshot: &RuntimeSnapshot) -> Value {
    let agents = snapshot
        .agents
        .values()
        .map(|agent| {
            let memory_architecture_status =
                (agent.agent_id == "agent:memory").then_some("runtime_core_ready");
            json!({
                "agentId": agent.agent_id,
                "displayName": agent.display_name,
                "observedState": agent.observed_state,
                "desiredState": agent.desired_state,
                "capabilityIds": agent.capabilities.iter()
                    .map(|capability| capability.capability_id.as_str())
                    .collect::<Vec<_>>(),
                "memoryArchitectureStatus": memory_architecture_status
            })
        })
        .collect::<Vec<_>>();
    let pending_directives = snapshot
        .directives
        .values()
        .filter(|directive| directive.status == super::DirectiveStatus::Pending)
        .count();
    json!({
        "schemaVersion": snapshot.schema_version,
        "projectionSequence": snapshot.last_sequence,
        "identity": snapshot.identity,
        "agents": agents,
        "activeMissionCount": snapshot.missions.values()
            .filter(|mission| mission.status == super::MissionStatus::Active)
            .count(),
        "runningRunCount": snapshot.runs.values()
            .filter(|run| run.status == super::RunStatus::Running)
            .count(),
        "pendingDirectiveCount": pending_directives,
        "resources": snapshot.resources,
        "connectivity": snapshot.connectivity,
        "inference": snapshot.inference,
        "memoryArchitecture": {
            "status": "runtime_core_ready",
            "authority": "docs/architecture/pinvouos-memory-architecture.html",
            "legacyMemoryAgentIsTruthSource": false,
            "runtimeDecisionStreamConnected": true,
            "trustedEvidenceMetadataConnected": true,
            "exactStructuredClaimBindingConnected": true,
            "stableContextProjectionConnected": true,
            "asyncCandidateWorkerConnected": false,
            "frontContextConsumerConnected": false,
            "verifiedTaskOutcomeConnected": false,
            "privacyIngressConnected": false,
            "periodicCheckpointConnected": false,
            "coldArchiveConnected": false,
            "obsidianAdapterConnected": false
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_payload_reports_runtime_memory_core_without_overstating_future_adapters() {
        let payload = runtime_status_payload(&RuntimeSnapshot::default());
        assert_eq!(
            payload["memoryArchitecture"]["status"],
            "runtime_core_ready"
        );
        assert_eq!(
            payload["memoryArchitecture"]["legacyMemoryAgentIsTruthSource"],
            false
        );
        assert_eq!(
            payload["memoryArchitecture"]["stableContextProjectionConnected"],
            true
        );
        assert_eq!(
            payload["memoryArchitecture"]["exactStructuredClaimBindingConnected"],
            true
        );
        assert_eq!(
            payload["memoryArchitecture"]["asyncCandidateWorkerConnected"],
            false
        );
        assert_eq!(
            payload["memoryArchitecture"]["verifiedTaskOutcomeConnected"],
            false
        );
        assert_eq!(
            payload["memoryArchitecture"]["periodicCheckpointConnected"],
            false
        );
        assert_eq!(
            payload["memoryArchitecture"]["obsidianAdapterConnected"],
            false
        );
    }

    #[test]
    fn tool_names_are_atomic_and_stable() {
        assert_eq!(RUNTIME_STATUS_TOOL_NAME, "pinvou_runtime_status");
        assert_eq!(CAPABILITY_REPORT_TOOL_NAME, "pinvou_capability_report");
        assert_eq!(ORCHESTRATOR_PLAN_TOOL_NAME, "pinvou_orchestrator_plan");
        assert_eq!(ATTENTION_PLAN_TOOL_NAME, "pinvou_attention_plan");
        assert_eq!(ASR_CONTEXT_STATUS_TOOL_NAME, "pinvou_asr_context_status");
    }
}
