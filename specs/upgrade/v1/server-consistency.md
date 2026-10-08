# 升级服务端一致性与恢复规范

> 状态：V1.10，关键不变量草案，待合约冻结及实现验收
>
> 适用协议：`protocolVersion=1`
>
> 对齐基准：需求、技术设计与安全要求 V1.0。本文不是 OpenAPI、Schema、固定向量及真实平台验收已完成的声明。历史综合设计不构成规范输入。

依据：[产品需求](../../../docs/pinvou-upgrade-platform-requirements.zh-CN.md)、[技术设计](../../../docs/pinvou-upgrade-platform-technical-design.zh-CN.md)、[协议与安全要求](../../../docs/pinvou-upgrade-protocol-security.zh-CN.md)。发生冲突必须共同修订，不能以本草案覆盖主文档。

## 1. 范围与原则

本规范约束 `check/refresh`、`download-info`、三用途`validate/consume/cancel`、`consume-status`、独立对账/质量复核任务及`events`、后台正式版同步草稿创建及相关后台恢复任务。实现可以选择数据库和消息系统，但必须满足：同一业务请求只有一个结果、失效所有者不能提交、响应丢失能够恢复原结果、安全状态变化不能产生新的旧资格凭据、恢复不依赖客户端继续重试。

时间比较统一使用服务端可信 UTC：`now < deadline` 才表示有效，`now == deadline` 已过期。

## 2. 幂等记录

| 操作 | 幂等作用域 | 必须纳入规范请求摘要的字段 |
|---|---|---|
| check/refresh | installationScopeId + requestNonce | 完整规范化请求、客户端事实摘要 |
| download-info | installationScopeId + decisionId + packageId + downloadKey | 决定修订、渠道修订、文件身份、请求约束 |
| validate | installationScopeId + purpose + telemetrySessionId + validateKey | 当前决定/双链、模式、前置状态、精确字节、当前准备/冻结及适用备份、activate原staged修订 |
| consume | installationScopeId + purpose + transactionId + authorizationJti + consumeKey | 授权/当前事实、准备所有权、获批helper/接管计划、activate staged修订、recoveryHandleHash及规范请求摘要 |
| cancel | installationScopeId + purpose + telemetrySessionId + cancelKey | 被取消授权、当前修订、取消原因；已消费安全停止通过事务合法终态处理，不反转授权 |
| events | credentialType + purpose + sessionId/transactionId/taskId + eventId | scope、sequence、from/to state、occurredAt、原目标/窗口与事件摘要 |
| 独立任务/处置 | taskPurpose + scope + taskId/revision + actionKey | 管理身份/权限、原请求授权事务、永久停用及完整安全证明、复核目标/窗口 |
| 正式版同步草稿创建 | product + component + syncKey | 操作者、逐 targetKey 的来源 Stable Deployment ID/revision、Release/Release Target 身份与修订、所选 channel/targetKey 集合、逐渠道确认的完整投放配置 |

幂等记录只有 `processing` 和 `committed` 两类结果状态。同键不同规范请求摘要必须稳定返回 `IDEMPOTENCY_CONFLICT`；同键同摘要在 `committed` 后必须返回同一业务结果和凭据身份，但每次 HTTP 响应生成新的 `requestId`。唯一例外是已提交 events 使用的 keyId 后续被即时 deny：此时不重放成功结果，而按第 9 节返回稳定专用拒绝，领域结果仍不重复写入。

进入 `processing` 时，服务端必须在同一原子事务写入加密的最小恢复投影，不得只保存内存中的计算结果。恢复投影至少包含幂等作用域、规范请求摘要、当前阶段、读取集、待提交结果身份和所需安全上下文；不得存放明文 SN、原始 IP、私钥或完整下载 URL。

除 events 外，`committed` 幂等结果可在线恢复 24 小时；该期限只控制重放存储，不延长任何决定、URL 或安装授权的有效期。events 使用独立去重账本，至少保留到该请求事件凭据 `exp+2m`，覆盖完整 7/37 天上传窗口及处理裕量。

三用途及required/notRequired的consume均原子保存最小ConsumeOutcomeTombstone至authorizationExp+62d；包含最小索引、purpose/原授权与请求摘要绑定、recoveryHandleHash/statusBindingHash、consumed/cancelled/expired、transactionId及committedAt，不保存secret或完整可重放请求。精确域分离/JCS公式（包含product/component/purpose与全部绑定）以凭据规范§3.2为唯一来源，单NUL字节不能解释成反斜杠文本；必要索引采用受限存储，普通日志/审计不含可重放组合。24 小时内仍按普通规则重放原响应；普通恢复期结束后且 `now<authorizationExp+61d` 时，只能通过专用 consume-status 和 256-bit recovery secret 返回状态与 transactionId，不返回或续签安装/事件凭据。达到 +61d 后查询统一关闭，tombstone 在最后 1 天仅作不可查询的清理裕量。该 tombstone 只用于本地 consume_pending 对账，不产生安装资格；同键不同摘要仍冲突。`now >= authorizationExp+62d` 时后台强制删除 tombstone。

