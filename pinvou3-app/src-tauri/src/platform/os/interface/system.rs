use std::ffi::OsStr;

use std::path::{Path, PathBuf};

pub fn open_target(target: impl AsRef<OsStr>, label: &str) -> Result<(), String> {
    super::super::platform::open_target(target, label)
}

pub fn reveal_target(target: &Path) -> Result<(), String> {
    super::super::platform::reveal_target(target)
}

pub fn command_exists(command: &str) -> bool {
    super::super::platform::command_exists(command)
}

pub fn current_system_locale() -> Option<String> {
    super::super::platform::current_system_locale()
}

#[cfg(target_os = "windows")]
pub fn bios_serial_number() -> Result<String, String> {
    super::super::platform::bios_serial_number()
}

pub fn pandoc_tool_path() -> PathBuf {
    super::super::platform::pandoc_tool_path()
}

pub fn libreoffice_tool_path() -> PathBuf {
    super::super::platform::libreoffice_tool_path()
}

pub fn libreoffice_missing_message() -> &'static str {
    super::super::platform::libreoffice_missing_message()
}

pub fn ocr_tool_path() -> PathBuf {
    super::super::platform::ocr_tool_path()
}

pub fn ocr_tessdata_dir() -> Option<PathBuf> {
    super::super::platform::ocr_tessdata_dir()
}

pub fn archive_tool_path() -> PathBuf {
    super::super::platform::archive_tool_path()
}

pub fn pandoc_tool_exists() -> bool {
    super::super::platform::pandoc_tool_exists()
}

pub fn ocr_tool_exists() -> bool {
    super::super::platform::ocr_tool_exists()
}

pub fn archive_tool_exists() -> bool {
    super::super::platform::archive_tool_exists()
}

pub fn msg_native_supported() -> bool {
    super::super::platform::msg_native_supported()
}

pub fn msg_converter_required() -> bool {
    super::super::platform::msg_converter_required()
}

pub fn email_tool_exists() -> bool {
    super::super::platform::email_tool_exists()
}

pub fn show_pandoc_dependency_check() -> bool {
    super::super::platform::show_pandoc_dependency_check()
}

pub fn show_ocr_dependency_check() -> bool {
    super::super::platform::show_ocr_dependency_check()
}

pub fn show_archive_dependency_check() -> bool {
    super::super::platform::show_archive_dependency_check()
}

pub fn pandoc_dependency_packages() -> &'static str {
    super::super::platform::pandoc_dependency_packages()
}

pub fn archive_dependency_packages() -> &'static str {
    super::super::platform::archive_dependency_packages()
}

pub fn email_dependency_packages() -> &'static str {
    super::super::platform::email_dependency_packages()
}

pub fn email_manual_hint() -> Option<&'static str> {
    super::super::platform::email_manual_hint()
}

pub fn pandoc_missing_message() -> &'static str {
    super::super::platform::pandoc_missing_message()
}

pub fn pdf_tool_path(command: &str) -> PathBuf {
    super::super::platform::pdf_tool_path(command)
}

pub fn pdf_tool_exists(command: &str) -> bool {
    super::super::platform::pdf_tool_exists(command)
}

pub fn show_pdf_dependency_check() -> bool {
    super::super::platform::show_pdf_dependency_check()
}

pub fn pdf_dependency_packages() -> &'static str {
    super::super::platform::pdf_dependency_packages()
}

pub fn ocr_dependency_packages() -> &'static str {
    super::super::platform::ocr_dependency_packages()
}

pub fn pdf_text_missing_message() -> &'static str {
    super::super::platform::pdf_text_missing_message()
}

pub fn pdf_render_missing_message() -> &'static str {
    super::super::platform::pdf_render_missing_message()
}

pub fn pdf_ocr_missing_message() -> &'static str {
    super::super::platform::pdf_ocr_missing_message()
}

pub fn presentation_pdf_missing_message() -> &'static str {
    super::super::platform::presentation_pdf_missing_message()
}

pub fn system_default_open_supported(path: &Path) -> bool {
    super::super::platform::system_default_open_supported(path)
}

pub fn libreoffice_open_fallback_needed(path: &Path) -> bool {
    super::super::platform::libreoffice_open_fallback_needed(path)
}

pub fn nvidia_smi_candidates() -> Vec<&'static str> {
    super::super::platform::nvidia_smi_candidates()
}

/// 浏览器功能专用有头 Chrome 的可执行候选（绝对路径或 PATH 命令名）。
/// 由各平台实现（macos/linux/windows/unsupported），消费方做存在性/`which` 探测。
/// 返回 `String` 而非 `&'static str`：Windows 用户级安装路径（LOCALAPPDATA）是
/// 运行时环境变量，无法静态表达。
pub fn chrome_candidates() -> Vec<String> {
    super::super::platform::chrome_candidates()
}

