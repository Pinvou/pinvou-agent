# 代码模块品悟原生会话（code-native-agent）技术文档

> 分支：`feat/code-native-agent`（PR #138，已 rebase 至最新 `main`）
> 周期：2026-08-01 单日完成，共 19 个开发节点（squash 为 PR 的 6 个提交，见 §4）。
> 关联文档：`codex-acp.md`（使用与验证）、`multi-agent-acp.md`（多 agent 单一真相源）；原 `Codex-ACP-整体架构决策.md` 的决策变更记录由本文第 3 节承接（该文档已随主线清理下线）。

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
| 事件流 | `acp:event` | `chat:*`（13+2 个事件） |
| 前端投影 | `projectAcpTimeline` | `code-native-lane.js` → `projectDeepSeekConversation` |
| 展示层 | `ConversationTimeline`（共用） | 同左 |
| 会话存储 | `SessionStore` + `SessionAgentStore` | 同左（`code_session` 标记） |

关键共用：`SessionStore`（`~/.pinvou3/sessions/`）、`SessionAgentStore`（`~/.pinvou3/session-agents.json`）、统一会话列表、工作区面板、附件体系、i18n。

## 3. 核心设计决策

### 3.1 会话标记而非新链路

- `AgentBackend` 枚举复用既有原生变体 `Deepseek`（display_name "品悟"），`parse` 增加 `"pinvou"` 别名，kebab-case 序列化契约不变。
- `SessionAgentRecord` 增加 `code_session: bool`（serde default + skip_serializing_if，旧记录兼容），普通聊天会话无记录默认 false。
- 原生代码会话对 `AcpPool::is_acp` 保持 false：`chat` 命令天然放行，ACP 命令经 `acp_record` 检查自然拒绝，两链路互不串台。
- "会话创建时绑定 Agent/目录，开始后不可切换"对原生与 ACP 同样生效。

### 3.2 两个根：执行根 ≠ 账本根

绑项目目录的核心安全设计：

- **执行根**（LLM 面向）：engine cwd、文件工具相对路径根、shell 执行目录。经 `Pinvou3Bridge` 注入的执行根解析器（共享 AcpPool 那份 `SessionAgentStore` 实例）推导；engine spawn（`engine.rs`）与 shell_workspace（`engine_pool.rs`）两个消费点经 `bridge.session_workspace` 自动同源。
- **账本根**（应用面向）：附件、暂存、审计、产物、远程授权，恒为 `~/.pinvou3/sessions/<sid>/`（`SessionStore::execution_workspace` **一行未改**，25+ 调用点全是账本语义）。
- 效果：项目目录零污染（无 `attachments/`、`.pinvou3/`、`workflow_audit.jsonl` 写入）、删会话零残留、远程下载授权面不扩大、产物面板不误扫项目。
- 例外适配：附件引用在绑项目会话改用绝对路径（相对路径会相对项目根解析落空）。

### 3.3 前端双车道而非复用 bridge

bridge 的 chat 状态机绑定单一 activeSession，代码页与主聊天并存会串台。改为每会话一个本地 lane（纯数据模块 `code-native-lane.js`，可单测）：消费 `chat:*` 事件、从 SavedSession + `timing_events.jsonl` 还原历史、乐观气泡 + 失败回滚 + 回声去重。确认卡（`chat:user_input_required`）用显式 sessionId 调 `submit_user_input`/`cancel_user_input`，不经 bridge。

### 3.4 提示词分层（共享层 + 模式层）

- 原唯一 prompt 来源 `bundle/instructions.md` 拆为 `instructions-shared.md`（身份/底线/工具纪律/红线/输出，骨架含两个模式层占位行）与 `instructions-work.md`（产出物面板/tmp 语义、present_artifact 条）。
- 代码会话 = 骨架 + `instructions-code.md`（代码身份头、工作区段含项目路径占位符、代码场景纪律：就地修改/不建 tmp//git 纪律/范围纪律/验证纪律/交付汇报）+ **底座 `CORE_EXECUTION_PROFILE_PROMPT` 原样引用**（Rust 渲染层拼接 `deepseek_tui::prompts::CORE_EXECUTION_PROFILE_PROMPT`，零文本复制，上游更新自动跟随）。
- 未引入底座 `BASE_PROMPT`（含 "You are Codewhale" 身份；其权威顺序/防编造干货在共享层 §底线已有等价物）。
- 工作会话渲染结果与拆分前**逐字节相等**（golden 测试内嵌 33 行原文快照断言）。
- 工具整形：代码会话 `disallowed_tools` 追加 `mcp_pinvou3_present_artifact`（spawn 初值与 `set_disallowed_all` 热刷统一经 `bridge.shape_disallowed_tools`）。

### 3.5 配置控件复用策略

聊天页输入框底栏四控件（Plan/Yolo、模型、工具菜单、知识库挂载）搬入代码模块原生车道，视觉对齐 ACP 配置组（`CodexComposerConfigSelect` pill 形态）。关键约束：bridge 的 models/knowledge/interaction 方法绑聊天 active 且草稿态会物化聊天会话，代码车道一律直调 per-session Tauri 命令显式传 sessionId；草稿态选择暂存，建会话后按序应用（model → kb → mode），失败显式报错。

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

- `features/codex/code-native-lane.js`（新增）：每会话 lane（items/busy/thinking/tokens），13+2 个 `chat:*` 事件消费、hydration、乐观气泡、回声去重、shell 后台任务收口、compaction 系统项。
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
- 配置：`set_session_model`/`get_session_model_id`、`session_mount_collection`/`session_unmount_collection`/`session_mounted_collection`、`set_plan_mode_next`/`exit_plan_to_yolo`/`get_mode_state`，全部显式 sessionId。

## 7. 验证记录

