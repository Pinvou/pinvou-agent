# Codex ACP 整体架构决策

> 文档状态：架构已确认，MVP 已在 `feat/codex-acp` 实现，待体验复核
> 评审范围：架构与产品边界，不代表已实现
> 记录日期：2026-07-23
> 对应分支：`feat/codex-acp`
> 当前原型基线：`2b84908b`（实现已继续演进，最终提交号待收口）
> AionUI 参考基线：`1b215f2f`，AionCore `v0.1.50` / `10cdd579`

## 1. 一句话决策

pinvou 采用“**Codex 作为 Agent 内核，pinvou 作为 ACP Host 和产品外壳**”的方案：

- Codex 负责自身 system prompt、原生 tools、tool loop、上下文和原生 skill 发现。
- pinvou 负责 ACP 进程托管、会话管理、权限交互、事件持久化和对话 UI。
- CodeWhale 继续走现有 Engine / ToolRegistry / SkillRegistry / `chat:*` 链路，不因 Codex ACP 重构而改变原有行为。
- 主页输入区统一提供“工作 / 代码”入口；两类会话在左侧按时间混排，通过图标区分。
- Codex ACP 使用独立的事件模型和对话展示层，不再把 ACP 事件压成 CodeWhale 的消息和工具卡片。

这个方案吸收 AionUI 对 ACP 的成熟处理方式，但不引入 AionCore、Electron、HTTP/WS 服务层和 Agent 市场等额外体系。

本次 MVP 只做 Codex 原生能力的完整接入，不接入 pinvou skill、MCP、知识库和 persona。上述 pinvou 扩展能力保留为后续阶段。

## 2. 前置澄清

### 2.1 视觉和交互的分工

前文截图是 **pinvou 当前调用 Codex ACP 的效果**，它反映的是当前实现问题，不是 Codex 原生 UI。

因此：

1. 不以该截图的视觉密度和操作方式作为目标。
2. pinvou 的品牌、颜色、字体、导航、窗口、输入框外观和整体视觉体系保持不变。
3. Codex/AionUI 只作为会话内部内容结构和交互语义的参考，包括 reasoning、tool step、diff、permission、plan 和状态更新方式。
4. 不像素级复制 Codex 或 AionUI；AionUI 只作为 ACP 信息完整度的最低验收线。

### 2.2 ACP 不等于“所有内容都由 Codex 接管”

ACP 是 Host 与 Agent 之间的协议。Codex 提供 Agent 能力，pinvou 仍然必须完成进程托管、事件还原、权限响应、UI 展示和扩展配置。

正确边界是：**Codex 原生能力不重造，pinvou 产品能力不缺席。**

## 3. 目标和非目标

### 3.1 MVP 目标

- 在 Linux 开发/内部体验环境中可安装、可登录、可创建和恢复 Codex ACP 会话。
- 在同一个 pinvou 主窗口的主页提供“工作 / 代码”入口；左侧统一展示两类会话，不再通过现有 CodeWhale 输入框的 backend chip 或独立 Codex Tab 切换。
- 完整展示 ACP 中的回答、思考状态、tool call/update、plan、permission、model、mode、config、usage 和错误。
- 会话内部的内容结构和操作方式参考 Codex/AionUI，整体视觉继续使用 pinvou 设计体系。
- 保留 Codex 自己的 system prompt、tools、tool loop、skills、MCP 配置和上下文能力。
- 不新增跨后端能力兼容，但必须保证 CodeWhale 现有会话、工具、skill、知识库、远程控制和历史记录零回归。

### 3.2 MVP 非目标

- 不在 pinvou 里重写 Codex 的 tool loop、system prompt 或上下文压缩。
- 不把 CodeWhale `ToolRegistry` 中的工具硬塞进 Codex 工具循环。
- 不接入 pinvou skill、pinvou MCP、知识库和 persona。
- 不做 Codex 富事件的远程控制兼容；现有 CodeWhale 远程控制保持不变。
- 不新增独立操作系统窗口；MVP 使用同一主窗口内的“代码”模式。
- 不要求正式发布所需的应用隔离 Node runtime 在 MVP 内同步完成。
- 不做通用 Agent 市场，也不一次性支持所有 ACP Agent。
- 不复制 AionUI 的后端服务、数据库和整套前端。

### 3.3 后续阶段

- 用户显式选择的 pinvou skill 通过 Codex 原生 skill 机制接入。
- pinvou 工具和知识库通过 MCP 接入 Codex。
- persona 作为可见、可选的首次 prompt 扩展接入，不替换 Codex system prompt。
- 正式 Linux x64 / arm64 发布包内置 Pinvou 应用隔离 Node runtime，用户无需安装 Node/npm。
- Codex 富事件逐步接入远程控制。
- 如有真实并排使用需求，再提供“弹出为独立窗口”，不改变同窗口“代码”模式作为默认入口。

## 4. 当前 pinvou 原型的真实状态

当前 `feat/codex-acp` 已经打通了最小可运行链路：

