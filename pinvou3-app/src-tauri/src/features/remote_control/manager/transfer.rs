//! Browser-side attachment and session transfer buffers.
//!
//! Every helper here mutates `Inner` fields through a borrowed `&mut Inner`
//! (or inspects them via `&Inner`). They never acquire the mutex themselves:
//! the facade in `mod.rs` holds the `parking_lot::Mutex` guard and lends the
//! state, preserving the single-lock concurrency contract.
// architecture-guard: allow-target-cfg -- Unix-only symlink regression proves startup cleanup never follows a linked snapshot root; OS metadata checks live in platform::filesystem

use std::collections::HashSet;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::PathBuf;
use std::time::Instant;

use super::Inner;
use super::{
    CachedWebAttachment, WebAttachmentUpload, WebSessionDownload, WebSessionDownloadChunk,
    MAX_WEB_ATTACHMENT_UPLOADS, MAX_WEB_ATTACHMENT_UPLOAD_TOTAL_BYTES, MAX_WEB_SESSION_DOWNLOADS,
    MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES, WEB_ATTACHMENT_UPLOAD_DIR_PREFIX,
    WEB_SESSION_TRANSFER_TTL,
};
use crate::platform::paths;

pub(super) fn remove_web_attachment_upload_dir(cached: &CachedWebAttachment) {
    if let Some(dir) = &cached.upload_dir {
        let _ = std::fs::remove_dir_all(dir);
    }
}

pub(super) fn clear_web_attachments(inner: &mut Inner) {
    for (_, cached) in inner.web_attachments.drain() {
        remove_web_attachment_upload_dir(&cached);
    }
    inner.web_attachment_order.clear();
    inner.web_attachment_bytes = 0;
    inner.web_attachment_uploads.clear();
    inner.web_attachment_upload_order.clear();
    inner.web_session_uploads.clear();
    inner.web_session_upload_order.clear();
    inner.web_session_downloads.clear();
    inner.web_session_download_order.clear();
}

pub(super) fn remove_cached_web_attachment(inner: &mut Inner, handle: &str) -> bool {
    let Some(cached) = inner.web_attachments.remove(handle) else {
        return false;
    };
    inner.web_attachment_bytes = inner.web_attachment_bytes.saturating_sub(cached.bytes);
    inner
        .web_attachment_order
        .retain(|candidate| candidate != handle);
    remove_web_attachment_upload_dir(&cached);
    true
}

pub(super) fn request_web_attachment_discard(inner: &mut Inner, handle: &str) {
    if let Some(cached) = inner.web_attachments.get_mut(handle) {
        if cached.reservation_id.is_some() {
            cached.discard_requested = true;
            return;
        }
    }
    remove_cached_web_attachment(inner, handle);
}

pub(super) fn finish_web_attachment_reservation_inner(
    inner: &mut Inner,
    reservation_id: &str,
    handles: &[String],
    consume: bool,
) -> Result<(), String> {
    for handle in handles {
        let Some(cached) = inner.web_attachments.get(handle) else {
            // Stop/rotate deliberately clears the whole lease cache.
            continue;
        };
        if cached.reservation_id.as_deref() != Some(reservation_id) {
            return Err(format!("远程控制附件预留已不再持有句柄：{handle}"));
        }
    }
    for handle in handles {
        let should_remove = consume
            || inner
                .web_attachments
                .get(handle)
                .is_some_and(|cached| cached.discard_requested);
        if should_remove {
            remove_cached_web_attachment(inner, handle);
        } else if let Some(cached) = inner.web_attachments.get_mut(handle) {
            cached.reservation_id = None;
        }
    }
    Ok(())
}

