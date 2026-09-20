# 工具管理说明（Tool Governance）

本文档描述 pinvou3 在 **CodeWhale v0.9.12** 基线上的模型工具治理（子模块随 main 持续演进，以仓库当前 gitlink 为准；v0.9.12 首次落地时的 gitlink 见 §8 迁移记录）。v0.9.12 以 canonical model-visible tool name 作为模型侧工具面，由 `allowed_tools` 正向白名单同时约束首轮目录、`tool_search` 结果和实际执行；v0.9.0 的 `PINVOU3_HIDDEN_TOOLS`、`hidden_tools` 注入和逐个旧工具名放出机制已经退役。v0.9.5 → v0.9.12 的迁移过程见 `docs/codewhale-upgrade-0.9.5-to-0.9.12.md`。

> **沿革**：`docs/tool-governance.md` 这一文件名曾在 #287 中被删除，本文是其继任者（后随通用文档回流恢复并按 v0.9.12 工具面重写）。它与被删除的旧文档不是同一份内容，旧外链请勿按其内容理解本文。

## 1. 当前治理链路

模型最终可见且可执行的工具由以下约束共同决定：

```text
CodeWhale 按 Plan / Agent / Operate 注册 canonical 工具（v0.9.12 模式集，crates/config/src/app_mode.rs）
        ↓
Pinvou allowed_tools 正向白名单（硬边界）
        ↓
CodeWhale 首轮加载 / tool_search 延迟加载策略
        ↓
Pinvou disallowed_tools（模式、连接器、技能空态等动态拒绝）
        ↓
单轮工具策略（例如 restricted turn 经 TurnToolSecurityPolicy 收敛到零 dynamic tools）
```

- `allowed_tools` 同时约束首轮目录、`tool_search` 结果和实际执行，不在白名单中的工具不能通过搜索重新激活。
- `disallowed_tools` 只能继续收窄白名单；模式差量、连接器停用集和运行期空态均走这条通道，下轮请求生效。
- Plan / Agent / Operate 的注册面和 sandbox 仍由 CodeWhale 决定。白名单允许一个工具或工具族，不代表每种模式都注册其全部写操作。
- `request_user_input` 与 `image_analyze` 由 `PINVOU3_ALWAYS_LOADED_TOOLS` 要求首轮直接可见；其他允许工具可按底座策略延迟加载。

关键实现：

| 职责 | 文件 |
|---|---|
| Pinvou 正向白名单 | `pinvou3-app/src-tauri/src/features/assistant/tool_policy.rs` |
| 白名单正反向测试 | `pinvou3-app/src-tauri/src/lib.rs`（`mod tool_allowlist_contract`，约 :1650-1811）、`pinvou3-app/src-tauri/src/features/assistant/multiagent_regression_tests.rs`（真实 bridge 回合目录逐项校验，约 :535） |
| 模式能力静态表 | `pinvou3-app/src-tauri/src/features/assistant/session_policy.rs` 的 `MODE_TABLE` |
| 模式与动态拒绝整形 | `pinvou3-app/src-tauri/src/features/assistant/session_policy.rs`、`features/assistant/platform/bridge.rs::shape_disallowed_tools` |
| 能力包与 scope 治理 | `docs/capability-governance.md` |
| 底座注册、过滤与执行 | `CodeWhale/crates/tui/src/core/engine.rs`、`CodeWhale/crates/tui/src/tools/registry.rs` 及工具注册模块 |

## 2. Pinvou 正向白名单

`PINVOU3_ALLOWED_TOOLS`（`tool_policy.rs:20-49`）当前允许 28 项，与 v0.9.12 模型可见目录逐字对应：

