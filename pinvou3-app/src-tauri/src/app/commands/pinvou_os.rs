use tauri::State;

use crate::features::pinvou_os::{
    CapabilityAvailability, ControlDirective, EventEnvelope, MissionStart, OpenMissionRequest,
    PinvouOsRuntime, RegisterMissionAgentRequest, ResourceDecision, ResourceObservation,
    RuntimeSnapshot,
};

#[tauri::command]
pub fn get_pinvou_os_snapshot(runtime: State<'_, PinvouOsRuntime>) -> RuntimeSnapshot {
    runtime.snapshot()
}

#[tauri::command]
pub fn open_pinvou_os_mission(
    request: OpenMissionRequest,
    runtime: State<'_, PinvouOsRuntime>,
) -> Result<MissionStart, String> {
    runtime
        .open_mission(request)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn register_pinvou_os_mission_agent(
    request: RegisterMissionAgentRequest,
    runtime: State<'_, PinvouOsRuntime>,
) -> Result<crate::features::pinvou_os::AgentManifest, String> {
    runtime
        .register_mission_agent(request)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn explain_pinvou_os_capability(
    capability_id: String,
    runtime: State<'_, PinvouOsRuntime>,
) -> CapabilityAvailability {
    runtime.explain_capability(&capability_id)
}

#[tauri::command]
pub fn report_pinvou_os_resources(
    observation: ResourceObservation,
    runtime: State<'_, PinvouOsRuntime>,
) -> Result<ResourceDecision, String> {
    runtime
        .observe_resources(observation)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn acknowledge_pinvou_os_directive(
    directive_id: String,
    applied: bool,
    detail: String,
    runtime: State<'_, PinvouOsRuntime>,
) -> Result<ControlDirective, String> {
    runtime
        .acknowledge_directive(&directive_id, applied, detail)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn list_pinvou_os_events(
    after_sequence: Option<u64>,
    limit: Option<usize>,
    runtime: State<'_, PinvouOsRuntime>,
) -> Result<Vec<EventEnvelope>, String> {
    runtime
        .list_events(after_sequence, limit.unwrap_or(200))
        .map_err(|error| error.to_string())
}
