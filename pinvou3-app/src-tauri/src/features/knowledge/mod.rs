//! 本地知识底座 L0：全系统元数据索引 + 秒搜 + 去重。
//!
//! v0 以 in-process 模块落地（复用
//! `bridge::paths`/Tauri 命令通路），用 [`KnowledgeService`]（UI 无关）收口，
//! 便于日后抽成独立 `pinvou3-knowledged` daemon + MCP（`kb_*`）。
//!
//! 分层提醒：本模块只做 **L0 元数据**（零模型）。内容解析 / 全文 / 向量是 L1（后续），
//! LLM 理解是 L2（纯按需）。**绝不在这里全盘跑模型分类**——那是 Marvis 的坑。

#[cfg(test)]
mod e2e_test;
mod exclude;
mod import_jobs;
mod kb_tool;
mod l1;
/// embedding 模型按需下载命令（pub mod：tauri::command 宏生成的 `__cmd__` 助手需经全路径
/// `knowledge::model_download::kb_model_*` 引用，`pub use` 重导出函数带不出宏）。
pub mod model_download;
mod query;
mod scanner;
mod store;

pub(crate) use exclude::Excluder;
pub use import_jobs::{FailedImportFilePage, ImportJobState as IndexState};
pub use kb_tool::{KbOpenSourceTool, KbSearchTool};
pub use l1::{Collection, Document};

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::State;

pub use store::{FileHit, Stats, TypeCount};
use store::{SearchQuery, Store};

/// Background scan progress (polled by the frontend). The frontend reads
/// running/phase/scanned/finishedAt; `roots` (added with the headless
/// surface) reports the scanned roots and is ignored by the GUI today.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScanState {
    pub running: bool,
    /// idle / scanning / done / cancelled / interrupted（扫描线程 panic 被兜底，
    /// 见 `finish_scan_after_panic`）。前端只在 `done` 时刷新 L0，所以
    /// `interrupted` 不会被误当成「扫完了」。
    pub phase: String,
    pub roots: Vec<String>,
    pub scanned: u64,
    pub finished_at: i64,
}

/// 知识服务：L0 元数据库 + 后台扫描状态 + L1 知识库(共享同一连接)。Tauri managed state。
/// Clone 是句柄语义（内部全 Arc/连接池）：后台导入线程持 clone 补载模型并
/// 经 `install_embedder` 启动空闲巡检，与 managed state 共享同一份运行时状态。
#[derive(Clone)]
pub struct KnowledgeService {
    store: Store,
    l1: l1::L1Store,
    /// Serializes collection deletion with session mount mutations at the Tauri boundary.
    /// The database and SessionStore use separate locks, so this coordinator closes the
    /// validate-then-mount race without coupling either domain to the other.
    mount_mutation: Arc<tokio::sync::Mutex<()>>,
    scan_state: Arc<Mutex<ScanState>>,
    cancel: Arc<AtomicBool>,
    imports: import_jobs::ImportJobStore,
    active_import: Arc<Mutex<Option<String>>>,
    index_cancel: Arc<AtomicBool>,
    /// embedder 空闲卸载巡检任务句柄 + 防振荡时钟。模型常驻 ~570MB 内存；
    /// 巡检在空闲超阈值时 `set_embedder(None)` 卸载，kb_model_status 的热加载
    /// 钩子（用户意图）会自动重载。放 Option 使「未装模型 → 不起巡检」零开销。
    embedder_reaper: Arc<Mutex<Option<EmbedderReaper>>>,
    /// 上次自动卸载 embedding 模型的 UNIX 秒（防振荡）：距上次卸载不足
    /// EMBEDDER_UNLOAD_COOLDOWN_SECS 时不再自动卸载。放在服务级字段（而非
    /// 巡检任务内）是因为卸载/热加载会重启巡检任务，冷却必须跨任务存活。
    embedder_last_unload_epoch: Arc<AtomicI64>,
}

/// embedder 空闲卸载巡检任务句柄（Drop 即停）。
struct EmbedderReaper {
    #[allow(dead_code)]
    guard: EmbedderReaperGuard,
}

struct EmbedderReaperGuard {
    cancel: tokio_util::sync::CancellationToken,
    handle: tauri::async_runtime::JoinHandle<()>,
}

impl Drop for EmbedderReaperGuard {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.handle.abort();
    }
}

/// embedder 空闲卸载阈值：无检索/索引活动超过该时长 → 卸载模型释放 ~570MB。
/// 20 分钟取偏保守值：语义检索是聊天增强路径，卸载后下一次检索退化为纯全文，
/// 宁可多留一会儿也不让常用用户频繁掉级。
const EMBEDDER_IDLE_UNLOAD_AFTER_SECS: u64 = 20 * 60;
/// embedder 空闲卸载巡检间隔（与 AcpPool/EnginePool 空闲回收节奏一致）。
const EMBEDDER_REAP_INTERVAL_SECS: u64 = 5 * 60;
/// 卸载冷却：距上次自动卸载不足该时长时不再自动卸载。防「kb_model_status
/// 热加载（用户查状态=用户意图）→ 巡检立刻又卸载」的振荡循环；热加载视为
/// 用户意图，重载后空闲时钟与冷却都重新计时。
const EMBEDDER_UNLOAD_COOLDOWN_SECS: i64 = 10 * 60;

impl KnowledgeService {
    /// 只用磁盘库初始化（`~/.pinvou3/knowledge/index.db`）。embedding 模型必须在首帧后
    /// 通过后台 blocking 线程加载，避免读取/构建大型 ONNX 模型阻塞 Tauri setup 和首屏。
    pub fn new(db_path: &Path) -> rusqlite::Result<Self> {
        Self::open(db_path, true)
    }

    /// Like [`Self::new`], but a crashed import is NOT reconciled at boot.
    /// Read-only consumers (the headless CLI) must be able to open the store
    /// without degrading an import a live app process is still running:
    /// recovery flips that job to terminal state, which would wedge the
    /// owner's bookkeeping. Write paths keep [`Self::new`].
    pub fn new_without_recovery(db_path: &Path) -> rusqlite::Result<Self> {
        Self::open(db_path, false)
    }

    fn open(db_path: &Path, recover: bool) -> rusqlite::Result<Self> {
        let store = Store::open(db_path)?;
        let last_scan_finished_at = store.last_scan_finished_at().unwrap_or(0);
        let conn = store.conn_arc();
        let l1 = l1::L1Store::new(conn.clone());
        let imports = import_jobs::ImportJobStore::new(conn);
        let interrupted = if recover {
            imports.recover_interrupted()?
        } else {
            None
        };
        if let Some(job) = &interrupted {
            if job.resumable {
                l1.set_collection_status(job.collection_id, "pending");
            }
        }
        Ok(Self {
            store,
            l1,
            mount_mutation: Arc::new(tokio::sync::Mutex::new(())),
            scan_state: Arc::new(Mutex::new(ScanState {
                phase: if last_scan_finished_at > 0 {
                    "done".into()
                } else {
                    "idle".into()
                },
                finished_at: last_scan_finished_at,
                ..Default::default()
            })),
            cancel: Arc::new(AtomicBool::new(false)),
            imports,
            active_import: Arc::new(Mutex::new(None)),
            index_cancel: Arc::new(AtomicBool::new(false)),
            embedder_reaper: Arc::new(Mutex::new(None)),
            embedder_last_unload_epoch: Arc::new(AtomicI64::new(0)),
        })
    }

