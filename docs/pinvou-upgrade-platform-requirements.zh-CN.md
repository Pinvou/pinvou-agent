# 品悟统一升级平台需求文档

> 状态：V1.0，产品需求主文档
>
> 日期：2026-10-08
>
> 适用产品：Pinvou Agent
>
> 首期平台：Windows、Linux、macOS

## 1. 文档定位

本文定义品悟统一升级平台的产品范围、业务规则、后台能力、客户端流程、跨平台要求和验收标准，供产品、研发、测试和运营共同评审。

本文刻意不展开数据库事务、租约、CAS、恢复 worker、密钥阈值和内部账本结构等实现细节。相关内容分别见：

- [品悟升级协议与安全要求](pinvou-upgrade-protocol-security.zh-CN.md)
- [品悟统一升级平台技术设计草案](pinvou-upgrade-platform-technical-design.zh-CN.md)

发生冲突时，产品范围和用户可观察行为以本文为准，安全边界以安全要求文档为准；技术设计不得改变前两者的业务语义。

## 2. 背景与目标

品悟现有升级能力主要围绕 Windows 和旧升级服务建立，难以统一支持多平台、灰度发布、发布撤回、升级助手演进及后续增量升级。需要建设一个统一升级平台，使后台发布、客户端检查、下载安装和结果观测形成完整闭环。

首期目标：

1. 统一 Windows、Linux、macOS 的版本发布和客户端升级流程。
2. 支持 Stable、Beta、Internal 三个公开渠道及基于硬件 SN 的灰度分组。
3. 支持完整包升级，并从包结构、Manifest 和接口层面预留增量升级。
4. 支持第三方文件服务，不把安装文件限定在控制面域名下。
5. 支持升级助手独立于主应用运行，使应用退出或被替换后仍能完成安装和结果确认。
6. 支持旧客户端通过旧升级系统升级至迁移引导版本，再切换到新平台。
7. 为未来新操作系统提供稳定的平台适配接口，避免修改版本选择和发布主流程。

## 3. 范围与非目标

### 3.1 首期范围

- 升级后台服务和管理后台。
- Windows、Linux、macOS 桌面客户端完整包升级。
- 普通升级、静默升级和强制升级三种封闭升级类型。
- 版本、平台目标、渠道、发布、灰度、暂停、撤回和前向修复管理。
- 更新检查、文件信息、下载、校验、安装前复核、安装和结果上报。
- 升级助手及其版本化接管能力。
- 完整包与增量包共用的复合更新包模型；首期增量候选为空。
- 发布、安装和质量观测。

### 3.2 非目标

- 不提供降级安装或“回滚发布”。
- 不自动恢复升级前的应用版本。
- 不下载、缓存或执行恢复包。
- 首期不执行增量升级，但不能采用阻碍后续增量接入的包结构或接口。
- 不兼容旧升级协议；旧客户端通过旧系统取得一次迁移引导版本。
- 硬件 SN 不承担设备认证、许可证校验或安全准入职责。

当前版本出现问题时，应暂停或撤回问题发布，并发布一个版本号更高的修复版本，使客户端继续向上升级。

## 4. 核心原则

1. **控制面地址固定**：更新检查和控制接口直接访问 `https://update.pinvou.com`，不经过域名发现或域名引导。
2. **控制面与文件面分离**：控制面返回具体下载地址；地址可以属于品悟，也可以属于经批准的第三方对象存储或 CDN。
3. **只向前升级**：目标版本必须严格高于客户端当前版本；切换渠道也不能触发降级。
4. **发布内容不可变**：同一发布和同一 packageId 指向的 Manifest、文件大小及哈希一经发布不得原地替换。
5. **SN 仅用于灰度**：SN 可以参与灰度分组，但缺失、不可读取或关闭发送不得阻止安全基线版本和 100% 发布。
6. **安装前最终复核**：下载成功不等于允许预安装、安装或激活；客户端执行相应写入或切换前必须重新确认版本、平台、文件、授权用途和当前发布资格。
7. **主应用不自我覆盖**：安装由外部升级助手持有事务并调用平台安装器。
8. **失败可解释**：失败必须给出稳定原因、可执行的重试或人工修复入口，不能永久停留在“更新中”。

## 5. 版本、渠道与平台模型

### 5.1 产品版本

- 产品版本采用 SemVer 三段式 `major.minor.patch`，例如 `0.11.0`。
- Windows 四段文件版本映射为 `<major>.<minor>.<patch>.0`，第四段不参与产品版本排序。
- Internal、Beta、Stable 通过渠道表达成熟度，不使用 prerelease 字段制造另一套排序规则。
- 所有升级均要求 `targetVersion > currentVersion`。

### 5.2 渠道

| 渠道 | 定位 | 加入方式 |
|---|---|---|
| Stable | 默认稳定发布渠道 | 默认加入 |
| Beta | 公开测试渠道 | 用户自愿加入并确认风险 |
| Internal | 公开早期体验渠道 | 用户自愿加入并确认更高风险 |

Internal/Beta 不是员工专属或私有白名单渠道。渠道切换影响后续检查，不允许因为从预览渠道切回 Stable 而自动降级。

三个渠道独立投放，不存在客户端或服务端检查时的隐式渠道继承：仅在 Stable 发布的版本不会自动成为 Beta/Internal 的更新。后台提供“将正式版同步投放到 Beta、Internal”的显式操作，按 8.3.1 为所选渠道分别创建投放、完成审批并生效；客户端继续只检查自己当前渠道，渠道设置和 channelRevision 不因后台同步改变。

同步投放仍遵守三段产品版本及只向前升级规则。例如 Stable 正式版为 `1.2.5`，Beta 用户当前为 `1.2.4` 时可在 Beta 投放生效且其他条件满足后收到更新；当前为 `1.2.5` 或 `1.3.0` 时不会收到该版本的升级。同一三段版本从预览状态转为正式状态不触发重装；制品修复必须发布更高产品版本，不能通过渠道成熟度绕过版本比较。

渠道切换原子增加 channelRevision，并立即使未消费的旧决定、下载资格、预安装授权、安装授权和激活授权失效；旧遥测会话进入 `channel_changed` 终态。normal/forced 已消费的安装事务和 silent 已消费的激活事务是唯一继续例外：它们不被渠道切换中止，继续使用事务创建时冻结的 channelRevision 完成核对和事件上报；这些事件只报告既有事务，不能取得新的下载、预安装、安装或激活权。silent 已消费但仍在进行的预安装不属于该例外，必须立即取消并清理不完整槽位；已经完成 staged 但尚未进入已消费激活事务的旧渠道内容立即失去激活资格、隐藏重启提示并清理，不允许因新渠道恰好选择相同 packageId 而复用。事务结束后的下一次检查使用新渠道 revision。

per-machine 安装的渠道配置由本机所有用户共享。渠道切换需要管理员权限并记录确认；不同用户不能同时使用互相冲突的渠道设置。

### 5.3 平台目标

一个平台目标由 `os + arch + packageFormat + installScope` 唯一确定，例如：

- `windows-x86_64-nsis-perMachine`
- `linux-x86_64-deb-perMachine`
- `macos-universal-dmg-perMachine`（请求中的 hostArch 仍区分 Intel 与 Apple Silicon）

服务端只能向与客户端实际操作系统、架构、系统版本、安装范围和包格式匹配的目标返回更新。

`targetKey` 必须完整包含上述四个维度；未来增加 per-user 安装时必须创建新的 targetKey，不能与 per-machine 目标共享发布、缓存或选择世代。

### 5.4 支持下界

每个 `product + component + channel + targetKey` 配置一个 `supportFloorVersion`，表示新平台承诺提供自动升级路径的最低实际来源版本。

- Stable 初始下界等于该目标的迁移引导版本。
- Internal/Beta 或未来新渠道在首次激活前必须显式审批 support floor 初值；默认建议继承同 targetKey 的 Stable 当前下界，但系统不得静默生成或接受空值。
- `currentVersion < supportFloorVersion` 时，不再尝试 `minimumSourceVersion=0.0.0` 的自动路径，固定返回人工升级入口。
- 提高下界会切断更旧来源的自动升级，必须双人审批、展示受影响安装量并提前公告人工升级方案。
- 降低下界会扩大兼容承诺，必须先完成新增来源版本的真实迁移和升级测试。
- 下界按渠道和平台目标隔离，修改后必须重算该作用域的完整可达路径并写审计。

### 5.5 升级类型

每个 Deployment 必须且只能声明一种 `upgradeType=normal|silent|forced`。升级类型决定客户端检查后的用户交互、下载、预安装和使用限制；后台不再提供可与升级类型任意组合的独立交互、下载或安装策略。

| 类型 | 检查与发起 | 下载与安装 | 对当前版本使用的影响 |
|---|---|---|---|
| `normal` 普通升级 | 用户主动检查，或客户端定期检查后提示；必须由用户明确点击升级 | 用户发起后才下载、校验、申请权限并安装 | 用户可关闭或稍后处理，并继续使用当前版本 |
| `silent` 静默升级 | 客户端定期自动检查，无需用户发起 | 自动下载、校验并把更新文件完整预安装到非活动版本槽；完成后提示用户重启，重启时切换并启动新版本 | 切换前不关闭当前进程、不改变活动版本；用户可延后重启并继续使用当前版本 |
| `forced` 强制升级 | 客户端在启动及运行期间自动检查；达到可安装时间且命中后进入强制升级门 | 自动下载和校验，用户只能完成必要的权限确认、重试或退出；升级成功前不能进入普通功能 | 只保留当前未保存内容的安全落盘、升级/修复、网络设置和退出入口，不允许继续普通业务操作 |

三种类型共同遵守发布暂停/撤回、制品吊销/隔离、安装有效期、平台兼容、系统权限、备份和安全校验，任何类型都不能绕过这些条件。

本文中的“安装授权”“预安装授权”和“激活授权”均指升级服务端签发、由助手自动请求的短期许可，分别限定直接安装、写入指定非活动槽和切换指定已预安装版本；不是管理员权限、UAC 确认或用户点击同意。静默重启切换的“激活授权”即“服务端激活授权”。“本机执行权限”指受保护 helper 在操作系统上执行对应动作的既有权限。silent 必须使用已部署且具备必要权限的可信 helper，无需用户再次提权；既有权限不足时不得自动弹出提权窗口、执行迁移或切换版本，应保持原活动版本并提示用户主动修复升级组件。用户主动修复组件属于独立、明确确认的操作，不得冒充静默升级，也不得绕过已有强制或人工修复门。

为统一处理到期、暂停和撤回，三种类型使用明确的 `executionCommitBoundary`：normal/forced 的边界是升级助手调用平台安装器的瞬间；silent 预安装不跨越该边界，silent 激活的边界是首次写入活动数据或切换活动 launcher 指针二者中较早发生的瞬间。客户端必须在跨越边界的紧前一步在线复核安装有效期和当前授权；边界前能够安全取消并保持原活动版本，边界后不得因策略变化、到期或失败自动恢复旧版，必须继续收敛到 succeeded 或 failed_manual_repair_required。

executionCommitBoundary 属于一个实际 hop 的执行事务，不属于整条升级路径。强制升级门记录期望终点和当前 hop；当前 hop 尚未创建、未开始或已消费但未越界时，均按当前 hop 边界前处理。已成功结束的历史 hop 不使后续 hop 永久处于“已经越界”，也不产生人工修复门。中间 hop succeeded 后先确认当前活动版本和数据健康，再重新检查终点：终点仍有效时保持强制门并继续下一跳；可信确认终点到期或在线确认终点失效时，取消尚未越界的下一跳并解除本次强制门，保留已成功安装的当前版本。仅路径失效但终点仍有效时保持受限门。任何独立的既有失败事务修复门均不随本次强制门解除；当前 hop 已越界且未成功时仍须先收敛，失败或无法确认成功则保留人工修复门。

静默升级中的“预安装”只表示把已验证的完整目标版本释放到受保护的非活动版本槽并验证可启动性，不等于切换活动版本，也不在当前进程运行期间迁移活动数据。预安装完成后固定展示目标版本和“重启后完成升级”；下一次由用户发起的应用重启或系统重启才尝试切换。切换前必须在线重新检查发布资格并取得用于激活的短期授权；无法联网、发布已暂停/撤回、Target 已吊销、Artifact 已隔离或授权失败时，继续启动原活动版本，不切换、不自动回滚，也不把静默预安装误报为升级成功。

静默预安装结果设置 `stagedValidUntil=min(stagedAt+30d, installNotAfter)`；`installNotAfter` 为空时只使用 `stagedAt+30d`。到期、资格被明确撤销或权威检查不再选择精确相同的 staged 下一跳时，该内容立即失去激活资格并取消重启提示。同一 Deployment/Rollout 的临时 paused 是唯一例外：客户端隐藏重启提示、保留槽位但禁止激活；恢复后必须在线重新检查，仍精确选择同一 hop 才恢复提示，否则立即清理。若新结果是 normal/forced 或无更新，客户端清理旧非活动槽；若新结果是更高的 silent 下一跳，可在临时受保护区域完成新版本后再原子替换旧槽，但旧版本在此期间也不得激活或继续提示。每个安装范围同一时刻最多有一个具备激活资格的待激活版本。需要的数据冻结和 required 备份在重启后的激活准备流程中、服务端激活授权签发前完成；不可逆迁移只能在该授权消费并完成边界前最终复核后、活动版本指针切换前执行。

在线进入强制升级门时，服务端可信时间必须同时位于强制期望终点和当前实际下一跳的安装有效区间，且该下一跳此刻可取得与 forced 兼容的安装授权；在此之前仅可做非阻断提示或预下载，不能提前禁用普通功能。若客户端已在线验证一个将在未来生效的 forced 决定，随后离线且能够可信证明已经跨越 installNotBefore，则到达边界时进入“联网复核受限门”：只允许安全保存、网络设置、升级/修复和退出，在重新联网取得当前安装授权前不得安装或进入普通功能；时间无法可信判定时按 17.1 执行。该决定的普通凭据到期不自动解门。可信确认已经到达 Deployment.installNotAfter，或联网确认该 forced 终点已暂停、撤回、失去资格或不再命中时，按当前 hop 的执行边界处理：当前 hop 未越界或尚未创建，且当前活动版本健康时解除本次强制门；前一 hop 已成功结束不妨碍解除。当前 hop 已越界且尚未成功时继续收敛，成功后终点仍失效且当前版本健康即可解除，失败或无法确认成功则进入人工修复门。任何独立的既有失败事务修复门均不因本次强制终点失效解除。客户端从未取得过可信 forced 决定时，离线状态不得凭空推断新强制发布，下一次联网检查后再按结果处理。进入任何强制门时，客户端先安全保存当前可保存状态，再禁止发起新的普通业务操作。升级因权限拒绝、校验失败或安装失败而未完成，且终点仍有效时，保留上述受限入口，不得绕过升级门。

强制升级若需要一个或多个向上跳转，发布审核必须确认 supportFloor 及当前来源事实登记范围内的每个受支持来源都有完整路径，且每个实际 hop（ordinary 与 bridge）都支持 forced 的 directInstall 模式、每个实际 hop 的安装有效区间都完整覆盖强制 Deployment 的有效区间；任一受支持来源不满足时，forced Deployment 不得激活，必须先补齐路径或按 supportFloor 变更流程正式停止支持。运行时遇到登记外的不兼容来源时返回人工升级入口，不能进入无可执行动作的强制升级门。进入门后，每个下一跳成功都必须重新检查并确认下一跳仍可立即安装；终点仍有效时，中间 hop 成功不解门；终点在后续 hop 开始前失效时按本节逐 hop 规则处理。正常路径以原强制期望终点的事务达到 `succeeded` 为完成条件。仅版本号已经等于终点但活动版本核对或健康检查失败时，不得解除升级门，而是保持人工修复、网络设置、安装更高前向修复版本和退出入口。失败后的前向修复必须创建独立事务，绑定同一 installationScopeId、原强制终点和被修复事务；修复目标版本必须同时高于原强制终点和本机已经安装或已发生受保护写入的最高目标版本，并通过当前来源及迁移兼容性复核。只有修复事务完成活动版本核对、数据一致性核对和 11.8 的健康检查并达到 `succeeded`，才可解除与其明确关联的修复门；原失败事务保持原终态，不改写为成功。原终点暂停、撤回或到期不解除已经跨越 executionCommitBoundary 的修复门；边界前的失效解门仍按 6.2 执行，时间无法可信判定时按 17.1 执行。任何路径均不得合并为一次多跳安装。

normal 的用户确认只授权当前响应中的一个实际 hop，不授权整条升级路径。界面必须同时展示期望终点和“本次先升级到”的实际 hop；该 hop 成功后重新检查，即使 endpoint、upgradeType 和路径均未变化，也必须重新提示，用户再次点击前不得下载或安装下一跳。ordinary 与 bridge 使用相同的逐跳确认规则。

## 6. 发布对象与生命周期

### 6.1 核心对象

| 对象 | 作用 |
|---|---|
| Product / Component | 定义可独立升级的产品组件 |
| Platform Target | 定义操作系统、架构、包格式和宿主约束 |
| Artifact | 后台存储的内容不可变文件，如完整包、增量包、SBOM 和构建证明 |
| Package | 客户端实际下载的完整包或增量包；每个下载单元有独立 packageId |
| Release | 同一产品版本、发布说明和多个平台目标的逻辑分组，不承担逐平台审批、暂停或吊销 |
| Release Target | 一个 Release 在一个 targetKey 上的制品、最低来源版本、迁移策略和独立审批/吊销单元 |
| Deployment | 一个已批准 Release Target 在一个渠道中的投放、策略、安装有效期和暂停/撤回单元 |
| Rollout | 一个 Deployment 的候选灰度百分比、SN 分组和扩量状态 |
| SupplyChainApproval | Release Target 在具体渠道上的供应链风险审批；绑定扫描证据与限时例外 |

### 6.2 状态