- 安装/发现 `@agentclientprotocol/codex-acp` 运行时。
- 复用 `codex login` 登录态。
- 启动 ACP 子进程并通过 stdio 连接。
- `initialize`、`session/new`、`session/load`、`prompt`、`cancel`、模型切换已接通。
- pinvou session 与 ACP session ID / model 的映射已持久化。
- 当前原型曾在 CodeWhale 输入框的 chip 中选择 Codex，创建后锁定 backend。目标设计改为主页“工作 / 代码”模式；“代码”当前选择 Codex，底层会话仍永久绑定 `codex-acp`，不能中途切换成 CodeWhale。

但它仍然是“通路原型”，不是可交付的完整 ACP 架构：

| 问题 | 当前实现 | 直接后果 |
|---|---|---|
| 事件被压扁 | `EventBridge` 把 ACP 转成旧 `chat:delta/tool_start/tool_end` | tool kind、locations、分段 content、中间状态等信息丢失 |
| 思考事件丢失 | `AgentThoughtChunk` 直接忽略 | 只能显示笼统的“思考中”，无法还原 Codex/AionUI 节奏 |
| 权限语义错位 | Plan 自动拒绝，YOLO 自动允许 | 用户看不到真实 permission request，合作模式与安全授权被绑死 |
| 新会话无 pinvou MCP 注入 | `NewSessionRequest::new(workspace)` 没有 `mcp_servers` | 不影响 MVP；后续 pinvou MCP 接入阶段需要补齐 |
| skill/persona 沿用旧注入 | ACP 分支前已把 skill body / persona body prepend 到用户消息 | MVP 必须关闭这条旧注入，确保只保留 Codex 原生能力 |
| UI 复用过度 | Codex 仍走 `ChatView` 和旧 `ToolCard` | 工具输出过密、信息层级混乱，也无法表达 ACP 状态 |
| 持久化不够 | 仅保存 ACP session ID / model，历史 UI 仍依赖旧 messages | 重启后无法准确恢复思考、工具、权限和 plan 时间线 |

结论：**当前代码适合保留作为运行时 PoC；MVP 必须重构事件、权限、持久化和 UI，并明确关闭 pinvou skill/persona 等旧注入。pinvou 扩展能力后置，不作为 MVP 阻塞项。**

## 5. AionUI 怎么做，和 pinvou 有什么差异

### 5.1 AionUI 的实现特点

AionUI 并不会替换 Codex 自带的 Agent 内核。它的做法是：

1. **独立 ACP 对话面**
   `AcpChat.tsx` / `useAcpMessage.ts` 单独处理 ACP 运行状态、thinking、tool、permission 和 usage，不用普通 chat 减法适配。

2. **保真事件翻译**
   `protocol/events/translate.rs` 保留 tool call 的 `kind/status/raw_input/raw_output/content/locations/meta`，并单独翻译 thinking、plan、commands、mode、config、session info 和 usage。

3. **权限请求可交互、可恢复**
   `permission_router.rs` 把待处理请求以 `tool_call_id` 登记，UI 选择 ACP 提供的 option 后再回传；会话恢复时能重新查询 pending confirmations。

4. **MCP 是 session 扩展**
   AionUI 从数据库读取启用或会话选中的 MCP，按 Agent capability 过滤，再通过 `session/new.mcp_servers` 交给 ACP Agent。

5. **Codex skill 走原生目录**
   Agent metadata 声明 native skill dirs，用户选择的 skill 会物化/链接到 workspace 的 Codex skill 目录，由 Codex 自己发现。

6. **persona/rules 不替换 system prompt**
   AionUI 把 `preset_context` 作为第一次用户 prompt 的 `[Assistant Rules]` 前缀，而不是改写 Codex 内部 system prompt。

7. **非原生 skill Agent 才使用文本降级**
   AionUI 的 `[LOAD_SKILL: name]` 是无 native skill 能力时的兼容逻辑，不是 Codex 接入应优先复制的机制。

### 5.2 对比结论

| 维度 | pinvou 当前原型 | AionUI | pinvou 目标方案 |
|---|---|---|---|
| 定位 | 最小 Codex ACP 通路 | 通用 ACP 产品平台 | 聚焦 Codex 的 ACP Host |
| 后端 | Tauri/Rust 内直接托管 | Desktop + AionCore 完整服务层 | 保留 Tauri/Rust，补齐 ACP RuntimeManager |
| 事件 | 压成旧 `chat:*` | ACP 类型化事件 | 版本化 `acp:event`，保留完整语义 |
| UI | 复用 CodeWhale ChatView/ToolCard | 独立 ACP surface | 独立 ACP surface，共享外壳和基础组件 |
| 权限 | Plan/YOLO 自动决定 | 待处理列表 + UI 回复 | PermissionBroker + 可恢复内联卡片 |
| MCP | 无 session 注入 | 按 capability 注入 | MVP 只保留 Codex 自身配置；pinvou session MCP 后置 |
| skill | 直接 prepend skill body | Codex 用 native dir，其他 Agent 有降级 | MVP 只保留 Codex 原生发现；pinvou skill 后置 |
| prompt | skill/persona 混在用户输入 | preset context 仅扩展首次 prompt | MVP 不注入；后续 persona 也不改 Codex system prompt |
| 持久化 | session 映射 + 旧 messages | 独立会话/事件体系 | ACP state + timeline sidecar + 旧消息兼容投影 |
| 复杂度 | 低 | 高 | 中等，只补齐 Codex 必需层 |

