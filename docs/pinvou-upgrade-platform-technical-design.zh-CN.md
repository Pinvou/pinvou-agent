# 品悟统一升级平台技术设计

> 状态：V1.0，架构与模块边界草案
>
> 日期：2026-10-08
>
> 适用产品：Pinvou Agent
>
> 产品基准：需求文档 V1.0；本次修订不代表下层规范或实现已完成验收

## 1. 文档定位

本文只定义升级平台的总体架构、模块责任、关键数据流和必须由后续版本化规范实现的技术约束，不重复产品策略、密钥细节或逐字段协议。

- 产品范围和用户可观察行为以 [品悟统一升级平台需求文档](pinvou-upgrade-platform-requirements.zh-CN.md) 为准。
- 信任边界和最低安全结果以 [品悟升级协议与安全要求](pinvou-upgrade-protocol-security.zh-CN.md) 为准。
- 本文不能增加一期产品范围，也不能降低前两份文档的验收要求。

原综合设计已移至 `docs/archive/pinvou-upgrade-platform-comprehensive-design-record.zh-CN.md`，仅供追溯，不是规范输入。

## 2. 规范层级与模块化制品

### 2.1 权威关系

产品需求文档与安全要求文档是相互正交的最高层权威，不存在“产品高于安全”或“安全高于产品”的线性优先级：

- 产品需求文档决定产品范围、业务规则和用户可观察行为，但不能降低安全要求规定的最低保护结果。
- 安全要求文档决定信任边界、准入条件和最低安全结果，但不能自行增加产品范围或改变业务策略。
- 已冻结的版本化 OpenAPI、JSON Schema、状态机和测试向量必须实现二者的交集；本技术设计负责架构与模块边界。
- 示例、历史记录和实现注释仅用于说明，不构成规范依据。

产品与安全要求发生交叉冲突时必须失败关闭，阻止开发完成或发布，并由两份主文档共同修订形成一致结论；实现、技术设计或下层规范不得自行选择其一。

### 2.2 规范制品索引

下列制品必须在对应功能进入开发完成门槛前落盘；缺少、版本未冻结或合约测试未通过都表示该模块尚未完成：

| 模块 | 规范路径 | 责任人 | 内容 |
|---|---|---|---|
| 控制 API | `specs/upgrade/v1/openapi.yaml` | 升级服务负责人 | 路径、请求响应、错误码、幂等 Header |
| 元数据 Schema | `specs/upgrade/v1/schemas/metadata/` | 发布平台负责人 | Target、Release、Package、Snapshot 等 Schema |
| 动态 API Schema | `specs/upgrade/v1/schemas/api/` | 升级服务负责人 | check、download-info、三用途 validate/consume/cancel、consume-status、独立对账/质量复核任务及 events |
| 状态机 | `specs/upgrade/v1/state-machines.md` | 客户端与服务端共同负责人 | 三用途遥测/事务、准备及对账所有权、待激活修订、升级/修复门、发布实体状态 |
| 服务端一致性与恢复 | `specs/upgrade/v1/server-consistency.md` | 升级服务负责人 | 幂等、所有者隔离、双链/用途资格、独立对账、质量投影、灰度复核及后台恢复 |
| 来源事实注册表 | `specs/upgrade/v1/source-facts-registry.md` | 客户端与升级服务共同负责人 | 已安装事实、迁移画像、状态及世代联动 |
| 可信元数据与凭据 | `specs/upgrade/v1/trusted-metadata-and-credentials.md` | 安全负责人 | 元数据角色、字段绑定、时限、密钥操作 |
| 规范化与签名向量 | `specs/upgrade/v1/vectors/crypto/` | 安全负责人 | JCS、哈希、签名、令牌绑定固定向量 |
| 选择算法向量 | `specs/upgrade/v1/vectors/selection/` | 产品与服务端共同负责人 | support floor、minimum source、灰度、桥接 |
| 平台契约 | `specs/upgrade/v1/platforms/` | 各平台负责人 | 两种激活模式、执行边界、多用户冻结、阶段预算、可信期限与健康检查 |
| 备份格式 | `specs/upgrade/v1/backup/` | 客户端负责人 | scope、容器、加密、冻结和恢复接口 |

本次待提交的四份Markdown规范按需求、技术设计和安全要求V1.0共同修订关键不变量，文本一致性审核不替代OpenAPI、JSON Schema和固定测试向量，也不表示合约或实现验收通过。第 2.2 节中与待交付功能有关的全部制品必须完成本次变更的兼容性评审、落盘、冻结及合约测试，才能宣称模块开发完成或作为上线基线。首次冻结统一标记 protocolVersion=1；破坏性变更提升协议或 Schema 版本。本文第 20 章追踪需求到模块及验证入口，不表示测试已经通过。

## 3. 总体架构

```text
管理后台
  ├─ 上传与制品校验
  ├─ Release Target 审批
  ├─ Deployment / Rollout 管理
  └─ 暂停、撤回、吊销、隔离与审计
           │
           ▼
升级控制面（update.pinvou.com）
  ├─ 元数据与选择服务
  ├─ 文件信息服务
  ├─ 三用途授权与独立对账服务
  ├─ 事件、阶段质量投影与复核任务
  └─ 恢复与运维任务
           │                    ┌─ 品悟对象存储
           ├─ 返回文件身份和 URL ├─ CDN
           │                    └─ 第三方文件服务
           ▼
桌面应用 → 稳定 launcher → 版本化 helper → 平台适配器（直接安装/非活动槽/重启激活）
```

控制面不代理大文件。文件服务只获得制品路径或短期签名参数，不获得 SN、安装标识、决定凭据或安装授权。客户端升级协调器管理类型交互、准备所有权、槽位和升级门；helper 执行受保护操作，平台适配器提供 OS 能力。Pinvou 业务逻辑置于前后端 features/upgrade/，宿主及 OS 差异经平台接口提供；跨 feature 复用的 OS 原语才进入全局 platform。依赖方向保持 app→features→platform/core，不在 CodeWhale 重实现升级业务或基础 Agent 生命周期。

## 4. 后台领域模型

### 4.1 实体责任

| 实体 | 唯一标识 | 责任与状态 |
|---|---|---|
| Artifact | artifactId | 内容不可变文件；uploading→validating，validating→valid/rejected，valid→quarantined |
| Package | packageId | 客户端下载单元；引用 Artifact 并绑定最终安装器身份 |
| Release | releaseId | 同一产品版本和发布说明的多平台逻辑分组；draft→assembled，assembled→closed/cancelled |
| Release Target | releaseTargetId | releaseId+targetKey 的制品、来源和迁移审批单元；draft→in_review/cancelled，in_review→approved/rejected，approved→revoked |
| Deployment | deploymentId | 一个批准目标在一个渠道的投放；draft→in_review，in_review→scheduled/rejected，scheduled→active，active↔paused，scheduled/active/paused→withdrawn/superseded |
| Rollout | rolloutId | Deployment 的灰度扩量；draft→running/aborted，running→paused/completed/aborted，paused→running/aborted |
| SupplyChainApproval | releaseTargetId+channel+revision | 渠道级供应链审批；pending→approved/rejected，approved→expired/revoked |

Release 不承担暂停、撤回或安全吊销。管理后台上的“暂停发布”操作必须明确落到具体 Deployment；“安全吊销目标”落到 Release Target；“隔离文件”落到 Artifact。

正式版同步由后台投放编排模块负责，复用既有发布实体和状态机，不增加渠道继承或客户端跨渠道选择。操作员显式选择同一 Release 的平台目标和 Beta/Internal 目标渠道，每个所选 targetKey 分别绑定自己的来源 Stable Deployment；同一事务校验全部来源当前 active 且展示/下载资格有效、引用的 Release closed、Release Target approved、Artifact valid、Stable SupplyChainApproval 有效及操作者对所选渠道的权限，再创建同步批次与各渠道 Deployment/Rollout draft。来源身份和修订、目标集合、逐渠道确认的配置纳入请求摘要与审计；重复及并发处理遵循服务端一致性规范的后台同步规则。

各目标 Deployment 引用对应 targetKey 的同一不可变 Release Target/Package/Artifact，但拥有独立投放配置、Rollout 和同渠道 SupplyChainApproval。来源配置仅供确认，审批结果、BridgeEligibility、当前基线指针、实际灰度进度及质量观察不作为可复制资格。同步版本不高于目标 scope 当前基线时拒绝该所选集合并提示无需同步，不创建草稿或降低基线；unactivated scope 无基线比较。草稿创建不修改 current 元数据或 selectionGeneration；后续各渠道通过已有审批、首次激活或候选发布流程独立生效，所有影响资格的命令仍走 PublishSelectionChange。一个命令若改变多个 scope，仍须遵守该命令的全成或全败规则，不把分别提交的渠道审批和投放合并为隐式联动。

