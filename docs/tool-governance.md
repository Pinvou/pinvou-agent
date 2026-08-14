# 工具管理说明（Tool Governance）

本文档描述 pinvou3 当前的工具面（tool surface）管理机制、生效通道与两模式（work / code）下的工具放出情况。
数据来源为当前基线（CodeWhale v0.9.0 fork + pinvou3-app 能力档案机制），随代码演进需同步更新。

---

## 1. 工具面管理机制

工具可见性由 **注册面 → 三层过滤** 决定，最终喂给 LLM 的 catalog 是过滤后的结果。

```
注册面（Agent/Yolo surface，约 113 个原生工具 + MCP + 宿主注入）
        │
        ▼
① 底座编译期黑名单 PINVOU3_HIDDEN_TOOLS（81 个）
   ── catalog 构建时贴 defer 标签 → 模型不可见（唯一放出通道：hidden_tools 注入）
        │
        ▼
② disallowed_tools 通道（exclude / 连接器禁用 / 模式固有隐藏 / 动态隐藏）
   ── catalog 构建后 retain 硬删 → 任意工具名可藏（含黑名单外）
        │
        ▼
模型可见 catalog
```

### 1.1 注册面

Agent / Yolo 模式注册完整工具面（`with_agent_runtime_surface` + `with_subagent_tools`，见 `CodeWhale/crates/tui/src/core/engine.rs` turn registry 构建），再加 pinvou3-app 宿主注入的工具（`kb_search` / `kb_open_source`，见 `pinvou3-app/src-tauri/src/lib.rs` `EngineToolFactory`）。

### 1.2 底座编译期黑名单（唯一"默认隐藏"来源）

- 定义：`CodeWhale/crates/tui/src/tools/pinvou3_blocklist.rs` 的 `PINVOU3_HIDDEN_TOOLS`（81 个工具，按类别分组）。
- 语义：黑名单工具**仍注册**，但 catalog 构建时 `defer_loading = true`，模型默认看不到。
- 判定函数：
  - `is_pinvou3_hidden(name)`：查常量（含 `PINVOU3_BLOCKLIST_OVERRIDE` 测试豁免）。
  - `is_pinvou3_hidden_for_session(name, injected)`：`常量.contains(name) && 注入集.contains(name)`——**注入只能从黑名单中移除（放出），不能把常量外工具纳入隐藏**。
- 守护：`forkguard_blocklist_golden` 金丝雀测试钉死名单精确内容，上游 rebase 后新增/改名/折叠工具必须逐项确认。

### 1.3 能力档案（capability-profiles.json）

- 文件：`pinvou3-app/src-tauri/resources/common/capability-profiles.json`（**编译内嵌 JSON**，设计期产物，运行期不变）。
- 结构：`plain` / `code` 两个条目，每个含 `tools: { base, exclude, include }`。
- 语义：**基础集 + 差量**——上游新增工具仍被底座常量挡住，模式只表达差异。
- 解析：`capability_profile.rs` → `SessionPolicy::resolve()` 统一产出两通道差量，外部消费者不再 if 分流。

### 1.4 双通道生效语义

| 通道 | 数据来源 | 方向 | 可操作范围 | 生效时机 |
|---|---|---|---|---|
| **hidden_tools（include）** | 档案 `tools.include` | 从黑名单**放出**（hidden = 常量 − include） | 仅黑名单内工具 | **仅 respawn**（catalog 构建时定型，无热刷） |
| **disallowed_tools（exclude）** | 档案 `tools.exclude` + 连接器禁用 + 模式固有 + 动态判定 | 在可见集上**再藏**（硬删） | **任意工具名**（支持 `*` 前缀批量，大小写不敏感） | **下轮请求生效**（有热刷 `refresh_disallowed_tools`） |

- include 为空时 `hidden_tools` 注入 `None` → 底座回退常量，与历史行为逐字节等价（plain 零影响）。
- 两通道叠加顺序：include 在 catalog 构建时贴/反转 defer 标签，disallowed 在构建后硬删——**deny 恒优先，矛盾配置下 exclude 赢**。
- 注入点：`bridge.rs::build_engine_config_for_session_at`（hidden_tools）与 `shape_disallowed_tools`（disallowed）。

