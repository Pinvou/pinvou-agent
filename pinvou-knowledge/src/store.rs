use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use rusqlite::types::Type;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

use crate::embedding::{blob_to_vec, vec_to_blob};
use crate::model::{
    AccessScope, Collection, DeviceGrant, Document, JoinRequestRecord, JoinRequestStatus,
    SearchHit, ShareRecord, SourceChunk, TrashedDocument,
};

const SCHEMA_VERSION: i64 = 3;
const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const VECTOR_SIGNATURE_BITS: u32 = 16;
const VECTOR_SIGNATURE_RADIUS: u32 = 3;
// Exact search keeps recall deterministic for the small and medium knowledge
// bases used by a single team. Above this boundary the signature index bounds
// the amount of vector data read by one query.
const VECTOR_EXACT_SCAN_THRESHOLD: i64 = 20_000;

#[derive(Debug)]
pub enum DeviceMutationError {
    NotFound,
    OwnerProtected,
    HostOwnerProtected,
    Revoked,
    Database(rusqlite::Error),
}

impl std::fmt::Display for DeviceMutationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("device not found"),
            Self::OwnerProtected => {
                formatter.write_str("owner devices require local host management")
            }
            Self::HostOwnerProtected => {
                formatter.write_str("the host owner device cannot be changed")
            }
            Self::Revoked => formatter.write_str("the device is revoked"),
            Self::Database(error) => error.fmt(formatter),
        }
    }
}

impl From<rusqlite::Error> for DeviceMutationError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}
pub(crate) const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS collections (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'ready',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_remote_collections_deleted
    ON collections(deleted_at DESC, id DESC) WHERE deleted_at IS NOT NULL;

CREATE TABLE IF NOT EXISTS documents (
    id INTEGER PRIMARY KEY,
    collection_id INTEGER NOT NULL REFERENCES collections(id),
    name TEXT NOT NULL,
    ext TEXT,
    storage_path TEXT NOT NULL,
    size INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    n_chunks INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER,
    error TEXT
);
CREATE INDEX IF NOT EXISTS idx_remote_documents_collection
    ON documents(collection_id, deleted_at, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_remote_documents_active_sha256
    ON documents(collection_id, sha256) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_remote_documents_deleted
    ON documents(deleted_at DESC, id DESC) WHERE deleted_at IS NOT NULL;

CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY,
    document_id INTEGER NOT NULL REFERENCES documents(id),
    collection_id INTEGER NOT NULL REFERENCES collections(id),
    ord INTEGER NOT NULL,
    text TEXT NOT NULL,
    vec BLOB NOT NULL,
    vec_sig INTEGER,
    UNIQUE(document_id, ord)
);
CREATE INDEX IF NOT EXISTS idx_remote_chunks_collection ON chunks(collection_id);
CREATE INDEX IF NOT EXISTS idx_remote_chunks_document ON chunks(document_id, ord);

CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
    text,
    content='chunks', content_rowid='id',
    tokenize='trigram'
);
CREATE TRIGGER IF NOT EXISTS remote_chunks_ai AFTER INSERT ON chunks BEGIN
    INSERT INTO chunks_fts(rowid, text) VALUES(new.id, new.text);
END;
CREATE TRIGGER IF NOT EXISTS remote_chunks_ad AFTER DELETE ON chunks BEGIN
    INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES('delete', old.id, old.text);
END;

CREATE TABLE IF NOT EXISTS invites (
    id TEXT PRIMARY KEY,
    secret_hash TEXT NOT NULL UNIQUE,
    scope TEXT NOT NULL,
    label TEXT,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    consumed_at INTEGER
);

CREATE TABLE IF NOT EXISTS devices (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    scope TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    last_seen_at INTEGER,
    revoked INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_remote_devices_created ON devices(created_at DESC, id DESC);

CREATE TABLE IF NOT EXISTS shares (
    id TEXT PRIMARY KEY,
    secret_hash TEXT NOT NULL UNIQUE,
    endpoints_json TEXT NOT NULL,
    auto_approve_read INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    stopped_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_remote_shares_active
    ON shares(expires_at DESC, id DESC) WHERE stopped_at IS NULL;

CREATE TABLE IF NOT EXISTS join_requests (
    id TEXT PRIMARY KEY,
    claim_hash TEXT NOT NULL UNIQUE,
    device_name TEXT NOT NULL,
    device_id TEXT NOT NULL UNIQUE,
    token_hash TEXT NOT NULL UNIQUE,
    share_id TEXT REFERENCES shares(id),
    status TEXT NOT NULL,
    scope TEXT,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    resolved_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_remote_join_requests_status
    ON join_requests(status, created_at DESC, id DESC);
"#;

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
    read_path: Option<Arc<PathBuf>>,
}

#[derive(Debug, Clone)]
pub struct StoredDocument {
    pub document: Document,
    pub storage_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreDocumentOutcome {
    Restored,
    DuplicateActive { active_document_id: i64 },
}

pub(crate) struct DocumentIndexUpdate<'a> {
    pub name: &'a str,
    pub ext: Option<&'a str>,
    pub storage_path: &'a str,
    pub size: i64,
    pub sha256: &'a str,
    pub chunks: &'a [String],
    pub vectors: &'a [Vec<f32>],
}

struct NewDocument<'a> {
    collection_id: i64,
    name: &'a str,
    ext: Option<&'a str>,
    storage_path: &'a str,
    size: i64,
    sha256: &'a str,
}

#[derive(Debug, Clone, Copy)]
struct VectorCandidate {
    chunk_id: i64,
    score: f32,
}

impl PartialEq for VectorCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.chunk_id == other.chunk_id && self.score.to_bits() == other.score.to_bits()
    }
}

impl Eq for VectorCandidate {}

