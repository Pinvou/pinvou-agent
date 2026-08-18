use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::macos_path;
use objc2_foundation::NSLocale;

pub fn current_system_locale() -> Option<String> {
    NSLocale::preferredLanguages()
        .firstObject()
        .map(|locale| locale.to_string())
        .filter(|locale| !locale.trim().is_empty())
}

// Unix 通用 helper 从 posix.rs 继承（与 linux 侧同一份实现，见 posix.rs）。
pub use super::super::posix::process_alive;

/// 校验路径存在且至少有一个可执行位(owner/group/other 任一有 x bit)。
/// 与 Linux 侧 `which` 自带的可执行性校验对齐:`command_exists` 此前只调
/// `Path::is_file()`,若目录里有同名但无执行权限的残留文件(手动 touch、
/// 损坏的 brew 链接、备份)会误判依赖存在 → 依赖体检假阳性。
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

pub fn open_target(target: impl AsRef<OsStr>, label: &str) -> Result<(), String> {
    Command::new("/usr/bin/open")
        .arg(target.as_ref())
        .spawn()
        .map_err(|e| format!("系统打开失败({label}): {e}"))?;
    Ok(())
}

pub fn reveal_target(target: &Path) -> Result<(), String> {
    Command::new("/usr/bin/open")
        .arg("-R")
        .arg(target)
        .spawn()
        .map_err(|error| format!("文件管理器定位失败: {error}"))?;
    Ok(())
}

pub fn command_exists(command: &str) -> bool {
    // 先走 `which`(含 PATH 内命令),未命中再补查 extra_lookup_dirs() 中的
    // Homebrew bin 与 cask 应用目录(见该函数注释)。
    if Command::new("/usr/bin/which")
        .arg(command)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return true;
    }
    for dir in extra_lookup_dirs() {
        let path = Path::new(dir).join(command);
        // is_file() 后再校验可执行位:避免同名无执行权限的残留(手动 touch、
        // 损坏的 brew 链接、备份)被误判为依赖就位。与 Linux 侧 `which` 对齐。
        if path.is_file() && is_executable(&path) {
            return true;
        }
    }
    false
}

pub fn pandoc_tool_path() -> PathBuf {
    macos_path::pandoc_tool_path()
}

pub fn pandoc_tool_exists() -> bool {
    command_exists("pandoc")
}

pub fn show_pandoc_dependency_check() -> bool {
    true
}

pub fn pandoc_dependency_packages() -> &'static str {
    "pandoc"
}

pub fn pandoc_missing_message() -> &'static str {
    "缺少 pandoc。可通过 Homebrew 安装（brew install pandoc），或从 https://pandoc.org/installing.html 下载。"
}

pub fn pdf_tool_path(command: &str) -> PathBuf {
    PathBuf::from(command)
}

pub fn pdf_tool_exists(command: &str) -> bool {
    command_exists(command)
}

pub fn show_pdf_dependency_check() -> bool {
    true
}

pub fn pdf_dependency_packages() -> &'static str {
    "poppler"
}

pub fn ocr_dependency_packages() -> &'static str {
    // 包含 tesseract-lang:Homebrew 的 tesseract formula 默认只装英文语言数据,
    // pinvou3 面向国内政企(中文是刚需),缺 tesseract-lang 时中文 OCR 不可用。
    // tesseract-lang 已在 macos_dependency.rs 的 KNOWN_DEP_PACKAGES 白名单内。
    "tesseract tesseract-lang"
}

pub fn pdf_text_missing_message() -> &'static str {
    "缺少 PDF 文本解析组件 pdftotext。可通过 Homebrew 安装（brew install poppler），或从 https://poppler.freedesktop.org 下载。"
}

pub fn pdf_render_missing_message() -> &'static str {
    "缺少 PDF 渲染组件 pdftoppm。可通过 Homebrew 安装（brew install poppler），或从 https://poppler.freedesktop.org 下载。"
}

pub fn pdf_ocr_missing_message() -> &'static str {
    "缺少 OCR 组件 tesseract。可通过 Homebrew 安装（brew install tesseract），或从 https://tesseract-ocr.github.io 下载。"
}

pub fn presentation_pdf_missing_message() -> &'static str {
    "缺少生成 PDF 所需的 LibreOffice。可通过 Homebrew 安装（brew install --cask libreoffice），或从 https://www.libreoffice.org/download 下载。"
}

// ====== file_ingest.rs 跨平台缺失消息 + 依赖包名 ======
// file_ingest.rs 原先在所有平台上硬编码 "sudo apt install ..." 文案,
// macOS/Windows 用户看到 Linux apt 指令会产生误导。以下函数让每个平台
// 给出自己正确的安装指引,跟已有的 pdf_text_missing_message 等同模式。

pub fn libreoffice_missing_message() -> &'static str {
    "需要 LibreOffice。可通过 Homebrew 安装（brew install --cask libreoffice），或从 https://www.libreoffice.org/download 下载。"
}

pub fn email_dependency_packages() -> &'static str {
    // msgconvert 需要 Perl 模块 Email::Outlook::Message，Homebrew 无对应 formula，
    // 无法一键安装。返回空串 → check_dependencies 的 apt 字段为空 → 前端不显示
    // 「一键安装」按钮，用户需自行通过 `sudo cpan -i Email::Outlook::Message` 安装。
    ""
}

