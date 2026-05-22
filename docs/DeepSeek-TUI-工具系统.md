# DeepSeek-TUI 工具系统

> 创建：2026-05-15
> 范围：DeepSeek-TUI v0.8.37 主线，pinvou3 视角
> 目的：摸清工具注册、调度、执行链路；列出全部工具与使用场景

---

## 1. 总览

```
LLM 输出 tool_call (assistant message)
        │
        ▼
core/engine/dispatch.rs        ── 解析 / 批处理 / 并行决策
        │
        ▼
core/engine/tool_execution.rs::execute_tool_with_lock
        │
        ├─ MCP tool？  ─▶  McpPool::execute
        └─ Registry tool？  ─▶  ToolRegistry::execute_full_with_context
                                  │
                                  ▼
                          ToolSpec::execute(input, &ToolContext)
                                  │
                                  ▼
                          ToolResult { success, content, metadata }
                                  │
                                  ▼
                          作为 tool message 喂回下一轮
```

四个关键模块：
- `tools/spec.rs` — `ToolSpec` trait + `ToolContext` + `ToolResult`
- `tools/registry.rs` — `ToolRegistryBuilder` 的所有 `with_*` 方法
- `core/engine/tool_setup.rs` — 每轮按 mode + features 组装 builder
- `core/engine/tool_execution.rs` — 实际并发与执行调度

---

## 2. ToolSpec trait

每个工具实现这套接口（`tools/spec.rs:598`）：

```rust
pub trait ToolSpec: Send + Sync {
    fn name(&self) -> &str;                 // LLM 看到的工具名
    fn description(&self) -> &str;          // schema 里的 description
    fn input_schema(&self) -> Value;        // JSON Schema, 喂给 LLM
    fn capabilities(&self) -> Vec<ToolCapability>;
    fn approval_requirement(&self) -> ApprovalRequirement;
    fn is_read_only(&self) -> bool;         // 默认从 capabilities 推
    fn supports_parallel(&self) -> bool;    // 默认 false
    fn defer_loading(&self) -> bool;        // 默认 false (true 不发给模型)
    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError>;
}
```

`ApprovalRequirement` 三档：`Auto` / `Suggest` / `Required`，由 capabilities 自动推断：含 `ExecutesCode` → Required，含 `WritesFiles` → Suggest，否则 Auto。

`ToolContext`（同文件）携带：workspace 路径、sandbox policy、shell_manager、task_manager、handle_store、rlm_sessions、hook_executor、features、trust_mode、auto_approve、approval_mode 等会话级状态。

---

## 3. 注册流程

### 3.1 ToolRegistryBuilder（`tools/registry.rs`）

链式调用，每个 `with_*` 方法塞一组相关工具。常见调用单元：

| Builder 方法 | 添加的工具 |
|---|---|
| `with_file_tools` | read_file, write_file, append_file, edit_file, list_dir |
| `with_read_only_file_tools` | read_file, list_dir |
| `with_search_tools` | grep_files, file_search |
| `with_web_tools` | web_search, fetch_url, web_run |
| `with_shell_tools` | exec_shell, exec_shell_cancel, note |
| `with_patch_tools` | apply_patch |
| `with_git_tools` | git_status, git_diff |
| `with_git_history_tools` | git_log, git_show, git_blame |
| `with_diagnostics_tool` | diagnostics |
| `with_pandoc_tools` | pandoc_convert（运行时探测 pandoc 二进制） |
| `with_image_ocr_tools` | image_ocr（探测 tesseract） |
| `with_skill_tools` | load_skill |
| `with_project_tools` | project_map |
| `with_test_runner_tool` | run_tests |
| `with_validation_tools` | validate_data |
| `with_tool_result_retrieval_tool` | retrieve_tool_result |
| `with_handle_tools` | handle_read |
| `with_runtime_task_tools` | task_create, task_list, task_read, task_cancel, task_gate_run, task_shell_start, task_shell_wait, pr_attempt_* |
| `with_runtime_read_only_task_tools` | task_list, task_read |
| `with_revert_turn_tool` | revert_turn |
| `with_rlm_tool` | rlm_open, rlm_eval, rlm_configure, rlm_close |
| `with_review_tool` | review |
| `with_recall_archive_tool` | recall_archive |
| `with_user_input_tool` | request_user_input |
| `with_parallel_tool` | multi_tool_use.parallel |
| `with_note_tool` | note |
| `with_fim_tool` | fim_edit |
| `with_todo_tool` | todo_write, todo_add, todo_update, todo_list（checklist + 普通各一份，共 8） |
| `with_plan_tool` | update_plan |
| `with_subagent_tools` | agent_open, agent_eval, agent_result, agent_cancel, agent_close, agent_list, resume_agent, delegate_to_agent |
| `with_vision_tools` | image_analyze |
| `with_remember_tool` | remember |
| `with_notify_tool` | notify |
| `with_mcp_tools` | （MCP server 动态注册） |

