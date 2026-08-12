use super::prelude::*;
use crate::features::assistant::engine_pool::user_display_message;

#[derive(Debug, Clone, Serialize)]
pub struct MemoryProfileState {
    pub profile: crate::features::memory::MemoryProfile,
    pub runtime: Option<crate::features::memory::RuntimeMemorySnapshot>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryWriteState<T> {
    pub value: T,
    pub runtime: Option<crate::features::memory::RuntimeMemorySnapshot>,
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
    pub warnings: Vec<String>,
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
    Vec<String>,
) {
    match refresh_memory_runtime_for_command(session_id, store, app) {
        Ok(runtime) => (runtime, Vec::new()),
        Err(warning) => {
            eprintln!("[memory] {warning}");
            (None, vec![warning])
        }
    }
}

fn keep_memory_source_or_default<T: Default>(
    label: &str,
    result: std::io::Result<T>,
    warnings: &mut Vec<String>,
) -> T {
    match result {
        Ok(value) => value,
        Err(error) => {
            let warning = format!("{label}: {error}");
            eprintln!("[memory] {warning}");
            warnings.push(warning);
            T::default()
        }
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
            Err(error) => (None, vec![format!("render runtime memory: {error}")]),
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
    let profile =
        crate::features::memory::load_profile().map_err(|e| format!("load profile: {e}"))?;
    let mut warnings = Vec::new();
    let preferences = keep_memory_source_or_default(
        "load preferences",
        crate::features::memory::list_preferences(),
        &mut warnings,
    );
    let work_context = keep_memory_source_or_default(
        "load work context",
        crate::features::memory::load_work_context(),
        &mut warnings,
    );
    let current_focus = keep_memory_source_or_default(
        "load current focus",
        crate::features::memory::load_current_focus(),
        &mut warnings,
    );
    let recent_activity = keep_memory_source_or_default(
        "load recent activity",
        crate::features::memory::load_recent_activity(),
        &mut warnings,
    );
    let recent_work = keep_memory_source_or_default(
        "load recent work",
        crate::features::memory::load_recent_work(),
        &mut warnings,
    );
    let pending = keep_memory_source_or_default(
        "load pending memory",
        crate::features::memory::load_pending_memory(),
        &mut warnings,
    );
    let never = keep_memory_source_or_default(
        "load never memory",
        crate::features::memory::load_never_memory(),
        &mut warnings,
    );
    let runtime = match resolve_memory_session_id(session_id, &store) {
        Some(sid) => match crate::features::memory::runtime_snapshot(&sid) {
            Ok(snapshot) => Some(snapshot),
            Err(error) => {
                let warning = format!("render runtime memory: {error}");
                eprintln!("[memory] {warning}");
                warnings.push(warning);
                None
            }
        },
        None => None,
    };
    let snapshot_path = match crate::features::memory::write_memory_snapshot_document(
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
        Ok(path) => path.display().to_string(),
        Err(error) => {
            let warning = format!("write memory snapshot: {error}");
            eprintln!("[memory] {warning}");
            warnings.push(warning);
            String::new()
        }
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
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: item,
        runtime,
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
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: event,
        runtime,
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
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: event,
        runtime,
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
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: event,
        runtime,
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
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: item,
        runtime,
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
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: changed,
        runtime,
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
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: changed,
        runtime,
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
    let item = crate::features::memory::update_preference(&id, patch)
        .map_err(|e| format!("update preference: {e}"))?;
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
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: item,
        runtime,
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
    let item = crate::features::memory::update_work_context(&id, patch)
        .map_err(|e| format!("update work context: {e}"))?;
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
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: item,
        runtime,
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
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: changed,
        runtime,
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
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: item,
        runtime,
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
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: changed,
        runtime,
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
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    let reservation = pool
        .reserve_turn(&sid)
        .map_err(|e| format!("reserve edit_last_turn: {e:#}"))?;
    let mode_state = store.mode_state(&sid);
    let full = super::multiagent::prepend_delegation_reminder(
        pool.inner(),
        &sid,
        mode_state.multi_agent,
        &new_message,
        new_message.clone(),
    );
    let display_message = user_display_message(new_message);
    // 定时会话不走 ensure_chat_session:编辑重发与继续追问同路,EnginePool 内部
    // 按 scheduled_profile 做 turn gate;会话管理类命令(删除/改名/归档)仍然拒绝。
    pool.edit_last_turn_reserved(&sid, full, display_message, reservation)
        .await
        .map_err(|e| format!("edit_last_turn: {e:?}"))
}
