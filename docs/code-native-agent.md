# 代码模块品悟原生会话（code-native-agent）技术文档

> 分支：`feat/code-native-agent`（PR #138，已 rebase 至最新 `main`）
> 周期：2026-08-01 单日完成，共 19 个开发节点（squash 为 PR 的 6 个提交，见 §4）。
> 关联文档：`codex-acp.md`（使用与验证）、`multi-agent-acp.md`（多 agent 单一真相源）、`code-mode-解耦与权限持久化-改动说明.md`（模式解耦、能力回补与 mode 持久化的合并留档）；原 `Codex-ACP-整体架构决策.md` 的决策变更记录由本文第 3 节承接（该文档已随主线清理下线）。

## 1. 背景与目标

"代码"模块此前只接入三个第三方 ACP Agent（Codex / Claude Code / Kimi），原生能力缺位。本特性把 pinvou 原生（CodeWhale Engine）编码能力作为第四个 agent"品悟"（agent_id `pinvou`）加入代码模块，目标：

- 代码模式下可直接创建品悟原生会话并对话，支持绑定用户自选项目目录；
- 不依赖第三方 CLI 安装与登录，无 Node/无账号环境可用；
- ACP 会话行为零变化；普通聊天页行为零变化；
- 提示词与编码场景对齐，避免工作模式语义污染代码任务。

## 2. 架构总览

两条 agent 链路在同一展示层汇合：

| | ACP 链路（既有） | 原生链路（本特性） |
|---|---|---|
| Agent 进程 | 独立子进程（ACP stdio JSON-RPC） | 进程内 CodeWhale Engine（Rust 库） |
| 会话池 | `AcpPool` | `EnginePool` |
| 发消息命令 | `codex_acp_prompt` | `chat`（显式 sessionId） |
| 事件流 | `acp:event` | `chat:*`（前端订阅清单 `NATIVE_CHAT_EVENTS` 共 18 项，R-1/R-3 增补后） |
| 前端投影 | `projectAcpTimeline` | `code-native-lane.js` → `projectDeepSeekConversation` |
| 展示层 | `ConversationTimeline`（共用） | 同左 |
| 会话存储 | `SessionStore` + `SessionAgentStore` | 同左（`SessionMode` 模式标记，见 §3.1） |

关键共用：`SessionStore`（`~/.pinvou3/sessions/`）、`SessionAgentStore`（`~/.pinvou3/session-agents.json`）、统一会话列表、工作区面板、附件体系、i18n。

## 3. 核心设计决策

### 3.1 会话标记而非新链路

- `AgentBackend` 枚举复用既有原生变体 `Deepseek`（display_name "品悟"），`parse` 增加 `"pinvou"` 别名，kebab-case 序列化契约不变。
- 会话类型按**两条正交轴**类型化（2026-08-05 解耦，D-1）：产品模式轴 `SessionAgentRecord.mode: SessionMode::{Plain, Code}`（`core/session_mode.rs`），运行时轴即 `AgentBackend`（Deepseek=原生、其余=ACP）——替代原 `code_session: bool` 布尔约定（互斥曾靠调用顺序维持）。磁盘 `session-agents.json` 保持原 `code_session` 键与布尔格式（自研 serde 适配层），新旧版本互读兼容、零迁移；普通会话无记录默认 `Plain`。
- 原生代码会话对 `AcpPool::is_acp` 保持 false：`chat` 命令天然放行，ACP 命令经 `acp_record` 检查自然拒绝，两链路互不串台。
- "会话创建时绑定 Agent/目录，开始后不可切换"对原生与 ACP 同样生效。

### 3.2 两个根：执行根 ≠ 账本根

绑项目目录的核心安全设计：

- **执行根**（LLM 面向）：engine cwd、文件工具相对路径根、shell 执行目录。经 `Pinvou3Bridge` 注入的执行根解析器（共享 AcpPool 那份 `SessionAgentStore` 实例）推导；engine spawn（`engine.rs`）与 shell_workspace（`engine_pool.rs`）两个消费点经 `bridge.session_workspace` 自动同源。
- **账本根**（应用面向）：附件、暂存、审计、产物、远程授权，恒为 `~/.pinvou3/sessions/<sid>/`（`SessionStore::execution_workspace` **一行未改**，25+ 调用点全是账本语义）。CodeWhale delegated-agent 的账本、transcript 与协调锁通过 `subagent_state_root` 使用该会话的 `workspace/`；Pinvou 专家由全局专家池生成内存 `fleet.profiles`，不写入账本根。
- 效果：项目目录零污染（无 `attachments/`、`.pinvou3/`、`workflow_audit.jsonl` 或 Pinvou 专家名册写入）、删会话零残留、远程下载授权面不扩大、产物面板不误扫项目。
- 例外适配：附件引用在绑项目会话改用绝对路径（相对路径会相对项目根解析落空）。

