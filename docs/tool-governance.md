# 工具管理说明（Tool Governance）

本文档描述 pinvou3 当前的工具面（tool surface）管理机制、生效通道与两模式（work / code）下的工具放出情况。
数据来源为当前基线（CodeWhale v0.9.5 fork + pinvou3-app 能力档案机制），随代码演进需同步更新。

---

## 1. 工具面管理机制

v0.9.5 起，模型侧工具面是 **canonical action family**：单一工具名（`Bash` / `File` / `Git` / `Web` / `Run` 等）+ 必填 `action` 参数；旧工具名（`exec_shell` / `read_file` / `git_status` / `work_update` 等）已整体退役（见 §3 对照表）。可见性由 **注册面 → 白名单/禁用收窄 → deferred/tool_search** 决定：

```
注册面（Plan / Yolo 各自注册一批 canonical 家族 + MCP + 宿主注入）
        │
        ▼
① allowed_tools 白名单（app 侧 PINVOU3_ALLOWED_TOOLS，canonical 家族粒度）
   ── ToolSurfacePolicy 构建 catalog 时 retain 收窄；同时约束首轮目录、
      tool_search 结果与实际执行（执行门先查 deny 再查 allow，deny 优先）
        │
        ▼
② disallowed_tools 通道（档案 exclude / extra_hidden / 连接器禁用 / 动态隐藏）
   ── 与白名单同一份 retain + 执行门；任意工具名可藏（含白名单内）
        │
        ▼
③ deferred + tool_search：注册且过白名单但不在 DEFAULT_ACTIVE_NATIVE_TOOLS
   的工具首回合 deferred，模型经 tool_search 搜索激活；白名单外搜不到
        │
        ▼
模型可见 catalog
```

### 1.1 注册面（按 mode 分化）

由底座 `build_turn_tool_registry_builder_for_route` 按 mode 构建（`CodeWhale/crates/tui/src/core/engine/tool_setup.rs`）：

- **Yolo / Agent**：完整面——`File`（含 write/edit/patch）、`Bash`、`Git`、`Run`、`Web`、`tasks` / `github` / `automation` / `rlm` 等 durable 家族、`agent`、`workflow`、`revert_turn`、`todo_write`、`request_user_input` 等。Agent 与 Yolo 注册面相同，差异在审批姿态与 MCP 工具 deferral（Yolo 不 defer MCP 工具）。
- **Plan（只读）**：`File` 只读实例（仅 read/list/search_name/search_content）、`Git`（5 个 action 本就只读，全量注册）、`todo_write`、`request_user_input`、`load_skill` 等；**不注册 `Bash` / `Run` / `File` 写与 patch**（`shell_policy_for_mode(Plan)=None`）。

### 1.2 Pinvou 白名单（唯一"默认可见"来源）

- 定义：`pinvou3-app/src-tauri/src/features/assistant/tool_policy.rs` 的 `PINVOU3_ALLOWED_TOOLS`。
- 语义：与底座 `allowed_tools` 一致——规则与工具名均转小写，尾部 `*` 为前缀匹配，否则精确匹配；**按 catalog 工具名匹配**，写 `"Git"` 即放行整个 Git 家族，无 family.action 语法。
- 内容：`Bash` / `File` / `Git` / `Web` / `agent` / `load_skill` / `request_user_input` / `revert_turn` / `todo_write` / `workflow` / `tool_search` / `image_analyze` / `kb_search` / `kb_open_source` / `mcp_*` / `list_mcp_resources` / `list_mcp_resource_templates` / `read_mcp_resource`。
- 未放出的底座家族：`Run` / `tasks` / `automation` / `github` / `rlm` 等（`lib.rs` 契约测试反向断言钉死）。
- `PINVOU3_ALWAYS_LOADED_TOOLS`（`request_user_input` / `image_analyze`）：首轮直接可见、不依赖 tool_search 激活。
- 守护：`lib.rs::tool_allowlist_contract` 契约测试钉死白名单正反断言；`tests/canonical_tool_contract.test.js` 保证运行时指导只教 canonical 家族、不教退役名。

### 1.3 能力档案（capability-profiles.json）

- 文件：`pinvou3-app/src-tauri/resources/common/capability-profiles.json`（**编译内嵌 JSON**，设计期产物，运行期不变）。
- 结构：`plain` / `code` 两个条目，每个含 `tools: { exclude, extra_hidden }` 与 `connectors: { scope }`。
- 语义：**基础集 + 差量**——基础集由白名单承担（v0.9.5 起不再有底座侧黑名单/放出通道），档案只声明模式差量：
  - `exclude`：基础集上再藏（可变策略，下轮生效）——"该模式还不想要什么"；
  - `extra_hidden`：模式固有隐藏——"该模式不可能有什么"（恒定，不可被用户开关覆盖）；
  - `connectors.scope`：该模式连接器禁用集取哪个 scope（plain/code 各自持久化）。
