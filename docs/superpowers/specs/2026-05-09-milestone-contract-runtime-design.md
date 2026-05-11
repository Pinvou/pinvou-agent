# pinvou3 Milestone Contract Runtime Design

> 状态：待评审  
> 日期：2026-05-09  
> 范围：静态里程碑契约运行时 + 动态拆解接入 Web 主路径

---

## 一、目标

把当前“里程碑 + prompt 软约束”的编排方式升级为“配置化契约 + 运行时决策 + 可验证输出”的编排系统。

目标不是继续补 prompt，而是让代码明确负责流程规则：

- 每个里程碑声明自己的契约：当前阶段允许问什么、必须产出什么、是否允许工具、何时推进。
- Web 主路径接入动态拆解：用户第一条需求进入后，优先生成贴合任务的动态里程碑；失败时回退到 app.toml 静态里程碑。
- StepBuilder 不再猜阶段规则，只把 ContractRuntime 的决策渲染成 prompt。
- Validator 做硬边界检查：例如“方案对比”必须给 2-3 个方案，不能只问一个偏好；“最终输出”不能继续发 choice card。
- Choice result 是状态机事件，不是普通聊天文本。

---

## 二、现状问题

当前状态机已经能做到：

```text
用户选择 -> tool_result -> 写入 ConversationState.context -> 推进当前里程碑
```

但还没有做到：

```text
当前里程碑契约 -> 决定本轮该问问题 / 产出方案 / 细化方案 / 输出最终文档
```

实际测试里暴露的问题：

- 「方案对比」阶段没有真正输出 2-3 个方案，而是继续问“自然风光类型”。
- 「细化方案」阶段先写了半成品计划，随后又补问“上午/下午”。
- `request_user_input` 被过度使用，缺少每个阶段的问题预算和工具策略。
- `decompose_and_execute()` 已有雏形，但 Web 主路径仍走静态里程碑。

---

## 三、非目标

- 不重写 DeepSeek-TUI 底层。
- 不重做 Web UI 视觉设计。
- 不引入数据库。
- 不把所有业务知识硬编码进 Rust。
- 不要求动态拆解永远成功；动态拆解失败必须稳定 fallback 到静态契约。

---

## 四、方案选择

### 方案 A：继续强化 prompt

在 `StepBuilder::ban_list()` 里继续加阶段提示。

优点：改动小。  
缺点：模型仍可违反；无法系统性测试；动态拆解接入后会继续失控。  
结论：不采用。

### 方案 B：只给静态里程碑加 validator

保留现有 app.toml，新增 ResponseChecker 规则。

优点：能修一部分当前问题。  
缺点：规则会散落在代码里，不能表达 app 差异；动态拆解仍没有契约落点。  
结论：不采用为最终方案。

### 方案 C：Milestone Contract Runtime

把里程碑升级为契约对象；静态 app.toml 和动态拆解都输出同一种 `MilestoneContract`；运行时根据契约决定下一步动作；Validator 负责硬边界。

优点：边界清楚，可测试，可扩展，能同时解决静态和动态路径。  
缺点：需要新增几组模型和运行时模块。  
结论：采用。

---

## 五、核心架构

```text
用户消息
  |
  v
Engine.ensure_plan_initialized()
  |
  +-- DynamicPlanner 生成动态 MilestoneContract 列表
  |      |
  |      +-- PlanValidator 通过 -> ConversationState 使用动态计划
  |      |
  |      +-- 失败 -> 使用 app.toml 静态 contracts
  |
  v
ContractRuntime::next_directive()
  |
  +-- AskUser        -> Web 直接渲染 choice_request
  +-- CallLlm        -> StepBuilder 渲染 contract prompt -> LLM
  +-- CompleteStep   -> mark_done / wait / advance
  +-- Blocked        -> 停住等用户
  |
  v
ContractValidator
  |
  +-- validate_tool_call()
  +-- validate_response()
  +-- validate_choice_result()
```

---

## 六、数据模型

### 6.1 AppConfig 扩展

`AppConfig` 增加 planning 配置。

```rust
pub struct PlanningConfig {
    pub mode: PlanningMode,
    pub confirm_dynamic_plan: bool,
    pub max_plan_retries: u8,
}

pub enum PlanningMode {
    StaticOnly,
    DynamicWithStaticFallback,
}
```

默认值：

- `mode = DynamicWithStaticFallback`
- `confirm_dynamic_plan = false`
- `max_plan_retries = 2`

### 6.2 Milestone 扩展

现有 `Milestone` 保留 `id / label / prompt_hint / icon`，新增契约字段。

