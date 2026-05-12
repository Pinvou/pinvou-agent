//! EngineHarness — `AgentHarness` 实现，包装 DeepSeek-TUI 的 `EngineHandle`。
//!
//! 这是「不重写 deepseek-tui 已有 engine」的正确路径：
//! - LLM 调用循环、tool dispatch、approval、capacity 管理、context compaction 等
//!   全部由 deepseek-tui 的 engine 负责
//! - pinvou-platform 只通过 `Op` / `Event` 协议跟 engine 通信
//!
//! 替代了旧的 `DeepSeekHarness` 自写 300+ 行 tool loop。
//!
//! ## 协议适配
//!
//! pinvou-platform 上层假设 `request_user_input` 走 tool_use 透传协议
//! （tool_use 出现 → 弹选择卡 → tool_result 写回 messages → 下轮 LLM）。
//! 但 deepseek-tui engine 把 `request_user_input` 当成内置工具，
//! 发出 `Event::UserInputRequired` 走 `tx_user_input` 专用通道，而不是
//! 普通 tool_use。我们这层做协议适配：
//!
//! 1. `UserInputRequired` → yield `StreamEvent::ToolCallStart` 给上层
//! 2. 立即 `Op::CancelRequest` 取消 engine 当前 turn（engine 否则会
//!    一直 wait `tx_user_input` 不结束 turn）
//! 3. `Done` 结束 stream
//! 4. 上层（web/mod.rs）接到选择卡数据 → 弹卡 → 用户选完后正常构造
//!    下一次 ChatRequest（用户选择作为 context 注入）

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use futures_util::stream::Stream;
use tokio::sync::Mutex;

use deepseek_tui::config::Config as DtConfig;
use deepseek_tui::core::engine::{spawn_engine, EngineConfig, EngineHandle};
use deepseek_tui::core::events::Event;
use deepseek_tui::core::ops::Op;
use deepseek_tui::models::{ContentBlock, Message, SystemPrompt};
use deepseek_tui::tui::app::AppMode;
use deepseek_tui::tui::approval::ApprovalMode;

use super::harness::{
    AgentHarness, ChatRequest, Checkpoint, HistoryMessage, ModelInfo, StreamEvent, ToolDef,
};

pub struct EngineHarness {
    handle: EngineHandle,
    model: String,
    workspace: PathBuf,
    tools: Vec<ToolDef>,
    models: Vec<ModelInfo>,
    /// 串行化 chat_stream 调用：rx_event 是单消费者，且 engine turn 是顺序的。
    chat_lock: Arc<Mutex<()>>,
}

impl EngineHarness {
    pub fn new(
        engine_config: EngineConfig,
        api_config: &DtConfig,
        tools: Vec<ToolDef>,
    ) -> Self {
        let model = engine_config.model.clone();
        let workspace = engine_config.workspace.clone();
        let handle = spawn_engine(engine_config, api_config);
        Self {
            handle,
            model: model.clone(),
            workspace,
            tools,
            models: vec![ModelInfo {
                id: model,
                provider: "vllm".to_string(),
                capability: "large".to_string(),
            }],
            chat_lock: Arc::new(Mutex::new(())),
        }
    }

    /// 把 pinvou-platform 的 `HistoryMessage` + `context` 转换成 engine 期望的
    /// `Message` 列表。context 用一条 system-style user message 注入到最前面。
    fn build_messages(req: &ChatRequest) -> Vec<Message> {
        let mut messages: Vec<Message> = Vec::with_capacity(req.previous_messages.len() + 1);

        if !req.context.is_empty() {
            let ctx_text: String = req
                .context
                .iter()
                .map(|(k, v)| format!("[{k}]: {v}"))
                .collect::<Vec<_>>()
                .join("\n");
            messages.push(Message {
                role: "user".into(),
                content: vec![ContentBlock::Text {
                    text: format!("## 上下文信息\n\n{ctx_text}"),
                    cache_control: None,
                }],
            });
        }

        for m in &req.previous_messages {
            messages.push(Message {
                role: m.role.clone(),
                content: vec![ContentBlock::Text {
                    text: m.content.clone(),
                    cache_control: None,
                }],
            });
        }

        messages
    }
}

