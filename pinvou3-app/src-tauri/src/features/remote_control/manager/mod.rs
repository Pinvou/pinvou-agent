mod persistence;
mod rpc;
mod transfer;
mod workspace_grants;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter};

use super::platform;
use super::protocol::{
    PROTOCOL_VERSION, WebAccessConfig, WebAccessInfo, WebAccessStatus, WebAccessStatusKind,
};
use super::relay_client::{self, RelayInbound, RelayOutbound, RelaySender};
use crate::platform::paths;

// Host filesystem browsing (`list_host_files` / `read_artifact_chunk` /
// `ArtifactChunk` / `HostFileListing`) lives in `super::file_access`, extracted
// upstream by the file-access boundary split. This module no longer re-exports
// that surface; callers reach it via `remote_control::file_access`.

// Persistent config / RPC ledger / pending revocation / relay settings helpers
// are pure and stateless; the facade calls them by name.
#[cfg(test)]
use persistence::atomic_write_private_json;
use persistence::{
    PENDING_REVOCATION_QUEUE_FULL_PREFIX, acquire_process_lock, apply_runtime_relay_override,
    clip_pending_endpoint_id_for_log, config_path, eligible_pending_revocations, fresh_config,
    load_config, load_config_from, load_or_initialize_rpc_ledger,
    load_pending_revocations_for_replay, new_stream_epoch, pairing_info, pending_revocation_key,
    persist_config, persist_pending_revocations, persist_rpc_ledger, process_lock_path, public_url,
    queue_pending_revocation, remote_relay_ws_url, remove_config, remove_rpc_ledger,
    validate_config,
};

// RPC admission, response shaping, scope validation, and the stream/event
// protocol message builders live in `rpc`. The facade dispatches against the
// enums (`RpcRequestAction`, `EventSource`, ...) and calls
// the pure helpers, so the whole surface is imported by name.
use rpc::{
    EventSource, NewRpcAdmission, RPC_TIMED_OUT_CODE, ReplayMessageContext, RpcRejection,
    RpcRequestAction, WebScopeRejection, bounded_rpc_completion, enqueue_stream_reset,
    event_message, prepare_bridge_generation, prepare_new_rpc_admission, prune_rpc_cache,
    request_conflict_completion, rpc_admission_rejection, rpc_error_completion, rpc_fingerprint,
    rpc_in_flight_expired, rpc_response, snapshot_message, stream_reset_message,
    subscription_filtered_replay_messages, tombstone_completion, tombstone_completion_cacheable,
    try_enqueue_message_batch, validate_bridge_generation, validate_rpc_command,
    validate_web_rpc_scope,
};

// The transfer buffer helpers mutate `Inner` through borrowed guards. They are
// imported by name so the facade calls compile unchanged, and the two
// crate-level entry points stay available to the command layer via `manager::`.
use transfer::{
    append_web_attachment_upload_chunk, clear_web_attachments, discard_web_attachment_upload,
    ensure_web_session_download_capacity, finish_web_attachment_reservation_inner,
    open_web_session_downloads_base, prune_expired_web_session_transfers,
    read_web_session_download_chunk, remove_web_attachment_upload_dir,
    request_web_attachment_discard, sweep_stale_web_session_downloads, take_web_session_download,
};
// The download-id ownership rule is the mint-side contract (`begin_web_session_download`
// issues ids through it); the command-layer cancel gate re-uses it so the two
// cannot drift again.
pub(crate) use transfer::{
    is_owned_session_download_id, sweep_stale_web_attachment_uploads, web_attachment_uploads_base,
};
use workspace_grants::{
    WebWorkspaceGrantReservation, WebWorkspaceGrantStore, WebWorkspaceIdentity,
    require_host_workspace_authorization,
};

// The compile-time default points at the local development Relay (the same value
// run-dev.sh injects, so it is equivalent to no override); there is currently no
// packaging-time production endpoint injection, so production deployments rely on the
// user saving a custom Relay in settings (highest priority).
const DEFAULT_PUBLIC_BASE_URL: &str = "http://127.0.0.1:8787/pinvou3/remote";
const DEFAULT_RELAY_WS_URL: &str = "ws://127.0.0.1:8787/pinvou3/remote/ws";
const MAX_WEB_ACCESS_CONFIG_BYTES: usize = 16 * 1024;
const JOURNAL_CAPACITY: usize = 1_024;
const JOURNAL_BYTES_CAPACITY: usize = 16 * 1024 * 1024;
// Relay-issued lease IDs are `lease_` plus 24 base64url characters. Use an
// equal-length placeholder while no browser is connected so a journaled
// event's complete replay envelope is still sized conservatively.
const LEASE_ID_WIRE_PLACEHOLDER: &str = "lease_000000000000000000000000";
const RPC_CACHE_CAPACITY: usize = 512;
const RPC_CACHE_BYTES_CAPACITY: usize = 32 * 1024 * 1024;
const MAX_RPC_IN_FLIGHT: usize = 32;
/// The browser side gives up on `invoke` after 180s; desktop commands stuck past this
/// limit have their in-flight slots lazily released, so `too_many_in_flight_requests`
/// never permanently refuses later requests.
const RPC_IN_FLIGHT_TTL: Duration = Duration::from_secs(300);
const MAX_RPC_COMMAND_BYTES: usize = 128;
/// The new-form multi-agent sessions (ordinary id plus a toggle flag) are read-only
/// over Web (ADR-0006, desktop-only): view/list still pass (the read-only banner needs
/// its data), while every entrance that could trigger a new round of model execution
/// or change a running turn's state is intercepted uniformly — previously only the
/// input box was blocked, so edit-resend / plan adjudication / approval cancellation /
/// stop generation could all slip through (review P1). The desktop does not go through
/// this funnel and is unaffected.
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
    // Documented exception (round-34 minor 7; comment reworded per the
    // round-37 review — that reword (commit ef48f0701) was dropped by a
    // branch rewrite before it could merge, never reaching main in any
    // form, and is restored here re-keyed on the mechanism, ADR-0024): the
    // early return keys on the SESSION ARGUMENT being aux-shaped — an
    // `aux-` id bypasses this denylist scope check for ANY command. That
    // is exactly what keeps an aux conversation fully usable over the
    // relay while its MAIN session runs multi-agent. The aux-id carriers
    // through this validator today are the WebUI aux send (`web_access_chat`,
    // denylist entry #11) and the aux panel's hydration reads
    // (`web_access_load_session_chunk`, `get_session_timeline`, the
    // persona/reviews/scene sidecar reads, `web_access_cancel_session_download`);
    // the three aux management commands carry the PARENT id (the aux id is
    // minted server-side and never sent back). The invariant to preserve is
    // "aux ids are never subject to the multi-agent web execution denylist",
    // because the aux engine is the isolated pure-Q&A config (at most one
    // scoped read-only context tool, never an execution surface, no
    // subagents — the denylist's threat model never applies to it), the
    // desktop offers the same entry, and blocking it would break the side
    // chat for exactly the heaviest users. The denylist's "web is read-only"
    // promise is scoped to the multi-agent EXECUTION surface, not to its
    // side chat.
    if crate::features::sessions::is_aux_session_id(session_id) {
        return Ok(());
    }
    if !MULTI_AGENT_WEB_EXECUTION_DENYLIST.contains(&command) {
        return Ok(());
    }
    let is_multi_agent = {
        use tauri::Manager as _;
        app.state::<crate::features::sessions::SessionStore>()
            .mode_state(session_id)
            .multi_agent
    };
    if is_multi_agent {
        return Err(format!(
            "{command} is not available for desktop-only multi-agent Sessions over Web"
        ));
    }
    Ok(())
}

const MAX_RPC_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_RPC_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const RPC_LEDGER_VERSION: u8 = 1;
/// Session chunks get a separate larger cap: opening a session over a high-latency
/// link with 256KB serial pulls was too slow. The cap stays under
/// `MAX_RPC_RESPONSE_BYTES` (2MiB), leaving headroom for the JSON envelope after
/// base64 expansion.
pub const MAX_SESSION_CHUNK_BYTES: usize = 1024 * 1024;
const MAX_WEB_ATTACHMENTS: usize = 16;
const MAX_WEB_ATTACHMENT_BYTES: usize = 32 * 1024 * 1024;
const MAX_WEB_ATTACHMENTS_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_WEB_ATTACHMENT_UPLOADS: usize = 4;
const MAX_WEB_ATTACHMENT_UPLOAD_TOTAL_BYTES: usize = 64 * 1024 * 1024;
/// Prefix of the desktop's short-lived staging directories for browser-local uploads.
/// Only directories carrying this prefix are removed by the startup sweep and
/// attachment-consumption cleanup, so other uploads subdirectories used by legacy E2E
/// are never touched.
pub(crate) const WEB_ATTACHMENT_UPLOAD_DIR_PREFIX: &str = "webup_";
const MAX_WEB_SESSION_DOWNLOADS: usize = 2;
const PENDING_REVOCATIONS_VERSION: u8 = 1;
const MAX_PENDING_REVOCATIONS: usize = 32;
const MAX_PENDING_REVOCATIONS_FILE_BYTES: usize = 256 * 1024;
/// Bounded read cap for the RPC ledger: a full 512 tombstones (each ≤256-byte id plus
/// a 64-bit fingerprint) leave several times of headroom under 1 MiB; anything larger
/// is a corrupt file and must not be read unbounded.
const MAX_RPC_LEDGER_FILE_BYTES: usize = 1024 * 1024;
const MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES: usize = 256 * 1024 * 1024;
const WEB_SESSION_TRANSFER_TTL: Duration = Duration::from_secs(10 * 60);
/// Round-16 review (M2): freshness window for full-slot eviction. "ready" cannot tell
/// a "response-lost orphan" from a "download the phone already holds an id for and is
/// still chunk-reading"; entries read within the window (a chunk read refreshes
/// last_touched) are never victims. The window is far smaller than the TTL: a real
/// orphan's cleanup is delayed at most one window, still far earlier than the 10-minute
/// Reaper.
const WEB_SESSION_DOWNLOAD_EVICTION_ACTIVE_WINDOW: Duration = Duration::from_secs(30);
const WEB_SESSION_TRANSFER_REAPER_INTERVAL: Duration = Duration::from_secs(60);

/// These error codes terminate the registration (relay_client's run_loop stops
/// reconnecting), and the manager must react with full cleanup or by resting in an
/// Error state carrying the reason. The relay's reject frames carry no endpoint_id, so
/// handle_relay_message's entrance must let them through; all other error codes
/// (including `endpoint_not_found` and the transient capacity/throttle kinds) keep
/// their existing behavior. This list stays aligned with relay_client::terminal_message's
/// RegisterRejected/Revoked arms (deliberately excluding `endpoint_not_found`: its
/// reject frame carries no endpoint_id and the entrance drops it anyway; letting the
/// list admit it would only create an unconsumed terminal path); the alignment is
/// pinned by the relay_client test
/// `terminal_registration_code_lists_stay_aligned`.
pub(crate) const TERMINAL_REGISTRATION_ERROR_CODES: &[&str] = &[
    "endpoint_revoked",
    "invalid_access_token",
    "invalid_desktop_secret",
    "bad_desktop_endpoint_register",
    "unsupported_protocol_version",
];

/// Extracts the session id from an event payload (different events use different field names).
fn event_session_id(payload: &Value) -> Option<&str> {
    payload
        .get("session_id")
        .or_else(|| payload.get("id"))
        .or_else(|| payload.get("sessionId"))
        .and_then(Value::as_str)
}

/// Filters native code-session events until the remote side officially supports code
/// sessions. With a None predicate (not injected) nothing is filtered and behavior
/// matches pre-injection.
fn should_filter_code_session_event(
    predicate: Option<&CodeSessionPredicate>,
    payload: &Value,
) -> bool {
    match predicate {
        Some(predicate) => {
            event_session_id(payload).is_some_and(|session_id| predicate(session_id))
        }
        None => false,
    }
}

/// A forwarded rpc_request must target the currently active Web lease (the same check
/// as `client_ready`). With no active lease nothing is filtered: the main admission
/// path has no replyable endpoint in that state, matching pre-check behavior.
/// A changed lease always invalidates subscriptions: they belong to a
/// concrete Web lease and must rebuild before `client_ready` requests a
/// replay, otherwise old subscriptions can advance the new client's event
/// cursor before it is ready. Staged transfers are only discarded on a
/// takeover — the relay marks a join that arrives after the previous Web
/// socket already died (`web_rejoining`); that client holds the same
/// endpoint token, so a watchdog- or network-driven rejoin must keep its
/// staged attachments, in-flight uploads, and session downloads instead of
/// destroying them on every idle reconnect.
fn apply_web_lease_change(inner: &mut Inner, web_rejoining: bool) {
    inner.subscriptions.clear();
    // Round-15 review (documenting a deliberate asymmetry): the takeover wipe grants no
    // reservation exemption — `clear_web_attachments` deletes attachment staging
    // directories even with an active reservation, unlike the capacity-eviction loop's
    // reservation-sparing; an in-flight turn's staging source then disappears and that
    // turn fails (its error chain folds to a stable code, impact limited to one turn).
    // This is part of the "new client, clean slate" semantics: the replacing client must
    // not be able to touch any of the previous lease's staged data, and an exemption
    // would leave behind directories the new client could probe after the takeover.
    if !web_rejoining {
        clear_web_attachments(inner);
    }
}

/// The in-memory side of revocation: syncs the active endpoint's authorization flag
/// and immediately invalidates all minted workspace grants. A grant is an endpoint-bound
/// one-shot handle (30-minute TTL); if not cleared, grants the phone minted before the
/// revocation could still be redeemed to open host-directory sessions for the whole
/// TTL window.
fn revoke_host_workspace_state(inner: &mut Inner) {
    if let Some(endpoint) = inner.endpoint.as_mut() {
        endpoint.config.allow_host_workspace = false;
    }
    inner.web_workspace_grants.clear();
}

/// `attachment_` prefix plus non-empty alphanumerics is the desktop-minted attachment
/// handle shape (`attachment_{short_token(32)}`). The shape check keeps caller-chosen
/// arbitrary bytes out of the error chain (Round-11 review minor).
fn attachment_handle_is_well_formed(handle: &str) -> bool {
    match handle.strip_prefix("attachment_") {
        Some(body) => !body.is_empty() && body.chars().all(|c| c.is_ascii_alphanumeric()),
        None => false,
    }
}

/// Relay text cap for storage/replay: the same caliber as `rpc_error_completion`'s
/// error truncation (char-boundary, never splitting a multi-byte character).
const RELAY_ERROR_TEXT_MAX_CHARS: usize = 16_384;

/// Round-29 review MINOR-1: the in-memory backlog is the whole-ledger
/// acknowledgement source, and `validate_pending_revocation_ledger` rejects
/// anything over MAX_PENDING_REVOCATIONS — a salvage degradation that leaves
/// memory-only siblings plus later distinct-key rotations can push the
/// backlog past the cap, and from then on every acknowledgement persist
/// fails validation, wedging all pending revocations for the process
/// lifetime (the wedge heals only at restart, which silently drops the
/// memory-only leftovers anyway). Retiring the OLDEST entries loudly
/// converts that permanent wedge into a bounded, named loss under the same
/// "endpoints stay joinable until revoked manually" discipline as the
/// salvage truncation log; the newest intents stay ackable, and a retired
/// entry re-enters only by being requeued. The ack error arm needs no cap:
/// its `remaining` is taken from an already-capped backlog and drops the
/// acked entry before re-pushing it.
fn cap_pending_revocation_backlog(backlog: &mut Vec<PendingRevocation>) {
    let excess = backlog.len().saturating_sub(MAX_PENDING_REVOCATIONS);
    if excess == 0 {
        return;
    }
    let retired: Vec<String> = backlog
        .drain(0..excess)
        .map(|entry| clip_pending_endpoint_id_for_log(&entry.endpoint_id))
        .collect();
    eprintln!(
        "[web-access] pending revocation backlog passed the capacity of {} — retired the oldest {} {} (endpoints stay joinable at the relay until revoked manually): {}",
        MAX_PENDING_REVOCATIONS,
        excess,
        if excess == 1 { "entry" } else { "entries" },
        retired.join(", ")
    );
}

/// Char-boundary truncation of relay-supplied free text (error messages, host naming
/// etc.) before storage — otherwise up-to-4-MiB frame text would be stored verbatim
/// and re-serialized on every status resend (Round-11 review NIT).
fn bound_relay_text(mut text: String) -> String {
    if let Some((boundary, _)) = text.char_indices().nth(RELAY_ERROR_TEXT_MAX_CHARS) {
        text.truncate(boundary);
    }
    text
}

/// Actionable recovery hints for registration-class error frames. For a version
/// mismatch the correct recovery is upgrading to a matching latest version — rotation
/// only mints a new endpoint with the same protocol version, so a "rotate" hint there
/// would be wrong guidance. One code covers both sides: a too-old desktop (the default
/// case) and a too-old self-hosted relay (where the desktop is actually new) cannot be
/// distinguished from the code, so the hint must cover both (Round-10 review minor).
fn relay_registration_error_reason(error_code: &str, relay_message: &str) -> String {
    match error_code {
        "invalid_access_token" | "invalid_desktop_secret" => {
            format!("{relay_message}; rotate Web access to mint a fresh endpoint")
        }
        "bad_desktop_endpoint_register" => format!(
            "{relay_message}; the persisted Web access config is unusable, rotate Web access to recover"
        ),
        "unsupported_protocol_version" => format!(
            "{relay_message}; update this desktop (and a self-hosted relay, if you run one) to the latest version to restore Web access"
        ),
        _ => relay_message.to_string(),
    }
}

/// Stable recovery codes for registration terminal codes: the raw `last_error` text is
/// English (rendered into the desktop status line); SettingsView maps this code to
/// trilingual recovery copy, falling back to the raw text when absent. The branches must
/// stay consistent with [`relay_registration_error_reason`]; both sides are pinned by
/// the same test.
fn relay_registration_error_code(error_code: &str) -> Option<&'static str> {
    match error_code {
        "invalid_access_token" | "invalid_desktop_secret" => Some("rotate_credentials"),
        "bad_desktop_endpoint_register" => Some("rotate_recover_config"),
        "unsupported_protocol_version" => Some("update_desktop"),
        _ => None,
    }
}

fn rpc_request_targets_active_lease(inner: &Inner, lease_id: &str) -> bool {
    inner
        .endpoint
        .as_ref()
        .and_then(|endpoint| endpoint.lease_id.as_deref())
        .map_or(true, |active| active == lease_id)
}

/// `client_ready` two-phase handshake (PROTOCOL.md): a missing or non-boolean
/// `state_ready` is always treated as phase-1 (not ready) — the desktop only negotiates
/// capabilities and the replay cursor and delivers no business events; defaulting to
/// true would let clients that never send the field skip the readiness barrier.
fn client_state_ready(value: &Value) -> bool {
    value
        .get("state_ready")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

const RUST_FORWARDED_EVENTS: &[&str] = &[
    "acp:event",
    "artifact:disk",
    "chat:compaction",
    "chat:delta",
    "chat:reasoning_start",
    "chat:reasoning_delta",
    "chat:reasoning_done",
    "chat:done",
    "chat:steer_committed",
    "chat:steer_dropped",
    "chat:plan_ready",
    "chat:plan_resolved",
    "chat:plan_snapshot",
    "chat:transcript_committed",
    "chat:turn_started",
    "chat:tool_end",
    "chat:tool_start",
    "chat:transient_error",
    "chat:usage",
    "chat:user_message",
    "chat:user_input_required",
    "multiagent:agent_progress",
    "remote_control:tools_changed",
    "scheduled_task:run_updated",
    "session:deleted",
    "session:list_changed",
    "session:model_changed",
    "session:persona_changed",
];

/// Until the remote side officially supports code-session listing/authorization/UI,
/// native code-session events carrying a session_id are never forwarded.
pub type CodeSessionPredicate = Arc<dyn Fn(&str) -> bool + Send + Sync>;

#[derive(Clone)]
pub struct RemoteControlManager {
    app: AppHandle,
    inner: Arc<Mutex<Inner>>,
    lifecycle: Arc<Mutex<()>>,
    policy: Arc<AccessPolicy>,
    policy_error: Option<String>,
}

struct Inner {
    /// Held for the remainder of this desktop process after Web access is
    /// first touched. This makes the persistent endpoint and RPC ledger a
    /// single-process authority even if two app instances are launched.
    /// The guard keeps the fd-lock write lock held; see
    /// `persistence::acquire_process_lock` for the `'static` lifetime story.
    process_lock: Option<fd_lock::RwLockWriteGuard<'static, File>>,
    endpoint: Option<ActiveEndpoint>,
    idle_status: WebAccessStatusKind,
    stream: StreamState,
    subscriptions: HashSet<String>,
    bridge_generation: Option<String>,
    rpc_ledger: RpcLedger,
    rpc_cache: HashMap<String, RpcCacheEntry>,
    rpc_order: VecDeque<String>,
    web_attachments: HashMap<String, CachedWebAttachment>,
    web_attachment_order: VecDeque<String>,
    web_attachment_bytes: usize,
    web_attachment_uploads: HashMap<String, WebAttachmentUpload>,
    web_attachment_upload_order: VecDeque<String>,
    web_workspace_grants: WebWorkspaceGrantStore,
    web_session_downloads: HashMap<String, WebSessionDownload>,
    web_session_download_order: VecDeque<String>,
    pending_revocations_in_flight: HashSet<String>,
    /// Round-11 review minor: this process's authoritative pending-revocation ledger.
    /// Loaded from disk at replay and appended in lockstep with disk writes at queue
    /// time; acknowledgement persists the whole of it, rather than "re-read disk →
    /// remove one → rewrite" — after a salvage degradation (salvage persist failed)
    /// there is no ledger on disk, and a re-read-style acknowledgement would take an
    /// empty ledger and clear sibling entries still in memory, losing this whole batch
    /// of pending revocations across a restart (the old endpoint stays joinable on the
    /// relay side).
    pending_revocation_backlog: Vec<PendingRevocation>,
    /// Native code-session predicate; None = not injected (no filtering, startup-early behavior unchanged).
    code_session_predicate: Option<CodeSessionPredicate>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            process_lock: None,
            endpoint: None,
            idle_status: WebAccessStatusKind::Idle,
            stream: StreamState::new(),
            subscriptions: HashSet::new(),
            bridge_generation: None,
            rpc_ledger: RpcLedger::default(),
            rpc_cache: HashMap::new(),
            rpc_order: VecDeque::new(),
            web_attachments: HashMap::new(),
            web_attachment_order: VecDeque::new(),
            web_attachment_bytes: 0,
            web_attachment_uploads: HashMap::new(),
            web_attachment_upload_order: VecDeque::new(),
            web_workspace_grants: WebWorkspaceGrantStore::default(),
            web_session_downloads: HashMap::new(),
            web_session_download_order: VecDeque::new(),
            pending_revocations_in_flight: HashSet::new(),
            pending_revocation_backlog: Vec::new(),
            code_session_predicate: None,
        }
    }
}

/// In-memory accumulation buffer for chunked browser-local file uploads. Persisting
/// and ingest happen only when the final chunk commits.
#[derive(Debug)]
struct WebAttachmentUpload {
    file_name: String,
    total: usize,
    data: Vec<u8>,
    last_touched: Instant,
}

#[derive(Debug)]
struct WebSessionDownload {
    session_id: String,
    reservation_id: String,
    file: File,
    reserved_bytes: usize,
    total: usize,
    ready: bool,
    last_touched: Instant,
}

pub struct WebSessionDownloadReservation {
    manager: RemoteControlManager,
    download_id: String,
    session_id: String,
    reservation_id: String,
    writer: File,
    committed: bool,
}

impl WebSessionDownloadReservation {
    pub fn try_clone_writer(&self) -> Result<File, String> {
        self.writer
            .try_clone()
            .map_err(|error| format!("clone serialized Session download handle: {error}"))
    }

    pub fn commit(mut self) -> Result<String, String> {
        let result = self.manager.commit_web_session_download(
            &self.download_id,
            &self.session_id,
            &self.reservation_id,
        );
        if result.is_ok() {
            self.committed = true;
        }
        result.map(|()| self.download_id.clone())
    }
}

impl Drop for WebSessionDownloadReservation {
    fn drop(&mut self) {
        if !self.committed {
            self.manager
                .remove_web_session_download(&self.download_id, Some(&self.reservation_id));
        }
    }
}

#[derive(Debug)]
pub struct WebSessionDownloadChunk {
    pub total: usize,
    pub data: Vec<u8>,
    pub eof: bool,
}