同步批次保存各目标对象及步骤结果，用于恢复、重试和审计，不成为新的资格实体。来源 Stable Deployment 在草稿创建之后的暂停/撤回不作为目标投放的额外守卫；各渠道投放独立处理。共享 Release Target 吊销或 Artifact 隔离继续通过现有正向实体守卫影响全部引用渠道。后台逐渠道呈现状态，不能以批次草稿已创建代替发布成功。

实体进入任何终态后不得原地修改业务字段；Release Target 进入 approved 后也不得原地修改。Deployment 使用封闭 `upgradeType=normal|silent|forced`；Release Target 分别绑定 `migrationMode=none|backwardCompatible|irreversible` 与 `backupPolicy=notRequired|required`，不可逆必须 required。Package Manifest 声明并认证 directInstall 和/或 stagedRestart，三种类型按需求 §5.5 使用对应模式，不能跨模式解释。更改制品、来源下界、类型、渠道策略、时间窗或迁移规则时创建新修订并重新审批。旧 backupRequired 表述不得作为新的可选产品策略；完整枚举与合法边在版本化规范中统一。

跨实体正向守卫由同一领域事务强制：Release draft→assembled 要求业务字段与至少一个 Target 组成的非空目标集合冻结；Release Target 进入 in_review/approved 要求父 Release assembled、全部 Artifact valid 且包/供应链校验通过；Release assembled→closed 允许目标处于 approved/rejected/revoked/cancelled，但要求全部 Target 终态且提交点至少一个 approved。自动 assembled→cancelled 只允许全部 Target 均为 rejected/revoked/cancelled；仍有 draft/in_review 时即使零 approved 也不得触发。显式 `CancelRelease` 是高权限 CAS 命令，在同一事务将 draft/in_review Target→cancelled、approved Target→revoked、Release→cancelled并写审计。Deployment 进入 scheduled/active 要求 Release closed、Release Target approved、全部 Artifact valid、渠道 SupplyChainApproval approved/未过期和合法时间窗。unactivated scope 的首次 scheduled 以签名 Release/Package 和不可公开的 StagedActivationSet 代替 current Target/Snapshot 检查；active scope 不适用该例外。选择与所有动态端点继续按 current 正向状态复核，不能只排除少数失败状态。

Rollout 完成采用 PublishSelectionChange 的可线性化跨实体命令：锁定 scope 基线、候选 revision 和两级发布 head；提交点验证父 Deployment active、Rollout running、候选严格高于当前基线、有效灰度 100%、最终阶段自身完整观察时长与逐分组合格样本/质量条件满足、无扩量冻结、审批有效且仍在安装有效区间，不能仅以百分比或未定义的资格布尔值替代这些条件。一次提交写 completed、新基线、旧基线 superseded、新 current 元数据和 generation；任一失败零写入。当前基线只能由该命令 superseded，普通 supersede 拒绝；paused/withdrawn 停投保留指针，不能清空后再次首次激活。父 Deployment pause/resume/withdraw/supersede 与进行中 Rollout 联动走 PublishSelectionChange，终态 Rollout 不重启；outbox 保证事件与领域写入一致。active scope 恰有一个基线指针、最多一个 running 候选，每个 Deployment 最多一个非终态 Rollout；unactivated 无基线。

普通新建、重建与同步投放复用同一 CandidateAboveBaselineGuard：审核、排期、激活/恢复、Rollout 启动/恢复、扩量及 completed 的最终提交均重读当前基线身份/revision，并要求候选终点严格更高。基线暂停/撤回不降低比较基准，冲突或不满足返回 candidate_not_above_baseline 且零写入；首次激活与路径中的合法中间跳不套用候选终点比较。

管理编排模块在审核前拒绝非法或不可执行时间窗：installNotBefore 必填，非空 installNotAfter 须严格晚于 installNotBefore 与 releaseVisibleAt，silent 可见至终点须有可完成预安装区间；forced 完整路径对 support floor 与来源登记内全部受支持来源验证 directInstall 认证、所有 hop 可见时间不晚于 forced.installNotBefore，且各 hop 安装区间覆盖强制终点整个区间。无日历终点时所有 hop 也须无终点，不能排除不兼容来源后继续激活。SN 包含/排除组和 1—100 严格递增、最终 100% 的阶段计划一并审批；启动后不可改写，排除优先于包含、再按稳定百分比分桶，缺失/关闭 SN 在不足 100% 时走基线、100% 时命中候选；成为基线后不再应用排除组。

暂停投放恢复由管理命令校验实体链、审批、时间窗、路径、原计划及暂停原因合法处置，原子恢复同一百分比资格，不以暂停期间被禁止的新样本为前置条件，不清除质量冻结。没有冻结时自动启动当前阶段完整重新观察；有冻结时另按 §16.1 发起复核再解除。基线自身恢复不做候选比较；其 Rollout 不存在或已 completed 时只恢复基线资格，不启动灰度或重新切换基线。终止/吊销/隔离状态不可恢复。

### 4.2 即时安全状态

安全操作先写即时 deny，再异步刷新签名元数据和缓存。动态接口在最终提交前重新读取 deny 和选择世代，确保状态变更后不再产生新的旧资格决定或授权。

SupplyChainApproval 或其例外到期/吊销时，以同一紧急收紧事务写即时 deny、增加 selectionGeneration 并置 metadata_sync_pending。Deployment 进入 scheduled/active、从 paused 恢复，Rollout start/expand/complete 和所有新下载/安装资格均绑定并复核 approval ID/revision/effectiveExpiresAt；draft/in_review 可在审批前存在但没有投放资格。当前链切换到新批准 revision 以恢复或扩大资格时必须通过 PublishSelectionChange 原子增加 selectionGeneration 并发布新 current Timestamp/Snapshot/Target；不同渠道的审批不能复用。

`RootPublishHead` 按 product 唯一，绑定 current Root 的 identity/version/bytesHash；Root 首次建立及逐版本轮换只通过 `PublishRootChange` CAS 该产品级 head，同版本不同字节和非连续版本拒绝。RootChange 对产品 head 取排他锁；组件发布在同一可串行化事务中取共享锁并于提交点比较 revision。后继 Root 必须能验证每个 active component 的 current 未过期元数据；角色 key 退出采用先双信任、再逐 component 刷新、最后移除旧 key 的顺序。`MetadataPublishHead` 按 product+component 绑定 current Timestamp/Snapshot 和 `rootHeadRevisionAtPublish`；尚无 active scope 的 component 使用 `published=false`、revision=0、Timestamp/Snapshot/rootHeadRevisionAtPublish absent 的 genesis component head。Root 轮换不逐个改写 component head，但每次组件发布都绑定并在最终事务同时比较 base RootPublishHead 与 base MetadataPublishHead。只有首次激活可把 component head 置为 `published=true`。

`rootHeadRevisionAtPublish` 可落后于产品 current Root revision；读取不以二者相等为条件，而是以 current Root 能否验证该 component 当前完整链为准。下一次组件发布绑定最新 Root head并提升该字段。

所有扩大资格或改变可选择集合的操作调用统一 `PublishSelectionChange`：以两级 base head 预签受影响全部 Target、完整 Snapshot 和 successor Timestamp，在同一事务断言产品 Root head 未变化、CAS component head并提交实体 revision、各 scope generation 和 current 指针。它覆盖 supportFloor、Deployment、Rollout、BridgeEligibility、SupplyChainApproval、Source Facts Registry 及其他 generation 变化；跨渠道 Registry 扇出全成或全败。紧急收紧可先写 deny、generation 和 metadata_sync_pending，但同一事务 upsert 唯一 MetadataReconciliationJob。恢复器至少每 30 秒扫描，以最新两级 head 和完整集合重建；只在 PublishSelectionChange 成功的同一提交清 pending/完成匹配 job，冲突重算、退避不超过 5 分钟，15 分钟未收敛告警。元数据追平前所有动态端点失败关闭，禁止人工直接清 pending。动态端点强制 `Target.selectionGeneration == SelectionScope.selectionGeneration`。

不改变可选择集合的 Release 续签或 Target/Snapshot/Timestamp 续签走 `PublishMetadataRefresh`：绑定两级 base head，基于完整 current 集合预签，在一个事务比较 Root head、CAS component head并提交新 current 指针；除非同时改变可选择集合，否则不递增 selectionGeneration。Root 只由 PublishRootChange 更新产品级 head。任何冲突都基于最新两级 head 重建并重签，不允许陈旧 Snapshot 覆盖并发结果。

## 5. 平台与目标身份

`targetKey` 由 `os + arch + packageFormat + installScope` 唯一确定。首期目标：