### 1.5 动态层（运行期判定，非档案）

| 维度 | 机制 | 说明 |
|---|---|---|
| 连接器 scope | `marketplace::ConnectorScope` | plain / code 两个 scope 各自持久化禁用集；code 会话换用 Code scope（`shape_disallowed_tools` 替换） |
| `load_skill` | 组合目录空否（`session_skills_is_empty`） | **仅 code 模式**：组合目录为空（无启用技能）→ 隐藏，避免"开关开着但没技能"的假状态；plain 恒可见 |
| `kb_search` / `kb_open_source` | `ToolPolicy`（app 侧） | 知识库无索引内容或语义引擎未就绪 → 禁用（两模式一致） |
| shell 工具 | `allow_shell` | shell 组仅在会话允许时注册（pinvou3 GUI 恒开） |
| `mcp_pinvou3_present_artifact` | `extra_hidden_tools`（模式固有） | **仅 code 模式**恒藏（原生代码会话无产物卡语义）；走 disallowed 通道 |
| 市场 MCP 工具 / IMA | 用户安装启用 | 启用后以 `mcp_<server>_<tool>` / `ima_openapi` 形态暴露，禁用则进 disallowed |
| CLI 连接器（飞书/企微/钉钉/腾讯会议） | scope 门禁（`disabled_connectors.json`） | 配套技能走 companion 联动排除（`companion_skills` 查能力包注册表静态表）；code scope 未初始化默认全关（含 CLI id）；连接成功视同新装，已初始化 code scope 自动保持关 |

### 1.5.1 CLI 连接器的三通道生效（同一 scope 禁用集派生）

CLI 连接器（`bundle::BUILTIN_CLI_BUNDLES` 登记 `cli` 二进制 + 配套技能目录，能力包注册表为单一真相源）被禁后，三个执行通道同时生效，数据全部来自该 scope 的 `disabled_connectors.json`：

| 通道 | 机制 | 生效时机 |
|---|---|---|
| ① 技能可见性（软门控） | companion 联动 → 组合目录排除（`skill_materialization::disabled_skill_names_for`） | 下轮 prompt |
| ② 全局停用（marker，独立一层） | `feishu_disabled` 等 marker 文件 → 删除 `bundle/skills/` 技能文件，全模式不可见 | 立即（删文件） |
| ③ CLI 二进制硬拦截 | execpolicy deny 规则（`bridge::cli_deny_ruleset` → spawn 注入 + `engine_pool::refresh_permission_rulesets` 热刷），spawn 前硬拒、全模式含 YOLO、链式/wrapper 形态覆盖 | 下轮请求 |

- ②与①③叠加取交集：marker 停用时 scope 开关无法单独放出（技能文件已删）。
- UI 语义：composer 工具菜单 CLI 行 = 「已连接」徽章（连接状态，只读）+ 开关胶囊（该 scope 门禁，可写）。

### 1.6 安全边界（任何通道都绕不过）

- **`tool_search` 永不注入**：catalog 注入 gate 恒查编译期常量（`is_pinvou3_hidden(TOOL_SEARCH_NAME)`），不查注入集——模型无法搜索/激活任何被藏工具，黑名单是硬边界。
- **`request_user_input` / `read_file` 等名单外工具不可被 hidden_tools 隐藏**：`is_pinvou3_hidden_for_session` 对常量外名称恒返回 false。
- ⚠️ 注意：**disallowed 通道没有上述限制**——exclude 可以藏任意工具（含 `request_user_input`），产品层需自备核心工具守卫（现役 `lib.rs` 断言只守护黑名单）。

---

## 2. 当前工具放出清单

> 计算方式：注册面 − 底座黑名单 + include 放出 − disallowed（档案 exclude / 连接器禁用 / 模式固有 / 动态）。

### 2.1 work（plain）模式——恒可见 31 个

