# 可信元数据与凭据规范

> 状态：V1.9，关键不变量草案，待合约冻结及实现验收
>
> 适用协议：`protocolVersion=1`
>
> 对齐基准：需求、技术设计与安全要求 V1.0。本文规定应满足的约束，不表示 OpenAPI、Schema、固定向量或真实平台验收已完成。历史综合设计不构成规范输入。

依据：[产品需求](../../../docs/pinvou-upgrade-platform-requirements.zh-CN.md)、[技术设计](../../../docs/pinvou-upgrade-platform-technical-design.zh-CN.md)、[协议与安全要求](../../../docs/pinvou-upgrade-protocol-security.zh-CN.md)。发生冲突必须共同修订，不能以本草案覆盖主文档。

## 1. 元数据角色

可信元数据角色固定为 Root、Timestamp、Snapshot、Target、Release 和 Package Manifest：

- Root 建立角色公钥、门限和轮换信任；Timestamp 指向当前 Snapshot；Snapshot 固定本次可见元数据集合。
- Target 绑定渠道、targetKey、support floor、selectionGeneration、公开基线 Release 和不可反查版本且可由客户端验证 opening 的 rolloutSetCommitment；候选 Release ID/版本/内容哈希和可解析地址不得公开。
- Release 绑定版本、多平台目标与发布说明，并在封装外引用 Package Manifest 的 manifestEnvelopeSha256；Package Manifest 逐项绑定上传内容中的下载子包、最终安装器、helper、SBOM 和来源证明，不绑定外层 ZIP 或自身封装哈希。
- Package Manifest 不设置到期时间；可选 manifestId 仅为逻辑标识，不参与安全身份。Root、Timestamp、Snapshot、Target 和 Release 必须按安全要求设置并校验有效期。

V1 每个 SelectionScope 最多一个 running 候选。`rolloutSetCommitment` 使用 SHA-256 域分离承诺：空集合为规范空值；非空时输入为规范化 targetKey、Deployment/Rollout ID 与 revision、Release envelope SHA-256 和随机 256-bit leafSalt。leafSalt 仅向命中者开封，因此公开承诺不能通过版本字典反查。命中决定必须绑定上述 opening 和有效期不超过决定凭据且最长 15 分钟的不透明 locator；客户端重算当前 Target 的承诺并相等后才能接受候选 Release。未命中决定和公共元数据不得泄露 opening、候选 ID、版本、内容哈希或 locator。承诺不是对象键或 URL。候选可见状态、policy 或任一引用 revision 变化时，fresh leafSalt、新 current Timestamp/Snapshot/Target 与 selectionGeneration 递增必须同事务提交；旧 opening 对新 Target 无效。

所有签名对象使用版本化JSON Schema与RFC8785 JCS规范化字节，拒绝重复键、未知关键字段、非法数值/字符及资源超限；签名输入按角色、product/component、作用域、协议版本做显式域分离，不接受文件自带key成为新根或跨角色/用途解释。算法/角色门限/keyID只来自当前可信Root，未冻结规范输入与固定向量前不得称合约完成。

`StagedActivationSet(Target, Snapshot)` 使用相同角色、Schema 和验签规则，绑定产品级 `baseRootPublishHead`、product/component `baseMetadataPublishHead`（current Timestamp/Snapshot identity、revision、bytesHash）及 Root、supportFloor 和引用实体完整读集，最长有效 15 分钟；Snapshot 必须包含 component base head 的全部 current 引用再加入新 Target。它不被 current Timestamp 引用、不可由客户端端点解析，也不能用于签发凭据。首次激活只有两级 head 比较/CAS 与完整读集重验通过，才能同时发布 successor Timestamp 并提升 staged 集合；任一变化必须基于最新完整集合重签。成功、替换或到期后 24 小时内删除完整 staged 内容，只留审计摘要。

产品只有一个 RootPublishHead；Root 建立或逐版本轮换必须通过 PublishRootChange 对该 head 做 CAS，同版本不同字节、跳版本及并发分叉均拒绝。每个 component 有独立 MetadataPublishHead，绑定 current Timestamp/Snapshot 和最近发布依据的 rootHeadRevision；全新 component 的 genesis component head 为 `published=false`、Timestamp/Snapshot absent、revision=0。所有改变可选择集合或 selectionGeneration 的正常操作必须发布一组由同一 `baseRootPublishHead + baseMetadataPublishHead` 派生的受影响 Target、完整 Snapshot 和 successor Timestamp，并在同一事务断言产品 Root head 未变化、CAS component head后一次成为 current。仅首次激活可把 genesis component head 置为 `published=true`。紧急收紧若先进入 metadata_sync_pending，则旧 Target 不得与新 generation 混用，任何动态凭据签发都失败关闭。

