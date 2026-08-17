//! Agentic RAG: `kb_search` 工具——AI 自主调用检索本会话挂载的知识集。
//!
//! 取代注入式(每条消息自动注入片段):由 LLM 自己决定何时查/查什么。工具 per-session
//! 构造(持 `session_id`),`execute` 时查该会话挂载的知识集,复用 [`L1Store::retrieve_for_chat`]。
//! 经底座 `EngineConfig.extra_tools` 注入(spawn_for_session)。配套 Self-RAG 自检引导见
//! `commands::build_kb_agentic_guide`。

use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use futures_util::future::join_all;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tokio::time::{timeout, Duration};

use deepseek_tui::tools::spec::{ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec};

use super::{l1::ScopedChunkHit, Document};
use crate::features::{
    knowledge::KnowledgeService, remote_knowledge::RemoteKnowledgeService, sessions::SessionStore,
};

/// 工具单次检索 top-K(精排;太多稀释小模型注意力)。
pub(crate) const KB_INJECT_TOP_K: usize = 5;
/// 邻域扩展半径,暂关(=0,只给命中块)。见 `L1Store::expand_neighbors`,改回 1 即恢复。
pub(crate) const KB_NEIGHBOR_RADIUS: usize = 0;
/// 返回片段总字符上限,超出截断(保证至少第一条)。~6K 字符 ≈ 5K token。
pub(crate) const KB_INJECT_MAX_CHARS: usize = 6_000;
/// `kb_open_source` 默认/最大返回 chunk 数。索引切块约 600 字符，8 块仍远低于单次
/// 工具结果预算，避免把整份工作簿重新灌入上下文。
const KB_OPEN_DEFAULT_CHUNKS: usize = 3;
const KB_OPEN_MAX_CHUNKS: usize = 8;
/// 单个远程来源不能无限拖住本地及其他健康来源。超时只降级该来源。
const KB_REMOTE_SOURCE_TIMEOUT: Duration = Duration::from_secs(12);

fn source_ref(document_id: i64, ord: i64) -> String {
    format!("kbdoc:{document_id}:chunk:{}", ord.max(0))
}

fn parse_source_ref(value: &str) -> Option<(i64, i64)> {
    let mut parts = value.trim().split(':');
    let prefix = parts.next()?;
    let document_id = parts.next()?.parse::<i64>().ok()?;
    let chunk_label = parts.next()?;
    let ord = parts.next()?.parse::<i64>().ok()?;
    if parts.next().is_some()
        || prefix != "kbdoc"
        || chunk_label != "chunk"
        || document_id <= 0
        || ord < 0
    {
        return None;
    }
    Some((document_id, ord))
}

fn remote_source_ref(server_id: &str, collection_id: i64, document_id: i64, ord: i64) -> String {
    format!(
        "kbremote:{}:{collection_id}:{document_id}:chunk:{}",
        URL_SAFE_NO_PAD.encode(server_id.as_bytes()),
        ord.max(0)
    )
}

fn parse_remote_source_ref(value: &str) -> Option<(String, i64, i64, i64)> {
    let mut parts = value.trim().split(':');
    if parts.next()? != "kbremote" {
        return None;
    }
    let server_id = String::from_utf8(URL_SAFE_NO_PAD.decode(parts.next()?).ok()?).ok()?;
    let collection_id = parts.next()?.parse::<i64>().ok()?;
    let document_id = parts.next()?.parse::<i64>().ok()?;
    if parts.next()? != "chunk" {
        return None;
    }
    let ord = parts.next()?.parse::<i64>().ok()?;
    if parts.next().is_some()
        || server_id.is_empty()
        || collection_id <= 0
        || document_id <= 0
        || ord < 0
    {
        return None;
    }
    Some((server_id, collection_id, document_id, ord))
}

struct UnifiedHit {
    collection_name: String,
    document_name: String,
    source_path: String,
    source_ref: String,
    text: String,
    score: f64,
}

fn source_rank_score(rank: usize) -> f64 {
    1.0 / (rank as f64 + 1.0)
}

