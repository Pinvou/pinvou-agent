# pinvou3 设计架构文档

> 基于 DeepSeek-TUI 二次开发的本地 AI 平台 TUI。
> 运行于 NVIDIA GB10，面向普通用户，覆盖数据分析、文档生成、计划敲定等日常场景。
>
> 本文档是 pinvou3 的总纲领，所有实现决策以此为准。

---

## 一、背景与定位

### 1.1 从 pinvou2 到 pinvou3

| | pinvou2 | pinvou3 |
|---|---|---|
| **架构** | Python + Electron + Docker + vLLM + Claw-Code | Rust 单体 + ratatui TUI |
| **运行时组件** | 6 层（Python host、Claw-Code x2、vLLM、proxy x2、Docker xN、Electron） | 1 层（单二进制） |
| **分发** | .run 自解压包 + pip + npm + docker build | 单二进制，cargo build 即得 |
| **隔离方式** | Docker 容器 | Skills / Sub-agent / MCP |
| **工作流** | 硬性三段式 gate（Plan -> Bridging -> Execute） | 对话状态机，无固定 gate |
| **任务拆解** | Claw LLM (Plan) + Bridging LLM (翻译 -> cards) | 动态 contract + 按需确认 + 逐步执行 |
| **颗粒度修正** | 超时 1200s x3 -> Refine -> Claw 细分 | 越界即截断 + LLM 自评 [OK]/[MORE]/[BLOCKED] |
| **目标用户** | 技术用户（编码代理） | 普通用户（通用 AI 平台） |

### 1.2 核心定位

- **通用 AI 平台**（非纯编码工具），coding 占比小
- 面向**非专业用户**：数据分析、文档生成、计划敲定
- 纯 TUI 界面，**对话为主**，侧边栏为可选步骤导航
- **单二进制分发**，开箱即用

### 1.3 关键约束

- **本地 LLM 质量有限**（Qwen 7B-35B on GB10）
- 不能依赖 LLM 自主管理复杂流程
- 不能依赖 LLM 在该停的时候自己停
- **每轮只让 LLM 做一件限定范围的小事**

---

## 二、总体架构

### 2.1 分层架构

```
+------------------------------------------+
|         crates/tui/src/platform/         |
|                                          |
|  +-------------+  +-------------------+  |
|  | App 配置系统  |  |  StepBuilder      |  |
|  | (toml+prompt)|  |  (Prompt 构造)     |  |
|  +-------------+  +-------------------+  |
|  +-------------+  +-------------------+  |
|  | LLM Reviewer |  |  ResponseChecker   |  |
|  | (拆解语义审阅) |  |  (LLM 自评路由)    |  |
|  +-------------+  +-------------------+  |
|  +-------------+  +-------------------+  |
|  | ModelRouter  |  |  AgentHarness     |  |
|  | (模型路由)    |  |  (底层替换边界)     |  |
|  +-------------+  +-------------------+  |
|  +-----------------------------------+   |
|  |  ConversationState (对话状态机)     |   |
|  +-----------------------------------+   |
|  +-----------------------------------+   |
|  |  TUI (启动器 + 对话 + 侧边栏 + 输入)  |   |
|  +-----------------------------------+   |
|             PlatformEngine                |
|           (编排主入口)                     |
+--------------+---------------------------+
               | AgentHarness trait
               v
+------------------------------------------+
|       底层 Agent（可替换）                  |
|  DeepSeek-TUI engine / OpenCode / 其他    |
|  (原有代码，不改逻辑)                       |
+------------------------------------------+
```

### 2.2 核心原则

1. **扩展而非替换**：不修改原有 crate 逻辑。仅加 1 行 `mod platform;` 和 1 个 `[[bin]]`。
2. **底层可替换**：`AgentHarness` trait 是边界。换 OpenCode 只需重新实现此 trait。
3. **对话永远是主角**：工作流是侧边栏的可选建议，不是必经之路。
4. **应用即配置**：新场景 = 一个目录 + 一个 `app.toml` + 一个 `prompt.md`。零代码。

---

## 三、任务拆解与执行流程（核心编排）

### 3.1 职责划分

**代码控流程，LLM 做语义工作，用户做关键决策。**

LLM 的 `[OK]` / `[MORE]` / `[BLOCKED]` 只能作为执行反馈，不能作为唯一推进依据。每个里程碑都有 `MilestoneContract`，代码按 contract 校验工具边界、问题预算、输出形态和推进策略。