#[derive(Debug, Clone)]
struct CachedWebAttachment {
    result: crate::features::files::file_ingest::IngestResult,
    bytes: usize,
    reservation_id: Option<String>,
    /// The browser removed the chip while a chat submission still held the
    /// handle. Defer deletion until that reservation finishes so the engine
    /// never races a disappearing upload source.
    discard_requested: bool,
    /// Desktop staging directory for a browser-local upload. Deleted together with the
    /// attachment on consumption, eviction, or clearing, guaranteeing uploaded files do
    /// not persist; desktop host-file attachments are always None.
    upload_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebAttachmentSummary {
    pub handle: String,
    pub kind: String,
    pub basename: String,
    pub token_estimate: u32,
    pub byte_size: u64,
    pub warning: Option<String>,
}

#[derive(Clone)]
struct ActiveEndpoint {
    config: WebAccessConfig,
    url: String,
    status: WebAccessStatusKind,
    web_client_connected: bool,
    client_ready: bool,
    lease_id: Option<String>,
    last_lease_id: Option<String>,
    /// Stream head captured when a browser without a cursor joins. Readiness
    /// is asynchronous, so fresh-client replay must start here rather than at
    /// the later `client_ready` instant.
    fresh_baseline_seq: Option<u64>,
    last_error: Option<String>,
    /// Stable recovery code paired with `last_error`; any error write that carries no
    /// code must clear it, so an old code is never mapped onto new error text.
    last_error_code: Option<String>,
    sender: RelaySender,
}

#[derive(Debug, Clone)]
enum RpcCacheEntry {
    Pending(PendingRpc),
    Complete {
        fingerprint: String,
        completion: RpcCompletion,
        /// Round-14 review minor: marks the timeout placeholders the TTL sweep writes for
        /// "acknowledged but still executing" requests (only the sweep arm sets true). The
        /// generation arm on restart/reload uses this to rewrite such placeholders into the
        /// honest `outcome_unknown` — otherwise replay would forever return a fabricated
        /// `rpc_timed_out` where the contract's honest answer for that state is exactly
        /// outcome_unknown; unacknowledged placeholders are not marked (a timeout is an
        /// accurate answer for a never-executed request).
        /// In-memory only: the RPC ledger persists tombstones, not this cache.
        swept_acknowledged: bool,
    },
}

#[derive(Debug, Clone)]
struct PendingRpc {
    lease_id: String,
    command: String,
    args: Value,
    fingerprint: String,
    dispatched_generation: Option<String>,
    acknowledged_generation: Option<String>,
    dispatched_at: Instant,
}

#[derive(Debug, Clone)]
struct RpcDispatch {
    request_id: String,
    command: String,
    args: Value,
    bridge_generation: String,
}

#[derive(Debug, Clone)]
struct RpcCompletion {
    ok: bool,
    result: Value,
    error: Option<String>,
    error_code: Option<String>,
    error_category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RpcLedger {
    #[serde(default = "rpc_ledger_version")]
    version: u8,
    #[serde(default)]
    endpoint_id: String,
    #[serde(default)]
    entries: VecDeque<RpcTombstone>,
}

impl Default for RpcLedger {
    fn default() -> Self {
        Self {
            version: RPC_LEDGER_VERSION,
            endpoint_id: String::new(),
            entries: VecDeque::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RpcTombstone {
    request_id: String,
    fingerprint: String,
    #[serde(default)]
    acknowledged: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingRevocation {
    relay_url: String,
    endpoint_id: String,
    desktop_secret: String,
}

// Hand-written Debug: desktop_secret is a plaintext credential and a derived Debug
// would leak it into logs and panic messages; endpoint_id/relay_url stay visible for
// diagnosis.
impl std::fmt::Debug for PendingRevocation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingRevocation")
            .field("relay_url", &self.relay_url)
            .field("endpoint_id", &self.endpoint_id)
            .field("desktop_secret", &"[redacted]")
            .finish()
    }
}

impl From<&WebAccessConfig> for PendingRevocation {
    fn from(config: &WebAccessConfig) -> Self {
        Self {
            relay_url: config.relay_url.clone(),
            endpoint_id: config.endpoint_id.clone(),
            desktop_secret: config.desktop_secret.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingRevocationLedger {
    version: u8,
    #[serde(default)]
    entries: VecDeque<PendingRevocation>,
}

impl Default for PendingRevocationLedger {
    fn default() -> Self {
        Self {
            version: PENDING_REVOCATIONS_VERSION,
            entries: VecDeque::new(),
        }
    }
}

const fn rpc_ledger_version() -> u8 {
    RPC_LEDGER_VERSION
}

impl RpcLedger {
    fn for_endpoint(endpoint_id: &str) -> Self {
        Self {
            version: RPC_LEDGER_VERSION,
            endpoint_id: endpoint_id.to_string(),
            entries: VecDeque::new(),
        }
    }

    fn get(&self, request_id: &str) -> Option<&RpcTombstone> {
        self.entries
            .iter()
            .find(|entry| entry.request_id == request_id)
    }

    fn remember(&mut self, request_id: &str, fingerprint: &str) -> Result<bool, String> {
        if let Some(existing) = self.get(request_id) {
            if existing.fingerprint != fingerprint {
                return Err("client_request_id was reused with different command or args".into());
            }
            return Ok(false);
        }
        if self.entries.len() == RPC_CACHE_CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(RpcTombstone {
            request_id: request_id.to_string(),
            fingerprint: fingerprint.to_string(),
            acknowledged: false,
        });
        Ok(true)
    }

    fn acknowledge(&mut self, request_id: &str, fingerprint: &str) -> Result<(), String> {
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.request_id == request_id)
            .ok_or_else(|| format!("RPC tombstone is missing: {request_id}"))?;
        if entry.fingerprint != fingerprint {
            return Err("RPC tombstone fingerprint conflict".to_string());
        }
        entry.acknowledged = true;
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct StreamEvent {
    seq: u64,
    event: String,
    payload: Value,
    wire_bytes: usize,
}

#[derive(Debug)]
struct StreamState {
    epoch: String,
    seq: u64,
    journal: VecDeque<StreamEvent>,
    journal_bytes: usize,
}

#[derive(Debug, PartialEq, Eq)]
enum StreamRecordError {
    Serialize(String),
    Oversized { wire_bytes: usize, limit: usize },
}

impl std::fmt::Display for StreamRecordError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialize(error) => write!(formatter, "序列化远程控制事件帧失败：{error}"),
            Self::Oversized { wire_bytes, limit } => write!(
                formatter,
                "远程控制事件帧过大（{wire_bytes} 字节；上限 {limit}）"
            ),
        }
    }
}

impl StreamState {
    fn new() -> Self {
        Self {
            epoch: new_stream_epoch(),
            seq: 0,
            journal: VecDeque::new(),
            journal_bytes: 0,
        }
    }

    fn reset(&mut self) {
        self.epoch = new_stream_epoch();
        self.seq = 0;
        self.journal.clear();
        self.journal_bytes = 0;
    }

    fn record(
        &mut self,
        endpoint_id: &str,
        lease_id: &str,
        event: String,
        payload: Value,
    ) -> Result<Value, StreamRecordError> {
        self.record_with_limits(
            endpoint_id,
            lease_id,
            event,
            payload,
            relay_client::MAX_RELAY_FRAME_BYTES,
            JOURNAL_CAPACITY,
            JOURNAL_BYTES_CAPACITY,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn record_with_limits(
        &mut self,
        endpoint_id: &str,
        lease_id: &str,
        event: String,
        payload: Value,
        max_wire_bytes: usize,
        journal_capacity: usize,
        journal_bytes_capacity: usize,
    ) -> Result<Value, StreamRecordError> {
        let mut recorded = StreamEvent {
            seq: self.seq.saturating_add(1),
            event,
            payload,
            wire_bytes: 0,
        };
        let message = event_message(endpoint_id, lease_id, &self.epoch, &recorded);
        let wire_bytes = serde_json::to_vec(&message)
            .map_err(|error| StreamRecordError::Serialize(error.to_string()))?
            .len();
        if wire_bytes > max_wire_bytes {
            // The rejected event never advances seq or enters the journal. A
            // fresh epoch makes every existing browser cursor explicitly
            // invalid instead of leaving a permanent, unreplayable hole.
            self.reset();
            return Err(StreamRecordError::Oversized {
                wire_bytes,
                limit: max_wire_bytes,
            });
        }

        recorded.wire_bytes = wire_bytes;
        self.seq = recorded.seq;
        self.journal_bytes = self.journal_bytes.saturating_add(wire_bytes);
        self.journal.push_back(recorded);
        while self.journal.len() > journal_capacity || self.journal_bytes > journal_bytes_capacity {
            let Some(evicted) = self.journal.pop_front() else {
                break;
            };
            self.journal_bytes = self.journal_bytes.saturating_sub(evicted.wire_bytes);
        }
        Ok(message)
    }

    /// `None` means the cursor cannot be satisfied from the bounded journal.
    fn replay_after(&self, after_seq: u64) -> Option<Vec<StreamEvent>> {
        if after_seq > self.seq {
            return None;
        }
        if after_seq == self.seq {
            return Some(Vec::new());
        }
        let oldest = self.journal.front()?.seq;
        if after_seq.saturating_add(1) < oldest {
            return None;
        }
        Some(
            self.journal
                .iter()
                .filter(|entry| entry.seq > after_seq)
                .cloned()
                .collect(),
        )
    }
}

#[derive(Debug, Deserialize)]
struct EmbeddedPolicy {
    protocol_version: u8,
    #[serde(default)]
    allowed_commands: Vec<String>,
    #[serde(default)]
    allowed_events: Vec<String>,
}

#[derive(Debug, Default)]
struct AccessPolicy {
    commands: HashSet<String>,
    events: HashSet<String>,
}

impl AccessPolicy {
    fn load() -> Result<Self, String> {
        let parsed: EmbeddedPolicy = serde_json::from_str(include_str!(
            "../../../../../src/platform/web/access-policy.json"
        ))
        .map_err(|error| format!("parse embedded Web access policy: {error}"))?;
        if parsed.protocol_version != PROTOCOL_VERSION {
            return Err(format!(
                "Web access policy protocol {} does not match backend protocol {}",
                parsed.protocol_version, PROTOCOL_VERSION
            ));
        }
        let policy = Self {
            commands: parsed.allowed_commands.into_iter().collect(),
            events: parsed.allowed_events.into_iter().collect(),
        };
        policy.validate_capability_snapshot()?;
        Ok(policy)
    }

    fn capability_snapshot_wire_bytes(&self) -> Result<usize, String> {
        let mut commands = self.commands.iter().cloned().collect::<Vec<_>>();
        commands.sort();
        let mut events = self.events.iter().cloned().collect::<Vec<_>>();
        events.sort();
        let endpoint_id = "e".repeat(128);
        let stream_epoch = format!("epoch_{}", "0".repeat(24));
        serde_json::to_vec(&snapshot_message(
            &endpoint_id,
            LEASE_ID_WIRE_PLACEHOLDER,
            &stream_epoch,
            u64::MAX,
            &commands,
            &events,
        ))
        .map(|encoded| encoded.len())
        .map_err(|error| format!("serialize Web access capability snapshot: {error}"))
    }

    fn validate_capability_snapshot(&self) -> Result<(), String> {
        let wire_bytes = self.capability_snapshot_wire_bytes()?;
        if wire_bytes > relay_client::MAX_CONTROL_FRAME_BYTES {
            return Err(format!(
                "Web access capability snapshot is too large ({wire_bytes} bytes; limit {})",
                relay_client::MAX_CONTROL_FRAME_BYTES
            ));
        }
        Ok(())
    }
}

/// Tear down per-endpoint Web state shared by every lifecycle transition
/// (rotate, stop, revoke, replace). Callers that also retire the durable RPC
/// ledger or the stream journal reset those next to this helper.
fn reset_web_state(inner: &mut Inner) {
    inner.subscriptions.clear();
    inner.rpc_cache.clear();
    inner.rpc_order.clear();
    clear_web_attachments(inner);
    inner.web_workspace_grants.clear();
}

/// Fault-tolerant read dedicated to the stop path: a corrupt or oversized
/// web-access.json continues as "no config" so stop always works (stop deletes the
/// file right after). start/resume/refresh still follow `load_config`'s fail-closed
/// semantics and report the concrete path.
fn load_config_for_stop_at(path: &Path) -> Option<WebAccessConfig> {
    match load_config_from(path) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("[web-access] stop ignores unreadable Web access config: {error}");
            None
        }
    }
}

fn load_config_for_stop() -> Option<WebAccessConfig> {
    load_config_for_stop_at(&config_path())
}

impl RemoteControlManager {
    pub fn new(app: AppHandle) -> Self {
        let (policy, policy_error) = match AccessPolicy::load() {
            Ok(policy) => (policy, None),
            Err(error) => {
                eprintln!("[web-access] {error}");
                (AccessPolicy::default(), Some(error))
            }
        };
        let manager = Self {
            app,
            inner: Arc::new(Mutex::new(Inner::default())),
            lifecycle: Arc::new(Mutex::new(())),
            policy: Arc::new(policy),
            policy_error,
        };
        manager.start_web_session_transfer_reaper();
        manager
    }

    /// Resume the persistent endpoint after all authoritative application
    /// state (notably `EnginePool`) has been managed by Tauri.
    pub fn resume(&self) -> Result<bool, String> {
        let _lifecycle = self.lifecycle.lock();
        self.ensure_policy()?;
        self.ensure_process_ownership()?;
        self.replay_pending_revocations()?;
        if self.inner.lock().endpoint.is_some() {
            return Ok(true);
        }
        let Some(config) = load_config()? else {
            return Ok(false);
        };
        let config = apply_runtime_relay_override(config);
        validate_config(&config)?;
        self.activate(config)?;
        Ok(true)
    }

    /// Enable WebUI access. Repeated calls return the same persistent endpoint
    /// instead of creating session-scoped rooms.
    pub fn start(&self, allow_host_workspace: bool) -> Result<WebAccessInfo, String> {
        let _lifecycle = self.lifecycle.lock();
        self.ensure_policy()?;
        self.ensure_process_ownership()?;
        self.replay_pending_revocations()?;
        let active_endpoint_id = self
            .inner
            .lock()
            .endpoint
            .as_ref()
            .map(|endpoint| endpoint.config.endpoint_id.clone());
        let (mut config, is_new) = match load_config()? {
            Some(config) => {
                validate_config(&config)?;
                (config, false)
            }
            None if active_endpoint_id.is_some() => {
                return Err("Web access endpoint configuration is missing".to_string());
            }
            None => (fresh_config(false), true),
        };
        if active_endpoint_id
            .as_ref()
            .is_some_and(|endpoint_id| endpoint_id != &config.endpoint_id)
        {
            return Err("Web access endpoint configuration changed".to_string());
        }
        let authorization_changed = allow_host_workspace && !config.allow_host_workspace;
        config.allow_host_workspace |= allow_host_workspace;
        if is_new || authorization_changed {
            persist_config(&config)?;
        }
        if active_endpoint_id.is_some() {
            let mut inner = self.inner.lock();
            let endpoint = inner
                .endpoint
                .as_mut()
                .ok_or_else(|| "Web access endpoint stopped while enabling access".to_string())?;
            endpoint.config.allow_host_workspace = config.allow_host_workspace;
            return Ok(pairing_info(endpoint));
        }

        self.activate(apply_runtime_relay_override(config))?;
        let inner = self.inner.lock();
        let endpoint = inner
            .endpoint
            .as_ref()
            .ok_or_else(|| "Web access endpoint failed to start".to_string())?;
        Ok(pairing_info(endpoint))
    }

    /// Rotate both browser and desktop credentials. The previous endpoint is
    /// revoked at the relay and can no longer reconnect.
    pub fn refresh(&self) -> Result<WebAccessInfo, String> {
        let _lifecycle = self.lifecycle.lock();
        self.ensure_policy()?;
        self.ensure_process_ownership()?;
        self.replay_pending_revocations()?;
        let persisted_previous = load_config()?;
        let previous_config = self
            .inner
            .lock()
            .endpoint
            .as_ref()
            .map(|endpoint| endpoint.config.clone())
            .or_else(|| persisted_previous.clone());
        if let Some(previous) = previous_config.as_ref() {
            match queue_pending_revocation(previous) {
                Ok(()) => self.sync_pending_revocation_backlog(previous),
                // Round-15 review: a full queue no longer wedges rotation — rotation is a
                // recovery path, and from the 33th relay-less rotation onward the queue is
                // full, where `?` propagation would make every later refresh fail.
                // Degrade to logging and continue; the cost must be stated honestly
                // (Round-17 review correction — the original "revocation intent stays in
                // place" claim was inaccurate): for this queue-full rotation the old
                // credentials' revocation intent landed nowhere — the full queue rejects
                // before appending, the backlog sync is skipped with it, and persist_config
                // below overwrites web-access.json — so the old endpoint stays joinable on
                // the relay until relay expiry or manual cleanup, the same honest caliber
                // as the salvage truncation log. Same-key-different-secret conflicts remain
                // fail-closed per the design boundary recorded in round-14.
                Err(error) if error.starts_with(PENDING_REVOCATION_QUEUE_FULL_PREFIX) => {
                    eprintln!(
                        "[web-access] refresh ignores full pending-revocation queue: {error}"
                    );
                }
                Err(error) => return Err(error),
            }
        }
        let allow_host_workspace = previous_config
            .as_ref()
            .is_some_and(|config| config.allow_host_workspace);
        let config = fresh_config(allow_host_workspace);
        // The old credentials are already durably queued above. If this write
        // fails, the queue entry still matches the current config and replay
        // deliberately leaves the still-active endpoint alone.
        persist_config(&config)?;
        let previous = {
            let mut inner = self.inner.lock();
            inner.rpc_ledger = RpcLedger::default();
            reset_web_state(&mut inner);
            inner.stream.reset();
            inner.endpoint.take()
        };
        if let Some(previous) = previous {
            let _ = previous.sender.try_send(RelayOutbound::Shutdown);
        }
        // Round-15 review: a `?` here used to report the half-committed window "rotation
        // committed (old credentials retired, new ones persisted) but activate never ran"
        // as a failure. Replay is a cleanup path with the same tolerance as stop; a
        // failure does not roll back the committed rotation.
        if let Err(error) = self.replay_pending_revocations() {
            eprintln!("[web-access] refresh ignores pending-revocation replay failure: {error}");
        }
        self.activate(apply_runtime_relay_override(config))?;
        let inner = self.inner.lock();
        Ok(pairing_info(inner.endpoint.as_ref().ok_or_else(|| {
            "Web access endpoint failed to rotate".to_string()
        })?))
    }

    /// Revokes host-directory authorization without disconnecting the link: later
    /// workspace grants are refused at mint time and already-issued grants are
    /// invalidated immediately with it (otherwise a one-shot 30-minute-TTL handle could
    /// still be redeemed after revocation, leaving the authorization flag inconsistent
    /// with the actually accessible surface). This is the settings dialog's explicit
    /// revocation entrance — making the allow_host_workspace that older versions'
    /// single "enable access" button wrote down visible and individually retractable,
    /// without rotating or stopping the whole link (the only clear path besides Stop).
    /// Idempotent: when already unauthorized the credentials file is not rewritten, but
    /// state invalidation and the status refresh still run (repeat revocations have no
    /// side effects).
    pub fn revoke_host_workspace(&self) -> Result<WebAccessInfo, String> {
        let _lifecycle = self.lifecycle.lock();
        self.ensure_policy()?;
        self.ensure_process_ownership()?;
        // Round-11 review minor: state invalidation does not depend on the config file —
        // invalidate the in-memory flag and every issued grant first, then read back /
        // persist the config. If web-access.json corrupts while the endpoint is alive, a
        // strict-read failure must not wedge the revocation (same rationale as
        // stop_current's tolerant read: stop/revocation are themselves recovery means),
        // or one-shot grants would stay redeemable for the rest of their TTL. On a
        // read/validation failure the durable side never changed and start/enable are
        // fail-closed on a corrupt file, so there is no resurrection surface and the
        // error is presented on the revoked side; persist failure is a different matter —
        // the disk still latches authorization, and if memory rested on the revoked side,
        // the next start/enable would rebuild authorization from disk with no consent
        // gesture (Round-15 review MA1). A persist failure must therefore re-latch the
        // in-memory flag to the disk side before returning: both state sides consistent,
        // the UI showing the real authorization state, and the error saying plainly that
        // the revocation did not take effect and should be retried.
        {
            let mut inner = self.inner.lock();
            revoke_host_workspace_state(&mut inner);
        }
        // Round-12 review P3: revoking something never configured (before enable) is an
        // idempotent no-op — with no config, neither the flag nor grants can exist, and
        // the invalidation above was already a no-op. Returning Err previously let the
        // bridge record it into last_error, so the settings page showed a failure for a
        // click with no actual effect; return the inactive info with an empty endpoint_id
        // instead (the same return shape stop_current uses for a non-active endpoint). No
        // status is emitted: there is no state change to push, and the settings page
        // merges with the payload returned this time.
        // (Round-19 review NIT: that premise relies on the endpoint being empty too — in
        // the malformed state where the config file was deleted externally while
        // inner.endpoint remains, the invalidation above really did flip the flag/clear
        // grants, and not emitting status would leave the UI stuck on an authorized
        // display until the next unrelated status push. Emit one status refresh when an
        // endpoint is present; the normal empty-config path stays as-is and emits
        // nothing.)
        let mut config = match load_config() {
            Ok(Some(config)) => config,
            Err(error) => {
                // Round-20 review MINOR: on a strict-read failure the invalidation above
                // really did flip the flag/clear grants (a real state change) — same
                // rationale as the config-absent arm's Round-19 NIT below, so emit one
                // status refresh before returning the error, keeping the UI from staying
                // stuck on an authorized display until the next unrelated status push. No
                // inner guard is held here (the invalidation block above is closed), so the
                // exact lock-span pin is not triggered.
                self.emit_status();
                return Err(error);
            }
            Ok(None) => {
                let endpoint_present = {
                    // Take and drop the guard inside a standalone block: the lock must not
                    // span into emit_status (a non-reentrant lock self-deadlocks on the same
                    // thread), and this also satisfies the exact lock-span pin's brace-depth
                    // criterion above.
                    let inner = self.inner.lock();
                    inner.endpoint.is_some()
                };
                if endpoint_present {
                    self.emit_status();
                }
                return Ok(WebAccessInfo {
                    endpoint_id: String::new(),
                    url: String::new(),
                    qr_data_url: None,
                    status: WebAccessStatusKind::Stopped,
                });
            }
        };
        validate_config(&config).map_err(|error| {
            // Round-22 review MINOR: the validation-failure arm follows the same
            // discipline as the persist-failure arm — the durable side is untouched while
            // memory has already had its flag cleared by the invalidation block above; if
            // memory rested on the revoked side, a later benign start/enable would rebuild
            // authorization from disk with no consent gesture (the same resurrection
            // surface as Round-15 review MA1). When the disk still latches authorization,
            // re-latch the flag, refresh status, and say plainly the revocation did not
            // take effect; when the disk was already unauthorized the revocation
            // substantively succeeded — just refresh status and report the corrupt config.
            // No inner guard is held here, so the exact lock-span pin is not triggered.
            let remains_enabled = config.allow_host_workspace;
            if remains_enabled {
                {
                    let mut inner = self.inner.lock();
                    if let Some(endpoint) = inner.endpoint.as_mut() {
                        endpoint.config.allow_host_workspace = true;
                    }
                }
            }
            self.emit_status();
            // The copy deliberately does not reuse the persist-failure arm's error
            // phrase — that phrase is the terminator needle of the existing behavioral
            // pin's (revoke_persist_failure_re_latches…) failure-arm window, and reusing it
            // here would reverse its window slice and turn it red; the two arms are
            // different failure shapes anyway, so separate copy is also more accurate.
            // (Round-28 review calibration: this pin now runs through comment_filtered,
            // pure comment lines are stripped, and comment mentions no longer form an
            // injection surface; trailing comments and block comments can still shield it,
            // handled as a disclosed residual.)
            if remains_enabled {
                format!(
                    "host-workspace revocation did not take effect; folder consent stays enabled on disk: {error}"
                )
            } else {
                format!("host-workspace revocation found an invalid persisted config: {error}")
            }
        })?;
        let latched = config.allow_host_workspace;
        if latched {
            config.allow_host_workspace = false;
            // Round-14 review minor: a concurrent desktop_endpoint_revoked (the message
            // pump arm, which holds no lifecycle lock) may delete the config file right
            // between this read and the persist; the revocation goal is then already
            // achieved (the relay-side endpoint is tombstoned, local credentials cleared),
            // and a flag=false config file must not be resurrected into a "configured"
            // phantom (otherwise the next start spins one round on a tombstoned endpoint
            // before handle_revoked cleans it up). The existence check narrows the window
            // without closing it (closing it fully would need the pump arm to hold the
            // lifecycle lock — a deadlock surface not worth it); the residual shape
            // self-heals with no access gain, consistent with this function's existing
            // comments.
            if config_path().exists() {
                if let Err(error) = persist_config(&config) {
                    // Persist failure: the disk latch is untouched, so memory must not rest
                    // on the revoked side (otherwise memory/disk diverge and later benign
                    // start/enable silently resurrect authorization). Re-latch the flag,
                    // refresh status, and return an actionable error.
                    eprintln!(
                        "[web-access] host-workspace revocation failed to persist, re-latching in-memory consent: {error}"
                    );
                    {
                        let mut inner = self.inner.lock();
                        if let Some(endpoint) = inner.endpoint.as_mut() {
                            endpoint.config.allow_host_workspace = true;
                        }
                    }
                    self.emit_status();
                    return Err(format!(
                        "host-workspace revocation could not be persisted; folder access remains enabled: {error}"
                    ));
                }
            }
        }
        // State invalidation runs unconditionally first (on idempotent re-entry the
        // grant clearing equally really happens), so the status refresh is always sent;
        // emit_status→status locks inner again, so the standalone block that takes the
        // guard must have left scope first, or parking_lot's non-reentrant lock
        // self-deadlocks on the same thread (revocation would deadlock; the deadlock pin
        // judges by brace depth).
        self.emit_status();
        let inner = self.inner.lock();
        // Round-5 review minor: when the endpoint is not active (resume not yet
        // successful / activation failed), the revocation has still persisted
        // successfully — reporting failure would make the settings dialog show an error
        // for a revocation that really happened. Return inactive info carrying the
        // config's endpoint_id instead of Err.
        match inner.endpoint.as_ref() {
            Some(endpoint) => Ok(pairing_info(endpoint)),
            None => Ok(WebAccessInfo {
                endpoint_id: config.endpoint_id.clone(),
                url: String::new(),
                qr_data_url: None,
                status: WebAccessStatusKind::Stopped,
            }),
        }
    }

    /// Injects the native code-session predicate; events carrying code-session ids are
    /// no longer forwarded to the remote WebUI afterwards.
    pub fn set_code_session_predicate(&self, predicate: CodeSessionPredicate) {
        self.inner.lock().code_session_predicate = Some(predicate);
    }

    pub fn stop_current(&self) -> Result<(), String> {
        let _lifecycle = self.lifecycle.lock();
        self.ensure_process_ownership()?;
        // Stopping is itself a recovery means; corrupt persistent state must not wedge
        // stopping too: start/resume/refresh stay fail-closed (reporting the concrete
        // path) while this only degrades the read and continues cleanup.
        if let Err(error) = self.replay_pending_revocations() {
            eprintln!("[web-access] stop ignores pending-revocation replay failure: {error}");
        }
        let persisted = load_config_for_stop();
        let current_config = self
            .inner
            .lock()
            .endpoint
            .as_ref()
            .map(|endpoint| endpoint.config.clone())
            .or_else(|| persisted.clone());
        if let Some(config) = current_config.as_ref() {
            match queue_pending_revocation(config) {
                Ok(()) => self.sync_pending_revocation_backlog(config),
                // Round-15 review: a full queue does not wedge stop (a recovery path; same
                // rationale as the refresh arm); the cost is recorded honestly with the same
                // caliber as the refresh arm — this stop's revocation intent landed nowhere,
                // and the endpoint stays joinable on the relay until expiry or manual
                // cleanup. Same-key-different-secret conflicts keep the round-14 fail-closed
                // boundary.
                Err(error) if error.starts_with(PENDING_REVOCATION_QUEUE_FULL_PREFIX) => {
                    eprintln!("[web-access] stop ignores full pending-revocation queue: {error}");
                }
                Err(error) => return Err(error),
            }
        }
        // A failed removal leaves the old config authoritative. Its queued
        // entry therefore remains dormant instead of revoking a live link.
        // A corrupt config file is deleted too: after stop, no unparseable state remains on disk.
        remove_config()?;
        let endpoint = {
            let mut inner = self.inner.lock();
            reset_web_state(&mut inner);
            inner.rpc_ledger = RpcLedger::default();
            inner.stream.reset();
            inner.idle_status = WebAccessStatusKind::Stopped;
            inner.endpoint.take()
        };
        if let Some(endpoint) = endpoint {
            let _ = endpoint.sender.try_send(RelayOutbound::Shutdown);
        }
        if let Err(error) = self.replay_pending_revocations() {
            eprintln!("[web-access] stop ignores pending-revocation replay failure: {error}");
        }
        // Round-15 review: the stop itself is complete (config deleted, endpoint torn
        // down), so a ledger-file removal failure only degrades — returning Err would
        // make the user see a failure for a stop that already took effect.
        if let Err(error) = remove_rpc_ledger() {
            eprintln!("[web-access] stop ignores rpc-ledger removal failure: {error}");
        }
        self.emit_status();
        Ok(())
    }

    fn ensure_process_ownership(&self) -> Result<(), String> {
        let mut inner = self.inner.lock();
        if inner.process_lock.is_some() {
            return Ok(());
        }
        let lock = acquire_process_lock(&process_lock_path())?;
        inner.process_lock = Some(lock);
        drop(inner);
        // Run only after acquiring the single-process authority. A second app
        // instance must never delete snapshots owned by the active process.
        sweep_stale_web_attachment_uploads();
        sweep_stale_web_session_downloads();
        Ok(())
    }

    fn start_web_session_transfer_reaper(&self) {
        let inner = Arc::downgrade(&self.inner);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(WEB_SESSION_TRANSFER_REAPER_INTERVAL);
                let Some(inner) = inner.upgrade() else {
                    break;
                };
                prune_expired_web_session_transfers(&mut inner.lock());
            }
        });
    }

    /// Start independent retry workers for every retired endpoint. A queue
    /// entry matching the current config is a prepared-but-not-yet-committed
    /// rotation/stop and must remain dormant. This makes crashes between
    /// queueing and replacing/removing `web-access.json` safe.
    fn replay_pending_revocations(&self) -> Result<(), String> {
        let ledger = load_pending_revocations_for_replay()?;
        let current = load_config()?;
        // The ledger loaded at replay is this process's authoritative copy: acknowledgement
        // persists the whole of it — see finish_pending_revocation. Round-14 review minor:
        // loading changed to **union with memory-priority** — same-key same-value entries
        // defer to the ledger, while entries the ledger lacks but memory holds (siblings
        // left memory-only after a salvage degradation's re-persist failed) must be kept,
        // otherwise the replay of the second lifecycle call after start would overwrite
        // memory wholesale with the disk snapshot and degraded entries would vanish from
        // both sides at once; same-key different-value conflicts keep the memory version
        // (it is exactly what the in-flight worker is retrying). When the ack worker
        // deletes an entry, memory and disk are updated in the same batch
        // (finish_pending_revocation persists the whole ledger), so there is no
        // "memory already acked, disk still holds the residue" resurrection surface; the
        // lifecycle lock serializes replay and acknowledgement.
        {
            let mut inner = self.inner.lock();
            let held = std::mem::take(&mut inner.pending_revocation_backlog);
            let mut merged = held;
            for entry in &ledger.entries {
                if !merged.iter().any(|existing| {
                    pending_revocation_key(existing) == pending_revocation_key(entry)
                }) {
                    merged.push(entry.clone());
                }
            }
            // Round-29 review MINOR-1: the union of salvage-leftover memory entries
            // and the disk ledger can cross the validation cap — keep the cap
            // binding right where the merged backlog is adopted.
            cap_pending_revocation_backlog(&mut merged);
            inner.pending_revocation_backlog = merged;
        }
        for pending in eligible_pending_revocations(&ledger, current.as_ref()) {
            let key = pending_revocation_key(&pending);
            {
                let mut inner = self.inner.lock();
                if !inner.pending_revocations_in_flight.insert(key.clone()) {
                    continue;
                }
            }

            let mut receiver = relay_client::spawn_revocation(
                pending.relay_url.clone(),
                pending.endpoint_id.clone(),
                pending.desktop_secret.clone(),
            );
            let manager = self.clone();
            tauri::async_runtime::spawn(async move {
                let mut acknowledged = false;
                while let Some(inbound) = receiver.recv().await {
                    if let RelayInbound::Message(text) = inbound {
                        match serde_json::from_str::<Value>(&text) {
                            Ok(value)
                                if relay_client::terminal_relay_message(
                                    &value,
                                    &pending.endpoint_id,
                                ) =>
                            {
                                acknowledged = true;
                                break;
                            }
                            Ok(_) => {}
                            Err(error) => {
                                eprintln!("[web-access] queued revocation JSON ignored: {error}")
                            }
                        }
                    }
                }
                manager.finish_pending_revocation(pending, key, acknowledged);
            });
        }
        Ok(())
    }

    /// Round-12 review P2: `queue_pending_revocation` only writes to disk and does not
    /// sync the in-process backlog — an entry just queued into the disk-authoritative
    /// ledger only enters memory when `replay_pending_revocations` re-syncs after the
    /// queue. If that replay fails (refresh's `?` aborts here; stop logs and continues),
    /// any later ack's whole-ledger persist would overwrite the disk with a stale backlog
    /// lacking the entry, permanently deleting the just-queued revocation record and
    /// keeping the old endpoint's `desktop_secret` valid on the relay indefinitely —
    /// exactly the asset the whole-ledger persist semantics protect. After a successful
    /// queue (the caller holds the lifecycle lock) the backlog is advanced directly in
    /// sync, demoting the replay re-sync to a defensive measure. Same-key old entries are
    /// removed first: queue returns Err for a same-key different-content entry, so
    /// reaching here means same content and the convergence direction is unique.
    fn sync_pending_revocation_backlog(&self, config: &WebAccessConfig) {
        let pending = PendingRevocation::from(config);
        let key = pending_revocation_key(&pending);
        let mut inner = self.inner.lock();
        inner
            .pending_revocation_backlog
            .retain(|entry| pending_revocation_key(entry) != key);
        inner.pending_revocation_backlog.push(pending);
        cap_pending_revocation_backlog(&mut inner.pending_revocation_backlog);
    }

    fn finish_pending_revocation(
        &self,
        pending: PendingRevocation,
        key: String,
        acknowledged: bool,
    ) {
        let _lifecycle = self.lifecycle.lock();
        if acknowledged {
            // Round-11 review minor: acknowledgement persists the whole of the in-process
            // authoritative ledger (`pending_revocation_backlog`), not "re-read disk →
            // remove one → rewrite" — after a salvage degradation there is no ledger on
            // disk, and a re-read-style acknowledgement would take an empty ledger and
            // clear sibling entries still in memory, losing this whole batch of pending
            // revocations across a restart (the old endpoint stays joinable on the relay
            // side). On persist failure the full set (including this entry) goes back into
            // memory for the next lifecycle's retry; replay/queue keep the ledger in
            // lockstep with disk writes, and restarts rebuild it from disk. The lifecycle
            // lock serializes both ack workers' take-store windows, so nothing is
            // concurrently lost.
            let endpoint_id = pending.endpoint_id.clone();
            let mut remaining = {
                let mut inner = self.inner.lock();
                std::mem::take(&mut inner.pending_revocation_backlog)
            };
            remaining.retain(|entry| entry != &pending);
            let mut ledger = PendingRevocationLedger::default();
            ledger.entries = remaining.iter().cloned().collect();
            match persist_pending_revocations(&ledger) {
                Ok(()) => {
                    self.inner.lock().pending_revocation_backlog = remaining;
                }
                Err(error) => {
                    remaining.push(pending);
                    self.inner.lock().pending_revocation_backlog = remaining;
                    eprintln!(
                        "[web-access] persist revocation acknowledgement for {endpoint_id} \
                         failed ({error}); the entry stays queued for the next lifecycle replay"
                    );
                }
            }
        }
        self.inner.lock().pending_revocations_in_flight.remove(&key);
    }

    /// Mint a short-lived capability for a directory that the host-file
    /// picker has already listed through `validate_browsable_path`. The
    /// browser may display the host path, but code-session creation receives
    /// only this opaque, endpoint-bound, one-shot handle.
    pub fn issue_web_workspace_grant(&self, raw_path: &str) -> Result<String, String> {
        let authorized_endpoint_id = {
            let inner = self.inner.lock();
            let endpoint = inner
                .endpoint
                .as_ref()
                .ok_or_else(|| "Web access endpoint is not active".to_string())?;
            require_host_workspace_authorization(endpoint.config.allow_host_workspace)?;
            endpoint.config.endpoint_id.clone()
        };
        let path = crate::features::files::file_ingest::validate_browsable_path(raw_path)?;
        if !path.is_dir() {
            return Err(format!(
                "Web code workspace must be a directory: {}",
                path.display()
            ));
        }
        let identity = WebWorkspaceIdentity::capture(&path)?;
        let mut inner = self.inner.lock();
        let endpoint = inner
            .endpoint
            .as_ref()
            .ok_or_else(|| "Web access endpoint is not active".to_string())?;
        require_host_workspace_authorization(endpoint.config.allow_host_workspace)?;
        if endpoint.config.endpoint_id != authorized_endpoint_id {
            return Err("Web access endpoint changed while authorizing workspace".to_string());
        }
        let endpoint_id = endpoint.config.endpoint_id.clone();
        let handle = loop {
            let candidate = format!(
                "workspace_{}",
                crate::features::remote_control::short_token(32)
            );
            if !inner.web_workspace_grants.contains(&candidate) {
                break candidate;
            }
        };
        inner.web_workspace_grants.issue(
            handle.clone(),
            endpoint_id,
            path,
            identity,
            Instant::now(),
        );
        Ok(handle)
    }

    /// Atomically reserve a workspace capability while Session creation runs.
    /// Invalid, expired, cross-endpoint, and concurrent replays still consume
    /// or fail the one-shot handle. Only the original reservation can be
    /// restored after a failed creation, for the same endpoint and TTL.
    pub(crate) fn reserve_web_workspace_grant(
        &self,
        handle: &str,
    ) -> Result<WebWorkspaceGrantReservation, String> {
        let mut inner = self.inner.lock();
        let endpoint_id = inner
            .endpoint
            .as_ref()
            .map(|endpoint| endpoint.config.endpoint_id.clone())
            .ok_or_else(|| "Web access endpoint is not active".to_string())?;
        let reservation =
            inner
                .web_workspace_grants
                .reserve(handle, &endpoint_id, Instant::now())?;
        drop(inner);
        reservation.revalidate()
    }

    /// Authorization-flag recheck at bind/redeem time: the user may have revoked
    /// authorization between reserve and session creation; within the one-shot handle's
    /// TTL window a pre-revocation reservation must not bind the host directory into a
    /// new session.
    pub(crate) fn require_host_workspace_authorized(&self) -> Result<(), String> {
        let inner = self.inner.lock();
        let authorized = inner
            .endpoint
            .as_ref()
            .is_some_and(|endpoint| endpoint.config.allow_host_workspace);
        if authorized {
            Ok(())
        } else {
            Err("Web workspace authorization was revoked".to_string())
        }
    }

    pub(crate) fn restore_web_workspace_grant(
        &self,
        reservation: WebWorkspaceGrantReservation,
    ) -> bool {
        let mut inner = self.inner.lock();
        let Some(allow_host_workspace) = inner
            .endpoint
            .as_ref()
            .map(|endpoint| endpoint.config.allow_host_workspace)
        else {
            return false;
        };
        let Some(endpoint_id) = inner
            .endpoint
            .as_ref()
            .map(|endpoint| endpoint.config.endpoint_id.clone())
        else {
            return false;
        };
        inner.web_workspace_grants.restore(
            reservation,
            &endpoint_id,
            Instant::now(),
            allow_host_workspace,
        )
    }

    /// Keep parsed attachment contents on the desktop. The browser receives
    /// only an opaque handle and compact display metadata, so neither the
    /// ingest response nor the subsequent chat request can overflow Relay.
    pub fn cache_web_attachment(
        &self,
        result: crate::features::files::file_ingest::IngestResult,
    ) -> Result<WebAttachmentSummary, String> {
        self.cache_web_attachment_entry(result, None)
    }

    /// Same semantics as `cache_web_attachment`, but the attachment comes from a
    /// browser-local upload: its desktop staging directory is registered and deleted
    /// together with the cache entry's death (consumption/eviction/clearing).
    pub fn cache_web_attachment_from_upload(
        &self,
        result: crate::features::files::file_ingest::IngestResult,
        upload_dir: PathBuf,
    ) -> Result<WebAttachmentSummary, String> {
        self.cache_web_attachment_entry(result, Some(upload_dir))
    }

    fn cache_web_attachment_entry(
        &self,
        result: crate::features::files::file_ingest::IngestResult,
        upload_dir: Option<PathBuf>,
    ) -> Result<WebAttachmentSummary, String> {
        let bytes = serde_json::to_vec(&result)
            .map_err(|error| format!("serialize Web attachment: {error}"))?
            .len();
        if bytes > MAX_WEB_ATTACHMENT_BYTES {
            return Err(format!(
                "解析后的附件超过远程控制桌面缓存上限 {} MiB",
                MAX_WEB_ATTACHMENT_BYTES / (1024 * 1024)
            ));
        }
        let handle = format!(
            "attachment_{}",
            crate::features::remote_control::short_token(32)
        );
        let summary = WebAttachmentSummary {
            handle: handle.clone(),
            kind: result.kind.clone(),
            basename: result.basename.clone(),
            token_estimate: result.token_estimate,
            byte_size: result.byte_size,
            warning: result.warning.clone(),
        };
        let mut inner = self.inner.lock();
        let mut eviction_attempts = inner.web_attachment_order.len();
        while (inner.web_attachments.len() >= MAX_WEB_ATTACHMENTS
            || inner.web_attachment_bytes.saturating_add(bytes) > MAX_WEB_ATTACHMENTS_TOTAL_BYTES)
            && eviction_attempts > 0
        {
            eviction_attempts -= 1;
            let Some(oldest) = inner.web_attachment_order.pop_front() else {
                break;
            };
            if inner
                .web_attachments
                .get(&oldest)
                .is_some_and(|cached| cached.reservation_id.is_some())
            {
                inner.web_attachment_order.push_back(oldest);
                continue;
            }
            if let Some(removed) = inner.web_attachments.remove(&oldest) {
                inner.web_attachment_bytes =
                    inner.web_attachment_bytes.saturating_sub(removed.bytes);
                remove_web_attachment_upload_dir(&removed);
            }
        }
        if inner.web_attachment_bytes.saturating_add(bytes) > MAX_WEB_ATTACHMENTS_TOTAL_BYTES {
            return Err("远程控制附件缓存已满，请移除一个附件后重试".into());
        }
        inner.web_attachment_bytes = inner.web_attachment_bytes.saturating_add(bytes);
        inner.web_attachment_order.push_back(handle.clone());
        inner.web_attachments.insert(
            handle,
            CachedWebAttachment {
                result,
                bytes,
                reservation_id: None,
                discard_requested: false,
                upload_dir,
            },
        );
        Ok(summary)
    }

    /// Atomically reserve opaque handles while a chat submission crosses the
    /// async engine admission boundary. A rejected turn releases them for the
    /// same browser to retry; only an accepted turn consumes them.
    pub fn reserve_web_attachments(
        &self,
        handles: &[String],
    ) -> Result<
        (
            String,
            Vec<crate::features::files::file_ingest::IngestResult>,
        ),
        String,
    > {
        if handles.len() > MAX_WEB_ATTACHMENTS {
            return Err("单条消息中的远程控制附件过多".into());
        }
        let mut inner = self.inner.lock();
        let mut unique = HashSet::new();
        for handle in handles {
            if !unique.insert(handle.as_str()) {
                return Err("远程控制附件句柄重复".into());
            }
            // Round-11 review minor: the refusal string only echoes the desktop-minted
            // shape (`attachment_<alphanumeric>`). Caller bytes otherwise enter the error
            // chain verbatim, and after fold_web_chat_error's busy-anchor folding a handle
            // containing a `reserve Web chat turn: …` segment would disguise a hard failure
            // as transient busyness, driving the phone to remotely reconcile a turn that
            // was never admitted.
            if !attachment_handle_is_well_formed(handle) {
                return Err("远程控制附件句柄不存在或已过期".into());
            }
            if !inner.web_attachments.contains_key(handle) {
                return Err(format!("远程控制附件句柄不存在或已过期：{handle}"));
            }
            if inner
                .web_attachments
                .get(handle)
                .and_then(|cached| cached.reservation_id.as_ref())
                .is_some()
            {
                return Err(format!("远程控制附件句柄已在使用：{handle}"));
            }
        }
        let reservation_id = format!(
            "attachment_use_{}",
            crate::features::remote_control::short_token(24)
        );
        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            // Already validated via contains_key in the critical section above;
            // still return an error as a fallback instead of panicking.
            let Some(cached) = inner.web_attachments.get_mut(handle) else {
                return Err(format!("远程控制附件句柄不存在或已过期：{handle}"));
            };
            cached.reservation_id = Some(reservation_id.clone());
            results.push(cached.result.clone());
        }
        Ok((reservation_id, results))
    }