六类对象使用独立生命周期，不能用一个“版本状态”同时表示上传、审批和投放：

- Artifact：`uploading → validating → valid|rejected`，以及 `valid → quarantined`；有效后内容不可修改。
- Release：`draft → assembled → closed|cancelled`；自动取消只发生在全部 Target 均已进入 rejected/revoked/cancelled 终态且不再可能 approved 时。主动放弃必须执行独立的高权限 `CancelRelease`，在同一事务把 draft/in_review Target 置为 cancelled、approved Target 置为 revoked，并把 Release 置为 cancelled，记录原因和操作者。closed/cancelled 为终态，均不出现 paused/withdrawn 等投放状态。
- Release Target：`draft → in_review|cancelled`，`in_review → approved|rejected`，以及 `approved → revoked`；另允许高权限 `CancelRelease` 在父子原子收敛中专用地执行 `in_review → cancelled`，普通单 Target 操作不得触发该边。不同平台目标独立审批和吊销，cancelled/rejected/revoked 为终态。
- Deployment：`draft → in_review → scheduled|rejected`，`scheduled → active ↔ paused`，以及 `scheduled|active|paused → withdrawn|superseded`。
- Rollout：`draft → running|aborted`，`running → paused|completed|aborted`，以及 `paused → running|aborted`；paused 不能直接 completed。
- SupplyChainApproval：`pending → approved|rejected`，以及 `approved → expired|revoked`；恢复或扩大资格创建更高 revision。

其中：

- Deployment `paused`：停止新的下载以及预安装、安装和激活授权；进行中的 silent 预安装安全取消并清理不完整槽位，已经完成 staged 的内容隐藏重启提示并保留至 stagedValidUntil，但不得激活，恢复后只有在线重新检查仍精确选择同一 hop 才恢复提示，否则清理。
- Deployment `withdrawn`：永久停止该次投放，不再恢复。
- Deployment `superseded`：不再作为当前终点，但可在满足 9.2 的桥接资格时作为中间跳。
- Release Target `revoked` 或 Artifact `quarantined`：立即失去全部渠道的新下载、桥接、预安装、安装和激活资格；silent 已 staged 内容在下次本地收敛时清理。
- 不提供把某历史版本重新标记为“回滚目标”的操作。

Rollout 的用户可观察语义固定如下：

- 每个 `product + component + channel + targetKey` 形成一个 SelectionScope，初始为 unactivated，此时没有基线；active 后必须恰有一个当前基线指针，最多有一个 running 候选 Rollout。每个 Deployment 最多有一个非终态 Rollout。违反唯一性时选择服务失败关闭，不按创建时间猜测。
- 除 6.3 为 unactivated scope 首次建立基线外，候选终点版本必须严格高于本 SelectionScope 当前基线版本，基线指针只能切换到严格更高版本。该规则适用于普通新建、重建及同步投放；提交审核、排期、激活或恢复候选 Deployment、启动或恢复 Rollout、扩量及 completed 均在提交点重新比较当前基线版本和身份，不能只依赖草稿创建时的结果。基线已推进到相同或更高版本时拒绝该候选操作，保持全部实体和基线指针不变，返回 `candidate_not_above_baseline` 并提示终止过时候选或创建更高版本。基线 paused/withdrawn 或失去下载资格不降低比较基准，也不允许通过清空指针再次走首次激活来退回旧基线。此约束针对候选期望终点，不禁止合法路径中低于基线、但严格高于本机当前版本的 ordinary/bridge 中间跳。
- 只有 `Deployment=active + Rollout=running` 才是候选灰度；`scheduled/rejected/paused/withdrawn/superseded` Deployment 均不能作为当前终点，superseded 只能按 9.2 进入桥接分支。
- Rollout `paused`：停止该候选的新决定、下载 URL 以及预安装、安装和激活授权；尚未消费的安装/激活授权失败关闭，已消费但未跨越 executionCommitBoundary 的安装/激活事务安全取消，已经跨越边界的事务继续核对和上报；进行中的 silent 预安装安全取消并清理不完整槽位。silent 已完成 staged 但未激活的内容隐藏重启提示并保留至 stagedValidUntil，暂停期间不得切换；恢复后按 5.5 的同一 hop 复核规则恢复提示或清理。暂停保留当时的实际百分比；恢复只能回到同一 active Deployment 的 running，并从恢复提交时间重新计算当前阶段的完整观察时长和样本，不能因暂停期间经过的时间直接扩量。
- Rollout `aborted`：永久终止本次候选。主动终止记为 `abortCause=operator_abort` 并在同一管理操作中把父 Deployment 置为 withdrawn；因父状态变化而级联终止时使用 `parent_withdrawn` 或 `parent_superseded`，父 Deployment 保持该管理命令选择的终态。后续重试必须创建新 Deployment/Rollout 修订，审计记录固定原因。
- Rollout `completed`：仅能从 running 在有效灰度达到 100%，且 100% 最终阶段自身的最短观察时长已经结束、合格样本达到该阶段最小样本量、全部质量指标低于阈值、不存在“停止自动扩量”锁存标记，并完成审批，且线性化提交点的服务端可信时间仍位于父 Deployment 安装有效区间时使用。刚进入 100% 阶段不得立即 completed。paused 必须先显式恢复为 running，并通过 PublishSelectionChange 刷新 current Timestamp/Snapshot/Target、commitment 和 selectionGeneration，不能直接完成；恢复后须重新满足 100% 阶段的完整观察条件。完成操作本身也必须通过 PublishSelectionChange，把新 Deployment 设为渠道基线、把旧基线置为 superseded、把 Rollout 置为 completed、发布新 Timestamp/Snapshot/Target 并增加 selectionGeneration；任一前置条件或写入失败都不改变上述任何对象，也不暴露部分完成结果。
- 父 Deployment paused 时，运行中的 Rollout 在同一管理操作中进入 paused；恢复时父 Deployment 与进行中的 Rollout 在同一管理操作中分别回到 active/running。父 Deployment withdrawn/superseded 时，未完成 Rollout 在同一管理操作中进入 aborted。Rollout 只能在 active 父 Deployment 下进入 running。

“恢复投放资格”与“恢复自动扩量”是两项独立操作。paused→active/running 只重新开放暂停前同一百分比下的资格；提交点必须复核正向实体链、供应链审批、安装时间窗、候选高于当前基线、原灰度计划及完整升级路径，确认暂停原因已被合法处置，记录原因和审批，并通过 PublishSelectionChange 原子发布。当前基线 Deployment 自身恢复不适用候选高于基线的比较；若其 Rollout 不存在或已经 completed，只恢复基线投放资格，保持原基线指针及 completed 终态，不创建或重启灰度、不重新完成基线切换，仍须满足上述适用审批、安全原因处置、时间窗及路径条件。下述百分比、重新观察和扩量规则只适用于仍有进行中 Rollout 的候选。恢复投放不要求在暂停状态下先收集暂停所禁止的新升级样本，不增加百分比、不改写阶段计划、不清除任何扩量冻结；新的完整观察窗口从恢复提交时刻开始，样本不足时可以在同一百分比继续取得合法流程和样本，但不能扩量或 completed。存在扩量冻结时，必须在投放已恢复后另按 8.2.1 完成全部问题分组复核，才可申请解除冻结；没有扩量冻结时，恢复提交自动启动当前阶段的完整重新观察，仍满足全部样本、时长和质量条件后才自动进入紧邻阶段。withdrawn/aborted/revoked/quarantined 等终止或安全禁止状态不能借此恢复。

每个 Deployment 必须声明安装有效期：`installNotBefore` 必填，`installNotAfter` 可为空。有效区间使用服务端可信 UTC，定义为 `[installNotBefore, installNotAfter)`；`installNotAfter=null` 表示无计划日历终点，但仍受暂停、撤回、吊销和隔离控制。到达 `installNotAfter` 的瞬间，结果按 executionCommitBoundary 唯一确定：

1. 授权未消费：拒绝消费，不开始预安装、安装或激活。
2. 授权已消费但尚未跨越 executionCommitBoundary：事务必须收敛为安装前取消，禁止到期后再开始平台安装器、活动数据写入或 launcher 切换；normal 保持旧版，silent 使 staged 内容失效并清理，forced 解除本次门并显示 `deployment_window_closed`。
3. 已跨越 executionCommitBoundary：既有事务不因到期中止或恢复普通功能，继续收敛到 succeeded 或 failed_manual_repair_required；失败后只能人工修复或安装更高版本前向修复。
4. 已完成安装或活动指针切换但健康检查失败：保持 failed_manual_repair_required，不受 installNotAfter 解门影响。

同时停止签发新决定、下载 URL 以及预安装、安装和激活授权；已有下载 URL 最多残留其原始有效期，不能据此开始新动作。未开始事务的客户端显示稳定原因 `deployment_window_closed`，重新检查更高版本；没有可用版本时提供固定人工入口。修改时间窗必须创建新 Deployment 修订、重新审批，并通过 PublishSelectionChange 增加选择世代和发布 current 元数据。

除 installNotAfter 使用上述到期矩阵、用户主动切换渠道使用 5.2 的冻结事务规则外，其他服务端资格收紧统一使用以下“资格失效矩阵”。适用事件包括 Deployment/Rollout paused，Deployment withdrawn/superseded，Rollout aborted，Release Target revoked，Artifact quarantined，BridgeEligibility disabled，SupplyChainApproval expired/revoked，以及元数据即时 deny：

1. 决定或授权未消费：立即拒绝并把对应会话收敛到带稳定原因的资格失效终态；forced 按 5.5 判断当前 hop 和终点失效解门，下一跳尚未创建时也适用，不能因历史 hop 已成功越界而保持无可执行终点的强制门。独立失败事务修复门保留。
2. 安装/激活授权已消费但尚未跨越 executionCommitBoundary：必须安全取消并进入 `cancelled_before_install`，释放产品锁；normal 保持旧版，silent 保持旧活动版本。forced 若期望终点本身失效则解除本次门；若仅当前 hop 或桥接资格失效而 forced 终点仍有效，则保持只含网络、重新检查、人工升级/修复和退出的受限门，直到取得新的可执行路径或终点失效。
3. silent 预安装正在进行：立即取消并清理不完整槽位。已经完成 staged 的内容只有在同一 Deployment/Rollout 临时 paused 时按 5.5 隐藏提示并保留；withdrawn、superseded、aborted、revoked、quarantined、BridgeEligibility disabled 或权威选择改变时立即取消激活资格、移除提示并清理。
4. 已跨越 executionCommitBoundary：资格收紧不强制中止或恢复旧版，事务继续收敛到 succeeded 或 failed_manual_repair_required；事件仍按原事务 lineage 接受，但不能取得任何新资格。

上述矩阵适用于当前基线和候选，不以是否存在 running Rollout 为前提。

跨实体正向条件固定为：Release 只有在业务字段及至少一个 Target 组成的非空目标集合冻结后从 draft 进入 assembled；Release Target 只有在父 Release assembled、全部引用 Artifact=valid、Package Manifest/供应链闸门通过时才能进入 in_review/approved；全部 Target 均处于 approved/rejected/revoked/cancelled 且提交点至少一个 approved 后 Release 才能 closed；Deployment 只有引用 Release=closed、Release Target=approved、全部 Artifact=valid 且时间窗合法时才能 scheduled/active。选择与最终复核必须继续满足这些正向状态，不得只排除 revoked/quarantined 等少数负面状态。

若 assembled Release 的所有 Target 均已处于 rejected/revoked/cancelled 终态，自动收敛命令才把 Release 置为 cancelled；只要仍有 draft/in_review Target，就不得因当前 approved 数为零而自动取消。操作员主动放弃使用 `CancelRelease`：以 Release/全部 Target revision 做 CAS，在一个领域提交中把 draft/in_review Target 置为 cancelled、approved Target 置为 revoked、保留既有失败终态并把 Release 置为 cancelled；任一竞争变化使整笔零写入。Release closed 允许其 Target 终态集合包含 approved、rejected、revoked、cancelled，但关闭提交点至少有一个 approved；关闭后某 approved Target 再 revoked 不改写 Release 终态，只会阻止引用该 Target 的投放。

### 6.3 首次激活与渠道供应链审批

SelectionScope 首次激活使用一个可线性化命令。unactivated scope 的首个 Deployment 从 in_review 进入 scheduled 时，校验签名 Release/Package 及一组 schema 完整、绑定预期 scheduled revision、生成时产品 Root head、product/component metadata head 和完整安全读集的 staged Target/Snapshot；staged 集合最长 15 分钟，不设为 current、不被 Timestamp 引用且任何客户端不可取得。激活命令必须比较产品 Root head并 CAS component metadata head，重验 Root、supportFloor 及全部实体 revision，再执行 scheduled→active，发布包含原有全部引用及新 Target 的 current Snapshot、新 Timestamp，写入 `SelectionScope=active`、唯一 baseline pointer 和初始 `selectionGeneration=1`。其他 scope 在 staging 后发生发布、产品 Root 轮换或两个 scope 并发激活时，旧 staged 失败、零写入并基于最新完整集合重签，不能回退或覆盖别的 scope。staged 在成功、替换或到期后 24 小时内删除完整内容，只留审计摘要。unactivated check 固定返回 HTTP 409、`SCOPE_UNACTIVATED`、`retryable=true` 和普通检查间隔，不创建决定凭据、telemetrySession 或伪造的 current 元数据/基线；客户端展示“渠道尚未激活”。

SupplyChainApproval 按 `releaseTargetId + channel` 唯一作用域，使用单调 revision，并绑定漏洞数据快照、风险结论以及每个安全例外的 ID/revision/expiresAt；`effectiveExpiresAt` 为所有绑定例外及审批自身非空到期时间的最小值，无到期项时为空。Stable 的 Critical/High 必须为零或逐项具有当前有效的双人限时例外；Internal/Beta 也必须有显式风险接受记录。Deployment 进入 scheduled/active、从 paused 恢复，Rollout 启动/扩量/completed 以及动态下载/授权均在提交点复核该审批；draft/in_review 可在审批完成前创建，但不能产生投放资格。到达非空 effectiveExpiresAt 或审批 revoked 时，先以同一紧急收紧事务写即时 deny、递增对应 SelectionScope 的 selectionGeneration 并置 `metadata_sync_pending`；当前链改用新批准 revision 恢复/扩大资格时，必须通过 PublishSelectionChange 同时递增 generation 并发布新的 current Timestamp/Snapshot/Target。已经交给安装器的事务继续上报。

产品级 Root 只通过唯一 RootPublishHead 逐版本轮换；同版本不同字节或两个 component 分叉发布均拒绝。所有扩大资格或改变可选择集合的操作统一使用 `PublishSelectionChange`：绑定产品 Root head与 `product+component` metadata head，在一个提交中比较前者、CAS 后者，并写实体 revision、受影响全部渠道的 selectionGeneration、current Target、完整 Snapshot 和 successor Timestamp。该规则适用于 supportFloor、Deployment、Rollout、BridgeEligibility、SupplyChainApproval、Source Facts Registry 及其他 generation 变化；Registry 跨渠道变化必须全成或全败。安全收紧可以先写即时 deny 和 `metadata_sync_pending`，但同一事务必须登记唯一、持久化的元数据追平任务；元数据追平且 `Target.selectionGeneration` 等于当前 scope generation 前，所有新决定、下载、预安装、安装和激活资格失败关闭。恢复器至少每 30 秒发现待办，以最新完整集合重签并只在发布成功的同一提交清除 pending；失败持续重试，15 分钟未收敛告警，禁止人工直接清标记。

Root 轮换只 CAS 产品级 RootPublishHead。Release 续签以及不改变可选择集合的 Target/Snapshot/Timestamp 续签绑定产品 Root head 与 `product+component` metadata head：基于最新完整集合重签，在一次提交中比较前者、CAS 后者并切换 current 指针。它们不递增 selectionGeneration，除非同一操作同时改变可选择集合；任一冲突必须重读、重建、重签，不能覆盖并发发布。

暂停、撤回、制品隔离及恢复投放均属于高风险操作，必须记录操作者、原因、时间和审批结果。

## 7. 更新包要求

### 7.1 复合包

上传包采用一个复合 ZIP，语义参考现有 Windows 更新样包。逻辑结构如下：

```text
PinvouUpdatePackage.zip
├── UpdatePackInfo.json       # Package Manifest
├── SBOM.cdx.json             # 与最终安装器绑定的软件物料清单
├── Provenance.intoto.jsonl   # 与最终安装器绑定的构建来源证明
├── FullPack.zip              # 必需的客户端下载单元
└── IncrementalPack_*.zip     # 一期生产发布禁止，协议测试夹具允许
```

物理目录可以按平台调整，但以下语义必须统一：

- Package Manifest 必须存在，首期文件名固定为 `UpdatePackInfo.json`，描述上传内容、目标平台、完整包、预留增量候选和各下载单元内的升级助手，但不绑定包含自身的外层 ZIP 字节哈希。
- Package Manifest 必须绑定 SBOM 和构建来源证明的文件名、Schema/格式、大小和 SHA-256；二者只由后台供应链闸门使用，不随子包提供给客户端。
- `FullPack.zip` 首期必须存在，且能够独立完成升级。
- 一期生产 Release 的增量候选必须为 `[]`，不得上传或下发非空增量包；非空候选只用于隔离的协议合约测试。
- 升级助手制品必须有独立版本、大小、哈希、平台身份和安装目标。
- 包内路径只能是规范化相对路径，不得写入任意绝对路径或逃逸目标目录。

外层复合 ZIP 只用于后台上传。其大小和哈希由上传会话在 Package Manifest 外单独记录，用于传输完整性和审计，不能作为 Package Manifest 的自引用字段。后台解析、校验后，把 FullPack 和未来每个 IncrementalPack 分别存为独立不可变下载制品；客户端不下载或校验整个外层复合 ZIP。

### 7.2 Package Manifest

Package Manifest 至少描述：