    /// L1 知识集句柄（命令层直接用）。
    pub fn l1(&self) -> &l1::L1Store {
        &self.l1
    }

    pub(crate) fn mount_mutation_coordinator(&self) -> Arc<tokio::sync::Mutex<()>> {
        self.mount_mutation.clone()
    }

    /// 语义检索是否就绪（embedding 模型已加载）。完全门控用：模型没装 → 知识库不可用。
    pub fn semantic_ready(&self) -> bool {
        self.l1.has_embedder()
    }

    /// 构建 embedding 模型。调用方必须把它放进 `spawn_blocking`，该过程会同步读取约
    /// 558 MiB 的 ONNX/Tokenizer 文件并创建推理会话。
    fn load_embedder(
        model_dir: Option<&Path>,
    ) -> Result<Arc<pinvou_knowledge::embedding::Embedder>, String> {
        crate::platform::os::configure_onnxruntime_dylib()?;
        pinvou_knowledge::embedding::Embedder::from_env_or_dir(model_dir).map(Arc::new)
    }

    /// 严格从调用方指定目录构建 embedding，不读取开发环境的模型目录覆盖。
    /// 下载修复必须使用该入口验证候选目录，避免验证了外部目录却替换托管目录。
    fn load_embedder_from_dir(
        model_dir: &Path,
    ) -> Result<Arc<pinvou_knowledge::embedding::Embedder>, String> {
        crate::platform::os::configure_onnxruntime_dylib()?;
        let name = std::env::var("PINVOU3_KB_EMBED_MODEL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| model_download::MODEL_VERSION.to_string());
        pinvou_knowledge::embedding::Embedder::from_dir(model_dir, &name)
            .map(Arc::new)
            .map_err(|error| format!("embedding 模型加载失败({}): {error}", model_dir.display()))
    }

    /// 将后台构建完成的模型原子换入共享槽；所有 L1Store clone 立即可见。
    /// 同时重置空闲时钟并确保巡检在跑：热加载（下载完成 / kb_model_status
    /// 状态查询 / 导入前补载）都视为用户意图，从加载时刻重新计空闲。
    fn install_embedder(&self, embedder: Arc<pinvou_knowledge::embedding::Embedder>) -> bool {
        eprintln!(
            "[knowledge] L1 embedding 已启用: {} ({})",
            embedder.model(),
            embedder.source()
        );
        self.l1.note_embed_activity();
        self.l1.set_embedder(Some(embedder));
        self.ensure_embedder_reaper();
        true
    }

    /// 启动 embedder 空闲卸载巡检（幂等）。巡检常驻直到服务释放：模型在场
    /// 才可能触发卸载，已卸载时每轮只做一次原子读，开销可忽略。
    fn ensure_embedder_reaper(&self) {
        let mut slot = self.embedder_reaper.lock();
        if slot.is_some() {
            return;
        }
        let cancel = tokio_util::sync::CancellationToken::new();
        let task_cancel = cancel.clone();
        let l1 = self.l1.clone();
        let last_unload = self.embedder_last_unload_epoch.clone();
        let handle = tauri::async_runtime::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_secs(EMBEDDER_REAP_INTERVAL_SECS));
            // tokio::interval 首个 tick 立即到期：跳过，统一走周期节奏。
            interval.tick().await;
            loop {
                tokio::select! {
                    _ = task_cancel.cancelled() => break,
                    _ = interval.tick() => {}
                }
                // 单轮 panic 隔离：巡检循环 panic 会让整个 async task 静默终止
                // （guard Drop 只在服务释放时触发），模型从此常驻不再卸载。风险
                // 本就低（条件读取 + 原子写），兜底只为「停摆不静默」。
                let tick_l1 = l1.clone();
                let tick_unload = last_unload.clone();
                if let Err(panic) = catch_unwind(AssertUnwindSafe(move || {
                    // 条件逐轮重读：模型可能已被热卸载/热加载。
                    let should_unload = tick_l1.has_embedder()
                        && tick_l1.embedder_idle_secs() >= EMBEDDER_IDLE_UNLOAD_AFTER_SECS
                        && now() - tick_unload.load(Ordering::Acquire)
                            >= EMBEDDER_UNLOAD_COOLDOWN_SECS;
                    if should_unload {
                        eprintln!(
                            "[knowledge] embedding 模型空闲超过 {} 秒，卸载释放内存（下次状态查询/导入自动重载）",
                            EMBEDDER_IDLE_UNLOAD_AFTER_SECS
                        );
                        // set_embedder(None) 后所有 L1Store clone 即刻回到纯全文检索；
                        // 正在跑的推理持有 Arc 副本，跑完自然释放。
                        tick_l1.set_embedder(None);
                        tick_unload.store(now(), Ordering::Release);
                    }
                })) {
                    eprintln!(
                        "[knowledge] embedding 空闲巡检单轮 panic（已隔离，下轮继续）: {panic:?}"
                    );
                }
            }
        });
        *slot = Some(EmbedderReaper {
            guard: EmbedderReaperGuard { cancel, handle },
        });
    }

    /// 后台索引入口的补载：模型被空闲卸载或被首帧门控跳过（deferred）后，导入
    /// 线程在向量化开始前同步重载（阻塞读 ONNX ~570MB，只在导入线程，不占 UI）。
    /// 索引需要向量化，模型缺位会让整个导入静默降级为纯全文（chunks.vec 全
    /// NULL，事后无法补）。必须经 `install_embedder` 落位：补载是首帧门控主干
    /// 路径（deferred→导入）上的**首次也是唯一**一次加载，绕过它会让空闲卸载
    /// 巡检永不启动，~570MB 常驻直到退出。
    fn reload_embedder_if_import_needed(&self) {
        self.reload_embedder_if_import_needed_with(model_download::model_installed(), || {
            Self::load_embedder(Some(&model_dir()))
        });
    }

    /// 上一方法的可测核心：`model_installed` 与加载器注入，便于断言门控与
    /// 失败路径不落 embedder、不误起巡检。成功路径经 install_embedder 语义
    /// 由既有 KbModelStatus/deferred 契约测试覆盖。
    fn reload_embedder_if_import_needed_with(
        &self,
        model_installed: bool,
        load: impl FnOnce() -> Result<Arc<pinvou_knowledge::embedding::Embedder>, String>,
    ) {
        if self.l1.has_embedder() || !model_installed {
            return;
        }
        // 门控放行 = 即将真实加载:清除「故意延迟」标记,此后的失败按真实失败
        // 上报,不被 deferred 状态掩盖(与 kb_model_status 热加载同语义)。
        model_download::clear_deferred_no_usage();
        match load() {
            Ok(embedder) => {
                self.install_embedder(embedder);
                eprintln!("[knowledge] 导入前重载 embedding 模型完成（向量化解锁）");
            }
            Err(error) => {
                // 与首帧加载同语义：加载失败保持全文降级，不阻断导入。
                eprintln!("[knowledge] 导入前重载 embedding 模型失败（降级仅全文）: {error}");
            }
        }
    }

    /// 知识库是否有任何已入库内容（任一知识集存在文档）。门控 kb_search/kb_open_source
    /// 工具的对外可见性：
    /// 为空时把工具加入引擎 disallowed → 模型看不到，AI 不再宣称「能本地知识库检索」。
    /// 读失败保守按「无内容」处理（宁可隐藏也不误宣传能力）。
    pub fn has_indexed_content(&self) -> bool {
        self.l1.has_any_document().unwrap_or(false)
    }

    /// kb 工具可见性判定（lib.rs tool_policy 的单一真相源）：
    /// **只看有没有内容，不看 embedding 模型在位状态**。模型可能被空闲卸载/首帧
    /// 门控跳过，可见性若随之波动，快照式重算会把 kb_search 写进 disallowed，
    /// 新 spawn 的 engine 看不到工具，工具内的按需重载自愈路径反而不可达——
    /// 这是审阅 blocker 的修复语义，回归测试锚定在 model_download.rs
    /// （`kb_tools_stay_visible_after_model_unload_when_content_present`）。
    /// 模型缺位由 kb_search 执行时走租约化重载兜底（失败降级纯全文，不阻断检索）。
    pub fn kb_tools_usable(indexed_content: bool, remote_connections: bool) -> bool {
        indexed_content || remote_connections
    }
    pub fn index_status(&self) -> IndexState {
        self.imports
            .latest_state()
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    /// **指定任务**的导入状态（不是「最新任务」）。
    ///
    /// [`Self::index_status`] 走 `ImportJobStore::latest_state` 的优先级排序
    /// （preparing/running → interrupted → done_with_errors → 其余，再按
    /// updated_at 倒序）：`cancelled` 落在最后一档，任何更早的
    /// done_with_errors / interrupted 任务都会反超它。GUI 不受影响——它只轮询
    /// 「当前最该给用户看的任务」，这正是该排序的设计意图；但无头 CLI 的
    /// `index cancel <job-id>` / `index status <job-id>` 报的是**点名的那个
    /// 任务**，取消之后再读 latest 会串到另一个任务的状态上（人类行说取消了
    /// B，JSON 体却是 A）。与 `resume_index`/`retry_index_item` 收尾时的
    /// `imports.state(&job_id)` 同一入口、同一语义。
    ///
    /// 任务不存在按错误上报（底层是 `QueryReturnedNoRows`），调用方据此区分
    /// 「任务没了」与「任务在但状态是空」。
    pub fn index_job_state(&self, job_id: &str) -> Result<IndexState, String> {
        self.imports.state(job_id).map_err(|e| e.to_string())
    }

    pub fn cancel_index(&self) -> Result<(), String> {
        let job_id = self
            .active_import
            .lock()
            .clone()
            .or_else(|| self.index_status().job_id);
        if let Some(job_id) = job_id {
            let st = self.imports.state(&job_id).map_err(|e| e.to_string())?;
            if st.running || st.resumable {
                self.imports.cancel(&job_id).map_err(|e| e.to_string())?;
                self.index_cancel.store(true, Ordering::Relaxed);
                self.l1.set_collection_status(st.collection_id, "ready");
            }
        }
        Ok(())
    }

    pub fn cancel_index_for_collection(&self, collection_id: i64) -> Result<(), String> {
        let status = self.index_status();
        if status.collection_id == collection_id && (status.running || status.resumable) {
            self.cancel_index()?;
        }
        Ok(())
    }

    /// 将**点名**的导入任务退回 `interrupted`（可续跑）。CLI 等待超时路径的收口
    /// 入口：导入线程已死/卡住时把任务留在 interrupted（而非 running），下次
    /// `resume_index` 无需等桌面端启动恢复即可续跑。与 [`Self::cancel_index`]
    /// 的防御查表同构——先点名读状态，未知任务按错误上报（而不是静默 no-op）；
    /// 状态迁移完全委托 `ImportJobStore::interrupt`：仅对 preparing/running
    /// 生效（SQL WHERE 兜底），已完结/interrupted 的任务是幂等 no-op。
    pub fn interrupt_index(&self, job_id: &str) -> Result<(), String> {
        let state = self.imports.state(job_id).map_err(|e| e.to_string())?;
        if state.running {
            self.imports.interrupt(job_id);
        }
        Ok(())
    }

    pub fn failed_index_files(
        &self,
        job_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<FailedImportFilePage, String> {
        self.imports
            .failed_files_page(job_id, offset, limit)
            .map_err(|e| e.to_string())
    }

    /// 后台把若干路径(文件或目录)加入知识集：先持久化任务，再展开目录→解析→切块→入库。
    pub fn start_index(&self, collection_id: i64, roots: Vec<PathBuf>) -> IndexState {
        let mut active = self.active_import.lock();
        if active.is_some() {
            return self.index_status();
        }
        if let Ok(Some(previous)) = self.imports.latest_state() {
            if previous.resumable {
                return previous;
            }
        }
        let job_id = match self.imports.create(collection_id, &roots) {
            Ok(id) => id,
            Err(_) => return self.index_status(),
        };
        *active = Some(job_id.clone());
        drop(active);
        self.launch_import(job_id.clone());
        self.imports
            .state(&job_id)
            .unwrap_or_else(|_| self.index_status())
    }

    pub fn resume_index(&self, job_id: String) -> Result<IndexState, String> {
        let mut active = self.active_import.lock();
        if active.is_some() {
            return Err("已有知识集导入任务正在运行".into());
        }
        self.imports.resume(&job_id).map_err(|e| e.to_string())?;
        *active = Some(job_id.clone());
        drop(active);
        self.launch_import(job_id.clone());
        self.imports.state(&job_id).map_err(|e| e.to_string())
    }

    pub fn retry_index_item(&self, job_id: String, item_id: i64) -> Result<IndexState, String> {
        let mut active = self.active_import.lock();
        if active.is_some() {
            return Err("已有知识集导入任务正在运行".into());
        }
        self.imports
            .retry_item(&job_id, item_id)
            .map_err(|e| e.to_string())?;
        *active = Some(job_id.clone());
        drop(active);
        self.launch_import(job_id.clone());
        self.imports.state(&job_id).map_err(|e| e.to_string())
    }

    fn launch_import(&self, job_id: String) {
        self.index_cancel.store(false, Ordering::Relaxed);
        let imports = self.imports.clone();
        // 服务句柄（Clone 共享同一份 embedder 槽/巡检状态）：导入线程用它补载
        // 模型，必须经 install_embedder 语义落位，否则空闲卸载巡检不会启动。
        let service = self.clone();
        let cancel = self.index_cancel.clone();
        let active = self.active_import.clone();
        // panic 兜底需要一份不被闭包 move 走的句柄，否则 panic 后无法清理。
        let panic_imports = imports.clone();
        let panic_active = active.clone();
        let panic_job_id = job_id.clone();
        thread::spawn(move || {
            // 导入线程处理任意用户文件（PDF/Office/图片 OCR 等），底层解析可能 panic。
            // 进程死亡已由启动时的 recover_interrupted 兜底，但进程内线程 panic 不会
            // 触发它：若不在此兜住，active_import 会永久卡在已死的任务上，直到完全重启。
            let outcome = catch_unwind(AssertUnwindSafe(move || {
                // 模型可能已被空闲卸载或被首帧门控跳过：索引需要向量化，模型缺位
                // 会让整个导入静默降级为纯全文（chunks.vec 全 NULL，事后无法补），
                // 先经 install_embedder 补载。
                service.reload_embedder_if_import_needed();
                let l1 = service.l1().clone();
                let state = match imports.state(&job_id) {
                    Ok(v) => v,
                    Err(_) => {
                        imports.interrupt(&job_id);
                        *active.lock() = None;
                        return;
                    }
                };
                l1.set_collection_status(state.collection_id, "indexing");
                let mut infrastructure_error = false;
                let prepare_result = imports.item_count(&job_id).and_then(|count| {
                    if count > 0 {
                        return Ok(());
                    }
                    let roots = imports.roots(&job_id)?;
                    let files = expand_import_roots(&roots, &cancel);
                    imports.prepare_items(&job_id, &files)
                });
                if prepare_result.is_err() {
                    imports.interrupt(&job_id);
                    infrastructure_error = true;
                }
                loop {
                    if infrastructure_error
                        || cancel.load(Ordering::Relaxed)
                        || imports.is_cancelled(&job_id)
                    {
                        break;
                    }
                    let item = match imports.claim_next(&job_id) {
                        Ok(Some(v)) => v,
                        Ok(None) => break,
                        Err(_) => {
                            imports.interrupt(&job_id);
                            infrastructure_error = true;
                            break;
                        }
                    };
                    match l1.ingest_import_item(
                        &job_id,
                        item.id,
                        state.collection_id,
                        &item.path,
                        &cancel,
                    ) {
                        l1::ImportIngestOutcome::Completed | l1::ImportIngestOutcome::Skipped => {}
                        l1::ImportIngestOutcome::Cancelled => break,
                        l1::ImportIngestOutcome::Failed(error) => {
                            imports.mark_failed(&job_id, item.id, &error);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(3));
                }
                if !infrastructure_error {
                    let _ = imports.finish(&job_id);
                }
                let pending = imports
                    .state(&job_id)
                    .map(|s| s.resumable)
                    .unwrap_or(infrastructure_error);
                l1.set_collection_status(
                    state.collection_id,
                    if pending { "pending" } else { "ready" },
                );
                let mut current = active.lock();
                if current.as_deref() == Some(job_id.as_str()) {
                    *current = None;
                }
            }));
            if outcome.is_err() {
                // panic 与正常退出走同样的中断+清理：把任务退回 interrupted，清空 active_import，
                // 下次启动（或用户续作）仍可恢复，导入子系统不会卡死。
                panic_imports.interrupt(&panic_job_id);
                let mut current = panic_active.lock();
                if current.as_deref() == Some(panic_job_id.as_str()) {
                    *current = None;
                }
            }
        });
    }

    /// 启动一轮增量扫描（后台线程，立即返回；已在跑则原样返回当前状态）。**懒触发**：由前端
    /// 进入文件管理页时调，不进页 = 零扫描。不再常驻 watcher / 周期重扫——文件管理是低频功能，
    /// 不该长期占资源。增量只处理 mtime/size 变化的文件，进页时前端先用缓存秒显、扫完再刷新。
    pub fn start_scan(&self, roots: Vec<PathBuf>) -> ScanState {
        {
            let mut st = self.scan_state.lock();
            if st.running {
                return st.clone();
            }
            self.cancel.store(false, Ordering::Relaxed);
            *st = ScanState {
                running: true,
                phase: "scanning".into(),
                roots: roots.iter().map(|p| p.display().to_string()).collect(),
                ..Default::default()
            };
        }

        let store = self.store.clone();
        let scan_state = self.scan_state.clone();
        let cancel = self.cancel.clone();
        // panic 兜底需要一份不被闭包 move 走的状态句柄，否则 panic 后无法收口。
        let panic_scan_state = self.scan_state.clone();

        thread::spawn(move || {
            // 扫描线程 panic 兜底（与 `launch_import` 的导入线程同语义）：running
            // 只在下面的正常收尾处清零，而 `scan_state` 是 parking_lot::Mutex——
            // 不会中毒，panic 之后锁照常可取，状态却永远停在 running:true。GUI 只是
            // 进度条不再前进（懒触发，下次进页重扫），但 `pinvou knowledge scan
            // start` 是唯一**阻塞等待**该标志的调用方，会无输出、无退出码地挂死。
            let outcome = catch_unwind(AssertUnwindSafe(move || {
                let ex = Excluder::default();
                // 增量：载入现有快照，scanner 只写 mtime/size 变化的文件，未变的跳过。
                let existing = store.load_index().unwrap_or_default();
                let mut visited = std::collections::HashSet::new();
                let mut scanned_total = 0u64;
                // 删除授权只按**真正遍历过**的根给，不按**请求**的根给（见
                // `root_authorizes_deletion`）：scanner::scan 对走不动的根静默返回 0，
                // 而清理阶段照跑，会把该根下整片索引当作「已消失」删光。
                let mut swept_roots: Vec<PathBuf> = Vec::with_capacity(roots.len());
                for root in &roots {
                    let base = scanned_total;
                    let walked =
                        scanner::scan(root, &store, &ex, &cancel, &existing, &mut visited, |n| {
                            scan_state.lock().scanned = base + n;
                        });
                    scanned_total = base + walked;
                    scan_state.lock().scanned = scanned_total;
                    if root_authorizes_deletion(root, walked) {
                        swept_roots.push(root.clone());
                    }
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                }

                // 清理「已消失」的文件（上次在库、本次没遍历到）。取消时不删，避免误删没扫完的部分。
                if !cancel.load(Ordering::Relaxed) {
                    let stale = stale_entries(&existing, &visited, &swept_roots);
                    if !stale.is_empty() {
                        let _ = store.delete_many(&stale);
                    }
                }

                // 去重(算 hash)不在扫描里跑——读盘昂贵、百万文件下永远跑不完且拖卡设备。去重功能已下线。
                let cancelled = cancel.load(Ordering::Relaxed);
                let finished_at = now();
                if !cancelled {
                    let _ = store.set_last_scan_finished_at(finished_at);
                }
                let mut st = scan_state.lock();
                st.running = false;
                st.finished_at = finished_at;
                st.phase = if cancelled { "cancelled" } else { "done" }.into();
            }));
            if let Err(panic) = outcome {
                // 与空闲巡检的单轮兜底同样「停摆不静默」：不打印的话，扫描线程
                // 死掉只表现为「进度条停了」，无从定位。
                eprintln!(
                    "[knowledge] 扫描线程 panic（已兜底收口，本轮不做已消失清理）: {panic:?}"
                );
                finish_scan_after_panic(&panic_scan_state);
            }
        });

        self.scan_state.lock().clone()
    }

    /// Request cancellation of an in-progress scan. The GUI's lazy scan has
    /// no frontend cancel entry, and the intended consumer is a
    /// `knowledge scan cancel` subcommand in the stacked CLI families PR —
    /// which does not exist in this tree (the in-tree `pinvou-cli` exposes
    /// `benchmark` and `agent` only), so this is pre-landed surface, not a
    /// wired-up entry point. In-process one-shot signal: the scan thread's
    /// cancel branch wraps up early on it (semantics unchanged).
    pub fn cancel_scan(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn status(&self) -> ScanState {
        self.scan_state.lock().clone()
    }

    // ───────────────────── headless (CLI) read entry points ─────────────────────
    //
    // Same semantics as the Tauri commands below (kb_stats / kb_type_counts /
    // kb_search), but synchronous: `spawn_db` exists to move blocking queries
    // off the Tauri **main thread** (synchronous commands run there, and a
    // full-table COUNT on a large library freezes the UI). A headless caller
    // (the one-shot pinvou-cli process) runs outside the async runtime / UI
    // main thread, so querying the store directly needs no runtime dependency
    // and must not introduce one.

    /// L0 index overview (same semantics as `kb_stats`).
    pub fn stats(&self) -> Result<Stats, String> {
        self.store.stats().map_err(|e| e.to_string())
    }

    /// L0: per-extension counts (same semantics as `kb_type_counts`).
    pub fn type_counts(&self) -> Result<Vec<TypeCount>, String> {
        self.store.type_counts().map_err(|e| e.to_string())
    }

    /// Instant search (same semantics as `kb_search`, including the NL-rule
    /// merge through `merge_nl_rules`).
    pub fn search(&self, query: SearchQueryDto) -> Result<Vec<FileHit>, String> {
        let sq = merge_nl_rules(query.into());
        self.store.search(&sq).map_err(|e| e.to_string())
    }
}

/// 后台索引入口的补载实现已上收到 `KnowledgeService::
/// reload_embedder_if_import_needed`（导入线程持有服务句柄，补载必须经
/// install_embedder 启动空闲巡检，自由函数直写 l1 槽会绕过巡检启动）。
/// 剪枝遍历复用 `scanner::walk_pruned`（与全盘扫描同一排除语义）。
fn expand_import_roots(roots: &[PathBuf], cancel: &AtomicBool) -> Vec<PathBuf> {
    let ex = Excluder::default();
    let mut files = Vec::new();
    for root in roots {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if root.is_file() {
            files.push(root.clone());
            continue;
        }
        for entry in scanner::walk_pruned(root, &ex) {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            if entry.file_type().is_file() {
                files.push(entry.path().to_path_buf());
            }
        }
    }
    import_jobs::unique_existing_files(files)
}

/// `~/.pinvou3/knowledge/index.db`。
pub fn default_db_path() -> PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("knowledge")
        .join("index.db")
}

/// embedding 模型按需下载落点：`~/.pinvou3/knowledge/models/bge-m3`。
/// 模型不再随 deb 打包（deb 瘦 ~559MB）；用户在知识库页主动下载部署到此目录后才启用语义检索。
/// dev 仍可用 env `PINVOU3_KB_EMBED_MODEL_DIR` 覆盖（见 pinvou_knowledge::embedding::Embedder::from_env_or_dir）。
pub fn model_dir() -> PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("knowledge")
        .join("models")
        .join("bge-m3")
}

/// 扫描线程 panic 后的状态收口：把 running 放掉，相位落到 `interrupted`。
///
/// 独立相位而不是复用 done/cancelled：既没扫完（`last_scan_finished_at` 不落库，下次
/// 进页仍按「没扫过」重扫；内存里的 finished_at 只记中止时刻），也不是用户取消。前端
/// 只在 `done` 时刷新 L0，`interrupted` 因此不会被误当成成功；阻塞等待的 CLI 则据此
/// 报错而不是宣布扫描完成。
fn finish_scan_after_panic(scan_state: &Mutex<ScanState>) {
    let mut st = scan_state.lock();
    st.running = false;
    st.finished_at = now();
    st.phase = "interrupted".into();
}

/// 本轮扫描是否有资格在 `root` 之内执行「已消失」删除。
///
/// `scanner::scan` 对**走不动**的根静默返回 0（遍历错误在 `walk_pruned` 里被跳过）：
/// 挂载点掉了、权限没了、根在预检之后被删掉，都只表现为「遍历到 0 个条目」。此时把该根
/// 交给 [`stale_entries`] 当作删除边界，等于把它下面的整片索引判成已消失——`scan start
/// --root /mnt/usb` 在掉盘后会删光 `/mnt/usb` 下的每一条，还报 `phase: done`、
/// `scanned: 0`。
///
/// 两种「遍历到 0」必须分开：
/// - **走到了、里面是空的**（根仍是可读目录）：用户确实把里面删光了，这一片就该扫掉，
///   否则库里的幽灵条目永远清不掉——这是删除功能存在的理由，不能为了安全把它关掉。
/// - **走不动**（根不存在 / 不是目录 / 读不动）：库里该根下条目的存亡无从判断，本轮就
///   不该替它做决定，跳过即可（下一轮根恢复了自然会清）。
///
/// 残留的模糊地带只有一种：根是静态挂载点，卸载后仍留下一个可读的空目录——它与「用户
/// 把目录删空」在文件系统层面完全同形，任何纯路径探测都分不开。这种情况按「空目录」
/// 处理（即照旧清理），与掉盘会连挂载点一起消失/读不动的常见形态（udisks 自动挂载、
/// 设备拔出后的 ESTALE/EIO、权限回收）相比是少数，且它至少不会静默：删除只发生在
/// 用户点名的那个根之内。
fn root_authorizes_deletion(root: &Path, walked: u64) -> bool {
    walked > 0 || std::fs::read_dir(root).is_ok()
}

/// 计算本轮该删除的「已消失」条目：**只在本次真正遍历过的根之内**判定。
///
/// 旧逻辑把「库里有、本次没遍历到」一律当作已消失删掉。GUI 下这没问题——它永远只扫用户
/// 家目录这一个根（`kb_start_scan(roots: null)`），库里每一条都在该根之下，所以本函数对
/// GUI 的结果与旧逻辑逐条相同：家目录下的条目照旧按 visited 判定，根外条目本就不存在。
/// 但 CLI 的 `knowledge scan start --root <DIR>` 允许扫任意目录：扫 B 目录时，从 A 目录
/// 索引进来的条目会被整片误判为 stale，于是静默清库还报 `done`。删除的前提因此是该条目
/// 落在本次扫过的某个根之内——根外条目从来不在本轮扫描的职责范围里，无从判断其存亡。
///
/// `roots` 是**真正遍历过**的根（[`root_authorizes_deletion`] 过滤后的），不是调用方
/// 请求的根：请求与遍历之间隔着一次 TOCTOU，请求的根走不动时把它当边界就会删光整片。
///
/// 包含关系用 [`Path::starts_with`] 按**路径分量**比较，不能用字符串前缀：`/home/a` 不得
/// 匹配 `/home/abc`。
fn stale_entries(
    existing: &std::collections::HashMap<String, (i64, u64)>,
    visited: &std::collections::HashSet<String>,
    roots: &[PathBuf],
) -> Vec<String> {
    // 每个根只 canonicalize 一次（失败则退回原样，例如根在判定前被删掉）。原样与规范化
    // 两种形态都参与比较：库里的键既可能是按原样根写入的，也可能是按规范化根写入的。
    let bounds: Vec<(PathBuf, PathBuf)> = roots
        .iter()
        .map(|root| {
            let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
            (root.clone(), canonical)
        })
        .collect();
    existing
        .keys()
        .filter(|path| !visited.contains(*path))
        .filter(|path| within_scanned_roots(Path::new(path.as_str()), &bounds))
        .cloned()
        .collect()
}

/// 路径是否落在本次扫过的某个根之内（含根自身）。
///
/// 先做**纯内存**的原样比较并短路：GUI 只有家目录一个根，库里每一条都在这一步命中，
/// 整个清理阶段因此退回旧版 HashSet 差集的开销。反过来若每条都先 canonicalize，一次
/// 大目录删除后的几万条已消失条目会各打一次注定失败的系统调用，全压在扫描线程上。
///
/// canonicalize 只作兜底：被扫的根是软链、或库里的键本身是相对/软链形态时，只有规范化
/// 之后才判得出包含关系。库里的路径多半已经不在盘上（这正是 stale 的常态），对它
/// canonicalize 必然失败，此时按「不在边界内」处理——不授权删除，语义与旧版一致。
fn within_scanned_roots(path: &Path, bounds: &[(PathBuf, PathBuf)]) -> bool {
    if bounds.iter().any(|(raw_root, canonical_root)| {
        path.starts_with(raw_root) || path.starts_with(canonical_root)
    }) {
        return true;
    }
    let Ok(canonical) = std::fs::canonicalize(path) else {
        return false;
    };
    bounds.iter().any(|(raw_root, canonical_root)| {
        canonical.starts_with(raw_root) || canonical.starts_with(canonical_root)
    })
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

// ───────────────────────── Tauri 命令层 ─────────────────────────

/// 前端搜索条件（camelCase）。空 text + 各过滤可组合。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchQueryDto {
    pub text: Option<String>,
    #[serde(default)]
    pub exts: Vec<String>,
    pub mtime_after: Option<i64>,
    pub mtime_before: Option<i64>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    #[serde(default)]
    pub limit: usize,
}

impl From<SearchQueryDto> for SearchQuery {
    fn from(d: SearchQueryDto) -> Self {
        SearchQuery {
            text: d.text,
            exts: d.exts,
            mtime_after: d.mtime_after,
            mtime_before: d.mtime_before,
            min_size: d.min_size,
            max_size: d.max_size,
            limit: d.limit,
        }
    }
}

/// 把阻塞的 DB 查询挪出主线程执行。Tauri 同步命令(`fn`)在**主线程**跑，大库(百万行)
/// 全表 COUNT/GROUP BY 会冻死整个 UI——慢查询命令一律改 `async fn` + 本 helper。
async fn spawn_db<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("db task join: {e}"))?
}

