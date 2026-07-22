//! Tauri 命令实现。前端通过 `invoke(name, args)` 调到这里。
//!
//! 暴露的命令：
//! - `chat(message)`         — 发送用户消息（流式响应通过 chat:* 事件）
//! - `get_settings()`        — 读 `~/.pinvou3/settings.json`（UserPrefs）
//! - `update_settings(prefs)`— 写盘；GUI 项立即生效，引擎相关项需重启 app
//! - `clear_session()`       — 清前端显示（MVP）；后端 session 重启 app 才真清
//! - `get_monitor_snapshot()`— Monitor 视图完整数据
//! - `get_backend_status()`  — ChatRoom 顶部 live dot 用，简版健康指示
//! - `discover_local_vllm()` — 设置页手动探测本机 vLLM 候选端点
//!
//! 阶段 C 新增（多对话历史）：
//! - `list_sessions()` / `create_session()` / `load_session(id)`
//! - `delete_session(id)` / `rename_session(id, title)` / `get_active_session()`

use deepseek_tui::models::Message;
use deepseek_tui::session_manager::{SavedSession, SessionMetadata};
use deepseek_tui::tools::user_input::{UserInputAnswer, UserInputResponse};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::bridge::mode_state::{SerializableMode, SessionModeState};
use crate::bridge::prefs::{SavedModel, SearchProvider, UserPrefs};
use crate::bridge::sessions::{SessionKind, SessionStore};
use crate::credential_store::{
    CredentialEditAction, CredentialState, CredentialStore, SystemCredentialStore,
};
use crate::engine_pool::EnginePool;
use crate::features::monitor::{MonitorSnapshot, MonitorState, VllmStatus};
use crate::knowledge::KnowledgeService;

// 命令实现按业务域拆分，但通过 include! 保持在同一个 commands 模块中。
// 这样既缩小单文件冲突面，也不会改变 Tauri 命令名、参数协议和现有 Rust 调用路径。
include!("commands/sessions.rs");
include!("commands/chat.rs");
include!("commands/llmapi.rs");
include!("commands/knowledge.rs");
include!("commands/attachments.rs");
include!("commands/settings.rs");
include!("commands/voice.rs");
include!("commands/monitor.rs");
include!("commands/runtime.rs");
include!("commands/connectors.rs");
include!("commands/memory.rs");
include!("commands/artifacts.rs");
include!("commands/files.rs");
include!("commands/interaction.rs");
include!("commands/personas.rs");
include!("commands/workflows.rs");
include!("commands/tests.rs");
include!("commands/marketplace.rs");
include!("commands/timeline.rs");
