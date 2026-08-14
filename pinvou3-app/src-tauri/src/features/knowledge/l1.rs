//! L1 知识库：选定文件 → 解析(file_ingest) → 切块 → 入库 → 全文检索。
//!
//! 设计见 docs/本地文件与知识库-设计文档.html。本阶段做**全文版**(chunks_fts)，
//! `chunks.vec` 列留 NULL；向量(embedding)是 Phase 3，检索届时升级为 fts+向量混合。
//! 复用 L0 的同一个 SQLite 连接(同库 index.db，见 [`super::store::Store::conn_arc`])。

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
#[cfg(test)]
use std::sync::Barrier;
use std::time::{Duration, UNIX_EPOCH};

use parking_lot::{Mutex, RwLock};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::embed::{self, Embedder};

/// chunk 切块参数：~512 token ≈ 中文 600 字符；15% 重叠保留上下文。
const CHUNK_CHARS: usize = 600;
const CHUNK_OVERLAP: usize = 90;
/// 单文档向量化块数上限。超过 → 跳过向量、**仅全文检索**（避免上千块在 CPU 上一次性 embedding
/// 把入库卡死，如 5000 行电子表格 ≈ 1845 块）。表格类大文档关键词检索本就够用。
const MAX_EMBED_CHUNKS: usize = 300;
/// embedding 分批大小：批间让步，不长时间独占模型锁/CPU。
const EMBED_BATCH: usize = 32;

/// 对话注入门控阈值：bge-m3 归一化向量，相关内容余弦通常 0.4~0.7，闲聊/无关 < 0.3。
/// 向量 top 余弦低于此且 FTS 也无命中 → 判定该消息与知识集无关，不注入（治"挂了知识集后
/// 连'谢谢''继续'都注入无关片段"）。经验值，偏保守（宁可多注入也别漏召回真问题）。
const RELEVANCE_MIN_COSINE: f64 = 0.35;

/// 知识集（camelCase 回前端）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub id: i64,
    pub name: String,
    pub category: Option<String>,
    pub description: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub status: String,
    pub doc_count: i64,
    pub chunk_count: i64,
    pub total_bytes: i64,
}

/// 知识集内文档（来源文件）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    pub id: i64,
    pub collection_id: i64,
    pub coll_name: Option<String>,
    pub path: String,
    pub name: String,
    pub ext: Option<String>,
    pub size: i64,
    pub mtime: i64,
    pub parse_status: String,
    pub n_chunks: i64,
}

/// 检索命中（带溯源：可定位回原文件）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChunkHit {
    pub document_id: i64,
    pub text: String,
    pub score: f64,
    pub doc_name: String,
    pub doc_path: String,
    pub ord: i64,
}

/// 跨知识集检索命中。相同来源同时存在于多个知识集时合并 collection_ids，正文只返回一份。
#[derive(Debug, Clone, PartialEq)]
pub struct ScopedChunkHit {
    pub collection_ids: Vec<i64>,
    pub hit: ChunkHit,
}

pub(super) enum ImportIngestOutcome {
    Completed,
    Skipped,
    Cancelled,
    Failed(String),
}

#[derive(Clone)]
pub struct L1Store {
    conn: Arc<Mutex<Connection>>,
    /// 配了 embedding 模型则启用向量;None → 纯全文 fts。**可换共享槽**:模型按需下载完成后
    /// `set_embedder` 热加载,所有 L1Store clone(含已在跑的后台线程/会话)立即见新,免重启。
    embedder: Arc<RwLock<Option<Arc<Embedder>>>>,
    /// 单测专用：把后台导入稳定阻塞在 embedding 完成、写检查点之前，以覆盖取消/删除竞态。
    #[cfg(test)]
    checkpoint_gate: Arc<RwLock<Option<(Arc<Barrier>, Arc<Barrier>)>>>,
}

impl L1Store {
    pub fn new(conn: Arc<Mutex<Connection>>, embedder: Option<Arc<Embedder>>) -> Self {
        Self {
            conn,
            embedder: Arc::new(RwLock::new(embedder)),
            #[cfg(test)]
            checkpoint_gate: Arc::new(RwLock::new(None)),
        }
    }

    #[cfg(test)]
    fn set_checkpoint_gate(&self, entered: Arc<Barrier>, release: Arc<Barrier>) {
        *self.checkpoint_gate.write() = Some((entered, release));
    }

    #[cfg(test)]
    fn wait_at_checkpoint_gate(&self) {
        if let Some((entered, release)) = self.checkpoint_gate.read().clone() {
            entered.wait();
            release.wait();
        }
    }

    /// 取当前 embedder 的克隆句柄(只在锁内拷 Arc,立即释锁,不持锁跑推理)。
    fn embedder(&self) -> Option<Arc<Embedder>> {
        self.embedder.read().clone()
    }

    /// 热加载:换 embedding 模型(模型下载完成后调)。None=卸载回纯全文检索。
    pub fn set_embedder(&self, embedder: Option<Arc<Embedder>>) {
        *self.embedder.write() = embedder;
    }

    pub fn has_embedder(&self) -> bool {
        self.embedder.read().is_some()
    }

    /// (source, model) — 给前端显示语义检索状态。
    pub fn embed_info(&self) -> Option<(String, String)> {
        self.embedder
            .read()
            .as_ref()
            .map(|e| (e.source().to_string(), e.model().to_string()))
    }

    // ───────────────────────── 知识集 CRUD ─────────────────────────

    pub fn create_collection(
        &self,
        name: &str,
        category: Option<&str>,
        description: Option<&str>,
    ) -> rusqlite::Result<i64> {
        let now = now();
        let c = self.conn.lock();
        c.execute(
            "INSERT INTO collections(name,category,description,created_at,updated_at,status) \
             VALUES(?1,?2,?3,?4,?4,'ready')",
            params![name, category, description, now],
        )?;
        Ok(c.last_insert_rowid())
    }