### 2.1 后台正式版同步

同步命令只接受显式选择的 Beta/Internal 目标集合，每个所选 targetKey 分别绑定同一 Release 下的有效 Stable 来源。最终事务比较全部来源 Stable Deployment revision 及其当前 active/展示下载资格、Release closed、Release Target approved、Artifact valid、Stable SupplyChainApproval 有效和操作员对全部所选渠道的权限；来源在准备与提交之间暂停、撤回、到期或审批失效时零写入。提交点还读取各目标 scope 的当前基线，若同步版本不高于基线则拒绝整批并提示该目标无需同步；unactivated scope 无基线比较。一次批次记录与全部目标 Deployment/Rollout draft、审计和幂等结果原子提交；既有同版本目标投放冲突也使整批零写入，不静默覆盖。来源策略经逐渠道确认后才写入草稿，不复制任何审批或当前投放资格。

每个批次的 `channel + targetKey` 映射唯一；同渠道同 Release Target 的并发同步必须串行检查既有投放并拒绝重复创建，不仅依赖不同 syncKey 的请求去重。同键同摘要在恢复窗口重放原批次与对象身份，同键不同摘要按 IDEMPOTENCY_CONFLICT 拒绝。24 小时幂等恢复窗口结束后仍保留批次到目标对象的唯一映射及原请求摘要；重复旧请求定位原批次，同键不同摘要仍冲突，不能因响应丢失、恢复或过期重试而再次创建。该最小持久映射只用于后台投放审计和去重，不包含客户端凭据。

草稿创建不更新 selectionGeneration/current 元数据。之后每个渠道通过既有状态命令独立审批与投放，重试基于目标对象的当前状态和 revision，不再创建新草稿；后续资格变化仍受 PublishSelectionChange 原子规则约束。同步批次只记录步骤结果，不能作为绕过渠道 SupplyChainApproval、首次激活、候选唯一约束或基线切换的授权。来源投放在草稿创建后暂停/撤回不联动目标；共享 Release Target/Artifact 安全状态仍进入每个目标渠道的最终复核读取集。

同步目标在审核、激活及 Rollout completed 的最终事务重读并锁定/比较目标渠道当前基线指针和 revision；基线存在时同步版本必须严格高于基线，否则零写入并提示无需同步。验收同时覆盖草稿创建之后、审核/激活/完成提交之前的基线推进竞争，不能降低基线或覆盖已生效的更高版本。

## 3. 所有者、租约与隔离

每个 `processing` 操作具有单调递增的 `ownerEpoch`、`ownerStartedAt`、`leaseExpiresAt`：

- 初始租约 30 秒，活动所有者每 10 秒续租一次。
- 单个所有者自 `ownerStartedAt` 起最多持有 2 分钟；达到边界必须由新 `ownerEpoch` 接管，不能无限续租。
- 每次最终领域写入和幂等提交必须在同一原子事务中执行 CAS：`ownerEpoch` 仍匹配、`now < leaseExpiresAt` 且 `now < ownerStartedAt + 2m`。
- 任一条件失败时，旧所有者必须丢弃预计算结果和未签发凭据，返回可恢复冲突；它不能在新所有者接管后提交。

后台恢复任务至少每 30 秒扫描一次已过租约的 `processing` 记录，并以更高 `ownerEpoch` 接管。恢复必须依据持久化投影重新读取当前安全状态并重算，不能信任旧内存结果，也不要求客户端再次发送请求。

## 4. 选择读取集与最终复核