pub(super) fn append_web_attachment_upload_chunk(
    inner: &mut Inner,
    upload_id: &str,
    file_name: &str,
    offset: usize,
    total: usize,
    data: &[u8],
    commit: bool,
) -> Result<Option<(String, Vec<u8>)>, String> {
    if upload_id.len() < 8
        || upload_id.len() > 128
        || !upload_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("远程控制附件上传 ID 无效".into());
    }
    let file_name = file_name.trim();
    if file_name.is_empty() || file_name.chars().count() > 255 {
        return Err("远程控制附件上传需要有效的文件名".into());
    }
    if total as u64 > crate::features::files::file_ingest::MAX_FILE_BYTES {
        return Err(format!(
            "文件超过附件 {} MB 上限",
            crate::features::files::file_ingest::MAX_FILE_BYTES / (1024 * 1024)
        ));
    }
    prune_expired_web_session_transfers(inner);
    if offset == 0 {
        inner.web_attachment_uploads.remove(upload_id);
        while inner.web_attachment_uploads.len() >= MAX_WEB_ATTACHMENT_UPLOADS {
            let Some(oldest) = inner.web_attachment_upload_order.pop_front() else {
                break;
            };
            inner.web_attachment_uploads.remove(&oldest);
        }
        inner
            .web_attachment_upload_order
            .retain(|id| id != upload_id);
        inner
            .web_attachment_upload_order
            .push_back(upload_id.to_string());
        inner.web_attachment_uploads.insert(
            upload_id.to_string(),
            WebAttachmentUpload {
                file_name: file_name.to_string(),
                total,
                data: Vec::with_capacity(total.min(4 * 1024 * 1024)),
                last_touched: Instant::now(),
            },
        );
    }
    let retained_upload_bytes: usize = inner
        .web_attachment_uploads
        .iter()
        .filter(|(id, _)| id.as_str() != upload_id)
        .map(|(_, upload)| upload.data.len())
        .sum();
    if retained_upload_bytes
        .saturating_add(offset)
        .saturating_add(data.len())
        > MAX_WEB_ATTACHMENT_UPLOAD_TOTAL_BYTES
    {
        return Err("远程控制附件上传缓存超过总容量上限".into());
    }
    let upload = inner
        .web_attachment_uploads
        .get_mut(upload_id)
        .ok_or_else(|| "远程控制附件上传不存在或已过期".to_string())?;
    if upload.file_name != file_name || upload.total != total {
        return Err("远程控制附件上传元数据已变化".into());
    }
    if upload.data.len() != offset {
        return Err(format!(
            "远程控制附件上传预期偏移量为 {}，实际为 {offset}",
            upload.data.len()
        ));
    }
    if upload.data.len().saturating_add(data.len()) > total {
        return Err("远程控制附件上传超过声明大小".into());
    }
    upload.data.extend_from_slice(data);
    upload.last_touched = Instant::now();
    if !commit {
        return Ok(None);
    }
    if upload.data.len() != total {
        return Err(format!(
            "远程控制附件上传不完整：已上传 {} / {total} 字节",
            upload.data.len()
        ));
    }
    let completed = inner
        .web_attachment_uploads
        .remove(upload_id)
        .expect("upload exists above");
    inner
        .web_attachment_upload_order
        .retain(|id| id != upload_id);
    Ok(Some((completed.file_name, completed.data)))
}

pub(super) fn discard_web_attachment_upload(inner: &mut Inner, upload_id: &str) {
    inner.web_attachment_uploads.remove(upload_id);
    inner
        .web_attachment_upload_order
        .retain(|id| id != upload_id);
}

/// 浏览器本机上传的桌面暂存根目录。目录布局 `uploads/webup_<token>/<file>`
/// 与旧版远控上传一致，`verify_upload` E2E 命令可直接校验。
pub(crate) fn web_attachment_uploads_base() -> PathBuf {
    paths::pinvou3_home().join("uploads")
}

