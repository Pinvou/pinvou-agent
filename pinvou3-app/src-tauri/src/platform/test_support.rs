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

/// 测试用：把目录压到当前用户不可读（Unix chmod 000），模拟「已装目录不可扫」
/// 的权限降级场景（如 marketplace scope 的 DenyAll 默认集枚举降级）。返回是否
/// 真正生效：非 Unix 平台无 POSIX 权限位概念，root 不受 000 约束，都返回 false
/// ——调用方据此跳过依赖 EACCES 语义的断言（语义由非 root Unix 环境覆盖）。
/// 用 [`restore_dir_permissions_for_test`] 配对恢复，保证临时目录可被清理。
#[cfg(test)]
pub(crate) fn make_dir_unreadable_for_test(dir: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000)).is_err() {
            return false;
        }
        // root 不受 000 权限约束：探测不到 EACCES 就无法模拟，恢复权限并报告失败。
        let enforced = std::fs::read_dir(dir).is_err();
        if !enforced {
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
        }
        enforced
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        false
    }
}

/// [`make_dir_unreadable_for_test`] 的恢复配对：Unix 恢复 0755（测试目录均由
/// `create_dir_all` 以默认权限创建），其余平台 no-op。
#[cfg(test)]
pub(crate) fn restore_dir_permissions_for_test(dir: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
}
