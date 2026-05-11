//! pinvou3 平台层 — 任务编排 + TUI + 应用配置系统。
//!
//! 提供:
//! - AgentHarness trait：底层可替换 agent 的抽象边界
//! - PlatformEngine：编排主入口（任务拆解 → 逐步执行）
//! - AppConfig / AppRegistry：「应用即配置」系统
//! - ConversationState：对话状态机 + 里程碑跟踪
//! - TUI：启动器 + 对话视图 + 侧边栏 + 输入框

pub mod app;
pub mod contract;
pub mod contract_runtime;
pub mod contract_validator;
pub mod deepseek_harness;
pub mod dynamic_planner;
pub mod engine;
pub mod engine_factory;
pub mod harness;
pub mod response_checker;
pub mod reviewer;
pub mod router;
pub mod step_builder;
pub mod tui;
pub mod web;
pub mod workflow;
