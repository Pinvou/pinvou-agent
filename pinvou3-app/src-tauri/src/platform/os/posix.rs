//! Unix 通用 helper —— linux 与 macos 共享的纯 POSIX 逻辑。
//!
//! Wave 3 提取：原先 `linux/linux_path.rs` 和 `unsupported.rs`（macOS 经 glob
//! `pub use super::unsupported::*` 继承）各自维护一份相同实现。此文件收口
//! 真正等价的 Unix helper，消除重复源。
//!
//! **不提取的 helper**（各有平台差异，保留在各自文件）：
//! - `user_home_dir`：linux 硬编码 `/tmp`（品悟临时产物目录），macOS 用 `temp_dir()`
//! - `kill_pid_tree`：linux 经 `connector_cli_command("", "kill")` 路由，macOS 用 `Command::new("kill")`
//! - `connector_cli_command` / `apply_user_npm_prefix`：有结构性差异
//!
//! `validate_upload_location` 不提取：其 body 调用 `user_home_dir()`，后者按平台不同。

use std::ffi::OsStr;
use std::path::PathBuf;

/// spawn 一个"点火即忘"的短命外部进程并负责收割，避免 Unix 僵尸。
///
/// std 的 `Command::spawn()` 返回的 Child 被 drop 时**不会**回收子进程
/// （不像 tokio 的 kill_on_drop），父进程也不自动 reap；每次 open/xdg-open
/// 都会留一个 zombie 直到父进程退出。这里起一个 detached 收割线程 `wait()`，
/// 打开文件/发通知类命令通常毫秒级退出，线程随即结束，常驻成本可忽略。
/// 收割线程创建失败（线程数/内存受限的极端场景）时在调用线程同步 `wait()`：
/// 命令已成功启动，此时慢命令的极端卡顿优于僵尸累积 + 误报打开失败。
pub fn spawn_detached_and_reap(command: &mut std::process::Command) -> std::io::Result<()> {
    use std::sync::{Arc, Mutex};
    // `Builder::spawn` 失败时闭包被 drop（不归还），Child 的所有权无法要回；
    // 经共享的 Option 中转：收割线程与失败回退路径先到先得，只有一方能 take。
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
        // `/usr/bin/true` 毫秒级退出：验证 spawn 成功且收割线程不 panic。
        // 僵尸是否复排除非查 proc 表不可见，这里至少锁定接口契约（Ok + 不死锁）。
        let mut command = std::process::Command::new("true");
        spawn_detached_and_reap(&mut command).expect("spawn /usr/bin/true");
        // 给收割线程一点时间完成 wait，测试本身无阻塞断言。
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    #[test]
    fn spawn_detached_and_reap_reports_missing_binary() {
        let mut command = std::process::Command::new("/nonexistent/pinvou3-reaper-test");
        assert!(spawn_detached_and_reap(&mut command).is_err());
    }
}
