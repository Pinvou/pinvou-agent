# EngineHandle 重构 — 状态记录

> 这是一个未完成的重构。在做架构梳理 / 重启这个项目时先读这份文档。
> 最后更新：2026-05-12

## 动机

pinvou-platform 的 `deepseek_harness.rs::chat_stream` 自写了 300+ 行 tool loop（LLM
call → tool execute → 累积 messages → 继续）。DeepSeek-TUI 在
`crates/tui/src/core/engine/` 已经有完整的 agent loop（9673 行），含：

- turn_loop / dispatch / streaming / loop_guard / approval / capacity_flow / tool_catalog
- ToolCallProgress（进度反馈，pinvou3 缺）
- ThinkingDelta / ThinkingStarted/Complete（thinking 段独立通道）
- ApprovalRequired / UserInputRequired（专用事件）
- CompactionStarted/Completed（context 自动压缩）
- CapacityDecision/Intervention（context window 智能管理）
- CycleAdvanced（长会话自动 checkpoint）

DeepSeek-TUI 还有 `EngineHandle` 这个 **headless message-passing 接口**
（无 TUI 依赖，专为 TUI + 其他前端解耦设计）。pinvou-platform 应该复用
而不是重写。

按 CLAUDE.md「复用优先」原则，重构方向：让 `AgentHarness` 的实现从
`DeepSeekHarness`（自写 tool loop）切到 `EngineHarness`（包装 EngineHandle）。

## 已完成的工作

### 代码层面（已 commit 在主分支留作未来基础）

- **`pinvou-platform/src/engine_harness.rs`**：`EngineHarness` 实现 `AgentHarness`
  trait；`Harness` enum 包装 `Legacy(DeepSeekHarness)` / `Engine(EngineHarness)`
  两种路径，dispatch trait method
- **`pinvou-platform/src/engine_factory.rs`**：`PinvouEngine` 类型改为
  `PlatformEngine<Harness>`；env `PINVOU_USE_ENGINE_HARNESS=1` 切换路径
- **`pinvou-platform/examples/engine_smoke.rs`**：最小原型，直接调
  `spawn_engine` + `Op::SendMessage` + 读 `rx_event`
- **DeepSeek-TUI fork main 上加了一个 commit**：`effective_max_output_tokens`
  支持 `DEEPSEEK_MAX_OUTPUT_TOKENS` env override，避免 vLLM 小 context model
  撞顶。也是 PR-worthy 给上游

### 验证层面

- engine_smoke 跑通了「发送消息 → 收到事件」基础链路
- pinvou3 主 web 路径切到 Engine 跑「我周六去黄埔水声水库徒步」
  - ✅ 拆解 → planning agent 6 阶段
  - ✅ Stage 0 collect 弹选择卡（`Event::UserInputRequired` 被我们适配成
    `StreamEvent::ToolCallStart{request_user_input}`）
  - ✅ 用户选完后 Stage 1 走起来
  - ❌ **Stage 2 produce_options 卡死** — 见下方根因

## 卡点：协议层根本性不匹配

### 现象

第二次 chat_stream 调用（用户选完进 ms_1 freeform → ms_2 produce_options）后：

```
[engine_harness] SyncSession msgs=5 sys_prompt_len=948 session_id=planning-1
[engine_harness] SendMessage content="（系统：...）" reasoning_effort=Some("off")
                                                                ← 然后没有任何 rx_event
```

没 TurnComplete / 没 MessageDelta / 没 Error。engine 静默。

### 根因分析（未完成投入更多调研）

DeepSeek-TUI engine 假设 `request_user_input` 是**它的内置工具**：

- engine 内部识别这个 tool_use → 发 `Event::UserInputRequired { id, request }`
- engine **阻塞 turn 等 `EngineHandle.tx_user_input.send(UserInputDecision)`**
- 用户回应后 engine 内部把 UserInputDecision 拼成 ToolResult 写入 session →
  继续 turn → 发后续 LLM 输出

pinvou-platform 当前协议**完全不同**：

- 用户选择 → 新一次 HTTP 请求 + tool_result body + `apply_choice_result` 在
  `ConversationState` 层 mark_done milestone + 用 `set_context` 写入选择