**哪个更优：**

- 只看“最快打通”，当前 pinvou 原型更轻。
- 看 ACP 完整度、交互可用性和后续扩展，AionUI 当前更成熟。
- 看 pinvou 的长期维护成本，**本文的混合方案最优**：学 AionUI 的 ACP 语义和会话管理，但不把 AionUI 的平台复杂度搬进 pinvou。

## 6. 目标总体架构

```mermaid
flowchart TB
  Window[PINVOU 主窗口]
  Nav[主导航<br/>品悟对话 / Codex / 知识库 / 工具 / 设置]
  Shell[pinvou ChatShell<br/>窗口・Markdown・产物・主题]

  Window --> Nav
  Nav --> Shell
  Shell --> DS[WorkConversationSurface]
  Shell --> ACPUI[CodexAcpConversationSurface]

  DS --> Engine[CodeWhale EnginePool<br/>原有 chat:* 链路]

  ACPUI --> Timeline[AcpTimeline / Reasoning / ToolStepGroup]
  ACPUI --> Permission[PermissionCard / Plan / RuntimeStatus]
  ACPUI --> Composer[AcpComposer]

  Timeline --> Router[Tauri Command + acp:event]
  Permission --> Router
  Composer --> Router

  Router --> Runtime[AcpRuntimeManager]
  Runtime --> Adapter[CodexAdapter]
  Runtime --> Aggregate[AcpSessionAggregate]
  Runtime --> Broker[PermissionBroker]
  Runtime --> Persistence[AcpPersistence]

  Adapter --> ACP[@agentclientprotocol/codex-acp]
  ACP --> Codex[Codex CLI / Agent]

  Runtime -. 后续阶段 .-> Extensions[ExtensionAssembler]
  Extensions -.-> Skills[pinvou skills -> Codex native dirs]
  Extensions -.-> MCP[pinvou MCP / 知识库]
  Extensions -.-> Persona[persona first-prompt extension]
```

### 6.1 入口与页面决策

- 主页输入区提供“工作 / 代码”模式，不在主导航增加独立 Codex Tab。
- “工作”保持原有品悟输入框；“代码”当前只有 Codex，未来可扩展其他代码 Agent。
- CodeWhale 与 Codex 会话在左侧统一列表中按最近更新时间混排，Codex 会话名前显示代码图标。
- Codex 使用专属 timeline、输入框和运行状态，不复用 CodeWhale `ChatView` 的消息语义。
- 代码草稿默认选择临时目录，首条发送时才物化会话；输入框下方也可选择项目目录或最近项目。
- 现有 CodeWhale 输入框不再显示 Codex backend chip。
- 每个 Codex 会话在数据层永久绑定 `codex-acp`，不能中途切换为 CodeWhale。
- 后续可增加“弹出为独立窗口”，但它只是同一代码模式的一种承载方式，不是另一套会话实现。

### 6.2 分层原则

#### A. 共享产品外壳，不共享消息语义

可共享：

- 主窗口、顶层导航、主题和窗口控制。
- Markdown 渲染、文件预览、artifact panel。
- 主题 token、基础按钮、弹层、无障碍能力。
- 顶层页面路由。
- 附件选择的基础组件；是否能发送以及支持哪些类型，以 Codex ACP capability 为准。

不共享：

- CodeWhale 和 Codex 的消息数据、输入框逻辑与运行时状态；左侧列表只合并各自的会话摘要。
- 现有输入框中的 backend 选择 chip。
- CodeWhale `ToolCard` 与 ACP tool step。
- CodeWhale message reducer 与 ACP timeline reducer。
- `pendingAssistantBlocks`、`toolMeta`、CodeWhale 专属 selector。
- 用旧 history messages 重建 ACP 运行时时间线的逻辑。

#### B. 后端内部可扩展，产品上只发 Codex

`AcpRuntimeManager` 使用内部 adapter 边界，避免以后再接 ACP Agent 时重写进程和事件层；但第一期只实现 `CodexAdapter`，不做市场化抽象。

## 7. 能力归属决策