> 口径说明：清单按 **Yolo 模式**（GUI work 会话默认）统计，即注册面非黑名单全可见；`terminal/*` 随 `allow_shell`（GUI 恒开）注册。`checklist_*`（write/add/update/list）为 legacy 别名，`model_visible()=false` 恒不对模型暴露，故不计入。

| # | 工具 | 类别 | 功能 |
|---|---|---|---|
| 1 | `read_file` | 文件 | 读 UTF-8 文件（PDF 自动抽取、pages 切片） |
| 2 | `write_file` | 文件 | 创建/覆盖文件 |
| 3 | `append_file` | 文件 | 追加内容，输出内联 diff |
| 4 | `edit_file` | 文件 | 单文件搜索替换 |
| 5 | `list_dir` | 文件 | 结构化、gitignore 感知列目录 |
| 6 | `grep_files` | 搜索 | 纯 Rust 正则全文搜索 |
| 7 | `file_search` | 搜索 | 文件名模糊匹配 |
| 8 | `web_search` | Web | DuckDuckGo 默认搜索（可配 Bing/Tavily 等） |
| 9 | `fetch_url` | Web | 已知 URL 直接抓取，HTML 转文本 |
| 10 | `wait_for_dev_server` | Web | 轮询本地端口/健康地址等 dev server 就绪（随 WebSearch feature 注册） |
| 11 | `exec_shell` | Shell | 前台命令（限单行，超时杀进程并提示转后台）（`allow_shell` 时） |
| 12 | `exec_shell_wait` | Shell | 轮询后台任务增量输出 |
| 13 | `terminal/run` | Shell | 持久 PTY 会话前台命令（cd/export/函数跨调用保留）（随 `allow_shell` 注册） |
| 14 | `terminal/send` | Shell | 向持久会话发送原始输入（含 ETX 中断） |
| 15 | `terminal/wait` | Shell | 等待前台命令并取缓冲输出 |
| 16 | `terminal/cancel` | Shell | ETX 中断前台命令（会话保留可复用） |
| 17 | `terminal/reset` | Shell | 重建会话（丢失运行中工作，保留历史摘要） |
| 18 | `work_update` | 进度 | 工作进度正式记账（canonical） |
| 19 | `update_plan` | 计划 | 策略级 PlanArtifact（阶段上下文/路径规划） |
| 20 | `request_user_input` | 交互 | 回合中气泡提问（硬保留） |
| 21 | `load_skill` | 技能 | 按 id 加载已安装 skill |
| 22 | `revert_turn` | 回滚 | 回滚工作区到回合前快照（需审批） |
| 23 | `image_analyze` | 视觉 | 视觉模型读用户附图 |
| 24 | `agent` | 子代理 | 创建入口：启动后台子代理，返回 agent_id（上限 20 并发） |
| 25 | `agents/list` | 子代理 | 列出子代理：id、层级、状态、进度、预算 |
| 26 | `agents/message` | 子代理 | 给子代理发消息 |
| 27 | `agents/followup` | 子代理 | 追问/续跑子代理 |
| 28 | `agents/interrupt` | 子代理 | 中断子代理（含 fail-closed 自中断保护） |
| 29 | `agents/wait` | 子代理 | 等待子代理完成（默认 5 分钟，可调至 30 分钟） |
| 30 | `workflow` | 工作流 | 运行工作流脚本（QuickJS） |
| 31 | `mcp_pinvou3_present_artifact` | 产物 | 产出物卡片（bundle 内置 MCP server，work 恒可见；code 模式经 extra_hidden 恒藏，见 §2.2） |

### 2.2 code 模式——在 work 基础上

