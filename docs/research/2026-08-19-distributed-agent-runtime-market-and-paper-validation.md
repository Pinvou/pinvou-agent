# Pinvou 分布式 Agent Runtime：市场产品、标准与论文验证

> 日期：2026-08-19
>
> 验证对象：`docs/superpowers/specs/2026-08-18-pinvou-distributed-node-runtime-design.md` 及其事件、迁移、威胁模型和阶段冻结配套规范
>
> 证据边界：只采用官方产品文档、官方协议/开源仓库、标准/RFC 和原始论文；不以二手博客作为依据。
>
> 本文性质：外部证据验证与修订建议，不是新的实现规范。来源链接按 2026-08-19 可访问内容记录。

## 1. 执行结论

Pinvou 的主方向有充分的一手证据支持，不是脱离行业实践的自造架构。以下核心选择已经出现明显的市场趋同或有成熟分布式系统先例：

- Controller/Node 与 Runtime Adapter 分层、远程运行后继续连接同一逻辑任务；
- 本地先落盘、事件日志重放、累计确认、至少一次传输和幂等去重；
- 每个并行写任务使用隔离 workspace；
- 以资源 request/reservation、硬上限和有界队列完成 admission/backpressure；
- 资源通过带完整性信息的稳定引用传递，而不是跨设备裸路径；
- 事件、协议、存储版本分离，未知字段和不支持能力显式处理。

但当前蓝图存在几处“方向正确、合同尚未闭合”的问题。其中四项应在对应阶段开工前调整：

1. **把 outbound secure relay 的传输接口前移到阶段 4**。Direct TLS 仍保留为局域网/离线优先路径，但不能再把 Relay 整体视为阶段 10 才考虑的附加项。
2. **把自动隔离 workspace 的最小能力前移到阶段 8 fan-out 之前**。不必提前完成通用跨 Node `WorkspaceSyncProvider`，但必须能自动创建、绑定、清理独立 worktree/checkout，不能让“用户手工准备多个目录”成为并发写安全性的核心保证。
3. **修正 `WorkspaceWriteLease` 的语义**。当前对象不因超时失效，更准确的名字是 `WorkspaceWriteGrant` 或 `WorkspaceWriteLock`；普通文件系统也无法在每次写入时校验 epoch，因此不能宣称 epoch 已形成可执行 fencing。
4. **为多 Agent 增加相对单 Agent 的价值门禁**。阶段 8 的 fan-out 不应仅证明“能并行”，还要证明质量、耗时、token/成本和冲突率的综合收益。

另有两个优先级很高、虽非用户特别点名但会影响阶段 1/4 正确性的合同缺口：

- 单一 `main` transport sequence/累计 ACK 与“R0 可越过 R1、延迟不受影响”不能同时成立；控制流需要独立 stream/sequence/ACK，或承认单流队头阻塞并通过最坏积压门禁。
- 若审批、取消和文本 delta 共用 stdout/JSON-RPC 通道，Adapter 不能靠停止读取进程管道给 R1 背压，否则会连 R0 一起堵住；必须持续 drain，上游不可流控时用 disk spool、合并、拒绝新 admission 或中断 runtime 收口。

## 2. 证据矩阵