    pub fn list_collections(&self) -> rusqlite::Result<Vec<Collection>> {
        let c = self.conn.lock();
        let mut stmt = c.prepare(
            "SELECT c.id,c.name,c.category,c.description,c.created_at,c.updated_at,c.status, \
                    (SELECT COUNT(*) FROM documents d WHERE d.collection_id=c.id), \
                    (SELECT COUNT(*) FROM chunks   k WHERE k.collection_id=c.id), \
                    COALESCE((SELECT SUM(size) FROM documents d WHERE d.collection_id=c.id),0) \
             FROM collections c ORDER BY c.updated_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Collection {
                id: r.get(0)?,
                name: r.get(1)?,
                category: r.get(2)?,
                description: r.get(3)?,
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
                status: r.get(6)?,
                doc_count: r.get(7)?,
                chunk_count: r.get(8)?,
                total_bytes: r.get(9)?,
            })
        })?;
        rows.collect()
    }

    /// 单个知识集名（注入对话上下文块的标题用）。不存在返回 None。
    pub fn collection_name(&self, id: i64) -> rusqlite::Result<Option<String>> {
        self.conn
            .lock()
            .query_row(
                "SELECT name FROM collections WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
    }

    pub fn update_collection(
        &self,
        id: i64,
        name: &str,
        category: Option<&str>,
        description: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.conn.lock().execute(
            "UPDATE collections SET name=?2,category=?3,description=?4,updated_at=?5 WHERE id=?1",
            params![id, name, category, description, now()],
        )?;
        Ok(())
    }

    /// 删知识集 + 其全部文档/块（chunks_fts 由触发器同步）。
    pub fn delete_collection(&self, id: i64) -> rusqlite::Result<()> {
        let mut c = self.conn.lock();
        let tx = c.transaction()?;
        tx.execute(
            "DELETE FROM knowledge_import_staged_chunks WHERE job_id IN \
             (SELECT id FROM knowledge_import_jobs WHERE collection_id=?1)",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM knowledge_import_items WHERE job_id IN \
             (SELECT id FROM knowledge_import_jobs WHERE collection_id=?1)",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM knowledge_import_jobs WHERE collection_id=?1",
            params![id],
        )?;
        tx.execute("DELETE FROM chunks WHERE collection_id=?1", params![id])?;
        tx.execute("DELETE FROM documents WHERE collection_id=?1", params![id])?;
        tx.execute("DELETE FROM collections WHERE id=?1", params![id])?;
        tx.commit()
    }

    pub fn set_collection_status(&self, id: i64, status: &str) {
        let _ = self.conn.lock().execute(
            "UPDATE collections SET status=?2,updated_at=?3 WHERE id=?1",
            params![id, status, now()],
        );
    }

    // ───────────────────────── 文档 ─────────────────────────

    /// 库里是否存在任意已入库文档（任一知识集）。用于「知识库为空时隐藏
    /// kb_search/kb_open_source 工具」门控：
    /// 删光所有文件后不让模型目录里还留着检索工具。EXISTS 子查询走索引，常数时间。
    pub fn has_any_document(&self) -> rusqlite::Result<bool> {
        self.conn
            .lock()
            .query_row("SELECT EXISTS(SELECT 1 FROM documents)", [], |r| r.get(0))
    }

    /// 列出某知识集文档（collection_id<=0 则列出全部知识集的，按最近解析倒序，给"知识库内文件"表）。
    pub fn list_documents(
        &self,
        collection_id: i64,
        limit: usize,
    ) -> rusqlite::Result<Vec<Document>> {
        let c = self.conn.lock();
        let lim = if limit == 0 { 500 } else { limit } as i64;
        let sql = if collection_id > 0 {
            "SELECT d.id,d.collection_id,c.name,d.path,d.name,d.ext,d.size,d.mtime,d.parse_status,d.n_chunks \
             FROM documents d JOIN collections c ON c.id=d.collection_id \
             WHERE d.collection_id=?1 ORDER BY d.name LIMIT ?2"
        } else {
            "SELECT d.id,d.collection_id,c.name,d.path,d.name,d.ext,d.size,d.mtime,d.parse_status,d.n_chunks \
             FROM documents d JOIN collections c ON c.id=d.collection_id \
             ORDER BY d.parsed_at DESC, d.id DESC LIMIT ?2"
        };
        let mut stmt = c.prepare(sql)?;
        let map = |r: &rusqlite::Row| -> rusqlite::Result<Document> {
            Ok(Document {
                id: r.get(0)?,
                collection_id: r.get(1)?,
                coll_name: r.get(2)?,
                path: r.get(3)?,
                name: r.get(4)?,
                ext: r.get(5)?,
                size: r.get(6)?,
                mtime: r.get(7)?,
                parse_status: r.get(8)?,
                n_chunks: r.get(9)?,
            })
        };
        let rows = stmt.query_map(params![collection_id, lim], map)?;
        rows.collect()
    }

    /// 按 id 读取当前知识集内的一份文档。`collection_id` 是权限边界：即使调用方猜到
    /// 其他知识集的 document id，也不会拿到路径或内容。
    pub fn document_in_collection(
        &self,
        collection_id: i64,
        document_id: i64,
    ) -> rusqlite::Result<Option<Document>> {
        let c = self.conn.lock();
        c.query_row(
            "SELECT d.id,d.collection_id,c.name,d.path,d.name,d.ext,d.size,d.mtime,d.parse_status,d.n_chunks \
             FROM documents d JOIN collections c ON c.id=d.collection_id \
             WHERE d.id=?1 AND d.collection_id=?2 AND d.parse_status='parsed'",
            params![document_id, collection_id],
            |r| {
                Ok(Document {
                    id: r.get(0)?,
                    collection_id: r.get(1)?,
                    coll_name: r.get(2)?,
                    path: r.get(3)?,
                    name: r.get(4)?,
                    ext: r.get(5)?,
                    size: r.get(6)?,
                    mtime: r.get(7)?,
                    parse_status: r.get(8)?,
                    n_chunks: r.get(9)?,
                })
            },
        )
        .optional()
    }

    /// 分页读取某份已解析文档的 chunk 快照。这里只查数据库，不重新打开原始 Office/PDF
    /// 文件；二进制解析已经在建索引时由 `file_ingest` 完成。
    pub fn document_chunk_window(
        &self,
        collection_id: i64,
        document_id: i64,
        start_ord: i64,
        limit: usize,
    ) -> rusqlite::Result<Vec<(i64, String)>> {
        let c = self.conn.lock();
        let mut stmt = c.prepare(
            "SELECT k.ord,k.text FROM chunks k JOIN documents d ON d.id=k.document_id \
             WHERE k.document_id=?1 AND k.collection_id=?2 AND d.collection_id=?2 \
               AND k.ord>=?3 ORDER BY k.ord LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![document_id, collection_id, start_ord.max(0), limit as i64],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        rows.collect()
    }

    /// 插入或更新一条文档记录，返回 doc id。
    pub fn upsert_document(
        &self,
        collection_id: i64,
        path: &str,
        name: &str,
        ext: Option<&str>,
        size: i64,
        mtime: i64,
    ) -> rusqlite::Result<i64> {
        let c = self.conn.lock();
        c.execute(
            "INSERT INTO documents(collection_id,path,name,ext,size,mtime,parse_status,parsed_at) \
             SELECT ?1,?2,?3,?4,?5,?6,'pending',0 \
             WHERE EXISTS(SELECT 1 FROM collections WHERE id=?1) \
             ON CONFLICT(collection_id,path) DO UPDATE SET \
               name=excluded.name,ext=excluded.ext,size=excluded.size,mtime=excluded.mtime",
            params![collection_id, path, name, ext, size, mtime],
        )?;
        c.query_row(
            "SELECT id FROM documents WHERE collection_id=?1 AND path=?2",
            params![collection_id, path],
            |r| r.get(0),
        )
    }

    /// 删除文档及其块。
    pub fn remove_document(&self, doc_id: i64) -> rusqlite::Result<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM chunks WHERE document_id=?1", params![doc_id])?;
        c.execute("DELETE FROM documents WHERE id=?1", params![doc_id])?;
        Ok(())
    }

    fn replace_doc_chunks(
        &self,
        doc_id: i64,
        collection_id: i64,
        chunks: &[String],
        vecs: Option<&[Vec<f32>]>,
    ) -> rusqlite::Result<()> {
        let mut guard = self.conn.lock();
        let tx = guard.transaction()?;
        tx.execute("DELETE FROM chunks WHERE document_id=?1", params![doc_id])?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO chunks(document_id,collection_id,ord,text,n_tokens,vec) VALUES(?1,?2,?3,?4,?5,?6)",
            )?;
            for (i, ch) in chunks.iter().enumerate() {
                let blob: Option<Vec<u8>> =
                    vecs.and_then(|vs| vs.get(i)).map(|v| embed::vec_to_blob(v));
                stmt.execute(params![
                    doc_id,
                    collection_id,
                    i as i64,
                    ch,
                    ch.chars().count() as i64,
                    blob
                ])?;
            }
        }
        tx.commit()
    }

    fn set_doc_status(&self, doc_id: i64, status: &str, n_chunks: usize) {
        let _ = self.conn.lock().execute(
            "UPDATE documents SET parse_status=?1,n_chunks=?2,parsed_at=?3 WHERE id=?4",
            params![status, n_chunks as i64, now(), doc_id],
        );
    }

    // ───────────────────────── 解析 + 入库（单文件，供后台批量调） ─────────────────────────

    /// 解析单个文件 → 切块 → 写入。返回 parse_status（parsed/skipped/failed）。
    pub fn ingest_file(&self, collection_id: i64, path: &Path) -> String {
        let path_str = path.to_string_lossy().to_string();
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("(unnamed)")
            .to_string();
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_lowercase());
        let (size, mtime) = match std::fs::metadata(path) {
            Ok(m) => (
                m.len() as i64,
                m.modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
            ),
            Err(_) => (0, 0),
        };

        let doc_id = match self.upsert_document(
            collection_id,
            &path_str,
            &name,
            ext.as_deref(),
            size,
            mtime,
        ) {
            Ok(id) => id,
            Err(_) => return "failed".into(),
        };

        // 复用 file_ingest 解析正文（pdf/docx/md/xlsx/pptx/...）。
        let res = crate::features::files::file_ingest::ingest(path);
        // 图片无正文 → KB 专用 OCR 取图中文字（截图/扫描件/PPT 图等）。仅知识库入库触发，
        // 对话附件图仍走视觉。OCR 没装/失败/识别为空 → 落 skipped（下面 `_` 分支）。
        let body = match res.markdown {
            Some(md) if !md.trim().is_empty() => Some(md),
            _ if res.kind == "image" => crate::features::files::file_ingest::ocr_image_for_kb(path),
            _ => None,
        };
        match body {
            Some(md) if !md.trim().is_empty() => {
                let chunks = chunk_text(&md, CHUNK_CHARS, CHUNK_OVERLAP);
                let n = chunks.len();
                // 配了 embedding 则算向量;大文档跳向量、失败降级——都只影响向量,不阻断入库(仍走全文)。
                let vecs = self.embed_chunks_bounded(&chunks);
                if self
                    .replace_doc_chunks(doc_id, collection_id, &chunks, vecs.as_deref())
                    .is_err()
                {
                    self.set_doc_status(doc_id, "failed", 0);
                    return "failed".into();
                }
                self.set_doc_status(doc_id, "parsed", n);
                "parsed".into()
            }
            // 图片/二进制/空 → 跳过（无可索引文本）。
            _ => {
                self.set_doc_status(doc_id, "skipped", 0);
                "skipped".into()
            }
        }
    }

    /// 可恢复任务使用的单文件入库：每个 embedding 批次先写暂存表；全部完成后，在同一
    /// 事务中替换正式 chunks 并把任务文件标记为 completed。崩溃只会留下不可检索的暂存
    /// 块，续跑会跳过它们，绝不会暴露半份文档或重复正式块。
    pub(super) fn ingest_import_item(
        &self,
        job_id: &str,
        item_id: i64,
        collection_id: i64,
        path: &Path,
        cancel: &AtomicBool,
    ) -> ImportIngestOutcome {
        let path_str = path.to_string_lossy().to_string();
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("(unnamed)")
            .to_string();
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_lowercase());
        let (size, mtime) = match std::fs::metadata(path) {
            Ok(m) => (
                m.len() as i64,
                m.modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
            ),
            Err(e) => return ImportIngestOutcome::Failed(format!("读取文件信息失败: {e}")),
        };
        if cancel.load(Ordering::Relaxed) {
            return ImportIngestOutcome::Cancelled;
        }

        let res = crate::features::files::file_ingest::ingest(path);
        let warning = res.warning.clone();
        let body = match res.markdown {
            Some(md) if !md.trim().is_empty() => Some(md),
            _ if res.kind == "image" => crate::features::files::file_ingest::ocr_image_for_kb(path),
            _ => None,
        };
        let Some(markdown) = body.filter(|v| !v.trim().is_empty()) else {
            let mut c = self.conn.lock();
            let result = c.transaction().and_then(|tx| {
                ensure_import_item_running(&tx, job_id, item_id)?;
                let doc_id = upsert_import_document(
                    &tx,
                    collection_id,
                    &path_str,
                    &name,
                    ext.as_deref(),
                    size,
                    mtime,
                )?;
                tx.execute("DELETE FROM knowledge_import_staged_chunks WHERE job_id=?1 AND item_id=?2", params![job_id, item_id])?;
                // 重导同一路径但新内容为空/不可解析时，旧正式块必须与 skipped 状态原子清除，
                // 否则检索仍会命中已经不存在的旧内容。
                tx.execute("DELETE FROM chunks WHERE document_id=?1", params![doc_id])?;
                tx.execute("UPDATE documents SET parse_status='skipped',n_chunks=0,parsed_at=?2 WHERE id=?1", params![doc_id, now()])?;
                tx.execute(
                    "UPDATE knowledge_import_items SET state='skipped',error=?3,total_chunks=0,completed_chunks=0,updated_at=?4 WHERE id=?1 AND job_id=?2 AND state='running'",
                    params![item_id, job_id, warning, now()],
                )?;
                tx.commit()
            });
            return match result {
                Ok(()) => ImportIngestOutcome::Skipped,
                Err(rusqlite::Error::QueryReturnedNoRows) => ImportIngestOutcome::Cancelled,
                Err(e) => ImportIngestOutcome::Failed(format!("保存跳过状态失败: {e}")),
            };
        };
        let chunks = chunk_text(&markdown, CHUNK_CHARS, CHUNK_OVERLAP);
        let content_hash = {
            let mut hasher = Sha256::new();
            for chunk in &chunks {
                hasher.update((chunk.len() as u64).to_le_bytes());
                hasher.update(chunk.as_bytes());
            }
            crate::platform::encoding::hex_lower(&hasher.finalize())
        };

        // 源内容变化时丢弃旧检查点；内容相同则保留已经完成的分块。
        {
            let mut c = self.conn.lock();
            let result = c.transaction().and_then(|tx| {
                let previous: Option<String> = tx.query_row(
                    "SELECT i.content_hash FROM knowledge_import_items i \
                     JOIN knowledge_import_jobs j ON j.id=i.job_id \
                     WHERE i.id=?1 AND i.job_id=?2 AND i.state='running' AND j.state='running'",
                    params![item_id, job_id],
                    |r| r.get(0),
                )?;
                if previous.as_deref() != Some(content_hash.as_str()) {
                    tx.execute("DELETE FROM knowledge_import_staged_chunks WHERE job_id=?1 AND item_id=?2", params![job_id, item_id])?;
                }
                let staged: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM knowledge_import_staged_chunks WHERE job_id=?1 AND item_id=?2",
                    params![job_id, item_id],
                    |r| r.get(0),
                )?;
                tx.execute(
                    "UPDATE knowledge_import_items SET content_hash=?3,total_chunks=?4,completed_chunks=?5,updated_at=?6 WHERE id=?1 AND job_id=?2",
                    params![item_id, job_id, content_hash, chunks.len() as i64, staged, now()],
                )?;
                tx.commit()
            });
            if let Err(e) = result {
                if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                    return ImportIngestOutcome::Cancelled;
                }
                return ImportIngestOutcome::Failed(format!("初始化分块检查点失败: {e}"));
            }
        }

        let (staged_ords, staged_has_null): (std::collections::HashSet<usize>, bool) = {
            let c = self.conn.lock();
            let mut stmt = match c.prepare(
                "SELECT ord FROM knowledge_import_staged_chunks WHERE job_id=?1 AND item_id=?2",
            ) {
                Ok(v) => v,
                Err(e) => return ImportIngestOutcome::Failed(e.to_string()),
            };
            let ords = match stmt.query_map(params![job_id, item_id], |r| r.get::<_, i64>(0)) {
                Ok(rows) => match rows.collect::<rusqlite::Result<Vec<_>>>() {
                    Ok(v) => v.into_iter().map(|n| n as usize).collect(),
                    Err(e) => return ImportIngestOutcome::Failed(e.to_string()),
                },
                Err(e) => return ImportIngestOutcome::Failed(e.to_string()),
            };
            let has_null: bool = c
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM knowledge_import_staged_chunks \
                     WHERE job_id=?1 AND item_id=?2 AND vec IS NULL)",
                    params![job_id, item_id],
                    |r| r.get(0),
                )
                .unwrap_or(true);
            (ords, has_null)
        };
        let missing: Vec<usize> = (0..chunks.len())
            .filter(|ord| !staged_ords.contains(ord))
            .collect();
        // 同一文档保持“全向量或全全文”语义。若上次某批已降级为 NULL，续跑不能再形成
        // 一半有向量、一半无向量的文档；模型未就绪或超限时也清除既有暂存向量。
        let mut use_vectors = if staged_has_null {
            None
        } else {
            self.embedder().filter(|_| chunks.len() <= MAX_EMBED_CHUNKS)
        };
        if use_vectors.is_none() && !staged_ords.is_empty() {
            let _ = self.conn.lock().execute(
                "UPDATE knowledge_import_staged_chunks SET vec=NULL WHERE job_id=?1 AND item_id=?2",
                params![job_id, item_id],
            );
        }

        for ord_batch in missing.chunks(EMBED_BATCH) {
            if cancel.load(Ordering::Relaxed) {
                return ImportIngestOutcome::Cancelled;
            }
            let texts: Vec<String> = ord_batch.iter().map(|ord| chunks[*ord].clone()).collect();
            let vectors = if let Some(emb) = &use_vectors {
                match emb.embed(&texts) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        eprintln!("[knowledge] embedding 批失败，该文档降级仅全文: {e}");
                        use_vectors = None;
                        let _ = self.conn.lock().execute(
                            "UPDATE knowledge_import_staged_chunks SET vec=NULL WHERE job_id=?1 AND item_id=?2",
                            params![job_id, item_id],
                        );
                        None
                    }
                }
            } else {
                None
            };
            #[cfg(test)]
            self.wait_at_checkpoint_gate();
            let mut c = self.conn.lock();
            let result = c.transaction().and_then(|tx| {
                ensure_import_item_running(&tx, job_id, item_id)?;
                {
                    let mut stmt = tx.prepare_cached(
                        "INSERT OR REPLACE INTO knowledge_import_staged_chunks(job_id,item_id,ord,text,n_tokens,vec) VALUES(?1,?2,?3,?4,?5,?6)",
                    )?;
                    for (batch_pos, ord) in ord_batch.iter().enumerate() {
                        let text = &chunks[*ord];
                        let vec = vectors
                            .as_ref()
                            .and_then(|all| all.get(batch_pos))
                            .map(|v| embed::vec_to_blob(v));
                        stmt.execute(params![job_id, item_id, *ord as i64, text, text.chars().count() as i64, vec])?;
                    }
                }
                let completed: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM knowledge_import_staged_chunks WHERE job_id=?1 AND item_id=?2",
                    params![job_id, item_id],
                    |r| r.get(0),
                )?;
                tx.execute(
                    "UPDATE knowledge_import_items SET completed_chunks=?2,updated_at=?3 WHERE id=?1",
                    params![item_id, completed, now()],
                )?;
                tx.commit()
            });
            if let Err(e) = result {
                if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                    return ImportIngestOutcome::Cancelled;
                }
                return ImportIngestOutcome::Failed(format!("保存分块检查点失败: {e}"));
            }
            std::thread::sleep(Duration::from_millis(2));
        }

        if cancel.load(Ordering::Relaxed) {
            return ImportIngestOutcome::Cancelled;
        }
        let mut c = self.conn.lock();
        let result = c.transaction().and_then(|tx| {
            ensure_import_item_running(&tx, job_id, item_id)?;
            let staged: i64 = tx.query_row(
                "SELECT COUNT(*) FROM knowledge_import_staged_chunks WHERE job_id=?1 AND item_id=?2",
                params![job_id, item_id],
                |r| r.get(0),
            )?;
            if staged != chunks.len() as i64 {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let doc_id = upsert_import_document(
                &tx,
                collection_id,
                &path_str,
                &name,
                ext.as_deref(),
                size,
                mtime,
            )?;
            tx.execute("DELETE FROM chunks WHERE document_id=?1", params![doc_id])?;
            tx.execute(
                "INSERT INTO chunks(document_id,collection_id,ord,text,n_tokens,vec) \
                 SELECT ?3,?4,ord,text,n_tokens,vec FROM knowledge_import_staged_chunks \
                 WHERE job_id=?1 AND item_id=?2 ORDER BY ord",
                params![job_id, item_id, doc_id, collection_id],
            )?;
            tx.execute(
                "UPDATE documents SET parse_status='parsed',n_chunks=?2,parsed_at=?3 WHERE id=?1",
                params![doc_id, chunks.len() as i64, now()],
            )?;
            tx.execute(
                "UPDATE knowledge_import_items SET state='completed',completed_chunks=total_chunks,error=NULL,updated_at=?2 WHERE id=?1",
                params![item_id, now()],
            )?;
            tx.execute(
                "DELETE FROM knowledge_import_staged_chunks WHERE job_id=?1 AND item_id=?2",
                params![job_id, item_id],
            )?;
            tx.commit()
        });
        match result {
            Ok(()) => ImportIngestOutcome::Completed,
            Err(rusqlite::Error::QueryReturnedNoRows) => ImportIngestOutcome::Cancelled,
            Err(e) => ImportIngestOutcome::Failed(format!("提交文档失败: {e}")),
        }
    }

    /// 算分块向量（配了 embedding 才算）。**大文档保护**：块数超 `MAX_EMBED_CHUNKS` 直接跳过
    /// 向量化（仅全文检索），避免上千块在 CPU 上一次性 embedding 把入库卡死（5000 行表格 ≈ 1845
    /// 块的实测卡死根因）。块数内则**分批** embedding，批间让步，不长时间独占模型锁/CPU。
    /// 无 embedder / 任一批失败 → None（降级仅全文，不阻断入库）。
    fn embed_chunks_bounded(&self, chunks: &[String]) -> Option<Vec<Vec<f32>>> {
        let emb = self.embedder()?;
        if chunks.len() > MAX_EMBED_CHUNKS {
            eprintln!(
                "[knowledge] 文档块数 {} 超向量化上限 {}，跳过向量、仅全文检索（关键词可命中）",
                chunks.len(),
                MAX_EMBED_CHUNKS
            );
            return None;
        }
        let mut out = Vec::with_capacity(chunks.len());
        for batch in chunks.chunks(EMBED_BATCH) {
            match emb.embed(batch) {
                Ok(mut v) => out.append(&mut v),
                Err(e) => {
                    eprintln!("[knowledge] embedding 批失败，该文档降级仅全文: {e}");
                    return None;
                }
            }
            std::thread::sleep(Duration::from_millis(2)); // 让步：别长时间霸占 CPU/模型锁
        }
        Some(out)
    }

    // ───────────────────────── 检索（全文，Phase 3 升级为混合） ─────────────────────────

    /// 全文：≥3 字符走 FTS5 trigram(bm25)，1-2 字符 LIKE 兜底。
    fn search_fts(
        &self,
        collection_id: i64,
        q: &str,
        lim: usize,
    ) -> rusqlite::Result<Vec<ChunkHit>> {
        let c = self.conn.lock();
        let map = |r: &rusqlite::Row| -> rusqlite::Result<ChunkHit> {
            Ok(ChunkHit {
                document_id: r.get(0)?,
                text: r.get(1)?,
                score: r.get(2)?,
                doc_name: r.get(3)?,
                doc_path: r.get(4)?,
                ord: r.get(5)?,
            })
        };
        if q.chars().count() >= 3 {
            let m = format!("\"{}\"", q.replace('"', "\"\""));
            let mut stmt = c.prepare(
                "SELECT d.id,k.text,bm25(chunks_fts) AS score,d.name,d.path,k.ord \
                 FROM chunks_fts JOIN chunks k ON k.id=chunks_fts.rowid \
                 JOIN documents d ON d.id=k.document_id \
                 WHERE k.collection_id=?1 AND chunks_fts MATCH ?2 \
                 ORDER BY score, d.path, k.ord, d.id LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![collection_id, m, lim as i64], map)?;
            rows.collect()
        } else {
            let like = format!("%{}%", q.replace(['%', '_'], ""));
            let mut stmt = c.prepare(
                "SELECT d.id,k.text,0.0 AS score,d.name,d.path,k.ord \
                 FROM chunks k JOIN documents d ON d.id=k.document_id \
                 WHERE k.collection_id=?1 AND k.text LIKE ?2 \
                 ORDER BY d.path, k.ord, d.id LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![collection_id, like, lim as i64], map)?;
            rows.collect()
        }
    }

    /// 向量召回：加载该集所有有 vec 的 chunk，暴力算余弦，取 top（选定知识集规模下足够）。
    fn search_vec(
        &self,
        collection_id: i64,
        qv: &[f32],
        lim: usize,
    ) -> rusqlite::Result<Vec<ChunkHit>> {
        let c = self.conn.lock();
        let mut stmt = c.prepare(
            "SELECT d.id,k.text,k.vec,d.name,d.path,k.ord \
             FROM chunks k JOIN documents d ON d.id=k.document_id \
             WHERE k.collection_id=?1 AND k.vec IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![collection_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?;
        let mut scored: Vec<(f32, ChunkHit)> = Vec::new();
        for row in rows {
            let (document_id, text, blob, doc_name, doc_path, ord) = row?;
            let s = embed::cosine(qv, &embed::blob_to_vec(&blob));
            scored.push((
                s,
                ChunkHit {
                    document_id,
                    text,
                    score: s as f64,
                    doc_name,
                    doc_path,
                    ord,
                },
            ));
        }
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.doc_path.cmp(&b.1.doc_path))
                .then_with(|| a.1.ord.cmp(&b.1.ord))
                .then_with(|| a.1.document_id.cmp(&b.1.document_id))
        });
        Ok(scored.into_iter().take(lim).map(|(_, h)| h).collect())
    }

    /// 对话注入专用检索（区别于通用 `search`：知识库页主动检索不该门控/聚合，仍用 `search`）。
    /// 在混合检索基础上加两层处理：
    /// 1. **相关性门控**：配了 embedding 时，向量 top 余弦低于 [`RELEVANCE_MIN_COSINE`] 且
    ///    FTS 也无命中 → 判定与知识集无关，返回空（调用方据此不注入）。纯 FTS 降级模式无
    ///    统一阈值，维持"有命中即返回"。
    /// 2. **邻域扩展**：命中 chunk 按文档聚合，各取 ord±`neighbor_radius` 的相邻块拼成连续
    ///    上下文（答案常跨多个相邻 chunk，只给命中块易缺信息）。数据全在库内，纯 SQL，
    ///    不重读原文件（原文件多是二进制，read_file 也读不出）。
    pub fn retrieve_for_chat(
        &self,
        collection_id: i64,
        query: &str,
        k: usize,
        neighbor_radius: usize,
    ) -> rusqlite::Result<Vec<ChunkHit>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(vec![]);
        }
        let query_vector = self
            .embedder()
            .and_then(|embedder| embedder.embed_one(q).ok());
        self.retrieve_for_chat_with_vector(
            collection_id,
            q,
            k,
            neighbor_radius,
            query_vector.as_deref(),
        )
    }

    /// 跨多个知识集检索。查询向量只计算一次；每个知识集独立召回后按库内混合检索分稳定归并，
    /// 同一路径、同一 chunk 且正文相同的结果只保留一份，并记录其全部知识集来源；
    /// 同路径的冲突正文分别保留，避免去重掩盖版本差异。
    pub fn retrieve_for_chat_multi(
        &self,
        collection_ids: &[i64],
        query: &str,
        k: usize,
        neighbor_radius: usize,
    ) -> rusqlite::Result<Vec<ScopedChunkHit>> {
        let q = query.trim();
        if q.is_empty() || collection_ids.is_empty() {
            return Ok(Vec::new());
        }
        let lim = if k == 0 { 5 } else { k };
        let query_vector = self
            .embedder()
            .and_then(|embedder| embedder.embed_one(q).ok());
        let mut unique_ids = Vec::new();
        for collection_id in collection_ids.iter().copied().filter(|id| *id > 0) {
            if !unique_ids.contains(&collection_id) {
                unique_ids.push(collection_id);
            }
        }

        let mut candidates = Vec::new();
        for (collection_order, collection_id) in unique_ids.into_iter().enumerate() {
            let hits = self.retrieve_for_chat_with_vector(
                collection_id,
                q,
                lim,
                neighbor_radius,
                query_vector.as_deref(),
            )?;
            for (rank, hit) in hits.into_iter().enumerate() {
                let score = if query_vector.is_some() {
                    hit.score
                } else {
                    1.0 / (60.0 + rank as f64 + 1.0)
                };
                candidates.push((score, collection_order, rank, collection_id, hit));
            }
        }
        candidates.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
                .then_with(|| a.4.doc_path.cmp(&b.4.doc_path))
                .then_with(|| a.4.ord.cmp(&b.4.ord))
        });

        let mut merged: Vec<ScopedChunkHit> = Vec::new();
        let mut positions: HashMap<(String, i64, String), usize> = HashMap::new();
        for (_, _, _, collection_id, hit) in candidates {
            let source_key = (
                dedupe_path_key(&hit.doc_path),
                hit.ord,
                hit.text.replace("\r\n", "\n").trim().to_string(),
            );
            if let Some(index) = positions.get(&source_key).copied() {
                let collection_ids = &mut merged[index].collection_ids;
                if !collection_ids.contains(&collection_id) {
                    collection_ids.push(collection_id);
                }
                continue;
            }
            positions.insert(source_key, merged.len());
            merged.push(ScopedChunkHit {
                collection_ids: vec![collection_id],
                hit,
            });
        }
        merged.truncate(lim);
        Ok(merged)
    }

    fn retrieve_for_chat_with_vector(
        &self,
        collection_id: i64,
        q: &str,
        k: usize,
        neighbor_radius: usize,
        query_vector: Option<&[f32]>,
    ) -> rusqlite::Result<Vec<ChunkHit>> {
        let lim = if k == 0 { 5 } else { k };
        let fts = self.search_fts(collection_id, q, lim * 2)?;
        let ranked = if let Some(query_vector) = query_vector {
            let vec = self.search_vec(collection_id, query_vector, lim * 2)?;
            // search_vec 的 score 此刻是余弦（rrf_merge 之后才被改写成 RRF 分）。
            let top_cos = vec.first().map(|h| h.score).unwrap_or(f64::NEG_INFINITY);
            if top_cos < RELEVANCE_MIN_COSINE && fts.is_empty() {
                return Ok(vec![]); // 门控：既无语义相关也无关键词命中
            }
            rrf_merge(fts, vec, lim)
        } else {
            fts.into_iter().take(lim).collect()
        };
        if ranked.is_empty() || neighbor_radius == 0 {
            return Ok(ranked);
        }
        self.expand_neighbors(collection_id, ranked, neighbor_radius)
    }

    /// 命中按文档聚合，各文档取其命中 ord 的 ±radius 邻域并集，从库里拉这些 chunk 按 ord
    /// 升序拼成连续上下文。文档间保持相关性排序（按各文档最高命中分），不连续的 ord 区间
    /// 之间插 `…` 断档标记。切块本身有 ~15% 重叠，拼接处少量重复无伤注入，不额外去重。
    fn expand_neighbors(
        &self,
        collection_id: i64,
        hits: Vec<ChunkHit>,
        radius: usize,
    ) -> rusqlite::Result<Vec<ChunkHit>> {
        // path -> (最高分, doc_name, 命中 ord 列表)；order 记录首次出现顺序以保相关性序。
        let mut order: Vec<String> = Vec::new();
        let mut by_doc: HashMap<String, (i64, f64, String, Vec<i64>)> = HashMap::new();
        for h in hits {
            let e = by_doc.entry(h.doc_path.clone()).or_insert_with(|| {
                order.push(h.doc_path.clone());
                (
                    h.document_id,
                    f64::NEG_INFINITY,
                    h.doc_name.clone(),
                    Vec::new(),
                )
            });
            if h.score > e.1 {
                e.1 = h.score;
            }
            e.3.push(h.ord);
        }
        let c = self.conn.lock();
        let mut stmt = c.prepare(
            "SELECT k.ord, k.text FROM chunks k JOIN documents d ON d.id=k.document_id \
             WHERE k.collection_id=?1 AND d.path=?2 AND k.ord BETWEEN ?3 AND ?4 ORDER BY k.ord",
        )?;
        let mut out: Vec<ChunkHit> = Vec::new();
        for path in order {
            let (document_id, best, doc_name, ords) = by_doc.remove(&path).expect("aggregated");
            // 命中 ord 各自 ±radius 的并集（去重、有序）。
            let mut want: BTreeSet<i64> = BTreeSet::new();
            for o in ords {
                let lo = (o - radius as i64).max(0);
                for x in lo..=(o + radius as i64) {
                    want.insert(x);
                }
            }
            let lo = *want.iter().next().unwrap();
            let hi = *want.iter().last().unwrap();
            let rows = stmt.query_map(params![collection_id, path, lo, hi], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?;
            let mut text = String::new();
            let mut start_ord = lo;
            let mut prev: Option<i64> = None;
            for row in rows {
                let (ord, t) = row?;
                if !want.contains(&ord) {
                    continue; // 落在 [lo,hi] 但不在并集内（多命中区间之间的空档）
                }
                match prev {
                    None => start_ord = ord,
                    Some(p) if ord > p + 1 => text.push_str("\n…\n"), // 区间断档
                    Some(_) => text.push('\n'),
                }
                text.push_str(t.trim());
                prev = Some(ord);
            }
            if !text.trim().is_empty() {
                out.push(ChunkHit {
                    document_id,
                    text,
                    score: best,
                    doc_name,
                    doc_path: path,
                    ord: start_ord,
                });
            }
        }
        out.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.doc_path.cmp(&b.doc_path))
                .then_with(|| a.ord.cmp(&b.ord))
                .then_with(|| a.document_id.cmp(&b.document_id))
        });
        Ok(out)
    }
}

