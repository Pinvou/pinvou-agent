use std::path::{Path, PathBuf};

use super::super::voice_asr::{self, AsrModelSpec};
use super::voice_asr_speech;

const ASR_MODEL_URL: &str =
    "https://www.modelscope.cn/models/lovemefan/SenseVoiceGGUF/resolve/master/sense-voice-small-q4_k.gguf";
const ASR_MODEL_MIRROR_URL: &str =
    "https://huggingface.co/lovemefan/sense-voice-gguf/resolve/main/sense-voice-small-q4_k.gguf";
const ASR_MODEL_SIZE: u64 = 182_278_688;
const ASR_MODEL_SHA256: &str = "c8e7bf77acd860c5b83d2106da44aa7b985026ef4e7dbf5236c7f0f4001d9e9b";
pub fn engine_binary_name() -> &'static str {
    // macOS 不再打包引擎；该名称仅保留给显式配置的兼容 CLI 路径。
    "pinvou-asr"
}

/// 平台 ASR 引擎完整性契约:macOS 二期未打包 ASR 引擎,恒为完整。
/// 该函数与 linux/windows 同名实现构成跨平台接口,仅被 voice_asr 内部调用;
/// macOS 用 Speech 框架不走该路径,故在此目标下为间接死代码。
#[allow(dead_code)]
pub fn bundled_engine_intact(_path: &Path, _bundled_dir: Option<&Path>) -> bool {
    // macOS 二期没有打包 ASR 引擎。
    true
}

fn explicit_asr_tool_path() -> Option<PathBuf> {
    for name in [
        "PINVOU3_ASR_CMD",
        "PINVOU3_DEEPSPEECH2_CMD",
        "PADDLESPEECH_BIN",
    ] {
        if let Ok(path) = std::env::var(name) {
            if !path.trim().is_empty() {
                return Some(PathBuf::from(path));
            }
        }
    }
    None
}

pub fn asr_tool_path() -> PathBuf {
    explicit_asr_tool_path().unwrap_or_else(voice_asr::engine_path)
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
    // 系统 Speech 不需要应用侧模型。这里返回 true 是为了让平台中立的
    // VoiceAsrStatus 不再把旧 SenseVoice 模型当成 macOS 语音输入前置条件。
    true
}

pub fn asr_tool_exists() -> bool {
    // 项目最低支持 macOS 11，系统 Speech framework 是平台能力，不是需要安装的
    // 应用依赖。`SFSpeechRecognizer::isAvailable` 表示指定 locale 的瞬时服务状态，
    // 不能拿系统默认 locale 的结果充当录音前置门禁；否则默认语言不可用时会误拦截
    // 另一个仍可用的 UI 语言。授权、locale 与在线服务状态在实际识别时检查。
    true
}

pub fn asr_bundled_runtime_status() -> Option<bool> {
    // Some(true) 表示系统 Speech framework 无需安装；公共状态机不再继续检查
    // SenseVoice 引擎和 ffmpeg。瞬时服务状态由真正的识别请求报告。
    Some(asr_tool_exists())
}

pub fn asr_dependency_installable() -> bool {
    // 系统 Speech 不属于应用可安装依赖。不可用时应引导检查系统权限/服务，
    // 不能展示一个实际无动作的“安装”按钮。
    false
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
/// macOS 不走内置 SenseVoice：系统 Speech 框架无需打包额外引擎、跨架构可用。
/// 返回 `Some` 表示 macOS 始终走系统 Speech（失败时由调用方回退 CLI）。
///
/// `locale_tag` 决定识别语言（跟随 UI 语言偏好，而非系统默认 locale——
/// 后者可能是 en-US，把中文音频当英文解析出无意义字母）。
pub fn recognize_native(
    wav_path: &std::path::Path,
    locale_tag: &str,
) -> Option<Result<String, String>> {
    Some(voice_asr_speech::transcribe_with_speech(
        wav_path, locale_tag,
    ))
}

/// 原生识别后端的来源标签（用于前端展示/日志区分）。
pub fn native_recognition_source() -> &'static str {
    "system_speech"
}

pub async fn reset_microphone_permission(_window: tauri::WebviewWindow) -> Result<bool, String> {
    Ok(false)
}