/// 启动/续跑全盘扫描。`roots` 省略时默认用户家目录。
pub fn kb_start_scan(state: State<'_, KnowledgeService>, roots: Option<Vec<String>>) -> ScanState {
    let roots = roots
        .filter(|v| !v.is_empty())
        .map(|v| v.into_iter().map(PathBuf::from).collect())
        .unwrap_or_else(|| vec![crate::platform::paths::user_home_dir()]);
    state.start_scan(roots)
}
pub fn kb_scan_status(state: State<'_, KnowledgeService>) -> ScanState {
    state.status()
}

/// L0：按扩展名分类计数（文件管理「按类型浏览」用）。
pub async fn kb_type_counts(state: State<'_, KnowledgeService>) -> Result<Vec<TypeCount>, String> {
    let store = state.store.clone();
    spawn_db(move || store.type_counts().map_err(|e| e.to_string())).await
}

// ───────────────────────── L1 知识库命令 ─────────────────────────
pub async fn kb_collection_list(
    state: State<'_, KnowledgeService>,
) -> Result<Vec<Collection>, String> {
    let l1 = state.l1().clone();
    spawn_db(move || l1.list_collections().map_err(|e| e.to_string())).await
}
pub async fn kb_collection_create(
    state: State<'_, KnowledgeService>,
    name: String,
    category: Option<String>,
    description: Option<String>,
) -> Result<i64, String> {
    let l1 = state.l1().clone();
    spawn_db(move || {
        l1.create_collection(&name, category.as_deref(), description.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
}
pub async fn kb_collection_update(
    state: State<'_, KnowledgeService>,
    id: i64,
    name: String,
    category: Option<String>,
    description: Option<String>,
) -> Result<(), String> {
    let l1 = state.l1().clone();
    spawn_db(move || {
        l1.update_collection(id, &name, category.as_deref(), description.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
}
pub async fn kb_collection_delete(
    state: State<'_, KnowledgeService>,
    pool: State<'_, crate::features::assistant::engine_pool::EnginePool>,
    id: i64,
) -> Result<(), String> {
    state.cancel_index_for_collection(id)?;
    let l1 = state.l1().clone();
    spawn_db(move || l1.delete_collection(id).map_err(|e| e.to_string())).await?;
    refresh_kb_tool_gate(&pool).await;
    Ok(())
}

/// 删文档/知识集后重算工具门控:若库已空,kb_search/kb_open_source 进 disallowed 并广播给所有在跑会话 →
/// 实时从模型目录消失。加文件后重新出现走新会话即可(老会话实时性次要)。
async fn refresh_kb_tool_gate(pool: &crate::features::assistant::engine_pool::EnginePool) {
    pool.refresh_disallowed_tools().await;
}

/// 把文件/目录加入知识集，后台解析+切块+入库。进度走 kb_index_status。
pub fn kb_collection_add_sources(
    state: State<'_, KnowledgeService>,
    collection_id: i64,
    paths: Vec<String>,
) -> IndexState {
    let roots = paths.into_iter().map(PathBuf::from).collect();
    state.start_index(collection_id, roots)
}
pub fn kb_index_status(state: State<'_, KnowledgeService>) -> IndexState {
    state.index_status()
}
pub fn kb_index_cancel(state: State<'_, KnowledgeService>) -> Result<(), String> {
    state.cancel_index()
}
pub fn kb_index_failed_files(
    state: State<'_, KnowledgeService>,
    job_id: String,
    offset: usize,
    limit: usize,
) -> Result<FailedImportFilePage, String> {
    state.failed_index_files(&job_id, offset, limit)
}
pub fn kb_index_resume(
    state: State<'_, KnowledgeService>,
    job_id: String,
) -> Result<IndexState, String> {
    state.resume_index(job_id)
}
pub fn kb_index_retry_file(
    state: State<'_, KnowledgeService>,
    job_id: String,
    item_id: i64,
) -> Result<IndexState, String> {
    state.retry_index_item(job_id, item_id)
}

/// 列出知识集文档（collectionId<=0 列出全部知识集，给「知识库内文件」表）。
pub async fn kb_documents(
    state: State<'_, KnowledgeService>,
    collection_id: i64,
    limit: Option<usize>,
) -> Result<Vec<Document>, String> {
    let l1 = state.l1().clone();
    spawn_db(move || {
        l1.list_documents(collection_id, limit.unwrap_or(0))
            .map_err(|e| e.to_string())
    })
    .await
}
pub async fn kb_remove_document(
    state: State<'_, KnowledgeService>,
    pool: State<'_, crate::features::assistant::engine_pool::EnginePool>,
    doc_id: i64,
) -> Result<(), String> {
    let l1 = state.l1().clone();
    spawn_db(move || l1.remove_document(doc_id).map_err(|e| e.to_string())).await?;
    refresh_kb_tool_gate(&pool).await;
    Ok(())
}

/// 语义检索(embedding)状态，给前端显示「语义检索:已启用/未配置」。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbedInfo {
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
}
pub fn kb_embed_info(state: State<'_, KnowledgeService>) -> EmbedInfo {
    match state.l1().embed_info() {
        Some((base_url, model)) => EmbedInfo {
            enabled: true,
            base_url,
            model,
        },
        None => EmbedInfo {
            enabled: false,
            base_url: String::new(),
            model: String::new(),
        },
    }
}

/// Core of the NL-rule merge shared by `kb_search` and the headless
/// [`KnowledgeService::search`] so both surfaces keep identical semantics:
/// the text runs through NL-rule parsing ("上周的 pdf" → exts + time filter +
/// residual text); structured filters passed **explicitly** by the caller
/// win over the parsed result and are never overwritten.
fn merge_nl_rules(mut sq: SearchQuery) -> SearchQuery {
    if let Some(text) = sq.text.clone() {
        let parsed = query::parse(&text);
        sq.text = parsed.text; // residual text (time/size/type words stripped)
        if sq.exts.is_empty() {
            sq.exts = parsed.exts;
        }
        if sq.mtime_after.is_none() {
            sq.mtime_after = parsed.mtime_after;
        }
        if sq.min_size.is_none() {
            sq.min_size = parsed.min_size;
        }
        if sq.max_size.is_none() {
            sq.max_size = parsed.max_size;
        }
    }
    sq
}

/// Instant search. The text first runs through NL-rule parsing ("上周的 pdf"
/// → exts + time filter + residual text); structured filters passed
/// **explicitly** by the frontend take precedence over the parsed result and
/// are not overwritten.
pub async fn kb_search(
    state: State<'_, KnowledgeService>,
    query: SearchQueryDto,
) -> Result<Vec<FileHit>, String> {
    let sq = merge_nl_rules(query.into());
    let store = state.store.clone();
    spawn_db(move || store.search(&sq).map_err(|e| e.to_string())).await
}
pub async fn kb_stats(state: State<'_, KnowledgeService>) -> Result<Stats, String> {
    let store = state.store.clone();
    spawn_db(move || store.stats().map_err(|e| e.to_string())).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> KnowledgeService {
        // The path must be unique per call: line!() expands to the same value
        // for every caller inside this helper, so parallel tests would share
        // one SQLite file — Store::open's staleness detection concurrently
        // reading a mid-state user_version deletes and recreates the whole
        // store, and another test then hits "no such table". This caused a
        // real flake (reproduced in the 2026-09-12 full parallel run).
        static SERVICE_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pinvou3_kb_import_reload_{}_{}",
            std::process::id(),
            SERVICE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        KnowledgeService::new(&dir.join("index.db")).expect("KnowledgeService::new")
    }

    /// 导入补载的门控契约：模型未安装或已在位时不发起加载（加载器注入计数
    /// 验证门控短路），加载失败不落 embedder、不误起空闲巡检（补载失败走
    /// 纯全文降级，起巡检只会空转）。
    #[test]
    fn import_reload_skips_when_installed_or_ready_and_survives_load_failure() {
        let svc = service();
        let mut calls = 0;
        svc.reload_embedder_if_import_needed_with(false, || {
            calls += 1;
            unreachable!("模型未安装不应触发加载")
        });
        assert_eq!(calls, 0);
        assert!(!svc.semantic_ready());

        let svc = service();
        svc.reload_embedder_if_import_needed_with(true, || Err("模拟加载失败".into()));
        assert!(!svc.semantic_ready(), "加载失败不得落入 embedder 槽");
        assert!(
            svc.embedder_reaper.lock().is_none(),
            "加载失败不应启动空闲巡检"
        );

        let svc = service();
        svc.reload_embedder_if_import_needed_with(true, || Err("再次失败".into()));
        assert!(!svc.semantic_ready(), "失败后再次导入仍应重试补载");
    }

    /// 点名任务的状态必须来自**那个任务**，不能退回 `latest_state` 的优先级
    /// 排序。回归场景：老任务 A 以 done_with_errors 收尾（排序第 2 档），新任务
    /// B 被取消后落到最后一档，A 于是反超 B——CLI 的 `index cancel B` 人类行说
    /// 「取消了 B」，JSON 体却是 A 的状态、jobId 也是 A。
    #[test]
    fn index_job_state_reports_the_named_job_not_the_outranking_latest_one() {
        let svc = service();
        let older = svc
            .l1
            .create_collection("older", None, None)
            .expect("older collection");
        let newer = svc
            .l1
            .create_collection("newer", None, None)
            .expect("newer collection");

        // 老任务：一个失败项后收尾 → done_with_errors（排序第 2 档）。
        let old_job = svc.imports.create(older, &[]).expect("older job");
        svc.imports
            .prepare_items(&old_job, &[PathBuf::from("/nonexistent/older.txt")])
            .expect("prepare older item");
        let item = svc
            .imports
            .claim_next(&old_job)
            .expect("claim older item")
            .expect("one pending item");
        svc.imports
            .mark_failed(&old_job, item.id, "fixture failure");
        svc.imports.finish(&old_job).expect("finish older job");

        // 新任务：刚创建时是 preparing（排序第 0 档），所以它才是 latest。
        let new_job = svc.imports.create(newer, &[]).expect("newer job");
        assert_eq!(
            svc.index_status().job_id.as_deref(),
            Some(new_job.as_str()),
            "running/preparing 的新任务必须压过老的 done_with_errors"
        );

        svc.imports.cancel(&new_job).expect("cancel newer job");
        // 取消后 latest 串到老任务上——这正是被修复的错报来源，锚定住它，
        // 免得将来有人以为 index_status() 在这里也够用。
        assert_eq!(
            svc.index_status().job_id.as_deref(),
            Some(old_job.as_str()),
            "cancelled 落到最后一档，latest_state 会被老的 done_with_errors 反超"
        );

        let named = svc
            .index_job_state(&new_job)
            .expect("被取消的任务必须仍可按 id 读到");
        assert_eq!(named.job_id.as_deref(), Some(new_job.as_str()));
        assert_eq!(named.collection_id, newer);
        assert!(named.cancelled, "点名读到的必须是取消后的状态");
        assert!(!named.running && !named.resumable);
        assert!(
            svc.index_job_state("kb-import-does-not-exist").is_err(),
            "不存在的任务必须报错，不能退化成另一个任务的状态"
        );
    }

    /// 清理「已消失」条目的删除边界只能是**真正遍历过**的根。
    ///
    /// `scanner::scan` 对走不动的根静默返回 0（挂载掉了/权限没了/根被删），若仍
    /// 把它当边界，该根下的整片索引会被当作已消失删光，还报 `done/scanned:0`。
    /// 同时必须保住「用户真把目录删空了」这条合法路径：根还在、是空的，照删。
    #[test]
    fn stale_sweep_bounds_exclude_a_root_that_could_not_be_walked() {
        let unique = format!("{}_{}", std::process::id(), now());
        let walkable = std::env::temp_dir().join(format!("pinvou3_kb_stale_bounds_{unique}"));
        std::fs::create_dir_all(&walkable).expect("create walkable root");
        // 兄弟目录而非子目录：子目录会被 `starts_with(walkable)` 顺带命中，
        // 测不出「走不动的根不进边界」。该路径从不创建 = 走不动的根。
        let vanished = std::env::temp_dir().join(format!("pinvou3_kb_stale_gone_{unique}"));

        // 走到了（walked>0）→ 授权；走到 0 但根仍是可读目录（用户删空了）→ 授权；
        // 走到 0 且根读不动（掉盘/被删/没权限）→ 不授权。
        assert!(root_authorizes_deletion(&walkable, 12));
        assert!(
            root_authorizes_deletion(&walkable, 0),
            "空目录是「真的空了」，这一片该清"
        );
        assert!(
            !root_authorizes_deletion(&vanished, 0),
            "走不动的根不得授权删除"
        );

        // 端到端：库里两片条目，只有走得动的那个根进入边界。
        let existing = std::collections::HashMap::from([
            (
                walkable.join("kept.txt").to_string_lossy().into_owned(),
                (1i64, 1u64),
            ),
            (
                vanished.join("kept.txt").to_string_lossy().into_owned(),
                (1i64, 1u64),
            ),
        ]);
        let visited = std::collections::HashSet::new();
        let stale = stale_entries(&existing, &visited, std::slice::from_ref(&walkable));
        assert_eq!(
            stale,
            vec![walkable.join("kept.txt").to_string_lossy().into_owned()],
            "只有走过的根之内可以删；走不动的根之下必须原样留着"
        );

        // 对照：把请求的根（含走不动的那个）整片交进来，正是修复前的行为。
        let unscoped = stale_entries(&existing, &visited, &[walkable.clone(), vanished.clone()]);
        assert_eq!(unscoped.len(), 2, "未过滤的边界会连走不动的根一起删");

        let _ = std::fs::remove_dir_all(&walkable);
    }

    /// 扫描线程 panic 的兜底收口：running 必须落回 false。`scan_state` 是
    /// parking_lot::Mutex（不中毒），没有兜底就永远停在 running:true，而
    /// `pinvou knowledge scan start` 是唯一阻塞等待该标志的调用方，会挂死。
    #[test]
    fn scan_panic_guard_always_clears_the_running_flag() {
        let state = Mutex::new(ScanState {
            running: true,
            phase: "scanning".into(),
            roots: vec!["/tmp".into()],
            scanned: 7,
            finished_at: 0,
        });
        finish_scan_after_panic(&state);
        let st = state.lock();
        assert!(!st.running, "panic 之后必须放掉 running，否则阻塞方挂死");
        assert_eq!(
            st.phase, "interrupted",
            "既不是 done（前端只在 done 刷 L0）也不是 cancelled（不是用户取消）"
        );
        assert!(st.finished_at > 0);
    }

    /// Headless read contract (stats / type_counts / search): zero-state
    /// reads, post-seed reads, search with the NL-rule merge ("上周的 pdf" →
    /// exts + mtime filter + residual text stripping) matching kb_stats /
    /// kb_type_counts / kb_search semantics; explicit structured filters win
    /// over the parsed result.
    #[test]
    fn headless_stats_type_counts_and_search_match_gui_semantics() {
        let svc = service();
        assert_eq!(svc.stats().expect("zero-state stats"), Stats::default());
        assert!(
            svc.type_counts()
                .expect("zero-state type counts")
                .is_empty()
        );

        // Seed one L0 record through the same upsert path the scanner uses
        // (no full scan involved).
        svc.store
            .upsert_many(&[store::FileRecord {
                path: "/tmp/docs/季度报告.pdf".into(),
                name: "季度报告.pdf".into(),
                ext: Some("pdf".into()),
                size: 2048,
                mtime: now(),
                is_dir: false,
            }])
            .expect("seed one file record");

        let stats = svc.stats().expect("stats after seed");
        assert_eq!(stats.total_files, 1);
        assert_eq!(
            svc.type_counts().expect("type counts after seed"),
            vec![TypeCount {
                ext: "pdf".into(),
                count: 1
            }]
        );

        // Explicit structured search: text goes through FTS/LIKE on the same
        // store path as the GUI command.
        let hits = svc
            .search(SearchQueryDto {
                text: Some("季度报告".into()),
                limit: 10,
                ..Default::default()
            })
            .expect("structured search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].ext.as_deref(), Some("pdf"));

        // NL-rule merge: "上周的 pdf" → exts=[pdf] + mtime_after ≈ 7 days ago
        // + empty residual text. Without the merge (raw FTS on "上周的 pdf")
        // nothing would match.
        let hits = svc
            .search(SearchQueryDto {
                text: Some("上周的 pdf".into()),
                limit: 10,
                ..Default::default()
            })
            .expect("nl-rule merged search");
        assert_eq!(hits.len(), 1, "NL merge must hit the seeded pdf: {hits:?}");
        assert_eq!(hits[0].name, "季度报告.pdf");

        // Explicit exts win over the parsed result and are not overwritten
        // (GUI contract).
        let hits = svc
            .search(SearchQueryDto {
                text: Some("上周的 pdf".into()),
                exts: vec!["txt".into()],
                limit: 10,
                ..Default::default()
            })
            .expect("explicit ext wins over parsed");
        assert!(
            hits.is_empty(),
            "explicit txt filter must not hit a pdf: {hits:?}"
        );
    }
}
