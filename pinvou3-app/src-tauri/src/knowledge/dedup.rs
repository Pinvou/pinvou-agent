//! 去重 pass：对「同 size 冲突组」补算 sha256，回填 store。唯一大小的文件永不读取。

use std::fs::File;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

use super::store::Store;

/// 超过此大小的文件跳过 hash（大视频/镜像，去重收益低、读取昂贵）。留 hash=NULL，不参与去重。
const MAX_HASH_BYTES: u64 = 1024 * 1024 * 1024; // 1 GiB

/// 跑一轮去重。返回实际算了 hash 的文件数。`on_progress(done, total)` 周期回调。
pub fn run(
    store: &Store,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, u64),
) -> rusqlite::Result<u64> {
    let candidates = store.dup_hash_candidates()?;
    let total = candidates.len() as u64;
    let mut done = 0u64;
    for c in candidates {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        done += 1;
        if c.size > MAX_HASH_BYTES {
            on_progress(done, total);
            continue;
        }
        if let Ok(h) = hash_file(&c.path) {
            let _ = store.set_hash(c.id, &h);
        }
        on_progress(done, total);
    }
    Ok(done)
}

/// 分块读取算 sha256，避免一次性载入大文件。返回小写 hex。
fn hash_file(path: &str) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::scanner;
    use crate::knowledge::store::Store;
    use crate::knowledge::Excluder;
    use std::fs;

    #[test]
    fn dedup_finds_identical_content() {
        let base = std::env::temp_dir().join(format!("pinvou3_kb_dedup_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        // 两个内容相同（同 size 同 hash）+ 一个同 size 不同内容 + 一个唯一
        fs::write(base.join("a.pdf"), b"DUPLICATE-CONTENT").unwrap();
        fs::write(base.join("b.pdf"), b"DUPLICATE-CONTENT").unwrap();
        fs::write(base.join("c.pdf"), b"DIFFERENT-CONTENT").unwrap(); // 同 size 不同内容
        fs::write(base.join("u.pdf"), b"unique").unwrap();

        let store = Store::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        scanner::scan(&base, &store, &Excluder::default(), &cancel, |_| {});
        run(&store, &cancel, |_, _| {}).unwrap();

        let groups = store.duplicate_groups(10).unwrap();
        assert_eq!(groups.len(), 1, "只有 a/b 内容相同算一组");
        assert_eq!(groups[0].paths.len(), 2);

        let _ = fs::remove_dir_all(&base);
    }
}
