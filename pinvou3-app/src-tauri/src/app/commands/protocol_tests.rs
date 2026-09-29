fn command_names(source: &str) -> Vec<&str> {
    let mut commands = Vec::new();
    let mut command_attribute_seen = false;
    for line in source.lines() {
        let line = line.trim();
        if line.starts_with("async_command_passthrough!(")
            || line.starts_with("sync_command_passthrough!(")
        {
            let name = line
                .split_once(',')
                .expect("passthrough domain")
                .1
                .trim()
                .split('(')
                .next()
                .expect("passthrough command name");
            commands.push(name);
            continue;
        }
        if line.starts_with("#[tauri::command") {
            command_attribute_seen = true;
            continue;
        }
        if !command_attribute_seen {
            continue;
        }
        if let Some((_, suffix)) = line.split_once("fn ") {
            commands.push(
                suffix
                    .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                    .next()
                    .expect("command function name"),
            );
            command_attribute_seen = false;
        }
    }
    commands
}

macro_rules! command_protocol {
    ($test_name:ident, $source:literal, [$($command:literal),* $(,)?]) => {
        #[test]
        fn $test_name() {
            let expected: &[&str] = &[$($command),*];
            assert_eq!(command_names(include_str!($source)).as_slice(), expected);
        }
    };
}

command_protocol!(
    artifacts_protocol,
    "artifacts.rs",
    [
        "read_artifact_text",
        "write_artifact_text",
        "list_deliverable_index",
        "artifact_info",
        "read_artifact_image_b64",
        "read_artifact_thumbnail",
        "render_artifact_visual",
        "open_external_url",
        "open_user_external_url",
        "detect_obsidian",
        "open_in_system",
        "open_containing_folder",
        "reveal_session_folder",
        "open_scheduled_task_folder",
        "open_artifact_window"
    ]
);
command_protocol!(attachments_protocol, "attachments.rs", []);
command_protocol!(
    assistant_response_protocol,
    "assistant_response.rs",
    ["export_assistant_response", "open_assistant_share_target"]
);
command_protocol!(
    chat_protocol,
    "chat.rs",
    ["chat", "steer_chat", "withdraw_steer"]
);
command_protocol!(
    computer_use_protocol,
    "computer_use.rs",
    [
        "computer_use_get_status",
        "computer_use_grant",
        "computer_use_revoke",
        "computer_use_stop",
        "computer_use_confirm",
        "computer_use_deny",
        "computer_use_set_enabled",
        "computer_use_request_permissions"
    ]
);
command_protocol!(
    connectors_protocol,
    "connectors.rs",
    [
        "set_disabled_connectors",
        "get_disabled_connectors",
        "set_bundle_visibility",
        "get_bundle_visibility",
        "set_project_skills_enabled",
        "get_project_skills_enabled",
        "refresh_connector_auth_gates",
        "feishu_ensure_cli",
        "feishu_connect_begin",
        "feishu_cancel",
        "feishu_logout",
        "feishu_apply_skills",
        "feishu_skills_state",
        "wecom_ensure_cli",
        "wecom_connect_begin",
        "wecom_cancel",
        "wecom_logout",
        "wecom_apply_skills",
        "wecom_skills_state",
        "dingtalk_ensure_cli",
        "dingtalk_connect_begin",
        "dingtalk_cancel",
        "dingtalk_logout",
        "dingtalk_apply_skills",
        "dingtalk_skills_state",
        "tmeet_ensure_cli",
        "tmeet_connect_begin",
        "tmeet_cancel",
        "tmeet_logout",
        "tmeet_apply_skills",
        "tmeet_skills_state",
        "ima_connect",
        "ima_logout"
    ]
);
command_protocol!(
    dependencies_protocol,
    "dependencies.rs",
    ["check_dependencies", "install_dependencies"]
);
command_protocol!(
    files_protocol,
    "files.rs",
    [
        "ingest_file",
        "ingest_draft_file_chunk",
        "cancel_draft_file_upload",
        "adopt_draft_attachment",
        "discard_dropped_attachment",
        "resolve_conversation_attachment",
        "open_conversation_attachment",
        "reveal_conversation_attachment",
        "save_paste_image",
        "paste_clipboard_image",
    ]
);
command_protocol!(
    interaction_protocol,
    "interaction.rs",
    [
        "compact_now",
        "get_mode_state",
        "get_code_permission_prefs",
        "confirm_code_yolo",
        "get_mode_defaults",
        "set_mode_default",
        "set_plan_mode_next",
        "exit_plan_to_yolo",
        "set_multi_agent_mode",
        "accept_plan",
        "get_super_permission_status",
        "set_super_permission",
        "discard_plan",
        "submit_user_input",
        "cancel_user_input",
        "get_pending_user_inputs",
        "summon_pinvou"
    ]
);
command_protocol!(
    knowledge_protocol,
    "knowledge.rs",
    [
        "session_mount_collection",
        "session_add_mounted_collection",
        "session_set_mounted_collection_enabled",
        "session_remove_mounted_collection",
        "session_unmount_collection",
        "session_mounted_collection",
        "session_mounted_collections",
        "session_mounted_collections_snapshot",
        "kb_start_scan",
        "kb_scan_status",
        "kb_type_counts",
        "kb_collection_list",
        "kb_collection_create",
        "kb_collection_update",
        "kb_collection_delete",
        "kb_collection_add_sources",
        "kb_index_status",
        "kb_index_cancel",
        "kb_index_failed_files",
        "kb_index_resume",
        "kb_index_retry_file",
        "kb_documents",
        "kb_remove_document",
        "kb_embed_info",
        "kb_search",
        "kb_stats",
        "kb_model_status",
        "kb_model_load_after_first_frame",
        "kb_model_download"
    ]
);