component head 的 rootHeadRevisionAtPublish 可以落后于 current RootPublishHead；客户端和服务端读取时必须用 current Root 验证 component 当前完整链，不得仅因历史 revision 不等而拒绝。若 current Root 已不能验证该链，则 PublishRootChange 的前置条件本应失败，动态端点也必须失败关闭。

后继Root逐版本同时满足旧Root角色阈值与新Root自身阈值；已可信缓存Root过期仅可验连续后继，只有未过期最终Root及完整当前链恢复新资格，不提供远程重置根/高水位入口。产品轮换复核所有active component当前完整链，退出角色key先双信任、逐组件刷新、再移除旧key。

Root 轮换只使用产品级 PublishRootChange。Release 续签和不改变可选择集合的 Target/Snapshot/Timestamp 续签使用 PublishMetadataRefresh：绑定两级 base head 与完整 current 集合，在同一事务比较 Root head、CAS component head并原子更新 current 指针，不递增 selectionGeneration；如同时改变可选择集合则必须使用 PublishSelectionChange。任一冲突对应的签名集合不得发布或覆盖新 head。

## 2. 决定凭据

成功 check 的决定凭据必须至少绑定：`jti`、`iat`、`nbf`、`exp`、decisionId/revision、telemetrySessionId、requestNonce、installId/installationScopeId、product/component/channel/channelRevision/targetKey、host OS/arch 与客户端事实摘要、currentVersion、selectionGeneration、metadataSet（Root/Timestamp/Snapshot/Target 及 endpointChain/hopChain 引用的全部 Release Manifest）的身份、version/revision 与 expiresAt、Registry 版本/sourceProfileId、forward-only 时的 policy ID/revision/hash 与命中边摘要、updater/helper/launcher 事实摘要及 `updateAvailable`。签发点必须满足 `Target.selectionGeneration == SelectionScope.selectionGeneration` 且不存在 metadata_sync_pending。unactivated SelectionScope 固定返回 HTTP 409、`SCOPE_UNACTIVATED`、`retryable=true` 和普通检查间隔，不创建决定或 telemetrySession，因而不适用 metadataSet Schema。

有更新时还必须绑定逻辑workflowId、首次资格阶段及Deployment的 `releaseVisibleAt/installNotBefore/installNotAfter`、Release Target、Release、Package Manifest、packageId、最终安装器、迁移/备份计划身份；无更新时必须绑定稳定 reason code。决定凭据最长 15 分钟，`now == exp` 即失效。

所有有更新决定必须绑定当前基线指针，以及两个独立字段：`endpointKind=baseline|candidate` 和 `hopKind=ordinary|bridge`。endpointChain 与 hopChain 分别绑定 Deployment ID/revision、installNotBefore/installNotAfter、Release ID/revision、Release Target ID/revision、manifestEnvelopeSha256、package/Artifact 身份、正向状态快照和渠道 SupplyChainApproval ID/revision/effectiveExpiresAt；实际相同时两条链使用相同摘要。endpointKind=candidate 时必须绑定 Rollout ID/revision/state、灰度策略修订以及 rolloutSetCommitment opening，否则这些字段为空；hopKind=bridge 时必须绑定 BridgeEligibility ID/revision，否则这些字段为空。candidate-bridge 同时包含两组资格。ordinary非终点hop另绑定当前渠道有效基线或当前Target/Snapshot独立前置路径审批ID/revision，实际hop自身Rollout paused/aborted拒绝，未命中候选/仅制品批准不能借终点绕过。决定绑定endpoint.upgradeType及每hop认证的activationMode、migrationMode、backupPolicy；后续下载资格和三用途授权完整继承双链/用途相关绑定。

## 3. 文件资格与三用途授权

