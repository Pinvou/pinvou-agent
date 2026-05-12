# pinvou3 设计架构文档

> 基于 DeepSeek-TUI 二次开发的本地 AI 平台。
> 运行于 NVIDIA GB10，面向普通用户，覆盖文档生成、数据分析、计划制定、知识问答等场景。

---

## 一、背景与定位

### 1.1 设计目标

- **通用 AI 平台**，不是编码工具。面向非专业用户。
- **本地运行**（Qwen 7B-35B on GB10），单二进制分发。
- **对话为主**，侧边栏为辅。
- **零代码扩展**：加新 agent = 加一个 `prompts/<id>.md`。

pinvou2 的教训：必经的 Plan → Bridging → Execute 三段式 gate + Docker 容器化太重，本地小模型管不住流程。pinvou3 用 **代码硬边界 + LLM 语义 + 用户纠错** 三方分工，每方只做自己擅长的。

### 1.2 关键约束

**本地 LLM 能力有限**（即便是较强的 MoE 模型，也无法完全自主管理流程）→ 设计原则：

- **不能依赖 LLM 在该停的时候自己停** → question_budget / advance_policy 硬卡
- **不能依赖 LLM 自主管理复杂流程** → ContractRuntime 决定本轮动作
- **能用代码校验的事不交给 LLM** → ContractValidator 拦截越界
- **需要语义判断的事必须交给 LLM** → 拆解 / 产出 / 自然语言生成
- **方向纠错必须由用户控制** → slash 命令（`/back` `/redo` `/replan` `/use`）

### 1.3 模型假设与目标硬件

**当前部署**：`Qwen3.6-35B-A3B-FP8`（MoE / 3B 激活 / 35B 总参 / 128K context）on NVIDIA GB10

| 指标 | 数据 | 对设计的含义 |
|---|---|---|
| 激活参数 | 3B | 推理速度接近 7B 密集 |
| 总参数 | 35B | 复杂推理质量接近 30B 密集 |
| Context 窗口 | 128K | 长上下文累积**不是瓶颈** |
| FP8 量化 | 无显著质量损失 | / |

**Model-agnostic 原则**：
- 设计**不依赖单一模型强弱**——上限是 35B-A3B 的能力，下限要 7B 密集也能用
- 强模型来时：自动获益（拆解更准、单步生成更稳）
- 弱模型来时：代码硬规则兜底，不崩溃
- 边界由 `AgentHarness` trait 隔离，换模型只换一个文件

### 1.4 目标场景范围

5 mode 架构面向 **单次会话 / 半小时内 / 中等复杂度** 任务：

| 场景规模 | 适用度 | 例子 |
|---|---|---|
| 简单问答 / 翻译 / 解释 | ✅ 完美 | qa agent 直答 |
| 单文档生成 | ✅ 完美 | 周报、邮件、纪要 |
| 单次数据分析 | ✅ 完美 | CSV 探索 + 出图 |
| 半天到一周规划 | ✅ 完美 | 周末出行、项目里程碑 |
| 5-7 天行程 | ✅ 适用 | 35B-A3B + 128K context 下稳定 |
| 8-12 步复杂拆解 | ✅ 适用 | milestone 上限 12，覆盖多约束/多阶段 |

### 1.5 非目标场景

以下场景**架构上**缺关键能力，需要 P2 新增（强行硬塞 5 mode 体验会差）：

| 场景 | 缺什么能力（架构问题，非模型问题） | 路线 |
|---|---|---|
| **超长规划**（15+ 天 / 月度项目） | 嵌套 milestone、变更传播 | P2: `SubAgentRouter` |
| **跨会话持续任务**（每天追踪进度） | 对话状态持久化 | P2: `CheckpointStore` |
| **多文档输出**（行程 + 装备 + 预算分别成册） | 结构化产出形态（多文件树） | P2 |
| **多人协作 / 多用户** | 权限与同步机制 | 不计划 |
| **实时事件驱动**（监控、告警） | 后台 agent loop | 不计划（cron 范畴） |

**关键区分**：
- 「能力跟不上」= **模型问题** → 升级模型自动改善
- 「结构跟不上」= **架构问题**（树状嵌套、跨会话状态等）→ 必须靠 P2 新模块

5 mode 架构解决的是**结构**层，模型强弱在已知边界内**线性影响体验**，但不改变能不能做。

**设计取舍**：保持 5 mode 简单清晰，超长 / 跨会话场景等 SubAgentRouter + CheckpointStore 落地后再覆盖。不为这些场景临时扩 mode，避免污染当前架构。

---

## 二、总体架构

### 2.1 分层架构

