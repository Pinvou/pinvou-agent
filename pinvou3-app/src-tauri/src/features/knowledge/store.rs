// architecture-guard: allow-target-cfg -- the probe regression test constructs an "open-read failure" with POSIX permission bits to verify the failure is no longer folded into version 0, which would trigger the store deletion (the OS-metadata check stays inside the most cohesive test per precedent)
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
use rusqlite::{Connection, params, params_from_iter, types::Value};
use serde::Serialize;

/// schema 版本。v3 起包含用户创建的 L1 数据，后续版本必须提供原地迁移。
/// v2：FTS 砍掉 path 列（path-trigram 实测占 2/3 库体积、是写入 CPU 大头）。
/// v3：新增 L1 知识库表（collections/documents/chunks/chunks_fts）。
/// v4：新增可恢复的批量导入任务、文件状态与分块暂存表。
const SCHEMA_VERSION: i64 = 4;

/// Whether a store at `version` may be deleted and rebuilt on open.
///
/// Only pre-v3 schemas qualify: v3 is the first that holds knowledge-set
/// business data, which a rescan cannot reconstruct. Deliberately NOT
/// "anything that is neither 3 nor `SCHEMA_VERSION`" — whitelisting the
/// current version is a landmine for the next schema bump, because the day
/// `SCHEMA_VERSION` becomes 5 every existing v4 store stops matching the
/// whitelist and is deleted on first launch, taking exactly the
/// non-rebuildable data this rule exists to protect. Anything from v3 up
/// migrates in place (the idempotent `IF NOT EXISTS` batch) or is refused
/// (the newer-than-us check); it is never deleted.
fn schema_is_disposable(version: i64) -> bool {
    version < 3
}

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

-- ============ 可恢复的知识集批量导入任务 ============
CREATE TABLE IF NOT EXISTS knowledge_import_jobs (
    id              TEXT PRIMARY KEY,
    collection_id   INTEGER NOT NULL,
    roots_json      TEXT NOT NULL,
    state           TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    finished_at     INTEGER
);
CREATE INDEX IF NOT EXISTS idx_import_jobs_collection
    ON knowledge_import_jobs(collection_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS knowledge_import_items (
    id               INTEGER PRIMARY KEY,
    job_id           TEXT NOT NULL,
    path             TEXT NOT NULL,
    name             TEXT NOT NULL,
    state            TEXT NOT NULL DEFAULT 'pending',
    attempts         INTEGER NOT NULL DEFAULT 0,
    total_chunks     INTEGER NOT NULL DEFAULT 0,
    completed_chunks INTEGER NOT NULL DEFAULT 0,
    content_hash     TEXT,
    error            TEXT,
    updated_at       INTEGER NOT NULL,
    UNIQUE(job_id, path)
);
CREATE INDEX IF NOT EXISTS idx_import_items_job_state
    ON knowledge_import_items(job_id, state, id);

-- 未完成文件的分块暂存区。全部分块就绪后再原子替换正式 chunks，避免半成品被检索。
CREATE TABLE IF NOT EXISTS knowledge_import_staged_chunks (
    job_id    TEXT NOT NULL,
    item_id   INTEGER NOT NULL,
    ord       INTEGER NOT NULL,
    text      TEXT NOT NULL,
    n_tokens  INTEGER NOT NULL DEFAULT 0,
    vec       BLOB,
    PRIMARY KEY(job_id, item_id, ord)
);
"#;

// hash 只剩历史库里的遗留值（去重功能已下线，不再写入/读取）；列保留 nullable 以兼容
// 用户的既有 index.db，UPSERT 不再触碰它。
const UPSERT_SQL: &str = r#"
INSERT INTO files(path, name, ext, size, mtime, is_dir, status, indexed_at)
VALUES(?1, ?2, ?3, ?4, ?5, ?6, 'indexed', strftime('%s','now'))
ON CONFLICT(path) DO UPDATE SET
    name=excluded.name, ext=excluded.ext, size=excluded.size,
    mtime=excluded.mtime, is_dir=excluded.is_dir, status='indexed',
    indexed_at=excluded.indexed_at
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
}

