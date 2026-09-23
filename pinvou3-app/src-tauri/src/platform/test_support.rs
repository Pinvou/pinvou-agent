//! 跨特性共享的测试 helper（全部条目 `#[cfg(test)]`，不进发布产物）。
//!
//! 进程级 env（PINVOU3_HOME / DEEPSEEK_* 等）是 cargo test 并行执行下的隔离
//! 硬约束：所有改写 env 的测试必须先持有 `platform::paths::tests::ENV_LOCK`
//! （crate 唯一 env 锁——本模块不另立锁源，只复用它），并在退出（含 panic
//! 路径）时恢复原值。本模块把「PINVOU3_HOME 指向干净临时目录跑闭包」与
//! 「快照/恢复一组 env」两个高频脚手架收敛为单一实现，供 assistant /
//! connectors / marketplace 等模块的测试复用，避免逐文件复制出细微漂移。
//!
//! 模块声明位于 `platform/mod.rs`（`#[cfg(test)] pub(crate) mod test_support;`），
//! 不进发布产物。

#[cfg(test)]
use std::ffi::OsString;

/// 把 PINVOU3_HOME 指到干净临时目录跑闭包，跑完恢复并清理。目录名前缀由
/// 调用方给出（按特性/用例区分，避免同 pid 下不同测试互删），并叠加进程 id
/// 与 `unique_suffix()` 保证并行轮次间不碰撞。借
/// `platform::paths::tests::ENV_LOCK` 与其它 mutate PINVOU3_HOME 的测试串行。
#[cfg(test)]
pub(crate) fn with_temp_home(prefix: &str, f: impl FnOnce()) {
    let _g = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        crate::platform::paths::tests::unique_suffix()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let prev = std::env::var("PINVOU3_HOME").ok();
    // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
    unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
    f();
    match prev {
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        None => unsafe { std::env::remove_var("PINVOU3_HOME") },
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// 取 crate 唯一 env 锁并一步完成一组 env 的快照（原各文件私有的
/// `locked_env` 配对形式的单一实现）：
/// `let (_lock, _env) = locked_env(&["PINVOU3_HOME"]);`
/// 不可重入——已持 ENV_LOCK 时不得再调。guard 在作用域退出（含 panic 路径）
/// 释放锁并恢复 env。
#[cfg(test)]
pub(crate) fn locked_env(
    vars: &[&'static str],
) -> (std::sync::MutexGuard<'static, ()>, EnvRestore) {
    let lock = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    (lock, EnvRestore::capture(vars))
}

/// RAII 快照/恢复一组环境变量：`capture` 记录现值，`Drop`（含 panic 路径）
/// 逐一恢复。调用方测试必须先持有 `platform::paths::tests::ENV_LOCK` 再
/// capture，保证 env 写全程在锁内串行。
#[cfg(test)]
pub(crate) struct EnvRestore {
    saved: Vec<(&'static str, Option<OsString>)>,
    /// env 恢复完成后执行的一次性收尾动作（如 multiagent 回归测试恢复
    /// PINVOU3_HOME 后刷新 personas 缓存）；普通用例为 `None`。
    post_restore: Option<Box<dyn FnOnce() + Send>>,
}

#[cfg(test)]
impl EnvRestore {
    pub(crate) fn capture(names: &[&'static str]) -> Self {
        Self {
            saved: names
                .iter()
                .map(|name| (*name, std::env::var_os(name)))
                .collect(),
            post_restore: None,
        }
    }

    /// 同 [`EnvRestore::capture`]，并在 env 全部恢复完成后执行一次
    /// `post_restore`（先恢复 env、后收尾，保证收尾读到的是恢复后的环境）。
    pub(crate) fn capture_with_post_restore(
        names: &[&'static str],
        post_restore: impl FnOnce() + Send + 'static,
    ) -> Self {
        let mut this = Self::capture(names);
        this.post_restore = Some(Box::new(post_restore));
        this
    }
}

#[cfg(test)]
impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (name, value) in self.saved.drain(..) {
            match value {
                // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
                Some(value) => unsafe { std::env::set_var(name, value) },
                // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
                None => unsafe { std::env::remove_var(name) },
            }
        }
        if let Some(post_restore) = self.post_restore.take() {
            post_restore();
        }
    }
}

/// Test helper: drop the current user's read permission on `dir` (Unix chmod
/// 000) to simulate an unscannable installed directory (e.g. the marketplace
/// scope's DenyAll default enumeration degradation). Returns `None` when the
/// simulation cannot take effect — non-Unix has no POSIX mode bits, and root
/// is not constrained by 000 — so callers skip the assertions that depend on
/// EACCES semantics (covered on non-Unix-root environments, e.g. the ubuntu CI
/// runner). The returned guard restores 0755 on drop, including panic
/// unwinding, so the temp home stays cleanable even when an assertion fires
/// while the directory is unreadable.
#[cfg(test)]
pub(crate) fn make_dir_unreadable_for_test(dir: &std::path::Path) -> Option<UnreadableDirForTest> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000)).is_err() {
            return None;
        }
        // Root is not constrained by 000: without an observable EACCES the
        // simulation is ineffective, so restore and report failure.
        if std::fs::read_dir(dir).is_ok() {
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
            return None;
        }
        Some(UnreadableDirForTest {
            dir: dir.to_path_buf(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        None
    }
}

/// Restore pairing for [`make_dir_unreadable_for_test`]: restores 0755 (test
/// directories are created by `create_dir_all` with default permissions) on
/// drop, in the `EnvRestore` house pattern. Only ever constructed on Unix.
#[cfg(test)]
pub(crate) struct UnreadableDirForTest {
    dir: std::path::PathBuf,
}

#[cfg(test)]
impl Drop for UnreadableDirForTest {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o755));
        }
        #[cfg(not(unix))]
        {
            let _ = &self.dir;
        }
    }
}
