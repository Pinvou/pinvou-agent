use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::monitor::RamSnapshot;

pub fn open_target(_target: impl AsRef<OsStr>, label: &str) -> Result<(), String> {
    Err(format!("当前平台不支持系统打开: {label}"))
}

pub fn command_exists(_command: &str) -> bool {
    false
}

pub fn nvidia_smi_candidates() -> Vec<&'static str> {
    vec!["nvidia-smi"]
}

pub fn ram_snapshot() -> Option<RamSnapshot> {
    None
}

pub fn user_home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
}

pub fn platform_compat_path(value: &str) -> PathBuf {
    PathBuf::from(value)
}

pub fn super_permission_is_enabled() -> bool {
    false
}

pub fn enable_super_permission() -> Result<(), String> {
    Err("当前系统不支持 Linux sudo 超级权限开关".to_string())
}

pub fn disable_super_permission() -> Result<(), String> {
    Ok(())
}

pub fn super_permission_turn_reminder() -> &'static str {
    "当前系统不支持 Linux sudo 超级权限开关。需要管理员权限时,请使用系统提供的管理员方式执行,不要尝试 sudo/apt/systemctl/pkexec。"
}

pub fn install_dependencies(_packages: Vec<String>) -> Result<(), String> {
    Err("当前系统不支持一键安装 Linux 依赖；请按本系统方式手动安装缺失工具".into())
}

pub fn check_update_platform_support() -> Result<(), String> {
    Err("当前平台暂不支持应用内 .deb 更新".to_string())
}

pub fn install_update_package(_path: &Path) -> Result<(), String> {
    Err("当前平台暂不支持应用内 .deb 更新".to_string())
}
