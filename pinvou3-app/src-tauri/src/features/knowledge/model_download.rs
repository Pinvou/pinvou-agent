//! 知识库 embedding 模型（bge-m3）按需下载 + 校验 + 部署 + 热加载。
//!
//! 模型不再随安装包打包；用户在知识库页主动下载到 [`super::model_dir`]
//! （`~/.pinvou3/knowledge/models/bge-m3`）。固定 revision 的五个文件由
//! `pinvou-knowledge` 统一流式下载并逐文件校验。候选目录通过真实
//! embedding 加载后才带回滚地替换托管模型并刷新工具门控，**免重启**即可建库/入库/检索。
//!
//! 进度事件 `kb_model:progress`：`{ stage: download|verify|prepare|done, downloaded, total, ready }`。

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use super::KnowledgeService;
use serde::Serialize;

/// 桌面端可单独指定镜像；未配置时回退到两端统一的镜像变量。
const DESKTOP_HF_BASE_URL_ENV: &str = "PINVOU3_KB_HF_BASE_URL";
/// 展示用：固定清单的下载量与实际模型文件占用（不含文件系统簇开销）。
const DISPLAY_DOWNLOAD_BYTES: u64 =
    pinvou_knowledge::model_download::KNOWLEDGE_MODEL_DOWNLOAD_BYTES;
const DISPLAY_INSTALLED_BYTES: u64 =
    pinvou_knowledge::model_download::KNOWLEDGE_MODEL_DOWNLOAD_BYTES;
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
    configured_model_dir_from(
        std::env::var("PINVOU3_KB_EMBED_MODEL_DIR").ok(),
        super::model_dir(),
    )
}

fn configured_model_dir_from(
    configured: Option<String>,
    managed: std::path::PathBuf,
) -> std::path::PathBuf {
    configured
        .filter(|value| !value.trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or(managed)
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
    pinvou_knowledge::model_download::model_directory_is_complete(dir)
}

/// 当前配置模型是否已部署：显式开发覆盖优先，否则检查应用托管目录。
pub(crate) fn model_installed() -> bool {
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

pub(crate) fn current_status(service: &KnowledgeService) -> KbModelStatus {
    let installed = model_installed();
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

/// 取消进行中的下载（下次网络数据块或文件校验边界生效）。
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

/// 按需下载 + 校验 + 部署 embedding 模型，完成后热加载并刷新工具门控（免重启）。
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
    let external_model_dir = uses_external_model_dir();
    let configured_dir = configured_model_dir();
    if external_model_dir && (repair || !model_directory_is_complete(&configured_dir)) {
        return Err(
            "当前使用 PINVOU3_KB_EMBED_MODEL_DIR 指定的外部模型目录；应用不会覆盖该目录，请修复该目录或移除环境变量后重试"
                .into(),
        );
    }
    let dir = super::model_dir();
    // 共享服务与桌面端在 Linux 宿主上复用同一个模型目录。必须在第一次
    // 完整性检查前获取跨进程锁，并持有到候选模型构造和目录替换完成。
    // 外部只读模型目录不由应用管理，因此不创建锁文件。
    let _install_lock = (!external_model_dir)
        .then(|| pinvou_knowledge::try_lock_knowledge_model_install(&dir))
        .transpose()?;
    if !external_model_dir {
        if let Some(warning) = pinvou_knowledge::model_download::recover_model_directory(&dir)? {
            eprintln!("[knowledge] {warning}");
            set_model_load_error(Some(warning));
        }
    }
    if model_directory_is_complete(&configured_dir) && !repair {
        if service.semantic_ready() {
            return Ok(current_status(&service));
        }
        load_installed_embedder_unlocked(&service, &pool, configured_dir).await?;
        return Ok(current_status(&service));
    }
    if DOWNLOADING.swap(true, Ordering::SeqCst) {
        return Err("模型正在下载中".into());
    }
    CANCEL.store(false, Ordering::Relaxed);
    // 守卫：任何提前 return（含 ?、取消）退出时都复位 DOWNLOADING。
    let _guard = DownloadGuard;

    let parent = dir
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| dir.clone());
    std::fs::create_dir_all(&parent).map_err(|e| format!("创建目录失败: {e}"))?;
    // ── 1. 固定 revision 五文件清单下载 + 逐文件大小/SHA-256 校验 ──
    let tmp = dir.with_extension("tmp");
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp)
            .map_err(|e| format!("清理上次模型候选目录失败({}): {e}", tmp.display()))?;
    }
    let hf_base_url = std::env::var(DESKTOP_HF_BASE_URL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(pinvou_knowledge::model_download::knowledge_model_hf_base_url);
    let progress_app = app.clone();
    pinvou_knowledge::model_download::download_knowledge_model_candidate(
        &tmp,
        &hf_base_url,
        move |progress| {
            let stage = match progress.stage {
                pinvou_knowledge::model_download::KnowledgeModelDownloadStage::Download => {
                    "download"
                }
                pinvou_knowledge::model_download::KnowledgeModelDownloadStage::Verify => "verify",
            };
            let _ = progress_app.emit(
                "kb_model:progress",
                serde_json::json!({
                    "stage": stage,
                    "downloaded": progress.downloaded_bytes,
                    "total": progress.total_bytes,
                    "fileIndex": progress.file_index,
                    "fileCount": progress.file_count,
                    "file": progress.source_path,
                }),
            );
        },
        || CANCEL.load(Ordering::Relaxed),
    )
    .await?;
    if !model_directory_is_complete(&tmp) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("下载结果缺少完整的 ONNX 模型或 tokenizer 配置".into());
    }

    // ── 2. 真实加载候选模型，再原子换入并热加载（失败时保留旧模型）──
    let _ = app.emit(
        "kb_model:progress",
        serde_json::json!({
            "stage": "prepare",
            "downloaded": DISPLAY_DOWNLOAD_BYTES,
            "total": DISPLAY_DOWNLOAD_BYTES,
        }),
    );
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
            return Err(error);
        }
        Err(error) => {
            let error = format!("embedding 后台加载任务失败: {error}");
            set_model_load_error(Some(error.clone()));
            let _ = std::fs::remove_dir_all(&tmp);
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
    let _ = app.emit(
        "kb_model:progress",
        serde_json::json!({ "stage": "done", "ready": ready }),
    );
    drop(load_lease);
    drop(_guard);
    Ok(current_status(&service))
}

