# 升级会话、三用途事务与保护状态机规范

> 状态：V1.10，关键不变量草案，待合约冻结及实现验收
>
> 适用协议：`protocolVersion=1`
>
> 对齐基准：需求、技术设计与安全要求 V1.0。本文规定应满足的约束，不表示 OpenAPI、Schema、固定向量或真实平台验收已完成。历史综合设计不构成规范输入。

依据：[产品需求](../../../docs/pinvou-upgrade-platform-requirements.zh-CN.md)、[技术设计](../../../docs/pinvou-upgrade-platform-technical-design.zh-CN.md)、[协议与安全要求](../../../docs/pinvou-upgrade-protocol-security.zh-CN.md)。发生冲突必须共同修订，不能以本草案覆盖主文档。

## 1. 通用规则

状态迁移由服务端按凭据、当前状态、单调序号和规范边校验。终态不可离开；重复上报同一事件标识通常返回原结果，但事件 key 后续被即时 deny 时按第 5 节返回专用稳定拒绝且不重复写入；序号回退、跳边、串用 session/transaction 或凭据过期均被拒绝。客户端日志不能直接改写服务端状态。

## 2. 遥测会话与准备

会话显式绑定 `purpose=preinstall|install|activate`，分别对应 silent 非活动槽、normal/forced 直接安装和 silent 重启激活，不能串用。以下状态是待 Schema/固定向量冻结的规范集合；未列出的边或守卫不满足均拒绝。普通 UI 不能伪造服务端内部状态。

- 检查创建 `decision_received → no_update|update_offered|expired`；no_update 为终态。`update_offered ↔ deferred`，只有 normal 可由本次实际 hop 的明确用户点击进入 user_confirmed；silent/forced 使用服务器已绑定的自动流程事实，不伪造用户确认。
- 下载用途 install/preinstall：normal 必须 `update_offered|deferred → user_confirmed → download_started`；silent/forced 在资格有效时 `update_offered|deferred → download_started`。`download_started → download_succeeded|download_failed|cancelled|expired`；`download_succeeded → verification_started → verification_succeeded|verification_failed|cancelled|expired`。短期会话到期只终结会话，下载缓存及逻辑流程按凭据规范的续接规则处理，不设置下载总时长终止条件。
- activate 不重复下载：用户应用重启或系统重启触发，新 check 精确选择原 staged hop，服务端确认原预安装 staging_completed、槽及原期限/渠道仍有效后 `update_offered → staged_verified`。staged_verified 与 verification_succeeded 均可进入 `preflight_started → preflight_succeeded|preflight_failed`。
- `preflight_succeeded → permission_checked → permission_granted|permission_denied`；normal/forced 在冻结前完成必要 OS 权限确认，silent 只核对既有 helper 权限，缺权不自动提权。preinstall 从 permission_granted 直接进入 preparation_ready，且不得冻结/备份活动数据。
- install/activate 从 permission_granted 在协调写入者前取得唯一准备所有权，进入 `preparation_started → writers_frozen|preparation_failed`。required 时 `writers_frozen → backup_started → backup_succeeded|backup_failed`，验证本次冻结快照后进入 preparation_ready；notRequired 直接 `writers_frozen → preparation_ready`，不生成备份记录/成功事件；irreversible+notRequired 拒绝。
- preinstall 也须受保护记录和范围互斥，但不能冻结活动业务。服务端确认所有用途对应准备事实后，`preparation_ready → authorization_requested → authorized|authorization_failed|coordination_aborted`；只有授权服务写这三个结果。每会话最多一个 active validate，协调中止是终态，不复用旧 lineage。
- `authorized → authorization_consumed|cancelled_before_install|authorization_expired` 与独立授权 `available → consumed|cancelled|expired` 同事务提交；取消或到期不能终结已经 consumed 的执行事务。授权消费把准备所有权连续转交唯一新事务。
- 除已终态/已消费外，安全取消、业务失败、会话到期或渠道改变可依对应守卫终结为 cancelled、阶段失败、expired 或 channel_changed。终态不离开；取消/到期仅服务端会话结果，本地解冻还须确认本次所有者安全停止且无活动版本/数据执行写入。消费未知不能假取消，按 §3.2 保留对账占用。

