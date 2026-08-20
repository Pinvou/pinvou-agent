use std::fs::File;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplaceState {
    Committed,
    RolledBack,
    RecoveryRequired,
}

#[derive(Debug)]
pub(crate) struct ReplaceError {
    state: ReplaceState,
    source: io::Error,
}

impl ReplaceError {
    pub(crate) fn new(state: ReplaceState, source: io::Error) -> Self {
        Self { state, source }
    }

    pub(crate) fn state(&self) -> ReplaceState {
        self.state
    }

    pub(crate) fn into_io_error(self) -> io::Error {
        io::Error::new(self.source.kind(), self.to_string())
    }
}

impl std::fmt::Display for ReplaceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "atomic replace {:?}: {}",
            self.state, self.source
        )
    }
}

impl std::error::Error for ReplaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

pub(crate) type ReplaceResult = Result<ReplaceState, ReplaceError>;

pub(crate) fn replace_file_atomically(tmp: &Path, target: &Path, backup: &Path) -> ReplaceResult {
    replace_file_atomically_impl(tmp, target, backup)
}

/// Converge a layout left by an interrupted Windows replacement. Callers must
/// hold the same lifecycle lock used for writes so active temporary files are
/// never mistaken for recovery candidates.
pub(crate) fn recover_interrupted_replace(
    replacement: &Path,
    target: &Path,
    backup: &Path,
) -> ReplaceResult {
    if target.is_file() {
        return Ok(ReplaceState::Committed);
    }
    if backup.is_file() {
        return promote_replacement(backup, target).map_or_else(
            |error| {
                Err(ReplaceError::new(
                    ReplaceState::RecoveryRequired,
                    error.into_io_error(),
                ))
            },
            |_| {
                Err(ReplaceError::new(
                    ReplaceState::RolledBack,
                    io::Error::new(
                        io::ErrorKind::Other,
                        "interrupted replacement was rolled back",
                    ),
                ))
            },
        );
    }
    if replacement.is_file() {
        return promote_replacement(replacement, target);
    }
    Err(ReplaceError::new(
        ReplaceState::RecoveryRequired,
        io::Error::new(
            io::ErrorKind::NotFound,
            "interrupted replacement has no recoverable candidate",
        ),
    ))
}

/// 原子写文件：先写同目录临时文件并 sync，再原子替换目标。
///
/// 直接 `std::fs::write(target)` 在进程被中断（强杀/崩溃）时会留下截断或
/// 半写的目标文件；对持久化的会话状态（如 `_session_mode_states.json`、
/// settings.json）而言，一个损坏的目标会让启动恢复静默失败，表现为
/// "切换成功但重启后丢失"。tmp + rename 保证目标要么是旧完整内容、
/// 要么是新完整内容，永无中间态。失败清理按替换终态区分（对齐
/// artifacts 版）：不得把装着旧完整内容的恢复候选一并删掉。
pub(crate) fn atomic_write(path: &Path, content: &[u8]) -> io::Result<()> {
    use std::io::Write as _;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("state.json");
    let token = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let tmp = parent.join(format!(".{file_name}.tmp-{token}"));
    let backup = parent.join(format!(".{file_name}.bak-{token}"));

    let stage_result = (|| -> io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(content)?;
        file.sync_all()?;
        drop(file);
        Ok(())
    })();
    if let Err(error) = stage_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }

    // Windows 状态机内部自带「目标不存在 → promote（单文件移动）」分支，
    // POSIX 实现就是 rename，无需外层再区分首写/覆盖。
    match replace_file_atomically(&tmp, path, &backup) {
        Ok(ReplaceState::Committed) => {
            let _ = std::fs::remove_file(&backup);
            if let Ok(dir) = std::fs::File::open(parent) {
                let _ = dir.sync_all();
            }
            Ok(())
        }
        Ok(state) => Err(io::Error::other(format!(
            "unexpected successful replacement state: {state:?}"
        ))),
        Err(error) => {
            // RolledBack：布局已收敛回旧内容，tmp/backup 是残留垃圾；
            // RecoveryRequired 且目标路径仍被占用（如目录占位）：恢复永无
            // 可能，同样是垃圾；目标真正丢失的 RecoveryRequired 保留候选，
            // 供恢复流程 promote——这正是不能无条件清理的原因。
            if error.state() == ReplaceState::RolledBack
                || (error.state() == ReplaceState::RecoveryRequired && path.exists())
            {
                let _ = std::fs::remove_file(&tmp);
                let _ = std::fs::remove_file(&backup);
            }
            Err(error.into_io_error())
        }
    }
}