- Manifest Schema 版本、可选的 `manifestId` 逻辑标识、产品和组件；manifestId 不得是内容哈希或安全身份。
- 目标产品版本、平台目标、包格式。
- 完整包及每个预留增量下载单元各自的 packageId、容器大小和哈希。
- 每个下载单元重建或解压所得最终安装器的大小和哈希。
- 增量候选数组；首期允许为 `[]`。
- 每个增量候选的基础版本、基础制品身份、算法、补丁包身份和重建结果身份。
- 升级助手数组、实际执行事务的助手 ID。
- 目标安装后 launcher 身份。
- 支持的激活模式：`directInstall` 表示 normal/forced 使用的平台安装器直接安装模式；`stagedRestart` 表示 silent 使用的非活动槽布局、目标入口和重启切换前验证信息。一个包可声明一种或两种模式，但每种都必须独立通过目标平台认证。

Package Manifest 是不可变制品描述，**不设置到期时间**。Release、Deployment 和在线授权决定“当前是否允许安装”，不能通过修改或到期 Package Manifest 改写历史制品身份。

Package Manifest 的权威内容身份是上层 Release 在封装外计算并引用的 `manifestEnvelopeSha256`。该字段禁止出现在被其自身哈希覆盖的 Manifest/签名封装内；Manifest 不能声明“自己的封装哈希”。外层上传 ZIP 哈希仍由上传会话独立保存。

### 7.3 增量升级预留

首期不执行增量升级，但必须完成以下预留：

- Schema 和接口字段支持多个增量候选，且每个候选绑定唯一基础版本和基础制品。
- 检查请求声明客户端支持的增量算法和格式；首期均为空数组。
- 合约测试环境使用非空候选夹具，验证服务端能够解析并返回字段、客户端在能力为空时忽略候选并选择完整包。
- 一期不实现补丁重建、增量基础缓存或增量生产下载；这些能力和非空生产候选必须由二期开关及重新审核启用。
- 下载、校验、安装前复核和事件 Schema 预留区分完整包、补丁包及重建后最终安装器的字段。

未来启用增量升级时不得修改完整包流程的既有语义。

## 8. 管理后台需求

### 8.1 上传与解析

后台应支持拖拽或选择复合包上传，并自动完成：

- ZIP 安全解析、目录和大小限制检查。
- Package Manifest Schema 校验。
- 完整包、最终安装器和助手制品的大小及哈希核对；非空增量只在隔离合约测试中做 Schema/字段校验，不进入生产制品库。
- SBOM、构建来源证明、漏洞扫描结果及限时安全例外校验；最低安全口径以安全要求文档为准。
- Windows、Linux、macOS 平台身份验证。
- 包声明的激活模式与目标平台能力核对；normal/forced 的每个实际 hop 必须支持 directInstall，silent 的每个实际 hop 必须支持 stagedRestart，不能跨模式解释。
- 版本、平台、包格式和安装范围一致性检查。
- 重复 packageId、重复文件或不可变内容冲突检查。

解析失败时不得进入可发布状态，界面需定位到具体字段或文件。

### 8.2 发布配置

每个待发布版本至少配置：

- product、component、channel、platform target。
- targetVersion。
- `minimumSourceVersion`，可缺省；缺省按 `0.0.0` 处理。
- 当前作用域的 `supportFloorVersion` 及其影响提示；该字段属于 Target 级策略，不由单个 Release Target 私自覆盖。
- 更新说明（简体中文、英文、日文）。
- `upgradeType`，使用普通、静默、强制三种一期封闭枚举。
- `migrationMode` 与 `backupPolicy` 两个正交策略，使用下述一期封闭枚举。
- 自动扩量阶段计划、SN 包含组、SN 排除组。阶段百分比为 1—100 的整数，计划内严格递增；每个阶段配置最短观察时长和最小样本量，首期最后一个阶段必须为 100%。阶段计划和两类 SN 组作为同一份待审批灰度计划冻结，包含组与排除组的交集按排除优先处理。
- 发布时间 `releaseVisibleAt`、自动观察阈值和负责人。
- `installNotBefore` 和可空的 `installNotAfter`；界面必须展示 UTC 值、本地换算、当前状态和到期影响。

一期发布策略固定如下，不接受枚举外值：

- `upgradeType=normal|silent|forced`，行为严格按 5.5 执行。客户端和运营端不得把类型解释为可自由组合的提示、下载或安装选项，也不得在发布后原地改变类型。
- normal 必须等待用户明确发起后才下载；silent 必须自动完成检查、下载、校验和非活动版本槽预安装；forced 必须在达到可安装时间后阻断普通功能直至成功或服务端确认该强制终点失效。
- normal/forced Release Target 必须具备 directInstall 认证；silent Release Target 必须通过该 targetKey 的 stagedRestart、非活动版本槽、无交互预安装、重启激活、旧活动版本安全保留和预安装清理能力认证。任一所需能力不支持时不得审核或发布对应类型。首期 Windows、Linux、macOS 正式 targetKey 均须完成两种模式认证。
- `migrationMode=none|backwardCompatible|irreversible`，分别表示无数据结构迁移、迁移后旧版仍可读取、迁移后旧版不能安全读取；`backupPolicy=notRequired|required` 独立配置。none/backwardCompatible 可使用任一 backupPolicy；irreversible 在一期只允许与 required 组合。目标平台无法完成并验证所需备份时，不得审核通过 irreversible Release Target。

时间和状态优先级固定为：撤回、暂停、吊销、隔离或安全失败最高，随后是安装有效区间和系统权限，最后才是升级类型交互。`releaseVisibleAt` 之前不展示、不下载；从 releaseVisibleAt 起，normal 在用户点击前只展示提示，用户点击后允许下载和校验但等待 installNotBefore 才能请求安装授权，silent 自动下载并预安装，forced 可以预下载但不能提前启用强制升级门。`installNotBefore` 之前三种类型均不能切换活动版本或消费安装/激活授权；达到该时间后 normal 才可继续安装、silent 可在重启时申请激活、forced 在当前实际 hop 也可立即安装时启用强制升级门。达到非空 `installNotAfter` 后，停止新的下载、预安装、安装和激活资格，并严格按 6.2 的四类到期矩阵处理：executionCommitBoundary 前取消并保持旧版或解除门，边界后继续保持升级或人工修复门；尚未激活的 silent 预安装内容失效并清理。

后台在提交审核前必须拒绝永远不可执行的配置：`installNotAfter` 非空时必须同时满足 `installNotBefore < installNotAfter` 和 `releaseVisibleAt < installNotAfter`。silent 还必须验证从 releaseVisibleAt 到 installNotAfter 之间存在可完成预安装的有效区间，并且 `stagedValidUntil` 不晚于 installNotAfter。forced 的阻断起点固定为 installNotBefore，不能另配更早时间，并且 forced.releaseVisibleAt 必须不晚于 forced.installNotBefore；对 supportFloor 及当前来源事实登记范围内的每个受支持来源，路径中的全部实际 hop（ordinary 与 bridge）必须支持 directInstall，且同时满足 `hop.releaseVisibleAt <= forced.installNotBefore`、`hop.installNotBefore <= forced.installNotBefore`，并且 hop.installNotAfter 为空或不早于 forced.installNotAfter；forced.installNotAfter 为空时所有实际 hop.installNotAfter 也必须为空。任一受支持来源不满足时拒绝 forced Deployment 激活，不能只排除该来源后继续发布。

一期支持按预先审批的阶段计划自动扩量。Rollout 启动时使用计划的第一阶段百分比；某阶段只有在其最短观察时长结束、合格样本达到该阶段最小样本量，且下载失败率、校验失败率、安装失败率和健康失败率均低于所配置阈值时，才由系统自动进入紧邻的下一阶段。条件不足时保持当前百分比，不按日历时间跳级。首期不提供人工推进阶段的入口。运行中实际百分比只能按已审批计划单调增加，不允许降低、跳级或临时改写；阶段百分比、SN 包含组和 SN 排除组在 Rollout 启动后均不可修改，冻结或正常运行期间都不能通过扩大包含组、缩小排除组或人工推进来扩大资格。需要缩小资格时必须暂停或终止 Rollout；需要改变阶段序列或 SN 组时，必须终止当前投放并创建新的已审批 Deployment/Rollout 修订。进入 100% 后继续按该最终阶段配置观察，只有最终阶段的时长、样本和全部质量阈值均满足且无扩量冻结时才可申请 completed；达到 100% 本身不自动成为基线。

任一自动观察阈值达到时，只把候选 Rollout 标记为“停止自动扩量”并告警，不改变 Deployment/Rollout 生命周期，也不撤销当前百分比下已命中的资格。该标记锁存，指标自行恢复不会自动清除；冻结期间禁止自动或人工提高实际百分比。每项冻结原因必须绑定实际触发它的原阶段、原质量分组、指标、阈值、评估时间及证据快照，前一阶段迟到失败触发的冻结也不能改记为当前阶段。有权限的操作员按 8.2.1 确认当前阶段及全部尚未解除冻结原因的原阶段/分组均通过完整窗口复核、当前没有暂停/撤回/吊销/隔离状态后，才能执行带原因和审计的“恢复自动扩量”；恢复时保持当前百分比，并从恢复提交时间重新观察当前阶段及此次恢复关联的原问题分组。操作员也可复核后选择 Rollout paused（停止候选新资格）或 Deployment paused（停止该投放新的下载以及预安装、安装和激活资格）。

### 8.2.1 自动扩量的质量口径

每个阶段必须审批非零最小样本量、最短观察时长和各项失败率阈值；阈值必须大于 0 且不大于 100%。正常阶段观察窗口从该阶段生效提交时刻开始，到本次评估的服务端可信时间结束；用于本阶段判定的升级流程须在该阶段首次取得资格，后续事件和对账仍归属原阶段，不把前一阶段的成功移入新阶段。暂停后的投放资格恢复按 6.2 执行，不以尚无法收集的质量样本作为恢复当前百分比资格的前置条件；投放恢复后的重新观察、解除扩量冻结的恢复复核及其后重新观察，按本节的独立窗口执行，不要求已经升级的设备重新下载或重复安装。

解除扩量冻结的恢复复核由有权限的操作员在投放资格有效时发起，必须覆盖当前阶段和全部尚未解除冻结原因所关联的原阶段/质量分组；单纯暂停且没有质量冻结时，投放恢复命令自动启动当前阶段的重新观察，无需再由操作员发起解冻。窗口长度至少为这些阶段各自完整最短观察时长的最大值，每个原分组分别使用所属阶段已审批的最小样本、指标和阈值，不能用当前阶段的较宽条件代替历史阶段条件。可以纳入各被复核原阶段已经完成升级设备的合法事务对账，并在该窗口内独立复核其活动版本、受影响数据一致性及 11.8 的本地健康；新发起的合法升级流程仍按首次取得资格的阶段归属，只能贡献该阶段的样本。既有下载、校验和安装证据必须属于同一 Deployment 及其对应原阶段、来源和实际 hop，精确对应原目标版本、targetKey、签名 Package/制品哈希和激活模式，仍通过原凭据和服务端记录校验，不得复制到另一阶段或投放、伪装成新安装。各项既有证据只证明对应指标；既有范围只有原安装/激活结果已合法确认成功、当前活动内容仍与上述原目标身份完全一致且窗口内健康复核通过，才能贡献原候选的安装/激活及健康复核成功样本，下载或校验成功不能替代这些条件。新的健康复核不补签原过期凭据或改写原事务终态。每个 installationScopeId 在同一复核窗口、同一原阶段及同一原分组最多贡献一个样本；跨阶段的独立流程必须各有合法 lineage，不能把一次成功重复归入其他阶段。

独立健康复核使用服务端新建的复核任务和专用短期事件凭据，精确绑定复核窗口、安装范围、关联原事务、原 Deployment/阶段/实际 hop，以及原目标版本、targetKey、签名 Package/制品哈希和激活模式；只允许上报本次复核结果，不授予下载、预安装、安装、激活或旧事务改写能力。任务和结果均需权限校验、去重及审计，不能把普通客户端自报的“当前正常”作为已确认安装结果。活动内容身份不符时必须记录实际身份和不匹配结果，不能把任务目标改为设备现有的更高版本。复核样本集合必须覆盖当前阶段所有已开始流程，以及每个尚未解除冻结原因的原阶段/分组中全部已开始流程，包括已转入其他版本的范围；未完成、未知及失败范围不得通过挑选成功设备排除，已恢复范围需保留原失败和独立成功处置的完整关联。复核期间及恢复提交点新增冻结原因时，须补入对应原分组并满足它的完整窗口条件，不能按旧原因集合直接恢复。

该窗口必须按各原分组逐项达到最小样本、失败率及未知结果上界要求，分组间不合并分母、不相互抵消；窗口内没有新流程不单独构成失败，但只有历史成功且没有窗口内健康复核也不能通过。真实执行失败不可被对账抹去，操作员须记录原因及处置证据。独立合法重试或关联前向修复可以证明设备已恢复健康，并按 11.7 解除明确关联的本机修复门；不同目标版本或制品的成功不能充当原候选的成功样本、增加其成功分母或降低其保守失败率上界。原已确认失败继续计入原候选复核和恢复后观察的失败，仍未知的原执行结果继续计入保守上界，不能用新版本健康或一次新健康检查推定原安装成功。若处置需要修改应用代码、Package 或制品，必须终止原投放并发布更高版本；原投放只能在原不可变内容、合法证据和全部质量条件满足时恢复，例如交付或环境问题已经解决，不能用更高修复版本的结果恢复原候选。全部适用分组通过后才允许操作员恢复，逐项关联冻结原因、复核结果及解除记录，保留原冻结及评估历史；恢复不提高百分比，从恢复提交时刻再完成一个覆盖当前阶段和此次恢复关联原问题分组的完整独立观察窗口，窗口长度及分组条件沿用上述规则。历史原因标记已解除不移除该重新观察窗口中的原分组，使用同样的对账和新健康复核规则，全部通过后系统才可进入紧邻阶段；再次达到阈值时追加冻结原因并重新锁存。

自动判断按 `Deployment + Rollout + 阶段 + targetKey + 来源版本 + 实际 hop + upgradeType` 分组；只统计通过凭据与合法状态校验、并能与服务端资格及授权记录对应的安装范围。每个 installationScopeId 在同一窗口和同一分组最多贡献一个样本。同一流程的断点续传、备用源切换、重复事件和重试不增加样本量；流程在明确终结前的重试只更新该样本，终态失败后重新发起的流程不能覆盖该窗口已经记录的失败。客户端报表和自动判断必须使用相同的计数口径，并独立执行第 16 章的反滥用贡献限制；不得与网络来源隐私立方体联查或共享导出关联键，也不得用其周快照替代阶段质量样本。

| 指标 | 合格分母与成功条件 | 失败及未完成处理 |
|---|---|---|
| 下载失败率 | 已开始下载且取得明确结果的不同安装范围；完整目标文件下载完成为成功 | 只有合法证据确认不可恢复下载错误才计执行失败；持续有可信进度或可确认网络等待/暂停时为未完成，无总时限；失联或无法确认进度为outcome_unknown，不因超过24小时自动失败或冻结 |
| 校验失败率 | 已开始完整文件校验且取得明确结果的不同安装范围；全部容器、最终安装器及平台身份校验通过为成功 | 合法证据确认校验不符或本地校验超时为执行失败；首次开始校验起 30 分钟未收到结果则标记 outcome_unknown |
| 安装失败率 | directInstall 以平台安装器开始后完成安装及活动版本核对为成功；silent 分别统计预安装和激活，预安装以完整槽位验证为成功，激活以迁移及活动版本核对通过为成功 | 合法证据确认对应执行失败或本地执行超时为执行失败；首次开始该执行阶段起 2 小时未收到结果则标记 outcome_unknown；staged 不能计为激活或升级成功 |
| 健康失败率 | 已启动新活动版本健康检查的不同安装范围；仅 11.8 的检查通过为成功 | 合法证据确认检查失败或本地超过 11.8 等待上限为执行失败；超过总等待上限仍未收到结果则标记 outcome_unknown |

每项真实失败率为 `该项已确认执行失败样本数 / 该项已确认成功或执行失败样本数`；分母为零时显示“样本不足”，不能解释为 0% 失败。用户在执行开始前取消、延后重启、资格失效导致的安全取消和尚未激活的 staged 分别展示，不伪装为成功，也不混入执行失败分母。后台分别展示成功、已确认执行失败、未完成及 outcome_unknown 的数量、年龄和最新对账状态；服务器未收到结果不是执行失败证据。

下载是否未知依11.3可确认进度/状态，其他有硬预算阶段在等待上限仍无结果才未知。outcome_unknown在保守失败率上界中按失败贡献，达到阈值可冻结扩量并以“结果未确认”告警；不伪造安装事务终态、不中止已越界事务，也不替代 11.7 的上报收敛。凭据允许补报期内的合法迟到证据可以把质量样本从未知更新为已确认成功或失败；追加对账记录并保留原评估、冻结原因和历史快照，不自动解除冻结。事务已经进入不可改写终态时，迟到证据仅作该事务诊断，但经校验可更新独立的质量投影，不能据此重签授权、重写事务或直接解本机修复门。未知结果不能凭无凭据自报或新健康复核变成成功；超出合法证据窗口仍无法确认时继续作为未知，并阻止不满足保守上界的恢复或扩量。