每份会话自 sessionStartedAt+24h 收敛：authorized 与 available 授权原子进入 authorization_expired/expired，authorization_requested 及更早阶段进入 expired 并 fencing active validate。timeout、validate、consume、cancel 竞争同一 session/authorization revision，CAS 任一失配整笔零写入。7d 合法补报仅诊断旧会话或更新独立质量证据，不复活旧会话；会话到期不是下载失败、逻辑下载终止或新安装权。安装/激活准备不跨其当前新会话期限。

## 3. 三用途事务、槽位与保护

consume 只从 authorized 创建与 purpose 一致的唯一 transaction；事务 sequence 从1独立计数，遥测凭据不能推进它。服务端消费终态与执行终态分开：已消费执行安全取消按install的abandoned_before_install/activate的cancelled_before_install区分，preinstall为staging_cancelled；消费前放弃为会话取消，二者不能与授权记录 cancelled 混淆。

| purpose | 正常执行边 | 终态及安全规则 |
|---|---|---|
| preinstall | authorization_consumed → staging_started → staging_verified → staging_completed | staging_failed/staging_cancelled 需安全停止并隔离/清理槽；不写活动数据、不跨 executionCommitBoundary、不建立应用修复门 |
| install | authorization_consumed → helper_plan_started（仅获批计划需要时）→ execution_ready → installer_started → reconciling → installation_verified → health_check_started → succeeded | 调用平台安装器的瞬间越界；未越界且证明安全停止可abandoned_before_install，越界失败/不能证明成功为 failed_manual_repair_required |
| activate | authorization_consumed → activation_ready → activation_started → reconciling → installation_verified → health_check_started → succeeded | 最后在线复核后，首次活动数据写入或活动指针切换较早者越界；迁移在指针切换前完成，越界失败同 install，不重启旧版 |

install无需helper迁移时显式允许authorization_consumed→execution_ready，有计划时helper_plan_started→execution_ready；每条阶段失败只从对应阶段的已列出失败边产生，不能从任意状态伪造authorization_failed。preinstall的授权消费后尚未开始写槽可安全终结staging_cancelled，写槽后须停止并隔离/清理；install/activate的非终态只有按边界及安全事实进入对应取消/失败终态。

helper/安装器/槽位及全部受影响数据身份事实驱动每条边；helper 计划失败不凭阶段名称一律推定应用已损坏，须按实际边界及能否证明旧版/数据安全分支。activation_started 必须携带受保护最早越界事实，不能按进程存在/指针已切而忽略先前迁移；install 不能用旧 protectedMutationStarted 布尔量把已调用平台安装器解释为未越界。可执行/迁移/受保护 helper 写入与接管仅在相应用途消费后，准备记录/冻结/备份允许消费前建立。

直接安装/激活消费后30d仍非终态，服务端 CAS 为 failed_manual_repair_required(reason=reporting_timeout)；preinstall 为 staging_failed(reason=reporting_timeout)，不产生应用门。37d内合法晚报只诊断并可更新独立 QualityProjection，不改终态/历史评估/本机门。服务端缺报不证明本机失败，已本地验证 succeeded 不重新封锁。各执行/校验/健康预算以需求为准，有限预算耗尽的真实本地失败与上报未知分别记录。

### 3.1 待激活槽

staging_completed、staging_failed、staging_cancelled 结束预安装执行占用；`staged_waiting_restart` 是独立内容状态，不是活跃事务。StagedRecord 绑定 scope/channelRevision、原预安装事务、Deployment/hop、Package/制品及模式、原质量阶段、slotIdentity、stagedAt、revision，`stagedValidUntil=min(stagedAt+30d,installNotAfter)`，end=null 时30d。创建/替换/激活/清理/新修订登记仍共用范围互斥。

新激活 consume 原子比较 waiting revision，置 activating、创建新 transactionId 并连续转交准备/冻结所有权。必须原预安装已合法提交 staging_completed；服务端结果未知、失败或无法合法确认时不得激活，完成适用对账并清理后新预安装。暂停隐藏提示、保留原期限但不激活；恢复后在线精确匹配才恢复提示；到期、撤销、改选及渠道变化立即失去资格并安全清理。