#[test]
fn preinstalled_model_startup_commands_are_not_registered() {
    let app_entrypoint = include_str!("../../lib.rs");
    for removed in [
        "detect_local_vllm_setup",
        "bootstrap_local_vllm",
        "decline_local_vllm_setup",
    ] {
        assert!(
            !app_entrypoint.contains(removed),
            "removed preinstalled-model startup command is still registered: {removed}"
        );
    }
}
command_protocol!(
    marketplace_protocol,
    "marketplace.rs",
    [
        "list_marketplace_tools",
        "install_marketplace_tool",
        "get_marketplace_tool_auth_status",
        "start_marketplace_tool_oauth_login",
        "cancel_marketplace_tool_oauth_login",
        "uninstall_marketplace_tool",
        "list_marketplace_skills",
        "install_marketplace_skill",
        "update_marketplace_skill",
        "update_bundle_display_meta",
        "import_plugin_package_cmd",
        "import_plugin_package_bytes_cmd",
        "import_skill_md_bytes",
        "uninstall_marketplace_skill",
        "list_recycled_plugins",
        "restore_recycled_plugin",
        "purge_recycled_plugin",
        "export_recycled_plugin",
        "export_installed_plugin",
        "bundle_readiness",
        "export_plugin_spec"
    ]
);
command_protocol!(
    memory_protocol,
    "memory.rs",
    [
        "update_memory_profile",
        "get_memory_overview",
        "organize_memory",
        "get_memory_organize_history",
        "confirm_pending_memory",
        "ignore_pending_memory",
        "never_pending_memory",
        "delete_memory_preference",
        "update_memory_preference",
        "update_work_context_memory",
        "delete_work_context_memory",
        "update_timed_memory",
        "delete_timed_memory",
        "edit_last_turn"
    ]
);
command_protocol!(
    monitor_protocol,
    "monitor.rs",
    [
        "get_monitor_snapshot",
        "discover_local_vllm",
        "get_backend_status"
    ]
);
command_protocol!(
    pet_protocol,
    "pet.rs",
    [
        "begin_detach_drag",
        "set_pet_enabled",
        "get_pet_scale",
        "set_pet_scale",
        "set_pet_activity_visible",
        "save_pet_position",
        "save_pet_vertical_alignment",
        "open_main_from_pet",
        "take_pet_navigation",
        "queue_pet_reply",
        "take_pet_reply",
        "get_selected_pet",
        "set_selected_pet"
    ]
);
command_protocol!(
    personas_protocol,
    "personas.rs",
    [
        "list_personas",
        "read_persona_body",
        "equip_persona",
        "create_persona",
        "update_persona",
        "delete_persona",
        "save_session_persona_events",
        "get_session_persona_events",
        "save_session_pinvou_reviews",
        "get_session_pinvou_reviews",
        "unequip_persona",
        "get_active_persona"
    ]
);
command_protocol!(
    projects_protocol,
    "projects.rs",
    [
        "list_projects",
        "create_project",
        "update_project",
        "delete_project",
        "move_session_to_project",
        "rebind_workspace_root"
    ]
);
command_protocol!(
    runtime_protocol,
    "runtime.rs",
    [
        "cancel_generation",
        "get_platform_capabilities",
        "list_shell_tasks",
        "cancel_shell_task"
    ]
);
command_protocol!(
    remote_control_protocol,
    "remote_control.rs",
    [
        "web_access_enable",
        "web_access_disable",
        "web_access_status",
        "web_access_rotate",
        "web_access_relay_settings",
        "web_access_set_relay",
        "web_access_bridge_ready",
        "web_access_rpc_begin",
        "web_access_rpc_respond",
        "web_access_publish_event",
        "web_access_list_host_files",
        "web_access_list_sessions",
        "web_access_list_archived_sessions",
        "web_access_create_session",
        "web_access_load_session_chunk",
        "web_access_cancel_session_download",
        "web_access_ingest_file",
        "web_access_upload_attachment_chunk",
        "web_access_abort_attachment_upload",
        "web_access_discard_attachment",
        "web_access_read_conversation_attachment_chunk",
        "web_access_create_session_and_chat",
        "web_access_chat",
        "web_access_create_codex_acp_session",
        "web_access_list_codex_workspace",
        "web_access_search_codex_workspace",
        "web_access_preview_codex_workspace_file",
        "web_access_get_codex_workspace_changes",
        "web_access_get_codex_workspace_diff",
        "web_access_cancel_codex_acp",
        "web_access_codex_acp_prompt",
        "web_access_get_codex_acp_timeline",
        "web_access_get_codex_acp_session_info",
        "web_access_set_codex_acp_model",
        "web_access_set_codex_acp_mode",
        "web_access_set_codex_acp_config_option",
        "web_access_get_codex_acp_pending_permissions",
        "web_access_respond_codex_acp_permission",
        "web_access_get_codex_acp_pending_elicitations",
        "web_access_respond_codex_acp_elicitation",
        "web_access_list_codex_acp_sessions",
        "web_access_list_acp_agents",
        "web_access_get_acp_agent_status",
        "web_access_transcribe_voice_audio",
        "web_access_read_artifact_chunk",
        "web_access_update_settings",
        "web_access_artifact_info",
        "web_access_read_artifact_text",
        "web_access_write_artifact_text",
        "web_access_read_artifact_image_b64",
        "web_access_read_artifact_thumbnail",
        "web_access_render_artifact_visual"
    ]
);
command_protocol!(
    scheduled_protocol,
    "scheduled.rs",
    [
        "list_scheduled_tasks",
        "read_scheduled_task",
        "list_scheduled_task_runs",
        "list_scheduled_runs",
        "create_scheduled_task",
        "update_scheduled_task",
        "pause_scheduled_task",
        "resume_scheduled_task",
        "set_scheduled_task_pinned",
        "delete_scheduled_task",
        "run_scheduled_task_now",
        "mark_scheduled_run_viewed",
        "scheduled_task_chat_prompt"
    ]
);
command_protocol!(
    sessions_protocol,
    "sessions.rs",
    [
        "list_sessions",
        "list_archived_sessions",
        "create_session",
        "get_session_workspace_binding",
        "load_session",
        "delete_session",
        "export_session",
        "rename_session",
        "set_session_pinned",
        "set_session_archived",
        "get_or_create_aux_session",
        "discard_aux_session",
        "reset_aux_session",
        "save_session_artifacts",
        "save_session_pinvou_scene_events",
        "get_session_pinvou_scene_events",
        "save_session_steered_messages",
        "get_session_steered_messages",
        "list_workspace_files"
    ]
);
command_protocol!(
    settings_protocol,
    "settings.rs",
    [
        "get_settings",
        "submit_feedback",
        "get_effective_model_config",
        "list_models",
        "probe_local_server_kind",
        "reveal_model_api_key",
        "save_model",
        "delete_model",
        "set_active_model",
        "set_session_model",
        "get_session_model_id",
        "get_image_input_capability",
        "test_model_connection",
        "test_image_input_capability",
        "update_settings",
        "update_search_settings",
        "save_settings_and_restart",
        "save_search_settings_and_restart"
    ]
);
command_protocol!(timeline_protocol, "timeline.rs", ["get_session_timeline"]);
command_protocol!(
    startup_protocol,
    "startup.rs",
    ["report_frontend_startup", "reveal_startup_window"]
);
command_protocol!(
    updater_protocol,
    "updater.rs",
    [
        "get_app_version",
        "check_for_update",
        "download_update",
        "install_update",
        "restart_app",
        "cancel_download",
        "report_pending_update_result"
    ]
);
command_protocol!(
    voice_protocol,
    "voice.rs",
    [
        "set_voice_shortcut_enabled",
        "set_voice_shortcut_recording",
        "transcribe_voice_audio",
        "postprocess_voice_text",
        "reset_microphone_permission",
        "voice_asr_status",
        "install_voice_asr",
        "cancel_voice_asr"
    ]
);
// 多智能体（会话内主动委派，ADR-0006）。独立入口/台账/审批命令已随收缩
// 退役，只剩子智能体执行记录的只读投影；开关命令在 interaction.rs。
command_protocol!(
    multiagent_protocol,
    "multiagent.rs",
    ["list_subagent_transcripts", "read_subagent_transcript"]
);
// The four files below were previously outside the protocol snapshot; this is
// additive coverage, and the command lists match the current source files.
command_protocol!(
    acp_providers_protocol,
    "acp_providers.rs",
    [
        "list_acp_providers",
        "save_acp_provider",
        "delete_acp_provider",
        "switch_acp_provider",
        "switch_acp_provider_official",
        "uninstall_acp_agent",
        "cancel_acp_agent_install",
        "get_acp_provider_key",
        "logout_acp_agent",
        "export_acp_providers",
        "import_acp_providers",
        "probe_acp_agent_models",
        "set_codex_acp_session_provider"
    ]
);
command_protocol!(
    remote_knowledge_protocol,
    "remote_knowledge.rs",
    [
        "remote_kb_connections",
        "remote_kb_request_join",
        "remote_kb_probe_private_endpoint",
        "remote_kb_request_join_confirmed",
        "remote_kb_connection_identity",
        "remote_kb_pending_joins",
        "remote_kb_refresh_join",
        "remote_kb_cancel_join",
        "remote_kb_create_share",
        "remote_kb_shares",
        "remote_kb_stop_share",
        "remote_kb_join_requests",
        "remote_kb_approve_join_request",
        "remote_kb_reject_join_request",
        "remote_kb_model_status",
        "remote_kb_download_model",
        "remote_kb_devices",
        "remote_kb_update_device",
        "remote_kb_remove_device",
        "remote_kb_permanently_delete_collection",
        "remote_kb_permanently_delete_document",
        "remote_kb_remove_connection",
        "remote_kb_collections",
        "remote_kb_create_collection",
        "remote_kb_delete_collection",
        "remote_kb_restore_collection",
        "remote_kb_documents",
        "remote_kb_document_statuses",
        "remote_kb_discover_folder_files",
        "remote_kb_upload_files",
        "remote_kb_replace_document",
        "remote_kb_delete_document",
        "remote_kb_restore_document",
        "remote_kb_download_document",
        "remote_kb_search",
        "session_mounted_remote_collections",
        "session_add_mounted_remote_collection",
        "session_set_mounted_remote_collection_enabled",
        "session_remove_mounted_remote_collection"
    ]
);
command_protocol!(
    shared_knowledge_host_protocol,
    "shared_knowledge_host.rs",
    [
        "shared_kb_host_status",
        "shared_kb_host_lan_endpoints",
        "shared_kb_discover_nearby",
        "shared_kb_host_install",
        "shared_kb_host_upgrade",
        "shared_kb_host_reconnect",
        "shared_kb_host_set_owner_device",
        "shared_kb_host_remove",
        "shared_kb_host_backup",
        "shared_kb_host_restore"
    ]
);
command_protocol!(
    diagnostics_protocol,
    "diagnostics.rs",
    ["record_authority_sync_diagnostics"]
);