`RootPublishHead` 按 `product` 唯一，绑定 current Root 的 identity/version/bytesHash；Root 首次建立和每次逐版本轮换都只能由 `PublishRootChange` 对该 head 做 CAS，同版本不同字节或非连续版本直接拒绝。PublishRootChange 取得该行排他锁；组件发布在同一可串行化事务中取得共享锁并在提交点比较 revision，二者不能穿越提交。后继 Root 必须仍能验证所有 active component 的 current 未过期元数据链；移除旧角色 key 前必须先经过“Root 同时信任新旧 key→各 component 用新 key 刷新→后继 Root 移除旧 key”的顺序，否则轮换拒绝。`MetadataPublishHead` 按 `product+component` 唯一，绑定 current Timestamp/Snapshot 的 identity/version/revision/bytesHash，并记录最近一次组件发布所依据的 `rootHeadRevisionAtPublish`。全新 component 在产品 Root 已建立但尚无已激活 scope 时使用 `published=false` 的唯一 genesis component head：首次建立时 `headRevision=0`，Timestamp/Snapshot 与 rootHeadRevisionAtPublish 明确 absent；产品 Root 后续轮换不需要逐个改写 genesis head。只有首次激活可将 component head CAS 为 `published=true` 并写入首组 Timestamp/Snapshot。

`rootHeadRevisionAtPublish` 是历史发布栅栏，可小于 current RootPublishHead revision；读取时不要求二者相等，而是要求该 component 的 current 完整元数据链能由 current Root 验证且不回退、未过期。下一次组件发布必须绑定 current Root head，并把该字段提升为提交时 revision。

所有组件发布均同时绑定 `baseRootPublishHead + baseMetadataPublishHead`。所有扩大资格或改变可选择集合的操作统一使用 `PublishSelectionChange`：预签受影响全部 Target、完整新 Snapshot 和 successor Timestamp，绑定两级 base head、受影响 SelectionScope/entity revision 与完整安全读集；最终提交在同一数据库事务断言 RootPublishHead 未变化并 CAS component head，写业务状态、各 scope selectionGeneration、current Target/Snapshot/Timestamp 指针和新 component head。不同 scope 的发布竞争同一 component head；不同 component 与 Root 轮换则竞争同一产品 RootPublishHead 读版本，Root 一旦变化，所有基于旧 Root 的组件发布必须失败并按新 Root 重建、重签。CAS 失败禁止覆盖或合并陈旧 Snapshot。

PublishSelectionChange 至少覆盖 supportFloor 变化、Deployment 激活/恢复及影响选择的 revision、Rollout 全部可见状态/policy、BridgeEligibility 启用/禁用/重启用、SupplyChainApproval 恢复/扩大、Registry 状态/transform/版本变化及任何其他 generation 变化。Registry 影响多个渠道时，所有受影响 Target/generation 与全局 Snapshot/Timestamp 全成或全败。安全收紧允许先原子写即时 deny、generation 和 `metadata_sync_pending`，但同一事务必须按 `product+component` upsert 唯一 `MetadataReconciliationJob(pendingGeneration, denyRevision, jobRevision, status=pending)`；不得先提交 pending 再异步登记任务。直到 PublishSelectionChange 追平前，check/refresh/download-info/validate/consume 都失败关闭，不得用新 generation 配旧 Target 签发凭据。

reconciliation worker 至少每 30 秒扫描 pending/job；每次从最新 RootPublishHead、MetadataPublishHead 和完整安全读集重建、重签，通过 PublishSelectionChange 追平全部受影响 Target/Snapshot/Timestamp，并只在同一成功提交中清除对应 metadata_sync_pending、完成匹配 jobRevision。重复执行幂等；若 generation/deny/jobRevision 已变化则旧任务零写入并由最新任务接管。依赖可用时重试退避上限 5 分钟，pending 超过 15 分钟必须告警但继续失败关闭和重试，禁止人工直接清标记。

Root 首次建立与轮换只使用产品级 `PublishRootChange`；它 CAS RootPublishHead，并原子切换 current Root 指针。Release 续签以及不改变可选择集合的 Target/Snapshot/Timestamp 续签使用 `PublishMetadataRefresh`：绑定 base RootPublishHead、base MetadataPublishHead 和完整 current 集合，预签 successor 元数据，在一个提交中断言产品 Root head 未变化、CAS component head并切换全部 current 指针；除非同时改变可选择集合，否则不递增 selectionGeneration。任一冲突都必须基于最新两级 head 重建并重签。

check 首先读取 SelectionScope。若为 unactivated，固定返回 HTTP 409、`SCOPE_UNACTIVATED`、`retryable=true` 和普通检查间隔，不创建 telemetrySession、决定凭据或幂等恢复中的 metadataSet；该结果仍按 check 幂等键稳定重放。以下成功选择规则仅适用于 active scope。

