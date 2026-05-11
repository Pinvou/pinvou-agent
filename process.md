# pinvou3 现状与待办

> 最后更新: 2026-05-11

---

## 一、当前状态（一句话）

新设计主路径接通：`prompts/*.md` 注册 agent → LLM 单次调用拆解 + 选工具 → harness 自动执行工具循环（web_search 等真能跑）→ Contract 校验硬边界 → slash 命令回退状态机。`cargo test --lib` = **173 passed**。

---

## 二、已落地

| 模块 | 行数 | 状态 |
|---|---|---|
| `agent_registry` | ~280 | ✅ |
| `combined_planner`（单次调用分类 + 拆解） | ~400 | ✅ |
| `rollback`（/back, /skip, /redo, /replan, /use） | ~270 | ✅ |
| `contract` + `runtime` + `validator`（硬边界） | ~750 | ✅ |
| `workflow`（GlobalMode, ContextEntry 归属, rewind） | ~480 | ✅ |
| `engine.ensure_combined_plan` + `agent_system_prompt` | — | ✅ |
| `web/mod.rs` 主路径（agent path + slash + auto-continue） | — | ✅ |
| `DeepSeekHarness` 工具循环（async-stream） | — | ✅ |
| 11 个工具接入（web_search/fetch_url/read_file/write_file/edit_file/list_dir/grep_files/file_search/exec_shell + 变体） | — | ✅ |
| 工具交互文本化进 engine.messages（跨轮历史） | — | ✅ |
| 5 个初始 agent（qa / doc_generation / data_analysis / planning / generic） | — | ✅ |
| 设计文档全量重写 | — | ✅ |

---

## 三、下一步 P1（按优先度排）

### 1. Legacy 清理（无功能影响，纯减债）

- 删除 `apps/` 目录（旧 App 配置）
- 删除 `pinvou-platform/src/app.rs`（AppConfig / AppRegistry）
- 删除 `pinvou-platform/src/dynamic_planner.rs`
- `engine.rs` 删除 `ensure_plan_initialized` / `decompose_and_execute` / `current_app` 字段
- `engine_factory.rs` 不再创建 AppRegistry
- `main.rs` 删除 `--apps-dir` 参数

风险：低。新路径已不依赖。删完 cargo check 跑通即可。

### 2. 从 web 主路径移除 `ResponseChecker` / `LLMReviewer` 调用

- `web/mod.rs:514` 还在 LLM 响应后跑 `ResponseChecker::check`
- Contract Validator 已覆盖其推进逻辑；`[OK]/[MORE]/[BLOCKED]` 信号路由可退役
- 移除后删除 `response_checker.rs` + `reviewer.rs`

风险：中。需要验证 ContractValidator 的 advance 判断完备（OnChoice/OnValidOutput/ManualContinue 三种 policy 都试过）。

### 3. Frontend 改造

- HTML/JS 不再发 `app_id`（新路径已经忽略，但发了浪费且误导）
- 处理 `sse_milestone_progress` 事件（不带 `done` 标志）：状态更新但保持 EventSource 连接，等下一段 LLM stream
- 移除"等待用户输入'继续'"的 UI 提示

风险：低。当前 frontend `if (p.done) continue;` 已经能跳过中间 done 事件，但 UI 文案需更新。

### 4. 真实 LLM 集成测试

- `tests/` 目录加端到端测试：
  - MockLlmClient 按预设序列返回 JSON（模拟拆解结果）
  - 走完整 `chat_stream` 路径，断言 SSE 事件序列
  - 验证 tool loop 多轮调用 + 透传工具行为
- 当前 173 个 unit test 都是模块级，缺整链路 mock 测试

风险：低。补丁式增加。

### 5. 真正的审批流（替代 YOLO）

当前 `ToolContext.auto_approve = true`，写盘 / shell 命令直接跑。MVP 可接受（本地单用户、workspace 边界），但用户哪天 LLM 跑飞了会想要审批。

实现：
- `ApprovalRequirement::Required` 工具调用 → harness 发 `ToolApprovalRequest` SSE 事件 → 前端弹审批 UI → 用户响应通过新 `tool_result` 回到 stream → harness 继续 / 取消
- 需要 SSE 双向交互模式（类似现有 `request_user_input` 但更通用）

风险：中。架构改动较大（harness 工具循环要支持挂起 + 恢复）。

### 6. 结构化历史

当前 tool 交互以文本形式（`🔧 [tool] args` / `📄 结果`）写进 `engine.messages`，跨轮 LLM 看的是 text。够用但不严谨。

正解：
- `HistoryMessage` 加 `tool_calls: Vec<ToolCallBlock>` + `tool_result_for: Option<String>` 字段
- `deepseek_harness::to_message_request` 根据这些字段重建 `ContentBlock::ToolUse` / `ContentBlock::ToolResult`
- Harness 工具循环结束时通过 `StreamEvent::HistorySnapshot { messages }` 把完整历史回给 web 层

风险：中。HistoryMessage 跨模块边界。

### 7. 流式分类优化

CombinedPlanner 当前等完整 JSON（~500ms on Qwen 7B）才路由。优化：

- 解析 JSON 时按字段顺序流式出
- `agent` 字段一旦解析出（前 ~20-50ms）即可决定路径
- `agent="qa"` 立刻中止后续 JSON，转去做真正的 QnA 流式回答

风险：低-中。需要流式 JSON 解析。

---

## 四、P2（远期）

| 项 | 描述 |
|---|---|
| `CheckpointStore` | 对话断点持久化，浏览器刷新后恢复 |
| `SubAgentRouter` | 并行子任务分发 |
| LLM-as-judge | 替代当前结构性正则的语义校验（生成文档是否合理等） |
| 多层 agent 选择 | agent 数量 > 20 时，先选部门再选 agent |
| 隐式回退检测 | LLM 检测「重做/不对」信号 → 弹回退选项卡片 |

---

## 五、已知不完整 / 取舍点

- **YOLO 工具执行**：写盘/shell 不弹审批，靠 workspace 边界。见 P1.5。
- **跨轮 tool 历史用 text 而非结构化**：见 P1.6。
- **CombinedPlanner 第一次调用 500ms 延迟**：见 P1.7。
- **`apps/` 目录还在**：legacy fallback 用，等清理后删除。见 P1.1。
- **ResponseChecker 还在主路径**：双重判断推进，目前不冲突但冗余。见 P1.2。

---

## 六、检查点

```
cargo test --lib                            173 passed
cargo build                                 clean (1 deprecation warning, 已 allow)
cargo run --example agent_smoke             5 agents loaded
```

跑起来的命令：
```bash
DEEPSEEK_API_KEY=... cargo run --bin pinvou-platform
# 访问 http://127.0.0.1:9876
```
