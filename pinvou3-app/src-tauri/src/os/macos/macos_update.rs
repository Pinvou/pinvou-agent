//! macOS OTA：Phase 4 填实。骨架版本返回「不可用」避免阻塞编译。
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use tauri::AppHandle;

pub async fn check_for_update_info(
    _client: &reqwest::Client,
    _current_version: &str,
) -> Result<crate::updater::UpdateInfo, String> {
    Err("macOS OTA 暂未实现（Phase 4）".to_string())
}

pub async fn download_update_package(
    _info: &crate::updater::UpdateInfo,
    _app: AppHandle,
    _cancel: &AtomicBool,
    _stall_timeout: Duration,
) -> Result<crate::updater::DownloadUpdateResult, String> {
    Err("macOS OTA 暂未实现（Phase 4）".to_string())
}

pub fn install_downloaded_update(
    _deb_path: Option<String>,
    _installer_path: Option<String>,
    _info: Option<crate::updater::UpdateInfo>,
) -> Result<bool, String> {
    Err("macOS OTA 暂未实现（Phase 4）".to_string())
}

pub async fn report_pending_update_result_info(
    _client: &reqwest::Client,
    _current_version: &str,
) -> Result<crate::updater::PendingUpdateReportResult, String> {
    Ok(crate::updater::PendingUpdateReportResult {
        had_pending: false,
        reported: true,
        result: "macOS no pending".to_string(),
        message: String::new(),
    })
}

/// 清理上次 OTA 升级残留的旧 app 备份(stub,Phase 4 填实)。
pub fn cleanup_stale_backup() {}