| targetKey | 宿主要求 |
|---|---|
| `windows-x86_64-nsis-perMachine` | Windows x86_64 workstation |
| `linux-x86_64-deb-perMachine` | Ubuntu x86_64 Desktop |
| `linux-arm64-deb-perMachine` | Ubuntu arm64 Desktop |
| `macos-universal-dmg-perMachine` | Intel 与 Apple Silicon 分别匹配 hostArch |

Target Metadata 按 `product + component + channel + targetKey` 发布，包含：

- supportFloorVersion。
- installScope 和允许的宿主系统范围。
- 当前 selectionGeneration。
- 当前公开基线 Release 引用，以及不可反查版本的 `rolloutSetCommitment`。候选版本、Release ID 和可解析 Manifest 地址不出现在公共 Target Metadata。

每个 SelectionScope 最多一个 running 候选。Target 中的 `rolloutSetCommitment` 是 SHA-256 域分离承诺：对空集合使用规范空值；存在候选时，承诺规范化的 targetKey、Deployment/Rollout ID 与 revision、Release envelope SHA-256 和服务端随机生成的 256-bit leafSalt。leafSalt 不公开。check 命中后通过决定绑定、有效期不超过决定凭据且最长 15 分钟的不透明 locator 返回候选 Release，并返回上述规范字段和 leafSalt 作为 opening。客户端重算承诺并验证相等后才接受 Release；签名合法但 opening 不属于当前 Target 的候选必须拒绝。未命中决定不返回 opening、locator、候选 ID、版本或内容哈希。随机盐使公共承诺不能被版本字典反查；该承诺不是存储键或 URL。

Rollout 的 draft→running、running→paused、paused→running、任意非终态→aborted、completed，以及 policy/reference revision 变化，都通过 PublishSelectionChange 生成 fresh leafSalt、新 current Timestamp/Snapshot/Target 并递增 selectionGeneration；空集合使用规范空承诺。旧 opening 对新 Target 必须验证失败。

新增 per-user 安装时创建新的 targetKey、平台适配器和协议能力，不能复用 per-machine 的事务和渠道记录。

## 6. 上传包与制品存储

### 6.1 上传与下载对象分离

管理后台接收一个复合上传 ZIP，其中包含 Package Manifest、必需 FullPack 和一期为空的增量候选数组。外层 ZIP 只是上传传输容器，其大小和哈希保存在上传会话中，不写入其内部 Package Manifest。

后台负责验证：

- 外层上传包、目录安全和 Package Manifest。
- FullPack 子包容器及最终安装器。
- helper/launcher 身份。
- SBOM、构建来源证明和漏洞闸门。
- 平台、版本、架构和 installScope 一致性。
- 激活模式与目标能力认证、迁移及备份策略合法组合；irreversible 的备份能力须已在真实来源场景完成验证，模式/所需能力不支持则不能审核发布。

解析成功后，FullPack 和未来每个 IncrementalPack 分别成为独立 Artifact/Package。客户端只下载本次选择的一个 packageId，不下载外层复合 ZIP。

客户端验证顺序：可信元数据和 Package Manifest → 下载子包大小/哈希 → 安全解压 → 最终安装器大小/哈希 → 平台真实性。

### 6.2 身份字段

- Package Manifest 内部可有非安全逻辑 manifestId，但不得包含自身封装哈希。权威 `manifestEnvelopeSha256` 由 Release 对完整签名封装外部计算并引用；它逐项绑定内部 Artifact，不绑定包含自身的外层 ZIP 字节哈希，也不具有单一 packageId。
- 每个 FullPack 或 IncrementalPack 是独立下载单元，拥有全局唯一 packageId、size、sha256。
- `resultArtifactSize + resultArtifactSha256` 标识最终交给操作系统的安装器。
- 同一 packageId 的内容不可替换；需要修改时生成新 Artifact、packageId 和审批修订。
- Package Manifest 不设置到期时间；当前可安装性由 Target、Deployment、安全状态和在线授权决定。

## 7. 一期增量边界

一期生产能力严格限定为：

- 生产 Release Target 的 `incrementalPackages` 必须为 `[]`。
- 正式客户端上报空的增量算法和格式能力，只选择 FullPack。
- OpenAPI、JSON Schema、事件和包身份字段预留增量候选语义。
- 合约测试可使用非空候选夹具，验证服务端字段解析/返回，以及客户端在能力为空时忽略候选并选择完整包。
- 一期不实现生产补丁上传、基础安装器缓存、补丁重建、增量下载或增量安装。

二期启用前必须新增算法规范、资源预算、参考重建验证、真实平台测试和独立发布开关；不得修改一期完整包字段语义。

## 8. 版本选择

### 8.1 顺序

服务端在一个一致性选择快照中执行：

1. 匹配 product、component、channel、targetKey 和协议能力。
2. 检查 supportFloorVersion；低于下界直接返回人工升级入口。
3. 使用展示/下载正向条件：已达 releaseVisibleAt 且尚未到非空 installNotAfter；endpoint 必须 Deployment active、Release closed、Release Target approved、Artifact valid、渠道供应链审批有效，candidate 还需同父 Deployment 的 Rollout running。ordinary hop 满足同一正向条件，bridge 仅替换为 superseded+BridgeEligibility enabled。installNotBefore 不阻止展示、用户发起后的 normal 下载、forced 预下载或 silent 非活动槽预安装；它只限制直接安装/激活资格。空 installNotAfter 无日历终点。
4. 从当前 Snapshot 只读取当前 channel/targetKey 的唯一基线指针和运行中候选 Rollout 的 ID、revision、状态及灰度策略修订；不回退到 Stable 或其他渠道。同一正式版只有其本渠道 Deployment 生效后才进入该渠道的选择集合。
5. 使用 SN 组和百分比分桶确定期望终点；SN 不参与安全准入。
6. 若当前来源满足期望终点的 minimumSourceVersion 和能力要求，返回终点。
7. 否则从当前渠道有效基线、明确批准的 active 前置路径跳或合法 superseded 历史桥接中，选择严格高于当前版本且能够继续通往期望终点的最高下一跳。
8. 没有路径时返回稳定无更新原因或协议升级人工入口。

一次决定只包含一个目标版本。安装完成后客户端重新检查。

显式同步不改变客户端 channel/channelRevision，也不绕过 `targetVersion > currentVersion`。当前版本相同或更高时不返回同步版本；已有更高候选仍按本渠道灰度及可达路径选择。版本成熟度通过渠道表达，不引入 prerelease 排序或同版本重装。

所有候选操作复用 §4 的 CandidateAboveBaselineGuard，不限于同步投放；基线比较与状态/元数据发布在同一最终事务完成，不以草稿创建时的旧基线覆盖并发推进。所有 endpoint 与实际 hop 均遵守 targetVersion>currentVersion，中间跳严格向上且不高于期望终点。

Source Facts Registry将稳定已安装事实投影ID与动态owner/freeze代次分开；sourceProfileId哈希排除自身及policy/transform/资格引用，完整Registry另作摘要，避免自引用/策略边循环，动态事实仍绑定clientFactsDigest并由helper复核。状态/计划变化新Registry修订，不用重新登记每个实际ownerEpoch。`forward-only`画像必须引用不可变ForwardPathPolicy。每条允许边精确绑定来源画像、目标 Release Target/更高版本、package、transform 与渠道。服务端形成唯一候选路径后与 policy 求精确交集：恰好一条边匹配才可选择；零匹配返回 `source_forward_path_unavailable` 和人工入口，多匹配按 Registry 配置错误失败关闭。决定绑定 policy ID/revision/hash 与命中边摘要，validate/consume 重验；policy 或边换版按 Registry 资格变化发布。

选择输出包含 endpointKind=baseline|candidate 与 hopKind=ordinary|bridge 两个独立维度，分别绑定 endpointChain 与 hopChain 的完整实体、Package/Artifact 身份、策略、时间窗及适用资格修订；相同链可使用相同摘要。candidate 复核 endpoint 的 Rollout，bridge 复核跳的 BridgeEligibility。ordinary 实际跳不等于终点时，只能是本渠道有效基线，或当前 Target/Snapshot 明确登记且有独立前置路径审批的 active 跳；审批绑定路径及修订并纳入 PublishSelectionChange。该跳自身 Rollout paused/aborted 必须拒绝，不能借有效 endpoint 绕过；其他 active、未命中候选或仅制品批准不足以取得 ordinary 资格。所有动态端点及边界前最终复核验证两条链和适用的前置路径审批，superseded 仍只用独立 BridgeEligibility。

releaseVisibleAt/installNotBefore/installNotAfter 是版本化 Deployment 字段，时间窗、修订和 generation 进入决定与三用途授权绑定。check/download-info 校验可见且未到期，不提前要求 installNotBefore；预安装 validate/consume 只允许非活动槽写入，直接安装与激活 validate/consume 还须在 [installNotBefore,installNotAfter)。跨 executionCommitBoundary 的紧前一步再次在线复核两链当前资格和授权。endpoint.upgradeType 决定交互；返回前验证完整路径每个 hop 的模式认证，normal/forced 用 directInstall，silent 用 stagedRestart。类型或模式不兼容返回人工入口，不擅自改类型。

