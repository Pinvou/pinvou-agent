mod dependency;
mod memory;
mod path;
mod permission;
mod system;
mod update;

pub use dependency::install_dependencies;
pub use memory::ram_snapshot;
pub use path::{platform_compat_path, user_home_dir};
pub use permission::{
    disable_super_permission, enable_super_permission, super_permission_is_enabled,
    super_permission_turn_reminder,
};
pub use system::{
    command_exists, nvidia_smi_candidates, ocr_dependency_packages, open_target,
    pdf_dependency_packages, pdf_ocr_missing_message, pdf_render_missing_message,
    pdf_text_missing_message, pdf_tool_exists, pdf_tool_path, presentation_pdf_missing_message,
    show_pdf_dependency_check,
};
pub use update::{
    check_for_update_info, download_update_package, install_downloaded_update,
    report_pending_update_result_info,
};