/// Upper bound on a single search's row LIMIT. The GUI pages at a few dozen
/// hits; the headless callers now share this entry point, and an unclamped
/// `usize` limit would both invite a full-table materialization and — at
/// `usize::MAX` — wrap to `-1` in the `i64` conversion, which SQLite reads
/// as "no limit at all".
pub(crate) const SEARCH_LIMIT_CAP: usize = 1000;

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
    ///
    /// All three connections (probe / write / read-only) carry busy_timeout:
    /// the desktop app and the headless CLI are a supported two-process pair,
    /// and any connection hitting the other process's write transaction must
    /// wait instead of failing with "database is locked". A failed probe read
    /// must NOT fold into version 0 — the stale branch below deletes the
    /// database, and since v3 it holds non-rebuildable data. When no trusted
    /// version can be read, the error must surface as-is: failing the open
    /// outright always beats a wrong deletion. (The GUI boot path surfaces
    /// the error and continues without the knowledge service until restart;
    /// headless callers can simply retry the open.)
    ///
    /// A store written by a newer binary is never deleted by a downgrade:
    /// the open refuses with a clear error and leaves every file intact.

    pub fn open(db_path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let existed = db_path.exists();
        let current_version = if existed {
            let probe = Connection::open(db_path)?;
            probe.busy_timeout(std::time::Duration::from_millis(5_000))?;
            let version = probe.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?;
            Some(version)
        } else {
            None
        }; // the probe connection must drop here so the file can be deleted below
        // A store written by a NEWER binary must never be deleted by a
        // downgrade: refuse with a clear error and leave every file intact.
        if let Some(version) = current_version {
            if version > SCHEMA_VERSION {
                return Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_NOTADB),
                    Some(format!(
                        "knowledge store was written by a newer version (schema v{version} \
                         > v{SCHEMA_VERSION}); upgrade pinvou"
                    )),
                ));
            }
        }
        let stale = matches!(current_version, Some(version) if schema_is_disposable(version));
        if stale {
            let p = db_path.display().to_string();
            let _ = std::fs::remove_file(db_path);
            let _ = std::fs::remove_file(format!("{p}-wal"));
            let _ = std::fs::remove_file(format!("{p}-shm"));
            eprintln!(
                "[knowledge] 不兼容 schema 升级到 v{SCHEMA_VERSION}，旧索引库已清空，需重新扫描"
            );
        }
        let w = Connection::open(db_path)?;
        w.busy_timeout(std::time::Duration::from_millis(5_000))?;
        if current_version == Some(SCHEMA_VERSION) {
            // Steady state: the schema is already current. Skip the DDL batch
            // and the user_version write so opening the store takes no write
            // lock at all in the two-process contention case (exactly the window
            // busy_timeout could only shorten). The cost is losing the
            // "self-heal externally dropped tables on every open" behavior:
            // statements fail explicitly once tables were externally destroyed,
            // which beats silent rebuilds masking the damage.
            //
            // Connection-level PRAGMAs do not persist (only journal_mode is
            // written into the database file header), so the DDL batch's
            // synchronous=NORMAL must be re-applied here for the steady-state
            // and create/migrate paths to give write connections the same
            // durability. It is a pure connection setting and takes no
            // database write lock.
            w.execute_batch("PRAGMA synchronous = NORMAL;")?;
        } else {
            // Fresh create / in-place v3 migration: the DDL batch and the
            // version write each take the write lock once; busy_timeout makes
            // them wait out the other process's short transaction instead of
            // failing immediately.
            w.execute_batch(SCHEMA)?;
            w.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
        }
        // 独立只读连接：WAL 下与写连接并发，扫描写锁不堵前端查询。
        let r = Connection::open(db_path)?;
        r.busy_timeout(std::time::Duration::from_millis(5_000))?;
        r.execute_batch("PRAGMA query_only = ON;")?;
        Ok(Self {
            conn: Arc::new(Mutex::new(w)),
            read: Arc::new(Mutex::new(r)),
        })
    }

    /// In-memory store (unit tests only). Test fixtures build stores solely
    /// through this constructor; production always goes through [`Store::open`].
    #[cfg(test)]
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    #[cfg(test)]
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
        let limit = if q.limit == 0 {
            200
        } else {
            q.limit.min(SEARCH_LIMIT_CAP)
        } as i64;
        let mut sql = String::new();
        let mut vals: Vec<Value> = Vec::new();

        let text = q.text.as_deref().map(str::trim).filter(|s| !s.is_empty());

        // Text present with >=3 chars goes to FTS; the branch holds the Some
        // value directly, avoiding a repeated unwrap.
        if let Some(t) = text.filter(|t| t.chars().count() >= 3) {
            sql.push_str(
                "SELECT f.path, f.name, f.ext, f.size, f.mtime \
                 FROM files_fts JOIN files f ON f.id = files_fts.rowid \
                 WHERE f.status='indexed' AND f.is_dir=0 AND files_fts MATCH ?",
            );
            // trigram：双引号包成字符串字面量做子串匹配，内部引号翻倍转义。
            let t = t.replace('"', "\"\"");
            vals.push(Value::Text(format!("\"{t}\"")));
        } else {
            sql.push_str("SELECT f.path, f.name, f.ext, f.size, f.mtime FROM files f WHERE f.status='indexed' AND f.is_dir=0");
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
            })
        })?;
        rows.collect()
    }

    /// 索引概况。去重统计已随去重功能一起下线（hash 不再写入，恒为 NULL）；
    /// 前端只消费 totalFiles。
    pub fn stats(&self) -> rusqlite::Result<Stats> {
        let guard = self.read.lock();
        let total_files = guard.query_row(
            "SELECT COUNT(*) FROM files WHERE status='indexed' AND is_dir=0",
            [],
            |r| Ok(r.get::<_, i64>(0)? as u64),
        )?;
        Ok(Stats { total_files })
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
    s.replace(['%', '_'], "")
}

