# pinvou3 Phase 3 — 本地模型编排层 Spec

> 基准文档：`设计架构文档-pinvou3.md`（项目根），本文档是其 Phase 3 实现规范。
>
> 状态：待实现 | 日期：2026-05-08

---

## 一、概述

### 1.1 目标

Phase 3 实现 pinvou3 的**本地模型编排层**——将架构文档中定义的「代码做路由器、LLM 做执行者、用户做决策者」三角色分离设计落地为可运行的 Rust 代码。

### 1.2 四个模块

| 模块 | 文件 | 职责 | 依赖 |
|------|------|------|------|
| **DeepSeekHarness** | `pinvou-platform/src/deepseek_harness.rs` | 实现 `AgentHarness` trait，对接本地 LLM | `LlmClient` trait（已有） |
| **StepBuilder** | `pinvou-platform/src/step_builder.rs` | 构造小范围执行 prompt + 拆解/审阅 prompt | `AppConfig`, `Milestone`, `ConversationState` |
| **LLMReviewer** | `pinvou-platform/src/reviewer.rs` | 调用 LLM 审阅拆解结果（语义检查） | `AgentHarness`, `StepBuilder` |
| **ResponseChecker** | `pinvou-platform/src/response_checker.rs` | 解析自评信号 + 越界检测 + 路由决策 | `Milestone`, `AppConfig` |

### 1.3 总原则

- **独立 crate**：pinvou-platform 独立于 DeepSeek-TUI，通过 path dependency 复用 `LlmClient`/`DeepSeekClient`/`MessageRequest`/`ToolRegistry` 等底层能力
- **宽松解析 + 降级兜底**：格式解析失败不阻塞流程，默认放行
- **无状态模块**：StepBuilder / ResponseChecker 为纯函数，不持有状态
- **用户确认是安全网**：强制确认拆解结果，越界/阻塞时停住等用户

---

## 二、数据流全景

```
PlatformEngine.decompose_and_execute()
│
├─ 1. StepBuilder::build_decomposition() → decomposition prompt
├─ 2. harness.chat(prompt) → LLM 拆解输出
├─ 3. 可解析性检查（代码：非空？有序号？可提取？）
│     └─ 不过 → 重拆 (max 2)
├─ 4. LLMReviewer::review() → 语义审阅
│     └─ NG → format_feedback() 注入重拆 prompt → 回到 step 2 (max 2)
├─ 5. 用户确认 gate（TUI 渲染里程碑列表）
│
└─ 6. 逐步执行循环（对每个 active milestone）:
    ├─ a. StepBuilder::build() → 小范围 prompt
    ├─ b. harness.chat(prompt) → LLM 完成任务
    ├─ c. ResponseChecker::check() → 信号解析 + 越界检测 → NextAction
    └─ d. 根据 NextAction 推进/等待/阻断
```

---

## 三、模块详细设计

### 3.1 DeepSeekHarness

**文件：** `crates/tui/src/platform/deepseek_harness.rs`

#### 接口

```rust
pub struct DeepSeekHarness<C: LlmClient> {
    client: C,
    tools: Vec<ToolDef>,
    models: Vec<ModelInfo>,
    workspace: PathBuf,
    checkpoint_dir: PathBuf,
}

impl<C: LlmClient> DeepSeekHarness<C> {
    pub fn new(
        client: C,
        tools: Vec<ToolDef>,
        models: Vec<ModelInfo>,
        workspace: PathBuf,
    ) -> Self;

    /// 配置 checkpoint 存储目录，默认 workspace/.checkpoints
    pub fn with_checkpoint_dir(mut self, dir: PathBuf) -> Self;
}

#[async_trait]
impl<C: LlmClient + Send + Sync> AgentHarness for DeepSeekHarness<C> {
    // 见 AgentHarness trait 定义
}
```

#### 核心映射：ChatRequest → MessageRequest

```
ChatRequest.user_message             → Message.messages[].role="user", content=[TextBlock]
ChatRequest.platform_system_prompt   → Message.system
ChatRequest.context                  → 拼入 system prompt 或首条 user message
ChatRequest.tools                    → MessageRequest.tools (ToolDef → 内部 Tool 格式)
ChatRequest.model                    → MessageRequest.model
```

#### 核心映射：StreamEvent ↔ 流式事件

