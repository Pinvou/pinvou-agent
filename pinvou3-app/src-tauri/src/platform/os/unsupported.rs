//! 不支持平台(linux/macos/windows 之外)的契约存根。
//!
//! 这些函数构成跨平台 OS trait 的「不支持」分支:当某能力在当前平台未实现时返回
//! Err/None/false。它们在具体平台编译目标下会被 `macos`/`linux`/`windows` 模块的同名
//! 实现通过 glob re-export 阴影,因此 clippy 在某平台下会误报为 dead code,但删除会
//! 破坏其它平台的 `pub use` 解析。整文件豁免 dead_code。
#![allow(dead_code)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

// macOS 侧由 `macos_system::current_system_locale` 显式实现阴影本符号,该目标下
// unused(unused_imports=deny 会拦);其余 unsupported 平台它是唯一来源,保留。
#[cfg(not(target_os = "macos"))]
pub(crate) use super::locale::current_system_locale;

pub fn open_target(_target: impl AsRef<OsStr>, label: &str) -> Result<(), String> {
    Err(format!("当前平台不支持系统打开: {label}"))
}

pub fn reveal_target(target: &Path) -> Result<(), String> {
    Err(format!(
        "当前平台不支持文件管理器定位: {}",
        target.display()
    ))
}

pub fn command_exists(_command: &str) -> bool {
    false
}

pub fn pandoc_tool_path() -> PathBuf {
    PathBuf::from("pandoc")
}

pub fn configure_onnxruntime_dylib() -> Result<(), String> {
    Ok(())
}

pub fn obsidian_config_path() -> Option<PathBuf> {
    None
}

pub fn libreoffice_tool_path() -> PathBuf {
    PathBuf::from("soffice")
}

pub fn ocr_tool_path() -> PathBuf {
    PathBuf::from("tesseract")
}

pub fn ocr_tessdata_dir() -> Option<PathBuf> {
    None
}

pub fn archive_tool_path() -> PathBuf {
    PathBuf::from("7z")
}

pub fn pandoc_tool_exists() -> bool {
    false
}

pub fn ocr_tool_exists() -> bool {
    false
}

pub fn archive_tool_exists() -> bool {
    false
}

pub fn msg_native_supported() -> bool {
    false
}

pub fn msg_converter_required() -> bool {
    false
}

pub fn email_tool_exists() -> bool {
    false
}

pub fn show_pandoc_dependency_check() -> bool {
    false
}

pub fn show_ocr_dependency_check() -> bool {
    false
}

pub fn show_archive_dependency_check() -> bool {
    false
}

pub fn pandoc_dependency_packages() -> &'static str {
    ""
}

pub fn archive_dependency_packages() -> &'static str {
    ""
}

pub fn pandoc_missing_message() -> &'static str {
    "当前平台缺少可用的文档解析组件。"
}

pub fn libreoffice_missing_message() -> &'static str {
    "当前平台缺少可用的 Office 文档预览组件。"
}

pub fn email_dependency_packages() -> &'static str {
    ""
}

pub fn email_manual_hint() -> Option<&'static str> {
    None
}

pub fn pdf_tool_path(command: &str) -> PathBuf {
    PathBuf::from(command)
}

pub fn pdf_tool_exists(_command: &str) -> bool {
    false
}

pub fn show_pdf_dependency_check() -> bool {
    false
}

pub fn pdf_dependency_packages() -> &'static str {
    ""
}

pub fn ocr_dependency_packages() -> &'static str {
    "tesseract"
}

pub fn pdf_text_missing_message() -> &'static str {
    "当前平台缺少可用的 PDF 文本解析组件。"
}

pub fn pdf_render_missing_message() -> &'static str {
    "当前平台缺少可用的 PDF 渲染组件。"
}

pub fn pdf_ocr_missing_message() -> &'static str {
    "当前平台缺少可用的 PDF OCR 组件。"
}

pub fn presentation_pdf_missing_message() -> &'static str {
    "当前平台缺少可用的演示文稿 PDF 文本解析组件。"
}

pub fn system_default_open_supported(_path: &Path) -> bool {
    false
}

pub fn libreoffice_open_fallback_needed(_path: &Path) -> bool {
    false
}

pub fn nvidia_smi_candidates() -> Vec<&'static str> {
    vec!["nvidia-smi"]
}

pub fn user_home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
}

pub fn validate_upload_location(canon: &Path) -> Result<(), String> {
    let home_raw = user_home_dir();
    let home = platform_compat_path(
        &std::fs::canonicalize(&home_raw)
            .unwrap_or_else(|_| home_raw.clone())
            .to_string_lossy(),
    );
    if !canon.starts_with(&home) {
        return Err(format!("path {} not under $HOME", canon.display()));
    }
    Ok(())
}

// Unix 通用 helper 从 posix.rs 继承（Wave 3 去重，与 linux_path.rs 一致）。
// `posix` 模块仅在 `#[cfg(unix)]` 编译，故这里必须守卫：macOS（unix）走
// re-export；其余非 unix 兜底平台走本地等价实现，避免 E0432 未解析模块。
#[cfg(unix)]
pub use super::posix::{path_component_eq, platform_compat_path, python_command};

// macOS 由 macos_path 提供自有实现（APFS 注释在案），经 glob 消费时被显式
// re-export 遮蔽，这里的转发在其编译面上不可达；仅当 unsupported 自身作为
// platform（linux/windows/macos 之外的 unix 兜底目标）时才需要从 posix 继承。
#[cfg(all(unix, not(target_os = "macos")))]
pub use super::posix::filesystem_path_identity_key;

#[cfg(not(unix))]
pub fn path_component_eq(component: &OsStr, expected: &str) -> bool {
    component == OsStr::new(expected)
}

#[cfg(not(unix))]
pub fn filesystem_path_identity_key(path: &str) -> String {
    path.to_string()
}

#[cfg(not(unix))]
pub fn platform_compat_path(value: &str) -> PathBuf {
    PathBuf::from(value)
}

#[cfg(not(unix))]
pub fn python_command() -> String {
    "python3".to_string()
}

pub fn connector_cli_command(_cli_bin: &str, program: &str) -> Command {
    Command::new(program)
}

pub fn apply_user_npm_prefix(_cmd: &mut Command) {}

pub fn kill_pid_tree(pid: u32) {
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
}

pub fn super_permission_is_enabled() -> bool {
    false
}

pub fn enable_super_permission() -> Result<(), String> {
    Err("当前系统不支持 Linux sudo 超级权限开关".to_string())
}

pub fn disable_super_permission() -> Result<(), String> {
    Ok(())
}

pub fn super_permission_turn_reminder() -> &'static str {
    "当前系统不支持 Linux sudo 超级权限开关。需要管理员权限时,请使用系统提供的管理员方式执行,不要尝试 sudo/apt/systemctl/pkexec。"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_archive_runtime_is_not_advertised() {
        assert!(!archive_tool_exists());
        assert!(!show_archive_dependency_check());
        assert_eq!(archive_dependency_packages(), "");
        assert_eq!(archive_tool_path(), PathBuf::from("7z"));
    }

    #[test]
    fn upload_location_rejects_outside_home() {
        assert!(validate_upload_location(Path::new("/etc/passwd")).is_err());
    }
}
