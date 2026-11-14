mod clipboard;
mod path;
mod permission;
mod system;

pub use clipboard::read_clipboard_image;

pub use path::{
    apply_user_npm_prefix, configure_onnxruntime_dylib, connector_cli_command,
    external_application_path, file_url_from_path, filesystem_path_identity_key, kill_pid_tree,
    null_device, obsidian_config_path, path_component_eq, path_component_prefix,
    path_component_suffix, path_identity_is_same_or_nested, path_relative_suffix_under,
    path_starts_with_folded, platform_compat_path, python_command, user_home_dir,
    validate_upload_location,
};
// The folding primitives are consumed by the os adapters through this module
// path (`super::interface::volume_fold_*`); each alias's allowance is active
// exactly on the platforms where it is unreferenced, so a stopped consumer
// resurfaces as an unused-import tripwire (round-5 review: a blanket allow
// removed that tripwire). macOS (posix.rs) and Windows (windows_path.rs)
// consume eq/prefix/suffix; `volume_fold_spelling` is consumed only by the
// Windows identity key (round-15 wave) — its İ-expansion behavior is pinned
// by a test in `path.rs`. The E0425 class is about a MISSING re-export,
// which stays impossible: every consumed alias is unconditional in these
// lists, and a consumer on a new platform only widens its allowance.
#[cfg_attr(not(target_os = "windows"), allow(unused_imports))]
pub(crate) use path::volume_fold_spelling;
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    allow(unused_imports)
)]
pub(crate) use path::{volume_fold_eq, volume_fold_prefix, volume_fold_suffix};
pub use permission::{
    disable_super_permission, enable_super_permission, super_permission_is_enabled,
    super_permission_turn_reminder,
};
pub use system::{
    archive_dependency_packages, archive_tool_exists, archive_tool_path, bundled_node,
    command_exists, current_system_locale, email_dependency_packages, email_manual_hint,
    email_tool_exists, libreoffice_missing_message, libreoffice_open_fallback_needed,
    libreoffice_tool_path, make_private_dir, msg_converter_required, nvidia_smi_candidates,
    ocr_dependency_packages, ocr_tessdata_dir, ocr_tool_exists, ocr_tool_path, open_target,
    pandoc_dependency_packages, pandoc_missing_message, pandoc_tool_exists, pandoc_tool_path,
    pdf_dependency_packages, pdf_ocr_missing_message, pdf_render_missing_message,
    pdf_text_missing_message, pdf_tool_exists, pdf_tool_path, presentation_pdf_missing_message,
    process_alive, reveal_target, show_archive_dependency_check, show_ocr_dependency_check,
    show_pandoc_dependency_check, show_pdf_dependency_check, spawn_detached_and_reap,
    startup_platform_env, system_default_open_supported, try_make_private_dir,
};
