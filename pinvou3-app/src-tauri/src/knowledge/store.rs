//! L0 元数据存储：SQLite + FTS5(trigram) 做全系统秒搜 + 去重候选查询。
//!
//! 设计（见 docs/本地知识底座-产品形态与架构.md §4.0/§5）：
//! - `files` 表只存元数据（名/路径/大小/时间/类型/hash），**不存内容**。L1 内容/向量是后续层。
//! - `files_fts` 是 external-content FTS5 虚表，trigram 分词 —— 对中文文件名和子串搜索都友好
//!   （unicode61 会把一串中文当成单 token，搜不到子串；trigram 按 3-字符窗口可子串命中）。
//! - 去重省钱：内容相同 → 大小必相同。只对「同 size 冲突组」补算 hash（[`Store::dup_hash_candidates`]）。
//!
//! 并发：`Connection` Send 不 Sync，整库放 `Arc<Mutex<Connection>>`。扫描线程批量写、
//! 前端查询读，都短暂持锁；v0 单连接足够，日后抽 daemon 再上 WAL/多连接。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{params, params_from_iter, types::Value, Connection};
use serde::Serialize;

/// schema 版本。bump 后旧库会被删除重建（L0 是可重建缓存，无数据损失）。
/// v2：FTS 砍掉 path 列（path-trigram 实测占 2/3 库体积、是写入 CPU 大头）。
const SCHEMA_VERSION: i64 = 2;

/// 建表 + FTS5 虚表 + 同步触发器。幂等（`IF NOT EXISTS`）。
const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;

CREATE TABLE IF NOT EXISTS files (
    id         INTEGER PRIMARY KEY,
    path       TEXT NOT NULL UNIQUE,
    name       TEXT NOT NULL,
    ext        TEXT,
    size       INTEGER NOT NULL,
    mtime      INTEGER NOT NULL,
    is_dir     INTEGER NOT NULL DEFAULT 0,
    hash       TEXT,
    status     TEXT NOT NULL DEFAULT 'indexed',
    indexed_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_files_size  ON files(size);
CREATE INDEX IF NOT EXISTS idx_files_ext   ON files(ext);
CREATE INDEX IF NOT EXISTS idx_files_mtime ON files(mtime);
CREATE INDEX IF NOT EXISTS idx_files_hash  ON files(hash);

-- FTS5 只索引文件名（trigram 子串搜索）。**不索引 path 全路径**：
-- path-trigram 实测占 2/3 库体积、是写入 CPU 大头，而 path 精确匹配已有 UNIQUE 索引、
-- 路径子串是低频需求（退回 search() 里 1-2 字符那条 LIKE 兜底）。
CREATE VIRTUAL TABLE IF NOT EXISTS files_fts USING fts5(
    name,
    content='files', content_rowid='id',
    tokenize='trigram'
);

-- external-content FTS5 同步：只在 name 变化时重建索引行（改内容=mtime/size 变但 name 不变 → 不触发）。
CREATE TRIGGER IF NOT EXISTS files_ai AFTER INSERT ON files BEGIN
    INSERT INTO files_fts(rowid, name) VALUES (new.id, new.name);
END;
CREATE TRIGGER IF NOT EXISTS files_ad AFTER DELETE ON files BEGIN
    INSERT INTO files_fts(files_fts, rowid, name) VALUES('delete', old.id, old.name);
END;
CREATE TRIGGER IF NOT EXISTS files_au AFTER UPDATE OF name ON files BEGIN
    INSERT INTO files_fts(files_fts, rowid, name) VALUES('delete', old.id, old.name);
    INSERT INTO files_fts(rowid, name) VALUES (new.id, new.name);
END;
"#;

const UPSERT_SQL: &str = r#"
INSERT INTO files(path, name, ext, size, mtime, is_dir, status, indexed_at, hash)
VALUES(?1, ?2, ?3, ?4, ?5, ?6, 'indexed', strftime('%s','now'), NULL)
ON CONFLICT(path) DO UPDATE SET
    name=excluded.name, ext=excluded.ext, size=excluded.size,
    mtime=excluded.mtime, is_dir=excluded.is_dir, status='indexed',
    indexed_at=excluded.indexed_at,
    hash=CASE WHEN files.size != excluded.size THEN NULL ELSE files.hash END
"#;

/// 一条待写入的文件元数据。
#[derive(Debug, Clone)]
pub struct FileRecord {
    pub path: String,
    pub name: String,
    pub ext: Option<String>,
    pub size: u64,
    pub mtime: i64,
    pub is_dir: bool,
}

/// 搜索命中项（回前端）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileHit {
    pub path: String,
    pub name: String,
    pub ext: Option<String>,
    pub size: u64,
    pub mtime: i64,
    pub is_dir: bool,
}

