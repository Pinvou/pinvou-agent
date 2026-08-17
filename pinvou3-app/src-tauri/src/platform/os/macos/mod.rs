//! macOS platform adapter.
//!
//! macOS currently keeps the explicit unsupported behavior for capabilities
//! that have not been implemented yet. Keeping a dedicated adapter makes each
//! future capability an intentional macOS change instead of falling through an
//! unknown-platform branch.

pub use super::unsupported::*;

mod macos_path;
mod macos_permission;
mod macos_system;

pub use macos_path::{
    apply_user_npm_prefix, connector_cli_command, filesystem_path_identity_key,
    platform_compat_path, user_home_dir,
};
pub use macos_permission::{
    disable_super_permission, enable_super_permission, super_permission_is_enabled,
    super_permission_turn_reminder,
};
pub use macos_system::{
    archive_dependency_packages, archive_tool_exists, archive_tool_path, bundled_node,
    chrome_candidates, command_exists, current_system_locale, email_dependency_packages,
    email_manual_hint, email_tool_exists, libreoffice_missing_message,
    libreoffice_open_fallback_needed, libreoffice_tool_path, msg_converter_required,
    msg_native_supported, nvidia_smi_candidates, ocr_dependency_packages, ocr_tessdata_dir,
    ocr_tool_exists, ocr_tool_path, open_target, pandoc_dependency_packages,
    pandoc_missing_message, pandoc_tool_exists, pandoc_tool_path, pdf_dependency_packages,
    pdf_ocr_missing_message, pdf_render_missing_message, pdf_text_missing_message, pdf_tool_exists,
    pdf_tool_path, presentation_pdf_missing_message, reveal_target, show_archive_dependency_check,
    show_ocr_dependency_check, show_pandoc_dependency_check, show_pdf_dependency_check,
    system_default_open_supported,
};

use std::path::PathBuf;

pub fn obsidian_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join("Library/Application Support/obsidian/obsidian.json"))
}
