use crate::process::HiddenCommand;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::windows_path;

const ASR_MODEL_URL: &str =
    "https://www.modelscope.cn/models/FunAudioLLM/SenseVoiceSmall-GGUF/resolve/master/sensevoice-small-q8.gguf";
const ASR_MODEL_MIRROR_URL: &str =
    "https://huggingface.co/FunAudioLLM/SenseVoiceSmall-GGUF/resolve/main/sensevoice-small-q8.gguf";
const ASR_MODEL_SIZE: u64 = 254_208_320;
const ASR_MODEL_SHA256: &str = "4ae45c94422de949b387e2e0fb10d7e14e4c42c69db30c3444ecc7d4b844b7c5";

pub fn open_target(target: impl AsRef<OsStr>, label: &str) -> Result<(), String> {
    HiddenCommand::new("cmd")
        .args(["/C", "start", ""])
        .arg(target.as_ref())
        .spawn()
        .map_err(|e| format!("系统打开失败({label}): {e}"))?;
    Ok(())
}

pub fn command_exists(command: &str) -> bool {
    let command_path = Path::new(command);
    if command_path.components().count() > 1 || command_path.extension().is_some() {
        return command_path.is_file();
    }

    let path = std::env::var_os("PATH").unwrap_or_default();
    let pathext = std::env::var_os("PATHEXT")
        .and_then(|v| v.into_string().ok())
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string());
    let mut extensions: Vec<String> = pathext
        .split(';')
        .filter_map(|ext| {
            let ext = ext.trim();
            if ext.is_empty() {
                None
            } else if ext.starts_with('.') {
                Some(ext.to_string())
            } else {
                Some(format!(".{ext}"))
            }
        })
        .collect();
    extensions.insert(0, String::new());

    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for ext in &extensions {
            if dir.join(format!("{command}{ext}")).is_file() {
                return true;
            }
        }
    }
    if let Some(path) = common_libreoffice_tool_path(command) {
        if let Some(dir) = path.parent() {
            ensure_dir_on_process_path(dir.to_path_buf());
        }
        return true;
    }
    false
}

pub fn pdf_tool_path(command: &str) -> std::path::PathBuf {
    ensure_bundled_poppler_on_process_path();
    windows_path::pdf_tool_path(command)
}

pub fn pandoc_tool_path() -> std::path::PathBuf {
    ensure_bundled_pandoc_on_process_path();
    windows_path::pandoc_tool_path()
}

pub fn libreoffice_tool_path() -> PathBuf {
    if let Ok(path) = std::env::var("PINVOU3_LIBREOFFICE_CMD") {
        if !path.trim().is_empty() {
            return PathBuf::from(path);
        }
    }
    if let Some(path) = common_libreoffice_tool_path("soffice") {
        if let Some(dir) = path.parent() {
            ensure_dir_on_process_path(dir.to_path_buf());
        }
        return path;
    }
    PathBuf::from("soffice")
}

pub fn ocr_tool_path() -> PathBuf {
    ensure_bundled_tesseract_on_process_path();
    windows_path::tesseract_tool_path()
}

pub fn ocr_tessdata_dir() -> Option<PathBuf> {
    windows_path::bundled_tessdata_dir()
}

pub fn asr_tool_path() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("PINVOU3_ASR_CMD") {
        if !path.trim().is_empty() {
            return std::path::PathBuf::from(path);
        }
    }
    if let Ok(path) = std::env::var("PINVOU3_DEEPSPEECH2_CMD") {
        if !path.trim().is_empty() {
            return std::path::PathBuf::from(path);
        }
    }
    if let Ok(path) = std::env::var("PADDLESPEECH_BIN") {
        if !path.trim().is_empty() {
            return std::path::PathBuf::from(path);
        }
    }
    ensure_bundled_asr_on_process_path();
    if let Some(path) = windows_path::bundled_asr_tool_path() {
        return path;
    }
    std::path::PathBuf::from("pinvou-asr")
}

pub fn asr_model_filename() -> &'static str {
    "sensevoice-small-q8.gguf"
}

