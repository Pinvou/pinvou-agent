# pinvou3 设计架构文档

> 基于 DeepSeek-TUI 二次开发的本地 AI 平台 TUI。
> 运行于 NVIDIA GB10，面向普通用户，覆盖日常文档、数据、计划、问答等场景。
>
> 本文档是 pinvou3 的总纲领，所有实现决策以此为准。

---

## 一、背景与定位

### 1.1 从 pinvou2 到 pinvou3

| | pinvou2 | pinvou3 |
|---|---|---|
| **架构** | Python + Electron + Docker + vLLM + Claw-Code | Rust 单体 + ratatui TUI + Web |
| **运行时组件** | 6 层 | 1 层（单二进制） |
| **分发** | .run + pip + npm + docker build | 单二进制，cargo build 即得 |
| **隔离方式** | Docker 容器 | Skills / Sub-agent / MCP |
| **任务入口** | 必经 Plan → Bridging → Execute 三段式 gate | **LLM 单次调用同时完成分类 + 拆解 + 工具选择**，简单问答不进框架 |
| **任务拆解** | Claw LLM (Plan) + Bridging LLM (翻译 → cards) | LLM 自由产出 milestone 列表，mode 受枚举约束 |
| **领域配置** | 固定 App 列表（写死） | `prompts/*.md` agent 注册表，加 agent = 加一个 markdown 文件 |
| **颗粒度控制** | 超时 1200s × 3 → Refine → Claw 细分 | 阶段 Contract 控制工具 / 预算 / 输出形态 |
| **中途纠错** | 仅 Plan 期审方案 | 显式命令（`/back`、`/redo`、`/replan`）+ 隐式信号检测 |
| **目标用户** | 技术用户（编码代理） | 普通用户（通用 AI 平台） |

### 1.2 核心定位

- **通用 AI 平台**（非纯编码工具），coding 占比小
- 面向**非专业用户**：文档生成、数据分析、计划制定、知识问答
- **对话为主**，侧边栏为可选步骤导航
- **简单问答不走框架**，避免给"问个问题"加无谓仪式
- **领域即 prompt**：新场景 = 在 `prompts/` 加一个 markdown 文件，零代码
- **单二进制分发**，开箱即用

### 1.3 关键约束

- **本地 LLM 质量有限**（Qwen 7B-35B on GB10）
- 不能依赖 LLM 自主管理复杂流程
- 不能依赖 LLM 在该停的时候自己停
- **可校验的事（工具 / 预算 / 输出结构）由代码硬卡**
- **需要语义判断的事（拆几步、每步叫什么、选什么 agent）由 LLM 决定，用户可纠错**

---

## 二、总体架构

### 2.1 分层架构

```
+----------------------------------------------------+
|                  用户输入                            |
+-------------------------+--------------------------+
                          v
              startswith("/")? ─yes→ RollbackManager
                          │ no
                          v
+----------------------------------------------------+
|   LLM 单次调用（分类 + 拆解 + 工具选择）              |
|   输入: 用户消息 + AgentRegistry + mode 字典         |
|   输出: { agent: "...", milestones: [...] }         |
+--------+----------------+---------------+---------+
         │                │                │
   agent=qa         agent=其他       校验失败
         v                v                v
   流式回答          注入 prompt        回退默认
   (无 milestone)    + Contract 编排    fallback
         |                |
         v                v
      结束           +------------------+
                     | ContractRuntime  |
                     | + Validator      |
                     | + StepBuilder    |
                     | + ConversationSt |
                     | + RollbackMgr    |
                     +------------------+
                              |
                              | AgentHarness trait
                              v
                     +-------------------+
                     | DeepSeek-TUI      |
                     | (LLM 调用 / 工具 / |
                     |  TUI 渲染)         |
                     +-------------------+
```

### 2.2 核心原则