fn build_unified_context_block(hits: &[UnifiedHit], warnings: &[String]) -> String {
    let mut out = "在本会话启用的本地与远程知识集中检索到以下相关片段(已统一排序)。请严格基于这些片段作答并注明来源文件；上下文不足时可再次调用 `kb_search`，或用 `source_ref` 调用 `kb_open_source`。\n\n".to_string();
    if !warnings.is_empty() {
        out.push_str("部分来源暂时不可用：");
        out.push_str(&warnings.join("；"));
        out.push_str("。以下结果仍可正常使用。\n\n");
    }
    let mut spent = 0usize;
    for (index, hit) in hits.iter().enumerate() {
        let text = hit.text.trim();
        if spent > 0 && spent + text.len() > KB_INJECT_MAX_CHARS {
            out.push_str(&format!(
                "(还有 {} 条相关片段因长度限制未展开)\n",
                hits.len() - index
            ));
            break;
        }
        out.push_str(&format!(
            "### [{}] {}\n知识库: 《{}》\nsource_ref: `{}`\n来源: `{}`\n{}\n\n",
            index + 1,
            hit.document_name,
            hit.collection_name,
            hit.source_ref,
            hit.source_path,
            text
        ));
        spent += text.len();
    }
    out
}

/// 把检索命中拼成给模型的文本(带出处)。命中为空时调用方不应调用本函数。
pub(crate) fn build_kb_context_block(
    collections: &[(i64, String)],
    hits: &[ScopedChunkHit],
) -> String {
    let title = if collections.is_empty() {
        "《知识库》".to_string()
    } else {
        collections
            .iter()
            .map(|(_, name)| format!("《{name}》"))
            .collect::<Vec<_>>()
            .join("、")
    };
    let mut out = format!(
        "在已启用知识集{title}中检索到以下相关片段(按相关度稳定排序)。请**严格基于这些片段**作答\
         并注明来源文件;若片段足够就直接回答,不要继续打开源文件。若上下文不足,可再次\
         `kb_search`,或用结果中的 `source_ref` 调用 `kb_open_source` 查看相邻片段。对于\
         XLSX/DOCX/PPTX 等二进制来源,禁止调用 `File(action=\"read\")` 或用 `Bash(action=\"run\")` 全量展开。\n\n"
    );
    let mut spent = 0usize;
    for (i, scoped) in hits.iter().enumerate() {
        let h = &scoped.hit;
        let text = h.text.trim();
        // 整体超限即停(但保证第一条一定注入),余下条数提示给模型。
        if spent > 0 && spent + text.len() > KB_INJECT_MAX_CHARS {
            out.push_str(&format!(
                "(还有 {} 条相关片段因长度限制未展开)\n",
                hits.len() - i
            ));
            break;
        }
        let collection_names = scoped
            .collection_ids
            .iter()
            .filter_map(|collection_id| {
                collections
                    .iter()
                    .find(|(id, _)| id == collection_id)
                    .map(|(_, name)| format!("《{name}》"))
            })
            .collect::<Vec<_>>()
            .join("、");
        out.push_str(&format!(
            "### [{}] {}\n知识库: {}\nsource_ref: `{}`\n来源: `{}`\n{}\n\n",
            i + 1,
            h.doc_name,
            if collection_names.is_empty() {
                "《知识库》"
            } else {
                &collection_names
            },
            source_ref(h.document_id, h.ord),
            h.doc_path,
            text
        ));
        spent += text.len();
    }
    out
}

fn render_source_window(
    source_ref_value: &str,
    document: &Document,
    chunks: &[(i64, String)],
) -> Value {
    let mut content = String::new();
    for (ord, text) in chunks {
        if !content.is_empty() {
            content.push('\n');
        }
        content.push_str(&format!("## chunk {ord}\n{}\n", text.trim()));
    }
    let next_start_chunk = chunks
        .last()
        .map(|(ord, _)| ord + 1)
        .filter(|next| *next < document.n_chunks);
    json!({
        "type": "kb_source",
        "source_ref": source_ref_value,
        "name": document.name,
        "path": document.path,
        "collectionId": document.collection_id,
        "collectionName": document.coll_name,
        "extension": document.ext,
        "start_chunk": chunks.first().map(|(ord, _)| *ord),
        "shown_chunks": chunks.len(),
        "total_chunks": document.n_chunks,
        "next_start_chunk": next_start_chunk,
        "truncated": next_start_chunk.is_some(),
        "content": content,
    })
}

