# pinvou3 设计架构文档

> 基于 DeepSeek-TUI 二次开发的本地 AI 平台。运行于 NVIDIA GB10，面向普通用户，
> 覆盖文档生成、数据分析、计划制定、知识问答等场景。
>
> 最后更新：2026-05-12

---

## 一、定位与原则

### 1.1 是什么

通用 AI 助手 web app。非编码工具，非聊天玩具。零代码扩展：加 agent =
加一个 `prompts/<id>.md`。

### 1.2 三方分工（核心原则）

```
代码硬卡（contract / mode 规则）
    ↕
LLM 做语义（拆解 / 产出 / 自然语言）
    ↕
用户控方向（slash 命令 /back /redo /replan）
```

每方只做自己擅长的事。本地 LLM 不可靠，**用代码 + 用户兜底**。

### 1.3 模型假设

当前部署 `Qwen3.6-35B-A3B-FP8`（MoE / 3B 激活 / 35B 总参 / 32K context）on
NVIDIA GB10 + vLLM。设计**模型无关**——上限取决于模型强弱，下限要 7B 密集
也能用。代码硬规则兜底，不靠 LLM 自觉。

### 1.4 适用场景与边界

| 场景 | 适用度 |
|---|---|
| 简单问答 / 翻译 / 解释 | ✅ qa agent 直答 |
| 单文档生成（周报 / 邮件 / 纪要） | ✅ |
| 单次数据分析 | ✅ |
| 半天到一周规划 | ✅ |
| 5-7 天行程 | ✅ |
| 8-12 步复杂拆解 | ✅ |
| 15+ 天 / 跨会话 / 多文档输出 | ❌ 缺嵌套 milestone / checkpoint，等 P2 SubAgentRouter 才覆盖 |
| 多人协作 / 实时事件驱动 | ❌ 不计划 |

---

## 二、架构总览

### 2.1 分层

```
┌─────────────────────────────────────────────┐
│              用户输入                          │
└──────────────────┬──────────────────────────┘
                   ↓
            slash 命令？──yes→ RollbackManager → 状态机变化
                   │ no
                   ↓
        CombinedPlanner（单次 LLM 调用）
        → { agent, milestones[{label, mode, tools, hint}] }
                   │
        ┌──────────┴──────────┐
        ↓                     ↓
    agent=qa              其他 agent
    流式答（无 ms）       逐 milestone 执行
                              │
                              ↓
            ┌─────────────────────────────────┐
            │ ContractRuntime: 按 mode 出 directive │
            │   CallLlm / AskUser / Blocked        │
            │ ContractValidator: 硬规则拦截         │
            │ Web Looper: 同流串接 OnValidOutput   │
            │ AgentHarness: LLM 调用 + 工具执行    │
            └────────────┬────────────────────┘
                         │
                  AgentHarness trait
                         ↓
                 DeepSeekHarness（当前实现）
                         ↓
                 DeepSeek-TUI Client + ToolRegistry
```

### 2.2 核心原则

1. **扩展不替换**：不改 DeepSeek-TUI 源码（详见 §五唯一例外：fork 中的 vLLM
   `chat_template_kwargs` 修复 + `DEEPSEEK_MAX_OUTPUT_TOKENS` env，已提 PR）
2. **AgentHarness trait 是唯一边界**：换底层只重新实现 trait
3. **复用优先**：在 pinvou-platform 新增任何能力前先检查 DeepSeek-TUI 是否已有
4. **领域即 prompt**：新 agent = 一个 markdown 文件
5. **代码做路由器、LLM 做执行者、用户做决策者**

---

## 三、核心机制

### 3.1 编排流程

**首句拆解 + 逐阶段执行**：

```
首条用户消息（非 slash）
    ↓
CombinedPlanner: 单次 LLM 调用同时
  - 选 agent（qa / planning / doc_generation / data_analysis / generic）
  - 拆 milestones（每个含 label / mode / tools / prompt_hint）
    ↓
agent=qa → 直接流式回答，无 milestone
其他 → 进入逐 milestone 执行
    ↓
每个 milestone 通过 ContractRuntime 决定本轮 directive：
  - CallLlm: 调 LLM
  - AskUser: 发选择卡
  - Blocked: 预算超
    ↓
  Validator 硬规则检查 → Looper 推进或断流
```

### 3.2 Mode / Agent / Milestone 三层分工

**核心：每层只管自己的事，互不污染**：