- 解析：`capability_profile.rs` → `SessionPolicy::resolve()` 纯数据投影，外部消费者（`shape_disallowed_tools` 等）统一走 resolve，不再 if 分流。

### 1.4 disallowed_tools 通道

| 维度 | 说明 |
|---|---|
| 数据来源 | 档案 `exclude` + `extra_hidden` + 连接器禁用集（按 scope）+ 动态判定（load_skill / kb_*） |
| 方向 | 在可见集上**再藏**（catalog retain 硬删 + 执行门拒绝） |
| 可操作范围 | **任意工具名**（支持 `*` 前缀批量，大小写不敏感；写家族名整族隐藏） |
| 生效时机 | **下轮请求生效**——app 侧 `engine_pool.refresh_disallowed_tools()` / `set_disallowed_all` 经 `Op::SetDisallowedTools` 热刷 |

- 注入点：`bridge.rs::shape_disallowed_tools`（spawn 初值与全局热刷都经此整形，按会话策略差量驱动）。
- 与白名单叠加：deny 恒优先——白名单外本不可见；白名单内被 disallowed 命中则隐藏。

### 1.5 动态层（运行期判定，非档案）

| 维度 | 机制 | 说明 |
|---|---|---|
| 连接器 scope | `marketplace::ConnectorScope` | plain / code 两个 scope 各自持久化禁用集；code 会话换用 Code scope（`shape_disallowed_tools` 替换） |
| `load_skill` | 组合目录空否（`session_skills_is_empty`） | **仅 code 模式**：组合目录为空（无启用技能）→ 隐藏，避免"开关开着但没技能"的假状态；plain 恒可见 |
| `kb_search` / `kb_open_source` | 知识库就绪否 | 宿主按会话注入（`engine.rs` Agentic RAG）；知识库无索引内容或语义引擎未就绪 → 进 disallowed（两模式一致） |
| `Bash` | mode 注册面 | Plan 不注册；Yolo 全量（GUI 会话 shell 恒开） |
| `mcp_pinvou3_present_artifact` | `extra_hidden`（模式固有） | **仅 code 模式**恒藏（原生代码会话无产物卡语义） |
| 市场 MCP 工具 / IMA | 用户安装启用 | 启用后以 `mcp_<server>_<tool>` / `ima_openapi` 形态暴露（受 `mcp_*` 白名单放行），禁用则进 disallowed |

### 1.6 安全边界

- **白名单是硬边界**：`allowed_tools` 同时约束首轮目录、`tool_search` 结果与执行门——白名单外工具模型既看不到、搜不到、也调不动。v0.9.0 时代的底座编译期黑名单（`pinvou3_blocklist.rs` / `PINVOU3_HIDDEN_TOOLS` / `hidden_tools` 注入）已整体删除，不再存在"从黑名单放出"的通道。
- ⚠️ **disallowed 通道无名单限制**——exclude 可以藏任意工具（含 `request_user_input` 等核心工具），产品层需自审档案差量；`lib.rs` 契约测试只守护白名单内容，不守护 exclude 误伤。

---

## 2. 当前工具面清单

> 计算方式：注册面 ∩ 白名单 − disallowed（档案 exclude / extra_hidden / 连接器禁用 / 动态）。deferred 工具（白名单内但非默认 active）经 tool_search 激活后同样可用，下表不区分 active/deferred。

### 2.1 两模式共用基础面（Yolo，白名单内）

| 工具 | action / 形态 | 功能 |
|---|---|---|
| `Bash` | run / wait / interact / cancel（后台为 run 的 `background` 参数） | 前台/后台命令、轮询、交互、取消 |
| `File` | read / list / search_name / search_content / write / edit / patch | 读、列目录、文件名/内容搜索、写、搜索替换、事务式补丁 |
| `Git` | status / diff / log / show / blame（全只读） | 仓库状态、差异、历史、提交详情、行级归属 |
| `Web` | search / fetch / wait | 搜索、URL 抓取、等 dev server 就绪 |
| `agent` | 子代理入口 | 启动/管理后台子代理 |
| `workflow` | QuickJS 工作流 | 运行工作流脚本 |
| `todo_write` | 整表替换 todos | 工作进度记账 / Plan 方案步骤（方案卡由 engine 监听其结果触发） |
| `load_skill` | 按 id 加载 | 加载已安装 skill |
| `request_user_input` | 回合中提问 | 气泡提问（硬保留，ALWAYS_LOADED） |
| `revert_turn` | 回滚 | 回滚工作区到回合前快照（需审批） |
| `image_analyze` | 视觉 | 视觉模型读用户附图（ALWAYS_LOADED） |
| `tool_search` | query/match/max_results | 搜索并激活 deferred 工具（白名单内） |
| `kb_search` / `kb_open_source` | 宿主注入 | 知识库检索 / 打开来源（就绪才可见，见 §1.5） |
| `mcp_*` / `list_mcp_resources` / `list_mcp_resource_templates` / `read_mcp_resource` | 动态 | 已启用连接器工具与 MCP 资源 |
| `mcp_pinvou3_present_artifact` | bundle 内置 MCP | 产物卡（仅 work，见 §2.2） |