#[async_trait]
impl AgentHarness for EngineHarness {
    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<Box<dyn Stream<Item = Result<StreamEvent>> + Send + Unpin>> {
        // 串行化：确保同时只有一个 chat_stream 在等 rx_event
        let _guard = self.chat_lock.clone().lock_owned().await;

        // 1. SyncSession 注入 system_prompt + messages
        let session_id = req.session_id.clone().unwrap_or_else(|| {
            // 不引入 uuid 依赖，用纳秒时间戳作 session id 足够唯一
            format!(
                "pv-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            )
        });
        let messages = Self::build_messages(&req);
        let system_prompt = req
            .platform_system_prompt
            .as_ref()
            .map(|p| SystemPrompt::Text(p.clone()));

        eprintln!(
            "[engine_harness] SyncSession msgs={} sys_prompt_len={} session_id={}",
            messages.len(),
            system_prompt
                .as_ref()
                .and_then(|sp| match sp {
                    SystemPrompt::Text(t) => Some(t.chars().count()),
                    _ => None,
                })
                .unwrap_or(0),
            session_id
        );

        self.handle
            .send(Op::SyncSession {
                session_id: Some(session_id),
                messages,
                system_prompt,
                model: self.model.clone(),
                workspace: self.workspace.clone(),
            })
            .await?;

        // 2. 发用户消息
        let reasoning_effort = std::env::var("DEEPSEEK_REASONING_EFFORT").ok();
        let user_message = req.user_message.clone();
        eprintln!(
            "[engine_harness] SendMessage content={:?} reasoning_effort={:?}",
            user_message, reasoning_effort
        );
        self.handle
            .send(Op::SendMessage {
                content: user_message,
                mode: AppMode::Agent,
                model: self.model.clone(),
                goal_objective: None,
                reasoning_effort,
                reasoning_effort_auto: false,
                auto_model: false,
                allow_shell: false,
                trust_mode: false,
                auto_approve: true,
                approval_mode: ApprovalMode::Auto,
            })
            .await?;

        // 3. 构造 Stream 从 rx_event 拉 Event 转 StreamEvent
        let handle = self.handle.clone();
        let stream = async_stream::stream! {
            // 守住 lock：stream 消费完之前不允许并发 chat_stream
            let _guard_alive = _guard;
            let mut rx = handle.rx_event.write().await;
            let t0 = std::time::Instant::now();
            let mut done_emitted = false;

            while let Some(event) = rx.recv().await {
                let dt = t0.elapsed();
                match event {
                    Event::MessageDelta { content, .. } => {
                        yield Ok(StreamEvent::TextDelta { content });
                    }
                    Event::ThinkingDelta { content, .. } => {
                        // 丢弃 thinking 段（不显示给前端）
                        let _ = content;
                    }
                    Event::ToolCallStarted { id, name, input } => {
                        eprintln!("[engine_harness +{:?}] ToolCallStarted {name} id={id}", dt);
                        yield Ok(StreamEvent::ToolCallStart {
                            call_id: id,
                            tool_name: name,
                            arguments: input,
                        });
                    }
                    Event::ToolCallComplete { id, name, result } => {
                        let output = match result {
                            Ok(r) => r.content,
                            Err(e) => format!("ERROR: {e:?}"),
                        };
                        eprintln!(
                            "[engine_harness +{:?}] ToolCallComplete {name} id={id} out_len={}",
                            dt,
                            output.len()
                        );
                        yield Ok(StreamEvent::ToolCallResult { call_id: id, output });
                    }
                    Event::UserInputRequired { id, request } => {
                        // 协议适配：engine 把 request_user_input 当内置工具走专用通道；
                        // pinvou-platform 上层用 tool_use 透传协议。这里转译：
                        //   UserInputRequired → ToolCallStart{tool_name: request_user_input}
                        // 然后 CancelRequest 取消 engine 当前 turn（否则它一直 wait
                        // tx_user_input 不结束 turn，rx_event 阻塞）。
                        let arguments =
                            serde_json::to_value(&request).unwrap_or(serde_json::Value::Null);
                        eprintln!(
                            "[engine_harness +{:?}] UserInputRequired id={id} → emit ToolCallStart + CancelRequest",
                            dt
                        );
                        yield Ok(StreamEvent::ToolCallStart {
                            call_id: id,
                            tool_name: "request_user_input".into(),
                            arguments,
                        });
                        // 释放 rx 锁后才能 send（send 需要新的可变借用）
                        drop(rx);
                        let _ = handle.send(Op::CancelRequest).await;
                        yield Ok(StreamEvent::Done);
                        done_emitted = true;
                        return;
                    }
                    Event::TurnComplete { status, error, .. } => {
                        eprintln!(
                            "[engine_harness +{:?}] TurnComplete status={:?} error={:?}",
                            dt, status, error
                        );
                        if let Some(err) = error {
                            yield Ok(StreamEvent::Error { message: err });
                        }
                        yield Ok(StreamEvent::Done);
                        done_emitted = true;
                        return;
                    }
                    Event::Error { envelope, .. } => {
                        eprintln!(
                            "[engine_harness +{:?}] Error category={:?} message={}",
                            dt, envelope.category, envelope.message
                        );
                        yield Ok(StreamEvent::Error { message: envelope.message });
                        yield Ok(StreamEvent::Done);
                        done_emitted = true;
                        return;
                    }
                    _ => {
                        // 其他 Event（MessageStarted/Complete/Thinking lifecycle/
                        // ApprovalRequired/CompactionStarted 等）暂时忽略，
                        // 不影响主流程。后续可针对性透传给前端做 UI 优化。
                    }
                }
            }

            // rx 通道关闭，engine 可能 shutdown
            if !done_emitted {
                yield Ok(StreamEvent::Done);
            }
        };

        Ok(Box::new(Box::pin(stream)))
    }

    fn tools(&self) -> Vec<ToolDef> {
        self.tools.clone()
    }

    fn models(&self) -> Vec<ModelInfo> {
        self.models.clone()
    }

    /// pinvou3 用 ConversationState（自己的状态机）做断点，不依赖 engine 内置
    /// session 持久化。这里保持兼容：no-op。
    fn save_checkpoint(&self, _state: &Checkpoint) -> Result<()> {
        Ok(())
    }

    fn load_checkpoint(&self, _id: &str) -> Result<Option<Checkpoint>> {
        Ok(None)
    }

    fn list_sessions(&self) -> Result<Vec<String>> {
        Ok(vec![])
    }

    fn workspace_dir(&self) -> PathBuf {
        self.workspace.clone()
    }
}

impl Drop for EngineHarness {
    fn drop(&mut self) {
        // engine 通过 tokio task 跑，不显式 shutdown 也会随进程退出。
        // 显式 send Shutdown 是 best-effort。
        let handle = self.handle.clone();
        tokio::spawn(async move {
            let _ = handle.send(Op::Shutdown).await;
        });
    }
}

// =============================================================================
// Harness enum：让 PinvouEngine 在 Legacy / Engine 两种路径间切换
// =============================================================================
//
// PlatformEngine<H: AgentHarness> 是 generic，但具体类型 PinvouEngine 需要单一
// 实例化。enum + trait dispatch 让我们用 env `PINVOU_USE_ENGINE_HARNESS=1`
// 在两条路径间选，无需 trait object。
//
// 切换完成、新路径验证稳定后，Phase 4 删除 Legacy variant + DeepSeekHarness。

use super::deepseek_harness::DeepSeekHarness;
use deepseek_tui::client::DeepSeekClient;

pub enum Harness {
    Legacy(DeepSeekHarness<DeepSeekClient>),
    Engine(EngineHarness),
}

#[async_trait]
impl AgentHarness for Harness {
    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<Box<dyn Stream<Item = Result<StreamEvent>> + Send + Unpin>> {
        match self {
            Self::Legacy(h) => h.chat_stream(req).await,
            Self::Engine(h) => h.chat_stream(req).await,
        }
    }

