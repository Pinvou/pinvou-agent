# Pinvou TUI 阶段 3 设计

> 状态：已完成交互讨论，待用户审阅书面规格
> 日期：2026-08-24
> 范围：Pinvou CLI 自有 TUI、Runtime/Model 选择、会话恢复、统一工具审批
> 不涉及：CodeWhale TUI、Desktop/Tauri 接入、Remote Node、完整 Tasks/Plugins/Collaboration 页面

## 1. 目标

阶段 3 的首要目标不是展示一个终端 Demo，而是交付可持续使用的 Pinvou Agent TUI：用户在工作目录执行 `pinvou` 后即可开始新聊天，并能在界面内完成恢复会话、选择 Runtime、选择模型、响应工具审批、中止回合和查看流式结果。

完整阶段 3 仍以 Codex、Claude Code、CodeBuddy 复用同一 TUI 产品合同为验收门槛。第一阶段性目标先用真实 Codex 打通完整纵向闭环，但不得把 Codex 私有协议或界面状态泄漏到 TUI。Kimi Code 按后续 Adapter 批次接入，其权限映射已经在本设计中预留。

## 2. 参考与决策优先级

本设计参考以下材料：

1. `2026-08-18-pinvou-distributed-node-runtime-design.md`：Controller/Node/Runtime 总体边界、TUI 技术栈、事件恢复和阶段路线。
2. `D:/Worksapce/SourceCode/Task/SmartPerfetto/private/multi-runtime-agent-control-plane-chat-session.md`：Portable Checkpoint、Context Compiler、Unified Tool Plane、审批与断线恢复原则。该文档只作参考，不直接成为 Pinvou 实现要求。
3. `docs/research/2026-08-24-runtime-approval-capability-matrix.md`：Codex、Claude Code、CodeBuddy、Kimi Code 的官方权限接口调研。
4. 本轮用户确认：唯一交互入口、Claude Code 风格、Runtime/Model 切换和三种权限模式。

发生冲突时，本设计和用户已确认的产品行为优先于长期蓝图中的旧入口示例。保留现有 Pinvou Controller/Node 独立进程架构，不采纳参考文档中“单个可执行文件承担所有后台角色”的未冻结建议。

## 3. 产品入口合同

`pinvou` 是唯一的 TUI 用户入口，不提供 `pinvou tui` 或 `pinvou tui --force`。

| 调用 | 行为 |
|---|---|
| `pinvou` | stdin/stdout 都是交互终端时，启动 TUI，并以当前工作目录创建一个新逻辑会话。 |
| `pinvou <subcommand>` | 执行脚本化 CLI 命令，完成后退出，不进入 TUI。 |
| `pinvou --help` / `pinvou --version` | 输出信息后退出。 |
| 非 TTY 中无参数执行 `pinvou` | 快速失败并提示使用具体子命令，不初始化全屏终端，也不等待输入。 |
| 未知子命令 | 返回稳定 usage 错误，不回退到 TUI。 |

阶段 3 上线前，阶段 1–2 的无参数帮助行为继续保留；只有真实 TUI 纵向闭环达到门禁后，才切换无参数默认行为。

## 4. 交互与视觉方向

TUI 采用 Claude Code 风格的连续文本流：以缩进、符号和颜色表达用户消息、Agent 输出、思考、计划和工具活动，避免把所有事件堆成卡片。

以下高风险或高信息密度交互使用轻量边框卡片：

- 文件 diff；
- 工具审批和用户输入请求；
- Runtime/Model handoff 预检；
- 错误、恢复和不确定副作用。

主界面保持单列沉浸式聊天。Runtime、Model、Workspace、Session、权限模式和连接状态放在欢迎区与底部状态栏；选择器以居中 overlay 打开，不设置常驻侧栏挤占窄终端。

## 5. 启动与会话恢复

### 5.1 新会话

`pinvou` 在当前目录直接进入一个新会话，不先显示会话启动页。启动过程解析默认 Runtime 和模型，连接 Controller，并在准备完成后才允许发送第一条消息。

### 5.2 `/resume`

`/resume` 打开当前 Workspace 的最近聊天列表，支持搜索、键盘选择和恢复状态提示。默认不混入其他 Workspace 的会话；跨 Workspace 恢复留给后续显式入口。