### 3.3 前端双车道而非复用 bridge

bridge 的 chat 状态机绑定单一 activeSession，代码页与主聊天并存会串台。改为每会话一个本地 lane（纯数据模块 `code-native-lane.js`，可单测）：消费 `chat:*` 事件、从 SavedSession + `timing_events.jsonl` 还原历史、乐观气泡 + 失败回滚 + 回声去重。确认卡（`chat:user_input_required`）用显式 sessionId 调 `submit_user_input`/`cancel_user_input`，不经 bridge。

### 3.4 提示词分层（共享层 + 模式层）

- 原唯一 prompt 来源 `bundle/instructions.md` 拆为 `instructions-shared.md`（身份/底线/工具纪律/红线/输出，骨架含两个模式层占位行）与 `instructions-work.md`（产出物面板/tmp 语义、present_artifact 条）。
- 代码会话 = 骨架 + `instructions-code.md`（代码身份头、工作区段含项目路径占位符、代码场景纪律：就地修改/不建 tmp//git 纪律/范围纪律/验证纪律/交付汇报）+ **底座 `CORE_EXECUTION_PROFILE_PROMPT` 原样引用**（Rust 渲染层拼接 `deepseek_tui::prompts::CORE_EXECUTION_PROFILE_PROMPT`，零文本复制，上游更新自动跟随）。
- 未引入底座 `BASE_PROMPT`（含 "You are Codewhale" 身份；其权威顺序/防编造干货在共享层 §底线已有等价物）。
- 工作会话渲染结果与拆分前**逐字节相等**（golden 测试内嵌 33 行原文快照断言）。
- 工具整形：代码会话 `disallowed_tools` 追加 `mcp_pinvou3_present_artifact`（恒隐藏，code 无产物卡语义）；`load_skill` 按**该会话组合目录是否为空**动态决定（skill 双 scope 治理后，非空 → 放行；空 → 隐藏，避免"开关开着但没技能"的假状态）。spawn 初值与 `set_disallowed_all` 热刷统一经 `bridge.shape_disallowed_tools`（策略取值见 §3.6，目录判定见 §8.6）。

### 3.5 配置控件复用策略

聊天页输入框底栏四类常驻控件（Plan/Yolo、模型、工具菜单、知识库挂载）搬入代码模块原生车道，模型选择列表中的多智能体会话开关一并复用，视觉对齐 ACP 配置组（`CodexComposerConfigSelect` pill 形态）。关键约束：bridge 的 models/knowledge/interaction 方法绑聊天 active 且草稿态会物化聊天会话，代码车道一律直调 per-session Tauri 命令显式传 sessionId；草稿态选择暂存，建会话后按序应用（model → kb → mode → multi-agent），任一步失败都中止首发并显式报错。

### 3.6 模式策略对象（2026-08-05 解耦，D-2/D-3）

- `SessionPolicy`（`features/assistant/session_policy.rs`）把 plain/code 的行为差异收敛为数据：模式能力差量查编译期静态表 `MODE_TABLE`（`unavailable_tools`——code 恒追加 `present_artifact`；`load_skill` 不在此列——按组合目录空否动态决定，由表字段 `skills_empty_hides_load_skill` 驱动，见 §8.6；连接器禁用集 scope 即 `policy.mode()` 本身，scope 键 = 模式名）、`plan_reminder()`（两模式同文，R-1 审批卡落地后为真实描述）、`approval_params()`（本期两模式同为全自动+Auto，S-1 安全分化的挂载点）。能力面分化是编译期常量（能力档案 JSON + 统一解析器已退役，见 `docs/capability-governance.md`）。
- 共享链路按策略取数：`shape_disallowed_tools` 与 `build_send_message_op`（新增 `session_id` 参数）不再散 `is_code_session` 裸判断；统一查询入口为 `SessionAgentStore::session_mode()` 与 `Pinvou3Bridge::session_policy()`。
- 效果：改一个模式的策略取值不经过另一个模式的代码路径；新增模式取值时编译器强制审查分支。详细背景与验收见 `code-mode-解耦与权限持久化-改动说明.md`。

## 4. 开发节点记录（19 个提交）

