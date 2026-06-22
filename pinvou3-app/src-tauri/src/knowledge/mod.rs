//! 本地知识底座 L0：全系统元数据索引 + 秒搜 + 去重。
//!
//! 见 docs/本地知识底座-产品形态与架构.md。v0 以 in-process 模块落地（复用 `notify`/
//! `bridge::paths`/Tauri 命令通路），用 [`KnowledgeService`]（UI 无关）收口，
//! 便于日后抽成独立 `pinvou3-knowledged` daemon + MCP（`kb_*`）。
//!
//! 分层提醒：本模块只做 **L0 元数据**（零模型）。内容解析 / 全文 / 向量是 L1（后续），
//! LLM 理解是 L2（纯按需）。**绝不在这里全盘跑模型分类**——那是 Marvis 的坑。

mod dedup;
mod exclude;
mod query;
mod scanner;
mod store;
mod watcher;

pub use exclude::Excluder;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::State;

use store::{SearchQuery, Store};
pub use store::{DupGroup, FileHit, Stats};

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

/// L0 知识服务：持有元数据库 + 后台扫描状态。Tauri managed state。
pub struct KnowledgeService {
    store: Store,
    scan_state: Arc<Mutex<ScanState>>,
    cancel: Arc<AtomicBool>,
    /// 实时 watcher 只起一次（首次 start_scan 时）。
    watcher_started: Arc<AtomicBool>,
}

impl KnowledgeService {
    /// 用磁盘库初始化（`~/.pinvou3/knowledge/index.db`）。
    pub fn new(db_path: &Path) -> rusqlite::Result<Self> {
        Ok(Self {
            store: Store::open(db_path)?,
            scan_state: Arc::new(Mutex::new(ScanState {
                phase: "idle".into(),
                ..Default::default()
            })),
            cancel: Arc::new(AtomicBool::new(false)),
            watcher_started: Arc::new(AtomicBool::new(false)),
        })
    }

    /// 启动后台全盘扫描（已在跑则原样返回当前状态）。立即返回，扫描在独立线程跑。
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

        // 首次扫描时把实时 watcher 起在同一批根上（只起一次）。
        if !self.watcher_started.swap(true, Ordering::Relaxed) {
            watcher::spawn(self.store.clone(), roots.clone());
        }

        let store = self.store.clone();
        let scan_state = self.scan_state.clone();
        let cancel = self.cancel.clone();

        thread::spawn(move || {
            let ex = Excluder::default();
            let mut scanned_total = 0u64;
            for root in &roots {
                let base = scanned_total;
                scanner::scan(root, &store, &ex, &cancel, |n| {
                    scan_state.lock().scanned = base + n;
                });
                scanned_total = scan_state.lock().scanned;
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
            }

            let cancelled = cancel.load(Ordering::Relaxed);
            scan_state.lock().phase = if cancelled { "cancelled" } else { "deduping" }.into();
            if !cancelled {
                let _ = dedup::run(&store, &cancel, |done, total| {
                    let mut st = scan_state.lock();
                    st.dedup_done = done;
                    st.dedup_total = total;
                });
            }

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

/// 秒搜。文本会先过 NL 规则解析（"上周的 pdf" → exts+时间过滤+残余文本）；
/// 前端**显式**传入的结构化过滤优先于解析结果，不被覆盖。
#[tauri::command]
pub fn kb_search(
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
    state.store.search(&sq).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn kb_stats(state: State<'_, KnowledgeService>) -> Result<Stats, String> {
    state.store.stats().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn kb_find_duplicates(
    state: State<'_, KnowledgeService>,
    limit: Option<usize>,
) -> Result<Vec<DupGroup>, String> {
    state
        .store
        .duplicate_groups(limit.unwrap_or(200))
        .map_err(|e| e.to_string())
}
