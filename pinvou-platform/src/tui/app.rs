//! 平台 TUI 核心状态。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::app::{AppConfig, AppRegistry};
use crate::engine_factory::PinvouEngine;
use crate::workflow::{ConversationState, MilestoneStatus};

// === 消息模型 ===

/// 一条对话消息
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: Instant,
    pub milestone_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

// === 焦点 / 面板 ===

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Input,
    Sidebar,
    Chat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformScreen {
    Launcher,
    Conversation,
    QuitConfirm,
}

// === 引擎状态 ===

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineStatus {
    Idle,
    Thinking,
    Streaming,
    Error,
}

// === TUI App ===

pub struct PlatformApp {
    // --- 应用 ---
    /// 应用注册表（engine 持有所有权，这里存 Arc 引用）
    pub registry: Arc<AppRegistry>,
    /// 应用目录路径
    pub apps_dir: PathBuf,
    /// 当前加载的应用
    pub current_app: Option<AppConfig>,

    // --- 对话 ---
    /// 对话状态（里程碑跟踪）
    pub conv_state: Option<ConversationState>,
    /// 所有消息
    pub messages: Vec<ChatMessage>,
    /// 当前流式消息
    pub streaming_content: Option<String>,

    // --- UI ---
    pub screen: PlatformScreen,
    pub focus: Focus,
    pub sidebar_visible: bool,
    pub scroll_offset: usize,

    // --- 引擎 ---
    pub engine_status: EngineStatus,
    pub current_model: String,
    pub consecutive_out_of_scope: u32,

    // --- 异步运行时 + 编排引擎 ---
    pub runtime: tokio::runtime::Runtime,
    pub engine: PinvouEngine,

    // --- 运行标记 ---
    pub running: bool,
    pub should_quit: bool,
}

impl PlatformApp {
    pub fn new(apps_dir: PathBuf, engine: PinvouEngine) -> Self {
        let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        let registry = engine.registry.clone();

        Self {
            current_app: None,
            conv_state: None,
            messages: Vec::new(),
            streaming_content: None,
            screen: PlatformScreen::Launcher,
            focus: Focus::Input,
            sidebar_visible: true,
            scroll_offset: 0,
            engine_status: EngineStatus::Idle,
            current_model: "auto".to_string(),
            consecutive_out_of_scope: 0,
            registry,
            apps_dir,
            runtime,
            engine,
            running: true,
            should_quit: false,
        }
    }

    // === 应用操作 ===

    pub fn select_app(&mut self, app_id: &str) {
        if let Some(app) = self.registry.find(app_id).cloned() {
            // 同步 engine 状态
            if let Err(e) = self.engine.load_app(app_id) {
                self.add_system_message(&format!("加载应用失败: {e}"));
                return;
            }

            let conv_state = ConversationState::new(app_id.to_string(), app.milestones.clone());
            self.conv_state = Some(conv_state);
            self.current_app = Some(app);
            self.screen = PlatformScreen::Conversation;
            self.messages.clear();
            self.streaming_content = None;
            self.scroll_offset = 0;

            self.add_system_message(&format!(
                "已进入「{}」模式。{} 你可以随时输入消息，或点击右侧步骤来引导流程。",
                self.current_app
                    .as_ref()
                    .map(|a| a.name.as_str())
                    .unwrap_or(""),
                self.current_app
                    .as_ref()
                    .map(|a| a.description.as_str())
                    .unwrap_or("")
            ));
        }
    }

    pub fn back_to_launcher(&mut self) {
        self.screen = PlatformScreen::Launcher;
        self.current_app = None;
        self.conv_state = None;
        self.messages.clear();
        self.streaming_content = None;
        self.engine_status = EngineStatus::Idle;
    }

    // === 消息操作 ===

    pub fn add_message(
        &mut self,
        role: MessageRole,
        content: String,
        milestone_id: Option<String>,
    ) {
        self.messages.push(ChatMessage {
            role,
            content,
            timestamp: Instant::now(),
            milestone_id,
        });
    }

    pub fn add_user_message(&mut self, content: String) {
        self.add_message(MessageRole::User, content, None);
        if let Some(ref mut cs) = self.conv_state {
            cs.increment_turn();
        }
    }

    pub fn add_assistant_message(&mut self, content: String) {
        self.add_message(MessageRole::Assistant, content, None);
    }

    pub fn add_system_message(&mut self, content: &str) {
        self.add_message(MessageRole::System, content.to_string(), None);
    }

    pub fn append_streaming(&mut self, delta: &str) {
        if let Some(ref mut content) = self.streaming_content {
            content.push_str(delta);
        } else {
            self.streaming_content = Some(delta.to_string());
        }
    }

    pub fn finalize_streaming(&mut self) {
        if let Some(content) = self.streaming_content.take() {
            self.add_assistant_message(content);
        }
    }

    // === 里程碑操作 ===

    pub fn mark_milestone_done(&mut self, milestone_id: &str) {
        if let Some(ref mut cs) = self.conv_state {
            cs.mark_done(milestone_id);
        }
    }

    pub fn skip_milestone(&mut self, milestone_id: &str) {
        if let Some(ref mut cs) = self.conv_state {
            cs.skip(milestone_id);
        }
    }

    pub fn active_milestone(&self) -> Option<String> {
        self.conv_state
            .as_ref()
            .and_then(|cs| cs.active_milestone().map(|m| m.id.clone()))
    }

    // === 焦点 ===

    pub fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Input => Focus::Sidebar,
            Focus::Sidebar => Focus::Chat,
            Focus::Chat => Focus::Input,
        };
    }

    /// 构建注入 LLM 的完整 prompt
    pub fn build_augmented_prompt(&self, user_input: &str) -> String {
        let mut parts = Vec::new();

        if let Some(ref app) = self.current_app {
            if let Some(ref prompt) = app.prompt {
                parts.push(format!("## 角色\n{prompt}"));
            }
        }

        if let Some(ref cs) = self.conv_state {
            if let Some(ctx) = cs.context_prompt() {
                parts.push(ctx);
            }
            if let Some(phase) = cs.phase_prompt() {
                parts.push(phase);
            }
        }

        parts.push(format!("## 用户消息\n{user_input}"));

        parts.join("\n\n")
    }

    /// 获取里程碑建议列表
    pub fn milestone_suggestions(&self) -> Vec<(String, String, MilestoneStatus)> {
        self.conv_state
            .as_ref()
            .map(|cs| {
                cs.suggestions()
                    .into_iter()
                    .map(|s| (s.id, s.label, s.status))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 同步 engine 的 conv_state 到 TUI
    pub fn sync_engine_state(&mut self) {
        if let Some(ref engine_cs) = self.engine.conv_state {
            self.conv_state = Some(engine_cs.clone());
        }
    }
}
