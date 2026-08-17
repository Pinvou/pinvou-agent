mod windows_path;
mod windows_permission;
mod windows_system;

pub use windows_path::{
    apply_user_npm_prefix, configure_onnxruntime_dylib, connector_cli_command,
    filesystem_path_identity_key, kill_pid_tree, obsidian_config_path, path_component_eq,
    platform_compat_path, python_command, user_home_dir, validate_upload_location,
};
pub(crate) use windows_path::{asr_model_path, bundled_asr_backend_path, bundled_asr_tool_path};
pub use windows_permission::{
    disable_super_permission, enable_super_permission, super_permission_is_enabled,
    super_permission_turn_reminder,
};
pub use windows_system::{
    archive_dependency_packages, archive_tool_exists, archive_tool_path, bios_serial_number,
    bundled_node, chrome_candidates, command_exists, current_system_locale,
    email_dependency_packages, email_manual_hint, email_tool_exists, libreoffice_missing_message,
    libreoffice_open_fallback_needed, libreoffice_tool_path, msg_converter_required,
    msg_native_supported, nvidia_smi_candidates, ocr_dependency_packages, ocr_tessdata_dir,
    ocr_tool_exists, ocr_tool_path, open_target, pandoc_dependency_packages,
    pandoc_missing_message, pandoc_tool_exists, pandoc_tool_path, pdf_dependency_packages,
    pdf_ocr_missing_message, pdf_render_missing_message, pdf_text_missing_message, pdf_tool_exists,
    pdf_tool_path, presentation_pdf_missing_message, process_alive, reveal_target,
    show_archive_dependency_check, show_ocr_dependency_check, show_pandoc_dependency_check,
    show_pdf_dependency_check, system_default_open_supported,
};
