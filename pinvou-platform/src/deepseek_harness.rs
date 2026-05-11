//! DeepSeekHarness — 实现 AgentHarness trait，对接本地和远程 LLM。
//!
//! 泛型参数 C: LlmClient 支持注入 DeepSeekClient（生产）、Ollama client、MockLlmClient（测试）。

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures_util::{Stream, StreamExt};

use super::harness::{AgentHarness, ChatRequest, Checkpoint, ModelInfo, StreamEvent, ToolDef};
use deepseek_tui::llm_client::{LlmClient, StreamEventBox};
use deepseek_tui::models::{
    ContentBlock, ContentBlockStart, Delta, Message, MessageRequest, StreamEvent as DtStreamEvent,
    SystemPrompt, Tool,
};

pub struct DeepSeekHarness<C: LlmClient> {
    client: C,
    tools: Vec<ToolDef>,
    models: Vec<ModelInfo>,
    workspace: PathBuf,
    checkpoint_dir: PathBuf,
}

impl<C: LlmClient> DeepSeekHarness<C> {
    pub fn new(client: C, tools: Vec<ToolDef>, models: Vec<ModelInfo>, workspace: PathBuf) -> Self {
        let checkpoint_dir = workspace.join(".checkpoints");
        Self {
            client,
            tools,
            models,
            workspace,
            checkpoint_dir,
        }
    }

    pub fn with_checkpoint_dir(mut self, dir: PathBuf) -> Self {
        self.checkpoint_dir = dir;
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
impl<C: LlmClient> AgentHarness for DeepSeekHarness<C> {
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
        use std::collections::HashMap;
        use std::sync::{Arc, Mutex};

        let msg_req = self.to_message_request(&req);
        let llm_stream: StreamEventBox = self.client.create_message_stream(msg_req).await?;

        // 流式 tool call: ToolUse 的 input 初始为空，参数通过 InputJsonDelta 增量传输
        // index → (call_id, tool_name, accumulated_json_buffer)
        type ToolState = (String, String, String);
        let tool_state: Arc<Mutex<HashMap<u32, ToolState>>> = Arc::new(Mutex::new(HashMap::new()));
        let idx_map = tool_state.clone();

        let mapped = llm_stream.map(move |result| match result {
            Ok(DtStreamEvent::ContentBlockStart {
                index,
                content_block,
            }) => match content_block {
                ContentBlockStart::ToolUse { id, name, .. } => {
                    if let Ok(mut map) = idx_map.lock() {
                        map.insert(index, (id, name, String::new()));
                    }
                    // 通知前端 tool call 正在生成，避免用户以为卡住
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
                    let evt = StreamEvent::ToolCallStart {
                        call_id,
                        tool_name,
                        arguments: args,
                    };
                    eprintln!("[harness] accumulated tool call: {evt:?}");
                    return Ok(evt);
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