`check/refresh` 的一致性读取集至少包含：current RootPublishHead、current MetadataPublishHead 与 metadataSet（Root/Timestamp/Snapshot/Target 及 endpointChain/hopChain 引用的全部 Release Manifest）的身份、version/revision、expiresAt 和指针，Source Facts Registry 版本、画像及 forward-only policy/命中边、`channelRevision`、`selectionGeneration`、即时 deny/metadata_sync_pending、渠道基线指针、候选 Rollout 的 ID/revision/state/灰度策略修订，endpointChain 与 hopChain 各自的 Deployment、Release、Release Target、Package/Artifact 修订、正向状态和 `installNotBefore/installNotAfter`，适用的 BridgeEligibility，以及两条链各自的 SupplyChainApproval ID/revision/status/effectiveExpiresAt。`Target.selectionGeneration` 必须等于 SelectionScope 当前值，否则任何动态端点失败关闭。

在签发决定凭据并提交幂等结果的同一原子边界，服务端必须重新读取并比较上述读取集：

- 元数据链任一角色过期、指针/revision变化或Registry/generation/实体变化，废弃预计算结果和未提交凭据，以当前事实完整重算。两链还绑定releaseVisibleAt、endpoint.upgradeType/hop认证模式及适用ordinary独立前置路径审批；展示/下载/preinstall不提前要求installNotBefore，install/activate与边界前复核要求安装区间。
- endpoint要求active Deployment/closed Release/approved Target/valid Artifact/有效本渠道供应链，candidate另需同父running Rollout和灰度命中。ordinary实际跳同一正向链，非终点须本渠道有效基线或current Target/Snapshot独立前置路径审批，跳自身Rollout paused/aborted拒绝；bridge仅换superseded+有效BridgeEligibility且满足同渠道历史资格。未命中候选/仅制品批准不能借有效终点下发。两链任一deny失败关闭。
- 决定凭据中的完整元数据链、Registry、generation、endpointChain、hopChain 和实体修订必须与最终复核值一致。

`download-info` 必须在签发 URL 并提交幂等结果的同一原子边界复核决定修订、endpointKind、hopKind、channelRevision、完整 metadataSet、generation、Registry 画像、即时 deny、基线指针、endpointChain 与 hopChain 各自的正向实体状态/修订/安装有效期/SupplyChainApproval，以及 hopChain packageId；endpointKind=candidate 时复核 Rollout ID/revision/state/策略修订，hopKind=bridge 时复核 BridgeEligibility ID/revision/state，candidate-bridge 同时复核两组。任一失配均不得签发新 URL。已签发 URL 不主动延长，最多残留原 15 分钟；后续 validate/consume 仍失败关闭。

`validate` 和 `consume` 也必须在最终提交前复核决定/授权绑定的 endpointKind、hopKind、channelRevision、完整 metadataSet、generation、Registry 画像、即时 deny、基线指针、endpointChain 与 hopChain 各自的正向实体状态/修订/安装有效期/SupplyChainApproval，并按 endpointKind/hopKind 分别复核 Rollout 和 BridgeEligibility；candidate-bridge 必须同时通过两组复核。复核还含ordinary路径审批、purpose/mode、准备所有权及本次冻结/备份；activate比较waiting修订/原预安装完成/原期限。metadata续签或任一链失配要求新check；install/activate紧前在线复核后才越界，preinstall无活动写入。全部时间可信UTC且分层使用visible/start/end，end=null不比较；消费后资格/渠道收紧按需求§6.2用途×边界矩阵，不泛化为强杀或继续所有用途。

## 5. validate 协调

同一 telemetrySession 同时只能有一个服务端可见的 `activeValidateOperation`。创建该标记、写入 validate 幂等记录和取得所有者必须在同一原子事务完成。

合法终态为 `authorized`、`authorization_failed` 或 `coordination_aborted`。`coordination_aborted` 是持久化终态，不能回退为 ready，也不能复用旧 lineage；客户端需要重新 check。只有authorized且用途前置条件齐全的授权可消费；preinstall不得要求活动数据备份或normal用户确认。重入validate不能借准备旧代次/解冻后旧备份重用。

## 6. 授权消费、取消与过期

每个preinstall/install/activate授权有独立purpose、精确绑定记录和单调 `authorizationRevision`，生命周期固定为 `available → consumed|cancelled|expired`。三个终态不可离开，并在同一记录上以预期 revision 执行 CAS：

- consume 仅能把未过期的 available 原子改为 consumed，并在同一事务创建唯一 transaction；
- cancel 仅能把尚未消费且未过期的 available 原子改为 cancelled；
- 当 `now >= exp` 时，后台任务或任一访问端点把 available 原子改为 expired；即使投影尚未写入，consume/cancel 也必须按服务端可信时间拒绝。

