# DeepSeek-TUI 工具系统

> 创建：2026-05-15
> 范围：DeepSeek-TUI v0.8.37 主线，pinvou3 视角
> 目的：摸清工具注册、调度、执行链路；列出全部工具与使用场景
>
> **注（2026-11 回填）**：本文为 v0.8.37 机制史料。底座已更名 `CodeWhale`（文中 `DeepSeek-TUI/`
> 路径对应现 `CodeWhale/`），且工具面在 v0.9.x
> 完成治理重构（`PINVOU3_ALLOWED_TOOLS` 白名单 + canonical 工具名，部分 builder 方法已更名/移除，
> Plan 模式现注册只读文件/shell 之外的完整只读面）；现行工具面与迁移记录见
> [`tool-governance.md`](./tool-governance.md)，本文请勿按现状使用。
> 2026-11-15 修正：按 v0.8.37 tag 逐 builder 实测校正全部清单与计数（`append_file` 并不存在于
> v0.8.37，系 2026-06-29 fork 提交 `092e3c885` 引入——该提交不在现行主干谱系（对象可在 CodeWhale 子模块按哈希复核、公开仓 Pinvou/CodeWhale 亦可解），属重建前内部 fork；`code_execution` 在 v0.8.37 已是独立
> catalog 工具；`with_shell_tools` 含 exec_shell_wait / exec_shell_interact；v0.8.37 无
> `/tools` slash command；web 工具含 finance 且 web_run 实名 `web.run`；task 全量
> 23 个 / 只读 8 个，github 与 automation 均已注册；子代理仅 agent_open / agent_eval /
> agent_close 三个；`multi_tool_use.parallel` 未注册；MCP 命名为 `mcp_<server>_<tool>`；
> Yolo 注册+子代理约 77（含 catalog 推送约 81，见 §6）、Plan 约 39（关 WebSearch 约 35））。

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
        ├─ MCP tool？  ─▶  McpPool::call_tool
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
- `tools/spec.rs` — `ToolSpec` trait + `ToolContext` + `ToolResult`（`ToolResult` 实定义于 `crates/tools/src/lib.rs:138`，`spec.rs` 经 `pub use` 再导出）
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

`ToolContext`（同文件）携带：workspace 路径、sandbox policy、shell_manager、task_manager、handle_store、rlm_sessions、hook_executor、features、trust_mode、auto_approve 等会话级状态（`approval_mode` 挂在 App 层，不在 `ToolContext`）。

---

## 3. 注册流程

### 3.1 ToolRegistryBuilder（`tools/registry.rs`）

链式调用，每个 `with_*` 方法塞一组相关工具。常见调用单元：

| Builder 方法 | 添加的工具 |
|---|---|
| `with_file_tools` | read_file, write_file, edit_file, list_dir |
| `with_read_only_file_tools` | read_file, list_dir（另注册 retrieve_tool_result） |
| `with_search_tools` | grep_files, file_search |
| `with_web_tools` | web_search, fetch_url, finance, web_run（实名 `web.run`） |
| `with_shell_tools` | exec_shell, exec_shell_wait, exec_shell_interact, exec_shell_cancel（另注册 exec_wait / exec_interact 别名，共 6 个注册） |
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
| `with_runtime_task_tools` | task_create, task_list, task_read, task_cancel, task_gate_run, task_shell_start, task_shell_wait, github_issue_context, github_pr_context, github_comment, github_close_issue, pr_attempt_record, pr_attempt_list, pr_attempt_read, pr_attempt_preflight, automation_create, automation_list, automation_read, automation_update, automation_pause, automation_resume, automation_delete, automation_run（共 23） |
| `with_runtime_read_only_task_tools` | task_list, task_read, github_issue_context, github_pr_context, pr_attempt_list, pr_attempt_read, automation_list, automation_read（共 8） |
| `with_revert_turn_tool` | revert_turn |
| `with_rlm_tool` | rlm_open, rlm_eval, rlm_configure, rlm_close |
| `with_review_tool` | review |
| `with_recall_archive_tool` | recall_archive |
| `with_user_input_tool` | request_user_input |
| `with_parallel_tool` | （空操作 —— v0.8.37 未注册 multi_tool_use.parallel，仅 dispatch 层保留对其的 legacy 解析） |
| `with_note_tool` | note |
| `with_fim_tool` | fim_edit |
| `with_todo_tool` | todo_write, todo_add, todo_update, todo_list, checklist_write, checklist_add, checklist_update, checklist_list（8 个不同名注册） |
| `with_plan_tool` | update_plan |
| `with_subagent_tools` | agent_open, agent_eval, agent_close（仅 3 个） |
| `with_vision_tools` | image_analyze |
| `with_remember_tool` | remember |
| `with_notify_tool` | notify |
| `with_mcp_tools` | （v0.8.37 为 `#[allow(dead_code)]`；MCP 工具实际经 catalog 层注入，见 §5 MCP） |

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
- `Feature::ApplyPatch && !Plan` → `with_patch_tools()` （对 Agent/Yolo 多余 —— `with_agent_tools` 已包含）
- `Feature::WebSearch` → `with_web_tools()` （对 Agent/Yolo 多余；但 Plan 分支没有其它 web 来源，此守卫开关直接增减 Plan 的工具列表）
- `Feature::ShellTool && allow_shell && !Plan` → `with_shell_tools()` （多余）
- `Feature::VisionModel && vision_config.is_some()` → `with_vision_tools()`
- `memory_enabled` → `with_remember_tool()`
- 无条件 → `with_notify_tool()`

