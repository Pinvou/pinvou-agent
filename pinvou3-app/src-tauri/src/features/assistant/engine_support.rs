//! Cohesive helpers used by the engine event bridge.
//!
//! Keeping notification, scheduled policy, artifact persistence, and turn
//! signal correlation here prevents the already-large event forwarder from
//! accumulating unrelated lifecycle support code.

use std::path::{Path, PathBuf};

use anyhow::Result;
use deepseek_tui::core::events::TurnOutcomeStatus;
use deepseek_tui::core::ops::{Op, UserInputProvenance};
use deepseek_tui::tui::approval::ApprovalMode;
use tauri::{AppHandle, Manager};

use crate::features::sessions::{ScheduledRunProfile, SessionStore};

pub(super) fn maybe_notify_task_completed(
    app: &AppHandle,
    _store: &SessionStore,
    session_id: &str,
    turn_id: Option<String>,
    status: TurnOutcomeStatus,
    error: Option<&str>,
) {
    if status != TurnOutcomeStatus::Completed || error.is_some() {
        return;
    }
    if !crate::platform::notifications::task_completion_enabled() {
        return;
    }
    let key = turn_id.unwrap_or_else(|| chrono::Utc::now().timestamp_millis().to_string());
    let notify_key = format!("{session_id}:{key}");
    if app
        .try_state::<crate::platform::notifications::NotificationState>()
        .map(|state| state.should_notify(notify_key))
        // Windowless composition roots (for example eval_smoke) deliberately
        // omit the desktop notification plugin and its state. Treat that as
        // notifications unavailable instead of calling NotificationExt, which
        // panics when the plugin has not been managed.
        .unwrap_or(false)
    {
        crate::platform::notifications::notify_task_completed(app);
    }
}

pub(super) fn persist_successful_tool_artifact(
    store: &SessionStore,
    session_id: &str,
    workspace: &Path,
    tool_name: &str,
    tool_input: &serde_json::Value,
    output: &str,
) -> Result<Option<PathBuf>> {
    if store.scheduled_profile(session_id).is_none() {
        return Ok(None);
    }
    let mut raw_paths = artifact_paths_from_tool_output(output);
    if is_file_artifact_tool(tool_name, tool_input) {
        raw_paths.extend(artifact_paths_from_value(tool_input));
    }
    raw_paths.sort();
    raw_paths.dedup();
    if raw_paths.is_empty() {
        return Ok(None);
    }
    let patch_tool = is_patch_artifact_tool(tool_name, tool_input);
    let mut persisted = Vec::new();
    for raw_path in raw_paths {
        let resolved =
            crate::platform::path_policy::resolve_artifact_path_in_workspace(&raw_path, workspace);
        // PatchResult.touched_files also reports deletions. They are successful
        // mutations, but no file remains to append to the scheduled artifact list.
        if patch_tool && !Path::new(&resolved).exists() {
            continue;
        }
        let path = crate::platform::path_policy::validate_user_path(&resolved)
            .map_err(|error| anyhow::anyhow!(error))?;
        if !path.is_file() {
            anyhow::bail!("tool artifact is not an existing file: {}", path.display());
        }
        store.append_scheduled_artifact_path(session_id, path.clone())?;
        persisted.push(path);
    }
    Ok(persisted.into_iter().next())
}

fn is_patch_artifact_tool(name: &str, input: &serde_json::Value) -> bool {
    (name.eq_ignore_ascii_case("File")
        && input.get("action").and_then(serde_json::Value::as_str) == Some("patch"))
        || name == "apply_patch"
        || name.ends_with("_apply_patch")
}

fn push_artifact_path(paths: &mut Vec<String>, path: &str) {
    let path = path.trim();
    if !path.is_empty() && !paths.iter().any(|existing| existing == path) {
        paths.push(path.to_string());
    }
}

fn artifact_paths_from_patch(patch: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in patch.lines() {
        if let Some(path) = ["*** Add File:", "*** Update File:", "*** Delete File:"]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
        {
            push_artifact_path(&mut paths, path);
            continue;
        }
        if let Some(path) = line.strip_prefix("+++ ") {
            let path = path.strip_prefix("b/").unwrap_or(path);
            if path != "/dev/null" {
                push_artifact_path(&mut paths, path);
            }
        }
    }
    paths
}

fn artifact_paths_from_changes(value: &serde_json::Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(artifact_paths_from_value)
        .collect()
}

fn artifact_paths_from_tool_output(output: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(output) else {
        return Vec::new();
    };
    let payload = value
        .get("content")
        .and_then(serde_json::Value::as_array)
        .and_then(|content| content.first())
        .and_then(|item| item.get("text"))
        .and_then(serde_json::Value::as_str)
        .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
        .unwrap_or(value);
    artifact_paths_from_value(&payload)
}