直接安装边界前安全取消的规范终态为abandoned_before_install；激活为cancelled_before_install，两者对备份留存均属安全放弃分类。

只有原激活服务端确认 cancelled_before_install、本地证明未越界/无活动写入、旧版/槽完整、原渠道/期限和精确选择仍有效，才终结原 activating revision 并原子建立唯一更高 waiting revision，关联原预安装/取消事务且继承原期限；响应丢失恢复同一新修订。下次用户/系统重启完整重新准备、required备份、新授权/新事务。消费/取消未知不得重建，其他终态/越界失败不复活槽，未完成安全停用的 activating 槽不能被普通清理。

### 3.2 准备、消费未知及独立对账

范围保护状态为 `idle → preparing → executing → idle|repair_protected`；本地永久停用只能进入 `reconciliation_required`，业务是否可用与能否再次升级分开。所有权在协调/冻结/备份前建立并持久化跨重启代次，消费后连续转交；旧所有者写入/解冻/启动/清理失效。per-machine覆盖全部用户、断开会话和后台写入者及受影响数据。槽内容不占执行权但仍互斥；已有门不因新的准备释放而解除。

三用途及两种备份策略发请求前均建立 ConsumeAttempt/recovery绑定，首次 consume 最多一次5min等待，重试/重启不刷新。未知消费不是 cancelled；仍有效所有者证明未越界、无活动写入、旧版/数据完整、全部本次执行者停止并永久 fencing 原授权/事务/代次，才持久化 local_execution_retired、解除自己的冻结和实体锁，保留跨重启独立对账占用阻止新准备/执行。normal/silent 无门时可用健康旧版，forced/失败门按当前事实保持；预安装只隔离槽。迟到 ACK/旧 helper 不复活旧执行权，清理到期备份/secret/槽也不解占用。

正常恢复确认旧消费结果后，以独立处置解除对账；+61d查询关闭或secret丢失时，MFA/最小权限任务核对旧请求摘要/scope/purpose/授权事务、旧执行永久失效、无写入者及旧版/数据安全，服务端核对旧授权和占用，全部满足才追加幂等处置。历史缺失保留未知，旧终态不可改写，任务不能下载/执行/补签旧事件；证明不足保持保护及前向修复入口，再升级完整新准备及 required备份。

### 3.3 强制门、修复门与期限

GateRecord按 scope/原endpoint/当前hop/关联事务分别持久化。强制门当前hop未越界且健康、终点可信失效时安全取消后可解本次强制门；仅路径失效保持受限门，过去hop越界不影响新hop判定。独立失败门只由同scope/原失败关联、严格高于本机受保护写入最高目标且forced时高于原终点、完整活动身份/全部数据/健康成功的前向修复解除，原失败不改写。时间未知不提前入门、不凭日期解门；离线未来forced决定及越界状态按需求§5.5/§17.1。

required备份首字节登记、初始24h/consume_pending+61d/confirmed consumed60d及终态7/1/30d较早期限按需求§11.5，本地预算/可信期限不因重启/离线/普通日期变更延长。notRequired没有备份但仍有ConsumeAttempt。仅确认计时组件损坏进入 retention_time_fault，保留加密备份/解密密钥、三语告知并禁新依赖动作，立即删recovery secret及副本，不延服务端查询；恢复按原期限首轮≤10min清理，永久故障由管理员明确删除。删除期限对象不解除对账占用；越界安全收敛不被故障中止。

## 4. 发布实体状态机

合法边固定如下，未列出的单步转换全部拒绝：

