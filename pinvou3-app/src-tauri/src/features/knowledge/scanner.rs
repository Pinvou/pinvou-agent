//! 全盘遍历器：walkdir + 排除剪枝 → 批量喂 [`Store`]。只取元数据，不读内容。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use walkdir::{DirEntry, WalkDir};

use super::exclude::Excluder;
use super::store::{FileRecord, Store};

/// 单事务批量写入的条数。
const BATCH: usize = 2000;

/// 每写一批后让步，避免后台扫描抢占前台 I/O/CPU（治「扫描时设备卡顿」）。
const THROTTLE_MS: u64 = 4;

/// 从一个根遍历并写入 store。返回**遍历**到的条目数（进度量）。
/// 增量：`existing`(path→mtime,size) 里 mtime+size 都没变的文件直接跳过，不重写、不触发 FTS。
/// 本次遍历到的每个 path 记入 `visited`，调用方据此删除「已消失」的旧条目。
/// `on_progress(walked)` 周期回调；`cancel` 置位时尽快收尾。
pub fn scan(
    root: &Path,
    store: &Store,
    ex: &Excluder,
    cancel: &AtomicBool,
    existing: &HashMap<String, (i64, u64)>,
    visited: &mut HashSet<String>,
    mut on_progress: impl FnMut(u64),
) -> u64 {
    let mut buf: Vec<FileRecord> = Vec::with_capacity(BATCH);
    let mut walked: u64 = 0;

    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !skipped(ex, e));

    for entry in walker {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(entry) = entry else { continue }; // 权限不足等：跳过
        let Some(rec) = to_record(&entry) else {
            continue;
        };
        walked += 1;
        visited.insert(rec.path.clone());
        // 增量：mtime + size 都没变 → 跳过（省去 upsert 写入 + FTS 触发器开销）。
        if let Some(&(mt, sz)) = existing.get(&rec.path) {
            if mt == rec.mtime && sz == rec.size {
                if walked % 5000 == 0 {
                    on_progress(walked);
                }
                continue;
            }
        }
        buf.push(rec);
        if buf.len() >= BATCH {
            let _ = store.upsert_many(&buf);
            buf.clear();
            std::thread::sleep(Duration::from_millis(THROTTLE_MS));
        }
        if walked % 5000 == 0 {
            on_progress(walked);
        }
    }
    if !buf.is_empty() {
        let _ = store.upsert_many(&buf);
    }
    on_progress(walked);
    walked
}

fn skipped(ex: &Excluder, e: &DirEntry) -> bool {
    let name = e.file_name().to_str().unwrap_or("");
    let is_dir = e.file_type().is_dir();
    let ext = if is_dir { None } else { ext_of(e.path()) };
    ex.is_skipped(name, is_dir, ext.as_deref())
}

fn to_record(e: &DirEntry) -> Option<FileRecord> {
    let ft = e.file_type();
    if !ft.is_file() && !ft.is_dir() {
        return None; // symlink/socket/fifo 等不入库
    }
    let path = e.path().to_str()?.to_string();
    let name = e.file_name().to_str()?.to_string();
    let is_dir = ft.is_dir();
    let md = e.metadata().ok();
    let size = if is_dir { 0 } else { md.as_ref().map(|m| m.len()).unwrap_or(0) };
    let mtime = md
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some(FileRecord {
        path,
        name,
        ext: if is_dir { None } else { ext_of(e.path()) },
        size,
        mtime,
        is_dir,
    })
}

/// 小写、无点扩展名。
fn ext_of(p: &Path) -> Option<String> {
    p.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
}

/// 从单个路径 stat 出一条记录（watcher 增量用）。用 `symlink_metadata` 不跟随软链，
/// 与扫描器 `follow_links(false)` 一致；symlink/socket 等返回 None 跳过。
pub(super) fn record_from_path(path: &Path) -> Option<FileRecord> {
    let md = std::fs::symlink_metadata(path).ok()?;
    let ft = md.file_type();
    if !ft.is_file() && !ft.is_dir() {
        return None;
    }
    let is_dir = ft.is_dir();
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some(FileRecord {
        path: path.to_str()?.to_string(),
        name: path.file_name().and_then(|s| s.to_str())?.to_string(),
        ext: if is_dir { None } else { ext_of(path) },
        size: if is_dir { 0 } else { md.len() },
        mtime,
        is_dir,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::knowledge::store::SearchQuery;
    use std::fs;

    /// 在唯一临时目录建一棵小树，验证扫描 + 排除剪枝 + 入库。
    #[test]
    fn scan_indexes_and_prunes() {
        let base = std::env::temp_dir().join(format!("pinvou3_kb_scan_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("Documents")).unwrap();
        fs::create_dir_all(base.join("node_modules/pkg")).unwrap(); // 应被剪枝
        fs::create_dir_all(base.join(".ssh")).unwrap(); // 应被剪枝
        fs::write(base.join("Documents/保险报价单.pdf"), b"hello").unwrap();
        fs::write(base.join("Documents/notes.md"), b"# note").unwrap();
        fs::write(base.join("node_modules/pkg/index.js"), b"x").unwrap();
        fs::write(base.join(".ssh/id_rsa"), b"secret").unwrap();

        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let cancel = AtomicBool::new(false);
        let mut visited = HashSet::new();
        scan(&base, &store, &ex, &cancel, &HashMap::new(), &mut visited, |_| {});

        // 能搜到 Documents 下的文件
        let pdf = store
            .search(&SearchQuery {
                text: Some("保险报价单".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(pdf.len(), 1);

        // node_modules / .ssh 整株被剪
        let js = store
            .search(&SearchQuery {
                text: Some("index.js".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert!(js.is_empty(), "node_modules 应被剪枝");
        let key = store
            .search(&SearchQuery {
                text: Some("id_rsa".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert!(key.is_empty(), ".ssh 应被剪枝");

        let _ = fs::remove_dir_all(&base);
    }
}
