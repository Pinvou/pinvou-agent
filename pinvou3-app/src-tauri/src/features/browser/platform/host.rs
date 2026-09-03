//! Shared hosting layer for desktop system WebViews.
//!
//! Workspace, tab, layout, and page lifecycles are browser-engine independent.
//! Platform implementations only configure the WebView builder and declare an
//! available automation backend. macOS and Linux can therefore reuse real
//! system WebViews without being misreported as Chrome CDP capable.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, LazyLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tauri::{
    Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewBuilder, WebviewUrl,
    webview::{DownloadEvent, NewWindowResponse, PageLoadEvent},
};

use super::super::{NativeSurfaceBounds, TabInfo};
use super::state::{
    AgentCallerEpoch, ControlSnapshot, NativeControlOwner, NativeRequestCancel, NativeRequestClaim,
    NativeTabLease, NavigationCommitDecision, RequestLedger, RetainedAgentOperation, SurfaceEntry,
    TabRegistry, UserNavigationState, WorkspaceControl,
};
use super::{NativeSurfaceCapabilities, NativeWorkspaceRestore};

const BROWSER_CORE_RUNTIME: &str =
    include_str!("../../../../resources/common/bundle/mcp-servers/browser-core-runtime.js");
use crate::platform::paths;

const WEBVIEW_LABEL_PREFIX: &str = "agent-browser-";
const USER_TAKEOVER_SCHEME: &str = "pinvou-user-takeover";
const LOCATION_CHANGE_SCHEME: &str = "pinvou-location-change";
const USER_CONTROL_IDLE_TIMEOUT: Duration = Duration::from_secs(3);
const WORKSPACE_RESTORE_VERSION: u8 = 1;
/// Maximum combined published and hidden staging tabs per task browser. Restore
/// manifests and runtime creation share this limit so window.open or concurrent
/// new_page calls cannot create child WebViews without bound.
const MAX_WORKSPACE_TABS: usize = 64;
const MAX_RESTORE_URL_LEN: usize = 16 * 1024;
const MAX_SAFE_PAGE_ID: u64 = (1_u64 << 53) - 1;
const NATIVE_PAGE_ID_SEQUENCE_BITS: u32 = 21;
const NATIVE_PAGE_ID_SEQUENCE_LIMIT: u64 = 1_u64 << NATIVE_PAGE_ID_SEQUENCE_BITS;
const NATIVE_PAGE_ID_INCARNATION_LIMIT: u64 = 1_u64 << (53 - NATIVE_PAGE_ID_SEQUENCE_BITS);
const ACTION_COMMIT_UNKNOWN_TAB_NAVIGATION: &str =
    "browser/action-commit-unknown-after-tab-navigation";
static NATIVE_PAGE_ID_INCARNATION: LazyLock<u64> =
    LazyLock::new(|| rand::random::<u64>() % NATIVE_PAGE_ID_INCARNATION_LIMIT);
static NEXT_NATIVE_PAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct WebviewBuildError {
    message: String,
    /// `add_child` succeeded, but the mandatory initial hide and the compensating
    /// close both failed.  The WebView is still a real native resource and must
    /// be transferred into the host registry instead of being orphaned.
    survivor: Option<SurfaceEntry>,
}

impl WebviewBuildError {
    fn new(message: String) -> Self {
        Self {
            message,
            survivor: None,
        }
    }

    fn with_survivor(message: String, survivor: SurfaceEntry) -> Self {
        Self {
            message,
            survivor: Some(survivor),
        }
    }
}

fn next_native_page_id() -> Result<u64, String> {
    let sequence = NEXT_NATIVE_PAGE_SEQUENCE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            (current < NATIVE_PAGE_ID_SEQUENCE_LIMIT).then(|| current + 1)
        })
        .map_err(|_| "Browser pageId space is exhausted; restart the app".to_string())?;
    compose_native_page_id(*NATIVE_PAGE_ID_INCARNATION, sequence)
        .ok_or_else(|| "Browser pageId space is exhausted; restart the app".to_string())
}

