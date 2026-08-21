use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn filesystem_path_identity_key(path: &str) -> String {
    // APFS may be configured case-sensitive; preserve the stored path exactly.
    path.to_string()
}

pub fn user_home_dir() -> PathBuf {
    // 与 unsupported.rs 对齐:HOME 缺失时用 std::env::temp_dir()(而非硬编码 "/tmp")。
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
}

pub fn platform_compat_path(value: &str) -> PathBuf {
    PathBuf::from(value)
}

pub fn pandoc_tool_path() -> PathBuf {
    PathBuf::from("pandoc")
}

/// 连接器 CLI 解析(与 linux_path.rs 同策略):应用管理的 bin 目录优先,
/// 其次常见 npm 全局前缀,最后交给 PATH。
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
    if program == cli_bin && matches!(cli_bin, "tmeet" | "weibo-cli") {
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

/// 腾讯会议首次使用时的 npm 安装目录：把 prefix 收到用户可写的
/// `~/.npm-global`，避免 GUI 无 sudo 写 `/usr/local` 失败。
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

    let mut paths = vec![bin];
    if let Some(current) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&current));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        cmd.env("PATH", joined);
    }
}
