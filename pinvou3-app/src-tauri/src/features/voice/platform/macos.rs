use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::super::voice_asr::{self, AsrModelSpec};

const ASR_MODEL_URL: &str =
    "https://www.modelscope.cn/models/lovemefan/SenseVoiceGGUF/resolve/master/sense-voice-small-q4_k.gguf";
const ASR_MODEL_MIRROR_URL: &str =
    "https://huggingface.co/lovemefan/sense-voice-gguf/resolve/main/sense-voice-small-q4_k.gguf";
const ASR_MODEL_SIZE: u64 = 182_278_688;
const ASR_MODEL_SHA256: &str =
    "c8e7bf77acd860c5b83d2106da44aa7b985026ef4e7dbf5236c7f0f4001d9e9b";
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
    let path = asr_tool_path();
    path.is_file()
        && std::fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

pub fn asr_bundled_runtime_status() -> Option<bool> {
    None
}

pub fn asr_dependency_installable() -> bool {
    asr_tool_exists()
}

pub fn asr_install_unavailable_message() -> &'static str {
    "本地语音识别引擎未随应用提供，请重新安装 pinvou3。"
}

pub async fn install_asr_runtime(app: tauri::AppHandle) -> Result<(), String> {
    if !asr_tool_exists() {
        return Err(asr_install_unavailable_message().to_string());
    }
    if !voice_asr::ffmpeg_available() {
        crate::features::dependencies::install_dependencies(vec!["ffmpeg".to_string()])?;
    }
    if !voice_asr::model_available() {
        voice_asr::download_current_model(&app).await?;
    }
    Ok(())
}

pub fn asr_dependency_packages() -> &'static str {
    "ffmpeg"
}

pub fn asr_missing_message() -> &'static str {
    "缺少本地语音识别组件，请重新安装应用或在设置页补齐模型。"
}

pub async fn reset_microphone_permission(_window: tauri::WebviewWindow) -> Result<bool, String> {
    Ok(false)
}