pub fn asr_model_spec() -> crate::voice_asr::AsrModelSpec {
    crate::voice_asr::AsrModelSpec {
        id: "sensevoice-q8",
        filename: asr_model_filename(),
        expected_size: ASR_MODEL_SIZE,
        sha256: ASR_MODEL_SHA256,
        primary_url: ASR_MODEL_URL,
        mirror_url: ASR_MODEL_MIRROR_URL,
    }
}

pub fn asr_model_path() -> PathBuf {
    windows_path::asr_model_path()
}

pub fn asr_model_exists() -> bool {
    crate::voice_asr::model_available()
}

pub fn archive_tool_path() -> PathBuf {
    ensure_bundled_archive_on_process_path();
    windows_path::archive_tool_path()
}

pub fn pdf_tool_exists(command: &str) -> bool {
    ensure_bundled_poppler_on_process_path();
    windows_path::bundled_pdf_tool_path(command).is_some() || command_exists(command)
}

pub fn pandoc_tool_exists() -> bool {
    ensure_bundled_pandoc_on_process_path();
    windows_path::bundled_pandoc_tool_path().is_some() || command_exists("pandoc")
}

pub fn ocr_tool_exists() -> bool {
    ensure_bundled_tesseract_on_process_path();
    if windows_path::bundled_tesseract_dir().is_some() {
        return windows_path::bundled_tesseract_tool_path().is_some()
            && windows_path::bundled_tessdata_has_required_languages();
    }
    command_exists("tesseract")
}

pub fn asr_tool_exists() -> bool {
    if let Ok(path) = std::env::var("PINVOU3_ASR_CMD") {
        if !path.trim().is_empty() {
            return command_exists(&path);
        }
    }
    if let Ok(path) = std::env::var("PINVOU3_DEEPSPEECH2_CMD") {
        if !path.trim().is_empty() {
            return command_exists(&path);
        }
    }
    if let Ok(path) = std::env::var("PADDLESPEECH_BIN") {
        if !path.trim().is_empty() {
            return command_exists(&path);
        }
    }
    ensure_bundled_asr_on_process_path();
    windows_path::bundled_asr_tool_path().is_some()
        && windows_path::bundled_asr_backend_path().is_some()
}

pub fn asr_bundled_runtime_status() -> Option<bool> {
    Some(asr_tool_exists())
}

pub fn asr_dependency_installable() -> bool {
    asr_tool_exists()
}

pub fn asr_install_unavailable_message() -> &'static str {
    "本地语音识别运行时缺失，请修复或重新安装 pinvou。"
}

pub async fn install_asr_runtime(app: tauri::AppHandle) -> Result<(), String> {
    if !asr_tool_exists() {
        return Err(asr_install_unavailable_message().to_string());
    }
    if !crate::voice_asr::model_available() {
        crate::voice_asr::download_current_model(&app).await?;
    }
    Ok(())
}

pub fn archive_tool_exists() -> bool {
    ensure_bundled_archive_on_process_path();
    windows_path::bundled_archive_tool_path().is_some() || command_exists("7z")
}

pub fn msg_native_supported() -> bool {
    true
}

pub fn msg_converter_required() -> bool {
    false
}

pub fn email_tool_exists() -> bool {
    msg_native_supported()
}

pub fn show_pdf_dependency_check() -> bool {
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

pub fn pdf_dependency_packages() -> &'static str {
    ""
}

pub fn pandoc_dependency_packages() -> &'static str {
    ""
}

pub fn asr_dependency_packages() -> &'static str {
    "下载 SenseVoice q8 ASR 模型到用户目录"
}

pub fn archive_dependency_packages() -> &'static str {
    ""
}

pub fn email_dependency_packages() -> &'static str {
    ""
}

pub fn ocr_dependency_packages() -> &'static str {
    ""
}

pub fn asr_missing_message() -> &'static str {
    "本地语音识别组件缺失或不可用：运行时缺失时请修复或重新安装 pinvou；仅缺 ASR 模型时可在应用内下载。"
}