恢复列表至少显示：

- 会话标题和最后活动时间；
- 原 Runtime 与模型；
- completed/interrupted/awaiting approval 等状态；
- 是否存在可用原生 Session；
- 是否需要 Portable Checkpoint 恢复或上下文重新编译。

恢复的事实源是 Controller 中的 Logical Session、事件账本和 checkpoint。Runtime 原生 Session 只是带能力/指纹约束的恢复优化，不能取代统一会话。

## 6. Runtime 与模型选择

### 6.1 默认 Runtime

新会话按以下优先级解析 Runtime：

1. CLI 显式 `--runtime`；
2. 当前 Workspace 上次成功使用的 Runtime；
3. 用户设置的全局默认 Runtime；
4. 如果只有一个已安装、已登录的 Runtime，自动选择它；
5. 否则弹出 Runtime 选择 overlay。

记忆中的 Runtime 失效时允许在新会话启动阶段回退，但必须显示原因和最终选择。活动会话中 Runtime 失败不得静默回退。

### 6.2 `/runtime` 与 `Ctrl+R`

两者打开同一 Runtime 选择器。切换遵循现有 Prepare/Commit、回合边界和 attachment epoch 合同：

1. 等待当前回合结束，或由用户先中止；
2. 探测目标 Runtime、登录状态和能力；
3. Flush 旧 Attachment，并生成/刷新 Portable Checkpoint；
4. 按目标 Runtime/模型窗口编译上下文；
5. 展示压缩内容、能力差异、工具缺口和风险；
6. 用户确认后启动目标 Attachment；
7. 目标接受后原子提交新 epoch，并 fence/drain 旧 Attachment；
8. 任一步失败都保留原 Attachment 为活动写者。

### 6.3 `/model`

Runtime 和 Model 是两个独立维度。`/model` 只展示当前 Runtime 通过统一能力接口实时报告的模型，不在 TUI 中写死模型 ID。

默认模型按以下顺序解析：

1. 当前 Workspace + Runtime 上次成功使用的模型；
2. Runtime 报告的默认模型。

模型目录至少包含稳定 ID、显示名、是否默认、可用状态和切换方式。选择只在回合边界提交：

- Runtime 支持会话内切换时，更新下一回合使用的模型；
- Runtime 只支持 Attachment 创建时指定模型时，执行同 Runtime 的 checkpoint + 新 Attachment handoff；
- 切换失败时保留当前模型和 Attachment；
- 记忆模型在新会话启动时失效，可回退到 Runtime 默认模型并明确提示。

为此，`runtime-api` 和 Controller IPC 必须增加统一 `ModelDescriptor`、模型目录查询和模型选择能力；Codex Adapter 内部已有的 `model/list` 不能继续只作为私有默认值探测。

## 7. 权限与审批模式

Pinvou 定义稳定的三种产品模式，Adapter 负责映射到 Runtime 原生权限接口：

| 模式 | 产品语义 | 默认决策主体 |
|---|---|---|
| 请求批准 | 有副作用或需要提权的动作交给用户决定。 | 用户 |
| 帮我批准 | Pinvou 自动处理预授权范围内的低风险请求；高风险、越界和未知请求仍阻止或询问。 | Pinvou；明确选择原生分类器时可为 Runtime |
| 完全访问 | 启用 Runtime 当前能提供的最高权限，仍受 OS、企业策略、Runtime 硬保护和审计约束。 | Runtime |

默认使用“请求批准”。“帮我批准”按 Workspace/Session 显式开启；“完全访问”必须风险确认，不能静默成为全局默认。

权限状态不能只保存一个枚举，至少包含：

```text
ApprovalProfile
- product_mode: request | assisted | full_access
- decision_owner: user | pinvou | runtime
- control_strength: pinvou_enforced | runtime_enforced | partial | unsupported
- native_mode: string
- sandbox_profile: string?
- residual_guards: string[]
- capability_evidence_version: string
```

四级控制强度定义：

