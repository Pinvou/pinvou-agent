# DeepSeek-TUI 架构详解（中文版）

> 给 pinvou3 后续设计当作参考手册用。
> 基于 v0.8.30 代码探索，源码位置：`DeepSeek-TUI/crates/tui/src/`。
> 最后更新：2026-05-12

---

## 0. 阅读指引

这份文档分**九大主题**，每个主题独立成章，可以按需跳读。

| 章节 | 主题 | 你能在这章学到什么 |
|---|---|---|
| 1 | **整体分层** | DeepSeek-TUI 由哪些 crate 组成，进程内怎么协作 |
| 2 | **三大基石**：Provider / Mode / Personality | 启动一个会话前要选哪三个东西 |
| 3 | **Engine 核心**：EngineHandle / Turn Loop | 一句用户输入到底经历什么 |
| 4 | **Tools 系统** | LLM 怎么操作世界，39 个内置工具清单 |
| 5 | **Prompt 拼装** | system prompt 怎么来的，4 层 + 7 块结构 |
| 6 | **高级编排**：Subagent / Skills / Plan / Todo / Tasks | 多步任务、并行子代理、长任务 |
| 7 | **上下文管理**：Compaction / Cycle | 长对话怎么不爆 context |
| 8 | **周边扩展**：Commands / MCP / Hooks / Sessions | 用户怎么扩展、怎么集成外部世界 |
| 9 | **配置与启动 + 给 pinvou3 的启示** | 配置文件层级、启动参数、复用清单 |

所有源码引用都给了**文件路径:行号**，可以直接打开看。

---

## 1. 整体分层

### 1.1 Workspace 结构

DeepSeek-TUI 是个 Cargo workspace，14 个 crate（`DeepSeek-TUI/Cargo.toml`）：

```
crates/
├─ tui/            ← 主体，~80% 代码在这（含 engine、tools、prompts、commands）
├─ tui-core/       ← TUI 渲染核心（ratatui 相关）
├─ cli/            ← `deepseek` 二进制（CLI wrapper，delegate 到 tui）
├─ agent/          ← agent 协议相关
├─ app-server/     ← `deepseek-app-server` 二进制（web/IDE 后端）
├─ core/           ← 共享工具类
├─ config/         ← 配置加载
├─ execpolicy/     ← shell 安全策略（safe/sandboxable/restricted）
├─ hooks/          ← lifecycle hook 协议
├─ mcp/            ← MCP 协议实现
├─ protocol/       ← 内部消息协议
├─ secrets/        ← API key 存储
├─ state/          ← 持久化状态
└─ tools/          ← 工具 trait + 通用实现
```

两个可执行入口：
- `deepseek`（`crates/cli`）— CLI wrapper，把多数子命令 delegate 到 tui
- `deepseek-tui`（`crates/tui`）— 实际 TUI 二进制，启动后用户主要交互的入口

### 1.2 进程内运行时分层

一个跑起来的 `deepseek-tui` 进程内部：

```
┌─────────────────────────────────────────────────────────┐
│  TUI 层（ratatui）                                       │
│  - 终端渲染、键盘事件、modal 弹窗、侧边栏（plan/todo/agents）│
└─────────────────────────┬───────────────────────────────┘
                          │ 4 条 tokio MPSC channel
                          ↓
┌─────────────────────────────────────────────────────────┐
│  EngineHandle（core/engine.rs:195）                      │
│  - tx_op:        TUI→Engine 发指令（SendMessage 等）      │
│  - rx_event:     Engine→TUI 推事件（MessageDelta 等）     │
│  - tx_approval:  TUI→Engine 推审批决策                    │
│  - tx_user_input:TUI→Engine 推 request_user_input 回应   │
│  - tx_steer:     TUI→Engine 中途追加用户内容              │
└─────────────────────────┬───────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────┐
│  Engine 后台 task（spawn_supervised + Engine::run）       │
│  ├─ turn_loop.rs       一个 turn 的生命周期               │
│  ├─ streaming.rs       SSE 解析 + delta 累积              │
│  ├─ dispatch.rs        Op 分发                            │
│  ├─ tool_execution.rs  工具执行循环                       │
│  ├─ tool_catalog.rs    工具白名单 + 延迟加载              │
│  ├─ approval.rs        审批阻塞协议                       │
│  ├─ capacity_flow.rs   context window 智能管理            │
│  └─ loop_guard.rs      防死循环                           │
└──────────┬──────────────────────┬──────────────────────┬─┘
           ↓                       ↓                       ↓
   LlmClient（client.rs）   ToolRegistry（tools/）   MCP Pool（mcp.rs）
       ↓
   HTTP/SSE 到 vLLM / DeepSeek / OpenAI / ...
```

**核心解耦点**：TUI 跟 Engine 完全通过 channel 通信。Engine 是 headless 的，理论上可以接任何前端（pinvou3 当初想做的就是接 web）。

### 1.3 跟 pinvou3 的对应关系

pinvou-platform 现在通过 `engine_factory` 拿到 DeepSeek-TUI 的 `LlmClient` + `ToolRegistry`，**绕过了 Engine**，自己写了 1000+ 行 tool loop 在 `deepseek_harness.rs`。Engine 路径（`engine_harness.rs`）做了一半，卡在 `request_user_input` 协议不兼容（详见 `engine-refactor-status.md`）。

读完本文档你会发现：**Engine 上面那 7000 行代码，pinvou3 80% 都没用上**。

---

## 2. 三大基石：Provider / Mode / Personality

启动一个会话前，本质上你在选三个正交维度：

```
       Provider × Mode × Personality
       (跟谁说话)  (干啥)   (怎么说)
```

### 2.1 Provider（跟哪个 LLM 后端通话）

`ApiProvider` enum（`config.rs:64-145`）支持 10 种：

| Provider | 用途 | 默认 base_url |
|---|---|---|
| `Deepseek` | DeepSeek 官方 | https://api.deepseek.com/beta |
| `DeepseekCN` | DeepSeek 国内（实际同上） | 同上 |
| `NvidiaNim` | NVIDIA 托管 DeepSeek | nim 端点 |
| `Openai` | OpenAI 兼容端点 | api.openai.com |
| `Openrouter` | OpenRouter 聚合 | openrouter.ai |
| `Novita` / `Fireworks` / `Sglang` | 各家托管 | 各自端点 |
| `Vllm` | **自部署 vLLM（这是 pinvou3 用的）** | 用户提供 |
| `Ollama` | 本地 Ollama | localhost:11434 |

