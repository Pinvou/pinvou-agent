use base64::Engine as _;
use serde::Serialize;
use serde_json::Value;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, State, WebviewWindow};

use crate::features::assistant::engine_pool::EnginePool;
use crate::features::codex_acp::workspace::{
    WorkspaceChanges, WorkspaceDiff, WorkspaceEntry, WorkspaceListing, WorkspacePreview,
};
use crate::features::codex_acp::{
    AcpEventEnvelope, AcpPool, AgentBackend, CodexAcpPendingElicitation, CodexAcpPendingPermission,
    CodexAcpSessionInfo, project_acp_elicitation_request_for_web,
    project_acp_permission_request_for_web, project_acp_value_for_web,
};
use crate::features::remote_control::{MAX_TRANSFER_CHUNK_BYTES, file_access, manager};
use crate::features::remote_control::{
    RelaySettingsInfo, RemoteControlManager, WebAccessInfo, WebAccessStatus,
};
use crate::features::sessions::SessionStore;
use crate::platform::prefs::UserPrefs;

const MAX_WEB_ARTIFACT_RPC_BYTES: usize = 2 * 1024 * 1024;
const WEB_SESSION_SERIALIZATION_HEADROOM: usize = 1024 * 1024;
const DEFAULT_WEB_ACP_TIMELINE_PAGE_EVENTS: usize = 128;
const MAX_WEB_ACP_TIMELINE_PAGE_EVENTS: usize = 256;
// Keep ample room for the RPC envelope below manager::MAX_RPC_RESPONSE_BYTES.
const MAX_WEB_ACP_TIMELINE_PAGE_BYTES: usize = 1024 * 1024;
const MAX_WEB_ACP_TIMELINE_EVENT_BYTES: usize = MAX_WEB_ACP_TIMELINE_PAGE_BYTES;

fn web_acp_agent_id(agent_id: Option<String>) -> Result<String, String> {
    let backend = AgentBackend::parse(agent_id.as_deref().or(Some("codex")))
        .map_err(|error| format!("Unsupported Web ACP agent: {error:#}"))?;
    backend
        .agent_id()
        .map(str::to_owned)
        .ok_or_else(|| "Web code sessions support ACP agents only".to_string())
}

/// Stable error-code operations for `web_access_*` ACP commands. The wire
/// format `web_acp_{operation}_failed` is consumed by `acpErrors.js`;
/// `stable_web_error_codes_are_locked` pins the full list in Rust so a
/// rename fails on the Rust side instead of only in source-text regexes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebAcpOperation {
    SessionCreate,
    Cancel,
    Prompt,
    Timeline,
    SessionInfo,
    SetModel,
    SetMode,
    SetConfigOption,
    PendingPermissions,
    RespondPermission,
    PendingElicitations,
    RespondElicitation,
    ListSessions,
    ListAgents,
    AgentStatus,
}

impl WebAcpOperation {
    fn as_str(self) -> &'static str {
        match self {
            Self::SessionCreate => "session_create",
            Self::Cancel => "cancel",
            Self::Prompt => "prompt",
            Self::Timeline => "timeline",
            Self::SessionInfo => "session_info",
            Self::SetModel => "set_model",
            Self::SetMode => "set_mode",
            Self::SetConfigOption => "set_config_option",
            Self::PendingPermissions => "pending_permissions",
            Self::RespondPermission => "respond_permission",
            Self::PendingElicitations => "pending_elicitations",
            Self::RespondElicitation => "respond_elicitation",
            Self::ListSessions => "list_sessions",
            Self::ListAgents => "list_agents",
            Self::AgentStatus => "agent_status",
        }
    }
}

/// Stable error-code operations for `web_access_*` workspace commands, wire
/// format `web_workspace_{operation}_failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebWorkspaceOperation {
    Listing,
    Search,
    Preview,
    Changes,
    Diff,
}

impl WebWorkspaceOperation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Listing => "listing",
            Self::Search => "search",
            Self::Preview => "preview",
            Self::Changes => "changes",
            Self::Diff => "diff",
        }
    }
}

/// Stable error-code operations for generic `web_access_*` session commands,
/// wire format `web_session_{operation}_failed`. Native errors from the
/// session store may embed host paths; folding them keeps Relay responses
/// controlled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebSessionOperation {
    ListSessions,
    ListArchivedSessions,
    CreateSession,
    LoadSessionChunk,
}

impl WebSessionOperation {
    fn as_str(self) -> &'static str {
        match self {
            Self::ListSessions => "list_sessions",
            Self::ListArchivedSessions => "list_archived_sessions",
            Self::CreateSession => "create_session",
            Self::LoadSessionChunk => "load_session_chunk",
        }
    }
}

fn web_session_result<T, E: std::fmt::Display>(
    operation: WebSessionOperation,
    result: Result<T, E>,
) -> Result<T, String> {
    result.map_err(|error| {
        log::warn!(
            "[remote_control] Web session {} failed: {error}",
            operation.as_str()
        );
        format!("web_session_{}_failed", operation.as_str())
    })
}

fn web_workspace_result<T, E: std::fmt::Display>(
    operation: WebWorkspaceOperation,
    result: Result<T, E>,
) -> Result<T, String> {
    result.map_err(|error| {
        log::warn!(
            "[remote_control] Web code workspace {} failed: {error}",
            operation.as_str()
        );
        format!("web_workspace_{}_failed", operation.as_str())
    })
}

fn web_acp_result<T, E: std::fmt::Display>(
    operation: WebAcpOperation,
    result: Result<T, E>,
) -> Result<T, String> {
    result.map_err(|error| {
        log::warn!(
            "[remote_control] Web ACP {} failed: {error:#}",
            operation.as_str()
        );
        format!("web_acp_{}_failed", operation.as_str())
    })
}

fn require_main_webview(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("Web access bridge is restricted to the main desktop WebView".to_string())
    }
}

#[tauri::command]
pub fn web_access_enable(
    allow_host_workspace: Option<bool>,
    window: WebviewWindow,
    manager: State<'_, RemoteControlManager>,
) -> Result<WebAccessInfo, String> {
    require_main_webview(&window)?;
    manager.start(allow_host_workspace.unwrap_or(false))
}

#[tauri::command]
pub fn web_access_disable(manager: State<'_, RemoteControlManager>) -> Result<(), String> {
    manager.stop_current()
}

#[tauri::command]
pub fn web_access_status(
    manager: State<'_, RemoteControlManager>,
) -> Result<WebAccessStatus, String> {
    Ok(manager.status())
}

#[tauri::command]
pub fn web_access_rotate(
    manager: State<'_, RemoteControlManager>,
) -> Result<WebAccessInfo, String> {
    manager.refresh()
}

/// 桌面专属：查询/设置自定义 Relay 地址。均不进 Web 命令白名单——Relay 指向
/// 哪台服务器只能由桌面端决定。
#[tauri::command]
pub fn web_access_relay_settings(manager: State<'_, RemoteControlManager>) -> RelaySettingsInfo {
    manager.relay_settings()
}

#[tauri::command]
pub fn web_access_set_relay(
    address: String,
    manager: State<'_, RemoteControlManager>,
) -> Result<RelaySettingsInfo, String> {
    manager.set_relay_address(&address)
}

#[tauri::command]
pub fn web_access_reset_relay(
    manager: State<'_, RemoteControlManager>,
) -> Result<RelaySettingsInfo, String> {
    manager.reset_relay_address()
}

/// Desktop-only readiness handshake. It is intentionally absent from the Web
/// command allowlist.
#[tauri::command]
pub fn web_access_bridge_ready(
    generation: String,
    window: WebviewWindow,
    manager: State<'_, RemoteControlManager>,
) -> Result<(), String> {
    require_main_webview(&window)?;
    manager.mark_frontend_ready(generation)
}

/// Persist the dispatch ACK before the desktop WebView invokes any command.
/// This command is desktop-only and intentionally absent from the Web policy.
#[tauri::command]
pub fn web_access_rpc_begin(
    request_id: String,
    generation: String,
    window: WebviewWindow,
    manager: State<'_, RemoteControlManager>,
) -> Result<bool, String> {
    require_main_webview(&window)?;
    manager.begin_rpc(&request_id, &generation)
}