**Features 守卫（engine.rs 内，非 tool_setup）**：
- `Feature::Subagents` → `with_subagent_tools(...)` + 启用 fork_context + mailbox
- `Feature::Mcp` → McpPool 工具经 catalog 层合并注入（不注册进 ToolRegistry，与 §5 口径一致）

**关键认知**：对 Agent/Yolo 而言 `Feature::ApplyPatch / WebSearch / ShellTool` 在 registry 层**没有任何效果**——`with_agent_tools` 内部已无条件添加。但 `WebSearch` 是例外中例外：它同时是 Plan 模式 web 工具的唯一开关（Plan 分支无其它 web 来源）。真正能改变 Agent/Yolo 工具列表的只有 `Subagents / Mcp / VisionModel`（外加 `memory_enabled`）。

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
- **串行 vs 并行**：`tool_plan_is_parallel_safe` 判断（`should_parallelize_tool_batch` 为生产谓词 `tool_plan_is_parallel_safe` 的 `#[cfg(test)]` 测试包装，两函数不同名）——实际谓词为 `read_only && supports_parallel && !approval_required && !interactive`（dispatch.rs:274-279；「无副作用冲突」的代码化表述）才并行
- **批处理**：`multi_tool_use.parallel` 是个特殊工具，模型可以用一次调用包多个子调用

### 4.2 单次执行（`tool_execution.rs::execute_tool_with_lock`）

1. 判 MCP 还是 registry tool
2. 抢锁：`supports_parallel` 工具用读锁可共享，写工具独占写锁
3. `InteractiveTerminalGuard` 处理交互式工具（如 shell wait）对 TUI scrollback 的影响
4. 调 `ToolSpec::execute`
5. 记 `tracing` + duration_ms + output_bytes
6. 返回 `ToolResult`

### 4.3 审批门控

每个工具的 `approval_requirement` 决定是否在执行前发 `Event::ApprovalRequired`（2026-11 订正：v0.8.37 门控无 Suggest/Required 行为分层——`turn_loop.rs:1162` 把 requirement 折叠为单一布尔、对一切非 `Auto` 同样发审批事件（实发点 :1585））：
- `Auto` — 直接跑（read_file / git_log / web_search 等）
- `Suggest` / `Required` — 同样发审批事件（write_file / edit_file / exec_shell 等；apply_patch 在 v0.8.37 是显式 `ApprovalRequirement::Suggest` 覆盖，不走 capabilities 推断）；是否弹窗由 UI 侧 `approval_mode` 决定——`suggest`（默认档）弹审批、`auto`（YOLO）全部自动放行、`never` 一律拒绝

`auto_approve: true`（YOLO，即 `approval_mode=auto`）跳过**全部**审批弹窗（`Suggest`/`Required` 一视同仁），非仅「Required」。

### 4.4 大输出处理

超长 tool_result 的处理（v0.8.37）分两层：
- `tools/truncate.rs` — 截断为头部摘要，尾部追加溢出指针（`SPILLOVER_HEAD_BYTES` 头 + 按 `tool_call_id` 回取的尾注）
- `tools/tool_result_retrieval.rs` — 供 `retrieve_tool_result` 按 `tool_call_id` 重取完整历史结果

