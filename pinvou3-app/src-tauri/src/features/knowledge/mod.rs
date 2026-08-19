//! 本地知识底座 L0：全系统元数据索引 + 秒搜 + 去重。
//!
//! 见 docs/本地知识底座-产品形态与架构.md。v0 以 in-process 模块落地（复用 `notify`/
//! `bridge::paths`/Tauri 命令通路），用 [`KnowledgeService`]（UI 无关）收口，
//! 便于日后抽成独立 `pinvou3-knowledged` daemon + MCP（`kb_*`）。
//!
//! 分层提醒：本模块只做 **L0 元数据**（零模型）。内容解析 / 全文 / 向量是 L1（后续），
//! LLM 理解是 L2（纯按需）。**绝不在这里全盘跑模型分类**——那是 Marvis 的坑。

#[cfg(test)]
mod e2e_test;
mod embed;
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
/// 实时 watcher 现已不接（懒触发后不常驻，避免监听全 $HOME 长期占 inotify/内存）。
/// 保留模块供未来 daemon 版的「热点 watch + 周期重扫」混合策略复用。
#[allow(dead_code)]
mod watcher;

pub use exclude::Excluder;
pub use import_jobs::{FailedImportFilePage, ImportJobState as IndexState};
pub use kb_tool::{KbOpenSourceTool, KbSearchTool};
pub use l1::{Collection, Document};

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::State;
use walkdir::WalkDir;

pub use store::{FileHit, Stats, TypeCount};
use store::{SearchQuery, Store};

