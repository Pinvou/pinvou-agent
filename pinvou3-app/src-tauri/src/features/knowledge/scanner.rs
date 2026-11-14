//! 全盘遍历器：walkdir + 排除剪枝 → 批量喂 [`Store`]。只取元数据，不读内容。

// architecture-guard: allow-target-cfg -- the walk-error counting test needs
// a chmod-000 subtree fixture, and mode bits are a unix concept; the veto
// logic under test is platform-independent (any walkdir error), only the
// fixture cannot be expressed portably.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use walkdir::{DirEntry, WalkDir};

use super::exclude::Excluder;
use super::store::{FileRecord, Store};

/// 单事务批量写入的条数。
const BATCH: usize = 2000;

/// 每写一批后让步，避免后台扫描抢占前台 I/O/CPU（治「扫描时设备卡顿」）。
const THROTTLE_MS: u64 = 4;

/// Raw entries enumerated between `on_raw` heartbeats — and, at the same
/// cadence, the walked-entry `on_progress` ticks in [`scan`], so the two
/// heartbeats share one constant and cannot drift apart. Deliberately
/// counted pre-prune: a tree of millions of disallowed files yields almost
/// nothing countable, and the headless `scan start` stall bound (300 s
/// default) watches progress — without a raw tick a healthy walk over a
/// pruned-heavy tree is indistinguishable from a wedged thread and gets
/// killed mid-walk.
const HEARTBEAT: u64 = 5000;

/// 共享的剪枝遍历：walkdir + [`Excluder::is_skipped`] 逐层剪枝，不跟随软链。
/// `scan`（全盘扫描）与知识库导入的 `expand_import_roots`（mod.rs）共用，
/// 保证两条入口的排除语义一致。遍历错误（权限不足等）以 `Err` 透传给调用方
/// 自行处置：全盘扫描把它们计入根的 walk 错误数（陈旧删除授权的否决项），
/// 导入侧自行 flatten 跳过——两条入口对错误的处置不同，跳过语义因此不再
/// 藏在这一层。
pub(super) fn walk_pruned_with(
    root: &Path,
    ex: &Excluder,
    mut on_raw: impl FnMut(),
) -> impl Iterator<Item = walkdir::Result<DirEntry>> {
    let mut raw = 0u64;
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(move |e| {
            raw += 1;
            if raw.is_multiple_of(HEARTBEAT) {
                on_raw();
            }
            !skipped(ex, e)
        })
}