fn compose_native_page_id(incarnation: u64, sequence: u64) -> Option<u64> {
    if incarnation >= NATIVE_PAGE_ID_INCARNATION_LIMIT || sequence >= NATIVE_PAGE_ID_SEQUENCE_LIMIT
    {
        return None;
    }
    let page_id = (incarnation << NATIVE_PAGE_ID_SEQUENCE_BITS) | sequence;
    (page_id <= MAX_SAFE_PAGE_ID).then_some(page_id)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceRestoreFile {
    version: u8,
    active_index: usize,
    tabs: Vec<WorkspaceRestoreTab>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceRestoreTab {
    url: String,
}

pub(crate) trait PlatformWebviewConfig: Default {
    /// BrowserManager `prepare` means that this same page supports Agent
    /// automation. WebKit platforms must keep this false until their own tool
    /// backend is connected so callers cannot attempt CDP by mistake.
    const ACTIVATION_READY: bool;

    fn capabilities(&self) -> NativeSurfaceCapabilities;

    fn requires_reset(&self, automation_port: Option<u16>, data_directory: &Path) -> bool;

    fn prepare(
        &mut self,
        automation_port: Option<u16>,
        data_directory: &Path,
    ) -> Result<(), String>;

    fn configure_builder(
        &self,
        builder: WebviewBuilder<tauri::Wry>,
        data_directory: &Path,
    ) -> Result<WebviewBuilder<tauri::Wry>, String>;

    fn reset(&mut self);

    fn owns_port(&self, port: u16) -> bool;

    fn is_initialized(&self) -> bool;
}

struct Workspace {
    session_token: String,
    tabs: TabRegistry,
    active_tab: String,
    bounds: Option<NativeSurfaceBounds>,
    visible: bool,
    control: Arc<WorkspaceControl>,
    /// A host Prepare that created this process-local workspace may be
    /// compensated only while this exact request still owns the untouched
    /// preparation generation. A later UI/host prepare clears or replaces it;
    /// any user/Agent mutation advances `revision` and makes rollback a no-op.
    prepare_generation: Option<PrepareGeneration>,
}

#[derive(Debug, Clone)]
struct PrepareGeneration {
    request_id: String,
    revision: u64,
}

pub(crate) struct DesktopBrowserSurface<P: PlatformWebviewConfig> {
    platform: P,
    data_directory: Option<PathBuf>,
    workspaces: HashMap<String, Workspace>,
    /// Hidden candidate pages for Agent create_tab. A candidate moves into the
    /// workspace only after target discovery, initial navigation, and final
    /// lease CAS all succeed, so asynchronous binding failure cannot close a
    /// published page already taken over by the user.
    staged_tabs: HashMap<(String, String), SurfaceEntry>,
    /// UI/user popups also complete automation binding behind a hidden marker.
    /// This records only whether final publication stays in the background;
    /// staged_tabs continues to own and clean the candidate WebView itself.
    staged_user_tabs: HashMap<(String, String), bool>,
    /// Native children whose initial hide and compensating close both failed.
    /// They are owned for cleanup/capacity only and are never eligible for
    /// binding, activation, or publication as a normal workspace/staged tab.
    quarantined_tabs: HashMap<(String, String), SurfaceEntry>,
    active_session: Option<String>,
    requests: RequestLedger,
}

impl<P: PlatformWebviewConfig> Default for DesktopBrowserSurface<P> {
    fn default() -> Self {
        Self {
            platform: P::default(),
            data_directory: None,
            workspaces: HashMap::new(),
            staged_tabs: HashMap::new(),
            staged_user_tabs: HashMap::new(),
            quarantined_tabs: HashMap::new(),
            active_session: None,
            requests: RequestLedger::default(),
        }
    }
}

impl<P: PlatformWebviewConfig> DesktopBrowserSurface<P> {
    pub fn capabilities(&self) -> NativeSurfaceCapabilities {
        self.platform.capabilities()
    }

    /// Idempotently claim requestId. Only `Execute` may create resources.
    pub fn claim_request(
        &mut self,
        session_id: &str,
        request_id: &str,
    ) -> Result<NativeRequestClaim, String> {
        self.requests.claim(session_id, request_id)
    }

    /// Commit a request result. false means the cancellation tombstone arrived
    /// first and the caller must roll back the resource.
    pub fn complete_request(
        &mut self,
        session_id: &str,
        request_id: &str,
        result: serde_json::Value,
    ) -> Result<bool, String> {
        self.requests.complete(session_id, request_id, result)
    }

    /// Cancel a request. `AlreadyCompleted` carries the original result needed
    /// by the caller for compensating rollback.
    pub fn cancel_request(
        &mut self,
        session_id: &str,
        request_id: &str,
    ) -> Result<NativeRequestCancel, String> {
        self.requests.cancel(session_id, request_id)
    }

    pub fn acknowledge_request_cancellation(
        &mut self,
        session_id: &str,
        request_id: &str,
    ) -> Result<(), String> {
        self.requests
            .acknowledge_cancellation(session_id, request_id)
    }

    pub fn purge_session_requests(&mut self, session_id: &str) -> Result<usize, String> {
        self.requests.purge_session(session_id)
    }

    #[cfg(test)]
    pub(crate) fn request_record_count(&self) -> usize {
        self.requests.len()
    }

    /// Compatibility preparation entry point for the existing BrowserManager.
    ///
    /// macOS and Linux can host real pages, but must not return true and let
    /// chrome-devtools-mcp connect to WebKit before their own Agent tool backend
    /// is ready. These platforms expose a compilable, independently verifiable
    /// display layer through [`Self::prepare_display_only`] while existing
    /// runtime paths continue to fail safely.
    pub fn prepare(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        session_token: &str,
        port: u16,
        data_directory: &Path,
    ) -> Result<bool, String> {
        if !P::ACTIVATION_READY {
            return Ok(false);
        }
        self.prepare_surface(
            app,
            session_id,
            session_token,
            Some(port),
            data_directory,
            NativeControlOwner::Agent,
        )
    }

    /// Create a blank workspace on demand when the user opens the browser from
    /// ordinary mode. Opening the side panel alone is not user takeover, so it
    /// stays Unclaimed. Real user interaction or the Agent's first tool then
    /// claims it through the existing control protocol.
    pub fn prepare_unclaimed(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        session_token: &str,
        port: u16,
        data_directory: &Path,
    ) -> Result<bool, String> {
        if !P::ACTIVATION_READY {
            return Ok(false);
        }
        self.prepare_surface(
            app,
            session_id,
            session_token,
            Some(port),
            data_directory,
            NativeControlOwner::Unclaimed,
        )
    }

    /// Create a real system-WebView workspace without promising Agent automation.
    pub fn prepare_display_only(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        session_token: &str,
        data_directory: &Path,
    ) -> Result<bool, String> {
        self.prepare_surface(
            app,
            session_id,
            session_token,
            None,
            data_directory,
            NativeControlOwner::Unclaimed,
        )
    }

    /// Read the minimal page manifest recoverable across app processes. It is
    /// completely separate from the MCP runtime target mapping and contains no
    /// session/tab token, targetId, lease, or control-owner state.
    pub fn read_restore_workspace(
        session_id: &str,
    ) -> Result<Option<NativeWorkspaceRestore>, String> {
        let session_token = paths::browser_session_token(session_id);
        let path = paths::browser_workspace_restore_json(&session_token);
        let encoded = match crate::platform::filesystem::read_private_file_anchored(&path) {
            Ok(Some(encoded)) => encoded,
            Ok(None) => return Ok(None),
            Err(error) => return Err(format!("Failed to read browser restore manifest: {error}")),
        };
        parse_restore_workspace(&encoded).map(Some)
    }

    /// On restore failure, rewrite the manifest read before the call verbatim so
    /// about:blank navigation events during construction cannot overwrite the
    /// last usable snapshot.
    pub fn write_restore_workspace(
        session_id: &str,
        restore: &NativeWorkspaceRestore,
    ) -> Result<(), String> {
        write_restore_workspace_file(
            &paths::browser_workspace_restore_json(&paths::browser_session_token(session_id)),
            restore,
        )
    }

    /// Create fresh native WebViews from a restore manifest. Any backend that
    /// advertises Agent automation remains behind a private marker until the
    /// caller binds a new process-local target and completes initial navigation
    /// for every tab; publish all only after the final tab completes. Only
    /// display-only platforms may load URLs directly.
    pub fn prepare_restored_surface(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        session_token: &str,
        automation_port: Option<u16>,
        data_directory: &Path,
        restore: &NativeWorkspaceRestore,
    ) -> Result<Vec<String>, String> {
        if session_id.is_empty() || !is_valid_token(session_token) {
            return Err("Browser session identity is invalid".to_string());
        }
        if restore.urls.is_empty()
            || restore.urls.len() > MAX_WORKSPACE_TABS
            || restore.active_index >= restore.urls.len()
            || restore
                .urls
                .iter()
                .any(|url| !is_trackable_surface_url(url))
        {
            return Err("Browser restore manifest is invalid".to_string());
        }
        if automation_port.is_some() && !P::ACTIVATION_READY {
            return Err("No browser automation backend is available on this platform".to_string());
        }
        self.reap_quarantined_for_session(app, session_id)?;
        if let Some(workspace) = self.workspaces.get(session_id) {
            return Ok(workspace.tabs.iter().map(|tab| tab.token.clone()).collect());
        }
        if self
            .platform
            .requires_reset(automation_port, data_directory)
            || self
                .data_directory
                .as_deref()
                .is_some_and(|current| current != data_directory)
        {
            self.close(Some(app))?;
        }

        std::fs::create_dir_all(data_directory)
            .map_err(|error| format!("Failed to create browser data directory: {error}"))?;
        crate::platform::os::make_private_dir(data_directory);
        self.platform.prepare(automation_port, data_directory)?;
        self.data_directory = Some(data_directory.to_path_buf());

        // Restore manifests do not persist control ownership. A restart is
        // neither user takeover nor Agent authorization. Restored pages remain
        // neutral until real UI/trusted input or an Agent lease claims ownership.
        let control = Arc::new(WorkspaceControl::new(1, NativeControlOwner::Unclaimed));
        let mut entries: Vec<SurfaceEntry> = Vec::with_capacity(restore.urls.len());
        let mut tab_tokens = Vec::with_capacity(restore.urls.len());
        let requires_automation_binding =
            automation_port.is_some() || self.platform.capabilities().agent_automation;
        if requires_automation_binding {
            // Old target mappings belong only to the previous process. Remove
            // them before creating replacement WebViews, using the pinned
            // workspace-state root so a hostile root swap cannot redirect the
            // destructive operation.
            remove_workspace_state_file(session_token)?;
        }
        for url in &restore.urls {
            let tab_token = self.fresh_tab_token(&tab_tokens);
            let initial_url = if requires_automation_binding {
                format!("about:blank#pinvou-tab-{tab_token}")
            } else {
                marked_blank_url(url, &tab_token)
            };
            let entry = match build_webview(
                &self.platform,
                app,
                session_id,
                &tab_token,
                data_directory,
                &initial_url,
                Arc::clone(&control),
                false,
            ) {
                Ok(entry) => entry,
                Err(mut error) => {
                    if let Some(survivor) = error.survivor.take() {
                        entries.push(survivor);
                    }
                    let cleanup = self.quarantine_and_reconcile_entries(app, session_id, entries);
                    return match cleanup {
                        Ok(()) => Err(error.message),
                        Err(cleanup_error) => Err(format!(
                            "{}; surface reconciliation after restore construction failure is incomplete: {cleanup_error}",
                            error.message
                        )),
                    };
                }
            };
            // Automation-capable restores start on a private marker. Keep the
            // manifest's already-validated URL as the fallback until the new
            // process observes a valid top-level URL from the fresh WebView.
            entry.remember_url(url);
            entries.push(entry);
            tab_tokens.push(tab_token);
        }

        let mut entries = entries.into_iter();
        // restore.urls was validated non-empty at the top of this function and
        // entries was built one-to-one from it, so the first entry exists.
        #[allow(clippy::expect_used)]
        let first = entries
            .next()
            .expect("restore manifest contains at least one tab");
        let mut tabs = TabRegistry::from_entry(first);
        for entry in entries {
            tabs.insert(entry)?;
        }
        if !requires_automation_binding {
            for entry in tabs.iter() {
                entry.publish();
            }
        }
        let active_tab = tab_tokens[restore.active_index].clone();
        self.workspaces.insert(
            session_id.to_string(),
            Workspace {
                session_token: session_token.to_string(),
                tabs,
                active_tab: active_tab.clone(),
                bounds: None,
                visible: false,
                control,
                prepare_generation: None,
            },
        );
        if !requires_automation_binding {
            if let Err(error) = self.persist_workspace(session_id) {
                if let Some(mut workspace) = self.workspaces.remove(session_id) {
                    if let Err(close_error) = close_workspace(app, &mut workspace) {
                        if !workspace.tabs.is_empty() {
                            self.workspaces.insert(session_id.to_string(), workspace);
                        }
                        return Err(format!(
                            "{error}; failed to roll back restored workspace: {close_error}"
                        ));
                    }
                }
                return Err(error);
            }
            if let Some(state) = self.control_state(session_id) {
                emit_control_changed(app, session_id, &active_tab, state);
            }
        }
        Ok(tab_tokens)
    }

    fn fresh_tab_token(&self, pending: &[String]) -> String {
        loop {
            let candidate = format!("{:016x}", rand::random::<u64>());
            if pending.iter().any(|token| token == &candidate) {
                continue;
            }
            if self
                .workspaces
                .values()
                .any(|workspace| workspace.tabs.by_token(&candidate).is_some())
            {
                continue;
            }
            if self
                .staged_tabs
                .keys()
                .any(|(_, tab_token)| tab_token == &candidate)
            {
                continue;
            }
            if self
                .quarantined_tabs
                .keys()
                .any(|(_, tab_token)| tab_token == &candidate)
            {
                continue;
            }
            return candidate;
        }
    }

    /// Transfer native surfaces that could not be closed into a cleanup-only
    /// registry. Quarantined entries never masquerade as a usable workspace or
    /// as a legitimate in-flight create-tab candidate.
    fn quarantine_and_reconcile_entries(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        entries: Vec<SurfaceEntry>,
    ) -> Result<(), String> {
        self.quarantine_entries(session_id, entries);
        self.reap_quarantined_for_session(app, session_id)
    }

    fn quarantine_entries(&mut self, session_id: &str, entries: Vec<SurfaceEntry>) {
        for mut entry in entries {
            entry.unpublish();
            entry.created_by_request_id = None;
            self.quarantined_tabs
                .insert((session_id.to_string(), entry.token.clone()), entry);
        }
    }

    /// Remove a failed restore transaction from every business-visible lookup
    /// before retrying native close. The restore manifest remains authoritative;
    /// any close survivor is cleanup-only and cannot be mistaken for an
    /// `Existing` workspace by a later Prepare.
    fn quarantine_workspace_for_failed_restore(&mut self, session_id: &str) -> Result<(), String> {
        let Some(mut workspace) = self.workspaces.remove(session_id) else {
            return Ok(());
        };
        let session_token = workspace.session_token.clone();
        let tokens = workspace
            .tabs
            .iter()
            .map(|entry| entry.token.clone())
            .collect::<Vec<_>>();
        let entries = tokens
            .into_iter()
            .filter_map(|token| workspace.tabs.remove_token(&token).map(|(_, entry)| entry))
            .collect::<Vec<_>>();
        if self.active_session.as_deref() == Some(session_id) {
            self.active_session = None;
        }
        self.quarantine_entries(session_id, entries);
        // Business visibility is revoked before the fallible durable cleanup.
        // A failed delete therefore retains only cleanup-owned WebViews and can
        // be retried without exposing the provisional restore as a workspace.
        remove_workspace_state_file(&session_token)
    }

    fn reap_quarantined_for_session(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
    ) -> Result<(), String> {
        reconcile_quarantined_close(&mut self.quarantined_tabs, session_id, |entry| {
            if let Some(webview) = app.get_webview(&entry.label) {
                webview.close().map_err(|error| {
                    format!("Failed to close quarantined system WebView: {error}")
                })?;
            }
            Ok(())
        })
    }

    pub fn generate_tab_token(&self) -> String {
        self.fresh_tab_token(&[])
    }

    fn ensure_tab_capacity(&self, session_id: &str) -> Result<(), String> {
        let published = self
            .workspaces
            .get(session_id)
            .map(|workspace| workspace.tabs.len())
            .unwrap_or(0);
        let staged = self
            .staged_tabs
            .keys()
            .filter(|(owner_session, _)| owner_session == session_id)
            .count();
        let quarantined = self
            .quarantined_tabs
            .keys()
            .filter(|(owner_session, _)| owner_session == session_id)
            .count();
        if published.saturating_add(staged).saturating_add(quarantined) >= MAX_WORKSPACE_TABS {
            return Err(format!(
                "A task can open at most {MAX_WORKSPACE_TABS} browser tabs; close unused tabs first"
            ));
        }
        Ok(())
    }

    fn prepare_surface(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        session_token: &str,
        automation_port: Option<u16>,
        data_directory: &Path,
        initial_owner: NativeControlOwner,
    ) -> Result<bool, String> {
        if session_id.is_empty() || !is_valid_token(session_token) {
            return Err("Browser session identity is invalid".to_string());
        }
        self.reap_quarantined_for_session(app, session_id)?;
        if self
            .platform
            .requires_reset(automation_port, data_directory)
            || self
                .data_directory
                .as_deref()
                .is_some_and(|current| current != data_directory)
        {
            self.close(Some(app))?;
        }
        if let Some(workspace) = self.workspaces.get(session_id) {
            if workspace.session_token != session_token {
                return Err(
                    "Browser session token does not match the existing workspace".to_string(),
                );
            }
            if !workspace.tabs.is_empty() {
                return Ok(true);
            }
        }
        if self.workspaces.iter().any(|(owner_session, workspace)| {
            owner_session != session_id && workspace.tabs.by_token(session_token).is_some()
        }) {
            return Err("Browser tab token already belongs to another conversation".to_string());
        }

        std::fs::create_dir_all(data_directory)
            .map_err(|e| format!("Failed to create browser data directory: {e}"))?;
        crate::platform::os::make_private_dir(data_directory);
        self.platform.prepare(automation_port, data_directory)?;
        self.data_directory = Some(data_directory.to_path_buf());

        let control = Arc::new(WorkspaceControl::new(1, initial_owner));
        let entry = match build_webview(
            &self.platform,
            app,
            session_id,
            session_token,
            data_directory,
            &format!("about:blank#pinvou-session-{session_token}"),
            Arc::clone(&control),
            false,
        ) {
            Ok(entry) => entry,
            Err(mut error) => {
                let entries = error.survivor.take().into_iter().collect();
                let cleanup = self.quarantine_and_reconcile_entries(app, session_id, entries);
                return match cleanup {
                    Ok(()) => Err(error.message),
                    Err(cleanup_error) => Err(format!(
                        "{}; surface reconciliation after initialization failure is incomplete: {cleanup_error}",
                        error.message
                    )),
                };
            }
        };
        entry.publish();
        self.workspaces.insert(
            session_id.to_string(),
            Workspace {
                session_token: session_token.to_string(),
                active_tab: session_token.to_string(),
                tabs: TabRegistry::from_entry(entry),
                bounds: None,
                visible: false,
                control,
                prepare_generation: None,
            },
        );
        if let Err(error) = self.persist_workspace(session_id) {
            if let Some(mut workspace) = self.workspaces.remove(session_id) {
                if let Err(close_error) = close_workspace(app, &mut workspace) {
                    if !workspace.tabs.is_empty() {
                        self.workspaces.insert(session_id.to_string(), workspace);
                    }
                    return Err(format!(
                        "{error}; failed to roll back browser workspace: {close_error}"
                    ));
                }
            }
            return Err(error);
        }
        if let Err(error) = self.persist_restore_workspace(app, session_id) {
            if let Some(mut workspace) = self.workspaces.remove(session_id) {
                if let Err(close_error) = close_workspace(app, &mut workspace) {
                    if !workspace.tabs.is_empty() {
                        self.workspaces.insert(session_id.to_string(), workspace);
                    }
                    return Err(format!(
                        "{error}; failed to roll back browser workspace: {close_error}"
                    ));
                }
            }
            return match remove_workspace_state_file(session_token) {
                Ok(()) => Err(error),
                Err(cleanup_error) => Err(format!(
                    "{error}; failed to remove browser workspace state after restore persistence failure: {cleanup_error}"
                )),
            };
        }
        if let Some(state) = self.control_state(session_id) {
            emit_control_changed(app, session_id, session_token, state);
        }
        Ok(true)
    }

    pub fn create_tab(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        tab_token: &str,
        url: &str,
        background: bool,
    ) -> Result<Option<String>, String> {
        self.create_user_tab(app, session_id, tab_token, url, background)
    }

    /// Wrapper tab creation must carry the same host lease as the currently
    /// visible page. A WebView may be staged while hidden, but must CAS-commit
    /// under the control lock before workspace registration. If user takeover
    /// commits first, close the staged WebView immediately and never overwrite
    /// the User owner.
    pub fn create_tab_for_agent(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        tab_token: &str,
        url: &str,
        background: bool,
        authorization: &NativeTabLease,
        creation_id: &str,
    ) -> Result<Option<String>, String> {
        if !is_valid_token(tab_token) || creation_id.is_empty() {
            return Err("Browser tab or creation generation identity is invalid".to_string());
        }
        self.reap_quarantined_for_session(app, session_id)?;
        if self
            .workspaces
            .values()
            .any(|workspace| workspace.tabs.by_token(tab_token).is_some())
            || self
                .staged_tabs
                .keys()
                .any(|(_, staged_token)| staged_token == tab_token)
            || self
                .quarantined_tabs
                .keys()
                .any(|(_, quarantined_token)| quarantined_token == tab_token)
        {
            return Err(
                "Browser tab token is already reserved by another create request".to_string(),
            );
        }
        self.ensure_tab_capacity(session_id)?;
        let workspace = match self.workspaces.get(session_id) {
            Some(workspace) => workspace,
            None => return Ok(None),
        };
        validate_agent_mutation(workspace, session_id, authorization, None)?;
        if !workspace
            .control
            .assert_agent_lease(authorization.revision, &authorization.lease)
        {
            return Err(
                "Agent mutation lease expired; the user may have taken over the browser"
                    .to_string(),
            );
        }
        if !self.platform.is_initialized() {
            return Err("Browser system WebView is not ready".to_string());
        }
        let data_directory = self
            .data_directory
            .clone()
            .ok_or_else(|| "Browser data directory is not ready".to_string())?;
        let mut entry = match build_webview(
            &self.platform,
            app,
            session_id,
            tab_token,
            &data_directory,
            &marked_blank_url(url, tab_token),
            Arc::clone(&workspace.control),
            false,
        ) {
            Ok(entry) => entry,
            Err(mut error) => {
                if let Some(survivor) = error.survivor.take() {
                    self.quarantined_tabs
                        .insert((session_id.to_string(), tab_token.to_string()), survivor);
                }
                return Err(error.message);
            }
        };
        entry.created_by_request_id = Some(creation_id.to_string());
        self.staged_tabs
            .insert((session_id.to_string(), tab_token.to_string()), entry);
        let _ = background; // publishing/activation is decided at the final CAS.
        Ok(Some(tab_token.to_string()))
    }

    /// Called by BrowserManager after discovery: navigate the hidden page first,
    /// then write the authoritative mapping, publish the page, and optionally
    /// activate it according to background within the same host-lease CAS section.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_created_tab_for_agent<F>(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        tab_token: &str,
        automation_target: &str,
        requested_url: &str,
        background: bool,
        authorization: &NativeTabLease,
        creation_id: &str,
        retained_popup: Option<&RetainedAgentOperation>,
        mut caller_guard: F,
    ) -> Result<bool, String>
    where
        F: FnMut() -> Result<(), String>,
    {
        let result = self.commit_created_tab_for_agent_inner(
            app,
            session_id,
            tab_token,
            automation_target,
            requested_url,
            background,
            authorization,
            creation_id,
            retained_popup,
            &mut caller_guard,
        );
        match result {
            Ok(true) => Ok(true),
            Ok(false) => match self.rollback_staged_agent_creation(
                Some(app),
                session_id,
                tab_token,
                creation_id,
            ) {
                Ok(_) => Ok(false),
                Err(rollback_error) => Err(format!(
                    "New tab was not committed; exact rollback of hidden candidate failed: {rollback_error}"
                )),
            },
            Err(error) => match self.rollback_staged_agent_creation(
                Some(app),
                session_id,
                tab_token,
                creation_id,
            ) {
                Ok(_) => Err(error),
                Err(rollback_error) => Err(format!(
                    "{error}; exact rollback of hidden candidate failed: {rollback_error}"
                )),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_created_tab_for_agent_inner<F>(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        tab_token: &str,
        automation_target: &str,
        requested_url: &str,
        background: bool,
        authorization: &NativeTabLease,
        creation_id: &str,
        retained_popup: Option<&RetainedAgentOperation>,
        caller_guard: &mut F,
    ) -> Result<bool, String>
    where
        F: FnMut() -> Result<(), String>,
    {
        if automation_target.is_empty() || automation_target.len() > 512 {
            return Err("Browser automation targetId is invalid".to_string());
        }
        if self
            .workspaces
            .values()
            .any(|workspace| workspace.tabs.token_for_target(automation_target).is_some())
        {
            return Err("Browser automation target already belongs to another tab".to_string());
        }
        let key = (session_id.to_string(), tab_token.to_string());
        let mut entry = self
            .staged_tabs
            .get(&key)
            .cloned()
            .ok_or_else(|| "Pending browser tab does not exist".to_string())?;
        if entry.created_by_request_id.as_deref() != Some(creation_id) {
            return Err("Creation generation does not match the pending tab".to_string());
        }
        let workspace = self
            .workspaces
            .get_mut(session_id)
            .ok_or_else(|| "Browser workspace does not exist".to_string())?;
        validate_agent_mutation(workspace, session_id, authorization, None)?;

        let webview = app
            .get_webview(&entry.label)
            .ok_or_else(|| "Pending browser tab surface does not exist".to_string())?;
        if !is_trackable_surface_url(requested_url) {
            return Err("browser/url-not-allowed".to_string());
        }
        let requested_url = requested_url
            .parse::<tauri::Url>()
            .map_err(|error| format!("Initial browser navigation URL is invalid: {error}"))?;
        let control = Arc::clone(&workspace.control);
        caller_guard()?;
        if retained_popup
            .is_some_and(|retained| !control.authorize_retained_agent_operation(retained))
        {
            return Err("Popup Agent operation holder expired".to_string());
        }
        // With entry.published=false, initial navigation and its synchronous
        // callbacks cannot publish UI events or change control ownership.
        // URL admission above excludes the control-taking takeover scheme. The
        // short navigation guard ends inside begin_external_navigation before
        // the allowed-target callback can run; keep control through the native
        // enqueue so no authorize-then-navigate gap remains.
        if control
            .dispatch_if_agent_authorized(authorization, || {
                entry.begin_external_navigation(true);
                webview.navigate(requested_url).map_err(|error| {
                    format!(
                        "{ACTION_COMMIT_UNKNOWN_TAB_NAVIGATION}: hidden browser tab initial-navigation response is uncertain: {error}"
                    )
                })
            })?
            .is_none()
        {
            return Err("Agent mutation lease expired; the user may have taken over the browser".to_string());
        }
        caller_guard().map_err(|error| {
            format!(
                "{ACTION_COMMIT_UNKNOWN_TAB_NAVIGATION}: hidden-tab initial navigation was dispatched but the caller epoch expired: {error}"
            )
        })?;
        if retained_popup
            .is_some_and(|retained| !control.authorize_retained_agent_operation(retained))
        {
            return Err(format!(
                "{ACTION_COMMIT_UNKNOWN_TAB_NAVIGATION}: hidden-tab initial navigation was dispatched but the popup holder expired"
            ));
        }
        entry.automation_target = Some(automation_target.to_string());
        entry.created_at_revision = Some(authorization.revision.saturating_add(1));

        let previous_active = workspace.active_tab.clone();
        let staged_publication = Arc::clone(&entry.published);
        let committed = control.commit_agent_mutation(authorization, || {
            workspace.tabs.insert(entry.clone())?;
            if !background {
                workspace.active_tab = tab_token.to_string();
            }
            let committed_revision = authorization.revision.saturating_add(1);
            if let Err(error) = persist_workspace_snapshot(workspace, committed_revision) {
                let _ = workspace.tabs.remove_token(tab_token);
                workspace.active_tab = previous_active.clone();
                return Err(error);
            }
            if !background && workspace.visible {
                if let Err(error) = show_active_workspace(app, workspace) {
                    let _ = workspace.tabs.remove_token(tab_token);
                    workspace.active_tab = previous_active.clone();
                    if let Err(restore_error) =
                        persist_workspace_snapshot(workspace, authorization.revision)
                    {
                        eprintln!("[browser] Failed to roll back new-tab mapping: {restore_error}");
                    }
                    if let Err(restore_error) = show_active_workspace(app, workspace) {
                        eprintln!("[browser] Failed to roll back new-tab display: {restore_error}");
                    }
                    return Err(error);
                }
            }
            staged_publication.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        });
        if !matches!(committed, Ok(Some(_))) {
            return match committed {
                Ok(None) => Err(format!(
                    "{ACTION_COMMIT_UNKNOWN_TAB_NAVIGATION}: initial navigation was dispatched but the Agent mutation lease expired before publication"
                )),
                Err(error) => Err(format!(
                    "{ACTION_COMMIT_UNKNOWN_TAB_NAVIGATION}: initial navigation was dispatched but host publication failed: {error}"
                )),
                Ok(Some(_)) => unreachable!(),
            };
        }
        self.staged_tabs.remove(&key);
        let snapshot = control.snapshot();
        emit_control_changed(app, session_id, &workspace.active_tab, snapshot);
        if let Err(error) = self.persist_restore_workspace(app, session_id) {
            eprintln!("[browser] New tab committed but restore-manifest refresh failed: {error}");
        }
        Ok(true)
    }

    fn create_user_tab(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        tab_token: &str,
        url: &str,
        background: bool,
    ) -> Result<Option<String>, String> {
        if !is_valid_token(tab_token) {
            return Err("Browser tab identity is invalid".to_string());
        }
        if !is_trackable_surface_url(url) {
            return Err("browser/url-not-allowed".to_string());
        }
        self.reap_quarantined_for_session(app, session_id)?;
        if self.workspaces.iter().any(|(owner_session, workspace)| {
            owner_session != session_id && workspace.tabs.by_token(tab_token).is_some()
        }) || self
            .staged_tabs
            .keys()
            .any(|(_, staged_token)| staged_token == tab_token)
            || self
                .quarantined_tabs
                .keys()
                .any(|(_, quarantined_token)| quarantined_token == tab_token)
        {
            return Err("Browser tab token already belongs to another conversation".to_string());
        }
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(None);
        };
        if workspace.tabs.by_token(tab_token).is_some() {
            return Ok(Some(tab_token.to_string()));
        }
        self.ensure_tab_capacity(session_id)?;
        // The workspace lookup above returned Some under the same &mut self,
        // and nothing since could have removed it.
        #[allow(clippy::expect_used)]
        let workspace = self
            .workspaces
            .get(session_id)
            .expect("workspace existence was checked before capacity");
        if !self.platform.is_initialized() {
            return Err("Browser system WebView is not ready".to_string());
        }
        let data_directory = self
            .data_directory
            .clone()
            .ok_or_else(|| "Browser data directory is not ready".to_string())?;
        let control = Arc::clone(&workspace.control);
        let initial_url = marked_blank_url(url, tab_token);
        let mut entry = match build_webview(
            &self.platform,
            app,
            session_id,
            tab_token,
            &data_directory,
            &initial_url,
            Arc::clone(&control),
            false,
        ) {
            Ok(entry) => entry,
            Err(mut error) => {
                if let Some(survivor) = error.survivor.take() {
                    self.quarantined_tabs
                        .insert((session_id.to_string(), tab_token.to_string()), survivor);
                }
                return Err(error.message);
            }
        };
        entry.created_by_request_id = None;
        if has_internal_marker_for_token(&initial_url, tab_token) {
            let key = (session_id.to_string(), tab_token.to_string());
            self.staged_tabs.insert(key.clone(), entry);
            self.staged_user_tabs.insert(key, background);
            return Ok(Some(tab_token.to_string()));
        }
        let created_label = entry.label.clone();
        let created_publication = Arc::clone(&entry.published);

        // The workspace lookup above returned Some under the same &mut self,
        // and nothing since could have removed it.
        #[allow(clippy::expect_used)]
        let workspace = self
            .workspaces
            .get_mut(session_id)
            .expect("workspace was checked above");
        if let Err(error) = workspace.tabs.insert(entry) {
            if let Some(webview) = app.get_webview(&created_label) {
                let _ = webview.close();
            }
            return Err(error);
        }
        let previous_active = workspace.active_tab.clone();
        if !background {
            workspace.active_tab = tab_token.to_string();
        }
        control.bump(Some(NativeControlOwner::User));
        if !background && workspace.visible {
            if let Err(error) = show_active_workspace(app, workspace) {
                let _ = workspace.tabs.remove_token(tab_token);
                workspace.active_tab = previous_active;
                let rollback = control.bump(Some(NativeControlOwner::User));
                if let Some(webview) = app.get_webview(&created_label) {
                    let _ = webview.close();
                }
                if let Err(restore_error) = persist_workspace_snapshot(workspace, rollback.revision)
                {
                    eprintln!(
                        "[browser] Failed to roll back user-created tab mapping: {restore_error}"
                    );
                }
                if let Err(restore_error) = show_active_workspace(app, workspace) {
                    eprintln!(
                        "[browser] Failed to roll back user-created tab display: {restore_error}"
                    );
                }
                emit_control_changed(app, session_id, &workspace.active_tab, rollback);
                return Err(error);
            }
        }
        created_publication.store(true, std::sync::atomic::Ordering::SeqCst);
        // The user may take over again after CAS. Read current state before
        // emitting and never overwrite updated UI with an old Agent snapshot.
        let snapshot = control.snapshot();
        emit_control_changed(app, session_id, &workspace.active_tab, snapshot);
        if let Err(error) = self.persist_workspace(session_id) {
            eprintln!("[browser] Failed to persist mapping after user-created tab: {error}");
        }
        if let Err(error) = self.persist_restore_workspace(app, session_id) {
            eprintln!(
                "[browser] Failed to refresh restore manifest after user-created tab: {error}"
            );
        }
        Ok(Some(tab_token.to_string()))
    }

    /// Bind an underlying target discovered by the wrapper to a host tab. This
    /// bijection is globally unique across all conversations.
    pub fn bind_target(
        &mut self,
        session_id: &str,
        tab_token: &str,
        automation_target: &str,
    ) -> Result<bool, String> {
        if automation_target.is_empty() || automation_target.len() > 512 {
            return Err("Browser automation targetId is invalid".to_string());
        }
        if self.workspaces.iter().any(|(owner_session, workspace)| {
            workspace
                .tabs
                .token_for_target(automation_target)
                .is_some_and(|bound_tab| owner_session != session_id || bound_tab != tab_token)
        }) || self
            .staged_tabs
            .iter()
            .any(|((owner_session, owner_tab), entry)| {
                entry.automation_target.as_deref() == Some(automation_target)
                    && (owner_session != session_id || owner_tab != tab_token)
            })
        {
            return Err("Browser automation target already belongs to another tab".to_string());
        }
        let staged_key = (session_id.to_string(), tab_token.to_string());
        if self.staged_user_tabs.contains_key(&staged_key) {
            let entry = self
                .staged_tabs
                .get_mut(&staged_key)
                .ok_or_else(|| "User tab awaiting binding has incomplete state".to_string())?;
            if entry.automation_target.as_deref() == Some(automation_target) {
                return Ok(true);
            }
            entry.automation_target = Some(automation_target.to_string());
            return Ok(true);
        }
        let Some(workspace) = self.workspaces.get_mut(session_id) else {
            return Ok(false);
        };
        if workspace.tabs.target_for_token(tab_token) == Some(automation_target) {
            return Ok(true);
        }
        let restoring_unpublished_workspace =
            workspace.tabs.iter().any(|entry| !entry.is_published());
        workspace.tabs.bind_target(tab_token, automation_target)?;
        if restoring_unpublished_workspace {
            return Ok(true);
        }
        workspace.control.bump(None);
        self.persist_workspace(session_id)?;
        Ok(true)
    }

    /// BrowserManager only needs initial CDP marker discovery for these tabs and
    /// never reads ownership from the page main world.
    pub fn unbound_tabs(&self, session_id: &str) -> Vec<String> {
        self.workspaces
            .get(session_id)
            .map(|workspace| {
                workspace
                    .tabs
                    .iter()
                    .filter(|entry| entry.automation_target.is_none())
                    .map(|entry| entry.token.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn control_state(&self, session_id: &str) -> Option<ControlSnapshot> {
        self.workspaces
            .get(session_id)
            .map(|workspace| workspace.control.snapshot())
    }

    pub fn show(
        &mut self,
        window: &tauri::Window,
        session_id: &str,
        bounds: NativeSurfaceBounds,
    ) -> Result<bool, String> {
        if !self
            .workspaces
            .get(session_id)
            .and_then(active_entry)
            .is_some_and(SurfaceEntry::is_published)
        {
            return Ok(false);
        }
        let previous_active = self.active_session.clone();
        let previous_bounds = self
            .workspaces
            .get(session_id)
            .and_then(|workspace| workspace.bounds);
        hide_all(window.app_handle(), &self.workspaces).map_err(|error| {
            format!("Failed to hide old surface before switching native browser workspace: {error}")
        })?;
        set_exclusive_workspace_visibility(&mut self.workspaces, session_id);
        {
            // The is_some_and check above confirmed the workspace exists, and
            // this method holds &mut self throughout.
            #[allow(clippy::expect_used)]
            let workspace = self
                .workspaces
                .get_mut(session_id)
                .expect("workspace was checked above");
            workspace.bounds = Some(bounds);
        }
        // Same checked-existence invariant as the block directly above.
        #[allow(clippy::expect_used)]
        let workspace = self
            .workspaces
            .get(session_id)
            .expect("workspace was checked above");
        let show_result = show_active_workspace(window.app_handle(), workspace);
        if let Err(error) = show_result {
            if let Some(workspace) = self.workspaces.get_mut(session_id) {
                workspace.visible = false;
                workspace.bounds = previous_bounds;
            }
            self.active_session = None;
            if let Some(previous_session) = previous_active.as_deref() {
                if let Some(previous) = self.workspaces.get_mut(previous_session) {
                    previous.visible = true;
                }
                if let Some(previous) = self.workspaces.get(previous_session) {
                    match show_active_workspace(window.app_handle(), previous) {
                        Ok(()) => self.active_session = Some(previous_session.to_string()),
                        Err(restore_error) => {
                            eprintln!(
                                "[browser] Failed to roll back native workspace display: {restore_error}"
                            );
                            if let Some(previous) = self.workspaces.get_mut(previous_session) {
                                previous.visible = false;
                            }
                        }
                    }
                }
            }
            return Err(error);
        }
        self.active_session = Some(session_id.to_string());
        Ok(true)
    }

    pub fn hide(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: Option<&str>,
    ) -> Result<(), String> {
        if let Some(app) = app {
            match session_id {
                Some(session_id) => {
                    if let Some(workspace) = self.workspaces.get(session_id) {
                        hide_workspace(app, workspace)?;
                    }
                }
                None => hide_all(app, &self.workspaces)?,
            }
        }
        match session_id {
            Some(session_id) => {
                if let Some(workspace) = self.workspaces.get_mut(session_id) {
                    workspace.visible = false;
                }
                if self.active_session.as_deref() == Some(session_id) {
                    self.active_session = None;
                }
            }
            None => {
                for workspace in self.workspaces.values_mut() {
                    workspace.visible = false;
                }
                self.active_session = None;
            }
        }
        Ok(())
    }

    pub fn activate_tab(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
    ) -> Result<bool, String> {
        Ok(self
            .activate_tab_as(app, session_id, tab_token, NativeControlOwner::User, false)?
            .is_some())
    }

    pub fn rollback_agent_activation(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        activated_tab: &str,
        previous_tab: &str,
        expected_revision: u64,
        previous_owner: NativeControlOwner,
    ) -> Result<bool, String> {
        let session_owns_visible_surface = self.active_session.as_deref() == Some(session_id);
        let Some(workspace) = self.workspaces.get_mut(session_id) else {
            return Ok(false);
        };
        if workspace.active_tab != activated_tab || workspace.tabs.by_token(previous_tab).is_none()
        {
            return Ok(false);
        }
        let visible_app = if workspace_may_present_native_surface(
            session_owns_visible_surface,
            workspace.visible,
        ) {
            Some(app.ok_or_else(|| "App handle is not ready".to_string())?)
        } else {
            None
        };
        let control = Arc::clone(&workspace.control);
        let committed = control.rollback_agent_activation(
            expected_revision,
            previous_owner,
            |rollback_revision| {
                workspace.active_tab = previous_tab.to_string();
                if let Err(error) = persist_workspace_snapshot(workspace, rollback_revision) {
                    workspace.active_tab = activated_tab.to_string();
                    return Err(error);
                }
                if let Some(app) = visible_app {
                    if let Err(error) = show_active_workspace(app, workspace) {
                        workspace.active_tab = activated_tab.to_string();
                        if let Err(restore_error) =
                            persist_workspace_snapshot(workspace, expected_revision)
                        {
                            eprintln!("[browser] Failed to roll back cancelled tab activation: {restore_error}");
                        }
                        let _ = show_active_workspace(app, workspace);
                        return Err(error);
                    }
                }
                Ok(())
            },
        )?;
        let Some((snapshot, ())) = committed else {
            return Ok(false);
        };
        if let Some(app) = app {
            emit_control_changed(app, session_id, previous_tab, snapshot);
        }
        Ok(true)
    }

    /// Return a short-lived lease when the Agent activates a tab. Every tool
    /// must call [`Self::assert_lease`] before execution.
    pub fn activate_tab_with_lease(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
    ) -> Result<Option<NativeTabLease>, String> {
        self.activate_tab_with_lease_authorized(app, session_id, tab_token, false)
    }

    fn activate_tab_with_lease_authorized(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
        explicit_user_handback: bool,
    ) -> Result<Option<NativeTabLease>, String> {
        let Some((snapshot, target_id, lease)) = self.activate_tab_as(
            app,
            session_id,
            tab_token,
            NativeControlOwner::Agent,
            explicit_user_handback,
        )?
        else {
            return Ok(None);
        };
        Ok(Some(NativeTabLease {
            session_id: session_id.to_string(),
            tab_token: tab_token.to_string(),
            target_id,
            revision: snapshot.revision,
            owner: NativeControlOwner::Agent,
            lease,
        }))
    }

    /// Immediately return the current tab to the Agent from the UI. Automatic
    /// idle handback does not preissue a lease. This new lease exists only for
    /// the explicit shortcut and replaces and revokes the old lease.
    pub fn hand_back_to_agent(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
    ) -> Result<Option<NativeTabLease>, String> {
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(None);
        };
        let tab_token = workspace.active_tab.clone();
        self.activate_tab_with_lease_authorized(app, session_id, &tab_token, true)
    }

    fn activate_tab_as(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
        owner: NativeControlOwner,
        explicit_user_handback: bool,
    ) -> Result<Option<(ControlSnapshot, String, String)>, String> {
        let session_owns_visible_surface = self.active_session.as_deref() == Some(session_id);
        let Some(workspace) = self.workspaces.get_mut(session_id) else {
            return Ok(None);
        };
        if workspace.tabs.by_token(tab_token).is_none() {
            return Err("Tab does not exist or does not belong to this conversation".to_string());
        }
        let target_id = workspace
            .tabs
            .target_for_token(tab_token)
            .map(ToOwned::to_owned)
            .unwrap_or_default();
        if owner == NativeControlOwner::Agent && target_id.is_empty() {
            return Err(
                "Browser tab is not bound to an authoritative host automation target".to_string(),
            );
        }
        // workspace.visible expresses only this workspace's intent while the
        // physical surface is app-global. Only the foreground workspace holding
        // active_session may show on activate. Background Agent activation only
        // updates that workspace's tab/lease and cannot preempt another task the
        // user is viewing.
        let visible_app = if workspace_may_present_native_surface(
            session_owns_visible_surface,
            workspace.visible,
        ) {
            Some(app.ok_or_else(|| "App handle is not ready".to_string())?)
        } else {
            None
        };
        let control = Arc::clone(&workspace.control);
        let (snapshot, lease) = if owner == NativeControlOwner::Agent {
            let previous_active = workspace.active_tab.clone();
            let previous_revision = control.snapshot().revision;
            let issued =
                control.issue_agent_lease_with(explicit_user_handback, |committed_revision| {
                    workspace.active_tab = tab_token.to_string();
                    if let Err(error) = persist_workspace_snapshot(workspace, committed_revision) {
                        workspace.active_tab = previous_active.clone();
                        return Err(error);
                    }
                    if let Some(app) = visible_app {
                        if let Err(error) = show_active_workspace(app, workspace) {
                            workspace.active_tab = previous_active.clone();
                            if let Err(restore_error) =
                                persist_workspace_snapshot(workspace, previous_revision)
                            {
                                eprintln!("[browser] Failed to roll back tab activation mapping: {restore_error}");
                            }
                            let _ = show_active_workspace(app, workspace);
                            return Err(error);
                        }
                    }
                    Ok(())
                })?;
            let Some((snapshot, lease, ())) = issued else {
                return Err(
                    "The user just operated the browser. Agent control resumes after 3 seconds of inactivity or immediately after Return to Agent."
                        .to_string(),
                );
            };
            (snapshot, lease)
        } else {
            let previous_active = workspace.active_tab.clone();
            workspace.active_tab = tab_token.to_string();
            let snapshot = control.bump(Some(owner));
            if let Some(app) = visible_app {
                if let Err(error) = show_active_workspace(app, workspace) {
                    workspace.active_tab = previous_active;
                    // Never rewind control revision to its pre-failure value.
                    // Publish rollback as a new User mutation so every old lease
                    // or event that observed the failed activation becomes stale.
                    let rollback = control.bump(Some(NativeControlOwner::User));
                    if let Err(restore_error) =
                        persist_workspace_snapshot(workspace, rollback.revision)
                    {
                        eprintln!(
                            "[browser] Failed to roll back user tab activation mapping: {restore_error}"
                        );
                    }
                    if let Err(restore_error) = show_active_workspace(app, workspace) {
                        eprintln!(
                            "[browser] Failed to roll back user tab activation display: {restore_error}"
                        );
                    }
                    emit_control_changed(app, session_id, &workspace.active_tab, rollback);
                    return Err(error);
                }
            }
            (snapshot, String::new())
        };
        if let Some(app) = app {
            // The user can take over immediately after the Agent critical section
            // releases. Emit the current snapshot so a late Agent event cannot
            // overwrite the updated User owner in the UI.
            emit_control_changed(app, session_id, tab_token, control.snapshot());
        }
        if owner != NativeControlOwner::Agent {
            if let Err(error) = self.persist_workspace(session_id) {
                eprintln!("[browser] Failed to persist mapping after user tab switch: {error}");
            }
        }
        if let Some(app) = app {
            if let Err(error) = self.persist_restore_workspace(app, session_id) {
                eprintln!(
                    "[browser] Failed to refresh restore manifest after tab activation: {error}"
                );
            }
        }
        Ok(Some((snapshot, target_id, lease)))
    }

    /// Validate session, tab, target, revision, owner, and the opaque host
    /// capability token together.
    pub fn assert_lease(&self, lease: &NativeTabLease) -> Result<bool, String> {
        let Some(workspace) = self.workspaces.get(&lease.session_id) else {
            return Ok(false);
        };
        if workspace.active_tab != lease.tab_token
            || workspace.tabs.target_for_token(&lease.tab_token) != Some(lease.target_id.as_str())
        {
            return Ok(false);
        }
        Ok(lease.owner == NativeControlOwner::Agent
            && workspace
                .control
                .assert_agent_lease(lease.revision, &lease.lease))
    }

    /// Atomically validate the lease before every tool dispatch. Input tools
    /// additionally open a short trusted-event suppression window.
    pub fn begin_agent_operation(
        &self,
        lease: &NativeTabLease,
        emits_trusted_input: bool,
        observational_only: bool,
        caller_pid: u32,
        wrapper_instance_nonce: &str,
    ) -> Result<bool, String> {
        if !self.assert_lease(lease)? {
            return Ok(false);
        }
        if !observational_only
            && self
                .workspaces
                .get(&lease.session_id)
                .and_then(|workspace| workspace.tabs.by_token(&lease.tab_token))
                .is_some_and(SurfaceEntry::navigation_admission_busy)
        {
            // Automatic hand-back after user idleness must not let an Agent
            // mutation navigate/rebind the WebView while an accepted native
            // navigation generation (including same-document/history) still
            // owns anonymous callbacks. Explicit read-only observations remain
            // available so the Agent can verify whether a load completed or
            // failed.
            return Ok(false);
        }
        let caller_epoch = AgentCallerEpoch::new(caller_pid, wrapper_instance_nonce.to_string())?;
        Ok(self
            .workspaces
            .get(&lease.session_id)
            .is_some_and(|workspace| {
                workspace.control.begin_agent_operation_for_caller(
                    lease,
                    emits_trusted_input,
                    caller_epoch,
                )
            }))
    }

    /// Renew only the current begun trusted-input dispatch. `assert_lease` first
    /// validates workspace session/tab/target/revision/opaque lease;
    /// WorkspaceControl then requires active_agent_operation to equal the whole
    /// lease under the same lock. A late heartbeat therefore cannot renew a
    /// finished operation, a new operation, or authorization predating takeover.
    pub fn refresh_agent_input(&self, lease: &NativeTabLease) -> Result<bool, String> {
        if !self.assert_lease(lease)? {
            return Ok(false);
        }
        Ok(self
            .workspaces
            .get(&lease.session_id)
            .is_some_and(|workspace| workspace.control.refresh_agent_input_window(lease)))
    }

    /// Renew only the bounded liveness of an exact begun operation. This is
    /// used by long non-input upstream calls and intentionally does not open
    /// the trusted-input provenance window.
    pub fn refresh_agent_operation(&self, lease: &NativeTabLease) -> Result<bool, String> {
        if !self.assert_lease(lease)? {
            return Ok(false);
        }
        Ok(self
            .workspaces
            .get(&lease.session_id)
            .is_some_and(|workspace| workspace.control.refresh_agent_operation(lease)))
    }

    /// End the active operation immediately after dispatch. A dispatched WebKit
    /// event may not invoke the takeover delegate until the next run-loop turn,
    /// so retain at most 100ms of callback grace. Explicit UI takeover still
    /// bumps revision and clears the window immediately.
    pub fn end_agent_operation(&self, lease: &NativeTabLease) {
        if let Some(workspace) = self.workspaces.get(&lease.session_id) {
            workspace.control.end_agent_operation(lease);
        }
    }

    /// Release the exact holder retained by the WebView popup callback. The
    /// holder carries the original caller epoch, so an async cleanup from an
    /// older wrapper incarnation cannot consume current operation state.
    pub fn release_popup_agent_operation(&self, retained: &RetainedAgentOperation) {
        if let Some(workspace) = self.workspaces.get(&retained.authorization().session_id) {
            workspace.control.release_retained_agent_operation(retained);
        }
    }

    /// Validate that this exact retained popup holder is still registered;
    /// checking only its shared lease would let a released popup borrow a
    /// sibling holder from the same upstream operation.
    pub fn authorize_popup_agent_operation(&self, retained: &RetainedAgentOperation) -> bool {
        self.workspaces
            .get(&retained.authorization().session_id)
            .is_some_and(|workspace| {
                workspace
                    .control
                    .authorize_retained_agent_operation(retained)
            })
    }

    /// Revoke a claimed BrowserCore request as soon as a durable cancellation
    /// tombstone wins. The caller still polls any already-dispatched platform
    /// future to real settlement; this method blocks later sub-dispatches and
    /// rolls back only tabs carrying this request's exact creation id.
    pub fn cancel_in_flight_core_request(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        request_id: &str,
    ) -> Result<(), String> {
        if let Some(workspace) = self.workspaces.get(session_id) {
            workspace
                .control
                .cancel_agent_operation_for_session(session_id);
        }

        let mut created_tokens = self
            .staged_tabs
            .iter()
            .filter_map(|((owner_session, token), entry)| {
                (owner_session == session_id
                    && entry.created_by_request_id.as_deref() == Some(request_id))
                .then(|| token.clone())
            })
            .collect::<Vec<_>>();
        if let Some(workspace) = self.workspaces.get(session_id) {
            created_tokens.extend(workspace.tabs.iter().filter_map(|entry| {
                (entry.created_by_request_id.as_deref() == Some(request_id))
                    .then(|| entry.token.clone())
            }));
        }
        created_tokens.sort();
        created_tokens.dedup();

        let mut errors = Vec::new();
        for token in created_tokens {
            if let Err(error) = self.rollback_created_tab(app, session_id, &token, request_id) {
                errors.push(format!("{token}: {error}"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "browser/core-cancellation-compensation-failed: {}",
                errors.join("; ")
            ))
        }
    }

    pub fn close_tab(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
    ) -> Result<bool, String> {
        self.close_tab_as(
            app,
            session_id,
            tab_token,
            Some(NativeControlOwner::User),
            None,
        )
    }

    pub fn close_tab_for_agent(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
        authorization: &NativeTabLease,
    ) -> Result<bool, String> {
        self.close_tab_as(
            app,
            session_id,
            tab_token,
            Some(NativeControlOwner::Agent),
            Some(authorization),
        )
    }

    fn rollback_staged_agent_creation(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
        creation_id: &str,
    ) -> Result<bool, String> {
        let staged_key = (session_id.to_string(), tab_token.to_string());
        let Some(entry) = self.staged_tabs.get(&staged_key) else {
            return Ok(false);
        };
        if entry.created_by_request_id.as_deref() != Some(creation_id) {
            return Err(
                "Creation-compensation generation does not match the pending tab".to_string(),
            );
        }
        let label = entry.label.clone();
        if let Some(webview) = app.and_then(|app| app.get_webview(&label)) {
            webview
                .close()
                .map_err(|error| format!("Failed to close pending browser tab: {error}"))?;
        }
        super::unregister_browser_core_webview_binding(&label);
        self.staged_tabs.remove(&staged_key);
        self.staged_user_tabs.remove(&staged_key);
        Ok(true)
    }

    /// Used only by a request tombstone to roll back the tab created by that
    /// request. It does not change the existing control owner.
    pub fn rollback_created_tab(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
        creation_id: &str,
    ) -> Result<bool, String> {
        if self.rollback_staged_agent_creation(app, session_id, tab_token, creation_id)? {
            return Ok(true);
        }
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(false);
        };
        let Some(entry) = workspace.tabs.by_token(tab_token) else {
            return Ok(false);
        };
        if entry.created_by_request_id.as_deref() != Some(creation_id) {
            return Err(
                "Creation-compensation generation does not match the current tab".to_string(),
            );
        }
        let expected_revision = entry.created_at_revision.ok_or_else(|| {
            "Creation compensation is missing the committed generation".to_string()
        })?;
        let current = workspace.control.snapshot();
        if current.owner != NativeControlOwner::Agent || current.revision != expected_revision {
            // User takeover or any later mutation safely superseded this creation
            // generation. A late tombstone may only acknowledge and retain the
            // page; it must neither retry forever nor close a user page.
            return Ok(false);
        }
        if workspace.tabs.len() <= 1 {
            return Err("At least one browser tab must remain".to_string());
        }
        let entry = entry.clone();
        let control = Arc::clone(&workspace.control);
        // The workspace lookup above returned Some under the same &mut self,
        // and nothing since could have removed it.
        #[allow(clippy::expect_used)]
        let workspace = self
            .workspaces
            .get_mut(session_id)
            .expect("workspace was checked above");
        let committed = control.commit_agent_generation_rollback(expected_revision, || {
            if let Some(webview) = app.and_then(|app| app.get_webview(&entry.label)) {
                webview
                    .close()
                    .map_err(|error| format!("Failed to close browser tab: {error}"))?;
            }
            remove_tab_from_workspace(workspace, tab_token);
            if workspace.visible {
                let app = app.ok_or_else(|| "App handle is not ready".to_string())?;
                if let Err(error) = show_active_workspace(app, workspace) {
                    eprintln!("[browser] Failed to show fallback page after creation compensation: {error}");
                }
            }
            Ok(())
        })?;
        if committed.is_none() {
            return Ok(false);
        }
        super::unregister_browser_core_webview_binding(&entry.label);
        if let Err(error) = self.persist_workspace(session_id) {
            eprintln!("[browser] Failed to persist mapping after creation compensation: {error}");
        }
        if let Some(app) = app {
            if let Err(error) = self.persist_restore_workspace(app, session_id) {
                eprintln!(
                    "[browser] Failed to refresh restore manifest after creation compensation: {error}"
                );
            }
            // The workspace lookup above returned Some under the same &mut
            // self, and the compensation path never removes it.
            #[allow(clippy::expect_used)]
            let workspace = self
                .workspaces
                .get(session_id)
                .expect("workspace still exists after compensation commit");
            emit_control_changed(app, session_id, &workspace.active_tab, control.snapshot());
        }
        Ok(true)
    }

    /// Exact compensation for failed UI/user popup creation. It can affect only
    /// tabs without an Agent creation generation, preventing deletion of a new
    /// resource concurrently created by the Agent.
    pub fn rollback_user_created_tab(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
    ) -> Result<bool, String> {
        let staged_key = (session_id.to_string(), tab_token.to_string());
        if self.staged_user_tabs.contains_key(&staged_key) {
            let label = self
                .staged_tabs
                .get(&staged_key)
                .map(|entry| entry.label.clone())
                .ok_or_else(|| "User tab awaiting publication has incomplete state".to_string())?;
            if let Some(webview) = app.and_then(|app| app.get_webview(&label)) {
                webview.close().map_err(|error| {
                    format!("Failed to close user browser tab awaiting publication: {error}")
                })?;
            }
            super::unregister_browser_core_webview_binding(&label);
            self.staged_tabs.remove(&staged_key);
            self.staged_user_tabs.remove(&staged_key);
            return Ok(true);
        }
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(false);
        };
        let Some(entry) = workspace.tabs.by_token(tab_token) else {
            return Ok(false);
        };
        if entry.created_by_request_id.is_some() {
            return Err(
                "User creation compensation cannot delete an Agent-generation tab".to_string(),
            );
        }
        self.close_tab_as(app, session_id, tab_token, None, None)
    }

    fn close_tab_as(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
        owner: Option<NativeControlOwner>,
        authorization: Option<&NativeTabLease>,
    ) -> Result<bool, String> {
        let Some(workspace) = self.workspaces.get_mut(session_id) else {
            return Ok(false);
        };
        if workspace.tabs.len() <= 1 {
            return Err("At least one browser tab must remain".to_string());
        }
        let entry = workspace.tabs.by_token(tab_token).cloned().ok_or_else(|| {
            "Tab does not exist or does not belong to this conversation".to_string()
        })?;
        let app =
            app.ok_or_else(|| "App handle is not ready; cannot close browser tab".to_string())?;
        let agent_committed = owner == Some(NativeControlOwner::Agent);
        if agent_committed {
            let authorization = authorization
                .ok_or_else(|| "Agent tab close is missing a host mutation lease".to_string())?;
            validate_agent_mutation(workspace, session_id, authorization, Some(tab_token))?;
            let control = Arc::clone(&workspace.control);
            let committed = control.commit_agent_mutation(authorization, || {
                if let Some(webview) = app.get_webview(&entry.label) {
                    webview
                        .close()
                        .map_err(|error| {
                            format!(
                                "browser/action-commit-unknown-after-tab-close: native tab-close response is uncertain: {error}"
                            )
                        })?;
                }
                remove_tab_from_workspace(workspace, tab_token);
                if workspace.visible {
                    // Failure to show fallback after the page is closed cannot
                    // reverse close. Record it but commit registry/control so host
                    // state never claims the closed page still exists.
                    if let Err(error) = show_active_workspace(app, workspace) {
                        eprintln!("[browser] Failed to show fallback page after Agent tab close: {error}");
                    }
                }
                Ok(())
            })?;
            if committed.is_none() {
                return Err(
                    "Agent mutation lease expired; the user may have taken over the browser"
                        .to_string(),
                );
            }
        } else {
            if let Some(webview) = app.get_webview(&entry.label) {
                webview.close().map_err(|error| {
                    format!(
                        "browser/action-commit-unknown-after-tab-close: native tab-close response is uncertain: {error}"
                    )
                })?;
            }
            remove_tab_from_workspace(workspace, tab_token);
            workspace.control.bump(owner);
            if workspace.visible {
                // WebView close is physically committed. A failed fallback show
                // cannot disguise the successful close as failure, or caller
                // retry would only report that the tab does not exist.
                if let Err(error) = show_active_workspace(app, workspace) {
                    eprintln!(
                        "[browser] Failed to show fallback page after user tab close: {error}"
                    );
                }
            }
        }
        super::unregister_browser_core_webview_binding(&entry.label);
        let snapshot = workspace.control.snapshot();
        emit_control_changed(app, session_id, &workspace.active_tab, snapshot);
        // WebView close is an irreversible physical commit. A later snapshot
        // failure must not disguise success as failure, or retry reports that the
        // tab is absent. Preserve success and let later state refresh repair it.
        if let Err(error) = self.persist_workspace(session_id) {
            eprintln!("[browser] Failed to persist mapping after tab close: {error}");
        }
        if let Err(error) = self.persist_restore_workspace(app, session_id) {
            eprintln!("[browser] Failed to refresh restore manifest after tab close: {error}");
        }
        Ok(true)
    }

    pub fn close(&mut self, app: Option<&tauri::AppHandle>) -> Result<(), String> {
        self.close_impl(app, true)
    }

    /// App-process exit destroys only current WebView/target mappings and keeps
    /// the atomically written URL manifest for lazy per-conversation rebuild.
    /// Explicit Stop Browser still calls [`Self::close`] and deletes the manifest.
    pub fn close_preserving_restore(
        &mut self,
        app: Option<&tauri::AppHandle>,
    ) -> Result<(), String> {
        self.close_impl(app, false)
    }

    fn close_impl(
        &mut self,
        app: Option<&tauri::AppHandle>,
        delete_restore: bool,
    ) -> Result<(), String> {
        if app.is_none() && self.has_sessions() {
            return Err("App handle is not ready; cannot close browser".to_string());
        }
        let mut session_ids = self.workspaces.keys().cloned().collect::<Vec<_>>();
        for (session_id, _) in self.staged_tabs.keys() {
            if !session_ids.contains(session_id) {
                session_ids.push(session_id.clone());
            }
        }
        for (session_id, _) in self.quarantined_tabs.keys() {
            if !session_ids.contains(session_id) {
                session_ids.push(session_id.clone());
            }
        }
        session_ids.sort();
        let mut errors = Vec::new();
        for session_id in session_ids {
            if let Err(error) = self.close_session_impl(app, &session_id, delete_restore) {
                errors.push(format!("{session_id}: {error}"));
            }
        }
        if !self.has_sessions() {
            self.active_session = None;
            self.data_directory = None;
            self.platform.reset();
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    pub fn close_session(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
    ) -> Result<bool, String> {
        self.close_session_impl(app, session_id, true)
    }

    /// If some WebViews fail during restore, roll back only native resources
    /// created by this attempt and retain the original manifest for the next
    /// status query to retry.
    pub fn close_session_preserving_restore(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
    ) -> Result<bool, String> {
        self.close_session_impl(app, session_id, false)
    }

    /// Roll back an uncommitted restore transaction. Unlike an ordinary stop,
    /// this first removes the provisional workspace from business state, then
    /// reconciles its native children through the cleanup-only quarantine. A
    /// failed close therefore remains owned and retryable without making
    /// `has_session` return true.
    pub fn quarantine_failed_restore(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
    ) -> Result<bool, String> {
        let owns_workspace = self.workspaces.contains_key(session_id);
        let owns_quarantine = self
            .quarantined_tabs
            .keys()
            .any(|(owner_session, _)| owner_session == session_id);
        if !owns_workspace && !owns_quarantine {
            return Ok(self.has_sessions());
        }
        if !owns_workspace {
            // A prior attempt may already have moved every provisional WebView
            // into cleanup-only quarantine before the durable runtime delete
            // failed. Retry that exact deterministic state path before ACKing
            // the compensation or reaping the remaining native children.
            remove_workspace_state_file(&paths::browser_session_token(session_id))?;
        }
        let app = app.ok_or_else(|| {
            "App handle is not ready; cannot reconcile failed browser restore".to_string()
        })?;
        self.quarantine_workspace_for_failed_restore(session_id)?;
        self.reap_quarantined_for_session(app, session_id)?;
        Ok(self.has_sessions())
    }

    fn close_session_impl(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        delete_restore: bool,
    ) -> Result<bool, String> {
        let has_workspace = self.workspaces.contains_key(session_id);
        let has_staging = self
            .staged_tabs
            .keys()
            .any(|(owner_session, _)| owner_session == session_id);
        let has_quarantine = self
            .quarantined_tabs
            .keys()
            .any(|(owner_session, _)| owner_session == session_id);
        if !has_workspace && !has_staging && !has_quarantine {
            remove_workspace_state_file(&paths::browser_session_token(session_id))?;
            if delete_restore {
                remove_restore_file(&paths::browser_session_token(session_id))?;
            }
            return Ok(self.has_sessions());
        }

        let app = app.ok_or_else(|| {
            "App handle is not ready; cannot close conversation browser".to_string()
        })?;
        let mut errors = Vec::new();
        if let Some(workspace) = self.workspaces.get_mut(session_id) {
            if let Err(error) = close_workspace(app, workspace) {
                errors.push(error);
            }
        }
        if let Err(error) = self.close_staged_for_session(Some(app), session_id) {
            errors.push(error);
        }
        if let Err(error) = self.reap_quarantined_for_session(app, session_id) {
            errors.push(error);
        }

        let workspace_empty = self
            .workspaces
            .get(session_id)
            .is_some_and(|workspace| workspace.tabs.is_empty());
        if workspace_empty {
            // workspace_empty above proved the key exists; this method holds
            // &mut self, so the entry cannot disappear before removal.
            #[allow(clippy::expect_used)]
            let workspace = self
                .workspaces
                .remove(session_id)
                .expect("empty workspace remains protected by the same mutex during close");
            if let Err(error) = remove_workspace_state_file(&workspace.session_token) {
                errors.push(error);
            }
            if delete_restore {
                if let Err(error) = remove_restore_file(&workspace.session_token) {
                    errors.push(error);
                }
            }
        } else if let Some(workspace) = self.workspaces.get(session_id) {
            // WebView close commits irreversibly per page. On failure, the
            // registry retains only real survivors. Published survivors become
            // the new restore truth; unpublished restore staging retains the
            // pre-call manifest for a full retry after cleanup succeeds.
            if let Err(error) = self.persist_workspace(session_id) {
                errors.push(format!(
                    "Failed to save browser state after partial close: {error}"
                ));
            }
            if workspace.tabs.iter().any(SurfaceEntry::is_published) {
                if let Err(error) = self.persist_restore_workspace(app, session_id) {
                    errors.push(format!(
                        "Failed to save restore manifest after partial close: {error}"
                    ));
                }
            }
        } else if delete_restore {
            // A staging-only exceptional state has no recoverable user page.
            // Stop still deletes the old manifest while staging remains in memory
            // for the next idempotent stop to continue closing it.
            if let Err(error) = remove_restore_file(&paths::browser_session_token(session_id)) {
                errors.push(error);
            }
        }

        if self.active_session.as_deref() == Some(session_id)
            && !self.workspaces.contains_key(session_id)
        {
            self.active_session = None;
        }
        let has_remaining = self.has_sessions();
        if errors.is_empty() {
            Ok(has_remaining)
        } else {
            Err(errors.join("; "))
        }
    }

    fn close_staged_for_session(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
    ) -> Result<(), String> {
        let has_entries = self
            .staged_tabs
            .keys()
            .any(|(owner_session, _)| owner_session == session_id);
        if !has_entries {
            self.staged_user_tabs
                .retain(|(owner_session, _), _| owner_session != session_id);
            return Ok(());
        }
        let app = app.ok_or_else(|| {
            "App handle is not ready; cannot close pending browser tab".to_string()
        })?;
        reconcile_staged_close(
            &mut self.staged_tabs,
            &mut self.staged_user_tabs,
            session_id,
            |entry| {
                if let Some(webview) = app.get_webview(&entry.label) {
                    webview
                        .close()
                        .map_err(|error| format!("Failed to close system WebView: {error}"))?;
                }
                Ok(())
            },
        )
    }

    pub fn delete_restore_workspace(session_id: &str) -> Result<(), String> {
        remove_restore_file(&paths::browser_session_token(session_id))
    }

    pub fn session_state(
        &self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
    ) -> Option<(String, String)> {
        let workspace = self.workspaces.get(session_id)?;
        let entry = active_entry(workspace)?;
        if !entry.is_published() {
            return None;
        }
        let webview = app?.get_webview(&entry.label)?;
        let url = resolve_surface_url(entry, Some(&webview)).ok()?;
        Some((entry.token.clone(), url))
    }

    pub fn list_tabs(
        &self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
    ) -> Option<Vec<TabInfo>> {
        let workspace = self.workspaces.get(session_id)?;
        let app = app?;
        Some(
            workspace
                .tabs
                .iter()
                .filter(|entry| entry.is_published())
                .filter_map(|entry| {
                    let webview = app.get_webview(&entry.label)?;
                    let url = resolve_surface_url(entry, Some(&webview)).ok()?;
                    let title = entry.title_for_url(&url).unwrap_or_else(|| {
                        url.parse::<tauri::Url>()
                            .ok()
                            .and_then(|url| url.host_str().map(ToOwned::to_owned))
                            .unwrap_or_else(|| "about:blank".to_string())
                    });
                    Some(TabInfo {
                        target_id: entry.token.clone(),
                        page_id: Some(entry.page_id),
                        title,
                        url,
                    })
                })
                .collect(),
        )
    }

    pub fn tab_token_for_page_id(&self, session_id: &str, page_id: u64) -> Option<String> {
        self.workspaces
            .get(session_id)?
            .tabs
            .token_for_page_id(page_id)
            .map(ToOwned::to_owned)
    }

    pub fn navigate(
        &self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        url: &str,
        request_id: &str,
    ) -> Result<bool, String> {
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(false);
        };
        let entry =
            active_entry(workspace).ok_or_else(|| "Current tab does not exist".to_string())?;
        let app = app.ok_or_else(|| "App handle is not ready".to_string())?;
        let webview = app
            .get_webview(&entry.label)
            .ok_or_else(|| "Conversation browser surface does not exist".to_string())?;
        let target_url = marked_blank_url(url, &entry.token);
        let target_url: tauri::Url = target_url
            .parse()
            .map_err(|e| format!("Browser URL is invalid: {e}"))?;
        let intent_url =
            validated_surface_url(target_url.to_string(), &entry.token).ok_or_else(|| {
                "Browser navigation target cannot be used for event correlation".to_string()
            })?;
        let cross_document = !is_fragment_only_navigation(&entry.last_known_url(), &intent_url);
        let had_navigation_in_flight = entry.navigation_in_flight();
        self.mark_user_control(app, session_id, &entry.token)?;
        entry.begin_user_navigation(request_id, intent_url, cross_document)?;
        if had_navigation_in_flight {
            // Advance (or fail-closed reject) the generation before stopping
            // the old document. Otherwise a rejected same-URL retry would
            // accidentally terminate the only generation we can still track.
            let _ = webview.eval("window.stop()");
        }
        if let Err(error) = webview.navigate(target_url) {
            entry.fail_user_navigation(request_id);
            return Err(format!("Browser navigation failed: {error}"));
        }
        Ok(true)
    }

    /// Dispatch a Hosted BrowserCore navigation through the same generation
    /// seam used by the address bar. The caller already holds and revalidates
    /// the Agent lease; this method owns only navigation identity/lifecycle.
    pub fn navigate_tab_for_agent(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        tab_token: &str,
        url: &str,
        authorization: &NativeTabLease,
    ) -> Result<bool, String> {
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(false);
        };
        let entry = workspace.tabs.by_token(tab_token).ok_or_else(|| {
            "Tab does not exist or does not belong to this conversation".to_string()
        })?;
        if !is_trackable_surface_url(url) {
            return Err("browser/url-not-allowed".to_string());
        }
        let webview = app
            .get_webview(&entry.label)
            .ok_or_else(|| "Conversation browser surface does not exist".to_string())?;
        let target_url = marked_blank_url(url, tab_token);
        let target_url: tauri::Url = target_url
            .parse()
            .map_err(|error| format!("Browser URL is invalid: {error}"))?;
        let intent_url =
            validated_surface_url(target_url.to_string(), tab_token).ok_or_else(|| {
                "Browser navigation target cannot be used for event correlation".to_string()
            })?;
        let cross_document = !is_fragment_only_navigation(&entry.last_known_url(), &intent_url);
        dispatch_agent_tab_action(workspace, session_id, tab_token, authorization, || {
            if entry.navigation_admission_busy() {
                return Err("Browser page is still loading; verify the current page before dispatching another navigation".to_string());
            }
            entry.begin_external_navigation(cross_document);
            webview.navigate(target_url).map_err(|error| {
                format!(
                    "browser/action-commit-unknown-after-navigation-dispatch: URL navigation acknowledgement was inconclusive: {error}"
                )
            })
        })?;
        Ok(true)
    }

    pub fn history_step_tab_for_agent(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        tab_token: &str,
        delta: i8,
        authorization: &NativeTabLease,
    ) -> Result<bool, String> {
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(false);
        };
        let entry = workspace.tabs.by_token(tab_token).ok_or_else(|| {
            "Tab does not exist or does not belong to this conversation".to_string()
        })?;
        let webview = app
            .get_webview(&entry.label)
            .ok_or_else(|| "Conversation browser surface does not exist".to_string())?;
        dispatch_agent_tab_action(workspace, session_id, tab_token, authorization, || {
            if entry.navigation_admission_busy() {
                return Err("Browser page is still loading; verify the current page before dispatching history navigation".to_string());
            }
            entry.begin_external_navigation(false);
            webview
                .eval(if delta < 0 {
                    "history.back()"
                } else {
                    "history.forward()"
                })
                .map_err(|error| {
                format!(
                    "browser/action-commit-unknown-after-navigation-dispatch: history navigation acknowledgement was inconclusive: {error}"
                )
                })
        })?;
        Ok(true)
    }

    pub fn reload_tab_for_agent(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        tab_token: &str,
        authorization: &NativeTabLease,
    ) -> Result<bool, String> {
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(false);
        };
        let entry = workspace.tabs.by_token(tab_token).ok_or_else(|| {
            "Tab does not exist or does not belong to this conversation".to_string()
        })?;
        let webview = app
            .get_webview(&entry.label)
            .ok_or_else(|| "Conversation browser surface does not exist".to_string())?;
        dispatch_agent_tab_action(workspace, session_id, tab_token, authorization, || {
            if entry.navigation_admission_busy() {
                return Err(
                    "Browser page is still loading; verify the current page before reloading"
                        .to_string(),
                );
            }
            entry.begin_external_navigation(true);
            webview.reload().map_err(|error| {
                format!(
                    "browser/action-commit-unknown-after-navigation-dispatch: reload acknowledgement was inconclusive: {error}"
                )
            })
        })?;
        Ok(true)
    }

    /// A newly created tab starts on an internal about:blank marker so the
    /// host can bind its authoritative CDP target before any remote page is
    /// exposed.  UI-created tabs use this narrow post-bind navigation path;
    /// it deliberately does not issue another control revision because
    /// `create_tab` has already transferred control to the user.
    pub fn navigate_tab_after_bind(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
        url: &str,
    ) -> Result<bool, String> {
        let app = app.ok_or_else(|| "App handle is not ready".to_string())?;
        if !is_trackable_surface_url(url) {
            return Err("browser/url-not-allowed".to_string());
        }
        let staged_key = (session_id.to_string(), tab_token.to_string());
        if let Some(background) = self.staged_user_tabs.get(&staged_key).copied() {
            let entry =
                self.staged_tabs.get(&staged_key).cloned().ok_or_else(|| {
                    "User tab awaiting publication has incomplete state".to_string()
                })?;
            if entry.automation_target.is_none() {
                return Err(
                    "Tab is not bound to an authoritative host automation target".to_string(),
                );
            }
            let webview = app
                .get_webview(&entry.label)
                .ok_or_else(|| "Browser surface awaiting publication does not exist".to_string())?;
            let target_url = marked_blank_url(url, tab_token);
            let target_url = target_url
                .parse()
                .map_err(|error| format!("Browser URL is invalid: {error}"))?;
            entry.begin_external_navigation(true);
            if let Err(error) = webview.navigate(target_url) {
                entry.cancel_active_navigation();
                return Err(format!(
                    "Hidden browser tab initial navigation failed: {error}"
                ));
            }

            let (snapshot, active_tab) = {
                let workspace = self
                    .workspaces
                    .get_mut(session_id)
                    .ok_or_else(|| "Browser workspace does not exist".to_string())?;
                let control = Arc::clone(&workspace.control);
                let previous_active = workspace.active_tab.clone();
                workspace.tabs.insert(entry.clone())?;
                if !background {
                    workspace.active_tab = tab_token.to_string();
                }
                let snapshot = control.bump(Some(NativeControlOwner::User));
                if let Err(error) = persist_workspace_snapshot(workspace, snapshot.revision) {
                    let _ = workspace.tabs.remove_token(tab_token);
                    workspace.active_tab = previous_active.clone();
                    let rollback = control.bump(Some(NativeControlOwner::User));
                    if let Err(restore_error) =
                        persist_workspace_snapshot(workspace, rollback.revision)
                    {
                        eprintln!(
                            "[browser] Failed to roll back user pending-tab mapping: {restore_error}"
                        );
                    }
                    emit_control_changed(app, session_id, &workspace.active_tab, rollback);
                    return Err(error);
                }
                if !background && workspace.visible {
                    if let Err(error) = show_active_workspace(app, workspace) {
                        let _ = workspace.tabs.remove_token(tab_token);
                        workspace.active_tab = previous_active;
                        let rollback = control.bump(Some(NativeControlOwner::User));
                        if let Err(restore_error) =
                            persist_workspace_snapshot(workspace, rollback.revision)
                        {
                            eprintln!(
                                "[browser] Failed to roll back user pending-tab display mapping: {restore_error}"
                            );
                        }
                        if let Err(restore_error) = show_active_workspace(app, workspace) {
                            eprintln!(
                                "[browser] Failed to roll back user pending-tab physical surface: {restore_error}"
                            );
                        }
                        emit_control_changed(app, session_id, &workspace.active_tab, rollback);
                        return Err(error);
                    }
                }
                (snapshot, workspace.active_tab.clone())
            };

            entry.publish();
            self.staged_tabs.remove(&staged_key);
            self.staged_user_tabs.remove(&staged_key);
            emit_control_changed(app, session_id, &active_tab, snapshot);
            if let Err(error) = self.persist_restore_workspace(app, session_id) {
                eprintln!(
                    "[browser] User tab published but restore-manifest refresh failed: {error}"
                );
            }
            return Ok(true);
        }

        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(false);
        };
        let entry = workspace.tabs.by_token(tab_token).ok_or_else(|| {
            "Tab does not exist or does not belong to this conversation".to_string()
        })?;
        if entry.automation_target.is_none() {
            return Err("Tab is not bound to an authoritative host automation target".to_string());
        }
        let webview = app
            .get_webview(&entry.label)
            .ok_or_else(|| "Conversation browser surface does not exist".to_string())?;
        let target_url = marked_blank_url(url, tab_token);
        let target_url = target_url
            .parse()
            .map_err(|error| format!("Browser URL is invalid: {error}"))?;
        entry.begin_external_navigation(true);
        if let Err(error) = webview.navigate(target_url) {
            entry.cancel_active_navigation();
            return Err(format!("Browser navigation failed: {error}"));
        }

        // Keep every restored WebView unpublished until the final tab has a
        // current-process target and completed initial navigation. list, status,
        // and popup therefore never observe a partially restored workspace.
        let should_publish_restore = workspace
            .tabs
            .iter()
            .all(|candidate| candidate.automation_target.is_some())
            && workspace
                .tabs
                .iter()
                .any(|candidate| !candidate.is_published());
        if should_publish_restore {
            for candidate in workspace.tabs.iter() {
                candidate.publish();
            }
            if let Err(error) = self.persist_workspace(session_id) {
                for candidate in workspace.tabs.iter() {
                    candidate.unpublish();
                }
                return Err(error);
            }
            if let Some(state) = self.control_state(session_id) {
                emit_control_changed(app, session_id, &workspace.active_tab, state);
            }
        }
        Ok(true)
    }

    pub fn history_step(
        &self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        delta: i8,
    ) -> Result<bool, String> {
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(false);
        };
        let entry =
            active_entry(workspace).ok_or_else(|| "Current tab does not exist".to_string())?;
        let app = app.ok_or_else(|| "App handle is not ready".to_string())?;
        let webview = app
            .get_webview(&entry.label)
            .ok_or_else(|| "Conversation browser surface does not exist".to_string())?;
        if entry.navigation_in_flight() {
            let _ = webview.eval("window.stop()");
        }
        self.mark_user_control(app, session_id, &entry.token)?;
        entry.begin_external_navigation(false);
        if let Err(error) = webview.eval(if delta < 0 {
            "history.back()"
        } else {
            "history.forward()"
        }) {
            entry.cancel_active_navigation();
            return Err(format!("Browser history navigation failed: {error}"));
        }
        Ok(true)
    }

    pub fn reload(&self, app: Option<&tauri::AppHandle>, session_id: &str) -> Result<bool, String> {
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(false);
        };
        let entry =
            active_entry(workspace).ok_or_else(|| "Current tab does not exist".to_string())?;
        let app = app.ok_or_else(|| "App handle is not ready".to_string())?;
        let webview = app
            .get_webview(&entry.label)
            .ok_or_else(|| "Conversation browser surface does not exist".to_string())?;
        if entry.navigation_in_flight() {
            let _ = webview.eval("window.stop()");
        }
        self.mark_user_control(app, session_id, &entry.token)?;
        entry.begin_external_navigation(true);
        if let Err(error) = webview.reload() {
            entry.cancel_active_navigation();
            return Err(format!("Failed to reload browser page: {error}"));
        }
        Ok(true)
    }

    pub fn has_session(&self, session_id: &str) -> bool {
        self.workspaces.contains_key(session_id)
            || self
                .staged_tabs
                .keys()
                .any(|(owner_session, _)| owner_session == session_id)
    }

    /// Resource-lifecycle ownership is deliberately broader than the business
    /// `has_session` predicate: cleanup-only quarantine must be stoppable but
    /// must never make prepare/restore treat a failed build as a live session.
    pub fn owns_session_resources(&self, session_id: &str) -> bool {
        self.has_session(session_id)
            || self
                .quarantined_tabs
                .keys()
                .any(|(owner_session, _)| owner_session == session_id)
    }

    /// Record (or supersede) the only Prepare request allowed to compensate a
    /// process-local workspace. Existing workspaces deliberately receive no
    /// generation: a later prepare means the previous request no longer owns
    /// teardown, even if its response/tombstone arrives late.
    pub fn record_prepare_generation(
        &mut self,
        session_id: &str,
        request_id: Option<&str>,
    ) -> Result<(), String> {
        let workspace = self
            .workspaces
            .get_mut(session_id)
            .ok_or_else(|| "Browser workspace does not exist".to_string())?;
        workspace.prepare_generation = request_id.map(|request_id| PrepareGeneration {
            request_id: request_id.to_string(),
            revision: workspace.control.snapshot().revision,
        });
        Ok(())
    }

    pub fn prepare_generation_revision(&self, session_id: &str, request_id: &str) -> Option<u64> {
        let workspace = self.workspaces.get(session_id)?;
        let generation = workspace.prepare_generation.as_ref()?;
        (generation.request_id == request_id).then_some(generation.revision)
    }

    /// Compensate a lost Prepare acknowledgement only while the exact request
    /// still owns an untouched workspace generation. A newer prepare, user
    /// takeover/navigation, tab mutation, or Agent activation makes this a
    /// successful no-op rather than deleting current user state.
    pub fn rollback_prepare_generation(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        request_id: &str,
        expected_revision: u64,
        preserve_restore: bool,
    ) -> Result<Option<bool>, String> {
        let Some(workspace) = self.workspaces.get(session_id) else {
            // A previous compensation attempt may already have closed and
            // removed the runtime workspace before the durable restore delete
            // failed. Retrying the same CreatedBlank tombstone must finish
            // that durable delete instead of ACKing a manifest that can revive
            // the cancelled workspace on the next launch. RestoredExisting
            // intentionally keeps the last complete manifest.
            return complete_absent_prepare_rollback(preserve_restore, self.has_sessions(), || {
                remove_restore_file(&paths::browser_session_token(session_id))
            });
        };
        let matches = workspace
            .prepare_generation
            .as_ref()
            .is_some_and(|generation| {
                generation.request_id == request_id
                    && generation.revision == expected_revision
                    && workspace.control.snapshot().revision == expected_revision
            });
        if !matches {
            return Ok(None);
        }
        self.close_session_impl(app, session_id, !preserve_restore)
            .map(Some)
    }

    pub fn has_published_session(&self, session_id: &str) -> bool {
        self.workspaces
            .get(session_id)
            .is_some_and(|workspace| workspace.tabs.iter().any(SurfaceEntry::is_published))
    }

    /// Resolve a task-owned published or exact hidden-staging tab to its Tauri child-WebView
    /// label. BrowserManager holds the random tab token and uses this only after ownership/
    /// generation validation; page code never receives the label or a Tauri bridge.
    pub fn webview_label_for_tab(&self, session_id: &str, tab_token: &str) -> Option<String> {
        self.workspaces
            .get(session_id)
            .and_then(|workspace| workspace.tabs.by_token(tab_token))
            .or_else(|| {
                self.staged_tabs
                    .get(&(session_id.to_string(), tab_token.to_string()))
            })
            .map(|entry| entry.label.clone())
    }

    pub fn active_tab_token(&self, session_id: &str) -> Option<String> {
        self.workspaces
            .get(session_id)
            .map(|workspace| workspace.active_tab.clone())
    }

    pub fn has_sessions(&self) -> bool {
        !self.workspaces.is_empty()
            || !self.staged_tabs.is_empty()
            || !self.quarantined_tabs.is_empty()
    }

    pub fn session_ids(&self) -> Vec<String> {
        let mut ids = self.workspaces.keys().cloned().collect::<Vec<_>>();
        for (session_id, _) in self.staged_tabs.keys() {
            if !ids.contains(session_id) {
                ids.push(session_id.clone());
            }
        }
        for (session_id, _) in self.quarantined_tabs.keys() {
            if !ids.contains(session_id) {
                ids.push(session_id.clone());
            }
        }
        ids.sort();
        ids
    }

    pub fn is_initialized(&self) -> bool {
        self.platform.is_initialized()
    }

    pub fn owns_port(&self, port: u16) -> bool {
        self.platform.owns_port(port)
    }

    fn mark_user_control(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        tab_token: &str,
    ) -> Result<(), String> {
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(());
        };
        let state = workspace.control.bump(Some(NativeControlOwner::User));
        emit_control_changed(app, session_id, tab_token, state);
        if let Err(error) = self.persist_workspace(session_id) {
            eprintln!("[browser] Failed to persist mapping after user takeover: {error}");
        }
        Ok(())
    }

    /// Automatically release control only while the timer still corresponds to
    /// the latest user action. Commit and revision validation share the
    /// WorkspaceControl lock so a late timer cannot overwrite a later action.
    pub fn release_user_control_if_idle(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        expected_revision: u64,
    ) -> Result<bool, String> {
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(false);
        };
        let Some(snapshot) = workspace
            .control
            .release_user_control_if_unchanged(expected_revision)
        else {
            return Ok(false);
        };
        let active_tab = workspace.active_tab.clone();
        emit_control_changed(app, session_id, &active_tab, snapshot);
        if let Err(error) = self.persist_workspace(session_id) {
            eprintln!(
                "[browser] Failed to persist mapping after automatic control handback: {error}"
            );
        }
        Ok(true)
    }

    /// Atomically save the minimal user-recoverable page manifest. URLs come
    /// from host WebViews, never frontend or MCP target mappings. Internal
    /// markers normalize to about:blank.
    pub fn persist_restore_workspace(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
    ) -> Result<(), String> {
        let workspace = self
            .workspaces
            .get(session_id)
            .ok_or_else(|| "Browser workspace does not exist".to_string())?;
        // A restored workspace remains an unpublished transaction until every
        // fresh WebView has been bound and navigated. Navigation callbacks and
        // process exit may race that transaction; preserve the last complete
        // manifest instead of replacing it with temporary marker pages.
        if !workspace_restore_ready(workspace) {
            return Ok(());
        }
        let active_index = workspace
            .tabs
            .iter()
            .position(|tab| tab.token == workspace.active_tab)
            .ok_or_else(|| "Current browser tab is not in the workspace".to_string())?;
        let mut tabs = Vec::with_capacity(workspace.tabs.len());
        for entry in workspace.tabs.iter() {
            let webview = app
                .get_webview(&entry.label)
                .ok_or_else(|| "Browser tab surface does not exist".to_string())?;
            let url = resolve_surface_url(entry, Some(&webview))?;
            tabs.push(json!({ "url": url }));
        }
        if tabs.is_empty() || tabs.len() > MAX_WORKSPACE_TABS {
            return Err("Browser restore tab count is invalid".to_string());
        }
        let restore = NativeWorkspaceRestore {
            urls: tabs
                .into_iter()
                .filter_map(|tab| tab.get("url")?.as_str().map(ToOwned::to_owned))
                .collect(),
            active_index,
        };
        write_restore_workspace_file(
            &paths::browser_workspace_restore_json(&workspace.session_token),
            &restore,
        )
    }

    pub fn persist_all_restore(&self, app: &tauri::AppHandle) -> Result<(), String> {
        for session_id in self.workspaces.keys() {
            self.persist_restore_workspace(app, session_id)?;
        }
        Ok(())
    }

    pub fn persist_navigation_state(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
    ) -> Result<(), String> {
        self.persist_workspace(session_id)?;
        self.persist_restore_workspace(app, session_id)
    }

    fn persist_workspace(&self, session_id: &str) -> Result<(), String> {
        let workspace = self
            .workspaces
            .get(session_id)
            .ok_or_else(|| "Browser workspace does not exist".to_string())?;
        persist_workspace_snapshot(workspace, workspace.control.snapshot().revision)
    }
}

