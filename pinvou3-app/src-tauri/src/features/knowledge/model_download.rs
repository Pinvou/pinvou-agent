//! 知识库 embedding 模型（bge-m3）按需下载 + 校验 + 部署 + 热加载。
//!
//! 模型不再随 deb 打包（deb 瘦 ~559MB）；用户在知识库页主动下载到 [`super::model_dir`]
//! （`~/.pinvou3/knowledge/models/bge-m3`）。下载范式照搬语音 ASR：流式 reqwest +
//! `.part` 临时文件 + 进度事件；额外做 sha256 校验 + tar.gz 解压。完成后 `reload_embedder`
//! 热加载 embedding + 刷新工具门控，**免重启**即可建库/入库/检索。
//!
//! 进度事件 `kb_model:progress`：`{ stage: download|verify|extract|done, downloaded, total, ready }`。

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::KnowledgeService;

/// Community model asset hosted in the public GitHub release. The
/// `PINVOU3_KB_MODEL_URL` environment variable can override it for mirrors and tests.
const MODEL_URL: &str =
    "https://github.com/Pinvou/pinvou-agent/releases/download/kb-model-v1/bge-m3.tar.gz";
/// tar.gz 的 sha256（发布模型后回填；空=跳过校验）。
/// env `PINVOU3_KB_MODEL_SHA256` 覆盖。重发模型务必同步更新此值。
const MODEL_SHA256: &str = "86438791d1ee7c9989c75878d3623ab28a7e4cd57aa3a7816480043d1de62efe";
/// tar.gz 字节数（content-length 缺失时的进度兜底；2026-06-30 发布实测值）。
const MODEL_TARGZ_SIZE: u64 = 407_925_014;
/// 展示用：下载包(~389MB tar.gz) / 安装占用(~558MB 解压后) 近似大小（前端 chip 显示）。
const DISPLAY_DOWNLOAD_BYTES: u64 = 407_925_014;
const DISPLAY_INSTALLED_BYTES: u64 = 585_556_897;
/// 模型版本标识（前端 `.pkg-ver` 显示）。
pub const MODEL_VERSION: &str = "bge-m3";

static DOWNLOADING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
/// 防止前端重复首帧 effect 同时构建多份 558 MiB 模型。
static STARTUP_LOADING: AtomicBool = AtomicBool::new(false);

