use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tokio::io::AsyncWriteExt;

use super::file_ingest::{self, IngestResult, MAX_FILE_BYTES};

pub const MAX_ATTACHMENT_CHUNK_BYTES: usize = 256 * 1024;
const STALE_ATTACHMENT_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const CONVERSATION_ATTACHMENT_REFS_FILE: &str = "conversation-attachments.json";
static CONVERSATION_ATTACHMENT_REFS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ConversationAttachmentReference {
    pub basename: String,
    pub path: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct ConversationAttachmentRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    message_index: usize,
    display_text: String,
    attachments: Vec<ConversationAttachmentReference>,
}

fn conversation_attachment_refs_path(workspace: &Path) -> PathBuf {
    workspace
        .join(".pinvou3")
        .join(CONVERSATION_ATTACHMENT_REFS_FILE)
}

pub fn conversation_attachment_names_for_display_prefix(
    workspace: &Path,
    session_id: &str,
    display_prefix: &str,
    allow_legacy_unscoped: bool,
) -> Result<Vec<String>, String> {
    let refs_path = conversation_attachment_refs_path(workspace);
    let records = match std::fs::read(&refs_path) {
        Ok(bytes) => serde_json::from_slice::<Vec<ConversationAttachmentRecord>>(&bytes)
            .map_err(|error| format!("读取附件引用失败：{error}"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("读取附件引用失败：{error}")),
    };
    Ok(records
        .iter()
        .find(|record| {
            record_matches_session(record, session_id, allow_legacy_unscoped)
                && record.display_text.starts_with(display_prefix)
        })
        .map(|record| {
            record
                .attachments
                .iter()
                .map(|attachment| attachment.basename.clone())
                .collect()
        })
        .unwrap_or_default())
}

/// Persist display-only attachment references outside the LLM transcript.
pub fn record_conversation_attachments(
    workspace: &Path,
    session_id: &str,
    message_index: usize,
    display_text: &str,
    attachments: Vec<ConversationAttachmentReference>,
) -> Result<(), String> {
    if attachments.is_empty() {
        return Ok(());
    }
    let _write_guard = CONVERSATION_ATTACHMENT_REFS_LOCK
        .lock()
        .map_err(|_| "附件引用写入锁不可用".to_string())?;
    let refs_path = conversation_attachment_refs_path(workspace);
    let mut records = match std::fs::read(&refs_path) {
        Ok(bytes) => serde_json::from_slice::<Vec<ConversationAttachmentRecord>>(&bytes)
            .map_err(|error| format!("读取附件引用失败：{error}"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(format!("读取附件引用失败：{error}")),
    };
    let record = ConversationAttachmentRecord {
        session_id: Some(session_id.to_string()),
        message_index,
        display_text: display_text.to_string(),
        attachments,
    };
    if let Some(existing) = records.iter_mut().find(|existing| {
        existing.session_id.as_deref() == Some(session_id)
            && existing.message_index == message_index
    }) {
        *existing = record;
    } else {
        records.push(record);
        records.sort_by_key(|entry| entry.message_index);
    }
    let parent = refs_path
        .parent()
        .ok_or_else(|| "附件引用目录无效".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| format!("创建附件引用目录失败：{error}"))?;
    let payload = serde_json::to_vec_pretty(&records)
        .map_err(|error| format!("序列化附件引用失败：{error}"))?;
    deepseek_tui::utils::write_atomic(&refs_path, &payload)
        .map_err(|error| format!("保存附件引用失败：{error:#}"))?;
    Ok(())
}

pub fn resolve_conversation_attachment(
    workspace: &Path,
    session_id: &str,
    allow_legacy_unscoped: bool,
    message_index: usize,
    attachment_index: usize,
    expected_basename: &str,
    expected_display_text: &str,
) -> Result<PathBuf, String> {
    validate_filename(expected_basename)?;
    let refs_path = conversation_attachment_refs_path(workspace);
    let payload =
        std::fs::read(&refs_path).map_err(|_| "该附件没有可用的本地文件引用".to_string())?;
    let records: Vec<ConversationAttachmentRecord> =
        serde_json::from_slice(&payload).map_err(|error| format!("读取附件引用失败：{error}"))?;
    let reference = records
        .iter()
        .find(|record| {
            // New records use stable session/message identity. Display text is
            // presentation-only and may differ when the frontend hides an
            // injected guide. Legacy chat workspaces remain text-bound because
            // their records predate session scoping.
            record.message_index == message_index
                && (record.session_id.as_deref() == Some(session_id)
                    || (allow_legacy_unscoped
                        && record.session_id.is_none()
                        && record.display_text == expected_display_text))
        })
        .and_then(|record| record.attachments.get(attachment_index))
        .filter(|reference| reference.basename == expected_basename)
        .ok_or_else(|| "该附件引用已失效或与当前消息不匹配".to_string())?;
    let path = file_ingest::validate_path(&reference.path)?;
    if path.file_name().and_then(|name| name.to_str()) != Some(reference.basename.as_str()) {
        return Err("附件文件名与引用不匹配".into());
    }
    Ok(path)
}

fn record_matches_session(
    record: &ConversationAttachmentRecord,
    session_id: &str,
    allow_legacy_unscoped: bool,
) -> bool {
    record.session_id.as_deref() == Some(session_id)
        || (allow_legacy_unscoped && record.session_id.is_none())
}

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

pub(crate) fn draft_attachment_workspace() -> PathBuf {
    crate::platform::paths::pinvou3_home().join("draft-attachments")
}

async fn remove_dir_if_present(path: &Path) -> Result<(), String> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("清理附件目录失败：{error}")),
    }
}