| 层 | 来源 | 内容 |
|---|---|---|
| **Mode** | 代码硬编码 | 结构标签（7 种枚举）；推导 question_budget / advance_policy / output_requirements |
| **Agent** | `prompts/<id>.md` | 领域风格 + 行业规范 + 场景词 |
| **Milestone** | LLM 拆解时填 | 单次目标 label + prompt_hint + tools |

7 种 mode：

```rust
enum MilestoneMode {
    Collect,                // 收信息（用 request_user_input）
    ProduceOptions,         // 出 2-3 个方案让用户选
    RefineSelectedOption,   // 细化已选方案
    Freeform,               // 自由产出（写作 / 分析 / 搜索）
    FinalOutput,            // 最终交付（必须是 last 或 review 前）
    Review,                 // 产物审核：满意/微调/重做（可选）
    PatchOutput,            // 局部修订 read_file + edit_file 做 patch；
                            // 不在初始拆解出现，由 Review tweak 动态插入
}
```

**为什么 Mode 是枚举而不是 LLM 自由填**：本地小模型对结构化分类（5-7 种枚举）
比对开放配置（question_budget=1, advance_policy=on_choice）准确得多。

**为什么 Mode 不带场景词**：同一个 `produce_options` 用在 planning 时关注
「成本对比」，用在 doc_generation 时关注「结构选择」。场景词的归属是 agent
prompt，不是 mode。

### 3.3 Mode → 规则映射

| mode | question_budget | advance_policy | 主要 output_requirements |
|---|---|---|---|
| `collect` | 1 | on_choice | requires_tool_call(request_user_input), no_open_question |
| `produce_options` | 1 | on_choice | requires_tool_call(request_user_input), min_options=2, max_options=3, no_open_question |
| `refine_selected_option` | 0 | on_valid_output | no_open_question |
| `freeform` | 0 | on_valid_output | no_open_question |
| `final_output` | 0 | on_valid_output | forbid_tool(request_user_input), no_open_question |
| `review` | 1 | on_choice | requires_tool_call(request_user_input), min_options=2, max_options=4 |
| `patch_output` | 0 | on_valid_output | no_open_question；engine 动态填 `[read_file, edit_file]` |

### 3.4 硬规则 vs 软建议

```rust
struct ContractPrompt {
    milestone_id: String,
    user_message: String,
    allowed_tools: Vec<String>,
    hard_rules: Vec<String>,    // 违反会被 ContractValidator 拦截
    soft_hints: Vec<String>,    // 仅建议，不拦
}
```

| 类型 | 例子 | 拦截位置 |
|---|---|---|
| 硬规则 | `produce_options` 必须含 `request_user_input` 工具 / 选项数 2-3 / 描述 ≥ 30 字 | ContractValidator |
| 软建议 | "做方案对比时含成本/时间/风险对比" / "选项之间差异要明显" | agent prompt |

StepBuilder 渲染时**分两节**渲染，LLM 看到「必须遵守」与「建议」就知道哪些不能违反。

### 3.5 状态机

```
里程碑级:
Pending → Active → Done
              ↓ skip → Skipped
              ↓ redo → Active (清自身 context)
              ↓ back → 上一个 Active

全局级 (GlobalMode):
QnA | Executing | Replan | Done
```

`ConversationState`：

```rust
struct ConversationState {
    agent_id: Option<String>,
    global_mode: GlobalMode,
    milestones: Vec<(Milestone, MilestoneStatus)>,
    context: HashMap<String, ContextEntry>,  // 含 produced_by 归属
    turn_count: u32,
    plan_initialized: bool,
    question_counts: HashMap<MilestoneId, u8>,
    history: Vec<StateTransition>,
}
```

**关键操作**：
- `mark_done(id)`：把 milestone 标 Done + 激活下一个
- `rewind_to(id)`：把目标 milestone 标 Active + 之后的 Done 标 Pending +
  清除受影响 context
- `insert_milestone_before(target, ms)`：动态插入（Review tweak 用，插
  PatchOutput）

### 3.6 中途纠错

| 命令 | 行为 |
|---|---|
| `/back` | 当前 milestone Done → Active，清自身及之后 context |
| `/skip` | 当前 milestone 标 Skipped，推进下一个 |
| `/redo` | 当前 milestone 重做（清自身 context） |
| `/replan` | 整个对话重新拆解，已收集 context 注入新计划 |
| `/use <agent>` | 切 agent（仅 QnA 或未拆解时合法） |