两个聚合方法：
- `with_agent_tools(allow_shell)` — 用于 Agent/Yolo 通用基线
- `with_full_agent_surface(...)` — 子代理用的全集

### 3.2 每轮组装（`core/engine/tool_setup.rs::build_turn_tool_registry_builder`）

按 mode 分支：

**Plan 模式（只读）**：
```
with_read_only_file_tools + with_search_tools + with_git_tools + with_git_history_tools
+ with_diagnostics_tool + with_skill_tools + with_validation_tools + with_handle_tools
+ with_runtime_read_only_task_tools + with_todo_tool + with_plan_tool
```

**Agent / Yolo 模式**：
```
with_agent_tools(allow_shell) + with_todo_tool + with_plan_tool
```

**两个 mode 都附加**：
```
+ with_review_tool + with_user_input_tool + with_parallel_tool + with_recall_archive_tool
```

**仅 Agent/Yolo 附加**：
```
+ with_rlm_tool + with_fim_tool
```

**Features 守卫（tool_setup.rs 内）**：
- `Feature::ApplyPatch && !Plan` → `with_patch_tools()` （多余 —— `with_agent_tools` 已包含）
- `Feature::WebSearch` → `with_web_tools()` （多余 —— 同上）
- `Feature::ShellTool && allow_shell && !Plan` → `with_shell_tools()` （多余）
- `Feature::VisionModel && vision_config.is_some()` → `with_vision_tools()`
- `memory_enabled` → `with_remember_tool()`
- 无条件 → `with_notify_tool()`

**Features 守卫（engine.rs 内，非 tool_setup）**：
- `Feature::Subagents` → `with_subagent_tools(...)` + 启用 fork_context + mailbox
- `Feature::Mcp` → 把 McpPool 工具注册进 registry

**关键认知**：`Feature::ApplyPatch / WebSearch / ShellTool` 在 registry 层**没有任何效果**——因为 `with_agent_tools` 内部已无条件添加。真正能改变工具列表的只有 `Subagents / Mcp / VisionModel`。

### 3.3 沙箱

`tool_setup.rs::sandbox_policy_for_mode`：
- Plan → `ReadOnly`
- Agent → `WorkspaceWrite { workspace, network_access: true }`
- Yolo → `DangerFullAccess`

Plan 模式的沙箱配合工具白名单双重保险：即使有人意外把 exec_shell 注册到 Plan，沙箱也会拦住。

---

## 4. 工具调用执行链

### 4.1 调度（`core/engine/dispatch.rs`）

模型一轮可能输出多个 tool_call。dispatch 决定：
- **串行 vs 并行**：`should_parallelize_tool_batch` 判断——全部工具 `supports_parallel: true` 且无副作用冲突才并行
- **批处理**：`multi_tool_use.parallel` 是个特殊工具，模型可以用一次调用包多个子调用

### 4.2 单次执行（`tool_execution.rs::execute_tool_with_lock`）

1. 判 MCP 还是 registry tool
2. 抢锁：`supports_parallel` 工具用读锁可共享，写工具独占写锁
3. `InteractiveTerminalGuard` 处理交互式工具（如 shell wait）对 TUI scrollback 的影响
4. 调 `ToolSpec::execute`
5. 记 `tracing` + duration_ms + output_bytes
6. 返回 `ToolResult`

### 4.3 审批门控

每个工具的 `approval_requirement` 决定是否在执行前发 `Event::ApprovalRequest`：
- `Auto` — 直接跑（read_file / git_log / web_search 等）
- `Suggest` — 默认跑，但 `approval_mode=suggest` 时弹审批（write_file / edit_file 等）
- `Required` — 必须用户点同意才跑（exec_shell / apply_patch 等）

`auto_approve: true`（YOLO）跳过所有 Required 审批。

### 4.4 大输出处理

`large_output_router.rs` 把超长 tool_result 拆成：
- 头部摘要直接喂模型
- 完整内容存 handle，下次模型用 `handle_read` 按需读
- 同时存 `tool_result_retrieval` 索引，可按 tool_call_id 重取

