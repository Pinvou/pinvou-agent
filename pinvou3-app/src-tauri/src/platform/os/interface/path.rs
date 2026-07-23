use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn user_home_dir() -> PathBuf {
    super::super::platform::user_home_dir()
}

pub fn platform_compat_path(value: &str) -> PathBuf {
    super::super::platform::platform_compat_path(value)
}

pub fn validate_upload_location(canon: &Path) -> Result<(), String> {
    super::super::platform::validate_upload_location(canon)
}

pub fn path_component_eq(component: &OsStr, expected: &str) -> bool {
    super::super::platform::path_component_eq(component, expected)
}

pub fn python_command() -> String {
    super::super::platform::python_command()
}

pub fn configure_onnxruntime_dylib() -> Result<(), String> {
    super::super::platform::configure_onnxruntime_dylib()
}

pub fn obsidian_config_path() -> Option<PathBuf> {
    super::super::platform::obsidian_config_path()
}

/// Convert a filesystem path to the native form accepted by desktop applications.
pub fn external_application_path(path: &Path) -> PathBuf {
    platform_compat_path(&path.to_string_lossy())
}

/// Build a standards-compliant file URL after normalising platform-only path prefixes.
pub fn file_url_from_path(path: &Path) -> Result<tauri::Url, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| std::env::temp_dir())
            .join(path)
    };
    let native = external_application_path(&absolute);
    tauri::Url::from_file_path(&native).map_err(|_| format!("convert file url: {}", path.display()))
}

pub fn connector_cli_command(cli_bin: &str, program: &str) -> Command {
    super::super::platform::connector_cli_command(cli_bin, program)
}

pub fn apply_user_npm_prefix(cmd: &mut Command) {
    super::super::platform::apply_user_npm_prefix(cmd);
}

pub fn kill_pid_tree(pid: u32) {
    super::super::platform::kill_pid_tree(pid);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_path_api_returns_pathbuf() {
        let p = platform_compat_path("/tmp/pinvou3-os-test");
        assert!(!p.as_os_str().is_empty());
    }

    #[test]
    fn user_home_dir_returns_some_path() {
        assert!(!user_home_dir().as_os_str().is_empty());
    }
}