impl PartialOrd for VectorCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for VectorCandidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.score
            .total_cmp(&other.score)
            // Lower row ids win deterministic ties, so higher ids are the first
            // candidates evicted from the bounded heap.
            .then_with(|| other.chunk_id.cmp(&self.chunk_id))
    }
}

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        }
        let connection = Connection::open(path)?;
        configure_connection(&connection)?;
        connection.execute_batch(SCHEMA)?;
        ensure_vector_signature_schema(&connection)?;
        connection.execute_batch(&format!("PRAGMA user_version={SCHEMA_VERSION};"))?;
        Ok(Self {
            conn: Arc::new(Mutex::new(connection)),
            read_path: Some(Arc::new(path.to_path_buf())),
        })
    }

    #[cfg(test)]
    pub fn in_memory() -> rusqlite::Result<Self> {
        let connection = Connection::open_in_memory()?;
        configure_connection(&connection)?;
        connection.execute_batch(SCHEMA)?;
        ensure_vector_signature_schema(&connection)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(connection)),
            read_path: None,
        })
    }

    fn with_read_connection<T>(
        &self,
        query: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        if let Some(path) = &self.read_path {
            let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX;
            let mut connection = Connection::open_with_flags(path.as_ref(), flags)?;
            configure_connection(&connection)?;
            return query(&mut connection);
        }

        // An in-memory database cannot be reopened without creating a different
        // database. These stores are test-only, so use the primary connection.
        let mut connection = self.conn.lock();
        query(&mut connection)
    }

    pub fn meta(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.with_read_connection(|connection| {
            connection
                .query_row("SELECT value FROM meta WHERE key=?1", params![key], |row| {
                    row.get(0)
                })
                .optional()
        })
    }

    pub fn set_meta(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.lock().execute(
            "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn delete_meta(&self, key: &str) -> rusqlite::Result<()> {
        self.conn
            .lock()
            .execute("DELETE FROM meta WHERE key=?1", params![key])?;
        Ok(())
    }

    pub fn list_collections(&self, include_deleted: bool) -> rusqlite::Result<Vec<Collection>> {
        let filter = if include_deleted {
            ""
        } else {
            "WHERE c.deleted_at IS NULL"
        };
        let sql = format!(
            "SELECT c.id,c.name,c.description,c.status,c.created_at,c.updated_at,c.deleted_at,\
             (SELECT COUNT(*) FROM documents d WHERE d.collection_id=c.id AND d.deleted_at IS NULL),\
             (SELECT COUNT(*) FROM chunks k JOIN documents d ON d.id=k.document_id WHERE k.collection_id=c.id AND d.deleted_at IS NULL),\
              COALESCE((SELECT SUM(d.size) FROM documents d WHERE d.collection_id=c.id AND d.deleted_at IS NULL),0) \
             FROM collections c {filter} ORDER BY c.updated_at DESC,c.id DESC"
        );
        self.with_read_connection(|connection| {
            let mut statement = connection.prepare(&sql)?;
            let rows = statement.query_map([], map_collection)?;
            rows.collect()
        })
    }

    pub fn list_trashed_collections_page(
        &self,
        limit: usize,
        offset: usize,
    ) -> rusqlite::Result<Vec<Collection>> {
        let limit = i64::try_from(limit.clamp(1, 500)).unwrap_or(500);
        let offset = i64::try_from(offset).unwrap_or(i64::MAX);
        self.with_read_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT c.id,c.name,c.description,c.status,c.created_at,c.updated_at,c.deleted_at,\
                 (SELECT COUNT(*) FROM documents d WHERE d.collection_id=c.id),\
                 (SELECT COUNT(*) FROM chunks k WHERE k.collection_id=c.id),\
                 COALESCE((SELECT SUM(d.size) FROM documents d WHERE d.collection_id=c.id),0) \
                 FROM collections c WHERE c.deleted_at IS NOT NULL \
                 ORDER BY c.deleted_at DESC,c.id DESC LIMIT ?1 OFFSET ?2",
            )?;
            let rows = statement.query_map(params![limit, offset], map_collection)?;
            rows.collect()
        })
    }

    pub fn create_collection(
        &self,
        name: &str,
        description: Option<&str>,
    ) -> rusqlite::Result<Collection> {
        let now = now();
        let connection = self.conn.lock();
        connection.execute(
            "INSERT INTO collections(name,description,status,created_at,updated_at) VALUES(?1,?2,'ready',?3,?3)",
            params![name, description, now],
        )?;
        let id = connection.last_insert_rowid();
        drop(connection);
        self.collection(id, true)?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn collection(
        &self,
        id: i64,
        include_deleted: bool,
    ) -> rusqlite::Result<Option<Collection>> {
        let deleted = if include_deleted {
            ""
        } else {
            "AND c.deleted_at IS NULL"
        };
        let sql = format!(
            "SELECT c.id,c.name,c.description,c.status,c.created_at,c.updated_at,c.deleted_at,\
             (SELECT COUNT(*) FROM documents d WHERE d.collection_id=c.id AND d.deleted_at IS NULL),\
             (SELECT COUNT(*) FROM chunks k JOIN documents d ON d.id=k.document_id WHERE k.collection_id=c.id AND d.deleted_at IS NULL),\
              COALESCE((SELECT SUM(d.size) FROM documents d WHERE d.collection_id=c.id AND d.deleted_at IS NULL),0) \
             FROM collections c WHERE c.id=?1 {deleted}"
        );
        self.with_read_connection(|connection| {
            connection
                .query_row(&sql, params![id], map_collection)
                .optional()
        })
    }

    pub fn update_collection(
        &self,
        id: i64,
        name: &str,
        description: Option<&str>,
    ) -> rusqlite::Result<()> {
        let changed = self.conn.lock().execute(
            "UPDATE collections SET name=?2,description=?3,updated_at=?4 WHERE id=?1 AND deleted_at IS NULL",
            params![id, name, description, now()],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    pub fn trash_collection(&self, id: i64) -> rusqlite::Result<()> {
        let mut connection = self.conn.lock();
        let transaction = connection.transaction()?;
        // The collection deletion timestamp doubles as the cascade marker for
        // documents that were active at the time. Keep it distinct from any
        // pre-existing document deletion timestamp so restore_collection can
        // leave documents that were already in the recycle bin untouched.
        let mut timestamp = now();
        while transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM documents WHERE collection_id=?1 AND deleted_at=?2)",
            params![id, timestamp],
            |row| row.get::<_, bool>(0),
        )? {
            timestamp += 1;
        }
        let changed = transaction.execute(
            "UPDATE collections SET deleted_at=?2,updated_at=?2 WHERE id=?1 AND deleted_at IS NULL",
            params![id, timestamp],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        transaction.execute(
            "UPDATE documents SET deleted_at=?2,updated_at=?2 WHERE collection_id=?1 AND deleted_at IS NULL",
            params![id, timestamp],
        )?;
        transaction.commit()
    }

    pub fn restore_collection(&self, id: i64) -> rusqlite::Result<()> {
        let mut connection = self.conn.lock();
        let transaction = connection.transaction()?;
        let collection_deleted_at = transaction
            .query_row(
                "SELECT deleted_at FROM collections WHERE id=?1 AND deleted_at IS NOT NULL",
                params![id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        let restored_at = now();
        let changed = transaction.execute(
            "UPDATE collections SET deleted_at=NULL,updated_at=?2 WHERE id=?1 AND deleted_at IS NOT NULL",
            params![id, restored_at],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        transaction.execute(
            "UPDATE documents SET deleted_at=NULL,updated_at=?2 WHERE collection_id=?1 AND deleted_at=?3",
            params![id, restored_at, collection_deleted_at],
        )?;
        transaction.commit()
    }

    pub fn set_collection_status(&self, id: i64, status: &str) -> rusqlite::Result<()> {
        self.conn.lock().execute(
            "UPDATE collections SET status=?2,updated_at=?3 WHERE id=?1",
            params![id, status, now()],
        )?;
        Ok(())
    }

    pub fn insert_document(
        &self,
        collection_id: i64,
        name: &str,
        ext: Option<&str>,
        storage_path: &str,
        size: i64,
        sha256: &str,
    ) -> rusqlite::Result<Document> {
        self.insert_document_with_deduplication(
            NewDocument {
                collection_id,
                name,
                ext,
                storage_path,
                size,
                sha256,
            },
            false,
        )
        .map(|(document, _)| document)
    }

    pub fn insert_document_if_new(
        &self,
        collection_id: i64,
        name: &str,
        ext: Option<&str>,
        storage_path: &str,
        size: i64,
        sha256: &str,
    ) -> rusqlite::Result<(Document, bool)> {
        self.insert_document_with_deduplication(
            NewDocument {
                collection_id,
                name,
                ext,
                storage_path,
                size,
                sha256,
            },
            true,
        )
    }

    fn insert_document_with_deduplication(
        &self,
        input: NewDocument<'_>,
        deduplicate: bool,
    ) -> rusqlite::Result<(Document, bool)> {
        let now = now();
        let connection = self.conn.lock();
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM collections WHERE id=?1 AND deleted_at IS NULL)",
            params![input.collection_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        if deduplicate {
            let existing_id = connection
                .query_row(
                    "SELECT id FROM documents WHERE collection_id=?1 AND sha256=?2 \
                     AND deleted_at IS NULL ORDER BY id LIMIT 1",
                    params![input.collection_id, input.sha256],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(id) = existing_id {
                drop(connection);
                let document = self
                    .document(id, false)?
                    .map(|stored| stored.document)
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                return Ok((document, false));
            }
        }
        connection.execute(
            "INSERT INTO documents(collection_id,name,ext,storage_path,size,sha256,status,created_at,updated_at) \
             VALUES(?1,?2,?3,?4,?5,?6,'pending',?7,?7)",
            params![
                input.collection_id,
                input.name,
                input.ext,
                input.storage_path,
                input.size,
                input.sha256,
                now
            ],
        )?;
        let id = connection.last_insert_rowid();
        drop(connection);
        let document = self
            .document(id, true)?
            .map(|stored| stored.document)
            .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        Ok((document, true))
    }

    pub fn list_documents(
        &self,
        collection_id: i64,
        include_deleted: bool,
    ) -> rusqlite::Result<Vec<Document>> {
        self.list_documents_page(collection_id, include_deleted, None, 0)
    }

    pub fn list_documents_page(
        &self,
        collection_id: i64,
        include_deleted: bool,
        limit: Option<usize>,
        offset: usize,
    ) -> rusqlite::Result<Vec<Document>> {
        let filter = if include_deleted {
            ""
        } else {
            "AND deleted_at IS NULL"
        };
        let sql = format!(
            "SELECT id,collection_id,name,ext,size,sha256,status,n_chunks,created_at,updated_at,deleted_at,error \
             FROM documents WHERE collection_id=?1 {filter} ORDER BY created_at DESC,id DESC \
             LIMIT ?2 OFFSET ?3"
        );
        let limit = limit
            .map(|value| i64::try_from(value.clamp(1, 500)).unwrap_or(500))
            .unwrap_or(-1);
        let offset = i64::try_from(offset).unwrap_or(i64::MAX);
        self.with_read_connection(|connection| {
            let mut statement = connection.prepare(&sql)?;
            let rows = statement.query_map(params![collection_id, limit, offset], map_document)?;
            rows.collect()
        })
    }

    pub fn list_trashed_documents_page(
        &self,
        limit: usize,
        offset: usize,
    ) -> rusqlite::Result<Vec<TrashedDocument>> {
        let limit = i64::try_from(limit.clamp(1, 500)).unwrap_or(500);
        let offset = i64::try_from(offset).unwrap_or(i64::MAX);
        self.with_read_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT d.id,d.collection_id,d.name,d.ext,d.size,d.sha256,d.status,d.n_chunks,\
                 d.created_at,d.updated_at,d.deleted_at,d.error,c.name \
                 FROM documents d JOIN collections c ON c.id=d.collection_id \
                 WHERE d.deleted_at IS NOT NULL AND c.deleted_at IS NULL \
                 ORDER BY d.deleted_at DESC,d.id DESC LIMIT ?1 OFFSET ?2",
            )?;
            let rows = statement.query_map(params![limit, offset], |row| {
                Ok(TrashedDocument {
                    document: map_document(row)?,
                    collection_name: row.get(12)?,
                })
            })?;
            rows.collect()
        })
    }

    pub fn document(
        &self,
        id: i64,
        include_deleted: bool,
    ) -> rusqlite::Result<Option<StoredDocument>> {
        let filter = if include_deleted {
            ""
        } else {
            "AND deleted_at IS NULL"
        };
        let sql = format!(
            "SELECT id,collection_id,name,ext,size,sha256,status,n_chunks,created_at,updated_at,deleted_at,error,storage_path \
             FROM documents WHERE id=?1 {filter}"
        );
        self.with_read_connection(|connection| {
            connection
                .query_row(&sql, params![id], |row| {
                    Ok(StoredDocument {
                        document: map_document(row)?,
                        storage_path: row.get(12)?,
                    })
                })
                .optional()
        })
    }

    pub fn documents_by_ids(&self, ids: &[i64]) -> rusqlite::Result<Vec<Document>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids = ids
            .iter()
            .copied()
            .filter(|id| *id > 0)
            .take(500)
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let sql = format!(
            "SELECT d.id,d.collection_id,d.name,d.ext,d.size,d.sha256,d.status,d.n_chunks,\
             d.created_at,d.updated_at,d.deleted_at,d.error FROM documents d \
             JOIN collections c ON c.id=d.collection_id WHERE d.id IN ({}) \
             AND d.deleted_at IS NULL AND c.deleted_at IS NULL ORDER BY d.id",
            id_list(&ids)
        );
        self.with_read_connection(|connection| {
            let mut statement = connection.prepare(&sql)?;
            let rows = statement.query_map([], map_document)?;
            rows.collect()
        })
    }

    pub(crate) fn replace_document_index(
        &self,
        id: i64,
        update: DocumentIndexUpdate<'_>,
    ) -> rusqlite::Result<()> {
        if update.chunks.len() != update.vectors.len() || update.chunks.is_empty() {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let mut connection = self.conn.lock();
        let transaction = connection.transaction()?;
        let collection_id: i64 = transaction.query_row(
            "SELECT collection_id FROM documents WHERE id=?1 AND deleted_at IS NULL",
            params![id],
            |row| row.get(0),
        )?;
        transaction.execute("DELETE FROM chunks WHERE document_id=?1", params![id])?;
        {
            let mut statement = transaction.prepare_cached(
                "INSERT INTO chunks(document_id,collection_id,ord,text,vec,vec_sig) \
                 VALUES(?1,?2,?3,?4,?5,?6)",
            )?;
            for (ord, (text, vector)) in update.chunks.iter().zip(update.vectors).enumerate() {
                statement.execute(params![
                    id,
                    collection_id,
                    ord as i64,
                    text,
                    vec_to_blob(vector),
                    vector_signature(vector)
                ])?;
            }
        }
        transaction.execute(
            "UPDATE documents SET name=?2,ext=?3,storage_path=?4,size=?5,sha256=?6,status='ready',\
             n_chunks=?7,updated_at=?8,error=NULL WHERE id=?1",
            params![
                id,
                update.name,
                update.ext,
                update.storage_path,
                update.size,
                update.sha256,
                update.chunks.len() as i64,
                now()
            ],
        )?;
        transaction.execute(
            "UPDATE collections SET status='ready',updated_at=?2 WHERE id=?1",
            params![collection_id, now()],
        )?;
        transaction.commit()
    }

    pub fn backfill_vector_signatures(&self, limit: usize) -> rusqlite::Result<usize> {
        let mut connection = self.conn.lock();
        let transaction = connection.transaction()?;
        let rows = {
            let mut statement = transaction
                .prepare("SELECT id,vec FROM chunks WHERE vec_sig IS NULL ORDER BY id LIMIT ?1")?;
            let rows = statement
                .query_map(params![limit.clamp(1, 2_000) as i64], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        {
            let mut update =
                transaction.prepare_cached("UPDATE chunks SET vec_sig=?2 WHERE id=?1")?;
            for (id, blob) in &rows {
                update.execute(params![id, vector_signature(&blob_to_vec(blob))])?;
            }
        }
        transaction.commit()?;
        Ok(rows.len())
    }

    pub fn mark_document_failed(&self, id: i64, error: &str) -> rusqlite::Result<()> {
        self.conn.lock().execute(
            "UPDATE documents SET status='failed',error=?2,updated_at=?3 WHERE id=?1",
            params![id, error, now()],
        )?;
        Ok(())
    }

    pub fn trash_document(&self, id: i64) -> rusqlite::Result<()> {
        let changed = self.conn.lock().execute(
            "UPDATE documents SET deleted_at=?2,updated_at=?2 WHERE id=?1 AND deleted_at IS NULL",
            params![id, now()],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    pub fn restore_document(&self, id: i64) -> rusqlite::Result<RestoreDocumentOutcome> {
        let mut connection = self.conn.lock();
        let transaction = connection.transaction()?;
        let (collection_id, sha256): (i64, String) = transaction
            .query_row(
                "SELECT d.collection_id,d.sha256 FROM documents d \
                 JOIN collections c ON c.id=d.collection_id \
                 WHERE d.id=?1 AND d.deleted_at IS NOT NULL AND c.deleted_at IS NULL",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        let duplicate_id = transaction
            .query_row(
                "SELECT id FROM documents WHERE collection_id=?1 AND sha256=?2 \
                 AND deleted_at IS NULL AND id<>?3 ORDER BY id LIMIT 1",
                params![collection_id, sha256, id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(active_document_id) = duplicate_id {
            return Ok(RestoreDocumentOutcome::DuplicateActive { active_document_id });
        }
        let changed = transaction.execute(
            "UPDATE documents SET deleted_at=NULL,updated_at=?2 WHERE id=?1 AND deleted_at IS NOT NULL",
            params![id, now()],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        transaction.commit()?;
        Ok(RestoreDocumentOutcome::Restored)
    }

    pub fn permanently_delete_document(&self, id: i64) -> rusqlite::Result<String> {
        let mut connection = self.conn.lock();
        let transaction = connection.transaction()?;
        let (collection_id, storage_path) = transaction
            .query_row(
                "SELECT d.collection_id,d.storage_path FROM documents d \
                 JOIN collections c ON c.id=d.collection_id \
                 WHERE d.id=?1 AND d.deleted_at IS NOT NULL AND c.deleted_at IS NULL",
                params![id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        transaction.execute("DELETE FROM chunks WHERE document_id=?1", params![id])?;
        transaction.execute("DELETE FROM documents WHERE id=?1", params![id])?;
        transaction.execute(
            "UPDATE collections SET updated_at=?2 WHERE id=?1",
            params![collection_id, now()],
        )?;
        transaction.commit()?;
        Ok(storage_path)
    }

    pub fn permanently_delete_collection(&self, id: i64) -> rusqlite::Result<Vec<String>> {
        let mut connection = self.conn.lock();
        let transaction = connection.transaction()?;
        let exists = transaction
            .query_row(
                "SELECT 1 FROM collections WHERE id=?1 AND deleted_at IS NOT NULL",
                params![id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        let paths = {
            let mut statement =
                transaction.prepare("SELECT storage_path FROM documents WHERE collection_id=?1")?;
            let paths = statement
                .query_map(params![id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            paths
        };
        transaction.execute(
            "DELETE FROM chunks WHERE document_id IN (SELECT id FROM documents WHERE collection_id=?1)",
            params![id],
        )?;
        transaction.execute("DELETE FROM documents WHERE collection_id=?1", params![id])?;
        transaction.execute("DELETE FROM collections WHERE id=?1", params![id])?;
        transaction.commit()?;
        Ok(paths)
    }

    /// Permanently remove trash entries older than the retention cutoff and return their
    /// managed source paths so the service can remove bytes after the database transaction.
    pub fn purge_expired_trash(&self, cutoff: i64) -> rusqlite::Result<Vec<String>> {
        let mut connection = self.conn.lock();
        let transaction = connection.transaction()?;
        let paths = {
            let mut statement = transaction.prepare(
                "SELECT d.storage_path FROM documents d JOIN collections c ON c.id=d.collection_id \
                 WHERE (d.deleted_at IS NOT NULL AND d.deleted_at<=?1) \
                    OR (c.deleted_at IS NOT NULL AND c.deleted_at<=?1)",
            )?;
            let paths = statement
                .query_map(params![cutoff], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            paths
        };
        transaction.execute(
            "DELETE FROM chunks WHERE document_id IN (\
             SELECT d.id FROM documents d JOIN collections c ON c.id=d.collection_id \
             WHERE (d.deleted_at IS NOT NULL AND d.deleted_at<=?1) \
                OR (c.deleted_at IS NOT NULL AND c.deleted_at<=?1))",
            params![cutoff],
        )?;
        transaction.execute(
            "DELETE FROM documents WHERE (deleted_at IS NOT NULL AND deleted_at<=?1) \
             OR collection_id IN (SELECT id FROM collections WHERE deleted_at IS NOT NULL AND deleted_at<=?1)",
            params![cutoff],
        )?;
        transaction.execute(
            "DELETE FROM collections WHERE deleted_at IS NOT NULL AND deleted_at<=?1",
            params![cutoff],
        )?;
        transaction.commit()?;
        Ok(paths)
    }

    pub fn pending_document_ids(&self) -> rusqlite::Result<Vec<i64>> {
        self.with_read_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT d.id FROM documents d JOIN collections c ON c.id=d.collection_id \
                 WHERE d.status IN ('pending','failed') AND d.deleted_at IS NULL AND c.deleted_at IS NULL \
                 ORDER BY d.updated_at,d.id",
            )?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect()
        })
    }

    pub fn search(
        &self,
        collection_ids: &[i64],
        query: &str,
        query_vector: &[f32],
        limit: usize,
    ) -> rusqlite::Result<Vec<SearchHit>> {
        if collection_ids.is_empty() || query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let ids = id_list(collection_ids);
        let candidate_limit = limit.clamp(1, 50) * 4;
        let fts = self.search_fts(&ids, query, candidate_limit)?;
        let vectors = self.search_vectors(&ids, query_vector, candidate_limit)?;
        let mut merged: HashMap<(i64, i64), (SearchHit, f64)> = HashMap::new();
        for (rank, hit) in fts.into_iter().enumerate() {
            let key = (hit.document_id, hit.ord);
            let entry = merged.entry(key).or_insert((hit, 0.0));
            entry.1 += 1.0 / (60.0 + rank as f64 + 1.0);
        }
        for (rank, hit) in vectors.into_iter().enumerate() {
            let key = (hit.document_id, hit.ord);
            let entry = merged.entry(key).or_insert((hit, 0.0));
            entry.1 += 1.0 / (60.0 + rank as f64 + 1.0);
        }
        let mut output = merged
            .into_values()
            .map(|(mut hit, score)| {
                hit.score = score;
                hit
            })
            .collect::<Vec<_>>();
        output.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.document_id.cmp(&right.document_id))
                .then_with(|| left.ord.cmp(&right.ord))
        });
        output.truncate(limit.clamp(1, 50));
        Ok(output)
    }

    fn search_fts(&self, ids: &str, query: &str, limit: usize) -> rusqlite::Result<Vec<SearchHit>> {
        self.with_read_connection(|connection| {
            search_fts_on_connection(connection, ids, query, limit)
        })
    }

    fn search_vectors(
        &self,
        ids: &str,
        query: &[f32],
        limit: usize,
    ) -> rusqlite::Result<Vec<SearchHit>> {
        if limit == 0 || query.is_empty() {
            return Ok(Vec::new());
        }

        self.with_read_connection(|connection| {
            search_vectors_on_connection(connection, ids, query, limit)
        })
    }

    pub fn source_chunks(
        &self,
        collection_id: i64,
        document_id: i64,
        start_ord: i64,
        limit: usize,
    ) -> rusqlite::Result<Vec<SourceChunk>> {
        self.with_read_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT k.ord,k.text FROM chunks k JOIN documents d ON d.id=k.document_id \
                 JOIN collections c ON c.id=k.collection_id WHERE k.collection_id=?1 AND k.document_id=?2 \
                 AND k.ord>=?3 AND d.status='ready' AND d.deleted_at IS NULL AND c.deleted_at IS NULL \
                 ORDER BY k.ord LIMIT ?4",
            )?;
            let rows = statement.query_map(
                params![
                    collection_id,
                    document_id,
                    start_ord.max(0),
                    limit.clamp(1, 20) as i64
                ],
                |row| {
                    Ok(SourceChunk {
                        ord: row.get(0)?,
                        text: row.get(1)?,
                    })
                },
            )?;
            rows.collect()
        })
    }

    pub fn create_share(
        &self,
        id: &str,
        secret_hash: &str,
        endpoints: &[String],
        auto_approve_read: bool,
        expires_at: i64,
    ) -> rusqlite::Result<ShareRecord> {
        let created_at = now();
        let endpoints_json = serde_json::to_string(endpoints)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        self.conn.lock().execute(
            "INSERT INTO shares(id,secret_hash,endpoints_json,auto_approve_read,created_at,expires_at) \
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                id,
                secret_hash,
                endpoints_json,
                auto_approve_read,
                created_at,
                expires_at
            ],
        )?;
        Ok(ShareRecord {
            id: id.to_string(),
            endpoints: endpoints.to_vec(),
            auto_approve_read,
            created_at,
            expires_at,
            stopped_at: None,
        })
    }

    pub fn list_shares(&self) -> rusqlite::Result<Vec<ShareRecord>> {
        self.with_read_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id,endpoints_json,auto_approve_read,created_at,expires_at,stopped_at \
                 FROM shares ORDER BY created_at DESC,id DESC",
            )?;
            let records = statement.query_map([], map_share)?.collect();
            records
        })
    }

    pub fn stop_share(&self, id: &str) -> rusqlite::Result<ShareRecord> {
        let stopped_at = now();
        let changed = self.conn.lock().execute(
            "UPDATE shares SET stopped_at=?2 WHERE id=?1 AND stopped_at IS NULL",
            params![id, stopped_at],
        )?;
        if changed != 1 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        self.with_read_connection(|connection| {
            connection.query_row(
                "SELECT id,endpoints_json,auto_approve_read,created_at,expires_at,stopped_at \
                 FROM shares WHERE id=?1",
                params![id],
                map_share,
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_join_request(
        &self,
        id: &str,
        claim_hash: &str,
        device_name: &str,
        device_id: &str,
        token_hash: &str,
        share_secret_hash: Option<&str>,
        expires_at: i64,
    ) -> rusqlite::Result<JoinRequestRecord> {
        let mut connection = self.conn.lock();
        let transaction = connection.transaction()?;
        let created_at = now();
        let (share_id, auto_approve_read) = match share_secret_hash {
            Some(secret_hash) => transaction.query_row(
                "SELECT id,auto_approve_read FROM shares \
                 WHERE secret_hash=?1 AND stopped_at IS NULL AND expires_at>=?2",
                params![secret_hash, created_at],
                |row| Ok((Some(row.get::<_, String>(0)?), row.get::<_, bool>(1)?)),
            )?,
            None => (None, false),
        };
        let status = if auto_approve_read {
            JoinRequestStatus::Approved
        } else {
            JoinRequestStatus::Pending
        };
        let scope = auto_approve_read.then_some(AccessScope::Read);
        let resolved_at = auto_approve_read.then_some(created_at);
        transaction.execute(
            "INSERT INTO join_requests(\
                id,claim_hash,device_name,device_id,token_hash,share_id,status,scope,created_at,expires_at,resolved_at\
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                id,
                claim_hash,
                device_name,
                device_id,
                token_hash,
                share_id,
                join_status_text(status),
                scope.map(scope_text),
                created_at,
                expires_at,
                resolved_at
            ],
        )?;
        if auto_approve_read {
            transaction.execute(
                "INSERT INTO devices(id,name,scope,token_hash,created_at) VALUES(?1,?2,'read',?3,?4)",
                params![device_id, device_name, token_hash, created_at],
            )?;
        }
        transaction.commit()?;
        Ok(JoinRequestRecord {
            id: id.to_string(),
            device_name: device_name.to_string(),
            status,
            scope,
            share_id,
            device_id: scope.map(|_| device_id.to_string()),
            created_at,
            expires_at,
            resolved_at,
        })
    }

    pub fn join_request_by_claim(
        &self,
        id: &str,
        claim_hash: &str,
    ) -> rusqlite::Result<JoinRequestRecord> {
        let timestamp = now();
        let mut connection = self.conn.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE join_requests SET status='expired',resolved_at=?3 \
             WHERE id=?1 AND claim_hash=?2 AND status='pending' AND expires_at<?3",
            params![id, claim_hash, timestamp],
        )?;
        let request = transaction.query_row(
            "SELECT id,device_name,status,scope,share_id,device_id,created_at,expires_at,resolved_at \
             FROM join_requests WHERE id=?1 AND claim_hash=?2",
            params![id, claim_hash],
            map_join_request,
        )?;
        transaction.commit()?;
        Ok(request)
    }

    pub fn list_join_requests(
        &self,
        limit: Option<usize>,
        offset: usize,
    ) -> rusqlite::Result<Vec<JoinRequestRecord>> {
        self.with_read_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id,device_name,status,scope,share_id,device_id,created_at,expires_at,resolved_at \
                 FROM join_requests ORDER BY created_at DESC,id DESC LIMIT ?1 OFFSET ?2",
            )?;
            let records = statement
                .query_map(
                    params![
                        limit.map(|value| value.clamp(1, 500) as i64).unwrap_or(-1),
                        offset as i64
                    ],
                    map_join_request,
                )?
                .collect();
            records
        })
    }

    pub fn approve_join_request(
        &self,
        id: &str,
        scope: AccessScope,
    ) -> rusqlite::Result<JoinRequestRecord> {
        let mut connection = self.conn.lock();
        let transaction = connection.transaction()?;
        let timestamp = now();
        let (device_name, device_id, token_hash, expires_at) = transaction.query_row(
            "SELECT device_name,device_id,token_hash,expires_at FROM join_requests \
             WHERE id=?1 AND status='pending'",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )?;
        if expires_at < timestamp {
            transaction.execute(
                "UPDATE join_requests SET status='expired',resolved_at=?2 WHERE id=?1",
                params![id, timestamp],
            )?;
            transaction.commit()?;
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        transaction.execute(
            "INSERT INTO devices(id,name,scope,token_hash,created_at) VALUES(?1,?2,?3,?4,?5)",
            params![
                device_id,
                device_name,
                scope_text(scope),
                token_hash,
                timestamp
            ],
        )?;
        transaction.execute(
            "UPDATE join_requests SET status='approved',scope=?2,resolved_at=?3 WHERE id=?1",
            params![id, scope_text(scope), timestamp],
        )?;
        let request = transaction.query_row(
            "SELECT id,device_name,status,scope,share_id,device_id,created_at,expires_at,resolved_at \
             FROM join_requests WHERE id=?1",
            params![id],
            map_join_request,
        )?;
        transaction.commit()?;
        Ok(request)
    }

    pub fn reject_join_request(&self, id: &str) -> rusqlite::Result<JoinRequestRecord> {
        self.resolve_pending_join_request(id, None, JoinRequestStatus::Rejected)
    }

    pub fn cancel_join_request(
        &self,
        id: &str,
        claim_hash: &str,
    ) -> rusqlite::Result<JoinRequestRecord> {
        let timestamp = now();
        let changed = self.conn.lock().execute(
            "UPDATE join_requests SET status='cancelled',resolved_at=?3 \
             WHERE id=?1 AND claim_hash=?2 AND status='pending'",
            params![id, claim_hash, timestamp],
        )?;
        if changed != 1 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        self.join_request_by_claim(id, claim_hash)
    }

    fn resolve_pending_join_request(
        &self,
        id: &str,
        scope: Option<AccessScope>,
        status: JoinRequestStatus,
    ) -> rusqlite::Result<JoinRequestRecord> {
        let timestamp = now();
        let changed = self.conn.lock().execute(
            "UPDATE join_requests SET status=?2,scope=?3,resolved_at=?4 WHERE id=?1 AND status='pending'",
            params![id, join_status_text(status), scope.map(scope_text), timestamp],
        )?;
        if changed != 1 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        self.with_read_connection(|connection| {
            connection.query_row(
                "SELECT id,device_name,status,scope,share_id,device_id,created_at,expires_at,resolved_at \
                 FROM join_requests WHERE id=?1",
                params![id],
                map_join_request,
            )
        })
    }

    pub fn add_device(
        &self,
        device_id: &str,
        device_name: &str,
        scope: AccessScope,
        token_hash: &str,
    ) -> rusqlite::Result<()> {
        self.conn.lock().execute(
            "INSERT INTO devices(id,name,scope,token_hash,created_at) VALUES(?1,?2,?3,?4,?5)",
            params![device_id, device_name, scope_text(scope), token_hash, now()],
        )?;
        Ok(())
    }

    pub fn add_host_owner_device(
        &self,
        device_id: &str,
        device_name: &str,
        token_hash: &str,
        owner_meta_key: &str,
    ) -> rusqlite::Result<()> {
        let mut connection = self.conn.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO devices(id,name,scope,token_hash,created_at) VALUES(?1,?2,'owner',?3,?4)",
            params![device_id, device_name, token_hash, now()],
        )?;
        transaction.execute(
            "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![owner_meta_key, device_id],
        )?;
        transaction.commit()
    }

    pub fn recover_host_owner_device(
        &self,
        device_id: &str,
        device_name: &str,
        token_hash: &str,
        owner_meta_key: &str,
    ) -> rusqlite::Result<()> {
        let mut connection = self.conn.lock();
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE devices SET name=?2,scope='owner',token_hash=?3,revoked=0 WHERE id=?1",
            params![device_id, device_name, token_hash],
        )?;
        if changed == 0 {
            transaction.execute(
                "INSERT INTO devices(id,name,scope,token_hash,created_at) VALUES(?1,?2,'owner',?3,?4)",
                params![device_id, device_name, token_hash, now()],
            )?;
        }
        transaction.execute(
            "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![owner_meta_key, device_id],
        )?;
        transaction.commit()
    }

    pub fn authorize_token(&self, token_hash: &str) -> rusqlite::Result<Option<DeviceGrant>> {
        let device = self.with_read_connection(|connection| {
            connection
                .query_row(
                    "SELECT id,name,scope,created_at,last_seen_at,revoked FROM devices WHERE token_hash=?1",
                    params![token_hash],
                    map_device,
                )
                .optional()
        })?;
        if let Some(grant) = &device {
            if !grant.revoked {
                // Authentication must remain responsive while a large index
                // transaction owns the primary connection. Presence updates are
                // telemetry, so skip rather than queue behind that transaction.
                if let Some(connection) = self.conn.try_lock() {
                    let timestamp = now();
                    let _ = connection.execute(
                        "UPDATE devices SET last_seen_at=?2 WHERE id=?1 AND (last_seen_at IS NULL OR last_seen_at<?3)",
                        params![grant.id, timestamp, timestamp - 60],
                    );
                }
            }
        }
        Ok(device.filter(|grant| !grant.revoked))
    }

    pub fn list_devices(&self) -> rusqlite::Result<Vec<DeviceGrant>> {
        self.list_devices_page(None, 0)
    }

    pub fn device_count(&self) -> rusqlite::Result<i64> {
        self.with_read_connection(|connection| {
            connection.query_row("SELECT COUNT(*) FROM devices WHERE revoked=0", [], |row| {
                row.get(0)
            })
        })
    }

    pub fn list_devices_page(
        &self,
        limit: Option<usize>,
        offset: usize,
    ) -> rusqlite::Result<Vec<DeviceGrant>> {
        self.with_read_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id,name,scope,created_at,last_seen_at,revoked FROM devices \
                 ORDER BY created_at DESC,id DESC LIMIT ?1 OFFSET ?2",
            )?;
            let rows = statement.query_map(
                params![
                    limit.map(|value| value.clamp(1, 500) as i64).unwrap_or(-1),
                    offset as i64
                ],
                map_device,
            )?;
            rows.collect()
        })
    }

    pub fn device(&self, id: &str) -> rusqlite::Result<Option<DeviceGrant>> {
        self.with_read_connection(|connection| {
            connection
                .query_row(
                    "SELECT id,name,scope,created_at,last_seen_at,revoked FROM devices WHERE id=?1",
                    params![id],
                    map_device,
                )
                .optional()
        })
    }

    pub fn update_device(
        &self,
        id: &str,
        name: Option<&str>,
        scope: Option<AccessScope>,
        revoked: Option<bool>,
    ) -> Result<DeviceGrant, DeviceMutationError> {
        let mut connection = self.conn.lock();
        let transaction = connection.transaction()?;
        let current = transaction
            .query_row(
                "SELECT id,name,scope,created_at,last_seen_at,revoked FROM devices WHERE id=?1",
                params![id],
                map_device,
            )
            .optional()?
            .ok_or(DeviceMutationError::NotFound)?;
        if current.scope.is_owner() {
            return Err(DeviceMutationError::OwnerProtected);
        }
        transaction.execute(
            "UPDATE devices SET name=?2,scope=?3,revoked=?4 WHERE id=?1",
            params![
                id,
                name.unwrap_or(&current.name),
                scope_text(scope.unwrap_or(current.scope)),
                revoked.unwrap_or(current.revoked),
            ],
        )?;
        let updated = transaction.query_row(
            "SELECT id,name,scope,created_at,last_seen_at,revoked FROM devices WHERE id=?1",
            params![id],
            map_device,
        )?;
        transaction.commit()?;
        Ok(updated)
    }

    pub fn delete_device(&self, id: &str) -> Result<(), DeviceMutationError> {
        let mut connection = self.conn.lock();
        let transaction = connection.transaction()?;
        let scope = transaction
            .query_row(
                "SELECT scope FROM devices WHERE id=?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or(DeviceMutationError::NotFound)?;
        if parse_scope(&scope).is_owner() {
            return Err(DeviceMutationError::OwnerProtected);
        }
        // Unauthenticated join requests cannot be reliably tied to an
        // existing device. Invalidate all pending requests in the same
        // transaction so removing a member cannot later resurrect a token
        // that was staged before the removal.
        transaction.execute(
            "UPDATE join_requests SET status='rejected',resolved_at=?1 WHERE status='pending'",
            params![now()],
        )?;
        let changed = transaction.execute("DELETE FROM devices WHERE id=?1", params![id])?;
        if changed != 1 {
            return Err(DeviceMutationError::NotFound);
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn set_owner_role(
        &self,
        id: &str,
        owner: bool,
        host_owner_meta: &str,
    ) -> Result<DeviceGrant, DeviceMutationError> {
        let mut connection = self.conn.lock();
        let transaction = connection.transaction()?;
        let host_owner = transaction
            .query_row(
                "SELECT value FROM meta WHERE key=?1",
                params![host_owner_meta],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if host_owner.as_deref() == Some(id) {
            return Err(DeviceMutationError::HostOwnerProtected);
        }
        let current = transaction
            .query_row(
                "SELECT id,name,scope,created_at,last_seen_at,revoked FROM devices WHERE id=?1",
                params![id],
                map_device,
            )
            .optional()?
            .ok_or(DeviceMutationError::NotFound)?;
        if current.revoked {
            return Err(DeviceMutationError::Revoked);
        }
        transaction.execute(
            "UPDATE devices SET scope=?2 WHERE id=?1",
            params![
                id,
                scope_text(if owner {
                    AccessScope::Owner
                } else {
                    AccessScope::Manage
                })
            ],
        )?;
        let updated = transaction.query_row(
            "SELECT id,name,scope,created_at,last_seen_at,revoked FROM devices WHERE id=?1",
            params![id],
            map_device,
        )?;
        transaction.commit()?;
        Ok(updated)
    }
}

pub(crate) fn configure_connection(connection: &Connection) -> rusqlite::Result<()> {
    connection.busy_timeout(SQLITE_BUSY_TIMEOUT)
}

fn search_fts_on_connection(
    connection: &Connection,
    ids: &str,
    query: &str,
    limit: usize,
) -> rusqlite::Result<Vec<SearchHit>> {
    let base = "SELECT k.collection_id,k.document_id,d.name,k.text,k.ord,bm25(chunks_fts) \
                FROM chunks_fts JOIN chunks k ON k.id=chunks_fts.rowid \
                JOIN documents d ON d.id=k.document_id JOIN collections c ON c.id=k.collection_id";
    let sql;
    if query.chars().count() >= 3 {
        sql = format!(
            "{base} WHERE k.collection_id IN ({ids}) AND d.status='ready' AND d.deleted_at IS NULL \
             AND c.deleted_at IS NULL AND chunks_fts MATCH ?1 ORDER BY bm25(chunks_fts),k.id LIMIT ?2"
        );
        let phrase = format!("\"{}\"", query.replace('"', "\"\""));
        let mut statement = connection.prepare(&sql)?;
        let mapped = statement.query_map(params![phrase, limit as i64], map_search_hit)?;
        mapped.collect()
    } else {
        sql = format!(
            "{base} WHERE k.collection_id IN ({ids}) AND d.status='ready' AND d.deleted_at IS NULL \
             AND c.deleted_at IS NULL AND k.text LIKE ?1 ESCAPE '\\' ORDER BY k.id LIMIT ?2"
        );
        let pattern = format!("%{}%", escape_like(query));
        let mut statement = connection.prepare(&sql)?;
        let mapped = statement.query_map(params![pattern, limit as i64], map_search_hit)?;
        mapped.collect()
    }
}

fn search_vectors_on_connection(
    connection: &mut Connection,
    ids: &str,
    query: &[f32],
    limit: usize,
) -> rusqlite::Result<Vec<SearchHit>> {
    let transaction = connection.transaction()?;
    let mut best = BinaryHeap::<Reverse<VectorCandidate>>::with_capacity(limit);
    let active_chunks_sql = format!(
        "SELECT COUNT(*) FROM (SELECT 1 FROM chunks k \
         JOIN documents d ON d.id=k.document_id JOIN collections c ON c.id=k.collection_id \
         WHERE k.collection_id IN ({ids}) AND d.status='ready' AND d.deleted_at IS NULL \
         AND c.deleted_at IS NULL LIMIT {})",
        VECTOR_EXACT_SCAN_THRESHOLD + 1
    );
    let active_chunks =
        transaction.query_row(&active_chunks_sql, [], |row| row.get::<_, i64>(0))?;
    let signature_filter = if active_chunks <= VECTOR_EXACT_SCAN_THRESHOLD {
        String::new()
    } else {
        let signature_ids = id_list(&vector_signature_neighbors(vector_signature(query)));
        format!("AND (k.vec_sig IS NULL OR k.vec_sig IN ({signature_ids}))")
    };
    let vector_sql = format!(
        "SELECT k.id,k.vec FROM chunks k \
         JOIN documents d ON d.id=k.document_id JOIN collections c ON c.id=k.collection_id \
         WHERE k.collection_id IN ({ids}) {signature_filter} \
         AND d.status='ready' AND d.deleted_at IS NULL AND c.deleted_at IS NULL"
    );
    {
        let mut statement = transaction.prepare(&vector_sql)?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let chunk_id = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            let Some(score) = cosine_blob(query, &blob) else {
                continue;
            };
            let candidate = VectorCandidate { chunk_id, score };
            if best.len() < limit {
                best.push(Reverse(candidate));
            } else if best.peek().is_some_and(|worst| candidate > worst.0) {
                best.pop();
                best.push(Reverse(candidate));
            }
        }
    }

    let mut candidates = best
        .into_iter()
        .map(|candidate| candidate.0)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.cmp(left));
    if candidates.is_empty() {
        transaction.commit()?;
        return Ok(Vec::new());
    }

    // Only materialize names and chunk text for the bounded winner set. Keeping
    // both passes in one read transaction gives the scan and metadata lookup the
    // same WAL snapshot while writers continue on the primary connection.
    let chunk_ids = id_list(
        &candidates
            .iter()
            .map(|candidate| candidate.chunk_id)
            .collect::<Vec<_>>(),
    );
    let metadata_sql = format!(
        "SELECT k.id,k.collection_id,k.document_id,d.name,k.text,k.ord FROM chunks k \
         JOIN documents d ON d.id=k.document_id JOIN collections c ON c.id=k.collection_id \
         WHERE k.id IN ({chunk_ids}) AND d.status='ready' AND d.deleted_at IS NULL AND c.deleted_at IS NULL"
    );
    let mut metadata = HashMap::with_capacity(candidates.len());
    {
        let mut statement = transaction.prepare(&metadata_sql)?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                SearchHit {
                    collection_id: row.get(1)?,
                    document_id: row.get(2)?,
                    document_name: row.get(3)?,
                    text: row.get(4)?,
                    ord: row.get(5)?,
                    score: 0.0,
                },
            ))
        })?;
        for row in rows {
            let (chunk_id, hit) = row?;
            metadata.insert(chunk_id, hit);
        }
    }
    transaction.commit()?;

    Ok(candidates
        .into_iter()
        .filter_map(|candidate| {
            let mut hit = metadata.remove(&candidate.chunk_id)?;
            hit.score = candidate.score as f64;
            Some(hit)
        })
        .collect())
}