> 说明：下表 hash 为作者开发期（2026-08-01）的本地节点，squash 后**不存在于本
> 仓库**，仅供追溯阶段内容与评审思路；PR #138 实际提交为 6 个（自上而下）：
> `feat(codex): 原生代码会话后端链路、两个根与确认卡恢复` →
> `feat(codex): 原生代码会话前端双车道与四配置控件` →
> `feat(codex): 原生代码会话编码专用系统提示词` →
> `docs(codex): 归档原生代码会话技术文档` →
> `fix(codex): 清理原生代码会话车道内存泄漏并删除重复文档注释` →
> `fix(codex): 按审阅意见收口原生代码会话三项架构建议`。

### 阶段一：原生会话基线（方案 A）

| Hash | 说明 |
|---|---|
| `241fe054` | 代码模块接入品悟原生代码会话：store `code_session` 标记、`create_codex_acp_session` 原生分支、列表纳入、workspace_info 支持、前端双车道（code-native-lane）、选择器加"品悟"、i18n 三语、文档同步 |

### 阶段二：项目目录绑定（两个根）

| Hash | 说明 |
|---|---|
| `ce6139e0` | Rust 执行根：bridge 注入执行根解析器、`bind_code_native_session` 支持 kind/path、workspace_info/execution_workspace Project 分支、create 命令放开项目路径 |
| `be791705` | 账本锚定：附件绝对路径引用、审计分流、产物面板 code_session 守卫、远程授权回归测试（授权根本来就是账本根，无需改代码） |
| `f88817ba` | 前端开放项目目录选择（移除 pinvou 三处钳制、i18n 清理） |
| `fcf2702e` | 文档更新 + 架构决策变更记录 |

### 阶段三：实机验证驱动的缺陷修复（含一次全盘审查，发现 11 项）

| Hash | 说明 |
|---|---|
| `667cad6b` | 原生代码会话不再重复出现在普通会话列表（list_sessions 补 is_code_session 守卫） |
| `8b32c129` | 原生首发不再丢失已选项目目录（sendNative 漏传 draftWorkspacePath；测试契约禁任何 `createSession(null)`） |
| `c12c2adc` | 标题自动命名下沉后端公共函数 `apply_default_session_title`（ACP/chat 两链路统一，28 字符，已命名不覆盖） |
| `70be4b45` | bridge 不再覆盖代码会话标题与产物索引（`!meta` 恒真导致误读 active 聊天首条消息命名代码会话；tauri/web 两处同修） |
| `e5411a30` | 确认卡与忙碌态跨组件重建可恢复（app 侧 pending 登记表 + `get_pending_user_inputs` 命令，engine/submodule 不碰） |
| `babfccc2` | 原生车道补消费 `chat:shell_task_status`/`chat:compaction`（新增三语 key） |
| `3eab0c6e` | 死代码防御性分支注释 + 决策记录时效标注 |

### 阶段四：配置控件搬入

| Hash | 说明 |
|---|---|
| `10489fa7` | 提取 `composer-controls.jsx`（ChatView 行为不变），三控件支持显式会话态驱动 |
| `461a319f` | 原生车道接入四控件 + 会话态 glue + 草稿暂存应用 |
| `61a38f67` | 前端契约断言（门控/直调命令/禁用 bridge 绑定方法） |
| `56bb278b` | 视觉对齐 ACP 配置组（复用 CodexComposerConfigSelect；工具菜单加 `triggerVariant='pill'`） |

### 阶段五：编码专用提示词

| Hash | 说明 |
|---|---|
| `334a2bba` | 拆分系统提示词为共享层与工作模式层（golden 测试锁逐字节相等） |
| `b01238fc` | 原生代码会话启用编码专用系统提示词（分支注入 + present_artifact 禁用） |
| `bd2e29cf` | 文档记录提示词分层 |

## 5. 改动说明（按模块）

### Rust 后端

- `features/codex_acp/store.rs`：`code_session` 字段、`bind_code_native_session(kind, path)`（成对校验 + 禁改绑）、`is_code_session`、`code_project_workspace`、`parse("pinvou")` 别名。
- `features/codex_acp/mod.rs`：`code_native_workspace_info`、workspace_info/execution_workspace 的 code 分支（Project 走 `validate_codex_project_workspace`）。
- `app/commands/codex.rs`：`create_code_native_session`（校验/绑定/基线/回滚）、`list_codex_acp_sessions` 纳入原生（agent_id="pinvou"）、`codex_workspace_root` 放行 code_session、标题命名复用公共函数。
- `app/commands/chat.rs`：接入 `apply_default_session_title`（失败仅告警）、绑项目会话附件绝对路径引用。
- `app/commands/sessions.rs`：`apply_default_session_title` 公共函数 + 单测、`list_sessions` 排除 code_session、产物面板守卫。
- `features/assistant/platform/bridge.rs`：执行根解析器与 code_session 判定注入、`shape_disallowed_tools` 会话级整形、提示词按层分支拼装（`session_instructions`）。
- `features/assistant/pending_user_input.rs`（新增）：进程内按会话登记挂起的 user_input；engine.rs 三处接线（登记/收口/终端兜底清空）；`get_pending_user_inputs` 命令返回 `{busy, pending}`。
- `resources/common/bundle/`：`instructions-shared.md` / `instructions-work.md` / `instructions-code.md`（build.rs 的 `BUNDLE_INSTRUCTIONS_HASH` 清单同步）。