/// 秒搜查询条件。`text` 为名/路径子串；其余为结构化过滤。
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    pub text: Option<String>,
    pub exts: Vec<String>,
    pub mtime_after: Option<i64>,
    pub mtime_before: Option<i64>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub limit: usize,
}

/// 索引概况。
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub total_files: u64,
    pub total_bytes: u64,
    pub hashed: u64,
    pub duplicate_groups: u64,
    pub duplicate_files: u64,
    /// 去重可回收字节 = Σ(组内冗余份数 × 单份大小)。
    pub duplicate_wasted_bytes: u64,
}

/// 一组内容相同的重复文件。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DupGroup {
    pub hash: String,
    pub size: u64,
    pub paths: Vec<String>,
}

/// 同 size 冲突、尚未算 hash 的待办项（去重 pass 的输入）。
#[derive(Debug, Clone)]
pub struct HashCandidate {
    pub id: i64,
    pub path: String,
    pub size: u64,
}

/// 路径里不可能出现的分隔符，用于 GROUP_CONCAT 拆分组内路径。
const SEP: char = '\u{1f}'; // ASCII Unit Separator

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    /// 打开（或新建）磁盘库，建表。父目录会自动创建。
    /// schema 版本不符 → 删库重建（L0 是可重建缓存，重扫即恢复；顺带回收旧版撑大的体积）。
    pub fn open(db_path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let stale = {
            match Connection::open(db_path) {
                Ok(c) => c
                    .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                    .unwrap_or(0)
                    != SCHEMA_VERSION,
                Err(_) => false,
            }
        }; // 连接在此 drop，才能删文件
        if stale {
            let p = db_path.display().to_string();
            let _ = std::fs::remove_file(db_path);
            let _ = std::fs::remove_file(format!("{p}-wal"));
            let _ = std::fs::remove_file(format!("{p}-shm"));
            eprintln!("[knowledge] schema 升级到 v{SCHEMA_VERSION}，旧索引库已清空，需重新扫描");
        }
        Self::from_conn(Connection::open(db_path)?)
    }

    /// 内存库（单测用）。
    #[allow(dead_code)] // 仅 #[cfg(test)] 引用；保留给日后 daemon/集成测试
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn from_conn(conn: Connection) -> rusqlite::Result<Self> {
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// 批量 upsert（扫描器用，单事务）。size 变化会让旧 hash 失效。
    pub fn upsert_many(&self, recs: &[FileRecord]) -> rusqlite::Result<()> {
        let mut guard = self.conn.lock();
        let tx = guard.transaction()?;
        {
            let mut stmt = tx.prepare_cached(UPSERT_SQL)?;
            for r in recs {
                stmt.execute(params![
                    r.path,
                    r.name,
                    r.ext,
                    r.size as i64,
                    r.mtime,
                    r.is_dir as i64,
                ])?;
            }
        }
        tx.commit()
    }

    /// 物理删除一条（watcher 的 remove 用）。FTS 由触发器同步。
    #[allow(dead_code)] // 下一增量(实时 watcher)接线;先建好 API
    pub fn delete_by_path(&self, path: &str) -> rusqlite::Result<()> {
        self.conn
            .lock()
            .execute("DELETE FROM files WHERE path = ?1", params![path])?;
        Ok(())
    }

    /// 现有索引快照 `path → (mtime, size)`，给增量扫描比对用（只取未变文件可跳过 upsert）。
    pub fn load_index(&self) -> rusqlite::Result<HashMap<String, (i64, u64)>> {
        let guard = self.conn.lock();
        let mut stmt = guard.prepare("SELECT path, mtime, size FROM files")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get::<_, i64>(1)?, r.get::<_, i64>(2)? as u64),
            ))
        })?;
        rows.collect()
    }

    /// 批量删除（增量扫描清理本次未再见到的「已消失」文件）。单事务，FTS 由触发器同步。
    pub fn delete_many(&self, paths: &[String]) -> rusqlite::Result<()> {
        let mut guard = self.conn.lock();
        let tx = guard.transaction()?;
        {
            let mut stmt = tx.prepare_cached("DELETE FROM files WHERE path = ?1")?;
            for p in paths {
                stmt.execute(params![p])?;
            }
        }
        tx.commit()
    }

    /// 回填 hash（去重 pass 用）。只动 hash 列，不触发 FTS 重建。
    pub fn set_hash(&self, id: i64, hash: &str) -> rusqlite::Result<()> {
        self.conn
            .lock()
            .execute("UPDATE files SET hash = ?1 WHERE id = ?2", params![hash, id])?;
        Ok(())
    }

    /// 秒搜：text 走 FTS5(≥3 字符) 或 LIKE 兜底(1-2 字符)，叠加结构化过滤。
    pub fn search(&self, q: &SearchQuery) -> rusqlite::Result<Vec<FileHit>> {
        let limit = if q.limit == 0 { 200 } else { q.limit } as i64;
        let mut sql = String::new();
        let mut vals: Vec<Value> = Vec::new();

        let text = q.text.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let use_fts = text.map(|t| t.chars().count() >= 3).unwrap_or(false);

        if use_fts {
            sql.push_str(
                "SELECT f.path, f.name, f.ext, f.size, f.mtime, f.is_dir \
                 FROM files_fts JOIN files f ON f.id = files_fts.rowid \
                 WHERE f.status='indexed' AND files_fts MATCH ?",
            );
            // trigram：双引号包成字符串字面量做子串匹配，内部引号翻倍转义。
            let t = text.unwrap().replace('"', "\"\"");
            vals.push(Value::Text(format!("\"{t}\"")));
        } else {
            sql.push_str("SELECT f.path, f.name, f.ext, f.size, f.mtime, f.is_dir FROM files f WHERE f.status='indexed'");
            if let Some(t) = text {
                sql.push_str(" AND (f.name LIKE ? OR f.path LIKE ?)");
                let like = format!("%{}%", escape_like(t));
                vals.push(Value::Text(like.clone()));
                vals.push(Value::Text(like));
            }
        }

        if !q.exts.is_empty() {
            let ph = vec!["?"; q.exts.len()].join(",");
            sql.push_str(&format!(" AND f.ext IN ({ph})"));
            for e in &q.exts {
                vals.push(Value::Text(e.to_lowercase()));
            }
        }
        if let Some(v) = q.mtime_after {
            sql.push_str(" AND f.mtime >= ?");
            vals.push(Value::Integer(v));
        }
        if let Some(v) = q.mtime_before {
            sql.push_str(" AND f.mtime <= ?");
            vals.push(Value::Integer(v));
        }
        if let Some(v) = q.min_size {
            sql.push_str(" AND f.size >= ?");
            vals.push(Value::Integer(v as i64));
        }
        if let Some(v) = q.max_size {
            sql.push_str(" AND f.size <= ?");
            vals.push(Value::Integer(v as i64));
        }
        sql.push_str(" ORDER BY f.mtime DESC LIMIT ?");
        vals.push(Value::Integer(limit));

        let guard = self.conn.lock();
        let mut stmt = guard.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(vals.iter()), |row| {
            Ok(FileHit {
                path: row.get(0)?,
                name: row.get(1)?,
                ext: row.get(2)?,
                size: row.get::<_, i64>(3)? as u64,
                mtime: row.get(4)?,
                is_dir: row.get::<_, i64>(5)? != 0,
            })
        })?;
        rows.collect()
    }

    /// 索引概况 + 去重统计。
    pub fn stats(&self) -> rusqlite::Result<Stats> {
        let guard = self.conn.lock();
        let (total_files, total_bytes, hashed) = guard.query_row(
            "SELECT COUNT(*), COALESCE(SUM(size),0), \
             COALESCE(SUM(CASE WHEN hash IS NOT NULL THEN 1 ELSE 0 END),0) \
             FROM files WHERE status='indexed' AND is_dir=0",
            [],
            |r| {
                Ok((
                    r.get::<_, i64>(0)? as u64,
                    r.get::<_, i64>(1)? as u64,
                    r.get::<_, i64>(2)? as u64,
                ))
            },
        )?;
        let (groups, dup_files, wasted) = guard.query_row(
            "SELECT COUNT(*), COALESCE(SUM(cnt),0), COALESCE(SUM((cnt-1)*size),0) FROM (\
               SELECT size, COUNT(*) cnt FROM files \
               WHERE status='indexed' AND is_dir=0 AND hash IS NOT NULL \
               GROUP BY hash HAVING cnt>1)",
            [],
            |r| {
                Ok((
                    r.get::<_, i64>(0)? as u64,
                    r.get::<_, i64>(1)? as u64,
                    r.get::<_, i64>(2)? as u64,
                ))
            },
        )?;
        Ok(Stats {
            total_files,
            total_bytes,
            hashed,
            duplicate_groups: groups,
            duplicate_files: dup_files,
            duplicate_wasted_bytes: wasted,
        })
    }

    /// 待算 hash 的同 size 冲突项。唯一大小的文件不返回（永远不会重复）。
    pub fn dup_hash_candidates(&self) -> rusqlite::Result<Vec<HashCandidate>> {
        let guard = self.conn.lock();
        let mut stmt = guard.prepare(
            "SELECT id, path, size FROM files \
             WHERE status='indexed' AND is_dir=0 AND hash IS NULL AND size>0 \
               AND size IN (SELECT size FROM files \
                            WHERE status='indexed' AND is_dir=0 AND size>0 \
                            GROUP BY size HAVING COUNT(*)>1)",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(HashCandidate {
                id: row.get(0)?,
                path: row.get(1)?,
                size: row.get::<_, i64>(2)? as u64,
            })
        })?;
        rows.collect()
    }

    /// 已确认的重复组（hash 相同、份数>1），按可回收空间降序。
    pub fn duplicate_groups(&self, limit: usize) -> rusqlite::Result<Vec<DupGroup>> {
        let guard = self.conn.lock();
        let mut stmt = guard.prepare(
            "SELECT hash, size, GROUP_CONCAT(path, char(31)) \
             FROM files \
             WHERE status='indexed' AND is_dir=0 AND hash IS NOT NULL \
             GROUP BY hash HAVING COUNT(*)>1 \
             ORDER BY size*(COUNT(*)-1) DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let hash: String = row.get(0)?;
            let size = row.get::<_, i64>(1)? as u64;
            let joined: String = row.get(2)?;
            Ok(DupGroup {
                hash,
                size,
                paths: joined.split(SEP).map(|s| s.to_string()).collect(),
            })
        })?;
        rows.collect()
    }
}

