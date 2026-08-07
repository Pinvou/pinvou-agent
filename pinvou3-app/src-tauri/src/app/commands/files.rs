// ===================== 阶段 C: 输入文件上传 =====================

use base64::Engine as _;
use tauri::State;

use crate::features::sessions::{SessionKind, SessionStore};

fn conversation_attachment_context(
    store: &SessionStore,
    session_id: &str,
) -> Result<(std::path::PathBuf, SessionKind), String> {
    crate::features::sessions::validate_session_id(session_id)
        .map_err(|_| "会话 ID 无效".to_string())?;
    store
        .load(session_id)
        .map_err(|error| format!("会话不存在：{error:#}"))?;
    let kind = store
        .session_kind(session_id)
        .map_err(|error| format!("解析会话类型失败：{error:#}"))?;
    let workspace = store
        .ledger_root(session_id)
        .map_err(|error| format!("解析会话附件工作区失败：{error:#}"))?;
    Ok((workspace, kind))
}

/// 把一个用户上传的文件转成 markdown（或标记不支持），返回 IngestResult。
/// 前端在 chip 行展示 token 估算 / 警告，发送时拼接 markdown 到 user message。
#[tauri::command]
pub async fn ingest_file(
    path: String,
) -> Result<crate::features::files::file_ingest::IngestResult, String> {
    let p = crate::features::files::file_ingest::validate_path(&path)?;
    Ok(crate::features::files::file_ingest::ingest(&p))
}

/// Receive an HTML5 `File` into a sessionless draft area. Dropping or pasting
/// a file must not materialize a conversation; `adopt_draft_attachment`
/// transfers it into the selected session immediately before submission.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn ingest_draft_file_chunk(
    upload_id: String,
    filename: String,
    offset: usize,
    total: usize,
    data_base64: String,
    commit: bool,
) -> Result<Option<crate::features::files::file_ingest::IngestResult>, String> {
    let result = async {
        let data = base64::engine::general_purpose::STANDARD
            .decode(data_base64)
            .map_err(|error| format!("解码附件分块失败：{error}"))?;
        crate::features::files::attachment_upload::append_draft_chunk(
            &upload_id, &filename, offset, total, &data, commit,
        )
        .await
    }
    .await;
    if result.is_err() {
        let _ = crate::features::files::attachment_upload::abort_draft_upload(&upload_id).await;
    }
    result
}

#[tauri::command]
pub async fn cancel_draft_file_upload(upload_id: String) -> Result<(), String> {
    crate::features::files::attachment_upload::cancel_draft_upload(&upload_id).await
}

#[tauri::command]
pub async fn adopt_draft_attachment(
    session_id: String,
    upload_id: String,
    store: State<'_, SessionStore>,
) -> Result<crate::features::files::file_ingest::IngestResult, String> {
    let (workspace, _) = conversation_attachment_context(&store, &session_id)?;
    crate::features::files::attachment_upload::adopt_draft_upload(&workspace, &upload_id).await
}

/// Windows WebView2 的 HTML5 文件拖拽无法暴露源文件路径，因此通过有界分块
/// 直接写入当前会话工作区，再复用统一的文件摄入流程。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn ingest_dropped_file_chunk(
    session_id: String,
    upload_id: String,
    filename: String,
    offset: usize,
    total: usize,
    data_base64: String,
    commit: bool,
    store: State<'_, SessionStore>,
) -> Result<Option<crate::features::files::file_ingest::IngestResult>, String> {
    let (workspace, _) = conversation_attachment_context(&store, &session_id)?;
    let result = async {
        let data = base64::engine::general_purpose::STANDARD
            .decode(data_base64)
            .map_err(|error| format!("解码附件分块失败：{error}"))?;
        crate::features::files::attachment_upload::append_chunk(
            &workspace, &upload_id, &filename, offset, total, &data, commit,
        )
        .await
    }
    .await;
    if result.is_err() {
        let _ =
            crate::features::files::attachment_upload::abort_staging_upload(&workspace, &upload_id)
                .await;
    }
    result
}

#[tauri::command]
pub async fn cancel_dropped_file_upload(
    session_id: String,
    upload_id: String,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    let (workspace, _) = conversation_attachment_context(&store, &session_id)?;
    crate::features::files::attachment_upload::cancel_upload(&workspace, &upload_id).await
}

#[tauri::command]
pub async fn discard_dropped_attachment(
    session_id: String,
    path: String,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    let (workspace, _) = conversation_attachment_context(&store, &session_id)?;
    crate::features::files::attachment_upload::discard_attachment(&workspace, &path).await
}