1. **扩展而非替换**：不修改原有 crate 逻辑。仅加 `mod platform;` 和 `[[bin]]`。
2. **底层可替换**：`AgentHarness` trait 是边界。换 OpenCode 只需重新实现此 trait。
3. **分类与拆解合并**：单次 LLM 调用同时输出 agent + milestones，避免多次往返。命令检测靠 `startswith("/")` 一行代码。
4. **对话永远是主角**：工作流是侧边栏的可选建议，不是必经之路。
5. **领域即 prompt**：新 agent = 一个 `prompts/<id>.md`。零代码。
6. **可校验交给代码，语义交给 LLM，方向交给用户**：硬边界（工具 / 预算 / 结构）代码守，拆解 / 产出 LLM 做，纠错 / 回退用户控。

---

## 三、任务拆解与执行流程（核心编排）

### 3.1 职责划分

| | 代码 | LLM | 用户 |
|---|---|---|---|
| 命令检测 | `startswith("/")` 路由到 RollbackManager | - | 输入 `/back`、`/skip` 等 |
| 任务分类 + 拆解 | 校验 agent ∈ 已知列表；milestone 数量 / mode / 工具合法 | 单次调用输出 `{agent, milestones[]}` | 不满意可 `/replan` |
| Agent prompt 注入 | 读 `prompts/<agent>.md` 注入 system prompt | - | - |
| 每步 Prompt 构造 | `StepBuilder` 按 contract 限定范围 | - | - |
| 每步实际执行 | - | 拿到限定范围 Prompt 完成任务 | - |
| 工具调用 | `ContractValidator` 按 allowed_tools 硬拦截 | 选择调用与否 | - |
| 输出校验 | 结构性校验（选项数、工具调用、mode 形态） | - | - |
| 推进 / 等待 | `ContractRuntime` 按 advance_policy 路由 | - | choice_request 时回应 |
| 中途回退 | 命令 / 信号 → 状态机回退 | 可建议「是否回退」 | `/back`、`/redo`、`/replan` |

### 3.2 整体流程

```
用户输入
    |
    v
startswith("/") ? ──yes──> RollbackManager → 状态机变化 → 结束
    | no
    v
+-- LLM 单次调用：分类 + 拆解 ---------------------+
|   输入: 用户消息 + AgentRegistry + mode 字典      |
|   输出 JSON:                                     |
|   {                                              |
|     "agent": "<id>",                              |
|     "milestones": [                               |
|       { "label": "...", "mode": "...",            |
|         "tools": [...], "prompt_hint": "..." }    |
|     ]                                             |
|   }                                              |
+--------+----------------+------------------------+
         |                |
   agent=qa         agent=非 qa
   milestones=[]    milestones=[2-8]
         v                v
+-- Q&A 路径 ------+ +-- 场景路径 -----------------+
| 流式输出回答      | | 注入 prompts/<agent>.md      |
| 不挂 milestone   | | 进入 ContractRuntime         |
| 不挂 Contract    | | 按 milestone 逐步执行         |
| 历史保留          | | (见 §3.5 - §3.8)            |
+----+------------+ +-------+---------------------+
     |                      |
     v                      v
   结束               最终 final_output → 结束
```

### 3.3 LLM 单次调用：分类 + 拆解

**核心原则：用一次 LLM 调用同时完成「选 agent」和「拆 milestone」。**

不做单独的 Router 分类器，不维护关键词列表，不养第二个小模型。**唯一的代码规则是 `startswith("/")` 判断 slash 命令。**

#### 拆解 Prompt（示意）