- 文件资格/实际URL最长15min，继承当前决定、scope/installId、渠道修订/generation、metadataSet/Registry、双链及唯一package/精确字节身份。releaseVisibleAt后且未到installNotAfter可展示/下载，installNotBefore不挡下载或preinstall。URL与下载总时长分开，到期重新取得当前资格，不延原URL、不转发控制凭据/安装标识/环境隐式认证到文件服务，所有重定向按安全要求§7。
- 三用途授权各最长5min，封闭purpose=preinstall|install|activate和专用credentialType/audience，完整绑定product/component、scope/installId、decision/session/workflow/authorizationJti/transactionId、当前来源/平台/updater事实、两链/元数据/Registry/资格修订、渠道/generation、Package/最终字节、模式、目标helper及本次准备所有权代次。
- install/activate在安装区间，权限/全范围冻结及required备份已验证，授权绑定本次freezeEpoch/backup摘要。preinstall只允许既有helper权限下指定非活动槽，无活动数据备份摘要；irreversible不可用notRequired。activate另绑定原预安装transactionId、waiting stagedRevision、slotIdentity/stagedAt/原期限；消费CAS比较waiting修订并创建新事务。
- helper清理许可最长2min，绑定实际前后事实、Registry/generation、精确删除身份集合；行动前重新核对准备/执行/对账/槽引用和所有权，不能删后来接管所需helper。
- validate/consume最终在线复核双链和用途分层时间资格；install/activate跨executionCommitBoundary紧前一步再次在线核对当前两链、期限及本次合法消费状态。签发/消费/复核失败不能借签名合法或幂等重放增加执行权；越界以后按安全矩阵核对收敛，不因5min到期强杀或回滚。

### 3.1 下载续接

下载不设置总时长、累计重试时长或因运行超过24h而自动失败的硬上限。单次连接/无响应检测、可重试网络超时、资源不足等待和有上限退避可按平台能力配置，只决定本次请求重试/换源/提示，不终结整个下载。正常下载不取得数据冻结或升级准备锁；用户可取消，forced已有门不被重试绕过，权限/发布资格失效按需求安全处理。

URL/决定/遥测会话仍有原短期窗口，不能续签旧凭据或接收过期事件。新check/refresh在当前资格有效时建立新的有期限会话/凭据，续接同一逻辑workflow的未来下载进度和结果：绑定原workflowId及原首次资格阶段/分组、前后会话、scope/channelRevision、来源、endpoint/hop、Package/字节身份和upgradeType/mode，服务端核对原workflow仍未终结（会话expired不等于workflow终结）、唯一当前会话关联及当前事实后才关联；同workflow同时最多一个可写下载续接会话，新会话以原修订CAS转交未来动作，旧会话只合法补报/诊断，丢ACK恢复原新会话，不能双写。原workflow已确认失败/取消/资格终结不得续接覆盖旧结果，只能独立新流程并保留原失败样本。current元数据/灰度阶段或candidate→baseline变化使用新决定的当前正向链与endpointKind，不将历史资格当执行权；只在原实体/字节/类型/模式仍精确匹配时关联原质量阶段。旧会话终态保留、新会话不重签过去发生事件，不移动阶段或增加样本；normal精确同一已确认实际hop可继续原用户意愿，改选/类型或目标改变须新流程及适用确认，缓存只有精确可信字节可安全复用。

部分缓存仅用于断点续传，不视为可信完整制品；本地分段hash/Range/ETag只校对缓存/对象一致性，所有字节完成后仍须签名声明的完整大小/SHA-256及最终安装器/平台验证。完整缓存只有这些验证成功且当前对象未被替换才可复用校验结果，不因续接跳过安全验证。

新会话使用明确download_resume_context进入download_started，记录原阶段首次开始及下载缓存事实；校验/权限/冻结/备份/授权前重新完成对应当前会话的合法前置事件，旧冻结或备份不沿用。换源/续接/重复事件仍一个样本，旧会话过期不伪造下载失败；质量状态按需求§8.2.1：有可信进度/可确认网络等待或暂停为未完成，按审批的有限上报/失联间隔无有效状态才未知；24h会话到期不自动冻结/失败，不终止客户端下载。key deny不能补签旧事件，新的合法会话仅记录新动作，不把缺失的旧证据推定成功。无法合法验证旧workflow关联时原结果保持未知，建立独立新流程可使用重新校验的相同缓存，不因关联丢失强制重下或改写旧失败。

### 3.2 消费查询绑定

全部purpose及required/notRequired请求前均生成并保护256-bit随机recovery secret，consume只发送recoveryHandleHash=SHA-256(secret)。consumeRequestDigest对版本化语义请求做JCS/SHA-256，排除其自身digest字段、secret、传输requestId及易变Header，包含purpose/authorizationJti/consumeKey/transactionId、全部身份/事实及recoveryHandleHash，禁止循环摘要。域分离前缀为ASCII `pinvou-consume-status-v1` 后接单个NUL字节0x00（不是反斜杠文本），再接JCS对象{recoveryHandleHash,product,component,installationScopeId,purpose,authorizationJti,consumeKey,transactionId,consumeRequestDigest}，整体SHA-256得statusBindingHash；精确编码由固定向量冻结。