/// 后台扫描进度（回前端轮询）。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScanState {
    pub running: bool,
    /// idle / scanning / deduping / done / cancelled
    pub phase: String,
    pub roots: Vec<String>,
    pub scanned: u64,
    pub dedup_done: u64,
    pub dedup_total: u64,
    pub started_at: i64,
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
        let store = Store::open(db_path)?;
        let last_scan_finished_at = store.last_scan_finished_at().unwrap_or(0);
        let conn = store.conn_arc();
        let l1 = l1::L1Store::new(conn.clone(), None);
        let imports = import_jobs::ImportJobStore::new(conn);
        let interrupted = imports.recover_interrupted()?;
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
    fn load_embedder(model_dir: Option<&Path>) -> Result<Arc<embed::Embedder>, String> {
        crate::platform::os::configure_onnxruntime_dylib()?;
        embed::Embedder::from_env_or_dir(model_dir).map(Arc::new)
    }

    /// 严格从调用方指定目录构建 embedding，不读取开发环境的模型目录覆盖。
    /// 下载修复必须使用该入口验证候选目录，避免验证了外部目录却替换托管目录。
    fn load_embedder_from_dir(model_dir: &Path) -> Result<Arc<embed::Embedder>, String> {
        crate::platform::os::configure_onnxruntime_dylib()?;
        let name = std::env::var("PINVOU3_KB_EMBED_MODEL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| model_download::MODEL_VERSION.to_string());
        embed::Embedder::from_dir(model_dir, &name)
            .map(Arc::new)
            .map_err(|error| format!("embedding 模型加载失败({}): {error}", model_dir.display()))
    }

    /// 将后台构建完成的模型原子换入共享槽；所有 L1Store clone 立即可见。
    /// 同时重置空闲时钟并确保巡检在跑：热加载（下载完成 / kb_model_status
    /// 状态查询 / 导入前补载）都视为用户意图，从加载时刻重新计空闲。
    fn install_embedder(&self, embedder: Arc<embed::Embedder>) -> bool {
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
                // 条件逐轮重读：模型可能已被热卸载/热加载。
                let should_unload = l1.has_embedder()
                    && l1.embedder_idle_secs() >= EMBEDDER_IDLE_UNLOAD_AFTER_SECS
                    && now() - last_unload.load(Ordering::Acquire) >= EMBEDDER_UNLOAD_COOLDOWN_SECS;
                if should_unload {
                    eprintln!(
                        "[knowledge] embedding 模型空闲超过 {} 秒，卸载释放内存（下次状态查询/导入自动重载）",
                        EMBEDDER_IDLE_UNLOAD_AFTER_SECS
                    );
                    // set_embedder(None) 后所有 L1Store clone 即刻回到纯全文检索；
                    // 正在跑的推理持有 Arc 副本，跑完自然释放。
                    l1.set_embedder(None);
                    last_unload.store(now(), Ordering::Release);
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
        load: impl FnOnce() -> Result<Arc<embed::Embedder>, String>,
    ) {
        if self.l1.has_embedder() || !model_installed {
            return;
        }
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

    /// 热加载 embedding 模型（按需下载完成后调）：按 dev-env 优先 / 下载落点兜底重新定位并加载，
    /// 换进所有在跑会话/后台线程共享的 embedder 槽，**免重启**。返回是否就绪。
    pub fn reload_embedder(&self) -> Result<bool, String> {
        let embedder = Self::load_embedder(Some(&model_dir()))?;
        Ok(self.install_embedder(embedder))
    }

    /// 知识库是否有任何已入库内容（任一知识集存在文档）。门控 kb_search/kb_open_source
    /// 工具的对外可见性：
    /// 为空时把工具加入引擎 disallowed → 模型看不到，AI 不再宣称「能本地知识库检索」。
    /// 读失败保守按「无内容」处理（宁可隐藏也不误宣传能力）。
    pub fn has_indexed_content(&self) -> bool {
        self.l1.has_any_document().unwrap_or(false)
    }

    pub fn index_status(&self) -> IndexState {
        self.imports
            .latest_state()
            .ok()
            .flatten()
            .unwrap_or_else(|| IndexState {
                phase: "idle".into(),
                ..Default::default()
            })
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
                started_at: now(),
                ..Default::default()
            };
        }

        let store = self.store.clone();
        let scan_state = self.scan_state.clone();
        let cancel = self.cancel.clone();

        thread::spawn(move || {
            let ex = Excluder::default();
            // 增量：载入现有快照，scanner 只写 mtime/size 变化的文件，未变的跳过。
            let existing = store.load_index().unwrap_or_default();
            let mut visited = std::collections::HashSet::new();
            let mut scanned_total = 0u64;
            for root in &roots {
                let base = scanned_total;
                let walked =
                    scanner::scan(root, &store, &ex, &cancel, &existing, &mut visited, |n| {
                        scan_state.lock().scanned = base + n;
                    });
                scanned_total = base + walked;
                scan_state.lock().scanned = scanned_total;
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
            }

            // 清理「已消失」的文件（上次在库、本次没遍历到）。取消时不删，避免误删没扫完的部分。
            if !cancel.load(Ordering::Relaxed) {
                let stale: Vec<String> = existing
                    .keys()
                    .filter(|p| !visited.contains(*p))
                    .cloned()
                    .collect();
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
        });

        self.scan_state.lock().clone()
    }

    pub fn cancel_scan(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn status(&self) -> ScanState {
        self.scan_state.lock().clone()
    }
}

/// 后台索引入口的补载实现已上收到 `KnowledgeService::
/// reload_embedder_if_import_needed`（导入线程持有服务句柄，补载必须经
/// install_embedder 启动空闲巡检，自由函数直写 l1 槽会绕过巡检启动）。

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
        let walker = WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                let name = entry.file_name().to_str().unwrap_or("");
                let is_dir = entry.file_type().is_dir();
                let ext = if is_dir {
                    None
                } else {
                    entry
                        .path()
                        .extension()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_lowercase())
                };
                !ex.is_skipped(name, is_dir, ext.as_deref())
            });
        for entry in walker.flatten() {
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
/// dev 仍可用 env `PINVOU3_KB_EMBED_MODEL_DIR` 覆盖（见 embed::from_env_or_dir）。
pub fn model_dir() -> PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("knowledge")
        .join("models")
        .join("bge-m3")
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
pub fn kb_cancel_scan(state: State<'_, KnowledgeService>) {
    state.cancel_scan();
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

/// 秒搜。文本会先过 NL 规则解析（"上周的 pdf" → exts+时间过滤+残余文本）；
/// 前端**显式**传入的结构化过滤优先于解析结果，不被覆盖。
pub async fn kb_search(
    state: State<'_, KnowledgeService>,
    query: SearchQueryDto,
) -> Result<Vec<FileHit>, String> {
    let mut sq: SearchQuery = query.into();
    if let Some(text) = sq.text.clone() {
        let parsed = query::parse(&text);
        sq.text = parsed.text; // 残余文本（已剥离时间/类型/大小词）
        if sq.exts.is_empty() {
            sq.exts = parsed.exts;
        }
        if sq.mtime_after.is_none() {
            sq.mtime_after = parsed.mtime_after;
        }
        if sq.mtime_before.is_none() {
            sq.mtime_before = parsed.mtime_before;
        }
        if sq.min_size.is_none() {
            sq.min_size = parsed.min_size;
        }
        if sq.max_size.is_none() {
            sq.max_size = parsed.max_size;
        }
    }
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
        let dir = std::env::temp_dir().join(format!(
            "pinvou3_kb_import_reload_{}_{}",
            std::process::id(),
            line!()
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
}