fn load_source_window(
    l1: &super::l1::L1Store,
    collection_id: i64,
    document_id: i64,
    start_ord: i64,
    max_chunks: usize,
    source_ref_value: &str,
) -> rusqlite::Result<Option<Value>> {
    let Some(document) = l1.document_in_collection(collection_id, document_id)? else {
        return Ok(None);
    };
    let chunks = l1.document_chunk_window(collection_id, document_id, start_ord, max_chunks)?;
    Ok(Some(render_source_window(
        source_ref_value,
        &document,
        &chunks,
    )))
}

/// AI 可调的本地知识检索工具。per-session 构造,`execute` 查该会话挂载的知识集。
pub struct KbSearchTool {
    app: AppHandle,
    session_id: String,
}

impl KbSearchTool {
    pub fn new(app: AppHandle, session_id: String) -> Self {
        Self { app, session_id }
    }
}

#[async_trait]
impl ToolSpec for KbSearchTool {
    fn name(&self) -> &str {
        "kb_search"
    }

    fn description(&self) -> &str {
        "检索本会话挂载的本地与远程知识集(用户自己的文档/资料),返回带出处的片段。\
         当用户问题涉及这些资料时,先用本工具检索,再严格基于返回片段作答。"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "检索词或问题(自然语言或关键词均可)"
                }
            },
            "required": ["query"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let query = input
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if query.is_empty() {
            return Err(ToolError::missing_field("query"));
        }
        // 本地与远程挂载都是软依赖；两类来源可以在同一会话同时启用。
        let (collection_ids, remote_mounts) = self
            .app
            .try_state::<SessionStore>()
            .map(|store| {
                (
                    store.mounted_collection_ids(&self.session_id),
                    store
                        .mounted_remote_collections(&self.session_id)
                        .into_iter()
                        .filter(|collection| collection.enabled)
                        .collect::<Vec<_>>(),
                )
            })
            .unwrap_or_default();
        if collection_ids.is_empty() && remote_mounts.is_empty() {
            return Ok(ToolResult::success(
                "本会话未启用任何知识集，无法检索。请提示用户在输入框上方挂载并启用本地或远程知识集后再试。",
            ));
        }

        let mut unified = Vec::new();
        let mut warnings = Vec::new();

        if !collection_ids.is_empty() {
            if let Some(kb) = self.app.try_state::<KnowledgeService>() {
                let l1 = kb.l1().clone();
                let q = query.clone();
                let local_ids = collection_ids.clone();
                match tauri::async_runtime::spawn_blocking(move || {
                    let hits = l1.retrieve_for_chat_multi(
                        &local_ids,
                        &q,
                        KB_INJECT_TOP_K,
                        KB_NEIGHBOR_RADIUS,
                    )?;
                    let collections: Vec<(i64, String)> = local_ids
                        .into_iter()
                        .map(|collection_id| {
                            let name = l1
                                .collection_name(collection_id)
                                .ok()
                                .flatten()
                                .unwrap_or_else(|| format!("#{collection_id}"));
                            (collection_id, name)
                        })
                        .collect();
                    Ok::<_, rusqlite::Error>((hits, collections))
                })
                .await
                {
                    Ok(Ok((hits, collections))) => {
                        for (rank, scoped) in hits.into_iter().enumerate() {
                            let hit = scoped.hit;
                            let name = scoped
                                .collection_ids
                                .iter()
                                .find_map(|id| {
                                    collections
                                        .iter()
                                        .find(|(candidate, _)| candidate == id)
                                        .map(|(_, name)| name.clone())
                                })
                                .unwrap_or_else(|| "本地知识库".to_string());
                            unified.push(UnifiedHit {
                                collection_name: name,
                                document_name: hit.doc_name,
                                source_path: hit.doc_path,
                                source_ref: source_ref(hit.document_id, hit.ord),
                                text: hit.text,
                                // Local and remote engines expose scores on different scales.
                                // Fuse their independently ranked result lists instead of
                                // comparing incomparable raw values.
                                score: source_rank_score(rank),
                            });
                        }
                    }
                    Ok(Err(error)) => warnings.push(format!("本地知识库：{error}")),
                    Err(error) => warnings.push(format!("本地知识库任务失败：{error}")),
                }
            } else {
                warnings.push("本地知识库服务不可用".to_string());
            }
        }

        if !remote_mounts.is_empty() {
            if let Some(remote) = self.app.try_state::<RemoteKnowledgeService>() {
                let mut by_server = std::collections::BTreeMap::<String, Vec<i64>>::new();
                for mount in remote_mounts {
                    by_server
                        .entry(mount.server_id)
                        .or_default()
                        .push(mount.collection_id);
                }
                let requests = by_server.into_iter().map(|(server_id, ids)| {
                    let query = query.clone();
                    let remote = remote.clone();
                    async move {
                        let result = match timeout(KB_REMOTE_SOURCE_TIMEOUT, async {
                            // Collection labels and retrieval are independent requests; doing
                            // them together removes one network round trip from every search.
                            let (collections, hits) = tokio::try_join!(
                                remote.collections(&server_id, false),
                                remote.search(&server_id, ids, query, KB_INJECT_TOP_K),
                            )?;
                            Ok::<_, String>((collections, hits))
                        })
                        .await
                        {
                            Ok(result) => result,
                            Err(_) => Err(format!(
                                "检索超过 {} 秒，已跳过该来源",
                                KB_REMOTE_SOURCE_TIMEOUT.as_secs()
                            )),
                        };
                        (server_id, result)
                    }
                });
                for (server_id, result) in join_all(requests).await {
                    match result {
                        Ok((collections, hits)) => {
                            let server_name = remote
                                .connection(&server_id)
                                .map(|connection| connection.name)
                                .unwrap_or_else(|_| server_id.clone());
                            for (rank, hit) in hits.into_iter().enumerate() {
                                let collection_name = collections
                                    .iter()
                                    .find(|collection| collection.id == hit.collection_id)
                                    .map(|collection| collection.name.clone())
                                    .unwrap_or_else(|| format!("#{}", hit.collection_id));
                                unified.push(UnifiedHit {
                                    collection_name: format!("{server_name} / {collection_name}"),
                                    document_name: hit.document_name.clone(),
                                    source_path: format!(
                                        "remote://{server_name}/{}",
                                        hit.document_name
                                    ),
                                    source_ref: remote_source_ref(
                                        &server_id,
                                        hit.collection_id,
                                        hit.document_id,
                                        hit.ord,
                                    ),
                                    text: hit.text,
                                    score: source_rank_score(rank),
                                });
                            }
                        }
                        Err(error) => {
                            let name = remote
                                .connection(&server_id)
                                .map(|connection| connection.name)
                                .unwrap_or(server_id);
                            warnings.push(format!("{name}：{error}"));
                        }
                    }
                }
            } else {
                warnings.push("远程知识库连接服务不可用".to_string());
            }
        }

        unified.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.document_name.cmp(&right.document_name))
        });
        unified.truncate(KB_INJECT_TOP_K);

        if unified.is_empty() {
            let unavailable = if warnings.is_empty() {
                String::new()
            } else {
                format!(" 部分来源不可用：{}。", warnings.join("；"))
            };
            return Ok(ToolResult::success(format!(
                "在已启用知识集中未找到与「{query}」相关的内容。{unavailable}如无其他依据，请如实告知用户未检索到，不要编造。"
            )));
        }
        Ok(ToolResult::success(build_unified_context_block(
            &unified, &warnings,
        )))
    }
}

