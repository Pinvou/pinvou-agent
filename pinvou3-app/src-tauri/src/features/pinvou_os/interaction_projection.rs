use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    AgentState, InteractionRun, InteractionRunStatus, MissionStatus, RuntimeSnapshot,
    PINVOU_INTERACTION_SCOPE_ID,
};

pub const A2UI_PROTOCOL_VERSION: &str = "v0.9";
pub const PINVOU_PROJECTION_NAMESPACE: &str = "projection";
pub const PINVOU_RUNTIME_SURFACE_ID: &str = "projection/runtime-overview";
pub const PINVOU_PROJECTION_CATALOG_ID: &str = "urn:pinvou:a2ui:catalog:projection:v1";

/// A2UI messages plus the Runtime sequence they were deterministically derived from.
/// `messages` are processed in order, exactly as required by A2UI v0.9.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PinvouA2uiProjection {
    pub namespace: String,
    pub basis_sequence: u64,
    pub messages: Vec<Value>,
}

pub fn create_runtime_projection(snapshot: &RuntimeSnapshot) -> PinvouA2uiProjection {
    PinvouA2uiProjection {
        namespace: PINVOU_PROJECTION_NAMESPACE.to_string(),
        basis_sequence: snapshot.last_sequence,
        messages: vec![
            json!({
                "version": A2UI_PROTOCOL_VERSION,
                "createSurface": {
                    "surfaceId": PINVOU_RUNTIME_SURFACE_ID,
                    "catalogId": PINVOU_PROJECTION_CATALOG_ID,
                    "sendDataModel": false
                }
            }),
            json!({
                "version": A2UI_PROTOCOL_VERSION,
                "updateComponents": {
                    "surfaceId": PINVOU_RUNTIME_SURFACE_ID,
                    "components": projection_components()
                }
            }),
            data_model_message(snapshot),
        ],
    }
}

pub fn update_runtime_projection(snapshot: &RuntimeSnapshot) -> PinvouA2uiProjection {
    PinvouA2uiProjection {
        namespace: PINVOU_PROJECTION_NAMESPACE.to_string(),
        basis_sequence: snapshot.last_sequence,
        messages: vec![data_model_message(snapshot)],
    }
}

fn projection_components() -> Value {
    json!([
        {
            "id": "root",
            "component": "PinvouCanvas",
            "children": ["identity", "interaction", "health"]
        },
        {
            "id": "identity",
            "component": "PinvouIdentityCard",
            "displayName": { "path": "/identity/displayName" },
            "continuity": { "path": "/identity/continuity" }
        },
        {
            "id": "interaction",
            "component": "PinvouInteractionCard",
            "status": { "path": "/interaction/status" },
            "modality": { "path": "/interaction/modality" },
            "hasInteraction": { "path": "/interaction/hasInteraction" }
        },
        {
            "id": "health",
            "component": "PinvouRuntimeHealth",
            "runningAgents": { "path": "/runtime/runningAgents" },
            "totalAgents": { "path": "/runtime/totalAgents" },
            "activeMissions": { "path": "/runtime/activeMissions" },
            "resourcePressure": { "path": "/runtime/resourcePressure" },
            "connectivity": { "path": "/runtime/connectivity" },
            "inference": { "path": "/runtime/inference" }
        }
    ])
}

fn data_model_message(snapshot: &RuntimeSnapshot) -> Value {
    let latest_interaction = latest_interaction(snapshot);
    let running_agents = snapshot
        .agents
        .values()
        .filter(|agent| agent.observed_state == AgentState::Running)
        .count();
    let active_missions = snapshot
        .missions
        .values()
        .filter(|mission| mission.status == MissionStatus::Active)
        .count();
    json!({
        "version": A2UI_PROTOCOL_VERSION,
        "updateDataModel": {
            "surfaceId": PINVOU_RUNTIME_SURFACE_ID,
            "path": "/",
            "value": {
                "interactionScopeId": PINVOU_INTERACTION_SCOPE_ID,
                "identity": {
                    "displayName": snapshot.identity.as_ref().map(|identity| identity.display_name.as_str()).unwrap_or("Pinvou"),
                    "continuity": snapshot.identity.as_ref().map(|identity| identity.continuity).map(|_| "continuous").unwrap_or("starting")
                },
                "interaction": {
                    "hasInteraction": latest_interaction.is_some(),
                    "status": latest_interaction.map(interaction_status).unwrap_or("idle"),
                    "modality": latest_interaction.map(|run| match run.modality {
                        super::InteractionModality::Voice => "voice",
                        super::InteractionModality::Text => "text",
                        super::InteractionModality::Touch => "touch",
                        super::InteractionModality::System => "system",
                    }).unwrap_or("none")
                },
                "runtime": {
                    "runningAgents": running_agents,
                    "totalAgents": snapshot.agents.len(),
                    "activeMissions": active_missions,
                    "resourcePressure": format!("{:?}", snapshot.resources.pressure).to_lowercase(),
                    "connectivity": format!("{:?}", snapshot.connectivity.status).to_lowercase(),
                    "inference": format!("{:?}", snapshot.inference.status).to_lowercase()
                }
            }
        }
    })
}

fn latest_interaction(snapshot: &RuntimeSnapshot) -> Option<&InteractionRun> {
    snapshot.interaction_runs.values().max_by(|left, right| {
        (left.submitted_at_ms, &left.interaction_run_id)
            .cmp(&(right.submitted_at_ms, &right.interaction_run_id))
    })
}

fn interaction_status(interaction: &InteractionRun) -> &'static str {
    match interaction.status {
        InteractionRunStatus::Submitted => "submitted",
        InteractionRunStatus::Running => "running",
        InteractionRunStatus::Interrupted => "interrupted",
        InteractionRunStatus::Completed => "completed",
        InteractionRunStatus::Cancelled => "cancelled",
        InteractionRunStatus::Failed => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_uses_v09_order_and_contains_no_actions() {
        let projection = create_runtime_projection(&RuntimeSnapshot::default());
        assert_eq!(projection.namespace, "projection");
        assert!(projection.messages[0].get("createSurface").is_some());
        assert!(projection.messages[1].get("updateComponents").is_some());
        assert!(projection.messages[2].get("updateDataModel").is_some());
        assert!(projection
            .messages
            .iter()
            .all(|message| message.get("version") == Some(&json!("v0.9"))));
        let serialized = serde_json::to_string(&projection).unwrap();
        assert!(!serialized.contains("\"action\""));
        assert!(!serialized.contains("Button"));
        assert!(!serialized.contains("front/"));
        assert!(!serialized.contains("system/"));
    }

    #[test]
    fn incremental_projection_only_updates_the_data_model() {
        let projection = update_runtime_projection(&RuntimeSnapshot::default());
        assert_eq!(projection.messages.len(), 1);
        assert!(projection.messages[0].get("updateDataModel").is_some());
    }
}
