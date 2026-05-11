# pinvou3 现状与待办

> 最后更新: 2026-05-11

---

## 一、当前状态

### 已完成的

| 模块 | 说明 |
|------|------|
| AppConfig + AppRegistry | TOML 加载正常，含 granularity/confirm_at/ban_list 字段 |
| ConversationState | 里程碑 Pending→Active→Done 生命周期正常 |
| StepBuilder | 旧版 step prompt + contract prompt 渲染，含 request_user_input 引导 |
| ResponseChecker | [OK]/[MORE]/[BLOCKED] 信号解析 + NextAction 路由 |
| DeepSeekHarness | 非流式 chat() + SSE 流式 chat_stream() |
| Web SSE 流式 | 逐 token 渲染 + Markdown + choice_request 检测 |
| Web 侧边栏 | 里程碑渲染，从服务端 /api/milestones 加载（不再硬编码） |
| Web Choice Card | 前端 choice card 组件 + `/api/chat/stream` tool_result 回传 |
| Choice Result 状态事件 | tool_result 会写入 ConversationState.context，并完成当前里程碑 |
| fine 模式继续语义 | choice 后“继续”用于启动下一阶段；阶段完成后的“继续”用于推进下一里程碑 |
| 3 个 app.toml | 均含 planning 配置、milestone contracts、request_user_input 工具 + granularity 配置 |
| granularity 控制 | fine/medium/coarse 三种自动推进策略已实现 |
| engine.decompose_and_execute() | 拆解→审阅→转换 流程已实现，作为较早 helper 保留；Web 主路径不再依赖该方法 |
| Milestone Contract Runtime | app.toml 提供阶段 contract，动态拆解只能特化模板标题/提示/context，运行时按 contract 决策 |
| DynamicPlanner Web 接入 | Web 第一条用户消息优先生成动态计划，失败回退静态 contracts |
| ContractValidator | 工具调用和输出按阶段契约做硬边界检查 |

### 后续优化

| 项 | 说明 |
|----|------|
| ban_list 迁移到 app.toml | AppConfig 已有 ban_list 字段，StepBuilder 会合并使用，但当前 TOML 中未配置自定义 ban 规则 |
| 消息历史去重 | 已核实 — build_request/stream_chat 无 N+1 重复，previous_messages 与 user_message 正确分离 |

---

## 二、本轮契约运行时改动清单

### 代码

| 文件 | 改动 |
|------|------|
| `contract.rs` | 新增 `MilestoneContract`、planning 配置、阶段模式、推进策略和 output requirements |
| `contract_runtime.rs` | 新增按 contract 输出 `TurnDirective` 的运行时；问题预算、阶段动作、最终输出工具边界都由契约决定 |
| `contract_validator.rs` | 新增工具调用和阶段输出机械校验，拦截越界工具、无效选择题、开放问题等 |
| `dynamic_planner.rs` | 新增首轮动态拆解解析与静态模板回退 |
| `step_builder.rs` | 新增 contract prompt 渲染，保留旧 step prompt 兼容路径 |
| `engine.rs` | 初始化动态/静态计划，生成下一轮 contract prompt，并在缺失状态时补建 `ConversationState` |
| `web/mod.rs` | Web SSE 主路径接入 dynamic planner、contract runtime、contract validator；阻断非法工具调用和超预算问题 |
| `apps/*/app.toml` (3) | 新增 planning 配置和里程碑 contracts，覆盖计划敲定、文档生成、数据分析 |

### 文档

| 文件 | 改动 |
|------|------|
| `设计架构文档-pinvou3.md` | 增补 Milestone Contract Runtime 说明，并把模块清单更新为当前事实 |
| `process.md` | 更新当前状态、Web 主路径和契约运行时改动清单 |

---

## 三、本轮设计评审收敛

### P0：主编排协议先收敛

`设计架构文档-pinvou3.md` 已把主编排从“自然语言步骤 + LLM 自评推进”收敛为 `MilestoneContract + ContractRuntime + ContractValidator`：

- `MilestoneContract` 当前字段为 `mode`、`question_budget`、`required_context`、`produced_context`、`allowed_tools`、`forbidden_tools`、`output_requirements`、`advance_policy`。
- 静态 `app.toml` 是 contract 来源；动态拆解必须完整复用静态模板 id 和 mode，只能特化 `label`、`prompt_hint`、`required_context`、`produced_context`。
- `ContractRuntime` 负责本轮动作和问题预算，`ContractValidator` 负责工具边界、选择题形状和输出形态。
- Web 主路径只注册 `/api/chat/stream`；旧的非流式 `/api/chat` 不注册，避免绕过 runtime / validator。

### P1：后续串行恢复能力

以下能力是后续增强，不属于本轮已落地范围：

- 计划确认页允许用户编辑 contract，但只修改当前会话计划，不回写全局 app 配置。
- 当前步骤遇到连续阻塞或用户反馈太粗时，只细分当前 contract，并经用户确认后 splice 回计划。
- 跳过、回退、断连恢复都必须通过显式状态迁移和 checkpoint 记录。
- checkpoint 至少保存 active contract、contract 状态、context、最近输入输出和工具结果摘要。

### P2：暂缓项

`SubAgentRouter` 暂不进入 P0/P1。串行 contract 编排稳定前，不做并行子任务分发。

进入 P2 的条件：

- P0/P1 的状态机和 checkpoint 恢复路径稳定。
- 单任务 contract 的工具边界、输出验收和阻塞恢复已经可预测。
- UI 能清楚展示多个子任务的状态、错误和合并结果。

P2 初步范围：

- 只允许拆分互不依赖的只读分析、资料整理、对比评审类任务。
- 每个 sub-agent 必须拿到独立 `MilestoneContract`，不能共享可写文件范围。
- 父编排只负责分发、等待、汇总和冲突处理，不把子 agent 的自评直接当作完成依据。