// Round-34 MAJOR-2: the reset's emit-before-error sequencing must be
// EXECUTED, not inferred — the pre-fix bug (event gated behind the
// command's `?`) left web clients buffering a stale aux transcript when
// the create half failed after the delete half committed.
#[test]
fn reset_finish_emits_the_committed_delete_even_when_the_create_half_failed() {
    use crate::app::commands::sessions::reset_aux_session_finish;

    // The exact post-fix failure shape: delete committed (Some), create
    // half failed (Err) — the event fires AND the error still returns.
    let outcome: (Option<String>, Result<(), std::io::Error>) = (
        Some("aux-parent".to_string()),
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the parent session no longer exists",
        )),
    );
    let emitted = std::cell::RefCell::new(Vec::new());
    let result = reset_aux_session_finish(outcome, |aux_id| {
        emitted.borrow_mut().push(aux_id.to_string());
    });
    assert!(
        result.is_err(),
        "the create-half failure must still surface to the command"
    );
    assert_eq!(
        emitted.borrow().clone(),
        vec!["aux-parent".to_string()],
        "the committed delete's event must fire before the error returns"
    );

    // Control: no committed delete, no event, and success passes through.
    let emitted = std::cell::RefCell::new(Vec::new());
    let result: Result<u8, std::io::Error> = reset_aux_session_finish((None, Ok(7u8)), |aux_id| {
        emitted.borrow_mut().push(aux_id.to_string());
    });
    assert_eq!(result.expect("success passes through"), 7);
    assert!(emitted.borrow().is_empty(), "no committed delete, no event");

    // And the event still fires on the success shape (fresh session).
    let emitted = std::cell::RefCell::new(Vec::new());
    let result: Result<u8, std::io::Error> =
        reset_aux_session_finish((Some("aux-parent".to_string()), Ok(7u8)), |aux_id| {
            emitted.borrow_mut().push(aux_id.to_string());
        });
    assert_eq!(result.expect("success passes through"), 7);
    assert_eq!(emitted.borrow().clone(), vec!["aux-parent".to_string()]);
}