### 8.2 支持下界

supportFloorVersion 作用域为 `product + component + channel + targetKey`。Stable 初值等于 migrationBootstrapVersion。

Internal、Beta 和未来新渠道首次激活前必须显式审批非空初值；默认可提议继承同 targetKey 的 Stable 当前值，但后台和 Target Schema 均不得静默生成或接受空值。选择固定向量必须覆盖首次激活、继承提议、拒绝空值和各渠道隔离。

SelectionScope 状态为 unactivated→active。首个 Deployment in_review→scheduled 时，以预期 next deployment revision 做 CAS，并保存已验签、schema 完整的 `StagedActivationSet(Target, Snapshot)`；它绑定 generation=1、`baseRootPublishHead`、`baseMetadataPublishHead` 以及 Root、support floor、Deployment/Release/Artifact/SupplyChainApproval 的完整 identity+revision+bytesHash 读集，最长有效 15 分钟。staged Snapshot 必须包含 component base head 的全部 current 引用再加入新 Target，但不设为 current、不被 Timestamp 引用且不允许客户端下载。首次激活锁定 scope、scheduled Deployment、staged revision 和两级 head，提交点重读全部绑定；只有两级 base head 均未变化才通过 PublishSelectionChange 写 baseline、scope active、current Target/Snapshot、新 Timestamp 与 generation=1。其他 scope 发布、Root/supportFloor/entity 变化、过期或任一 CAS 失败均零写入并按最新完整集合重签。完整 staged 内容在成功、替换或到期后 24 小时内从独立加密存储删除，只留审计摘要。unactivated check 固定返回 HTTP 409、`SCOPE_UNACTIVATED`、`retryable=true` 和普通检查间隔，不创建 decision、telemetrySession 或 metadataSet。

提高下界必须通过 PublishSelectionChange 生成新的 current Target/Snapshot/Timestamp 和 selectionGeneration，展示受影响安装量并验证人工升级入口；降低下界必须先增加真实来源测试和路径数据，并同样通过该命令扩大资格。该规则属于产品能力，不由数据库配置临时绕过。

### 8.3 历史桥接

BridgeEligibility 是 Deployment 的版本化附属资格，不修改不可变 Deployment 字段。

只有满足以下条件的 superseded Deployment 才进入桥接图：

- 当前 Snapshot 可达。
- 同渠道 BridgeEligibility=enabled。
- 曾达到同渠道 100% 或成为过基线。
- Release=closed、Release Target=approved、全部 Artifact=valid、渠道 SupplyChainApproval 当前有效；bridge 只把 Deployment active 条件替换为 superseded+BridgeEligibility enabled。
- 从来源到期望终点的每一跳都满足版本、平台、helper 和数据迁移约束。

禁用资格可立即生效；重新启用创建更高修订并重新审批。历史桥接仍需当前在线决定和安装授权，因此不构成旧发布重放。

BridgeEligibility 具有独立 ID 和单调 revision。任意状态变更都增加 selectionGeneration，并通过 PublishSelectionChange 发布 current 元数据；紧急禁用可先 deny/metadata_sync_pending。决定、下载资格和安装授权绑定 ID/revision，download-info、validate、consume 最终提交前重新读取并比较，禁用再启用不能形成 ABA。

## 9. 控制 API 模块

控制面固定为 https://update.pinvou.com，逻辑端点包括：

- check：新检查和决定刷新，返回 endpoint 与实际 hop、类型、资格及来源绑定。
- download-info：只为当前允许的 packageId 返回短期文件地址。
- validate：分别执行预安装、直接安装和重启激活校验并签发用途隔离授权；预安装不提前冻结/备份活动数据，安装/激活须完成适用准备及 required 备份。
- consume/cancel：按用途和全部绑定原子消费或取消；不同用途不可串用，激活比较 staged revision 并创建新 transactionId。
- consume-status：覆盖三用途及 required/notRequired，用高熵 recovery secret 和完整绑定查询最小 consumed/cancelled/expired 与 transactionId，不返回或续签凭据。
- 独立对账任务：在旧凭据或查询窗口不可用时核对旧尝试并追加受控处置，不授予下载或执行权。
- 独立质量复核任务：只给特定窗口、范围、原事务及原目标身份签发复核事件凭据，不授予升级或旧事件补签权。
- events：按用途隔离的遥测、执行事务和独立任务事件。

逐字段 Schema、错误码、幂等 Header 与路由由版本化 OpenAPI/Schema 定义；架构须保留用途、时间及权限隔离，不能因共用端点合并语义。

## 10. 幂等、并发与恢复

### 10.1 结果要求

- 同一业务幂等键和同一规范请求只能产生一个业务结果。
- 同键不同语义请求稳定返回冲突。
- 任何最终领域写入必须同时提交幂等结果，响应丢失后可恢复原业务结果，但生成新的 HTTP requestId。
- 过期执行所有者不能在新所有者接管后提交。
- 客户端不再重试时，后台恢复任务仍能使 processing 操作收敛，不永久占用 lineage。
- check/refresh、download-info、三用途 validate/consume 在最终提交前重验 channelRevision、generation、即时 deny、基线指针、两条正向链和适用 Rollout/BridgeEligibility/前置路径审批；时间按 §8 的动作资格分层，不把 check/download-info 当作安装。复核当前 Root/Timestamp/Snapshot/Target 与两链全部 Release 身份、revision、expiresAt、指针；续签/到期后旧决定要求重新 check。直接安装/激活消费后的主动渠道切换例外按 §11.3，不能扩大其他用途资格。

### 10.2 实现边界

实现可以使用租约、owner epoch、CAS 和加密恢复投影，但这些是内部机制。数据库 Schema、超时和恢复算法在单独的服务端设计记录中评审，并通过故障注入证明上述结果。

## 11. 客户端协调、事务与恢复

### 11.1 遥测与准备所有权

客户端协调器按 endpoint.upgradeType 驱动流程：normal 每个实际 hop 都须用户明确点击才下载，展示期望终点与本次 hop；成功后重新检查并再次确认。silent 自动下载/校验/预安装，完成才提示重启，等待期间继续旧版。forced 按 §11.4 管理门。提示、风险、稳定错误和修复入口统一使用简中/英文/日文资源；不可逆迁移明确说明备份仅供人工修复、不会自动回滚。

检查创建短期遥测会话并登记逻辑workflowId/首次资格阶段。下载无总时长硬上限，决定/URL/会话到期取得当前资格及新会话续接未来动作，保留同一Package下载缓存；服务器核对同scope/渠道/来源/endpoint/hop/类型/模式及原流程关系，不迁移阶段或增样本。normal同一已确认实际hop保留意愿，改选新hop重新确认。旧终态/凭据不复活、不重签旧事件；完整校验/准备/权限/备份/授权前当前会话必要前置事件须合法确认。逻辑下载不持准备锁或冻结业务，单请求超时仅重试/退避/换源/提示。安装/激活的权限确认在冻结前完成；silent 只验证已部署可信 helper 的既有权限，不弹 UAC 或管理员密码，缺权保持旧版及已有门，展示用户主动修复组件入口。

协调任何写入者、冻结或写备份前，先在安装范围取得系统级产品锁并持久化唯一 PreparationRecord(ownerEpoch,freezeEpoch,scope,requestLineage)。从准备到 consume 转交连续持有互斥；崩溃/重启先恢复记录，不以进程消失放行另一准备。旧代次不能写入、解冻或清理接管后的数据。预安装在发出 consume 前也须建立受保护请求记录及取得产品锁，但不得冻结当前业务或迁移活动数据。

per-machine 协调全部登录/断开用户、后台任务及登记写入者，安全保存、阻止新写入/旧版启动，验证冻结；备份/迁移覆盖共享和全部受影响用户数据，包括未登录用户，不能仅确认发起进程退出。任一范围不可访问或无法证明安全时禁止受保护写入，按类型安全取消或保留 forced 门；不得强杀其他用户未保存内容。per-user 同样遵守准备先于冻结。所有权只能解除自己的冻结代次，成功前其他会话遵守同一升级/修复保护。

validate 最终进入授权成功、业务失败或协调中止；协调中止为终态不能复用旧 lineage。未 consume 会话在 sessionStartedAt+24h 阶段化 CAS 收敛：authorized→authorization_expired，同时 available 授权→expired；更早阶段→expired 并 fencing active validate。timeout/validate/consume/cancel 竞争同一 revision，合法七天补报内迟到事件仅诊断；24h只限制该会话，不限制逻辑下载时长，续接会话保留原workflow首次阶段。

### 11.2 三用途执行事务与待激活记录