```
LlmClient 返回的 StreamEventBox 事件:
  ContentBlockStart(Text)    → (忽略，后续 TextDelta 携带内容)
  ContentBlockDelta(Text)    → StreamEvent::TextDelta { content }
  ContentBlockStart(ToolUse) → StreamEvent::ToolCallStart { call_id, tool_name, arguments }
  ContentBlockDelta(ToolResult) → StreamEvent::ToolCallResult { call_id, output }
  MessageStop                → StreamEvent::Done
  Error                      → StreamEvent::Error { message }
```

#### 依赖现有模块

| 现有模块 | 用途 |
|---------|------|
| `crate::llm_client::LlmClient` | LLM 调用底层 interface |
| `crate::client::DeepSeekClient` | 生产环境 HTTP 客户端 |
| `crate::llm_client::mock::MockLlmClient` | 测试用 mock |
| `crate::llm_client::RetryConfig` | 重试配置（可复用 `with_retry`） |
| `crate::models::{MessageRequest, Message, ContentBlock}` | 请求/响应类型 |
| `crate::tui::streaming::StreamingState` | 流式渲染（TUI 侧使用） |
| `crate::tools::ToolRegistry` | 工具注册表（提取 ToolDef 列表） |

#### Checkpoint 持久化

```
格式: JSON 文件
路径: {checkpoint_dir}/{session_id}.json
内容: { session_id, app_id, conversation_state (serialized), created_at }
```

#### 验证

1. **单元测试**：注入 `MockLlmClient` 验证 `chat()` / `chat_stream()` 映射正确
2. **集成测试**：需要本地 Ollama 运行时，`#[ignore]` 标记

---

### 3.2 StepBuilder

**文件：** `crates/tui/src/platform/step_builder.rs`

#### 接口

```rust
pub struct StepBuilder;

#[derive(Debug, Clone)]
pub struct StepPrompt {
    pub system: String,          // 替代默认 system prompt 的小范围指令
    pub append_user_message: bool, // true = 在 system 外追加用户消息
}

impl StepBuilder {
    /// 为当前里程碑构造小范围执行 prompt
    pub fn build(
        milestone: &Milestone,
        context: &HashMap<String, String>,
        user_message: &str,
        app_config: &AppConfig,
    ) -> StepPrompt;

    /// 构造任务拆解 prompt（架构文档 3.3 节）
    pub fn build_decomposition(
        user_request: &str,
        app_config: &AppConfig,
        available_tools: &[String],
        context_summary: &str,
    ) -> String;

    /// 构造审阅 prompt（架构文档 3.4 节）
    pub fn build_review_prompt(
        decomposition: &str,
        user_request: &str,
        available_tools: &[String],
    ) -> String;

    /// 根据 app id 和当前阶段返回禁止清单
    pub fn ban_list(app_id: &str, phase: &str) -> Vec<&'static str>;
}
```

#### 执行 Prompt 模板

```
## 当前任务（只做这个）
{milestone.prompt_hint}

## 已知信息
{context: key: value 逐行}

## 禁止
- {app 特定禁止清单}
- 不要一次完成多个步骤
- 不要自己编造不存在的数据

## 输出末尾附加当前步骤状态：
[OK] / [MORE] 还需要: {具体内容} / [BLOCKED] 原因: {具体原因}
```

#### 拆解 Prompt 模板

按架构文档 3.3 节实现，核心要素：
- 当前应用 + 可用工具 + 已知上下文
- 拆解规则（writing-plans 方法论适配版）
- 禁止清单（笼统步骤、无产出物、TBD placeholder）
- 好/差示例
- 输出格式：每行 `N. {具体动词+具体对象+明确产出}`

#### 审阅 Prompt 模板

按架构文档 3.4 节实现，核心要素：
- 检查项：具体性、产出物、连贯性、覆盖度
- 输出格式：JSON `{ok, issues[], overall}`

#### 禁止清单映射

```rust
// 通用禁止（所有 app + 所有阶段）:
//   "不要一次完成多个步骤"
//   "完成当前任务后必须附加自评信号"

// 按 app 类型:
//   文档生成 + 需求阶段 → "不要生成完整文档", "不要跳过询问直接假设"
//   数据分析 + 探索阶段 → "不要跳过数据验证", "不要忽略异常值"
//   计划敲定 + 方案阶段 → "不要只给一个方案，给出选项让用户选"
//   计划敲定 + 评估阶段 → "不要替用户做决定"

// 按阶段:
//   "需求"/"确认"/"收集" → "只问不写，不要提前生成内容"
//   "生成"/"草稿"/"撰写" → "输出完整内容，末尾问用户'需要调整哪里'"
//   "定稿"/"保存"/"提交" → "执行保存操作，不要重新生成内容"
```

