//! 知识库 embedding 模型（bge-m3）按需下载 + 校验 + 部署 + 热加载。
//!
//! 模型不再随 deb 打包（deb 瘦 ~559MB）；用户在知识库页主动下载到 [`super::model_dir`]
//! （`~/.pinvou3/knowledge/models/bge-m3`）。下载范式照搬语音 ASR：流式 reqwest +
//! `.part` 临时文件 + 进度事件；额外做 sha256 校验 + tar.gz 解压。候选目录通过真实
//! embedding 加载后才带回滚地替换托管模型并刷新工具门控，**免重启**即可建库/入库/检索。
//!
//! 进度事件 `kb_model:progress`：`{ stage: download|verify|extract|done, downloaded, total, ready }`。

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;

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
static MODEL_LOAD: ModelLoadCoordinator = ModelLoadCoordinator::new();
static MODEL_LOAD_ERROR: Mutex<Option<String>> = Mutex::new(None);

struct ModelLoadCoordinator {
    lock: tokio::sync::Mutex<()>,
    loading: AtomicBool,
}

impl ModelLoadCoordinator {
    const fn new() -> Self {
        Self {
            lock: tokio::sync::Mutex::const_new(()),
            loading: AtomicBool::new(false),
        }
    }

    async fn acquire(&self) -> ModelLoadLease<'_> {
        let guard = self.lock.lock().await;
        self.loading.store(true, Ordering::SeqCst);
        ModelLoadLease {
            coordinator: self,
            _guard: guard,
        }
    }

    fn is_loading(&self) -> bool {
        self.loading.load(Ordering::Acquire)
    }
}

struct ModelLoadLease<'a> {
    coordinator: &'a ModelLoadCoordinator,
    _guard: tokio::sync::MutexGuard<'a, ()>,
}

impl Drop for ModelLoadLease<'_> {
    fn drop(&mut self) {
        self.coordinator.loading.store(false, Ordering::Release);
    }
}

