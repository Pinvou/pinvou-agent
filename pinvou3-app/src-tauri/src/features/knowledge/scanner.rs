//! 全盘遍历器：walkdir + 排除剪枝 → 批量喂 [`Store`]。只取元数据，不读内容。

// architecture-guard: allow-target-cfg -- the walk-error counting test needs
// a chmod-000 subtree fixture, and mode bits are a unix concept; the veto
// logic under test is platform-independent (any walkdir error), only the
// fixture cannot be expressed portably. The same class covers this file's
// other unix-only test fixtures (the non-UTF-8 scan fixture via OsStrExt raw
// bytes, and the record_from_path test's symlink creation, likewise
// unexpressible portably).

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
    // A metadata failure (the entry was concurrently deleted) still stores a
    // placeholder row with size/mtime=0, preserving existing behavior.
    record_from_parts(
        e.path(),
        e.file_name().to_str()?,
        ft.is_dir(),
        e.metadata().ok().as_ref(),
    )
}

/// FileRecord field assembly: shared by the scanner (`to_record`, a DirEntry)
/// and the watcher (`record_from_parts`, an event-path stat) so the two
/// paths' record fields cannot drift — directories get size=0/ext=None,
/// mtime in seconds, 0 before the epoch. Paths/file names that are not
/// UTF-8 return None (consistent with the store's existing UTF-8-only
/// constraint). With `md` = None, size/mtime are 0.
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

/// Assembles one record from a per-path stat. Reserved single-path entry:
/// the wiring PR's (#869) rescan/refresh goes through here; the watcher's
/// event path does not (`handle_event` needs the stat result to dispatch
/// NotFound/other errors first, which `Option<FileRecord>` cannot express).
/// Uses `symlink_metadata` — no symlink following, consistent with the
/// scanner's follow_links(false); symlink/socket etc. return None and are
/// skipped. **Stat-failure contract**: *any* `symlink_metadata` failure —
/// including transient jitter (EACCES on a tightened parent, ESTALE/EIO) —
/// returns None, where the scanner for the same state stores a placeholder
/// row (size/mtime 0) and keeps the path in its visited set; a refresh
/// caller must therefore treat non-NotFound failures as a no-op (mirroring
/// `handle_event`'s error arm) and never map None to a row delete, or a
/// live file's row is dropped until rescan. **The caller must first apply
/// the scanner's same-source
/// exclusion policy** (component-level names/secrets +
/// [`Excluder::is_skipped`]): this function only assembles fields and does
/// no exclusion — the scanner path is already excluded by filter_entry, the
/// watcher path excludes first in `handle_event`, and refresh code going
/// through here must carry the same judgment, otherwise .ssh/keys enter the
/// store through the single-path entry.
#[allow(dead_code)] // watcher is library-first: only tests reference this before wiring
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

    /// record_from_path's field assembly: file (name/ext/size/mtime),
    /// directory (size=0, ext=None), symlink rejection — the store shape
    /// shared by the watcher's incremental and the scanner's paths.
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
        assert!(
            file.mtime > 0,
            "mtime is assembled from stat, must not degrade to 0"
        );

        let dir = record_from_path(&base).unwrap();
        assert!(dir.is_dir);
        assert_eq!(dir.ext, None, "directories get no extension");
        assert_eq!(dir.size, 0);

        // A dot-suffixed directory name likewise gets no extension (the
        // fixture must be dotted, otherwise ext_of returns None anyway and
        // the is_dir branch bites nothing).
        fs::create_dir_all(base.join("backup.old")).unwrap();
        let dotted_dir = record_from_path(&base.join("backup.old")).unwrap();
        assert!(dotted_dir.is_dir);
        assert_eq!(
            dotted_dir.ext, None,
            "a dot-suffixed directory name's ext must still be None"
        );

        // The symlink body does not enter the store (symlink_metadata does
        // not follow), consistent with the scanner.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(base.join("报告.pdf"), base.join("link.pdf")).unwrap();
            assert!(
                record_from_path(&base.join("link.pdf")).is_none(),
                "the symlink body does not enter the store"
            );
        }

        let _ = fs::remove_dir_all(&base);
    }

    /// Non-UTF-8 paths: the scanner's own semantics (to_str() failure →
    /// skip), must not panic. Note APFS and friends reject non-UTF-8 file
    /// names (EILSEQ), so the fixture cannot be created there — the same
    /// probe as the chmod-000 fixture: skip when it cannot be created, no
    /// false red on hostile filesystems.
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
        if fs::write(&p, b"x").is_err() {
            let _ = fs::remove_dir_all(&base);
            return;
        }

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

    /// The scan leg's non-UTF-8 pruning (the scanner half of the watcher's
    /// `non_utf8_spellings_never_enter_the_index`, pinning the "both sides
    /// store the same outcome" claim on the scan side): a non-UTF-8-named
    /// directory is pruned whole at filter_entry via the empty-name
    /// semantics (clean leaves stay out too), and `to_record`'s `to_str`
    /// gate catches the remaining shapes. APFS cannot create the fixture
    /// (probe-skip); the ext4 CI (ubuntu full leg) really executes it — the
    /// same probe convention as the chmod-000 fixture.
    #[test]
    #[cfg(unix)]
    fn scan_prunes_non_utf8_names_like_the_watcher() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;

        let base =
            std::env::temp_dir().join(format!("pinvou3_kb_scan_nonutf8_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let bad_dir = base.join(OsStr::from_bytes(b"\xff\xfe.dir"));
        if fs::create_dir_all(&bad_dir).is_err() {
            // A hostile filesystem (APFS rejects non-UTF-8 names) cannot
            // create the fixture; skip.
            let _ = fs::remove_dir_all(&base);
            return;
        }
        let bad_leaf = bad_dir.join(OsStr::from_bytes(b"ok.pdf"));
        if fs::write(&bad_leaf, b"p").is_err() {
            let _ = fs::remove_dir_all(&base);
            return;
        }
        fs::write(base.join("报告.pdf"), b"r").unwrap();

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

        assert_eq!(errors, 0, "pruning is a policy decision, not a walk error");
        // walked is exactly 2: walkdir also yields one entry for the root
        // itself (to_record builds a row for the root), plus the clean leaf;
        // the pruned subtree contributes nothing (round-6 CI ext4 measured
        // left=2).
        assert_eq!(
            walked, 2,
            "only the root row and the clean sibling are countable; the pruned \
             subtree yields nothing"
        );
        assert!(
            !visited.iter().any(|p| p.contains('\u{FFFD}')),
            "no lossy-spelled path may enter the visit set"
        );
        let mut indexed: Vec<String> = store.load_index().unwrap().into_keys().collect();
        indexed.sort();
        assert_eq!(
            indexed,
            vec![
                base.to_string_lossy().into_owned(),
                base.join("报告.pdf").to_string_lossy().into_owned(),
            ],
            "the root row + clean sibling are indexed; the non-UTF-8 subtree \
             stays out entirely"
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