| 能力 | 主责 | pinvou 的职责 | 禁止做法 |
|---|---|---|---|
| Codex base system prompt | Codex | 不修改，仅记录当前 Agent/版本 | 注入 CodeWhale `instructions.md` 替换它 |
| Codex 原生 tools | Codex | 保真显示 ACP tool event | 在 pinvou 重写 tool loop |
| Codex tool loop | Codex | 回应 permission，支持 cancel | 在前端推测下一步工具 |
| Codex context/compaction | Codex | 显示 usage，保存 ACP session ID | 用 CodeWhale 压缩逻辑接管 |
| Codex 原生 skills | Codex | MVP 不注入 pinvou skill，不干扰 Codex 自己发现 | 默认 prepend pinvou skill body |
| Codex 自身 MCP 配置 | Codex | MVP 保留并验证实际可用性 | 为了简化 Host 而屏蔽 Codex 原生能力 |
| pinvou skill | 后续扩展 | 通过 Codex 原生 skill 目录物化，MVP 不做 | 在 MVP 中继续沿用旧 prompt 注入 |
| pinvou 自定义工具 | 后续 MCP server | 选择、配置并通过 ACP session 注入，MVP 不做 | 混入 CodeWhale ToolRegistry 后再伪装成 Codex tool |
| 知识库 | 后续 pinvou MCP | 提供 `kb_search` 等 MCP tool，MVP 不做 | 每轮在 prompt 里塞检索片段/工具引导 |
| persona / assistant rules | 后续可选扩展 | 仅第一次 prompt 用明确标记的规则前缀，MVP 不做 | 声称替换了 Codex system prompt |
| 权限决策 | 用户 + ACP option | 呈现、等待、回传、持久化决策 | Plan 就全拒绝，YOLO 就全允许 |

## 8. Skill、MCP、Tools 和 System Prompt 的具体方案

### 8.1 MVP：只保留 Codex 原生能力

MVP 默认行为：

- Codex system prompt、tools、tool loop、context/compaction 由 Codex 自己负责。
- Codex 使用自己能够发现的 skills；pinvou 不扫描、选择、物化或注入 bundle skills。
- Codex 使用自身配置中实际生效的 MCP；pinvou 不在 `session/new` 中追加 MCP。
- pinvou 只解释 ACP tool call/update、呈现 permission 并支持 cancel，不接管工具执行。
- capability 没有上报的 model、mode、config、附件或工具开关不在 UI 中伪造。

MVP 验收时需要用 pinvou 固定的 `codex-acp` / Codex CLI 版本验证 Codex 原生 skill 和 MCP 确实可用；不能只根据配置文件存在就判定已经接入。

### 8.2 后续：pinvou skills

- 用户在 pinvou 中明确“加持/启用”某个 skill 时，pinvou 才把它物化或安全链接到该 workspace 的 Codex 原生 skill 目录。
- 会话保存 skill 选择快照，不因全局 skill 启停而在中途漂移。
- 不通过首轮 prompt 塞入完整 skill body，也不为 Codex 发明 `[LOAD_SKILL]` 文本协议。

> 实现前需再次验证固定 Codex CLI 版本的原生 skill 目录和 capability。AionUI 当前使用 `.codex/skills`，但不把这个路径当成永不变的协议常量。

### 8.3 后续：pinvou MCP 和知识库

- 用户在 pinvou 创建会话时选中的 MCP，以及知识库等内建 MCP，通过 ACP session 扩展接入。
- `ExtensionAssembler` 按 ACP Agent 的 MCP capability 过滤 transport 类型。
- 处理同名 server 冲突，不静默覆盖。
- 在 `session/new` 和 `session/load` 时使用同一份会话快照。
- 记录生效和失败的 MCP 状态，供 UI 展示。
- 对敏感 env 只保存引用，事件和日志中脱敏。

### 8.4 Tools

- Codex 自带 shell、文件编辑、搜索等工具完全由 Codex 管理。
- pinvou 只解释 ACP tool call/update 并提供权限 UI。
- pinvou 专属工具后续优先做成独立 MCP server。
- 前端必须保留 `tool_call_id`，将多次 update 以 upsert 方式合并，不把一个工具过程渲染成多张重复卡片。

### 8.5 System prompt / persona

- Codex base system prompt 是 Codex 内部实现，pinvou 不覆盖、不拼接 CodeWhale 的 `instructions.md`。
- MVP 不向 Codex 注入 pinvou persona。
- 后续用户显式加持 persona / assistant rules 时，仅在首次真实用户消息前加一段明确边界的扩展文本，并在 UI 中让用户看到本会话有额外规则。
- 后续 persona 仍是“首次用户 prompt 扩展”，不对外称为 system prompt。

## 9. ACP 事件合同

新链路使用单一、版本化的 Tauri 事件：

```json
{
  "version": 1,
  "sessionId": "pinvou-session-id",
  "turnId": "turn-id",
  "seq": 42,
  "timestamp": 1784780000000,
  "event": {
    "type": "tool_upsert",
    "data": {}
  }
}
```

首期必须支持的 `event.type`：

- `runtime_status`
- `turn_started`
- `agent_text_delta`
- `reasoning_delta`
- `reasoning_completed`
- `tool_upsert`
- `permission_requested`
- `permission_resolved`
- `plan_updated`
- `capabilities_updated`
- `model_updated`
- `mode_updated`
- `config_updated`
- `usage_updated`
- `turn_completed`
- `turn_cancelled`
- `error`
- `unknown`

合同规则：