```
用户输入: "{user_message}"

可用 agents:
- qa: 简单问答、概念解释、翻译、闲聊。无需多步拆解
- doc_generation: 根据用户素材生成结构化文档（周报、报告、邮件等）
- data_analysis: 用 Python 处理表格数据，输出分析和图表
- planning: 制定计划、时间表、行程
- generic: 不属于上述但需要多步处理的任务

可用 mode（每个 milestone 必须选一个）:
- collect: 收集用户决策性信息
- produce_options: 给 2-3 个方案让用户选
- refine_selected_option: 细化已选方案
- freeform: 自由产出（写作 / 分析 / 计算）
- final_output: 最终交付物（必须是最后一个 milestone）

可用工具池（每个 milestone 从中选 0-N 个）:
- request_user_input: 让用户做选择题
- file_read, file_write: 文件读写
- python_exec: 执行 Python（沙箱内）
- web_search: 联网搜索

请输出 JSON:
{
  "agent": "<agent_id>",
  "milestones": [
    {
      "label": "...",
      "mode": "...",
      "tools": ["..."],
      "prompt_hint": "..."
    }
  ]
}

约束:
- 如果 agent=qa，milestones 输出空数组
- 否则 milestones 数量 2-8 个
- 最后一个 milestone 必须 mode=final_output
- tools 必须从工具池中选
- mode 必须从枚举中选
```

#### 三种典型输出

**输出 1：简单问答**
```json
{
  "agent": "qa",
  "milestones": []
}
```
→ 流式回答用户问题，不挂 milestone，不挂 Contract。后续追问保留对话历史。

**输出 2：场景任务**
```json
{
  "agent": "doc_generation",
  "milestones": [
    {"label": "确认结构", "mode": "produce_options", "tools": ["request_user_input"], "prompt_hint": "..."},
    {"label": "生成草稿", "mode": "freeform", "tools": [], "prompt_hint": "..."},
    {"label": "定稿", "mode": "final_output", "tools": ["file_write"], "prompt_hint": "..."}
  ]
}
```
→ 注入 `prompts/doc_generation.md`，按 milestones 走 Contract 编排。

**输出 3：JSON 解析失败 / 校验失败**
→ 回退到 `agent=generic` + 默认 fallback milestones（`collect → freeform → final_output` 三步）。

#### 结构性校验

代码不校验「内容是否合理」，只校验「结构是否合法」：

- `agent` ∈ AgentRegistry 已注册的 id
- 如果 `agent != "qa"`：
  - `milestones.length` ∈ [2, 8]
  - 每个 `mode` ∈ {collect, produce_options, refine_selected_option, freeform, final_output}
  - 最后一个 `mode == final_output`
  - 每个 `tools[i]` ∈ 全局工具池
  - `tools` 满足 mode 限制（如 `final_output` 不能含 `request_user_input`）
  - `label` 非空且不重复

校验失败 → 回退到 `agent=generic` + fallback milestones。

#### 流式分类优化（可选，P1）

LLM 输出 JSON 时按字段顺序流式出。`agent` 字段一旦解析出，即可决定路径：

- `agent == "qa"` → 立刻中止后续 JSON 输出，发起 Q&A 流式回答
- `agent != "qa"` → 继续等 milestones 完整后进入编排

P0 阶段不做此优化，接受 ~500ms 的"分类延迟"。

### 3.4 Agent 注册表

#### 文件结构

每个 agent 是 `prompts/<id>.md`，含 YAML frontmatter + markdown 正文：

```markdown
---
id: doc_generation
name: 文档生成
description: 根据用户提供的素材生成结构化文档：周报、报告、邮件、纪要等。
emoji: 📝
---

# 角色

你是文档生成助手，擅长根据用户素材生成结构化、可读性高的中文文档。

# 适用场景
...

# 风格
...

# 注意事项
...
```

#### 加载时机

启动时扫描 `prompts/*.md`：

- 解析 frontmatter → 注册到 `AgentRegistry`
- 正文内容缓存到内存，执行阶段注入 system prompt
- frontmatter 的 `description` 字段拼接到 §3.3 的拆解 prompt 中给 LLM 看

#### 初始 agent 集合（P0）

| id | 一句话定位 |
|---|---|
| `qa` | 简单问答、翻译、概念解释、闲聊 |
| `doc_generation` | 周报、报告、邮件、纪要 |
| `data_analysis` | CSV / Excel 数据探索 + Python + 图表 |
| `planning` | 计划、时间表、行程 |
| `generic` | 兜底，多步但无明显领域归属 |

#### 添加新 agent 的流程