只有每个已经取得样本的来源版本/实际 hop 分组中，每项必需指标的已确认分母均达到该阶段最小样本量、真实失败率均严格低于阈值且最短观察时长满足，才允许扩量；无实际来源样本的已认证路径仍受发布前可达性审核，不被当作质量通过样本。令 U 为未完成与 outcome_unknown 的不同样本总数，保守失败率上界为 `(已确认执行失败 + U) / (已确认成功 + 已确认执行失败 + U)`；上界未低于阈值时等待，不能仅靠快速成功样本扩量。有可信下载进度（含可确认网络等待/暂停）及其他预算内未完成只阻止本次通过，不单独锁存冻结；失联/进度不可确认的下载未知、其他阶段到上限的未知及已确认失败按阈值冻结。下载超过24小时本身既不改变为未知，也不触发冻结。silent 的预安装、激活和健康检查必须分别达到最小样本量；即使下载和 staged 样本充足，实际激活样本不足时仍保持当前阶段。迟到失败仍按原阶段重新评估，达到阈值时冻结当前 Rollout；迟到成功可降低质量投影中的未知上界，但只有完成独立复核及操作员确认才能恢复。恢复后的新观察可使用本节明确允许的已完成事务对账及窗口内健康复核，不要求重新安装，不能用它们稀释真实失败或跳过下一阶段观察。阶段完成的评估记录不可改写，后续证据、重新评估和处置以追加记录留存。

### 8.3 审核与发布

- Internal/Beta 可采用较轻审核流程，但不能降低制品真实性和平台匹配要求。
- Stable 发布、撤回、安全恢复和关键策略变更需要双人审核。
- 发布前必须展示受支持来源版本和到目标版本的可达路径。
- 发布后不可原地替换 Manifest 或文件；修复必须创建新版本或新发布修订。

### 8.3.1 正式版同步投放

一期后台支持从同一 Release 的当前有效 Stable Deployment 集合发起同步，操作员显式选择 Beta、Internal 中的一个或两个渠道以及平台目标；不预选目标渠道，不因 Stable 发布而自动执行同步。每个所选平台目标必须分别绑定自己的有效 Stable 来源投放，不能借另一个平台的 Stable 发布同步尚未在 Stable 有效投放的制品。

- 提交前展示来源 Release/Release Target、版本和制品摘要，以及各目标渠道的基线、候选、支持下界、可达升级路径和现有同版本投放。同步版本不高于目标渠道当前基线版本时，不创建该渠道的新投放并提示无需同步，操作员须调整所选集合后提交；尚未激活的渠道不适用此基线比较。当前为相同或更高版本的用户不会因同步得到该版本更新；已有更高候选时仍按目标渠道原有选择规则处理，同步不能覆盖其候选或基线。
- 复用不可变的 Release、Release Target、Package 和 Artifact，不重新上传或替换文件；为每个所选 `channel + targetKey` 创建独立 Deployment 草稿及关联 Rollout 草稿，并记录该 targetKey 对应的来源 Stable Deployment ID/revision 和同步批次。
- 来源策略只作为待确认配置参考。操作员必须逐渠道确认 upgradeType、安装时间窗、灰度计划、SN 组和负责人；源渠道的审批、SupplyChainApproval、BridgeEligibility、基线身份、实际灰度进度和质量观察结果均不复制为目标渠道资格。目标渠道 supportFloor、来源事实和路径要求独立校验。
- 同步提交只创建草稿，不直接产生展示、下载、预安装、安装或激活资格。每个目标渠道按自己的审批流程取得 SupplyChainApproval 并审核、排期、激活；未激活渠道走首次激活流程，已激活渠道走既有候选和基线切换流程，不能直接替换基线。目标渠道基线在草稿创建后推进至相同或更高版本时，后续审核、激活或基线切换必须拒绝该同步投放继续生效，提示无需同步，不能降低渠道基线。
- 创建所选草稿及批次记录全成或全败；重复提交同一请求不会产生重复草稿，同一目标已有同版本投放时展示现有对象并要求处理冲突，不能静默覆盖或重复创建。创建之后各渠道独立审批和投放，后台逐渠道展示草稿、待审核、生效、失败及原因，并允许重试失败步骤而不重复已成功步骤。
- 某个目标渠道审核未通过、权限不足、排期冲突或灰度暂停，不影响 Stable 及其他渠道的独立投放。来源 Stable Deployment 后续暂停或撤回也不自动暂停或撤回同步得到的投放；如需跨渠道处理，必须显式选择目标投放并执行受审计的操作。共享 Release Target 的吊销或 Artifact 隔离仍阻止所有引用它的渠道取得新资格。
- 审计记录操作者、原因、确认的目标渠道和配置、来源身份及修订、生成的各 Deployment/Rollout、渠道审批和逐渠道执行结果；不能把“草稿已创建”展示为“全部渠道已发布”。

### 8.4 运维与观察

后台至少展示：

- 检查量、有更新量、无更新原因。
- 各渠道、平台、来源版本和目标版本分布。
- 下载成功率、校验失败率、安装开始率、安装成功率、健康失败率。
- 灰度覆盖、SN 缺失比例和不同文件源表现。
- 当前暂停、撤回、隔离和人工修复事务。

普通运营的“发布质量报表”首期只提供固定、不重叠的 UTC 自然周快照，完整叶子维度固定为 `(platform family, targetKey, channel, sourceVersion, targetVersion, upgradeType, fileSourceClass)`。每个安装范围在一个 UTC 周只以该周首个合法升级流程归入一个叶子；fileSourceClass 的归因规则在发布前固定，流程未切换源时为批准的文件源分类，发生切换时归入固定 multi_source 分类，归类在快照发布前确定后不再修改，不能把同一范围拆入多个文件源分组。只有通过去重和反滥用校验、归类已完整确定的范围才贡献可见样本；不能在两个叶子重复计数，也不能跨周公布可关联的范围标识。

周结束后 7 天发布唯一冻结快照，单个叶子至少包含 20 个合格安装范围才显示聚合结果，不足时只显示“样本不足”，不返回人数、近似值或上下界。发布前的滚动计数和可比较的快照修订不向普通运营开放；迟到对账只进入受控诊断及内部质量判断，不重写已公开快照。界面可以分页或选择完整叶子，但不得提供总计、部分维度汇总、任意或相邻时间窗、临时过滤、合并单元及下钻；不得以未知查询形状生成新统计。人数和各项成功/失败结果使用同一固定归类与抑制规则，不能单独查询隐藏类别并通过相减得到结果。

普通运营导出只复制同一冻结快照的可见完整叶子，不添加合计、隐藏单元或可关联键；界面也不补算合计。报表不包含 IP、网络前缀或 ASN，不与第 16 章网络来源隐私立方体联查或相互补算。8.2.1 的即时阶段质量计算和恢复复核由内部自动化及受控故障处置角色访问，不作为第二套可由普通运营任意查询的统计面；普通运营可以查看发布状态、冻结/恢复原因和上述周报。受控诊断查询与导出另按最小权限及审计处理。多层汇总、滚动报表或其他查询形状不在一期范围，后续启用前必须完成互补抑制和跨结果隐私评审。

客户端遥测只能作为质量判断证据之一，不能仅凭可伪造的客户端事件自动执行安全撤回或永久暂停。

## 9. 更新选择规则

服务端按以下顺序产生唯一结果：

1. 校验请求 Schema、产品、平台目标和协议版本。
2. 匹配 `product + component + channel + targetKey` 的 `supportFloorVersion`；低于下界时直接返回人工升级入口。
3. 只保留具备展示/下载资格的正向有效链：服务端时间已达到 releaseVisibleAt 且尚未达到非空 installNotAfter，期望终点必须是 Deployment active、Release closed、Release Target approved、全部 Artifact valid；候选终点还必须有 running Rollout。ordinary 实际跳满足相同正向实体条件；bridge 实际跳仅把 Deployment 条件替换为 superseded 且 BridgeEligibility enabled。`installNotBefore` 不阻止返回更新、展示或 normal/forced 预下载，也不阻止 silent 取得仅写非活动槽的预安装授权；它只阻止 normal/forced 安装授权以及 silent 激活授权的取得、消费和执行。平台不兼容项被排除，任何其他状态都不能依靠“未列入排除表”进入选择。
4. 只读取当前渠道的唯一基线指针，以及该渠道父 Deployment 为 active 的 running Rollout 候选；不读取其他渠道作为回退。同步正式版只有其当前渠道 Deployment 生效后才参与该渠道选择，仍须目标版本严格高于当前版本并满足灰度及路径规则。
5. 使用硬件 SN 包含/排除组和当前实际百分比判断是否命中候选；SN 只影响候选灰度。规则按“排除组 > 包含组 > 百分比分桶”执行：同一 SN 同时位于两组时按排除处理；仅位于排除组时不命中，仅位于包含组时命中；其余可用 SN 按稳定分桶结果与当前实际百分比比较。SN 缺失、不可读取或用户关闭发送时，实际百分比低于 100% 固定不命中候选，达到 100% 固定命中候选。Rollout completed 后该 Deployment 已成为基线，上述灰度规则不再适用，所有设备按基线路径选择。
6. 确定本设备的期望终点：命中候选则为候选，否则为当前基线。
7. 检查当前来源版本、升级助手能力、数据迁移能力以及直接边或桥接边资格。
8. 返回能够继续通往期望终点的最高可用下一跳；一次响应只返回一个目标版本。

选择结果必须区分两个正交维度：`endpointKind=baseline|candidate` 表示本轮路径的期望终点，`hopKind=ordinary|bridge` 表示本次实际下一跳是否需要 BridgeEligibility。ordinary 仅表示“非历史桥接”，实际跳可以是已批准的基线路径前置跳而不等于期望终点；bridge 表示 superseded 历史跳。每次决定分别绑定期望终点实体链和实际跳实体链，candidate 终点始终受 Rollout 当前资格约束，bridge 跳同时受 BridgeEligibility 当前资格约束，因此必须处理 baseline-ordinary、baseline-bridge、candidate-ordinary 和 candidate-bridge 四种组合。

ordinary 前置跳不能仅凭其 Deployment=active 就取得资格。实际跳等于期望终点时使用该终点的完整基线或候选资格；不等于终点时，只能来自当前渠道有效基线，或当前 Target/Snapshot 中明确登记且经独立路径审批的 active 前置跳，绑定该路径审批修订。该跳自身关联的 Rollout=paused/aborted 时禁止作为 ordinary 前置跳，新下载及三类授权均须拒绝；不能借另一有效终点的 Rollout 绕过它的暂停或终止。没有独立前置路径审批的其他 active Deployment、未命中候选以及仅“制品已批准”的版本也不能被当作普通路径中间跳。前置路径资格及修订变化纳入 PublishSelectionChange，决定、下载和授权绑定并复核期望终点与实际跳两条链；失效按 6.2 矩阵执行，不扩大该跳原候选的灰度资格。superseded 跳仍只按 9.2 的独立 BridgeEligibility 执行，不用 ordinary 前置资格替代。

用户可观察的 `upgradeType` 由期望终点 Deployment 决定，并在每次下一跳决定中保持绑定；实际 hop 历史上使用过何种类型不改变本次交互。服务端返回决定前必须验证当前 hop 及完整可达路径的激活模式：normal/forced 的每个 hop 均须具备 directInstall 认证，silent 的每个 hop 均须具备 stagedRestart 认证。任一 hop 不兼容时，该来源没有对应类型的自动路径，返回无兼容更新和人工升级入口，客户端不得跨模式执行或擅自改为另一类型。forced 还须验证期望终点和所有实际 hop（ordinary 与 bridge）均已在 forced.installNotBefore 前可见、所有 hop 的安装有效区间覆盖期望终点有效区间，并且当前 hop 此刻可安装，才在在线决定中返回 `forceGateEligible=true`；客户端已经验证的未来 forced 决定离线跨越生效边界时，按 5.5 进入联网复核受限门，而不是把该字段解释为离线放行。normal 和 silent 每次只处理一个实际 hop，成功后重新检查；normal 必须再次取得用户确认，silent 可以按新决定自动处理下一跳，但两者都不能预先串联或一次安装多个版本。

### 9.1 minimumSourceVersion

- `minimumSourceVersion` 只能表达连续下界，不支持排除单个坏版本、定向降级或离散来源集合。
- 缺省值为 `0.0.0`。
- 若原始 `currentVersion < 某 Release Target.minimumSourceVersion`，该目标不能直接返回。
- 服务端应在当前 Snapshot 可达且仍安全有效的 Deployment 中，选择满足 `currentVersion >= minimumSourceVersion`、版本号最高且能够继续通往期望终点的版本作为下一跳。
- 客户端安装下一跳后必须重新检查，不能在一次安装事务中静默执行完整升级链。
- 如果不存在可用下一跳，则返回无兼容更新并提供固定人工升级入口。

### 9.2 历史桥接资格

`superseded` 仅表示某 Deployment 不再是渠道当前终点，不等于该制品被安全吊销。历史 Deployment 只有同时满足以下条件才能取得新的桥接下载以及预安装、安装或激活授权：

- 仍被当前可信 Snapshot 明确列为可达对象。
- 其 Release=closed、Release Target=approved、全部 Artifact=valid；bridge 只把 Deployment 的 active 条件替换为 superseded+BridgeEligibility enabled，不放宽其他正向条件。
- 曾在同一渠道达到 100% 或成为过该渠道基线。
- 经过独立路径影响评估和人工审批，当前 `BridgeEligibility=enabled`。
- 位于从当前来源通往当前期望终点的完整可验证路径上。

未达到 100% 的历史灰度版本默认不能成为桥接；桥接资格不能跨渠道复用。资格被禁用后需要新的审批修订才能重新启用。桥接只允许向上安装，不能作为回滚目标。

BridgeEligibility 使用独立 ID 和单调 revision。启用、禁用或重新启用都必须创建更高 revision，并通过 PublishSelectionChange 发布该渠道的新 generation 与 current 元数据；紧急禁用可先 deny，但 metadata_sync_pending 期间失败关闭。决定、下载资格以及预安装、安装和激活授权均绑定该 ID/revision，相应授权签发与消费前必须在线复核，防止禁用后或禁用再启用的 ABA 复用。

### 9.3 无更新结果

无更新原因采用稳定枚举，至少包括：

- 已是最新版本。
- 当前版本高于该渠道版本，且不允许降级。
- 当前来源没有兼容升级路径。
- 当前渠道没有安全可用的基线。
- 尚未达到 releaseVisibleAt，或已经达到 installNotAfter（`deployment_window_closed`）；客户端重新检查可用后继版本，没有后继时显示固定人工入口。处于 releaseVisibleAt 与 installNotBefore 之间不属于无更新，仍可展示和预下载。

不得向未命中候选的用户泄露候选版本或 SN 名单细节。

公共 Target 只公布不可枚举的 `rolloutSetCommitment`。命中候选时，决定同时返回候选 Release 封装摘要、随机开封盐和承诺开封证明；客户端必须用当前签名 Target 中的承诺自行验证成员关系，再接受候选 Release。在线决定指向未被当前承诺授权的合法历史 Release 也必须拒绝。

候选从不可见变为 running、暂停、恢复、终止、完成，或其策略/引用 revision 改变时，必须通过 PublishSelectionChange 生成新的随机开封盐和 current Timestamp/Snapshot/Target、递增 selectionGeneration；旧 opening 不得对新 Target 继续有效。

## 10. 控制面与文件面

### 10.1 控制面

客户端直接访问 `https://update.pinvou.com`，至少包含：

- 更新检查。
- 获取指定 packageId 的文件信息。
- 决定刷新。
- 安装前复核和带用途的短期授权：normal/forced 使用安装用途，silent 分别使用非活动槽预安装用途和重启激活用途。
- 预安装、安装或激活授权的消费或取消；每次请求必须绑定授权用途，不能跨用途消费。
- 查询所有预安装、安装和激活场景下授权消费的最小结果，覆盖 backupPolicy=required/notRequired，只返回 consumed/cancelled/expired 与 transactionId，不返回或续签授权；长期查询及窗口关闭后的受控对账处置按 11.6.1 执行。
- 事件上报。

接口的字段、签名和错误细节见安全要求及技术设计。

### 10.2 文件面

文件信息接口返回实际下载 URL、文件大小、SHA-256、packageId 和有效期。URL 可以属于第三方文件服务。

- 客户端不得根据控制面域名拼接下载地址。
- 下载 URL 只能用于取得字节，不能改变 packageId、文件大小、哈希或平台身份。
- URL 失效时客户端重新请求文件信息，不重新使用过期地址。
- 文件服务故障时后台可以切换供应商，但不得替换同 packageId 的内容。

## 11. 客户端升级流程

### 11.1 检查

受控升级助手收集产品版本、平台目标、渠道、升级协议和本机 updater 事实。硬件 SN 可按用户隐私设置携带，只用于灰度。

客户端验证检查结果属于本次请求、平台和当前安装；不能接受另一设备、另一渠道或另一检查的结果。

- normal 支持用户主动检查，也支持客户端定期检查后只提示用户。
- silent 由客户端定期自动检查，命中后在网络和本机资源条件允许时继续自动下载和预安装。
- forced 在应用启动及运行期间定期检查；命中但尚未达到 installNotBefore 时只提示或预下载，达到后进入强制升级门。

### 11.2 展示与确认

- 展示目标版本、更新说明、下载大小、重启要求、migrationMode、backupPolicy 和对应数据风险；irreversible 必须明确提示“升级后旧版不能安全读取已迁移数据，备份仅供人工修复，不会自动回滚”。
- normal 提示提供“升级”“稍后”和关闭入口；用户未点击“升级”时不得下载或申请安装权限。
- silent 在后台检查、下载和预安装过程中不得用模态窗口打断用户；预安装成功后必须持续提供明确的“重启后完成升级”提示以及“立即重启”“稍后”入口。涉及数据迁移或 required 备份时，重启提示必须同时展示相应风险和预计耗时。
- silent 不因本机执行权限缺失弹出 UAC、管理员密码或其他提权窗口；展示升级组件修复入口，未完成修复前继续使用原活动版本。服务端激活授权由 helper 自动请求，不展示为需要用户批准的“权限申请”。已有强制或人工修复门的安装范围仍遵守相应使用限制。
- forced 到达可安装时间后显示不可关闭的升级门和当前进度；除安全保存当前未保存内容、升级/修复、网络设置及退出外，不提供进入普通功能的入口。权限被拒绝或发生错误时仍停留在该门内，并提供可执行的重试或人工修复说明。
- Internal/Beta 用户持续看到渠道风险标识，并可主动切回 Stable；切回不触发降级。