    /// Discard an opaque attachment handle after its browser chip is removed.
    /// Unknown handles are accepted for idempotency. A handle currently owned
    /// by a chat turn is marked for deletion and cleaned when that reservation
    /// finishes, regardless of whether the turn succeeds or fails.
    pub fn discard_web_attachment(&self, handle: &str) -> Result<(), String> {
        if !crate::features::files::attachment_upload::validate_opaque_token(
            handle,
            12,
            128,
            Some("attachment_"),
        ) {
            return Err("远程控制附件句柄无效".into());
        }
        request_web_attachment_discard(&mut self.inner.lock(), handle);
        Ok(())
    }

    pub fn finish_web_attachment_reservation(
        &self,
        reservation_id: &str,
        handles: &[String],
        consume: bool,
    ) -> Result<(), String> {
        finish_web_attachment_reservation_inner(
            &mut self.inner.lock(),
            reservation_id,
            handles,
            consume,
        )
    }

    /// Accumulates one bounded chunk of a browser-local file; the final chunk's `commit`
    /// returns the complete file name and bytes. The per-file cap is tightened to
    /// `file_ingest::MAX_FILE_BYTES` so the transfer does not finish 20 MiB only to yield
    /// an oversize degraded attachment.
    pub fn append_web_attachment_upload(
        &self,
        upload_id: &str,
        file_name: &str,
        offset: usize,
        total: usize,
        data: &[u8],
        commit: bool,
        sha256: Option<&str>,
    ) -> Result<Option<(String, Vec<u8>)>, String> {
        append_web_attachment_upload_chunk(
            &mut self.inner.lock(),
            upload_id,
            file_name,
            offset,
            total,
            data,
            commit,
            sha256,
        )
    }

    /// Cancels an in-progress browser-local upload and releases its memory buffer.
    /// Idempotent for unknown or already-finished ids — a late cancel must not error.
    pub fn abort_web_attachment_upload(&self, upload_id: &str) {
        discard_web_attachment_upload(&mut self.inner.lock(), upload_id);
    }

    pub fn begin_web_session_download(
        &self,
        session_id: &str,
        requested_download_id: Option<&str>,
        reserved_bytes: usize,
    ) -> Result<WebSessionDownloadReservation, String> {
        if reserved_bytes > MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES {
            return Err(format!(
                "会话数据超过远程控制 {} MiB 上限",
                MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES / (1024 * 1024)
            ));
        }
        let download_id = match requested_download_id
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            Some(download_id) if is_owned_session_download_id(download_id) => {
                download_id.to_string()
            }
            Some(_) => return Err("invalid requested Session download id".into()),
            None => format!(
                "download_{}",
                crate::features::remote_control::short_token(32)
            ),
        };
        let download_dir = open_web_session_downloads_base()?;
        let mut inner = self.inner.lock();
        prune_expired_web_session_transfers(&mut inner);
        if inner.web_session_downloads.contains_key(&download_id) {
            return Err("requested Session download id is already active".into());
        }
        // Round-25 review MINOR: eviction previously hooked only into the slot gate — the
        // byte gate (MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES) failing wedged retries just the
        // same: one ≥128 MiB lost-response orphan pushed active_bytes over the total cap
        // while the slot count stayed under, so eviction never fired and retries hard-failed
        // to the TTL Reaper (the same failure surface this fix removes, just in another
        // dimension). Eviction now hooks uniformly into the capacity gate (slots + bytes):
        // on a capacity-check failure the oldest expired ready orphans are evicted first and
        // the check re-run, until capacity is satisfied or nothing is evictable (failing with
        // the capacity gate's real error, original copy kept); each victim gets one diagnostic
        // — eviction was previously silent, with the phone seeing only "does not exist or has
        // expired".
        let mut evictions = 0_usize;
        // Round-28 review MINOR-1: a capacity-gate failure after eviction and a serialized-
        // file creation failure previously exited the function via `?` directly, losing the
        // diagnostics of evictions that already happened (contradicting this section's own
        // "one diagnostic per victim"). Funneled into one closure exit: on success or failure
        // the lock is released first, per-count diagnostics are filled in, and only then is
        // the result propagated.
        let begin = (|| -> Result<WebSessionDownloadReservation, String> {
            while ensure_web_session_download_capacity(&inner, reserved_bytes).is_err() {
                if !transfer::evict_oldest_ready_session_download(&mut inner) {
                    ensure_web_session_download_capacity(&inner, reserved_bytes)?;
                }
                evictions += 1;
            }
            let file = download_dir
                .create_delete_on_close_file(&format!("{download_id}.json"))
                .map_err(|error| format!("create serialized Session download: {error}"))?;
            let writer = file
                .try_clone()
                .map_err(|error| format!("clone serialized Session download handle: {error}"))?;
            let reservation_id = crate::features::remote_control::short_token(24);
            inner
                .web_session_download_order
                .push_back(download_id.clone());
            inner.web_session_downloads.insert(
                download_id.clone(),
                WebSessionDownload {
                    session_id: session_id.to_string(),
                    reservation_id: reservation_id.clone(),
                    file,
                    reserved_bytes,
                    total: 0,
                    ready: false,
                    last_touched: Instant::now(),
                },
            );
            Ok(WebSessionDownloadReservation {
                manager: self.clone(),
                download_id,
                session_id: session_id.to_string(),
                reservation_id,
                writer,
                committed: false,
            })
        })();
        // Round-27 review M2: diagnostics persist after the manager lock is released (the
        // same caliber as the cancelled point's "record outside the lock") — once the
        // allowlist takes effect, record_backend performs append+flush file I/O and must not
        // run inside the eviction loop's critical section.
        drop(inner);
        for _ in 0..evictions {
            crate::features::sessions::diagnostics::record_backend(
                "web_session_download_evicted",
                serde_json::json!({ "trigger": "begin_capacity_gate" }),
            );
        }
        begin
    }

    fn commit_web_session_download(
        &self,
        download_id: &str,
        session_id: &str,
        reservation_id: &str,
    ) -> Result<(), String> {
        let mut inner = self.inner.lock();
        prune_expired_web_session_transfers(&mut inner);
        let reserved = inner
            .web_session_downloads
            .get(download_id)
            .ok_or_else(|| "远程控制会话下载预留已过期".to_string())?;
        if reserved.session_id != session_id || reserved.reservation_id != reservation_id {
            return Err("远程控制会话下载预留已变化".into());
        }
        let total = reserved
            .file
            .metadata()
            .map_err(|error| format!("检查远程控制会话下载数据失败：{error}"))?
            .len();
        let total = usize::try_from(total).map_err(|_| "远程控制会话下载数据过大".to_string())?;
        if total > MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES {
            return Err(format!(
                "会话数据超过远程控制 {} MiB 上限",
                MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES / (1024 * 1024)
            ));
        }
        let other_bytes = inner
            .web_session_downloads
            .iter()
            .filter(|(id, _)| id.as_str() != download_id)
            .map(|(_, download)| download.reserved_bytes)
            .sum::<usize>();
        if other_bytes.saturating_add(total) > MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES {
            return Err(format!(
                "进行中的远程控制会话下载将超过 {} MiB 总上限",
                MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES / (1024 * 1024)
            ));
        }
        // Reservation presence was verified in the same critical section
        // above; still return an error as a fallback instead of panicking.
        let Some(download) = inner.web_session_downloads.get_mut(download_id) else {
            return Err("remote-control session download reservation is missing or expired".into());
        };
        download.reserved_bytes = total;
        download.total = total;
        download.ready = true;
        download.last_touched = Instant::now();
        Ok(())
    }

    fn remove_web_session_download(
        &self,
        download_id: &str,
        expected_reservation_id: Option<&str>,
    ) {
        let _removed = {
            let mut inner = self.inner.lock();
            let matches = inner
                .web_session_downloads
                .get(download_id)
                .is_some_and(|download| {
                    expected_reservation_id
                        .is_none_or(|reservation_id| download.reservation_id == reservation_id)
                });
            if matches {
                inner
                    .web_session_download_order
                    .retain(|id| id != download_id);
                inner.web_session_downloads.remove(download_id)
            } else {
                None
            }
        };
    }