fn stale_upload_directory(path: &Path, metadata: &std::fs::Metadata, now: SystemTime) -> bool {
    if !metadata.file_type().is_dir() {
        return false;
    }
    let latest_file_modified = std::fs::read_dir(path).ok().and_then(|entries| {
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::symlink_metadata(entry.path()).ok())
            .filter(|metadata| metadata.file_type().is_file())
            .filter_map(|metadata| metadata.modified().ok())
            .max()
    });
    latest_file_modified
        .or_else(|| metadata.modified().ok())
        .and_then(|modified| now.duration_since(modified).ok())
        .is_some_and(|age| age >= STALE_ATTACHMENT_AGE)
}

fn sweep_stale_upload_root(root: &Path, now: SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            std::fs::symlink_metadata(entry.path())
                .is_ok_and(|metadata| stale_upload_directory(&entry.path(), &metadata, now))
        })
        .filter(|entry| std::fs::remove_dir_all(entry.path()).is_ok())
        .count()
}

fn sweep_stale_draft_attachment_workspace(workspace: &Path, now: SystemTime) -> usize {
    sweep_stale_upload_root(&staging_root(workspace), now)
        + sweep_stale_upload_root(&completed_root(workspace), now)
}

/// Remove abandoned sessionless uploads at process startup. This is a
/// best-effort sweep over application-owned directories only. It deliberately
/// never runs while accepting uploads, so a long-lived ready draft cannot be
/// removed by a later upload in the same process.
pub(crate) fn sweep_stale_draft_attachments() -> usize {
    sweep_stale_draft_attachment_workspace(&draft_attachment_workspace(), SystemTime::now())
}

/// Append a browser `File` into an application-owned draft area. Unlike the
/// legacy session-scoped drop command this does not require, or create, a
/// conversation. The completed upload is adopted by the target session only
/// when the user actually sends the draft.
pub async fn append_draft_chunk(
    upload_id: &str,
    filename: &str,
    offset: usize,
    total: usize,
    data: &[u8],
    commit: bool,
) -> Result<Option<IngestResult>, String> {
    let workspace = draft_attachment_workspace();
    append_draft_chunk_in_workspace(&workspace, upload_id, filename, offset, total, data, commit)
        .await
}

async fn append_draft_chunk_in_workspace(
    workspace: &Path,
    upload_id: &str,
    filename: &str,
    offset: usize,
    total: usize,
    data: &[u8],
    commit: bool,
) -> Result<Option<IngestResult>, String> {
    append_chunk(workspace, upload_id, filename, offset, total, data, commit).await
}

pub async fn abort_draft_upload(upload_id: &str) -> Result<(), String> {
    abort_staging_upload(&draft_attachment_workspace(), upload_id).await
}

pub async fn cancel_draft_upload(upload_id: &str) -> Result<(), String> {
    cancel_upload(&draft_attachment_workspace(), upload_id).await
}