---

## 5. 全部工具清单（按场景分组）

### 文件 IO

| 工具 | 入口 | 用途 |
|---|---|---|
| read_file | with_file_tools | 读文件（支持 chunked / 内嵌 PDF 抽取） |
| write_file | with_file_tools | 写文件（创建或覆盖） |
| append_file | with_file_tools | 追加写文件（适合大产物分块生成） |
| edit_file | with_file_tools | 字符串级 patch 编辑 |
| list_dir | with_file_tools | 列目录 |
| apply_patch | with_patch_tools | 类 git diff 风格的多文件 patch |
| revert_turn | with_revert_turn_tool | 撤销本 turn 的所有文件修改（基于 snapshot） |
| fim_edit | with_fim_tool | Fill-in-the-Middle 边写边推理的编辑 |

### 搜索 / 索引

| 工具 | 入口 | 用途 |
|---|---|---|
| grep_files | with_search_tools | ripgrep 内容搜索 |
| file_search | with_search_tools | 按文件名 / 路径搜索 |
| project_map | with_project_tools | 仓库结构概览（生成树状摘要） |
| recall_archive | with_recall_archive_tool | 翻历史 cycle 归档 |

### Shell / 代码执行

| 工具 | 入口 | 用途 |
|---|---|---|
| exec_shell | with_shell_tools | 跑 shell 命令（同步或后台 task） |
| exec_shell_cancel | with_shell_tools | 取消后台 shell task |
| task_shell_start | with_runtime_task_tools | 起后台 shell job |
| task_shell_wait | with_runtime_task_tools | 等后台 shell job 输出（增量） |
| note | with_note_tool | 写 scratch 笔记到 notes 文件 |

> code_execution 不是独立工具——上游通过 `exec_shell` 跑 `python -c` 实现。pinvou3 的 INSTRUCTIONS_MD 给它别名引导。

### Web

| 工具 | 入口 | 用途 |
|---|---|---|
| web_search | with_web_tools | 关键词搜索（走 search_provider） |
| fetch_url | with_web_tools | 拉单 URL 内容 |
| web_run | with_web_tools | 浏览器/JS 执行的高级 fetch |

### Git

| 工具 | 入口 | 用途 |
|---|---|---|
| git_status | with_git_tools | working tree 状态 |
| git_diff | with_git_tools | diff |
| git_log | with_git_history_tools | 提交历史 |
| git_show | with_git_history_tools | 单次 commit 详情 |
| git_blame | with_git_history_tools | 行级追责 |

### GitHub（按需启用）

`github_issue_context / github_pr_context / github_comment / github_close_issue` — 走 GitHub API，需要 token，pinvou3 未注册。

### 任务 / 流程

| 工具 | 入口 | 用途 |
|---|---|---|
| task_create / task_list / task_read / task_cancel | with_runtime_task_tools | 持久化任务管理（DeepSeek-TUI 的 RLM workflow） |
| task_gate_run | with_runtime_task_tools | task 依赖门控 |
| pr_attempt_record / _list / _read / _preflight | with_runtime_task_tools | PR 提交尝试记录（用于自动化迭代） |
| todo_write / todo_add / todo_update / todo_list | with_todo_tool | 模型自维护 TODO 清单（两套 view：checklist + plan，故 8 个变体） |
| update_plan | with_plan_tool | Plan 模式下的方案大纲 |
| review | with_review_tool | 自审子流程（模型回看刚做的改动） |
| recall_archive | with_recall_archive_tool | 翻 checkpoint 归档 |

### 用户交互

| 工具 | 入口 | 用途 |
|---|---|---|
| request_user_input | with_user_input_tool | 弹模态框问用户（pinvou3 渲染为内嵌卡片） |
| notify | with_notify_tool | OSC 9 桌面通知 |
| multi_tool_use.parallel | with_parallel_tool | 单次调用里并发多个子工具 |

### 子代理（experimental）

`agent_open / agent_eval / agent_result / agent_cancel / agent_close / agent_list / resume_agent / delegate_to_agent` —— 起独立 sub-agent 跑子任务，需 `Feature::Subagents`。pinvou3 默认开但 UI 不支撑。

### RLM（Read-Eval-Loop Model）持久会话

`rlm_open / rlm_eval / rlm_configure / rlm_close` —— 起一个保留状态的 Python/REPL 子内核，多轮共享变量。Qwen3.6 撑不住这套编排，pinvou3 不引导。

### 视觉 / OCR

