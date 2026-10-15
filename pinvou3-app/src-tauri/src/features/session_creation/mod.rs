// architecture-guard: allow-target-cfg -- the FIFO-gate test plants a real
// unix FIFO at the marker path (the system mkfifo(1); std's unix mkfifo is
// nightly-only) to pin that the gated read never blocks; the test is
// cfg(unix)-gated because FIFOs are a unix concept.
//! Session-creation request watcher — the app-side consumer of the
//! session-reader MCP family's `create_session` spool
//! (docs/builtin-toolset-contract.md §5 L1 / §6; design notes in
//! docs/session-reader-session-creation-tool-design-and-acceptance.md).
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
//!   Delivery is **at-least-once**, with the window stated honestly
//!   (round-1 M-B): the marker is written AFTER the post-create steps
//!   (workspace binding, title, first-message delivery — up to the 30s
//!   delivery bound), so a crash anywhere in that span can re-apply and
//!   duplicate the session. RETRYABLE creator errors get 3 attempts per
//!   attempt-EPISODE (the budget clears on every terminal disposition and
//!   a failure marker re-arms the key — round-11 minor 5 reword; the old
//!   "3 total per key per process" was false as written)
//!   per key per process (the budget is in-memory; a restart re-buys it —
//!   disclosed); permanent validation classes (unknown model, invalid
//!   workspace, isolated requester) poison on the first failure, so a
//!   persistent marker-write failure re-creates up to three sessions —
//!   each created session is audited and announced even when its marker
//!   write fails (an unaudited duplicate is worse than a duplicate).
//!   Post-create steps are best-effort once the session exists: it must
//!   never be retried into a second one by their failures, so those are
//!   logged and audited instead (`bind_session_workspace` failure is the
//!   one exception: it rolls the fresh session back and retries,
//!   mirroring the `create_session` command's rollback, because a session
//!   that silently falls back to the execution root after restart is
//!   worse);
//! - poison files (schema drift, hostile content, oversize) are quarantined
//!   under `spool/failed/` immediately with a `{"ok":false,"error"}`
//!   marker; *transient* failures retry up to [`MAX_CREATE_ATTEMPTS`] times
//!   and then quarantine with the same marker; marker and quarantine state
//!   older than [`STATE_RETENTION`] is pruned;
//! - terminal outcomes append an audit record into the requesting session's
//!   execution root (kind `session_create` /
//!   `session_create_failed`; model-supplied `from_session` is the same
//!   unauthenticated provenance as messaging's — the Ask rule and the audit
//!   trail are the trust boundary). Audit is best-effort and FAIL-OPEN in
//!   three disclosed corners: a charset-valid but NONEXISTENT `from_session`
//!   (the audit append does not create parent dirs), the non-UTF-8-name
//!   quarantine (log line only), and a persistently failing audit sink — a
//!   created session can exist with no audit line; the list event still
//!   fires, and every successful create emits
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
/// Round-2 M5: pending-file ceiling, ported from features/messaging — N
/// planted/backlog records would otherwise mean N real sessions (plus up to
/// 30s of head-of-line first-message delivery each) with every MCP caller's
/// 5s window degrading to "pending". The sorted tail beyond the ceiling
/// quarantines with a failure marker instead of creating.
const MAX_PENDING_FILES: usize = 256;
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
    // Round-2 M1: the sender is REQUIRED at the watcher layer too — the
    // server's schema requirement is not trusted here (the spool directory
    // is user-writable; this module's own trust-boundary docs say so), and
    // a sender-less direct spool write would otherwise create a real
    // session with zero audit lines (the scheduled family closed the same
    // hole with its shadow audit; requiring the field is the cleaner
    // equivalent for a creation tool whose provenance IS the audit).
    let Some(id) = session_id else {
        bail!(
            "missing from_session: the creation audit trail names the requester; the field is required"
        );
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
    // design, eval_ is benchmark-private, aux- marks auxiliary side-chats,
    // and agentic_ marks headless `agent run` sessions (full-product Yolo
    // turns, no unattended shield — round-23 M2: a prompt-injected headless
    // run could otherwise delegate session creation here) (the MCP server
    // rejects all four; re-checked here because the spool directory is
    // user-writable — defense in depth, mirrors messaging and
    // features/scheduled).
    if is_sched_session_id(id)
        || is_aux_session_id(id)
        || id.to_ascii_lowercase().starts_with("agentic_")
        || id.to_ascii_lowercase().starts_with("eval_")
    {
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

/// Model resolution for a creation request (review round-3 M4: extracted so
/// the unknown-model rejection — the fail-fast that prevents a session
/// pinned to a nonexistent saved model — is unit-testable without a live
/// engine pool). An explicit empty/blank id means "app default", matching
/// the panel's creation path.
fn resolve_creation_model(
    default: (String, Option<String>),
    model_id: Option<&str>,
) -> std::result::Result<(String, Option<String>), String> {
    match model_id.map(str::trim) {
        Some(id) if !id.is_empty() => {
            let prefs = UserPrefs::load();
            let Some(saved) = prefs.model_by_id(id) else {
                return Err(format!("model_id '{id}' does not match a saved model"));
            };
            Ok((saved.model.clone(), Some(saved.id.clone())))
        }
        _ => Ok(default),
    }
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
        let (model, model_id) = resolve_creation_model(
            self.pool.default_model_for_new_session(),
            input.model_id.as_deref(),
        )?;
        let workspace = binding
            .clone()
            .unwrap_or_else(|| self.pool.bridge.workspace.clone());
        let session = self
            .store
            .create_new(model, model_id, workspace)
            .map_err(|error| format!("create_new: {error:#}"))?;
        let id = session.metadata.id.clone();
        // Round-4 R2: the work-lane default the frontend applies at session
        // materialization (bridge/sessions.js) never runs for tool-created
        // sessions — without this mirror, a user whose work default is Plan
        // gets a Yolo full-tools first turn from the tool while the panel
        // creates Plan sessions with the identical message. Best-effort:
        // a failed default application logs and leaves Yolo (disclosed).
        {
            use crate::core::mode_state::SerializableMode;
            // Round-7 D: the panel deliberately does NOT apply the work-lane
            // default to workspace-BOUND sessions (they resolve via the code
            // lane's last mode) — mirror that: the Plan default applies only
            // to unbound sessions, restoring the claimed panel parity.
            if binding.is_none() {
                let work_default = self.store.mode_defaults().work;
                if matches!(work_default, Some(SerializableMode::Plan)) {
                    if let Err(error) = self.store.set_mode(&id, SerializableMode::Plan) {
                        log::warn!(
                            "[session-creation] applying the work-lane Plan default to {id} failed: {error:#}"
                        );
                    }
                }
            }
        }
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
                    // Round-11 minor 3: the rollback-failed ghost session is
                    // the one created-session shape whose trail otherwise
                    // lives only in this warn — carry it in the returned
                    // error so the requester-side audit_failure line (the
                    // Poison/retry arms) names the possible leftover.
                    return Err(format!(
                        "bind workspace: {error:#}; rollback delete failed — a created session may remain ({id}): {rollback_error:#}"
                    ));
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
                // Round-5 M1: an explicit title applies as before.
                if let Err(error) = self.store.set_title(&id, title.to_string()) {
                    log::warn!(
                        "[session-creation] title set failed for {id} (title stays default): {error:#}"
                    );
                    // Round-7 should-fix 6: report the session's ACTUAL
                    // title (the derived branch's fallback) — the marker and
                    // audit must not name a title the session doesn't have.
                    session.metadata.title.clone()
                } else {
                    title.to_string()
                }
            }
            // Round-5 M1: the tool description's auto-name promise, honored —
            // the panel chat path names via apply_default_session_title on
            // the first send; the tool path's opening turn never passes
            // there, so derive the title from first_message here with the
            // same guard (only while the title is still the default).
            None => {
                let derived = input
                    .first_message
                    .as_deref()
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(|message| message.chars().take(28).collect::<String>());
                if let Some(title) = derived {
                    if let Err(error) = self.store.set_title(&id, title.clone()) {
                        log::warn!(
                            "[session-creation] auto-title from first_message failed for {id}: {error:#}"
                        );
                        session.metadata.title.clone()
                    } else {
                        title
                    }
                } else {
                    session.metadata.title.clone()
                }
            }
        };
        let first_message_delivered = match input.first_message.as_deref().map(str::trim) {
            Some(message) if !message.is_empty() => {
                let dispatched = tokio::time::timeout(
                    DELIVERY_TIMEOUT,
                    // Round-9 M6: the watcher-delivered opening turn is unattended —
                    // shield the new session's engine against recursive
                    // create_session / goal / scheduled-task writes.
                    self.pool
                        .deliver_messaging_turn(&id, message.to_string(), true),
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
        // Round-7 C: the sender is a model-supplied claim (the scheduled
        // family's provenance flag) — a forged from_session writes this line
        // into the VICTIM's trail; the flag keeps planted evidence
        // distinguishable from verified provenance.
        "claimed_from_session_verified": false,
        // The claimed sender title is recorded verbatim (round-1
        // should-fix: the schema advertises it "for the audit trail").
        "claimed_from_title": request.from_title,
    });
    if request.workspace_path.is_some() {
        detail["workspace_bound"] = serde_json::json!(true);
        // Round-9 minor (forensics) + round-12 minor 6: record the CANONICAL
        // bound path the comment always promised — the raw claimed field
        // coincides with it only for server-spooled requests; direct spool
        // writes audit the uncanonicalized form. Re-canonicalize through the
        // same validator the create path used; on failure fall back to the
        // raw claim (the audit line must not vanish over forensics).
        let canonical = request
            .workspace_path
            .as_deref()
            .map(str::trim)
            .filter(|raw| !raw.is_empty())
            .and_then(|raw| validate_user_workspace_path(raw).ok());
        detail["workspace_path"] = serde_json::json!(
            canonical
                .map(|path| path.to_string_lossy().to_string())
                .or_else(|| request.workspace_path.clone())
        );
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
    // Round-3 F2: re-stat gate (messaging's read_record_for_audit port) —
    // the audit re-read must not load a hostile multi-GB blob whole.
    // Round-12 MAJOR 3 / round-13 MAJOR 2: the re-read is REGULAR-FILE
    // gated and take()-bounded (the same stat→read race as the main path;
    // read_spool_bounded carries both the gate and the cap); best-effort:
    // any refusal skips the line.
    let bytes = match read_spool_bounded(path) {
        SpoolRead::Ok(bytes) => bytes,
        _ => {
            return;
        }
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
        // Round-9 minor: symmetric with the success line — the claimed
        // sender is unverified provenance on the failure trail too.
        "claimed_from_session_verified": false,
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
    retries: &mut RetryState,
) -> Processed {
    let poisoned = |error: anyhow::Error| Processed::Poison(error);
    // Round-13 MAJOR 2: the entry read goes through the regular-file gated,
    // take()-bounded helper — the size-only metadata gate let a FIFO swapped
    // in after collection block File::open forever (the collection-time
    // is_file() filter is stale by processing time), and the old inline
    // open+take had no regular-file gate at all.
    let bytes = match read_spool_bounded(path) {
        SpoolRead::Ok(bytes) => bytes,
        // Round-9 minor: the file vanished mid-scan (a concurrent drain or
        // manual surgery) — nothing to apply and nobody to answer; the old
        // shape classified it permanent poison and wrote a spurious
        // ok:false marker for a file that no longer exists.
        SpoolRead::Vanished => return Processed::Done,
        // A non-regular entry at the spool path (FIFO / symlink / directory)
        // is hostile garbage, not a record: quarantine it — the rename moves
        // it out without any blocking op on the watcher.
        SpoolRead::NotRegular => {
            return poisoned(anyhow::anyhow!("spool path is not a regular file"));
        }
        SpoolRead::Oversize => {
            return poisoned(anyhow::anyhow!(
                "spool file exceeds the {} byte cap",
                MAX_SPOOL_FILE_BYTES
            ));
        }
        SpoolRead::Failed(error) => {
            return poisoned(anyhow::Error::new(error).context("read spool file"));
        }
    };
    let request: SpooledSessionRequest = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(error) => return poisoned(anyhow::Error::new(error).context("parse spool file")),
    };
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let done_marker = done_dir().join(format!("{stem}.json"));
    // Round-9 M1: the digest computed from THIS parsed record — a forged
    // {"ok":true} marker (wrong or absent digest) must not swallow it.
    let expected_digest = session_request_digest(&request);
    // Round-15 minor 7: capture the marker's mtime at probe time — the
    // pre-apply remove below only unlinks a marker UNCHANGED since the
    // probes (the server carries the same mtime re-stat guard on its
    // identical unlink): a fresh marker published in the probe→remove
    // window (even a SUCCESS receipt) must not be destroyed; the record
    // then takes the retry path inside the disclosed window.
    let probe_mtime = std::fs::metadata(&done_marker)
        .and_then(|meta| meta.modified())
        .ok();
    // Round-12 minor 4: the suppression probe runs BEFORE validation (the
    // sibling's recorded-reason ordering, creation_requests.rs) — a leftover
    // record that no longer validates but carries a genuine digest-bound
    // ok:true receipt must take the suppress arm (the key already has its
    // outcome), not the Poison arm's false session_create_failed line with
    // the evidence moved to failed/.
    if result_marker_suppresses(&done_marker, &expected_digest) {
        // Idempotent retry after a completed create: the success marker
        // wins and the leftover file is dropped WITHOUT a second create. A
        // failure marker does NOT suppress: the retry re-applies so one
        // terminal failure cannot poison the key forever.
        // Round-8 M4: the skip is a terminal disposition — audit it once
        // per stem (messaging's already_delivered_skip), so spool surgery
        // or a stale-backup replay never makes a record vanish silently.
        if let (Some(from), Ok(roots)) = (
            request.from_session.as_deref(),
            sessions.session_roots(request.from_session.as_deref().unwrap_or_default()),
        ) {
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            if !from.is_empty() && !retries.skip_audited.contains(&file_name) {
                let detail = serde_json::json!({
                    "tool": SESSION_CREATE_TOOL,
                    "session_id": "",
                    "outcome": "already_created_skip",
                    "claimed_from_session": request.from_session,
                    "claimed_from_session_verified": false,
                });
                crate::features::assistant::audit::append(
                    &roots.execution,
                    "session_create",
                    "app",
                    detail,
                );
                // Round-13 minor 2: the skip-audit dedup set is capped
                // (messaging's shape) — unbounded growth is reachable via
                // endless distinct spool names.
                if retries.skip_audited.len() >= SKIP_AUDITED_CAP {
                    retries.skip_audited.clear();
                }
                retries.skip_audited.insert(file_name);
            }
        }
        return Processed::Done;
    }
    // Round-12 minor 3: a readable ok:true RECEIPT (a genuine
    // write_done_marker payload always carries request_digest) bound to a
    // DIFFERENT payload is terminal-mismatch — drop the record WITHOUT
    // applying and WITHOUT deleting the receipt. The pre-apply remove below
    // would have destroyed the server's answer and un-guarded a completed
    // key, contradicting the module's own never-overwrite-a-readable-ok:true
    // invariant (write_failure_marker enforces it on its side); the server's
    // entry check answers payload_mismatch from the receipt, matching the
    // sibling's shape. A digest-LESS ok:true marker is not a receipt (forged
    // or pre-M1) and does not stop the apply.
    if marker_is_terminal_mismatch(&done_marker, &expected_digest) {
        log::warn!(
            "[session-creation] dropping spool record {stem}: a completed receipt for this key binds a different payload"
        );
        // Round-13 minor 3: the drop is a terminal disposition of a record
        // the tool accepted — it leaves the same once-per-stem audit trace
        // the suppress arm and the ceiling arm honor (deduped via
        // skip_audited so a sticky file cannot append one line per poll).
        if let (Some(from), Ok(roots)) = (
            request.from_session.as_deref(),
            sessions.session_roots(request.from_session.as_deref().unwrap_or_default()),
        ) {
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            if !from.is_empty() && !retries.skip_audited.contains(&file_name) {
                let detail = serde_json::json!({
                    "tool": SESSION_CREATE_TOOL,
                    "session_id": "",
                    "outcome": "dropped_terminal_mismatch",
                    "claimed_from_session": request.from_session,
                    "claimed_from_session_verified": false,
                });
                crate::features::assistant::audit::append(
                    &roots.execution,
                    "session_create",
                    "app",
                    detail,
                );
                if retries.skip_audited.len() >= SKIP_AUDITED_CAP {
                    retries.skip_audited.clear();
                }
                retries.skip_audited.insert(file_name);
            }
        }
        return Processed::Done;
    }
    // Round-12 minor 4: validation now runs AFTER the terminal-marker
    // dispositions (suppress / mismatch) — an unvalidatable leftover with a
    // recorded outcome must not poison.
    if let Err(error) = request.validate() {
        return poisoned(error);
    }
    // Drop the stale failure marker BEFORE applying: the server's first poll
    // must not replay the previous attempt's error while this fresh create
    // is in flight.
    if probe_mtime.is_some()
        && std::fs::metadata(&done_marker)
            .and_then(|meta| meta.modified())
            .ok()
            == probe_mtime
    {
        // Round-24 M1: refuse a non-regular entry (planted directory) at
        // the FINAL marker path — remove_file on a directory is a silent
        // no-op, the rename in write_done_marker would fail, and the
        // record would burn its episode creating real sessions with no
        // honest terminal receipt. Terminal refusal instead.
        if let Ok(meta) = std::fs::symlink_metadata(&done_marker) {
            if !meta.is_file() {
                return poisoned(anyhow::anyhow!(
                    "result marker path is a non-regular entry: {}",
                    done_marker.display()
                ));
            }
        }
        let _ = std::fs::remove_file(&done_marker);
    }
    // Round-4 R1: the retry budget GATES the create. The exhausted state
    // (marker write + quarantine rename both persistently failing) must
    // stop creating sessions — the round-3 shape consulted the budget only
    // for logging, so the compound failure created one session per poll,
    // forever. Exhausted records poison without a further create; the
    // budget is per-process (a restart re-buys attempts, disclosed).
    let budget_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    let exhausted = retries
        .attempts
        .get(&budget_name)
        .map(|count| *count >= MAX_CREATE_ATTEMPTS)
        .unwrap_or(false);
    if exhausted {
        // Round-23 M3: a materially-different body at the same deterministic
        // name resets the budget — the round-12 claim "a terminal failure
        // marker re-arms the key" was false in this corner (the sticky
        // quarantine rename kept the dead gate closed forever). The
        // messaging sibling's replaced-since-first-read shape, ported.
        if retries
            .budget_digest
            .get(&budget_name)
            .map(|recorded| recorded != &expected_digest)
            .unwrap_or(false)
        {
            retries.attempts.remove(&budget_name);
            retries.budget_digest.remove(&budget_name);
            log::warn!(
                "[session-creation] {budget_name}: body replaced since the budget was counted; fresh attempt episode granted"
            );
        } else {
            return poisoned(anyhow::anyhow!(
                "creation retry budget exhausted for {budget_name}; not re-creating — resolve the failure or free room, or ask the user"
            ));
        }
    }
    // Round-17 (#657) MAJOR 1: the chat-budget gate. Tool-created sessions
    // are plain ids competing for the user's 50-chat retention budget, and
    // a looping model at tool pace reaches the cap within minutes — after
    // which every further save EVICTS a pre-existing conversation (with its
    // workspace and checkpoints) unattended, the exact headless
    // carve-out lesson (MAX_HEADLESS_SESSIONS). At cap the watcher refuses
    // with an honest permanent error instead of letting the save evict:
    // nothing was created, no pre-existing session is displaced, and the
    // failure marker tells the model to free room or ask the user. The
    // gate's counting mirrors the sweep exactly (pinned exempted before
    // counting), so "pin sessions" is real advice — a pin moves a
    // conversation out of the counted set while keeping it alive.
    match sessions.plain_chat_budget_full() {
        Ok(true) => {
            // Round-18 minor 1: "nothing was created" is only guaranteed on
            // the FIRST attempt — a retry reaching this arm may follow an
            // attempt whose create succeeded and whose marker write failed
            // (audited and announced); the marker must not deny it.
            let fresh = retries.attempts.get(&budget_name).copied().unwrap_or(0) == 0;
            // Round-20 minor 2: the refusal itself never creates; whether
            // an EARLIER attempt of this key created a session is not
            // provable from the in-memory attempt count (a terminal
            // disposition clears it), so both arms refuse to deny it.
            let qualifier = if fresh {
                "the refused create did not run, though an earlier attempt of this key may have — "
            } else {
                "the refused create did not run; an earlier attempt of this key may have created a session — "
            };
            return poisoned(anyhow::anyhow!(
                "session budget full: the chat retention cap is reached; {qualifier}delete or pin sessions to make room, or ask the user"
            ));
        }
        // Round-18 minor 2: a transient listing fault is not a permanent
        // budget state — spend a retry instead of poisoning with a false
        // "budget full" diagnosis.
        Err(error) => {
            return Processed::Retry(anyhow::anyhow!(
                "could not verify the chat budget; refusing to create: {error:#}"
            ));
        }
        Ok(false) => {}
    }
    let input = CreateSessionInput {
        title: trimmed_non_empty(&request.title),
        first_message: trimmed_non_empty(&request.first_message),
        workspace_path: trimmed_non_empty(&request.workspace_path),
        model_id: trimmed_non_empty(&request.model_id),
    };
    // Round-7 A / round-9 M1 disposition: a keyed retry os.replace'ing the
    // file mid-create (the create runs up to the 30s delivery bound) does
    // NOT re-queue — the completed create's marker is published and the
    // newest body is dropped (the key is completed; the server answers
    // payload_mismatch from the marker). See the Ok arm (the round-12
    // minor-5 digest comparison decides "replaced" now).
    match creator.create(input).await {
        Ok(created) => {
            // Round-12 minor 5: the comparison projects the PAYLOAD fields
            // (the same projection session_request_digest hashes), nulling
            // the server's re-stamped created_at — the byte compare logged
            // the alarming "replaced mid-create" for an identical keyed
            // retry re-spooled inside the create window (messaging's
            // spool_records_equal reasoning). Outcome unchanged; a
            // divergent payload still warns and is dropped.
            // Round-13 MAJOR 1: this re-read is GATED and BOUNDED like every
            // other read of the hostile path — the create awaited up to the
            // 30s delivery bound, so this is the widest stat→read window in
            // the file: the old bare std::fs::read slurped a multi-GB
            // os.replace whole per event and blocked open on a planted FIFO
            // (single watcher task wedged). Any gate refusal (vanished,
            // non-regular, oversize, unreadable) treats the record as
            // replaced — the conservative branch (drop the newest body, the
            // completed create's marker is authoritative).
            let unchanged = match read_spool_bounded(path) {
                SpoolRead::Ok(current) => serde_json::from_slice::<SpooledSessionRequest>(&current)
                    .ok()
                    .map(|current| session_request_digest(&current) == expected_digest)
                    .unwrap_or(false),
                _ => false,
            };
            let mut payload = serde_json::json!({
                "ok": true,
                "session_id": created.id,
                "title": created.title,
                // Round-9 M1: what was APPLIED, digested — the server
                // compares this against a retried call's payload to
                // answer payload_mismatch truthfully (the session_id
                // alone cannot).
                "request_digest": session_request_digest(&request),
            });
            if let Some(delivered) = created.first_message_delivered {
                payload["first_message_delivered"] = serde_json::json!(delivered);
            }
            // Round-1 M-B: the session EXISTS from here on — audit and
            // announce it regardless of the marker write's fate. A failed
            // marker write makes the record retryable (the next attempt can
            // re-create a duplicate — the disclosed at-least-once window),
            // but the created session must never silently exist with no
            // audit line and no list event.
            audit_request(sessions, &request, &created);
            notifier.created(&created.id);
            // Round-9 M1: a keyed resend that os.replace'd the record
            // mid-create no longer re-queues the newest body — that
            // re-queue created a SECOND session while the eventual marker
            // answered the retry "no second session was created". THIS
            // create succeeded: publish its marker (carrying ITS digest)
            // so a same-key waiter gets the first outcome, and drop the
            // newest body — the key is completed, and the server answers
            // the divergent resend with payload_mismatch + "use a new
            // idempotency_key". A failed marker write keeps the retryable
            // path (the documented at-least-once window).
            if !unchanged {
                log::warn!(
                    "[session-creation] spool record replaced mid-create; the completed create's marker is authoritative and the newest body is dropped"
                );
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
            Processed::Done
        }
        Err(error) => {
            let error = anyhow::anyhow!(error);
            // Round-1 should-fix: validation-shaped rejections (unknown
            // model_id, nonexistent workspace_path) can never succeed on
            // retry — poison immediately instead of burning three polls
            // (~3s) of the MCP server's 5s sync window into "pending".
            if is_permanent_creation_error(&format!("{error:#}")) {
                Processed::Poison(error)
            } else {
                // Round-23 M3: record the digest the budget is counted
                // against — the exhausted gate compares it to detect a
                // materially-replaced body and grant a fresh episode.
                retries
                    .budget_digest
                    .entry(budget_name.clone())
                    .or_insert_with(|| expected_digest.clone());
                Processed::Retry(error)
            }
        }
    }
}

/// Domain rejections that cannot succeed on retry (the scheduled family's
/// `is_permanent_domain_error`, ported for the creation vocabulary).
/// Round-9 M3: the two dead entries ("not an existing directory" only
/// ever appears inside the "invalid workspace_path: …" wrapper, and
/// "isolated" is validate-time vocabulary that never reaches the create
/// Err arm this matches against) are removed.
fn is_permanent_creation_error(error: &str) -> bool {
    const PERMANENT_MARKERS: [&str; 2] = ["does not match a saved model", "invalid workspace_path"];
    PERMANENT_MARKERS
        .iter()
        .any(|marker| error.contains(marker))
}

/// Round-9 M1: canonical digest of a spooled request's payload-bearing
/// fields (the app-automations R5-M1 digest, ported): identical field set
/// on both sides, alphabetical keys, compact separators, raw UTF-8,
/// sha256-hex — the server compares a retried payload's digest against the
/// marker's to answer payload_mismatch truthfully (the session_id alone
/// cannot). Provenance fields (from_session, from_title,
/// idempotency_key, created_at, id) name the caller, not the payload, and
/// are deliberately excluded; golden vectors pin the exact bytes both
/// languages produce (`server.py::session_request_digest`).
fn session_request_digest(request: &SpooledSessionRequest) -> String {
    use sha2::Digest as _;
    // Round-10 minor: serialize through a BTreeMap — serde_json resolves
    // with preserve_order (CodeWhale's tui dep), so the json! literal's
    // alphabetical key order was the only thing holding byte parity with
    // the python twin's sort_keys; the map anchors it mechanically.
    let canonical = std::collections::BTreeMap::from([
        ("first_message", serde_json::json!(&request.first_message)),
        ("model_id", serde_json::json!(&request.model_id)),
        ("title", serde_json::json!(&request.title)),
        ("workspace_path", serde_json::json!(&request.workspace_path)),
    ]);
    let serialized = serde_json::to_string(&canonical).unwrap_or_default();
    let digest = sha2::Sha256::digest(serialized.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn trimmed_non_empty(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|trimmed| !trimmed.is_empty())
        .map(str::to_string)
}

/// Round-11 M3 / round-13 MAJOR 2: the spool read is REGULAR-FILE gated and
/// take()-bounded at [`MAX_SPOOL_FILE_BYTES`]. The regular-file gate is not
/// paranoia: the collection-time `path.is_file()` filter is stale by
/// processing time — the drain is sequential across awaited creates of up
/// to 30s each, so the last file of a 256-batch is opened potentially hours
/// after listing, and `metadata()` follows symlinks while a FIFO reports
/// size 0 (the size cap alone passes). `File::open` on a FIFO swapped into
/// the gate→open window would still block (the same residual window as the
/// marker probes, sibling-identical); the scheduled sibling's
/// `read_regular_bounded` carries this gate with this rationale
/// (creation_requests.rs).
enum SpoolRead {
    Ok(Vec<u8>),
    /// The path vanished mid-scan (a concurrent drain or manual surgery).
    Vanished,
    /// A non-regular entry (FIFO / symlink / directory) sits at the path.
    NotRegular,
    /// Over the cap — either side of the read (the racer class).
    Oversize,
    /// Any other I/O failure.
    Failed(std::io::Error),
}

fn read_spool_bounded(path: &Path) -> SpoolRead {
    // Round-23 M1 pre-check: symlink_metadata does not follow symlinks —
    // a planted FIFO or symlink-to-FIFO is refused BEFORE any open can
    // block (the round-25 delegation to the platform primitive lost this
    // gate because the primitive's post-open refusal maps to a different
    // error kind; restoring the pre-check here keeps NotRegular
    // producible and the FIFO test red-on-deletion).
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return SpoolRead::Vanished;
        }
        Err(error) => return SpoolRead::Failed(error),
    };
    if !meta.is_file() {
        return SpoolRead::NotRegular;
    }
    // Round-25 M1: delegate the bounded read to the platform primitive
    // (take-bounded, post-open regular-file refusal).
    match crate::platform::filesystem::read_private_data_file_bytes_within(
        path,
        MAX_SPOOL_FILE_BYTES + 1,
    ) {
        Ok(bytes) => {
            if bytes.len() as u64 > MAX_SPOOL_FILE_BYTES {
                SpoolRead::Oversize
            } else {
                SpoolRead::Ok(bytes)
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SpoolRead::Vanished,
        Err(error) => SpoolRead::Failed(error),
    }
}

/// Round-15 minor 5: the read behind the marker probes' stat gates is
/// take()-bounded too — the stat cap cannot see a file swapped into the
/// gate→open window or grown inside stat→read (the spool entry helper and
/// the sibling's read_regular_bounded carry the same bound). Residual: a
/// FIFO swapped into that window still blocks the open — inside the
/// disclosed same-user boundary, exactly as in the sibling.
fn read_marker_capped(path: &Path) -> Option<Vec<u8>> {
    // Round-25 M1: route through the platform's hardened primitive —
    // O_NOFOLLOW|O_NONBLOCK + post-open regular-file refusal + take-bounded
    // read (the stat→open swap window is closed; the manual pre-checks are
    // deleted because the primitive subsumes them).
    let bytes =
        crate::platform::filesystem::read_private_data_file_bytes_within(path, 64 * 1024 + 1)
            .ok()?;
    (bytes.len() as u64 <= 64 * 1024).then_some(bytes)
}

fn write_done_marker(path: &Path, payload: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create done dir {}", parent.display()))?;
    }
    // Atomic tmp+rename: the server polls this file, so it must never
    // observe a torn write. Round-8 M2 / round-25 M1: the tmp path is
    // model-computable (sha256(from|create|key)) — routed through the
    // platform's hardened primitive (O_NOFOLLOW|O_NONBLOCK + post-open
    // regular-file refusal) so a planted FIFO cannot block the write and
    // a swapped non-regular entry is refused post-open.
    let tmp = path.with_extension("json.tmp");
    {
        let mut writer = crate::platform::filesystem::open_private_tmp_write_file(&tmp)
            .with_context(|| format!("open result marker tmp {}", tmp.display()))?;
        use std::io::Write as _;
        writer
            .write_all(payload.to_string().as_bytes())
            .with_context(|| format!("write result marker {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("publish result marker {}", path.display()))
}

/// Whether a replayed request with an existing result marker should be
/// suppressed (round-9 M1 rule, the authoritative wording): ONLY a readable,
/// digest-bound `ok:true` marker suppresses. `ok:false` lets the retry
/// re-apply, and unreadable/undecipherable/hostile/digest-less markers do
/// NOT suppress — the retry re-applies and the server's poll surfaces the
/// fresh outcome.
fn result_marker_suppresses(path: &Path, expected_digest: &str) -> bool {
    // Round-3 F6 + round-4 R3 caps stay: regular-file gate AND size cap
    // before reading — a planted FIFO must not block the loop, and a
    // multi-GB planted marker must not be slurped whole (real markers are
    // ~100 bytes; 64 KiB is the file's own metadata head bound).
    // Round-9 M1 adds the digest binding (the app-automations round-7
    // gate, ported): suppression now requires a parseable ok:true marker
    // whose request_digest matches THIS request. A forged {"ok":true}
    // (wrong or absent digest) must not silently swallow the queue, and
    // unparseable/unreadable markers no longer suppress — the retry
    // re-applies and the server's poll surfaces the fresh outcome instead
    // of a pending lie. (Round-10 wording: the non-regular, oversize and
    // stat-failure gates return false SILENTLY by design — caps do not
    // need a log line; the parse/digest arms log.)
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => {}
        _ => return false,
    }
    if std::fs::metadata(path)
        .map(|m| m.len() > 64 * 1024)
        .unwrap_or(true)
    {
        return false;
    }
    // Round-15 minor 5: bounded read; the read-failure arms return false
    // silently like the other gates (parse failures still log below).
    let bytes = match read_marker_capped(path) {
        Some(bytes) => bytes,
        None => return false,
    };
    match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(value) => {
            if value.get("ok").and_then(serde_json::Value::as_bool) == Some(false) {
                return false;
            }
            match value
                .get("request_digest")
                .and_then(serde_json::Value::as_str)
            {
                Some(digest) if digest == expected_digest => {}
                Some(_) => {
                    log::warn!(
                        "[session-creation] result marker digest mismatch: {:?}",
                        path
                    );
                    return false;
                }
                None => {
                    log::warn!(
                        "[session-creation] result marker lacks digest binding: {:?}",
                        path
                    );
                    return false;
                }
            }
            log::info!(
                "[session-creation] result marker suppresses replay: {:?}",
                path
            );
            true
        }
        Err(_) => {
            log::warn!("[session-creation] result marker unparseable: {:?}", path);
            false
        }
    }
}

/// Round-12 minor 3: whether the marker is a well-formed ok:true RECEIPT
/// (a genuine write_done_marker payload always carries request_digest)
/// whose digest differs from THIS record's — i.e. a completed create for a
/// different payload of the same key. Such a record is dropped without
/// applying and without deleting the receipt. A digest-LESS ok:true marker
/// is not a receipt (forged or pre-M1) and must not swallow the queue —
/// the caller proceeds. Same hostile-path gates as the suppression probe
/// (regular file + size cap before the read; round-12 MAJOR 2's class).
fn marker_is_terminal_mismatch(path: &Path, expected_digest: &str) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => {}
        _ => return false,
    }
    if std::fs::metadata(path)
        .map(|m| m.len() > 64 * 1024)
        .unwrap_or(true)
    {
        return false;
    }
    let bytes = match read_marker_capped(path) {
        Some(bytes) => bytes,
        None => return false,
    };
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .filter(|value| value.get("ok").and_then(serde_json::Value::as_bool) == Some(true))
        .and_then(|value| {
            value
                .get("request_digest")
                .and_then(serde_json::Value::as_str)
                .map(|digest| digest != expected_digest)
        })
        .unwrap_or(false)
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
    let marker = done_dir().join(format!("{stem}.json"));
    // Round-11 minor 2: NEVER overwrite a readable ok:true receipt — the
    // early poison arms bypass the suppression probe, so a stuck spool
    // file whose bytes later corrupt would have its success receipt
    // replaced, and the model's next retry re-creates a duplicate session.
    // Round-12 MAJOR 2: the receipt read is GATED like every other read of
    // this hostile path (the comment at the tmp gate claims exactly this
    // class): symlink_metadata regular-file gate + size cap BEFORE the
    // read — the stem is model-influenceable (sha256 over
    // from|create|idempotency_key) and .done/ is user-writable; an
    // ungated fs::read on a planted FIFO blocks until a writer appears
    // and wedges the single watcher task while every call answers
    // "pending".
    // Round-24 M1: refuse a non-regular entry at the marker path — the
    // rename in write_done_marker would fail on a planted directory and
    // the record would lose its failure receipt entirely.
    if std::fs::symlink_metadata(&marker)
        .map(|meta| !meta.is_file())
        .unwrap_or(false)
    {
        log::warn!(
            "[session-creation] failure marker path is a non-regular entry: {:?}",
            marker
        );
        return;
    }
    let receipt_gated = std::fs::symlink_metadata(&marker)
        .map(|meta| meta.is_file())
        .unwrap_or(false)
        && !std::fs::metadata(&marker)
            .map(|meta| meta.len() > 64 * 1024)
            .unwrap_or(true);
    if receipt_gated {
        if let Some(existing) = read_marker_capped(&marker) {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&existing) {
                // Round-13 MAJOR 4b (with the round-13 minor-4 alignment): a
                // receipt is a DIGEST-BOUND ok:true — the same definition
                // the suppression probe, the terminal-mismatch check, and
                // the server's entry check all enforce. A digest-LESS
                // ok:true marker is not a recorded result (forged or
                // pre-digest): the failure marker replaces it, exactly as
                // the retry path's pre-apply remove would.
                if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true)
                    && value
                        .get("request_digest")
                        .and_then(serde_json::Value::as_str)
                        .is_some()
                {
                    log::warn!(
                        "[session-creation] refusing to overwrite an ok:true receipt for {stem}"
                    );
                    return;
                }
            }
        }
    }
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
    /// Round-7 B: names whose poison arm has audited this failure streak —
    /// a persistently failing quarantine rename must not append one
    /// audit line per 1 Hz poll (~20 MB/day) forever.
    poison_audited: std::collections::HashSet<String>,
    /// Round-8 M4: stems whose marker-suppression skip has audited — the
    /// same once-per-key dedup as messaging's already_delivered_skip.
    /// Round-13 minor 2: capped (messaging's SKIP_AUDITED_CAP shape) — the
    /// set must not grow without bound on endless distinct spool names.
    skip_audited: std::collections::HashSet<String>,
    /// Round-15 minor 4: stems whose processed file could not be removed —
    /// the Done arm warns per streak and escalates to quarantine after
    /// [`MAX_CREATE_ATTEMPTS`] (messaging's removal_failures shape): a
    /// delivered-but-stuck spool file must not warn at 1 Hz for the
    /// process lifetime.
    removal_failures: HashMap<String, u32>,
    /// Round-23 M3: the request digest the budget was counted against —
    /// at the exhausted gate a DIFFERENT digest means the body was
    /// materially replaced (same deterministic name), so the budget
    /// resets and the replacement gets a fresh episode (messaging's
    /// replaced-since-first-read shape).
    budget_digest: HashMap<String, String>,
}

