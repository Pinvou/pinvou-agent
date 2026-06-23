//! L1 知识库：选定文件 → 解析(file_ingest) → 切块 → 入库 → 全文检索。
//!
//! 设计见 docs/本地文件与知识库-设计文档.html。本阶段做**全文版**(chunks_fts)，
//! `chunks.vec` 列留 NULL；向量(embedding)是 Phase 3，检索届时升级为 fts+向量混合。
//! 复用 L0 的同一个 SQLite 连接(同库 index.db，见 [`super::store::Store::conn_arc`])。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::Serialize;

use super::embed::{self, Embedder};

/// chunk 切块参数：~512 token ≈ 中文 600 字符；15% 重叠保留上下文。
const CHUNK_CHARS: usize = 600;
const CHUNK_OVERLAP: usize = 90;

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
    pub text: String,
    pub score: f64,
    pub doc_name: String,
    pub doc_path: String,
    pub ord: i64,
}

#[derive(Clone)]
pub struct L1Store {
    conn: Arc<Mutex<Connection>>,
    /// 配了 embedding 端点则启用向量;None → 纯全文 fts。
    embedder: Option<Arc<Embedder>>,
}

impl L1Store {
    pub fn new(conn: Arc<Mutex<Connection>>, embedder: Option<Arc<Embedder>>) -> Self {
        Self { conn, embedder }
    }

    #[allow(dead_code)] // 保留:将来检索/UI 判断是否启用向量
    pub fn has_embedder(&self) -> bool {
        self.embedder.is_some()
    }