`tools/large_output_router.rs` 是 workshop/V4-Flash 的合成结果路由（`last_tool_result` + `promote_to_context`），与上述不是同一机制；`handle_read` 属 var_handle 大对象机制，也不承载 tool 输出溢出。

---

## 5. 全部工具清单（按场景分组）

### 文件 IO

| 工具 | 入口 | 用途 |
|---|---|---|
| read_file | with_file_tools | 读文件（支持 chunked / 内嵌 PDF 抽取） |
| write_file | with_file_tools | 写文件（创建或覆盖） |
| edit_file | with_file_tools | 字符串级 patch 编辑 |
| list_dir | with_file_tools | 列目录 |
| apply_patch | with_patch_tools | 类 git diff 风格的多文件 patch |
| revert_turn | with_revert_turn_tool | 撤销本 turn 的所有文件修改（基于 snapshot） |
| fim_edit | with_fim_tool | Fill-in-the-Middle 边写边推理的编辑 |

### 搜索 / 索引

| 工具 | 入口 | 用途 |
|---|---|---|
| grep_files | with_search_tools | 内置 grep/ripgrep 式内容搜索（纯 Rust 自实现，不调外部 rg 进程） |
| file_search | with_search_tools | 按文件名 / 路径搜索 |
| project_map | with_project_tools | 仓库结构概览（生成树状摘要） |
| recall_archive | with_recall_archive_tool | 翻历史 cycle 归档 |

### Shell / 代码执行

| 工具 | 入口 | 用途 |
|---|---|---|
| exec_shell | with_shell_tools | 跑 shell 命令（同步或后台 task） |
| exec_shell_wait / exec_shell_interact（别名 exec_wait / exec_interact） | with_shell_tools | 等待 / 交互式驱动后台 shell task |
| exec_shell_cancel | with_shell_tools | 取消后台 shell task |
| task_shell_start | with_runtime_task_tools | 起后台 shell job |
| task_shell_wait | with_runtime_task_tools | 等后台 shell job 输出（增量） |
| note | with_note_tool | 写 scratch 笔记到 notes 文件 |

> code_execution 是独立注册的 catalog 工具（`core/engine/tool_catalog.rs` 的
> `CODE_EXECUTION_TOOL_NAME`，探测到 Python 解释器且非 Plan 模式才对外声明），并非通过
> `exec_shell` 跑 `python -c` 代替。

### Web

| 工具 | 入口 | 用途 |
|---|---|---|
| web_search | with_web_tools | 关键词搜索（走 search_provider） |
| fetch_url | with_web_tools | 拉单 URL 内容 |
| web_run（实名 `web.run`） | with_web_tools | 浏览器/JS 执行的高级 fetch |
| finance | with_web_tools | 金融/行情数据查询 |

### Git

| 工具 | 入口 | 用途 |
|---|---|---|
| git_status | with_git_tools | working tree 状态 |
| git_diff | with_git_tools | diff |
| git_log | with_git_history_tools | 提交历史 |
| git_show | with_git_history_tools | 单次 commit 详情 |
| git_blame | with_git_history_tools | 行级追责 |

### GitHub

`github_issue_context / github_pr_context / github_comment / github_close_issue` — 经 gh CLI 访问 GitHub API，鉴权依赖 gh 本地登录态、工具 schema 无 token 参数（2026-11-18 r16 订正）。v0.8.37 由 `with_runtime_task_tools` 无条件注册（Agent/Yolo 全量 4 个；Plan 经 `with_runtime_read_only_task_tools` 也有 issue_context / pr_context 两个只读）。

### 任务 / 流程

| 工具 | 入口 | 用途 |
|---|---|---|
| task_create / task_list / task_read / task_cancel | with_runtime_task_tools | 持久化任务管理（与 RLM workflow 是两套机制，rlm_* 见 §5） |
| task_gate_run | with_runtime_task_tools | task 依赖门控 |
| pr_attempt_record / _list / _read / _preflight | with_runtime_task_tools | PR 提交尝试记录（用于自动化迭代） |
| todo_write / todo_add / todo_update / todo_list（另有 checklist_write / checklist_add / checklist_update / checklist_list，8 个不同名注册） | with_todo_tool | 模型自维护 TODO 清单（todo_* 与 checklist_* 两套命名） |
| update_plan | with_plan_tool | Plan 模式下的方案大纲 |
| review | with_review_tool | 自审子流程（模型回看刚做的改动） |
| recall_archive | with_recall_archive_tool | 翻 checkpoint 归档（与上文「搜索 / 索引」表同一注册，勿双计——源码仅注册一次；v0.8.37 口径，该工具在现行基座已整体移除） |

