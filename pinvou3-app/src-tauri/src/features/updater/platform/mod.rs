mod unsupported;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub(super) use windows::show_startup_admission_failure;
#[cfg(not(target_os = "windows"))]
pub(super) fn show_startup_admission_failure(title: &str, message: &str) {
    // Configured Windows host profiles are unsupported here. Preserve the
    // localized fatal diagnostic without borrowing a Windows implementation.
    eprintln!("{title}\n\n{message}");
}

pub use unsupported::{
    check_for_update_info, cleanup_stale_backup, download_update_package,
    install_downloaded_update, report_pending_update_result_info,
};
