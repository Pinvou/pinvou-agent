//! Session-creation request watcher — the app-side consumer of the
//! session-reader MCP family's `create_session` spool
//! (docs/builtin-toolset-contract.md §5 L1 / §6; design notes in
//! docs/session-reader-会话创建工具-设计与验收.md).
//!
//! The session-reader MCP server validates a `create_session` call and
//! spools it to `<pinvou3 home>/session-requests/spool/<name>.json` (the
//! spool record schema is the contract between the two sides; the file name
//! is the idempotency identity: the sha256 of
//! `"<from_session>|create|<idempotency_key>"` when a key is given (a key
//! requires `from_session`, so the namespace is never global), a random
//! uuid otherwise, so a retried operation replaces its own pending record).
//! This module is the app-side consumer:
//!
//! - a poll watcher picks spool files up, re-validates them (server-side
//!   checks are not trusted — the spool directory is user-writable), and
//!   creates the session through the panel's own `create_session_record`
//!   semantics (`SessionStore::create_new` with the app-default model and
//!   workspace unless the request pins a saved model / binds an existing
//!   directory; never `set_active` — a tool-created session must not steal
//!   the user's focus, the one deliberate divergence from the Tauri
//!   command's default);
//! - an explicit `title` is applied via `set_title` (which also stops the
//!   first-turn auto-naming: it only fires while the title is still the
//!   default); `first_message`, when given, is delivered as the new
//!   session's opening plain user turn through `deliver_messaging_turn`
//!   (no cross-session header block — it is the opening instruction, not a
//!   relayed message);
//! - a processed request leaves a result marker `spool/.done/<stem>.json`
//!   (`{"ok":true,"session_id","title"[,"first_message_delivered"]}`) so
//!   the MCP server's short synchronous wait can return the session id.
//!   Delivery is **at-least-once**: the marker is written immediately after
//!   `create_new`, and only marker-write failures retry (a crash in that
//!   gap can re-apply and duplicate the session — same accepted window as
//!   features/scheduled, bounded by the marker write following the apply
//!   directly). Post-create steps (title, workspace binding, first-message
//!   delivery) are best-effort after the marker: a session that exists must
//!   never be retried into a second one, so their failures are logged and
//!   audited instead (`bind_session_workspace` failure is the one
//!   exception: it rolls the fresh session back and retries, mirroring the
//!   `create_session` command's rollback, because a session that silently
//!   falls back to the execution root after restart is worse);
//! - poison files (schema drift, hostile content, oversize) are quarantined
//!   under `spool/failed/` immediately with a `{"ok":false,"error"}`
//!   marker; *transient* failures retry up to [`MAX_CREATE_ATTEMPTS`] times
//!   and then quarantine with the same marker; marker and quarantine state
//!   older than [`STATE_RETENTION`] is pruned;
//! - every terminal outcome appends an audit record into the requesting
//!   session's execution root (kind `session_create` /
//!   `session_create_failed`; model-supplied `from_session` is the same
//!   unauthenticated provenance as messaging's — the Ask rule and the audit
//!   trail are the trust boundary), and every successful create emits
//!   `session:list_changed {id, action:"created"}` (the event both
//!   frontends already listen to for list refreshes).
//!
//! The watcher is started once from `lib.rs` setup and lives for the app
//! lifetime (the messaging-watcher form, no cancellation guard). It must be
//! spawned through `tauri::async_runtime::spawn` (not `tokio::spawn`): the
//! setup hook runs outside any raw tokio context (a6d135840 lesson).
//!
//! Why a module of its own (not features/sessions): the watcher needs
//! [`EnginePool`] to resolve the app-default model and to dispatch the first
//! turn, and features::assistant already depends on features::sessions — a
//! sessions → assistant edge would close the feature cycle the architecture
//! guard forbids. A leaf feature that only lib.rs mounts keeps the graph
//! acyclic (same reason features/messaging, not features/sessions, owns the
//! send-message watcher).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::features::assistant::engine_pool::EnginePool;
use crate::features::assistant::platform::bridge::SESSION_CREATE_TOOL;
use crate::features::sessions::SessionStore;
use crate::features::sessions::validators::{
    is_aux_session_id, is_sched_session_id, validate_user_workspace_path,
};
use crate::platform::prefs::UserPrefs;

/// Same bounds as the MCP server's caps — re-checked here because the spool
/// directory is user-writable.
const MAX_TITLE_CHARS: usize = 200;
const MAX_FIRST_MESSAGE_CHARS: usize = 32 * 1024;
const MAX_WORKSPACE_PATH_CHARS: usize = 1024;
const MAX_MODEL_ID_CHARS: usize = 200;
const MAX_SENDER_TITLE_CHARS: usize = 200;
const MAX_IDEMPOTENCY_KEY_CHARS: usize = 128;
const MAX_SESSION_ID_LEN: usize = 128;
/// Spool file size cap: a legitimate record is bounded by the 32k-char
/// first message (~96 KB as UTF-8 CJK) plus small metadata; anything bigger
/// is hostile and is quarantined before being read into memory.
const MAX_SPOOL_FILE_BYTES: u64 = 256 * 1024;
/// Transient creation failures retry on consecutive polls; quarantine only
/// after this many attempts (same shape as features/scheduled).
const MAX_CREATE_ATTEMPTS: u32 = 3;
/// Watch poll interval: session creations are rare; the MCP server's
/// synchronous wait covers up to 5s, so 1s keeps the typical create inside
/// one poll.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Bound for the first-message dispatch (a wedged engine must not stall the
/// watcher forever) — same bound as features/messaging's delivery timeout.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(30);
/// Retention for terminal state (result markers are the cross-restart
/// idempotency window; `failed/` holds quarantine evidence).
const STATE_RETENTION: Duration = Duration::from_secs(14 * 24 * 3600);
/// How often the watcher sweeps stale terminal state and stray `*.tmp`
/// crash leftovers.
const PRUNE_INTERVAL: Duration = Duration::from_secs(60);

