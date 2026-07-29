use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;

use super::file_ingest::{self, IngestResult, MAX_FILE_BYTES};

pub const MAX_ATTACHMENT_CHUNK_BYTES: usize = 256 * 1024;
const STALE_STAGING_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

fn validate_upload_id(upload_id: &str) -> Result<&str, String> {
    if upload_id.len() < 8
        || upload_id.len() > 128
        || !upload_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("附件上传 ID 无效".into());
    }
    Ok(upload_id)
}

fn validate_filename(filename: &str) -> Result<&str, String> {
    if filename.is_empty()
        || filename.len() > 255
        || filename.chars().any(char::is_control)
        || filename.contains('/')
        || filename.contains('\\')
    {
        return Err("附件文件名无效".into());
    }
    let plain = Path::new(filename)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty() && *value != "." && *value != "..")
        .ok_or_else(|| "附件文件名无效".to_string())?;
    if plain != filename {
        return Err("附件文件名不得包含路径".into());
    }
    Ok(plain)
}

fn staging_root(workspace: &Path) -> PathBuf {
    workspace.join(".pinvou3").join("attachment-drop-staging")
}

fn completed_root(workspace: &Path) -> PathBuf {
    workspace.join("attachments")
}

fn upload_staging_dir(workspace: &Path, upload_id: &str) -> PathBuf {
    staging_root(workspace).join(upload_id)
}

fn upload_completed_dir(workspace: &Path, upload_id: &str) -> PathBuf {
    completed_root(workspace).join(upload_id)
}

async fn remove_dir_if_present(path: &Path) -> Result<(), String> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("清理附件目录失败：{error}")),
    }
}

async fn cleanup_stale_staging(workspace: &Path, keep_upload_id: &str) {
    let mut entries = match tokio::fs::read_dir(staging_root(workspace)).await {
        Ok(entries) => entries,
        Err(_) => return,
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.file_name().to_str() == Some(keep_upload_id) {
            continue;
        }
        let Ok(metadata) = entry.metadata().await else {
            continue;
        };
        if !metadata.is_dir()
            || !metadata
                .modified()
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age >= STALE_STAGING_AGE)
        {
            continue;
        }
        let _ = tokio::fs::remove_dir_all(entry.path()).await;
    }
}

/// Append one HTML5 drop chunk inside a session-owned workspace.
///
/// Incomplete bytes live below `.pinvou3/attachment-drop-staging/`. Commit is
/// an atomic rename into `attachments/<upload_id>/`, so the application keeps
/// exactly one managed copy and session deletion owns its final lifecycle.
pub async fn append_chunk(
    workspace: &Path,
    upload_id: &str,
    filename: &str,
    offset: usize,
    total: usize,
    data: &[u8],
    commit: bool,
) -> Result<Option<IngestResult>, String> {
    let upload_id = validate_upload_id(upload_id)?;
    let filename = validate_filename(filename)?;
    if total == 0 || total as u64 > MAX_FILE_BYTES {
        return Err("附件为空或超过 20 MiB 上限".into());
    }
    if data.len() > MAX_ATTACHMENT_CHUNK_BYTES {
        return Err("附件分块超过 256 KiB".into());
    }
    let end = offset
        .checked_add(data.len())
        .ok_or_else(|| "附件分块偏移溢出".to_string())?;
    if offset > total || end > total {
        return Err("附件分块超出声明大小".into());
    }

    let staging_dir = upload_staging_dir(workspace, upload_id);
    let staging_path = staging_dir.join(filename);
    if offset == 0 {
        cleanup_stale_staging(workspace, upload_id).await;
        remove_dir_if_present(&staging_dir).await?;
        tokio::fs::create_dir_all(&staging_dir)
            .await
            .map_err(|error| format!("创建附件暂存目录失败：{error}"))?;
    }

    let existing_len = tokio::fs::metadata(&staging_path)
        .await
        .map(|metadata| metadata.len() as usize)
        .unwrap_or(0);
    if existing_len != offset {
        return Err(format!(
            "附件分块偏移不连续：期望 {existing_len}，收到 {offset}"
        ));
    }

    let mut output = tokio::fs::OpenOptions::new()
        .create(offset == 0)
        .append(true)
        .open(&staging_path)
        .await
        .map_err(|error| format!("打开附件暂存文件失败：{error}"))?;
    output
        .write_all(data)
        .await
        .map_err(|error| format!("写入附件分块失败：{error}"))?;
    output
        .flush()
        .await
        .map_err(|error| format!("刷新附件分块失败：{error}"))?;
    drop(output);

    if !commit {
        return Ok(None);
    }
    if end != total {
        return Err(format!("附件提交大小不完整：应为 {total}，实际为 {end}"));
    }

    let completed_dir = upload_completed_dir(workspace, upload_id);
    tokio::fs::create_dir_all(completed_root(workspace))
        .await
        .map_err(|error| format!("创建会话附件目录失败：{error}"))?;
    if tokio::fs::try_exists(&completed_dir).await.unwrap_or(false) {
        return Err("附件上传 ID 已存在".into());
    }
    tokio::fs::rename(&staging_dir, &completed_dir)
        .await
        .map_err(|error| format!("提交会话附件失败：{error}"))?;
    let completed_path = completed_dir.join(filename);
    let validated_path = file_ingest::validate_path(completed_path.to_string_lossy().as_ref())?;
    Ok(Some(file_ingest::ingest(&validated_path)))
}