```
┌────────────────────────────────────────────────────┐
│                  用户输入                            │
└─────────────────────┬──────────────────────────────┘
                      ↓
              startswith("/")? ─yes→ RollbackManager
                      │ no
                      ↓
┌────────────────────────────────────────────────────┐
│ LLM 单次调用（CombinedPlanner）                      │
│ 输入: 用户消息 + AgentRegistry + Mode 枚举           │
│ 输出: { agent: "...", milestones: [{label, mode,    │
│        tools, prompt_hint}] }                       │
└──────┬──────────────┬──────────────────┬───────────┘
       │              │                  │
   agent=qa      agent=其他          校验失败
       ↓              ↓                  ↓
   流式回答     注入三层 prompt        回退 generic
   (无 milestone) Contract 编排        + 默认 milestones
       │              │
       ↓              ↓
     结束       ┌────────────────┐
                │ ContractRuntime │
                │ + Validator     │
                │ + StepBuilder   │
                │ + ConversationSt │
                │ + RollbackMgr   │
                └────────┬────────┘
                         │ AgentHarness trait
                         ↓
                ┌────────────────────┐
                │ DeepSeekHarness    │
                │ + 工具执行循环      │
                └────────┬───────────┘
                         │
                         ↓
                ┌────────────────────┐
                │ DeepSeek-TUI       │
                │ (LLM client +      │
                │  ToolRegistry +    │
                │  30+ ToolSpec 实现)│
                └────────────────────┘
```

### 2.2 核心原则

1. **扩展不替换**：不改 DeepSeek-TUI 源码。
2. **AgentHarness trait 是唯一边界**：换底层 LLM 引擎只重新实现 trait。
3. **分类与拆解合并**：单次 LLM 调用同时出 agent + milestones。
4. **领域即 prompt**：新 agent = 一个 markdown 文件。
5. **三方分工**：代码硬卡、LLM 做语义、用户控方向。

---

## 三、任务编排流程

### 3.1 整体流程

```
用户输入
   ↓
startswith("/") ? ──yes──> RollbackManager → 状态机变化 → 结束
   │ no
   ↓
┌─ Step 1: LLM 单次调用：分类 + 拆解 ──────────────┐
│  输入: 用户消息 + agent 列表 + Mode 字典         │
│  输出 JSON:                                     │
│  {                                              │
│    "agent": "<id>",                              │
│    "milestones": [                               │
│      { "label", "mode", "tools", "prompt_hint" } │
│    ]                                             │
│  }                                              │
└─────────┬───────────────────────┬───────────────┘
          │                       │
      agent=qa                agent=非 qa
          ↓                       ↓
    Q&A 流式回答              进入 Contract 编排
    (无 milestone)            
          │                       ↓
          │            ┌─ Step 2: 逐 milestone 执行 ─┐
          │            │  本轮 directive 由 mode 决定: │
          │            │  - CallLlm: 调 LLM           │
          │            │  - AskUser: 发选择卡         │
          │            │  - Blocked: 提问预算超       │
          │            │                              │
          │            │  StepBuilder 拼接三层 prompt │
          │            │  → LLM stream                │
          │            │                              │
          │            │  Harness 自动执行循环:        │
          │            │  - tool call → spec.execute()│
          │            │  - 结果回 LLM 继续生成        │
          │            │                              │
          │            │  ContractValidator 硬边界:    │
          │            │  - 工具白名单                 │
          │            │  - 选项数 / 结构校验         │
          │            │                              │
          │            │  推进:                       │
          │            │  - OnChoice: 收到选择即完成   │
          │            │  - OnValidOutput: 校验通过即完成│
          │            └──────────────────────────────┘
          │                       │
          ↓                       ↓
        结束               final_output → 结束
```

### 3.2 分类与拆解：CombinedPlanner

**用一次 LLM 调用同时完成「选 agent」+「拆 milestone」+「为每 milestone 选工具」。** 唯一的代码规则是 `startswith("/")` 判断 slash 命令。

#### Prompt 结构（示意）

```
用户输入: "{user_message}"

可用 agents:
- qa: 简单问答、概念解释、翻译。不需要多步
- doc_generation: 文档生成（周报、邮件、报告）
- data_analysis: 数据分析（Python + 图表）
- planning: 计划制定（出行、项目、目标拆解）
- generic: 兜底

可用 mode（每个 milestone 选一个）:
- collect: 收集用户决策性信息
- produce_options: 给 2-3 个方案让用户选
- refine_selected_option: 细化已选方案
- freeform: 自由产出（写作 / 分析 / 计算）
- final_output: 最终交付（必须是最后一个）

可用工具池: <由 harness.tools() 提供的实际工具名>

输出 JSON:
{
  "agent": "<id>",
  "milestones": [
    { "label", "mode", "tools", "prompt_hint" }
  ]
}

约束:
- agent=qa → milestones=[]
- 否则 milestones 数量 2-12（与 §1.4 对齐，覆盖多约束/多阶段任务）
- 最后一个 mode 必须 final_output
- tools 必须从工具池中选
```

#### 结构性校验（CombinedPlanner.parse_plan）

代码只校验结构，不校验内容合理性：

- agent ∈ AgentRegistry 注册项
- milestones 数量 2-12（qa 为 0）
- 每个 mode 是合法枚举
- 最后一个 mode = final_output
- 每个 tool ∈ available_tools（来自 harness.tools()）
- tools 满足 mode 兼容性（如 final_output 不能含 request_user_input）