#### 验证

```rust
#[test]
fn test_execution_prompt_contains_scope_limit() { /* 检查 prompt 含 "只做这个" */ }
#[test]
fn test_execution_prompt_contains_ban_list() { /* 检查 prompt 含禁止清单 */ }
#[test]
fn test_execution_prompt_contains_context() { /* 检查已知信息已注入 */ }
#[test]
fn test_decomposition_prompt_structure() { /* 检查拆解 prompt 包含所有必需段落 */ }
#[test]
fn test_ban_list_by_app_and_phase() { /* 检查不同 app/phase 的禁止项不同 */ }
```

---

### 3.3 LLMReviewer

**文件：** `crates/tui/src/platform/reviewer.rs`

#### 接口

```rust
pub struct LLMReviewer;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewResult {
    pub ok: bool,
    pub issues: Vec<Issue>,
    pub overall: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub step: Option<u32>,
    pub problem: String,
    pub suggestion: String,
}

impl LLMReviewer {
    /// 审阅拆解结果。内部调用 harness.chat() + 解析 JSON。
    pub async fn review(
        harness: &dyn AgentHarness,
        decomposition: &str,
        user_request: &str,
        available_tools: &[String],
    ) -> Result<ReviewResult>;

    /// 审阅结果 → 自然语言反馈（注入重拆 prompt 用）
    pub fn format_feedback(result: &ReviewResult) -> String;
}
```

#### 解析策略（三层降级）

```
1. serde_json::from_str::<ReviewResult>(text)
   └─ 成功 → 返回

2. fallback_parse(text):
   ├─ ok: 搜索 "ok": false / "ok":true
   ├─ issues: 正则提取 {"step": N, "problem": "...", "suggestion": "..."}
   ├─ overall: 搜索 "overall": "..."
   └─ 返回 ReviewResult

3. 完全失败 → ReviewResult { ok: true, issues: [], overall: "审阅解析失败，默认放行" }
   └─ 不阻塞，人工确认 gate 是安全网
```

#### 反馈格式化

```rust
pub fn format_feedback(result: &ReviewResult) -> String {
    let mut feedback = String::new();
    feedback.push_str("审阅意见：\n");
    for issue in &result.issues {
        if let Some(step) = issue.step {
            feedback.push_str(&format!("- 步骤 {}: {}, 建议: {}\n", step, issue.problem, issue.suggestion));
        } else {
            feedback.push_str(&format!("- {}, 建议: {}\n", issue.problem, issue.suggestion));
        }
    }
    feedback.push_str(&format!("\n总体评价: {}\n请根据以上反馈修改拆解。", result.overall));
    feedback
}
```

#### 验证

```rust
#[tokio::test]
async fn test_review_parses_valid_json() { /* mock 返回合法 JSON */ }
#[tokio::test]
async fn test_review_fallback_malformed_json() { /* mock 返回残缺 JSON */ }
#[tokio::test]
async fn test_review_fallback_ungrammatical() { /* mock 返回纯文本 */ }
#[test]
fn test_format_feedback() { /* 检查反馈包含步骤号和问题描述 */ }
```

---

### 3.4 ResponseChecker

**文件：** `crates/tui/src/platform/response_checker.rs`

#### 接口

```rust
pub struct ResponseChecker;

#[derive(Debug, Clone, PartialEq)]
pub enum CompletionSignal { Done, More { reason: String }, Blocked { reason: String } }

#[derive(Debug, Clone, PartialEq)]
pub enum NextAction { Advance, WaitForUser, Continue { reason: String }, Block { reason: String } }

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub out_of_scope: bool,
    pub safe_content: Option<String>,
    pub signal: Option<CompletionSignal>,
    pub next_action: NextAction,
}

impl ResponseChecker {
    pub fn check(
        response: &str,
        current_milestone: &Milestone,
        app_config: &AppConfig,
    ) -> CheckResult;
}
```

#### 信号解析（宽松正则）

只看回复**最后 500 字符**。匹配优先级：`[BLOCKED]` > `[MORE]` > `[OK]`。

匹配模式（按优先级）：

```
[BLOCKED]: "\[BLOCKED\]" | "\[阻塞\]" | "卡住了[：:]" | "无法继续[：:]"
[MORE]:     "\[MORE\]"   | "\[继续\]" | "还需要[：:]" | "还没完[：:]"
[OK]:       "\[OK\]"     | "\[完成\]" | "\[✓\]"     | 行首"完成。"
```

