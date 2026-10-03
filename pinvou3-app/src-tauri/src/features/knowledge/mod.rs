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
// Embedding-model on-demand download logic. Plain functions consumed by the
// app command layer (`app::commands::knowledge` owns the Tauri command
// wrappers via the passthrough macros).
pub mod model_download;
// The headless CLI (`pinvou knowledge asr`-adjacent model checks) consumes
// the completeness predicate through this re-export instead of a copy: it
// cannot name the transitive `pinvou-knowledge` crate, and a copy silently
// desynced the two surfaces every time the manifest changed.
pub use pinvou_knowledge::model_download::model_directory_is_complete;
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
    /// idle / scanning / done / interrupted (a scan-thread panic
    /// is caught and contained; see `finish_scan_after_panic`). The frontend
    /// refreshes L0 only on `done`, so `interrupted` cannot read as
    /// "scanned".
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
    imports: import_jobs::ImportJobStore,
    active_import: Arc<Mutex<Option<String>>>,
    index_cancel: Arc<AtomicBool>,
    /// embedder 空闲卸载巡检任务句柄 + 防振荡时钟。模型常驻 ~570MB 内存；
    /// 巡检在空闲超阈值时 `set_embedder(None)` 卸载，kb_model_status 的热加载
    /// 钩子（用户意图）会自动重载。放 Option 使「未装模型 → 不起巡检」零开销。
    embedder_reaper: Arc<Mutex<Option<EmbedderReaperGuard>>>,
    /// 上次自动卸载 embedding 模型的 UNIX 秒（防振荡）：距上次卸载不足
    /// EMBEDDER_UNLOAD_COOLDOWN_SECS 时不再自动卸载。放在服务级字段（而非
    /// 巡检任务内）是因为卸载/热加载会重启巡检任务，冷却必须跨任务存活。
    embedder_last_unload_epoch: Arc<AtomicI64>,
}