// Round-34 minor 17: the four inline aux refusals are command-layer guards
// with no executable driver (State/AppHandle construction is not unit-wide),
// so their presence is pinned by source shape — a deleted guard flips its
// pin red.
#[test]
fn aux_refusal_guards_stay_in_their_commands() {
    let sessions = include_str!("sessions.rs");
    let interaction = include_str!("interaction.rs");
    let projects = include_str!("projects.rs");

    // The discard command must not silently no-op on aux-shaped ids.
    assert!(
        sessions.contains("discard_aux_session: auxiliary conversations do not own an aux session"),
        "discard_aux_session must reject aux-shaped parent ids",
    );
    assert!(
        interaction
            .contains("set_multi_agent_mode: auxiliary conversations do not take multi-agent mode"),
        "set_multi_agent_mode must reject aux ids explicitly",
    );
    assert!(
        sessions.contains(
            "rename_session: auxiliary conversations are managed through their main session"
        ),
        "rename_session must keep its aux refusal",
    );
    assert!(
        sessions.contains(
            "set_session_pinned: auxiliary conversations are managed through their main session"
        ),
        "set_session_pinned must keep its aux refusal",
    );
    assert!(
        sessions.contains(
            "set_session_archived: auxiliary conversations are managed through their main session"
        ),
        "set_session_archived must keep its aux refusal",
    );
    assert!(
        projects.contains("move_session_to_project: auxiliary conversations are managed through their main session"),
        "move_session_to_project must keep its aux refusal",
    );
    // Round-36 minor 3: the newest campaign member (round-34 minor 6).
    let settings = include_str!("settings.rs");
    assert!(
        settings.contains(
            "set_session_model: auxiliary conversations inherit the main session's model"
        ),
        "set_session_model must keep its aux refusal",
    );
    // The sched- send gates go through the alias-defeating predicate, not
    // exact prefix matching (round-34 minor 4; the contains() form was
    // corrected to the exact call shape — a re-spelled prefix would
    // false-pass a substring hunt, round-35 minor 8).
    let pool = include_str!("../../features/assistant/engine_pool.rs");
    assert!(
        pool.matches("crate::features::sessions::is_sched_session_id(session_id)")
            .count()
            >= 2,
        "both sched- send gates must use the case-insensitive predicate",
    );
    assert!(
        !pool.contains("starts_with(\"sched-\")"),
        "no sched- send gate may keep the exact prefix check",
    );
}