### 2.2 plain（work）vs code 差量

| 变化 | 工具 | 通道 | 说明 |
|---|---|---|---|
| ➖ plain 隐藏 | `Git`（整族） | 档案 `exclude`（disallowed） | 普通工作会话不放 git 结构化能力 |
| ➖ code 隐藏 | `mcp_pinvou3_present_artifact` | 档案 `extra_hidden`（disallowed） | 原生代码会话无产物卡语义，恒藏 |
| ➖ code 动态 | `load_skill` | 组合目录空否 | 该会话无启用技能 → 隐藏 |
| 🔄 code 替换 | 连接器禁用集 | Code scope | 与 plain 各自持久化，互不影响 |

### 2.3 Plan / Yolo 前提

- code 会话默认 Plan（只读）：`Git` 全 5 action 可用（只读）；`File` 仅只读 action；**无 `Bash`、无写/patch**。
- 切到 Yolo 后：`File` write/edit/patch 与 `Bash`（含后台与取消）放出，读/改/验证/后台闭环中除验证（`Run` 未放出，见 §2.4）外齐备。
- work 会话默认 Yolo，基础面即 §2.1（差量见 §2.2）。

### 2.4 白名单外（两模式均不可见）

| 家族 / 名称 | 状态 | 影响与替代 |
|---|---|---|
| `Run`（tests / verifiers） | 未放出 | 验证退化为 `Bash` 手动跑测试/检查；如需放出，白名单加 `"Run"`（见 §4.1） |
| `tasks` / `automation` / `github` / `rlm` | 未放出 | 持久任务、定时自动化、GitHub 门控、持久 REPL 均不可用 |
| 其余上游家族（fim / speech / lsp / pandoc / image_ocr / finance / notify 等） | 未放出 | 各 feature/opt-in 门内工具，白名单外不可达 |

---

## 3. 旧工具名 → v0.9.5 对照

v0.9.5 起旧名分两类归宿，**任何配置（白名单 / exclude / 文档 / 运行时指导）都不应再写旧名**：

| 旧名 | 归宿 |
|---|---|
| `read_file` / `write_file` / `edit_file` / `list_dir` / `file_search` / `grep_files` | 删除 → `File` 对应 action（read / write / edit / list / search_name / search_content） |
| `exec_shell` / `exec_shell_wait` / `exec_shell_interact` / `exec_shell_cancel` / `exec_wait` / `exec_interact` | 删除 → `Bash`（run / wait / interact / cancel，后台为 run 的 `background` 参数） |
| `git_status` / `git_diff` / `git_log` / `git_show` / `git_blame` | 删除 → `Git` 同名 action |
| `web_search` / `fetch_url` / `wait_for_dev_server` | 删除 → `Web`（search / fetch / wait） |
| `run_tests` / `run_verifiers` | 删除 → `Run`（tests / verifiers；**当前未放出**） |
| `apply_patch` | 隐藏 replay 别名（`model_visible=false`）→ 模型侧用 `File` action=patch |
| `work_update` / `checklist_write` / `checklist_update` / `update_plan` / `todo` / `TodoWrite` | 隐藏 replay 别名 → 模型侧用 canonical `todo_write` |
| `task_*` / `pr_attempt_*` / `github_*` / `automation_*` 旧名 | 删除 → `tasks` / `github` / `automation` 家族（**当前未放出**） |

> #238 放出的 8 个工具在新底座的归宿：git 域 5 个 → `Git` 家族（两 mode 注册，plain 经档案 exclude 隐藏）；`apply_patch` → `File` patch（Yolo）；`exec_shell_cancel` → `Bash` cancel（Yolo）；`run_verifiers` → `Run` verifiers（未放出）。原 include 通道（hidden_tools 注入）随底座黑名单一并删除，不再存在。

---

## 4. 工具面变更操作指引

### 4.1 放出一个白名单外家族（如 Run）

1. 在 `tool_policy.rs::PINVOU3_ALLOWED_TOOLS` 追加家族名（canonical 名，不是旧名）。
2. 同步 `lib.rs::tool_allowlist_contract` 契约断言（正/反向清单）。
3. 生效时机：白名单在 engine spawn 时定型，已开会话需重建引擎（重开/切换会话）。
4. 若需首轮直接可见（不依赖 tool_search），追加 `PINVOU3_ALWAYS_LOADED_TOOLS`。