| 类别 | 名称 | 说明 |
|---|---|---|
| 文件与前台 shell 小契约 | `bash`、`read`、`write`、`edit` | 文件读写与前台 shell 已从 v0.9.x action family 迁出为小写小契约工具 |
| 文件发现与内容检索 | `list_dir`、`file_search`、`grep_files` | v0.9.12 重新作为独立 canonical 工具发布（见 §3） |
| 保留的 action family | `Git`、`Web` | 仍以 `action` 参数分派子动作（`status`/`diff`/…、`search`/`fetch`） |
| 持久终端 | `terminal/run`、`terminal/send`、`terminal/wait`、`terminal/cancel`、`terminal/reset` | 精确名五件套，原因见下 |
| 协作与工作流 | `agent`、`workflow` | 子 agent 与工作流入口 |
| 交互与状态 | `request_user_input`、`revert_turn`、`todo_write` | `todo_write` 是唯一模型可见的 canonical 进度工具 |
| 技能与发现 | `load_skill`、`tool_search` | 仍受技能组合目录与底座延迟加载规则约束 |
| 视觉与知识 | `image_analyze`、`kb_search`、`kb_open_source` | 知识工具还受索引/语义引擎就绪状态约束 |
| MCP | `mcp_*`、`list_mcp_resources`、`list_mcp_resource_templates`、`read_mcp_resource` | `mcp_*` 是前缀规则；具体连接器停用集走 `disallowed_tools` |

匹配规则（`is_pinvou3_allowed`，`tool_policy.rs:63-70`）与 CodeWhale `allowed_tools` 语义一致：名称大小写不敏感，规则尾部 `*` 表示前缀匹配。由此：

- `mcp_*` 放行全部标准 `mcp_` 命名空间的动态 MCP 工具；具体工具名由已启用连接器发现，连接器开关仍通过 `disallowed_tools` 施加更窄的拒绝。
- 持久终端五个工具使用精确名而非 `terminal/*`：避免未来新增的执行原语仅凭共享前缀就自动穿透产品白名单（`tool_policy.rs:13-15` 注释；反向测试断言 `terminal/future-capability` 被拒，lib.rs）。
- 大小写不敏感使隐藏回放名 `Bash` 能命中 `bash` 规则、在历史回放中继续可执行（新会话目录只教 canonical `bash`）；`File` 没有任何可命中的规则、不可执行（lib.rs `tool_allowlist_contract` 的对应断言）。

`PINVOU3_ALWAYS_LOADED_TOOLS = ["request_user_input", "image_analyze"]`（`tool_policy.rs:52`）要求这两个工具首轮直接可见，不能依赖模型先调用 `tool_search`。

新增白名单项必须同步更新白名单正反向测试（lib.rs `tool_allowlist_contract`、`multiagent_regression_tests.rs`）和 fork guard。

## 3. canonical 工具面与隐藏兼容名

v0.9.12 的模型侧名称以 model-visible canonical 名称为准（§2 白名单与之逐字一致）：

| 模型侧名称 | 形态 | 说明 |
|---|---|---|
| `bash`、`read`、`write`、`edit` | 独立小契约工具 | 文件与前台 shell 从 v0.9.x action family 迁出；`write` 单次内容硬上限 64 KiB（`CodeWhale/crates/tui/src/tools/file_tool.rs` 工具描述与 `file/tests.rs` 边界测试） |
| `list_dir`、`file_search`、`grep_files` | 独立小契约工具 | 重新作为独立 canonical 工具发布（`canonical_action.rs:209` "published standalone tools again"），不再是 `File` 家族的子动作 |
| `Git`、`Web` | action family | 子动作经 `action` 参数分派，别名表见 `CANONICAL_ACTION_ALIASES`（`canonical_action.rs:19`） |
| `terminal/run` 等 | 持久终端工具 | 精确名五件套，见 §2 |
| `todo_write` | 进度工具 | 唯一模型可见的 canonical 进度名 |

其余模型可见工具（`agent`、`workflow`、`load_skill`、`tool_search`、`request_user_input`、`revert_turn`、`image_analyze`、`kb_search`、`kb_open_source`、MCP 资源工具等）直接使用各自名称，无家族/别名分化。

**隐藏 replay 兼容名**（`model_visible=false`，不进模型目录，不得写入产品白名单、提示词或新文档）：