- Artifact：`uploading → validating`；`validating → valid|rejected`；`valid → quarantined`。`rejected/quarantined` 为终态。
- Release：`draft → assembled`；`assembled → closed|cancelled`。`closed/cancelled` 为终态。
- Release Target：`draft → in_review|cancelled`；`in_review → approved|rejected`；`approved → revoked`。高权限 CancelRelease 专用地允许 in_review→cancelled，普通单Target命令不得使用。`cancelled/rejected/revoked` 为终态。
- Deployment：`draft → in_review`；`in_review → scheduled|rejected`；`scheduled → active|paused`；`active → paused`；`paused → active`；`scheduled|active|paused → withdrawn|superseded`。`rejected/withdrawn/superseded` 为终态；但当前 baseline 只能在 Rollout completed 的原子基线切换中进入 superseded，普通 supersede 命令必须拒绝。superseded 只有另行审批的 BridgeEligibility 可以赋予桥接资格，不能恢复为 active。
- Rollout：`draft → running|aborted`；`running → paused|completed|aborted`；`paused → running|aborted`。`completed/aborted` 为终态；paused 不能直接 completed。
- SupplyChainApproval：`pending → approved|rejected`；`approved → expired|revoked`。rejected/expired/revoked 为终态；扩大或恢复资格创建更高 revision。
- SelectionScope：`unactivated → active`。active 不回退；停用渠道通过撤回基线投放表达，不能删除高水位。

Stable 正式版显式同步仅为所选 Beta/Internal 渠道创建新的 Deployment draft 和关联 Rollout draft，复用 approved Release Target 及不可变制品；不复制来源 Deployment 状态、SupplyChainApproval、BridgeEligibility、基线身份或 Rollout 进度。每个目标分别沿上述合法边完成审批和激活，unactivated scope 仍执行首次激活，已有 scope 仍执行候选与基线切换。同步不提供 draft→active/completed 快捷边，不改变客户端渠道修订。来源 Stable Deployment 的后续暂停/撤回不级联到目标；共享 Release Target 吊销或 Artifact 隔离仍使各引用投放的正向资格失败关闭。

同步目标审核、激活及 Rollout completed 还必须在最终提交点比较本渠道当前基线：已有基线时目标版本严格高于基线，否则拒绝转换且零写入，提示无需同步。该守卫防止草稿创建后本渠道基线推进导致旧同步版本覆盖新基线；unactivated scope 无基线比较。

Rollout 只有在父 Deployment active 且 SupplyChainApproval 有效时可进入 running。父 Deployment active→paused 与关联 running Rollout→paused 原子提交，恢复时父 Deployment paused→active 与 Rollout paused→running 也原子提交。draft/running/paused→aborted 必须写入封闭枚举 abortCause：operator_abort 与父 Deployment→withdrawn 同事务提交；parent_withdrawn 或 parent_superseded 仅由对应父状态命令级联写入，父 Deployment 保持 withdrawn 或 superseded。每次 candidate 可见状态、policy 或引用 revision 变化都必须通过 PublishSelectionChange，在同一提交生成 fresh commitment salt、新 current Timestamp/Snapshot/Target 和递增 selectionGeneration。Rollout completed 仅允许 Rollout=running、父 Deployment active、渠道 SupplyChainApproval 有效、有效灰度 100%、审批通过且同一提交时间戳下安装区间/两链/版本条件有效、候选严格高于基线、最终100%阶段自身完整时长/逐组最小样本/真实失败率及未知上界通过、无冻结锁存，并通过 PublishSelectionChange 与新基线指针、旧基线 superseded、规范空候选承诺、新 current Timestamp/Snapshot/Target 和 selectionGeneration 递增共同提交；paused 必须先经 PublishSelectionChange resume，直接 completed 拒绝。到达 installNotAfter 或安全例外 expiresAt 的边界时整个命令失败且不得写入。active SelectionScope 必须恰有一个基线指针且最多一个 running Rollout，每个 Deployment 最多一个非终态 Rollout；unactivated 没有基线，唯一约束冲突使整个命令失败。

Rollout运行中阶段计划、SN包含/排除组不可改写；百分比只由系统按审批计划在当前阶段完整时长、逐来源/hop必需指标最小样本、真实失败率和未知上界严格低于阈值且无冻结时自动进入紧邻阶段，不允许人工推进、降低或跳级。expandPolicy比较预期计划/Rollout/质量证据水位及所有冻结原因、父链/审批/时间，原子PublishSelectionChange；条件或水位变化零写入。需要改计划/SN组时终止原投放并建新审批修订。