1. 在 `prompts/` 加 `<new_id>.md`
2. frontmatter 写 `id` / `name` / `description` / `emoji`
3. 正文 200-400 字（角色 / 场景 / 风格 / 注意事项）
4. 重启服务（启动时重扫）

不改代码，不改全局配置。

#### 致谢

agent 概念格式（frontmatter + markdown body）参考 [agency-agents-zh](https://github.com/jnMetaCode/agency-agents-zh)。pinvou3 重新撰写了适配本地小模型的精简版（200-400 字 / agent），未直接依赖该仓库。

### 3.5 MilestoneContract

LLM 拆解决定 milestone 的 `label`、`mode`、`tools`、`prompt_hint`、`required_context`、`produced_context`。**Mode 对应的结构性约束（`question_budget`、`advance_policy`、`output_requirements`）由代码内置，不可配置。**

```rust
struct MilestoneContract {
    label: String,
    mode: MilestoneMode,                            // LLM 选
    tools: Vec<String>,                             // LLM 选，受全局池约束
    prompt_hint: Option<String>,                    // LLM 填
    required_context: Vec<String>,                  // LLM 填
    produced_context: Vec<String>,                  // LLM 填
    // 以下由 mode 决定，代码内置:
    question_budget: u8,
    advance_policy: AdvancePolicy,
    output_requirements: Vec<OutputRequirement>,
}

enum MilestoneMode {
    Collect,                  // 收集信息
    ProduceOptions,           // 产出 2-3 个方案让用户选
    RefineSelectedOption,     // 细化已选方案
    Freeform,                 // 自由产出
    FinalOutput,              // 最终输出（必须是最后一个）
}

enum AdvancePolicy {
    OnChoice,        // 用户选择后完成
    OnValidOutput,   // 输出通过校验后完成
    ManualContinue,  // 显式继续
}
```

#### mode → 内置规则（代码硬编码）

| mode | question_budget | advance_policy | output_requirements |
|---|---|---|---|
| `collect` | 1 | `on_choice` | 无 |
| `produce_options` | 1 | `on_choice` | `min_options=2`, `max_options=3` |
| `refine_selected_option` | 0 | `on_valid_output` | `no_open_question` |
| `freeform` | 1 | `on_valid_output` | `no_open_question` |
| `final_output` | 0 | `on_valid_output` | `no_tool_call_except_export` |

### 3.6 执行硬边界

**「调什么工具 / 问几次 / 输出什么形状」由代码硬卡，不靠 LLM 自觉。**

| 边界 | 实现 | 违反时 |
|------|------|--------|
| Agent 合法性 | `agent` ∈ 已注册 AgentRegistry | 回退 generic |
| 工具池白名单 | `milestone.tools[i]` ∈ 全局工具池 | 拆解时拒绝该 milestone |
| Mode-工具规则 | `final_output` 不能含 `request_user_input` 等 | 拆解时拒绝 |
| 选项数量 | `request_user_input.questions[].options` 长度 | 不发 choice card |
| 提问预算 | `state.question_count(ms_id) < budget` | 返回 blocked 文案 |
| 输出结构 | mode 对应的结构校验（如 markdown table 必须有 `|---|`） | 不自动推进 |
| Q&A 模式 | 不挂 Contract，但工具调用一律拒绝 | 直接拒绝 |

#### 全局工具池（P0）

| 工具 | 危险等级 | 说明 |
|---|---|---|
| `request_user_input` | 安全 | 让用户做选择题 |
| `file_read` | 安全 | 读 workspace 内文件 |
| `file_write` | 中 | 写 workspace 内文件（沙箱） |
| `web_search` | 安全 | 联网搜索 |
| `python_exec` | 中 | 沙箱内 Python 执行 |

#### 结构校验 vs 语义校验

- **结构性校验**（代码做，P0）：调了不该调的工具、选项数量不对、markdown 表格缺 `|---|` 分隔
- **语义性校验**（LLM judge 做，P1）：内容是否合理、文档是否完整、答案是否准确

### 3.7 中途纠错与回退

**单向流水线不符合真实对话。** 真实场景中用户可能：

- 走到 Step 3 时说"等下，第 1 步的需求我说错了"
- 拿到草稿后说"重写"
- 看到拆解后说"第 2 步跳过"
- 写到一半说"这个方向不对"

#### 显式回退命令

| 命令 | 行为 |
|------|------|
| `/back` | 当前 milestone 标回 Active，清自身及之后的 context；前一个 Done 标回 Active |
| `/skip` | 当前 milestone 标 Skipped，推进到下一个 |
| `/redo` | 当前 milestone 重做（仅清自身 context，前面保留） |
| `/replan` | 整个对话重新拆解，已收集 context 注入新计划 |
| `/use <agent_id>` | 显式切换 agent（如 `/use planning`） |

#### 隐式回退检测

LLM 检测到「重做 / 不对 / 换一种」类信号时，**不直接执行**，向用户显示选项卡片：

```
A) 重写当前步骤（保留前几步）
B) 回到上一步，从那里改起
C) 改整体计划，重新拆解
D) 算了，继续当前
```

#### 状态机扩展

```
Pending --start--> Active --done--> Done
                     |                |
                     |                +--back--> Active
                     |
                     +--skip--> Skipped
                     |
                     +--redo--> Active  (清自身 context)
```

#### Context 归属

每条 context 必须挂在产生它的 milestone 上：

```rust
struct ContextEntry {
    key: String,
    value: serde_json::Value,
    produced_by: MilestoneId,
}
```

回退时按 `produced_by` 清理：

- `/back`：清退回点之后（含当前）所有 milestone 产生的 context
- `/redo`：仅清当前 milestone 的 context
- `/replan`：保留全部 context，让新计划的 `required_context` 自行决定

### 3.8 交互模式：选择题优于问答题

**核心原则：用户做决策，不做作文。**

```
反模式:
  LLM: "请描述你的数据文件结构、想分析什么维度、关心什么指标？"
  用户: 需要组织语言写一段话回答

正确模式:
  LLM 调用 request_user_input({
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

DeepSeek-TUI 已有完整实现：

- Tool spec：1-3 题，每题 2-3 选项 + "Other" 自定义输入
- TUI 模态框：数字键快速选择
- Engine 集成：`await_user_input()` 挂起 agent loop

Platform 层职责：

1. LLM 拆解时把 `request_user_input` 选入 collect / produce_options 的 `tools`
2. 在 contract prompt 中引导 LLM 用此工具而非开放题
3. `ContractValidator` 校验选择题形状（2-3 选项）
4. Web 在发送 choice card 前预留问题预算

---

## 四、Agent 系统（替代旧 App 系统）

### 4.1 文件布局

```
pinvou3/
├── prompts/                    ← Agent 注册表（核心）
│   ├── qa.md
│   ├── doc_generation.md
│   ├── data_analysis.md
│   ├── planning.md
│   └── generic.md
└── ...
```

旧的 `apps/<App名>/app.toml` 目录结构在 P0 退役阶段移除。

### 4.2 Agent 文件格式

```markdown
---
id: doc_generation
name: 文档生成
description: 根据用户提供的素材生成结构化文档：周报、报告、邮件、纪要等。
emoji: 📝
---

# 角色

你是文档生成助手...（200-400 字）

# 适用场景
- ...

# 风格
- ...

# 注意事项
- ...
```

**Frontmatter 字段：**

| 字段 | 必填 | 用途 |
|---|---|---|
| `id` | ✓ | 程序内唯一标识，必须匹配文件名（`<id>.md`） |
| `name` | ✓ | 用户可见的显示名 |
| `description` | ✓ | 一句话场景描述，给 LLM 看用于选 agent |
| `emoji` | 可选 | UI 上的图标 |

**正文：** 200-400 字，分 4 块：角色 / 适用场景 / 风格 / 注意事项。
正文在执行阶段作为 system prompt 中段注入。

### 4.3 全局配置（代码硬编码）

以下由代码内置，不开放配置：

- **Mode → 规则映射**（见 §3.5）：question_budget、advance_policy、output_requirements
- **全局工具池**（见 §3.6）：5 个内置工具及危险等级
- **Mode-工具兼容规则**：如 `final_output` 禁用 `request_user_input`

理由：这些是契约的安全底线，开放配置会让 LLM 有机会给自己放权。

### 4.4 System Prompt 组装

执行某个 milestone 时，StepBuilder 组装的完整 system prompt：

```
┌─────────────────────────────────────────┐
│ [全局基础 prompt]                         │
│   "你是 pinvou 助手，本地运行..."           │
├─────────────────────────────────────────┤
│ [agent prompt]    ← prompts/<id>.md     │
│   "你是文档生成助手..."                     │
├─────────────────────────────────────────┤
│ [mode 阶段指令]    ← ContractRuntime 生成 │
│   "当前阶段：生成草稿 (freeform)            │
│    允许工具: [file_read]                   │
│    输出要求: markdown                      │
│    问题预算: 1"                            │
├─────────────────────────────────────────┤
│ [已知 context]    ← ConversationState    │
│   "用户已选结构：三段式..."                  │
├─────────────────────────────────────────┤
│ [用户当轮消息]                              │
└─────────────────────────────────────────┘
```

---

## 五、对话状态机

### 5.1 ConversationState

```rust
struct ConversationState {
    agent_id: Option<String>,                // None = Q&A 模式或未初始化
    global_mode: GlobalMode,
    milestones: Vec<(Milestone, MilestoneStatus)>,
    context: HashMap<String, ContextEntry>,  // 含 produced_by
    turn_count: u32,
    plan_initialized: bool,
    question_counts: HashMap<MilestoneId, u8>,
    history: Vec<StateTransition>,           // 用于回退追溯
}
```

### 5.2 里程碑状态

```
Pending --start--> Active --done--> Done --back--> Active
                     |
                     +--skip--> Skipped
                     |
                     +--redo--> Active
```

### 5.3 全局会话状态（GlobalMode）

```
QnAMode       Router 判定为 Q&A，没有 milestones
PlanningMode  动态拆解中（极短，通常 <1s）
ExecutingMode 按 milestone 推进
ReplanMode    用户触发 /replan
DoneMode      final_output 完成
```

### 5.4 Web 主路径

`/api/chat/stream` 是唯一对话入口。前置 `startswith("/")` 决定后续路径：

- **命令模式**：调用 RollbackManager 触发状态机变化
- **首轮（非命令）**：发起 LLM 单次调用 → 分类 + 拆解 → 根据 agent 路由
- **后续轮（QnA Mode）**：直接转发到 LLM 流，保留对话历史
- **后续轮（Executing Mode）**：经过 ContractRuntime + ContractValidator

旧的非流式 `/api/chat` 不注册，避免绕过 Router 和 Contract 系统。

---

## 六、执行示例

### 6.1 简单问答

```
用户: "K-means 是什么？"

[startswith("/")? no]
   ↓
LLM 单次调用 →
{"agent": "qa", "milestones": []}
   ↓
[agent=qa] 进入 Q&A 路径
注入 prompts/qa.md 到 system prompt
LLM 流式输出: "K-means 是一种无监督聚类算法..."
   ↓
完成（对话历史保留）

后续:
用户: "那 K-medoids 呢？"
[已在 QnAMode] 跳过分类，直接流式回答
注入 prompts/qa.md + 历史
LLM: "K-medoids 跟 K-means 类似但..."
```

### 6.2 场景任务：写周报

```
用户: "帮我写本周周报，团队 3 人，做了 A/B feature 和事故复盘"

[startswith("/")? no]
   ↓
LLM 单次调用 →
{
  "agent": "doc_generation",
  "milestones": [
    {"label": "确认结构偏好", "mode": "produce_options", "tools": ["request_user_input"]},
    {"label": "生成草稿", "mode": "freeform", "tools": []},
    {"label": "调整事故段落", "mode": "refine_selected_option", "tools": []},
    {"label": "定稿", "mode": "final_output", "tools": ["file_write"]}
  ]
}
   ↓
[结构校验通过] 注入 prompts/doc_generation.md
进入 Contract 编排，侧边栏渲染 4 个 milestone

Round 2: "确认结构偏好" (produce_options)
  ContractRuntime: 允许 request_user_input
  LLM 调用 request_user_input 给 3 个结构选项
  Validator 检查选项数: 通过
  用户选 → context.structure = "三段式" (produced_by="确认结构偏好")
  milestone 标 Done

Round 3: "生成草稿" (freeform)
  注入已选结构到 context
  LLM 输出三段式草稿
  Validator 检查 markdown 结构: 通过

Round 4: "调整事故段落" (refine_selected_option)
  用户: "事故复盘那段太轻描淡写"
  LLM 重写该段

Round 5: "定稿" (final_output)
  Validator: allowed_tools 仅含 file_write
  LLM 调用 file_write 保存最终 markdown
  完成

总计: 5 轮，~3 分钟
```

### 6.3 中途纠错

```
用户在 Round 4 拿到草稿后:
"不行，整体方向不对，重新写"

[LLM 检测到回退信号]
LLM 显示选项卡片:
  A) 重写当前步骤
  B) 回到上一步改起
  C) 改整体计划，重新拆解
  D) 继续当前