### 11.3 下载与校验

- 支持断点续传和重新获取下载地址。下载不设置总时长、累计重试时间或固定24小时自动终止上限；网络缓慢、长时间下载、休眠或重启本身不构成下载失败。
- 单次连接/无响应超时用于重试、退避、换源或显示网络设置/暂停/继续入口，不是整个下载的硬截止；下载不冻结业务数据、不长期持有升级准备锁，forced已有门仍遵守门规则。只有明确不可恢复的下载错误、用户取消或当前资格失效才按对应事实终结/改选，不能仅凭超过时长伪造失败。
- 决定、URL和遥测凭据保留原短期有效期。到期重新在线检查并取得新资格/新会话，精确相同scope/渠道修订、来源、实际目标/Package/模式/类型可续接已确认流程并保留下载缓存，normal不重复询问同一实际hop；改选新hop须新流程及适用确认。原会话终态不复活，不补签旧事件，续接只记录新会话内的未来动作/结果；同一逻辑流程保留首次资格阶段/分组并去重，不能用会话轮换移动样本。缓存身份不符隔离，资格到期/撤销仍不能据缓存越界。
- normal 只有用户明确发起后才开始下载；silent 自动下载；forced 可在 releaseVisibleAt 后预下载，并在强制升级门内自动继续下载。
- 验证可信 Package Manifest、实际下载的子包容器及最终安装器；客户端不下载或校验后台外层复合上传包。
- 校验失败必须删除或隔离不可信文件，不能进入安装。
- 首期始终选择完整包；增量候选不支持或不可用时必须明确记录回退原因。
- 下载进度/网络等待/用户暂停按合法凭据、单调序号及helper缓存事实确认；无凭据UI进度不充质量证据。每个已审批阶段给出独立非零、有限的进度上报/失联判定间隔，属于可观测性规则而非下载终止期限，运行中不得随意放宽。仅无有效进度/状态确认超过该间隔时为未知，客户端仍可续传并重新对账；新可信状态/合法完成证据可更新独立投影，冻结锁存按8.2.1复核解除。不能用同一阶段开始时刻、24小时会话寿命或一次连接超时推定下载失败/未知。

### 11.4 本地预检

安装前至少检查：

- 磁盘空间、安装目录权限和系统安装器可用性。
- 未保存任务、运行中进程、包管理器锁和重启要求。
- 当前实际版本、安装范围和渠道是否仍与检查结果一致。
- 升级助手和 launcher 是否可信且满足目标要求。
- 数据迁移能力是否满足 migrationMode，备份能力是否满足独立的 backupPolicy；不得用“不可逆”隐含跳过备份检查。
- silent 还必须检查非活动版本槽、活动版本安全保留、预安装空间、重启激活和失效内容清理能力；任一项不满足时不得把该更新按 silent 继续处理或降级解释为 normal。

### 11.4.1 per-machine 多用户协调

per-machine 安装的升级影响本机全部用户共用的应用文件和入口，不能仅以发起用户的进程已经退出作为安全条件。每个 Release Target 必须明确声明受迁移影响的共享数据和用户数据范围、全部可能写入者，以及尚未使用新版本的用户首次启动行为；不得把未登录用户的未迁移数据留给新版本无条件读取。irreversible 发布必须证明全部受影响数据均可发现、可冻结、可备份并可完成迁移；任一用户范围不可访问或无法验证时不得开始受保护写入。

- silent 预安装不停止其他用户的旧版进程，不迁移任何活动用户数据。用户 A 选择“立即重启”只表达 A 的重启意愿，不授权丢弃用户 B 的未保存内容、退出 B 的系统会话或强制停止 B 的业务进程。
- normal/forced 的安装准备和 silent 的激活准备均须在协调写入者、冻结数据或创建备份之前取得本安装范围的独占准备所有权和系统级产品锁；per-user 安装也遵守这一顺序。其他启动入口、用户会话或 helper 必须等待或退出，不能先冻结或备份、再竞争授权消费。准备所有权、冻结代次及恢复记录须受保护地持久化，从准备开始连续保有互斥，经授权消费转交给唯一执行事务，直到确认安全取消、执行安全收敛或按 11.6.1 完成本地永久停用；消费结果未知时仍保留受保护的对账占用，不能放行另一升级准备流程，但满足本地停用条件后可以解除业务冻结。崩溃或重启后，恢复入口先依据该记录重建互斥并确认唯一所有者，不能把进程消失视为准备已取消。取消或本地停用只能解除自己仍拥有的冻结代次，不能解冻其他准备或已接管事务的数据；所有权转交后，旧所有者的写入、解冻和清理请求全部失效。
- normal/forced 调用安装器前、silent 重启激活前，取得上述独占所有权的 helper 必须协调本安装范围内所有用户的应用实例、后台任务和其他登记写入者：安全保存可保存状态，阻止新写入和新的旧版实例启动，并确认已有写入者均停止或进入可验证的冻结状态。系统级产品锁只保证升级互斥，不能替代这一确认。
- 数据快照必须覆盖此次迁移影响的全部已声明范围，并与冻结状态连续保持一致；休眠、断开的登录会话或未登录用户不豁免其受影响数据。无法保存、冻结、读取或验证任一必需范围时，normal/尚未越界的 silent 按所有权规则确认安全取消，释放自己的冻结与准备占用并保留原活动版本；silent 不自动请求提权。forced 保留受限门，允许有关用户完成安全保存、协调重试或退出，不能以超时为由丢弃数据或绕过门。
- 在安装/激活与健康检查完成前，其他用户只能看到升级状态及适用的受限入口，不得从另一个会话启动旧版绕过冻结或修复门。事务 succeeded 后解除升级冻结，各用户下一次启动进入经过验证的新活动版本；11.6.1 已证明未越界并永久停用旧尝试时，可按其规则恢复原活动版本的本地使用，同时保留升级对账占用。需要人工修复时所有相关用户均被同一安装范围的修复门保护。

### 11.4.2 本地等待与执行上限

每个 targetKey 的能力认证必须给出协调/冻结及备份准备、预安装、直接安装和激活的非零、有限本地执行预算，发布审核核对这些预算及超时处置已通过真实系统验证，不接受无限等待或仅以“进程仍存在”延长预算。准备不得晚于本次有效遥测会话期限（长下载结束后先按11.3取得当前会话）；预安装、直接安装和激活各自执行预算不得超过2小时，完整校验本地总预算不得超过30分钟，健康检查按11.8。下载不适用本节有限执行预算，其请求超时和续接按11.3。有硬预算的阶段从首次实际开始计算，重试、用户切换、helper崩溃和系统重启不重置；下载续传/换源不设置累计时间上限，已开始完整校验的同一内容预算不能借新会话刷新；权限确认等待须在冻结前进行并明确展示，不把等待用户确认伪装成执行进度。预算耗尽或无法证明剩余预算时，立即展示稳定超时/修复原因及可执行入口。

边界前只有确认本次写入者已安全停止、未越界且原活动版本及数据完整时，才可安全取消并按所有权规则解冻；消费结果未知还须按 11.6.1 保留对账占用。预安装超时隔离或清理非活动槽，不建立应用修复门。已经跨 executionCommitBoundary 的安装/激活超时进入 failed_manual_repair_required，持久化同一范围的修复门；若系统安装器或迁移写入者仍在运行，helper 继续安全接管，只提供状态、诊断、安全退出及适用的修复入口，不强杀系统包管理器、不释放数据冻结、不允许并发修复安装，直到确认写入者停止并完成安全核对。不能因为超时后安装器晚到退出码为成功而改写已提交失败终态或自动解除修复门。8.2.1 的服务端“未收到结果”仍仅是 outcome_unknown；本地超时必须有合法事实及事件才能计为执行失败。

### 11.5 数据备份

发布必须分别声明 `migrationMode=none|backwardCompatible|irreversible` 和 `backupPolicy=notRequired|required`。irreversible 必须配 required；备份不会触发应用或数据的自动回滚。

- backupPolicy=required 时，备份必须在获得系统权限、取得独占准备所有权并冻结相关数据写入后完成且验证成功，且绑定本次准备及冻结代次；备份能力缺失、备份失败或验证失败都阻止 normal/forced 安装授权或 silent 激活授权，但不要求 silent 在只写非活动版本槽的预安装阶段提前迁移或备份活动数据。任何类型都不允许用户或运营绕过。backupPolicy=notRequired 时不创建快照或伪造 backup_succeeded 阶段。
- 快照不自动上传，不作为自动回滚机制，只供人工修复使用。
- 活动授权或安装事务期间，普通用户不能删除快照。
- succeeded：快照至少保留至终态后 7 天，正常自动清理在第 7 天执行。
- abandoned_before_install：确认未发生受保护写入后保留 24 小时用于诊断，随后自动清理；不把它当作人工修复备份。
- failed_manual_repair_required：快照保留至终态后 30 天供人工修复，并在第 30 天执行自动清理。
- 终态后管理员提前删除必须明确确认“将失去人工修复备份且不可恢复”，并写审计记录。
- 只要会话已经 backup_succeeded 但尚未 consume，之后无论进入 authorization_failed、coordination_aborted、channel_changed、cancelled_before_install、authorization_expired、cancelled 或 expired 终态，确认未发生受保护安装写入后都保留快照 24 小时用于诊断，随后自动清理。
- 首个备份字节写入前，helper 必须在系统级受保护本地事务中先登记 staging 路径、密钥引用和保守 `localCleanupAt=sessionExpiresAt+24h`；该记录由 launcher/helper 在启动、重启和离线状态下扫描执行。consume 成功后原子切换为安装事务留存类别；部分写入、backup_failed 或创建中崩溃必须清理 staging 和孤立密钥引用。服务端终态可同步调整期限，但自动清理不依赖服务端直接访问本机或设备再次联网。
- 所有用途的 consume_pending 记录、恢复凭据和查询期限统一按 11.6.1 执行，不因 backupPolicy=notRequired 而省略。required 场景的备份同时绑定该记录；pendingCleanupAt 边界后，本地运行中 10 分钟内或关机后下次启动且联网前删除平台快照、备份 staging 和密钥引用。本地已停用执行但服务端尚未确认时，备份仍按原 consume_pending 期限管理，不能假定 consume 未成功而改用 24 小时清理；完成权威对账后再进入相应留存类别。用户主动导出副本使用独立路径和审计，不能静默延长平台记录。

- consume 已确认 consumed 时，本地记录必须原子进入 `transaction_retention`，保存服务端 transactionStartedAt 并固定 `hardCleanupAt=transactionStartedAt+60d`。可信期限能力正常时，即使设备此后永久离线，也按同一 10 分钟/启动且联网前规则在硬上限删除平台快照、staging 和密钥引用；可信期限能力损坏时仅适用 17.1 明确告知、审计的加密保留例外；较早取得可信 succeeded、abandoned_before_install、failed_manual_repair_required 终态时，分别使用 `min(hardCleanupAt, terminalAt+7d|1d|30d)`。

### 11.6 安装授权与安装

normal 在用户明确发起且完成权限、预检、独占准备所有权下的写入者协调、冻结和 required 备份后请求短期安装授权。forced 在强制升级门内自动推进，但仍须取得必要系统权限并按相同顺序完成准备；用户拒绝权限不会解锁普通功能。两者均按 11.4.1 从准备阶段保持互斥，消费成功时把准备所有权及冻结原子转交给对应唯一执行事务，不得在两者之间释放产品锁；消费结果未知时保持受保护的待确认记录并按 11.6.1 对账或本地停用。两者授权前，服务端重新检查发布、制品、渠道、来源资格和 `[installNotBefore, installNotAfter)` 安装区间。

silent 使用两个明确阶段：

1. **预安装**：下载和校验完成后请求并消费仅允许写入指定非活动版本槽的短期预安装授权，创建独立预安装事务；升级助手释放全部目标文件并验证目标可启动性，不得改变活动 launcher 指针或迁移活动数据。完整槽位验证成功后，原子登记 staged 记录及初始 revision、把预安装事务置为不可改写的 `staging_completed` 终态，并释放系统级产品锁和活跃事务占用。staged 记录的 `staged_waiting_restart` 是待激活内容状态，不是仍占用执行权的安装事务；该记录绑定预安装事务、安装范围、渠道修订、Deployment/实际 hop、package/槽位身份、stagedAt 及 stagedValidUntil，不授予激活权。失败或安全取消分别进入 `staging_failed` 或 `staging_cancelled` 终态，清理不完整槽位并释放占用；任何预安装结果均不计为应用升级 succeeded，不产生人工修复门。
2. **重启激活**：用户重启时先在线重新检查发布资格并确认仍精确选择已 staged 的下一跳，再验证 helper 的既有本机执行权限，取得 11.4.1 的独占准备所有权和产品锁后，才协调全部写入者、冻结数据，并完成且验证 required 备份；不需要备份时也必须完成适用的预检和冻结。只有前置条件全部满足后，才自动请求短期服务端激活授权并消费；备份失败或验证失败不得取得该授权。资格复核与本地准备不是激活授权，不能据此提前迁移或切换。授权签发失败或消费被权威确认拒绝且未发生受保护写入时，只解除本所有者的冻结，按规则保留或清理备份和槽位，释放准备占用并安全启动原活动版本；消费或取消结果未知时先对账，不把超时当作拒绝或安全取消。存在先前强制或人工修复门时仍保留该门。消费成功后连续保持锁与冻结，在 executionCommitBoundary 的紧前一步按 5.5 再次在线复核当前资格和授权，才开始迁移及切换。在边界前，前置条件失败可以安全取消；首次活动数据写入或 launcher 指针切换任一先发生即跨越边界。边界后的任一失败，尤其是 irreversible 迁移部分写入、迁移后指针切换失败或无法证明数据仍处于一致状态，都必须持久化禁止旧版启动的修复门并进入 `failed_manual_repair_required`。只有迁移成功且全部条件满足后才能启动新版本并按 11.8 进行健康检查。

预安装授权不能用于激活，激活授权也不能写入未绑定的版本槽。silent 预安装完成只上报 staged，不得上报安装成功；成功只以激活后实际版本核对和健康检查通过为准。

短期服务端激活授权及其消费请求必须精确绑定 staged 记录的 revision、槽位身份和原预安装事务；消费时比较该 revision 仍为 staged_waiting_restart，并原子创建新的唯一激活事务、将该 revision 转为 activating、取得该安装范围的活跃事务占用。新激活事务使用不同于预安装事务的 transactionId，本地将独占准备所有权和冻结连续转交给该事务。其他准备或事务占用时不得并发激活；消费提交但响应丢失时仍由原准备所有者持有待确认占用，恢复后只对账同一次请求。staged 记录的该 revision 进入 activating 后不能被第二次消费、普通清理或新预安装替换；激活成功、安全取消或失败均按相应执行边界收敛，再按记录状态保留或清理槽位。已完成预安装事务不等待重启、不等待激活结果，也不因 staged 过期、暂停、撤回或用户延后重启改写终态；这些事件只改变待激活记录并取消资格或清理，不属于安装失败。预安装事务和激活事务使用各自凭据及事件 lineage，不能串用。

已消费激活的安全取消只有在服务端权威确认原激活事务进入 `cancelled_before_install`，且 helper 证明未跨 executionCommitBoundary、未发生受保护写入、原活动版本和 staged 槽位仍完整时，才可结束原 activating revision。原 revision 以安全取消终结并保持不可改写；旧授权和旧事务不能重放。若原 stagedValidUntil 尚未到期、渠道修订未变，且在线复核仍精确选择同一 Deployment/实际 hop 和 Package/槽位，可在独占所有权下原子建立单调递增的新 staged revision，关联原 revision、原预安装事务和被取消的激活事务，恢复为 staged_waiting_restart；继承原 stagedAt 和 stagedValidUntil，不延长留存或激活期限。新 revision 只允许在下一次用户发起的应用重启或系统重启时，重新取得准备所有权、重新冻结并完成本次 required 备份，使用新授权和新 transactionId 尝试激活；不能复用已解冻的旧快照作为本次准备成功依据。建立新 revision 的响应丢失或崩溃须幂等恢复同一结果，不能生成多个可激活记录。仅签发失败或消费已确认未成功、记录仍为 staged_waiting_restart 时无需重建 revision，但再次尝试仍须完整准备及新授权。

消费或安全取消尚未权威确认时，禁止建立新 revision、取得另一激活事务或普通清理占用中的槽位，必须先恢复原所有权并对账；可以按 11.6.1 永久停用本次执行、恢复安全的旧版使用，但仍保留升级对账占用，不能把本地停用冒充服务端 cancelled_before_install。所有权转交后旧 helper 不得继续操作。原内容暂停时只按 5.5 隐藏提示并保留，恢复后满足上述条件才可重建待激活记录；到期、撤销、渠道变化或改选时按 5.5 清理，不能用重建 revision 恢复资格。已经停用且证明无写入者的槽位仍按原 stagedValidUntil 执行到期清理，不因等待对账延长期限。任何已越界失败均进入人工修复流程，不能重新激活原 staged 内容或伪装成安全取消。

授权成功后由外部升级助手：

1. 在准备和执行期间连续持有系统级产品锁及唯一所有权；直接安装和激活从准备阶段继承，预安装在 consume 前建立受保护记录时取得，不得把取得锁延迟到数据冻结或备份之后。只有完成安全收敛或 11.6.1 的本地永久停用后，才按对应规则释放实体产品锁。
2. 如有需要，按当前授权用途安装或接管目标升级助手。
3. normal/forced 调用当前平台安装器；silent 预安装到绑定的非活动版本槽，并在激活阶段切换 launcher 指针。
4. 在主应用退出、用户切换或系统重启后继续恢复事务。
5. 直接安装或激活时核对实际活动版本和平台身份，不能把仅存在于非活动槽的版本视为当前版本；预安装只核对绑定槽位并结束为 staging_completed。
6. 仅直接安装或激活后启动新活动版本并进行健康检查；预安装的隔离可启动性验证不得切换活动版本。

