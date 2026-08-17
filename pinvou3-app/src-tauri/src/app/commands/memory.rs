use super::prelude::*;
use crate::features::assistant::engine_pool::user_display_message;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct MemoryWarning {
    pub code: String,
    pub source: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemorySourceStatus {
    pub available: bool,
    pub code: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryProfileState {
    pub profile: crate::features::memory::MemoryProfile,
    pub runtime: Option<crate::features::memory::RuntimeMemorySnapshot>,
    pub warnings: Vec<MemoryWarning>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryWriteState<T> {
    pub value: T,
    pub runtime: Option<crate::features::memory::RuntimeMemorySnapshot>,
    pub warnings: Vec<MemoryWarning>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryOverviewState {
    pub profile: crate::features::memory::MemoryProfile,
    pub preferences: Vec<crate::features::memory::PreferenceFile>,
    pub work_context: Vec<crate::features::memory::WorkContextFile>,
    pub current_focus: Vec<crate::features::memory::TimedMemoryItem>,
    pub recent_activity: Vec<crate::features::memory::TimedMemoryItem>,
    pub recent_work: Vec<crate::features::memory::RecentWorkItem>,
    pub pending: Vec<crate::features::memory::PendingMemoryItem>,
    pub never: Vec<crate::features::memory::NeverMemoryItem>,
    pub runtime: Option<crate::features::memory::RuntimeMemorySnapshot>,
    pub snapshot_path: String,
    pub warnings: Vec<MemoryWarning>,
    pub sources: BTreeMap<String, MemorySourceStatus>,
}

fn resolve_memory_session_id(session_id: Option<String>, store: &SessionStore) -> Option<String> {
    session_id.or_else(|| store.active_id())
}

fn emit_memory_write_events(
    app: &AppHandle,
    session_id: &str,
    events: &[crate::features::memory::MemoryWriteEvent],
) {
    if events.is_empty() {
        return;
    }
    let _ = app.emit(
        "chat:memory_write",
        serde_json::json!({
            "session_id": session_id,
            "events": events,
        }),
    );
}

fn emit_memory_snapshot(
    app: &AppHandle,
    session_id: &str,
    snapshot: &crate::features::memory::RuntimeMemorySnapshot,
) {
    let _ = app.emit(
        "chat:memory",
        serde_json::json!({
            "session_id": session_id,
            "items": &snapshot.items,
            "runtime_path": &snapshot.runtime_path,
        }),
    );
}

fn refresh_memory_runtime_for_command(
    session_id: Option<String>,
    store: &SessionStore,
    app: &AppHandle,
) -> Result<Option<crate::features::memory::RuntimeMemorySnapshot>, String> {
    match resolve_memory_session_id(session_id, store) {
        Some(sid) => {
            let snapshot = crate::features::memory::runtime_snapshot(&sid)
                .map_err(|e| format!("render runtime memory: {e}"))?;
            emit_memory_snapshot(app, &sid, &snapshot);
            Ok(Some(snapshot))
        }
        None => Ok(None),
    }
}

fn refresh_memory_runtime_best_effort(
    session_id: Option<String>,
    store: &SessionStore,
    app: &AppHandle,
) -> (
    Option<crate::features::memory::RuntimeMemorySnapshot>,
    Vec<MemoryWarning>,
) {
    match refresh_memory_runtime_for_command(session_id, store, app) {
        Ok(runtime) => (runtime, Vec::new()),
        Err(detail) => {
            eprintln!("[memory] {detail}");
            (
                None,
                vec![memory_warning("runtime_refresh_failed", "runtime", detail)],
            )
        }
    }
}

fn memory_warning(code: &str, source: &str, detail: impl Into<String>) -> MemoryWarning {
    MemoryWarning {
        code: code.to_string(),
        source: source.to_string(),
        detail: detail.into(),
    }
}

fn load_memory_source<T: Default>(
    source: &str,
    result: std::io::Result<T>,
    warnings: &mut Vec<MemoryWarning>,
    sources: &mut BTreeMap<String, MemorySourceStatus>,
) -> T {
    match result {
        Ok(value) => {
            sources.insert(
                source.to_string(),
                MemorySourceStatus {
                    available: true,
                    code: None,
                },
            );
            value
        }
        Err(error) => {
            let detail = format!("load {source}: {error}");
            eprintln!("[memory] {detail}");
            let code = "memory_source_unavailable";
            warnings.push(memory_warning(code, source, detail));
            sources.insert(
                source.to_string(),
                MemorySourceStatus {
                    available: false,
                    code: Some(code.to_string()),
                },
            );
            T::default()
        }
    }
}

fn load_topic_memory_source<T: Default>(
    source: &str,
    result: std::io::Result<crate::features::memory::TopicRead<T>>,
    warnings: &mut Vec<MemoryWarning>,
    sources: &mut BTreeMap<String, MemorySourceStatus>,
) -> T {
    match result {
        Ok(read) => {
            let code = read
                .cleanup_warning
                .as_ref()
                .map(|_| "memory_topic_cleanup_required".to_string());
            if let Some(detail) = read.cleanup_warning {
                warnings.push(memory_warning(
                    "memory_topic_cleanup_required",
                    source,
                    detail,
                ));
            }
            sources.insert(
                source.to_string(),
                MemorySourceStatus {
                    available: true,
                    code,
                },
            );
            read.value
        }
        Err(error) => load_memory_source(source, Err(error), warnings, sources),
    }
}

#[tauri::command]
pub async fn get_memory_profile(
    session_id: Option<String>,
    store: State<'_, SessionStore>,
) -> Result<MemoryProfileState, String> {
    let profile =
        crate::features::memory::load_profile().map_err(|e| format!("load profile: {e}"))?;
    let (runtime, warnings) = match resolve_memory_session_id(session_id, &store) {
        Some(sid) => match crate::features::memory::runtime_snapshot(&sid) {
            Ok(snapshot) => (Some(snapshot), Vec::new()),
            Err(error) => (
                None,
                vec![memory_warning(
                    "runtime_refresh_failed",
                    "runtime",
                    format!("render runtime memory: {error}"),
                )],
            ),
        },
        None => (None, Vec::new()),
    };
    Ok(MemoryProfileState {
        profile,
        runtime,
        warnings,
    })
}

#[tauri::command]
pub async fn update_memory_profile(
    patch: crate::features::memory::ProfilePatch,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryProfileState, String> {
    let profile = crate::features::memory::update_profile(patch)
        .map_err(|e| format!("update profile: {e}"))?;
    let (runtime, warnings) = refresh_memory_runtime_best_effort(session_id, &store, &app);
    Ok(MemoryProfileState {
        profile,
        runtime,
        warnings,
    })
}

#[tauri::command]
pub async fn clear_memory_profile(
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryProfileState, String> {
    let profile =
        crate::features::memory::clear_profile().map_err(|e| format!("clear profile: {e}"))?;
    let (runtime, warnings) = refresh_memory_runtime_best_effort(session_id, &store, &app);
    Ok(MemoryProfileState {
        profile,
        runtime,
        warnings,
    })
}

#[tauri::command]
pub async fn get_memory_overview(
    session_id: Option<String>,
    store: State<'_, SessionStore>,
) -> Result<MemoryOverviewState, String> {
    let mut warnings = Vec::new();
    let mut sources = BTreeMap::new();
    let profile = load_memory_source(
        "profile",
        crate::features::memory::load_profile(),
        &mut warnings,
        &mut sources,
    );
    let preferences = load_topic_memory_source(
        "preferences",
        crate::features::memory::list_preferences_with_cleanup(),
        &mut warnings,
        &mut sources,
    );
    let work_context = load_topic_memory_source(
        "work_context",
        crate::features::memory::load_work_context_with_cleanup(),
        &mut warnings,
        &mut sources,
    );
    let current_focus = load_memory_source(
        "current_focus",
        crate::features::memory::load_current_focus(),
        &mut warnings,
        &mut sources,
    );
    let recent_activity = load_memory_source(
        "recent_activity",
        crate::features::memory::load_recent_activity(),
        &mut warnings,
        &mut sources,
    );
    let recent_work = load_memory_source(
        "recent_work",
        crate::features::memory::load_recent_work(),
        &mut warnings,
        &mut sources,
    );
    let pending = load_memory_source(
        "pending",
        crate::features::memory::load_pending_memory(),
        &mut warnings,
        &mut sources,
    );
    let never = load_memory_source(
        "never",
        crate::features::memory::load_never_memory(),
        &mut warnings,
        &mut sources,
    );
    let authoritative_sources_available = sources.values().all(|status| status.available);
    let runtime = match resolve_memory_session_id(session_id, &store) {
        Some(sid) => match crate::features::memory::runtime_snapshot(&sid) {
            Ok(snapshot) => {
                sources.insert(
                    "runtime".to_string(),
                    MemorySourceStatus {
                        available: true,
                        code: None,
                    },
                );
                Some(snapshot)
            }
            Err(error) => {
                let detail = format!("render runtime memory: {error}");
                eprintln!("[memory] {detail}");
                warnings.push(memory_warning("runtime_refresh_failed", "runtime", detail));
                sources.insert(
                    "runtime".to_string(),
                    MemorySourceStatus {
                        available: false,
                        code: Some("runtime_refresh_failed".to_string()),
                    },
                );
                None
            }
        },
        None => {
            sources.insert(
                "runtime".to_string(),
                MemorySourceStatus {
                    available: true,
                    code: None,
                },
            );
            None
        }
    };
    let snapshot_path = if authoritative_sources_available && sources["runtime"].available {
        match crate::features::memory::write_memory_snapshot_document(
            &profile,
            &preferences,
            &work_context,
            &current_focus,
            &recent_activity,
            &recent_work,
            &pending,
            &never,
            runtime.as_ref(),
        ) {
            Ok(path) => {
                sources.insert(
                    "snapshot".to_string(),
                    MemorySourceStatus {
                        available: true,
                        code: None,
                    },
                );
                path.display().to_string()
            }
            Err(error) => {
                let detail = format!("write memory snapshot: {error}");
                eprintln!("[memory] {detail}");
                warnings.push(memory_warning(
                    "snapshot_refresh_failed",
                    "snapshot",
                    detail,
                ));
                sources.insert(
                    "snapshot".to_string(),
                    MemorySourceStatus {
                        available: false,
                        code: Some("snapshot_refresh_failed".to_string()),
                    },
                );
                String::new()
            }
        }
    } else {
        sources.insert(
            "snapshot".to_string(),
            MemorySourceStatus {
                available: false,
                code: Some("snapshot_refresh_deferred".to_string()),
            },
        );
        String::new()
    };
    Ok(MemoryOverviewState {
        profile,
        preferences,
        work_context,
        current_focus,
        recent_activity,
        recent_work,
        pending,
        never,
        runtime,
        snapshot_path,
        warnings,
        sources,
    })
}

#[tauri::command]
pub async fn list_pending_memory() -> Result<Vec<crate::features::memory::PendingMemoryItem>, String>
{
    crate::features::memory::load_pending_memory().map_err(|e| format!("load pending memory: {e}"))
}

#[tauri::command]
pub async fn suggest_memory(
    suggestion: crate::features::memory::MemorySuggestion,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<crate::features::memory::PendingMemoryItem>, String> {
    let item = crate::features::memory::enqueue_memory_candidate(suggestion)
        .map_err(|e| format!("suggest memory: {e}"))?;
    if let Some(sid) = resolve_memory_session_id(session_id.clone(), &store) {
        emit_memory_write_events(
            &app,
            &sid,
            &[crate::features::memory::MemoryWriteEvent {
                kind: item.kind.clone(),
                action: "pending".to_string(),
                id: item.id.clone(),
                text: item.content.clone(),
            }],
        );
    }
    let (runtime, warnings) = refresh_memory_runtime_best_effort(session_id, &store, &app);
    Ok(MemoryWriteState {
        value: item,
        runtime,
        warnings,
    })
}

#[tauri::command]
pub async fn confirm_pending_memory(
    id: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<Option<crate::features::memory::MemoryWriteEvent>>, String> {
    let event = crate::features::memory::confirm_pending_memory(&id)
        .map_err(|e| format!("confirm pending memory: {e}"))?;
    if let (Some(sid), Some(event)) = (
        resolve_memory_session_id(session_id.clone(), &store),
        event.as_ref(),
    ) {
        emit_memory_write_events(&app, &sid, std::slice::from_ref(event));
    }
    let (runtime, warnings) = refresh_memory_runtime_best_effort(session_id, &store, &app);
    Ok(MemoryWriteState {
        value: event,
        runtime,
        warnings,
    })
}

#[tauri::command]
pub async fn ignore_pending_memory(
    id: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<Option<crate::features::memory::MemoryWriteEvent>>, String> {
    let event = crate::features::memory::ignore_pending_memory(&id)
        .map_err(|e| format!("ignore pending memory: {e}"))?;
    if let (Some(sid), Some(event)) = (
        resolve_memory_session_id(session_id.clone(), &store),
        event.as_ref(),
    ) {
        emit_memory_write_events(&app, &sid, std::slice::from_ref(event));
    }
    let (runtime, warnings) = refresh_memory_runtime_best_effort(session_id, &store, &app);
    Ok(MemoryWriteState {
        value: event,
        runtime,
        warnings,
    })
}

#[tauri::command]
pub async fn never_pending_memory(
    id: String,
    reason: Option<String>,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<Option<crate::features::memory::MemoryWriteEvent>>, String> {
    let event = crate::features::memory::never_pending_memory(&id, reason)
        .map_err(|e| format!("never pending memory: {e}"))?;
    if let (Some(sid), Some(event)) = (
        resolve_memory_session_id(session_id.clone(), &store),
        event.as_ref(),
    ) {
        emit_memory_write_events(&app, &sid, std::slice::from_ref(event));
    }
    let (runtime, warnings) = refresh_memory_runtime_best_effort(session_id, &store, &app);
    Ok(MemoryWriteState {
        value: event,
        runtime,
        warnings,
    })
}

#[tauri::command]
pub async fn list_recent_work_memory(
) -> Result<Vec<crate::features::memory::RecentWorkItem>, String> {
    crate::features::memory::load_recent_work().map_err(|e| format!("load recent work memory: {e}"))
}

#[tauri::command]
pub async fn upsert_recent_work_memory(
    patch: crate::features::memory::RecentWorkPatch,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<crate::features::memory::RecentWorkItem>, String> {
    let item = crate::features::memory::upsert_recent_work(patch)
        .map_err(|e| format!("upsert recent work: {e}"))?;
    if let Some(sid) = resolve_memory_session_id(session_id.clone(), &store) {
        emit_memory_write_events(
            &app,
            &sid,
            &[crate::features::memory::MemoryWriteEvent {
                kind: "recent_work".to_string(),
                action: "remembered".to_string(),
                id: item.id.clone(),
                text: item.title.clone(),
            }],
        );
    }
    let (runtime, warnings) = refresh_memory_runtime_best_effort(session_id, &store, &app);
    Ok(MemoryWriteState {
        value: item,
        runtime,
        warnings,
    })
}

#[tauri::command]
pub async fn archive_recent_work_memory(
    id: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<bool>, String> {
    let changed = crate::features::memory::archive_recent_work(&id)
        .map_err(|e| format!("archive recent work: {e}"))?;
    if changed {
        if let Some(sid) = resolve_memory_session_id(session_id.clone(), &store) {
            emit_memory_write_events(
                &app,
                &sid,
                &[crate::features::memory::MemoryWriteEvent {
                    kind: "recent_work".to_string(),
                    action: "archived".to_string(),
                    id,
                    text: "近期工作已归档".to_string(),
                }],
            );
        }
    }
    let (runtime, warnings) = refresh_memory_runtime_best_effort(session_id, &store, &app);
    Ok(MemoryWriteState {
        value: changed,
        runtime,
        warnings,
    })
}

#[tauri::command]
pub async fn delete_memory_preference(
    id: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<bool>, String> {
    let changed = crate::features::memory::delete_preference(&id)
        .map_err(|e| format!("delete preference: {e}"))?;
    if changed {
        if let Some(sid) = resolve_memory_session_id(session_id.clone(), &store) {
            emit_memory_write_events(
                &app,
                &sid,
                &[crate::features::memory::MemoryWriteEvent {
                    kind: "preference".to_string(),
                    action: "deleted".to_string(),
                    id,
                    text: "偏好已删除".to_string(),
                }],
            );
        }
    }
    let (runtime, warnings) = refresh_memory_runtime_best_effort(session_id, &store, &app);
    Ok(MemoryWriteState {
        value: changed,
        runtime,
        warnings,
    })
}

#[tauri::command]
pub async fn update_memory_preference(
    id: String,
    patch: crate::features::memory::MemoryTextPatch,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<Option<crate::features::memory::PreferenceFile>>, String> {
    let mutation = crate::features::memory::update_preference(&id, patch)
        .map_err(|e| format!("update preference: {e}"))?;
    let mut persistence_warnings = Vec::new();
    let item = mutation.map(|mutation| {
        if let Some(detail) = mutation.cleanup_warning {
            persistence_warnings.push(memory_warning(
                "memory_topic_cleanup_required",
                "preferences",
                detail,
            ));
        }
        mutation.value
    });
    if let (Some(sid), Some(item)) = (
        resolve_memory_session_id(session_id.clone(), &store),
        item.as_ref(),
    ) {
        emit_memory_write_events(
            &app,
            &sid,
            &[crate::features::memory::MemoryWriteEvent {
                kind: "preference".to_string(),
                action: "remembered".to_string(),
                id: item.id.clone(),
                text: item.text.clone(),
            }],
        );
    }
    let (runtime, mut warnings) = refresh_memory_runtime_best_effort(session_id, &store, &app);
    persistence_warnings.append(&mut warnings);
    Ok(MemoryWriteState {
        value: item,
        runtime,
        warnings: persistence_warnings,
    })
}

#[tauri::command]
pub async fn update_work_context_memory(
    id: String,
    patch: crate::features::memory::MemoryTextPatch,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<Option<crate::features::memory::WorkContextFile>>, String> {
    let mutation = crate::features::memory::update_work_context(&id, patch)
        .map_err(|e| format!("update work context: {e}"))?;
    let mut persistence_warnings = Vec::new();
    let item = mutation.map(|mutation| {
        if let Some(detail) = mutation.cleanup_warning {
            persistence_warnings.push(memory_warning(
                "memory_topic_cleanup_required",
                "work_context",
                detail,
            ));
        }
        mutation.value
    });
    if let (Some(sid), Some(item)) = (
        resolve_memory_session_id(session_id.clone(), &store),
        item.as_ref(),
    ) {
        emit_memory_write_events(
            &app,
            &sid,
            &[crate::features::memory::MemoryWriteEvent {
                kind: "work_context".to_string(),
                action: "remembered".to_string(),
                id: item.id.clone(),
                text: item.text.clone(),
            }],
        );
    }
    let (runtime, mut warnings) = refresh_memory_runtime_best_effort(session_id, &store, &app);
    persistence_warnings.append(&mut warnings);
    Ok(MemoryWriteState {
        value: item,
        runtime,
        warnings: persistence_warnings,
    })
}

#[tauri::command]
pub async fn delete_work_context_memory(
    id: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<bool>, String> {
    let changed = crate::features::memory::delete_work_context(&id)
        .map_err(|e| format!("delete work context: {e}"))?;
    if changed {
        if let Some(sid) = resolve_memory_session_id(session_id.clone(), &store) {
            emit_memory_write_events(
                &app,
                &sid,
                &[crate::features::memory::MemoryWriteEvent {
                    kind: "work_context".to_string(),
                    action: "deleted".to_string(),
                    id,
                    text: "工作背景已删除".to_string(),
                }],
            );
        }
    }
    let (runtime, warnings) = refresh_memory_runtime_best_effort(session_id, &store, &app);
    Ok(MemoryWriteState {
        value: changed,
        runtime,
        warnings,
    })
}

#[tauri::command]
pub async fn update_timed_memory(
    kind: String,
    id: String,
    patch: crate::features::memory::MemoryTextPatch,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<Option<crate::features::memory::TimedMemoryItem>>, String> {
    let item = crate::features::memory::update_timed_memory(&kind, &id, patch)
        .map_err(|e| format!("update timed memory: {e}"))?;
    if let (Some(sid), Some(item)) = (
        resolve_memory_session_id(session_id.clone(), &store),
        item.as_ref(),
    ) {
        emit_memory_write_events(
            &app,
            &sid,
            &[crate::features::memory::MemoryWriteEvent {
                kind: item.kind.clone(),
                action: "remembered".to_string(),
                id: item.id.clone(),
                text: item.text.clone(),
            }],
        );
    }
    let (runtime, warnings) = refresh_memory_runtime_best_effort(session_id, &store, &app);
    Ok(MemoryWriteState {
        value: item,
        runtime,
        warnings,
    })
}

#[tauri::command]
pub async fn delete_timed_memory(
    kind: String,
    id: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<bool>, String> {
    let changed = crate::features::memory::delete_timed_memory(&kind, &id)
        .map_err(|e| format!("delete timed memory: {e}"))?;
    if changed {
        if let Some(sid) = resolve_memory_session_id(session_id.clone(), &store) {
            emit_memory_write_events(
                &app,
                &sid,
                &[crate::features::memory::MemoryWriteEvent {
                    kind,
                    action: "deleted".to_string(),
                    id,
                    text: "记忆已删除".to_string(),
                }],
            );
        }
    }
    let (runtime, warnings) = refresh_memory_runtime_best_effort(session_id, &store, &app);
    Ok(MemoryWriteState {
        value: changed,
        runtime,
        warnings,
    })
}

/// 编辑/重发最后一轮 user 消息。
/// engine 砍掉 session 末尾最近的 user+assistant 后，用 new_message 重发。
/// 前端在调这个命令之前必须自己更新 state.messages（删最后一对，加新 user）。
#[tauri::command]
pub async fn edit_last_turn(
    new_message: String,
    session_id: Option<String>,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    if new_message.trim().is_empty() {
        return Err("empty new_message".into());
    }
    let sid = require_active_sid(session_id, &store)?;
    let reservation = pool
        .reserve_turn(&sid)
        .map_err(|e| format!("reserve edit_last_turn: {e:#}"))?;
    let mode_state = store.mode_state(&sid);
    let full = super::multiagent::prepend_delegation_replay_reminder(
        pool.inner(),
        &sid,
        mode_state.multi_agent,
        new_message.clone(),
    );
    let display_message = user_display_message(new_message);

    // 定时会话不走 ensure_chat_session:编辑重发与继续追问同路,EnginePool 内部
    // 按 scheduled_profile 做 turn gate;会话管理类命令(删除/改名/归档)仍然拒绝。
    pool.edit_last_turn_reserved(&sid, full, display_message, reservation)
        .await
        .map_err(|e| format!("edit_last_turn: {e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_source_status_distinguishes_empty_from_unavailable() {
        let mut warnings = Vec::new();
        let mut sources = BTreeMap::new();
        let empty: Vec<String> =
            load_memory_source("preferences", Ok(Vec::new()), &mut warnings, &mut sources);
        assert!(empty.is_empty());
        assert!(warnings.is_empty());
        assert!(sources["preferences"].available);

        let unavailable: Vec<String> = load_memory_source(
            "pending",
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "locked",
            )),
            &mut warnings,
            &mut sources,
        );
        assert!(unavailable.is_empty());
        assert!(!sources["pending"].available);
        assert_eq!(
            sources["pending"].code.as_deref(),
            Some("memory_source_unavailable")
        );
        assert_eq!(warnings[0].code, "memory_source_unavailable");
        assert_eq!(warnings[0].source, "pending");

        let pending: Vec<String> = load_topic_memory_source(
            "preferences",
            Ok(crate::features::memory::TopicRead {
                value: vec!["new".to_string()],
                cleanup_warning: Some("old topic file is occupied".to_string()),
            }),
            &mut warnings,
            &mut sources,
        );
        assert_eq!(pending, ["new"]);
        assert!(sources["preferences"].available);
        assert_eq!(
            sources["preferences"].code.as_deref(),
            Some("memory_topic_cleanup_required")
        );
        assert!(warnings.iter().any(|warning| {
            warning.code == "memory_topic_cleanup_required" && warning.source == "preferences"
        }));
    }

    #[test]
    fn warnings_are_serialized_as_stable_codes() {
        let warning = memory_warning(
            "runtime_refresh_failed",
            "runtime",
            "runtime cache is occupied",
        );
        let value = serde_json::to_value(warning).unwrap();
        assert_eq!(value["code"], "runtime_refresh_failed");
        assert_eq!(value["source"], "runtime");
        assert!(value["detail"].as_str().unwrap().contains("occupied"));

        let cleanup = serde_json::to_value(memory_warning(
            "memory_topic_cleanup_required",
            "preferences",
            "old topic file is occupied",
        ))
        .unwrap();
        assert_eq!(cleanup["code"], "memory_topic_cleanup_required");
        assert_eq!(cleanup["source"], "preferences");
    }
}