| | 代码 | LLM | 用户 |
|---|---|---|---|
| App 选择 | Web 请求携带 `app_id`，服务端加载对应 app | - | 前端选择 App |
| 任务拆解 | 校验动态计划必须完整复用静态模板 id 和 mode | 特化里程碑 label / prompt_hint / context | - |
| 每步 Prompt 构造 | `StepBuilder` 按 contract 限定范围 | - | - |
| 每步实际执行 | - | 拿到限定范围 Prompt 完成任务 | - |
| 每步完成判断 | `ResponseChecker` 解析信号，`ContractValidator` 校验工具和输出 | 自评 `[OK]` / `[MORE]` / `[BLOCKED]` | - |
| 推进/等待决定 | `ContractRuntime` 按 `advance_policy` 输出下一动作 | - | 需要决策时确认 |
| 越界检测 | 按 `allowed_tools` 和 `output_requirements` 机械检测 | - | - |

### 3.2 完整流程

```
用户输入
    |
    v
+-- Step 1: App 选择 (Web) -------------------+
|  Web 请求携带 app_id，服务端加载对应 app.toml  |
+---------------------------------------------+
    |
    v
+-- Step 2: 任务拆解 (LLM) -------------------+
|  发给 LLM 拆解 prompt:                      |
|    - 当前应用 + 可用工具 + 已知上下文          |
|    - app.toml 静态模板 id / label / mode      |
|    - 必须完整复用模板，不能新增/删除/重排 id    |
|                                             |
|  LLM 输出动态计划 JSON，用于特化模板标题和提示 |
+---------------------------------------------+
    |
    v
+-- Step 3: 可解析性检查 (代码) ---------------+
|  判断「能不能用」(不判断「好不好」):           |
|    - 能解析为 JSON?                           |
|    - id 是否完整复用静态模板且顺序一致?         |
|    - contract_mode 是否和静态模板一致?          |
|    - label 是否非空?                           |
|                                             |
|  能 -> 使用动态计划                            |
|  不能 -> 回退 app.toml 静态 milestones          |
+---------------------------------------------+
    |
    v
+-- Step 4: 运行时决策 (ContractRuntime) -------+
|  侧边栏渲染动态里程碑                         |
|  根据当前 active milestone contract 决定:      |
|    - CallLlm                                  |
|    - Blocked                                  |
|    - AskUser / CompleteStep（保留扩展）         |
+---------------------------------------------+
    |
    v
+-- Step 5: 逐步执行 (LLM 自评 + 硬边界校验) ----+
|  对当前 active 里程碑:                       |
|                                             |
|  a. StepBuilder 构造小范围 prompt:            |
|     - 当前 contract (只做这个)                 |
|     - 已知上下文                              |
|     - allowed_tools / output_requirements      |
|                                             |
|  b. LLM 执行 -> 完成任务                     |
|                                             |
|  c. LLM 自评 (在回复末尾附加信号):             |
|     [OK]      这步完成，可以推进               |
|     [MORE]    还需要继续，原因: {具体要做什么}   |
|     [BLOCKED] 卡住了，原因: {卡在哪}            |
|                                             |
|  d. ContractValidator 硬校验:                 |
|     - LLM 自评信号                             |
|     - output_requirements 是否满足             |
|     - tool calls 是否在 allowed_tools 内        |
|     - question_budget 是否超限                 |
|                                             |
|  e. UI 刷新侧边栏状态；choice_result 推进阶段   |
+---------------------------------------------+
```

### 3.3 MilestoneContract（P0 核心协议）

静态 `app.toml` 是 contract 的来源。动态拆解只能完整复用静态模板 id 和 mode，并特化 `label`、`prompt_hint`、`required_context`、`produced_context`；不能改写工具边界、问题预算、输出要求或推进策略。

```
MilestoneContract {
  mode: MilestoneMode,
  question_budget: u8,
  required_context: Vec<String>,
  produced_context: Vec<String>,
  allowed_tools: Vec<String>,
  forbidden_tools: Vec<String>,
  output_requirements: Vec<OutputRequirement>,
  advance_policy: AdvancePolicy,
}

MilestoneMode:
  collect                  收集信息，通常只允许 request_user_input
  produce_options          产出 2-3 个方案并让用户选择
  refine_selected_option   基于已选方案细化，适用于计划敲定
  final_output             最终输出或导出
  freeform                 按阶段 hint 完成任务，适合文档草稿、数据分析、可视化

AdvancePolicy:
  on_choice       用户选择后完成当前阶段
  on_valid_output 输出通过 contract 校验后完成阶段
  manual_continue 保留手动推进
```

