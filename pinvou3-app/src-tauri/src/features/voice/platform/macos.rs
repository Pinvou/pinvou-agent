use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::super::voice_asr::{self, AsrModelSpec};
use super::voice_asr_speech;

const ASR_MODEL_URL: &str =
    "https://www.modelscope.cn/models/lovemefan/SenseVoiceGGUF/resolve/master/sense-voice-small-q4_k.gguf";
const ASR_MODEL_MIRROR_URL: &str =
    "https://huggingface.co/lovemefan/sense-voice-gguf/resolve/main/sense-voice-small-q4_k.gguf";
const ASR_MODEL_SIZE: u64 = 182_278_688;
const ASR_MODEL_SHA256: &str = "c8e7bf77acd860c5b83d2106da44aa7b985026ef4e7dbf5236c7f0f4001d9e9b";
const BUNDLED_ENGINE_SHA256: &str =
    "7cc7fc5c31d67b82df36d605c55db1abd685daa73180066afdc1b9d3324bd1b4";

pub fn engine_binary_name() -> &'static str {
    "sense-voice-darwin-arm64"
}

pub fn bundled_engine_intact(path: &Path, bundled_dir: Option<&Path>) -> bool {
    let Some(dir) = bundled_dir else {
        return true;
    };
    if !path.starts_with(dir) {
        return true;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 65_536];
    loop {
        let count = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(_) => return false,
        };
        hasher.update(&buffer[..count]);
    }
    format!("{:x}", hasher.finalize()) == BUNDLED_ENGINE_SHA256
}

pub fn asr_tool_path() -> PathBuf {
    std::env::var("PINVOU3_ASR_CMD")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(voice_asr::engine_path)
}

pub fn asr_model_spec() -> AsrModelSpec {
    AsrModelSpec {
        id: "sensevoice-q4-k",
        filename: "sense-voice-small-q4_k.gguf",
        expected_size: ASR_MODEL_SIZE,
        sha256: ASR_MODEL_SHA256,
        primary_url: ASR_MODEL_URL,
        mirror_url: ASR_MODEL_MIRROR_URL,
    }
}

pub fn asr_model_path() -> PathBuf {
    voice_asr::model_download_path()
}

pub fn asr_model_exists() -> bool {
    voice_asr::model_available()
}

pub fn asr_tool_exists() -> bool {
    // macOS 二期改走系统 Speech 框架（voice_asr_speech），不再依赖 SenseVoice 引擎。
    // 体检项用 speech_available() 判断系统 Speech 服务是否就绪，而非查引擎文件。
    speech_available()
}

/// 探测系统 Speech 识别器是否可用（默认 locale）。
///
/// 委托给 `voice_asr_speech::speech_available`——创建默认 locale 的 recognizer 并读
/// `isAvailable`（recognizer 创建成功但服务临时不可用时 isAvailable=false）。**不在此处
/// 阻塞请求授权**：首次语音输入时由 Tauri 命令上下文触发。
pub fn speech_available() -> bool {
    voice_asr_speech::speech_available()
}

pub fn asr_bundled_runtime_status() -> Option<bool> {
    None
}

pub fn asr_dependency_installable() -> bool {
    // macOS 走系统 Speech，无需安装额外依赖。
    speech_available()
}

pub fn asr_install_unavailable_message() -> &'static str {
    "macOS 使用系统语音识别框架，无需安装额外组件。如不可用，请检查「系统设置 > 隐私与安全性 > 语音识别」是否已授权。"
}

pub async fn install_asr_runtime(_app: tauri::AppHandle) -> Result<(), String> {
    // macOS 走系统 Speech 框架，无需安装引擎/模型/ffmpeg。
    Ok(())
}

pub fn asr_dependency_packages() -> &'static str {
    // macOS 系统内置 Speech 框架，无外部依赖包。
    ""
}

pub fn asr_missing_message() -> &'static str {
    "macOS 使用系统语音识别框架。如不可用，请检查系统设置中的语音识别权限。"
}

/// 用系统 Speech 框架识别临时 wav 文件。
///
/// macOS 不走内置 SenseVoice：系统 Speech 框架零体积、跨架构、开箱即用。
/// 返回 `Some` 表示 macOS 始终走系统 Speech（失败时由调用方回退 CLI）。
///
/// `locale_tag` 决定识别语言（跟随 UI 语言偏好，而非系统默认 locale——
/// 后者可能是 en-US，把中文音频当英文解析出无意义字母）。
pub fn recognize_native(
    wav_path: &std::path::Path,
    locale_tag: &str,
) -> Option<Result<String, String>> {
    Some(voice_asr_speech::transcribe_with_speech(wav_path, locale_tag))
}

/// 原生识别后端的来源标签（用于前端展示/日志区分）。
pub fn native_recognition_source() -> &'static str {
    "system_speech"
}

pub async fn reset_microphone_permission(_window: tauri::WebviewWindow) -> Result<bool, String> {
    Ok(false)
}