/// 从一个根遍历并写入 store。返回**遍历**到的条目数（进度量）与**遍历错误数**。
/// 增量：`existing`(path→mtime,size) 里 mtime+size 都没变的文件直接跳过，不重写、不触发 FTS。
/// 本次遍历到的每个 path 记入 `visited`，调用方据此删除「已消失」的旧条目。
/// 错误数交给调用方解读：任何一层读不了（权限/EIO）都意味着这个根的切片
/// 去留不可判定，全盘扫描据此否决该根的陈旧删除授权（见
/// `root_authorizes_deletion`），导入侧忽略。
pub fn scan(
    root: &Path,
    store: &Store,
    ex: &Excluder,
    existing: &HashMap<String, (i64, u64)>,
    visited: &mut HashSet<String>,
    mut on_progress: impl FnMut(u64),
    mut on_raw: impl FnMut(),
) -> (u64, u64) {
    let mut buf: Vec<FileRecord> = Vec::with_capacity(BATCH);
    let mut walked: u64 = 0;
    let mut walk_errors: u64 = 0;

    for entry in walk_pruned_with(root, ex, &mut on_raw) {
        let Ok(entry) = entry else {
            walk_errors += 1;
            continue;
        };
        let Some(rec) = to_record(&entry) else {
            continue;
        };
        walked += 1;
        visited.insert(rec.path.clone());
        // 增量：mtime + size 都没变 → 跳过（省去 upsert 写入 + FTS 触发器开销）。
        if let Some(&(mt, sz)) = existing.get(&rec.path) {
            if mt == rec.mtime && sz == rec.size {
                if walked.is_multiple_of(HEARTBEAT) {
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
        if walked.is_multiple_of(HEARTBEAT) {
            on_progress(walked);
        }
    }
    if !buf.is_empty() {
        let _ = store.upsert_many(&buf);
    }
    on_progress(walked);
    (walked, walk_errors)
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
    // metadata 失败（条目被并发删除）时仍入一条 size/mtime=0 的占位记录，保持既有行为。
    record_from_parts(
        e.path(),
        e.file_name().to_str()?,
        ft.is_dir(),
        e.metadata().ok().as_ref(),
    )
}

/// FileRecord 字段装配：扫描器（`to_record`，DirEntry）与 watcher（`record_from_parts`，
/// 事件路径 stat）共用一份策略——目录 size=0/ext=None、mtime 秒级、历元前取 0，
/// 两条路的记录字段不会漂移。路径/文件名非 UTF-8 返回 None（与库内只存 UTF-8 的
/// 既有约束一致）。`md` 传 None 时 size/mtime 取 0。
pub(super) fn record_from_parts(
    path: &Path,
    name: &str,
    is_dir: bool,
    md: Option<&std::fs::Metadata>,
) -> Option<FileRecord> {
    Some(FileRecord {
        path: path.to_str()?.to_string(),
        name: name.to_string(),
        ext: if is_dir { None } else { ext_of(path) },
        size: if is_dir {
            0
        } else {
            md.map(|m| m.len()).unwrap_or(0)
        },
        mtime: md
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        is_dir,
    })
}

/// 小写、无点扩展名。
pub(super) fn ext_of(p: &Path) -> Option<String> {
    p.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
}

/// 从单个路径 stat 出一条记录（watcher 增量用的单路径入口）。用 `symlink_metadata`
/// 不跟随软链，与扫描器 `follow_links(false)` 一致；symlink/socket 等返回 None 跳过。
#[allow(dead_code)] // watcher 库先行：接线前仅测试引用（watcher 走 record_from_parts stat 先行）
pub(super) fn record_from_path(path: &Path) -> Option<FileRecord> {
    let md = std::fs::symlink_metadata(path).ok()?;
    let ft = md.file_type();
    if !ft.is_file() && !ft.is_dir() {
        return None;
    }
    record_from_parts(
        path,
        path.file_name().and_then(|s| s.to_str())?,
        ft.is_dir(),
        Some(&md),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::knowledge::store::SearchQuery;
    use std::fs;

    /// record_from_path 的字段装配：文件（name/ext/size/mtime）、目录（size=0、
    /// ext=None）、symlink 拒收——watcher 增量与扫描器两条路共用的入库形状。
    #[test]
    fn record_from_path_builds_records_like_the_scanner() {
        let base = std::env::temp_dir().join(format!("pinvou3_kb_rfp_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("报告.pdf"), b"hello pdf").unwrap();

        let file = record_from_path(&base.join("报告.pdf")).unwrap();
        assert_eq!(file.name, "报告.pdf");
        assert_eq!(file.ext.as_deref(), Some("pdf"));
        assert_eq!(file.size, 9);
        assert!(!file.is_dir);
        assert!(file.mtime > 0, "mtime 装配自 stat，不应退化为 0");

        let dir = record_from_path(&base).unwrap();
        assert!(dir.is_dir);
        assert_eq!(dir.ext, None, "目录不入扩展名");
        assert_eq!(dir.size, 0);

        // 带点目录名同样不入扩展名（fixture 必须带点，否则 ext_of 本来就返回
        // None，is_dir 分支咬不住）。
        fs::create_dir_all(base.join("backup.old")).unwrap();
        let dotted_dir = record_from_path(&base.join("backup.old")).unwrap();
        assert!(dotted_dir.is_dir);
        assert_eq!(dotted_dir.ext, None, "带点目录名的 ext 仍须为 None");

        // symlink 本体不入库（symlink_metadata 不跟随），与扫描器一致。
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(base.join("报告.pdf"), base.join("link.pdf")).unwrap();
            assert!(
                record_from_path(&base.join("link.pdf")).is_none(),
                "symlink 本体不入库"
            );
        }

        let _ = fs::remove_dir_all(&base);
    }

    /// 非 UTF-8 路径：与扫描器同语义（to_str() 失败即跳过），不得 panic。
    #[test]
    #[cfg(unix)]
    fn record_from_path_skips_non_utf8_paths() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;

        let base =
            std::env::temp_dir().join(format!("pinvou3_kb_rfp_nonutf8_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let p = base.join(OsStr::from_bytes(b"\xff\xfe.bad"));
        fs::write(&p, b"x").unwrap();

        assert!(record_from_path(&p).is_none());

        let _ = fs::remove_dir_all(&base);
    }

    /// 不可读子树的 walk 错误必须被计数（全盘扫描据此否决该根的陈旧删除
    /// 授权，见 mod.rs `root_authorizes_deletion`）：根本体可走、子树
    /// chmod 000 → (walked>0, errors>0)；恢复权限后重扫 errors=0。
    #[test]
    #[cfg(unix)]
    fn scan_counts_walk_errors_under_an_unreadable_subtree() {
        use std::os::unix::fs::PermissionsExt as _;

        let base = std::env::temp_dir().join(format!("pinvou3_kb_scan_err_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("sub")).unwrap();
        fs::write(base.join("readable.txt"), b"hello").unwrap();
        fs::write(base.join("sub/locked.md"), b"# locked").unwrap();

        fs::set_permissions(base.join("sub"), fs::Permissions::from_mode(0o000)).unwrap();
        if base.join("sub").read_dir().is_ok() {
            // Running as root (or on a filesystem that ignores mode bits)
            // bypasses the permission check, so the veto cannot be exercised
            // this way; skip instead of failing spuriously.
            let _ = fs::set_permissions(base.join("sub"), fs::Permissions::from_mode(0o755));
            let _ = fs::remove_dir_all(&base);
            return;
        }

        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let mut visited = HashSet::new();
        let (walked, errors) = scan(
            &base,
            &store,
            &ex,
            &HashMap::new(),
            &mut visited,
            |_| {},
            || {},
        );
        assert!(walked >= 1, "the readable slice still walks");
        assert!(
            errors >= 1,
            "the unreadable subtree must surface as walk errors, not vanish"
        );

        fs::set_permissions(base.join("sub"), fs::Permissions::from_mode(0o755)).unwrap();
        let mut visited_again = HashSet::new();
        let (_, errors_again) = scan(
            &base,
            &store,
            &ex,
            &HashMap::new(),
            &mut visited_again,
            |_| {},
            || {},
        );
        assert_eq!(
            errors_again, 0,
            "a fully readable root reports no walk errors"
        );
        assert!(
            visited_again.contains(&base.join("sub/locked.md").to_string_lossy().into_owned()),
            "the previously locked file is indexed once the subtree is readable"
        );
        let _ = fs::remove_dir_all(&base);
    }

    /// A tree of millions of disallowed files yields almost nothing
    /// countable, so the countable-only progress callback would never fire
    /// and the headless `scan start` stall bound would kill a HEALTHY walk
    /// mid-tree (round-35 minor). The raw heartbeat must tick pre-prune.
    #[test]
    fn raw_heartbeat_ticks_on_a_pruned_heavy_tree() {
        let base = std::env::temp_dir().join(format!("pinvou3_kb_scan_raw_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        for i in 0..5001 {
            // `.vmdk` is on the skip extension list: every entry prunes, so
            // the countable walk stays at zero.
            fs::write(base.join(format!("disk-{i}.vmdk")), b"x").unwrap();
        }

        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let mut visited = HashSet::new();
        let mut raw_ticks = 0u64;
        let mut progress_calls = 0u32;
        let (walked, _errors) = scan(
            &base,
            &store,
            &ex,
            &HashMap::new(),
            &mut visited,
            |_| progress_calls += 1,
            || raw_ticks += 1,
        );
        // Only the root directory itself is countable (every FILE is
        // pruned); a healthy walk over a much larger pruned-heavy tree
        // would stay at single digits for its whole duration.
        assert_eq!(walked, 1, "every file is pruned");
        // The countable lane must have stayed silent through the walk (only
        // the unconditional final report at walked=0 fires): without the raw
        // heartbeat this tree spends the headless stall bound with no
        // progress at all.
        assert!(
            progress_calls <= 1,
            "a fully pruned tree must not report intermediate countable progress: \
             {progress_calls}"
        );
        assert!(
            raw_ticks >= 1,
            "the raw heartbeat must have ticked: {raw_ticks}"
        );
        let _ = fs::remove_dir_all(&base);
    }

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
        let mut visited = HashSet::new();
        scan(
            &base,
            &store,
            &ex,
            &HashMap::new(),
            &mut visited,
            |_| {},
            || {},
        );

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