### 3.4 DynamicPlanner Prompt（Step 2）

```
用户想: "{user_request}"
当前应用: {app_name} -- {app_description}
静态模板:
- id=goal label=明确目标 mode=collect
- id=constraints label=梳理约束 mode=collect
- id=options label=方案对比 mode=produce_options
- id=detail label=细化方案 mode=refine_selected_option
- id=output label=输出计划书 mode=final_output

输出 JSON：必须完整输出每个模板 id，顺序一致，不能新增、删除或重排；`contract_mode` 必须与模板 mode 一致。
```

### 3.5 执行反馈与校验矩阵

```
每步执行完成后，LLM 在回复末尾附加自评信号：

[OK]
  这步已完成。所有产出物已生成，无需继续。
  注意: 这只是信号，不直接等于推进。

[MORE] 还需要: {具体还要做什么}
  这步只完成了一部分，需要继续。

[BLOCKED] 原因: {卡在哪}
  无法继续执行。
```

当前硬边界：

| 条件 | 行为 | 原因 |
|------|------|------|
| 工具调用越过 `allowed_tools` | 立即返回错误并停止本轮 SSE | 工具边界是硬约束 |
| `request_user_input` 选项数不满足 `min_options` / `max_options` | 不发 choice card | 防止无效选择题进入前端 |
| 阶段超过 `question_budget` | 返回 blocked 文案 | 防止反复追问 |
| 输出缺少 `must_contain_table` / `must_contain_schedule` / `must_contain_risk_section` | 不自动推进；有 choice_request 也不发给前端 | 输出不满足阶段契约 |
| 输出尾部出现开放式追问 | 不自动推进 | 用户做选择，不做作文 |

### 3.6 交互模式：选择题优于问答题

**核心原则：用户做决策，不做作文。**

```
反模式（当前）:
  LLM: "请描述你的数据文件结构、你想分析什么维度、你关心什么指标？"
  用户: 需要组织语言，写一段话回答

正确模式:
  LLM: 调用 request_user_input({
    questions: [{
      header: "分析维度",
      question: "你想从哪个角度分析？",
      options: [
        {label: "销售趋势", description: "按时间展示销售额变化"},
        {label: "地区对比", description: "不同地区的销售差异"},
        {label: "都看看", description: "同时展示趋势和对比"}
      ]
    }]
  })
  用户: 按一个数字键
```

**实现机制：复用 DeepSeek-TUI 的 `request_user_input` 工具。**

DeepSeek-TUI 已有完整实现（`crates/tui/src/tools/user_input.rs` + `crates/tui/src/tui/user_input.rs`）：
- Tool spec 定义：1-3 题，每题 2-3 选项 + "Other" 自定义输入
- TUI 模态框：数字键快速选择，上下键移动，Esc 取消
- Engine 集成：`await_user_input()` 挂起 agent loop，等待用户响应后返回 LLM

**Platform 层当前职责：**
1. 将 `request_user_input` 加入 AppConfig.tools 白名单或阶段 contract 的 `allowed_tools`
2. 在 contract prompt 中引导 LLM 调用此工具：「当需要用户决策时，调用 `request_user_input` 工具，提供 2-3 个选项，不要问开放式文字题」
3. 由 `ContractValidator` 校验选择题形状和工具边界，Web 在发出 choice card 前预留问题预算

**Web 前端适配：** 当 LLM 调用 `request_user_input` 时，前端渲染选择卡片（对标 TUI 模态框），用户点击后结果以 tool result 形式返回 LLM。

### 3.7 Milestone Contract Runtime

静态 `app.toml` 提供 `MilestoneContract` 模板。动态拆解必须完整复用这些模板，只能特化 UI 标题、阶段提示和上下文键。`ContractRuntime` 决定本轮动作，`StepBuilder` 只渲染 prompt，`ContractValidator` 负责工具调用和输出的硬边界检查。

---

## 四、应用配置系统

### 4.1 App 定义

```toml
# apps/文档生成/app.toml
[app]
name = "文档生成"
description = "生成周报、报告、总结等各类文档"
icon = "[..]"
model_preference = "medium"       # small/medium/large
granularity = "fine"
prompt_file = "prompt.md"
tools = ["file_read", "file_write", "web_search", "request_user_input"]

[app.planning]
mode = "dynamic_with_static_fallback"
confirm_dynamic_plan = false
max_plan_retries = 2

[[milestones]]
id = "requirement"
label = "明确需求"
prompt_hint = "确认文档类型、受众、长度、风格等要求"
contract_mode = "collect"
question_budget = 1
allowed_tools = ["request_user_input"]
advance_policy = "on_choice"
```