兜底：无匹配 → `None`

#### 越界检测（纯机械规则）

```
规则 1: 「该问的阶段却写了大量内容」
  触发: label 含 "需求"/"确认"/"收集" + hint 含 "问"/"确认"
        + 回复 > 300 字 + 无问号
  处理: 截断到前 150 字符

规则 2: 「生成了完整文档但不在生成阶段」
  触发: 回复有 3+ 自然段 (\n\n) 分隔
        + label 不含 "生成"/"草稿"/"撰写"
  处理: 截取前 2 段

规则 3: 安全 → 放行 (false, None)
```

#### 路由决策矩阵

```
signal       | condition               | next_action
-------------|--------------------------|---------------
[OK]         | milestone.id in confirm_at | WaitForUser
[OK]         | granularity == "fine"      | WaitForUser
[OK]         | granularity == "medium"    | Advance
[OK]         | granularity == "coarse"    | Advance (confirm_at 已拦截关键节点)
[MORE]       | (any)                      | Continue { reason }
[BLOCKED]    | (any)                      | Block { reason }
None         | granularity == "fine"      | WaitForUser (安全)
None         | granularity != "fine"      | Advance (乐观)
out_of_scope | (any)                      | Continue { "只做X，不要Y" }
```

#### 连续越界处理

在 `PlatformEngine` 层面追踪 `consecutive_out_of_scope: u32`：
- 连续 1 次：截断 + 下轮 prompt 加强禁止
- 连续 2 次：暂停，展示给用户

#### 验证

```rust
#[test]
fn test_parse_signal_ok()               // "[OK]" → Done
#[test]
fn test_parse_signal_more()             // "[MORE] 还需要: ..." → More
#[test]
fn test_parse_signal_blocked_chinese()  // "[阻塞] 原因: ..." → Blocked
#[test]
fn test_parse_signal_missing()          // 无信号 → None
#[test]
fn test_out_of_scope_asking_phase()     // 需求阶段生成内容 → 越界
#[test]
fn test_out_of_scope_safe()             // 生成阶段正常输出 → 放行
#[test]
fn test_out_of_scope_multi_paragraph()  // 3+ 段不在生成阶段 → 越界
#[test]
fn test_decide_wait_on_confirm_at()     // confirm_at 命中 → WaitForUser
#[test]
fn test_decide_advance_on_medium()      // medium + OK → Advance
#[test]
fn test_decide_block_on_blocked()       // BLOCKED → Block
#[test]
fn test_decide_continue_on_more()       // MORE → Continue
#[test]
fn test_decide_default_fine()           // fine + no signal → WaitForUser
```

---

## 四、模块间调用关系

```
PlatformEngine::decompose_and_execute()
│
├── StepBuilder::build_decomposition()  ─┐
├── harness.chat()                       │ 拆解 + 审阅阶段
├── [可解析性检查]                         │
├── LLMReviewer::review() ───────────────┘
│   └── 内部调用:
│       ├── StepBuilder::build_review_prompt()
│       └── harness.chat()
│
├── [用户确认 gate]
│
└── 逐步执行循环:
    ├── StepBuilder::build()  → 构造小范围 prompt
    ├── harness.chat()        → LLM 执行
    └── ResponseChecker::check() → 信号解析 + 越界检测
        └── 返回 NextAction → 推进/等待/阻断/继续
```

### 编译依赖图

```
response_checker.rs  (无依赖 LLM)
       ↑
step_builder.rs     (无依赖 LLM)
       ↑
reviewer.rs         → 依赖 AgentHarness trait (调 LLM)
       ↑
deepseek_harness.rs → 依赖 LlmClient trait, AgentHarness trait
       ↑
engine.rs           → 依赖以上全部 + ConversationState + AppRegistry
       ↑
tui/ui.rs           → 依赖 PlatformEngine
```

---

## 五、集成点

### 5.1 与现有代码的连接