async fn managed_completed_file(
    workspace: &Path,
    upload_id: &str,
) -> Result<(PathBuf, PathBuf), String> {
    let upload_id = validate_upload_id(upload_id)?;
    let root = completed_root(workspace);
    let canonical_root = tokio::fs::canonicalize(&root)
        .await
        .map_err(|_| "附件草稿不存在".to_string())?;
    let upload_dir = upload_completed_dir(workspace, upload_id);
    let canonical_dir = tokio::fs::canonicalize(&upload_dir)
        .await
        .map_err(|_| "附件草稿不存在".to_string())?;
    if canonical_dir.parent() != Some(canonical_root.as_path()) {
        return Err("附件草稿目录无效".into());
    }

    let mut entries = tokio::fs::read_dir(&canonical_dir)
        .await
        .map_err(|error| format!("读取附件草稿失败：{error}"))?;
    let mut file = None;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| format!("读取附件草稿失败：{error}"))?
    {
        let metadata = tokio::fs::symlink_metadata(entry.path())
            .await
            .map_err(|error| format!("读取附件草稿失败：{error}"))?;
        if !metadata.file_type().is_file() || file.is_some() {
            return Err("附件草稿内容无效".into());
        }
        file = Some(entry.path());
    }
    let file = file.ok_or_else(|| "附件草稿为空".to_string())?;
    let filename = file
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "附件草稿文件名无效".to_string())?;
    validate_filename(filename)?;
    Ok((canonical_dir, file))
}

/// Move a sessionless draft upload into the target session workspace. Copy is
/// used instead of rename because project workspaces may live on another
/// volume. A repeated call is idempotent so an IPC response lost after the
/// copy cannot strand the attachment.
pub async fn adopt_draft_upload(
    target_workspace: &Path,
    upload_id: &str,
) -> Result<IngestResult, String> {
    validate_upload_id(upload_id)?;
    let draft_workspace = draft_attachment_workspace();
    adopt_upload(&draft_workspace, target_workspace, upload_id).await
}

async fn adopt_upload(
    source_workspace: &Path,
    target_workspace: &Path,
    upload_id: &str,
) -> Result<IngestResult, String> {
    validate_upload_id(upload_id)?;
    let target_dir = upload_completed_dir(target_workspace, upload_id);

    if tokio::fs::try_exists(&target_dir).await.unwrap_or(false) {
        let (_, target_file) = managed_completed_file(target_workspace, upload_id).await?;
        let _ = remove_dir_if_present(&upload_completed_dir(source_workspace, upload_id)).await;
        return Ok(file_ingest::ingest(&target_file));
    }

    let (source_dir, source_file) = managed_completed_file(source_workspace, upload_id).await?;
    let filename = source_file
        .file_name()
        .map(ToOwned::to_owned)
        .ok_or_else(|| "附件草稿文件名无效".to_string())?;
    tokio::fs::create_dir_all(completed_root(target_workspace))
        .await
        .map_err(|error| format!("创建会话附件目录失败：{error}"))?;
    tokio::fs::create_dir(&target_dir)
        .await
        .map_err(|error| format!("创建会话附件目录失败：{error}"))?;
    let target_file = target_dir.join(&filename);
    if let Err(error) = tokio::fs::copy(&source_file, &target_file).await {
        let _ = remove_dir_if_present(&target_dir).await;
        return Err(format!("迁移附件草稿失败：{error}"));
    }
    let result = file_ingest::ingest(&target_file);
    if let Err(error) = remove_dir_if_present(&source_dir).await {
        eprintln!("[attachment] cleanup adopted draft failed: {error}");
    }
    Ok(result)
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
    Ok(Some(file_ingest::ingest(&completed_path)))
}