fn ensure_import_item_running(
    tx: &rusqlite::Transaction<'_>,
    job_id: &str,
    item_id: i64,
) -> rusqlite::Result<()> {
    let running: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM knowledge_import_items i \
         JOIN knowledge_import_jobs j ON j.id=i.job_id \
         WHERE i.id=?2 AND i.job_id=?1 AND i.state='running' AND j.state='running')",
        params![job_id, item_id],
        |r| r.get(0),
    )?;
    if running {
        Ok(())
    } else {
        Err(rusqlite::Error::QueryReturnedNoRows)
    }
}

/// 可恢复导入只在最终提交事务里创建/更新文档元数据。这样取消新文件不会留下 pending
/// 文档，取消重导也不会提前改动既有文档状态或正式 chunks。
fn upsert_import_document(
    tx: &rusqlite::Transaction<'_>,
    collection_id: i64,
    path: &str,
    name: &str,
    ext: Option<&str>,
    size: i64,
    mtime: i64,
) -> rusqlite::Result<i64> {
    tx.execute(
        "INSERT INTO documents(collection_id,path,name,ext,size,mtime,parse_status,parsed_at) \
         SELECT ?1,?2,?3,?4,?5,?6,'pending',0 \
         WHERE EXISTS(SELECT 1 FROM collections WHERE id=?1) \
         ON CONFLICT(collection_id,path) DO UPDATE SET \
           name=excluded.name,ext=excluded.ext,size=excluded.size,mtime=excluded.mtime",
        params![collection_id, path, name, ext, size, mtime],
    )?;
    tx.query_row(
        "SELECT id FROM documents WHERE collection_id=?1 AND path=?2",
        params![collection_id, path],
        |r| r.get(0),
    )
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn dedupe_path_key(path: &str) -> String {
    crate::platform::os::filesystem_path_identity_key(path)
}

/// 倒数排名融合(RRF)：两路结果按排名给分，按 (docPath#ord) 去重合并，取前 k。
fn rrf_merge(fts: Vec<ChunkHit>, vec: Vec<ChunkHit>, k: usize) -> Vec<ChunkHit> {
    const RRF_K: f64 = 60.0;
    let mut score: HashMap<String, f64> = HashMap::new();
    let mut keep: HashMap<String, ChunkHit> = HashMap::new();
    for (rank, h) in fts.iter().enumerate() {
        let key = format!("{}#{}", dedupe_path_key(&h.doc_path), h.ord);
        *score.entry(key.clone()).or_insert(0.0) += 1.0 / (RRF_K + rank as f64 + 1.0);
        keep.entry(key).or_insert_with(|| h.clone());
    }
    for (rank, h) in vec.iter().enumerate() {
        let key = format!("{}#{}", dedupe_path_key(&h.doc_path), h.ord);
        *score.entry(key.clone()).or_insert(0.0) += 1.0 / (RRF_K + rank as f64 + 1.0);
        keep.entry(key).or_insert_with(|| h.clone());
    }
    let mut merged: Vec<ChunkHit> = keep
        .into_iter()
        .map(|(key, mut h)| {
            h.score = *score.get(&key).unwrap_or(&0.0);
            h
        })
        .collect();
    merged.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.doc_path.cmp(&b.doc_path))
            .then_with(|| a.ord.cmp(&b.ord))
            .then_with(|| a.document_id.cmp(&b.document_id))
    });
    merged.into_iter().take(k).collect()
}

