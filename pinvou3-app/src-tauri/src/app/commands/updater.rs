use super::prelude::*;
use crate::features::updater as updater_domain;
use updater_domain::*;

sync_command_passthrough!(updater_domain, get_app_version() -> String);

/// Local-only diagnostic read; never performs a time HTTP request.
#[tauri::command]
pub async fn get_upgrade_trusted_time_status()
-> Result<updater_domain::trusted_time::TrustedTimeStatus, String> {
    tauri::async_runtime::spawn_blocking(updater_domain::trusted_time::inspect)
        .await
        .map_err(|_| "HOST_UNAVAILABLE".to_owned())?
}
/// Explicit user request. Accepted is not Known time or a cleanup result.
#[tauri::command]
pub async fn request_upgrade_trusted_time_refresh()
-> Result<updater_domain::trusted_time::TrustedTimeRefreshReply, String> {
    tauri::async_runtime::spawn_blocking(updater_domain::trusted_time::request_refresh)
        .await
        .map_err(|_| "HOST_UNAVAILABLE".to_owned())?
}
async_command_passthrough!(updater_domain, check_for_update() -> Result<UpdateInfo, String>);
async_command_passthrough!(
    updater_domain,
    download_update(info: UpdateInfo, app: AppHandle) -> Result<(), String>
);
async_command_passthrough!(
    updater_domain,
    install_update(
        deb_path: Option<String>,
        installer_path: Option<String>,
        info: Option<UpdateInfo>,
        app: AppHandle,
    ) -> Result<(), String>
);
#[tauri::command]
pub async fn restart_app(app: AppHandle) -> Result<(), String> {
    crate::app::lifecycle::restart_application(app).await
}
sync_command_passthrough!(updater_domain, cancel_download());
async_command_passthrough!(updater_domain, report_pending_update_result() -> Result<PendingUpdateReportResult, String>);