1. 每个会话的 `seq` 单调递增，前端按 `seq` 去重和排序。
2. tool call/update 都转成 `tool_upsert`，并保留 kind、status、title、raw input/output、content、locations 和 meta。
3. ACP 新增未识别事件时，用 `unknown` 保留脱敏后的 raw payload，不静默丢弃。
4. 大对象、base64、密钥和过长 terminal output 在进入前端/持久化前做大小限制和脱敏。
5. 旧 `chat:*` 事件只服务 CodeWhale 链路和过渡期兼容，不再作为 ACP 的主数据模型。

## 10. 权限模型

### 10.1 解耦两组概念

| 概念 | 回答的问题 | 例子 |
|---|---|---|
| Collaboration mode | Agent 应该怎样工作 | plan / default |
| Security permission | 这一次高风险操作能不能执行 | allow once / allow always / reject once / reject always |

Plan 不再等于“所有 permission 自动拒绝”，YOLO 也不再等于“所有 permission 自动通过”。

### 10.2 PermissionBroker

`PermissionBroker` 需要：

- 以 `session_id + turn_id + tool_call_id` 作为完整标识。
- 展示 Agent 传来的工具摘要、可选 option 和风险信息。
- 精确回传用户选中的 ACP `option_id`。
- 对过期、重复、跨会话回复直接拒绝。
- UI 重载后向运行时查询 pending requests 并恢复卡片。
- Agent 进程退出时把待处理项标记为 `expired`，不显示伪 pending。

MVP 不硬编码 `workspace-write + on-request` 这类未经当前 codex-acp capability 验证的名称，而是：

- 使用 Codex ACP 实际上报的默认 mode。
- Agent 发出 permission request 时必须真实展示给用户，不自动允许或拒绝。
- UI 只展示 Agent 实际提供的 permission options，并原样回传用户选择的 `option_id`。
- 全权限模式只有在 Agent 明确上报且用户主动选择时才启用，不由会话名称或 Plan/YOLO 隐式切换。
- 权限模式以 Agent 的 `config_options.mode` 为唯一优先控制面；只有 Agent 未上报
  该配置时才回退 `session/set_mode`。Prompt 不再重复携带本地 mode。
- 用户确认的权限模式属于 `desired` 会话状态，必须持久化；ACP
  `session/new`、`session/load` 和进程重连后先重新应用并等待确认，再开放发送。
- Turn 运行期间禁止切换权限模式；配置请求使用
  `requested / applied / failed` 事件记录，避免无法判断某一轮实际使用了什么模式。

## 11. Codex ACP 对话 UI

### 11.1 组件边界

```text
features/codex/
├─ CodexAcpConversationSurface
├─ AcpEventTimeline（不可变事实源）
├─ Thread / Turn / Item 投影
├─ TurnPresentation（仅做视觉聚合）
├─ ReasoningItem
├─ ToolStepGroup
│  ├─ CommandExecutionItem
│  ├─ FileChangeItem
│  └─ GenericToolItem
├─ PermissionItem
├─ PlanItem
├─ AgentMessageItem
├─ RuntimeStatus
├─ AcpComposer
└─ acp-state.js
```

`Thread → Turn → Item` 是会话展示的稳定语义边界：

- 一个 Pinvou Codex session 对应一个 Thread。
- 一次用户输入到 `turn_completed` 对应一个 Turn。
- reasoning、command execution、file change、generic tool、permission、plan 和
  agent message 分别对应不同 Item。
- Item 遵循 `started → delta/update → completed/failed` 生命周期。
- `ToolStepGroup` 只在 presentation 层聚合相邻操作；底层 Item 和
  `tool_call_id` 不合并、不丢失。

### 11.2 交互决策

- **回答**：保留阅读宽度，不把每段文本放进大卡片。
- **思考**：使用可折叠的轻量区域，显示 ACP 实际传出的 reasoning/thought 摘要和持续时间；不伪造 Codex 未传出的隐藏思维链。
- **运行状态**：Turn 运行期间始终显示“正在处理 · 时长”；等待权限时切换为“等待授权 · 时长”。计时由事件时间戳推导，重载后不会归零。
- **工具**：同一 `tool_call_id` 始终是一个 Item，运行中就地更新。默认显示标题、类型、状态和最关键摘要，详细命令/输出/diff 再展开。
- **命令**：从 ACP `rawInput.command/cwd` 和
  `rawOutput.formatted_output/exit_code` 提取产品字段；普通界面不直接展示
  ACP JSON。`completed` 表示调用生命周期结束，不文案化为“所有子命令成功”。
- **文件变更**：优先显示文件列表和 diff，可跳转到 artifact/file preview。
- **权限**：在工具 step 下原位显示，不用普通文本或 toast 代替。
- **Plan**：是会话内的动态状态块，可展示 pending / in progress / completed。
- **输入框**：展示 Codex Agent、model、reasoning effort/mode、当前权限策略和运行状态；只展示 Agent capability 真正支持的选项。
- **附件**：复用 pinvou 的选择/预览基础组件，但只允许发送 Codex ACP 实际支持的内容类型；不支持时不显示伪入口。
- **异常**：安装、登录、断线、超时、进程退出和会话恢复失败都使用可操作状态，而不是混入 assistant 正文。