| 变化 | 工具 | 通道 | 说明 |
|---|---|---|---|
| ➕ 放出 | `git_status` | include（hidden_tools） | git 域：查看 repo 状态（分支/脏净/未提交） |
| ➕ 放出 | `git_diff` | include（hidden_tools） | git 域：查看工作区/暂存区差异 |
| ➕ 放出 | `git_log` | include（hidden_tools） | git 域：查看提交历史 |
| ➕ 放出 | `git_show` | include（hidden_tools） | git 域：查看单次提交详情 |
| ➕ 放出 | `git_blame` | include（hidden_tools） | git 域：查看行级归属 |
| ➕ 放出 | `apply_patch` | include（hidden_tools） | 修改域：多 hunk/多文件事务式 diff 应用（git 看差异 → 事务改 → git_diff 验证） |
| ➕ 放出 | `run_verifiers` | include（hidden_tools） | 验证域：多生态并行验证门禁（改完验证闭环） |
| ➕ 放出 | `exec_shell_cancel` | include（hidden_tools） | Shell 后台域：取消后台任务（补取消闭环） |
| ➖ 隐藏 | `mcp_pinvou3_present_artifact` | extra_hidden（disallowed） | 原生代码会话无产物卡语义，恒藏 |
| ➖ 动态 | `load_skill` | 组合目录空否 | 该会话组合目录为空（无启用技能）→ 隐藏 |
| 🔄 替换 | 连接器禁用集 | Code scope | 与 plain 各自持久化，互不影响 |

code 模式恒可见（Yolo） = 31 − 1 + 8 = **38 个**（−1：`mcp_pinvou3_present_artifact` 经 extra_hidden 恒藏；不含动态/条件项），动态项按会话实际状态取并集。

> ⚠️ **mode 前提（重要）**：上述「8 工具放出」在 **Yolo 模式**下成立（GUI work 会话默认 Yolo；code 会话需用户切 Yolo 后同样成立）。**code 会话默认 Plan（只读）**（`last_mode` 无记录时默认 Plan，`reconcile_code_default_modes` 强制回 Plan）时，Plan 注册面**不含** `apply_patch` / `run_verifiers`（无 `with_patch_tools` / `with_test_runner_tool`），且 `shell_policy=None` 不注册 `exec_shell_cancel`；`git_blame` 虽注册但不在上游 allowlist（`DEFAULT_ACTIVE_NATIVE_TOOLS`），被 defer——即默认 Plan 下 8 个 include 实际仅 `git_status`/`git_diff`/`git_log`/`git_show` 4 个可见。若需默认即全量放出，须在产品层决策（如调整 Plan 注册面 / 上游 allowlist），本文档仅如实记录现状。

### 2.3 条件性工具（不恒可见，按配置/用户启用状态）

| 工具 | 条件 |
|---|---|
| `kb_search` / `kb_open_source` | 知识库有索引内容且语义引擎就绪（两模式一致；宿主注入） |
| `ima_openapi` | 用户启用 IMA 连接器并配置凭据；未启用进 disallowed |
| 市场 MCP 工具（`weather` / `iwencai` / `qcc` 等） | 用户安装并启用后，以 `mcp_<server>_<tool>` 形态暴露 |

### 2.4 两模式对比速览

| 维度 | work（plain） | code |
|---|---|---|
| include（放出） | 无 | git 域 5 个 + `apply_patch` + `run_verifiers` + `exec_shell_cancel`（共 8 个） |
| exclude（档案再藏） | 空 | 空（v1） |
| 模式固有隐藏 | 无 | `mcp_pinvou3_present_artifact` |
| `load_skill` | 恒可见 | 组合目录为空 → 隐藏 |
| 连接器禁用集 | Plain scope | Code scope |

---

## 3. 隐藏工具全清单（81 个，逐行）

> 状态列标记该工具**当前在 code 模式是否已启用（放出）**。本期档案 include 放出 8 个（git 域 5 个 + `apply_patch`/`run_verifiers`/`exec_shell_cancel`）；其余 73 个两模式均保持隐藏。
> 黑名单内工具仍注册（可被 `tool_search` 理论激活，但 pinvou3 不注入 tool_search，实际不可达）。