fn workspace_restore_ready(workspace: &Workspace) -> bool {
    !workspace.tabs.is_empty() && workspace.tabs.iter().all(SurfaceEntry::is_published)
}

fn persist_workspace_snapshot(workspace: &Workspace, revision: u64) -> Result<(), String> {
    let path = paths::browser_workspace_state_json(&workspace.session_token);
    // Publish only complete, strictly parseable authoritative v2 mappings.
    // Delete old snapshots between prepare/create and bind_target so the wrapper
    // cannot treat a null or stale target as executable state.
    if workspace
        .tabs
        .iter()
        .any(|tab| tab.automation_target.is_none())
    {
        remove_workspace_state_file(&workspace.session_token).map_err(|error| {
            format!("Failed to remove incomplete browser workspace state: {error}")
        })?;
        return Ok(());
    }
    let value = workspace_state_value_with_revision(workspace, revision);
    let encoded = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
    crate::platform::filesystem::atomic_write_private_anchored(&path, &encoded)
        .map_err(|e| format!("Failed to write browser workspace state: {e}"))
}

fn workspace_state_value_with_revision(workspace: &Workspace, revision: u64) -> serde_json::Value {
    debug_assert!(
        workspace
            .tabs
            .iter()
            .all(|tab| tab.automation_target.is_some())
    );
    json!({
        "version": 2,
        "mapping_authority": "host",
        "revision": revision,
        "session_token": workspace.session_token,
        "active_tab": workspace.active_tab,
        "tabs": workspace.tabs.iter().map(|tab| json!({
            "token": tab.token,
            "target_id": tab.automation_target,
        })).collect::<Vec<_>>(),
    })
}