### 用户交互

| 工具 | 入口 | 用途 |
|---|---|---|
| request_user_input | with_user_input_tool | 弹模态框问用户（pinvou3 渲染为内嵌卡片） |
| notify | with_notify_tool | OSC 9 桌面通知 |

> `multi_tool_use.parallel` 在 v0.8.37 **未注册**（`with_parallel_tool` 是空操作）；dispatch 层仅保留对它的 legacy 解析。

### 子代理（experimental）

`agent_open / agent_eval / agent_close`（v0.8.37 仅注册这三个；`agent_result / agent_cancel / agent_list / resume_agent / delegate_to_agent` 等名字在 v0.8.37 源码中已存在、但只是未注册的历史结构体/字面量，后续各版本至现行 HEAD 也未将其注册为工具名——系被移除/替代的遗留，而非「后来才注册」）—— 起独立 sub-agent 跑子任务，需 `Feature::Subagents`。pinvou3 默认开但 UI 不支撑。

### RLM（Recursive Language Model，递归语言模型）持久会话（2026-11-18 订正：原书「Read-Eval-Loop Model」系写档误展开，v0.8.37 源码全部作 Recursive Language Model——`src/rlm/mod.rs:1`、`commands/mod.rs:707`）

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
| diagnostics | with_diagnostics_tool | 环境自检：workspace/git 检测、sandbox 可用性、rust 工具链版本（无文件参数、不走 LSP——v0.8.37 `diagnostics.rs` description 即此；LSP 诊断系 apply_patch 等工具内部 helper）（2026-11-18 r16 订正：原「跑 LSP 拉当前文件诊断」为假） |
| validate_data | with_validation_tools | JSON / TOML schema 校验 |
| handle_read | with_handle_tools | 读 var_handle 大对象 |
| retrieve_tool_result | with_tool_result_retrieval_tool | 按 tool_call_id 回取历史结果 |
| run_tests | with_test_runner_tool | 跑测试套件 |
| load_skill | with_skill_tools | 加载 SKILL.md 引导 |
| remember | with_remember_tool | 写入 user_memory（pinvou3 关闭） |
| automation_create / _list / _read / _update / _pause / _resume / _delete / _run | with_runtime_task_tools | 持久 automation 定义（Agent/Yolo 全量 8 个；Plan 有 list / read 两个只读） |

### MCP（动态）

任何 MCP server 暴露的工具会被注入 catalog，v0.8.37 命名前缀为 `mcp_<server>_<tool>`（单下划线，`mcp.rs:1936`；现行版本模型可见注册名仍为单下划线，双下划线 `mcp__<server>__<tool>` 形式出现在 hooks 条件匹配与 codewhale-mcp 管理面限定名（自由函数 `qualify_tool_name`，`crates/mcp/src/lib.rs`——CodeWhale 子模块代码，本仓无 `crates/` 目录，`McpManager` 内部调用）等非注册名场景，并非注册名改名）。注入路径：`with_mcp_tools` 在 v0.8.37 是 dead_code，实际由 `build_model_tool_catalog` 在 catalog 层完成。pinvou3 当前未连接任何 MCP server。（v0.8.37 时点观察）

---

## 6. pinvou3 实际可见的工具

`build_turn_tool_registry_builder` Yolo 路径展开后约 **74 个不同名注册**（构建器对同名工具按 v0.8.37 覆盖语义去重，末态 74 名，见 §8 overwrite-warn 披露），`engine.rs` 另注入 3 个 `agent_*` 子代理工具，合计约 **77 个**（pandoc/OCR 探测失败时 75；含
exec_wait/exec_interact 两个别名）。catalog 层 `ensure_advanced_tooling`（`turn_loop.rs` 每种模式都会调用——精确条件 `if !tool_catalog.is_empty()`，默认特性下三模式 catalog 均非空）另推送 4 个：code_execution / js_execution（Python/Node 探针门控）与
tool_search_tool_regex / tool_search_tool_bm25（无条件推送，`base.md` 即列有这两个 deferred
discovery 工具），模型可见合计约 **81**（pandoc/OCR 探测失败时约 79，解释器探针亦不可用则约 77）：

