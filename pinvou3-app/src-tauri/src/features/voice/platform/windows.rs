use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::super::voice_asr::{self, AsrModelSpec};

pub fn engine_binary_name() -> &'static str {
    "pinvou-asr.exe"
}

pub fn bundled_engine_intact(
    _path: &std::path::Path,
    _bundled_dir: Option<&std::path::Path>,
) -> bool {
    true
}

const ASR_MODEL_URL: &str =
    "https://www.modelscope.cn/models/FunAudioLLM/SenseVoiceSmall-GGUF/resolve/master/sensevoice-small-q8.gguf";
const ASR_MODEL_MIRROR_URL: &str =
    "https://huggingface.co/FunAudioLLM/SenseVoiceSmall-GGUF/resolve/main/sensevoice-small-q8.gguf";
const ASR_MODEL_SIZE: u64 = 254_208_320;
const ASR_MODEL_SHA256: &str = "4ae45c94422de949b387e2e0fb10d7e14e4c42c69db30c3444ecc7d4b844b7c5";

pub fn asr_tool_path() -> PathBuf {
    for name in [
        "PINVOU3_ASR_CMD",
        "PINVOU3_DEEPSPEECH2_CMD",
        "PADDLESPEECH_BIN",
    ] {
        if let Ok(path) = std::env::var(name) {
            if !path.trim().is_empty() {
                return PathBuf::from(path);
            }
        }
    }
    crate::platform::os::windows::bundled_asr_tool_path()
        .unwrap_or_else(|| PathBuf::from("pinvou-asr"))
}

pub fn asr_model_spec() -> AsrModelSpec {
    AsrModelSpec {
        id: "sensevoice-q8",
        filename: "sensevoice-small-q8.gguf",
        expected_size: ASR_MODEL_SIZE,
        sha256: ASR_MODEL_SHA256,
        primary_url: ASR_MODEL_URL,
        mirror_url: ASR_MODEL_MIRROR_URL,
    }
}

pub fn asr_model_path() -> PathBuf {
    crate::platform::os::windows::asr_model_path()
}

pub fn asr_model_exists() -> bool {
    voice_asr::model_available()
}

pub fn asr_tool_exists() -> bool {
    if let Ok(path) = std::env::var("PINVOU3_ASR_CMD") {
        if !path.trim().is_empty() {
            return crate::platform::os::command_exists(&path);
        }
    }
    if let Ok(path) = std::env::var("PINVOU3_DEEPSPEECH2_CMD") {
        if !path.trim().is_empty() {
            return crate::platform::os::command_exists(&path);
        }
    }
    if let Ok(path) = std::env::var("PADDLESPEECH_BIN") {
        if !path.trim().is_empty() {
            return crate::platform::os::command_exists(&path);
        }
    }
    crate::platform::os::windows::bundled_asr_tool_path().is_some()
        && crate::platform::os::windows::bundled_asr_backend_path().is_some()
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
    if !voice_asr::model_available() {
        voice_asr::download_current_model(&app).await?;
    }
    Ok(())
}

pub fn asr_dependency_packages() -> &'static str {
    "下载 SenseVoice q8 ASR 模型到用户目录"
}

pub fn asr_missing_message() -> &'static str {
    "本地语音识别组件缺失或不可用：运行时缺失时请修复或重新安装 pinvou；仅缺 ASR 模型时可在应用内下载。"
}

/// Windows 的打包运行时是 CLI 形态，保持原有路径交给调用方启动。
///
/// 返回 `None` 可避免把 `pinvou-asr.exe` 误送进只支持 SenseVoice.cpp 参数协议的
/// Rust `voice_asr::transcribe`。
pub fn recognize_native(
    _wav_path: &std::path::Path,
    _locale_tag: &str,
) -> Option<Result<String, String>> {
    None
}

/// 原生识别后端的来源标签（用于前端展示/日志区分）。
pub fn native_recognition_source() -> &'static str {
    "pinvou-webview-sensevoice-local"
}

pub async fn reset_microphone_permission(window: tauri::WebviewWindow) -> Result<bool, String> {
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::{
            ICoreWebView2Profile4, ICoreWebView2_13, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
            COREWEBVIEW2_PERMISSION_STATE_DEFAULT,
        },
        SetPermissionStateCompletedHandler,
    };
    use windows_core::{Interface, HSTRING};

    let origin = window
        .url()
        .map_err(|error| format!("读取当前页面地址失败：{error}"))?
        .origin()
        .ascii_serialization();
    if origin == "null" {
        return Err("当前页面没有可重置的 WebView2 权限来源".to_string());
    }

    let (sender, receiver) = tokio::sync::oneshot::channel::<Result<(), String>>();
    let sender = Arc::new(Mutex::new(Some(sender)));
    window
        .with_webview(move |webview| {
            let callback_sender = Arc::clone(&sender);
            let schedule_result: windows_core::Result<()> = (|| unsafe {
                let webview13 = webview
                    .controller()
                    .CoreWebView2()?
                    .cast::<ICoreWebView2_13>()?;
                let profile = webview13.Profile()?.cast::<ICoreWebView2Profile4>()?;
                let callback = SetPermissionStateCompletedHandler::create(Box::new(
                    move |completion_result| {
                        if let Some(sender) = callback_sender
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .take()
                        {
                            let _ =
                                sender.send(completion_result.map_err(|error| error.to_string()));
                        }
                        Ok(())
                    },
                ));
                profile.SetPermissionState(
                    COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
                    &HSTRING::from(origin),
                    COREWEBVIEW2_PERMISSION_STATE_DEFAULT,
                    &callback,
                )
            })();

            if let Err(error) = schedule_result {
                if let Some(sender) = sender
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .take()
                {
                    let _ = sender.send(Err(error.to_string()));
                }
            }
        })
        .map_err(|error| format!("访问 Windows WebView2 失败：{error}"))?;

    tokio::time::timeout(Duration::from_secs(3), receiver)
        .await
        .map_err(|_| "重置麦克风权限超时".to_string())?
        .map_err(|_| "麦克风权限重置任务被取消".to_string())??;
    Ok(true)
}