    /// Release an incomplete browser Session download explicitly. The opaque
    /// token remains bound to its Session so one browser context cannot cancel
    /// another Session's transfer even if it supplies a mismatched id.
    pub fn cancel_web_session_download(
        &self,
        download_id: &str,
        session_id: &str,
    ) -> Result<bool, String> {
        let removed = {
            let mut inner = self.inner.lock();
            take_web_session_download(&mut inner, download_id, session_id)?
        };
        if let Some(download) = removed {
            crate::features::sessions::diagnostics::record_backend(
                "web_session_download_cancelled",
                serde_json::json!({
                    "session_id": session_id,
                    "download_id": download_id,
                    "total_bytes": download.total,
                    "ready": download.ready,
                }),
            );
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn read_web_session_download(
        &self,
        download_id: &str,
        session_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<WebSessionDownloadChunk, String> {
        read_web_session_download_chunk(
            &mut self.inner.lock(),
            download_id,
            session_id,
            offset,
            limit,
        )
    }

    pub fn status(&self) -> WebAccessStatus {
        let inner = self.inner.lock();
        if let Some(endpoint) = &inner.endpoint {
            WebAccessStatus {
                active: true,
                endpoint_id: Some(endpoint.config.endpoint_id.clone()),
                url: Some(endpoint.url.clone()),
                qr_data_url: crate::features::connectors::connector_cli::make_qr(&endpoint.url),
                status: endpoint.status,
                relay_url: endpoint.config.relay_url.clone(),
                web_client_connected: endpoint.web_client_connected,
                host_workspace_authorized: endpoint.config.allow_host_workspace,
                last_error: endpoint.last_error.clone(),
                last_error_code: endpoint.last_error_code.clone(),
            }
        } else {
            WebAccessStatus {
                active: false,
                endpoint_id: None,
                url: None,
                qr_data_url: None,
                status: inner.idle_status,
                relay_url: remote_relay_ws_url(),
                web_client_connected: false,
                host_workspace_authorized: false,
                // Round-15 review NIT: the old idle_error field was never assigned a
                // non-None value (always None + or_else) and was removed; the only source of
                // idle errors is policy_error.
                last_error: self.policy_error.clone(),
                last_error_code: None,
            }
        }
    }

    pub fn forward_local_event(&self, event: &str, payload: Value) {
        if let Err(error) = self.publish_event_inner(event, payload, EventSource::Rust) {
            eprintln!("[web-access] forward {event} failed: {error}");
        }
    }

    /// Whether the remote-control endpoint is active at all. Producers use
    /// this as the journal gate: while the endpoint is active, events are
    /// journaled even if the browser is momentarily disconnected, so replay
    /// on reconnect covers the disconnect window. When remote control is off,
    /// producers skip both the projection cost and the journal entirely.
    pub(crate) fn has_active_web_transport(&self) -> bool {
        self.inner.lock().endpoint.is_some()
    }

    pub fn publish_frontend_event(&self, event: &str, payload: Value) -> Result<(), String> {
        self.publish_event_inner(event, payload, EventSource::Frontend)
    }

    /// Register a concrete desktop WebView generation. Unacknowledged work may
    /// be dispatched to a new generation; work whose begin ACK was persisted
    /// is never re-run and is resolved as `outcome_unknown` instead.
    pub fn mark_frontend_ready(&self, generation: String) -> Result<(), String> {
        validate_bridge_generation(&generation)?;
        let (actions, subscriptions) = {
            let mut inner = self.inner.lock();
            let actions = prepare_bridge_generation(&mut inner, &generation);
            let subscriptions = inner.subscriptions.iter().cloned().collect::<Vec<_>>();
            (actions, subscriptions)
        };
        let mut first_error = None;
        for action in actions {
            match action {
                RpcRequestAction::Dispatch(dispatch) => {
                    if let Err(error) = self.emit_rpc_dispatch(dispatch) {
                        first_error.get_or_insert(error);
                    }
                }
                RpcRequestAction::Respond(sender, response) => {
                    if sender.try_send(RelayOutbound::Message(response)).is_err() {
                        first_error
                            .get_or_insert_with(|| "Web access relay task has stopped".to_string());
                    }
                }
                // `prepare_bridge_generation` never emits the None variant.
                RpcRequestAction::None => {}
            }
        }
        // The browser may reconnect and subscribe before a restarted desktop
        // WebView has installed its forwarding listeners. Replaying the set
        // after the main bridge readiness barrier closes that lost-event race.
        for event in subscriptions {
            if let Err(error) = self.app.emit_to(
                "main",
                "web_access:event_subscribe",
                json!({ "event": event }),
            ) {
                first_error.get_or_insert_with(|| {
                    format!("restore Web access frontend subscription: {error}")
                });
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    /// Persist the side-effect barrier before allowing the desktop WebView to
    /// invoke the requested command. Returning `false` means this dispatch was
    /// already acknowledged and must not be executed again.
    pub fn begin_rpc(&self, request_id: &str, generation: &str) -> Result<bool, String> {
        validate_bridge_generation(generation)?;
        let mut inner = self.inner.lock();
        if inner.bridge_generation.as_deref() != Some(generation) {
            return Err("stale WebView bridge generation".to_string());
        }
        let (fingerprint, lease_id, already_acknowledged) = match inner.rpc_cache.get(request_id) {
            Some(RpcCacheEntry::Pending(pending)) => {
                if pending.dispatched_generation.as_deref() != Some(generation) {
                    return Err("RPC was not dispatched to this WebView generation".to_string());
                }
                (
                    pending.fingerprint.clone(),
                    pending.lease_id.clone(),
                    pending.acknowledged_generation.is_some(),
                )
            }
            Some(RpcCacheEntry::Complete { .. }) => return Ok(false),
            None => return Err(format!("unknown Web RPC request: {request_id}")),
        };
        if already_acknowledged {
            return Ok(false);
        }

        let mut next_ledger = inner.rpc_ledger.clone();
        next_ledger.acknowledge(request_id, &fingerprint)?;
        if let Err(error) = persist_rpc_ledger(&next_ledger) {
            // A persist failure is transient (disk full/EACCES etc.), adjudicated the same
            // way as handle_rpc_request's admission path: respond but do not write the
            // Complete cache. The command has not executed, so a retry with the same
            // client_request_id redisperses through the Pending arm and re-runs begin (the
            // settle exit clears the dispatch generation marker); caching it as Complete
            // would replay this transient failure to the phone forever (replay poisoning).
            Self::settle_begin_persist_failure(&mut inner, request_id, &lease_id, &error);
            return Ok(false);
        }
        inner.rpc_ledger = next_ledger;
        let Some(RpcCacheEntry::Pending(pending)) = inner.rpc_cache.get_mut(request_id) else {
            return Err(format!("unknown Web RPC request: {request_id}"));
        };
        pending.acknowledged_generation = Some(generation.to_string());
        Ok(true)
    }

    pub fn complete_rpc(
        &self,
        request_id: &str,
        generation: &str,
        ok: bool,
        result: Option<Value>,
        error: Option<String>,
        error_code: Option<String>,
        error_category: Option<String>,
    ) -> Result<(), String> {
        validate_bridge_generation(generation)?;
        let (sender, response) = {
            let mut inner = self.inner.lock();
            if inner.bridge_generation.as_deref() != Some(generation) {
                return Err("stale WebView bridge generation".to_string());
            }
            let (lease_id, fingerprint) = match inner.rpc_cache.get(request_id) {
                Some(RpcCacheEntry::Pending(pending)) => {
                    if pending.acknowledged_generation.as_deref() != Some(generation) {
                        return Err("Web RPC completion arrived before its begin ACK".to_string());
                    }
                    (pending.lease_id.clone(), pending.fingerprint.clone())
                }
                Some(RpcCacheEntry::Complete {
                    fingerprint,
                    completion,
                    ..
                }) => {
                    // Round-5 review M1: the TTL sweep writes timeout placeholders first for
                    // requests still executing (or having just finished). A real completion must
                    // replace the placeholder instead of being swallowed by idempotency, or a
                    // phone replaying this id gets a stale timeout ("released") and the real
                    // result never reaches it. Only a placeholder is replaceable: after the
                    // placeholder is replaced, a second complete still takes the idempotent-ignore
                    // path; real terminal states like deterministic refusals can never be covered
                    // by a fabricated complete (the command was never dispatched, so no real
                    // completion exists).
                    // The replacement deliberately "stores without sending": the phone already
                    // received the timeout terminal state, and proactively double-sending the
                    // same client_request_id would break the single-response contract; when the
                    // phone replays the id, the Complete arm replays the real result substituted
                    // here.
                    if Self::rpc_completion_is_in_flight_timeout(completion) {
                        let fingerprint = fingerprint.clone();
                        inner.rpc_cache.insert(
                            request_id.to_string(),
                            RpcCacheEntry::Complete {
                                fingerprint,
                                completion: bounded_rpc_completion(
                                    ok,
                                    result,
                                    error,
                                    error_code,
                                    error_category,
                                ),
                                swept_acknowledged: false,
                            },
                        );
                        // Round-7 review: the replacement insertion keeps the same accounting
                        // symmetry as the Pending→Complete arm — prune by capacity right after
                        // inserting, so one ≤2MiB late real result cannot push cache bytes over
                        // the cap until the next insertion.
                        prune_rpc_cache(&mut inner);
                    }
                    return Ok(());
                }
                None => return Err(format!("unknown Web RPC request: {request_id}")),
            };
            let completion = bounded_rpc_completion(ok, result, error, error_code, error_category);
            inner.rpc_cache.insert(
                request_id.to_string(),
                RpcCacheEntry::Complete {
                    fingerprint,
                    completion: completion.clone(),
                    swept_acknowledged: false,
                },
            );
            prune_rpc_cache(&mut inner);
            let endpoint = inner
                .endpoint
                .as_ref()
                .ok_or_else(|| "Web access endpoint is not active".to_string())?;
            (
                endpoint.sender.clone(),
                rpc_response(
                    &endpoint.config.endpoint_id,
                    &lease_id,
                    request_id,
                    &completion,
                ),
            )
        };
        sender
            .try_send(RelayOutbound::Message(response))
            .map_err(|_| "Web access relay task has stopped".to_string())
    }

    fn ensure_policy(&self) -> Result<(), String> {
        match &self.policy_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn activate(&self, config: WebAccessConfig) -> Result<(), String> {
        let rpc_ledger = load_or_initialize_rpc_ledger(&config.endpoint_id)?;
        let url = public_url(&config);
        let (sender, mut receiver) = relay_client::spawn(config.clone());
        {
            let mut inner = self.inner.lock();
            inner.idle_status = WebAccessStatusKind::Idle;
            inner.rpc_ledger = rpc_ledger;
            inner.rpc_cache.clear();
            inner.rpc_order.clear();
            inner.endpoint = Some(ActiveEndpoint {
                config: config.clone(),
                url,
                status: WebAccessStatusKind::ConnectingRelay,
                web_client_connected: false,
                client_ready: false,
                lease_id: None,
                last_lease_id: None,
                fresh_baseline_seq: None,
                last_error: None,
                last_error_code: None,
                sender,
            });
        }
        self.emit_status();

        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(inbound) = receiver.recv().await {
                match inbound {
                    RelayInbound::Message(text) => match serde_json::from_str::<Value>(&text) {
                        Ok(value) => manager.handle_relay_message(value),
                        Err(error) => {
                            eprintln!("[web-access] queued relay JSON ignored: {error}")
                        }
                    },
                    RelayInbound::Connection {
                        endpoint_id,
                        connected,
                        error,
                    } => manager.handle_connection(&endpoint_id, connected, error),
                }
            }
        });
        Ok(())
    }

    fn handle_connection(&self, endpoint_id: &str, connected: bool, error: Option<String>) {
        {
            let mut inner = self.inner.lock();
            let Some(endpoint) = inner.endpoint.as_mut() else {
                return;
            };
            if endpoint.config.endpoint_id != endpoint_id {
                return;
            }
            endpoint.status = WebAccessStatusKind::ConnectingRelay;
            if connected {
                endpoint.last_error = None;
                endpoint.last_error_code = None;
            } else {
                endpoint.web_client_connected = false;
                endpoint.lease_id = None;
                endpoint.fresh_baseline_seq = None;
                // Round-14 review NIT: bound through the same caliber as the error-frame
                // entrance (locally formatted text is normally far below the bound; one
                // consistent funnel guards against future relay-side free text entering
                // through this arm).
                endpoint.last_error = error.map(bound_relay_text);
                // Field contract: any error write carrying no code must clear the old one, so
                // an old recovery code is never mapped onto new error text (round-5 review minor).
                endpoint.last_error_code = None;
            }
        }
        self.emit_status();
    }

    fn handle_relay_message(&self, value: Value) {
        let endpoint_id = value
            .get("endpoint_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let message_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let error_code = if message_type == "error" {
            value
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or_default()
        } else {
            ""
        };
        // The relay's reject frame carries no endpoint_id (it applies to this socket's
        // registration); relay_client stamps this socket's registered endpoint before
        // forwarding, making the explicit-match gate effective (a late terminal frame in a
        // rotation race is dropped from now on and can never hurt the new endpoint). The
        // allow branch for an empty endpoint_id remains only as defense-in-depth for
        // un-stamped frames; all other error frames and every other message still require
        // explicitly matching the current endpoint, preventing cross-endpoint frame
        // crosstalk or transient errors jittering the state.
        let terminal_registration_error =
            endpoint_id.is_empty() && TERMINAL_REGISTRATION_ERROR_CODES.contains(&error_code);
        if !self.is_current_endpoint(endpoint_id) && !terminal_registration_error {
            return;
        }
        match value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "desktop_endpoint_registered" => self.handle_registered(&value),
            "web_client_connected" => self.handle_web_connected(&value),
            "web_client_disconnected" => self.handle_web_disconnected(&value),
            "rpc_request" => self.handle_rpc_request(&value),
            "event_subscribe" | "event_unsubscribe" => {
                // Round-11 review minor: subscription frames share rpc_request's takeover-race
                // surface (the PR gated rpc_request/client_ready but missed subscription frames)
                // — a late dead-lease subscription frame (the relay stamps the lease at socket
                // time) would let the unsubscribe strip a subscription the new client just
                // installed, and the subscription backfill would replace the wrong list.
                // Round-14 review minor: the gate check does not take its own lock here —
                // verifying the lease first and then admitting would leave a takeover window
                // between the two locks (the shape this file's rpc-gate comment names); within
                // that window a takeover lets the late frame still strip a subscription the new
                // lease just installed. The gate check moved into handle_event_subscription,
                // under the same lock and the same acquisition as the subscription change.
                self.handle_event_subscription(
                    &value,
                    value.get("type").and_then(Value::as_str) == Some("event_subscribe"),
                );
            }
            "client_ready" => self.synchronize_client_from_value(&value),
            "desktop_endpoint_revoked" => self.handle_revoked(endpoint_id),
            "desktop_endpoint_replaced" => self.handle_replaced(endpoint_id),
            "error" => {
                if error_code == "endpoint_revoked" {
                    // The relay explicitly says the registration hit a tombstone: the reject
                    // frame carries no endpoint_id, so resolve the current endpoint first and
                    // then run the same full cleanup as desktop_endpoint_revoked — otherwise
                    // the local config would retry a registration that can never succeed.
                    // (Take the id out before calling; the lock's temporary must not live for
                    // the whole if-let statement.)
                    let current = self
                        .inner
                        .lock()
                        .endpoint
                        .as_ref()
                        .map(|endpoint| endpoint.config.endpoint_id.clone());
                    if let Some(current) = current {
                        self.handle_revoked(&current);
                    }
                    return;
                }
                let relay_message = value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("relay rejected Web access request");
                // When a registration is deterministically refused, run_loop has stopped
                // reconnecting; provide an actionable recovery path, and the reason must stay
                // sticky (the run_loop termination path no longer sends Connection events, so
                // last_error is never overwritten by "relay connection lost").
                self.set_endpoint_error_with_code(
                    relay_registration_error_code(error_code),
                    relay_registration_error_reason(error_code, relay_message),
                );
            }
            _ => {}
        }
    }

    fn handle_registered(&self, value: &Value) {
        let connected = value
            .get("web_client_connected")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let lease_id = value
            .get("lease_id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        {
            let mut inner = self.inner.lock();
            let lease_changed = connected
                && inner
                    .endpoint
                    .as_ref()
                    .and_then(|endpoint| endpoint.last_lease_id.as_ref())
                    != lease_id.as_ref();
            let web_rejoining = value
                .get("web_rejoining")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if lease_changed {
                apply_web_lease_change(&mut inner, web_rejoining);
            }
            let Some(endpoint) = inner.endpoint.as_mut() else {
                return;
            };
            endpoint.web_client_connected = connected;
            endpoint.client_ready = false;
            endpoint.fresh_baseline_seq = None;
            if connected {
                endpoint.lease_id = lease_id.clone();
                endpoint.last_lease_id = lease_id;
            } else {
                endpoint.lease_id = None;
            }
            endpoint.status = if connected {
                WebAccessStatusKind::WebClientConnected
            } else {
                WebAccessStatusKind::WaitingWebClient
            };
            endpoint.last_error = None;
            endpoint.last_error_code = None;
        }
        self.emit_status();
    }

    fn handle_web_connected(&self, value: &Value) {
        let lease_id = value
            .get("lease_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if lease_id.is_empty() {
            return;
        }
        {
            let mut inner = self.inner.lock();
            let fresh_client = value
                .get("stream_epoch")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty);
            let fresh_baseline_seq = fresh_client.then_some(inner.stream.seq);
            let lease_changed = inner
                .endpoint
                .as_ref()
                .and_then(|endpoint| endpoint.last_lease_id.as_deref())
                != Some(lease_id.as_str());
            let web_rejoining = value
                .get("web_rejoining")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if lease_changed {
                apply_web_lease_change(&mut inner, web_rejoining);
            }
            let Some(endpoint) = inner.endpoint.as_mut() else {
                return;
            };
            endpoint.web_client_connected = true;
            endpoint.client_ready = false;
            endpoint.lease_id = Some(lease_id.clone());
            endpoint.last_lease_id = Some(lease_id.clone());
            endpoint.fresh_baseline_seq = fresh_baseline_seq;
            endpoint.status = WebAccessStatusKind::WebClientConnected;
            endpoint.last_error = None;
            endpoint.last_error_code = None;
            // A pending request may finish just after a reconnect. Route that
            // first completion to the new valid lease; a completed request is
            // still replayed from cache if the browser resends it.
            for entry in inner.rpc_cache.values_mut() {
                if let RpcCacheEntry::Pending(pending) = entry {
                    pending.lease_id = lease_id.clone();
                }
            }
        }
        self.emit_status();
    }

    fn handle_web_disconnected(&self, value: &Value) {
        let disconnected_lease = value.get("lease_id").and_then(Value::as_str);
        {
            let mut inner = self.inner.lock();
            let Some(endpoint) = inner.endpoint.as_mut() else {
                return;
            };
            if disconnected_lease.is_some() && endpoint.lease_id.as_deref() != disconnected_lease {
                return;
            }
            endpoint.web_client_connected = false;
            endpoint.client_ready = false;
            endpoint.lease_id = None;
            endpoint.fresh_baseline_seq = None;
            endpoint.status = WebAccessStatusKind::WebClientDisconnected;
        }
        self.emit_status();
    }

    fn handle_revoked(&self, endpoint_id: &str) {
        let revoked = {
            let mut inner = self.inner.lock();
            if inner
                .endpoint
                .as_ref()
                .is_none_or(|endpoint| endpoint.config.endpoint_id != endpoint_id)
            {
                return;
            }
            let revoked = inner.endpoint.take();
            inner.rpc_ledger = RpcLedger::default();
            reset_web_state(&mut inner);
            inner.stream.reset();
            inner.idle_status = WebAccessStatusKind::Revoked;
            revoked
        };
        if let Some(endpoint) = revoked {
            let _ = endpoint.sender.try_send(RelayOutbound::Shutdown);
        }
        if let Err(error) = remove_config() {
            eprintln!("[web-access] remove revoked config failed: {error}");
        }
        if let Err(error) = remove_rpc_ledger() {
            eprintln!("[web-access] remove revoked RPC ledger failed: {error}");
        }
        self.emit_status();
    }

    fn handle_replaced(&self, endpoint_id: &str) {
        {
            let mut inner = self.inner.lock();
            let Some(endpoint) = inner.endpoint.as_mut() else {
                return;
            };
            if endpoint.config.endpoint_id != endpoint_id {
                return;
            }
            endpoint.status = WebAccessStatusKind::Error;
            endpoint.web_client_connected = false;
            endpoint.client_ready = false;
            endpoint.lease_id = None;
            endpoint.fresh_baseline_seq = None;
            endpoint.last_error = Some(
                "this endpoint is active in another desktop process; rotate Web access to take a new endpoint"
                    .to_string(),
            );
            endpoint.last_error_code = None;
            reset_web_state(&mut inner);
        }
        self.emit_status();
    }

    fn handle_event_subscription(&self, value: &Value, subscribe: bool) {
        let Some(event) = value.get("event").and_then(Value::as_str) else {
            return;
        };
        if !self.policy.events.contains(event) {
            return;
        }
        // Subscribe/unsubscribe bookkeeping only; no desktop-side notification
        // is needed for unsubscribe.
        let mut inner = self.inner.lock();
        // Round-14 review minor: the lease gate takes the same lock, in the same
        // acquisition, as the subscription change (same caliber as rpc_request's
        // admission). A frame whose lease does not match the active lease is dropped; with
        // no active lease it keeps passing (the main path has no replyable target anyway,
        // and a subscription can only be consumed after the first join — consistent with
        // the existing semantics).
        let lease_id = value
            .get("lease_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !rpc_request_targets_active_lease(&inner, lease_id) {
            return;
        }
        if subscribe {
            inner.subscriptions.insert(event.to_string());
        } else {
            inner.subscriptions.remove(event);
        }
    }

    /// Terminal-settle epilogue shared by the cacheable-terminal branches of
    /// `handle_rpc_request` (deterministic rejections and acknowledged
    /// tombstone outcomes): record the completion in the RPC cache
    /// and append the request to the journal order before pruning. Must be
    /// called while `Inner` is locked (the caller's `inner.lock()` guard);
    /// the cache insert, order push, and prune are kept one unit so no branch
    /// can forget the ordering bookkeeping that keeps replays exact.
    /// Transient rejections and fingerprint conflicts intentionally skip this
    /// helper so a retry re-runs admission instead of replaying them.
    fn settle_complete(
        inner: &mut Inner,
        request_id: &str,
        fingerprint: String,
        completion: RpcCompletion,
    ) {
        inner.rpc_cache.insert(
            request_id.to_string(),
            RpcCacheEntry::Complete {
                fingerprint,
                completion,
                swept_acknowledged: false,
            },
        );
        inner.rpc_order.push_back(request_id.to_string());
        prune_rpc_cache(inner);
    }

    /// Shared admission tail of the two Pending branches in `handle_rpc_request`
    /// (reconstructed-tombstone and fresh-durable): insert the Pending cache
    /// entry, queue the request id, prune, then map the bridge generation to the
    /// dispatch action. Both arms were token-identical; this is the single home
    /// of the idempotency bookkeeping (`settle_complete` is the Complete-insert
    /// analogue). The cache entry clones `command`/`args`; the dispatch consumes
    /// the originals — exactly the pre-extraction sharing.
    fn admit_pending_rpc(
        inner: &mut Inner,
        request_id: &str,
        lease_id: String,
        command: String,
        args: Value,
        fingerprint: String,
        bridge_generation: Option<String>,
    ) -> RpcRequestAction {
        inner.rpc_cache.insert(
            request_id.to_string(),
            RpcCacheEntry::Pending(PendingRpc {
                lease_id,
                command: command.clone(),
                args: args.clone(),
                fingerprint,
                dispatched_generation: bridge_generation.clone(),
                acknowledged_generation: None,
                dispatched_at: Instant::now(),
            }),
        );
        inner.rpc_order.push_back(request_id.to_string());
        prune_rpc_cache(inner);
        if let Some(bridge_generation) = bridge_generation {
            RpcRequestAction::Dispatch(RpcDispatch {
                request_id: request_id.to_string(),
                command,
                args,
                bridge_generation,
            })
        } else {
            RpcRequestAction::None
        }
    }

    /// Settlement funnel for the tombstone branch: the completion value computation and
    /// the "does it enter the Complete cache" judgment live in this one function, consumed
    /// by handle_rpc_request's tombstone arm and by tests. Only the outcome_unknown of
    /// "tombstone's own fingerprint + acknowledged" is cacheable; fingerprint conflicts
    /// are recomputed per request and never cached — caching under the tombstone
    /// fingerprint would replay the probe's conflict against the legitimate fingerprint
    /// after a restart, and caching under the incoming fingerprint would let the Complete
    /// branch preempt the legitimate fingerprint's redispatch (see
    /// `tombstone_completion_cacheable`). Tests drive this function directly to pin the
    /// decision itself; the three wiring sites are now trivial calls into it, so any
    /// change is reviewable at a glance.
    fn settle_tombstone_outcome(
        inner: &mut Inner,
        request_id: &str,
        fingerprint: String,
        tombstone: &RpcTombstone,
    ) -> RpcCompletion {
        let completion = tombstone_completion(tombstone, &fingerprint);
        if tombstone_completion_cacheable(tombstone, &fingerprint) {
            Self::settle_complete(inner, request_id, fingerprint, completion.clone());
        }
        completion
    }

    /// Settlement funnel for the admission-refusal branch: cacheable (deterministic)
    /// refusals enter the Complete cache, while transient refusals (in-flight saturation,
    /// Session/ledger temporarily unavailable, the disk-touching kb path validation)
    /// answer only this request — caching a transient refusal would make retries with the
    /// same client_request_id replay it forever, until FIFO eviction. Same funnel
    /// rationale as [`Self::settle_tombstone_outcome`].
    fn settle_admission_rejection(
        inner: &mut Inner,
        request_id: &str,
        fingerprint: String,
        rejection: &RpcRejection,
    ) -> RpcCompletion {
        if rejection.cacheable {
            Self::settle_complete(inner, request_id, fingerprint, rejection.completion.clone());
        }
        rejection.completion.clone()
    }

    /// Settlement funnel for the begin (side-effect barrier) branch: a ledger persist
    /// failure is transient — respond but do not enter the Complete cache. The command has
    /// not executed, so a retry redispatches and re-runs begin; caching would replay this
    /// one failure forever (the same class as the admission path's old replay poisoning).
    /// The error text folds the storage-root prefix here: it reaches the phone as
    /// rpc_response.error.
    /// Round-7 review: the funnel also clears Pending's dispatch generation marker — after
    /// a failure Pending still carries the current generation, so a same-id retry would be
    /// swallowed idempotently at the Pending arm (silent until the TTL placeholder hardens
    /// into a fabricated timeout), contradicting the "retry redispatches" promise above.
    /// After clearing, the retry redisperses through the Pending arm and re-runs begin; a
    /// begin failure means the command never executed (the WebView executes only after
    /// begin returns true), so redispatch carries no double-execution risk. Same funnel
    /// rationale as [`Self::settle_tombstone_outcome`].
    fn settle_begin_persist_failure(
        inner: &mut Inner,
        request_id: &str,
        lease_id: &str,
        error: &str,
    ) -> RpcCompletion {
        let completion = rpc_error_completion(
            "rpc_ledger_unavailable",
            format!(
                "cannot durably begin Web RPC: {}",
                paths::redact_storage_prefix(error)
            ),
        );
        if let Some(RpcCacheEntry::Pending(pending)) = inner.rpc_cache.get_mut(request_id) {
            pending.dispatched_generation = None;
        }
        if let Some(endpoint) = inner.endpoint.as_ref() {
            let response = rpc_response(
                &endpoint.config.endpoint_id,
                lease_id,
                request_id,
                &completion,
            );
            let _ = endpoint.sender.try_send(RelayOutbound::Message(response));
        }
        completion
    }

    /// Whether this is a timeout placeholder written by the TTL sweep (the only
    /// `rpc_timed_out` producer is handle_rpc_request's sweep). `complete_rpc` uses this
    /// to let a real completion replace the placeholder; real terminal codes like
    /// deterministic refusals all differ and are never mistakenly replaced.
    fn rpc_completion_is_in_flight_timeout(completion: &RpcCompletion) -> bool {
        !completion.ok && completion.error_code.as_deref() == Some(RPC_TIMED_OUT_CODE)
    }

    fn handle_rpc_request(&self, value: &Value) {
        let request_id = value
            .get("client_request_id")
            .or_else(|| value.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let command = value
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let lease_id = value
            .get("lease_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if request_id.is_empty() || request_id.len() > 256 || lease_id.is_empty() {
            return;
        }
        let args = value.get("args").cloned().unwrap_or_else(|| json!({}));
        let command_error = validate_rpc_command(&command).err();
        let fingerprint_command = if command_error.is_some() {
            "__invalid_web_rpc_command__"
        } else {
            command.as_str()
        };
        let fingerprint = match rpc_fingerprint(fingerprint_command, &args) {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                self.set_endpoint_error(error);
                return;
            }
        };

        let allowed = command_error.is_none() && self.policy.commands.contains(&command);
        let preflight_rejection = if let Some(error) = command_error {
            Some(RpcRejection::deterministic(rpc_error_completion(
                "invalid_command",
                error,
            )))
        } else if serde_json::to_vec(&args)
            .map(|encoded| encoded.len() > MAX_RPC_REQUEST_BYTES)
            .unwrap_or(true)
        {
            Some(RpcRejection::deterministic(rpc_error_completion(
                "request_too_large",
                format!(
                    "远程控制请求参数超过 {} MiB 上限",
                    MAX_RPC_REQUEST_BYTES / (1024 * 1024)
                ),
            )))
        } else if allowed {
            validate_web_rpc_scope(&self.app, &command, &args)
                .err()
                .map(WebScopeRejection::into_rpc_rejection)
        } else {
            None
        };
        let action = {
            let mut inner = self.inner.lock();
            let Some(endpoint) = inner.endpoint.as_ref() else {
                return;
            };
            let endpoint_id = endpoint.config.endpoint_id.clone();
            let sender = endpoint.sender.clone();
            let bridge_generation = inner.bridge_generation.clone();
            // An in-flight request past its TTL turns into a timeout placeholder, freeing
            // the in-flight slot and blocking same-id replay from triggering a redispatch
            // (idempotent dedup still holds). The placeholder is not terminal truth: when a
            // real completion arrives after the sweep, complete_rpc replaces the placeholder
            // (Round-5 review M1), so the "idempotent ignore" for late begin/complete holds
            // only for normal completion values after a replacement.
            let now = Instant::now();
            let expired: Vec<String> = inner
                .rpc_cache
                .iter()
                .filter_map(|(id, entry)| match entry {
                    RpcCacheEntry::Pending(pending)
                        if rpc_in_flight_expired(pending.dispatched_at, now) =>
                    {
                        Some(id.clone())
                    }
                    _ => None,
                })
                .collect();
            for id in expired {
                if let Some(RpcCacheEntry::Pending(pending)) = inner.rpc_cache.get(&id) {
                    let fingerprint = pending.fingerprint.clone();
                    // Round-14 review minor: requests that were acknowledged (already started
                    // executing) get marked; the generation-change arm rewrites those into
                    // outcome_unknown. Unacknowledged placeholders are not marked (a timeout is
                    // an accurate answer for a never-executed request).
                    let swept_acknowledged = pending.acknowledged_generation.is_some();
                    inner.rpc_cache.insert(
                        id,
                        RpcCacheEntry::Complete {
                            fingerprint,
                            completion: rpc_error_completion(
                                RPC_TIMED_OUT_CODE,
                                "远程控制请求执行超时，已释放该请求",
                            ),
                            swept_acknowledged,
                        },
                    );
                }
            }
            let in_flight_count = inner
                .rpc_cache
                .values()
                .filter(|entry| matches!(entry, RpcCacheEntry::Pending(_)))
                .count();
            // The same lease validation as client_ready: an rpc_request carrying a lease
            // that has already been superseded is a late frame in a takeover race — ignore it
            // directly, without entering the idempotency cache.
            // The validation must sit under the same lock as admission: validating the lease
            // separately and then admitting leaves a takeover window between the two locks
            // where a late frame could still enter the cache and be dispatched. On takeover,
            // pending RPCs are re-stamped with the new lease before being re-delivered, so
            // the redelivery path is never hurt; with no active lease the original behavior
            // is kept (the main admission path has no replyable endpoint anyway).
            if !rpc_request_targets_active_lease(&inner, &lease_id) {
                return;
            }
            match inner.rpc_cache.get_mut(&request_id) {
                Some(RpcCacheEntry::Pending(pending)) => {
                    if pending.fingerprint != fingerprint {
                        RpcRequestAction::Respond(
                            sender,
                            rpc_response(
                                &endpoint_id,
                                &lease_id,
                                &request_id,
                                &request_conflict_completion(),
                            ),
                        )
                    } else {
                        pending.lease_id = lease_id;
                        if pending.acknowledged_generation.is_none()
                            && bridge_generation.is_some()
                            && pending.dispatched_generation != bridge_generation
                        {
                            pending.dispatched_generation = bridge_generation.clone();
                            // Redispatch must reset the TTL clock: a Pending entry may be the
                            // retry arm after a begin persist failure, and inheriting the original
                            // dispatched_at would let retries near the window's end be fabricated
                            // into rpc_timed_out by the TTL sweep almost immediately.
                            pending.dispatched_at = Instant::now();
                            RpcRequestAction::Dispatch(RpcDispatch {
                                request_id: request_id.clone(),
                                command: pending.command.clone(),
                                args: pending.args.clone(),
                                bridge_generation: bridge_generation.unwrap_or_default(),
                            })
                        } else {
                            RpcRequestAction::None
                        }
                    }
                }
                Some(RpcCacheEntry::Complete {
                    fingerprint: existing_fingerprint,
                    completion,
                    ..
                }) => {
                    let completion = if existing_fingerprint == &fingerprint {
                        completion.clone()
                    } else {
                        request_conflict_completion()
                    };
                    RpcRequestAction::Respond(
                        sender,
                        rpc_response(&endpoint_id, &lease_id, &request_id, &completion),
                    )
                }
                None => {
                    if let Some(tombstone) = inner.rpc_ledger.get(&request_id).cloned() {
                        if tombstone.fingerprint != fingerprint || tombstone.acknowledged {
                            // The caching decision is funneled into settle_tombstone_outcome
                            // (only own-fingerprint + acknowledged is cacheable; conflicts recompute
                            // per request).
                            let completion = Self::settle_tombstone_outcome(
                                &mut inner,
                                &request_id,
                                fingerprint.clone(),
                                &tombstone,
                            );
                            RpcRequestAction::Respond(
                                sender,
                                rpc_response(&endpoint_id, &lease_id, &request_id, &completion),
                            )
                        } else {
                            // `remember` was durable but the desktop never
                            // acknowledged execution. Reconstructing the
                            // pending request is safe after a process/WebView
                            // restart; only acknowledged tombstones become
                            // outcome_unknown.
                            let rejection = rpc_admission_rejection(
                                preflight_rejection.as_ref(),
                                allowed,
                                in_flight_count,
                            );
                            if let Some(rejection) = rejection {
                                // A transient refusal (in-flight saturation, Session temporarily
                                // unavailable) answers only this request; caching would make retries
                                // with the same client_request_id replay the refusal forever, until
                                // FIFO eviction. The settlement funnels into
                                // settle_admission_rejection, which tests pin directly.
                                let completion = Self::settle_admission_rejection(
                                    &mut inner,
                                    &request_id,
                                    fingerprint.clone(),
                                    &rejection,
                                );
                                RpcRequestAction::Respond(
                                    sender,
                                    rpc_response(&endpoint_id, &lease_id, &request_id, &completion),
                                )
                            } else {
                                Self::admit_pending_rpc(
                                    &mut inner,
                                    &request_id,
                                    lease_id,
                                    command,
                                    args,
                                    fingerprint,
                                    bridge_generation,
                                )
                            }
                        }
                    } else {
                        match prepare_new_rpc_admission(
                            &inner.rpc_ledger,
                            &request_id,
                            &fingerprint,
                            preflight_rejection.as_ref(),
                            allowed,
                            in_flight_count,
                        ) {
                            Ok(NewRpcAdmission::Rejected(rejection)) => {
                                let completion = Self::settle_admission_rejection(
                                    &mut inner,
                                    &request_id,
                                    fingerprint.clone(),
                                    &rejection,
                                );
                                RpcRequestAction::Respond(
                                    sender,
                                    rpc_response(&endpoint_id, &lease_id, &request_id, &completion),
                                )
                            }
                            Ok(NewRpcAdmission::Durable(next_ledger)) => {
                                if let Err(error) = persist_rpc_ledger(&next_ledger) {
                                    // A persist failure is transient: no response cache write — a
                                    // retry with the same client_request_id walks the full admission
                                    // again, instead of replaying this one failure forever.
                                    let completion = rpc_error_completion(
                                        "rpc_ledger_unavailable",
                                        format!(
                                            "cannot durably accept Web RPC: {}",
                                            paths::redact_storage_prefix(&error)
                                        ),
                                    );
                                    RpcRequestAction::Respond(
                                        sender,
                                        rpc_response(
                                            &endpoint_id,
                                            &lease_id,
                                            &request_id,
                                            &completion,
                                        ),
                                    )
                                } else {
                                    inner.rpc_ledger = next_ledger;
                                    Self::admit_pending_rpc(
                                        &mut inner,
                                        &request_id,
                                        lease_id,
                                        command,
                                        args,
                                        fingerprint,
                                        bridge_generation,
                                    )
                                }
                            }
                            Err(error) => {
                                // Same as above: ledger unavailability is a transient state, so the refusal is not cached.
                                let completion = rpc_error_completion(
                                    "rpc_ledger_unavailable",
                                    format!(
                                        "cannot durably accept Web RPC: {}",
                                        paths::redact_storage_prefix(&error)
                                    ),
                                );
                                RpcRequestAction::Respond(
                                    sender,
                                    rpc_response(&endpoint_id, &lease_id, &request_id, &completion),
                                )
                            }
                        }
                    }
                }
            }
        };

        match action {
            RpcRequestAction::None => {}
            RpcRequestAction::Respond(sender, response) => {
                let _ = sender.try_send(RelayOutbound::Message(response));
            }
            RpcRequestAction::Dispatch(dispatch) => {
                // Round-12 review P3: an emit failure was previously swallowed silently —
                // the recovery logic inside emit_rpc_dispatch (clearing dispatched_generation,
                // allowing re-emit) still ran, but the phone received no receipt and the request
                // had no endpoint until the TTL sweep fabricated rpc_timed_out. Logging makes
                // such failures diagnosable.
                if let Err(error) = self.emit_rpc_dispatch(dispatch) {
                    eprintln!("[web-access] rpc dispatch emit to main webview failed: {error}");
                }
            }
        }
    }

    fn emit_rpc_dispatch(&self, dispatch: RpcDispatch) -> Result<(), String> {
        let request_id = dispatch.request_id;
        let generation = dispatch.bridge_generation;
        let emitted = self.app.emit_to(
            "main",
            "web_access:rpc_request",
            json!({
                "request_id": &request_id,
                "id": &request_id,
                "command": dispatch.command,
                "args": dispatch.args,
                "bridge_generation": &generation,
            }),
        );
        if let Err(error) = emitted {
            let mut inner = self.inner.lock();
            if let Some(RpcCacheEntry::Pending(pending)) = inner.rpc_cache.get_mut(&request_id) {
                if pending.acknowledged_generation.is_none()
                    && pending.dispatched_generation.as_deref() == Some(generation.as_str())
                {
                    pending.dispatched_generation = None;
                }
            }
            return Err(format!("dispatch Web RPC to desktop UI: {error}"));
        }
        Ok(())
    }

    fn synchronize_client_from_value(&self, value: &Value) {
        let lease_id = value
            .get("lease_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if lease_id.is_empty() {
            return;
        }
        let requested_epoch = value
            .get("stream_epoch")
            .and_then(Value::as_str)
            .filter(|epoch| !epoch.is_empty());
        let after_seq = value.get("after_seq").and_then(Value::as_u64).unwrap_or(0);
        let state_ready = client_state_ready(value);

        let mut capability_commands = self.policy.commands.iter().cloned().collect::<Vec<_>>();
        capability_commands.sort();
        let mut capability_events = self.policy.events.iter().cloned().collect::<Vec<_>>();
        capability_events.sort();
        {
            let mut inner = self.inner.lock();
            let Some(endpoint) = inner.endpoint.as_ref() else {
                return;
            };
            if endpoint.lease_id.as_deref() != Some(lease_id) {
                return;
            }
            let endpoint_id = endpoint.config.endpoint_id.clone();
            let sender = endpoint.sender.clone();
            let fresh_baseline_seq = endpoint.fresh_baseline_seq;
            let epoch_matches = requested_epoch
                .map(|epoch| epoch == inner.stream.epoch)
                .unwrap_or(false);
            if !state_ready {
                if requested_epoch.is_some() && (!epoch_matches || after_seq > inner.stream.seq) {
                    inner.stream.reset();
                    let stream_epoch = inner.stream.epoch.clone();
                    if let Some(endpoint) = inner.endpoint.as_mut() {
                        endpoint.client_ready = false;
                        endpoint.fresh_baseline_seq = None;
                    }
                    enqueue_stream_reset(
                        sender,
                        stream_reset_message(
                            &endpoint_id,
                            lease_id,
                            &stream_epoch,
                            "epoch_changed",
                        ),
                    );
                    return;
                }
                let baseline = if requested_epoch.is_none() {
                    fresh_baseline_seq.unwrap_or(inner.stream.seq)
                } else {
                    after_seq
                };
                let snapshot = snapshot_message(
                    &endpoint_id,
                    lease_id,
                    &inner.stream.epoch,
                    baseline,
                    &capability_commands,
                    &capability_events,
                );
                if sender.try_send(RelayOutbound::Message(snapshot)).is_err() {
                    inner.stream.reset();
                    let stream_epoch = inner.stream.epoch.clone();
                    if let Some(endpoint) = inner.endpoint.as_mut() {
                        endpoint.client_ready = false;
                        endpoint.fresh_baseline_seq = None;
                    }
                    enqueue_stream_reset(
                        sender,
                        stream_reset_message(
                            &endpoint_id,
                            lease_id,
                            &stream_epoch,
                            "snapshot_enqueue_failed",
                        ),
                    );
                }
                return;
            }
            // A client without an epoch is a fresh browser/device, not a
            // reconnect from sequence zero. Give it a baseline snapshot at
            // the current head so historical interactive events are never
            // replayed as if they had just happened.
            let fresh_client = requested_epoch.is_none();
            let replay = if fresh_client {
                inner
                    .stream
                    .replay_after(fresh_baseline_seq.unwrap_or(inner.stream.seq))
            } else if epoch_matches {
                inner.stream.replay_after(after_seq)
            } else {
                None
            };

            let mut messages = match replay {
                Some(events) if fresh_client => {
                    let baseline = fresh_baseline_seq.unwrap_or(inner.stream.seq);
                    let mut messages = vec![snapshot_message(
                        &endpoint_id,
                        lease_id,
                        &inner.stream.epoch,
                        baseline,
                        &capability_commands,
                        &capability_events,
                    )];
                    messages.extend(subscription_filtered_replay_messages(
                        events,
                        &inner.subscriptions,
                        ReplayMessageContext {
                            endpoint_id: &endpoint_id,
                            lease_id,
                            stream_epoch: &inner.stream.epoch,
                            capability_commands: &capability_commands,
                            capability_events: &capability_events,
                        },
                    ));
                    messages
                }
                Some(events) => subscription_filtered_replay_messages(
                    events,
                    &inner.subscriptions,
                    ReplayMessageContext {
                        endpoint_id: &endpoint_id,
                        lease_id,
                        stream_epoch: &inner.stream.epoch,
                        capability_commands: &capability_commands,
                        capability_events: &capability_events,
                    },
                ),
                None => {
                    // The browser reload behavior deliberately persists the new
                    // epoch with cursor zero. Resetting the journal here makes
                    // that next join satisfiable instead of causing a loop.
                    let reason = if fresh_client || epoch_matches {
                        "journal_gap"
                    } else {
                        "epoch_changed"
                    };
                    inner.stream.reset();
                    let stream_epoch = inner.stream.epoch.clone();
                    if let Some(endpoint) = inner.endpoint.as_mut() {
                        endpoint.client_ready = false;
                        endpoint.fresh_baseline_seq = None;
                    }
                    enqueue_stream_reset(
                        sender,
                        stream_reset_message(&endpoint_id, lease_id, &stream_epoch, reason),
                    );
                    return;
                }
            };
            if !messages.iter().any(|message| {
                message.get("type").and_then(Value::as_str) == Some("desktop_snapshot")
            }) {
                messages.push(snapshot_message(
                    &endpoint_id,
                    lease_id,
                    &inner.stream.epoch,
                    inner.stream.seq,
                    &capability_commands,
                    &capability_events,
                ));
            }
            // Enqueue the complete replay while holding the stream lock. Live
            // events also enqueue under this lock, so seq N+1 cannot overtake
            // the replay suffix ending at N.
            let replay_enqueued = try_enqueue_message_batch(messages, |message| {
                sender.try_send(RelayOutbound::Message(message)).is_ok()
            });
            if !replay_enqueued {
                // A partially enqueued suffix is always followed by a reset;
                // never advertise readiness after silently losing its tail.
                inner.stream.reset();
                let stream_epoch = inner.stream.epoch.clone();
                if let Some(endpoint) = inner.endpoint.as_mut() {
                    endpoint.client_ready = false;
                    endpoint.fresh_baseline_seq = None;
                }
                enqueue_stream_reset(
                    sender,
                    stream_reset_message(
                        &endpoint_id,
                        lease_id,
                        &stream_epoch,
                        "replay_enqueue_failed",
                    ),
                );
                return;
            }
            if let Some(endpoint) = inner.endpoint.as_mut() {
                if endpoint.lease_id.as_deref() == Some(lease_id) {
                    endpoint.client_ready = true;
                    endpoint.fresh_baseline_seq = None;
                }
            }
        }
    }

    fn publish_event_inner(
        &self,
        event: &str,
        payload: Value,
        source: EventSource,
    ) -> Result<(), String> {
        if !self.policy.events.contains(event) {
            return Err(format!("远程控制不允许发送该事件：{event}"));
        }
        // The engine/file watcher already call `forward_app_event` directly.
        // The desktop WebView proxy also sees the corresponding Tauri event;
        // ignore that second source to keep sequence semantics exact.
        if source == EventSource::Frontend && RUST_FORWARDED_EVENTS.contains(&event) {
            return Ok(());
        }
        // Until the remote side officially supports code-session listing/authorization/UI,
        // native code-session events are filtered first: when the session id carried by the
        // event payload (`session_id` for chat:* / artifact:disk, `id` for session:*) points
        // at a Pinvou-native code session (native only, not ACP sessions), the event is not
        // forwarded (scheduled_task:run_updated's payload is currently empty and never hits
        // this filter). The remote WebUI never receives a code session's message stream that
        // it cannot display or authorize; the predicate returns true only for real code
        // session ids, so ordinary sessions are unaffected.
        if should_filter_code_session_event(
            self.inner.lock().code_session_predicate.as_ref(),
            &payload,
        ) {
            return Ok(());
        }
        {
            let mut inner = self.inner.lock();
            let endpoint = inner
                .endpoint
                .as_ref()
                .ok_or_else(|| "Web access endpoint is not active".to_string())?;
            let endpoint_id = endpoint.config.endpoint_id.clone();
            let lease_id = endpoint.lease_id.clone();
            let client_ready = endpoint.client_ready;
            let sender = endpoint.sender.clone();
            let subscribed = inner.subscriptions.contains(event);
            // Journal every allowlisted event independently of the current
            // lease's subscribe handshake. A reconnect can otherwise lose
            // deltas emitted between `web_client_connected` and the browser's
            // subscribe/client_ready messages without leaving a sequence gap.
            let sizing_lease_id = lease_id.as_deref().unwrap_or(LEASE_ID_WIRE_PLACEHOLDER);
            let message =
                match inner
                    .stream
                    .record(&endpoint_id, sizing_lease_id, event.to_string(), payload)
                {
                    Ok(message) => message,
                    Err(error @ StreamRecordError::Oversized { .. }) => {
                        // `record` already rotated the epoch and cleared the
                        // journal atomically. Stop live delivery until the WebUI
                        // reloads and negotiates that new epoch, so no later event
                        // can overtake this recovery barrier.
                        let stream_epoch = inner.stream.epoch.clone();
                        if let Some(endpoint) = inner.endpoint.as_mut() {
                            endpoint.client_ready = false;
                            endpoint.fresh_baseline_seq = None;
                        }
                        if let Some(lease_id) = lease_id.as_deref() {
                            enqueue_stream_reset(
                                sender,
                                stream_reset_message(
                                    &endpoint_id,
                                    lease_id,
                                    &stream_epoch,
                                    "event_too_large",
                                ),
                            );
                        }
                        return Err(error.to_string());
                    }
                    Err(error) => return Err(error.to_string()),
                };
            let Some(lease_id) = lease_id.as_deref() else {
                // Keep the event in the journal for a reconnecting subscribed
                // browser, but there is no valid destination at this instant.
                return Ok(());
            };
            if !client_ready {
                // Subscriptions arrive before client_ready. Keep journaling,
                // but do not let a live high sequence overtake its replay.
                return Ok(());
            }
            if !subscribed {
                // New events an older WebUI did not negotiate must not be pushed directly,
                // or it would judge the desktop incompatible and disconnect; the event is
                // already in the journal, so later subscribing clients can still recover it
                // via replay.
                return Ok(());
            }
            debug_assert_eq!(
                message.get("lease_id").and_then(Value::as_str),
                Some(lease_id)
            );
            match sender.try_send(RelayOutbound::Message(message)) {
                Ok(()) => Ok(()),
                Err(send_error) => {
                    // The just-recorded event is the journal tail. Rebase it
                    // into a fresh epoch before exposing the reset barrier, so
                    // a full relay queue cannot strand a final `done` or
                    // `user_input_required` with no later gap to trigger a
                    // reconnect.
                    let failed_event = inner
                        .stream
                        .journal
                        .back()
                        .cloned()
                        .ok_or_else(|| "远程控制事件流末尾数据丢失".to_string())?;
                    inner.stream.reset();
                    let rebase_result = inner.stream.record(
                        &endpoint_id,
                        lease_id,
                        failed_event.event,
                        failed_event.payload,
                    );
                    let stream_epoch = inner.stream.epoch.clone();
                    if let Some(endpoint) = inner.endpoint.as_mut() {
                        endpoint.client_ready = false;
                        endpoint.fresh_baseline_seq = None;
                    }
                    enqueue_stream_reset(
                        sender,
                        stream_reset_message(
                            &endpoint_id,
                            lease_id,
                            &stream_epoch,
                            "live_enqueue_failed",
                        ),
                    );
                    rebase_result.map_err(|error| {
                        format!("远程控制事件入队失败后重建事件流失败：{error}")
                    })?;
                    Err(format!(
                        "远程控制实时事件入队失败，已安排事件流恢复：{send_error}"
                    ))
                }
            }
        }
    }

    fn is_current_endpoint(&self, endpoint_id: &str) -> bool {
        !endpoint_id.is_empty()
            && self
                .inner
                .lock()
                .endpoint
                .as_ref()
                .is_some_and(|endpoint| endpoint.config.endpoint_id == endpoint_id)
    }

    fn set_endpoint_error(&self, error: String) {
        self.set_endpoint_error_with_code(None, error);
    }

    /// Writes error text + an optional stable recovery code as a pair; an error carrying
    /// no code must clear the old one, so old recovery copy is never mapped onto a new
    /// error. The text passes through [`bound_relay_text`] first (Round-11 review NIT: a
    /// relay error frame's message can reach the single-frame cap of ~4 MiB; storing it
    /// verbatim would re-serialize the whole thing on every `emit_status`).
    fn set_endpoint_error_with_code(&self, code: Option<&'static str>, error: String) {
        let error = bound_relay_text(error);
        {
            let mut inner = self.inner.lock();
            if let Some(endpoint) = inner.endpoint.as_mut() {
                endpoint.status = WebAccessStatusKind::Error;
                endpoint.last_error = Some(error);
                endpoint.last_error_code = code.map(str::to_string);
            }
        }
        self.emit_status();
    }

    fn emit_status(&self) {
        let status = self.status();
        // The status payload carries a pairing URL containing an access_token fragment
        // plus a QR code — sensitive content sent only to the main window (consistent with
        // the other web_access:* events), never broadcast to auxiliary WebViews such as
        // reader/pet.
        let _ = self.app.emit_to("main", "web_access:status", status);
    }
}

#[cfg(test)]
mod tests {
    use super::persistence::{
        acknowledge_pending_revocation_at, load_config_from, load_or_initialize_rpc_ledger_at,
        load_pending_revocations_from, queue_pending_revocation_at,
    };
    use super::rpc::{WebSessionScope, web_session_scope};
    use super::*;

    const TEST_ENDPOINT_ID: &str = "ep_test";
    const TEST_LEASE_ID: &str = "lease_000000000000000000000000";

    #[test]
    fn replay_skips_unsubscribed_events_without_creating_sequence_gaps() {
        let mut stream = StreamState::new();
        record_stream_event(&mut stream, "session:deleted", json!({ "id": "one" }));
        record_stream_event(&mut stream, "session:list_changed", json!({}));
        record_stream_event(&mut stream, "chat:done", json!({ "id": "one" }));
        let events = stream.replay_after(0).expect("replay");
        let subscriptions = HashSet::from(["session:deleted".to_string(), "chat:done".to_string()]);
        let commands = Vec::new();
        let capabilities = Vec::new();

        let messages = subscription_filtered_replay_messages(
            events,
            &subscriptions,
            ReplayMessageContext {
                endpoint_id: TEST_ENDPOINT_ID,
                lease_id: TEST_LEASE_ID,
                stream_epoch: &stream.epoch,
                capability_commands: &commands,
                capability_events: &capabilities,
            },
        );

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["type"], "event");
        assert_eq!(messages[0]["seq"], 1);
        assert_eq!(messages[0]["event"], "session:deleted");
        assert_eq!(messages[1]["type"], "desktop_snapshot");
        assert_eq!(messages[1]["seq"], 2);
        assert_eq!(messages[2]["type"], "event");
        assert_eq!(messages[2]["seq"], 3);
        assert_eq!(messages[2]["event"], "chat:done");
    }

    #[test]
    fn stalled_in_flight_rpc_expires_after_ttl_only() {
        let dispatched_at = Instant::now();
        assert!(!rpc_in_flight_expired(
            dispatched_at,
            dispatched_at + Duration::from_secs(180)
        ));
        assert!(rpc_in_flight_expired(
            dispatched_at,
            dispatched_at + RPC_IN_FLIGHT_TTL + Duration::from_secs(1)
        ));
    }

    #[test]
    fn compiled_defaults_target_the_local_dev_relay() {
        assert_eq!(
            DEFAULT_PUBLIC_BASE_URL,
            "http://127.0.0.1:8787/pinvou3/remote"
        );
        assert_eq!(
            DEFAULT_RELAY_WS_URL,
            "ws://127.0.0.1:8787/pinvou3/remote/ws"
        );
    }

    #[test]
    fn web_attachment_upload_accumulates_chunks_until_commit() {
        let mut inner = Inner::default();

        let first = append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_1",
            "报告.pdf",
            0,
            6,
            b"abc",
            false,
            None,
        )
        .expect("first chunk");
        assert!(first.is_none());

        let completed = append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_1",
            "报告.pdf",
            3,
            6,
            b"def",
            true,
            None,
        )
        .expect("final chunk")
        .expect("commit returns the payload");
        assert_eq!(completed.0, "报告.pdf");
        assert_eq!(completed.1, b"abcdef");
        assert!(inner.web_attachment_uploads.is_empty());
        assert!(inner.web_attachment_upload_order.is_empty());
    }

    #[test]
    fn web_attachment_upload_verifies_sha256_on_commit() {
        use sha2::{Digest, Sha256};

        let mut inner = Inner::default();
        let mut hasher = Sha256::new();
        hasher.update(b"abcdef");
        let good = crate::platform::encoding::hex_lower(&hasher.finalize());

        append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_hash",
            "notes.txt",
            0,
            6,
            b"abc",
            false,
            None,
        )
        .expect("first chunk");