/// 把正文切成 ~max_chars 的块，相邻块重叠 overlap 字符。按字符窗口滑动（中文友好）。
/// 短于一块的整体返回一块；空白块丢弃。
pub fn chunk_text(text: &str, max_chars: usize, overlap: usize) -> Vec<String> {
    pinvou_knowledge::chunk_text(text, max_chars, overlap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::knowledge::import_jobs::ImportJobStore;
    use crate::features::knowledge::store::Store;
    use std::fs;
    use std::thread;

    fn mem() -> L1Store {
        let store = Store::open_in_memory().unwrap();
        L1Store::new(store.conn_arc(), None) // 单测：纯全文,不接 embedding
    }

    #[test]
    fn chunk_overlap_and_short() {
        assert_eq!(chunk_text("  ", 10, 2), Vec::<String>::new());
        assert_eq!(chunk_text("短文本", 10, 2), vec!["短文本".to_string()]);
        let long: String = "甲乙丙丁戊己庚辛壬癸".chars().cycle().take(50).collect();
        let cs = chunk_text(&long, 20, 5);
        assert!(cs.len() >= 3, "50 字按 20/5 应切多块, got {}", cs.len());
        assert!(cs.iter().all(|c| c.chars().count() <= 20));
    }

    #[test]
    fn collection_crud() {
        let l1 = mem();
        let id = l1
            .create_collection("产品资料", Some("产品"), Some("PRD"))
            .unwrap();
        let list = l1.list_collections().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "产品资料");
        assert_eq!(list[0].doc_count, 0);
        l1.update_collection(id, "产品资料库", Some("产品"), None)
            .unwrap();
        assert_eq!(l1.list_collections().unwrap()[0].name, "产品资料库");
        l1.delete_collection(id).unwrap();
        assert!(l1.list_collections().unwrap().is_empty());
    }

    /// kb_search/kb_open_source 门控核心:has_any_document 反映"库里有没有文档"。空知识集不算;
    /// 删文档 / 删知识集后都应归 false —— 对应 kb_remove_document / kb_collection_delete
    /// 删后 refresh_kb_tool_gate 把两个知识工具加进 disallowed（库空就该隐藏）。
    #[test]
    fn has_any_document_reflects_emptiness() {
        let l1 = mem();
        assert!(!l1.has_any_document().unwrap(), "空库应为 false");

        let cid = l1.create_collection("库", None, None).unwrap();
        assert!(
            !l1.has_any_document().unwrap(),
            "空知识集（无文档）不算有内容"
        );

        let doc = l1
            .upsert_document(cid, "/tmp/a.md", "a.md", Some("md"), 10, 0)
            .unwrap();
        assert!(l1.has_any_document().unwrap(), "入库一篇后应为 true");

        l1.remove_document(doc).unwrap();
        assert!(!l1.has_any_document().unwrap(), "删光文档后应回到 false");

        // 删整个知识集这条路径:连带清文档,也应归 false
        l1.upsert_document(cid, "/tmp/b.md", "b.md", Some("md"), 10, 0)
            .unwrap();
        assert!(l1.has_any_document().unwrap());
        l1.delete_collection(cid).unwrap();
        assert!(!l1.has_any_document().unwrap(), "删知识集应连带清空文档");
    }

    #[test]
    fn document_window_is_scoped_to_collection_and_pageable() {
        let l1 = mem();
        let allowed = l1.create_collection("已挂载", None, None).unwrap();
        let other = l1.create_collection("未挂载", None, None).unwrap();
        let doc = l1
            .upsert_document(
                allowed,
                "/tmp/report.xlsx",
                "report.xlsx",
                Some("xlsx"),
                10,
                0,
            )
            .unwrap();
        let chunks = ["chunk-0", "chunk-1", "chunk-2", "chunk-3"].map(str::to_string);
        l1.replace_doc_chunks(doc, allowed, &chunks, None).unwrap();
        l1.set_doc_status(doc, "parsed", chunks.len());

        let metadata = l1
            .document_in_collection(allowed, doc)
            .unwrap()
            .expect("document in mounted collection");
        assert_eq!(metadata.id, doc);
        assert_eq!(metadata.n_chunks, 4);
        assert!(
            l1.document_in_collection(other, doc).unwrap().is_none(),
            "same document id must not cross collection boundary"
        );

        let page = l1.document_chunk_window(allowed, doc, 1, 2).unwrap();
        assert_eq!(
            page,
            vec![(1, "chunk-1".to_string()), (2, "chunk-2".to_string())]
        );
        assert!(l1
            .document_chunk_window(other, doc, 0, 8)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn multi_collection_retrieval_deduplicates_same_source_and_keeps_mount_order() {
        let l1 = mem();
        let first = l1.create_collection("项目资料", None, None).unwrap();
        let second = l1.create_collection("团队规范", None, None).unwrap();
        let conflicting = l1.create_collection("历史版本", None, None).unwrap();
        for collection_id in [first, second] {
            let document_id = l1
                .upsert_document(
                    collection_id,
                    "/tmp/shared.md",
                    "shared.md",
                    Some("md"),
                    10,
                    0,
                )
                .unwrap();
            l1.replace_doc_chunks(
                document_id,
                collection_id,
                &["shared knowledge answer".to_string()],
                None,
            )
            .unwrap();
        }
        let conflicting_document_id = l1
            .upsert_document(
                conflicting,
                "/tmp/shared.md",
                "shared.md",
                Some("md"),
                10,
                0,
            )
            .unwrap();
        l1.replace_doc_chunks(
            conflicting_document_id,
            conflicting,
            &["shared knowledge conflicting answer".to_string()],
            None,
        )
        .unwrap();

        let hits = l1
            .retrieve_for_chat_multi(&[second, first, conflicting, second], "shared", 5, 0)
            .unwrap();
        assert_eq!(hits.len(), 2, "相同正文去重，但同路径冲突正文必须保留");
        assert_eq!(hits[0].collection_ids, vec![second, first]);
        assert_eq!(hits[0].hit.doc_path, "/tmp/shared.md");
        assert_eq!(hits[1].collection_ids, vec![conflicting]);
        assert!(hits[1].hit.text.contains("conflicting"));
    }

    #[test]
    fn rrf_equal_scores_use_stable_source_order() {
        let hit = |document_id: i64, path: &str| ChunkHit {
            document_id,
            text: path.to_string(),
            score: 0.0,
            doc_name: path.to_string(),
            doc_path: path.to_string(),
            ord: 0,
        };
        // 两条分别只在一路排第一，RRF 分数完全相同；结果不能依赖 HashMap 随机种子。
        let merged = rrf_merge(vec![hit(2, "/tmp/b.md")], vec![hit(1, "/tmp/a.md")], 2);
        assert_eq!(
            merged
                .iter()
                .map(|item| item.doc_path.as_str())
                .collect::<Vec<_>>(),
            vec!["/tmp/a.md", "/tmp/b.md"]
        );
    }

    #[test]
    fn rrf_deduplicates_using_platform_path_identity() {
        let hit = |document_id: i64, path: &str| ChunkHit {
            document_id,
            text: path.to_string(),
            score: 0.0,
            doc_name: path.to_string(),
            doc_path: path.to_string(),
            ord: 0,
        };
        let candidates = [
            (r"C:\Docs\Guide.md", "c:/docs/guide.md"),
            ("/tmp/docs/guide.md", "/tmp/docs/guide.md"),
        ];
        let (first_path, second_path) = candidates
            .into_iter()
            .find(|(first, second)| dedupe_path_key(first) == dedupe_path_key(second))
            .expect("at least the identical-path fallback must share an identity");
        let merged = rrf_merge(vec![hit(1, first_path)], vec![hit(1, second_path)], 10);

        assert_eq!(merged.len(), 1);
        assert!(merged[0].score > 1.0 / 61.0);
    }

    #[test]
    fn equal_rank_inputs_use_stable_source_order() {
        let l1 = mem();
        let collection_id = l1.create_collection("stable", None, None).unwrap();
        for path in ["/tmp/b.md", "/tmp/a.md"] {
            let document_id = l1
                .upsert_document(collection_id, path, path, Some("md"), 10, 0)
                .unwrap();
            l1.replace_doc_chunks(
                document_id,
                collection_id,
                &["stable ranking term".to_string()],
                Some(&[vec![1.0, 0.0]]),
            )
            .unwrap();
        }

        for hits in [
            l1.search_fts(collection_id, "stable", 10).unwrap(),
            l1.search_fts(collection_id, "st", 10).unwrap(),
            l1.search_vec(collection_id, &[1.0, 0.0], 10).unwrap(),
        ] {
            assert_eq!(
                hits.iter()
                    .map(|hit| hit.doc_path.as_str())
                    .collect::<Vec<_>>(),
                vec!["/tmp/a.md", "/tmp/b.md"]
            );
        }
    }

    /// 热加载槽:set_embedder 换值,L1Store clone 共享同一槽(下载完成后所有在跑线程见新);
    /// 换槽不破坏纯全文检索通路。无法廉价构造真实 Embedder,故只验 None 语义 + 共享 + fts 存活。
    #[test]
    fn embedder_slot_swap_and_share() {
        let l1 = mem();
        assert!(!l1.has_embedder());
        let clone = l1.clone();
        l1.set_embedder(None); // 卸载(本就 None)
        assert!(!clone.has_embedder(), "clone 共享同一 embedder 槽");

        // 换槽后纯全文检索仍可用。
        let cid = l1.create_collection("库", None, None).unwrap();
        let doc = l1
            .upsert_document(cid, "/tmp/x.md", "x.md", Some("md"), 10, 0)
            .unwrap();
        l1.replace_doc_chunks(doc, cid, &["语义检索测试段落".to_string()], None)
            .unwrap();
        assert!(!clone
            .retrieve_for_chat(cid, "语义检索", 5, 0)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn ingest_and_search() {
        let l1 = mem();
        let cid = l1.create_collection("调研", None, None).unwrap();
        let dir = std::env::temp_dir().join(format!("pinvou3_l1_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("访谈纪要.md");
        fs::write(&f, "# 用户访谈\n受访者认为保险报价流程过于繁琐，希望一键比价。\n竞品在交强险环节体验更顺畅。").unwrap();

        let st = l1.ingest_file(cid, &f);
        assert_eq!(st, "parsed");
        let docs = l1.list_documents(cid, 0).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].parse_status, "parsed");
        assert!(docs[0].n_chunks >= 1);

        // FTS（≥3 字符）—— 走对话检索通路(retrieve_for_chat,半径0)
        let hits = l1.retrieve_for_chat(cid, "交强险", 10, 0).unwrap();
        assert!(!hits.is_empty(), "应检索到含'交强险'的块");
        assert!(hits[0].text.contains("交强险"));
        assert_eq!(hits[0].doc_name, "访谈纪要.md");

        // 知识集计数更新
        let coll = &l1.list_collections().unwrap()[0];
        assert_eq!(coll.doc_count, 1);
        assert!(coll.chunk_count >= 1);

        let _ = fs::remove_dir_all(&dir);
    }

    fn prepare_import(
        l1: &L1Store,
        collection_id: i64,
        file: &Path,
    ) -> (ImportJobStore, String, i64) {
        let jobs = ImportJobStore::new(l1.conn.clone());
        let job_id = jobs.create(collection_id, &[file.to_path_buf()]).unwrap();
        jobs.prepare_items(&job_id, &[file.to_path_buf()]).unwrap();
        let item = jobs.claim_next(&job_id).unwrap().unwrap();
        (jobs, job_id, item.id)
    }

    #[test]
    fn cancel_while_embedding_cannot_write_a_late_checkpoint() {
        let l1 = mem();
        let collection_id = l1.create_collection("并发取消", None, None).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "pinvou3_import_cancel_{}_{}",
            std::process::id(),
            now()
        ));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("cancel.md");
        fs::write(&file, "需要在取消后保持不可见的导入内容").unwrap();
        let (jobs, job_id, item_id) = prepare_import(&l1, collection_id, &file);
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        l1.set_checkpoint_gate(entered.clone(), release.clone());
        let worker_l1 = l1.clone();
        let worker_job = job_id.clone();
        let worker_file = file.clone();
        let worker = thread::spawn(move || {
            worker_l1.ingest_import_item(
                &worker_job,
                item_id,
                collection_id,
                &worker_file,
                &AtomicBool::new(false),
            )
        });

        entered.wait();
        jobs.cancel(&job_id).unwrap();
        release.wait();
        assert!(matches!(
            worker.join().unwrap(),
            ImportIngestOutcome::Cancelled
        ));
        let c = l1.conn.lock();
        let state: String = c
            .query_row(
                "SELECT state FROM knowledge_import_items WHERE id=?1",
                params![item_id],
                |r| r.get(0),
            )
            .unwrap();
        let staged: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM knowledge_import_staged_chunks WHERE item_id=?1",
                params![item_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "cancelled");
        assert_eq!(staged, 0);
        let documents: i64 = c
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(documents, 0, "取消新文件不能遗留 pending 文档");
        drop(c);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn delete_collection_while_embedding_cannot_recreate_import_rows() {
        let l1 = mem();
        let collection_id = l1.create_collection("并发删除", None, None).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "pinvou3_import_delete_{}_{}",
            std::process::id(),
            now()
        ));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("delete.md");
        fs::write(&file, "知识集删除后不能被后台结果复活").unwrap();
        let (_jobs, job_id, item_id) = prepare_import(&l1, collection_id, &file);
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        l1.set_checkpoint_gate(entered.clone(), release.clone());
        let worker_l1 = l1.clone();
        let worker_job = job_id.clone();
        let worker_file = file.clone();
        let worker = thread::spawn(move || {
            worker_l1.ingest_import_item(
                &worker_job,
                item_id,
                collection_id,
                &worker_file,
                &AtomicBool::new(false),
            )
        });

        entered.wait();
        l1.delete_collection(collection_id).unwrap();
        release.wait();
        assert!(matches!(
            worker.join().unwrap(),
            ImportIngestOutcome::Cancelled
        ));
        let c = l1.conn.lock();
        for table in [
            "collections",
            "documents",
            "chunks",
            "knowledge_import_jobs",
            "knowledge_import_items",
            "knowledge_import_staged_chunks",
        ] {
            let count: i64 = c
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{table} 不应被晚到的 checkpoint 复活");
        }
        drop(c);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn reimporting_empty_content_removes_old_searchable_chunks() {
        let l1 = mem();
        let collection_id = l1.create_collection("空内容重导", None, None).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "pinvou3_import_empty_{}_{}",
            std::process::id(),
            now()
        ));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("same.md");
        fs::write(&file, "旧内容应该能够被检索命中").unwrap();
        let (_first_jobs, first_job, first_item) = prepare_import(&l1, collection_id, &file);
        assert!(matches!(
            l1.ingest_import_item(
                &first_job,
                first_item,
                collection_id,
                &file,
                &AtomicBool::new(false)
            ),
            ImportIngestOutcome::Completed
        ));
        assert!(!l1
            .retrieve_for_chat(collection_id, "旧内容", 5, 0)
            .unwrap()
            .is_empty());

        fs::write(&file, "   \n").unwrap();
        let (_second_jobs, second_job, second_item) = prepare_import(&l1, collection_id, &file);
        assert!(matches!(
            l1.ingest_import_item(
                &second_job,
                second_item,
                collection_id,
                &file,
                &AtomicBool::new(false)
            ),
            ImportIngestOutcome::Skipped
        ));
        assert!(l1
            .retrieve_for_chat(collection_id, "旧内容", 5, 0)
            .unwrap()
            .is_empty());
        let document = l1.list_documents(collection_id, 0).unwrap().pop().unwrap();
        assert_eq!(document.parse_status, "skipped");
        assert_eq!(document.n_chunks, 0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cancelling_reimport_preserves_existing_document_and_chunks() {
        let l1 = mem();
        let collection_id = l1.create_collection("取消重导", None, None).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "pinvou3_import_reimport_cancel_{}_{}",
            std::process::id(),
            now()
        ));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("existing.md");
        fs::write(&file, "旧正式内容必须保留").unwrap();
        assert_eq!(l1.ingest_file(collection_id, &file), "parsed");
        let before = l1.list_documents(collection_id, 0).unwrap().pop().unwrap();

        fs::write(&file, "取消后不能覆盖的全新内容").unwrap();
        let (jobs, job_id, item_id) = prepare_import(&l1, collection_id, &file);
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        l1.set_checkpoint_gate(entered.clone(), release.clone());
        let worker_l1 = l1.clone();
        let worker_job = job_id.clone();
        let worker_file = file.clone();
        let worker = thread::spawn(move || {
            worker_l1.ingest_import_item(
                &worker_job,
                item_id,
                collection_id,
                &worker_file,
                &AtomicBool::new(false),
            )
        });
        entered.wait();
        jobs.cancel(&job_id).unwrap();
        release.wait();
        assert!(matches!(
            worker.join().unwrap(),
            ImportIngestOutcome::Cancelled
        ));

        let after = l1.list_documents(collection_id, 0).unwrap().pop().unwrap();
        assert_eq!(after.id, before.id);
        assert_eq!(after.parse_status, before.parse_status);
        assert_eq!(after.n_chunks, before.n_chunks);
        assert!(!l1
            .retrieve_for_chat(collection_id, "旧正式内容", 5, 0)
            .unwrap()
            .is_empty());
        assert!(l1
            .retrieve_for_chat(collection_id, "全新内容", 5, 0)
            .unwrap()
            .is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn retrieve_for_chat_neighbor_and_gate() {
        let l1 = mem(); // 无 embedder → 纯 FTS,门控走"无命中即空"分支
        let cid = l1.create_collection("调研", None, None).unwrap();
        let doc = l1
            .upsert_document(cid, "/tmp/doc.md", "doc.md", Some("md"), 100, 0)
            .unwrap();
        let chunks: Vec<String> = vec![
            "第零段甲甲甲".into(),
            "第一段乙乙乙".into(),
            "第二段丙丙丙独有锚点词".into(),
            "第三段丁丁丁".into(),
            "第四段戊戊戊".into(),
        ];
        l1.replace_doc_chunks(doc, cid, &chunks, None).unwrap();

        // 命中 ord=2,radius=1 → 聚合成一个文档块,拼接 ord 1/2/3。
        let hits = l1.retrieve_for_chat(cid, "独有锚点词", 5, 1).unwrap();
        assert_eq!(hits.len(), 1, "同文档命中聚合成 1 块");
        let t = &hits[0].text;
        assert!(t.contains("第一段乙乙乙"), "带前邻 ord=1");
        assert!(t.contains("第二段丙丙丙"), "含命中 ord=2");
        assert!(t.contains("第三段丁丁丁"), "带后邻 ord=3");
        assert!(!t.contains("第零段"), "ord=0 在邻域外");
        assert!(!t.contains("第四段"), "ord=4 在邻域外");
        assert_eq!(hits[0].ord, 1, "起始 ord");

        // radius=0 → 不扩展,只给命中块。
        let only = l1.retrieve_for_chat(cid, "独有锚点词", 5, 0).unwrap();
        assert_eq!(only.len(), 1);
        assert!(only[0].text.contains("第二段丙丙丙"));
        assert!(!only[0].text.contains("第一段"), "radius=0 不带邻居");

        // 门控:无关键词命中 → 空(不注入)。空 query → 空。
        assert!(l1
            .retrieve_for_chat(cid, "彻底无关的查询词组", 5, 1)
            .unwrap()
            .is_empty());
        assert!(l1.retrieve_for_chat(cid, "   ", 5, 1).unwrap().is_empty());
    }
}
