//! 多智能体（会话内主动委派）的领域模块，见 ADR-0006。
//!
//! 多智能体 = 普通会话能力 + 主动委派 + 专家可视化 + 只读/执行权限：
//! - 委派实例（子智能体）的身份、任务摘要与状态由**底座自己的落盘记录**承载
//!   （worker ledger + subagent transcripts），App 零新增持久化——读取投影见
//!   [`transcripts`]；
//! - 专家名册装配见 [`roster`]；工作区 git 初始化（并行子任务 spawn 的前置）
//!   见 [`platform`]；
//! - Workflow 专属运行台账（run.json 状态机、attempt tracker、进程租约、审批
//!   落盘）已随"每图必停/唯一协议"的旧设计整体退役且未曾发布，无遗留数据。
//!   `workflow` 工具保持主线原状（底座 subagents_enabled 连带注册，对所有
//!   会话可用）：不禁用、也不在委派提醒里教学或推荐——底座把 read_only 子
//!   任务钳成四个本地文件工具、结构化阶段默认不传递上游结果两处已知限制
//!   记录于 ADR-0006。

pub(crate) mod platform;
pub mod roster;
pub mod transcripts;
