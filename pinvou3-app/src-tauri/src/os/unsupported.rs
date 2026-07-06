use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use tauri::AppHandle;

use crate::monitor::RamSnapshot;

pub fn open_target(_target: impl AsRef<OsStr>, label: &str) -> Result<(), String> {
    Err(format!("当前平台不支持系统打开: {label}"))
}

pub fn command_exists(_command: &str) -> bool {
    false
}

pub fn pandoc_tool_path() -> PathBuf {
    PathBuf::from("pandoc")
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

pub fn asr_tool_path() -> PathBuf {
    PathBuf::from("paddlespeech")
}

pub fn asr_model_filename() -> &'static str {
    "sense-voice-small-q4_k.gguf"
}

pub fn asr_model_spec() -> crate::voice_asr::AsrModelSpec {
    crate::voice_asr::AsrModelSpec {
        id: "unsupported",
        filename: asr_model_filename(),
        expected_size: 0,
        sha256: "",
        primary_url: "",
        mirror_url: "",
    }
}

pub fn asr_model_path() -> PathBuf {
    user_home_dir()
        .join(".pinvou3")
        .join("asr")
        .join(asr_model_filename())
}

pub fn asr_model_exists() -> bool {
    false
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

pub fn asr_tool_exists() -> bool {
    false
}

pub fn asr_bundled_runtime_status() -> Option<bool> {
    None
}

pub fn asr_dependency_installable() -> bool {
    false
}

pub fn asr_install_unavailable_message() -> &'static str {
    "ASR runtime installation is not supported on this platform."
}

pub async fn install_asr_runtime(_app: tauri::AppHandle) -> Result<(), String> {
    Err(asr_install_unavailable_message().to_string())
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

pub fn asr_dependency_packages() -> &'static str {
    ""
}

pub fn archive_dependency_packages() -> &'static str {
    ""
}

pub fn pandoc_missing_message() -> &'static str {
    "当前平台缺少可用的文档解析组件。"
}

pub fn asr_missing_message() -> &'static str {
    "当前平台缺少可用的本地语音识别组件。"
}

pub fn email_dependency_packages() -> &'static str {
    ""
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

pub fn nvidia_smi_candidates() -> Vec<&'static str> {
    vec!["nvidia-smi"]
}

pub fn ram_snapshot() -> Option<RamSnapshot> {
    None
}

pub fn user_home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
}

pub fn platform_compat_path(value: &str) -> PathBuf {
    PathBuf::from(value)
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

pub fn path_component_eq(component: &OsStr, expected: &str) -> bool {
    component == OsStr::new(expected)
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

pub fn install_dependencies(_packages: Vec<String>) -> Result<(), String> {
    Err("当前系统不支持一键安装 Linux 依赖；请按本系统方式手动安装缺失工具".into())
}

pub fn check_update_platform_support() -> Result<(), String> {
    Err("当前平台暂不支持应用内 .deb 更新".to_string())
}

pub async fn check_for_update_info(
    _client: &reqwest::Client,
    current_version: &str,
) -> Result<crate::updater::UpdateInfo, String> {
    Ok(crate::updater::UpdateInfo {
        available: false,
        current_version: current_version.to_string(),
        latest_version: current_version.to_string(),
        notes: String::new(),
        pub_date: String::new(),
        url: String::new(),
        sha256: String::new(),
        size: 0,
        package_md5: String::new(),
        software_id: String::new(),
        sn: String::new(),
        update_type: String::new(),
        platform: std::env::consts::OS.to_string(),
        ota_host: String::new(),
    })
}

pub async fn download_update_package(
    _info: &crate::updater::UpdateInfo,
    _app: AppHandle,
    _cancel: &AtomicBool,
    _stall_timeout: Duration,
) -> Result<crate::updater::DownloadUpdateResult, String> {
    Err("当前平台暂不支持应用内更新".to_string())
}

pub fn install_update_package(_path: &Path) -> Result<(), String> {
    Err("当前平台暂不支持应用内 .deb 更新".to_string())
}

pub fn install_downloaded_update(
    _deb_path: Option<String>,
    _installer_path: Option<String>,
    _info: Option<crate::updater::UpdateInfo>,
) -> Result<bool, String> {
    Err("当前平台暂不支持应用内更新".to_string())
}

pub async fn report_pending_update_result_info(
    _client: &reqwest::Client,
    _current_version: &str,
) -> Result<crate::updater::PendingUpdateReportResult, String> {
    Ok(crate::updater::PendingUpdateReportResult {
        had_pending: false,
        reported: false,
        result: String::new(),
        message: "当前平台没有待反馈升级结果".to_string(),
    })
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