| 现有代码 | 变更 |
|---------|------|
| `pinvou-platform/src/engine.rs` | 添加 `decompose_and_execute()` 方法，调用 StepBuilder + LLMReviewer + ResponseChecker |
| `pinvou-platform/src/workflow.rs` | 无需变更，ConversationState 已足够 |
| `pinvou-platform/src/app.rs` | 无需变更，AppConfig 已包含 granularity/confirm_at/tools |
| `pinvou-platform/src/harness.rs` | 无需变更，AgentHarness trait 已完整 |
| `pinvou-platform/src/tui/ui.rs` | 替换 `simulate_engine_response()` 为真实 `engine.process_message()` 调用 |
| `pinvou-platform/src/tui/app.rs` | 添加 `current_model` 连接 DeepSeekHarness，添加 `consecutive_out_of_scope` 追踪 |
| `pinvou-platform/src/lib.rs` | 添加 4 个新模块声明 `pub mod deepseek_harness; pub mod step_builder; pub mod reviewer; pub mod response_checker;` |
| `pinvou-platform/Cargo.toml` | 无需新增依赖（所需 crate 均已在 deepseek-tui path 依赖中） |

### 5.2 与 TUI 的连接

```
handle_input_key (ui.rs):
  Enter → platform_app.add_user_message(input)
       → engine.process_message(input)  // 替代 simulate_engine_response
       → 流式更新 platform_app.streaming_content
       → ResponseChecker.check() 决定里程碑状态
       → sidebar 更新图标
```

---

## 六、错误处理矩阵

| 场景 | 处理 |
|------|------|
| LLM 调用超时 | `LlmClient` 已有重试逻辑（`RetryConfig`），超时 3 次后返回 Error |
| LLM 返回非 JSON | LLMReviewer 降级正则解析，再失败默认 ok=true 放行 |
| LLM 未输出自评信号 | ResponseChecker 返回 None → decide() 按 granularity 降级 |
| 连续 2 次越界 | Engine 暂停，TUI 展示警告 |
| checkpoint 读取失败 | 返回 None，用户手动选择应用重新开始 |
| embedding 服务不可用（路由） | 降级到 final_score 排序 |
| 空拆解结果 | 可解析性检查拒绝，退回 LLM 重拆 |
| 可解析性检查连续 2 次失败 | 降级到 AppConfig.milestones fallback |

---

## 七、测试策略

### 单元测试（不依赖外部服务）

- StepBuilder: prompt 模板正确性（含禁止清单、上下文注入）
- ResponseChecker: 信号解析（所有变体 + 边界）+ 越界检测 + 路由决策
- LLMReviewer: JSON 解析 / 降级解析 / 反馈格式化
- DeepSeekHarness: 注入 `MockLlmClient` 验证请求构建和响应映射

### 集成测试（需要本地服务）

- DeepSeekHarness + Ollama: 真实一次 chat 调用
- Engine + Mock LLM: 完整 decompose_and_execute 流程
- 所有集成测试标记 `#[ignore]`，通过 feature flag 或 `--ignored` 运行

### 验收标准

| 测试场景 | 验收条件 |
|---------|---------|
| 文档生成全流程 | 6 轮对话不越界，每步停住等用户确认 |
| 数据分析全流程 | CSV 探索→分析→可视化自动推进（medium granularity） |
| 简单问答 | 侧边栏灰色，直接回答，不触发拆解流程 |
| LLM 格式漂移 | 无信号 + 非 JSON → 不卡死，降级推进 |
| 连续越界 2 次 | 自动暂停，展示原因给用户 |

---

## 八、实现顺序

```
1. DeepSeekHarness    (1-2 天) — 可独立实现和测试，不依赖其他新模块
2. StepBuilder        (1 天)   — 纯函数，可并行开发
3. ResponseChecker    (1-2 天) — 纯函数，可并行开发
4. LLMReviewer        (1 天)   — 依赖 DeepSeekHarness + StepBuilder
5. Engine 集成        (2 天)   — 串联四个模块，替换 simulate_engine_response
6. TUI 对接 + 调优    (2-3 天) — 流式渲染 + 三个应用 prompt 调优
```

**总计：约 1.5-2 周**

---

## 九、不做的事项（明确排除）

- ❌ 不做 Sub-agent 并行分发（Phase 5）
- ❌ 不做 checkpoint 端到端恢复 UX（Phase 4。DeepSeekHarness 实现 `save_checkpoint`/`load_checkpoint` 基础 JSON 持久化，但从 checkpoint 恢复会话的完整流程留到 Phase 4）
- ❌ 不做 MCP 工具集成（Phase 5）
- ❌ 不做多模型热切换（Phase 4）
- ❌ 不做 Web UI（架构文档明确纯 TUI）
- ❌ 不修改 DeepSeek-TUI 原有 crate 逻辑（`mod platform;` 和 `[[bin]]` 是唯一入口）

---

> 本文档与 `设计架构文档-pinvou3.md` 保持一致。所有设计决策以前者为准。
