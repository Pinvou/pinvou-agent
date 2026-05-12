//! pinvou3 平台层 — 任务编排 + Web UI。
//!
//! ## 模块图
//! - `agent_registry`：扫 `prompts/*.md` 注册 agent
//! - `combined_planner`：单次 LLM 调用同时输出 agent + milestones
//! - `rollback`：slash 命令 + 状态机回退
//! - `contract` + `contract_runtime` + `contract_validator`：硬边界
//! - `engine`：编排主入口（`ensure_combined_plan` 等）
//! - `workflow`：`ConversationState` / `GlobalMode` / `Milestone`
//! - `step_builder`：阶段 prompt 渲染
//! - `router`：模型路由（small / medium / large）
//! - `web`：Axum SSE 入口
//!
//! ## 边界
//! - `harness::AgentHarness` trait：可替换底层 agent（DeepSeek / OpenCode / Mock）
//! - `deepseek_harness`：trait 的 DeepSeek-TUI 实现 + 工具自动执行循环
//! - `engine_factory`：从环境变量构建生产引擎，注册 DeepSeek-TUI 工具池

pub mod agent_registry;
pub mod combined_planner;
pub mod contract;
pub mod contract_runtime;
pub mod contract_validator;
pub mod deepseek_harness;
pub mod engine;
pub mod engine_factory;
pub mod engine_harness;
pub mod harness;
pub mod rollback;
pub mod router;
pub mod step_builder;
pub mod web;
pub mod workflow;