/// Idle-unload reaper handle for the embedding model: dropping the guard
/// cancels the inspection task (the semantics used to live in a wrapping
/// `EmbedderReaper` struct, now inlined into the slot type).
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
        *slot = Some(EmbedderReaperGuard { cancel, handle });
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
                model_download::set_model_load_error(None);
                eprintln!("[knowledge] 导入前重载 embedding 模型完成（向量化解锁）");
            }
            Err(error) => {
                // 与首帧加载同语义：加载失败保持全文降级，不阻断导入。失败诊断必须
                // 落 MODEL_LOAD_ERROR——否则状态停在 installed+未就绪且 error=None，
                // 失败门上的 Retry/Repair 按钮虽在，却显示不出任何失败原因。
                model_download::set_model_load_error(Some(error.clone()));
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

    /// Import state of the NAMED job (not "the latest job").
    ///
    /// [`Self::index_status`] goes through `ImportJobStore::latest_state`'s
    /// priority ordering (preparing/running → interrupted → done_with_errors →
    /// the rest, then updated_at descending): `cancelled` lands in the last
    /// tier, so any earlier done_with_errors / interrupted job outranks it.
    /// The GUI is unaffected — it only polls "the job currently most worth
    /// showing the user", which is exactly what that ordering is designed to
    /// answer; but the headless CLI's `index cancel <job-id>` /
    /// `index status <job-id>` report the job they NAMED, and reading latest
    /// after a cancel would cross-wire to another job's state (the human line
    /// says B was cancelled, the JSON body carries A). Same entry point and
    /// same semantics as the `imports.state(&job_id)` lookups that close out
    /// `resume_index`/`retry_index_item`.
    ///
    /// A nonexistent job is reported as an error (the layer below surfaces
    /// `QueryReturnedNoRows`), letting callers distinguish "the job is gone"
    /// from "the job exists but its state row is empty".
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

    /// Cancels the NAMED import job to `cancelled` and returns its
    /// pre-transition state.
    ///
    /// A named-job entry point isomorphic to [`Self::interrupt_index`] (the
    /// only difference is the transition: this method goes through
    /// ImportJobStore::cancel, applies to preparing/running/interrupted
    /// and commits synchronously). The headless CLI's `index cancel
    /// <job-id>` uses it instead of [`Self::cancel_index`]'s "latest job"
    /// re-derivation: the CLI validates `active == job_id` before calling,
    /// while `cancel_index` re-derives the target on its own — the window
    /// between the two store reads could cancel a job the caller never
    /// named. A nonexistent job is reported as an error (same as
    /// `interrupt_index`), a finished job stays an idempotent no-op — the
    /// named job can never be silently swapped for another.
    pub fn cancel_index_job(&self, job_id: &str) -> Result<IndexState, String> {
        let state = self.imports.state(job_id).map_err(|e| e.to_string())?;
        if state.running || state.resumable {
            self.imports.cancel(job_id).map_err(|e| e.to_string())?;
            self.index_cancel.store(true, Ordering::Relaxed);
            self.l1.set_collection_status(state.collection_id, "ready");
        }
        Ok(state)
    }

    /// Returns the NAMED import job to `interrupted` (resumable). The closing
    /// entry point for the CLI's wait-timeout path: when the import thread is
    /// dead or wedged, the job is left in interrupted (not running), so the
    /// next `resume_index` can continue without waiting for the desktop app
    /// to boot and recover it. Structurally the same defensive lookup as
    /// [`Self::cancel_index`] — the named state is read first and an unknown
    /// job is reported as an error (never a silent no-op); the transition
    /// itself is delegated to `ImportJobStore::interrupt`: it only applies to
    /// preparing/running (enforced in the SQL WHERE), and finished/interrupted
    /// jobs are an idempotent no-op. The collection is only parked at
    /// "pending" when the transition APPLIED: a job whose last item finished
    /// between the state read and the interrupt must keep its collection at
    /// "ready" instead of a permanently stale "pending" with no self-healing
    /// path (`index resume` refuses a non-interrupted job).
    pub fn interrupt_index(&self, job_id: &str) -> Result<(), String> {
        let state = self.imports.state(job_id).map_err(|e| e.to_string())?;
        if state.running {
            let applied = self.imports.interrupt(job_id);
            // The stall-timeout premise is that the import thread is gone or
            // wedged, so `launch_import`'s close-out (which resets the
            // "indexing" status this job set) will never run. Park the
            // collection at "pending" — resumable, exactly like boot
            // recovery relabels an interrupted job — instead of leaving the
            // GUI a permanently "indexing" collection until the next
            // resume/cancel.
            if applied {
                self.l1
                    .set_collection_status(state.collection_id, "pending");
            }
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
        let panic_l1 = self.l1.clone();
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
                    // `is_stopped` covers an external interrupt AND a cancel
                    // (both leave the runnable states): a slow-but-alive
                    // thread must not keep claiming the items the interrupt
                    // moved back to pending and end the job fully-ingested
                    // yet `interrupted`.
                    if infrastructure_error
                        || cancel.load(Ordering::Relaxed)
                        || imports.is_stopped(&job_id)
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
                let state = panic_imports.state(&panic_job_id).ok();
                let applied = panic_imports.interrupt(&panic_job_id);
                // 与 `interrupt_index` 同款 applied-parking：中断落地时集合停回
                // "pending"（可续作），而不是卡死在线程 panic 前已置上的
                // "indexing"。interrupt 未落地说明最后一个条目恰好在读态与中断
                // 之间完成，close-out 的语义仍然成立，集合保持原状。
                if applied && state.as_ref().is_some_and(|s| s.running) {
                    if let Some(state) = state {
                        panic_l1.set_collection_status(state.collection_id, "pending");
                    }
                }
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
            *st = ScanState {
                running: true,
                phase: "scanning".into(),
                roots: roots.iter().map(|p| p.display().to_string()).collect(),
                ..Default::default()
            };
        }

        let store = self.store.clone();
        let scan_state = self.scan_state.clone();
        // The panic backstop needs its own state handle that the closure
        // cannot move away, otherwise there is no way to close out after a
        // panic.
        let panic_scan_state = self.scan_state.clone();

        thread::spawn(move || {
            // Scan-thread panic backstop (same semantics as the import
            // thread in `launch_import`): `running` is only cleared by the
            // normal close-out below, and `scan_state` is a parking_lot::Mutex
            // — it cannot poison, so after a panic the lock stays lockable
            // while the state is stuck at running:true forever. The GUI just
            // shows a progress bar that stops advancing (lazy trigger, next
            // visit rescans), but `pinvou knowledge scan start` is the one
            // caller that BLOCKS on that flag and would hang with no output
            // and no exit code.
            let outcome = catch_unwind(AssertUnwindSafe(move || {
                let ex = Excluder::default();
                // Incremental: load the existing snapshot; the scanner only
                // writes files whose mtime/size changed and skips the rest.
                let existing = store.load_index().unwrap_or_default();
                let mut visited = std::collections::HashSet::new();
                let mut scanned_total = 0u64;
                // Deletion authority is granted only for roots that were
                // actually WALKED WITHOUT ERROR, not for roots that were
                // REQUESTED (see `root_authorizes_deletion`): scanner::scan
                // reports zero entries for a root it cannot walk, and the
                // cleanup phase still runs — treating that root as a deletion
                // boundary would wipe its whole indexed slice as "discovered
                // missing". A walk ERROR under an otherwise walkable root is
                // the same veto (an unreadable subtree's slice is just as
                // undecidable), so the error count rides along.
                let mut swept_roots: Vec<PathBuf> = Vec::with_capacity(roots.len());
                for root in &roots {
                    let base = scanned_total;
                    let (walked, walk_errors) =
                        scanner::scan(root, &store, &ex, &existing, &mut visited, |n| {
                            scan_state.lock().scanned = base + n;
                        });
                    scanned_total = base + walked;
                    scan_state.lock().scanned = scanned_total;
                    if root_authorizes_deletion(root, walked, walk_errors) {
                        swept_roots.push(root.clone());
                    }
                }

                // Clean up files that "disappeared" (indexed last time, not
                // walked this time).
                let stale = stale_entries(&existing, &visited, &swept_roots);
                if !stale.is_empty() {
                    let _ = store.delete_many(&stale);
                }

                // Deduplication (hashing) no longer runs inside the scan — it is
                // disk-expensive, unbounded on million-file libraries and stalls
                // slow devices. The dedup feature itself is retired.
                let finished_at = now();
                let _ = store.set_last_scan_finished_at(finished_at);
                let mut st = scan_state.lock();
                st.running = false;
                st.finished_at = finished_at;
                st.phase = "done".into();
            }));
            if let Err(panic) = outcome {
                // Same "never stall silently" rule as the idle patrol's
                // single-round backstop: without this, a dead scan thread
                // would only manifest as a stopped progress bar with
                // nothing to trace.
                //
                // The state backstop runs FIRST: Rust ignores SIGPIPE, so
                // with stderr closed the write itself panics and an
                // `eprintln!` here would strand `running: true` — the
                // exact stall this arm exists to prevent. The write's
                // errors are ignored on purpose (assistant's `note_stderr`
                // semantics, inlined so knowledge does not reach into
                // another feature for two lines).
                finish_scan_after_panic(&panic_scan_state);
                use std::io::Write as _;
                let _ = writeln!(
                    std::io::stderr(),
                    "[knowledge] scan thread panicked (contained; the stale sweep's outcome is unknown): {panic:?}"
                );
            }
        });

        self.scan_state.lock().clone()
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
        // 导入侧沿用「错误即跳过」：导入的目的是收录可读文件，单个不可读
        // 子树不否定其余条目（与全盘扫描的删除授权不同，那边必须否决）。
        for entry in scanner::walk_pruned(root, &ex).flatten() {
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

/// Post-panic close-out for the scan thread: clears `running` and lands the
/// phase on `interrupted`.
///
/// A distinct phase instead of reusing done/cancelled: the scan neither
/// finished (`last_scan_finished_at` is not recorded, so the next visit still
/// treats it as never-scanned; the in-memory finished_at only records the
/// moment of the abort) nor was it user-cancelled. The frontend refreshes L0
/// only on `done`, so `interrupted` cannot be mistaken for success; the
/// blocking CLI caller uses it to report an error instead of announcing the
/// scan complete.
fn finish_scan_after_panic(scan_state: &Mutex<ScanState>) {
    let mut st = scan_state.lock();
    st.running = false;
    st.finished_at = now();
    st.phase = "interrupted".into();
}

/// Whether THIS scan round is authorized to run "disappeared" deletions
/// inside `root`.
///
/// `scanner::scan` reports zero entries for a root it cannot WALK: an
/// unmounted drive, revoked permissions, or the root being deleted after the
/// pre-flight all look identical to "0 entries walked". Handing that root to
/// [`stale_entries`] as a deletion boundary would condemn its whole indexed
/// slice as disappeared — `scan start --root /mnt/usb` after the drive drops
/// would delete every entry under `/mnt/usb` and still report
/// `phase: done`. The flavors that must NOT delete:
/// - **Could not walk the root** (missing / not a directory / unreadable):
///   the fate of that root's indexed entries is undecidable this round, so
///   the round must not decide for it — skip (the next round cleans up
///   naturally once the root is back).
/// - **Walk errors under the root** (`walk_errors > 0`, the round-27 review
///   gap): a top-level probe cannot see a subtree that failed mid-walk, but
///   the slice under a chmod-000 subdirectory is just as undecidable — the
///   entries are absent from `visited` because the walker could not read
///   them, which is indistinguishable from "disappeared". `scanner::scan`
///   therefore surfaces the walk-error count and any error vetoes the
///   root's stale sweep for this round; the slice stays and the next fully
///   readable round cleans it. The trade-off is disclosed: one permanently
///   unreadable file pauses ghost cleanup for its whole root (the safe
///   direction — lingering, not wiping).
///
/// The flavors that MUST delete:
/// - **Walked, and it is empty** (the root is still a readable directory):
///   the user really did empty it; the slice should be cleaned, otherwise
///   ghost entries linger forever — this is why the deletion feature exists
///   at all, so it cannot be switched off for safety.
///
/// One ambiguous corner remains: a static mountpoint whose unmount leaves a
/// readable empty directory behind — filesystem-identical to "the user
/// emptied the directory", indistinguishable by any pure path probe. That
/// case is treated as "empty directory" (cleanup proceeds): it is the
/// minority compared to the common forms where a dropped drive takes the
/// mountpoint with it or makes it unreadable (udisks automounts, ESTALE/EIO
/// after device removal, permission revocation), and it is at least not
/// silent — deletion only ever happens inside a root the user NAMED.
fn root_authorizes_deletion(root: &Path, walked: u64, walk_errors: u64) -> bool {
    if walk_errors != 0 {
        return false;
    }
    if walked > 0 {
        return true;
    }
    // A root the walker refuses BY POLICY (its basename sits on the
    // exclusion list — `build`, `dist`, `venv`, `.cache`, …) is not "walked
    // and empty": `walk_pruned` subjects the depth-0 root to the same skip
    // predicate as every entry, so a readable, non-empty excluded-name root
    // also reports zero entries and zero errors. Authorizing the sweep here
    // would delete the slice of a directory nobody ever looked at while
    // reporting `done, scanned: 0`. The empty-looking exclusion-root round
    // therefore vetoes, exactly like a walk error (the safe lingering
    // direction); a root the user genuinely emptied AND renamed off the
    // exclusion list sweeps on the next round.
    if let Some(name) = root.file_name().and_then(|name| name.to_str())
        && Excluder::default().is_skipped(name, true, None)
    {
        return false;
    }
    // A symlinked root is followed for traversal (walkdir's follow_root_links
    // defaults to true; only the root ENTRY itself reports is_symlink and is
    // skipped by the recorder), so a non-empty target yields walked > 0 and
    // is authorized before this probe — correctly, since `visited` really
    // covers the target's children. The corner this probe owns is the EMPTY
    // target: it walks as zero entries, and authorizing it via read_dir
    // alone would let the sweep run against a root whose readable surface
    // (the link target) was never the indexed directory. A zero-walked root
    // therefore authorizes only when it is a real (non-symlink) directory
    // the probe can list.
    let is_real_dir = std::fs::symlink_metadata(root)
        .map(|meta| meta.is_dir())
        .unwrap_or(false);
    is_real_dir && std::fs::read_dir(root).is_ok()
}

/// Computes this round's "disappeared" entries: decided ONLY within the
/// roots actually walked this round.
///
/// The old logic treated "indexed but not walked this time" as disappeared
/// unconditionally. Under the GUI that is harmless — it only ever scans the
/// single root of the user's home directory (`kb_start_scan(roots: null)`),
/// every indexed entry lives under that root, so this function's result is
/// identical to the old logic entry for entry. But the CLI's
/// `knowledge scan start --root <DIR>` can scan any directory: while scanning
/// directory B, entries indexed from directory A would be condemned as stale
/// wholesale — a silent full-index wipe reported as `done`. Deletion
/// therefore requires the entry to fall under a root WALKED this round —
/// entries outside every walked root were never this round's responsibility,
/// so their fate cannot be decided.
///
/// `roots` are the roots that were actually WALKED (filtered through
/// [`root_authorizes_deletion`]), not the roots the caller requested: a
/// TOCTOU stands between request and walk, and treating an unwalkable
/// requested root as a boundary would wipe its whole slice.
///
/// Containment is compared by path COMPONENT via [`Path::starts_with`],
/// never by string prefix: `/home/a` must not match `/home/abc`.
fn stale_entries(
    existing: &std::collections::HashMap<String, (i64, u64)>,
    visited: &std::collections::HashSet<String>,
    roots: &[PathBuf],
) -> Vec<String> {
    // Each root is canonicalized exactly once (falling back to the raw path
    // on failure, e.g. the root was deleted between request and judgment).
    // Both the raw and the canonicalized spellings participate in the
    // comparison: store keys may have been written under either form.
    let bounds: Vec<(PathBuf, PathBuf)> = roots
        .iter()
        .map(|root| {
            let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
            (root.clone(), canonical)
        })
        .collect();
    let mut canonical_cache: std::collections::HashMap<PathBuf, Option<PathBuf>> =
        std::collections::HashMap::new();
    existing
        .keys()
        .filter(|path| !visited.contains(*path))
        .filter(|path| {
            within_scanned_roots(Path::new(path.as_str()), &bounds, &mut canonical_cache)
        })
        .cloned()
        .collect()
}

/// Whether the path falls under one of this round's scanned roots (the root
/// itself included).
///
/// The raw in-memory comparison runs FIRST and short-circuits: the GUI has
/// exactly one root (the home directory) and every indexed entry hits on
/// this step, so the whole cleanup phase degrades to the old HashSet
/// difference's cost. The reverse ordering (canonicalize every entry first)
/// would fire one doomed syscall per entry — tens of thousands of them after
/// a large directory deletion, all on the scan thread.
///
/// canonicalize is only the fallback: when a scanned root is a symlink, or
/// the store's key itself is a relative/symlinked spelling, containment is
/// only decidable after normalizing. Stale paths are usually already gone
/// from disk (that is what stale means), so canonicalizing them necessarily
/// fails — and failure answers "not inside a boundary": no deletion
/// authority, the same semantics as the old version.
fn within_scanned_roots(
    path: &Path,
    bounds: &[(PathBuf, PathBuf)],
    canonical_cache: &mut std::collections::HashMap<PathBuf, Option<PathBuf>>,
) -> bool {
    if bounds.iter().any(|(raw_root, canonical_root)| {
        path.starts_with(raw_root) || path.starts_with(canonical_root)
    }) {
        return true;
    }
    // The fallback normalizes a store key whose spelling diverges from the
    // scanned roots (symlinked root, symlinked store key). Entries are
    // sibling-dense, so canonicalization is cached per PARENT directory:
    // one syscall per directory per round instead of one per file, and a
    // gone file whose parent is also gone still answers from the cache.
    let canonical: PathBuf = match path.parent() {
        Some(parent) => {
            let cached = canonical_cache
                .entry(parent.to_path_buf())
                .or_insert_with(|| std::fs::canonicalize(parent).ok());
            let Some(canonical_parent) = cached else {
                // The parent itself does not exist, so the child cannot have
                // an alternative readable spelling: same answer as
                // canonicalize failing on the child.
                return false;
            };
            match path.file_name() {
                Some(name) => canonical_parent.join(name),
                // A path with no file name is a root spelling; the
                // raw/canonical bound check above already covered it.
                None => return false,
            }
        }
        // No parent: nothing left to normalize.
        None => return false,
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
    /// Tie half of the `(mtime, id)` keyset cursor (`idBefore` on the wire).
    /// Only valid together with `mtimeBefore`; a half cursor is rejected.
    pub id_before: Option<i64>,
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
            id_before: d.id_before,
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
        // 本测试向进程级 MODEL_LOAD_ERROR 写入失败诊断并精确断言其值，与
        // model_download 的 leased_reload 测试互斥，避免并行读到对方写入值。
        let _guard = model_download::MODEL_LOAD_ERROR_TEST_LOCK.blocking_lock();
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
        // 失败诊断必须落 MODEL_LOAD_ERROR：否则状态停在 installed+未就绪且
        // error=None，前端失败门给得出 Retry/Repair 却显示不了失败原因
        // （回归锚点：崩溃中断安装后 installed+未就绪+error=None 的僵尸状态）。
        assert_eq!(
            model_download::model_load_error().as_deref(),
            Some("模拟加载失败")
        );

        let svc = service();
        svc.reload_embedder_if_import_needed_with(true, || Err("再次失败".into()));
        assert!(!svc.semantic_ready(), "失败后再次导入仍应重试补载");
        assert_eq!(
            model_download::model_load_error().as_deref(),
            Some("再次失败"),
            "后续失败覆盖旧诊断，状态不得停留在上一次的错误上"
        );
    }

    /// The named job's state must come from THAT job, never from
    /// `latest_state`'s priority ordering. Regression scenario: old job A
    /// finishes with one failed item → done_with_errors (second tier); new
    /// job B gets cancelled and lands in the last tier, so A outranks B —
    /// the CLI's `index cancel B` human line says "cancelled B" while the
    /// JSON body carries A's state and A's jobId.
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

        // Old job: finishes after one failed item → done_with_errors
        // (second tier of the ordering).
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

        // New job: preparing at creation (tier zero), so it is the latest
        // one.
        let new_job = svc.imports.create(newer, &[]).expect("newer job");
        assert_eq!(
            svc.index_status().job_id.as_deref(),
            Some(new_job.as_str()),
            "a running/preparing new job must outrank an old done_with_errors"
        );

        svc.imports.cancel(&new_job).expect("cancel newer job");
        // After the cancel, latest crosses over to the old job — this is
        // exactly the misreport being fixed; pin it so nobody later assumes
        // index_status() would have been good enough here.
        assert_eq!(
            svc.index_status().job_id.as_deref(),
            Some(old_job.as_str()),
            "cancelled falls to the last tier; latest_state is outranked by the old done_with_errors"
        );

        let named = svc
            .index_job_state(&new_job)
            .expect("the cancelled job must still be readable by id");
        assert_eq!(named.job_id.as_deref(), Some(new_job.as_str()));
        assert_eq!(named.collection_id, newer);
        assert!(
            named.cancelled,
            "the named read must see the post-cancel state"
        );
        assert!(!named.running && !named.resumable);
        assert!(
            svc.index_job_state("kb-import-does-not-exist").is_err(),
            "a nonexistent job must error, never degrade into another job's state"
        );
    }

    /// A named cancel must cancel only THAT job and hand the pre-transition
    /// state back to the caller (the CLI uses it to distinguish "a signal
    /// was really sent" from "the job already finished, nothing to do").
    /// Regression scenario: the old `cancel_index` path re-derived "the
    /// latest job" after the CLI's validation, and a desktop-app job started
    /// between the two store reads would be cancelled by mistake.
    #[test]
    fn cancel_index_job_cancels_only_the_named_job_and_reports_its_pre_state() {
        let svc = service();
        let collection = svc
            .l1
            .create_collection("cancel-by-id", None, None)
            .expect("collection");

        // A running job plus a later preparing job: the latter is the
        // latest.
        let older = svc
            .imports
            .create(collection, &[PathBuf::from("/tmp/a.txt")])
            .expect("older job");
        svc.imports
            .prepare_items(&older, &[PathBuf::from("/tmp/a.txt")])
            .expect("prepare older item");
        let item = svc
            .imports
            .claim_next(&older)
            .expect("claim older item")
            .expect("one pending item");
        // Neither finishes: older stays in preparing/running (running=true).
        let _ = item;
        let newer = svc
            .imports
            .create(collection, &[PathBuf::from("/tmp/b.txt")])
            .expect("newer job");
        // Two simultaneously-live jobs (tied at the prepare tier) is
        // exactly the race window: the old path
        // `cancel_index` re-derived latest (updated_at has second-level
        // precision, tie order undefined);
        // the named path must look at job_id only.

        // Cancel OLDER by name: it must hit older (pre-state running),
        // with newer left untouched.
        let pre = svc.cancel_index_job(&older).expect("cancel older job");
        assert_eq!(pre.job_id.as_deref(), Some(older.as_str()));
        assert!(pre.running, "the pre-transition state must be running");
        let after = svc.index_job_state(&older).expect("older still readable");
        assert!(after.cancelled && !after.running && !after.resumable);
        let newer_state = svc
            .index_job_state(&newer)
            .expect("newer must stay untouched");
        assert!(
            !newer_state.cancelled,
            "a job that was not named must not be cancelled"
        );

        // A finished/cancelled job is an idempotent no-op (the pre state
        // is returned as-is).
        let pre_again = svc
            .cancel_index_job(&older)
            .expect("second cancel is a no-op");
        assert!(!pre_again.running && !pre_again.resumable);

        // A nonexistent job is reported as an error, never silently
        // retargeted.
        assert!(svc.cancel_index_job("kb-import-does-not-exist").is_err());
    }

    /// `interrupt_index` parks the collection at `pending` only when the
    /// interrupt actually APPLIED: a job whose last item finished between
    /// the caller's state read and the interrupt must keep its collection
    /// where it is — a fully-indexed collection must not read as needing
    /// work with no self-healing path (`index resume` refuses a
    /// non-interrupted job).
    #[test]
    fn interrupt_index_parks_the_collection_only_when_the_transition_applies() {
        let svc = service();
        let collection = svc
            .l1
            .create_collection("interrupt-by-id", None, None)
            .expect("collection");
        let collection_status = |svc: &KnowledgeService| {
            svc.l1
                .list_collections()
                .expect("collections readable")
                .into_iter()
                .find(|c| c.id == collection)
                .expect("collection row")
                .status
        };

        // A job with one running item: the interrupt applies.
        let job = svc
            .imports
            .create(collection, &[PathBuf::from("/tmp/a.txt")])
            .expect("job");
        svc.imports
            .prepare_items(&job, &[PathBuf::from("/tmp/a.txt")])
            .expect("prepare one item");
        let _item = svc
            .imports
            .claim_next(&job)
            .expect("claim the item")
            .expect("one running item");
        svc.l1.set_collection_status(collection, "indexing");
        svc.interrupt_index(&job).expect("interrupt applies");
        let state = svc.index_job_state(&job).expect("state readable");
        assert!(
            state.resumable && !state.running,
            "the job must land at interrupted"
        );
        assert_eq!(
            collection_status(&svc),
            "pending",
            "an applied interrupt parks the collection at pending"
        );

        // A second interrupt on the interrupted job is a no-op for the
        // collection too (it must not flap a ready collection back).
        svc.l1.set_collection_status(collection, "ready");
        svc.interrupt_index(&job)
            .expect("a second interrupt is an idempotent no-op");
        assert_eq!(
            collection_status(&svc),
            "ready",
            "a no-op interrupt must not park the collection"
        );

        // The store-level lost race: a job that finished before the
        // interrupt reports applied=false (the caller's state read said
        // running, the transition no-oped) — the guard that keeps the
        // collection honest.
        let finished = svc
            .imports
            .create(collection, &[PathBuf::from("/tmp/b.txt")])
            .expect("second job");
        svc.imports
            .prepare_items(&finished, &[PathBuf::from("/tmp/b.txt")])
            .expect("prepare second item");
        let item = svc
            .imports
            .claim_next(&finished)
            .expect("claim second item")
            .expect("one running item");
        svc.imports
            .mark_failed(&finished, item.id, "fixture failure");
        svc.imports.finish(&finished).expect("finish second job");
        assert!(
            !svc.imports.interrupt(&finished),
            "an interrupt that lost the race must report applied=false"
        );
    }

    /// The deletion boundary for "disappeared" entries can only be roots
    /// that were actually WALKED.
    ///
    /// `scanner::scan` silently returns 0 for an unwalkable root (drive
    /// dropped / permissions revoked / root deleted); treating it as a
    /// boundary all the same would condemn that root's whole indexed slice
    /// as disappeared while reporting `done/scanned:0`. The legitimate path —
    /// the user really emptied the directory — must keep working: a root
    /// that is still there and still empty still cleans.
    #[test]
    fn stale_sweep_bounds_exclude_a_root_that_could_not_be_walked() {
        let unique = format!("{}_{}", std::process::id(), now());
        let walkable = std::env::temp_dir().join(format!("pinvou3_kb_stale_bounds_{unique}"));
        std::fs::create_dir_all(&walkable).expect("create walkable root");
        // A sibling directory, not a child: a child would be incidentally
        // matched by `starts_with(walkable)` and could not test "an
        // unwalkable root stays out of the boundary". The path is never
        // created = the unwalkable root.
        let vanished = std::env::temp_dir().join(format!("pinvou3_kb_stale_gone_{unique}"));

        // Walked (walked>0) → authorized; walked 0 but the root is still a
        // readable directory (the user emptied it) → authorized; walked 0
        // and the root is unreadable (drive dropped / deleted / no
        // permission) → not authorized; ANY walk error under the root →
        // not authorized (the unreadable-subtree veto: the round-27 review
        // gap, where a chmod-000 subtree's slice was wiped as "disappeared"
        // because the top-level probe saw a walkable root).
        assert!(root_authorizes_deletion(&walkable, 12, 0));
        assert!(
            root_authorizes_deletion(&walkable, 0, 0),
            "an empty directory is genuinely empty; this slice must clean"
        );
        assert!(
            !root_authorizes_deletion(&vanished, 0, 0),
            "an unwalkable root must not authorize deletion"
        );
        assert!(
            !root_authorizes_deletion(&walkable, 12, 1),
            "a walk error under the root vetoes the stale sweep: the fate of              everything under the unreadable subtree is undecidable"
        );
        assert!(
            !root_authorizes_deletion(&walkable, 0, 3),
            "the veto applies on the empty-root flavor too"
        );

        // End to end: two indexed slices in the store; only the walkable
        // root enters the boundary.
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
            "only walked roots may delete; under an unwalkable root everything must stay"
        );

        // Contrast: handing in the requested roots wholesale (including
        // the unwalkable one) is exactly the pre-fix behavior.
        let unscoped = stale_entries(&existing, &visited, &[walkable.clone(), vanished.clone()]);
        assert_eq!(
            unscoped.len(),
            2,
            "an unfiltered boundary would delete the unwalkable root too"
        );

        let _ = std::fs::remove_dir_all(&walkable);
    }

    /// Post-panic close-out for the scan thread: `running` must fall back
    /// to false. `scan_state` is a parking_lot::Mutex (cannot poison), so
    /// without the backstop it would stay at running:true forever — and
    /// `pinvou knowledge scan start` is the one caller that blocks on that
    /// flag and would hang.
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
        assert!(
            !st.running,
            "running must be released after a panic or the blocking caller hangs"
        );
        assert_eq!(
            st.phase, "interrupted",
            "neither done (the frontend refreshes L0 only on done) nor cancelled (no user cancel)"
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

    /// Unique per-test db path (see `service()` for why the path must be
    /// unique), returning the path so a SECOND service can be opened over
    /// the same file — that is the whole point of these tests.
    fn unique_db_path(tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pinvou3_kb_headless_{}_{}_{}",
            tag,
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        dir.join("index.db")
    }

    /// `new_without_recovery` is the headless open: it must NOT run
    /// `recover_interrupted`, or a read-only CLI glance over a live store
    /// flips a GUI import's running job to interrupted. The contrast arm
    /// (plain `new`) must still recover.
    #[test]
    fn new_without_recovery_leaves_running_imports_alone() {
        let db = unique_db_path("norecover");
        let svc = KnowledgeService::new(&db).expect("seed service");
        let collection = svc
            .l1
            .create_collection("seed 集", None, None)
            .expect("seed collection");
        let job = svc
            .imports
            .create(collection, &[std::path::PathBuf::from("/tmp")])
            .expect("seed import job");
        // create() lands state='preparing' — the exact in-flight shape
        // recover_interrupted() exists to terminalize.
        let seeded = svc.imports.state(&job).expect("seeded job state");
        assert!(seeded.running, "seeded job must be in-flight: {seeded:?}");

        let headless =
            KnowledgeService::new_without_recovery(&db).expect("headless open over the live store");
        let untouched = headless.imports.state(&job).expect("headless view of job");
        assert!(
            untouched.running && !untouched.resumable,
            "headless open must not terminalize a live GUI import: {untouched:?}"
        );

        // Contrast: the recovering open flips it to interrupted/resumable.
        let recovering = KnowledgeService::new(&db).expect("recovering open");
        assert!(
            recovering
                .imports
                .state(&job)
                .expect("recovered view of job")
                .resumable,
            "plain open must still recover a dead process's in-flight job"
        );
    }

    /// `document_exists` is the headless existence probe: true for an id the
    /// store knows, false for one it does not (and for one deleted in
    /// between).
    #[test]
    fn document_exists_reports_store_truth() {
        let db = unique_db_path("docexists");
        let svc = KnowledgeService::new(&db).expect("seed service");
        let collection = svc
            .l1
            .create_collection("seed 集", None, None)
            .expect("seed collection");
        let doc = svc
            .l1
            .upsert_document(collection, "/tmp/a.pdf", "a.pdf", Some("pdf"), 10, now())
            .expect("seed document");

        assert!(
            svc.l1.document_exists(doc).expect("probe existing doc"),
            "seeded document must exist"
        );
        assert!(
            !svc.l1.document_exists(doc + 1).expect("probe unknown doc"),
            "unknown id must report absent, not error"
        );

        svc.l1.remove_document(doc).expect("remove document");
        assert!(
            !svc.l1.document_exists(doc).expect("probe removed doc"),
            "removed document must report absent"
        );
    }

    /// `mtime_before` upper-bound filter: inclusive at the boundary, excludes
    /// newer files, composes with `mtime_after` into a half-open window.
    #[test]
    fn search_mtime_before_bounds_the_window() {
        let db = unique_db_path("mtimebefore");
        let svc = KnowledgeService::new(&db).expect("seed service");
        let old_mt = now() - 10_000;
        let new_mt = now();
        svc.store
            .upsert_many(&[
                store::FileRecord {
                    path: "/tmp/old.txt".into(),
                    name: "old.txt".into(),
                    ext: Some("txt".into()),
                    size: 1,
                    mtime: old_mt,
                    is_dir: false,
                },
                store::FileRecord {
                    path: "/tmp/new.txt".into(),
                    name: "new.txt".into(),
                    ext: Some("txt".into()),
                    size: 1,
                    mtime: new_mt,
                    is_dir: false,
                },
            ])
            .expect("seed two records");

        // The bound is inclusive: at mtime_before == old_mt only the old
        // file passes (newer file excluded) — pin that exclusion direction.
        let old_only = svc
            .search(SearchQueryDto {
                mtime_before: Some(old_mt),
                limit: 10,
                ..Default::default()
            })
            .expect("before-old search");
        assert_eq!(
            old_only.len(),
            1,
            "newer file must be excluded: {old_only:?}"
        );
        assert_eq!(old_only[0].name, "old.txt");

        // An inclusive boundary means mtime_before == new_mt still returns
        // BOTH files (<= matches at the boundary value itself).
        let both = svc
            .search(SearchQueryDto {
                mtime_before: Some(new_mt),
                limit: 10,
                ..Default::default()
            })
            .expect("inclusive-boundary search");
        assert_eq!(
            both.len(),
            2,
            "inclusive at the exact boundary value: {both:?}"
        );

        // Below the oldest mtime, nothing.
        let none = svc
            .search(SearchQueryDto {
                mtime_before: Some(old_mt - 1),
                limit: 10,
                ..Default::default()
            })
            .expect("before-oldest search");
        assert!(none.is_empty(), "below the oldest mtime: {none:?}");

        // The two-sided window (after old, before new] excludes nothing here;
        // a tighter after bound proves composition.
        let window = svc
            .search(SearchQueryDto {
                mtime_after: Some(old_mt + 1),
                mtime_before: Some(new_mt),
                limit: 10,
                ..Default::default()
            })
            .expect("window search");
        assert_eq!(
            window.len(),
            1,
            "after+before composes into a half-open window: {window:?}"
        );
        assert_eq!(window[0].name, "new.txt");
    }
}