- `pinvou_enforced`：Pinvou 能在执行前收到结构化请求并决定 allow/deny，或工具由 Unified Tool Gateway 执行；
- `runtime_enforced`：Runtime 自身 sandbox、规则或分类器执行策略，Pinvou 能配置和观察但不拥有逐次决策；
- `partial`：只有部分工具、权限类型或路径可拦截；
- `unsupported`：当前接口无法证明可在执行前阻止，不得伪装为已控制。

初始 Adapter 映射遵循权限调研矩阵：

- Codex：app-server；
- Claude Code：permission prompt MCP tool 或 Agent SDK 审批接口；
- CodeBuddy：ACP 或 Agent SDK；
- Kimi Code：ACP、Server API 或 Agent SDK；Kimi `auto` 不映射为“帮我批准”，完全访问优先映射 `yolo`。

TUI 提供 `/permissions` 展示产品模式、原生映射、决策主体、控制强度、覆盖工具和剩余保护。

## 8. Unified Tool Plane

Adapter 只翻译 Runtime 协议，不定义工具业务语义。统一工具契约由 Controller/Node 侧 Tool Registry 和 Gateway 持有，包含名称、Schema、版本、副作用、幂等语义、权限需求和执行结果。

工具分为：

1. Portable Tools：由 Pinvou Gateway 执行，可形成完整 `pinvou_enforced` 审批与审计；
2. Observed Native Tools：Adapter 能归一化事件，但控制强度按实测标记；
3. Runtime Private Tools：原生压缩、隐藏推理、私有子 Agent 等，不伪装成可移植工具。

所有 Runtime 工具事件投影为统一状态：

```text
queued -> awaiting_approval -> running -> succeeded | failed | cancelled | uncertain
```

审批决定必须绑定 Session、Attachment epoch、tool call ID 和参数 digest。已成功完成的工具只恢复结果，不重新执行；结果不确定的非幂等调用标为 `uncertain` 并等待人工处理。

## 9. 架构与模块边界

```text
pinvou CLI command router
        |
        v
pinvou-tui
  Action -> Model -> Update -> View
        |
        v
authenticated Controller IPC client
        |
        v
pinvou-controller -> local pinvou-node -> Runtime Host -> Agent Adapter
```

`pinvou-tui` 是独立 crate，只依赖 Controller IPC client、协议 ViewModel 和终端库；不得依赖 Controller Core、Store、Node、Agent Adapter、Tauri、Desktop 或任何 `codewhale-*` crate。

建议模块：

```text
tui/
  app          # Tokio 事件循环与页面/overlay 路由
  action       # Terminal、Controller、Timer 动作
  model        # 纯 UI 状态与焦点/滚动状态
  update       # 确定性状态转换
  view         # Ratatui 渲染
  terminal     # Crossterm guard、能力检测与恢复
  controller   # 窄 IPC client adapter
  commands     # slash command 解析与补全
  screens/chat
  overlays/resume
  overlays/runtime
  overlays/model
  overlays/permissions
  widgets      # transcript、composer、tool、diff、status
```

终端事件只能由一个任务读取。后台 IPC、事件订阅和计时器只产生 `Action`；渲染线程不执行 Agent、文件、网络或插件工作。

## 10. 事件、重绘与恢复

TUI 消费 Controller 的只读 snapshot 和事件 cursor：

1. 初次连接获取 ViewModel snapshot 与 cursor；
2. 订阅 cursor 之后的 durable 事件和实时增量；
3. 高频文本/思考/tool output 增量按最大帧率合并渲染，完整事件仍由 Controller 持久化；
4. 断线后保留只读画面并重连；
5. cursor 仍有效时补放离线事件；
6. cursor 过期或 compacted 时重新获取 snapshot，再从新 cursor 订阅。

TUI 退出等价于 detach，不终止 Controller、Node 或活动任务。明确 stop/cancel 必须发送单独命令。异常断开也按 detach 处理。

## 11. 错误与安全恢复