SupplyChainApproval 的 `effectiveExpiresAt` 是其自身及所有绑定例外非空 expiresAt 的最小值；无到期项时为空。到达非空 effectiveExpiresAt 时资格在服务端可信时间边界立即为 false；持久定时任务在同一事务写入 expired、即时 deny、递增 selectionGeneration、metadata_sync_pending 与唯一 MetadataReconciliationJob，恢复扫描补偿任务延迟。当前链切换到更高 approved revision 以恢复或扩大资格时使用 PublishSelectionChange，递增 SelectionScope generation 并发布新 current Timestamp/Snapshot/Target，不能只替换审批指针。即使状态投影尚未刷新，所有守卫也直接按 effectiveExpiresAt 或 pending 状态失败关闭。

任何会增加 selectionGeneration 或改变可选择集合的非紧急命令都必须作为 PublishSelectionChange，比较产品 RootPublishHead、CAS product/component MetadataPublishHead，在一个提交中写实体 revision、受影响全部 scope generation、current Target、完整 Snapshot 和 successor Timestamp。该规则覆盖 supportFloor、Deployment、Rollout、BridgeEligibility、SupplyChainApproval 和 Registry；Registry 多渠道扇出全成或全败。紧急收紧可先写即时 deny、generation 和 metadata_sync_pending，但同事务必须登记持久化 reconciliation job；恢复器以最新 head 发布并仅在成功提交清 pending，元数据追平前全部动态端点失败关闭。所有动态端点固定守卫 `Target.selectionGeneration == SelectionScope.selectionGeneration`。

每次转换必须携带预期 revision 并以 CAS 提交；同一 revision 的竞争转换最多一个成功，失败方重新读取后返回稳定冲突。安全终止不可逆：quarantined Artifact/revoked Release Target及withdrawn/superseded Deployment不能用新审批修订复活同一对象，须合法新对象/制品及完整审批。Deployment达到releaseVisibleAt且未到非空installNotAfter可按合法排期进入active，允许早于installNotBefore展示/下载/预安装；直接安装/激活及forced入门仍要求安装区间。到达非空 installNotAfter 不产生隐式生命周期转换，而是使 `effectiveEligibility=false`。后台可刷新该投影，但即使投影尚未刷新，动态端点也必须直接按时间窗失败关闭。

跨实体边的正向守卫固定为：Release draft→assembled 要求业务字段和至少一个 Target 组成的非空目标集合冻结；Release Target draft→in_review 及 in_review→approved 要求父 Release assembled、全部引用 Artifact valid、Package Manifest 与供应链闸门通过；Release assembled→closed 要求全部 Target 位于 approved/rejected/revoked/cancelled 且至少一个 approved；自动 assembled→cancelled 要求全部 Target 均位于 rejected/revoked/cancelled，draft/in_review 存在时不得因零 approved 取消。主动放弃只能由高权限 CancelRelease 以父子 revision CAS，在同一事务执行 draft/in_review Target→cancelled、approved Target→revoked、Release→cancelled及审计；任一 CAS 失败零写入。Deployment in_review→scheduled、scheduled→active 或 paused→active 要求父 Release closed、Release Target approved、全部 Artifact valid、同渠道 SupplyChainApproval approved/未过期、元数据有效且安装时间窗合法。唯一例外是 unactivated scope 的首个 in_review→scheduled：它验证签名 Release/Package 和绑定预期 scheduled revision 的未过期 StagedActivationSet，不要求不存在的 current Target/Snapshot；active scope 不适用该例外。守卫在转换提交点重新检查，任一非正向状态均拒绝。选择和动态端点始终使用 current 完整元数据链及同一正向条件；bridge hop 仅把 Deployment active 替换为 superseded+BridgeEligibility enabled，其他条件不放宽。

SelectionScope 首次激活以 scope revision、scheduled Deployment revision、StagedActivationSet revision、产品 RootPublishHead 和 product/component MetadataPublishHead 做比较/CAS。staged set 绑定两级 base head、Root/角色密钥、support floor 与所有实体 revision，最长有效 15 分钟，Snapshot 包含 component base head 的完整 current 集合。提交点全部重读相等后，才以 PublishSelectionChange 执行 Deployment scheduled→active，并写 scope active、baseline pointer、current Target/Snapshot、新 Timestamp 指针和 selectionGeneration=1；任一产品 Root head、component head或实体 revision 变化、过期或 CAS 失败零写入并基于最新完整集合重签。同 component 的不同 scope 并发激活竞争 component head，Root 轮换与所有 component 发布比较同一产品 Root head。staged 集合从不被 unactivated check 返回，并在成功、替换或到期后 24 小时内删除完整内容；该 check 固定返回 HTTP 409、`SCOPE_UNACTIVATED`、`retryable=true` 且不创建 decision 或 telemetrySession。