consume 按用途关闭对应遥测授权阶段、创建唯一事务并转交准备所有权；执行事件不能再用旧遥测凭据。遥测、事务、槽位与对账记录分别持久化，状态机在规范中统一维护：

| 用途 | 本地写入与执行边界 | 收敛及占用 |
|---|---|---|
| 直接安装 | normal/forced 调用平台安装器的瞬间即 executionCommitBoundary，不能等待首个文件写入才算越界 | 成功以活动身份、迁移一致性及健康检查为准；边界前安全取消，边界后失败保持修复门 |
| 预安装 | 仅写指定非活动槽、隔离验证可启动性，不迁移活动数据、不切指针、不跨 executionCommitBoundary | staging_completed/staging_failed/staging_cancelled；完整槽与 StagedRecord 原子登记后释放活跃事务及锁，不报应用 succeeded，不建应用修复门 |
| 激活 | 用户发起的应用重启或系统重启时完整准备；消费后最后在线复核；首次活动数据写入或指针切换二者较早者即边界 | 使用独立新事务及凭据；不可逆迁移须在消费/最终复核后、指针切换前完成；边界后失败不得启动旧版 |

StagedRecord 固定原预安装事务、scope/channelRevision、Deployment/hop、Package/槽位、原质量阶段、stagedAt、revision 和 stagedValidUntil=min(stagedAt+30d,installNotAfter)；installNotAfter=null 时固定为 stagedAt+30d。staged_waiting_restart 是内容状态，不是活跃事务；同范围最多一个有激活资格的槽。激活消费 CAS 比较 revision 仍 waiting，原子置 activating 并创建新 transactionId/范围占用；本地准备、锁和冻结连续转交，响应丢失恢复原请求，不再消费第二次。质量归属仍按升级流程首次取得资格的原阶段，创建激活事务不把同一流程搬入当前阶段。

激活前先在线确认仍精确选择该 staged hop，验证既有权限，再独占协调/冻结及完成 required 备份，之后才能请求激活授权。无法联网或前置条件失败时，只在证明无受保护写入、安全停止并合法取消后启动旧活动版本，保留已有门；消费未知按 §11.5 本地停用/对账。暂停只隐藏提示并在原期限保留槽，恢复精确匹配才提示；到期、撤销、渠道改变或改选立即失去资格并安全清理。未停用的 activating 槽不得被普通清理或新预安装替换。

已消费激活只有服务端确认为 cancelled_before_install、helper 证明未越界/无受保护写入且旧版及槽完整时，才能终结原 activating revision；原期限内原渠道及精确选择仍有效时原子创建唯一更高 waiting revision，关联原修订/预安装/取消事务，继承原 stagedAt/期限。下一次用户应用重启或系统重启重新准备、冻结、required 备份及新授权/新事务，旧授权及解冻后的旧快照不可复用。仅签发失败或消费已确认未成功、记录仍 waiting 时无需重建修订，但下次尝试仍完整准备及新授权。响应丢失幂等恢复同一新修订；消费/取消未知不得重建。其他终态不得复活旧槽，安全清理后按新决定预安装；越界失败只能前向修复。

### 11.3 资格收紧、渠道切换与报告超时

客户端最后执行检查与服务端动态资格共同执行需求 §6.2 矩阵：未消费拒绝新资格；直接安装/激活已消费未越界安全取消；已越界继续核对到成功或人工修复。进行中预安装在暂停、撤回、吊销等收紧下安全取消并清理；已完成 staged 只有同对象临时暂停可保留至原期限，其他永久失效清理。安装有效期到期适用同一边界区分，不因下载过或已消费而获得越界权。边界前取消不能仅凭超时推定安全。

决定/下载/三用途授权绑定 installId、installationScopeId 和 channelRevision。per-machine 渠道由全部用户共享，切换须管理员确认并审计；切换原子递增 revision，未消费会话 channel_changed，available 授权 cancelled。唯一继续例外是已消费直接安装或已消费激活事务：使用冻结 revision 完成既有核对/events，不得获得新资格。已消费且进行中的预安装取消并清理，staged 尚未进入已消费激活时立即失效清理，不因新渠道选相同 packageId 复用。

直接安装/激活 consume 后三十天无合法终态时，服务端 CAS 写 failed_manual_repair_required(reason=reporting_timeout)；预安装只写 staging_failed(reason=reporting_timeout)，不建立应用修复门。已提交 staging_completed 不因等待重启/槽到期改终态。未确认服务端完成的槽禁止激活：仅原预安装提交 staging_completed 且槽仍有效才可继续；已失败或不能合法确认时先解除适用对账占用再重新预安装。三十七天合法补报内迟到事件不能改终态，但可按 §16.1 更新独立质量投影。

本地已证明 succeeded 的安装不因服务端缺报重新封锁；按 §11.5 证明未越界并永久停用的健康旧版也不因 reporting_timeout 新建门，仍须完成对账才能再次升级。原失败事务不可改写，关联前向修复用独立事务。

### 11.4 强制门与关联修复门

受保护 GateRecord 绑定 scope、原 endpoint、当前 hop、原因与关联事务，跨用户/重启由 launcher 和协调器共同执行；门只开放安全保存、升级/修复、网络设置及退出。进入门先安全保存，不允许从另一个会话或旧入口绕过。在线只有 endpoint 与实际 hop 均处于安装区间、可见、支持 directInstall 且可立即取得兼容授权才入 forced 门；installNotBefore 前仅提示/预下载。

已可信验证未来 forced 决定后离线跨生效边界，能证明时间时进入联网复核受限门；未知时间不提前入门，既有门不凭系统日期或旧凭据到期解除。从未有可信 forced 决定时不凭离线推断阻断。权限拒绝、下载/校验失败且 endpoint 仍有效保持门；仅路径失效保持受限门与人工入口。中间 hop succeeded 先核对当前健康再重新检查，终点有效继续，历史 hop 越界不使下一跳永远越界。

终点可信到期或在线确认失效时：当前 hop 未创建/未越界且当前版本健康，安全取消该尝试后解除本次强制门；当前 hop 已越界未成功则先收敛，成功且终点失效可解除，失败/不明保持人工修复门。独立失败门不随强制门解除。强制正常完成须原终点事务 succeeded，仅版本相等不够。

前向修复创建独立事务，绑定同 scope、原强制终点（适用时）和被修复事务，目标在 forced 关联场景同时高于原终点及本机已安装或已受保护写入的最高目标版本，其他失败修复也须严格高于本机最高目标版本，复核来源/迁移兼容性。只有活动版本、全部受影响数据和健康验证 succeeded 才解除明确关联的门；原失败终态不变，原发布暂停/撤回/到期不解除已越界修复门，不能以其他范围或无关联成功解门。

### 11.5 消费未知、本地永久停用与独立对账

三用途及 required/notRequired 均在请求前、产品锁下建立受保护 ConsumeAttempt：256-bit recovery secret 只保存在本机，请求送哈希；固定 scope、purpose、transactionId、authorizationJti、consumeKey、规范 consumeRequestDigest、authorizationExp 及 pendingCleanupAt=authorizationExp+61d。服务端把授权 CAS、事务、会话、幂等结果和绑定全部字段的域分离 statusBindingHash/outcome tombstone 原子提交，取消/到期竞争胜方也原子记录结果。字段及哈希规范由凭据规范和固定向量冻结，不在备份策略中分叉。

普通恢复期后仅 now<pendingCleanupAt 可用 secret 和全部绑定查询最小 consumed/cancelled/expired、transactionId；错绑/不存在/过期统一无信息，禁止返回或续签凭据。+61d 关闭查询并清除本地 secret，+62d 删除服务端 tombstone，计时故障不延长服务器窗口。notRequired 不创建备份、不伪造备份事件。

从首次发送 consume 起最多一次五分钟对账等待，重试/崩溃/重启不重置，用户可停止尝试；无法证明剩余预算时立即核对本机安全事实。ACK 丢失/断网不等于未消费或已取消。只有仍有效的所有者证明未越界、无受保护写入、旧版/数据完整、全部本次执行者已停止且永久 fencing 该授权/事务/ownerEpoch，才持久化 local_execution_retired。否则直接安装/激活继续执行或人工修复保护，预安装仅隔离槽并安全收敛，不因缺报建应用门。

永久停用是独立本地处置，不是服务端终态。可解除自己仍拥有的冻结、使旧准备备份失去后续执行用途并释放实体锁，但受保护 ReconciliationReservation 跨重启保留，禁止另一准备/预安装/安装/激活；所有迟到 ACK/旧 helper/恢复 worker 不得重获执行权。normal/silent 无既有门时立即使用经验证旧版，forced/独立修复门按 §11.4；显示 consume_reconciliation_required 及适用入口，不把健康旧版伪造为失败。清除到期 secret、槽位或备份不解除对账占用及旧执行权永久 fencing，最小无秘密处置证据仍保留用于独立对账。

