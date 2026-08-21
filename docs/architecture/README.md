# PinvouOS 架构权威索引

状态：2026-08-21 已冻结 Front 插话、后台回流边界与长流内存修复；资源治理的 schema v6、HostWork 异步控制面、首版 Linux Supervisor、显式 MegaBook profile helper、固定 E2E harness 与 deb 固定 payload mode/hash 门禁已在当前工作树实现。MegaBook 的真实 deb 安装、High、OOM 与 purge 仍未执行；脚本或打包门禁存在不等于实机 E2E 通过。2026-08-18 的交互平面首阶段已在 MegaBook 完成白盒与黑盒验收，本次 checkpoint 仍需以 x86_64 release canary 单独部署验收。

这份索引不替代各专题文档，只规定它们分别对什么负责，以及发生冲突时按什么顺序裁决。

## 文档所有权

| 范围 | 唯一权威 | 说明 |
|---|---|---|
| 无 Session、Identity、Mission、Runtime Run、Event Ledger | [ADR-0007](../adr/0007-PinvouOS-无Session连续运行时.md) | 产品与运行时领域模型 |
| Resource Agent、Governor、HostWork、Linux Supervisor、cgroup 与停止/恢复权限 | [ADR-0009](../adr/0009-PinvouOS-资源治理与Host-Supervisor.md) | 资源治理和主机控制唯一权威；ADR-0007 保留历史顶层原则 |
| 长期记忆语义、证据门槛、当前实现状态 | [Memory ADR](../adr/0008-PinvouOS-Memory-Agent-整理内核.md) + [Memory HTML](pinvouos-memory-architecture.html) | Memory ADR 负责硬约束，HTML 负责可视化与状态 |
| 常驻系统角色、调用拓扑、中断权限、同步/异步关系 | [Multi-Agent HTML](pinvouos-multi-agent-collaboration.html) | 描述本地工作树；不代表 MegaBook 已部署版本 |
| Front / 交互 Agent 的能力、四路由、工具硬边界、当前与目标态 | [Front Agent HTML](pinvouos-front-agent-design.html) | 以代码 checkpoint 为基线，明确区分已运行、部分接通、Prompt 协议和目标态 |
| AG-UI、A2UI、画布写权、Host Renderer、交互迁移路线 | [Interaction HTML](pinvouos-interaction-surface-agui-a2ui.html) | 同时分开 MegaBook 部署态、本地工作树和目标态 |
| 历史原因与产品方向 | Obsidian 产品主张与项目记录 | 用于解释设计来源；已接受 ADR 和当前代码优先 |

同一事实发生冲突时，按以下顺序处理：可复现实机行为 → 当前代码与自动化测试 → 已接受 ADR → 对应专题 HTML → Obsidian 历史记录 → 未来提案。历史文档不能覆盖较新的已接受决策。

## 跨文档共同不变量

