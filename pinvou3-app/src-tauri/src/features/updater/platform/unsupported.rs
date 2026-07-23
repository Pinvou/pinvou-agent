use std::sync::atomic::AtomicBool;
use std::time::Duration;

use tauri::AppHandle;

use super::super::{DownloadUpdateResult, PendingUpdateReportResult, UpdateInfo};

pub fn cleanup_stale_backup() {}

pub async fn check_for_update_info(
    _client: &reqwest::Client,
    current_version: &str,
) -> Result<UpdateInfo, String> {
    Ok(UpdateInfo {
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
        ota_host: String::new(),
    })
}

pub async fn download_update_package(
    _info: &UpdateInfo,
    _app: AppHandle,
    _cancel: &AtomicBool,
    _stall_timeout: Duration,
) -> Result<DownloadUpdateResult, String> {
    Err("当前平台暂不支持应用内更新".to_string())
}

pub fn install_downloaded_update(
    _deb_path: Option<String>,
    _installer_path: Option<String>,
    _info: Option<UpdateInfo>,
) -> Result<bool, String> {
    Err("当前平台暂不支持应用内更新".to_string())
}

pub async fn report_pending_update_result_info(
    _client: &reqwest::Client,
    _current_version: &str,
) -> Result<PendingUpdateReportResult, String> {
    Ok(PendingUpdateReportResult {
        had_pending: false,
        reported: false,
        result: String::new(),
        message: "当前平台没有待反馈升级结果".to_string(),
    })
}
