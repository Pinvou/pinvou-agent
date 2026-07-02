#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "windows")]
pub(crate) mod windows;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(crate) mod unsupported;

mod interface;

#[cfg(target_os = "linux")]
pub(crate) use linux as platform;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(crate) use unsupported as platform;
#[cfg(target_os = "windows")]
pub(crate) use windows as platform;

pub use interface::{
    asr_bundled_runtime_status, asr_dependency_packages, asr_missing_message, asr_tool_exists,
    asr_tool_path, check_for_update_info, command_exists, disable_super_permission,
    download_update_package, enable_super_permission, install_dependencies, install_downloaded_update,
    ocr_dependency_packages, pandoc_dependency_packages,
    pandoc_missing_message, pandoc_tool_exists, pandoc_tool_path, pdf_dependency_packages,
    pdf_ocr_missing_message, pdf_render_missing_message, pdf_text_missing_message, pdf_tool_exists,
    pdf_tool_path, platform_compat_path, presentation_pdf_missing_message,
    report_pending_update_result_info, show_pandoc_dependency_check, show_pdf_dependency_check,
    super_permission_is_enabled, super_permission_turn_reminder, user_home_dir,
};
