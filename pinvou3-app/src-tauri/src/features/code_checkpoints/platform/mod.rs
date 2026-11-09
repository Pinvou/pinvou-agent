//! code_checkpoints 的 OS 差异适配层。
//!
//! 目前只承载一件事：文件系统目录再解析点（Windows junction / mount point）
//! 的枚举。快照（add -A）与恢复（checkout-index / clean）路径据此 fail-closed
//! 拒绝——git-for-windows 会把 junction 当普通目录递归，业务层两层语义必须
//! 与之同向（r42 M6）。平台差异用 `cfg` 表达、以显式接口暴露：非 Windows
//! 恒返回空集（无再解析点概念，零开销）。

/// 枚举 `root` 下的目录再解析点（junction / mount point），跳过 `skip_dirs`
/// 中的目录名（大小写不敏感，与 SHADOW_EXCLUDES 的目录级排除同语义——排除
/// 目录里的 junction 不进 index、不被 checkout/clean 触及，拒绝它们只会误伤
/// pnpm 式联接农场）。判定依据：Rust 的 `file_type` 把 junction 表现为
/// symlink，`DirEntry::metadata()` 不跟随链接，其 reparse 位 + 目录位同时
/// 置位即目录再解析点；普通文件 symlink 不是目录，checkout-index 不会穿越，
/// 不拒。
pub(crate) fn reparse_dir_paths_under(
    root: &std::path::Path,
    skip_dirs: &std::collections::HashSet<&str>,
) -> Vec<std::path::PathBuf> {
    #[cfg(not(windows))]
    {
        let _ = (root, skip_dirs);
        Vec::new()
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY
        const REPARSE_POINT: u32 = 0x0400;
        const DIRECTORY: u32 = 0x0010;
        let mut reparse: Vec<std::path::PathBuf> = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(read) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in read.flatten() {
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_symlink() {
                    let is_reparse_dir = entry
                        .metadata()
                        .ok()
                        .map(|metadata| {
                            metadata.file_attributes() & (REPARSE_POINT | DIRECTORY)
                                == (REPARSE_POINT | DIRECTORY)
                        })
                        .unwrap_or(false);
                    if is_reparse_dir {
                        reparse.push(entry.path());
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
        reparse
    }
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
        let found = reparse_dir_paths_under(root.path(), &skip);
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
}