/// 清理上一次进程遗留的上传暂存目录（崩溃或强杀时正常清理不会执行）。
/// 只清 `webup_` 前缀，避免波及 uploads 下的其他历史内容。
pub(crate) fn sweep_stale_web_attachment_uploads() {
    let Ok(entries) = std::fs::read_dir(web_attachment_uploads_base()) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(WEB_ATTACHMENT_UPLOAD_DIR_PREFIX)
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Session snapshots live in a dedicated process-owned directory so startup
/// recovery can remove every crash remnant without inspecting user data.
pub(crate) fn web_session_downloads_base() -> PathBuf {
    paths::pinvou3_home().join("web-session-downloads")
}

pub(crate) fn open_web_session_downloads_base(
) -> Result<crate::platform::filesystem::PrivateFileDirectory, String> {
    let base = web_session_downloads_base();
    crate::platform::filesystem::open_private_file_directory(&base)
        .map_err(|error| format!("prepare remote-control Session download directory: {error}"))
}

pub(crate) fn sweep_stale_web_session_downloads() {
    let Ok(directory) = open_web_session_downloads_base() else {
        return;
    };
    sweep_stale_web_session_downloads_in(&directory);
}

fn is_owned_session_snapshot_name(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let Some(download_id) = name.strip_suffix(".json") else {
        return false;
    };
    is_owned_session_download_id(download_id)
}

pub(super) fn is_owned_session_download_id(download_id: &str) -> bool {
    download_id.starts_with("download_")
        && download_id.len() > "download_".len()
        && download_id.len() <= 128
        && download_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn sweep_stale_web_session_downloads_in(
    directory: &crate::platform::filesystem::PrivateFileDirectory,
) {
    let Ok(entries) = directory.entry_names() else {
        return;
    };
    for name in entries {
        if !is_owned_session_snapshot_name(&name) {
            continue;
        }
        let _ = directory.remove_plain_file(&name);
    }
}

pub(super) fn prune_expired_web_session_transfers(inner: &mut Inner) {
    prune_expired_web_session_transfers_at(inner, Instant::now());
}

fn prune_expired_web_session_transfers_at(inner: &mut Inner, now: Instant) {
    inner.web_attachment_uploads.retain(|_, upload| {
        now.saturating_duration_since(upload.last_touched) <= WEB_SESSION_TRANSFER_TTL
    });
    let active_attachment_uploads = inner
        .web_attachment_uploads
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    inner
        .web_attachment_upload_order
        .retain(|id| active_attachment_uploads.contains(id));
    inner.web_session_uploads.retain(|_, upload| {
        now.saturating_duration_since(upload.last_touched) <= WEB_SESSION_TRANSFER_TTL
    });
    let active_uploads = inner
        .web_session_uploads
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    inner
        .web_session_upload_order
        .retain(|id| active_uploads.contains(id));
    let expired_downloads = inner
        .web_session_downloads
        .iter()
        .filter(|(_, download)| {
            now.saturating_duration_since(download.last_touched) > WEB_SESSION_TRANSFER_TTL
        })
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    for id in expired_downloads {
        inner.web_session_downloads.remove(&id);
    }
    let active_downloads = inner
        .web_session_downloads
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    inner
        .web_session_download_order
        .retain(|id| active_downloads.contains(id));
}

pub(super) fn ensure_web_session_download_capacity(
    inner: &Inner,
    reserved_bytes: usize,
) -> Result<(), String> {
    if inner.web_session_downloads.len() >= MAX_WEB_SESSION_DOWNLOADS {
        return Err(format!(
            "远程控制已有 {} 个进行中的会话下载，请等待完成或重试已有下载",
            MAX_WEB_SESSION_DOWNLOADS
        ));
    }
    let active_bytes = inner
        .web_session_downloads
        .values()
        .map(|download| download.reserved_bytes)
        .sum::<usize>();
    if active_bytes.saturating_add(reserved_bytes) > MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES {
        return Err(format!(
            "进行中的远程控制会话下载将超过 {} MiB 总上限",
            MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES / (1024 * 1024)
        ));
    }
    Ok(())
}

pub(super) fn take_web_session_download(
    inner: &mut Inner,
    download_id: &str,
    session_id: &str,
) -> Result<Option<WebSessionDownload>, String> {
    prune_expired_web_session_transfers(inner);
    let Some(download) = inner.web_session_downloads.get(download_id) else {
        return Ok(None);
    };
    if download.session_id != session_id {
        return Err("Session download token belongs to another Session".into());
    }
    inner
        .web_session_download_order
        .retain(|id| id != download_id);
    Ok(inner.web_session_downloads.remove(download_id))
}

pub(super) fn read_web_session_download_chunk(
    inner: &mut Inner,
    download_id: &str,
    session_id: &str,
    offset: usize,
    limit: usize,
) -> Result<WebSessionDownloadChunk, String> {
    prune_expired_web_session_transfers(inner);
    let (total, end, mut file) = {
        let download = inner
            .web_session_downloads
            .get(download_id)
            .ok_or_else(|| "远程控制会话下载不存在或已过期".to_string())?;
        if download.session_id != session_id {
            return Err("远程控制会话下载属于另一个会话".into());
        }
        if !download.ready {
            return Err("远程控制会话下载仍在准备中".into());
        }
        if offset > download.total {
            return Err(format!(
                "Session offset {offset} exceeds payload size {}",
                download.total
            ));
        }
        let total = download.total;
        let end = offset.saturating_add(limit).min(total);
        let file = download
            .file
            .try_clone()
            .map_err(|error| format!("克隆远程控制会话下载句柄失败：{error}"))?;
        (total, end, file)
    };
    file.seek(SeekFrom::Start(offset as u64))
        .map_err(|error| format!("定位远程控制会话下载失败：{error}"))?;
    let mut data = vec![0_u8; end.saturating_sub(offset)];
    file.read_exact(&mut data)
        .map_err(|error| format!("读取远程控制会话下载失败：{error}"))?;
    let eof = end == total;
    if eof {
        inner.web_session_downloads.remove(download_id);
        inner
            .web_session_download_order
            .retain(|id| id != download_id);
    } else if let Some(download) = inner.web_session_downloads.get_mut(download_id) {
        download.last_touched = Instant::now();
    }
    Ok(WebSessionDownloadChunk { total, data, eof })
}

#[cfg(test)]
mod tests {
    use super::prune_expired_web_session_transfers_at;
    #[cfg(any(
        windows,
        target_os = "macos",
        all(target_os = "linux", target_pointer_width = "64")
    ))]
    use super::sweep_stale_web_session_downloads_in;
    use crate::features::remote_control::manager::{
        Inner, WebSessionDownload, WEB_SESSION_TRANSFER_TTL,
    };
    use std::time::{Duration, Instant};
    #[cfg(any(
        windows,
        target_os = "macos",
        all(target_os = "linux", target_pointer_width = "64")
    ))]
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(any(
        windows,
        target_os = "macos",
        all(target_os = "linux", target_pointer_width = "64")
    ))]
    fn test_directory(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "pinvou-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn ttl_pruning_releases_only_the_expired_snapshot_handle() {
        let now = Instant::now();
        let mut inner = Inner::default();
        for (id, last_touched) in [
            (
                "expired",
                now - WEB_SESSION_TRANSFER_TTL - Duration::from_secs(1),
            ),
            ("fresh", now),
        ] {
            inner.web_session_download_order.push_back(id.to_string());
            inner.web_session_downloads.insert(
                id.to_string(),
                WebSessionDownload {
                    session_id: format!("session_{id}"),
                    reservation_id: format!("reservation_{id}"),
                    file: tempfile::tempfile().unwrap(),
                    reserved_bytes: 1,
                    total: 1,
                    ready: true,
                    last_touched,
                },
            );
        }

        prune_expired_web_session_transfers_at(&mut inner, now);

        assert!(!inner.web_session_downloads.contains_key("expired"));
        assert!(inner.web_session_downloads.contains_key("fresh"));
        assert_eq!(inner.web_session_download_order, ["fresh"]);
    }

    #[cfg(any(
        windows,
        target_os = "macos",
        all(target_os = "linux", target_pointer_width = "64")
    ))]
    #[test]
    fn startup_sweep_removes_owned_snapshot_and_preserves_unknown_entries() {
        let directory = test_directory("session-download-startup");
        let unknown_directory = directory.join("download_unknown.json");
        std::fs::create_dir_all(&unknown_directory).unwrap();
        let owned_snapshot = directory.join("download_orphan.json");
        let unknown_json = directory.join("orphan.json");
        let unknown_file = directory.join("notes.txt");
        std::fs::write(&owned_snapshot, b"orphan").unwrap();
        std::fs::write(&unknown_json, b"unknown").unwrap();
        std::fs::write(&unknown_file, b"unknown").unwrap();
        std::fs::write(unknown_directory.join("nested.txt"), b"unknown").unwrap();

        let stable = crate::platform::filesystem::open_private_file_directory(&directory).unwrap();
        sweep_stale_web_session_downloads_in(&stable);
        drop(stable);

        assert!(!owned_snapshot.exists());
        assert!(unknown_json.exists());
        assert!(unknown_file.exists());
        assert!(unknown_directory.join("nested.txt").exists());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(any(
        windows,
        target_os = "macos",
        all(target_os = "linux", target_pointer_width = "64")
    ))]
    #[test]
    fn private_download_directory_is_created_as_a_real_directory() {
        let parent = test_directory("session-download-private");
        let directory = parent.join("web-session-downloads");

        let stable = crate::platform::filesystem::open_private_file_directory(&directory).unwrap();

        let metadata = std::fs::symlink_metadata(&directory).unwrap();
        assert!(metadata.is_dir());
        assert!(!metadata.file_type().is_symlink());
        drop(stable);
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[cfg(any(
        target_os = "macos",
        all(target_os = "linux", target_pointer_width = "64")
    ))]
    #[test]
    fn startup_sweep_refuses_a_symlink_download_root() {
        use std::os::unix::fs::symlink;

        let parent = test_directory("session-download-symlink-root");
        let external = parent.join("external");
        let linked = parent.join("web-session-downloads");
        std::fs::create_dir_all(&external).unwrap();
        let external_snapshot = external.join("download_external.json");
        std::fs::write(&external_snapshot, b"must survive").unwrap();
        symlink(&external, &linked).unwrap();

        assert!(crate::platform::filesystem::open_private_file_directory(&linked).is_err());

        assert!(external_snapshot.exists());
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[cfg(any(
        target_os = "macos",
        all(target_os = "linux", target_pointer_width = "64")
    ))]
    #[test]
    fn rooted_handles_survive_directory_replacement_for_read_cancel_and_reap() {
        use super::{read_web_session_download_chunk, take_web_session_download};
        use std::io::Write as _;
        use std::os::unix::fs::symlink;

        let parent = test_directory("session-download-root-replaced");
        let base = parent.join("web-session-downloads");
        let retired = parent.join("retired-downloads");
        let replacement = parent.join("replacement");
        let directory = crate::platform::filesystem::open_private_file_directory(&base).unwrap();
        let now = Instant::now();
        let mut inner = Inner::default();
        for (id, session_id, content, last_touched) in [
            (
                "download_read",
                "session_read",
                b"original-read".as_slice(),
                now,
            ),
            (
                "download_cancel",
                "session_cancel",
                b"original-cancel".as_slice(),
                now,
            ),
            (
                "download_reap",
                "session_reap",
                b"original-reap".as_slice(),
                now - WEB_SESSION_TRANSFER_TTL - Duration::from_secs(1),
            ),
        ] {
            let mut file = directory
                .create_delete_on_close_file(&format!("{id}.json"))
                .unwrap();
            file.write_all(content).unwrap();
            inner.web_session_download_order.push_back(id.to_string());
            inner.web_session_downloads.insert(
                id.to_string(),
                WebSessionDownload {
                    session_id: session_id.to_string(),
                    reservation_id: format!("reservation_{id}"),
                    file,
                    reserved_bytes: content.len(),
                    total: content.len(),
                    ready: true,
                    last_touched,
                },
            );
        }

        std::fs::rename(&base, &retired).unwrap();
        std::fs::create_dir_all(&replacement).unwrap();
        for id in ["download_read", "download_cancel", "download_reap"] {
            std::fs::write(replacement.join(format!("{id}.json")), b"replacement").unwrap();
        }
        symlink(&replacement, &base).unwrap();

        let chunk = read_web_session_download_chunk(
            &mut inner,
            "download_read",
            "session_read",
            0,
            usize::MAX,
        )
        .unwrap();
        assert_eq!(chunk.data, b"original-read");
        assert!(chunk.eof);
        assert!(!inner.web_session_downloads.contains_key("download_read"));

        let cancelled =
            take_web_session_download(&mut inner, "download_cancel", "session_cancel").unwrap();
        assert!(cancelled.is_some());
        drop(cancelled);

        prune_expired_web_session_transfers_at(&mut inner, now);
        assert!(!inner.web_session_downloads.contains_key("download_reap"));
        assert_eq!(std::fs::read_dir(&retired).unwrap().count(), 0);
        for id in ["download_read", "download_cancel", "download_reap"] {
            assert_eq!(
                std::fs::read(replacement.join(format!("{id}.json"))).unwrap(),
                b"replacement"
            );
        }

        drop(directory);
        std::fs::remove_file(&base).unwrap();
        std::fs::remove_dir_all(parent).unwrap();
    }
}