### 11.6.1 消费结果未知与本地收敛

每次预安装、直接安装和激活 consume 都必须具备同等的恢复能力，覆盖 backupPolicy=required/notRequired。客户端发送请求前，在产品锁下建立受保护的 consume_pending 记录，生成 256-bit recovery secret 和规范 consumeRequestDigest，固定 transactionId、authorizationJti、授权用途、安装范围、consume 幂等键、authorizationExp、consumeRequestDigest 及唯一 pendingCleanupAt=authorizationExp+61d；请求只提交 secret 的哈希，不为 notRequired 创建备份或伪造备份事件。服务端以这些绑定字段生成域分离 statusBindingHash，将 consume 业务状态、普通幂等结果和含该哈希的最小 outcome tombstone 原子提交。普通响应恢复期结束后、且仅在 now<pendingCleanupAt 时，consume-status 接受 secret 及全部绑定字段，常量时间校验后仅返回 consumed/cancelled/expired 与 transactionId，不返回任何凭据。任一字段错误、记录不存在或到达 pendingCleanupAt 都返回同一无信息结果；边界后查询关闭并清除本地恢复 secret，tombstone 仅为服务端清理裕量再保留 1 天，期间也不可查询，随后删除。备份内容及解密密钥按 11.5 和 17.1 清理，查询期限不因计时组件故障延长。确认 retention_time_fault 时，应立即清除本地 recovery secret 及其可恢复副本，保留不含秘密的原请求摘要、绑定标识、原期限、执行状态及停用凭据，改走本节独立对账任务；该 secret 不属于备份解密密钥，不适用加密备份保留例外，计时能力修复后也不能重建旧 secret 或恢复旧查询窗口。服务端查询及 tombstone 的原期限保持不变。

消费或取消响应丢失、断网、服务端不可用均不能直接解释为未消费或已取消。等待消费对账最多占用一次 5 分钟的本地等待预算，从首次发送 consume 起计算，重试、崩溃或重启不重置；用户也可以主动停止本次尝试。未取得明确结果时，helper 应在受保护所有权下核对本机事实，只有证明未跨 executionCommitBoundary、没有受保护写入、原活动版本及数据仍完整，并确认全部本次执行者已停止、永久禁止该授权/事务/所有者代次再执行，才能持久化 local_execution_retired 处置。不能证明上述条件时，直接安装/激活维持执行或人工修复保护并提供诊断及修复入口；预安装仅隔离非活动槽、继续安全收敛，不因上报缺失产生应用修复门。时间或预算无法可信判定时也不能继续无上限等待，应立即进入上述本机安全核对。

local_execution_retired 不是服务端消费结果或事务终态。完成该处置后，仅由仍有效的所有者解除自己的业务冻结、使已有的原准备备份失去后续执行用途并释放实体产品锁；受保护的升级对账占用跨重启保留，阻止新准备、预安装、安装及激活。normal/silent 无既有门时可以立即继续使用经验证的原活动版本；forced 及已有独立修复门仍按 5.5 保持或解除，不因断网或本地停用放行。界面以稳定原因 consume_reconciliation_required 展示“旧版可继续使用，升级结果待确认”及联网对账/受控修复入口，适用的门内只展示允许的入口，使用三语资源。任何迟到 ACK、恢复 worker 或旧 helper 均不能重新获得被永久停用的本地执行权；再次升级必须重新冻结、预检和 required 备份。

联网后先恢复同一次 consume 的合法结果，再使用既有合法恢复凭据或独立受限对账任务完成服务端处置；确认未消费时取消/失效旧授权，确认 consumed 时核对关联事务并确认安全取消或既有终态，然后才能释放该范围的对账占用。对账任务只允许核对原请求及其本机执行事实、停用旧资格和提交对应处置，不授予安装、激活、下载或旧事件补签能力。事务已进入 reporting_timeout 等不可改写终态时只追加关联处置，不能改写为取消或成功；可信本地未越界及旧版完整证据可以解除此次对账占用，不额外制造本机修复门，原结果在质量口径中仍按 8.2.1 的合法证据规则处理。激活原事务确实为 cancelled_before_install 时，才可按 11.6 重建 staged revision；原事务已为其他终态时不得复活原记录，旧槽位经安全清理后只能按新决定重新预安装。

查询窗口已关闭、恢复 secret 已清理或原凭据已无法合法恢复时，转入管理后台受控故障处置角色发起的独立对账任务，绑定安装范围、原请求摘要、授权/事务标识和本地停用处置；管理身份须满足 MFA、最小权限及完整审计。由可信 helper 核对原活动版本、受影响数据、无受保护写入及旧执行权永久失效，服务端同时核对该范围占用和旧授权状态，全部满足才以独立处置释放对账占用。历史消费结果缺失时明确保留“原结果未知”，不能伪造原取消/成功或复活已删除凭据；任一安全条件无法证明时保持保护并提供人工诊断及前向修复路径。任务失败或响应丢失须恢复同一处置，不产生重复占用或新执行权；网络恢复不能重置原备份、槽位或查询期限。

### 11.7 失败处理

- normal 在安装开始前失败可以安全取消并重新检查；forced 在同类失败后仍停留于强制升级门并允许重试。
- silent 预安装失败时清理不完整的非活动槽并继续运行原版本；预安装成功但在 executionCommitBoundary 前发生复核、权限、备份等前置步骤失败时，可以按 11.4.1 确认安全取消、只解除本所有者的冻结，继续启动原版本并按 stagedValidUntil 保留或清理。已经消费激活且需要复用原槽位再次尝试时，必须按 11.6 确认原事务安全取消并建立新 staged revision，不能重放旧授权或原 activating 记录；结果未知的本地停用、对账及其他终态后的清理重试按 11.6.1 执行。边界后的迁移部分失败、迁移完成后指针切换失败或其他任何激活失败都禁止启动旧版并进入 `failed_manual_repair_required`；不能以“活动指针尚未切换”为由当作安全取消或自动回滚。
- 安装器启动后不自动安装旧版本。
- normal/forced 在平台安装器启动后，或 silent 激活跨越 executionCommitBoundary 后，若无法确认成功、安装失败或健康检查失败，则进入“需要人工修复”；仅 silent 预安装失败或激活边界前安全取消不进入该终态。
- 当前版本存在问题时，客户端只能安装版本号更高的前向修复版本。
- 原失败事务及其上报超时终态保持不可改写；前向修复采用独立关联事务，按 5.5 完成安全复核、版本及数据一致性核对和健康检查后才解除相关修复门。修复失败继续保持该门，不能仅因原发布失效或新版本号更高就放行。
- 任一事务最终必须进入与用途相符的明确终态：预安装使用 staging_completed/staging_failed/staging_cancelled；直接安装和激活使用 succeeded、安装前放弃或 failed_manual_repair_required。待激活记录不是活跃事务。
- 直接安装或激活 consume 后 30 天仍未取得合法终态事件时，服务端把非终态事务收敛为“需要人工修复（reporting_timeout）”；不能据此重新封锁已本地验证 succeeded，或已按 11.6.1 证明未越界并永久停用的健康旧版安装，后者仍须完成对账才能再次升级。预安装 consume 后 30 天仍无合法终态时，服务端只置 staging_failed(reason=reporting_timeout)，不产生本机人工修复门；客户端未完成槽位安全取消并清理。已本地完成但无法确认服务端结果的槽位禁止激活：只有服务端确认原事务已经提交 staging_completed 且 staged 仍有效才可继续；服务端已置 staging_failed 或已无法在合法窗口内确认时，清理旧槽位，并在按 11.6.1 解除适用对账占用后重新预安装，迟到完成证据不能复活失败事务或旧槽位。已提交 staging_completed 的事务不因之后等待重启或 staged 到期触发此超时。超时后到达的离线证据不改写事务终态，只作事务诊断；独立质量投影按 8.2.1 对账。
- 未consume的每份遥测会话创建后24小时仍非终态按阶段原子收敛（不终止11.3逻辑下载）：authorized 会话进入 authorization_expired，关联 available 授权记录进入 expired；authorization_requested 和更早阶段的会话进入 expired，并 fencing 活动 validate。该超时与 validate 提交、consume、cancel 竞争同一 revision，只有一个结果。7天上传窗口内晚到旧事件只作诊断，不改终态；长下载取得新会话仅续接未来动作，流程/原阶段和样本保持，不能用新凭据补旧事件。

### 11.8 本地健康检查与成功条件

升级成功必须同时满足：安装或激活完成；活动版本、平台身份和入口与本次授权目标一致；必需迁移完成且受影响数据一致；新版本能够进入正常本地使用状态。仅进程存在、文件已经替换、非活动槽可启动或版本号相等都不能单独证明 succeeded。

本地健康检查至少验证新活动进程持续存活、主界面或等价本地就绪入口可响应、当前用户配置和本地数据存储可安全打开、必需本地组件初始化成功，且没有升级或迁移引起的启动循环、数据损坏或致命错误。检查过程不得向模型或外部服务发起付费请求、上传业务数据或修改用户业务内容。其他受影响用户的数据兼容性按 11.4.1 的完整范围核对，不要求每个用户都实际登录后才判定。

首次启动后单次检查最长 120 秒；可在不回滚、不重复迁移且不解除修复门的条件下自动重启新版本并重试一次。从首次启动尝试起，包括两次启动、检查和等待在内，本地总等待上限为 5 分钟；无法启动、明确的数据一致性错误或平台身份错误直接失败，不靠重试掩盖。明确失败或总等待上限到达仍无法证明全部条件通过时，进入 failed_manual_repair_required 并提供诊断、联网设置、前向修复和退出入口。系统关机或 helper 崩溃不重置已经消耗的等待预算；重启后必须核对既有检查结果及剩余预算，无法证明剩余预算时按无法确认成功处理。

模型服务、业务服务器、账号登录或其他外部网络不可用，不单独构成本地健康失败；本地条件通过后可以记录 succeeded，并另行提示外部服务异常。forced 只按安装事务成功或 5.5 的关联前向修复成功解门，不能以远端服务可达性作为无限等待或提前解门的条件。本地已经验证 succeeded 但暂时无法上报时，不阻止该成功安装的本地使用；离线上报及服务端 reporting_timeout 仍按 11.7 执行，后台上报缺失不能倒推本机升级失败或重新封锁已验证成功的安装。

## 12. 升级助手与主应用边界

- 升级助手必须位于不会被主应用安装器原地覆盖的受保护目录。
- launcher 提供稳定入口，选择并启动经过验证的版本化 helper。
- 同一安装范围同一时刻只能有一个有效准备、执行或对账所有者，以及至多一个非终态执行事务；准备所有权从首次协调写入者之前取得，消费后连续转交给唯一执行事务，按 11.4.1 跨崩溃和重启恢复。11.6.1 本地永久停用后，对账占用可以不冻结业务数据、不持有实体产品锁，但必须继续阻止另一升级取得准备或执行权。staging_completed 等历史终态事务及待激活记录不占执行权。槽位的创建、激活、替换、新 revision 登记及清理仍必须在同一范围内互斥，不能因已释放预安装事务占用而并发写同一槽位或解冻其他所有者的数据；已停用槽位的期限清理由唯一对账所有者按原期限执行。
- helper 可以按版本并存；新 helper 接管成功前旧 helper 保持可用，接管成功后旧 helper 不得继续写事务。
- 主安装器不能删除当前事务正在使用的 helper。
- 清理旧 helper 只能执行服务端签发、绑定精确删除集合的短期清理授权；客户端不能推测服务端内部允许状态。
- helper 自更新不等于应用回滚，也不能用于安装更低版本的品悟应用。

## 13. 跨平台要求

### 13.1 公共能力接口

业务流程只依赖以下语义能力，不直接判断操作系统：

- 探测当前安装和规范版本。
- 验证平台安装包和可执行文件身份。
- 获取系统权限。
- silent 验证已部署 helper 的既有本机执行权限，不自动向用户申请提权；权限不足返回明确修复原因。
- 创建系统级锁和受保护事务存储。
- 运行平台安装器并解释结果。
- 把完整目标版本预安装到非活动版本槽、验证可启动性、在重启时原子切换活动 launcher 指针，并安全清理失效槽位。
- 启动目标应用并执行健康检查。
- 创建、导出和清理受保护备份。
- 按 11.4.1 协调全部用户及后台写入者，并提供 17.1 所需的跨休眠、关机及重启期限判定能力；能力不可用必须明确返回不支持或需修复，不能用普通系统日期冒充。

不支持的能力必须明确返回 `unsupported`，不能静默复用其他平台实现。

### 13.2 Windows

- 首期支持已认证的 Windows 10/11 x64 目标。
- 使用 Authenticode 校验，支持 NSIS 或经批准的安装格式。
- 升级助手和事务数据保存在 per-machine 受保护位置。

### 13.3 Linux

- 首期支持 Ubuntu 22.04/24.04 的 x86_64/arm64 目标，以实际批准列表为准。
- 首期使用 DEB，不在升级过程中临时访问未知软件源解决依赖。
- 正确处理 dpkg 锁、半配置状态、root 权限和系统级受保护目录。

### 13.4 macOS

- 支持 Intel 和 Apple Silicon，Universal 包必须分别校验两种宿主约束。
- 校验代码签名、公证和 Team ID 等平台身份。
- 使用稳定 launcher 或特权助手完成主应用替换。

### 13.5 新平台接入

新增平台必须通过新的 Platform Target 和适配器接入，并提供：

- 版本和架构规范化规则。
- 安装包格式与真实性验证规则。
- 权限、锁、事务存储、安装执行和健康检查实现。
- normal、silent、forced 三种升级类型的能力声明；silent 必须提供非活动版本槽和重启激活实现，不能用覆盖当前活动目录冒充预安装。
- 固定测试向量、真实系统集成测试和人工修复路径。

新增平台不得修改既有 Windows/Linux/macOS 的版本选择和状态语义。

## 14. 事件与可观测性

至少记录：检查及触发方式、升级类型、展示或强制门进入/解除、下载、校验、预检、权限、备份、预安装授权、staged、重启提示、激活授权、助手接管、安装器或激活启动、活动版本核对、健康检查和人工修复入口展示。

- 事件必须绑定服务端签发的遥测会话或安装事务凭据。
- 服务端验证事件属于对应决定或事务，并验证合法阶段迁移。
- 客户端不能伪造服务端授权状态，也不能把失败终态改写为成功。
- 离线事件允许在凭据规定的补报期内上传。
- 事件不得包含明文 SN、业务文档、完整文件路径、下载 URL、令牌或密钥。

质量指标需要按平台、版本、渠道、来源版本和文件服务分层观察，并防止单一网络或伪造客户端样本主导自动判断。

## 15. 存量客户端迁移

1. 尚未迁移的旧客户端继续使用原升级系统。
2. 旧系统向符合条件的客户端发布一个迁移引导版本。
3. 引导版本仍按旧流程安装，但安装完成后写入新平台所需的安装范围、渠道、升级助手和信任初始化信息。
4. 只有初始化自检通过后，后续检查才切换到 `https://update.pinvou.com`。
5. 新平台不提供旧协议兼容端点。
6. 旧服务停止前必须统计迁移覆盖并保留公开人工升级入口。

## 16. 隐私与权限

- SN 是可选灰度数据。用户可以关闭发送；服务端只保存灰度所需的不可逆受密钥摘要，不保存原始 SN 到普通日志或事件。
- installId 是伪匿名客户端标识；installationScopeId 用于本机安装事务和多用户互斥，均不得用作用户身份或广告画像。
- 为限制伪造遥测对质量指标的影响，服务端可以在网络边缘将源 IP 即时派生为截断网络前缀和 ASN 簇；用途只限滥用防护、质量去重和单簇贡献上限。只有合法事件 lineage 可贡献样本。每 installationScopeId 在 `UTC自然周+完整固定单元` 最多计 1；前缀 HMAC 使用按 UTC 周派生的用途隔离 key，在同一 `周+完整单元+前缀HMAC+UTC日` 最多让 1 个不同 scope 计入 k，不以 scope 首次创建时间判断，因此预养标识不能绕过，单一前缀一周最多贡献 7。普通运营在“网络来源隐私分析面”只可读取非重叠 UTC 周、完整维度 `(platform family,targetKey,major.minor version,channel,ASN class)` 的叶子单元；不发布总计、边际、部分维度汇总、相邻窗口，也不提供临时过滤或下钻。叶子单元在应用上述上限后 `k>=20` 才显示，否则只返回 suppressed；周快照冻结后不重写。该限制不取消 8.4 的发布质量报表，但两者不得联查、共享关联键或相互补算。
- 普通运营报表默认展示聚合数据，逐实例明细仅限受控故障处置和审计角色。
- 发布、暂停、撤回、制品隔离、密钥操作和人工修复导出必须审计。
- 管理后台使用企业身份认证、MFA 和最小权限角色。

## 17. 可用性与异常处理

- 控制面不可用时不开始新的安装；已开始的本地安装仍由助手核对并收敛。
- 第三方文件服务失败时重新请求文件地址或切换备用源，不改变制品身份。
- 元数据、文件或平台身份校验失败时失败关闭。
- 客户端时钟异常时不得提前启用强制升级门、激活未到生效时间的静默版本或接受已过期授权。
- 后台恢复不能产生重复决定、重复预安装/安装/激活授权或重复消费。
- 任何恢复、重试和幂等机制均属于技术实现，但用户可观察结果必须唯一且可解释。

### 17.1 时间异常与离线跨边界

发布生效、到期、凭据有效期及本地留存期限必须以可验证的时间或经过时长判断，不能仅凭用户可修改的系统日期。客户端必须能区分“已可信到达边界”“尚未到达边界”和“时间无法可信判定”，明确展示稳定原因 `trusted_time_unavailable` 及联网校时/修复入口；界面可展示本地换算时间，但不能把它当作执行授权。时间前跳、回拨、休眠、系统重启和关机期间的经过时长均属于平台验收范围。