| 设计主题 | 蓝图映射 | 一手证据 | 判断 | 应否调整 |
|---|---|---|---|---|
| 远程 Agent runtime、会话恢复与接管 | §8、§12、§23 阶段 4–5 | [OpenAI Codex app-server](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md) 支持 thread start/resume/fork、审批和 interrupt；[ACP session setup](https://agentclientprotocol.com/protocol/v1/session-setup) 区分 load（重放历史）与 resume（重连但不重放）；[OpenHands Agent Server](https://docs.openhands.dev/sdk/guides/agent-server/api-sandbox) 通过 conversation id 重连远程 workspace | 强支持 Logical Session 与 Runtime Attachment 分离，也支持显式区分“恢复历史”和“恢复运行时” | 保留主设计；补充不同 resume 能力的显式协商与失败语义 |
| 跨设备 secure relay | §7.2、§8、阶段 10 | [Codex Anywhere](https://openai.com/index/work-with-codex-from-anywhere/) 采用 secure relay 让授权设备访问可信机器而无需公网暴露；[Codex exec-server relay](https://github.com/openai/codex/blob/main/codex-rs/exec-server/README.md) 定义 rendezvous/Noise relay、stream、sequence、ACK bitmap、resume/reset/heartbeat | 市场趋同于“机器主动出站 + relay 穿越 NAT/防火墙”，而不是要求远端机器提供可入站地址 | Relay seam 前移阶段 4；实际公网 relay 服务可分阶段交付 |
| 每任务隔离 workspace | §15.7、§23 阶段 8/10、关键决策 32–33 | [OpenAI Codex](https://openai.com/index/introducing-codex/) 每任务独立云沙箱；[Codex 帮助文档](https://help.openai.com/en/articles/11369540-using-codex-with-your-chatgpt-plan) 明确内建 worktree、每任务隔离；[GitHub Copilot agent sessions](https://docs.github.com/en/copilot/how-tos/github-copilot-app/agent-sessions) 每 session 独立 workspace/branch；[Cursor Background Agents](https://docs.cursor.com/background-agent) 每 agent 独立 VM、clone 和 branch | 已是并行 coding agent 的默认安全边界，而非后期易用性增强 | 把最小 `WorkspaceIsolationProvider` 前移到阶段 8；通用同步/合并仍可留阶段 10 |
| durable event log、spool、ACK | §13、事件规范、存储迁移规范 | [Kafka design](https://kafka.apache.org/42/design/design/) 的 offset/checkpoint/idempotent retry；[Temporal architecture](https://github.com/temporalio/temporal/blob/main/docs/architecture/README.md) 的 durable history/replay；[NATS JetStream pull consumers](https://docs.nats.io/learn/jetstream/pull-consumers) 的显式 ACK、redelivery 与 MaxAckPending | 强支持 raw spool → durable barrier → transport sequence → Controller WAL durable → ACK，以及 at-least-once + dedup | 保留；修复 R0/R1 单 stream 队头阻塞和混合进程管道背压 |
| admission、reservation、backpressure | §14、§14.5–14.6、阶段 7/9 | [Kubernetes resource management](https://kubernetes.io/docs/concepts/configuration/manage-resources-containers/) 用 request 调度、limit 约束；[Borg 论文](https://research.google/pubs/large-scale-cluster-management-at-google-with-borg/) 的 admission、packing、overcommit 和隔离；[SEDA 原论文](https://doi.org/10.1145/502059.502057) 的显式队列、动态资源控制和 load shedding；[Reactive Streams](https://github.com/reactive-streams/reactive-streams-jvm) 要求 demand 驱动且队列有界 | 强支持两级 admission、reservation、软/硬/紧急水位、滞回和拒绝策略 | 保留；AiFlow 只能作为补充证据，成熟先例应成为主要论据 |
| workspace claim、lease、fencing | §15.7、恢复矩阵 | [Gray/Cheriton Lease](https://www.cs.cmu.edu/~15712/papers/gray89.pdf) 是有明确有效期的合同；[Chubby 论文](https://research.google.com/archive/chubby-osdi06.pdf) 用单调 sequencer/acquisition count 让资源服务器拒绝旧 holder；[Kubernetes Lease](https://kubernetes.io/docs/concepts/architecture/leases/) 同样包含 renew time 和 duration | 当前“不因超时回收”的对象不是严格 lease；普通文件系统写入也没有 fencing-token 校验点 | 重命名为 Grant/Lock；保留失联后不自动重授的保守策略；未来由存储 adapter 执行 fencing |
| ResourceRef 与范围读取 | §17 | [OCI Descriptor](https://github.com/opencontainers/image-spec/blob/main/descriptor.md) 的 digest/size/mediaType；[OCI Distribution](https://github.com/opencontainers/distribution-spec/blob/main/spec.md) 的 digest、opaque location 和 range；[RFC 9110 If-Range](https://datatracker.ietf.org/doc/html/rfc9110#section-13.1.5) 的 strong validator；[S3 GetObject](https://docs.aws.amazon.com/AmazonS3/latest/API/API_GetObject.html) 的 version/checksum/range | 强支持 opaque resource id + version/checksum + conditional range | 保留；明确 If-Match/If-Range mismatch 的精确返回与 partial 丢弃规则 |
| 本地 IPC 安全边界 | §13.1、阶段冻结 D-04、威胁模型 | [Windows Named Pipe 安全](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights) 要求自定义 ACL；[CreateNamedPipe](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-createnamedpipew) 提供 `PIPE_REJECT_REMOTE_CLIENTS` 与 `FILE_FLAG_FIRST_PIPE_INSTANCE`；[Linux unix(7)](https://man7.org/linux/man-pages/man7/unix.7.html) 规定 pathname socket 权限和 `SO_PEERCRED` | 方向正确，但权限和会话身份措辞还不够精确 | 父目录 0700/socket 0600 + peer credential；禁止抽象 UDS；Windows 增加 first-instance 和 logon SID 语义 |
| 事件 schema、版本和 cursor | §13.1、§21、事件规范、迁移规范 | [CloudEvents](https://github.com/cloudevents/spec/blob/main/cloudevents/spec.md) 的 source+id/specversion/type；[Protobuf](https://protobuf.dev/programming-guides/proto3/) 的字段编号不可复用、未知字段与 JSON 限制；[Avro schema resolution](https://avro.apache.org/docs/current/specification/)；[Kubernetes API concepts](https://kubernetes.io/docs/reference/using-api/api-concepts) 的 list/watch/resourceVersion/410 Gone | 分版本方向正确；“cursor 永远可继续”和“自动最低共同版本”都不足 | 增加 cursor compacted → snapshot+resubscribe；版本范围 + feature bits；兼容性 CI 与稳定 event identity |
| 跨 Agent 任务/产物互操作 | §11、§15、§17 | [A2A specification](https://a2a-protocol.org/latest/specification/) 的 Task、Artifact、stream/poll/push；[MCP Tasks extension](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/extensions/tasks/overview.mdx) 的 durable task handle、poll/input-required/cancel；[MCP Resources](https://modelcontextprotocol.io/specification/2025-06-18/server/resources) 的 URI/resource | 适合做边界 adapter，不适合作为 Pinvou 内部权威账本的替代品 | 明确 A2A/MCP 映射是有损、能力协商的 adapter；内部状态机不被外部协议绑死 |

## 3. 市场产品对蓝图阶段顺序的影响

### 3.1 Secure relay 不应整体后置到阶段 10

蓝图当前阶段 4 以手动 endpoint + `DirectTlsTransport` 为主，把 `RelayDiscovery`、`RelayTransport` 和跨网络连接放到后期/阶段 10。这对同一 LAN、VPN 或用户可配置端口转发的 MVP 是可行的，但与远程 coding agent 产品已经形成的安全连接习惯有差异。

[Codex Anywhere](https://openai.com/index/work-with-codex-from-anywhere/) 的核心不是把工作目录搬到云端，而是通过 secure relay 访问仍留在可信机器上的文件、凭据和权限，同时不要求该机器暴露公网入口。[Codex exec-server 的官方 relay 协议](https://github.com/openai/codex/blob/main/codex-rs/exec-server/README.md) 进一步表明，relay 只负责 rendezvous/转发，端点自行处理可靠性、stream sequence、ACK 和 resume。这个形态与 Pinvou“用户自有 Node、Controller 权威、事件可靠性属于端点”的边界高度一致。

建议将阶段拆分改为：

- 阶段 4 的 `NodeConnection` 合同同时覆盖 `DirectTlsTransport` 和 **outbound relay-capable transport**，配对、Owner、事件、资源语义不随传输变化；
- 阶段 4 至少完成一个 relay contract test / loopback 或可自托管测试 relay，证明 Node 不需要公网入站端口；
- 商业托管 relay、全球路由、移动端入口和规模化运营仍可留在阶段 10；
- 社区版不得依赖 Pinvou 私有服务，所以 relay 必须是可替换 provider，并给出自托管或第三方实现边界；Direct TLS 继续作为离线/LAN 首选。

不建议直接复制 Codex relay wire protocol。Codex relay 的 sequence/ACK 是传输分段可靠性，Pinvou 的 ACK 是“Controller WAL 已耐久”的应用层确认；二者可以叠加，但不能混为同一语义。

### 3.2 自动 workspace 隔离必须早于阶段 8 fan-out

蓝图已经正确要求并行写任务使用不同 Workspace，但又把 `WorkspaceSyncProvider/Bare Git` 自动化放到阶段 10，并把仓库准备交给用户。这会造成阶段 8 的产品闭环缺口：系统自动 fan-out，却依赖用户手工为每个任务创建不冲突的可写目录。

市场上三种主要实现都把隔离放在并行执行之前：OpenAI Codex 每任务独立环境且 Codex app 内建 worktree；GitHub Copilot agent session 可选择新的 working tree 或隔离 cloud sandbox；Cursor Background Agent 为每个 agent clone repository 并使用独立 branch。它们并不证明“必须用 Git worktree”，但共同证明 **每任务可写文件系统隔离是执行前置条件**。

建议不要把完整 `WorkspaceSyncProvider` 整体前移，而是拆成两个合同：

```text
WorkspaceIsolationProvider        # 阶段 8 前置
- provision(base_binding, task_id, base_revision) -> IsolatedWorkspace
- verify_distinct_identity(...)
- collect_result(patch | commit | ResourceRef)
- cleanup(policy)

WorkspaceSyncProvider             # 阶段 10
- materialize_across_nodes(...)
- synchronize_incrementally(...)
- integrate/merge_assist(...)
```

阶段 8 的最小实现可以只覆盖本地 Git worktree、独立 clone 或用户已提供的独立目录，但必须由 Controller/Node 验证其 workspace identity 确实不同。网络盘别名、跨 Node 同一物理目录和最终自动合并仍可明确拒绝或后置。

## 4. Durable event、ACK 与背压：主方向正确，但控制流合同需修正

Kafka、Temporal、JetStream、Reactive Streams、gRPC 和 SEDA 从不同层面支持当前设计：先耐久化再确认、断线重放、显式消费位置、至少一次 + 幂等去重、未确认水位和有界队列。尤其 [gRPC flow control](https://grpc.io/docs/guides/flow-control/) 明确指出“write API 返回”不等于数据已经上网，手动流控若双方互相等待还可能死锁。

### 4.1 P0：R0 独立优先级与单一累计序列矛盾

事件规范阶段 1 固定 `stream_id = main`，并使用单一连续 transport sequence 和累计 ACK；蓝图同时要求 R0 使用独立队列/预留容量、可跨越 R1 且满足低延迟。若已有大量 R1 获得较小 sequence，后到的 R0 有两个选择：

- 等待前面的 R1，违反 R0 延迟承诺；
- 越过 R1 发送，Controller 先看到 sequence 缺口，无法安全推进累计 ACK。

“物理独立队列”本身解决不了这个逻辑队头阻塞。[HTTP/2](https://datatracker.ietf.org/doc/html/rfc9113) 通过同一连接上的独立 stream 避免应用层队头阻塞，Pinvou 也应采用类似的合同：

- `control` 与 `main` 至少两个 transport stream；
- 每个 stream 独立 sequence、累计 ACK、replay cursor 和 source-span 映射；
- emergency segment 只承载 control/R0 的终止、取消、审批和缺口事件；
- G3 增加“R1 已积压到 high-water 后注入 R0”的最坏情况测试。

若阶段 1 坚持单流，文档必须删除“R0 可绕过已编号 R1”的暗示，并以最大 backlog 推导 R0 最坏延迟；这会显著削弱现有 SLO，因此不推荐。

### 4.2 P0：不能靠停止读取混合 stdout/JSON-RPC 通道实现背压

当 Agent CLI 把文本 delta、审批请求、cancel 响应和终止状态复用在同一 stdout/JSON-RPC 通道时，Adapter 停止读取会同时阻塞控制消息，甚至导致 runtime 等待审批、Pinvou 等待 runtime 输出的双向死锁。建议冻结：

- Adapter reader 对混合协议通道必须持续 drain；
- R1 可写 disk spool、批量合并；R2/R3 可按已声明策略丢弃；
- 达到 hard pressure 时停止新的 admission，并请求 interrupt/terminate runtime；用 emergency R0 记录终止或显式缺口；
- 只有 runtime 官方接口提供独立流/显式 demand 时，才能把 flow-control 信号安全传回上游。

## 5. WorkspaceWriteLease：应改名，并收窄 fencing 声明

当前策略“失联后不因超时自动重授，必须确认旧进程已停止”是正确的保守安全策略。但 [Gray/Cheriton](https://www.cs.cmu.edu/~15712/papers/gray89.pdf) 和 [Kubernetes Lease](https://kubernetes.io/docs/concepts/architecture/leases/) 中的 lease 都有时间有效期与 renew deadline；当前对象更像 renewable write grant/lock。

另一个关键差异是，`epoch` 只有在**副作用接收端**每次写入时校验才是 fencing token。[Chubby](https://research.google.com/archive/chubby-osdi06.pdf) 的 sequencer 由被保护资源服务器拒绝旧 generation。Agent 直接写普通 NTFS/ext4 文件时，文件系统不会读取 Pinvou epoch，因此 epoch 可以隔离 Controller 账本和迟到事件，却不能阻止失联旧进程继续写文件。

建议：

- 全文把 `WorkspaceWriteLease` 改为 `WorkspaceWriteGrant` 或 `WorkspaceWriteLock`；
- 明确 heartbeat 用于 liveness/reconcile，不使 grant 自动到期；
- 把 epoch 的保证写成“账本 holder 版本和调度防重授”，不得表述成普通文件系统已被硬 fencing；
- 未来只有共享存储/文件代理 adapter 能让每次写请求携带 epoch，且存储端拒绝旧 epoch 时，才引入 `FencedWorkspaceLease`；
- 保留“未知旧进程仍可能产生副作用时禁止重授”这一强约束。

## 6. ViewModel cursor 必须定义 compacted/expired 恢复

蓝图把 cursor 描述为一致快照句柄，但没有定义它在 WAL/投影历史压缩后的失效行为。无限保留所有 cursor 对应版本会把客户端恢复合同变成无界存储承诺。

[Kubernetes list-watch](https://kubernetes.io/docs/reference/using-api/api-concepts) 提供成熟先例：list 返回 snapshot 和 `resourceVersion`，watch 从该版本继续；当旧版本已压缩时服务端返回 `410 Gone`，客户端清空旧缓存、重新 list，再从新版本 watch。Pinvou 建议采用等价而非同名的合同：

```text
subscribe(cursor)
  -> events
  | cursor_expired { latest_snapshot_ref, snapshot_version }

recover:
  1. 原子加载 snapshot_version 对应快照
  2. 丢弃旧的局部投影状态
  3. 从 snapshot_version 之后重新订阅
```

需要覆盖 cursor 在“仍有效、已压缩、未知/伪造、属于其他 filter/schema version”四种情况，并保证 snapshot 与后续订阅之间无丢事件窗口。

## 7. 多 Agent 论文：可用于提出假设，不能直接升级为生产定律

蓝图引用的 TIPEX、ALIGN、History Matters、AiFlow 和 LATTE 均是 2026 年非常新的研究，其中 TIPEX、History Matters、AiFlow 在本文验证日之前仅约两周。它们支持“值得实验”的方向，但不足以替代本项目实测门禁。

| 论文 | 原论文实际支持 | 当前蓝图可能写得过强之处 | 建议措辞 |
|---|---|---|---|
| [TIPEX](https://arxiv.org/abs/2608.05791) | 在 GAIA 等实验中区分 structural/replica parallelism；可能提升准确率和端到端延迟，但增加 token；过度并行未必更好 | 把两层并行模型直接当作通用执行分类依据 | 保留 schema 扩展点；明确是实验性 taxonomy，`best_of_n` 默认关闭并受价值门禁控制 |
| [ALIGN](https://arxiv.org/abs/2602.00127) | 在其 aligned delegation game、候选访问相等和公平比较条件下给出期望性能保证 | “best_of_n 可证明优于单路径”容易被读成任意任务/调度都成立 | 改成“在 ALIGN 的特定激励和公平候选访问假设下成立；Pinvou 不继承该保证” |
| [History Matters](https://arxiv.org/abs/2608.03833) | 异构 MARL、顺序委派和特定成本/拓扑设定下，历史相关策略可能改善协调 | 把历史直接解释成生产 Node/Runtime 的互惠信任或跨时激励 | 历史成功率、延迟、成本仅作为可解释的候选特征；不作为安全信任；上线前做漂移、冷启动和可操纵性验证 |
| [AiFlow](https://arxiv.org/abs/2608.00558) | token-native streaming graph 中 Node Guardian 的本地队列边界、并发、顺序、溢出、取消和 retry；有限 microbenchmark/trace replay | 直接借用其名字可能让人以为 Controller per-Node ingress 已获得广泛生产验证 | 设计继续使用成熟 bounded queue/backpressure 原则；AiFlow 只列为新近相似实现，主要依据改为 SEDA/Reactive Streams/JetStream |
| [LATTE](https://arxiv.org/abs/2605.06320) | 多个协作任务中，shared evolving coordination graph 可降低 token、wall time、通信和冲突并保持/提高准确率 | 可能被读成动态 DAG 一定优于静态分解 | 把 `discover_task`/动态图变更作为受限实验能力；与静态 DAG、leader-worker 和 single-agent 做同预算对照 |
| [LAMaS](https://arxiv.org/abs/2601.10560) | 在特定 learned controller/benchmark 中利用 critical path 进行多 Agent 调度 | 不能直接推出所有生产 workload 中“非关键任务不得与关键任务竞争”的普遍最优规则 | 保留关键路径优先和 reservation 作为启发式；用 starvation 上界、失败传播和本地 benchmark 验证 |

因此，蓝图应把“借鉴 X 的理论保证/核心论点”统一分成三个标签：

- **mature invariant**：由分布式系统标准/多年生产系统支持，可写成硬合同；
- **market convergence**：多个产品独立采用，可作为默认 UX/安全基线；
- **research hypothesis**：新论文中在特定数据集和假设下观察到，必须经 Pinvou eval 才能晋级。

动态 DAG、历史评分、replica parallelism 均属于第三类。

## 8. 阶段 8 增加“相对单 Agent 价值门禁”

多 Agent 的验收不能只有 fan-out、汇总、取消和失败恢复。TIPEX 明确展示 token 增长和过度并行的反效果；LATTE 也把 token、wall-clock、file conflicts 和 redundant outputs 当作必须同时衡量的效率指标。这支持在阶段 8 加入基线对照。

建议对固定任务集，在相同模型版本、工具权限、输入上下文和最大预算下，比较 single-agent 与 CollaborativeRun：

| 维度 | 最低记录项 |
|---|---|
| 质量 | 任务通过率、可执行测试/验收通过率、回归数、人工接受率 |
| 时间 | wall-clock p50/p95、首个可用结果时间、关键路径等待时间 |
| 成本 | input/output/total token、模型调用数、货币成本、Node 时间 |
| 协调 | 文件冲突率、重复工作率、merge/reconcile 失败率、`unknown_outcome` 率 |
| 人工负担 | 审批次数、人工修复/合并分钟数、需要重新下发任务的比例 |

推荐晋级规则不是写死一个虚假通用百分比，而是先冻结项目可接受阈值，并至少满足：

- 质量显著提高且成本/时间/冲突没有超过上限；或
- 质量不退化且 wall-clock 的改善足以覆盖额外 token、冲突和人工负担；
- 任一方案的安全隔离、可恢复性和预算硬上限必须先通过，不能用质量收益交换；
- `best_of_n`、动态 fan-out 和历史驱动放置分别独立开关、独立归因，避免一次实验无法判断收益来源。

若未过门禁，阶段 8 仍可以交付单个自动调度 AgentTask、静态 DAG 和结果汇总，但默认并发/replica 不应开启。

## 9. A2A、MCP Tasks 与 Pinvou 内部合同的互操作边界

[A2A](https://a2a-protocol.org/latest/specification/) 面向 Agent 与 Agent/remote agent service 之间的任务交换，核心是 Task 状态、Message、Artifact、history，以及 streaming/polling/push notification；[MCP Tasks extension](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/extensions/tasks/overview.mdx) 为可能长时间运行的工具/请求提供 durable task handle、poll、input-required 和 cooperative cancel；[MCP Resources](https://modelcontextprotocol.io/specification/2025-06-18/server/resources) 提供 URI 资源发现/读取。三者的层级并不相同。

| Pinvou 概念 | A2A 可映射 | MCP 可映射 | 不应丢失的 Pinvou 私有语义 |
|---|---|---|---|
| `AgentTask` | A2A Task | 带 task augmentation 的 MCP request/task | reservation、Node placement、workspace grant、execution journal、unknown outcome |
| `Child Session` | Task history/contextId 的一部分，非等价 | 不存在统一 session 等价物 | Logical Session/Attachment/epoch/native resume |
| `ResultRef` | Artifact/Message part | Resource link / tool result / task result | provenance、policy decision、注入父 Session 的授权 |
| `ResourceRef` | Artifact 可内联 bytes/URI | Resource URI | node ownership、checksum/version、read token、range、lifecycle 和审计 |
| streaming event | Task status/artifact update | progress/log/result 取决于具体 request/extension | R0–R3、durable sequence/ACK、spool gap、Controller WAL durability |

建议明确：

- A2A/MCP 均通过 Adapter 进入 `runtime-api`，不是 Controller Store 的内部 schema；
- A2A `Artifact` 和 MCP `Resource` 必须经过拉取/校验/策略判断后转换为 Pinvou `ResourceRef`，不能把外部 URI 当作已受信任本地资源；
- A2A stream/MCP progress 的完成只表示上游协议状态，不等价于 Pinvou Controller WAL 已 durable；
- 必须通过 capability/version negotiation 才启用 task、stream、push、resume/cancel；不支持时显式返回 unsupported，不能静默退化导致丢审批或取消；
- MCP Tasks 当前是官方 extension surface，但仍属于快速演进的扩展且 host 支持不一，阶段计划不能假定所有 MCP host 都已实现；
- 不追求内部状态机与 A2A/MCP 一一同构，因为这样会丢掉 workspace、fencing、admission、审计和 unknown-outcome 等 Pinvou 必需语义。

## 10. ResourceRef、事件版本与 IPC 的精确修订

### 10.1 Range/validator

当前“Range 绑定 version/checksum，版本变化返回冲突”的意图正确，但应选择明确的 HTTP 语义：

- 若使用 `Range + If-Match: <strong-etag/checksum>`，不匹配返回 `412 Precondition Failed`；
- 若使用 `If-Range`，validator 不匹配时 RFC 9110 的行为是返回完整新 representation（通常 200），客户端必须丢弃旧 partial 后从头开始，绝不能拼接；
- 无论采用哪种，服务端都要在发送前校验 ResourceRef version/checksum，客户端完成后校验 size/digest。

### 10.2 事件 identity 和兼容性 CI

建议为可导出/去重事件冻结稳定 identity：`event_id`，或明确 `(node_id, attachment_id, stream_id, seq)` 是不可变唯一键。transport sequence、source-native sequence 和 schema version 不能复用一个字段表达。

版本协商建议从“自动选择最低共同 protocol version”改为：支持版本范围 + feature bits + required features。像 [A2A versioning](https://a2a-protocol.org/latest/specification/) 一样，不支持请求版本或关键能力时显式失败；否则最低版本可能让连接成功，却静默丢失审批、取消或 durable ACK 语义。

每次 schema 变化至少运行：

- N-1 writer → N reader golden；
- N writer → N-1 reader golden；
- unknown field 二进制 round-trip；
- JSON/vendor extension 是否保留未知字段的显式测试（ProtoJSON 转换可能丢未知字段）；
- event type 的 incompatible change 必须新 type/version，字段编号/含义不得复用。

### 10.3 本地 IPC

建议把宽泛的“Unix UDS 0700”改为：pathname UDS 位于 0700 父目录，socket 0600，连接后校验 `SO_PEERCRED`（BSD/macOS 用等价 peer credential）；不使用 Linux abstract namespace，因为它没有 pathname permission 语义。

Windows Named Pipe 使用自定义 DACL、`PIPE_REJECT_REMOTE_CLIENTS`、`FILE_FLAG_FIRST_PIPE_INSTANCE` 和客户端身份校验。`user SID hash` 只能防跨用户命名碰撞，不能区分同一用户的不同登录会话；若合同需要 per-logon session 隔离，应使用 logon SID 进入名称/DACL，而不是笼统写 user SID。

## 11. 安全模型的一个市场差异：Owner 授权应与可轮换传输凭据分离

蓝图的 OwnerBinding 永久不自动释放是合理的**授权关系**，但把持久 Owner 公钥同时作为长期重连认证密钥，会把密钥泄漏风险扩大为“必须逐 Node 物理 release”。成熟 workload identity 体系通常把稳定身份/授权与短期、自动轮换的认证凭据分开。例如 [SPIFFE 概念](https://spiffe.io/docs/latest/spiffe/concepts/) 和 [X.509-SVID 规范](https://spiffe.io/docs/latest/spiffe-specs/x509-svid/) 使用短期证书和轮换来缩小凭据泄漏窗口。

Pinvou 不需要引入完整 SPIFFE，但建议阶段 4 冻结：

- `OwnerBinding` 是稳定授权，不能因网络断开过期；
- Controller/device transport credential 是可撤销、可轮换、短期或有 generation 的证明；
- Node 本地 release 仍是最终恢复路径，但 Owner 可在已认证状态下轮换凭据、吊销旧设备/旧 generation；
- relay 只看到路由元数据，端到端身份验证和内容加密仍由 Controller/Node 完成。

## 12. 不应盲目追随的市场做法

1. **不要把云端临时容器变成唯一运行形态。** OpenAI、GitHub、Cursor 的隔离证明 workspace 边界重要，不证明 Pinvou 应放弃用户自有异构 Node；本地凭据、离线运行和用户控制仍是合理差异化。
2. **不要把 runtime 原生 session id 升级为权威身份。** ACP/Codex/OpenHands 的 resume 能力各不相同，继续以 Pinvou Logical Session + Attachment 能力协商为准。
3. **不要宣称端到端 exactly-once。** Kafka/JetStream/网络超时都保留不确定结果；外部工具副作用继续使用 intent/result、幂等键和 `unknown_outcome`。
4. **不要用 A2A/MCP 替换内部账本。** 它们解决互操作，不覆盖 Pinvou 的 workspace/admission/fencing/audit 语义。
5. **不要默认打开 best-of-N 或动态 fan-out。** 新论文同时显示 token、协调和冲突成本，必须先过相对单 Agent 的价值门禁。
6. **不要因为采用 relay 而移除 Direct TLS。** relay 解决可达性，Direct TLS 解决离线/LAN、自托管和最小外部依赖；二者应共享业务合同。

## 13. 建议修订清单

| 优先级 | 目标文档/章节 | 建议修订 |
|---|---|---|
| P0 | 主蓝图 §8、§23 阶段 4/10 | `RelayTransport` 的 contract/测试前移阶段 4；托管规模化能力仍留阶段 10；保留 Direct TLS |
| P0 | 主蓝图 §15.7、§23 阶段 8/10、关键决策 32–33 | 拆出 `WorkspaceIsolationProvider`，在 fan-out 前自动 provision/verify distinct workspace；通用 cross-node sync/merge 后置 |
| P0 | 事件 schema + 主蓝图 §13–14 | `control`/`main` 独立 sequence/ACK；增加 R1 满载下 R0 延迟与重连测试 |
| P0 | Codex/Claude/CodeBuddy Adapter 合同 | 混合 stdout/JSON-RPC 必须持续 drain；禁止用停止读取同时阻塞控制流 |
| P1 | 主蓝图 §15.7、恢复矩阵、术语表 | `WorkspaceWriteLease` 改名 Grant/Lock；区分账本 epoch 与存储端 executable fencing |
| P1 | 主蓝图 §13.1、TUI/ViewModel 合同 | 增加 cursor expired/compacted → snapshot + resubscribe，无丢口恢复 |
| P1 | 主蓝图 §15、§23 阶段 8 验收 | 增加相对 single-agent 的质量/时间/token/冲突/人工负担价值门禁 |
| P1 | 主蓝图 §14.5–15.8 的论文论据 | TIPEX/ALIGN/History Matters/AiFlow/LATTE/LAMaS 标为 research hypothesis；删除无条件保证措辞 |
| P1 | 威胁模型、§7.4、阶段 4 冻结 | 分离永久 Owner 授权与可轮换 transport/device credential；定义吊销/generation |
| P2 | A2A/MCP/Adapter 相关章节 | 增加有损映射表、capability negotiation、Artifact/ResourceRef 校验和 durable 边界 |
| P2 | 主蓝图 §17 | 明确 If-Match 或 If-Range 行为及 partial restart |
| P2 | §21、事件 schema、CI 规范 | version range + feature bits；N-1/N 双向兼容 golden；稳定 event identity；JSON unknown-field 策略 |
| P2 | 本地 IPC/阶段冻结 | UDS 目录/socket 权限 + peer credentials；Windows first-instance/logon SID 精确语义 |

## 14. 最终判断

Pinvou 规划要做的不是一个不存在的市场方向。OpenAI Codex、GitHub Copilot、Cursor、OpenHands 和 ACP 已共同验证“远程/后台 agent + 可恢复 session + 隔离 workspace”的需求；Kafka、Temporal、JetStream、SEDA、Kubernetes、Chubby、OCI、HTTP 和 Protobuf/Avro 又为可靠性、调度、fencing、资源与 schema 提供了成熟基础。

真正需要调整的是设计承诺的精确度和阶段依赖：relay 可达性、自动 workspace 隔离、控制流独立性和 cursor 失效恢复要更早闭合；`WorkspaceWriteLease`、A2A/MCP 映射和新论文结论要收窄；多 Agent 必须用 single-agent 基线证明价值。完成这些修订后，蓝图会更接近可验证的长期架构，而不是堆叠研究名词或市场功能列表。