    async fn chat(&self, req: ChatRequest) -> Result<String> {
        match self {
            Self::Legacy(h) => h.chat(req).await,
            Self::Engine(h) => h.chat(req).await,
        }
    }

    fn tools(&self) -> Vec<ToolDef> {
        match self {
            Self::Legacy(h) => h.tools(),
            Self::Engine(h) => h.tools(),
        }
    }

    fn models(&self) -> Vec<ModelInfo> {
        match self {
            Self::Legacy(h) => h.models(),
            Self::Engine(h) => h.models(),
        }
    }

    fn save_checkpoint(&self, state: &Checkpoint) -> Result<()> {
        match self {
            Self::Legacy(h) => h.save_checkpoint(state),
            Self::Engine(h) => h.save_checkpoint(state),
        }
    }

    fn load_checkpoint(&self, id: &str) -> Result<Option<Checkpoint>> {
        match self {
            Self::Legacy(h) => h.load_checkpoint(id),
            Self::Engine(h) => h.load_checkpoint(id),
        }
    }

    fn list_sessions(&self) -> Result<Vec<String>> {
        match self {
            Self::Legacy(h) => h.list_sessions(),
            Self::Engine(h) => h.list_sessions(),
        }
    }

    fn workspace_dir(&self) -> PathBuf {
        match self {
            Self::Legacy(h) => h.workspace_dir(),
            Self::Engine(h) => h.workspace_dir(),
        }
    }
}