| 隐藏名 | 回放行为 | 依据 |
|---|---|---|
| `Bash`、`File` | 保存的 v0.9.x 转录回放仍可 dispatch；经产品白名单时 `Bash` 命中 `bash` 规则可执行，`File` 无规则不可执行 | `HIDDEN_COMPAT_TOOL_NAMES = ["Bash", "File"]`（`canonical_action.rs:237`）；lib.rs `tool_allowlist_contract` |
| `read_file`、`write_file`、`edit_file`、`git_status`/`git_diff`/`git_log`/`git_show`/`git_blame`、`exec_shell*`、`web_search`、`fetch_url`、`wait_for_dev_server`、`run_tests`、`run_verifiers` 等 | 拼写已彻底移除，`ToolRegistry::resolve` 无模糊解析，完全不能 dispatch | `RETIRED_TOOL_NAMES`（`canonical_action.rs:211`） |
| `work_update`、`TodoWrite`、`todo`、`checklist_write`、`checklist_update`、`update_plan` | 进度工具的隐藏 replay 别名；写进白名单只是死条目，且会让进度工具整体不可见 | `tool_policy.rs:17-19`；`registry.rs:1344-1360`（`update_plan` 经 `with_plan_tool` 另行注册，约 :1365）；lib.rs 反向测试 |

兼容解析：`CANONICAL_ACTION_ALIASES` 把旧名 + `action`（如 `File{read}`、`Bash{run}`）解析到现行工具，实时调用与保存的历史转录得到相同的下游行为；前端渲染器同时识别旧名与 canonical 名，以保证历史会话可读。

## 4. work / code 模式差量

模式差量由编译期静态表 `MODE_TABLE` 声明（`session_policy.rs:58-75`），不再维护运行时能力档案：

| 维度 | work（plain） | code |
|---|---|---|
| 原生工具 | 公共白名单（含 `Git` family） | 公共白名单（含 `Git` family） |
| `MODE_TABLE.unavailable_tools` | 空 | `mcp_pinvou3_present_artifact` |
| 能力包默认策略 | AllowAll | DenyAll |
| 连接器/技能禁用集 | `plain` scope | `code` scope |
| `load_skill` | 按正常技能状态 | 会话组合技能目录为空时隐藏 |
| 项目规则与代码层指令 | 不绑定 | 绑定项目并使用 code instructions |

因此，#238 在 v0.9.0 上“逐个 include 8 个旧工具名”的产品目标，由公共正向白名单承接；那批旧名在 v0.9.12 已是隐藏 replay 别名或彻底移除的拼写（见 §3）。底座能力是产品承诺，plain 与 code 均允许 `Git` family；两种模式的外部能力风险姿态则由各自 scope 的默认策略和用户选择控制。

code 会话默认 Plan 时，最终可用 action 仍受 Plan 注册面与只读 sandbox 限制；需要写入/执行面时由用户显式提升运行档位与授权。v0.9.12 的模式集是 Plan / Agent / Operate，旧的 Yolo 不再是独立模式，bypass 姿态改由权限面承载（`crates/config/src/app_mode.rs`）。产品层不得用白名单绕过底座模式安全边界。

## 5. 动态拒绝与优先级

`shape_disallowed_tools`（`features/assistant/platform/bridge.rs:655-698`）在持久禁用集上按以下顺序叠加：

1. 当前模式 `MODE_TABLE` 的 `unavailable_tools`（先并入；缺席名单不得与连接器禁用全名重叠，否则会被后续 retain 误删——`bridge.rs:658-660` 顺序约束）；
2. 对应模式 scope 的连接器禁用工具（非 plain 模式先剔除 plain scope 禁用集、再并入本 scope 禁用集）；
3. 当前模式要求且技能组合目录为空时的 `load_skill`（表字段 `skills_empty_hides_load_skill` 门控）；
4. 知识库、会话或产品状态产生的其他动态拒绝项。

同一名称重复加入时会去重。允许与拒绝冲突时拒绝优先（deny 优先于 allow）；restricted turn 经 `TurnToolSecurityPolicy` 随消息下发，可进一步收敛到零 dynamic tools（见 `docs/codewhale-upgrade-0.9.5-to-0.9.12.md` §4）。

## 6. 修改流程

### 放出或新增工具