| 状态 | # | 工具 | 类别 | 功能（替代/影响） |
|---|---|---|---|---|
| ❌ 未启用 | 1 | `task_create` | 持久任务 | 创建持久后台任务——长任务无法持久化，中断/重启即丢失 |
| ❌ 未启用 | 2 | `task_list` | 持久任务 | 列出持久任务 |
| ❌ 未启用 | 3 | `task_read` | 持久任务 | 读任务详情（时间线/证据/产物） |
| ❌ 未启用 | 4 | `task_cancel` | 持久任务 | 取消任务（需审批） |
| ❌ 未启用 | 5 | `task_gate_run` | 持久任务 | 跑验证命令并挂结构化证据 |
| ❌ 未启用 | 6 | `task_shell_start` | 持久任务 | 后台启动长命令——长命令只能前台跑或放弃 |
| ❌ 未启用 | 7 | `task_shell_wait` | 持久任务 | 轮询后台命令 + 挂 gate 证据 |
| ❌ 未启用 | 8 | `pr_attempt_record` | PR 跟踪 | 记录尝试 diff + patch 产物——多次方案尝试无法存档对比回放 |
| ❌ 未启用 | 9 | `pr_attempt_list` | PR 跟踪 | 列出尝试记录 |
| ❌ 未启用 | 10 | `pr_attempt_read` | PR 跟踪 | 查看单条尝试 |
| ❌ 未启用 | 11 | `pr_attempt_preflight` | PR 跟踪 | `git apply --check` 预检 |
| ❌ 未启用 | 12 | `tool_agent` | 子代理旧入口 | 实验性子代理——由 `agent` + `agents/*` 新表面取代（零影响） |
| ❌ 未启用 | 13 | `agent_spawn` | 子代理旧入口 | 旧 spawn 入口 |
| ❌ 未启用 | 14 | `agent_result` | 子代理旧入口 | 旧收结果入口 |
| ❌ 未启用 | 15 | `agent_cancel` | 子代理旧入口 | 旧取消入口 |
| ❌ 未启用 | 16 | `agent_list` | 子代理旧入口 | 旧列表入口 |
| ❌ 未启用 | 17 | `resume_agent` | 子代理旧入口 | 旧续跑入口 |
| ❌ 未启用 | 18 | `delegate_to_agent` | 子代理旧入口 | 旧委托入口 |
| ❌ 未启用 | 19 | `rlm_open` | RLM | 打开持久 Python REPL——无持久 REPL，数据分析退化为一次性脚本 |
| ❌ 未启用 | 20 | `rlm_eval` | RLM | 有界 Python 执行 + 16 路并行批量查询 |
| ❌ 未启用 | 21 | `rlm_configure` | RLM | 调整会话策略 |
| ❌ 未启用 | 22 | `rlm_close` | RLM | 关闭 REPL 并返回统计 |
| ❌ 未启用 | 23 | `create_goal` | 目标 | 创建运行时目标——无目标跟踪护栏，长对话目标易漂移 |
| ❌ 未启用 | 24 | `get_goal` | 目标 | 查目标状态（预算/时长/阻塞） |
| ❌ 未启用 | 25 | `update_goal` | 目标 | 更新完成门禁 |
| ✅ 已启用 | 26 | `git_status` | Git | 查看 repo 状态（分支/脏净/未提交）——**code 模式经 include 放出** |
| ✅ 已启用 | 27 | `git_diff` | Git | 查看工作区/暂存区差异——**code 模式经 include 放出** |
| ✅ 已启用 | 28 | `git_log` | Git | 查看提交历史——**code 模式经 include 放出** |
| ✅ 已启用 | 29 | `git_show` | Git | 查看单次提交详情——**code 模式经 include 放出** |
| ✅ 已启用 | 30 | `git_blame` | Git | 查看行级归属——**code 模式经 include 放出** |
| ✅ 已启用 | 31 | `apply_patch` | Patch/FIM | 多 hunk/多文件事务式修改——**code 模式经 include 放出** |
| ❌ 未启用 | 32 | `fim_edit` | Patch/FIM | FIM 中间补全式编辑 |
| ❌ 未启用 | 33 | `pandoc_convert` | 附件预处理 | 文档格式互转——转换不可用，docx 只能读二进制乱码 |
| ❌ 未启用 | 34 | `image_ocr` | 附件预处理 | 本地 OCR——截图文字只能靠视觉模型读 |
| ❌ 未启用 | 35 | `todo_write` | todo 别名 | 旧清单别名——由 `checklist_*` / `work_update` 完全等价（零影响） |
| ❌ 未启用 | 36 | `todo_add` | todo 别名 | 旧清单别名（零影响） |
| ❌ 未启用 | 37 | `todo_update` | todo 别名 | 旧清单别名（零影响） |
| ❌ 未启用 | 38 | `todo_list` | todo 别名 | 旧清单别名（零影响） |
| ✅ 已启用 | 39 | `exec_shell_cancel` | Shell 后台 | 取消后台任务——**code 模式经 include 放出** |
| ❌ 未启用 | 40 | `exec_shell_interact` | Shell 后台 | 向后台进程写 stdin——无法与交互式进程对话 |
| ❌ 未启用 | 41 | `exec_wait` | Shell 后台 | 旧别名（仅回放） |
| ❌ 未启用 | 42 | `exec_interact` | Shell 后台 | 旧别名（仅回放） |
| ❌ 未启用 | 43 | `automation_create` | Automation | 创建定时自动化——模型无法建立定时任务 |
| ❌ 未启用 | 44 | `automation_delete` | Automation | 删除自动化 |
| ❌ 未启用 | 45 | `automation_list` | Automation | 列出自动化 |
| ❌ 未启用 | 46 | `automation_pause` | Automation | 暂停自动化 |
| ❌ 未启用 | 47 | `automation_read` | Automation | 读自动化详情 |
| ❌ 未启用 | 48 | `automation_resume` | Automation | 恢复自动化 |
| ❌ 未启用 | 49 | `automation_run` | Automation | 立即触发一次 |
| ❌ 未启用 | 50 | `automation_update` | Automation | 更新自动化 |
| ❌ 未启用 | 51 | `github_issue_context` | GitHub | 只读 issue 上下文——只能裸 `exec_shell gh`，无审批门控/证据约束 |
| ❌ 未启用 | 52 | `github_pr_context` | GitHub | 只读 PR 上下文 |
| ❌ 未启用 | 53 | `github_comment` | GitHub | 审批门控评论 |
| ❌ 未启用 | 54 | `github_close_issue` | GitHub | 审批门控关 issue（需验收证据） |
| ❌ 未启用 | 55 | `finance` | 杂项 | 行情/股票数据——只能 `web_search` 兜底 |
| ❌ 未启用 | 56 | `web.run` | 杂项 | 浏览器自动化（JS 渲染/表单）——无法操作动态网页 |
| ❌ 未启用 | 57 | `diagnostics` | 元工具 | 环境体检——代码自检降级为手动 shell |
| ❌ 未启用 | 58 | `multi_tool_use.parallel` | 元工具 | 并行元工具——注册即 no-op（DeepSeek-v4 原生并行），零影响 |
| ❌ 未启用 | 59 | `note` | 元工具 | 一次性事实——跨轮记事实靠对话本身，压缩后易丢 |
| ❌ 未启用 | 60 | `validate_data` | 元工具 | JSON/TOML schema 校验——配置校验不可用 |
| ❌ 未启用 | 61 | `run_tests` | 元工具 | `cargo test`——测试退化为手动 `exec_shell` |
| ❌ 未启用 | 62 | `handle_read` | 元工具 | 大输出有界投影——大输出全量进上下文，token 膨胀 |
| ❌ 未启用 | 63 | `retrieve_tool_result` | 元工具 | 旧输出摘要/切片——同上 |
| ❌ 未启用 | 64 | `project_map` | 元工具 | 项目结构地图——陌生代码库导航降级 |
| ❌ 未启用 | 65 | `recall_archive` | 元工具 | 归档会话检索——无跨会话记忆 |
| ❌ 未启用 | 66 | `review` | 元工具 | 结构化代码评审——评审能力不可用 |
| ❌ 未启用 | 67 | `notify` | 元工具 | 桌面通知——无任务完成通知 |
| ❌ 未启用 | 68 | `remember` | 元工具 | 跨会话记忆写入——偏好每次重述 |
| ❌ 未启用 | 69 | `web_run` | 杂项 | `web.run` 旧名（与 56 同源） |
| ❌ 未启用 | 70 | `speech` | 语音 | 语音合成——模型无 TTS 输出（app 输入侧 ASR 仍在） |
| ❌ 未启用 | 71 | `tts` | 语音 | 语音合成（同 70） |
| ❌ 未启用 | 72 | `rlm_session_objects` | RLM | 会话对象卡片（补漏）——随 RLM 组一起隐藏 |
| ❌ 未启用 | 73 | `github_close_pr` | GitHub | 审批门控关 PR（补漏）——随 GitHub 组一起隐藏 |
| ✅ 已启用 | 74 | `run_verifiers` | 元工具 | 多项目并行验证门禁——**code 模式经 include 放出** |
| ❌ 未启用 | 75 | `slop_ledger_append` | 反 slop | 反 slop 内部账本——底座内部机制，零用户影响 |
| ❌ 未启用 | 76 | `slop_ledger_export` | 反 slop | 账本导出（零影响） |
| ❌ 未启用 | 77 | `slop_ledger_query` | 反 slop | 账本查询（零影响） |
| ❌ 未启用 | 78 | `slop_ledger_update` | 反 slop | 账本更新（零影响） |
| ❌ 未启用 | 79 | `tool_search` | tool_search | 搜索并激活 deferred 工具——**不注入**；模型无任何途径激活被藏工具（治理硬边界） |
| ❌ 未启用 | 80 | `tool_search_tool_regex` | tool_search | 旧双名（前向兼容占位） |
| ❌ 未启用 | 81 | `tool_search_tool_bm25` | tool_search | 旧双名（前向兼容占位） |