每个 provider 在 `[providers.<name>]` 配置存独立凭证，可以快速切换。`/provider <name>` 命令热切换不用重启。

**provider 选择决定**：
- HTTP 端点 + 鉴权方式
- 模型字符串如何 normalize（vllm provider 在 v0.8.30 后保留自定义 model 字符串，不会强转）
- `reasoning_effort=off` 翻译成什么参数（vllm provider 翻译成 `chat_template_kwargs.enable_thinking=false`，DeepSeek provider 翻译成 `thinking={type: disabled}`）

### 2.2 Mode（这次会话的纪律）

三种 mode，启动时 CLI `--mode` 或会话中 `/mode` 切换。**当前实现 mode 切换主要影响下一次 turn 的工具白名单和 approval policy**。

| Mode | Prompt 文件 | 工具权限 | Approval 默认 | 用途 |
|---|---|---|---|---|
| **Agent** | `prompts/modes/agent.md` | 读工具静默、写/patch/shell/spawn 要审批 | `Suggest` | 标准工作流 |
| **Plan** | `prompts/modes/plan.md` | 只读，写/shell 完全阻止 | `Never` | 先规划后执行 |
| **Yolo** | `prompts/modes/yolo.md` | 所有工具预批准 | `Auto` | 全自动，可信环境 |

**关键差异**（看 `prompts/modes/plan.md` 全文 10 行就懂）：

> "Use `update_plan` to lay out high-level strategy and `checklist_write` for granular, verifiable steps. All writes and patches are blocked — you can read the world but you can't change it."

Plan mode 就是**强制 LLM 先写 plan**。这是 DeepSeek-TUI 解决"模型不自觉规划"问题的方式——不靠代码硬卡，靠工具白名单 + prompt 引导。

### 2.3 Personality（语气）

两种 personality（`prompts/personalities/`）：

- **Calm**（默认）：冷静、保留、避免感叹号和情绪词，"应用了补丁" 而不是 "完美运作！"
- **Playful**（接线但未通过 CLI 暴露）：温暖、有玩笑、用 em dash 和括号

跟 mode 是**叠加关系**（不是替换），personality 修饰 mode 的语气。

### 2.4 Approval Policy（审批策略）

三种 policy（`prompts/approvals/`）：

| Policy | 行为 | 默认 mode |
|---|---|---|
| `Auto` | 工具直接执行 | Yolo |
| `Suggest` | 写工具调用前请求批准 | Agent（默认） |
| `Never` | 阻止所有写工具 | Plan |

Yolo 强制 Auto、Plan 强制 Never、Agent 默认 Suggest 但可 `--approval-policy` 覆盖（`prompts.rs:373-383`）。

---

## 3. Engine 核心：一句用户输入到底经历什么

### 3.1 EngineHandle（`core/engine.rs:195-296`）

EngineHandle 是 TUI 跟 Engine 之间的唯一接口，4 条独立的 tokio MPSC channel：

```rust
pub struct EngineHandle {
    pub tx_op:          mpsc::Sender<Op>,          // 32 buffer
    pub rx_event:       Arc<Mutex<mpsc::Receiver<Event>>>,  // 256
    pub tx_approval:    mpsc::Sender<ApprovalDecision>,    // 64
    pub tx_user_input:  mpsc::Sender<UserInputDecision>,   // 32
    pub tx_steer:       mpsc::Sender<SteerInput>,          // 64
}
```

**spawn 方法**（`engine.rs:1930`）：

```rust
let (engine, handle) = Engine::new(config, api_config);
spawn_supervised(async move { engine.run().await });
// 返回 handle 给前端
```

### 3.2 主要的 Op 和 Event

**Op 枚举**（TUI 发给 Engine 的指令）：
- `SendMessage { content, attachments }` — 发用户消息
- `CancelRequest` — 取消当前 turn
- `SpawnSubAgent { ... }` — 启动子代理
- `SyncSession { messages, system_prompt }` — 同步会话状态（pinvou-platform 的 engine_harness 用这个）
- `CompactNow` — 强制 compact

**Event 枚举**（Engine 推给 TUI 的事件）：
- `MessageDelta { text }` — LLM 文本流式输出
- `ThinkingDelta { text }` — V4 模型 reasoning 段
- `ThinkingStarted` / `ThinkingComplete`
- `ToolCallStarted { id, name, input }` — 工具开始执行
- `ToolCallProgress { id, status }` — 进度反馈
- `ToolCallCompleted { id, result }`
- `ApprovalRequired { id, tool, args, summary }` — 等审批
- `UserInputRequired { id, request }` — `request_user_input` 触发，**engine 阻塞等 `tx_user_input.send()`**
- `TurnComplete { usage, status, error }` — 一个 turn 完成
- `CompactionStarted` / `CompactionCompleted`
- `CycleAdvanced { briefing }` — 进入新 cycle
- `CapacityDecision { action, risk }`

### 3.3 Turn Loop 的完整生命周期（`engine/turn_loop.rs`）

从 `Op::SendMessage` 到 `Event::TurnComplete` 的 10 个阶段：

```
1. TurnContext 初始化（turn_loop.rs:11-43）
   - 检查 cancel_token
   - 处理 rx_steer 收到的额外用户内容（一个 turn 可被多次 steer）
   ↓
2. Pre-request 检查（turn_loop.rs:72-170）
   - 是否达到 max_steps？
   - 是否需要自动 compaction？
   - 容量预检查（pre_request_checkpoint）
   ↓
3. 上下文溢出恢复（最多 5 次，turn_loop.rs:172-200）
   - 若输入 token 超预算，尝试 compaction 或回退
   - 失败返回 TurnOutcomeStatus::Failed
   ↓
4. 流式 LLM 调用（streaming.rs）
   - client.create_message_stream() → SSE
   - 流式重试最多 3 次（仅在「零内容接收」时，避免重复计费）
   - delta 累积到 ContentBlockKind::{Text, Thinking, ToolUse}
   ↓
5. 工具调用解析（tool_execution.rs）
   - 从 ToolUse 块解析 (name, input, id)
   - 检查内置工具（request_user_input / agent_spawn / ...）
   ↓
6. Approval / UserInput 阻塞（approval.rs）
   - 若需审批：发 Event::ApprovalRequired，阻塞 await_tool_approval()
   - 若 request_user_input：发 Event::UserInputRequired，阻塞 await_user_input()
   ↓
7. 工具执行
   - tool_registry.execute() / MCP / 内置特殊处理
   - 大输出（>4096 token）走 large_output_router 合成
   - 工具结果回流到 messages
   ↓
8. Loop Guard 检查（loop_guard.rs）
   - 相同 (tool, args) ≥3 次 → block
   - 连续失败 ≥8 次 → halt turn
   ↓
9. 决定下一轮：还有工具调用？回 4。无 → 走 10
   ↓
10. Cycle 边界 + Post-turn snapshot
    - 输入 token ≥ cycle 阈值且无 in-flight → 触发 cycle archive
    - 保存 git workdir 快照
    - 发 Event::TurnComplete
```

