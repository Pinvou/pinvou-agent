#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(target_os = "windows")]
pub(crate) mod windows;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(crate) mod unsupported;

mod interface;

#[cfg(target_os = "linux")]
pub(crate) use linux as platform;
#[cfg(target_os = "macos")]
pub(crate) use macos as platform;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(crate) use unsupported as platform;
#[cfg(target_os = "windows")]
pub(crate) use windows as platform;

pub use interface::{
    apply_user_npm_prefix, archive_dependency_packages, archive_tool_exists, archive_tool_path,
    command_exists, configure_onnxruntime_dylib, connector_cli_command, disable_super_permission,
    email_dependency_packages, email_tool_exists, enable_super_permission,
    external_application_path, file_url_from_path, kill_pid_tree, libreoffice_missing_message,
    libreoffice_open_fallback_needed, libreoffice_tool_path, msg_converter_required,
    msg_native_supported, nvidia_smi_candidates, obsidian_config_path, ocr_dependency_packages,
    ocr_tessdata_dir, ocr_tool_exists, ocr_tool_path, open_target, pandoc_dependency_packages,
    pandoc_missing_message, pandoc_tool_exists, pandoc_tool_path, path_component_eq,
    pdf_dependency_packages, pdf_ocr_missing_message, pdf_render_missing_message,
    pdf_text_missing_message, pdf_tool_exists, pdf_tool_path, platform_compat_path,
    presentation_pdf_missing_message, python_command, reveal_target, show_archive_dependency_check,
    show_ocr_dependency_check, show_pandoc_dependency_check, show_pdf_dependency_check,
    super_permission_is_enabled, super_permission_turn_reminder, system_default_open_supported,
    user_home_dir, validate_upload_location,
};
