# Pinvou TUI 会话、模型与权限纵向闭环设计

> 状态：用户已确认设计方向，待书面规格复核
> 日期：2026-08-25
> 范围：`/resume`、`/model`、`/permissions`、Workspace 默认选择、WAL cursor/snapshot 恢复

## 1. 目标

在现有可运行 Pinvou TUI 上补齐真实 Codex 纵向闭环。用户在当前 Workspace 运行 `pinvou` 后，可以恢复最近聊天、查看并切换当前 Runtime 实际支持的模型、选择 Pinvou 三种权限模式，并在退出重开后继续看到一致的会话、模型、权限和任务结果。

本阶段不接入 Claude Code、CodeBuddy、Kimi Code，不修改 Desktop/Tauri 或 CodeWhale，也不实现 Remote Node、跨 Workspace 浏览、协作任务或插件管理。

## 2. 核心边界

- Controller 持有 Logical Session、Workspace 索引、事件 WAL、cursor/snapshot 和用户选择的权威状态。
- Runtime 原生 session/thread ID 只作为带 Runtime、模型、能力和 epoch 指纹的 Attachment，不成为会话主键。
- Node/Adapter 负责把统一操作映射到 Codex `thread/list|read|resume`、`model/list` 和审批/sandbox 配置。
- TUI 只依赖扩展后的统一 Backend port；不得读取 Controller 文件或调用 Codex 私有协议。
- 已完成工具调用只恢复结果，不重新执行；结果不确定的非幂等调用必须显示 `uncertain` 并等待用户处理。

## 3. 用户流程

### 3.1 启动与 Workspace 默认值

`pinvou` 仍默认进入新会话，不增加启动页。初始化按以下顺序解析 Runtime、模型和权限：

1. 当前 Workspace 最近一次成功选择；
2. 用户全局默认；
3. Runtime 报告的默认值；
4. 无法安全选择时打开对应 overlay。

失效记忆只允许在新会话初始化时回退，并在状态区说明原值、原因和最终选择。活动会话不得静默回退。

### 3.2 `/resume`

`/resume` 打开当前 Workspace 的最近会话 overlay，支持搜索、方向键选择、取消和确认。每项显示标题、最后活动时间、Runtime、模型、终态以及恢复策略。

恢复顺序为：加载 Controller snapshot → 从 WAL cursor 补放事件 → 校验 Attachment 指纹 → 能力允许时 native resume → 否则从 Portable Checkpoint/规范化历史创建新 Attachment。任何失败都保留当前会话，不能把半恢复状态提交为活动会话。

### 3.3 `/model`

`/model` 只展示当前 Runtime 经统一接口实时返回的模型目录，不写死模型 ID。条目包含稳定 ID、显示名、默认/当前标记、可用状态、支持/default reasoning level 和切换方式。常驻欢迎区与状态栏必须显示当前 Runtime、下一回合有效模型和有效 reasoning level；尚无 Session 级选择时，模型回退为 Runtime 的可用默认项，level 优先使用 Runtime 报告的当前配置，再回退为模型默认值，不能长期显示含糊的 `auto`。

切换只允许在回合边界执行。支持 reasoning level 的模型按 Enter 后进入二级 level 选择器，只展示 Runtime 实时声明的 levels；不支持 level 的模型直接切换。模型与 level 作为一次原子选择提交，Controller 必须校验 level 属于目标模型，Codex Adapter 将其映射为下一回合及后续回合的 `turn/start.effort`。Runtime 支持会话内切换时更新下一回合模型；只支持创建 Attachment 时指定模型时，执行同 Runtime checkpoint/handoff。提交失败必须保留原模型、level、Attachment 和 epoch。

### 3.4 `/permissions`

Pinvou 暴露稳定的三种产品模式：

- `request`（请求批准）：有副作用或提权动作由用户决定，作为默认值。
- `assisted`（帮我批准）：只自动处理显式预授权的低风险范围；未知、高风险和越界请求继续询问或阻止。
- `full_access`（完全访问）：映射到 Runtime 可提供的最高权限，仍显示 OS、企业策略和 Runtime 硬保护；选择前必须确认风险。