**关键概念**：一个 turn 可能跑**多次 LLM 调用**（每次工具调用后回流结果再喊 LLM），不是一次 LLM 调用 = 一个 turn。

### 3.4 Tool Execution 的并行 + 顺序（`engine/tool_execution.rs`）

一个 turn 内多个工具调用默认**串行**（持 `tool_exec_lock` 写锁）。但 LLM 可以输出特殊的 `multi_tool_use.parallel` 工具调用，让 engine 起 `FuturesUnordered` 并行执行。

**并行执行的安全检查**（`tool_execution.rs:172-207`）：
- 只允许 `is_read_only() == true` 的工具
- 只允许 `approval_requirement() == Auto` 的工具
- 只允许 `supports_parallel() == true` 的工具
- MCP 工具白名单：`list_mcp_resources`、`read_mcp_resource`、`mcp_get_prompt`

**交互式工具**（编辑器、shell stdin）会主动释放 `tool_exec_lock`，让 TUI 渲染交互界面。

### 3.5 Approval 协议（`engine/approval.rs`）

```rust
// Engine 在工具执行前
if tool.approval_requirement() != Auto {
    await_tool_approval(tool_id).await?;  // 阻塞等 tx_approval
}

// 等三种东西之一
tokio::select! {
    _ = cancel_token.cancelled() => Err("cancelled"),
    decision = rx_approval.recv() => match decision {
        Approved          => 工具继续执行,
        Denied            => 返回拒绝错误,
        RetryWithPolicy(policy) => 提升沙箱策略后重试,
    }
}
```

### 3.6 Capacity Flow + Cycle（`engine/capacity_flow.rs` + `cycle_manager.rs`）

**两层 context window 管理**：

**Layer 1：Compaction**（轻度，重写消息）
- 阈值：500K-800K token（`compaction.rs:43-65`）
- 保留：最近 4 条消息完整 + working-set 路径
- 丢弃：老消息文本被截断到 800 字符（大型上下文 2000）
- 摘要模型：默认 `deepseek-v4-flash`

**Layer 2：Cycle**（重度，重新开始）
- 阈值：768K token（`cycle_manager.rs:70`）默认 75% of 1M window
- 触发条件：超阈值 + 干净 turn 边界（无 in-flight tool/stream/approval）
- 流程：
  1. 旧 cycle 全量消息 → JSONL 存档（可 `/recall` 搜索）
  2. 生成 briefing（最多 ~3000 token，含决策、约束、悬而未决的问题）
  3. 抓取 StructuredState：mode、workspace、todos、plan、running subagents、用户未发送消息
  4. 新 cycle 从 briefing + StructuredState 开始
- 跨 cycle 的 ID 保留：todo、plan items 用稳定 ID，引用不会断

**GuardrailAction**（`capacity_flow.rs`）三种级别：
- `NoIntervention` — 继续
- `TargetedContextRefresh` — 删除最旧消息留最新 K 条
- `VerifyWithToolReplay` — 重新执行最后 N 个工具校验一致性
- `VerifyAndReplan` — 强制 LLM 重新规划

### 3.7 Loop Guard（`engine/loop_guard.rs`）

防死循环，纯数据结构：

```rust
struct LoopGuard {
    call_counts:    HashMap<(tool_name, args_hash), u32>,
    failure_counts: HashMap<tool_name, u32>,
}
```

阈值：
- 相同 (tool, args) ≥ 3 次 → `AttemptDecision::Block`
- 连续失败 ≥ 3 次 → `Warn`
- 连续失败 ≥ 8 次 → `Halt`

任何成功重置失败计数。

### 3.8 Streaming + 假工具包装过滤（`engine/streaming.rs`）

**流状态机**根据 `ContentBlockKind`：
- `Text` → 累积到 message delta，推 `Event::MessageDelta`
- `Thinking` → 累积到独立 thinking 块，推 `Event::ThinkingDelta`
- `ToolUse` → JSON 逐字符到 `input_buffer`，块完成时一次性解析

**假工具包装过滤**（`streaming.rs:89-152`）— 这是个有意思的细节：小模型/某些后端会在文本里塞 `[TOOL_CALL]` `<deepseek:tool_call>` 这类"假工具调用标记"。Engine 用状态机识别并剥除，让用户看到干净文本，同时发 `FAKE_WRAPPER_NOTICE` 解释为什么文本"缩水了"。

**超时**：默认 SSE idle timeout 300 秒（env `DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS` 可覆盖），墙钟硬上限 1800 秒。

### 3.9 Tool Catalog 延迟加载（`engine/tool_catalog.rs`）

为了节省 token 和保持前缀缓存稳定，Engine 不是把所有工具一次性塞给 LLM，而是**延迟加载**：

- **始终加载**：read_file, list_dir, grep_files, update_plan, request_user_input, recall_archive
- **Agent mode 额外始终加载**：shell 工具（exec_shell 等）
- **延迟加载**：其他工具，需要 LLM 调 ToolSearch（regex 或 BM25）激活
- **Yolo mode**：所有工具始终加载

**MCP 工具白名单**（始终加载）：`list_mcp_resources`、`read_mcp_resource`、`mcp_get_prompt`，其余 MCP 工具延迟。

**前缀缓存稳定性**：工具按名字排序，不延迟的工具固定排前，激活的延迟工具追加到末尾（不在中间插入，避免移位破坏缓存）。

**高级工具动态注入**（`tool_catalog.rs:122-182`）：engine 额外注入这些内置（不在 ToolRegistry 里）：
- `code_execution_20250825` — Python 沙箱执行
- `tool_search_tool_regex_20251119` — 正则搜索延迟工具
- `tool_search_tool_bm25_20251119` — 自然语言搜索延迟工具