- 下一次 chat_stream 调用走**新 turn**，注入 context，让 LLM 看到上下文继续

我用 `Op::CancelRequest` 的 workaround：第一次 `UserInputRequired` 出现就发
CancelRequest 取消 engine turn，然后 yield Done 让 web 层走它的协议。但
**CancelRequest 后 engine 的状态没复位干净**——下一次 SyncSession +
SendMessage 进 Op queue 但 engine 处理不出新 turn 的 event。

## 三个候选解决方向（按工作量排序）

### A. 让 web 层走 `tx_user_input` 协议（最对的方向，工作量大）

把"用户选完发新 HTTP"改成"用户选完通过同一个 EngineHandle 发 UserInputDecision，
engine 继续当前 turn 自然吐出后续 LLM 输出"。

需要重设计：

- `ConversationState` 跟 engine session 的对应关系（engine 有自己的 session）
- `apply_choice_result` 不再 mark_done milestone + set_context，而是 send
  UserInputDecision
- web SSE 流不在选择卡处 yield Done，而是保持开放等 engine 继续吐 events
- milestone 概念可能要重新审视：pinvou3 用 milestone 切片，engine 把整个对话
  看成连续 turn — 这两套世界观要对齐

工作量：**1-2 周的设计 + 实现**。

### B. 关掉 engine 的内置 `request_user_input`（中等工作量）

让 engine 不要把 `request_user_input` 当内置工具——它就是个普通 tool_use，
透传给上层，pinvou-platform 自处理（现状的 pinvou3 协议）。

需要看 `EngineConfig.features` / `tool_catalog.rs` 是否允许：

- BYO ToolRegistry（用我们的 11 工具集合，不要 engine 默认的 23+）
- 或者：禁用某个 feature flag 关掉内置 user_input

如果 deepseek-tui 不允许，**要改 deepseek-tui fork**——可能 PR 给上游不一定接，
因为人家的设计就是有内置 user_input。

工作量：**几小时调研 + 几小时改 fork**（如果可行）。

### C. 永久回退 Legacy（重构放弃）

承认 deepseek-tui engine 跟 pinvou3 协议世界观差太大，重构不值得。继续维护
`DeepSeekHarness` 自写 tool loop，逐项把 engine 已有的功能（进度反馈/审批/
ThinkingDelta 区分等）"借鉴"过来。

工作量：**0（删 EngineHarness 代码即可）**，但长期维护负担继续累积。

## 当前状态

- `run-local.sh` 已注释掉 `PINVOU_USE_ENGINE_HARNESS=1`，**默认走 Legacy**
- `EngineHarness` / `Harness` enum / 切换逻辑 **代码保留**（不删除）
- `DEEPSEEK_MAX_OUTPUT_TOKENS=16384` env 保留（对两条路径无害）
- DeepSeek-TUI fork main 上的 `DEEPSEEK_MAX_OUTPUT_TOKENS` env override
  commit 留着（也作为给上游的 PR 候选）

## 重启这个项目时该做的事

1. **决定方向**：A / B / C 哪条路
2. **如果走 A**：先做架构设计 doc，对齐 ConversationState ↔ engine session 模型；
   然后逐步迁移 web 层 → engine 协议
3. **如果走 B**：先做 PoC 看 engine 能否 BYO ToolRegistry / 禁用 user_input
   feature；可行就改适配代码 + Engine 路径重测试
4. **如果走 C**：删 `engine_harness.rs` + `Harness::Engine` variant + 相关
   切换逻辑；engine_factory 直接返回 DeepSeekHarness

## 关联资源

- DeepSeek-TUI EngineHandle: `DeepSeek-TUI/crates/tui/src/core/engine.rs:197`
- Op / Event: `DeepSeek-TUI/crates/tui/src/core/ops.rs` / `events.rs`
- 当前 EngineHarness 实现: `pinvou-platform/src/engine_harness.rs`
- Engine smoke 原型: `pinvou-platform/examples/engine_smoke.rs`
- vLLM fix PR: https://github.com/Hmbown/DeepSeek-TUI/pull/?（自己看 fork 状态）