fn model_load_error() -> Option<String> {
    MODEL_LOAD_ERROR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

fn set_model_load_error(error: Option<String>) {
    *MODEL_LOAD_ERROR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = error;
}

fn configured_model_dir() -> std::path::PathBuf {
    std::env::var("PINVOU3_KB_EMBED_MODEL_DIR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(super::model_dir)
}

fn uses_external_model_dir() -> bool {
    let configured = configured_model_dir();
    let managed = super::model_dir();
    match (configured.canonicalize(), managed.canonicalize()) {
        (Ok(configured), Ok(managed)) => configured != managed,
        _ => configured != managed,
    }
}

fn model_directory_is_complete(dir: &Path) -> bool {
    let onnx = dir.join("model.onnx").is_file()
        || dir.join("onnx").join("model_int8.onnx").is_file()
        || dir.join("onnx").join("model.onnx").is_file();
    onnx && [
        "tokenizer.json",
        "config.json",
        "special_tokens_map.json",
        "tokenizer_config.json",
    ]
    .iter()
    .all(|file| dir.join(file).is_file())
}

/// 当前配置模型是否已部署：显式开发覆盖优先，否则检查应用托管目录。
fn installed() -> bool {
    model_directory_is_complete(&configured_model_dir())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KbModelStatus {
    /// 模型文件已部署到磁盘；不等同于进程内推理已就绪。
    pub installed: bool,
    /// 模型已成功加载进当前进程，可执行语义检索和挂载。
    pub ready: bool,
    /// 启动后的后台模型加载仍在进行。
    pub loading: bool,
    /// 模型文件存在，但最近一次进程内加载失败。
    pub failed: bool,
    /// 最近一次模型加载失败的本地诊断信息。
    pub error: Option<String>,
    /// 正在下载/部署中。
    pub downloading: bool,
    /// 下载包近似大小（展示用）。
    pub size_bytes: u64,
    /// 安装占用近似大小（展示用）。
    pub installed_bytes: u64,
    pub version: String,
}

fn current_status(service: &KnowledgeService) -> KbModelStatus {
    let installed = installed();
    let ready = service.semantic_ready();
    let loading = MODEL_LOAD.is_loading();
    let error = model_load_error();
    KbModelStatus {
        installed,
        ready,
        loading,
        failed: installed && !ready && !loading && error.is_some(),
        error,
        downloading: DOWNLOADING.load(Ordering::Relaxed),
        size_bytes: DISPLAY_DOWNLOAD_BYTES,
        installed_bytes: DISPLAY_INSTALLED_BYTES,
        version: MODEL_VERSION.to_string(),
    }
}

/// 前端查询模型状态（offline，不联网）。
pub fn kb_model_status(service: tauri::State<'_, KnowledgeService>) -> KbModelStatus {
    current_status(&service)
}

/// 取消进行中的下载（下次 chunk / 解压前生效）。
pub fn kb_model_cancel() {
    CANCEL.store(true, Ordering::Relaxed);
}

/// React 首帧提交后调用：在 blocking 线程池读取并构建 embedding 模型，完成后原子换入
/// KnowledgeService。模型未安装/加载失败时保持纯全文降级，不影响主界面。
pub async fn kb_model_load_after_first_frame(
    _app: tauri::AppHandle,
    service: tauri::State<'_, KnowledgeService>,
    pool: tauri::State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<bool, String> {
    if service.semantic_ready() {
        return Ok(true);
    }
    if !installed() {
        return Ok(false);
    }

    crate::platform::startup::mark("knowledge_embedder_async:start");
    let ready = load_installed_embedder(&service, &pool)
        .await
        .map_err(|e| {
            crate::platform::startup::mark_with_detail(
                "rust",
                "knowledge_embedder_async:error",
                &e,
            );
            e
        })?;
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
    repair: Option<bool>,
) -> Result<KbModelStatus, String> {
    use tauri::Emitter;

    if DOWNLOADING.load(Ordering::Acquire) {
        return Err("模型正在下载中".into());
    }
    let repair = repair.unwrap_or(false);
    if uses_external_model_dir() && (repair || !installed()) {
        return Err(
            "当前使用 PINVOU3_KB_EMBED_MODEL_DIR 指定的外部模型目录；应用不会覆盖该目录，请修复该目录或移除环境变量后重试"
                .into(),
        );
    }
    if installed() && !repair {
        if service.semantic_ready() {
            return Ok(current_status(&service));
        }
        load_installed_embedder(&service, &pool).await?;
        return Ok(current_status(&service));
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
            let got = crate::platform::hashing::sha256_file(&part2)
                .map_err(|e| format!("读校验文件失败: {e}"))?;
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
        if !model_directory_is_complete(&tmp) {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err("解压结果缺少完整的 ONNX 模型或 tokenizer 配置".into());
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("解压任务失败: {e}"))??;

    // ── 4. 先验证候选模型，再原子换入并热加载（失败时保留旧模型）──────
    let tmp = dir.with_extension("tmp");
    let load_lease = MODEL_LOAD.acquire().await;
    set_model_load_error(None);
    let service_was_ready = service.semantic_ready();
    let candidate_dir = tmp.clone();
    let embedder = match tokio::task::spawn_blocking(move || {
        KnowledgeService::load_embedder_from_dir(&candidate_dir)
    })
    .await
    {
        Ok(Ok(embedder)) => embedder,
        Ok(Err(error)) => {
            set_model_load_error(Some(error.clone()));
            let _ = std::fs::remove_dir_all(&tmp);
            let _ = std::fs::remove_file(&part);
            return Err(error);
        }
        Err(error) => {
            let error = format!("embedding 后台加载任务失败: {error}");
            set_model_load_error(Some(error.clone()));
            let _ = std::fs::remove_dir_all(&tmp);
            let _ = std::fs::remove_file(&part);
            return Err(error);
        }
    };
    let deployment = match deploy_validated_model(&tmp, &dir, service_was_ready) {
        Ok(deployment) => deployment,
        Err(error) => {
            set_model_load_error(Some(error.clone()));
            return Err(error);
        }
    };
    let ready = if deployment.install_embedder {
        service.install_embedder(embedder)
    } else {
        service.semantic_ready()
    };
    if let Some(warning) = deployment.cleanup_warning {
        eprintln!("[knowledge] {warning}");
        set_model_load_error(Some(warning));
    } else {
        set_model_load_error(None);
    }
    super::refresh_kb_tool_gate(&pool).await;
    let _ = std::fs::remove_file(&part);
    let _ = app.emit(
        "kb_model:progress",
        serde_json::json!({ "stage": "done", "ready": ready }),
    );
    drop(load_lease);
    drop(_guard);
    Ok(current_status(&service))
}

async fn load_installed_embedder(
    service: &KnowledgeService,
    pool: &crate::features::assistant::engine_pool::EnginePool,
) -> Result<bool, String> {
    let _lease = MODEL_LOAD.acquire().await;
    if service.semantic_ready() {
        return Ok(true);
    }
    set_model_load_error(None);
    let dir = super::model_dir();
    let embedder = match tokio::task::spawn_blocking(move || {
        KnowledgeService::load_embedder(Some(&dir))
    })
    .await
    {
        Ok(Ok(embedder)) => embedder,
        Ok(Err(error)) => {
            set_model_load_error(Some(error.clone()));
            return Err(error);
        }
        Err(error) => {
            let error = format!("embedding 后台加载任务失败: {error}");
            set_model_load_error(Some(error.clone()));
            return Err(error);
        }
    };
    service.install_embedder(embedder);
    set_model_load_error(None);
    super::refresh_kb_tool_gate(pool).await;
    Ok(service.semantic_ready())
}

struct ModelDeployment {
    install_embedder: bool,
    cleanup_warning: Option<String>,
}

/// 部署已通过真实推理会话验证的候选目录。即使进程内模型已就绪，也必须补齐磁盘目录；
/// 这种情况下保留正在使用的实例，只修复下次启动所需的持久化副本。
fn deploy_validated_model(
    candidate: &Path,
    destination: &Path,
    service_ready: bool,
) -> Result<ModelDeployment, String> {
    let cleanup_warning = replace_model_directory(candidate, destination)?;
    Ok(ModelDeployment {
        install_embedder: !service_ready,
        cleanup_warning,
    })
}

fn replace_model_directory(candidate: &Path, destination: &Path) -> Result<Option<String>, String> {
    let backup = destination.with_extension("backup");
    if backup.exists() {
        if destination.exists() {
            std::fs::remove_dir_all(&backup)
                .map_err(|e| format!("清理上次遗留的模型备份失败({}): {e}", backup.display()))?;
        } else {
            std::fs::rename(&backup, destination).map_err(|e| {
                format!(
                    "恢复上次中断留下的模型备份失败({} -> {}): {e}",
                    backup.display(),
                    destination.display()
                )
            })?;
        }
    }
    let had_destination = destination.exists();
    if had_destination {
        std::fs::rename(destination, &backup).map_err(|e| format!("备份现有模型失败: {e}"))?;
    }
    if let Err(error) = std::fs::rename(candidate, destination) {
        if had_destination {
            if let Err(rollback_error) = std::fs::rename(&backup, destination) {
                return Err(format!(
                    "部署模型失败: {error}; 回滚旧模型也失败: {rollback_error}; 旧模型仍保留在 {}",
                    backup.display()
                ));
            }
        }
        return Err(format!("部署模型失败: {error}"));
    }
    let cleanup_warning = had_destination
        .then(|| std::fs::remove_dir_all(&backup))
        .and_then(Result::err)
        .map(|error| {
            format!(
                "新模型已部署，但清理旧模型备份失败({}): {error}",
                backup.display()
            )
        });
    Ok(cleanup_warning)
}

/// `DOWNLOADING` 复位守卫（任何提前 return 都复位，含 `?` 早退与取消）。
struct DownloadGuard;
impl Drop for DownloadGuard {
    fn drop(&mut self) {
        DOWNLOADING.store(false, Ordering::SeqCst);
    }
}

fn extract_targz(targz: &Path, dest: &Path) -> Result<(), String> {
    let f = std::fs::File::open(targz).map_err(|e| format!("打开下载包失败: {e}"))?;
    let dec = flate2::read::GzDecoder::new(f);
    let mut ar = tar::Archive::new(dec);
    ar.unpack(dest).map_err(|e| format!("解压失败: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    fn temporary_model_root(label: &str) -> std::path::PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "pinvou-model-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[tokio::test]
    async fn model_load_coordinator_serializes_all_loaders() {
        let coordinator = Arc::new(ModelLoadCoordinator::new());
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let coordinator = coordinator.clone();
            let active = active.clone();
            let maximum = maximum.clone();
            tasks.push(tokio::spawn(async move {
                let _lease = coordinator.acquire().await;
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                maximum.fetch_max(now, Ordering::SeqCst);
                tokio::task::yield_now().await;
                active.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for task in tasks {
            task.await.expect("coordinator task should finish");
        }
        assert_eq!(maximum.load(Ordering::SeqCst), 1);
        assert!(!coordinator.is_loading());
    }

    #[tokio::test]
    async fn model_load_coordinator_releases_after_cancelled_loader() {
        let coordinator = Arc::new(ModelLoadCoordinator::new());
        let acquired = Arc::new(tokio::sync::Notify::new());
        let task = {
            let coordinator = coordinator.clone();
            let acquired = acquired.clone();
            tokio::spawn(async move {
                let _lease = coordinator.acquire().await;
                acquired.notify_one();
                std::future::pending::<()>().await;
            })
        };
        acquired.notified().await;
        assert!(coordinator.is_loading());
        task.abort();
        let _ = task.await;
        let lease = tokio::time::timeout(std::time::Duration::from_secs(1), coordinator.acquire())
            .await
            .expect("cancelled loader must release the coordinator");
        drop(lease);
        assert!(!coordinator.is_loading());
    }

    #[test]
    fn model_directory_replacement_removes_verified_old_copy() {
        let root = temporary_model_root("replace");
        let destination = root.join("bge-m3");
        let candidate = root.join("bge-m3.tmp");
        std::fs::create_dir_all(&destination).expect("create destination");
        std::fs::create_dir_all(&candidate).expect("create candidate");
        std::fs::write(destination.join("model.onnx"), b"old").expect("write old model");
        std::fs::write(candidate.join("model.onnx"), b"new").expect("write new model");

        let warning =
            replace_model_directory(&candidate, &destination).expect("replace model directory");

        assert!(warning.is_none());
        assert_eq!(
            std::fs::read(destination.join("model.onnx")).expect("read deployed model"),
            b"new"
        );
        assert!(!destination.with_extension("backup").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ready_service_still_deploys_missing_managed_model() {
        let root = temporary_model_root("ready-missing-disk");
        let destination = root.join("bge-m3");
        let candidate = root.join("bge-m3.tmp");
        std::fs::create_dir_all(&candidate).expect("create candidate");
        std::fs::write(candidate.join("model.onnx"), b"recovered").expect("write recovered model");

        let deployment = deploy_validated_model(&candidate, &destination, true)
            .expect("ready service must still deploy the candidate");

        assert!(!deployment.install_embedder);
        assert!(deployment.cleanup_warning.is_none());
        assert_eq!(
            std::fs::read(destination.join("model.onnx")).expect("read recovered model"),
            b"recovered"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn model_directory_replacement_recovers_interrupted_backup_first() {
        let root = temporary_model_root("recover-backup");
        let destination = root.join("bge-m3");
        let backup = destination.with_extension("backup");
        let candidate = root.join("bge-m3.tmp");
        std::fs::create_dir_all(&backup).expect("create interrupted backup");
        std::fs::create_dir_all(&candidate).expect("create candidate");
        std::fs::write(backup.join("model.onnx"), b"old").expect("write old backup");
        std::fs::write(candidate.join("model.onnx"), b"new").expect("write candidate");

        let warning = replace_model_directory(&candidate, &destination)
            .expect("replacement should recover interrupted backup");

        assert!(warning.is_none());
        assert_eq!(
            std::fs::read(destination.join("model.onnx")).expect("read deployed model"),
            b"new"
        );
        assert!(!backup.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn model_directory_replacement_rolls_back_when_candidate_is_missing() {
        let root = temporary_model_root("rollback");
        let destination = root.join("bge-m3");
        let candidate = root.join("missing.tmp");
        std::fs::create_dir_all(&destination).expect("create destination");
        std::fs::write(destination.join("model.onnx"), b"old").expect("write old model");

        let error = replace_model_directory(&candidate, &destination)
            .expect_err("missing candidate must fail deployment");

        assert!(error.contains("部署模型失败"));
        assert_eq!(
            std::fs::read(destination.join("model.onnx")).expect("read rolled-back model"),
            b"old"
        );
        assert!(!destination.with_extension("backup").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn model_directory_accepts_supported_hugging_face_int8_layout() {
        let root = temporary_model_root("hf-int8");
        std::fs::create_dir_all(root.join("onnx")).expect("create ONNX directory");
        std::fs::write(root.join("onnx").join("model_int8.onnx"), b"model")
            .expect("write int8 model");
        for file in [
            "tokenizer.json",
            "config.json",
            "special_tokens_map.json",
            "tokenizer_config.json",
        ] {
            std::fs::write(root.join(file), b"{}").expect("write tokenizer config");
        }

        assert!(model_directory_is_complete(&root));
        std::fs::remove_file(root.join("tokenizer_config.json")).expect("remove required config");
        assert!(!model_directory_is_complete(&root));
        let _ = std::fs::remove_dir_all(root);
    }
}