### 前端

- `features/codex/code-native-lane.js`（新增）：每会话 lane（items/busy/thinking/tokens），消费 `NATIVE_CHAT_EVENTS` 订阅清单的 18 个 `chat:*` 事件（R-1/R-3 增补后；switch 另有 `chat:plan_resolved` 分支对齐 bridge 语义：该事件由 `app/commands/interaction.rs` 的 `discard_plan` 运行时 emit 并转发远控、bridge 路径有监听，但未列入 `NATIVE_CHAT_EVENTS`，该分支经此订阅不可达）、hydration、乐观气泡、回声去重、shell 后台任务收口、compaction 系统项。
- `features/codex/CodexAcpView.jsx`：按 `agent_id === 'pinvou'` 分流（发送/事件/确认卡/取消/busy），ACP 专属 UI 隐藏，工作区选择器与项目目录流程，四配置控件接入与草稿暂存，pending/busy 恢复。
- `features/chat/composer-controls.jsx`（新增，自 ChatView 提取）：`ComposerKbSelector`/`ComposerModeChip`/`COMPOSER_ICON_BUTTON_CLASS`，可选显式会话态 props。
- `features/settings/SettingsView.jsx`：`ComposerModelSelector` 增加显式会话态 props（sessionId/sessionModelId/onSwitchModel）；`ComposerToolMenu` 增加 `triggerVariant='pill'`。
- `features/conversation/HomeModeSwitcher.jsx`：CODE_AGENT_OPTIONS 增加"品悟"。
- `app/main.jsx`：原生会话 busy 全局监听、CodexAcpView 补传 bs 与导航回调。
- `platform/tauri/bridge.js` + `platform/web/bridge.js`：persist 守卫——会话不在聊天列表则跳过 persist+rename。
- `shared/i18n.js`：uiCodex 三语 key（nativeDraftHint/nativeActiveHint/nativeBlockedNotice/compactStart/compactDone/compactFail 等）。

### 测试

- `tests/code_native_lane.test.mjs`（新增）：流式投影/tool 归类/确认卡生命周期/回滚/hydration/busy 保留/回声去重/shell 状态/compaction。
- `tests/codex_acp_timeline.test.mjs`：契约断言（原生控件门控、直调命令、禁 `createSession(null)`、bridge 绑定方法禁令）。
- Rust 单测：绑定/不可改绑、resolver 三态、workspace_info Project、标题命名/截断/不覆盖、附件绝对引用、审计分流、远程授权收窄、提示词 golden/分支/占位/disallowed。

## 6. 接口契约（前端集成要点）

- 创建：`create_codex_acp_session { agentId: "pinvou", workspacePath }`；workspacePath 为空=临时会话，非空=项目绑定（校验失败/回滚均中文报错）。
- 列表：原生会话 `agent_id="pinvou"`、`agent_name="品悟"`；标题创建时为"新对话"，首条消息后自动命名（前 28 字符）。
- 发送：`chat { sessionId, message, attachments, restrictTools: false }`；取消 `cancel_generation { sessionId }`；确认卡 `submit_user_input`/`cancel_user_input`（显式 sessionId）。
- 恢复：`get_pending_user_inputs { sessionId }` → `{busy, pending}`。
- Plan 闭环（R-1）：事件 `chat:plan_snapshot` / `chat:plan_ready`；批准 `accept_plan { sessionId, planId, planMarkdown, displayMessage? }`，放弃 `discard_plan { sessionId, planId }`。
- 压缩与记忆（R-3）：手动压缩 `compact_now { sessionId }`（兼入口=用量 chip）；事件 `chat:usage`（已用 token）、`chat:memory`（记忆快照）。
- 权限默认值与确认（2026-08-06）：`get_code_permission_prefs` → `{ last_mode, yolo_confirmed }`；`confirm_code_yolo`（首次切 yolo 确认后置全局标志）。语义见 §8.7。
- 配置：`set_session_model`/`get_session_model_id`、`session_mount_collection`/`session_unmount_collection`/`session_mounted_collection`、`set_plan_mode_next`/`exit_plan_to_yolo`/`get_mode_state`，全部显式 sessionId。