/// macOS 邮件依赖的手动安装指引。msgconvert 来自 Perl 模块 Email::Outlook::Message，
/// 无 Homebrew formula，且非 root 安装实测会坏（模块不在系统 Perl 的 @INC），
/// 只能 `sudo cpan` 装到系统 Perl 路径。返回语义化 hint key（非界面文案），
/// 由前端 i18n 按 `depHint_<key>` 映射当前语言的完整指引（含命令与文档链接）；
/// 不直接返回中文文案，否则英文/日文界面会看到中文，违反三语文案约束。
/// 其他平台返回 None（apt/winget 可装）。
pub fn email_manual_hint() -> Option<&'static str> {
    Some("email_manual")
}

/// Mac 无 NVIDIA 驱动。
pub fn nvidia_smi_candidates() -> Vec<&'static str> {
    Vec::new()
}

/// 专用有头 Chrome 的可执行候选（macOS 以应用内可执行文件绝对路径为主；
/// 含 `~/Applications` 用户级安装——仅装用户级 Chrome 的机器否则永远探测不到）。
/// 与 `browser-wrapper.mjs` 的 darwin 候选保持一致（漂移时两侧同步）。
pub fn chrome_candidates() -> Vec<String> {
    let mut candidates = vec![
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_string(),
        "/Applications/Chromium.app/Contents/MacOS/Chromium".to_string(),
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser".to_string(),
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge".to_string(),
    ];
    // `~/Applications` 用户级安装（HOME 缺失时不产出相对路径候选，避免随 cwd 误判）。
    if let Some(home) = std::env::var_os("HOME") {
        let home_apps = std::path::PathBuf::from(home).join("Applications");
        for name in [
            "Google Chrome.app/Contents/MacOS/Google Chrome",
            "Chromium.app/Contents/MacOS/Chromium",
        ] {
            candidates.push(home_apps.join(name).to_string_lossy().into_owned());
        }
    }
    candidates
}

/// 随安装包捆绑的 node：macOS 复用 codex-bridge 运行时 node（无捆绑时 None，
/// 消费方回退系统 PATH 探测）。
pub fn bundled_node() -> Option<PathBuf> {
    crate::platform::paths::bundled_connector_node()
}

pub fn libreoffice_tool_path() -> PathBuf {
    if command_exists("soffice") {
        PathBuf::from("soffice")
    } else {
        PathBuf::from("/Applications/LibreOffice.app/Contents/MacOS/soffice")
    }
}

pub fn ocr_tool_path() -> PathBuf {
    PathBuf::from("tesseract")
}

pub fn ocr_tessdata_dir() -> Option<PathBuf> {
    None
}

pub fn archive_tool_path() -> PathBuf {
    if command_exists("7zz") {
        PathBuf::from("7zz")
    } else {
        PathBuf::from("7z")
    }
}

pub fn ocr_tool_exists() -> bool {
    command_exists("tesseract")
}
pub fn archive_tool_exists() -> bool {
    command_exists("7zz") || command_exists("7z")
}
pub fn msg_native_supported() -> bool {
    false
}
pub fn msg_converter_required() -> bool {
    true
}
pub fn email_tool_exists() -> bool {
    command_exists("python3") && command_exists("msgconvert")
}
pub fn show_ocr_dependency_check() -> bool {
    true
}
pub fn show_archive_dependency_check() -> bool {
    true
}
pub fn archive_dependency_packages() -> &'static str {
    "p7zip"
}
pub fn system_default_open_supported(_path: &Path) -> bool {
    true
}
pub fn libreoffice_open_fallback_needed(_path: &Path) -> bool {
    false
}

/// command_exists 补查的目录列表(Homebrew bin + cask 应用 MacOS 目录)。
/// GUI 进程不继承 shell PATH,brew 装的 CLI 与 cask 应用(LibreOffice 装在
/// /Applications/*.app/Contents/MacOS/)都查不到 → 收敛为单一来源,
/// 供 command_exists 与测试共用,防止实现与契约两处漂移。
fn extra_lookup_dirs() -> &'static [&'static str] {
    &[
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/Applications/LibreOffice.app/Contents/MacOS",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 确保生产补查目录列表(见 extra_lookup_dirs,command_exists 的唯一来源)
    /// 包含 cask 应用路径(LibreOffice)与两代 Homebrew 目录。若有人误删任一
    /// 路径,此测试会失败 → 防止 office 文档转换/依赖体检回归。
    #[test]
    fn extra_lookup_dirs_includes_cask_paths() {
        let dirs = extra_lookup_dirs();
        assert!(
            dirs.contains(&"/Applications/LibreOffice.app/Contents/MacOS"),
            "LibreOffice cask 路径必须在补查目录列表内,否则 office 文档转换不可用"
        );
        assert!(
            dirs.contains(&"/opt/homebrew/bin"),
            "Apple Silicon Homebrew 路径必须在补查目录列表内"
        );
        assert!(
            dirs.contains(&"/usr/local/bin"),
            "Intel Mac Homebrew 路径必须在补查目录列表内"
        );
    }
}
