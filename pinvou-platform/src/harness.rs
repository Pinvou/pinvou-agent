//! AgentHarness trait — Platform 对底层 agent 的接口抽象。
//!
//! 这个 trait 就是"可替换底层"的边界。
//! 换 agent 后端只需重新实现这个 trait。
//!
//! Phase 2 将在此实现 DeepSeekEngine，当前仅定义接口。

#![allow(dead_code)] // Phase 1 定义类型，Phase 2 使用

use anyhow::Result;
use async_trait::async_trait;
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// 流式事件类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StreamEvent {
    /// 文本增量
    TextDelta { content: String },
    /// 工具调用开始
    ToolCallStart {
        call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
    },
    /// 工具调用结果
    ToolCallResult { call_id: String, output: String },
    /// 错误
    Error { message: String },
    /// 流结束
    Done,
}

/// 历史消息 — 平台无关的轻量表示
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessage {
    pub role: String,
    pub content: String,
}

/// 聊天请求
#[derive(Debug, Clone)]
pub struct ChatRequest {
    /// 用户消息
    pub user_message: String,
    /// 系统 prompt（平台层注入的，独立于 agent 自身的 system prompt）
    pub platform_system_prompt: Option<String>,
    /// 对话上下文（用于注入额外信息，如列名、之前结论等）
    pub context: HashMap<String, String>,
    /// 可用工具列表（平台级过滤后的）
    pub tools: Vec<ToolDef>,
    /// 指定模型（可选，不指定则用默认）
    pub model: Option<String>,
    /// 会话 ID（用于断点恢复）
    pub session_id: Option<String>,
    /// 之前的历史消息，按时间从旧到新
    pub previous_messages: Vec<HistoryMessage>,
}

/// 工具定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// 模型信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub provider: String,
    /// 模型能力标签: "small" / "large" / "embedding"
    pub capability: String,
}

/// 检查点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub session_id: String,
    pub app_id: String,
    pub conversation_state: serde_json::Value,
    pub created_at: i64,
}

/// Platform 对底层 agent 的接口要求。
///
/// 实现者: DeepSeekEngine (包装现有 engine), MockEngine (测试), 未来可换 OpenCode/ClawCode。
#[async_trait]
pub trait AgentHarness: Send + Sync {
    /// 发送消息并获取流式响应
    async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<Box<dyn Stream<Item = Result<StreamEvent>> + Send + Unpin>>;

    /// 同步聊天（等待完整响应）
    async fn chat(&self, req: ChatRequest) -> Result<String> {
        use futures_util::StreamExt;
        let mut stream = self.chat_stream(req).await?;
        let mut output = String::new();
        while let Some(event) = stream.next().await {
            if let Ok(StreamEvent::TextDelta { content }) = event {
                output.push_str(&content);
            }
        }
        Ok(output)
    }

    /// 获取可用工具列表
    fn tools(&self) -> Vec<ToolDef>;

    /// 获取可用模型列表
    fn models(&self) -> Vec<ModelInfo>;

    /// 会话检查点
    fn save_checkpoint(&self, state: &Checkpoint) -> Result<()>;
    fn load_checkpoint(&self, id: &str) -> Result<Option<Checkpoint>>;

    /// 列出历史会话
    fn list_sessions(&self) -> Result<Vec<String>>;

    /// 当前工作目录
    fn workspace_dir(&self) -> PathBuf;
}