consume、cancel 和 expire 的任意竞争只有一个 CAS 成功；失败方返回已经提交的稳定终态，不能创建第二个 transaction。各自的幂等记录用于恢复响应，不取代共享授权记录的互斥 CAS。

渠道切换使用同一 installationScope 的序列化事务：增加 channelRevision，把未消费会话置为 channel_changed、active validate 置为 coordination_aborted、available 授权置为 cancelled。它与 consume 竞争同一授权 revision；切换先提交则不能 consume，consume先提交只允许install/activate冻结旧revision继续既有核对/events；preinstall进行中安全取消清理，尚未消费激活的staged失效，新渠道相同package不复用。事务取消是独立安全终态，不把consumed授权改回cancelled。

## 7. 首次激活与基线切换

SelectionScope 使用 `unactivated|active` 和单调 scopeRevision。unactivated scope 的首个 Deployment 执行 in_review→scheduled 时，以 scope revision 和预期 next deployment revision 做 CAS，在同一提交校验 Release/Package 签名及正向实体条件，并保存已验签的 `StagedActivationSet(Target, Snapshot)`。每个 staged set 绑定 `baseRootPublishHead`、`baseMetadataPublishHead`、Root/角色密钥、supportFloor/Deployment/Release/Release Target/Artifact/SupplyChainApproval 的 identity+revision+bytesHash 完整读集，且 `expiresAt<=createdAt+15m`；Snapshot 包含 component base head 的全部 current Target/Release 引用再加入新 Target，不能只含新 scope。

首次激活命令锁定 unactivated、scheduled Deployment、staged set revision、产品 RootPublishHead 和 component MetadataPublishHead，在提交点重读完整绑定。只有两级 base head 与 current head 完全相等且 staged 未过期时，才能执行 Deployment scheduled→active，并以 PublishSelectionChange 提交 scope active、唯一 baseline pointer、current Target/Snapshot、新 Timestamp 指针和 selectionGeneration=1。任一产品 Root、component head、supportFloor 或实体 revision 变化、CAS 冲突或到期均零写入，必须基于最新完整集合重建并重签 staged；同 component 的不同 scope 并发激活竞争同一 component head，Root 轮换与所有 component 发布竞争产品 Root head。active scope 的基线唯一约束不能延迟到异步任务。

staged 对象使用独立加密存储和发布服务最小权限，不进入普通日志、运营导出或公共缓存。激活成功、被新 revision 替换或到期后 24 小时内强制删除完整 Target/Snapshot/locator，只保留对象摘要、操作者、原因和时间的审计记录；重复任务不得延长 TTL。

Rollout 可见状态、policy 或候选引用 revision 变化时，命令必须通过 PublishSelectionChange 生成 fresh commitment salt、新 current Timestamp/Snapshot/Target 并递增 selectionGeneration。当前链切换到更高 approved SupplyChainApproval revision 以恢复或扩大资格时，也通过该命令更换审批指针、递增 generation 并刷新 current 元数据；不能只更新审批表。

Rollout completed 只接受 running 状态；paused 必须先以显式 resume 命令返回 running 并刷新 commitment/current Timestamp/Snapshot/Target/generation。当前 baseline Deployment 只能在 Rollout completed 的原子指针切换中进入 superseded，普通 supersede 命令若命中当前 baseline 必须零写入拒绝；paused/withdrawn保留唯一基线指针，不能删除再首次激活降低基线；选择按状态失败关闭。completed还需当前候选严格高于基线、100%阶段完整时长及每组样本/真实率/未知上界通过、无冻结，最终事务比较质量读集及全部原因水位，任一变化零写入并重评估。

## 8. 会话、事务与备份超时

创建每份telemetrySession注册sessionStartedAt+24h持久任务；24h只限制会话，当前资格下按凭据规范§3.1续接同workflow未来下载，不设置逻辑下载终止条件；恢复扫描至少每 30 秒补偿。到达边界后任何访问先执行一个阶段化超时事务：authorized 会话写 authorization_expired，关联 available 授权记录写 expired，两者使用 session/authorization 预期 revision 同时提交；authorization_requested 与更早阶段的会话写 expired，同时关闭并提升 activeValidateOperation fence/ownerEpoch，使旧 worker 永不能提交 authorized。timeout、validate 最终提交、consume、cancel 读取和竞争同一 session/authorization revision，最多一个成功；任一部分 CAS 失败则整个事务零写入并按已提交结果重算。7天合法晚报只诊断原会话或追加独立质量投影，不能改会话终态/新动作；按凭据规范续接新会话时不清除原workflow阶段和质量结果。