## 7. 验证记录

- 最终全量（bd2e29cf 节点）：`cargo test -p pinvou3-tauri --lib` 851 过 / 1 败（`write_artifact_text_cleans_temp_file_on_error`，既有 Windows 平台差异，CI 在 Windows 只 `--no-run`）；clippy 干净；`cargo fmt --check` 通过。
- `npm test` 全量通过；`npm run build:ui` 通过；`python3 scripts/architecture-guard.py` exit 0；`./scripts/fork-guard.sh --fast` 指纹层全过（未改 CodeWhale submodule，fork-modifications 无需登记）。
- 实机验证（Windows 10 + Watt Toolkit 网络环境）：应用启动、原生会话创建/对话、项目目录绑定、标题自动命名、四控件生效均通过；过程中发现的缺陷均已修复（见第 4 节阶段三）。

## 8. 安全默认边界（审阅收口后）

> 本节省略收口前默认态的描述，直接记录当前（PR #138 审阅修复后）的行为。

### 8.1 持久化真相源（sidecar）

- 原生代码会话的模式（`SessionMode::Code`）与项目目录绑定以 per-session 权威 sidecar
  `~/.pinvou3/sessions/<sid>/code-session.json` 持久化（`CodeSessionSidecar`：
  `workspace_kind` / `workspace_path` / `bound_at` / `version`）。
- 写入时机：`bind_code_native_session` 成功时原子写入；`set_acp_workspace` 绑定
  ACP 时重置为 `Plain` 并删除 sidecar（防止 ACP 会话被误判/误恢复）；
  `remove` 删除会话时同步清理。
- 恢复时机：`AcpPool::new` 启动时 `restore_code_native_sessions_from_sidecars` 扫描
  各 session 私有目录，辅助索引缺失但 sidecar 存在时恢复 `SessionAgentRecord`
  （仿 ACP 的 `restore_missing_acp_record`）。恢复会拒绝覆盖已被 ACP 占用的会话，
  并校验 kind/path 组合合法性。
- `session-agents.json` 仍是运行期主索引（加速），sidecar 是权威持久层（可重建）。

### 8.2 两个根统一接口（SessionRoots）

- `Pinvou3Bridge::session_roots(session_id) -> SessionRoots { execution, ledger }`
  是单一解析入口：`execution`（Engine cwd / shell 执行目录）= 项目目录（绑项目）
  或会话私有目录；`ledger`（附件/审计/产物/远程授权）= 绑项目会话恒为会话私有
  目录，其余会话与 execution 相同。
- `session_workspace()` 与 `audit_workspace()` 收敛为该接口的字段，调用方按用途
  显式选择，避免新增调用点时把执行根误当账本根写盘（或反之）。
- Engine 配置把 `workspace` 设为 `execution`，把 `subagent_state_root` 设为 `ledger`；
  delegated-agent transcript 与协调状态只读写 `ledger`。多智能体专家名册由全局专家池转换为
  原生 `fleet.profiles` 随配置提供，不在项目或会话目录生成 `.codewhale/agents/`；个人与项目
  profile 仍按 CodeWhale 原生优先级加载并允许同名覆盖，而子智能体始终在真实项目目录完成任务。

### 8.3 代码会话工具开关（按模式 scope）

> **修订注记（2026-08-22）**：本节与 §8.6 描述的双文件（`disabled_connectors.json` /
> `disabled_skills.json`）形态已被 #287/#302 的能力包统一模型取代——现行实现为单一
> `~/.pinvou3/disabled_bundles.json`（`features/marketplace/scope.rs`：包 id × 模式
> 禁用集 + `hidden_scopes` 可见性轴，读到旧双文件即迁移不删；`skill_scope.rs` 为纯
> re-export 兼容别名层）。开关粒度从"连接器 id"变为"包 id"（含技能/CLI）。
> 本节其余机制（DenyAll 安全默认、初始化语义、组合目录联动）仍准确；现行完整
> 形态以 `docs/capability-governance.md` 为准。下文保留双文件描述作历史语义参考。

- 连接器禁用列表按会话模式 scope 分开持久化（`disabled_connectors.json` 存
  `{ "scopes": { "<mode>": [...] }, "initialized": [...] }`，scope 键即
  `SessionMode` 的 kebab-case 名；旧版裸数组与 `{ plain, code,
  code_initialized }` 对象读时自动迁移）。
