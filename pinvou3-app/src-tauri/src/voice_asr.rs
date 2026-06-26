//! Shared local ASR orchestration.
//!
//! Runtime layout, dependency installation, and model download policy are owned by `crate::os`.
//! This module keeps platform-neutral status, transcription, and Tauri command glue.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::bridge::paths;

/// `~/.pinvou3/asr/` —— 引擎、模型、下载缓存的落地目录。
pub fn asr_dir() -> PathBuf {
    paths::pinvou3_home().join("asr")
}

/// 引擎可执行：优先 `~/.pinvou3/asr/`（按需/手动装的），回退打包资源目录。
/// 打包资源目录由 [`set_bundled_engine_dir`] 在启动时注入（需要 AppHandle）。
pub fn engine_path() -> PathBuf {
    let local = asr_dir().join("sense-voice-main");
    if local.is_file() {
        return local;
    }
    if let Some(dir) = bundled_engine_dir() {
        let bundled = dir.join("sense-voice-main");
        if bundled.is_file() {
            return bundled;
        }
    }
    local
}

pub fn model_path() -> PathBuf {
    asr_dir().join(crate::os::asr_model_filename())
}

// 打包引擎目录：启动时从 resource_dir 解析后存这里，供 engine_path 回退使用。
use std::sync::OnceLock;
static BUNDLED_ENGINE_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_bundled_engine_dir(dir: PathBuf) {
    let _ = BUNDLED_ENGINE_DIR.set(dir);
}

fn bundled_engine_dir() -> Option<PathBuf> {
    BUNDLED_ENGINE_DIR.get().cloned()
}

pub fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 各组件就绪状态，前端据此决定是否弹「安装依赖」框。
#[derive(Debug, Clone, Serialize)]
pub struct VoiceAsrStatus {
    pub engine: bool,
    pub ffmpeg: bool,
    pub model: bool,
    /// 三者齐全 = 可直接用语音。
    pub ready: bool,
    /// Whether the frontend may offer the built-in installer flow. Windows ships ASR in the MSI,
    /// so missing runtime there means repair/reinstall instead of Linux dependency installation.
    pub installable: bool,
    /// 还差哪些（前端展示 + 估算下载体积）。
    pub missing: Vec<String>,
}

pub fn status() -> VoiceAsrStatus {
    if let Some(runtime) = crate::os::asr_bundled_runtime_status() {
        return VoiceAsrStatus {
            engine: runtime,
            ffmpeg: true,
            model: runtime,
            ready: runtime,
            installable: crate::os::asr_dependency_installable(),
            missing: if runtime {
                Vec::new()
            } else {
                vec!["runtime".to_string()]
            },
        };
    }

    let engine = engine_path().is_file();
    let ffmpeg = ffmpeg_available();
    let model = model_path().is_file();
    let mut missing = Vec::new();
    if !model {
        missing.push("model".to_string());
    }
    if !ffmpeg {
        missing.push("ffmpeg".to_string());
    }
    if !engine {
        missing.push("engine".to_string());
    }
    VoiceAsrStatus {
        engine,
        ffmpeg,
        model,
        ready: engine && ffmpeg && model,
        installable: crate::os::asr_dependency_installable(),
        missing,
    }
}

/// 转码到 16k 单声道 → 调 sense-voice-main → 清洗输出。供 transcribe_voice_audio 调用。
pub fn transcribe(wav: &Path) -> Result<String, String> {
    let engine = engine_path();
    if !engine.is_file() {
        return Err("本地语音识别引擎未安装".to_string());
    }
    let model = model_path();
    if !model.is_file() {
        return Err("本地语音识别模型未下载".to_string());
    }

    // 浏览器录音多为 48k/立体声，sense-voice 只吃 16k mono，先转码。
    let norm = std::env::temp_dir().join(format!("pinvou3-asr-{}.wav", std::process::id()));
    let input = if ffmpeg_available() {
        let ff = Command::new("ffmpeg")
            .args(["-y", "-i"])
            .arg(wav)
            .args(["-ar", "16000", "-ac", "1", "-f", "wav"])
            .arg(&norm)
            .output();
        match ff {
            Ok(o) if o.status.success() && norm.metadata().map(|m| m.len() > 44).unwrap_or(false) => {
                norm.clone()
            }
            _ => wav.to_path_buf(),
        }
    } else {
        wav.to_path_buf()
    };

    // 引擎运行时会把 fbank_lfr_cmvn_feature.json(~290KB)写进当前工作目录。
    // 默认 CWD 在 dev 下是 src-tauri/——会被 tauri dev 的文件监视器当成源码改动而
    // 重编/重启 app(表现为「识别完 app 崩溃」),在 deb 安装态还可能是只读目录。
    // 钉死 CWD 到可写的 asr_dir,让这个副产物落在那里、不污染源码树。
    let work_dir = asr_dir();
    let _ = std::fs::create_dir_all(&work_dir);
    let out = Command::new(&engine)
        .current_dir(&work_dir)
        .arg("-m")
        .arg(&model)
        .arg(&input)
        .args(["-t", "4", "-l", "auto", "-itn"])
        .output();
    let _ = std::fs::remove_file(&norm);
    let out = out.map_err(|e| format!("启动语音识别引擎失败: {e}"))?;
    if !out.status.success() {
        let tail = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "语音识别引擎失败: {}",
            tail.lines().last().unwrap_or("").trim()
        ));
    }
    let text = clean_engine_output(&String::from_utf8_lossy(&out.stdout));
    if text.is_empty() {
        return Err("未识别到语音内容".to_string());
    }
    Ok(text)
}

/// 剥 `[start-end]` 时间戳前缀，拼接多段，再去掉 `<|zh|><|NEUTRAL|>` 等控制标记。
fn clean_engine_output(stdout: &str) -> String {
    let mut parts = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        // 形如 "[1.22-1.86] 文字"
        if let Some(rest) = line.strip_prefix('[') {
            if let Some(idx) = rest.find(']') {
                let text = rest[idx + 1..].trim();
                if !text.is_empty() {
                    parts.push(text.to_string());
                }
            }
        }
    }
    strip_control_markers(&parts.join(""))
}

/// 去掉所有 `<|...|>` 控制标记（SenseVoice 偶发泄漏的语言/情感/事件标记）。
fn strip_control_markers(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' && chars.peek() == Some(&'|') {
            chars.next(); // 吃掉 '|'
            // 跳到 "|>"
            while let Some(n) = chars.next() {
                if n == '|' && chars.peek() == Some(&'>') {
                    chars.next();
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// 前端查询本地语音识别各组件就绪状态。
#[tauri::command]
pub fn voice_asr_status() -> VoiceAsrStatus {
    status()
}

/// 一键安装本地语音识别依赖：缺 ffmpeg 走 pkexec apt，缺模型则下载（带进度）。
/// Install local ASR runtime through the current platform implementation.
#[tauri::command]
pub async fn install_voice_asr(app: tauri::AppHandle) -> Result<VoiceAsrStatus, String> {
    if !crate::os::asr_dependency_installable() {
        let _ = app;
        return Err(crate::os::asr_install_unavailable_message().to_string());
    }

    use tauri::Emitter;
    crate::os::install_asr_runtime(app.clone()).await?;

    let st = status();
    let _ = app.emit(
        "voice_asr:progress",
        serde_json::json!({ "stage": "done", "ready": st.ready }),
    );
    Ok(st)
}