/// 转义 LIKE 的通配符（默认无 ESCAPE 子句时 `%`/`_` 会被当通配）。这里用 `\` 转义，
/// 但调用处 SQL 未加 `ESCAPE '\'`，所以仅做最朴素处理——把已有反斜杠也保留。
/// v0 文件名含 `%`/`_` 的子串搜索可能略宽，可接受；FTS5 路径(≥3 字符)才是主路径。
fn escape_like(s: &str) -> String {
    s.replace('%', "").replace('_', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(path: &str, name: &str, ext: Option<&str>, size: u64, mtime: i64) -> FileRecord {
        FileRecord {
            path: path.into(),
            name: name.into(),
            ext: ext.map(|s| s.into()),
            size,
            mtime,
            is_dir: false,
        }
    }

    fn seed() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_many(&[
            rec("/home/u/Documents/保险报价单.pdf", "保险报价单.pdf", Some("pdf"), 2048, 1000),
            rec("/home/u/Downloads/平安交强险保单.pdf", "平安交强险保单.pdf", Some("pdf"), 1024, 2000),
            rec("/home/u/Desktop/notes.md", "notes.md", Some("md"), 64, 3000),
            rec("/home/u/Downloads/report.docx", "report.docx", Some("docx"), 4096, 500),
        ])
        .unwrap();
        s
    }

    #[test]
    fn fts_substring_cjk() {
        let s = seed();
        let q = SearchQuery {
            text: Some("保险".into()), // 2 字符 → LIKE 兜底
            limit: 10,
            ..Default::default()
        };
        let hits = s.search(&q).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].name.contains("保险报价单"));

        let q3 = SearchQuery {
            text: Some("交强险".into()), // 3 字符 → FTS5 trigram
            limit: 10,
            ..Default::default()
        };
        let hits3 = s.search(&q3).unwrap();
        assert_eq!(hits3.len(), 1);
        assert!(hits3[0].name.contains("平安交强险"));
    }

    #[test]
    fn filter_by_ext_and_size_and_time() {
        let s = seed();
        let pdfs = s
            .search(&SearchQuery {
                exts: vec!["pdf".into()],
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(pdfs.len(), 2);
        // mtime DESC：最新的保单(2000)在前
        assert!(pdfs[0].name.contains("平安"));

        let big = s
            .search(&SearchQuery {
                min_size: Some(2000),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(big.len(), 2); // 2048 + 4096

        let recent = s
            .search(&SearchQuery {
                mtime_after: Some(2500),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].name, "notes.md");
    }

    #[test]
    fn upsert_updates_and_invalidates_hash_on_size_change() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_many(&[rec("/a/x.bin", "x.bin", Some("bin"), 100, 1)])
            .unwrap();
        // 同 size 再来一个，构成冲突组
        s.upsert_many(&[rec("/a/y.bin", "y.bin", Some("bin"), 100, 1)])
            .unwrap();
        let cands = s.dup_hash_candidates().unwrap();
        assert_eq!(cands.len(), 2);
        for c in &cands {
            s.set_hash(c.id, "deadbeef").unwrap();
        }
        // hash 一致 → 一组重复
        let groups = s.duplicate_groups(10).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].paths.len(), 2);

        // 改 x.bin 的 size → hash 失效，且不再与 y 同 size
        s.upsert_many(&[rec("/a/x.bin", "x.bin", Some("bin"), 999, 2)])
            .unwrap();
        let groups2 = s.duplicate_groups(10).unwrap();
        assert!(groups2.is_empty(), "size 变了不该还算重复");
    }

    #[test]
    fn dedup_stats_and_wasted_bytes() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_many(&[
            rec("/a/1.pdf", "1.pdf", Some("pdf"), 500, 1),
            rec("/a/2.pdf", "2.pdf", Some("pdf"), 500, 1),
            rec("/a/3.pdf", "3.pdf", Some("pdf"), 500, 1),
            rec("/a/u.pdf", "u.pdf", Some("pdf"), 777, 1), // 唯一大小，不参与
        ])
        .unwrap();
        for c in s.dup_hash_candidates().unwrap() {
            s.set_hash(c.id, "samehash").unwrap();
        }
        let st = s.stats().unwrap();
        assert_eq!(st.total_files, 4);
        assert_eq!(st.duplicate_groups, 1);
        assert_eq!(st.duplicate_files, 3);
        assert_eq!(st.duplicate_wasted_bytes, 2 * 500); // 3 份留 1，省 2 份
    }

    #[test]
    fn delete_removes_from_fts() {
        let s = seed();
        s.delete_by_path("/home/u/Desktop/notes.md").unwrap();
        let hits = s
            .search(&SearchQuery {
                text: Some("notes".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert!(hits.is_empty());
    }
}
