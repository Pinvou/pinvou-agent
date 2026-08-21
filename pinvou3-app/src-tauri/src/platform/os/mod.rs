#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(target_os = "windows")]
pub(crate) mod windows;

/// Unix 通用 helper（linux 与 macOS 共享）。
#[cfg(unix)]
pub(crate) mod posix;

// `unsupported` 是「尚未实现能力」的桩集合,供 macos 借用未实现的符号(见
// `macos/mod.rs` 的 `pub use super::unsupported::*`),并在非三大桌面平台上作为
// 默认 platform。因此除 linux/windows 外均需声明此模块。
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(crate) mod unsupported;

// GNU 消息 locale 语义的环境变量探测，仅 linux 与 unsupported 平台消费
// (windows/macos 各有原生实现)，按 posix.rs 先例门控到消费平台并集，
// 避免在非消费平台累积 dead_code(见 [lints] 中 dead_code 升 deny 计划)。
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod locale;

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
    bundled_node, command_exists, configure_onnxruntime_dylib, connector_cli_command,
    current_system_locale, disable_super_permission, email_dependency_packages, email_manual_hint,
    email_tool_exists, enable_super_permission, external_application_path, file_url_from_path,
    filesystem_path_identity_key, find_chrome, kill_pid_tree, libreoffice_missing_message,
    libreoffice_open_fallback_needed, libreoffice_tool_path, make_private_dir,
    msg_converter_required, msg_native_supported, nvidia_smi_candidates, obsidian_config_path,
    ocr_dependency_packages, ocr_tessdata_dir, ocr_tool_exists, ocr_tool_path, open_target,
    pandoc_dependency_packages, pandoc_missing_message, pandoc_tool_exists, pandoc_tool_path,
    path_component_eq, pdf_dependency_packages, pdf_ocr_missing_message,
    pdf_render_missing_message, pdf_text_missing_message, pdf_tool_exists, pdf_tool_path,
    platform_compat_path, presentation_pdf_missing_message, process_alive, python_command,
    reveal_target, show_archive_dependency_check, show_ocr_dependency_check,
    show_pandoc_dependency_check, show_pdf_dependency_check, super_permission_is_enabled,
    super_permission_turn_reminder, system_default_open_supported, user_home_dir,
    validate_upload_location, write_private_file,
};