- 最终全量（bd2e29cf 节点）：`cargo test -p pinvou3-tauri --lib` 851 过 / 1 败（`write_artifact_text_cleans_temp_file_on_error`，既有 Windows 平台差异，CI 在 Windows 只 `--no-run`）；clippy 干净；`cargo fmt --check` 通过。
- `npm test` 全量通过；`npm run build:ui` 通过；`python3 scripts/architecture-guard.py` exit 0；`./scripts/fork-guard.sh --fast` 指纹层全过（未改 CodeWhale submodule，fork-modifications 无需登记）。
- 实机验证（Windows 10 + Watt Toolkit 网络环境）：应用启动、原生会话创建/对话、项目目录绑定、标题自动命名、四控件生效均通过；过程中发现的缺陷均已修复（见第 4 节阶段三）。

## 8. 安全默认边界（审阅收口后）

> 本节省略收口前默认态的描述，直接记录当前（PR #138 审阅修复后）的行为。

### 8.1 持久化真相源（sidecar）

- 原生代码会话的类型（`code_session`）与项目目录绑定以 per-session 权威 sidecar
  `~/.pinvou3/sessions/<sid>/code-session.json` 持久化（`CodeSessionSidecar`：
  `workspace_kind` / `workspace_path` / `bound_at` / `version`）。
- 写入时机：`bind_code_native_session` 成功时原子写入；`set_acp_workspace` 绑定
  ACP 时清除 `code_session` 标志并删除 sidecar（防止 ACP 会话被误判/误恢复）；
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

### 8.3 代码会话工具开关（双 scope）

- 连接器禁用列表按会话类型分开持久化（`disabled_connectors.json` 存
  `{ plain, code, code_initialized }`，旧版裸数组兼容迁移为 plain）。
- **安全默认**：code scope 未初始化时（用户从未改过代码会话开关），代码会话默认
  禁用**所有已安装连接器**（外部能力显式开启）；一旦用户改过 code 开关
  （`code_initialized=true`），以落盘列表为准。
- 安装连接器后：code 已初始化时新装连接器默认仍关闭（自动加入 code 禁用集）；
  未初始化无需落盘（读取时按「默认全禁已装连接器」兜底）。卸载连接器时从
  plain/code 两个禁用集移除残留 id（含运行时清理路径）。
- 前端工具菜单按会话类型传 `scope`（普通 = `plain` / 代码 = `code`），读写各自
  scope；`shape_disallowed_tools` 对代码会话用 code scope 的连接器禁用集替换
  plain scope 的（非连接器禁用如 `kb_search` 保留），并继续隐藏
  `present_artifact`。

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

## 9. 已知限制与遗留风险

- Plan 模式降级：方案以文本/普通工具卡呈现，无 `accept_plan` 审批卡，需手动切回 Yolo（后续专项）。
- `chat:usage` 无 context 上限字段，用量 chip 不显示（`tokens.max` 恒 0）。
- 项目规则注入（审阅建议③a）仅覆盖绑项目代码会话的 root→cwd 各层 `AGENTS.md`，
  不注入用户家目录/全局上下文；规则文件内容计入 token 成本（100KB 截断由底座
  处理）；symlink 拒读、家目录边界归一化比较与 fail-closed 语义见 §8.5。
- 远程端 `web_access_*` RPC 面未封：事件流已按 §8.4 对原生代码会话不转发，但
  远程端凭 session id 仍可读/写代码会话消息。远程正式支持代码会话前暂保留该
  通道（事件面先封是主要泄露面），是否同步封 RPC 面需审阅者确认。
- 代码会话 skill 侧路残留（过渡方案 D）：代码会话已整体禁用 `load_skill` 工具
  + 代码页 skill 分组只读诚实化，但 `## Skills` catalogue 仍印在提示词中（含
  skill 磁盘路径），被注入引导的模型可经 `read_file` 读取 SKILL.md。根治方案
  见 `docs/code-native-agent-会话能力档案设计.md`（按会话能力档案过滤
  catalogue，落地后方案 D 两组件退役）。
- 确认卡/busy 恢复依赖进程内登记表，进程重启后挂起 input 随旧 engine 消亡（与 ACP orphaned turn 语义一致）。
- 原生会话无模型 API key 前置 gate，未配置凭据时错误在对话流内联展示。
- 回声去重为启发式（文本一致或 30 秒窗口），极端时序理论可能双气泡。
- 工作区基线对大型非 git 项目为全量指纹（与 ACP 项目会话同语义）。

## 10. 待决策事项（已记录，未实施）

1. 渐进重构：`codex_acp` / `CodexAcpView` 同时承载 ACP 与 Native 两种运行时，
   建议逐步上提为 `code_sessions` / `CodeView`，两条链路分别做 adapter/hook。
2. CI 增强：正式 `rust-test` 目前 skipped（Windows 只 `--no-run`），建议加
   `ci:full-rust` 让完整测试成为该 head 的正式 check。
3. 代码会话工具/技能 profile（审阅建议②，恢复登记）：代码会话当前继承全集
   工具，已落地的隔离仅有——连接器工具按 scope 整形（§8.3）、隐藏
   `present_artifact`、过渡方案 D 禁用 `load_skill`（§9 残留说明）。完整的
   「代码专用工具 profile」未兑现：skill 维度由
   `docs/code-native-agent-会话能力档案设计.md` 承接（按会话能力档案过滤
   catalogue + `load_skill` 按档案判定），其余工具维度的 profile 化待该设计
   评审后一并实施。

## 11. 过程产物

实施各阶段交接笔记与全过程总结（含每阶段验证原始记录）存于贡献者本地 `.luzeyang/code-native-agent-实施总结.md`（不入库）。本文件为归档版单一真相源。
