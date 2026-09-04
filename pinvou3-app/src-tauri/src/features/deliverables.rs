//! 跨会话交付物索引。
//!
//! 从 `app::commands::artifacts` 回流的纯域逻辑。Tauri 命令薄壳留在
//! `app::commands::artifacts`，本模块只根据会话磁盘真相装配交付物索引。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// 「产出物」跨会话索引:遍历 `~/.pinvou3/sessions/*.json`,把每个会话跟踪的
/// artifacts 汇成一张扁平表(供「产出物」一级入口用)。只走磁盘真相:
/// 文件已被删则跳过;mtime/size 现取 fs。
#[derive(Debug, Clone, Deserialize)]
struct DvSessionView {
    metadata: DvMeta,
    #[serde(default)]
    artifacts: Vec<DvArtifact>,
}
#[derive(Debug, Clone, Deserialize)]
struct DvMeta {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
}
#[derive(Debug, Clone, Deserialize)]
struct DvArtifact {
    storage_path: std::path::PathBuf,
    #[serde(default)]
    byte_size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeliverableItem {
    name: String,
    path: String,
    ext: String,
    category: String,
    #[serde(rename = "sessionId")]
    session_id: String,
    source: String,
    mtime: i64,
    size: u64,
}

const DELIVERABLE_EXTS: &[&str] = &[
    "pptx", "ppt", "docx", "doc", "pdf", "html", "htm", "xlsx", "xls", "md", "csv", "png", "jpg",
    "jpeg", "svg", "gif", "webp", "zip",
];

/// 会话 JSON 解析缓存的条目上限。单条只含 metadata+artifacts，量级很小；
/// 满员逐出一条（哈希表任意序），不整表清空，避免清单刷新时的缓存雪崩。
const DV_VIEW_CACHE_LIMIT: usize = 512;

/// 按 (mtime, len) 缓存每个会话 JSON 解析出的索引视图（metadata+artifacts）。
/// 会话文件每回合整体重写、mtime 必变，缓存只对未变化的文件省去重复整读
/// +解析——正是「产出物」列表高频刷新时的常见情形。视图的其余派生数据
/// （产物文件的 mtime/size 现取 fs、扩展名过滤、排序）每次调用都重新计算，
/// 解析是纯读取、无副作用，缓存值可安全复用。
static DV_VIEW_CACHE: OnceLock<
    Mutex<HashMap<PathBuf, ((Option<SystemTime>, u64), DvSessionView)>>,
> = OnceLock::new();

fn deliverable_category(ext: &str) -> &'static str {
    match ext {
        "html" | "htm" | "mhtml" | "mht" => "web",
        "ppt" | "pptx" | "odp" | "dps" => "ppt",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "heic" => "img",
        _ => "doc",
    }
}

/// 「产出物」跨会话索引的装配实现(`list_deliverable_index` 命令薄壳调它)。
/// 只读磁盘真相:文件已删则跳过;mtime/size 现取 fs;同一物理路径只留最新。
pub(crate) fn list_deliverable_index_impl() -> Vec<DeliverableItem> {
    let sessions_dir = crate::platform::paths::sessions_root();
    let entries = match std::fs::read_dir(&sessions_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut by_path: std::collections::HashMap<String, DeliverableItem> =
        std::collections::HashMap::new();

    for entry in entries.flatten() {
        let file = entry.path();
        if file.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&file) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let stamp = (meta.modified().ok(), meta.len());
        let cache = DV_VIEW_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let cached = {
            let guard = cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard
                .get(&file)
                .filter(|(cached_stamp, _)| *cached_stamp == stamp)
                .map(|(_, view)| view.clone())
        };
        let view = match cached {
            Some(view) => view,
            None => {
                let Ok(raw) = std::fs::read_to_string(&file) else {
                    continue;
                };
                let Ok(view) = serde_json::from_str::<DvSessionView>(&raw) else {
                    continue;
                };
                let mut guard = cache
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                // 满员只逐出一条（理由见 DV_VIEW_CACHE_LIMIT），签名不匹配的
                // 条目靠 (mtime, len) 自然失效。
                if guard.len() >= DV_VIEW_CACHE_LIMIT && !guard.contains_key(&file) {
                    if let Some(evicted) = guard.keys().next().cloned() {
                        guard.remove(&evicted);
                    }
                }
                guard.insert(file.clone(), (stamp, view.clone()));
                view
            }
        };

        for art in view.artifacts {
            let p = &art.storage_path;
            let Ok(meta) = std::fs::metadata(p) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let name = p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if !DELIVERABLE_EXTS.contains(&ext.as_str()) {
                continue;
            }
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let path = p.to_string_lossy().to_string();
            let item = DeliverableItem {
                name,
                path: path.clone(),
                ext: ext.clone(),
                category: deliverable_category(&ext).to_string(),
                session_id: view.metadata.id.clone(),
                source: view.metadata.title.clone(),
                mtime,
                size: if meta.len() > 0 {
                    meta.len()
                } else {
                    art.byte_size
                },
            };
            by_path
                .entry(path)
                .and_modify(|cur| {
                    if item.mtime >= cur.mtime {
                        *cur = item.clone();
                    }
                })
                .or_insert(item);
        }
    }

    let mut out: Vec<DeliverableItem> = by_path.into_values().collect();
    out.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.name.cmp(&b.name)));
    out
}
