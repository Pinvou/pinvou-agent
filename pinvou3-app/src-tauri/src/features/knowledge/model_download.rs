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
/// 最近一次首帧/热加载跳过确因「无使用场景」门控：模型已装但被故意延迟加载，
/// 用于让前端把该状态与真实加载失败区分开（故意跳过 ≠ 失败）。任何真实加载
/// 尝试（成功或失败）都会在入口清除该标记。
static MODEL_DEFERRED_NO_USAGE: AtomicBool = AtomicBool::new(false);

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

/// 任何真实加载尝试（成功或失败）前清除「无使用场景故意延迟」标记:导入补载
/// 等绕过状态命令的加载入口也在其列——否则首帧被门控跳过的用户导入文件时,
/// 补载失败仍被前端误标为 deferred(「故意延迟」),真实失败被掩盖。
pub(super) fn clear_deferred_no_usage() {
    MODEL_DEFERRED_NO_USAGE.store(false, Ordering::SeqCst);
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
    /// 模型已安装，但因本地无已入库内容且远程无连接被首帧门控故意延迟加载；
    /// 语义检索退化为全文（既有降级语义），区别于真实加载失败。
    pub deferred_no_usage: bool,
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
        deferred_no_usage: deferred_no_usage(
            MODEL_DEFERRED_NO_USAGE.load(Ordering::Acquire),
            installed,
            ready,
        ),
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
    // 用户主动下载/修复 = 明确使用意图：解除「无使用场景故意延迟」标记，
    // 保证其后真实的下载/部署/加载失败按失败上报，而不会被 deferred 掩盖。
    MODEL_DEFERRED_NO_USAGE.store(false, Ordering::SeqCst);
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
    // 首帧门控（kb_model_load_after_first_frame 与 kb_model_status 热加载共用本
    // 入口，改这一处两个调用点同时生效）：磁盘目录完整 ≠ 值得加载。本地无已
    // 入库内容且远程无连接时，加载 ~570MB 模型纯属白占内存（没有任何检索、
    // 建库路径会用它）；曾建库后清空的用户同样跳过——has_indexed_content 就是
    // 「当前有可检索内容」的语义。返回 Ok(false) = 未安装式静默降级，语义与
    // 模型未部署完全一致；用户建库/下载模型/挂载校验的既有路径不受影响。
    if !knowledge_usage_present(service) {
        MODEL_DEFERRED_NO_USAGE.store(true, Ordering::SeqCst);
        crate::platform::startup::mark_with_detail(
            "rust",
            "knowledge_embedder_async:skipped_no_usage",
            &format!(
                "indexed_content={} remote_connections={}",
                service.has_indexed_content(),
                remote_has_connections(),
            ),
        );
        return Ok(false);
    }
    // 门控放行 = 即将真实加载：清除故意延迟标记，此后的失败按真实失败上报。
    MODEL_DEFERRED_NO_USAGE.store(false, Ordering::SeqCst);
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

/// 是否存在会用到 embedding 模型的使用迹象：本地有已入库内容，或配置了远程
/// 知识库连接（与 tool_policy 的 kb_usable 同口径：远程连接存在即视为知识库
/// 可用，宁可保守加载；远程检索本身在服务端嵌入，本地模型只为挂载校验与
/// 可能的本地混检兜底）。两者皆无 → 首帧不加载。
fn knowledge_usage_present(service: &KnowledgeService) -> bool {
    usage_present(service.has_indexed_content(), remote_has_connections())
}

/// usage_present 的纯函数核心（便于单测）：任一使用迹象存在即加载。
fn usage_present(indexed_content: bool, remote_connections: bool) -> bool {
    indexed_content || remote_connections
}

/// deferred_no_usage 的纯函数核心（便于单测）：只有「已安装、未就绪、最近一次
/// 跳过确因无使用场景」三者同时成立才算故意延迟。加载成功（ready）或模型目录
/// 被移除（!installed）自然解除；真实加载尝试（含用户点重试/下载/导入补载）在
/// 入口即清除标记，因此其后的真实失败不会被误标为 deferred。
fn deferred_no_usage(deferred_flag: bool, installed: bool, ready: bool) -> bool {
    deferred_flag && installed && !ready
}

/// 远程知识库连接是否存在。不经过 Tauri managed state——model_download 不便
/// 依赖 RemoteKnowledgeService 实例（各 feature 自行 manage 自己的 state），
/// 因此直接读默认路径的连接文件；读不到（文件缺失/解析失败）按「无连接」
/// 处理，语义保守（跳过加载，用户连上远程库后热加载钩子会补上）。
fn remote_has_connections() -> bool {
    crate::features::remote_knowledge::RemoteKnowledgeService::remote_connections_present()
}

/// 调用方已经持有模型目录安装锁，或使用不由应用管理的外部只读目录。
async fn load_installed_embedder_unlocked(
    service: &KnowledgeService,
    pool: &crate::features::assistant::engine_pool::EnginePool,
    model_dir: std::path::PathBuf,
) -> Result<bool, String> {
    // 真实加载尝试（含 kb_model_download 绕过门控的补载）：清除故意延迟标记。
    MODEL_DEFERRED_NO_USAGE.store(false, Ordering::SeqCst);
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

/// kb_search 自愈路径的租约化重载：模型被空闲卸载/首帧门控跳过后，工具执行时
/// 按需恢复。与模型下载、首帧加载共用 `MODEL_LOAD` 租约：并发 kb_search（同
/// turn 并行工具调用）等待先行者完成后在租约内复查短路，不重复读 ~570MB；
/// 与 kb_model_download 的目录替换互斥，不会读到半替换状态。失败写
/// `MODEL_LOAD_ERROR`（前端状态页可见）并返回 Err，调用方降级纯全文。
///
/// 工具门控（lib.rs tool_policy）不依赖 semantic_ready，重载成败都不需要刷新
/// disallowed_tools；被 `tokio::time::timeout` 取消等待时，后台加载任务本身
/// 不受影响（spawn_blocking 闭包继续执行并落位），仅当次检索降级。
pub(super) async fn reload_installed_embedder_leased(
    service: &KnowledgeService,
) -> Result<bool, String> {
    let model_dir = configured_model_dir();
    let ready_probe = service.clone();
    let load_service = service.clone();
    leased_reload_with(
        model_installed(),
        move || ready_probe.semantic_ready(),
        move || {
            Box::pin(async move {
                tokio::task::spawn_blocking(move || {
                    KnowledgeService::load_embedder(Some(&model_dir))
                        .map(|embedder| load_service.install_embedder(embedder))
                })
                .await
                .unwrap_or_else(|error| Err(format!("embedding 后台加载任务失败: {error}")))
            })
        },
    )
    .await
}

/// 上一方法的可测核心：`installed` 门控 → 清 deferred 标记 → 拿租约 →
/// **租约内**复查 ready（并发调用等待先行加载后短路）→ 加载并安装。
/// `ready` 与 `load_and_install` 注入，使并发回归测试无需真实模型文件。
/// 加载器必须自行把阻塞 IO 放进 spawn_blocking（生产实现已做）。
async fn leased_reload_with(
    installed: bool,
    ready: impl Fn() -> bool,
    load_and_install: impl FnOnce() -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<bool, String>> + Send>,
    >,
) -> Result<bool, String> {
    if !installed {
        return Ok(false);
    }
    // 真实加载尝试：清除「无使用场景故意延迟」标记，失败按真实失败上报。
    MODEL_DEFERRED_NO_USAGE.store(false, Ordering::SeqCst);
    let _lease = MODEL_LOAD.acquire().await;
    if ready() {
        return Ok(true);
    }
    set_model_load_error(None);
    match load_and_install().await {
        Ok(ready) => {
            set_model_load_error(None);
            Ok(ready)
        }
        Err(error) => {
            set_model_load_error(Some(error.clone()));
            Err(error)
        }
    }
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
    fn first_frame_load_requires_local_content_or_remote_connections() {
        // 两者皆无 → 跳过加载（磁盘目录即使完整也不白占 ~570MB）。
        assert!(!usage_present(false, false));
        // 本地有已入库内容（含曾建库后仍有文档的用户）→ 加载。
        assert!(usage_present(true, false));
        // 远程连接存在（远程检索要本地嵌入查询向量）→ 加载。
        assert!(usage_present(false, true));
        assert!(usage_present(true, true));
    }

    #[test]
    fn deferred_no_usage_requires_installed_not_ready_and_skip_marker() {
        // 跳过标记 + 已安装 + 未就绪 → 故意延迟（前端据此区分真实加载失败）。
        assert!(deferred_no_usage(true, true, false));
        // 加载成功后 ready=true，即使标记未及时清除也不应再报 deferred。
        assert!(!deferred_no_usage(true, true, true));
        // 模型目录被移除（未安装）时按「未安装」语义处理，不算 deferred。
        assert!(!deferred_no_usage(true, false, false));
        // 真实加载尝试在入口清除标记：其后的真实失败不得被标为 deferred。
        assert!(!deferred_no_usage(false, true, false));
        assert!(!deferred_no_usage(false, false, false));
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

    /// kb_search 自愈重载的并发回归：N 路并发重载同一槽位，真实加载只允许发生
    /// 一次（租约串行 + 租约内复查 ready 短路），其余等待者拿到 Ok(true)。
    /// 复现审计缺陷：并发 kb_search 各自 spawn_blocking 重载 ~570MB 模型。
    #[tokio::test]
    async fn leased_reload_concurrent_calls_load_once() {
        let ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let loads = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let ready = ready.clone();
            let loads = loads.clone();
            let ready_after_load = ready.clone();
            tasks.push(tokio::spawn(async move {
                leased_reload_with(
                    true,
                    move || ready.load(Ordering::SeqCst),
                    move || {
                        let loads = loads.clone();
                        Box::pin(async move {
                            loads.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                            ready_after_load.store(true, Ordering::SeqCst);
                            Ok(true)
                        })
                    },
                )
                .await
            }));
        }
        for task in tasks {
            let outcome = task.await.expect("reload task should finish");
            assert!(outcome.is_ok(), "等待先行加载后短路应返回 Ok: {outcome:?}");
            assert_eq!(outcome, Ok(true));
        }
        assert_eq!(
            loads.load(Ordering::SeqCst),
            1,
            "并发重载必须只真实加载一次（租约内复查短路）"
        );
    }

    /// 未安装时重载直接 Ok(false) 短路，不触碰租约也不发起加载。
    #[tokio::test]
    async fn leased_reload_skips_when_not_installed() {
        let loads = Arc::new(AtomicUsize::new(0));
        let loads_probe = loads.clone();
        let outcome = leased_reload_with(
            false,
            || false,
            move || {
                loads_probe.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(true) })
            },
        )
        .await;
        assert_eq!(outcome, Ok(false));
        assert_eq!(loads.load(Ordering::SeqCst), 0);
    }

    /// 加载失败不落 ready，错误穿透给调用方（kb_search 降级纯全文），后续
    /// 调用仍会重试（不缓存失败）。
    #[tokio::test]
    async fn leased_reload_failure_propagates_and_retries() {
        let loads = Arc::new(AtomicUsize::new(0));
        for expected_error in ["第一次失败", "第二次失败"] {
            let loads = loads.clone();
            let outcome = leased_reload_with(
                true,
                || false,
                move || {
                    let loads = loads.clone();
                    Box::pin(async move {
                        loads.fetch_add(1, Ordering::SeqCst);
                        Err(expected_error.to_string())
                    })
                },
            )
            .await;
            assert_eq!(outcome, Err(expected_error.to_string()));
        }
        assert_eq!(loads.load(Ordering::SeqCst), 2, "失败不缓存,下次重试");
        // 真实加载尝试清 deferred 标记（失败按真实失败上报,不误标「故意延迟」）。
        assert!(!MODEL_DEFERRED_NO_USAGE.load(Ordering::SeqCst));
    }

    /// 工具门控语义：锚定 lib.rs tool_policy 实际引用的纯函数
    /// KnowledgeService::kb_tools_usable（此前该测试断言测试内自定义闭包，属
    /// 同义反复——生产判定若回退掺回 semantic_ready 此灯不会红）。模型卸载
    /// （!ready）后只要有已入库内容,kb_search/kb_open_source 就必须保持可见
    /// ——否则快照式重算会把工具写进 disallowed,工具内按需重载的自愈路径不可达。
    /// 函数签名刻意不含模型状态参数:任何把 semantic_ready 掺回来的改动会直接
    /// 编译错或在此红灯。
    #[test]
    fn kb_tools_stay_visible_after_model_unload_when_content_present() {
        use super::KnowledgeService;
        // 有内容（本地任一分支）+ 模型已卸载：工具仍可见（自愈重载路径可达）。
        assert!(KnowledgeService::kb_tools_usable(true, false));
        // 有内容 + 远程连接：工具可见（远程检索不依赖本地模型）。
        assert!(KnowledgeService::kb_tools_usable(false, true));
        assert!(KnowledgeService::kb_tools_usable(true, true));
        // 库空且无远程连接：工具不可见（不空宣传能力）。
        assert!(!KnowledgeService::kb_tools_usable(false, false));
    }

    /// 集成层：空闲卸载语义（set_embedder(None)）不改变 has_indexed_content 的
    /// 读数——门控数据源与 embedder 槽解耦。
    #[test]
    fn idle_unload_keeps_has_indexed_content_stable() {
        let dir = std::env::temp_dir().join(format!(
            "pinvou3_kb_gate_unload_{}_{}",
            std::process::id(),
            line!()
        ));
        let service = KnowledgeService::new(&dir.join("index.db")).expect("KnowledgeService::new");
        let l1 = service.l1();
        let collection_id = l1
            .create_collection("回归知识集", None, None)
            .expect("create collection");
        let doc_id = l1
            .upsert_document(
                collection_id,
                dir.to_string_lossy().as_ref(),
                "回归文档",
                Some("md"),
                1,
                0,
            )
            .expect("upsert document");
        assert!(doc_id > 0);
        assert!(service.has_indexed_content(), "入库后有内容");
        // 模拟空闲卸载：embedder 槽置空后内容读数不变（工具可见性不波动）。
        assert!(!service.semantic_ready());
        service.l1().set_embedder(None);
        assert!(!service.semantic_ready());
        assert!(
            service.has_indexed_content(),
            "卸载后 has_indexed_content 读数不变"
        );
        let _ = std::fs::remove_dir_all(&dir);
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