---

## 4. 工具面变更操作指引

### 4.1 放出黑名单内工具（如 git_diff）

1. 在 `capability-profiles.json` 对应模式的 `tools.include` 追加工具名（只能引用黑名单内名称）。
2. **respawn 生效**：include 无热刷通道，已开会话需重建引擎（重开/切换会话）。
3. 同步更新测试断言：`capability_profile.rs`、`session_policy.rs`（如 `tool_include` 精确断言）。
4. 建议按"能力域 + 行为验证"节奏（先例：本期放出 git 域 5 个 + 修改/验证/后台取消 3 个，GitHub 域待 gh 前置验证）。

### 4.2 隐藏任意已可见工具（含已放出/黑名单外）

1. 在 `capability-profiles.json` 对应模式的 `tools.exclude` 追加工具名，支持 `*` 前缀批量（如 `agents/*`）。
2. **下轮请求生效**：disallowed 有热刷（`refresh_disallowed_tools`），无需 respawn。
3. ⚠️ 核心工具（`read_file` / `request_user_input` / `exec_shell` 等）可被 exclude 硬删但无天然防护——需产品层守卫（现役 `lib.rs` 核心断言只守护黑名单，exclude 是覆盖盲区）。
4. ⚠️ 矛盾配置（同一工具同时 include + exclude）exclude 赢（deny 恒优先），建议解析期校验拒绝。