- **安全默认**：某 scope 未初始化时（用户从未改过该类会话开关），按其模式的
  包默认策略兜底（`SessionMode::pack_default_policy`）——plain 与 code 均为
  DenyAll：默认禁用**所有已安装连接器**（外部能力显式开启；plain 的存量装机
  由读时迁移锁定升级前状态，见 `docs/capability-governance.md`）。一旦用户
  改过该 scope 开关（进入 `initialized`），以落盘列表为准。
- 安装连接器后：DenyAll 且已初始化的 scope 中新装连接器默认仍关闭（自动加入
  禁用集）；未初始化无需落盘（读取时按「默认全禁已装连接器」兜底）。卸载
  连接器时从所有 scope 禁用集移除残留 id（含运行时清理路径）。
- 前端工具菜单按会话类型传 `scope`（普通 = `plain` / 代码 = `code`），读写各自
  scope；`shape_disallowed_tools` 经 `SessionPolicy` 策略化（§3.6）：code 会话
  按 `policy.mode()` 取 code scope 禁用集替换 plain scope 的（非连接器
  禁用如 `kb_search` 保留），并按模式能力静态表 `MODE_TABLE` 的
  `unavailable_tools` 恒隐藏 `present_artifact`；`load_skill` 按该会话组合
  目录是否为空动态决定（表字段 `skills_empty_hides_load_skill` 驱动，§8.6）。

### 8.4 远程端过滤原生代码会话事件

- 远程控制端正式支持代码会话列表/授权/UI 之前，`publish_event_inner` 对携带
  `session_id` / `id` / `sessionId` 的原生代码会话事件一律不转发（与 Engine
  bridge 共用同一份 `SessionAgentStore` 判定，`lib.rs` 注入）。普通会话事件不受
  影响；predicate 未注入时行为与注入前一致。

### 8.5 受限项目规则注入（AGENTS.md）
- 底座 C5 fork 已砍空 `PROJECT_CONTEXT_FILES`（不再自动扫描 AGENTS.md），
  `project_context_pack_enabled` 在 pinvou3 显式关闭；为满足审阅建议③a，
  在 app 侧（`Pinvou3Bridge::code_session_project_rules`）按安全边界补齐。
- **注入范围**：仅对**绑定了项目目录的原生代码会话**（execution root resolver
  命中且 `is_code_session`，双门控）注入 `AGENTS.md`。覆盖项目根逐级向上到
  用户家目录为止（家目录本身不入链，`~/AGENTS.md` 不注入，避免泄露桌面/用户
  全局配置）；项目根即家目录时一层都不注入；项目不在家目录之下时覆盖到文件
  系统根（支持 monorepo 根规则）。
- **边界比较**：project_root 与 home 均经 `canonicalize` + 去 `\\?\` verbatim
  前缀归一化后比较（与绑定入口归一化同源），Windows 上家目录边界真实生效。
  归一化失败时 fail-closed：项目根归一化失败 → 完全不注入；家目录归一化失败
  → 只注入项目根本层、不上溯祖先。
- **文件安全**：`AGENTS.md` 为 symlink 或非普通文件时跳过（`symlink_metadata`
  拒读，对齐上游 `load_context_file` 防御范式），防恶意仓库 symlink 注入工作
  区外敏感文件进提示词。
- **注入顺序**：root→cwd（祖先层在前、项目根最后），与 codex/claude 的项目
  规则叠加惯例一致。
- 普通会话、临时代码会话（resolver 未命中）不注入，行为与之前一致。
- 注入方式：`session_instructions` 中追加 `InstructionSource::File`，由底座
  `render_instructions_block` 处理 100KB 截断与缺失文件跳过。
- **会话中途编辑不生效**：注入清单与内容在 spawn 时定型；会话进行中编辑
  `AGENTS.md` 需重开会话才生效。
- 取舍：AGENTS.md 内容计入每次请求的 token 成本；仅绑项目会话注入 + 家目录
  边界 + symlink 拒读，是「不引入全局上下文风险」与「项目规则可用」之间的
  折中。

### 8.6 skill 按 scope 治理（双 scope 持久化 + 组合目录）

> **修订注记（2026-08-22）**：本节开头描述的 `disabled_skills.json` 已并入 §8.3
> 注记所述的单一 `disabled_bundles.json`（包 id 粒度）；组合目录物化、三段式时机、
> `load_skill` 联动与项目级 skills 机制仍为现行行为。历史双文件描述保留作参考。

- **开关按模式 scope 持久化**：`~/.pinvou3/disabled_skills.json` 存
  `{ "scopes": { "<mode>": [...] }, "initialized": [...], project_skills_enabled }`
  （与 `disabled_connectors.json` 同构）；旧数据迁移——裸数组 → plain scope，
  旧版借道 `disabled_connectors.json` 的 `skill:<id>` 条目 → 提取进 plain 并
  清除连接器文件残留，旧 `{ plain, code, code_initialized }` 对象 → 新 map。
  **安全默认**：DenyAll 模式（code）scope 未初始化时默认禁用所有已安装
  技能（外部能力显式开启）——这一步同时封闭 P1 泄露面（code 默认 catalogue
  为空）。
- **组合目录物化**：`EngineConfig.skills_dir` 按会话指向
  `~/.pinvou3/sessions/<sid>/skills/`（`features/assistant/skill_materialization.rs`），
  内容 = 该会话 scope 的启用技能集（来源：用户 skills 目录 + bundle/skills，
  first-wins 同名去重；另排除该 scope 被禁用连接器的 companion skills）。
  底座对 `skills_dir` 每轮重新发现渲染——空目录 → 整个 `## Skills` 块不渲染，
  catalogue 不再印任何技能名与磁盘路径（V-1 封口机制）；`load_skill` 走同一
  单根发现，按会话过滤（P4 双 scope 独立）。
