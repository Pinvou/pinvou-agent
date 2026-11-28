//! code_checkpoints 的 OS 差异适配层。
//!
//! 目前只承载一件事：文件系统目录再解析点（Windows junction / mount point）
//! 的枚举。快照（add -A）与恢复（checkout-index / clean）路径据此 fail-closed
//! 拒绝——git-for-windows 会把 junction 当普通目录递归，业务层两层语义必须
//! 与之同向（r42 M6）。平台差异用 `cfg` 表达、以显式接口暴露：非 Windows
//! 恒返回空集（无再解析点概念，零开销）。

/// Enumerate directory reparse points (junctions / directory symlinks)
/// under `root`, skipping directory names listed in `skip_dirs`
/// (case-insensitive, the same semantics as SHADOW_EXCLUDES' directory-level
/// exclusions - junctions inside excluded directories never enter the index
/// and are never touched by checkout/clean, so refusing them would only
/// punish pnpm-style junction farms).
///
/// r52 M2 (review round-50 M2 landed): classification no longer relies on
/// the raw reparse bit - Rust std's `is_symlink()` on Windows tests the
/// reparse bit regardless of tag, and OneDrive Files-On-Demand/KFM marks
/// synced directories with `IO_REPARSE_TAG_CLOUD_*` (hydrated and normally
/// traversable), so the raw-bit test misclassified every cloud-managed
/// directory as a junction and the snapshot/restore/preview paths failed
/// closed permanently with an unactionable cause. This probe reads the
/// reparse tag via `FindFirstFileW`: only `IO_REPARSE_TAG_MOUNT_POINT`
/// (junctions) and directory-form `IO_REPARSE_TAG_SYMLINK` are refused;
/// cloud placeholders and any other tag pass; an undecidable probe (path
/// lost to a race / permissions) refuses the entry fail-closed.
///
/// The entry budget is shared with the size walk
/// ([`super::SIZE_WALK_MAX_ENTRIES`], 200k): an enumeration that cannot
/// complete within budget returns Err - with incomplete coverage "not found
/// does not mean absent", so callers must fail closed (r52 minor 9: the
/// walk used to be uncapped and silently skipped subtrees on read_dir
/// errors).
pub(crate) fn reparse_dir_paths_under(
    root: &std::path::Path,
    skip_dirs: &std::collections::HashSet<&str>,
) -> anyhow::Result<Vec<std::path::PathBuf>> {
    #[cfg(not(windows))]
    {
        let _ = (root, skip_dirs);
        Ok(Vec::new())
    }
    #[cfg(windows)]
    {
        walk_reparse_dirs(root, skip_dirs, super::SIZE_WALK_MAX_ENTRIES)
    }
}

/// Pure classifier (unit-testable): whether this reparse tag plus the
/// directory bit requires a refusal. Only junctions and directory-form
/// symlinks are refused; cloud placeholders (IO_REPARSE_TAG_CLOUD_* etc.)
/// and unknown tags are not traversal hazards and pass.
#[cfg(windows)]
fn reparse_tag_requires_refusal(tag: u32, is_directory: bool) -> bool {
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;
    tag == IO_REPARSE_TAG_MOUNT_POINT || (tag == IO_REPARSE_TAG_SYMLINK && is_directory)
}