fn ensure_vector_signature_schema(connection: &Connection) -> rusqlite::Result<()> {
    let has_signature = {
        let mut statement = connection.prepare("PRAGMA table_info(chunks)")?;
        let column_names = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        column_names.iter().any(|name| name == "vec_sig")
    };
    if !has_signature {
        connection.execute("ALTER TABLE chunks ADD COLUMN vec_sig INTEGER", [])?;
    }
    connection.execute(
        "CREATE INDEX IF NOT EXISTS idx_remote_chunks_vector_bucket \
         ON chunks(collection_id,vec_sig)",
        [],
    )?;
    Ok(())
}

fn vector_signature(vector: &[f32]) -> i64 {
    let mut signature = 0i64;
    for bit in 0..VECTOR_SIGNATURE_BITS {
        let projection = vector
            .iter()
            .enumerate()
            .fold(0.0f32, |total, (index, value)| {
                let seed = (index as u64)
                    .wrapping_mul(0x9e37_79b9_7f4a_7c15)
                    .wrapping_add((bit as u64 + 1).wrapping_mul(0xbf58_476d_1ce4_e5b9));
                let mixed = splitmix64(seed);
                total + if mixed & 1 == 0 { -*value } else { *value }
            });
        if projection >= 0.0 {
            signature |= 1i64 << bit;
        }
    }
    signature
}