### 4.2 按模式隐藏工具（含白名单内）

1. 在 `capability-profiles.json` 对应模式的 `tools.exclude` 追加工具名——**用 catalog 名（家族名或 MCP 全名）**，支持 `*` 前缀批量，大小写不敏感；模式固有、恒定不可覆盖的隐藏用 `tools.extra_hidden`。
2. **下轮请求生效**：disallowed 有热刷（`refresh_disallowed_tools` → `Op::SetDisallowedTools`），无需 respawn。
3. ⚠️ exclude 可藏任意工具（含核心工具），无天然防护，需自审；同步 `capability_profile.rs` / `session_policy.rs` 断言。

### 4.3 变更验证

- 单元：`cargo test --locked --lib`（`tool_allowlist_contract`、`capability_profile`、`session_policy`、`shape_disallowed_tools` 断言）+ `npm test`（`canonical_tool_contract.test.js` 等）。
- 行为：respawn 后核对模型可见 catalog；放出/隐藏后对应工具可调/不可调。
- fork 侧：涉及 fork-distinct 行为时更新 `docs/fork-modifications.md` 与指纹，跑 `./scripts/fork-guard.sh --fast`。
- 上游 sync：rebase 后核对 canonical 家族/action 漂移（`canonical_action.rs` 别名表与契约测试是锚点）。

---

## 5. 相关代码索引

| 组件 | 位置 |
|---|---|
| Pinvou 白名单 / ALWAYS_LOADED | `pinvou3-app/src-tauri/src/features/assistant/tool_policy.rs` |
| 白名单契约测试 | `pinvou3-app/src-tauri/src/lib.rs::tool_allowlist_contract` |
| 退役名/指导契约测试 | `pinvou3-app/tests/canonical_tool_contract.test.js` |
| 能力档案 | `pinvou3-app/src-tauri/resources/common/capability-profiles.json` |
| 档案解析 | `pinvou3-app/src-tauri/src/features/assistant/capability_profile.rs` |
| 会话策略统一解析 | `pinvou3-app/src-tauri/src/features/assistant/session_policy.rs` |
| disallowed 整形 | `pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs::shape_disallowed_tools` |
| disallowed 热刷 | `pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs::refresh_disallowed_tools` → `Op::SetDisallowedTools` |
| kb_* 会话注入与门控 | `pinvou3-app/src-tauri/src/features/assistant/engine.rs`（Agentic RAG 注入） |
| 底座 allowed/disallowed 匹配与 catalog 收窄 | `CodeWhale/crates/tui/src/core/engine/tool_catalog.rs`（`ToolSurfacePolicy::new` / `tool_matches_any_rule`） |
| 底座执行门（deny 优先） | `CodeWhale/crates/tui/src/core/engine/turn_loop.rs` |
| 底座注册面（Plan/Yolo 分化） | `CodeWhale/crates/tui/src/core/engine/tool_setup.rs`、`tools/registry.rs` |
| canonical 家族与旧名别名表 | `CodeWhale/crates/tui/src/tools/canonical_action.rs` |
| tool_search（deferred 激活） | `CodeWhale/crates/tui/src/core/engine/tool_catalog.rs`（`execute_tool_search`） |

---

## 6. 变更历史

> 每次工具面变更在此登记：日期 + 本次 PR 提交名，便于追溯"哪个能力在哪个版本/哪个提交放出"。

| 日期 | 变更内容 | PR 提交名 |
|---|---|---|
| 2026-08-10 | code 档案放出 **git 域 5 个**（git_status/git_diff/git_log/git_show/git_blame）——状态/差异/历史/单次/行归属完整认知线 | `feat(codex): code 模式能力档案放出 8 工具 + 工具管理文档`（与下行合并提交） |
| 2026-08-10 | code 档案追加 **修改/验证/后台取消 3 个**（apply_patch 事务修改、run_verifiers 验证闭环、exec_shell_cancel 后台取消）——code 会话闭环补齐（读/改/验证/后台） | 同上（合并提交） |
| 2026-08-11 | **底座升级 CodeWhale v0.9.5**：工具面迁移 canonical 家族——底座黑名单与 hidden_tools/include 通道删除，档案收敛为 exclude/extra_hidden 纯差量；git 域由 `Git` 家族承担（plain 经 exclude 整族隐藏），`apply_patch`→`File` patch、`exec_shell_cancel`→`Bash` cancel（均 Yolo），`run_verifiers`→`Run` verifiers（未放出）；进度工具收敛 canonical `todo_write` | `feat(engine): 升级 CodeWhale 至 v0.9.5` (#231)，本文档同步重写 |