### 4.2 推进策略

当前推进由 `advance_policy`、`question_budget`、`output_requirements` 和 app 级 `granularity` 共同决定。`granularity=fine` 的 Web 主路径会在 choice_result 后停住，等待用户输入“继续”进入下一阶段。

| advance_policy | 适用场景 | 行为 |
|----------------|----------|------|
| `on_choice` | 信息收集、方案选择 | 用户选择后完成当前阶段 |
| `on_valid_output` | 细化、分析、最终输出 | 输出通过 contract 校验后完成阶段 |
| `manual_continue` | 保留扩展 | 需要用户显式继续 |

---

## 五、对话状态机

### 5.1 ConversationState

```
ConversationState {
    app_id: String,
    milestones: Vec<(Milestone, MilestoneStatus)>,
    context: HashMap<k, v>,
    turn_count: u32,
    plan_initialized: bool,
    question_counts: HashMap<milestone_id, u8>,
}
```

### 5.2 里程碑状态

```
Pending -> Active -> Done
                 -> Skipped
```

Web 主路径以 `/api/chat/stream` 为唯一对话入口。旧的非流式 `/api/chat` 不注册，避免绕过 contract runtime / validator。

---

## 六、执行示例：文档生成（pinvou2 vs pinvou3）

用户：「帮我写周报」。场景：供应链迁移完成、3 家供应商审核、方案初稿。一家供应商数据对接延迟。

### pinvou2 流程

```
Plan Round 1: Claw 调 ask_user_question 收集信息
Plan Round 2: Claw 生成 markdown 方案，Pinvou 质疑，更新 plan
Bridging:     宿主 LLM 翻译成 4 张 cards，从 1078 人格池召回 executor
              Boss 点 UI 批准 << 硬性 gate >>
Execute:      Card-1 完成 -> Card-2 超时 x3 (1200s each)
              -> WorkflowPausedForRefine -> Claw 细分 cards
              -> Boss 再批 -> splice -> resume -> 完成
总计: 6 个容器，4 轮心跳，1 次细分，~40分钟
```

### pinvou3 流程

```
Round 1 计划:
  DynamicPlanner -> 复用 app.toml 静态模板，特化 5 个里程碑标题和提示
  结构校验 -> 必须完整复用模板 id 和 mode
  失败 -> 直接回退 app.toml 静态 milestones

Round 2 明确要求:
  contract_mode=collect，question_budget=1
  LLM 调用 request_user_input 给出 2-3 个选项
  用户选择后写入 context
  choice_result 完成当前里程碑，等待用户输入“继续”

Round 3 生成草稿:
  contract_mode=freeform，StepBuilder 渲染 contract prompt
  LLM 输出三段式周报草稿
  自评: [OK]
  ContractValidator 检查输出要求，不通过则不自动推进

Round 4 修改风险段:
  用户选择"供应商延迟风险写得更稳妥"
  StepBuilder 构造 review/freeform contract prompt
  LLM 输出修改后段落
  自评: [OK]

Round 5 定稿:
  用户确认
  final_output 阶段只允许契约声明的导出/保存工具
  自评: [OK]
  全部完成

总计: 5 轮轻交互，0 容器，~3分钟
```

### 对比

| | pinvou2 | pinvou3 |
|---|---|---|
| 拆解 | Claw LLM + Bridging LLM | 动态模板特化 + 静态 fallback |
| 执行 | Docker 容器 xN | PlatformEngine 进程内 |
| 完成判断 | Pinvou 心跳 + review | LLM 自评 + ContractValidator 硬边界 |
| 粒度修正 | 超时 1200s x3 -> Refine | 阶段 contract 控制问题预算和输出形态 |
| 用户介入 | Plan 期审方案 | choice_result 后显式“继续”进入下一阶段 |
| 完成时间 | ~40分钟 | ~3分钟 |

---

## 七、模块清单

### 7.1 已实现

