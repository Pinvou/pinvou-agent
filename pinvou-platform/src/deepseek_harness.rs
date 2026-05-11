//! DeepSeekHarness — 实现 AgentHarness trait，对接本地和远程 LLM。
//!
//! 泛型参数 C: LlmClient 支持注入 DeepSeekClient（生产）、Ollama client、MockLlmClient（测试）。
//!
//! ## 工具执行循环（自动 tool loop）
//!
//! 通过 `with_tools()` 注入 `ToolRegistry` + `ToolContext` 后，harness 在 LLM
//! 调用工具时会自动 dispatch：
//! - 工具名在 `auto_tool_names` 集合中 → harness 调 `ToolSpec::execute()`，把结果
//!   作为 `ToolResult` 写回对话历史，再次调 LLM，让它基于工具结果继续生成
//! - 工具名不在集合中（如 `request_user_input`）→ 把 `ToolCallStart` 透传给
//!   上层，由 web/TUI 处理用户交互
//!
//! 这避免 LLM 输出形如 `[web_search: ...]` 的伪工具调用：因为现在它真的会被执行。

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_stream::stream;
use async_trait::async_trait;
use futures_util::{Stream, StreamExt};

use super::harness::{AgentHarness, ChatRequest, Checkpoint, ModelInfo, StreamEvent, ToolDef};
use deepseek_tui::llm_client::{LlmClient, StreamEventBox};
use deepseek_tui::models::{
    ContentBlock, ContentBlockStart, Delta, Message, MessageRequest, StreamEvent as DtStreamEvent,
    SystemPrompt, Tool,
};
use deepseek_tui::tools::registry::ToolRegistry;
use deepseek_tui::tools::spec::ToolContext;

/// 工具自动循环的软上限。撞到上限**不会**直接报错杀流，而是触发 graceful
/// degradation：通知 LLM 不再允许调用工具，让它基于已收集的信息直接给出最终
/// 回复。换言之，这是一个"软兜底"——任何具体的 N 都可能被合法场景打满
/// （比如 freeform research 多维度搜索），关键是撞了之后体验不崩，而不是
/// 把 N 调得多大。详细见 §3.7。
const TOOL_LOOP_MAX_ITERATIONS: usize = 12;
const TOOL_ARGS_VISIBLE_MAX: usize = 200;
const TOOL_RESULT_VISIBLE_MAX: usize = 1500;

/// 把工具调用参数压缩成可视摘要（长 JSON 截断）
fn render_args_compact(args: &serde_json::Value) -> String {
    let raw = serde_json::to_string(args).unwrap_or_else(|_| "{}".into());
    truncate_chars(&raw, TOOL_ARGS_VISIBLE_MAX)
}

/// 把工具结果压缩成可视摘要（保留前 N 字符 + 省略号）
fn render_result_compact(content: &str) -> String {
    truncate_chars(content, TOOL_RESULT_VISIBLE_MAX)
}

fn truncate_chars(s: &str, limit: usize) -> String {
    let total = s.chars().count();
    if total <= limit {
        return s.to_string();
    }
    let prefix: String = s.chars().take(limit).collect();
    format!("{prefix}… (节选自 {total} 字符)")
}