pub fn pandoc_missing_message() -> &'static str {
    "文档解析组件缺失或不可用：内置 Pandoc 未在安装目录 pandoc 下找到，请修复或重新安装 pinvou。"
}

pub fn pdf_text_missing_message() -> &'static str {
    "PDF 解析组件缺失或不可用：内置 Poppler 未在安装目录 poppler 下找到，请修复或重新安装 pinvou。"
}

pub fn pdf_render_missing_message() -> &'static str {
    "PDF 渲染组件缺失或不可用：内置 Poppler 未在安装目录 poppler 下找到，请修复或重新安装 pinvou。"
}

pub fn pdf_ocr_missing_message() -> &'static str {
    "扫描件 PDF OCR 需要 Tesseract；PDF 渲染组件由内置 Poppler 提供，如仍失败请修复或重新安装 pinvou。"
}

pub fn presentation_pdf_missing_message() -> &'static str {
    "演示文稿解析需要 LibreOffice；PDF 文本组件由内置 Poppler 提供，如缺失请修复或重新安装 pinvou。"
}

fn ensure_bundled_poppler_on_process_path() {
    let Some(dir) = windows_path::bundled_poppler_dir() else {
        return;
    };
    ensure_dir_on_process_path(dir);
}

fn ensure_bundled_pandoc_on_process_path() {
    let Some(dir) = windows_path::bundled_pandoc_dir() else {
        return;
    };
    ensure_dir_on_process_path(dir);
}

fn ensure_bundled_asr_on_process_path() {
    let Some(dir) = windows_path::bundled_asr_dir() else {
        return;
    };
    ensure_dir_on_process_path(dir);
}

fn ensure_bundled_tesseract_on_process_path() {
    let Some(dir) = windows_path::bundled_tesseract_dir() else {
        return;
    };
    ensure_dir_on_process_path(dir);
}

fn ensure_bundled_archive_on_process_path() {
    let Some(dir) = windows_path::bundled_archive_dir() else {
        return;
    };
    ensure_dir_on_process_path(dir);
}

fn ensure_dir_on_process_path(dir: std::path::PathBuf) {
    let current = std::env::var_os("PATH").unwrap_or_default();
    if std::env::split_paths(&current).any(|path| same_path(&path, &dir)) {
        return;
    }
    let mut paths = vec![dir];
    paths.extend(std::env::split_paths(&current));
    if let Ok(joined) = std::env::join_paths(paths) {
        std::env::set_var("PATH", joined);
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    left.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
}

fn common_libreoffice_tool_path(command: &str) -> Option<PathBuf> {
    if !is_libreoffice_command(command) {
        return None;
    }
    let mut roots = Vec::new();
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        roots.push(PathBuf::from(program_files));
    }
    if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
        roots.push(PathBuf::from(program_files_x86));
    }
    roots.push(PathBuf::from(r"C:\Program Files"));
    roots.push(PathBuf::from(r"C:\Program Files (x86)"));

    roots.into_iter().find_map(|root| {
        let program = root.join("LibreOffice").join("program");
        [program.join("soffice.com"), program.join("soffice.exe")]
            .into_iter()
            .find(|path| path.is_file())
    })
}

fn is_libreoffice_command(command: &str) -> bool {
    let name = Path::new(command)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(command)
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "soffice" | "soffice.exe" | "soffice.com" | "libreoffice" | "libreoffice.exe"
    )
}

pub fn nvidia_smi_candidates() -> Vec<&'static str> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_hides_archive_dependency_check() {
        assert!(!show_archive_dependency_check());
        assert_eq!(archive_dependency_packages(), "");
    }

    #[test]
    fn detects_libreoffice_command_names() {
        assert!(is_libreoffice_command("soffice"));
        assert!(is_libreoffice_command("soffice.exe"));
        assert!(is_libreoffice_command("soffice.com"));
        assert!(is_libreoffice_command("libreoffice"));
        assert!(!is_libreoffice_command("pandoc"));
    }

    #[test]
    fn libreoffice_tool_path_returns_program() {
        assert!(!libreoffice_tool_path().as_os_str().is_empty());
    }
}