fn parse_restore_workspace(encoded: &[u8]) -> Result<NativeWorkspaceRestore, String> {
    let decoded: WorkspaceRestoreFile = serde_json::from_slice(encoded)
        .map_err(|error| format!("Failed to parse browser restore manifest: {error}"))?;
    if decoded.version != WORKSPACE_RESTORE_VERSION {
        return Err(format!(
            "Unsupported browser restore manifest version: {}",
            decoded.version
        ));
    }
    if decoded.tabs.is_empty()
        || decoded.tabs.len() > MAX_WORKSPACE_TABS
        || decoded.active_index >= decoded.tabs.len()
    {
        return Err("Browser restore manifest has an invalid tab count or active tab".to_string());
    }
    let mut urls = Vec::with_capacity(decoded.tabs.len());
    for tab in decoded.tabs {
        if !is_trackable_surface_url(&tab.url) {
            return Err("Browser restore manifest contains an unsupported page URL".to_string());
        }
        urls.push(tab.url);
    }
    Ok(NativeWorkspaceRestore {
        urls,
        active_index: decoded.active_index,
    })
}

fn write_restore_workspace_file(
    path: &Path,
    restore: &NativeWorkspaceRestore,
) -> Result<(), String> {
    if restore.urls.is_empty()
        || restore.urls.len() > MAX_WORKSPACE_TABS
        || restore.active_index >= restore.urls.len()
        || restore
            .urls
            .iter()
            .any(|url| !is_trackable_surface_url(url))
    {
        return Err("Browser restore manifest is invalid".to_string());
    }
    let tabs = restore
        .urls
        .iter()
        .map(|url| json!({ "url": url }))
        .collect::<Vec<_>>();
    let encoded = serde_json::to_vec(&json!({
        "version": WORKSPACE_RESTORE_VERSION,
        "active_index": restore.active_index,
        "tabs": tabs,
    }))
    .map_err(|error| format!("Failed to encode browser restore manifest: {error}"))?;
    crate::platform::filesystem::atomic_write_private_anchored(path, &encoded)
        .map_err(|error| format!("Failed to write browser restore manifest: {error}"))
}