// Round-35 MAJOR-2: the aux-cascade wiring inside
// `EnginePool::delete_chat_session` is load-bearing production routing (its
// doc forbids substituting a bare delete) and had NO executing caller — a
// mutation swapping the wrapper for a direct gate call kept every Rust test
// green. Source-shape pin (the same class as the refusal pins above): the
// method body must route through `delete_chat_session_with_aux_cascade`
// and must not call the gate directly.
#[test]
fn delete_chat_session_keeps_its_aux_cascade_wrapper() {
    let pool = include_str!("../../features/assistant/engine_pool.rs");
    let method_start = pool
        .find("pub(crate) async fn delete_chat_session(")
        .expect("delete_chat_session must exist");
    let method_end = pool[method_start..]
        .find("/// Atomically reset a task's auxiliary conversation")
        .expect("the next method doc must exist");
    let body = &pool[method_start..method_start + method_end];
    assert!(
        body.contains("delete_chat_session_with_aux_cascade("),
        "delete_chat_session must route through the aux-cascade wrapper (the gate call inside it is the wrapper's own closure argument — the reviewed mutation removes the wrapper call entirely, which fails this pin)",
    );
}

// Round-36 minors 4+5: the two aux-classification surfaces an independent
// re-enumeration found beyond the 26 guard sites. The deliverables index is a
// cross-session surface, so it must skip aux records by the derived-id rule;
// the ACP classification must never call an aux id ACP — an aux of an ACP
// main inherits the exact ACP model string, so sniffing alone would let a
// raw-metadata scan hand a full-tool ACP agent an aux id outside the
// EnginePool pins.
#[test]
fn aux_stays_out_of_the_deliverables_index_and_acp_classification() {
    let deliverables = include_str!("../../features/deliverables.rs");
    assert!(
        deliverables.contains("crate::features::sessions::is_aux_session_id"),
        "the deliverables index must skip aux records (round-36 minor 4)",
    );
    let acp = include_str!("../../features/codex_acp/mod.rs");
    assert!(
        acp.matches("crate::features::sessions::is_aux_session_id")
            .count()
            >= 2,
        "is_acp_metadata and is_acp must both reject aux ids (round-36 minor 5)",
    );
}
