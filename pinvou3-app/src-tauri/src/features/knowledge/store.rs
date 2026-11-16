// architecture-guard: allow-target-cfg -- the probe regression test constructs an "open-read failure" with POSIX permission bits to verify the failure is no longer folded into version 0, which would trigger the store deletion (the OS-metadata check stays inside the most cohesive test per precedent)
//! L0 元数据存储：SQLite + FTS5(trigram) 做全系统秒搜 + 去重候选查询。
//!
//! 设计：
//! - `files` 表只存元数据（名/路径/大小/时间/类型），**不存内容**。L1 内容/向量是后续层。
//! - `files_fts` 是 external-content FTS5 虚表，trigram 分词 —— 对中文文件名和子串搜索都友好
//!   （unicode61 会把一串中文当成单 token，搜不到子串；trigram 按 3-字符窗口可子串命中）。
//!
//! 并发：`Connection` Send 不 Sync，整库放 `Arc<Mutex<Connection>>`。扫描线程批量写、
//! 前端查询读，都短暂持锁；v0 单连接足够，日后抽 daemon 再上 WAL/多连接。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{
    Connection, OptionalExtension, TransactionBehavior, params, params_from_iter, types::Value,
};
use serde::{Deserialize, Serialize};

/// schema 版本。v3 起包含用户创建的 L1 数据，后续版本必须提供原地迁移。
/// v2：FTS 砍掉 path 列（path-trigram 实测占 2/3 库体积、是写入 CPU 大头）。
/// v3：新增 L1 知识库表（collections/documents/chunks/chunks_fts）。
/// v4：新增可恢复的批量导入任务、文件状态与分块暂存表。v4 现容忍两种物理形态：
/// 本版本起的全新建库不再创建 collections.embed_model/embed_dim（从未有代码读写），
/// 旧库的这两列原样保留——所有语句都走显式列清单，两形态行为一致，故不 bump 版本。
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
///
/// The range is bounded below as well as above. `user_version` is a signed
/// 32-bit field at byte offset 60 of the database header, carried by no
/// checksum outside WAL frames, so a healthy v4 store that takes a single bit
/// flip in that byte reads back negative — and a bare `version < 3` would
/// classify it as a disposable v0-era store and delete exactly the
/// non-rebuildable data this rule exists to protect. A negative version is
/// not an old schema, it is a damaged header: refuse it like a future one.
fn schema_is_disposable(version: i64) -> bool {
    (0..3).contains(&version)
}

