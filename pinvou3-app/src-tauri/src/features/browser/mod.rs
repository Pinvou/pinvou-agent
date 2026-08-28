// architecture-guard: allow-target-cfg -- Unix-only symlink regressions verify browser recovery never follows hostile journal or quarantine roots; no platform implementation detail leaves platform::filesystem.
//! Browser feature: manages native browser sessions, navigation, and tabs shared by
//! the Agent and user. The user-visible path uses only system WebView child views in
//! the main window. An unavailable native surface is reported explicitly; continuous
//! screenshot streaming and external-browser display fallbacks are not used.
//!
//! The MCP wrapper (`bundle/mcp-servers/browser-wrapper.mjs`) coordinates the same
//! browser instance through `~/.pinvou3/browser/cdp-port.json`. On Windows, the
//! wrapper first writes `host-requests/*.json` so the main app creates a task-owned
//! WebView2, then connects chrome-devtools-mcp to its loopback CDP port. CDP is used
//! only for Windows Agent automation, not for transporting the user-visible page.
//!
//! Scope: **desktop only for this release**. `browser:*` events are emitted locally
//! and are not forwarded to the remote Web UI (the relay `access-policy.json`
//! allowlist contains no `browser:*` events or commands). Web/mobile browser tabs
//! and interaction remain future work and must not be documented as supported.

mod cdp;
mod core;
mod platform;

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::platform::filesystem::{MovePlainFileOutcome, PrivateFileDirectory};
use crate::platform::paths;
use platform::state::{
    NativeRequestCancel, NativeRequestClaim, NativeTabLease, RetainedAgentOperation,
};

pub use cdp::CdpSession;

/// Must run before Tauri creates any WebKit context.
pub(crate) fn prepare_process_environment() {
    platform::prepare_process_environment();
}

/// Installs platform automation state during Tauri setup, before BrowserManager
/// can create a task-owned browser child WebView.
pub(crate) fn install_automation_context(app: &mut tauri::App) {
    platform::install_automation_context(app);
}

#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeSurfaceBounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl NativeSurfaceBounds {
    fn validate(self) -> Result<Self, String> {
        const MAX_COORDINATE: i32 = 32_768;
        const MAX_SIZE: i32 = 16_384;
        if !(-MAX_COORDINATE..=MAX_COORDINATE).contains(&self.x)
            || !(-MAX_COORDINATE..=MAX_COORDINATE).contains(&self.y)
            || !(1..=MAX_SIZE).contains(&self.width)
            || !(1..=MAX_SIZE).contains(&self.height)
        {
            return Err("native browser bounds are outside the valid range".to_string());
        }
        Ok(self)
    }
}

/// Tab identity (targetId) to flattened sessionId cache. Reuses attachments within
/// one automation connection and repairs active state as targets change. It is not
/// the source of UI event scope.
type PageSessions = Arc<parking_lot::Mutex<HashMap<String, String>>>;
type BrowserSessionValidator = Arc<dyn Fn(&str) -> bool + Send + Sync>;

const BROWSER_WATCH_RETRY_INITIAL_MS: u64 = 250;
const BROWSER_WATCH_RETRY_MAX_MS: u64 = 30_000;

/// One page tab.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TabInfo {
    pub target_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_id: Option<u64>,
    pub title: String,
    pub url: String,
}

#[derive(Default)]
struct Inner {
    port: Option<u16>,
    /// Browser-level CDP session (one connection manages all tabs).
    session: Option<Arc<CdpSession>>,
    /// Flattened sessionId for the active tab.
    active_session: Option<String>,
    /// targetId for the active tab, kept in sync with active_session. Public status
    /// and event payloads always identify tabs by targetId. A sessionId is created
    /// for each attachment and changes when the same tab is reattached.
    active_target: Option<String>,
    /// Event-loop task handle, used to prevent duplicate loops and allow aborting.
    loop_task: Option<tokio::task::JoinHandle<()>>,
    /// CDP WebSocket reader task, aborted on stop/crash reset to avoid stale readers.
    reader_task: Option<tokio::task::JoinHandle<()>>,
}

/// Browser manager injected as singleton Tauri state.
pub struct BrowserManager {
    inner: tokio::sync::Mutex<Inner>,
    /// Startup critical-section mutex. Serializes browser coordination, automation
    /// connection, attachment, and event-loop setup so watcher polling and Tauri
    /// commands cannot create duplicate loops or lose handles. stop() also joins
    /// this single-flight lock so it cannot return on transient empty state and be
    /// overwritten by the rest of an in-progress startup.
    start_mtx: tokio::sync::Mutex<()>,
    /// Serializes lifecycle mutations for one task without making slow
    /// automation readiness/binding waits block unrelated task workspaces.
    /// Weak entries are pruned whenever a task lock is requested.
    session_lifecycle_locks: parking_lot::Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
    /// Admission gate held through each hosted request's final platform
    /// dispatch. Data and control scanners take concurrent read guards so a
    /// slow page operation cannot starve the control-lease heartbeat; restart
    /// takes the write guard before `start_mtx` and waits for all accepted work.
    hosted_request_gate: tokio::sync::RwLock<()>,
    /// Stop generation. Every stop increments it; ensure_started snapshots and
    /// rechecks it, discarding startup results interrupted by a stop.
    stop_gen: std::sync::atomic::AtomicU64,
    /// Whether browser:activated has been emitted. Shared by watch and stop; stop
    /// and crash paths clear it so reconnection always republishes the frontend tab.
    activated: std::sync::atomic::AtomicBool,
    /// Main-process shutdown flag. Once shutdown_on_exit sets it, watch exits and
    /// ensure_started rejects work, preventing an orphan browser at process exit.
    shutting_down: std::sync::atomic::AtomicBool,
    /// Host-issued renderer generation with monotonically increasing sequences per
    /// generation. After renderer/HMR reload, late show/hide calls from an older
    /// generation cannot override the new renderer. This lock covers the native
    /// visibility mutation commit point.
    surface_visibility: parking_lot::Mutex<SurfaceVisibilityClock>,
    /// target_id to flattened sessionId cache. CDP creates a distinct session for
    /// every attachment and does not release it automatically; without reuse,
    /// frequent enumeration and switching would leak Chrome-side sessions.
    page_sessions: PageSessions,
    app: parking_lot::Mutex<Option<AppHandle>>,
    /// The composition root injects a narrow session-lifecycle validator so the
    /// browser feature does not depend on its sessions sibling. Deletion first sets
    /// a local pending deny marker, then tears down WebViews/files asynchronously.
    /// Successful cleanup removes the marker; the durable validator then remains
    /// the authority for rejecting absent tasks.
    session_validator: parking_lot::Mutex<Option<BrowserSessionValidator>>,
    pending_deleted_session_ids: parking_lot::RwLock<HashSet<String>>,
    /// After a native mutation commits physically, restore-manifest/authoritative-map
    /// I/O failure must not make the operation appear uncommitted. Keep task-visible
    /// degraded state and repair it through a per-task backoff queue. Restore writes
    /// and warning/worker state updates share one critical section so an older worker
    /// cannot consume a fresh failure while exiting successfully.
    persistence_io: parking_lot::Mutex<()>,
    persistence_warnings: parking_lot::Mutex<HashMap<String, String>>,
    persistence_retries: parking_lot::Mutex<HashSet<String>>,
    /// Single-flight coalescing for native-restore persistence per task. A page
    /// can fire reserved takeover/location signals in a tight loop; while one
    /// write is in flight a later signal only marks the session dirty, and the
    /// finishing flight performs exactly one follow-up write. The map holds an
    /// entry only while a flight is running, so it cannot grow with history.
    persistence_inflight: parking_lot::Mutex<HashMap<String, bool>>,
    /// A durable Prepare journal is recovered synchronously before transient
    /// host requests are reset. Any recovery failure blocks every browser
    /// entry point so a stale restore manifest cannot be published first.
    prepare_recovery_error: parking_lot::Mutex<Option<String>>,
    /// Startup recovery/consumer installation is re-armed after transient I/O
    /// failures. Only the current capped delay is retained; successful startup
    /// resets it, so this cannot grow with process lifetime or failure count.
    watch_retry_delay_ms: std::sync::atomic::AtomicU64,
    /// Only commits produced by this process may use disappearance of its
    /// transient request/response artifacts as a success acknowledgement.
    /// Recovered commits are deliberately absent: process-start reset also
    /// makes those artifacts disappear and must not be mistaken for wrapper ACK.
    locally_committed_prepares: parking_lot::Mutex<HashSet<(String, String)>>,
    /// Startup orphan cleanup may delete only files that predate this cutoff. Files
    /// created by this process after the static session snapshot are never eligible.
    startup_reconcile_cutoff: SystemTime,
    /// Three-platform native browser host state. Platform details stay in feature
    /// adapters; unsupported capabilities are explicit and never switch to a
    /// screenshot or external-browser fallback.
    native_surface: parking_lot::Mutex<platform::NativeBrowserSurface>,
}

#[derive(Debug, serde::Deserialize)]
struct HostedBrowserRequest {
    protocol_version: u8,
    request_id: String,
    idempotency_key: String,
    session_id: String,
    session_token: String,
    caller_pid: u32,
    wrapper_instance_nonce: String,
    operation: HostedBrowserOperation,
    requested_at: u64,
    tab_token: Option<String>,
    authorization_tab_token: Option<String>,
    creation_id: Option<String>,
    url: Option<String>,
    target_id: Option<String>,
    revision: Option<u64>,
    lease: Option<String>,
    tool_name: Option<String>,
    tool_arguments: Option<Value>,
    #[serde(default)]
    background: bool,
    #[serde(default)]
    emits_trusted_input: bool,
    /// Only the small, explicit observation allowlist may set this. Missing or
    /// future tool metadata therefore remains fail-closed while a user
    /// navigation generation is loading.
    #[serde(default)]
    observational_only: bool,
}

#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum HostedBrowserOperation {
    Prepare,
    CreateTab,
    ActivateTab,
    CloseTab,
    RollbackCreatedTab,
    AssertHostLease,
    BeginAgentOperation,
    RefreshAgentOperation,
    RefreshAgentInput,
    EndAgentOperation,
    CoreTool,
}

impl HostedBrowserOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::CreateTab => "create_tab",
            Self::ActivateTab => "activate_tab",
            Self::CloseTab => "close_tab",
            Self::RollbackCreatedTab => "rollback_created_tab",
            Self::AssertHostLease => "assert_host_lease",
            Self::BeginAgentOperation => "begin_agent_operation",
            Self::RefreshAgentOperation => "refresh_agent_operation",
            Self::RefreshAgentInput => "refresh_agent_input",
            Self::EndAgentOperation => "end_agent_operation",
            Self::CoreTool => "core_tool",
        }
    }

    /// Control-plane requests must never queue behind a different session's
    /// slow prepare/CDP/BrowserCore operation. They touch only the in-memory
    /// lease state and are serviced by a dedicated lightweight scanner.
    const fn is_control_plane(self) -> bool {
        matches!(
            self,
            Self::AssertHostLease
                | Self::BeginAgentOperation
                | Self::RefreshAgentOperation
                | Self::RefreshAgentInput
                | Self::EndAgentOperation
        )
    }

    /// Match the wrapper's externally observable request deadlines. A request
    /// that could no longer have a live caller must not acquire fresh Agent
    /// authority merely because a crashed wrapper left its artifact behind.
    const fn maximum_artifact_age_ms(self) -> u64 {
        match self {
            Self::Prepare | Self::CoreTool => 25_000,
            _ => 12_000,
        }
    }

    /// Cleanup requests must remain executable after their wrapper epoch has
    /// died. Every other request can grant authority or touch an external
    /// browser surface, and therefore requires a live matching caller epoch.
    const fn requires_live_caller(self) -> bool {
        !matches!(self, Self::EndAgentOperation | Self::RollbackCreatedTab)
    }
}

/// Keep this allowlist aligned with `browserToolMayMutate` in the Windows
/// wrapper protocol. Unknown and script-capable tools are intentionally
/// classified as mutations so a future tool cannot bypass an accepted user
/// navigation merely because this host predates it.
fn browser_core_tool_is_observational(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "list_pages"
            | "select_page"
            | "take_snapshot"
            | "wait_for"
            | "get_console_message"
            | "get_network_request"
            | "list_console_messages"
            | "list_network_requests"
            | "performance_analyze_insight"
    )
}