**Review 选择分支**（apply_choice_result 在 engine 层处理）：

| 用户选项 label 前缀 | 行为 |
|---|---|
| 「满意」 | mark_done(review) → AllDone |
| 「重做」 | mark_done(review) + 注入 review_outcome=redo + summary 提示 `/replan` |
| 其他（微调点） | 动态构造 PatchOutput milestone 插入到 review 之前；review 改 Pending；用户选项作为 review_feedback context |

### 3.7 流式编排两层 loop

**Layer 1: 工具自动执行循环（在 AgentHarness 实现内部）**：

```
chat_stream(req)
  loop (软上限 12 轮)
    stream = client.create_message_stream(msg_req)
    累积 assistant_text + tool_calls
    if 无 tool_call → yield Done
    if 副作用工具（write_file/exec_shell 等）成功 → yield Done（避免复述）
    if request_user_input → yield Done（透传上层）
    else: spec.execute + 结果回流 messages → continue
  超 12 轮 → graceful degradation（禁工具 + 提示 LLM 总结收尾）
```

软上限是兜底，不是 fatal error。撞了通知 LLM 基于已收集信息总结，不杀流。

**Layer 2: Web milestone looper（在 web/mod.rs）**：

```
build_milestone_loop_stream(engine_mutex, prep)
  loop (软上限 8 个阶段)
    active = conv_state.active_milestone()
    directive = ContractRuntime.next_directive(active, cs, msg)
    ├ Blocked/CompleteStep → yield delta+done, return
    ├ AskUser → yield choice_request, return
    └ CallLlm/FreeFlow → 构造 chat_req
        stream = harness.chat_stream(chat_req)
        累积 full_text + invoked_tools + choice_state
        Done:
          ├ 有选择卡 → yield choice_request, return
          ├ should_advance（OnValidOutput + 校验通过）→
              mark_done(active) →
              ├ 有下一阶段 → yield stage_advanced, continue loop
              └ 全部完成 → yield all_milestones_complete, return
          └ 不推进 → yield stage_done_wait, return
```

**关键**：OnValidOutput 阶段（freeform/refine/final_output/patch_output）完成后
**不断流**，继续下一阶段；OnChoice 阶段（collect/produce_options/review）完成
后**必断流**等用户行为。两条路径对称：都是「需要用户行为」才断流。

**SSE 事件（前端契约）**：

| 事件 | 含义 |
|---|---|
| `delta: <text>` | LLM 文本输出 |
| `milestone signal=AutoAdvance next_action=Advance` | 阶段自动推进，**不**断流 |
| `done signal=AllDone next_action=Complete` | 所有 milestone 完成 |
| `done milestone={next_action: WaitForUser}` | OnChoice 完成等用户 |
| `done milestone={next_action: WaitValidation}` | 校验失败等纠错 |
| `choice_request` | 渲染选择卡 |

### 3.8 选择卡设计

**信息密度优化**（避免用户「选了不知道选了什么」）：

- `description` 字段必须 ≥ 30 字（ContractValidator 硬规则）
- `description` 支持 markdown（前端用 marked.js 渲染粗体/列表/表格）
- `recommended: true` 标记 → 前端显示「推荐」橙色 badge
- LLM 在调 request_user_input **之前**的流式文本作为「context 区」展示
  在选择卡顶部（web/mod.rs 用 `strip_tool_banners` 清掉技术横幅）

### 3.9 拆解结构性校验（CombinedPlanner.parse_plan）

代码只校验结构：

- agent ∈ AgentRegistry
- milestones 数量 2-12（qa 为 0）
- mode 是合法枚举
- 最后一个 mode 必须 final_output 或 review（review 前必须 final_output）
- 中间不能出现 final_output / review / patch_output

**工具名容错（软错误，不整盘 fallback）**：

1. 已知 alias 表（`file_read` → `read_file` 等高频反序 typo）→ 直接映射
2. Levenshtein 距离 ≤ 2 → 替换为最近的 available_tools 项
3. 都不命中 → 静默移除该工具 + log warn，plan 继续

理由：LLM 偶发把 `read_file` 写成 `file_read` 不应让整个 planning agent 拆解
退到 generic 3 阶段 fallback。容错让 95% typo case 仍能进入正确 agent。

**硬错误（仍然整盘 fallback）**：mode 顺序错 / agent 不存在 / RequiresToolCall
违反（normalize 后再次检查）。

