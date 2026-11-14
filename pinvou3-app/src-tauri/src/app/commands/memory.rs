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
            // Round-8 review: warning/detail text leaves with success payloads (the
            // commands are all within the web access policy), so it uniformly goes
            // through the shared redact_storage_prefix to fold the storage-root
            // prefix (platform/paths.rs documented caliber; io.rs display_redacted is
            // the same implementation). The other three error texts in this file were
            // hardened in the same batch.
            let detail = crate::platform::paths::redact_storage_prefix(&format!(
                "render runtime memory: {error}"
            ));
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
            let detail = crate::platform::paths::redact_storage_prefix(&format!(
                "write memory snapshot: {error}"
            ));
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
            let detail =
                crate::platform::paths::redact_storage_prefix(&format!("load {source}: {error}"));
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
    let profile = crate::features::memory::update_profile(patch).map_err(|e| {
        crate::platform::paths::redact_storage_prefix(&format!("update profile: {e}"))
    })?;
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
                let detail = crate::platform::paths::redact_storage_prefix(&format!(
                    "write memory snapshot: {error}"
                ));
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
        Some(sid) => pool.fresh_bridge_for(&sid).await.map_err(|e| {
            // Endpoint/credential probing details + storage root must both be
            // scrubbed before this web-visible arm returns (Round-15 review
            // minor: the organize command folds with the same caliber as the other
            // memory commands).
            // Round-20 review (S10-1): fold first — the old sanitize→fold order let
            // redact_secret's whitespace tokens consume the tail of the folded needle
            // on whitespace-containing homes, leaving drive-letter+username fragments
            // to egress in cleartext; fold first, then run the uniform secret scan,
            // and no home fragment leaks in any shape.
            sanitize_command_error(
                &format!("organize memory: resolve bridge for {sid}"),
                crate::platform::paths::redact_storage_prefix(&format!("{e:#}")),
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
        .map_err(|e| {
            sanitize_command_error(
                "organize memory",
                // Round-20 review (S10-1): same as above, fold first (fixes the
                // whitespace-home fragment leak).
                crate::platform::paths::redact_storage_prefix(&format!("{e:#}")),
            )
        })?;
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
    let event = crate::features::memory::confirm_pending_memory(&id).map_err(|e| {
        crate::platform::paths::redact_storage_prefix(&format!("confirm pending memory: {e}"))
    })?;
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
    let event = match crate::features::memory::ignore_pending_memory(&id).map_err(|e| {
        crate::platform::paths::redact_storage_prefix(&format!("ignore pending memory: {e}"))
    })? {
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
    let event = crate::features::memory::never_pending_memory(&id, reason).map_err(|e| {
        crate::platform::paths::redact_storage_prefix(&format!("never pending memory: {e}"))
    })?;
    finish_memory_write(&app, session_id, &store, event.clone(), event, Vec::new())
}

#[tauri::command]
pub async fn delete_memory_preference(
    id: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<bool>, String> {
    let changed = crate::features::memory::delete_preference(&id).map_err(|e| {
        crate::platform::paths::redact_storage_prefix(&format!("delete preference: {e}"))
    })?;
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
    let mutation = crate::features::memory::update_preference(&id, patch).map_err(|e| {
        crate::platform::paths::redact_storage_prefix(&format!("update preference: {e}"))
    })?;
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
    let mutation = crate::features::memory::update_work_context(&id, patch).map_err(|e| {
        crate::platform::paths::redact_storage_prefix(&format!("update work context: {e}"))
    })?;
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
    let changed = crate::features::memory::delete_work_context(&id).map_err(|e| {
        crate::platform::paths::redact_storage_prefix(&format!("delete work context: {e}"))
    })?;
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
    let item = crate::features::memory::update_timed_memory(&kind, &id, patch).map_err(|e| {
        crate::platform::paths::redact_storage_prefix(&format!("update timed memory: {e}"))
    })?;
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
    let changed = crate::features::memory::delete_timed_memory(&kind, &id).map_err(|e| {
        crate::platform::paths::redact_storage_prefix(&format!("delete timed memory: {e}"))
    })?;
    let event = changed.then(|| crate::features::memory::MemoryWriteEvent {
        kind,
        action: "deleted".to_string(),
        id,
        text: "记忆已删除".to_string(),
    });
    finish_memory_write(&app, session_id, &store, changed, event, Vec::new())
}

/// Optimistic concurrency guard for `edit_last_turn`: rejects the edit when
/// `expected` does not match the currently persisted transcript revision (the
/// transcript is untouched and the turn is not consumed). The stable error codes
/// are in the same family as the store's `compare_and_swap_messages`
/// (`session_revision_conflict`); load/compute failures collapse to
/// `session_revision_unavailable`, with the original error chain (possibly
/// embedding host paths) going only to desktop logs — this guard is the only
/// entrance for web edit-resends crossing the Relay.
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

/// Edits/resends the last user message of a turn.
/// The engine truncates the most recent user+assistant pair at the end of the
/// session and resends with new_message.
/// The frontend must update state.messages itself before calling this command
/// (drop the last pair, add the new user message).
/// `expected_transcript_revision` is an optional optimistic concurrency guard: web
/// edit-resends pass the revision of the snapshot they hold, desktop passes none
/// (None = current behavior unchanged). The guard runs inside the turn gate —
/// once the gate is held, no other turn can be committed, so the current revision
/// cannot advance between check and act — a mismatch is rejected, preventing the
/// phone from truncating, based on a stale view, the newer round the desktop just
/// committed.
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
    let reservation = pool.reserve_turn(&sid).map_err(|e| {
        crate::platform::paths::redact_storage_prefix(&format!("reserve edit_last_turn: {e:#}"))
    })?;
    if let Some(expected) = expected_transcript_revision.as_deref() {
        // If the guard rejects, the reservation Drops here: the uncommitted
        // reservation expires automatically, the gate reopens immediately, and the
        // transcript stays untouched.
        ensure_transcript_revision_current(&store, &sid, expected)?;
    }
    // The edit-resend sends only the edited original text: `EditLastTurn` carries no
    // route, and the engine re-derives the provider routing from the current config
    // (including expert fleet config re-read with the config); on re-derivation the
    // base re-asserts the route quotas the host declared, and does not fall back to
    // directory guessing. The swarm contract lives in spawn-level instructions, so
    // no reminder content needs to be (or should be) re-assembled here.
    let display_message = user_display_message(new_message.clone());

    // Scheduled sessions skip ensure_chat_session: like a steer, EnginePool
    // gates the turn on scheduled_profile; session commands refuse.
    crate::features::assistant::timing::start_turn(&sid);
    if let Err(error) = pool
        .edit_last_turn_reserved(&sid, new_message, display_message, reservation)
        .await
    {
        // Round-27 review M3 (rewriting the round-12 note): the same raw error
        // previously egressed through two paths — finish_turn's send_error text was
        // written into the assistant_done timeline event and read back verbatim via
        // the web-reachable get_session_timeline, while only the return Err side
        // folded. Folding only on the Err side gave false confidence (the new pin
        // pins the single-fold coverage verbatim). Fold once; both egresses share the
        // same folded text. (Round-29 review correction: this arm has no raw-chain
        // `[edit_last_turn]` log leg — both egresses consume `folded` only, so the
        // desktop's own diagnostics lose the raw detail here too; that is the
        // strictly-safer direction and the round-27 wording above no longer
        // claims otherwise.)
        let folded = crate::platform::paths::redact_storage_prefix(&format!("{error:#}"));
        crate::features::assistant::timing::finish_turn(&sid, "send_error", Some(&folded));
        return Err(format!("edit_last_turn: {folded}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Round-24 review (m5) source pin: edit_last_turn's awaited error arm is the
    /// only web-visible fold in this file with no regression signal — the census
    /// only scans `.map_err(|` spellings, and this arm is an `if let Err` +
    /// `return Err` shape outside the census window table (the round-12 comment
    /// disclosed "intentional rather than counted", but disclosure is not pinning:
    /// deleting the fold previously kept every suite green). The window is the
    /// command body; the fold must sit after the `.edit_last_turn_reserved(` call
    /// (arm position — the round-21 arm-boundary lesson); pure comment lines are
    /// filtered first (round-23 caliber).
    #[test]
    fn edit_last_turn_awaited_error_folds_the_storage_prefix() {
        let source = include_str!("memory.rs");
        let start = source
            .find("pub async fn edit_last_turn(")
            .expect("edit_last_turn must exist");
        let body = &source[start..];
        let end = [
            "\n#[tauri::command]",
            "\n#[cfg(test)]",
            "\npub async fn ",
            "\npub fn ",
            "\nfn ",
        ]
        .iter()
        .filter_map(|delimiter| body.find(delimiter))
        .min()
        .expect("edit_last_turn body must be terminated");
        let counted = crate::platform::test_support::comment_filtered(&body[..end]);
        let call_at = counted
            .find(".edit_last_turn_reserved(")
            .expect("the awaited edit_last_turn_reserved call must stay");
        let tail = &counted[call_at..];
        let fold_at = tail
            .find("crate::platform::paths::redact_storage_prefix(&format!(")
            .expect(
                "the awaited-error arm must fold the storage root \
             before egress: deleting the fold previously kept every suite \
             green because the census scans .map_err arms only",
            );
        // Round-27 review M3: the fold must precede the fan-out and both egresses
        // must share the same folded binding — a mutation returning the timeline side
        // (finish_turn) to raw `{error:#}`, or fanning out before the fold, turns
        // either order assertion red.
        let finish_turn_at = tail
            .find("finish_turn(")
            .expect("the send_error timeline egress must stay");
        let folded_use_at = tail
            .find("Some(&folded)")
            .expect("the timeline egress must consume the shared folded text");
        let err_use_at = tail
            .find("edit_last_turn: {folded}")
            .expect("the returned Err must consume the shared folded text");
        assert!(
            fold_at < finish_turn_at
                && finish_turn_at < folded_use_at
                && folded_use_at < err_use_at,
            "the fold must run before the timeline/Err fan-out, and both \
             egresses must consume the shared folded binding; folding only \
             the Err copy (round-24 shape) or a raw timeline egress turns \
             this red"
        );
        assert_eq!(
            counted
                .matches("crate::platform::paths::redact_storage_prefix(&format!(")
                .count(),
            2,
            "two folds in this command: the shared awaited-error binding \
             and the reserve arm (folded as of round-27); a new web-visible \
             arm must join a pin (enumerated-pin residual)"
        );
    }
    use super::*;
    use deepseek_tui::models::{ContentBlock, Message};

    #[test]
    fn whitespace_home_folds_before_the_secret_scan_so_no_fragment_leaks() {
        // Round-20 review (S10-1) behavioral pin: on whitespace-containing homes the
        // old sanitize→fold order let redact_secret's whitespace tokens consume the
        // tail of the folded needle (the `...John [REDACTED]` shape), leaving
        // drive-letter/username fragments egressing in cleartext. The new order
        // (production arm) folds first and scans second: short tokens survive as the
        // folded form and long tokens are masked wholesale as [REDACTED] — no home
        // fragment survives in either direction. The control leg reproduces the leak
        // shape by hand under the old order (pinning the regression's true shape);
        // removing fold-first (reverting to the old order) turns red. Residual note
        // (round-21 review): fold-first shortens a whitespace-free long token that
        // starts with the home (now beginning ~/.pinvou3) — such a token exactly
        // ≥24 long (digits+letters included) may drop below the family threshold
        // after shortening and escape the scan; directionally this trades the old
        // order's guaranteed fragment leak for a narrower masked residual; scanning
        // of non-prefix text is unaffected.
        let base = std::env::temp_dir().join(format!("jd{}", std::process::id()));
        let home = base.join("John Doe").join(".pinvou3");
        std::fs::create_dir_all(&home).unwrap();
        let (_env_guard, _env_restore) =
            crate::platform::test_support::locked_env(&["PINVOU3_HOME"]);
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes
        // serialized in-process (platform::test_support discipline).
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };
        // Paths always take pinvou3_home()'s reading (normalization differences such
        // as macOS /tmp symlinks), keeping the fold needle and the detail same-source.
        let home_now = crate::platform::paths::pinvou3_home();
        let detail = format!(
            "os error 3: {}/sessions2/x.json: No such file or directory",
            home_now.display()
        );
        // Production new order: fold first (sanitize_command_error's inner layer only scans secrets).
        let new_order = sanitize_command_error(
            "organize memory",
            crate::platform::paths::redact_storage_prefix(&detail),
        );
        assert!(
            !new_order.contains("John Doe")
                && !new_order.contains(home_now.display().to_string().as_str()),
            "the fold-first order must not leak any home fragment, got: {new_order}"
        );
        // Control leg: the old order (sanitize first) does produce the fragment leak —
        // this test pins that the shape no longer appears in the production arm. The
        // leak shape requires the whitespace-leading segment to be shorter than the
        // inner secret scan's ≥24-length family; on platforms whose temp dir is
        // itself overlong the shape cannot be constructed (macOS /var/folders…), so
        // the control leg skips; there the whitespace-leading segment itself joins
        // the ≥24 family, and the two orders produce byte-different output (two
        // [REDACTED] under the old order, one folded under the new) but neither
        // leaves fragments — the production-invariant leg (!contains fragments) has
        // no signal for order regressions on that platform, and the order tooth
        // relies on Linux CI's short /tmp (round-23 review caliber note; round-27
        // review m11 correction: the original "both orders produce identical output"
        // is false at the byte level).
        let pre_space = format!("{}/John", base.display());
        if pre_space.len() < 24 {
            let old_order = crate::platform::paths::redact_storage_prefix(&sanitize_command_error(
                "organize memory",
                &detail,
            ));
            assert!(
                old_order.contains("John"),
                "control: the reversed order must reproduce the fragment leak shape this pin forbids, got: {old_order}"
            );
        }

        // The regular direction without a whitespace home is unaffected: the folded form leaves intact.
        let plain_home = base.join("plainhome").join(".pinvou3");
        std::fs::create_dir_all(&plain_home).unwrap();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes
        // serialized in-process (platform::test_support discipline).
        unsafe { std::env::set_var("PINVOU3_HOME", &plain_home) };
        let plain_now = crate::platform::paths::pinvou3_home();
        let plain_detail = format!("os error 3: {}/mem/x.json: boom", plain_now.display());
        let folded = sanitize_command_error(
            "organize memory",
            crate::platform::paths::redact_storage_prefix(&plain_detail),
        );
        assert!(
            folded.contains("organize memory: os error 3: ~/.pinvou3/mem/x.json"),
            "the plain-home fold must survive the secret scan intact, got: {folded}"
        );
    }

    fn revision_message(role: &str, text: &str) -> Message {
        Message {
            role: role.into(),
            content: vec![ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
        }
    }

    /// FIX (remote-control audit): the optimistic concurrency guard of web
    /// `edit_last_turn`. A stale revision must be refused with the transcript intact;
    /// the current revision passes; a missing session collapses to a stable code. The
    /// command body Drops the uncommitted reservation when the guard refuses (gate
    /// reopens, transcript untouched); this test pins the guard's judgment and
    /// no-mutation semantics directly.
    #[test]
    fn edit_last_turn_revision_guard_blocks_stale_views_without_mutating() {
        // Round-6 review: PINVOU3_HOME save/restore moved to the Drop-backed
        // with_temp_home — straight-line restoration is skipped when an assertion
        // fails (panic unwind), leaking the env and temp dir to every later test in
        // this process (the helper's docs explicitly warn about this).
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

            // Phone-side view: only the first round's user message.
            store
                .update_messages(&sid, vec![revision_message("user", "first")])
                .expect("seed stale transcript");
            let stale_revision = crate::features::sessions::transcript_revision(
                &store.load(&sid).expect("load stale").messages,
            )
            .expect("stale revision");

            // The desktop then committed two more rounds (unseen by the phone view).
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

            // The stale view must be refused, and the transcript must not be mutated.
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

            // An edit holding the current revision passes (the actual rewrite is still executed by the engine inside the gate).
            assert!(ensure_transcript_revision_current(&store, &sid, &current_revision).is_ok());

            // A missing session collapses to the stable code; host paths go only to desktop logs.
            assert_eq!(
                ensure_transcript_revision_current(&store, "missing-session", &current_revision)
                    .expect_err("missing session must be refused"),
                "session_revision_unavailable"
            );
        });
    }

    #[test]
    fn memory_warning_details_fold_the_storage_prefix() {
        // Round-9 review minor: the command-level warning/detail texts at four sites
        // must pass through `redact_storage_prefix`. `load_memory_source` has a direct
        // io::Result injection seam and is pinned at the content level (when the
        // error's Display embeds a storage-root absolute path, the outbound detail
        // folds to the ~/.pinvou3 prefix); the other three same-shaped format!
        // wirings have no natural injection seam and are pinned as source contracts
        // over the function bodies (deleting any wiring turns red).
        crate::platform::test_support::with_temp_home("memory-redact-detail", || {
            let mut warnings = Vec::new();
            let mut sources = BTreeMap::new();
            let error = std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "{}: locked",
                    crate::platform::paths::pinvou3_home()
                        .join("memory")
                        .join("pending.json")
                        .display()
                ),
            );
            let _: Vec<String> =
                load_memory_source("pending", Err(error), &mut warnings, &mut sources);
            assert_eq!(
                warnings[0].detail, "load pending: ~/.pinvou3/memory/pending.json: locked",
                "the outbound detail must fold the storage root prefix"
            );
        });

        let source = include_str!("memory.rs");
        for fn_name in [
            "fn runtime_snapshot_best_effort(",
            "fn refresh_memory_snapshot_document(",
            "fn load_memory_source<T: Default>(",
            "pub async fn get_memory_overview(",
        ] {
            let start = source
                .find(fn_name)
                .unwrap_or_else(|| panic!("{fn_name} must exist"));
            let body = &source[start..];
            // Round-14 review minor: the window must close at the nearest next item —
            // previously `find("\npub async fn ").or_else(find("\nfn "))` let a distant
            // async item swallow all intervening function bodies: the
            // runtime_snapshot_best_effort / refresh_memory_snapshot_document windows
            // each swallowed load_memory_source's wiring, and deleting both production
            // wirings kept all four assertions green ("deleting any wiring turns red"
            // held for only half the members). Take the earliest marker hit so the
            // window is exactly the function itself.
            let end = ["\npub async fn ", "\npub fn ", "\nfn ", "\n#[test]"]
                .iter()
                .filter_map(|marker| body.find(marker))
                .min()
                .unwrap_or_else(|| panic!("{fn_name} body must be terminated by the next item"));
            let body = &body[..end];
            assert!(
                body.contains("redact_storage_prefix(&format!("),
                "{fn_name} must fold warning details through redact_storage_prefix"
            );
        }
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

    /// Round-11 review minor (shape-agnostic pin): memory commands are all
    /// web-reachable and their `Err<String>` crosses the relay verbatim — error
    /// assembly must fold the storage-root prefix via the shared
    /// `redact_storage_prefix` (same policy as the Round-9-fixed warning details;
    /// today's std os error Display embeds no paths, but any future
    /// path-embedding error constructor would make this a leak point). Since
    /// round-27 the `reserve edit_last_turn` exception arm folds too (its error
    /// chain carries storage paths via store.load, and the command is
    /// web-reachable; the raw chain goes only to desktop logs per the established
    /// policy). Per-`map_err(|e|` arm windows are asserted, without depending on
    /// rustfmt's line shapes (the single-line shape count was once silently evaded
    /// by folding the line — confirmed by mutation re-runs).
    #[test]
    fn organize_command_arms_fold_before_the_secret_scan() {
        // Round-20 review (S10-1) source pin: on whitespace homes the old
        // sanitize→fold order leaks home fragments (see the whitespace_home
        // behavioral pin); the production arms must fold first — both arm windows
        // must contain the "fold applied to the raw detail" shape
        // (`redact_storage_prefix(&format!(`) and must no longer contain the old
        // order marker (`redact_storage_prefix(&sanitize_command_error`). Reverting
        // to the old order turns this pin red.
        let source = include_str!("memory.rs");
        for (needle, label) in [
            (
                "organize memory: resolve bridge for {sid}",
                "the bridge-resolution arm",
            ),
            (
                "organize_memory_with_llm(&bridge, None)",
                "the organize arm",
            ),
        ] {
            let at = source
                .find(needle)
                .unwrap_or_else(|| panic!("{label} must exist"));
            // The window's lower bound window_lo extends 400 bytes before the needle
            // and lands on a char boundary: the old-order mutation's marker
            // (redact_storage_prefix(&sanitize_command_error(…)) sits before the
            // needle, and a window without leading context would miss it (confirmed
            // by the F6 mutation). Round-21 review: the upper bound window_hi extends
            // the same way and lands on a char boundary — slicing into a multi-byte
            // char would panic (a compile crash rather than a red assertion), the
            // same constraint as the siblings; this also decouples the closure
            // parameter spelling inside the required literals (per the M6b caliber
            // `|e|` is not part of the pinned contract, so renaming to `|error|`
            // must not false-red). Round-22 review (m4): the upper bound tightens
            // from +900 to +500 — both arms' own folds sit within +310 after the
            // needle, while +900 already swept the neighboring arm's fold (about
            // +832 from the bridge needle) into the window, so deleting this arm's
            // own fold would be compensated by the neighbor and go unreported; +500
            // keeps margin for the arm's own fold and excludes the neighbor.
            let mut window_lo = at.saturating_sub(400);
            while window_lo > 0 && !source.is_char_boundary(window_lo) {
                window_lo -= 1;
            }
            let mut window_hi = (at + 500).min(source.len());
            while window_hi > window_lo && !source.is_char_boundary(window_hi) {
                window_hi -= 1;
            }
            let arm = &source[window_lo..window_hi];
            // Round-23 review: the window filters pure comment lines before
            // asserting — the compensating mutation "delete the fold, mention the
            // needle in a comment inside the window" was mutation-proven on the
            // sibling pin (round-16 M6a); this pin did not filter previously, so an
            // existing comment block inside the window could absorb the literal and
            // stay green. (Same caliber note as the web_visible pin: line-start
            // judgment — trailing `//` comments and block-comment mentions can still
            // shield it; fully closing this requires a string-aware lexical scan,
            // handled as a disclosed residual.)
            let arm = crate::platform::test_support::comment_filtered(arm);
            assert!(
                arm.contains("redact_storage_prefix(&format!("),
                "{label} must apply the fold to the raw error detail \
                 (fold-before-scan order)"
            );
            assert!(
                !arm.contains("redact_storage_prefix(&sanitize_command_error"),
                "{label} must not revert to the sanitize-first order that \
                 leaks home fragments on whitespace homes"
            );
        }
    }

    #[test]
    fn web_visible_memory_errors_fold_the_storage_prefix() {
        // Scan only the production half: include_str includes the test module itself, and assertion literals would pollute the count.
        let source = include_str!("memory.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("memory.rs must keep its tests behind #[cfg(test)]");
        let mut arms = 0;
        let mut raw_arms = 0;
        // Round-15 review minor: the scan covers both `.map_err(|e|` and
        // `.map_err(|error|` spellings — the organize command's two arms used the
        // latter, which this pin could not see, so the assertion message overstated
        // coverage. Round-16 review (M6b): switched to scanning `.map_err(|`
        // uniformly — enumerating spellings is whack-a-mole (the `|err|` spelling
        // mutation was proven to bypass), and closure parameter names are not part of
        // the pinned contract. Round-17 review (M4 remainder): window judgment now
        // looks only at non-comment lines — the mutation "delete the fold, mention
        // `redact_storage_prefix` (or `[edit_last_turn]`) in a comment within the
        // 400-byte window and push raw text" stayed green under the original
        // implementation; with comment lines filtered first, literal mentions can no
        // longer shield a bare arm (same filtering caliber as organize.rs's three
        // pins). (Round-18 review caliber note: the filter is line-start judgment —
        // literal mentions in trailing `//` comments or `/* */` block comments can
        // still shield a bare arm; fully closing this requires a string-aware lexical
        // scan, beyond source-pin scope, handled as a disclosed residual.)
        let scan = crate::platform::test_support::comment_filtered(production);
        let spelling = ".map_err(|";
        {
            let mut pos = 0;
            while let Some(found) = scan[pos..].find(spelling) {
                arms += 1;
                let start = pos + found;
                let mut window_end = (start + 400).min(scan.len());
                // The 400-byte window must not land mid multi-byte character: if
                // start+400 crashes into a CJK line the slice panics (a compile crash
                // rather than a red assertion, with a misleading failure language);
                // fall back to the nearest char boundary.
                while window_end < scan.len() && !scan.is_char_boundary(window_end) {
                    window_end -= 1;
                }
                let window = &scan[start..window_end];
                // Round-27 review m7: the `reserve edit_last_turn` exception arm now
                // folds and the exception marker was removed with it — the marker was
                // previously self-satisfied by the arm's own message text, leaving it
                // blind to revert mutations (fold deleted while the message text
                // remained in the window).
                if !(window.contains("redact_storage_prefix")
                    || window.contains("[edit_last_turn]"))
                {
                    raw_arms += 1;
                }
                pos = start + spelling.len();
            }
        }
        assert!(
            arms >= 11,
            "expected the folded error arms plus the edit_last_turn chain, found {arms}"
        );
        assert_eq!(
            raw_arms, 0,
            "every web-visible memory error arm must fold the storage prefix via \
             redact_storage_prefix; the allowed exceptions are the deliberately-raw \
             edit_last_turn reservation chain and its revision-guard log arms"
        );
    }

    /// Round-7 review: the overview's `snapshot_path` display field egresses under
    /// the web access policy (get_memory_overview is allowlisted), so reverting to a
    /// bare `path.display()` would carry the storage-root absolute path across the
    /// relay. That field's redaction site (write_memory_snapshot_document's success
    /// arm) previously had no content-level pin. `State<'_, SessionStore>` cannot be
    /// safely constructed in tests (tauri 2.12's State fields are private, the repo
    /// does not enable tauri's "test" feature, and there is no mock_app precedent —
    /// same conclusion as the checkpoints.rs tests), so a same-width transmute of a
    /// single-reference-field newtype is placed into State to drive the real command
    /// body: empty store + no args → runtime=None → snapshot written → snapshot_path
    /// must be the `~/.pinvou3` folded form.
    #[test]
    fn overview_snapshot_path_folds_host_paths_before_the_relay() {
        crate::platform::test_support::with_temp_home(
            "pinvou3-overview-snapshot-path-redact",
            || {
                let root = crate::platform::paths::pinvou3_home();
                let store = crate::features::sessions::SessionStore::boot_with_scheduled_root(
                    root.join("scheduled"),
                )
                .expect("session store");
                // SAFETY: State<'r, T> is a newtype over a single reference field;
                // the compiler statically verifies both transmute sides have the same
                // size. The reference's lifetime lives as 'r until this closure ends,
                // and State is used read-only via Deref only inside the command under
                // test, introducing no new aliasing or alignment constraints.
                let state: State<'_, SessionStore> = unsafe {
                    std::mem::transmute::<&SessionStore, State<'_, SessionStore>>(&store)
                };
                let overview = tauri::async_runtime::block_on(get_memory_overview(None, state))
                    .expect("overview");
                assert!(
                    overview.sources["snapshot"].available,
                    "a fresh home must produce a written snapshot document"
                );
                let home_text = root.to_string_lossy().to_string();
                assert!(
                    !overview.snapshot_path.contains(home_text.as_str()),
                    "snapshot_path must not carry the absolute storage root: {}",
                    overview.snapshot_path
                );
                assert_eq!(
                    overview.snapshot_path, "~/.pinvou3/user/memory/snapshot.md",
                    "snapshot_path must use the redacted storage-root prefix"
                );
            },
        );
    }

    /// Round-11 review minor (whole-payload scan pin): per-field pins only cover
    /// known path fields; a newly added field carrying absolute paths would be seen
    /// by no existing pin. Serializing the whole `MemoryOverviewState` (the bytes
    /// that actually cross the relay) asserts the absolute storage root does not
    /// appear — a future unredacted path field turns red here.
    #[test]
    fn serialized_memory_overview_carries_no_absolute_storage_root() {
        crate::platform::test_support::with_temp_home(
            "pinvou3-overview-whole-payload-redact",
            || {
                let root = crate::platform::paths::pinvou3_home();
                let store = crate::features::sessions::SessionStore::boot_with_scheduled_root(
                    root.join("scheduled"),
                )
                .expect("session store");
                // Create an active session: the overview's runtime snapshot is built
                // from the active session; with no session, runtime is None and
                // runtime_path never enters the payload, making the scan false-green
                // for that field (confirmed by the first mutation re-run).
                let created = store
                    .create_new("/model".into(), None, root.join("workspace"))
                    .expect("active session");
                store.set_active(Some(created.metadata.id.clone()));
                // SAFETY: State<'r, T> is a newtype over a single reference field;
                // the compiler statically verifies both transmute sides have the same
                // size. The reference's lifetime lives as 'r until this closure ends,
                // and State is used read-only via Deref only inside the command under
                // test, introducing no new aliasing or alignment constraints.
                let state: State<'_, SessionStore> = unsafe {
                    std::mem::transmute::<&SessionStore, State<'_, SessionStore>>(&store)
                };
                let overview = tauri::async_runtime::block_on(get_memory_overview(None, state))
                    .expect("overview");
                let home_text = root.to_string_lossy().to_string();
                let serialized = serde_json::to_string(&overview).expect("overview serializes");
                assert!(
                    !serialized.contains(home_text.as_str()),
                    "the serialized overview payload must not carry the absolute storage root: \
                     {serialized}"
                );
                // Non-empty criterion for the scan: the payload must genuinely contain
                // the redacted storage-prefix form, otherwise the assertion above is
                // false-green for "payload is empty".
                assert!(
                    serialized.contains("~/.pinvou3"),
                    "the overview payload should reference redacted storage paths: {serialized}"
                );
                // Non-empty criterion for runtime_path (it rides the payload with the
                // active session): after redaction it is the
                // ~/.pinvou3/user/memory/runtime/<sid>.md form.
                assert!(
                    serialized.contains("~/.pinvou3/user/memory/runtime/"),
                    "the runtime prompt path must ride the payload redacted: {serialized}"
                );
            },
        );
    }
}