    /// (source, model) — 给前端显示语义检索状态。
    pub fn embed_info(&self) -> Option<(String, String)> {
        self.embedder
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
        let c = self.conn.lock();
        c.execute("DELETE FROM chunks WHERE collection_id=?1", params![id])?;
        c.execute("DELETE FROM documents WHERE collection_id=?1", params![id])?;
        c.execute("DELETE FROM collections WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn set_collection_status(&self, id: i64, status: &str) {
        let _ = self.conn.lock().execute(
            "UPDATE collections SET status=?2,updated_at=?3 WHERE id=?1",
            params![id, status, now()],
        );
    }

    // ───────────────────────── 文档 ─────────────────────────

    /// 列出某知识集文档（collection_id<=0 则列出全部知识集的，按最近解析倒序，给"知识库内文件"表）。
    pub fn list_documents(&self, collection_id: i64, limit: usize) -> rusqlite::Result<Vec<Document>> {
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
             VALUES(?1,?2,?3,?4,?5,?6,'pending',0) \
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

        let doc_id =
            match self.upsert_document(collection_id, &path_str, &name, ext.as_deref(), size, mtime) {
                Ok(id) => id,
                Err(_) => return "failed".into(),
            };

        // 复用 file_ingest 解析正文（pdf/docx/md/xlsx/pptx/...）。
        let res = crate::file_ingest::ingest(path);
        match res.markdown {
            Some(md) if !md.trim().is_empty() => {
                let chunks = chunk_text(&md, CHUNK_CHARS, CHUNK_OVERLAP);
                let n = chunks.len();
                // 配了 embedding 端点则算向量(失败则降级为仅全文,不阻断入库)。
                let vecs = self.embedder.as_ref().and_then(|e| e.embed(&chunks).ok());
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

    // ───────────────────────── 检索（全文，Phase 3 升级为混合） ─────────────────────────

    /// 知识集内检索。配了 embedding → fts + 向量 RRF 混合;否则纯全文。
    /// 注意：调用方需保证在非 async 上下文（embedding 走 reqwest::blocking）。
    pub fn search(&self, collection_id: i64, query: &str, k: usize) -> rusqlite::Result<Vec<ChunkHit>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(vec![]);
        }
        let lim = if k == 0 { 20 } else { k };
        let fts = self.search_fts(collection_id, q, lim * 2)?;
        if let Some(emb) = &self.embedder {
            if let Ok(qv) = emb.embed_one(q) {
                let vec = self.search_vec(collection_id, &qv, lim * 2)?;
                return Ok(rrf_merge(fts, vec, lim));
            }
        }
        Ok(fts.into_iter().take(lim).collect())
    }

    /// 全文：≥3 字符走 FTS5 trigram(bm25)，1-2 字符 LIKE 兜底。
    fn search_fts(&self, collection_id: i64, q: &str, lim: usize) -> rusqlite::Result<Vec<ChunkHit>> {
        let c = self.conn.lock();
        let map = |r: &rusqlite::Row| -> rusqlite::Result<ChunkHit> {
            Ok(ChunkHit {
                text: r.get(0)?,
                score: r.get(1)?,
                doc_name: r.get(2)?,
                doc_path: r.get(3)?,
                ord: r.get(4)?,
            })
        };
        if q.chars().count() >= 3 {
            let m = format!("\"{}\"", q.replace('"', "\"\""));
            let mut stmt = c.prepare(
                "SELECT k.text, bm25(chunks_fts) AS score, d.name, d.path, k.ord \
                 FROM chunks_fts JOIN chunks k ON k.id=chunks_fts.rowid \
                 JOIN documents d ON d.id=k.document_id \
                 WHERE k.collection_id=?1 AND chunks_fts MATCH ?2 \
                 ORDER BY score LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![collection_id, m, lim as i64], map)?;
            rows.collect()
        } else {
            let like = format!("%{}%", q.replace('%', "").replace('_', ""));
            let mut stmt = c.prepare(
                "SELECT k.text, 0.0 AS score, d.name, d.path, k.ord \
                 FROM chunks k JOIN documents d ON d.id=k.document_id \
                 WHERE k.collection_id=?1 AND k.text LIKE ?2 LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![collection_id, like, lim as i64], map)?;
            rows.collect()
        }
    }

    /// 向量召回：加载该集所有有 vec 的 chunk，暴力算余弦，取 top（选定知识集规模下足够）。
    fn search_vec(&self, collection_id: i64, qv: &[f32], lim: usize) -> rusqlite::Result<Vec<ChunkHit>> {
        let c = self.conn.lock();
        let mut stmt = c.prepare(
            "SELECT k.text, k.vec, d.name, d.path, k.ord \
             FROM chunks k JOIN documents d ON d.id=k.document_id \
             WHERE k.collection_id=?1 AND k.vec IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![collection_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        let mut scored: Vec<(f32, ChunkHit)> = Vec::new();
        for row in rows {
            let (text, blob, doc_name, doc_path, ord) = row?;
            let s = embed::cosine(qv, &embed::blob_to_vec(&blob));
            scored.push((s, ChunkHit { text, score: s as f64, doc_name, doc_path, ord }));
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        Ok(scored.into_iter().take(lim).map(|(_, h)| h).collect())
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// 倒数排名融合(RRF)：两路结果按排名给分，按 (docPath#ord) 去重合并，取前 k。
fn rrf_merge(fts: Vec<ChunkHit>, vec: Vec<ChunkHit>, k: usize) -> Vec<ChunkHit> {
    const RRF_K: f64 = 60.0;
    let mut score: HashMap<String, f64> = HashMap::new();
    let mut keep: HashMap<String, ChunkHit> = HashMap::new();
    for (rank, h) in fts.iter().enumerate() {
        let key = format!("{}#{}", h.doc_path, h.ord);
        *score.entry(key.clone()).or_insert(0.0) += 1.0 / (RRF_K + rank as f64 + 1.0);
        keep.entry(key).or_insert_with(|| h.clone());
    }
    for (rank, h) in vec.iter().enumerate() {
        let key = format!("{}#{}", h.doc_path, h.ord);
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
    merged.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    merged.into_iter().take(k).collect()
}

/// 把正文切成 ~max_chars 的块，相邻块重叠 overlap 字符。按字符窗口滑动（中文友好）。
/// 短于一块的整体返回一块；空白块丢弃。
pub fn chunk_text(text: &str, max_chars: usize, overlap: usize) -> Vec<String> {
    let normalized = text.trim();
    if normalized.is_empty() {
        return vec![];
    }
    let chars: Vec<char> = normalized.chars().collect();
    if chars.len() <= max_chars {
        return vec![normalized.to_string()];
    }
    let step = max_chars.saturating_sub(overlap).max(1);
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let end = (i + max_chars).min(chars.len());
        let s: String = chars[i..end].iter().collect();
        let s = s.trim();
        if !s.is_empty() {
            out.push(s.to_string());
        }
        if end == chars.len() {
            break;
        }
        i += step;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::store::Store;
    use std::fs;

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
        let id = l1.create_collection("产品资料", Some("产品"), Some("PRD")).unwrap();
        let list = l1.list_collections().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "产品资料");
        assert_eq!(list[0].doc_count, 0);
        l1.update_collection(id, "产品资料库", Some("产品"), None).unwrap();
        assert_eq!(l1.list_collections().unwrap()[0].name, "产品资料库");
        l1.delete_collection(id).unwrap();
        assert!(l1.list_collections().unwrap().is_empty());
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

        // FTS（≥3 字符）
        let hits = l1.search(cid, "交强险", 10).unwrap();
        assert!(!hits.is_empty(), "应检索到含'交强险'的块");
        assert!(hits[0].text.contains("交强险"));
        assert_eq!(hits[0].doc_name, "访谈纪要.md");

        // 知识集计数更新
        let coll = &l1.list_collections().unwrap()[0];
        assert_eq!(coll.doc_count, 1);
        assert!(coll.chunk_count >= 1);

        let _ = fs::remove_dir_all(&dir);
    }
}