### 11.3 视觉验收方法

视觉和交互验收分两步：

1. 先按本文完成 ACP 信息架构和交互语义，达到 AionUI 级别的完整度。
2. 再用真实 Codex/AionUI 的对话、tool、permission、plan、composer 作为内容结构和交互节奏对照；颜色、字体、间距、导航和整体外观继续服从 pinvou 设计体系。

目标不是像素复制 Codex，而是避免“整体看起来仍是 pinvou，但 ACP 状态和交互语义是假的”。

## 12. 会话状态与持久化

### 12.1 AcpSessionAggregate

每个 Codex ACP 会话保留三类状态：

- `desired`：MVP 保存 pinvou/用户希望的 model、mode 和 permission policy；后续再加入 pinvou skills、MCP、persona 快照。
- `observed`：ACP Agent 实际回报的 capabilities、model、mode、config、usage、pending permission。
- `advertised`：当前可向 UI 展示和允许操作的最终集合。

当 `desired` 与 `observed` 不一致时，UI 显示“切换中/未生效/不支持”，不提前伪装成已成功。

运行时状态至少包含：

```text
not_installed -> installing -> starting -> authenticating -> ready
ready -> running -> waiting_permission -> running -> ready
running -> cancelling -> ready
* -> disconnected / failed / shutting_down
```

### 12.2 持久化分层

```text
~/.pinvou3/sessions/<pinvou-session-id>/
├─ acp-state.json
└─ acp-timeline.jsonl
```

- `acp-state.json`：MVP 保存 ACP session ID、adapter/Agent 版本、model、mode、permission policy 和最后状态；后续再加入 pinvou skill/MCP/persona 快照。
- `acp-timeline.jsonl`：脱敏后、可恢复 UI 的版本化 `acp:event`。

三个真相源的职责：

| 内容 | 真相源 |
|---|---|
| Codex 上下文与继续对话 | ACP session / Codex |
| pinvou 丰富时间线 | `acp-timeline.jsonl` |
| 旧列表、搜索、远程摘要兼容 | `SavedSession.messages` 投影 |

后续为列表、搜索和远程摘要兼容生成 `SavedSession.messages` 投影：用户文本、最终 assistant 文本、必要的工具摘要。它不承担恢复 ACP reasoning、permission、tool update 和 plan 的职责。

只持久化 ACP 实际向 Host 发送、且 UI 可见的 reasoning/thought 内容；不存储或推测 Agent 未发送的隐藏思维链。

### 12.3 项目目录与会话目录

代码模式新建会话时必须由用户明确选择一种 workspace：

| 类型 | Codex 执行目录 | 适用场景 |
|---|---|---|
| 项目会话 | 用户选择的真实项目目录 | 日常按仓库使用 Codex |
| 临时会话 | `~/.pinvou3/sessions/<id>/workspace/` | 临时问答、试验或不希望关联项目 |

目录边界如下：

- Pinvou 会话状态、ACP timeline 和 UI 数据始终保存在
  `~/.pinvou3/sessions/<id>/`，不写入真实项目。
- 项目会话把 canonical absolute path 保存在 `session-agents.json`；ACP adapter
  进程 cwd、`session/new` 和 `session/load` 始终使用这一路径。
- 一个项目可以创建多个独立 Codex 会话；workspace 不是 session 的替代品。
- 会话第一次启动后 workspace 不可修改；切换项目必须新建会话，避免 Codex 上下文跨仓库漂移。
- 恢复项目会话时目录不存在或不可访问，必须明确报错；禁止自动退回临时目录。
- 旧 Codex ACP 记录没有 workspace 字段时按临时会话兼容，不要求迁移。
- Codex 继续使用用户的 `HOME` / `~/.codex` 管理登录、全局配置、原生 skills、MCP
  和 Codex 自身记忆；Pinvou 不复制或注入自身记忆。

## 13. 运行时、安装与发布

`RuntimeInstaller` 继续复用当前原型已有的路径解析、版本锁定、登录检查和启动逻辑，但要明确区分：

- 已内置可用运行时。
- MVP 开发/内部体验环境中可用的 system Node 20+。
- adapter 未内置时需要 npm 下载/修复的回退路径。
- Codex 未登录。
- adapter 与 Codex CLI 版本不兼容。

阶段决策：

- MVP 开发和内部体验阶段允许依赖机器已经安装的 Node 20+，优先完成 ACP 协议、会话和 UI。
- 如果 adapter/node_modules 已随包提供，运行时只需要 Node；npm 仅用于未内置或损坏后的安装回退。
- 正式 Linux 发布按 x64 / arm64 分架构打包 Codex ACP adapter、Codex CLI、Pinvou 应用隔离 Node runtime 及所需 native dependencies。
- 正式用户不需要理解或手工安装 Node/npm。
- 保留 `PINVOU3_CODEX_ACP_BIN` 作为开发/诊断覆盖，不作为普通用户安装步骤。

## 14. 远程控制和兼容

