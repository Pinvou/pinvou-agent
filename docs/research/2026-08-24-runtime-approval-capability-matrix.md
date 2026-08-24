# Agent Runtime 权限与审批能力矩阵

> 检索日期：2026-08-24
> 调研对象：OpenAI Codex CLI、Claude Code、CodeBuddy Code、Kimi Code CLI
> 证据边界：仅使用官方文档、官方仓库源码/协议说明，以及本机第一方 CLI `--help`。不采用博客和二手评测。
> 本文性质：为 Pinvou TUI 和 Runtime Adapter 的权限设计提供事实依据，不代替正式实现规范。

## 1. 结论

Pinvou 可以向用户提供统一的三种权限体验：

1. **请求批准**：有副作用或需要提权的动作暂停并交给用户决定。
2. **帮我批准**：Pinvou 或 Runtime 的安全策略自动处理低风险请求；高风险、越界或无法判断的请求仍阻止或询问用户。
3. **完全访问**：启用该 Runtime 所能提供的最高权限，尽量不中断，但仍受操作系统、企业策略和 Runtime 硬保护约束。

这三项应该是 **Pinvou 的稳定产品语义**，而不是四个 Runtime 原生枚举的简单别名。各 Runtime 的原生模式数量、命名和边界都不相同；尤其是 `auto`：Claude Code 和 CodeBuddy 使用独立分类器做安全判断，Kimi Code 的 `auto` 是无人值守且不再询问用户，含义并不等价。

四个 Runtime 都能接入三模式，但成立有前提：

- Codex 必须使用 `app-server`；
- Claude Code 应使用 Agent SDK 的审批回调与 `PreToolUse`，或非交互模式的 permission prompt MCP tool；
- CodeBuddy 应使用 ACP 或 Agent SDK；
- Kimi Code 应使用 Agent SDK、Server API 或 ACP；
- 单纯启动 headless/`stream-json` 并传入一个原生 mode，只能说明 Runtime 执行了自己的策略，不能宣称 Pinvou 真正拦截了审批。

因此，UI 不应只显示 `enforced/unsupported` 一个布尔值，而应同时显示产品模式、实际映射、控制主体和剩余保护。例如：

```text
帮我批准 · Claude Code · Runtime 安全审查
请求批准 · Codex · Pinvou 可拦截
完全访问 · CodeBuddy · 仍保留灾难命令保护
```

## 2. 控制强度定义

| 标记 | 定义 | UI 含义 |
|---|---|---|
| `pinvou_enforced` | 工具执行前，Pinvou 能收到结构化请求并决定 allow/deny；或工具本身由 Pinvou Unified Tool Gateway 执行 | “Pinvou 可拦截” |
| `runtime_enforced` | Runtime 的 sandbox、规则或分类器负责决定；Pinvou 可以设置模式并观察结果，但并非每次决策都经过 Pinvou | “Runtime 安全审查” |
| `partial` | 只有部分工具、部分权限类型或部分启动配置可被拦截；存在绕过审批回调的 allow 规则、原生工具或未验证路径 | “部分受控”并列出缺口 |
| `unsupported` | 当前接口无法提供该产品语义，或无法证明在执行前可阻止动作 | 禁止选择或明确降级 |

`pinvou_enforced` 还必须满足以下条件：

- Adapter 是该 Runtime 的唯一控制客户端；
- 启动配置不会让用户已有的 allow/bypass 规则跳过 Pinvou 的审批通道，或者 Adapter 有独立的 `PreToolUse`/policy hook 覆盖这些路径；
- 内建工具、MCP 工具、子 Agent 工具和后台任务分别经过合同测试；
- 未知审批类型 fail closed，不能当普通日志忽略；
- 审批请求和批准结果绑定 request/tool-call digest，不能把旧批准复用于被替换的调用。

Pinvou Unified Tool Gateway 自己执行的工具可以始终标记 `pinvou_enforced`。Runtime 原生工具则必须按 Adapter 的实际机器接口和测试结果逐项报告。

## 3. 总览矩阵