1. 确认 CodeWhale 当前模式确实注册该 canonical 名称/action，并核对 sandbox 与审批语义。
2. 在 `PINVOU3_ALLOWED_TOOLS` 添加精确名称或最窄前缀；不要添加隐藏 replay 别名（写进白名单只是死条目）。
3. 若某模式架构上不提供该工具，在 `MODE_TABLE` 对应行的 `unavailable_tools` 中声明；用户可选的外部能力应走能力包 scope，不得混入模式身份。
4. 同步工具策略、模式静态表、前端渲染与提示词契约测试。
5. 更新本文和 `scripts/fork-guard.sh`，运行完整 fork guard 与 app 测试。

### 隐藏工具

1. 全产品禁止：从正向白名单移除。
2. 按模式禁止：仅架构缺席项写入 `MODE_TABLE.unavailable_tools`。
3. 按连接器、技能或运行状态禁止：通过 scope 状态和 `disallowed_tools` 动态计算，不修改模式静态表。
4. 不得仅隐藏 UI；执行层必须同步不可达。

## 7. 安全不变量

- 白名单是产品级放行边界；不引入第二套 fork-only 黑名单，不恢复 `hidden_tools` 注入（`tool_policy.rs` 头注释）。
- `tool_search` 只能发现白名单内且未被拒绝的工具：`allowed_tools` 同时约束首轮目录、`tool_search` 结果和实际执行（`tool_policy.rs:3-5`；lib.rs `conditional_allowlist_rules_match_model_visible_registry_entries` 以 v0.9.12 真实 registry 投影验证条件性宿主/MCP 条目均有白名单规则）。
- canonical `write` 单次内容有 64 KiB 硬上限（`CodeWhale/crates/tui/src/tools/file_tool.rs:121`；`file/tests.rs` 断言超限写入在触盘前失败），并继续受工作区边界和审批/claim 约束。
- canonical `bash` 执行前经命令安全分析（`CodeWhale/crates/tui/src/command_safety.rs`：命令前缀分类与危险模式预检）；破坏性命令不能被自动批准绕过。
- 宿主额外工具经 `spawn_for_session` 的单一注入路径进入引擎（`engine_pool.rs` 的 `extra_tools` 组装与 `AppEngine::spawn_for_session` 调用，约 :1299-1339；如 `kb_tool.rs` 经 `EngineConfig.extra_tools` 注入），并继续受白名单（lib.rs、`multiagent_regression_tests.rs` 对真实 bridge 回合目录的逐项校验）和拒绝集约束。
- 宿主产生的结构化产出与审计文件写声明过的位置（会话执行根与应用审计根，见 `SessionStore::session_roots` 与 `bridge.rs::audit_workspace`）；工具不应借 canonical `write`/`edit` 扩大权限。原「三省六部」结构化产出子系统已随 bundle 0.21 完整移除（`runtime_bundle/platform/mod.rs` 变更记录），不再是安全边界的承载者。

## 8. 迁移记录

| 日期 | 变更 |
|---|---|
| 2026-08-10 | v0.9.0 能力档案曾为 code 模式从底座隐藏集放出 5 个 Git 工具及 `apply_patch`、`run_verifiers`、`exec_shell_cancel`。 |
| 2026-08-11 | 升级 CodeWhale v0.9.5：切换到 canonical family、`allowed_tools` 正向白名单和单一 `disallowed_tools` 差量通道；旧 8 项 include 被等价迁移，不再作为模型侧工具名维护。 |
| 2026-08-14 | 退役无运行时写入者的能力档案，模式架构差量收敛为 `MODE_TABLE`；plain 放开 `Git` family，外部能力默认姿态改由按模式 scope 管理。 |
| 2026-09-09 | 升级 CodeWhale v0.9.12（gitlink `pinvou-v0.9.12-r1-18-g92427bd8d`）：产品工具面迁移到 model-visible canonical 名称——`bash`/`read`/`write`/`edit` 小契约、`list_dir`/`file_search`/`grep_files` 重新独立发布、`terminal/*` 精确名五件套；`Bash`/`File` 退为隐藏 replay 兼容名；白名单与正反向测试同步重写。见 `docs/codewhale-upgrade-0.9.5-to-0.9.12.md`（§4"主应用改造"）。 |

每次 CodeWhale 升级都必须重新核对 canonical 名称、各模式注册面、首轮/延迟加载策略、前端历史回放兼容和本文档；不能只凭旧工具名相同就判定行为等价。