- 所有正常退出、Ctrl+C、panic 和初始化失败都必须恢复 Raw Mode、备用屏幕、光标和输入模式；
- Controller 断线时禁止新的副作用命令，已有画面保持可读；
- Runtime/Model handoff 在 Prepare、压缩、能力探测或目标启动失败时不改变活动 epoch；
- 请求批准模式下，TUI/Controller 断开时 pending approval 保持等待或保守拒绝；
- 帮我批准模式下，只有已持久化、未过期且与精确 request digest 匹配的策略可以继续；
- 完全访问模式下 Runtime 可继续，但重连后必须补齐审计事件；
- 无法证明工具是否执行成功时，不自动重放非幂等操作；
- Runtime 不支持 pending approval 恢复时，明确显示取消、重试或从 checkpoint 恢复选项。

## 12. 第一阶段性目标

### 12.1 控制面合同

- 模型目录、模型选择和选择方式能力；
- 默认 Runtime/模型的 Workspace 级持久选择；
- 当前 Workspace 最近 Session 查询与恢复；
- TUI ViewModel snapshot、cursor 订阅和 cursor-expired 恢复；
- `ApprovalProfile` 与 Adapter 控制强度报告。

### 12.2 TUI 外壳

- `pinvou` TTY 默认启动与非 TTY 安全失败；
- Terminal Guard、单一事件源、Resize、焦点、滚动和退出；
- Claude Code 风格 transcript、composer、状态栏和 overlay 基础组件。

### 12.3 Codex 真实闭环

- 当前目录新会话；
- 多轮聊天和流式文本/思考；
- 统一工具事件、内联审批和取消；
- `/resume`、`/runtime`、`/model`、`/permissions`；
- TUI 退出重开后的会话视图恢复；
- 至少一次真实工具任务与可核验结果。

达到 12.3 后可称“Pinvou TUI 对 Codex 可用”，但不能称完整阶段 3 完成。

### 12.4 多 Runtime 验收

Claude Code、CodeBuddy 实现同一 Adapter 合同后，原样复用 Codex TUI 用例。Kimi Code 进入后续 Adapter 批次。TUI 不出现 `if runtime == ...` 的产品行为分支；必要差异通过统一 capability/ViewModel 呈现。

## 13. 测试合同

### 13.1 单元与属性测试

- Action/Model 状态转换、slash command、焦点、滚动和 overlay；
- 默认 Runtime/模型解析；
- handoff Prepare/Commit 和失败不变性；
- ApprovalProfile 映射、未知能力 fail closed；
- 工具状态机和 request digest 绑定。

### 13.2 PTY 与终端合同

- TTY 无参数进入 TUI；
- 非 TTY 无参数快速失败；
- help/version/子命令不误入 TUI；
- 正常退出、Ctrl+C、panic、初始化失败后终端恢复；
- Resize、最低尺寸、低色彩和无 Unicode 降级。

### 13.3 Controller/TUI 集成测试

- Fake Controller 流式事件、审批、输入请求和取消；
- 断线重连、cursor 补放、cursor expired -> snapshot + resubscribe；
- Runtime/Model handoff 成功、Prepare 失败和 Commit 恢复；
- TUI detach 后任务继续，重新连接恢复 ViewModel。

### 13.4 真实 Runtime 验收

第一阶段性目标必须用真实 Codex 完成多轮聊天、工具审批、取消、模型切换、恢复和真实工具任务。完整阶段 3 再使用真实 Claude Code 与 CodeBuddy 重复同一套产品用例；mock 只用于故障注入，不能替代真实验收。

## 14. 明确不做

- 不复用或启动 CodeWhale TUI；
- 不让 TUI 直接创建 Runtime 进程、写 Store 或读取 Adapter 私有状态；
- 不在第一切片实现 Nodes、Remote Node、Tasks、Collaboration、Jobs、Plugins、Resources、Settings 全页面；
- 不把 Runtime 原生 `auto`、`yolo`、`bypass` 等同名/相近模式机械映射；
- 不以行式 `You:` 输入循环、假事件或静态 Demo 作为 TUI 完成状态；
- 不因 TUI 开发修改 Desktop、Tauri 或 CodeWhale 构建路径。

## 15. 验收结论用语

- 只有 TUI 外壳：称“终端框架完成”，不能称“可用”；
- 12.3 全部通过：称“Pinvou TUI 对 Codex 可用”；
- Codex、Claude Code、CodeBuddy 全部复用同一真实用例通过：称“阶段 3 完成”；
- Remote Node 在阶段 4 单独验收，不反向混入阶段 3 完成条件。