/// Complete an allowlisted RPC previously emitted to the authoritative
/// desktop WebView as `web_access:rpc_request`.
#[tauri::command]
pub fn web_access_rpc_respond(
    request_id: String,
    generation: String,
    ok: bool,
    result: Option<Value>,
    error: Option<String>,
    window: WebviewWindow,
    manager: State<'_, RemoteControlManager>,
) -> Result<(), String> {
    require_main_webview(&window)?;
    manager.complete_rpc(&request_id, &generation, ok, result, error)
}

/// Publish a subscribed application event that is only observable in the
/// desktop WebView layer. Engine and watcher events continue to use
/// `forward_app_event` directly and are de-duplicated by source.
#[tauri::command]
pub fn web_access_publish_event(
    event: String,
    payload: Value,
    window: WebviewWindow,
    manager: State<'_, RemoteControlManager>,
) -> Result<(), String> {
    require_main_webview(&window)?;
    manager.publish_frontend_event(&event, payload)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebHostFileListing {
    #[serde(flatten)]
    listing: file_access::HostFileListing,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace_handle: Option<String>,
}

/// Browse the desktop host filesystem for the WebUI file picker. Directory
/// mode may request an opaque workspace capability for the validated current
/// directory; native host paths are never accepted by ACP Web RPCs.
#[tauri::command]
pub fn web_access_list_host_files(
    path: Option<String>,
    issue_workspace_handle: Option<bool>,
    manager: State<'_, RemoteControlManager>,
) -> Result<WebHostFileListing, String> {
    let listing = file_access::list_host_files(path)?;
    let workspace_handle = issue_workspace_handle
        .unwrap_or(false)
        .then(|| manager.issue_web_workspace_grant(&listing.path))
        .transpose()?;
    Ok(WebHostFileListing {
        listing,
        workspace_handle,
    })
}

#[tauri::command]
pub async fn web_access_list_sessions(
    store: State<'_, SessionStore>,
    acp_pool: State<'_, AcpPool>,
) -> Result<Vec<super::sessions::SessionListItem>, String> {
    let mut sessions = web_session_result(
        WebSessionOperation::ListSessions,
        super::sessions::list_sessions(store, acp_pool).await,
    )?;
    for session in &mut sessions {
        session.metadata = super::codex::redact_session_metadata_for_web(session.metadata.clone());
    }
    Ok(sessions)
}

#[tauri::command]
pub async fn web_access_list_archived_sessions(
    store: State<'_, SessionStore>,
) -> Result<Vec<super::sessions::HiddenSessionListItem>, String> {
    let mut sessions = web_session_result(
        WebSessionOperation::ListArchivedSessions,
        super::sessions::list_archived_sessions(store).await,
    )?;
    for session in &mut sessions {
        session.metadata = super::codex::redact_session_metadata_for_web(session.metadata.clone());
    }
    Ok(sessions)
}

/// WebUI navigation owns an independent selected Session. These wrappers
/// intentionally never mutate the desktop process-wide `SessionStore.active`
/// pointer used by the native window.
#[tauri::command]
pub async fn web_access_create_session(
    app: AppHandle,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<WebSessionMetadata, String> {
    let metadata = web_session_result(
        WebSessionOperation::CreateSession,
        super::sessions::create_session(Some(false), None, app, store, pool).await,
    )?;
    let transcript_revision = crate::features::sessions::transcript_revision(&[])
        .map_err(|error| format!("create empty transcript revision: {error:#}"))?;
    Ok(WebSessionMetadata::project(metadata, transcript_revision))
}

/// Preserve the native `SessionMetadata` response shape while adding the
/// browser-only optimistic-concurrency token.
#[derive(Debug, Serialize)]
pub struct WebSessionMetadata {
    #[serde(flatten)]
    metadata: deepseek_tui::session_manager::SessionMetadata,
    transcript_revision: String,
}

impl WebSessionMetadata {
    fn project(
        metadata: deepseek_tui::session_manager::SessionMetadata,
        transcript_revision: String,
    ) -> Self {
        Self {
            metadata: super::codex::redact_session_metadata_for_web(metadata),
            transcript_revision,
        }
    }
}

/// Browser projection of a saved chat. Keep this allowlist explicit so newly
/// added desktop-only SavedSession fields cannot cross the Relay by default.
#[derive(Serialize)]
struct WebSavedSession<'a> {
    metadata: deepseek_tui::session_manager::SessionMetadata,
    messages: &'a [deepseek_tui::models::Message],
    artifacts: Vec<deepseek_tui::artifacts::ArtifactRecord>,
    transcript_revision: &'a str,
}

impl<'a> WebSavedSession<'a> {
    fn project(
        session: &'a deepseek_tui::session_manager::SavedSession,
        artifact_roots: &[PathBuf],
        transcript_revision: &'a str,
    ) -> Self {
        Self {
            metadata: super::codex::redact_session_metadata_for_web(session.metadata.clone()),
            messages: &session.messages,
            artifacts: session
                .artifacts
                .iter()
                .filter_map(|artifact| {
                    let storage_path = if artifact.storage_path.is_absolute()
                        || is_absolute_host_path(&artifact.storage_path.to_string_lossy())
                    {
                        artifact_roots.iter().find_map(|root| {
                            web_artifact_storage_path(&artifact.storage_path, root)
                        })?
                    } else {
                        safe_relative_web_path(&artifact.storage_path.to_string_lossy())?
                    };
                    let mut projected = artifact.clone();
                    projected.storage_path = storage_path;
                    Some(projected)
                })
                .collect(),
            transcript_revision,
        }
    }
}

fn is_absolute_host_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    Path::new(path).is_absolute()
        // `Path::is_absolute` follows the build host's syntax. A POSIX path
        // received from a saved cross-platform Session is therefore not
        // absolute on Windows unless we recognize its leading slash here.
        || path.starts_with('/')
        || path.starts_with("\\\\")
        || path.starts_with("//")
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

fn normalized_host_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let normalized = if normalized
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("//?/UNC/"))
    {
        format!("//{}", &normalized[8..])
    } else if let Some(stripped) = normalized.strip_prefix("//?/") {
        stripped.to_string()
    } else {
        normalized
    };
    if normalized == "/" {
        normalized
    } else {
        normalized.trim_end_matches('/').to_string()
    }
}

fn safe_relative_web_path(path: &str) -> Option<PathBuf> {
    if path.is_empty() || is_absolute_host_path(path) {
        return None;
    }
    let normalized = path.replace('\\', "/");
    let mut components = Vec::new();
    for component in normalized.split('/') {
        match component {
            "" | "." => {}
            ".." => return None,
            component => components.push(component),
        }
    }
    (!components.is_empty()).then(|| PathBuf::from(components.join("/")))
}

fn web_artifact_storage_path(storage_path: &Path, workspace: &Path) -> Option<PathBuf> {
    let storage_path = storage_path.to_string_lossy();
    if !is_absolute_host_path(&storage_path) {
        return safe_relative_web_path(&storage_path);
    }

    let workspace = workspace.to_string_lossy();
    if !is_absolute_host_path(&workspace) {
        return None;
    }
    let storage_path = normalized_host_path(&storage_path);
    let workspace = normalized_host_path(&workspace);
    let prefix = if workspace == "/" {
        "/".to_string()
    } else {
        format!("{workspace}/")
    };
    let windows_style = workspace.starts_with("//")
        || workspace
            .as_bytes()
            .get(1)
            .is_some_and(|separator| *separator == b':');
    let prefix_matches = storage_path.get(..prefix.len()).is_some_and(|candidate| {
        if windows_style {
            candidate.eq_ignore_ascii_case(&prefix)
        } else {
            candidate == prefix
        }
    });
    // Legacy absolute records are useful to WebUI only when they can be
    // represented by the same session-scoped relative artifact authority.
    prefix_matches
        .then(|| &storage_path[prefix.len()..])
        .and_then(safe_relative_web_path)
}

#[derive(Debug, Serialize)]
pub struct SessionDataChunk {
    pub download_id: String,
    pub offset: u64,
    pub total: u64,
    pub data_base64: String,
    pub eof: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_revision: Option<String>,
}

/// Serialize a Session on the desktop and transfer it in bounded chunks. A
/// full SavedSession can exceed Relay's frame ceiling after a long history.
///
/// Every error path folds into the stable `web_session_load_session_chunk_failed`
/// code: the native chain (store errors embed host paths, e.g. the sessions
/// directory) goes to the desktop log only.
#[tauri::command]
pub async fn web_access_load_session_chunk(
    id: String,
    download_id: Option<String>,
    requested_download_id: Option<String>,
    offset: u64,
    limit: Option<usize>,
    manager: State<'_, RemoteControlManager>,
    store: State<'_, SessionStore>,
) -> Result<SessionDataChunk, String> {
    web_session_result(
        WebSessionOperation::LoadSessionChunk,
        load_session_chunk_inner(
            id,
            download_id,
            requested_download_id,
            offset,
            limit,
            manager,
            store,
        )
        .await,
    )
}