fn spool_root() -> PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("session-requests")
        .join("spool")
}

fn failed_dir() -> PathBuf {
    spool_root().join("failed")
}

fn done_dir() -> PathBuf {
    spool_root().join(".done")
}

/// One spooled session-creation request. Field names mirror server.py's
/// `_spool_session_request_payload` exactly (snake_case JSON on disk);
/// unknown fields are skipped on read (contract §4.4 drift defense). The
/// `id` field is informational only — the watcher keys its state on the
/// directory-listed file name, never on this field (a user-writable spool
/// must not control watcher paths).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct SpooledSessionRequest {
    pub schema_version: u32,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub first_message: Option<String>,
    #[serde(default)]
    pub workspace_path: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub from_session: Option<String>,
    #[serde(default)]
    pub from_title: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

impl SpooledSessionRequest {
    /// Server-side re-validation of a spool record (contract §4.4: errors
    /// are explicit; §5: the L1 write re-checks everything it was told).
    /// Mirrors every MCP-server rule checkable without live app state
    /// (shape, caps, charsets, isolation prefixes, absolute workspace);
    /// live probes (workspace exists, model_id is saved) stay with the
    /// domain port and surface as retried failures with a terminal marker.
    pub(crate) fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            bail!("unsupported spool schema_version {}", self.schema_version);
        }
        check_optional_field(&self.title, "title", MAX_TITLE_CHARS)?;
        check_optional_field(
            &self.first_message,
            "first_message",
            MAX_FIRST_MESSAGE_CHARS,
        )?;
        check_optional_field(&self.model_id, "model_id", MAX_MODEL_ID_CHARS)?;
        check_optional_field(&self.from_title, "from_title", MAX_SENDER_TITLE_CHARS)?;
        if let Some(key) = &self.idempotency_key {
            if key.trim().is_empty() {
                bail!("idempotency_key is blank");
            }
            if key.chars().count() > MAX_IDEMPOTENCY_KEY_CHARS {
                bail!("idempotency_key exceeds the {MAX_IDEMPOTENCY_KEY_CHARS} character limit");
            }
            if self.from_session.is_none() {
                // Without a sender the key's namespace would degrade to
                // global: two unattributed senders reusing one key would
                // clobber each other's pending request (mirrors the MCP
                // server's validation).
                bail!("idempotency_key requires from_session so the key is scoped to one sender");
            }
        }
        if let Some(workspace) = self
            .workspace_path
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if workspace.chars().count() > MAX_WORKSPACE_PATH_CHARS {
                bail!("workspace_path exceeds the {MAX_WORKSPACE_PATH_CHARS} character limit");
            }
            // Shape only here; existence/canonicalization is live state and
            // is checked by the domain port (validate_user_workspace_path).
            if !Path::new(workspace).is_absolute() {
                bail!("workspace_path must be an absolute directory path");
            }
        }
        check_sender_session_id(self.from_session.as_ref())?;
        Ok(())
    }
}

fn check_optional_field(value: &Option<String>, label: &str, max_chars: usize) -> Result<()> {
    if let Some(value) = value {
        let value = value.trim();
        if value.is_empty() {
            bail!("{label} is blank");
        }
        if value.chars().count() > max_chars {
            bail!("{label} exceeds the {max_chars} character limit");
        }
    }
    Ok(())
}

fn check_sender_session_id(session_id: Option<&String>) -> Result<()> {
    let Some(id) = session_id else {
        return Ok(());
    };
    if id.is_empty()
        || id.len() > MAX_SESSION_ID_LEN
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("invalid from_session id");
    }
    // The from_session is audit provenance only, but isolated sessions must
    // never appear as the requester: sched- sessions are unattended by
    // design, eval_ is benchmark-private, and aux- marks auxiliary
    // side-chats (the MCP server rejects all three; re-checked here because
    // the spool directory is user-writable — defense in depth, mirrors
    // messaging and features/scheduled).
    if is_sched_session_id(id) || is_aux_session_id(id) || id.starts_with("eval_") {
        bail!("from_session {id} is an isolated session and cannot request session creation");
    }
    Ok(())
}

/// Domain port: the real impl drives the panel's own creation pipeline; the
/// loop's tests inject a stub (native async fn in trait, static dispatch
/// only — same pattern as features/scheduled's TaskCreator).
trait SessionCreator {
    async fn create(
        &self,
        input: CreateSessionInput,
    ) -> std::result::Result<CreatedSession, String>;
}

