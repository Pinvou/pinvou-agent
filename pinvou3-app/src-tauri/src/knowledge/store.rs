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
/// v3：新增 L1 知识库表（collections/documents/chunks/chunks_fts）。
const SCHEMA_VERSION: i64 = 3;

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

-- 可重建索引的轻量运行元数据。与业务数据分表，后续增加 key 无需 bump schema
-- 并清空整个大索引库。
CREATE TABLE IF NOT EXISTS knowledge_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

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

-- ============ L1 知识库（见 l1.rs）============
-- 知识集：用户圈定的一批文件，内容化后供检索/问答。
CREATE TABLE IF NOT EXISTS collections (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    category    TEXT,
    description TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    embed_model TEXT,                       -- 绑定的 embedding 模型（NULL=仅全文）
    embed_dim   INTEGER NOT NULL DEFAULT 0, -- 向量维度，换模型→重建
    status      TEXT NOT NULL DEFAULT 'ready' -- ready / indexing / pending
);

-- 知识集内文档（来源文件）。collection_id+path 唯一，避免重复加入。
CREATE TABLE IF NOT EXISTS documents (
    id            INTEGER PRIMARY KEY,
    collection_id INTEGER NOT NULL,
    path          TEXT NOT NULL,
    name          TEXT NOT NULL,
    ext           TEXT,
    mtime         INTEGER NOT NULL DEFAULT 0,
    size          INTEGER NOT NULL DEFAULT 0,
    parse_status  TEXT NOT NULL DEFAULT 'pending', -- pending/parsed/skipped/failed
    n_chunks      INTEGER NOT NULL DEFAULT 0,
    parsed_at     INTEGER NOT NULL DEFAULT 0,
    UNIQUE(collection_id, path)
);
CREATE INDEX IF NOT EXISTS idx_docs_coll ON documents(collection_id);

-- 文本块 + 向量（vec 为 NULL 时退回全文检索）。
CREATE TABLE IF NOT EXISTS chunks (
    id            INTEGER PRIMARY KEY,
    document_id   INTEGER NOT NULL,
    collection_id INTEGER NOT NULL,
    ord           INTEGER NOT NULL,
    text          TEXT NOT NULL,
    n_tokens      INTEGER NOT NULL DEFAULT 0,
    vec           BLOB
);
CREATE INDEX IF NOT EXISTS idx_chunks_doc  ON chunks(document_id);
CREATE INDEX IF NOT EXISTS idx_chunks_coll ON chunks(collection_id);

-- chunk 全文索引（trigram 子串，中文友好）。
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
    text, content='chunks', content_rowid='id', tokenize='trigram'
);
CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
    INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
    INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES('delete', old.id, old.text);
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

