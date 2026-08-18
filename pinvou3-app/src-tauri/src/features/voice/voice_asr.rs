//! Shared local ASR orchestration.
//!
//! Platform runtime policy is owned by the sibling `platform` module. This module keeps
//! platform-neutral status, transcription, model download, and Tauri command glue.
//!
//! 其中 model_available、download_current_model、transcribe、
//! model_file_verified 等函数仅被 `platform/{windows,linux}.rs` 调用;macOS 用系统
//! Speech 框架(见 `platform/voice_asr_speech.rs`),不引用这些函数。因此它们在 macOS
//! 编译目标下被 clippy 误报为 dead code,但删除会破坏 Windows/Linux 构建。
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use tauri::Emitter;

use crate::platform::paths;

static ASR_INSTALLING: AtomicBool = AtomicBool::new(false);
static ASR_CANCEL: AtomicBool = AtomicBool::new(false);
static MODEL_VERIFICATION_CACHE: OnceLock<Mutex<Option<ModelVerificationCache>>> = OnceLock::new();

#[derive(Debug, Clone)]
struct ModelVerificationCache {
    path: PathBuf,
    len: u64,
    modified: Option<std::time::SystemTime>,
    expected_size: u64,
    sha256: String,
    verified: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct AsrModelSpec {
    pub id: &'static str,
    pub filename: &'static str,
    pub expected_size: u64,
    pub sha256: &'static str,
    pub primary_url: &'static str,
    pub mirror_url: &'static str,
}

/// `~/.pinvou3/asr/` —— 引擎、模型、下载缓存的落地目录。
pub fn asr_dir() -> PathBuf {
    paths::pinvou3_home().join("asr")
}

pub fn current_model_spec() -> AsrModelSpec {
    super::platform::asr_model_spec()
}

/// 引擎可执行：优先 `~/.pinvou3/asr/`（按需/手动装的），回退打包资源目录。
/// 打包资源目录由 [`set_bundled_engine_dir`] 在启动时注入（需要 AppHandle）。
pub fn engine_path() -> PathBuf {
    let name = super::platform::engine_binary_name();
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

/// 当前可用模型路径：Windows 优先用户目录，兼容旧内置模型；Linux 为用户目录。
pub fn model_path() -> PathBuf {
    super::platform::asr_model_path()
}

/// 按需下载的目标路径，始终落在用户目录，避免写安装目录。
pub fn model_download_path() -> PathBuf {
    asr_dir().join(current_model_spec().filename)
}

pub fn model_available() -> bool {
    model_file_verified(&model_path(), &current_model_spec())
}

// 打包引擎目录：启动时从 resource_dir 解析后存这里，供 engine_path 回退使用。
static BUNDLED_ENGINE_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_bundled_engine_dir(dir: PathBuf) {
    let _ = BUNDLED_ENGINE_DIR.set(dir);
}

fn bundled_engine_dir() -> Option<PathBuf> {
    BUNDLED_ENGINE_DIR.get().cloned()
}

fn bundled_engine_intact(path: &Path) -> bool {
    let bundled_dir = bundled_engine_dir();
    super::platform::bundled_engine_intact(path, bundled_dir.as_deref())
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
    let model = super::platform::asr_model_exists();
    if let Some(runtime) = super::platform::asr_bundled_runtime_status() {
        return compose_status(
            runtime,
            true,
            model,
            super::platform::asr_dependency_installable(),
        );
    }

    let engine = engine_path().is_file();
    let ffmpeg = ffmpeg_available();
    compose_status(
        engine,
        ffmpeg,
        model,
        super::platform::asr_dependency_installable(),
    )
}

fn compose_status(engine: bool, ffmpeg: bool, model: bool, installable: bool) -> VoiceAsrStatus {
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
        installable,
        missing,
    }
}

pub async fn download_current_model(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    download_asr_model(app, current_model_spec()).await
}

pub async fn download_asr_model(
    app: &tauri::AppHandle,
    spec: AsrModelSpec,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(asr_dir()).map_err(|e| format!("创建 ASR 目录失败: {e}"))?;
    let dest = model_download_path();
    debug_assert_eq!(
        dest.file_name().and_then(|name| name.to_str()),
        Some(spec.filename)
    );
    if model_file_verified(&dest, &spec) {
        return Ok(dest);
    }

    let tmp = temp_model_path(&dest);
    let _ = std::fs::remove_file(&tmp);
    let mut last_err = None;
    for url in model_download_urls(&spec) {
        if ASR_CANCEL.load(Ordering::Acquire) {
            emit_download_cancelled(app);
            return Err("已取消".to_string());
        }
        // 进度节流:每 1 MiB 或到达 total 才 emit(与原 download_model_from_url 一致)。
        // helper 把真实 Content-Length(或缺失时回退到 max_bytes=expected_size)作为
        // 进度 total 传入闭包,这里直接透传,不改写。helper 的 on_progress 是
        // `Fn`(非 FnMut),节流用的 last_emit 状态用 Arc<Mutex> 承载以保持 `Fn`。
        let app_clone = app.clone();
        let model_id = spec.id;
        let filename = spec.filename;
        let last_emit_clone = std::sync::Arc::new(std::sync::Mutex::new(0u64));
        let on_progress: Box<dyn Fn(u64, u64) + Send> = Box::new(move |downloaded: u64, t: u64| {
            let mut guard = match last_emit_clone.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            if downloaded - *guard >= 1_048_576 || downloaded >= t {
                *guard = downloaded;
                drop(guard);
                let _ = app_clone.emit(
                    "voice_asr:progress",
                    serde_json::json!({ "stage": "model", "modelId": model_id, "filename": filename, "downloaded": downloaded, "total": t }),
                );
            }
        });
        let is_cancelled = std::sync::Arc::new(|| ASR_CANCEL.load(Ordering::Acquire));
        // 进度 total 的回退估算:与重构前一致用 `expected_size`(缺 Content-Length 时
        // 进度按它算 100%)。max_bytes 只做离谱大文件挡板,刻意与 total 分开——此前
        // 复用单字段会让进度停在约 50%(2*expected_size 当 total)。
        let total_hint = spec.expected_size;
        let max_bytes = spec.expected_size.max(16 * 1024 * 1024) * 2;
        // `verify` 事件恢复到重构前时点:下载成功后、sha256 校验开始前 emit(helper 在
        // sync_all 后、sha256_file 前调用此闭包)。frontend 据此从下载进度(0–95%)
        // 切到「校验中」(96%),不再出现 96%→0% 倒退。
        let app_for_verify = app.clone();
        let on_pre_verify: Option<Box<dyn FnOnce() + Send>> = Some(Box::new(move || {
            let _ = app_for_verify.emit(
                "voice_asr:progress",
                serde_json::json!({ "stage": "verify" }),
            );
        }));
        let req = crate::platform::download::DownloadRequest {
            url: &url,
            dest: &dest,
            part: &tmp,
            expected_sha256: spec.sha256,
            max_bytes,
            total_hint,
            is_cancelled,
            user_agent: Some("pinvou3-asr/1.0"),
            on_progress,
            on_pre_verify,
        };
        match crate::platform::download::download_to_part_with_verify(req).await {
            Ok(()) => {
                remember_model_verification(&dest, &spec, true);
                return Ok(dest);
            }
            Err(err) if err == "已取消" => {
                emit_download_cancelled(app);
                return Err(err);
            }
            Err(err) => {
                let _ = std::fs::remove_file(&tmp);
                if ASR_CANCEL.load(Ordering::Acquire) {
                    emit_download_cancelled(app);
                    return Err("已取消".to_string());
                }
                last_err = Some(err);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| "ASR 模型下载失败".to_string()))
}

fn model_download_urls(spec: &AsrModelSpec) -> Vec<String> {
    if let Ok(url) = std::env::var("PINVOU3_ASR_MODEL_URL") {
        if !url.trim().is_empty() {
            return vec![url];
        }
    }
    let mut urls = vec![spec.primary_url.to_string()];
    if !spec.mirror_url.is_empty() {
        urls.push(spec.mirror_url.to_string());
    }
    urls
}

fn emit_download_cancelled(app: &tauri::AppHandle) {
    let _ = app.emit(
        "voice_asr:progress",
        serde_json::json!({ "stage": "cancelled" }),
    );
}

fn temp_model_path(dest: &Path) -> PathBuf {
    let filename = dest
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("asr-model.gguf");
    dest.with_file_name(format!("{filename}.part"))
}

pub fn model_file_verified(path: &Path, spec: &AsrModelSpec) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() || metadata.len() != spec.expected_size {
        return false;
    }
    let modified = metadata.modified().ok();
    let sha256 = spec.sha256.trim().to_ascii_lowercase();
    if let Ok(cache) = model_verification_cache().lock() {
        if let Some(cache) = cache.as_ref() {
            if cache.path == path
                && cache.len == metadata.len()
                && cache.modified == modified
                && cache.expected_size == spec.expected_size
                && cache.sha256 == sha256
            {
                return cache.verified;
            }
        }
    }
    if spec.sha256.trim().is_empty() {
        remember_model_verification_with_metadata(path, spec, &metadata, true);
        return true;
    }
    let verified = crate::platform::hashing::sha256_file(path)
        .map(|got| got.eq_ignore_ascii_case(spec.sha256))
        .unwrap_or(false);
    remember_model_verification_with_metadata(path, spec, &metadata, verified);
    verified
}

fn model_verification_cache() -> &'static Mutex<Option<ModelVerificationCache>> {
    MODEL_VERIFICATION_CACHE.get_or_init(|| Mutex::new(None))
}