服务端与消费/取消/到期结果原子保存唯一最小outcome及绑定摘要，secret不在服务端；本地ConsumeAttempt独立于备份存在。24h完整恢复不重签，之后now<authorizationExp+61d仅secret及全部绑定正确可查consumed/cancelled/expired与transactionId；常量时间比较，错绑/不存在/过期统一无信息限流，不返回凭据。+61d关闭并删本地secret及副本，+62d删tombstone；只有无秘密最小lineage/摘要进受保护审计，秘密/完整可重放组合不进普通日志/URL。

五分钟未知消费、本地永久fencing及+61d后MFA独立对账任务按状态机/一致性规范。任务不恢复旧执行权，历史缺失保留未知；计时组件故障立即删secret且不延服务器窗口，备份解密密钥与查询secret独立。

## 4. 遥测和事务凭据

- 遥测会话凭据必须绑定 `credentialType=telemetry-event`、`aud=update-events`、signingKeyId、credentialPurpose、denyRevisionAtIssue、installId、installationScopeId、decisionId/revision、telemetrySessionId、channelRevision、selectionGeneration、允许的 `telemetry-v1` 状态/事件集合、`iat/nbf/exp` 和 eventNotAfter。事件业务发生时间不得晚于会话签发后 24 小时；上传凭据最长 7 天。
- 三用途事务credentialType分别为preinstall-transaction-event/install-transaction-event/activate-transaction-event，purpose分别preinstall/install/activate，必须绑定`aud=update-events`、signingKeyId、credentialPurpose、denyRevisionAtIssue、installId、installationScopeId、来源 decisionId/revision、authorizationJti、transactionId、channelRevision、允许的 `transaction-v1` 状态/事件集合、`iat/nbf/exp` 和 eventNotAfter。事件业务发生时间不得晚于授权消费后 30 天；上传凭据最长 37 天。
- 服务端同时校验事件业务时间、接收时间、凭据有效期、序号和状态机边；较长的上传窗口不延长下载或安装资格。
- 独立对账/质量复核任务使用专用type/audience/purpose、taskId/revision/window、scope及原事务/精确目标Package/模式绑定；动作许可≤5min，任务事件发生≤24h且在观察窗口、上传≤7d。刷新动作许可只用于当前任务未来动作，不延旧执行证据。所有凭据跨类型/用途/范围/session/transaction/task或阶段拒绝，事件不调用升级动作接口。
- 渠道变化未消费会话/可用授权失效；唯一继续例外为已消费install/activate按冻结revision核对既有事务/events，不取得新权利。preinstall进行中取消并清理；尚未消费激活的staged失效，新渠道同package不能复用资格。
- 三用途30d无合法终态按用途收敛：install/activate失败人工修复，preinstall staging_failed且不建应用门；staging_completed不等待重启。30d内发生/37d内上传的合法晚报仅诊断，可更新独立质量投影，不改终态/门/历史评估/公开快照。
- sessionStartedAt+24h 后，未 consume 的非终态会话按阶段收敛：authorized 会话与关联 available 授权记录分别原子进入 authorization_expired 和 expired；authorization_requested 与更早阶段的会话进入 expired，并 fencing active validate；24 小时内发生且在 7 天凭据窗口内晚到的事件只能保存为 late diagnostic，不改变终态；新会话按§3.1续接未来下载，旧会话期限不限制总下载。
- 每个事件提交在线比较 signingKeyId/purpose 与当前 deny；正常轮换未 deny 的旧 keyId 可用，紧急 deny 后立即拒绝。events 去重账本至少保留 eventId、规范摘要与结果至凭据 exp 后 2 分钟；deny 后无论同 eventId 是否已提交，对外统一返回 `EVENT_KEY_DENIED`，不暴露存在性、不重放成功结果且不产生新状态写入，提交事实只进入内部审计。

## 5. 密钥操作

角色密钥必须在受控密钥服务或硬件保护边界内生成和使用，私钥不可导出到构建产物、日志或业务数据库。新公钥先通过现有信任链发布，发给具体客户端的新凭据须已在该客户端当前可信链可验证后才用新key签发，不等待无法上线的全部历史客户端；轮换、启用、停用和紧急吊销均需双人审批与不可篡改审计。