- **底座配合（fork 最小改动）**：`skills_directories_with_home_and_mode` 不再
  硬编码 bundle/skills，技能发现完全由 `EngineConfig.skills_dir` 单一配置根
  注入（fork-guard T3 指纹与行为测试已更新，见 `docs/fork-modifications.md`）。
- **物化时机三段式**（不做每轮 diff）：① 引擎 spawn 前全量拼（staged +
  rename 原子替换）；② toggle/安装/卸载命令落盘后增量重写在线会话组合目录
  （diff 增删，幂等，下一轮 prompt 即生效）；③ 发送路径 `exists()` 自愈
  （缺失即重建，微秒级 stat）。
- **load_skill 放行联动**：code 会话组合目录非空 → `load_skill` 可用；空 →
  隐藏（spawn 初值与 `set_disallowed_all` 热刷两路都经
  `bridge.shape_disallowed_tools`）。
- **项目级 skills**（P3 兜底）：fork #41 已砍断 workspace 并集扫描，项目技能
  （`.agents/skills`、`.pinvou/skills`、`skills`、`.opencode/skills`、
  `.claude/skills`、`.cursor/skills`、`.codewhale/skills`，按上游优先级，
  `.pinvou/skills` 为 pinvou3 自有约定）在**策略开关默认关**
  下经同一物化通道拷入组合目录；开启路径有注入风险警告（项目内文本是
  prompt-injection 面）。catalogue 显示组合目录路径而非 bundle 内部结构。
- 治理机制的当前形态见 `docs/capability-governance.md`。

### 8.7 权限默认值与两层持久化（2026-08-06）

> **修订注记（2026-08-22）**：本节的"两层持久化"语义已被 PR #263（三工作区三分
> lane）取代，现行语义以 `docs/code-mode-解耦与权限持久化-改动说明.md` 第六节与
> `features/sessions/mode_state.rs` 为准。主要变化：① per-session mode 持久化
> 现覆盖**所有会话**（含 plain），文件更名为 `~/.pinvou3/sessions/_session_mode_states.json`
> （旧 `_code_mode_states.json` 回退读一次）；② `accept_plan` 的任务级 yolo 切换
> 已纳入 per-session 持久化（确认提交后写入）；③ 全局 lane 默认只由草稿态显式
> 切换经 `set_mode_default` 写入。下文保留 2026-08-06 语义作历史参考。

原生 code 会话的 Plan/Yolo 默认值与记忆策略（plain 会话行为不变：mode 仅驻
内存、默认 Yolo、不落盘）：

- **首次默认 Plan**：用户从未使用过 code 模式时，新建原生 code 会话默认 Plan
  （只读调研 + R-1 方案审批卡兜底）；`mode_state` 无记录时按
  `resolved_default_mode` 解析——code 会话取全局 `last_mode`，无记录回退 Plan。
- **切 yolo 一次性确认**：任何 code 会话首次切到 yolo 时弹确认卡（"全自动读写
  项目目录、可执行 shell、无逐步审批"），确认后写全局 `yolo_confirmed`，之后
  任何会话、任何次切换不再弹；确认是 UI 层语义，`exit_plan_to_yolo` 后端不
  强制门控（与 VS Code 首次选 Bypass 弹警告同款）。