/// What the watcher hands the domain port: the validated-but-unresolved
/// request fields (workspace/model resolution is live app state and belongs
/// to the port).
struct CreateSessionInput {
    title: Option<String>,
    first_message: Option<String>,
    workspace_path: Option<String>,
    model_id: Option<String>,
}

/// What comes back: the created session's identity plus the best-effort
/// first-message outcome (`None` = no first message was requested).
struct CreatedSession {
    id: String,
    title: String,
    first_message_delivered: Option<bool>,
}

struct PoolCreator<'a> {
    pool: &'a EnginePool,
    store: &'a SessionStore,
}

impl SessionCreator for PoolCreator<'_> {
    async fn create(
        &self,
        input: CreateSessionInput,
    ) -> std::result::Result<CreatedSession, String> {
        // Live validation + resolution, all BEFORE the session exists — an
        // Err from this block is safe to retry (nothing was created).
        let binding = match input.workspace_path.as_deref().map(str::trim) {
            Some(raw) if !raw.is_empty() => Some(
                validate_user_workspace_path(raw)
                    .map_err(|error| format!("invalid workspace_path: {error:#}"))?,
            ),
            _ => None,
        };
        let (model, model_id) = match input.model_id.as_deref().map(str::trim) {
            Some(id) if !id.is_empty() => {
                // Fail fast on an unknown id: a session pinned to a
                // nonexistent saved model would fall back silently after
                // restart — the exact failure the model sidecar write order
                // exists to prevent.
                let prefs = UserPrefs::load();
                let Some(saved) = prefs.model_by_id(id) else {
                    return Err(format!("model_id '{id}' does not match a saved model"));
                };
                (saved.model.clone(), Some(saved.id.clone()))
            }
            _ => self.pool.default_model_for_new_session(),
        };
        let workspace = binding
            .clone()
            .unwrap_or_else(|| self.pool.bridge.workspace.clone());
        let session = self
            .store
            .create_new(model, model_id, workspace)
            .map_err(|error| format!("create_new: {error:#}"))?;
        let id = session.metadata.id.clone();
        // Post-create steps are best-effort by design (see the module docs):
        // a session that exists must never be retried into a duplicate.
        if let Some(canonical) = binding {
            if let Err(error) = self.store.bind_session_workspace(&id, canonical) {
                // Mirror the create_session command's rollback: a failed
                // binding persist must not leave a session that "looked
                // bound but falls back to the execution root after
                // restart". Deleting the fresh empty session makes the
                // whole create retryable.
                let rollback = self.store.delete(&id);
                if let Err(rollback_error) = rollback {
                    log::warn!(
                        "[session-creation] workspace bind failed and rollback delete failed for {id}: bind {error:#}; rollback {rollback_error:#}"
                    );
                }
                return Err(format!("bind workspace: {error:#}"));
            }
        }
        let effective_title = match input
            .title
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            Some(title) => {
                if let Err(error) = self.store.set_title(&id, title.to_string()) {
                    log::warn!(
                        "[session-creation] title set failed for {id} (title stays default): {error:#}"
                    );
                }
                title.to_string()
            }
            None => session.metadata.title.clone(),
        };
        let first_message_delivered = match input.first_message.as_deref().map(str::trim) {
            Some(message) if !message.is_empty() => {
                let dispatched = tokio::time::timeout(
                    DELIVERY_TIMEOUT,
                    self.pool.deliver_messaging_turn(&id, message.to_string()),
                )
                .await;
                match dispatched {
                    Ok(Ok(())) => Some(true),
                    Ok(Err(error)) => {
                        log::warn!(
                            "[session-creation] first message dispatch failed for {id} (session exists; deliver manually): {error:#}"
                        );
                        Some(false)
                    }
                    Err(_) => {
                        log::warn!(
                            "[session-creation] first message dispatch timed out for {id} (session exists; deliver manually)"
                        );
                        Some(false)
                    }
                }
            }
            _ => None,
        };
        Ok(CreatedSession {
            id,
            title: effective_title,
            first_message_delivered,
        })
    }
}

/// Session-list refresh signal: production emits `session:list_changed`
/// with the same `{id, action:"created"}` payload shape the
/// `create_session` command uses (both frontends refresh from the event
/// name); tests count calls.
trait SessionListNotifier: Send + Sync {
    fn created(&self, id: &str);
}

struct EventNotifier<'a>(&'a tauri::AppHandle);

impl SessionListNotifier for EventNotifier<'_> {
    fn created(&self, id: &str) {
        use tauri::Emitter;
        let payload = serde_json::json!({"id": id, "action": "created"});
        let _ = self.0.emit("session:list_changed", payload.clone());
        crate::platform::app_events::forward_app_event(self.0, "session:list_changed", payload);
    }
}

/// Terminal disposition of one spool file after processing. Parse/validate/
/// oversize problems are permanent (retrying cannot succeed); creation
/// failures are treated as transient and retried up to
/// [`MAX_CREATE_ATTEMPTS`].
enum Processed {
    /// Created (or an already-created skip): the marker exists, the spool
    /// file can be removed.
    Done,
    /// Permanent rejection: quarantine now.
    Poison(anyhow::Error),
    /// Transient creation failure: retry.
    Retry(anyhow::Error),
}