async fn load_session_chunk_inner(
    id: String,
    download_id: Option<String>,
    requested_download_id: Option<String>,
    offset: u64,
    limit: Option<usize>,
    manager: State<'_, RemoteControlManager>,
    store: State<'_, SessionStore>,
) -> Result<SessionDataChunk, String> {
    let started = std::time::Instant::now();
    crate::features::sessions::validate_session_id(&id)
        .map_err(|error| format!("invalid Session id: {error:#}"))?;
    let limit = limit.unwrap_or(manager::MAX_SESSION_CHUNK_BYTES);
    if limit == 0 || limit > manager::MAX_SESSION_CHUNK_BYTES {
        return Err(format!(
            "Session chunk limit must be between 1 and {}",
            manager::MAX_SESSION_CHUNK_BYTES
        ));
    }
    let offset = usize::try_from(offset).map_err(|_| "Session offset is too large".to_string())?;
    let (download_id, transcript_revision) = match download_id {
        Some(download_id) if !download_id.trim().is_empty() => (download_id, None),
        _ => {
            if offset != 0 {
                return Err("Session download id is required after the first chunk".into());
            }
            let persisted_size = store
                .persisted_size(&id)
                .map_err(|error| format!("inspect Session {id}: {error:#}"))?;
            let persisted_size = usize::try_from(persisted_size)
                .map_err(|_| "Session payload is too large for this platform".to_string())?;
            let reserved_bytes = persisted_size
                .checked_add(WEB_SESSION_SERIALIZATION_HEADROOM)
                .ok_or_else(|| "Session payload size overflow".to_string())?;
            let reservation = manager.begin_web_session_download(
                &id,
                requested_download_id.as_deref(),
                reserved_bytes,
            )?;
            let output_file = reservation.try_clone_writer()?;
            let store = store.inner().clone();
            let session_id = id.clone();
            let revision = tokio::task::spawn_blocking(move || -> Result<String, String> {
                let saved = store
                    .load(&session_id)
                    .map_err(|error| format!("load Session {session_id}: {error:#}"))?;
                let revision = crate::features::sessions::transcript_revision(&saved.messages)
                    .map_err(|error| {
                        format!("compute Session {session_id} transcript revision: {error:#}")
                    })?;
                let artifact_roots = [
                    store
                        .ledger_root(&session_id)
                        .map_err(|error| format!("resolve Session workspace ledger: {error:#}"))?,
                    crate::platform::paths::session_artifacts_dir(&session_id),
                ];
                let mut writer = BufWriter::new(output_file);
                serde_json::to_writer(
                    &mut writer,
                    &WebSavedSession::project(&saved, &artifact_roots, &revision),
                )
                .map_err(|error| format!("serialize Session {session_id}: {error}"))?;
                writer
                    .flush()
                    .map_err(|error| format!("flush serialized Session {session_id}: {error}"))?;
                Ok(revision)
            })
            .await
            .map_err(|error| format!("prepare Session {id} download task: {error}"))??;
            (reservation.commit()?, Some(revision))
        }
    };
    let chunk = manager.read_web_session_download(&download_id, &id, offset, limit)?;
    if offset == 0 || chunk.eof {
        crate::features::sessions::diagnostics::record_backend(
            "web_session_chunk_served",
            serde_json::json!({
                "session_id": id,
                "download_id": download_id,
                "offset": offset,
                "chunk_bytes": chunk.data.len(),
                "total_bytes": chunk.total,
                "eof": chunk.eof,
                "transcript_revision": transcript_revision,
                "elapsed_ms": started.elapsed().as_millis(),
            }),
        );
    }
    Ok(SessionDataChunk {
        download_id,
        offset: offset as u64,
        total: chunk.total as u64,
        data_base64: base64::engine::general_purpose::STANDARD.encode(chunk.data),
        eof: chunk.eof,
        transcript_revision,
    })
}

/// Idempotently release a browser Session download that did not reach EOF.
/// This is a separate bounded command because a dropped Relay connection can
/// leave the desktop-side temporary file alive after the browser has stopped
/// requesting chunks.
#[tauri::command]
pub fn web_access_cancel_session_download(
    id: String,
    download_id: String,
    manager: State<'_, RemoteControlManager>,
) -> Result<bool, String> {
    crate::features::sessions::validate_session_id(&id)
        .map_err(|error| format!("invalid Session id: {error:#}"))?;
    if download_id.len() < 8
        || download_id.len() > 128
        || !download_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("invalid Session download id".into());
    }
    manager.cancel_web_session_download(&download_id, &id)
}

/// Parse a desktop-host file without returning its extracted markdown to the
/// browser. The opaque handle is resolved only by `web_access_chat`.
#[tauri::command]
pub async fn web_access_ingest_file(
    path: String,
    manager: State<'_, RemoteControlManager>,
) -> Result<manager::WebAttachmentSummary, String> {
    let result = super::files::ingest_file(path).await?;
    manager.cache_web_attachment(result)
}

/// Receive one bounded chunk of a browser-device file. Relay only forwards the
/// chunk frames; on the final `commit` the bytes land in a short-lived staging
/// directory, run through the shared ingest pipeline, and come back as the same
/// opaque `WebAttachmentSummary` the host-file flow returns. Nothing survives
/// past attachment consumption, eviction, or endpoint teardown.
#[tauri::command]
pub async fn web_access_upload_attachment_chunk(
    upload_id: String,
    file_name: String,
    offset: usize,
    total: usize,
    data_base64: String,
    commit: bool,
    sha256: Option<String>,
    manager: State<'_, RemoteControlManager>,
) -> Result<Option<manager::WebAttachmentSummary>, String> {
    let data = base64::engine::general_purpose::STANDARD
        .decode(data_base64)
        .map_err(|error| format!("decode attachment upload chunk: {error}"))?;
    if data.len() > MAX_TRANSFER_CHUNK_BYTES {
        return Err("attachment upload chunk exceeds 256 KiB".into());
    }
    let Some((file_name, bytes)) = manager.append_web_attachment_upload(
        &upload_id,
        &file_name,
        offset,
        total,
        &data,
        commit,
        sha256.as_deref(),
    )?
    else {
        return Ok(None);
    };
    let staging_dir = manager::web_attachment_uploads_base().join(format!(
        "{}{}",
        manager::WEB_ATTACHMENT_UPLOAD_DIR_PREFIX,
        crate::features::remote_control::short_token(24)
    ));
    let ingest_dir = staging_dir.clone();
    let ingested = tokio::task::spawn_blocking(move || -> Result<_, String> {
        std::fs::create_dir_all(&ingest_dir)
            .map_err(|error| format!("create attachment upload dir: {error}"))?;
        let path = ingest_dir
            .join(crate::features::files::file_ingest::sanitize_upload_filename(&file_name));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("write uploaded attachment: {error}"))?;
        file.write_all(&bytes)
            .map_err(|error| format!("write uploaded attachment: {error}"))?;
        crate::features::files::file_ingest::ingest_attachment(&path)
    })
    .await
    .map_err(|error| format!("ingest uploaded attachment task: {error}"))
    .and_then(std::convert::identity);
    ingested
        .and_then(|result| manager.cache_web_attachment_from_upload(result, staging_dir.clone()))
        .map(Some)
        .inspect_err(|_| {
            let _ = std::fs::remove_dir_all(&staging_dir);
        })
}

/// Cancel an in-progress browser-device upload and release its desktop buffer.
/// Idempotent so a late cancel after commit or expiry stays silent.
#[tauri::command]
pub fn web_access_abort_attachment_upload(
    upload_id: String,
    manager: State<'_, RemoteControlManager>,
) -> Result<(), String> {
    manager.abort_web_attachment_upload(&upload_id);
    Ok(())
}

/// Release a parsed attachment after the browser removes its chip. If a chat
/// turn already reserved the handle, cleanup is deferred until that turn
/// finishes so the engine keeps a stable source path.
#[tauri::command]
pub fn web_access_discard_attachment(
    handle: String,
    manager: State<'_, RemoteControlManager>,
) -> Result<(), String> {
    manager.discard_web_attachment(&handle)
}

