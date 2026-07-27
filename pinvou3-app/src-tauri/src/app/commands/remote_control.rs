use base64::Engine as _;
use serde::Serialize;
use serde_json::Value;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use tauri::{AppHandle, State, WebviewWindow};

use crate::features::assistant::engine_pool::EnginePool;
use crate::features::remote_control::manager;
use crate::features::remote_control::{
    RelaySettingsInfo, RemoteControlManager, WebAccessInfo, WebAccessStatus,
};
use crate::features::sessions::SessionStore;
use crate::platform::prefs::UserPrefs;

const MAX_WEB_ARTIFACT_RPC_BYTES: usize = 2 * 1024 * 1024;
const WEB_SESSION_SERIALIZATION_HEADROOM: usize = 1024 * 1024;

fn require_main_webview(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("Web access bridge is restricted to the main desktop WebView".to_string())
    }
}

#[tauri::command]
pub fn web_access_enable(
    manager: State<'_, RemoteControlManager>,
) -> Result<WebAccessInfo, String> {
    manager.start()
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

/// Browse the desktop host filesystem for the WebUI file picker. This returns
/// paths only; browser bytes are never uploaded to the desktop.
#[tauri::command]
pub fn web_access_list_host_files(
    path: Option<String>,
) -> Result<manager::HostFileListing, String> {
    manager::list_host_files(path)
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
    let metadata = super::sessions::create_session(Some(false), app, store, pool).await?;
    let transcript_revision = crate::features::sessions::transcript_revision(&[])
        .map_err(|error| format!("create empty transcript revision: {error:#}"))?;
    Ok(WebSessionMetadata {
        metadata,
        transcript_revision,
    })
}

/// Preserve the native `SessionMetadata` response shape while adding the
/// browser-only optimistic-concurrency token.
#[derive(Debug, Serialize)]
pub struct WebSessionMetadata {
    #[serde(flatten)]
    metadata: deepseek_tui::session_manager::SessionMetadata,
    transcript_revision: String,
}

#[derive(Serialize)]
struct WebSavedSession<'a> {
    #[serde(flatten)]
    session: &'a deepseek_tui::session_manager::SavedSession,
    transcript_revision: &'a str,
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
#[tauri::command]
pub async fn web_access_load_session_chunk(
    id: String,
    download_id: Option<String>,
    offset: u64,
    limit: Option<usize>,
    manager: State<'_, RemoteControlManager>,
    store: State<'_, SessionStore>,
) -> Result<SessionDataChunk, String> {
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
            let reservation = manager.begin_web_session_download(&id, reserved_bytes)?;
            let output_path = reservation.path().to_path_buf();
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
                let file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&output_path)
                    .map_err(|error| format!("create serialized Session download: {error}"))?;
                let mut writer = BufWriter::new(file);
                serde_json::to_writer(
                    &mut writer,
                    &WebSavedSession {
                        session: &saved,
                        transcript_revision: &revision,
                    },
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
    Ok(SessionDataChunk {
        download_id,
        offset: offset as u64,
        total: chunk.total as u64,
        data_base64: base64::engine::general_purpose::STANDARD.encode(chunk.data),
        eof: chunk.eof,
        transcript_revision,
    })
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
    crate::features::sessions::validate_session_id(&session_id)
        .map_err(|error| format!("invalid Session id: {error:#}"))?;
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
    let result = super::chat::chat_with_reservation(
        message,
        Some(attachments),
        session_id,
        restrict_tools,
        turn_reservation,
        &pool,
        &store,
        &app,
    )
    .await;
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
    if data.len() > manager::MAX_ARTIFACT_CHUNK_BYTES {
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

#[tauri::command]
pub async fn web_access_start_skill_session(
    name: String,
    app: AppHandle,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<super::workflows::StartSkillSessionResult, String> {
    super::workflows::start_skill_session(name, Some(false), app, store, pool).await
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
) -> Result<manager::ArtifactChunk, String> {
    manager::read_artifact_chunk(&store, &path, &session_id, offset, limit)
}

/// Merge only settings that remain visible and meaningful in WebUI. Hidden
/// desktop authority (shell, model bootstrap, pet, notifications, theme and
/// language) always stays sourced from the current desktop preferences.
#[tauri::command]
pub async fn web_access_update_settings(prefs: UserPrefs) -> Result<(), String> {
    let mut current = UserPrefs::load();
    current.memory_enabled = prefs.memory_enabled;
    current.search = prefs.search;
    super::settings::update_settings(current).await
}

fn scoped_artifact_path(
    store: &SessionStore,
    session_id: &str,
    path: &str,
) -> Result<String, String> {
    manager::resolve_session_artifact_path(store, session_id, path)
        .map(|resolved| resolved.to_string_lossy().into_owned())
}

fn scoped_workflow_project(store: &SessionStore, session_id: &str) -> Result<String, String> {
    crate::features::sessions::validate_session_id(session_id)
        .map_err(|error| format!("invalid workflow Session id: {error:#}"))?;
    store
        .load(session_id)
        .map_err(|error| format!("load workflow Session {session_id}: {error:#}"))?;
    let binding = store
        .active_skill(session_id)
        .ok_or_else(|| format!("Session {session_id} has no workflow binding"))?;
    let configured = binding
        .project_dir
        .ok_or_else(|| format!("Session {session_id} is not a workflow Session"))?;
    let project = std::path::PathBuf::from(configured)
        .canonicalize()
        .map_err(|error| format!("resolve workflow project: {error}"))?;
    let workspace = store
        .execution_workspace(session_id)
        .map_err(|error| format!("resolve workflow workspace: {error:#}"))?
        .canonicalize()
        .map_err(|error| format!("resolve workflow workspace root: {error}"))?;
    if !project.starts_with(&workspace) {
        return Err("workflow project is outside its Session workspace".to_string());
    }
    Ok(project.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn web_access_list_deliverables(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<Value, String> {
    super::artifacts::list_deliverables(scoped_workflow_project(&store, &session_id)?).await
}

#[tauri::command]
pub async fn web_access_get_role_prompt(
    role_id: String,
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<super::workflows::RolePromptPayload, String> {
    let project = scoped_workflow_project(&store, &session_id)?;
    super::workflows::get_role_prompt(role_id, Some(project)).await
}

#[tauri::command]
pub async fn web_access_get_role_outputs(
    role_id: String,
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<Vec<super::workflows::OutputFile>, String> {
    super::workflows::get_role_outputs(role_id, scoped_workflow_project(&store, &session_id)?).await
}

#[tauri::command]
pub async fn web_access_get_role_logs(
    role_id: String,
    session_id: String,
    tail: Option<usize>,
    store: State<'_, SessionStore>,
) -> Result<Vec<Value>, String> {
    super::workflows::get_role_logs(role_id, scoped_workflow_project(&store, &session_id)?, tail)
        .await
}

#[tauri::command]
pub async fn web_access_get_gate_report(
    role_id: String,
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<Option<Value>, String> {
    super::workflows::get_gate_report(role_id, scoped_workflow_project(&store, &session_id)?).await
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
