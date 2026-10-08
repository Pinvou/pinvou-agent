# 来源事实注册表规范

> 状态：V1.4，关键不变量草案，待合约冻结及实现验收
>
> 适用协议：`protocolVersion=1`
>
> 对齐基准：需求、技术设计与安全要求 V1.0。本文不是 OpenAPI、Schema、固定向量及真实平台验收已完成的声明。历史综合设计不构成规范输入。

依据：[产品需求](../../../docs/pinvou-upgrade-platform-requirements.zh-CN.md)、[技术设计](../../../docs/pinvou-upgrade-platform-technical-design.zh-CN.md)、[协议与安全要求](../../../docs/pinvou-upgrade-protocol-security.zh-CN.md)。发生冲突必须共同修订，不能以本草案覆盖主文档。

## 1. 定位

Source Facts Registry 是服务端内部、强一致的已安装来源事实账本，用于判断当前来源能否执行完整包、helper 迁移、launcher 迁移和旧 helper 清理。它不是 Root/Snapshot/Target 元数据的一部分，不直接下发给客户端，也不能替代客户端实时采集的本机事实。

## 2. 作用域与画像

Registry 快照按 `product + component + targetKey` 维护一个单调递增的 `registryVersion`。同一 `canonicalAppVersion` 可以同时存在多个合法来源画像；每条画像记录按 `product + component + targetKey + canonicalAppVersion + sourceProfileId` 唯一定位，包含：

- sourceProfileId是稳定已安装事实投影的JCS/SHA-256，投影含product/component/targetKey/canonicalAppVersion及当前应用/helper/launcher/安装格式/数据迁移事实身份；排除sourceProfileId自身、Registry版本、权限状态、policy/transform/审批引用及每次运行ownerEpoch/freezeEpoch/transactionId，避免自引用及policy边循环。完整Registry快照另做不可变摘要，资格状态/计划引用仍独立绑定及逐项复核；固定投影字段由Schema/向量冻结；
- `activeHelperRef` 与当前 helper 精确身份；
- 当前launcher精确身份及已认证互斥/所有权协议能力；每次实际执行epoch由受保护本机记录及请求事实另行绑定，不登记成全体安装共享静态画像；
- 已验证的安装格式、installScope 和迁移事实；
- 状态：`selectable`、`forward-only`、`repair-only` 或 `revoked`；
- `allowedForwardPathPolicyRef`：仅 `forward-only` 必填，绑定不可变 policyId/revision/sha256；其他状态必须为空；
- 可执行的确定性 transform 集及其前置/后置事实；
- 证据、审批修订和创建时间。

注册表写入采用 RPO=0 的强一致存储。`registryVersion` 不回退、不复用；任何记录内容变化创建新Registry快照；稳定已安装事实变化创建新sourceProfileId，只有资格/policy/transform/审批变化时保留事实ID但新快照/引用修订，不覆盖旧快照或同版本其他画像。每个快照列出全部有效画像及逐画像状态，因此 helper/launcher 迁移期间可以并存旧画像和新画像。

## 3. 状态语义

- `selectable`：允许作为正常更新来源。
- `forward-only`：只允许命中 `allowedForwardPathPolicyRef` 中精确批准的更高版本前向边；未命中时不得退化为 selectable。
- `repair-only`：不能参与普通选择，只能返回人工修复入口或专用修复流程。
- `revoked`：任何新决定、下载或安装授权均被拒绝。

风险偏序固定为 `selectable < forward-only < repair-only < revoked`。向右收紧必须创建新 Registry 快照并可先通过即时 deny 生效；任何向左放宽，包括 `forward-only → selectable`、`repair-only → forward-only|selectable` 和 `revoked` 的恢复，均必须创建更高 registryVersion、重新审批并通过 PublishSelectionChange 增加所有引用渠道的 `selectionGeneration`、发布 current Target/Snapshot/Timestamp。扩大 transform 能力同样视为放宽。状态不得在原快照中原地修改。

