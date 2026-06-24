use crate::process::HiddenCommand;
use std::ffi::OsStr;
use std::path::Path;

use super::windows_path;

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

    let path = match std::env::var_os("PATH") {
        Some(path) => path,
        None => return false,
    };
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

pub fn pdf_tool_exists(command: &str) -> bool {
    ensure_bundled_poppler_on_process_path();
    windows_path::bundled_pdf_tool_path(command).is_some() || command_exists(command)
}

pub fn pandoc_tool_exists() -> bool {
    ensure_bundled_pandoc_on_process_path();
    windows_path::bundled_pandoc_tool_path().is_some() || command_exists("pandoc")
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
    windows_path::bundled_asr_tool_path().is_some() || command_exists("pinvou-asr")
}

pub fn show_pdf_dependency_check() -> bool {
    false
}

pub fn show_pandoc_dependency_check() -> bool {
    false
}

pub fn pdf_dependency_packages() -> &'static str {
    ""
}

pub fn pandoc_dependency_packages() -> &'static str {
    ""
}

pub fn asr_dependency_packages() -> &'static str {
    "安装 pinvou3-asr-windows-x64 离线语音包到安装目录 asr 文件夹"
}

pub fn ocr_dependency_packages() -> &'static str {
    "tesseract-ocr tesseract-ocr-chi-sim"
}

pub fn asr_missing_message() -> &'static str {
    "本地语音识别组件缺失或不可用：请安装 pinvou3-asr-windows-x64 SenseVoice 离线语音包到安装目录 asr 文件夹，或通过 PINVOU3_ASR_CMD 指向 pinvou-asr.exe。"
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

pub fn nvidia_smi_candidates() -> Vec<&'static str> {
    vec![
        "nvidia-smi",
        r"C:\Windows\System32\nvidia-smi.exe",
        r"C:\Program Files\NVIDIA Corporation\NVSMI\nvidia-smi.exe",
    ]
}