| Runtime | 原生权限能力 | 首选机器接口 | 请求批准 | 帮我批准 | 完全访问 | 断线/恢复结论 |
|---|---|---|---|---|---|---|
| Codex CLI | approval policy 与 sandbox 两条独立轴；还支持自动 reviewer | `codex app-server` 双向 JSON-RPC | `pinvou_enforced` | Pinvou 审批器时 `pinvou_enforced`；Codex auto reviewer 时 `runtime_enforced` | `danger-full-access + never`，`runtime_enforced` | 有 thread history/read/resume，但没有通用事件 cursor 精确补放合同；Pinvou 必须保留 WAL/cursor |
| Claude Code | `default`、`acceptEdits`、`plan`、`auto`、`dontAsk`、`bypassPermissions` | Claude Agent SDK；CLI fallback 为 permission prompt MCP tool | SDK 正确接线后 `pinvou_enforced` | 原生 `auto` 为 `runtime_enforced`；Pinvou 自审需 `default + canUseTool/PreToolUse` | `bypassPermissions`，仍有极端删除熔断与企业策略 | 审批回调依赖活动进程；未找到待审批跨进程重启的保证，按 fail closed 处理 |
| CodeBuddy | `default`、`acceptEdits`、`auto`、`dontAsk`、`plan`、`bypassPermissions`；规则优先级细化 | ACP 或 Agent SDK；可选 `--serve` ACP/HTTP | 合同测试通过后 `pinvou_enforced` | 原生 `auto` 为 `runtime_enforced`；Pinvou 自审可用 ACP/SDK | `bypassPermissions`，仍有危险命令/保护路径/组织策略 | ACP 支持 session load/history；无 durable exactly-once cursor 保证，Pinvou 自行对账 |
| Kimi Code | `manual`、`yolo`、`auto`；`auto` 是无人值守语义 | Agent SDK 或 Server API；ACP 可作为互操作路径 | `pinvou_enforced` | 应保持 `manual`，由 Pinvou 回答 approval；不要映射为 Kimi `auto` | `yolo` 更贴近“最高权限但仍可问问题”；`auto` 属无人值守 | Server API 原生支持 cursor replay 与 snapshot/resync；SDK/stdio ACP 仍需 Pinvou WAL |

## 4. OpenAI Codex CLI

### 4.1 原生模式

本机 Codex CLI 0.139.0 的 `--help` 显示：

- 审批策略：`untrusted`、`on-request`、`never`，以及已废弃的 `on-failure`；
- sandbox：`read-only`、`workspace-write`、`danger-full-access`；
- `--dangerously-bypass-approvals-and-sandbox` 同时跳过审批与 Codex sandbox。

这证明审批策略和执行隔离是两条独立轴。“完全访问”不能只切换 approval policy，也必须明确 sandbox/permission profile。

官方 [`codex app-server` 协议](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md) 支持在 `thread/start`、`turn/start` 和 thread settings 中指定/更新 model、cwd、approval policy、sandbox 或 permission profile，以及 approval reviewer。

### 4.2 外部审批和工具控制