#[derive(Debug, serde::Deserialize)]
struct HostedBrowserCancellation {
    protocol_version: u8,
    kind: String,
    request_id: String,
    idempotency_key: String,
    session_id: String,
    session_token: String,
    caller_pid: u32,
    wrapper_instance_nonce: String,
    #[serde(default)]
    prepare_compensation: Option<HostedPrepareCompensation>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum HostedPreparePhase {
    Pending,
    Prepared,
    Committed,
    Cancelled,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct HostedPrepareCompensation {
    protocol_version: u8,
    kind: String,
    request_id: String,
    idempotency_key: String,
    session_id: String,
    session_token: String,
    caller_pid: u32,
    wrapper_instance_nonce: String,
    rollback_kind: String,
    revision: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HostedPrepareJournal {
    protocol_version: u8,
    kind: String,
    phase: HostedPreparePhase,
    compensation: HostedPrepareCompensation,
    requested_at: u64,
    updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    response: Option<Value>,
}

enum HostedPrepareJournalMatch {
    Absent,
    Matching(HostedPrepareJournal),
    Superseded,
}

fn should_remove_prepare_restore(rollback_kind: &str, rollback_applied: bool) -> bool {
    rollback_kind == "prepared_session" && rollback_applied
}

#[derive(Debug, serde::Deserialize)]
struct HostedCallerHeartbeat {
    protocol_version: u8,
    kind: String,
    session_id: String,
    session_token: String,
    caller_pid: u32,
    wrapper_instance_nonce: String,
    heartbeat_at: u64,
}

struct HostedBrowserOutcome {
    result: Value,
    error: Option<String>,
    /// Serialized compensation metadata retained in the in-memory request
    /// ledger. A cancellation that arrives after completion uses it to undo
    /// only resources created by this request.
    rollback: Value,
}

impl HostedBrowserOutcome {
    fn new(result: Value) -> Self {
        Self {
            result,
            error: None,
            rollback: json!({ "kind": "none" }),
        }
    }

    fn with_rollback(result: Value, rollback: Value) -> Self {
        Self {
            result,
            error: None,
            rollback,
        }
    }

    fn failed_with_rollback(error: String, rollback: Value) -> Self {
        Self {
            result: Value::Null,
            error: Some(error),
            rollback,
        }
    }
}

#[derive(Debug)]
struct PrepareWorkspaceError {
    message: String,
    rollback: Option<Value>,
}

impl PrepareWorkspaceError {
    fn compensated(message: String, rollback: Value) -> Self {
        Self {
            message,
            rollback: Some(rollback),
        }
    }
}

impl From<String> for PrepareWorkspaceError {
    fn from(message: String) -> Self {
        Self {
            message,
            rollback: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestoreWorkspaceOutcome {
    Missing,
    Existing,
    Restored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreparedWorkspaceDisposition {
    Existing,
    CreatedBlank,
    RestoredExisting,
}

fn has_restore_automation_backend(
    capabilities: platform::NativeSurfaceCapabilities,
    browser_core_available: bool,
) -> bool {
    capabilities.chrome_devtools_protocol || browser_core_available
}

impl PreparedWorkspaceDisposition {
    const fn rollback_kind(self) -> Option<&'static str> {
        match self {
            Self::Existing => None,
            Self::CreatedBlank => Some("prepared_session"),
            Self::RestoredExisting => Some("restored_session"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopedStopAction {
    CloseNativeSession,
    IgnoreUnknownNativeSession,
    StopManagedRuntime,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct SurfaceVisibilityClock {
    generation: u64,
    sequence: u64,
}

impl SurfaceVisibilityClock {
    fn begin_generation(&mut self) -> u64 {
        self.generation = self.generation.saturating_add(1).max(1);
        self.sequence = 0;
        self.generation
    }

    fn claim(&mut self, generation: u64, sequence: u64) -> bool {
        if generation == 0
            || generation != self.generation
            || sequence == 0
            || sequence <= self.sequence
        {
            return false;
        }
        self.sequence = sequence;
        true
    }
}

/// Native workspaces are closed per task. While any workspace is registered, an
/// unknown session is an idempotent success and must never fall back to a global stop.
/// The shared automation runtime is cleaned only when the registry is empty.
fn scoped_stop_action(requested_exists: bool, has_native_sessions: bool) -> ScopedStopAction {
    if requested_exists {
        ScopedStopAction::CloseNativeSession
    } else if has_native_sessions {
        ScopedStopAction::IgnoreUnknownNativeSession
    } else {
        ScopedStopAction::StopManagedRuntime
    }
}

impl BrowserManager {
    pub fn new() -> Self {
        Self {
            inner: tokio::sync::Mutex::new(Inner::default()),
            start_mtx: tokio::sync::Mutex::new(()),
            session_lifecycle_locks: parking_lot::Mutex::new(HashMap::new()),
            hosted_request_gate: tokio::sync::RwLock::new(()),
            stop_gen: std::sync::atomic::AtomicU64::new(0),
            activated: std::sync::atomic::AtomicBool::new(false),
            shutting_down: std::sync::atomic::AtomicBool::new(false),
            surface_visibility: parking_lot::Mutex::new(SurfaceVisibilityClock::default()),
            page_sessions: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            app: parking_lot::Mutex::new(None),
            session_validator: parking_lot::Mutex::new(None),
            pending_deleted_session_ids: parking_lot::RwLock::new(HashSet::new()),
            persistence_io: parking_lot::Mutex::new(()),
            persistence_warnings: parking_lot::Mutex::new(HashMap::new()),
            persistence_retries: parking_lot::Mutex::new(HashSet::new()),
            persistence_inflight: parking_lot::Mutex::new(HashMap::new()),
            prepare_recovery_error: parking_lot::Mutex::new(None),
            watch_retry_delay_ms: std::sync::atomic::AtomicU64::new(BROWSER_WATCH_RETRY_INITIAL_MS),
            locally_committed_prepares: parking_lot::Mutex::new(HashSet::new()),
            startup_reconcile_cutoff: SystemTime::now(),
            native_surface: parking_lot::Mutex::new(platform::NativeBrowserSurface::default()),
        }
    }

    pub(crate) fn bind_session_validator(&self, validator: BrowserSessionValidator) {
        *self.session_validator.lock() = Some(validator);
    }

    fn session_lifecycle_lock(&self, session_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.session_lifecycle_locks.lock();
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(session_id).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(session_id.to_string(), Arc::downgrade(&lock));
        lock
    }

    /// Called by the composition root after durable deletion and before asynchronous
    /// cleanup. All late wrapper requests then fail closed, while WebView, restore,
    /// and MCP file cleanup can retry independently.
    pub(crate) fn mark_session_deleted(&self, session_id: &str) {
        self.pending_deleted_session_ids
            .write()
            .insert(session_id.to_string());
    }

    fn clear_session_deleted(&self, session_id: &str) {
        self.pending_deleted_session_ids.write().remove(session_id);
    }

    fn next_watch_retry_delay(&self) -> Duration {
        let current = self
            .watch_retry_delay_ms
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |delay| Some(delay.saturating_mul(2).min(BROWSER_WATCH_RETRY_MAX_MS)),
            )
            .unwrap_or(BROWSER_WATCH_RETRY_MAX_MS);
        Duration::from_millis(current)
    }

    fn reset_watch_retry_delay(&self) {
        self.watch_retry_delay_ms.store(
            BROWSER_WATCH_RETRY_INITIAL_MS,
            std::sync::atomic::Ordering::SeqCst,
        );
    }

    fn mark_watch_consumer_ready(&self) {
        *self.prepare_recovery_error.lock() = None;
        self.reset_watch_retry_delay();
    }

    fn ensure_browser_session_allowed(&self, session_id: &str) -> Result<(), String> {
        if let Some(error) = self.prepare_recovery_error.lock().clone() {
            if error.starts_with("browser/host-consumer-unavailable:") {
                return Err(error);
            }
            return Err(format!(
                "browser/prepare-recovery-pending: durable Prepare compensation is incomplete: {error}"
            ));
        }
        if self.pending_deleted_session_ids.read().contains(session_id) {
            return Err("task was deleted; rejecting a late browser host request".to_string());
        }
        let validator = self
            .session_validator
            .lock()
            .clone()
            .ok_or_else(|| "task lifecycle validator is not ready".to_string())?;
        if !validator(session_id) {
            return Err(
                "task does not exist; rejecting an orphan browser host request".to_string(),
            );
        }
        Ok(())
    }

    fn browser_session_is_deleted_or_absent(&self, session_id: &str) -> bool {
        if self.pending_deleted_session_ids.read().contains(session_id) {
            return true;
        }
        self.session_validator
            .lock()
            .as_ref()
            .is_some_and(|validator| !validator(session_id))
    }

    fn ensure_accepting_browser_work(&self) -> Result<(), String> {
        if self.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
            Err(
                "application is shutting down; browser operations are no longer accepted"
                    .to_string(),
            )
        } else {
            Ok(())
        }
    }

    /// Each renderer lifecycle first requests a host generation. It increases only
    /// within the Rust process and does not depend on JS state reset by HMR/crashes.
    pub fn begin_surface_generation(&self) -> u64 {
        self.surface_visibility.lock().begin_generation()
    }

    /// Attempts to show the platform-native WebView surface. False means the surface
    /// has not been created; the frontend reports/retries without changing display path.
    pub async fn show_native_surface(
        &self,
        window: &tauri::Window,
        session_id: &str,
        bounds: NativeSurfaceBounds,
        visibility_generation: u64,
        visibility_sequence: u64,
    ) -> Result<bool, String> {
        self.ensure_accepting_browser_work()?;
        let bounds = bounds.validate()?;
        let mut visibility = self.surface_visibility.lock();
        if !visibility.claim(visibility_generation, visibility_sequence) {
            return Ok(false);
        }
        // Keep the visibility lock through the native mutation commit so issuing a
        // new generation cannot interleave between claim and show.
        self.native_surface.lock().show(window, session_id, bounds)
    }

    pub fn hide_native_surface(
        &self,
        session_id: &str,
        visibility_generation: u64,
        visibility_sequence: u64,
    ) -> Result<(), String> {
        let mut visibility = self.surface_visibility.lock();
        if !visibility.claim(visibility_generation, visibility_sequence) {
            return Ok(());
        }
        let app = self.app.lock().clone();
        self.native_surface
            .lock()
            .hide(app.as_ref(), Some(session_id))
    }

    async fn prepare_requested_native_surfaces(&self, app: &AppHandle) -> Result<bool, String> {
        self.prepare_requested_native_surfaces_filtered(app, false)
            .await
    }

    async fn prepare_requested_native_control_requests(
        &self,
        app: &AppHandle,
    ) -> Result<bool, String> {
        self.prepare_requested_native_surfaces_filtered(app, true)
            .await
    }

    async fn prepare_requested_native_surfaces_filtered(
        &self,
        app: &AppHandle,
        control_only: bool,
    ) -> Result<bool, String> {
        let _hosted_request_guard = self.hosted_request_gate.read().await;
        if self.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(false);
        }
        let dir = paths::browser_host_requests_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(false);
        };
        let mut cancellation_paths = Vec::new();
        let mut request_paths = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            match path.extension().and_then(|value| value.to_str()) {
                Some("cancelled") => cancellation_paths.push(path),
                Some("json") => request_paths.push(path),
                _ => {}
            }
        }
        cancellation_paths.sort();
        request_paths.sort();

        let mut handled = false;
        let mut errors = Vec::new();
        let mut blocked_requests = HashSet::new();
        // Tombstones win over requests even when both artifacts were already
        // present when the watcher woke. This prevents a timed-out wrapper
        // call from creating a late workspace or tab.
        for cancellation_path in cancellation_paths {
            if control_only {
                continue;
            }
            // Requests with this stem cannot pass the cancellation marker in this
            // scan. Even if compensation fails, retain both artifacts for retry and
            // never execute the original create/close operation.
            blocked_requests.insert(cancellation_path.with_extension("json"));
            // A compensation failure retains the cancellation marker and ledger
            // record for explicit watcher retry. Removing them would turn transient
            // WebView/I/O failure into a permanent resource leak.
            if let Err(error) = self
                .process_hosted_cancellation(app, &cancellation_path)
                .await
            {
                errors.push(format!("{}: {error}", cancellation_path.display()));
            }
            handled = true;
        }

        for request_path in request_paths {
            if blocked_requests.contains(&request_path) {
                continue;
            }
            let raw = match std::fs::read_to_string(&request_path) {
                Ok(raw) => raw,
                Err(error) => {
                    eprintln!("[browser] failed to read browser host request: {error}");
                    let _ = std::fs::remove_file(&request_path);
                    continue;
                }
            };
            let request = match serde_json::from_str::<HostedBrowserRequest>(&raw) {
                Ok(request) => request,
                Err(error) => {
                    eprintln!("[browser] malformed browser host request: {error}");
                    let _ = std::fs::remove_file(&request_path);
                    continue;
                }
            };
            if control_only && !request.operation.is_control_plane() {
                continue;
            }
            if let Err(error) = validate_hosted_request(&request, &request_path) {
                eprintln!("[browser] invalid browser host request identity: {error}");
                let _ = std::fs::remove_file(&request_path);
                handled = true;
                continue;
            }
            if let Err(error) = self.ensure_browser_session_allowed(&request.session_id) {
                let response = hosted_response(&request, Err(error));
                if let Err(error) = write_hosted_response(&request_path, &response) {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
                let _ = std::fs::remove_file(&request_path);
                handled = true;
                continue;
            }

            // Claiming creates the in-memory idempotency generation. Re-read
            // the epoch immediately at that boundary instead of relying only
            // on the earlier artifact/schema validation.
            if let Err(error) = ensure_hosted_caller_live(&request) {
                let response = hosted_response(&request, Err(error));
                if let Err(error) = write_hosted_response(&request_path, &response) {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
                let _ = std::fs::remove_file(&request_path);
                handled = true;
                continue;
            }

            let claim = self
                .native_surface
                .lock()
                .claim_request(&request.session_id, &request.request_id);
            let claim = match claim {
                Ok(claim) => claim,
                Err(error) => {
                    let response = hosted_response(&request, Err(error));
                    if let Err(error) = write_hosted_response(&request_path, &response) {
                        errors.push(format!("{}: {error}", request_path.display()));
                        continue;
                    }
                    let _ = std::fs::remove_file(&request_path);
                    handled = true;
                    continue;
                }
            };
            match claim {
                NativeRequestClaim::Canceled => {
                    let _ = std::fs::remove_file(&request_path);
                    handled = true;
                    continue;
                }
                NativeRequestClaim::Replay(record) => {
                    if request_path.with_extension("cancelled").exists() {
                        if let Err(error) = self
                            .process_hosted_cancellation(
                                app,
                                &request_path.with_extension("cancelled"),
                            )
                            .await
                        {
                            errors.push(format!("{}: {error}", request_path.display()));
                            continue;
                        }
                    } else if let Some(response) = record.get("response") {
                        if let Err(error) = write_hosted_response(&request_path, response) {
                            errors.push(format!("{}: {error}", request_path.display()));
                            continue;
                        }
                    }
                    let _ = std::fs::remove_file(&request_path);
                    handled = true;
                    continue;
                }
                NativeRequestClaim::InFlight => {
                    // A dedicated control-plane scanner may observe the same
                    // artifact after the data-plane worker has claimed it (or
                    // vice versa). The claimant still owns the response path;
                    // never remove that shared artifact here.
                    continue;
                }
                NativeRequestClaim::Execute => {}
            }

            let cancellation_path = request_path.with_extension("cancelled");
            let cancelled_before_execution = if cancellation_path.exists() {
                if let Err(error) = self
                    .process_hosted_cancellation(app, &cancellation_path)
                    .await
                {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
                true
            } else {
                false
            };

            // A wrapper can cancel after this process has claimed the request
            // but while BrowserCore is resolving DOM state or waiting for an
            // already-dispatched native action. Revoke authorization as soon
            // as the tombstone wins, but keep polling the core future to its
            // bounded real settlement. Dropping the HTTP/WebKit future would
            // release the operation gate while the platform could still commit
            // the old action, allowing a newer mutation to overtake it.
            let mut core_cancellation_needs_compensation = false;
            let outcome = if cancelled_before_execution {
                core_cancellation_needs_compensation =
                    matches!(request.operation, HostedBrowserOperation::CoreTool);
                Err("browser/request-cancelled".to_string())
            } else if matches!(request.operation, HostedBrowserOperation::CoreTool) {
                let request_future = self.handle_hosted_browser_request(app, &request);
                tokio::pin!(request_future);
                let raced_outcome = tokio::select! {
                    biased;
                    _ = wait_for_hosted_cancellation(&cancellation_path) => None,
                    outcome = &mut request_future => Some(outcome),
                };
                match raced_outcome {
                    Some(outcome) => outcome,
                    None => {
                        core_cancellation_needs_compensation = true;
                        let revoke_error = self
                            .native_surface
                            .lock()
                            .cancel_in_flight_core_request(
                                Some(app),
                                &request.session_id,
                                &request.request_id,
                            )
                            .err();
                        // The platform request is bounded (including its own
                        // transport timeout). Its result is intentionally not
                        // published after cancellation; awaiting it only keeps
                        // mutation ordering and commit semantics truthful.
                        let _settled_outcome = request_future.await;
                        match revoke_error {
                            Some(error) => Err(format!(
                                "browser/request-cancelled; immediate compensation failed: {error}"
                            )),
                            None => Err("browser/request-cancelled".to_string()),
                        }
                    }
                }
            } else {
                self.handle_hosted_browser_request(app, &request).await
            };
            let (response, rollback) = match outcome {
                Ok(outcome) => {
                    let response = match outcome.error {
                        Some(error) => hosted_response(&request, Err(error)),
                        None => hosted_response(&request, Ok(outcome.result)),
                    };
                    (response, outcome.rollback)
                }
                Err(error) => {
                    let rollback = if core_cancellation_needs_compensation {
                        json!({
                            "kind": "cancelled_core_request",
                            "session_id": request.session_id,
                            "request_id": request.request_id,
                        })
                    } else {
                        json!({ "kind": "none" })
                    };
                    (hosted_response(&request, Err(error)), rollback)
                }
            };
            let record = json!({ "response": response, "rollback": rollback });

            // A timeout can arrive while prepare is waiting for WebView2/CDP.
            // Re-check immediately before committing the result to the ledger.
            if cancellation_path.exists() {
                if let Err(error) = self
                    .process_hosted_cancellation(app, &cancellation_path)
                    .await
                {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
            }
            let committed = match self.native_surface.lock().complete_request(
                &request.session_id,
                &request.request_id,
                record.clone(),
            ) {
                Ok(committed) => committed,
                Err(error) => {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
            };
            if !committed {
                if let Err(error) = self.rollback_hosted_record(app, &record).await {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
                if let Err(error) = self
                    .native_surface
                    .lock()
                    .acknowledge_request_cancellation(&request.session_id, &request.request_id)
                {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
                if matches!(request.operation, HostedBrowserOperation::Prepare) {
                    if let Err(error) = remove_matching_hosted_prepare_journal_for_request(&request)
                    {
                        errors.push(format!("{}: {error}", request_path.display()));
                        continue;
                    }
                    self.locally_committed_prepares
                        .lock()
                        .remove(&(request.session_token.clone(), request.request_id.clone()));
                }
                if let Err(error) = remove_hosted_request_artifacts(&cancellation_path) {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
                let _ = std::fs::remove_file(&request_path);
                handled = true;
                continue;
            }

            // Close the final race between ledger commit and response write.
            // The wrapper also quarantines late responses, so either side of
            // this boundary produces at most one observable outcome.
            if cancellation_path.exists() {
                if let Err(error) = self
                    .process_hosted_cancellation(app, &cancellation_path)
                    .await
                {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
            } else {
                if let Err(error) = write_hosted_response(&request_path, &record["response"]) {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
            }
            let _ = std::fs::remove_file(&request_path);
            handled = true;
        }
        if !control_only {
            if let Err(error) = reap_acknowledged_committed_prepare_journals(
                &mut self.locally_committed_prepares.lock(),
            ) {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            Ok(handled)
        } else {
            // The watcher will schedule another scan; other sessions still receive
            // fair service in this pass.
            Err(errors.join("; "))
        }
    }

    async fn process_hosted_cancellation(
        &self,
        app: &AppHandle,
        cancellation_path: &std::path::Path,
    ) -> Result<(), String> {
        let raw = std::fs::read_to_string(cancellation_path)
            .map_err(|error| format!("failed to read browser host cancellation record: {error}"))?;
        let cancellation: HostedBrowserCancellation = serde_json::from_str(&raw)
            .map_err(|error| format!("invalid browser host cancellation record: {error}"))?;
        validate_hosted_cancellation(&cancellation, cancellation_path)?;
        if self
            .discard_absent_session_cancellation(&cancellation, cancellation_path)
            .await?
        {
            return Ok(());
        }
        let disposition = self
            .native_surface
            .lock()
            .cancel_request(&cancellation.session_id, &cancellation.request_id)?;
        match disposition {
            NativeRequestCancel::AlreadyCompleted(record) => {
                self.rollback_hosted_record(app, &record).await?;
                self.native_surface
                    .lock()
                    .acknowledge_request_cancellation(
                        &cancellation.session_id,
                        &cancellation.request_id,
                    )?;
                self.remove_matching_hosted_prepare_journal_serialized(&cancellation)
                    .await?;
                self.locally_committed_prepares.lock().remove(&(
                    cancellation.session_token.clone(),
                    cancellation.request_id.clone(),
                ));
                remove_hosted_request_artifacts(cancellation_path)?;
            }
            // The request is still running in this serial consumer. Retain the
            // cancellation marker; after the executor commits its rollback record,
            // the `!committed` branch above compensates and acknowledges it.
            NativeRequestCancel::AwaitingCompletion => {}
            NativeRequestCancel::Tombstoned | NativeRequestCancel::AlreadyCanceled => {
                let _journal_match = self
                    .rollback_and_remove_matching_hosted_prepare_journal(app, &cancellation)
                    .await?;
                // An embedded compensation without the matching durable WAL is
                // not authority to mutate current task state. The WAL may have
                // been quarantined or superseded by a fresh generation; treat
                // that late cancellation as an acknowledged no-op.
                self.locally_committed_prepares.lock().remove(&(
                    cancellation.session_token.clone(),
                    cancellation.request_id.clone(),
                ));
                remove_hosted_request_artifacts(cancellation_path)?;
            }
        }
        Ok(())
    }

    async fn discard_absent_session_cancellation(
        &self,
        cancellation: &HostedBrowserCancellation,
        cancellation_path: &std::path::Path,
    ) -> Result<bool, String> {
        if !self.browser_session_is_deleted_or_absent(&cancellation.session_id) {
            return Ok(false);
        }
        // A deleted task's write-side teardown drains the scanner and owns
        // complete workspace cleanup. Do not create a new ledger tombstone from
        // a late wrapper cancellation; remove only its process-local protocol
        // artifacts. The same rule drops artifacts orphaned across startup when
        // the durable task validator reports no owner.
        self.remove_matching_hosted_prepare_journal_serialized(cancellation)
            .await?;
        self.locally_committed_prepares.lock().remove(&(
            cancellation.session_token.clone(),
            cancellation.request_id.clone(),
        ));
        remove_hosted_request_artifacts(cancellation_path)?;
        Ok(true)
    }

    async fn remove_matching_hosted_prepare_journal_serialized(
        &self,
        cancellation: &HostedBrowserCancellation,
    ) -> Result<(), String> {
        let session_lock = self.session_lifecycle_lock(&cancellation.session_id);
        let _session_guard = session_lock.lock().await;
        let _start_guard = self.start_mtx.lock().await;
        remove_matching_hosted_prepare_journal(cancellation)
    }

    async fn rollback_and_remove_matching_hosted_prepare_journal(
        &self,
        app: &AppHandle,
        cancellation: &HostedBrowserCancellation,
    ) -> Result<HostedPrepareJournalMatch, String> {
        let session_lock = self.session_lifecycle_lock(&cancellation.session_id);
        let _session_guard = session_lock.lock().await;
        let _start_guard = self.start_mtx.lock().await;
        let journal_match = classify_hosted_prepare_journal_for_cancellation(cancellation)?;
        let HostedPrepareJournalMatch::Matching(journal) = journal_match else {
            return Ok(journal_match);
        };
        self.rollback_prepare_journal_with_start_lock(app, &journal)
            .await?;
        remove_matching_hosted_prepare_journal(cancellation)?;
        Ok(HostedPrepareJournalMatch::Matching(journal))
    }

    async fn rollback_hosted_record(&self, app: &AppHandle, record: &Value) -> Result<(), String> {
        let rollback = &record["rollback"];
        let session_id = rollback
            .get("session_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match rollback.get("kind").and_then(Value::as_str) {
            Some(kind @ ("prepared_session" | "restored_session")) if !session_id.is_empty() => {
                let request_id = rollback
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let revision = rollback
                    .get("revision")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                if !request_id.is_empty() && revision > 0 {
                    self.rollback_prepared_session(
                        session_id,
                        request_id,
                        revision,
                        kind == "restored_session",
                    )
                    .await?;
                }
            }
            Some("created_tab") if !session_id.is_empty() => {
                let tab_token = rollback
                    .get("tab_token")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let creation_id = rollback
                    .get("creation_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !tab_token.is_empty() && !creation_id.is_empty() {
                    // False is a safe terminal state: user takeover or a later
                    // mutation invalidated the generation. Preserve the page but
                    // acknowledge the cancellation marker; only Err is retryable.
                    let _ = self.native_surface.lock().rollback_created_tab(
                        Some(app),
                        session_id,
                        tab_token,
                        creation_id,
                    )?;
                    self.persist_native_restore_best_effort(session_id);
                }
            }
            Some("cancelled_core_request") if !session_id.is_empty() => {
                let request_id = rollback
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !request_id.is_empty() {
                    self.native_surface.lock().cancel_in_flight_core_request(
                        Some(app),
                        session_id,
                        request_id,
                    )?;
                    self.persist_native_restore_best_effort(session_id);
                }
            }
            Some("agent_control") if !session_id.is_empty() => {
                let activated_tab = rollback
                    .get("activated_tab")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let previous_tab = rollback
                    .get("previous_tab")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let revision = rollback
                    .get("revision")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                let previous_owner = match rollback.get("previous_owner").and_then(Value::as_str) {
                    Some("unclaimed") => Some(platform::state::NativeControlOwner::Unclaimed),
                    Some("agent") => Some(platform::state::NativeControlOwner::Agent),
                    Some("user") => Some(platform::state::NativeControlOwner::User),
                    _ => None,
                };
                if !activated_tab.is_empty() && !previous_tab.is_empty() && revision > 0 {
                    if let Some(previous_owner) = previous_owner {
                        // false is the safe terminal state: later user/Agent work
                        // superseded this activation generation, so preserve it.
                        let _ = self.native_surface.lock().rollback_agent_activation(
                            Some(app),
                            session_id,
                            activated_tab,
                            previous_tab,
                            revision,
                            previous_owner,
                        )?;
                        self.persist_native_restore_best_effort(session_id);
                    }
                }
            }
            Some("agent_input") => {
                if let Ok(lease) = native_lease_from_value(rollback) {
                    self.native_surface.lock().end_agent_operation(&lease);
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_hosted_browser_request(
        &self,
        app: &AppHandle,
        request: &HostedBrowserRequest,
    ) -> Result<HostedBrowserOutcome, String> {
        self.ensure_browser_session_allowed(&request.session_id)?;
        match request.operation {
            HostedBrowserOperation::Prepare => {
                ensure_hosted_caller_live(request)?;
                let prepared = self
                    .prepare_native_workspace(
                        app,
                        &request.session_id,
                        &request.session_token,
                        true,
                        Some(&request.request_id),
                        Some(request),
                    )
                    .await;
                let (result, disposition) = match prepared {
                    Ok(prepared) => prepared,
                    Err(error) => match error.rollback {
                        Some(rollback) => {
                            return Ok(HostedBrowserOutcome::failed_with_rollback(
                                error.message,
                                rollback,
                            ));
                        }
                        None => return Err(error.message),
                    },
                };
                match disposition.rollback_kind() {
                    None => Ok(HostedBrowserOutcome::new(result)),
                    Some(kind) => {
                        let rollback_revision = self
                            .native_surface
                            .lock()
                            .prepare_generation_revision(&request.session_id, &request.request_id);
                        match rollback_revision {
                            Some(revision) => Ok(HostedBrowserOutcome::with_rollback(
                                result,
                                json!({
                                    "kind": kind,
                                    "session_id": request.session_id,
                                    "request_id": request.request_id,
                                    "revision": revision,
                                }),
                            )),
                            None => Ok(HostedBrowserOutcome::new(result)),
                        }
                    }
                }
            }
            HostedBrowserOperation::CreateTab => {
                let tab_token = request
                    .tab_token
                    .as_deref()
                    .ok_or_else(|| "create-tab request is missing tab_token".to_string())?;
                let requested_url = request.url.as_deref().unwrap_or("about:blank");
                if !is_allowed_url(requested_url) {
                    return Err("only http, https, and about:blank URLs are supported".to_string());
                }
                let authorization = native_mutation_lease_from_request(request)?;
                // First create a WebView2 target with a unique marker. After the host
                // discovers and binds it, navigate the hidden surface to requested_url,
                // then publish through lease CAS to avoid popups and wrong-tab navigation.
                let marker = format!("about:blank#pinvou-tab-{tab_token}");
                ensure_hosted_caller_live(request)?;
                let created = self.native_surface.lock().create_tab_for_agent(
                    app,
                    &request.session_id,
                    tab_token,
                    &marker,
                    request.background,
                    &authorization,
                    &request.request_id,
                )?;
                if created.is_none() {
                    return Err(
                        "current session is not an automatable native browser workspace"
                            .to_string(),
                    );
                }
                if let Err(error) = ensure_hosted_caller_live(request) {
                    return match self.rollback_staged_agent_tab(
                        app,
                        &request.session_id,
                        tab_token,
                        &request.request_id,
                    ) {
                        Ok(()) => Err(error),
                        Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
                    };
                }
                let target_id = match self
                    .bind_staged_native_target(app, &request.session_id, tab_token)
                    .await
                {
                    Ok(target_id) => target_id,
                    Err(error) => {
                        return match self.rollback_staged_agent_tab(
                            app,
                            &request.session_id,
                            tab_token,
                            &request.request_id,
                        ) {
                            Ok(()) => Err(error),
                            Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
                        };
                    }
                };
                if let Err(error) = ensure_hosted_caller_live(request) {
                    return match self.rollback_staged_agent_tab(
                        app,
                        &request.session_id,
                        tab_token,
                        &request.request_id,
                    ) {
                        Ok(()) => Err(error),
                        Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
                    };
                }
                if !self.native_surface.lock().commit_created_tab_for_agent(
                    app,
                    &request.session_id,
                    tab_token,
                    &target_id,
                    requested_url,
                    request.background,
                    &authorization,
                    &request.request_id,
                    None,
                    || ensure_hosted_caller_live(request),
                )? {
                    return Err("new tab was closed before commit".to_string());
                }
                let _ = app.emit(
                    "browser:tabs-changed",
                    json!({ "sessionId": request.session_id, "tab": tab_token }),
                );
                self.persist_native_restore_best_effort(&request.session_id);
                let result = json!({
                    "tabToken": tab_token,
                    "targetId": target_id,
                    "creationId": request.request_id,
                });
                Ok(HostedBrowserOutcome::with_rollback(
                    result,
                    json!({
                        "kind": "created_tab",
                        "session_id": request.session_id,
                        "tab_token": tab_token,
                        "creation_id": request.request_id,
                    }),
                ))
            }
            HostedBrowserOperation::ActivateTab => {
                let tab_token = request
                    .tab_token
                    .as_deref()
                    .ok_or_else(|| "activate-tab request is missing tab_token".to_string())?;
                ensure_hosted_caller_live(request)?;
                let (previous_tab, previous_control, lease) = {
                    let mut surface = self.native_surface.lock();
                    let previous_tab =
                        surface
                            .active_tab_token(&request.session_id)
                            .ok_or_else(|| {
                                "current session is not an automatable native browser workspace"
                                    .to_string()
                            })?;
                    let previous_control =
                        surface.control_state(&request.session_id).ok_or_else(|| {
                            "current session is not an automatable native browser workspace"
                                .to_string()
                        })?;
                    let Some(lease) = surface.activate_tab_with_lease(
                        Some(app),
                        &request.session_id,
                        tab_token,
                    )?
                    else {
                        return Err(
                            "current session is not an automatable native browser workspace"
                                .to_string(),
                        );
                    };
                    (previous_tab, previous_control, lease)
                };
                let _ = app.emit(
                    "browser:tabs-changed",
                    json!({ "sessionId": request.session_id, "tab": tab_token }),
                );
                self.persist_native_restore_best_effort(&request.session_id);
                let result = lease.to_json_value()?;
                Ok(HostedBrowserOutcome::with_rollback(
                    result,
                    json!({
                        "kind": "agent_control",
                        "session_id": request.session_id,
                        "activated_tab": tab_token,
                        "previous_tab": previous_tab,
                        "previous_owner": previous_control.owner.as_str(),
                        "revision": lease.revision,
                    }),
                ))
            }
            HostedBrowserOperation::CloseTab => {
                let tab_token = request
                    .tab_token
                    .as_deref()
                    .ok_or_else(|| "close-tab request is missing tab_token".to_string())?;
                let authorization = native_mutation_lease_from_request(request)?;
                if authorization.tab_token != tab_token {
                    return Err(
                        "close-tab authorization_tab_token must match the target tab".to_string(),
                    );
                }
                ensure_hosted_caller_live(request)?;
                if !self.native_surface.lock().close_tab_for_agent(
                    Some(app),
                    &request.session_id,
                    tab_token,
                    &authorization,
                )? {
                    return Err(
                        "current session is not an automatable native browser workspace"
                            .to_string(),
                    );
                }
                let _ = app.emit(
                    "browser:tabs-changed",
                    json!({ "sessionId": request.session_id }),
                );
                self.persist_native_restore_best_effort(&request.session_id);
                Ok(HostedBrowserOutcome::new(json!({})))
            }
            HostedBrowserOperation::RollbackCreatedTab => {
                let tab_token = request.tab_token.as_deref().ok_or_else(|| {
                    "create compensation request is missing tab_token".to_string()
                })?;
                let creation_id = request.creation_id.as_deref().ok_or_else(|| {
                    "create compensation request is missing creation_id".to_string()
                })?;
                if !valid_host_request_id(creation_id) {
                    return Err("create compensation generation is invalid".to_string());
                }
                if !self.native_surface.lock().rollback_created_tab(
                    Some(app),
                    &request.session_id,
                    tab_token,
                    creation_id,
                )? {
                    return Err("tab pending create compensation does not exist".to_string());
                }
                let _ = app.emit(
                    "browser:tabs-changed",
                    json!({ "sessionId": request.session_id }),
                );
                self.persist_native_restore_best_effort(&request.session_id);
                Ok(HostedBrowserOutcome::new(json!({})))
            }
            HostedBrowserOperation::AssertHostLease => {
                let lease = native_lease_from_request(request)?;
                ensure_hosted_caller_live(request)?;
                if !self.native_surface.lock().assert_lease(&lease)? {
                    return Err(
                        "browser host lease expired; the user may have taken over the page"
                            .to_string(),
                    );
                }
                Ok(HostedBrowserOutcome::new(json!({})))
            }
            HostedBrowserOperation::BeginAgentOperation => {
                let lease = native_lease_from_request(request)?;
                ensure_hosted_caller_live(request)?;
                if !self.native_surface.lock().begin_agent_operation(
                    &lease,
                    request.emits_trusted_input,
                    request.observational_only,
                    request.caller_pid,
                    &request.wrapper_instance_nonce,
                )? {
                    return Err(
                        "browser host lease expired; tool execution was blocked".to_string()
                    );
                }
                Ok(HostedBrowserOutcome::with_rollback(
                    json!({}),
                    json!({
                        "kind": "agent_input",
                        "session_id": lease.session_id,
                        "tab_token": lease.tab_token,
                        "target_id": lease.target_id,
                        "revision": lease.revision,
                        "lease": lease.lease,
                    }),
                ))
            }
            HostedBrowserOperation::RefreshAgentInput => {
                let lease = native_lease_from_request(request)?;
                ensure_hosted_caller_live(request)?;
                if !self.native_surface.lock().refresh_agent_input(&lease)? {
                    return Err(
                        "browser/agent-input-refresh-rejected: tool operation ended or its lease expired"
                            .to_string(),
                    );
                }
                Ok(HostedBrowserOutcome::new(json!({})))
            }
            HostedBrowserOperation::RefreshAgentOperation => {
                let lease = native_lease_from_request(request)?;
                ensure_hosted_caller_live(request)?;
                if !self.native_surface.lock().refresh_agent_operation(&lease)? {
                    return Err(
                        "browser/agent-operation-refresh-rejected: tool operation ended or its lease expired"
                            .to_string(),
                    );
                }
                Ok(HostedBrowserOutcome::new(json!({})))
            }
            HostedBrowserOperation::EndAgentOperation => {
                let lease = native_lease_from_request(request)?;
                self.native_surface.lock().end_agent_operation(&lease);
                Ok(HostedBrowserOutcome::new(json!({})))
            }
            HostedBrowserOperation::CoreTool => self.handle_browser_core_tool(app, request).await,
        }
    }

    async fn handle_browser_core_tool(
        &self,
        app: &AppHandle,
        request: &HostedBrowserRequest,
    ) -> Result<HostedBrowserOutcome, String> {
        ensure_hosted_caller_live(request)?;
        if !platform::browser_core_available() {
            return Err("browser/core-backend-unavailable".to_string());
        }
        let tool_name = request
            .tool_name
            .as_deref()
            .ok_or_else(|| "browser/missing-tool-name".to_string())?;
        let arguments = request
            .tool_arguments
            .as_ref()
            .cloned()
            .unwrap_or_else(|| json!({}));
        let arguments = arguments
            .as_object()
            .ok_or_else(|| "browser/tool-arguments-must-be-object".to_string())?;
        let arguments = Value::Object(arguments.clone());

        if tool_name == "list_pages" {
            ensure_hosted_caller_live(request)?;
            let surface = self.native_surface.lock();
            let tabs = surface
                .list_tabs(Some(app), &request.session_id)
                .ok_or_else(|| "browser/workspace-unavailable".to_string())?;
            let active = surface
                .active_tab_token(&request.session_id)
                .ok_or_else(|| "browser/workspace-unavailable".to_string())?;
            let pages = tabs
                .iter()
                .map(|tab| {
                    let page_id = tab
                        .page_id
                        .ok_or_else(|| "browser/page-id-unavailable".to_string())?;
                    Ok(json!({
                        "id": page_id,
                        "url": tab.url,
                        "title": tab.title,
                        "selected": tab.target_id == active,
                    }))
                })
                .collect::<Result<Vec<_>, String>>()?;
            let text = pages
                .iter()
                .map(|page| {
                    format!(
                        "{}: {}{}",
                        page["id"].as_u64().unwrap_or_default(),
                        page["url"].as_str().unwrap_or_default(),
                        if page["selected"].as_bool().unwrap_or(false) {
                            " [selected]"
                        } else {
                            ""
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            return Ok(HostedBrowserOutcome::new(browser_core_tool_result(
                if text.is_empty() {
                    "No pages".to_string()
                } else {
                    text
                },
                Some(json!({ "pages": pages })),
            )));
        }

        let tab_token_for_page = |page_id: u64| -> Result<String, String> {
            let surface = self.native_surface.lock();
            surface
                .tab_token_for_page_id(&request.session_id, page_id)
                .ok_or_else(|| "browser/page-not-found".to_string())
        };

        if tool_name == "new_page" {
            let requested_url = arguments
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| "browser/missing-argument: url".to_string())?;
            if !is_allowed_url(requested_url) {
                return Err("browser/url-not-allowed".to_string());
            }
            let background = arguments
                .get("background")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let authorization_tab = self
                .native_surface
                .lock()
                .active_tab_token(&request.session_id)
                .ok_or_else(|| "browser/workspace-unavailable".to_string())?;
            let reuse_initial_blank = {
                let surface = self.native_surface.lock();
                let tabs = surface
                    .list_tabs(Some(app), &request.session_id)
                    .ok_or_else(|| "browser/workspace-unavailable".to_string())?;
                should_reuse_browser_core_initial_tab(
                    &request.session_id,
                    &authorization_tab,
                    &tabs,
                    background,
                )
            };
            ensure_hosted_caller_live(request)?;
            let authorization = self
                .native_surface
                .lock()
                .activate_tab_with_lease(Some(app), &request.session_id, &authorization_tab)?
                .ok_or_else(|| "browser/workspace-unavailable".to_string())?;
            let tab_token = self.native_surface.lock().generate_tab_token();
            if reuse_initial_blank {
                ensure_hosted_caller_live(request)?;
                if !self.native_surface.lock().begin_agent_operation(
                    &authorization,
                    false,
                    false,
                    request.caller_pid,
                    &request.wrapper_instance_nonce,
                )? {
                    return Err("browser/control-lease-lost".to_string());
                }
                if let Err(error) = ensure_hosted_caller_live(request) {
                    self.native_surface
                        .lock()
                        .end_agent_operation(&authorization);
                    return Err(error);
                }
                let navigation_result = (|| {
                    self.native_surface
                        .lock()
                        .navigate_tab_for_agent(
                            app,
                            &request.session_id,
                            &authorization_tab,
                            requested_url,
                            &authorization,
                        )?
                        .then_some(())
                        .ok_or_else(|| "browser/native-surface-missing".to_string())
                })();
                self.native_surface
                    .lock()
                    .end_agent_operation(&authorization);
                if let Err(error) = navigation_result {
                    return match core::committed_platform_outcome("Navigation", &error) {
                        Some(outcome) => Ok(HostedBrowserOutcome::new(outcome)),
                        None => Err(error),
                    };
                }
                let _ = app.emit(
                    "browser:tabs-changed",
                    json!({ "sessionId": request.session_id, "tab": authorization_tab }),
                );
                self.persist_native_restore_best_effort(&request.session_id);
                return Ok(HostedBrowserOutcome::new(browser_core_tool_result(
                    format!(
                        "Navigation dispatched to {requested_url}; page load is not verified. Call take_snapshot or list_pages to verify."
                    ),
                    Some(json!({
                        "tabToken": authorization_tab,
                        "targetId": format!("native:{authorization_tab}"),
                        "reusedInitialBlank": true,
                        "navigationDispatched": true,
                        "loadVerified": false,
                    })),
                )));
            }

            ensure_hosted_caller_live(request)?;
            if !self.native_surface.lock().begin_agent_operation(
                &authorization,
                false,
                false,
                request.caller_pid,
                &request.wrapper_instance_nonce,
            )? {
                return Err("browser/control-lease-lost".to_string());
            }
            let creation_result = async {
                ensure_hosted_caller_live(request)?;
                self.native_surface.lock().create_tab_for_agent(
                    app,
                    &request.session_id,
                    &tab_token,
                    "about:blank",
                    background,
                    &authorization,
                    &request.request_id,
                )?;
                if let Err(error) = ensure_hosted_caller_live(request) {
                    return match self.rollback_staged_agent_tab(
                        app,
                        &request.session_id,
                        &tab_token,
                        &request.request_id,
                    ) {
                        Ok(()) => Err(error),
                        Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
                    };
                }
                let target_id = match self
                    .bind_staged_native_target(app, &request.session_id, &tab_token)
                    .await
                {
                    Ok(target_id) => target_id,
                    Err(error) => {
                        return match self.rollback_staged_agent_tab(
                            app,
                            &request.session_id,
                            &tab_token,
                            &request.request_id,
                        ) {
                            Ok(()) => Err(error),
                            Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
                        };
                    }
                };
                if let Err(error) = ensure_hosted_caller_live(request) {
                    return match self.rollback_staged_agent_tab(
                        app,
                        &request.session_id,
                        &tab_token,
                        &request.request_id,
                    ) {
                        Ok(()) => Err(error),
                        Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
                    };
                }
                if !self.native_surface.lock().commit_created_tab_for_agent(
                    app,
                    &request.session_id,
                    &tab_token,
                    &target_id,
                    requested_url,
                    background,
                    &authorization,
                    &request.request_id,
                    None,
                    || ensure_hosted_caller_live(request),
                )? {
                    return Err("browser/create-tab-cancelled".to_string());
                }
                let _ = app.emit(
                    "browser:tabs-changed",
                    json!({ "sessionId": request.session_id, "tab": tab_token }),
                );
                self.persist_native_restore_best_effort(&request.session_id);
                Ok(HostedBrowserOutcome::with_rollback(
                    browser_core_tool_result(
                        format!(
                            "Navigation dispatched to {requested_url}; page load is not verified. Call take_snapshot or list_pages to verify."
                        ),
                        Some(json!({
                            "tabToken": tab_token,
                            "targetId": target_id,
                            "navigationDispatched": true,
                            "loadVerified": false,
                        })),
                    ),
                    json!({
                        "kind": "created_tab",
                        "session_id": request.session_id,
                        "tab_token": tab_token,
                        "creation_id": request.request_id,
                    }),
                ))
            }
            .await;
            self.native_surface
                .lock()
                .end_agent_operation(&authorization);
            return creation_result;
        }

        let page_id_value = arguments
            .get("pageId")
            .ok_or_else(|| "browser/missing-argument: pageId".to_string())?;
        let page_id = page_id_value
            .as_u64()
            .filter(|page_id| *page_id <= (1_u64 << 53) - 1)
            .ok_or_else(|| "browser/invalid-argument: pageId".to_string())?;
        let tab_token = tab_token_for_page(page_id)?;

        if tool_name == "select_page" {
            ensure_hosted_caller_live(request)?;
            self.native_surface
                .lock()
                .activate_tab_with_lease(Some(app), &request.session_id, &tab_token)?
                .ok_or_else(|| "browser/page-not-found".to_string())?;
            let _ = app.emit(
                "browser:tabs-changed",
                json!({ "sessionId": request.session_id, "tab": tab_token }),
            );
            return Ok(HostedBrowserOutcome::new(browser_core_tool_result(
                "Selected page".to_string(),
                None,
            )));
        }

        ensure_hosted_caller_live(request)?;
        let lease = self
            .native_surface
            .lock()
            .activate_tab_with_lease(Some(app), &request.session_id, &tab_token)?
            .ok_or_else(|| "browser/page-not-found".to_string())?;

        if tool_name == "close_page" {
            ensure_hosted_caller_live(request)?;
            if !self.native_surface.lock().begin_agent_operation(
                &lease,
                false,
                false,
                request.caller_pid,
                &request.wrapper_instance_nonce,
            )? {
                return Err("browser/control-lease-lost".to_string());
            }
            if let Err(error) = ensure_hosted_caller_live(request) {
                self.native_surface.lock().end_agent_operation(&lease);
                return Err(error);
            }
            let close_result = self.native_surface.lock().close_tab_for_agent(
                Some(app),
                &request.session_id,
                &tab_token,
                &lease,
            );
            self.native_surface.lock().end_agent_operation(&lease);
            let closed = match close_result {
                Ok(closed) => closed,
                Err(error) => {
                    if let Some(outcome) = core::committed_platform_outcome("Close page", &error) {
                        return Ok(HostedBrowserOutcome::new(outcome));
                    }
                    return Err(error);
                }
            };
            if !closed {
                return Err("browser/page-not-found".to_string());
            }
            let _ = app.emit(
                "browser:tabs-changed",
                json!({ "sessionId": request.session_id }),
            );
            self.persist_native_restore_best_effort(&request.session_id);
            return Ok(HostedBrowserOutcome::new(browser_core_tool_result(
                "Closed page".to_string(),
                None,
            )));
        }

        // BrowserCore adapters revalidate the original lease at the native
        // dispatch boundary. Mouse/key dispatch opens the short provenance
        // window there; DOM resolution must not suppress real user takeover.
        ensure_hosted_caller_live(request)?;
        if !self.native_surface.lock().begin_agent_operation(
            &lease,
            false,
            browser_core_tool_is_observational(tool_name),
            request.caller_pid,
            &request.wrapper_instance_nonce,
        )? {
            return Err("browser/control-lease-lost".to_string());
        }

        let result = async {
            ensure_hosted_caller_live(request)?;
            let label = self
                .native_surface
                .lock()
                .webview_label_for_tab(&request.session_id, &tab_token)
                .ok_or_else(|| "browser/page-not-found".to_string())?;
            let webview = app
                .get_webview(&label)
                .ok_or_else(|| "browser/native-surface-missing".to_string())?;

            if tool_name == "navigate_page" {
                let navigation_type = arguments
                    .get("type")
                    .and_then(Value::as_str)
                    .or_else(|| arguments.get("url").map(|_| "url"))
                    .ok_or_else(|| "browser/missing-argument: type".to_string())?;
                match navigation_type {
                    "url" => {
                        let url = arguments
                            .get("url")
                            .and_then(Value::as_str)
                            .ok_or_else(|| "browser/missing-argument: url".to_string())?;
                        if !is_allowed_url(url) {
                            return Err("browser/url-not-allowed".to_string());
                        }
                        ensure_hosted_caller_live(request)?;
                        self.native_surface
                            .lock()
                            .navigate_tab_for_agent(
                                app,
                                &request.session_id,
                                &tab_token,
                                url,
                                &lease,
                            )?
                            .then_some(())
                            .ok_or_else(|| "browser/native-surface-missing".to_string())?;
                    }
                    "back" => {
                        ensure_hosted_caller_live(request)?;
                        self.native_surface
                            .lock()
                            .history_step_tab_for_agent(
                                app,
                                &request.session_id,
                                &tab_token,
                                -1,
                                &lease,
                            )?
                            .then_some(())
                            .ok_or_else(|| "browser/native-surface-missing".to_string())?
                    }
                    "forward" => {
                        ensure_hosted_caller_live(request)?;
                        self.native_surface
                            .lock()
                            .history_step_tab_for_agent(
                                app,
                                &request.session_id,
                                &tab_token,
                                1,
                                &lease,
                            )?
                            .then_some(())
                            .ok_or_else(|| "browser/native-surface-missing".to_string())?
                    }
                    "reload" => {
                        ensure_hosted_caller_live(request)?;
                        self.native_surface
                            .lock()
                            .reload_tab_for_agent(
                                app,
                                &request.session_id,
                                &tab_token,
                                &lease,
                            )?
                            .then_some(())
                            .ok_or_else(|| "browser/native-surface-missing".to_string())?
                    }
                    _ => return Err("browser/invalid-navigation-type".to_string()),
                }
                Ok(browser_core_tool_result(
                    format!(
                        "Navigation command dispatched ({navigation_type}); page load is not verified. Call take_snapshot or list_pages to verify."
                    ),
                    Some(json!({
                        "navigationType": navigation_type,
                        "navigationDispatched": true,
                        "loadVerified": false,
                    })),
                ))
            } else {
                ensure_hosted_caller_live(request)?;
                core::execute_page_tool(&webview, &lease, tool_name, &arguments).await
            }
        }
        .await;
        self.native_surface.lock().end_agent_operation(&lease);
        match result {
            Ok(result) => Ok(HostedBrowserOutcome::new(result)),
            Err(error) => match core::committed_platform_outcome("Navigation", &error) {
                Some(outcome) => Ok(HostedBrowserOutcome::new(outcome)),
                None => Err(error),
            },
        }
    }

    /// Caller holds `start_mtx`. A single host-owned journal is allowed per
    /// session. Resolve an older generation before creating a new intent so an
    /// old CreatedBlank rollback can never race a newer committed manifest.
    async fn begin_hosted_prepare_with_start_lock(
        &self,
        app: &AppHandle,
        session_id: &str,
        session_token: &str,
        hosted_request: Option<&HostedBrowserRequest>,
    ) -> Result<(), PrepareWorkspaceError> {
        if let Some(request) = hosted_request {
            if !matches!(request.operation, HostedBrowserOperation::Prepare)
                || request.session_id != session_id
                || request.session_token != session_token
            {
                return Err("browser Prepare journal request identity mismatch"
                    .to_string()
                    .into());
            }
        }

        let journal_path = hosted_prepare_journal_path_for(session_token);
        let mut superseded_commit = None;
        if let Some(journal) = read_hosted_prepare_journal_if_present(&journal_path)? {
            if journal.compensation.session_id != session_id {
                return Err(
                    "browser Prepare journal does not belong to the current task"
                        .to_string()
                        .into(),
                );
            }
            let cancellation_wins =
                matching_hosted_cancellation_for_compensation(&journal.compensation)?;
            if journal.phase == HostedPreparePhase::Committed && !cancellation_wins {
                if let Some(request) = hosted_request {
                    let same_request = journal.compensation.request_id == request.request_id
                        && journal.compensation.idempotency_key == request.idempotency_key
                        && journal.compensation.caller_pid == request.caller_pid
                        && journal.compensation.wrapper_instance_nonce
                            == request.wrapper_instance_nonce;
                    if same_request {
                        return Err(
                            "browser/prepare-acknowledgement-pending: identical committed Prepare is still awaiting caller settlement"
                                .to_string()
                                .into(),
                        );
                    }
                    // A distinct Prepare for the same task explicitly adopts
                    // the committed workspace. Its Pending record atomically
                    // replaces the old WAL below, so there is no record-free
                    // crash window in which the old manifest can be orphaned.
                    superseded_commit = Some((
                        journal.compensation.session_token.clone(),
                        journal.compensation.request_id.clone(),
                    ));
                } else {
                    // A direct in-app Prepare is also an explicit adoption, but
                    // has no wrapper generation that requires a replacement WAL.
                    remove_hosted_prepare_journal(&journal_path)?;
                    self.locally_committed_prepares.lock().remove(&(
                        journal.compensation.session_token.clone(),
                        journal.compensation.request_id.clone(),
                    ));
                }
            } else {
                self.rollback_prepare_journal_with_start_lock(app, &journal)
                    .await?;
                remove_hosted_prepare_journal(&journal_path)?;
                self.locally_committed_prepares.lock().remove(&(
                    journal.compensation.session_token.clone(),
                    journal.compensation.request_id.clone(),
                ));
            }
        }

        let Some(request) = hosted_request else {
            return Ok(());
        };
        let had_session = self.native_surface.lock().has_session(session_id);
        let rollback_kind = if had_session {
            "none"
        } else if paths::browser_workspace_restore_json(session_token).exists() {
            "restored_session"
        } else {
            "prepared_session"
        };
        let journal = new_hosted_prepare_journal(request, rollback_kind, hosted_protocol_now_ms()?);
        write_hosted_prepare_journal(&journal)?;
        if let Some(old_key) = superseded_commit {
            self.locally_committed_prepares.lock().remove(&old_key);
        }
        Ok(())
    }

    /// Caller holds `start_mtx`. Pending journals without a revision represent
    /// a crash during restore/build; no later generation can exist because a
    /// new prepare cannot pass this gate until cleanup succeeds.
    async fn rollback_prepare_journal_with_start_lock(
        &self,
        app: &AppHandle,
        journal: &HostedPrepareJournal,
    ) -> Result<(), String> {
        let compensation = &journal.compensation;
        let mut rollback_applied = true;
        match (compensation.rollback_kind.as_str(), compensation.revision) {
            ("prepared_session", Some(revision)) => {
                rollback_applied = self
                    .rollback_prepared_session_with_start_lock(
                        &compensation.session_id,
                        &compensation.request_id,
                        revision,
                        false,
                    )
                    .await?;
            }
            ("restored_session", Some(revision)) => {
                rollback_applied = self
                    .rollback_prepared_session_with_start_lock(
                        &compensation.session_id,
                        &compensation.request_id,
                        revision,
                        true,
                    )
                    .await?;
            }
            ("prepared_session", None) => {
                let has_remaining = self
                    .native_surface
                    .lock()
                    .close_session(Some(app), &compensation.session_id)?;
                if !has_remaining {
                    self.stop_with_start_lock().await?;
                }
            }
            ("restored_session", None) => {
                let has_remaining = self
                    .native_surface
                    .lock()
                    .close_session_preserving_restore(Some(app), &compensation.session_id)?;
                if !has_remaining {
                    self.stop_with_start_lock().await?;
                }
            }
            ("none", None) => {}
            _ => {
                return Err("browser Prepare journal compensation generation is invalid".to_string())
            }
        }
        // A revision mismatch means newer task state superseded this Prepare.
        // Retire the old WAL but preserve the newer durable manifest.
        if should_remove_prepare_restore(&compensation.rollback_kind, rollback_applied) {
            remove_private_plain_file_and_verify_absent(
                &paths::browser_workspace_restore_json(&compensation.session_token),
                "remove uncommitted Prepare restore manifest",
            )?;
        }
        Ok(())
    }

    async fn prepare_native_workspace(
        &self,
        app: &AppHandle,
        session_id: &str,
        session_token: &str,
        agent_initiated: bool,
        prepare_request_id: Option<&str>,
        hosted_request: Option<&HostedBrowserRequest>,
    ) -> Result<(Value, PreparedWorkspaceDisposition), PrepareWorkspaceError> {
        if !crate::platform::capabilities::browser_product_enabled() {
            return Err("in-app browser is not enabled in this product build"
                .to_string()
                .into());
        }
        // Restore and prepare form one per-task lifecycle transaction. The
        // global start lock is released while waiting for automation binding,
        // allowing an unrelated task workspace to make progress.
        let session_lock = self.session_lifecycle_lock(session_id);
        let _session_guard = session_lock.lock().await;
        let start_guard = self.start_mtx.lock().await;
        // A direct UI Prepare may have started waiting before restart closed
        // admission. Recheck after the shared lifecycle gate so it cannot
        // recreate a WebView during child harvesting.
        self.ensure_accepting_browser_work()?;
        self.ensure_browser_session_allowed(session_id)?;
        self.begin_hosted_prepare_with_start_lock(app, session_id, session_token, hosted_request)
            .await?;
        let prepared = async {
            if platform::browser_core_available() {
                let restore_outcome = if !self.native_surface.lock().has_session(session_id) {
                    self.restore_saved_workspace_releasing_start_lock(session_id, start_guard)
                        .await?
                } else {
                    drop(start_guard);
                    RestoreWorkspaceOutcome::Existing
                };
                let _start_guard = self.start_mtx.lock().await;
                self.ensure_accepting_browser_work()?;
                self.ensure_browser_session_allowed(session_id)?;
                return self
                    .prepare_browser_core_workspace_with_start_lock(
                        app,
                        session_id,
                        session_token,
                        restore_outcome,
                        prepare_request_id,
                        hosted_request,
                    )
                    .await;
            }

            // The wrapper can arrive before the UI status query after an app
            // restart. Restore the URL manifest first so an ordinary Prepare
            // cannot replace a saved multi-tab workspace with a blank page.
            let restore_outcome = if !self.native_surface.lock().has_session(session_id) {
                self.restore_saved_workspace_releasing_start_lock(session_id, start_guard)
                    .await?
            } else {
                drop(start_guard);
                RestoreWorkspaceOutcome::Existing
            };
            let _start_guard = self.start_mtx.lock().await;
            self.ensure_accepting_browser_work()?;
            self.ensure_browser_session_allowed(session_id)?;
            let (had_session, had_sessions) = {
                let surface = self.native_surface.lock();
                (surface.has_session(session_id), surface.has_sessions())
            };
            let existing_port = live_port().await;
            if had_sessions {
                let port = existing_port.ok_or_else(|| {
                    PrepareWorkspaceError::from(
                        "native browser workspaces are running but automation endpoint state is missing; existing tasks were preserved, retry the operation"
                            .to_string(),
                    )
                })?;
                if !self.native_surface.lock().owns_port(port) {
                    return Err("automation endpoint does not match the existing native browser workspace".to_string().into());
                }
            } else if let Some(port) = existing_port {
                if !self.native_surface.lock().owns_port(port) {
                    return Err("detected an automation endpoint not owned by the current native browser workspace"
                        .to_string()
                        .into());
                }
            }
            let port = match existing_port {
                Some(port) => port,
                None => pick_free_port().await?,
            };
            let profile = paths::browser_webview_profile_dir();
            let prepared = if agent_initiated {
                self.native_surface.lock().prepare(
                    app,
                    session_id,
                    session_token,
                    port,
                    &profile,
                )?
            } else {
                self.native_surface.lock().prepare_unclaimed(
                    app,
                    session_id,
                    session_token,
                    port,
                    &profile,
                )?
            };
            if !prepared {
                return Err("native browser surfaces are unsupported on this platform".to_string().into());
            }
            if existing_port.is_none() {
                if !probe_cdp(port, Duration::from_secs(15)).await {
                    self.rollback_new_native_workspace(app, session_id, had_session);
                    return Err("WebView2 was created but CDP did not become ready".to_string().into());
                }
                if let Err(error) = write_port_file(port, "app", None) {
                    self.rollback_new_native_workspace(app, session_id, had_session);
                    return Err(error.into());
                }
            }
            let unbound_tabs = self.native_surface.lock().unbound_tabs(session_id);
            for tab_token in unbound_tabs {
                let target_id = match discover_native_target(port, &tab_token).await {
                    Ok(target_id) => target_id,
                    Err(error) => {
                        self.rollback_new_native_workspace(app, session_id, had_session);
                        return Err(error.into());
                    }
                };
                if let Err(error) = self
                    .native_surface
                    .lock()
                    .bind_target(session_id, &tab_token, &target_id)
                {
                    self.rollback_new_native_workspace(app, session_id, had_session);
                    return Err(error.into());
                }
            }
            let disposition = match restore_outcome {
                RestoreWorkspaceOutcome::Restored => PreparedWorkspaceDisposition::RestoredExisting,
                RestoreWorkspaceOutcome::Existing => PreparedWorkspaceDisposition::Existing,
                RestoreWorkspaceOutcome::Missing if had_session => {
                    // Another prepare won while this task waited for its
                    // lifecycle lock.
                    PreparedWorkspaceDisposition::Existing
                }
                RestoreWorkspaceOutcome::Missing => PreparedWorkspaceDisposition::CreatedBlank,
            };
            let result = json!({ "sessionId": session_id });
            self.publish_prepared_workspace_with_start_lock(
                app,
                session_id,
                disposition,
                prepare_request_id,
                hosted_request,
                &result,
            )
            .await?;
            Ok((result, disposition))
        }
        .await;

        match prepared {
            Ok(prepared) => Ok(prepared),
            Err(mut error) => {
                // The restore helper intentionally released the global lock
                // before automation waits. Reacquire it for durable rollback
                // and shared-runtime compensation.
                let _cleanup_start_guard = self.start_mtx.lock().await;
                if hosted_request.is_some() {
                    let journal_path = hosted_prepare_journal_path_for(session_token);
                    match read_hosted_prepare_journal_if_present(&journal_path) {
                        Ok(Some(journal)) => {
                            if let Err(cleanup_error) = self
                                .rollback_prepare_journal_with_start_lock(app, &journal)
                                .await
                                .and_then(|()| remove_hosted_prepare_journal(&journal_path))
                            {
                                error.message = format!(
                                        "{}; durable compensation after Prepare failure is incomplete: {cleanup_error}",
                                        error.message
                                    );
                                if error.rollback.is_none() {
                                    error.rollback = journal.compensation.rollback_value();
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(cleanup_error) => {
                            error.message = format!(
                                    "{}; durable journal after Prepare failure is unreadable: {cleanup_error}",
                                    error.message
                                );
                        }
                    }
                }
                Err(error)
            }
        }
    }

    /// Caller holds `start_mtx` across restore and this final prepare phase.
    async fn prepare_browser_core_workspace_with_start_lock(
        &self,
        app: &AppHandle,
        session_id: &str,
        session_token: &str,
        restore_outcome: RestoreWorkspaceOutcome,
        prepare_request_id: Option<&str>,
        hosted_request: Option<&HostedBrowserRequest>,
    ) -> Result<(Value, PreparedWorkspaceDisposition), PrepareWorkspaceError> {
        let had_session = self.native_surface.lock().has_session(session_id);
        if !had_session {
            let profile = paths::browser_webview_profile_dir();
            if !self.native_surface.lock().prepare_display_only(
                app,
                session_id,
                session_token,
                &profile,
            )? {
                return Err("browser/native-surface-unavailable".to_string().into());
            }
        }

        if let Err(error) = platform::wait_browser_core_ready().await {
            self.rollback_new_native_workspace(app, session_id, had_session);
            return Err(error.into());
        }
        let unbound_tabs = self.native_surface.lock().unbound_tabs(session_id);
        for tab_token in unbound_tabs {
            let label = match self
                .native_surface
                .lock()
                .webview_label_for_tab(session_id, &tab_token)
            {
                Some(label) => label,
                None => {
                    self.rollback_new_native_workspace(app, session_id, had_session);
                    return Err("browser/native-surface-missing".to_string().into());
                }
            };
            let Some(webview) = app.get_webview(&label) else {
                self.rollback_new_native_workspace(app, session_id, had_session);
                return Err("browser/native-surface-missing".to_string().into());
            };
            if let Err(error) = platform::bind_browser_core_webview(&webview).await {
                self.rollback_new_native_workspace(app, session_id, had_session);
                return Err(error.into());
            }
            let target_id = format!("native:{tab_token}");
            let bound = match self
                .native_surface
                .lock()
                .bind_target(session_id, &tab_token, &target_id)
            {
                Ok(bound) => bound,
                Err(error) => {
                    self.rollback_new_native_workspace(app, session_id, had_session);
                    return Err(error.into());
                }
            };
            if !bound {
                self.rollback_new_native_workspace(app, session_id, had_session);
                return Err("browser/native-tab-binding-failed".to_string().into());
            }
        }
        let disposition = if matches!(restore_outcome, RestoreWorkspaceOutcome::Restored) {
            PreparedWorkspaceDisposition::RestoredExisting
        } else if had_session {
            PreparedWorkspaceDisposition::Existing
        } else {
            PreparedWorkspaceDisposition::CreatedBlank
        };
        let result = json!({
            "sessionId": session_id,
            "backend": platform::browser_core_backend_name(),
        });
        self.publish_prepared_workspace_with_start_lock(
            app,
            session_id,
            disposition,
            prepare_request_id,
            hosted_request,
            &result,
        )
        .await?;
        Ok((result, disposition))
    }

    /// Caller holds `start_mtx`. A hosted prepare may spend seconds restoring
    /// surfaces and probing its automation backend, so its wrapper epoch must
    /// be checked again at the publication boundary rather than only when the
    /// request artifact is claimed.
    async fn publish_prepared_workspace_with_start_lock(
        &self,
        app: &AppHandle,
        session_id: &str,
        disposition: PreparedWorkspaceDisposition,
        prepare_request_id: Option<&str>,
        hosted_request: Option<&HostedBrowserRequest>,
        prepared_result: &Value,
    ) -> Result<(), PrepareWorkspaceError> {
        self.native_surface.lock().record_prepare_generation(
            session_id,
            disposition.rollback_kind().and(prepare_request_id),
        )?;
        let prepare_revision = prepare_request_id.and_then(|request_id| {
            self.native_surface
                .lock()
                .prepare_generation_revision(session_id, request_id)
        });

        if let Some(request) = hosted_request {
            let journal_path = hosted_prepare_journal_path(request);
            let mut journal = read_hosted_prepare_journal(&journal_path)?;
            if journal.compensation.request_id != request.request_id
                || journal.compensation.idempotency_key != request.idempotency_key
                || journal.compensation.session_id != request.session_id
            {
                return Err(
                    "browser Prepare durable journal is owned by another generation"
                        .to_string()
                        .into(),
                );
            }
            journal.compensation.rollback_kind =
                disposition.rollback_kind().unwrap_or("none").to_string();
            journal.compensation.revision = match disposition.rollback_kind() {
                Some(_) => Some(prepare_revision.ok_or_else(|| {
                    PrepareWorkspaceError::from(
                        "hosted prepare generation metadata is missing".to_string(),
                    )
                })?),
                None => None,
            };
            journal.phase = HostedPreparePhase::Prepared;
            journal.updated_at = hosted_protocol_now_ms()?;
            journal.response = None;
            write_hosted_prepare_journal(&journal)?;

            let final_liveness = ensure_hosted_caller_live(request).and_then(|_| {
                if hosted_cancellation_path(request).exists() {
                    Err("browser/request-cancelled".to_string())
                } else {
                    Ok(())
                }
            });
            if let Err(liveness_error) = final_liveness {
                journal.phase = HostedPreparePhase::Cancelled;
                journal.updated_at = hosted_protocol_now_ms()?;
                let journal_error = write_hosted_prepare_journal(&journal).err();
                let cancellation_error = self
                    .ensure_internal_prepare_cancellation(app, request, Some(&journal.compensation))
                    .err();
                let mut details = Vec::new();
                if let Some(error) = journal_error {
                    details.push(format!(
                        "failed to update durable compensation phase: {error}"
                    ));
                }
                if let Some(error) = cancellation_error {
                    details.push(format!("failed to write cancellation record: {error}"));
                }
                let message = if details.is_empty() {
                    liveness_error
                } else {
                    format!("{liveness_error}; {}", details.join("; "))
                };
                return match journal.compensation.rollback_value() {
                    Some(rollback) => Err(PrepareWorkspaceError::compensated(message, rollback)),
                    None => Err(message.into()),
                };
            }

            // This host-owned atomic phase transition is the publication
            // commit witness. It precedes every UI event and the wrapper-owned,
            // consumable response artifact. A matching cancellation still wins
            // until this process observes wrapper acknowledgement; therefore a
            // recovered Committed WAL remains durable across request-dir reset.
            journal.phase = HostedPreparePhase::Committed;
            journal.updated_at = hosted_protocol_now_ms()?;
            journal.response = Some(hosted_response(request, Ok(prepared_result.clone())));
            write_hosted_prepare_journal(&journal)?;
            self.locally_committed_prepares
                .lock()
                .insert((request.session_token.clone(), request.request_id.clone()));
        }

        self.activated
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = app.emit("browser:activated", json!({ "sessionId": session_id }));
        Ok(())
    }

    /// Publish a host-owned cancellation tombstone so the existing request
    /// ledger performs exact compensation before ACK. If the first atomic
    /// write fails, keep an in-memory retry alive; the failed Hosted outcome
    /// still carries the same rollback record, so a later tombstone can move a
    /// completed request into CancelPendingRollback and retry safely.
    fn ensure_internal_prepare_cancellation(
        &self,
        app: &AppHandle,
        request: &HostedBrowserRequest,
        prepare_compensation: Option<&HostedPrepareCompensation>,
    ) -> Result<(), String> {
        let cancellation_path = hosted_cancellation_path(request);
        let cancellation = hosted_internal_cancellation_value(
            request,
            hosted_protocol_now_ms()?,
            prepare_compensation,
        );
        let encoded = serde_json::to_vec(&cancellation).map_err(|error| {
            format!("failed to encode internal browser host cancellation record: {error}")
        })?;
        match crate::platform::filesystem::atomic_write(&cancellation_path, &encoded) {
            Ok(()) => Ok(()),
            Err(error) => {
                let first_error = format!(
                    "failed to write internal browser host cancellation record {}: {error}",
                    cancellation_path.display()
                );
                let retry_app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let mut delay = Duration::from_millis(100);
                    loop {
                        tokio::time::sleep(delay).await;
                        if crate::platform::filesystem::atomic_write(&cancellation_path, &encoded)
                            .is_ok()
                        {
                            return;
                        }
                        let manager = retry_app.state::<BrowserManager>();
                        if manager
                            .shutting_down
                            .load(std::sync::atomic::Ordering::SeqCst)
                        {
                            eprintln!(
                                "[browser] still unable to persist Prepare compensation before application exit: {}",
                                cancellation_path.display()
                            );
                            return;
                        }
                        delay = delay.saturating_mul(2).min(Duration::from_secs(5));
                    }
                });
                Err(first_error)
            }
        }
    }

    /// The browser entry remains visible in normal mode. A blank WebView workspace
    /// is created for the current task only when the user first expands it.
    pub async fn prepare_for_user(&self, browser_session_id: &str) -> Result<Value, String> {
        let _admission_guard = self.hosted_request_gate.read().await;
        self.ensure_accepting_browser_work()?;
        let app = self
            .app
            .lock()
            .clone()
            .ok_or_else(|| "application handle is not ready".to_string())?;
        let session_token = paths::browser_session_token(browser_session_id);
        let (result, _) = self
            .prepare_native_workspace(&app, browser_session_id, &session_token, false, None, None)
            .await
            .map_err(|error| error.message)?;
        Ok(result)
    }

    /// A late Prepare tombstone may close only the exact, untouched process-
    /// local generation created by that request. If UI/Agent work has already
    /// superseded the generation, rollback is an acknowledged no-op.
    async fn rollback_prepared_session(
        &self,
        browser_session_id: &str,
        request_id: &str,
        expected_revision: u64,
        preserve_restore: bool,
    ) -> Result<bool, String> {
        let session_lock = self.session_lifecycle_lock(browser_session_id);
        let _session_guard = session_lock.lock().await;
        let _start_guard = self.start_mtx.lock().await;
        self.rollback_prepared_session_with_start_lock(
            browser_session_id,
            request_id,
            expected_revision,
            preserve_restore,
        )
        .await
    }

    async fn rollback_prepared_session_with_start_lock(
        &self,
        browser_session_id: &str,
        request_id: &str,
        expected_revision: u64,
        preserve_restore: bool,
    ) -> Result<bool, String> {
        let app = self.app.lock().clone();
        let rollback = self.native_surface.lock().rollback_prepare_generation(
            app.as_ref(),
            browser_session_id,
            request_id,
            expected_revision,
            preserve_restore,
        )?;
        let Some(has_remaining) = rollback else {
            return Ok(false);
        };
        if !has_remaining {
            self.stop_with_start_lock().await?;
        } else if let Some(app) = app {
            let _ = app.emit(
                "browser:stopped",
                json!({ "sessionId": browser_session_id }),
            );
        }
        Ok(true)
    }

    /// If post-Prepare probing or coordination-file commit fails, roll back only
    /// the workspace created by this request. Existing sessions remain untouched;
    /// reset shared platform state only after removing the last new workspace.
    fn rollback_new_native_workspace(
        &self,
        app: &AppHandle,
        session_id: &str,
        existed_before_prepare: bool,
    ) {
        if existed_before_prepare {
            return;
        }
        let mut surface = self.native_surface.lock();
        match surface.close_session(Some(app), session_id) {
            Ok(false) => {
                let _ = surface.close(Some(app));
                let _ = std::fs::remove_file(paths::browser_cdp_port_json());
            }
            Ok(true) => {}
            Err(error) => {
                eprintln!("[browser] failed to roll back native browser workspace: {error}");
            }
        }
    }

    /// Binds AppHandle once during setup.
    pub fn bind_app(&self, app: AppHandle) {
        *self.app.lock() = Some(app);
    }

    /// Called after the WebView navigation callback unwinds to avoid reentering
    /// the native_surface lock.
    pub(crate) fn persist_native_restore(&self, browser_session_id: &str) -> Result<(), String> {
        // Signals such as repeated takeover navigation bumps can arrive far
        // faster than one atomic snapshot write. Coalesce: while a write for
        // this session is in flight, a concurrent caller only marks the state
        // dirty and the finishing flight performs exactly one follow-up write,
        // so the persisted state still converges to the latest revision with
        // bounded I/O instead of one spawn+write per signal.
        {
            let mut inflight = self.persistence_inflight.lock();
            if let Some(dirty) = inflight.get_mut(browser_session_id) {
                *dirty = true;
                return Ok(());
            }
            inflight.insert(browser_session_id.to_string(), false);
        }
        let app = match self.app.lock().clone() {
            Some(app) => app,
            None => {
                self.persistence_inflight.lock().remove(browser_session_id);
                return Err("application handle is not ready".to_string());
            }
        };
        self.run_persistence_flight(&app, browser_session_id)
    }

    fn run_persistence_flight(
        &self,
        app: &AppHandle,
        browser_session_id: &str,
    ) -> Result<(), String> {
        loop {
            let result = {
                let _persistence_guard = self.persistence_io.lock();
                self.persist_native_restore_once(app, browser_session_id)
            };
            match &result {
                Ok(()) => self.clear_persistence_warning(app, browser_session_id),
                Err(error) => {
                    self.record_persistence_warning(app, browser_session_id, error);
                    self.schedule_persistence_retry(app, browser_session_id);
                }
            }
            let follow_up = {
                let mut inflight = self.persistence_inflight.lock();
                match inflight.remove(browser_session_id) {
                    Some(dirty) if dirty => {
                        inflight.insert(browser_session_id.to_string(), false);
                        true
                    }
                    _ => false,
                }
            };
            if !follow_up {
                return result;
            }
        }
    }

    fn persist_native_restore_once(
        &self,
        app: &AppHandle,
        browser_session_id: &str,
    ) -> Result<(), String> {
        self.native_surface
            .lock()
            .persist_navigation_state(app, browser_session_id)
    }

    fn record_persistence_warning(&self, app: &AppHandle, session_id: &str, error: &str) {
        let changed = self
            .persistence_warnings
            .lock()
            .insert(session_id.to_string(), error.to_string())
            .as_deref()
            != Some(error);
        if changed {
            let _ = app.emit(
                "browser:persistence-warning",
                json!({ "sessionId": session_id, "error": error }),
            );
        }
    }

    fn clear_persistence_warning(&self, app: &AppHandle, session_id: &str) {
        if self
            .persistence_warnings
            .lock()
            .remove(session_id)
            .is_some()
        {
            let _ = app.emit(
                "browser:persistence-restored",
                json!({ "sessionId": session_id }),
            );
        }
    }

    fn clear_persistence_state(&self, session_id: &str) {
        self.persistence_warnings.lock().remove(session_id);
        self.persistence_retries.lock().remove(session_id);
    }

    fn schedule_persistence_retry(&self, app: &AppHandle, session_id: &str) {
        if !self
            .persistence_retries
            .lock()
            .insert(session_id.to_string())
        {
            return;
        }
        let retry_app = app.clone();
        let retry_session_id = session_id.to_string();
        tauri::async_runtime::spawn(async move {
            let mut delay = Duration::from_millis(250);
            loop {
                tokio::time::sleep(delay).await;
                let manager = retry_app.state::<BrowserManager>();
                let _persistence_guard = manager.persistence_io.lock();
                let session_gone = manager
                    .pending_deleted_session_ids
                    .read()
                    .contains(&retry_session_id)
                    || !manager.native_surface.lock().has_session(&retry_session_id);
                if manager
                    .shutting_down
                    .load(std::sync::atomic::Ordering::SeqCst)
                    || session_gone
                {
                    manager.clear_persistence_state(&retry_session_id);
                    return;
                }
                match manager.persist_native_restore_once(&retry_app, &retry_session_id) {
                    Ok(()) => {
                        manager.clear_persistence_warning(&retry_app, &retry_session_id);
                        manager.persistence_retries.lock().remove(&retry_session_id);
                        return;
                    }
                    Err(error) => {
                        manager.record_persistence_warning(&retry_app, &retry_session_id, &error);
                        delay = delay.saturating_mul(2).min(Duration::from_secs(30));
                    }
                }
            }
        });
    }

    fn persist_native_restore_best_effort(&self, browser_session_id: &str) {
        if let Err(error) = self.persist_native_restore(browser_session_id) {
            eprintln!("[browser] native browser state committed; persistence will retry in the background: {error}");
        }
    }

    /// Lazily rebuilds native pages from the URL manifest saved by the previous
    /// process. WebViews, tab tokens, CDP targets, and leases are always new in this
    /// process; active_index selects a new tab and never reuses runtime identity.
    async fn restore_saved_workspace(
        &self,
        browser_session_id: &str,
    ) -> Result<RestoreWorkspaceOutcome, String> {
        let _admission_guard = self.hosted_request_gate.read().await;
        let session_lock = self.session_lifecycle_lock(browser_session_id);
        let _session_guard = session_lock.lock().await;
        let start_guard = self.start_mtx.lock().await;
        // status() can be queued behind restart cleanup. Never rebuild native
        // WebViews from the preserved manifest after process admission closes.
        self.ensure_accepting_browser_work()?;
        let outcome = self
            .restore_saved_workspace_releasing_start_lock(browser_session_id, start_guard)
            .await?;
        if matches!(outcome, RestoreWorkspaceOutcome::Restored) {
            self.native_surface
                .lock()
                .record_prepare_generation(browser_session_id, None)?;
            self.activated
                .store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(app) = self.app.lock().clone() {
                let _ = app.emit(
                    "browser:activated",
                    json!({ "sessionId": browser_session_id, "restored": true }),
                );
            }
        }
        Ok(outcome)
    }

    /// Restore phase for callers that hold the task lifecycle lock and pass
    /// ownership of the global start lock. The lock protects shared runtime
    /// staging, then is released before slow per-tab automation waits.
    /// Publication is deliberately left to the caller so a hosted Prepare can
    /// record its rollback generation before UI/user code can mutate it.
    async fn restore_saved_workspace_releasing_start_lock(
        &self,
        browser_session_id: &str,
        start_guard: tokio::sync::MutexGuard<'_, ()>,
    ) -> Result<RestoreWorkspaceOutcome, String> {
        // When the product gate is closed, do not even read the restore manifest or
        // create hidden WebViews from it. Preserve it unchanged for a future enabled
        // or preview acceptance build.
        if !crate::platform::capabilities::browser_product_enabled() {
            return Ok(RestoreWorkspaceOutcome::Missing);
        }
        if self
            .native_surface
            .lock()
            .has_published_session(browser_session_id)
        {
            return Ok(RestoreWorkspaceOutcome::Existing);
        }
        let session_token = paths::browser_session_token(browser_session_id);
        let journal_path = hosted_prepare_journal_path_for(&session_token);
        if read_hosted_prepare_journal_if_present(&journal_path)?
            .is_some_and(|journal| journal.phase == HostedPreparePhase::Committed)
        {
            return Err(
                "browser/prepare-acknowledgement-pending: recovered Prepare is awaiting caller settlement"
                    .to_string(),
            );
        }
        let Some(restore) =
            platform::NativeBrowserSurface::read_restore_workspace(browser_session_id)?
        else {
            return Ok(RestoreWorkspaceOutcome::Missing);
        };

        let app = self
            .app
            .lock()
            .clone()
            .ok_or_else(|| "application handle is not ready".to_string())?;
        self.ensure_browser_session_allowed(browser_session_id)?;
        if self.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("application is shutting down; browser restore was rejected".to_string());
        }
        {
            let mut surface = self.native_surface.lock();
            if surface.has_published_session(browser_session_id) {
                return Ok(RestoreWorkspaceOutcome::Existing);
            }
            if surface.has_session(browser_session_id) {
                let has_remaining = surface
                    .close_session_preserving_restore(Some(&app), browser_session_id)
                    .map_err(|error| format!("residual surface from the previous browser restore is not fully cleaned: {error}"))?;
                if !has_remaining {
                    surface
                        .close_preserving_restore(Some(&app))
                        .map_err(|error| {
                            format!(
                                "failed to reset the previous browser restore environment: {error}"
                            )
                        })?;
                }
            }
        }
        let capabilities = self.native_surface.lock().capabilities();
        if !capabilities.native_display {
            return Ok(RestoreWorkspaceOutcome::Missing);
        }
        let browser_core_available = platform::browser_core_available();
        if !has_restore_automation_backend(capabilities, browser_core_available) {
            return Err(
                "browser/core-backend-unavailable: cannot restore an automation workspace"
                    .to_string(),
            );
        }

        let existing_port = if capabilities.chrome_devtools_protocol {
            live_port().await
        } else {
            None
        };
        if let Some(port) = existing_port {
            if !self.native_surface.lock().owns_port(port) {
                return Err(
                    "detected a browser automation endpoint not owned by this application"
                        .to_string(),
                );
            }
        }
        if capabilities.chrome_devtools_protocol
            && self.native_surface.lock().has_sessions()
            && existing_port.is_none()
        {
            return Err(
                "another task's native browser is running but automation endpoint state is missing"
                    .to_string(),
            );
        }
        let port = if capabilities.chrome_devtools_protocol {
            Some(match existing_port {
                Some(port) => port,
                None => pick_free_port().await?,
            })
        } else {
            None
        };
        let profile = paths::browser_webview_profile_dir();
        let tab_tokens = self.native_surface.lock().prepare_restored_surface(
            &app,
            browser_session_id,
            &session_token,
            port,
            &profile,
            &restore,
        )?;
        let created_new_port = capabilities.chrome_devtools_protocol && existing_port.is_none();
        drop(start_guard);

        let restored = async {
            if let Some(port) = port {
                if created_new_port {
                    if !probe_cdp(port, Duration::from_secs(15)).await {
                        return Err(
                            "WebView2 was restored but CDP did not become ready".to_string()
                        );
                    }
                    write_port_file(port, "app", None)?;
                }
                for (tab_token, url) in tab_tokens.iter().zip(&restore.urls) {
                    let target_id = discover_native_target(port, tab_token).await?;
                    if !self.native_surface.lock().bind_target(
                        browser_session_id,
                        tab_token,
                        &target_id,
                    )? {
                        return Err(
                            "restored tab closed before binding its new automation target"
                                .to_string(),
                        );
                    }
                    if !self.native_surface.lock().navigate_tab_after_bind(
                        Some(&app),
                        browser_session_id,
                        tab_token,
                        url,
                    )? {
                        return Err("restored tab closed before navigation".to_string());
                    }
                }
            } else if browser_core_available {
                platform::wait_browser_core_ready().await?;
                for (tab_token, url) in tab_tokens.iter().zip(&restore.urls) {
                    let label = self
                        .native_surface
                        .lock()
                        .webview_label_for_tab(browser_session_id, tab_token)
                        .ok_or_else(|| "browser/native-surface-missing".to_string())?;
                    let webview = app
                        .get_webview(&label)
                        .ok_or_else(|| "browser/native-surface-missing".to_string())?;
                    platform::bind_browser_core_webview(&webview).await?;
                    let target_id = format!("native:{tab_token}");
                    if !self.native_surface.lock().bind_target(
                        browser_session_id,
                        tab_token,
                        &target_id,
                    )? {
                        return Err(
                            "restored tab closed before binding its BrowserCore target".to_string()
                        );
                    }
                    if !self.native_surface.lock().navigate_tab_after_bind(
                        Some(&app),
                        browser_session_id,
                        tab_token,
                        url,
                    )? {
                        return Err(
                            "restored tab closed before its initial BrowserCore navigation"
                                .to_string(),
                        );
                    }
                }
            }
            // prepare_restored_surface selected the active tab from active_index.
            // Restore must not call UI activate_tab: doing so would misclassify an app
            // restart as user takeover and force the Agent to await a nonexistent handoff.
            // The next real operation atomically claims the neutral restored owner.
            if tab_tokens.get(restore.active_index).is_none() {
                return Err("restore manifest has an invalid active tab".to_string());
            }
            // Navigation is submitted asynchronously. Keep the validated manifest
            // unchanged here; real navigation events will update it from the host
            // WebView URL without letting a transient marker replace the snapshot.
            platform::NativeBrowserSurface::write_restore_workspace(browser_session_id, &restore)?;
            Ok::<(), String>(())
        }
        .await;

        if let Err(error) = restored {
            // Another task may have adopted this transaction's newly published
            // CDP endpoint while the global start lock was released for target
            // binding. Reenter the shared-runtime transaction before removing
            // A's surfaces or deciding whether the port file is now unowned.
            let _cleanup_start_guard = self.start_mtx.lock().await;
            let mut surface = self.native_surface.lock();
            let cleanup = surface
                .quarantine_failed_restore(Some(&app), browser_session_id)
                .and_then(|has_remaining| {
                    if has_remaining {
                        Ok(true)
                    } else {
                        surface.close_preserving_restore(Some(&app)).map(|()| false)
                    }
                });
            drop(surface);
            let cleanup = cleanup.and_then(|has_remaining| {
                if created_new_port {
                    remove_failed_restore_port_if_unshared(
                        port.expect("a newly created CDP endpoint always has a port"),
                        has_remaining,
                    )?;
                }
                Ok(())
            });
            let restore_write = platform::NativeBrowserSurface::write_restore_workspace(
                browser_session_id,
                &restore,
            );
            return match (cleanup, restore_write) {
                (Ok(()), Ok(())) => Err(error),
                (Err(cleanup_error), Ok(())) => Err(format!(
                    "{error}; surface reconciliation after restore failure is incomplete: {cleanup_error}"
                )),
                (Ok(()), Err(restore_error)) => Err(format!(
                    "{error}; failed to rewrite the original restore manifest: {restore_error}"
                )),
                (Err(cleanup_error), Err(restore_error)) => Err(format!(
                    "{error}; surface reconciliation after restore failure is incomplete: {cleanup_error}; failed to rewrite the original restore manifest: {restore_error}"
                )),
            };
        }

        Ok(RestoreWorkspaceOutcome::Restored)
    }

    fn schedule_watch_retry(app: AppHandle) {
        let delay = {
            let manager = app.state::<BrowserManager>();
            if manager
                .shutting_down
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return;
            }
            manager.next_watch_retry_delay()
        };
        eprintln!(
            "[browser] browser startup recovery/request consumer will retry in {} ms",
            delay.as_millis()
        );
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(delay).await;
            let shutting_down = app
                .state::<BrowserManager>()
                .shutting_down
                .load(std::sync::atomic::Ordering::SeqCst);
            if !shutting_down {
                BrowserManager::spawn_watch(app);
            }
        });
    }

    /// Watches `cdp-port.json`. When the app's native host publishes a valid port
    /// and Pinvou is not connected, calls ensure_started and emits browser:activated.
    /// The frontend uses this to reveal the browser only when a model actually uses
    /// browser capability in Work mode.
    ///
    /// Also handles failure recovery: if an attached WebView2 CDP endpoint is lost,
    /// reset only automation and emit task-scoped browser:automation-unavailable.
    /// The real page remains visible and manually usable.
    pub fn spawn_watch(app: AppHandle) {
        let manager = app.state::<BrowserManager>();
        match recover_hosted_prepare_journals_for_process_start() {
            Ok(()) => {}
            Err(error) => {
                *manager.prepare_recovery_error.lock() = Some(error.clone());
                eprintln!("[browser] failed to recover durable Prepare compensation: {error}");
                drop(manager);
                Self::schedule_watch_retry(app);
                return;
            }
        }
        // Host request/response/cancelled artifacts are process-local protocol state,
        // not recoverable task state. Atomically replace the old directory before
        // installing the watcher so create/close requests left by a crashed process
        // are never replayed. Fail closed if isolation fails.
        if let Err(error) =
            reset_host_request_directory_for_process_start(&paths::browser_host_requests_dir())
        {
            *manager.prepare_recovery_error.lock() = Some(format!(
                "browser/host-consumer-unavailable: failed to isolate stale host requests: {error}"
            ));
            eprintln!("[browser] failed to isolate stale native browser host requests: {error}");
            drop(manager);
            Self::schedule_watch_retry(app);
            return;
        }
        // Isolate the previous process's transient requests before applying the
        // product gate. When disabled, install no watcher, scanner, or health check.
        if !crate::platform::capabilities::browser_product_enabled() {
            manager.mark_watch_consumer_ready();
            return;
        }
        // Tab operations are foreground interactions and cannot wait for the two-second
        // automation health cadence. A single consumer orders data-plane activate/close
        // requests, while lease heartbeat and begin/end use a lightweight in-memory
        // control path that slow Prepare/CDP work in another task cannot block.
        let request_app = app.clone();
        tauri::async_runtime::spawn(async move {
            let request_dir = paths::browser_host_requests_dir();
            if let Err(error) = std::fs::create_dir_all(&request_dir) {
                *request_app
                    .state::<BrowserManager>()
                    .prepare_recovery_error
                    .lock() = Some(format!(
                    "browser/host-consumer-unavailable: failed to create host request directory: {error}"
                ));
                eprintln!("[browser] failed to create native browser request directory: {error}");
                BrowserManager::schedule_watch_retry(request_app.clone());
                return;
            }
            // Keep requests fail-closed, and preserve exponential backoff, until
            // the asynchronous consumer has a usable directory. The watcher may
            // still fall back to periodic scanning, so directory readiness is the
            // last fallible prerequisite for installing a consumer.
            request_app
                .state::<BrowserManager>()
                .mark_watch_consumer_ready();
            let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
            let (control_tx, mut control_rx) = tokio::sync::mpsc::unbounded_channel();
            let notify_tx = event_tx.clone();
            let notify_control_tx = control_tx.clone();
            let watcher = match notify::recommended_watcher(
                move |event: notify::Result<notify::Event>| {
                    if let Ok(event) = event {
                        let contains_request = event.paths.iter().any(|path| {
                            matches!(
                                path.extension().and_then(|value| value.to_str()),
                                Some("json" | "cancelled")
                            )
                        });
                        if contains_request {
                            let _ = notify_tx.send(());
                            let _ = notify_control_tx.send(());
                        }
                    }
                },
            ) {
                Ok(watcher) => Some(watcher),
                Err(error) => {
                    eprintln!(
                            "[browser] failed to initialize native browser request watcher; using periodic scanning: {error}"
                        );
                    None
                }
            };
            let _watcher = watcher.and_then(|mut watcher: RecommendedWatcher| {
                match watcher.watch(&request_dir, RecursiveMode::NonRecursive) {
                    Ok(()) => Some(watcher),
                    Err(error) => {
                        eprintln!("[browser] failed to watch native browser request directory; using periodic scanning: {error}");
                        None
                    }
                }
            });
            let control_app = request_app.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    let mgr = control_app.state::<BrowserManager>();
                    if mgr.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                    if let Err(error) = mgr
                        .prepare_requested_native_control_requests(&control_app)
                        .await
                    {
                        eprintln!(
                            "[browser] failed to process browser host control request: {error}"
                        );
                    }
                    tokio::select! {
                        event = control_rx.recv() => {
                            if event.is_none() {
                                break;
                            }
                            while control_rx.try_recv().is_ok() {}
                        }
                        // Notification is only a latency optimization. Even if a file
                        // event is lost, discover control requests within the Windows
                        // 400 ms input-heartbeat budget.
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                    }
                }
            });
            // Requests from this process may arrive during watcher installation, so
            // scan immediately. The synchronous startup barrier already isolated any
            // artifacts from the previous process.
            {
                let mgr = request_app.state::<BrowserManager>();
                if let Err(error) = mgr.prepare_requested_native_surfaces(&request_app).await {
                    eprintln!("[browser] failed to process native browser request: {error}");
                    // Transient WebView/I/O failure during cancellation rollback retains
                    // the marker and ledger record. Queue an explicit retry instead of
                    // relying on the same file to trigger another notification.
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    let _ = event_tx.send(());
                }
            }
            loop {
                let mgr = request_app.state::<BrowserManager>();
                if mgr.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                tokio::select! {
                    event = event_rx.recv() => {
                        if event.is_none() {
                            break;
                        }
                        // Coalesce duplicate events from one atomic write/rename.
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        while event_rx.try_recv().is_ok() {}
                        if let Err(error) = mgr.prepare_requested_native_surfaces(&request_app).await {
                            eprintln!("[browser] failed to process native browser request: {error}");
                            tokio::time::sleep(Duration::from_millis(250)).await;
                            let _ = event_tx.send(());
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {
                        // notify is only a latency optimization. Inotify can lose
                        // events (watch limits, overlay filesystems, atomic rename
                        // races), so the periodic branch must also reconcile the
                        // request directory instead of becoming a no-op heartbeat.
                        if let Err(error) = mgr.prepare_requested_native_surfaces(&request_app).await {
                            eprintln!("[browser] periodic hosted request reconciliation failed: {error}");
                        }
                    }
                }
            }
        });
        // setup runs synchronously on the wry event-loop thread without a Tokio
        // context. Use tauri::async_runtime; raw tokio::spawn would panic because
        // no reactor is running and crash application startup.
        tauri::async_runtime::spawn(async move {
            let mut fail_count = 0u32;
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let mgr = app.state::<BrowserManager>();
                // Do not reconnect automation after main-process shutdown begins.
                if mgr.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                // If an attached automation endpoint remains unavailable, reset only
                // the CDP connection. The host still owns the user-visible surface;
                // automation failure must not destroy its page or login state.
                {
                    let mut inner = mgr.inner.lock().await;
                    if inner.session.is_some() {
                        let port = inner
                            .port
                            .or_else(|| inner.session.as_ref().map(|s| s.port()))
                            .unwrap_or(0);
                        if probe_cdp(port, Duration::from_millis(800)).await {
                            fail_count = 0;
                            continue;
                        }
                        // Apply the same debounce as the stale-port-file path below.
                        // A single probe can time out during resume or high load; tearing
                        // down immediately would lose all visible tabs and Agent context.
                        fail_count += 1;
                        if fail_count < 5 {
                            continue;
                        }
                        fail_count = 0;
                        eprintln!("[browser] automation endpoint lost on port {port}; resetting CDP connection");
                        if let Some(task) = inner.loop_task.take() {
                            task.abort();
                        }
                        if let Some(task) = inner.reader_task.take() {
                            task.abort();
                        }
                        if let Some(session) = inner.session.take() {
                            let _ = session.close().await;
                        }
                        inner.port = None;
                        inner.active_session = None;
                        inner.active_target = None;
                        mgr.page_sessions.lock().clear();
                        mgr.activated
                            .store(false, std::sync::atomic::Ordering::SeqCst);
                        // The frontend filters events strictly by task and ignores a
                        // global payload. Emit the exact sessionId for every workspace
                        // still hosted natively, preserving the page while marking only
                        // Agent automation unavailable.
                        let session_ids = mgr.native_surface.lock().session_ids();
                        for session_id in session_ids {
                            let _ = app.emit(
                                "browser:automation-unavailable",
                                json!({ "sessionId": session_id, "port": port }),
                            );
                        }
                        continue;
                    }
                }
                // When detached, connect only to an endpoint still owned and published
                // by this application's native host.
                let Some(port) = live_port().await else {
                    fail_count = 0;
                    continue;
                };
                if !mgr.native_surface.lock().owns_port(port) {
                    continue;
                }
                fail_count = 0;
                if mgr.ensure_started().await.is_ok() {
                    mgr.activated
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                } else {
                    eprintln!("[browser] failed to attach the native-page automation endpoint; retrying later");
                }
            }
        });
    }

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    /// Reactivates a page on the existing browser-level connection. Shared by the
    /// initial and post-start_mtx checks. Opening another connection would leak the
    /// old reader/event-loop tasks and duplicate browser-level Target events.
    async fn reattach_existing(
        &self,
        session: Arc<cdp::CdpSession>,
        gen: u64,
    ) -> Result<(), String> {
        let (target_id, sid) = attach_first_page_cached(&session, &self.page_sessions).await?;
        let mut inner = self.inner.lock().await;
        // If stop() ran during attachment, discard this generation. Otherwise an old
        // connection could switch to a new session after its prior stream stopped.
        if self.stop_gen.load(std::sync::atomic::Ordering::SeqCst) != gen
            || self.shutting_down.load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(
                "browser was stopped during startup or the application is shutting down"
                    .to_string(),
            );
        }
        switch_active_session_locked(&mut inner, &sid).await?;
        self.page_sessions.lock().insert(target_id.clone(), sid);
        inner.active_target = Some(target_id);
        Ok(())
    }

    /// Ensures the Windows native browser CDP automation connection is attached.
    /// Idempotently reuses an existing connection.
    pub async fn ensure_started(&self) -> Result<(), String> {
        // Reject startup during main-process shutdown to avoid an orphan browser.
        if self.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("application is shutting down; browser startup was rejected".to_string());
        }
        // Snapshot the stop generation before waiting for start_mtx. If stop completes
        // while waiting, abandon startup after acquiring the lock so polling cannot
        // resurrect an explicitly stopped browser or create one during shutdown.
        let gen_before_wait = self.stop_gen.load(std::sync::atomic::Ordering::SeqCst);
        {
            let inner = self.inner.lock().await;
            if inner.session.is_some() && inner.active_session.is_some() {
                return Ok(());
            }
            // If the connection remains but active_session is empty after the last tab
            // closes, reactivate a page on the existing connection instead of opening
            // a second WebSocket and leaking/duplicating its reader and event loop.
            if inner.session.is_some() {
                let session = inner.session.clone().expect("session presence was checked");
                let gen = self.stop_gen.load(std::sync::atomic::Ordering::SeqCst);
                // Do not hold inner across CDP attachment I/O; reacquire it to commit state.
                drop(inner);
                return self.reattach_existing(session, gen).await;
            }
        }

        // start_mtx makes startup single-flight. Concurrent callers wait and reuse
        // committed state instead of starting duplicate loops and losing handles.
        let _start_guard = self.start_mtx.lock().await;
        // The initial check may have passed before restart closed admission.
        // Recheck after the lifecycle lock so a queued watch iteration cannot
        // reconnect automation after restart cleanup releases the lock.
        self.ensure_accepting_browser_work()?;
        if self.stop_gen.load(std::sync::atomic::Ordering::SeqCst) != gen_before_wait {
            return Err("browser was stopped while startup was waiting".to_string());
        }
        // Discard startup results if stop increments the generation while starting.
        let gen_at_start = self.stop_gen.load(std::sync::atomic::Ordering::SeqCst);
        {
            let inner = self.inner.lock().await;
            if let Some(session) = inner.session.clone() {
                // Match the initial check after acquiring start_mtx. State may become
                // "connection present, active session absent" while waiting (for example,
                // after closing the last tab). Full startup would overwrite and leak the
                // old connection/tasks, so release both locks and reattach over the network.
                if inner.active_session.is_some() {
                    return Ok(());
                }
                drop(inner);
                drop(_start_guard);
                return self.reattach_existing(session, gen_at_start).await;
            }
        }

        // Connect only to an automation endpoint published by the native workspace.
        // Never launch external Chrome here; native-host failure must remain explicit
        // rather than silently changing surface, identity, or interaction semantics.
        let port = live_port()
            .await
            .ok_or_else(|| "native browser automation endpoint is not ready".to_string())?;
        if !self.native_surface.lock().owns_port(port) {
            return Err(
                "automation endpoint is not owned by this application's native browser workspace"
                    .to_string(),
            );
        }
        // Connect CDP, attach, enable domains, and start the event loop. Keep session
        // and reader handles outside the closure so every failure closes/aborts them.
        let mut boot_session: Option<Arc<cdp::CdpSession>> = None;
        let mut boot_reader: Option<tokio::task::JoinHandle<()>> = None;
        let boot: Result<(), String> = async {
            let connected = cdp::connect(port)
                .await
                .map_err(|e| format!("CDP connection failed: {e:#}"))?;
            let session = connected.session;
            boot_session = Some(Arc::clone(&session));
            boot_reader = Some(connected.reader_task);

            // Enable Target discovery so internal state can repair target/session
            // mappings. The native host sends task-scoped UI events.
            session
                .call(
                    None,
                    "Target.setDiscoverTargets",
                    json!({ "discover": true }),
                )
                .await
                .map_err(|e| format!("Target.setDiscoverTargets failed: {e}"))?;

            let (target_id, session_id) =
                attach_first_page_cached(&session, &self.page_sessions).await?;

            session
                .call(Some(&session_id), "Page.enable", json!({}))
                .await
                .map_err(|e| format!("Page.enable failed: {e}"))?;
            let app = self
                .app
                .lock()
                .clone()
                .ok_or_else(|| "BrowserManager has no bound AppHandle".to_string())?;
            let loop_task = tokio::spawn(run_event_loop(app, connected.events));

            // If stop interrupted startup, discard the result so stop is not lost and
            // the browser cannot remain connected without UI publication. The common
            // failure path closes the WebSocket and aborts its reader.
            if self.stop_gen.load(std::sync::atomic::Ordering::SeqCst) != gen_at_start {
                return Err("browser was stopped during startup".to_string());
            }

            let mut inner = self.inner.lock().await;
            // Recheck generation and shutdown under inner. stop/shutdown can complete
            // between the prior check and this lock; committing then would orphan a
            // connection. Discard through the common cleanup path instead.
            if self.stop_gen.load(std::sync::atomic::Ordering::SeqCst) != gen_at_start
                || self.shutting_down.load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err(
                    "browser was stopped during startup or the application is shutting down"
                        .to_string(),
                );
            }
            inner.port = Some(port);
            inner.session = Some(session);
            inner.active_session = Some(session_id);
            inner.active_target = Some(target_id);
            inner.loop_task = Some(loop_task);
            inner.reader_task = boot_reader.take();
            Ok(())
        }
        .await;

        if let Err(e) = &boot {
            // Close this startup's WebSocket and abort its reader so retries do not
            // leak one connection/task each. close is idempotent.
            if let Some(session) = boot_session.take() {
                let _ = session.close().await;
            }
            if let Some(task) = boot_reader.take() {
                task.abort();
            }
            // Clear page_sessions because sessionIds are WebSocket-local. Retaining
            // entries after closing this connection would make the next connection
            // reuse dead ids, fail forever, and never publish browser:activated.
            self.page_sessions.lock().clear();
            return Err(e.clone());
        }
        // Clear historical failure state after successful attachment so models do not
        // receive a stale browser-unavailable reason for the next 24 hours.
        let _ = std::fs::remove_file(paths::browser_last_error_json());
        Ok(())
    }

    /// Stops the browser: disconnects automation, closes app-owned native pages,
    /// cleans coordination files, and emits browser:stopped for the frontend.
    ///
    /// Shares start_mtx with ensure_started (start_mtx before inner), preventing stop
    /// from returning on transient empty state. Incrementing the generation makes an
    /// in-progress startup discard its result.
    pub async fn stop(&self) -> Result<(), String> {
        let _admission_guard = self.hosted_request_gate.write().await;
        // Join the same lock order as ensure_started so stop is serialized with startup
        // and native workspace creation.
        let _start_guard = self.start_mtx.lock().await;
        // A UI stop queued before restart must not run after restart has
        // preserved restore state and released the lifecycle lock.
        self.ensure_accepting_browser_work()?;
        self.stop_with_start_lock().await
    }

    /// Full stop path for callers already holding start_mtx. Reused when closing the
    /// last task workspace so a newly inserted workspace cannot be hit by old cleanup.
    async fn stop_with_start_lock(&self) -> Result<(), String> {
        // Advance the generation so startup interrupted by this stop self-discards.
        self.stop_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Do not clear the publication bit before irreversible native close:
        // if close fails, a retry must still emit the eventual stopped event.
        let was_activated = self.activated.load(std::sync::atomic::Ordering::SeqCst);

        let mut inner = self.inner.lock().await;
        let had_session = inner.session.is_some();
        let native_initialized = {
            let surface = self.native_surface.lock();
            inner
                .port
                .map(|port| surface.owns_port(port))
                // Native pages may exist before the CDP watcher commits the port
                // into inner. Closing the final workspace must still clean the
                // native runtime and coordination files.
                .unwrap_or_else(|| surface.is_initialized())
        };
        // CDP is only the automation channel for in-app native pages. Do not use
        // Browser.close to stop the entire WebView runtime; disconnect first and
        // let the host destroy its child views precisely.
        if let Some(session) = inner.session.take() {
            let _ = session.close().await;
        }
        if native_initialized {
            let app = self.app.lock().clone();
            self.native_surface.lock().close(app.as_ref())?;
            let _ = std::fs::remove_file(paths::browser_cdp_port_json());
        }
        // BrowserCore's WebDriver is process-shared across task workspaces. This full-stop path
        // is reached only for global stop or after the last native workspace has closed; scoped
        // close with remaining workspaces never comes here.
        platform::shutdown_browser_core_for_stop().await;
        if let Some(task) = inner.loop_task.take() {
            task.abort();
        }
        if let Some(task) = inner.reader_task.take() {
            task.abort();
        }
        inner.port = None;
        inner.active_session = None;
        inner.active_target = None;
        self.page_sessions.lock().clear();
        self.activated
            .store(false, std::sync::atomic::Ordering::SeqCst);
        // Tell the frontend to hide the browser tab (main.jsx and BrowserView
        // listen for browser:stopped). Emit only after the browser actually ran
        // or activated; a stop that never started, such as RunEvent::Exit fallback,
        // must not fabricate an event.
        if had_session || was_activated {
            if let Some(app) = self.app.lock().clone() {
                let _ = app.emit("browser:stopped", json!({}));
            }
        }
        self.persistence_warnings.lock().clear();
        self.persistence_retries.lock().clear();
        Ok(())
    }

    /// Stop the native browser for the current conversation. While other
    /// conversation pages remain, destroy only this page and retain the shared
    /// WebView2 environment. Closing the final page reuses the complete stop path
    /// to clean CDP and coordination files.
    pub async fn stop_for_session(&self, browser_session_id: &str) -> Result<(), String> {
        let _admission_guard = self.hosted_request_gate.read().await;
        self.stop_for_session_with_hosted_gate_held(browser_session_id)
            .await
    }

    /// Performs scoped stop after the caller has acquired hosted-request
    /// admission. Task deletion holds the write side as a drain barrier, while
    /// ordinary UI stop holds a read guard so unrelated request scans continue.
    async fn stop_for_session_with_hosted_gate_held(
        &self,
        browser_session_id: &str,
    ) -> Result<(), String> {
        let session_lock = self.session_lifecycle_lock(browser_session_id);
        let _session_guard = session_lock.lock().await;
        // Serialize with creation and global stop. A new workspace cannot appear
        // between closing the last workspace and cleaning the shared runtime, or
        // the old close request would destroy the newly created conversation.
        let _start_guard = self.start_mtx.lock().await;
        // Keep task deletion and UI stop fail-closed during process restart.
        // Internal hosted-request compensation intentionally calls the
        // *_with_start_lock helpers instead: its admission read guard makes
        // restart wait until that already-accepted transaction has drained.
        self.ensure_accepting_browser_work()?;
        let app = self.app.lock().clone();
        let action = {
            let surface = self.native_surface.lock();
            scoped_stop_action(
                surface.owns_session_resources(browser_session_id),
                surface.has_sessions(),
            )
        };
        let mut result = match action {
            ScopedStopAction::IgnoreUnknownNativeSession => {
                // Share the restore-point lock with navigation/tab updates;
                // otherwise a late persistence task could recreate the manifest.
                let _persistence_guard = self.persistence_io.lock();
                platform::NativeBrowserSurface::delete_restore_workspace(browser_session_id)
            }
            // Clean the shared automation runtime when the registry is empty.
            // This branch never touches another conversation workspace.
            ScopedStopAction::StopManagedRuntime => {
                // Restore-point deletion is the durable commit for Stop. Do not
                // destroy the runtime first when it fails, or the command reports
                // failure while next startup restores a page the user closed.
                {
                    let _persistence_guard = self.persistence_io.lock();
                    platform::NativeBrowserSurface::delete_restore_workspace(browser_session_id)?;
                }
                self.stop_with_start_lock().await
            }
            ScopedStopAction::CloseNativeSession => {
                // Commit "do not restore" before irreversibly closing WebViews.
                // Reconcile per page: remove successfully closed pages immediately,
                // retain failures as survivors, and rewrite the manifest. The next
                // stop retries only real survivors, never physically closed pages.
                let has_remaining = {
                    // This lock covers only the synchronous read-old-manifest ->
                    // delete -> close/compensate commit section. It cannot span the
                    // async runtime stop below or the Tauri command future is not Send.
                    let _persistence_guard = self.persistence_io.lock();
                    // Acquire the native lock before reading/deleting restore state.
                    // UI tab operations skip persistence_io but always use
                    // native_surface; otherwise one could write a new manifest
                    // between delete and close and resurrect a stopped page.
                    let mut surface = self.native_surface.lock();
                    platform::NativeBrowserSurface::delete_restore_workspace(browser_session_id)?;
                    let close_result =
                        surface.close_session_preserving_restore(app.as_ref(), browser_session_id);
                    match close_result {
                        Ok(has_remaining) => has_remaining,
                        Err(close_error) => return Err(close_error),
                    }
                };
                if !has_remaining {
                    self.stop_with_start_lock().await
                } else {
                    if let Some(app) = app {
                        let _ = app.emit(
                            "browser:stopped",
                            json!({ "sessionId": browser_session_id }),
                        );
                    }
                    Ok(())
                }
            }
        };
        if result.is_ok() {
            match remove_hosted_prepare_journal_for_session(browser_session_id) {
                Ok(()) => {
                    let session_token = paths::browser_session_token(browser_session_id);
                    self.locally_committed_prepares
                        .lock()
                        .retain(|(token, _)| token != &session_token);
                }
                Err(error) => result = Err(error),
            }
        }
        if result.is_ok() {
            if let Err(error) = remove_hosted_prepare_quarantine_for_session(browser_session_id) {
                result = Err(error);
            }
        }
        if result.is_ok() {
            self.clear_persistence_state(browser_session_id);
        }
        result
    }

    /// Full browser cleanup used when deleting a task. Ordinary UI Close Browser
    /// retains the task MCP configuration; task deletion removes it only after
    /// WebView/restore cleanup succeeds. NotFound is idempotent success; other I/O
    /// errors return to the composition-root retry queue.
    pub async fn delete_for_session(&self, browser_session_id: &str) -> Result<(), String> {
        // Drain every accepted scanner before teardown. No cancellation can
        // create or mutate this task's request-ledger generation between native
        // cleanup and the final purge below.
        let _admission_guard = self.hosted_request_gate.write().await;
        self.stop_for_session_with_hosted_gate_held(browser_session_id)
            .await?;
        remove_private_plain_file_and_verify_absent(
            &paths::browser_session_mcp_json(browser_session_id),
            "task browser MCP configuration",
        )?;
        remove_hosted_prepare_journal_for_session(browser_session_id)?;
        remove_hosted_request_artifacts_for_session(browser_session_id)?;

        // Purge retry/compensation records only after every fallible teardown
        // step succeeds. A failed attempt deliberately retains CancelPending
        // state so the next cleanup can compensate the exact committed result.
        self.native_surface
            .lock()
            .purge_session_requests(browser_session_id)?;
        // The injected durable validator is now the source of truth; release
        // the process-local deny marker only after host artifacts and ledger
        // obligations are gone.
        self.clear_session_deleted(browser_session_id);
        Ok(())
    }

    /// At process startup, use the current task set to remove browser files left
    /// by the previous crash. Disk names contain only session tokens, so hash
    /// active session IDs in memory without storing raw IDs in restore manifests.
    /// Aggregate I/O failures for bounded-backoff composition-root retries;
    /// rerunning after an orphan is deleted remains idempotent.
    pub fn reconcile_session_files(&self, active_session_ids: &[String]) -> Result<(), String> {
        let active_tokens = active_session_ids
            .iter()
            .map(|session_id| paths::browser_session_token(session_id))
            .collect::<HashSet<_>>();
        reconcile_browser_session_file_dirs(
            &active_tokens,
            &[
                paths::browser_workspace_restore_dir(),
                paths::browser_workspaces_dir(),
                paths::browser_session_mcp_dir(),
                hosted_prepare_journal_dir(),
            ],
            self.startup_reconcile_cutoff,
        )?;
        reconcile_hosted_prepare_quarantine_files(&active_tokens, self.startup_reconcile_cutoff)
    }

    /// `Target.targetCreated` activation repair, called by the event loop. When
    /// all tabs were closed and active is empty, attach the automation session to
    /// a tab newly created by the model through MCP.
    async fn on_target_created(&self, target_id: &str) {
        let gen = self.stop_gen.load(std::sync::atomic::Ordering::SeqCst);
        let session = {
            let inner = self.inner.lock().await;
            if inner.active_session.is_some() {
                return; // An active page exists; keep the new background tab hidden.
            }
            let Some(session) = inner.session.clone() else {
                return;
            };
            session
        };
        // Attach without holding inner across CDP network await, then reacquire it
        // for commit.
        let Ok(sid) = attach_page_cached(&session, &self.page_sessions, target_id).await else {
            return;
        };
        let mut inner = self.inner.lock().await;
        if self.stop_gen.load(std::sync::atomic::Ordering::SeqCst) != gen {
            return; // Stop raced with attach; discard the result.
        }
        if inner.active_session.is_some() {
            return; // A concurrent path already activated another page.
        }
        if switch_active_session_locked(&mut inner, &sid).await.is_ok() {
            inner.active_target = Some(target_id.to_string());
        }
    }

    /// `Target.targetDestroyed` repair, called by the event loop. When MCP or page
    /// script closes the active tab, switch to a survivor. If none remain, clear
    /// active so the next `ensure_started` reuses the connection through
    /// `reattach_existing` instead of freezing on the destroyed target's final
    /// frame. Explicit close_tab handles itself first; this path is idempotent.
    async fn on_target_destroyed(&self, target_id: &str) {
        self.page_sessions.lock().remove(target_id);
        let mut inner = self.inner.lock().await;
        if inner.active_target.as_deref() != Some(target_id) {
            return;
        }
        let Some(session) = inner.session.clone() else {
            inner.active_session = None;
            inner.active_target = None;
            return;
        };
        // Enumerate survivors. Explicitly exclude the destroyed target, which may
        // remain in Chrome's list; list_page_tabs skips dying targets that fail attach.
        if let Ok(tabs) = list_page_tabs(&session, &self.page_sessions).await {
            if let Some(first) = tabs.iter().find(|t| t.target_id != target_id) {
                let sid = self.page_sessions.lock().get(&first.target_id).cloned();
                if let Some(sid) = sid {
                    if switch_active_session_locked(&mut inner, &sid).await.is_ok() {
                        inner.active_target = Some(first.target_id.clone());
                        return;
                    }
                }
            }
        }
        // With no survivor or failed switching, the destroyed target's flattened
        // session is already invalid. Clear active without stopping a stream and
        // await the next reattach.
        inner.active_session = None;
        inner.active_target = None;
    }

    /// A bounded CDP lifecycle queue may intentionally coalesce a target churn
    /// burst. Rebuild both the attach cache and active target from one
    /// authoritative Target.getTargets snapshot rather than applying deltas
    /// after an overflow gap.
    async fn reconcile_target_lifecycle(&self) -> Result<TargetLifecycleReconcileOutcome, String> {
        let generation = self.stop_gen.load(std::sync::atomic::Ordering::SeqCst);
        let (session, cached_sessions) = {
            let inner = self.inner.lock().await;
            let Some(session) = inner.session.clone() else {
                return Ok(TargetLifecycleReconcileOutcome::Reconciled);
            };
            (session, self.page_sessions.lock().clone())
        };

        // Never let an old connection write the process-wide attach cache
        // before the generation/session check below. Seed a private snapshot
        // with reusable sessions, attach missing live targets there, then
        // merge only if this exact connection is still current.
        let snapshot_sessions = Arc::new(parking_lot::Mutex::new(cached_sessions.clone()));
        let mut tabs = match list_page_tabs_authoritative(&session, &snapshot_sessions).await {
            Ok(tabs) => tabs,
            Err(error) => {
                // An incomplete delta stream may no longer be trusted. If the
                // authoritative snapshot itself fails on the same connection,
                // invalidate that automation connection fail-closed. The
                // existing watch reconnects it without closing native pages.
                self.invalidate_target_lifecycle_connection(generation, &session)
                    .await;
                return Ok(TargetLifecycleReconcileOutcome::ConnectionInvalidated(
                    format!("CDP target lifecycle resynchronization failed: {error}"),
                ));
            }
        };
        tabs.sort_by(|left, right| left.target_id.cmp(&right.target_id));
        let live_targets = tabs
            .iter()
            .map(|tab| tab.target_id.clone())
            .collect::<HashSet<_>>();
        let mut live_sessions = snapshot_sessions.lock().clone();
        live_sessions.retain(|target_id, _| live_targets.contains(target_id));
        let captured_targets = cached_sessions.keys().cloned().collect::<HashSet<_>>();

        let mut inner = self.inner.lock().await;
        if self.stop_gen.load(std::sync::atomic::Ordering::SeqCst) != generation
            || !inner
                .session
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &session))
        {
            return Ok(TargetLifecycleReconcileOutcome::Reconciled);
        }
        let desired_target = reconciled_active_target(
            inner.active_target.as_deref(),
            tabs.iter().map(|tab| tab.target_id.as_str()),
        );
        let Some(desired_target) = desired_target else {
            inner.active_session = None;
            inner.active_target = None;
            merge_reconciled_page_sessions(&self.page_sessions, &captured_targets, &live_sessions);
            return Ok(TargetLifecycleReconcileOutcome::Reconciled);
        };
        let Some(desired_session) = live_sessions.get(&desired_target) else {
            let error = format!(
                "CDP target lifecycle resynchronization has no attach session: {desired_target}"
            );
            drop(inner);
            self.invalidate_target_lifecycle_connection(generation, &session)
                .await;
            return Ok(TargetLifecycleReconcileOutcome::ConnectionInvalidated(
                error,
            ));
        };
        if inner.active_target.as_deref() == Some(desired_target.as_str())
            && inner.active_session.as_deref() == Some(desired_session.as_str())
        {
            merge_reconciled_page_sessions(&self.page_sessions, &captured_targets, &live_sessions);
            return Ok(TargetLifecycleReconcileOutcome::Reconciled);
        }
        if let Err(error) = switch_active_session_locked(&mut inner, desired_session).await {
            drop(inner);
            self.invalidate_target_lifecycle_connection(generation, &session)
                .await;
            return Ok(TargetLifecycleReconcileOutcome::ConnectionInvalidated(
                format!("CDP target lifecycle resynchronization switch failed: {error}"),
            ));
        }
        merge_reconciled_page_sessions(&self.page_sessions, &captured_targets, &live_sessions);
        inner.active_target = Some(desired_target);
        Ok(TargetLifecycleReconcileOutcome::Reconciled)
    }

    async fn invalidate_target_lifecycle_connection(
        &self,
        generation: u64,
        session: &Arc<CdpSession>,
    ) {
        let (current_session, reader_task) = {
            let mut inner = self.inner.lock().await;
            if self.stop_gen.load(std::sync::atomic::Ordering::SeqCst) != generation
                || !inner
                    .session
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, session))
            {
                return;
            }
            // This method runs inside loop_task. Taking and dropping its
            // JoinHandle detaches it; aborting our own task here could cancel
            // cleanup before the reader/session senders are closed.
            inner.loop_task.take();
            let reader_task = inner.reader_task.take();
            let current_session = inner.session.take();
            inner.port = None;
            inner.active_session = None;
            inner.active_target = None;
            self.page_sessions.lock().clear();
            (current_session, reader_task)
        };
        self.activated
            .store(false, std::sync::atomic::Ordering::SeqCst);
        if let Some(current_session) = current_session {
            current_session.close().await;
        }
        if let Some(reader_task) = reader_task {
            reader_task.abort();
        }
    }

    /// Complete asynchronous cleanup before app restart: close app-owned native
    /// pages, terminate automation connections, and remove coordination files.
    /// Native page lifetime does not depend on an external browser process.
    ///
    /// This runs in the explicit restart command's async path and can await
    /// lifecycle and persistence locks. Lock order is hosted admission -> start
    /// -> persistence_io -> native_surface, allowing active WebView getters and
    /// persistence transactions to settle before manifest refresh and surface close.
    pub async fn shutdown_before_restart(&self) {
        // Restart is an intentional async lifecycle boundary, unlike the
        // non-blocking RunEvent::Exit fallback below. Stop new work first and
        // serialize with any start/restore transaction so no late startup can
        // publish resources after cleanup.
        self.shutting_down
            .store(true, std::sync::atomic::Ordering::SeqCst);
        platform::begin_browser_core_process_shutdown();
        self.stop_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Hosted scanners acquire a read guard before any request dispatch;
        // restart takes the write side before start_mtx. Once held, every
        // request accepted before shutdown has settled, while later scans fail
        // closed after acquiring their read guard.
        let _hosted_request_guard = self.hosted_request_gate.write().await;
        let _start_guard = self.start_mtx.lock().await;

        let app = self.app.lock().clone();
        {
            // Preserve the established persistence_io -> native_surface lock
            // order. Restart may wait here; it must not silently skip the last
            // restore snapshot or native surface close merely because a
            // persistence callback was already in flight.
            let _persistence_guard = self.persistence_io.lock();
            let mut surface = self.native_surface.lock();
            if surface.is_initialized() {
                if let Some(app) = app.as_ref() {
                    if let Err(error) = surface.persist_all_restore(app) {
                        eprintln!(
                            "[browser] Failed to refresh browser restore manifest before restart; retaining the previous complete manifest: {error}"
                        );
                    }
                }
                if let Err(error) = surface.close_preserving_restore(app.as_ref()) {
                    eprintln!("[browser] Failed to close native browser pages before restart; process exit will reclaim them: {error}");
                }
            }
        }

        // Linux waits for the process-wide WebDriver operation gate after the
        // permanent admission latch is closed. This drains operations accepted
        // before restart and performs the final session/child reset.
        platform::shutdown_browser_core_for_stop().await;

        let mut inner = self.inner.lock().await;
        if let Some(task) = inner.loop_task.take() {
            task.abort();
        }
        if let Some(task) = inner.reader_task.take() {
            task.abort();
        }
        if let Some(session) = inner.session.take() {
            let _ = session.close().await;
        }
        inner.port = None;
        inner.active_session = None;
        inner.active_target = None;
        drop(inner);

        self.page_sessions.lock().clear();
        self.persistence_warnings.lock().clear();
        self.persistence_retries.lock().clear();
        self.activated
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let _ = std::fs::remove_file(paths::browser_cdp_port_json());
        clear_host_request_files();
    }

    /// Non-blocking fallback for the Tauri Exit event. Explicit async
    /// lifecycle boundaries such as app.restart() must use
    /// [`Self::shutdown_before_restart`] instead.
    pub fn shutdown_on_exit(&self) {
        // Set the exit marker first so the watcher exits on its next iteration and
        // ensure_started rejects new connections.
        self.shutting_down
            .store(true, std::sync::atomic::Ordering::SeqCst);
        // Also bump the stop generation. An in-flight startup sequence does not
        // hold inner during network await, so try_lock below may observe it empty.
        // Check the generation before commit and discard/clean mismatches, or a
        // startup racing exit could commit into the cleared inner state.
        self.stop_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        platform::shutdown_browser_core_for_exit();
        let app = self.app.try_lock().and_then(|app| app.as_ref().cloned());
        if let Some(_persistence_io) = self.persistence_io.try_lock() {
            if let Some(mut surface) = self.native_surface.try_lock() {
                if surface.is_initialized() {
                    if let Some(app) = app.as_ref() {
                        if let Err(error) = surface.persist_all_restore(app) {
                            // Navigation and tab changes persist continuously. If
                            // the exit snapshot fails, retain the previous complete
                            // manifest instead of damaging it with partial write/delete.
                            eprintln!("[browser] Failed to refresh browser restore manifest during exit: {error}");
                        }
                    }
                    if let Err(error) = surface.close_preserving_restore(app.as_ref()) {
                        eprintln!(
                            "[browser] Failed to close native browser pages during exit: {error}"
                        );
                    }
                }
            } else {
                eprintln!(
                    "[browser] Native browser state lock is busy during exit; retaining the latest restore point and letting process exit destroy pages"
                );
            }
        } else {
            eprintln!("[browser] Restore persistence is active during exit; retaining the latest restore point and letting process exit destroy pages");
        }

        if let Ok(mut inner) = self.inner.try_lock() {
            if let Some(task) = inner.loop_task.take() {
                task.abort();
            }
            if let Some(task) = inner.reader_task.take() {
                task.abort();
            }
            if let Some(session) = inner.session.take() {
                // Best-effort close WS to terminate the read loop; the exit event
                // does not await asynchronous close completion.
                let session = Arc::clone(&session);
                tauri::async_runtime::spawn(async move { session.close().await });
            }
            inner.port = None;
            inner.active_session = None;
            inner.active_target = None;
        } else {
            eprintln!("[browser] Automation state lock is busy during exit; process exit will reclaim the connection");
        }
        if let Some(mut page_sessions) = self.page_sessions.try_lock() {
            page_sessions.clear();
        }
        self.activated
            .store(false, std::sync::atomic::Ordering::SeqCst);
        // Only the app publishes this port. The endpoint necessarily becomes
        // invalid after exit even when an in-memory lock is busy.
        let _ = std::fs::remove_file(paths::browser_cdp_port_json());
        clear_host_request_files();
    }

    /// Query status for frontend mount/polling. `activeTab` is the active tab's
    /// targetId. Frontend tab identity always uses targetId because sessionId
    /// changes on every attach.
    pub async fn status(&self, browser_session_id: &str) -> Value {
        if !crate::platform::capabilities::browser_product_enabled() {
            return json!({
                "running": false,
                "sessionId": browser_session_id,
                "native": false,
                "missing": true,
                "unavailable": true,
            });
        }
        let restore_error = match self.restore_saved_workspace(browser_session_id).await {
            Ok(_) => None,
            Err(error) => {
                eprintln!("[browser] Failed to restore conversation browser: {error}");
                Some(error)
            }
        };
        let app = self.app.lock().clone();
        let (session_state, control_state) = {
            let surface = self.native_surface.lock();
            (
                surface.session_state(app.as_ref(), browser_session_id),
                surface.control_state(browser_session_id),
            )
        };
        if let Some((token, mut url)) = session_state {
            if url.contains("#pinvou-session-") || url.contains("#pinvou-tab-") {
                url = "about:blank".to_string();
            }
            let mut status = json!({
                "running": true,
                "activeTab": token,
                "url": url,
                "sessionId": browser_session_id,
                "native": true,
            });
            if let Some(control) = control_state {
                status["controlOwner"] = json!(control.owner);
                status["controlRevision"] = json!(control.revision);
            }
            if let Some(warning) = self
                .persistence_warnings
                .lock()
                .get(browser_session_id)
                .cloned()
            {
                status["persistenceWarning"] = json!(warning);
            }
            return status;
        }
        if let Some(error) = restore_error {
            return json!({
                "running": false,
                "sessionId": browser_session_id,
                "native": true,
                "restoreError": error,
            });
        }
        json!({
            "running": false,
            "sessionId": browser_session_id,
            "native": true,
            "missing": true,
        })
    }

    /// UI hand-back is the immediate-resume shortcut and atomically signs a fresh
    /// Agent lease. Idle auto-release only changes owner after its revision guard
    /// succeeds; the next Agent activation signs its own lease. Opaque leases are
    /// never exposed to the React renderer.
    pub fn hand_back_to_agent(&self, browser_session_id: &str) -> Result<Value, String> {
        let app = self
            .app
            .lock()
            .clone()
            .ok_or_else(|| "App handle is not ready".to_string())?;
        let control = {
            let mut surface = self.native_surface.lock();
            if surface
                .hand_back_to_agent(Some(&app), browser_session_id)?
                .is_none()
            {
                return Err(
                    "Native browser workspace for the specified conversation does not exist"
                        .to_string(),
                );
            }
            surface
                .control_state(browser_session_id)
                .ok_or_else(|| "Browser control state is unavailable".to_string())?
        };
        self.persist_native_restore_best_effort(browser_session_id);
        Ok(json!({
            "sessionId": browser_session_id,
            "controlOwner": control.owner,
            "controlRevision": control.revision,
        }))
    }

    pub(crate) fn release_user_control_if_idle(
        &self,
        browser_session_id: &str,
        expected_revision: u64,
    ) -> Result<bool, String> {
        let app = self
            .app
            .lock()
            .clone()
            .ok_or_else(|| "App handle is not ready".to_string())?;
        self.native_surface.lock().release_user_control_if_idle(
            &app,
            browser_session_id,
            expected_revision,
        )
    }

    /// List tabs by live enumeration of page targets. Attach reuses the cache to
    /// prevent session leaks.
    pub async fn list_tabs(&self, browser_session_id: &str) -> Result<Vec<TabInfo>, String> {
        let app = self.app.lock().clone();
        self.native_surface
            .lock()
            .list_tabs(app.as_ref(), browser_session_id)
            .ok_or_else(|| {
                "Native browser workspace for the specified conversation does not exist".to_string()
            })
    }

    /// Resolve a hidden marker WebView to this process's automation identity without exposing
    /// it first. Linux binds the concrete WebKit WebView through BrowserCore/WebDriver; Windows
    /// keeps the existing CDP marker discovery path.
    async fn bind_staged_native_target(
        &self,
        app: &AppHandle,
        browser_session_id: &str,
        tab_token: &str,
    ) -> Result<String, String> {
        if platform::browser_core_available() {
            platform::wait_browser_core_ready().await?;
            let label = self
                .native_surface
                .lock()
                .webview_label_for_tab(browser_session_id, tab_token)
                .ok_or_else(|| "browser/native-staged-surface-missing".to_string())?;
            let webview = app
                .get_webview(&label)
                .ok_or_else(|| "browser/native-staged-surface-missing".to_string())?;
            platform::bind_browser_core_webview(&webview).await?;
            return Ok(format!("native:{tab_token}"));
        }

        let port = live_port()
            .await
            .ok_or_else(|| "Native browser automation endpoint is not ready".to_string())?;
        if !self.native_surface.lock().owns_port(port) {
            return Err(
                "Automation endpoint does not belong to this app's native browser workspace"
                    .to_string(),
            );
        }
        discover_native_target(port, tab_token).await
    }

    fn rollback_staged_agent_tab(
        &self,
        app: &AppHandle,
        browser_session_id: &str,
        tab_token: &str,
        creation_id: &str,
    ) -> Result<(), String> {
        match self.native_surface.lock().rollback_created_tab(
            Some(app),
            browser_session_id,
            tab_token,
            creation_id,
        )? {
            true => Ok(()),
            false => {
                Err("Hidden candidate tab disappeared before bind-failure compensation".to_string())
            }
        }
    }

    fn rollback_staged_user_tab(
        &self,
        app: &AppHandle,
        browser_session_id: &str,
        tab_token: &str,
    ) -> Result<(), String> {
        match self.native_surface.lock().rollback_user_created_tab(
            Some(app),
            browser_session_id,
            tab_token,
        )? {
            true => Ok(()),
            false => Err(
                "Hidden user candidate tab disappeared before bind-failure compensation"
                    .to_string(),
            ),
        }
    }

    /// Embedded WebView popups are always denied at the engine boundary and recreated as
    /// task-owned tabs. A popup raised inside an already-begun Agent dispatch carries the
    /// complete in-memory host lease through hidden staging and final CAS; a page popup with
    /// no such authorization is deliberately published as User-owned.
    pub(crate) async fn create_popup_tab(
        &self,
        browser_session_id: &str,
        url: String,
        authorization: Option<RetainedAgentOperation>,
    ) -> Result<String, String> {
        // Popup callbacks bypass the hosted-request scanner. Serialize their
        // complete create/bind/publish transaction with restart cleanup.
        let _admission_guard = self.hosted_request_gate.read().await;
        let session_lock = self.session_lifecycle_lock(browser_session_id);
        let _session_guard = session_lock.lock().await;
        let _start_guard = self.start_mtx.lock().await;
        if let Some(retained) = authorization {
            let result = async {
                self.ensure_accepting_browser_work()?;
                if !is_allowed_url(&url) {
                    return Err("Only http, https, and about:blank URLs are supported".to_string());
                }
                let app = self
                    .app
                    .lock()
                    .clone()
                    .ok_or_else(|| "App handle is not ready".to_string())?;
                self.create_agent_popup_tab(&app, browser_session_id, url, &retained)
                    .await
            }
            .await;
            // The callback retained one exact holder before spawning. Release
            // it for every validation/setup/bind/commit outcome, including an
            // invalid URL or missing AppHandle before staging starts.
            self.native_surface
                .lock()
                .release_popup_agent_operation(&retained);
            return result;
        }
        self.ensure_accepting_browser_work()?;
        if !is_allowed_url(&url) {
            return Err("Only http, https, and about:blank URLs are supported".to_string());
        }
        let app = self
            .app
            .lock()
            .clone()
            .ok_or_else(|| "App handle is not ready".to_string())?;
        self.create_native_bound_tab(&app, browser_session_id, url, false)
            .await
    }

    async fn create_agent_popup_tab(
        &self,
        app: &AppHandle,
        browser_session_id: &str,
        url: String,
        retained: &RetainedAgentOperation,
    ) -> Result<String, String> {
        let authorization = retained.authorization();
        if authorization.session_id != browser_session_id {
            return Err("Popup Agent lease does not match the current conversation".to_string());
        }
        let caller_epoch = retained.caller_epoch();
        let session_token = paths::browser_session_token(browser_session_id);
        ensure_hosted_caller_epoch_live(
            browser_session_id,
            &session_token,
            caller_epoch.caller_pid(),
            caller_epoch.wrapper_instance_nonce(),
        )?;
        if !self
            .native_surface
            .lock()
            .authorize_popup_agent_operation(retained)
        {
            return Err("Popup Agent operation holder expired".to_string());
        }
        let tab_token = self.native_surface.lock().generate_tab_token();
        let creation_id = format!(
            "popup-{:016x}{:016x}",
            rand::random::<u64>(),
            rand::random::<u64>()
        );
        let marker = format!("about:blank#pinvou-tab-{tab_token}");
        let created = self.native_surface.lock().create_tab_for_agent(
            app,
            browser_session_id,
            &tab_token,
            &marker,
            false,
            authorization,
            &creation_id,
        )?;
        if created.is_none() {
            return Err(
                "Native browser workspace for the specified conversation does not exist"
                    .to_string(),
            );
        }

        if let Err(error) = ensure_hosted_caller_epoch_live(
            browser_session_id,
            &session_token,
            caller_epoch.caller_pid(),
            caller_epoch.wrapper_instance_nonce(),
        ) {
            return match self.rollback_staged_agent_tab(
                app,
                browser_session_id,
                &tab_token,
                &creation_id,
            ) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
            };
        }
        if !self
            .native_surface
            .lock()
            .authorize_popup_agent_operation(retained)
        {
            return match self.rollback_staged_agent_tab(
                app,
                browser_session_id,
                &tab_token,
                &creation_id,
            ) {
                Ok(()) => Err("Popup Agent operation holder expired".to_string()),
                Err(rollback_error) => Err(format!(
                    "Popup Agent operation holder expired; {rollback_error}"
                )),
            };
        }

        let target_id = match self
            .bind_staged_native_target(app, browser_session_id, &tab_token)
            .await
        {
            Ok(target_id) => target_id,
            Err(error) => {
                return match self.rollback_staged_agent_tab(
                    app,
                    browser_session_id,
                    &tab_token,
                    &creation_id,
                ) {
                    Ok(()) => Err(error),
                    Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
                };
            }
        };
        // The retained operation keeps lease provenance across the async bind,
        // but caller liveness must still be re-read at the final native commit
        // boundary. A SIGKILL immediately after this check remains an explicit
        // acknowledgement-unknown window, not an at-most-once proof.
        if let Err(error) = ensure_hosted_caller_epoch_live(
            browser_session_id,
            &session_token,
            caller_epoch.caller_pid(),
            caller_epoch.wrapper_instance_nonce(),
        ) {
            return match self.rollback_staged_agent_tab(
                app,
                browser_session_id,
                &tab_token,
                &creation_id,
            ) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
            };
        }
        if !self
            .native_surface
            .lock()
            .authorize_popup_agent_operation(retained)
        {
            return match self.rollback_staged_agent_tab(
                app,
                browser_session_id,
                &tab_token,
                &creation_id,
            ) {
                Ok(()) => Err("Popup Agent operation holder expired".to_string()),
                Err(rollback_error) => Err(format!(
                    "Popup Agent operation holder expired; {rollback_error}"
                )),
            };
        }
        if !self.native_surface.lock().commit_created_tab_for_agent(
            app,
            browser_session_id,
            &tab_token,
            &target_id,
            &url,
            false,
            authorization,
            &creation_id,
            Some(retained),
            || {
                ensure_hosted_caller_epoch_live(
                    browser_session_id,
                    &session_token,
                    caller_epoch.caller_pid(),
                    caller_epoch.wrapper_instance_nonce(),
                )
            },
        )? {
            return Err("Popup tab closed before commit".to_string());
        }
        let _ = app.emit(
            "browser:tabs-changed",
            json!({ "sessionId": browser_session_id, "tab": tab_token }),
        );
        self.persist_native_restore_best_effort(browser_session_id);
        Ok(tab_token)
    }

    async fn create_native_bound_tab(
        &self,
        app: &AppHandle,
        browser_session_id: &str,
        url: String,
        background: bool,
    ) -> Result<String, String> {
        let tab_token = self.native_surface.lock().generate_tab_token();
        // Always create on a private marker first. Publishing/navigating a
        // remote page before target binding would make the strict v2 host
        // mapping disappear and break every Agent tool after the user clicks
        // the toolbar "+" button.
        let marker = format!("about:blank#pinvou-tab-{tab_token}");
        let created = {
            let mut surface = self.native_surface.lock();
            surface.create_tab(app, browser_session_id, &tab_token, &marker, background)?
        };
        let Some(created) = created else {
            return Err(
                "Native browser workspace for the specified conversation does not exist"
                    .to_string(),
            );
        };
        let binding = async {
            let target_id = self
                .bind_staged_native_target(app, browser_session_id, &tab_token)
                .await?;
            if !self.native_surface.lock().bind_target(
                browser_session_id,
                &tab_token,
                &target_id,
            )? {
                return Err("New tab closed before binding its automation target".to_string());
            }
            if !self.native_surface.lock().navigate_tab_after_bind(
                Some(app),
                browser_session_id,
                &tab_token,
                &url,
            )? {
                return Err("New tab closed before navigation".to_string());
            }
            Ok::<(), String>(())
        }
        .await;
        if let Err(error) = binding {
            return match self.rollback_staged_user_tab(app, browser_session_id, &tab_token) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
            };
        }
        let _ = app.emit(
            "browser:tabs-changed",
            json!({ "sessionId": browser_session_id, "tab": created }),
        );
        self.persist_native_restore_best_effort(browser_session_id);
        Ok(created)
    }

    /// Create a tab in the specified conversation's native browser workspace.
    pub async fn create_tab(
        &self,
        browser_session_id: &str,
        url: String,
        background: bool,
    ) -> Result<String, String> {
        // Toolbar tab creation is also a direct WebView creator, so it joins
        // the same lifecycle gate as prepare and page popup creation.
        let _admission_guard = self.hosted_request_gate.read().await;
        let session_lock = self.session_lifecycle_lock(browser_session_id);
        let _session_guard = session_lock.lock().await;
        let _start_guard = self.start_mtx.lock().await;
        self.ensure_accepting_browser_work()?;
        // Use the same scheme allowlist as navigate to prevent injection of local
        // or script schemes such as file:// and javascript:.
        if !is_allowed_url(&url) {
            return Err("Only http, https, and about:blank URLs are supported".to_string());
        }
        let app = self
            .app
            .lock()
            .clone()
            .ok_or_else(|| "App handle is not ready".to_string())?;
        self.create_native_bound_tab(&app, browser_session_id, url, background)
            .await
    }

    /// Close a tab. Switch to the first survivor only when closing the active tab;
    /// closing a background tab must not move the page the user is viewing.
    pub async fn close_tab(
        &self,
        browser_session_id: &str,
        target_id: String,
    ) -> Result<(), String> {
        let app = self.app.lock().clone();
        if self
            .native_surface
            .lock()
            .close_tab(app.as_ref(), browser_session_id, &target_id)?
        {
            if let Some(app) = app {
                let _ = app.emit(
                    "browser:tabs-changed",
                    json!({ "sessionId": browser_session_id }),
                );
            }
            self.persist_native_restore_best_effort(browser_session_id);
            return Ok(());
        }
        Err(
            "Native browser workspace for the specified conversation or tab does not exist"
                .to_string(),
        )
    }

    /// Switch the active tab by targetId.
    pub async fn activate_tab(
        &self,
        browser_session_id: &str,
        target_id: String,
    ) -> Result<(), String> {
        let app = self.app.lock().clone();
        if self
            .native_surface
            .lock()
            .activate_tab(app.as_ref(), browser_session_id, &target_id)?
        {
            if let Some(app) = app {
                let _ = app.emit(
                    "browser:tabs-changed",
                    json!({ "sessionId": browser_session_id, "tab": target_id }),
                );
            }
            self.persist_native_restore_best_effort(browser_session_id);
            return Ok(());
        }
        Err(
            "Native browser workspace for the specified conversation or tab does not exist"
                .to_string(),
        )
    }

    // -----------------------------------------------------------------------
    // Navigation / interaction
    // -----------------------------------------------------------------------

    /// Navigate to the specified URL.
    pub async fn navigate(
        &self,
        browser_session_id: &str,
        url: String,
        request_id: &str,
    ) -> Result<(), String> {
        if !is_allowed_url(&url) {
            return Err("Only http, https, and about:blank URLs are supported".to_string());
        }
        let app = self.app.lock().clone();
        if self.native_surface.lock().navigate(
            app.as_ref(),
            browser_session_id,
            &url,
            request_id,
        )? {
            return Ok(());
        }
        Err("Native browser workspace for the specified conversation does not exist".to_string())
    }

    pub async fn go_back(&self, browser_session_id: &str) -> Result<(), String> {
        self.history_step(browser_session_id, -1).await
    }

    pub async fn go_forward(&self, browser_session_id: &str) -> Result<(), String> {
        self.history_step(browser_session_id, 1).await
    }

    async fn history_step(&self, browser_session_id: &str, delta: i64) -> Result<(), String> {
        let app = self.app.lock().clone();
        if self.native_surface.lock().history_step(
            app.as_ref(),
            browser_session_id,
            delta.signum() as i8,
        )? {
            return Ok(());
        }
        Err("Native browser workspace for the specified conversation does not exist".to_string())
    }

    pub async fn reload(&self, browser_session_id: &str) -> Result<(), String> {
        let app = self.app.lock().clone();
        if self
            .native_surface
            .lock()
            .reload(app.as_ref(), browser_session_id)?
        {
            return Ok(());
        }
        Err("Native browser workspace for the specified conversation does not exist".to_string())
    }
}

// ---------------------------------------------------------------------------
// Event loop: maintain only internal target lifecycle state. The native host
// emits UI-facing navigation, title, and tab events by sessionId. Never broadcast
// taskless events from the global CDP connection.
// ---------------------------------------------------------------------------
enum TargetLifecycleReconcileOutcome {
    Reconciled,
    ConnectionInvalidated(String),
}

async fn run_event_loop(app: AppHandle, mut events: tokio::sync::mpsc::Receiver<cdp::CdpEvent>) {
    use cdp::CdpEvent;
    while let Some(ev) = events.recv().await {
        match ev {
            CdpEvent::Event { method, params } => match method.as_str() {
                "Target.targetCreated" | "Target.targetDestroyed" => {
                    // See route_target_event for protocol-shape differences:
                    // created carries full targetInfo for filtering non-page targets,
                    // while destroyed carries only { targetId }.
                    match route_target_event(&method, &params) {
                        TargetEventRoute::Ignore => continue,
                        // When MCP or page script destroys the active page, repair
                        // selection before notifying the frontend.
                        TargetEventRoute::Destroy(tid) => {
                            app.state::<BrowserManager>()
                                .on_target_destroyed(&tid)
                                .await;
                        }
                        // If the model creates a tab after all tabs closed,
                        // automatically activate the new page.
                        TargetEventRoute::Create(tid) => {
                            app.state::<BrowserManager>().on_target_created(&tid).await;
                        }
                    }
                }
                _ => {}
            },
            CdpEvent::LifecycleResync { signal } => {
                // Rearm before the network snapshot: any overflow after this
                // receive schedules one later reconciliation, while all drops
                // before it are covered by the snapshot below.
                signal.begin_consume();
                match app
                    .state::<BrowserManager>()
                    .reconcile_target_lifecycle()
                    .await
                {
                    Ok(TargetLifecycleReconcileOutcome::Reconciled) => {}
                    Ok(TargetLifecycleReconcileOutcome::ConnectionInvalidated(error)) => {
                        eprintln!("[browser] {error}; reset CDP connection and waiting for automatic reconnect");
                        break;
                    }
                    Err(error) => eprintln!("[browser] {error}"),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Internal free functions for call sites without &self.
// ---------------------------------------------------------------------------

fn reconcile_browser_session_file_dirs(
    active_tokens: &HashSet<String>,
    directories: &[PathBuf],
    startup_cutoff: SystemTime,
) -> Result<(), String> {
    let mut errors = Vec::new();
    for directory in directories {
        let anchored =
            match crate::platform::filesystem::open_existing_private_file_directory(directory) {
                Ok(Some(directory)) => directory,
                Ok(None) => continue,
                Err(error) => {
                    errors.push(format!(
                        "Failed to open browser session directory {}: {error}",
                        directory.display()
                    ));
                    continue;
                }
            };
        let entries = match anchored.entry_names() {
            Ok(entries) => entries,
            Err(error) => {
                errors.push(format!(
                    "Failed to enumerate browser session directory {}: {error}",
                    directory.display()
                ));
                continue;
            }
        };
        for entry_name in entries {
            let path = directory.join(&entry_name);
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Some(token) = path.file_stem().and_then(|value| value.to_str()) else {
                errors.push(format!(
                    "Browser session filename is not valid UTF-8: {}",
                    path.display()
                ));
                continue;
            };
            if active_tokens.contains(token) {
                continue;
            }
            let file = match anchored.open_plain_file(&entry_name) {
                Ok(Some(file)) => file,
                Ok(None) => continue,
                Err(error) => {
                    errors.push(format!(
                        "Failed to open stable orphaned browser file {}: {error}",
                        path.display()
                    ));
                    continue;
                }
            };
            let modified = match file.metadata().and_then(|metadata| metadata.modified()) {
                Ok(modified) => modified,
                Err(error) => {
                    // Retain fail-safely when previous-process ownership cannot
                    // be proven and retry on the next startup.
                    errors.push(format!(
                        "Failed to read browser file timestamp {}: {error}",
                        path.display()
                    ));
                    continue;
                }
            };
            if modified >= startup_cutoff {
                // The current process may have created this file after the static
                // active-session snapshot. Background reconciliation must never
                // use a stale token set to delete new task state.
                continue;
            }
            drop(file);
            if let Err(error) = anchored.remove_plain_file(&entry_name) {
                errors.push(format!(
                    "Failed to delete orphaned browser file {}: {error}",
                    path.display()
                ));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn private_directory_modified_at_or_after(
    directory: &PrivateFileDirectory,
    startup_cutoff: SystemTime,
    child_depth: usize,
) -> Result<bool, String> {
    if directory
        .metadata()
        .and_then(|metadata| metadata.modified())
        .map_err(|error| {
            format!("Failed to inspect browser Prepare quarantine timestamp: {error}")
        })?
        >= startup_cutoff
    {
        return Ok(true);
    }
    for name in directory.entry_names().map_err(|error| {
        format!("Failed to enumerate browser Prepare quarantine directory: {error}")
    })? {
        match directory.open_child_directory(&name) {
            Ok(child) if child_depth > 0 => {
                if private_directory_modified_at_or_after(&child, startup_cutoff, child_depth - 1)?
                {
                    return Ok(true);
                }
            }
            Ok(_) => {
                return Err("Browser Prepare quarantine nesting is deeper than expected".to_string())
            }
            Err(_) => {
                let file = directory.open_plain_file(&name).map_err(|error| {
                    format!("Browser Prepare quarantine entry is not a stable file: {error}")
                })?;
                let Some(file) = file else {
                    return Err(
                        "Browser Prepare quarantine entry changed during inspection".to_string()
                    );
                };
                if file
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .map_err(|error| {
                        format!("Failed to inspect browser Prepare quarantine timestamp: {error}")
                    })?
                    >= startup_cutoff
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn reconcile_unassigned_hosted_prepare_quarantine(
    root: &PrivateFileDirectory,
    startup_cutoff: SystemTime,
) -> Result<(), String> {
    let directory = match root.open_child_directory(OsStr::new("unassigned")) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Failed to open unassigned browser Prepare quarantine: {error}"
            ))
        }
    };
    let mut slots = directory.entry_names().map_err(|error| {
        format!("Failed to enumerate unassigned browser Prepare quarantine: {error}")
    })?;
    slots.sort();
    let mut errors = Vec::new();
    for slot_name in slots {
        let Some(name) = slot_name.to_str() else {
            errors.push("Browser Prepare quarantine slot name is not valid UTF-8".to_string());
            continue;
        };
        if !valid_prepare_quarantine_token(name) {
            continue;
        }
        let result = (|| {
            let slot = directory
                .open_child_directory(&slot_name)
                .map_err(|error| {
                    format!("Failed to open browser Prepare quarantine slot {name}: {error}")
                })?;
            if private_directory_modified_at_or_after(&slot, startup_cutoff, 0)? {
                return Ok(());
            }
            drop(slot);
            remove_hosted_prepare_quarantine_slot(&directory, &slot_name, true)
        })();
        if let Err(error) = result {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn reconcile_hosted_prepare_quarantine_files(
    active_tokens: &HashSet<String>,
    startup_cutoff: SystemTime,
) -> Result<(), String> {
    let Some(root) = crate::platform::filesystem::open_existing_private_file_directory(
        &hosted_prepare_quarantine_dir(),
    )
    .map_err(|error| {
        format!(
            "Failed to open browser Prepare quarantine directory {}: {error}",
            hosted_prepare_quarantine_dir().display()
        )
    })?
    else {
        return Ok(());
    };
    let mut errors = Vec::new();
    for name in root.entry_names().map_err(|error| {
        format!("Failed to enumerate browser Prepare quarantine directory: {error}")
    })? {
        let Some(name_text) = name.to_str() else {
            continue;
        };
        let result = if name_text == "unassigned" {
            reconcile_unassigned_hosted_prepare_quarantine(&root, startup_cutoff)
        } else if valid_prepare_quarantine_token(name_text) {
            if active_tokens.contains(name_text) {
                Ok(())
            } else {
                (|| {
                    let token_directory = root.open_child_directory(&name).map_err(|error| {
                        format!(
                            "Failed to open browser Prepare token quarantine {name_text}: {error}"
                        )
                    })?;
                    if private_directory_modified_at_or_after(&token_directory, startup_cutoff, 1)?
                    {
                        return Ok(());
                    }
                    drop(token_directory);
                    remove_hosted_prepare_quarantine_for_token_from_root(&root, name_text)
                })()
            }
        } else {
            // Unexpected names never grant task identity or deletion authority.
            Ok(())
        };
        if let Err(error) = result {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Route browser-level Target events. Pure for protocol-shape unit tests.
///
/// Observed/documented protocol-shape difference: `Target.targetCreated` params
/// carry complete `targetInfo`, allowing type-based filtering of non-page targets
/// such as iframe/worker before their event storms reach the internal state
/// machine. `Target.targetDestroyed` params contain only `{ targetId }`, without
/// targetInfo. The CDP reader filters non-page destruction using the page-ID set
/// maintained from targetCreated; this function must read top-level targetId and
/// never require the nonexistent targetInfo.type.
#[derive(Debug, PartialEq, Eq)]
enum TargetEventRoute {
    Create(String),
    Destroy(String),
    Ignore,
}

fn reconciled_active_target<'a>(
    current: Option<&str>,
    live_targets: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    let live_targets = live_targets.into_iter().collect::<Vec<_>>();
    current
        .filter(|current| live_targets.contains(current))
        .or_else(|| live_targets.first().copied())
        .map(str::to_string)
}

fn merge_reconciled_page_sessions(
    pages: &PageSessions,
    captured_targets: &HashSet<String>,
    live_sessions: &HashMap<String, String>,
) {
    let mut pages = pages.lock();
    // Remove only stale entries that belonged to the cache generation used by
    // this snapshot. A target attached concurrently after the snapshot began
    // must not be erased merely because it was absent from the older response.
    pages.retain(|target_id, _| {
        !captured_targets.contains(target_id) || live_sessions.contains_key(target_id)
    });
    pages.extend(
        live_sessions
            .iter()
            .map(|(target_id, session_id)| (target_id.clone(), session_id.clone())),
    );
}

fn route_target_event(method: &str, params: &Value) -> TargetEventRoute {
    match method {
        "Target.targetCreated" => {
            let info = params.get("targetInfo");
            if info.and_then(|i| i.get("type")).and_then(Value::as_str) != Some("page") {
                return TargetEventRoute::Ignore;
            }
            match info.and_then(|i| i.get("targetId")).and_then(Value::as_str) {
                Some(tid) => TargetEventRoute::Create(tid.to_string()),
                None => TargetEventRoute::Ignore,
            }
        }
        "Target.targetDestroyed" => match params.get("targetId").and_then(Value::as_str) {
            Some(tid) => TargetEventRoute::Destroy(tid.to_string()),
            None => TargetEventRoute::Ignore,
        },
        _ => TargetEventRoute::Ignore,
    }
}

/// URL-scheme allowlist shared by navigation/new-tab UI and host WebView
/// callbacks: case-insensitive http, https, and about:blank, matching frontend
/// address preflight `/^https?:\/\//i`. Reject local/script schemes including
/// file, javascript, data, and chrome. Published and unpublished native pages use
/// the same callback validation, so MCP navigation cannot bypass privileged Tauri
/// release or development origins.
fn is_allowed_url(url: &str) -> bool {
    let Ok(parsed) = tauri::Url::parse(url) else {
        return false;
    };
    match parsed.scheme() {
        // Tauri serves the privileged application UI from `tauri.localhost`
        // in Windows release builds and from the configured Vite origin while
        // developing. A remote browser tab must never turn itself into a
        // second local application UI. The capability manifest separately
        // scopes IPC to named application webviews as a defense in depth.
        "http" | "https" => parsed.host_str().is_some_and(|host| {
            let reserved_release_origin = host.eq_ignore_ascii_case("tauri.localhost");
            let reserved_dev_host = host.eq_ignore_ascii_case("localhost")
                || host == "127.0.0.1"
                || host == "[::1]"
                || host == "::1";
            let reserved_dev_origin =
                reserved_dev_host && parsed.port_or_known_default() == Some(1420);
            !reserved_release_origin && !reserved_dev_origin
        }),
        // A WebView tab stores a local random marker in the fragment while
        // remaining about:blank. Other internal pages such as about:config and
        // about:srcdoc remain denied.
        "about" => parsed.path() == "blank" && parsed.query().is_none(),
        _ => false,
    }
}

/// Attach a page target and reuse its cached flattened session. CDP creates an
/// independent session for every attach to the same target and does not release
/// it automatically. Without caching, high-frequency enumeration on every
/// tabs-changed frontend refresh leaks Chrome-side sessions without bound.
async fn attach_page_cached(
    session: &CdpSession,
    pages: &PageSessions,
    target_id: &str,
) -> Result<String, String> {
    if let Some(sid) = pages.lock().get(target_id) {
        return Ok(sid.clone());
    }
    let sid = session
        .call(
            None,
            "Target.attachToTarget",
            json!({ "targetId": target_id, "flatten": true }),
        )
        .await
        .map_err(|e| format!("attach failed: {e}"))?
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if sid.is_empty() {
        return Err("attachToTarget did not return sessionId".to_string());
    }
    pages.lock().insert(target_id.to_string(), sid.clone());
    Ok(sid)
}

async fn attach_first_page_cached(
    session: &CdpSession,
    pages: &PageSessions,
) -> Result<(String, String), String> {
    let targets = session
        .call(None, "Target.getTargets", json!({}))
        .await
        .map_err(|e| format!("Target.getTargets failed: {e}"))?;
    let mut page_id: Option<String> = None;
    if let Some(infos) = targets.get("targetInfos").and_then(Value::as_array) {
        for info in infos {
            if info.get("type").and_then(Value::as_str) == Some("page") {
                page_id = info
                    .get("targetId")
                    .and_then(Value::as_str)
                    .map(String::from);
                break;
            }
        }
    }
    let target_id = match page_id {
        Some(id) => id,
        None => {
            let v = session
                .call(None, "Target.createTarget", json!({ "url": "about:blank" }))
                .await
                .map_err(|e| format!("Target.createTarget failed: {e}"))?;
            v.get("targetId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        }
    };
    let sid = attach_page_cached(session, pages, &target_id).await?;
    Ok((target_id, sid))
}

/// Precisely bind a newly host-created, not-yet-navigated about:blank WebView to
/// its CDP target. The token exists only on a one-time internal blank page. Match
/// URL fragment first; if WebView2 omits it from the Target list, read the host
/// initialization script's bootstrap marker only on about:blank candidates. No
/// http(s) page is ever a binding source.
async fn discover_native_target(port: u16, tab_token: &str) -> Result<String, String> {
    let connected = cdp::connect(port).await.map_err(|error| {
        format!("Failed to connect to native-page automation endpoint: {error:#}")
    })?;
    let session = connected.session;
    let result = async {
        let targets = session
            .call(None, "Target.getTargets", json!({}))
            .await
            .map_err(|error| format!("Failed to enumerate native-page targets: {error}"))?;
        let expected_session_marker = format!("#pinvou-session-{tab_token}");
        let expected_tab_marker = format!("#pinvou-tab-{tab_token}");
        let mut matches = Vec::new();
        for info in targets
            .get("targetInfos")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if info.get("type").and_then(Value::as_str) != Some("page") {
                continue;
            }
            let Some(target_id) = info.get("targetId").and_then(Value::as_str) else {
                continue;
            };
            let url = info.get("url").and_then(Value::as_str).unwrap_or("");
            if url.contains(&expected_session_marker) || url.contains(&expected_tab_marker) {
                matches.push(target_id.to_string());
                continue;
            }
            // A remote page cannot forge ownership by defining a same-name global.
            if url != "about:blank" && !url.starts_with("about:blank#") {
                continue;
            }
            let attached = session
                .call(
                    None,
                    "Target.attachToTarget",
                    json!({ "targetId": target_id, "flatten": true }),
                )
                .await
                .map_err(|error| format!("Failed to bind native-page target: {error}"))?;
            let sid = attached
                .get("sessionId")
                .and_then(Value::as_str)
                .ok_or_else(|| "Binding native-page target did not return sessionId".to_string())?;
            let marker = session
                .call(
                    Some(sid),
                    "Runtime.evaluate",
                    json!({
                        "expression": "globalThis.__PINVOU_BROWSER_BOOTSTRAP_TOKEN__ || null",
                        "returnByValue": true,
                    }),
                )
                .await
                .map_err(|error| format!("Failed to read native-page bootstrap marker: {error}"))?;
            if marker.pointer("/result/value").and_then(Value::as_str) == Some(tab_token) {
                matches.push(target_id.to_string());
            }
        }
        matches.sort();
        matches.dedup();
        match matches.as_slice() {
            [target_id] => Ok(target_id.clone()),
            [] => Err("No unique automation target matches the host-created page".to_string()),
            _ => Err(
                "Host-page bootstrap marker matches multiple automation targets; binding denied"
                    .to_string(),
            ),
        }
    }
    .await;
    let _ = session.close().await;
    connected.reader_task.abort();
    result
}

/// Return the port only when its file is valid and a live CDP probe succeeds.
async fn live_port() -> Option<u16> {
    let raw = std::fs::read_to_string(paths::browser_cdp_port_json()).ok()?;
    let p = parse_host_owned_port_json(&raw)?;
    probe_cdp(p, Duration::from_millis(800)).await.then_some(p)
}

async fn list_page_tabs(
    session: &CdpSession,
    pages: &PageSessions,
) -> Result<Vec<TabInfo>, String> {
    list_page_tabs_with_policy(session, pages, PageTabAttachPolicy::BestEffort).await
}

/// Lifecycle reconciliation must distinguish a truly destroyed page from a
/// still-live page whose flatten-session attach failed temporarily. Treating
/// the latter as absent would make an incomplete snapshot delete a live cache
/// entry or active target, so this variant fails the entire authoritative
/// snapshot and lets the caller reconnect.
async fn list_page_tabs_authoritative(
    session: &CdpSession,
    pages: &PageSessions,
) -> Result<Vec<TabInfo>, String> {
    list_page_tabs_with_policy(session, pages, PageTabAttachPolicy::Authoritative).await
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PageTabAttachPolicy {
    BestEffort,
    Authoritative,
}

fn accept_page_attachment(
    policy: PageTabAttachPolicy,
    target_id: &str,
    result: Result<String, String>,
) -> Result<bool, String> {
    match result {
        Ok(_) => Ok(true),
        Err(_) if policy == PageTabAttachPolicy::BestEffort => Ok(false),
        Err(error) => Err(format!(
            "Authoritative Target.getTargets snapshot cannot attach live page {target_id}: {error}"
        )),
    }
}

async fn list_page_tabs_with_policy(
    session: &CdpSession,
    pages: &PageSessions,
    policy: PageTabAttachPolicy,
) -> Result<Vec<TabInfo>, String> {
    let targets = session
        .call(None, "Target.getTargets", json!({}))
        .await
        .map_err(|e| format!("Target.getTargets failed: {e}"))?;
    let mut tabs = Vec::new();
    let Some(infos) = targets.get("targetInfos").and_then(Value::as_array) else {
        return if policy == PageTabAttachPolicy::Authoritative {
            Err("Authoritative Target.getTargets snapshot is missing targetInfos".to_string())
        } else {
            Ok(tabs)
        };
    };
    for info in infos {
        if info.get("type").and_then(Value::as_str) != Some("page") {
            continue;
        }
        let target_id = info
            .get("targetId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if target_id.is_empty() {
            if policy == PageTabAttachPolicy::Authoritative {
                return Err(
                    "Live page in authoritative Target.getTargets snapshot is missing targetId"
                        .to_string(),
                );
            }
            continue;
        }
        // Reuse cached attaches. Enumeration occurs on every tab addition/removal;
        // without caching, each pass creates a flattened session that CDP never
        // releases automatically, producing an unbounded leak.
        if !accept_page_attachment(
            policy,
            &target_id,
            attach_page_cached(session, pages, &target_id).await,
        )? {
            continue;
        }
        tabs.push(TabInfo {
            target_id,
            page_id: None,
            title: info
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            url: info
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        });
    }
    Ok(tabs)
}

/// Switch the flattened session used for Agent automation. The native WebView of
/// the same page renders directly for the user, so enable only page protocol
/// domains here and never start a continuous screenshot stream.
async fn switch_active_session_locked(inner: &mut Inner, sid: &str) -> Result<(), String> {
    let session = inner
        .session
        .as_ref()
        .ok_or_else(|| "Browser is not started".to_string())?;
    session
        .call(Some(sid), "Page.enable", json!({}))
        .await
        .map_err(|e| format!("Page.enable failed: {e}"))?;
    inner.active_session = Some(sid.to_string());
    Ok(())
}

// ---------------------------------------------------------------------------
// Native-host automation endpoint coordination
// ---------------------------------------------------------------------------

fn valid_host_token(value: &str) -> bool {
    value.len() == 16 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_host_request_id(value: &str) -> bool {
    (1..=160).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_wrapper_instance_nonce(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn validate_hosted_caller_identity(
    caller_pid: u32,
    wrapper_instance_nonce: &str,
) -> Result<(), String> {
    if caller_pid == 0 || !valid_wrapper_instance_nonce(wrapper_instance_nonce) {
        return Err("Browser host caller epoch identity is invalid".to_string());
    }
    Ok(())
}

fn valid_browser_session_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn validate_hosted_identity(
    protocol_version: u8,
    session_id: &str,
    session_token: &str,
    request_id: &str,
    idempotency_key: &str,
    path: &std::path::Path,
    extension: &str,
) -> Result<(), String> {
    if protocol_version != 3 {
        return Err("Browser host protocol version is unsupported".to_string());
    }
    if !valid_browser_session_id(session_id)
        || !valid_host_token(session_token)
        || paths::browser_session_token(session_id) != session_token
    {
        return Err("Browser host request session identity validation failed".to_string());
    }
    if !valid_host_request_id(request_id) {
        return Err("Browser host request_id is invalid".to_string());
    }
    if idempotency_key != format!("{session_token}/{request_id}") {
        return Err("Browser host idempotency_key is invalid".to_string());
    }
    let expected_name = format!("{session_token}-{request_id}.{extension}");
    if path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str()) {
        return Err("Browser host request filename does not match request identity".to_string());
    }
    Ok(())
}

fn validate_hosted_request(
    request: &HostedBrowserRequest,
    path: &std::path::Path,
) -> Result<(), String> {
    validate_hosted_identity(
        request.protocol_version,
        &request.session_id,
        &request.session_token,
        &request.request_id,
        &request.idempotency_key,
        path,
        "json",
    )?;
    if request.operation.as_str().is_empty() {
        return Err("Browser host operation is invalid".to_string());
    }
    validate_hosted_caller_identity(request.caller_pid, &request.wrapper_instance_nonce)?;
    let now_ms = hosted_protocol_now_ms()?;
    validate_hosted_request_freshness_at(request, now_ms)?;
    Ok(())
}

fn hosted_protocol_now_ms() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| "System time predates Unix epoch; rejecting browser host request".to_string())?
        .as_millis()
        .try_into()
        .map_err(|_| "System time exceeds browser host protocol range".to_string())
}

fn validate_hosted_request_freshness_at(
    request: &HostedBrowserRequest,
    now_ms: u64,
) -> Result<(), String> {
    const CLOCK_SKEW_TOLERANCE_MS: u64 = 5_000;
    if request.requested_at == 0
        || request.requested_at > now_ms.saturating_add(CLOCK_SKEW_TOLERANCE_MS)
    {
        return Err("Browser host request timestamp is invalid".to_string());
    }
    if now_ms.saturating_sub(request.requested_at) > request.operation.maximum_artifact_age_ms() {
        return Err("browser/host-request-expired: caller is no longer live".to_string());
    }
    Ok(())
}

fn hosted_caller_heartbeat_path_for(session_token: &str, wrapper_instance_nonce: &str) -> PathBuf {
    paths::browser_host_requests_dir().join(format!(
        "{}-{}.heartbeat",
        session_token, wrapper_instance_nonce
    ))
}

fn hosted_caller_heartbeat_path(request: &HostedBrowserRequest) -> PathBuf {
    hosted_caller_heartbeat_path_for(&request.session_token, &request.wrapper_instance_nonce)
}

fn hosted_cancellation_path(request: &HostedBrowserRequest) -> PathBuf {
    paths::browser_host_requests_dir().join(format!(
        "{}-{}.cancelled",
        request.session_token, request.request_id
    ))
}

fn hosted_prepare_journal_dir() -> PathBuf {
    paths::browser_home().join("prepare-journal")
}

fn hosted_prepare_journal_path_for(session_token: &str) -> PathBuf {
    hosted_prepare_journal_dir().join(format!("{session_token}.json"))
}

fn hosted_prepare_quarantine_dir() -> PathBuf {
    paths::browser_home().join("prepare-quarantine")
}

fn hosted_prepare_quarantine_token_dir(session_token: &str) -> PathBuf {
    hosted_prepare_quarantine_dir().join(session_token)
}

fn hosted_prepare_unassigned_quarantine_dir() -> PathBuf {
    hosted_prepare_quarantine_dir().join("unassigned")
}

fn hosted_prepare_quarantine_slot_dir(parent: &Path, sequence: u64) -> PathBuf {
    parent.join(format!("{sequence:016x}"))
}

fn hosted_prepare_quarantine_state_path(slot: &Path, state_kind: &str) -> PathBuf {
    slot.join(state_kind)
}

fn valid_prepare_quarantine_token(value: &str) -> bool {
    value.len() == 16
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn prepare_journal_token_from_path(path: &Path) -> Result<Option<String>, String> {
    if path.parent() != Some(hosted_prepare_journal_dir().as_path()) {
        return Err("Browser Prepare journal path is outside the journal directory".to_string());
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(|file_name| file_name.strip_suffix(".json"));
    let Some(token) = file_name.filter(|value| valid_prepare_quarantine_token(value)) else {
        return Ok(None);
    };
    if path != hosted_prepare_journal_path_for(token) {
        return Err("Browser Prepare journal path is outside its canonical token slot".to_string());
    }
    Ok(Some(token.to_string()))
}

fn hosted_prepare_journal_path(request: &HostedBrowserRequest) -> PathBuf {
    hosted_prepare_journal_path_for(&request.session_token)
}

impl HostedPrepareCompensation {
    fn from_request(request: &HostedBrowserRequest, rollback_kind: &str) -> Self {
        Self {
            protocol_version: request.protocol_version,
            kind: "host_prepare_compensation".to_string(),
            request_id: request.request_id.clone(),
            idempotency_key: request.idempotency_key.clone(),
            session_id: request.session_id.clone(),
            session_token: request.session_token.clone(),
            caller_pid: request.caller_pid,
            wrapper_instance_nonce: request.wrapper_instance_nonce.clone(),
            rollback_kind: rollback_kind.to_string(),
            revision: None,
        }
    }

    fn rollback_value(&self) -> Option<Value> {
        let revision = self.revision?;
        matches!(
            self.rollback_kind.as_str(),
            "prepared_session" | "restored_session"
        )
        .then(|| {
            json!({
                "kind": self.rollback_kind,
                "session_id": self.session_id,
                "request_id": self.request_id,
                "revision": revision,
            })
        })
    }
}

fn new_hosted_prepare_journal(
    request: &HostedBrowserRequest,
    rollback_kind: &str,
    now_ms: u64,
) -> HostedPrepareJournal {
    HostedPrepareJournal {
        protocol_version: request.protocol_version,
        kind: "host_prepare_journal".to_string(),
        phase: HostedPreparePhase::Pending,
        compensation: HostedPrepareCompensation::from_request(request, rollback_kind),
        requested_at: request.requested_at,
        updated_at: now_ms,
        response: None,
    }
}

fn write_hosted_prepare_journal(journal: &HostedPrepareJournal) -> Result<(), String> {
    validate_hosted_prepare_journal(
        journal,
        &hosted_prepare_journal_path_for(&journal.compensation.session_token),
    )?;
    let path = hosted_prepare_journal_path_for(&journal.compensation.session_token);
    let name = path
        .file_name()
        .ok_or_else(|| "Browser Prepare journal has no filename".to_string())?;
    let directory =
        crate::platform::filesystem::open_private_file_directory(&hosted_prepare_journal_dir())
            .map_err(|error| {
                format!(
                    "Failed to open browser Prepare journal directory {}: {error}",
                    hosted_prepare_journal_dir().display()
                )
            })?;
    let encoded = serde_json::to_vec(journal)
        .map_err(|error| format!("Failed to encode browser Prepare journal: {error}"))?;
    directory
        .atomic_write_private_file(name, &encoded)
        .map_err(|error| {
            format!(
                "Failed to write browser Prepare journal {}: {error}",
                path.display()
            )
        })
}

fn read_hosted_prepare_journal(path: &Path) -> Result<HostedPrepareJournal, String> {
    if path.parent() != Some(hosted_prepare_journal_dir().as_path()) {
        return Err("Browser Prepare journal path is outside the journal directory".to_string());
    }
    let name = path
        .file_name()
        .ok_or_else(|| "Browser Prepare journal has no filename".to_string())?;
    let directory = crate::platform::filesystem::open_existing_private_file_directory(
        &hosted_prepare_journal_dir(),
    )
    .map_err(|error| {
        format!(
            "Failed to open browser Prepare journal directory {}: {error}",
            hosted_prepare_journal_dir().display()
        )
    })?
    .ok_or_else(|| {
        format!(
            "Browser Prepare journal directory is missing: {}",
            hosted_prepare_journal_dir().display()
        )
    })?;
    read_hosted_prepare_journal_from_directory(&directory, name, path)
}

fn read_hosted_prepare_journal_if_present(
    path: &Path,
) -> Result<Option<HostedPrepareJournal>, String> {
    if path.parent() != Some(hosted_prepare_journal_dir().as_path()) {
        return Err("Browser Prepare journal path is outside the journal directory".to_string());
    }
    let name = path
        .file_name()
        .ok_or_else(|| "Browser Prepare journal has no filename".to_string())?;
    let Some(directory) = crate::platform::filesystem::open_existing_private_file_directory(
        &hosted_prepare_journal_dir(),
    )
    .map_err(|error| {
        format!(
            "Failed to open browser Prepare journal directory {}: {error}",
            hosted_prepare_journal_dir().display()
        )
    })?
    else {
        return Ok(None);
    };
    let Some(file) = directory.open_plain_file(name).map_err(|error| {
        format!(
            "Failed to open browser Prepare journal {}: {error}",
            path.display()
        )
    })?
    else {
        return Ok(None);
    };
    decode_hosted_prepare_journal(file, path).map(Some)
}

fn read_hosted_prepare_journal_from_directory(
    directory: &PrivateFileDirectory,
    name: &OsStr,
    validation_path: &Path,
) -> Result<HostedPrepareJournal, String> {
    let file = directory
        .open_plain_file(name)
        .map_err(|error| {
            format!(
                "Failed to open browser Prepare journal {}: {error}",
                validation_path.display()
            )
        })?
        .ok_or_else(|| {
            format!(
                "Browser Prepare journal is missing: {}",
                validation_path.display()
            )
        })?;
    decode_hosted_prepare_journal(file, validation_path)
}

fn decode_hosted_prepare_journal(
    mut file: std::fs::File,
    validation_path: &Path,
) -> Result<HostedPrepareJournal, String> {
    let mut raw = String::new();
    file.read_to_string(&mut raw).map_err(|error| {
        format!(
            "Failed to read browser Prepare journal {}: {error}",
            validation_path.display()
        )
    })?;
    let journal: HostedPrepareJournal = serde_json::from_str(&raw)
        .map_err(|error| format!("Browser Prepare journal has an invalid format: {error}"))?;
    validate_hosted_prepare_journal(&journal, validation_path)?;
    Ok(journal)
}

fn remove_hosted_prepare_journal(path: &Path) -> Result<(), String> {
    if path.parent() != Some(hosted_prepare_journal_dir().as_path()) {
        return Err("Browser Prepare journal path is outside the journal directory".to_string());
    }
    let Some(name) = path.file_name() else {
        return Err("Browser Prepare journal has no filename".to_string());
    };
    let Some(directory) = crate::platform::filesystem::open_existing_private_file_directory(
        &hosted_prepare_journal_dir(),
    )
    .map_err(|error| {
        format!(
            "Failed to open browser Prepare journal directory {}: {error}",
            hosted_prepare_journal_dir().display()
        )
    })?
    else {
        return Ok(());
    };
    directory.remove_plain_file(name).map_err(|error| {
        format!(
            "Failed to delete browser Prepare journal {}: {error}",
            path.display()
        )
    })?;
    if directory
        .open_plain_file(name)
        .map_err(|error| {
            format!(
                "Failed to verify browser Prepare journal deletion {}: {error}",
                path.display()
            )
        })?
        .is_some()
    {
        Err(format!(
            "Browser Prepare journal {} remains visible after deletion",
            path.display()
        ))
    } else {
        Ok(())
    }
}

fn quarantine_regular_file_anchored(
    source: &Path,
    destination: &PrivateFileDirectory,
    destination_name: &OsStr,
    description: &str,
    required: bool,
) -> Result<(), String> {
    let destination_exists = destination
        .open_plain_file(destination_name)
        .map_err(|error| format!("Failed to inspect quarantined {description}: {error}"))?
        .is_some();
    let parent = source
        .parent()
        .ok_or_else(|| format!("{description} has no parent directory"))?;
    let Some(source_name) = source.file_name() else {
        return Err(format!("{description} has no filename"));
    };
    let source_directory = crate::platform::filesystem::open_existing_private_file_directory(
        parent,
    )
    .map_err(|error| {
        format!(
            "Failed to open active {description} directory {}: {error}",
            parent.display()
        )
    })?;
    let Some(source_directory) = source_directory else {
        return match (required, destination_exists) {
            (_, true) => Ok(()),
            (true, false) => Err(format!(
                "Required {description} disappeared before quarantine: {}",
                source.display()
            )),
            (false, false) => Ok(()),
        };
    };
    match source_directory
        .move_plain_file_to(source_name, destination, destination_name)
        .map_err(|error| {
            format!(
                "Failed to quarantine {description} {}: {error}",
                source.display()
            )
        })? {
        MovePlainFileOutcome::Moved | MovePlainFileOutcome::AlreadyMoved => Ok(()),
        MovePlainFileOutcome::Missing if required => Err(format!(
            "Required {description} disappeared before quarantine: {}",
            source.display()
        )),
        MovePlainFileOutcome::Missing => Ok(()),
    }
}

fn hosted_prepare_quarantine_parent(
    session_token: Option<&str>,
) -> Result<PrivateFileDirectory, String> {
    let root =
        crate::platform::filesystem::open_private_file_directory(&hosted_prepare_quarantine_dir())
            .map_err(|error| {
                format!(
                    "Failed to create or open browser Prepare quarantine root {}: {error}",
                    hosted_prepare_quarantine_dir().display()
                )
            })?;
    let name = session_token.unwrap_or("unassigned");
    root.create_private_child_directory(OsStr::new(name))
        .map_err(|error| {
            format!("Failed to create or open browser Prepare quarantine parent {name}: {error}")
        })
}

fn next_hosted_prepare_quarantine_slot(
    parent: &PrivateFileDirectory,
) -> Result<(OsString, PrivateFileDirectory), String> {
    let mut max_sequence = None::<u64>;
    let mut incomplete_slot = None::<(OsString, PrivateFileDirectory)>;
    for name in parent
        .entry_names()
        .map_err(|error| format!("Failed to enumerate browser Prepare quarantine slots: {error}"))?
    {
        let Some(name_text) = name.to_str() else {
            continue;
        };
        if !valid_prepare_quarantine_token(name_text) {
            continue;
        }
        let sequence = u64::from_str_radix(name_text, 16).map_err(|error| {
            format!("Invalid browser Prepare quarantine slot {name_text}: {error}")
        })?;
        max_sequence = Some(max_sequence.map_or(sequence, |current| current.max(sequence)));
        let slot = parent.open_child_directory(&name).map_err(|error| {
            format!("Failed to open browser Prepare quarantine slot {name_text}: {error}")
        })?;
        match slot.open_plain_file(OsStr::new("journal")) {
            Ok(Some(_)) => {}
            Ok(None) => {
                if incomplete_slot.replace((name, slot)).is_some() {
                    return Err(
                        "Multiple incomplete browser Prepare quarantine slots are ambiguous"
                            .to_string(),
                    );
                }
            }
            Err(error) => {
                return Err(format!(
                    "Browser Prepare quarantine marker is not a stable regular file: {error}"
                ))
            }
        }
    }
    if let Some(slot) = incomplete_slot {
        return Ok(slot);
    }
    let sequence = match max_sequence {
        Some(sequence) => sequence
            .checked_add(1)
            .ok_or_else(|| "Browser Prepare quarantine slot sequence is exhausted".to_string())?,
        None => 0,
    };
    let name = OsString::from(format!("{sequence:016x}"));
    let slot = parent
        .create_private_child_directory(&name)
        .map_err(|error| format!("Failed to create browser Prepare quarantine slot: {error}"))?;
    Ok((name, slot))
}

/// Isolate one unreadable transaction by the only identity that is still
/// trustworthy: its canonical filename token. Runtime and restore state move
/// first; the unreadable journal moves last and is the durable completion marker.
/// A crash or I/O error before that final rename leaves the active journal in
/// place, so the next startup reuses the incomplete slot and remains fail-closed.
/// A completed slot is historical evidence only: active paths are empty and a
/// fresh Prepare generation for the same task is safe.
fn quarantine_unreadable_hosted_prepare_state(
    journal_directory: &PrivateFileDirectory,
    journal_name: &OsStr,
) -> Result<Option<String>, String> {
    let path = hosted_prepare_journal_dir().join(journal_name);
    journal_directory
        .open_plain_file(journal_name)
        .map_err(|error| {
            format!("Unreadable browser Prepare journal is not a stable file: {error}")
        })?
        .ok_or_else(|| "Unreadable browser Prepare journal disappeared".to_string())?;
    let session_token = prepare_journal_token_from_path(&path)?;
    let parent = hosted_prepare_quarantine_parent(session_token.as_deref())?;
    let (_slot_name, slot) = next_hosted_prepare_quarantine_slot(&parent)?;
    if let Some(session_token) = session_token.as_deref() {
        quarantine_regular_file_anchored(
            &paths::browser_workspace_state_json(session_token),
            &slot,
            OsStr::new("runtime"),
            "browser runtime mapping",
            false,
        )?;
        quarantine_regular_file_anchored(
            &paths::browser_workspace_restore_json(session_token),
            &slot,
            OsStr::new("restore"),
            "browser restore manifest",
            false,
        )?;
    }
    match journal_directory
        .move_plain_file_to(journal_name, &slot, OsStr::new("journal"))
        .map_err(|error| format!("Failed to quarantine browser Prepare journal: {error}"))?
    {
        MovePlainFileOutcome::Moved | MovePlainFileOutcome::AlreadyMoved => {}
        MovePlainFileOutcome::Missing => {
            return Err("Unreadable browser Prepare journal disappeared".to_string())
        }
    }
    Ok(session_token)
}

fn remove_hosted_prepare_quarantine_slot(
    parent: &PrivateFileDirectory,
    slot_name: &OsStr,
    unassigned: bool,
) -> Result<(), String> {
    let slot = match parent.open_child_directory(slot_name) {
        Ok(slot) => slot,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Failed to open browser Prepare quarantine slot: {error}"
            ))
        }
    };
    if !unassigned {
        slot.remove_plain_file(OsStr::new("runtime"))
            .map_err(|error| format!("Failed to remove quarantined runtime mapping: {error}"))?;
        slot.remove_plain_file(OsStr::new("restore"))
            .map_err(|error| format!("Failed to remove quarantined restore manifest: {error}"))?;
    }
    slot.remove_plain_file(OsStr::new("journal"))
        .map_err(|error| format!("Failed to remove quarantined Prepare journal: {error}"))?;
    drop(slot);
    parent
        .remove_empty_child_directory(slot_name)
        .map_err(|error| format!("Failed to remove browser Prepare quarantine slot: {error}"))?;
    Ok(())
}

fn remove_hosted_prepare_quarantine_for_token_from_root(
    root: &PrivateFileDirectory,
    session_token: &str,
) -> Result<(), String> {
    let name = OsStr::new(session_token);
    let directory = match root.open_child_directory(name) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Failed to open browser Prepare token quarantine {session_token}: {error}"
            ))
        }
    };
    let mut slots = directory.entry_names().map_err(|error| {
        format!("Failed to enumerate browser Prepare token quarantine: {error}")
    })?;
    slots.sort();
    for slot_name in slots {
        let Some(slot_text) = slot_name.to_str() else {
            return Err("Browser Prepare quarantine slot name is not valid UTF-8".to_string());
        };
        if !valid_prepare_quarantine_token(slot_text) {
            return Err(format!(
                "Refusing to remove unexpected browser Prepare quarantine entry: {slot_text}"
            ));
        }
        remove_hosted_prepare_quarantine_slot(&directory, &slot_name, false)?;
    }
    drop(directory);
    root.remove_empty_child_directory(name).map_err(|error| {
        format!("Failed to remove browser Prepare token quarantine {session_token}: {error}")
    })?;
    Ok(())
}

fn remove_hosted_prepare_quarantine_for_token(session_token: &str) -> Result<(), String> {
    if !valid_prepare_quarantine_token(session_token) {
        return Err("Refusing to remove an invalid browser Prepare quarantine token".to_string());
    }
    let Some(root) = crate::platform::filesystem::open_existing_private_file_directory(
        &hosted_prepare_quarantine_dir(),
    )
    .map_err(|error| {
        format!(
            "Failed to open browser Prepare quarantine root {}: {error}",
            hosted_prepare_quarantine_dir().display()
        )
    })?
    else {
        return Ok(());
    };
    remove_hosted_prepare_quarantine_for_token_from_root(&root, session_token)
}

fn remove_hosted_prepare_quarantine_for_session(session_id: &str) -> Result<(), String> {
    remove_hosted_prepare_quarantine_for_token(&paths::browser_session_token(session_id))
}

fn remove_hosted_prepare_journal_for_session(session_id: &str) -> Result<(), String> {
    let session_token = paths::browser_session_token(session_id);
    let path = hosted_prepare_journal_path_for(&session_token);
    let Some(journal) = read_hosted_prepare_journal_if_present(&path)? else {
        return Ok(());
    };
    if journal.compensation.session_id != session_id
        || journal.compensation.session_token != session_token
    {
        return Err("Refusing to delete another task's Prepare journal".to_string());
    }
    remove_hosted_prepare_journal(&path)
}

fn remove_matching_hosted_prepare_journal(
    cancellation: &HostedBrowserCancellation,
) -> Result<(), String> {
    if matching_hosted_prepare_journal_for_cancellation(cancellation)?.is_none() {
        return Ok(());
    }
    remove_hosted_prepare_journal(&hosted_prepare_journal_path_for(
        &cancellation.session_token,
    ))
}

fn matching_hosted_prepare_journal_for_cancellation(
    cancellation: &HostedBrowserCancellation,
) -> Result<Option<HostedPrepareJournal>, String> {
    match classify_hosted_prepare_journal_for_cancellation(cancellation)? {
        HostedPrepareJournalMatch::Matching(journal) => Ok(Some(journal)),
        HostedPrepareJournalMatch::Absent | HostedPrepareJournalMatch::Superseded => Ok(None),
    }
}

fn classify_hosted_prepare_journal_for_cancellation(
    cancellation: &HostedBrowserCancellation,
) -> Result<HostedPrepareJournalMatch, String> {
    let path = hosted_prepare_journal_path_for(&cancellation.session_token);
    let Some(journal) = read_hosted_prepare_journal_if_present(&path)? else {
        return Ok(HostedPrepareJournalMatch::Absent);
    };
    let compensation = &journal.compensation;
    if compensation.request_id != cancellation.request_id
        || compensation.idempotency_key != cancellation.idempotency_key
        || compensation.session_id != cancellation.session_id
        || compensation.session_token != cancellation.session_token
        || compensation.caller_pid != cancellation.caller_pid
        || compensation.wrapper_instance_nonce != cancellation.wrapper_instance_nonce
    {
        // A distinct newer Prepare atomically supersedes the old per-session
        // WAL. Its generation must remain untouched, while the validated late
        // cancellation is an idempotent no-op that can still be acknowledged.
        return Ok(HostedPrepareJournalMatch::Superseded);
    }
    Ok(HostedPrepareJournalMatch::Matching(journal))
}

/// Read the cancellation path reserved for one durable Prepare generation and
/// require its full caller epoch to match. Merely observing a file at that path
/// is not authority to compensate a committed workspace.
fn matching_hosted_cancellation_for_compensation(
    compensation: &HostedPrepareCompensation,
) -> Result<bool, String> {
    let cancellation_path = paths::browser_host_requests_dir().join(format!(
        "{}-{}.cancelled",
        compensation.session_token, compensation.request_id
    ));
    if !cancellation_path.exists() {
        return Ok(false);
    }
    let raw = std::fs::read_to_string(&cancellation_path).map_err(|error| {
        format!(
            "Failed to read Prepare cancellation record {}: {error}",
            cancellation_path.display()
        )
    })?;
    let cancellation: HostedBrowserCancellation = serde_json::from_str(&raw)
        .map_err(|error| format!("Prepare cancellation record has an invalid format: {error}"))?;
    validate_hosted_cancellation(&cancellation, &cancellation_path)?;
    if cancellation.request_id != compensation.request_id
        || cancellation.idempotency_key != compensation.idempotency_key
        || cancellation.session_id != compensation.session_id
        || cancellation.session_token != compensation.session_token
        || cancellation.caller_pid != compensation.caller_pid
        || cancellation.wrapper_instance_nonce != compensation.wrapper_instance_nonce
    {
        return Err("Prepare cancellation record does not match persisted generation".to_string());
    }
    Ok(true)
}

fn remove_matching_hosted_prepare_journal_for_request(
    request: &HostedBrowserRequest,
) -> Result<(), String> {
    let path = hosted_prepare_journal_path(request);
    let Some(journal) = read_hosted_prepare_journal_if_present(&path)? else {
        return Ok(());
    };
    let compensation = &journal.compensation;
    if compensation.request_id != request.request_id
        || compensation.idempotency_key != request.idempotency_key
        || compensation.session_id != request.session_id
        || compensation.session_token != request.session_token
        || compensation.caller_pid != request.caller_pid
        || compensation.wrapper_instance_nonce != request.wrapper_instance_nonce
    {
        return Err("Refusing to delete another request's Prepare journal".to_string());
    }
    remove_hosted_prepare_journal(&path)
}

fn validate_hosted_prepare_journal(
    journal: &HostedPrepareJournal,
    path: &Path,
) -> Result<(), String> {
    let compensation = &journal.compensation;
    if journal.protocol_version != 3
        || journal.kind != "host_prepare_journal"
        || compensation.protocol_version != journal.protocol_version
        || compensation.kind != "host_prepare_compensation"
        || journal.requested_at == 0
        || journal.updated_at == 0
    {
        return Err("Browser Prepare journal protocol is invalid".to_string());
    }
    validate_hosted_caller_identity(
        compensation.caller_pid,
        &compensation.wrapper_instance_nonce,
    )?;
    let synthetic_request_path = paths::browser_host_requests_dir().join(format!(
        "{}-{}.json",
        compensation.session_token, compensation.request_id
    ));
    validate_hosted_identity(
        compensation.protocol_version,
        &compensation.session_id,
        &compensation.session_token,
        &compensation.request_id,
        &compensation.idempotency_key,
        &synthetic_request_path,
        "json",
    )?;
    if path != hosted_prepare_journal_path_for(&compensation.session_token) {
        return Err("Browser Prepare journal path does not match session identity".to_string());
    }
    match compensation.rollback_kind.as_str() {
        "none" if compensation.revision.is_none() => {}
        "prepared_session" | "restored_session" => {
            if !matches!(journal.phase, HostedPreparePhase::Pending)
                && compensation.revision.unwrap_or_default() == 0
            {
                return Err("Browser Prepare journal is missing compensation revision".to_string());
            }
        }
        _ => return Err("Browser Prepare journal compensation type is invalid".to_string()),
    }
    match journal.phase {
        HostedPreparePhase::Committed => {
            let response = journal
                .response
                .as_ref()
                .ok_or_else(|| "Committed Prepare journal is missing its response".to_string())?;
            if response.get("protocol_version").and_then(Value::as_u64)
                != Some(compensation.protocol_version as u64)
                || response.get("request_id").and_then(Value::as_str)
                    != Some(compensation.request_id.as_str())
                || response.get("idempotency_key").and_then(Value::as_str)
                    != Some(compensation.idempotency_key.as_str())
                || response.get("ok").and_then(Value::as_bool) != Some(true)
            {
                return Err("Committed Prepare journal response identity is invalid".to_string());
            }
        }
        _ if journal.response.is_some() => {
            return Err("Uncommitted Prepare journal cannot contain a success response".to_string())
        }
        _ => {}
    }
    Ok(())
}

fn validate_hosted_prepare_compensation(
    compensation: &HostedPrepareCompensation,
    allow_missing_revision: bool,
) -> Result<(), String> {
    if compensation.protocol_version != 3 || compensation.kind != "host_prepare_compensation" {
        return Err("Browser Prepare compensation protocol is invalid".to_string());
    }
    validate_hosted_caller_identity(
        compensation.caller_pid,
        &compensation.wrapper_instance_nonce,
    )?;
    let synthetic_request_path = paths::browser_host_requests_dir().join(format!(
        "{}-{}.json",
        compensation.session_token, compensation.request_id
    ));
    validate_hosted_identity(
        compensation.protocol_version,
        &compensation.session_id,
        &compensation.session_token,
        &compensation.request_id,
        &compensation.idempotency_key,
        &synthetic_request_path,
        "json",
    )?;
    match compensation.rollback_kind.as_str() {
        "none" if compensation.revision.is_none() => Ok(()),
        "prepared_session" | "restored_session"
            if allow_missing_revision || compensation.revision.unwrap_or_default() > 0 =>
        {
            Ok(())
        }
        _ => Err("Browser Prepare compensation generation is invalid".to_string()),
    }
}

fn reap_acknowledged_committed_prepare_journals(
    locally_committed: &mut HashSet<(String, String)>,
) -> Result<(), String> {
    let journal_dir = hosted_prepare_journal_dir();
    let entries = match std::fs::read_dir(&journal_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Failed to read browser Prepare journal directory {}: {error}",
                journal_dir.display()
            ))
        }
    };
    for entry in entries {
        let path = entry
            .map_err(|error| format!("Failed to enumerate browser Prepare journals: {error}"))?
            .path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let journal = read_hosted_prepare_journal(&path)?;
        if journal.phase != HostedPreparePhase::Committed {
            continue;
        }
        let compensation = &journal.compensation;
        let local_key = (
            compensation.session_token.clone(),
            compensation.request_id.clone(),
        );
        if !locally_committed.contains(&local_key) {
            // A process restart removes transient request/response artifacts
            // before the still-live wrapper necessarily reaches its timeout.
            // Only a commit produced by this process may interpret absence as
            // acknowledgement; recovered commits wait for a matching cancel or
            // an explicit newer Prepare to supersede them.
            continue;
        }
        let artifact = |extension: &str| {
            paths::browser_host_requests_dir().join(format!(
                "{}-{}.{}",
                compensation.session_token, compensation.request_id, extension
            ))
        };
        if artifact("cancelled").exists() {
            continue;
        }
        if !artifact("json").exists() && !artifact("response").exists() {
            remove_hosted_prepare_journal(&path)?;
            locally_committed.remove(&local_key);
        }
    }
    Ok(())
}

fn hosted_internal_cancellation_value(
    request: &HostedBrowserRequest,
    cancelled_at: u64,
    prepare_compensation: Option<&HostedPrepareCompensation>,
) -> Value {
    json!({
        "protocol_version": request.protocol_version,
        "kind": "host_request_cancelled",
        "request_id": request.request_id,
        "idempotency_key": request.idempotency_key,
        "session_id": request.session_id,
        "session_token": request.session_token,
        "caller_pid": request.caller_pid,
        "wrapper_instance_nonce": request.wrapper_instance_nonce,
        "reason": "host-caller-epoch-lost-before-prepare-publication",
        "cancelled_at": cancelled_at,
        "prepare_compensation": prepare_compensation,
    })
}

fn validate_hosted_caller_heartbeat_at(
    session_id: &str,
    session_token: &str,
    caller_pid: u32,
    wrapper_instance_nonce: &str,
    heartbeat: &HostedCallerHeartbeat,
    now_ms: u64,
    caller_process_alive: bool,
) -> Result<(), String> {
    const HEARTBEAT_TTL_MS: u64 = 5_000;
    const CLOCK_SKEW_TOLERANCE_MS: u64 = 5_000;
    if heartbeat.protocol_version != 3 || heartbeat.kind != "host_caller_heartbeat" {
        return Err("browser/host-caller-not-live: heartbeat protocol is invalid".to_string());
    }
    if heartbeat.session_id != session_id
        || heartbeat.session_token != session_token
        || heartbeat.caller_pid != caller_pid
        || heartbeat.wrapper_instance_nonce != wrapper_instance_nonce
    {
        return Err(
            "browser/host-caller-not-live: heartbeat epoch does not match request".to_string(),
        );
    }
    if heartbeat.heartbeat_at == 0
        || heartbeat.heartbeat_at > now_ms.saturating_add(CLOCK_SKEW_TOLERANCE_MS)
        || now_ms.saturating_sub(heartbeat.heartbeat_at) > HEARTBEAT_TTL_MS
    {
        return Err("browser/host-caller-not-live: heartbeat is stale".to_string());
    }
    if !caller_process_alive {
        return Err("browser/host-caller-not-live: wrapper process has exited".to_string());
    }
    Ok(())
}

fn ensure_hosted_caller_epoch_live_at(
    session_id: &str,
    session_token: &str,
    caller_pid: u32,
    wrapper_instance_nonce: &str,
    now_ms: u64,
) -> Result<(), String> {
    validate_hosted_caller_identity(caller_pid, wrapper_instance_nonce)?;
    let heartbeat_path = hosted_caller_heartbeat_path_for(session_token, wrapper_instance_nonce);
    let raw = std::fs::read_to_string(&heartbeat_path).map_err(|error| {
        format!(
            "browser/host-caller-not-live: cannot read {}: {error}",
            heartbeat_path.display()
        )
    })?;
    let heartbeat: HostedCallerHeartbeat = serde_json::from_str(&raw)
        .map_err(|error| format!("browser/host-caller-not-live: heartbeat is invalid: {error}"))?;
    validate_hosted_caller_heartbeat_at(
        session_id,
        session_token,
        caller_pid,
        wrapper_instance_nonce,
        &heartbeat,
        now_ms,
        crate::platform::os::process_alive(caller_pid),
    )
}

fn ensure_hosted_caller_live_at(request: &HostedBrowserRequest, now_ms: u64) -> Result<(), String> {
    if !request.operation.requires_live_caller() {
        return Ok(());
    }
    ensure_hosted_caller_epoch_live_at(
        &request.session_id,
        &request.session_token,
        request.caller_pid,
        &request.wrapper_instance_nonce,
        now_ms,
    )
}

fn ensure_hosted_caller_epoch_live(
    session_id: &str,
    session_token: &str,
    caller_pid: u32,
    wrapper_instance_nonce: &str,
) -> Result<(), String> {
    ensure_hosted_caller_epoch_live_at(
        session_id,
        session_token,
        caller_pid,
        wrapper_instance_nonce,
        hosted_protocol_now_ms()?,
    )
}

fn ensure_hosted_caller_live(request: &HostedBrowserRequest) -> Result<(), String> {
    // This narrows abandoned-artifact execution but cannot make a distributed
    // native mutation exactly-once: the wrapper can still be SIGKILLed after
    // this final check and before the platform commit. That residual window
    // remains an acknowledgement-unknown outcome handled by the existing
    // idempotency/tombstone/compensation protocol, not proof of at-most-once.
    ensure_hosted_caller_live_at(request, hosted_protocol_now_ms()?)
}

fn validate_hosted_cancellation(
    cancellation: &HostedBrowserCancellation,
    path: &std::path::Path,
) -> Result<(), String> {
    if cancellation.kind != "host_request_cancelled" {
        return Err("Browser host cancellation record type is invalid".to_string());
    }
    validate_hosted_caller_identity(
        cancellation.caller_pid,
        &cancellation.wrapper_instance_nonce,
    )?;
    validate_hosted_identity(
        cancellation.protocol_version,
        &cancellation.session_id,
        &cancellation.session_token,
        &cancellation.request_id,
        &cancellation.idempotency_key,
        path,
        "cancelled",
    )?;
    if let Some(compensation) = cancellation.prepare_compensation.as_ref() {
        validate_hosted_prepare_compensation(compensation, false)?;
        if compensation.protocol_version != cancellation.protocol_version
            || compensation.request_id != cancellation.request_id
            || compensation.idempotency_key != cancellation.idempotency_key
            || compensation.session_id != cancellation.session_id
            || compensation.session_token != cancellation.session_token
            || compensation.caller_pid != cancellation.caller_pid
            || compensation.wrapper_instance_nonce != cancellation.wrapper_instance_nonce
        {
            return Err(
                "Browser host cancellation record has mismatched Prepare compensation identity"
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn hosted_response(request: &HostedBrowserRequest, result: Result<Value, String>) -> Value {
    match result {
        Ok(result) => json!({
            "protocol_version": request.protocol_version,
            "request_id": request.request_id,
            "idempotency_key": request.idempotency_key,
            "ok": true,
            "result": result,
        }),
        Err(error) => json!({
            "protocol_version": request.protocol_version,
            "request_id": request.request_id,
            "idempotency_key": request.idempotency_key,
            "ok": false,
            "error": error,
        }),
    }
}

fn browser_core_tool_result(text: String, structured: Option<Value>) -> Value {
    let mut result = json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false,
    });
    if let Some(structured) = structured {
        result["structuredContent"] = structured;
    }
    result
}

fn should_reuse_browser_core_initial_tab(
    browser_session_id: &str,
    active_tab_token: &str,
    tabs: &[TabInfo],
    background: bool,
) -> bool {
    let initial_tab_token = paths::browser_session_token(browser_session_id);
    !background
        && tabs.len() == 1
        && active_tab_token == initial_tab_token
        && tabs[0].target_id == initial_tab_token
        && tabs[0].url == "about:blank"
}

fn write_hosted_response(request_path: &std::path::Path, response: &Value) -> Result<(), String> {
    let response_path = request_path.with_extension("response");
    if let Some(parent) = response_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!("Failed to create browser host response directory: {error}")
        })?;
        // The response carries the opaque tab lease; keep other local users
        // from reading it while it exists. Same pattern as the Prepare journal.
        crate::platform::os::make_private_dir(parent);
    }
    let encoded = serde_json::to_vec(response)
        .map_err(|error| format!("Failed to encode browser host response: {error}"))?;
    crate::platform::filesystem::atomic_write_private(&response_path, &encoded)
        .map_err(|error| format!("Failed to write browser host response: {error}"))
}

fn remove_hosted_request_artifacts(path: &std::path::Path) -> Result<(), String> {
    let mut errors = Vec::new();
    for artifact in [
        path.with_extension("response"),
        path.with_extension("json"),
        path.with_extension("cancelled"),
    ] {
        if let Err(error) = std::fs::remove_file(&artifact) {
            if error.kind() != std::io::ErrorKind::NotFound {
                errors.push(format!("Failed to delete {}: {error}", artifact.display()));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn remove_hosted_request_artifacts_for_session(session_id: &str) -> Result<(), String> {
    let request_dir = paths::browser_host_requests_dir();
    let entries = match std::fs::read_dir(&request_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Failed to read browser host request directory {}: {error}",
                request_dir.display()
            ));
        }
    };
    let prefix = format!("{}-", paths::browser_session_token(session_id));
    let mut failures = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                failures.push(format!(
                    "Failed to enumerate browser host artifacts: {error}"
                ));
                continue;
            }
        };
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !file_name.starts_with(&prefix)
            || !matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("json" | "response" | "cancelled")
            )
        {
            continue;
        }
        if let Err(error) = std::fs::remove_file(&path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                failures.push(format!("Failed to delete {}: {error}", path.display()));
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn native_lease_from_request(request: &HostedBrowserRequest) -> Result<NativeTabLease, String> {
    NativeTabLease::from_assertion(
        request.session_id.clone(),
        request
            .tab_token
            .clone()
            .ok_or_else(|| "Browser host lease is missing tab_token".to_string())?,
        request
            .target_id
            .clone()
            .ok_or_else(|| "Browser host lease is missing target_id".to_string())?,
        request
            .revision
            .ok_or_else(|| "Browser host lease is missing revision".to_string())?,
        request
            .lease
            .clone()
            .ok_or_else(|| "Browser host lease is missing capability token".to_string())?,
    )
}

fn native_mutation_lease_from_request(
    request: &HostedBrowserRequest,
) -> Result<NativeTabLease, String> {
    NativeTabLease::from_assertion(
        request.session_id.clone(),
        request.authorization_tab_token.clone().ok_or_else(|| {
            "Browser host mutation lease is missing authorization_tab_token".to_string()
        })?,
        request
            .target_id
            .clone()
            .ok_or_else(|| "Browser host mutation lease is missing target_id".to_string())?,
        request
            .revision
            .ok_or_else(|| "Browser host mutation lease is missing revision".to_string())?,
        request
            .lease
            .clone()
            .ok_or_else(|| "Browser host mutation lease is missing capability token".to_string())?,
    )
}

fn native_lease_from_value(value: &Value) -> Result<NativeTabLease, String> {
    NativeTabLease::from_assertion(
        value
            .get("session_id")
            .or_else(|| value.get("sessionId"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        value
            .get("tab_token")
            .or_else(|| value.get("tabToken"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        value
            .get("target_id")
            .or_else(|| value.get("targetId"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        value.get("revision").and_then(Value::as_u64).unwrap_or(0),
        value
            .get("lease")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )
}

async fn probe_cdp(port: u16, timeout: Duration) -> bool {
    let url = format!("http://127.0.0.1:{port}/json/version");
    // Loopback probes bypass system proxies. reqwest defaults to auto_sys_proxy,
    // so HTTP_PROXY without NO_PROXY would send 127.0.0.1 through the proxy and
    // fail. Probes are infrequent, so a one-shot client is sufficient.
    let Ok(client) = reqwest::Client::builder().no_proxy().build() else {
        return false;
    };
    // `pick_free_port` chooses by connect-failure, so another local process can
    // still claim the port before WebView2 binds it. Require CDP-shaped JSON so
    // a foreign 2xx server is rejected here instead of at webSocketDebuggerUrl
    // pinning; at worst prepare fails once and retries on a fresh port.
    let request = async {
        let response = client.get(&url).send().await.ok()?;
        response.status().is_success().then_some(())?;
        response.json::<serde_json::Value>().await.ok()
    };
    let Ok(Some(version)) = tokio::time::timeout(timeout, request).await else {
        return false;
    };
    cdp_version_matches_loopback_endpoint(&version, port)
}

fn cdp_version_matches_loopback_endpoint(version: &Value, expected_port: u16) -> bool {
    let product_matches = version
        .get("Browser")
        .and_then(Value::as_str)
        .is_some_and(|browser| {
            ["Edg/", "Chrome/", "HeadlessChrome/"]
                .iter()
                .any(|prefix| browser.starts_with(prefix))
        });
    let protocol_matches = version
        .get("Protocol-Version")
        .and_then(Value::as_str)
        .is_some_and(|protocol| {
            let mut segments = protocol.split('.');
            matches!(segments.next(), Some("1"))
                && segments.next().is_some_and(|minor| {
                    !minor.is_empty() && minor.bytes().all(|byte| byte.is_ascii_digit())
                })
                && segments.next().is_none()
        });
    let websocket_matches = version
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .and_then(|raw| tauri::Url::parse(raw).ok())
        .is_some_and(|url| {
            url.scheme() == "ws"
                && url.port() == Some(expected_port)
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path().starts_with("/devtools/browser/")
                && url.host_str().is_some_and(|host| {
                    host.eq_ignore_ascii_case("localhost")
                        || host
                            .parse::<std::net::IpAddr>()
                            .is_ok_and(|address| address.is_loopback())
                })
        });
    product_matches && protocol_matches && websocket_matches
}

async fn pick_free_port() -> Result<u16, String> {
    use rand::RngExt;
    let base = 9222 + rand::rng().random_range(0..3000);
    for port in base..(base + 200) {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err()
        {
            return Ok(port);
        }
    }
    // If every candidate is occupied, report failure instead of falling back to
    // the known-occupied base.
    Err(format!(
        "Port range {base}..{} is fully occupied; cannot create native browser automation endpoint",
        base + 200
    ))
}

/// Wait until the wrapper publishes the durable cancellation artifact for a
/// claimed host request. The surrounding `tokio::select!` revokes the platform
/// lease immediately, then keeps polling any already-dispatched DOM/WebDriver
/// work to its bounded settlement so a later mutation cannot overtake it.
async fn wait_for_hosted_cancellation(path: &Path) {
    while !path.exists() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Parse port-file contents with explicit valid-range checks. A corrupt or
/// foreign value such as 65536+k would silently wrap through `as u16`, probing an
/// unrelated endpoint and delaying stale cleanup by roughly 10 seconds.
fn parse_port_json(raw: &str) -> Option<u16> {
    let v: Value = serde_json::from_str(raw).ok()?;
    v.get("port")
        .and_then(Value::as_u64)
        .filter(|p| (1..=65535).contains(p))
        .map(|p| p as u16)
}

/// Only a port published by this app's native host may enter automation. Reject
/// legacy wrapper/external Chrome files with `owner=mcp` or browser_pid even when
/// their port is live, so native-page failure cannot silently change browser
/// identity or interaction semantics.
fn parse_host_owned_port_json(raw: &str) -> Option<u16> {
    let v: Value = serde_json::from_str(raw).ok()?;
    if v.get("owner").and_then(Value::as_str) != Some("app")
        || v.get("browser_pid").is_some_and(|value| !value.is_null())
    {
        return None;
    }
    parse_port_json(raw)
}

fn write_port_file(port: u16, owner: &str, browser_pid: Option<u32>) -> Result<(), String> {
    let path = paths::browser_cdp_port_json();
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let mut data = json!({
        "port": port,
        "pid": std::process::id(),
        "owner": owner,
        "started_at": chrono::Utc::now().timestamp_millis(),
    });
    if let Some(browser_pid) = browser_pid {
        data["browser_pid"] = json!(browser_pid);
    }
    let encoded = serde_json::to_vec_pretty(&data).map_err(|e| e.to_string())?;
    // CDP has no authentication. Restrict the temp file to 0600 on creation and
    // replace old files through the cross-platform state machine. Ordinary Windows
    // rename cannot overwrite a crash-left cdp-port.json and would break recovery.
    crate::platform::filesystem::atomic_write_private(&path, &encoded)
        .map_err(|e| format!("Failed to write port file: {e}"))
}

/// Remove a failed restore's endpoint publication only while the transaction
/// still owns the last native workspace and the file still names its exact
/// port. A concurrently staged workspace counts as a remaining consumer even
/// before publication, so its adopted endpoint survives the failed restore.
fn remove_failed_restore_port_if_unshared(
    expected_port: u16,
    has_remaining_sessions: bool,
) -> Result<(), String> {
    if has_remaining_sessions {
        return Ok(());
    }
    let path = paths::browser_cdp_port_json();
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Failed to verify failed browser restore endpoint {}: {error}",
                path.display()
            ))
        }
    };
    if parse_host_owned_port_json(&raw) != Some(expected_port) {
        return Ok(());
    }
    remove_file_and_verify_absent(&path, "remove failed browser restore endpoint")
}

fn remove_file_and_verify_absent(path: &Path, description: &str) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("{description} {} failed: {error}", path.display())),
    }
    if path.exists() {
        return Err(format!(
            "File remains visible after {description} {}",
            path.display()
        ));
    }
    Ok(())
}

fn remove_private_plain_file_and_verify_absent(
    path: &Path,
    description: &str,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{description} has no parent directory"))?;
    let name = path
        .file_name()
        .ok_or_else(|| format!("{description} has no filename"))?;
    let Some(directory) = crate::platform::filesystem::open_existing_private_file_directory(parent)
        .map_err(|error| format!("Failed to open {description} directory: {error}"))?
    else {
        return Ok(());
    };
    directory
        .remove_plain_file(name)
        .map_err(|error| format!("Failed to remove {description} {}: {error}", path.display()))?;
    if directory
        .open_plain_file(name)
        .map_err(|error| format!("Failed to verify {description} deletion: {error}"))?
        .is_some()
    {
        Err(format!(
            "File remains visible after removing {description}: {}",
            path.display()
        ))
    } else {
        Ok(())
    }
}

fn private_plain_file_exists(path: &Path, description: &str) -> Result<bool, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{description} has no parent directory"))?;
    let name = path
        .file_name()
        .ok_or_else(|| format!("{description} has no filename"))?;
    let Some(directory) = crate::platform::filesystem::open_existing_private_file_directory(parent)
        .map_err(|error| format!("Failed to open {description} directory: {error}"))?
    else {
        return Ok(false);
    };
    directory
        .open_plain_file(name)
        .map(|file| file.is_some())
        .map_err(|error| format!("Failed to inspect {description}: {error}"))
}

fn parse_host_runtime_revision(value: &Value, expected_session_token: &str) -> Option<u64> {
    (value.get("version").and_then(Value::as_u64) == Some(2)
        && value.get("mapping_authority").and_then(Value::as_str) == Some("host")
        && value.get("session_token").and_then(Value::as_str) == Some(expected_session_token))
    .then(|| value.get("revision").and_then(Value::as_u64))
    .flatten()
    .filter(|revision| *revision > 0)
}

fn read_host_runtime_revision(
    path: &Path,
    expected_session_token: &str,
) -> Result<Option<u64>, String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Browser runtime mapping has no parent directory".to_string())?;
    let name = path
        .file_name()
        .ok_or_else(|| "Browser runtime mapping has no filename".to_string())?;
    let Some(directory) = crate::platform::filesystem::open_existing_private_file_directory(parent)
        .map_err(|error| format!("Failed to open browser runtime mapping directory: {error}"))?
    else {
        return Ok(None);
    };
    let Some(mut file) = directory
        .open_plain_file(name)
        .map_err(|error| format!("Failed to open browser runtime mapping: {error}"))?
    else {
        return Ok(None);
    };
    let mut raw = Vec::new();
    file.read_to_end(&mut raw)
        .map_err(|error| format!("Failed to read browser runtime mapping: {error}"))?;
    let Ok(value) = serde_json::from_slice::<Value>(&raw) else {
        return Ok(None);
    };
    Ok(parse_host_runtime_revision(&value, expected_session_token))
}

/// Recover host-owned Prepare journals before the transient request directory
/// is reset and before any status/restore path can publish a stale manifest.
/// Native WebViews cannot survive an application-process restart, so recovery
/// only has to reconcile the durable restore/runtime mapping files.
fn recover_hosted_prepare_journals_for_process_start() -> Result<(), String> {
    let journal_dir = hosted_prepare_journal_dir();
    let Some(journal_directory) =
        crate::platform::filesystem::open_existing_private_file_directory(&journal_dir).map_err(
            |error| {
                format!(
                    "Failed to open browser Prepare journal directory {}: {error}",
                    journal_dir.display()
                )
            },
        )?
    else {
        return Ok(());
    };
    let mut journal_names = journal_directory
        .entry_names()
        .map_err(|error| format!("Failed to enumerate browser Prepare journals: {error}"))?
        .into_iter()
        .filter(|name| Path::new(name).extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    journal_names.sort();
    let mut errors = Vec::new();
    for name in journal_names {
        let path = journal_dir.join(&name);
        let result = (|| {
            let journal = match read_hosted_prepare_journal_from_directory(
                &journal_directory,
                &name,
                &path,
            ) {
                Ok(journal) => journal,
                Err(read_error) => {
                    let session_token =
                        quarantine_unreadable_hosted_prepare_state(&journal_directory, &name)?;
                    if let Some(session_token) = session_token {
                        eprintln!(
                            "[browser] quarantined unreadable Prepare state for token {session_token}: {read_error}"
                        );
                    } else {
                        eprintln!(
                            "[browser] quarantined unreadable Prepare journal with an untrusted filename: {read_error}"
                        );
                    }
                    return Ok(());
                }
            };
            let compensation = &journal.compensation;
            let matching_cancellation =
                matching_hosted_cancellation_for_compensation(compensation)?;
            let committed_without_cancellation =
                journal.phase == HostedPreparePhase::Committed && !matching_cancellation;
            let runtime_path = paths::browser_workspace_state_json(&compensation.session_token);
            let runtime_revision =
                read_host_runtime_revision(&runtime_path, &compensation.session_token)?;
            let superseded_committed_prepare = journal.phase == HostedPreparePhase::Committed
                && compensation.rollback_kind == "prepared_session"
                && compensation
                    .revision
                    .zip(runtime_revision)
                    .is_some_and(|(expected, actual)| actual > expected);
            if superseded_committed_prepare {
                // Persist the no-op compensation state before discarding the
                // newer runtime-revision witness. Every crash point is safe:
                // before this write the revision proves supersession; after it
                // a late cancellation cannot remove the current restore file.
                let mut neutralized = journal.clone();
                neutralized.phase = HostedPreparePhase::Cancelled;
                neutralized.compensation.rollback_kind = "none".to_string();
                neutralized.compensation.revision = None;
                neutralized.response = None;
                write_hosted_prepare_journal(&neutralized)?;
            }
            // Runtime labels/targets are process-local in every phase. Remove
            // them even when the committed manifest/WAL must remain available
            // for a still-live wrapper's late cancellation.
            remove_private_plain_file_and_verify_absent(
                &runtime_path,
                "delete stale browser runtime mapping",
            )?;
            if superseded_committed_prepare {
                // A newer host-authoritative revision permanently superseded
                // this committed Prepare, or a prior crash already persisted
                // that terminal decision. Retire the old WAL after cleanup.
                journal_directory
                    .remove_plain_file(&name)
                    .map_err(|error| format!("Failed to retire superseded Prepare WAL: {error}"))?;
                return Ok(());
            }
            if committed_without_cancellation {
                // The wrapper can still be alive and reach its timeout after
                // this process starts. Keep the host-owned compensation record
                // across transient request-directory reset so that late cancel
                // remains authoritative. This recovered WAL is never reaped by
                // transient artifact absence; only a matching tombstone or a
                // newer Prepare may settle/supersede it.
                return Ok(());
            }
            if compensation.rollback_kind == "prepared_session" {
                remove_private_plain_file_and_verify_absent(
                    &paths::browser_workspace_restore_json(&compensation.session_token),
                    "delete uncommitted Prepare restore manifest",
                )?;
            }
            // Re-read the CreatedBlank delete boundary before removing the WAL.
            // A SIGKILL after the manifest deletion therefore leaves the WAL
            // behind and the next process repeats the idempotent verification.
            if compensation.rollback_kind == "prepared_session"
                && private_plain_file_exists(
                    &paths::browser_workspace_restore_json(&compensation.session_token),
                    "uncommitted Prepare restore manifest",
                )?
            {
                return Err("Uncommitted Prepare restore manifest still exists".to_string());
            }
            journal_directory
                .remove_plain_file(&name)
                .map_err(|error| format!("Failed to remove browser Prepare journal: {error}"))?;
            Ok(())
        })();
        if let Err(error) = result {
            errors.push(format!("{}: {error}", path.display()));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Create a fresh transient host-request directory for this app process. First
/// atomically rename the old directory to a sibling quarantine the watcher never
/// observes, then create an empty directory. Requests left by an old process can
/// therefore never mix with current-process requests during watcher registration.
fn reset_host_request_directory_for_process_start(
    request_dir: &std::path::Path,
) -> Result<(), String> {
    let parent = request_dir
        .parent()
        .ok_or_else(|| "Browser host request directory has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| {
        format!("Failed to create browser host coordination directory: {error}")
    })?;

    let mut quarantined = None;
    if request_dir.exists() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let stem = request_dir
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("host-requests");
        let mut last_error = None;
        for attempt in 0..8_u8 {
            let candidate = parent.join(format!(
                ".{stem}.stale-{}-{nonce}-{attempt}",
                std::process::id()
            ));
            match std::fs::rename(request_dir, &candidate) {
                Ok(()) => {
                    quarantined = Some(candidate);
                    last_error = None;
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    last_error = None;
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    last_error = Some(error);
                }
                Err(error) => {
                    return Err(format!(
                        "Failed to atomically quarantine {}: {error}",
                        request_dir.display()
                    ));
                }
            }
        }
        if let Some(error) = last_error {
            return Err(format!(
                "Could not allocate quarantine directory for {}: {error}",
                request_dir.display()
            ));
        }
    }

    std::fs::create_dir(request_dir)
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(error)
            }
        })
        .map_err(|error| {
            format!("Failed to create current-process browser host request directory: {error}")
        })?;
    // Request files carry session tokens and opaque leases; tighten at creation
    // instead of relying on a later browser_home sweep to chmod it.
    crate::platform::os::make_private_dir(request_dir);

    if let Some(quarantine) = quarantined {
        let cleanup = match std::fs::symlink_metadata(&quarantine) {
            Ok(metadata) if metadata.file_type().is_dir() => std::fs::remove_dir_all(&quarantine),
            Ok(_) => std::fs::remove_file(&quarantine),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        };
        if let Err(error) = cleanup {
            // Old requests are physically isolated from the watcher. Cleanup
            // failure cannot expose or replay them; next startup still watches
            // only the standard directory. Keep diagnostics without sacrificing
            // current-process browser availability.
            eprintln!(
                "[browser] Failed to delete quarantined old host request directory {}: {error}",
                quarantine.display()
            );
        }
    }
    Ok(())
}

fn clear_host_request_files() {
    let Ok(entries) = std::fs::read_dir(paths::browser_host_requests_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hosted_admission_keeps_control_reads_concurrent_and_restart_exclusive() {
        let manager = BrowserManager::new();
        let data_guard = manager.hosted_request_gate.read().await;
        let control_guard = tokio::time::timeout(
            Duration::from_millis(20),
            manager.hosted_request_gate.read(),
        )
        .await
        .expect("control-plane reader must not wait behind a data request");

        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                manager.hosted_request_gate.write()
            )
            .await
            .is_err(),
            "restart writer must wait for every accepted hosted request"
        );

        drop(control_guard);
        drop(data_guard);
        let restart_guard = tokio::time::timeout(
            Duration::from_millis(20),
            manager.hosted_request_gate.write(),
        )
        .await
        .expect("restart writer should proceed after hosted requests settle");
        drop(restart_guard);
    }

    #[tokio::test]
    async fn deleted_session_teardown_waits_for_an_active_request_scanner() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let manager = Arc::new(BrowserManager::new());
        let scanner_guard = manager.hosted_request_gate.read().await;
        let deletion = {
            let manager = Arc::clone(&manager);
            tokio::spawn(async move { manager.delete_for_session("session-a").await })
        };
        tokio::pin!(deletion);

        assert!(
            tokio::time::timeout(Duration::from_millis(20), deletion.as_mut())
                .await
                .is_err(),
            "task deletion must drain accepted request scanners before purging their ledger"
        );
        drop(scanner_guard);
        deletion
            .await
            .expect("deletion task should join")
            .expect("deletion should proceed after scanner drain");
    }

    #[test]
    fn browser_watch_recovery_retry_is_capped_and_resets_only_when_consumer_is_ready() {
        let manager = BrowserManager::new();
        *manager.prepare_recovery_error.lock() =
            Some("browser/host-consumer-unavailable: simulated directory failure".to_string());
        for expected in [250, 500, 1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000] {
            assert_eq!(manager.next_watch_retry_delay().as_millis(), expected);
        }
        assert!(manager.prepare_recovery_error.lock().is_some());

        manager.mark_watch_consumer_ready();
        assert!(manager.prepare_recovery_error.lock().is_none());
        assert_eq!(manager.next_watch_retry_delay().as_millis(), 250);
    }

    #[test]
    fn persistence_coalescing_without_app_handle_fails_closed_and_clears_inflight() {
        // No app handle is bound: the direct call must fail, and — critically —
        // must remove its own inflight marker so a later call (once an app
        // exists) starts a fresh flight instead of being coalesced forever.
        let manager = BrowserManager::new();
        assert!(manager.persist_native_restore("session-x").is_err());
        assert!(
            !manager
                .persistence_inflight
                .lock()
                .contains_key("session-x"),
            "early failure must clear the inflight marker"
        );
    }

    #[test]
    fn persistence_coalescing_marks_dirty_only_while_a_flight_is_registered() {
        let manager = BrowserManager::new();
        // Simulate an in-flight write: a concurrent caller is coalesced into
        // dirty and reported Ok; the state converges via the follow-up write
        // the finishing flight performs.
        manager
            .persistence_inflight
            .lock()
            .insert("session-y".to_string(), false);
        assert!(manager.persist_native_restore("session-y").is_ok());
        assert_eq!(
            manager.persistence_inflight.lock().get("session-y"),
            Some(&true),
            "coalesced call must mark the session dirty"
        );
    }

    #[tokio::test]
    async fn task_lifecycle_lock_does_not_block_an_unrelated_session() {
        let manager = BrowserManager::new();
        let session_a = manager.session_lifecycle_lock("session-a");
        let same_session_a = manager.session_lifecycle_lock("session-a");
        let session_b = manager.session_lifecycle_lock("session-b");
        assert!(Arc::ptr_eq(&session_a, &same_session_a));

        let _session_a_guard = session_a.lock().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(20), same_session_a.lock())
                .await
                .is_err(),
            "the same task must wait for its active lifecycle transaction"
        );
        let session_b_guard = tokio::time::timeout(Duration::from_millis(20), session_b.lock())
            .await
            .expect("an unrelated task must not wait behind session A");
        drop(session_b_guard);
    }

    #[tokio::test]
    async fn queued_lifecycle_commands_fail_closed_after_restart_admission_closes() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());

        async fn queue_behind_start_lock(
            manager: Arc<BrowserManager>,
            operation: impl std::future::Future<Output = Result<(), String>> + Send + 'static,
        ) -> Result<(), String> {
            let start_guard = manager.start_mtx.lock().await;
            let queued = tokio::spawn(operation);
            tokio::task::yield_now().await;
            manager
                .shutting_down
                .store(true, std::sync::atomic::Ordering::SeqCst);
            drop(start_guard);
            queued.await.expect("queued lifecycle command should join")
        }

        let stop_manager = Arc::new(BrowserManager::new());
        let stop_result = queue_behind_start_lock(Arc::clone(&stop_manager), {
            let manager = Arc::clone(&stop_manager);
            async move { manager.stop().await }
        })
        .await;
        assert_eq!(
            stop_result,
            Err(
                "application is shutting down; browser operations are no longer accepted"
                    .to_string()
            )
        );
        assert_eq!(
            stop_manager
                .stop_gen
                .load(std::sync::atomic::Ordering::SeqCst),
            0,
            "rejected stop must not mutate lifecycle generation"
        );

        let scoped_session = "queued-session";
        let restore_path =
            paths::browser_workspace_restore_json(&paths::browser_session_token(scoped_session));
        std::fs::create_dir_all(restore_path.parent().unwrap()).unwrap();
        std::fs::write(&restore_path, b"restore-sentinel").unwrap();
        let scoped_manager = Arc::new(BrowserManager::new());
        let scoped_result = queue_behind_start_lock(Arc::clone(&scoped_manager), {
            let manager = Arc::clone(&scoped_manager);
            async move { manager.stop_for_session(scoped_session).await }
        })
        .await;
        assert_eq!(
            scoped_result,
            Err(
                "application is shutting down; browser operations are no longer accepted"
                    .to_string()
            )
        );
        assert_eq!(
            std::fs::read(&restore_path).unwrap(),
            b"restore-sentinel",
            "a queued scoped stop must not delete restart recovery state"
        );

        let restore_manager = BrowserManager::new();
        restore_manager
            .shutting_down
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            restore_manager
                .restore_saved_workspace("queued-session")
                .await,
            Err(
                "application is shutting down; browser operations are no longer accepted"
                    .to_string()
            )
        );
    }

    #[test]
    fn failed_restore_preserves_an_endpoint_adopted_by_a_concurrent_workspace() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let shared_port = 19_222;

        // Restore A publishes P and releases start_mtx while binding. Restore B
        // then adopts P and stages its workspace before A's bind fails.
        write_port_file(shared_port, "app", None).unwrap();
        remove_failed_restore_port_if_unshared(shared_port, true).unwrap();

        let still_published = std::fs::read_to_string(paths::browser_cdp_port_json()).unwrap();
        assert_eq!(
            parse_host_owned_port_json(&still_published),
            Some(shared_port),
            "B must still be able to use the endpoint it adopted from A"
        );

        // The same failed transaction may remove P once no native workspace
        // remains. The expected-port comparison also prevents deleting a file
        // that a different runtime replaced while start_mtx was released.
        remove_failed_restore_port_if_unshared(shared_port, false).unwrap();
        assert!(!paths::browser_cdp_port_json().exists());

        write_port_file(shared_port + 1, "app", None).unwrap();
        remove_failed_restore_port_if_unshared(shared_port, false).unwrap();
        let replacement = std::fs::read_to_string(paths::browser_cdp_port_json()).unwrap();
        assert_eq!(
            parse_host_owned_port_json(&replacement),
            Some(shared_port + 1)
        );
    }

    struct PinvouHomeGuard(Option<std::ffi::OsString>);

    impl PinvouHomeGuard {
        fn install(path: &Path) -> Self {
            let previous = std::env::var_os("PINVOU3_HOME");
            std::env::set_var("PINVOU3_HOME", path);
            Self(previous)
        }
    }

    impl Drop for PinvouHomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(previous) => std::env::set_var("PINVOU3_HOME", previous),
                None => std::env::remove_var("PINVOU3_HOME"),
            }
        }
    }

    fn hosted_request_at(
        operation: HostedBrowserOperation,
        requested_at: u64,
    ) -> HostedBrowserRequest {
        HostedBrowserRequest {
            protocol_version: 3,
            request_id: "request-a".to_string(),
            idempotency_key: "unused-in-freshness-test".to_string(),
            session_id: "session-a".to_string(),
            session_token: "0123456789abcdef".to_string(),
            caller_pid: 42,
            wrapper_instance_nonce: "0123456789abcdef0123456789abcdef".to_string(),
            operation,
            requested_at,
            tab_token: None,
            authorization_tab_token: None,
            creation_id: None,
            url: None,
            target_id: None,
            revision: None,
            lease: None,
            tool_name: None,
            tool_arguments: None,
            background: false,
            emits_trusted_input: false,
            observational_only: false,
        }
    }

    fn valid_hosted_prepare_request(now_ms: u64) -> HostedBrowserRequest {
        let session_id = "prepare-journal-session".to_string();
        let session_token = paths::browser_session_token(&session_id);
        let request_id = "prepare.request-a".to_string();
        HostedBrowserRequest {
            protocol_version: 3,
            idempotency_key: format!("{session_token}/{request_id}"),
            request_id,
            session_id,
            session_token,
            caller_pid: 42,
            wrapper_instance_nonce: "0123456789abcdef0123456789abcdef".to_string(),
            operation: HostedBrowserOperation::Prepare,
            requested_at: now_ms,
            tab_token: None,
            authorization_tab_token: None,
            creation_id: None,
            url: None,
            target_id: None,
            revision: None,
            lease: None,
            tool_name: None,
            tool_arguments: None,
            background: false,
            emits_trusted_input: false,
            observational_only: false,
        }
    }

    fn hosted_caller_heartbeat_at(heartbeat_at: u64) -> HostedCallerHeartbeat {
        HostedCallerHeartbeat {
            protocol_version: 3,
            kind: "host_caller_heartbeat".to_string(),
            session_id: "session-a".to_string(),
            session_token: "0123456789abcdef".to_string(),
            caller_pid: 42,
            wrapper_instance_nonce: "0123456789abcdef0123456789abcdef".to_string(),
            heartbeat_at,
        }
    }

    fn validate_test_heartbeat(
        request: &HostedBrowserRequest,
        heartbeat: &HostedCallerHeartbeat,
        now_ms: u64,
        caller_process_alive: bool,
    ) -> Result<(), String> {
        validate_hosted_caller_heartbeat_at(
            &request.session_id,
            &request.session_token,
            request.caller_pid,
            &request.wrapper_instance_nonce,
            heartbeat,
            now_ms,
            caller_process_alive,
        )
    }

    #[test]
    fn hosted_request_artifacts_expire_with_their_live_caller_budget() {
        let now = 100_000;
        assert!(validate_hosted_request_freshness_at(
            &hosted_request_at(HostedBrowserOperation::CoreTool, now - 25_000),
            now,
        )
        .is_ok());
        assert!(validate_hosted_request_freshness_at(
            &hosted_request_at(HostedBrowserOperation::CoreTool, now - 25_001),
            now,
        )
        .is_err());
        assert!(validate_hosted_request_freshness_at(
            &hosted_request_at(HostedBrowserOperation::CreateTab, now - 12_001),
            now,
        )
        .is_err());
        assert!(validate_hosted_request_freshness_at(
            &hosted_request_at(HostedBrowserOperation::EndAgentOperation, now - 12_000),
            now,
        )
        .is_ok());
        assert!(validate_hosted_request_freshness_at(
            &hosted_request_at(HostedBrowserOperation::Prepare, now + 5_001),
            now,
        )
        .is_err());
        assert!(validate_hosted_request_freshness_at(
            &hosted_request_at(HostedBrowserOperation::Prepare, 0),
            now,
        )
        .is_err());
    }

    #[test]
    fn authority_bearing_host_requests_require_a_matching_live_epoch() {
        let now = 100_000;
        let request = hosted_request_at(HostedBrowserOperation::CreateTab, now);
        let heartbeat = hosted_caller_heartbeat_at(now - 5_000);
        assert!(validate_test_heartbeat(&request, &heartbeat, now, true).is_ok());

        let stale = hosted_caller_heartbeat_at(now - 5_001);
        assert!(validate_test_heartbeat(&request, &stale, now, true).is_err());
        let future = hosted_caller_heartbeat_at(now + 5_001);
        assert!(validate_test_heartbeat(&request, &future, now, true).is_err());
        assert!(validate_test_heartbeat(&request, &heartbeat, now, false).is_err());

        let mut wrong_epoch = hosted_caller_heartbeat_at(now);
        wrong_epoch.wrapper_instance_nonce = "fedcba9876543210fedcba9876543210".to_string();
        assert!(validate_test_heartbeat(&request, &wrong_epoch, now, true).is_err());
        let mut wrong_pid = hosted_caller_heartbeat_at(now);
        wrong_pid.caller_pid += 1;
        assert!(validate_test_heartbeat(&request, &wrong_pid, now, true).is_err());
        let mut wrong_protocol = hosted_caller_heartbeat_at(now);
        wrong_protocol.protocol_version = 2;
        assert!(validate_test_heartbeat(&request, &wrong_protocol, now, true).is_err());
        let mut wrong_session = hosted_caller_heartbeat_at(now);
        wrong_session.session_id = "session-b".to_string();
        assert!(validate_test_heartbeat(&request, &wrong_session, now, true).is_err());
        let mut wrong_token = hosted_caller_heartbeat_at(now);
        wrong_token.session_token = "fedcba9876543210".to_string();
        assert!(validate_test_heartbeat(&request, &wrong_token, now, true).is_err());
    }

    #[test]
    fn wrapper_epoch_identity_and_artifact_name_are_canonical() {
        assert!(valid_wrapper_instance_nonce(
            "0123456789abcdef0123456789abcdef"
        ));
        assert!(!valid_wrapper_instance_nonce(
            "0123456789ABCDEF0123456789ABCDEF"
        ));
        assert!(!valid_wrapper_instance_nonce("0123456789abcdef"));
        assert!(validate_hosted_caller_identity(42, "0123456789abcdef0123456789abcdef").is_ok());
        assert!(validate_hosted_caller_identity(0, "0123456789abcdef0123456789abcdef").is_err());

        let request = hosted_request_at(HostedBrowserOperation::Prepare, 1);
        assert_eq!(
            hosted_caller_heartbeat_path(&request)
                .file_name()
                .and_then(|name| name.to_str()),
            Some("0123456789abcdef-0123456789abcdef0123456789abcdef.heartbeat")
        );
    }

    #[test]
    fn cleanup_requests_do_not_depend_on_a_live_wrapper_epoch() {
        for operation in [
            HostedBrowserOperation::EndAgentOperation,
            HostedBrowserOperation::RollbackCreatedTab,
        ] {
            let request = hosted_request_at(operation, 100_000);
            assert!(!operation.requires_live_caller());
            // No heartbeat artifact is present. Cleanup must still pass the
            // epoch gate so it can release authority/resources after SIGKILL.
            assert!(ensure_hosted_caller_live_at(&request, 100_000).is_ok());
        }
        for operation in [
            HostedBrowserOperation::Prepare,
            HostedBrowserOperation::CreateTab,
            HostedBrowserOperation::ActivateTab,
            HostedBrowserOperation::CloseTab,
            HostedBrowserOperation::AssertHostLease,
            HostedBrowserOperation::BeginAgentOperation,
            HostedBrowserOperation::RefreshAgentOperation,
            HostedBrowserOperation::RefreshAgentInput,
            HostedBrowserOperation::CoreTool,
        ] {
            assert!(operation.requires_live_caller());
        }
    }

    #[test]
    fn host_request_and_cancellation_require_wrapper_epoch_fields() {
        let request_without_epoch = json!({
            "protocol_version": 3,
            "request_id": "request-a",
            "idempotency_key": "0123456789abcdef/request-a",
            "session_id": "session-a",
            "session_token": "0123456789abcdef",
            "operation": "prepare",
            "requested_at": 100_000,
        });
        assert!(serde_json::from_value::<HostedBrowserRequest>(request_without_epoch).is_err());

        let cancellation_without_epoch = json!({
            "protocol_version": 3,
            "kind": "host_request_cancelled",
            "request_id": "request-a",
            "idempotency_key": "0123456789abcdef/request-a",
            "session_id": "session-a",
            "session_token": "0123456789abcdef",
        });
        assert!(
            serde_json::from_value::<HostedBrowserCancellation>(cancellation_without_epoch)
                .is_err()
        );
    }

    #[test]
    fn dead_prepare_failure_retains_exact_retryable_compensation_metadata() {
        let request = hosted_request_at(HostedBrowserOperation::Prepare, 100_000);
        let mut created_blank_compensation =
            HostedPrepareCompensation::from_request(&request, "prepared_session");
        created_blank_compensation.revision = Some(7);
        let created_blank = created_blank_compensation.rollback_value().unwrap();
        assert_eq!(created_blank["kind"], "prepared_session");
        assert_eq!(created_blank["revision"], 7);

        let mut restored_compensation =
            HostedPrepareCompensation::from_request(&request, "restored_session");
        restored_compensation.revision = Some(8);
        let restored = restored_compensation.rollback_value().unwrap();
        assert_eq!(restored["kind"], "restored_session");
        assert_eq!(restored["revision"], 8);

        let failed = HostedBrowserOutcome::failed_with_rollback(
            "browser/host-caller-not-live".to_string(),
            created_blank.clone(),
        );
        assert_eq!(
            failed.error.as_deref(),
            Some("browser/host-caller-not-live")
        );
        assert_eq!(failed.rollback, created_blank);

        let tombstone = hosted_internal_cancellation_value(&request, 100_001, None);
        let decoded: HostedBrowserCancellation = serde_json::from_value(tombstone).unwrap();
        assert_eq!(decoded.request_id, request.request_id);
        assert_eq!(decoded.caller_pid, request.caller_pid);
        assert_eq!(
            decoded.wrapper_instance_nonce,
            request.wrapper_instance_nonce
        );
        assert_eq!(
            hosted_cancellation_path(&request)
                .file_name()
                .and_then(|name| name.to_str()),
            Some("0123456789abcdef-request-a.cancelled")
        );
    }

    #[test]
    fn prepare_journal_is_host_owned_outside_the_transient_request_directory() {
        let request = valid_hosted_prepare_request(100_000);
        let journal_path = hosted_prepare_journal_path(&request);
        let expected_name = format!("{}.json", request.session_token);
        assert!(journal_path.starts_with(paths::browser_home()));
        assert!(!journal_path.starts_with(paths::browser_host_requests_dir()));
        assert_eq!(
            journal_path.file_name().and_then(|name| name.to_str()),
            Some(expected_name.as_str())
        );
    }

    #[test]
    fn first_prepare_journal_write_creates_private_durable_directory() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let journal = new_hosted_prepare_journal(&request, "prepared_session", 100_001);

        assert!(!hosted_prepare_journal_dir().exists());
        write_hosted_prepare_journal(&journal).unwrap();
        assert!(hosted_prepare_journal_dir().is_dir());
        assert!(hosted_prepare_journal_path(&request).is_file());
    }

    #[test]
    fn successful_session_stop_removes_exact_prepare_wal_idempotently() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let journal = new_hosted_prepare_journal(&request, "prepared_session", 100_001);
        write_hosted_prepare_journal(&journal).unwrap();

        remove_hosted_prepare_journal_for_session(&request.session_id).unwrap();
        remove_hosted_prepare_journal_for_session(&request.session_id).unwrap();
        assert!(!hosted_prepare_journal_path(&request).exists());
    }