---

## 4. Tools 系统：让 LLM 操作世界

### 4.1 ToolSpec trait（`tools/spec.rs:561-614`）

每个工具实现：

```rust
pub trait ToolSpec: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;
    fn capabilities(&self) -> Vec<ToolCapability>;
    fn approval_requirement(&self) -> ApprovalRequirement;
    fn is_sandboxable(&self) -> bool;
    fn is_read_only(&self) -> bool;
    fn supports_parallel(&self) -> bool;
    fn defer_loading(&self) -> bool;
    async fn execute(&self, input: Value, context: &ToolContext)
        -> Result<ToolResult, ToolError>;
}
```

**ToolContext** 给工具提供的服务（`tools/spec.rs:70-141`）：

| 字段 | 用途 |
|---|---|
| `workspace` | 工作目录路径 |
| `shell_manager` | 共享 shell 管理（后台任务、流式 IO） |
| `trust_mode` | 是否允许工作目录外的路径 |
| `auto_approve` | YOLO 模式标志 |
| `network_policy` | 域名过滤 |
| `runtime` | 持久化服务（TaskManager / Automation） |
| `notes_path` / `memory_path` | 笔记、用户记忆文件位置 |
| `lsp_manager` | LSP 诊断注入（编辑后） |
| `large_output_router` | 大输出路由 |
| `workshop_vars` | 上一个工具结果原始内容 |
| `sandbox_backend` | 远程沙箱后端 |

**关键枚举**：
- `ApprovalRequirement`：`Auto` / `Suggest` / `Required`
- `ToolCapability`：`ReadOnly` / `WritesFiles` / `ExecutesCode` / `Sandboxable` / `NetworkAccess` / `HighRisk`

### 4.2 ToolRegistry（`tools/registry.rs`）

存储所有工具的 `HashMap<String, Arc<dyn ToolSpec>>`。注册时自动：
- 触发 schema sanitization（DeepSeek strict mode 兼容）
- 失效化 API 缓存（确保前缀缓存稳定）

**builder 模式**（`registry.rs:393+`）链式构建：

```rust
ToolRegistry::builder()
    .with_file_tools()         // read_file, write_file, edit_file, list_dir
    .with_shell_tools()        // exec_shell 等
    .with_search_tools()       // grep_files, file_search
    .with_git_tools()
    .with_web_tools()          // web_search, fetch_url, finance, web_run
    .with_user_input_tool()    // request_user_input
    .with_patch_tools()        // apply_patch
    .with_mcp_tools(pool)      // MCP 适配
    .with_skill_tools()        // load_skill
    // ... 还有 20 多个 with_* 方法
    .build()
```

**BYO 完全支持**：任何实现 `ToolSpec` 的类型都可以 `.with_tool(Arc::new(YourTool))` 注册。Engine 接受 `Option<&ToolRegistry>`，传 None 时只用内置工具。

### 4.3 完整工具清单（39 个）

按功能分类：

#### 文件操作（4 个）
| 工具 | Approval | 特点 |
|---|---|---|
| `read_file` | Suggest | UTF-8 文本，PDF 自动提取 |
| `write_file` | Suggest | 覆盖或创建 |
| `edit_file` | Suggest | 行编辑 / unified diff，触发 LSP 诊断 |
| `list_dir` | Auto | 列目录 |

#### 代码搜索（2 个）
| 工具 | Approval |
|---|---|
| `grep_files` | Auto |
| `file_search` | Auto |

#### Shell / 命令（5 个）
| 工具 | Approval | 特点 |
|---|---|---|
| `exec_shell` | Required | 前台/后台，可沙箱 |
| `exec_shell_wait` | Auto | 等后台命令完成 |
| `exec_shell_interact` | Auto | 交互式 stdin |
| `exec_wait` / `exec_interact` | 同上 | 别名 |
| `note` | Auto | 追加到 `.deepseek/notes.md` |

#### Git（5 个，全 Auto + ReadOnly）
`git_status` / `git_diff` / `git_log` / `git_show` / `git_blame`

#### GitHub（4 个）
| 工具 | Approval |
|---|---|
| `github_issue_context` | Auto |
| `github_pr_context` | Auto |
| `github_comment` | Suggest |
| `github_close_issue` | Required |

#### Patch / Revert（2 个）
- `apply_patch` (Suggest) — 多文件 unified diff + 模糊匹配
- `revert_turn` (Required) — 撤销上一步编辑（快照侧库）

#### 计划 / 待办 / 任务（pinvou3 重点关注）

**update_plan**（`tools/plan.rs`，Auto）—— **会话内**的高层 plan：
- 状态机：`Pending` → `InProgress` → `Completed`
- 强制单一 InProgress（新标进行时旧的自动降级）
- 时间戳追踪：每个 step 的 `started_at` / `completed_at`
- 输出：`PlanSnapshot { explanation, items, completion_pct }`
- 渲染：TUI 侧边栏 Plan 面板

**todo_* / checklist_*（`tools/todo.rs`，Auto）—— **session 级**的细粒度 checklist：
- 内存存储（关闭会话丢失）
- 三个工具：`todo_add` / `todo_write`（全量覆盖）/ `todo_update`
- 跟 plan 区别：plan 是高层战略（3-10 步），todo 是具体可验证步骤（10-30 个）

**task_*（`tools/tasks.rs`，多数 Required）—— **持久化任务**：
- 磁盘存储（跨会话恢复）
- 7 个工具：`task_create` / `task_list` / `task_read` / `task_cancel` / `task_gate_run` / `task_shell_start` / `task_shell_wait`
- 用途：后台长任务、CI gate、PR attempt 记录
- 配套 `pr_attempt_*`（4 个）记录 PR 尝试历史

**关键区别**：

| 维度 | update_plan | todo_* | task_* |
|---|---|---|---|
| 抽象层次 | 高层战略（3-10 步） | 中层 checklist | 单步可执行任务 |
| 存储 | 会话内 | 会话内 | 磁盘持久化 |
| 跨会话 | ✗ | ✗ | ✓ |
| 时间追踪 | ✓ | ✗ | ✓ |
| Approval | Auto | Auto | Required（多数） |
| 渲染 | Plan 面板 | Todos 面板 | Tasks 面板 |

