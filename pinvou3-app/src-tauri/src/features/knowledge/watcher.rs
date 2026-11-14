// architecture-guard: allow-target-cfg -- the stat-error-vs-delete test needs a
// chmod-000 parent fixture, and mode bits are a unix concept; the production
// branch under test (stat error != NotFound must not delete) is
// platform-independent, only the fixture cannot be expressed portably.

//! 实时增量索引：notify 监听已纳入的根，文件变化即时更新 L0 元数据库。
//!
//! 事件处理（对齐 file_watcher.rs 的「按存在性判定」做法），对事件里的每条路径：
//! - `symlink_metadata` 成功且是普通文件/目录 → 组件级排除 + [`Excluder::is_skipped`]
//!   （与扫描器同一策略源，含 `is_dir` 上下文，带点目录/无扩展名文件与扫描结果一致）
//!   后 `upsert`
//! - `NotFound`（已删除 / rename 移走）→ [`Store::delete_tree`]：目录记录连带子树
//!   一并删（dir rename/move-out 只会冒出父路径事件），文件退化为精确删除
//! - 其他 stat 错误（权限收紧/挂载抖动）→ 只告警不删，避免把仍存在的记录误删
//!
//! 排除：recursive 监听不会自动剪枝，事件里仍会冒出 node_modules/.ssh 等深处的路径，
//! 先做组件级 [`Excluder::is_excluded_path`]（删除事件同样适用），存活路径再按
//! `is_skipped` 判类型。
//!
//! 已知限制（接线方需知）：
//! - Linux 递归监听 `$HOME` 级根会消耗大量 inotify watch，可能撞
//!   `fs.inotify.max_user_watches`；`watch()` 失败只告警不重试。符号链接根会被
//!   notify 后端静默空 watch（不下探 symlink）。
//! - 兜底重扫是**懒触发**的（进文件管理页才跑 `kb_start_scan`），没有周期重扫；
//!   溢出/掉 watch（Q_OVERFLOW、MaxFilesWatch）只能告警，索引在下次重扫前保持
//!   陈旧——接线方应把 `need_rescan` 告警接成受影响根的重扫触发。
//! - macOS fsevent 的 rename 只报新路径，旧路径记录要等重扫清理；被监听根自身被删
//!   则 watch 永久失效（无父 watch 可重挂）。
//! - 无去抖：同一文件的连发事件各写一次（幂等、量小）；大批量灌入场景由接线方
//!   评估 coalescing（参考 file_watcher.rs 的 trailing-edge 防抖）。
//!
//! 当前未接线：`spawn` 尚无生产调用方（与闭源侧一致为落库待接线状态）；入口由
//! 后续接线方在启动扫描时按根调用。

#![allow(dead_code)] // 库先行：接线前 spawn 无生产调用方（见上方注记）

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::exclude::Excluder;
use super::scanner::{ext_of, record_from_parts};
use super::store::{FileRecord, Store};

/// 启动后台 watcher 线程，监听给定根。spawn 后即返回，线程随 app 存活。
pub fn spawn(store: Store, roots: Vec<PathBuf>) {
    if roots.is_empty() {
        return;
    }
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
        let mut watched = 0usize;
        for root in &roots {
            match watcher.watch(root, RecursiveMode::Recursive) {
                Ok(()) => watched += 1,
                // 多半是 inotify 上限/权限；告警即可，懒触发重扫兜底。
                Err(e) => eprintln!("[kb_watcher] watch({}) failed: {e}", root.display()),
            }
        }
        if watched == 0 {
            eprintln!("[kb_watcher] no root could be watched; idle until restart");
            return;
        }
        eprintln!("[kb_watcher] watching {watched}/{} root(s)", roots.len());

        for res in rx {
            match res {
                Ok(ev) => {
                    // 后端丢事件信号（inotify Q_OVERFLOW / fsevent KERNEL_DROPPED 等，
                    // 以 EventKind::Other + Rescan flag 到达）：这是「增量索引已不可靠」
                    // 的唯一主动通知，必须喊出来，接线方据此安排重扫。
                    if ev.need_rescan() {
                        eprintln!(
                            "[kb_watcher] backend dropped events (rescan requested); \
                             index stays stale until the next scan"
                        );
                    }
                    if matches!(
                        ev.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                    ) {
                        apply(&ex, &store, &ev);
                    }
                }
                // 通道上的 Err：读循环 IO 错误、中途 MaxFilesWatch（notify 之后不再扩
                // watch）等，同样不能静默吞掉。
                Err(e) => eprintln!("[kb_watcher] event error: {e}"),
            }
        }
    });
}