fn remove_workspace_state_file(session_token: &str) -> Result<(), String> {
    crate::platform::filesystem::remove_private_file_anchored(&paths::browser_workspace_state_json(
        session_token,
    ))
    .map(|_| ())
    .map_err(|error| format!("Failed to delete browser workspace state: {error}"))
}

fn remove_restore_file(session_token: &str) -> Result<(), String> {
    crate::platform::filesystem::remove_private_file_anchored(
        &paths::browser_workspace_restore_json(session_token),
    )
    .map(|_| ())
    .map_err(|error| format!("Failed to delete browser restore manifest: {error}"))
}

/// Finish a Prepare compensation after its process-local workspace is already
/// gone. Keeping the durable delete as a separate fallible step is important:
/// a first attempt may close every native WebView successfully and then fail
/// filesystem cleanup, so a retry must execute this step again.
fn complete_absent_prepare_rollback(
    preserve_restore: bool,
    has_remaining_sessions: bool,
    delete_restore: impl FnOnce() -> Result<(), String>,
) -> Result<Option<bool>, String> {
    if preserve_restore {
        return Ok(None);
    }
    delete_restore()?;
    Ok(Some(has_remaining_sessions))
}

fn build_webview<P: PlatformWebviewConfig>(
    platform: &P,
    app: &tauri::AppHandle,
    session_id: &str,
    tab_token: &str,
    data_directory: &Path,
    url: &str,
    control: Arc<WorkspaceControl>,
    published: bool,
) -> Result<SurfaceEntry, Box<WebviewBuildError>> {
    let window = app.get_window("main").ok_or_else(|| {
        Box::new(WebviewBuildError::new(
            "Main window is not ready".to_string(),
        ))
    })?;
    let parsed_url = url.parse().map_err(|e| {
        Box::new(WebviewBuildError::new(format!(
            "Initial browser URL is invalid: {e}"
        )))
    })?;
    let initial_last_known_url =
        validated_surface_url(url.to_string(), tab_token).ok_or_else(|| {
            Box::new(WebviewBuildError::new(
                "Initial browser URL cannot be written to restore state".to_string(),
            ))
        })?;
    let page_id = next_native_page_id().map_err(|error| Box::new(WebviewBuildError::new(error)))?;
    let label = format!("{WEBVIEW_LABEL_PREFIX}{tab_token}");
    if let Some(stale) = app.get_webview(&label) {
        stale.close().map_err(|e| {
            Box::new(WebviewBuildError::new(format!(
                "Failed to close invalid browser tab: {e}"
            )))
        })?;
    }

    let navigation_app = app.clone();
    let navigation_session_id = session_id.to_string();
    let navigation_tab_token = tab_token.to_string();
    let navigation_label = label.clone();
    let navigation_control = Arc::clone(&control);
    let publication = Arc::new(std::sync::atomic::AtomicBool::new(published));
    let navigation_publication = Arc::clone(&publication);
    let committed_navigation_app = app.clone();
    let committed_navigation_session_id = session_id.to_string();
    let committed_navigation_tab_token = tab_token.to_string();
    let committed_navigation_label = label.clone();
    let committed_navigation_control = Arc::clone(&control);
    let committed_navigation_publication = Arc::clone(&publication);
    let last_known_url = Arc::new(parking_lot::RwLock::new(initial_last_known_url));
    let committed_last_known_url = Arc::clone(&last_known_url);
    let last_known_title = Arc::new(parking_lot::RwLock::new(None));
    let title_last_known_title = Arc::clone(&last_known_title);
    let user_navigation = Arc::new(parking_lot::Mutex::new(UserNavigationState::default()));
    let navigation_user_navigation = Arc::clone(&user_navigation);
    let committed_user_navigation = Arc::clone(&user_navigation);
    let navigation_callback_last_known_url = Arc::clone(&last_known_url);
    let navigation_callback_last_known_title = Arc::clone(&last_known_title);
    let title_app = app.clone();
    let title_session_id = session_id.to_string();
    let title_tab_token = tab_token.to_string();
    let title_publication = Arc::clone(&publication);
    let popup_app = app.clone();
    let popup_session_id = session_id.to_string();
    let popup_tab_token = tab_token.to_string();
    let popup_control = Arc::clone(&control);
    let popup_publication = Arc::clone(&publication);
    let download_app = app.clone();
    let download_session_id = session_id.to_string();
    let download_tab_token = tab_token.to_string();
    let cdp_tab_token = platform
        .capabilities()
        .chrome_devtools_protocol
        .then_some(tab_token);
    super::register_browser_core_webview_binding(
        &label,
        tab_token,
        &control,
        &user_navigation,
        has_internal_marker_for_token(url, tab_token),
    )
    .map_err(|error| Box::new(WebviewBuildError::new(error)))?;
    let location_signal_nonce = format!("{:032x}", rand::random::<u128>());
    let navigation_location_signal_nonce = location_signal_nonce.clone();
    let init_script = browser_initialization_script(cdp_tab_token, &location_signal_nonce);
    let builder = WebviewBuilder::new(&label, WebviewUrl::External(parsed_url));
    let builder = match platform.configure_builder(builder, data_directory) {
        Ok(builder) => builder,
        Err(error) => {
            super::unregister_browser_core_webview_binding(&label);
            return Err(Box::new(WebviewBuildError::new(error)));
        }
    }
        // BrowserCore runs in every frame on all three desktop engines. The
        // script exposes DOM-only helpers; native input remains behind the
        // task lease in Rust and is never callable by page JavaScript.
        .initialization_script_for_all_frames(init_script)
        .on_download(move |_webview, event| {
            if let DownloadEvent::Requested { url, .. } = event {
                // URLs may contain one-time tokens or query parameters. Expose
                // only the origin to UI and never record a local destination.
                // Embedded downloads are denied; users can use a system browser.
                let source = match (url.scheme(), url.host_str()) {
                    (scheme, Some(host)) => format!("{scheme}://{host}"),
                    (scheme, None) => format!("{scheme}:"),
                };
                let _ = download_app.emit(
                    "browser:download-blocked",
                    json!({
                        "sessionId": download_session_id,
                        "tab": download_tab_token,
                        "source": source,
                    }),
                );
            }
            false
        })
        .on_navigation(move |url| {
            if let Some(signal_nonce) = location_change_signal_nonce(url) {
                if signal_nonce == navigation_location_signal_nonce {
                    let live_url = navigation_app
                        .get_webview(&navigation_label)
                        .and_then(|webview| webview.url().ok())
                        .and_then(|url| {
                            validated_surface_url(url.to_string(), &navigation_tab_token)
                        });
                    if let Some(live_url) = live_url {
                        let request_id = {
                            let mut navigation = navigation_user_navigation.lock();
                            if navigation.navigation_in_flight() {
                                // A trusted history mutation can happen after
                                // Started but before Finished. Keep it inside
                                // that generation without publishing early;
                                // Finished remains the cross-document commit.
                                let _ =
                                    navigation.observe_same_document_during_load(&live_url);
                                None
                            } else {
                                match navigation.finish_same_document(&live_url) {
                                    NavigationCommitDecision::Current { request_id } => {
                                        Some(request_id)
                                    }
                                    NavigationCommitDecision::Stale => None,
                                }
                            }
                        };
                        let Some(request_id) = request_id else {
                            return false;
                        };
                        let Some(changed) = commit_surface_url_before_publication(
                            &navigation_publication,
                            &navigation_callback_last_known_url,
                            Some(&navigation_callback_last_known_title),
                            &live_url,
                        ) else {
                            // Hidden staging surfaces still settle their
                            // generation and in-memory committed URL, but may
                            // not publish UI/control/durable state before the
                            // host atomically commits the tab.
                            return false;
                        };
                        if changed {
                            let payload = json!({
                                "sessionId": navigation_session_id,
                                "tab": navigation_tab_token,
                                "url": live_url,
                                "requestId": request_id,
                            });
                            let _ = navigation_app.emit("browser:navigation", &payload);
                            let _ = navigation_app.emit("browser:tabs-changed", &payload);
                            if let Some(snapshot) = navigation_control
                                .bump_for_navigation_if_no_active_agent_operation()
                            {
                                emit_control_changed(
                                    &navigation_app,
                                    &navigation_session_id,
                                    &navigation_tab_token,
                                    snapshot,
                                );
                            }
                            let persist_app = navigation_app.clone();
                            let persist_session_id = navigation_session_id.clone();
                            tauri::async_runtime::spawn(async move {
                                let manager =
                                    persist_app.state::<super::super::BrowserManager>();
                                if let Err(error) =
                                    manager.persist_native_restore(&persist_session_id)
                                {
                                    eprintln!(
                                        "[browser] Failed to persist same-document page navigation: {error}"
                                    );
                                }
                            });
                        }
                    }
                }
                // The reserved signal never becomes a page/history entry.
                return false;
            }
            let binding_marker = super::classify_browser_core_binding_navigation(
                &navigation_label,
                url.as_str(),
            );
            let internal_marker =
                has_internal_marker_for_token(url.as_str(), &navigation_tab_token);
            if has_reserved_marker_shape(url.as_str()) && !binding_marker && !internal_marker {
                return false;
            }
            // Agent create_tab initial navigation occurs in a hidden staging
            // WebView. Validate protocol only, preventing marker/real URL
            // callbacks from changing ownership, restore state, or UI early.
            if !navigation_publication.load(std::sync::atomic::Ordering::SeqCst) {
                if let Some(target_url) =
                    validated_surface_url(url.to_string(), &navigation_tab_token)
                {
                    navigation_user_navigation
                        .lock()
                        .observe_requested_target(&target_url);
                }
                return binding_marker || internal_marker || is_trackable_surface_url(url.as_str());
            }
            // Linux temporarily navigates an already-published WebView to a
            // process-local marker while recovering its WebDriver handle.
            // Do not expose or persist that transient URL; the exact restored
            // real URL closes the binding window in the platform registry.
            if binding_marker {
                return true;
            }
            if let Some(interaction) = user_takeover_interaction(url) {
                // CDP/platform input may also produce isTrusted=true. Only the
                // short input window explicitly opened by the wrapper after lease
                // validation can suppress this fail-safe takeover.
                if navigation_control.agent_input_in_progress() {
                    return false;
                }
                // This is deliberately a low-privilege one-way signal. Even if a
                // remote page navigates to the reserved scheme, it can only pause
                // the Agent and hand control to the user; it cannot call arbitrary
                // Tauri commands or obtain host data.
                // Every real user activity advances the revision. This both
                // revokes any Agent lease and restarts the 3-second idle
                // hand-back window; timers created by earlier activity then
                // fail their revision CAS instead of stealing control back.
                let snapshot = navigation_control.bump(Some(NativeControlOwner::User));
                let _ = navigation_app.emit(
                    "browser:user-takeover",
                    json!({
                        "sessionId": navigation_session_id,
                        "tabToken": navigation_tab_token,
                        "interaction": interaction,
                        "revision": snapshot.revision,
                    }),
                );
                emit_control_changed(
                    &navigation_app,
                    &navigation_session_id,
                    &navigation_tab_token,
                    snapshot,
                );
                let persist_app = navigation_app.clone();
                let persist_session_id = navigation_session_id.clone();
                tauri::async_runtime::spawn(async move {
                    let manager = persist_app.state::<super::super::BrowserManager>();
                    if let Err(error) = manager.persist_native_restore(&persist_session_id) {
                        eprintln!("[browser] Failed to persist user takeover state; retrying in background: {error}");
                    }
                });
                // Reject before WebView commits navigation so the reserved scheme
                // never enters page history.
                return false;
            }
            if !internal_marker && !is_trackable_surface_url(url.as_str()) {
                // Wry policy callbacks do not expose main-frame identity on
                // every backend. A blocked iframe must not cancel the active
                // top-level generation; failed top-level redirects are closed
                // by the bounded generation watchdog instead.
                let request_id = navigation_user_navigation
                    .lock()
                    .current_request_id_for_blocked_target(url.as_str());
                let _ = navigation_app.emit(
                    "browser:navigation-blocked",
                    json!({
                        "sessionId": navigation_session_id,
                        "tab": navigation_tab_token,
                        "scheme": url.scheme(),
                        "requestId": request_id,
                    }),
                );
                return false;
            }
            if let Some(target_url) =
                validated_surface_url(url.to_string(), &navigation_tab_token)
            {
                navigation_user_navigation
                    .lock()
                    .observe_requested_target(&target_url);
            }
            true
        })
        .on_page_load(move |webview, payload| {
            if payload.event() == PageLoadEvent::Started {
                let payload_url = payload.url().as_str();
                let live_url = webview.url().ok().map(|url| url.to_string());
                if let Some(started_url) = committed_top_level_url(
                    payload_url,
                    live_url.as_deref(),
                    &committed_navigation_tab_token,
                ) {
                    committed_user_navigation
                        .lock()
                        .observe_started(&started_url);
                }
                // Started is only a generation proof. It must never publish
                // an address, title or restore snapshot.
                return;
            }
            // A finished payload whose URL equals Webview::url() is the only
            // engine-independent evidence available here that this callback
            // belongs to the current top-level document. WKWebView may report
            // redirect/frame starts, while WebKitGTK may transiently expose a
            // relative URL during document replacement; neither may update
            // the address bar or the durable restore manifest.
            if payload.event() != PageLoadEvent::Finished {
                return;
            }
            let payload_url = payload.url().as_str();
            // The exact current-tab marker is how an explicit about:blank
            // navigation is represented; committed_top_level_url canonicalizes
            // it after Finished/live equality. Binding markers never represent
            // user-visible navigation and stay excluded before that seam.
            if super::is_browser_core_binding_url(payload_url) {
                return;
            }
            let live_url = webview.url().ok().map(|url| url.to_string());
            let Some(committed_url) = committed_top_level_url(
                payload_url,
                live_url.as_deref(),
                &committed_navigation_tab_token,
            ) else {
                // A redirect/obsolete callback whose payload no longer equals
                // the WebView's live top-level URL cannot close the load gate.
                // Only a matching Finished commit may admit same-document
                // signals again; this is the macOS HTTP/HTTPS flicker barrier.
                return;
            };
            let request_id = match committed_user_navigation.lock().finish(&committed_url) {
                NavigationCommitDecision::Current { request_id } => request_id,
                NavigationCommitDecision::Stale => {
                    // payload==live rules out an iframe callback but does not
                    // prove which overlapping top-level navigation produced
                    // it. The host generation map is the second mandatory
                    // gate; an obsolete generation may neither publish nor
                    // reopen same-document sampling.
                    return;
                }
            };
            super::settle_browser_core_host_bootstrap(
                &committed_navigation_label,
                payload_url,
                live_url.as_deref(),
            );
            let Some(changed) = commit_surface_url_before_publication(
                &committed_navigation_publication,
                &committed_last_known_url,
                None,
                &committed_url,
            ) else {
                // `navigate` may synchronously deliver Finished on some native
                // engines. Preserve the commit for the staging transaction,
                // while leaving UI/control/persistence behind publication.
                return;
            };
            let payload = json!({
                "sessionId": committed_navigation_session_id,
                "tab": committed_navigation_tab_token,
                "url": committed_url,
                "requestId": request_id,
            });
            // Completion is also a synchronization signal for optimistic UI
            // navigation. Re-submitting the same URL must still settle it.
            let _ = committed_navigation_app.emit("browser:navigation", &payload);
            if changed {
                let _ = committed_navigation_app.emit("browser:tabs-changed", &payload);
                if let Some(snapshot) = committed_navigation_control
                    .bump_for_navigation_if_no_active_agent_operation()
                {
                    emit_control_changed(
                        &committed_navigation_app,
                        &committed_navigation_session_id,
                        &committed_navigation_tab_token,
                        snapshot,
                    );
                }
            }
            // Page-load delegates run on the native UI thread and may still be
            // nested under WebView dispatch. Persist asynchronously to avoid
            // re-entering the native_surface lock.
            let persist_app = committed_navigation_app.clone();
            let persist_session_id = committed_navigation_session_id.clone();
            tauri::async_runtime::spawn(async move {
                let manager = persist_app.state::<super::super::BrowserManager>();
                if let Err(error) = manager.persist_native_restore(&persist_session_id) {
                    eprintln!("[browser] Failed to persist page navigation: {error}");
                }
            });
        })
        .on_document_title_changed(move |webview, title| {
            let Some(title_url) = webview
                .url()
                .ok()
                .and_then(|url| validated_surface_url(url.to_string(), &title_tab_token))
            else {
                return;
            };
            let title: String = title.chars().take(512).collect();
            *title_last_known_title.write() = Some((title_url, title.clone()));
            if !title_publication.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            let _ = title_app.emit(
                "browser:tab-title",
                json!({
                    "sessionId": title_session_id,
                    "tab": title_tab_token,
                    "title": title,
                }),
            );
        })
        .on_new_window(move |url, _features| {
            if !popup_publication.load(std::sync::atomic::Ordering::SeqCst) {
                return NewWindowResponse::Deny;
            }
            if !is_trackable_surface_url(url.as_str()) {
                let _ = popup_app.emit(
                    "browser:navigation-blocked",
                    json!({
                        "sessionId": popup_session_id,
                        "scheme": url.scheme(),
                    }),
                );
                return NewWindowResponse::Deny;
            }
            // Only a begun atomic dispatch may copy the complete lease from Rust
            // control state. A spontaneous popup without valid authorization is
            // User-owned. With authorization, BrowserManager uses hidden staging
            // plus final CAS publication and safely rejects a late page when user
            // takeover commits first.
            let authorization =
                popup_agent_authorization(&popup_control, &popup_session_id, &popup_tab_token);
            let app = popup_app.clone();
            let session_id = popup_session_id.clone();
            tauri::async_runtime::spawn(async move {
                let manager = app.state::<super::super::BrowserManager>();
                if let Err(error) = manager
                    .create_popup_tab(&session_id, url.to_string(), authorization)
                    .await
                {
                    eprintln!("[browser] Failed to adopt new page window: {error}");
                }
            });
            NewWindowResponse::Deny
        });
    let webview = match window.add_child(
        builder,
        PhysicalPosition::new(0, 0),
        PhysicalSize::new(1, 1),
    ) {
        Ok(webview) => webview,
        Err(error) => {
            super::unregister_browser_core_webview_binding(&label);
            return Err(Box::new(WebviewBuildError::new(format!(
                "Failed to create system-WebView browser tab: {error}"
            ))));
        }
    };
    let entry = SurfaceEntry {
        label,
        token: tab_token.to_string(),
        page_id,
        automation_target: None,
        created_by_request_id: None,
        published: publication,
        created_at_revision: None,
        last_known_url,
        last_known_title,
        user_navigation,
    };
    if let Err(hide_error) = webview.hide() {
        entry.unpublish();
        return match webview.close() {
            Ok(()) => {
                super::unregister_browser_core_webview_binding(&entry.label);
                Err(Box::new(WebviewBuildError::new(format!(
                    "Failed to initialize hidden browser tab: {hide_error}"
                ))))
            }
            Err(close_error) => Err(Box::new(WebviewBuildError::with_survivor(
                format!(
                    "Failed to initialize hidden browser tab: {hide_error}; compensating close failed: {close_error}"
                ),
                entry,
            ))),
        };
    }
    if let Err(attach_error) = super::attach_native_surface(&webview) {
        entry.unpublish();
        return match webview.close() {
            Ok(()) => {
                super::unregister_browser_core_webview_binding(&entry.label);
                Err(Box::new(WebviewBuildError::new(format!(
                    "Failed to initialize native browser-tab container: {attach_error}"
                ))))
            }
            Err(close_error) => Err(Box::new(WebviewBuildError::with_survivor(
                format!(
                    "Failed to initialize native browser-tab container: {attach_error}; compensating close failed: {close_error}"
                ),
                entry,
            ))),
        };
    }
    Ok(entry)
}

