//! 按 (mtime, len) 签名失效的单条目淘汰缓存。
//!
//! deliverables（会话 JSON 的 index view 解析缓存）与 multiagent/transcripts
//! （transcript 受阻判定缓存）此前各维护一份同构实现：文件被整体重写时 mtime
//! 必然前移，签名不匹配即自然失效，无需显式失效；缓存满时只按 HashMap 迭代序
//! 淘汰单条——清空整表会让下一次轮询重读所有文件（雪崩式回退），哈希表无序，
//! 任淘汰一条即收敛。本模块是该模式的唯一实现（`pub(crate)`，仅 crate 内复用）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// 文件内容签名：len + mtime。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl FileStamp {
    pub(crate) fn of(metadata: &std::fs::Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }
}

/// 满时单条目淘汰的 (FileStamp, V) 缓存。查询与写入是两段独立加锁：调用方在
/// `get` 未命中后重算（含文件 IO），再经 `insert` 回填——锁不跨文件 IO 持有。
pub(crate) struct StampCache<V> {
    limit: usize,
    entries: OnceLock<Mutex<HashMap<PathBuf, (FileStamp, V)>>>,
}

impl<V: Clone> StampCache<V> {
    pub(crate) const fn new(limit: usize) -> Self {
        Self {
            limit,
            entries: OnceLock::new(),
        }
    }

    /// 签名一致时返回缓存值；未命中或已过期返回 None。
    pub(crate) fn get(&self, path: &Path, stamp: FileStamp) -> Option<V> {
        self.lock()
            .get(path)
            .filter(|(cached, _)| *cached == stamp)
            .map(|(_, value)| value.clone())
    }

    /// 回填缓存；满且为新键时淘汰单条既有条目。
    pub(crate) fn insert(&self, path: &Path, stamp: FileStamp, value: V) {
        let mut guard = self.lock();
        if guard.len() >= self.limit && !guard.contains_key(path) {
            if let Some(evicted) = guard.keys().next().cloned() {
                guard.remove(&evicted);
            }
        }
        guard.insert(path.to_path_buf(), (stamp, value));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, (FileStamp, V)>> {
        self.entries
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
