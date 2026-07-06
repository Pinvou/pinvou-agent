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
    archive_dependency_packages, archive_tool_exists, archive_tool_path,
    asr_bundled_runtime_status, asr_dependency_installable, asr_dependency_packages,
    asr_install_unavailable_message, asr_missing_message, asr_model_exists, asr_model_path,
    asr_model_spec, asr_tool_exists, asr_tool_path, check_for_update_info, command_exists,
    disable_super_permission, download_update_package, email_dependency_packages,
    email_tool_exists, enable_super_permission, install_asr_runtime, install_dependencies,
    install_downloaded_update, libreoffice_tool_path, msg_converter_required, msg_native_supported,
    nvidia_smi_candidates, ocr_dependency_packages, ocr_tessdata_dir, ocr_tool_exists,
    ocr_tool_path, open_target, pandoc_dependency_packages, pandoc_missing_message,
    pandoc_tool_exists, pandoc_tool_path, path_component_eq, pdf_dependency_packages,
    pdf_ocr_missing_message, pdf_render_missing_message, pdf_text_missing_message, pdf_tool_exists,
    pdf_tool_path, platform_compat_path, presentation_pdf_missing_message, python_command,
    ram_snapshot, report_pending_update_result_info, show_archive_dependency_check,
    show_ocr_dependency_check, show_pandoc_dependency_check, show_pdf_dependency_check,
    super_permission_is_enabled, super_permission_turn_reminder, user_home_dir,
    validate_upload_location,
};