创建 transaction 时注册 `transactionStartedAt + 30d` 的持久定时任务；恢复扫描至少每 30 秒补偿丢失任务。到达边界后任何读取或写入先按有效终态处理，并以transaction revision CAS按purpose写终态：install/activate为failed_manual_repair_required(reason=reporting_timeout)，preinstall仅staging_failed且不建立应用门；staging_completed是已有终态，不等重启或槽到期。客户端终态事件先提交则超时 CAS 失败；超时先提交则晚到事件只写独立诊断记录，不修改领域终态。该机制不依赖客户端重试，告警在边界前发现积压。

服务端在 backup_succeeded 但未创建 transaction 的会话进入任一终态时，在同一事务登记 `backupCleanupAt=terminalAt+24h`，供客户端下次连接同步；它不能直接删除本机文件。客户端 helper 必须在写入首个备份字节前，先原子创建受保护 LocalBackupRecord，记录 staging、密钥引用、sessionExpiresAt 与保守 `localCleanupAt=sessionExpiresAt+24h`。launcher/helper 的本地恢复扫描跨重启且不依赖网络：仅已确认无消费/无受保护写入、session_retention到期时清理完整快照；已有ConsumeAttempt pending未知即按pending规则，不能因缺transaction ACK误按会话到期清理；backup_failed、创建中崩溃或孤立 staging/密钥引用按失败清理规则收敛。

所有purpose及required/notRequired在发consume前均生成不可预测256-bit consumeRecoverySecret，只在受保护ConsumeAttempt保存，required的LocalBackupRecord关联该attempt；请求只携带 `recoveryHandleHash=SHA-256(secret)`。客户端按凭据规范对排除自身digest等字段的完整语义请求计算consumeRequestDigest，持有产品锁、唯一准备所有权并以记录revision CAS写入 `consume_pending(transactionId, authorizationJti, consumeKey, authorizationExp, consumeRequestDigest, recoverySecret, pendingCleanupAt=authorizationExp+61d)` 后才能发网。服务端创建 consume processing reservation 时把 recoveryHandleHash 纳入规范请求摘要和恢复投影，并以请求字段计算 statusBindingHash。

consume 胜出时，authorization→consumed、transaction 创建、session→authorization_consumed、普通 committed 幂等结果和 ConsumeOutcomeTombstone 必须在同一原子提交写入；已注册 consume attempt 与 cancel/expire 竞争时，胜方也在写授权终态的同一提交写对应 cancelled/expired tombstone。若终态先发生、consume 后到达，则 consume 最终事务在复核终态后原子提交同一 tombstone 与幂等结果。任何一步不能异步补写。

consume-status输入为recovery secret、product/component、installationScopeId、purpose、authorizationJti、consumeKey、transactionId及consumeRequestDigest；原字段/Hash规范以凭据规范§3.2为准。服务端以 secret 计算 recoveryHandleHash，再按规范公式计算 statusBindingHash，与 tombstone 值做常量时间比较；不接受仅凭 installId、installationScopeId、transactionId 或幂等键的查询。任一字段错误、记录不存在或边界关闭都返回同一无信息错误并共享限流策略。它只在 `now<authorizationExp+61d` 时返回 consumed/cancelled/expired 与 transactionId，达到边界后无论 tombstone 是否仍存在都返回统一关闭结果；不返回制品、事件内容或任何凭据。secret及完整可重放组合不得写URL/普通日志/指标/审计正文，受保护审计只保留用途需要的无秘密最小lineage/摘要。

全部用途确认consumed后以产品锁更新ConsumeAttempt的原事务归属；required另把LocalBackupRecord转transaction_retention，保存服务端transactionStartedAt并固定hardCleanupAt=transactionStartedAt+60d，notRequired不创建备份记录；确认 cancelled/expired 后转 terminal/session retention。在 consume_pending 的 `now == pendingCleanupAt`，或 transaction_retention 的 `now == hardCleanupAt` 及之后，无论网络状态和服务端查询是否可用，launcher/helper 都必须在下一次本地扫描中删除平台管理的快照、staging 与密钥引用并进入 `pending_cleanup_completed`；运行中每10min扫描，关机跨边界下次启动且联网前扫描；此为正常已认证可信期限能力的规则，普通离线/日期改变不能触发故障例外。用户明确导出的副本使用独立路径、记录和审计，不能修改或延长平台 LocalBackupRecord。本机在硬上限前取得可信 succeeded/abandoned/failed 终态时，清理时刻改为 `min(hardCleanupAt, terminalAt+7d|1d|30d)`。consume_pending、transaction_retention、清理与 helper 接管竞争同一产品锁和 LocalBackupRecord revision；任一 CAS 失败零写入重算。

