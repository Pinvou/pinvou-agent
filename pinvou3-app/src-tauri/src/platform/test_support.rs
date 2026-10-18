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
    // Keyring hermeticity rides along — the same trio the marketplace
    // harness installs. A test reaching a real credential write through
    // this harness otherwise lands in the developer's OS keychain, and on
    // macOS the OS probe from an ad-hoc-signed test binary can block
    // indefinitely on the ACL consent dialog. `PINVOU3_TEST_KEYRING_FILE_FALLBACK`
    // exists only in test builds (credential_store reads it before probing
    // the OS keyring); `CODEWHALE_HOME` rides to the temp dir so
    // CodeWhale-side state stays hermetic too. The trio is restored through
    // the shared `EnvRestore`.
    // Declared after `_g` so it drops BEFORE the lock is released (no other
    // test may observe the temporary values), and before the set_var calls
    // below so the snapshot sees the caller's values. Restore runs on Drop —
    // a failing assertion inside the closure unwinds, and a straight-line
    // restore would be skipped, leaking the env AND the temp dir into every
    // later test in this process — with the dir removal as the post_restore
    // tail so it happens after every env restore.
    let _env = EnvRestore::capture_with_post_restore(
        &[
            "PINVOU3_HOME",
            "CODEWHALE_HOME",
            "PINVOU3_TEST_KEYRING_FILE_FALLBACK",
        ],
        {
            let dir = dir.clone();
            move || {
                let _ = std::fs::remove_dir_all(dir);
            }
        },
    );
    // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
    unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
    // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
    unsafe { std::env::set_var("CODEWHALE_HOME", &dir) };
    // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
    unsafe { std::env::set_var("PINVOU3_TEST_KEYRING_FILE_FALLBACK", "1") };
    f();
}

/// Take the crate-unique env lock and snapshot a set of env vars in one step:
/// `let (_lock, _env) = locked_env(&["PINVOU3_HOME"]);`
/// Not reentrant — never call while already holding ENV_LOCK. The guard
/// releases the lock and restores the env when scope exits (including panic
/// unwind). (`bridge.rs`'s test module still keeps a private
/// `locked_env`/`EnvGuard` pair; its ~hundred call sites have not been
/// migrated here — new code always uses this implementation so the old pair
/// stops spreading.)
#[cfg(test)]
pub(crate) fn locked_env(
    vars: &[&'static str],
) -> (std::sync::MutexGuard<'static, ()>, EnvRestore) {
    let lock = crate::platform::paths::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    (lock, EnvRestore::capture(vars))
}

/// 连接器旧布局 bin 目录的测试入口：返回 `managed_connector_bin_dir()`；为
/// `None` 时硬断言当前目标平台确实不受 lock 表覆盖。消费方测试（存量迁移、
/// PATH 次序等）据此软跳过——受支持平台上的 wrapper 回归在这里失败，而不是
/// 被各消费方测试的 `else { return }` 静默吞掉（否则 PATH 注入、存量迁移/
/// 残留清理、spawn 回退解析的回归会全部变哑）。
#[cfg(test)]
pub(crate) fn managed_connector_bin_dir_or_assert_unsupported() -> Option<std::path::PathBuf> {
    let dir = crate::platform::paths::managed_connector_bin_dir();
    if dir.is_none() {
        assert!(
            crate::platform::paths::connector_platform_dir(
                std::env::consts::OS,
                std::env::consts::ARCH
            )
            .is_none(),
            "managed_connector_bin_dir must be Some on a lock-covered platform (os={}, arch={})",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
    }
    dir
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

/// Root-skip probes for the fail-closed permission fixtures:
/// chmod the target to the denial mode, check the denial actually took
/// effect, and report `false` — restoring the default mode and eprintln-ing
/// the loud, per-test greppable `ROOT-SKIP[<test>]` marker — when it did not
/// (root is not constrained by mode bits), so the caller skips the
/// assertions that depend on EACCES instead of passing vacuously:
/// `if !file_unreadable_or_skip(&path, "test_name") { return; }`. Unix-only:
/// every caller is a `#[cfg(unix)]` test. Deny/restore modes match the
/// fixtures this replaces — files deny with 000 and restore 644, directories
/// deny with 0555 and restore 755.
#[cfg(all(test, unix))]
pub(crate) fn file_unreadable_or_skip(file: &std::path::Path, test: &str) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    let _ = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o000));
    // Root is not constrained by 000: with the file still readable the
    // unreadable-read branch would never run — restore and skip loudly.
    if std::fs::read(file).is_ok() {
        let _ = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o644));
        eprintln!(
            "ROOT-SKIP[{test}]: running as root - the unreadable-file fixture stays readable; NOT exercised"
        );
        return false;
    }
    true
}

