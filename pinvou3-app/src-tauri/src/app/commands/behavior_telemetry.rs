use super::prelude::*;

use crate::features::behavior_telemetry::BehaviorEventRequest;

// 命令 wire DTO 与 feature 共用同一声明（BehaviorEventRequest，曾在此处逐字段
// 镜像一份 TrackBehaviorEventRequest）。
type TrackBehaviorEventRequest = BehaviorEventRequest;

#[tauri::command]
pub fn track_behavior_event(
    request: TrackBehaviorEventRequest,
    app: AppHandle,
) -> Result<(), String> {
    let event = request.build_event(&["voice_started", "scene_triggered"])?;
    crate::features::behavior_telemetry::track(&app, event);
    Ok(())
}