pub(crate) async fn load_installed_embedder(
    service: &KnowledgeService,
    pool: &crate::features::assistant::engine_pool::EnginePool,
) -> Result<bool, String> {
    let external_model_dir = uses_external_model_dir();
    let configured_dir = configured_model_dir();
    let _install_lock = (!external_model_dir)
        .then(|| pinvou_knowledge::try_lock_knowledge_model_install(&configured_dir))
        .transpose()?;
    if !external_model_dir {
        if let Some(warning) =
            pinvou_knowledge::model_download::recover_model_directory(&configured_dir)?
        {
            eprintln!("[knowledge] {warning}");
            set_model_load_error(Some(warning));
        }
    }
    if !model_directory_is_complete(&configured_dir) {
        return Ok(false);
    }
    load_installed_embedder_unlocked(service, pool, configured_dir).await
}

/// 调用方已经持有模型目录安装锁，或使用不由应用管理的外部只读目录。
async fn load_installed_embedder_unlocked(
    service: &KnowledgeService,
    pool: &crate::features::assistant::engine_pool::EnginePool,
    model_dir: std::path::PathBuf,
) -> Result<bool, String> {
    let _lease = MODEL_LOAD.acquire().await;
    if service.semantic_ready() {
        return Ok(true);
    }
    set_model_load_error(None);
    let embedder = match tokio::task::spawn_blocking(move || {
        KnowledgeService::load_embedder(Some(&model_dir))
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
    pinvou_knowledge::model_download::install_model_candidate(candidate, destination)
}

/// `DOWNLOADING` 复位守卫（任何提前 return 都复位，含 `?` 早退与取消）。
struct DownloadGuard;
impl Drop for DownloadGuard {
    fn drop(&mut self) {
        DOWNLOADING.store(false, Ordering::SeqCst);
    }
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

    #[test]
    fn configured_model_directory_prefers_non_empty_external_override() {
        let managed = std::path::PathBuf::from("managed/bge-m3");
        let external = std::path::PathBuf::from("external/bge-m3");

        assert_eq!(
            configured_model_dir_from(
                Some(external.to_string_lossy().into_owned()),
                managed.clone()
            ),
            external
        );
        assert_eq!(
            configured_model_dir_from(Some("  ".into()), managed.clone()),
            managed
        );
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