fn remember_model_verification(path: &Path, spec: &AsrModelSpec, verified: bool) {
    if let Ok(metadata) = path.metadata() {
        remember_model_verification_with_metadata(path, spec, &metadata, verified);
    }
}

fn remember_model_verification_with_metadata(
    path: &Path,
    spec: &AsrModelSpec,
    metadata: &std::fs::Metadata,
    verified: bool,
) {
    if let Ok(mut cache) = model_verification_cache().lock() {
        *cache = Some(ModelVerificationCache {
            path: path.to_path_buf(),
            len: metadata.len(),
            modified: metadata.modified().ok(),
            expected_size: spec.expected_size,
            sha256: spec.sha256.trim().to_ascii_lowercase(),
            verified,
        });
    }
}

/// 转码到 16k 单声道 → 调 sense-voice-main → 清洗输出。供 transcribe_voice_audio 调用。
pub fn transcribe(wav: &Path) -> Result<String, String> {
    let engine = engine_path();
    if !engine.is_file() {
        return Err("本地语音识别引擎未安装".to_string());
    }
    let model = model_path();
    if !model_file_verified(&model, &current_model_spec()) {
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
            Ok(o)
                if o.status.success() && norm.metadata().map(|m| m.len() > 44).unwrap_or(false) =>
            {
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
    if !bundled_engine_intact(&engine) {
        return Err("本地语音识别引擎完整性校验失败，已拒绝执行；请重新安装 pinvou3。".to_string());
    }
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
pub async fn voice_asr_status() -> VoiceAsrStatus {
    tokio::task::spawn_blocking(status)
        .await
        .unwrap_or_else(|_| status())
}

/// 一键安装本地语音识别依赖：缺 ffmpeg 走 pkexec apt，缺模型则下载（带进度）。
/// Install local ASR runtime through the current platform implementation.
pub async fn install_voice_asr(app: tauri::AppHandle) -> Result<VoiceAsrStatus, String> {
    if !super::platform::asr_dependency_installable() {
        let _ = app;
        return Err(super::platform::asr_install_unavailable_message().to_string());
    }

    let _guard = begin_asr_install(&app)?;
    super::platform::install_asr_runtime(app.clone()).await?;
    finish_asr_install(&app)
}

/// 仅下载并校验当前平台的按需 ASR 模型，不要求打包运行时在开发目录中可见。
/// 与完整安装入口共享互斥和进度事件，避免两个入口并发写同一个 `.part` 文件。
pub async fn install_voice_asr_model(app: tauri::AppHandle) -> Result<VoiceAsrStatus, String> {
    let _guard = begin_asr_install(&app)?;
    download_current_model(&app).await?;
    finish_asr_install(&app)
}

fn begin_asr_install(app: &tauri::AppHandle) -> Result<InstallGuard, String> {
    if ASR_INSTALLING.swap(true, Ordering::SeqCst) {
        return Err("ASR 模型正在下载或安装中".to_string());
    }
    ASR_CANCEL.store(false, Ordering::Release);
    let _ = app.emit(
        "voice_asr:progress",
        serde_json::json!({ "stage": "start" }),
    );
    Ok(InstallGuard)
}

fn finish_asr_install(app: &tauri::AppHandle) -> Result<VoiceAsrStatus, String> {
    let st = status();
    let _ = app.emit(
        "voice_asr:progress",
        serde_json::json!({ "stage": "done", "ready": st.ready }),
    );
    Ok(st)
}
pub fn cancel_voice_asr() {
    ASR_CANCEL.store(true, Ordering::Release);
}

struct InstallGuard;

impl Drop for InstallGuard {
    fn drop(&mut self) {
        ASR_INSTALLING.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_spec(expected_size: u64, sha256: &'static str) -> AsrModelSpec {
        AsrModelSpec {
            id: "test",
            filename: "test.gguf",
            expected_size,
            sha256,
            primary_url: "",
            mirror_url: "",
        }
    }

    #[test]
    fn model_verification_checks_size_and_sha256() {
        let root = std::env::temp_dir().join(format!("pinvou3-asr-verify-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        let file = root.join("model.gguf");
        std::fs::write(&file, b"abc").unwrap();

        assert!(model_file_verified(
            &file,
            &test_spec(
                3,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            )
        ));
        assert!(!model_file_verified(
            &file,
            &test_spec(
                4,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            )
        ));
        assert!(!model_file_verified(&file, &test_spec(3, "bad")));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn model_verification_cache_records_and_invalidates_file_fingerprint() {
        let root = std::env::temp_dir().join(format!("pinvou3-asr-cache-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        let file = root.join("model.gguf");
        std::fs::write(&file, b"abc").unwrap();
        let spec = test_spec(
            3,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        );

        assert!(model_file_verified(&file, &spec));
        let cached = model_verification_cache()
            .lock()
            .unwrap()
            .clone()
            .expect("verification result should be cached");
        assert_eq!(cached.path, file);
        assert_eq!(cached.len, 3);
        assert!(cached.verified);

        std::fs::write(&file, b"abcd").unwrap();
        assert!(!model_file_verified(&file, &spec));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn status_can_report_runtime_ready_but_model_missing() {
        let st = compose_status(true, true, false, true);

        assert!(st.engine);
        assert!(st.ffmpeg);
        assert!(!st.model);
        assert!(!st.ready);
        assert!(st.installable);
        assert_eq!(st.missing, vec!["model".to_string()]);
    }
}