- CodeWhale 现有远程协议不改。
- MVP 不接入 Codex 富事件远程控制，也不要求老远程端展示 Codex 会话。
- 后续会话 snapshot 增加 `backend: "codex-acp"` 标识，最终 assistant 文本可进入旧兼容摘要。
- 后续 reasoning、tool、permission 和 plan 使用单独、版本化的 ACP timeline 数据，新远程端按 capability 渐进支持。
- 后续远程 permission response 必须验证 session / turn / tool call / pending 状态，不允许仅用 tool call ID 跨会话回复。

## 15. 分阶段实施

### 阶段 0：锁定协议和交互样本

- 固定 pinvou 使用的 codex-acp / Codex CLI 版本。
- 录制真实 ACP fixture：text、thinking、tool start/update/end、diff/location、permission、plan、model/mode/config、usage、cancel/error。
- 采集真实 Codex/AionUI 对照图，明确需要参考的内容结构和交互；整体视觉继续使用 pinvou 品牌样式。

### 阶段 1：ACP 合同和测试夹具

- 定义 `acp:event v1`。
- 完成 `CodexAdapter`、`AcpEventTranslator`、`AcpSessionAggregate` 边界。
- 用 fixture 做 Rust 翻译测试和前端 reducer 测试。
- 保证 unknown event 和大输出不会让应用崩溃。

### 阶段 2：会话、权限和持久化

- 实现 runtime lifecycle 和 desired/observed/advertised 状态。
- 实现 PermissionBroker 及 pending recovery。
- 实现 `acp-state.json` / `acp-timeline.jsonl` 原子写入和损坏容错。
- 将 collaboration mode 与 permission policy 解耦。

### 阶段 3：代码模式 ACP UI

- 在主页增加“工作 / 代码”模式，移除独立 Codex Tab 和现有 CodeWhale 输入框中的 Codex backend chip。
- 把 Codex 会话摘要并入左侧统一会话列表，底层 timeline 和运行时状态继续隔离。
- 建立 `features/chat/acp/`。
- 完成 timeline、reasoning、tool upsert、permission、plan、runtime status 和 composer。
- 按 Codex/AionUI 的内容结构和交互语义验收，并保持 pinvou 视觉体系。
- 强制回归 CodeWhale 原有对话。

### 阶段 4：原生扩展

- skill 物化/链接、快照和清理。
- MCP capability 过滤、session 注入和状态展示。
- persona 首次 prompt 扩展。
- 知识库通过 MCP 接入，不复用 CodeWhale 的 per-turn prompt 引导。

### 阶段 5：发布与全链路回归

- Linux x64 / arm64 应用隔离 Node runtime 打包与无系统 Node 安装验收。
- 远程控制渐进兼容。
- 断网、进程退出、重启、升级、会话恢复、过期 permission 回归。
- 打包体积、启动时间、长对话内存和 timeline 上限验收。

## 16. 验收门槛

### 16.1 MVP 验收

#### 基础链路

- pinvou 主页有“工作 / 代码”入口，左侧会话按时间混排，现有 CodeWhale 输入框不再承担 backend 切换。
- 未安装、安装中、未登录、登录成功、启动失败均有明确 UI。
- MVP 环境缺少 Node 20+ 时给出明确诊断，不进入伪启动状态。
- 新建会话、继续会话、切换会话、重启应用后恢复可用。
- 新建会话可明确选择真实项目目录或临时目录；同项目可创建多个会话。
- 项目会话的 adapter cwd、`session/new`、`session/load` 均为所选目录，恢复时不漂移。
- 项目目录丢失时明确报错，不静默退回临时目录。
- model、mode、config option 以 Agent 回报的 capability 为准。
- 附件只在 capability 和实际发送链路支持时显示。

#### 对话与工具

- 回答流式文本无丢字、重复和乱序。
- thinking/reasoning 过程和完成状态正确。
- 同一 tool call 的多次 update 只显示为一个就地更新的 step。
- command、file change/diff、search/location，以及 Codex 自身 MCP 产生的 tool event 可读、可折叠、可看错误。
- cancel 不留下伪运行状态。

#### 权限

- allow once / allow always / reject once / reject always 以 Agent 实际提供的 option 为准。
- UI 重载可恢复 pending request。
- 跨会话、过期和重复回复被拒绝。
- Plan / default 切换不再隐式改写权限策略。
- Host 不自动允许或拒绝 permission request。

#### Codex 原生能力边界

- Codex 原生 skill 发现不受 pinvou 干扰。
- Codex 自身配置的 MCP 在固定版本上完成真实调用验证。
- pinvou bundle skill、MCP、知识库和 persona 均未注入 Codex 会话。
- Codex system prompt、tools、tool loop 和 context/compaction 未被 pinvou 接管。

#### 兼容

- CodeWhale 新建/历史会话、工具、skill、知识库、Plan/YOLO 原行为回归通过。
- 旧会话数据不需迁移即可读。
- CodeWhale 现有远程控制不受主页代码模式影响。

### 16.2 后续阶段验收