fn popup_agent_authorization(
    control: &WorkspaceControl,
    session_id: &str,
    source_tab_token: &str,
) -> Option<RetainedAgentOperation> {
    control.retain_agent_operation_for_popup(session_id, source_tab_token)
}

fn active_entry(workspace: &Workspace) -> Option<&SurfaceEntry> {
    workspace.tabs.by_token(&workspace.active_tab)
}

fn validate_agent_mutation(
    workspace: &Workspace,
    session_id: &str,
    authorization: &NativeTabLease,
    required_authorization_tab: Option<&str>,
) -> Result<(), String> {
    if authorization.owner != NativeControlOwner::Agent
        || authorization.session_id != session_id
        || workspace.active_tab != authorization.tab_token
        || required_authorization_tab.is_some_and(|required| required != authorization.tab_token)
        || workspace.tabs.target_for_token(&authorization.tab_token)
            != Some(authorization.target_id.as_str())
    {
        return Err("Agent mutation lease does not match the current host tab".to_string());
    }
    Ok(())
}

/// Validate host identity, then keep the workspace control lock through the
/// final native enqueue. This is the common linearization seam for Agent tab
/// navigation/history/reload operations. Its lock order is control -> the
/// short navigation-state calls below -> native enqueue; those navigation
/// guards are released before the WebView call. Agent URL admission excludes
/// the takeover scheme, so a synchronous allowed-target `on_navigation`
/// callback only touches navigation state and never re-enters control. Page
/// self-navigation can run only after the asynchronous enqueue returns.
fn dispatch_agent_tab_action<T, F>(
    workspace: &Workspace,
    session_id: &str,
    tab_token: &str,
    authorization: &NativeTabLease,
    dispatch: F,
) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String>,
{
    validate_agent_mutation(workspace, session_id, authorization, Some(tab_token))?;
    workspace
        .control
        .dispatch_if_agent_authorized(authorization, dispatch)?
        .ok_or_else(|| "browser/control-lease-lost".to_string())
}

// Callers check the tab exists before closing its WebView, so remove_token
// always succeeds; after a non-empty close, `fallback` is clamped into
// 0..tabs.len(), so token_at always succeeds.
#[allow(clippy::expect_used)]
fn remove_tab_from_workspace(workspace: &mut Workspace, tab_token: &str) {
    let (index, _) = workspace
        .tabs
        .remove_token(tab_token)
        .expect("tab was checked before closing WebView");
    if workspace.active_tab == tab_token {
        if workspace.tabs.is_empty() {
            workspace.active_tab.clear();
            return;
        }
        let fallback = index.saturating_sub(1).min(workspace.tabs.len() - 1);
        workspace.active_tab = workspace
            .tabs
            .token_at(fallback)
            .expect("at least one tab remains after close")
            .to_string();
    }
}

fn hide_workspace(app: &tauri::AppHandle, workspace: &Workspace) -> Result<(), String> {
    let mut errors = Vec::new();
    for entry in workspace.tabs.iter() {
        if let Some(webview) = app.get_webview(&entry.label) {
            if let Err(error) = super::hide_native_surface(&webview) {
                errors.push(format!("{}: {error}", entry.label));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Failed to hide native browser tab: {}",
            errors.join("; ")
        ))
    }
}