fn artifact_paths_from_value(value: &serde_json::Value) -> Vec<String> {
    let mut paths = Vec::new();
    for key in ["abs_path", "path", "file_path", "local_path", "filename"] {
        if let Some(path) = value.get(key).and_then(serde_json::Value::as_str) {
            push_artifact_path(&mut paths, path);
        }
    }
    if let Some(touched) = value
        .get("touched_files")
        .and_then(serde_json::Value::as_array)
    {
        for path in touched.iter().filter_map(serde_json::Value::as_str) {
            push_artifact_path(&mut paths, path);
        }
    }
    paths.extend(artifact_paths_from_changes(value, "replace"));
    paths.extend(artifact_paths_from_changes(value, "changes"));
    if let Some(patch) = value.get("patch").and_then(serde_json::Value::as_str) {
        paths.extend(artifact_paths_from_patch(patch));
    }
    paths.sort();
    paths.dedup();
    paths
}

fn is_file_artifact_tool(name: &str, input: &serde_json::Value) -> bool {
    if name.eq_ignore_ascii_case("File") {
        return input
            .get("action")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|action| matches!(action, "write" | "edit" | "patch"));
    } else {
        ["write_file", "edit_file", "apply_patch"]
            .iter()
            .any(|tool| name == *tool || name.ends_with(&format!("_{tool}")))
    }
}

/// Lifecycle signal emitted by the single authoritative engine event consumer.
/// Scheduled-task execution subscribes to this channel instead of competing for
/// `EngineHandle::rx_event` with the UI forwarder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EngineTurnSignal {
    Started {
        turn_id: String,
    },
    Terminal {
        turn_id: String,
        status: TurnOutcomeStatus,
        error: Option<String>,
    },
    ForwarderStopped {
        error: String,
    },
}

/// Correlates terminal events with the currently running turn. A fatal
/// `Event::Error` is advisory until the engine emits its authoritative
/// `Event::TurnComplete`; the cached error fills a missing terminal error.
#[derive(Debug, Default)]
pub(super) struct TurnCompletionTracker {
    active_turn_id: Option<String>,
    pending_fatal_error: Option<String>,
}

impl TurnCompletionTracker {
    pub(super) fn on_started(&mut self, turn_id: String) -> EngineTurnSignal {
        self.active_turn_id = Some(turn_id.clone());
        self.pending_fatal_error = None;
        EngineTurnSignal::Started { turn_id }
    }

    pub(super) fn on_fatal_error(&mut self, error: String) {
        self.pending_fatal_error = Some(error);
    }

    pub(super) fn on_terminal(
        &mut self,
        status: TurnOutcomeStatus,
        error: Option<String>,
    ) -> Option<EngineTurnSignal> {
        let error = error.or_else(|| self.pending_fatal_error.take());
        self.active_turn_id
            .take()
            .map(|turn_id| EngineTurnSignal::Terminal {
                turn_id,
                status,
                error,
            })
    }
}

/// Apply the persisted automation policy to the normal bridge-built operation.
/// The bridge remains the source of hooks, tool restrictions, and provider
/// details; scheduled execution only overrides fields explicitly owned by the
/// automation record.
pub(super) fn apply_scheduled_turn_policy(
    op: &mut Op,
    profile: &ScheduledRunProfile,
    resolved_route: deepseek_tui::route_runtime::ResolvedRuntimeRoute,
    compaction_config: deepseek_tui::compaction::CompactionConfig,
) -> Result<()> {
    let Op::SendMessage {
        mode,
        route,
        compaction,
        auto_model,
        allow_shell,
        trust_mode,
        auto_approve,
        approval_mode,
        provenance,
        ..
    } = op
    else {
        anyhow::bail!("scheduled turn requires a SendMessage operation");
    };

    *mode = profile.execution_mode().to_app_mode();
    **route = resolved_route;
    **compaction = compaction_config;
    *auto_model = false;
    *allow_shell = profile.allow_shell;
    *trust_mode = profile.trust_mode;
    *auto_approve = profile.auto_approve;
    *approval_mode = if profile.auto_approve {
        ApprovalMode::Auto
    } else {
        ApprovalMode::Never
    };
    *provenance = UserInputProvenance::ExternalUser;
    Ok(())
}

pub(super) fn scheduled_tool_should_auto_approve(
    profile: Option<&ScheduledRunProfile>,
    approval_force_prompt: bool,
) -> bool {
    profile.is_none_or(|profile| profile.auto_approve && !approval_force_prompt)
}