#[cfg(test)]
mod tests {

    /// No schema at or above v3 may ever be deleted on open — v3 is where
    /// non-rebuildable knowledge-set data starts. The loop is the point: a
    /// predicate written as "neither 3 nor SCHEMA_VERSION" passes for today's
    /// 3 and 4 and then deletes every user's store the day SCHEMA_VERSION is
    /// bumped, which is precisely the regression this pins.
    #[test]
    fn only_pre_v3_schemas_are_disposable() {
        assert!(
            super::schema_is_disposable(0),
            "v0 is a rebuildable L0 index"
        );
        assert!(
            super::schema_is_disposable(2),
            "v2 is a rebuildable L0 index"
        );
        for version in 3..64 {
            assert!(
                !super::schema_is_disposable(version),
                "schema v{version} holds non-rebuildable data and must migrate or refuse, \
                 never be deleted"
            );
        }
    }
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
            rec(
                "/home/u/Documents/保险报价单.pdf",
                "保险报价单.pdf",
                Some("pdf"),
                2048,
                1000,
            ),
            rec(
                "/home/u/Downloads/平安交强险保单.pdf",
                "平安交强险保单.pdf",
                Some("pdf"),
                1024,
                2000,
            ),
            rec("/home/u/Desktop/notes.md", "notes.md", Some("md"), 64, 3000),
            rec(
                "/home/u/Downloads/report.docx",
                "report.docx",
                Some("docx"),
                4096,
                500,
            ),
        ])
        .unwrap();
        s
    }

    /// When the probe connection cannot read the file it must error instead
    /// of folding into version 0: the old code treated any probe failure as
    /// v0, exactly what drove the stale branch to delete the database (v3+
    /// holds non-rebuildable data). A permission fault is a deterministically
    /// constructible probe failure; environments running as root bypass file
    /// permissions, so skip there.
    #[cfg(unix)]
    #[test]
    fn failed_probe_never_deletes_an_existing_store() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-knowledge-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let db = tmp.join("index.db");
        {
            let store = Store::open(&db).expect("create store");
            assert_eq!(store.stats().unwrap().total_files, 0);
            // Seed one document so the survival assert below can tell
            // "original store survived" apart from "deleted and a fresh
            // empty store re-initialized over it" — the delete-and-recreate
            // regression this test exists to catch.
            store
                .upsert_many(&[rec(
                    "/tmp/docs/annual.pdf",
                    "annual.pdf",
                    Some("pdf"),
                    2048,
                    0,
                )])
                .expect("seed one file record");
        }
        let restore = |mode: u32| {
            let mut perm = std::fs::metadata(&db).unwrap().permissions();
            perm.set_mode(mode);
            std::fs::set_permissions(&db, perm).unwrap();
        };
        restore(0o000);
        if std::fs::File::open(&db).is_ok() {
            restore(0o644);
            let _ = std::fs::remove_dir_all(&tmp);
            eprintln!("skipping: privileged environment bypasses file permissions");
            return;
        }
        assert!(
            Store::open(&db).is_err(),
            "an unreadable store must fail loud instead of probing version 0"
        );
        restore(0o644);
        let reopened = Store::open(&db).expect("the store must survive a failed probe");
        assert_eq!(
            reopened
                .stats()
                .expect("stats after the failed probe")
                .total_files,
            1,
            "the seeded record must still be there — a delete-and-recreate over the \
             probe failure would come back empty"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Windows twin of `failed_probe_never_deletes_an_existing_store`: a file
    /// held open with no sharing mode makes the probe connection's open fail
    /// (ERROR_SHARING_VIOLATION), the same deterministic probe failure the
    /// unix test constructs through permissions. `Store::open` must fail loud
    /// and leave the store file in place.
    #[cfg(windows)]
    #[test]
    fn failed_probe_never_deletes_an_existing_store_windows() {
        use std::os::windows::fs::OpenOptionsExt;

        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-knowledge-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let db = tmp.join("index.db");
        {
            let store = Store::open(&db).expect("create store");
            assert_eq!(store.stats().unwrap().total_files, 0);
            // Seed so the survival assert can detect delete-and-recreate
            // (same rationale as the unix twin).
            store
                .upsert_many(&[rec(
                    "/tmp/docs/annual.pdf",
                    "annual.pdf",
                    Some("pdf"),
                    2048,
                    0,
                )])
                .expect("seed one file record");
        }
        // Hold the store exclusively: any subsequent open (the probe's) fails.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&db)
            .expect("hold the store without sharing");
        assert!(
            Store::open(&db).is_err(),
            "an unopenable store must fail loud instead of probing version 0"
        );
        drop(held);
        assert!(
            db.exists(),
            "the store file must survive a failed probe untouched"
        );
        let reopened =
            Store::open(&db).expect("the store must reopen after the blocking handle is released");
        assert_eq!(
            reopened
                .stats()
                .expect("stats after the failed probe")
                .total_files,
            1,
            "the seeded record must survive — a delete-and-recreate would come back empty"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A steady-state open (schema already current) no longer runs the DDL
    /// batch and the user_version write: this is what lets the open skip the
    /// schema write lock in the GUI+CLI two-process scenario. A connection
    /// holding a write transaction simulates the other process's write window;
    /// the second open must still succeed.
    #[test]
    fn steady_state_open_does_not_take_the_schema_write_lock() {
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-knowledge-steady-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let db = tmp.join("index.db");
        {
            Store::open(&db).expect("create store at current schema version");
        }
        // Simulate the other process holding a write transaction (the WAL write lock is taken).
        let writer = Connection::open(&db).unwrap();
        writer
            .busy_timeout(std::time::Duration::from_millis(5_000))
            .unwrap();
        writer.execute_batch("BEGIN IMMEDIATE;").unwrap();
        writer
            .execute_batch("CREATE TABLE IF NOT EXISTS _probe_lock (x INTEGER);")
            .unwrap();
        let opened = Store::open(&db);
        writer.execute_batch("ROLLBACK;").unwrap();
        assert!(
            opened.is_ok(),
            "steady-state open must not need the write lock another process holds: {:?}",
            opened.err()
        );
        drop(writer);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Once the steady state skips the DDL batch, connection-level PRAGMAs
    /// must still apply: synchronous is a connection setting and does not
    /// persist (only journal_mode is written into the file header); dropping
    /// it would silently fall steady-state write connections back to the FULL
    /// default.
    #[test]
    fn steady_state_open_keeps_connection_level_pragmas() {
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-knowledge-pragma-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let db = tmp.join("index.db");
        {
            Store::open(&db).expect("create store at current schema version");
        }
        let store = Store::open(&db).expect("steady-state reopen");
        let synchronous: i64 = store
            .conn
            .lock()
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            synchronous, 1,
            "steady-state write connection must keep synchronous=NORMAL (1), not the FULL default (2)"
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&tmp);
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
    fn search_excludes_directories() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_many(&[
            FileRecord {
                path: "/a/项目文档".into(),
                name: "项目文档".into(),
                ext: None,
                size: 0,
                mtime: 1,
                is_dir: true,
            },
            FileRecord {
                path: "/a/项目文档.pdf".into(),
                name: "项目文档.pdf".into(),
                ext: Some("pdf".into()),
                size: 100,
                mtime: 2,
                is_dir: false,
            },
        ])
        .unwrap();
        // FTS 路径(≥3 字符)：目录不应出现在结果里
        let hits = s
            .search(&SearchQuery {
                text: Some("项目文档".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "项目文档.pdf");
        // 无 text 全量路径：同样排除目录
        let all = s
            .search(&SearchQuery {
                limit: 10,
                ..Default::default()
            })
            .unwrap();
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

    #[test]
    fn v3_database_is_migrated_without_losing_collections() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pinvou3_store_v3_{}_{}.db",
            std::process::id(),
            suffix
        ));
        let c = Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE collections(\
               id INTEGER PRIMARY KEY,name TEXT NOT NULL,category TEXT,description TEXT,\
               created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,embed_model TEXT,\
               embed_dim INTEGER NOT NULL DEFAULT 0,status TEXT NOT NULL DEFAULT 'ready'\
             );\
             INSERT INTO collections(id,name,created_at,updated_at,status)\
             VALUES(7,'保留的知识集',1,1,'ready');\
             PRAGMA user_version=3;",
        )
        .unwrap();
        drop(c);

        let store = Store::open(&path).unwrap();
        let conn = store.conn_arc();
        let name: String = conn
            .lock()
            .query_row("SELECT name FROM collections WHERE id=7", [], |r| r.get(0))
            .unwrap();
        let version: i64 = conn
            .lock()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        let import_table: i64 = conn
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='knowledge_import_jobs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "保留的知识集");
        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(import_table, 1);
        drop(conn);
        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    #[test]
    fn newer_schema_store_is_refused_without_deleting_files() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pinvou3_store_newer_{}_{}.db",
            std::process::id(),
            suffix
        ));
        // Build a valid store first, then pretend a newer binary wrote it.
        drop(Store::open(&path).unwrap());
        let c = Connection::open(&path).unwrap();
        c.execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION + 1))
            .unwrap();
        drop(c);

        let error = match Store::open(&path) {
            Ok(_) => panic!("newer-schema store must be refused, not reopened"),
            Err(error) => error,
        };
        let message = error.to_string();
        assert!(
            message.contains("newer version") && message.contains("upgrade pinvou"),
            "{message}"
        );
        // The refusal happens before any destructive step: the store files
        // stay intact for the newer binary.
        assert!(path.exists(), "store file must survive the refusal");

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }
}