pub(super) fn resolve_conversation_attachment_path(
    store: &SessionStore,
    session_id: &str,
    message_index: usize,
    attachment_index: usize,
    basename: &str,
    display_text: &str,
) -> Result<std::path::PathBuf, String> {
    let (workspace, kind) = conversation_attachment_context(store, session_id)?;
    crate::features::files::attachment_upload::resolve_conversation_attachment(
        &workspace,
        session_id,
        kind == SessionKind::Chat,
        message_index,
        attachment_index,
        basename,
        display_text,
    )
}

#[tauri::command]
pub async fn resolve_conversation_attachment(
    session_id: String,
    message_index: usize,
    attachment_index: usize,
    basename: String,
    display_text: String,
    store: State<'_, SessionStore>,
) -> Result<String, String> {
    resolve_conversation_attachment_path(
        &store,
        &session_id,
        message_index,
        attachment_index,
        &basename,
        &display_text,
    )
    .map(|path| path.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn open_conversation_attachment(
    session_id: String,
    message_index: usize,
    attachment_index: usize,
    basename: String,
    display_text: String,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    let path = resolve_conversation_attachment_path(
        &store,
        &session_id,
        message_index,
        attachment_index,
        &basename,
        &display_text,
    )?;
    crate::platform::os::open_target(
        crate::platform::os::external_application_path(&path),
        "对话附件",
    )
}

#[tauri::command]
pub async fn reveal_conversation_attachment(
    session_id: String,
    message_index: usize,
    attachment_index: usize,
    basename: String,
    display_text: String,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    let path = resolve_conversation_attachment_path(
        &store,
        &session_id,
        message_index,
        attachment_index,
        &basename,
        &display_text,
    )?;
    crate::platform::os::reveal_target(&path)
}

/// 返回系统工具检测结果（pandoc / pdftotext 是否可用）。
/// 前端启动时调一次，缺工具时给一次性 toast 引导 apt install。
#[tauri::command]
pub async fn detect_system_tools(
) -> Result<crate::features::files::file_ingest::SystemTools, String> {
    Ok(crate::features::files::file_ingest::system_tools())
}

/// 把剪贴板粘贴的图片 bytes 落盘到 `~/.pinvou3/pastes/<ts>-<name>` → 返回路径，
/// 前端拿到 path 后再 invoke `ingest_file`。
/// 只用于粘贴图片场景；文件选择器直接拿原 path，HTML5 拖拽走有界分块命令。
#[tauri::command]
pub async fn save_paste_image(filename: String, bytes: Vec<u8>) -> Result<String, String> {
    let path = crate::features::files::file_ingest::save_paste_image(&filename, &bytes)?;
    Ok(path.to_string_lossy().to_string())
}

/// Web-remote E2E-only command that verifies the persisted upload digest.
#[tauri::command]
pub async fn verify_upload(upload_id: String) -> Result<VerifyUploadOutput, String> {
    if !matches!(std::env::var("PINVOU3_E2E").as_deref(), Ok("1")) {
        return Err("verify_upload is disabled: e2e-only command (set PINVOU3_E2E=1)".to_string());
    }
    crate::features::sessions::validate_session_id(&upload_id)
        .map_err(|_| "invalid upload_id".to_string())?;
    let upload_dir = crate::platform::paths::pinvou3_home()
        .join("uploads")
        .join(&upload_id);
    let file_path = match std::fs::read_dir(&upload_dir).ok().and_then(|entries| {
        entries
            .filter_map(|entry| entry.ok())
            .find(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.path())
    }) {
        Some(path) => path,
        None => return Err("upload not available".to_string()),
    };
    const VERIFY_UPLOAD_MAX_BYTES: usize = 20 * 1024 * 1024;
    let mut file = tokio::fs::File::open(&file_path)
        .await
        .map_err(|_| "upload not available".to_string())?;
    let metadata_len = file
        .metadata()
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(VERIFY_UPLOAD_MAX_BYTES as u64);
    if metadata_len as usize > VERIFY_UPLOAD_MAX_BYTES {
        return Err("upload not available".to_string());
    }
    let mut bytes = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut file, &mut bytes)
        .await
        .map_err(|_| "upload not available".to_string())?;
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(VerifyUploadOutput {
        sha256: crate::platform::encoding::hex_lower(&hasher.finalize()),
        byte_size: bytes.len() as u64,
    })
}

#[derive(Debug, serde::Serialize)]
pub struct VerifyUploadOutput {
    pub sha256: String,
    pub byte_size: u64,
}
