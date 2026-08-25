# Multica 控制外部 Agent CLI 的方式（洁净室调研）

调研日期：2026-08-18

调研基线：Multica `main` 提交 [`14c2e4e831e3658fe5df3d06b5f6dfe461ca78df`](https://github.com/multica-ai/multica/commit/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df)

## 结论

Multica **不是统一通过 ACP 控制所有 Agent CLI**。它在本机 daemon 内为不同 CLI 实现不同的协议适配器，再把各协议产生的消息统一为内部 `Backend / Session / Message / Result` 模型。ACP 只是适配器家族之一；Claude Code、CodeBuddy、Codex、Pi 等均使用各自的原生非交互协议。[统一运行时接口与消息模型](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/pkg/agent/agent.go#L15-L230)；[各运行时的启动协议标签](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/pkg/agent/agent.go#L425-L452)。

ACP 只存在于“本机 daemon ↔ 本机 Agent CLI”这一段。daemon 与 Multica 服务端之间另走 HTTP 控制接口与 WebSocket 通知，不使用 ACP。[daemon 客户端的任务领取和消息上报](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/internal/daemon/client.go#L210-L270)；[官方 daemon 调度说明](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/apps/docs/content/docs/daemon-runtimes.mdx#dispatch-and-online-status)。

这对 Pinvou 最有价值的不是某段代码，而是一个产品级判断：应建立稳定的 `AgentRuntimeAdapter` 边界，ACP 原生 Agent 使用 ACP adapter，其他 Agent 使用各自的官方 headless/streaming 接口；不能把所有 CLI 强行包装成 ACP，也不能把节点协议做成 ACP。

## 设备上如何发现 Agent CLI

Multica daemon 维护一份已知 CLI 清单。每个 provider 有默认命令名和 `MULTICA_*_PATH` / `MULTICA_*_MODEL` 覆盖项，主要通过 `exec.LookPath` 查找；GUI 启动导致 PATH 不完整时，会调用用户 login shell 做缓存式补充解析；Codex 在 macOS 还有 Desktop app 内置 CLI 的特殊查找路径。[探测实现](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/internal/daemon/agents_probe.go#L49-L176)。

探测不是只在启动时做一次：当前实现会周期性重扫可用 CLI 和版本，发现变化后更新已注册 runtime。[周期刷新机制](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/internal/daemon/agents_refresh.go#L9-L76)。官方文档把一个 runtime 定义为“某台电脑 + 某个具体 AI 工具（或兼容协议 profile）”，同一电脑安装多个 CLI 时会注册多个 runtime。[daemon 与 runtime 的关系](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/apps/docs/content/docs/daemon-runtimes.mdx#daemon-vs-runtime)。

## 如何启动和控制不同 Agent

### 与 Pinvou 目标直接相关的 Agent

| Agent | Multica 当前任务传输 | 是否 ACP | 会话恢复方式 |
|---|---|---:|---|
| Claude Code | 启动 CLI 的 `stream-json` 输入/输出模式，stdin/stdout 双向流 | 否 | CLI 原生 resume 参数；session id 从事件中取得。[源码](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/pkg/agent/claude.go#L33-L52) |
| CodeBuddy | 与 Claude 风格相近的 `stream-json` 模式 | 否（任务执行） | CLI 原生 resume 参数。[源码](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/pkg/agent/codebuddy.go#L15-L47) |
| Codex | 启动 `codex app-server`，经 stdin/stdout 使用 Codex 自己的 JSON-RPC | 否 | `thread/resume`，失败时可回退 `thread/start`，再用 `turn/start` 驱动回合。[传输说明](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/pkg/agent/codex.go#L257-L265)；[会话启动/恢复](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/pkg/agent/codex.go#L1789-L1872) |
| Pi | 启动非交互 JSON mode，stdout 输出事件；session 文件路径本身作为 session id | 否 | 重用 CLI 管理的 session 文件。[源码](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/pkg/agent/pi.go#L18-L22) |
| Gemini CLI | 当前 `main` 已无该 runtime | 当前不适用 | 2026-06-24 的提交明确移除了 Gemini runtime。[移除提交](https://github.com/multica-ai/multica/commit/76c58a4ee860b1b4a0fd57ac72997b6743a4fcf8) |

CodeBuddy 有独立的 ACP 探测用于模型目录 fallback，但其正常任务执行仍是 `stream-json`，不能据此归类为 ACP runtime。[CodeBuddy 任务 adapter](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/pkg/agent/codebuddy.go#L15-L47)。

### 当前明确使用 ACP 的 Agent

当前仓库中明确走 ACP stdio JSON-RPC 的包括：

- Hermes：`hermes acp`，同时承载共享 ACP client 的主要实现。[源码](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/pkg/agent/hermes.go#L246-L273)
- Kimi：`kimi acp`。[源码](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/pkg/agent/kimi.go#L30-L69)
- Reasonix：ACP adapter。[源码](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/pkg/agent/reasonix.go#L32-L60)
- Kiro：`kiro-cli acp`。[源码](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/pkg/agent/kiro.go#L33-L66)
- Qoder / Qoder CN：全局 `--acp` 模式。[源码](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/pkg/agent/qoder.go#L26-L103)
- TRAE CLI：`traecli acp serve`。[源码](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/pkg/agent/traecli.go#L34-L113)
- Grok 与 QwenPaw 也通过各自 CLI 的 ACP/stdio 模式运行。[Grok](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/pkg/agent/grok.go#L54-L145)；[QwenPaw](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/pkg/agent/qwenpaw.go#L23-L91)

它们的公共流程大体是：启动子进程并接好 stdin/stdout；执行 `initialize`；按能力做可选认证；创建或恢复 session；可选设置 model/config；发送 `session/prompt`；消费 `session/update`；响应权限请求；最后把 session id、输出、用量和错误归一化。各 CLI 在恢复方法、启动参数、权限选项和模型配置上仍有差异，所以即使都叫 ACP，也不是零适配成本。[共享 ACP 请求/通知处理](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/pkg/agent/hermes.go#L869-L1013)；[session update 归一化](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/pkg/agent/hermes.go#L1396-L1439)。

ACP 官方将其定位为编辑器/客户端与 coding agent 的互操作协议，当前稳定 wire protocol version 为 1；协议仓库提供独立 schema/SDK，许可为 Apache-2.0。[ACP 官方仓库](https://github.com/agentclientprotocol/agent-client-protocol)；[ACP 官方架构说明](https://agentclientprotocol.com/get-started/architecture)。Pinvou 若实现 ACP，应以这些官方资料和具体 CLI 的官方文档为依据，而不是以 Multica adapter 为规范。

## 任务、会话与事件如何流动

Multica 的高层链路是：服务端保存任务并通知目标 daemon；daemon 也会轮询作为断线后的兜底；daemon 领取任务、准备工作区、创建对应 adapter、启动本机 CLI，并把 CLI 原生事件转换为统一消息。[官方 dispatch 说明](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/apps/docs/content/docs/daemon-runtimes.mdx#dispatch-and-online-status)；[公共 `Session` / `Message` / `Result`](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/pkg/agent/agent.go#L135-L230)。

统一消息至少区分 text、thinking、tool-use、tool-result、status 和 error。daemon 为持久化消息分配单调序号，将连续文本/思考增量合并，并按固定时间窗口批量上报；工具调用和结果保持结构化。任务结束前等待消息尾部落盘。[daemon 的事件 drain 与批量上报](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/internal/daemon/daemon.go#L7572-L7875)；[消息上报 HTTP 接口](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/internal/daemon/client.go#L410-L427)。服务端对消息再做处理、持久化并广播给 UI。[服务端 ingest 与广播](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/server/internal/handler/daemon.go#L4250-L4341)。

会话恢复依赖 adapter 返回的原生 session id。Multica 保存这个 id，后续同一 issue 再运行时尝试恢复；失败则根据 adapter 能力与错误类型创建新会话。官方文档也明确指出，恢复要求原会话仍存在且执行 runtime 能访问它；Pi 因 session 是本地文件，更依赖原设备。[官方 session resumption 说明](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/apps/docs/content/docs/providers.mdx#session-resumption)。

## 对 Pinvou 架构的可用事实（非代码借鉴）

1. `Node Protocol` 与 `Agent Runtime Protocol` 必须分离。ACP 只适合控制支持 ACP 的 Agent 进程，不能承担设备发现、配对、租约、资源上报和跨节点调度。
2. `AgentRuntimeAdapter` 应是统一契约，但 adapter 内允许 ACP、Codex app-server、stream-json、JSONL 等不同 transport。
3. “检测到 CLI”不等于“可正常工作”。节点 capability 还应包含版本、认证/健康状态、支持的会话能力、模型发现方式、交互审批能力和协议版本。
4. 逻辑会话必须由 Pinvou 主设备保存；CLI 原生 session id 只是可替换的 runtime attachment。Multica 的原生 resume 机制可以作为兼容性事实，但不能满足 Pinvou 已确定的跨设备、跨 Agent 会话迁移语义。
5. 高频事件应先规范化并带序号，再做短窗口合并和批量持久化；工具事件、审批和状态事件不能只当文本处理。具体数据模型应从 Pinvou 的断线重放、主设备权威账本和实时 UI 要求独立推导。

## 许可证与商业使用边界

“Multica 完全不可商用”并不准确。当前许可证是完整 Apache-2.0 文本加 Part I 附加条件组成的 **Multica License**：它允许商业使用和单一组织内部使用；但未经商业许可，禁止使用 Multica 源码向第三方提供 hosted service，也禁止把 Multica 作为组件嵌入销售、许可或商业分发给第三方的产品/服务。即使免费向组织外用户提供公开托管实例，也属于受限用途。[LICENSE Part I 1(a)](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/LICENSE#L1-L65)。

使用其 UI 派生代码还受 logo、产品名和版权/归属信息保留条件约束；只用 backend、daemon 或 CLI 而不用 UI 时，若再分发或运营基于它的产品/服务，也要保留 notice，并在用户文档声明产品 built on Multica 及链接。派生或再分发还必须交付完整 Multica License，而非只带 Apache-2.0。[LICENSE Part I 1(b)-3](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/LICENSE#L66-L175)；[NOTICE](https://github.com/multica-ai/multica/blob/14c2e4e831e3658fe5df3d06b5f6dfe461ca78df/NOTICE)。

因此 Pinvou 这种计划商业分发的产品不应复制、移植、翻译或逐行改写 Multica 的 daemon/adapter 实现。若未来确实要纳入任何 Multica 源文件或形成其派生实现，应先取得商业许可并做正式法律审查。本段是工程风险识别，不构成法律意见。

## 洁净室实施边界

- 本文仅记录公开可观察行为、协议选择和产品边界，不包含 Multica 实现代码。
- Pinvou adapter 接口、状态机、事件 schema、重试策略和测试用例均应从 Pinvou 已确认的需求独立设计。
- ACP adapter 只依据 ACP 官方规范、官方 SDK/schema 和各 Agent CLI 的官方文档实现。
- 非 ACP adapter 只依据对应 CLI 的官方 headless/API 文档与 Pinvou 自己的黑盒兼容测试实现。
- 后续设计文档可以引用本文的“事实结论”，但不应把 Multica 源码结构或函数流程直接转写成 Pinvou 代码结构。