/// 按扩展名的文件计数（文件管理「按类型浏览」用）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TypeCount {
    pub ext: String,
    pub count: u64,
}

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
    /// 独立只读连接：WAL 下并发读，扫描的写锁不堵查询(治「扫描中切 tab 卡死」)。
    /// 内存库(测试)无并发扫描，read 与 conn 共用同一连接。
    read: Arc<Mutex<Connection>>,
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
        let w = Connection::open(db_path)?;
        w.execute_batch(SCHEMA)?;
        w.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
        // 独立只读连接：WAL 下与写连接并发，扫描写锁不堵前端查询。
        let r = Connection::open(db_path)?;
        r.execute_batch("PRAGMA query_only = ON;")?;
        Ok(Self {
            conn: Arc::new(Mutex::new(w)),
            read: Arc::new(Mutex::new(r)),
        })
    }

    /// 内存库（单测用）。
    #[allow(dead_code)] // 仅 #[cfg(test)] 引用；保留给日后 daemon/集成测试
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn from_conn(conn: Connection) -> rusqlite::Result<Self> {
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
        // 内存库(测试)：无并发扫描，读写共用同一连接。
        let arc = Arc::new(Mutex::new(conn));
        Ok(Self {
            conn: arc.clone(),
            read: arc,
        })
    }

    /// 共享底层连接给 L1（同一个 index.db、同一把锁，避免多连接 WAL 复杂度）。
    pub(super) fn conn_arc(&self) -> Arc<Mutex<Connection>> {
        self.conn.clone()
    }

    /// 最近一次完整扫描完成时间。旧库首次升级还没有 meta 时，回退到文件记录里最新的
    /// indexed_at，避免 app 每次重启后首次进入知识库都立刻重扫整个 HOME。
    pub fn last_scan_finished_at(&self) -> rusqlite::Result<i64> {
        self.read.lock().query_row(
            "SELECT COALESCE(\
                (SELECT CAST(value AS INTEGER) FROM knowledge_meta WHERE key='last_scan_finished_at'),\
                (SELECT MAX(indexed_at) FROM files),\
                0)",
            [],
            |row| row.get(0),
        )
    }

    pub fn set_last_scan_finished_at(&self, timestamp: i64) -> rusqlite::Result<()> {
        self.conn.lock().execute(
            "INSERT INTO knowledge_meta(key, value) VALUES('last_scan_finished_at', ?1) \
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![timestamp.to_string()],
        )?;
        Ok(())
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
        let guard = self.read.lock();
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
                 WHERE f.status='indexed' AND f.is_dir=0 AND files_fts MATCH ?",
            );
            // trigram：双引号包成字符串字面量做子串匹配，内部引号翻倍转义。
            let t = text.unwrap().replace('"', "\"\"");
            vals.push(Value::Text(format!("\"{t}\"")));
        } else {
            sql.push_str("SELECT f.path, f.name, f.ext, f.size, f.mtime, f.is_dir FROM files f WHERE f.status='indexed' AND f.is_dir=0");
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

        let guard = self.read.lock();
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
        let guard = self.read.lock();
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

    /// 按扩展名分组计数（非目录、已索引），降序。
    pub fn type_counts(&self) -> rusqlite::Result<Vec<TypeCount>> {
        let guard = self.read.lock();
        let mut stmt = guard.prepare(
            "SELECT ext, COUNT(*) FROM files \
             WHERE status='indexed' AND is_dir=0 AND ext IS NOT NULL AND ext!='' \
             GROUP BY ext ORDER BY COUNT(*) DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(TypeCount {
                ext: r.get::<_, String>(0)?,
                count: r.get::<_, i64>(1)? as u64,
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

    #[test]
    fn search_excludes_directories() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_many(&[
            FileRecord { path: "/a/项目文档".into(), name: "项目文档".into(), ext: None, size: 0, mtime: 1, is_dir: true },
            FileRecord { path: "/a/项目文档.pdf".into(), name: "项目文档.pdf".into(), ext: Some("pdf".into()), size: 100, mtime: 2, is_dir: false },
        ])
        .unwrap();
        // FTS 路径(≥3 字符)：目录不应出现在结果里
        let hits = s
            .search(&SearchQuery { text: Some("项目文档".into()), limit: 10, ..Default::default() })
            .unwrap();
        assert!(hits.iter().all(|h| !h.is_dir), "search(FTS) 不应返回目录");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "项目文档.pdf");
        // 无 text 全量路径：同样排除目录
        let all = s.search(&SearchQuery { limit: 10, ..Default::default() }).unwrap();
        assert!(all.iter().all(|h| !h.is_dir), "search(全量) 不应返回目录");
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn scan_timestamp_falls_back_to_index_and_then_persists() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.last_scan_finished_at().unwrap(), 0);
        s.upsert_many(&[rec("/a/notes.md", "notes.md", Some("md"), 10, 1)])
            .unwrap();
        assert!(s.last_scan_finished_at().unwrap() > 0);

        s.set_last_scan_finished_at(12345).unwrap();
        assert_eq!(s.last_scan_finished_at().unwrap(), 12345);
    }
}