/// Append the L1 audit record into the requesting session's execution root
/// (failures inside `audit::append` only log, never panic). Without a
/// usable `from_session` the session record itself plus the result marker
/// are the trail.
fn audit_request(
    sessions: &SessionStore,
    request: &SpooledSessionRequest,
    created: &CreatedSession,
) {
    let Some(from) = request.from_session.as_deref() else {
        return;
    };
    let mut detail = serde_json::json!({
        "tool": SESSION_CREATE_TOOL,
        "session_id": created.id,
        "title": created.title,
        "outcome": "created",
    });
    if request.workspace_path.is_some() {
        detail["workspace_bound"] = serde_json::json!(true);
    }
    if let Some(model_id) = request.model_id.as_deref() {
        detail["model_id"] = serde_json::json!(model_id);
    }
    if request.first_message.is_some() {
        detail["first_message_delivered"] =
            serde_json::json!(created.first_message_delivered.unwrap_or(false));
    }
    if let Ok(roots) = sessions.session_roots(from) {
        crate::features::assistant::audit::append(
            &roots.execution,
            "session_create",
            "app",
            detail,
        );
    }
}

/// Best-effort failure audit: a poison or retry-exhausted record with a
/// usable `from_session` leaves a trace in that session's execution root —
/// the audit trail must not be success-only. Runs before quarantine because
/// it re-reads the spool file.
fn audit_failure(sessions: &SessionStore, path: &Path, error: &anyhow::Error) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let Ok(request) = serde_json::from_slice::<SpooledSessionRequest>(&bytes) else {
        return;
    };
    let Some(from) = request.from_session.as_deref() else {
        return;
    };
    let Ok(roots) = sessions.session_roots(from) else {
        return;
    };
    let error_text = sanitize_marker_text(&format!("{error:#}"));
    let detail = serde_json::json!({
        "tool": SESSION_CREATE_TOOL,
        "outcome": "failed",
        "error": error_text.chars().take(500).collect::<String>(),
    });
    crate::features::assistant::audit::append(
        &roots.execution,
        "session_create_failed",
        "app",
        detail,
    );
}

/// Process one spool file: read → validate → (marker check) → create →
/// marker + audit + notify. The spool identity is the directory-listed file
/// name — the JSON `id` field is never trusted for watcher paths.
async fn process_spool_file<C: SessionCreator, N: SessionListNotifier + ?Sized>(
    path: &Path,
    creator: &C,
    sessions: &SessionStore,
    notifier: &N,
) -> Processed {
    let poisoned = |error: anyhow::Error| Processed::Poison(error);
    let size = match std::fs::metadata(path).map(|meta| meta.len()) {
        Ok(size) => size,
        Err(error) => return poisoned(anyhow::Error::new(error).context("stat spool file")),
    };
    if size > MAX_SPOOL_FILE_BYTES {
        return poisoned(anyhow::anyhow!(
            "spool file exceeds the {} byte cap",
            MAX_SPOOL_FILE_BYTES
        ));
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => return poisoned(anyhow::Error::new(error).context("read spool file")),
    };
    let request: SpooledSessionRequest = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(error) => return poisoned(anyhow::Error::new(error).context("parse spool file")),
    };
    if let Err(error) = request.validate() {
        return poisoned(error);
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let done_marker = done_dir().join(format!("{stem}.json"));
    if result_marker_suppresses(&done_marker) {
        // Idempotent retry after a completed create: the success marker
        // wins and the leftover file is dropped WITHOUT a second create. A
        // failure marker does NOT suppress: the retry re-applies so one
        // terminal failure cannot poison the key forever.
        return Processed::Done;
    }
    // Drop the stale failure marker BEFORE applying: the server's first poll
    // must not replay the previous attempt's error while this fresh create
    // is in flight.
    let _ = std::fs::remove_file(&done_marker);
    let input = CreateSessionInput {
        title: trimmed_non_empty(&request.title),
        first_message: trimmed_non_empty(&request.first_message),
        workspace_path: trimmed_non_empty(&request.workspace_path),
        model_id: trimmed_non_empty(&request.model_id),
    };
    match creator.create(input).await {
        Ok(created) => {
            let mut payload = serde_json::json!({
                "ok": true,
                "session_id": created.id,
                "title": created.title,
            });
            if let Some(delivered) = created.first_message_delivered {
                payload["first_message_delivered"] = serde_json::json!(delivered);
            }
            if let Err(error) =
                write_done_marker(&done_marker, &payload).context("write result marker")
            {
                // A missing marker means the MCP server's short wait times
                // out into "pending" and a retried call could re-create:
                // surface this as a retryable failure instead of reporting
                // success (the documented at-least-once duplicate window).
                return Processed::Retry(error);
            }
            audit_request(sessions, &request, &created);
            notifier.created(&created.id);
            Processed::Done
        }
        Err(error) => Processed::Retry(anyhow::anyhow!(error)),
    }
}

fn trimmed_non_empty(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|trimmed| !trimmed.is_empty())
        .map(str::to_string)
}

