// ===================== 阶段 C: 输入文件上传 =====================

/// 把一个用户上传的文件转成 markdown（或标记不支持），返回 IngestResult。
/// 前端在 chip 行展示 token 估算 / 警告，发送时拼接 markdown 到 user message。
#[tauri::command]
pub async fn ingest_file(path: String) -> Result<crate::file_ingest::IngestResult, String> {
    let p = crate::file_ingest::validate_path(&path)?;
    Ok(crate::file_ingest::ingest(&p))
}

/// 返回系统工具检测结果（pandoc / pdftotext 是否可用）。
/// 前端启动时调一次，缺工具时给一次性 toast 引导 apt install。
#[tauri::command]
pub async fn detect_system_tools() -> Result<crate::file_ingest::SystemTools, String> {
    Ok(crate::file_ingest::system_tools())
}

/// 把剪贴板粘贴的图片 bytes 落盘到 `~/.pinvou3/pastes/<ts>-<name>` → 返回路径，
/// 前端拿到 path 后再 invoke `ingest_file`。
/// 只用于粘贴图片场景；选文件 / 拖拽走 Tauri native dialog 直接拿原 path。
#[tauri::command]
pub async fn save_paste_image(filename: String, bytes: Vec<u8>) -> Result<String, String> {
    let path = crate::file_ingest::save_paste_image(&filename, &bytes)?;
    Ok(path.to_string_lossy().to_string())
}

/// Web-remote E2E-only command that verifies the persisted upload digest.
#[tauri::command]
pub async fn verify_upload(upload_id: String) -> Result<VerifyUploadOutput, String> {
    if !matches!(std::env::var("PINVOU3_E2E").as_deref(), Ok("1")) {
        return Err("verify_upload is disabled: e2e-only command (set PINVOU3_E2E=1)".to_string());
    }
    crate::bridge::sessions::validate_session_id(&upload_id)
        .map_err(|_| "invalid upload_id".to_string())?;
    let upload_dir = crate::bridge::paths::pinvou3_home()
        .join("uploads")
        .join(&upload_id);
    let file_path = match std::fs::read_dir(&upload_dir).ok().and_then(|entries| {
        entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().map(|kind| kind.is_file()).unwrap_or(false))
            .next()
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
        sha256: format!("{:x}", hasher.finalize()),
        byte_size: bytes.len() as u64,
    })
}

#[derive(Debug, serde::Serialize)]
pub struct VerifyUploadOutput {
    pub sha256: String,
    pub byte_size: u64,
}
