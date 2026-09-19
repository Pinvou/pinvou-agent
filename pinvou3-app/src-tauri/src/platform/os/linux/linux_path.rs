use std::path::{Path, PathBuf};

// Unix 通用 helper 从 posix.rs 继承（Wave 3 去重；连接器 CLI 解析、npm prefix、
// 树杀等语义一致实现已收口，见 posix.rs 模块注释）。
pub use super::super::posix::{
    apply_user_npm_prefix, connector_cli_command, filesystem_path_identity_key, kill_pid_tree,
    null_device, path_component_eq, platform_compat_path, python_command,
};

pub fn user_home_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
}

pub fn validate_upload_location(canon: &Path) -> Result<(), String> {
    super::super::posix::validate_upload_location_under_home(&user_home_dir(), canon)
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