/// 以 0600 权限创建（或截断）文件：写入含明文密钥的 CLI 配置时**直接**以
/// 0600 创建，避免「先按默认 umask 0644 写、再收紧」的暴露窗口（复审低危 4）；
/// Windows 无 POSIX 权限概念，忽略权限位。
pub(crate) fn create_secret_file(path: &Path) -> io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(windows)]
fn replace_file_atomically_impl(tmp: &Path, target: &Path, backup: &Path) -> ReplaceResult {
    replace_file_atomically_with(tmp, target, backup, system_replace_file)
}

#[cfg(windows)]
fn system_replace_file(target: &Path, replacement: &Path, backup: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_WRITE_THROUGH};

    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>()
    };
    let target_wide = wide(target);
    let tmp_wide = wide(replacement);
    let backup_wide = wide(backup);
    let replaced = unsafe {
        ReplaceFileW(
            target_wide.as_ptr(),
            tmp_wide.as_ptr(),
            backup_wide.as_ptr(),
            REPLACEFILE_WRITE_THROUGH,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
pub(crate) fn replace_file_atomically_with<F>(
    tmp: &Path,
    target: &Path,
    backup: &Path,
    replace: F,
) -> ReplaceResult
where
    F: FnOnce(&Path, &Path, &Path) -> io::Result<()>,
{
    if !target.exists() {
        return promote_replacement(tmp, target);
    }

    // The target is authoritative before ReplaceFileW starts. A stale backup
    // from an earlier completed operation is no longer a recovery candidate.
    let _ = std::fs::remove_file(backup);
    match replace(target, tmp, backup) {
        Ok(()) => {
            let _ = std::fs::remove_file(backup);
            Ok(ReplaceState::Committed)
        }
        Err(error) => converge_failed_windows_replace(tmp, target, backup, error),
    }
}

fn promote_replacement(replacement: &Path, target: &Path) -> ReplaceResult {
    std::fs::rename(replacement, target).map_or_else(
        |error| {
            let state = if target.is_file() {
                ReplaceState::RolledBack
            } else {
                ReplaceState::RecoveryRequired
            };
            Err(ReplaceError::new(state, error))
        },
        |_| Ok(ReplaceState::Committed),
    )
}

#[cfg(windows)]
fn converge_failed_windows_replace(
    replacement: &Path,
    target: &Path,
    backup: &Path,
    replace_error: io::Error,
) -> ReplaceResult {
    // ReplaceFileW documents partially-mutated layouts for errors 1175/1176/1177.
    // First restore an authoritative name. Until that is confirmed, neither rollback nor
    // replacement is deleted.
    if !target.is_file() {
        if backup.is_file() {
            if let Err(restore_error) = promote_replacement(backup, target) {
                return Err(ReplaceError::new(
                    ReplaceState::RecoveryRequired,
                    io::Error::new(
                        io::ErrorKind::Other,
                        format!(
                            "replace failed ({replace_error}); restoring backup failed: {restore_error}"
                        ),
                    ),
                ));
            }
        } else if replacement.is_file() {
            // No old authority survived, so the durable replacement is the only recoverable
            // candidate. Promoting it completes the write rather than returning an empty state.
            return promote_replacement(replacement, target);
        }
    }

    if !target.is_file() {
        return Err(ReplaceError::new(
            ReplaceState::RecoveryRequired,
            io::Error::new(
                replace_error.kind(),
                format!("atomic replace left no authoritative file: {replace_error}"),
            ),
        ));
    }

    // The old target either retained its name or was restored from backup.
    // Preserve every remaining candidate; callers may clean them only after
    // observing RolledBack and completing their own lifecycle transaction.
    Err(ReplaceError::new(ReplaceState::RolledBack, replace_error))
}

#[cfg(not(windows))]
fn replace_file_atomically_impl(tmp: &Path, target: &Path, _backup: &Path) -> ReplaceResult {
    promote_replacement(tmp, target)
}

pub(crate) fn reserved_target_is_unchanged(file: &File, path: &Path) -> bool {
    reserved_target_is_unchanged_impl(file, path)
}

#[cfg(unix)]
fn reserved_target_is_unchanged_impl(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    let (Ok(opened), Ok(named)) = (file.metadata(), std::fs::symlink_metadata(path)) else {
        return false;
    };
    named.file_type().is_file() && opened.dev() == named.dev() && opened.ino() == named.ino()
}

#[cfg(windows)]
fn reserved_target_is_unchanged_impl(_file: &File, path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt as _;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let Ok(named) = std::fs::symlink_metadata(path) else {
        return false;
    };
    named.file_type().is_file() && named.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
}

#[cfg(not(any(unix, windows)))]
fn reserved_target_is_unchanged_impl(_file: &File, path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    use super::atomic_write;

    #[test]
    fn atomic_write_round_trips_content() {
        let dir = std::env::temp_dir().join(format!("pinvou3-atomic-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");

        // 首次创建（目标不存在）：Windows 上必须走普通 rename，不能失败。
        let target = dir.join("_code_mode_states.json");
        atomic_write(&target, br#"{"session-1":"yolo"}"#).expect("first write");
        assert_eq!(
            std::fs::read_to_string(&target).expect("read back"),
            r#"{"session-1":"yolo"}"#
        );

        // 覆盖写入仍保持完整内容（原子替换语义）。
        atomic_write(&target, br#"{"session-1":"plan","session-2":"yolo"}"#).expect("rewrite");
        assert_eq!(
            std::fs::read_to_string(&target).expect("read rewritten"),
            r#"{"session-1":"plan","session-2":"yolo"}"#
        );

        // 成功后不残留临时/备份文件。
        let leftover: Vec<_> = std::fs::read_dir(&dir)
            .expect("list dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name != "_code_mode_states.json"
            })
            .collect();
        assert!(leftover.is_empty(), "leftover files: {leftover:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_failure_is_reported_without_residue() {
        let dir =
            std::env::temp_dir().join(format!("pinvou3-atomic-write-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");

        // 父级是普通文件 → tmp 无法创建，写入必须报错且不留任何残留。
        let file_as_parent = dir.join("not-a-dir");
        std::fs::write(&file_as_parent, "x").expect("seed parent file");
        let target = file_as_parent.join("state.json");
        assert!(atomic_write(&target, b"new").is_err());
        assert_eq!(
            std::fs::read_to_string(&file_as_parent).expect("parent untouched"),
            "x"
        );

        let leftover: Vec<_> = std::fs::read_dir(&dir)
            .expect("list dir")
            .filter_map(|entry| entry.ok())
            .collect();
        assert_eq!(leftover.len(), 1, "no tmp/backup residue: {leftover:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    pub(crate) fn try_link_file(target: &Path, link: &Path) -> bool {
        try_link_file_impl(target, link)
    }

    pub(crate) fn try_link_dir(target: &Path, link: &Path) -> bool {
        try_link_dir_impl(target, link)
    }

    pub(crate) fn remove_dir_link(link: &Path) {
        remove_dir_link_impl(link)
    }

    #[test]
    fn failed_atomic_replace_preserves_the_authoritative_target() {
        let root = std::env::temp_dir().join(format!(
            "pinvou-atomic-replace-failure-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("profile.json");
        let missing_tmp = root.join("missing.tmp");
        let backup = root.join("profile.bak");
        std::fs::write(&target, "authoritative").unwrap();

        assert!(super::replace_file_atomically(&missing_tmp, &target, &backup).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "authoritative");
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    fn windows_replace_fixture(
        name: &str,
    ) -> (
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let root = std::env::temp_dir().join(format!(
            "pinvou-windows-replace-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("memory.json");
        let replacement = root.join("memory.tmp");
        let backup = root.join("memory.bak");
        (root, target, replacement, backup)
    }

    #[cfg(windows)]
    #[test]
    fn windows_atomic_replace_covers_first_write_and_success_cleanup() {
        let (root, target, replacement, backup) = windows_replace_fixture("success");
        std::fs::write(&replacement, "first").unwrap();
        super::replace_file_atomically(&replacement, &target, &backup).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "first");

        std::fs::write(&replacement, "second").unwrap();
        super::replace_file_atomically(&replacement, &target, &backup).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "second");
        assert!(!replacement.exists());
        assert!(!backup.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_1175_retains_the_two_official_original_names() {
        let (root, target, replacement, backup) = windows_replace_fixture("1175");
        std::fs::write(&target, "old").unwrap();
        std::fs::write(&replacement, "new").unwrap();

        let error =
            super::replace_file_atomically_with(&replacement, &target, &backup, |_, _, _| {
                Err(std::io::Error::from_raw_os_error(1175))
            })
            .unwrap_err();

        assert_eq!(error.state(), super::ReplaceState::RolledBack);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(&replacement).unwrap(), "new");
        assert!(!backup.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_1176_with_backup_name_retains_the_two_official_original_names() {
        let (root, target, replacement, backup) = windows_replace_fixture("1176");
        std::fs::write(&target, "old").unwrap();
        std::fs::write(&replacement, "new").unwrap();

        let error =
            super::replace_file_atomically_with(&replacement, &target, &backup, |_, _, _| {
                Err(std::io::Error::from_raw_os_error(1176))
            })
            .unwrap_err();

        assert_eq!(error.state(), super::ReplaceState::RolledBack);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(&replacement).unwrap(), "new");
        assert!(!backup.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_1177_restores_the_official_backup_layout() {
        let (root, target, replacement, backup) = windows_replace_fixture("1177");
        std::fs::write(&target, "old").unwrap();
        std::fs::write(&replacement, "new").unwrap();

        let error = super::replace_file_atomically_with(
            &replacement,
            &target,
            &backup,
            |target, _, backup| {
                std::fs::rename(target, backup)?;
                Err(std::io::Error::from_raw_os_error(1177))
            },
        )
        .unwrap_err();

        assert_eq!(error.state(), super::ReplaceState::RolledBack);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(&replacement).unwrap(), "new");
        assert!(!backup.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_partial_replace_promotes_only_surviving_replacement() {
        let (root, target, replacement, backup) = windows_replace_fixture("replacement-only");
        std::fs::write(&replacement, "new").unwrap();
        let state = super::converge_failed_windows_replace(
            &replacement,
            &target,
            &backup,
            std::io::Error::from_raw_os_error(1177),
        )
        .unwrap();
        assert_eq!(state, super::ReplaceState::Committed);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        assert!(!replacement.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_partial_replace_keeps_candidates_when_no_authority_can_be_restored() {
        let (root, target, replacement, backup) = windows_replace_fixture("no-candidate");
        let result = super::converge_failed_windows_replace(
            &replacement,
            &target,
            &backup,
            std::io::Error::from_raw_os_error(1176),
        );
        assert!(result.is_err());
        assert!(!target.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_partial_replace_preserves_occupied_recovery_candidates() {
        use std::cell::RefCell;
        use std::os::windows::fs::OpenOptionsExt as _;

        let (root, target, replacement, backup) = windows_replace_fixture("occupied-backup");
        std::fs::write(&target, "old").unwrap();
        std::fs::write(&replacement, "new").unwrap();
        let occupied = RefCell::new(None);
        let error = super::replace_file_atomically_with(
            &replacement,
            &target,
            &backup,
            |target, _, backup| {
                std::fs::rename(target, backup)?;
                *occupied.borrow_mut() = Some(
                    std::fs::OpenOptions::new()
                        .read(true)
                        .share_mode(0)
                        .open(backup)?,
                );
                Err(std::io::Error::from_raw_os_error(1177))
            },
        )
        .unwrap_err();
        assert_eq!(error.state(), super::ReplaceState::RecoveryRequired);
        assert!(!target.exists());
        assert!(replacement.exists());
        assert!(backup.exists());
        drop(occupied.borrow_mut().take());

        let recovered =
            super::recover_interrupted_replace(&replacement, &target, &backup).unwrap_err();
        assert_eq!(recovered.state(), super::ReplaceState::RolledBack);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(&replacement).unwrap(), "new");
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_failed_first_promotion_preserves_the_complete_replacement() {
        let (root, target, replacement, backup) = windows_replace_fixture("promotion-blocked");
        std::fs::write(&replacement, "complete-new-value").unwrap();
        // 目标名被目录占用:target.exists() 为 true,走 ReplaceFileW(目标是目录,
        // 必然失败)→ 收敛分支 promote_replacement,rename(文件→目录名)同样必然
        // ERROR_ACCESS_DENIED,且 target.is_file() 为 false → RecoveryRequired。
        // 此前版本用 share_mode(0) 独占打开 replacement 期望首写提升的 rename
        // 失败,但 NTFS 的 rename 只改目录项、不受源文件独占句柄影响,Windows CI
        // 上 rename 直接成功,前提不成立(与 memory 侧
        // memory_write_cleans_tmp_backup_on_permanently_occupied_target 同构)。
        std::fs::create_dir(&target).unwrap();

        let error = super::replace_file_atomically(&replacement, &target, &backup).unwrap_err();

        assert_eq!(error.state(), super::ReplaceState::RecoveryRequired);
        assert!(target.is_dir());
        assert_eq!(
            std::fs::read_to_string(&replacement).unwrap(),
            "complete-new-value"
        );
        assert!(!backup.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_atomic_replace_never_exposes_a_partial_target_to_readers() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let (root, target, replacement, backup) = windows_replace_fixture("concurrent-reader");
        let old = "a".repeat(32 * 1024);
        let new = "b".repeat(32 * 1024);
        std::fs::write(&target, &old).unwrap();
        let running = Arc::new(AtomicBool::new(true));
        let reader_running = Arc::clone(&running);
        let reader_target = target.clone();
        let old_for_reader = old.clone();
        let new_for_reader = new.clone();
        let reader = std::thread::spawn(move || {
            while reader_running.load(Ordering::Acquire) {
                if let Ok(value) = std::fs::read_to_string(&reader_target) {
                    assert!(value == old_for_reader || value == new_for_reader);
                }
            }
        });

        for index in 0..32 {
            let value = if index % 2 == 0 { &new } else { &old };
            std::fs::write(&replacement, value).unwrap();
            super::replace_file_atomically(&replacement, &target, &backup).unwrap();
        }
        running.store(false, Ordering::Release);
        reader.join().unwrap();
        let final_value = std::fs::read_to_string(&target).unwrap();
        assert!(final_value == old || final_value == new);
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    fn try_link_file_impl(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn try_link_file_impl(target: &Path, link: &Path) -> bool {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }

    #[cfg(not(any(unix, windows)))]
    fn try_link_file_impl(_target: &Path, _link: &Path) -> bool {
        false
    }

    #[cfg(unix)]
    fn try_link_dir_impl(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn try_link_dir_impl(target: &Path, link: &Path) -> bool {
        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            return true;
        }
        std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                &link.to_string_lossy(),
                &target.to_string_lossy(),
            ])
            .status()
            .is_ok_and(|status| status.success())
    }

    #[cfg(not(any(unix, windows)))]
    fn try_link_dir_impl(_target: &Path, _link: &Path) -> bool {
        false
    }

    #[cfg(windows)]
    fn remove_dir_link_impl(link: &Path) {
        let _ = std::fs::remove_dir(link);
    }

    #[cfg(not(windows))]
    fn remove_dir_link_impl(link: &Path) {
        let _ = std::fs::remove_file(link);
    }
}