#### 用户交互（1 个，但最特殊）
**request_user_input**（`tools/user_input.rs`，Auto）：
- 接口：最多 3 个问题，每问 2-3 个选项
- **Engine 特殊处理**：LLM 调它后 engine 不继续执行 `tool.execute()`，而是发 `Event::UserInputRequired` + 阻塞 `await_user_input()` 等 `tx_user_input` 推回应
- 这是 pinvou3 engine refactor 卡住的核心点：pinvou3 protocol 把它当普通工具透传，engine 把它当内置阻塞工具

#### Web / 网络（4 个）
| 工具 | Approval |
|---|---|
| `web_search` | Auto |
| `fetch_url` | Suggest |
| `finance` | Auto |
| `web_run`（Playwright 浏览器自动化） | Required |

#### 自动化（7 个）
`automation_*`：`create` / `list` / `read` / `update` / `pause` / `resume` / `delete` / `run`
基于 RRULE 的定时任务。

#### 输出管理（3 个内部）
- `retrieve_tool_result` — 检索溢出的工具结果（summary/head/tail/query）
- `truncate` — 截断到 `~/.deepseek/tool_outputs`
- `large_output_router` — 大结果（>4k token）走 V4-Flash 合成

#### LLM 高级（3 个）
- `rlm`（Auto）— 递归 LLM 调用（处理超长输入）
- `review`（Suggest）— 代码审查（内嵌 LLM）
- `fim_edit`（Suggest）— Fill-in-the-Middle 编辑（通过 LLM）

#### 记忆 / 技能 / 通知（4 个）
- `remember`（Auto）— 追加到 `~/.deepseek/user_memory`
- `load_skill`（Auto）— 加载 SKILL.md
- `notify`（Auto）— 桌面通知
- `project_map`（Auto）— 项目结构

#### 数据 / 测试（2 个）
- `validate_data`（Auto）— JSON/YAML schema 校验
- `run_tests`（Required）— cargo test

#### 内部 / 代理（1 个）
- `agent_spawn`（Required）— 见 §6.1

### 4.4 容错机制

**arg_repair**（`tools/arg_repair.rs:1-62`）—— LLM 流式吐 JSON 时遇到边界问题（控制字符、未闭合括号、尾随逗号），五级修复阶梯：
1. 严格解析 → 成功返回
2. 去字符串内控制字符
3. 去尾随逗号
4. 平衡括号（最多 50 层）
5. 修剪多余闭包
6. 回退到 `{}`

**schema_sanitize**（`tools/schema_sanitize.rs`）—— DeepSeek strict mode 兼容性：
- 折叠 nullable union（`anyOf:[string,null]` → `{type:"string",nullable:true}`）
- 给裸 object 注入空 `properties`
- 修剪悬挂 `required`
- 单元素 `oneOf/allOf` 折叠

---

## 5. Prompt 拼装：system prompt 怎么来的

### 5.1 拼装的 4 层 mode prompt（编译时常量）

`prompts.rs:385-419` 的 `compose_prompt_with_approval()`：

```
base.md                              ← 217 行核心身份
  ↓
prompts/personalities/calm.md        ← 语气
  ↓
prompts/modes/agent.md               ← 模式纪律
  ↓
prompts/approvals/suggest.md         ← 审批策略
```

这 4 段是编译时常量，缓存友好度最高。

### 5.2 完整 7 块运行时拼装

`system_prompt_for_mode_with_context_skills_session_and_approval()`（`prompts.rs:520-671`）按顺序拼接：

```
0. locale 强化前言（非英文 locale，可选）
1. mode_prompt（4 层合成，§5.1）
2. project context（workspace 静态）
2.25. ## Environment 块（locale/version/platform/shell/pwd）
2.5a. instructions 文件（PathBuf 数组，每个 ≤100KB）
2.5b. user memory 块
3. skills 块（§6.3）
4. Context Management 提示（Agent/Yolo only）
5. compact.md 模板（教 /compact handoff 格式）
─── 缓存边界 ───
6. handoff 块（/compact 时重写）
7. locale 强化后言（可选，最接近用户消息）
```

**设计要点**：越静态的内容越靠前（缓存友好），越动态的越靠后（破坏缓存影响小）。

### 5.3 base.md（217 行）核心章节

1. **Language**（行 3-17）— 从用户最新消息推导语言。**项目上下文不是语言信号**（中文文件名不代表用户要中文回复）
2. **Runtime Identity**（行 18-24）— 不要试图启动 `deepseek` 二进制（已经在里面了）
3. **Preamble Rhythm**（行 25-33）— 开场白要有动作感，不要感情词
4. **Decomposition Philosophy**（行 35-55）— **核心方法论**：
   - PREVIEW：扫目录结构再行动
   - CHUNK + map-reduce：切分独立子任务并行
   - RECURSIVE：子问题再分解
5. **Verification Principle**（行 57-75）— 每次工具调用后验证，不信任内存
6. **Composition Pattern**（行 77-86）— 5+ 步任务：`update_plan` → `checklist_write` → 批量并行工具
7. **Sub-Agent Strategy**（行 88-105）— Flash 便宜，并行调查，max_concurrent=10
8. **Parallel-First Heuristic**（行 97-107）— 每次 tool call 前问：能否并行？
9. **RLM**（行 108-121）— CHUNK / BATCH / RECURSE 三种模式
10. **Context & Thinking Budget**（行 122-153）— 1M 窗口，按任务复杂度选 thinking 深度
11. **Toolbox**（行 155-167）— 工具快速参考
12. **Tool Selection Guide**（行 170-187）— apply_patch vs edit_file vs write_file 的选择

### 5.4 Mode prompt 全文（很短，直接抄）

**plan.md（10 行）**：
> Investigate first, act later. Use `update_plan` to lay out high-level strategy and `checklist_write` for granular, verifiable steps. All writes and patches are blocked.

**agent.md（31 行）**：
> Read-only tools run silently. Any write, patch, shell execution, sub-agent spawn, or CSV batch operation will ask for approval first.
> Before requesting approval for writes, lay out your work with `checklist_write` so the user can see what you intend to do.

**yolo.md（10 行）**：
> All tools pre-approved. Move fast, but verify after each action.

---

## 6. 高级编排

### 6.1 Subagent / agent_spawn（`tools/subagent/mod.rs`）

**用途**：让主 agent 把独立子任务分发给并行的子代理，每个用便宜的 Flash 模型跑（默认）。

