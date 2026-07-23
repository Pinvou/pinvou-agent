#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "windows")]
pub(crate) mod windows;
#[cfg(target_os = "macos")]
pub(crate) mod macos;

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub(crate) mod unsupported;

mod interface;

#[cfg(target_os = "linux")]
pub(crate) use linux as platform;
#[cfg(target_os = "windows")]
pub(crate) use windows as platform;
#[cfg(target_os = "macos")]
pub(crate) use macos as platform;
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub(crate) use unsupported as platform;

pub use interface::{
    asr_bundled_runtime_status, asr_dependency_packages, asr_missing_message, asr_tool_exists,
    asr_tool_path, check_for_update_info, command_exists, disable_super_permission,
    download_update_package, email_dependency_packages, enable_super_permission,
    install_dependencies, install_downloaded_update, libreoffice_dependency_packages,
    libreoffice_missing_message, msgconvert_missing_message, ocr_dependency_packages,
    pandoc_dependency_packages, pandoc_missing_message, pandoc_tool_exists, pandoc_tool_path,
    pdf_dependency_packages, pdf_ocr_missing_message, pdf_render_missing_message,
    pdf_text_missing_message, pdf_tool_exists, pdf_tool_path, platform_compat_path,
    presentation_pdf_missing_message, python3_missing_message, report_pending_update_result_info,
    sevenzip_dependency_packages, sevenzip_missing_message, show_pandoc_dependency_check,
    show_pdf_dependency_check, super_permission_is_enabled, super_permission_turn_reminder,
    user_home_dir,
};
