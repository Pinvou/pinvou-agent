# PinvouOS 架构权威索引

状态：2026-08-18 交互平面首阶段已实施，并在 MegaBook 完成白盒与黑盒验收。

这份索引不替代各专题文档，只规定它们分别对什么负责，以及发生冲突时按什么顺序裁决。

## 文档所有权

| 范围 | 唯一权威 | 说明 |
|---|---|---|
| 无 Session、Identity、Mission、Runtime Run、Event Ledger | [ADR-0007](../adr/0007-PinvouOS-无Session连续运行时.md) | 产品与运行时领域模型 |
| 长期记忆语义、证据门槛、当前实现状态 | [Memory ADR](../adr/0008-PinvouOS-Memory-Agent-整理内核.md) + [Memory HTML](pinvouos-memory-architecture.html) | Memory ADR 负责硬约束，HTML 负责可视化与状态 |
| 常驻系统角色、调用拓扑、中断权限、同步/异步关系 | [Multi-Agent HTML](pinvouos-multi-agent-collaboration.html) | 描述本地工作树；不代表 MegaBook 已部署版本 |
| AG-UI、A2UI、画布写权、Host Renderer、交互迁移路线 | [Interaction HTML](pinvouos-interaction-surface-agui-a2ui.html) | 同时分开 MegaBook 部署态、本地工作树和目标态 |
| 历史原因与产品方向 | Obsidian 产品主张与项目记录 | 用于解释设计来源；已接受 ADR 和当前代码优先 |

同一事实发生冲突时，按以下顺序处理：可复现实机行为 → 当前代码与自动化测试 → 已接受 ADR → 对应专题 HTML → Obsidian 历史记录 → 未来提案。历史文档不能覆盖较新的已接受决策。

## 跨文档共同不变量

1. 用户始终面对一个 `pinvou` 身份。Front 是唯一面向用户表达、普通业务提问和最终答复的 Agent。
2. 产品领域没有 Session。旧 Session/thread/conversation 只能作为迁移期私有执行缓存，不能成为身份、任务、记忆、前端协议或账本主键。
3. Front 先走 Direct 快路径，最多三轮工具批次；明显复杂或三轮未完成时，才调用唯一 Orchestrator。Orchestrator 不投机并跑第二份完整答案。
4. Runtime Ledger 是业务事实与可恢复意图的唯一耐久来源。AG-UI state、A2UI data model、客户端 store 和 Memory projection 都不是第二真相源。
5. Policy Agent 负责评估 `Allow / Deny / RequireConfirmation`；Kernel/AuthorityStore 负责强制。Front 不能取消硬拒绝，也不能隐藏系统确认。
6. Surface 写权分区：`projection/*` 只归 Runtime Projector，`front/*` 只归 Front，`system/*` 只归 Kernel/Host。Surface Agent 只观察窗口与可访问性场景，不因同名获得 A2UI 写权。
7. Memory 不保存任务进度。Runtime Mission/Run Work Graph 保存承诺、进度、阻塞和下一步；Memory 只从可信、已验证结果形成行动经历和教训。
8. Dynamic Mission Agent 的 `completed` 只是执行自报，不等于 Mission 已验证完成。只有 Runtime 的验证回执能关闭任务成功并成为 Memory 证据。

## 标识与生命周期

| 标识 | 含义 | 约束 |
|---|---|---|
| `mission_id` | 持久目标 | 可跨多次交互与多个 Runtime Run |
| `runtime_run_id` | Mission 的一次任务执行尝试 | 可运行、暂停、恢复、完成、取消或失败 |
| AG-UI `threadId` | 交互协议范围 | Mission 工作区通常由 Mission 派生；全局 Pinvou 画面使用稳定 scope；不是 Session 领域对象 |
| AG-UI `runId` | 一次 Front—用户交互执行 | 使用独立 `interaction_run_id`，通过 correlation 关联一个或多个 Runtime Run |
| `interruptId` | 一次等待用户输入或审批 | `RUN_FINISHED` 使用 interrupt outcome；同 thread 的新 AG-UI run 通过 `resume[].interruptId` 恢复 |
| A2UI `surfaceId` | 逻辑 UI 区域 | 必须带命名空间、catalog 版本、revision、basis Runtime sequence 和幂等键 |

AG-UI 的 interrupt 不是独立生命周期事件。正确协议是先发送恢复所需 snapshot，再发送 `RUN_FINISHED { outcome: { type: "interrupt" } }`；恢复请求使用同一 `threadId`、新的 AG-UI `runId` 和 `resume[].interruptId`。实现时以 [AG-UI Interrupts](https://docs.ag-ui.com/concepts/interrupts) 为准。

## 当前实现边界

- Memory Runtime 核心已接统一账本：细粒度 decision/checkpoint/replay、单写保护、结构热索引和有界 Top-K 可用；异步 worker、`VerifiedTaskOutcome`、Front Context Compiler、冷归档和 Obsidian 尚未闭环。
- Front → 唯一 Orchestrator 的 manager-as-tool 与三轮 Direct 硬门禁已接通；本次部署保留该执行策略，只在外层增加 interaction trace。
- Surface、Device、Policy 仍处于 Starting；Policy 算法存在，但 AuthorityStore 与所有副作用工具的统一执行闸门未接。
- Mission/Runtime Run 目前只有 Opened/Started 主事件，完整终态、暂停、恢复、取消和结果验证事件仍需补齐。
- Interaction Plane 首切片已落地：Runtime 账本具有独立 `interaction_run_id`、工具/消息摘要、唯一终态、interrupt 与精确 resume；VoiceShell 消费只读 `projection/*` A2UI v0.9 ordered delta，并显示受信 user-input / artifact 卡。
- 尚未完成的是标准 AG-UI wire envelope、Mission/Runtime Run 权威关联、Front `front/*` 编排、`system/*` Policy Gate、steering 与跨断连 surface recovery；回答正文仍由 `chat:*` 兼容流承载。

## 继续扩展交互平面前的门禁

1. Event Envelope 与独立 Interaction Run 已升级到 schema v4；继续冻结 Mission、Runtime Run 的权威 correlation 语义。
2. 冻结 Mission Agent Result、Orchestrator Receipt、`VerifiedTaskOutcome` 三层回执。
3. 补齐 Runtime 生命周期和用户/模型/工具/Policy/artifact/receipt 事件，再做 AG-UI Adapter。
4. 第一版确定性、只读 `projection/*` 已落；副作用 Action 在 Kernel Policy Gate 接通前继续保持关闭。
5. interrupt/resume、exactly-one terminal、未知组件/跨 Surface 拒绝和旧 sequence 丢弃已有测试；下一步补 front/system 分区写权、revision 冲突与跨断连重放测试。

更新任一跨文档不变量时，必须同时检查三份 HTML、两份 ADR、相关代码测试和该索引；不得只改一张图。
