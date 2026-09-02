//! 实时增量索引：notify 监听已纳入的根，文件变化即时更新 L0 元数据库。
//!
//! 事件处理（对齐 file_watcher.rs 的「按存在性判定」做法）：
//! - 路径仍存在 → `upsert`（新建/修改）
//! - 路径已消失 → `delete_by_path`（删除 / rename 移走）
//!
//! 排除：recursive 监听不会自动剪枝，事件里仍会冒出 node_modules/.cache 等路径，
//! 用 [`Excluder::is_excluded_path`] 在事件级过滤掉，避免污染索引。
//!
//! 已知限制（见 docs §4.2）：监听整个 `$HOME` 可能撞 `fs.inotify.max_user_watches`。
//! 这里对 `watch()` 失败只告警不崩；**周期重扫（kb_start_scan）是兜底**。daemon 版再上
//! 「热点 watch + 周期重扫」的混合策略。

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::exclude::Excluder;
use super::scanner::record_from_path;
use super::store::Store;

/// 启动后台 watcher 线程，监听给定根。spawn 后即返回，线程随 app 存活。
pub fn spawn(store: Store, roots: Vec<PathBuf>) {
    thread::spawn(move || {
        let ex = Excluder::default();
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        let mut watcher: RecommendedWatcher = match notify::recommended_watcher(tx) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("[kb_watcher] init failed: {e}");
                return;
            }
        };
        for root in &roots {
            if let Err(e) = watcher.watch(root, RecursiveMode::Recursive) {
                // 多半是 inotify 上限/权限；告警即可，周期重扫兜底。
                eprintln!("[kb_watcher] watch({}) failed: {e}", root.display());
            }
        }
        eprintln!("[kb_watcher] watching {} root(s)", roots.len());

        for res in rx {
            let Ok(ev) = res else { continue };
            if !matches!(
                ev.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            ) {
                continue;
            }
            for path in &ev.paths {
                if ex.is_excluded_path(path) {
                    continue;
                }
                if path.exists() {
                    if let Some(rec) = record_from_path(path) {
                        let _ = store.upsert_many(&[rec]);
                    }
                } else {
                    let _ = store.delete_by_path(&path.to_string_lossy());
                }
            }
        }
    });
}