---

## 四、Agent 系统

### 4.1 文件布局

```
prompts/
├── qa.md
├── doc_generation.md
├── data_analysis.md
├── planning.md
└── generic.md
```

启动时扫描，frontmatter 解析为 `AgentDefinition`，body 缓存到内存。

### 4.2 文件格式

```markdown
---
id: planning
name: 计划制定
description: 模糊目标拆成可执行步骤，识别约束与风险。
emoji: 📅
---

# 角色
你是计划制定助手...

# 适用场景
- 出行、活动安排
- 项目里程碑规划

# 风格
- 步骤具体到「谁在什么时间做什么」
- 做方案对比时必须含成本/时间/风险/收益对比

# 注意事项
- 信息不全时用 request_user_input 选择题
- 不拍脑袋编预算数字
```

`description` 必填，CombinedPlanner 用它来选 agent。body 200-400 字含
「角色 / 场景 / 风格 / 注意事项」四块，**所有领域风格、场景词都在这一层**。

### 4.3 System prompt 组装

每个 milestone 执行时拼接的完整 system prompt：

```
## Agent 角色与风格   ← prompts/<id>.md body
## 当前阶段           ← milestone.label / prompt_hint
## 阶段必须遵守       ← ContractRuntime.hard_rules
## 阶段建议           ← ContractRuntime.soft_hints
## 可用工具
## 已知信息           ← ConversationState.context
## 用户消息
```

QnA 模式不挂 Contract，只注入 Agent body + 用户消息 + 历史。

---

## 五、模块清单（当前实现状态）

### 5.1 已实现

| 模块 | 职责 |
|---|---|
| `harness.rs` | `AgentHarness` trait + ChatRequest / StreamEvent / ToolDef |
| `deepseek_harness.rs` | trait 实现（**Legacy 路径**，自写 tool loop） |
| `engine_harness.rs` | `EngineHarness` 实现（**Engine 路径**，包装 DeepSeek-TUI EngineHandle）；`Harness` enum dispatch；**未完成**，详见 `engine-refactor-status.md` |
| `engine_factory.rs` | 构造 harness、按 env `PINVOU_USE_ENGINE_HARNESS` 切换 |
| `agent_registry.rs` | 扫 `prompts/*.md` 解析 frontmatter |
| `combined_planner.rs` | 单次调用：分类 + 拆解 + 工具选择 + 结构校验 + 工具名容错 |
| `rollback.rs` | slash 命令解析 + 状态机回退 |
| `contract.rs` | `MilestoneContract` + Mode 枚举（7 种）+ Mode → 规则映射 |
| `contract_runtime.rs` | 按 mode 决定本轮 directive，产出 hard_rules / soft_hints |
| `contract_validator.rs` | 工具调用 / 输出结构硬边界检查（含 description 长度） |
| `workflow.rs` | ConversationState / GlobalMode / Milestone / insert_milestone_before |
| `step_builder.rs` | 拼三层 system prompt |
| `engine.rs` | 编排主入口 + Review/PatchOutput 状态机分支 |
| `web/mod.rs` | Axum SSE 路径 + Backend looper（Layer 2 loop） |
| `web/index.html` | 前端 SSE 渲染 + 选择卡 markdown + RAF 节流 |
| `prompts/*.md` | 5 个 agent |
| `tests/full_flow.rs` | 端到端集成测试 |

### 5.2 DeepSeek-TUI 复用

| 提供 | pinvou 使用方式 |
|---|---|
| `client::DeepSeekClient` | `DeepSeekHarness` 内部封装 |
| `tools::ToolRegistry` / `ToolSpec` | `engine_factory::build_default_tool_registry` 挂工具 |
| `tools::ToolContext` | `with_auto_approve` 构造 |
| 30+ ToolSpec 实现 | 通过 ToolRegistry 调用 |
| `core::engine::EngineHandle` | Engine 路径包装（**未完成**） |

### 5.3 DeepSeek-TUI fork 改动（pinvou3 专用，已 PR 给上游待审）

