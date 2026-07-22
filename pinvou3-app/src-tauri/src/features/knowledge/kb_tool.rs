//! Agentic RAG: `kb_search` 工具——AI 自主调用检索本会话挂载的知识集。
//!
//! 取代注入式(每条消息自动注入片段):由 LLM 自己决定何时查/查什么。工具 per-session
//! 构造(持 `session_id`),`execute` 时查该会话挂载的知识集,复用 [`L1Store::retrieve_for_chat`]。
//! 经底座 `EngineConfig.extra_tools` 注入(spawn_for_session)。配套 Self-RAG 自检引导见
//! `commands::build_kb_agentic_guide`。

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use deepseek_tui::tools::spec::{ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec};

use super::ChunkHit;
use crate::features::sessions::SessionStore;
use crate::features::knowledge::KnowledgeService;

/// 工具单次检索 top-K(精排;太多稀释小模型注意力)。
pub(crate) const KB_INJECT_TOP_K: usize = 5;
/// 邻域扩展半径,暂关(=0,只给命中块)。见 `L1Store::expand_neighbors`,改回 1 即恢复。
pub(crate) const KB_NEIGHBOR_RADIUS: usize = 0;
/// 返回片段总字符上限,超出截断(保证至少第一条)。~6K 字符 ≈ 5K token。
pub(crate) const KB_INJECT_MAX_CHARS: usize = 6_000;

/// 把检索命中拼成给模型的文本(带出处)。命中为空时调用方不应调用本函数。
pub(crate) fn build_kb_context_block(collection_name: Option<&str>, hits: &[ChunkHit]) -> String {
    let title = collection_name.unwrap_or("知识库");
    let mut out = format!(
        "在知识集《{title}》中检索到以下相关片段(按相关度排序)。请**严格基于这些片段**作答\
         并注明来源文件;若不足以回答,如实说明,不要凭空编造。\n\n"
    );
    let mut spent = 0usize;
    for (i, h) in hits.iter().enumerate() {
        let text = h.text.trim();
        // 整体超限即停(但保证第一条一定注入),余下条数提示给模型。
        if spent > 0 && spent + text.len() > KB_INJECT_MAX_CHARS {
            out.push_str(&format!("(还有 {} 条相关片段因长度限制未展开)\n", hits.len() - i));
            break;
        }
        out.push_str(&format!(
            "### [{}] {}\n来源: `{}`\n{}\n\n",
            i + 1,
            h.doc_name,
            h.doc_path,
            text
        ));
        spent += text.len();
    }
    out
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
        "检索本会话挂载的本地知识集(用户自己的文档/资料),返回带出处的片段。\
         当用户问题涉及其本地资料时,先用本工具检索,再严格基于返回片段作答。"
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
        // 挂载集 + 知识库服务都软依赖(会话可能未挂集;KnowledgeService 条件 manage)。
        let cid = self
            .app
            .try_state::<SessionStore>()
            .and_then(|s| s.mounted_collection(&self.session_id));
        let Some(cid) = cid else {
            return Ok(ToolResult::success(
                "本会话未挂载任何本地知识集,无法检索。请提示用户在输入框上方挂载一个知识集后再试。",
            ));
        };
        let Some(kb) = self.app.try_state::<KnowledgeService>() else {
            return Ok(ToolResult::success("本地知识库服务不可用。"));
        };
        let l1 = kb.l1().clone();
        let q = query.clone();
        // retrieve_for_chat 含 blocking embedding,挪出 async executor。
        let (hits, name) = tauri::async_runtime::spawn_blocking(move || {
            let hits = l1
                .retrieve_for_chat(cid, &q, KB_INJECT_TOP_K, KB_NEIGHBOR_RADIUS)
                .unwrap_or_default();
            let name = l1.collection_name(cid).ok().flatten();
            (hits, name)
        })
        .await
        .unwrap_or_default();
        if hits.is_empty() {
            return Ok(ToolResult::success(format!(
                "在知识集《{}》中未找到与「{}」相关的内容。如无其他依据,请如实告知用户未检索到,不要编造。",
                name.as_deref().unwrap_or("知识库"),
                query
            )));
        }
        Ok(ToolResult::success(build_kb_context_block(name.as_deref(), &hits)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(name: &str, ord: i64, text: &str) -> ChunkHit {
        ChunkHit {
            text: text.to_string(),
            score: 1.0,
            doc_name: name.to_string(),
            doc_path: format!("/docs/{name}"),
            ord,
        }
    }

    /// 片段块:带知识集名、逐条出处、命中文本;空名兜底「知识库」。
    #[test]
    fn kb_context_block_renders_hits_with_sources() {
        let hits = vec![hit("散热报告.xlsx", 3, "CPU 峰值温度 78℃"), hit("规格.pdf", 0, "TDP 28W")];
        let block = build_kb_context_block(Some("硬件资料"), &hits);
        assert!(block.contains("《硬件资料》"));
        assert!(block.contains("散热报告.xlsx"));
        assert!(block.contains("`/docs/散热报告.xlsx`"));
        assert!(block.contains("CPU 峰值温度 78℃"));
        assert!(block.contains("TDP 28W"));

        let none = build_kb_context_block(None, &hits);
        assert!(none.contains("《知识库》"));
    }

    /// 超总字符预算:保证第一条一定注入,余下截断并提示剩余条数。
    #[test]
    fn kb_context_block_truncates_over_budget() {
        let big = "字".repeat(KB_INJECT_MAX_CHARS);
        let hits = vec![hit("a.txt", 0, &big), hit("b.txt", 1, &big), hit("c.txt", 2, &big)];
        let block = build_kb_context_block(Some("X"), &hits);
        assert!(block.contains("a.txt")); // 第一条必注入
        assert!(!block.contains("b.txt")); // 超预算被截断
        assert!(block.contains("还有 2 条"));
    }
}