/// Round-13 minor 2: the skip/mismatch audit dedup cap (messaging's value).
const SKIP_AUDITED_CAP: usize = 4096;
/// Round-15 minor 3: the poison-audit dedup cap (messaging caps every dedup
/// set) — one entry is ~100 bytes, but endless distinct externally-abandoned
/// sticky streaks must not grow the set unbounded.
const POISON_AUDITED_CAP: usize = 4096;

/// Round-15 minor 3: the single gated insert for the poison-audit dedup.
fn note_poison_audited(retries: &mut RetryState, name: &str) {
    if retries.poison_audited.len() >= POISON_AUDITED_CAP {
        retries.poison_audited.clear();
    }
    retries.poison_audited.insert(name.to_string());
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
    // Round-13 minor 2 / round-15 minor 2: the retain runs UNCONDITIONALLY
    // (messaging's shape), not after the empty-listing early return — an
    // externally-deleted record's budget entry must not survive the
    // process, and a same-stem re-spool (sha256 names) must not inherit
    // the stale count (worst case: an externally-deleted
    // sticky-quarantine name re-spooled was poisoned with zero create
    // attempts left).
    let listed: std::collections::HashSet<String> = files
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
        .collect();
    retries.attempts.retain(|name, _| listed.contains(name));
    retries
        .removal_failures
        .retain(|name, _| listed.contains(name));
    retries
        .budget_digest
        .retain(|name, _| listed.contains(name));
    // Round-20 minor 1: the dedup sets ride the same sweep — an
    // externally-deleted sticky record re-spools (deterministic sha256
    // names) and its stale dedup entry would suppress the second failure
    // episode's audit line, contradicting the retain principle above.
    retries.poison_audited.retain(|name| listed.contains(name));
    retries.skip_audited.retain(|name| listed.contains(name));
    // Round-13 minor 1: the empty-listing return sits BEFORE the switch
    // consult (messaging's shape) — an idle app must not read and parse
    // settings.json every 1 Hz forever just to learn the feature is on.
    if files.is_empty() {
        return;
    }
    // Round-2 M5: the ceiling quarantines the sorted tail (hostile growth
    // bounded; each excess record gets a failure marker so the server's
    // next call for it answers the recorded error, not fresh pending).
    // Round-13 minor 1: the ceiling sits BEFORE the switch gate (messaging's
    // round-11 minor-3 shape) — with the feature switched off the gate used
    // to return first and direct spool writes accumulated uncapped.
    if files.len() > MAX_PENDING_FILES {
        for path in files.split_off(MAX_PENDING_FILES) {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("<unnamed>")
                .to_string();
            log::warn!(
                "[session-creation] quarantining {name}: pending-file ceiling {MAX_PENDING_FILES} exceeded"
            );
            // Round-7 G: the ceiling arm audits and clears retry entries
            // (messaging's port) — every terminal disposition of a record
            // the tool accepted leaves a trace, and a stale attempts entry
            // must not leak into a same-key retry.
            let mut suppressed = false;
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                let marker = done_dir().join(format!("{stem}.json"));
                // Round-7 G/should-fix 3: never overwrite a readable
                // ok:true marker — a crash-window leftover beyond the
                // ceiling would otherwise re-apply an already-succeeded
                // key and duplicate the session. Round-9 M1: "readable
                // ok:true" now means digest-bound to THIS record; a record
                // that fails to parse carries no digest and cannot prove a
                // recorded success — refuse to suppress for it.
                // Round-11 M3: BOUNDED (stat-gated take) — the ceiling tail
                // is exactly the hostile zone; a multi-GB record was read
                // whole once per 1 Hz poll.
                suppressed = match read_spool_bounded(&path) {
                    SpoolRead::Ok(bytes) => serde_json::from_slice::<SpooledSessionRequest>(&bytes)
                        .ok()
                        .map(|request| {
                            result_marker_suppresses(&marker, &session_request_digest(&request))
                        })
                        .unwrap_or(false),
                    _ => false,
                };
                if !suppressed {
                    write_failure_marker(
                        stem,
                        &anyhow::anyhow!("pending-file ceiling {MAX_PENDING_FILES} exceeded"),
                    );
                }
            }
            // Round-9 minor: the audit is deduped per name (the poison
            // arm's poison_audited) and skipped when a digest-bound
            // success suppressed — a persistently failing quarantine
            // rename must not append one session_create_failed line per
            // 1 Hz poll, and a recorded success must not gain a false
            // failure line.
            if !suppressed && !retries.poison_audited.contains(&name) {
                audit_failure(
                    sessions,
                    &path,
                    &anyhow::anyhow!("pending-file ceiling {MAX_PENDING_FILES} exceeded"),
                );
                note_poison_audited(retries, &name);
            }
            quarantine(&path);
            if !path.exists() {
                retries.attempts.remove(&name);
                retries.poison_audited.remove(&name);
            }
        }
    }
    // Round-2 SF2 / round-13 minor 1: the switch gate comes AFTER the
    // listing, the empty return, and the ceiling (messaging's shape) —
    // `session-creation` off hides the tool AND pauses creation from
    // already-spooled records, but never lets the pending set grow
    // unbounded while off.
    if crate::features::marketplace::builtin::feature_disabled_tool_names()
        .iter()
        .any(|tool| tool == SESSION_CREATE_TOOL)
    {
        return;
    }
    for path in files {
        // A non-UTF-8 file name can never be processed or keyed — it would
        // silently occupy a ceiling slot forever (messaging's port): quaran-
        // tine it immediately instead.
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
        else {
            log::warn!(
                "[session-creation] quarantining non-UTF-8 spool name {:?}",
                path
            );
            quarantine(&path);
            continue;
        };
        // Round-11 M2: the panic guard is PER FILE — the round-9 poll-level
        // wrap let a deterministic panic at a fixed sorted position starve
        // every later-sorted request for the process lifetime (the server
        // kept spooling into a "living" queue). A panicking record burns
        // its own budget and is quarantined after MAX attempts, like any
        // other persistent failure (messaging's round-8 M2 shape — the
        // old comment wrongly claimed this was already ported).
        let outcome = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
            process_spool_file(path.as_path(), creator, sessions, notifier, retries),
        ))
        .await;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(panic) => {
                log::error!("[session-creation] processing {name} panicked: {panic:?}");
                let count = {
                    let attempt = retries.attempts.entry(name.clone()).or_insert(0);
                    *attempt += 1;
                    *attempt
                };
                if count >= MAX_CREATE_ATTEMPTS {
                    log::warn!(
                        "[session-creation] quarantining {name} after {count} panicking attempts"
                    );
                    let panic_error = anyhow::anyhow!("processing panicked {count} times");
                    // Round-12 minor 2: the same once-per-name dedup every
                    // other terminal arm carries — a deterministic panic
                    // plus a persistently failing quarantine rename would
                    // otherwise append one audit line per 1 Hz tick (the
                    // exact ~20 MB/day shape the dedup was built for).
                    if !retries.poison_audited.contains(&name) {
                        audit_failure(sessions, &path, &panic_error);
                        note_poison_audited(retries, &name);
                    }
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(&name)
                        .to_string();
                    write_failure_marker(&stem, &panic_error);
                    quarantine(&path);
                    if !path.exists() {
                        retries.attempts.remove(&name);
                        // Same streak-end cleanup as the Retry arm (round-12
                        // minor 1): the dedup must not outlive its file.
                        retries.poison_audited.remove(&name);
                    }
                }
                continue;
            }
        };
        match outcome {
            Processed::Done => {
                retries.attempts.remove(&name);
                // Round-10 M3: the skip-audit key is cleared ONLY when the
                // file actually moved (the Poison arm's post-rename guard
                // shape) — the round-9 form removed it unconditionally in
                // the same pass that inserts it, so the once-per-stem dedup
                // never deduped: a sticky spool file (dir readable but not
                // writable, digest-bound success marker present) re-entered
                // the suppress branch every 1 Hz poll with one audit line +
                // warn per second, app lifetime. Round-18 minor 4: the clear
                // runs AFTER the removal — the pre-removal `!path.exists()`
                // guard was dead on the normal path (the file the suppress/
                // mismatch arms saw still exists at this point), leaving the
                // dedup insert-only until the 4096 wipe.
                if !path.exists() {
                    retries.skip_audited.remove(&name);
                }
                if let Err(error) = std::fs::remove_file(&path) {
                    // Round-13 minor 13: a record that vanished mid-scan is
                    // the read arms' intentional Done — not a warn.
                    if error.kind() != std::io::ErrorKind::NotFound {
                        // Round-15 minor 4: the marker exists, so the create
                        // stays deduped — but a stuck file must not loop
                        // Done/remove at 1 Hz for the process lifetime:
                        // count the streak and quarantine after MAX
                        // (messaging's removal_failures shape; the marker
                        // keeps the quarantined record deduped).
                        let fails = {
                            let entry = retries.removal_failures.entry(name.clone()).or_insert(0);
                            *entry += 1;
                            *entry
                        };
                        log::warn!(
                            "[session-creation] processed {name} but could not remove it (attempt {fails}): {error}"
                        );
                        if fails >= MAX_CREATE_ATTEMPTS {
                            log::warn!(
                                "[session-creation] quarantining stuck spool file {name} after {fails} removal failures"
                            );
                            quarantine(&path);
                            retries.removal_failures.remove(&name);
                        }
                    } else {
                        retries.removal_failures.remove(&name);
                    }
                } else {
                    // Round-18 minor 4: the move happened — the next
                    // same-stem episode must audit again.
                    retries.skip_audited.remove(&name);
                    retries.removal_failures.remove(&name);
                }
            }
            Processed::Poison(error) => {
                // Round-5 M2: the budget clears only when the quarantine
                // rename actually moved the file (post-rename check, the
                // Retry arm's round-3 F3 shape) — the round-4 version
                // removed unconditionally, so a persistently failing rename
                // recycled the budget every cycle and re-created ~3 sessions
                // per cycle forever (the exact compound failure R1 claimed
                // to close). Round-23 M3: the arm-level warn joins the audit
                // behind the once-per-streak dedup — a sticky poison must
                // not howl at 1 Hz forever.
                if !retries.poison_audited.contains(&name) {
                    log::warn!("[session-creation] quarantining {name}: {error:#}");
                }
                if !retries.poison_audited.contains(&name) {
                    audit_failure(sessions, &path, &error);
                    note_poison_audited(retries, &name);
                }
                quarantine(&path);
                if !path.exists() {
                    retries.attempts.remove(&name);
                    retries.poison_audited.remove(&name);
                }
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
                    // Round-10 nit: the same once-per-name dedup as the
                    // Poison arm — a persistently failing quarantine rename
                    // must not append one line per 1 Hz poll here either.
                    if !retries.poison_audited.contains(&name) {
                        audit_failure(sessions, &path, &error);
                        note_poison_audited(retries, &name);
                    }
                    quarantine(&path);
                    // Round-3 F3: reset the budget only AFTER quarantine
                    // actually moved the file (checked post-rename) — the
                    // round-2 SF3 guard tested pre-quarantine where the
                    // file always still existed, making it dead code; a
                    // persistently failing rename must not re-create ~3
                    // sessions every poll forever.
                    if !path.exists() {
                        retries.attempts.remove(&name);
                        // Round-12 minor 1: the failure streak ended with
                        // the move — clear the audit dedup too. Leaving it
                        // in `poison_audited` left a same-key second
                        // failure episode silently un-audited and grew the
                        // set unboundedly (nothing else removes entries
                        // keyed by a file that no longer exists).
                        retries.poison_audited.remove(&name);
                    }
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
/// Round-13 minor 10: the prune pass runs OUTSIDE the poll-level
/// catch_unwind (pre-loop and in-loop) — messaging wraps the identical pass
/// in the same guard, because a panic here kills the watcher task silently
/// (the JoinHandle is dropped and every later call answers "pending").
fn guarded_prune() {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(prune_stale_state)).is_err() {
        log::error!("[session-creation] prune pass panicked; continuing");
    }
}

pub(crate) fn spawn_session_creation_watcher(
    pool: EnginePool,
    store: SessionStore,
    app: tauri::AppHandle,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        guarded_prune();
        let mut retries = RetryState::default();
        let mut last_prune = tokio::time::Instant::now();
        loop {
            // Round-1 M-C: the poll-level backstop (round-11 M2 moved the
            // per-FILE guard into the drain loop — this outer wrap now
            // only catches panics outside the per-file span, e.g. the
            // read_dir/sort pass).
            let creator = PoolCreator {
                pool: &pool,
                store: &store,
            };
            let notifier = EventNotifier(&app);
            let poll = process_pending_spool(&creator, &store, &notifier, &mut retries);
            if let Err(panic) =
                futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(poll)).await
            {
                log::error!("[session-creation] watcher poll panicked: {panic:?}");
            }
            tokio::time::sleep(POLL_INTERVAL).await;
            if last_prune.elapsed() >= PRUNE_INTERVAL {
                guarded_prune();
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

    /// Round-13 MAJOR 4c: count audit lines whose serialized JSON contains
    /// `needle` in the requester's execution trail — the audit-dedup gates
    /// are otherwise unobservable to the suite.
    fn count_audit_lines(
        store: &crate::features::sessions::SessionStore,
        from: &str,
        needle: &str,
    ) -> usize {
        let Ok(roots) = store.session_roots(from) else {
            return 0;
        };
        let Ok(text) = std::fs::read_to_string(roots.execution.join("workflow_audit.jsonl")) else {
            return 0;
        };
        text.lines().filter(|line| line.contains(needle)).count()
    }

    fn count_audit_kinds(
        store: &crate::features::sessions::SessionStore,
        from: &str,
        kind: &str,
    ) -> usize {
        count_audit_lines(store, from, &format!("\"kind\":\"{kind}\""))
    }

    #[tokio::test]
    async fn validation_rejects_bad_shapes() {
        let request =
            serde_json::from_str::<SpooledSessionRequest>(&spool_record_json(&[])).unwrap();
        assert!(request.validate().is_ok());
        // Round-3 F7: the required-sender gate pinned ALONE (no idempotency
        // key present — the key-requires-sender arm must not be the only
        // thing catching a missing sender; a direct spool write is the
        // threat model round-2 M1 added the gate for).
        let no_sender = serde_json::from_str::<SpooledSessionRequest>(&spool_record_json(&[(
            "from_session",
            serde_json::Value::Null,
        )]))
        .unwrap();
        let err = no_sender.validate().unwrap_err();
        assert!(
            err.to_string().contains("from_session"),
            "the missing-sender bail must fire on its own: {err}"
        );
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
        for prefix in ["sched-x", "aux-x", "eval_x", "EVAL_x", "AUX-x"] {
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
        // Round-9 M1: the planted marker must be digest-bound to the
        // record — the gate refuses bare {"ok":true} plants now.
        let digest = session_request_digest(
            &serde_json::from_str::<SpooledSessionRequest>(&spool_record_json(&[])).unwrap(),
        );
        std::fs::create_dir_all(done_dir()).unwrap();
        std::fs::write(
            done_dir().join("bbbb2222.json"),
            serde_json::json!({"ok": true, "session_id": "old", "request_digest": digest})
                .to_string(),
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

    /// Round-5 M2/M6: the retry budget GATES the create (pre-seed the
    /// exhausted budget and assert no create fires). The rename-stickiness
    /// half is pinned by its own failing-rename fixture,
    /// `exhausted_budget_sticks_when_quarantine_rename_fails` (round-9 M5):
    /// deleting the gate, the >=, or either post-rename guard turns one of
    /// the two tests red.
    /// Round-8 M5d: the pending-file ceiling is pinned — the sorted tail
    /// beyond 256 quarantines with a failure marker and creates nothing.
    #[tokio::test]
    async fn pending_ceiling_quarantines_tail_without_creating() {
        let _home = TempHome::new();
        let store = crate::features::sessions::SessionStore::boot_for_process_startup().unwrap();
        let creator = StubCreator::always_ok();
        let notifier = StubNotifier::default();
        let spool = spool_root();
        std::fs::create_dir_all(&spool).unwrap();
        for i in 0..=(MAX_PENDING_FILES as u32) {
            let name = format!("c{i:05}.json");
            std::fs::write(spool.join(&name), spool_record_json(&[])).unwrap();
        }
        let mut retries = RetryState::default();
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;
        assert_eq!(
            creator.inputs.lock().unwrap().len(),
            MAX_PENDING_FILES,
            "exactly the ceiling creates; the tail never reaches creation"
        );
        assert!(
            failed_dir()
                .join(format!("c{:05}.json", MAX_PENDING_FILES))
                .exists(),
            "the sorted tail is the quarantined excess"
        );
    }

    /// Round-8 M5a (Rust twin): a non-regular marker file does not
    /// suppress (the FIFO/regular-file gate on result_marker_suppresses).
    #[test]
    fn nonregular_marker_does_not_suppress() {
        let _home = TempHome::new();
        let dir = done_dir();
        std::fs::create_dir_all(&dir).unwrap();
        // A directory at the marker path is the portable non-regular file.
        std::fs::create_dir_all(dir.join("dir.json")).unwrap();
        assert!(
            !result_marker_suppresses(&dir.join("dir.json"), "d"),
            "a non-regular marker must not suppress"
        );
    }

    #[tokio::test]
    async fn exhausted_budget_gates_the_create() {
        let _home = TempHome::new();
        let store = crate::features::sessions::SessionStore::boot_for_process_startup().unwrap();
        let creator = StubCreator::always_ok();
        let notifier = StubNotifier::default();
        let path = write_spool("ffff6666", &spool_record_json(&[]));
        let mut retries = RetryState::default();
        retries
            .attempts
            .insert("ffff6666.json".to_string(), MAX_CREATE_ATTEMPTS);
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;
        assert_eq!(
            creator.inputs.lock().unwrap().len(),
            0,
            "an exhausted budget must not create again"
        );
        assert!(
            failed_dir().join("ffff6666.json").exists(),
            "the exhausted record poisons (renamable here)"
        );
        assert!(!path.exists());
    }

    /// Round-5 M6 / round-10 M2(a): the marker-read gates — an oversize
    /// regular-file marker is refused (not slurped) and does not suppress.
    /// The payload is VALID JSON over the cap (the python twin's round-6
    /// lesson): the round-5 form used 65 KiB of `x`, so parse-refusal
    /// masked cap deletion and the cap arm was unpinned. Round-13 MAJOR
    /// 4a: the payload also carries the EXPECTED digest — the round-12
    /// form omitted it, so the None-digest arm refused anyway and cap
    /// deletion stayed green (the same refusal-masking mistake, one round
    /// later). With the digest bound, ONLY the cap can refuse this marker.
    #[tokio::test]
    async fn oversize_marker_does_not_suppress() {
        let _home = TempHome::new();
        std::fs::create_dir_all(done_dir()).unwrap();
        let oversize = format!(
            "{{\"ok\": true, \"session_id\": \"s\", \"request_digest\": \"d\", \"pad\": \"{}\"}}",
            "x".repeat(66 * 1024),
        );
        std::fs::write(done_dir().join("huge.json"), oversize).unwrap();
        assert!(
            !result_marker_suppresses(&done_dir().join("huge.json"), "d"),
            "an oversize marker must be refused before read/parsing — the \
             payload is valid JSON with the matching digest so only the cap \
             can refuse it"
        );
    }

    /// Round-9 M1: committed golden vectors — canonicalization drift (key
    /// set, separators, coercion, encoding) turns these red because the
    /// expectations are literal hex, not suite-computed; the python twin
    /// pins the same two vectors (test_session_reader_server.py).
    #[test]
    fn session_request_digest_golden_vectors() {
        let request = SpooledSessionRequest {
            schema_version: 1,
            id: "irrelevant".to_string(),
            title: Some("早报会话".to_string()),
            first_message: Some("汇总今天的新闻".to_string()),
            workspace_path: None,
            model_id: None,
            from_session: Some("reqsrc01".to_string()),
            from_title: Some("请求来源".to_string()),
            created_at: "2026-09-30T00:00:00Z".to_string(),
            idempotency_key: Some("k".to_string()),
        };
        assert_eq!(
            session_request_digest(&request),
            "eb1b90a155dee08c5c09c9e74cce0276244ada0445a50f14f866106ac799a97b"
        );
        let mut divergent = request;
        divergent.title = Some("夜报".to_string());
        divergent.first_message = None;
        divergent.model_id = Some("gpt-x".to_string());
        divergent.workspace_path = Some("/tmp/ws".to_string());
        assert_eq!(
            session_request_digest(&divergent),
            "77cd5ad19bed3538ee2b5fbdd230e5fe27c8c67c74dcf2735deba27d81b6d0a4"
        );
    }

    /// Round-9 M1: the suppression gate's digest binding — a matching
    /// digest suppresses; a wrong digest, an absent digest, `ok:false`,
    /// and garbage all refuse (the forged-{"ok":true} defense).
    #[test]
    fn result_marker_suppression_requires_digest_binding() {
        let dir = std::env::temp_dir().join(format!("pinvou-sr-marker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("m.json");
        let good = serde_json::json!({
            "ok": true, "session_id": "s", "request_digest": "d1",
        })
        .to_string();
        std::fs::write(&marker, &good).unwrap();
        assert!(result_marker_suppresses(&marker, "d1"));
        assert!(
            !result_marker_suppresses(&marker, "d2"),
            "wrong digest refuses"
        );
        std::fs::write(
            &marker,
            serde_json::json!({"ok": true, "session_id": "s"}).to_string(),
        )
        .unwrap();
        assert!(
            !result_marker_suppresses(&marker, "d1"),
            "an unbound ok:true refuses"
        );
        std::fs::write(
            &marker,
            serde_json::json!({"ok": false, "error": "e"}).to_string(),
        )
        .unwrap();
        assert!(!result_marker_suppresses(&marker, "d1"));
        std::fs::write(&marker, b"not json{").unwrap();
        assert!(
            !result_marker_suppresses(&marker, "d1"),
            "an unparseable marker refuses"
        );
        assert!(!result_marker_suppresses(&dir.join("missing.json"), "d1"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Round-9 minor: the permanent-error vocabulary is pinned — a wording
    /// drift here converts fail-fast into three wasted polls (green suite,
    /// worse behavior), and the two dead round-8 entries stay dead.
    #[test]
    fn permanent_error_vocabulary_is_pinned() {
        assert!(is_permanent_creation_error(
            "model_id gpt-x does not match a saved model"
        ));
        assert!(is_permanent_creation_error(
            "invalid workspace_path: not an existing directory"
        ));
        assert!(!is_permanent_creation_error("spool store is read-only"));
        // The removed dead entries: validate-time vocabulary that never
        // reaches this matcher must not resurrect as live markers.
        assert!(!is_permanent_creation_error("transient isolate glitch"));
    }

    /// A creator whose create() is in flight while the spool file is
    /// os.replace'd with a divergent same-key payload (round-9 M1).
    struct MidCreateSwapper {
        path: std::path::PathBuf,
        inputs: SyncMutex<Vec<CreateSessionInput>>,
    }

    impl SessionCreator for MidCreateSwapper {
        async fn create(
            &self,
            input: CreateSessionInput,
        ) -> std::result::Result<CreatedSession, String> {
            self.inputs.lock().unwrap().push(input);
            // The resend lands WHILE the create is in flight: same key,
            // divergent payload (title changed).
            std::fs::write(
                &self.path,
                spool_record_json(&[("title", serde_json::json!("改名会话"))]),
            )
            .unwrap();
            Ok(CreatedSession {
                id: "sess0001".to_string(),
                title: "早报会话".to_string(),
                first_message_delivered: Some(true),
            })
        }
    }

    /// Round-9 M1: a keyed resend landing mid-create no longer re-queues
    /// the newest body — the completed create's marker (with ITS digest)
    /// is published, the divergent body is dropped, and no second session
    /// is created; the server answers payload_mismatch from the marker.
    #[tokio::test]
    async fn replaced_mid_create_publishes_marker_and_drops_newest_body() {
        let _home = TempHome::new();
        let store = crate::features::sessions::SessionStore::boot_for_process_startup().unwrap();
        let path = write_spool("mid0009", &spool_record_json(&[]));
        let creator = MidCreateSwapper {
            path: path.clone(),
            inputs: SyncMutex::new(Vec::new()),
        };
        let notifier = StubNotifier::default();
        let mut retries = RetryState::default();
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;

        assert_eq!(
            creator.inputs.lock().unwrap().len(),
            1,
            "exactly the first create ran"
        );
        assert!(
            !path.exists(),
            "the newest body is dropped (Done removes it)"
        );
        assert_eq!(notifier.created.load(Ordering::SeqCst), 1);
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("mid0009.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(marker["ok"], serde_json::json!(true));
        assert_eq!(marker["session_id"], serde_json::json!("sess0001"));
        // The marker binds the FIRST create's payload — the digest of the
        // original record, not the swapped-in divergent body.
        let original_digest = session_request_digest(
            &serde_json::from_str::<SpooledSessionRequest>(&spool_record_json(&[])).unwrap(),
        );
        assert_eq!(marker["request_digest"], serde_json::json!(original_digest));

        // The second poll has nothing left: no re-queue, no second create.
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;
        assert_eq!(
            creator.inputs.lock().unwrap().len(),
            1,
            "the dropped newest body is never applied"
        );
    }

    /// Round-9 M6: the watcher-delivered opening turn carries the
    /// unattended shield — source-pinned on both ends (the pattern of
    /// engine.rs::scheduled_sends_carry_the_shield_via_set_disallowed_tools):
    /// (a) the pool's deliver path sends the shield BEFORE the turn is
    /// reserved/submitted, and (b) this feature's caller passes `true`
    /// while the attended senders (messaging tool, scheduled executor)
    /// pass `false`.
    #[test]
    fn unattended_delivery_carries_the_shield() {
        let pool_source = include_str!("../assistant/engine_pool.rs");
        let deliver_start = pool_source
            .find("fn deliver_messaging_turn")
            .expect("deliver must exist");
        let deliver_end = pool_source[deliver_start..]
            .find("\n    pub")
            .map(|offset| deliver_start + offset)
            .unwrap_or(pool_source.len());
        let deliver_body = &pool_source[deliver_start..deliver_end];
        let shield = deliver_body
            .find("engine.apply_unattended_shield().await?")
            .expect("the shield send must exist in the deliver span");
        let guarded = deliver_body
            .find("if unattended_shield {")
            .expect("the shield must be behind the flag");
        let reserve = deliver_body
            .find("let reservation = self.reserve_turn(session_id)?;")
            .expect("the turn reservation must exist");
        // Round-13 MAJOR 3: the reserve comes FIRST — a reserve_turn
        // failure must not leave the engine silenced with no mark and no
        // restore duty (the round-12 shape applied the shield before the
        // reserve, and an early `?` bypassed the tail's strip arm). The
        // reservation is held from here, so the shield op still precedes
        // this delivery's turn op on the channel (no other turn can submit
        // in between).
        assert!(
            reserve < guarded && guarded < shield,
            "the shield is applied AFTER the reservation (reserve-failure cannot leak it)"
        );
        // The engine side actually carries the unattended list.
        let engine_source = include_str!("../assistant/engine.rs");
        let method = engine_source
            .find("pub(crate) async fn apply_unattended_shield")
            .expect("the engine method must exist");
        let method_body = &engine_source[method..method + 700];
        assert!(
            method_body.contains("Op::SetDisallowedTools"),
            "the method sends the engine op"
        );
        assert!(
            method_body.contains("self.scheduled_disallowed_tools.clone()"),
            "the method carries the shield list"
        );
        // This feature's delivery passes true; the attended senders false.
        // Round-10 M1: the needle is BUILT by concatenation — the round-9
        // form asserted a literal that also matched the assert's own text
        // inside this included file, so flipping the real call to false
        // kept the suite green (mutation-proven by the review).
        let own = include_str!("mod.rs");
        let shielded_call = [
            "deliver_messaging_turn(&id, message.to_string(), ",
            "true",
            ")",
        ]
        .concat();
        assert!(
            own.contains(&shielded_call),
            "the session-creation delivery is shielded"
        );
        let unshielded_call = [
            "deliver_messaging_turn(&id, message.to_string(), ",
            "false",
            ")",
        ]
        .concat();
        assert!(
            !own.contains(&unshielded_call),
            "the session-creation delivery must not carry the unshielded flag"
        );
        let messaging = include_str!("../messaging/mod.rs");
        assert!(
            messaging.contains("deliver_messaging_turn(&to_session, text.clone(), false)"),
            "the messaging tool delivery stays attended (closure seam)"
        );
        let executor = include_str!("../scheduled/executor.rs");
        assert!(
            executor.contains("deliver_messaging_turn(target_session, message, false)"),
            "the scheduled-message delivery stays on its own review track"
        );
        // Round-12 (#657) BLOCKER 1: the MARK moves after the delivered turn
        // — the insert sits in the tail (after the send), so the delivery's
        // own pass through the shared restore seam cannot dequeue the shield
        // it just applied.
        let turn_submit = deliver_body
            .find("let reservation = self.reserve_turn(session_id)?;")
            .expect("the turn reservation must exist");
        let mark_insert = deliver_body
            .find("self.watcher_shielded_sessions")
            .expect("the delivery records the mark");
        assert!(
            turn_submit < mark_insert,
            "the mark is recorded after the turn is submitted (post-completion)"
        );
        // Round-15 MAJOR 1: the drop guard is armed right after the shield
        // op and disarmed in the tail BEFORE the mark/strip — a dropped
        // future (the 30s delivery timeout or the per-file panic) then
        // STRIPS the shield instead of stranding it unmarked on a live
        // engine. Deleting the guard (or its disarm) turns these red.
        let guard_arm = deliver_body
            .find("self.watcher_shield_drop_guard(session_id)")
            .expect("the drop guard must be armed in the deliver span");
        assert!(
            shield < guard_arm,
            "the guard arms after the shield op resolves"
        );
        let disarm = deliver_body
            .find("guard.disarm()")
            .expect("the tail must disarm the guard");
        assert!(
            guard_arm < disarm && disarm < mark_insert,
            "the guard is disarmed exactly when the tail owns the disposition"
        );
        let guard_drop_strip = include_str!("../assistant/engine_pool.rs");
        let drop_span_start = guard_drop_strip
            .find("impl Drop for WatcherShieldDropGuard")
            .expect("the guard Drop impl must exist");
        let drop_span = &guard_drop_strip[drop_span_start..drop_span_start + 900];
        assert!(
            drop_span.contains("send_ordinary_disallowed_tools"),
            "an armed drop strips the shield (the ordinary catalog)"
        );
        // Round-19 MAJOR 1: the Err arm leaves the guard ARMED — its Drop
        // detaches the idempotent strip, which survives this future being
        // dropped (the round-18 shape awaited the strip after disarming,
        // and a timeout inside that await re-entered the strand).
        let failed_arm = deliver_body
            .find("drop(shield_guard);")
            .expect("a failed delivery leaves the armed guard's Drop to strip");
        assert!(
            mark_insert < failed_arm,
            "the armed-drop strip follows the record arm in the tail"
        );
        assert!(
            !deliver_body[mark_insert..].contains("send_ordinary_disallowed_tools"),
            "the Err arm must not await the strip after disarming"
        );
    }

    /// Round-10 M2(c): the round-8 M2 tmp-gate finally has a pin — a
    /// DIRECTORY planted at the `.tmp` path (the portable non-regular
    /// file) must make write_done_marker refuse instead of writing.
    #[tokio::test]
    async fn marker_tmp_nonregular_refusal_is_pinned() {
        let _home = TempHome::new();
        std::fs::create_dir_all(done_dir()).unwrap();
        std::fs::create_dir_all(done_dir().join("planted.json.tmp")).unwrap();
        let marker = done_dir().join("planted.json");
        let result = write_done_marker(&marker, &serde_json::json!({"ok": true}));
        // Round-25 M1: the tmp write goes through the platform's
        // open_private_tmp_write_file (O_NOFOLLOW|O_NONBLOCK + post-open
        // regular-file refusal). A planted directory fails at OS open
        // (EISDIR/Access Denied) or at the post-open check — either way
        // the call must error, not silently write through.
        assert!(
            result.is_err(),
            "a non-regular tmp path must refuse: {:?}",
            result
        );
    }

    /// Round-10 M2(b), rewritten for the round-25 MAJOR-1 mechanism: the
    /// entry read routes through the platform's gated primitive, so the pin
    /// asserts (a) the helper call sits in process_spool_file's production
    /// span and (b) the helper delegates to read_private_data_file_bytes_
    /// within (O_NOFOLLOW|O_NONBLOCK + post-open regular-file refusal +
    /// take-bounded read) — replacing with a bare read turns this red.
    #[test]
    fn post_read_growth_recheck_is_pinned() {
        let source = include_str!("mod.rs");
        let test_module = source.find("mod tests").expect("test module");
        let production = &source[..test_module];
        let entry_start = production
            .find("async fn process_spool_file")
            .expect("the entry fn must exist");
        let entry_end = production[entry_start..]
            .find("\n/// ")
            .map(|offset| entry_start + offset)
            .unwrap_or(production.len());
        assert!(
            production[entry_start..entry_end].contains("read_spool_bounded(path)"),
            "the entry read must go through the gated+bounded helper"
        );
        let helper_start = production
            .find("fn read_spool_bounded")
            .expect("the helper must exist");
        let helper_body = &production[helper_start..helper_start + 2000];
        assert!(
            helper_body.contains("read_private_data_file_bytes_within"),
            "the helper must delegate to the platform gated read"
        );
        assert!(
            helper_body.contains("symlink_metadata"),
            "the helper must carry the pre-check regular-file (FIFO) gate"
        );
    }

    /// Round-11 M1 / round-12 (#657) BLOCKER 1 + MAJOR 1 rewrite: the
    /// shield LIFETIME is pinned at the PRODUCTION seams. The round-11 form
    /// pinned the restore inside `send_user_message` — which is
    /// `#[cfg(any(feature = "benchmark-hooks", test))]` — so the pin passed
    /// while production builds had NO restore at all (the exact escape the
    /// round-12 review called out; a source needle cannot see a `#[cfg]`
    /// compile-out). The new needles point at the seams every attended
    /// send converges on:
    /// - the restore helper is called from `send_reserved_user_message`
    ///   AND `edit_last_turn_reserved` (after their reservations), and is
    ///   NOT called from the cfg-gated `send_user_message`;
    /// - the restore re-shapes the catalog from
    ///   `compute_disallowed_tools()` — an empty base re-exposed
    ///   tool-policy-disabled tools for the engine's remaining lifetime
    ///   (MAJOR 1);
    /// - the delivery records the mark post-completion and a failed
    ///   delivery strips the shield (pinned by
    ///   unattended_delivery_carries_the_shield).
    #[test]
    fn shield_is_marked_on_delivery_and_restored_on_first_attended_send() {
        let pool_source = include_str!("../assistant/engine_pool.rs");
        let restore_call = "self.restore_watcher_shield_if_marked(session_id).await;";

        // The cfg-gated benchmark path must NOT be where the restore lives.
        let gated_start = pool_source
            .find("#[cfg(any(feature = \"benchmark-hooks\", test))]\n    pub async fn send_user_message")
            .expect("the benchmark-gated send_user_message must exist");
        let gated_end = pool_source[gated_start..]
            .find("\n    ///")
            .map(|offset| gated_start + offset)
            .unwrap_or(pool_source.len());
        assert!(
            !pool_source[gated_start..gated_end].contains(restore_call),
            "the restore must not live in the cfg-gated path (the round-11 escape)"
        );

        // Both production attended seams call the restore, AFTER the
        // reservation is acquired (a busy-loser attended send must not
        // strip a still-pending opening turn's shield).
        for seam in ["send_reserved_user_message", "edit_last_turn_reserved"] {
            let start = pool_source
                .find(&format!("async fn {seam}"))
                .unwrap_or_else(|| panic!("the {seam} seam must exist"));
            let end = pool_source[start..]
                .find("\n    pub")
                .map(|offset| start + offset)
                .unwrap_or(pool_source.len());
            let body = &pool_source[start..end];
            let restore = body
                .find(restore_call)
                .unwrap_or_else(|| panic!("the {seam} seam must call the shield restore"));
            let ensured = body
                .find("reservation.ensure_active()?;")
                .unwrap_or_else(|| panic!("the {seam} seam must ensure the reservation"));
            assert!(
                ensured < restore,
                "the {seam} seam restores only after the reservation is acquired"
            );
        }

        // MAJOR 1: the restore shapes the ordinary catalog from the GLOBAL
        // tool-policy base, and clears the mark.
        let helpers_start = pool_source
            .find("async fn send_ordinary_disallowed_tools")
            .expect("the ordinary-catalog helper must exist");
        let helpers_end = pool_source
            .find("/// Deliver a cross-session message as a new turn")
            .expect("the deliver fn follows the helpers");
        let helpers = &pool_source[helpers_start..helpers_end];
        assert!(
            helpers.contains("shape_disallowed_tools(session_id, self.compute_disallowed_tools())"),
            "the restore shapes the catalog from the GLOBAL tool-policy base"
        );
        assert!(
            helpers.contains("self.watcher_shielded_sessions.lock().remove(session_id)"),
            "the restore takes the mark"
        );
    }

    /// Round-9 M5: the budget-stickiness fix finally has an effective pin —
    /// with failed/ squatted by a regular file every quarantine rename
    /// fails, so the post-rename `!path.exists()` guard must NOT clear the
    /// budget: the second poll re-poisons without a further create.
    /// Deleting either post-rename guard turns this red (the round-8
    /// comment claimed the pair already pinned this; now one actually
    /// does).
    #[tokio::test]
    async fn exhausted_budget_sticks_when_quarantine_rename_fails() {
        let _home = TempHome::new();
        // Round-13 MAJOR 4c: the store materializes the requester's
        // execution root so the audit-dedup assertions below can count lines.
        let store = store_with_requester();
        let creator = StubCreator::always_ok();
        let notifier = StubNotifier::default();
        let path = write_spool("ggg7777", &spool_record_json(&[]));
        // Squat the quarantine directory path with a regular file:
        // create_dir_all and every rename into failed/ fail.
        std::fs::write(failed_dir(), b"not a directory").unwrap();
        let mut retries = RetryState::default();
        retries
            .attempts
            .insert("ggg7777.json".to_string(), MAX_CREATE_ATTEMPTS);
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;
        assert_eq!(
            creator.inputs.lock().unwrap().len(),
            0,
            "first poll: the exhausted budget gates the create"
        );
        assert!(path.exists(), "the rename failed; the record stays");
        assert_eq!(
            retries.attempts.get("ggg7777.json").copied(),
            Some(MAX_CREATE_ATTEMPTS),
            "the budget was not recycled by the failing rename"
        );
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;
        assert_eq!(
            creator.inputs.lock().unwrap().len(),
            0,
            "second poll: still gated — the budget does not re-buy"
        );
        // Round-13 MAJOR 4c: the exhausted arm's audit DEDUP is observable —
        // with failed/ squatted the streak is sticky, and the compound must
        // produce exactly ONE session_create_failed line across polls
        // (deleting the poison_audited gate turns this red).
        assert_eq!(
            count_audit_kinds(&store, "reqsrc01", "session_create_failed"),
            1,
            "one audit line per sticky exhausted streak"
        );
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;
        assert_eq!(
            count_audit_kinds(&store, "reqsrc01", "session_create_failed"),
            1,
            "still one line after another sticky poll"
        );
    }

    /// Round-12 minor 3: a readable ok:true receipt bound to a DIFFERENT
    /// payload is terminal-mismatch — the record is dropped WITHOUT a second
    /// create and the receipt survives byte-for-byte (the pre-apply remove
    /// used to destroy the server's answer). A digest-LESS ok:true marker is
    /// NOT a receipt and must not swallow the queue (the apply proceeds).
    #[tokio::test]
    async fn digest_mismatched_receipt_drops_record_and_survives() {
        let _home = TempHome::new();
        let store = store_with_requester();
        let creator = StubCreator::always_ok();
        let notifier = StubNotifier::default();
        let mut retries = RetryState::default();

        // A completed create for a DIFFERENT payload of the same key.
        let other = spool_record_json(&[("title", serde_json::json!("另一个会话"))]);
        let other_digest =
            session_request_digest(&serde_json::from_str::<SpooledSessionRequest>(&other).unwrap());
        std::fs::create_dir_all(done_dir()).unwrap();
        let marker = done_dir().join("mmm3333.json");
        std::fs::write(
            &marker,
            serde_json::json!({
                "ok": true,
                "session_id": "sess_other",
                "title": "另一个会话",
                "request_digest": other_digest,
            })
            .to_string(),
        )
        .unwrap();
        let receipt_bytes = std::fs::read(&marker).unwrap();

        let path = write_spool("mmm3333", &spool_record_json(&[]));
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;

        assert!(!path.exists(), "the record is dropped without applying");
        assert_eq!(
            creator.inputs.lock().unwrap().len(),
            0,
            "no second create for a completed key"
        );
        assert_eq!(
            std::fs::read(&marker).unwrap(),
            receipt_bytes,
            "the receipt is neither overwritten nor deleted"
        );
        assert!(
            !failed_dir().join("mmm3333.json").exists(),
            "the record is not quarantined — it is a Done disposition"
        );
        // Round-13 minor 3: the terminal-mismatch drop leaves its audit
        // trace (the same once-per-stem invariant the suppress arm honors).
        assert_eq!(
            count_audit_lines(&store, "reqsrc01", "dropped_terminal_mismatch"),
            1,
            "the mismatch drop is audited once"
        );
    }

    #[tokio::test]
    async fn digest_less_ok_marker_does_not_swallow_the_queue() {
        let _home = TempHome::new();
        let store = store_with_requester();
        let creator = StubCreator::always_ok();
        let notifier = StubNotifier::default();
        let mut retries = RetryState::default();
        std::fs::create_dir_all(done_dir()).unwrap();
        let marker = done_dir().join("nnn4444.json");
        // Forged / pre-M1 shape: ok:true without a digest binding.
        std::fs::write(&marker, serde_json::json!({"ok": true}).to_string()).unwrap();

        let path = write_spool("nnn4444", &spool_record_json(&[]));
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;

        assert_eq!(
            creator.inputs.lock().unwrap().len(),
            1,
            "an unbound ok:true marker is not a receipt — the apply proceeds"
        );
        let applied: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&marker).unwrap()).unwrap();
        assert_eq!(applied["ok"], serde_json::json!(true));
        assert_eq!(applied["session_id"], serde_json::json!("sess0001"));
        assert!(applied["request_digest"].is_string(), "re-bound on apply");
        assert!(!path.exists());
    }

    /// Round-12 minor 4: the suppression probe runs BEFORE validation — a
    /// leftover record that no longer validates but carries a genuine
    /// digest-bound ok:true receipt takes the suppress arm (no create, no
    /// poison, nothing quarantined), not the Poison arm's false
    /// session_create_failed line.
    #[tokio::test]
    async fn stale_invalid_record_with_valid_receipt_suppresses() {
        let _home = TempHome::new();
        let store = store_with_requester();
        let creator = StubCreator::always_ok();
        let notifier = StubNotifier::default();
        let mut retries = RetryState::default();

        let invalid = spool_record_json(&[("title", serde_json::json!("x".repeat(201)))]);
        let digest = session_request_digest(
            &serde_json::from_str::<SpooledSessionRequest>(&invalid).unwrap(),
        );
        std::fs::create_dir_all(done_dir()).unwrap();
        std::fs::write(
            done_dir().join("sss6666.json"),
            serde_json::json!({
                "ok": true,
                "session_id": "sess_earlier",
                "request_digest": digest,
            })
            .to_string(),
        )
        .unwrap();
        let path = write_spool("sss6666", &invalid);
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;

        assert!(!path.exists(), "suppressed: the leftover record is dropped");
        assert_eq!(creator.inputs.lock().unwrap().len(), 0, "no create");
        assert!(
            !failed_dir().join("sss6666.json").exists(),
            "not poisoned — the key already has its outcome"
        );
        // Round-15 minor 6: the suppress arm's audit line is asserted — the
        // already_created_skip trace fires once for the suppressed key.
        assert_eq!(
            count_audit_lines(&store, "reqsrc01", "already_created_skip"),
            1,
            "the suppress arm leaves its once-per-stem audit trace"
        );
    }

    /// Round-12 minor 1: a quarantined Retry-exhausted streak clears its
    /// audit dedup together with its budget — a same-key second failure
    /// episode is audited again, and the dedup set does not grow with dead
    /// names.
    #[tokio::test]
    async fn exhausted_streak_cleanup_rearms_the_audit_dedup() {
        let _home = TempHome::new();
        let store = store_with_requester();
        let creator = StubCreator::with_outcomes(vec![]);
        let notifier = StubNotifier::default();
        let mut retries = RetryState::default();
        // A stale dedup entry from an earlier episode of the same key.
        retries.poison_audited.insert("ttt8888.json".to_string());
        retries
            .attempts
            .insert("ttt8888.json".to_string(), MAX_CREATE_ATTEMPTS);
        let path = write_spool("ttt8888", &spool_record_json(&[]));
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;

        assert!(!path.exists(), "quarantined (the rename succeeds)");
        assert!(
            !retries.poison_audited.contains("ttt8888.json"),
            "the audit dedup is cleared with the file"
        );
        assert!(
            !retries.attempts.contains_key("ttt8888.json"),
            "the budget is cleared with the file"
        );
    }

    /// Round-12 minor 2: the per-file panic arm dedups its audit line like
    /// every other terminal arm, and clears the dedup when the quarantine
    /// moves the file (streak end).
    #[tokio::test]
    async fn panicking_record_quarantines_and_clears_the_dedup() {
        struct PanickingCreator;
        impl SessionCreator for PanickingCreator {
            async fn create(
                &self,
                _input: CreateSessionInput,
            ) -> std::result::Result<CreatedSession, String> {
                panic!("deterministic panic");
            }
        }
        let _home = TempHome::new();
        let store = store_with_requester();
        let notifier = StubNotifier::default();
        let mut retries = RetryState::default();
        let path = write_spool("ppp9999", &spool_record_json(&[]));
        for _ in 0..MAX_CREATE_ATTEMPTS {
            process_pending_spool(&PanickingCreator, &store, &notifier, &mut retries).await;
        }
        assert!(!path.exists(), "quarantined after MAX panicking attempts");
        assert!(failed_dir().join("ppp9999.json").exists());
        assert!(
            retries.poison_audited.is_empty(),
            "the dedup is cleared with the file"
        );
    }

    /// Round-17 MAJOR 1 / round-18 MAJOR 2: the budget gate's semantics are
    /// pinned by LITERAL mixed-id fixtures, not by seeding through the gate
    /// itself (the round-17 form was its own oracle: 4 of 5 single-token
    /// mutations stayed green and the realistic off-by-one hung the suite
    /// while silently restoring the eviction behavior). Each assert here is
    /// one mutation away from red: the headless-prefix exclusion, the cap
    /// constant, and the fail-closed listing arm are all observed.
    #[tokio::test]
    async fn budget_gate_counts_like_the_sweep() {
        use deepseek_tui::session_manager::create_saved_session_with_id_and_mode;
        let _home = TempHome::new();
        let store = store_with_requester();
        let seed = |id: &str| {
            let session = create_saved_session_with_id_and_mode(
                id.to_string(),
                &[],
                "/model",
                &std::env::temp_dir(),
                0,
                None,
                None,
            );
            store.save_session_atomic(&session).unwrap();
        };
        for index in 0..45 {
            seed(&format!("budgetgate{index:04}"));
        }
        // Headless ids ride their OWN budget — dropping this exclusion from
        // the gate turns the next assert red.
        for index in 0..5 {
            seed(&format!("agentic_4242_{index}"));
        }
        assert!(
            !store.plain_chat_budget_full().unwrap(),
            "45 plain + 5 headless: headless does not consume the chat budget"
        );
        seed("budgetgate_zzz46");
        seed("budgetgate_zzz47");
        seed("budgetgate_zzz48");
        seed("budgetgate_zzz49");
        assert!(
            !store.plain_chat_budget_full().unwrap(),
            "49 plain: one slot left"
        );
        seed("budgetgate_fff");
        assert!(store.plain_chat_budget_full().unwrap(), "50 plain: at cap");
    }

    /// Round-19 minor (the reviewer's most-wanted pin): the PINNED
    /// exclusion conjunct gets its own red — 45 plain + 5 pinned ⇒ NOT full
    /// (pinned sessions sit outside the counted set exactly as the sweep
    /// exempts them); the round-17 pinned-inclusive count turns this assert
    /// red (50 counted ⇒ full).
    #[tokio::test]
    async fn budget_gate_exempts_pinned_sessions() {
        use deepseek_tui::session_manager::create_saved_session_with_id_and_mode;
        let _home = TempHome::new();
        let store = store_with_requester();
        let pin_file = crate::platform::paths::sessions_root().join("_pinned_sessions.json");
        let pins: Vec<serde_json::Value> = (0..5)
            .map(|index| {
                serde_json::json!({
                    "id": format!("budgetgatepin{index:04}"),
                    "pinned_at": "2026-10-13T00:00:00Z",
                })
            })
            .collect();
        std::fs::write(&pin_file, serde_json::to_string(&pins).unwrap()).unwrap();
        let seed = |id: &str| {
            let session = create_saved_session_with_id_and_mode(
                id.to_string(),
                &[],
                "/model",
                &std::env::temp_dir(),
                0,
                None,
                None,
            );
            store.save_session_atomic(&session).unwrap();
        };
        for index in 0..45 {
            seed(&format!("budgetgateplain{index:04}"));
        }
        for index in 0..5 {
            seed(&format!("budgetgatepin{index:04}"));
        }
        assert!(
            !store.plain_chat_budget_full().unwrap(),
            "45 unpinned plain + 5 pinned: pinned sessions are exempt"
        );
    }

    /// Round-18 MAJOR 2: a listing failure is FAIL-CLOSED — the gate
    /// surfaces the error (the watcher maps it to a retry), it does not
    /// mask it as "budget fine". Squatting the sessions directory with a
    /// regular file makes list_sessions_cached fail.
    #[tokio::test]
    async fn budget_gate_listing_failure_is_fail_closed() {
        let _home = TempHome::new();
        let store = store_with_requester();
        assert!(
            !store.plain_chat_budget_full().unwrap(),
            "empty store: headroom"
        );
        let sessions_dir = crate::platform::paths::pinvou3_home().join("sessions");
        std::fs::remove_dir_all(&sessions_dir).unwrap();
        std::fs::write(&sessions_dir, b"not a directory").unwrap();
        assert!(
            store.plain_chat_budget_full().is_err(),
            "a listing failure is surfaced for the fail-closed refusal"
        );
    }

    /// Round-17 MAJOR 1: at the plain-chat retention cap the watcher
    /// refuses with the honest budget-full error instead of letting the
    /// save evict a pre-existing conversation — no create runs, the record
    /// poisons, and the marker names the budget (and the marker's advice
    /// "delete or pin" is REAL: the gate exempts pinned sessions exactly
    /// like the sweep, so a pin frees a slot).
    #[tokio::test]
    async fn chat_budget_full_refuses_instead_of_evicting() {
        use deepseek_tui::session_manager::create_saved_session_with_id_and_mode;
        let _home = TempHome::new();
        let store = store_with_requester();
        for index in 0..50 {
            let session = create_saved_session_with_id_and_mode(
                format!("budgetgatefull{index:04}"),
                &[],
                "/model",
                &std::env::temp_dir(),
                0,
                None,
                None,
            );
            store.save_session_atomic(&session).unwrap();
        }
        assert!(
            store.plain_chat_budget_full().unwrap(),
            "the cap gate is live"
        );
        let creator = StubCreator::always_ok();
        let notifier = StubNotifier::default();
        let mut retries = RetryState::default();
        let path = write_spool("bbb2222", &spool_record_json(&[]));
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;

        assert_eq!(
            creator.inputs.lock().unwrap().len(),
            0,
            "no create runs at the cap"
        );
        assert!(!path.exists(), "the record is quarantined (poisoned)");
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("bbb2222.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(marker["ok"], serde_json::json!(false));
        assert!(
            marker["error"]
                .as_str()
                .unwrap()
                .contains("session budget full"),
            "the honest budget-full refusal: {marker}"
        );
    }

    /// Round-17 MAJOR 1: below the cap the create proceeds normally — the
    /// budget gate only fires at the cap, never before.
    #[tokio::test]
    async fn chat_budget_headroom_allows_the_create() {
        use deepseek_tui::session_manager::create_saved_session_with_id_and_mode;
        let _home = TempHome::new();
        let store = store_with_requester();
        for index in 0..49 {
            let session = create_saved_session_with_id_and_mode(
                format!("budgetgatefree{index:04}"),
                &[],
                "/model",
                &std::env::temp_dir(),
                0,
                None,
                None,
            );
            store.save_session_atomic(&session).unwrap();
        }
        assert!(!store.plain_chat_budget_full().unwrap(), "headroom left");
        let creator = StubCreator::always_ok();
        let notifier = StubNotifier::default();
        let mut retries = RetryState::default();
        let path = write_spool("ccc3333", &spool_record_json(&[]));
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;

        assert_eq!(
            creator.inputs.lock().unwrap().len(),
            1,
            "the create proceeds with headroom"
        );
        assert!(!path.exists(), "the record is consumed");
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(done_dir().join("ccc3333.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(marker["ok"], serde_json::json!(true));
    }

    /// Round-24 M1: a directory planted at the FINAL marker path defeats
    /// every gate (probes treat it as "not a receipt", remove_file is a
    /// silent no-op, rename fails, write_failure_marker dies) — the record
    /// must take the terminal Poison arm instead of applying into a wedged
    /// marker path. Deleting the symlink_metadata gate turns this red.
    #[tokio::test]
    async fn directory_at_marker_path_is_terminal_not_wedged() {
        let _home = TempHome::new();
        let store = store_with_requester();
        let creator = StubCreator::always_ok();
        let notifier = StubNotifier::default();
        let mut retries = RetryState::default();
        let path = write_spool("zzz8888", &spool_record_json(&[]));
        std::fs::create_dir_all(done_dir()).unwrap();
        let marker = done_dir().join("zzz8888.json");
        std::fs::create_dir_all(&marker).unwrap();
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;

        assert_eq!(
            creator.inputs.lock().unwrap().len(),
            0,
            "no create runs into a wedged marker path"
        );
        assert!(!path.exists(), "the record is quarantined (terminal)");
        assert!(
            failed_dir().join("zzz8888.json").exists(),
            "quarantined into failed/"
        );
    }

    /// Round-23 M3: a materially-different body at the same deterministic
    /// name resets the exhausted budget — the sticky-quarantine corner no
    /// longer permanently closes the gate (messaging's
    /// replaced-since-first-read shape). The digest recording rides the
    /// Retry arm; the reset fires at the exhausted gate.
    #[tokio::test]
    async fn replaced_body_gets_a_fresh_budget_episode() {
        let _home = TempHome::new();
        let store = store_with_requester();
        let creator = StubCreator::with_outcomes(vec![
            Err("transient".to_string()),
            Err("transient".to_string()),
            Err("transient".to_string()),
        ]);
        let notifier = StubNotifier::default();
        let mut retries = RetryState::default();
        let _ = write_spool("xxx7777", &spool_record_json(&[]));
        for _ in 0..MAX_CREATE_ATTEMPTS {
            process_pending_spool(&creator, &store, &notifier, &mut retries).await;
        }
        assert_eq!(creator.inputs.lock().unwrap().len(), 3, "budget exhausted");
        // The server's same-key retry os.replace's a fresh (different) body
        // at the same deterministic name.
        let _replaced = write_spool(
            "xxx7777",
            &spool_record_json(&[("title", serde_json::json!("替换后的标题"))]),
        );
        process_pending_spool(&creator, &store, &notifier, &mut retries).await;
        assert_eq!(
            creator.inputs.lock().unwrap().len(),
            4,
            "the replaced body gets a fresh attempt episode"
        );
    }

    /// Round-13 MAJOR 4c: the panic arm's audit DEDUP is observable — with
    /// failed/ squatted (sticky rename) the arm re-fires every poll past
    /// MAX, and the compound fixture must produce exactly ONE audit line
    /// (deleting the `poison_audited` gate turns this red: 4 polls past MAX
    /// would append one line per poll).
    #[tokio::test]
    async fn panic_arm_audits_once_per_sticky_streak() {
        struct PanickingCreator;
        impl SessionCreator for PanickingCreator {
            async fn create(
                &self,
                _input: CreateSessionInput,
            ) -> std::result::Result<CreatedSession, String> {
                panic!("deterministic panic");
            }
        }
        let _home = TempHome::new();
        let store = store_with_requester();
        let notifier = StubNotifier::default();
        let mut retries = RetryState::default();
        let path = write_spool("qqq0001", &spool_record_json(&[]));
        // Squat the quarantine directory path: every rename fails, so the
        // record stays and the panic arm re-fires each poll.
        std::fs::write(failed_dir(), b"not a directory").unwrap();
        for _ in 0..MAX_CREATE_ATTEMPTS + 1 {
            process_pending_spool(&PanickingCreator, &store, &notifier, &mut retries).await;
        }
        assert!(path.exists(), "sticky: the rename keeps failing");
        assert_eq!(
            count_audit_kinds(&store, "reqsrc01", "session_create_failed"),
            1,
            "the panic arm's audit is deduped across the sticky streak"
        );
        process_pending_spool(&PanickingCreator, &store, &notifier, &mut retries).await;
        assert_eq!(
            count_audit_kinds(&store, "reqsrc01", "session_create_failed"),
            1,
            "still one line after another sticky poll"
        );
    }

    /// Round-13 MAJOR 4b: the round-12 write_failure_marker receipt
    /// protection has a DIRECT pin — a digest-bound ok:true receipt is
    /// never clobbered by a later failure marker (deleting the refusal arm
    /// turns this red).
    #[tokio::test]
    async fn failure_marker_never_clobbers_a_digest_bound_receipt() {
        let _home = TempHome::new();
        std::fs::create_dir_all(done_dir()).unwrap();
        let marker = done_dir().join("rrr1212.json");
        std::fs::write(
            &marker,
            serde_json::json!({
                "ok": true,
                "session_id": "sess0001",
                "request_digest": "d",
            })
            .to_string(),
        )
        .unwrap();
        let before = std::fs::read(&marker).unwrap();
        write_failure_marker("rrr1212", &anyhow::anyhow!("boom"));
        assert_eq!(
            std::fs::read(&marker).unwrap(),
            before,
            "the readable digest-bound receipt survives byte-for-byte"
        );
    }

    /// Round-13 MAJOR 4b: the receipt DEFINITION is pinned too — a
    /// digest-LESS ok:true marker is not a receipt and IS replaced by the
    /// failure marker (the watcher's retry path depends on this).
    #[tokio::test]
    async fn failure_marker_overwrites_a_digest_less_ok_marker() {
        let _home = TempHome::new();
        std::fs::create_dir_all(done_dir()).unwrap();
        let marker = done_dir().join("sss1313.json");
        std::fs::write(&marker, serde_json::json!({"ok": true}).to_string()).unwrap();
        write_failure_marker("sss1313", &anyhow::anyhow!("boom"));
        let replaced: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&marker).unwrap()).unwrap();
        assert_eq!(replaced["ok"], serde_json::json!(false));
        assert_eq!(replaced["error"], serde_json::json!("boom"));
    }

    /// Round-15 minor 6 / round-18 minor 8: the oversize refusal is
    /// attributed to the INNER take() bound (`read_marker_capped`), which
    /// refuses the 66 KiB receipt before any parse — the outer stat-cap
    /// conjunct alone is NOT mutation-red for this fixture (the inner bound
    /// masks it). Deleting the inner bound's take lets the read through to
    /// the parse, the receipt survives, and this goes red.
    #[tokio::test]
    async fn failure_marker_overrides_an_oversize_receipt() {
        let _home = TempHome::new();
        std::fs::create_dir_all(done_dir()).unwrap();
        let marker = done_dir().join("ooo1515.json");
        let oversize = format!(
            "{{\"ok\": true, \"request_digest\": \"d\", \"pad\": \"{}\"}}",
            "x".repeat(66 * 1024),
        );
        std::fs::write(&marker, oversize).unwrap();
        write_failure_marker("ooo1515", &anyhow::anyhow!("boom"));
        let replaced: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&marker).unwrap()).unwrap();
        assert_eq!(
            replaced["ok"],
            serde_json::json!(false),
            "an oversize marker is beyond the receipt cap, not a receipt"
        );
    }

    /// Round-15 minor 6 / round-18 minor 8: the terminal-mismatch probe's
    /// regular-file gate and oversize refusal are pinned — a FIFO at the
    /// marker path is refused without blocking (deleting the gate blocks
    /// the open and this times out red), and the oversize receipt is
    /// refused by the inner `read_marker_capped` take() bound before any
    /// parse (attributed there per the round-18 masking note).
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_mismatch_probe_gates_fifo_and_oversize() {
        use std::time::Duration;
        let _home = TempHome::new();
        std::fs::create_dir_all(done_dir()).unwrap();

        // Oversize: a 66 KiB receipt-shaped marker with a DIFFERENT digest —
        // only the cap can refuse it (the digest match would otherwise
        // report a mismatch).
        let oversize = format!(
            "{{\"ok\": true, \"request_digest\": \"other\", \"pad\": \"{}\"}}",
            "x".repeat(66 * 1024),
        );
        let oversize_marker = done_dir().join("www1616.json");
        std::fs::write(&oversize_marker, oversize).unwrap();
        assert!(
            !marker_is_terminal_mismatch(&oversize_marker, "expected"),
            "an oversize receipt is refused by the cap, not read"
        );

        // FIFO: mkfifo at the marker path — the gate must refuse before any
        // open can block.
        let fifo_marker = done_dir().join("xxx1717.json");
        let _ = std::fs::remove_file(&fifo_marker);
        let created = std::process::Command::new("mkfifo")
            .arg(&fifo_marker)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !created {
            return;
        }
        let handle = {
            let fifo = fifo_marker.clone();
            tokio::task::spawn_blocking(move || marker_is_terminal_mismatch(&fifo, "expected"))
        };
        let refused = tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("the mismatch probe's gate must not block on a FIFO")
            .unwrap();
        assert!(!refused, "a FIFO at the marker path is never a receipt");
        let _ = std::fs::remove_file(&fifo_marker);
    }

    /// Round-15 minor 1: behavioral pin for the ENTRY-read FIFO gate — a
    /// FIFO at the spool path returns NotRegular without blocking. The
    /// round-13 form needle-pinned the gate only (the reviewer's live
    /// mutation run neutered it with the whole suite green); deleting the
    /// `symlink_metadata` gate here blocks `File::open` and this times out
    /// red.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn entry_fifo_gate_refuses_without_blocking() {
        use std::time::Duration;
        let _home = TempHome::new();
        std::fs::create_dir_all(spool_root()).unwrap();
        let path = spool_root().join("fifo6666.json");
        let _ = std::fs::remove_file(&path);
        let created = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !created {
            return;
        }
        let handle = {
            let fifo = path.clone();
            tokio::task::spawn_blocking(move || {
                matches!(read_spool_bounded(&fifo), SpoolRead::NotRegular)
            })
        };
        let refused = tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("the entry FIFO gate must not block")
            .unwrap();
        assert!(refused, "a FIFO at the spool path is refused as NotRegular");
        let _ = std::fs::remove_file(&path);
    }

    /// Round-13 MAJOR 4b: the FIFO half of the write_failure_marker gate —
    /// a planted FIFO at the model-influenceable marker stem must be
    /// SKIPPED (no blocking read), not wedged. Runs the sync fn on a
    /// blocking thread under a timeout: deleting the `symlink_metadata`
    /// gate makes `fs::read` block forever and this times out red.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failure_marker_skips_a_planted_fifo_marker() {
        use std::time::Duration;
        let _home = TempHome::new();
        std::fs::create_dir_all(done_dir()).unwrap();
        let marker = done_dir().join("ttt1414.json");
        let _ = std::fs::remove_file(&marker);
        // `std::os::unix::fs::mkfifo` is nightly-only (unix_mkfifo): shell
        // out to the system mkfifo(1), the twin of the python suite's
        // os.mkfifo. An environment without the binary has nothing to pin.
        let created = std::process::Command::new("mkfifo")
            .arg(&marker)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !created {
            return;
        }
        let handle = {
            let fifo = marker.clone();
            tokio::task::spawn_blocking(move || {
                write_failure_marker("ttt1414", &anyhow::anyhow!("boom"));
                fifo.extension().is_some_and(|ext| ext == "json")
            })
        };
        // The PIN is the non-block itself: the ungated form blocks fs::read
        // on the FIFO forever and this times out red. (Round-13 CI note:
        // do NOT assert the marker's end state — POSIX rename atomically
        // REPLACES a FIFO destination, so write_done_marker legitimately
        // overwrites the planted FIFO with the failure marker after the
        // gate skipped the read; that is the write path's business.)
        let _ = tokio::time::timeout(Duration::from_secs(10), handle)
            .await
            .expect("the gated read must not block on a planted FIFO")
            .unwrap();
        let _ = std::fs::remove_file(&marker);
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

    /// Review round-3 M4 / round-21 minor 4e: the POSITIVE arm — a saved
    /// model id resolves to that model's wire name and carries the id
    /// through (the pair the creator records into the session).
    #[test]
    fn resolve_creation_model_resolves_a_saved_model_id() {
        let _home = TempHome::new();
        let saved: crate::platform::prefs::SavedModel = serde_json::from_value(serde_json::json!({
            "id": "saved-model-1",
            "name": "Saved One",
            "preset": "deepseek",
            "model": "deepseek-v4-pro",
            "base_url": "https://api.deepseek.com"
        }))
        .unwrap();
        crate::platform::prefs::UserPrefs::update_transaction(|prefs| {
            prefs.advanced.saved_models.push(saved);
            Ok(())
        })
        .unwrap();
        let default = ("app-default-model".to_string(), None);
        let (model, model_id) = resolve_creation_model(default, Some("saved-model-1")).unwrap();
        assert_eq!(model, "deepseek-v4-pro");
        assert_eq!(model_id.as_deref(), Some("saved-model-1"));
    }

    /// Review round-3 M4: the unknown-model rejection is the live creator's
    /// fail-fast against sessions pinned to nonexistent saved models —
    /// pinned here so deleting it turns a test red (the workspace-bind path
    /// is covered by sessions::tests' validator suite).
    #[test]
    fn resolve_creation_model_rejects_unknown_ids_and_defaults_on_blank() {
        let _home = TempHome::new();
        let default = ("app-default-model".to_string(), None);
        // Unknown id under a fresh home (no saved models): rejected with the
        // named id, never silently defaulted.
        let error = resolve_creation_model(default.clone(), Some("no-such-model")).unwrap_err();
        assert!(
            error.contains("no-such-model") && error.contains("does not match a saved model"),
            "{error}"
        );
        // Blank / whitespace / None all mean "app default".
        for blank in [Some("   "), Some(""), None] {
            let (model, model_id) = resolve_creation_model(default.clone(), blank).unwrap();
            assert_eq!(model, "app-default-model");
            assert!(model_id.is_none());
        }
    }

    /// Round-23 M2: headless `agentic_` sessions are the fourth unattended
    /// family — a prompt-injected headless run must not delegate session
    /// creation. Both layers reject (server side pinned in python; this
    /// pins the watcher's defense-in-depth re-check, lowercase AND the
    /// forged mixed-case form).
    #[test]
    fn agentic_requester_is_rejected_as_isolated() {
        let err = |id: &str| {
            check_sender_session_id(Some(&id.to_string()))
                .unwrap_err()
                .to_string()
        };
        for id in ["agentic_4242_0", "AGENTIC_x"] {
            let e = err(id);
            assert!(
                e.contains("isolated session"),
                "{id} must be rejected as isolated: {e}"
            );
        }
        // A legitimate plain id is untouched.
        assert!(check_sender_session_id(Some(&"reqsrc01".to_string())).is_ok());
    }
}