`app-server` 在 shell、文件修改和额外权限请求发生时向客户端发起 server-initiated JSON-RPC request；客户端可返回 `accept`、`acceptForSession`、策略 amendment、`decline` 或 `cancel`。因此 Pinvou 作为唯一 app-server client 时，可以在执行前真正决定这些请求，属于 `pinvou_enforced`。[官方 Approvals 协议](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md#approvals)

Codex 还支持 client-owned dynamic tools：Runtime 发出工具调用请求，客户端执行并返回结果。这比让 Runtime 直接执行 Pinvou 工具更容易形成统一审计和权限边界。[官方 Dynamic tool calls](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md#dynamic-tool-calls)

MCP elicitation 和 MCP tool approval 同样可通过 app-server 进入客户端；协议还携带 session/always persistence hint。Pinvou 仍应在 Gateway 内执行最终策略，不能因为 MCP server 被 Runtime 加载就默认信任。

限制：本机 help 明确 native web search 没有逐次审批；此外如果把 approval reviewer 交给 Codex 的自动 reviewer，请求可能由 Runtime 内部处理而不再发给 Pinvou。因此：

- **请求批准**：使用用户 reviewer，并由 Pinvou 显示/回答 app-server 请求；
- **帮我批准**：首选仍使用用户 reviewer，由 Pinvou policy engine 自动回答低风险请求；若直接启用 Codex auto reviewer，应显示 `runtime_enforced`；
- **完全访问**：映射为 `danger-full-access + never` 或明确的危险 bypass；文案必须写“Runtime 最高可用权限”，不能暗示绕过 OS/企业策略。

### 4.3 Headless 与断线

`codex exec --json` 适合一次性 JSONL 自动化，不适合需要实时审批的 TUI。TUI Adapter 应使用 app-server 的 stdio/受保护本地 transport。

app-server 支持 thread start/resume/read/list、turn start/interrupt 和流式 item 生命周期；持久 thread 可以重新读取历史。但官方没有提供与 Pinvou 事件日志等价的“任意断点、精确 cursor 补放”保证。因此重连后必须以 Controller WAL 为主，读取 Runtime snapshot/history 对账；不能宣称 Runtime 原生 exactly-once。[官方 thread 生命周期](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md#lifecycle-overview)

## 5. Claude Code

### 5.1 原生模式

Claude Code 当前提供：

- `default`：除读取外通常询问；
- `acceptEdits`：自动文件编辑和常见文件操作，其他动作仍询问；
- `plan`：以只读探索和计划为主；
- `auto`：独立分类器在后台审核动作；
- `dontAsk`：未预先允许的动作直接拒绝；
- `bypassPermissions`：跳过权限层，仅保留有限硬熔断和外部约束。

详见官方 [Permission modes](https://code.claude.com/docs/en/permission-modes)。其中 `auto` 是 research preview，受账号、模型、Provider 和管理员策略限制；分类器会阻止越界、破坏性和可疑外部操作，但官方明确不保证安全。

### 5.2 外部审批和工具控制

Claude Agent SDK 的 `canUseTool` 在未被规则或模式预先处理的调用上触发，返回 allow/deny；流式 session 还能动态切换 permission mode。[SDK Permissions](https://code.claude.com/docs/en/agent-sdk/permissions)；[处理审批和用户输入](https://code.claude.com/docs/en/agent-sdk/user-input)

仅有 `canUseTool` 还不足以声称覆盖所有调用：已有 allow 规则、`acceptEdits`、`auto` 或 bypass 路径可能在回调前完成决策。若要让 Pinvou 的硬策略覆盖全部 Runtime 原生工具，应同时使用 `PreToolUse` hook，或为 Adapter 提供隔离、经过净化的 settings，并逐工具验证。

CLI 非交互模式还提供 `--permission-prompt-tool`，把未解决的权限询问交给指定 MCP tool，并返回结构化 allow/deny；这是可用 fallback，但 SDK 的生命周期和取消能力更适合作为主 Adapter。[Claude CLI reference](https://code.claude.com/docs/en/cli-usage)

建议映射：

- **请求批准**：`default + canUseTool`，关键硬策略再用 `PreToolUse`；
- **帮我批准**：若采用 Claude 原生 `auto`，显示“Runtime 安全审查”；若要求 Pinvou 统一判断，则保持 `default`，由 Pinvou 在回调中自动 allow/deny/ask；
- **完全访问**：`bypassPermissions`，同时展示管理员禁用、极端路径熔断和 OS 权限等 residual guards。

### 5.3 Headless、MCP 与断线

Claude Code 支持 `-p --output-format stream-json --input-format stream-json` 的多轮流式输入输出。CLI 也支持 MCP；Agent SDK 可注册进程内 MCP/custom tools。对 Pinvou 自有工具，最好由 Pinvou 进程持有实现，从而在工具执行点形成 `pinvou_enforced`。[Agent SDK MCP](https://code.claude.com/docs/en/agent-sdk/mcp)

审批回调依赖活动 SDK/CLI 进程。官方资料未承诺“待审批状态可在进程退出后恢复并重新发出”。因此连接丢失时 Pinvou 不应自动批准或猜测结果，应保持自己的 `awaiting_approval` 记录，重新 attach 后通过 Runtime 状态对账；无法证明调用未执行时，非幂等动作不得自动重放。

## 6. CodeBuddy Code

### 6.1 原生模式

CodeBuddy 提供 `default`、`acceptEdits`、`auto`、`dontAsk`、`plan`、`bypassPermissions`。它的实际决策是分层的：deny、可信 allow、危险命令检查、ask、mode baseline、non-interactive fallback 等共同决定结果；`auto` 只处理最后仍为 ask 的动作，显式 ask/deny 规则优先。[Permission Rules](https://www.codebuddy.ai/docs/cli/permissions)；[Identity and Access Management](https://www.codebuddy.ai/docs/cli/iam)

`auto` 使用分类器，并允许用户/本地/CLI 提供环境、allow、soft deny、hard deny 语义；因此它适合“Runtime 帮我批准”，但决策权不在 Pinvou。[Settings / Auto Mode](https://www.codebuddy.ai/docs/cli/settings#auto-mode-configuration)

### 6.2 外部审批和工具控制

CodeBuddy 有两条合适的主路径：

1. ACP：官方明确用于外部 Agent Client，支持实时流、工具代理和 permission request interaction。[ACP Integration](https://www.codebuddy.ai/docs/cli/acp)；[IDE Integration](https://www.codebuddy.ai/docs/cli/ide-integrations)
2. Agent SDK：`canUseTool(toolName, input)` 返回 allow/deny/interrupt，并支持动态切换 permission mode。[SDK Permission Control](https://www.codebuddy.ai/docs/cli/sdk-permissions)

ACP 只有在合同测试证明内建工具、MCP、子 Agent 和后台任务的待审批动作都会产生 permission request 后，才能标记 `pinvou_enforced`；否则先标 `partial`。SDK callback 的可控性更明确，但同样要检查预先 allow/bypass 是否跳过回调，并用 `PreToolUse` 或隔离 settings 补齐。

建议映射：

- **请求批准**：`default`，由 Pinvou 回答 ACP/SDK request；
- **帮我批准**：若直接使用 CodeBuddy `auto`，显示 `runtime_enforced`；若需要统一 Pinvou 策略，则仍使用 `default`，由 Pinvou 自动回答；
- **完全访问**：`bypassPermissions`/`-y`，同时展示灾难命令、保护路径和组织策略仍可能阻止动作。

MCP 工具同样参与 allow/ask/deny；外部 MCP host 或 MCP Apps 不能绕开 Pinvou Gateway 的二次策略。[CodeBuddy MCP](https://www.codebuddy.ai/docs/cli/mcp)

### 6.3 Headless 与断线

CodeBuddy 的 `-p --output-format stream-json --input-format stream-json` 能流式工作，但非交互模式里无法展示的 ask 会转为拒绝/阻断；因此不适合作为 TUI 的交互审批主通道。[Headless Mode](https://www.codebuddy.ai/docs/cli/headless)

首选 ACP；`--serve` 还公开 REST 与基于 HTTP/SSE 的 ACP，并具备鉴权和 session API。[HTTP API](https://www.codebuddy.ai/docs/cli/http-api)

ACP 支持 session load/history 恢复，但官方没有给出 durable exactly-once event cursor 保证。连接中断后，Pinvou 仍需依据自己的 WAL/cursor 恢复视图并与 Runtime history 对账。

## 7. Kimi Code

### 7.1 原生模式

当前 Kimi Code 使用 `manual`、`yolo`、`auto`：

- `manual`：需要批准的动作暂停询问；
- `yolo`：自动批准普通工具调用，但 Agent 仍可能向用户提问，计划退出仍有独立确认语义；
- `auto`：完全无人值守，工具审批自动处理，Agent 也不再等待用户回答。

详见官方 [Configuration files](https://moonshotai.github.io/kimi-code/en/configuration/config-files) 和 [`kimi` command](https://moonshotai.github.io/kimi-code/en/reference/kimi-command)。因此 Kimi `auto` 不能映射为 Pinvou“帮我批准”；它改变的不只是工具审批，还包括用户输入行为。

### 7.2 外部审批和工具控制

Kimi Agent SDK 的低层 Session API 直接流出 `ApprovalRequest`，调用方必须 resolve 为 `approve`、`approve_for_session` 或 `reject`；未 resolve 会阻塞回合，close 会取消进行中的 prompt 并清理工具。[Kimi Agent SDK Session API](https://github.com/MoonshotAI/kimi-agent-sdk/blob/main/guides/python/session.md)

Kimi Server API 提供本地 REST + WebSocket：会话可逐次指定 permission mode，事件流包含 approval 请求/解决，客户端可处理 pending approval；同时它原生支持 durable event seq、断线 cursor replay、`resync_required` 和 snapshot 恢复。接口当前仍应视为可演进面，Adapter 启动时应读取 live `/openapi.json` 和 `/asyncapi.json` 做版本校验。[Kimi Server API](https://moonshotai.github.io/kimi-code/en/reference/server-api.html)

Kimi ACP 同样提供 `session/request_permission`，并支持 allow once/always/reject、session cancel、模型/模式配置项和 MCP 转发，适合作为标准互操作路径。[`kimi acp`](https://moonshotai.github.io/kimi-code/en/reference/kimi-acp)

建议映射：

- **请求批准**：保持 `manual`，所有 approval request 交给 Pinvou/用户；
- **帮我批准**：仍保持 `manual`，由 Pinvou policy 自动回答低风险请求；不要切为 Kimi `auto`；
- **完全访问**：优先映射 `yolo`，因为它只降低工具审批摩擦而不主动关闭用户问答；无人值守应作为未来独立产品状态，不塞进“完全访问”。

Kimi 内建工具与 MCP 工具共享审批机制；MCP 工具未命中 allow rule 时进入审批，适合统一投影。[Kimi MCP](https://moonshotai.github.io/kimi-code/en/customization/mcp.html)

### 7.3 Headless 与断线

`kimi -p --output-format stream-json` 默认采用无人值守语义，不具备交互审批闭环，所以不能作为 Pinvou TUI 的主 Adapter。Agent SDK、Server API 或 ACP 才能提供 `pinvou_enforced`。

在四个 Runtime 中，Kimi Server API 对断线恢复的公开合同最完整：durable event 可按 `{seq, epoch}` 补放，超出缓冲区后要求 snapshot/resubscribe；volatile delta 需要按 offset 发现缺口并回快照。即便如此，Pinvou Controller 仍应保持自身事件 WAL，因为 Runtime journal 不能替代跨 Runtime 的统一恢复合同。

## 8. Pinvou 统一审批合同建议

### 8.1 产品模式与 Adapter 映射分离

不要只定义一个 `ApprovalPolicy` 枚举。建议状态至少包含：

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

其中：

- `request` 默认 `decision_owner=user`；
- `assisted` 可以是 `decision_owner=pinvou`，也可以在用户明确接受 Runtime 自带分类器时为 `runtime`；
- `full_access` 只表示 Runtime 最高可用权限，不等于管理员权限、突破容器/OS 或忽略企业策略；
- 用户切换模式时必须展示实际映射和剩余保护，不得只换一个 UI 标签。

### 8.2 Adapter 能力报告

每个 Adapter 在启动和版本变化后至少报告：

```text
approval_request_roundtrip
approval_session_scope
native_policy_switch
sandbox_profiles
native_tools_intercepted
mcp_tools_intercepted
subagent_tools_intercepted
background_tools_intercepted
client_owned_tools
pending_approval_resume
event_cursor_replay
snapshot_recovery
audit_completeness
```

能力不能仅靠静态版本号推断；安装后需跑不产生真实副作用的协议探测，并把探测结果与 Runtime 版本缓存。未知能力按 unsupported/partial 处理。

### 8.3 “帮我批准”的实现原则

建议优先级：

1. Pinvou Unified Tool Gateway 自有工具：始终由 Pinvou policy 决策和执行；
2. Runtime 能发完整 pre-execution request：保持 Runtime 的询问模式，由 Pinvou 自动回答；
3. Runtime 只有可靠的原生分类器：允许使用，但 UI 标记 `runtime_enforced`；
4. 只有启动参数、日志或事后事件：不得显示“Pinvou 已控制”，只能标 `partial/observed`；
5. 未知、高风险、请求内容不完整、策略服务不可用：fail closed，拒绝或转人工，不自动批准。

这能保留各 Runtime 的成熟 sandbox/permission system，同时让 Pinvou 拥有稳定产品语义和清晰责任边界。

### 8.4 断线规则

“不自动批准”应精确定义为：

- `request` 模式下，TUI/Controller 断开时 pending approval 保持等待或被保守拒绝；
- `assisted` 模式下，只有 Controller 已持久化、仍在有效期内、与精确 request digest 匹配的 Pinvou 预授权策略可以继续；
- `full_access` 模式下，Runtime 可以按其最高权限继续，但审计事件必须在重连后补齐；
- 不确定工具是否已经执行时，禁止自动重放非幂等调用；
- Runtime 不支持 pending approval 恢复时，Adapter 报 `pending_approval_resume=unsupported`，重连后由用户决定取消、重试或从 checkpoint 恢复。

## 9. 对阶段 3 TUI 的直接影响

第一版权限 UI 可以保持三项，但详情面板必须展示真实状态：

```text
请求批准
  Runtime: Codex
  原生映射: on-request + workspace-write
  控制主体: Pinvou
  控制强度: 可拦截（shell / file change / permission request）
  未覆盖: native web search 无逐次审批
```

`/permissions` 或模式选择面板应能展开：

- 当前 Runtime 原生 mode/sandbox；
- 谁在做决定：用户、Pinvou、Runtime classifier；
- 哪些工具是 `pinvou_enforced`、`runtime_enforced` 或 `partial`；
- “完全访问”仍存在的 OS/管理员/企业/Runtime 硬保护；
- 断线后会等待、拒绝还是继续。

默认仍建议“请求批准”。“帮我批准”按 Workspace/Session 显式开启；“完全访问”需要风险确认，不静默成为全局默认。

## 10. 来源索引

### OpenAI Codex

- [Codex app-server README](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md)
- [Codex app-server protocol schema](https://github.com/openai/codex/blob/main/codex-rs/app-server-protocol/schema/json/codex_app_server_protocol.v2.schemas.json)
- [Codex approval policy prompt implementation](https://github.com/openai/codex/tree/main/codex-rs/prompts/templates/permissions/approval_policy)

### Claude Code

- [Permission modes](https://code.claude.com/docs/en/permission-modes)
- [Agent SDK permissions](https://code.claude.com/docs/en/agent-sdk/permissions)
- [Agent SDK user input and approvals](https://code.claude.com/docs/en/agent-sdk/user-input)
- [CLI reference](https://code.claude.com/docs/en/cli-usage)
- [MCP in Agent SDK](https://code.claude.com/docs/en/agent-sdk/mcp)

### CodeBuddy Code

- [Identity and Access Management](https://www.codebuddy.ai/docs/cli/iam)
- [Permission Rules](https://www.codebuddy.ai/docs/cli/permissions)
- [SDK Permission Control](https://www.codebuddy.ai/docs/cli/sdk-permissions)
- [ACP Protocol Integration](https://www.codebuddy.ai/docs/cli/acp)
- [HTTP API](https://www.codebuddy.ai/docs/cli/http-api)
- [Headless Mode](https://www.codebuddy.ai/docs/cli/headless)
- [MCP](https://www.codebuddy.ai/docs/cli/mcp)

### Kimi Code

- [Kimi Code permission configuration](https://moonshotai.github.io/kimi-code/en/configuration/config-files)
- [`kimi` command](https://moonshotai.github.io/kimi-code/en/reference/kimi-command)
- [`kimi acp`](https://moonshotai.github.io/kimi-code/en/reference/kimi-acp)
- [Server API](https://moonshotai.github.io/kimi-code/en/reference/server-api.html)
- [Kimi Agent SDK Session API](https://github.com/MoonshotAI/kimi-agent-sdk/blob/main/guides/python/session.md)
- [Kimi MCP](https://moonshotai.github.io/kimi-code/en/customization/mcp.html)