fn write_done_marker(path: &Path, payload: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create done dir {}", parent.display()))?;
    }
    // Atomic tmp+rename: the server polls this file, so it must never
    // observe a torn write.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, payload.to_string())
        .with_context(|| format!("write result marker {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("publish result marker {}", path.display()))
}

/// Whether a replayed request with an existing result marker should be
/// suppressed. Only a readable marker with `ok:false` lets the retry
/// re-apply; success (or an unreadable/undecipherable marker — mid-write or
/// hostile) suppresses.
fn result_marker_suppresses(path: &Path) -> bool {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return path.exists(),
    };
    match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(value) => value.get("ok").and_then(serde_json::Value::as_bool) != Some(false),
        Err(_) => true,
    }
}

fn quarantine(path: &Path) {
    let _ = std::fs::create_dir_all(failed_dir());
    let mut target = failed_dir().join(path.file_name().unwrap_or_default());
    if target.exists() {
        // Never overwrite earlier evidence: suffix a counter until free.
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        for counter in 1..u32::MAX {
            let candidate = failed_dir().join(format!("{stem}.{counter}{ext}"));
            if !candidate.exists() {
                target = candidate;
                break;
            }
        }
    }
    if let Err(error) = std::fs::rename(path, &target) {
        // Terminal-path failure: the poison file stays in the spool and will
        // be re-processed (and re-logged) every poll — say so loudly instead
        // of failing silently forever.
        log::warn!(
            "[session-creation] quarantine rename failed for {:?}: {error}",
            path
        );
    }
}

/// Failure markers must not leak absolute host paths into model-visible
/// context (the MCP server scrubs its own errors the same way): fold the
/// user home and the pinvou3 home prefixes into opaque placeholders.
fn sanitize_marker_text(text: &str) -> String {
    let pinvou3_home = crate::platform::paths::pinvou3_home();
    let home = crate::platform::paths::user_home_dir();
    // Longest prefix first so the pinvou3-home placeholder wins over "~".
    let longest_first = [(&pinvou3_home, "<pinvou3-home>"), (&home, "~")];
    let mut sanitized = text.to_string();
    for (prefix, placeholder) in longest_first {
        let prefix_text = prefix.to_string_lossy().to_string();
        if !prefix_text.is_empty() {
            sanitized = sanitized.replace(&prefix_text, placeholder);
        }
    }
    sanitized
}

/// Write a failure marker for an already-consumed spool file (the caller has
/// quarantined it): a still-waiting MCP call receives the failure instead of
/// hanging until its 5s timeout. The marker also unblocks the idempotency
/// key: a later retry re-applies instead of replaying the stale failure
/// forever.
fn write_failure_marker(stem: &str, error: &anyhow::Error) {
    let payload = serde_json::json!({
        "ok": false,
        "error": sanitize_marker_text(&format!("{error:#}")),
    });
    if let Err(marker_error) = write_done_marker(&done_dir().join(format!("{stem}.json")), &payload)
    {
        log::warn!("[session-creation] failure marker write failed for {stem}: {marker_error:#}");
    }
}

/// Age-based retention for terminal state (see [`STATE_RETENTION`]): prune
/// stale markers in `.done/` and evidence in `failed/`, and sweep stray
/// `*.tmp` files left by a crash between mkstemp and rename on the server
/// side (nothing else ever removes them).
fn prune_stale_state() {
    prune_stale_state_at(std::time::SystemTime::now());
}

/// The clock is injected so tests can age files without sleeping.
fn prune_stale_state_at(now: std::time::SystemTime) {
    let prune_dir = |dir: &Path| {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let stale = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age >= STATE_RETENTION);
            if stale {
                // Best effort: a busy file is retried on the next sweep.
                let _ = std::fs::remove_file(&path);
            }
        }
    };
    prune_dir(&done_dir());
    prune_dir(&failed_dir());
    if let Ok(entries) = std::fs::read_dir(spool_root()) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_stray_tmp = path.extension().is_some_and(|ext| ext == "tmp");
            if !is_stray_tmp {
                continue;
            }
            let stale = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age >= STATE_RETENTION);
            if stale {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// Retry bookkeeping per file: attempt count. Entries are removed on every
/// terminal path. `spool_dir_error_logged` keeps the unreadable-directory
/// warning to one line per streak instead of 1 Hz spam.
#[derive(Default)]
struct RetryState {
    attempts: HashMap<String, u32>,
    spool_dir_error_logged: bool,
}

/// Watch loop body: process every pending spool file (sorted names — uuid /
/// key-hash hex), quarantining poison files immediately and creation
/// failures after [`MAX_CREATE_ATTEMPTS`] attempts.
async fn process_pending_spool<C: SessionCreator, N: SessionListNotifier + ?Sized>(
    creator: &C,
    sessions: &SessionStore,
    notifier: &N,
    retries: &mut RetryState,
) {
    let root = spool_root();
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => {
            retries.spool_dir_error_logged = false;
            entries
        }
        // No spool directory yet = nothing was ever requested.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        // Anything else (permissions, ...) is a real anomaly: warn once per
        // streak instead of silently spinning at 1 Hz forever.
        Err(error) => {
            if !retries.spool_dir_error_logged {
                log::warn!("[session-creation] cannot read the spool directory: {error}");
                retries.spool_dir_error_logged = true;
            }
            return;
        }
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    for path in files {
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        match process_spool_file(path.as_path(), creator, sessions, notifier).await {
            Processed::Done => {
                retries.attempts.remove(&name);
                if let Err(error) = std::fs::remove_file(&path) {
                    // The marker exists, so the create stays deduped, but a
                    // stuck file would loop Done/remove every poll — say so
                    // instead of failing silently.
                    log::warn!(
                        "[session-creation] processed {name} but could not remove it: {error}"
                    );
                }
            }
            Processed::Poison(error) => {
                retries.attempts.remove(&name);
                log::warn!("[session-creation] quarantining {name}: {error:#}");
                audit_failure(sessions, &path, &error);
                quarantine(&path);
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&name)
                    .to_string();
                write_failure_marker(&stem, &error);
            }
            Processed::Retry(error) => {
                let count = {
                    let attempt = retries.attempts.entry(name.clone()).or_insert(0);
                    *attempt += 1;
                    *attempt
                };
                if count >= MAX_CREATE_ATTEMPTS {
                    log::warn!(
                        "[session-creation] quarantining {name} after {count} create attempts: {error:#}"
                    );
                    retries.attempts.remove(&name);
                    audit_failure(sessions, &path, &error);
                    quarantine(&path);
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(&name)
                        .to_string();
                    write_failure_marker(&stem, &error);
                } else {
                    log::warn!(
                        "[session-creation] create attempt {count} for {name} failed (will retry): {error:#}"
                    );
                }
            }
        }
    }
}

