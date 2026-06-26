mod windows_dependency;
mod windows_domain_bootstrap;
mod windows_memory;
mod windows_path;
mod windows_permission;
mod windows_system;
mod windows_update;

pub use windows_dependency::install_dependencies;
pub use windows_memory::ram_snapshot;
pub use windows_path::{platform_compat_path, user_home_dir};
pub use windows_permission::{
    disable_super_permission, enable_super_permission, super_permission_is_enabled,
    super_permission_turn_reminder,
};
pub use windows_system::{
    asr_dependency_packages, asr_missing_message, asr_tool_exists, asr_tool_path,
    command_exists, nvidia_smi_candidates, ocr_dependency_packages, open_target,
    pandoc_dependency_packages, pandoc_missing_message, pandoc_tool_exists, pandoc_tool_path,
    pdf_dependency_packages, pdf_ocr_missing_message, pdf_render_missing_message,
    pdf_text_missing_message, pdf_tool_exists, pdf_tool_path, presentation_pdf_missing_message,
    show_pandoc_dependency_check, show_pdf_dependency_check,
};
pub use windows_update::{
    check_for_update_info, download_update_package, install_downloaded_update,
    report_pending_update_result_info,
};