| 文件 | 职责 |
|------|------|
| `platform/harness.rs` | `AgentHarness` trait -- 底层可替换边界 |
| `platform/app.rs` | `AppConfig` + `AppRegistry` -- 应用配置系统 |
| `platform/workflow.rs` | `ConversationState` -- 对话状态机 + 里程碑 |
| `platform/contract.rs` | `MilestoneContract` -- 静态契约模型 + `app.toml` 解析 |
| `platform/contract_runtime.rs` | `ContractRuntime` -- 按阶段契约决定本轮动作 |
| `platform/contract_validator.rs` | `ContractValidator` -- 工具调用与阶段输出硬边界检查 |
| `platform/dynamic_planner.rs` | `DynamicPlanner` -- 首轮动态拆解 + 静态契约回退 |
| `platform/step_builder.rs` | `StepBuilder` -- 旧版 step prompt + contract prompt 渲染 |
| `platform/reviewer.rs` | `LLMReviewer` -- LLM 审阅拆解结果（语义检查） |
| `platform/response_checker.rs` | `ResponseChecker` -- `[OK]`/`[MORE]`/`[BLOCKED]` 信号解析 + NextAction 路由 |
| `platform/deepseek_harness.rs` | `DeepSeekHarness` -- 实现 `AgentHarness`，对接本地和远程 LLM |
| `platform/router.rs` | `ModelRouter` -- 模型路由 + GB10 预置 |
| `platform/engine.rs` | `PlatformEngine` -- 编排主入口 (+ Mock) |
| `platform/tui/app.rs` | `PlatformApp` -- TUI 核心状态 |
| `platform/tui/launcher.rs` | 应用启动器 |
| `platform/tui/chat.rs` | 对话视图 |
| `platform/tui/sidebar.rs` | 里程碑侧边栏 |
| `platform/tui/input.rs` | 输入框 + 状态栏 |
| `platform/tui/ui.rs` | 主事件循环 |
| `pinvou-platform/src/main.rs` | 二进制入口 (`pinvou-platform`) |
| `pinvou-platform/src/web/mod.rs` | Web SSE 主路径 + dynamic planner + contract runtime/validator 接入 |
| DeepSeek-TUI `tools/user_input.rs` | `request_user_input` tool spec（复用，无需新写） |
| DeepSeek-TUI `tui/user_input.rs` | 选择器模态框（复用，无需新写） |

### 7.2 后续增强

| 模块 | 职责 | 优先级 |
|------|------|--------|
| `CheckpointStore` | 对话断点持久化 | P1 |
| `SubAgentRouter` | 并行子任务分发 | P2 |

---

## 八、AgentHarness 接口（底层替换边界）

```rust
#[async_trait]
pub trait AgentHarness: Send + Sync {
    async fn chat_stream(&self, req: ChatRequest)
        -> Result<Box<dyn Stream<Item = Result<StreamEvent>> + Send + Unpin>>;
    async fn chat(&self, req: ChatRequest) -> Result<String>;
    fn tools(&self) -> Vec<ToolDef>;
    fn models(&self) -> Vec<ModelInfo>;
    fn save_checkpoint(&self, state: &Checkpoint) -> Result<()>;
    fn load_checkpoint(&self, id: &str) -> Result<Option<Checkpoint>>;
    fn list_sessions(&self) -> Result<Vec<String>>;
    fn workspace_dir(&self) -> PathBuf;
}
```

---

## 九、关键决策记录

| # | 决策 | 原因 |
|---|------|------|
| 1 | **代码做路由器，LLM 做执行者，用户做决策者** | 代码不判断语义，LLM 不做流程控制 |
| 2 | **砍掉 StepChecker 格式校验** | 步骤数不限死；代码无法判断语义；改为可解析性检查 |
| 3 | **LLM Reviewer 语义审阅保留** | 审阅比拆解简单，LLM 能做 |
| 4 | **LLM 自评只是信号，推进必须通过 contract 验收** | 本地 LLM 不能自主管理流程，代码要校验产出、工具边界和预算 |
| 5 | **按 contract 决定确认边界** | LLM 可能不准，但确认边界由运行时决策；需要用户决策时停住确认 |
| 6 | **扩展而非替换** | 不修改原有代码逻辑 |
| 7 | **动态 contract 优先，静态 contract fallback** | 保留应用可配置性，同时避免动态拆解失败后无路可走 |
| 8 | **无 Bridging 翻译步骤** | pinvou2 已验证多余 |
| 9 | **越界和阻塞按状态机即时修正，不等超时** | 比 pinvou2 的 1200s 超时更快，也更可恢复 |
| 10 | **选择题优于问答题** | 用户做决策而非做作文；LLM 调用 `request_user_input` 出题而非开放式提问 |
| 11 | **优先复用 DeepSeek-TUI** | `request_user_input` 等工具/TUI 组件 DeepSeek-TUI 已有，Platform 不重复造轮子 |

---

> 本文档是 pinvou3 的总纲领。所有实现决策、模块边界、接口定义以此为准。
>
> 最后更新: 2026-05-11