1. 用户始终面对一个 `pinvou` 身份。Front 是唯一面向用户表达、普通业务提问和最终答复的 Agent。
2. 产品领域没有 Session。旧 Session/thread/conversation 只能作为迁移期私有执行缓存，不能成为身份、任务、记忆、前端协议或账本主键。
3. Front 的 Direct 路径最多三轮工具批次；这是 Engine 防止无界工具循环的安全上限，不是前台时间预算、三秒承诺或应当用满的配额。明显复杂、涉及慢工具或需要调查→实施→验证时，Prompt 契约要求尽早通过 `agent(profile="pinvou-orchestrator")` 委派。该 happy-path 已验证，但唯一 profile 与同批单实例尚未内核化。
4. 普通 VoiceShell / chat turn 运行时，本机仍可提交语音，兼容/远端文字入口也可继续提交；Host 把新输入放入可见、可撤销的内存 FIFO，只在整个当前 turn 到达 terminal 后逐条发送。快速 Hold 先给排队项一个 Engine mailbox 线性化点；队首真正成为本地 turn 前还必须通过 `Acquire → Applied → Confirm → Confirmed(active)` 两阶段 barrier，Host 与 backend 都在 reserve、`localTurnOwned` 和乐观回答之前复核。若 detached completion 已先启动，它不会被回卷，而是先作为独立、非授权的 runtime turn 完整收口；排队项保持可见，barrier 随后才放行。VoiceShell 本身尚无文字输入框；不同 Host 各有自己的 FIFO，并不合并成全局队列。该机制不是模型请求或工具批次的实时 interrupt / steering，进程重启或页面重载也不能恢复队列；旧 plan-accept / edit 入口尚未纳入这条 turn lease 保证。
5. VoiceShell 不再按 `3 / 15 / 30` 秒显示催促用户继续说话的等待文案；顶部只根据 Runtime 的真实 `running` 状态显示持续工作动效。该展示不取消请求、不抢占工具、不自动 handoff，也不改变 Engine 状态；当前没有 `45s` 硬 lease。语音采集的开始、停止与取消不受 generation busy 控制，采集期间新 summary / artifact 延后显示。
6. Runtime Ledger 是业务事实与可恢复意图的唯一耐久来源。AG-UI state、A2UI data model、客户端 store、插话 FIFO 和 Memory projection 都不是第二真相源。
7. Policy Agent 负责评估 `Allow / Deny / RequireConfirmation`；Kernel/AuthorityStore 负责强制。Front 不能取消硬拒绝，也不能隐藏系统确认。
8. A2UI Surface 写权分区：`projection/*` 只归 Runtime Projector，`front/*` 只归 Front，`system/*` 只归 Kernel/Host。界面感知 Agent（Screen Observer Agent）只观察窗口与可访问性场景，没有 A2UI 写权。
9. Memory 不保存任务进度。Runtime Mission/Run Work Graph 保存承诺、进度、阻塞和下一步；Memory 只从可信、已验证结果形成行动经历和教训。
10. Dynamic Mission Agent 的 `completed` 只是执行自报，不等于 Mission 已验证完成。只有 Runtime 的验证回执能关闭任务成功并成为 Memory 证据。
11. Resource Agent 只观察并提交 Claim，Governor 只签确定性 `Pending` Directive；只有受信 HostWork Adapter / Supervisor 能执行控制。执行链是 `Pending → HostWorkDirectiveDispatchRecorded → ACK → status reconcile`；dispatch 事件必须在副作用前经 append + flush + `sync_data` 落盘，但它只是 attempt fence，不是成功证明。已有 marker 的 `Pending`，以及 prior-boot 遗留但没有 marker 的 `Pending`，都只能以同一 `directive_id` 进入 `OutcomeUnknown` / status-only 对账，不得重放副作用。模型、Front、Renderer 和 Resource Agent 都不能注册 HostWork，也不能提交任意 PID、unit 名、命令或 `systemctl`。当前工作树已装配 scheduled、knowledge、固定连接器、受限 detached sub-agent 与 ASR 的异步 Stop Adapter；app cgroup 只做 `essential + non-governable` 状态观测。旧 Mission 同步 callback 生产面已移除，其 Adapter 数量仍为 0。

## 标识与生命周期

| 标识 | 含义 | 约束 |
|---|---|---|
| `mission_id` | 持久目标 | 可跨多次交互与多个 Runtime Run |
| `runtime_run_id` | Mission 的一次任务执行尝试 | 可运行、暂停、恢复、完成、取消或失败 |
| AG-UI `threadId` | 交互协议范围 | Mission 工作区通常由 Mission 派生；全局 Pinvou 画面使用稳定 scope；不是 Session 领域对象 |
| AG-UI `runId` | 一次 Front—用户交互执行 | 使用独立 `interaction_run_id`，通过 correlation 关联一个或多个 Runtime Run |
| `interruptId` | 一次等待用户输入或审批 | `RUN_FINISHED` 使用 interrupt outcome；同 thread 的新 AG-UI run 通过 `resume[].interruptId` 恢复 |
| A2UI `surfaceId` | 逻辑 UI 区域 | 必须带命名空间、catalog 版本、revision、basis Runtime sequence 和幂等键 |
| `agent:screen-observer` | 界面感知 Agent | 能力为 `screen.observe`；旧账本中的 `agent:surface` / `surface.observe` 只由 schema v5 replay upcaster 与一版兼容入口读取。历史 `list_events` 仍按历史值原样返回旧标识、不做 canonical upcast，这是审计例外；canonical snapshot、当前输出和新事件只发新标识 |