fn splitmix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn vector_signature_neighbors(signature: i64) -> Vec<i64> {
    let mut output = Vec::with_capacity(697);
    output.push(signature);
    for first in 0..VECTOR_SIGNATURE_BITS {
        output.push(signature ^ (1i64 << first));
        if VECTOR_SIGNATURE_RADIUS < 2 {
            continue;
        }
        for second in (first + 1)..VECTOR_SIGNATURE_BITS {
            output.push(signature ^ (1i64 << first) ^ (1i64 << second));
            if VECTOR_SIGNATURE_RADIUS < 3 {
                continue;
            }
            for third in (second + 1)..VECTOR_SIGNATURE_BITS {
                output.push(signature ^ (1i64 << first) ^ (1i64 << second) ^ (1i64 << third));
            }
        }
    }
    output.sort_unstable();
    output
}

fn cosine_blob(query: &[f32], blob: &[u8]) -> Option<f32> {
    if query.is_empty() || query.len().checked_mul(4)? != blob.len() {
        return None;
    }
    let mut score = 0.0f32;
    for (left, chunk) in query.iter().zip(blob.chunks_exact(4)) {
        let right = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        score += left * right;
    }
    score.is_finite().then_some(score)
}

fn map_collection(row: &rusqlite::Row<'_>) -> rusqlite::Result<Collection> {
    Ok(Collection {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        status: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        deleted_at: row.get(6)?,
        doc_count: row.get(7)?,
        chunk_count: row.get(8)?,
        total_bytes: row.get(9)?,
    })
}