| 改动 | 说明 |
|---|---|
| `crates/tui/src/client.rs` | Vllm provider 的 `reasoning_effort=off` 注入 `chat_template_kwargs.enable_thinking=false`（替代 Anthropic 风格 `thinking: {type: disabled}`）。已提 PR |
| `crates/tui/src/core/engine/context.rs` | `effective_max_output_tokens` 支持 `DEEPSEEK_MAX_OUTPUT_TOKENS` env override（vLLM 小 context 模型用） |
| `crates/tui/src/lib.rs` | 把 binary-only 模块 export 成 library（pinvou-platform 接入需要，fork-only 不 PR） |
| `crates/tui/src/llm_client/mod.rs` | `LlmClient::create_message_stream` 改 RPIT（外部 crate 实现 trait 需要，fork-only） |

### 5.4 待新增 / 未完成

| 项 | 说明 |
|---|---|
| **EngineHandle 重构** | 切到 DeepSeek-TUI engine，删除 1000+ 行自写 tool loop。卡在 request_user_input 协议适配（详见 `engine-refactor-status.md`） |
| 真正的审批流 | 当前 YOLO 模式；EngineHandle 路径自带（`Event::ApprovalRequired`） |
| Tool args 生成期进度反馈 | EngineHandle 路径自带（`Event::ToolCallProgress`） |
| `CheckpointStore` | 跨会话持久化（P2） |
| LLM-as-judge | 替代结构性正则的语义校验（P2） |

---

## 六、典型执行流程

用户输入「我周六要去广州黄埔区水声水库徒步」：

```
Round 1: CombinedPlanner 单次调用
  → {"agent": "planning", "milestones": [
       {label: "收偏好",   mode: collect,         tools: [request_user_input]},
       {label: "搜资源",   mode: freeform,        tools: [web_search]},
       {label: "出方案",   mode: produce_options, tools: [request_user_input]},
       {label: "细化",     mode: refine_selected_option, tools: []},
       {label: "输出计划", mode: final_output,    tools: [write_file]},
       {label: "审核",     mode: review,         tools: [request_user_input]}
     ]}

Round 2: "收偏好" (collect)
  LLM 调 request_user_input：时长/同行人/兴趣 → 用户选 → context 记录 → ms_0 Done

Round 3: "搜资源" (freeform)
  harness 自动执行 web_search → 结果回流 → LLM 综合 → Validator 通过 →
  Backend looper 在同一个 SSE 流内自动推进到 ms_2

Round 4: "出方案" (produce_options)
  LLM 调 request_user_input 给 3 方案（含成本/时间/风险对比表）→ 用户选 →
  ms_2 Done

Round 5: "细化" (refine_selected_option)
  LLM 输出选定方案的详细执行 → ms_3 Done → 同流推进 ms_4

Round 6: "输出计划" (final_output)
  LLM 流式输出完整 markdown → 调 write_file → 副作用工具断流（避免复述）→
  ms_4 Done → 同流推进 ms_5

Round 7: "审核" (review)
  LLM 调 request_user_input 给选项 [满意, 调整时间, 换交通, 重做]
  用户选「调整时间」→ engine 动态插入 patch_0 (PatchOutput) →
  ms_5 改 Pending，patch_0 Active

Round 8: "局部修订" (patch_output)
  Context 含 review_feedback="调整时间" + last_output_path="plan.md"
  LLM 调 read_file → edit_file 做精确替换 → ms (patch_0) Done →
  review (ms_5) 再次 Active → 进 Round 9

Round 9: 再次 review
  用户选「满意」→ mark_done(review) → AllDone

总计 7+2N 轮（N = 微调次数）；每次微调 ≤10 秒，不再 final_output 全文重写
```

---

## 七、AgentHarness 接口

```rust
#[async_trait]
pub trait AgentHarness: Send + Sync {
    async fn chat_stream(&self, req: ChatRequest)
        -> Result<Box<dyn Stream<Item = Result<StreamEvent>> + Send + Unpin>>;
    async fn chat(&self, req: ChatRequest) -> Result<String>;  // 默认走 stream 累积
    fn tools(&self) -> Vec<ToolDef>;
    fn models(&self) -> Vec<ModelInfo>;
    fn save_checkpoint(&self, state: &Checkpoint) -> Result<()>;
    fn load_checkpoint(&self, id: &str) -> Result<Option<Checkpoint>>;
    fn list_sessions(&self) -> Result<Vec<String>>;
    fn workspace_dir(&self) -> PathBuf;
}
```

**两个实现**：

- `DeepSeekHarness<C: LlmClient>`：Legacy 路径，自写 tool loop 走 `LlmClient`
  低层接口。当前默认使用。
