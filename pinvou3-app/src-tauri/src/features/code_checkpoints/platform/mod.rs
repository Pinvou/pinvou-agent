//! code_checkpoints 的 OS 差异适配层。
//!
//! 目前只承载一件事：文件系统目录再解析点（Windows junction / mount point）
//! 的枚举。快照（add -A）与恢复（checkout-index / clean）路径据此 fail-closed
//! 拒绝——git-for-windows 会把 junction 当普通目录递归，业务层两层语义必须
//! 与之同向（r42 M6）。平台差异用 `cfg` 表达、以显式接口暴露：非 Windows
//! 恒返回空集（无再解析点概念，零开销）。

/// 枚举 `root` 下的目录再解析点（junction / 目录 symlink），跳过 `skip_dirs`
/// 中的目录名（大小写不敏感，与 SHADOW_EXCLUDES 的目录级排除同语义——排除
/// 目录里的 junction 不进 index、不被 checkout/clean 触及，拒绝它们只会误伤
/// pnpm 式联接农场）。
///
/// r52 M2（评审 round-50 M2 落地）：判定不再依赖裸 reparse 位——Rust std 的
/// `is_symlink()` 在 Windows 上按 reparse 位判定、与 tag 无关，OneDrive
/// Files-On-Demand/KFM 给云同步目录打 `IO_REPARSE_TAG_CLOUD_*`（已水合、
/// 可正常穿越），裸位判定把每个云管理目录误判为联接，快照/恢复/预览三路
/// 永久 fail-closed 且成因不可行动。此处以 `FindFirstFileW` 读 reparse
/// tag：只拒绝 `IO_REPARSE_TAG_MOUNT_POINT`（junction）与目录形态的
/// `IO_REPARSE_TAG_SYMLINK`；云占位与其余 tag 放行；无法判定（探测失败：
/// 路径竞态消失/权限）按 fail-closed 拒绝该条目。
///
/// 条目预算与体积走读共享 [`super::SIZE_WALK_MAX_ENTRIES`]（200k）：预算内
/// 未能完成枚举时返回 Err——覆盖不完备时「未检出 ≠ 不存在」，调用方必须
/// fail-closed（r52 minor 9：此前无界且 read_dir 失败静默跳过子树）。
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

/// 纯判别（单元可测）：该 reparse tag + 目录位是否要求拒绝。只拒 junction
/// 与目录形态 symlink；云占位（IO_REPARSE_TAG_CLOUD_* 等）及其余未知 tag
/// 不是穿越危害，放行。
#[cfg(windows)]
fn reparse_tag_requires_refusal(tag: u32, is_directory: bool) -> bool {
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;
    tag == IO_REPARSE_TAG_MOUNT_POINT || (tag == IO_REPARSE_TAG_SYMLINK && is_directory)
}

/// FindFirstFileW 读取路径的现势属性位 + reparse tag（dwReserved0 仅在
/// reparse 位存在时有定义；枚举与探测之间再解析位解除的竞态按 tag=0
/// 放行）。Err = 无法判定（路径消失/权限/FFI 失败）。
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
    // r52 P2（评审）：FindFirstFileW 的搜索句柄必须以 FindClose 释放——
    // CloseHandle 对搜索句柄是未定义的资源生命周期操作（内核搜索上下文
    // 由 FindClose 专管），此处读值已完成，失败仅记 warn 不致命。
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

/// 带条目预算的枚举实现（`max_entries` 参数化以便测试钉住超限行为）。
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
        // r52 minor 9：读失败的子树不得再静默消失——记 warn 后继续（单个
        // 不可读目录不构成整次扫描不完备；预算超限才是）。
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
                // std is_symlink = 裸 reparse 位（与 tag 无关）。以现势
                // FindFirstFileW 数据判 tag；探测失败按 fail-closed 拒绝。
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

    /// r52 M2 纯判别钉：只拒 junction（MOUNT_POINT）与目录形态 symlink；
    /// OneDrive 云占位 tag（CLOUD_* 家族）与未知 tag 放行——裸 reparse 位
    /// 判定曾把每个云管理目录误拒，三路永久 fail-closed。
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

    /// r52 M2 预算钉：条目预算超限必须 Err（fail-closed）而非静默截断——
    /// 覆盖不完备时「未检出 ≠ 不存在」。`max_entries` 参数化使预算可测。
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

    /// r52 M2 探测钉：junction 的现势 tag 必须读作 MOUNT_POINT（此前裸位
    /// 判定无法区分 tag；FindFirstFileW dwReserved0 是唯一的用户态读法）。
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
        // 普通目录：无 reparse 位 → tag=0。
        let plain = root.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        let (_attributes, tag) = super::probe_reparse_attributes(&plain).expect("plain probe");
        assert_eq!(tag, 0, "plain dir has no reparse tag");
    }
}
