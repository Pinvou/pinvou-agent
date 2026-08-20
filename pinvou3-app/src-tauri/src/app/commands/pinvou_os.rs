use tauri::State;

use crate::features::pinvou_os::{
    create_runtime_projection, CapabilityAvailability, EventEnvelope, PinvouA2uiProjection,
    PinvouOsRuntime, RuntimeSnapshot,
};

#[tauri::command]
pub fn get_pinvou_os_snapshot(runtime: State<'_, PinvouOsRuntime>) -> RuntimeSnapshot {
    runtime.snapshot()
}

#[tauri::command]
pub fn get_pinvou_os_projection(runtime: State<'_, PinvouOsRuntime>) -> PinvouA2uiProjection {
    create_runtime_projection(&runtime.snapshot())
}

#[tauri::command]
pub fn explain_pinvou_os_capability(
    capability_id: String,
    runtime: State<'_, PinvouOsRuntime>,
) -> CapabilityAvailability {
    runtime.explain_capability(&capability_id)
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