/// Tables + FTS5 virtual table + sync triggers. Idempotent (`IF NOT EXISTS`,
/// except for the trigger replacement pair). The trigger statement is listed
/// separately as [`FILES_AU_TRIGGER`] (shared verbatim by the fresh-create
/// batch and the steady-state upgrade); this constant is spliced together by
/// [`schema_sql`].
const SCHEMA_HEAD: &str = r#"
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
-- files_au is not created here (the replacement pair is listed separately as
-- [`FILES_AU_TRIGGER`]; this batch splices the same text via [`schema_sql`]):
-- the old trigger had no WHEN clause while the UPSERT's SET list always
-- includes name=, and `AFTER UPDATE OF name` inspects the SET list, not the
-- values — so writes that leave the name unchanged (the watcher's Modify
-- events, the scanner's status refreshes) still paid a full trigram
-- delete+insert round. WHEN makes the trigger semantics match the comment
-- above. This batch covers only fresh creates and migrations; existing
-- stores already in the v4 steady state are upgraded in place by open()'s
-- read-only probe (see the FILES_AU_TRIGGER comment).
"#;

/// The paired replacement statements for files_au. DROP+CREATE is wrapped in
/// BEGIN IMMEDIATE for atomicity: when two processes race a first-time store
/// creation (or this same upgrade), statement-level interleaving would let
/// the unconditional CREATE hit "already exists" — the old all-`IF NOT
/// EXISTS` batch had no such failure mode — and with the pair atomized, the
/// loser merely waits for the lock and replays idempotently. The SCHEMA
/// batch (fresh create / migration) and open()'s steady-state upgrade share
/// this one text, so the shape cannot drift
/// (`trigger_replacement_matches_the_schema_built_shape`). The upgrade probe
/// recognizes the old shape by the [`FILES_AU_WHEN_CLAUSE`] substring:
/// sqlite_master.sql preserves the statement text as the trigger was
/// created, and the old shape lacks that clause.
const FILES_AU_TRIGGER: &str = r#"
BEGIN IMMEDIATE;
DROP TRIGGER IF EXISTS files_au;
CREATE TRIGGER files_au AFTER UPDATE OF name ON files
WHEN old.name IS NOT new.name
BEGIN
    INSERT INTO files_fts(files_fts, rowid, name) VALUES('delete', old.id, old.name);
    INSERT INTO files_fts(rowid, name) VALUES (new.id, new.name);
END;
COMMIT;
"#;

/// The WHEN clause text unique to the new-shape `files_au`. open()'s
/// steady-state probe uses this substring to tell the old shape (no WHEN;
/// every upsert paid a pointless trigram rewrite) from the new one.
/// Extracted into a constant so the probe and the trigger text share one
/// matcher: if a future reflow of [`FILES_AU_TRIGGER`] changes the clause's
/// wording, `trigger_replacement_matches_the_schema_built_shape` goes red
/// instead of the silent degradation where a probe-substring mismatch
/// replays the upgrade on every steady-state open.
const FILES_AU_WHEN_CLAUSE: &str = "WHEN old.name IS NOT new.name";

/// Tail of the fresh-create batch (the L1 knowledge base and import job
/// tables), spliced with [`SCHEMA_HEAD`] and [`FILES_AU_TRIGGER`] into the
/// full batch.
const SCHEMA_TAIL: &str = r#"

-- ============ L1 知识库（见 l1.rs）============
-- 知识集：用户圈定的一批文件，内容化后供检索/问答。
CREATE TABLE IF NOT EXISTS collections (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    category    TEXT,
    description TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
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

/// The fresh-create batch: head (L0 tables and trigger declarations) +
/// files_au replacement + tail (L1 tables). Idempotent (`IF NOT EXISTS`,
/// except the files_au replacement pair — it carries its own BEGIN IMMEDIATE
/// atomization, and repeating it is idempotent).
fn schema_sql() -> String {
    format!("{SCHEMA_HEAD}{FILES_AU_TRIGGER}{SCHEMA_TAIL}")
}

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

/// Watcher incremental path only ([`upsert_many_if_changed`]): the only
/// difference from [`UPSERT_SQL`] is a WHERE in the conflict branch — the
/// whole row is skipped when its content (name/ext/size/mtime/is_dir) is
/// fully identical to the store's. Burst events for the same file within a
/// second, chmod and other pure-metadata churn no longer rewrite the row per
/// event. When a row is skipped, its indexed_at keeps the time of the last
/// actual change. The restart fallback in `last_scan_finished_at` reads
/// `MAX(indexed_at)` (see that method), so the scan path keeps
/// [`UPSERT_SQL`]: every row the scanner actually writes touches
/// `indexed_at` — rows whose (mtime, size) match the snapshot are
/// pre-skipped by the scanner and never reach the statement — which keeps
/// the fallback's semantics unchanged while the write-amplification elision
/// lives only on the incremental side. The WHERE uses bare column names for
/// the old row and `excluded.` for the new one; `IS NOT` is NULL-safe
/// (SQLite's NULL-safe comparison).
const UPSERT_IF_CHANGED_SQL: &str = r#"
INSERT INTO files(path, name, ext, size, mtime, is_dir, status, indexed_at)
VALUES(?1, ?2, ?3, ?4, ?5, ?6, 'indexed', strftime('%s','now'))
ON CONFLICT(path) DO UPDATE SET
    name=excluded.name, ext=excluded.ext, size=excluded.size,
    mtime=excluded.mtime, is_dir=excluded.is_dir, status='indexed',
    indexed_at=excluded.indexed_at
WHERE excluded.name IS NOT name OR excluded.ext IS NOT ext
   OR excluded.size IS NOT size OR excluded.mtime IS NOT mtime
   OR excluded.is_dir IS NOT is_dir
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
    /// Rowid, exposed so paging callers can round-trip the `(mtime, id)`
    /// keyset cursor. `mtime` alone is second-resolution and cannot express
    /// a page boundary that falls inside a tie.
    pub id: i64,
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

/// FTS5 文本的最短字符数：≥ 该值走 trigram FTS，否则 LIKE 兜底。文件检索
/// （[`Store::search`]）与 chunk 检索（`l1::search_fts`）共用一个阈值，
/// 避免两条检索路径各自漂移。
pub(crate) const FTS_MIN_CHARS: usize = 3;

/// FTS5 MATCH 的短语字面量：双引号包成字符串字面量做子串匹配，内部引号
/// 翻倍转义。文件与 chunk 两条 FTS 路径共用同一拼写。
pub(crate) fn fts_phrase(q: &str) -> String {
    format!("\"{}\"", q.replace('"', "\"\""))
}

/// LIKE 兜底的模式串：`%` 包裹，内容剥掉 `%`/`_` 通配符（见
/// [`escape_like`]）。文件与 chunk 两条 LIKE 路径共用同一拼写。
pub(crate) fn like_pattern(q: &str) -> String {
    format!("%{}%", escape_like(q))
}

/// 秒搜查询条件。`text` 为名/路径子串；其余为结构化过滤。
/// The serde shape is the `kb_search` IPC payload (camelCase); the headless
/// CLI constructs it Rust-to-Rust, so there is exactly one query type.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    pub text: Option<String>,
    #[serde(default)]
    pub exts: Vec<String>,
    pub mtime_after: Option<i64>,
    pub mtime_before: Option<i64>,
    /// Tie half of the `(mtime, id)` keyset cursor; requires
    /// `mtime_before`. Together the pair means "strictly after position
    /// `(mtime_before, id_before)` in the `(mtime DESC, id DESC)` order",
    /// which keeps paging exact when same-second mtimes straddle the page
    /// boundary. `mtime_before` alone stays an inclusive time-upper-bound
    /// filter (what the NL rules and the GUI time filters use).
    pub id_before: Option<i64>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    #[serde(default)]
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
    /// 仅 v0–v2 的旧库（纯可重建 L0 缓存）才走删库重建；v3 原地迁移，高于
    /// 本版本的库拒绝打开、绝不删除（见下）。
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
            // A damaged header is refused like a newer store: never migrated
            // in place, never deleted (see `schema_is_disposable` — a bit
            // flip in the signed 32-bit `user_version` field reads back
            // negative but is not an old schema).
            if version > SCHEMA_VERSION {
                return Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_NOTADB),
                    Some(format!(
                        "knowledge store was written by a newer version (schema v{version} \
                         > v{SCHEMA_VERSION}); upgrade pinvou"
                    )),
                ));
            }
            if version < 0 {
                return Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_NOTADB),
                    Some(format!(
                        "knowledge store has a damaged header (schema v{version} < 0); \
                         restore it from a backup or delete it manually after verifying \
                         nothing else needs it"
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
            //
            // journal_mode is re-asserted for a different reason: it lives
            // only in the DDL batch this branch skips, and the create path
            // can silently fail to take it (`PRAGMA journal_mode = WAL`
            // returns the CURRENT mode as a row instead of erroring when
            // another connection holds a read lock, which the two-process
            // pair makes reachable). A store stuck in rollback-journal mode
            // would then get synchronous=NORMAL forever — a combination
            // SQLite documents as risking corruption on power loss, whereas
            // NORMAL under WAL is safe. Re-asserting is a no-op on a store
            // that is already WAL and takes no write lock.
            let mode: String = w.query_row("PRAGMA journal_mode = WAL;", [], |row| row.get(0))?;
            if !mode.eq_ignore_ascii_case("wal") {
                return Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                    Some(format!(
                        "knowledge store is in '{mode}' journal mode, not WAL; refusing to \
                         apply synchronous=NORMAL (unsafe without WAL). To recover, set \
                         `PRAGMA journal_mode=WAL;` on the store file by hand (keeps the \
                         knowledge sets), or delete the store and rescan (rebuilds the \
                         index but loses the knowledge sets)"
                    )),
                ));
            }
            w.execute_batch("PRAGMA synchronous = NORMAL;")?;
            // A v4 steady-state store may still carry the old-shape files_au
            // (opened before the WHEN upgrade landed; this branch skips the
            // DDL batch, so the batch's replacement never reaches existing
            // stores). Identify the old shape with a read-only probe
            // (sqlite_master.sql without the WHEN clause) and run the
            // replacement when the trigger is missing or wrong-shaped —
            // the probe is a plain SELECT and takes no write lock, so the
            // steady-state open's two-process contention window is unchanged
            // (a present, correct trigger means neither branch fires); the
            // replacement statement carries its own BEGIN IMMEDIATE, so
            // racing another process's identical upgrade just waits for the
            // lock and replays idempotently (FTS content unchanged, a pure
            // metadata operation). Rebuilding covers the externally-dropped
            // trigger too — unlike an externally dropped *table*, a missing
            // `files_au` fails nothing: DML just stops updating FTS, so
            // renames would silently go stale in search until a rescan (the
            // "statements fail explicitly" trade does not apply to
            // triggers). Only `files_au` is probed and rebuilt: its shape
            // changed in this schema generation (the WHEN clause), so
            // existing stores legitimately carry a stale shape; `files_ai`
            // and `files_ad` never changed shape, so an externally dropped
            // one is tampering rather than a stale upgrade and stays under
            // the table-level no-self-heal stance instead of a per-open
            // rebuild. Should the replacement itself lose the
            // busy_timeout race (a one-time migration window on stores
            // still carrying the old shape; a >5s write-lock holder makes
            // BEGIN IMMEDIATE time out where a pre-upgrade binary's
            // steady-state open took no write lock at all), report and open
            // with the old trigger — correct, just slower — and let the
            // next open retry the upgrade.
            let trigger_sql: Option<String> = w
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'files_au'",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            if !trigger_sql
                .as_deref()
                .is_some_and(|sql| sql.contains(FILES_AU_WHEN_CLAUSE))
            {
                if let Err(err) = w.execute_batch(FILES_AU_TRIGGER) {
                    eprintln!(
                        "[kb_store] files_au upgrade skipped (will retry on next open): {err}"
                    );
                }
            }
        } else {
            // Fresh create / in-place v3 migration: the DDL batch and the
            // version write each take the write lock once; busy_timeout makes
            // them wait out the other process's short transaction instead of
            // failing immediately.
            w.execute_batch(&schema_sql())?;
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
        conn.execute_batch(&schema_sql())?;
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

    /// 批量 upsert（扫描器用，单事务）。
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

    /// Incremental upsert (watcher only, single transaction): rows whose
    /// content matches the store are skipped whole; see
    /// [`UPSERT_IF_CHANGED_SQL`] for the write-amplification elision. The
    /// insert branch is identical to [`upsert_many`]'s.
    pub fn upsert_many_if_changed(&self, recs: &[FileRecord]) -> rusqlite::Result<()> {
        let mut guard = self.conn.lock();
        let tx = guard.transaction()?;
        {
            let mut stmt = tx.prepare_cached(UPSERT_IF_CHANGED_SQL)?;
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

    /// Watcher deletes: a directory record is deleted together with its
    /// subtree, a file record degrades to an exact delete. Single transaction;
    /// FTS is kept in sync by the triggers.
    ///
    /// The transaction is `Immediate`: a SELECT-first write transaction under
    /// the deferred behavior, when its write upgrade collides in WAL with a
    /// concurrent process's commit (GUI + headless CLI on one store is
    /// supported usage), fails with `SQLITE_BUSY_SNAPSHOT` and the busy
    /// handler does not retry — turning a waitable lock race into a
    /// whole-batch delete failure that drops events. The write lock is taken
    /// at BEGIN, so the waiting happens up front. (The library has two more
    /// isomorphic SELECT-first write transactions: import_jobs' job
    /// preparation and entry claiming. Those are self-recovering job-state
    /// machines — failure returns Err and the entries stay pending for the
    /// next retry — left unchanged here; see `prepare_items`/`claim_next`.)
    ///
    /// Why subtree deletion is needed: when a directory is renamed/moved
    /// out, inotify only surfaces that directory's own event on the (still
    /// watched) parent; its files get no events at all, and an exact delete
    /// would leave a whole tree of "ghosts" in the index until the next
    /// rescan. `is_dir` is probed first, and only a directory takes the
    /// prefix delete — the prefix range rides the UNIQUE index (`path TEXT
    /// NOT NULL UNIQUE`, BINARY ordering): an index seek plus deletion of
    /// the hit rows, not a full-table scan; files (the vast majority of
    /// events) keep the index exact-delete cost. Known residual gap (ghost
    /// direction, rescan covers): the store holds a stale `is_dir=false`
    /// row while the disk already has a directory — this call only
    /// exact-deletes, and the subtree rows wait for rescan (the is_dir gate
    /// is a scan-saving trade; batch merging of event streams is a
    /// watcher-side matter).
    ///
    /// The prefix match is an exact range compare over
    /// `[path + primary separator, path + separator successor)`: paths may
    /// contain `%`/`_`/`[`, so LIKE/GLOB's pattern semantics are unusable.
    /// The lower bound is `p + sep`; the upper bound is `p + the character
    /// after the separator byte` (unix `/`(0x2F)→`0`(0x30), Windows
    /// `\`(0x5C)→`]`(0x5D); both separators are ASCII, so the successor is
    /// unambiguous) — under BINARY byte ordering, any row starting with
    /// `p+sep` decides its comparison against the upper bound at the
    /// separator byte itself, and extending the suffix never changes the
    /// outcome, so the half-open interval is exactly the set of rows
    /// byte-prefixed by `p+sep` (including a root row stored under the
    /// `p+sep` spelling itself). Every row in the store passed
    /// `record_from_parts`'s `to_str` gate (legal UTF-8, and UTF-8 byte
    /// order equals code point order), so CJK/emoji paths compare exactly;
    /// no in-app BLOB rows can exist (the only writers bind TEXT).
    /// Externally injected BLOB rows match neither delete — SQLite's
    /// storage-class ordering puts BLOB above every TEXT, so `path = ?` (a
    /// TEXT binding) is not equal and the range's upper-bound comparison
    /// also judges larger — they ghost until the next rescan; no in-app
    /// path can reach this, recorded as the fact it is. The same miss
    /// befalls a row spelled with a **non-primary** separator (e.g. a
    /// `/`-spelled root row on Windows, whose bytes sort below every
    /// `\`-prefixed range): the same ghost-until-rescan class, unreachable
    /// while scanner and watcher share one canonical spelling (the wiring
    /// contract).
    ///
    /// Separator spellings: the input tolerates a **trailing separator**
    /// (the wiring side may pass a root PathBuf carrying `/`), and mixed
    /// tails (`dir\/`); both are stripped in one pass by "primary
    /// separator or `/`" before comparing. Rows, on the other hand, may be
    /// stored under a **trailing-separator** spelling (walkdir records scan
    /// roots verbatim; passing `--root /media/usb/` stores `/media/usb/`)
    /// — the is_dir probe falls back to the `p + primary separator`
    /// root-row spelling when the canonical spelling misses, and the prefix
    /// pattern hits that row exactly, so the whole tree is still deleted.
    ///
    /// Whole-index inputs are skipped, not swept: an input that trims to
    /// empty (the bare separator spelling, `/` or `\`) is dropped, and so is
    /// a drive-root spelling (`C:` after `C:\` trims — the spelling a
    /// Windows GUI root actually produces), because a single transient
    /// false NotFound event must not turn into an unbounded sweep of every
    /// indexed row; the rows wait for the rescan instead. The verbatim
    /// spellings of the same root are guarded identically: the
    /// canonical-spelling contract makes rows carry the `\\?\C:\…` prefix,
    /// so such a NotFound event can arrive spelled `\\?\C:\` (which trims
    /// to `\\?\C:`, or keeps its trailing backslash on Unix where `\` is
    /// not MAIN_SEPARATOR) — see `is_drive_root`.
    pub fn delete_tree(&self, paths: &[String]) -> rusqlite::Result<()> {
        let mut guard = self.conn.lock();
        let tx = guard.transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut q = tx.prepare_cached("SELECT is_dir FROM files WHERE path = ?1")?;
            let mut del_exact = tx.prepare_cached("DELETE FROM files WHERE path = ?1")?;
            let mut del_sub =
                tx.prepare_cached("DELETE FROM files WHERE path >= ?1 AND path < ?2")?;
            for p_raw in paths {
                let p = p_raw.trim_end_matches(|c| c == std::path::MAIN_SEPARATOR || c == '/');
                if p.is_empty() || is_drive_root(p) {
                    continue;
                }
                // A row may be stored under the canonical spelling or the
                // "p + separator" root-row spelling; probe both. The exact
                // DELETE only runs when the exact probe hit: it uses the
                // same BINARY equality as the probe, so after a double miss
                // it provably affects zero rows — and double misses are the
                // hot path (delete events for never-indexed paths: build
                // churn, symlink replacements).
                let with_sep = format!("{}{}", p, std::path::MAIN_SEPARATOR);
                let exact_hit: Option<bool> = q.query_row(params![p], |r| r.get(0)).optional()?;
                let is_dir: Option<bool> = match exact_hit {
                    Some(dir) => Some(dir),
                    None => q.query_row(params![with_sep], |r| r.get(0)).optional()?,
                };
                if exact_hit.is_some() {
                    del_exact.execute(params![p])?;
                }
                if is_dir == Some(true) {
                    // Rows can only be `p` itself, `p + separator` (a
                    // trailing-separator root-row spelling), or
                    // `p + separator + descendant`; the half-open interval
                    // `[p+sep, p+sep-successor)` is exactly that whole set
                    // (see the method doc), so siblings sharing only prefix
                    // characters (like `prefix-sibling.md`, or a `p0` that
                    // exactly equals the upper bound) are not wrongly
                    // deleted, while the trailing-separator root row itself
                    // is caught exactly by the lower bound and cleared with
                    // the rest.
                    let upper = format!("{}{}", p, (std::path::MAIN_SEPARATOR as u8 + 1) as char);
                    del_sub.execute(params![with_sep, upper])?;
                }
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
        if let Some(t) = text.filter(|t| t.chars().count() >= FTS_MIN_CHARS) {
            sql.push_str(
                "SELECT f.id, f.path, f.name, f.ext, f.size, f.mtime \
                 FROM files_fts JOIN files f ON f.id = files_fts.rowid \
                 WHERE f.status='indexed' AND f.is_dir=0 AND files_fts MATCH ?",
            );
            // trigram：短语字面量拼写见 [`fts_phrase`]。
            vals.push(Value::Text(fts_phrase(t)));
        } else {
            sql.push_str("SELECT f.id, f.path, f.name, f.ext, f.size, f.mtime FROM files f WHERE f.status='indexed' AND f.is_dir=0");
            if let Some(t) = text {
                sql.push_str(" AND (f.name LIKE ? OR f.path LIKE ?)");
                let like = like_pattern(t);
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
            if let Some(id) = q.id_before {
                // Keyset step: strictly after `(v, id)` in the
                // `(mtime DESC, id DESC)` order — the lexicographic
                // decomposition of `(mtime, id) < (v, id)`. Both arms lead
                // with `f.mtime`, so the planner can still drive the scan
                // from `idx_files_mtime`.
                sql.push_str(" AND (f.mtime < ? OR (f.mtime = ? AND f.id < ?))");
                vals.push(Value::Integer(v));
                vals.push(Value::Integer(v));
                vals.push(Value::Integer(id));
            } else {
                sql.push_str(" AND f.mtime <= ?");
                vals.push(Value::Integer(v));
            }
        } else if q.id_before.is_some() {
            // A half cursor would silently degrade to the tie-losing
            // mtime-only filter; failing loudly keeps callers honest.
            return Err(rusqlite::Error::InvalidParameterName(
                "id_before requires mtime_before: the keyset cursor is the (mtime, id) pair".into(),
            ));
        }
        // Saturating, not `as`: SQLite integers are signed, so a `u64` past
        // `i64::MAX` wraps negative and inverts the filter — `size >= -1`
        // matches every row instead of none. Same failure class as the raw
        // `limit` cast fixed above, and now reachable with caller-controlled
        // values through the headless `KnowledgeService::search`.
        if let Some(v) = q.min_size {
            sql.push_str(" AND f.size >= ?");
            vals.push(Value::Integer(i64::try_from(v).unwrap_or(i64::MAX)));
        }
        if let Some(v) = q.max_size {
            sql.push_str(" AND f.size <= ?");
            vals.push(Value::Integer(i64::try_from(v).unwrap_or(i64::MAX)));
        }
        // The order is total: `f.id` breaks the second-resolution mtime ties
        // that bulk copies make routine. `f.id` is the rowid, so
        // `idx_files_mtime` still serves the scan. (SQLite's index order
        // happens to sort ties by rowid today; naming it in the ORDER BY
        // makes the total order contractual for whatever plan the optimizer
        // picks, instead of an artifact of the current index layout.) The
        // cursor over this order is the `(mtime, id)` pair carried by
        // `mtime_before` + `id_before`; `mtime_before` alone is just an
        // inclusive time filter and re-returns or skips ties at a page
        // boundary, so paging callers must round-trip the last hit's `id`.
        sql.push_str(" ORDER BY f.mtime DESC, f.id DESC LIMIT ?");
        vals.push(Value::Integer(limit));

        let guard = self.read.lock();
        let mut stmt = guard.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(vals.iter()), |row| {
            Ok(FileHit {
                id: row.get(0)?,
                path: row.get(1)?,
                name: row.get(2)?,
                ext: row.get(3)?,
                size: row.get::<_, i64>(4)? as u64,
                mtime: row.get(5)?,
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

/// A trimmed delete input that is a drive-root spelling (`C:` — what `C:\`
/// trims to; the same whole-index class as a bare-separator input for a
/// Windows store root). The canonical-spelling contract makes rows carry the
/// verbatim `\\?\C:\…` prefix, so the verbatim forms (`\\?\C:` after the
/// caller's separator trim, and `\\?\C:\` whose backslash is not
/// MAIN_SEPARATOR on Unix) denote the same root and are guarded identically.
/// Only the exact two-character spelling matches after the prefix/backslash
/// strip: a one-character name or a longer path ending in `:` is a legal
/// entity name and deletes normally. Side effect of the unconditional
/// check: on Unix a directory literally named `C:` is also skipped (legal
/// name, ghosts until rescan) — accepted, the class is Windows-shaped and
/// the alternative (Path::parent semantics) would make the guard
/// platform-dependent. UNC share roots (`\\srv\share`, verbatim
/// `\\?\UNC\srv\share`) are *not* covered here — same whole-index class,
/// still unguarded, listed in the watcher's known limits.
fn is_drive_root(p: &str) -> bool {
    let bare = p.strip_prefix(r"\\?\").unwrap_or(p).trim_end_matches('\\');
    let b = bare.as_bytes();
    b.len() == 2 && b[1] == b':' && b[0].is_ascii_alphabetic()
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
        // `user_version` is a signed field in an unchecksummed header slot, so
        // a bit flip in a healthy v4 store reads back negative. A bare
        // `version < 3` classified that as a disposable v0-era store and
        // deleted exactly the data this rule protects. A negative version is a
        // damaged header, not an old schema.
        for version in -64..0 {
            assert!(
                !super::schema_is_disposable(version),
                "a negative user_version ({version}) is a damaged header, not a pre-v3 \
                 schema — it must be refused, never deleted"
            );
        }
        assert!(!super::schema_is_disposable(i64::MIN));
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

    /// Fixture paths are spelled uniformly via the primary separator: the
    /// boundary math of delete_tree (lower bound `p+sep`, upper bound
    /// `p + separator successor`) uses MAIN_SEPARATOR, so `/`-literal
    /// fixtures on Windows (`\` separator, successor `]`) would turn these
    /// pure-SQL tests all-red (rows sorted under `/` fall outside the lower
    /// bound). Paths built here share the delete implementation's source, so
    /// the semantics agree on any platform.
    fn sep_path(parts: &[&str]) -> String {
        let sep = std::path::MAIN_SEPARATOR;
        let mut s = String::from(sep);
        s.push_str(&parts.join(&sep.to_string()));
        s
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
    fn search_orders_mtime_ties_by_rowid() {
        // `mtime` is second-resolution, so ties are routine (bulk copies
        // preserve mtimes), and the `(mtime, id)` pair is the keyset cursor
        // callers page with. A mtime-only ORDER BY leaves tie rows to the
        // query plan: identical queries could reorder same-second hits,
        // shuffling the cursor boundary between pages. The `f.id` tiebreak
        // makes the order total — insert a/b/c in that order, expect the tie
        // read back id-DESC — so a repeated query must replay it exactly.
        // SQLite's current index layout satisfies this by accident; the
        // clause and this test pin it as a contract against plan drift.
        let s = Store::open_in_memory().unwrap();
        s.upsert_many(&[
            rec("/t/a.pdf", "a.pdf", Some("pdf"), 10, 500),
            rec("/t/b.pdf", "b.pdf", Some("pdf"), 11, 500),
            rec("/t/c.pdf", "c.pdf", Some("pdf"), 12, 500),
            rec("/t/older.md", "older.md", Some("md"), 13, 400),
        ])
        .unwrap();
        let q = SearchQuery {
            mtime_before: Some(500),
            limit: 10,
            ..Default::default()
        };
        let hits = s.search(&q).unwrap();
        let names: Vec<_> = hits.iter().map(|hit| hit.name.as_str()).collect();
        assert_eq!(names, vec!["c.pdf", "b.pdf", "a.pdf", "older.md"]);
        let again = s.search(&q).unwrap();
        assert_eq!(hits, again, "a repeated query must not reorder tie rows");
    }

    #[test]
    fn keyset_cursor_pages_ties_without_duplicates_or_gaps() {
        // The page-boundary shape: three rows tied at one mtime with a page
        // size smaller than the tie. An mtime-only cursor cannot express the
        // boundary — inclusive `<=` re-returns the tie, decrementing skips
        // it — so the cursor must carry the composite `(mtime, id)` and the
        // next page must resume strictly after the last row. Drives the full
        // partition: every row exactly once, then exhaustion.
        let s = Store::open_in_memory().unwrap();
        s.upsert_many(&[
            rec("/t/a.pdf", "a.pdf", Some("pdf"), 10, 500),
            rec("/t/b.pdf", "b.pdf", Some("pdf"), 11, 500),
            rec("/t/c.pdf", "c.pdf", Some("pdf"), 12, 500),
            rec("/t/older.md", "older.md", Some("md"), 13, 400),
        ])
        .unwrap();

        let mut pages: Vec<Vec<FileHit>> = Vec::new();
        let mut cursor: Option<(i64, i64)> = None;
        loop {
            let page = s
                .search(&SearchQuery {
                    mtime_before: cursor.map(|(m, _)| m),
                    id_before: cursor.map(|(_, i)| i),
                    limit: 2,
                    ..Default::default()
                })
                .unwrap();
            let exhausted = page.is_empty();
            cursor = page.last().map(|h| (h.mtime, h.id));
            pages.push(page);
            if exhausted {
                break;
            }
        }
        assert_eq!(
            pages.len(),
            3,
            "page size 2 over 4 rows: two full pages + empty tail"
        );
        let names: Vec<Vec<_>> = pages
            .iter()
            .map(|p| p.iter().map(|h| h.name.as_str()).collect())
            .collect();
        assert_eq!(
            names,
            vec![vec!["c.pdf", "b.pdf"], vec!["a.pdf", "older.md"], vec![]],
            "page 2 must resume strictly after (500, b.id), not re-return the tie"
        );

        // No duplicates, no omissions across the whole drive.
        let mut seen: Vec<&str> = pages
            .iter()
            .flat_map(|p| p.iter().map(|h| h.name.as_str()))
            .collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen, vec!["a.pdf", "b.pdf", "c.pdf", "older.md"]);

        // A half cursor fails loudly instead of silently degrading to the
        // tie-losing mtime-only filter.
        let half = s.search(&SearchQuery {
            id_before: Some(1),
            limit: 2,
            ..Default::default()
        });
        assert!(half.is_err(), "id_before without mtime_before must error");
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
               created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,\
               status TEXT NOT NULL DEFAULT 'ready'\
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

    /// A negative `user_version` is a damaged header (it is read back
    /// negative only when the signed 32-bit field was corrupted — see
    /// `schema_is_disposable`), so it must be REFUSED like a newer store:
    /// never migrated in place (which would stamp over the damage) and never
    /// deleted. Pins the refusal at the `Store::open` dispatch level; the
    /// predicate test below only pins `schema_is_disposable`.
    #[test]
    fn negative_user_version_store_is_refused_without_migration_or_deletion() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pinvou3_store_negative_{}_{}.db",
            std::process::id(),
            suffix
        ));
        // Build a valid store, then damage the version field exactly the way
        // a bit flip in header bytes 60..=63 would read back.
        drop(Store::open(&path).unwrap());
        let c = Connection::open(&path).unwrap();
        c.execute_batch("PRAGMA user_version = -1;").unwrap();
        drop(c);

        let error = match Store::open(&path) {
            Ok(_) => panic!("negative user_version must be refused, not self-healed"),
            Err(error) => error,
        };
        let message = error.to_string();
        assert!(
            message.contains("damaged header"),
            "the refusal must name the damage: {message}"
        );
        // The refusal happens before any destructive step AND before the
        // migration branch: the file keeps its damaged version (not stamped
        // to the current schema) and stays on disk.
        assert!(path.exists(), "store file must survive the refusal");
        let c = Connection::open(&path).unwrap();
        let version: i64 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        drop(c);
        assert_eq!(
            version, -1,
            "the damaged header must not be stamped to v{SCHEMA_VERSION} by a refusal-path open"
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    /// Steady-state upgrade pin: the DDL batch runs only on fresh create /
    /// migration, and a v4 steady-state open takes the branch that skips the
    /// batch — if an existing store still carries the old (no-WHEN) files_au,
    /// open() must identify it via the read-only probe and replace it in
    /// place. Only this test can see that scenario (open_in_memory always
    /// runs the full batch, so its trigger is naturally new-shape). Also pins
    /// upgrade idempotence (a second open leaves the shape stable).
    #[test]
    fn open_upgrades_a_v4_store_still_carrying_the_old_trigger_shape() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pinvou3_store_oldtrig_{}_{}.db",
            std::process::id(),
            suffix
        ));

        let trigger_sql = |path: &Path| {
            let c = Connection::open(path).unwrap();
            c.query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'files_au'",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap()
        };

        // After a fresh binary creates the store, degrade files_au back to
        // the old (no-WHEN) shape — exactly simulating every difference a
        // "v4 steady-state store written by an old binary" presents to the
        // probe.
        drop(Store::open(&path).unwrap());
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "DROP TRIGGER files_au;
                 CREATE TRIGGER files_au AFTER UPDATE OF name ON files
                 BEGIN
                     INSERT INTO files_fts(files_fts, rowid, name) VALUES('delete', old.id, old.name);
                     INSERT INTO files_fts(rowid, name) VALUES (new.id, new.name);
                 END;",
            )
            .unwrap();
        }
        assert!(
            !trigger_sql(&path).contains("WHEN old.name IS NOT new.name"),
            "fixture must start from the old (no-WHEN) shape"
        );

        // Reopen: the steady-state probe identifies the old shape →
        // in-place replacement.
        let store = Store::open(&path).unwrap();
        drop(store);
        let upgraded = trigger_sql(&path);
        assert!(
            upgraded.contains("WHEN old.name IS NOT new.name"),
            "a v4 steady-state open must upgrade the old trigger in place, got: {upgraded}"
        );

        // Upgrade idempotence: open once more, shape stable, no error.
        drop(Store::open(&path).unwrap());
        assert_eq!(trigger_sql(&path), upgraded, "second open must be a no-op");

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    /// An **externally dropped** `files_au` must be rebuilt on open, not
    /// just an old-shaped one: unlike a dropped table, a missing trigger
    /// fails nothing at DML time — renames silently stop updating FTS and
    /// search goes stale until a rescan, so the "statements fail explicitly"
    /// trade does not cover this class. When the trigger exists with the
    /// right shape, the replacement batch never runs at all (the probe
    /// short-circuits), so the steady-state open takes no write lock; when
    /// the batch does run, it is wrapped in `BEGIN IMMEDIATE` and takes the
    /// write lock for its whole (one-time) duration.
    #[test]
    fn open_rebuilds_an_externally_dropped_files_au_trigger() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pinvou3_store_droptrig_{}_{}.db",
            std::process::id(),
            suffix
        ));

        let trigger_sql = |path: &Path| {
            let c = Connection::open(path).unwrap();
            c.query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'files_au'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .unwrap()
        };

        drop(Store::open(&path).unwrap());
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch("DROP TRIGGER files_au;").unwrap();
        }
        assert!(
            trigger_sql(&path).is_none(),
            "fixture must start with files_au gone"
        );

        // Reopen: the probe's missing arm rebuilds the trigger in place.
        drop(Store::open(&path).unwrap());
        let rebuilt = trigger_sql(&path);
        assert!(
            rebuilt.is_some(),
            "a missing files_au must be rebuilt on open"
        );
        assert!(
            rebuilt
                .as_deref()
                .unwrap()
                .contains("WHEN old.name IS NOT new.name"),
            "the rebuilt trigger must carry the WHEN clause, got: {rebuilt:?}"
        );

        // And the rebuilt trigger actually follows renames (the FTS
        // staleness this class would otherwise cause).
        let store = Store::open(&path).unwrap();
        let p = sep_path(&["t", "rebuilt.pdf"]);
        store
            .upsert_many(&[rec(&p, "before-name.pdf", Some("pdf"), 1, 1)])
            .unwrap();
        store
            .upsert_many(&[rec(&p, "after-name.pdf", Some("pdf"), 2, 2)])
            .unwrap();
        let hits = |text: &str| {
            store
                .search(&SearchQuery {
                    text: Some(text.into()),
                    limit: 10,
                    ..Default::default()
                })
                .unwrap()
                .len()
        };
        assert_eq!(hits("before"), 0, "the rebuilt trigger must follow renames");
        assert_eq!(hits("after"), 1);

        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    /// The upgrade path and the create path share the same FILES_AU_TRIGGER
    /// text (spliced by schema_sql); this pins that the trigger statement
    /// text built by both routes (sqlite_master.sql) is identical, so a
    /// future inline-back into SCHEMA cannot drift the two copies apart.
    #[test]
    fn trigger_replacement_matches_the_schema_built_shape() {
        let via_schema = Connection::open_in_memory().unwrap();
        via_schema.execute_batch(&schema_sql()).unwrap();
        // The trigger validates referenced tables at creation, so the
        // replacement statement needs SCHEMA_HEAD to create files/files_fts
        // first.
        let via_upgrade = Connection::open_in_memory().unwrap();
        via_upgrade.execute_batch(SCHEMA_HEAD).unwrap();
        via_upgrade.execute_batch(FILES_AU_TRIGGER).unwrap();
        let sql = |conn: &Connection| {
            conn.query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'files_au'",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap()
        };
        assert_eq!(sql(&via_schema), sql(&via_upgrade));
        // Probe-substring anti-drift pin: both the trigger const text and
        // the schema-built stored sql must contain the WHEN clause the
        // steady-state probe matches on — otherwise every open would
        // misjudge the old shape and idly replay the upgrade (idempotent,
        // but breaks the "steady-state open takes no write lock" design).
        assert!(
            FILES_AU_TRIGGER.contains(FILES_AU_WHEN_CLAUSE),
            "the trigger const must keep the WHEN clause the steady-state probe matches on"
        );
        assert!(
            sql(&via_schema).contains(FILES_AU_WHEN_CLAUSE),
            "the schema-built stored sql must contain the probe substring"
        );
    }

    /// Every connection a file-backed open creates must carry the 5 s
    /// `busy_timeout`: the two-process contention case (a headless `agent
    /// run` and the GUI, or two headless runs) is exactly what the timeout
    /// exists for, and the steady-state no-DDL open only stays lock-free if
    /// a contended `BEGIN` waits instead of failing immediately. Pinning the
    /// pragma value on both connections catches a dropped `busy_timeout`
    /// without having to construct real cross-process contention in a test.
    #[test]
    fn file_backed_open_sets_busy_timeout_on_both_connections() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pinvou3_store_busy_{}_{}.db",
            std::process::id(),
            suffix
        ));
        let store = Store::open(&path).unwrap();
        let write_timeout: i64 = store
            .conn
            .lock()
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        // `read` is query_only; reading a pragma is allowed there.
        let read_timeout: i64 = store
            .read
            .lock()
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            write_timeout, 5_000,
            "the write connection must keep its 5s busy_timeout"
        );
        assert_eq!(
            read_timeout, 5_000,
            "the read-only connection must keep its 5s busy_timeout"
        );
        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    /// The search limit is clamped to [`SEARCH_LIMIT_CAP`] rather than cast
    /// raw: a `usize::MAX` caller limit must not become an unbounded query
    /// (the old bare `as i64` wrapped to -1, which SQLite reads as no limit
    /// at all) — the headless search surface takes caller-provided limits,
    /// so the clamp is load-bearing, not cosmetic.
    #[test]
    fn search_limit_is_clamped_to_the_cap_by_seed() {
        let store = Store::open_in_memory().unwrap();
        // Seed more than the cap through the same upsert path the scanner
        // uses; one distinct file per record.
        let records: Vec<FileRecord> = (0..SEARCH_LIMIT_CAP + 40)
            .map(|i| FileRecord {
                path: format!("/clamp/docs/file_{i}.md"),
                name: format!("file_{i}.md"),
                ext: Some("md".into()),
                size: 16,
                mtime: 1_700_000_000 + i as i64,
                is_dir: false,
            })
            .collect();
        store.upsert_many(&records).unwrap();

        // A below-cap explicit limit is honored exactly.
        let hits = store
            .search(&SearchQuery {
                text: None,
                exts: Vec::new(),
                mtime_after: None,
                mtime_before: None,
                id_before: None,
                min_size: None,
                max_size: None,
                limit: 5,
            })
            .unwrap();
        assert_eq!(hits.len(), 5, "an explicit below-cap limit is honored");

        // A cap-exceeding limit is clamped, and a usize::MAX limit (the raw
        // cast regression shape) is clamped too — never unbounded.
        for limit in [SEARCH_LIMIT_CAP, SEARCH_LIMIT_CAP + 1, usize::MAX] {
            let hits = store
                .search(&SearchQuery {
                    text: None,
                    exts: Vec::new(),
                    mtime_after: None,
                    mtime_before: None,
                    id_before: None,
                    min_size: None,
                    max_size: None,
                    limit,
                })
                .unwrap();
            assert_eq!(
                hits.len(),
                SEARCH_LIMIT_CAP,
                "limit {limit} must clamp to SEARCH_LIMIT_CAP ({SEARCH_LIMIT_CAP}), \
                 never wrap to unbounded"
            );
        }
    }

    /// delete_tree must degrade to an exact delete for a file record —
    /// deleting only itself, touching no path that merely shares a prefix.
    #[test]
    fn delete_tree_on_a_file_is_exact() {
        let s = Store::open_in_memory().unwrap();
        let f = sep_path(&["a", "b", "notes.md"]);
        let f_bak = sep_path(&["a", "b", "notes.md.bak"]);
        let f2 = sep_path(&["a", "b", "notes.md2"]);
        s.upsert_many(&[
            rec(&f, "notes.md", Some("md"), 10, 1),
            // Sibling paths sharing only prefix characters must not be
            // wrongly deleted.
            rec(&f_bak, "notes.md.bak", Some("bak"), 10, 1),
            rec(&f2, "notes.md2", None, 10, 1),
        ])
        .unwrap();

        s.delete_tree(std::slice::from_ref(&f)).unwrap();

        let left = s.load_index().unwrap();
        assert!(!left.contains_key(&f));
        assert!(left.contains_key(&f_bak));
        assert!(left.contains_key(&f2));
    }

    /// A directory record takes its subtree: a dir rename/move-out only
    /// surfaces the parent-path event, so the prefix delete must cover the
    /// children; the prefix compare aligns exactly on "path + separator",
    /// siblings sharing only prefix characters and parents are unaffected,
    /// and CJK paths compare just as exactly (UTF-8 byte order).
    #[test]
    fn delete_tree_on_a_dir_removes_the_subtree_only() {
        let s = Store::open_in_memory().unwrap();
        let dir_p = sep_path(&["home", "u", "项目"]);
        let dir_en_p = sep_path(&["home", "u", "proj"]);
        let child_cjk = sep_path(&["home", "u", "项目", "报告.pdf"]);
        let child_sub = sep_path(&["home", "u", "项目", "sub", "a.md"]);
        let child_src = sep_path(&["home", "u", "proj", "src", "notes.md"]);
        // Sibling sharing only prefix characters with the deleted dir
        // (`项目` vs `项目备份`).
        let sib_cjk = sep_path(&["home", "u", "项目备份.md"]);
        let sib_en = sep_path(&["home", "u", "proj-notes.md"]);
        let mut dir = rec(&dir_p, "项目", None, 0, 1);
        dir.is_dir = true;
        let mut dir_en = rec(&dir_en_p, "proj", None, 0, 1);
        dir_en.is_dir = true;
        s.upsert_many(&[
            dir,
            dir_en,
            rec(&child_cjk, "报告.pdf", Some("pdf"), 8, 2),
            rec(&child_sub, "a.md", Some("md"), 3, 3),
            rec(&child_src, "notes.md", Some("md"), 4, 4),
            rec(&sib_cjk, "项目备份.md", Some("md"), 5, 5),
            rec(&sib_en, "proj-notes.md", Some("md"), 6, 6),
        ])
        .unwrap();

        s.delete_tree(&[
            dir_p.clone(),
            dir_en_p.clone(),
            // A file path mixed into the same batch must stay an exact
            // delete.
            sib_en.clone(),
        ])
        .unwrap();

        let left = s.load_index().unwrap();
        assert!(!left.contains_key(&dir_p));
        assert!(!left.contains_key(&child_cjk));
        assert!(!left.contains_key(&child_sub));
        assert!(!left.contains_key(&dir_en_p));
        assert!(!left.contains_key(&child_src));
        assert!(!left.contains_key(&sib_en));
        assert!(
            left.contains_key(&sib_cjk),
            "deleting 项目 must not touch the 项目备份.md sibling"
        );
    }

    /// Paths not in the store (never indexed / already deleted by an earlier
    /// path) must pass quietly without error — the watcher's delete branch
    /// fires a delete for every NotFound event; when real paths are mixed
    /// into the batch, unknown ones must not block the rest.
    #[test]
    fn delete_tree_tolerates_unknown_paths() {
        let s = Store::open_in_memory().unwrap();
        let real = sep_path(&["real", "notes.md"]);
        s.upsert_many(&[rec(&real, "notes.md", Some("md"), 1, 1)])
            .unwrap();
        s.delete_tree(&[
            sep_path(&["never", "indexed.md"]),
            sep_path(&["also", "missing"]),
            real.clone(),
        ])
        .unwrap();
        assert!(s.load_index().unwrap().is_empty());
    }

    /// A scan root may be stored under a trailing-separator spelling
    /// (walkdir records scan roots verbatim; passing `--root /media/usb/`
    /// stores `/media/usb/`). It must still be hit when the delete input
    /// uses the canonical spelling: the is_dir probe falls back to the
    /// "p + separator" root-row spelling and the prefix pattern hits that
    /// row exactly, so the whole tree is still deleted — no silent miss.
    #[test]
    fn delete_tree_matches_a_root_row_stored_with_a_trailing_separator() {
        let s = Store::open_in_memory().unwrap();
        let dir_p = sep_path(&["t", "sub"]);
        let root_row = format!("{dir_p}{}", std::path::MAIN_SEPARATOR);
        let child = sep_path(&["t", "sub", "a.md"]);
        let mut dir = rec(&root_row, "sub", None, 0, 1);
        dir.is_dir = true;
        s.upsert_many(&[dir, rec(&child, "a.md", Some("md"), 3, 2)])
            .unwrap();

        s.delete_tree(&[dir_p]).unwrap();

        assert!(
            s.load_index().unwrap().is_empty(),
            "a root row stored with a trailing separator must still be swept"
        );
    }

    /// FTS stays in sync with name changes and decoupled from content-only
    /// changes: the files_au trigger carries WHEN (an UPSERT leaving the name
    /// unchanged no longer rewrites trigrams), a name change must still
    /// propagate to search, and a pure size/mtime change must not disturb
    /// the FTS row. Scope note: these assertions pin FTS **correctness**,
    /// which holds under both trigger shapes — under the old no-WHEN shape a
    /// content-only upsert rewrites trigrams with the identical name (a net
    /// no-op), so search results cannot distinguish WHEN's presence; the
    /// WHEN clause is performance-only and pinned textually by
    /// `trigger_replacement_matches_the_schema_built_shape`. Paths
    /// deliberately contain no search term (the LIKE
    /// fallback matches path too, which would mask FTS's real state);
    /// assertions use >= 3 chars to hit the FTS MATCH branch.
    #[test]
    fn fts_follows_name_changes_and_ignores_content_only_updates() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_many(&[rec("/t/doc-1.pdf", "报告书.pdf", Some("pdf"), 1, 1)])
            .unwrap();
        let hits = |text: &str| {
            s.search(&SearchQuery {
                text: Some(text.into()), // 3 chars → FTS5 trigram MATCH
                limit: 10,
                ..Default::default()
            })
            .unwrap()
            .len()
        };
        assert_eq!(hits("报告书"), 1);

        // Only size/mtime change (name unchanged): the FTS row is untouched,
        // search still hits.
        s.upsert_many(&[rec("/t/doc-1.pdf", "报告书.pdf", Some("pdf"), 99, 2)])
            .unwrap();
        assert_eq!(
            hits("报告书"),
            1,
            "content-only upsert must not disturb FTS"
        );

        // Name change: the old name leaves, the new name enters.
        s.upsert_many(&[rec("/t/doc-1.pdf", "会议纪要.pdf", Some("pdf"), 99, 3)])
            .unwrap();
        assert_eq!(hits("报告书"), 0, "old name must leave the FTS index");
        assert_eq!(hits("会议纪要"), 1, "new name must enter the FTS index");
    }

    /// Trailing separators must be normalized before comparing: the wiring
    /// side may pass a root PathBuf carrying `/`; splicing `dir` + `/`
    /// directly would build `dir//` and silently miss the whole subtree
    /// (ghosts until rescan).
    #[test]
    fn delete_tree_normalizes_a_trailing_separator() {
        let s = Store::open_in_memory().unwrap();
        let dir_p = sep_path(&["t", "sub"]);
        let child = sep_path(&["t", "sub", "a.md"]);
        let mut dir = rec(&dir_p, "sub", None, 0, 1);
        dir.is_dir = true;
        s.upsert_many(&[dir, rec(&child, "a.md", Some("md"), 3, 2)])
            .unwrap();

        // Input carries a trailing separator (the wiring side may pass a
        // root PathBuf carrying `/`).
        s.delete_tree(&[format!("{dir_p}{}", std::path::MAIN_SEPARATOR)])
            .unwrap();

        assert!(
            s.load_index().unwrap().is_empty(),
            "a trailing-separator path must still remove the whole subtree"
        );
    }

    /// The prefix match must be literal: if `%`/`_`/`[` inside the deleted
    /// directory's name were interpreted as LIKE/GLOB pattern semantics,
    /// siblings sharing only prefix characters would be wrongly deleted. The
    /// current range form is immune to all three (exact byte compare, no
    /// pattern semantics).
    #[test]
    fn delete_tree_is_literal_about_pattern_metachars() {
        let s = Store::open_in_memory().unwrap();
        let dir_p = sep_path(&["home", "u", "met_100%[1]"]);
        let child = sep_path(&["home", "u", "met_100%[1]", "报告.pdf"]);
        // Sibling literally containing the same prefix characters: a
        // non-pattern compare must not touch it.
        let sib = sep_path(&["home", "u", "met_100%[1]_备份.md"]);
        // A name that an unescaped LIKE ('met_100%[1]%') would
        // false-positive on: must survive.
        let other = sep_path(&["home", "u", "metX100Yfoo1Z.md"]);
        let mut dir = rec(&dir_p, "met_100%[1]", None, 0, 1);
        dir.is_dir = true;
        s.upsert_many(&[
            dir,
            rec(&child, "报告.pdf", Some("pdf"), 8, 2),
            rec(&sib, "met_100%[1]_备份.md", Some("md"), 5, 3),
            rec(&other, "metX100Yfoo1Z.md", Some("md"), 6, 4),
        ])
        .unwrap();

        s.delete_tree(std::slice::from_ref(&dir_p)).unwrap();

        let left = s.load_index().unwrap();
        assert!(!left.contains_key(&dir_p));
        assert!(!left.contains_key(&child));
        assert!(
            left.contains_key(&sib),
            "literal-prefix sibling must survive"
        );
        assert!(
            left.contains_key(&other),
            "a name that unescaped-LIKE would false-positive must survive"
        );
    }

    /// Both bounds of the prefix range must be exact: the sibling exactly at
    /// the upper bound (`sub0` = `p + separator successor`) and the sibling
    /// below the lower bound (`sub-…`, byte order before the separator) must
    /// survive — widening the half-open interval by one byte-order neighbor
    /// is a wrong delete.
    #[test]
    fn delete_tree_range_bounds_exclude_boundary_siblings() {
        let s = Store::open_in_memory().unwrap();
        let sep = std::path::MAIN_SEPARATOR;
        // Upper bound = p + separator successor character, computed per
        // platform (unix '/'(0x2F)→'0'(0x30), Windows '\'(0x5C)→']'(0x5D))
        // — the same fixture pins the exact upper bound on any platform.
        let succ = (sep as u8 + 1) as char;
        let dir_p = sep_path(&["t", "sub"]);
        let child = sep_path(&["t", "sub", "a.md"]);
        // A spelling exactly at the upper-bound character: must survive.
        let at_upper = format!("{dir_p}{succ}");
        // A near neighbor preceding the separator's byte order ('-' < sep;
        // both separators are ASCII): must survive.
        let below = sep_path(&["t", "sub-文件.md"]);
        let mut dir = rec(&dir_p, "sub", None, 0, 1);
        dir.is_dir = true;
        s.upsert_many(&[
            dir,
            rec(&child, "a.md", Some("md"), 3, 2),
            rec(&at_upper, &format!("sub{succ}"), None, 0, 3),
            rec(&below, "sub-文件.md", Some("md"), 4, 4),
        ])
        .unwrap();

        s.delete_tree(std::slice::from_ref(&dir_p)).unwrap();

        let left = s.load_index().unwrap();
        assert!(!left.contains_key(&child));
        assert!(
            left.contains_key(&at_upper),
            "the exact upper-bound sibling must survive"
        );
        assert!(
            left.contains_key(&below),
            "a below-bound sibling must survive"
        );
    }

    /// Write-amplification elision on the watcher's incremental path: a
    /// repeat upsert with identical content skips the whole row (indexed_at
    /// keeps the time of the last actual change — asserted with a sentinel
    /// instead of the real clock, deterministically), and any content-column
    /// change rewrites as usual. This is the WHERE pin of
    /// [`UPSERT_IF_CHANGED_SQL`]: the WHERE-removal mutation lets
    /// strftime('now') overwrite the sentinel and goes red.
    #[test]
    fn watcher_upsert_skips_a_content_identical_row() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_many_if_changed(&[rec("/t/f.md", "f.md", Some("md"), 10, 100)])
            .unwrap();
        // Sentinel: set indexed_at to a definite past value.
        s.conn
            .lock()
            .execute("UPDATE files SET indexed_at = 1234567890", [])
            .unwrap();
        s.upsert_many_if_changed(&[rec("/t/f.md", "f.md", Some("md"), 10, 100)])
            .unwrap();
        let at: i64 = s
            .read
            .lock()
            .query_row(
                "SELECT indexed_at FROM files WHERE path = '/t/f.md'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            at, 1234567890,
            "a content-identical re-upsert must skip the row (sentinel must survive)"
        );

        // Any content-column change: rewrite as usual (the sentinel is
        // replaced), row content updated.
        s.upsert_many_if_changed(&[rec("/t/f.md", "f.md", Some("md"), 11, 100)])
            .unwrap();
        let (at, size): (i64, i64) = s
            .read
            .lock()
            .query_row(
                "SELECT indexed_at, size FROM files WHERE path = '/t/f.md'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(at != 1234567890, "a real change must rewrite the row");
        assert_eq!(size, 11);
    }

    /// A delete input that names a whole filesystem root must be skipped,
    /// not turned into an unbounded range: a bare separator spelling trims
    /// to empty, and a drive root (`C:\`) trims to the drive-root spelling
    /// `C:` — both are guarded (see [`is_drive_root`]). The load-bearing
    /// scenario needs a store root row
    /// spelled as the bare separator (walkdir records a scanned filesystem
    /// root `/` verbatim as an is_dir row): without the empty guard the
    /// with_sep probe fallback resolves the empty path to that row, is_dir
    /// comes back Some(true), and the range `["/", "0")` — on unix every
    /// absolute row — wipes the whole index. The guard's removal is inert
    /// against every other delete_tree fixture (no row answers the probe for
    /// the empty path), which is why this branch went unpinned.
    #[test]
    fn delete_tree_skips_an_input_that_trims_to_empty() {
        let s = Store::open_in_memory().unwrap();
        let root_row = std::path::MAIN_SEPARATOR.to_string();
        let f = sep_path(&["t", "notes.md"]);
        let other = sep_path(&["elsewhere", "doc.md"]);
        let mut root = rec(&root_row, "", None, 0, 1);
        root.is_dir = true;
        s.upsert_many(&[
            root,
            rec(&f, "notes.md", Some("md"), 1, 1),
            rec(&other, "doc.md", Some("md"), 2, 2),
        ])
        .unwrap();

        // The bare-separator input trims to empty and must be ignored (the
        // scanned-root row it names is cleaned by rescan, never by a
        // whole-index subtree delete); the real path in the same batch is
        // still deleted (the guard skips per path, not the whole batch).
        let bare = std::path::MAIN_SEPARATOR.to_string();
        s.delete_tree(&[bare, f.clone()]).unwrap();

        let left = s.load_index().unwrap();
        assert!(
            !left.contains_key(&f),
            "the real path in the batch must still be deleted"
        );
        assert_eq!(
            left.len(),
            2,
            "the separator-only input must not sweep the store (a guardless \
             bare-separator input would wipe the whole index)"
        );
        assert!(left.contains_key(&root_row));
        assert!(left.contains_key(&other));
    }

    /// A drive-root input (`C:\`, the spelling a Windows GUI root actually
    /// produces) is the same whole-index class as the bare separator: it
    /// trims to `C:` — non-empty, so the empty guard alone misses it — and
    /// under the guardless flow the `with_sep` probe resolves the stored
    /// root row to `is_dir`, whose range sweeps every row under the drive.
    /// Fixtures use the platform separator so the sweep is real on both
    /// platforms (unix: rows spelled `C:/…`; windows: `C:\…`).
    #[test]
    fn delete_tree_skips_drive_root_spellings() {
        let s = Store::open_in_memory().unwrap();
        let sep = std::path::MAIN_SEPARATOR;
        let root_row = format!("C:{sep}");
        let child = format!("C:{sep}Users{sep}notes.md");
        let other = sep_path(&["elsewhere", "doc.md"]);
        let mut root = rec(&root_row, "", None, 0, 1);
        root.is_dir = true;
        s.upsert_many(&[
            root,
            rec(&child, "notes.md", Some("md"), 1, 1),
            rec(&other, "doc.md", Some("md"), 2, 2),
        ])
        .unwrap();

        // The drive-root input must be skipped (rescan owns the rows); the
        // unrelated path in the same batch proves the skip is per-path,
        // not a dead call.
        let related = sep_path(&["elsewhere", "doc2.md"]);
        s.upsert_many(&[rec(&related, "doc2.md", Some("md"), 3, 3)])
            .unwrap();
        s.delete_tree(&[root_row.clone(), related.clone()]).unwrap();

        let left = s.load_index().unwrap();
        assert!(
            left.contains_key(&root_row),
            "the drive-root row must survive"
        );
        assert!(
            left.contains_key(&child),
            "the drive subtree must not be swept by a drive-root input"
        );
        assert!(
            !left.contains_key(&related),
            "the unrelated path in the same batch must still be deleted"
        );
        assert!(left.contains_key(&other));
    }

    /// Verbatim drive-root spellings must be skipped like the plain ones:
    /// the canonical-spelling contract stores rows under the `\\?\C:\…`
    /// prefix, so a transient NotFound for the root arrives spelled
    /// `\\?\C:\` (or trims to `\\?\C:` on Windows, where `\` is
    /// MAIN_SEPARATOR; on Unix the backslash survives the trim and is
    /// handled inside `is_drive_root`) — the same whole-index class the
    /// plain guard closes.
    #[test]
    fn delete_tree_skips_verbatim_drive_root_spellings() {
        let s = Store::open_in_memory().unwrap();
        let root_row = r"\\?\C:\";
        let child = r"\\?\C:\Users\notes.md";
        let other = sep_path(&["elsewhere", "doc.md"]);
        let mut root = rec(root_row, "", None, 0, 1);
        root.is_dir = true;
        s.upsert_many(&[
            root,
            rec(child, "notes.md", Some("md"), 1, 1),
            rec(&other, "doc.md", Some("md"), 2, 2),
        ])
        .unwrap();

        // The verbatim inputs must both be skipped (rescan owns the rows);
        // the unrelated path in the same batch proves the skip is per-path,
        // not a dead call.
        let related = sep_path(&["elsewhere", "doc2.md"]);
        s.upsert_many(&[rec(&related, "doc2.md", Some("md"), 3, 3)])
            .unwrap();
        s.delete_tree(&[root_row.to_string(), r"\\?\C:".to_string(), related.clone()])
            .unwrap();

        let left = s.load_index().unwrap();
        assert!(
            left.contains_key(root_row),
            "the verbatim root row must survive"
        );
        assert!(
            left.contains_key(child),
            "the verbatim drive subtree must not be swept"
        );
        assert!(
            !left.contains_key(&related),
            "the unrelated path in the same batch must still be deleted"
        );
        assert!(left.contains_key(&other));
    }

    /// `handle_event`'s delete-before-write order is load-bearing: if one
    /// event ever carried both a directory delete and a new file under it,
    /// the wrong order would let the prefix range delete eat the freshly
    /// upserted child row (see the write-phase comment in watcher.rs's
    /// `handle_event`). The contradictory shape (parent gone, child alive
    /// under it) cannot exist on a consistent filesystem at one instant, so
    /// the mechanism is pinned here at the store level, in both orders.
    #[test]
    fn delete_first_ordering_preserves_a_child_upserted_under_the_deleted_dir() {
        let s = Store::open_in_memory().unwrap();
        let dir = sep_path(&["t", "d"]);
        let child = sep_path(&["t", "d", "new.md"]);
        // The real store state before a rename-away event: the directory's
        // own row is indexed (is_dir=true), which is what arms the subtree
        // sweep when the delete event arrives.
        let mut dir_row = rec(&dir, "d", None, 0, 1);
        dir_row.is_dir = true;

        // The wrong order (upsert, then the prefix delete): the child row is
        // eaten — proving the danger the ordering guards against is real.
        s.upsert_many(&[dir_row.clone()]).unwrap();
        s.upsert_many_if_changed(&[rec(&child, "new.md", Some("md"), 1, 1)])
            .unwrap();
        s.delete_tree(std::slice::from_ref(&dir)).unwrap();
        assert!(
            !s.load_index().unwrap().contains_key(&child),
            "upsert-first must lose the child row to the range delete"
        );

        // The right order (handle_event's): the delete runs first, so the
        // freshly upserted child row survives.
        s.upsert_many(&[dir_row]).unwrap();
        s.delete_tree(std::slice::from_ref(&dir)).unwrap();
        s.upsert_many_if_changed(&[rec(&child, "new.md", Some("md"), 1, 1)])
            .unwrap();
        assert!(
            s.load_index().unwrap().contains_key(&child),
            "delete-first must let the new child row survive"
        );
    }

    /// The delete-side FTS trigger (`files_ad`) must keep parity when a
    /// rowid is reused: delete a row, recreate the same path under a new
    /// name — SQLite hands the INSERT the freed rowid, so a broken delete
    /// side leaves the old name's ghost in FTS and `search` returns the
    /// deleted file's name as a hit for a different file. The insert/update
    /// triggers alone cannot expose this (the rowid JOIN hides a plain
    /// ghost row), which is why the other FTS tests stay green under that
    /// regression.
    #[test]
    fn fts_delete_side_does_not_ghost_a_reused_rowid() {
        let s = Store::open_in_memory().unwrap();
        let p = sep_path(&["t", "doc-7.pdf"]);
        s.upsert_many(&[rec(&p, "quarterly-report.pdf", Some("pdf"), 1, 1)])
            .unwrap();
        let hits = |text: &str| {
            s.search(&SearchQuery {
                text: Some(text.into()), // 3 chars → FTS5 trigram MATCH
                limit: 10,
                ..Default::default()
            })
            .unwrap()
            .len()
        };
        assert_eq!(hits("quarterly"), 1);

        s.delete_tree(std::slice::from_ref(&p)).unwrap();
        s.upsert_many(&[rec(&p, "annual-summary.pdf", Some("pdf"), 2, 2)])
            .unwrap();

        assert_eq!(
            hits("quarterly"),
            0,
            "the deleted name must leave FTS even when its rowid is reused"
        );
        assert_eq!(hits("annual"), 1, "the recreated name must be searchable");
    }
}