联网恢复原请求结果：未消费则取消/失效旧授权，已消费则确认安全取消或既有终态，完成独立处置后解除占用。旧凭据或 +61d 查询不可用时，由 MFA/最小权限管理角色签发绑定原请求/范围/授权事务/本地停用证明的独立对账任务，可信 helper 核对无写入者及旧执行权永久失效，服务端核对旧授权和占用后只追加处置。历史结果缺失保留未知，不伪造取消/成功、不复活凭据；证明不足保持保护并提供诊断/前向修复。任务不授予下载/执行/旧事件补签能力；失败/丢响应恢复同一任务，无双处置，不重置备份/槽/查询期限。再次升级完整重新准备及 required 备份。

## 12. Helper 与 Launcher

- 主应用只发起流程，不直接覆盖自身或受保护 updater。
- launcher 位于稳定、受保护的系统位置，校验准备、执行或对账的唯一所有权代次；从冻结前即恢复互斥和已有门，不能只在安装事务创建后加锁。历史终态事务及待激活槽不占执行权，但创建/激活/替换/清理槽仍在同范围互斥；停用后的期限清理由唯一对账所有者执行，不让清理旧槽误删新代次。

- helper 按版本并存，目录独立于会被主安装器替换的应用目录；主安装器不得删除本次使用中的 helper。接管绑定 transactionId（执行阶段）或准备/对账 lineage（对应阶段）、ownerEpoch 及目标 helper 精确身份，不能因无执行事务而失去准备保护。
- 新 helper 接管成功前旧 helper 保持恢复能力；成功后旧所有者写入、启动、解冻和清理请求失效，不允许两个有效所有者。
- helper 增删通过受保护事实事务，文件和事实记录可恢复地收敛；旧 helper 清理只执行短期签名授权，绑定删除前事实、精确删除集合及删除后事实。
- helper 更新不提供应用降级、恢复包或自动回滚。

## 13. 数据备份与可信期限

备份模块按 migrationMode 与 backupPolicy 两个正交策略执行；irreversible+notRequired 拒绝。required 的直接安装/激活须在既有权限、独占准备和完整冻结下创建验证加密快照，绑定本次准备/冻结代次，validate→consume→执行连续保持；silent 预安装不提前备份活动数据。notRequired 不生成快照或 backup_succeeded。scope 使用签名登记的语义 ID，不接受任意路径，全部受影响用户范围按 §11.1 处理。

首个备份字节前原子登记 LocalBackupRecord(staging路径、解密密钥引用、准备/冻结代次、sessionExpiresAt、localCleanupAt=sessionExpiresAt+24h)。部分写入、backup_failed 或崩溃留下的 staging/孤立密钥由 launcher/helper 扫描安全清理，动作竞争产品锁和 revision，不清理其他有效所有者。活动授权/事务时普通用户不可删除，终态管理员提前删除须明确不可恢复确认和审计；快照不上传、不自动还原应用或数据。

| 留存状态 | 正常可信期限能力下的清理 |
|---|---|
| backup_succeeded、尚未 consume 后安全结束会话 | 确认无受保护写入后，从合法会话终态起24小时；无终态依初始 localCleanupAt |
| consume_pending，包括本地已停用但服务端仍未知 | 绑定 §11.5 的 ConsumeAttempt，不假定未消费；authorizationExp+61d 清理快照/staging/解密密钥引用 |
| 已确认 consumed | 原子固定 transactionStartedAt 与 hardCleanupAt=transactionStartedAt+60d；永久离线仍执行 |
| 已合法确认终态 | min(hardCleanupAt,terminalAt+7d/1d/30d)，分别对应 succeeded、未写入的安装前放弃、failed_manual_repair_required |

运行中清理扫描间隔不超过十分钟，关机跨期限在下次启动且联网前执行；用户导出副本独立路径和审计，不能延长平台记录。服务端终态调整不得刷新原期限；本地停用不允许复用已解冻快照作为新准备成功证据。

平台 TrustedDeadlineProvider 必须覆盖正常离线、休眠、重启及关机时长，不依赖普通系统日期前跳/回拨，不需等联网才清理；原本不支持者拒绝认证。它输出已到/未到/不可可信判定，协调器按需求 §17.1 保留既有门、禁止未知时间的激活，不凭日期提前入门或解门。

确认组件本身损坏时持久化 retention_time_fault，保留原期限、既有加密备份/解密密钥引用及访问控制，明确三语告知可能超期和修复/管理员确认删除入口；暂停新增依赖期限能力的备份及升级动作，staged 禁止激活并隐藏提示，已越界事务安全收敛。消费查询 recovery secret 及可恢复副本立即删除，仅保留无秘密对账证据供 §11.5 独立任务；备份例外不保护该 secret 或服务器窗口。永久故障下备份可保留到管理员明确删除，不能声称硬上限仍成立。恢复可信能力后按原期限首轮、最迟十分钟清理已到期备份/staging/密钥，不重建旧 secret。故障及处置受保护审计，不上传备份内容；普通断网、日期调整、延后重启不可触发例外。

## 14. 平台适配器与本地执行预算

公共接口按语义定义，具体 OS 算法、返回类型和固定向量由各平台契约冻结：

- detectInstallation / normalizeVersion：三段产品 SemVer；Windows 文件版本映射 major.minor.patch.0，第四段不排序。
- verifyArtifact / verifyExecutableIdentity：安装器、helper、launcher 及宿主目标身份。
- acquirePrivilege / verifyExistingHelperPrivilege：normal/forced 取得必要权限；silent 只检查已有权限，主动组件修复为独立确认操作。
- acquireGlobalLock / persistPreparation / transferOwnership / recoverOwnership / fenceOwner：从首次协调前至安全收敛、跨重启的准备/执行/对账代次。
- coordinateWriters / freezeAffectedScopes / verifyDataConsistency：覆盖全部受影响共享/用户范围，不以产品锁代替写入者确认。
- runDirectInstaller / inspectInstalledVersion：直接调用瞬间报告 executionCommitBoundary；不存在绕过可信 helper 的安装路径，不能按首次文件变更延后边界。
- preinstallInactiveSlot / verifyStagedSlot / activateStagedSlot / cleanInactiveSlot：认证隔离预安装与 stagedRestart，激活报告首次活动数据写入或指针切换的最早边界；清理遵守所有权及槽 revision。
- create/export/deleteBackup：完整范围验证及受保护留存记录。
- TrustedDeadlineProvider / persistStageBudget：跨正常关机和日期变更的期限/已耗时预算，区分组件故障。
- launchAndHealthCheck：返回活动身份、数据一致性与本地就绪证据，不以进程存在或非活动槽可启动代替。

每个 targetKey 认证非零有限的协调/冻结/备份准备、预安装、直接安装及激活预算；准备不跨当前有效会话期限，完整校验≤30min、各执行阶段≤2h；下载无累计时限，单请求超时/重试与短期凭据续接按§11.1/§15。有硬预算阶段首次开始起计，换源、重试、用户切换、崩溃/重启不刷新校验/执行预算；等待用户权限确认在冻结前明确显示。预算耗尽或无法证明剩余预算立即显示稳定超时/修复入口。边界前仅安全停止且证明旧版完整后才取消/解冻；预安装隔离槽而不建应用门；越界安装/激活进入不可改写 failed_manual_repair_required。仍在运行的系统安装器/迁移写入者由 helper 保持安全接管和冻结，不强杀系统包管理器、不并发前向修复，直到停止并完成安全核对；晚到成功退出码不能改写失败或自动解门。服务端缺报仍是 outcome_unknown，不伪造本地执行超时。

健康检查要求新活动身份/入口正确、必需迁移及全部受影响数据一致、主界面或本地就绪入口响应、配置/存储能安全打开、本地组件初始化、无启动循环/致命错误；不发付费外部请求、上传或修改业务内容。单次≤120s，最多重启新版本重试一次，含启动/等待总计≤5min；明确数据/身份错误直接失败，崩溃重启不重置预算，不能证明成功时保持人工修复门。模型/业务服务/登录或外网不可用不单独构成本地健康失败；已本地 succeeded 可使用，不因上报超时重新封锁。

前端只消费语义能力，不读 user-agent/Tauri 全局；Rust 通过 OS 接口和 cfg 适配，不支持明确 unsupported。Windows 10/11 x64、Ubuntu 22.04/24.04 x86_64/arm64、macOS Intel/Apple Silicon 按已认证 OS 列表分别验证；Universal 不代替两宿主测试。正式 targetKey 必须分别通过 directInstall 和 stagedRestart，按 hostArch×OS×upgradeType 覆盖主路径、权限缺失、失败/暂停/撤回、跨重启、可信期限及人工修复，不能只测一个系统/架构。

## 15. 文件服务