/// 查看 `kb_search` 命中的同一份知识文档的相邻 chunk。工具只接受受控的
/// `source_ref`，并用 session 当前挂载的 collection id 再校验，不接受任意文件路径。
pub struct KbOpenSourceTool {
    app: AppHandle,
    session_id: String,
}

impl KbOpenSourceTool {
    pub fn new(app: AppHandle, session_id: String) -> Self {
        Self { app, session_id }
    }
}

#[async_trait]
impl ToolSpec for KbOpenSourceTool {
    fn name(&self) -> &str {
        "kb_open_source"
    }

    fn description(&self) -> &str {
        "查看 `kb_search` 返回的某个本地或远程知识来源的相邻内容。只接受检索结果中的 `source_ref`,\
         不接受文件路径;默认从命中 chunk 的前一块开始返回 3 块。对于 XLSX/DOCX/PPTX 等\
         来源使用本工具,不要调用 `File(action=\"read\")` 或用 `Bash(action=\"run\")` 全量展开。需要定位其他内容时先再次\
         调用 `kb_search`,再打开它返回的新 `source_ref`。"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "source_ref": {
                    "type": "string",
                    "description": "kb_search 返回的来源引用,例如 kbdoc:128:chunk:3"
                },
                "start_chunk": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "可选,从指定 chunk 序号开始分页;默认展开命中位置附近"
                },
                "max_chunks": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": KB_OPEN_MAX_CHUNKS,
                    "description": "最多返回多少个 chunk,默认 3,最大 8"
                }
            },
            "required": ["source_ref"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let source_ref_value = input
            .get("source_ref")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if source_ref_value.is_empty() {
            return Err(ToolError::missing_field("source_ref"));
        }
        let remote_reference = parse_remote_source_ref(&source_ref_value);
        let local_reference = parse_source_ref(&source_ref_value);
        let (document_id, anchor_ord) = remote_reference
            .as_ref()
            .map(|(_, _, document_id, ord)| (*document_id, *ord))
            .or(local_reference)
            .ok_or_else(|| {
                ToolError::invalid_input(
                    "source_ref 格式无效；请原样使用 kb_search 返回的引用".to_string(),
                )
            })?;
        let start_ord = match input.get("start_chunk") {
            Some(value) => value.as_i64().filter(|v| *v >= 0).ok_or_else(|| {
                ToolError::invalid_input("start_chunk 必须是大于等于 0 的整数".to_string())
            })?,
            None => anchor_ord.saturating_sub(1),
        };
        let max_chunks = match input.get("max_chunks") {
            Some(value) => value.as_u64().filter(|v| *v > 0).ok_or_else(|| {
                ToolError::invalid_input("max_chunks 必须是大于 0 的整数".to_string())
            })? as usize,
            None => KB_OPEN_DEFAULT_CHUNKS,
        }
        .min(KB_OPEN_MAX_CHUNKS);

        if let Some((server_id, collection_id, document_id, _)) = remote_reference {
            let mounted = self
                .app
                .try_state::<SessionStore>()
                .map(|store| {
                    store
                        .mounted_remote_collections(&self.session_id)
                        .into_iter()
                        .any(|collection| {
                            collection.enabled
                                && collection.server_id == server_id
                                && collection.collection_id == collection_id
                        })
                })
                .unwrap_or(false);
            if !mounted {
                return Ok(ToolResult::success(
                    "该远程 source_ref 不属于本会话当前启用的知识集。请重新调用 kb_search。",
                ));
            }
            let Some(remote) = self.app.try_state::<RemoteKnowledgeService>() else {
                return Ok(ToolResult::success("远程知识库连接服务不可用。"));
            };
            let window = remote
                .source_window(
                    &server_id,
                    pinvou_knowledge::model::SourceWindowRequest {
                        collection_id,
                        document_id,
                        start_ord,
                        limit: max_chunks,
                    },
                )
                .await
                .map_err(|error| {
                    ToolError::execution_failed(format!("kb_open_source remote failed: {error}"))
                })?;
            let mut content = String::new();
            for chunk in &window.chunks {
                content.push_str(&format!("## chunk {}\n{}\n", chunk.ord, chunk.text.trim()));
            }
            let next_start_chunk = window
                .chunks
                .last()
                .map(|chunk| chunk.ord + 1)
                .filter(|next| *next < window.document.n_chunks);
            let rendered = json!({
                "type": "kb_remote_source",
                "source_ref": source_ref_value,
                "name": window.document.name,
                "serverId": server_id,
                "collectionId": collection_id,
                "start_chunk": window.chunks.first().map(|chunk| chunk.ord),
                "shown_chunks": window.chunks.len(),
                "total_chunks": window.document.n_chunks,
                "next_start_chunk": next_start_chunk,
                "truncated": next_start_chunk.is_some(),
                "content": content,
            });
            return ToolResult::json(&rendered).map_err(|error| {
                ToolError::execution_failed(format!(
                    "serialize remote kb_open_source result: {error}"
                ))
            });
        }

        let collection_ids = self
            .app
            .try_state::<SessionStore>()
            .map(|store| store.mounted_collection_ids(&self.session_id))
            .unwrap_or_default();
        if collection_ids.is_empty() {
            return Ok(ToolResult::success(
                "本会话未启用任何本地知识集,无法打开来源。请先挂载并启用知识集。",
            ));
        }
        let Some(kb) = self.app.try_state::<KnowledgeService>() else {
            return Ok(ToolResult::success("本地知识库服务不可用。"));
        };
        let l1 = kb.l1().clone();
        let source_ref_for_query = source_ref_value.clone();
        let loaded =
            tauri::async_runtime::spawn_blocking(move || -> rusqlite::Result<Option<Value>> {
                for collection_id in collection_ids {
                    if let Some(rendered) = load_source_window(
                        &l1,
                        collection_id,
                        document_id,
                        start_ord,
                        max_chunks,
                        &source_ref_for_query,
                    )? {
                        return Ok(Some(rendered));
                    }
                }
                Ok(None)
            })
            .await
            .map_err(|e| ToolError::execution_failed(format!("kb_open_source join failed: {e}")))?
            .map_err(|e| {
                ToolError::execution_failed(format!("kb_open_source query failed: {e}"))
            })?;

        let Some(rendered) = loaded else {
            return Ok(ToolResult::success(
                "该 source_ref 不属于本会话当前启用的知识集,或文档尚未解析完成。请重新调用 kb_search。",
            ));
        };
        ToolResult::json(&rendered).map_err(|e| {
            ToolError::execution_failed(format!("serialize kb_open_source result: {e}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{l1::L1Store, store::Store};
    use super::*;
    use std::path::Path;

    fn hit(name: &str, ord: i64, text: &str) -> ScopedChunkHit {
        ScopedChunkHit {
            collection_ids: vec![7],
            hit: super::super::l1::ChunkHit {
                document_id: 42,
                text: text.to_string(),
                score: 1.0,
                doc_name: name.to_string(),
                doc_path: format!("/docs/{name}"),
                ord,
            },
        }
    }

    /// 片段块:带知识集名、逐条出处、命中文本;空名兜底「知识库」。
    #[test]
    fn kb_context_block_renders_hits_with_sources() {
        let mut hits = vec![
            hit("散热报告.xlsx", 3, "CPU 峰值温度 78℃"),
            hit("规格.pdf", 0, "TDP 28W"),
        ];
        hits[0].collection_ids.push(8);
        let block = build_kb_context_block(
            &[(7, "硬件资料".to_string()), (8, "团队规范".to_string())],
            &hits,
        );
        assert!(block.contains("《硬件资料》"));
        assert!(block.contains("《团队规范》"));
        assert!(block.contains("散热报告.xlsx"));
        assert!(block.contains("source_ref: `kbdoc:42:chunk:3`"));
        assert!(block.contains("`/docs/散热报告.xlsx`"));
        assert!(block.contains("CPU 峰值温度 78℃"));
        assert!(block.contains("TDP 28W"));
        assert!(block.contains("kb_open_source"));
        assert!(block.contains("禁止调用 `File(action=\"read\")`"));

        let none = build_kb_context_block(&[], &hits);
        assert!(none.contains("《知识库》"));
    }

    #[test]
    fn remote_source_reference_round_trips_without_exposing_paths() {
        let reference = remote_source_ref("cube/server:1", 7, 42, 3);
        assert!(!reference.contains("cube/server:1"));
        assert_eq!(
            parse_remote_source_ref(&reference),
            Some(("cube/server:1".to_string(), 7, 42, 3))
        );
        assert!(parse_remote_source_ref("kbremote:bad:7:42:chunk:-1").is_none());
    }

    /// 超总字符预算:保证第一条一定注入,余下截断并提示剩余条数。
    #[test]
    fn kb_context_block_truncates_over_budget() {
        let big = "字".repeat(KB_INJECT_MAX_CHARS);
        let hits = vec![
            hit("a.txt", 0, &big),
            hit("b.txt", 1, &big),
            hit("c.txt", 2, &big),
        ];
        let block = build_kb_context_block(&[(7, "X".to_string())], &hits);
        assert!(block.contains("a.txt")); // 第一条必注入
        assert!(!block.contains("b.txt")); // 超预算被截断
        assert!(block.contains("还有 2 条"));
    }

    #[test]
    fn kb_source_ref_roundtrips_and_rejects_forgery() {
        let value = source_ref(128, 3);
        assert_eq!(value, "kbdoc:128:chunk:3");
        assert_eq!(parse_source_ref(&value), Some((128, 3)));
        for invalid in [
            "",
            "kbdoc:0:chunk:3",
            "kbdoc:128:chunk:-1",
            "kbdoc:128:path:3",
            "kbdoc:128:chunk:3:extra",
            "/tmp/report.xlsx",
        ] {
            assert_eq!(parse_source_ref(invalid), None, "accepted {invalid}");
        }
    }

    #[test]
    fn kb_open_source_window_is_bounded_and_pageable() {
        let document = Document {
            id: 128,
            collection_id: 7,
            coll_name: Some("散热".to_string()),
            path: "/docs/report.xlsx".to_string(),
            name: "report.xlsx".to_string(),
            ext: Some("xlsx".to_string()),
            size: 1024,
            mtime: 1,
            parse_status: "parsed".to_string(),
            n_chunks: 6,
        };
        let chunks = vec![
            (2, "SSD主控 | 75.2 | 85".to_string()),
            (3, "SSD颗粒 | 73.7 | 85".to_string()),
            (4, "CPU_DTS | 76.9 | 100".to_string()),
        ];
        let rendered = render_source_window("kbdoc:128:chunk:3", &document, &chunks);
        assert_eq!(rendered["type"], "kb_source");
        assert_eq!(rendered["collectionId"], 7);
        assert_eq!(rendered["collectionName"], "散热");
        assert_eq!(rendered["shown_chunks"], 3);
        assert_eq!(rendered["next_start_chunk"], 5);
        assert_eq!(rendered["truncated"], true);
        assert!(rendered["content"].as_str().unwrap().contains("SSD主控"));
        assert!(rendered["content"].as_str().unwrap().contains("SSD颗粒"));
    }

    /// 真实模拟二进制来源链路：XLSX 建索引 → kb_search 命中 → source_ref →
    /// kb_open_source 展开已解析 chunk。整个打开阶段不再次读取原始工作簿。
    #[test]
    fn kb_open_source_simulates_search_then_open_for_xlsx() {
        let store = Store::open_in_memory().unwrap();
        let l1 = L1Store::new(store.conn_arc(), None);
        let mounted = l1.create_collection("散热报告", None, None).unwrap();
        let other = l1.create_collection("其他资料", None, None).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/multi_sheet.xlsx");

        assert_eq!(l1.ingest_file(mounted, &fixture), "parsed");
        let hits = l1.retrieve_for_chat(mounted, "83.6", 5, 0).unwrap();
        let hit = hits
            .first()
            .expect("kb_search should hit non-first XLSX sheet");
        let reference = source_ref(hit.document_id, hit.ord);
        let (document_id, anchor_ord) = parse_source_ref(&reference).unwrap();

        let opened = load_source_window(
            &l1,
            mounted,
            document_id,
            anchor_ord.saturating_sub(1),
            KB_OPEN_DEFAULT_CHUNKS,
            &reference,
        )
        .unwrap()
        .expect("mounted source should open");
        assert_eq!(opened["extension"], "xlsx");
        assert_eq!(opened["source_ref"], reference);
        assert!(opened["content"].as_str().unwrap().contains("83.6"));
        if std::env::var_os("PINVOU_SHOW_KB_OPEN_SOURCE").is_some() {
            println!(
                "kb_search source_ref = {reference}\nkb_open_source result = {}",
                serde_json::to_string_pretty(&opened).unwrap()
            );
        }
        assert!(
            load_source_window(
                &l1,
                other,
                document_id,
                0,
                KB_OPEN_DEFAULT_CHUNKS,
                &reference,
            )
            .unwrap()
            .is_none(),
            "source_ref must not cross the mounted collection boundary"
        );
    }
}