- **两层持久化**：每会话 mode 存 `~/.pinvou3/sessions/_code_mode_states.json`
  （仿 `_skill_bindings.json` 模式，删除会话同步清理，仅 code 会话落盘）；
  全局 `code_permission.last_mode` / `yolo_confirmed` 挂 settings.json
  （`UserPrefs` 字段级事务写），新建 code 会话默认跟随上次使用的 mode。
- **任务级切换不记忆**：`accept_plan` 经 `claim_pending_plan` 切 yolo 属任务级
  动作（执行该方案），不写两层持久化——重启后会话恢复其持久化的 mode。
- 关键正确性：plan/技能/persona/知识库等 9 处条目物化站点统一走
  `mode_state_entry`，避免把"首次默认 Plan"的会话物化成 Yolo 导致 plan 卡
  静默丢失（有单测钉住）。

## 9. 已知限制与遗留风险

- Plan 模式审批闭环已落地（R-1）：code 页消费 `chat:plan_snapshot`/`chat:plan_ready`，
  渲染方案审批卡（【✅ 就这么干】调 `accept_plan`/【🚪 算了】调 `discard_plan`）；
  remount 后历史方案降级为只读卡（与 work 冷启动语义一致）。
- `chat:usage` 无 context 上限字段（`tokens.max` 恒 0）：code 页用量 chip 按降级处理，
  只显示已用 token、不显示上限与百分比；chip 兼手动压缩入口（点击调 `compact_now`）。
- 项目规则注入（审阅建议③a）仅覆盖绑项目代码会话的 root→cwd 各层 `AGENTS.md`，
  不注入用户家目录/全局上下文；规则文件内容计入 token 成本（100KB 截断由底座
  处理）；symlink 拒读、家目录边界归一化比较与 fail-closed 语义见 §8.5。
- 远程端 `web_access_*` RPC 面未封：事件流已按 §8.4 对原生代码会话不转发，但
  远程端凭 session id 仍可读/写代码会话消息。远程正式支持代码会话前暂保留该
  通道（事件面先封是主要泄露面），是否同步封 RPC 面需审阅者确认。
- ~~代码会话 skill 侧路残留（过渡方案 D）~~ 已根治（skill-scope-governance，
  2026-08-06）：组合目录空时 `## Skills` 块不渲染（catalogue 不再印技能名与
  磁盘路径，read_file 侧读无从得知路径）；代码页 skill 开关恢复可写并按 code
  scope 持久。原 X-1 标记项由 §8.6 方案落地，过渡方案 D 两组件（整体禁用
  load_skill + 只读灰显）退役。
- 确认卡/busy 恢复依赖进程内登记表，进程重启后挂起 input 随旧 engine 消亡（与 ACP orphaned turn 语义一致）。恢复方案已定为「挂起输入写入 sidecar + 重启后 UI 标记中断，可继续/丢弃」，**不恢复在途回合**（VS Code 同款取舍，避免半执行状态）；待实施（T-3）。
- 原生会话无模型 API key 前置 gate，未配置凭据时错误在对话流内联展示。
- 回声去重为启发式（文本一致或 30 秒窗口），极端时序理论可能双气泡。
- 工作区基线对大型非 git 项目为全量指纹（与 ACP 项目会话同语义）。

## 10. 待决策事项（已记录，未实施）

1. 渐进重构：`codex_acp` / `CodexAcpView` 同时承载 ACP 与 Native 两种运行时，
   建议逐步上提为 `code_sessions` / `CodeView`，两条链路分别做 adapter/hook。
2. CI hardening: high-blast-radius paths automatically run full Linux/Windows
   `rust-test`; maintainers can add `ci:full-rust` to another ready Rust PR, and
   high-risk merge groups retain the full Linux regression on the combined tree.
3. 代码会话工具/技能分化（审阅建议②，已定论）：代码会话当前继承全集
   工具，已落地的隔离有——连接器工具按 scope 整形（§8.3）、隐藏
   `present_artifact`、skill 双 scope 治理（§8.6：组合目录过滤 catalogue +
   `load_skill` 按目录空否放行），且上述差异已收编进 `SessionPolicy` 策略对象
   （§3.6）。工具面进一步分化的形态已定为**编译期常量**（能力档案 JSON 方案
   评审后退役，见 `docs/capability-governance.md`），原 X-1 标记项关闭。

## 11. 过程产物

实施各阶段交接笔记与全过程总结（含每阶段验证原始记录）存于贡献者本地 `.luzeyang/code-native-agent-实施总结.md`（不入库）。本文件为归档版单一真相源。