```rust
pub struct Milestone {
    pub id: String,
    pub label: String,
    pub prompt_hint: Option<String>,
    pub icon: Option<String>,
    pub contract: MilestoneContract,
}
```

### 6.3 MilestoneContract

```rust
pub struct MilestoneContract {
    pub mode: MilestoneMode,
    pub question_budget: u8,
    pub required_context: Vec<String>,
    pub produced_context: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub forbidden_tools: Vec<String>,
    pub output_requirements: Vec<OutputRequirement>,
    pub advance_policy: AdvancePolicy,
}

pub enum MilestoneMode {
    Collect,
    ProduceOptions,
    RefineSelectedOption,
    FinalOutput,
    Freeform,
}

pub enum AdvancePolicy {
    OnChoice,
    OnValidOutput,
    ManualContinue,
}
```

### 6.4 OutputRequirement

第一版只做可机械检查的要求。

```rust
pub enum OutputRequirement {
    MinOptions(u8),
    MaxOptions(u8),
    MustContainTable,
    MustContainSchedule,
    MustContainRiskSection,
    NoOpenQuestion,
    NoToolCall,
}
```

后续可以扩展为更强的 schema validator，但本轮不引入复杂外部依赖。

---

## 七、app.toml 契约示例

### 7.1 计划敲定

```toml
[app.planning]
mode = "dynamic_with_static_fallback"
confirm_dynamic_plan = false
max_plan_retries = 2

[[milestones]]
id = "goal"
label = "明确目标"
prompt_hint = "确认目标、兴趣、时长、强度"
contract_mode = "collect"
question_budget = 1
produced_context = ["goal", "interest", "duration", "intensity"]
allowed_tools = ["request_user_input"]
advance_policy = "on_choice"

[[milestones]]
id = "constraints"
label = "梳理约束"
prompt_hint = "确认预算、交通、人数、时间段"
contract_mode = "collect"
question_budget = 1
required_context = ["goal", "duration"]
produced_context = ["budget", "transport", "party_size", "time_window"]
allowed_tools = ["request_user_input"]
advance_policy = "on_choice"

[[milestones]]
id = "options"
label = "方案对比"
prompt_hint = "给出 2-3 个可选方案，列出成本、时间、风险、收益，并让用户选择一个"
contract_mode = "produce_options"
question_budget = 1
required_context = ["goal", "duration", "budget", "transport"]
produced_context = ["selected_option"]
allowed_tools = ["request_user_input"]
output_requirements = ["min_options:2", "max_options:3", "must_contain_table", "no_open_question"]
advance_policy = "on_choice"

[[milestones]]
id = "detail"
label = "细化方案"
prompt_hint = "基于已选方案生成时间表、资源分配和风险预案"
contract_mode = "refine_selected_option"
question_budget = 0
required_context = ["selected_option"]
allowed_tools = []
output_requirements = ["must_contain_schedule", "must_contain_risk_section", "no_open_question"]
advance_policy = "on_valid_output"

[[milestones]]
id = "output"
label = "输出计划书"
prompt_hint = "输出最终 markdown 计划书"
contract_mode = "final_output"
question_budget = 0
allowed_tools = []
output_requirements = ["no_tool_call"]
advance_policy = "on_valid_output"
```

---

## 八、运行时行为

### 8.1 初始化计划

Web 第一条用户消息到达时：

1. 加载 app。
2. 如果 `planning.mode = DynamicWithStaticFallback`，调用 `DynamicPlanner`。
3. 动态 planner 输出 `Vec<Milestone>`，每个 milestone 必须包含 contract。
4. `PlanValidator` 检查：
   - id 唯一
   - mode 合法
   - 每步有具体产出
   - collect / produce_options / refine / final_output 顺序合理
   - 没有笼统步骤，例如“分析”“处理”“做计划”
5. 通过则写入 `ConversationState`。
6. 失败则使用 app.toml 静态 contracts。

### 8.2 每轮指令

`ContractRuntime::next_directive()` 输入：

- 当前 app config
- 当前 active milestone
- 当前 context
- 当前用户消息
- 当前里程碑已问问题次数
- 是否有 pending choice

输出：

```rust
pub enum TurnDirective {
    AskUser(ChoiceRequest),
    CallLlm(ContractPrompt),
    CompleteStep(MilestoneAdvanceResult),
    Blocked(String),
}
```

### 8.3 collect 阶段

`question_budget > 0` 且缺少 produced_context 时：

- 允许 `request_user_input`
- 一轮 choice 后写入 context
- `advance_policy = on_choice` 时完成当前里程碑

如果 question budget 已用完但 context 仍缺失：

