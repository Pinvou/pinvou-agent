//! pinvou3 平台层 — 任务编排 + TUI + Web。
//!
//! ## 当前主路径（新设计）
//! - `agent_registry`：扫 `prompts/*.md` 注册 agent
//! - `combined_planner`：单次 LLM 调用同时输出 agent + milestones
//! - `rollback`：slash 命令 + 状态机回退
//! - `contract` + `contract_runtime` + `contract_validator`：硬边界
//! - `engine::ensure_combined_plan`：新设计入口
//! - `workflow::GlobalMode`：QnA / Planning / Executing / Replan / Done
//!
//! ## Legacy（计划 P1 退役）
//! - `app` (AppConfig / AppRegistry)：被 `agent_registry` 替代
//! - `dynamic_planner`：被 `combined_planner` 替代
//! - `response_checker`：[OK]/[MORE]/[BLOCKED] 信号解析，被 contract 系统替代
//! - `reviewer`：LLM 拆解审阅，新版结构性校验替代
//! - `engine::ensure_plan_initialized` / `decompose_and_execute`
//!
//! Legacy 模块仍在主路径上作为 fallback；待新路径稳定后逐步删除。
//!
//! ## 边界
//! - `harness::AgentHarness` trait：可替换底层 agent（DeepSeek、OpenCode、Mock）
//! - `deepseek_harness`：trait 的 DeepSeek 实现
//! - `engine_factory`：从环境变量构建生产引擎
//! - `tui` / `web`：两种前端

pub mod agent_registry;
pub mod app;
pub mod combined_planner;
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
pub mod rollback;
pub mod router;
pub mod step_builder;
pub mod tui;
pub mod web;
pub mod workflow;