/// Read a bounded chunk from an attachment already referenced by a saved
/// conversation message. The browser never receives or supplies a host path.
#[tauri::command]
pub async fn web_access_read_conversation_attachment_chunk(
    session_id: String,
    message_index: usize,
    attachment_index: usize,
    basename: String,
    display_text: String,
    offset: u64,
    limit: Option<usize>,
    store: State<'_, SessionStore>,
) -> Result<file_access::ArtifactChunk, String> {
    let path = super::files::resolve_conversation_attachment_path(
        &store,
        &session_id,
        message_index,
        attachment_index,
        &basename,
        &display_text,
    )?;
    file_access::read_resolved_file_chunk(&path, offset, limit)
}

/// Materialize a draft Web conversation and admit its first turn as one
/// idempotent RPC. The Relay request ledger guarantees that reconnecting with
/// the same client_request_id cannot create or submit the turn twice.
#[tauri::command]
pub async fn web_access_create_session_and_chat(
    message: String,
    attachment_handles: Option<Vec<String>>,
    restrict_tools: Option<bool>,
    manager: State<'_, RemoteControlManager>,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<WebSessionMetadata, String> {
    let transcript_revision = crate::features::sessions::transcript_revision(&[])
        .map_err(|error| format!("create empty transcript revision: {error:#}"))?;
    let metadata = super::sessions::create_session_record(false, &store, &pool, None)?;
    let session_id = metadata.id.clone();

    if let Err(error) = web_access_chat_for_session(
        message,
        attachment_handles,
        session_id.clone(),
        restrict_tools,
        &manager,
        &pool,
        &store,
        &app,
    )
    .await
    {
        pool.evict(&session_id).await;
        let rollback = store
            .delete(&session_id)
            .map_err(|rollback_error| format!("rollback Session {session_id}: {rollback_error:#}"));
        pool.forget_session(&session_id);
        // The chat may have already run (possibly past start_turn):
        // process-level residual keys such as the timing queue / pending
        // user input are cleaned uniformly by the SessionPurgedHook fired
        // from store.delete, matching the delete_session command.
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
        };
    }

    super::sessions::emit_session_event(&app, "session:list_changed", &session_id, "created");
    Ok(WebSessionMetadata::project(metadata, transcript_revision))
}

/// Web-safe chat entry point: Session routing is mandatory and attachment
/// contents never cross Relay in either direction.
#[tauri::command]
pub async fn web_access_chat(
    message: String,
    attachment_handles: Option<Vec<String>>,
    session_id: String,
    restrict_tools: Option<bool>,
    manager: State<'_, RemoteControlManager>,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<(), String> {
    web_access_chat_for_session(
        message,
        attachment_handles,
        session_id,
        restrict_tools,
        &manager,
        &pool,
        &store,
        &app,
    )
    .await
}

/// Create a Web code Session from either a private temporary workspace or a
/// one-shot capability minted by the host-file picker. The browser never gets
/// to submit a native workspace path to this command.
#[tauri::command]
pub async fn web_access_create_codex_acp_session(
    workspace_handle: Option<String>,
    agent_id: Option<String>,
    manager: State<'_, RemoteControlManager>,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
    acp_pool: State<'_, AcpPool>,
) -> Result<deepseek_tui::session_manager::SessionMetadata, String> {
    let outcome = async {
        // Validate before consuming the one-shot workspace grant. A malformed
        // or desktop-only request must not burn a capability that the user selected.
        let agent_id = web_acp_agent_id(agent_id)?;
        let workspace_reservation = workspace_handle
            .as_deref()
            .map(|handle| manager.reserve_web_workspace_grant(handle))
            .transpose()?;
        let workspace_path = workspace_reservation
            .as_ref()
            .map(|reservation| reservation.path().to_path_buf());
        let result = {
            let verify_workspace = |path: &Path| {
                workspace_reservation
                    .as_ref()
                    .ok_or_else(|| "Web workspace authorization is missing".to_string())?
                    .verify_bound_path(path)
            };
            let workspace_verifier = workspace_reservation.as_ref().map(|_| {
                &verify_workspace as &(dyn Fn(&Path) -> Result<(), String> + Send + Sync)
            });
            super::codex::create_codex_acp_session_with_workspace_binding(
                workspace_path,
                Some(agent_id),
                store,
                pool,
                acp_pool,
                workspace_verifier,
            )
            .await
        };
        let metadata = match result {
            Ok(metadata) => metadata,
            Err(error) => {
                if workspace_reservation
                    .map(|reservation| manager.restore_web_workspace_grant(reservation))
                    .is_some_and(|restored| !restored)
                {
                    log::warn!(
                        "[remote_control] failed to restore Web workspace authorization after Session creation failure"
                    );
                }
                return Err(error);
            }
        };
        Ok(super::codex::redact_session_metadata_for_web(metadata))
    }
    .await;
    web_acp_result(WebAcpOperation::SessionCreate, outcome)
}

/// Session-bound Web workspace reads. Draft browsing is intentionally owned
/// by the host picker; once a code Session exists, its validated Session id is
/// the only authority used to resolve the workspace root.
#[tauri::command]
pub async fn web_access_list_codex_workspace(
    session_id: String,
    relative_path: Option<String>,
    acp_pool: State<'_, AcpPool>,
) -> Result<WorkspaceListing, String> {
    let result =
        super::codex::list_codex_workspace(Some(session_id), relative_path, None, acp_pool).await;
    web_workspace_result(WebWorkspaceOperation::Listing, result)
}

