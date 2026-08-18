//! Tauri 命令实现。前端通过 `invoke(name, args)` 调到这里。
//!
//! 暴露的命令：
//! - `chat(message)`         — 发送用户消息（流式响应通过 chat:* 事件）
//! - `get_settings()`        — 读 `~/.pinvou3/settings.json`（UserPrefs）
//! - `update_settings(patch)`— 字段级写盘；GUI 项立即生效，引擎相关项需重启 app
//! - `clear_session()`       — 清前端显示（MVP）；后端 session 重启 app 才真清
//! - `get_monitor_snapshot()`— Monitor 视图完整数据
//! - `get_backend_status()`  — ChatRoom 顶部 live dot 用，简版健康指示
//! - `discover_local_vllm()` — 设置页手动探测本机 vLLM 候选端点
//!
//! 阶段 C 新增（多对话历史）：
//! - `list_sessions()` / `create_session()` / `load_session(id)`
//! - `delete_session(id)` / `rename_session(id, title)` / `get_active_session()`

pub(crate) mod acp_providers;
pub(crate) mod artifacts;
pub(crate) mod assistant_response;
pub(crate) mod attachments;
pub(crate) mod chat;
pub(crate) mod codex;
pub(crate) mod connectors;
pub(crate) mod dependencies;
pub(crate) mod files;
pub(crate) mod interaction;
pub(crate) mod knowledge;
pub(crate) mod local_llm;
pub(crate) mod marketplace;
pub(crate) mod memory;
pub(crate) mod monitor;
pub(crate) mod multiagent;
pub(crate) mod personas;
pub(crate) mod pet;
mod prelude;
pub(crate) mod remote_control;
pub(crate) mod remote_knowledge;
pub(crate) mod runtime;
pub(crate) mod scheduled;
pub(crate) mod sessions;
pub(crate) mod settings;
pub(crate) mod shared_knowledge_host;
pub(crate) mod startup;
pub(crate) mod timeline;
pub(crate) mod updater;
pub(crate) mod voice;

#[cfg(test)]
mod protocol_tests;
#[cfg(test)]
mod tests;