/// [`file_unreadable_or_skip`] for a read-only directory: chmod 0555 and
/// probe by creating a file inside it. On `true` the directory stays
/// read-only for the caller's denial assertions; on `false` the fixture was
/// ineffective (root) — restored to 755 and skipped loudly.
#[cfg(all(test, unix))]
pub(crate) fn dir_read_only_or_skip(dir: &std::path::Path, test: &str) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555));
    // Root is not constrained by 0555: with the directory still writable the
    // failure branch would never run — restore and skip loudly.
    let probe = dir.join(".root-probe");
    if std::fs::write(&probe, b"").is_ok() {
        let _ = std::fs::remove_file(&probe);
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
        eprintln!(
            "ROOT-SKIP[{test}]: running as root - the read-only-dir fixture stays writable; NOT exercised"
        );
        return false;
    }
    true
}

/// POSIX mode bits of `path`, or `None` where the platform has none.
///
/// Permission assertions belong to the platform layer: a feature test that
/// writes its own `#[cfg(unix)]` block puts an OS conditional outside an
/// adapter (architecture guard `rust_target_cfg_outside_adapter`), and every
/// such copy drifts on its own. Callers assert `Some(0o600)` and skip on
/// `None`.
#[cfg(test)]
pub(crate) fn permission_bits(path: &std::path::Path) -> Option<u32> {
    crate::platform::filesystem::permission_bits(path)
        .ok()
        .flatten()
}

/// Loosens `path` to 0644 the way a pre-hardening build's umask left it, so a
/// test can prove the next write tightens it. No-op (and reports `false`)
/// where the platform has no mode bits.
#[cfg(test)]
pub(crate) fn loosen_to_world_readable_for_test(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

/// Bounded-refusal assertion for an operation that must fail fast on a
/// planted non-regular target (FIFO/device) instead of hanging: runs `f` on
/// a worker thread and requires it to report `true` within 5 seconds. On
/// timeout the test panics while the abandoned worker (holding no lock) is
/// left to finish or die on its own, so the red does not cascade; a `false`
/// result within the bound is a plain refusal-assertion failure. The repo
/// convention for the hardened private-data primitives.
#[cfg(test)]
pub(crate) fn assert_refused_without_hanging(
    label: &str,
    f: impl FnOnce() -> bool + Send + 'static,
) {
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    let refused = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap_or_else(|_| panic!("{label} must refuse without hanging (5s bound)"));
    worker.join().unwrap();
    assert!(refused, "{label} must be refused");
}

/// Bounded-run containment for a worker that may hold a cross-process lock
/// (mcp.lock, a scope/transaction lock, the landing lease): spawns `worker`
/// on its own thread and returns its value, but only if it finishes within
/// 5 seconds. On a timeout or a channel disconnect (worker panic) the
/// helper eprints the abort line and kills the process with
/// `std::process::abort()`: a plain test panic would unwind the assertion
/// thread while the abandoned worker stays blocked HOLDING the lock, and
/// that leaked holder cascade-hangs every later test in the serial lane —
/// the abort variant of [`assert_refused_without_hanging`], whose workers
/// hold no lock and may safely be left to finish or die on their own. The
/// abort line reads `FAIL: {label} — aborting the test process to contain
/// the leaked lock holder`; `label` carries the per-site context (what
/// still blocks, on what). Per-site refusal/value predicates stay at the
/// call sites — this helper owns only the spawn/5s-bound/eprintln+abort/
/// join discipline.
#[cfg(test)]
pub(crate) fn run_bounded_worker_or_abort<R: Send + 'static>(
    label: &str,
    worker: impl FnOnce() -> R + Send + 'static,
) -> R {
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let value = worker();
        let _ = tx.send(value);
    });
    let value = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap_or_else(|_| {
            eprintln!(
                "FAIL: {label} — aborting the test process to contain the leaked lock holder"
            );
            std::process::abort();
        });
    handle.join().unwrap();
    value
}

/// Shared zip-fixture builder for the plugin-package import tests: writes
/// `entries` (archive path → bytes, in order) into a uniquely named temp zip
/// (`{prefix}-{pid}-{suffix}.zip`) and returns its path. Zip bytes and temp
/// names match the per-test inline builders this replaces.
#[cfg(test)]
pub(crate) fn write_test_zip(prefix: &str, entries: &[(&str, Vec<u8>)]) -> std::path::PathBuf {
    let mut zip_buf = std::io::Cursor::new(Vec::new());
    {
        use std::io::Write;
        let mut zw = zip::ZipWriter::new(&mut zip_buf);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, bytes) in entries {
            zw.start_file((*name).to_string(), opts).unwrap();
            zw.write_all(bytes).unwrap();
        }
        zw.finish().unwrap();
    }
    let tmp = std::env::temp_dir().join(format!(
        "{prefix}-{}-{}.zip",
        std::process::id(),
        crate::platform::paths::tests::unique_suffix()
    ));
    std::fs::write(&tmp, zip_buf.into_inner()).unwrap();
    tmp
}