/// Spawn the session-creation watcher: processes the boot backlog first,
/// then polls until the process exits (the app lifetime is the watcher
/// lifetime — the messaging-watcher form; started once from `lib.rs`
/// setup). Uses `tauri::async_runtime::spawn` (not `tokio::spawn`): the
/// setup hook runs outside any raw tokio context.
pub(crate) fn spawn_session_creation_watcher(
    pool: EnginePool,
    store: SessionStore,
    app: tauri::AppHandle,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        prune_stale_state();
        let mut retries = RetryState::default();
        let mut last_prune = tokio::time::Instant::now();
        loop {
            process_pending_spool(
                &PoolCreator {
                    pool: &pool,
                    store: &store,
                },
                &store,
                &EventNotifier(&app),
                &mut retries,
            )
            .await;
            tokio::time::sleep(POLL_INTERVAL).await;
            if last_prune.elapsed() >= PRUNE_INTERVAL {
                prune_stale_state();
                last_prune = tokio::time::Instant::now();
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as SyncMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// RAII temp PINVOU3_HOME; the ENV_LOCK guard is held across awaits —
    /// safe on the current-thread test runtime, and other suites' env use is
    /// serialized by the same lock (same pattern as features/scheduled).
    struct TempHome {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev: Option<String>,
        dir: std::path::PathBuf,
    }

    impl TempHome {
        fn new() -> Self {
            let lock = crate::platform::paths::tests::ENV_LOCK
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let dir = std::env::temp_dir().join(format!(
                "pinvou3-session-creation-{}-{}",
                std::process::id(),
                crate::platform::paths::tests::unique_suffix()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let prev = std::env::var("PINVOU3_HOME").ok();
            // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
            unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
            TempHome {
                _lock: lock,
                prev,
                dir,
            }
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            match &self.prev {
                // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
                Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
                // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
                None => unsafe { std::env::remove_var("PINVOU3_HOME") },
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn spool_record_json(overrides: &[(&str, serde_json::Value)]) -> String {
        let mut value = serde_json::json!({
            "schema_version": 1,
            "id": "abc123",
            "title": "早报会话",
            "first_message": "汇总今天的新闻",
            "workspace_path": null,
            "model_id": null,
            "from_session": "reqsrc01",
            "from_title": "请求来源",
            "created_at": "2026-09-30T00:00:00Z",
            "idempotency_key": null,
        });
        if let Some(object) = value.as_object_mut() {
            for (key, field) in overrides {
                object.insert((*key).to_string(), field.clone());
            }
        }
        value.to_string()
    }

    /// Programmable creator stub: records the inputs it received and returns
    /// the programmed outcome per call.
    struct StubCreator {
        inputs: SyncMutex<Vec<CreateSessionInput>>,
        outcomes: tokio::sync::Mutex<
            std::collections::VecDeque<std::result::Result<CreatedSession, String>>,
        >,
    }

    impl StubCreator {
        fn always_ok() -> Self {
            Self::with_outcomes(vec![Ok(CreatedSession {
                id: "sess0001".to_string(),
                title: "早报会话".to_string(),
                first_message_delivered: Some(true),
            })])
        }

        fn with_outcomes(outcomes: Vec<std::result::Result<CreatedSession, String>>) -> Self {
            Self {
                inputs: SyncMutex::new(Vec::new()),
                outcomes: tokio::sync::Mutex::new(outcomes.into_iter().collect()),
            }
        }

        async fn next_outcome(&self) -> std::result::Result<CreatedSession, String> {
            let mut queue = self.outcomes.lock().await;
            queue.pop_front().unwrap_or(Ok(CreatedSession {
                id: "sess0001".to_string(),
                title: "早报会话".to_string(),
                first_message_delivered: None,
            }))
        }
    }

    impl SessionCreator for StubCreator {
        async fn create(
            &self,
            input: CreateSessionInput,
        ) -> std::result::Result<CreatedSession, String> {
            self.inputs.lock().unwrap().push(input);
            self.next_outcome().await
        }
    }

    #[derive(Default)]
    struct StubNotifier {
        created: AtomicUsize,
        ids: SyncMutex<Vec<String>>,
    }

    impl SessionListNotifier for StubNotifier {
        fn created(&self, id: &str) {
            self.created.fetch_add(1, Ordering::SeqCst);
            self.ids.lock().unwrap().push(id.to_string());
        }
    }

    fn write_spool(stem: &str, record: &str) -> std::path::PathBuf {
        let dir = spool_root();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{stem}.json"));
        std::fs::write(&path, record).unwrap();
        path
    }

    /// A real session store under the temp home, with the (synthetic)
    /// requester session's execution root materialized so audit lines are
    /// assertable — session_roots is a path authority, it resolves any valid
    /// id without requiring the session to exist.
    fn store_with_requester() -> crate::features::sessions::SessionStore {
        let store = crate::features::sessions::SessionStore::boot_for_process_startup()
            .expect("sessions store");
        let roots = store.session_roots("reqsrc01").expect("roots");
        std::fs::create_dir_all(&roots.execution).unwrap();
        store
    }

    #[tokio::test]
    async fn validation_rejects_bad_shapes() {
        let request =
            serde_json::from_str::<SpooledSessionRequest>(&spool_record_json(&[])).unwrap();
        assert!(request.validate().is_ok());
        let bad = |overrides: &[(&str, serde_json::Value)]| {
            serde_json::from_str::<SpooledSessionRequest>(&spool_record_json(overrides))
                .unwrap()
                .validate()
        };
        assert!(bad(&[("schema_version", serde_json::json!(2))]).is_err());
        assert!(bad(&[("title", serde_json::json!("   "))]).is_err());
        assert!(bad(&[("title", serde_json::json!("x".repeat(201)))]).is_err());
        assert!(
            bad(&[(
                "first_message",
                serde_json::json!("x".repeat(32 * 1024 + 1))
            )])
            .is_err()
        );
        assert!(
            bad(&[("workspace_path", serde_json::json!("relative/path"))]).is_err(),
            "a relative workspace must poison before the domain port"
        );
        assert!(bad(&[("workspace_path", serde_json::json!(""))]).is_ok());
        assert!(bad(&[("model_id", serde_json::json!("x".repeat(201)))]).is_err());
        assert!(
            bad(&[
                ("idempotency_key", serde_json::json!("k")),
                ("from_session", serde_json::Value::Null),
            ])
            .is_err()
        );
        for prefix in ["sched-x", "aux-x", "eval_x", "AUX-x"] {
            assert!(
                bad(&[("from_session", serde_json::json!(prefix))]).is_err(),
                "{prefix} must never appear as the requester"
            );
        }
        assert!(bad(&[("from_session", serde_json::json!("../escape"))]).is_err());
    }

    #[tokio::test]
    async fn creates_session_writes_marker_audits_and_notifies() {
        let _home = TempHome::new();
        let store = store_with_requester();
        let creator = StubCreator::always_ok();
        let notifier = StubNotifier::default();
        let path = write_spool("aaaa1111", &spool_record_json(&[]));

        let mut retries = RetryState::default();
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;

        assert!(!path.exists(), "the spool file is removed after processing");
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("aaaa1111.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(marker["ok"], serde_json::json!(true));
        assert_eq!(marker["session_id"], serde_json::json!("sess0001"));
        assert_eq!(marker["title"], serde_json::json!("早报会话"));
        assert_eq!(marker["first_message_delivered"], serde_json::json!(true));
        let inputs = creator.inputs.lock().unwrap();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].title.as_deref(), Some("早报会话"));
        assert_eq!(inputs[0].first_message.as_deref(), Some("汇总今天的新闻"));
        assert_eq!(inputs[0].workspace_path, None);
        assert_eq!(inputs[0].model_id, None);
        assert_eq!(notifier.created.load(Ordering::SeqCst), 1);
        assert_eq!(*notifier.ids.lock().unwrap(), vec!["sess0001".to_string()]);
        let roots = store.session_roots("reqsrc01").unwrap();
        let audit = std::fs::read_to_string(roots.execution.join("workflow_audit.jsonl")).unwrap();
        assert!(audit.contains("\"kind\":\"session_create\""));
        assert!(audit.contains("\"tool\":\"mcp_session-reader_create_session\""));
        assert!(audit.contains("\"session_id\":\"sess0001\""));
        assert!(audit.contains("\"first_message_delivered\":true"));
    }

    #[tokio::test]
    async fn done_marker_suppresses_recreation() {
        let _home = TempHome::new();
        let store = crate::features::sessions::SessionStore::boot_for_process_startup().unwrap();
        let creator = StubCreator::always_ok();
        let notifier = StubNotifier::default();
        let path = write_spool("bbbb2222", &spool_record_json(&[]));
        std::fs::create_dir_all(done_dir()).unwrap();
        std::fs::write(
            done_dir().join("bbbb2222.json"),
            serde_json::json!({"ok": true, "session_id": "old"}).to_string(),
        )
        .unwrap();

        let mut retries = RetryState::default();
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;

        assert_eq!(creator.inputs.lock().unwrap().len(), 0, "no second create");
        assert!(!path.exists(), "the leftover file is still dropped");
        assert_eq!(notifier.created.load(Ordering::SeqCst), 0);
        // The recorded result stays the authoritative one.
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("bbbb2222.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(marker["session_id"], serde_json::json!("old"));
    }

    #[tokio::test]
    async fn failure_marker_lets_retry_reapply() {
        let _home = TempHome::new();
        let store = crate::features::sessions::SessionStore::boot_for_process_startup().unwrap();
        let creator = StubCreator::always_ok();
        let notifier = StubNotifier::default();
        let _path = write_spool("cccc3333", &spool_record_json(&[]));
        std::fs::create_dir_all(done_dir()).unwrap();
        std::fs::write(
            done_dir().join("cccc3333.json"),
            serde_json::json!({"ok": false, "error": "stale"}).to_string(),
        )
        .unwrap();

        let mut retries = RetryState::default();
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;

        assert_eq!(
            creator.inputs.lock().unwrap().len(),
            1,
            "a failure marker re-applies"
        );
        assert_eq!(notifier.created.load(Ordering::SeqCst), 1);
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("cccc3333.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            marker["ok"],
            serde_json::json!(true),
            "success overwrites the failure"
        );
    }

    #[tokio::test]
    async fn tampered_spool_is_quarantined_without_creating() {
        let _home = TempHome::new();
        let store = crate::features::sessions::SessionStore::boot_for_process_startup().unwrap();
        let creator = StubCreator::always_ok();
        let notifier = StubNotifier::default();
        let path = write_spool(
            "dddd4444",
            &spool_record_json(&[("from_session", serde_json::json!("sched-run1"))]),
        );

        let mut retries = RetryState::default();
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;

        assert!(!path.exists());
        assert!(failed_dir().join("dddd4444.json").exists(), "quarantined");
        assert_eq!(creator.inputs.lock().unwrap().len(), 0);
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("dddd4444.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(marker["ok"], serde_json::json!(false));
        assert!(
            !marker["error"]
                .as_str()
                .unwrap()
                .contains(&_home.dir.to_string_lossy() as &str),
            "the failure marker must not leak the host path"
        );
    }

    #[tokio::test]
    async fn persistent_failure_retries_then_quarantines_with_failure_marker() {
        let _home = TempHome::new();
        let store = crate::features::sessions::SessionStore::boot_for_process_startup().unwrap();
        let creator = StubCreator::with_outcomes(vec![
            Err("transient".to_string()),
            Err("transient".to_string()),
            Err("transient".to_string()),
        ]);
        let notifier = StubNotifier::default();
        let path = write_spool("eeee5555", &spool_record_json(&[]));

        let mut retries = RetryState::default();
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;
        assert!(path.exists(), "still queued after the first failure");
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;
        assert!(path.exists(), "still queued after the second failure");
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;
        assert!(!path.exists(), "quarantined after MAX_CREATE_ATTEMPTS");
        assert!(failed_dir().join("eeee5555.json").exists());
        assert_eq!(creator.inputs.lock().unwrap().len(), 3);
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("eeee5555.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(marker["ok"], serde_json::json!(false));
        assert!(marker["error"].as_str().unwrap().contains("transient"));
    }

    #[tokio::test]
    async fn id_field_is_never_trusted_for_watcher_paths() {
        let _home = TempHome::new();
        let store = crate::features::sessions::SessionStore::boot_for_process_startup().unwrap();
        let creator = StubCreator::always_ok();
        let notifier = StubNotifier::default();
        // A hostile `id` with traversal bytes must not influence where the
        // marker lands: the watcher keys everything on the file name.
        let path = write_spool(
            "ffff6666",
            &spool_record_json(&[("id", serde_json::json!("../../escape"))]),
        );

        let mut retries = RetryState::default();
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;

        assert!(!path.exists());
        assert!(done_dir().join("ffff6666.json").exists());
        assert!(
            std::fs::read_dir(&done_dir())
                .unwrap()
                .flatten()
                .all(|entry| entry.file_name().to_string_lossy().starts_with("ffff6666"))
        );
    }

    #[test]
    fn prune_sweeps_terminal_state_only_past_retention() {
        let _home = TempHome::new();
        std::fs::create_dir_all(done_dir()).unwrap();
        std::fs::create_dir_all(failed_dir()).unwrap();
        let marker = done_dir().join("marker.json");
        std::fs::write(&marker, "{\"ok\":true}").unwrap();
        let evidence = failed_dir().join("evidence.json");
        std::fs::write(&evidence, "{\"ok\":false}").unwrap();
        // Fresh terminal state (mtime ~now) survives a sweep at the real
        // clock: retention is measured from the file's mtime.
        prune_stale_state();
        assert!(marker.exists());
        assert!(evidence.exists());
        // A sweep run as if retention had already elapsed sees both files as
        // stale (the injected clock stands in for aged mtimes) and removes
        // them, together with stray *.tmp crash leftovers in the spool root.
        std::fs::write(spool_root().join("crash.tmp"), "{}").unwrap();
        let aged = std::time::SystemTime::now() + STATE_RETENTION + Duration::from_secs(1);
        prune_stale_state_at(aged);
        assert!(!marker.exists(), "aged marker swept");
        assert!(!evidence.exists(), "aged evidence swept");
        assert!(!spool_root().join("crash.tmp").exists(), "stray tmp swept");
    }
}
