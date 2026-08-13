use super::prelude::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackBehaviorEventRequest {
    pub event_name: String,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub input_type: Option<String>,
    pub status: Option<String>,
    pub stage: Option<String>,
    pub tool_key: Option<String>,
    pub tool_name: Option<String>,
    pub tool_type: Option<String>,
    pub success: Option<bool>,
    pub scene_l1: Option<String>,
    pub scene_l2: Option<String>,
}

#[tauri::command]
pub fn track_behavior_event(
    request: TrackBehaviorEventRequest,
    app: AppHandle,
) -> Result<(), String> {
    let event_name = match request.event_name.as_str() {
        "task_started" => "task_started",
        "voice_started" => "voice_started",
        "scene_triggered" => "scene_triggered",
        _ => return Err("unsupported behavior event".to_string()),
    };
    let mut event = crate::features::behavior_telemetry::BehaviorEvent::new(event_name);
    if let Some(value) = request.session_id {
        event = event.session(value);
    }
    if let Some(value) = request.turn_id {
        event = event.turn(value);
    }
    if let Some(value) = request.input_type {
        event = event.input_type(value);
    }
    if let Some(value) = request.status {
        event = event.status(value);
    }
    if let Some(value) = request.stage {
        event = event.stage(value);
    }
    if let Some(value) = request.tool_key {
        event.tool_key = Some(value);
    }
    if let Some(value) = request.tool_name {
        event.tool_name = Some(value);
    }
    if let Some(value) = request.tool_type {
        event.tool_type = Some(value);
    }
    if let Some(value) = request.success {
        event = event.success(value);
    }
    if request.scene_l1.is_some() || request.scene_l2.is_some() {
        event = event.scene(
            request.scene_l1.unwrap_or_default(),
            request.scene_l2.unwrap_or_default(),
        );
    }
    crate::features::behavior_telemetry::track(&app, event);
    Ok(())
}
