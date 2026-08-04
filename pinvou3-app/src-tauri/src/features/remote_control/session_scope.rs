use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::features::sessions::SessionStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebSessionScope {
    Required(&'static str),
    Optional(&'static str),
}

fn web_session_scope(command: &str) -> Option<WebSessionScope> {
    use WebSessionScope::{Optional, Required};
    let scope = match command {
        // Commands whose Rust API historically falls back to the desktop
        // process-wide active Session must be explicit over WebUI.
        "add_run_materials"
        | "approve_workflow_gate"
        | "archive_recent_work_memory"
        | "cancel_generation"
        | "cancel_user_input"
        | "compact_now"
        | "confirm_pending_memory"
        | "delete_memory_preference"
        | "delete_timed_memory"
        | "delete_work_context_memory"
        | "edit_last_turn"
        | "get_memory_overview"
        | "ignore_pending_memory"
        | "kick_workflow"
        | "never_pending_memory"
        | "reject_workflow_gate"
        | "retry_workflow_role"
        | "stop_workflow"
        | "submit_user_input"
        | "summon_pinvou"
        | "update_memory_profile"
        | "update_memory_preference"
        | "update_timed_memory"
        | "update_work_context_memory" => Required("sessionId"),

        "accept_plan"
        | "cancel_codex_acp"
        | "cancel_shell_task"
        | "discard_plan"
        | "equip_persona"
        | "exit_plan_to_yolo"
        | "get_active_persona"
        | "get_mode_state"
        | "get_codex_acp_pending_elicitations"
        | "get_codex_acp_pending_permissions"
        | "get_codex_acp_session_info"
        | "get_codex_workspace_changes"
        | "get_codex_workspace_diff"
        | "get_session_model_id"
        | "get_session_persona_events"
        | "get_session_pinvou_reviews"
        | "get_session_pinvou_scene_events"
        | "get_session_timeline"
        | "get_workflow_state"
        | "list_shell_tasks"
        | "list_workspace_files"
        | "save_session_persona_events"
        | "save_session_pinvou_reviews"
        | "save_session_pinvou_scene_events"
        | "respond_codex_acp_elicitation"
        | "respond_codex_acp_permission"
        | "session_mount_collection"
        | "session_add_mounted_collection"
        | "session_mounted_collection"
        | "session_mounted_collections"
        | "session_mounted_collections_snapshot"
        | "session_remove_mounted_collection"
        | "session_set_mounted_collection_enabled"
        | "session_set_mounted_collections"
        | "session_unmount_collection"
        | "set_plan_mode_next"
        | "set_codex_acp_config_option"
        | "set_codex_acp_mode"
        | "set_codex_acp_model"
        | "set_session_model"
        | "unbind_session_skill"
        | "unequip_persona"
        | "web_access_artifact_info"
        | "web_access_chat"
        | "web_access_codex_acp_prompt"
        | "web_access_get_codex_acp_pending_elicitations"
        | "web_access_get_codex_acp_pending_permissions"
        | "web_access_get_codex_acp_session_info"
        | "web_access_get_codex_acp_timeline"
        | "web_access_get_gate_report"
        | "web_access_get_role_logs"
        | "web_access_get_role_outputs"
        | "web_access_get_role_prompt"
        | "web_access_list_deliverables"
        | "web_access_read_artifact_chunk"
        | "web_access_read_artifact_image_b64"
        | "web_access_read_artifact_text"
        | "web_access_read_artifact_thumbnail"
        | "web_access_render_artifact_visual"
        | "web_access_read_conversation_attachment_chunk"
        | "web_access_set_codex_acp_config_option"
        | "web_access_set_codex_acp_mode"
        | "web_access_set_codex_acp_model"
        | "web_access_transcribe_voice_audio"
        | "web_access_write_artifact_text" => Required("sessionId"),

        "delete_session"
        | "rename_session"
        | "save_session_artifacts"
        | "set_session_archived"
        | "set_session_pinned"
        | "web_access_load_session_chunk"
        | "web_access_save_session_messages_chunk" => Required("id"),

        // Omitting these deliberately uses the global/default behavior without
        // consulting the desktop active pointer.
        "get_effective_model_config" | "start_workflow" => Optional("sessionId"),
        _ => return None,
    };
    Some(scope)
}

// The code-session event surface is filtered elsewhere, while these shared
// session RPCs intentionally remain available until code sessions receive a
// dedicated remote read/write policy.
/// Multi-agent Sessions are desktop-only for execution. WebUI may inspect
/// their state, but every entry point that starts or mutates a running turn
/// must be rejected here so alternate UI paths cannot bypass the restriction.
const MULTI_AGENT_WEB_EXECUTION_DENYLIST: &[&str] = &[
    "accept_plan",
    "cancel_generation",
    "cancel_shell_task",
    "cancel_user_input",
    "compact_now",
    "discard_plan",
    "edit_last_turn",
    "exit_plan_to_yolo",
    "submit_user_input",
    "summon_pinvou",
    "web_access_chat",
];

fn validate_multi_agent_session_web_scope(
    app: &AppHandle,
    command: &str,
    session_id: &str,
) -> Result<(), String> {
    if !MULTI_AGENT_WEB_EXECUTION_DENYLIST.contains(&command) {
        return Ok(());
    }
    let is_multi_agent = app
        .state::<SessionStore>()
        .mode_state(session_id)
        .multi_agent;
    if is_multi_agent {
        return Err(format!(
            "{command} is not available for desktop-only multi-agent Sessions over Web"
        ));
    }
    Ok(())
}

pub(super) fn validate_web_rpc_scope(
    app: &AppHandle,
    command: &str,
    args: &Value,
) -> Result<(), String> {
    let Some(scope) = web_session_scope(command) else {
        return Ok(());
    };
    let (field, required) = match scope {
        WebSessionScope::Required(field) => (field, true),
        WebSessionScope::Optional(field) => (field, false),
    };
    let session_id = args
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(session_id) = session_id else {
        return if required {
            Err(format!("{command} requires an explicit {field}"))
        } else {
            Ok(())
        };
    };
    crate::features::sessions::validate_session_id(session_id)
        .map_err(|error| format!("远程控制会话 ID 无效：{error:#}"))?;
    validate_multi_agent_session_web_scope(app, command, session_id)?;
    if (command == "web_access_load_session_chunk"
        && args
            .get("downloadId")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty()))
        || (command == "web_access_save_session_messages_chunk"
            && args.get("offset").and_then(Value::as_u64).unwrap_or(0) > 0)
    {
        // The opaque transfer token is already bound to the validated
        // Session id in RemoteControlManager; avoid re-reading a large Session
        // file for every 256 KiB chunk.
        return Ok(());
    }
    let store = app
        .try_state::<SessionStore>()
        .ok_or_else(|| "Session store is not ready".to_string())?;
    store
        .load(session_id)
        .map_err(|error| format!("远程控制会话 {session_id} 不存在：{error:#}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_active_session_fallbacks_are_not_valid_web_scopes() {
        for command in [
            "cancel_generation",
            "compact_now",
            "edit_last_turn",
            "get_session_pinvou_scene_events",
            "get_session_timeline",
            "kick_workflow",
            "save_session_pinvou_scene_events",
            "stop_workflow",
            "web_access_chat",
            "web_access_codex_acp_prompt",
            "web_access_get_codex_acp_timeline",
            "web_access_get_codex_acp_session_info",
            "web_access_get_codex_acp_pending_permissions",
            "web_access_get_codex_acp_pending_elicitations",
            "web_access_set_codex_acp_model",
            "web_access_set_codex_acp_mode",
            "web_access_set_codex_acp_config_option",
        ] {
            assert_eq!(
                web_session_scope(command),
                Some(WebSessionScope::Required("sessionId")),
                "{command} must require the browser-selected Session"
            );
        }
        assert_eq!(
            web_session_scope("start_workflow"),
            Some(WebSessionScope::Optional("sessionId"))
        );
    }

    #[test]
    fn workflow_sessions_reject_every_existing_web_session_command() {
        let scoped_commands = [
            ("get_session_timeline", "sessionId"),
            ("web_access_load_session_chunk", "id"),
            ("web_access_artifact_info", "sessionId"),
            ("web_access_list_deliverables", "sessionId"),
            ("web_access_read_artifact_chunk", "sessionId"),
            ("web_access_read_artifact_image_b64", "sessionId"),
            ("web_access_read_artifact_text", "sessionId"),
            ("web_access_read_artifact_thumbnail", "sessionId"),
            ("edit_last_turn", "sessionId"),
            ("accept_plan", "sessionId"),
            ("discard_plan", "sessionId"),
            ("set_plan_mode_next", "sessionId"),
            ("submit_user_input", "sessionId"),
            ("cancel_user_input", "sessionId"),
            ("cancel_generation", "sessionId"),
            ("web_access_chat", "sessionId"),
            ("delete_session", "id"),
            ("rename_session", "id"),
            ("set_session_model", "sessionId"),
            ("set_session_archived", "id"),
            ("set_session_pinned", "id"),
            ("save_session_artifacts", "id"),
            ("web_access_save_session_messages_chunk", "id"),
            ("web_access_write_artifact_text", "sessionId"),
            ("web_access_render_artifact_visual", "sessionId"),
        ];

        for (command, field) in scoped_commands {
            assert_eq!(
                web_session_scope(command),
                Some(WebSessionScope::Required(field)),
                "{command} must pass through the central Session scope validator"
            );
        }

        assert_eq!(
            web_session_scope("get_effective_model_config"),
            Some(WebSessionScope::Optional("sessionId"))
        );
    }

    #[test]
    fn multi_agent_web_denylist_blocks_execution_but_not_viewing() {
        for command in [
            "web_access_chat",
            "edit_last_turn",
            "accept_plan",
            "discard_plan",
            "exit_plan_to_yolo",
            "submit_user_input",
            "cancel_generation",
            "cancel_shell_task",
            "cancel_user_input",
            "compact_now",
            "summon_pinvou",
        ] {
            assert!(
                super::MULTI_AGENT_WEB_EXECUTION_DENYLIST.contains(&command),
                "执行入口 {command} 必须在 Web 只读封禁表内（复核 P1）"
            );
        }
        for command in [
            "get_session_timeline",
            "get_mode_state",
            "web_access_load_session_chunk",
        ] {
            assert!(
                !super::MULTI_AGENT_WEB_EXECUTION_DENYLIST.contains(&command),
                "只读查看 {command} 必须放行——Web 只读横幅要能取数"
            );
        }
    }
}