fn hide_all(app: &tauri::AppHandle, workspaces: &HashMap<String, Workspace>) -> Result<(), String> {
    let mut errors = Vec::new();
    // Hiding is fail-safe: try every workspace even after one physical ACK
    // fails, then return the aggregate so the caller keeps logical visibility
    // unchanged and can retry the transition.
    for (session_id, workspace) in workspaces {
        if let Err(error) = hide_workspace(app, workspace) {
            errors.push(format!("{session_id}: {error}"));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// The application owns one physical browser surface. Publishing one workspace therefore
/// revokes the visibility intent of every other workspace in the same state transition.
fn set_exclusive_workspace_visibility(
    workspaces: &mut HashMap<String, Workspace>,
    session_id: &str,
) {
    for (workspace_session_id, workspace) in workspaces.iter_mut() {
        workspace.visible = workspace_session_id == session_id;
    }
}

fn workspace_may_present_native_surface(
    session_owns_visible_surface: bool,
    workspace_visible: bool,
) -> bool {
    session_owns_visible_surface && workspace_visible
}

fn show_active_workspace(app: &tauri::AppHandle, workspace: &Workspace) -> Result<(), String> {
    hide_workspace(app, workspace)?;
    let entry = active_entry(workspace).ok_or_else(|| "Current tab does not exist".to_string())?;
    let webview = app
        .get_webview(&entry.label)
        .ok_or_else(|| "Current browser tab surface does not exist".to_string())?;
    super::show_native_surface(&webview, workspace.bounds)
}

fn reconcile_workspace_close(
    workspace: &mut Workspace,
    mut close_entry: impl FnMut(&SurfaceEntry) -> Result<(), String>,
) -> Result<(), String> {
    let tokens = workspace
        .tabs
        .iter()
        .map(|entry| entry.token.clone())
        .collect::<Vec<_>>();
    let mut errors = Vec::new();
    for token in tokens {
        // tokens was snapshotted from this workspace's tab list, and each
        // token is removed at most once below.
        #[allow(clippy::expect_used)]
        let entry = workspace
            .tabs
            .by_token(&token)
            .cloned()
            .expect("close list came from the same workspace snapshot");
        match close_entry(&entry) {
            Ok(()) => {
                super::unregister_browser_core_webview_binding(&entry.label);
                remove_tab_from_workspace(workspace, &token);
            }
            Err(error) => errors.push(format!("{}: {error}", entry.label)),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Failed to close conversation browser tabs; {} surfaces remain retryable: {}",
            workspace.tabs.len(),
            errors.join("; ")
        ))
    }
}

fn reconcile_staged_close(
    staged_tabs: &mut HashMap<(String, String), SurfaceEntry>,
    staged_user_tabs: &mut HashMap<(String, String), bool>,
    session_id: &str,
    mut close_entry: impl FnMut(&SurfaceEntry) -> Result<(), String>,
) -> Result<(), String> {
    let mut keys = staged_tabs
        .keys()
        .filter(|(owner_session, _)| owner_session == session_id)
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    let mut errors = Vec::new();
    for key in keys {
        // keys was snapshotted from staged_tabs, and each key is removed at
        // most once below.
        #[allow(clippy::expect_used)]
        let entry = staged_tabs
            .get(&key)
            .cloned()
            .expect("pending staging close list came from the same mapping snapshot");
        match close_entry(&entry) {
            Ok(()) => {
                super::unregister_browser_core_webview_binding(&entry.label);
                staged_tabs.remove(&key);
                staged_user_tabs.remove(&key);
            }
            Err(error) => errors.push(format!("{}: {error}", entry.label)),
        }
    }
    staged_user_tabs.retain(|key, _| staged_tabs.contains_key(key));
    if errors.is_empty() {
        Ok(())
    } else {
        let survivors = staged_tabs
            .keys()
            .filter(|(owner_session, _)| owner_session == session_id)
            .count();
        Err(format!(
            "Failed to close pending browser tabs; {survivors} surfaces remain retryable: {}",
            errors.join("; ")
        ))
    }
}

fn reconcile_quarantined_close(
    quarantined_tabs: &mut HashMap<(String, String), SurfaceEntry>,
    session_id: &str,
    mut close_entry: impl FnMut(&SurfaceEntry) -> Result<(), String>,
) -> Result<(), String> {
    let mut keys = quarantined_tabs
        .keys()
        .filter(|(owner_session, _)| owner_session == session_id)
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    let mut errors = Vec::new();
    for key in keys {
        // keys was snapshotted from quarantined_tabs, and each key is removed
        // at most once below.
        #[allow(clippy::expect_used)]
        let entry = quarantined_tabs
            .get(&key)
            .cloned()
            .expect("quarantine list came from the same mapping snapshot");
        match close_entry(&entry) {
            Ok(()) => {
                super::unregister_browser_core_webview_binding(&entry.label);
                quarantined_tabs.remove(&key);
            }
            Err(error) => errors.push(format!("{}: {error}", entry.label)),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        let survivors = quarantined_tabs
            .keys()
            .filter(|(owner_session, _)| owner_session == session_id)
            .count();
        Err(format!(
            "Failed to close quarantined browser tabs; {survivors} surfaces remain retryable: {}",
            errors.join("; ")
        ))
    }
}

fn close_workspace(app: &tauri::AppHandle, workspace: &mut Workspace) -> Result<(), String> {
    reconcile_workspace_close(workspace, |entry| {
        if let Some(webview) = app.get_webview(&entry.label) {
            webview
                .close()
                .map_err(|error| format!("Failed to close system WebView: {error}"))?;
        }
        Ok(())
    })
}

fn emit_control_changed(
    app: &tauri::AppHandle,
    session_id: &str,
    tab_token: &str,
    snapshot: ControlSnapshot,
) {
    let _ = app.emit(
        "browser:control-changed",
        json!({
            "sessionId": session_id,
            "tabToken": tab_token,
            "revision": snapshot.revision,
            "owner": snapshot.owner.as_str(),
        }),
    );
    if snapshot.owner == NativeControlOwner::User {
        let release_app = app.clone();
        let release_session_id = session_id.to_string();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(USER_CONTROL_IDLE_TIMEOUT).await;
            let manager = release_app.state::<super::super::BrowserManager>();
            if let Err(error) =
                manager.release_user_control_if_idle(&release_session_id, snapshot.revision)
            {
                eprintln!("[browser] Failed to return browser control automatically: {error}");
            }
        });
    }
}

fn sanitize_marker_url(url: String, expected_tab_token: &str) -> String {
    if has_internal_marker_for_token(&url, expected_tab_token) {
        "about:blank".to_string()
    } else {
        url
    }
}

/// A URL may enter native page history only when the host can also publish and
/// persist it. Applying the restore-size ceiling at the navigation-policy seam
/// prevents a remote page from committing an oversized HTTP(S) URL that later
/// callbacks would have to discard, leaving the WebView and host state split.
fn is_trackable_surface_url(url: &str) -> bool {
    url.len() <= MAX_RESTORE_URL_LEN && super::super::is_allowed_url(url)
}

fn validated_surface_url(url: String, expected_tab_token: &str) -> Option<String> {
    let url = sanitize_marker_url(url, expected_tab_token);
    // The current tab's bootstrap marker canonicalizes to about:blank above.
    // A marker carrying any other valid tab/session token is still an
    // implementation URL and must never become address-bar or restore state.
    if internal_marker_token(&url).is_some() || super::is_browser_core_binding_url(&url) {
        return None;
    }
    is_trackable_surface_url(&url).then_some(url)
}

fn committed_top_level_url(
    payload_url: &str,
    live_url: Option<&str>,
    expected_tab_token: &str,
) -> Option<String> {
    if super::is_browser_core_binding_url(payload_url) {
        return None;
    }
    let payload_url = validated_surface_url(payload_url.to_string(), expected_tab_token)?;
    let live_url = live_url
        .filter(|url| !super::is_browser_core_binding_url(url))
        .and_then(|url| validated_surface_url(url.to_string(), expected_tab_token))?;
    (payload_url == live_url).then_some(payload_url)
}

/// Commit host-owned in-memory navigation state before consulting the
/// publication barrier. Native engines may deliver a synchronous commit while
/// a new/restored tab is still staged; suppressing UI and durable side effects
/// must not also discard the only authoritative URL that the later publication
/// transaction needs.
fn commit_surface_url_before_publication(
    publication: &std::sync::atomic::AtomicBool,
    last_known_url: &parking_lot::RwLock<String>,
    same_document_title: Option<&parking_lot::RwLock<Option<(String, String)>>>,
    committed_url: &str,
) -> Option<bool> {
    let previous_url = {
        let mut previous = last_known_url.write();
        if *previous == committed_url {
            None
        } else {
            let old = previous.clone();
            *previous = committed_url.to_string();
            Some(old)
        }
    };
    if let (Some(title), Some(previous_url)) = (same_document_title, previous_url.as_deref()) {
        if let Some((title_url, _)) = title.write().as_mut() {
            if title_url == previous_url {
                *title_url = committed_url.to_string();
            }
        }
    }
    publication
        .load(std::sync::atomic::Ordering::SeqCst)
        .then_some(previous_url.is_some())
}

fn is_fragment_only_navigation(previous_url: &str, next_url: &str) -> bool {
    if previous_url == next_url {
        return false;
    }
    let (Ok(mut previous), Ok(mut next)) =
        (tauri::Url::parse(previous_url), tauri::Url::parse(next_url))
    else {
        return false;
    };
    previous.set_fragment(None);
    next.set_fragment(None);
    previous == next
}

/// Resolve the current top-level URL without allowing one transient engine
/// value to remove a live tab or invalidate the whole restore manifest.
/// The host-owned last committed value is authoritative. `Webview::url()` is
/// only a corruption-recovery fallback when that host value is invalid.
fn resolve_surface_url(
    entry: &SurfaceEntry,
    webview: Option<&tauri::Webview>,
) -> Result<String, String> {
    let live_url = webview
        .and_then(|webview| webview.url().ok())
        .map(|url| url.to_string());
    resolve_surface_url_value(entry, live_url)
}

fn resolve_surface_url_value(
    entry: &SurfaceEntry,
    live_url: Option<String>,
) -> Result<String, String> {
    let fallback = validated_surface_url(entry.last_known_url(), &entry.token);
    let live_url = live_url.and_then(|raw_url| {
        if has_internal_marker_for_token(&raw_url, &entry.token)
            || super::is_browser_core_binding_url(&raw_url)
        {
            return None;
        }
        let url = validated_surface_url(raw_url, &entry.token)?;
        // `about:blank` is a common transient replacement value. A completed
        // top-level blank navigation updates last_known_url in on_page_load;
        // until then it must not overwrite a real restore target.
        if url == "about:blank" {
            return None;
        }
        Some(url)
    });
    // The host-owned Finished/same-document commit remains authoritative,
    // preventing HTTP/HTTPS redirect starts from flashing into the address bar.
    if let Some(fallback) = fallback {
        return Ok(fallback);
    }

    if let Some(url) = live_url {
        entry.remember_url(&url);
        return Ok(url);
    }

    Err("Browser tab has no recoverable top-level URL".to_string())
}

fn has_internal_marker_for_token(url: &str, expected_tab_token: &str) -> bool {
    is_valid_token(expected_tab_token)
        && internal_marker_token(url).is_some_and(|token| token == expected_tab_token)
}

fn has_reserved_marker_shape(url: &str) -> bool {
    const PREFIXES: [&str; 6] = [
        "about:blank#pinvou-session-",
        "about:blank#pinvou-tab-",
        "about:blank%23pinvou-session-",
        "about:blank%23pinvou-tab-",
        "about:blank#pinvou-webdriver-bind-",
        "about:blank%23pinvou-webdriver-bind-",
    ];
    PREFIXES.iter().any(|prefix| url.starts_with(prefix))
}

fn internal_marker_token(url: &str) -> Option<&str> {
    // WKWebView/NSURL serializes the fragment delimiter in opaque `about:` URLs as `%23`.
    // Keep this compatibility at the host-owned marker seam: arbitrary about URLs, suffixes,
    // queries, and markers belonging to another tab must remain untrusted.
    const PREFIXES: [&str; 4] = [
        "about:blank#pinvou-session-",
        "about:blank#pinvou-tab-",
        "about:blank%23pinvou-session-",
        "about:blank%23pinvou-tab-",
    ];
    PREFIXES
        .iter()
        .find_map(|prefix| url.strip_prefix(prefix))
        .filter(|token| is_valid_token(token))
}

fn marked_blank_url(url: &str, tab_token: &str) -> String {
    if url == "about:blank" {
        format!("about:blank#pinvou-tab-{tab_token}")
    } else {
        url.to_string()
    }
}

fn browser_initialization_script(
    cdp_tab_token: Option<&str>,
    location_signal_nonce: &str,
) -> String {
    debug_assert_eq!(location_signal_nonce.len(), 32);
    debug_assert!(
        location_signal_nonce
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    );
    let bootstrap_identity = cdp_tab_token
        .map(|tab_token| {
            debug_assert!(is_valid_token(tab_token));
            format!(
                r#"
  const bootstrapUrl = globalThis.location.href;
  if (bootstrapUrl === 'about:blank#pinvou-session-{tab_token}' ||
      bootstrapUrl === 'about:blank#pinvou-tab-{tab_token}') {{
    Object.defineProperty(globalThis, '__PINVOU_BROWSER_BOOTSTRAP_TOKEN__', {{
      value: '{tab_token}', enumerable: false, configurable: true, writable: false
    }});
  }}"#
            )
        })
        .unwrap_or_default();
    let takeover = format!(
        r#"
(() => {{
{bootstrap_identity}
  const deferSignal = globalThis.queueMicrotask.bind(globalThis);
  const signalLocationChange = () => {{
    deferSignal(() => {{
      try {{ globalThis.location.href = '{LOCATION_CHANGE_SCHEME}://history/{location_signal_nonce}'; }} catch (_) {{}}
    }});
  }};
  const nativePushState = globalThis.history.pushState;
  const nativeReplaceState = globalThis.history.replaceState;
  Object.defineProperty(globalThis.history, 'pushState', {{
    configurable: true,
    writable: true,
    value: function (...args) {{
      const result = Reflect.apply(nativePushState, this, args);
      signalLocationChange();
      return result;
    }}
  }});
  Object.defineProperty(globalThis.history, 'replaceState', {{
    configurable: true,
    writable: true,
    value: function (...args) {{
      const result = Reflect.apply(nativeReplaceState, this, args);
      signalLocationChange();
      return result;
    }}
  }});
  globalThis.addEventListener('popstate', signalLocationChange, {{ passive: true }});
  globalThis.addEventListener('hashchange', signalLocationChange, {{ passive: true }});
  const signalTakeover = (event) => {{
    if (!event.isTrusted) return;
    deferSignal(() => {{
      try {{ globalThis.location.href = '{USER_TAKEOVER_SCHEME}://interaction/' + event.type; }} catch (_) {{}}
    }});
  }};
  for (const type of ['pointerdown', 'keydown', 'wheel']) {{
    globalThis.addEventListener(type, signalTakeover, {{ capture: true, passive: true }});
  }}
}})();
"#
    );
    format!("{takeover}\n{BROWSER_CORE_RUNTIME}")
}

fn user_takeover_interaction(url: &tauri::Url) -> Option<&str> {
    if url.scheme() != USER_TAKEOVER_SCHEME || url.host_str() != Some("interaction") {
        return None;
    }
    match url.path().trim_matches('/') {
        "pointerdown" => Some("pointerdown"),
        "keydown" => Some("keydown"),
        "wheel" => Some("wheel"),
        _ => None,
    }
}

fn location_change_signal_nonce(url: &tauri::Url) -> Option<&str> {
    if url.scheme() != LOCATION_CHANGE_SCHEME || url.host_str() != Some("history") {
        return None;
    }
    let nonce = url.path().trim_matches('/');
    (nonce.len() == 32 && nonce.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(nonce)
}

fn is_valid_token(token: &str) -> bool {
    token.len() == 16 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_page_ids_are_incarnation_scoped_and_javascript_safe() {
        assert_eq!(
            compose_native_page_id(7, 0),
            Some(7_u64 << NATIVE_PAGE_ID_SEQUENCE_BITS)
        );
        assert_ne!(compose_native_page_id(7, 0), compose_native_page_id(8, 0));
        assert_eq!(
            compose_native_page_id(
                NATIVE_PAGE_ID_INCARNATION_LIMIT - 1,
                NATIVE_PAGE_ID_SEQUENCE_LIMIT - 1
            ),
            Some(MAX_SAFE_PAGE_ID)
        );
        assert_eq!(
            compose_native_page_id(NATIVE_PAGE_ID_INCARNATION_LIMIT, 0),
            None
        );
        assert_eq!(
            compose_native_page_id(7, NATIVE_PAGE_ID_SEQUENCE_LIMIT),
            None
        );
    }

    #[derive(Default)]
    struct TestPlatform {
        initialized: bool,
    }

    struct TestHomeGuard {
        previous: Option<std::ffi::OsString>,
    }

    impl TestHomeGuard {
        fn install(path: &Path) -> Self {
            let previous = std::env::var_os("PINVOU3_HOME");
            // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
            unsafe { std::env::set_var("PINVOU3_HOME", path) };
            Self { previous }
        }
    }

    impl Drop for TestHomeGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
                Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
                // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
                None => unsafe { std::env::remove_var("PINVOU3_HOME") },
            }
        }
    }

    impl PlatformWebviewConfig for TestPlatform {
        const ACTIVATION_READY: bool = true;

        fn capabilities(&self) -> NativeSurfaceCapabilities {
            NativeSurfaceCapabilities::new(true, true, true)
        }

        fn requires_reset(&self, _automation_port: Option<u16>, _data_directory: &Path) -> bool {
            false
        }

        fn prepare(
            &mut self,
            _automation_port: Option<u16>,
            _data_directory: &Path,
        ) -> Result<(), String> {
            self.initialized = true;
            Ok(())
        }

        fn configure_builder(
            &self,
            builder: WebviewBuilder<tauri::Wry>,
            _data_directory: &Path,
        ) -> Result<WebviewBuilder<tauri::Wry>, String> {
            Ok(builder)
        }

        fn reset(&mut self) {
            self.initialized = false;
        }

        fn owns_port(&self, _port: u16) -> bool {
            self.initialized
        }

        fn is_initialized(&self) -> bool {
            self.initialized
        }
    }

    fn surface_with_workspace(session_id: &str) -> DesktopBrowserSurface<TestPlatform> {
        let token = "0123456789abcdef".to_string();
        let mut surface = DesktopBrowserSurface {
            platform: TestPlatform { initialized: true },
            data_directory: None,
            workspaces: HashMap::new(),
            staged_tabs: HashMap::new(),
            staged_user_tabs: HashMap::new(),
            quarantined_tabs: HashMap::new(),
            active_session: Some(session_id.to_string()),
            requests: RequestLedger::default(),
        };
        surface.workspaces.insert(
            session_id.to_string(),
            Workspace {
                session_token: token.clone(),
                tabs: TabRegistry::from_entry(SurfaceEntry {
                    label: "test-webview".to_string(),
                    token: token.clone(),
                    page_id: 1,
                    automation_target: None,
                    created_by_request_id: None,
                    published: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                    created_at_revision: None,
                    last_known_url: Arc::new(parking_lot::RwLock::new("about:blank".to_string())),
                    last_known_title: Arc::new(parking_lot::RwLock::new(None)),
                    user_navigation: Arc::new(parking_lot::Mutex::new(
                        UserNavigationState::default(),
                    )),
                }),
                active_tab: token,
                bounds: None,
                visible: false,
                control: Arc::new(WorkspaceControl::new(1, NativeControlOwner::Agent)),
                prepare_generation: None,
            },
        );
        surface
    }

    fn test_entry(
        token: &str,
        label: &str,
        page_id: u64,
        published: bool,
        creation_id: Option<&str>,
    ) -> SurfaceEntry {
        SurfaceEntry {
            label: label.to_string(),
            token: token.to_string(),
            page_id,
            automation_target: None,
            created_by_request_id: creation_id.map(ToOwned::to_owned),
            published: Arc::new(std::sync::atomic::AtomicBool::new(published)),
            created_at_revision: None,
            last_known_url: Arc::new(parking_lot::RwLock::new("about:blank".to_string())),
            last_known_title: Arc::new(parking_lot::RwLock::new(None)),
            user_navigation: Arc::new(parking_lot::Mutex::new(UserNavigationState::default())),
        }
    }

    #[test]
    fn last_known_url_stays_authoritative_over_live_webview_samples() {
        let entry = test_entry("0123456789abcdef", "test-webview", 1, true, None);
        entry.remember_url("http://127.0.0.1:8765/snake.html");
        entry.begin_external_navigation(true);

        assert_eq!(
            resolve_surface_url_value(&entry, None).unwrap(),
            "http://127.0.0.1:8765/snake.html"
        );
        for live_sample in [
            "https://example.com/a-different-page",
            "relative/path",
            "about:blank",
            "about:blank#pinvou-session-0123456789abcdef",
            "about:blank#pinvou-tab-0123456789abcdef",
            "about:blank%23pinvou-session-0123456789abcdef",
            "about:blank%23pinvou-tab-0123456789abcdef",
            "about:blank#pinvou-session-fedcba9876543210",
            "about:blank#pinvou-tab-fedcba9876543210",
            "about:blank%23pinvou-session-fedcba9876543210",
            "about:blank%23pinvou-tab-fedcba9876543210",
        ] {
            assert_eq!(
                resolve_surface_url_value(&entry, Some(live_sample.to_string())).unwrap(),
                "http://127.0.0.1:8765/snake.html",
                "status/persistence sampling must not bypass the Finished commit boundary"
            );
        }

        #[cfg(target_os = "linux")]
        assert_eq!(
            resolve_surface_url_value(
                &entry,
                Some(format!(
                    "about:blank#pinvou-webdriver-bind-{}",
                    "a".repeat(64)
                )),
            )
            .unwrap(),
            "http://127.0.0.1:8765/snake.html"
        );

        assert_eq!(entry.last_known_url(), "http://127.0.0.1:8765/snake.html");
    }

    #[test]
    fn legal_live_url_repairs_only_an_invalid_host_fallback() {
        let entry = test_entry("0123456789abcdef", "test-webview", 1, true, None);

        entry.remember_url("relative/path");
        assert!(resolve_surface_url_value(&entry, None).is_err());
        for rejected_live in [
            "relative/path",
            "about:blank",
            "about:blank#pinvou-session-0123456789abcdef",
            "about:blank#pinvou-tab-0123456789abcdef",
            "about:blank%23pinvou-session-0123456789abcdef",
            "about:blank%23pinvou-tab-0123456789abcdef",
            "about:blank#pinvou-session-fedcba9876543210",
            "about:blank#pinvou-tab-fedcba9876543210",
            "about:blank%23pinvou-session-fedcba9876543210",
            "about:blank%23pinvou-tab-fedcba9876543210",
        ] {
            assert!(resolve_surface_url_value(&entry, Some(rejected_live.to_string())).is_err());
            assert_eq!(entry.last_known_url(), "relative/path");
        }

        #[cfg(target_os = "linux")]
        {
            let binding_marker = format!("about:blank#pinvou-webdriver-bind-{}", "a".repeat(64));
            assert!(resolve_surface_url_value(&entry, Some(binding_marker)).is_err());
            assert_eq!(entry.last_known_url(), "relative/path");
        }

        assert_eq!(
            resolve_surface_url_value(
                &entry,
                Some("http://127.0.0.1:8765/recovered.html".to_string()),
            )
            .unwrap(),
            "http://127.0.0.1:8765/recovered.html"
        );
        assert_eq!(
            entry.last_known_url(),
            "http://127.0.0.1:8765/recovered.html"
        );

        entry.begin_external_navigation(true);
        assert_eq!(
            resolve_surface_url_value(
                &entry,
                Some("https://example.com/must-not-replace-recovery".to_string()),
            )
            .unwrap(),
            "http://127.0.0.1:8765/recovered.html"
        );
    }

    #[test]
    fn finished_top_level_commit_requires_matching_payload_and_live_url() {
        let token = "0123456789abcdef";
        let committed = "http://127.0.0.1:8765/snake.html";

        assert_eq!(
            committed_top_level_url(committed, Some(committed), token),
            Some(committed.to_string())
        );
        assert_eq!(committed_top_level_url(committed, None, token), None);
        assert_eq!(
            committed_top_level_url(committed, Some("https://example.com/redirect"), token),
            None
        );
        assert_eq!(
            committed_top_level_url("relative/path", Some("relative/path"), token),
            None
        );

        for marker in [
            "about:blank#pinvou-session-0123456789abcdef",
            "about:blank#pinvou-tab-0123456789abcdef",
            "about:blank%23pinvou-session-0123456789abcdef",
            "about:blank%23pinvou-tab-0123456789abcdef",
        ] {
            assert_eq!(
                committed_top_level_url(marker, Some(marker), token),
                Some("about:blank".to_string())
            );
            assert_eq!(
                committed_top_level_url(committed, Some(marker), token),
                None
            );
        }

        for marker in [
            "about:blank#pinvou-session-fedcba9876543210",
            "about:blank#pinvou-tab-fedcba9876543210",
            "about:blank%23pinvou-session-fedcba9876543210",
            "about:blank%23pinvou-tab-fedcba9876543210",
        ] {
            assert_eq!(committed_top_level_url(marker, Some(marker), token), None);
            assert_eq!(
                committed_top_level_url(committed, Some(marker), token),
                None
            );
        }

        #[cfg(target_os = "linux")]
        {
            let binding_marker = format!("about:blank#pinvou-webdriver-bind-{}", "a".repeat(64));
            assert_eq!(
                committed_top_level_url(&binding_marker, Some(&binding_marker), token),
                None
            );
            assert_eq!(
                committed_top_level_url(committed, Some(&binding_marker), token),
                None
            );
        }
    }

    #[test]
    fn unpublished_surface_keeps_in_memory_commit_behind_publication_barrier() {
        let publication = std::sync::atomic::AtomicBool::new(false);
        let last_known_url = parking_lot::RwLock::new("https://example.com/page".to_string());
        let title = parking_lot::RwLock::new(Some((
            "https://example.com/page".to_string(),
            "Example".to_string(),
        )));

        assert_eq!(
            commit_surface_url_before_publication(
                &publication,
                &last_known_url,
                Some(&title),
                "https://example.com/page#ready",
            ),
            None,
            "an unpublished surface must not emit UI/control side effects"
        );
        assert_eq!(*last_known_url.read(), "https://example.com/page#ready");
        assert_eq!(
            *title.read(),
            Some((
                "https://example.com/page#ready".to_string(),
                "Example".to_string(),
            )),
            "the staging transaction still needs the committed URL/title pair"
        );

        publication.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            commit_surface_url_before_publication(
                &publication,
                &last_known_url,
                None,
                "https://example.com/next",
            ),
            Some(true)
        );
    }

    #[test]
    fn completed_navigation_to_owned_blank_marker_replaces_remote_restore_url() {
        let token = "0123456789abcdef";
        let marker = "about:blank#pinvou-tab-0123456789abcdef";
        let entry = test_entry(token, "test-webview", 1, true, None);
        entry.remember_url("https://example.com/remote");

        let committed = committed_top_level_url(marker, Some(marker), token)
            .expect("the exact published tab marker is a completed blank page");
        entry.remember_url(&committed);

        assert_eq!(entry.last_known_url(), "about:blank");
        assert_eq!(
            resolve_surface_url_value(&entry, None).unwrap(),
            "about:blank"
        );
    }

    #[test]
    fn close_session_keeps_registration_when_app_handle_is_unavailable() {
        let mut surface = surface_with_workspace("session-a");

        let result = surface.close_session(None, "session-a");

        assert!(result.is_err());
        assert!(surface.has_session("session-a"));
        assert!(surface.has_sessions());
        assert!(surface.is_initialized());
    }

    #[test]
    fn close_unknown_session_is_idempotent_and_keeps_other_workspaces() {
        let mut surface = surface_with_workspace("session-a");

        assert_eq!(surface.close_session(None, "session-missing"), Ok(true));
        assert!(surface.has_session("session-a"));
    }

    #[test]
    fn runtime_tab_limit_counts_hidden_staging_entries() {
        let mut surface = surface_with_workspace("session-a");
        for index in 1..MAX_WORKSPACE_TABS {
            let token = format!("{index:016x}");
            surface.staged_tabs.insert(
                ("session-a".to_string(), token.clone()),
                SurfaceEntry {
                    label: format!("staged-webview-{index}"),
                    token,
                    page_id: index as u64 + 1,
                    automation_target: None,
                    created_by_request_id: Some(format!("create-{index}")),
                    published: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    created_at_revision: None,
                    last_known_url: Arc::new(parking_lot::RwLock::new("about:blank".to_string())),
                    last_known_title: Arc::new(parking_lot::RwLock::new(None)),
                    user_navigation: Arc::new(parking_lot::Mutex::new(
                        UserNavigationState::default(),
                    )),
                },
            );
        }

        assert!(surface.ensure_tab_capacity("session-a").is_err());
        surface
            .staged_tabs
            .remove(&("session-a".to_string(), format!("{:016x}", 1)));
        assert_eq!(surface.ensure_tab_capacity("session-a"), Ok(()));
    }

    #[test]
    fn partial_workspace_close_keeps_only_real_survivors_and_retry_is_idempotent() {
        let mut surface = surface_with_workspace("session-a");
        let workspace = surface.workspaces.get_mut("session-a").unwrap();
        workspace
            .tabs
            .insert(test_entry(
                "1111111111111111",
                "test-webview-b",
                2,
                true,
                None,
            ))
            .unwrap();
        workspace
            .tabs
            .insert(test_entry(
                "2222222222222222",
                "test-webview-c",
                3,
                true,
                None,
            ))
            .unwrap();
        workspace.active_tab = "1111111111111111".to_string();

        let first = reconcile_workspace_close(workspace, |entry| {
            if entry.label == "test-webview-b" {
                Err("injected close failure".to_string())
            } else {
                Ok(())
            }
        });

        assert!(first.is_err());
        assert_eq!(workspace.tabs.len(), 1);
        assert_eq!(workspace.active_tab, "1111111111111111");
        assert_eq!(workspace.tabs.token_at(0), Some("1111111111111111"));

        assert_eq!(reconcile_workspace_close(workspace, |_| Ok(())), Ok(()));
        assert!(workspace.tabs.is_empty());
        assert!(workspace.active_tab.is_empty());
    }

    #[test]
    fn partial_staging_close_retains_failed_generation_and_retries_only_survivor() {
        let mut surface = surface_with_workspace("session-a");
        let key_a = ("session-a".to_string(), "1111111111111111".to_string());
        let key_b = ("session-a".to_string(), "2222222222222222".to_string());
        let other = ("session-b".to_string(), "3333333333333333".to_string());
        surface.staged_tabs.insert(
            key_a.clone(),
            test_entry(&key_a.1, "staged-a", 2, false, Some("create-a")),
        );
        surface.staged_tabs.insert(
            key_b.clone(),
            test_entry(&key_b.1, "staged-b", 3, false, Some("create-b")),
        );
        surface.staged_tabs.insert(
            other.clone(),
            test_entry(&other.1, "staged-other", 4, false, Some("create-other")),
        );
        surface.staged_user_tabs.insert(key_a.clone(), false);
        surface.staged_user_tabs.insert(key_b.clone(), true);

        let first = reconcile_staged_close(
            &mut surface.staged_tabs,
            &mut surface.staged_user_tabs,
            "session-a",
            |entry| {
                if entry.label == "staged-b" {
                    Err("injected close failure".to_string())
                } else {
                    Ok(())
                }
            },
        );

        assert!(first.is_err());
        assert!(!surface.staged_tabs.contains_key(&key_a));
        assert!(surface.staged_tabs.contains_key(&key_b));
        assert!(surface.staged_tabs.contains_key(&other));
        assert!(!surface.staged_user_tabs.contains_key(&key_a));
        assert!(surface.staged_user_tabs.contains_key(&key_b));

        assert_eq!(
            reconcile_staged_close(
                &mut surface.staged_tabs,
                &mut surface.staged_user_tabs,
                "session-a",
                |_| Ok(()),
            ),
            Ok(())
        );
        assert!(!surface.staged_tabs.contains_key(&key_b));
        assert!(surface.staged_tabs.contains_key(&other));
        assert!(!surface.staged_user_tabs.contains_key(&key_b));
    }

    #[test]
    fn hide_and_compensating_close_failure_is_quarantined_and_never_becomes_a_session() {
        let mut surface = DesktopBrowserSurface::<TestPlatform>::default();
        let mut build_error = WebviewBuildError::with_survivor(
            "injected hide and close failure".to_string(),
            test_entry(
                "1111111111111111",
                "quarantined-hide-failure",
                2,
                true,
                Some("must-be-cleared"),
            ),
        );
        surface.quarantine_entries(
            "session-a",
            build_error.survivor.take().into_iter().collect(),
        );

        let key = ("session-a".to_string(), "1111111111111111".to_string());
        let survivor = &surface.quarantined_tabs[&key];
        assert!(!survivor.is_published());
        assert!(survivor.created_by_request_id.is_none());
        assert!(!surface.has_session("session-a"));
        assert!(surface.owns_session_resources("session-a"));
        assert!(surface.has_sessions());
        assert!(surface.webview_label_for_tab("session-a", &key.1).is_none());
    }

    #[test]
    fn repeated_create_gate_retries_one_quarantine_without_accumulating_staging() {
        let mut quarantined = HashMap::from([(
            ("session-a".to_string(), "1111111111111111".to_string()),
            test_entry("1111111111111111", "quarantined-create", 2, false, None),
        )]);

        for _ in 0..2 {
            let result = reconcile_quarantined_close(&mut quarantined, "session-a", |_| {
                Err("injected close failure".to_string())
            });
            assert!(result.is_err());
            assert_eq!(quarantined.len(), 1);
        }
        assert_eq!(
            reconcile_quarantined_close(&mut quarantined, "session-a", |_| Ok(())),
            Ok(())
        );
        assert!(quarantined.is_empty());
    }

    #[test]
    fn post_build_restore_close_failure_moves_survivors_out_of_business_state() {
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::install(temp.path());
        let mut surface = surface_with_workspace("session-a");
        surface
            .workspaces
            .get_mut("session-a")
            .unwrap()
            .tabs
            .insert(test_entry(
                "1111111111111111",
                "restored-second",
                2,
                true,
                None,
            ))
            .unwrap();

        // Model a bind/navigation/persist failure after all restore WebViews
        // were built (and possibly tentatively published).
        surface
            .quarantine_workspace_for_failed_restore("session-a")
            .unwrap();
        assert!(!surface.has_session("session-a"));
        assert!(surface.owns_session_resources("session-a"));
        assert!(
            surface
                .webview_label_for_tab("session-a", "0123456789abcdef")
                .is_none()
        );
        assert!(
            surface
                .webview_label_for_tab("session-a", "1111111111111111")
                .is_none()
        );
        assert!(
            surface
                .quarantined_tabs
                .values()
                .all(|entry| !entry.is_published())
        );

        let first =
            reconcile_quarantined_close(&mut surface.quarantined_tabs, "session-a", |entry| {
                if entry.label == "restored-second" {
                    Err("injected post-build close failure".to_string())
                } else {
                    Ok(())
                }
            });
        assert!(first.is_err());
        assert_eq!(surface.quarantined_tabs.len(), 1);
        assert!(!surface.has_session("session-a"));
        assert!(surface.owns_session_resources("session-a"));

        assert_eq!(
            reconcile_quarantined_close(&mut surface.quarantined_tabs, "session-a", |_| Ok(())),
            Ok(())
        );
        assert!(!surface.owns_session_resources("session-a"));
    }

    #[cfg(unix)]
    #[test]
    fn durable_workspace_operations_reject_replaced_roots_without_touching_targets() {
        use std::os::unix::fs::symlink;

        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::install(temp.path());

        let mut surface = surface_with_workspace("session-a");
        let runtime_path = paths::browser_workspace_state_json("0123456789abcdef");
        let runtime_root = runtime_path.parent().unwrap();
        std::fs::create_dir_all(runtime_root.parent().unwrap()).unwrap();
        let external_runtime = temp.path().join("external-runtime");
        std::fs::create_dir_all(&external_runtime).unwrap();
        let runtime_sentinel = external_runtime.join(runtime_path.file_name().unwrap());
        std::fs::write(&runtime_sentinel, b"outside-runtime-must-remain").unwrap();
        symlink(&external_runtime, runtime_root).unwrap();

        let workspace = surface.workspaces.get_mut("session-a").unwrap();
        assert!(persist_workspace_snapshot(workspace, 1).is_err());
        workspace
            .tabs
            .by_token_mut("0123456789abcdef")
            .unwrap()
            .automation_target = Some("target-a".to_string());
        assert!(persist_workspace_snapshot(workspace, 1).is_err());
        let error = surface
            .quarantine_workspace_for_failed_restore("session-a")
            .unwrap_err();
        assert!(error.contains("workspace state"));
        assert!(!surface.has_session("session-a"));
        assert!(surface.owns_session_resources("session-a"));
        assert_eq!(
            std::fs::read(&runtime_sentinel).unwrap(),
            b"outside-runtime-must-remain"
        );
        std::fs::remove_file(runtime_root).unwrap();

        let restore_token = paths::browser_session_token("session-a");
        let restore_path = paths::browser_workspace_restore_json(&restore_token);
        let restore_root = restore_path.parent().unwrap();
        std::fs::create_dir_all(restore_root.parent().unwrap()).unwrap();
        let external_restore = temp.path().join("external-restore");
        std::fs::create_dir_all(&external_restore).unwrap();
        let restore_sentinel = external_restore.join(restore_path.file_name().unwrap());
        std::fs::write(&restore_sentinel, b"outside-restore-must-remain").unwrap();
        symlink(&external_restore, restore_root).unwrap();
        let restore = NativeWorkspaceRestore {
            urls: vec!["https://example.com/".to_string()],
            active_index: 0,
        };

        assert!(
            DesktopBrowserSurface::<TestPlatform>::read_restore_workspace("session-a").is_err()
        );
        assert!(
            DesktopBrowserSurface::<TestPlatform>::write_restore_workspace("session-a", &restore)
                .is_err()
        );
        assert!(remove_restore_file(&restore_token).is_err());
        assert_eq!(
            std::fs::read(&restore_sentinel).unwrap(),
            b"outside-restore-must-remain"
        );
    }

    #[test]
    fn quarantined_restore_retry_repeats_durable_runtime_delete_before_ack() {
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::install(temp.path());
        let session_id = "retry-quarantined-restore";
        let session_token = paths::browser_session_token(session_id);
        let runtime_path = paths::browser_workspace_state_json(&session_token);
        let mut surface = surface_with_workspace(session_id);
        surface
            .workspaces
            .get_mut(session_id)
            .unwrap()
            .session_token = session_token;

        surface
            .quarantine_workspace_for_failed_restore(session_id)
            .unwrap();
        crate::platform::filesystem::atomic_write_private_anchored(
            &runtime_path,
            b"stale-after-first-delete-failure",
        )
        .unwrap();

        let error = surface
            .quarantine_failed_restore(None, session_id)
            .unwrap_err();
        assert!(error.contains("App handle is not ready"));
        assert!(!runtime_path.exists());
        assert!(!surface.has_session(session_id));
        assert!(surface.owns_session_resources(session_id));
    }

    #[test]
    fn exact_agent_staging_rollback_releases_capacity_and_checks_generation() {
        let mut surface = surface_with_workspace("session-a");
        let key = ("session-a".to_string(), "1111111111111111".to_string());
        surface.staged_tabs.insert(
            key.clone(),
            test_entry(&key.1, "staged-agent", 2, false, Some("create-a")),
        );

        assert!(
            surface
                .rollback_staged_agent_creation(None, "session-a", &key.1, "wrong-generation")
                .is_err()
        );
        assert!(surface.staged_tabs.contains_key(&key));
        assert_eq!(
            surface.rollback_staged_agent_creation(None, "session-a", &key.1, "create-a"),
            Ok(true)
        );
        assert!(!surface.staged_tabs.contains_key(&key));
        assert_eq!(surface.ensure_tab_capacity("session-a"), Ok(()));
    }

    #[test]
    fn hosted_core_cancellation_revokes_operation_and_exact_staging_generation() {
        let mut surface = surface_with_workspace("session-a");
        let key = ("session-a".to_string(), "1111111111111111".to_string());
        let other = ("session-a".to_string(), "2222222222222222".to_string());
        surface.staged_tabs.insert(
            key.clone(),
            test_entry(&key.1, "staged-cancelled", 2, false, Some("request-a")),
        );
        surface.staged_tabs.insert(
            other.clone(),
            test_entry(&other.1, "staged-other", 3, false, Some("request-b")),
        );

        let control = Arc::clone(&surface.workspaces["session-a"].control);
        let (snapshot, opaque_lease) = control.issue_agent_lease();
        let authorization = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            snapshot.revision,
            opaque_lease,
        )
        .unwrap();
        assert!(control.begin_agent_operation(&authorization, true));

        surface
            .cancel_in_flight_core_request(None, "session-a", "request-a")
            .unwrap();

        assert!(control.active_agent_operation().is_none());
        assert!(!control.agent_input_in_progress());
        assert!(!surface.staged_tabs.contains_key(&key));
        assert!(surface.staged_tabs.contains_key(&other));
    }

    #[test]
    fn unpublished_restore_transaction_is_not_eligible_for_snapshot() {
        let surface = surface_with_workspace("session-a");
        let workspace = &surface.workspaces["session-a"];
        assert!(workspace_restore_ready(workspace));

        workspace
            .tabs
            .by_token("0123456789abcdef")
            .unwrap()
            .unpublish();
        assert!(!workspace_restore_ready(workspace));
    }

    #[test]
    fn prepare_rollback_generation_is_invalidated_by_takeover_or_later_prepare() {
        let mut surface = surface_with_workspace("session-a");
        surface
            .record_prepare_generation("session-a", Some("prepare-a"))
            .unwrap();
        assert_eq!(
            surface.prepare_generation_revision("session-a", "prepare-a"),
            Some(1)
        );

        surface.workspaces["session-a"]
            .control
            .bump(Some(NativeControlOwner::User));
        assert_eq!(
            surface
                .rollback_prepare_generation(None, "session-a", "prepare-a", 1, false,)
                .unwrap(),
            None
        );
        assert!(surface.has_session("session-a"));

        surface
            .record_prepare_generation("session-a", Some("prepare-b"))
            .unwrap();
        surface
            .record_prepare_generation("session-a", None)
            .unwrap();
        assert_eq!(
            surface.prepare_generation_revision("session-a", "prepare-b"),
            None
        );
    }

    #[test]
    fn newer_prepare_supersedes_late_old_tombstone_even_at_same_revision() {
        let mut surface = surface_with_workspace("session-a");
        surface
            .record_prepare_generation("session-a", Some("prepare-old"))
            .unwrap();
        let old_revision = surface
            .prepare_generation_revision("session-a", "prepare-old")
            .unwrap();

        // Existing-workspace Prepare does not need to mutate control state, so
        // request identity—not revision alone—must make the old CAS fail.
        surface
            .record_prepare_generation("session-a", Some("prepare-new"))
            .unwrap();
        assert_eq!(
            surface
                .rollback_prepare_generation(None, "session-a", "prepare-old", old_revision, false,)
                .unwrap(),
            None
        );
        assert!(surface.has_session("session-a"));
        assert_eq!(
            surface.prepare_generation_revision("session-a", "prepare-new"),
            Some(old_revision)
        );
    }

    #[test]
    fn created_blank_prepare_rollback_retry_finishes_durable_delete_after_runtime_is_gone() {
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_home = std::env::var_os("PINVOU3_HOME");
        let temp = tempfile::tempdir().unwrap();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", temp.path()) };

        let session_id = format!(
            "missing-created-blank-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        );
        let restore_path =
            paths::browser_workspace_restore_json(&paths::browser_session_token(&session_id));
        std::fs::create_dir_all(restore_path.parent().unwrap()).unwrap();
        std::fs::write(&restore_path, b"stale-created-blank").unwrap();

        let mut surface = surface_with_workspace(&session_id);
        // Model the first rollback after native close has already succeeded:
        // the runtime workspace is gone, but its durable manifest deletion
        // fails. This boundary must remain retryable instead of ACKing early.
        surface.workspaces.remove(&session_id);
        surface.active_session = None;
        assert_eq!(
            complete_absent_prepare_rollback(false, surface.has_sessions(), || Err(
                "injected durable restore deletion failure".to_string()
            )),
            Err("injected durable restore deletion failure".to_string())
        );
        assert!(!surface.has_session(&session_id));
        assert!(restore_path.exists());

        // The repeated CreatedBlank compensation sees no runtime workspace,
        // retries only the durable deletion, and completes successfully.
        assert_eq!(
            surface
                .rollback_prepare_generation(None, &session_id, "prepare-a", 1, false)
                .unwrap(),
            Some(false)
        );
        assert!(!restore_path.exists());

        std::fs::write(&restore_path, b"restored-existing").unwrap();
        let delete_called = std::cell::Cell::new(false);
        assert_eq!(
            complete_absent_prepare_rollback(true, surface.has_sessions(), || {
                delete_called.set(true);
                Ok(())
            }),
            Ok(None)
        );
        assert!(!delete_called.get());
        assert_eq!(
            surface
                .rollback_prepare_generation(None, &session_id, "prepare-b", 1, true)
                .unwrap(),
            None
        );
        assert!(restore_path.exists());

        match previous_home {
            // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: holding platform::paths::tests::ENV_LOCK; in-process env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
    }

    #[test]
    fn selecting_visible_workspace_clears_every_other_visibility_flag() {
        let mut surface = surface_with_workspace("session-a");
        surface.workspaces.get_mut("session-a").unwrap().visible = true;
        let token = "fedcba9876543210".to_string();
        surface.workspaces.insert(
            "session-b".to_string(),
            Workspace {
                session_token: token.clone(),
                tabs: TabRegistry::from_entry(SurfaceEntry {
                    label: "test-webview-b".to_string(),
                    token: token.clone(),
                    page_id: 2,
                    automation_target: None,
                    created_by_request_id: None,
                    published: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                    created_at_revision: None,
                    last_known_url: Arc::new(parking_lot::RwLock::new("about:blank".to_string())),
                    last_known_title: Arc::new(parking_lot::RwLock::new(None)),
                    user_navigation: Arc::new(parking_lot::Mutex::new(
                        UserNavigationState::default(),
                    )),
                }),
                active_tab: token,
                bounds: None,
                // Reproduce a stale state left by an older show implementation.
                visible: true,
                control: Arc::new(WorkspaceControl::new(1, NativeControlOwner::Agent)),
                prepare_generation: None,
            },
        );

        set_exclusive_workspace_visibility(&mut surface.workspaces, "session-b");

        assert!(!surface.workspaces["session-a"].visible);
        assert!(surface.workspaces["session-b"].visible);
        assert_eq!(
            surface
                .workspaces
                .values()
                .filter(|workspace| workspace.visible)
                .count(),
            1
        );
    }

    #[test]
    fn background_workspace_is_not_eligible_for_physical_activation() {
        assert!(!workspace_may_present_native_surface(false, true));
        assert!(!workspace_may_present_native_surface(true, false));
        assert!(workspace_may_present_native_surface(true, true));
    }

    #[test]
    fn blank_page_keeps_a_hidden_workspace_marker() {
        assert_eq!(
            marked_blank_url("about:blank", "0123456789abcdef"),
            "about:blank#pinvou-tab-0123456789abcdef"
        );
        assert_eq!(
            sanitize_marker_url(
                "about:blank#pinvou-tab-0123456789abcdef".to_string(),
                "0123456789abcdef",
            ),
            "about:blank"
        );
    }

    #[test]
    fn wkwebview_percent_encoded_marker_is_canonicalized_for_its_exact_tab() {
        for marker in [
            "about:blank%23pinvou-session-0123456789abcdef",
            "about:blank%23pinvou-tab-0123456789abcdef",
        ] {
            assert!(has_internal_marker_for_token(marker, "0123456789abcdef"));
            assert_eq!(
                sanitize_marker_url(marker.to_string(), "0123456789abcdef"),
                "about:blank"
            );
        }
    }

    #[test]
    fn wkwebview_marker_alias_cannot_claim_another_tab_or_remote_url() {
        let marker = "about:blank%23pinvou-tab-0123456789abcdef";
        assert!(!has_internal_marker_for_token(marker, "fedcba9876543210"));
        assert_eq!(
            sanitize_marker_url(marker.to_string(), "fedcba9876543210"),
            marker
        );
        for untrusted in [
            "about:blank%23pinvou-tab-0123456789abcdef-extra",
            "about:blank%23pinvou-tab-0123456789abcdef?query=1",
            "https://example.com/%23pinvou-tab-0123456789abcdef",
        ] {
            assert!(!has_internal_marker_for_token(
                untrusted,
                "0123456789abcdef"
            ));
        }
    }

    #[test]
    fn normal_page_url_is_not_modified() {
        assert_eq!(
            marked_blank_url("https://example.com/path#section", "0123456789abcdef"),
            "https://example.com/path#section"
        );
    }

    #[test]
    fn takeover_script_exposes_only_a_trusted_low_privilege_signal() {
        let script = browser_initialization_script(
            Some("0123456789abcdef"),
            "0123456789abcdef0123456789abcdef",
        );
        assert!(script.contains("event.isTrusted"));
        assert!(script.contains("queueMicrotask.bind"));
        assert!(script.contains("pinvou-user-takeover://interaction/"));
        let global_debounce_marker = ["last", "SignalAt"].concat();
        assert!(!script.contains(&global_debounce_marker));
        assert!(script.contains("about:blank#pinvou-session-0123456789abcdef"));
        assert!(script.contains("about:blank#pinvou-tab-0123456789abcdef"));
        assert!(script.contains("Object.defineProperty"));
        assert!(script.contains("enumerable: false"));
        assert!(!script.contains("globalThis.__PINVOU_BROWSER_BOOTSTRAP_TOKEN__ ="));
        assert!(!script.contains("PINVOU_BROWSER_TAB_TOKEN"));
        assert!(!script.contains("__TAURI__"));
        assert!(!script.contains("invoke("));
        assert!(script.contains("history.pushState"));
        assert!(script.contains("history.replaceState"));
        assert!(script.contains("hashchange"));
        assert!(
            script.contains("pinvou-location-change://history/0123456789abcdef0123456789abcdef")
        );
    }

    #[test]
    fn same_document_signal_requires_the_exact_per_webview_nonce() {
        let url =
            tauri::Url::parse("pinvou-location-change://history/0123456789abcdef0123456789abcdef")
                .unwrap();
        assert_eq!(
            location_change_signal_nonce(&url),
            Some("0123456789abcdef0123456789abcdef")
        );
        for rejected in [
            "pinvou-location-change://other/0123456789abcdef0123456789abcdef",
            "pinvou-location-change://history/short",
            "pinvou-location-change://history/0123456789abcdef0123456789abcdef-extra",
        ] {
            assert_eq!(
                location_change_signal_nonce(&tauri::Url::parse(rejected).unwrap()),
                None
            );
        }
    }

    #[test]
    fn only_fragment_changes_bypass_the_cross_document_load_gate() {
        assert!(is_fragment_only_navigation(
            "https://example.com/path?q=1#old",
            "https://example.com/path?q=1#new"
        ));
        assert!(!is_fragment_only_navigation(
            "https://example.com/path",
            "https://example.com/other"
        ));
        assert!(!is_fragment_only_navigation(
            "https://example.com/path",
            "https://example.com/path"
        ));
    }

    #[test]
    fn oversized_http_self_navigation_is_rejected_before_native_commit() {
        let prefix = "https://example.com/";
        let at_limit = format!("{prefix}{}", "a".repeat(MAX_RESTORE_URL_LEN - prefix.len()));
        let oversized = format!("{at_limit}a");

        assert_eq!(at_limit.len(), MAX_RESTORE_URL_LEN);
        assert!(super::super::super::is_allowed_url(&oversized));
        assert!(is_trackable_surface_url(&at_limit));
        assert!(!is_trackable_surface_url(&oversized));
        assert_eq!(
            validated_surface_url(oversized, "0123456789abcdef"),
            None,
            "the navigation-policy callback must reject an allowed-scheme URL that cannot be persisted"
        );
    }

    #[test]
    fn browser_core_page_script_contains_no_task_or_tab_identity() {
        let location_nonce = "fedcbafedcbafedcbafedcbafedcbafe";
        let script = browser_initialization_script(None, location_nonce);
        assert!(
            script.contains(location_nonce),
            "the per-WebView history signal nonce is intentionally page-visible"
        );
        assert!(!script.contains("0123456789abcdef"));
        assert!(!script.contains("PINVOU_BROWSER_BOOTSTRAP_TOKEN"));
        assert!(!script.contains("session_token"));
        assert!(!script.contains("tab_token"));
        // BrowserCore's documentation may name the host-side lease boundary, but the
        // injected page runtime must never contain a lease field or page-visible handle.
        assert!(!script.contains("__PINVOU_BROWSER_LEASE"));
        assert!(!script.contains("\"lease\":"));
    }

    #[test]
    fn only_exact_about_blank_markers_are_internal() {
        for marker in [
            "about:blank#pinvou-session-0123456789abcdef",
            "about:blank#pinvou-tab-0123456789abcdef",
            "about:blank%23pinvou-session-0123456789abcdef",
            "about:blank%23pinvou-tab-0123456789abcdef",
        ] {
            assert!(has_internal_marker_for_token(marker, "0123456789abcdef"));
        }
        assert!(!has_internal_marker_for_token(
            "https://example.com/#pinvou-tab-0123456789abcdef",
            "0123456789abcdef"
        ));
        assert!(!has_internal_marker_for_token(
            "about:blank#pinvou-tab-0123456789abcdef-extra",
            "0123456789abcdef"
        ));
        assert!(!super::super::super::is_allowed_url(
            "about:blank%23pinvou-tab-0123456789abcdef"
        ));
        assert!(parse_restore_workspace(
            br#"{"version":1,"active_index":0,"tabs":[{"url":"about:blank%23pinvou-tab-0123456789abcdef"}]}"#,
        )
        .is_err());
    }

    #[test]
    fn restore_manifest_accepts_only_urls_order_and_active_index() {
        let restore = parse_restore_workspace(
            br#"{"version":1,"active_index":1,"tabs":[{"url":"https://example.com/a"},{"url":"about:blank"}]}"#,
        )
        .unwrap();
        assert_eq!(
            restore,
            NativeWorkspaceRestore {
                urls: vec![
                    "https://example.com/a".to_string(),
                    "about:blank".to_string()
                ],
                active_index: 1,
            }
        );
        assert!(parse_restore_workspace(
            br#"{"version":1,"active_index":0,"target_id":"old","tabs":[{"url":"https://example.com"}]}"#,
        )
        .is_err());
        assert!(
            parse_restore_workspace(
                br#"{"version":1,"active_index":0,"tabs":[{"url":"file:///secret"}]}"#,
            )
            .is_err()
        );
    }

    #[test]
    fn restore_writer_never_serializes_runtime_identity() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("restore.json");
        write_restore_workspace_file(
            &path,
            &NativeWorkspaceRestore {
                urls: vec!["https://example.com/path?q=1".to_string()],
                active_index: 0,
            },
        )
        .unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(value["version"], WORKSPACE_RESTORE_VERSION);
        assert_eq!(value["active_index"], 0);
        assert_eq!(value["tabs"][0]["url"], "https://example.com/path?q=1");
        let encoded = value.to_string();
        for forbidden in ["target_id", "lease", "session_token", "tab_token"] {
            assert!(!encoded.contains(forbidden));
        }
    }

    #[test]
    fn takeover_navigation_accepts_only_the_reserved_scheme_and_route() {
        let pointer = "pinvou-user-takeover://interaction/pointerdown"
            .parse::<tauri::Url>()
            .unwrap();
        let unrelated = "https://example.com/interaction/pointerdown"
            .parse::<tauri::Url>()
            .unwrap();
        let unknown = "pinvou-user-takeover://interaction/click"
            .parse::<tauri::Url>()
            .unwrap();
        assert_eq!(user_takeover_interaction(&pointer), Some("pointerdown"));
        assert_eq!(user_takeover_interaction(&unrelated), None);
        assert_eq!(user_takeover_interaction(&unknown), None);
        for allowed_agent_target in [
            "http://example.com/interaction/pointerdown",
            "https://example.com/interaction/pointerdown",
            "about:blank",
        ] {
            let allowed_agent_target = allowed_agent_target.parse::<tauri::Url>().unwrap();
            assert_eq!(
                user_takeover_interaction(&allowed_agent_target),
                None,
                "an admitted Agent target must not enter the control-taking callback branch"
            );
        }
        assert!(!super::super::super::is_allowed_url(
            "pinvou-user-takeover://interaction/pointerdown"
        ));
    }

    #[test]
    fn assert_lease_checks_host_target_revision_and_owner() {
        let mut surface = surface_with_workspace("session-a");
        let workspace = surface.workspaces.get_mut("session-a").unwrap();
        workspace
            .tabs
            .bind_target("0123456789abcdef", "target-a")
            .unwrap();
        let (snapshot, opaque_lease) = workspace.control.issue_agent_lease();
        let lease = NativeTabLease {
            session_id: "session-a".to_string(),
            tab_token: "0123456789abcdef".to_string(),
            target_id: "target-a".to_string(),
            revision: snapshot.revision,
            owner: NativeControlOwner::Agent,
            lease: opaque_lease,
        };

        assert!(surface.assert_lease(&lease).unwrap());
        let mut wrong_target = lease.clone();
        wrong_target.target_id = "target-b".to_string();
        assert!(!surface.assert_lease(&wrong_target).unwrap());
        let mut forged = lease.clone();
        forged.lease = "00000000000000000000000000000000".to_string();
        assert!(!surface.assert_lease(&forged).unwrap());
        surface.workspaces["session-a"]
            .control
            .bump(Some(NativeControlOwner::User));
        assert!(!surface.assert_lease(&lease).unwrap());
    }

    #[test]
    fn agent_tab_action_dispatch_is_atomic_with_final_control_authorization() {
        let mut surface = surface_with_workspace("session-a");
        let control = {
            let workspace = surface.workspaces.get_mut("session-a").unwrap();
            workspace
                .tabs
                .bind_target("0123456789abcdef", "target-a")
                .unwrap();
            Arc::clone(&workspace.control)
        };
        let (snapshot, opaque_lease) = control.issue_agent_lease();
        let authorization = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            snapshot.revision,
            opaque_lease,
        )
        .unwrap();
        assert!(control.begin_agent_operation(&authorization, false));

        let dispatches = std::sync::atomic::AtomicUsize::new(0);
        assert_eq!(
            dispatch_agent_tab_action(
                &surface.workspaces["session-a"],
                "session-a",
                "0123456789abcdef",
                &authorization,
                || {
                    dispatches.fetch_add(1, Ordering::SeqCst);
                    Ok("queued")
                },
            ),
            Ok("queued")
        );

        control.bump(Some(NativeControlOwner::User));
        assert_eq!(
            dispatch_agent_tab_action(
                &surface.workspaces["session-a"],
                "session-a",
                "0123456789abcdef",
                &authorization,
                || {
                    dispatches.fetch_add(1, Ordering::SeqCst);
                    Ok("must-not-run")
                },
            ),
            Err("browser/control-lease-lost".to_string())
        );
        assert_eq!(dispatches.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn popup_inside_begun_agent_dispatch_captures_complete_lease() {
        let mut surface = surface_with_workspace("session-a");
        let workspace = surface.workspaces.get_mut("session-a").unwrap();
        workspace
            .tabs
            .bind_target("0123456789abcdef", "target-a")
            .unwrap();
        let (snapshot, opaque_lease) = workspace.control.issue_agent_lease();
        let authorization = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            snapshot.revision,
            opaque_lease,
        )
        .unwrap();
        let epoch = AgentCallerEpoch::new(41, "0123456789abcdef0123456789abcdef").unwrap();
        assert!(workspace.control.begin_agent_operation_for_caller(
            &authorization,
            false,
            epoch.clone()
        ));

        let retained =
            popup_agent_authorization(&workspace.control, "session-a", "0123456789abcdef")
                .expect("popup must retain the begun operation");
        assert_eq!(retained.authorization(), &authorization);
        assert_eq!(retained.caller_epoch(), &epoch);
        workspace
            .control
            .release_retained_agent_operation(&retained);
        workspace.control.end_agent_operation(&authorization);
    }

    #[test]
    fn popup_without_valid_dispatch_is_user_owned() {
        let mut surface = surface_with_workspace("session-a");
        let workspace = surface.workspaces.get_mut("session-a").unwrap();
        workspace
            .tabs
            .bind_target("0123456789abcdef", "target-a")
            .unwrap();
        let (snapshot, opaque_lease) = workspace.control.issue_agent_lease();
        let authorization = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            snapshot.revision,
            opaque_lease,
        )
        .unwrap();

        assert!(
            popup_agent_authorization(&workspace.control, "session-a", "0123456789abcdef")
                .is_none()
        );
        assert!(workspace.control.begin_agent_operation_for_caller(
            &authorization,
            true,
            AgentCallerEpoch::new(41, "0123456789abcdef0123456789abcdef").unwrap()
        ));
        workspace.control.bump(Some(NativeControlOwner::User));
        assert!(
            popup_agent_authorization(&workspace.control, "session-a", "0123456789abcdef")
                .is_none()
        );
    }

    #[test]
    fn target_binding_is_unique_across_workspaces() {
        let mut surface = surface_with_workspace("session-a");
        surface
            .workspaces
            .get_mut("session-a")
            .unwrap()
            .tabs
            .bind_target("0123456789abcdef", "target-a")
            .unwrap();
        let token = "fedcba9876543210".to_string();
        surface.workspaces.insert(
            "session-b".to_string(),
            Workspace {
                session_token: token.clone(),
                tabs: TabRegistry::from_entry(SurfaceEntry {
                    label: "test-webview-b".to_string(),
                    token: token.clone(),
                    page_id: 2,
                    automation_target: None,
                    created_by_request_id: None,
                    published: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                    created_at_revision: None,
                    last_known_url: Arc::new(parking_lot::RwLock::new("about:blank".to_string())),
                    last_known_title: Arc::new(parking_lot::RwLock::new(None)),
                    user_navigation: Arc::new(parking_lot::Mutex::new(
                        UserNavigationState::default(),
                    )),
                }),
                active_tab: token,
                bounds: None,
                visible: false,
                control: Arc::new(WorkspaceControl::new(1, NativeControlOwner::Agent)),
                prepare_generation: None,
            },
        );

        assert!(
            surface
                .bind_target("session-b", "fedcba9876543210", "target-a")
                .is_err()
        );
    }

    #[test]
    fn workspace_schema_persists_revision_and_authoritative_target_binding() {
        let mut surface = surface_with_workspace("session-a");
        let workspace = surface.workspaces.get_mut("session-a").unwrap();
        workspace
            .tabs
            .bind_target("0123456789abcdef", "target-a")
            .unwrap();
        workspace.control.bump(Some(NativeControlOwner::Agent));

        let value =
            workspace_state_value_with_revision(workspace, workspace.control.snapshot().revision);
        assert_eq!(value["version"], 2);
        assert_eq!(value["mapping_authority"], "host");
        assert_eq!(value["revision"], 2);
        assert_eq!(value["tabs"][0]["token"], "0123456789abcdef");
        assert_eq!(value["tabs"][0]["target_id"], "target-a");
    }
}
