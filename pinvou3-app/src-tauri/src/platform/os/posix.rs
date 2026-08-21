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

/// 进程存活探测（`kill(pid, 0)` 语义：不发送信号，仅做存在性/权限检查）。
/// 返回 0 或 errno == EPERM（进程存在但属于其他用户）均视为存活；ESRCH 为已退出。
/// 消费方：browser watch 删除 stale 端口文件前的持有者护栏（经 interface/system.rs）。
pub fn process_alive(pid: u32) -> bool {
    // SAFETY: 信号 0 不产生实际信号，仅查询进程存在性，对任意 pid 调用安全。
    if unsafe { libc::kill(pid as i32, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
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
}
