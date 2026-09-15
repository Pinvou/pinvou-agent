use super::prelude::*;
use crate::features::updater as updater_domain;
use updater_domain::*;

sync_command_passthrough!(updater_domain, get_app_version() -> String);
async_command_passthrough!(updater_domain, check_for_update() -> Result<UpdateInfo, String>);
async_command_passthrough!(updater_domain, download_update(info: UpdateInfo, app: AppHandle) -> Result<DownloadUpdateResult, String>);
async_command_passthrough!(updater_domain, install_update(deb_path: Option<String>, installer_path: Option<String>, info: Option<UpdateInfo>, app: AppHandle) -> Result<(), String>);
async_command_passthrough!(updater_domain, restart_app(app: AppHandle) -> Result<(), String>);
sync_command_passthrough!(updater_domain, cancel_download());
async_command_passthrough!(updater_domain, report_pending_update_result() -> Result<PendingUpdateReportResult, String>);
