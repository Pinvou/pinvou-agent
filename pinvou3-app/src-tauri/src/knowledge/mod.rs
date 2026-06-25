//! 本地知识底座 L0：全系统元数据索引 + 秒搜 + 去重。
//!
//! 见 docs/本地知识底座-产品形态与架构.md。v0 以 in-process 模块落地（复用 `notify`/
//! `bridge::paths`/Tauri 命令通路），用 [`KnowledgeService`]（UI 无关）收口，
//! 便于日后抽成独立 `pinvou3-knowledged` daemon + MCP（`kb_*`）。
//!
//! 分层提醒：本模块只做 **L0 元数据**（零模型）。内容解析 / 全文 / 向量是 L1（后续），
//! LLM 理解是 L2（纯按需）。**绝不在这里全盘跑模型分类**——那是 Marvis 的坑。

mod embed;
mod exclude;
mod kb_tool;
mod l1;
mod query;
mod scanner;
mod store;
/// 实时 watcher 现已不接（懒触发后不常驻，避免监听全 $HOME 长期占 inotify/内存）。
/// 保留模块供未来 daemon 版的「热点 watch + 周期重扫」混合策略复用。
#[allow(dead_code)]
mod watcher;
#[cfg(test)]
mod e2e_test;

pub use exclude::Excluder;
pub use kb_tool::KbSearchTool;
pub use l1::{ChunkHit, Collection, Document};

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::State;
use walkdir::WalkDir;

use store::{SearchQuery, Store};
pub use store::{FileHit, Stats, TypeCount};

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

/// L1 知识集索引进度（解析+切块，回前端轮询）。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IndexState {
    pub running: bool,
    pub collection_id: i64,
    /// idle / parsing / done / cancelled
    pub phase: String,
    pub done: u64,
    pub total: u64,
    pub started_at: i64,
    pub finished_at: i64,
}

/// 知识服务：L0 元数据库 + 后台扫描状态 + L1 知识库(共享同一连接)。Tauri managed state。
pub struct KnowledgeService {
    store: Store,
    l1: l1::L1Store,
    scan_state: Arc<Mutex<ScanState>>,
    cancel: Arc<AtomicBool>,
    index_state: Arc<Mutex<IndexState>>,
    index_cancel: Arc<AtomicBool>,
}

impl KnowledgeService {
    /// 用磁盘库初始化（`~/.pinvou3/knowledge/index.db`）。`model_dir`=embedding 模型目录的
    /// fallback（生产=随 deb 打包的资源目录;dev 走 env 优先,见 embed::from_env_or_dir）。
    pub fn new(db_path: &Path, model_dir: Option<&Path>) -> rusqlite::Result<Self> {
        let store = Store::open(db_path)?;
        // env(dev) 优先,否则用打包资源目录(prod);都无/加载失败则知识库走纯全文(降级)。
        let embedder = embed::Embedder::from_env_or_dir(model_dir).map(Arc::new);
        if let Some(e) = &embedder {
            eprintln!("[knowledge] L1 embedding 已启用: {} ({})", e.model(), e.source());
        }
        let l1 = l1::L1Store::new(store.conn_arc(), embedder);
        Ok(Self {
            store,
            l1,
            scan_state: Arc::new(Mutex::new(ScanState {
                phase: "idle".into(),
                ..Default::default()
            })),
            cancel: Arc::new(AtomicBool::new(false)),
            index_state: Arc::new(Mutex::new(IndexState {
                phase: "idle".into(),
                ..Default::default()
            })),
            index_cancel: Arc::new(AtomicBool::new(false)),
        })
    }

    /// L1 知识集句柄（命令层直接用）。
    pub fn l1(&self) -> &l1::L1Store {
        &self.l1
    }

    pub fn index_status(&self) -> IndexState {
        self.index_state.lock().clone()
    }

    pub fn cancel_index(&self) {
        self.index_cancel.store(true, Ordering::Relaxed);
    }

