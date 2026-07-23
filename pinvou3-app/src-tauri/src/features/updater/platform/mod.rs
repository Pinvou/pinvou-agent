#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
mod windows_domain_bootstrap;

#[cfg(target_os = "linux")]
pub use linux::{
    check_for_update_info, cleanup_stale_backup, download_update_package,
    install_downloaded_update, report_pending_update_result_info,
};
#[cfg(target_os = "macos")]
pub use macos::{
    check_for_update_info, cleanup_stale_backup, download_update_package,
    install_downloaded_update, report_pending_update_result_info,
};
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub use unsupported::{
    check_for_update_info, cleanup_stale_backup, download_update_package,
    install_downloaded_update, report_pending_update_result_info,
};
#[cfg(target_os = "windows")]
pub use windows::{
    check_for_update_info, cleanup_stale_backup, download_update_package,
    install_downloaded_update, report_pending_update_result_info,
};
