mod macos_dependency;
mod macos_memory;
mod macos_path;
mod macos_permission;
mod macos_system;
mod macos_update;

pub use macos_dependency::install_dependencies;
pub use macos_memory::ram_snapshot;
pub use macos_path::{platform_compat_path, user_home_dir};

pub use macos_permission::{
    disable_super_permission, enable_super_permission, super_permission_is_enabled,
    super_permission_turn_reminder,
};

pub use macos_system::{
    asr_bundled_runtime_status, asr_dependency_packages, asr_missing_message, asr_tool_exists,
    asr_tool_path, command_exists, email_dependency_packages, libreoffice_dependency_packages,
    libreoffice_missing_message, msgconvert_missing_message, nvidia_smi_candidates,
    ocr_dependency_packages, open_target, pandoc_dependency_packages, pandoc_missing_message,
    pandoc_tool_exists, pandoc_tool_path, pdf_dependency_packages, pdf_ocr_missing_message,
    pdf_render_missing_message, pdf_text_missing_message, pdf_tool_exists, pdf_tool_path,
    presentation_pdf_missing_message, python3_missing_message, sevenzip_dependency_packages,
    sevenzip_missing_message, show_pandoc_dependency_check, show_pdf_dependency_check,
};

pub use macos_update::{
    check_for_update_info, cleanup_stale_backup, download_update_package, install_downloaded_update,
    report_pending_update_result_info,
};