#[tauri::command]
pub async fn web_access_search_codex_workspace(
    session_id: String,
    query: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<Vec<WorkspaceEntry>, String> {
    let result =
        super::codex::search_codex_workspace(Some(session_id), query, None, acp_pool).await;
    web_workspace_result(WebWorkspaceOperation::Search, result)
}

#[tauri::command]
pub async fn web_access_preview_codex_workspace_file(
    session_id: String,
    relative_path: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<WorkspacePreview, String> {
    let result =
        super::codex::preview_codex_workspace_file(Some(session_id), relative_path, None, acp_pool)
            .await;
    web_workspace_result(WebWorkspaceOperation::Preview, result)
}

#[tauri::command]
pub async fn web_access_get_codex_workspace_changes(
    session_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<WorkspaceChanges, String> {
    let result = super::codex::get_codex_workspace_changes(session_id, acp_pool).await;
    web_workspace_result(WebWorkspaceOperation::Changes, result)
}

#[tauri::command]
pub async fn web_access_get_codex_workspace_diff(
    session_id: String,
    relative_path: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<WorkspaceDiff, String> {
    let result = super::codex::get_codex_workspace_diff(session_id, relative_path, acp_pool).await;
    web_workspace_result(WebWorkspaceOperation::Diff, result)
}

#[tauri::command]
pub async fn web_access_cancel_codex_acp(
    session_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<(), String> {
    let result = super::codex::cancel_codex_acp(session_id, acp_pool).await;
    web_acp_result(WebAcpOperation::Cancel, result)
}

/// Web-safe ACP prompt entry point. Browser and host-picked attachments are
/// represented only by one-shot opaque handles; native paths and parsed
/// contents never cross Relay. The underlying ACP command remains the single
/// implementation of title, workspace-reference, and turn semantics.
#[tauri::command]
pub async fn web_access_codex_acp_prompt(
    session_id: String,
    message: String,
    attachment_handles: Option<Vec<String>>,
    workspace_references: Option<Vec<String>>,
    manager: State<'_, RemoteControlManager>,
    store: State<'_, SessionStore>,
    acp_pool: State<'_, AcpPool>,
) -> Result<(), String> {
    let outcome = async {
        crate::features::sessions::validate_session_id(&session_id)
            .map_err(|error| format!("invalid ACP Session id: {error:#}"))?;
        store
            .load(&session_id)
            .map_err(|error| format!("load ACP Session {session_id}: {error:#}"))?;
        if !acp_pool.is_acp(&session_id) {
            return Err("当前会话不是 ACP 会话".to_string());
        }

        let attachment_handles = attachment_handles.unwrap_or_default();
        let (attachment_reservation, attachments) =
            manager.reserve_web_attachments(&attachment_handles)?;
        let (attachments, staged_sources) = match stage_uploaded_attachments(
            attachments,
            &session_id,
            &store,
        ) {
            Ok(staged) => staged,
            Err(error) => {
                if let Err(release_error) = manager.finish_web_attachment_reservation(
                    &attachment_reservation,
                    &attachment_handles,
                    false,
                ) {
                    eprintln!(
                        "[web-access] release ACP attachment reservation failed: {release_error}"
                    );
                }
                return Err(error);
            }
        };
        let result = super::codex::codex_acp_prompt_with_attachments(
            session_id,
            message,
            attachments,
            workspace_references.unwrap_or_default(),
            &store,
            &acp_pool,
        )
        .await;
        if result.is_err() {
            cleanup_staged_attachment_sources(&staged_sources);
        }
        let consume = result.is_ok();
        if let Err(error) = manager.finish_web_attachment_reservation(
            &attachment_reservation,
            &attachment_handles,
            consume,
        ) {
            if consume {
                eprintln!("[web-access] finalize accepted ACP attachments failed: {error}");
            } else {
                return Err(format!(
                    "{}; additionally failed to release attachments: {error}",
                    result
                        .as_ref()
                        .err()
                        .cloned()
                        .unwrap_or_else(|| "ACP prompt submission failed".to_string())
                ));
            }
        }
        result
    }
    .await;
    web_acp_result(WebAcpOperation::Prompt, outcome)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebAcpTimelinePage {
    events: Vec<AcpEventEnvelope>,
    next_after_seq: Option<u64>,
    next_cursor: Option<u64>,
    has_more: bool,
}

/// Return one bounded page of the authoritative ACP timeline after projecting
/// out desktop-only adapter metadata and credential-bearing diagnostic fields.
#[tauri::command]
pub fn web_access_get_codex_acp_timeline(
    session_id: String,
    after_seq: Option<u64>,
    after_cursor: Option<u64>,
    limit: Option<usize>,
    acp_pool: State<'_, AcpPool>,
) -> Result<WebAcpTimelinePage, String> {
    let outcome = (|| {
        let limit = limit.unwrap_or(DEFAULT_WEB_ACP_TIMELINE_PAGE_EVENTS);
        if limit == 0 || limit > MAX_WEB_ACP_TIMELINE_PAGE_EVENTS {
            return Err(format!(
                "Web ACP timeline page limit must be between 1 and {MAX_WEB_ACP_TIMELINE_PAGE_EVENTS}"
            ));
        }
        let page = acp_pool
            .web_timeline_page(
                &session_id,
                after_seq.unwrap_or(0),
                after_cursor,
                limit,
                MAX_WEB_ACP_TIMELINE_PAGE_BYTES,
                MAX_WEB_ACP_TIMELINE_EVENT_BYTES,
            )
            .map_err(|error| format!("{error:#}"))?;
        Ok(WebAcpTimelinePage {
            next_after_seq: page.events.last().map(|event| event.seq),
            next_cursor: page.next_cursor,
            has_more: page.has_more,
            events: page.events,
        })
    })();
    web_acp_result(WebAcpOperation::Timeline, outcome)
}

fn project_acp_pending_permission_for_web(
    mut pending: CodexAcpPendingPermission,
) -> CodexAcpPendingPermission {
    pending.request = project_acp_permission_request_for_web(pending.request);
    pending
}

fn project_acp_pending_elicitation_for_web(
    mut pending: CodexAcpPendingElicitation,
) -> CodexAcpPendingElicitation {
    pending.request = project_acp_elicitation_request_for_web(pending.request);
    pending
}

fn project_acp_session_info_for_web(mut info: CodexAcpSessionInfo) -> anyhow::Result<Value> {
    info.pending_permissions = info
        .pending_permissions
        .into_iter()
        .map(project_acp_pending_permission_for_web)
        .collect();
    info.pending_elicitations = info
        .pending_elicitations
        .into_iter()
        .map(project_acp_pending_elicitation_for_web)
        .collect();
    Ok(project_acp_value_for_web(serde_json::to_value(info)?))
}

#[tauri::command]
pub async fn web_access_get_codex_acp_session_info(
    session_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<Value, String> {
    let outcome = acp_pool
        .session_info(&session_id)
        .await
        .and_then(project_acp_session_info_for_web);
    web_acp_result(WebAcpOperation::SessionInfo, outcome)
}

#[tauri::command]
pub async fn web_access_set_codex_acp_model(
    session_id: String,
    model_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<Value, String> {
    let outcome = acp_pool
        .set_model(&session_id, &model_id)
        .await
        .and_then(project_acp_session_info_for_web);
    web_acp_result(WebAcpOperation::SetModel, outcome)
}

#[tauri::command]
pub async fn web_access_set_codex_acp_mode(
    session_id: String,
    mode_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<Value, String> {
    let outcome = acp_pool
        .set_mode(&session_id, &mode_id)
        .await
        .and_then(project_acp_session_info_for_web);
    web_acp_result(WebAcpOperation::SetMode, outcome)
}

#[tauri::command]
pub async fn web_access_set_codex_acp_config_option(
    session_id: String,
    config_id: String,
    value_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<Value, String> {
    let outcome = acp_pool
        .set_config_option(&session_id, &config_id, &value_id)
        .await
        .and_then(project_acp_session_info_for_web);
    web_acp_result(WebAcpOperation::SetConfigOption, outcome)
}

#[tauri::command]
pub async fn web_access_get_codex_acp_pending_permissions(
    session_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<Vec<CodexAcpPendingPermission>, String> {
    let outcome = async {
        if !acp_pool.is_acp(&session_id) {
            return Err("当前会话不是 ACP 会话".to_string());
        }
        Ok(acp_pool
            .pending_permissions_for(&session_id)
            .await
            .into_iter()
            .map(project_acp_pending_permission_for_web)
            .collect())
    }
    .await;
    web_acp_result(WebAcpOperation::PendingPermissions, outcome)
}

#[tauri::command]
pub async fn web_access_respond_codex_acp_permission(
    session_id: String,
    tool_call_id: String,
    option_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<(), String> {
    let outcome = acp_pool
        .respond_permission(&session_id, &tool_call_id, &option_id)
        .await;
    web_acp_result(WebAcpOperation::RespondPermission, outcome)
}

#[tauri::command]
pub async fn web_access_get_codex_acp_pending_elicitations(
    session_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<Vec<CodexAcpPendingElicitation>, String> {
    let outcome = async {
        if !acp_pool.is_acp(&session_id) {
            return Err("当前会话不是 ACP 会话".to_string());
        }
        Ok(acp_pool
            .pending_elicitations_for(&session_id)
            .await
            .into_iter()
            .map(project_acp_pending_elicitation_for_web)
            .collect())
    }
    .await;
    web_acp_result(WebAcpOperation::PendingElicitations, outcome)
}

#[tauri::command]
pub async fn web_access_respond_codex_acp_elicitation(
    session_id: String,
    elicitation_id: String,
    action: String,
    content: Value,
    acp_pool: State<'_, AcpPool>,
) -> Result<(), String> {
    let outcome = acp_pool
        .respond_elicitation(&session_id, &elicitation_id, &action, content)
        .await;
    web_acp_result(WebAcpOperation::RespondElicitation, outcome)
}

fn project_acp_status_for_web(
    mut status: crate::features::codex_acp::CodexAcpStatus,
) -> crate::features::codex_acp::CodexAcpStatus {
    status.adapter_path = None;
    status.codex_path = None;
    status.login_url = None;
    status.login_code = None;
    status.error = None;
    // 安装命令行与输出可能含内部镜像源或主机路径，仅桌面端展示。
    status.install_command = None;
    status.install_latest_line = None;
    status
}

#[tauri::command]
pub async fn web_access_list_codex_acp_sessions(
    store: State<'_, SessionStore>,
    acp_pool: State<'_, AcpPool>,
) -> Result<Vec<crate::app::commands::codex::CodexAcpSessionListItem>, String> {
    let outcome = super::codex::list_codex_acp_sessions_for_web(&store, &acp_pool).await;
    web_acp_result(WebAcpOperation::ListSessions, outcome)
}

#[tauri::command]
pub async fn web_access_list_acp_agents(
    acp_pool: State<'_, AcpPool>,
) -> Result<Vec<crate::features::codex_acp::AcpAgentDescriptor>, String> {
    let outcome = super::codex::list_acp_agents_for_pool(&acp_pool).await;
    web_acp_result(WebAcpOperation::ListAgents, outcome)
}

#[tauri::command]
pub async fn web_access_get_acp_agent_status(
    agent_id: String,
    recheck: Option<bool>,
    acp_pool: State<'_, AcpPool>,
) -> Result<crate::features::codex_acp::CodexAcpStatus, String> {
    let outcome = super::codex::get_acp_agent_status_for_pool(agent_id, recheck, &acp_pool)
        .await
        .map(project_acp_status_for_web);
    web_acp_result(WebAcpOperation::AgentStatus, outcome)
}

async fn web_access_chat_for_session(
    message: String,
    attachment_handles: Option<Vec<String>>,
    session_id: String,
    restrict_tools: Option<bool>,
    manager: &RemoteControlManager,
    pool: &EnginePool,
    store: &SessionStore,
    app: &AppHandle,
) -> Result<(), String> {
    crate::features::sessions::validate_session_id(&session_id)
        .map_err(|error| format!("invalid Session id: {error:#}"))?;
    ensure_web_chat_session_supported(store.mode_state(&session_id).multi_agent)?;
    store
        .load(&session_id)
        .map_err(|error| format!("load Session {session_id}: {error:#}"))?;
    // Admit the turn before resolving any opaque attachment handles. This
    // guarantees that a competing desktop/Web submission cannot consume the
    // browser's one-shot attachment reservation or other per-turn state.
    let turn_reservation = pool
        .reserve_turn(&session_id)
        .map_err(|error| format!("reserve Web chat turn: {error:#}"))?;
    let attachment_handles = attachment_handles.unwrap_or_default();
    let (attachment_reservation, attachments) =
        manager.reserve_web_attachments(&attachment_handles)?;
    let (attachments, staged_sources) =
        match stage_uploaded_attachments(attachments, &session_id, store) {
            Ok(staged) => staged,
            Err(error) => {
                if let Err(release_error) = manager.finish_web_attachment_reservation(
                    &attachment_reservation,
                    &attachment_handles,
                    false,
                ) {
                    eprintln!(
                        "[web-access] release staged attachment reservation failed: {release_error}"
                    );
                }
                return Err(error);
            }
        };
    let result = super::chat::chat_with_reservation(
        message,
        Some(attachments),
        session_id,
        restrict_tools,
        turn_reservation,
        pool,
        store,
        app,
    )
    .await;
    if result.is_err() {
        cleanup_staged_attachment_sources(&staged_sources);
    }
    let consume = result.is_ok();
    if let Err(error) = manager.finish_web_attachment_reservation(
        &attachment_reservation,
        &attachment_handles,
        consume,
    ) {
        if consume {
            // The engine already accepted the turn. Never report a false
            // failure that could cause the browser to submit it again.
            eprintln!("[web-access] finalize accepted attachment reservation failed: {error}");
        } else {
            return Err(format!(
                "{}; additionally failed to release attachments: {error}",
                result
                    .as_ref()
                    .err()
                    .cloned()
                    .unwrap_or_else(|| "chat submission failed".to_string())
            ));
        }
    }
    result
}

/// Web 端只能续写未开启多智能体模式的会话。Web 界面没有专家卡/只读面板，
/// 放行会让浏览器启动它看不到、也停不掉的子智能体。
fn ensure_web_chat_session_supported(multi_agent: bool) -> Result<(), String> {
    if multi_agent {
        return Err(
            "multi-agent sessions are desktop-only; continue them in the desktop app".to_string(),
        );
    }
    Ok(())
}

/// 浏览器本机上传的附件落在短生命周期暂存目录里。引擎会按 `IngestResult.path`
/// 再次读取文件（图片拷贝进 workspace、超长文本注入路径供 `read_file`），因此
/// 在把路径交给引擎之前先复制进该会话的 workspace 并改写 path，暂存目录的
/// 清理就永远不会与后续读取竞争。桌面主机文件附件的路径原样保留。
fn stage_uploaded_attachments(
    mut attachments: Vec<crate::features::files::file_ingest::IngestResult>,
    session_id: &str,
    store: &SessionStore,
) -> Result<
    (
        Vec<crate::features::files::file_ingest::IngestResult>,
        Vec<std::path::PathBuf>,
    ),
    String,
> {
    let uploads_base = manager::web_attachment_uploads_base();
    if attachments
        .iter()
        .all(|attachment| !std::path::Path::new(&attachment.path).starts_with(&uploads_base))
    {
        return Ok((attachments, Vec::new()));
    }
    let workspace = store
        .ledger_root(session_id)
        .map_err(|error| format!("resolve attachment workspace: {error:#}"))?;
    let mut staged_sources = Vec::new();
    for attachment in &mut attachments {
        if !std::path::Path::new(&attachment.path).starts_with(&uploads_base) {
            continue;
        }
        let Some(staged) = super::attachments::stage_remote_attachment_source(
            &attachment.path,
            &attachment.basename,
            &workspace,
        ) else {
            cleanup_staged_attachment_sources(&staged_sources);
            return Err(format!("暂存远程上传附件失败：{}", attachment.basename));
        };
        attachment.path = staged.to_string_lossy().into_owned();
        staged_sources.push(staged);
    }
    Ok((attachments, staged_sources))
}

fn cleanup_staged_attachment_sources(paths: &[std::path::PathBuf]) {
    for path in paths {
        if let Err(error) = std::fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "[web-access] cleanup rejected staged attachment {} failed: {error}",
                    path.display()
                );
            }
        }
    }
}

/// Persist a potentially large Web transcript through bounded upload chunks.
/// The final chunk is decoded into the native Message schema and committed by
/// the SessionStore's content-revision CAS.
#[tauri::command]
pub async fn web_access_save_session_messages_chunk(
    id: String,
    upload_id: String,
    expected_revision: String,
    offset: usize,
    total: usize,
    data_base64: String,
    commit: bool,
    manager: State<'_, RemoteControlManager>,
    store: State<'_, SessionStore>,
) -> Result<Option<String>, String> {
    crate::features::sessions::validate_session_id(&id)
        .map_err(|error| format!("invalid Session id: {error:#}"))?;
    if offset == 0 {
        store
            .load(&id)
            .map_err(|error| format!("load Session {id}: {error:#}"))?;
    }
    let data = base64::engine::general_purpose::STANDARD
        .decode(data_base64)
        .map_err(|error| format!("decode Session upload chunk: {error}"))?;
    if data.len() > MAX_TRANSFER_CHUNK_BYTES {
        return Err("Session upload chunk exceeds 256 KiB".into());
    }
    let completed = manager.append_web_session_upload(
        &upload_id,
        &id,
        &expected_revision,
        offset,
        total,
        &data,
        commit,
    )?;
    if let Some(payload) = completed {
        let messages = serde_json::from_slice(&payload)
            .map_err(|error| format!("parse uploaded Session messages: {error}"))?;
        let revision = store
            .compare_and_swap_messages(&id, &expected_revision, messages)
            .map_err(|error| format!("save Session {id} transcript: {error:#}"))?;
        return Ok(Some(revision));
    }
    Ok(None)
}

/// Base64 keeps a normal 10-second WAV comfortably below the RPC envelope;
/// the raw desktop command's JSON byte array is several times larger.
#[tauri::command]
pub async fn web_access_transcribe_voice_audio(
    audio_base64: String,
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<super::voice::VoiceTranscriptionResponse, super::voice::VoiceCommandError> {
    crate::features::sessions::validate_session_id(&session_id).map_err(|error| {
        super::voice::VoiceCommandError::new(
            "context_mismatch",
            "transcribing",
            format!("invalid Session id: {error:#}"),
        )
    })?;
    store.load(&session_id).map_err(|error| {
        super::voice::VoiceCommandError::new(
            "context_mismatch",
            "transcribing",
            format!("load Session {session_id}: {error:#}"),
        )
    })?;
    let audio_bytes = base64::engine::general_purpose::STANDARD
        .decode(audio_base64)
        .map_err(|error| {
            super::voice::VoiceCommandError::new(
                "recording_failed",
                "transcribing",
                format!("解码远程控制语音音频失败：{error}"),
            )
        })?;
    if audio_bytes.len() > 1024 * 1024 {
        return Err(super::voice::VoiceCommandError::new(
            "recording_failed",
            "transcribing",
            "远程控制语音音频超过 1 MiB",
        ));
    }
    super::voice::transcribe_voice_audio(super::voice::VoiceTranscriptionRequest { audio_bytes })
        .await
}

/// Read a bounded chunk from a Session-owned artifact. The resolver rejects
/// files outside that Session's isolated workspace/artifact authority.
#[tauri::command]
pub fn web_access_read_artifact_chunk(
    path: String,
    session_id: String,
    offset: u64,
    limit: Option<usize>,
    store: State<'_, SessionStore>,
) -> Result<file_access::ArtifactChunk, String> {
    file_access::read_artifact_chunk(&store, &path, &session_id, offset, limit)
}

/// Merge only settings that remain visible and meaningful in WebUI. Hidden
/// desktop authority (shell, model bootstrap, pet, notifications, theme and
/// language) always stays sourced from the current desktop preferences.
#[tauri::command]
pub async fn web_access_update_settings(
    patch: super::settings::WebSettingsPatch,
) -> Result<UserPrefs, String> {
    super::settings::persist_web_settings(patch)
}

fn scoped_artifact_path(
    store: &SessionStore,
    session_id: &str,
    path: &str,
) -> Result<String, String> {
    file_access::resolve_session_artifact_path(store, session_id, path)
        .map(|resolved| resolved.to_string_lossy().into_owned())
}

fn ensure_web_artifact_file_size(path: &str, max_bytes: u64) -> Result<(), String> {
    let size = std::fs::metadata(path)
        .map_err(|error| format!("stat Web artifact {path}: {error}"))?
        .len();
    if size > max_bytes {
        return Err("产物过大，无法在远程控制中预览，请改为下载".to_string());
    }
    Ok(())
}

fn ensure_web_artifact_response<T: Serialize>(value: T) -> Result<T, String> {
    let size = serde_json::to_vec(&value)
        .map_err(|error| format!("serialize Web artifact preview: {error}"))?
        .len();
    if size > MAX_WEB_ARTIFACT_RPC_BYTES {
        return Err("产物预览超过远程控制响应上限，请改为下载".to_string());
    }
    Ok(value)
}

#[tauri::command]
pub fn web_access_artifact_info(
    path: String,
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<super::artifacts::ArtifactInfo, String> {
    let resolved = scoped_artifact_path(&store, &session_id, &path)?;
    super::artifacts::artifact_info_impl(&resolved)
}

#[tauri::command]
pub fn web_access_read_artifact_text(
    path: String,
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<String, String> {
    let resolved = scoped_artifact_path(&store, &session_id, &path)?;
    ensure_web_artifact_file_size(&resolved, MAX_WEB_ARTIFACT_RPC_BYTES as u64)?;
    ensure_web_artifact_response(super::artifacts::read_artifact_text_impl(&resolved)?)
}

#[tauri::command]
pub fn web_access_write_artifact_text(
    path: String,
    content: String,
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    if content.len() > MAX_WEB_ARTIFACT_RPC_BYTES {
        return Err("Markdown 过大，无法在远程控制中编辑，请改为下载".to_string());
    }
    let resolved = scoped_artifact_path(&store, &session_id, &path)?;
    super::artifacts::write_artifact_text_impl(&resolved, &content)
}

#[tauri::command]
pub async fn web_access_read_artifact_image_b64(
    path: String,
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<String, String> {
    let resolved = scoped_artifact_path(&store, &session_id, &path)?;
    // Base64 expands by roughly 4/3, leaving ample room below Relay's 4 MiB
    // frame ceiling for the surrounding RPC envelope.
    ensure_web_artifact_file_size(&resolved, 1_500_000)?;
    ensure_web_artifact_response(super::artifacts::read_artifact_image_b64(resolved).await?)
}

#[tauri::command]
pub async fn web_access_read_artifact_thumbnail(
    path: String,
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<Option<String>, String> {
    let resolved = scoped_artifact_path(&store, &session_id, &path)?;
    ensure_web_artifact_response(super::artifacts::read_artifact_thumbnail(resolved).await?)
}

#[tauri::command]
pub async fn web_access_render_artifact_visual(
    path: String,
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<super::artifacts::VisualResult, String> {
    let resolved = scoped_artifact_path(&store, &session_id, &path)?;
    ensure_web_artifact_response(super::artifacts::render_artifact_visual(resolved).await?)
}

#[cfg(test)]
mod tests {
    use super::{
        WebAcpOperation, WebSavedSession, WebSessionMetadata, WebSessionOperation,
        WebWorkspaceOperation, ensure_web_chat_session_supported, web_acp_agent_id, web_acp_result,
        web_artifact_storage_path, web_session_result, web_workspace_result,
    };
    use deepseek_tui::session_manager::{SavedSession, create_saved_session};
    use serde_json::{Value, json};
    use std::path::Path;

    #[test]
    fn stable_web_error_codes_are_locked() {
        // `acpErrors.js` treats every code below as a controlled Web failure;
        // `web_access_contract.test.mjs` additionally pins the mapping from
        // each web_access_* command to its operation. Renaming an operation
        // must change this list and the JS regex in the same PR.
        let acp_codes: Vec<String> = [
            WebAcpOperation::SessionCreate,
            WebAcpOperation::Cancel,
            WebAcpOperation::Prompt,
            WebAcpOperation::Timeline,
            WebAcpOperation::SessionInfo,
            WebAcpOperation::SetModel,
            WebAcpOperation::SetMode,
            WebAcpOperation::SetConfigOption,
            WebAcpOperation::PendingPermissions,
            WebAcpOperation::RespondPermission,
            WebAcpOperation::PendingElicitations,
            WebAcpOperation::RespondElicitation,
            WebAcpOperation::ListSessions,
            WebAcpOperation::ListAgents,
            WebAcpOperation::AgentStatus,
        ]
        .iter()
        .map(|operation| format!("web_acp_{}_failed", operation.as_str()))
        .collect();
        let expected_acp = [
            "web_acp_session_create_failed",
            "web_acp_cancel_failed",
            "web_acp_prompt_failed",
            "web_acp_timeline_failed",
            "web_acp_session_info_failed",
            "web_acp_set_model_failed",
            "web_acp_set_mode_failed",
            "web_acp_set_config_option_failed",
            "web_acp_pending_permissions_failed",
            "web_acp_respond_permission_failed",
            "web_acp_pending_elicitations_failed",
            "web_acp_respond_elicitation_failed",
            "web_acp_list_sessions_failed",
            "web_acp_list_agents_failed",
            "web_acp_agent_status_failed",
        ];
        assert_eq!(acp_codes, expected_acp);

        let workspace_codes: Vec<String> = [
            WebWorkspaceOperation::Listing,
            WebWorkspaceOperation::Search,
            WebWorkspaceOperation::Preview,
            WebWorkspaceOperation::Changes,
            WebWorkspaceOperation::Diff,
        ]
        .iter()
        .map(|operation| format!("web_workspace_{}_failed", operation.as_str()))
        .collect();
        assert_eq!(
            workspace_codes,
            [
                "web_workspace_listing_failed",
                "web_workspace_search_failed",
                "web_workspace_preview_failed",
                "web_workspace_changes_failed",
                "web_workspace_diff_failed",
            ]
        );

        // The mapping helper itself must keep the wire format stable and must
        // never leak host details from the underlying native error.
        let error: Result<(), &str> = Err("host detail /Users/alice/.pinvou3 must not cross");
        assert_eq!(
            web_acp_result(WebAcpOperation::Timeline, error).unwrap_err(),
            "web_acp_timeline_failed"
        );
        let error: Result<(), &str> = Err("host detail C:\\Users\\alice must not cross");
        assert_eq!(
            web_workspace_result(WebWorkspaceOperation::Diff, error).unwrap_err(),
            "web_workspace_diff_failed"
        );
        assert_eq!(
            web_session_result(WebSessionOperation::ListSessions, error).unwrap_err(),
            "web_session_list_sessions_failed"
        );
        assert_eq!(
            web_session_result(WebSessionOperation::ListArchivedSessions, error).unwrap_err(),
            "web_session_list_archived_sessions_failed"
        );
        assert_eq!(
            web_session_result(WebSessionOperation::CreateSession, error).unwrap_err(),
            "web_session_create_session_failed"
        );
        assert_eq!(
            web_session_result(WebSessionOperation::LoadSessionChunk, error).unwrap_err(),
            "web_session_load_session_chunk_failed"
        );
    }

    fn saved_session_with_private_paths(
        workspace: &str,
        context_target: &str,
        artifact_paths: &[&str],
    ) -> SavedSession {
        let session = create_saved_session(&[], "test-model", Path::new(workspace), 0, None);
        let mut value = serde_json::to_value(session).expect("serialize SavedSession fixture");
        value["context_references"] = json!([{
            "message_index": 0,
            "reference": {
                "kind": "file",
                "source": "at_mention",
                "badge": "file",
                "label": "main.rs",
                "target": context_target,
                "included": true,
                "expanded": false
            }
        }]);
        value["artifacts"] = Value::Array(
            artifact_paths
                .iter()
                .enumerate()
                .map(|(index, path)| {
                    json!({
                        "id": format!("artifact-{index}"),
                        "kind": "tool_output",
                        "session_id": value["metadata"]["id"],
                        "tool_call_id": format!("tool-call-{index}"),
                        "tool_name": "write_file",
                        "created_at": "2026-08-19T00:00:00Z",
                        "byte_size": 1,
                        "preview": "artifact",
                        "storage_path": path
                    })
                })
                .collect(),
        );
        serde_json::from_value(value).expect("deserialize SavedSession fixture")
    }

    fn assert_value_does_not_contain(value: &Value, private_path: &str) {
        match value {
            Value::String(value) => assert!(
                !value.contains(private_path),
                "serialized Web session exposed private path {private_path}: {value}"
            ),
            Value::Array(values) => values
                .iter()
                .for_each(|value| assert_value_does_not_contain(value, private_path)),
            Value::Object(values) => values
                .values()
                .for_each(|value| assert_value_does_not_contain(value, private_path)),
            _ => {}
        }
    }

    #[test]
    fn web_chat_rejects_desktop_only_multiagent_sessions() {
        assert!(ensure_web_chat_session_supported(false).is_ok());
        // 桌面开了多智能体开关的普通会话同样拒绝：Web 看不到也停不掉子智能体。
        assert!(ensure_web_chat_session_supported(true).is_err());
    }

    #[test]
    fn web_code_sessions_accept_only_acp_agents() {
        assert_eq!(web_acp_agent_id(None).unwrap(), "codex");
        assert_eq!(web_acp_agent_id(Some("claude".into())).unwrap(), "claude");
        assert_eq!(web_acp_agent_id(Some("kimi-acp".into())).unwrap(), "kimi");
        assert!(web_acp_agent_id(Some("pinvou".into())).is_err());
        assert!(web_acp_agent_id(Some("deepseek".into())).is_err());
    }

    #[test]
    fn web_workspace_errors_do_not_expose_host_paths() {
        for private_path in [
            r#"C:\Users\alice\secret-project"#,
            r#"\\server\private\project"#,
            "/Users/alice/secret-project",
            "/home/alice/secret-project",
        ] {
            let error = web_workspace_result::<(), _>(
                WebWorkspaceOperation::Preview,
                Err(format!("workspace unavailable: {private_path}")),
            )
            .unwrap_err();
            assert_eq!(error, "web_workspace_preview_failed");
            assert!(!error.contains(private_path));

            for (operation, code) in [
                (WebAcpOperation::Timeline, "timeline"),
                (WebAcpOperation::SessionCreate, "session_create"),
                (WebAcpOperation::Prompt, "prompt"),
                (WebAcpOperation::SessionInfo, "session_info"),
                (WebAcpOperation::SetModel, "set_model"),
                (WebAcpOperation::SetMode, "set_mode"),
                (WebAcpOperation::SetConfigOption, "set_config_option"),
                (WebAcpOperation::PendingPermissions, "pending_permissions"),
                (WebAcpOperation::PendingElicitations, "pending_elicitations"),
                (WebAcpOperation::RespondPermission, "respond_permission"),
                (WebAcpOperation::RespondElicitation, "respond_elicitation"),
                (WebAcpOperation::ListSessions, "list_sessions"),
                (WebAcpOperation::ListAgents, "list_agents"),
                (WebAcpOperation::AgentStatus, "agent_status"),
                (WebAcpOperation::Cancel, "cancel"),
            ] {
                let acp_error = web_acp_result::<(), _>(
                    operation,
                    Err(format!("ACP unavailable: {private_path}")),
                )
                .unwrap_err();
                assert_eq!(acp_error, format!("web_acp_{code}_failed"));
                assert!(!acp_error.contains(private_path));
            }
        }
    }

    #[test]
    fn web_saved_session_projects_only_browser_fields_and_safe_artifact_paths() {
        let fixtures = [
            (
                "/Users/alice/secret-project",
                "/Users/alice/secret-project/src/main.rs",
                "/Users/alice/secret-project/artifacts/report.md",
                "/Users/alice/.pinvou3/sessions/session-id/artifacts/chart.png",
                "/Users/alice/.pinvou3/sessions/session-id/artifacts",
                "/Users/alice/outside.txt",
            ),
            (
                r#"C:\Users\alice\secret-project"#,
                r#"C:\Users\alice\secret-project\src\main.rs"#,
                r#"C:\Users\alice\secret-project\artifacts\report.md"#,
                r#"C:\Users\alice\.pinvou3\sessions\session-id\artifacts\chart.png"#,
                r#"C:\Users\alice\.pinvou3\sessions\session-id\artifacts"#,
                r#"C:\Users\alice\outside.txt"#,
            ),
        ];

        for (
            workspace,
            context_target,
            in_workspace_artifact,
            private_artifact,
            private_artifact_root,
            outside_artifact,
        ) in fixtures
        {
            let session = saved_session_with_private_paths(
                workspace,
                context_target,
                &[
                    in_workspace_artifact,
                    private_artifact,
                    outside_artifact,
                    "artifacts/existing.txt",
                    "../escape.txt",
                ],
            );
            let projected = serde_json::to_value(WebSavedSession::project(
                &session,
                &[
                    Path::new(workspace).to_path_buf(),
                    Path::new(private_artifact_root).to_path_buf(),
                ],
                "revision",
            ))
            .expect("serialize projected Web session");

            let mut keys = projected
                .as_object()
                .expect("Web session object")
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>();
            keys.sort_unstable();
            assert_eq!(
                keys,
                ["artifacts", "messages", "metadata", "transcript_revision"]
            );
            assert_eq!(projected["metadata"]["workspace"], "secret-project");
            assert_eq!(projected["artifacts"].as_array().unwrap().len(), 3);
            assert_eq!(
                projected["artifacts"][0]["storage_path"],
                "artifacts/report.md"
            );
            assert_eq!(projected["artifacts"][1]["storage_path"], "chart.png");
            assert_eq!(
                projected["artifacts"][2]["storage_path"],
                "artifacts/existing.txt"
            );
            assert_value_does_not_contain(&projected, workspace);
            assert_value_does_not_contain(&projected, context_target);
            assert_value_does_not_contain(&projected, private_artifact_root);
            assert_value_does_not_contain(&projected, outside_artifact);
        }
    }

    #[test]
    fn web_artifact_projection_normalizes_windows_verbatim_paths() {
        for (workspace, storage_path, expected) in [
            (
                r#"C:\Users\alice\secret-project"#,
                r#"\\?\C:\Users\alice\secret-project\artifacts\report.md"#,
                "artifacts/report.md",
            ),
            (
                r#"\\?\C:\Users\alice\secret-project"#,
                r#"C:\Users\alice\secret-project\artifacts\report.md"#,
                "artifacts/report.md",
            ),
            (
                r#"\\server\private\secret-project"#,
                r#"\\?\UNC\server\private\secret-project\artifacts\report.md"#,
                "artifacts/report.md",
            ),
        ] {
            assert_eq!(
                web_artifact_storage_path(Path::new(storage_path), Path::new(workspace)),
                Some(expected.into())
            );
        }
        assert_eq!(
            web_artifact_storage_path(
                Path::new(r#"\\?\UNC\other\private\secret-project\artifacts\report.md"#),
                Path::new(r#"\\server\private\secret-project"#),
            ),
            None,
            "a verbatim UNC path from another authority must fail closed"
        );
    }

    #[test]
    fn web_session_metadata_redacts_workspace_paths() {
        for workspace in [
            "/Users/alice/secret-project",
            r#"C:\Users\alice\secret-project"#,
        ] {
            let session = create_saved_session(&[], "test-model", Path::new(workspace), 0, None);
            let projected = serde_json::to_value(WebSessionMetadata::project(
                session.metadata,
                "revision".into(),
            ))
            .expect("serialize projected Web metadata");
            assert_eq!(projected["workspace"], "secret-project");
            assert_value_does_not_contain(&projected, workspace);
        }
    }
}
