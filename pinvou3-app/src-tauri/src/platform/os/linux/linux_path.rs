use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

// Unix 通用 helper 从 posix.rs 继承（Wave 3 去重）。
pub use super::super::posix::{
    filesystem_path_identity_key, null_device, path_component_eq, platform_compat_path,
    python_command,
};

pub fn user_home_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
}

pub fn validate_upload_location(canon: &Path) -> Result<(), String> {
    let home_raw = user_home_dir();
    let home = platform_compat_path(
        &std::fs::canonicalize(&home_raw)
            .unwrap_or_else(|_| home_raw.clone())
            .to_string_lossy(),
    );
    if !canon.starts_with(&home) {
        return Err(format!("path {} not under $HOME", canon.display()));
    }
    Ok(())
}

pub fn configure_onnxruntime_dylib() -> Result<(), String> {
    Ok(())
}

pub fn obsidian_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join(".config/obsidian/obsidian.json"))
}

pub fn connector_cli_command(cli_bin: &str, program: &str) -> Command {
    if program == "npm" {
        if let (Some(node), Some(npm_cli)) = (
            crate::platform::paths::bundled_connector_node(),
            crate::platform::paths::bundled_connector_npm_cli(),
        ) {
            let mut command = Command::new(node);
            command.arg(npm_cli);
            return command;
        }
    }
    let resolved = connector_cli_program(cli_bin, program);
    // npm 的 Unix bin 是带 `#!/usr/bin/env node` 的 JS shim；GUI 环境不保证系统
    // PATH 有 node，因此腾讯会议也显式交给随包 Node 执行。
    if program == cli_bin && cli_bin == "tmeet" {
        let script = PathBuf::from(&resolved);
        if script.is_file() {
            if let Some(node) = crate::platform::paths::bundled_connector_node() {
                let mut command = Command::new(node);
                command.arg(script);
                return command;
            }
        }
    }
    Command::new(resolved)
}

fn connector_cli_program(cli_bin: &str, program: &str) -> OsString {
    if program == cli_bin {
        // 版本化资产库（lock 表单点解析）优先；旧布局（未迁移存量）其次。
        if let Some(path) = crate::platform::connector_lock::locked_cli_path(cli_bin) {
            if path.is_file() {
                return path.into_os_string();
            }
        }
        if let Some(bin_dir) = crate::platform::paths::managed_connector_bin_dir() {
            let bundled = bin_dir.join(cli_bin);
            if bundled.is_file() {
                return bundled.into_os_string();
            }
        }
    }
    if program == cli_bin {
        let mut candidates = Vec::new();
        if let Ok(prefix) = std::env::var("NPM_CONFIG_PREFIX") {
            candidates.push(Path::new(&prefix).join("bin").join(program));
        }
        if let Ok(home) = std::env::var("HOME") {
            let home = Path::new(&home);
            candidates.push(home.join(".npm-global").join("bin").join(program));
            candidates.push(home.join(".local").join("bin").join(program));
        }
        for p in candidates {
            if p.is_file() {
                return p.into_os_string();
            }
        }
    }
    program.into()
}

pub fn apply_user_npm_prefix(cmd: &mut Command) {
    if std::env::var_os("NPM_CONFIG_PREFIX").is_some()
        || std::env::var_os("npm_config_prefix").is_some()
    {
        return;
    }

    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let prefix = Path::new(&home).join(".npm-global");
    let bin = prefix.join("bin");
    let _ = std::fs::create_dir_all(&bin);
    cmd.env("NPM_CONFIG_PREFIX", &prefix)
        .env("npm_config_prefix", &prefix);
    prepend_connector_path_entries(cmd, [bin]);
}

/// 退出收割用的树杀。连接器 CLI 是 npm shim(shell 脚本→node 子进程),spawn 侧
/// 已用 `process_group(0)` 让 shim 独立成组,这里按负 pid 杀整组,否则单杀 shim
/// 的 pid 会把 node 孙进程孤儿化(与 platform::process::kill_process_tree 同语义,
/// 走 connector_cli_command 保持 PATH 解析一致)。若进程恰未成组(旧登记),
/// 追加一次单 pid 兜底。
pub fn kill_pid_tree(pid: u32) {
    if pid <= 1 {
        // pid<=1 expands to the kernel kill(0/-1) special semantics, which
        // signals every process of this user (once took down the whole
        // desktop session). Refuse and leave a backtrace on disk.
        crate::platform::process::log_refused_user_wide_kill("linux kill_pid_tree", pid);
        return;
    }
    let group_arg = format!("-{pid}");
    let out = connector_cli_command("", "kill")
        .args(["-9", group_arg.as_str()])
        .output();
    let group_ok = out.map(|o| o.status.success()).unwrap_or(false);
    if !group_ok {
        let _ = connector_cli_command("", "kill")
            .args(["-9", &pid.to_string()])
            .output();
    }
}

fn prepend_connector_path_entries(cmd: &mut Command, dirs: impl IntoIterator<Item = PathBuf>) {
    let mut paths: Vec<PathBuf> = dirs
        .into_iter()
        .filter(|p| !p.as_os_str().is_empty())
        .collect();
    if let Some(current) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&current));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        cmd.env("PATH", joined);
    }
}

pub fn pdf_tool_path(command: &str) -> PathBuf {
    PathBuf::from(command)
}

pub fn pandoc_tool_path() -> PathBuf {
    PathBuf::from("pandoc")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_location_rejects_outside_home() {
        assert!(validate_upload_location(Path::new("/etc/passwd")).is_err());
    }
}