fn map_document(row: &rusqlite::Row<'_>) -> rusqlite::Result<Document> {
    Ok(Document {
        id: row.get(0)?,
        collection_id: row.get(1)?,
        name: row.get(2)?,
        ext: row.get(3)?,
        size: row.get(4)?,
        sha256: row.get(5)?,
        status: row.get(6)?,
        n_chunks: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        deleted_at: row.get(10)?,
        error: row.get(11)?,
        already_exists: false,
    })
}

fn map_search_hit(row: &rusqlite::Row<'_>) -> rusqlite::Result<SearchHit> {
    Ok(SearchHit {
        collection_id: row.get(0)?,
        document_id: row.get(1)?,
        document_name: row.get(2)?,
        text: row.get(3)?,
        ord: row.get(4)?,
        score: row.get::<_, f64>(5)?,
    })
}

fn map_device(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeviceGrant> {
    Ok(DeviceGrant {
        id: row.get(0)?,
        name: row.get(1)?,
        scope: parse_scope(&row.get::<_, String>(2)?),
        created_at: row.get(3)?,
        last_seen_at: row.get(4)?,
        revoked: row.get(5)?,
    })
}

fn map_share(row: &rusqlite::Row<'_>) -> rusqlite::Result<ShareRecord> {
    let endpoints_json = row.get::<_, String>(1)?;
    let endpoints = serde_json::from_str(&endpoints_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(1, Type::Text, Box::new(error))
    })?;
    Ok(ShareRecord {
        id: row.get(0)?,
        endpoints,
        auto_approve_read: row.get(2)?,
        created_at: row.get(3)?,
        expires_at: row.get(4)?,
        stopped_at: row.get(5)?,
    })
}