### 8.1 本地停用、故障例外及独立对账

准备、执行和对账为不同保护状态。全部用途首次consume最多5min等待，不能因ACK丢失/断网推定取消。仍有效所有者证明未越界、无本次活动写入、旧版/数据完整、全部本次执行者停止并永久fencing原授权/事务/epoch，才能local_execution_retired、解除自己的冻结/实体锁；跨重启保留独立范围占用，拒绝新准备/执行。normal/silent无既有门可用健康旧版，forced/独立失败门不绕过；预安装只隔离槽。

联网合法恢复原结果，核对未消费取消/失效或已消费安全取消/既有终态后，以独立处置解除占用。旧窗口/secret不可用时，MFA最小权限管理角色签发独立任务，绑定原请求/授权事务/scope/purpose、本地永久停用证明与任务修订；可信helper核对全部安全事实，服务端核对旧授权及占用后原子追加唯一处置/审计，不改旧终态或补旧事件/执行权。缺失历史保留未知，证据不足保持保护及前向修复；失败/丢ACK恢复同任务，再升级须全新准备/required备份。

confirmed retention_time_fault仅保留既有加密备份及解密密钥引用/访问控制，三语告知可能超期及修复/管理员明确删除入口；立即删consume secret及副本，禁止新增依赖可信期限的动作/staged激活，已越界安全收敛。恢复原期限首轮≤10min清理，永久故障由管理员明确删除；不延服务端+61d/+62d，普通断网/回拨/延期不豁免硬上限。删除secret/备份/过期槽不释放对账占用或复活旧执行。

## 9. 事件签名密钥 deny

events 在提交事件与状态迁移的同一原子边界，重新读取 credential keyId、purpose 对应的即时 deny revision。正常轮换但未 deny 的旧 keyId 在凭据窗口内可用；紧急 deny 后旧凭据即使签名和状态边合法也拒绝。事件去重账本与状态迁移同时提交并保留至凭据 exp+2m；deny 在读取后变化时 CAS 失败并重算，不能提交 succeeded。当前 key denied 时，无论同 eventId/同摘要是否已 committed，对外统一返回 `EVENT_KEY_DENIED`、HTTP 401、`retryable=false`；已提交事实只写内部审计，不返回原事件内容、成功结果或存在性。未 deny 时才按普通幂等规则重放原结果。同 eventId 不同摘要永远不得产生第二次领域写入。客户端保留本地诊断并停止该 lineage 上传；受影响 lineage 不补发凭据，未来升级另行新 check。

## 10. 质量投影、复核与查询权限

LogicalDownloadRecord独立于24h遥测会话，保留首次开始、最新合法进度/等待状态及已审批失联判定参数；原workflow未终结、精确内容/当前资格有效才续接，以workflow revision CAS唯一当前可写会话并fencing旧会话的新动作，旧会话只合法补报。丢ACK恢复同一结果，明确终态不可覆盖。最小关联记录受限保存以支持仍在进行的流程，完成/取消/资格失效按既有审计与质量保留策略收敛；记录不可用时新流程可重验复用同一缓存，原未知/失败保留，不能因保留窗口或缺记录强制重新下载。

workflowId固定首次资格时的Deployment/Rollout/原阶段、scope/targetKey、来源、实际hop、upgradeType与精确目标Package/模式；下载续接会话/换源/重试不改变归属。QualityProjection与不可改写事务终态分开；同scope同窗口同原分组最多一份样本，真实失败不能被新流程覆盖。合法较晚证据可追加更新投影，超旧凭据期不补签；未知不冒充本机失败，已登记未投影事实按未完成/未知阻止评估通过。

扩量/解冻/completed最终事务比较stagePlan/Rollout/scope/两链、投影revision/事件处理水位、所有FreezeReason及窗口读集；新执行/事件/对账/原因使旧评估失效。逐有样本来源/hop所有必需指标的确认分母达到审批最小样本，真实率失败/(成功+失败)及保守上界(失败+未完成+未知)/(成功+失败+未完成+未知)严格低于阈值且完整时长结束才通过；零分母样本不足。下载持续有合法进度/可确认等待状态计未完成，没有24h未知或终止阈值；按审批的有限进度上报/失联间隔无有效状态才未知，超过阈值可冻结但不终止续传，新合法状态/完成证据去重更新原流程投影而不自动解锁。不因会话到期伪造失败；用户取消/延期/资格安全取消分开，不塞执行失败分母。

