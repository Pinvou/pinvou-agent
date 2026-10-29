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
    pub pending: Vec<crate::features::memory::PendingMemoryItem>,
    pub runtime: Option<crate::features::memory::RuntimeMemorySnapshot>,
    pub snapshot_path: String,
    pub warnings: Vec<MemoryWarning>,
    pub sources: BTreeMap<String, MemorySourceStatus>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryOrganizeState {
    pub report: crate::features::memory::MemoryOrganizeReport,
    pub runtime: Option<crate::features::memory::RuntimeMemorySnapshot>,
    pub warnings: Vec<MemoryWarning>,
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

// No `chat:memory` emission on the command path: the frontend listens only to
// `chat:memory_write` plus the code-native lane whose events originate in
// chat.rs/forwarder.rs. Command callers read the fresh snapshot from the
// command's return value instead.

/// Single home of the runtime-refresh fallback ladder (resolve → snapshot →
/// on failure the shared "render runtime memory" detail string, stderr log,
/// and `runtime_refresh_failed` warning); the write-command wrapper, the
/// snapshot-document lane, and the overview lane all consume it and add only
/// their own bookkeeping.
fn runtime_snapshot_best_effort(
    sid: Option<String>,
) -> (
    Option<crate::features::memory::RuntimeMemorySnapshot>,
    Option<MemoryWarning>,
) {
    match sid.map(|sid| crate::features::memory::runtime_snapshot(&sid)) {
        Some(Ok(snapshot)) => (Some(snapshot), None),
        Some(Err(error)) => {
            let detail = format!("render runtime memory: {error}");
            eprintln!("[memory] {detail}");
            (
                None,
                Some(memory_warning("runtime_refresh_failed", "runtime", detail)),
            )
        }
        None => (None, None),
    }
}

fn refresh_memory_runtime_best_effort(
    session_id: Option<String>,
    store: &SessionStore,
) -> (
    Option<crate::features::memory::RuntimeMemorySnapshot>,
    Vec<MemoryWarning>,
) {
    let (runtime, warning) = runtime_snapshot_best_effort(resolve_session_id(session_id, store));
    (runtime, warning.into_iter().collect())
}

/// Shared tail of every memory write command: emit the write's
/// `chat:memory_write` event (when one applies to the resolved session),
/// refresh the runtime memory snapshot best-effort, and wrap the command
/// value with the resulting runtime and warnings. `persistence_warnings`
/// (store-level cleanup notes) are reported before the refresh's own
/// warnings, matching the historical per-command ordering.
fn finish_memory_write<T>(
    app: &AppHandle,
    session_id: Option<String>,
    store: &SessionStore,
    value: T,
    event: Option<crate::features::memory::MemoryWriteEvent>,
    persistence_warnings: Vec<MemoryWarning>,
) -> Result<MemoryWriteState<T>, String> {
    if let (Some(sid), Some(event)) = (
        resolve_session_id(session_id.clone(), store),
        event.as_ref(),
    ) {
        emit_memory_write_events(app, &sid, std::slice::from_ref(event));
    }
    let (runtime, refresh_warnings) = refresh_memory_runtime_best_effort(session_id, store);
    let mut warnings = persistence_warnings;
    warnings.extend(refresh_warnings);
    Ok(MemoryWriteState {
        value,
        runtime,
        warnings,
    })
}

fn memory_warning(code: &str, source: &str, detail: impl Into<String>) -> MemoryWarning {
    MemoryWarning {
        code: code.to_string(),
        source: source.to_string(),
        detail: detail.into(),
    }
}

/// Loaded results for the eight authoritative memory sources, shared by
/// get_memory_overview and the post-organize snapshot refresh.
struct MemorySources {
    profile: crate::features::memory::MemoryProfile,
    preferences: Vec<crate::features::memory::PreferenceFile>,
    work_context: Vec<crate::features::memory::WorkContextFile>,
    current_focus: Vec<crate::features::memory::TimedMemoryItem>,
    recent_activity: Vec<crate::features::memory::TimedMemoryItem>,
    recent_work: Vec<crate::features::memory::RecentWorkItem>,
    pending: Vec<crate::features::memory::PendingMemoryItem>,
    never: Vec<crate::features::memory::NeverMemoryItem>,
}

/// Loads every authoritative source one by one, recording warnings and
/// per-source status. Shared by get_memory_overview and the post-organize
/// snapshot refresh so the two loading paths cannot drift apart.
fn load_memory_sources() -> (
    MemorySources,
    Vec<MemoryWarning>,
    BTreeMap<String, MemorySourceStatus>,
) {
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
    (
        MemorySources {
            profile,
            preferences,
            work_context,
            current_focus,
            recent_activity,
            recent_work,
            pending,
            never,
        },
        warnings,
        sources,
    )
}

/// Refreshes the `snapshot.md` device snapshot document, reusing the
/// get_memory_overview loading and snapshot-write helpers. Failures are
/// returned as warnings and do not disturb the caller's main flow.
///
/// Same gate as overview: write only when every authoritative source is
/// available — when an individual source fails to read, `load_memory_source`
/// returns an empty default, and writing anyway would wipe that memory
/// category from snapshot.md. `runtime_render_failed` means the caller
/// already attempted a runtime render and it failed (e.g. the organize
/// command's best-effort pre-render); rendering is not retried and no
/// snapshot is written (same policy as overview, where an unavailable runtime
/// defers the refresh).
fn refresh_memory_snapshot_document(
    session_id: Option<String>,
    store: &SessionStore,
    runtime: Option<&crate::features::memory::RuntimeMemorySnapshot>,
    runtime_render_failed: bool,
) -> Vec<MemoryWarning> {
    let (sources, mut warnings, status) = load_memory_sources();
    let authoritative_sources_available = status.values().all(|source| source.available);
    let MemorySources {
        profile,
        preferences,
        work_context,
        current_focus,
        recent_activity,
        recent_work,
        pending,
        never,
    } = sources;
    let mut runtime_available = !runtime_render_failed;
    let runtime = match (runtime, resolve_session_id(session_id, store)) {
        (Some(snapshot), _) => Some(snapshot.clone()),
        (None, Some(sid)) if !runtime_render_failed => {
            let (snapshot, warning) = runtime_snapshot_best_effort(Some(sid));
            if let Some(warning) = warning {
                warnings.push(warning);
                runtime_available = false;
            }
            snapshot
        }
        _ => None,
    };
    if authoritative_sources_available && runtime_available {
        if let Err(error) = crate::features::memory::write_memory_snapshot_document(
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
            let detail = format!("write memory snapshot: {error}");
            eprintln!("[memory] {detail}");
            warnings.push(memory_warning(
                "snapshot_refresh_failed",
                "snapshot",
                detail,
            ));
        }
    } else {
        warnings.push(memory_warning(
            "snapshot_refresh_deferred",
            "snapshot",
            "memory sources unavailable; snapshot refresh deferred",
        ));
    }
    warnings
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
pub async fn update_memory_profile(
    patch: crate::features::memory::ProfilePatch,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
) -> Result<MemoryProfileState, String> {
    let profile = crate::features::memory::update_profile(patch)
        .map_err(|e| format!("update profile: {e}"))?;
    let (runtime, warnings) = refresh_memory_runtime_best_effort(session_id, &store);
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
    let (sources_loaded, mut warnings, mut sources) = load_memory_sources();
    let MemorySources {
        profile,
        preferences,
        work_context,
        current_focus,
        recent_activity,
        recent_work,
        pending,
        never,
    } = sources_loaded;
    let authoritative_sources_available = sources.values().all(|status| status.available);
    let runtime = match resolve_session_id(session_id, &store) {
        Some(sid) => {
            let (snapshot, warning) = runtime_snapshot_best_effort(Some(sid));
            if let Some(warning) = warning {
                warnings.push(warning);
                sources.insert(
                    "runtime".to_string(),
                    MemorySourceStatus {
                        available: false,
                        code: Some("runtime_refresh_failed".to_string()),
                    },
                );
            } else {
                sources.insert(
                    "runtime".to_string(),
                    MemorySourceStatus {
                        available: true,
                        code: None,
                    },
                );
            }
            snapshot
        }
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
                crate::features::memory::display_redacted(&path)
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
        pending,
        runtime,
        snapshot_path,
        warnings,
        sources,
    })
}

/// Organizes and prunes memory: scans all six stores in full, has the LLM
/// produce delete/update/merge actions, and applies them. The frontend should
/// keep the button disabled while memory is off; the `memory_enabled` guard
/// here stays as a backstop.
#[tauri::command]
pub async fn organize_memory(
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<MemoryOrganizeState, String> {
    if !crate::features::memory::memory_enabled() {
        return Err("memory disabled".to_string());
    }
    // Resolved like chat's image routing: a fresh bridge bound to the session's
    // model (including local vLLM served-name probing and runtime credential
    // preparation). Without an active session yet (e.g. a brand-new draft),
    // degrade to the pool's shared bridge with global prefs loaded directly,
    // the same `get_image_input_capability` fallback.
    let bridge = match resolve_session_id(None, &store) {
        Some(sid) => pool.fresh_bridge_for(&sid).await.map_err(|error| {
            // The `fresh_bridge_for` error chain can carry endpoint/credential
            // probing details; pass it through `redact_secret` before returning
            // to the frontend (shared `sanitize_command_error` policy).
            sanitize_command_error(
                &format!("organize memory: resolve bridge for {sid}"),
                format!("{error:#}"),
            )
        })?,
        None => {
            let mut bridge = pool.bridge.clone();
            bridge.prefs = UserPrefs::load();
            bridge.session_model = None;
            bridge
        }
    };
    // The manual button has no cancellable owner, so pass None for cancel (only
    // the scheduled-task entry point passes a token).
    let report = crate::features::memory::organize_memory_with_llm(&bridge, None)
        .await
        .map_err(|error| sanitize_command_error("organize memory", format!("{error:#}")))?;
    let (runtime, mut warnings) = refresh_memory_runtime_best_effort(None, &store);
    // If the best-effort pre-render already failed, do not let the snapshot
    // refresh retry (avoiding a duplicate same-code warning and a second
    // failed render); the snapshot is skipped as deferred.
    let runtime_render_failed = warnings
        .iter()
        .any(|warning| warning.code == "runtime_refresh_failed");
    warnings.extend(refresh_memory_snapshot_document(
        None,
        &store,
        runtime.as_ref(),
        runtime_render_failed,
    ));
    Ok(MemoryOrganizeState {
        report,
        runtime,
        warnings,
    })
}

/// Most recent memory organize reports, newest first; empty when organize has
/// never run.
#[tauri::command]
pub fn get_memory_organize_history() -> Vec<crate::features::memory::MemoryOrganizeReport> {
    crate::features::memory::load_organize_history()
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
    finish_memory_write(&app, session_id, &store, event.clone(), event, Vec::new())
}

#[tauri::command]
pub async fn ignore_pending_memory(
    id: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<Option<crate::features::memory::MemoryWriteEvent>>, String> {
    // The command's contract is Option<event>: the AlreadyDecided/NotFound
    // distinction matters to organize's report wording, not to this caller,
    // which historically answers None for both.
    let event = match crate::features::memory::ignore_pending_memory(&id)
        .map_err(|e| format!("ignore pending memory: {e}"))?
    {
        crate::features::memory::PendingIgnoreOutcome::Ignored(event) => Some(event),
        crate::features::memory::PendingIgnoreOutcome::AlreadyDecided
        | crate::features::memory::PendingIgnoreOutcome::NotFound => None,
    };
    finish_memory_write(&app, session_id, &store, event.clone(), event, Vec::new())
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
    finish_memory_write(&app, session_id, &store, event.clone(), event, Vec::new())
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
    let event = changed.then(|| crate::features::memory::MemoryWriteEvent {
        kind: "preference".to_string(),
        action: "deleted".to_string(),
        id,
        text: "偏好已删除".to_string(),
    });
    finish_memory_write(&app, session_id, &store, changed, event, Vec::new())
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
    let event = item
        .as_ref()
        .map(|item| crate::features::memory::MemoryWriteEvent {
            kind: "preference".to_string(),
            action: "remembered".to_string(),
            id: item.id.clone(),
            text: item.text.clone(),
        });
    finish_memory_write(&app, session_id, &store, item, event, persistence_warnings)
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
    let event = item
        .as_ref()
        .map(|item| crate::features::memory::MemoryWriteEvent {
            kind: "work_context".to_string(),
            action: "remembered".to_string(),
            id: item.id.clone(),
            text: item.text.clone(),
        });
    finish_memory_write(&app, session_id, &store, item, event, persistence_warnings)
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
    let event = changed.then(|| crate::features::memory::MemoryWriteEvent {
        kind: "work_context".to_string(),
        action: "deleted".to_string(),
        id,
        text: "工作背景已删除".to_string(),
    });
    finish_memory_write(&app, session_id, &store, changed, event, Vec::new())
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
    let event = item
        .as_ref()
        .map(|item| crate::features::memory::MemoryWriteEvent {
            kind: item.kind.clone(),
            action: "remembered".to_string(),
            id: item.id.clone(),
            text: item.text.clone(),
        });
    finish_memory_write(&app, session_id, &store, item, event, Vec::new())
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
    let event = changed.then(|| crate::features::memory::MemoryWriteEvent {
        kind,
        action: "deleted".to_string(),
        id,
        text: "记忆已删除".to_string(),
    });
    finish_memory_write(&app, session_id, &store, changed, event, Vec::new())
}

/// `edit_last_turn` 的乐观并发护栏：`expected` 与当前已落盘 transcript 的
/// revision 不一致时拒绝编辑（不动 transcript、不消费 turn）。稳定错误码与
/// store 的 `compare_and_swap_messages`（`session_revision_conflict`）同族；
/// load/compute 失败折叠为 `session_revision_unavailable`，原始错误链（可能
/// 内嵌宿主路径）只进桌面日志——该护栏是 Web 编辑重发跨 Relay 的唯一入口。
fn ensure_transcript_revision_current(
    store: &SessionStore,
    sid: &str,
    expected: &str,
) -> Result<(), String> {
    let saved = store.load(sid).map_err(|error| {
        log::warn!("[edit_last_turn] load Session {sid} for revision guard: {error:#}");
        "session_revision_unavailable".to_string()
    })?;
    let current =
        crate::features::sessions::transcript_revision(&saved.messages).map_err(|error| {
            log::warn!("[edit_last_turn] compute Session {sid} transcript revision: {error:#}");
            "session_revision_unavailable".to_string()
        })?;
    if current != expected {
        log::warn!(
            "[edit_last_turn] Session {sid} transcript moved on: expected {expected}, current {current}"
        );
        return Err("session_revision_conflict".into());
    }
    Ok(())
}

/// 编辑/重发最后一轮 user 消息。
/// engine 砍掉 session 末尾最近的 user+assistant 后，用 new_message 重发。
/// 前端在调这个命令之前必须自己更新 state.messages（删最后一对，加新 user）。
/// `expected_transcript_revision` 是可选的乐观并发护栏：Web 编辑重发传入其
/// 所持快照的 revision，桌面端不传（None = 现行为不变）。护栏在 turn gate
/// 内执行——占住 gate 后其他 turn 无法再提交，check-then-act 之间当前
/// revision 不可能被推进——不匹配即拒绝，防止手机端基于过期视图砍掉桌面端
/// 刚提交的更新一轮。
#[tauri::command]
pub async fn edit_last_turn(
    new_message: String,
    session_id: Option<String>,
    expected_transcript_revision: Option<String>,
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
    if let Some(expected) = expected_transcript_revision.as_deref() {
        // 护栏拒绝时 reservation 在此 Drop：未提交预约自动失效，gate 立即
        // 重开，transcript 保持原样。
        ensure_transcript_revision_current(&store, &sid, expected)?;
    }
    // 编辑重发只发编辑后的原文：`EditLastTurn` 不携带 route，由引擎按当前
    // 配置重推导 provider 路由（含专家 fleet 配置随配置重读）；底座重推导时
    // 会重申宿主已申报的路由限额，不因重推导退回目录猜测。蜂群契约在 spawn
    // 级 instructions，无需（也不应）在此重新拼接任何提醒内容。
    let display_message = user_display_message(new_message.clone());

    // 定时会话不走 ensure_chat_session:编辑重发与继续追问同路,EnginePool 内部
    // 按 scheduled_profile 做 turn gate;会话管理类命令(删除/改名/归档)仍然拒绝。
    crate::features::assistant::timing::start_turn(&sid);
    if let Err(error) = pool
        .edit_last_turn_reserved(&sid, new_message, display_message, reservation)
        .await
    {
        crate::features::assistant::timing::finish_turn(
            &sid,
            "send_error",
            Some(&format!("{error:#}")),
        );
        return Err(format!("edit_last_turn: {error:#}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepseek_tui::models::{ContentBlock, Message};

    fn revision_message(role: &str, text: &str) -> Message {
        Message {
            role: role.into(),
            content: vec![ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
        }
    }

    /// FIX（remote-control audit）: web `edit_last_turn` 的乐观并发护栏。
    /// 过期 revision 必须被拒绝且 transcript 保持原样；当前 revision 放行；
    /// 不存在的会话折叠为稳定码。命令本体在护栏拒绝时 Drop 未提交预约
    /// （gate 重开、transcript 不动），此处直接钉护栏的判定与不改写语义。
    #[test]
    fn edit_last_turn_revision_guard_blocks_stale_views_without_mutating() {
        // Round-6 复审:PINVOU3_HOME 的保存/恢复改走 Drop 兜底的
        // with_temp_home——直线式恢复在断言失败(panic 展开)时被跳过,
        // 环境与临时目录会泄漏给本进程内所有后续测试(该 helper 的文档
        // 明确警告这一点)。
        crate::platform::test_support::with_temp_home("pinvou3-edit-revision-guard-test", || {
            let root = crate::platform::paths::pinvou3_home();
            let store = crate::features::sessions::SessionStore::boot_with_scheduled_root(
                root.join("scheduled"),
            )
            .expect("session store");
            let session = store
                .create_new("model".to_string(), None, root.clone())
                .expect("create session");
            let sid = session.metadata.id;

            // 手机端视图：只有第一轮 user 消息。
            store
                .update_messages(&sid, vec![revision_message("user", "first")])
                .expect("seed stale transcript");
            let stale_revision = crate::features::sessions::transcript_revision(
                &store.load(&sid).expect("load stale").messages,
            )
            .expect("stale revision");

            // 桌面端随后提交了两轮（手机视图未感知）。
            store
                .update_messages(
                    &sid,
                    vec![
                        revision_message("user", "first"),
                        revision_message("assistant", "done"),
                        revision_message("user", "second"),
                    ],
                )
                .expect("advance transcript");
            let current_revision = crate::features::sessions::transcript_revision(
                &store.load(&sid).expect("reload").messages,
            )
            .expect("current revision");
            assert_ne!(stale_revision, current_revision);

            // 过期视图必须被拒绝，且 transcript 不得被改动。
            assert_eq!(
                ensure_transcript_revision_current(&store, &sid, &stale_revision)
                    .expect_err("stale revision must be refused"),
                "session_revision_conflict"
            );
            let after_conflict = store.load(&sid).expect("load after conflict");
            assert_eq!(
                after_conflict.messages.len(),
                3,
                "guard refusal must not mutate the transcript"
            );
            assert_eq!(
                crate::features::sessions::transcript_revision(&after_conflict.messages)
                    .expect("revision after conflict"),
                current_revision
            );

            // 持有当前 revision 的编辑放行（真正的改写仍由引擎在 gate 内执行）。
            assert!(ensure_transcript_revision_current(&store, &sid, &current_revision).is_ok());

            // 不存在的会话折叠为稳定码，宿主路径只进桌面日志。
            assert_eq!(
                ensure_transcript_revision_current(&store, "missing-session", &current_revision)
                    .expect_err("missing session must be refused"),
                "session_revision_unavailable"
            );
        });
    }

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