/// 旧行为：单次 LLM 调用 + 透传 tool calls，不自动执行工具。
/// 当 harness 未注入 ToolRegistry 时使用，保留向后兼容。
async fn chat_stream_passthrough<C: LlmClient>(
    client: C,
    msg_req: MessageRequest,
) -> Result<Box<dyn Stream<Item = Result<StreamEvent>> + Send + Unpin>> {
    use std::collections::HashMap;
    use std::sync::Mutex;

    let llm_stream: StreamEventBox = client.create_message_stream(msg_req).await?;
    type ToolState = (String, String, String);
    let idx_map: Arc<Mutex<HashMap<u32, ToolState>>> = Arc::new(Mutex::new(HashMap::new()));

    let mapped = llm_stream.map(move |result| match result {
        Ok(DtStreamEvent::ContentBlockStart {
            index,
            content_block,
        }) => match content_block {
            ContentBlockStart::ToolUse { id, name, .. } => {
                if let Ok(mut map) = idx_map.lock() {
                    map.insert(index, (id, name, String::new()));
                }
                Ok(StreamEvent::TextDelta {
                    content: "\n\n⏳ 正在准备选项...\n".into(),
                })
            }
            _ => Ok(StreamEvent::TextDelta {
                content: String::new(),
            }),
        },
        Ok(DtStreamEvent::ContentBlockDelta { index, delta }) => match delta {
            Delta::TextDelta { text } => Ok(StreamEvent::TextDelta { content: text }),
            Delta::InputJsonDelta { partial_json } => {
                if let Ok(mut map) = idx_map.lock() {
                    if let Some(entry) = map.get_mut(&index) {
                        entry.2.push_str(&partial_json);
                    }
                }
                Ok(StreamEvent::TextDelta {
                    content: String::new(),
                })
            }
            _ => Ok(StreamEvent::TextDelta {
                content: String::new(),
            }),
        },
        Ok(DtStreamEvent::ContentBlockStop { index }) => {
            let removed = if let Ok(mut map) = idx_map.lock() {
                map.remove(&index)
            } else {
                None
            };
            if let Some((call_id, tool_name, buf)) = removed {
                let args: serde_json::Value = if buf.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::from_str(&buf).unwrap_or(serde_json::Value::Null)
                };
                return Ok(StreamEvent::ToolCallStart {
                    call_id,
                    tool_name,
                    arguments: args,
                });
            }
            Ok(StreamEvent::TextDelta {
                content: String::new(),
            })
        }
        Ok(DtStreamEvent::MessageStop) => Ok(StreamEvent::Done),
        Err(e) => Ok(StreamEvent::Error {
            message: e.to_string(),
        }),
        _ => Ok(StreamEvent::TextDelta {
            content: String::new(),
        }),
    });

    Ok(Box::new(mapped))
}

pub struct DeepSeekHarness<C: LlmClient> {
    client: C,
    tools: Vec<ToolDef>,
    models: Vec<ModelInfo>,
    workspace: PathBuf,
    checkpoint_dir: PathBuf,
    /// DeepSeek-TUI 工具注册表。注入后，harness 在 chat_stream 中自动 dispatch。
    tool_registry: Option<Arc<ToolRegistry>>,
    /// 工具执行上下文（workspace、sandbox 等）。
    tool_context: Option<ToolContext>,
    /// 哪些工具走 harness 自动执行；不在此集合的工具透传给上层（如 request_user_input）。
    auto_tool_names: HashSet<String>,
}

impl<C: LlmClient + Clone + 'static> DeepSeekHarness<C> {
    pub fn new(client: C, tools: Vec<ToolDef>, models: Vec<ModelInfo>, workspace: PathBuf) -> Self {
        let checkpoint_dir = workspace.join(".checkpoints");
        Self {
            client,
            tools,
            models,
            workspace,
            checkpoint_dir,
            tool_registry: None,
            tool_context: None,
            auto_tool_names: HashSet::new(),
        }
    }

    pub fn with_checkpoint_dir(mut self, dir: PathBuf) -> Self {
        self.checkpoint_dir = dir;
        self
    }

    /// 注入 DeepSeek-TUI 工具注册表 + 执行上下文。
    ///
    /// `auto_tool_names` 是 harness **自动执行** 的工具名集合：当 LLM 调用其中
    /// 任一工具时，harness 会调 `ToolSpec::execute()`，把结果写回对话历史并
    /// 自动触发下一轮 LLM 调用。
    ///
    /// 不在此集合的工具（典型如 `request_user_input`）—— harness 将 `ToolCallStart`
    /// 透传给上层（web/TUI），由那里处理用户交互。
    pub fn with_tools(
        mut self,
        registry: Arc<ToolRegistry>,
        context: ToolContext,
        auto_tool_names: HashSet<String>,
    ) -> Self {
        // tools 字段从 registry 派生，覆盖手工传入的列表
        let api_tools = registry.to_api_tools();
        self.tools = api_tools
            .into_iter()
            .map(|t| ToolDef {
                name: t.name,
                description: t.description,
                parameters: t.input_schema,
            })
            .collect();
        self.tool_registry = Some(registry);
        self.tool_context = Some(context);
        self.auto_tool_names = auto_tool_names;
        self
    }

    fn to_message_request(&self, req: &ChatRequest) -> MessageRequest {
        let model = req
            .model
            .clone()
            .unwrap_or_else(|| self.client.model().to_string());

        let system = req
            .platform_system_prompt
            .as_ref()
            .map(|p| SystemPrompt::Text(p.clone()));

        let mut messages = Vec::new();

        if !req.context.is_empty() {
            let ctx_text: String = req
                .context
                .iter()
                .map(|(k, v)| format!("[{k}]: {v}"))
                .collect::<Vec<_>>()
                .join("\n");
            messages.push(Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: format!("## 上下文信息\n\n{ctx_text}"),
                    cache_control: None,
                }],
            });
        }

        // 历史消息（对标 deepseek-tui Session.messages）
        for msg in &req.previous_messages {
            messages.push(Message {
                role: msg.role.clone(),
                content: vec![ContentBlock::Text {
                    text: msg.content.clone(),
                    cache_control: None,
                }],
            });
        }

        messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: req.user_message.clone(),
                cache_control: None,
            }],
        });

        let tools: Vec<Tool> = req
            .tools
            .iter()
            .map(|t| Tool {
                tool_type: None,
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.parameters.clone(),
                allowed_callers: None,
                defer_loading: None,
                input_examples: None,
                strict: None,
                cache_control: None,
            })
            .collect();

        let tools_opt = if tools.is_empty() { None } else { Some(tools) };
        // 对标 DeepSeek-TUI turn_loop: 有工具时设 {"type": "auto"}，否则 None
        let tool_choice = tools_opt
            .as_ref()
            .map(|_| serde_json::json!({"type": "auto"}));

        eprintln!(
            "[harness] tools: {:?}, tool_choice: {:?}",
            tools_opt
                .as_ref()
                .map(|ts| ts.iter().map(|t| &t.name).collect::<Vec<_>>()),
            tool_choice
        );

        MessageRequest {
            model,
            messages,
            max_tokens: 4096,
            system,
            tools: tools_opt,
            tool_choice,
            metadata: None,
            thinking: None,
            reasoning_effort: None,
            stream: None,
            temperature: None,
            top_p: None,
        }
    }
}