旧公钥和验证材料必须从该密钥最后一次签发时起，保留到相关对象的有效期、事件离线上传期、幂等结果恢复期及其他规范恢复窗口全部结束后的最晚时刻，再增加至少 2 分钟时钟/处理裕量；不得只按对象在线有效期计算。Package Manifest 作为不过期身份对象，其验证链必须按长期归档策略保留，不能因在线签名密钥轮换而失去验证能力。密钥泄露时先写即时 deny，再发布新 Root/元数据并推动客户端信任迁移。

Root、Timestamp、Snapshot、Target 和 Release 的最大有效期、刷新重叠和离线行为以安全要求 9.5 为准；Schema 不得接受超过该上限的 `expiresAt`。

## 6. 元数据防回滚

客户端按角色和作用域持久化 `highestVersion + acceptedBytesHash`：Root 为产品全局；Timestamp 和 Snapshot 为产品/组件；Target 为 product/component/channel/targetKey；Release 为 releaseId，其 revision 作为该作用域内的单调版本。验证通过且准备接受元数据时，必须把元数据字节、版本高水位和哈希在同一受保护本地事务中持久化，再允许其产生决定或安装行为。

- 版本低于 high-water 一律拒绝，即使签名和有效期仍合法。
- 版本等于 high-water 时，只接受与 `acceptedBytesHash` 完全相同的字节；同版本不同内容失败关闭。
- 版本高于 high-water 时先验证完整引用链、有效期和签名，再原子提升高水位；持久化失败不得使用新元数据。
- 已缓存可信Root到期仅允许连续根更新；最终未过期完整链验证前禁止新资格。Root 必须从当前版本开始逐版本验证；每个后继 Root 同时满足当前 Root 和自身规定的门限，禁止跳版本。接受新 Root 后不能回退旧 Root。
- Package Manifest 由不可变内容哈希和上层引用防回滚，不使用时间到期替代高水位。
- 任何续签或字段变化只要改变某角色的签名封装字节，就必须提升该作用域 metadata version；Release 续签使用相同 releaseId 和不可变业务内容创建更高 revision，并由新 Snapshot/Target 引用。原 revision 的不同字节续签必须被拒绝。

## 7. 验收

每类凭据均需固定向量和负向测试，至少覆盖字段删除/替换、对象串用、渠道/channelRevision/targetKey/packageId/世代/Registry/Rollout/BridgeEligibility/SupplyChainApproval 错绑、baseline-ordinary、baseline-bridge、candidate-ordinary、candidate-bridge 的端点/跳分别失效、Stable→Beta→Stable 渠道 ABA、事件凭据跨类型和跨 installationScopeId、正常轮换与紧急 key deny、未知算法、旧 Root、签名不规范、元数据/凭据/例外时间边界前 1 ms/等于/后 1 ms以及重放。元数据向量覆盖候选未命中防泄露、正确/错误 commitment opening、合法历史 Release 非成员、自身摘要拒绝、逐角色低版本、同版本不同字节、Release 跨到期续签、旧 revision 回放、Target generation 不等、metadata_sync_pending、PublishMetadataRefresh 与选择发布并发、产品双 component 的 genesis/active 混合 Root 轮换、同版本分叉 Root、单 component 轮换后另一 component 读取、staged 两级 base head/Root/supportFloor 变化、跨 scope component head CAS 和 staged 15m/24h 边界。consume-status 向量覆盖错误/枚举 secret、字段错绑、业务提交与 tombstone 原子崩溃、24h/61d/62d 边界且绝不返回凭据。事件向量覆盖提交成功、ACK 丢失、离线数日、key deny 后同 eventId 重试。密钥轮换测试必须跨越对象有效期、事件补报期和幂等恢复期中的最长窗口。任一失败不得退回仅依赖 TLS、SN 或客户端自报字段的准入方式。

追加验收覆盖三用途及required/notRequired、purpose错绑statusHash/非循环请求摘要/NUL编码、Root过期根更新、普通前置路径审批ABA、下载跨24h/多会话/URL及key轮换续接、不重签旧事件或增加样本、任务不可授升级权、原目标健康与更高修复不可替代，以及短期会话到期不形成隐性下载硬限。

下载续接固定向量另覆盖双会话/双check并发CAS、旧会话丢ACK/合法补报不得提交新阶段动作、已失败workflow不能被缓存成功覆盖、候选变基线仍重新验证当前链而原质量归属不变。所有时间发生窗口为[nbf,eventNotAfter)，上传为[nbf,exp)，等于边界拒绝；eventNotAfter≤规定发生上限，exp≤规定上传上限，取签发及原事务消费可信时刻，不从每次补报重算。
