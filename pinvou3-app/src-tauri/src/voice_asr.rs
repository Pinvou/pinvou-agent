//! 本地语音识别（SenseVoice.cpp）接入：引擎/模型路径、依赖检测、音频转码 +
//! 识别 + 输出清洗，以及模型按需下载。把 POC 阶段的 python shim 逻辑用 Rust 固化，
//! 不再依赖外部脚本/环境变量。
//!
//! - 引擎 `sense-voice-main`（~420KB）打进 deb，安装即有（resource_dir）；开发态
//!   或手动搭建时落在 `~/.pinvou3/asr/`。
//! - 模型 `sense-voice-small-q4_k.gguf`（174MB）首次用语音时按需下载到 `~/.pinvou3/asr/`。
//! - ffmpeg 优先用系统已有，缺了由前端引导走 pkexec apt 安装。

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::bridge::paths;

const MODEL_FILE: &str = "sense-voice-small-q4_k.gguf";
const MODEL_URL: &str =
    "https://www.modelscope.cn/models/lovemefan/SenseVoiceGGUF/resolve/master/sense-voice-small-q4_k.gguf";
/// modelscope 上 q4_k 的精确字节数（下载完整性校验，避免半包）。
const MODEL_SIZE: u64 = 182_278_688;

/// `~/.pinvou3/asr/` —— 引擎、模型、下载缓存的落地目录。
pub fn asr_dir() -> PathBuf {
    paths::pinvou3_home().join("asr")
}

/// 引擎二进制名：各平台不同（Linux 是 sense-voice-main，Mac 是
/// sense-voice-darwin-arm64，Windows 是 pinvou-asr.exe）。Mac 二进制由
/// Phase 3 Task 3.1 编译，在此之前 engine_path() 返回的路径不会 is_file，
/// 前端会显示 ASR 不可用 —— 这是预期行为。
#[cfg(target_os = "linux")]
pub fn engine_binary_name() -> &'static str {
    "sense-voice-main"
}

/// 注意:PR #212 仅打包 arm64 (Apple Silicon) 二进制。Intel Mac (x86_64) 无对应
/// 入库引擎,engine_path() 返回的路径不会 is_file → ASR 不可用(前端显示"不可用",
/// 不会崩溃)。如需 Intel Mac 支持需另行编译 sense-voice-darwin-x86_64 入库。
#[cfg(target_os = "macos")]
pub fn engine_binary_name() -> &'static str {
    "sense-voice-darwin-arm64"
}

#[cfg(target_os = "windows")]
pub fn engine_binary_name() -> &'static str {
    "pinvou-asr.exe"
}

/// 引擎可执行：优先 `~/.pinvou3/asr/`（按需/手动装的），回退打包资源目录。
/// 打包资源目录由 [`set_bundled_engine_dir`] 在启动时注入（需要 AppHandle）。
pub fn engine_path() -> PathBuf {
    let name = engine_binary_name();
    let local = asr_dir().join(name);
    if local.is_file() {
        return local;
    }
    if let Some(dir) = bundled_engine_dir() {
        let bundled = dir.join(name);
        if bundled.is_file() {
            return bundled;
        }
    }
    local
}

pub fn model_path() -> PathBuf {
    asr_dir().join(MODEL_FILE)
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
    /// 还差哪些（前端展示 + 估算下载体积）。
    pub missing: Vec<String>,
}

pub fn status() -> VoiceAsrStatus {
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

/// 下载模型到 `~/.pinvou3/asr/`，流式写盘 + 进度事件 `voice_asr:progress`。
/// 已存在且大小正确则跳过。
pub async fn download_model(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri::Emitter;
    let dir = asr_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {e}"))?;
    let dest = model_path();
    if dest.metadata().map(|m| m.len() == MODEL_SIZE).unwrap_or(false) {
        return Ok(());
    }

    let url = std::env::var("PINVOU3_ASR_MODEL_URL").unwrap_or_else(|_| MODEL_URL.to_string());
    // modelscope CDN 拒绝空 User-Agent（reqwest 默认不发 UA）→ 403；设一个非空 UA。
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .user_agent("pinvou3-asr/1.0")
        .build()
        .map_err(|e| format!("HTTP client 构建失败: {e}"))?;
    let mut resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("连接模型源失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("模型源响应异常: {e}"))?;

    let total = resp.content_length().unwrap_or(MODEL_SIZE);
    let tmp = dir.join(format!("{MODEL_FILE}.part"));
    let mut file = std::fs::File::create(&tmp).map_err(|e| format!("创建文件失败: {e}"))?;
    let mut downloaded: u64 = 0;
    let mut last_emit: u64 = 0;
    use std::io::Write;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("下载中断: {e}"))?
    {
        file.write_all(&chunk).map_err(|e| format!("写盘失败: {e}"))?;
        downloaded += chunk.len() as u64;
        if downloaded - last_emit >= 1_048_576 || downloaded == total {
            last_emit = downloaded;
            let _ = app.emit(
                "voice_asr:progress",
                serde_json::json!({ "stage": "model", "downloaded": downloaded, "total": total }),
            );
        }
    }
    drop(file);
    std::fs::rename(&tmp, &dest).map_err(|e| format!("保存模型失败: {e}"))?;
    Ok(())
}

/// 前端查询本地语音识别各组件就绪状态。
#[tauri::command]
pub fn voice_asr_status() -> VoiceAsrStatus {
    status()
}

/// 一键安装本地语音识别依赖：缺 ffmpeg 走 pkexec apt，缺模型则下载（带进度）。
/// 进度走 `voice_asr:progress` 事件，完成返回最新状态。
#[tauri::command]
pub async fn install_voice_asr(app: tauri::AppHandle) -> Result<VoiceAsrStatus, String> {
    use tauri::Emitter;

    // 1. ffmpeg：系统没有才装（pkexec apt，弹系统授权框，不依赖超级权限开关）
    if !ffmpeg_available() {
        let _ = app.emit(
            "voice_asr:progress",
            serde_json::json!({ "stage": "ffmpeg", "downloaded": 0, "total": 0 }),
        );
        tokio::task::spawn_blocking(|| crate::os::install_dependencies(vec!["ffmpeg".to_string()]))
            .await
            .map_err(|e| format!("ffmpeg 安装任务失败: {e}"))??;
    }

    // 2. 模型：缺则按需下载
    if !model_path().is_file() {
        download_model(&app).await?;
    }

    let st = status();
    let _ = app.emit(
        "voice_asr:progress",
        serde_json::json!({ "stage": "done", "ready": st.ready }),
    );
    Ok(st)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_binary_name_matches_platform() {
        let name = engine_binary_name();
        #[cfg(target_os = "linux")]
        assert_eq!(name, "sense-voice-main");
        #[cfg(target_os = "macos")]
        assert_eq!(name, "sense-voice-darwin-arm64");
        #[cfg(target_os = "windows")]
        assert_eq!(name, "pinvou-asr.exe");
    }
}