```
read_file write_file edit_file list_dir
grep_files file_search
exec_shell exec_shell_wait exec_shell_interact exec_shell_cancel
exec_wait exec_interact note
web_search fetch_url web.run finance
apply_patch revert_turn fim_edit
git_status git_diff git_log git_show git_blame
diagnostics validate_data handle_read retrieve_tool_result
project_map run_tests load_skill
pandoc_convert image_ocr
task_create task_list task_read task_cancel task_gate_run
task_shell_start task_shell_wait
github_issue_context github_pr_context github_comment github_close_issue
pr_attempt_record pr_attempt_list pr_attempt_read pr_attempt_preflight
automation_create automation_list automation_read automation_update
automation_pause automation_resume automation_delete automation_run
todo_write todo_add todo_update todo_list
checklist_write checklist_add checklist_update checklist_list
update_plan
review request_user_input recall_archive
rlm_open rlm_eval rlm_configure rlm_close
notify
agent_open agent_eval agent_close
＋ catalog 推送 4：code_execution js_execution tool_search_tool_regex tool_search_tool_bm25
```

`BASE_PROMPT`（v0.8.37 机制：`include_str!("prompts/base.md")`）只显式提及核心工具（read_file / write_file / edit_file / exec_shell + exec_shell_wait / exec_shell_interact / grep_files / file_search / web_search 等，以及独立的 code_execution Python sandbox 和 tool_search_tool_regex / tool_search_tool_bm25 deferred discovery）。剩余工具靠模型自行从 schema 探索。

Plan 模式约 **39 个工具**（`Feature::WebSearch` 默认开；关闭则约 35）。按 §3.2 组成展开：file 只读 3（含 retrieve_tool_result）、search 2、git 2、git history 3、diagnostics、skill、validation、handle、task 只读 8（task/pr_attempt/automation 各自的 list/read 与 github 的 issue_context/pr_context）、todo 8、plan、review、user_input、recall_archive、notify、web 4。（口径提示：39/35 为注册数；v0.8.37 的 `apply_native_tool_deferral` 对 Plan/Agent 均生效、仅 Yolo 短路不做 deferral——Plan keep-list 收缩到初始可见约 16 个（另有 catalog 恒推的 tool_search 两元工具 defer_loading=false，初始可见实为约 18），其余经 ToolSearch 激活；Agent 预设经同一函数按 keep-list 保留（另含 rlm_*、task_*、shell 家族：task_* 7 个中仅 task_cancel 被 defer；shell 家族 6 个中 exec_shell_cancel 被 defer、5 个保留），初始可见约 29（同理计入 tool_search 两元工具则约 31）；§6 推导的约 81 即为 Yolo 模型可见数。）

---

## 7. 与上游差异 / pinvou3 自定义

pinvou3 不改 ToolRegistry 或 ToolSpec —— 完全复用上游（本节为 v0.8.37 时点口径；现行 fork 已触及 `tools/registry.rs`/`file.rs`/`tool_catalog.rs`，pinvou3-app bridge 侧维护工具 allowlist，见文首注记）。差异只在：

1. **EngineConfig.features**：除 VisionModel（features 表 default_enabled=false）外默认全 on，未自定义裁剪（这是后续可优化点；下条 image_analyze 不注册同因）
2. **memory_enabled = false** → remember 不注册
3. **vision_config = None + Feature::VisionModel = false** → image_analyze 不注册
4. **allow_shell = true** + Yolo 模式 → exec_shell 等可用

详细取舍见内部文档《system-prompt-与底座的差异》（未随本仓库发布）。

---

## 8. 调试 / 诊断手段

观测 tool 行为：

```bash
RUST_LOG=engine.tool_execution=debug ./pinvou3-app/run-dev.sh
```

输出 `tool.exec.start` 和 `tool.exec.end`，含 tool 名、dispatch 路径、耗时、输出字节数、success 标志。

查工具是否注册：v0.8.37 的 registry 构建无逐工具注册日志（仅重名覆盖时 `tracing::warn!`）；可在 engine 调用点断点检查
`ToolRegistry::contains / names / len`，或按上文设置 `RUST_LOG=engine.tool_execution=debug`
观察每次实际执行（`tool.exec.start/end`）反推。注意 v0.8.37 没有 `/tools` slash command（该命令是后来版本加入的 debug
入口，且它展示的是最近一次 Prepared Tool Request 快照而非当前 registry）。

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
