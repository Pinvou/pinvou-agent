use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;

use super::macos_path;

pub fn open_target(target: impl AsRef<OsStr>, label: &str) -> Result<(), String> {
    Command::new("/usr/bin/open")
        .arg(target.as_ref())
        .spawn()
        .map_err(|e| format!("系统打开失败({label}): {e}"))?;
    Ok(())
}

pub fn command_exists(command: &str) -> bool {
    // 从 dmg/Finder 启动的 GUI 进程不继承 shell 的 PATH,/opt/homebrew/bin(Apple Silicon)
    // 与 /usr/local/bin(Intel) 不在 GUI 进程 PATH 内 → `which` 找不到 brew 装的 pandoc/
    // poppler/tesseract/soffice,依赖体检系统性误报"缺失"。先走 `which`,命中即返回;
    // 未命中再补查这两个标准 Homebrew 目录(macOS 适配最常见的实战坑)。
    if Command::new("/usr/bin/which")
        .arg(command)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return true;
    }
    for dir in ["/opt/homebrew/bin", "/usr/local/bin"] {
        if std::path::Path::new(dir).join(command).is_file() {
            return true;
        }
    }
    // cask 类 GUI 应用(如 LibreOffice)装在 /Applications/*.app/Contents/MacOS/,
    // 不在 Homebrew bin 目录,也不在 GUI 进程 PATH 内 → 依赖体检系统性误报缺失。
    for cask_dir in ["/Applications/LibreOffice.app/Contents/MacOS"] {
        if std::path::Path::new(cask_dir).join(command).is_file() {
            return true;
        }
    }
    false
}

pub fn pandoc_tool_path() -> PathBuf {
    macos_path::pandoc_tool_path()
}

pub fn asr_tool_path() -> PathBuf {
    if let Ok(path) = std::env::var("PINVOU3_ASR_CMD") {
        if !path.trim().is_empty() {
            return PathBuf::from(path);
        }
    }
    // 与 asr_tool_exists 一致:Mac 上的 ASR 执行体是 bundled 引擎本体
    // (sense-voice-darwin-arm64),而非名为 "pinvou-asr" 的独立 shim。回退 CLI
    // 路径(run_local_asr_cli,当进程内 transcribe 不可用时)应 spawn 同一个引擎。
    crate::voice_asr::engine_path()
}

pub fn pandoc_tool_exists() -> bool {
    command_exists("pandoc")
}

pub fn asr_tool_exists() -> bool {
    // 与 linux_system.rs 对齐:优先 PINVOU3_ASR_CMD 覆盖;否则看 bundled 引擎
    // (sense-voice-darwin-arm64)是否就位 —— 它就是 Mac 上的 ASR 执行体,不再
    // 依赖名为 "pinvou-asr" 的独立 CLI(Linux deb 同样以引擎本体作为存在性判据)。
    if let Ok(path) = std::env::var("PINVOU3_ASR_CMD") {
        if !path.trim().is_empty() {
            return std::path::Path::new(&path).is_file();
        }
    }
    crate::voice_asr::engine_path().is_file()
}

pub fn asr_bundled_runtime_status() -> Option<bool> {
    // 对齐 linux_system.rs(返回 None)而非 windows_system.rs(Some):
    // Mac 的 SenseVoice 引擎已随 app 打包为 bundled Mach-O,commands.rs:1314 的
    // 进程内 transcribe 路径(voice_asr::transcribe 直接 spawn 引擎本体)完全适用。
    // 此前返回 Some 会让该判据 `.is_none()` 恒为 false → 永不进进程内分支 → 改走
    // run_local_asr_cli(spawn 不存在的 "pinvou-asr")→ Mac 语音输入静默失效。
    None
}

pub fn show_pandoc_dependency_check() -> bool {
    true
}

pub fn pandoc_dependency_packages() -> &'static str {
    "pandoc"
}

pub fn asr_dependency_packages() -> &'static str {
    // Mac 上 ASR 引擎是 pinvou3 bundled Mach-O(sense-voice-darwin-arm64 随 app 打包),
    // 不是 brew formula。这里返回 "ffmpeg" 因为 ASR 后处理可能依赖 ffmpeg 解码音频格式,
    // 与 deb recommends 对齐。bundled 引擎本身的安装通过应用内更新或 dmg 重装完成。
    "ffmpeg"
}

pub fn pandoc_missing_message() -> &'static str {
    "缺少 pandoc。可通过 Homebrew 安装（brew install pandoc），或从 https://pandoc.org/installing.html 下载。"
}

pub fn asr_missing_message() -> &'static str {
    "缺少本地语音识别组件。请在设置页或应用内提示中安装。"
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

pub fn sevenzip_missing_message() -> &'static str {
    "压缩包解析需要 7-Zip。可通过 Homebrew 安装（brew install p7zip），或从 https://www.7-zip.org 下载。"
}

pub fn python3_missing_message() -> &'static str {
    "邮件解析需要 python3。macOS 自带 python3（需安装 Xcode Command Line Tools：xcode-select --install）；如仍缺失可从 https://www.python.org/downloads 下载。"
}

pub fn msgconvert_missing_message() -> &'static str {
    ".msg 邮件解析需要 msgconvert（Perl 模块 Email::Outlook::Message）。Homebrew 无对应 formula，请运行：sudo cpan -i Email::Outlook::Message，或改用其他工具转换 .msg 文件。"
}

pub fn libreoffice_dependency_packages() -> &'static str {
    "libreoffice"
}

pub fn sevenzip_dependency_packages() -> &'static str {
    "p7zip"
}

pub fn email_dependency_packages() -> &'static str {
    // msgconvert 需要 Perl 模块 Email::Outlook::Message，Homebrew 无对应 formula，
    // 无法一键安装。返回空串 → check_dependencies 的 apt 字段为空 → 前端不显示
    // 「一键安装」按钮，用户按 msgconvert_missing_message 的指引手动安装。
    ""
}

/// Mac 无 NVIDIA 驱动。
pub fn nvidia_smi_candidates() -> Vec<&'static str> {
    Vec::new()
}

/// command_exists 补查的目录列表(Homebrew bin + cask 应用 MacOS 目录)。
/// 抽取为常量便于测试验证:确保 cask 目录(LibreOffice)在列表内,
/// 否则 office 文档转换会被系统性误判为依赖缺失。
#[cfg(test)]
const EXTRA_LOOKUP_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/Applications/LibreOffice.app/Contents/MacOS",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// 确保 command_exists 的补查目录列表包含 cask 应用路径(LibreOffice)。
    /// 若有人误删该路径,此测试会失败 → 防止 office 文档转换功能回归。
    #[test]
    fn extra_lookup_dirs_includes_cask_paths() {
        assert!(
            EXTRA_LOOKUP_DIRS.contains(&"/Applications/LibreOffice.app/Contents/MacOS"),
            "LibreOffice cask 路径必须在补查目录列表内,否则 office 文档转换不可用"
        );
        assert!(
            EXTRA_LOOKUP_DIRS.contains(&"/opt/homebrew/bin"),
            "Apple Silicon Homebrew 路径必须在补查目录列表内"
        );
        assert!(
            EXTRA_LOOKUP_DIRS.contains(&"/usr/local/bin"),
            "Intel Mac Homebrew 路径必须在补查目录列表内"
        );
    }
}