**agent_spawn 工具 args**：
```json
{
  "agent_type": "general|explore|plan|review|implementer|verifier|custom",
  "system_prompt": "optional override",
  "initial_message": "the input to the sub-agent",
  "max_steps": 100,
  "fork_context": true,
  "workspace": "/path/to/workspace"
}
```

**子代理 = 独立 engine**：
- 新的 `LlmClient` + 独立 `ToolRegistry`（可按 agent_type 过滤工具子集）
- 独立 conversation history
- 通过 mailbox（`tools/subagent/mailbox.rs`）跟父 agent 异步通信

**生命周期超时**：
- Init: 30s
- 每步 LLM 调用: 120s
- 工具执行: 30s
- 结果轮询: 30s（默认）
- 终止后保留 3600s（用于查询结果）

**Resident File Leases**（`mod.rs:45-57`）—— 缓存优化：子代理获得文件租约，期间其他 agent 不读取这个文件，防止并发文件访问破坏缓存。

**子代理回报格式**（`prompts/subagent_output_format.md`）—— 强制结构化：

```markdown
### SUMMARY
One paragraph plain prose summary.

### EVIDENCE
- file_path:120-145
- grep match result

### CHANGES
- path/to/file.rs: edit description

### RISKS
- risk description

### BLOCKERS
- needed info, or "None."
```

**Whale 昵称**（`mod.rs` WHALE_NICKNAMES）：每个子代理按 spawn 索引循环分配一个鲸鱼绰号（蓝鲸、座头鲸…），TUI 侧边栏显示用。

### 6.2 Skills 系统（`skills/`）

**Skill ≠ Tool**：

| 维度 | Skill | Tool |
|---|---|---|
| 形式 | `SKILL.md`（frontmatter + Markdown） | Rust ToolSpec trait |
| 安装 | 拷文件到 `~/.deepseek/skills/<name>/SKILL.md` | Cargo 注册 |
| 发现 | 文件系统扫描（`SkillRegistry::discover`） | ToolRegistry build |
| 调用 | `load_skill` 工具加载到 context，LLM 自由执行 | LLM 发 tool_calls，engine 执行 |
| 权限 | 无（只是文本指导） | 受 approval policy 限制 |

**Skill 安装路径**（`skills/mod.rs:32-53`）—— DeepSeek-TUI 兼容多套生态：
1. `~/.deepseek/skills` — DeepSeek 原生
2. `~/.agents/skills` — agentskills.io 兼容
3. `~/.claude/skills` — Claude 全生态共享
4. Workspace：`.agents/skills`、`skills/`、`.opencode/skills`、`.cursor/skills`

**SKILL.md 格式**：
```markdown
---
name: skill-creator
description: Creates new skills from natural language descriptions
---

## Usage
[Markdown body 教 LLM 怎么使用]
```

**注入方式**：
- 启动时 `SkillRegistry::discover` 扫描全部 skill，把 description 注入 system prompt（每个 ≤512 字符，全块 ≤12000 字符）
- LLM 决定调 `load_skill <name>`，工具返回完整 SKILL.md body 注入 context
- 之后 LLM 自主按 SKILL 指导执行（不受 approval 限制）

**System Skills**（`skills/system.rs`）：binary 绑定 `skill-creator` SKILL，首次启动自动安装。

### 6.3 update_plan + checklist_write 是 DeepSeek-TUI 的"伪 milestone"

这两个工具加上 base.md 的 "Decomposition Philosophy" 章节，**功能上等价于 pinvou3 的 CombinedPlanner + Milestone**：

- **CombinedPlanner** = base.md 教 LLM 怎么 PREVIEW/CHUNK/RECURSIVE + plan mode 强制规划
- **Milestone** = update_plan 的 step + checklist_write 的 item
- **mark_done 状态机** = StepStatus 转移 + 时间戳

**关键差异**：DeepSeek-TUI 让 LLM **自主**决定何时调 update_plan、拆几步、何时 mark_done。pinvou3 用代码硬卡顺序（mode 7 枚举 + ContractValidator）。

DeepSeek-TUI 假设 LLM 强（Claude 级别），pinvou3 假设 LLM 弱（本地小模型）。**这个假设是否成立，是 pinvou3 编排层存废的核心问题**——具体见 `run-deepseek-tui.sh` 跑出来的对照实验。

---

## 7. 上下文管理：Compaction + Cycle

已经在 §3.6 讲过机制，这里只补充用户视角。

### 7.1 用户能感知到的事件

| 事件 | 触发 | 用户看到什么 |
|---|---|---|
| Compaction 自动启动 | 500K-800K token | TUI 顶部出现 `⚙ compacting...`，约 5-15 秒 |
| `/compact` 手动 | 用户输入 | 同上，但绕过 500K 地板 |
| Cycle 边界 | 768K + 干净边界 | 侧边栏出现新 cycle 编号，旧 cycle 进存档 |
| `/cycles` | 用户输入 | 列出所有 cycle 边界 + briefing 预览 |
| `/recall <query>` | 用户输入 | BM25 在 JSONL 存档中搜索 |

### 7.2 cycle_handoff.md（`prompts/cycle_handoff.md`）

新 cycle 启动时注入的 prompt 模板，教 LLM 怎么阅读 briefing + 继续工作。模板包含：
- 旧 cycle 的 briefing
- StructuredState（mode/workspace/todos/plan/running subagents）
- 用户未发送的消息（如果有）
- 继续指令

### 7.3 compact.md（`prompts/compact.md`）

教 LLM 怎么写 `/compact` 触发的 handoff 文件 `.deepseek/handoff.md`：
- 决策过程 / 测试中的假设
- 失败的方法 / 待解决的问题
- 不写：工具输出 bytes、文件内容、步骤式概述（这些在存档里可搜索）

---

## 8. 周边扩展：Commands / MCP / Hooks / Sessions

### 8.1 Slash Commands（37+ 内置）

**入口**：`commands/mod.rs:execute()` 把 `/xxx args` 分发到对应模块。

按用途归类：

**核心交互**：`/help` / `/clear` / `/exit` / `/anchor` / `/model` / `/models` / `/provider`

**会话管理**：`/save` / `/load` / `/sessions` / `/rename` / `/export` / `/tokens` / `/cost`

**上下文管理**：`/compact` / `/context` / `/cycles` / `/cycle <n>` / `/recall <query>`

**模式与配置**：`/mode {agent|plan|yolo}` / `/config` / `/theme` / `/verbose` / `/trust` / `/profile`