用户选 C → 触发 /replan
  保留 context (素材已收集)
  重新调用 LLM 单次调用 → 新 agent + milestones
  生成新计划

----

用户在 Round 5 (定稿后):
"/back"
  最后一个 milestone Done → Active
  produced_by="定稿" 的 context 清除
  LLM 基于现状重新生成
```

---

## 七、模块清单

### 7.1 当前已实现

| 文件 | 职责 |
|------|------|
| `platform/harness.rs` | `AgentHarness` trait —— 底层可替换边界 |
| `platform/app.rs` | `AppConfig` + `AppRegistry`（待退役，由 AgentRegistry 替代） |
| `platform/workflow.rs` | `ConversationState` —— 对话状态机（待扩展回退） |
| `platform/contract.rs` | `MilestoneContract` + mode 枚举 |
| `platform/contract_runtime.rs` | `ContractRuntime` —— 按阶段契约决定本轮动作 |
| `platform/contract_validator.rs` | `ContractValidator` —— 工具 / 输出硬边界检查 |
| `platform/dynamic_planner.rs` | `DynamicPlanner` —— 首轮动态拆解（待重写为合并分类+拆解） |
| `platform/step_builder.rs` | `StepBuilder` —— contract prompt 渲染 |
| `platform/deepseek_harness.rs` | `DeepSeekHarness` —— 实现 `AgentHarness` |
| `platform/router.rs` | `ModelRouter` —— 模型路由 + GB10 预置 |
| `platform/engine.rs` | `PlatformEngine` —— 编排主入口 |
| `pinvou-platform/src/web/mod.rs` | Web SSE 主路径 |
| DeepSeek-TUI `tools/user_input.rs` | `request_user_input` tool spec（复用） |
| DeepSeek-TUI `tui/user_input.rs` | 选择器模态框（复用） |

### 7.2 待新增（P0）

| 模块 | 职责 |
|------|------|
| `AgentRegistry` | 扫描 `prompts/*.md`，解析 frontmatter，注册 agent |
| `CombinedPlanner` | 替代 `DynamicPlanner`，一次 LLM 调用同时输出 agent + milestones |
| `CommandRouter` | `startswith("/")` 路由到 RollbackManager（极简，~20 行） |
| `RollbackManager` | 显式命令处理 + 状态机回退（`/back`、`/skip`、`/redo`、`/replan`、`/use`） |
| `ContextEntry.produced_by` | context 归属追踪，支持精准清理 |
| `GlobalMode` | 全局会话状态枚举（QnA / Planning / Executing / Replan / Done） |
| `ContractValidator v2` | 结构性正则替换关键词校验；mode-工具兼容规则 |
| Mode → 规则映射 | 代码内置（替代 app.toml 配置） |

### 7.3 待新增（P1）

| 模块 | 职责 |
|------|------|
| `LLM-as-judge` | 语义性校验替代关键词启发式 |
| `CheckpointStore` | 对话断点持久化 |
| 隐式回退信号检测 | "重做 / 不对 / 换一种" 等语义识别 |
| 流式分类优化 | LLM 输出 JSON 时按 `agent` 字段流式路由 |

### 7.4 待退役（legacy）

| 模块 | 状态 |
|------|------|
| `platform/app.rs` (AppConfig / AppRegistry) | 由 AgentRegistry 替代，P0 退役 |
| `apps/<App名>/app.toml` 目录 | 由 `prompts/<id>.md` 替代，P0 删除 |
| `platform/response_checker.rs` | LLM 自评信号解析，Contract 系统已替代，P1 退役 |
| `platform/reviewer.rs` | LLM 审阅拆解，新版动态拆解不需要语义审阅，P1 退役 |
| `engine.decompose_and_execute` | 旧版拆解执行流程，Web 主路径不再调用，P1 删除 |
| `WebTurnAction::LegacyFallback` | 动态计划失败已有 fallback 兜底，P1 删除 |
| `DynamicPlanner` | 重写为 `CombinedPlanner`（合并分类+拆解） |

### 7.5 待新增（P2）

| 模块 | 职责 |
|------|------|
| `SubAgentRouter` | 并行子任务分发 |
| 多层 agent 选择 | 当 agent 数量 >20 时，先选部门再选 agent |

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
| 2 | **砍掉 StepChecker 格式校验** | 步骤数不限死；改为结构性校验 |
| 3 | **LLM Reviewer 语义审阅 → P1 退役** | 新版动态拆解结构性校验足够 |
| 4 | **LLM 自评只是信号，推进必须通过 contract 验收** | 本地 LLM 不能自主管理流程 |
| 5 | **按 contract 决定确认边界** | LLM 可能不准，需要决策时由 runtime 停住 |
| 6 | **扩展而非替换** | 不修改原有代码逻辑 |
| 7 | **动态 milestone 列表自由，mode 仍是枚举** | 旧版「严格复用模板」让动态拆解变成翻译标题；mode 枚举保留契约挂载点 |
| 8 | **无 Bridging 翻译步骤** | pinvou2 已验证多余 |
| 9 | **越界和阻塞按状态机即时修正，不等超时** | 比 pinvou2 的 1200s 超时更快 |
| 10 | **选择题优于问答题** | 用户做决策而非做作文 |
| 11 | **优先复用 DeepSeek-TUI** | 工具 / TUI 组件 DeepSeek-TUI 已有 |
| 12 | **分类与拆解合并为单次 LLM 调用** | 避免多次 LLM 往返；agent 字段一出即可路由 |
| 13 | **命令路由只用 `startswith("/")` 一行代码** | 不养第二个小模型、不维护关键词列表 |
| 14 | **状态机支持显式回退（/back, /redo, /replan, /use）** | 真实对话不是单向流水线 |
| 15 | **Context 按 produced_by 归属** | 回退时精准清理，避免污染前序步骤 |
| 16 | **OutputRequirement 改为结构性 + LLM judge** | 关键词校验脆弱；结构性正则可靠，语义判断交给 LLM judge |
| 17 | **ResponseChecker / LLMReviewer / AppRegistry 标 legacy** | Contract 系统 + AgentRegistry 已替代其职责 |
| 18 | **Mode → 规则映射代码硬编码** | 这些是契约安全底线，开放配置等于让 LLM 给自己放权 |
| 19 | **Agent = 一个 markdown 文件** | 新场景零代码；frontmatter 参考 agency-agents-zh，正文为 pinvou3 适配本地小模型重写 |
| 20 | **工具池全局共享，LLM 按 milestone 挑** | App 概念解耦；危险等级由代码控制 |

---

> 本文档是 pinvou3 的总纲领。所有实现决策、模块边界、接口定义以此为准。
>
> 最后更新: 2026-05-11