download-info返回HTTPS URL、packageId、size、sha256、有效期和对象一致性条件。URL15min是访问资格窗口，不是下载时长预算；重新取得当前资格及地址后可断点续接精确相同内容，Range/ETag不替代最终签名大小/哈希验证。

- URL 可以属于第三方服务，客户端不从控制面域名推导文件地址。
- 文件服务切换只能改变 URL，不能改变制品身份。
- 断点续传要求字节范围与同一不可变对象绑定；无法证明同一对象时从头下载。
- 下载请求不携带 SN、installId、installationScopeId 或升级 API 凭据。
- 文件服务不得访问管理后台或授权数据库。

## 16. 事件、质量与网络隐私

事件服务分别按遥测、三用途事务及独立任务 lineage 校验凭据、sequence 和合法状态边，在线复核 keyId/purpose 与即时 deny revision。events 去重账本保留 eventId/摘要/提交结果至少至凭据 exp 后两分钟，不受普通 API 24h 恢复期限制。正常轮换保留旧合法窗口；紧急 deny 外部统一 EVENT_KEY_DENIED，提交过的事件也不重放 ACK，事实只留内部审计。事件不含明文 SN、业务内容、完整文件路径、下载 URL 或秘密；三用途事务报告超时按 §11.3，不以服务器缺报倒推本机失败。

### 16.1 阶段质量投影与恢复复核

StageObservationRecord 固定已审批 stagePlan revision、实际阶段及服务端生效提交时刻。首阶段从计划第一百分比开始，正常窗口为 [阶段生效提交时刻,本次评估可信时间]；暂停资格恢复、解冻复核及解冻后重新观察各有独立窗口 ID/起点，不计暂停经过时长，不跨窗口复用一次健康检查。每阶段非零最小样本/最短时长、失败率阈值 (0,100%] 在审核时冻结；新阶段不复制旧阶段成功。

QualityEvaluation 保存证据快照、各组计算结果、QualityProjection revision/事件及对账处理水位、冻结原因集合及恢复窗口 revision。新合法执行/复核事件、对账或已开始流程登记使待评估读集失效；尚未投影的已登记实际执行事实不得当作不存在，应作为相应未完成/待确认指标阻止通过。仅取得资格而未开始某执行阶段，不计该指标的执行样本，不把普通检查、未点击下载或延期重启伪装成执行失败。扩量、解冻、completed 在同一最终领域事务比较 scope/Rollout/计划修订与这些质量读集，确认无新冻结或新未知阻碍，失败零写入并重新评估；不能以异步缓存或评估时旧结果提交。完整事件日志与投影重建保持相同去重/阶段归属，灾备不回退已确认失败或冻结锁存。

QualityProjection 与不可改写事务终态分开。资格记录绑定 Deployment/Rollout/首次取得资格阶段/targetKey/来源版本/实际 hop/upgradeType；同 scope 在同窗口同分组最多一个样本，换源/重试/重复事件不增加样本，重新发起流程不能覆盖已记失败。合法证据计确认成功/执行失败，未完成及 outcome_unknown 分开；下载持续有可信进度或可确认网络等待/暂停为未完成，无累计时限，不因24h自动未知/冻结；按已审批阶段的有限进度上报/失联间隔无有效状态才未知，不终止续传。完整校验30min、预安装/安装/激活各2h、健康总5min仍未收结果为未知，不伪造执行失败。合法晚到证据可以追加更新独立质量投影，不改旧终态、历史评估/快照、授权或本机门。

每项真实失败率=确认失败/(确认成功+确认失败)，零分母样本不足。令 U 为未完成和未知的去重样本，保守上界=(失败+U)/(成功+失败+U)。逐有样本来源/hop 分组的每项必需指标均须确认分母达到阶段最小样本、真实率和上界严格低于阈值，完整时长结束，才允许进入紧邻阶段；无样本路径靠发布前认证，不能伪装为质量通过。silent 下载/校验/预安装/激活/健康分别判断，staged 不等于激活成功。用户取消、延期、资格安全取消不进执行失败分母。可信下载未完成及其他阶段预算内未完成阻止通过，不单独冻结；失联下载未知、其他预算后的未知及真实失败达到阈值锁存冻结，指标改善不自动解锁。100% 自身仍须完整观察才 completed，无人工阶段推进/降百分比/运行中改计划或 SN 组。

FreezeReason 固定原阶段、原分组、指标、阈值、评估时刻及证据；历史阶段迟到失败也冻结当前 Rollout，不能改记当前阶段。恢复投放资格按 §4，不要求暂停期间取得新样本；解扩量冻结由受控角色在资格有效时发起 RecoveryReview，覆盖当前阶段及全部未解除原因的原分组，每组沿用其原阶段阈值/最小样本，独立窗口长度至少各适用阶段最短时长最大值，分母不跨组抵消。复核中及恢复提交点新增原因必须补入并满足完整条件。

ReviewTask 精确绑定窗口、scope、原事务/阶段/hop、原目标版本/targetKey/签名 Package及制品哈希/激活模式，只签本次复核事件，不补旧凭据。样本集合覆盖被复核阶段/分组全部已开始流程，包括失败、未知、已转更高版本的范围，不能挑选成功设备。合法原下载/校验证据只贡献对应指标；仅原安装已合法确认成功、当前仍精确同一目标内容、窗口内活动数据一致及新健康复核通过，才贡献安装/健康成功，历史成功无本窗口健康也不够。新流程仍归首次资格阶段，不能搬旧成功；每个 scope 每窗口同原阶段/分组最多一次，跨阶段须独立合法 lineage。

原确认失败持续计失败、未知持续计上界；更高关联前向修复只证明设备恢复及解除关联本机门，不能增加原候选成功分母或以新健康推定原执行成功。需改代码/制品时终止原投放发布更高版本；原不可变候选仅能凭自身合法结果与全部质量条件恢复。各组完整通过后操作员带原因/审批/审计解除冻结，百分比不变，再从提交时刻完成新的独立观察，覆盖当前阶段及此次关联全部历史问题组；原因标记解除也不移除该窗口原组。全部通过才自动进邻阶段，再次触阈值追加原因并冻结。已有合法成功设备可以在窗口内复核，不要求重装同版本。该服务内部计算与受控诊断遵守反滥用贡献限制，不能与网络隐私面联查或共享关联键。

### 16.2 普通运营发布质量周报

ReleaseQualityWeeklySnapshot 独立于实时阶段投影和网络隐私立方体。固定不重叠 UTC 自然周，完整叶子为 (platform family,targetKey,channel,sourceVersion,targetVersion,upgradeType,fileSourceClass)。每 scope 每周仅首个合法流程归一个叶子；源分类预先批准，发生源切换统一 multi_source，发布前确定后不变，去重/反滥用校验合格才计入。周结束七天后发布唯一冻结快照，k≥20 才可见，否则只样本不足，不给人数/近似/上下界。界面/导出只同一快照可见完整叶子，不给总计、边际、部分维度、任意/相邻窗、临时过滤、合并、下钻或关联键；迟到对账仅内部诊断，不改公开快照。自动阶段计算和逐实例诊断只内部自动化及受控角色，最小权限/审计，普通运营只状态、冻结恢复原因及周报，不能形成第二个可绕过抑制的查询入口。

### 16.3 网络来源隐私立方体

边缘网关仅为滥用防护/贡献限制派生 ASN 和截断前缀，不用于资格或身份：原始 IP 不进普通事件/业务库，边缘安全日志最长七天，用途隔离摘要最长三十天。prefix HMAC key 由 KMS 按 ISO UTC week 派生，同周稳定、跨周不可关联，不落业务库。只有合法 lineage，每 scope 每 UTC周+完整单元最多一贡献；同周+单元+prefixHmac+UTC日按事件时间/eventId规范顺序仅选一个不同 scope，预养 ID 无法绕过，单前缀每周最多七个。

NetworkPrivacyWeeklyCube 完整叶子为 (platform family,targetKey,major.minor version,channel,ASN class)，非重叠 UTC 周，贡献限制后 k≥20 才显示，其余 suppressed；不发布合计/边际/部分维度/任意或相邻窗/逐前缀/过滤/下钻，冻结后不因迟到重写。与发布质量周报不联查、不共享导出关联键、不相互补算；实时诊断亦不能旁路此边界。公开遥测只能自动冻结及告警，永久暂停/撤回/吊销/隔离由有权人员结合服务器授权记录复核。

## 17. 存量迁移

旧系统只负责把存量客户端升级到 migrationBootstrapVersion。引导安装器在首次新协议检查前原子初始化：

- installationScopeId。
- stable 渠道及 channelRevision=1。
- 初始信任根和防回滚高水位。
- 当前产品、launcher、helper 和安装范围事实。
- upgradeProtocol=new-v1 标记。

初始化未提交或自检失败时继续由旧系统/人工入口修复；新平台不提供旧协议兼容端点。