#[async_trait]
impl<C: LlmClient + Clone + 'static> AgentHarness for DeepSeekHarness<C> {
    /// 非流式 chat — 绕过 SSE 流式请求以兼容 Ark 等平台
    async fn chat(&self, req: ChatRequest) -> Result<String> {
        let msg_req = self.to_message_request(&req);
        let response = self.client.create_message(msg_req).await?;
        // 提取第一个 text block 作为回复
        for block in &response.content {
            if let ContentBlock::Text { text, .. } = block {
                return Ok(text.clone());
            }
        }
        Ok(String::new())
    }

    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<Box<dyn Stream<Item = Result<StreamEvent>> + Send + Unpin>> {
        let initial_msg_req = self.to_message_request(&req);
        let client = self.client.clone();
        let registry = self.tool_registry.clone();
        let context = self.tool_context.clone();
        let auto_names = self.auto_tool_names.clone();

        // 没有 registry → 走兼容路径（旧行为：单次 LLM 调用 + 透传 tool calls）
        if registry.is_none() {
            return chat_stream_passthrough(client, initial_msg_req).await;
        }

        let registry = registry.unwrap();
        let context = context.expect("ToolContext required when registry is set");

        // 有 registry → tool loop：自动 dispatch + 多轮 LLM
        let s = stream! {
            let mut msg_req = initial_msg_req;
            let mut iteration: usize = 0;

            'outer: loop {
                iteration += 1;
                if iteration > TOOL_LOOP_MAX_ITERATIONS {
                    // === graceful degradation：上限不杀流 ===
                    // 通知用户 + LLM：禁工具，基于已收集信息直接出最终回复。
                    // 任何 N 都可能被合法场景打满，撞了之后体验必须不崩。
                    let notice = format!(
                        "\n\n（已达工具调用上限 {TOOL_LOOP_MAX_ITERATIONS} 轮，基于已收集信息总结输出，不再调用工具）\n\n"
                    );
                    yield Ok(StreamEvent::TextDelta {
                        content: notice,
                    });

                    // 禁工具 + 注入引导消息
                    msg_req.tools = None;
                    msg_req.tool_choice = None;
                    msg_req.messages.push(Message {
                        role: "user".to_string(),
                        content: vec![ContentBlock::Text {
                            text: format!(
                                "工具调用已达上限（{TOOL_LOOP_MAX_ITERATIONS} 轮）。请基于以上所有工具结果和已收集的信息，直接给出最终回复。不要再尝试调用任何工具，也不要请求更多搜索/读取——把现有信息组织好就够了。"
                            ),
                            cache_control: None,
                        }],
                    });

                    // 再做一次 LLM 调用，流式输出最终总结
                    let mut final_stream: StreamEventBox =
                        match client.create_message_stream(msg_req.clone()).await {
                            Ok(s) => s,
                            Err(e) => {
                                yield Ok(StreamEvent::Error { message: e.to_string() });
                                return;
                            }
                        };
                    while let Some(result) = final_stream.next().await {
                        match result {
                            Ok(DtStreamEvent::ContentBlockDelta {
                                delta: Delta::TextDelta { text },
                                ..
                            }) => {
                                yield Ok(StreamEvent::TextDelta { content: text });
                            }
                            Ok(DtStreamEvent::MessageStop) => break,
                            Err(e) => {
                                yield Ok(StreamEvent::Error { message: e.to_string() });
                                return;
                            }
                            _ => {}
                        }
                    }
                    yield Ok(StreamEvent::Done);
                    return;
                }

                let mut llm_stream: StreamEventBox =
                    match client.create_message_stream(msg_req.clone()).await {
                        Ok(s) => s,
                        Err(e) => {
                            yield Ok(StreamEvent::Error { message: e.to_string() });
                            return;
                        }
                    };

                let mut assistant_text = String::new();
                let mut tool_buffers: std::collections::HashMap<u32, (String, String, String)> =
                    std::collections::HashMap::new();
                let mut completed_calls: Vec<(String, String, serde_json::Value)> = Vec::new();

                while let Some(result) = llm_stream.next().await {
                    match result {
                        Ok(DtStreamEvent::ContentBlockStart { index, content_block }) => {
                            if let ContentBlockStart::ToolUse { id, name, .. } = content_block {
                                tool_buffers.insert(index, (id, name, String::new()));
                                yield Ok(StreamEvent::TextDelta {
                                    content: "\n\n⏳ 正在调用工具...\n".into(),
                                });
                            }
                        }
                        Ok(DtStreamEvent::ContentBlockDelta { index, delta }) => match delta {
                            Delta::TextDelta { text } => {
                                assistant_text.push_str(&text);
                                yield Ok(StreamEvent::TextDelta { content: text });
                            }
                            Delta::InputJsonDelta { partial_json } => {
                                if let Some(entry) = tool_buffers.get_mut(&index) {
                                    entry.2.push_str(&partial_json);
                                }
                            }
                            _ => {}
                        },
                        Ok(DtStreamEvent::ContentBlockStop { index }) => {
                            if let Some((call_id, tool_name, buf)) = tool_buffers.remove(&index) {
                                let args = if buf.is_empty() {
                                    serde_json::Value::Null
                                } else {
                                    serde_json::from_str(&buf).unwrap_or(serde_json::Value::Null)
                                };
                                // 把工具调用以文本形式 yield，方便 web 层把它纳入
                                // engine.messages（用于跨轮历史），同时也让用户看到。
                                // request_user_input 不暴露（前端会有专用 choice card UI）
                                if tool_name != "request_user_input" {
                                    let args_short = render_args_compact(&args);
                                    let banner =
                                        format!("\n\n🔧 [{tool_name}] {args_short}\n");
                                    assistant_text.push_str(&banner);
                                    yield Ok(StreamEvent::TextDelta { content: banner });
                                }
                                yield Ok(StreamEvent::ToolCallStart {
                                    call_id: call_id.clone(),
                                    tool_name: tool_name.clone(),
                                    arguments: args.clone(),
                                });
                                completed_calls.push((call_id, tool_name, args));
                            }
                        }
                        Ok(DtStreamEvent::MessageStop) => break,
                        Err(e) => {
                            yield Ok(StreamEvent::Error { message: e.to_string() });
                            return;
                        }
                        _ => {}
                    }
                }

                // 没有工具调用 → 终止（LLM 已生成完整回答）
                if completed_calls.is_empty() {
                    yield Ok(StreamEvent::Done);
                    return;
                }

                // 把工具调用分为：自动执行 / 透传给上层
                let mut auto_executed: Vec<(String, String, serde_json::Value, String)> = Vec::new();
                let mut has_pass_through = false;
                for (call_id, tool_name, args) in &completed_calls {
                    if auto_names.contains(tool_name) {
                        if let Some(spec) = registry.get(tool_name) {
                            match spec.execute(args.clone(), &context).await {
                                Ok(result) => {
                                    // 把工具结果以摘要文本 yield 进 stream，
                                    // 这样 web 层把它纳入 engine.messages，
                                    // 跨轮对话时 LLM 仍能看到工具交互上下文。
                                    let visible = render_result_compact(&result.content);
                                    let banner = format!("\n📄 结果:\n{visible}\n\n");
                                    yield Ok(StreamEvent::TextDelta {
                                        content: banner,
                                    });
                                    yield Ok(StreamEvent::ToolCallResult {
                                        call_id: call_id.clone(),
                                        output: result.content.clone(),
                                    });
                                    auto_executed.push((
                                        call_id.clone(),
                                        tool_name.clone(),
                                        args.clone(),
                                        result.content,
                                    ));
                                }
                                Err(e) => {
                                    let msg = format!("ERROR: {e}");
                                    let banner = format!("\n⚠️ {msg}\n\n");
                                    yield Ok(StreamEvent::TextDelta { content: banner });
                                    yield Ok(StreamEvent::ToolCallResult {
                                        call_id: call_id.clone(),
                                        output: msg.clone(),
                                    });
                                    auto_executed.push((
                                        call_id.clone(),
                                        tool_name.clone(),
                                        args.clone(),
                                        msg,
                                    ));
                                }
                            }
                        } else {
                            has_pass_through = true;
                        }
                    } else {
                        has_pass_through = true;
                    }
                }

                // 有透传工具（如 request_user_input）→ 上层处理，本流终止
                if has_pass_through {
                    yield Ok(StreamEvent::Done);
                    return;
                }

                // 全部 auto → 拼装 messages 进入下一轮
                let mut assistant_blocks = Vec::new();
                if !assistant_text.is_empty() {
                    assistant_blocks.push(ContentBlock::Text {
                        text: assistant_text,
                        cache_control: None,
                    });
                }
                for (id, name, args, _) in &auto_executed {
                    assistant_blocks.push(ContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: args.clone(),
                        caller: None,
                    });
                }
                msg_req.messages.push(Message {
                    role: "assistant".to_string(),
                    content: assistant_blocks,
                });

                let result_blocks: Vec<ContentBlock> = auto_executed
                    .iter()
                    .map(|(id, _, _, content)| ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: content.clone(),
                        is_error: None,
                        content_blocks: None,
                    })
                    .collect();
                msg_req.messages.push(Message {
                    role: "user".to_string(),
                    content: result_blocks,
                });

                continue 'outer;
            }
        };

        Ok(Box::new(Box::pin(s)))
    }

    fn tools(&self) -> Vec<ToolDef> {
        self.tools.clone()
    }

    fn models(&self) -> Vec<ModelInfo> {
        self.models.clone()
    }

    fn save_checkpoint(&self, state: &Checkpoint) -> Result<()> {
        if !self.checkpoint_dir.exists() {
            fs::create_dir_all(&self.checkpoint_dir)
                .context("Failed to create checkpoint directory")?;
        }
        let path = self
            .checkpoint_dir
            .join(format!("{}.json", state.session_id));
        let json = serde_json::to_string_pretty(state)?;
        fs::write(&path, json).context("Failed to write checkpoint")?;
        Ok(())
    }

    fn load_checkpoint(&self, id: &str) -> Result<Option<Checkpoint>> {
        let path = self.checkpoint_dir.join(format!("{id}.json"));
        if !path.exists() {
            return Ok(None);
        }
        let json = fs::read_to_string(&path).context("Failed to read checkpoint")?;
        let checkpoint: Checkpoint = serde_json::from_str(&json)?;
        Ok(Some(checkpoint))
    }

    fn list_sessions(&self) -> Result<Vec<String>> {
        if !self.checkpoint_dir.exists() {
            return Ok(vec![]);
        }
        let mut sessions = Vec::new();
        for entry in fs::read_dir(&self.checkpoint_dir)? {
            let entry = entry?;
            if entry
                .path()
                .extension()
                .map(|e| e == "json")
                .unwrap_or(false)
            {
                if let Some(stem) = entry.path().file_stem() {
                    sessions.push(stem.to_string_lossy().to_string());
                }
            }
        }
        Ok(sessions)
    }

    fn workspace_dir(&self) -> PathBuf {
        self.workspace.clone()
    }
}