/// Clean up bytes from a failed append without touching a previously completed
/// attachment that happens to use the same client-provided upload ID.
pub async fn abort_staging_upload(workspace: &Path, upload_id: &str) -> Result<(), String> {
    let upload_id = validate_upload_id(upload_id)?;
    remove_dir_if_present(&upload_staging_dir(workspace, upload_id)).await
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
        abort_staging_upload, adopt_upload, append_chunk, append_draft_chunk_in_workspace,
        conversation_attachment_names_for_display_prefix, conversation_attachment_refs_path,
        discard_attachment, record_conversation_attachments, resolve_conversation_attachment,
        sweep_stale_draft_attachment_workspace, upload_staging_dir, validate_filename,
        validate_upload_id, ConversationAttachmentRecord, ConversationAttachmentReference,
        MAX_ATTACHMENT_CHUNK_BYTES, STALE_ATTACHMENT_AGE,
    };
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn set_file_modified(path: &std::path::Path, modified: SystemTime) {
        std::fs::File::open(path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
    }

    fn test_workspace(name: &str) -> PathBuf {
        crate::platform::os::user_home_dir().join(format!(
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

    #[test]
    fn startup_sweep_removes_stale_staging_and_completed_drafts() {
        let workspace = test_workspace("startup-sweep");
        let staging = super::staging_root(&workspace).join("desktop_attach_stale_staging");
        let completed = super::completed_root(&workspace).join("desktop_attach_stale_completed");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&completed).unwrap();
        std::fs::write(staging.join("partial.bin"), b"partial").unwrap();
        std::fs::write(completed.join("draft.txt"), b"draft").unwrap();

        assert_eq!(
            sweep_stale_draft_attachment_workspace(&workspace, std::time::SystemTime::now()),
            0
        );
        assert!(staging.exists());
        assert!(completed.exists());

        let after_ttl =
            std::time::SystemTime::now() + STALE_ATTACHMENT_AGE + Duration::from_secs(1);
        assert_eq!(
            sweep_stale_draft_attachment_workspace(&workspace, after_ttl),
            2
        );
        assert!(!staging.exists());
        assert!(!completed.exists());
        assert!(workspace.exists());
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn later_upload_keeps_stale_but_still_active_completed_draft() {
        let workspace = test_workspace("long-lived-composer");
        let active_upload = super::completed_root(&workspace).join("desktop_attach_active_old");
        let active_file = active_upload.join("old.txt");
        std::fs::create_dir_all(&active_upload).unwrap();
        std::fs::write(&active_file, b"still referenced by composer").unwrap();
        set_file_modified(
            &active_file,
            SystemTime::now()
                .checked_sub(STALE_ATTACHMENT_AGE + Duration::from_secs(1))
                .unwrap(),
        );
        assert!(super::stale_upload_directory(
            &active_upload,
            &std::fs::symlink_metadata(&active_upload).unwrap(),
            SystemTime::now()
        ));

        let new_bytes = b"new upload";
        append_draft_chunk_in_workspace(
            &workspace,
            "desktop_attach_new_upload",
            "new.txt",
            0,
            new_bytes.len(),
            new_bytes,
            true,
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read(active_upload.join("old.txt")).unwrap(),
            b"still referenced by composer"
        );
        std::fs::remove_dir_all(workspace).unwrap();
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

    #[tokio::test]
    async fn failed_duplicate_upload_cleanup_preserves_completed_attachment() {
        let workspace = test_workspace("duplicate-id");
        let upload_id = "desktop_attach_duplicate";
        let original = append_chunk(
            &workspace,
            upload_id,
            "original.txt",
            0,
            8,
            b"original",
            true,
        )
        .await
        .unwrap()
        .unwrap();

        assert!(append_chunk(
            &workspace,
            upload_id,
            "replacement.txt",
            0,
            11,
            b"replacement",
            true,
        )
        .await
        .is_err());
        abort_staging_upload(&workspace, upload_id).await.unwrap();

        assert_eq!(std::fs::read(&original.path).unwrap(), b"original");
        assert!(!upload_staging_dir(&workspace, upload_id).exists());
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn adopts_sessionless_draft_into_target_workspace_idempotently() {
        let source_workspace = test_workspace("draft-source");
        let target_workspace = test_workspace("draft-target");
        let upload_id = "desktop_attach_adopt_draft";
        let bytes = b"draft attachment";
        let source = append_chunk(
            &source_workspace,
            upload_id,
            "draft.txt",
            0,
            bytes.len(),
            bytes,
            true,
        )
        .await
        .unwrap()
        .unwrap();

        let adopted = adopt_upload(&source_workspace, &target_workspace, upload_id)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&adopted.path).unwrap(), bytes);
        assert!(!std::path::Path::new(&source.path).exists());
        assert!(std::path::Path::new(&adopted.path).starts_with(&target_workspace));

        let retried = adopt_upload(&source_workspace, &target_workspace, upload_id)
            .await
            .unwrap();
        assert_eq!(retried.path, adopted.path);

        if source_workspace.exists() {
            std::fs::remove_dir_all(source_workspace).unwrap();
        }
        std::fs::remove_dir_all(target_workspace).unwrap();
    }

    #[test]
    fn conversation_attachment_references_are_scoped_by_session_and_replaceable() {
        let workspace = test_workspace("conversation-reference");
        let first = workspace.join("attachments").join("one.txt");
        let second = workspace.join("attachments").join("two.txt");
        let replacement = workspace.join("attachments").join("replacement.txt");
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        std::fs::write(&first, b"one").unwrap();
        std::fs::write(&second, b"two").unwrap();
        std::fs::write(&replacement, b"replacement").unwrap();

        record_conversation_attachments(
            &workspace,
            "session-a",
            3,
            "first",
            vec![ConversationAttachmentReference {
                basename: "one.txt".into(),
                path: first.to_string_lossy().into_owned(),
            }],
        )
        .unwrap();
        assert_eq!(
            conversation_attachment_names_for_display_prefix(
                &workspace,
                "session-a",
                "first",
                false,
            )
            .unwrap(),
            vec!["one.txt"]
        );
        record_conversation_attachments(
            &workspace,
            "session-b",
            3,
            "second",
            vec![ConversationAttachmentReference {
                basename: "two.txt".into(),
                path: second.to_string_lossy().into_owned(),
            }],
        )
        .unwrap();

        assert_eq!(
            resolve_conversation_attachment(
                &workspace,
                "session-a",
                false,
                3,
                0,
                "one.txt",
                "frontend display text may differ",
            )
            .unwrap(),
            first
        );
        assert_eq!(
            resolve_conversation_attachment(
                &workspace,
                "session-b",
                false,
                3,
                0,
                "two.txt",
                "second",
            )
            .unwrap(),
            second
        );
        record_conversation_attachments(
            &workspace,
            "session-a",
            3,
            "replacement",
            vec![ConversationAttachmentReference {
                basename: "replacement.txt".into(),
                path: replacement.to_string_lossy().into_owned(),
            }],
        )
        .unwrap();
        assert_eq!(
            resolve_conversation_attachment(
                &workspace,
                "session-a",
                false,
                3,
                0,
                "replacement.txt",
                "another frontend display",
            )
            .unwrap(),
            replacement
        );
        assert!(resolve_conversation_attachment(
            &workspace,
            "session-a",
            false,
            3,
            0,
            "one.txt",
            "first",
        )
        .is_err());
        assert_eq!(
            resolve_conversation_attachment(
                &workspace,
                "session-b",
                false,
                3,
                0,
                "two.txt",
                "second",
            )
            .unwrap(),
            second
        );
        assert!(resolve_conversation_attachment(
            &workspace,
            "session-a",
            false,
            3,
            0,
            "two.txt",
            "first",
        )
        .is_err());
        assert!(resolve_conversation_attachment(
            &workspace,
            "session-b",
            false,
            4,
            0,
            "two.txt",
            "second",
        )
        .is_err());
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn legacy_unscoped_references_are_only_available_to_isolated_chat_workspaces() {
        let workspace = test_workspace("legacy-conversation-reference");
        let file = workspace.join("attachments").join("legacy.txt");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"legacy").unwrap();
        let refs_path = conversation_attachment_refs_path(&workspace);
        std::fs::create_dir_all(refs_path.parent().unwrap()).unwrap();
        std::fs::write(
            &refs_path,
            serde_json::to_vec(&vec![ConversationAttachmentRecord {
                session_id: None,
                message_index: 0,
                display_text: "legacy display".into(),
                attachments: vec![ConversationAttachmentReference {
                    basename: "legacy.txt".into(),
                    path: file.to_string_lossy().into_owned(),
                }],
            }])
            .unwrap(),
        )
        .unwrap();

        assert_eq!(
            conversation_attachment_names_for_display_prefix(
                &workspace,
                "chat-session",
                "legacy",
                true,
            )
            .unwrap(),
            vec!["legacy.txt"]
        );
        assert_eq!(
            resolve_conversation_attachment(
                &workspace,
                "chat-session",
                true,
                0,
                0,
                "legacy.txt",
                "legacy display",
            )
            .unwrap(),
            file
        );
        assert!(resolve_conversation_attachment(
            &workspace,
            "scheduled-session",
            false,
            0,
            0,
            "legacy.txt",
            "legacy display",
        )
        .is_err());
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn concurrent_sessions_do_not_lose_records_in_a_shared_workspace() {
        let workspace = test_workspace("concurrent-conversation-reference");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let handles = ["session-a", "session-b"].map(|session_id| {
            let workspace = workspace.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let file = workspace
                    .join("attachments")
                    .join(format!("{session_id}.txt"));
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(&file, session_id.as_bytes()).unwrap();
                barrier.wait();
                record_conversation_attachments(
                    &workspace,
                    session_id,
                    0,
                    session_id,
                    vec![ConversationAttachmentReference {
                        basename: format!("{session_id}.txt"),
                        path: file.to_string_lossy().into_owned(),
                    }],
                )
                .unwrap();
            })
        });
        barrier.wait();
        for handle in handles {
            handle.join().unwrap();
        }

        for session_id in ["session-a", "session-b"] {
            assert!(resolve_conversation_attachment(
                &workspace,
                session_id,
                false,
                0,
                0,
                &format!("{session_id}.txt"),
                "display text is not an identity",
            )
            .is_ok());
        }
        std::fs::remove_dir_all(workspace).unwrap();
    }
}