ForwardPathPolicy 是 Registry 快照内的版本化不可变对象。每条允许边精确包含fromSourceProfileId、目标Release Target ID/revision、toCanonicalAppVersion、Package/精确制品身份、transformId/revision及allowedChannels；目标版本必须严格高于来源版本，所有引用必须在该快照生成时存在并通过审批。check 先按当前 Target/Deployment 形成唯一候选路径，再与 policy 的完整边求精确交集：恰好一条匹配才可继续；零匹配固定返回 `source_forward_path_unavailable` 与人工入口，多匹配视为 Registry 配置错误并失败关闭。policy 引用、边或其任一目标 revision 变化都创建新 Registry 快照并按资格变化发布，不允许运行时临时追加。

## 4. Transform

首期 transform 类型固定为：

- `build-native-full-install`；
- `helper-migration-plan`；
- `launcher-boundary-migration`；
- `obsolete-helper-cleanup`。

每个transform声明精确来源/目标事实、targetKey、认证activationMode、所需helper/launcher、用途及migrationMode/backupPolicy和失败边界处置。不能用独立备份布尔量覆盖Release Target枚举；需要活动备份而Release Target是notRequired时配置拒绝并重新审批required策略，preinstall不得迁移/备份活动数据。选择结果只能引用当前画像内已审批的 transform；不得由客户端或数据库临时配置组合未知迁移。

旧 helper 清理必须使用独立清理授权，精确绑定删除前事实、允许删除的路径/身份集合、删除后事实、Registry 版本与 selectionGeneration；授权最长2min，行动前重新核对实际scope/所有权和准备/执行/对账/槽引用，不删除仍需helper；不能用旧清理授权越过新接管或扩大为任意删除。

## 5. 决定与授权联动

服务端用同一版本的稳定已安装事实投影在当前Registry精确匹配，并对完整请求（含实际scope/动态epoch等）另算clientFactsDigest。稳定投影与动态运行事实分开，普通自报不能替代helper在执行前对实际scope、文件/权限/数据事实验证；SN/installId不提供设备认证。恰好一个画像的全部必需事实相等时返回该 `sourceProfileId`；零匹配返回 `source_profile_unknown`，两个及以上匹配返回 `source_profile_ambiguous`，两者都失败关闭并提供固定人工入口，不按数组顺序选择或猜测画像。

决定凭据必须绑定 `registryVersion + sourceProfileId + clientFactsDigest`；forward-only 还必须绑定 policyId/revision/sha256 与命中的唯一边摘要。download-info与三用途validate/consume最终重读当前Registry及双链/ordinary路径审批，边界前helper/在线服务再次复核；Registry/画像状态/policy/边变更不能使用旧决定越界，新check可按当前精确同一下载对象续接而不延旧资格。

任何 Registry 修改必须使用服务端一致性规范的 PublishSelectionChange：绑定产品 RootPublishHead 与 product/component MetadataPublishHead，在一个原子提交中比较前者、CAS 后者，并发布新 Registry 快照、该 `product + component + targetKey` 下所有引用渠道的新 `selectionGeneration`、受影响全部 Target、完整 Snapshot 和 successor Timestamp；Stable、Beta、Internal 全成或全败。扩大资格在该提交前不得可见；紧急收紧可先在同一事务写即时 deny、generation、metadata_sync_pending 和持久化 MetadataReconciliationJob，恢复器按一致性规范追平，在签名元数据发布成功前所有相关动态端点失败关闭。check/download-info/三用途validate/consume均验证 `Target.selectionGeneration == SelectionScope.selectionGeneration` 及 Registry version/profile 当前资格。

## 6. 验收

固定测试至少覆盖：同一应用版本两个合法且不同的画像、未知画像、因配置错误导致多个画像匹配、helper 身份漂移、launcher 世代漂移、状态收紧、状态恢复、forward-only 精确单边命中/零命中/多命中/目标不高于来源/渠道错绑/policy 换版、Registry 在 check/validate/consume 之间变化、清理授权越界、新旧 Registry 并发、Stable/Beta/Internal 原子扇出、两级 head 冲突及 metadata_sync_pending。服务端不得静默把未知来源当作 `0.0.0` 或任意可升级版本。

追加验收：画像摘要排除自身/运行代次/资格policy引用，ForwardPathPolicy hash与来源ID无循环；稳定事实相同而运行epoch不同仍匹配同一画像、完整请求digest及本地所有权不同，状态/计划变化仍旧凭据失效。清理与新准备/接管/对账及槽引用竞争不得误删；三用途/activationMode/backupPolicy冲突失败关闭，长下载续接不绕过Registry当前资格。