| 工具 | 入口 | 用途 |
|---|---|---|
| image_analyze | with_vision_tools | 调视觉模型分析图（pinvou3 关闭，Qwen3.6 无视觉） |
| image_ocr | with_image_ocr_tools | tesseract OCR（运行时探测） |
| pandoc_convert | with_pandoc_tools | docx/odt/pdf 等 → markdown |

### 元

| 工具 | 入口 | 用途 |
|---|---|---|
| diagnostics | with_diagnostics_tool | 跑 LSP 拉当前文件诊断 |
| validate_data | with_validation_tools | JSON / YAML schema 校验 |
| handle_read | with_handle_tools | 读 var_handle 大对象 |
| retrieve_tool_result | with_tool_result_retrieval_tool | 按 tool_call_id 回取历史结果 |
| run_tests | with_test_runner_tool | 跑测试套件 |
| load_skill | with_skill_tools | 加载 SKILL.md 引导 |
| remember | with_remember_tool | 写入 user_memory（pinvou3 关闭） |
| automation_create / _list / _read / _update / _run | （未在默认 builder） | 持久 automation 定义 |

### MCP（动态）

任何 MCP server 暴露的工具会被 `with_mcp_tools` 反射进 registry，命名前缀 `mcp__<server>__<tool>`。pinvou3 当前未连接任何 MCP server。

---

## 6. pinvou3 实际可见的工具

`build_turn_tool_registry_builder` Yolo 路径展开后大约 **45 个工具**：

```
read_file write_file append_file edit_file list_dir
grep_files file_search
exec_shell exec_shell_cancel note
web_search fetch_url web_run
apply_patch revert_turn fim_edit
git_status git_diff git_log git_show git_blame
diagnostics validate_data handle_read retrieve_tool_result
project_map run_tests load_skill
pandoc_convert image_ocr
task_create task_list task_read task_cancel task_gate_run
task_shell_start task_shell_wait
pr_attempt_record pr_attempt_list pr_attempt_read pr_attempt_preflight
todo_write × 2 todo_add × 2 todo_update × 2 todo_list × 2
update_plan
review request_user_input recall_archive
rlm_open rlm_eval rlm_configure rlm_close
multi_tool_use.parallel notify
agent_open agent_eval agent_result agent_cancel agent_close agent_list
resume_agent delegate_to_agent
```

INSTRUCTIONS_MD 只显式提及核心工具（read_file / write_file / append_file / edit_file / exec_shell / grep_files / file_search / web_search + code_execution 别名）。剩余工具靠模型自行从 schema 探索。

Plan 模式约 **15 个工具**：file 只读、search、git（含 history）、diagnostics、skill、validation、handle、task 只读、todo、plan、review、user_input、parallel、recall_archive。

---

## 7. 与上游差异 / pinvou3 自定义

pinvou3 不改 ToolRegistry 或 ToolSpec —— 完全复用上游。差异只在：

1. **EngineConfig.features**：默认全 on，未自定义裁剪（这是后续可优化点）
2. **memory_enabled = false** → remember 不注册
3. **vision_config = None + Feature::VisionModel = false** → image_analyze 不注册
4. **allow_shell = true** + Yolo 模式 → exec_shell 等可用

详细取舍见 `docs/system-prompt-与底座的差异.md`。

---

## 8. 调试 / 诊断手段

观测 tool 行为：

```bash
RUST_LOG=engine.tool_execution=debug ./pinvou3-app/run-dev.sh
```

输出 `tool.exec.start` 和 `tool.exec.end`，含 tool 名、dispatch 路径、耗时、输出字节数、success 标志。

查工具是否注册：在 DeepSeek-TUI 单独跑 `/tools` slash command 列当前 registry。

---

## 9. 相关代码索引

- trait + Context：`DeepSeek-TUI/crates/tui/src/tools/spec.rs`
- builder：`DeepSeek-TUI/crates/tui/src/tools/registry.rs`
- 按 mode 装配：`DeepSeek-TUI/crates/tui/src/core/engine/tool_setup.rs`
- 调度：`DeepSeek-TUI/crates/tui/src/core/engine/dispatch.rs`
- 执行：`DeepSeek-TUI/crates/tui/src/core/engine/tool_execution.rs`
- Features：`DeepSeek-TUI/crates/tui/src/features.rs`
- MCP 注入：`DeepSeek-TUI/crates/tui/src/core/engine.rs::1090`
- Subagent 注入：`DeepSeek-TUI/crates/tui/src/core/engine.rs::1077`