### 4.3 变更验证

- 单元：`cargo test -p codewhale-tui --lib forkguard_`（黑名单金丝雀）+ app 侧档案断言。
- 行为：respawn 后调 `/tools`（或等价目录）核对模型可见 catalog；验证放出/隐藏后对应工具可调/不可调。
- 上游 sync：rebase 后跑黑名单漂移检测，新增工具默认进黑名单再评估。

---

## 5. 相关代码索引

| 组件 | 位置 |
|---|---|
| 底座黑名单常量 | `CodeWhale/crates/tui/src/tools/pinvou3_blocklist.rs` |
| catalog defer 注入 | `CodeWhale/crates/tui/src/core/engine/tool_catalog.rs` |
| hidden_tools 字段 | `CodeWhale/crates/tui/src/core/engine.rs` |
| disallowed 硬删 | `CodeWhale/crates/tui/src/core/engine.rs::filter_tool_catalog_for_gates` |
| disallowed 匹配（`*` 前缀） | `CodeWhale/crates/tui/src/core/engine/turn_loop.rs::command_denies_tool` |
| 能力档案 | `pinvou3-app/src-tauri/resources/common/capability-profiles.json` |
| 档案解析 | `pinvou3-app/src-tauri/src/features/assistant/capability_profile.rs` |
| 会话策略统一解析 | `pinvou3-app/src-tauri/src/features/assistant/session_policy.rs` |
| disallowed 整形 | `pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs::shape_disallowed_tools` |
| hidden_tools 注入 | `pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs::build_engine_config_for_session_at` |
| 核心工具可见断言 | `pinvou3-app/src-tauri/src/lib.rs` |
| 工具注册面 | `CodeWhale/crates/tui/src/tools/registry.rs`、`core/engine/tool_setup.rs` |
| CLI 连接器登记表（二进制 + 配套技能，单一真相源） | `pinvou3-app/src-tauri/src/features/marketplace/bundle.rs::BUILTIN_CLI_BUNDLES` |
| CLI companion 排除 | `pinvou3-app/src-tauri/src/features/marketplace/mod.rs::companion_skills` |
| CLI 硬拦截规则集 | `pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs::cli_deny_ruleset` |
| CLI 硬拦截热刷 | `pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs::refresh_permission_rulesets` |
| execpolicy deny 执行 | `CodeWhale/crates/tui/src/core/engine/turn_loop.rs::exec_shell_ask_rule_decision` |