校验失败 → 退到 `CombinedPlanner::fallback_plan()`（agent=generic + 三步默认）。

---

### 3.3 三层分工：Mode / Agent / Milestone

**核心原则：每层只管自己的事，互不污染。**

```
┌──────────────────────────────────────────────────────┐
│  Mode 层（结构标签，5 种枚举）                          │
│                                                      │
│  由 LLM 在拆解时选。代码根据 mode 推导:                  │
│   • question_budget（提问预算）                       │
│   • advance_policy（OnChoice / OnValidOutput）       │
│   • output_requirements（结构约束）                   │
│   • mode-tool 兼容性（如 final_output 禁 user_input） │
│                                                      │
│  注入 prompt 的措辞 仅含通用结构语:                     │
│   "必须产出 2-3 个选项"                                │
│   "本阶段最多提问 1 次"                                │
│   "输出最终交付物，不再提问"                            │
│                                                      │
│  ✗ 不写场景词（"成本/时间/风险"、"markdown"）           │
└──────────────────────────────────────────────────────┘
                          ↑
                          │ 协同
                          ↓
┌──────────────────────────────────────────────────────┐
│  Agent 层（prompts/<id>.md 正文）                     │
│                                                      │
│  人写。提供领域风格 + 行业规范:                         │
│   • planning: "做方案对比时必须含成本/时间/风险对比"     │
│   • doc_generation: "选项关注结构、长度、读者"          │
│   • data_analysis: "拿到数据先看 shape 和 dtype"       │
│                                                      │
│  通过 frontmatter description 让 CombinedPlanner    │
│  知道何时选这个 agent；body 在执行阶段注入 prompt。     │
└──────────────────────────────────────────────────────┘
                          ↑
                          │ 协同
                          ↓
┌──────────────────────────────────────────────────────┐
│  Milestone 层（LLM 拆解时填，单次目标）                 │
│                                                      │
│  每个阶段的具体内容:                                    │
│   • label: "选择出行方案"                            │
│   • prompt_hint: "根据用户时长/兴趣给 2-3 个候选"      │
│   • tools: ["request_user_input"]                   │
│                                                      │
│  这一层告诉 LLM "这一步具体要做的事"。                  │
└──────────────────────────────────────────────────────┘
```

**为什么需要 Mode？** 直接让 LLM 输出 `question_budget: 1, advance_policy: "on_choice"` 这种底层枚举，本地小模型容易出错。Mode 是个**LLM 友好的分类标签**，代码反向推导出底层配置。LLM 只要会判断「这一步是收信息 / 出选项 / 自由产出 / 最终输出」就行。

**为什么 Mode 不带场景词？** 同一个 mode 用在不同场景：`produce_options` 用于 planning 时关注「成本对比」，用于 doc_generation 时关注「结构选择」。把场景词写进 mode 等于让 mode 替不相关场景做了决定。场景词的正确归属是 agent prompt。

---

### 3.4 阶段契约：MilestoneContract

每个 milestone 在拆解后挂一个 contract：

```rust
struct MilestoneContract {
    mode: MilestoneMode,                            // LLM 选
    tools: Vec<String>,                             // LLM 选（受 mode 兼容性约束）
    prompt_hint: Option<String>,                    // LLM 填
    required_context: Vec<String>,                  // LLM 填
    produced_context: Vec<String>,                  // LLM 填

    // 以下由 mode 自动推导，LLM 不能改:
    question_budget: u8,
    advance_policy: AdvancePolicy,
    output_requirements: Vec<OutputRequirement>,
}

enum MilestoneMode {
    Collect,                  // 收信息（通常用 request_user_input）
    ProduceOptions,           // 出 2-3 个方案让用户选
    RefineSelectedOption,     // 细化已选方案
    Freeform,                 // 自由产出（写作 / 分析 / 计算）
    FinalOutput,              // 最终交付（要么是最后一个，要么 review 前一个）
    Review,                   // 产物审核：让用户决定满意/微调/重做（可选，建议跟在 final_output 之后）
    PatchOutput,              // 局部修订（read_file + edit_file 做精确 patch）；不在初始拆解出现，由 review tweak 动态插入
}

enum AdvancePolicy {
    OnChoice,        // 用户选择即完成
    OnValidOutput,   // 输出通过校验即完成
    ManualContinue,  // 显式继续（保留扩展）
}
```

#### mode → 内置规则（代码硬编码）

| mode | question_budget | advance_policy | 主要 output_requirements |
|---|---|---|---|
| `collect` | 1 | `on_choice` | requires_tool_call(request_user_input), no_open_question |
| `produce_options` | 1 | `on_choice` | requires_tool_call(request_user_input), min_options=2, max_options=3, no_open_question |
| `refine_selected_option` | 0 | `on_valid_output` | no_open_question |
| `freeform` | 1 | `on_valid_output` | no_open_question |
| `final_output` | 0 | `on_valid_output` | forbid_tool(request_user_input), no_open_question |
| `review` | 1 | `on_choice` | requires_tool_call(request_user_input), min_options=2, max_options=4, no_open_question |
| `patch_output` | 0 | `on_valid_output` | no_open_question；`allowed_tools` 由 engine 动态填 `read_file` + `edit_file` |