fn map_join_request(row: &rusqlite::Row<'_>) -> rusqlite::Result<JoinRequestRecord> {
    let scope = row
        .get::<_, Option<String>>(3)?
        .map(|value| parse_scope(&value));
    let status = parse_join_status(&row.get::<_, String>(2)?).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid join request status",
            )),
        )
    })?;
    Ok(JoinRequestRecord {
        id: row.get(0)?,
        device_name: row.get(1)?,
        status,
        scope,
        share_id: row.get(4)?,
        device_id: scope.and_then(|_| row.get(5).ok()),
        created_at: row.get(6)?,
        expires_at: row.get(7)?,
        resolved_at: row.get(8)?,
    })
}

fn scope_text(scope: AccessScope) -> &'static str {
    match scope {
        AccessScope::Read => "read",
        AccessScope::Manage => "manage",
        AccessScope::Owner => "owner",
    }
}

fn parse_scope(value: &str) -> AccessScope {
    match value {
        "owner" => AccessScope::Owner,
        "manage" => AccessScope::Manage,
        _ => AccessScope::Read,
    }
}

fn join_status_text(status: JoinRequestStatus) -> &'static str {
    match status {
        JoinRequestStatus::Pending => "pending",
        JoinRequestStatus::Approved => "approved",
        JoinRequestStatus::Rejected => "rejected",
        JoinRequestStatus::Cancelled => "cancelled",
        JoinRequestStatus::Expired => "expired",
    }
}

fn parse_join_status(value: &str) -> Option<JoinRequestStatus> {
    match value {
        "pending" => Some(JoinRequestStatus::Pending),
        "approved" => Some(JoinRequestStatus::Approved),
        "rejected" => Some(JoinRequestStatus::Rejected),
        "cancelled" => Some(JoinRequestStatus::Cancelled),
        "expired" => Some(JoinRequestStatus::Expired),
        _ => None,
    }
}

