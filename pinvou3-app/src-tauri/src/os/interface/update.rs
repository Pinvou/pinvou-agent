use std::sync::atomic::AtomicBool;
use std::time::Duration;

use tauri::AppHandle;

pub async fn check_for_update_info(
    client: &reqwest::Client,
    current_version: &str,
) -> Result<crate::updater::UpdateInfo, String> {
    super::super::platform::check_for_update_info(client, current_version).await
}

pub async fn download_update_package(
    info: &crate::updater::UpdateInfo,
    app: AppHandle,
    cancel: &AtomicBool,
    stall_timeout: Duration,
) -> Result<crate::updater::DownloadUpdateResult, String> {
    super::super::platform::download_update_package(info, app, cancel, stall_timeout).await
}

pub fn install_downloaded_update(
    deb_path: Option<String>,
    installer_path: Option<String>,
    info: Option<crate::updater::UpdateInfo>,
) -> Result<bool, String> {
    super::super::platform::install_downloaded_update(deb_path, installer_path, info)
}

pub async fn report_pending_update_result_info(
    client: &reqwest::Client,
    current_version: &str,
) -> Result<crate::updater::PendingUpdateReportResult, String> {
    super::super::platform::report_pending_update_result_info(client, current_version).await
}