理由：这些是契约安全底线，开放配置等于让 LLM 给自己放权。

**`PatchOutput` 的特殊性**：
- **不出现在 CombinedPlanner 初始拆解**——`combined_planner.rs::validate_dto` 显式拒绝
- 由 `apply_choice_result` 在 `Review` 阶段用户选「微调」时**动态构造并插入**到 review 之前
- ID 规则 `patch_<n>`（按已有 patch 数递增），允许多次微调
- allowed_tools 由 engine 填 `[read_file, edit_file]`；**禁止 write_file**（那会全文覆盖违反 patch 语义）
- 完成后回到 review 等用户再次审核（review 自己处于 Pending 状态等 patch Done）

**Review 状态机分支（apply_choice_result 在 engine 层处理）**：
- 选项 label 以「满意」开头 → mark_done(review) → 整体 AllDone
- 选项 label 以「重做」开头 → mark_done(review) + 注入 `review_outcome=redo` context；summary 提示用户走 `/replan`
- 其他选项（微调点）→ **不再 rewind 到 final_output**（会触发全文重写）。改为：
  - 动态构造一个 `PatchOutput` milestone（id=`patch_<n>`，allowed_tools=`[read_file, edit_file]`）
  - 用 `insert_milestone_before(review_id, patch_ms)` 插入到 review 之前
  - patch 变 Active，review 改 Pending；用户选的 label 注入 `review_feedback` context
  - LLM 进 patch 阶段：先 `read_file(last_output_path)`，再用 `edit_file` 做精确 old_string→new_string 替换；不重写整篇
  - patch 完成 → review 再次 Active → 用户再次审核 → 可循环多次

**Patch 路径需要的 context**：
- `review_feedback`：用户选项 label，patch 阶段 prompt 引用作为修订指令
- `last_output_path`：write_file / edit_file 调用时由 `web/mod.rs` 自动捕获 `args.path` 注入，patch 阶段引用以定位文件

---

### 3.5 硬边界 vs 软建议

**这是契约系统能给本地小模型托底的关键。** 严格区分两类规则：

#### 硬规则（违反会被代码拦截）

| 规则 | 校验位置 | 违反后果 |
|---|---|---|
| 调用了 allowed_tools 外的工具 | ContractValidator.validate_tool_call | 拒绝执行，错误回到 LLM |
| 工具与 mode 不兼容（如 final_output 调 request_user_input） | mode_tool_compatibility | 同上 |
| request_user_input 选项数不符 min/max_options | ContractValidator | 不发选择卡 |
| 提问次数超过 question_budget | ContractRuntime → Blocked | 返回提示文案，本轮不调 LLM |
| 输出形态违反 output_requirements（如 must_contain_table） | ContractValidator.validate_response | 不自动推进 |

这些**在 prompt 中以「必须遵守」语气声明**，并且**代码真会拦截**。

#### 软建议（违反不拦截，LLM 自觉）

| 建议 | 来源 |
|---|---|
| "做方案对比时含成本/时间/风险" | agent prompt（planning） |
| "选项之间差异要明显" | agent prompt 或 milestone prompt_hint |
| "结论先行，再说算法" | agent prompt（data_analysis） |
| 文风、措辞、领域偏好 | agent prompt |

这些**在 prompt 中以「建议」语气声明**，违反不拦截。

#### 分节渲染

`ContractPrompt` 应当**显式区分**两类：

```rust
struct ContractPrompt {
    milestone_id: String,
    user_message: String,
    allowed_tools: Vec<String>,
    hard_rules: Vec<String>,    // 违反会被代码拦截
    soft_hints: Vec<String>,    // 仅建议
}
```

StepBuilder 渲染时分两节：

```
## 阶段必须遵守（违反会被系统拦截）
- 选项数量 2-3 个
- 本阶段最多提问 1 次
- 输出必须包含表格

## 阶段建议
- 选项之间差异要明显
```

LLM 看到「必须」与「建议」就知道哪些不能违反。

---

### 3.6 中途纠错与回退

#### 显式回退命令

| 命令 | 行为 |
|------|------|
| `/back` | 当前 milestone 退回 Active，清自身及之后的 context；上一个 Done 回 Active |
| `/skip` | 当前 milestone 标 Skipped，推进下一个 |
| `/redo` | 当前 milestone 重做（清自身 context，前面保留） |
| `/replan` | 整个对话重新拆解，已收集 context 注入新计划 |
| `/use <agent_id>` | 切换 agent（仅 Q&A 或未拆解时合法） |

#### 状态机扩展

```
Pending --start--> Active --done--> Done
                     │                │
                     │                +--back--> Active
                     │
                     +--skip--> Skipped
                     │
                     +--redo--> Active  (清自身 context)
```

#### Context 归属

每条 context 挂在产生它的 milestone：

