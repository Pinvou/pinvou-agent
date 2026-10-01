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

/// 共享的剪枝遍历：walkdir + [`Excluder::is_skipped`] 逐层剪枝，不跟随软链。
/// `scan`（全盘扫描）与知识库导入的 `expand_import_roots`（mod.rs）共用，
/// 保证两条入口的排除语义一致。遍历错误（权限不足等）以 `Err` 透传给调用方
/// 自行处置：全盘扫描把它们计入根的 walk 错误数（陈旧删除授权的否决项），
/// 导入侧自行 flatten 跳过——两条入口对错误的处置不同，跳过语义因此不再
/// 藏在这一层。
pub(super) fn walk_pruned(
    root: &Path,
    ex: &Excluder,
) -> impl Iterator<Item = walkdir::Result<DirEntry>> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !skipped(ex, e))
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
) -> (u64, u64) {
    let mut buf: Vec<FileRecord> = Vec::with_capacity(BATCH);
    let mut walked: u64 = 0;
    let mut walk_errors: u64 = 0;

    for entry in walk_pruned(root, ex) {
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
                if walked.is_multiple_of(5000) {
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
        if walked.is_multiple_of(5000) {
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
    let path = e.path().to_str()?.to_string();
    let name = e.file_name().to_str()?.to_string();
    let is_dir = ft.is_dir();
    let md = e.metadata().ok();
    let size = if is_dir {
        0
    } else {
        md.as_ref().map(|m| m.len()).unwrap_or(0)
    };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::knowledge::store::SearchQuery;
    use std::fs;

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
        let (walked, errors) = scan(&base, &store, &ex, &HashMap::new(), &mut visited, |_| {});
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
        scan(&base, &store, &ex, &HashMap::new(), &mut visited, |_| {});

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