    /// 后台把若干路径(文件或目录)加入知识集：展开目录→解析→切块→入库。限速、可中断。
    pub fn start_index(&self, collection_id: i64, roots: Vec<PathBuf>) -> IndexState {
        {
            let mut st = self.index_state.lock();
            if st.running {
                return st.clone();
            }
            self.index_cancel.store(false, Ordering::Relaxed);
            *st = IndexState {
                running: true,
                collection_id,
                phase: "parsing".into(),
                started_at: now(),
                ..Default::default()
            };
        }
        self.l1.set_collection_status(collection_id, "indexing");

        let l1 = self.l1.clone();
        let index_state = self.index_state.clone();
        let cancel = self.index_cancel.clone();
        thread::spawn(move || {
            // 展开目录(复用 Excluder 跳过 churn/隐私)，收集待解析文件。
            let ex = Excluder::default();
            let mut files: Vec<PathBuf> = Vec::new();
            for root in &roots {
                if root.is_file() {
                    files.push(root.clone());
                    continue;
                }
                let walker = WalkDir::new(root).follow_links(false).into_iter().filter_entry(|e| {
                    let name = e.file_name().to_str().unwrap_or("");
                    let is_dir = e.file_type().is_dir();
                    let ext = if is_dir {
                        None
                    } else {
                        e.path().extension().and_then(|s| s.to_str()).map(|s| s.to_lowercase())
                    };
                    !ex.is_skipped(name, is_dir, ext.as_deref())
                });
                for entry in walker.flatten() {
                    if entry.file_type().is_file() {
                        files.push(entry.path().to_path_buf());
                    }
                }
            }
            index_state.lock().total = files.len() as u64;

            for (i, f) in files.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let _ = l1.ingest_file(collection_id, f);
                index_state.lock().done = (i + 1) as u64;
                std::thread::sleep(Duration::from_millis(3)); // 让步前台
            }

            let cancelled = cancel.load(Ordering::Relaxed);
            l1.set_collection_status(collection_id, "ready");
            let mut st = index_state.lock();
            st.running = false;
            st.finished_at = now();
            st.phase = if cancelled { "cancelled" } else { "done" }.into();
        });
        self.index_state.lock().clone()
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
            let mut st = scan_state.lock();
            st.running = false;
            st.finished_at = now();
            st.phase = if cancel.load(Ordering::Relaxed) {
                "cancelled"
            } else {
                "done"
            }
            .into();
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

/// `~/.pinvou3/knowledge/index.db`。
pub fn default_db_path() -> PathBuf {
    crate::bridge::paths::pinvou3_home()
        .join("knowledge")
        .join("index.db")
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
#[tauri::command]
pub fn kb_start_scan(
    state: State<'_, KnowledgeService>,
    roots: Option<Vec<String>>,
) -> ScanState {
    let roots = roots
        .filter(|v| !v.is_empty())
        .map(|v| v.into_iter().map(PathBuf::from).collect())
        .unwrap_or_else(|| vec![crate::bridge::paths::user_home_dir()]);
    state.start_scan(roots)
}

#[tauri::command]
pub fn kb_scan_status(state: State<'_, KnowledgeService>) -> ScanState {
    state.status()
}

#[tauri::command]
pub fn kb_cancel_scan(state: State<'_, KnowledgeService>) {
    state.cancel_scan();
}


/// L0：按扩展名分类计数（文件管理「按类型浏览」用）。
#[tauri::command]
pub async fn kb_type_counts(state: State<'_, KnowledgeService>) -> Result<Vec<TypeCount>, String> {
    let store = state.store.clone();
    spawn_db(move || store.type_counts().map_err(|e| e.to_string())).await
}

// ───────────────────────── L1 知识库命令 ─────────────────────────

#[tauri::command]
pub async fn kb_collection_list(state: State<'_, KnowledgeService>) -> Result<Vec<Collection>, String> {
    let l1 = state.l1().clone();
    spawn_db(move || l1.list_collections().map_err(|e| e.to_string())).await
}

#[tauri::command]
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

#[tauri::command]
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

#[tauri::command]
pub async fn kb_collection_delete(state: State<'_, KnowledgeService>, id: i64) -> Result<(), String> {
    let l1 = state.l1().clone();
    spawn_db(move || l1.delete_collection(id).map_err(|e| e.to_string())).await
}

/// 把文件/目录加入知识集，后台解析+切块+入库。进度走 kb_index_status。
#[tauri::command]
pub fn kb_collection_add_sources(
    state: State<'_, KnowledgeService>,
    collection_id: i64,
    paths: Vec<String>,
) -> IndexState {
    let roots = paths.into_iter().map(PathBuf::from).collect();
    state.start_index(collection_id, roots)
}

#[tauri::command]
pub fn kb_index_status(state: State<'_, KnowledgeService>) -> IndexState {
    state.index_status()
}

#[tauri::command]
pub fn kb_index_cancel(state: State<'_, KnowledgeService>) {
    state.cancel_index();
}

/// 列出知识集文档（collectionId<=0 列出全部知识集，给「知识库内文件」表）。
#[tauri::command]
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

#[tauri::command]
pub async fn kb_remove_document(state: State<'_, KnowledgeService>, doc_id: i64) -> Result<(), String> {
    let l1 = state.l1().clone();
    spawn_db(move || l1.remove_document(doc_id).map_err(|e| e.to_string())).await
}

/// 语义检索(embedding)状态，给前端显示「语义检索:已启用/未配置」。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbedInfo {
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
}

#[tauri::command]
pub fn kb_embed_info(state: State<'_, KnowledgeService>) -> EmbedInfo {
    match state.l1().embed_info() {
        Some((base_url, model)) => EmbedInfo { enabled: true, base_url, model },
        None => EmbedInfo { enabled: false, base_url: String::new(), model: String::new() },
    }
}

/// 秒搜。文本会先过 NL 规则解析（"上周的 pdf" → exts+时间过滤+残余文本）；
/// 前端**显式**传入的结构化过滤优先于解析结果，不被覆盖。
#[tauri::command]
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

#[tauri::command]
pub async fn kb_stats(state: State<'_, KnowledgeService>) -> Result<Stats, String> {
    let store = state.store.clone();
    spawn_db(move || store.stats().map_err(|e| e.to_string())).await
}