fn id_list(ids: &[i64]) -> String {
    ids.iter()
        .copied()
        .filter(|id| *id > 0)
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn escape_like(value: &str) -> String {
    value.replace('%', "\\%").replace('_', "\\_")
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use rusqlite::Connection;

    use super::{
        vector_signature, vector_signature_neighbors, DeviceMutationError, DocumentIndexUpdate,
        RestoreDocumentOutcome, Store, SQLITE_BUSY_TIMEOUT, VECTOR_SIGNATURE_RADIUS,
    };

    #[test]
    fn collection_trash_is_hidden_and_restorable() {
        let store = Store::in_memory().unwrap();
        let collection = store.create_collection("共享资料", None).unwrap();
        store.trash_collection(collection.id).unwrap();
        assert!(store.list_collections(false).unwrap().is_empty());
        store.restore_collection(collection.id).unwrap();
        assert_eq!(store.list_collections(false).unwrap().len(), 1);
    }

    #[test]
    fn document_pages_are_stable_and_full_listing_remains_available() {
        let store = Store::in_memory().unwrap();
        let collection = store.create_collection("共享资料", None).unwrap();
        let mut inserted = Vec::new();
        for name in ["first.txt", "second.txt", "third.txt"] {
            inserted.push(
                store
                    .insert_document(collection.id, name, Some("txt"), name, 1, name)
                    .unwrap(),
            );
        }

        let first_page = store
            .list_documents_page(collection.id, false, Some(2), 0)
            .unwrap();
        let second_page = store
            .list_documents_page(collection.id, false, Some(2), 2)
            .unwrap();
        let full = store.list_documents(collection.id, false).unwrap();

        assert_eq!(
            first_page
                .iter()
                .map(|document| document.id)
                .collect::<Vec<_>>(),
            vec![inserted[2].id, inserted[1].id]
        );
        assert_eq!(
            second_page
                .iter()
                .map(|document| document.id)
                .collect::<Vec<_>>(),
            vec![inserted[0].id]
        );
        assert_eq!(full.len(), 3);
    }

    #[test]
    fn document_pagination_is_stable_when_a_later_document_is_updated() {
        let store = Store::in_memory().unwrap();
        let collection = store.create_collection("pagination", None).unwrap();
        let mut inserted = Vec::new();
        for name in ["first.txt", "second.txt", "third.txt", "fourth.txt"] {
            inserted.push(
                store
                    .insert_document(collection.id, name, Some("txt"), name, 1, name)
                    .unwrap(),
            );
        }

        let first_page = store
            .list_documents_page(collection.id, false, Some(2), 0)
            .unwrap();
        store
            .conn
            .lock()
            .execute(
                "UPDATE documents SET status='failed',updated_at=updated_at+10000 WHERE id=?1",
                rusqlite::params![inserted[1].id],
            )
            .unwrap();
        let second_page = store
            .list_documents_page(collection.id, false, Some(2), 2)
            .unwrap();

        let combined = first_page
            .iter()
            .chain(&second_page)
            .map(|document| document.id)
            .collect::<Vec<_>>();
        assert_eq!(
            combined,
            vec![
                inserted[3].id,
                inserted[2].id,
                inserted[1].id,
                inserted[0].id
            ],
            "an updated_at/status change must not move an item across offset pages"
        );
    }

    #[test]
    fn file_backed_reads_do_not_wait_for_the_primary_connection_mutex() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("knowledge.db")).unwrap();
        store.set_meta("server_name", "Read Replica").unwrap();
        let collection = store.create_collection("readable", None).unwrap();
        let document = store
            .insert_document(
                collection.id,
                "source.txt",
                Some("txt"),
                "managed-source",
                5,
                "source-sha",
            )
            .unwrap();
        let chunks = vec!["alpha".to_string(), "beta".to_string()];
        let vectors = vec![vec![1.0_f32, 0.0], vec![0.0_f32, 1.0]];
        store
            .replace_document_index(
                document.id,
                DocumentIndexUpdate {
                    name: "source.txt",
                    ext: Some("txt"),
                    storage_path: "managed-source",
                    size: 5,
                    sha256: "source-sha",
                    chunks: &chunks,
                    vectors: &vectors,
                },
            )
            .unwrap();
        store
            .add_device(
                "reader-1",
                "Reader",
                crate::model::AccessScope::Read,
                "reader-token",
            )
            .unwrap();

        let primary_connection = store.conn.lock();
        let worker_store = store.clone();
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result: rusqlite::Result<_> = (|| {
                Ok((
                    worker_store.meta("server_name")?,
                    worker_store
                        .list_collections(false)?
                        .into_iter()
                        .map(|value| value.id)
                        .collect::<Vec<_>>(),
                    worker_store
                        .collection(collection.id, false)?
                        .map(|value| value.id),
                    worker_store
                        .list_documents(collection.id, false)?
                        .into_iter()
                        .map(|value| value.id)
                        .collect::<Vec<_>>(),
                    worker_store
                        .list_documents_page(collection.id, false, Some(1), 0)?
                        .into_iter()
                        .map(|value| value.id)
                        .collect::<Vec<_>>(),
                    worker_store
                        .document(document.id, false)?
                        .map(|value| value.document.id),
                    worker_store
                        .documents_by_ids(&[document.id])?
                        .into_iter()
                        .map(|value| value.id)
                        .collect::<Vec<_>>(),
                    worker_store
                        .source_chunks(collection.id, document.id, 0, 20)?
                        .into_iter()
                        .map(|value| (value.ord, value.text))
                        .collect::<Vec<_>>(),
                    worker_store.device_count()?,
                    worker_store
                        .list_devices()?
                        .into_iter()
                        .map(|value| value.id)
                        .collect::<Vec<_>>(),
                    worker_store
                        .list_devices_page(Some(1), 0)?
                        .into_iter()
                        .map(|value| value.id)
                        .collect::<Vec<_>>(),
                    worker_store.device("reader-1")?.map(|value| value.id),
                ))
            })();
            sender
                .send(result.map_err(|error| error.to_string()))
                .unwrap();
        });
        let received = receiver.recv_timeout(Duration::from_secs(2));
        drop(primary_connection);
        worker.join().unwrap();

        let (
            meta,
            collections,
            selected_collection,
            documents,
            document_page,
            selected_document,
            selected_documents,
            source_chunks,
            device_count,
            devices,
            device_page,
            selected_device,
        ) = received
            .expect("pure reads waited for the primary connection mutex")
            .unwrap();
        assert_eq!(meta.as_deref(), Some("Read Replica"));
        assert_eq!(collections, vec![collection.id]);
        assert_eq!(selected_collection, Some(collection.id));
        assert_eq!(documents, vec![document.id]);
        assert_eq!(document_page, vec![document.id]);
        assert_eq!(selected_document, Some(document.id));
        assert_eq!(selected_documents, vec![document.id]);
        assert_eq!(
            source_chunks,
            vec![(0, "alpha".to_string()), (1, "beta".to_string())]
        );
        assert_eq!(device_count, 1);
        assert_eq!(devices, vec!["reader-1"]);
        assert_eq!(device_page, vec!["reader-1"]);
        assert_eq!(selected_device.as_deref(), Some("reader-1"));
    }

    #[test]
    fn restoring_collection_preserves_documents_trashed_before_collection() {
        let store = Store::in_memory().unwrap();
        let collection = store.create_collection("共享资料", None).unwrap();
        let previously_trashed = store
            .insert_document(
                collection.id,
                "old.txt",
                Some("txt"),
                "managed-old",
                3,
                "old-sha",
            )
            .unwrap();
        let cascaded = store
            .insert_document(
                collection.id,
                "active.txt",
                Some("txt"),
                "managed-active",
                6,
                "active-sha",
            )
            .unwrap();

        store.trash_document(previously_trashed.id).unwrap();
        store.trash_collection(collection.id).unwrap();
        store.restore_collection(collection.id).unwrap();

        assert!(
            store
                .document(previously_trashed.id, false)
                .unwrap()
                .is_none(),
            "a document trashed before the collection must stay in the recycle bin"
        );
        assert!(
            store.document(cascaded.id, false).unwrap().is_some(),
            "a document cascaded by the collection trash action should be restored"
        );
        assert!(
            store.pending_document_ids().unwrap().contains(&cascaded.id),
            "restored unfinished documents must be discoverable for re-indexing"
        );
    }

    #[test]
    fn index_commit_losing_a_trash_race_can_be_requeued_after_restore() {
        let store = Store::in_memory().unwrap();
        let collection = store.create_collection("共享资料", None).unwrap();
        let document = store
            .insert_document(
                collection.id,
                "race.txt",
                Some("txt"),
                "managed-race",
                4,
                "sha",
            )
            .unwrap();
        store.trash_document(document.id).unwrap();
        let chunks = vec!["race".to_string()];
        let vectors = vec![vec![1.0_f32, 0.0]];
        assert!(store
            .replace_document_index(
                document.id,
                DocumentIndexUpdate {
                    name: "race.txt",
                    ext: Some("txt"),
                    storage_path: "managed-race",
                    size: 4,
                    sha256: "sha",
                    chunks: &chunks,
                    vectors: &vectors,
                },
            )
            .is_err());
        store
            .mark_document_failed(document.id, "索引落库失败")
            .unwrap();
        store.restore_document(document.id).unwrap();
        assert_eq!(
            store.pending_document_ids().unwrap(),
            vec![document.id],
            "restoring a document must make an interrupted index job retryable"
        );
    }

    #[test]
    fn restoring_a_trashed_document_cannot_duplicate_active_content() {
        let store = Store::in_memory().unwrap();
        let collection = store.create_collection("共享资料", None).unwrap();
        let trashed = store
            .insert_document(
                collection.id,
                "original.txt",
                Some("txt"),
                "managed-original",
                4,
                "same-sha",
            )
            .unwrap();
        store.trash_document(trashed.id).unwrap();
        let active = store
            .insert_document_if_new(
                collection.id,
                "replacement.txt",
                Some("txt"),
                "managed-replacement",
                4,
                "same-sha",
            )
            .unwrap()
            .0;

        assert_eq!(
            store.restore_document(trashed.id).unwrap(),
            RestoreDocumentOutcome::DuplicateActive {
                active_document_id: active.id
            }
        );
        assert!(store.document(trashed.id, false).unwrap().is_none());
        assert_eq!(store.list_documents(collection.id, false).unwrap().len(), 1);
    }

    #[test]
    fn existing_databases_gain_and_backfill_the_vector_bucket_index() {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("knowledge.db");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE chunks(
                    id INTEGER PRIMARY KEY,
                    document_id INTEGER NOT NULL,
                    collection_id INTEGER NOT NULL,
                    ord INTEGER NOT NULL,
                    text TEXT NOT NULL,
                    vec BLOB NOT NULL,
                    UNIQUE(document_id,ord)
                 );",
            )
            .unwrap();
        drop(connection);

        let store = Store::open(&database).unwrap();
        let has_signature = store
            .conn
            .lock()
            .prepare("PRAGMA table_info(chunks)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .iter()
            .any(|name| name == "vec_sig");

        assert!(has_signature);
        assert_eq!(vector_signature_neighbors(42).len(), 697);
        assert!(vector_signature_neighbors(42)
            .into_iter()
            .all(|candidate| (candidate ^ 42).count_ones() <= VECTOR_SIGNATURE_RADIUS));
    }

    #[test]
    fn small_knowledge_bases_do_not_lose_semantic_hits_to_signature_prefiltering() {
        let store = Store::in_memory().unwrap();
        let collection = store.create_collection("semantic recall", None).unwrap();
        let document = store
            .insert_document(
                collection.id,
                "semantic.txt",
                Some("txt"),
                "managed-semantic",
                1,
                "semantic-sha",
            )
            .unwrap();
        let chunks = vec!["only semantic candidate".to_string()];
        let stored_vector = vec![-1.0_f32, 0.0];
        let query_vector = vec![1.0_f32, 0.0];
        assert!(
            (vector_signature(&stored_vector) ^ vector_signature(&query_vector)).count_ones()
                > VECTOR_SIGNATURE_RADIUS,
            "the fixture must sit outside the approximate signature neighborhood"
        );
        store
            .replace_document_index(
                document.id,
                DocumentIndexUpdate {
                    name: "semantic.txt",
                    ext: Some("txt"),
                    storage_path: "managed-semantic",
                    size: 1,
                    sha256: "semantic-sha",
                    chunks: &chunks,
                    vectors: &[stored_vector],
                },
            )
            .unwrap();

        let hits = store
            .search_vectors(&collection.id.to_string(), &query_vector, 5)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].document_id, document.id);
    }

    #[test]
    fn vector_search_streams_top_k_on_an_independent_read_connection() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("knowledge.db")).unwrap();
        let collection = store.create_collection("向量检索", None).unwrap();
        let document = store
            .insert_document(
                collection.id,
                "vectors.txt",
                Some("txt"),
                "managed-vectors",
                1,
                "sha",
            )
            .unwrap();
        let chunks = (0..1024)
            .map(|index| format!("chunk-{index}"))
            .collect::<Vec<_>>();
        let vectors = (0..1024)
            .map(|index| vec![index as f32, 0.0])
            .collect::<Vec<_>>();
        store
            .replace_document_index(
                document.id,
                DocumentIndexUpdate {
                    name: "vectors.txt",
                    ext: Some("txt"),
                    storage_path: "managed-vectors",
                    size: 1,
                    sha256: "sha",
                    chunks: &chunks,
                    vectors: &vectors,
                },
            )
            .unwrap();
        store
            .conn
            .lock()
            .execute("UPDATE chunks SET vec_sig=NULL", [])
            .unwrap();
        let mut migrated = 0;
        loop {
            let updated = store.backfill_vector_signatures(113).unwrap();
            migrated += updated;
            if updated < 113 {
                break;
            }
        }
        assert_eq!(migrated, 1024);
        assert_eq!(
            store
                .conn
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM chunks WHERE vec_sig IS NULL",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .unwrap(),
            0
        );

        // A file-backed search must not wait for the primary connection's Rust
        // mutex; WAL provides the consistent read snapshot on a separate handle.
        let primary_connection = store.conn.lock();
        let worker_store = store.clone();
        let ids = collection.id.to_string();
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = worker_store
                .search_vectors(&ids, &[1.0, 0.0], 5)
                .map_err(|error| error.to_string());
            sender.send(result).unwrap();
        });
        let received = receiver.recv_timeout(Duration::from_secs(5));
        let fts_store = store.clone();
        let fts_ids = collection.id.to_string();
        let (fts_sender, fts_receiver) = mpsc::channel();
        let fts_worker = std::thread::spawn(move || {
            let result = fts_store
                .search_fts(&fts_ids, "chunk-1023", 5)
                .map_err(|error| error.to_string());
            fts_sender.send(result).unwrap();
        });
        let fts_received = fts_receiver.recv_timeout(Duration::from_secs(5));
        drop(primary_connection);
        worker.join().unwrap();
        fts_worker.join().unwrap();

        let hits = received
            .expect("vector search waited for the primary connection mutex")
            .unwrap();
        assert_eq!(
            hits.iter().map(|hit| hit.ord).collect::<Vec<_>>(),
            vec![1023, 1022, 1021, 1020, 1019]
        );
        assert_eq!(hits[0].text, "chunk-1023");
        assert_eq!(hits.len(), 5);
        let fts_hits = fts_received
            .expect("full-text search waited for the primary connection mutex")
            .unwrap();
        assert_eq!(fts_hits.len(), 1);
        assert_eq!(fts_hits[0].text, "chunk-1023");
    }

    #[test]
    fn authorization_does_not_wait_for_the_primary_connection_mutex() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("knowledge.db")).unwrap();
        store
            .add_device(
                "reader-1",
                "Reader",
                crate::model::AccessScope::Read,
                "known-token-hash",
            )
            .unwrap();

        let primary_connection = store.conn.lock();
        let worker_store = store.clone();
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            sender
                .send(worker_store.authorize_token("known-token-hash"))
                .unwrap();
        });
        let authorized = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("authorization waited for the primary connection mutex")
            .unwrap();
        drop(primary_connection);
        worker.join().unwrap();

        assert_eq!(authorized.unwrap().id, "reader-1");
    }

    #[test]
    fn device_pages_are_bounded_and_stable() {
        let store = Store::in_memory().unwrap();
        for index in 0..205 {
            store
                .add_device(
                    &format!("device-{index:03}"),
                    &format!("Device {index}"),
                    crate::model::AccessScope::Read,
                    &format!("token-{index:03}"),
                )
                .unwrap();
        }

        let first = store.list_devices_page(Some(100), 0).unwrap();
        let second = store.list_devices_page(Some(100), 100).unwrap();
        let third = store.list_devices_page(Some(100), 200).unwrap();

        assert_eq!((first.len(), second.len(), third.len()), (100, 100, 5));
        assert_ne!(first.last().unwrap().id, second.first().unwrap().id);
        assert_eq!(store.device_count().unwrap(), 205);
    }

    #[test]
    fn device_mutations_protect_owners_inside_the_write_transaction() {
        let store = Store::in_memory().unwrap();
        store
            .add_device(
                "owner-1",
                "Owner",
                crate::model::AccessScope::Owner,
                "owner-token",
            )
            .unwrap();

        assert!(matches!(
            store.update_device(
                "owner-1",
                Some("Renamed"),
                Some(crate::model::AccessScope::Read),
                Some(true),
            ),
            Err(DeviceMutationError::OwnerProtected)
        ));
        assert!(matches!(
            store.delete_device("owner-1"),
            Err(DeviceMutationError::OwnerProtected)
        ));
        let owner = store.device("owner-1").unwrap().unwrap();
        assert_eq!(owner.name, "Owner");
        assert_eq!(owner.scope, crate::model::AccessScope::Owner);
        assert!(!owner.revoked);
    }

    #[test]
    fn deleting_a_member_invalidates_pending_join_credentials_atomically() {
        let store = Store::in_memory().unwrap();
        store
            .add_device(
                "member-1",
                "Member",
                crate::model::AccessScope::Read,
                "member-token",
            )
            .unwrap();
        let pending = store
            .create_join_request(
                "request-1",
                "claim-hash",
                "Member",
                "replacement-device",
                "replacement-token",
                None,
                i64::MAX,
            )
            .unwrap();
        assert_eq!(pending.status, crate::model::JoinRequestStatus::Pending);

        store.delete_device("member-1").unwrap();

        assert!(store.device("member-1").unwrap().is_none());
        assert!(store
            .approve_join_request("request-1", crate::model::AccessScope::Read)
            .is_err());
        let resolved = store.list_join_requests(None, 0).unwrap();
        assert_eq!(
            resolved[0].status,
            crate::model::JoinRequestStatus::Rejected
        );
    }

    #[test]
    fn recycle_bin_lists_only_top_level_entries_and_permanent_delete_removes_rows() {
        let store = Store::in_memory().unwrap();
        let active_collection = store.create_collection("在用资料", None).unwrap();
        let trashed_document = store
            .insert_document(
                active_collection.id,
                "removed.md",
                Some("md"),
                "managed-removed",
                7,
                "removed-sha",
            )
            .unwrap();
        let active_document = store
            .insert_document(
                active_collection.id,
                "active.md",
                Some("md"),
                "managed-active",
                6,
                "active-sha",
            )
            .unwrap();
        store.trash_document(trashed_document.id).unwrap();

        let trashed_collection = store.create_collection("整库删除", None).unwrap();
        let first = store
            .insert_document(
                trashed_collection.id,
                "first.txt",
                Some("txt"),
                "managed-first",
                5,
                "first-sha",
            )
            .unwrap();
        let second = store
            .insert_document(
                trashed_collection.id,
                "second.txt",
                Some("txt"),
                "managed-second",
                6,
                "second-sha",
            )
            .unwrap();
        let chunks = vec!["indexed content".to_string()];
        let vectors = vec![vec![1.0_f32, 0.0]];
        store
            .replace_document_index(
                first.id,
                DocumentIndexUpdate {
                    name: "first.txt",
                    ext: Some("txt"),
                    storage_path: "managed-first",
                    size: 5,
                    sha256: "first-sha",
                    chunks: &chunks,
                    vectors: &vectors,
                },
            )
            .unwrap();
        store.trash_collection(trashed_collection.id).unwrap();

        let documents = store.list_trashed_documents_page(200, 0).unwrap();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].document.id, trashed_document.id);
        assert_eq!(documents[0].collection_name, "在用资料");
        let collections = store.list_trashed_collections_page(200, 0).unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].id, trashed_collection.id);
        assert_eq!(collections[0].doc_count, 2);

        assert!(store
            .permanently_delete_document(active_document.id)
            .is_err());
        assert_eq!(
            store
                .permanently_delete_document(trashed_document.id)
                .unwrap(),
            "managed-removed"
        );
        assert!(store.document(trashed_document.id, true).unwrap().is_none());

        let mut paths = store
            .permanently_delete_collection(trashed_collection.id)
            .unwrap();
        paths.sort();
        assert_eq!(paths, vec!["managed-first", "managed-second"]);
        assert!(store
            .collection(trashed_collection.id, true)
            .unwrap()
            .is_none());
        assert!(store.document(first.id, true).unwrap().is_none());
        assert!(store.document(second.id, true).unwrap().is_none());
        let chunk_count: i64 = store
            .conn
            .lock()
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(chunk_count, 0);
    }

    #[test]
    fn expired_trash_is_purged_with_managed_paths() {
        let store = Store::in_memory().unwrap();
        let collection = store.create_collection("旧资料", None).unwrap();
        let document = store
            .insert_document(
                collection.id,
                "old.txt",
                Some("txt"),
                "managed-old",
                3,
                "sha",
            )
            .unwrap();
        store.trash_document(document.id).unwrap();
        store
            .conn
            .lock()
            .execute(
                "UPDATE documents SET deleted_at=1 WHERE id=?1",
                rusqlite::params![document.id],
            )
            .unwrap();
        assert_eq!(store.purge_expired_trash(2).unwrap(), vec!["managed-old"]);
        assert!(store.document(document.id, true).unwrap().is_none());
    }

    #[test]
    fn every_store_connection_uses_a_bounded_busy_timeout() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("knowledge.db")).unwrap();
        let expected = SQLITE_BUSY_TIMEOUT.as_millis() as i64;

        let primary_timeout = store
            .conn
            .lock()
            .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
            .unwrap();
        let read_timeout = store
            .with_read_connection(|connection| {
                connection.query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
            })
            .unwrap();
        let memory_store = Store::in_memory().unwrap();
        let memory_timeout = memory_store
            .conn
            .lock()
            .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
            .unwrap();

        assert_eq!(primary_timeout, expected);
        assert_eq!(read_timeout, expected);
        assert_eq!(memory_timeout, expected);
    }
}