```rust
struct ContextEntry {
    key: String,
    value: serde_json::Value,
    produced_by: MilestoneId,
}
```

回退时按 `produced_by` 清理，避免污染前序步骤产出。

---

### 3.7 DeepSeekHarness 工具自动执行循环

让 LLM 的 tool call **真的被执行**，而不只是文本声明：

```
chat_stream(req)
   │
   └─ async_stream::stream! {
        loop (软上限 TOOL_LOOP_MAX_ITERATIONS = 12)
        │
        ├─ stream = client.create_message_stream(msg_req)
        │  累积: assistant_text + completed_calls
        │
        ├─ 无 tool call         → yield Done, return
        │
        ├─ 有 tool call:
        │  ├─ name ∈ auto_tool_names ?
        │  │   - 是：spec.execute(args, ctx) → yield ToolCallResult
        │  │         追加 msg_req: assistant ToolUse + user ToolResult
        │  │   - 否：标 has_pass_through（如 request_user_input）
        │  │
        │  ├─ 有透传工具         → yield Done, return（上层处理交互）
        │  └─ 全部 auto         → loop（让 LLM 基于结果继续生成）
        │
        └─ 超 12 轮             → graceful degradation:
                                  ① 通知用户「已达上限，基于已收集信息总结」
                                  ② msg_req.tools = None（禁工具）
                                  ③ 注入 user 消息「不要再调工具，直接总结」
                                  ④ 再做一次 LLM 调用，流式输出
                                  ⑤ yield Done
      }
```

**关键设计原则**：上限是**软兜底**，不是 fatal。任何具体 N 值（5 / 12 / 50）都
可能被合法场景打满（多维度 research、复杂 debug），关键不是「N 多大」，是
「撞了之后体验不崩」。fail-stop 模式会让用户看到红字错误 + 已收集结果全废；
graceful degradation 让 LLM 基于现有信息组织最终回复，保住已经付出的成本。

更精细的失败模式控制（dedup 同工具同 args 重复调用、token budget、墙钟超时）
属于 P2 范畴，本期不展开。

**自动执行的工具**：web_search / fetch_url / read_file / write_file / edit_file /
list_dir / grep_files / file_search / exec_shell + 变体

**透传给上层的工具**：request_user_input（前端渲染选择卡）

工具调用前后还 yield text delta（`🔧 [tool] args` / `📄 结果`），让 web 层
能把工具交互写进 `engine.messages`，跨轮对话 LLM 看得到上下文。

---

### 3.8 Web 层 milestone 自动推进循环

DeepSeek-TUI tool loop（§3.7）只解决「单次 LLM 调用内执行工具」。
跨 milestone 推进需要再起一次 LLM 调用，**但用户体感上应该是连续的**。
Web 层在 `build_milestone_loop_stream` 内做这层串接：**在同一个 SSE 流内**
连续推进多个 `OnValidOutput` 阶段，只在遇到需要用户行为时断流。

```
build_milestone_loop_stream(engine_mutex, prep)
   │
   └─ async_stream! {
        loop (最多 MAX_AUTO_MILESTONE_ADVANCES = 8 轮)
        │
        ├─ active = conv_state.active_milestone()
        │
        ├─ ContractRuntime.next_directive(active, cs, msg) → 分类:
        │   ├─ Blocked/CompleteStep → yield delta+done, return
        │   ├─ AskUser              → yield choice_request, return
        │   └─ CallLlm/FreeFlow     → 构造 chat_req
        │
        ├─ stream = harness.chat_stream(chat_req)
        │  累积: full_text + invoked_tools + choice_state
        │
        ├─ 遇 Done:
        │   ├─ ContractValidator.validate_stage_completion(...)
        │   ├─ 有 choice 卡 (OnChoice)   → yield choice_request, return
        │   ├─ should_advance (OnValidOutput + 通过):
        │   │   ├─ mark_done(active)
        │   │   ├─ 有下一阶段 → yield stage_advanced, continue loop
        │   │   └─ 全部完成   → yield all_milestones_complete, return
        │   └─ !should_advance → yield stage_done_wait, return
        │
        └─ 超上限             → yield Error
      }
```

**断流条件**（流结束 = 等用户下一步操作）：
- `AskUser`：要发选择卡
- `OnChoice` 阶段完成（含 choice card 已发）：等用户做选择
- `OnValidOutput` 阶段校验失败：用户介入纠错
- `Blocked` / `CompleteStep`：契约硬卡
- 错误 / 推进次数超上限

**SSE 事件类型**（前端契约）：

| 事件 | 字段 | 含义 |
|---|---|---|
| `delta` | `delta: <text>` | LLM 文本输出 |
| `milestone` (signal=`AutoAdvance`) | `next_action: Advance` | 阶段自动推进，**不**断流 |
| `done` (signal=`AllDone`) | `next_action: Complete` | 所有 milestone 完成 |
| `done` + milestone (`WaitForUser`) | OnChoice 完成或无 milestone | 等用户输入 |
| `done` + milestone (`WaitValidation`) | `validation_issues: []` | 校验失败，等纠错 |
| `choice_request` | call_id + questions | 渲染选择卡 |