状态同时保存产品模式、决策主体、控制强度、原生模式、sandbox、剩余保护和能力证据版本。无法证明执行前可阻止时必须显示 `partial` 或 `unsupported`，不得伪装成 Pinvou 已控制。

## 4. 统一接口

新增或扩展以下领域类型与操作：

```text
SessionDescriptor / SessionSnapshot / ResumePlan / ResumeResult
ModelDescriptor / ModelCatalog / ModelSelection
ApprovalProfile / ApprovalCapability / PermissionSelection
WorkspacePreferences { runtime, model_by_runtime, reasoning_level_by_runtime, approval_profile }

session.list(workspace, query, cursor)
session.resume_prepare(session_id)
session.resume_commit(resume_token)
model.list(runtime_id)
model.switch_prepare(model_id, reasoning_level?)
model.switch_commit(switch_token)
permissions.inspect()
permissions.switch_prepare(profile)
permissions.switch_commit(switch_token)
```

所有切换使用 prepare/commit，token 绑定 session、attachment epoch、目标值和能力证据版本。Controller 重启后不得接受过期 token。

## 5. 数据与恢复

Controller 在自己的数据根保存版本化记录：

```text
sessions/<logical-session-id>/metadata.json
sessions/<logical-session-id>/events.seglog
sessions/<logical-session-id>/snapshot.json
workspaces/<workspace-hash>/preferences.json
```

写入顺序为 WAL durable → snapshot/metadata 原子替换 → 发布成功响应。snapshot 带最后应用 cursor；启动恢复只补放更大的 sequence。损坏、缺口、重复 sequence、Attachment 指纹不匹配均返回可分类错误，不猜测成功。

## 6. TUI 状态与交互

TUI 为 Session、Model、Permissions 增加独立 overlay state、请求 token、选择位置、搜索文本和错误状态。overlay 只在 idle 打开；流式回合、审批或输入请求期间拒绝切换。

后台调用继续使用有界 channel 和可取消 control lease。Ctrl+C 只 detach 本地 TUI；Escape 取消 overlay 或中止当前回合，不隐式批准、提交切换或停止 Controller。

### 6.1 已冻结的产品化视觉合同

2026-08-25 用户确认继续采用 Ratatui，并按 Claude Code / Codex 风格的连续文本流重构，不切换框架。实现和终端快照按以下合同验收：

- 欢迎态只出现一次，使用 Pinvou 品牌、当前 Workspace/Runtime/Model/权限上下文和少量可执行引导，不保留仪表盘式常驻标题。
- 用户、Agent、工具、审批、输入请求与错误使用不同语义标记和层级；用户消息使用全宽独立背景和内边距形成稳定消息块，Agent 输出保持普通连续文本背景；常规工具活动留在连续聊天流，不堆叠重边框卡片。
- `Starting`、`Streaming`、工具执行和取消等待必须在 transcript 内提供可见反馈，同时显示当前可执行的中断或 detach 操作。
- composer 使用独立边界与稳定焦点；可编辑状态必须显式显示终端光标，并按 Unicode 显示宽度定位，长输入裁切后光标仍留在边框内；底部状态区分开运行上下文、暂态消息和当前键位，不宣传无效快捷键。
- `/resume`、`/runtime`、`/model`、`/permissions` 使用统一 overlay 结构：来源/范围说明、当前选择、默认或 unsupported 标记、风险/剩余保护和底部键盘提示。
- 空闲态普通 composer 输入 `/` 时，在聊天区底部显示由真实命令目录驱动的内联 command menu；每项包含命令和用途，按输入前缀实时过滤，支持 ↑/↓ 选择、Enter 执行、Esc 关闭。菜单不得在运行中、审批、输入请求或其他 overlay 上争抢键盘。
- 60x16 为完整界面最小合同；更小尺寸只显示安全退出提示。overlay 在窄终端隐藏次要细节，但保留选择、当前状态和取消路径。
- 错误必须说明旧会话/模型/权限是否保持不变，并给出重试或返回选择器的下一步；不得只显示原始后端诊断。