/// Read the path's live attribute bits plus the reparse tag via
/// FindFirstFileW (dwReserved0 is defined only while the reparse bit is
/// set; a race that clears the bit between enumeration and probe passes
/// with tag=0). Err = undecidable (path vanished / permissions / FFI
/// failure).
#[cfg(windows)]
fn probe_reparse_attributes(path: &std::path::Path) -> std::io::Result<(u32, u32)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{FindClose, FindFirstFileW, WIN32_FIND_DATAW};

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let mut data: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let handle = unsafe { FindFirstFileW(wide.as_ptr(), &mut data) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    // r52 P2 (review): a FindFirstFileW search handle must be released with
    // FindClose - CloseHandle on a search handle is an undefined resource
    // lifetime operation (the kernel search context is owned by the Find*
    // pair). The read is already complete here, so a failure stays a
    // non-fatal warn.
    if unsafe { FindClose(handle) } == 0 {
        log::warn!(
            "[pinvou3][checkpoints] reparse probe FindClose failed for {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        );
    }
    if data.dwFileAttributes & 0x0400 == 0 {
        return Ok((data.dwFileAttributes, 0));
    }
    Ok((data.dwFileAttributes, data.dwReserved0))
}

/// The budgeted enumeration implementation (`max_entries` is parameterized
/// so tests can pin the over-budget behavior).
#[cfg(windows)]
pub(crate) fn walk_reparse_dirs(
    root: &std::path::Path,
    skip_dirs: &std::collections::HashSet<&str>,
    max_entries: usize,
) -> anyhow::Result<Vec<std::path::PathBuf>> {
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0010;
    let mut reparse: Vec<std::path::PathBuf> = Vec::new();
    let mut seen = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        // r52 minor 9: a subtree that fails to read must no longer vanish
        // silently - log a warn and continue (one unreadable directory does
        // not make the whole scan incomplete; exceeding the budget does).
        let read = match std::fs::read_dir(&dir) {
            Ok(read) => read,
            Err(error) => {
                log::warn!(
                    "[pinvou3][checkpoints] reparse walk: read_dir({}) failed, subtree skipped: {error}",
                    dir.display()
                );
                continue;
            }
        };
        for entry in read.flatten() {
            seen += 1;
            if seen > max_entries {
                anyhow::bail!(
                    "reparse walk exceeded the {max_entries}-entry budget; coverage is                      incomplete and absence of junctions cannot be proven"
                );
            }
            let Ok(file_type) = entry.file_type() else {
                log::warn!(
                    "[pinvou3][checkpoints] reparse walk: file_type({}) failed, entry skipped",
                    entry.path().display()
                );
                continue;
            };
            if file_type.is_symlink() {
                // std is_symlink = the raw reparse bit (tag-agnostic). Classify
                // by the live FindFirstFileW data; a probe failure refuses the
                // entry fail-closed.
                match probe_reparse_attributes(&entry.path()) {
                    Ok((attributes, tag)) => {
                        let is_directory = attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
                        if reparse_tag_requires_refusal(tag, is_directory) {
                            reparse.push(entry.path());
                        }
                    }
                    Err(error) => {
                        log::warn!(
                            "[pinvou3][checkpoints] reparse probe({}) failed, refusing the entry fail-closed: {error}",
                            entry.path().display()
                        );
                        reparse.push(entry.path());
                    }
                }
                continue;
            }
            if file_type.is_dir() {
                let path = entry.path();
                if !skip_dirs.contains(
                    entry
                        .file_name()
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .as_str(),
                ) {
                    stack.push(path);
                }
            }
        }
    }
    Ok(reparse)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// 与 mod.rs tests::TestDir 同款：进程级唯一临时目录，靠 OS temp 清理回收
    /// （junction 由 remove_dir_all 移除联接本身、不穿联接删除）。
    struct TestDir(std::path::PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "pinvou3-checkpoint-reparse-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|value| value.as_nanos())
                    .unwrap_or_default()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    /// r42 M6 回归：junction 被枚举出来；排除目录内的 junction 不误报。
    /// mklink /J 不需要管理员权限。
    #[test]
    fn enumerates_reparse_dirs_and_respects_the_skip_list() {
        let root = TestDir::new("root");
        let outside = TestDir::new("outside");
        let junc = root.path().join("wt-junc");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junc)
            .arg(outside.path())
            .status()
            .expect("mklink runs");
        assert!(status.success(), "mklink /J must succeed on Windows");
        std::fs::create_dir_all(root.path().join("node_modules")).unwrap();
        let junc_in_excluded = root.path().join("node_modules").join("pkg-junc");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junc_in_excluded)
            .arg(outside.path())
            .status()
            .expect("mklink runs");
        assert!(status.success(), "mklink /J must succeed on Windows");
        let skip: std::collections::HashSet<&str> = ["node_modules"].into_iter().collect();
        let found = reparse_dir_paths_under(root.path(), &skip).expect("budgeted walk");
        assert!(
            found.iter().any(|path| path == &junc),
            "顶层 junction 必须被枚举: {found:?}"
        );
        assert!(
            !found.iter().any(|path| path == &junc_in_excluded),
            "排除目录内的 junction 不得误报: {found:?}"
        );
    }

    /// r42 M6 接线回归：junction 让快照与恢复路径 fail-closed——checkout-index
    /// 穿联接写根外是 rc=0 静默灾难，业务层检测必须先于任何 git 写。（平台
    /// 相关的夹具在适配层测试内，业务函数经 crate 路径引用验证真实接线。）
    #[test]
    fn junctions_fail_closed_the_snapshot_and_restore_paths() {
        use crate::features::code_checkpoints::{
            CheckpointKind, CheckpointMeta, apply_restore, create_checkpoint, git_available,
        };
        if !git_available() {
            return;
        }
        struct TestDir(std::path::PathBuf);
        impl TestDir {
            fn new(label: &str) -> Self {
                let path = std::env::temp_dir().join(format!(
                    "pinvou3-checkpoint-juncwiring-{label}-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|value| value.as_nanos())
                        .unwrap_or_default()
                ));
                std::fs::create_dir_all(&path).unwrap();
                Self(path)
            }
            fn path(&self) -> &std::path::Path {
                &self.0
            }
            fn write(&self, relative: &str, content: &str) {
                std::fs::write(self.0.join(relative), content).unwrap();
            }
        }
        let ledger = TestDir::new("ledger");
        let exec = TestDir::new("exec");
        let outside = TestDir::new("outside");
        exec.write("a.txt", "a\n");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(exec.path().join("wt-junc"))
            .arg(outside.path())
            .status()
            .expect("mklink runs");
        assert!(status.success(), "mklink /J must succeed on Windows");
        let create_error = create_checkpoint(
            ledger.path(),
            exec.path(),
            Some(1),
            CheckpointKind::Turn,
            "t1",
        )
        .unwrap_err();
        assert!(
            create_error.to_string().contains("联接"),
            "快照面必须 fail-closed: {create_error:#}"
        );
        // 恢复面：reparse 检查先于任何 git 写——伪造 meta 即可验证拒绝顺序
        // （若顺序回归到 ensure_repo/read-tree 之后，错误将不再是「联接」）。
        let fake = CheckpointMeta {
            id: "c1-1".to_string(),
            turn: Some(1),
            kind: CheckpointKind::Turn,
            label: String::new(),
            commit: "0".repeat(40),
            created_at: 0,
        };
        let apply_error = apply_restore(ledger.path(), exec.path(), &fake).unwrap_err();
        assert!(
            apply_error.to_string().contains("联接"),
            "恢复面必须 fail-closed（先于 read-tree/checkout-index）: {apply_error:#}"
        );
    }

    /// r45 borderline 回归：refs 遍历**根自身**是链接时整体拒绝清扫——
    /// 工作区外植入的陈旧 *.lock 不得被 unlink（r44 M3 只堵了条目层，根
    /// 仍被 read_dir 跟随）。断言：清扫不产出清除、外部哨兵文件原样在场。
    #[test]
    fn sweep_refs_refuses_a_symlinked_root_and_never_touches_the_target() {
        use crate::features::code_checkpoints::sweep_refs;
        let root = TestDir::new("sweep-root");
        let outside = TestDir::new("sweep-outside");
        // 外部目标树内的「陈旧锁」（≥120s 由 is_stale 恒真代替）。
        let outside_lock = outside.path().join("head.lock");
        std::fs::write(&outside_lock, b"stale").unwrap();
        // 影子仓库形状：<repo>/refs 本体替换为指向外部的 junction。
        let repo = root.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let refs = repo.join("refs");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&refs)
            .arg(outside.path())
            .status()
            .expect("mklink runs");
        assert!(status.success(), "mklink /J must succeed on Windows");

        let cleared = sweep_refs(&refs, 4, &|_| true);
        assert!(!cleared, "symlinked refs root must refuse the sweep");
        assert!(
            outside_lock.exists(),
            "the out-of-workspace lock must never be deleted through the link"
        );
    }

    /// r52 M2 classifier pin: only junctions (MOUNT_POINT) and directory-form
    /// symlinks are refused; OneDrive cloud-placeholder tags (the CLOUD_*
    /// family) and unknown tags pass - the raw-reparse-bit test used to
    /// refuse every cloud-managed directory, failing all three paths closed
    /// permanently.
    #[test]
    fn reparse_tag_classifier_refuses_only_mount_points_and_dir_symlinks() {
        // tag 常量与 winnt.h 对齐（walk 内同值）。
        const MOUNT_POINT: u32 = 0xA000_0003;
        const SYMLINK: u32 = 0xA000_000C;
        assert!(super::reparse_tag_requires_refusal(MOUNT_POINT, true));
        assert!(super::reparse_tag_requires_refusal(SYMLINK, true));
        assert!(
            !super::reparse_tag_requires_refusal(SYMLINK, false),
            "文件形态 symlink 不被 checkout-index 穿越，不拒"
        );
        // CLOUD_* 家族（OneDrive Files-On-Demand/KFM）：0xA000_0004..=0xA000_001F。
        for tag in [0xA000_0004u32, 0xA000_0009, 0xA000_001F] {
            assert!(
                !super::reparse_tag_requires_refusal(tag, true),
                "cloud tag {tag:#x} must be allowed (hydrated, traversable)"
            );
        }
        assert!(!super::reparse_tag_requires_refusal(0, true));
        assert!(!super::reparse_tag_requires_refusal(0x8000_0000, true));
    }

    /// r52 M2 budget pin: exceeding the entry budget must Err (fail-closed)
    /// rather than truncate silently - with incomplete coverage "not found
    /// does not mean absent". `max_entries` is parameterized so the budget
    /// is testable.
    #[test]
    fn reparse_walk_budget_exceed_is_fail_closed() {
        let root = TestDir::new("budget");
        for index in 0..5 {
            std::fs::create_dir_all(root.path().join(format!("d{index}"))).unwrap();
        }
        let skip: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let error = super::walk_reparse_dirs(root.path(), &skip, 2)
            .err()
            .expect("budget exceed must be an Err");
        assert!(
            error.to_string().contains("budget"),
            "err must name the budget: {error:#}"
        );
        // 预算内照常 Ok。
        super::walk_reparse_dirs(root.path(), &skip, 200).expect("within budget");
    }

    /// r52 M2 probe pin: a junction's live tag must read as MOUNT_POINT (the
    /// previous raw-bit test could not tell tags apart; FindFirstFileW's
    /// dwReserved0 is the only user-mode read).
    #[test]
    fn probe_reads_mount_point_tag_from_a_real_junction() {
        let root = TestDir::new("probe");
        let outside = TestDir::new("probe-outside");
        let junc = root.path().join("wt-junc");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junc)
            .arg(outside.path())
            .status()
            .expect("mklink runs");
        assert!(status.success(), "mklink /J must succeed on Windows");
        let (attributes, tag) = super::probe_reparse_attributes(&junc).expect("junction probe");
        assert_eq!(tag, 0xA000_0003, "junction dwReserved0 must be MOUNT_POINT");
        assert_ne!(attributes & 0x0400, 0, "junction carries the reparse bit");
        // A plain directory: no reparse bit -> tag 0.
        let plain = root.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        let (_attributes, tag) = super::probe_reparse_attributes(&plain).expect("plain probe");
        assert_eq!(tag, 0, "plain dir has no reparse tag");
    }
}