AG-UI 的 interrupt 不是独立生命周期事件。正确协议是先发送恢复所需 snapshot，再发送 `RUN_FINISHED { outcome: { type: "interrupt" } }`；恢复请求使用同一 `threadId`、新的 AG-UI `runId` 和 `resume[].interruptId`。实现时以 [AG-UI Interrupts](https://docs.ag-ui.com/concepts/interrupts) 为准。

## 当前实现边界

- Memory Runtime 核心已接统一账本：细粒度 decision/checkpoint/replay、单写保护、结构热索引和有界 Top-K 可用；异步 worker、`VerifiedTaskOutcome`、Front Context Compiler、冷归档和 Obsidian 尚未闭环。
- Front → `pinvou-orchestrator` 的 manager-as-tool happy-path 与三轮 Direct 安全上限已接通；唯一 profile、同批单实例和回执 schema 仍是 Prompt 契约。2026-08-19 工作树另要求 Front 在 handoff 启动回执后结束当前 turn，避免前台轮询后台；这条“尽早释放”仍是 Prompt 契约，不是时钟驱动的 Engine lease。
- 界面感知 Agent、Device、Policy 仍处于 Starting；Policy 算法存在，但 AuthorityStore 与所有副作用工具的统一执行闸门未接。
- Resource Agent 已每 5 秒采样全机 CPU、内存、GPU、温度与功耗，在压力变化、30 秒心跳、app cgroup 治理证据变化或一次有资格的 fresh retry evidence 时入账，仍不执行控制。GPU cache 仅做异步单飞刷新并立即返回旧值 / `None`，GPU I/O 不持 cache 锁；NVIDIA / macOS / Windows 外部探针硬超时分别为 2 / 5 / 15 秒，GPU 卡死不会阻塞 RAM 与后续 cgroup `try_read` 的 5 秒节奏。只有不超过 15 秒的受信同实例 cgroup 状态才进采样；首张 counter 只建 baseline，但 `current >= high` 可立即 Critical。Critical 对 missing/stale/reset/实例切换 sticky，解除需同实例三计数无增且 `current < high`。Runtime schema v6 已接 HostWork Registry、`generation + directive_id` 单飞行、`Pending → HostWorkDirectiveDispatchRecorded → ACK → status reconcile` 账本与独立异步 worker；dispatch marker 在 Adapter 副作用前 append + flush + `sync_data`，只标记可能已执行的 attempt window，既不证明成功，也不允许已有 marker 或 prior-boot 无 marker 的 `Pending` 重放副作用。每 `work + generation + action + pressure_epoch` 最多首次 + 1 次由新鲜、已落账 Critical 证据授权的 retry。生产组合根已注册 scheduled、knowledge、编译期固定连接器、仅空闲且 root turn 已终态的 detached sub-agent，以及经 Supervisor 的 ASR Stop。App cgroup 只注册为 `essential + non-governable` 状态源，前台 turn、任意子进程与 app/WebKit 自停都不在 Governor 的当前动作面。首版 Supervisor 仅接受固定 app `status/launch` 与 ASR `status/stop`，使用 systemd `InvocationID`、双向 PID 凭据校验和耐久控制账本，并对 effective restart / StartLimit / memory policy fail closed。普通 desktop 仍直接启动 `/usr/bin/pinvou3-tauri`，4 GiB / 8 GiB / 2 GiB 只属于显式 MegaBook canary profile。Profile helper、固定 E2E harness 和 deb 固定 payload mode/hash 门禁已实现，但 MegaBook 的真实 deb 安装、High、OOM 与 purge 尚未执行；旧 f24 direct transient canary 不等于 Supervisor 验证。
- Mission/Runtime Run 目前只有 Opened/Started 主事件，完整终态、暂停、恢复、取消和结果验证事件仍需补齐。
- Interaction Plane 首切片已落地：Runtime 账本具有独立 `interaction_run_id`、工具/消息摘要、唯一终态、interrupt 与精确 resume；VoiceShell 消费只读 `projection/*` A2UI v0.9 ordered delta，并显示受信 user-input / artifact 卡。
- 2026-08-20 checkpoint 在普通 `chat:*` 边界增加了插话 FIFO 与 turn-scoped completion lease：busy 时本机语音与兼容/远端文字入口仍可提交，排队项可见且可撤销；同一 Host holder 从首个 admission 保留到最后一个 FIFO turn 的 terminal，随后才 Release。每个 holder 只在 Engine idle 时计算 30 秒遗弃窗口；active turn 本身不发无意义心跳，只有 barrier admission pending 或 Engine/UI idle 且仍有队列时才续自己的 holder。这只是页面崩溃后的 fail-open 回收，不是任务 SLA，也不会让一个存活 Host 延长另一个已崩 Host。Web RPC timeout / outcome-unknown 使用同一 request id 并把队首停在 `uncertain`，只接受精确权威 user/terminal 事件或用户取消，绝不自动换 id 重发。麦克风的请求权限、录音、转写都保持可取消/停止；采集期间后台 summary 与 artifact 不抢占显示。队列与 holder 都是 Renderer/Engine 易失协调状态，不写 Runtime Ledger；VoiceShell 当前仍没有本机文字输入框，scheduled / code / host-managed 与旧 plan-accept / edit 路径不使用这条 Front lease。
- CodeWhale 的 Front 专用 `BoundaryOnly` 回流策略不再把子 Agent 完成结果注入已经活跃的用户 turn；完成结果以独立的 runtime-authored handoff turn 回流，并在 `TurnStarted` 上携带 typed `SubAgentHandoff` provenance，不能被当成外部用户输入。两阶段 barrier 以 forwarder 串行水位证明此前 runtime turn 已完成持久化和 lifecycle 收口，再允许 Host reserve 新的外部用户 turn；manager 已登记但 completion channel frame 丢失时，也会在 idle 边界恢复并 exactly once claim。它隔离 active turn，但不等于标准 AG-UI steering，也不意味着后台完成提示永远不会自行开始新 turn。
- 尚未完成的是标准 AG-UI wire envelope、Mission/Runtime Run 权威关联、Front `front/*` 编排、`system/*` Policy Gate、真正的 live steering / cancel、耐久插话收件箱与跨断连 A2UI Surface recovery；`request_user_input` 的精确 resume 与 turn 终态后的 FIFO 消费都不能冒充这些能力。回答正文仍由 `chat:*` 兼容流承载。
- 当前 A2UI 只有固定 `projection/runtime-overview` 与四个只读组件；Front 没有 `begin_auth`、`present_surface`、Modal 或 AppSurface 能力。旧 ToolStore 虽有连接器 QR 链，当前 VoiceShell 入口不可达。ArtifactCard、Chat 与 Knowledge 的单文件交付物已统一进入 Host 自有的同 WebView `ArtifactBrowser`：它提供一致外壳、连贯放大、安全静态 HTML 沙箱和外链确认，但不是 A2UI Surface、Front 可编排 Modal 或可承载认证的交互 Widget。`ArtifactsPanel` 仍是多产物管理/编辑/设计工作台，其“打开”动作也回到同一 `ArtifactBrowser`。
- 在接入认证与交互组件前，必须先关闭当前 Markdown 可保留 fixed style、dialog、form/input 与远程图片的视觉伪造面；回答内 HTTP(S) 不得直接导航，只能由 Host 经 evidenceRef 解析、显示标准化的初始目标域名（含 Punycode / 端口）并按需确认。旧连接器 URL 的字符串包含判断必须升级为 HTTPS + 精确 Host / 端口 / Provider Adapter 校验；OAuth callback / redirect_uri 使用固定 allowlist，不能声称宿主能拦截手机浏览器里的远端跳转链。

## 继续扩展交互平面前的门禁

这些工作可并行推进，不代表严格瀑布顺序；每项能力只在其实际依赖的门禁全部通过后开放。认证纵切不依赖完整 Mission 工作台，但必须先通过 Presentation Host 安全门与 Auth/Policy 子门禁。

1. Event Envelope 与独立 Interaction Run 在 schema v5 引入；当前 Runtime schema v6 增加 HostWork 事件。v5 及更早的历史事件与账本前缀保持原始字节不变，只在 replay 时 upcast 到当前投影。继续冻结 Mission、Runtime Run 的权威 correlation 语义。
2. 冻结 Mission Agent Result、Orchestrator Receipt、`VerifiedTaskOutcome` 三层回执。
3. 补齐 Runtime 生命周期和用户/模型/工具/Policy/artifact/receipt 事件，再做 AG-UI Adapter。
4. 第一版确定性、只读 `projection/*` 已落；副作用 Action 在 Kernel Policy Gate 接通前继续保持关闭。
5. interrupt/resume、exactly-one terminal、未知组件/跨 Surface 拒绝和旧 sequence 丢弃已有测试；下一步补 front/system 分区写权、revision 冲突与跨断连重放测试。
6. 显示面的第一条互动纵切采用 Host 原生 `system/auth/*` AuthChallenge，复用现有 Connector QR 原语；Front 只传 opaque `providerCapabilityId` 与有界 `purpose`，不传 URL、二维码、flow type 或凭据。
7. 富 HTML 分成静态 ArtifactPreview 与沙箱 AppSurface 两条路；Front 只能引用 opaque `artifactId` / `appViewRef`，不得直接提交 HTML、CSS、JS、src 或 iframe 权限。

更新任一跨文档不变量时，必须同时检查四份 HTML、三份 ADR、相关代码测试和该索引；不得只改一张图。