- `EngineHarness`：Engine 路径，包装 DeepSeek-TUI 的 `EngineHandle`（消息
  passing 接口）。未完成，详见 `engine-refactor-status.md`。
- `Harness` enum dispatch 两者，env `PINVOU_USE_ENGINE_HARNESS=1` 切换。

---

## 八、关键设计决策

按主题归类。

### 8.1 控制权与协议

- **代码做路由、LLM 做执行、用户做决策** —— 三方不越权
- **越界即时拦截**（ContractValidator），不等超时
- **CombinedPlanner 单次拆解** —— 避免多次往返
- **命令路由用 `startswith("/")`** —— 不养第二个小模型

### 8.2 三层分工

- **Mode 是 LLM 友好的分类标签** —— 代码反向推导底层配置；不直接让 LLM 选
  on_choice/question_budget 这种枚举
- **Mode 不带场景词** —— 同一 mode 用在不同场景，场景词归属 agent prompt
- **Mode → 规则映射代码硬编码** —— 安全底线，开放配置等于让 LLM 给自己放权
- **Agent = 一个 markdown 文件** —— 零代码扩展

### 8.3 硬边界与软建议

- **Prompt 显式分「必须遵守」与「建议」两节**
- **OutputRequirement 用结构性正则** —— 不用关键词匹配（脆弱）；语义校验留 P2 LLM-as-judge

### 8.4 交互

- **选择题优于问答题** —— 用户做决策不做作文
- **选择卡 description ≥ 30 字 + markdown 渲染 + 上下文段同屏**
- **状态机支持显式回退**（`/back` `/redo` `/replan` `/use`）—— 真实对话不是单向
- **Context 按 `produced_by` 归属** —— 回退时精准清理

### 8.5 流式与编排

- **两层 loop**：harness 内部 tool loop + web 层 milestone looper
- **OnValidOutput 阶段同 SSE 流串接** —— 不强迫用户手动「继续」
- **OnChoice 阶段必断流等用户** —— 两条路径对称：都是「需要用户行为」才断流
- **副作用工具完成后断流** —— 避免 LLM 复述

### 8.6 修订与微调

- **Review 是第 6 个 mode** —— 让用户拿到产物后能微调
- **PatchOutput 是第 7 个 mode** —— 走 edit_file 精确 patch，不全文重写
- **PatchOutput 不在初始拆解中** —— 由 Review tweak 动态插入
- **工具名容错（fuzzy match + alias 表）** —— LLM typo 不让整盘 fallback

### 8.7 底层与复用

- **AgentHarness trait 是唯一接口边界** —— 换底层只重新实现
- **优先复用 DeepSeek-TUI，不修改源码**（fork 上的少量 fix 已 PR 给上游）
- **EngineHandle 重构** —— DeepSeek-TUI engine 已有完整 agent loop，pinvou3
  应该复用而不是重写。重构未完成，见 `engine-refactor-status.md`
- **YOLO 模式作为 MVP 默认**（auto_approve=true）—— 本地单用户 + workspace
  边界够安全；真审批流走 EngineHandle 路径（P1）

### 8.8 前端

- **用户直接输入需求** —— App 选择是无谓仪式
- **slash 命令显式控制流程** —— `/use` 切 agent / `/replan` 重拆 / `/back` 回退
- **SSE delta RAF 节流** —— 避免每个 token 都全量 markdown re-parse 的 O(N²)

---

## 九、未完成工作

- **EngineHandle 重构**（最重要的架构债）：详见 `engine-refactor-status.md`。
  当前 pinvou-platform 自写了 300+ 行 tool loop，跟 DeepSeek-TUI 已有的
  `core/engine/` (9673 行) 重复。重构卡在 `request_user_input` 协议适配。
- **QnA 模式锁定** —— 首句问候后所有任务被当 qa。需要在 QnA mode 下检测
  "新任务"信号 → 自动重新拆解
- **5+ 阶段拆解的用户体感** —— 简单任务有时被拆得过细。需要 prompt 引导
  "短任务别拆" 或加 "short-form" 规划路径
- **Tool args 生成期进度反馈** —— 当前前端只看到「⏳ 正在调用工具」静默；
  EngineHandle 路径自带 `Event::ToolCallProgress`，重构后白送
- **多文档输出 / 长会话持续** —— P2 SubAgentRouter + CheckpointStore

---

> 本文档是 pinvou3 总纲领。所有实现决策、模块边界、接口定义以此为准。