        let completed = append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_hash",
            "notes.txt",
            3,
            6,
            b"def",
            true,
            Some(&good),
        )
        .expect("digest match must commit");
        assert_eq!(completed.expect("payload").1, b"abcdef");

        append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_hash_bad",
            "notes.txt",
            0,
            6,
            b"abc",
            false,
            None,
        )
        .expect("first chunk");
        let error = append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_hash_bad",
            "notes.txt",
            3,
            6,
            b"def",
            true,
            Some(&"b".repeat(64)),
        )
        .expect_err("digest mismatch must abort the commit");
        assert_eq!(
            error,
            transfer::WEB_ATTACHMENT_INTEGRITY_MISMATCH,
            "the mismatch failure must surface as a stable wire code for localized client copy"
        );

        // Malformed digests are rejected before hashing and keep the upload
        // resumable (the entry is not consumed).
        append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_hash_invalid",
            "notes.txt",
            0,
            6,
            b"abc",
            false,
            None,
        )
        .expect("first chunk");
        let error = append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_hash_invalid",
            "notes.txt",
            3,
            6,
            b"def",
            true,
            Some("not-hex"),
        )
        .expect_err("malformed digest must be rejected");
        assert_eq!(
            error,
            transfer::WEB_ATTACHMENT_DIGEST_INVALID,
            "the malformed-digest failure must surface as a stable wire code for localized client copy"
        );
    }

    #[test]
    fn web_attachment_upload_rejects_totals_over_the_ingest_limit() {
        let mut inner = Inner::default();
        let oversized = crate::features::files::file_ingest::MAX_FILE_BYTES as usize + 1;

        let error = append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_1",
            "big.bin",
            0,
            oversized,
            b"x",
            false,
            None,
        )
        .expect_err("uploads above MAX_FILE_BYTES must fail before transfer");
        assert_eq!(
            error,
            crate::features::files::file_ingest::ATTACHMENT_FILE_TOO_LARGE,
            "the precheck failure must surface as a stable wire code for localized client copy"
        );
        assert!(inner.web_attachment_uploads.is_empty());
    }

    #[test]
    fn web_attachment_upload_requires_contiguous_offsets() {
        let mut inner = Inner::default();
        append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_1",
            "notes.txt",
            0,
            8,
            b"abcd",
            false,
            None,
        )
        .expect("first chunk");

        let error = append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_1",
            "notes.txt",
            2,
            8,
            b"cdef",
            false,
            None,
        )
        .expect_err("out-of-order chunks must be rejected");
        assert!(error.contains("偏移量"), "unexpected error: {error}");
    }

    #[test]
    fn web_attachment_upload_abort_discards_the_buffer() {
        let mut inner = Inner::default();
        append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_1",
            "notes.txt",
            0,
            8,
            b"abcd",
            false,
            None,
        )
        .expect("first chunk");

        discard_web_attachment_upload(&mut inner, "upload_device_1");
        assert!(inner.web_attachment_uploads.is_empty());
        assert!(inner.web_attachment_upload_order.is_empty());

        let error = append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_1",
            "notes.txt",
            4,
            8,
            b"efgh",
            true,
            None,
        )
        .expect_err("continuing an aborted upload must fail");
        assert!(
            error.contains("不存在或已过期"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn web_attachment_upload_capacity_evicts_the_oldest_buffer() {
        let mut inner = Inner::default();
        for index in 0..=MAX_WEB_ATTACHMENT_UPLOADS {
            append_web_attachment_upload_chunk(
                &mut inner,
                &format!("upload_device_{index}"),
                "notes.txt",
                0,
                8,
                b"abcd",
                false,
                None,
            )
            .expect("start upload");
        }

        assert_eq!(
            inner.web_attachment_uploads.len(),
            MAX_WEB_ATTACHMENT_UPLOADS
        );
        let error = append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_0",
            "notes.txt",
            4,
            8,
            b"efgh",
            true,
            None,
        )
        .expect_err("the evicted oldest upload must no longer continue");
        assert!(
            error.contains("不存在或已过期"),
            "unexpected error: {error}"
        );
    }

    fn cached_upload_for_test(
        staging: &std::path::Path,
        reservation_id: Option<&str>,
    ) -> CachedWebAttachment {
        CachedWebAttachment {
            result: crate::features::files::file_ingest::IngestResult {
                kind: "text".into(),
                basename: "notes.txt".into(),
                path: staging.join("notes.txt").to_string_lossy().into_owned(),
                markdown: Some("abc".into()),
                token_estimate: 2,
                byte_size: 3,
                warning: None,
            },
            bytes: 64,
            reservation_id: reservation_id.map(str::to_string),
            discard_requested: false,
            upload_dir: Some(staging.to_path_buf()),
        }
    }

    #[test]
    fn clearing_web_attachments_removes_upload_staging_dirs() {
        let staging = std::env::temp_dir().join(format!(
            "pinvou-webup-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(8)
        ));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("notes.txt"), b"abc").unwrap();

        let mut inner = Inner::default();
        inner.web_attachment_order.push_back("attachment_a".into());
        inner.web_attachments.insert(
            "attachment_a".into(),
            cached_upload_for_test(&staging, None),
        );
        inner.web_attachment_bytes = 64;

        clear_web_attachments(&mut inner);
        assert!(inner.web_attachments.is_empty());
        assert!(
            !staging.exists(),
            "clearing attachments must delete the upload staging dir"
        );
    }

    #[test]
    fn discarding_unreserved_web_attachment_removes_cache_and_staging_dir() {
        let staging = std::env::temp_dir().join(format!(
            "pinvou-webup-discard-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(8)
        ));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("notes.txt"), b"abc").unwrap();

        let mut inner = Inner::default();
        inner.web_attachment_order.push_back("attachment_a".into());
        inner.web_attachments.insert(
            "attachment_a".into(),
            cached_upload_for_test(&staging, None),
        );
        inner.web_attachment_bytes = 64;

        request_web_attachment_discard(&mut inner, "attachment_a");

        assert!(inner.web_attachments.is_empty());
        assert!(inner.web_attachment_order.is_empty());
        assert_eq!(inner.web_attachment_bytes, 0);
        assert!(
            !staging.exists(),
            "discarding a ready attachment must delete its staging dir"
        );
    }

    #[test]
    fn discarding_reserved_web_attachment_waits_for_turn_cleanup() {
        let staging = std::env::temp_dir().join(format!(
            "pinvou-webup-deferred-discard-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(8)
        ));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("notes.txt"), b"abc").unwrap();

        let mut inner = Inner::default();
        inner.web_attachment_order.push_back("attachment_a".into());
        inner.web_attachments.insert(
            "attachment_a".into(),
            cached_upload_for_test(&staging, Some("reservation_a")),
        );
        inner.web_attachment_bytes = 64;

        request_web_attachment_discard(&mut inner, "attachment_a");
        assert!(
            staging.exists(),
            "an active turn still needs the source file"
        );
        assert!(
            inner
                .web_attachments
                .get("attachment_a")
                .is_some_and(|cached| cached.discard_requested)
        );

        finish_web_attachment_reservation_inner(
            &mut inner,
            "reservation_a",
            &["attachment_a".into()],
            false,
        )
        .expect("a rejected turn still honors the deferred discard");

        assert!(inner.web_attachments.is_empty());
        assert!(inner.web_attachment_order.is_empty());
        assert_eq!(inner.web_attachment_bytes, 0);
        assert!(
            !staging.exists(),
            "turn cleanup must delete a browser-discarded attachment"
        );
    }

    fn record_stream_event(stream: &mut StreamState, event: &str, payload: Value) {
        stream
            .record(TEST_ENDPOINT_ID, TEST_LEASE_ID, event.to_string(), payload)
            .expect("record stream event");
    }

    #[test]
    fn embedded_policy_is_v2_and_blocks_native_authority() {
        let policy = AccessPolicy::load().expect("embedded policy");
        for command in [
            "chat",
            "ingest_file",
            "delete_model",
            "save_model",
            "set_active_model",
            "test_model_connection",
            "test_image_input_capability",
            "set_disabled_connectors",
            "install_marketplace_skill",
            "install_marketplace_tool",
            "uninstall_marketplace_skill",
            "uninstall_marketplace_tool",
            "install_voice_asr",
            "artifact_info",
            "read_artifact_text",
            "write_artifact_text",
            "read_artifact_image_b64",
            "read_artifact_thumbnail",
            "render_artifact_visual",
            "update_settings",
            "create_session",
            "load_session",
            "web_access_bridge_ready",
            "web_access_rpc_begin",
            "web_access_rpc_respond",
            "codex_acp_prompt",
            "get_codex_acp_timeline",
            "get_codex_acp_session_info",
            "get_codex_acp_pending_permissions",
            "get_codex_acp_pending_elicitations",
            "list_acp_agents",
            "get_acp_agent_status",
            "set_codex_acp_model",
            "set_codex_acp_mode",
            "set_codex_acp_config_option",
            "install_acp_agent",
            "login_acp_agent",
            "switch_acp_agent_account",
            "open_acp_agent_login_url",
            "submit_acp_agent_login_code",
        ] {
            assert!(
                !policy.commands.contains(command),
                "{command} must stay native-only"
            );
        }
        for command in [
            "web_access_artifact_info",
            "web_access_read_artifact_text",
            "web_access_write_artifact_text",
            "web_access_read_artifact_image_b64",
            "web_access_read_artifact_thumbnail",
            "web_access_render_artifact_visual",
            "web_access_update_settings",
            "web_access_create_session",
            "web_access_create_session_and_chat",
            "web_access_list_sessions",
            "web_access_list_archived_sessions",
            "web_access_chat",
            "web_access_cancel_codex_acp",
            "web_access_ingest_file",
            "web_access_upload_attachment_chunk",
            "web_access_abort_attachment_upload",
            "web_access_discard_attachment",
            "web_access_read_conversation_attachment_chunk",
            "web_access_cancel_session_download",
            "web_access_load_session_chunk",
            "web_access_codex_acp_prompt",
            "web_access_get_codex_acp_timeline",
            "web_access_get_codex_acp_session_info",
            "web_access_get_codex_acp_pending_permissions",
            "web_access_get_codex_acp_pending_elicitations",
            "web_access_get_codex_workspace_changes",
            "web_access_get_codex_workspace_diff",
            "web_access_list_acp_agents",
            "web_access_get_acp_agent_status",
            "web_access_set_codex_acp_model",
            "web_access_set_codex_acp_mode",
            "web_access_set_codex_acp_config_option",
        ] {
            assert!(
                policy.commands.contains(command),
                "{command} must be Web-scoped"
            );
        }
        // The auxiliary conversation (aux session) commands: the WebUI
        // auxChat domain (bridge.js auxChatEnsure/auxChatReset)
        // depends on them directly; once allowed, the central validator's
        // Required("sessionId") scope constraint pins them to an explicit
        // session.
        for command in ["get_or_create_aux_session", "reset_aux_session"] {
            assert!(
                policy.commands.contains(command),
                "{command} must be allowed on Web (aux chat)"
            );
        }
        assert!(!policy.commands.contains("list_sessions"));
        assert!(!policy.commands.contains("list_archived_sessions"));
        // The native clipboard fallback must stay desktop-only: a browser relay
        // session must never be able to read the host clipboard.
        assert!(!policy.commands.contains("paste_clipboard_image"));
        assert!(policy.events.contains("chat:delta"));
        assert!(policy.events.contains("chat:reasoning_start"));
        assert!(policy.events.contains("chat:reasoning_delta"));
        assert!(policy.events.contains("chat:reasoning_done"));
        assert!(
            RUST_FORWARDED_EVENTS.contains(&"remote_control:tools_changed"),
            "the tools_changed broadcast must be relayed (web hot refresh) but deduped against the app.emit leg"
        );
        assert!(RUST_FORWARDED_EVENTS.contains(&"chat:reasoning_start"));
        assert!(RUST_FORWARDED_EVENTS.contains(&"chat:reasoning_delta"));
        assert!(RUST_FORWARDED_EVENTS.contains(&"chat:reasoning_done"));
        assert!(policy.events.contains("chat:steer_committed"));
        assert!(policy.events.contains("chat:steer_dropped"));
        assert!(RUST_FORWARDED_EVENTS.contains(&"chat:steer_committed"));
        assert!(RUST_FORWARDED_EVENTS.contains(&"chat:steer_dropped"));
        assert!(policy.events.contains("chat:user_message"));
        assert!(policy.events.contains("acp:event"));
        assert!(RUST_FORWARDED_EVENTS.contains(&"acp:event"));
        assert!(policy.events.contains("chat:plan_resolved"));
        assert!(RUST_FORWARDED_EVENTS.contains(&"chat:plan_resolved"));
        assert!(policy.events.contains("chat:transcript_committed"));
        assert!(policy.events.contains("session:deleted"));
        assert!(RUST_FORWARDED_EVENTS.contains(&"session:deleted"));
        assert!(policy.events.contains("session:list_changed"));
        assert!(RUST_FORWARDED_EVENTS.contains(&"session:list_changed"));
        assert!(policy.events.contains("session:model_changed"));
        assert!(RUST_FORWARDED_EVENTS.contains(&"session:model_changed"));
        assert!(policy.events.contains("session:persona_changed"));
        assert!(RUST_FORWARDED_EVENTS.contains(&"session:persona_changed"));
        assert!(policy.events.contains("multiagent:agent_progress"));
        assert!(RUST_FORWARDED_EVENTS.contains(&"multiagent:agent_progress"));
    }

    #[test]
    fn capability_snapshot_policy_is_bounded_before_web_access_starts() {
        let policy = AccessPolicy::load().expect("embedded policy");
        assert!(
            policy.capability_snapshot_wire_bytes().unwrap()
                <= relay_client::MAX_CONTROL_FRAME_BYTES
        );

        let mut oversized = AccessPolicy::default();
        oversized
            .commands
            .insert("x".repeat(relay_client::MAX_CONTROL_FRAME_BYTES));
        let error = oversized
            .validate_capability_snapshot()
            .expect_err("oversized capability snapshot must fail closed");
        assert!(error.contains("capability snapshot is too large"));
    }

    #[test]
    fn web_access_config_matches_relay_bounds_and_has_a_bounded_loader() {
        let valid = WebAccessConfig {
            relay_url: "wss://example.test/ws".into(),
            endpoint_id: "ep_123456".into(),
            access_token: "a".repeat(24),
            desktop_secret: "d".repeat(24),
            allow_host_workspace: false,
        };
        validate_config(&valid).expect("valid Web access config");

        let mut invalid = valid.clone();
        invalid.endpoint_id = "e".repeat(129);
        assert!(validate_config(&invalid).is_err());
        invalid = valid.clone();
        invalid.access_token = "a".repeat(1_025);
        assert!(validate_config(&invalid).is_err());
        invalid = valid.clone();
        invalid.relay_url = "wss://example.test/ws with-space".into();
        assert!(validate_config(&invalid).is_err());

        let directory = std::env::temp_dir().join(format!(
            "pinvou-web-config-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(8)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("web-access.json");
        std::fs::write(&path, serde_json::to_vec(&valid).unwrap()).unwrap();
        assert_eq!(load_config_from(&path).unwrap(), Some(valid));

        std::fs::write(&path, vec![b'x'; MAX_WEB_ACCESS_CONFIG_BYTES + 1]).unwrap();
        let error = load_config_from(&path).expect_err("oversized config must fail closed");
        assert!(error.contains("config is larger"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn pending_rpc(
        dispatched_generation: Option<&str>,
        acknowledged_generation: Option<&str>,
    ) -> RpcCacheEntry {
        RpcCacheEntry::Pending(PendingRpc {
            lease_id: "lease".into(),
            command: "chat".into(),
            args: json!({ "sessionId": "s1" }),
            fingerprint: "a".repeat(64),
            dispatched_generation: dispatched_generation.map(ToOwned::to_owned),
            acknowledged_generation: acknowledged_generation.map(ToOwned::to_owned),
            dispatched_at: Instant::now(),
        })
    }

    #[test]
    fn a_new_bridge_generation_redelivers_only_unacknowledged_rpcs() {
        let mut inner = Inner::default();
        inner.bridge_generation = Some("generation-old".into());
        inner
            .rpc_cache
            .insert("queued".into(), pending_rpc(Some("generation-old"), None));
        inner.rpc_order.push_back("queued".into());

        let first = prepare_bridge_generation(&mut inner, "generation-new");
        assert_eq!(first.len(), 1);
        assert!(matches!(
            &first[0],
            RpcRequestAction::Dispatch(dispatch)
                if dispatch.request_id == "queued"
                    && dispatch.command == "chat"
                    && dispatch.bridge_generation == "generation-new"
        ));
        assert!(prepare_bridge_generation(&mut inner, "generation-new").is_empty());
    }

    #[test]
    fn a_redispatch_refreshes_the_dispatch_clock() {
        // Round-10 review minor: without refreshing dispatched_at, a redispatch (webview
        // reload, re-dispatch after a begin persist failure) inherits the TTL clock of its
        // original admission — retries near the window's end are fabricated into
        // rpc_timed_out by the TTL sweep almost immediately.
        let mut inner = Inner::default();
        inner.bridge_generation = Some("generation-old".into());
        let stale = Instant::now() - RPC_IN_FLIGHT_TTL - Duration::from_secs(10);
        inner.rpc_cache.insert(
            "queued".into(),
            RpcCacheEntry::Pending(PendingRpc {
                lease_id: "lease".into(),
                command: "chat".into(),
                args: json!({ "sessionId": "s1" }),
                fingerprint: "a".repeat(64),
                dispatched_generation: Some("generation-old".into()),
                acknowledged_generation: None,
                dispatched_at: stale,
            }),
        );
        inner.rpc_order.push_back("queued".into());

        assert_eq!(
            prepare_bridge_generation(&mut inner, "generation-new").len(),
            1
        );
        let Some(RpcCacheEntry::Pending(pending)) = inner.rpc_cache.get("queued") else {
            panic!("the re-armed request must stay pending");
        };
        assert_ne!(pending.dispatched_at, stale, "the clock must be reset");
        assert!(
            !rpc_in_flight_expired(pending.dispatched_at, Instant::now()),
            "a freshly re-dispatched request must not be sweep-eligible"
        );
    }

    #[test]
    fn an_acknowledged_rpc_becomes_outcome_unknown_after_bridge_replacement() {
        let mut inner = Inner::default();
        inner.bridge_generation = Some("generation-old".into());
        inner.rpc_cache.insert(
            "started".into(),
            pending_rpc(Some("generation-old"), Some("generation-old")),
        );
        inner.rpc_order.push_back("started".into());

        assert!(prepare_bridge_generation(&mut inner, "generation-new").is_empty());
        let Some(RpcCacheEntry::Complete { completion, .. }) = inner.rpc_cache.get("started")
        else {
            panic!("started request must become a conservative completion");
        };
        assert!(!completion.ok);
        assert_eq!(completion.error_code.as_deref(), Some("outcome_unknown"));
    }

    #[test]
    fn rpc_tombstones_survive_serialization_and_reject_conflicting_reuse() {
        let fingerprint = rpc_fingerprint("chat", &json!({ "b": 2, "a": 1 })).unwrap();
        let mut ledger = RpcLedger::for_endpoint("ep_test");
        assert!(ledger.remember("request-1", &fingerprint).unwrap());
        let encoded = serde_json::to_vec(&ledger).unwrap();
        let restored: RpcLedger = serde_json::from_slice(&encoded).unwrap();
        let tombstone = restored.get("request-1").expect("durable tombstone");
        assert_eq!(
            tombstone_completion(tombstone, &fingerprint)
                .error_code
                .as_deref(),
            Some("request_not_started")
        );
        assert_eq!(
            tombstone_completion(tombstone, &"f".repeat(64))
                .error_code
                .as_deref(),
            Some("request_id_conflict")
        );
        let mut acknowledged = restored.clone();
        acknowledged
            .acknowledge("request-1", &fingerprint)
            .expect("acknowledge tombstone");
        assert_eq!(
            tombstone_completion(
                acknowledged
                    .get("request-1")
                    .expect("acknowledged tombstone"),
                &fingerprint,
            )
            .error_code
            .as_deref(),
            Some("outcome_unknown")
        );
    }

    #[test]
    fn rpc_ledger_retains_only_the_most_recent_512_ids() {
        let mut ledger = RpcLedger::for_endpoint("ep_test");
        for index in 0..=RPC_CACHE_CAPACITY {
            ledger
                .remember(&format!("request-{index}"), &"a".repeat(64))
                .unwrap();
        }
        assert_eq!(ledger.entries.len(), RPC_CACHE_CAPACITY);
        assert!(ledger.get("request-0").is_none());
        assert!(
            ledger
                .get(&format!("request-{RPC_CACHE_CAPACITY}"))
                .is_some()
        );
    }

    #[test]
    fn rejected_rpcs_never_consume_durable_tombstone_capacity() {
        let protected_fingerprint = rpc_fingerprint("delete_session", &json!({ "id": "s1" }))
            .expect("protected fingerprint");
        let mut ledger = RpcLedger::for_endpoint("ep_test");
        ledger
            .remember("protected-request", &protected_fingerprint)
            .expect("remember protected request");
        ledger
            .acknowledge("protected-request", &protected_fingerprint)
            .expect("acknowledge protected request");

        let invalid_scope = RpcRejection::deterministic(rpc_error_completion(
            "invalid_session_scope",
            "missing Session",
        ));
        for index in 0..RPC_CACHE_CAPACITY {
            let fingerprint = rpc_fingerprint("rejected", &json!({ "index": index }))
                .expect("rejected fingerprint");
            let admission = match index % 3 {
                0 => prepare_new_rpc_admission(
                    &ledger,
                    &format!("rejected-{index}"),
                    &fingerprint,
                    Some(&invalid_scope),
                    true,
                    0,
                ),
                1 => prepare_new_rpc_admission(
                    &ledger,
                    &format!("rejected-{index}"),
                    &fingerprint,
                    None,
                    false,
                    0,
                ),
                _ => prepare_new_rpc_admission(
                    &ledger,
                    &format!("rejected-{index}"),
                    &fingerprint,
                    None,
                    true,
                    MAX_RPC_IN_FLIGHT,
                ),
            }
            .expect("classify rejected request");
            assert!(matches!(admission, NewRpcAdmission::Rejected(_)));
        }

        assert_eq!(ledger.entries.len(), 1);
        let protected = ledger
            .get("protected-request")
            .expect("acknowledged tombstone must remain");
        assert!(protected.acknowledged);
        assert_eq!(protected.fingerprint, protected_fingerprint);
    }

    /// Wiring pin: the caching decision funnels through `settle_admission_rejection` and
    /// `settle_tombstone_outcome`; driving both entrances directly pins "only deterministic
    /// outcomes (including acknowledged tombstones) enter the Complete cache; transient
    /// refusals and fingerprint conflicts never do". Earlier mutation testing proved that
    /// when handle_rpc_request's wiring points were reverted to unconditional caching, the
    /// existing helper-level tests all stayed green — now that the decision funnels into a
    /// single point, the decision itself is pinned here.
    #[test]
    fn rpc_settle_entry_points_cache_only_cacheable_outcomes() {
        let mut inner = Inner::default();

        let transient = RpcRejection::transient(rpc_error_completion(
            "too_many_in_flight_requests",
            "正在运行的远程控制请求过多",
        ));
        let completion = RemoteControlManager::settle_admission_rejection(
            &mut inner,
            "transient-req",
            "fp-transient".to_string(),
            &transient,
        );
        assert_eq!(
            completion.error_code.as_deref(),
            Some("too_many_in_flight_requests")
        );
        assert!(
            !inner.rpc_cache.contains_key("transient-req"),
            "a transient rejection must never enter the completion cache"
        );
        assert!(
            inner.rpc_order.is_empty(),
            "transient rejections must not consume journal-order slots"
        );

        let deterministic = RpcRejection::deterministic(rpc_error_completion(
            "command_not_allowed",
            "远程控制不允许调用该命令",
        ));
        let completion = RemoteControlManager::settle_admission_rejection(
            &mut inner,
            "deterministic-req",
            "fp-deterministic".to_string(),
            &deterministic,
        );
        assert_eq!(
            completion.error_code.as_deref(),
            Some("command_not_allowed")
        );
        assert!(matches!(
            inner.rpc_cache.get("deterministic-req"),
            Some(RpcCacheEntry::Complete { fingerprint, .. }) if fingerprint == "fp-deterministic",
        ));

        let acknowledged = RpcTombstone {
            request_id: "tomb-req".to_string(),
            fingerprint: "fp-owner".to_string(),
            acknowledged: true,
        };
        let outcome = RemoteControlManager::settle_tombstone_outcome(
            &mut inner,
            "tomb-req",
            "fp-owner".to_string(),
            &acknowledged,
        );
        assert_eq!(outcome.error_code.as_deref(), Some("outcome_unknown"));
        assert!(matches!(
            inner.rpc_cache.get("tomb-req"),
            Some(RpcCacheEntry::Complete { fingerprint, .. }) if fingerprint == "fp-owner",
        ));

        // Fingerprint conflict: answer with the conflict, but never cache under either fingerprint.
        inner.rpc_cache.remove("tomb-req");
        let conflict = RemoteControlManager::settle_tombstone_outcome(
            &mut inner,
            "tomb-req",
            "fp-probe".to_string(),
            &acknowledged,
        );
        assert_eq!(conflict.error_code.as_deref(), Some("request_id_conflict"));
        assert!(
            !inner.rpc_cache.contains_key("tomb-req"),
            "a fingerprint conflict must not be cached under either fingerprint"
        );
        assert_eq!(
            inner
                .rpc_order
                .iter()
                .filter(|request_id| *request_id == "tomb-req")
                .count(),
            1,
            "the conflict must not push a second journal-order slot (the first \
             Complete settle legitimately holds one)"
        );
    }

    /// Wiring pin: begin's (side-effect barrier) ledger persist failure goes through the
    /// settle funnel — respond but never enter the Complete cache (the command has not
    /// executed, so retrying is safe); reverting to unconditional caching (the same class
    /// as the admission path's old replay poisoning) turns red. The error text also folds
    /// the storage-root prefix: it reaches the phone as rpc_response.error.
    #[test]
    fn begin_persist_failure_responds_without_caching() {
        let mut inner = Inner::default();
        inner.rpc_cache.insert(
            "begin-req".to_string(),
            RpcCacheEntry::Pending(PendingRpc {
                lease_id: "lease-1".to_string(),
                command: "get_memory_overview".to_string(),
                args: Value::Null,
                fingerprint: "fp-begin".to_string(),
                dispatched_generation: Some("gen-1".to_string()),
                acknowledged_generation: None,
                dispatched_at: Instant::now(),
            }),
        );

        // Round-15 review (S18 F7): this test reads PINVOU3_HOME directly (via pinvou3_home
        // and the redact_storage_prefix inside settle), while paths' other tests rewrite that
        // env in parallel within the same test binary — without ENV_LOCK this would
        // intermittently false-red or produce empty folds. Same scaffolding as the other
        // env-sensitive tests.
        let (_env_guard, _env_restore) =
            crate::platform::test_support::locked_env(&["PINVOU3_HOME"]);
        let raw_error = format!(
            "create {}: disk full",
            paths::pinvou3_home()
                .join("web-access-rpc-ledger.json")
                .display()
        );
        let completion = RemoteControlManager::settle_begin_persist_failure(
            &mut inner,
            "begin-req",
            "lease-1",
            &raw_error,
        );
        assert_eq!(
            completion.error_code.as_deref(),
            Some("rpc_ledger_unavailable")
        );
        assert!(
            !completion
                .error
                .as_deref()
                .is_some_and(|text| text.contains(&paths::pinvou3_home().display().to_string())),
            "the phone-visible error must not carry the absolute home path: {:?}",
            completion.error
        );
        assert!(
            matches!(
                inner.rpc_cache.get("begin-req"),
                Some(RpcCacheEntry::Pending(pending))
                    if pending.acknowledged_generation.is_none()
                        && pending.dispatched_generation.is_none(),
            ),
            "the pending entry must stay uncached but re-armed: the dispatch generation is \
             cleared so a retry re-dispatches and re-runs begin (Round-7: keeping the stale \
             generation silenced the retry until the TTL sweep fabricated a timeout)"
        );

        // Source contract (same shape as the revoke deadlock pin): begin_rpc must answer a
        // persist failure through the settle funnel, and the failure branch must not
        // introduce Complete caching again (the old implementation's prune_rpc_cache appeared
        // only on the cache path) — bypassing the funnel turns red. Round-29 review NIT:
        // the needles run on comment-filtered source like the sibling pins — the raw match
        // let a trailing comment mentioning `prune_rpc_cache` false-red and a comment
        // mentioning `settle_begin_persist_failure` false-green (the comment-mention
        // calibration class).
        let source = include_str!("mod.rs");
        let begin_start = source
            .find("pub fn begin_rpc(&self, request_id: &str, generation: &str)")
            .expect("begin_rpc must exist");
        let begin_tail = &source[begin_start..];
        let begin_body = &begin_tail[..begin_tail
            .find("\n    pub fn ")
            .expect("begin_rpc body must be terminated by the next fn")];
        let begin_body = comment_filtered(begin_body);
        assert!(
            begin_body.contains("settle_begin_persist_failure"),
            "begin_rpc must route the persist failure through the settle funnel"
        );
        assert!(
            !begin_body.contains("prune_rpc_cache"),
            "begin_rpc must not cache a completion on the persist-failure path"
        );
    }

    #[test]
    fn only_the_sweep_timeout_completion_is_replaceable() {
        let placeholder = rpc_error_completion(RPC_TIMED_OUT_CODE, "timed out");
        assert!(
            RemoteControlManager::rpc_completion_is_in_flight_timeout(&placeholder),
            "the TTL sweep's timeout completion must be recognized as the replaceable placeholder"
        );
        let real =
            bounded_rpc_completion(true, Some(serde_json::json!({ "ok": 1 })), None, None, None);
        assert!(
            !RemoteControlManager::rpc_completion_is_in_flight_timeout(&real),
            "a real completion must never be replaceable"
        );
        let rejection = rpc_error_completion("invalid_session_scope", "rejected");
        assert!(
            !RemoteControlManager::rpc_completion_is_in_flight_timeout(&rejection),
            "a deterministic rejection is a real terminal value, not a placeholder"
        );
        let failed = rpc_error_completion("web_session_chat_failed", "failed");
        assert!(
            !RemoteControlManager::rpc_completion_is_in_flight_timeout(&failed),
            "a real command failure is a real terminal value, not a placeholder"
        );
    }

    // Round-25 review MAJOR-2 source pins share one caliber: pure comment lines are
    // filtered before matching.
    // Round-27 review m14: the filtering primitive was lifted into platform::test_support
    // for sharing — this module's 14 call sites and sibling pins like persistence.rs
    // previously each inlined the same closure (rung-2: the helper must delete the
    // callers' code, not coexist with it).
    use crate::platform::test_support::comment_filtered;

    /// Wiring pin (source contract, same shape as the lease gate pin): complete_rpc's
    /// Complete arm must recognize TTL timeout placeholders through the shared predicate
    /// and let a real completion replace them — helper-level predicate tests cannot pin
    /// "the wiring point reverted to unconditional idempotent ignore" (the root disease of
    /// Round-5 review M1: the placeholder swallows the real result and phone replays get a
    /// stale timeout forever). Deleting the replacement branch or bypassing the predicate
    /// turns red.
    #[test]
    fn complete_rpc_supersedes_the_in_flight_timeout_placeholder() {
        let source = include_str!("mod.rs");
        let start = source
            .find("pub fn complete_rpc(")
            .expect("complete_rpc must exist");
        let tail = &source[start..];
        let end = ["\n    fn ", "\n    pub fn ", "\n    #[test]"]
            .iter()
            .filter_map(|marker| tail.find(marker))
            .min()
            .expect("complete_rpc body must be terminated by the next item");
        let body = comment_filtered(&tail[..end]);
        assert!(
            body.contains("rpc_completion_is_in_flight_timeout(completion)"),
            "the Complete arm must route the placeholder decision through the shared predicate"
        );
        // Round-15 review MA3: a predicate-negation mutation (`if !Self::…`) kept the whole
        // suite green under plain contains + insertion counting alone (mutant executed) —
        // real completions swallowed and terminal states overwritable by later duplicate
        // completes, reviving both round-5 regression classes at once. Pin the predicate
        // call's **un-negated** verbatim form inside the if arm: negation, deleting the if,
        // or renaming the predicate — any of those mutations turns red.
        assert!(
            body.contains("if Self::rpc_completion_is_in_flight_timeout(completion) {"),
            "the supersede decision must stay an un-negated if-arm calling the shared \
             predicate on completion in complete_rpc; negating it swallows real \
             completions and lets a later duplicate overwrite terminal values"
        );
        // Round-14 review minor: plain contains is satisfied by the Pending arm's own
        // insertion — deleting only the placeholder-replacement arm's insertion (leaving an
        // empty if) kept the whole suite green, reviving round-5's result-swallowing
        // regression. Require at least two insertions in the function body: the
        // placeholder-replacement arm + the Pending→Complete arm; missing either turns red.
        assert!(
            body.matches("rpc_cache.insert").count() >= 2,
            "a real completion arriving after the TTL sweep must replace the placeholder, \
             not be idempotently swallowed (both the supersede arm and the Pending arm \
             must insert)"
        );
    }

    /// Round-14 review minor: marked TTL timeout placeholders must be rewritten to
    /// outcome_unknown on a generation change (after restart/reload, replay gets the honest
    /// "outcome unknown" instead of a fabricated "timed out"). Deleting
    /// prepare_bridge_generation's rewrite arm or the sweep arm's marking turns this pin
    /// red.
    #[test]
    fn swept_acknowledged_placeholders_repair_to_outcome_unknown_on_generation_change() {
        let rpc_source = include_str!("rpc.rs");
        let start = rpc_source
            .find("pub(super) fn prepare_bridge_generation(")
            .expect("prepare_bridge_generation must exist");
        let body = &rpc_source[start..];
        let end = ["\npub(super) fn ", "\npub fn ", "\nfn ", "\n#[test]"]
            .iter()
            .filter_map(|marker| body.find(marker))
            .min()
            .expect("prepare_bridge_generation body must be terminated by the next item");
        let body = comment_filtered(&body[..end]);
        assert!(
            body.contains("swept_acknowledged"),
            "the generation arm must recognize swept acknowledged placeholders"
        );
        assert!(
            body.contains("RPC_TIMED_OUT_CODE") && body.contains("outcome_unknown_completion()"),
            "the repair must convert the timeout placeholder to the truthful outcome_unknown"
        );
        // Round-15 review (S18 F8, mutant analysis): loosening the repair match (dropping
        // the `!completion.ok` or RPC_TIMED_OUT_CODE leg so every marked placeholder —
        // including never-acknowledged ones — gets rewritten to outcome_unknown) still kept
        // the plain contains above green. Pin the verbatim text of the match's three
        // conjunct legs: deleting any leg turns red.
        assert!(
            body.contains(
                "(*swept_acknowledged
                    && !completion.ok
                    && completion.error_code.as_deref() == Some(RPC_TIMED_OUT_CODE))"
            ),
            "the repair predicate must keep all three legs (swept-marked, failed, \
             timeout-coded); loosening any leg rewrites never-executed requests' \
             honest timeouts into fabricated outcome_unknown",
        );

        let source = include_str!("mod.rs");
        let start = source
            .find("let expired: Vec<String> = inner")
            .expect("the TTL sweep must exist");
        let body = &source[start..];
        let end = body
            .find("let in_flight_count = inner")
            .expect("the sweep body must be terminated by the in-flight count");
        let body = comment_filtered(&body[..end]);
        assert!(
            body.contains("pending.acknowledged_generation.is_some()"),
            "the sweep must mark placeholders for acknowledged (still-executing) requests"
        );
    }

    #[test]
    fn rpc_command_and_error_envelopes_are_bounded() {
        assert!(validate_rpc_command("web_access_chat").is_ok());
        assert!(validate_rpc_command("session:resume-2").is_ok());
        assert!(validate_rpc_command("").is_err());
        assert!(validate_rpc_command(&"x".repeat(MAX_RPC_COMMAND_BYTES + 1)).is_err());
        assert!(validate_rpc_command("chat/../../native").is_err());

        let completion = rpc_error_completion("test", "e".repeat(20_000));
        assert_eq!(
            completion.error.as_deref().unwrap_or_default().len(),
            16_384
        );
    }

    #[test]
    fn native_active_session_fallbacks_are_not_valid_web_scopes() {
        for command in [
            // `cancel_codex_acp` is not listed here: it is desktop-only and
            // absent from the Web access policy, so it never reaches the scope
            // table (the table no longer carries a dead entry for it).
            "cancel_generation",
            "compact_now",
            "edit_last_turn",
            "get_session_timeline",
            "web_access_chat",
            "web_access_cancel_codex_acp",
            "web_access_get_codex_workspace_changes",
            "web_access_get_codex_workspace_diff",
        ] {
            assert_eq!(
                web_session_scope(command),
                Some(WebSessionScope::Required("sessionId")),
                "{command} must require the browser-selected Session"
            );
        }
    }

    #[test]
    fn session_scoped_commands_use_the_central_validator() {
        let scoped_commands = [
            ("get_session_timeline", "sessionId"),
            ("web_access_cancel_session_download", "id"),
            ("web_access_load_session_chunk", "id"),
            ("web_access_artifact_info", "sessionId"),
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
            ("get_or_create_aux_session", "sessionId"),
            ("reset_aux_session", "sessionId"),
            ("delete_session", "id"),
            ("rename_session", "id"),
            ("set_session_model", "sessionId"),
            ("set_session_archived", "id"),
            ("set_session_pinned", "id"),
            ("save_session_artifacts", "id"),
            ("get_session_pinvou_scene_events", "sessionId"),
            ("save_session_pinvou_scene_events", "sessionId"),
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
    fn oversized_rpc_results_become_small_structured_errors() {
        let completion = bounded_rpc_completion(
            true,
            Some(Value::String("x".repeat(MAX_RPC_RESPONSE_BYTES + 1))),
            None,
            None,
            None,
        );
        assert!(!completion.ok);
        assert_eq!(completion.error_code.as_deref(), Some("response_too_large"));
        assert!(serde_json::to_vec(&completion.result).unwrap().len() < 1024);
    }

    #[test]
    fn js_rpc_completions_carry_error_code_and_category_to_the_browser() {
        // The desktop proxy strips the structured VoiceCommandError object down
        // to its message; the code/category must ride the completion so the web
        // lane's normalizeVoiceError can map the trilingual copy.
        let completion = bounded_rpc_completion(
            false,
            None,
            Some("识别超时，请重试".to_string()),
            Some("asr_timeout".to_string()),
            Some("timeout".to_string()),
        );
        let response = rpc_response("endpoint-1", "lease-1", "req-1", &completion);
        assert_eq!(response["error_code"], "asr_timeout");
        assert_eq!(response["error_category"], "timeout");
        assert_eq!(response["error"], "识别超时，请重试");
        // Structured completions from the manager itself carry no category.
        let internal = rpc_error_completion("response_too_large", "too large");
        let internal_response = rpc_response("endpoint-1", "lease-1", "req-2", &internal);
        assert_eq!(internal_response["error_code"], "response_too_large");
        assert!(internal_response["error_category"].is_null());
    }

    #[test]
    fn process_lock_rejects_a_second_desktop_owner() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-web-access-lock-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("owner.lock");
        let first = acquire_process_lock(&path).expect("first owner");
        assert!(acquire_process_lock(&path).is_err());
        drop(first);
        let replacement = acquire_process_lock(&path).expect("replacement after release");
        drop(replacement);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rpc_fingerprint_is_independent_of_object_key_order() {
        assert_eq!(
            rpc_fingerprint("chat", &json!({ "a": 1, "b": { "x": 2, "y": 3 } })).unwrap(),
            rpc_fingerprint("chat", &json!({ "b": { "y": 3, "x": 2 }, "a": 1 })).unwrap()
        );
    }

    /// The web read-only denylist: all execution-type entrances are in the table, view-type
    /// ones are not (the read-only banner needs its data). `cancel_generation`,
    /// `cancel_shell_task`, and `cancel_user_input` are all cancellation-type entrances that
    /// change a running turn's state (the first terminates a whole generation and cascades
    /// to sub-agents; the latter two can stop shell tasks delegated sub-agents are running),
    /// so they must be banned together.
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
            "web_access_cancel_session_download",
            "web_access_load_session_chunk",
        ] {
            assert!(
                !super::MULTI_AGENT_WEB_EXECUTION_DENYLIST.contains(&command),
                "只读查看 {command} 必须放行——Web 只读横幅要能取数"
            );
        }
    }

    #[test]
    fn stream_replays_an_exact_contiguous_suffix() {
        let mut stream = StreamState::new();
        record_stream_event(&mut stream, "chat:delta", json!({ "text": "a" }));
        record_stream_event(&mut stream, "chat:delta", json!({ "text": "b" }));
        let replay = stream.replay_after(1).expect("replay");
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].seq, 2);
        assert_eq!(replay[0].payload["text"], "b");
        assert!(stream.replay_after(3).is_none());
    }

    #[test]
    fn fresh_client_baseline_skips_history_but_keeps_ready_window_events() {
        let mut stream = StreamState::new();
        record_stream_event(&mut stream, "chat:done", json!({ "turn": "historical" }));
        let baseline_at_join = stream.seq;
        record_stream_event(&mut stream, "chat:turn_started", json!({ "turn": "live" }));

        let replay = stream.replay_after(baseline_at_join).expect("fresh replay");
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].event, "chat:turn_started");
        assert_eq!(replay[0].payload["turn"], "live");
    }

    #[test]
    fn bounded_stream_rejects_a_cursor_older_than_the_journal() {
        let mut stream = StreamState::new();
        for seq in 0..=JOURNAL_CAPACITY {
            record_stream_event(&mut stream, "chat:delta", json!(seq));
        }
        assert!(stream.replay_after(0).is_none());
        assert!(stream.replay_after(1).is_some());
    }

    #[test]
    fn oversized_complete_event_frame_rotates_epoch_without_recording_the_event() {
        let mut stream = StreamState::new();
        record_stream_event(&mut stream, "chat:delta", json!({ "text": "safe" }));
        let previous_epoch = stream.epoch.clone();
        let oversized_payload = json!({ "text": "x".repeat(400) });
        let test_wire_limit = 512;
        assert!(serde_json::to_vec(&oversized_payload).unwrap().len() < test_wire_limit);

        let error = stream
            .record_with_limits(
                TEST_ENDPOINT_ID,
                TEST_LEASE_ID,
                "chat:delta".into(),
                oversized_payload,
                test_wire_limit,
                JOURNAL_CAPACITY,
                JOURNAL_BYTES_CAPACITY,
            )
            .expect_err("the complete envelope, not just payload, must be bounded");
        assert!(matches!(
            error,
            StreamRecordError::Oversized {
                wire_bytes,
                limit: 512
            } if wire_bytes > 512
        ));
        assert_ne!(stream.epoch, previous_epoch);
        assert_eq!(stream.seq, 0);
        assert!(stream.journal.is_empty());
        assert_eq!(stream.journal_bytes, 0);
        assert!(
            stream
                .replay_after(0)
                .is_some_and(|events| events.is_empty())
        );

        stream
            .record_with_limits(
                TEST_ENDPOINT_ID,
                TEST_LEASE_ID,
                "chat:done".into(),
                json!({ "status": "failed" }),
                test_wire_limit,
                JOURNAL_CAPACITY,
                JOURNAL_BYTES_CAPACITY,
            )
            .expect("new epoch remains usable");
        let replay = stream.replay_after(0).expect("reconnect replay");
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].seq, 1);
        assert_eq!(replay[0].event, "chat:done");
    }

    #[test]
    fn stream_journal_evicts_oldest_complete_frames_by_total_bytes() {
        let mut stream = StreamState::new();
        stream
            .record_with_limits(
                TEST_ENDPOINT_ID,
                TEST_LEASE_ID,
                "chat:delta".into(),
                json!({ "text": "same-size" }),
                4 * 1024,
                JOURNAL_CAPACITY,
                usize::MAX,
            )
            .unwrap();
        let frame_bytes = stream.journal.back().unwrap().wire_bytes;
        let byte_capacity = frame_bytes * 2;
        for _ in 0..2 {
            stream
                .record_with_limits(
                    TEST_ENDPOINT_ID,
                    TEST_LEASE_ID,
                    "chat:delta".into(),
                    json!({ "text": "same-size" }),
                    4 * 1024,
                    JOURNAL_CAPACITY,
                    byte_capacity,
                )
                .unwrap();
        }

        assert_eq!(stream.seq, 3);
        assert_eq!(stream.journal.len(), 2);
        assert_eq!(stream.journal.front().map(|event| event.seq), Some(2));
        assert!(stream.journal_bytes <= byte_capacity);
        assert!(stream.replay_after(0).is_none());
        assert_eq!(stream.replay_after(1).unwrap().len(), 2);
    }

    #[test]
    fn failed_live_enqueue_can_rebase_the_critical_tail_for_reconnect() {
        let mut stream = StreamState::new();
        record_stream_event(&mut stream, "chat:delta", json!({ "text": "prefix" }));
        record_stream_event(
            &mut stream,
            "chat:user_input_required",
            json!({ "request_id": "request-1" }),
        );
        let failed_event = stream.journal.back().unwrap().clone();
        let previous_epoch = stream.epoch.clone();

        stream.reset();
        stream
            .record(
                TEST_ENDPOINT_ID,
                TEST_LEASE_ID,
                failed_event.event,
                failed_event.payload,
            )
            .unwrap();

        assert_ne!(stream.epoch, previous_epoch);
        assert_eq!(stream.seq, 1);
        let replay = stream.replay_after(0).unwrap();
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].event, "chat:user_input_required");
        assert_eq!(replay[0].payload["request_id"], "request-1");
    }

    #[test]
    fn replay_batch_failure_stops_at_the_gap_instead_of_claiming_the_tail() {
        let messages = vec![
            json!({ "seq": 1 }),
            json!({ "seq": 2 }),
            json!({ "seq": 3 }),
        ];
        let mut attempted = Vec::new();
        let complete = try_enqueue_message_batch(messages, |message| {
            let seq = message["seq"].as_u64().unwrap();
            attempted.push(seq);
            seq != 2
        });

        assert!(!complete);
        assert_eq!(attempted, vec![1, 2]);
        let reset = stream_reset_message(
            TEST_ENDPOINT_ID,
            TEST_LEASE_ID,
            "epoch_recovered",
            "replay_enqueue_failed",
        );
        assert_eq!(reset["seq"], 0);
        assert_eq!(reset["reason"], "replay_enqueue_failed");
        assert!(serde_json::to_vec(&reset).unwrap().len() < relay_client::MAX_RELAY_FRAME_BYTES);
    }

    #[test]
    fn public_link_keeps_secrets_in_the_fragment_and_encodes_relay() {
        let config = WebAccessConfig {
            relay_url: "ws://127.0.0.1:8787/ws?x=1".into(),
            endpoint_id: "ep_test".into(),
            access_token: "a+b/c".into(),
            desktop_secret: "never-in-url".into(),
            allow_host_workspace: false,
        };
        let link = public_url(&config);
        assert!(link.contains("#endpoint=ep_test&token=a%2Bb%2Fc"));
        assert!(link.contains("relay=ws%3A%2F%2F127.0.0.1%3A8787%2Fws%3Fx%3D1"));
        assert!(!link.contains("never-in-url"));
    }

    #[test]
    fn private_json_write_atomically_replaces_the_target() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-web-access-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("state.json");
        atomic_write_private_json(&path, &json!({ "value": 1 }), "test state").unwrap();
        atomic_write_private_json(&path, &json!({ "value": 2 }), "test state").unwrap();
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["value"], 2);
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        assert!(platform::private_file_is_restricted(&path).unwrap());
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn test_pending_revocation(endpoint_id: &str) -> PendingRevocation {
        PendingRevocation {
            relay_url: "wss://relay.example.test/ws".into(),
            endpoint_id: endpoint_id.into(),
            desktop_secret: format!("desktop-secret-{endpoint_id}-0123456789"),
        }
    }

    fn test_web_config(endpoint_id: &str) -> WebAccessConfig {
        let pending = test_pending_revocation(endpoint_id);
        WebAccessConfig {
            relay_url: pending.relay_url,
            endpoint_id: pending.endpoint_id,
            access_token: "browser-token-01234567890123456789".into(),
            desktop_secret: pending.desktop_secret,
            allow_host_workspace: false,
        }
    }

    /// Round-17 review: stop/refresh's full-queue degradation arms discriminate by the
    /// shared prefix PENDING_REVOCATION_QUEUE_FULL_PREFIX, but the binding between the
    /// error text and that constant had no pin — a mutation changing persistence.rs's
    /// format! back to a different-wording literal would break the degradation arm's match,
    /// revive the full-queue wedge regression, and keep the whole suite green (the
    /// degradation-arm pins only assert "references the constant and has no `?`", blind to
    /// text drift). Pin the discrimination contract itself: a full-queue error must start
    /// with the shared prefix.
    #[test]
    fn full_revocation_queue_error_carries_the_shared_degrade_prefix() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-revocation-queue-full-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("pending.json");
        for index in 0..MAX_PENDING_REVOCATIONS {
            queue_pending_revocation_at(
                &path,
                test_pending_revocation(&format!("ep_queue_full_{index}")),
            )
            .expect("filling the queue exactly to capacity must succeed");
        }
        let overflow =
            queue_pending_revocation_at(&path, test_pending_revocation("ep_queue_full_overflow"))
                .expect_err("the entry past capacity must be refused");
        assert!(
            overflow.starts_with(PENDING_REVOCATION_QUEUE_FULL_PREFIX),
            "the full-queue error must carry the shared degrade prefix so the \
             stop/refresh recovery arms can recognize it: {overflow}"
        );
        std::fs::remove_dir_all(directory).ok();
    }

    /// Round-29 review MINOR-1: the in-memory backlog must never grow past
    /// MAX_PENDING_REVOCATIONS — a salvage degradation that leaves memory-only
    /// siblings plus later distinct-key rotations used to push it over, and
    /// every acknowledgement persist then failed the ledger validation cap
    /// forever (no revocation could be acked in that process). The cap keeps
    /// the newest entries in order and retires the oldest; a no-op cap (the
    /// pre-fix behavior) or a newest-retiring cap turns this red.
    #[test]
    fn pending_revocation_backlog_cap_keeps_the_newest_entries() {
        let over_cap: Vec<PendingRevocation> = (0..MAX_PENDING_REVOCATIONS + 2)
            .map(|index| test_pending_revocation(&format!("ep_backlog_cap_{index:02}")))
            .collect();
        let mut backlog = over_cap.clone();
        cap_pending_revocation_backlog(&mut backlog);
        assert_eq!(
            backlog.len(),
            MAX_PENDING_REVOCATIONS,
            "the backlog must land exactly on the ledger validation cap"
        );
        assert!(
            backlog
                .iter()
                .all(|entry| entry.endpoint_id.starts_with("ep_backlog_cap_")),
            "only fixture entries may remain after the cap"
        );
        // The OLDEST entries retire: with memory-first seeding the front of
        // the backlog is the oldest intent, and a salvage leftover set is
        // exactly that — retiring the newest instead (or keeping everything,
        // the pre-fix wedge) breaks these two directions.
        assert!(
            backlog
                .iter()
                .all(|entry| entry.endpoint_id != "ep_backlog_cap_00"
                    && entry.endpoint_id != "ep_backlog_cap_01"),
            "the two entries past the cap must be the retired oldest, not the newest"
        );
        assert_eq!(
            backlog.last().map(|entry| entry.endpoint_id.as_str()),
            Some("ep_backlog_cap_33"),
            "the newest entry must survive the cap so the last rotation stays ackable"
        );
        // Exactly at cap the helper is an identity — a cap that trims even
        // one entry below the limit silently loses ackable intents.
        let mut at_cap: Vec<PendingRevocation> = (0..MAX_PENDING_REVOCATIONS)
            .map(|index| test_pending_revocation(&format!("ep_backlog_fit_{index:02}")))
            .collect();
        cap_pending_revocation_backlog(&mut at_cap);
        assert_eq!(at_cap.len(), MAX_PENDING_REVOCATIONS);
        assert_eq!(
            at_cap.first().map(|entry| entry.endpoint_id.as_str()),
            Some("ep_backlog_fit_00"),
            "at-cap backlogs must pass through unchanged"
        );
    }

    /// Round-29 review MINOR-1 wiring pin: the cap must be bound at BOTH backlog
    /// growth sites — the replay union (salvage leftovers + disk ledger) and
    /// the post-queue sync. Deleting either call keeps the behavioral cap
    /// test green (it drives the helper directly), so each site carries its
    /// own needle; deleting the helper body's retire direction is covered by
    /// the behavioral test.
    #[test]
    fn revocation_backlog_cap_is_wired_into_replay_and_sync() {
        let source = include_str!("mod.rs");
        let replay_start = source
            .find("fn replay_pending_revocations(")
            .expect("replay_pending_revocations must exist");
        let replay_body = &source[replay_start..];
        let replay_body = &replay_body[..replay_body
            .find("\n    fn ")
            .expect("replay_pending_revocations body must be terminated by the next fn")];
        let replay_body = comment_filtered(replay_body);
        assert!(
            replay_body.contains("cap_pending_revocation_backlog(&mut merged)"),
            "the replay union must run the backlog cap before adopting the \
             merged backlog, or salvage leftovers plus rotations re-wedge \
             acknowledgement persists"
        );
        let sync_start = source
            .find("fn sync_pending_revocation_backlog(")
            .expect("sync_pending_revocation_backlog must exist");
        let sync_body = &source[sync_start..];
        let sync_body = &sync_body[..sync_body
            .find("\n    fn ")
            .expect("sync_pending_revocation_backlog body must be terminated by the next fn")];
        let sync_body = comment_filtered(sync_body);
        let push_at = sync_body
            .find("pending_revocation_backlog.push(pending)")
            .expect("the sync must still push the queued entry");
        let cap_at = sync_body
            .find("cap_pending_revocation_backlog(&mut inner.pending_revocation_backlog)")
            .expect("the post-queue sync must run the backlog cap");
        assert!(
            push_at < cap_at,
            "the cap must run after the push, or the just-queued entry can be \
             the one silently over cap"
        );
    }

    #[test]
    fn pending_revocations_round_trip_in_a_bounded_private_ledger() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-web-revocation-roundtrip-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("pending.json");
        let first = test_pending_revocation("ep_retired_one");
        let second = test_pending_revocation("ep_retired_two");

        queue_pending_revocation_at(&path, first.clone()).unwrap();
        queue_pending_revocation_at(&path, second.clone()).unwrap();
        // An idempotent retry must not grow the durable queue.
        queue_pending_revocation_at(&path, first.clone()).unwrap();

        let restored = load_pending_revocations_from(&path).unwrap();
        assert_eq!(restored.version, PENDING_REVOCATIONS_VERSION);
        assert_eq!(restored.entries, VecDeque::from([first, second]));
        assert!(platform::private_file_is_restricted(&path).unwrap());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn corrupt_pending_revocation_ledger_does_not_block_rotation_or_stop() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-web-revocation-corrupt-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("pending.json");
        std::fs::write(&path, b"{corrupt json").unwrap();

        // Content corruption classifies as Corrupt: queue (a mandatory point on stop/rotate)
        // continues from an empty ledger and overwrites the bad file with legitimate content —
        // the old endpoint's revocation must not be wedged forever by a bad file.
        // load_pending_revocations_from still reports errors honestly outward; the fail-closed
        // semantics are unchanged.
        let pending = test_pending_revocation("ep_retired_corrupt");
        queue_pending_revocation_at(&path, pending.clone()).unwrap();
        let restored = load_pending_revocations_from(&path).unwrap();
        assert_eq!(restored.entries, VecDeque::from([pending]));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_ledger_salvages_valid_entries_and_quarantines_the_original() {
        // A parseable ledger that fails whole-file validation (here: one entry with an
        // illegal endpoint_id plus one duplicate sharing a valid entry's key) must have its
        // legitimate entries salvaged one by one with the originals quarantined, rather than
        // being rewritten wholesale — wholesale discard would keep the old endpoint joinable
        // on the relay side.
        // The duplicate is byte-identical to valid_a (same relay_url + endpoint_id = same
        // key): salvage skips it per "keep the first for duplicate keys" — if dedup dropped
        // the key, the salvage result would gain one entry and this pin's count assertion
        // would turn red (round-5 review: the original fixture changed the endpoint_id, so
        // the key differed and the duplicate-key branch was never truly pinned).
        let directory = std::env::temp_dir().join(format!(
            "pinvou-ledger-salvage-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("pending.json");

        let valid_a = PendingRevocation::from(&test_web_config("ep_salv_a"));
        let valid_b = PendingRevocation::from(&test_web_config("ep_salv_b"));
        let mut invalid = serde_json::to_value(&valid_a).unwrap();
        invalid["endpoint_id"] = json!("bad id");
        // Round-9 review minor: the duplicate shares valid_a's key (relay_url + endpoint_id)
        // but has a different desktop_secret — "keep first" and "keep last" are no longer
        // equivalent, so the pin genuinely discriminates (the old fixture's duplicate was a
        // byte-for-byte copy, both orders behaved identically, and keep-first was never
        // pinned).
        let mut duplicate = serde_json::to_value(&valid_a).unwrap();
        duplicate["desktop_secret"] = json!("stale-keep-last-probe-secret-0123456789");
        let body = json!({
            "version": PENDING_REVOCATIONS_VERSION,
            "entries": [
                serde_json::to_value(&valid_a).unwrap(),
                invalid,
                duplicate,
                serde_json::to_value(&valid_b).unwrap(),
            ],
        });
        std::fs::write(&path, serde_json::to_vec(&body).unwrap()).unwrap();

        // Salvage persists: after a bare load, the on-disk ledger must already hold the
        // salvaged legitimate entries (the quarantine copy only keeps evidence and is never
        // auto-replayed) — otherwise a crash before the worker ack would permanently lose
        // this batch of pending revocations.
        let salvaged = load_pending_revocations_from(&path).unwrap();
        assert_eq!(
            salvaged.entries.len(),
            2,
            "the two valid entries must be salvaged in memory"
        );
        let persisted: PendingRevocationLedger =
            serde_json::from_slice(&std::fs::read(&path).unwrap())
                .expect("the salvaged ledger must have been persisted as valid JSON");
        assert_eq!(
            persisted.entries.len(),
            2,
            "salvaged entries must be persisted immediately, not only in memory"
        );
        let persisted_keys: Vec<_> = persisted
            .entries
            .iter()
            .map(pending_revocation_key)
            .collect();
        assert!(persisted_keys.contains(&pending_revocation_key(&valid_a)));
        assert!(persisted_keys.contains(&pending_revocation_key(&valid_b)));
        // Round-9 review minor: a duplicate same-key entry must keep the "first" one's
        // credentials — keeping the last would swap the pending revocation record to old
        // ciphertext (the revocation worker authenticating with the old secret would be
        // refused, leaving the entry hanging forever).
        let persisted_a = persisted
            .entries
            .iter()
            .find(|entry| pending_revocation_key(entry) == pending_revocation_key(&valid_a))
            .expect("the first duplicate's key must be salvaged");
        assert_eq!(
            persisted_a.desktop_secret, valid_a.desktop_secret,
            "keep-first must preserve the first duplicate's credentials"
        );

        let fresh = PendingRevocation::from(&test_web_config("ep_salv_c"));
        queue_pending_revocation_at(&path, fresh)
            .expect("queue must salvage the valid entries and continue");

        let restored = load_pending_revocations_from(&path).unwrap();
        assert_eq!(
            restored.entries.len(),
            3,
            "two salvaged entries plus the freshly queued one must survive"
        );
        let keys: Vec<_> = restored
            .entries
            .iter()
            .map(pending_revocation_key)
            .collect();
        assert!(keys.contains(&pending_revocation_key(&valid_a)));
        assert!(keys.contains(&pending_revocation_key(&valid_b)));
        assert!(
            directory
                .read_dir()
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .contains("pending.json.corrupt-")),
            "the original invalid ledger must be quarantined for inspection"
        );
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn unparseable_ledger_is_quarantined_before_the_fresh_start() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-ledger-quarantine-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("pending.json");
        std::fs::write(&path, b"{corrupt json").unwrap();

        let pending = PendingRevocation::from(&test_web_config("ep_quar_a"));
        queue_pending_revocation_at(&path, pending.clone())
            .expect("an unparseable ledger must not block the recovery path");
        assert!(
            directory
                .read_dir()
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .contains("pending.json.corrupt-")),
            "the unparseable ledger must be quarantined, not silently overwritten"
        );
        let restored = load_pending_revocations_from(&path).unwrap();
        assert_eq!(restored.entries.len(), 1);
        assert_eq!(restored.entries[0], pending);
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn loading_pending_revocations_repairs_world_readable_private_files() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-revocation-perm-repair-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("pending.json");
        let pending = test_pending_revocation("ep_perm_repair");
        queue_pending_revocation_at(&path, pending).unwrap();
        // POSIX modes only exist on unix; the mode manipulation lives behind
        // the platform adapter so this file carries no target selector.
        if !crate::platform::test_support::force_private_file_mode(&path, 0o644) {
            std::fs::remove_dir_all(directory).unwrap();
            return;
        }

        // This file holds a desktop_secret: load must restore the 0600 baseline (same as config).
        assert!(load_pending_revocations_from(&path).is_ok());
        assert_eq!(
            crate::platform::test_support::permission_bits(&path),
            Some(0o600),
            "load must restore the private 0600 baseline for the secret-bearing ledger"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn revocation_ack_removes_only_the_matching_durable_entry() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-web-revocation-ack-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("pending.json");
        let first = test_pending_revocation("ep_retired_one");
        let second = test_pending_revocation("ep_retired_two");
        queue_pending_revocation_at(&path, first.clone()).unwrap();
        queue_pending_revocation_at(&path, second.clone()).unwrap();

        acknowledge_pending_revocation_at(&path, &first).unwrap();
        assert_eq!(
            load_pending_revocations_from(&path).unwrap().entries,
            VecDeque::from([second.clone()])
        );
        acknowledge_pending_revocation_at(&path, &second).unwrap();
        assert!(!path.exists(), "an empty revocation ledger is removed");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn restart_replays_retired_entries_but_not_an_uncommitted_current_one() {
        let current = test_web_config("ep_current");
        let retired = test_pending_revocation("ep_retired");
        let prepared_current = PendingRevocation::from(&current);
        let encoded = serde_json::to_vec(&PendingRevocationLedger {
            version: PENDING_REVOCATIONS_VERSION,
            entries: VecDeque::from([prepared_current.clone(), retired.clone()]),
        })
        .unwrap();
        let restored: PendingRevocationLedger = serde_json::from_slice(&encoded).unwrap();

        assert_eq!(
            eligible_pending_revocations(&restored, Some(&current)),
            vec![retired.clone()]
        );
        assert_eq!(
            eligible_pending_revocations(&restored, None),
            vec![prepared_current, retired]
        );
    }

    #[test]
    fn only_explicit_terminal_relay_messages_acknowledge_a_revocation() {
        assert!(relay_client::terminal_relay_message(
            &json!({
                "type": "desktop_endpoint_revoked",
                "endpoint_id": "ep_retired"
            }),
            "ep_retired"
        ));
        assert!(relay_client::terminal_relay_message(
            &json!({
                "type": "desktop_endpoint_replaced",
                "endpoint_id": "ep_retired"
            }),
            "ep_retired"
        ));
        assert!(relay_client::terminal_relay_message(
            &json!({ "type": "error", "code": "endpoint_not_found" }),
            "ep_retired"
        ));
        // An endpoint_not_found naming a different endpoint is not proof this endpoint is
        // gone: acking on it would delete this endpoint's persisted revocation entry, which
        // may still exist on the relay side.
        assert!(!relay_client::terminal_relay_message(
            &json!({
                "type": "error",
                "code": "endpoint_not_found",
                "endpoint_id": "ep_other"
            }),
            "ep_retired"
        ));
        // The endpoint_revoked error code proves the endpoint is tombstoned — equivalent to a successful revocation.
        assert!(relay_client::terminal_relay_message(
            &json!({ "type": "error", "code": "endpoint_revoked" }),
            "ep_retired"
        ));
        // A credential refusal does not prove the endpoint is gone: the persisted revocation entry must not be treated as confirmed-deleted.
        assert!(!relay_client::terminal_relay_message(
            &json!({ "type": "error", "code": "invalid_desktop_secret" }),
            "ep_retired"
        ));
        assert!(!relay_client::terminal_relay_message(
            &json!({
                "type": "desktop_endpoint_revoked",
                "endpoint_id": "ep_other"
            }),
            "ep_retired"
        ));
    }

    #[test]
    fn active_session_downloads_are_rejected_not_evicted() {
        let download = |id: &str, reserved_bytes: usize| {
            (
                id.to_string(),
                WebSessionDownload {
                    session_id: format!("session_{id}"),
                    reservation_id: format!("reservation_{id}"),
                    file: tempfile::tempfile().unwrap(),
                    reserved_bytes,
                    total: reserved_bytes,
                    ready: true,
                    last_touched: Instant::now(),
                },
            )
        };

        let mut by_count = Inner::default();
        by_count
            .web_session_downloads
            .extend([download("first", 1), download("second", 1)]);
        let ids_before = by_count
            .web_session_downloads
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        assert!(ensure_web_session_download_capacity(&by_count, 1).is_err());
        assert_eq!(
            by_count
                .web_session_downloads
                .keys()
                .cloned()
                .collect::<HashSet<_>>(),
            ids_before
        );

        let mut by_bytes = Inner::default();
        by_bytes
            .web_session_downloads
            .extend([download("large", MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES - 1)]);
        assert!(ensure_web_session_download_capacity(&by_bytes, 2).is_err());
        assert!(by_bytes.web_session_downloads.contains_key("large"));
    }

    #[test]
    fn eviction_active_window_stays_far_below_the_transfer_ttl() {
        // Round-29 review MINOR-3: both eviction tests derive fixture age FROM
        // the constant, so drifting the value itself (e.g. 30s → 590s, hugging
        // the 10-minute TTL) kept the whole suite green while the "orphans are
        // reaped far earlier than the TTL" property silently died — a phone
        // holding a download_id could then wait out nearly the whole TTL
        // before losing it to the reaper. Pin the value and the relationship
        // independent of the constants' use as fixture clocks; changing either
        // bound turns this red.
        assert_eq!(
            WEB_SESSION_DOWNLOAD_EVICTION_ACTIVE_WINDOW,
            std::time::Duration::from_secs(30),
            "the freshness window is a deliberate 30s value, not derivable from \
             the other bounds"
        );
        assert!(
            WEB_SESSION_DOWNLOAD_EVICTION_ACTIVE_WINDOW * 10 <= WEB_SESSION_TRANSFER_TTL,
            "the window must stay an order of magnitude below the TTL so eviction \
             reaps orphans far earlier than the TTL reaper"
        );
    }

    #[test]
    fn full_download_slots_evict_the_oldest_ready_orphan() {
        // Round-15 review: at full slots, begin first evicts the oldest ready orphan
        // (response lost; the phone never got a download_id); entries not yet ready that the
        // engine is still writing must never be victims, keeping the existing full-slot error
        // in that case. Round-16 review (M2): the victim criteria gain a freshness window —
        // "ready" cannot tell an orphan from a download being read (chunk reads refresh
        // last_touched); only ready entries whose updates have aged past the window are
        // evictable; ready entries inside the window are kept too.
        let aged = |id: &str, ready: bool| {
            (
                id.to_string(),
                WebSessionDownload {
                    session_id: "session_a".into(),
                    reservation_id: format!("reservation_{id}"),
                    file: tempfile::tempfile().unwrap(),
                    reserved_bytes: 1,
                    total: 1,
                    ready,
                    // Round-18 review: direct subtraction panics on hosts whose monotonic clock
                    // has been up for less than 31s (freshly booted) — a test crash rather than a
                    // red assertion, with a misleading failure language. Checked fallback: fall
                    // back to window+1s first, then to the window boundary, and finally to now;
                    // the degradation only lets the assertion fail cleanly as "not past the
                    // window", never panicking.
                    last_touched: match Instant::now().checked_sub(
                        WEB_SESSION_DOWNLOAD_EVICTION_ACTIVE_WINDOW
                            + std::time::Duration::from_secs(1),
                    ) {
                        Some(aged_at) => aged_at,
                        None => Instant::now()
                            .checked_sub(WEB_SESSION_DOWNLOAD_EVICTION_ACTIVE_WINDOW)
                            .unwrap_or_else(Instant::now),
                    },
                },
            )
        };
        let mut inner = Inner::default();
        // Round-16 follow-up review (M1): two ready, window-expired candidates — the victim
        // must be the insertion-oldest one; a reversed-iteration (.rev(), LIFO) mutant would
        // evict the newer candidate, so this assertion is genuinely order-sensitive.
        for (download_id, entry) in [
            aged("download_old", true),
            aged("download_second", true),
            aged("download_new", false),
        ] {
            inner
                .web_session_download_order
                .push_back(download_id.clone());
            inner.web_session_downloads.insert(download_id, entry);
        }
        assert!(transfer::evict_oldest_ready_session_download(&mut inner));
        assert!(!inner.web_session_downloads.contains_key("download_old"));
        assert!(inner.web_session_downloads.contains_key("download_second"));
        assert!(inner.web_session_downloads.contains_key("download_new"));
        assert_eq!(
            inner.web_session_download_order,
            ["download_second", "download_new"]
        );

        // The second eviction takes the next-oldest ready candidate under the same FIFO semantics.
        assert!(transfer::evict_oldest_ready_session_download(&mut inner));
        assert!(!inner.web_session_downloads.contains_key("download_second"));
        assert_eq!(inner.web_session_download_order, ["download_new"]);

        // Only not-ready entries remain (engine still writing): no victim, returns false and leaves everything as-is.
        assert!(!transfer::evict_oldest_ready_session_download(&mut inner));
        assert_eq!(inner.web_session_downloads.len(), 1);
        assert_eq!(inner.web_session_download_order, ["download_new"]);

        // Round-16 review (M2): ready entries inside the window (being chunk-read) are never
        // victims — the seconds-long read gaps where last_touched is not refreshed must not
        // evict a download mid-read.
        let fresh_ready = aged("download_fresh", true);
        inner
            .web_session_downloads
            .insert("download_fresh".into(), {
                let mut entry = fresh_ready.1;
                entry.last_touched = Instant::now();
                entry
            });
        inner
            .web_session_download_order
            .push_back("download_fresh".into());
        assert!(!transfer::evict_oldest_ready_session_download(&mut inner));
        assert!(inner.web_session_downloads.contains_key("download_fresh"));
        assert_eq!(
            inner.web_session_download_order,
            ["download_new", "download_fresh"]
        );
    }

    #[test]
    fn chunk_read_refreshes_last_touched_so_active_downloads_stay_uneligible() {
        // Round-20 review MAJOR-1: the 30s active window rests on chunk reads refreshing
        // last_touched in the non-eof arm (transfer.rs's eviction-predicate comment says so
        // explicitly). Behavioral pin: seed an entry inside the window but older than this
        // read, drive one non-eof chunk read, and assert the entry survives with last_touched
        // strictly advanced (compare, don't subtract — hosts with a freshly started monotonic
        // clock must not panic). The compilable mutant deleting the refresh branch previously
        // kept the whole suite green (mutant executed) — mid-read downloads became evictable
        // under full-slot pressure, exactly the regression this window guards against; this
        // pin turns it red. The eof arm's "remove the entry, no refresh" is covered by the
        // existing read tests.
        use std::io::Write as _;
        let mut inner = Inner::default();
        let marker = Instant::now()
            .checked_sub(std::time::Duration::from_secs(5))
            .unwrap_or_else(Instant::now);
        let file = tempfile::tempfile().expect("tempfile for the download payload");
        (&file)
            .write_all(b"0123456789")
            .expect("seed the payload bytes");
        inner
            .web_session_download_order
            .push_back("download_read".to_string());
        inner.web_session_downloads.insert(
            "download_read".to_string(),
            WebSessionDownload {
                session_id: "session_a".to_string(),
                reservation_id: "reservation_read".to_string(),
                file,
                reserved_bytes: 10,
                total: 10,
                ready: true,
                last_touched: marker,
            },
        );
        let chunk = transfer::read_web_session_download_chunk(
            &mut inner,
            "download_read",
            "session_a",
            0,
            4,
        )
        .expect("the seeded entry must serve a chunk");
        assert!(
            !chunk.eof,
            "offset 0/limit 4 of a 10-byte payload is not eof"
        );
        let entry = inner
            .web_session_downloads
            .get("download_read")
            .expect("a non-eof read must keep the entry");
        assert!(
            entry.last_touched > marker,
            "a non-eof chunk read must refresh last_touched — the eviction \
             freshness window is only sound while active reads stay fresh"
        );
        assert_eq!(
            inner.web_session_download_order,
            ["download_read"],
            "a non-eof read must not disturb the eviction order"
        );
    }

    /// Wiring pin (Round-16 review MAJOR-2): begin's full-slot eviction wiring was
    /// previously invisible to tests — driving `begin_web_session_download` directly needs an
    /// AppHandle (unconstructible in unit tests), and the only eviction behavioral pins ran
    /// through the helper, so deleting the wiring inside begin (or changing `>=` to `>`, or
    /// moving the call after the capacity check into dead code) kept the whole suite green
    /// (mutant executed). Per this module's established convention that "helper-level tests
    /// cannot pin wiring points" (the same module caliber as
    /// `rpc_request_lease_gate_is_wired_inside_the_admission_lock`), this uses a source pin:
    /// pinning the if line's and the call's byte-exact shape plus the "evict first, capacity
    /// gate after" ordering.
    #[test]
    fn begin_session_download_wires_the_eviction_before_the_capacity_gate() {
        let source = include_str!("mod.rs");
        let start = source
            .find("pub fn begin_web_session_download(")
            .expect("begin_web_session_download must exist");
        let body = &source[start..];
        let body = &body[..body
            .find("\n    fn ")
            .expect("begin body must be terminated by the next fn")];
        // Round-24 review (m7): pure comment lines are filtered first — after commenting
        // out the wiring line (`// transfer::evict_…`) the needle still lived inside the
        // comment text and a bare find stayed green (same filtering caliber as round-23's
        // three sibling pins; trailing // and block-comment mentions can still shield it,
        // a disclosed residual).
        let body = crate::platform::test_support::comment_filtered(body);
        let gate = body
            .find("while ensure_web_session_download_capacity(&inner, reserved_bytes).is_err() {")
            .expect(
                "begin must gate eviction on the capacity check itself: \
                     deleting the wiring silently reverts the orphan-wedge fix \
                     to hard failure (round-25 extends the gate to both the \
                     slot and the byte dimension)",
            );
        let call = body
            .find("transfer::evict_oldest_ready_session_download(&mut inner)")
            .expect(
                "the eviction call must stay wired inside the gate (a \
                     commented-out call keeps the needle only in a comment, \
                     which the line filter strips)",
            );
        let capacity = body
            .find("ensure_web_session_download_capacity(&inner, reserved_bytes)?;")
            .expect("the capacity gate must stay in begin");
        assert!(
            gate < call && call < capacity,
            "begin must evict the oldest aged ready orphan on a failed capacity \
             check and re-check BEFORE propagating the gate error; moving the \
             call after the propagate (or deleting the loop) would strand the \
             orphan-wedge fix as dead code"
        );
        assert_eq!(
            body.matches("transfer::evict_oldest_ready_session_download(&mut inner)")
                .count(),
            1,
            "begin must keep exactly one eviction call site: the capacity-gated \
             loop covers both the slot and the byte dimension"
        );
        assert!(
            body.contains("web_session_download_evicted"),
            "each eviction must emit a diagnostic; a silent eviction leaves the \
             phone staring at a bare download-not-found error"
        );
        // Round-27 review m3: the diagnostics' "placement" had no pin — hoisting record
        // out of the loop / moving it before `if !evict` (the failure-propagation path would
        // emit one too) both compiled and stayed green. Pin the count-increment arm right
        // after the eviction call and inside the loop body, with each per-victim record after
        // `drop(inner)` (the I/O-outside-the-lock caliber).
        let if_open = body
            .find("if !transfer::evict_oldest_ready_session_download(&mut inner) {")
            .expect("the failed-eviction propagate arm must exist");
        let increment = body
            .find("evictions += 1;")
            .expect("each successful eviction must be counted");
        let unlock = body
            .find("drop(inner);")
            .expect("the manager lock must be released before the diagnostic I/O");
        let record = body
            .find("record_backend(")
            .expect("the diagnostic record call must exist");
        assert_eq!(
            body.matches("evictions += 1;").count(),
            1,
            "exactly one counted eviction per failed capacity check: a second \
             increment site would double-count evictions"
        );
        assert!(
            gate < if_open && if_open < increment && increment < unlock && unlock < record,
            "the eviction counter must live inside the gate loop after the \
             propagate arm, and the per-eviction record must fire only after \
             the manager lock is released; hoisting the record above the loop \
             (fires with no eviction) or inside the lock (file I/O under the \
             critical section) both regress the wiring silently"
        );
        // Round-28 review MINOR-2: the trigger literal in the payload had no pin — changing
        // the mod.rs-side literal kept the whole suite green while diagnostics' enum
        // allowlist silently dropped it, degrading every eviction diagnostic to empty
        // details. The enum's other-side acceptance is guarded by diagnostics.rs's
        // acceptance pin; the two sides join into end-to-end coverage here.
        assert!(
            body.contains(r#""trigger": "begin_capacity_gate""#),
            "the eviction diagnostic payload must carry the trigger literal \
             that sessions/diagnostics.rs enum-normalizes; renaming it here \
             silently degrades every eviction diagnostic to empty details"
        );
    }

    /// Wiring pin (Round-16 follow-up review M2): the same critical section where commit
    /// flips ready must refresh last_touched — the begin→commit span can exceed the
    /// eviction freshness window; flipping ready without refreshing would make a just-
    /// committed download the phone has not started chunk-reading yet immediately evictable
    /// as "ready and aged past the window". Helper-level tests cannot construct a Manager
    /// (AppHandle); per this module's convention a source pin pins the flip and the refresh's
    /// adjacency, and the mutant deleting the refresh line turns this pin red.
    #[test]
    fn commit_marks_download_ready_with_a_fresh_last_touched() {
        let source = include_str!("mod.rs");
        let start = source
            .find("fn commit_web_session_download(")
            .expect("commit_web_session_download must exist");
        let body = &source[start..];
        let body = &body[..body
            .find("\n    fn ")
            .expect("commit body must be terminated by the next fn")];
        // Round-24 review (m7): pure comment lines are filtered first, same caliber as the
        // previous pin — after commenting out the last_touched refresh line the needle must
        // not stay green by hiding in comment text.
        let body = crate::platform::test_support::comment_filtered(body);
        let ready_flip = body
            .find("download.ready = true;")
            .expect("commit must flip the download ready at the reservation it verified");
        let freshness = body.find("download.last_touched = Instant::now();").expect(
            "commit must refresh last_touched together with the ready flip: \
                     deleting it makes a just-committed download instantly evictable \
                     when its begin->commit span crosses the eviction window",
        );
        assert!(
            ready_flip < freshness && freshness - ready_flip < 200,
            "the freshness refresh must stay adjacent to the ready flip"
        );
    }

    #[test]
    fn cancelled_session_download_releases_only_the_matching_lease() {
        let mut inner = Inner::default();
        inner.web_session_download_order =
            VecDeque::from(["download_keep".to_string(), "download_cancel".to_string()]);
        for (download_id, session_id) in [
            ("download_keep", "session_keep"),
            ("download_cancel", "session_cancel"),
        ] {
            inner.web_session_downloads.insert(
                download_id.to_string(),
                WebSessionDownload {
                    session_id: session_id.to_string(),
                    reservation_id: format!("reservation_{download_id}"),
                    file: tempfile::tempfile().unwrap(),
                    reserved_bytes: 1,
                    total: 1,
                    ready: true,
                    last_touched: Instant::now(),
                },
            );
        }

        assert!(take_web_session_download(&mut inner, "download_cancel", "session_other").is_err());
        assert!(inner.web_session_downloads.contains_key("download_cancel"));

        let removed =
            take_web_session_download(&mut inner, "download_cancel", "session_cancel").unwrap();
        assert!(removed.is_some());
        assert!(!inner.web_session_downloads.contains_key("download_cancel"));
        assert!(inner.web_session_downloads.contains_key("download_keep"));
        assert_eq!(inner.web_session_download_order, ["download_keep"]);
        assert!(
            take_web_session_download(&mut inner, "download_cancel", "session_cancel")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn code_session_event_filter_is_inert_without_predicate() {
        // With no predicate injected (startup-early), nothing is filtered — behavior matches pre-injection.
        let payload = json!({ "session_id": "code-sess-1" });
        assert!(!should_filter_code_session_event(None, &payload));
    }

    #[test]
    fn code_session_events_are_filtered_for_each_field_name_variant() {
        // Field-name contract (formerly event_session_id_reads_each_event_family_field_name):
        // chat:* / artifact:disk use session_id, session:* uses id,
        // scheduled_task:run_updated uses sessionId.
        assert_eq!(
            event_session_id(&json!({ "session_id": "s-chat" })),
            Some("s-chat")
        );
        assert_eq!(
            event_session_id(&json!({ "id": "s-session" })),
            Some("s-session")
        );
        assert_eq!(
            event_session_id(&json!({ "sessionId": "s-task" })),
            Some("s-task")
        );

        let predicate: CodeSessionPredicate =
            Arc::new(|session_id: &str| session_id.starts_with("code-"));
        for payload in [
            json!({ "session_id": "code-sess-1" }),
            json!({ "id": "code-sess-1" }),
            json!({ "sessionId": "code-sess-1" }),
        ] {
            assert!(
                should_filter_code_session_event(Some(&predicate), &payload),
                "code session event must be filtered: {payload}"
            );
        }

        // Under an always-true predicate the filter is equivalent to event_session_id's
        // existence check (formerly
        // event_session_id_ignores_missing_or_non_string_fields and
        // code_session_filter_only_fires_on_a_session_id_field): global events without a
        // session id, or whose id is not a string, must never be collateral damage.
        let always: CodeSessionPredicate = Arc::new(|_: &str| true);
        assert_eq!(
            event_session_id(&json!({ "kind": "session:list_changed" })),
            None
        );
        assert_eq!(event_session_id(&json!({ "session_id": 42 })), None);
        assert!(!should_filter_code_session_event(
            Some(&always),
            &json!({ "kind": "session:list_changed" })
        ));
        assert!(!should_filter_code_session_event(
            Some(&always),
            &json!({ "id": 42 })
        ));
    }

    #[test]
    fn plain_session_events_survive_the_code_session_filter() {
        // Ordinary session events must be unaffected: the predicate returns true only for real code-session ids.
        let predicate: CodeSessionPredicate =
            Arc::new(|session_id: &str| session_id.starts_with("code-"));
        for payload in [
            json!({ "session_id": "plain-sess-1" }),
            json!({ "id": "plain-sess-1" }),
            json!({ "sessionId": "plain-sess-1" }),
        ] {
            assert!(
                !should_filter_code_session_event(Some(&predicate), &payload),
                "plain session event must not be filtered: {payload}"
            );
        }
    }

    fn test_active_endpoint(lease_id: Option<&str>) -> ActiveEndpoint {
        let (sender, _receiver) =
            relay_client::outbound_channel(8, 64 * 1024, relay_client::MAX_RELAY_FRAME_BYTES);
        ActiveEndpoint {
            config: test_web_config(TEST_ENDPOINT_ID),
            url: "http://127.0.0.1:8787/pinvou3/remote".into(),
            status: WebAccessStatusKind::WebClientConnected,
            web_client_connected: true,
            client_ready: true,
            lease_id: lease_id.map(ToOwned::to_owned),
            last_lease_id: lease_id.map(ToOwned::to_owned),
            fresh_baseline_seq: None,
            last_error: None,
            last_error_code: None,
            sender,
        }
    }

    #[test]
    fn forwarded_rpc_requests_must_target_the_active_lease() {
        let mut inner = Inner::default();
        // No active endpoint: no filtering (the main admission path has no replyable endpoint anyway).
        assert!(rpc_request_targets_active_lease(&inner, TEST_LEASE_ID));

        inner.endpoint = Some(test_active_endpoint(Some(TEST_LEASE_ID)));
        assert!(rpc_request_targets_active_lease(&inner, TEST_LEASE_ID));
        assert!(
            !rpc_request_targets_active_lease(&inner, "lease_111111111111111111111111"),
            "a rpc_request carrying a replaced lease is a stale takeover-race frame \
             and must be dropped before touching the idempotency cache"
        );

        // No active lease (client already disconnected) keeps the original behavior: no filtering here.
        inner.endpoint = Some(test_active_endpoint(None));
        assert!(rpc_request_targets_active_lease(&inner, TEST_LEASE_ID));
    }

    /// Wiring pin (source contract, same shape as the revoke deadlock pin): the lease
    /// validation must stay inside the admission lock's scope and precede the cache lookup —
    /// helper-level tests cannot pin "the wiring point was deleted" (round-4 review, verified
    /// live: deleting the call kept every helper test green). Deleting the validation, or
    /// moving it out of `inner.lock()` and querying the cache afterwards, both turn this pin
    /// red.
    #[test]
    fn rpc_request_lease_gate_is_wired_inside_the_admission_lock() {
        let source = include_str!("mod.rs");
        let handle_start = source
            .find("fn handle_rpc_request(&self, value: &Value)")
            .expect("handle_rpc_request must exist");
        let body = &source[handle_start..];
        let body = &body[..body
            .find("\n    fn ")
            .expect("handle_rpc_request body must be terminated by the next fn")];
        let lock = body
            .find("let mut inner = self.inner.lock()")
            .expect("handle_rpc_request must take the admission lock");
        let gate = body
            .find("rpc_request_targets_active_lease(&inner, &lease_id)")
            .expect("handle_rpc_request must consult the lease gate inside the admission lock");
        let cache = body
            .find("match inner.rpc_cache")
            .expect("handle_rpc_request must consult the idempotency cache");
        assert!(
            lock < gate && gate < cache,
            "the lease gate must run inside the admission lock and before the cache match \
             (lock at {lock}, gate at {gate}, cache at {cache})"
        );
    }

    /// Wiring pin (Round-5 review M2): the settle funnel's safety contract (transient
    /// refusals never enter the Complete cache, tombstone conflicts never cached) only takes
    /// effect at the call sites in `handle_rpc_request` — reverting the three call sites to
    /// unconditional caching/unconditional settling kept every helper-level test green (the
    /// module's own comments attest it). Round-6 review minor: tombstone and precheck are
    /// asserted by existence, and the two admission arms each have one call site, asserted by
    /// **count** — rolling back either (count dropping to 1) or adding an arm bypassing the
    /// funnel (count exceeding 2) turns red; a negative pin forbids any direct
    /// `settle_complete` outside the funnel from entering the function body.
    #[test]
    fn rpc_cache_settle_funnels_are_wired_in_handle_rpc_request() {
        let source = include_str!("mod.rs");
        let handle_start = source
            .find("fn handle_rpc_request(&self, value: &Value)")
            .expect("handle_rpc_request must exist");
        let body = &source[handle_start..];
        let body = &body[..body
            .find("\n    fn ")
            .expect("handle_rpc_request body must be terminated by the next fn")];
        assert!(
            body.contains("Self::settle_tombstone_outcome("),
            "the tombstone branch must route its cache decision through settle_tombstone_outcome"
        );
        assert_eq!(
            body.matches("Self::settle_admission_rejection(").count(),
            2,
            "both admission branches (reconstructed tombstone and fresh durable) must route \
             their cache decision through settle_admission_rejection"
        );
        assert!(
            body.contains("validate_web_rpc_scope("),
            "the preflight must consult the web scope gate (KB paths / prompt references / session scope)"
        );
        assert!(
            !body.contains("Self::settle_complete("),
            "no arm inside handle_rpc_request may bypass the settle funnels with a direct \
             Complete-cache write"
        );
        assert_eq!(
            body.matches("rpc_cache.insert").count(),
            1,
            "the only direct rpc_cache.insert inside handle_rpc_request is the TTL sweep's \
             placeholder write; an arm that keeps its settle call and adds an inline insert \
             on the non-cacheable path would reintroduce replay poisoning while every \
             existing pin stays green (Round-8 复审)"
        );
    }

    /// Wiring pin (Round-6 review M5): the `web_rejoining` flag's safety semantics (a rejoin
    /// keeps staged transfers; only a live takeover clears them) only takes effect at the
    /// frame-path call sites — helper-level tests drive
    /// `apply_web_lease_change(inner, true)` directly and cannot see the wiring. Changing
    /// either call site back to `false` (or deleting the parse, letting the default false
    /// apply) would verbatim restore round-2's regression where staged state got wiped by the
    /// watchdog every ~2.5 minutes while the whole suite stayed green. Both arrival paths
    /// (`handle_registered` re-registration forwarding and `handle_web_connected` web join)
    /// must parse and pass the flag through.
    #[test]
    fn web_rejoining_flag_is_wired_into_both_lease_change_call_sites() {
        let source = include_str!("mod.rs");
        for fn_name in [
            "fn handle_registered(&self, value: &Value)",
            "fn handle_web_connected(&self, value: &Value)",
        ] {
            let start = source
                .find(fn_name)
                .unwrap_or_else(|| panic!("{fn_name} must exist"));
            let body = &source[start..];
            let body = &body[..body
                .find("\n    fn ")
                .unwrap_or_else(|| panic!("{fn_name} body must be terminated by the next fn"))];
            // Round-28 review: sibling wiring pins gained comment filtering across
            // round-23/24 but this pin missed it — with the real parse+call lines commented
            // out, the needle survived inside the comment text and the suite stayed green.
            // The same caliber is completed here (trailing comments and block comments can
            // still shield it — a disclosed residual).
            let body = crate::platform::test_support::comment_filtered(body);
            assert!(
                body.contains("get(\"web_rejoining\")"),
                "{fn_name} must parse the web_rejoining flag from the frame"
            );
            assert!(
                body.contains("apply_web_lease_change(&mut inner, web_rejoining);"),
                "{fn_name} must pass the parsed flag (not a constant) into apply_web_lease_change"
            );
        }
    }

    /// Wiring pin (Round-6 review M6): the disconnect half of the `last_error_code` field
    /// contract — an error write carrying no code must clear the old recovery code (or the
    /// old trilingual recovery hint would sit on top of new error text). The connected arm
    /// and the disconnect arm each clear codes; deleting either arm's clearing line turns
    /// red.
    #[test]
    fn handle_connection_clears_stale_error_code_on_both_branches() {
        let source = include_str!("mod.rs");
        let start = source
            .find("fn handle_connection(&self, endpoint_id: &str, connected: bool, error: Option<String>)")
            .expect("handle_connection must exist");
        let body = &source[start..];
        let body = &body[..body
            .find("\n    fn ")
            .expect("handle_connection body must be terminated by the next fn")];
        assert_eq!(
            body.matches("last_error_code = None").count(),
            2,
            "both the connected and the disconnected branch must clear the stale code"
        );
    }

    /// Wiring pin (the endpoint_revoked half of Round-5 review M2): the relay's
    /// endpoint_revoked error frame must route into handle_revoked's full cleanup — deleting
    /// that arm would make the local config retry forever on a registration that can never
    /// succeed, while terminal_relay_message's classification is pinned on the relay_client
    /// side and cannot see this manager-side routing. Deleting any key call inside the arm
    /// turns red.
    #[test]
    fn relay_error_frames_route_endpoint_revoked_to_the_revoke_cleanup() {
        let source = include_str!("mod.rs");
        let start = source
            .find("fn handle_relay_message(&self, value: Value)")
            .expect("handle_relay_message must exist");
        let tail = &source[start..];
        let body = &tail[..tail
            .find("\n    fn ")
            .expect("handle_relay_message body must be terminated by the next fn")];
        let arm = body
            .find("if error_code == \"endpoint_revoked\"")
            .expect("the endpoint_revoked error arm must exist");
        let arm_tail = &body[arm..];
        let arm_body = &arm_tail[..arm_tail
            .find("\n                    return;")
            .expect("the endpoint_revoked arm must be closed by its early return")];
        assert!(
            arm_body.contains("handle_revoked(&current)"),
            "the endpoint_revoked arm must run the full revoke cleanup for the current endpoint"
        );
    }

    #[test]
    fn web_rejoins_keep_staged_transfers_but_takeovers_do_not() {
        let staging = std::env::temp_dir().join(format!(
            "pinvou-webrejoin-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(8)
        ));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("notes.txt"), b"abc").unwrap();

        let mut inner = Inner::default();
        inner.endpoint = Some(test_active_endpoint(Some(TEST_LEASE_ID)));
        inner.subscriptions.insert("session_a".into());
        let cached = cached_upload_for_test(&staging, None);
        inner.web_attachment_order.push_back("attachment_a".into());
        inner.web_attachments.insert("attachment_a".into(), cached);
        inner.web_attachment_bytes = 64;
        // Round-15 review MA4: the rejoin arm must seed all three — with only attachments
        // seeded, mutants clearing in-flight uploads (or session downloads) in the keep arm
        // kept the whole suite green (mutant executed), half-reviving Round-2's
        // watchdog-swallows-uploads regression.
        inner.web_attachment_uploads.insert(
            "upload_a".to_string(),
            WebAttachmentUpload {
                file_name: "notes.txt".into(),
                total: 3,
                data: b"abc".to_vec(),
                last_touched: Instant::now(),
            },
        );
        // Round-29 review MINOR-4: the upload-order twin of the download-order
        // seam — the upload pop loop removes whatever entry the order's front
        // id names, so a stale id costs one live upload (over-eviction). Seed
        // the queue and pin its consistency with the map on both arms;
        // deleting `web_attachment_upload_order.clear()` in the takeover wipe
        // used to keep the whole suite green.
        inner
            .web_attachment_upload_order
            .push_back("upload_a".to_string());
        inner
            .web_session_download_order
            .push_back("download_a".to_string());
        inner.web_session_downloads.insert(
            "download_a".to_string(),
            WebSessionDownload {
                session_id: "session_a".into(),
                reservation_id: "reservation_a".into(),
                file: tempfile::tempfile().unwrap(),
                reserved_bytes: 1,
                total: 1,
                ready: false,
                last_touched: Instant::now(),
            },
        );

        // Rejoin (same token holder, previous socket already gone): the
        // staged transfer survives an idle-watchdog reconnect; only the
        // lease-bound subscriptions rebuild.
        apply_web_lease_change(&mut inner, true);
        assert!(
            inner.subscriptions.is_empty(),
            "a changed lease must always rebuild subscriptions"
        );
        assert!(
            inner.web_attachments.contains_key("attachment_a"),
            "a rejoin must keep staged attachments"
        );
        assert!(
            inner.web_attachment_uploads.contains_key("upload_a"),
            "a rejoin must keep in-flight uploads"
        );
        assert!(
            inner.web_session_downloads.contains_key("download_a"),
            "a rejoin must keep session downloads"
        );
        // Round-16 review (M2 test seam): the order queue and the map must move together. A
        // stale id itself would not be evicted (the victim criteria require the map entry to
        // be present; a missing one is skipped) — the real harm of missing the queue cleanup
        // is dead weight: the stale id occupies a slot until the next prune (which runs only
        // inside begin/commit) retains it away by the map; pin the consistency of both
        // cleanups.
        assert!(
            inner
                .web_session_download_order
                .contains(&"download_a".to_string()),
            "a rejoin must keep the download eviction order consistent with the map"
        );
        assert!(
            inner
                .web_attachment_upload_order
                .contains(&"upload_a".to_string()),
            "a rejoin must keep the upload order consistent with the map"
        );

        // Takeover (fresh client replaces a live one): the wipe must stay so
        // the replacing client cannot touch the previous lease's transfers.
        apply_web_lease_change(&mut inner, false);
        assert!(inner.web_attachments.is_empty());
        assert!(inner.web_attachment_uploads.is_empty());
        assert!(inner.web_session_downloads.is_empty());
        assert!(
            inner.web_session_download_order.is_empty(),
            "a takeover wipe must clear the eviction order alongside the map"
        );
        assert!(
            inner.web_attachment_upload_order.is_empty(),
            "a takeover wipe must clear the upload order alongside the map; a \
             stale id makes the upload pop loop remove a live upload"
        );

        std::fs::remove_dir_all(&staging).ok();
    }

    #[test]
    fn revoking_host_workspace_also_invalidates_minted_grants() {
        let staging = std::env::temp_dir().join(format!(
            "pinvou-revoke-grant-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(8)
        ));
        std::fs::create_dir_all(&staging).unwrap();

        let mut inner = Inner::default();
        inner.endpoint = Some(test_active_endpoint(Some(TEST_LEASE_ID)));
        inner.endpoint.as_mut().unwrap().config.allow_host_workspace = true;
        let identity = WebWorkspaceIdentity::capture(&staging).unwrap();
        inner.web_workspace_grants.issue(
            "workspace_test_handle".into(),
            TEST_ENDPOINT_ID.into(),
            staging.clone(),
            identity,
            Instant::now(),
        );
        assert!(inner.web_workspace_grants.contains("workspace_test_handle"));

        revoke_host_workspace_state(&mut inner);
        assert!(
            !inner.endpoint.as_ref().unwrap().config.allow_host_workspace,
            "the live endpoint must reflect the revoked consent"
        );
        assert!(
            !inner.web_workspace_grants.contains("workspace_test_handle"),
            "an already-minted grant must not stay redeemable after revoke"
        );
        std::fs::remove_dir_all(staging).ok();
    }

    #[test]
    fn revoke_host_workspace_never_holds_inner_across_emit_status() {
        // The depth must be shallower than where the inner guard is taken — i.e. the guard
        // has already left scope. Round-15 review MA1's persist-failure arm takes/drops the
        // guard in deeper nesting before emitting, which the naive "first emit depth < first
        // lock depth" criterion cannot see (mutant executed: false positive). Replaced with
        // exact span judgment: every emit_status site must not fall before the brace-block
        // closure of any self.inner.lock() site.
        //
        // Round-24 review (m) disclosed residual: the spans come from counting bare { / },
        // comments and strings included — a comment or string literal containing an unbalanced
        // `}` between a lock site and its block closure would truncate the span early, and a
        // future "emit inside the guard span" regression could be judged outside it (silently
        // green; same caliber as the round-23 sibling pins). Under today's text all five lock
        // spans close correctly; the green baseline was confirmed by manual review.
        let source = include_str!("mod.rs");
        let body = source
            .split("pub fn revoke_host_workspace")
            .nth(1)
            .expect("revoke_host_workspace must exist");
        let body = &body[..body.find("\n    pub fn ").unwrap_or(body.len())];
        let open = body.find('{').expect("function body braces");
        let depth_at = |pos: usize| {
            body[open..pos]
                .chars()
                .filter(|c| *c == '{')
                .count()
                .checked_sub(body[open..pos].chars().filter(|c| *c == '}').count())
                .expect("brace depth cannot go negative in valid Rust")
        };
        let mut lock_spans: Vec<(usize, usize)> = Vec::new();
        let mut search = open;
        while let Some(rel) = body[search..].find("self.inner.lock()") {
            let lock_pos = search + rel;
            let lock_depth = depth_at(lock_pos);
            let mut close = lock_pos;
            while close < body.len() && depth_at(close) >= lock_depth {
                close += body[close..].chars().next().expect("valid utf8").len_utf8();
            }
            lock_spans.push((lock_pos, close));
            search = lock_pos + "self.inner.lock()".len();
        }
        assert!(
            !lock_spans.is_empty(),
            "revoke must take the inner lock for the state invalidation"
        );
        let mut emit_sites: Vec<usize> = Vec::new();
        let mut search = open;
        while let Some(rel) = body[search..].find("self.emit_status()") {
            emit_sites.push(search + rel);
            search += rel + "self.emit_status()".len();
        }
        assert!(!emit_sites.is_empty(), "revoke must refresh the status");
        for emit in emit_sites {
            for (lock_pos, lock_end) in &lock_spans {
                assert!(
                    emit <= *lock_pos || emit >= *lock_end,
                    "emit_status at byte {emit} runs inside the live inner-guard span \
                     ({lock_pos}..{lock_end}); holding the guard across it re-locks the \
                     same non-reentrant mutex and deadlocks the revoke path",
                );
            }
        }
    }

    /// Wiring pin (Round-10 review M2): revocation's safety semantics — "invalidate minted
    /// workspace grants in sync + persist the consent flag" — only takes effect at the
    /// command-path call sites; `revoking_host_workspace_also_invalidates_minted_grants`
    /// drives the free helper directly and cannot see the wiring. Deleting
    /// `revoke_host_workspace_state(&mut inner)` or `persist_config(&config)?` would revert
    /// revocation to the pre-fix behavior "memory-only change; minted grants stay redeemable
    /// for their 30-minute TTL" while the whole suite stays green (mutant executed: all 20
    /// revoke-matching tests green).
    #[test]
    fn revoke_host_workspace_wires_grant_invalidation_and_latches_the_flag() {
        let source = include_str!("mod.rs");
        let body = source
            .split("pub fn revoke_host_workspace(")
            .nth(1)
            .expect("revoke_host_workspace must exist");
        let body = &body[..body.find("\n    pub fn ").unwrap_or(body.len())];
        let body = comment_filtered(body);
        assert!(
            body.contains("persist_config(&config)"),
            "revoke must latch the consent flag into the persisted config"
        );
        assert!(
            body.contains("revoke_host_workspace_state(&mut inner);"),
            "revoke must invalidate already-minted workspace grants on the live state"
        );
        assert!(
            body.contains("self.emit_status();"),
            "revoke must push the refreshed status so the modal reflects the revoked row"
        );
    }

    /// Wiring pin (Round-15 review MA1): a persist failure must re-latch the in-memory flag
    /// to the disk side before returning. Reverting to `persist_config(&config)?`'s `?`
    /// propagation (or deleting the re-latch assignment in the error arm) would leave the
    /// revocation in the forked state "memory revoked / disk still authorized" — the next
    /// start/enable rebuilds the authorization flag from disk with no consent gesture, while
    /// the whole suite stays green (mutant executed: restoring the `?` propagation turned this
    /// pin red with every other revoke pin green).
    #[test]
    fn revoke_persist_failure_re_latches_memory_to_the_disk_side() {
        let source = include_str!("mod.rs");
        let body = source
            .split("pub fn revoke_host_workspace(")
            .nth(1)
            .expect("revoke_host_workspace must exist");
        let body = &body[..body.find("\n    pub fn ").unwrap_or(body.len())];
        let body = comment_filtered(body);
        assert!(
            !body.contains("persist_config(&config)?"),
            "revoke must not propagate a persist failure with `?` while the \
             in-memory flag is already revoked: memory would stay revoked while \
             the disk latch stays authorized, so the next benign start/enable \
             resurrects folder consent without a gesture"
        );
        assert!(
            body.contains("if let Err(error) = persist_config(&config)"),
            "the persist failure must be handled inside revoke, not propagated"
        );
        assert!(
            body.contains("folder access remains enabled"),
            "the error must tell the user the revocation did not take effect"
        );
        // Round-16 review (M1c): the failure arm's status re-emit had no pin — deleting just
        // this emit_status kept the whole suite green (the B1 pin only requires some emit in
        // the function, and the function tail already has one). Assert that the emit falls
        // inside the failure-arm window (between the persist if-let and the error copy);
        // deleting the in-arm emit turns red.
        let failure_arm_start = body
            .find("if let Err(error) = persist_config(&config)")
            .expect("the persist failure arm must exist");
        let failure_arm_end = body
            .find("folder access remains enabled")
            .expect("the actionable failure message must exist");
        let failure_arm = &body[failure_arm_start..failure_arm_end];
        // Round-24 review MAJOR (mutant confirmed): the re-latch assertion was previously a
        // function-level contains — round-22 added a byte-identical re-latch line inside the
        // validate_config failure arm (inside the `Err(error) => {` window, outside this one),
        // and that line satisfied the function-level assertion: deleting this arm's re-latch
        // block (still compiling) kept this pin green (mutation executed this round, 1 passed /
        // 0 failed). Now an arm-bounded window assertion — deleting this arm's re-latch alone
        // turns red, and the validate arm's identical line can no longer backstop it.
        assert!(
            failure_arm.contains("endpoint.config.allow_host_workspace = true;"),
            "the persist-failure arm itself must re-latch the in-memory flag \
             to match the still-authorized disk state; the validate arm's \
             identical re-latch line must not satisfy this window (a \
             function-wide contains went blind when round-22 added it)"
        );
        assert!(
            failure_arm.contains("self.emit_status();"),
            "the failure arm must re-emit status so the UI shows the truthful \
             still-authorized consent state instead of the optimistic revoked row"
        );
        // Round-20 review (S04): the copy assertion only proves the text exists, not that it
        // is the returned Err — the "print the copy then allow success" mutant (misreporting a
        // failed persist as success) kept all five assertions green. Pin that `return Err(`
        // falls inside the failure-arm window (between the if-let and the copy; no
        // statement-level return↔copy pairing — the error copy itself contains a semicolon,
        // "persisted; folder", so a plain-text semicolon scan would false-red).
        assert!(
            failure_arm.contains("return Err("),
            "the actionable failure message must be returned as an Err from \
             inside the persist-failure arm; a log-and-continue mutation \
             would misreport a failed persist as success"
        );
        // Round-28 review MINOR-8a: the ordering had no pin — swapping the re-latch block and
        // emit_status wholesale compiles and keeps every member assertion above green (mutant
        // executed and confirmed), but the emitted status payload would carry the revoked flag
        // while memory/disk are already re-latched, flashing a false unauthorized row in the
        // UI. Pin the re-latch → emit → return order (the same caliber as the validate arm's
        // relatch<emit ordering).
        let relatch_at = failure_arm
            .find("endpoint.config.allow_host_workspace = true;")
            .expect("the re-latch line must exist");
        let emit_at = failure_arm
            .find("self.emit_status();")
            .expect("the failure-arm status re-emit must exist");
        let return_at = failure_arm
            .find("return Err(")
            .expect("the returned Err must exist");
        assert!(
            relatch_at < emit_at && emit_at < return_at,
            "order must be re-latch -> status re-emit -> returned Err: \
             emitting before the re-latch publishes a still-revoked flag \
             while memory and disk are already re-paired (order-swap \
             mutation executed green before this tooth)"
        );
    }

    /// Round-20 review MINOR pin: the strict-read failure arm (config corrupt/unreadable)
    /// previously returned via `?` right after the in-memory invalidation had really flipped
    /// the flag/cleared grants, with no status refresh — the UI stayed on the authorized
    /// display until the next unrelated status push (Round-19 NIT already added the same emit
    /// for the config-absent malformed state; this pin brings the corrupt state under the
    /// same discipline). Source pin: the failure-arm window must contain both the error return
    /// and emit_status; deleting the emit turns red. Round-21 review correction: the round-20
    /// window spanned both match arms, and the emit assertion was backstopped by the Ok(None)
    /// arm's own emit — deleting only the failure arm's emit kept the whole suite green (the
    /// split-confirmation bypass mutant); corrected to an arm-bounded window (the emit falls
    /// between `Err(error) => {` and `return Err(error)`, order verified too), after which a
    /// single deletion turns red.
    #[test]
    fn revoke_strict_load_failure_re_emits_status_before_returning() {
        let source = include_str!("mod.rs");
        let body = source
            .split("pub fn revoke_host_workspace(")
            .nth(1)
            .expect("revoke_host_workspace must exist");
        let body = &body[..body.find("\n    pub fn ").unwrap_or(body.len())];
        let body = comment_filtered(body);
        let load_start = body
            .find("let mut config = match load_config()")
            .expect("the strict config readback must exist");
        let load_end = body
            .find("validate_config(&config)")
            .expect("the readback match must be followed by validation");
        let match_body = &body[load_start..load_end];
        assert!(
            match_body.contains("Err(error) => {"),
            "the strict readback must still return its error: stop/revoke is a \
             recovery path and a corrupt config must not wedge it"
        );
        let err_arm_start = match_body
            .find("Err(error) => {")
            .expect("the strict-load failure arm must exist");
        let err_arm = &match_body[err_arm_start..];
        let return_pos = err_arm
            .find("return Err(error)")
            .expect("the strict-load failure arm must return the error");
        let err_body = &err_arm[..return_pos];
        assert!(
            err_body.contains("self.emit_status();"),
            "the strict-load failure arm must re-emit status BEFORE returning: \
             the invalidation above already flipped the flag and destroyed \
             grants, so the UI would hold a stale authorized row until an \
             unrelated push"
        );
    }

    /// Round-22 review MINOR source pin: the validate_config failure arm follows the same
    /// discipline as the persist-failure arm — previously a bare `?` pass-through with no
    /// status refresh, no in-memory re-latch, and error text never mentioning folder consent;
    /// the UI stayed on the authorized row (the same family as the strict-load/missing-config
    /// arms). Pin the arm opener (a bare `?` revert turns red), the re-latch needle (deletion
    /// turns red), and the emit-before-constructing-the-error order (block deletion/order swap
    /// turn red). Round-23 review adds the emit's unconditional position: the mutant moving it
    /// inside the conditional re-latch block previously stayed green.
    #[test]
    fn revoke_validation_failure_re_latches_and_emits() {
        let source = include_str!("mod.rs");
        let body = source
            .split("pub fn revoke_host_workspace(")
            .nth(1)
            .expect("revoke_host_workspace must exist");
        let body = &body[..body.find("\n    pub fn ").unwrap_or(body.len())];
        let body = comment_filtered(body);
        let arm_start = body
            .find("validate_config(&config).map_err(|error| {")
            .expect("the validation failure must be handled at the arm, not propagated with a bare `?` (the memory side was already invalidated above; the UI must not hold a stale authorized row)");
        let arm = &body[arm_start..];
        let arm_end = arm.find("})?;").expect("the map_err arm must close");
        let arm_body = &arm[..arm_end];
        let relatch_pos = arm_body
            .find("endpoint.config.allow_host_workspace = true;")
            .expect("the validation failure arm must re-latch the in-memory                      consent flag to the disk side when the disk latch is                      still authorized (same resurrection face as the                      persist-failure arm)");
        let emit_pos = arm_body.find("self.emit_status();").expect(
            "the validation failure arm must refresh status before                      returning",
        );
        assert!(
            relatch_pos < emit_pos,
            "the re-latch must precede the status refresh so the emitted              payload shows the truthful still-authorized state"
        );
        // Round-27 review m1: the re-latch's "inside the conditional" position had no pin —
        // the mutant hoisting the re-latch out of the `if remains_enabled {` block (making it
        // unconditional) compiled green and flipped the corrupt-config-but-disk-unauthorized
        // shape into fail-open (memory re-latched to authorized while the disk flag is false,
        // keeping in-memory host-folder listing alive until restart — the exact resurrection
        // surface this pin family guards). Pin method: the re-latch must sit after the
        // conditional opener, and the braces between opener and re-latch must be unbalanced
        // (the conditional block still open) — hoisting/postponing mutants both mismatch here
        // and turn red.
        let if_pos = arm_body
            .find("if remains_enabled {")
            .expect("the conditional re-latch block must exist");
        let before_relatch = &arm_body[if_pos..relatch_pos];
        assert!(
            relatch_pos > if_pos
                && before_relatch.matches('{').count() > before_relatch.matches('}').count(),
            "the re-latch must sit INSIDE the conditional remains_enabled \
             block: it must only fire when the disk latch is still \
             authorized; hoisting it out (unconditional re-latch) flips the \
             invalid-config flag-false shape fail-open"
        );
        // Round-23 review MINOR: the emit's unconditional position had no pin — the mutant
        // moving the emit inside the `if remains_enabled {` block stayed green, and the
        // corrupt-config-but-disk-unauthorized shape would silently lose its status refresh
        // (the persist-failure arm's existing same-shaped gap). The emit must sit outside the
        // conditional re-latch block: the braces from the block opener to the emit must be
        // balanced, and moving it into the block mismatches and turns red. (The slice is pure
        // code today; the brace-counting caliber matches the exact lock-span pin — braces
        // inside strings/comments are a drift-fragile surface, a disclosed residual.)
        let if_pos = arm_body
            .find("if remains_enabled {")
            .expect("the conditional re-latch block must exist");
        let between = &arm_body[if_pos..emit_pos];
        assert_eq!(
            between.matches('{').count(),
            between.matches('}').count(),
            "the status refresh must sit outside the conditional re-latch \
             block: both shapes (disk still authorized / config invalid) \
             must refresh the emitted state"
        );
        assert_eq!(
            arm_body.matches("self.emit_status();").count(),
            1,
            "the arm must keep exactly one unconditional status refresh"
        );
        assert!(
            arm_body.contains("did not take effect; folder consent stays enabled on disk"),
            "the error must say the revocation did not take effect when the              disk latch is still authorized"
        );
    }

    /// Round-19 review NIT pin (round-21 review addition: this arm previously had no pin at
    /// all — deleting the whole block kept the suite green): the malformed state where config
    /// is missing but an in-memory endpoint remains — the invalidation block really did flip
    /// the flag/clear grants, so one conditional status refresh is added, keeping the UI from
    /// staying stuck on the authorized display. Source pin: within the Ok(None) arm window,
    /// `if endpoint_present {` and its in-block emit_status must coexist; deleting the
    /// conditional block (or just the emit) turns red.
    #[test]
    fn revoke_missing_config_arm_emits_status_when_endpoint_present() {
        let source = include_str!("mod.rs");
        let body = source
            .split("pub fn revoke_host_workspace(")
            .nth(1)
            .expect("revoke_host_workspace must exist");
        let body = &body[..body.find("\n    pub fn ").unwrap_or(body.len())];
        let body = comment_filtered(body);
        let load_start = body
            .find("let mut config = match load_config()")
            .expect("the strict config readback must exist");
        let load_end = body
            .find("validate_config(&config)")
            .expect("the readback match must be followed by validation");
        let match_body = &body[load_start..load_end];
        let ok_arm_start = match_body
            .find("Ok(None) => {")
            .expect("the config-absent arm must exist");
        let ok_arm = &match_body[ok_arm_start..];
        let return_pos = ok_arm
            .find("return Ok(WebAccessInfo")
            .expect("the config-absent arm must return the stopped payload");
        let ok_body = &ok_arm[..return_pos];
        let guard_pos = ok_body.find("if endpoint_present {").expect(
            "the config-absent arm must conditionally refresh status when \
                 an in-memory endpoint is still alive: the invalidation above \
                 already flipped the flag, so the UI would hold a stale \
                 authorized row until an unrelated push",
        );
        let guard_tail = &ok_body[guard_pos..];
        let guard_end = guard_tail
            .find("}")
            .expect("the conditional refresh block must close");
        assert!(
            guard_tail[..guard_end].contains("self.emit_status();"),
            "the conditional refresh must emit status inside the \
             endpoint-present block"
        );
    }

    /// Wiring pin (Round-15 review minor: full-queue wedging of recovery paths). stop/refresh
    /// must degrade a full pending-revocation queue to log-and-continue by the shared prefix;
    /// reverting to `queue_pending_revocation(…)?`'s `?` propagation revives both wedge
    /// classes at once: stop cannot stop, and refresh fails permanently from the 33th rotation
    /// on (mutant executed). The same-key-different-secret conflict branch stays as-is (the
    /// round-14 fail-closed boundary).
    #[test]
    fn recovery_paths_degrade_a_full_revocation_queue_instead_of_wedging() {
        let source = include_str!("mod.rs");
        for (fn_name, label) in [("pub fn stop_current(", "stop"), ("fn refresh(", "refresh")] {
            let body = source
                .split(fn_name)
                .nth(1)
                .unwrap_or_else(|| panic!("{fn_name} must exist"));
            let end = ["\n    fn ", "\n    pub fn ", "\n    #[test]"]
                .iter()
                .filter_map(|marker| body.find(marker))
                .min()
                .unwrap_or(body.len());
            let body = comment_filtered(&body[..end]);
            assert!(
                !body.contains("queue_pending_revocation(config)?")
                    && !body.contains("queue_pending_revocation(previous)?"),
                "{label} must not propagate a full pending-revocation queue with `?`: a full queue would wedge the recovery path itself",
            );
            assert!(
                body.contains("PENDING_REVOCATION_QUEUE_FULL_PREFIX"),
                "{label} must recognize the shared queue-full prefix and degrade to a log",
            );
            // Round-18 review: the guard's and conflict arm's polarity was previously covered
            // only by weak "references the constant + no ?" assertions — widening the guard into
            // rewrites like `starts_with(PREFIX) || !error.is_empty()` would silently flip the
            // round-14-recorded same-key-different-secret fail-closed boundary into fail-open
            // with every existing assertion green. Pin the guard and the catch-all conflict arm
            // by byte-exact shape. (Round-19 review: the guard needle must include the arm
            // opener `=> {` — a bare guard needle is superstring-blind, and the disjunction
            // widening `|| !error.is_empty()` keeps the old needle verbatim; with the opener
            // included, any widening inserts text before `=> {` and the needle mismatches.)
            assert!(
                body.contains(
                    "Err(error) if error.starts_with(PENDING_REVOCATION_QUEUE_FULL_PREFIX) => {"
                ),
                "{label} must gate the degrade on the exact shared queue-full prefix arm; \
                 widening the guard (e.g. a disjunctive `||` clause) silently flips the \
                 documented conflicting-secret fail-closed boundary to fail-open",
            );
            assert!(
                body.contains("Err(error) => return Err(error),"),
                "{label} must keep the catch-all conflict arm fail-closed; degrading it \
                 drops revocation intents for conflicting credentials without any \
                 durable record",
            );
            // Round-22 review (m5): the two contains checks ignore arm order — swapping the
            // guard arm and the catch-all arm still compiles, the queue-full error is rerouted
            // to return through the fail-closed conflict arm, stop/refresh wedging revives, and
            // both needles are present verbatim. Pin by position that the guard precedes the
            // catch-all (the same style as this test's existing entry/tolerate order
            // assertions).
            let guard_at = body
                .find("Err(error) if error.starts_with(PENDING_REVOCATION_QUEUE_FULL_PREFIX) => {")
                .expect("guard needle pinned above");
            let catchall_at = body
                .find("Err(error) => return Err(error),")
                .expect("catch-all needle pinned above");
            assert!(
                guard_at < catchall_at,
                "{label} must match the queue-full guard arm before the \
                 catch-all conflict arm: swapping the two arms compiles and \
                 routes queue-full errors into the fail-closed conflict \
                 return, resurrecting the stop/refresh wedge with both \
                 needles still present"
            );
            // Round-24 review (m1, mutant confirmed): the order assertion is blind to "inserted
            // arms" — inserting a same-shaped log-and-continue arm between the guard and the
            // catch-all left both needles and the order assertion green (executed this round).
            // Round-25 review MAJOR-3 (another mutant confirmed): the [guard_at..catchall_at]
            // slice is still blind to arms inserted **before** the guard — the same arm inserted
            // between Ok(()) and the guard (compiles) leaves the in-window count at 1 and the
            // assertion green (executed this round). Changed to counting every `Err(` arm opener
            // (binding spelling irrelevant) from the match opener to the catch-all: any arm
            // inserted before or between pushes the count past the guard's own single one.
            let match_open = body
                .find("match queue_pending_revocation(")
                .expect("the queue_pending_revocation match must exist");
            assert!(
                match_open < guard_at,
                "{label} must keep the queue-full guard arm inside the \
                 queue_pending_revocation match"
            );
            assert_eq!(
                body[match_open..catchall_at].matches("Err(").count(),
                1,
                "{label} must keep the queue-full guard arm as the only Err \
                 arm before the catch-all conflict arm: an arm inserted \
                 before or between them (e.g. a conflicting-secret arm that \
                 logs and continues) silently flips the documented round-14 \
                 fail-closed boundary to fail-open (executed mutants)"
            );
        }
        // refresh's post-commit replay must likewise not wedge activate (the half-committed
        // window of a committed rotation must not be reported as a failure).
        let refresh_body = source
            .split("fn refresh(")
            .nth(1)
            .expect("refresh must exist");
        let end = ["\n    fn ", "\n    pub fn ", "\n    #[test]"]
            .iter()
            .filter_map(|marker| refresh_body.find(marker))
            .min()
            .unwrap_or(refresh_body.len());
        let refresh_body = comment_filtered(&refresh_body[..end]);
        // The entrance's fail-closed replay (start/resume/refresh, one caliber) is kept; the
        // post-commit, pre-activate replay must take the tolerant arm and never propagate `?`.
        assert!(
            refresh_body.contains("[web-access] refresh ignores pending-revocation replay failure"),
            "the post-commit replay before activate must tolerate failure like stop does",
        );
        // Round-16 review (M1b): the entrance replay's fail-closed `?` was previously an
        // optional assertion (a both-optional if-let) — deleting the entrance `?` line kept
        // this pin green, letting the entry path be silently flipped to tolerant. Changed to a
        // mandatory assertion + order verification.
        let entry_form = refresh_body.find("self.replay_pending_revocations()?;");
        assert!(
            entry_form.is_some(),
            "the entry-path replay must stay fail-closed (`?`): refresh may not \
             run against an undrained revocation backlog"
        );
        let tolerate_form =
            refresh_body.find("[web-access] refresh ignores pending-revocation replay failure");
        assert!(
            tolerate_form.is_some(),
            "the committed-rotation tolerate arm must exist",
        );
        assert!(
            entry_form < tolerate_form,
            "the fail-closed entry replay must precede the committed-rotation tolerate arm",
        );
        // Round-16 review (M1a): stop's rpc-ledger removal degradation had no pin — reverting
        // to `remove_rpc_ledger()?` would report a failure for a stop that already took effect,
        // and stop_current cannot be behaviorally driven (no mock_app), so the whole suite
        // stayed green.
        let stop_body = source
            .split("pub fn stop_current(")
            .nth(1)
            .expect("stop_current must exist");
        let stop_end = ["\n    fn ", "\n    pub fn ", "\n    #[test]"]
            .iter()
            .filter_map(|marker| stop_body.find(marker))
            .min()
            .unwrap_or(stop_body.len());
        let stop_body = comment_filtered(&stop_body[..stop_end]);
        assert!(
            !stop_body.contains("remove_rpc_ledger()?"),
            "stop must not propagate an rpc-ledger removal failure: the stop has \
             already taken effect and a `?` would report a completed stop as failed"
        );
        assert!(
            stop_body.contains("[web-access] stop ignores rpc-ledger removal failure"),
            "a failed rpc-ledger removal must degrade to a log, not fail the stop",
        );
    }

    /// Wiring pin (Round-10 review minor): the `web_access:status` payload carries the
    /// pairing URL and QR bitmap (the token's carrier) and must be emit_to("main"), not
    /// broadcast — reverting to `app.emit` would deliver it to aux WebViews (reader/pet)
    /// without any suite turning red.
    #[test]
    fn emit_status_scopes_the_payload_to_the_main_webview() {
        let source = include_str!("mod.rs");
        let start = source
            .find("fn emit_status(&self)")
            .expect("emit_status must exist");
        let body = &source[start..];
        let body = &body[..body.find("\n    fn ").unwrap_or(body.len())];
        assert!(
            body.contains("self.app.emit_to(\"main\", \"web_access:status\""),
            "web_access:status carries the pairing URL and QR; it must be scoped \
             to the main webview, not broadcast to every webview"
        );
    }

    /// Wiring pin (Round-11 review M2): redispatch must reset the TTL clock — both arms carry
    /// the same contract. handle_rpc_request's Pending arm is the real path for post-begin-
    /// persist-failure retries (after settle_begin_persist_failure clears
    /// dispatched_generation, redelivery necessarily passes through it); the
    /// prepare_bridge_generation arm already has a behavioral pin, but that pin cannot see
    /// this arm: the Round-11 mutant deleting this arm's clock reset kept all 3870 tests green
    /// while retries near the window's end were again fabricated into `rpc_timed_out` by the
    /// TTL sweep. Deleting either arm's reset turns this pin red.
    #[test]
    fn rpc_redispatch_rearms_the_dispatch_clock_in_both_arms() {
        let source = include_str!("mod.rs");
        let handle_start = source
            .find("fn handle_rpc_request(&self, value: &Value)")
            .expect("handle_rpc_request must exist");
        let body = &source[handle_start..];
        let body = &body[..body
            .find("\n    fn ")
            .expect("handle_rpc_request body must be terminated by the next fn")];
        assert!(
            body.contains("pending.dispatched_at = Instant::now();"),
            "the Pending re-dispatch arm must re-arm the TTL clock; without it a \
             begin-persist-failure retry inherits the original admission timestamp and \
             is fabricated into rpc_timed_out at the window tail"
        );
        let rpc_source = include_str!("rpc.rs");
        let prepare_start = rpc_source
            .find("fn prepare_bridge_generation(")
            .expect("prepare_bridge_generation must exist");
        let rpc_body = &rpc_source[prepare_start..];
        let rpc_body = &rpc_body[..rpc_body
            .find("\npub(super) fn ")
            .expect("prepare_bridge_generation body must be terminated by the next fn")];
        assert!(
            rpc_body.contains("pending.dispatched_at = Instant::now();"),
            "the prepare_bridge_generation re-dispatch arm must re-arm the TTL clock too"
        );
    }

    /// Wiring pin (Round-11 review): the phase-one default fix (a missing state_ready means
    /// the readiness gate applies) only takes effect at the `synchronize_client_from_value`
    /// call site — existing tests drive the helper directly and cannot see the call site;
    /// inlining the call site back to `unwrap_or(true)` (the pre-fix behavior, where clients
    /// omitting state_ready could skip the readiness gate and still receive business events)
    /// kept the whole suite green. Deleting the call site turns red.
    #[test]
    fn client_ready_readiness_gate_is_wired_at_the_synchronize_call_site() {
        let source = include_str!("mod.rs");
        let start = source
            .find("fn synchronize_client_from_value(&self, value: &Value)")
            .expect("synchronize_client_from_value must exist");
        let body = &source[start..];
        let body = &body[..body
            .find("\n    fn ")
            .expect("synchronize_client_from_value body must be terminated by the next fn")];
        let body = comment_filtered(body);
        assert!(
            body.contains("let state_ready = client_state_ready(value);"),
            "the readiness gate must be consulted at the synchronize call site; \
             inlining unwrap_or(true) back lets clients that omit state_ready skip it"
        );
        // Round-15 review (S18 F5): mere existence of the binding is not enough — deleting
        // the downstream consumption (so state_ready never participates in any judgment) kept
        // the contains above green. Assert that the binding is actually read afterwards.
        let bound = body
            .find("let state_ready = client_state_ready(value);")
            .expect("the binding assert above guarantees the anchor");
        let after_binding = &body[bound + "let state_ready = client_state_ready(value);".len()..];
        assert!(
            after_binding.contains("state_ready"),
            "state_ready must be consumed downstream of its binding, not bound and discarded",
        );
        // Round-16 follow-up review (M5): existence assertions cannot pin polarity — inverting
        // `if !state_ready {` into `if state_ready {` (delivering business events only to
        // not-ready clients) kept both the binding and downstream-consumption assertions green.
        // Pin the negated arm's byte-exact form (the only one in the function body).
        assert!(
            after_binding.contains("if !state_ready {"),
            "the readiness gate must stay negated (a not-ready client resets the \
             stream and must not receive business events); inverting the polarity \
             passes business events to not-ready clients while both presence pins \
             stay green"
        );
    }

    /// Wiring pin (Round-11 review minor): reserve_web_attachments' refusal string echoes
    /// only minted-shape handles. The method body needs an AppHandle (emit_status
    /// dependency), so behavioral pins cannot drive it; deleting the shape gate lets caller
    /// bytes back into the error chain (where a handle containing a `reserve Web chat turn: …`
    /// segment disguises a hard failure as transient busyness), turning this pin red.
    #[test]
    fn reserve_web_attachments_echoes_only_well_formed_handles() {
        assert!(attachment_handle_is_well_formed(
            "attachment_aB3xY9zWq1Mn2Vb5Cd7Ef0gh"
        ));
        assert!(!attachment_handle_is_well_formed(
            "attachment_reserve Web chat turn: session_turn_in_progress"
        ));
        assert!(!attachment_handle_is_well_formed("attachment_"));
        assert!(!attachment_handle_is_well_formed("attachment_x<y>"));
        assert!(!attachment_handle_is_well_formed("reserves"));
        let source = include_str!("mod.rs");
        let start = source
            .find("pub fn reserve_web_attachments(")
            .expect("reserve_web_attachments must exist");
        let body = &source[start..];
        let body = &body[..body
            .find("\n    pub fn ")
            .expect("reserve_web_attachments body must be terminated by the next fn")];
        let gate = body
            .find("attachment_handle_is_well_formed(handle)")
            .expect("reserve_web_attachments must gate echoed handles on the minted shape");
        let echo = body
            .find("远程控制附件句柄不存在或已过期：{handle}")
            .expect("the not-found arm must interpolate the handle");
        assert!(
            gate < echo,
            "the shape gate must run before the handle is interpolated into the error chain \
             (gate at {gate}, echo at {echo})"
        );
    }

    /// Wiring pin (Round-11 review): the manager half of restore-after-revoke —
    /// `restore_web_workspace_grant` must read the **live endpoint's** consent flag and pass
    /// it to the store (the store half's refusal semantics are discriminated by
    /// workspace_grants.rs's behavioral pin); otherwise reverting to a hardcoded `true`
    /// revives the "reserve → revoke → creation fails → restore resurrects the grant"
    /// sequence (the bind-time recheck still exists, making this a defense-in-depth gap rather
    /// than an open hole). Reverting the wiring turns this pin red.
    #[test]
    fn restore_web_workspace_grant_reads_the_live_consent_flag() {
        let source = include_str!("mod.rs");
        let start = source
            .find("pub(crate) fn restore_web_workspace_grant(")
            .expect("restore_web_workspace_grant must exist");
        let body = &source[start..];
        let body = &body[..body.find("\n    pub(crate) fn ").unwrap_or_else(|| {
            body.find("\n    fn ")
                .expect("restore_web_workspace_grant body must be terminated by the next fn")
        })];
        let flag = body
            .find("endpoint.config.allow_host_workspace")
            .expect("restore must read the live endpoint's consent flag");
        let pass = body
            .find("allow_host_workspace,")
            .expect("restore must pass the flag into the store");
        assert!(
            flag < pass,
            "the consent flag read must feed the store call \
             (flag read at {flag}, store pass at {pass})"
        );
    }

    /// Wiring pin (Round-11 review minor): acknowledgement must persist the whole of the
    /// in-process authoritative ledger (`persist_pending_revocations`) and maintain the
    /// backlog in sync; reverting to "re-read disk → remove one → rewrite"
    /// (`acknowledge_pending_revocation`) would, after a salvage degradation, take an empty
    /// ledger and clear in-memory sibling entries. The method body needs an AppHandle, so
    /// behavioral pins cannot drive it — pinned as a source contract per this module's
    /// convention.
    #[test]
    fn revocation_ack_persists_the_process_backlog_instead_of_rereading_disk() {
        let source = include_str!("mod.rs");
        let start = source
            .find("fn finish_pending_revocation(")
            .expect("finish_pending_revocation must exist");
        let body = &source[start..];
        let body = &body[..body
            .find("\n    fn ")
            .expect("finish_pending_revocation body must be terminated by the next fn")];
        assert!(
            body.contains("persist_pending_revocations(&ledger)"),
            "the acknowledgement must persist the caller-held backlog wholesale"
        );
        assert!(
            body.contains("pending_revocation_backlog"),
            "the acknowledgement must restore the backlog on failure and commit it on success"
        );
        assert!(
            !body.contains("acknowledge_pending_revocation("),
            "the disk-rereading acknowledgement must not come back; it drops salvaged \
             siblings when the on-disk ledger is absent"
        );
    }

    /// Round-14 review minor (source-contract pin): replay loading must merge as "union with
    /// memory priority" — a naive `backlog = ledger.entries…collect()` wholesale overwrite
    /// would, on the second lifecycle call, wipe the entries that exist only in memory after a
    /// salvage degradation from both sides at once. Reverting to wholesale overwrite turns
    /// this pin red.
    #[test]
    fn revocation_replay_merges_disk_entries_into_the_process_backlog() {
        let source = include_str!("mod.rs");
        let start = source
            .find("fn replay_pending_revocations(")
            .expect("replay_pending_revocations must exist");
        let body = &source[start..];
        let body = &body[..body
            .find("\n    fn ")
            .expect("replay_pending_revocations body must be terminated by the next fn")];
        let body = comment_filtered(body);
        assert!(
            body.contains("pending_revocation_key(existing) == pending_revocation_key(entry)"),
            "the replay must dedupe by key instead of overwriting the process backlog"
        );
        assert!(
            !body.contains("pending_revocation_backlog = ledger.entries"),
            "the plain disk-overwrite must not come back; it drops memory-only salvaged entries"
        );
        // Round-15 review (S14/S18 F4, mutant executed): swapping the merge seed from held to
        // ledger.entries (disk priority, same-key conflicts keeping the disk version) still
        // kept both assertions above green — the key-dedup line is token-symmetric and
        // direction-blind. Pin the memory-priority seed line: same-key different-value
        // conflicts keep the memory version (exactly what the in-flight worker retries).
        assert!(
            body.contains("let mut merged = held;"),
            "the merge must seed from the in-process backlog (memory wins conflicts); \
             seeding from the ledger silently inverts the documented precedence"
        );
        // Round-16 follow-up review (M5): the seed line cannot pin the conflict arm — if the
        // same-key-same-value site were rewritten into retain+push overwrite (ledger version
        // wins), the seed line stays green. Pin the "append only when the key is absent" guard
        // shape byte-exactly: same-key different-value conflicts keep the memory version
        // (exactly what the in-flight worker retries).
        assert!(
            body.contains(
                "if !merged.iter().any(|existing| {\n                    pending_revocation_key(existing) == pending_revocation_key(entry)\n                }) {\n                    merged.push(entry.clone());\n                }"
            ),
            "the conflict arm must stay append-only-on-absent-key: a retain+push \
             overwrite of the same key flips conflict resolution to ledger-wins \
             while the seed pin stays green"
        );
    }

    /// Wiring pin (Round-11 review minor): subscription frames must pass the active-lease
    /// gate before touching the subscription set — a late dead-lease subscription frame would
    /// let the unsubscribe strip a subscription the new client just installed. The dispatch
    /// body needs an AppHandle, so behavioral pins cannot drive it; deleting the gate or
    /// moving it after the subscription change turns red.
    #[test]
    fn event_subscriptions_are_lease_gated_under_the_mutation_lock() {
        // Round-14 review minor (alternative to the dispatch-site version): the gate check must
        // live inside handle_event_subscription's body, under the same lock as the subscription
        // change — the old shape (the dispatch arm validating the lease separately and then
        // admitting) left a takeover window between the two locks; this pin turns red on that
        // revert shape (move the gate check out of the function body and the predicate call is
        // gone).
        let source = include_str!("mod.rs");
        let start = source
            .find("fn handle_event_subscription(")
            .expect("handle_event_subscription must exist");
        let body = &source[start..];
        let end = ["\n    fn ", "\n    pub fn ", "\n    #[test]"]
            .iter()
            .filter_map(|marker| body.find(marker))
            .min()
            .expect("handle_event_subscription body must be terminated by the next item");
        let body = &body[..end];
        let lock = body
            .find("let mut inner = self.inner.lock();")
            .expect("the handler must lock Inner for the mutation");
        let gate = body
            .find("rpc_request_targets_active_lease(&inner, lease_id)")
            .expect("the lease gate must live inside handle_event_subscription");
        let mutate = body
            .find("inner.subscriptions")
            .expect("the handler must mutate the subscription set");
        assert!(
            lock < gate && gate < mutate,
            "the lease gate must run under the same lock acquisition as the subscription mutation"
        );
    }

    /// Round-11 review NIT: char-boundary truncation of relay free text before storage (the
    /// same caliber as `rpc_error_completion`'s 16,384), never splitting a multi-byte character.
    #[test]
    fn bound_relay_text_truncates_on_char_boundaries() {
        let short = "短错误".to_string();
        assert_eq!(bound_relay_text(short.clone()), short);
        let long = "界".repeat(RELAY_ERROR_TEXT_MAX_CHARS + 10);
        let bound = bound_relay_text(long);
        assert_eq!(bound.chars().count(), RELAY_ERROR_TEXT_MAX_CHARS);
        assert!(
            bound.ends_with('界'),
            "the cut must stay on a char boundary"
        );
    }

    /// Wiring pin: relay-supplied error text must be truncated via [`bound_relay_text`] at
    /// the `set_endpoint_error_with_code` storage point — the unit pin tests only the function
    /// itself and cannot see call sites; deleting the truncation from the storage point (up to
    /// 4 MiB of single-frame text stored verbatim, re-serialized on every emit_status) turns
    /// red.
    #[test]
    fn endpoint_error_persistence_bounds_the_relay_text() {
        let source = include_str!("mod.rs");
        let start = source
            .find(
                "fn set_endpoint_error_with_code(&self, code: Option<&'static str>, error: String)",
            )
            .expect("set_endpoint_error_with_code must exist");
        let body = &source[start..];
        let body = &body[..body
            .find("\n    fn ")
            .expect("set_endpoint_error_with_code body must be terminated by the next fn")];
        assert!(
            body.contains("let error = bound_relay_text(error);"),
            "the relay-supplied error text must be bounded before it is stored"
        );
    }

    #[test]
    fn registration_error_reasons_are_actionable() {
        // Credentials refused → rotate; config unusable → rotate to recover; version mismatch
        // → update the desktop (rotation will not help); other codes keep the relay text
        // verbatim. The recovery-code and raw-text branches must stay consistent: SettingsView
        // maps the code to trilingual copy, with the raw text as fallback.
        assert!(
            relay_registration_error_reason("invalid_access_token", "denied")
                .contains("rotate Web access")
        );
        assert!(
            relay_registration_error_reason("invalid_desktop_secret", "denied")
                .contains("rotate Web access")
        );
        assert!(
            relay_registration_error_reason("bad_desktop_endpoint_register", "rejected")
                .contains("rotate Web access")
        );
        let version = relay_registration_error_reason(
            "unsupported_protocol_version",
            "protocol version 2 is required",
        );
        assert!(
            version.contains("update this desktop"),
            "version mismatch must point at updating, not rotating: {version}"
        );
        assert!(!version.contains("rotate"));
        assert_eq!(
            relay_registration_error_reason("endpoint_capacity_reached", "capacity"),
            "capacity"
        );

        assert_eq!(
            relay_registration_error_code("invalid_access_token"),
            Some("rotate_credentials")
        );
        assert_eq!(
            relay_registration_error_code("invalid_desktop_secret"),
            Some("rotate_credentials")
        );
        assert_eq!(
            relay_registration_error_code("bad_desktop_endpoint_register"),
            Some("rotate_recover_config")
        );
        assert_eq!(
            relay_registration_error_code("unsupported_protocol_version"),
            Some("update_desktop")
        );
        assert_eq!(
            relay_registration_error_code("endpoint_capacity_reached"),
            None
        );

        // Round-7 review: the two maps' branch alignment is pinned by looping over the whole
        // terminal-code table — adding a branch to only one map (missing the other) would
        // render that code's raw text with no recovery code (fail-soft but inconsistent; a
        // per-code independent assertion cannot catch it). endpoint_revoked goes through
        // handle_revoked's full cleanup path where neither hint applies; that partition is
        // asserted explicitly.
        for code in TERMINAL_REGISTRATION_ERROR_CODES {
            let recovery = relay_registration_error_code(code);
            let hinted = relay_registration_error_reason(code, "relay said no") != "relay said no";
            assert_eq!(
                recovery.is_some(),
                hinted,
                "terminal code {code} must map to a recovery code and a reason hint together"
            );
            if code == &"endpoint_revoked" {
                assert!(
                    recovery.is_none(),
                    "revocation is handled by the full revoke path, not a hint"
                );
            } else {
                assert!(
                    recovery.is_some(),
                    "terminal code {code} must carry recovery guidance"
                );
            }
        }
    }

    #[test]
    fn client_ready_without_state_ready_defaults_to_phase_one() {
        // PROTOCOL.md phase-1: a missing/invalid state_ready must not count as ready, or
        // clients that never send the field would skip the readiness barrier and receive
        // business events directly.
        assert!(!client_state_ready(&json!({})));
        assert!(!client_state_ready(&json!({ "state_ready": false })));
        assert!(!client_state_ready(&json!({ "state_ready": "yes" })));
        assert!(client_state_ready(&json!({ "state_ready": true })));
    }

    #[test]
    fn oversize_private_ledgers_fail_closed_with_a_size_error() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-ledger-bounds-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        std::fs::create_dir_all(&directory).unwrap();

        let revocations = directory.join("pending.json");
        std::fs::write(
            &revocations,
            vec![b'x'; MAX_PENDING_REVOCATIONS_FILE_BYTES + 1],
        )
        .unwrap();
        let error = load_pending_revocations_from(&revocations)
            .expect_err("oversize revocation ledger must fail closed");
        assert!(error.contains("larger than"), "unexpected error: {error}");

        let ledger_path = directory.join("rpc-ledger.json");
        std::fs::write(&ledger_path, vec![b'x'; MAX_RPC_LEDGER_FILE_BYTES + 1]).unwrap();
        let error = load_or_initialize_rpc_ledger_at(&ledger_path, TEST_ENDPOINT_ID)
            .expect_err("oversize RPC ledger must fail closed before parsing");
        assert!(error.contains("larger than"), "unexpected error: {error}");
        assert!(
            !matches!(load_config_for_stop_at(&ledger_path), Some(_)),
            "the stop path treats an unreadable file as no config"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn loading_a_config_repairs_world_readable_private_files() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-config-perm-repair-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("web-access.json");
        let config = test_web_config("ep_perm_repair");
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        // POSIX modes only exist on unix; the mode manipulation lives behind
        // the platform adapter so this file carries no target selector.
        if !crate::platform::test_support::force_private_file_mode(&path, 0o644) {
            std::fs::remove_dir_all(directory).unwrap();
            return;
        }

        assert_eq!(load_config_from(&path).unwrap(), Some(config));
        assert_eq!(
            crate::platform::test_support::permission_bits(&path),
            Some(0o600),
            "load must restore the private 0600 baseline after a successful parse"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn stop_tolerates_a_corrupt_config_and_still_removes_it() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-stop-tolerant-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("web-access.json");
        std::fs::write(&path, b"{corrupt json").unwrap();

        // start/resume/refresh stay fail-closed;
        assert!(load_config_from(&path).is_err());
        // The stop path degrades a corrupt config to "no config" and still deletes the file.
        assert_eq!(load_config_for_stop_at(&path), None);
        std::fs::remove_file(&path).unwrap();
        assert!(!path.exists(), "stop must leave no unparsable state behind");
        std::fs::remove_dir_all(directory).unwrap();

        // Round-6 review minor: only the tolerant read was pinned above; the "still deletes
        // the file after degrading" wiring lives in stop_current, unreachable by unit tests
        // (no mock_app), so it is pinned per this module's source-contract convention — the
        // degraded read and the removal call must both exist in stop_current; missing either
        // turns red.
        let source = include_str!("mod.rs");
        let start = source
            .find("pub fn stop_current(&self)")
            .expect("stop_current must exist");
        let body = &source[start..];
        let body = &body[..body
            .find("\n    pub fn ")
            .or_else(|| body.find("\n    fn "))
            .expect("stop_current body must be terminated by the next fn")];
        assert!(
            body.contains("load_config_for_stop()"),
            "stop must degrade a corrupt config via the tolerant read"
        );
        assert!(
            body.contains("remove_config()"),
            "stop must still remove the (corrupt) config file after the tolerant read"
        );
    }

    #[test]
    fn pending_revocation_debug_redacts_the_desktop_secret() {
        let pending = test_pending_revocation("ep_debug");
        let rendered = format!("{pending:?}");
        assert!(
            rendered.contains("ep_debug"),
            "non-secret fields stay visible"
        );
        assert!(
            !rendered.contains(&pending.desktop_secret),
            "the desktop secret must never appear in Debug output"
        );
    }

    /// Round-6 review: the 0600 repair must hang off the first handle, before any content
    /// read — corrupt non-UTF-8 content takes the read leg's error return, so a repair
    /// attached after the read never runs, leaving a corrupt + world-readable credentials
    /// file sitting on disk (in direct contradiction to the load comment's threat model).
    /// Moving the repair back after the read, or deleting it, turns red.
    /// Windows/fallback has no POSIX mode; the mode assertion is skipped via the test_support
    /// hook.
    #[test]
    fn corrupt_world_readable_config_is_still_repaired_on_load() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-config-repair-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("web-access.json");
        // Non-UTF-8 bytes: read_to_string necessarily fails, simulating the "corrupt content + loosened permissions" combination.
        std::fs::write(&path, [0xff_u8, 0xfe, b'{', b'\n']).unwrap();
        let enforced = crate::platform::test_support::force_private_file_mode(&path, 0o644);
        let outcome = load_config_from(&path);
        assert!(outcome.is_err(), "corrupt content must still fail closed");
        if enforced {
            assert_eq!(
                crate::platform::test_support::permission_bits(&path),
                Some(0o600),
                "the load-time repair must land on the first handle, before the failing read"
            );
        }
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// Round-7 review M1: the ledger side's 0600 repair must mirror the config's shape —
    /// hanging off the **first** handle, before any content read. The ledger reads via
    /// read_to_end (which does not fail on non-UTF-8), so only the read leg's real I/O error
    /// can expose the ordering: "the path is a directory" is used here to produce an EISDIR
    /// that opens successfully on unix but whose read(2) must fail. With the repair attached
    /// after the read (the pre-round-6 shape) or deleted, Unavailable returns early with
    /// permissions still loosened and this test goes red. Windows has no POSIX mode; the mode
    /// assertion is skipped via the test_support hook (opening a directory fails outright on
    /// that platform, also taking the Unavailable path).
    #[test]
    fn corrupt_world_readable_ledger_is_still_repaired_on_load() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-ledger-repair-order-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("pending.json");
        std::fs::create_dir(&path).unwrap();
        let enforced = crate::platform::test_support::force_private_file_mode(&path, 0o755);
        let outcome = load_pending_revocations_from(&path);
        assert!(
            outcome.is_err(),
            "the read-leg I/O error must surface as unavailable"
        );
        if enforced {
            assert_eq!(
                crate::platform::test_support::permission_bits(&path),
                Some(0o600),
                "the load-time repair must land on the first handle, before the failing read"
            );
        }
        std::fs::remove_dir_all(directory).ok();
    }
}