/// Cancel an incomplete upload. The completed directory is also removed to
/// cover the small race where commit finished before the UI observed success.
pub async fn cancel_upload(workspace: &Path, upload_id: &str) -> Result<(), String> {
    let upload_id = validate_upload_id(upload_id)?;
    remove_dir_if_present(&upload_staging_dir(workspace, upload_id)).await?;
    remove_dir_if_present(&upload_completed_dir(workspace, upload_id)).await
}

/// Delete an unsent managed attachment without ever accepting an arbitrary
/// filesystem target from the frontend.
pub async fn discard_attachment(workspace: &Path, path: &str) -> Result<(), String> {
    let supplied = Path::new(path);
    let root = completed_root(workspace);
    let canonical_root = match tokio::fs::canonicalize(&root).await {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("解析会话附件目录失败：{error}")),
    };
    let canonical_file = match tokio::fs::canonicalize(supplied).await {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("解析会话附件失败：{error}")),
    };
    let upload_dir = canonical_file
        .parent()
        .ok_or_else(|| "附件路径无效".to_string())?;
    if upload_dir.parent() != Some(canonical_root.as_path()) {
        return Err("拒绝删除非会话拖拽附件".into());
    }
    let upload_id = upload_dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "附件上传 ID 无效".to_string())?;
    validate_upload_id(upload_id)?;

    let metadata = tokio::fs::symlink_metadata(&canonical_file)
        .await
        .map_err(|error| format!("读取会话附件失败：{error}"))?;
    if !metadata.file_type().is_file() {
        return Err("拒绝删除非文件附件".into());
    }
    tokio::fs::remove_file(&canonical_file)
        .await
        .map_err(|error| format!("删除会话附件失败：{error}"))?;
    match tokio::fs::remove_dir(upload_dir).await {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(format!("清理会话附件目录失败：{error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        append_chunk, discard_attachment, validate_filename, validate_upload_id,
        MAX_ATTACHMENT_CHUNK_BYTES,
    };
    use std::path::PathBuf;

    fn test_workspace(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "pinvou3-attachment-upload-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn validates_attachment_upload_identifiers() {
        assert!(validate_upload_id("desktop_attach_123").is_ok());
        assert!(validate_upload_id("../escape").is_err());
        assert!(validate_upload_id("short").is_err());
    }

    #[test]
    fn validates_plain_attachment_filenames() {
        assert_eq!(validate_filename("测试附件.pdf").unwrap(), "测试附件.pdf");
        for filename in ["", ".", "..", "../a.pdf", r"..\a.pdf", "a/b.pdf", "a\n.pdf"] {
            assert!(validate_filename(filename).is_err(), "{filename:?}");
        }
    }

    #[tokio::test]
    async fn commits_once_into_session_workspace_and_discards_safely() {
        let workspace = test_workspace("lifecycle");
        let upload_id = "desktop_attach_lifecycle";
        let bytes = b"hello";
        let result = append_chunk(
            &workspace,
            upload_id,
            "hello.txt",
            0,
            bytes.len(),
            bytes,
            true,
        )
        .await
        .unwrap()
        .unwrap();

        let expected = workspace
            .join("attachments")
            .join(upload_id)
            .join("hello.txt");
        assert_eq!(PathBuf::from(&result.path), expected);
        assert_eq!(std::fs::read(&expected).unwrap(), bytes);
        assert!(!workspace
            .join(".pinvou3")
            .join("attachment-drop-staging")
            .join(upload_id)
            .exists());

        discard_attachment(&workspace, &result.path).await.unwrap();
        assert!(!expected.exists());
        assert!(workspace.exists());
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn rejects_oversized_chunks_without_writing() {
        let workspace = test_workspace("chunk-cap");
        let bytes = vec![0_u8; MAX_ATTACHMENT_CHUNK_BYTES + 1];
        assert!(append_chunk(
            &workspace,
            "desktop_attach_chunk_cap",
            "large.bin",
            0,
            bytes.len(),
            &bytes,
            true,
        )
        .await
        .is_err());
        assert!(!workspace.exists());
    }
}