## 5. 凭据与事件窗口

遥测会话和安装事务的事件时间、上传窗口分别服从可信元数据与凭据规范。事件凭据只允许推进其绑定状态机，不赋予下载、安装或文件访问权。事件状态提交前必须复核 credential keyId/purpose 的即时 deny；正常轮换未 deny 的旧 key 可用，紧急 deny 后失败关闭。去重账本保留至凭据 exp+2m；deny 后无论事件是否已在此前提交，对外统一返回 `EVENT_KEY_DENIED`，不暴露存在性、不重放成功结果且不重复迁移，提交事实仅内部审计。上传凭据过期后拒绝事件；较晚上传仅指事件发生窗口已关闭但原上传凭据仍有效，不能改变发布资格、重新执行或离开终态。独立任务使用专用凭据，只追加对账处置/本次精确目标复核，不能补签旧事件。质量投影和两类权限隔离周报服从需求§8.2.1/§8.4及一致性规范，不把事务终态、未知、较高版本修复或公开遥测当作旧候选成功。

## 6. 验收

正式版同步模型须覆盖草稿不产生资格、各目标渠道分别审批和激活、拒绝跨渠道审批复用及快捷状态边、各渠道独立暂停/撤回、共享制品吊销影响全部引用渠道，以及不因同版本正式化或更低 Stable 版本而产生升级。

模型测试必须遍历客户端及全部发布实体的合法边和单步非法边；并发测试至少覆盖重复/乱序事件、双 validate、consume/cancel/expire、24 小时会话四方竞争、30/37 天窗口、key deny 外部不可区分、consume 状态与 tombstone 原子崩溃、secret/key/digest 错绑、pendingCleanupAt 与 transaction hardCleanupAt、Release 零 approved 但仍有 in_review、全部失败终态自动取消、CancelRelease 父子 CAS、paused Rollout 直接 complete、当前 baseline 普通 supersede、StagedActivationSet 两级 base head 变化/跨 scope 双并发/15m/24h 清理、PublishMetadataRefresh 与选择发布并发、双 component genesis/active Root 轮换及分叉拒绝、reconciliation job 崩溃恢复、SupplyChainApproval 到期恢复、Bridge/Registry 多渠道 PublishSelectionChange 全成全败、Target generation 不等、每种非正向父/子状态、三种 abortCause、directInstall调用安装器/activate最早活动写入边界前后及安装有效期边界。每个授权消费/同一幂等请求最多创建一个相应用途事务与唯一outcome，激活必须使用新授权/新事务；任一component只有一个current元数据head。另覆盖三用途错绑、消费未知5min/永久停用/独立任务、staged取消后新修订、准备先于冻结/多用户/旧epoch、暂停恢复精确槽、两类周报及质量复核原目标/原失败与更高修复不可替代；模型断言只限定同一请求/授权，不阻止合法下一跳或新激活。

下载可上报有凭据/单调sequence及helper事实的download_progress/download_waiting（网络等待或用户暂停），更新进度旁记录而不改变download_started主状态；按审批的有限上报/失联间隔无有效状态才在独立质量投影为未知，不因24h终止/冻结。新会话续接需原workflow未终结及精确身份/当前资格合法，唯一可写会话按workflow修订CAS转交；确认上下文的update_offered→download_resume_context→download_started是独立合法边（normal仍绑定原点击/本次hop），不允许一般新会话跳过normal确认或其他前置边；续接不刷新校验/执行预算，旧凭据key deny不补旧事件。

范围保护安全取消须仍有效所有者，满足事实后preparing/executing→idle；未知消费永久停用为preparing/executing→reconciliation_required，合法独立处置才→idle，不能从无秘密对账证据推定历史成功。已越界超时进入repair_protected，仍在运行写入者保持接管/冻结，前向修复须写入者已停止并完整核对后才可取得独立执行权，晚到成功退出码不改旧失败或自动解门。
