use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use tauri::AppHandle;

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

pub async fn check_for_update_info(
    _client: &reqwest::Client,
    current_version: &str,
) -> Result<crate::updater::UpdateInfo, String> {
    Ok(crate::updater::UpdateInfo {
        available: false,
        current_version: current_version.to_string(),
        latest_version: current_version.to_string(),
        notes: String::new(),
        pub_date: String::new(),
        url: String::new(),
        sha256: String::new(),
        size: 0,
        package_md5: String::new(),
        software_id: String::new(),
        sn: String::new(),
        update_type: String::new(),
        platform: std::env::consts::OS.to_string(),
    })
}

pub async fn download_update_package(
    _info: &crate::updater::UpdateInfo,
    _app: AppHandle,
    _cancel: &AtomicBool,
    _stall_timeout: Duration,
) -> Result<crate::updater::DownloadUpdateResult, String> {
    Err("当前平台暂不支持应用内更新".to_string())
}

pub fn install_update_package(_path: &Path) -> Result<(), String> {
    Err("当前平台暂不支持应用内 .deb 更新".to_string())
}

pub fn install_downloaded_update(
    _deb_path: Option<String>,
    _installer_path: Option<String>,
    _info: Option<crate::updater::UpdateInfo>,
) -> Result<bool, String> {
    Err("当前平台暂不支持应用内更新".to_string())
}

pub async fn report_pending_update_result_info(
    _client: &reqwest::Client,
    _current_version: &str,
) -> Result<crate::updater::PendingUpdateReportResult, String> {
    Ok(crate::updater::PendingUpdateReportResult {
        had_pending: false,
        reported: false,
        result: String::new(),
        message: "当前平台没有待反馈升级结果".to_string(),
    })
}