- pinvou skill 由 Codex 原生发现，不靠首次 prompt 塞入全文。
- pinvou MCP 和知识库能真实调用，禁用/失败状态可见。
- persona 默认不注入，显式加持后仅首次扩展。
- 正式 Linux x64 / arm64 安装包无需用户预装 Node/npm。
- 新远程端可按 capability 展示 Codex timeline；旧远程端至少能看到用户消息和最终回答。
- 如提供独立窗口，关闭、重开、多窗口事件和 permission 归属正确。

## 17. 风险与应对

| 风险 | 应对 |
|---|---|
| codex-acp / ACP schema 升级 | 固定版本 + fixture + `unknown` 事件 + adapter 边界 |
| 独立 UI 与现有 chat 重复 | 只共享外壳和 primitives，不共享语义 reducer |
| timeline 持续增长 | JSONL 大小上限、轮换/压缩、大输出截断和文件引用 |
| permission 绕过 | 服务端 PermissionBroker 验证，不信任前端传入的会话归属 |
| skill 链接污染 workspace | 会话快照、白名单路径、原子替换、可回收管理 |
| MCP 密钥泄露 | env 引用、日志脱敏、事件截断、持久化前清洗 |
| 参考 Codex 后破坏 pinvou 视觉一致性 | 只参考内容结构和交互语义，颜色、字体、导航和整体布局服从 pinvou 设计体系 |
| 正式包内置 Node 后体积增大 | x64/arm64 分架构打包并记录体积预算；MVP 先允许 system Node，不阻塞协议/UI 开发 |

## 18. 已确认的设计决策

以下内容已在首轮评审中确认：

1. Codex 原生能力做内核，pinvou 做 ACP Host、会话、权限、事件、持久化和 UI。
2. Codex ACP 和 CodeWhale 使用独立消息语义和 UI surface。
3. 同一个 pinvou 主窗口以“工作 / 代码”模式承载两类输入，左侧统一展示会话；MVP 不创建新操作系统窗口。
4. Codex 会话在数据层永久绑定 `codex-acp`，不能中途切换成 CodeWhale。
5. 视觉继续使用 pinvou 体系；Codex/AionUI 只作为会话内容结构和交互语义参考。
6. MVP 只保留 Codex 自身 system prompt、tools、tool loop、skills、MCP 配置和 context，不接入 pinvou skill、MCP、知识库和 persona。
7. pinvou 扩展统一后置：skill 走 Codex 原生目录，工具和知识库走 MCP，persona 只允许可见、可选的首次 prompt 扩展。
8. collaboration mode 与 security permission 分离；Host 不再按 Plan/YOLO 自动允许或拒绝。
9. MVP 开发/内部体验允许依赖 system Node 20+；正式 Linux x64 / arm64 包内置 Pinvou 应用隔离 Node runtime，用户无需安装 Node/npm。
10. CodeWhale 跨后端能力兼容后置，但现有 CodeWhale 功能必须零回归。
11. 不引入 AionCore，不重构 CodeWhale 底座。
12. Codex workspace 在创建会话时明确选择项目或临时目录，开始后不可更换；Pinvou 会话数据与项目目录分离。

实施前仍需通过现场能力探测确认，而不是产品层拍板的项目：

- 当前固定 codex-acp / Codex CLI 实际上报的 mode、config 和 permission options。
- Codex 原生 skill 与自身 MCP 配置在 ACP 会话中的真实可用性。
- ACP 支持的附件内容类型；不支持的类型不显示入口。

## 19. 参考代码落点

### pinvou 当前实现

- `pinvou3-app/src-tauri/src/features/codex_acp/mod.rs`：运行时、会话、权限、new/load/model。
- `pinvou3-app/src-tauri/src/features/codex_acp/events.rs`：ACP 原始事件持久化与页面投影。
- `pinvou3-app/src-tauri/src/features/codex_acp/store.rs`：pinvou session 到 ACP session/model/workspace 映射。
- `pinvou3-app/src-tauri/src/app/commands/codex.rs`：代码模式的 Codex Tauri 命令边界。
- `pinvou3-app/src/features/codex/CodexAcpView.jsx`：代码模式中的 Codex 对话与输入区。
- `pinvou3-app/src/platform/tauri/client.js`：前端统一 Tauri 平台适配。

### AionUI / AionCore 参考

- `packages/desktop/src/renderer/pages/conversation/platforms/acp/AcpChat.tsx`
- `packages/desktop/src/renderer/pages/conversation/platforms/acp/useAcpMessage.ts`
- `crates/aionui-ai-agent/src/protocol/events/translate.rs`
- `crates/aionui-ai-agent/src/manager/acp/permission_router.rs`
- `crates/aionui-ai-agent/src/factory/acp.rs`
- `crates/aionui-ai-agent/src/factory/acp_assembler.rs`
- `crates/aionui-ai-agent/src/capability/first_message_injector.rs`
- `crates/aionui-ai-agent/src/capability/skill_manager/prompt_builder.rs`
- `crates/aionui-conversation/src/service.rs`
- `crates/aionui-extension/src/skill_service.rs`