## 7. 错误与安全

- 所有远端/Runtime 错误映射为固定安全文案，原始内容不得泄漏 token、路径或凭据。
- native resume 失败可进入已明确展示的 checkpoint fallback，但不能静默创建新 Logical Session。
- 模型或权限切换失败保持旧配置为唯一活动写者。
- `full_access` 必须逐 Workspace/Session 确认，不能成为静默全局默认。
- cursor 缺口、未决审批无法恢复、工具结果不确定均在聊天流中显示可操作状态。

## 8. 实施顺序

1. 持久 Session/Workspace/WAL cursor 基础与 Controller IPC。
2. Codex Attachment 列举、读取和 native resume，完成 `/resume`。
3. 统一模型目录与切换，完成 `/model`。
4. 统一权限能力与切换，完成 `/permissions`。
5. 退出重开、恢复、模型切换、权限切换和真实工具任务端到端验收。

每一步都必须从 Runtime API 到 Adapter、Node、Controller、TUI 形成可运行纵向切片，不以 UI mock 或 Controller 假响应宣称完成。

## 9. 必要验证

只保留与本阶段风险直接相关的验证：

- 纯状态机和序列化/迁移单测；
- Controller/Node IPC 合约与 prepare/commit 原子性；
- Codex Adapter 黑盒合约；
- TUI app contract 与真实 Windows PTY 恢复；
- 分布式依赖、stage1 zero-diff、格式和 Clippy 门禁；
- 一次真实 Codex 多轮聊天：切模型、改权限、执行工具、退出、重开并恢复。

不运行无关 Desktop/Tauri/AWS、旧 benchmark 数据集或全仓库产品构建；若共享类型或依赖闭包发生变化，再扩大到受影响门禁。

## 10. 完成标准

- `/resume`、`/model`、`/permissions` 都由真实 Controller/Node/Codex 数据驱动。
- 当前 Workspace 的 Runtime、模型和权限选择可持久恢复。
- 会话退出重开后历史、终态、工具结果和 cursor 一致，已完成工具不重放。
- 任一 prepare/commit、Runtime 或磁盘失败都保持旧会话/Attachment/配置有效。
- 用户无需退出 TUI 或使用诊断 CLI 完成上述流程。

## 11. 2026-08-25 实施状态

本轮已落地 Runtime API、Codex Adapter、Node、Controller 持久化、TUI Backend 和终端交互纵向链路。无参数 `pinvou` 仍直接进入 TUI；无既有选择记录时默认 Runtime 为 Codex。TUI 已提供 `/resume`、`/model`、`/permissions`，其中 `full_access` 必须二次确认，权限页显示 Runtime 报告的 `enforced`、`partial` 或 `unsupported` 控制强度及 residual guards。

已通过 Runtime API、Codex Adapter、Node、Controller、TUI 状态机和真实 Windows PTY 定向测试；PTY 已验证三个命令打开真实 overlay、Escape 返回输入以及退出后恢复终端。正式 distributed 构建产物位于 `D:\pinvou-cargo-target-session-model-permissions\debug\pinvou.exe`，resolved dependency boundary 通过。

当前不能将本阶段标记为完整 live 闭环：尚未执行会产生真实模型调用的 Codex 多轮/工具/退出重开验收；严格 Clippy 被本阶段外既有 `seglog`、`benchmark-core`、Windows IPC/lock 告警阻断；stage1 zero-diff 相对 `origin/main` 的 merge-base 会包含当前长期分支上既有 Desktop、CodeWhale 与文档差异。后续还需收口：Controller 控制合约补齐全部 model/permission prepare/commit 的真实 Node 测试；把 capability evidence version 显式纳入 token 失效判定；native resume 增加 Attachment 指纹校验和显式 checkpoint fallback；解决 Node 已切换 Attachment、Controller 随后落盘失败时的跨进程补偿；初始化真正应用 Workspace Runtime 偏好并向 TUI 说明失效记忆的回退；按事务边界把 `ControllerSession` 内的 session/model/permission 控制服务渐进拆出。未完成前不得宣称所有完成标准已满足。