| 场景 | 用户可观察行为 |
|---|---|
| 已验证未来 forced 决定，离线且能够可信证明到达 installNotBefore | 按 5.5 进入联网复核受限门；不能凭已经过期的决定开始安装 |
| 尚未进入任何门，时间无法证明未来 forced 已生效 | 不仅因系统日期前跳而提前阻断普通功能；非阻断提示需要联网复核，取得可信时间及当前资格后再决定是否进入门 |
| 已存在强制或人工修复门，时间异常或无法判定 | 保留既有门，不因回拨、前跳或时间未知解门；只有可信到期/资格失效且尚未跨执行边界，或对应安装/修复成功，才按 5.5、6.2 解门 |
| silent 重启时生效、授权有效期或 staged 期限无法可信判定 | 不取得或消费服务端激活授权，不迁移、不切换；隐藏重启提示，保持原活动版本及已有门。时间恢复可信后重新在线检查，未过期且精确匹配才恢复提示，已经到期则清理 |
| 已跨越 executionCommitBoundary 的本地事务遇到时间异常 | 继续核对和安全收敛，不恢复旧版；无法确认成功时进入人工修复，不能通过修改系统日期重置健康检查预算 |

可信期限能力正常时，备份、密钥引用和非活动槽的本地期限清理必须独立于普通系统日期，可跨休眠、重启并覆盖关机时长；期限一经确定不能因回拨或重启延长，也不能仅因普通系统日期前跳而提前删除仍在合法留存期的备份。联网恢复不能重置原期限，关机跨越期限的设备必须在下次启动且联网前执行已到期清理。正常离线、休眠和关机仍须满足 11.5 的 24 小时、61 天未知消费结果上限及 60 天已消费上限，不能把正常离线视为故障例外或以等待联网代替清理。

如果本地可信期限能力本身损坏，必须持久化 `retention_time_fault` 状态，保留既有备份的加密内容和受保护密钥引用，维持原访问控制，不凭猜测时间误删人工修复备份。消费状态查询的 recovery secret 及其可恢复副本立即按 11.6.1 清除，仅保留无秘密的对账证据并使用独立对账任务；保留例外限于备份内容及其解密密钥引用。禁止创建新的受期限管理备份及发起依赖该能力的新预安装、安装或激活；已跨执行边界事务继续安全收敛，已有门保持适用规则。待激活槽位失去激活资格并隐藏提示。启动及用户查看升级/备份状态时明确提示“可信计时组件需要修复，备份可能超过原定保留期限”，给出联网校时/组件修复及管理员明确确认删除的入口；用户提示使用三语资源。故障发现、本地记录原期限、受影响记录及后续处置均写受保护审计，不上报备份内容。

retention_time_fault 是正常离线留存保障的显式例外：无法可信判定期限时，加密备份可以超期保留以避免误删；不是把原期限延后或建立新期限。可信时间及期限能力恢复后立即按原期限复核，已经到期的备份、staging 和密钥引用在恢复后的首轮清理中、最迟 10 分钟内删除，未到期记录继续原期限；管理员明确确认的提前删除仍按 11.5 审计。永久无法恢复时保持加密保留及故障告知，直到管理员明确删除，不能宣称仍满足硬上限。该例外只适用于经确认的计时组件故障，不能因无网络、普通系统日期改变或用户延后重启自动启用。

上线认证须证明正常离线、关机和系统日期变更仍可执行全部期限，并分别验证故障告知、加密保留、访问控制和恢复后的原期限清理；不要求通过未经声明的硬件能力保证计时组件损坏后仍绝不超期。原本就不具备正常可信期限能力的 targetKey 不得借故障例外启用依赖该能力的流程，也不得通过上线门槛。

## 18. 分期计划

### 一期

- 三平台完整包升级。
- 普通、静默、强制三种升级类型；三平台正式 targetKey 均完成静默预安装和重启激活认证。
- 新升级服务、管理后台和固定控制面域名。
- Stable/Beta/Internal、SN 灰度和 Deployment 暂停/撤回、Release Target 吊销及 Artifact 隔离。
- 后台将 Stable 正式版显式同步投放到 Beta/Internal，复用制品、逐渠道审批和独立投放。
- 复合包、Package Manifest、第三方文件地址。
- 升级助手、预安装/安装/激活授权、事件状态机和存量迁移。
- 增量字段、候选数组、能力协商和完整包回退合约完成；生产候选强制为空，非空候选仅用于合约测试，客户端能力为空。

### 二期候选

- 启用经验证的增量算法和重建流程。
- 更丰富的平台目标和安装格式。
- 更完善的发布质量自动化和企业扩展接口。

二期不能依赖修改一期已经发布的 Manifest 或破坏一期完整包客户端。

## 19. 验收标准

### 19.1 发布后台

- 仅发布 Stable 时 Beta/Internal 不收到该投放；显式选择 Beta、Internal 或二者同步时只创建所选渠道草稿，复用制品且不复制审批资格、基线身份或灰度进度。目标渠道未审批/未生效时不返回该更新，审批和激活后按各自选择规则返回。
- 同步全批次草稿创建失败时零写入；重复请求、同键不同配置、现有同版本投放、并发创建和响应丢失均不能产生重复或覆盖。部分平台没有有效 Stable 来源、目标基线等于/高于同步版本、权限不足、渠道审核拒绝、候选冲突及首次激活失败有明确逐渠道结果，修复重试不重复成功步骤。
- 同步后的 Deployment/Rollout 可分别灰度、暂停和撤回；Stable 来源投放暂停/撤回不自动影响其他渠道，目标渠道失败也不影响 Stable。共享 Release Target 吊销或 Artifact 隔离后，各引用渠道均拒绝新资格，并可从同步批次追溯完整审批及执行记录。
- 同一版本可分别为 Windows、Linux、macOS 上传和发布，错误平台或错误架构不能通过审核。
- `minimumSourceVersion` 缺省按 `0.0.0`；低于某目标下界时能够返回最新可用桥接版本，而不是错误下发目标版本。
- `supportFloorVersion` 初始值、提高/降低审批、受影响安装量和人工迁移入口均可验证；低于下界的来源不进入自动桥接。
- superseded Deployment 只有当前 Snapshot 可达且 BridgeEligibility=enabled 时可作为同渠道桥接；paused/withdrawn/revoked/quarantined 任一状态均阻止桥接。
- ordinary 前置跳验证独立路径资格及其修订：Deployment active 但自身 Rollout paused/aborted 时，不能借有效 1.3.0 终点把 1.2.0 重新下发；check、download-info、validate、consume 及边界前最终复核均拒绝。制品 approved 但不是本渠道有效基线、没有当前元数据及独立前置路径审批时也拒绝。合格前置跳与 superseded+BridgeEligibility 的历史跳分别验收；实际跳失效不被终点有效掩盖，已经安装 1.2.0 的来源仍可在自身条件满足时直接升级到 1.3.0。
- 当前基线 Deployment 只能在 Rollout completed 的原子基线切换事务中进入 superseded；普通 supersede 命令对当前基线必须拒绝。无替代版本停止投放使用 paused 或 withdrawn，基线指针保留用于审计但选择失败关闭。
- 普通新建、重建及同步投放均验证候选终点严格高于本 scope 当前基线：基线 1.3.0 时，1.2.0 和 1.3.0 候选在审核、排期、激活、恢复、Rollout 启动/恢复、扩量及 completed 提交点均被拒绝，1.4.0 才可继续其他检查。草稿创建后基线推进、基线暂停/撤回及并发完成也不能退回或覆盖新基线，失败零写入；首次 unactivated scope 建立基线正常通过。该条件不阻止 1.1.0 客户端经合法 1.2.0 中间跳通往 1.3.0 基线或更高候选。
- Release Target 审批/吊销、Deployment 暂停/撤回、Artifact 隔离和前向修复均有完整审计。
- assembled Release 只有在全部 Target 均为非 approved 终态时才能自动 cancelled；有待评审 Target 时即使零 approved 也不能自动取消。`CancelRelease` 可专用触发 in_review Target→cancelled，并对子 Target 与父 Release 原子收敛；普通单 Target 取消不得触发该特殊边。父投放撤回/替代时 draft Rollout 可级联 aborted 并释放唯一约束。
- 新渠道/targetKey 可从 in_review 生成绑定产品 Root head 与 component metadata head 的不可公开 staged Target/Snapshot 并进入 scheduled；首次激活比较前者、CAS 后者，原子建立 support floor、首个 active Deployment、唯一基线、current Timestamp/Snapshot/Target 和 generation。staging 后 Root 轮换、其他 scope 发布或跨 scope 双并发时旧 staged 失败并重签，unactivated 固定返回 SCOPE_UNACTIVATED 且不创建决定或会话。
- Beta/Internal 风险接受不能复用于 Stable；SupplyChainApproval/例外到期会阻止 Deployment 激活/恢复、Rollout 扩量/完成和新的下载、预安装、安装或激活资格。
- 后台没有“回滚发布”入口；问题版本只能被撤回并由更高版本修复。

### 19.2 包与下载

- 复合上传包必须含 Manifest 和完整包；后台校验外层上传包，客户端只校验可信 Manifest、实际下载的子包容器和最终安装器。
- 一期生产发布拒绝非空增量候选；合约测试中的非空候选在客户端能力为空时必须被忽略并稳定选择完整包，且不执行补丁重建。
- Package Manifest 不因时间到期失效。
- Package Manifest 不包含自身封装哈希；Release 外部引用 manifestEnvelopeSha256，修改内部 manifestId 不得替代内容身份。
- 文件地址可以来自第三方域名；篡改 URL、大小、哈希或文件内容均不能进入安装。
- SBOM/构建来源证明缺失或不匹配、漏洞库过期、未获批准的 Critical/High 风险均阻止 Stable 审批。

### 19.3 版本与灰度

- Stable/Beta/Internal 都可由用户自愿选择，默认 Stable。
- SN 灰度矩阵可验证：同时位于包含组和排除组时按排除处理；仅在排除组时不命中；仅在包含组时命中；普通 SN 按稳定分桶命中；SN 缺失、不可读取或关闭发送时，在实际百分比低于 100% 时走基线、达到 100% 时命中候选；Rollout completed 成为基线后不再应用灰度排除。
- SN 本身不能充当设备认证、安装范围绑定或预安装/安装/激活授权凭据。候选灰度命中可以决定本轮投放资格，但执行仍须使用服务端签发且绑定真实安装范围、渠道、制品、用途和事务的独立授权；不能仅凭 SN 相同、属于包含组或能够读取 SN 执行升级。
- 当前版本等于或高于渠道目标时不会降级。
- Stable `1.2.5` 同步到 Beta/Internal 后，当前 `1.2.4` 的用户只有在本渠道投放有效且命中对应选择条件时收到更新，当前 `1.2.5`/`1.3.0` 的用户不会收到该版本升级；已有更高候选的渠道仍按自己的灰度和路径规则选择，不被 Stable 同步强制改选。
- candidate 只来自 active Deployment 下的 running Rollout；暂停后不再签发下载、预安装、安装或激活资格。Rollout 启动后修改阶段计划、扩大包含组或缩小排除组均被拒绝；如需变更，须终止当前投放并创建新的已审批 Deployment/Rollout。100% 最终阶段的观察时长、最小样本和全部质量阈值未满足，或仍有扩量冻结时，completed 被拒绝；全部条件与审批满足后，基线指针、新旧 Deployment、Rollout、Timestamp/Snapshot/Target 和 selectionGeneration 在比较产品 Root head并 CAS component metadata head的同一事务原子切换，下一次检查不会回退旧基线。
- Rollout 在零样本、未达最小样本及 silent 只有 staged 尚无激活样本时暂停，安全原因已合法处置且审批、版本、路径和时间窗有效后，可恢复原百分比的投放资格并继续收集样本；不能要求在暂停期间取得被禁止的新授权来证明可恢复。恢复不解扩量冻结、不推进阶段、不完成基线切换，观察从恢复提交重新计时。没有质量冻结时正常完成完整窗口后自动判定；有质量冻结时须另行覆盖全部原问题分组的复核和操作员解冻，再完成完整观察。基线 Deployment 的自身恢复不被“候选必须高于基线”误拒绝；其 Rollout 不存在或已 completed 时，恢复基线投放不创建/重启灰度、不改写 completed、不重新切换基线，适用审批和安全校验仍完整执行。终态或安全禁止状态仍不能恢复。

### 19.4 客户端流程