**工具与扩展**：
- `/mcp [init|add|enable|disable|reload]` — MCP 服务器
- `/network [list|allow|deny|default]` — 网络策略
- `/hooks [list|events]` — Hook 配置
- `/skills` / `/skill <name>` — Skills 管理
- `/lsp` — LSP 诊断

**任务与作业**：`/jobs` / `/task` / `/queue` / `/stash`

**调试与回滚**：`/system` / `/edit` / `/undo` / `/retry` / `/restore N`

**用户自定义命令**：放 `.md` 文件到 `~/.deepseek/commands/` 或 `<workspace>/.deepseek/commands/`，文件名（无扩展名）就是命令名，文件内容直接作为用户消息发送。优先级 workspace > user-global，可覆盖内置命令。

### 8.2 MCP 集成（`mcp.rs` + `mcp_server.rs`）

DeepSeek-TUI **既是 MCP client 又是 MCP server**。

**作为 client**：
- 连接外部 MCP server（stdio 或 HTTP）
- 自动 discover 远程工具（`tools/list` RPC）
- 配置文件：`~/.deepseek/mcp.json`
- 超时：connect 10s / execute 60s / read 120s（可 per-server 覆盖）

**作为 server**（`mcp_server.rs`）：
- stdio 暴露 DeepSeek 工具给外部
- 配置文件：`~/.deepseek/mcp_server.toml`
- 可控暴露工具集 `[server].expose_tools`
- 支持 `require_approval = true`

**配置示例**（`~/.deepseek/mcp.json`）：
```json
{
  "timeouts": { "connect_timeout": 10, "execute_timeout": 60 },
  "servers": {
    "my-stdio": {
      "command": "/path/to/server",
      "args": ["--flag"],
      "env": { "TOKEN": "..." },
      "enabled": true
    },
    "my-http": {
      "url": "http://localhost:3000",
      "enabled": true
    }
  }
}
```

### 8.3 Hooks（lifecycle 钩子，`hooks.rs`）

**支持的事件**：

| 事件 | 触发 |
|---|---|
| `SessionStart` / `SessionEnd` | TUI 启动 / 关闭 |
| `MessageSubmit` | 用户提交 turn（LLM 调用前） |
| `ToolCallBefore` / `ToolCallAfter` | 工具调用前后 |
| `ModeChange` | mode 切换 |
| `OnError` | 错误（传输/容量/工具） |
| `ShellEnv` | **特殊**：`exec_shell` 前运行，stdout 解析为 `KEY=VALUE` 注入环境 |

**配置示例**：
```toml
[[hooks.hooks]]
event = "tool_call_before"
command = "echo 'about to call $TOOL_NAME'"
timeout_secs = 10
background = false
continue_on_error = true
condition = { type = "tool_name", name = "exec_shell" }
```

**条件类型**：`Always` / `ToolName` / `ToolCategory` / `Mode` / `ExitCode` / `All` / `Any`

**ShellEnv hook 妙用**：临时凭证、per-skill PATH、短期 token。失败不中止 shell，KEY 名（不是值）写到 `~/.deepseek/audit.log`。

### 8.4 Sessions（`session_manager.rs`）

**SessionMetadata**：id / title / created_at / updated_at / message_count / total_tokens / model / workspace / mode / cost

**限额**：
- 最多 50 个会话
- 每会话最多 500 条消息（超出 prepend truncation 提示）

**命令**：`/save [path]` / `/load [path]` / `/sessions [show|prune <days>]`

会话 JSON 包含完整 messages history + artifacts + system prompt。

---

## 9. 配置与启动 + 给 pinvou3 的启示

### 9.1 配置加载顺序

1. `~/.deepseek/config.toml` — 用户全局
2. `<workspace>/.deepseek/config.toml` — 项目级（覆盖全局）
3. 环境变量 — `DEEPSEEK_API_KEY` / `DEEPSEEK_MODEL` / `DEEPSEEK_BASE_URL` / `DEEPSEEK_PROVIDER` / ...
4. CLI flag — `--provider` / `--model` / `--mode` / `--approval-policy` / `--yolo` / ...

### 9.2 关键环境变量

| Env | 用途 |
|---|---|
| `DEEPSEEK_API_KEY` | API key（本地 vLLM 传任意非空值） |
| `DEEPSEEK_BASE_URL` | API 端点 |
| `DEEPSEEK_MODEL` | 模型字符串 |
| `DEEPSEEK_PROVIDER` | provider 名（`deepseek` / `vllm` / ...） |
| `DEEPSEEK_REASONING_EFFORT` | `off` / `low` / `medium` / `high` |
| `DEEPSEEK_ALLOW_INSECURE_HTTP` | 允许 HTTP（内网用） |
| `DEEPSEEK_FORCE_HTTP1` | 强制 HTTP/1.1（vLLM 内网 ALPN 问题） |
| `DEEPSEEK_MAX_OUTPUT_TOKENS` | engine output token 上限（vLLM 小 context 用） |
| `DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS` | SSE idle 超时 |

### 9.3 主要 CLI 参数

```bash
deepseek-tui [options] [prompt...]

--mode {agent|plan|yolo}
--approval-policy {auto|suggest|never}
--sandbox-mode workspace-write|...
--yolo                    # 等价 --mode yolo
--provider <name>
--model <name>
--base-url <url>
--api-key <key>
--profile <name>          # 配置 profile 切换
--skip-onboarding
-p, --prompt <text>       # 非交互模式（exec 一次）
```

子命令：`run` / `exec` / `review` / `serve` / `mcp` / `models` / `sessions` / `resume` / `fork` / `apply` / `eval` / `auth` / `config` / `thread` / `sandbox` / `app-server`

### 9.4 给 pinvou3 设计的启示

**A. 立刻能复用的（白嫖）**：

| pinvou3 想做 | 直接用 DeepSeek-TUI 哪个 |
|---|---|
| 工具调用循环 + 流式 | EngineHandle.tx_op SendMessage + rx_event |
| Tool 注册 + 执行 | ToolRegistryBuilder + ToolSpec |
| 审批流 | Event::ApprovalRequired + tx_approval |
| 用户输入 modal | request_user_input 工具（但协议适配是已知问题） |
| 子代理 | agent_spawn 工具 |
| Skills 系统 | `~/.deepseek/skills` + SkillRegistry |
| MCP 集成 | mcp_pool（已有 builder.with_mcp_tools()） |
| Context window 管理 | Compaction + Cycle 自动跑 |
| 防死循环 | LoopGuard 自动跑 |
| Hook 系统 | hooks.toml 配置 |