/// 事件准入：只有 Create/Modify/Remove 改变索引状态。Access（读/开/关）与
/// Other（无 Rescan flag 的杂项）不得进入存在性判定——否则一条 Access 事件会
/// 把「已索引但刚被删」的路径当作删除信号以外的误判源（读一个不存在的路径
/// 不意味着索引要删它）。
fn apply(ex: &Excluder, store: &Store, ev: &Event) {
    if !matches!(
        ev.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) {
        return;
    }
    handle_event(ex, store, ev);
}

/// 处理单条事件：按存在性分桶（存活 → upsert，消失 → delete），每事件一次批量写。
/// 独立于线程闭包，便于单元测试。
fn handle_event(ex: &Excluder, store: &Store, ev: &Event) {
    let mut ups: Vec<FileRecord> = Vec::new();
    let mut dels: Vec<String> = Vec::new();
    for path in &ev.paths {
        // 组件级排除（祖先 + 末级的 names/secrets）：churn 目录整条跳过，不 stat。
        if ex.is_excluded_path(path) {
            continue;
        }
        match std::fs::symlink_metadata(path) {
            Ok(md) => {
                let ft = md.file_type();
                if !ft.is_file() && !ft.is_dir() {
                    continue; // symlink/socket/fifo 本体不入库（与扫描器一致）
                }
                let is_dir = ft.is_dir();
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                let ext = if is_dir { None } else { ext_of(path) };
                if ex.is_skipped(name, is_dir, ext.as_deref()) {
                    continue;
                }
                if let Some(rec) = record_from_parts(path, name, is_dir, Some(&md)) {
                    ups.push(rec);
                }
            }
            // 已删除 / rename 移走：删记录；目录连带子树（inotify 对被移走的子树
            // 不会再有事件）。
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                dels.push(path.to_string_lossy().into_owned());
            }
            // 权限收紧/ESTALE 等抖动：不删——误删仍存在的记录比漏一次更新更伤，
            // 懒触发重扫会补齐。
            Err(e) => eprintln!("[kb_watcher] stat({}) failed: {e}", path.display()),
        }
    }
    if !ups.is_empty() {
        let _ = store.upsert_many(&ups);
    }
    if !dels.is_empty() {
        let _ = store.delete_tree(&dels);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{
        AccessKind, AccessMode, CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode,
    };
    use std::fs;
    use std::path::Path;

    fn tmp(tag: &str) -> PathBuf {
        let base =
            std::env::temp_dir().join(format!("pinvou3_kb_watch_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn ev(kind: EventKind, paths: &[&Path]) -> Event {
        let mut e = Event::new(kind);
        for p in paths {
            e = e.add_path(p.to_path_buf());
        }
        e
    }

    fn indexed(store: &Store) -> Vec<String> {
        let mut v: Vec<String> = store.load_index().unwrap().into_keys().collect();
        v.sort();
        v
    }

    /// Create → upsert（字段来自真实 stat）、Remove → delete；Access 事件不进入
    /// 存在性判定（对「已索引但已消失」的路径发 Access 不得触发删除）。
    #[test]
    fn create_and_remove_events_track_the_store() {
        let base = tmp("cr");
        fs::write(base.join("notes.md"), b"# note").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();

        apply(&ex, &store, &ev(creates_file(), &[&base.join("notes.md")]));
        let idx = indexed(&store);
        assert!(idx.contains(&base.join("notes.md").to_string_lossy().into_owned()));
        assert_eq!(store.stats().unwrap().total_files, 1);

        // Access（读/关）不改变索引状态：路径已从盘上消失但事件只是 Access——
        // 若准入失守，这条事件会顺着 NotFound 走到删除分支。
        fs::remove_file(base.join("notes.md")).unwrap();
        apply(
            &ex,
            &store,
            &ev(
                EventKind::Access(AccessKind::Close(AccessMode::Write)),
                &[&base.join("notes.md")],
            ),
        );
        assert_eq!(
            store.stats().unwrap().total_files,
            1,
            "access events must not reach the delete branch"
        );

        apply(
            &ex,
            &store,
            &ev(
                EventKind::Remove(RemoveKind::File),
                &[&base.join("notes.md")],
            ),
        );
        assert!(
            indexed(&store).is_empty(),
            "removed path must leave the index"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// 目录 Remove 事件：目录记录连带子树一并删（inotify 对子树不再有事件），
    /// 只共享前缀字符的兄弟路径不能被误删。
    #[test]
    fn dir_remove_deletes_the_subtree() {
        let base = tmp("dirsub");
        fs::create_dir_all(base.join("项目/sub")).unwrap();
        fs::write(base.join("项目/报告.pdf"), b"p").unwrap();
        fs::write(base.join("项目/sub/a.md"), b"a").unwrap();
        fs::write(base.join("项目备份.md"), b"s").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();

        apply(
            &ex,
            &store,
            &ev(EventKind::Create(CreateKind::Folder), &[&base.join("项目")]),
        );
        apply(
            &ex,
            &store,
            &ev(
                EventKind::Create(CreateKind::File),
                &[
                    &base.join("项目/报告.pdf"),
                    &base.join("项目/sub/a.md"),
                    &base.join("项目备份.md"),
                ],
            ),
        );
        assert_eq!(indexed(&store).len(), 4, "dir + 2 children + sibling");

        fs::remove_dir_all(base.join("项目")).unwrap();
        apply(
            &ex,
            &store,
            &ev(EventKind::Remove(RemoveKind::Folder), &[&base.join("项目")]),
        );

        let idx = indexed(&store);
        assert_eq!(
            idx,
            vec![base.join("项目备份.md").to_string_lossy().into_owned()],
            "subtree gone, prefix-overlapping sibling intact"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// rename（inotify 的 Both 事件带 [from, to]）：旧路径删除、新路径入库。
    #[test]
    fn rename_event_deletes_old_and_upserts_new() {
        let base = tmp("ren");
        fs::write(base.join("old.md"), b"# old").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();

        apply(
            &ex,
            &store,
            &ev(EventKind::Create(CreateKind::File), &[&base.join("old.md")]),
        );
        fs::rename(base.join("old.md"), base.join("新文档.md")).unwrap();
        apply(
            &ex,
            &store,
            &ev(
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                &[&base.join("old.md"), &base.join("新文档.md")],
            ),
        );

        let idx = indexed(&store);
        assert_eq!(
            idx,
            vec![base.join("新文档.md").to_string_lossy().into_owned()]
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// 排除策略与扫描器同源：churn 祖先整条跳过（即使末级是白名单类型）、无扩展名
    /// 敏感文件不入库、带点目录按目录放行、源码排除、白名单类型入库。
    #[test]
    fn exclusion_matches_the_scanner_policy() {
        let base = tmp("excl");
        fs::create_dir_all(base.join("node_modules/pkg")).unwrap();
        fs::create_dir_all(base.join("backup.old")).unwrap();
        fs::write(base.join("node_modules/pkg/保险报价单.pdf"), b"p").unwrap();
        fs::write(base.join(".bash_history"), b"hist").unwrap();
        fs::write(base.join("main.c"), b"int main").unwrap();
        fs::write(base.join("报告.pdf"), b"r").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();

        let creates = EventKind::Create(CreateKind::File);
        for p in [
            "node_modules/pkg/保险报价单.pdf",
            ".bash_history",
            "main.c",
            "报告.pdf",
        ] {
            apply(&ex, &store, &ev(creates, &[&base.join(p)]));
        }
        apply(
            &ex,
            &store,
            &ev(
                EventKind::Create(CreateKind::Folder),
                &[&base.join("backup.old")],
            ),
        );

        let idx = indexed(&store);
        assert_eq!(
            idx,
            vec![
                base.join("backup.old").to_string_lossy().into_owned(),
                base.join("报告.pdf").to_string_lossy().into_owned(),
            ],
            "allowed file + dot-suffixed dir indexed; churn ancestor, extensionless \
             secret and source ext are not"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// 带点目录的删除事件同样要走完（组件级排除不吞它）：目录记录与子树都被清掉，
    /// 而不是像旧版那样被末级扩展名判定挡在删除之外、留陈旧行等重扫。
    #[test]
    fn dot_suffixed_dir_delete_event_cleans_the_record() {
        let base = tmp("dotdir");
        fs::create_dir_all(base.join("backup.old")).unwrap();
        fs::write(base.join("backup.old/报告.pdf"), b"p").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();

        apply(
            &ex,
            &store,
            &ev(
                EventKind::Create(CreateKind::Folder),
                &[&base.join("backup.old")],
            ),
        );
        apply(
            &ex,
            &store,
            &ev(creates_file(), &[&base.join("backup.old/报告.pdf")]),
        );
        assert_eq!(indexed(&store).len(), 2);

        fs::remove_dir_all(base.join("backup.old")).unwrap();
        apply(
            &ex,
            &store,
            &ev(
                EventKind::Remove(RemoveKind::Folder),
                &[&base.join("backup.old")],
            ),
        );

        assert!(
            indexed(&store).is_empty(),
            "dot-suffixed dir rows must be removable by events, not wait for a rescan"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// stat 抖动（父目录权限收紧等）不得把仍存在的记录当删除处理——
    /// `Path::exists()` 会把一切错误折叠成 false，旧版在此误删活记录。
    #[test]
    #[cfg(unix)]
    fn transient_stat_errors_do_not_delete_records() {
        use std::os::unix::fs::PermissionsExt as _;

        let base = tmp("staterr");
        fs::create_dir_all(base.join("sub")).unwrap();
        fs::write(base.join("sub/notes.md"), b"# n").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();

        apply(
            &ex,
            &store,
            &ev(creates_file(), &[&base.join("sub/notes.md")]),
        );
        assert_eq!(store.stats().unwrap().total_files, 1);

        fs::set_permissions(base.join("sub"), fs::Permissions::from_mode(0o000)).unwrap();
        let readable = base.join("sub").read_dir().is_ok();
        fs::set_permissions(base.join("sub"), fs::Permissions::from_mode(0o755)).unwrap();
        if readable {
            // root（或忽略 mode bit 的 fs）绕过权限检查，无法制造 stat 错误，跳过。
            let _ = fs::remove_dir_all(&base);
            return;
        }

        fs::set_permissions(base.join("sub"), fs::Permissions::from_mode(0o000)).unwrap();
        apply(
            &ex,
            &store,
            &ev(
                EventKind::Modify(ModifyKind::Data(DataChange::Any)),
                &[&base.join("sub/notes.md")],
            ),
        );
        fs::set_permissions(base.join("sub"), fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(
            store.stats().unwrap().total_files,
            1,
            "an unstat-able path must not take the delete branch"
        );

        let _ = fs::remove_dir_all(&base);
    }

    fn creates_file() -> EventKind {
        EventKind::Create(CreateKind::File)
    }
}