// ============================================================================
// Tests
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use deepseek_tui::llm_client::LlmClient;
    use deepseek_tui::models::{MessageResponse, Usage};
    use futures_util::stream;

    /// 测试用 LlmClient 实现 — 返回固定文本。
    /// MockLlmClient 被 `#[cfg(test)]` 门控在 deepseek-tui 内部，
    /// 外部 crate 测试无法访问，因此这里提供最小实现。
    #[derive(Clone)]
    struct TestLlmClient {
        canned_text: String,
    }

    impl TestLlmClient {
        fn new(text: impl Into<String>) -> Self {
            Self {
                canned_text: text.into(),
            }
        }
    }

    impl LlmClient for TestLlmClient {
        fn provider_name(&self) -> &'static str {
            "test"
        }

        fn model(&self) -> &str {
            "test-model"
        }

        fn create_message(
            &self,
            _request: MessageRequest,
        ) -> impl std::future::Future<Output = Result<MessageResponse>> + Send {
            let text = self.canned_text.clone();
            async move {
                Ok(MessageResponse {
                    id: "test_msg".to_string(),
                    r#type: "message".to_string(),
                    role: "assistant".to_string(),
                    content: vec![ContentBlock::Text {
                        text,
                        cache_control: None,
                    }],
                    model: "test-model".to_string(),
                    stop_reason: Some("end_turn".to_string()),
                    stop_sequence: None,
                    container: None,
                    usage: Usage::default(),
                })
            }
        }

        async fn create_message_stream(&self, _request: MessageRequest) -> Result<StreamEventBox> {
            let text = self.canned_text.clone();
            let events: Vec<Result<DtStreamEvent>> = vec![
                Ok(DtStreamEvent::ContentBlockDelta {
                    index: 0,
                    delta: Delta::TextDelta { text },
                }),
                Ok(DtStreamEvent::MessageStop),
            ];
            Ok(Box::pin(stream::iter(events)))
        }
    }

    fn test_harness() -> DeepSeekHarness<TestLlmClient> {
        let mock = TestLlmClient::new("你好，这是一个测试回复");
        let tools = vec![ToolDef {
            name: "file_read".into(),
            description: "Read files".into(),
            parameters: serde_json::json!({}),
        }];
        let models = vec![ModelInfo {
            id: "test-model".into(),
            provider: "mock".into(),
            capability: "medium".into(),
        }];
        DeepSeekHarness::new(mock, tools, models, PathBuf::from("/tmp/test-workspace"))
    }

    #[tokio::test]
    async fn test_chat_returns_response() {
        let harness = test_harness();
        let result = harness
            .chat(ChatRequest {
                user_message: "你好".into(),
                platform_system_prompt: None,
                context: Default::default(),
                tools: vec![],
                model: None,
                session_id: None,
                previous_messages: vec![],
            })
            .await;
        assert!(result.is_ok());
        let text = result.unwrap();
        assert!(text.contains("测试回复"));
    }

    #[tokio::test]
    async fn test_chat_injects_context() {
        let harness = test_harness();
        let mut context = std::collections::HashMap::new();
        context.insert("file_path".into(), "/tmp/test.csv".into());
        let result = harness
            .chat(ChatRequest {
                user_message: "分析文件".into(),
                platform_system_prompt: Some("你是一个数据分析师".into()),
                context,
                tools: vec![],
                model: None,
                session_id: None,
                previous_messages: vec![],
            })
            .await;
        assert!(result.is_ok());
        let text = result.unwrap();
        assert!(text.contains("测试回复"));
    }

    #[test]
    fn test_tools_returns_cloned() {
        let harness = test_harness();
        let tools = harness.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "file_read");
    }

    #[test]
    fn test_models_returns_cloned() {
        let harness = test_harness();
        let models = harness.models();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "test-model");
    }

    #[test]
    fn test_checkpoint_save_and_load() {
        let dir = PathBuf::from("/tmp/test-checkpoints");
        // 清理上次残留
        let _ = std::fs::remove_dir_all(&dir);

        let harness = test_harness().with_checkpoint_dir(dir.clone());
        let checkpoint = Checkpoint {
            session_id: "test-session-1".into(),
            app_id: "test-app".into(),
            conversation_state: serde_json::json!({"turn": 1}),
            created_at: 1234567890,
        };
        harness.save_checkpoint(&checkpoint).unwrap();
        let loaded = harness.load_checkpoint("test-session-1").unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().session_id, "test-session-1");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_list_sessions() {
        let dir = PathBuf::from("/tmp/test-sessions-list");
        let _ = std::fs::remove_dir_all(&dir);

        let harness = test_harness().with_checkpoint_dir(dir.clone());
        let ckpt = Checkpoint {
            session_id: "s1".into(),
            app_id: "a".into(),
            conversation_state: serde_json::json!({}),
            created_at: 0,
        };
        harness.save_checkpoint(&ckpt).unwrap();
        let sessions = harness.list_sessions().unwrap();
        assert!(sessions.contains(&"s1".to_string()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_workspace_dir() {
        let harness = test_harness();
        assert_eq!(
            harness.workspace_dir(),
            PathBuf::from("/tmp/test-workspace")
        );
    }
}