**与 §3.7 的关系**：§3.7 是 LLM 内部 tool loop（单次 chat_stream 内多轮 LLM↔工具）；
§3.8 是 milestone 级 loop（同一个 SSE 响应内多次 chat_stream）。两层互不感知。

---

### 3.9 选择题优于问答题

**核心原则：让用户做决策，不做作文。**

反模式：
```
LLM: "请描述你的数据结构、想分析什么维度、关心什么指标？"
用户: （要打几段字）
```

正确模式：
```
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

实现：

- 复用 DeepSeek-TUI `tools/user_input.rs` 的 `RequestUserInputTool`
- ContractValidator 校验选项数量（2-3）
- harness 把 ToolCallStart 透传给 web，前端渲染选择卡
- 用户选完通过 `tool_result` 回到 stream

---

## 四、Agent 系统

### 4.1 文件布局

```
pinvou3/
├── prompts/
│   ├── qa.md
│   ├── doc_generation.md
│   ├── data_analysis.md
│   ├── planning.md
│   └── generic.md
└── ...
```

启动时扫描，frontmatter 解析为 `AgentDefinition`，body 缓存到内存。

### 4.2 Agent 文件格式

```markdown
---
id: planning
name: 计划制定
description: 制定计划、时间表、行程。模糊目标拆成可执行步骤，识别约束与风险。
emoji: 📅
---

# 角色
你是计划制定助手...

# 适用场景
- 出行、活动安排
- 项目里程碑规划
...

# 风格（领域偏好）
- 步骤具体到「谁在什么时间做什么」
- 做方案对比时必须含成本/时间/风险对比表    ← 场景词写在这里
- 风险章节是必须的，不是装饰

# 注意事项
- ...
```

| 字段 | 必填 | 用途 |
|---|---|---|
| `id` | ✓ | 程序内标识，必须匹配文件名 |
| `name` | ✓ | UI 显示名 |
| `description` | ✓ | 一句话场景描述，供 CombinedPlanner 选 agent |
| `emoji` | 可选 | UI 图标 |

正文 200-400 字，含「角色 / 场景 / 风格 / 注意事项」四块。**所有领域风格、行业规范、场景词都在这一层写**。

### 4.3 System Prompt 组装

执行某个 milestone 时拼接的完整 system prompt：

```
┌──────────────────────────────────────────────────┐
│ ## Agent 角色与风格   ← prompts/<id>.md body      │
│   "你是计划制定助手。做方案对比时必须含成本/时间/风险... " │
├──────────────────────────────────────────────────┤
│ ## 当前阶段           ← milestone.label/prompt_hint │
│   标题: 选择出行方案                                │
│   目标: 根据用户时长/兴趣给 2-3 候选                 │
├──────────────────────────────────────────────────┤
│ ## 阶段必须遵守       ← ContractRuntime.hard_rules │
│   - 必须产出 2-3 个选项                              │
│   - 本阶段最多提问 1 次                              │
│   - 选项必须有 label + description                  │
├──────────────────────────────────────────────────┤
│ ## 阶段建议           ← ContractRuntime.soft_hints │
│   - 选项之间差异要明显                              │
├──────────────────────────────────────────────────┤
│ ## 可用工具                                         │
│   - request_user_input                              │
├──────────────────────────────────────────────────┤
│ ## 已知信息           ← ConversationState.context  │
│   - 时长: 半天                                      │
│   - 兴趣: 徒步休闲                                  │
├──────────────────────────────────────────────────┤
│ ## 用户消息                                          │
└──────────────────────────────────────────────────┘
```

**三层各自来源清晰**，互不污染：

- Agent 层 ← 人写的 markdown
- Milestone 层 ← LLM 拆解时填
- Mode/Contract 层 ← 代码硬编码 + LLM 选的 mode

Q&A 模式不挂 Contract，只注入 Agent body + 用户消息 + 历史。

---

## 五、对话状态机

### 5.1 ConversationState

```rust
struct ConversationState {
    agent_id: Option<String>,                // None = 未初始化
    global_mode: GlobalMode,
    milestones: Vec<(Milestone, MilestoneStatus)>,
    context: HashMap<String, ContextEntry>,  // 含 produced_by
    turn_count: u32,
    plan_initialized: bool,
    question_counts: HashMap<MilestoneId, u8>,
    history: Vec<StateTransition>,
}
```

### 5.2 状态机

```
里程碑级:
Pending --start--> Active --done--> Done --back--> Active
                     │
                     +--skip--> Skipped
                     │
                     +--redo--> Active