    #[test]
    fn failed_session_stop_wal_delete_remains_retryable() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let path = hosted_prepare_journal_path(&request);
        std::fs::create_dir_all(&path).unwrap();

        assert!(remove_hosted_prepare_journal_for_session(&request.session_id).is_err());
        assert!(path.exists());
    }

    #[test]
    fn startup_pending_created_blank_recovery_deletes_manifest_before_journal() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let journal = new_hosted_prepare_journal(&request, "prepared_session", 100_001);
        let restore_path = paths::browser_workspace_restore_json(&request.session_token);
        let runtime_path = paths::browser_workspace_state_json(&request.session_token);
        std::fs::create_dir_all(restore_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(runtime_path.parent().unwrap()).unwrap();
        std::fs::write(&restore_path, b"created-blank").unwrap();
        std::fs::write(&runtime_path, b"runtime-mapping").unwrap();
        write_hosted_prepare_journal(&journal).unwrap();

        recover_hosted_prepare_journals_for_process_start().unwrap();

        assert!(!restore_path.exists());
        assert!(!runtime_path.exists());
        assert!(!hosted_prepare_journal_path(&request).exists());
    }

    #[test]
    fn startup_pending_restored_recovery_preserves_original_manifest() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let journal = new_hosted_prepare_journal(&request, "restored_session", 100_001);
        let restore_path = paths::browser_workspace_restore_json(&request.session_token);
        std::fs::create_dir_all(restore_path.parent().unwrap()).unwrap();
        std::fs::write(&restore_path, b"original-restore").unwrap();
        write_hosted_prepare_journal(&journal).unwrap();

        recover_hosted_prepare_journals_for_process_start().unwrap();

        assert_eq!(std::fs::read(&restore_path).unwrap(), b"original-restore");
        assert!(!hosted_prepare_journal_path(&request).exists());
    }

    #[test]
    fn startup_before_wrapper_timeout_retains_committed_wal_for_late_cancel() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let mut journal = new_hosted_prepare_journal(&request, "prepared_session", 100_001);
        journal.phase = HostedPreparePhase::Committed;
        journal.compensation.revision = Some(7);
        journal.response = Some(hosted_response(
            &request,
            Ok(json!({ "sessionId": request.session_id })),
        ));
        let restore_path = paths::browser_workspace_restore_json(&request.session_token);
        let runtime_path = paths::browser_workspace_state_json(&request.session_token);
        std::fs::create_dir_all(restore_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(runtime_path.parent().unwrap()).unwrap();
        std::fs::write(&restore_path, b"committed-restore").unwrap();
        std::fs::write(&runtime_path, b"stale-process-local-targets").unwrap();
        write_hosted_prepare_journal(&journal).unwrap();
        // The host crashed after Committed but before publishing a response.
        // The still-live wrapper has not reached its timeout yet.

        recover_hosted_prepare_journals_for_process_start().unwrap();

        assert_eq!(std::fs::read(&restore_path).unwrap(), b"committed-restore");
        assert!(!runtime_path.exists());
        assert!(hosted_prepare_journal_path(&request).exists());

        // Reset-induced artifact absence is not a wrapper acknowledgement for
        // a commit recovered from another host process.
        reset_host_request_directory_for_process_start(&paths::browser_host_requests_dir())
            .unwrap();
        let mut locally_committed = HashSet::new();
        reap_acknowledged_committed_prepare_journals(&mut locally_committed).unwrap();
        assert!(hosted_prepare_journal_path(&request).exists());

        // The old wrapper reaches its timeout after the new host is already up.
        // The retained WAL still supplies exact compensation metadata.
        let cancellation_path = hosted_cancellation_path(&request);
        std::fs::write(
            &cancellation_path,
            serde_json::to_vec(&hosted_internal_cancellation_value(&request, 100_002, None))
                .unwrap(),
        )
        .unwrap();
        recover_hosted_prepare_journals_for_process_start().unwrap();
        assert!(!restore_path.exists());
        assert!(!hosted_prepare_journal_path(&request).exists());
    }

    #[test]
    fn startup_matching_cancel_overrides_committed_prepare_before_transient_reset() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let mut journal = new_hosted_prepare_journal(&request, "prepared_session", 100_001);
        journal.phase = HostedPreparePhase::Committed;
        journal.compensation.revision = Some(7);
        journal.response = Some(hosted_response(
            &request,
            Ok(json!({ "sessionId": request.session_id })),
        ));
        let restore_path = paths::browser_workspace_restore_json(&request.session_token);
        let runtime_path = paths::browser_workspace_state_json(&request.session_token);
        std::fs::create_dir_all(restore_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(runtime_path.parent().unwrap()).unwrap();
        std::fs::write(&restore_path, b"must-be-rolled-back").unwrap();
        std::fs::write(
            &runtime_path,
            serde_json::to_vec(&json!({
                "version": 2,
                "mapping_authority": "host",
                "revision": 7,
                "session_token": request.session_token,
                "active_tab": "tab-a",
                "tabs": [{ "token": "tab-a", "target_id": "target-a" }],
            }))
            .unwrap(),
        )
        .unwrap();
        write_hosted_prepare_journal(&journal).unwrap();
        let cancellation_path = hosted_cancellation_path(&request);
        std::fs::create_dir_all(cancellation_path.parent().unwrap()).unwrap();
        std::fs::write(
            &cancellation_path,
            serde_json::to_vec(&hosted_internal_cancellation_value(
                &request,
                100_002,
                Some(&journal.compensation),
            ))
            .unwrap(),
        )
        .unwrap();

        // This is the exact spawn_watch ordering: durable recovery consumes
        // the old tombstone before reset quarantines transient artifacts.
        recover_hosted_prepare_journals_for_process_start().unwrap();
        reset_host_request_directory_for_process_start(&paths::browser_host_requests_dir())
            .unwrap();

        assert!(!restore_path.exists());
        assert!(!runtime_path.exists());
        assert!(!hosted_prepare_journal_path(&request).exists());
        assert_eq!(
            std::fs::read_dir(paths::browser_host_requests_dir())
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn startup_superseded_committed_prepare_preserves_newer_restore_without_cancel() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let mut journal = new_hosted_prepare_journal(&request, "prepared_session", 100_001);
        journal.phase = HostedPreparePhase::Committed;
        journal.compensation.revision = Some(7);
        journal.response = Some(hosted_response(
            &request,
            Ok(json!({ "sessionId": request.session_id })),
        ));
        let restore_path = paths::browser_workspace_restore_json(&request.session_token);
        let runtime_path = paths::browser_workspace_state_json(&request.session_token);
        std::fs::create_dir_all(restore_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(runtime_path.parent().unwrap()).unwrap();
        std::fs::write(&restore_path, b"newer-user-visible-restore").unwrap();
        std::fs::write(
            &runtime_path,
            serde_json::to_vec(&json!({
                "version": 2,
                "mapping_authority": "host",
                "revision": 8,
                "session_token": request.session_token,
                "active_tab": "tab-b",
                "tabs": [{ "token": "tab-b", "target_id": "target-b" }],
            }))
            .unwrap(),
        )
        .unwrap();
        write_hosted_prepare_journal(&journal).unwrap();

        recover_hosted_prepare_journals_for_process_start().unwrap();

        assert_eq!(
            std::fs::read(&restore_path).unwrap(),
            b"newer-user-visible-restore"
        );
        assert!(!runtime_path.exists());
        assert!(!hosted_prepare_journal_path(&request).exists());

        let cancellation_path = hosted_cancellation_path(&request);
        std::fs::create_dir_all(cancellation_path.parent().unwrap()).unwrap();
        std::fs::write(
            &cancellation_path,
            serde_json::to_vec(&hosted_internal_cancellation_value(
                &request,
                100_002,
                Some(&journal.compensation),
            ))
            .unwrap(),
        )
        .unwrap();
        recover_hosted_prepare_journals_for_process_start().unwrap();
        assert_eq!(
            std::fs::read(&restore_path).unwrap(),
            b"newer-user-visible-restore"
        );
    }

    #[test]
    fn startup_finishes_crash_safe_supersession_neutralization() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let mut original = new_hosted_prepare_journal(&request, "prepared_session", 100_001);
        original.compensation.revision = Some(7);
        let old_compensation = original.compensation.clone();
        let mut journal = new_hosted_prepare_journal(&request, "none", 100_001);
        journal.phase = HostedPreparePhase::Cancelled;
        let restore_path = paths::browser_workspace_restore_json(&request.session_token);
        let runtime_path = paths::browser_workspace_state_json(&request.session_token);
        std::fs::create_dir_all(restore_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(runtime_path.parent().unwrap()).unwrap();
        std::fs::write(&restore_path, b"newer-user-visible-restore").unwrap();
        std::fs::write(
            &runtime_path,
            serde_json::to_vec(&json!({
                "version": 2,
                "mapping_authority": "host",
                "revision": 8,
                "session_token": request.session_token,
                "active_tab": "tab-b",
                "tabs": [{ "token": "tab-b", "target_id": "target-b" }],
            }))
            .unwrap(),
        )
        .unwrap();
        write_hosted_prepare_journal(&journal).unwrap();
        let cancellation_path = hosted_cancellation_path(&request);
        std::fs::create_dir_all(cancellation_path.parent().unwrap()).unwrap();
        std::fs::write(
            &cancellation_path,
            serde_json::to_vec(&hosted_internal_cancellation_value(
                &request,
                100_002,
                Some(&old_compensation),
            ))
            .unwrap(),
        )
        .unwrap();

        recover_hosted_prepare_journals_for_process_start().unwrap();

        assert_eq!(
            std::fs::read(&restore_path).unwrap(),
            b"newer-user-visible-restore"
        );
        assert!(!runtime_path.exists());
        assert!(!hosted_prepare_journal_path(&request).exists());
    }

    #[test]
    fn host_runtime_revision_requires_exact_authoritative_identity() {
        let token = "0123456789abcdef";
        let valid = json!({
            "version": 2,
            "mapping_authority": "host",
            "revision": 8,
            "session_token": token,
        });
        assert_eq!(parse_host_runtime_revision(&valid, token), Some(8));
        assert_eq!(
            parse_host_runtime_revision(&valid, "fedcba9876543210"),
            None
        );
        for invalid in [
            json!({ "version": 1, "mapping_authority": "host", "revision": 8, "session_token": token }),
            json!({ "version": 2, "mapping_authority": "frontend", "revision": 8, "session_token": token }),
            json!({ "version": 2, "mapping_authority": "host", "revision": 0, "session_token": token }),
            json!({ "version": 2, "mapping_authority": "host", "revision": "8", "session_token": token }),
        ] {
            assert_eq!(parse_host_runtime_revision(&invalid, token), None);
        }
    }

    #[test]
    fn committed_journal_is_removed_only_after_response_artifacts_are_consumed() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let mut journal = new_hosted_prepare_journal(&request, "prepared_session", 100_001);
        journal.phase = HostedPreparePhase::Committed;
        journal.compensation.revision = Some(7);
        let response = hosted_response(&request, Ok(json!({ "sessionId": request.session_id })));
        journal.response = Some(response.clone());
        write_hosted_prepare_journal(&journal).unwrap();
        let request_path = paths::browser_host_requests_dir().join(format!(
            "{}-{}.json",
            request.session_token, request.request_id
        ));
        std::fs::create_dir_all(request_path.parent().unwrap()).unwrap();
        std::fs::write(&request_path, b"request").unwrap();
        write_hosted_response(&request_path, &response).unwrap();
        let key = (request.session_token.clone(), request.request_id.clone());
        let mut locally_committed = HashSet::from([key.clone()]);

        reap_acknowledged_committed_prepare_journals(&mut locally_committed).unwrap();
        assert!(hosted_prepare_journal_path(&request).exists());
        assert!(locally_committed.contains(&key));

        std::fs::remove_file(request_path.with_extension("response")).unwrap();
        std::fs::remove_file(&request_path).unwrap();
        reap_acknowledged_committed_prepare_journals(&mut locally_committed).unwrap();
        assert!(!hosted_prepare_journal_path(&request).exists());
        assert!(!locally_committed.contains(&key));
    }

    #[test]
    fn distinct_prepare_supersedes_late_old_cancel_without_deleting_new_wal() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let old_request = valid_hosted_prepare_request(100_000);
        let mut new_request = valid_hosted_prepare_request(100_010);
        new_request.request_id = "request-b".to_string();
        new_request.idempotency_key = format!("{}/request-b", new_request.session_token);
        new_request.wrapper_instance_nonce = "fedcba9876543210fedcba9876543210".to_string();
        let new_journal = new_hosted_prepare_journal(&new_request, "restored_session", 100_011);
        write_hosted_prepare_journal(&new_journal).unwrap();

        let old_cancellation_path = hosted_cancellation_path(&old_request);
        std::fs::create_dir_all(old_cancellation_path.parent().unwrap()).unwrap();
        std::fs::write(
            &old_cancellation_path,
            serde_json::to_vec(&hosted_internal_cancellation_value(
                &old_request,
                100_012,
                None,
            ))
            .unwrap(),
        )
        .unwrap();
        let cancellation: HostedBrowserCancellation =
            serde_json::from_slice(&std::fs::read(&old_cancellation_path).unwrap()).unwrap();
        validate_hosted_cancellation(&cancellation, &old_cancellation_path).unwrap();

        assert!(
            matching_hosted_prepare_journal_for_cancellation(&cancellation)
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            classify_hosted_prepare_journal_for_cancellation(&cancellation).unwrap(),
            HostedPrepareJournalMatch::Superseded
        ));
        remove_matching_hosted_prepare_journal(&cancellation).unwrap();
        remove_hosted_request_artifacts(&old_cancellation_path).unwrap();

        assert!(!old_cancellation_path.exists());
        let retained =
            read_hosted_prepare_journal(&hosted_prepare_journal_path(&new_request)).unwrap();
        assert_eq!(retained.compensation.request_id, new_request.request_id);
        assert_eq!(
            retained.compensation.wrapper_instance_nonce,
            new_request.wrapper_instance_nonce
        );
    }

    #[tokio::test]
    async fn late_cancel_rechecks_wal_identity_after_waiting_for_session_lifecycle() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let old_request = valid_hosted_prepare_request(100_000);
        let old_journal = new_hosted_prepare_journal(&old_request, "prepared_session", 100_001);
        write_hosted_prepare_journal(&old_journal).unwrap();
        let cancellation_value = hosted_internal_cancellation_value(&old_request, 100_002, None);
        let cancellation: HostedBrowserCancellation =
            serde_json::from_value(cancellation_value).unwrap();

        let manager = Arc::new(BrowserManager::new());
        let lifecycle = manager.session_lifecycle_lock(&old_request.session_id);
        let lifecycle_guard = lifecycle.lock().await;
        let remover = {
            let manager = Arc::clone(&manager);
            tokio::spawn(async move {
                manager
                    .remove_matching_hosted_prepare_journal_serialized(&cancellation)
                    .await
            })
        };
        tokio::task::yield_now().await;

        let mut new_request = valid_hosted_prepare_request(100_010);
        new_request.request_id = "request-b".to_string();
        new_request.idempotency_key = format!("{}/request-b", new_request.session_token);
        new_request.wrapper_instance_nonce = "fedcba9876543210fedcba9876543210".to_string();
        let new_journal = new_hosted_prepare_journal(&new_request, "restored_session", 100_011);
        write_hosted_prepare_journal(&new_journal).unwrap();
        drop(lifecycle_guard);

        remover.await.unwrap().unwrap();
        let retained =
            read_hosted_prepare_journal(&hosted_prepare_journal_path(&new_request)).unwrap();
        assert_eq!(retained.compensation.request_id, new_request.request_id);
        assert_eq!(
            retained.compensation.wrapper_instance_nonce,
            new_request.wrapper_instance_nonce
        );
    }

    #[test]
    fn failed_created_blank_delete_retains_wal_and_retries_after_repair() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let journal = new_hosted_prepare_journal(&request, "prepared_session", 100_001);
        let restore_path = paths::browser_workspace_restore_json(&request.session_token);
        std::fs::create_dir_all(&restore_path).unwrap();
        write_hosted_prepare_journal(&journal).unwrap();

        assert!(recover_hosted_prepare_journals_for_process_start().is_err());
        assert!(hosted_prepare_journal_path(&request).exists());

        std::fs::remove_dir(&restore_path).unwrap();
        recover_hosted_prepare_journals_for_process_start().unwrap();
        assert!(!hosted_prepare_journal_path(&request).exists());
    }

    #[test]
    fn startup_corrupt_prepare_journal_isolated_per_token_allows_fresh_generation() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let journal_path = hosted_prepare_journal_path(&request);
        let restore_path = paths::browser_workspace_restore_json(&request.session_token);
        let runtime_path = paths::browser_workspace_state_json(&request.session_token);
        std::fs::create_dir_all(journal_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(restore_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(runtime_path.parent().unwrap()).unwrap();
        std::fs::write(&journal_path, b"{truncated").unwrap();
        std::fs::write(&restore_path, b"unknown-restore-phase").unwrap();
        std::fs::write(&runtime_path, b"unknown-runtime-generation").unwrap();

        recover_hosted_prepare_journals_for_process_start().unwrap();
        assert!(!journal_path.exists());
        assert!(!restore_path.exists());
        assert!(!runtime_path.exists());
        let first_slot = hosted_prepare_quarantine_slot_dir(
            &hosted_prepare_quarantine_token_dir(&request.session_token),
            0,
        );
        assert_eq!(
            std::fs::read(hosted_prepare_quarantine_state_path(&first_slot, "restore")).unwrap(),
            b"unknown-restore-phase"
        );
        assert_eq!(
            std::fs::read(hosted_prepare_quarantine_state_path(&first_slot, "runtime")).unwrap(),
            b"unknown-runtime-generation"
        );
        assert_eq!(
            std::fs::read(hosted_prepare_quarantine_state_path(&first_slot, "journal")).unwrap(),
            b"{truncated"
        );

        let manager = BrowserManager::new();
        manager.bind_session_validator(Arc::new(|_| true));
        assert!(manager
            .ensure_browser_session_allowed(&request.session_id)
            .is_ok());
        assert!(manager
            .ensure_browser_session_allowed("brand-new-session")
            .is_ok());

        // A second damaged generation for the same task gets a new slot. The
        // first slot is never mistaken for active restore/runtime state.
        std::fs::write(&journal_path, b"{truncated-again").unwrap();
        std::fs::write(&restore_path, b"second-restore").unwrap();
        std::fs::write(&runtime_path, b"second-runtime").unwrap();
        recover_hosted_prepare_journals_for_process_start().unwrap();
        assert!(!journal_path.exists());
        assert!(!restore_path.exists());
        assert!(!runtime_path.exists());
        let second_slot = hosted_prepare_quarantine_slot_dir(
            &hosted_prepare_quarantine_token_dir(&request.session_token),
            1,
        );
        assert_eq!(
            std::fs::read(hosted_prepare_quarantine_state_path(
                &second_slot,
                "journal"
            ))
            .unwrap(),
            b"{truncated-again"
        );

        remove_hosted_prepare_quarantine_for_session(&request.session_id).unwrap();
        assert!(!hosted_prepare_quarantine_token_dir(&request.session_token).exists());
    }

    #[test]
    fn startup_corrupt_prepare_journal_with_untrusted_name_isolated_without_task_mutation() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let journal_path = hosted_prepare_journal_dir().join("NOT-A-SESSION-TOKEN.json");
        let restore_path = paths::browser_workspace_restore_json(&request.session_token);
        let runtime_path = paths::browser_workspace_state_json(&request.session_token);
        std::fs::create_dir_all(journal_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(restore_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(runtime_path.parent().unwrap()).unwrap();
        std::fs::write(&journal_path, b"{untrusted-name").unwrap();
        std::fs::write(&restore_path, b"unrelated-restore").unwrap();
        std::fs::write(&runtime_path, b"unrelated-runtime").unwrap();

        recover_hosted_prepare_journals_for_process_start().unwrap();

        assert!(!journal_path.exists());
        assert_eq!(std::fs::read(&restore_path).unwrap(), b"unrelated-restore");
        assert_eq!(std::fs::read(&runtime_path).unwrap(), b"unrelated-runtime");
        let slot =
            hosted_prepare_quarantine_slot_dir(&hosted_prepare_unassigned_quarantine_dir(), 0);
        assert_eq!(
            std::fs::read(hosted_prepare_quarantine_state_path(&slot, "journal")).unwrap(),
            b"{untrusted-name"
        );
        let manager = BrowserManager::new();
        manager.bind_session_validator(Arc::new(|_| true));
        assert!(manager
            .ensure_browser_session_allowed("brand-new-session")
            .is_ok());
    }

    #[test]
    fn startup_corrupt_prepare_quarantine_failure_keeps_journal_last_and_retries() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let journal_path = hosted_prepare_journal_path(&request);
        let restore_path = paths::browser_workspace_restore_json(&request.session_token);
        let runtime_path = paths::browser_workspace_state_json(&request.session_token);
        std::fs::create_dir_all(journal_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(restore_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(runtime_path.parent().unwrap()).unwrap();
        std::fs::write(&journal_path, b"{truncated").unwrap();
        std::fs::write(&restore_path, b"restore").unwrap();
        std::fs::write(&runtime_path, b"runtime").unwrap();
        let slot = hosted_prepare_quarantine_slot_dir(
            &hosted_prepare_quarantine_token_dir(&request.session_token),
            0,
        );
        std::fs::create_dir_all(&slot).unwrap();
        let restore_destination = hosted_prepare_quarantine_state_path(&slot, "restore");
        std::fs::create_dir(&restore_destination).unwrap();

        let error = recover_hosted_prepare_journals_for_process_start()
            .expect_err("a partial coordinated move must remain fail-closed");
        assert!(error.contains("Failed to inspect quarantined browser restore manifest"));
        assert!(journal_path.is_file(), "the journal marker must move last");
        assert!(restore_path.is_file());
        assert!(!runtime_path.exists());
        assert!(hosted_prepare_quarantine_state_path(&slot, "runtime").is_file());

        std::fs::remove_dir(restore_destination).unwrap();
        recover_hosted_prepare_journals_for_process_start().unwrap();
        assert!(!journal_path.exists());
        assert!(!restore_path.exists());
        assert!(hosted_prepare_quarantine_state_path(&slot, "restore").is_file());
        assert!(hosted_prepare_quarantine_state_path(&slot, "journal").is_file());
    }

    #[test]
    fn startup_journal_directory_io_error_fails_closed() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let journal_dir = hosted_prepare_journal_dir();
        std::fs::create_dir_all(journal_dir.parent().unwrap()).unwrap();
        std::fs::write(&journal_dir, b"not-a-directory").unwrap();

        assert!(recover_hosted_prepare_journals_for_process_start().is_err());
        std::fs::remove_file(&journal_dir).unwrap();
        recover_hosted_prepare_journals_for_process_start()
            .expect("the re-armed startup recovery succeeds after the I/O fault is repaired");
    }

    #[test]
    fn prepare_quarantine_reconciliation_removes_only_old_orphan_slots() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let active_token = paths::browser_session_token("active-session");
        let orphan_token = paths::browser_session_token("orphan-session");
        let fresh_token = paths::browser_session_token("fresh-session");
        let write_complete_slot = |parent: PathBuf| {
            let slot = hosted_prepare_quarantine_slot_dir(&parent, 0);
            std::fs::create_dir_all(&slot).unwrap();
            std::fs::write(
                hosted_prepare_quarantine_state_path(&slot, "journal"),
                b"quarantined",
            )
            .unwrap();
            slot
        };
        let active_slot = write_complete_slot(hosted_prepare_quarantine_token_dir(&active_token));
        let orphan_slot = write_complete_slot(hosted_prepare_quarantine_token_dir(&orphan_token));
        let unassigned_slot = write_complete_slot(hosted_prepare_unassigned_quarantine_dir());

        reconcile_hosted_prepare_quarantine_files(
            &HashSet::from([active_token.clone()]),
            SystemTime::now() + Duration::from_secs(1),
        )
        .unwrap();
        assert!(active_slot.exists());
        assert!(!orphan_slot.exists());
        assert!(!unassigned_slot.exists());

        let fresh_slot = write_complete_slot(hosted_prepare_quarantine_token_dir(&fresh_token));
        reconcile_hosted_prepare_quarantine_files(&HashSet::new(), SystemTime::UNIX_EPOCH).unwrap();
        assert!(fresh_slot.exists());
    }

    #[cfg(unix)]
    #[test]
    fn prepare_recovery_and_cleanup_reject_symlink_roots_without_touching_targets() {
        use std::os::unix::fs::symlink;

        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);

        let external_journal_dir = temp.path().join("external-journal");
        std::fs::create_dir_all(&external_journal_dir).unwrap();
        let external_journal = external_journal_dir.join(format!("{}.json", request.session_token));
        let journal = new_hosted_prepare_journal(&request, "prepared_session", 100_001);
        let external_journal_bytes = serde_json::to_vec(&journal).unwrap();
        std::fs::write(&external_journal, &external_journal_bytes).unwrap();
        std::fs::create_dir_all(paths::browser_home()).unwrap();
        symlink(&external_journal_dir, hosted_prepare_journal_dir()).unwrap();

        assert!(write_hosted_prepare_journal(&journal).is_err());
        assert_eq!(
            std::fs::read(&external_journal).unwrap(),
            external_journal_bytes
        );
        assert!(recover_hosted_prepare_journals_for_process_start().is_err());
        assert!(external_journal.is_file());
        std::fs::remove_file(hosted_prepare_journal_dir()).unwrap();

        std::fs::create_dir_all(hosted_prepare_journal_dir()).unwrap();
        let linked_journal = hosted_prepare_journal_path(&request);
        symlink(&external_journal, &linked_journal).unwrap();
        let restore_path = paths::browser_workspace_restore_json(&request.session_token);
        let runtime_path = paths::browser_workspace_state_json(&request.session_token);
        std::fs::create_dir_all(restore_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(runtime_path.parent().unwrap()).unwrap();
        std::fs::write(&restore_path, b"restore-must-remain").unwrap();
        std::fs::write(&runtime_path, b"runtime-must-remain").unwrap();
        assert!(recover_hosted_prepare_journals_for_process_start().is_err());
        assert_eq!(
            std::fs::read(&restore_path).unwrap(),
            b"restore-must-remain"
        );
        assert_eq!(
            std::fs::read(&runtime_path).unwrap(),
            b"runtime-must-remain"
        );
        std::fs::remove_file(linked_journal).unwrap();

        let dangling_target = temp.path().join("missing-prepare-journal");
        let dangling_journal = hosted_prepare_journal_path(&request);
        symlink(&dangling_target, &dangling_journal).unwrap();
        assert!(remove_hosted_prepare_journal_for_session(&request.session_id).is_err());
        assert!(std::fs::symlink_metadata(&dangling_journal).is_ok());
        std::fs::remove_file(dangling_journal).unwrap();

        let external_quarantine = temp.path().join("external-quarantine");
        let external_slot = hosted_prepare_quarantine_slot_dir(
            &external_quarantine.join(&request.session_token),
            0,
        );
        std::fs::create_dir_all(&external_slot).unwrap();
        let external_marker = hosted_prepare_quarantine_state_path(&external_slot, "journal");
        std::fs::write(&external_marker, b"must-remain").unwrap();
        symlink(&external_quarantine, hosted_prepare_quarantine_dir()).unwrap();

        assert!(remove_hosted_prepare_quarantine_for_token(&request.session_token).is_err());
        assert!(reconcile_hosted_prepare_quarantine_files(
            &HashSet::new(),
            SystemTime::now() + Duration::from_secs(1),
        )
        .is_err());
        assert_eq!(std::fs::read(&external_marker).unwrap(), b"must-remain");
    }

    #[cfg(unix)]
    #[test]
    fn session_file_reconciliation_rejects_symlink_roots_without_touching_targets() {
        use std::os::unix::fs::symlink;

        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let external = temp.path().join("external-session-files");
        std::fs::create_dir_all(&external).unwrap();
        let sentinel = external.join("0123456789abcdef.json");
        std::fs::write(&sentinel, b"outside-must-remain").unwrap();
        let manager = BrowserManager::new();

        for root in [
            paths::browser_workspace_restore_dir(),
            paths::browser_workspaces_dir(),
            paths::browser_session_mcp_dir(),
            hosted_prepare_journal_dir(),
        ] {
            std::fs::create_dir_all(root.parent().unwrap()).unwrap();
            symlink(&external, &root).unwrap();
            let error = manager.reconcile_session_files(&[]).unwrap_err();
            assert!(
                error.contains("Failed to open browser session directory"),
                "unexpected reconciliation error for {}: {error}",
                root.display()
            );
            assert_eq!(std::fs::read(&sentinel).unwrap(), b"outside-must-remain");
            std::fs::remove_file(root).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn destructive_session_cleanup_rejects_replaced_restore_and_mcp_roots() {
        use std::os::unix::fs::symlink;

        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let cleanup_paths = [
            paths::browser_workspace_restore_json("restore-session-token"),
            paths::browser_session_mcp_json("mcp-session"),
        ];

        for (index, cleanup_path) in cleanup_paths.into_iter().enumerate() {
            let root = cleanup_path.parent().unwrap();
            std::fs::create_dir_all(root.parent().unwrap()).unwrap();
            let external = temp.path().join(format!("external-cleanup-{index}"));
            std::fs::create_dir_all(&external).unwrap();
            let sentinel = external.join(cleanup_path.file_name().unwrap());
            std::fs::write(&sentinel, b"outside-must-remain").unwrap();
            symlink(&external, root).unwrap();

            let error = remove_private_plain_file_and_verify_absent(
                &cleanup_path,
                "destructive browser cleanup",
            )
            .unwrap_err();
            assert!(
                error.contains("Failed to open destructive browser cleanup directory"),
                "unexpected cleanup error for {}: {error}",
                root.display()
            );
            assert_eq!(std::fs::read(&sentinel).unwrap(), b"outside-must-remain");
            std::fs::remove_file(root).unwrap();
        }
    }

    #[test]
    fn startup_reconciliation_removes_only_orphan_browser_json_files() {
        let temp = tempfile::tempdir().unwrap();
        let directories = [
            temp.path().join("restore"),
            temp.path().join("workspaces"),
            temp.path().join("mcp-sessions"),
            temp.path().join("prepare-journal"),
        ];
        for directory in &directories {
            std::fs::create_dir_all(directory).unwrap();
            std::fs::write(directory.join("active-token.json"), b"active").unwrap();
            std::fs::write(directory.join("orphan-token.json"), b"orphan").unwrap();
            std::fs::write(directory.join("orphan-token.tmp"), b"temporary").unwrap();
        }
        let active = HashSet::from(["active-token".to_string()]);

        let cutoff_after_fixture = SystemTime::now() + Duration::from_secs(1);
        reconcile_browser_session_file_dirs(&active, &directories, cutoff_after_fixture).unwrap();
        // Idempotent retry after a partial/crash cleanup must also succeed.
        reconcile_browser_session_file_dirs(&active, &directories, cutoff_after_fixture).unwrap();

        for directory in &directories {
            assert!(directory.join("active-token.json").exists());
            assert!(!directory.join("orphan-token.json").exists());
            assert!(directory.join("orphan-token.tmp").exists());
        }
    }

    #[test]
    fn startup_reconciliation_never_deletes_files_newer_than_process_cutoff() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("restore");
        std::fs::create_dir_all(&directory).unwrap();
        let new_file = directory.join("new-session-token.json");
        std::fs::write(&new_file, b"new").unwrap();

        reconcile_browser_session_file_dirs(&HashSet::new(), &[directory], SystemTime::UNIX_EPOCH)
            .unwrap();
        assert!(new_file.exists());
    }

    #[test]
    fn process_start_quarantines_stale_host_requests_before_accepting_new_ones() {
        let temp = tempfile::tempdir().unwrap();
        let request_dir = temp.path().join("host-requests");
        std::fs::create_dir_all(&request_dir).unwrap();
        std::fs::write(request_dir.join("old-create.json"), b"stale").unwrap();
        std::fs::write(request_dir.join("old-close.cancelled"), b"stale").unwrap();

        reset_host_request_directory_for_process_start(&request_dir).unwrap();
        assert!(request_dir.is_dir());
        assert_eq!(std::fs::read_dir(&request_dir).unwrap().count(), 0);

        let current_request = request_dir.join("current.json");
        std::fs::write(&current_request, b"current").unwrap();
        assert!(current_request.exists());
    }

    #[test]
    fn native_visibility_rejects_old_renderer_and_out_of_order_sequence() {
        let mut clock = SurfaceVisibilityClock::default();
        let first = clock.begin_generation();
        assert_eq!(first, 1);
        assert!(clock.claim(first, 1));
        assert!(!clock.claim(first, 1));
        assert!(clock.claim(first, 10));
        assert!(!clock.claim(first, 9));

        let reloaded = clock.begin_generation();
        assert_eq!(reloaded, 2);
        assert!(!clock.claim(first, 100));
        assert!(clock.claim(reloaded, 1));
        assert_eq!(
            clock,
            SurfaceVisibilityClock {
                generation: reloaded,
                sequence: 1,
            }
        );
    }

    #[test]
    fn canceled_prepare_preserves_a_restored_manifest_but_deletes_a_new_blank() {
        assert_eq!(
            PreparedWorkspaceDisposition::RestoredExisting.rollback_kind(),
            Some("restored_session")
        );
        assert_eq!(
            PreparedWorkspaceDisposition::CreatedBlank.rollback_kind(),
            Some("prepared_session")
        );
        assert_eq!(PreparedWorkspaceDisposition::Existing.rollback_kind(), None);
        assert!(should_remove_prepare_restore("prepared_session", true));
        assert!(!should_remove_prepare_restore("prepared_session", false));
        assert!(!should_remove_prepare_restore("restored_session", true));
    }

    #[test]
    fn lease_heartbeats_and_end_use_the_independent_control_plane() {
        for operation in [
            HostedBrowserOperation::AssertHostLease,
            HostedBrowserOperation::BeginAgentOperation,
            HostedBrowserOperation::RefreshAgentOperation,
            HostedBrowserOperation::RefreshAgentInput,
            HostedBrowserOperation::EndAgentOperation,
        ] {
            assert!(operation.is_control_plane());
        }
        for operation in [
            HostedBrowserOperation::Prepare,
            HostedBrowserOperation::CreateTab,
            HostedBrowserOperation::ActivateTab,
            HostedBrowserOperation::CloseTab,
            HostedBrowserOperation::RollbackCreatedTab,
            HostedBrowserOperation::CoreTool,
        ] {
            assert!(!operation.is_control_plane());
        }
    }

    #[test]
    fn restore_requires_a_real_cdp_or_browser_core_backend() {
        let webkit_surface = platform::NativeSurfaceCapabilities::new(true, true, false);
        assert!(!has_restore_automation_backend(webkit_surface, false));
        assert!(has_restore_automation_backend(webkit_surface, true));

        let windows_surface = platform::NativeSurfaceCapabilities::new(true, true, true);
        assert!(has_restore_automation_backend(windows_surface, false));
    }

    #[test]
    fn browser_core_reuses_only_the_productized_unused_initial_blank() {
        let session_id = "session-a";
        let initial = paths::browser_session_token(session_id);
        let blank = TabInfo {
            target_id: initial.clone(),
            page_id: Some(0),
            title: "about:blank".to_string(),
            url: "about:blank".to_string(),
        };

        assert!(should_reuse_browser_core_initial_tab(
            session_id,
            &initial,
            std::slice::from_ref(&blank),
            false,
        ));
        assert!(!should_reuse_browser_core_initial_tab(
            session_id,
            &initial,
            std::slice::from_ref(&blank),
            true,
        ));

        let navigated = TabInfo {
            url: "https://example.com".to_string(),
            ..blank.clone()
        };
        assert!(!should_reuse_browser_core_initial_tab(
            session_id,
            &initial,
            &[navigated],
            false,
        ));
        let restored_blank = TabInfo {
            target_id: "fedcba9876543210".to_string(),
            ..blank.clone()
        };
        assert!(!should_reuse_browser_core_initial_tab(
            session_id,
            "fedcba9876543210",
            &[restored_blank],
            false,
        ));
        assert!(!should_reuse_browser_core_initial_tab(
            session_id,
            &initial,
            &[blank.clone(), blank],
            false,
        ));
    }

    #[test]
    fn session_validator_rejects_crash_orphans_and_pending_delete_marker_wins() {
        let manager = BrowserManager::new();
        manager.bind_session_validator(Arc::new(|session_id| session_id == "active-session"));
        assert!(manager
            .ensure_browser_session_allowed("active-session")
            .is_ok());
        assert!(manager
            .ensure_browser_session_allowed("orphan-from-previous-process")
            .is_err());

        manager.mark_session_deleted("active-session");
        assert!(manager
            .ensure_browser_session_allowed("active-session")
            .is_err());
    }

    #[tokio::test]
    async fn late_cancellation_for_absent_session_removes_artifacts_without_ledger_entry() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let manager = BrowserManager::new();
        manager.bind_session_validator(Arc::new(|_| false));
        let request = valid_hosted_prepare_request(100_000);
        let cancellation_value = hosted_internal_cancellation_value(&request, 100_001, None);
        let cancellation: HostedBrowserCancellation =
            serde_json::from_value(cancellation_value.clone()).unwrap();
        let cancellation_path = hosted_cancellation_path(&request);
        std::fs::create_dir_all(cancellation_path.parent().unwrap()).unwrap();
        std::fs::write(
            &cancellation_path,
            serde_json::to_vec(&cancellation_value).unwrap(),
        )
        .unwrap();
        std::fs::write(cancellation_path.with_extension("json"), b"request").unwrap();
        std::fs::write(cancellation_path.with_extension("response"), b"response").unwrap();
        validate_hosted_cancellation(&cancellation, &cancellation_path).unwrap();

        assert!(manager
            .discard_absent_session_cancellation(&cancellation, &cancellation_path)
            .await
            .unwrap());

        assert_eq!(manager.native_surface.lock().request_record_count(), 0);
        for extension in ["json", "response", "cancelled"] {
            assert!(!cancellation_path.with_extension(extension).exists());
        }
    }

    #[tokio::test]
    async fn successful_deleted_session_cleanup_releases_pending_deny_marker() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let manager = BrowserManager::new();
        manager.bind_session_validator(Arc::new(|session_id| session_id == "active-session"));
        manager.mark_session_deleted("active-session");
        assert!(manager
            .ensure_browser_session_allowed("active-session")
            .is_err());
        let request_dir = paths::browser_host_requests_dir();
        std::fs::create_dir_all(&request_dir).unwrap();
        let active_prefix = paths::browser_session_token("active-session");
        let other_prefix = paths::browser_session_token("other-session");
        for extension in ["json", "response", "cancelled"] {
            std::fs::write(
                request_dir.join(format!("{active_prefix}-request-a.{extension}")),
                b"active",
            )
            .unwrap();
            std::fs::write(
                request_dir.join(format!("{other_prefix}-request-b.{extension}")),
                b"other",
            )
            .unwrap();
        }

        manager
            .delete_for_session("active-session")
            .await
            .expect("idempotent browser artifact cleanup");

        assert!(manager
            .ensure_browser_session_allowed("active-session")
            .is_ok());
        assert!(manager.pending_deleted_session_ids.read().is_empty());
        for extension in ["json", "response", "cancelled"] {
            assert!(!request_dir
                .join(format!("{active_prefix}-request-a.{extension}"))
                .exists());
            assert!(request_dir
                .join(format!("{other_prefix}-request-b.{extension}"))
                .exists());
        }
    }

    #[tokio::test]
    async fn failed_deleted_session_teardown_retains_rollback_until_retry_purges_it() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let manager = BrowserManager::new();
        manager.bind_session_validator(Arc::new(|session_id| session_id == "active-session"));
        manager.mark_session_deleted("active-session");
        {
            let mut surface = manager.native_surface.lock();
            assert_eq!(
                surface
                    .claim_request("active-session", "request-retry")
                    .unwrap(),
                NativeRequestClaim::Execute
            );
            let record = json!({ "rollback": { "kind": "none" } });
            assert!(surface
                .complete_request("active-session", "request-retry", record.clone())
                .unwrap());
            assert_eq!(
                surface
                    .cancel_request("active-session", "request-retry")
                    .unwrap(),
                NativeRequestCancel::AlreadyCompleted(record)
            );
        }
        let mcp_path = paths::browser_session_mcp_json("active-session");
        std::fs::create_dir_all(&mcp_path).unwrap();

        let error = manager
            .delete_for_session("active-session")
            .await
            .expect_err("injected MCP path failure must keep teardown retryable");

        assert!(error.contains("MCP configuration"));
        assert_eq!(manager.native_surface.lock().request_record_count(), 1);
        assert!(manager
            .pending_deleted_session_ids
            .read()
            .contains("active-session"));

        std::fs::remove_dir(&mcp_path).unwrap();
        manager
            .delete_for_session("active-session")
            .await
            .expect("successful retry purges request rollback state");
        assert_eq!(manager.native_surface.lock().request_record_count(), 0);
        assert!(manager.pending_deleted_session_ids.read().is_empty());
    }

    #[test]
    fn host_consumer_startup_failure_blocks_browser_entry_points_immediately() {
        let manager = BrowserManager::new();
        *manager.prepare_recovery_error.lock() = Some(
            "browser/host-consumer-unavailable: injected request-dir reset failure".to_string(),
        );

        let error = manager
            .ensure_browser_session_allowed("session-a")
            .unwrap_err();
        assert!(error.contains("browser/host-consumer-unavailable"));
        assert!(error.contains("injected request-dir reset failure"));
    }

    // --- Per-conversation stop lifecycle routing ---

    #[test]
    fn scoped_stop_closes_a_registered_native_session() {
        assert_eq!(
            scoped_stop_action(true, true),
            ScopedStopAction::CloseNativeSession
        );
    }

    #[test]
    fn scoped_stop_ignores_an_unknown_session_while_native_workspaces_exist() {
        assert_eq!(
            scoped_stop_action(false, true),
            ScopedStopAction::IgnoreUnknownNativeSession
        );
    }

    #[test]
    fn scoped_stop_cleans_managed_runtime_without_native_workspaces() {
        assert_eq!(
            scoped_stop_action(false, false),
            ScopedStopAction::StopManagedRuntime
        );
    }

    // --- Browser-level Target event routing (targetDestroyed has only targetId) ---

    #[test]
    fn route_target_created_page_creates() {
        let params = json!({ "targetInfo": { "targetId": "T1", "type": "page" } });
        assert_eq!(
            route_target_event("Target.targetCreated", &params),
            TargetEventRoute::Create("T1".to_string())
        );
    }

    #[test]
    fn route_target_created_non_page_ignored() {
        // Non-page targets such as iframe/worker do not trigger enumeration or
        // notification, protecting against event storms.
        for ty in ["worker", "service_worker", "iframe", "other"] {
            let params = json!({ "targetInfo": { "targetId": "T1", "type": ty } });
            assert_eq!(
                route_target_event("Target.targetCreated", &params),
                TargetEventRoute::Ignore
            );
        }
    }

    #[test]
    fn route_target_destroyed_uses_top_level_target_id() {
        // Previously missed protocol shape: targetDestroyed params contain only
        // { targetId }, not targetInfo. Filtering by targetInfo.type would discard
        // every destruction event.
        let params = json!({ "targetId": "T9" });
        assert_eq!(
            route_target_event("Target.targetDestroyed", &params),
            TargetEventRoute::Destroy("T9".to_string())
        );
    }

    #[test]
    fn route_target_destroyed_without_target_id_ignored() {
        assert_eq!(
            route_target_event("Target.targetDestroyed", &json!({})),
            TargetEventRoute::Ignore
        );
        // Malformed shape: targetId inside targetInfo, the old incorrect
        // assumption, must not route.
        let params = json!({ "targetInfo": { "targetId": "T9", "type": "page" } });
        assert_eq!(
            route_target_event("Target.targetDestroyed", &params),
            TargetEventRoute::Ignore
        );
    }

    #[test]
    fn lifecycle_resync_preserves_a_live_active_target() {
        assert_eq!(
            reconciled_active_target(Some("T2"), ["T1", "T2", "T3"]),
            Some("T2".to_string())
        );
    }

    #[test]
    fn lifecycle_resync_replaces_a_dropped_destroy_with_a_live_target() {
        assert_eq!(
            reconciled_active_target(Some("destroyed"), ["T1", "T2"]),
            Some("T1".to_string())
        );
        assert_eq!(
            reconciled_active_target(Some("destroyed"), std::iter::empty()),
            None
        );
    }

    #[test]
    fn lifecycle_resync_removes_only_stale_entries_from_its_captured_generation() {
        let pages = Arc::new(parking_lot::Mutex::new(HashMap::from([
            ("stale".to_string(), "old-stale-session".to_string()),
            ("live".to_string(), "old-live-session".to_string()),
            ("concurrent".to_string(), "new-session".to_string()),
        ])));
        let captured = HashSet::from(["stale".to_string(), "live".to_string()]);
        let live = HashMap::from([("live".to_string(), "fresh-live-session".to_string())]);

        merge_reconciled_page_sessions(&pages, &captured, &live);

        assert_eq!(
            *pages.lock(),
            HashMap::from([
                ("live".to_string(), "fresh-live-session".to_string()),
                ("concurrent".to_string(), "new-session".to_string()),
            ])
        );
    }

    #[test]
    fn lifecycle_resync_requires_every_live_page_attachment() {
        assert_eq!(
            accept_page_attachment(
                PageTabAttachPolicy::BestEffort,
                "temporarily-unattachable",
                Err("target is busy".to_string()),
            ),
            Ok(false)
        );

        let error = accept_page_attachment(
            PageTabAttachPolicy::Authoritative,
            "temporarily-unattachable",
            Err("target is busy".to_string()),
        )
        .expect_err("authoritative resync must reject a partial live-page snapshot");
        assert!(error.contains("temporarily-unattachable"));
        assert!(error.contains("target is busy"));
        assert_eq!(
            accept_page_attachment(
                PageTabAttachPolicy::Authoritative,
                "attached",
                Ok("session-1".to_string()),
            ),
            Ok(true)
        );
    }

    #[test]
    fn browser_core_observation_allowlist_is_explicit_and_unknown_tools_fail_closed() {
        for tool in [
            "take_snapshot",
            "wait_for",
            "list_console_messages",
            "list_network_requests",
        ] {
            assert!(browser_core_tool_is_observational(tool), "tool={tool}");
        }
        for tool in [
            "navigate_page",
            "click",
            "fill",
            "evaluate_script",
            "future_unknown_tool",
        ] {
            assert!(!browser_core_tool_is_observational(tool), "tool={tool}");
        }
    }

    // --- URL-scheme allowlist for navigation/new tabs ---

    #[test]
    fn is_allowed_url_accepts_http_https_about_blank() {
        assert!(is_allowed_url("http://example.com"));
        assert!(is_allowed_url("https://example.com/path?q=1"));
        assert!(is_allowed_url("HTTP://EXAMPLE.COM"));
        assert!(is_allowed_url("Https://example.com"));
        assert!(is_allowed_url("about:blank"));
        assert!(is_allowed_url("about:blank#pinvou-tab-0123456789abcdef"));
    }

    #[test]
    fn is_allowed_url_rejects_local_and_script_schemes() {
        assert!(!is_allowed_url("file:///etc/passwd"));
        assert!(!is_allowed_url("javascript:alert(1)"));
        assert!(!is_allowed_url("data:text/html,<script></script>"));
        assert!(!is_allowed_url("chrome://settings"));
        assert!(!is_allowed_url(
            "pinvou-user-takeover://interaction/pointerdown"
        ));
        assert!(!is_allowed_url("http://tauri.localhost/"));
        assert!(!is_allowed_url("https://TAURI.LOCALHOST/index.html"));
        assert!(is_allowed_url("https://tauri.localhost.example.com/"));
        assert!(!is_allowed_url("http://127.0.0.1:1420/"));
        assert!(!is_allowed_url("http://LOCALHOST:1420/src/app/main.jsx"));
        assert!(!is_allowed_url("http://[::1]:1420/"));
        assert!(is_allowed_url("http://127.0.0.1:1421/"));
        assert!(!is_allowed_url(""));
        // Very short strings: get(..7)/get(..8) returns None and must not panic.
        assert!(!is_allowed_url("http:"));
        assert!(!is_allowed_url("ht"));
        // Non-ASCII prefix: get slicing at a non-character boundary returns None
        // and must not panic.
        assert!(!is_allowed_url("ｈｔｔｐ://example.com"));
    }

    // --- Port-file parsing (range checks prevent as-u16 wrapping) ---

    #[test]
    fn parse_port_json_accepts_valid_ports() {
        assert_eq!(parse_port_json(r#"{"port": 9222}"#), Some(9222));
        assert_eq!(parse_port_json(r#"{"port": 1}"#), Some(1));
        assert_eq!(parse_port_json(r#"{"port": 65535}"#), Some(65535));
    }

    #[test]
    fn cdp_version_accepts_edge_webview2_and_chrome_loopback_endpoints() {
        for browser in [
            "Edg/140.0.0.0",
            "Chrome/140.0.0.0",
            "HeadlessChrome/140.0.0.0",
        ] {
            let version = json!({
                "Browser": browser,
                "Protocol-Version": "1.3",
                "webSocketDebuggerUrl": "ws://localhost:9222/devtools/browser/01234567-89ab-cdef"
            });
            assert!(cdp_version_matches_loopback_endpoint(&version, 9222));
        }
    }

    #[test]
    fn cdp_version_rejects_foreign_or_misdirected_endpoints() {
        let valid = json!({
            "Browser": "Edg/140.0.0.0",
            "Protocol-Version": "1.3",
            "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/browser/01234567-89ab-cdef"
        });
        assert!(cdp_version_matches_loopback_endpoint(&valid, 9222));

        for invalid in [
            json!({
                "Browser": "Firefox/140.0",
                "Protocol-Version": "1.3",
                "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/browser/id"
            }),
            json!({
                "Browser": "Edg/140.0.0.0",
                "Protocol-Version": "2.0",
                "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/browser/id"
            }),
            json!({
                "Browser": "Edg/140.0.0.0",
                "Protocol-Version": "1.3",
                "webSocketDebuggerUrl": "ws://example.com:9222/devtools/browser/id"
            }),
            json!({
                "Browser": "Edg/140.0.0.0",
                "Protocol-Version": "1.3",
                "webSocketDebuggerUrl": "ws://127.0.0.1:9333/devtools/browser/id"
            }),
            json!({
                "Browser": "Edg/140.0.0.0",
                "Protocol-Version": "1.3",
                "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/id"
            }),
        ] {
            assert!(!cdp_version_matches_loopback_endpoint(&invalid, 9222));
        }
    }

    #[test]
    fn parse_port_json_rejects_out_of_range_and_garbage() {
        assert_eq!(parse_port_json(r#"{"port": 0}"#), None);
        assert_eq!(parse_port_json(r#"{"port": 65536}"#), None);
        assert_eq!(parse_port_json(r#"{"port": 70000}"#), None);
        // Non-numeric, negative, missing field, and invalid JSON.
        assert_eq!(parse_port_json(r#"{"port": "9222"}"#), None);
        assert_eq!(parse_port_json(r#"{"port": -1}"#), None);
        assert_eq!(parse_port_json(r#"{"pid": 123}"#), None);
        assert_eq!(parse_port_json("not json"), None);
    }

    #[test]
    fn host_owned_port_rejects_external_browser_endpoints() {
        assert_eq!(
            parse_host_owned_port_json(r#"{"port":9222,"owner":"app"}"#),
            Some(9222)
        );
        assert_eq!(
            parse_host_owned_port_json(r#"{"port":9222,"owner":"mcp"}"#),
            None
        );
        assert_eq!(
            parse_host_owned_port_json(r#"{"port":9222,"owner":"app","browser_pid":1234}"#),
            None
        );
        assert_eq!(parse_host_owned_port_json(r#"{"port":9222}"#), None);
    }
}