- 停住，返回 blocked，让用户选择“继续使用已有信息 / 重新补充”。

### 8.4 produce_options 阶段

必须输出 2-3 个方案，再让用户选择一个。

硬规则：

- 不能只问偏好。
- choice card 至少 2 个 option。
- 每个方案必须有明确 label 和 description。
- 输出或 prompt 中必须要求成本 / 时间 / 风险 / 收益。

### 8.5 refine 阶段

必须基于 `selected_option` 细化。

硬规则：

- 不允许 `request_user_input`，除非 contract 显式允许。
- 不能再问目标类问题。
- 必须有时间表和风险预案。

### 8.6 final_output 阶段

必须输出最终交付物。

硬规则：

- 不允许工具调用。
- 不允许开放式问题。
- 不允许继续收集需求。

---

## 九、StepBuilder 职责变化

StepBuilder 当前混合了“阶段规则”和“prompt 渲染”。改造后：

- 不再根据 app id / phase label 猜规则。
- 接收 `ContractPrompt`。
- 只负责渲染：
  - 当前任务
  - 当前契约
  - 已知 context
  - 允许工具
  - 禁止行为
  - 输出要求
  - 完成信号格式

---

## 十、Validator 职责

新增 `contract_validator.rs`。

### 10.1 Tool call 验证

检查：

- 当前阶段是否允许该工具。
- `request_user_input` 是否超过 question budget。
- choice questions 是否 1-3 个。
- options 是否 2-3 个。
- `produce_options` 阶段不能只有 1 个选项。
- `final_output` 阶段不能调用工具。

### 10.2 Response 验证

检查：

- `MinOptions(2)`：文本中至少能识别 2 个方案，或 tool call 至少 2 个 options。
- `MustContainTable`：含 Markdown 表格。
- `MustContainSchedule`：含时间表关键词或时间模式。
- `MustContainRiskSection`：含风险/预案。
- `NoOpenQuestion`：不能以泛化提问结束，例如“你想怎么样”“还需要什么”。
- `NoToolCall`：没有工具调用。

Validator 不追求语义完美，但必须捕捉当前测试暴露的错误。

---

## 十一、动态拆解策略

动态拆解不是让 LLM 随便发明流程，而是让它**基于 app 的静态 contract template 做任务定制**。

输入：

- 用户原始需求
- app 描述
- 静态 milestones + contract templates
- 可用工具
- 已知 context

输出 JSON：

```json
{
  "milestones": [
    {
      "id": "goal",
      "label": "明确水生水库徒步目标",
      "prompt_hint": "确认强度、兴趣、时长",
      "contract_mode": "collect",
      "produced_context": ["interest", "duration", "intensity"]
    }
  ]
}
```

限制：

- 动态计划只能覆盖 `label / prompt_hint / required_context / produced_context / output_requirements`。
- 不能随意改变危险字段，例如 `advance_policy` 和 `allowed_tools`，除非静态模板允许。
- 如果动态计划不合法，回退静态模板。

这能保留 app 的流程安全边界，同时让任务变得具体。

---

## 十二、Web 主路径

`stream_chat()` 改为：

```text
load app
ensure_plan_initialized(user_message)
consume tool_result if any
directive = ContractRuntime::next_directive(...)

if AskUser:
  SSE choice_request

if CallLlm:
  build contract prompt
  stream LLM
  validate tool calls / response
  update state

if CompleteStep:
  SSE summary + milestone

if Blocked:
  SSE blocked message
```

---

## 十三、测试策略

必须新增测试覆盖：

- app.toml 能加载 contract 字段，旧 app.toml 有默认 contract。
- ContractRuntime 在 collect / produce_options / refine / final_output 下返回正确 directive。
- produce_options 阶段拒绝单选项 choice_request。
- refine 阶段拒绝继续问目标类问题。
- final_output 阶段拒绝工具调用。
- DynamicPlanner 能解析合法 JSON。
- DynamicPlanner 对非法/笼统/缺 contract 的计划 fallback 静态模板。
- Web/Engine 主路径第一条消息会初始化计划。
- Choice result 写 context 并按 contract 推进。

---

## 十四、验收标准

以“水生水库徒步计划”作为人工验收样例：

1. 第一阶段最多问一次目标偏好。
2. 第二阶段最多问一次预算/交通/人数/时间段。
3. 方案对比阶段必须输出 2-3 个方案，并让用户选择一个；不能只问“湖畔水库”这种单选。
4. 细化阶段必须直接细化已选方案，不能再问目标类问题。
5. 最终阶段只输出完整计划书，不再继续问问题。
6. `cargo test --manifest-path pinvou-platform/Cargo.toml` 全部通过。