/// 按 [`chrome_candidates`] 探测首个可用的 Chrome/Chromium 可执行：绝对路径直接
/// 判存在，命令名经 PATH 解析（Windows 裸命令名补 .exe）。通用探测逻辑集中在
/// 接口层，features/browser 与 runtime_bundle 共用，避免双份实现漂移。
pub fn find_chrome() -> Option<std::path::PathBuf> {
    for c in chrome_candidates() {
        let p = std::path::PathBuf::from(&c);
        if p.is_absolute() && p.exists() {
            return Some(p);
        }
        // Windows 可执行带 .exe，裸命令名（candidates 含 "chrome"/"msedge"）在
        // PATH 上必须补扩展名才探测得到。经 env::consts 而非 cfg! 宏，避免
        // 调用侧出现平台条件编译（架构守卫）。
        let candidates: [std::path::PathBuf; 2] =
            if std::env::consts::OS == "windows" && p.extension().is_none() {
                [p.clone(), p.with_extension("exe")]
            } else {
                [p.clone(), p.clone()]
            };
        if let Ok(paths) = std::env::var("PATH") {
            for dir in std::env::split_paths(&paths) {
                for cand in &candidates {
                    let hit = dir.join(cand);
                    if hit.is_file() {
                        return Some(hit);
                    }
                }
            }
        }
    }
    None
}

/// 随安装包捆绑的 node 可执行（浏览器 MCP 等运行时使用；无捆绑时 None）。
/// linux/macos 复用 codex-bridge 运行时 node；Windows 用安装器释放的
/// `runtime/node/node.exe`（连接器链路同款，见 windows_path::bundled_node_dir）。
pub fn bundled_node() -> Option<std::path::PathBuf> {
    super::super::platform::bundled_node()
}

/// 进程存活探测（browser watch 删除 stale 端口文件前的持有者护栏）。
/// 平台差异必须在适配层实现，消费方（features）不得直接写 `#[cfg(unix)]`。
pub fn process_alive(pid: u32) -> bool {
    super::super::platform::process_alive(pid)
}

/// 收紧文件权限为仅当前用户可读写（browser 端口文件等无鉴权敏感文件的落盘路径）。
/// Unix 侧 chmod 0o600（与 browser-wrapper.mjs 一致）；Windows 无 POSIX 权限语义为 no-op。
/// 平台差异必须在适配层实现，消费方（features）不得直接写 `#[cfg(unix)]`。
#[cfg(unix)]
pub fn make_private_file(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    // 收紧失败不得静默吞掉：无鉴权敏感文件以宽松权限残留是安全问题，打警告便于诊断。
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        eprintln!("[platform] 收紧文件权限失败（{}）: {e}", path.display());
    }
}

/// 非 Unix 平台 no-op（Windows 无 POSIX 权限语义）。
#[cfg(not(unix))]
pub fn make_private_file(_path: &Path) {}

/// 收紧目录权限为仅当前用户可访问（browser profile 等含登录会话/缓存的目录）。
/// Unix 侧 chmod 0o700；Windows 无 POSIX 权限语义为 no-op（靠用户目录 ACL）。
#[cfg(unix)]
pub fn make_private_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    // 收紧失败不得静默吞掉：含登录会话的目录以宽松权限残留是安全问题，打警告便于诊断。
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)) {
        eprintln!("[platform] 收紧目录权限失败（{}）: {e}", path.display());
    }
}

/// 非 Unix 平台 no-op（Windows 无 POSIX 权限语义）。
#[cfg(not(unix))]
pub fn make_private_dir(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_smi_candidates_starts_with_generic_command() {
        let candidates = nvidia_smi_candidates();
        if !candidates.is_empty() {
            assert_eq!(candidates.first().copied(), Some("nvidia-smi"));
        }
    }

    #[test]
    fn pdf_tool_path_returns_non_empty_program() {
        assert!(!pdf_tool_path("pdftotext").as_os_str().is_empty());
    }

    #[test]
    fn pandoc_tool_path_returns_non_empty_program() {
        assert!(!pandoc_tool_path().as_os_str().is_empty());
    }

    #[test]
    fn libreoffice_tool_path_returns_non_empty_program() {
        assert!(!libreoffice_tool_path().as_os_str().is_empty());
    }

    #[test]
    fn ocr_tool_path_returns_non_empty_program() {
        assert!(!ocr_tool_path().as_os_str().is_empty());
    }

    #[test]
    fn archive_tool_path_returns_non_empty_program() {
        assert!(!archive_tool_path().as_os_str().is_empty());
    }

    /// 浏览器专用 Chrome 候选（macos/linux/windows）必须非空，且应包含 Chrome/Chromium。
    ///
    /// 接线门禁：`chrome_candidates()` 经 interface 委托 `platform::chrome_candidates()`，
    /// 各平台 `mod.rs` 必须 re-export `*_system::chrome_candidates`——若漏接线，macOS 会
    /// 经 `pub use super::unsupported::*` glob 静默解析到空列表（编译通过但运行时永远
    /// "未找到 Chrome"），Linux/Windows 则直接 E0425 编译失败。本测试在 macOS 上立即
    /// 暴露空列表回归；Linux/Windows 由交叉编译/CI 编译门禁拦截。
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    #[test]
    fn chrome_candidates_non_empty_on_desktop() {
        let candidates = chrome_candidates();
        assert!(
            !candidates.is_empty(),
            "桌面平台 chrome_candidates 不应为空（检查 platform/os/<os>/mod.rs 是否 re-export chrome_candidates）"
        );
        let joined = candidates.join("|").to_ascii_lowercase();
        assert!(
            joined.contains("chrome") || joined.contains("chromium"),
            "桌面平台 chrome_candidates 应包含 Chrome/Chromium 候选，实际: {joined}"
        );
    }
}
