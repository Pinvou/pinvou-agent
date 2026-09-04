//! Unix 通用 helper —— linux 与 macos 共享的纯 POSIX 逻辑。
//!
//! Wave 3 提取：原先 `linux/linux_path.rs` 和 `unsupported.rs`（macOS 经 glob
//! `pub use super::unsupported::*` 继承）各自维护一份相同实现。此文件收口
//! 真正等价的 Unix helper，消除重复源。
//!
//! **不提取的 helper**（各有平台差异，保留在各自文件）：
//! - `user_home_dir`：linux 硬编码 `/tmp`（品悟临时产物目录），macOS 用 `temp_dir()`
//! - `kill_pid_tree`：linux 与 macOS 的组杀实现现已统一为直调 kill(2)，不再委托外部 kill 命令
//! - `connector_cli_command` / `apply_user_npm_prefix`：有结构性差异
//!
//! `validate_upload_location` 不提取：其 body 调用 `user_home_dir()`，后者按平台不同。

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Spawn a short-lived fire-and-forget external process and reap it, avoiding Unix zombies.
///
/// Dropping the Child returned by std's `Command::spawn()` does **not**
/// reclaim the child (unlike tokio's kill_on_drop) and the parent never
/// auto-reaps; every open/xdg-open would leave a zombie until the parent
/// exits. This starts a detached reaper thread to `wait()`; open-file and
/// notification commands usually exit in milliseconds, ending the thread
/// immediately, so the steady-state cost is negligible.
/// The exception is agent-login browser launches
/// (`codex_acp::open_agent_login_url`): the first firefox/chrome instance
/// is itself a long-lived browser process, so the reaper thread parks in
/// `wait()` until the browser exits — one parked thread per long-lived
/// instance, still an acceptable cost; only the synchronous fallback on
/// thread-creation failure blocks for just as long (see below), which
/// remains preferable to zombie accumulation.
/// When reaper-thread creation fails (thread-count/memory-constrained edge
/// cases), `wait()` synchronously on the calling thread: the command has
/// already launched successfully, and a rare stall on a slow command beats
/// zombie accumulation plus a bogus "open failed" report.
pub fn spawn_detached_and_reap(command: &mut std::process::Command) -> std::io::Result<()> {
    use std::sync::{Arc, Mutex};
    // When `Builder::spawn` fails the closure is dropped (not returned),
    // so ownership of the Child cannot be taken back; route it through a
    // shared Option instead: the reaper thread and the failure fallback
    // race for it, and only one side can take it.
    let child = Arc::new(Mutex::new(Some(command.spawn()?)));
    let thread_child = Arc::clone(&child);
    match std::thread::Builder::new()
        .name("unix-child-reaper".to_string())
        .spawn(move || {
            if let Some(mut owned) = thread_child.lock().ok().and_then(|mut slot| slot.take()) {
                let _ = owned.wait();
            }
        }) {
        Ok(_) => Ok(()),
        Err(_) => {
            if let Some(mut owned) = child.lock().ok().and_then(|mut slot| slot.take()) {
                let _ = owned.wait();
            }
            Ok(())
        }
    }
}

/// 把外部传入的路径字符串原样转为 `PathBuf`。
/// linux 与 macOS 实现相同（皆 `PathBuf::from(value)`），收口于此。
pub fn platform_compat_path(value: &str) -> PathBuf {
    PathBuf::from(value)
}

/// 比较路径组件是否等于预期字符串。
/// Unix 上大小写敏感，linux 与 macOS 实现相同。
pub fn path_component_eq(component: &OsStr, expected: &str) -> bool {
    component == OsStr::new(expected)
}

/// Unix 文件系统路径的稳定标识 key(大小写敏感,直接用原串)。
pub fn filesystem_path_identity_key(path: &str) -> String {
    path.to_string()
}

/// Probes process liveness with `kill(pid, 0)` without sending a signal.
/// Success and EPERM both mean the process exists; ESRCH means it exited.
/// Browser watch uses this through interface/system.rs before removing a stale port file.
pub fn process_alive(pid: u32) -> bool {
    // pid 0 would probe this process's own process group (always "alive");
    // callers own non-zero pids, so guard the sentinel instead of reporting it.
    if pid == 0 {
        return false;
    }
    // SAFETY: Signal 0 only queries process existence and is safe for any pid.
    if unsafe { libc::kill(pid as i32, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Restricts a sensitive directory to the current user on POSIX systems.
pub fn make_private_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    if let Err(error) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)) {
        eprintln!(
            "[platform] failed to restrict directory permissions for {}: {error}",
            path.display()
        );
    }
}
/// 探测 PATH 中第一个可用的 python 解释器名。
/// 优先 `python3`，回退 `python`，最终默认 `python3`。
pub fn python_command() -> String {
    if which_in_path("python3") {
        return "python3".to_string();
    }
    if which_in_path("python") {
        return "python".to_string();
    }
    "python3".to_string()
}

/// POSIX 空设备路径。
pub fn null_device() -> &'static str {
    "/dev/null"
}

/// 在 `PATH` 环境变量中逐目录扫描给定命令是否可执行。
fn which_in_path(cmd: &str) -> bool {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            if dir.join(cmd).is_file() {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_compat_path_is_identity() {
        assert_eq!(platform_compat_path("/usr/bin"), PathBuf::from("/usr/bin"));
    }

    #[test]
    fn path_component_eq_is_case_sensitive() {
        assert!(path_component_eq(OsStr::new("home"), "home"));
        assert!(!path_component_eq(OsStr::new("Home"), "home"));
    }

    #[test]
    fn python_command_defaults_to_python3() {
        // 无论 PATH 状态如何，至少返回 python3
        let cmd = python_command();
        assert!(cmd == "python3" || cmd == "python");
    }

    #[test]
    fn spawn_detached_and_reap_reaps_true_command() {
        // `true` (PATH lookup, usually /bin/true) exits in milliseconds:
        // verifies the spawn succeeded and the reaper thread does not
        // panic.
        // Whether the zombie is actually reaped is invisible without
        // reading the proc table; this at least pins the interface
        // contract (Ok + no deadlock).
        let mut command = std::process::Command::new("true");
        spawn_detached_and_reap(&mut command).expect("spawn true");
        // Give the reaper thread a moment to finish its wait; the test itself makes no blocking assertion.
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    #[test]
    fn spawn_detached_and_reap_reports_missing_binary() {
        let mut command = std::process::Command::new("/nonexistent/pinvou3-reaper-test");
        assert!(spawn_detached_and_reap(&mut command).is_err());
    }
}
