# pinvou3 现状与待办

> 最后更新: 2026-05-11

---

## 一、当前状态（一句话）

新设计完整落地：`prompts/*.md` 注册 agent → LLM 单次调用拆解 + 选工具 → harness 自动执行工具循环（web_search 等真能跑）→ Contract 校验硬边界 → slash 命令回退状态机。**P1.1-4 已完成**，legacy 模块全删除，前端已去 app 选择器，端到端集成测试就位。`cargo test` = **99 lib + 11 integration = 110 passed**。

---

## 二、已落地

### 核心模块

| 模块 | 状态 |
|---|---|
| `agent_registry`（扫 prompts/*.md） | ✅ |
| `combined_planner`（单次调用分类 + 拆解） | ✅ |
| `rollback`（/back /skip /redo /replan /use） | ✅ |
| `contract` + `contract_runtime` + `contract_validator` | ✅ |
| `workflow`（GlobalMode / Milestone / ContextEntry 归属 / rewind） | ✅ |
| `engine.ensure_combined_plan` + `agent_system_prompt` | ✅ |
| `deepseek_harness` 工具自动执行循环（async-stream） | ✅ |
| 11 个工具接入（web_search/fetch_url/read_file/write_file/edit_file/list_dir/grep_files/file_search/exec_shell + 变体） | ✅ |
| 5 个初始 agent（qa/doc_generation/data_analysis/planning/generic） | ✅ |

### Web

| 项 | 状态 |
|---|---|
| `/api/chat/stream` 主路径（CombinedPlanner + slash + auto-continue + Contract） | ✅ |
| 前端去 app_id、`milestone_progress` 事件处理、侧边栏映射 agent | ✅ |

### 测试

| 项 | 状态 |
|---|---|
| 99 个 lib 单元测试 | ✅ |
| 11 个端到端集成测试（tests/full_flow.rs） | ✅ |
| MockHarness 公开（生产代码不调用，集成测试可用） | ✅ |

### Legacy 清理

| 项 | 状态 |
|---|---|
| 删除 `apps/` 目录 + `app.rs` + `dynamic_planner.rs` | ✅ |
| 删除 `response_checker.rs` + `reviewer.rs` | ✅ |
| 删除 `tui/` 模块（速效化的旧 TUI，新设计专注 web） | ✅ |
| 删除 engine.rs legacy 方法（load_app / ensure_plan_initialized / decompose_and_execute / step_execute） | ✅ |
| 删除 web/mod.rs LegacyFallback 分支 → 改为 FreeFlow | ✅ |
| `--apps-dir` / `--tui` CLI 参数移除 | ✅ |
| Milestone 类型从 app.rs 移到 workflow.rs（去掉 7 个 legacy 字段） | ✅ |
| step_builder 精简至只剩 `build_contract_prompt` | ✅ |

### 文档

| 项 | 状态 |
|---|---|
| `设计架构文档-pinvou3.md` 全量重写 | ✅ |
| `CLAUDE.md` 项目规则（边界、复用优先、常见错误） | ✅ |

---

## 三、下一步 P1（按优先度排）

### 1. 真正的审批流（替代 YOLO）

当前 `ToolContext.auto_approve = true`，写盘 / shell 命令直接跑。MVP 可接受（本地单用户、workspace 边界），但用户哪天 LLM 跑飞了会想要审批。

实现：
- `ApprovalRequirement::Required` 工具调用 → harness 发 `ToolApprovalRequest` SSE 事件 → 前端弹审批 UI → 用户响应通过新 `tool_result` 回到 stream → harness 继续 / 取消
- 需要 SSE 双向交互模式（类似现有 `request_user_input` 但更通用）

风险：中。harness 工具循环要支持挂起 + 恢复。

### 2. 结构化历史

当前 tool 交互以文本形式（`🔧 [tool] args` / `📄 结果`）写进 `engine.messages`，跨轮 LLM 看的是 text。够用但不严谨（LLM 可能把 `🔧` 解读为自己说的内容）。

正解：
- `HistoryMessage` 加 `tool_calls` + `tool_result_for` 字段
- `deepseek_harness::to_message_request` 据此重建 `ContentBlock::ToolUse` / `ContentBlock::ToolResult`
- Harness 工具循环结束时通过 `StreamEvent::HistorySnapshot { messages }` 把完整历史回给 web 层

风险：中。HistoryMessage 跨模块边界。

### 3. 流式分类优化

CombinedPlanner 当前等完整 JSON（~500ms on Qwen 7B）才路由。优化：

- 流式 JSON 解析
- `agent` 字段一旦解析出（前 ~20-50ms）即可决定路径
- `agent="qa"` 立刻中止后续 JSON，转去做真正的 QnA 流式回答

风险：低-中。需要流式 JSON 解析。

### 4. 真实 LLM 链路测试

当前集成测试用 `MockHarness`（绕过了 DeepSeekHarness 的工具循环）。补一组测试：
- 使用 `LlmClient` trait + 测试 mock client 模拟 SSE 事件序列
- 走完整 `DeepSeekHarness::chat_stream` 路径
- 验证 tool loop 多轮调用、自动 vs 透传工具的分流、消息历史拼接

风险：低。补丁式增加。

---

## 四、P2（远期）

| 项 | 描述 |
|---|---|
| `CheckpointStore` | 对话断点持久化，浏览器刷新后恢复 |
| `SubAgentRouter` | 并行子任务分发 |
| LLM-as-judge | 替代当前结构性正则的语义校验（生成文档是否合理等） |
| 多层 agent 选择 | agent 数量 > 20 时，先选部门再选 agent |
| 隐式回退检测 | LLM 检测「重做/不对」信号 → 弹回退选项卡片 |
| TUI 重建 | 基于新架构重写 ratatui 前端（旧 tui/ 已删除） |

---

## 五、已知不完整 / 取舍点

- **YOLO 工具执行**：写盘/shell 不弹审批，靠 workspace 边界。见 P1.1。
- **跨轮 tool 历史用 text 而非结构化**：见 P1.2。
- **CombinedPlanner 第一次调用 500ms 延迟**：见 P1.3。
- **DeepSeekHarness 工具循环未独立测试**：集成测试用 MockHarness 绕开了。见 P1.4。
- **ChatRequest.app_id** 字段保留兼容（Option<String>），前端已不发，后续删字段。

---

## 六、检查点

```
cargo test --lib                       99 passed
cargo test --test full_flow            11 passed
cargo build                            clean（仅 deepseek-tui 5 个外部 warning）
cargo run --example agent_smoke        5 agents loaded
```

启动：
```bash
DEEPSEEK_API_KEY=... cargo run --bin pinvou-platform
# 访问 http://127.0.0.1:9876
```