全局级 (GlobalMode):
QnAMode | PlanningMode | ExecutingMode | ReplanMode | DoneMode
```

### 5.3 Web 主路径

`/api/chat/stream` 是唯一入口：

- **命令模式**（`startswith("/")`）→ RollbackManager
- **首轮（非命令）** → CombinedPlanner → 根据 agent 路由
- **后续轮 QnA Mode** → 直接转发 LLM 流（保留历史）
- **后续轮 Executing Mode** → ContractRuntime + ContractValidator

---

## 六、执行示例（完整场景）

用户说「我周末要去广州黄埔区水生水库游玩，你来计划」：

```
Round 1: LLM 单次调用（CombinedPlanner）
  → {"agent": "planning", "milestones": [
       {label: "确认偏好",  mode: collect,         tools: [request_user_input]},
       {label: "搜索资源",  mode: freeform,        tools: [web_search]},
       {label: "出方案",    mode: produce_options, tools: [request_user_input]},
       {label: "细化方案",  mode: refine_selected_option, tools: []},
       {label: "输出计划书", mode: final_output,   tools: [file_write]},
       {label: "审核产物",  mode: review,         tools: [request_user_input]}
     ]}

Round 2: "确认偏好" (collect)
  注入 prompts/planning.md 的领域风格
  LLM 调用 request_user_input：时长（半天/一天）、目的（徒步/亲水/家庭）...
  用户选 → context: {duration: "半天", purpose: "徒步"}
  milestone Done，auto-continue → 进入 ms_1

Round 3: "搜索资源" (freeform)
  harness 收到 web_search ToolCallStart
  自动执行 → 拿到 DuckDuckGo 结果
  结果回写 messages，LLM 基于搜索内容生成
  Validator 检查输出结构：通过
  ms_1 Done

Round 4: "出方案" (produce_options)
  LLM 调用 request_user_input 给 3 个方案（含成本/时间/风险对比表，← 来自 agent prompt）
  用户选 A → context: {plan: "A"}
  ms_2 Done

Round 5: "细化方案" (refine_selected_option)
  LLM 输出 A 方案的详细执行
  ms_3 Done

Round 6: "输出计划书" (final_output)
  harness 收到 file_write ToolCallStart
  自动写入 workspace
  自动推进到下一阶段（同 SSE 流内 backend looper 串接）

Round 7: "审核产物" (review)
  LLM 基于产物预判用户可能想改什么，调 request_user_input：
    选项 [
      {label:"满意，按此输出", description:"...", recommended:true},
      {label:"调整时间安排", description:"..."},
      {label:"换成自驾路线", description:"..."},
      {label:"重做，重新规划", description:"..."}
    ]
  分支：
    用户选「满意...」→ mark_done(review) → AllDone
    用户选「调整...」→ 动态插入 patch_output milestone，进 Round 8（不再 rewind 重写）
    用户选「重做...」→ mark_done(review) + 提示用户输 /replan

Round 8 (可选, review 微调时进入): "局部修订" (patch_output)
  状态: 插入 patch_0 在 review 之前；patch_0 Active，review Pending
  Context:
    review_feedback = "调整时间安排"（用户选项 label）
    last_output_path = "plan.md"（来自 Round 6 write_file 的 args.path）
  LLM 流程:
    1. read_file("plan.md") → 拿到当前完整内容
    2. edit_file({path:"plan.md", old:"14:00 徒步", new:"15:00 徒步"}) → 改一处
    3. (可选) 再 edit_file 改另一处
    4. 文字简单说"已把上午徒步调整到下午"——不重输出整篇
  完成 → mark_done(patch_0) → review 再次 Active → 进 Round 9

Round 9: 再次 review
  用户继续选「满意/再微调一次/重做」
  再微调 → 插入 patch_1，循环 Round 8 / Round 9 直到满意

总计: 7+ 轮（review 直接满意）；或 7+2N 轮（N 次微调）
关键: 每次微调只走 patch_output（短 tool args + edit_file 精确替换），
      不再触发 final_output 全文重写——速度从分钟级降到秒级
