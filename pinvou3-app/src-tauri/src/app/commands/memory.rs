use super::prelude::*;
use crate::features::assistant::engine_pool::user_display_message;

#[derive(Debug, Clone, Serialize)]
pub struct MemoryProfileState {
    pub profile: crate::features::memory::MemoryProfile,
    pub runtime: Option<crate::features::memory::RuntimeMemorySnapshot>,
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

#[tauri::command]
pub async fn get_memory_profile(
    session_id: Option<String>,
    store: State<'_, SessionStore>,
) -> Result<MemoryProfileState, String> {
    let profile =
        crate::features::memory::load_profile().map_err(|e| format!("load profile: {e}"))?;
    let runtime = match resolve_memory_session_id(session_id, &store) {
        Some(sid) => Some(
            crate::features::memory::runtime_snapshot(&sid)
                .map_err(|e| format!("render runtime memory: {e}"))?,
        ),
        None => None,
    };
    Ok(MemoryProfileState { profile, runtime })
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
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryProfileState { profile, runtime })
}

#[tauri::command]
pub async fn clear_memory_profile(
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryProfileState, String> {
    let profile =
        crate::features::memory::clear_profile().map_err(|e| format!("clear profile: {e}"))?;
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryProfileState { profile, runtime })
}

#[tauri::command]
pub async fn get_memory_overview(
    session_id: Option<String>,
    store: State<'_, SessionStore>,
) -> Result<MemoryOverviewState, String> {
    let profile =
        crate::features::memory::load_profile().map_err(|e| format!("load profile: {e}"))?;
    let preferences = crate::features::memory::list_preferences()
        .map_err(|e| format!("load preferences: {e}"))?;
    let work_context = crate::features::memory::load_work_context()
        .map_err(|e| format!("load work context: {e}"))?;
    let current_focus = crate::features::memory::load_current_focus()
        .map_err(|e| format!("load current focus: {e}"))?;
    let recent_activity = crate::features::memory::load_recent_activity()
        .map_err(|e| format!("load recent activity: {e}"))?;
    let recent_work = crate::features::memory::load_recent_work()
        .map_err(|e| format!("load recent work: {e}"))?;
    let pending = crate::features::memory::load_pending_memory()
        .map_err(|e| format!("load pending memory: {e}"))?;
    let never = crate::features::memory::load_never_memory()
        .map_err(|e| format!("load never memory: {e}"))?;
    let runtime = match resolve_memory_session_id(session_id, &store) {
        Some(sid) => Some(
            crate::features::memory::runtime_snapshot(&sid)
                .map_err(|e| format!("render runtime memory: {e}"))?,
        ),
        None => None,
    };
    let snapshot_path = crate::features::memory::write_memory_snapshot_document(
        &profile,
        &preferences,
        &work_context,
        &current_focus,
        &recent_activity,
        &recent_work,
        &pending,
        &never,
        runtime.as_ref(),
    )
    .map_err(|e| format!("write memory snapshot: {e}"))?
    .display()
    .to_string();
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