---

## 7. 变更历史

> 每次工具面变更在此登记：日期 + 本次 PR 提交名，便于追溯"哪个能力在哪个版本/哪个提交放出"。

| 日期 | 变更内容 | PR 提交名 |
|---|---|---|
| 2026-08-14 | CLI 连接器纳入 scope 门禁：能力包注册表登记配套技能（单一真相源）、companion 联动排除覆盖 CLI、code scope 默认全关含 CLI id、execpolicy deny 硬拦截 CLI 二进制；composer 工具菜单 CLI 行改「已连接」徽章 + 开关胶囊。**行为变更**：已初始化 code 开关的用户，已连接 CLI 连接器在 code scope 收紧为默认关 | `feat(marketplace): CLI 连接器纳入 scope 门禁统一治理` |
| 2026-08-10 | code 档案放出 **git 域 5 个**（git_status/git_diff/git_log/git_show/git_blame）——状态/差异/历史/单次/行归属完整认知线 | `feat(codex): code 模式能力档案放出 8 工具 + 工具管理文档`（与下行合并提交） |
| 2026-08-10 | code 档案追加 **修改/验证/后台取消 3 个**（apply_patch 事务修改、run_verifiers 验证闭环、exec_shell_cancel 后台取消）——code 会话闭环补齐（读/改/验证/后台） | 同上（合并提交） |

---

## 6. 已知问题与待办

### 6.1 fork 侧注释过时（暂不改动，仅本文档标注）

能力档案放出 git 域后，fork（CodeWhale submodule）内有 2 处注释与现状不符。**决定：fork 侧暂不改代码**（待 fork 侧流程处理），当前状态以本文档为准：

| 位置 | 过时表述 | 现状 |
|---|---|---|
| `CodeWhale/crates/tui/src/tools/pinvou3_blocklist.rs` git 组注释（"…与 log/show/blame 一致**全隐藏**;需要走 exec_shell git"） | 全隐藏 | 仅 plain 模式成立；code 模式经档案 include 放出 8 个工具（黑名单是**默认**隐藏集，可被会话注入收窄） |
| `CodeWhale/crates/tui/src/core/engine/tests.rs` `forkguard_hidden_tools_injectable` 注释（"04 PR-E：首个 include 只放 git_status，其余仍隐藏"） | 注入集模拟内容与档案一致 | 仅验证 hidden_tools 注入**机制**，不代表当前档案内容（git_status/git_log 现均被 code 档案放出） |

**处理时机**：下次 fork 侧流程（sync/改动登记）时一并修正，不影响运行行为。