/// 模型是否已部署：落点目录下 `model.onnx`（或 `onnx/model.onnx`）+ `tokenizer.json` 都在。
fn installed() -> bool {
    let dir = super::model_dir();
    let onnx = dir.join("model.onnx").is_file() || dir.join("onnx").join("model.onnx").is_file();
    onnx && dir.join("tokenizer.json").is_file()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbModelStatus {
    /// 模型已部署（语义检索可用）。
    pub installed: bool,
    /// 正在下载/部署中。
    pub downloading: bool,
    /// 下载包近似大小（展示用）。
    pub size_bytes: u64,
    /// 安装占用近似大小（展示用）。
    pub installed_bytes: u64,
    pub version: String,
}

fn current_status() -> KbModelStatus {
    KbModelStatus {
        installed: installed(),
        downloading: DOWNLOADING.load(Ordering::Relaxed),
        size_bytes: DISPLAY_DOWNLOAD_BYTES,
        installed_bytes: DISPLAY_INSTALLED_BYTES,
        version: MODEL_VERSION.to_string(),
    }
}

/// 前端查询模型状态（offline，不联网）。
pub fn kb_model_status() -> KbModelStatus {
    current_status()
}

/// 取消进行中的下载（下次 chunk / 解压前生效）。
pub fn kb_model_cancel() {
    CANCEL.store(true, Ordering::Relaxed);
}

/// React 首帧提交后调用：在 blocking 线程池读取并构建 embedding 模型，完成后原子换入
/// KnowledgeService。模型未安装/加载失败时保持纯全文降级，不影响主界面。
pub async fn kb_model_load_after_first_frame(
    app: tauri::AppHandle,
    service: tauri::State<'_, KnowledgeService>,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<bool, String> {
    if service.semantic_ready() {
        return Ok(true);
    }
    if STARTUP_LOADING.swap(true, Ordering::SeqCst) {
        return Ok(false);
    }
    let _guard = StartupLoadGuard;

    crate::platform::startup::mark("knowledge_embedder_async:start");
    let dir = super::model_dir();
    let embedder = tokio::task::spawn_blocking(move || KnowledgeService::load_embedder(Some(&dir)))
        .await
        .map_err(|e| format!("embedding 后台加载任务失败: {e}"))?;

    // 若另一路热加载已抢先完成，不用较旧的构建结果覆盖它。
    if !service.semantic_ready() {
        service.install_embedder(embedder);
    }
    let ready = service.semantic_ready();
    if ready {
        super::refresh_kb_tool_gate(&pool).await;
    }
    crate::platform::startup::mark_with_detail(
        "rust",
        "knowledge_embedder_async:done",
        &format!("ready={ready}"),
    );
    Ok(ready)
}

/// 按需下载 + 校验 + 解压部署 embedding 模型，完成后热加载并刷新工具门控（免重启）。
pub async fn kb_model_download(
    app: tauri::AppHandle,
    service: tauri::State<'_, KnowledgeService>,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<KbModelStatus, String> {
    use tauri::Emitter;

    if installed() {
        return Ok(current_status());
    }
    if DOWNLOADING.swap(true, Ordering::SeqCst) {
        return Err("模型正在下载中".into());
    }
    CANCEL.store(false, Ordering::Relaxed);
    // 守卫：任何提前 return（含 ?、取消）退出时都复位 DOWNLOADING。
    let _guard = DownloadGuard;

    let dir = super::model_dir();
    let parent = dir
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| dir.clone());
    std::fs::create_dir_all(&parent).map_err(|e| format!("创建目录失败: {e}"))?;
    let part = parent.join("bge-m3.tar.gz.part");

    // ── 1. 流式下载 → .part ───────────────────────────────────────
    let url = std::env::var("PINVOU3_KB_MODEL_URL").unwrap_or_else(|_| MODEL_URL.to_string());
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .user_agent("pinvou3-kb/1.0")
        .build()
        .map_err(|e| format!("HTTP client 构建失败: {e}"))?;
    let mut resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("连接模型源失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("模型源响应异常: {e}"))?;
    let total = resp
        .content_length()
        .filter(|n| *n > 0)
        .unwrap_or(MODEL_TARGZ_SIZE);
    let mut file = std::fs::File::create(&part).map_err(|e| format!("创建文件失败: {e}"))?;
    let mut downloaded: u64 = 0;
    let mut last_emit: u64 = 0;
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("下载中断: {e}"))? {
        if CANCEL.load(Ordering::Relaxed) {
            drop(file);
            let _ = std::fs::remove_file(&part);
            return Err("已取消".into());
        }
        file.write_all(&chunk)
            .map_err(|e| format!("写盘失败: {e}"))?;
        downloaded += chunk.len() as u64;
        if downloaded - last_emit >= 2_097_152 || (total > 0 && downloaded >= total) {
            last_emit = downloaded;
            let _ = app.emit(
                "kb_model:progress",
                serde_json::json!({ "stage": "download", "downloaded": downloaded, "total": total }),
            );
        }
    }
    drop(file);

    // ── 2. sha256 校验 + 3. 解压部署（CPU/IO 重，挪 spawn_blocking）───
    let part2 = part.clone();
    let dir2 = dir.clone();
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        // 校验：const 或 env 给了 sha256 才校验（占位空串=跳过，dev 用本地包时方便）。
        let expected = std::env::var("PINVOU3_KB_MODEL_SHA256")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| (!MODEL_SHA256.is_empty()).then(|| MODEL_SHA256.to_string()));
        if let Some(exp) = expected {
            let _ = app2.emit(
                "kb_model:progress",
                serde_json::json!({ "stage": "verify" }),
            );
            let got = sha256_file(&part2)?;
            if !got.eq_ignore_ascii_case(&exp) {
                let _ = std::fs::remove_file(&part2);
                return Err(format!(
                    "模型校验失败(sha256 不匹配): 期望 {exp:.12} 实际 {got:.12}"
                ));
            }
        }
        // 解压到临时目录再原子换入（避免半包污染落点）。
        let _ = app2.emit(
            "kb_model:progress",
            serde_json::json!({ "stage": "extract" }),
        );
        let tmp = dir2.with_extension("tmp");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).map_err(|e| format!("创建解压目录失败: {e}"))?;
        extract_targz(&part2, &tmp)?;
        if !(tmp.join("model.onnx").is_file() || tmp.join("onnx").join("model.onnx").is_file()) {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err("解压结果缺 model.onnx".into());
        }
        let _ = std::fs::remove_dir_all(&dir2);
        std::fs::rename(&tmp, &dir2).map_err(|e| format!("部署模型失败: {e}"))?;
        let _ = std::fs::remove_file(&part2);
        Ok(())
    })
    .await
    .map_err(|e| format!("解压任务失败: {e}"))??;

    // ── 4. 热加载 + 刷新工具门控（免重启）─────────────────────────
    let ready = service.reload_embedder();
    super::refresh_kb_tool_gate(&pool).await;
    let _ = app.emit(
        "kb_model:progress",
        serde_json::json!({ "stage": "done", "ready": ready }),
    );
    Ok(current_status())
}

/// `DOWNLOADING` 复位守卫（任何提前 return 都复位，含 `?` 早退与取消）。
struct DownloadGuard;
impl Drop for DownloadGuard {
    fn drop(&mut self) {
        DOWNLOADING.store(false, Ordering::SeqCst);
    }
}

struct StartupLoadGuard;
impl Drop for StartupLoadGuard {
    fn drop(&mut self) {
        STARTUP_LOADING.store(false, Ordering::SeqCst);
    }
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("打开校验文件失败: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| format!("读校验文件失败: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest.iter() {
        s.push_str(&format!("{b:02x}"));
    }
    Ok(s)
}

fn extract_targz(targz: &Path, dest: &Path) -> Result<(), String> {
    let f = std::fs::File::open(targz).map_err(|e| format!("打开下载包失败: {e}"))?;
    let dec = flate2::read::GzDecoder::new(f);
    let mut ar = tar::Archive::new(dec);
    ar.unpack(dest).map_err(|e| format!("解压失败: {e}"))?;
    Ok(())
}