**B. pinvou3 独有，DeepSeek-TUI 没有的**：

| pinvou3 概念 | DeepSeek-TUI 是否有等价物 |
|---|---|
| CombinedPlanner（首句拆 6 阶段） | 部分等价：plan mode + update_plan + base.md 引导，**但 LLM 自主决定，非强制** |
| Mode 7 枚举（collect/produce_options/final_output…） | 无。DeepSeek-TUI 只有 3 mode 且不细分输出类型 |
| ContractValidator（硬规则拦截） | 无。DeepSeek-TUI 靠 prompt + approval 软约束 |
| Agent 系统（5 个 prompts/*.md） | 部分等价：Skills 系统功能近似（SKILL.md），但定位不同（skill 是"指令包"，agent 是"领域风格"） |
| 选择卡（信息密度 + markdown 渲染） | request_user_input 工具有，但 TUI 渲染样式不同 |
| Review/PatchOutput 模式 | 无。DeepSeek-TUI 走完整 LLM 复述 + apply_patch 工具，没有 mode 概念 |
| /back /redo /replan | 部分等价：`/restore N` 回滚工作区快照、`/undo`、`/retry`，但没有 milestone 级别回退 |

**C. 关键判断**：

pinvou3 编排层的核心论点是「**本地小模型不可靠，要代码硬卡**」。这个论点决定整层架构存废。

- 如果本地 Qwen3.6 在 DeepSeek-TUI 原生 plan mode 下能自主调 `update_plan` / `checklist_write` / `request_user_input`，那 pinvou3 编排层是过度工程化。
- 如果不行，pinvou3 编排层有价值，但**编排层应该只做"代码硬卡 LLM 自主决策"这一件事**——即 ContractValidator + Mode 枚举。
- 其他所有事情（tool loop / 审批 / 用户输入 / 子代理 / context 管理 / 持久化）应该全部委托给 EngineHandle。

具体说，**最小化 pinvou3 编排层** 的方向应该是：

```
pinvou-platform
├─ contract/           ← 保留：Mode 7 枚举 + 硬规则
├─ combined_planner    ← 保留：本地小模型不会自己 plan，必须硬拆
├─ rollback            ← 保留：milestone 级回退（/back /redo）
├─ workflow            ← 保留：ConversationState + Milestone 状态机
└─ harness 改造        ← 全力切到 EngineHandle，删 1000+ 行自写 tool loop
```

而 `deepseek_harness.rs` / `engine.rs` 里跟工具循环、流式、审批、用户输入相关的全部应该删，靠 EngineHandle 拿事件。

---

## 附录 A：常用文件路径速查

### Engine 核心
- 顶层句柄：`crates/tui/src/core/engine.rs`
- Turn loop：`crates/tui/src/core/engine/turn_loop.rs`
- Streaming：`crates/tui/src/core/engine/streaming.rs`
- Tool exec：`crates/tui/src/core/engine/tool_execution.rs`
- Approval：`crates/tui/src/core/engine/approval.rs`
- Loop guard：`crates/tui/src/core/engine/loop_guard.rs`
- Tool catalog：`crates/tui/src/core/engine/tool_catalog.rs`
- Capacity：`crates/tui/src/core/engine/capacity_flow.rs`

### Tools
- Trait：`crates/tui/src/tools/spec.rs`
- Registry：`crates/tui/src/tools/registry.rs`
- 各工具：`crates/tui/src/tools/*.rs`

### Prompts
- 入口：`crates/tui/src/prompts.rs`
- 模板：`crates/tui/src/prompts/*.md`

### 周边
- Sessions：`crates/tui/src/session_manager.rs`
- Compaction：`crates/tui/src/compaction.rs`
- Cycle：`crates/tui/src/cycle_manager.rs`
- MCP：`crates/tui/src/mcp.rs` + `mcp_server.rs`
- Hooks：`crates/tui/src/hooks.rs`
- Skills：`crates/tui/src/skills/`
- Commands：`crates/tui/src/commands/`
- Config：`crates/tui/src/config.rs`

### CLI 入口
- Cli wrapper：`crates/cli/src/lib.rs` + `main.rs`
- TUI binary：`crates/tui/src/main.rs`

---

## 附录 B：跟 pinvou3 当前实现的对照表

| pinvou3 模块 | 行数 | 对应 DeepSeek-TUI | 状态 |
|---|---|---|---|
| `harness.rs`（trait） | 128 | — | 保留（是唯一接口边界） |
| `deepseek_harness.rs`（自写 tool loop） | 1006 | `core/engine/turn_loop.rs` + `tool_execution.rs` | **应删，委托给 EngineHandle** |
| `engine_harness.rs`（Engine 路径） | 403 | EngineHandle 包装 | **应完成，走 B 路（关 engine 内置 user_input）** |
| `engine_factory.rs` | 248 | — | 改为只构造 EngineHandle |
| `agent_registry.rs` | 343 | `skills/mod.rs` 类似但定位不同 | 保留（领域风格 ≠ skill） |
| `combined_planner.rs` | 1013 | base.md decomposition + update_plan | **保留**（本地模型必须硬拆） |
| `rollback.rs` | 312 | `commands/restore.rs` / `revert_turn` | 保留（milestone 级回退是独家） |
| `contract.rs` + `contract_runtime.rs` + `contract_validator.rs` | 1311 | — | **保留**（硬规则兜底是 pinvou3 灵魂） |
| `workflow.rs` | 649 | — | **保留**（ConversationState + 状态机） |
| `step_builder.rs` | 125 | `prompts.rs` 拼装 | 简化（DeepSeek-TUI 已经会拼） |
| `engine.rs` | 1269 | 全在 `core/engine/` | **大部分应删** |
| `web/mod.rs` SSE looper | — | `rx_event` 流 | **应直接 forward `rx_event`** |
| `prompts/*.md`（5 个 agent） | — | — | 保留（独家） |

**预估清理后 pinvou-platform 代码量**：从 ~7000 行 → ~3500 行，纯粹是「代码硬卡 + 领域 agent」编排层。

---

> 这份文档是 pinvou3 后续设计的参考底座。任何「pinvou3 要新加 X 功能」的讨论，先翻这份文档看 DeepSeek-TUI 有没有等价物，有就复用，没有再自建。