RecoveryReview覆盖当前阶段及全部原原因组、各原阈值/最小样本、独立完整窗口，不能挑成功范围。专用任务只本次精确原目标/数据/健康；原安装合法成功+当前精确同内容+窗口内健康才计原安装/健康成功，原失败持续失败、未知持续上界，更高关联修复只设备恢复。解除冻结需权限/原因/审批/审计，不增百分比，提交后同组再完整独立观察；新原因补入，改代码/制品终止原投放发更高版本。暂停资格恢复与质量解冻分开，无冻结时从恢复时刻正常重新观察，不要求暂停期间新样本；基线completed恢复不重启Rollout。

普通运营只能状态/冻结原因及两类隔离周报：ReleaseQualityWeeklySnapshot七维完整叶子每scope每周首流程、multi_source分类、周末+7d冻结；NetworkPrivacyWeeklyCube五维完整叶子且scope/前缀贡献限制。均k≥20可见，否则无人数的样本不足/suppressed；禁合计/边际/任意或相邻窗/过滤/下钻/联查关联键/补算，迟到结果不改快照。实时投影/逐实例/复核仅内部自动化、受控故障/审计角色，不能旁路普通运营抑制；网络原始边缘日志7d/用途隔离摘要30d及跨周HMAC规则依安全要求§15。公开遥测只自动冻结/告警，永久停投/撤回/吊销由授权人员复核。

## 11. 故障注入验收

正式版同步另须覆盖：无显式渠道选择拒绝、部分渠道无权限整批零写入、部分 targetKey 缺少有效 Stable 来源、来源在准备与提交间暂停/撤回/到期/吊销、目标基线在提交前推进至相同或更高版本、草稿与批次/幂等结果提交任一点崩溃、响应丢失和超过 24 小时重试、同键不同配置、不同键并发同步同一目标、已有同版本投放冲突、目标首次激活与候选唯一约束冲突。草稿创建最多一次，各渠道后续独立审批和暂停/撤回，共享制品吊销仍使全部引用渠道失败关闭。

至少覆盖：响应提交前后断连、事务提交未知、租约到期前/等于/后 1 ms、旧所有者延迟提交、恢复任务重复执行、两级 metadata head 与跨 component/scope 并发发布、PublishMetadataRefresh 与选择发布并发、双 component genesis/active Root 轮换与同版本分叉拒绝、staging 后其他 scope 发布/Root 或 supportFloor 变化、所有 generation 变化的 Target 相等校验、Registry 多渠道全成全败、紧急 deny 事务提交前/任务登记点/签名前/发布前崩溃后的 reconciliation 收敛、元数据/SupplyChainApproval 到期或新 revision 恢复、deny/generation/Registry/基线/Rollout/BridgeEligibility 在计算与提交之间变化、四种 endpoint/hop 组合的两链分别失效、unactivated 固定响应、staged 15m/清理 24h 边界、paused Rollout 直接 complete 拒绝、当前 baseline 普通 supersede 拒绝、validate 双并发、渠道切换与 consume 竞争、24 小时会话四方竞争、30 天事务超时、事件 key deny 外部不可区分、consume 领域提交后 tombstone 前崩溃零窗口、正确 secret 配错误 consumeKey/digest、pendingCleanupAt 前 1ms/等于/后 1ms及关机跨边界重启、confirmed consumed 后永久离线 hardCleanupAt、consume-status +61d 关闭与 tombstone +62d 删除、部分备份写入崩溃，以及 consume/cancel/expire 的全部竞争顺序。同一授权/请求任何顺序不得产生两个结果、两个相应用途事务、丢失tombstone或失效资格新凭据。另覆盖三用途×备份策略错绑/未知停用5min/查询丢失独立任务/lateACK不复活，准备先于冻结、多用户/槽互斥/原预安装终态确认，取消激活新修订继承期限，计时故障secret立即删除/恢复清理，下载跨24h及多会话阶段归属不变，全部质量原因/精确目标/较高修复不充样本，证据迟到最终评估冲突及两类查询族不可旁路。

附加故障注入：下载超过24h/长期网络等待且合法状态可确认只阻止扩量不自动冻结；审批失联间隔内外、凭据/会话轮换和丢ACK去重、新状态恢复独立投影不解冻结。原时间统计/公开周快照不被续接或迟到证据重写。

全部三用途及独立任务绑定purpose/type/audience及窗口；上传到期不再接受事件，不把7/37天误当可无限补传。普通幂等完整响应超过24h后不再重发原凭据，有副作用操作仍依授权/任务/业务唯一索引及最小请求摘要拒绝重建同一次结果；缺少合法恢复证明不得复活旧执行，续下载使用新的当前资格和新会话。