- 每个正式启用的 `targetKey × hostArch × upgradeType` 组合都必须在对应真实系统完成适用主路径、权限失败、Deployment/Rollout 暂停与撤回、安装或激活失败、重启恢复和健康失败验收；首期至少分别覆盖 Windows x86_64、Ubuntu x86_64、Ubuntu arm64、macOS Intel 和 macOS Apple Silicon，Universal 包不能只测其中一种宿主架构。normal 另测逐 hop 点击前后行为，silent 另测 staged 生命周期和 executionCommitBoundary，forced 另测门的生效/解除、离线跨边界和多 hop 连续性。
- 应用退出、用户切换、助手崩溃或系统重启后，事务能够恢复到唯一终态。
- 每个 targetKey 的准备及各执行阶段预算均非零且有限；下载无硬性总时限且不因短期凭据到期停止，校验30分钟、安装/预安装/激活各不超过2小时，准备不跨当前有效会话期限，权限确认不冻结业务。慢速下载/换源/休眠/断网及跨24h验证缓存保留、当前资格续接、不伪造超时失败及原阶段不变；协调写入者无法停止、备份卡住、平台安装器不退出、迁移卡住和跨重启分别验证有预算阶段耗尽显示稳定原因/入口，不能通过重复请求或重启延长这些预算。边界前安全停止后才取消/解冻，边界后保持修复门，仍在运行的系统安装器不被强杀且不能并发前向修复；晚到成功退出码不复活已失败事务。仅服务端未收到结果时仍显示 outcome_unknown，不伪造本地超时。
- 安装失败不自动恢复旧版应用，必须显示人工修复入口。
- 活动备份不能被普通用户删除；终态后提前删除有明确不可恢复警告和审计。
- succeeded 快照在第 7 天、abandoned_before_install 快照在确认无受保护写入后的第 24 小时、failed_manual_repair_required 快照在第 30 天按各自规则自动清理；普通活动事务不按短期终态规则清理；可信期限能力正常时，已确认 consumed 后永久离线的记录仍受 transactionStartedAt+60d 硬上限约束，组件损坏时按 17.1 的加密保留、告知和恢复清理例外处理。
- 安装有效期在开始前 1 毫秒、开始边界、结束前 1 毫秒、结束边界及结束后 1 毫秒得到唯一结果；开始前只允许 silent 非活动槽预安装或 normal/forced 预下载，结束后旧下载不能取得任何预安装、安装或激活授权。
- 同一应用版本的不同合法 helper/launcher 来源画像可以分别升级，歧义画像失败关闭并提供人工入口。
- normal 在主动检查或系统提示后都只有用户点击升级才下载；点击发生在 installNotBefore 之前时可以完成下载和校验，但必须等待该边界才申请安装授权。一次确认只覆盖当前实际 hop，该 hop 成功后必须重新展示期望终点和新 hop，用户再次点击前不得下载；关闭或稍后处理不影响继续使用当前版本。silent 自动完成检查、下载、校验和非活动槽预安装，期间不打断当前使用，staged 后提示重启；forced 只有在期望终点和实际 hop 均可安装时才阻断普通功能，只保留安全保存、升级/修复、网络设置和退出。
- silent 预安装不得改变活动版本或迁移活动数据；重启时在线复核、激活授权或 executionCommitBoundary 前的前置步骤失败，按 11.6 已确认安全取消或按 11.6.1 已证明未越界并永久停用本次执行后，继续启动原版本。消费或取消响应丢失不等于安全取消；永久停用后仍保留对账占用，不能直接再次激活。首次活动数据写入或 launcher 指针切换任一先发生即越界，之后的迁移部分写入失败、迁移完成后指针切换失败或其他激活失败都禁止启动旧版并进入 failed_manual_repair_required。权威选择不再精确匹配 staged hop 时立即取消激活资格和重启提示，并按 5.5 清理或替换；同一 Deployment/Rollout paused 时隐藏提示、保留槽位且禁止激活，恢复后精确匹配才恢复提示，撤回或改选则清理。活动指针切换且新版本健康检查通过前不得上报成功。
- endpoint upgradeType × hop 激活模式矩阵可验证：normal/forced 的所有 ordinary/bridge hop 只使用 directInstall，silent 的所有 hop 只使用 stagedRestart；任一 hop 不兼容时返回人工升级入口且不跨模式解释。
- forced 在期望终点或实际 hop 未到 installNotBefore、已到 installNotAfter，尚未 releaseVisible，或当前 hop 无法取得兼容安装授权时不能在线进入普通强制门。发布审核对 endpoint 及任一 ordinary/bridge hop 验证 `releaseVisibleAt <= forced.installNotBefore`：晚于时拒绝、等于或早于时再继续校验；任一实际 hop 时间窗不能完整覆盖强制有效区间也拒绝。已验证未来 forced 决定后离线跨越生效边界时进入联网复核受限门；凭据普通到期不放行；到达 installNotAfter 或联网确认失效时，按当前 hop 的 executionCommitBoundary 判断解门；前一 hop 已成功不算当前 hop 越界，从未取得可信决定的离线客户端不凭空阻断。进入任一门后安装失败、权限拒绝或离线时不能进入普通功能。仅版本号到达终点但健康失败，或 installNotAfter 前已跨越 executionCommitBoundary 后失败时保持修复门，直至当前执行事务 succeeded 或更高关联前向修复成功；需要多 hop 时逐跳重新检查，终点仍有效才继续保持门，终点在后续 hop 未创建或尚未越界时失效且当前版本健康则解除本次强制门，独立失败事务修复门保留。
- releaseVisibleAt、installNotBefore、installNotAfter 的前 1 毫秒、等于和后 1 毫秒对三种升级类型均得到唯一结果：可见后 normal 点击前只提示、点击后可下载校验，silent 可预安装，forced 可预下载；installNotBefore 前均不切换活动版本，达到后才允许 normal 安装、silent 激活以及满足实际 hop 条件的 forced 阻断。对每种 upgradeType，分别在授权未消费、已消费但未跨 executionCommitBoundary、已跨边界、已安装或切换但健康失败四种状态验证 installNotAfter 前后 1 毫秒及边界瞬间：结果必须分别为拒绝开始、安全取消、继续收敛、保持人工修复门。
- 对每种 upgradeType，分别组合 Deployment/Rollout paused、Deployment withdrawn/superseded、Rollout aborted、Release Target revoked、Artifact quarantined、BridgeEligibility disabled、SupplyChainApproval expired/revoked、metadata immediate deny 与“未消费、已消费未越过 executionCommitBoundary、已越界”三种阶段验收资格失效矩阵；当前基线没有 running Rollout 时结果相同。silent 进行中预安装和已完成 staged 也分别覆盖：只有同一对象 paused 可隐藏提示并保留已完成 staged 槽，其他永久收紧均清理。forced 区分终点失效解门与仅路径失效时保留受限门。
- 自动扩量计划的首阶段必须在 1%—100%，严格按已审批的递增阶段逐级执行：观察时长或样本不足时百分比不变，条件满足时只由系统进入紧邻下一阶段，后台不存在人工推进入口，运行中不得降低、跳级或改写 SN 组；100% 阶段也必须完成自身的完整观察，刚进入 100% 不得立即完成基线切换。质量阈值触发后锁存“停止自动扩量”，冻结期间任何扩大资格均被拒绝；指标短暂恢复不自动解锁，只有满足完整观察窗口复核的有权限操作才能恢复，并从恢复提交时间重新观察当前阶段。`releaseVisibleAt == installNotAfter` 或 `installNotBefore >= installNotAfter` 的配置在审核前被拒绝。
- migrationMode 与 backupPolicy 的全部合法组合均可配置；irreversible+notRequired、required 但备份能力缺失、备份失败或备份验证失败均阻止审核、normal/forced 安装授权或 silent 激活授权。irreversible+required 成功时用户看到不可逆风险，快照只供人工修复且不会自动回滚。
- silent 在既有 helper 权限完整时，预安装和重启激活全程不出现 UAC、管理员密码或其他提权窗口；权限缺失时不切换、不迁移、不自动提权，保持原版本及已有门并展示主动修复入口。服务端激活授权与本机执行权限分别验收：required 备份未完成、验证失败或冻结中断时不得签发激活授权；备份完成后发布失效、授权签发失败、消费被权威确认拒绝或边界前最后复核失败时，按所有权规则确认安全取消并仅解冻本代次，未发生受保护写入；消费或取消结果未知时，仅 11.6.1 的本地安全停用可解除业务冻结，仍不得放行另一个升级准备流程。
- per-machine 在另一登录用户、断开的会话或后台服务仍写入时，任何安装/激活均不得绕过全范围保存与冻结；silent 预安装仍不打断这些用户。逐一覆盖未保存状态无法落盘、未登录用户数据不可访问、备份缺失任一必需范围、冻结中断和另一个用户尝试启动旧版：边界前安全取消或保留 forced 门，边界后保持全范围修复门，不丢弃数据、不以发起用户退出冒充全部写入者退出。成功后所有用户入口只启动新活动版本。
- normal/forced 安装准备和 silent 激活准备均覆盖两个 launcher/helper、两个用户会话及后台恢复入口同时竞争：最多一个在冻结或备份前取得准备所有权，其他流程等待或退出且不触碰活动数据。A 的取消响应晚于 B 合法接管或下一代准备开始时，A 不得解除 B 的冻结、清理 B 的槽位或继续写入；准备到消费转交之间无互斥空窗。分别在冻结后、备份中、consume 提交但响应丢失、所有权转交及取消确认时注入崩溃/重启，恢复后只有一个有效所有者，结果未知先对账，不因进程退出或请求超时放行并发准备；per-user 也须通过。
- consume/cancel ACK 丢失后持续断网，分别覆盖 normal/forced/silent 和 required/notRequired：等待消费对账不超过一次 5 分钟预算，崩溃/重启不重置。证明未越界、旧版及数据完整并永久停用全部旧执行者后，normal/silent 无既有门时立即恢复旧版业务使用，范围仍独占待对账，其他会话不能发起新升级；forced/独立修复门不被绕过。迟到 consumed ACK、旧 helper、后台恢复及重启均不能执行已停用授权或复用已解冻备份。无法证明安全停用或已经越界时不得启动旧版；预安装未知结果不建应用修复门。已停用槽位和备份仍按原期限清理，不能因对账延长期限。
- 原 forced 终点或中间 hop 已进入不可改写的失败终态后，更高且与原事务关联的前向修复只有在版本、全部受影响数据及本地健康检查均通过并 succeeded 后才解关联门；原失败终态保持不变。修复失败、只有更高版本号、原终点撤回/到期、其他安装范围的成功和无关联事务成功均不能解除该修复门。
- 健康检查分别覆盖只有进程存在但界面不响应、数据无法安全打开、启动循环、平台身份不符、单次 120 秒和总计 5 分钟边界、允许的单次重启重试及 helper/系统重启后不重置预算。外部模型/业务服务器离线且本地条件通过时记录本地成功，forced 正常解门；成功事件暂未上报及后续服务端 reporting_timeout 不重新封锁已经本地验证成功的安装。
- 阶段质量夹具按 8.2.1 验证相同安装范围的重复下载、备用源切换、重复事件和重试只贡献一次；各指标已确认分母为零、未达最小样本或未知结果上界未低于阈值时不扩量，silent 大量 staged 但激活/健康样本不足时不扩量。合法证据确认执行失败、下载有可信进度的未完成、失联/进度不可确认的下载未知，以及校验30分钟/执行2小时/健康5分钟仍无结果的未知分别展示；未知按保守上界冻结，不伪造执行失败或事务终态。合法迟到成功/失败可更新原阶段质量投影及对账记录，但历史快照和冻结保留，不自动解冻。来源/hop分组不能由其他分组成功稀释；下载超过24h且进度可确认仍持续等待，不自动冻结，单请求超时可继续；失联按审批间隔转未知可冻结但不终止客户端续传，新会话/新状态去重更新原投影。
- 当前灰度组全部已升级、无新来源可重复安装时，合法未知结果对账加完整窗口内独立健康复核能够申请恢复，无需重新下载或安装同版本；仅历史成功而无新健康复核、凭据过期且结果仍未知、未恢复真实失败、故意排除未知/失败范围或操作员缺权限均不能通过。专用复核凭据不能复用为安装或旧事务事件凭据。操作员恢复后百分比不变，重新完成整个独立观察窗口后才自动进入紧邻阶段，不能恢复即扩量或跳级。
- 阶段一的迟到失败达到其阈值、此时 Rollout 已在阶段二时，冻结原因必须绑定阶段一及实际来源/hop/指标；仅阶段二样本健康不能恢复。当前阶段和全部未解除原因的原阶段/分组各用原审批样本、阈值和完整窗口独立通过，不合并分母、搬移成功或用更宽阈值替代。复核中及恢复提交前新增另一历史原因时必须纳入并重新满足完整条件，不能按旧集合恢复；恢复后仍以当前百分比完整观察此次关联的所有问题分组，已解除标记不能移除它们。历史阶段失败/未知仍保留，新触发再次冻结，完成历史评估不被改写。
- 原候选 1.3 安装失败或结果未知，关联前向修复 1.4 成功且健康时，只记录设备修复并按关联解除本机门，不能增加 1.3 的成功分母或降低其未知上界，原失败/未知仍参与恢复复核及恢复后观察。复核任务必须绑定原候选的版本、targetKey、签名 Package/制品哈希及激活模式；同版本但制品身份不符、不同平台、不同模式及更高版本健康均不能冒充原候选健康通过。不能剔除已转入 1.4 的范围；代码或制品修复必须终止原投放并发布更高版本。原不可变候选只有在合法原结果、精确活动身份、窗口内健康和全部原分组质量条件都满足时才可恢复。
- silent 预安装消费、完整槽位验证、staging_completed 终态及待激活记录落盘后立即释放活跃事务占用；staged_waiting_restart 不阻止新的激活事务取得占用。激活使用新 transactionId 并精确关联槽位和原预安装事务，两个消费请求竞争时最多一个取得执行权。延期重启、staged 到期、暂停、撤回及清理不改变 staging_completed，不触发安装失败或人工修复门；预安装失败、取消及 30 天上报超时均使用预安装专用终态，未确认完成的槽位不能激活。崩溃恢复不得留下双占用、丢失 staged 记录或把槽位存在误报为升级成功。
- silent 激活消费成功后，在边界前安全取消：服务端确认 cancelled_before_install 且本地证明无受保护写入、槽位完整后，原 activating revision 终结并不可重放；原期限内精确资格仍有效时只建立一个更高 revision，关联原记录和取消事务，不修改原 stagedAt/stagedValidUntil。下一次用户应用重启或系统重启须重新冻结、required 备份、新授权和新 transactionId，旧授权、旧事务及已解冻旧备份均不能复用。分别覆盖消费/取消确认丢失、新 revision 提交响应丢失及重启，结果未知禁止重建和并发激活，幂等恢复不得产生双待激活记录。paused 仅保留并隐藏提示，恢复后再复核；到期、撤销、改选、渠道变化不能重建资格，已越界失败不能重启原 staged 激活。
- forced 的 1.1→1.2→1.3 路径在 1.2 已 succeeded 且活动版本健康后，分别于 1.3 事务尚未创建、授权未消费、已消费未越界时撤回或到期终点：停止下一跳并解除本次强制门，保留 1.2；1.3 已越界时继续收敛，只有成功且当前版本健康才可按终点失效解门，失败仍保留修复门。仅桥接路径失效但终点有效时保留受限门；任何此前独立失败事务的修复门不能随本次强制门解除。
- 可信期限能力正常时，backup_succeeded 后未 consume 的所有会话终态都在确认无受保护写入后保留快照 24 小时并自动清理；快照落盘后事件发送前永久离线、backup_failed 和部分写入崩溃也由受保护本地清理记录跨重启收敛。consume 提交但 ACK 丢失且永久离线时，consume_pending 在 authorizationExp+61d 边界强制本地清理；ACK 已确认后永久离线时，transaction_retention 在 transactionStartedAt+60d 强制清理。客户端计时组件损坏时，仅本地备份留存适用 17.1 的故障例外；服务端 consume-status 的 +61d 查询关闭和 tombstone 的 +62d 删除期限不延长，错误 secret、consumeKey 或请求摘要仍不得查询结果。

### 19.5 存量迁移

- 旧客户端先通过旧系统取得迁移引导版本。
- 引导安装成功且初始化自检通过后才切换新平台。
- 迁移失败仍可通过旧系统或固定人工入口修复，不产生半初始化的新平台安装。

### 19.6 协议与事件

- 控制请求只访问 `update.pinvou.com`；下载文件可访问响应返回的第三方地址。
- Deployment/Rollout 暂停、Deployment 撤回/替代、Rollout 终止、Release Target 吊销、Artifact 隔离、BridgeEligibility 禁用、SupplyChainApproval 到期/撤销或 metadata immediate deny 发生在授权消费前、消费后但 executionCommitBoundary 前、边界后时，客户端和服务端分别按 6.2 资格失效矩阵收敛，事件终态及是否保留强制门唯一；不能只验证“授权前”场景。
- Deployment/Rollout 暂停、BridgeEligibility 禁用或安装有效期结束后，download-info、validate 和 consume 均不能产生新资格。
- Stable→Beta→Stable 后旧 channelRevision 的未消费决定、下载资格和三类授权均被拒绝；normal/forced 已消费安装事务和 silent 已消费激活事务的事件继续按事务冻结 revision 与 lineage 接受，不能跨事务使用或授予新资格。silent 预安装进行中时切换渠道必须取消并清理不完整槽，已 staged 未进入消费激活事务时必须立即隐藏提示、取消资格并清理；三种状态分别验收。
- 无合法服务端凭据、跨事务事件、阶段倒退和终态改写全部被拒绝。
- 遥测会话 24 小时、执行事务 30 天到期后由服务端进入用途对应的规范终态；会话超时与 active validate、available 授权、consume/cancel 的边界竞争产生唯一结果。7/37 天合法窗口内晚到事件不改写会话/事务终态，只作对应 lineage 诊断；经校验的证据可以按 8.2.1 更新独立质量投影，不延长任何凭据、不直接解门或自动解冻。staging_completed 不受待重启记录的期限影响，非终态预安装的报告超时只进入 staging_failed，不能误建应用人工修复门。
- 事件签名 key 正常轮换后旧凭据仍可按窗口使用；紧急 deny 后同一旧凭据即使签名和状态边合法也统一返回 `EVENT_KEY_DENIED`，不泄露该 eventId 是否已提交。已提交事件不重复写入也不重放原成功响应，提交事实只进入内部审计。
- 未命中灰度的客户端无法从 Target、决定或公共元数据地址解析候选版本、Release ID 或 Manifest；命中客户端能验证候选 Release 对当前 Target 承诺的开封关系，错误成员证明被拒绝。
- 同一请求重试不会创建重复升级会话、重复授权或重复安装事务。
- 预安装、直接安装及激活 consume 响应丢失后，required/notRequired 场景均可通过控制面的最小消费结果查询确认 consumed/cancelled/expired 与 transactionId；授权用途、安装范围、请求摘要、secret 或其他绑定字段错误均不泄露结果，查询不返回或延长安装、下载和事件授权。authorizationExp+61d 查询关闭、+62d tombstone 删除后，通过 MFA/最小权限管理角色和绑定原请求及本地停用的独立受限对账任务处置；缺失历史结果保持未知，不伪造取消或成功。任一安全证明缺失不能释放占用，合法处置只追加关联记录、不改写旧终态；响应丢失/重启不重复处置或恢复旧执行权。只有服务端确认为 cancelled_before_install 才能重建原 staged revision，reporting_timeout 等其他终态须清理后按新决定重新预安装。
- 时间前跳、回拨、休眠、关机跨生效/到期边界和重启后无法可信判定，分别按 17.1 验收未入门、已入门、staged 待激活和已跨执行边界四种状态；未知时间不提前阻断、不解除既有门、不允许激活。正常可信期限能力下，无网络跨关机及系统日期改变不重置原期限：仍在合法期限的备份不被前跳误删，到期记录在规定扫描窗口或下次启动且联网前清理。确认组件损坏时进入 retention_time_fault，既有备份及密钥保持加密和访问控制，告知可能超期并禁止新增依赖能力的流程；不能把普通离线冒充故障。消费查询 recovery secret 及其可恢复副本立即删除，备份解密密钥引用保持受保护，无秘密的请求/事务证据及执行停用凭据仍可供独立对账任务使用；计时修复不能恢复旧 secret 或延长服务器查询窗口。能力恢复后按原期限在首轮、最迟 10 分钟内清理到期记录，管理员提前删除有明确确认及审计；永久故障下的加密保留是明确的硬上限例外，不能宣称绝不超期。
- 发布质量周报只能查看 8.4 的固定完整叶子与唯一冻结快照，k=19/20 时分别显示样本不足/聚合值；总计、部分维度汇总、任意或相邻窗口、临时过滤、合并和下钻请求均被拒绝，导出不附加隐藏值或合计。总数 21、可见子组 20、隐藏子组 1 的夹具不能同时取得总数和子组以相减；同一安装范围不重复进入两个叶子，文件源切换只进入 multi_source 分类。迟到对账不重写普通运营快照，界面/导出/即时阶段诊断/网络来源隐私面不能组合为绕过抑制的第二查询入口；越权诊断查询被拒绝并审计。
- 网络质量去重不在普通事件、运营导出或长期应用日志中暴露原始 IP；网络簇聚合在 `k=19/20/21` 边界分别抑制/允许/允许。同一周内预养 20 个 scope 仍受逐日逐前缀上限约束，单一前缀最多贡献 7；周 key 轮换后不可跨周关联。公开接口只存在完整叶子单元且拒绝总计/边际/部分维度/相邻窗口，无法通过互补查询恢复小样本；超过保留期的边缘安全日志和网络簇摘要按安全要求删除。
- supportFloor、Deployment、Rollout、BridgeEligibility、SupplyChainApproval 和 Registry 的所有 generation 变化均经 PublishSelectionChange；多渠道变更全成或全败，Target generation 不等或 metadata_sync_pending 时所有动态端点失败关闭。
- 产品双 component 的 genesis/active 混合场景共享唯一 RootPublishHead；同版本不同字节 Root 分叉拒绝，单 component 触发轮换后另一 component 可在新 Root 下继续读取。Release/Target/Snapshot/Timestamp 刷新经 PublishMetadataRefresh，与选择发布比较相同 Root head并竞争各自 component head，不能覆盖并发结果或回退完整 Snapshot。

## 20. 上线门槛

正式上线前必须满足：

1. 每个正式启用的 targetKey、hostArch 和已认证 OS 组合均完成真实系统端到端测试。
2. Stable Deployment 激活/暂停/撤回、Release Target 吊销、Artifact 隔离和更高版本前向修复演练通过。
3. 第三方文件服务故障切换和文件篡改演练通过。
4. 旧客户端迁移演练和人工升级入口可用。
5. 安全要求文档中的信任、签名、授权、吊销和隐私验收通过。
6. 技术设计中的幂等、并发、恢复和灾备方案经过架构评审，但其复杂度不得回流并遮蔽本文的产品主线。