## 18. 运维与灾备

- 发布元数据版本、selectionGeneration、即时 deny 和当前指针采用 RPO=0 高水位存储。
- Artifact 使用内容寻址不可变存储并跨故障域复制。
- 控制面恢复不能回退元数据版本、重复签发授权或恢复已撤回资格。
- 文件源故障允许切换备用供应商，不改变 packageId/size/hash。
- 审计覆盖审批、暂停、撤回、吊销、隔离、桥接资格、密钥操作和支持导出。
- processing 操作、元数据到期、文件源异常、安装失败和事件缺失均有可行动告警。

## 19. 实施阶段

### 19.1 一期 A：后台与协议骨架

- 落盘并冻结第 2.2 节中一期使用的规范制品。
- 完成完整包上传、供应链闸门、六类生命周期实体模型（Artifact、Release、Release Target、Deployment、Rollout、SupplyChainApproval）、support floor、minimum source、桥接、渠道和 SN 灰度。
- 完成 Stable 正式版到 Beta/Internal 的显式同步编排、原子草稿创建、幂等恢复、逐渠道审批与状态展示；检查更新继续严格按本渠道选择。
- 完成三用途控制合约、独立对账/质量复核任务、阶段质量投影及两类隔离周报，验证灰度恢复及最终 100% 观察闸门。
- 增量仅完成 Schema/字段和非空夹具“客户端忽略并回退完整包”合约测试。

### 19.2 一期 B：三平台可信完整升级

- 完成三种类型协调器、准备/对账所有权、三用途授权、独立预安装及重启激活、staged 修订、强制/修复门、多用户冻结、可信期限、完整包/备份/健康及跨崩溃恢复。
- 覆盖全部启用 targetKey、hostArch 和 OS 组合。
- 完成旧客户端迁移、第三方文件源切换和更高版本前向修复演练。

### 19.3 二期：增量启用候选

- 冻结增量算法、资源预算、基础缓存和参考重建规范。
- 完成真实制品重建和平台真实性验证后，才允许生产 Release Target 使用非空增量候选。

## 20. 需求—设计追踪

以下验证入口需在下层规范和实现阶段实际交付，不能以设计描述替代通过记录。

| 产品/安全要求 | 技术模块 | 验证入口 |
|---|---|---|
| 固定控制面、第三方文件地址 | §3、§9、§15 | OpenAPI 合约与文件源故障测试 |
| SN 仅灰度 | §8、§16 | 选择向量与授权负向测试 |
| minimumSourceVersion / support floor / 桥接 | §8 | selection 固定向量 |
| 候选严格高于基线、基线恢复（需求 §6.2） | §4、§8 | 普通/重建/同步/恢复/扩量/完成逐提交点 CAS；基线 paused/withdrawn 不降低比较；completed 基线恢复不重启灰度 |
| 暂停/吊销后的 download-info、安装有效期 | §9、§10、§15 | API 合约、时间边界与撤回竞态测试 |
| channelRevision ABA、同版本多来源画像 | §10、§11、来源事实规范 | 凭据错绑与画像匹配向量 |
| 成功备份7天目标与60天硬上限（需求 §11.5） | §13 | min截止边界、永久离线清理与提前删除审计；计时故障仅适用明示例外 |
| endpoint/hop 双实体链、元数据到期/续签 | §8、§10、可信元数据规范 | 四组合双链失效与 TTL 边界向量 |
| 渠道切换与用途终态（需求 §5.2、§11.7） | §11.2—11.3 | 三用途×切换/consume/30d 竞态；预安装仅 staging_failed；staging_completed 不等待重启；本地成功不重新封锁 |
| 发布对象正向状态守卫 | §4、状态机规范 | 每个非正向父/子状态拒绝测试 |
| 首次基线激活、Release/Rollout 失败终态 | §4、§8、状态机规范 | 两级 head 比较/CAS、跨 scope 并发、staged 15m/清理 24h、零 approved 但仍在评审、全部失败终态、CancelRelease 与 draft abort 模型测试 |
| 渠道 SupplyChainApproval 与限时例外 | §4、§8、服务端一致性规范 | 跨渠道复用拒绝、到期/扩量竞态 |
| 正式版显式同步投放 | §4、§8、服务端一致性规范；需求 §8.3.1 | 仅 Stable 无继承、目标草稿全成全败、重复/并发/响应丢失、逐渠道审批及独立暂停撤回、共享吊销、相同/更高版本无更新 |
| Manifest 外部摘要与候选防泄露 | §5、§6、可信元数据规范 | 自引用 Schema、未命中可见性、错误 opening 拒绝测试 |
| 消费未知恢复（需求 §11.6.1） | §9、§11.5、§13 | 三用途×required/notRequired、ACK丢失、5min 不重置、旧执行者永久失效、对账占用、窗口关闭/MFA独立任务、61d/62d边界及缺历史结果不伪造 |
| 事件密钥紧急 deny | §16、服务端一致性规范 | 正常轮换、长窗口去重及提交后 ACK 丢失再 deny 的外部不可区分向量 |
| 网络簇聚合隐私 | §16、安全要求 | k=19/20/21、同前缀预养 20 scope、跨日/跨周与周 key、所有非叶子查询固定拒绝测试 |
| 所有选择资格变化 | §4、§8、服务端一致性规范 | PublishSelectionChange、Target generation 相等与 Registry 多渠道全成全败 |
| 不改变资格的元数据刷新 | §4、§6、可信元数据规范 | PublishRootChange/PublishMetadataRefresh、双 component genesis/active、同版本 Root 分叉、角色 key 退出顺序与选择发布并发 |
| 无回滚、无恢复包、前向修复 | §11.2—11.4、§12—§13 | 状态机、关联更高修复及助手接管/清理故障演练 |
| Package Manifest 不过期 | §6 | 元数据与发布状态测试 |
| 一期完整包、增量预留 | §7、§19 | 空数组生产闸门和非空夹具合约测试 |
| 事件凭据与合法迁移 | §11、§16 | 状态机合约测试 |
| 三平台及未来扩展（需求 §13、§19.4） | §5、§14 | 每 targetKey×hostArch×已认证OS×upgradeType，两模式、权限缺失、暂停/撤回、失败/重启及人工修复 |
| 存量迁移 | §17 | 引导版本断电与重复初始化测试 |
| 隐私与网络簇 | §16 | 日志扫描、留存边界和权限测试 |
| 三种类型及静默两阶段（需求 §5.5、§11.6） | §4、§8—§12、§14 | normal逐hop点击；三用途错绑；非活动槽不写活动数据；staged终态释放占用；激活新事务/修订；暂停/到期/撤销/改选；不可逆边界前后失败 |
| 时间分层与执行边界（需求 §6.2、§8.2、§9） | §8、§11.2—11.4、§14 | releaseVisibleAt/NotBefore/NotAfter前后1ms，展示/下载/预安装/安装/激活矩阵；三类资格收紧×未消费/消费未越界/越界 |
| 独占准备与多用户（需求 §11.4.1） | §11.1、§12—§14 | 冻结/备份前双launcher/双用户竞争、断开/未登录用户、缺一数据范围、各转交点崩溃、旧代次迟到解冻/清理拒绝 |
| 强制及关联修复门（需求 §5.5） | §4、§8、§11.4 | 全来源路径/模式/覆盖时间窗，离线跨边界，多hop成功后终点失效，独立门不误解，仅关联更高且健康修复解门 |
| ordinary前置路径审批（需求 §9） | §8、§10 | active但自身Rollout paused/aborted，未命中/仅制品批准，审批修订ABA与双链最终复核，合法低于基线中间跳 |
| 阶段扩量与恢复（需求 §8.2.1） | §4、§16.1 | 零/不足样本暂停恢复、无人工推进、100%独立观察、未知上界、silent激活不足、历史迟到冻结、全部原组窗口、原目标身份及更高修复不稀释失败；评估后新事件/未知/冻结与扩量/解冻/completed最终CAS竞态 |
| 下载续接、预算与本地健康（需求 §11.3、§11.4.2、§11.8） | §11、§14—§16 | 下载跨24h/会话/URL/key轮换无总硬限、同workflow阶段/样本不变；30min/2h、准备当前会话期限、120s/5min不重置；活跃安装器不强杀/并发修复，晚到退出码不改失败，外部离线不算本地失败 |
| 可信期限与故障例外（需求 §17.1） | §11.4—11.5、§13—§14 | 日期前跳/回拨、离线跨关机、既有门/槽矩阵、retention_time_fault、secret立即清理、备份保护及告知、恢复原期限≤10min清理、服务器窗口不延长 |
| 发布质量周报（需求 §8.4） | §16.2—16.3 | 周末+7d唯一快照，首流程唯一归类/multi_source，k19/20，21总数与20可见组不能相减，UI/导出/诊断/网络面不可联查补算 |