```

中途纠错示例：

- 用户在 Round 4 后输入 `/back` → ms_2 Done → Active，相关 context 清除
- 用户在 Round 3 后输入 `/replan` → 清空 milestones，重新拆解（保留 collect 阶段的 context）
- 用户在 Round 7 选「调整时间安排」→ 动态插入 patch_output，LLM 走精确 patch 路径（≤10 秒），无需用户输 slash 命令

---

## 七、模块清单

### 7.1 当前实现

| 模块 | 职责 |
|------|------|
| `harness.rs` | `AgentHarness` trait（唯一接口边界） |
| `deepseek_harness.rs` | trait 实现 + **工具自动执行循环** |
| `engine_factory.rs` | 构造 harness、注入 ToolRegistry / ToolContext |
| `agent_registry.rs` | 扫 `prompts/*.md` 解析 frontmatter |
| `combined_planner.rs` | 单次调用：分类 + 拆解 + 工具选择 + 结构校验 |
| `rollback.rs` | slash 命令解析 + 状态机回退 |
| `contract.rs` | `MilestoneContract` + Mode 枚举 + Mode → 规则 |
| `contract_runtime.rs` | 按 mode 决定本轮 directive，产出 hard_rules / soft_hints |
| `contract_validator.rs` | 工具调用 / 输出结构硬边界检查 |
| `workflow.rs` | ConversationState / GlobalMode / Milestone |
| `step_builder.rs` | 拼三层 system prompt |
| `engine.rs` | 编排主入口 |
| `web/mod.rs` | Axum SSE 路径 |
| `web/index.html` | 前端 |
| `prompts/*.md` | 5 个初始 agent |
| `tests/full_flow.rs` | 端到端集成测试 |

### 7.2 DeepSeek-TUI 复用（不修改）

| DeepSeek-TUI 提供 | pinvou 使用方式 |
|---|---|
| `client::DeepSeekClient` | `DeepSeekHarness` 内部封装 |
| `tools::ToolRegistry` / `ToolSpec` | `engine_factory` 用 `ToolRegistryBuilder` 挂工具 |
| `tools::ToolContext` | `with_auto_approve` 构造，注入 harness |
| 30+ ToolSpec 实现 | 通过 ToolRegistry 调用 |
| `models::ContentBlock` / `StreamEvent` | 仅在 `deepseek_harness.rs` 桥接层用 |

### 7.3 待新增（P1）

| 项 | 描述 |
|---|---|
| Hard/Soft 规则分离 | `ContractPrompt` 加 `hard_rules` + `soft_hints` 字段；StepBuilder 分节渲染；mode 指令剥离场景词 |
| 真正的审批流 | 替代 YOLO 模式：`ApprovalRequirement::Required` 工具 → 前端审批 UI |
| 结构化历史 | 工具交互改为 `ContentBlock::ToolUse/ToolResult` 注入 messages |
| 流式分类优化 | CombinedPlanner 解 `agent` 字段一出即路由（去掉 500ms 等待） |
| 真实 LlmClient 链路测试 | mock LlmClient 模拟 SSE 测 chat_stream 工具循环 |

### 7.4 待新增（P2 / 远期）

| 项 | 描述 |
|---|---|
| `CheckpointStore` | 断点持久化 |
| LLM-as-judge | 替代结构性正则的语义校验 |
| 多层 agent 选择 | agent > 20 时分部门选 |
| 隐式回退检测 | LLM 检测「重做 / 不对」信号 |
| TUI 重建 | 基于新架构重写 |

---

## 八、AgentHarness 接口

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

按主题归类。

### 9.1 关于流程控制权

1. **代码做路由器、LLM 做执行者、用户做决策者** — 代码不判断语义，LLM 不做流程
2. **LLM 自评（[OK]/[MORE]/[BLOCKED]）只是信号，推进必须过 contract 验收** — 小模型靠不住
3. **越界即时拦截，不等超时** — 比 pinvou2 的 1200s 超时更快、更可恢复

### 9.2 关于拆解与分类

4. **分类与拆解合并为单次 LLM 调用** — 避免多次往返
5. **命令路由只用 `startswith("/")`** — 不养第二个小模型
6. **动态 milestone 列表自由，mode 仍是枚举** — mode 是 LLM 友好分类标签，让代码反向推导底层配置

### 9.3 关于三层分工（核心）

7. **Mode 只编码结构，场景词在 Agent prompt** — 同一 mode 用在不同场景，场景词侵入会污染
8. **Agent = 一个 markdown 文件** — 零代码扩展；frontmatter + body 两段
9. **Mode → 规则映射代码硬编码** — 这是安全底线，开放配置等于让 LLM 给自己放权
10. **工具池全局共享，LLM 按 milestone 挑** — App 概念解耦；危险等级由代码控制

### 9.4 关于硬边界与软建议

11. **Prompt 显式分「必须遵守」和「建议」两节** — LLM 才能区分哪些违反会被拦截
12. **OutputRequirement 用结构性正则**（如 markdown table 必须有 `|---|` 分隔行） — 不用关键词匹配（脆弱）；语义校验由 LLM judge 在 P1 加

### 9.5 关于交互模式

13. **选择题优于问答题** — 用户做决策不做作文
14. **状态机支持显式回退（/back, /redo, /replan, /use）** — 真实对话不是单向
15. **Context 按 produced_by 归属** — 回退时精准清理

### 9.6 关于底层与复用

16. **AgentHarness trait 是唯一接口边界** — 换底层只重新实现 trait
17. **优先复用 DeepSeek-TUI，不修改源码** — 工具 / TUI 组件已有
18. **DeepSeekHarness 内嵌工具自动执行循环** — pinvou 用 LlmClient 直连而非 DeepSeek-TUI 完整 engine
19. **YOLO 模式作为 MVP 默认**（auto_approve=true）— 本地单用户 + workspace 边界够安全
20. **工具交互以 text delta 注入 messages**（MVP） — 跨轮历史可见；结构化注入 P1

### 9.7 关于前端

21. **前端不发 app_id，用户直接输入需求** — App 选择是无谓仪式
22. **slash 命令显式控制流程** — `/use` 切 agent / `/replan` 重拆 / `/back` 回退

---

> 本文档是 pinvou3 的总纲领。所有实现决策、模块边界、接口定义以此为准。
>
> 最后更新: 2026-05-11
