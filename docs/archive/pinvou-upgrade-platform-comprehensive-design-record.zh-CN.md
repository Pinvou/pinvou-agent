# 品悟统一升级平台历史综合设计记录

> 状态：历史工作记录，非规范制品，不作为开发或验收依据
>
> 日期：2026-10-01
>
> 适用产品：Pinvou Agent
>
> 说明：本文仅保留历次对抗审核产生的设计探索，供追溯使用。内容可能与当前产品需求、安全要求和模块化技术设计冲突；实现、测试和上线不得引用本文作为规范依据。
>
> 首期目标平台：Windows、Linux、macOS
>
> 归档复核说明（2026-10-08）：下文的“V1”“必须”、旧状态/时限及待确认项均为当时讨论记录，不是当前契约。当前规范已由三份V1.0主文档及对应specs承接；下载当前不设硬性总时限。历史内容保留供追溯，不随当前规则改写，也不得从中抽取旧规则作为实现依据。

## 1. 文档说明

### 1.1 目的

本文是从原综合文档保留下来的历史设计记录，记录曾讨论过的更新包协议、服务端幂等与并发、状态恢复、来源事实账本和跨平台实现约束。它不是产品需求、当前技术设计或已冻结规范，不得作为架构评审、研发实现或测试验收的输入。

产品范围、优先级和用户可观察行为以 [品悟统一升级平台需求文档](../pinvou-upgrade-platform-requirements.zh-CN.md) 为准；安全信任边界以 [品悟升级协议与安全要求](../pinvou-upgrade-protocol-security.zh-CN.md) 为准；当前架构以 [品悟统一升级平台技术设计](../pinvou-upgrade-platform-technical-design.zh-CN.md) 为准。

### 1.2 参考资料

- 旧 OTA 平台 V0.7.0 需求设计文档。
- 旧 Windows 客户端 MegaBook OTA 流程文档及其中两张本地更新流程画板。
- Windows 现有 `Pinvou3_0.10.5.0.zip` 更新样包。
- 当前品悟仓库中的多平台构建、发布流水线和应用内升级占位实现。

### 1.3 方案取舍

- 删除域名引导。检查更新、灰度判断、文件信息、安装前复核和事件上报固定访问 `https://update.pinvou.com`。
- 下载文件属于数据面，实际 HTTPS 地址由文件信息接口动态返回，可位于第三方对象存储、CDN 或文件服务；客户端不得硬编码下载域名。
- 存量客户端不直接接入新协议，先由原有升级系统单跳升级到迁移引导版本；迁移引导版本成功启动后固定切换到 `update.pinvou.com`，此后不再回退旧升级协议。
- 硬件 SN 是可选的灰度分组输入，只影响候选灰度命中，不是身份凭证、安全授权边界或升级准入条件；随机安装实例 ID 负责实例级幂等、令牌防重放绑定和链路追踪。
- MD5 升级为 SHA-256；采用签名元数据、包清单签名与平台真实性验证组成的校验链。
- 首期使用平台原生完整安装包，不把 Windows 逐文件覆盖推广为跨平台方案。
- V1 同时定义完整包和增量包。首期正式客户端不执行增量，但服务端、Schema、候选接口和重建验证必须可处理非空增量包。
- OS 差异收敛到平台适配器；未知平台明确返回不支持。

### 1.4 术语和唯一标识

| 术语 | 定义 | 唯一标识 |
|---|---|---|
| 产品 Product | 可独立检查和升级的产品，本项目固定为 Pinvou Agent | `productId=pinvou-agent` |
| 组件 Component | 产品内独立安装的应用组件，首期仅桌面应用 | `componentId=pinvou-desktop` |
| 平台目标 Target | OS、架构和安装格式的精确组合 | `targetKey` |
| 制品 Artifact | 后台存储的任一内容不可变文件记录，例如包容器、SBOM 或构建证明；属于后台领域实体 | `artifactId` |
| 包容器 Package | 客户端作为一个下载单元获取的 `FullPack.zip` 或 `IncrementalPack_*.zip` | `packageId` |
| 包清单 Package Manifest | 描述一个复合上传包内完整包、增量包及哈希的构建期签名清单 | `packageManifestVersion` |
| 发布 Release | 同一产品版本、发布说明和多平台目标的逻辑分组，本身不作为逐平台审批或吊销边界 | `releaseId` |
| 发布目标 Release Target | 某 Release 下一个 `targetKey` 的制品、最低来源版本、迁移声明及独立审批/吊销单元 | `releaseTargetId` |
| 部署 Deployment | 某已批准 Release Target 在一个渠道和平台目标上的投放配置 | `deploymentId` |
| 灰度 Rollout | 部署的一次分批范围、分桶参数和质量阈值 | `rolloutId` |
| 发布清单 Release Manifest | 为一个 Deployment 签发的客户端可验证元数据，包含渠道、策略、候选制品和平台签名身份 | `releaseManifestVersion` |
| 目标策略元数据 Target Metadata | 某渠道和平台目标的受支持来源下界、已认证 OS/主机架构范围及安装范围策略 | `targetMetadataVersion` |
| 基线版本 Baseline | 某渠道和平台目标上未命中候选灰度时返回的当前默认 Deployment；不表示一定属于 stable 渠道 | `baselineDeploymentId` |
| 最低请求版本 / 最低来源版本 Minimum source version | 允许请求方直接升级到某发布的连续来源区间下界，边界包含该版本；缺省为 `0.0.0` | `minimumSourceVersion` |
| 桥接版本 Compatibility bridge | 期望终点不接受当前来源版本时，返回的同渠道安全中间版本；既可为当前基线，也可为历史正式版本 | `selectionMode` |
| 迁移引导版本 Migration bootstrap version | 由原有升级系统下发、内置新信任根及新 Updater，用于把存量客户端一次性切换到新升级平台的产品版本 | `migrationBootstrapVersion` |
| 安装范围标识 Installation scope ID | 由安装器为一个系统级产品安装生成并保存在受保护系统位置的 UUID，用于跨用户安装互斥和事务仲裁，不是用户或硬件身份 | `installationScopeId` |

`Pinvou3_Win` 仅可作为旧样包导入时的 `legacyAppId`，不得出现在 V1 新清单或跨平台 API 中。`appName` 只用于展示，不参与身份或安全判断。

## 2. 背景、范围与目标

### 2.1 现状

| 操作系统 | 首期架构 | 当前安装包 |
|---|---|---|
| Windows | x86_64 | NSIS EXE |
| Linux | x86_64、arm64 | DEB |
| macOS | Intel + Apple Silicon | Universal DMG |

当前应用内升级仍为占位实现，发布流水线主要生成 CI Artifact，尚未形成可信发布、灰度、下载、安装、健康确认和失败诊断闭环。现有 Windows 样包使用“外层包清单 + 完整子包 + 可选增量子包”的双层 ZIP，方向保留，字段语义和安全协议重新定义。

### 2.2 核心目标

1. 建设管理后台、升级 API、可信制品链、客户端通用升级核心、平台安装适配器和质量监控。
2. 首期完成 Windows、Linux 和 macOS 全量升级、安装结果确认、失败诊断和人工修复指引闭环。
3. 从 V1 起冻结全量/增量候选协议，后续启用增量不改变现有字段语义。
4. 支持不同平台目标独立上传、审核、灰度、暂停、撤回、晋升和前向修复发布。
5. 新增系统时只增加目标注册、构建流水线和平台适配器，不修改通用状态机。
6. 建立构建来源、双人审核、客户端校验、安装结果和审计日志的完整追踪链。
7. 支持按最低来源版本构造单跳兼容升级路径，使旧版本先升级到可接受它的最新桥接版本，再重新检查后续版本。
8. 通过唯一迁移引导版本完成存量客户端从原有升级系统到新平台的单向切换，不要求新平台兼容旧升级协议。

### 2.3 首期非目标

- 操作系统、驱动和固件自身升级。
- 模型、插件、技能和连接器等独立资源升级。
- Linux RPM、AppImage、Flatpak 和 Snap。
- 增量包在正式客户端上的选择、下载、重建和安装。
- 无网络安装和企业离线升级包。
- 升级失败后的自动回滚、恢复包生成/分发/执行，以及旧版本安装器或旧 App Bundle 的自动重装。首期只确认实际结果、保留诊断信息并提供人工修复入口。
- 需要重启操作系统才能完成的升级；首期只支持退出并重启品悟应用及其受控子进程。

非目标不等于不定义协议。首期仍须实现非空增量包解析、存储、服务端重建验证、API 候选返回和客户端安全忽略。

## 3. 设计原则

- **固定控制面、动态数据面**：升级控制请求固定访问 `update.pinvou.com`；文件地址由受控接口短期签发。
- **协议先行**：V1 同时覆盖全量和增量候选，不以首期未执行增量为由省略字段。
- **全量兜底**：没有可信基础制品、算法不支持或增量重建失败时改用完整包。
- **内容不可变**：已批准制品、哈希、签名和清单不得原地修改。
- **多重验证**：传输 TLS、SHA-256、Ed25519 元数据签名及平台真实性验证缺一不可；Windows/macOS 使用操作系统代码签名/公证，Linux 使用签名元数据链、制品哈希和受约束的 DEB 元数据/维护脚本验证。
- **精确目标**：OS、系统版本、架构和安装格式必须精确匹配。
- **结果可确认**：“启动安装器”不等于“升级成功”；必须通过外部健康观察确认结果，失败时进入可诊断、可人工修复的终态。
- **不打断工作**：普通更新不得直接终止运行任务或未保存内容。
- **灰度标识不等于安全身份**：硬件 SN 仅用于灰度分组，缺失、不可读、非法或变化均不得阻断基线、兼容桥接或已全量发布的前向安全修复；安装实例 ID 用于实例链路和短期令牌绑定，两者均按持久标识保护。
- **能力协商**：客户端显式声明协议版本、增量算法和平台能力；未知必需能力必须拒绝。

## 4. 用户、角色与职责隔离

| 角色 | 权限 |
|---|---|
| 客户端用户 | 查看说明、选择更新或延期、授权安装、查看安装结果及人工修复指引 |
| 制品上传者 | 创建草稿、上传制品、查看自动校验；不得批准本人上传的稳定制品 |
| 发布管理员 | 配置渠道、目标、灰度和策略；不得绕过安全校验 |
| 审核人员 | 批准或驳回稳定发布、强制更新、紧急修复发布和密钥变更 |
| 运维人员 | 监控、暂停部署、切换文件源、处理告警；不能修改已签名内容 |
| 安全响应人员 | 撤回或封禁高风险制品、发起密钥吊销 |
| 审计人员 | 只读查看制品、审批、策略、密钥和操作记录 |

稳定渠道、强制策略、紧急修复发布及根密钥变更必须双人批准，且上传者与最终审核者不能为同一人。internal/beta 虽允许单把发布密钥签名，但仍必须由至少一名与上传者不同的人员审批 Release Target 和 Deployment；自动化服务不得代替该人员审批。紧急安全撤回允许安全响应人员单人立即执行，但事后 24 小时内必须补充复核和事件记录。

管理后台与公开升级 API 必须逻辑隔离。后台使用企业 SSO/OIDC、强制 MFA 和最小权限 RBAC，禁止共享账号；稳定发布签名、强制策略、密钥操作、文件服务切换和安全撤回要求短时二次认证。服务账号使用独立工作负载身份、最小 scope 和定期轮换凭据，不得模拟人员审批。所有登录、权限变更、审批、拒绝、发布、暂停、恢复、撤回、导出和密钥操作记录操作者、时间、来源、前后值、原因及关联工单，审计日志对普通管理员只读且不可原地删除。

## 5. 总体架构

```text
构建流水线
  └─ 复合包、Package Manifest、SBOM、构建证明、平台真实性证明
        ↓
升级管理后台
  ├─ 上传/自动校验/独立审核
  ├─ Release / Release Target / Deployment / Rollout 状态管理
  ├─ Target / Release Manifest 签名
  └─ 制品存储与文件服务编排
        ↓ 固定控制面
update.pinvou.com
  ├─ 检查与可选 SN 灰度分组
  ├─ 短期文件地址签发
  ├─ 安装前复核与授权
  └─ 事件及质量监控
        ↓ 动态数据面
第三方对象存储 / CDN / 文件服务
        ↓
客户端通用升级状态机
  ├─ 签名验证与制品缓存
  ├─ Windows 适配器
  ├─ Linux DEB 适配器
  ├─ macOS 适配器
  └─ 后续系统适配器
```

控制面不得代理大文件内容。文件服务只获得下载所需的匿名制品路径或短期签名参数，不得获得硬件 SN、安装实例 ID 或升级 API 凭据。

## 6. 平台与版本模型

### 6.1 平台目标

平台目标由 `os + arch + packageFormat` 唯一标识：

| `targetKey` | OS | 架构 | 格式 | 首期系统范围 |
|---|---|---|---|---|
| `windows-x86_64-nsis` | Windows | x86_64 | NSIS | Windows 10 22H2、Windows 11 23H2 至平台目标注册表中已认证的最高版本 |
| `linux-x86_64-deb` | Linux | x86_64 | DEB | Ubuntu 22.04、24.04 LTS Desktop |
| `linux-arm64-deb` | Linux | arm64 | DEB | Ubuntu 22.04、24.04 LTS Desktop |
| `macos-universal-dmg` | macOS | Universal | DMG | macOS 11 至平台目标注册表中已认证的最高正式版，Intel/Apple Silicon |

Linux 请求的 `osVersion` 必须包含发行版 ID、版本和内核架构。未列出的发行版、服务器无桌面环境、WSL、容器环境及未知系统均返回 `unsupported_target`，不得套用 Ubuntu 安装命令。

平台目标注册表必须版本化记录每个 `targetKey` 的最低/最高已认证系统版本、架构、安装格式、启用状态和验证日期。“及以上”“当前正式版”不得使尚未验证的新系统自动获得支持；操作系统新大版本须先通过真实机安装、权限、重启和健康观察矩阵，再更新注册表。停止支持既有系统须至少提前 90 天公告，且不能影响该系统上已安装应用的正常运行或人工下载入口。

注册表只是后台编辑源；客户端和检查服务的实际准入以当前 Snapshot 所引用的已签名 Target Metadata 为准。`hostArch` 必须由升级助手通过操作系统可信 API 读取，不得根据用户配置或包名推断；其表示真实宿主 CPU 架构，与 `target.arch=universal` 分开传递。服务端选择前和客户端安装前都必须使用 `targetKey + hostArch + osVersion` 匹配 Target Metadata 中对应的 `hostConstraints`；架构、操作系统家族或已认证版本范围不匹配时返回 `unsupported_target`。

宿主信息的 Schema、来源和比较规则冻结如下，服务端与客户端共用固定向量：

- Windows：`osVersion={family:"windows",ntVersion:"10.0",build:22631,productType:"workstation"}`。由 RtlGetVersion/受信系统 API 读取，不使用兼容模式或用户可写注册表；`ntVersion` 按两段非负整数元组比较，`build` 按非负整数比较，仅 `productType=workstation` 可命中 V1。Target Metadata 使用 `minBuild`/`maxBuild` 表达已认证范围。
- Linux：`osVersion={family:"linux",distributionId:"ubuntu",versionId:"22.04",kernelArch:"x86_64"}`。由 root-owned `/etc/os-release` 和 `uname -m` 读取，`distributionId` 按 ASCII 小写精确匹配；`versionId` 是发行版提供并经允许列表校验的规范 ASCII 字符串，V1 只允许 `22.04`、`24.04` 等后台明确列入 Target Metadata `allowedVersionIds` 的原值，不作数值比较，`22.04` 中的 `04` 合法而 `22.4`、Unicode 数字或未列值拒绝。架构先按固定表规范化再比较：`uname x86_64 + DEB amd64 → hostArch=x86_64`，`uname aarch64|arm64 + DEB arm64 → hostArch=arm64`；`kernelArch` 保存规范化后的 `x86_64|arm64`，未知别名或三者映射不一致均拒绝。服务端、客户端和构建闸门共用同一映射测试向量。
- macOS：`osVersion={family:"macos",productVersion:"14.6.1",darwinMajor:23}`。由 `NSProcessInfo`/`sysctl` 等受信系统 API 读取，`productVersion` 规范化为 `major.minor.patch`（缺失 patch 补 0）并按三段非负整数元组比较；`hostArch` 取物理宿主 `x86_64|arm64`，在 Rosetta 下不得使用当前进程的 `x86_64` 代替 Apple Silicon 宿主。
- 通用：未知字段可忽略。Windows/macOS 数值版本段若为负数、含前导零、非 ASCII 数字或超出 32-bit 范围则拒绝；该数值段规则不适用于 Linux 的发行版原始 `versionId`，后者仅按上一条规范字符串与允许列表判断。缺失必需字段、架构规范化失败或无法通过可信 API 复核时均返回 `unsupported_target`。V1 `installScope` 枚举只允许 `perMachine`；未来支持 `perUser` 须提升 Target Metadata Schema 和安装协议版本。

### 6.2 版本规则

- 根目录 `VERSION` 是产品版本唯一事实来源。
- V1 `version` 使用 SemVer 2.0.0 的 `major.minor.patch` 核心形式，例如 `0.11.0`；所有渠道均禁止 prerelease 和 build metadata，以避免 DEB revision、macOS 版本字段和 Windows 四段版本的排序语义分叉。发布成熟度只由 `internal`/`beta`/`stable` 渠道表达；需要发布新构建时必须提升产品版本。整个版本串限 17 个 ASCII 字节，每个数字段的整数范围固定为 `0..65535`；拒绝前导零（单个 `0` 除外）、符号、空白和 Unicode 数字，所有实现按三个无符号整数比较固定向量，不转换为浮点数。该限制同样适用于 current/target/minimum/supportFloor/migrationBootstrap 等产品版本字段。旧四段版本仅存入 `legacyVersion`，不参与比较。
- 升级平台不支持服务端降级发布，任何服务端返回的目标版本必须高于客户端实际当前版本。当前版本存在问题时，应暂停、撤回或安全吊销问题版本，并发布 SemVer 与 `releaseSequence` 都更高的修复版本，使受影响客户端继续向上升级。
- 服务端按 SemVer 判断版本，不以字符串或 `legacyVersion` 比较。
- `releaseSequence` 在同一 `productId + componentId + channel + targetKey` 内严格单调递增且永不复用，用于元数据防降级和同版本冲突检测，不能替代 SemVer 的产品版本比较。
- 请求中的 `currentVersion` 必须由客户端从受保护的安装元数据、可执行文件签名版本或平台包管理数据库读取，不得来自用户可编辑配置；外部升级助手在安装授权消费前必须独立复核实际版本。

跨平台比较统一使用 `canonicalAppVersion`，其值必须等于根目录 `VERSION` 的严格 SemVer，并按下列来源读取，禁止各适配器自行选择替代字段：

- Windows：签名主可执行文件的 ProductVersion 必须直接保存 canonicalAppVersion；四段 FileVersion 唯一、可逆地映射为 `<major>.<minor>.<patch>.0`，四段均满足 Windows 16-bit 范围，第四段不承载独立构建号。FileVersion 仅用于 Windows 构建诊断，不能参与升级大小比较。ProductVersion、FileVersion 映射和安装注册信息不一致时拒绝授权。
- Linux：DEB `Version` 首期必须等于 canonicalAppVersion，不允许 epoch、Debian revision 或 `~` 等另行排序语义；需要重新打包时必须提升产品版本。`dpkg-query` 是安装事实来源。
- macOS：已签名 App Bundle 的 `CFBundleShortVersionString` 必须等于 canonicalAppVersion；`CFBundleVersion` 仅作单调构建号和制品诊断，不参与升级比较。
- 旧四段版本只允许通过迁移引导版本随包签名并在后台审计的固定映射转换为一个 canonicalAppVersion；无法唯一映射时不得自动进入新协议。

## 7. 更新包协议

### 7.1 复合上传包

```text
PinvouAgent_<version>_<os>_<arch>.zip
├── UpdatePackInfo.json
├── SBOM.cdx.json
├── Provenance.intoto.jsonl
├── FullPack.zip
│   ├── OtaInfo.json
│   ├── Files/PinvouAgent/<完整平台安装器>
│   └── Files/Updater/<可选版本化助手制品>
├── IncrementalPack_<base1>_<target>.zip       # 可选
│   ├── IncrementalOtaInfo.json
│   ├── Patches/<patch>
│   └── Files/Updater/<可选版本化助手制品>
└── IncrementalPack_<base2>_<target>.zip       # 可选
```

后台解析后将完整包和每个增量包作为独立不可变制品存储，客户端只下载其最终选择的一个制品，不下载整个复合包。

协议明确区分两层身份：`packageId + size + sha256` 标识客户端下载的包容器（`FullPack.zip` 或某个 `IncrementalPack_*.zip`）；`resultArtifactSize + resultArtifactSha256` 标识最终交给操作系统安装的安装器（EXE、DEB 或 DMG）。“制品”单独出现时必须说明是包容器还是最终安装器，不得用一个 `artifactSha256` 字段混指两者。`packageId` 在全平台全环境中全局唯一，实际值必须包含不可复用的构建标识或 UUID，本文可读示例不定义生成格式；生成后不得复用。相同 SHA-256 可以被多个发布引用，但不能被赋予冲突的产品、目标或版本语义。

ZIP 安全上限默认值如下，调整必须经安全审核并审计：外层上传包不超过 4 GiB、嵌套层级不超过 4、文件数不超过 10,000、单文件不超过 4 GiB、总解压大小不超过 8 GiB、总压缩比不超过 100:1，并限制解压 CPU/墙钟时间。禁止加密条目、绝对路径、`..`、盘符、备用数据流、设备名、重复路径、符号/硬链接，以及大小写或 Unicode 规范化后碰撞的路径。解压必须在无网络、无执行权限和资源配额受限的隔离目录中完成，任何清单外文件均不得进入最终安装阶段。

### 7.2 签名封装和规范化

所有可签名 JSON 使用同一封装：

```json
{
  "signed": {
    "_type": "package-manifest",
    "schemaVersion": 1,
    "packageManifestVersion": 7
  },
  "signatures": [
    {
      "keyId": "build-2026-a",
      "algorithm": "ed25519",
      "value": "<base64url-no-padding>"
    }
  ]
}
```

签名输入是 `signed` 对象按 RFC 8785 JSON Canonicalization Scheme 规范化后的 UTF-8 字节；`signatures` 字段不参与签名。时间统一使用 UTC RFC 3339 秒精度并以 `Z` 结尾，哈希使用小写十六进制，签名使用无填充 Base64URL。禁止 `NaN`、无穷值及超过 IEEE-754 安全整数范围的 JSON 数字；文件大小因此不得超过 `9007199254740991`。

签名数组可以包含多个 keyId，但阈值只按不同且在当前 Root 中有效、属于该角色的 keyId 计票；同一 keyId 的重复签名最多计一票，重复项内容不一致、算法不符或任一声称来自有效 keyId 的签名校验失败时必须拒绝并告警。客户端不依赖数组顺序。任何 signed 字段变化都必须重新签名，服务端和客户端发布同一组固定测试向量作为跨语言合约测试。

可签名元数据的网络封装必须是“完整封装对象按 RFC 8785 规范化”后的唯一 UTF-8 字节序列：无 BOM、无末尾换行、`Content-Encoding: identity`、`Content-Type: application/json; charset=utf-8`，`signatures` 按 `keyId` 字节序排序。Snapshot/Timestamp 中的 `length` 和 `sha256` 始终覆盖该完整网络封装字节；Release Manifest 的 `targetMetadataSha256` 和 `packageManifestSha256` 也指向对应完整封装字节。`signed` 对象单独的构建事实身份如需记录，使用明确命名的 `signedPayloadSha256`，不得与封装哈希混用。所有签名元数据在验签前均拒绝重复 JSON key、非法 UTF-8、非规范数字和超限嵌套。

### 7.3 Package Manifest

`UpdatePackInfo.json` 是构建期 Package Manifest，只描述制品事实，不包含发布说明、渠道或更新策略：

```json
{
  "signed": {
    "_type": "package-manifest",
    "schemaVersion": 1,
    "packageManifestVersion": 7,
    "productId": "pinvou-agent",
    "componentId": "pinvou-desktop",
    "version": "0.11.0",
    "legacyVersion": "0.11.0.0",
    "target": {
      "os": "windows",
      "arch": "x86_64",
      "packageFormat": "nsis"
    },
    "platformIdentity": {
      "type": "windows-authenticode",
      "publisherSubject": "<approved-publisher-subject>",
      "leafSpkiSha256": ["<approved-spki-sha256>"],
      "rfc3161TimestampRequired": true
    },
    "postInstallLauncher": {
      "version": "1.2.0",
      "protocolVersion": 1,
      "installTargetId": "pinvou-updater-launcher-stable",
      "size": 2100000,
      "sha256": "<launcher-sha256>",
      "launcherAuthenticity": {
        "type": "windows-authenticode-helper-v1",
        "publisherSubject": "<approved-publisher-subject>",
        "leafSpkiSha256": ["<approved-spki-sha256>"],
        "rfc3161TimestampRequired": true
      }
    },
    "sbom": {
      "fileName": "SBOM.cdx.json",
      "format": "cyclonedx-json-1.5",
      "sha256": "<sbom-sha256>"
    },
    "buildProvenance": {
      "fileName": "Provenance.intoto.jsonl",
      "format": "slsa-provenance-v1",
      "sha256": "<provenance-sha256>"
    },
    "fullPackage": {
      "packageId": "full-0.11.0-windows-x86_64",
      "transactionHelperId": "pinvou-updater-helper",
      "fileName": "FullPack.zip",
      "format": "full-installer-v1",
      "size": 170000000,
      "sha256": "<full-package-zip-sha256>",
      "resultArtifactSize": 169000000,
      "resultArtifactSha256": "<installer-sha256>",
      "helperArtifacts": [
        {
          "helperId": "pinvou-updater-helper",
          "version": "1.4.0",
          "protocolVersion": 2,
          "filePath": "Files/Updater/pinvou-updater-helper.exe",
          "installTargetId": "pinvou-updater-helper-versioned",
          "size": 8400000,
          "sha256": "<helper-sha256>",
          "helperAuthenticity": {
            "type": "windows-authenticode-helper-v1",
            "publisherSubject": "<approved-publisher-subject>",
            "leafSpkiSha256": ["<approved-spki-sha256>"],
            "rfc3161TimestampRequired": true
          },
          "installMode": "versioned-handoff-v1"
        }
      ]
    },
    "incrementalPackages": [
      {
        "packageId": "delta-0.10.5-to-0.11.0-windows-x86_64",
        "transactionHelperId": "pinvou-updater-helper",
        "baseVersion": "0.10.5",
        "targetVersion": "0.11.0",
        "basePackageId": "full-0.10.5-windows-x86_64",
        "fileName": "IncrementalPack_0.10.5_0.11.0.zip",
        "format": "artifact-delta-v1",
        "algorithm": "bsdiff-v1",
        "size": 32000000,
        "sha256": "<delta-package-sha256>",
        "baseArtifactSize": 160000000,
        "baseArtifactSha256": "<base-installer-sha256>",
        "resultArtifactSize": 169000000,
        "resultArtifactSha256": "<target-installer-sha256>",
        "helperArtifacts": []
      }
    ]
  },
  "signatures": [
    {
      "keyId": "build-2026-a",
      "algorithm": "ed25519",
      "value": "<base64url-no-padding>"
    }
  ]
}
```

`packageManifestVersion` 是同一 `productId + componentId + target` 下严格递增且不复用的正整数，不是产品版本；相同版本号对应不同规范化内容时一律视为冲突。Manifest 的对外内容身份以 7.2 定义的完整网络封装字节 SHA-256 为准，不是仅对 `signed` 对象求哈希。

`incrementalPackages` 必须存在并允许为 `[]`；`fullPackage.helperArtifacts` 和每个增量项自己的 `helperArtifacts` 也必须存在并允许为 `[]`。每个完整包和增量包都必须声明唯一 `transactionHelperId`，表示消费授权后实际持有该事务、写事务状态并启动主安装器的助手；该字段不是展示信息，必须进入三层一致性、决定、安装意图和安装授权绑定。`fullPackage.sha256` 和增量项的 `sha256` 均为对应 ZIP 包容器哈希；`baseArtifactSize + baseArtifactSha256` 绑定增量重建所需的完整基础安装器，`resultArtifactSize + resultArtifactSha256` 绑定最终平台安装器，并必须与子包清单一致。`helperArtifacts` 是在主安装器前可能需要部署的版本化升级助手，不是恢复包；每项必须位于其所属包容器的受约束路径，并签名绑定 helperId、严格 SemVer 版本、协议、大小、哈希、`installTargetId`、独立 `helperAuthenticity` 和固定安装模式。`installTargetId` 必须存在于客户端/launcher 的版本化受保护路径注册表，并唯一映射到该平台的 root/系统所有版本化路径模板；Manifest 不接受任意绝对路径。主应用的 `platformIdentity` 不得替代助手身份。增量包承载助手时，其 IncrementalOtaInfo 必须声明与该增量项完全相同的数组；不承载时两处均为空且只能使用已安装、满足 Release 最低助手协议的可信助手。每个增量包只对应一个基础版本、基础包、目标版本、平台和算法。同一组合不得出现语义冲突。旧 `fullPack`、`incrementalPacks` 和 `Pinvou3_Win` 只由导入器读取并转换，不暴露给 V1 客户端。

`helperAuthenticity` 是按 `type` 区分的封闭 tagged union，并在 Package Manifest、子包 OtaInfo 和 Release Manifest 三层逐字段一致：Windows 使用 `windows-authenticode-helper-v1`，必含 Publisher、允许的叶证书 SPKI 集合和 RFC 3161 时间戳要求；macOS 使用 `macos-code-requirement-helper-v1`，必含 Team ID、助手独立 identifier、designated requirement 和 `notarizationRequired=true`，不得复用主 App Bundle ID；Linux 使用 `linux-elf-helper-v1`，必含 `metadataRole=build`、允许的 ELF 类型/架构、root-owned 规范安装路径、`uid=0`、`gid=0` 及禁止组/其他用户写入的 mode，不能套用 DEB 的包名和维护脚本身份。助手/launcher 的精确内容哈希只使用其外层条目的必需字段 `sha256`；各平台 `helperAuthenticity`/`launcherAuthenticity` union 不重复声明 SHA-256 或签名 keyId。任一平台都先验证元数据签名与外层精确哈希，再验证该平台身份及最终受保护路径/权限；未知 type 必须拒绝。

`postInstallLauncher` 描述目标安装器执行成功后必须存在的稳定 launcher，不是可在安装前独立写入的 helperArtifact。Package Manifest、完整/增量 OtaInfo 和 Release Manifest 必须逐字段一致地绑定其严格版本、协议、受保护 `installTargetId`、大小、SHA-256 与独立 `launcherAuthenticity`；launcherAuthenticity 使用与 helperAuthenticity 相同的三平台 tagged-union 字段和规范化规则。launcher 只能由已验证的主平台安装器在其事务内更新，主应用或下载阶段不得直接替换；安装后健康确认先验证 launcher 的目标路径、权限、版本、协议、哈希、真实性与抗降级高水位，再提交应用事务。失败或断电时由安装器事务日志和旧稳定入口核对；无法唯一确认时进入人工修复，不运行身份不明的 launcher。任何 Release 若不更新 launcher，也必须声明与当前预期完全相同的 postInstallLauncher 事实，防止字段缺省绕过验证。

macOS 与 Linux 的规范分支固定如下；除展示占位符外，正式 Schema 中 Team ID、identifier 和架构仅允许 ASCII，designatedRequirement 以 UTF-8 NFC 保存且最大 2 KiB，路径模板必须命中 installTargetId 注册表且不得含 `..`、软链接或变量展开，mode 是四位 ASCII 八进制字符串。Linux 只绑定稳定 `metadataRole=build`，实际签名者由当前 Root 对 Package Manifest 的 Build 角色阈值决定，不把具体 keyId 写入不可变助手/launcher 身份，以允许正常密钥轮换：

```json
{
  "macos": {
    "type": "macos-code-requirement-helper-v1",
    "teamId": "<APPLE-TEAM-ID>",
    "identifier": "com.pinvou.agent.updater-helper",
    "designatedRequirement": "identifier \"com.pinvou.agent.updater-helper\" and anchor apple generic and certificate leaf[subject.OU] = \"<APPLE-TEAM-ID>\"",
    "notarizationRequired": true
  },
  "linux": {
    "type": "linux-elf-helper-v1",
    "metadataRole": "build",
    "elfType": "ET_DYN",
    "arch": "x86_64",
    "installPathTemplate": "/usr/lib/pinvou/updater/{version}/pinvou-updater-helper",
    "uid": 0,
    "gid": 0,
    "mode": "0755"
  }
}
```

### 7.4 完整子包

`FullPack.zip/OtaInfo.json` 示例：

```json
{
  "schemaVersion": 1,
  "productId": "pinvou-agent",
  "componentId": "pinvou-desktop",
  "version": "0.11.0",
  "target": {
    "os": "windows",
    "arch": "x86_64",
    "packageFormat": "nsis"
  },
  "platformIdentity": {
    "type": "windows-authenticode",
    "publisherSubject": "<approved-publisher-subject>",
    "leafSpkiSha256": ["<approved-spki-sha256>"],
    "rfc3161TimestampRequired": true
  },
  "postInstallLauncher": {
    "version": "1.2.0",
    "protocolVersion": 1,
    "installTargetId": "pinvou-updater-launcher-stable",
    "size": 2100000,
    "sha256": "<launcher-sha256>",
    "launcherAuthenticity": {
      "type": "windows-authenticode-helper-v1",
      "publisherSubject": "<approved-publisher-subject>",
      "leafSpkiSha256": ["<approved-spki-sha256>"],
      "rfc3161TimestampRequired": true
    }
  },
  "hostArch": "x86_64",
  "installer": {
    "fileName": "pinvou-agent_0.11.0-windows-x64-setup.exe",
    "filePath": "Files/PinvouAgent/pinvou-agent_0.11.0-windows-x64-setup.exe",
    "size": 169000000,
    "sha256": "<installer-sha256>",
    "authenticityProfile": "windows-authenticode"
  },
  "transactionHelperId": "pinvou-updater-helper",
  "helperArtifacts": [
    {
      "helperId": "pinvou-updater-helper",
      "version": "1.4.0",
      "protocolVersion": 2,
      "filePath": "Files/Updater/pinvou-updater-helper.exe",
      "installTargetId": "pinvou-updater-helper-versioned",
      "size": 8400000,
      "sha256": "<helper-sha256>",
      "helperAuthenticity": {
        "type": "windows-authenticode-helper-v1",
        "publisherSubject": "<approved-publisher-subject>",
        "leafSpkiSha256": ["<approved-spki-sha256>"],
        "rfc3161TimestampRequired": true
      },
      "installMode": "versioned-handoff-v1"
    }
  ]
}
```

校验责任分两层，不得要求客户端校验未下载的外层文件。后台入库/发布闸门的顺序为：外层大小/哈希/Package Manifest 签名 → 安全解压 → SBOM/构建证明文件哈希与 Schema → `OtaInfo` Schema → 安装器大小/哈希 → 产品/版本/目标 → 平台真实性验证。客户端只下载子包，因此顺序为：可信元数据链与 Package Manifest → 子包容器大小/哈希 → 安全解压与 `OtaInfo`/`IncrementalOtaInfo` → 最终安装器大小/哈希、产品/版本/目标 → 平台真实性验证。Windows/macOS 执行 8.4 的代码签名/公证校验；Linux 执行签名元数据链、哈希、DEB 身份及维护脚本哈希校验，不虚构 DEB 原生代码签名。SBOM 和构建证明不随子包下载，只由服务端闸门验证；其哈希继续由 Package Manifest 绑定以供审计。构建证明必须绑定源码修订、构建流水线身份和最终安装器 SHA-256，SBOM 必须能对应同一构建。

`OtaInfo.hostArch` 在非 Universal 目标上必须与 `target.arch` 相同；Universal DMG 上该字段省略，不得用一个构建时架构假装为宿主架构。Universal 包的实际 `hostArch` 一律取自运行时检查请求，并与已签名 Target Metadata 的对应约束匹配。

Package Manifest 中所选 packageId、该子包 OtaInfo 和 Release Manifest 对应 packageId 的 `transactionHelperId` 与 `helperArtifacts` 必须逐字段一致；数组必须按 `helperId` ASCII 升序，数组内 `helperId`、`installTargetId` 及其解析后的规范化目标安装路径均不得重复，同一数组不得为同一 helperId 声明多个版本，版本必须符合 6.2 的严格 SemVer。数组非空时，`transactionHelperId` 必须精确命中其中一项；数组为空时，它必须命中来源受保护事实中的一个已安装助手，且该助手满足 Release 最低协议。数组的 RFC 8785 SHA-256 记为 `helperArtifactsSha256`；Release 中所有允许 packageId 到 `{transactionHelperId,helperArtifactsSha256}` 的有序映射再规范化取哈希，记为 `helperArtifactsIndexSha256`，用于检查决定绑定；选择具体包后，安装授权同时绑定该包的 transactionHelperId 与 helperArtifactsSha256。非空数组只能解压到不可执行 staging，完成大小、哈希和 `helperAuthenticity` 校验，不能在安装授权消费前写入受保护目录或启动新助手。任一三层字段缺失、额外、重复、顺序错误、身份不一致，或 transactionHelperId 不可解析，均阻止 validate；是否需要写入字节及是否需要所有权接管按 11.2 的两个独立布尔值判断，不能仅因某个相同版本助手已安装但未激活就直接启动安装器。

### 7.5 增量子包、算法注册和重建验证

`artifact-delta-v1` 表示“基于已缓存完整安装制品重建目标完整安装制品”的容器协议，而非某一种算法。补丁算法必须在服务端和客户端注册表中以稳定标识协商；首个参考算法为 `bsdiff-v1`。未注册算法的包可以隔离存储和展示，但不得进入审核或候选响应。

`IncrementalOtaInfo.json` 示例：

```json
{
  "schemaVersion": 1,
  "productId": "pinvou-agent",
  "componentId": "pinvou-desktop",
  "baseVersion": "0.10.5",
  "targetVersion": "0.11.0",
  "target": {
    "os": "windows",
    "arch": "x86_64",
    "packageFormat": "nsis"
  },
  "deltaFormat": "artifact-delta-v1",
  "platformIdentity": {
    "type": "windows-authenticode",
    "publisherSubject": "<approved-publisher-subject>",
    "leafSpkiSha256": ["<approved-spki-sha256>"],
    "rfc3161TimestampRequired": true
  },
  "postInstallLauncher": {
    "version": "1.2.0",
    "protocolVersion": 1,
    "installTargetId": "pinvou-updater-launcher-stable",
    "size": 2100000,
    "sha256": "<launcher-sha256>",
    "launcherAuthenticity": {
      "type": "windows-authenticode-helper-v1",
      "publisherSubject": "<approved-publisher-subject>",
      "leafSpkiSha256": ["<approved-spki-sha256>"],
      "rfc3161TimestampRequired": true
    }
  },
  "algorithm": "bsdiff-v1",
  "baseArtifact": {
    "packageId": "full-0.10.5-windows-x86_64",
    "fileName": "pinvou-agent_0.10.5-windows-x64-setup.exe",
    "size": 160000000,
    "sha256": "<base-installer-sha256>"
  },
  "resultArtifact": {
    "fileName": "pinvou-agent_0.11.0-windows-x64-setup.exe",
    "size": 169000000,
    "sha256": "<target-installer-sha256>",
    "authenticityProfile": "windows-authenticode"
  },
  "transactionHelperId": "pinvou-updater-helper",
  "helperArtifacts": [],
  "patches": [
    {
      "fileName": "Patches/installer.patch",
      "size": 31000000,
      "sha256": "<patch-sha256>"
    }
  ]
}
```

后台接受非空增量包前必须使用注册算法的独立参考实现，以指定基础制品在隔离环境完成重建，并验证 `resultArtifactSize`、SHA-256 与平台真实性。仅比对声明的结果哈希不算通过。三平台真实安装包的性能、峰值内存和节省率须在启用前完成技术验证；默认只有增量传输大小不高于完整包 70% 才可下发。

算法注册表还必须固定客户端资源预算：`bsdiff-v1` 默认最多使用 2 个逻辑 CPU、`min(512 MiB, 可用内存 25%)` 峰值内存和 30 分钟墙钟时间；输出只允许流式写入隔离 staging，累计字节不得超过已签名 resultArtifactSize，达到精确大小后仍有输出或中间展开超过 `2 * resultArtifactSize` 即中止。用户取消、进程内存/时间预算超限或系统进入低资源状态时安全终止、删除不可信输出，并按 7.6 的 24 小时规则改用完整包；算法版本若需不同预算必须提升稳定 algorithm ID，不得由单个 Release 任意放宽。

### 7.6 客户端基础制品缓存

- 校验通过的完整平台安装器缓存在应用数据目录的 `updates/cache`，不得依赖系统下载目录或猜测旧安装器路径。
- 缓存索引持久保存来源 `packageId`、版本、目标、最终安装器大小、`resultArtifactSha256`、平台签名身份和最后访问时间，并使用原子写入及崩溃恢复日志；不得把包容器哈希登记成基础安装器哈希。
- 成功升级后可保留当前版本的完整基础制品，供后续增量重建使用；默认缓存总上限 4 GiB，超限按 LRU 清理，但不得清理正在下载、待安装或重建事务引用的制品。
- 迁移引导版本首次通过新平台升级、全新安装、用户清理、磁盘不足或缓存哈希不符时视为没有基础制品，直接选择完整包。
- 使用基础制品前再次验证大小、SHA-256 和平台真实性；重建只在 staging 目录进行，不修改基础缓存或运行目录。
- 增量失败后删除不可信 staging 结果，且同一 `releaseId + basePackageId` 在 24 小时内不再重试增量，改用完整包。

### 7.7 首期增量预留的完成定义

首期必须交付：同时覆盖空/非空数组的 JSON Schema；非空包解析、隔离、存储、展示和参考重建校验；完整候选与非空增量候选 API；客户端缓存索引和候选解析；算法能力协商及安全回退。首期正式客户端上报空的 `incrementalAlgorithms`，因此只选择完整包。后续启用仅增加算法能力和开关，不修改 V1 响应字段。

## 8. 签名、密钥和可信元数据

### 8.1 元数据角色

采用精简的多角色信任模型：

| 角色 | 用途 | 签名阈值 | 有效期 |
|---|---|---:|---:|
| Root Metadata | 声明各角色公钥、阈值、版本和撤销状态；初始版本随客户端内置 | 3 把根密钥中的 2 把 | 1 年以内 |
| Timestamp Metadata | 指向最新 Snapshot 的版本、长度和哈希，防止冻结 | 在线时间戳密钥 1 把 | 24 小时以内 |
| Snapshot Metadata | 列出所有有效 Target/Release/Package Manifest 的版本、长度和哈希 | 在线元数据密钥 1 把 | 7 天以内 |
| Target Metadata | 绑定渠道/目标支持下界、已认证系统范围、主机架构和安装范围 | stable 为 3 把发布密钥中的任意 2 把；internal/beta 至少 1 把 | 90 天以内 |
| Package Manifest | 绑定不可变的 CI 构建事实、完整包和增量包 | 构建签名密钥至少 1 把 | 不设置到期时间 |
| Release Manifest | 绑定发布、策略、平台身份、完整包和增量候选 | stable/强制/紧急修复为 3 把独立发布密钥中的任意 2 把；internal/beta 至少 1 把 | 90 天以内 |
| Online Authorization | 签发个性化决策、一次性安装授权和事件会话凭据，不授予修改发布内容的能力 | 在线授权密钥 1 把 | 决策不超过 15 分钟、安装授权不超过 5 分钟；遥测会话事件窗口 24 小时/上传期 7 天，安装事务事件窗口 30 天/上传期 37 天 |

客户端验证顺序为 Root → Timestamp → Snapshot → Target Metadata → Release Manifest → Package Manifest → 包容器/最终安装器大小与 SHA-256 → 平台真实性验证。Timestamp 引用 Snapshot，Snapshot 引用各 Target/Release/Package Manifest 的精确版本、长度和 SHA-256；Release Manifest 引用精确 `targetMetadataVersion + targetMetadataSha256` 及 Package Manifest SHA-256。下载 URL 永不进入离线签名清单，URL 变化不能改变制品身份。

Snapshot 只列出当前每个作用域的 Target Metadata，以及基线、活动候选、有效桥接 Deployment 和它们实际引用的 Package Manifest；不再可达的普通 superseded 历史修订不进入新 Snapshot，但仍按留存策略离线保存。V1 Snapshot 完整封装上限 8 MiB、条目上限 10,000；达到 70% 告警并在扩容前引入由顶层 Snapshot 签名引用的分片/委托元数据，不得通过截断响应解决。

Snapshot 为每个 `productId + componentId + channel + targetKey` 签名记录全局单调的 `selectionGeneration`。Target 业务约束、基线/候选/桥接集合、Deployment/Release Target/Artifact 安全状态、Rollout 运行资格或其他会改变检查/授权结果的状态每次提交都必须严格提升 generation；纯时间续签且业务集合逐字等价时不提升。generation 分配和当前值属于 17.2 的 RPO=0 高水位；检查令牌绑定它，validate/consume 默认必须精确等于当前 generation。唯一安全兼容例外是：某 generation 只因候选 Rollout 暂停/终止而被替换，旧决定的 `rolloutDecisionKind=baseline-prerequisite` 且所选 active 基线、Target 业务策略、Manifest、渠道 revision 和制品身份逐字未变；后台可在 RPO=0 允许表中仅保留该旧 generation 对该基线 packageId 的资格。Target/基线/安全状态等任何其他变化都不得继承。安全暂停/撤回还必须先命中即时 deny，不能等待 generation/元数据传播。

Root、Timestamp、Snapshot 使用各自角色的全局单调版本；`targetMetadataVersion` 在同一 `productId + componentId + channel + targetKey` 下单调递增；`releaseManifestVersion` 的分配作用域明确为 `productId + componentId + channel + targetKey`，在该作用域全局唯一且严格递增，但客户端防降级高水位仍按 `deploymentId` 保存；`packageManifestVersion` 按 7.3 的目标范围单调递增。所有版本均为不超过 JSON 安全整数上限的正整数且永不复用；同一作用域内同版本不同哈希必须拒绝并产生最高级别安全告警。

Target Metadata 至少包含 `productId`、`componentId`、`channel`、`targetKey`、`targetMetadataVersion`、`supportFloorVersion`、`installScope`、`hostConstraints[]`、`issuedAt` 和 `expiresAt`。`hostConstraints` 每项明确 `hostArch`、OS/发行版和 `versionScheme`：Windows/macOS 分别使用 6.1 的 min/max build 或版本元组，Ubuntu 使用精确 `allowedVersionIds`，不得混用比较模式；Universal DMG 必须分别列出 `x86_64` 与 `arm64`，允许两者使用不同系统范围。请求中的 `hostArch` 独立于包的 `arch=universal`，客户端和服务端均须匹配。可变平台注册表只是 Target Metadata 的后台编辑来源，未签名或尚未进入已验证 Snapshot 的变更不得影响客户端安装资格。

`supportFloorVersion` 是可达性矩阵的实际起点，作用域固定为 `productId + componentId + channel + targetKey`，必须是已正式发布或迁移引导版本，不枚举理论 SemVer。stable 初始值等于对应目标的 `migrationBootstrapVersion`。提高下界会停止更旧来源的自动路径，必须双人批准、展示受影响安装量并同时提供已公告的人工升级方案；降低下界扩大兼容承诺，必须先通过新增来源版本的真实迁移测试。任一变更生成更高 Target Metadata 版本、重新签名、进入新 Snapshot 并审计，不能只改数据库配置。

Package Manifest 描述的是不可变构建事实，不包含 `expiresAt`，不因墙钟时间自然失效。它本身可验证不代表当前允许分发或安装；客户端只有在有效的 Timestamp、Snapshot、Target Metadata、Release Manifest 和安装授权共同允许时，才能使用其制品。

后台必须对所有正在生效的 Target Metadata 以及基线、候选和桥接 Release Manifest 在过期前 14 天告警并按原角色阈值续签。Target Metadata 的任何变化都会改变版本和封装哈希，必须执行统一的原子级联发布：先冻结受影响作用域并计算仍需继续提供的全部基线、候选、当前基线中间跳和历史桥接 Deployment；生成更高 Target Metadata；为每个继续有效的 Deployment 生成更高 Release Manifest 修订并改为引用新 Target；不再符合新 `supportFloorVersion`、`hostConstraints` 或 `installScope` 的 Deployment 必须先暂停或撤回，不能带病续签；生成并签名包含完整新集合的更高 Snapshot 和 Timestamp；最后一次性切换选择指针。切换前只能提供成套旧集合，切换后只能提供成套新集合，不得出现新旧交叉引用或 Manifest 不在当前 Snapshot 中却被决策返回。

纯时间续签只允许提升元数据版本、更新 `issuedAt`/`expiresAt`、Target 引用和由此产生的签名，`enforcementEffectiveAt` 及其他业务字段必须不变；这种机械续签不重复业务审批，但仍必须达到原签名阈值并写入审计。`supportFloorVersion`、OS/架构约束、`installScope` 等业务变化必须展示安装量与路径影响，完成对应风险审批、兼容性测试和必要的 Deployment 处置，再使用同一原子级联流程发布。续签失败时元数据到期即停止新下载/安装并产生 P0 可达性告警，不得默默延长有效期。

### 8.2 元数据防降级（安全防回滚）、防冻结和时钟

本节“防回滚”专指攻击者不能让客户端接受旧的签名元数据或低版本目标，是安全协议概念；不表示升级失败后恢复旧程序。首期后者按 2.3 明确不支持。

- 客户端全局持久保存已接受的最高 Root、Timestamp 和 Snapshot 版本/哈希；Target Metadata 按 `productId + componentId + channel + targetKey` 保存最高版本/哈希；Release Manifest 按 `deploymentId` 保存该投放自身已接受的最高修订/哈希。相同作用域内更低版本或同版本不同哈希一律拒绝。不同 deploymentId 之间不得用 `releaseManifestVersion` 横向比较，同一 Release Target 在不同渠道的 Manifest 也必须相互隔离；历史桥接 Deployment 的较高修订不能使另一 Deployment 的当前有效 Manifest 被误判为回滚。哪些 Deployment/Release Target 当前有效由最新 Snapshot 的签名集合决定，产品升级方向另由 SemVer 与 `releaseSequence` 约束。
- Timestamp、Snapshot、Target Metadata 或 Release Manifest 过期时不得开始新的下载或安装。客户端验证 Timestamp 后计算 `trustedNow=max(localWallClock,lastTrustedTime)`，并且必须同时满足 `issuedAt <= trustedNow + 10min`、`trustedNow < expiresAt`、`trustedNow >= lastTrustedTime`。十分钟只容忍 Timestamp 签发时间略微领先本地可信时间，不得使用 `abs(localClock-issuedAt)`，也不得因 Timestamp 已签发超过十分钟而拒绝仍在 `expiresAt` 前的合法元数据。完成整条元数据验证后才原子提升 `lastTrustedTime` 到 `max(lastTrustedTime,issuedAt,trustedNow)`；未经签名的 HTTP Date 不能校准安全时间。
- 客户端每次成功验证 Timestamp 时还持久化 `trustedWallAnchor + suspendAwareMonotonicAnchor + bootId`。同一 bootId 内以包含休眠时间的单调时钟推算 expectedWall；本地墙钟相对 expectedWall 后退超过 2 分钟或前跳超过 10 分钟即进入持久化 `clockAnomalous=true`。bootId 变化且尚未取得新的可信时间锚点时同样视为异常。异常期间仍以单调 lastTrustedTime 防回滚，拒绝新下载/安装，并对首次将生效的 deadline/mandatory 失败开放；已经生效的限制不解除。普通缓存 Timestamp 无论是否仍在 expiresAt 前，都不能清除 bootId 变化或时钟异常。客户端必须通过 11.1 的无缓存 time-challenge 取得绑定本次随机数的新鲜签名时间证明，以其 `issuedAt` 原子重建墙钟/单调锚点并令 `trustedNow=max(lastTrustedTime,issuedAt+本 bootId 单调经过时间)`；在本地墙钟重新落入 ±10 分钟前，安全时间继续只由该锚点推算。只有随后成功验证完整元数据链才清除 `clockAnomalous` 并允许新下载/安装；用户只改本地标志、拨钟或重放旧证明不能恢复。固定向量必须覆盖 ±边界 1 秒、休眠、重启、前跳、后退、旧缓存 Timestamp、挑战重放和恢复。
- 同一渠道和目标的 `releaseSequence` 单调递增；新建候选发布的 `targetVersion` 必须高于当前基线版本。历史低版本只能作为满足来源版本约束的桥接版本，不能重新成为面向较高当前版本的降级目标。
- 新 Root 必须同时满足旧 Root 阈值和新 Root 阈值。根密钥轮换至少保留一个客户端版本的双签过渡期。
- 密钥吊销通过更高版本 Root 生效；疑似泄露时立即停止签发、撤回相关发布并触发客户端高优先级元数据刷新。
- 构建密钥状态区分 `active`、`retired` 和 `revoked`：正常轮换进入 `retired` 后不得再签新 Package Manifest，但既有签名仍可用于历史制品验证。只要当前 Snapshot 的可达 Package Manifest 中仍有对象仅由该 retired key 满足阈值，Root 就必须保留验证授权；计划移除前必须以 active 构建密钥重新生成等价但版本更高的 Package Manifest，并完成受影响 Release Manifest、Snapshot、Timestamp 的原子级联和客户端过渡观察，确认旧对象退出全部当前/兼容可达集合后方可从更高 Root 删除公钥，避免 retired build keys 无界累积。确认密钥泄露时进入 `revoked`，客户端拒绝该密钥签署的 Manifest，后台必须吊销所有受影响 Release Target 并撤回其 Deployment。不得用普通轮换代替安全吊销。
- Online Authorization 密钥状态固定为 `active_for_signing`、`retired_verify_only`、`revoked`。只有 active 可签新令牌；正常轮换后的 retired 不得签发，但其公钥与 Root 授权的删除时点必须晚于以下全部时点的最大值再加 2 分钟：该密钥最后签发凭据的 `exp`、引用该密钥凭据的 check/refresh/validate/consume/cancel 幂等记录到期时间，以及遥测凭据 `exp+24h` 的 lineage 幂等关闭截止时间。后台必须按每个 kid 维护并审计该保留高水位；最长安装事务凭据和所有 24 小时恢复窗口都结束前不得删除验证授权。轮换顺序固定为：先以 Root 阈值发布含新公钥但尚不签发的更高 Root → 经 Timestamp/Snapshot 传播并达到所有受支持客户端可获取的发布闸门 → 后台才把新密钥切为 active_for_signing → 旧密钥切为 retired_verify_only；不得先签新凭据再传播验证公钥。revoked 表示泄露等安全事件，所有相关令牌及包含这些令牌/凭据的幂等结果查询立即拒绝，并按应急手册暂停受影响的新安装、要求重新检查；不得把安全吊销伪装成普通 retired。V1 不向已签发 decision lineage 或安装事务补发替代凭据，因为 SN、installId 和 installationScopeId 都不能证明请求方是原合法设备。已消费且已经本地启动的事务只按受保护本地日志完成/诊断；服务端将其标记为 `credential_revoked_telemetry_unavailable` 派生运营状态，从普通第 37 天终态合成和安装质量失败样本中排除，只进入独立安全/数据缺失统计。客户端取得新 Root 后只能用新密钥创建完全独立的新检查会话，不继承旧 sequence、确认或授权。
- Target、Release、Snapshot、Timestamp 等元数据角色密钥同样区分 active-for-signing、retired-verify-only 和 revoked。retired 不再签新对象，但只要当前或兼容 Snapshot 可达集合仍引用其签名，Root 就必须保留其验证授权；移除前必须按原阈值重签全部仍可达对象、执行 Target→Release→Snapshot→Timestamp 级联并确认客户端过渡窗口。Timestamp/Snapshot 旧对象退出有效期和可达集合后方可移除对应 retired 公钥。revoked 立即拒绝并走安全撤回/重签流程。Build 密钥继续遵守上一条独立规则；不得因普通退休让历史 Package Manifest 突然不可验证。

### 8.3 密钥保管及签名故障

根密钥离线分权保管；构建、稳定发布、时间戳、Snapshot 和在线授权密钥使用相互隔离的 HSM/KMS 身份且禁止导出。stable 的 3 把发布密钥必须由至少两个独立审批身份控制，任何单一人员或单一自动化身份不能取得 2 个有效签名；使用 2-of-3 而非 2-of-2，使一把密钥维护或故障时仍可在双人批准下发布。在线授权密钥只能签发短期令牌，不能签 Manifest。签名、轮换、吊销和阈值变更均写入不可篡改审计日志。

发布签名服务故障时禁止创建新稳定发布，但已发布且未过期的 Release Manifest 可继续使用；在线元数据服务可以在不改变发布内容的情况下续签 Snapshot 和 Timestamp。时间戳服务故障超过 24 小时后客户端对新升级失败关闭，但不得影响已安装应用正常运行。

### 8.4 平台发布者身份

Release Manifest 必须声明并签名以下允许身份，客户端要求制品身份与其完全匹配：

- Windows：Authenticode 证书链有效，允许的 Publisher Subject 和叶证书 SPKI SHA-256；叶证书必须具有 Code Signing EKU，签名必须含可验证的 RFC 3161 时间戳。在可信签名时间证书有效的已发布制品可在证书自然过期后继续验证；证书或时间戳链明确吊销则拒绝。吊销检查优先使用 OS 在线状态，离线时仅接受尚在 nextUpdate 内的受信缓存状态；无法得到新鲜吊销状态时，internal、beta、stable 三个公开渠道的新安装一律失败关闭，不因预览渠道降低真实性要求。证书续期通过清单双身份过渡。
- macOS：Developer ID Application Team ID、Bundle ID 和公证票据；客户端使用 Security.framework/codesign 等系统 API 验证指定要求，并使用 SecAssessment/Gatekeeper 验证公证，优先要求随包 staple 的票据以支持离线校验。系统明确拒绝、票据与制品不匹配或身份不符时失败关闭；禁止移除 quarantine 或绕过 Gatekeeper。
- Linux：DEB 包名、必需 Maintainer、可选 Vendor、架构、版本及维护脚本哈希必须与三层 Manifest 声明一致；本地 DEB 不假定存在发行版仓库签名，真实性由品悟 Package/Release Manifest 签名链和制品哈希保证。若未来引入额外 DEB 签名，须作为显式能力和身份字段另行版本化。

`platformIdentity` 是按 `type` 判别的封闭 tagged union，必须在 Package Manifest、OtaInfo/IncrementalOtaInfo 与 Release Manifest 三层逐字段一致，未知 type 属于必需能力不支持。Windows 结构固定包含 `publisherSubject`、非空 `leafSpkiSha256[]` 和 `rfc3161TimestampRequired=true`；macOS 固定包含 `teamId`、`bundleId`、`notarizationRequired=true`。Linux 固定为 `type=linux-deb-manifest-v1`，并包含 `packageName`、规范产品 `version`、规范化 `architecture=amd64|arm64`、必需 `maintainer`、可选 `vendor`，以及 `maintainerScripts` 对象。控制字段先拒绝 NUL/控制字符与非法 UTF-8，再按 Unicode NFC、CRLF→LF、去除行尾空格并仅去除整体首尾空白进行规范化；规范化后 maintainer 必须非空且不超过 256 字节，vendor 不超过 256 字节，大小写保留并精确比较。维护脚本对象必须恰含 `preinst/postinst/prerm/postrm` 四个键；每个键值为 `{present:false}`，或 `{present:true,size,sha256}`。不存在脚本与大小为 0 的空脚本语义不同，空脚本仍须 `present=true,size=0` 并绑定空文件 SHA-256。任何脚本增删、替换、额外维护脚本、控制字段规范化差异或架构映射不一致均拒绝；V1 不执行未在固定键集合中的 DEB 触发器/维护代码。

### 8.5 Root 顺序更新与失效恢复

客户端从当前已信任 Root 版本 `n` 开始，只请求 `n+1`，不得跳号或直接信任 Timestamp/Snapshot 声称的任意 Root。候选 Root 必须满足：版本恰为 `n+1`；规范化内容同时达到旧 Root 对 Root 角色的阈值和候选新 Root 自身阈值；角色、keyId、算法、有效期和撤销字段 Schema 合法。验证通过后以原子写入持久化，再继续请求下一版本，单次最多连续处理 32 个版本以防资源耗尽；达到上限后在下一轮继续。

`404` 表示本次查询未发现 `n+1` Root，不是错误，但该响应必须使用 `Cache-Control: no-store, max-age=0`，CDN/代理禁止负缓存，客户端也不持久化“已是最新”结论；下次元数据刷新仍重新请求 `n+1`。网络/5xx 时保留旧 Root 并退避。当前 Root 过期后不得验证普通 Target/Release/Snapshot 元数据，但仍允许仅使用已持久化旧 Root 的公钥和阈值验证顺序的 `n+1` Root，以免合法轮换永久锁死客户端；除此之外不得放宽签名或跳号。若连续版本缺失、双阈值失败、同版本不同哈希或新 Root 已过期，停止升级并展示固定官方人工修复入口。人工恢复只能通过已完成对应平台真实性验证的更高版本迁移/修复安装器更新内置 Root，不得从网页、配置文件或未签名网络响应替换信任根。

clock-anomalous 模式增加一个严格受限的时间恢复例外：若当前持久化 Root 在异常发生前的 `lastTrustedTime` 上仍满足 `lastTrustedTime < root.expiresAt`，则即使错误本地墙钟使它“表面过期”，客户端仍可用该 Root 的 Timestamp 角色**仅**验证 11.1 中绑定本次 nonce 的 time-attestation；不得用该例外验证 Timestamp/Snapshot/Target/Release、决策或制品。若 `root.expiresAt <= lastTrustedTime`，连 time-attestation 也不得由该 Root 授权，只能按双阈值顺序验证 `n+1` Root；异常模式下候选 Root 的过期下界只用 lastTrustedTime 判断，不能用异常 localWallClock，且接受候选 Root 本身仍不授权普通元数据。每得到一个满足 `lastTrustedTime < newRoot.expiresAt` 且 Timestamp key 未 revoked 的更高 Root，就可仅用其尝试一次新 nonce time-attestation；建立新锚点后继续顺序获取 Root，直到得到在新 trustedNow 上未过期的当前 Root，再验证完整元数据链。若新可信时间表明已取得 Root 仍过期且没有合法下一版，则停止升级并人工恢复。该例外不能降低 Root 版本/哈希高水位，也不能接受已 revoked 的 Timestamp key。

## 9. 后台领域模型与生命周期

### 9.1 五类独立状态

不得使用一条“版本状态”同时表示上传、审批和投放。状态分别为：

1. **Artifact**：`uploading → validating → valid | rejected`，以及 `valid → quarantined`。内容一旦为 `valid` 即不可修改；`rejected` 或 `quarantined` 后只能重新上传为新 Artifact，不能原地恢复。
2. **Release**：`draft → assembled → closed`。只作为同一产品版本的多平台逻辑分组；Release 的“目标集合”指不可变的逻辑 targetKey 集合，同一 targetKey 可存在多个只读 Release Target 审批修订。Release closed 后不得新增 targetKey 或修改说明，但可在既有 targetKey 下为 preview→stable 资格、驳回重提或业务字段变化创建新的 Release Target ID；关闭后新增的修订不改变 Release 的 `closed` 状态，并按 Release Target 自身状态独立审批。若需新增平台目标则创建新 Release。Release 不承载“全部平台一起批准/吊销”的含义。
3. **Release Target**：`draft → in_review → approved | rejected`，以及 `approved → revoked`。每个 `releaseId + targetKey` 独立引用有效制品并配置最低来源版本、迁移、平台身份及不可变 `approvalClass=preview|stable`；驳回后修改必须创建新 ID 并重新审核。preview 级只能被 internal/beta Deployment 引用；stable Deployment 只能引用按 stable 双人规则批准的 stable 级 Release Target。预览制品晋升 stable 时必须克隆为新的 stable 级 `releaseTargetId`、重新执行稳定渠道闸门和双人审批，不能继承 preview 批准。某目标 rejected/revoked 不自动改变其他目标状态；全产品安全事件可在一个审计事务中批量撤销多个 Release Target。
4. **Deployment**：`draft → in_review → scheduled | rejected`，`scheduled → active ↔ paused`，且 `scheduled | active | paused → withdrawn | superseded`。每个已批准 Release Target 在每个渠道中创建独立 Deployment，审核其渠道、策略和投放约束，并为该 `deploymentId` 生成 Release Manifest；稳定、强制和紧急修复按第 4 章双人审核。恢复 paused 必须经过权限校验、写入原因并重新满足发布闸门。
5. **Rollout**：`draft → running ↔ paused`，且 `running | paused → completed | aborted`。记录不可变分桶参数、阈值和批次历史；质量阈值自动冻结扩量后须由有权限人员复核是否暂停，进入 paused 后只能由有权限人员在确认指标恢复和处置记录后手动继续。

Release Target 和 Deployment 是不可变业务修订：`rejected`、`approved`、`revoked`、`withdrawn` 或 `superseded` 实体的业务字段都不得原地修改或回到 draft。驳回后重提，或批准后更改制品、`minimumSourceVersion`、迁移声明、渠道、策略、平台身份等任一业务字段，必须克隆为新的 `releaseTargetId` 或 `deploymentId`，从 draft 开始并重走完整校验、审批和签名；旧审批不继承，旧实体只读保留审计。唯一例外是 8.1 定义的不改变业务语义的元数据机械续签，它只提升 Manifest 修订，不改变实体 ID 或状态。

状态不变量必须由数据库约束和服务层共同执行：Release **首次**进入 closed 时，目标集合必须已冻结、至少含一个有效 Release Target，且当时已存在的目标中不得有 `draft/in_review`；关闭后为既有 targetKey 新增的 Release Target 修订可以处于 `draft/in_review`，但不回退或改变 Release 状态，也不能在自身 approved 前被 Deployment 引用。Release Target 只有在引用的 Artifact 全部 valid、Package Manifest 身份一致并通过对应 `approvalClass` 闸门时才能 approved；Deployment 只有引用 approved 且渠道资格匹配的 Release Target 才能 scheduled/active；Rollout 只能绑定 active Deployment，目标、渠道和 `targetKey` 必须完全相同；任何依赖实体 quarantined/revoked/withdrawn 时，新的决策与授权立即失败关闭。

“暂停”是 Deployment/Rollout 上可恢复的投放操作；Deployment 的 `withdrawn` 是不可直接恢复的渠道投放撤回，Release Target 的 `revoked` 是跨投放的安全吊销。后两者都必须通过创建更高版本的新 Release Target/Deployment 替代，不得原地恢复。平台不提供指向低版本的服务端回滚发布；问题版本通过更高版本的前向修复发布替换。

### 9.2 基线与并发约束

- 每个 `channel + targetKey` 至多有一个基线发布，可同时有至多一个正在运行的候选 Rollout。渠道初始化、基线被暂停/撤回且修复版尚未激活时允许没有基线，此时返回 `channel_has_no_baseline`，不得回退到其他渠道或问题版本。
- 未命中候选灰度时返回适用的基线；暂停候选后新检查立即回到基线。若基线本身不可用，则返回无更新及稳定原因码，不得把 superseded 版本临时提升为基线。
- 候选晋升为基线必须原子完成：新基线生效、旧基线标记 superseded、候选 Rollout completed。
- 桥接资格使用独立附属状态资源 `BridgeEligibility(deploymentId, state, revision, reason, approvedBy, changedAt)`，不修改不可变 Deployment 业务字段。状态为 `disabled | enabled`：`disabled → enabled` 必须完成路径影响评估和人工审批；`enabled → disabled` 可因安全处置立即执行、不可原地恢复，重新启用必须创建更高 revision 并重新审批。已完成正式发布的 superseded Deployment 可在资格为 enabled 时作为历史桥接；当前 active 基线无需该资格也可作为命中候选设备的中间跳。paused、withdrawn、其 Release Target 已 revoked 或制品被隔离的版本始终无桥接资格。BridgeEligibility 是审计附属状态，不计入五类核心实体，也不进入 Release Manifest 业务字段。
- 同一 `channel + targetKey + releaseSequence` 唯一；所有状态写入要求幂等键和乐观并发版本。
- 已签发的下载地址不表示安装权利，安装权利由安装前复核的短期令牌决定。

激活、暂停、撤回和 Target 级联发布采用显式两阶段可见性。普通激活先在不可见区提交领域状态和不可变 Manifest、确认全部对象可从生产端读取，再签发 Snapshot、Timestamp，最后原子切换“当前选择世代”；检查服务只能返回当前世代 Snapshot 中存在的 Manifest。安全暂停/撤回先在授权/检查拒绝表中原子置 deny，使新决定和 validate/consume 立即失败关闭，再发布移除对象的新 Snapshot/Timestamp，最后执行 CDN 失效；即使元数据传播稍有延迟也不得继续签发安装权。

### 9.3 自动校验和发布闸门

上传后至少执行：目录和 Schema、ZIP 安全、产品/版本/目标一致性、文件存在性、大小、SHA-256、各级签名、增量参考重建、Windows Authenticode、macOS 签名与公证、Linux 包元数据、SBOM、高危漏洞、敏感信息和安装/健康观察测试结果检查。任一关键项失败不得进入审核。

稳定发布前必须证明对应平台能够可靠执行安装、重启后判定实际版本并由外部助手完成健康观察。首期不提供恢复包或自动恢复旧版本；含不可逆数据迁移的版本不得配置静默安装、限期安装或强制安装。

### 9.4 最低请求版本（最低来源版本）配置

- `minimumSourceVersion` 为可选字段，表示可以直接升级到该目标版本的最低当前安装版本，比较采用 SemVer 且包含等于边界；缺省时统一按 `0.0.0` 处理，与显式填写 `0.0.0` 语义完全相同。
- 管理后台字段名称显示为“最低请求版本”，协议字段固定为 `minimumSourceVersion`；“请求版本”指检查请求中的 `currentVersion`，不是 Updater 协议版本。
- 显式填写时必须是合法 SemVer，且严格小于该发布的 `targetVersion`。管理后台允许留空，但必须在审核页和可达性矩阵中以“缺省（按 `0.0.0`）”展示，不能把缺省显示成未知或未配置。
- 多平台发布可以分别配置该字段；管理后台默认复制同一值或缺省状态到各目标，但必须逐目标展示和审核。
- Release Manifest 允许省略该字段；服务端与客户端必须先将缺省值解析为 `0.0.0` 再执行所有比较。缺省和显式 `0.0.0` 虽然语义相同，但序列化内容不同；Release Target 批准后在两种表示之间切换属于业务字段变化，必须创建新 `releaseTargetId` 及新 Deployment，重新审核和签名，不得仅修改原 Manifest。
- 来源兼容性是连续区间产品约束：一个发布可直接接受的来源集合固定为 `[resolvedMinimumSourceVersion, targetVersion)`，不支持区间内排除单个版本、不支持多个离散区间，也不定义 `allowedSourceRanges`。发布方必须通过兼容和迁移测试证明区间内受支持版本均可升级。
- 平台不提供定向降级。无论 `minimumSourceVersion` 是否缺省，只有 `currentVersion < targetVersion` 才可能返回该发布；首期安装失败后也不自动恢复或安装旧版本。
- stable 发布页必须展示“可直接升级来源范围”和从当前已验证 Target Metadata 的 `supportFloorVersion` 到当前候选的可达性矩阵。存在路径断点时禁止发布，除非同时提供经过审批并公告的人工升级方案。
- 曾在同一渠道达到 100%、成为过该渠道基线、后续被 superseded 且未撤回的版本，可经审批创建 `BridgeEligibility=enabled`。未达到 100% 的灰度版本默认不得取得历史桥接资格；桥接版本不得跨渠道复用。
- 仍被任一升级路径引用的桥接 Deployment、其 Release Target、Manifest 和目标完整包不得物理删除或转冷存储到不满足升级 SLA 的介质。
- 桥接 Release Manifest 必须始终处于有效期内。后台应在过期前 14 天告警并按原发布阈值续签：保持 `deploymentId`、`releaseId`、`releaseTargetId`、目标版本、`minimumSourceVersion` 的原序列化表示、Target/Package Manifest 引用、策略和制品不变；允许变化的字段仅为更高 `releaseManifestVersion`、新的 `issuedAt`、新的 `expiresAt` 和由此产生的 signatures。续签必须与生成引用新版本/长度/哈希的更高 Snapshot 原子发布后才生效，并写入审计；任何业务字段变化仍按新修订重新审核。过期且未续签的版本立即退出候选集合并触发路径断点告警。

9.4 中“Target 引用不变”只适用于单独续签桥接 Release Manifest；若同时续签 Target Metadata，必须按 8.1 对全部有效 Deployment 级联修订，此时 Target 引用是除时间/版本/签名外唯一允许的机械变化。

### 9.5 问题版本处置与前向修复

- 升级平台不创建、不签发、也不选择目标版本低于客户端实际当前版本的发布。
- 发现当前版本存在功能或安全问题时，运维先暂停该 Deployment；确认不得继续在该渠道分发时将 Deployment 转为 `withdrawn`，制品或签名身份存在跨渠道安全风险时将对应 Release Target 转为 `revoked`，并停止新的下载地址和安装授权。
- 同时创建修复版本 `Vfix`，要求 `Vfix > Vbad` 且使用更高 `releaseSequence`。修复版本仍走完整构建、签名、测试、双人审核和灰度流程，不设置特殊的降级通道。
- 激活修复版本时，必须在同一后台事务中为同渠道问题 Deployment 写入更高 revision 的 `BridgeEligibility=disabled`；若 Release Target 已被安全吊销，其所有渠道 Deployment 都同时失去桥接资格。所有可达性矩阵重新计算，任何来源版本的桥接路径都不得经过已知问题版本。
- 修复版本解析后的 `minimumSourceVersion` 必须覆盖受影响版本；若字段缺省，则按 `0.0.0` 覆盖全部更低来源版本。需要通过桥接路径到达时，路径不得经过已暂停、撤回、吊销或已知存在问题的版本。
- 单次安装事务失败后由外部升级助手确认实际安装状态、记录失败并提供人工修复入口，不自动恢复安装前版本，也不允许服务端借此向客户端下发低版本。

## 10. 发布、灰度与策略

### 10.1 渠道

首期支持 `internal`、`beta`、`stable`。`internal` 和 `beta` 均为面向所有用户公开展示、由用户自愿加入或退出的预览渠道，不以员工身份、白名单、硬件 SN 或其他准入条件限制；`internal` 更新频率更高、成熟度最低，`beta` 用于公开测试，`stable` 为默认渠道。客户端不得默认、静默或远程把用户切入预览渠道；加入前须展示稳定性和数据风险说明并取得明确同意。退出预览渠道不触发降级：若当前版本高于 stable 基线，客户端保持当前版本，直到出现更高且兼容的 stable 版本。每个渠道按平台目标独立投放，一个目标失败不自动阻塞其他目标，但 stable 产品发布页必须清晰展示各目标状态。

per-machine 安装的渠道选择是 `installationScopeId` 级共享设置，不是 `installId` 或单个用户级设置。它保存在系统级受保护记录中，由升级助手串行化读取/写入；任一会话看到的当前渠道必须一致，后写入只能基于最新 `channelRevision` 做 CAS。每次成功写入 revision 严格递增，即使 `beta → stable → beta` 也不得复用旧值。切入 internal/beta 必须由本机管理员在系统授权后确认共享风险说明，并记录操作者、本机时间、渠道、同意文案版本和 revision；切回 stable 同样要求管理员确认，不能由普通用户静默覆盖另一会话的选择。渠道改变后通知所有活动会话；检查请求、decisionToken、遥测凭据和安装授权全链路绑定 revision，validate 前助手重新读取受保护记录并精确比较，因而旧渠道或旧 revision 的未消费决定失效。未来支持 per-user 安装时须提升安装范围与渠道协议版本，不能复用此共享语义。

### 10.2 硬件 SN 灰度

硬件 SN 是可选的灰度分组输入，不是设备认证信息。客户端能够读取时可在检查请求中携带；服务端不得假定其真实、唯一、不可伪造或稳定，也不得使用 SN 决定调用方是否有权检查、下载或安装。规范化规则存于唯一的签名协议注册表，Rollout 冻结 `snNormalizationVersion + snNormalizationProfileSha256`。V1 算法固定为：输入必须是 1～256 字节合法 UTF-8 且只含 ASCII 字母、数字、`.`、`_`、`:`、`-` 及首尾 ASCII SP/HT；去除首尾 SP/HT、把 a-z 转为 A-Z，内部字符和分隔符一律保留，结果须为 1～128 字节。V1 不删除任何展示分隔符、不做 Unicode 归一化、截断、模糊匹配或设备名替代；超限/非法即按 SN 缺失处理。Windows SMBIOS、Linux DMI 和 macOS 受信序列号读取器共用注册表固定测试向量，未来若需按来源移除分隔符必须发布新版本/哈希并创建新 Rollout。

SN 缺失、读取失败或格式非法时，检查请求仍正常处理：需要 SN 定向名单或低于 100% 百分比分桶的候选 Rollout 视为未命中，然后继续选择基线或兼容桥接版本；Rollout 已达到 100% 时不要求 SN。任何 SN 分组的包含/排除名单都只作用于当前候选 Rollout，不得封禁检查接口，不得阻止基线、兼容桥接或已面向全量设备发布的前向安全修复。

安全边界明确如下：SN 只参与“本次向客户端展示哪个候选版本”的分组计算，本身不授予下载或安装权。检查完成后，由服务端签发的短期 `decisionToken` 承载本次决定；后续文件信息和安装复核验证令牌、发布状态、版本、目标、制品哈希及安装实例绑定，不再次以 SN 作为条件。

百分比分桶算法冻结为：

```text
message = UTF8(RFC8785({"normalizedHardwareSn":normalizedHardwareSn,"releaseId":releaseId}))
digest  = HMAC-SHA256(bucketKey, message)
bucket  = UINT64_BE(digest[0..7]) mod 10000
eligible = bucket < percentageBasisPoints
```

`releaseId` 必须匹配 ASCII 正则 `^rel_[A-Za-z0-9_-]{1,64}$`，不得包含换行、控制字符或 Unicode 等价表示；`normalizedHardwareSn` 必须先按冻结的规范化注册表生成唯一 Unicode 标量序列，再与 releaseId 组成上述 RFC 8785 对象，禁止自行拼接分隔符。`bucketKeyId`、`snNormalizationVersion` 和 `snNormalizationProfileSha256` 都在 Rollout 创建时锁定，运行中不得轮换或修改；客户端上报原始 SN，服务端必须按该 Rollout 冻结且哈希匹配的注册表计算。百分比只允许单调增加，因此扩量不会把已命中设备移出。百分比分桶密钥与包含/排除组成员 HMAC 密钥必须是两个用途隔离的 KMS 密钥：前者按 Rollout 固定，后者按名单版本固定并记录 `groupKeyId`，不得把用不同密钥计算的 HMAC 值直接比较。后台只展示 keyId。相同发布和 SN 重复检查必须得到相同分桶，重装和 `installId` 变化不得改变结果。该 HMAC 只提供稳定分桶和服务端隐私保护，不证明设备身份；伪造或复制 SN 最多改变候选灰度分组，不能绕过制品签名、版本兼容、发布状态、短期令牌或安装授权校验。

任一 SN 分桶或名单 HMAC 密钥疑似泄露时，必须立即暂停受影响 Rollout、停止扩量并撤销其未消费候选决定；处置只能创建使用新 keyId、新名单版本和新 Rollout ID 的全新灰度，重新从初始比例开始。平台不承诺泄露前后分桶集合连续，也不得在运行中替换密钥后宣称保持单调性；基线和前向安全修复仍按非 SN 安全规则可用。

判断优先级固定为：发布/制品安全撤回 → 协议与平台目标兼容性 → 候选 Rollout 的 SN 排除组 → SN 包含组 → 百分比分桶 → 确定候选或基线期望终点 → `minimumSourceVersion` 来源兼容性与同渠道桥接路径选择。排除组优先于包含组，但两者都只决定是否命中候选 Rollout。强制或限期策略不得绕过来源版本、平台兼容性或候选灰度范围；SN 未命中时仍按正常规则返回可用基线或通往该基线的兼容桥接版本。

### 10.3 质量阈值、自动停止扩量与人工暂停

默认按安装授权已成功消费且观察满 30 分钟的成熟事务队列计算；滚动统计取最近 60 分钟内的成熟事务，至少 100 个才执行比例质量动作：`安装失败率=(进入 health_check_started 前的 failed_manual_repair_required + 消费后 30 分钟仍无合法终态且未进入健康检查的事务)/(成熟已消费事务 - abandoned_before_install)`；`健康失败率=健康检查失败/已进入 health_check_started 的成熟事务`。`abandoned_before_install` 单列为启动前放弃率，不计入安装失败率，但超过 5% 同样告警。安装失败率超过 5%、健康检查失败率超过 2%，或任一指标超过同平台基线 3 倍时，系统自动冻结继续扩量并告警，但**仅凭客户端遥测不得自动把 Rollout 改为 paused 或写入安装授权 deny**；有权限人员复核制品/平台/版本分布、网络簇异常及服务端安装授权记录后，才可执行人工暂停并原子写入 deny、提升 selectionGeneration。“3 倍基线”仅在同平台基线同期样本不少于 100 且基线率不低于 0.1% 时计算，避免零基线除法和小样本放大，否则只用绝对阈值。迟到的合法事件可修正统计，但不能自动恢复已冻结扩量或已暂停投放。阈值和观察窗口可按产品调整，但变更须审核、版本化和审计；样本不足只告警、不扩量。

原因是公开、无需设备认证的更新检查允许攻击者生成大量伪安装标识，事件凭据只能证明“事件属于一次服务端签发的会话”，不能证明真实独立设备或诚实执行。质量聚合必须按 installId、installationScopeId、网络前缀/ASN 簇、平台、来源/目标版本和文件源分层去重，并给单一网络簇设置贡献上限；代理池、标识批量变更或分布异常只降低数据可信度并触发安全告警。安全撤回、密钥吊销、制品隔离和服务端已确认的错误包不依赖该质量遥测，可按 9.5 的问题版本处置流程立即 deny。

建议阶梯为发布者实验室验证 → 1% → 5% → 20% → 50% → 100%，每级至少观察 2 小时且满足最小样本量。“发布者实验室验证”是发布前测试闸门，不是只允许员工进入的 `internal` 渠道，也不改变 `internal`/`beta` 的公开自愿属性。候选晋升为基线后的 24 小时继续应用同一质量阈值；触发阈值时自动冻结继续扩量并告警，是否暂停该 Deployment 仍须人工复核。

公式中“安装失败率”只计算安装事务未进入可健康观察目标版本的失败；已进入 `health_check_started` 后的失败只计入“健康失败率”，不重复计数。扣除 `abandoned_before_install` 后分母小于 1 时该指标不计算、只告警数据不足；不得除以零或把其视为 0% 成功率。

### 10.4 更新策略拆分

策略必须拆成三个正交维度：

| 维度 | 枚举 | 含义 |
|---|---|---|
| `enforcementPolicy` | `optional`、`recommended`、`deadline`、`mandatory` | 提示和使用限制强度 |
| `downloadPolicy` | `manual`、`autoUnmetered`、`autoAny` | 是否自动下载 |
| `installPolicy` | `interactive`、`autoWhenIdle` | 是否在满足空闲/权限条件后自动安装 |

`enforcementPolicy` 的可执行语义固定如下：

| 策略 | 提示与关闭 | 延期/重提示 | 限制开始时间 |
|---|---|---|---|
| `optional` | 首次发现展示，可关闭；之后仅在设置页保留 | 同一版本关闭后 7 天内不主动再提示 | 永不限制功能 |
| `recommended` | 可关闭，但明确标记建议更新 | 最多延期 24 小时；到期后再次提示，仍可继续延期 | 永不限制功能 |
| `deadline` | 截止前按 recommended；须显示倒计时 | 可延期但不得超过 `deadlineAt`；宽限期内每 4 小时提示 | `deadlineAt + gracePeriodSeconds` |
| `mandatory` | 首次接受有效 Manifest 后立即显示不可永久关闭的提示 | 仅在 `gracePeriodSeconds` 内延期 | `enforcementEffectiveAt + gracePeriodSeconds` |

`deadlineAt` 必须晚于首次批准的 Manifest `issuedAt`，`gracePeriodSeconds` 范围为 0～604800 秒。`deadline` 必须设置 deadlineAt，`enforcementEffectiveAt=null`；`mandatory` 禁止设置 deadlineAt，必须设置一次审批后不可变的 `enforcementEffectiveAt`，且不得早于首次批准时间。optional/recommended 的两个时间字段均为 null。元数据续签不得改变 `deadlineAt`、`enforcementEffectiveAt` 或重置客户端已持久化的限制状态。限制生效后只可阻止创建新任务，仍须允许完成/保存当前内容、导出、查看更新和人工修复指引、修改网络设置及正常退出，不能杀死运行任务。internal/beta 只允许 optional/recommended，且不得远程强制安装。

可信时间使用 8.2 的最后可信时间单调推进：系统时钟异常时不得首次启用新的 deadline/mandatory 限制，也不得因拨回时钟解除已经持久化生效的限制；离线时只有客户端此前已验证并持久化该 Manifest 且可信时间已达到限制点才执行限制。限制的离线租期不得超过该 Release Manifest `expiresAt`；到期仍无法取得可信元数据时失败开放并持续提示联网/人工修复。客户端实际版本已达到目标，或最新可信 Snapshot 已确认该 Deployment 处于 paused/withdrawn/superseded、Release Target revoked 或制品 quarantined 时，立即取消该 Deployment 带来的未完成功能限制；若有替代的有效强制 Deployment，按替代者独立计算。任何策略均不得绕过 UAC、管理员授权或 macOS 系统权限。

`autoWhenIdle` 仅可用于 stable、`dataMigration.mode=none|backwardCompatible`、无需新的系统提权交互且用户已显式开启自动安装的目标，并必须在失败后由外部升级助手确认终态和通知用户；首期不因强制策略自动勾选该选项。`autoAny` 必须单独取得计费网络下载同意。策略字段组合非法时后台、Schema 和客户端三处都必须拒绝，不能猜测优先级；这些策略不表示具备自动回滚能力。

### 10.5 最低请求版本与桥接选择

服务端始终使用本次检查请求中的 `currentVersion` 作为“原始来源版本”，不得在同一请求内假定客户端已经安装任何中间版本。检查请求的 `currentHelperFacts`、`currentLauncherFact`、两个协议版本及受保护事实 revision 由受控助手按 11.2 从受保护安装记录与实际文件共同复核，进入 capabilitiesSha256/currentUpdaterFactsSha256，不信任 UI 自报；服务端还须按“由已签名 Manifest 派生并由服务端账本管理的 Updater Source Facts Registry”复核整组事实。选择前先以当前已验证 Target Metadata 复核目标和宿主约束；`currentVersion < supportFloorVersion` 时固定返回 `no_compatible_update` 及不受请求参数控制的官方人工升级指引，不得用 `minimumSourceVersion=0.0.0` 绕过该下界。选择规则如下：

1. 在同一 `productId + componentId + channel + targetKey` 内先排除 paused、withdrawn、revoked、签名元数据过期、制品隔离和宿主平台不兼容的发布；此步暂不按 `minimumSourceVersion` 或当前 updater 能力排除期望终点。遇到当前不支持的顶层协议/必需语义时只标记 `capabilityBlocked`，仍保留该发布用于确定期望终点和搜索应用桥接路径；不得直接下发，最终无能力路径时按第 9 步返回 426。
2. 按候选 Rollout 的 SN 规则确定本设备是否命中候选；命中时以候选发布为期望终点，否则以当前 active 基线为期望终点。渠道没有安全可用的期望终点时返回 `channel_has_no_baseline`；不能为了找到桥接版本而让未命中灰度的设备进入候选路径。
3. 将期望终点缺省的 `minimumSourceVersion` 解析为 `0.0.0`。若 `targetVersion == currentVersion`，返回 `already_latest`；若 `targetVersion < currentVersion`（例如退出预览渠道后当前版本暂时领先），返回 `current_version_ahead_of_channel` 且绝不降级；只有 `currentVersion >= resolvedMinimumSourceVersion` **且第 8 步的当前来源→期望终点整条边兼容**时，才直接返回期望终点，否则继续搜索前置跳/历史桥接。
4. 设备命中候选但候选不能直接安装时，先允许同一渠道当前 active 基线作为候选前置跳，只要基线自身安全有效、`currentVersion < baselineVersion`、当前版本满足其来源下界且基线到候选存在完整可验证路径；此时返回基线但保留“本次命中候选”的服务端分桶快照，`selectionMode=candidate_prerequisite`。安装基线后必须以新 `currentVersion` 发起全新检查；只有新检查仍按同一活动 Rollout 规则命中时才返回候选，若 Rollout 已暂停/完成/变更则服从当时状态。未命中候选的设备不得因该路径进入候选。
5. 若第 4 步不适用，再只考虑同一渠道内 BridgeEligibility 为 enabled、自身安全状态有效、`currentVersion < targetVersion`、当前版本满足其来源下界，并且位于一条完整可验证路径通往期望终点的 superseded 历史正式发布。可达性以各跳的连续来源区间、安全状态、目标一致性和第 8 步的 updater 能力事实计算；中间版本安装后获得的 helper/launcher、备份注册表及算法能力取由该版本已签名 Manifest 确定并登记在服务端 Updater Source Facts Registry 的事实，不能继续沿用原来源能力或相信客户端预测。
6. 多个历史桥接版本满足第 5 步时，按 `targetVersion` 的 SemVer 从高到低排序，目标版本相同时按 `releaseSequence` 从高到低排序，返回第一项；不得选择虽版本更高但会形成死路的历史发布。
7. 返回已命中的候选，或当前没有活动候选而直接返回基线时，`selectionMode=direct`；存在活动候选但设备未命中而返回基线时为 `rollout_baseline`；命中候选但先返回当前基线时为 `candidate_prerequisite`；返回 superseded 历史发布时为 `compatibility_bridge`。后两种中间跳都不能被用来绕过候选灰度范围。
8. 每条直接边和桥接边必须同时满足 updater 可达性：当前受保护 helper 与目标包 helper 的 handoff 协议兼容，launcherProtocolVersion 能验证并授予两者 `helperOwnershipFencingToken`，且目标不含助手时 transactionHelperId 指向的当前 helper 满足 minHelperProtocolVersion；目标 `backupPolicy=required` 时，来源 updater 的 `backupScopeRegistryVersion + backupScopeRegistrySha256` 必须与目标静态策略精确相等，且同时支持目标 `backupHashAlgorithm` 与 `backupContainerFormat`。V1 还禁止在同一次安装事务中同时执行 required 写冻结备份、助手字节安装或助手所有权切换：对 required 目标，服务端只能把按来源受保护 helper 事实计算为 `helperBytesInstallRequired=false` 且 `helperHandoffRequired=false` 的 packageId 放入决定允许集合；`helperUpdateRequired` 只是两者逻辑或的兼容汇总字段。没有这种完整包或增量包时该边不存在，必须先选择一个 `backupPolicy=none` 且携带新助手或完成助手接管的应用桥接版本，安装成功并重新检查后才能进入 required 目标。任一条件不满足时该边不存在；应用版本桥接可通过安装已签名的新 updater 能力建立下一条边。
9. 服务端先计算忽略第 8 步 updater 能力、但保留版本/安全/平台约束的“结构路径集合”：集合为空时返回 `updateAvailable=false, reason=no_compatible_update`；集合非空但加入第 8 步后全部路径仅因 updater 协议、required 与 helper update 互斥、备份注册表/哈希算法/容器格式或其他 requiredCapabilities 被消除时，返回 HTTP 426 `updater_protocol_upgrade_required` 和固定人工升级指引，且其优先级高于无更新 reason；至少一条完整能力路径存在时按第 4～6 步返回下一跳。不得把不兼容的新版本、无后续路径的桥接版本或其他平台制品返回给客户端。

示例：已发布 `1.1.0(min=0.8.0)`、`1.2.0(min=1.0.0)`、`1.3.0(min=1.2.0)`。版本 `0.9.0` 请求时返回 `1.1.0`；安装成功后以 `1.1.0` 重新检查并返回 `1.2.0`；再安装后返回 `1.3.0`。每次响应只返回一个目标版本，客户端不得在一次事务中静默执行整条升级链。

灰度示例：active 基线为 `1.2.0(min=1.0.0)`，候选为 `1.3.0(min=1.2.0)`，命中候选的 `1.0.0` 设备先得到 `1.2.0` 与 `selectionMode=candidate_prerequisite`；安装后重新检查且仍命中才得到 `1.3.0`。未命中的 `1.0.0` 设备只得到普通基线并使用 `rollout_baseline`，不会进入候选。

若某发布省略 `minimumSourceVersion`，上述算法将其视为 `min=0.0.0`。平台不会表达“除 `1.0.3` 外均兼容”等单版本例外；一旦声明最低版本，该版本到目标版本之间的所有更低 SemVer 都被视为连续兼容来源。

V1 不提供独立的 helper-only 网络发布或授权类型。每个 Release Target 的批准闸门必须枚举 `[resolvedMinimumSourceVersion,targetVersion)` 内所有实际受支持来源版本及其随包安装的 helper/launcher 协议事实，证明每一来源都可直接完成目标包的受控 handoff；缺少测试事实、跨越超过一个 helper 协议代际或 launcher 不兼容时不得批准该边。需要跨多代，或某 required 目标对该来源必须更新助手时，必须发布应用版本严格递增的一个或多个 `backupPolicy=none` 桥接 Release，每跳携带可由当前/前一 helper 协议接管的新助手；required 目标本跳只能使用已经满足要求的助手。可达性矩阵同时计算应用版本边、助手/launcher 协议边以及“required 与 helper update 不共存”约束，不能只看 SemVer。检查、decision、validate 和安装授权均绑定受保护记录中的当前 helper/launcher 版本、协议和身份摘要，任一现场事实漂移即重新检查或人工修复。

### 10.6 存量客户端迁移切换

存量客户端迁移采用唯一的 `migrationBootstrapVersion`，流程边界如下：

1. 原有升级后台继续识别其支持的存量版本，并只向这些客户端下发迁移引导版本；这是旧升级系统内的一次普通向上升级，不调用新平台 API。`migrationBootstrapVersion` 必须严格高于迁移覆盖清单中每个规范化来源版本；无法满足的较新或异常来源必须在清单中显式排除并进入人工迁移，不得对其下发同版或降级引导包。
2. 迁移引导版本通过对应平台真实性验证，内置新 Updater、初始可信 Root Metadata 和固定控制面地址 `https://update.pinvou.com`，不得继续使用域名引导，也不得从网络下载或替换初始信任根。
3. 迁移状态按 `legacy → transitioning → new-v1` 单向变化并原子持久化。迁移引导安装器是“既有安装不得补生成 installationScopeId”规则的唯一初始化例外：它必须以安装器/系统权限，在 `transitioning` 阶段幂等生成一次随机 UUID `installationScopeId`，并把 `channel=stable`、`channelRevision=1`、canonicalAppVersion、安装范围/路径、平台真实性、稳定 launcher 与当前 helper 的完整事实、`protectedUpdaterFactsSchemaVersion=1`、`protectedUpdaterFactsRevision=1`、`helperOwnershipFencingToken` 高水位初值、新协议记录 Schema 版本和 `migrationSource=legacy` 一并原子写入系统级受保护安装记录；所有本机用户读取同一记录。重复运行必须复用已成功写入的同一 ID，不得产生第二份记录。只有操作系统确认安装成功，且迁移引导版本首次启动完成版本校验、Root 自检、Updater/受保护记录/权限自检、必要配置迁移和健康检查后，才在同一持久化事务中提交 `new-v1`。安装失败由原有升级机制按既有能力重试或进入人工修复，其回滚能力不属于本平台需求；安装成功但首次初始化失败或断电时保留 `transitioning`，下次启动从原子日志恢复或重试初始化，必要时只允许旧系统重发同一迁移引导版本或提供人工修复，不得下发其他业务版本。提交 `new-v1` 后记录缺失、权限异常或被篡改不得重新生成 ID，只能进入人工修复。
4. `new-v1` 一旦提交，后续检查、下载信息、安装复核和事件上报只访问新平台，不再回退旧端点。新平台故障时按正常退避处理，不得因网络错误、配置丢失或本地状态被删除而重新启用安全能力较弱的旧协议；高于迁移引导版本的客户端无条件使用新协议。
5. 迁移引导版本首次通过新平台检查时，按全量包客户端处理：增量能力数组为空，本地没有可信基础制品时不作推断，按新平台规则下载并验证目标完整包。
6. stable 对应目标的初始 Target Metadata 必须把 `supportFloorVersion` 设为 `migrationBootstrapVersion`；首个 stable 基线必须允许该版本直接升级，或存在从该版本出发且完整可验证的桥接路径。更早的存量版本不直接请求新平台，因此不纳入新平台可达性矩阵。

迁移引导版本应使用一个统一产品 SemVer，并同时声明原有升级系统可识别的 `legacyVersion`；两者的固定映射随迁移包发布记录审计，切换后只以 SemVer 请求新平台。存在存量客户的平台目标分别制作对应安装包，但不得为不同客户创建语义不同的迁移版本。原有升级后台在迁移观察期内不得再向存量客户端发布高于迁移引导版本的其他业务版本。达到约定迁移率并完成客户通知后才可停止旧服务；仍无法通过旧方式迁移的客户进入人工升级流程。

### 10.7 发布说明和迁移声明

发布说明必须提供 `zh-CN`、`en-US`、`ja-JP`，回退顺序为精确 locale → 同语言默认 → `zh-CN` → `en-US`。内容包含功能修复、安全说明、已知问题、重启要求、大小和预计耗时。

发布说明作为 Release Manifest 的签名内容，每种语言最多 32 KiB。客户端只渲染受限 Markdown：禁止原始 HTML、脚本、远程图片、自动打开链接和自定义 URI scheme；外部 HTTPS 链接必须显示实际主机并由用户主动确认，避免发布说明成为代码执行或凭据钓鱼入口。

每个发布必须声明静态 `dataMigration` 条件 Schema：`mode` 取 `none|backwardCompatible|backupRequired|irreversible`，并包含 `sourceAppVersionRange`、`backupPolicy=none|required`、`backupScopeIds[]`、可空的 `backupScopeRegistryVersion`/`backupScopeRegistrySha256`、`backupHashAlgorithm=none|pinvou-backup-manifest-v1`、`backupContainerFormat=none|pinvou-backup-container-v1`、`maxBackupBytes`、`maxBackupDurationSeconds`（0～86400）和 `snapshotVerification=none|hash-and-reopen`。V1 不声明独立数据 Schema 版本；`sourceAppVersionRange` 明确以升级助手从受保护安装事实复核的 `canonicalAppVersion` 为输入，取值只能为 `not_applicable`，或结构化对象 `{"minInclusive":"<semver>","maxExclusive":"<semver>"}`，禁止解析自由格式区间字符串。为避免两套来源兼容规则，`none` 必须使用 not_applicable；其余模式必须强制 `minInclusive == resolvedMinimumSourceVersion` 且 `maxExclusive == targetVersion`，否则 Release Target 不得进入审核、可达性矩阵不得建边、服务端不得选择该发布。`mode=none|backwardCompatible` 强制 `backupPolicy=none`；`mode=backupRequired` 强制 `backupPolicy=required`；`mode=irreversible` 必须显式二选一。policy=none 时 scope 为空、两个注册表字段均为 null、backupHashAlgorithm/backupContainerFormat 均为 none、maxBackupBytes/时长为 0 且 verification=none；policy=required 时必须给非空 scope、正整数注册表版本、64 位小写十六进制注册表 SHA-256、backupHashAlgorithm=pinvou-backup-manifest-v1、backupContainerFormat=pinvou-backup-container-v1、`1 MiB..50 GiB` 的 maxBackupBytes、正的最大时长和 `hash-and-reopen`。服务端只在 policy=required 时要求请求 updaterCapabilities 的注册表版本/哈希与发布要求精确相同，且能力数组分别包含该哈希算法与容器格式；不匹配时不得选择该直接边，应按 10.5 选择兼容桥接，仍无路径才返回 `updater_protocol_upgrade_required`。界面和审批必须显式标记 irreversible 及其备份选择。来源版本缺失、损坏、超出范围或与决定绑定版本不一致时，validate 必须失败，不得猜测数据版本。

静态配置与运行事实必须分层：Release Manifest、decisionToken 和 `installIntentSha256` 只绑定上述静态 `dataMigration` 全量字段，不得伪造尚未发生的实际备份结果；validate 请求和 installAuthorization 另绑定封闭 `backupFacts` 对象及其 RFC 8785 SHA-256。`backupFacts` 必含 `sourceAppVersion`、`hashAlgorithm`、`containerFormat`、可空的 `scopeRegistryVersion`/`scopeRegistrySha256`/`dataSourceRevisionSha256`、`actualBytes`、`durationMilliseconds`、可空的 `snapshotSha256`/`completedAt`。policy=none 时 sourceAppVersion 仍必填且等于助手复核版本，hashAlgorithm/containerFormat 均为 none，五个可空字段全部为 null，actualBytes=0、durationMilliseconds=0；policy=required 时算法/格式分别等于两个 V1 固定值且五个可空字段全部非 null，注册表身份必须与静态配置及 updaterCapabilities 相等，actualBytes 为非负整数且不超过 maxBackupBytes（空数据集合法），`0 <= durationMilliseconds <= maxBackupDurationSeconds*1000`，snapshotSha256/dataSourceRevisionSha256 为 64 位小写十六进制，completedAt 为 UTC RFC 3339。policy=required 的备份必须在系统权限授予后、全部数据源写冻结已生效时生成；备份成功前最后一次完整数据源 revision 复算属于同一次备份计时，成功后不再释放冻结或允许任何写入。validate 前助手只需以仍持有的原句柄/文件身份和该备份事务受保护记录中的 `quiesceFencingToken` 复核冻结连续性；若冻结、进程、句柄或身份发生变化，旧快照立即失去本次安装资格，必须走 `backup_invalidated`、删除快照及 keyRef，并重新取得权限和备份，不能靠一次未冻结 rehash 恢复资格。installAuthorization 必须绑定 `backupFactsSha256`，consume 时逐字段复核并验证冻结仍连续有效；任一静态身份、实际来源版本、算法/格式、数据源 revision、字节数、耗时、快照哈希、完成时间或冻结世代变化都必须失败。未满足条件字段即 Schema 失败。`backupPolicy=required` 必须在安装前完成可验证快照，但首期平台不自动使用该快照回滚程序或数据，只在人工修复时提供；`irreversible` 禁止自动、限期或强制安装，必须获得用户明确确认，并明确说明升级失败后平台不会自动恢复旧程序或旧数据。

`pinvou-backup-manifest-v1` 的数据源 revision 算法固定如下：受控助手以已打开目录/文件句柄遍历每个 `backupScopeId`，不跟随链接；只允许普通目录和普通文件。每项投影为 `{scopeId,relativePath,type,size,contentSha256}`，目录固定 `type=directory,size=0,contentSha256=null`，文件固定 `type=file` 并对完整内容计算小写十六进制 SHA-256。relativePath 使用相对 scope 根的 `/` 分隔 UTF-8 NFC 字符串，保留实际大小写，禁止空段、`.`、`..`、NUL 和控制字符；各平台必须拒绝会映射到同一实际对象或在该文件系统比较规则下冲突的两个路径。条目先按 `scopeId` ASCII、再按 relativePath UTF-8 字节、最后按 type ASCII 升序，拒绝重复后，对 `{"algorithm":"pinvou-backup-manifest-v1","entries":[...]}` 求 RFC 8785 UTF-8 字节的 SHA-256，所得即 `dataSourceRevisionSha256`。`actualBytes` 固定为全部 file 条目 size 的精确和，不包含目录、清单、压缩或加密开销；即使为 0 也必须生成可验证快照。单 relativePath 最长 1024 UTF-8 字节、条目最多 100,000、规范清单最多 16 MiB，超过即在 validate 前失败。哈希前后复核文件身份、大小和修改世代；遍历期间变化即失败并重试，不得只依赖 mtime、目录时间或文件名。

备份计时从取得所有 scope 的最终一致性/写冻结锁并开始第一次源读取时开始，到加密容器 `hash-and-reopen`、冻结下的最终数据源 revision 复算全部通过时结束，使用包含系统休眠时间的 suspend-aware monotonic clock；恰好等于 `maxBackupDurationSeconds` 允许完成，多 1 毫秒立即中止。过程中重启、bootId 变化或单调计时连续性丢失视为本次备份失败。超时/中断必须删除半成品、销毁本次数据密钥、释放冻结锁并上报 `backup_failed`，不得保留一个估算耗时继续 validate；成功的精确毫秒数进入 backupFacts。`maxBackupDurationSeconds` 只约束上述备份生成和验证，不包含成功后的短期授权链；从 `completedAt` 对应的同一单调时刻起另设固定 10 分钟 `postBackupQuiesceDeadline`，该时限同样包含休眠且不能因刷新、重试或重启延长。consume 尚未成功时若到达该时限、控制面持续不可用、权限上下文失效或冻结连续性丢失，客户端必须立即阻止 consume。validate 尚未 requested 时先上报 `backup_invalidated`，再删除快照/keyRef并释放锁；validate 请求或响应状态未知时，先以原 transactionId 恢复原结果，已授权则执行 cancel，仍无法恢复时等待原授权最迟 5 分钟自然到期，确认不能消费后才删除快照/keyRef、释放锁并在可联网时收敛状态。consume 已成功后的锁丢失继续按安装事务规则进入 `abandoned_before_install` 或 `failed_manual_repair_required`，不得回到备份状态。

`pinvou-backup-container-v1` 固定为不压缩的认证加密二进制格式。明文 payload 为：8 字节 ASCII magic `PVPAYL01`，随后 uint64 big-endian 的规范清单长度、该长度的 RFC 8785 UTF-8 清单字节，再按清单 file 条目顺序重复 `uint64 big-endian 内容长度 + 原始文件字节`；directory 不带内容。每份快照生成新的 256-bit 随机数据密钥和 96-bit 随机 nonce，V1 只允许 `AES-256-GCM` 与 16-byte tag，并必须保证同一数据密钥下 nonce 从不复用。容器字节为：8 字节 ASCII magic `PVBACK01`、uint32 big-endian headerLength、JCS header、ciphertext、16-byte tag；header 恰含 `{format,aead,keyProtectionProfile,keyRefId,nonceBase64url,payloadLength,dataSourceRevisionSha256}`，AAD 是 magic、headerLength 与 header 的原始连接字节。header 必须是无 BOM 的 RFC 8785 UTF-8，长度为 1..4096 字节；`keyRefId` 是规范小写 UUIDv4，`nonceBase64url` 按 RFC 4648 §5 无填充编码且对 12 字节 nonce 必为 16 个 ASCII 字符，payloadLength 是不超过容器与清单上限的非负 JSON 安全整数并须精确等于 AEAD 解密后的明文字节数。解析器使用 checked arithmetic，拒绝未知/缺失/重复 header 字段、非最短整数编码、长度溢出、截断及 tag 后任何尾随字节。keyRefId 只引用系统级受保护密钥记录，容器不得内嵌明文或仅混淆的数据密钥；允许的 keyProtectionProfile 固定为 `windows-dpapi-local-machine-v1|macos-keychain-system-v1|linux-root-keystore-v1`。Windows profile 使用 DPAPI LocalMachine 保护数据密钥；macOS profile 把数据密钥作为 system Keychain 的受访问控制 secret；Linux profile 使用 RFC 3394 AES-256-KW 在独立 256-bit CSPRNG 主密钥下包装数据密钥，主密钥仅存 root-owned 0600 受保护存储，支持 TPM 时可再封装该主密钥但不得改变 profile 语义。三种 profile 的 keyRef 记录均须绑定 container path、snapshotSha256（封口后原子回填）、installationScopeId 和创建时间，防止跨容器替换；创建失败、回填失败或记录/容器不能原子收敛时同时销毁两者。快照删除/过期时同步删除 keyRef，无法读取密钥或 tag 校验失败即拒绝。人工支持导出必须由平台适配器在管理员授权后解密并重新封装为另行审计的支持导出物，不把本地 keyRef 当作跨平台格式。

快照容器在完成写入、同步和原子封口后，对最终**加密容器的全部原始字节**计算 SHA-256，所得即 `snapshotSha256`；容器字节数不得超过 `actualBytes + 64 MiB`，否则视为实现/数据异常并失败。`hash-and-reopen` 必须重新打开同一受保护路径，先复核容器字节哈希，再按上述格式完成 AEAD 验证、解密并重新生成其中嵌入的 V1 规范清单，逐文件验证长度/内容哈希，且其总 revision 必须等于备份前的数据源 revision；全部通过后才写 completedAt。复用前重新计算当前数据源 revision、容器哈希并执行同一 reopen，三者任一不符即删除/隔离旧快照并重新备份。正式 Schema 和三平台固定向量必须覆盖空目录、空文件、Unicode NFC、大小写冲突、内容变化但 mtime 不变、多 scope 排序、拒绝链接/特殊文件、nonce/tag/header/AAD 篡改、keyRef 缺失和密文单字节损坏。

`backupScopeIds` 的注册表还必须为每个 scope 声明版本化 `quiesceProfile`，列出所有产品写入者、停止/冻结方法、OS 强制写排他能力、锁的进程/boot 身份、单调 `quiesceFencingToken` 和迁移接管点。该 token 只标识本次数据冻结世代，与 12.8 的 `helperOwnershipFencingToken` 是独立命名空间，禁止比较、继承或互换。系统权限授予后，受控助手必须停止/冻结所有登记写入者并取得能阻止其他进程改写这些对象的 OS 强制排他句柄/锁，随后才允许 `backup_started`；无法证明排他的平台注册为不支持该 scope，`backupRequired` 失败关闭。该冻结从第一次数据源读取前开始，持续覆盖最终 revision、快照生成与验证、决定刷新、validate、consume，直到安装器在受保护事务中启动、校验同一 quiesceFencingToken 并明确接管数据迁移后才释放；期间所有产品写入入口必须由同一强制机制阻断。consume 前发生锁/权限/进程/boot 身份变化、检测到写入或超过 postBackupQuiesceDeadline 时，快照只能进入 `backup_invalidated`，不能继续授权；validate 已授权但尚未消费时先 cancel/等到期。consume 后但安装器启动前保护失效或未按时接管时，不得启动安装器，事务进入 `abandoned_before_install`；安装器已启动/接管后发现保护失效则进入 `failed_manual_repair_required`。不得只依赖普通 advisory lock、进程已退出、一次性 rehash 或“validate 已成功”宣称消除 TOCTOU。

`backupScopeIds` 只能取客户端随版本交付并签名登记的封闭语义 ID（例如 `pinvou-settings-v1`、`pinvou-local-knowledge-v1`），发布方不得填写路径、glob、环境变量或用户输入。注册表逐平台固定允许根、是否递归和排除项；受控助手以已打开目录句柄逐项解析，拒绝符号链接、junction/reparse point、挂载点、硬链接逃逸、设备文件、套接字、跨文件系统/跨用户边界及 TOCTOU 后身份变化。备份前同时校验清单估算、`maxBackupBytes`、全局 50 GiB 硬上限、磁盘安全余量和单文件上限，流式复制一旦超额即删除未完成快照并在 validate 前失败。快照采用每份随机密钥加密，密钥封装到 OS 系统凭据存储，目录仅受控助手/管理员可读写；平台无法提供该保护时 `backupRequired` 失败关闭。备份不得自动上传，界面须告知范围、大小、用途和到期时间，并只允许经系统授权的人工支持流程导出。从 `backup_started` 到授权取消/过期/coordination-aborted，或 consume 后安装事务进入协议终态之前，普通 UI 与用户命令均不得提前删除快照/keyRef；协议因 backup_invalidated/cancel/未消费到期要求删除的受控清理由助手执行。成功终态的快照 7 天后自动删除；`failed_manual_repair_required` 或其他失败终态的快照最迟 30 天删除。达到协议终态后，管理员可在系统授权下提前删除，但必须展示“删除后无法用于人工修复且不可恢复”的明确二次确认并记录操作者、事务、时间和原因；非终态“未知”事务只能由恢复流程收敛，不能用删除规避。删除、导出和过期清理均写脱敏审计，法定冻结不适用于这些本地普通备份。

V1 的三类正式平台目标都必须在签名 scope 注册表中实现并启用 `pinvou-settings-v1`，该 scope 只包含品悟自身的系统级配置数据，不得包含用户业务文档；Windows、Ubuntu 和 macOS 的 quiesceProfile 必须各自具备至少一条经真实系统验证的强制写排他实现。平台可以对其他 scope 声明 unsupported，但不得把全部 scope 都标为 unsupported 仍宣称支持 backupRequired。任何 required Release 至少包含一个在其所有目标平台均受支持的 scope；发布闸门必须证明每个 targetKey 都能走通“权限→强制冻结→成功备份→validate→consume→安装器接管”路径，否则该 Release Target 不得批准。

## 11. 升级服务 API

### 11.1 通用约定

- 控制面根地址固定为 `https://update.pinvou.com`；API 使用 JSON 和 TLS 1.2 及以上。
- 动态 JSON API 与错误响应的响应体含本次 HTTP 交换唯一的 `requestId`；检查更新成功时另签发跨接口稳定的 `decisionId`，后续链路不得把某次接口的 requestId 当成 decisionId。Root/Snapshot/Target/Release/Package 等不可变元数据和第三方文件响应不得把 requestId 注入内容体，只可使用不进入签名、长度或内容哈希的 `X-Request-Id` HTTP Header。所有时间为 UTC RFC 3339；客户端请求含 `protocolVersion`。
- 个性化检查、文件信息和安装复核响应使用 `Cache-Control: private, no-store`，禁止共享缓存。ETag 只用于不可变制品下载或公开的签名元数据。
- 会创建或迁移服务端状态的动态接口必须通过 `Idempotency-Key` 幂等；普通 `/check` 固定使用与 requestNonce 相同的值，refresh、validate、consume、cancel 按各节专用规则。time-challenge 只消费本次随机数而不创建升级 lineage。429/503 使用 `Retry-After`，客户端增加随机抖动。
- “规范请求摘要”统一表示：JSON 通过 Schema 后取语义对象，数字按 Schema 类型解析，成员顺序和无意义空白不参与身份，字符串只执行各字段明确定义的规范化，然后对 RFC 8785 UTF-8 字节使用用途隔离的 KMS `idempotencyRequestDigestKey` 计算 HMAC-SHA256，以 base64url 无 padding 保存；因此 JSON 成员换序/空白变化不冲突，字段值、数组顺序或 Compact JWS 字符串变化会冲突。记录保存 digestKeyId，服务端按记录中的 keyId 复算；该 HMAC 密钥至少保留到最后引用它的幂等记录到期，不得复用为 Rollout 分桶、SN 名单或签名密钥。
- 普通 `/check` 的规范对象在计算上述 HMAC 前必须先做隐私投影：hardwareSn 缺失写固定对象 `{"state":"absent"}`，格式非法写 `{"state":"invalid"}`，合法值按服务端版本化且与 Rollout 无关的 `checkIdempotencySnNormalizationVersion` 得到 normalizedHardwareSn，再替换为 `{"state":"present","normalizationVersion":n,"digest":"<base64url-no-padding HMAC-SHA256(checkIdempotencySnKey, UTF8(normalizedHardwareSn))>"}`。`checkIdempotencySnKey` 是另一把用途隔离 KMS 密钥，记录其 keyId 和规范化版本并至少保留 24 小时，不得复用分桶、名单或全请求摘要密钥。服务端不得保存原始 SN、包含原始 SN 的请求体，或可离线字典枚举的无密钥 SN/请求 SHA-256；完成投影后立即丢弃原值。相同幂等键的重试使用原记录 keyId 和规范化版本复算，因 Rollout 配置变化不得产生冲突。
- `/check` 普通检查与刷新、validate、consume、cancel 共用版本化幂等记录模型。记录状态只允许 `processing|committed`：processing 必含 endpoint、作用域键、Idempotency-Key、规范请求摘要、随机 `ownerEpoch`、`ownerStartedAt`、`leaseExpiresAt`、createdAt，以及在创建 reservation 的同一事务写入的 `operationRecoveryProjection`、`recoverySchemaVersion` 和 `recoveryKeyId`；committed 在此基础上必含不可变业务响应投影、HTTP 业务状态和 committedAt。固定令牌/新鲜度/sequence/lineage 等无副作用前置检查在 reservation 之前完成，前置拒绝不创建幂等记录；通过全部前置条件后，首次请求必须以唯一索引原子创建 processing reservation 及恢复投影，成功后才能执行任何领域状态写入，唯一索引竞争失败则回到记录判定步骤。每任 owner 的租约固定 30 秒，活跃 owner 每 10 秒续租，但从自己的 ownerStartedAt 起最多持有 2 分钟，达到该边界必须停止续租并让接管者恢复；2 分钟是单任 owner 上限，不是阻止后继 owner 接管的全记录上限。相同 key 但摘要不同始终返回 `idempotency_conflict`。相同 key/摘要命中 processing 时以服务端可信时间判断：`trustedNow < leaseExpiresAt` 返回 HTTP 409 `idempotency_in_progress`、`Retry-After: 2` 和新的 requestId；`trustedNow >= leaseExpiresAt` 必须尝试对旧 `{ownerEpoch,leaseExpiresAt}` 做 CAS，胜者写入新的随机 ownerEpoch、ownerStartedAt=trustedNow 和 30 秒 lease 后进入恢复流程，败者返回同一 in-progress；命中 committed 才按各端点规则返回原业务结果。
- processing owner 生成令牌/响应后，必须在同一数据库事务中执行领域 CAS、写入不可变响应并把记录改为 committed；事务提交前崩溃不得留下领域副作用，提交后则必须同时存在 committed 结果。签名操作可在事务前完成，但未随 committed 原子落库的签名值没有服务端效力且不得恢复给客户端。两个不同幂等键都通过前置读取、但其中一个在领域 CAS 竞争中失败时，失败 owner 必须确认本 operation key 没有任何领域副作用，再以 ownerEpoch 条件原子删除自己的 processing reservation，回到步骤⑤并返回当时唯一的 authorization_in_progress/状态冲突；不得把瞬时 in-progress 永久提交为旧结果。owner 崩溃、租约到期或达到其 2 分钟持有上限后，新节点按上一条 CAS 接管同一 processing 记录，并先检查是否已有同 operation key 的 committed 领域事务；没有则从无副作用状态重算，有则补齐同一不可变响应，绝不能重复创建 session、提升 revision、签发第二授权、重复消费或重复取消。除上述“确认零副作用的 CAS 失败”外，processing 记录不得删除或按普通 TTL 清理，必须由当前或后继 owner 收敛为 committed；自 createdAt 起超过 2 分钟尚未收敛必须告警，但仍允许后继 owner 接管，人工修复也不得绕过唯一 operation key。24 小时恢复窗口从 committedAt 起算。OpenAPI 对五个端点统一声明该码和 Retry-After。
- 初始授予和每次续租都必须令 `leaseExpiresAt=min(trustedNow+30s, ownerStartedAt+2min)`，不得让最后一次续租越过单任 owner 上限；因此 `trustedNow == ownerStartedAt+2min` 时该 owner 的租约必已到期，后继请求可以立即竞争接管。CAS 胜者的新 ownerStartedAt 重新开始自己的 2 分钟上限，但不得重置记录 createdAt 或 operation key。
- check、refresh、validate、consume、cancel 的**每一次**最终领域副作用与 `processing → committed` 事务都必须重新 CAS 并同时满足：记录仍为相同 endpoint、作用域键、Idempotency-Key 和规范请求摘要的 processing，仍归当前 ownerEpoch，且 `trustedNow < leaseExpiresAt`、`trustedNow < ownerStartedAt + 2min`。任一条件不满足时，当前执行者必须丢弃预先计算的 JWS/响应，不得提交领域写入或 committed 结果，并回到现存记录/租约的判定与接管流程；`trustedNow == leaseExpiresAt` 或 `trustedNow == ownerStartedAt + 2min` 时旧 owner 已无提交权。新 owner CAS 成功后，旧 owner 无论是在 CAS 前还是后完成业务计算，都因 ownerEpoch/租约条件不符而不能提交。validate 的最终授权、业务失败和 coordination_abort 还必须在同一事务校验 marker 归同一 owner、租约字段一致且 lineageConcurrencyVersion 等于 reservedLineageConcurrencyVersion；refresh 的最终事务也必须遵守本条 live-owner/live-lease CAS。后台恢复器只有先按相同规则取得当前有效租约，才可执行最终提交。
- `operationRecoveryProjection` 是五端点统一的服务端恢复输入，不是客户端可读响应。它必须用授权恢复专用 AEAD 密钥加密并做记录主键/endpoint/digest 的 AAD 绑定，只允许当前 owner、恢复 worker 与安全审计角色读取，禁止进入日志、报表或通用导出。Schema 固定保存经隐私投影且通过 Schema 的语义请求、必要的已验证 token 字符串及 claims/kid/jti、前置读取的 lineage/选择快照和产生确定结果所需的 Manifest/发布引用；check 只保存 11.1 已 HMAC 化的 SN 状态/分桶输入，绝不保存原始 SN。记录大小上限 256 KiB，创建失败则整个 reservation 事务回滚并返回 503，不得留下无恢复投影的 processing。validate 的 activeValidateOperation.recoveryProjection 是该通用投影的受栅栏副本，两者摘要必须相等，不另造语义。
- 恢复 worker 至少每 30 秒扫描全部已过 leaseExpiresAt 的 processing；即使客户端永不重试，也必须按同一 ownerEpoch CAS 接管、从投影重新执行当前 endpoint 的固定判定管线并收敛为一个 committed 结果。恢复时必须重验 kid 吊销、凭据/授权时限、lineage、即时 deny、Registry/selectionGeneration/Snapshot 指针和发布资格；尚可执行则按当前一致性快照重算，已失效则提交该端点既有封闭错误/终态 tombstone，绝不能恢复或签发已失效凭据。check/refresh 若选择快照已变必须在当前有效租约内重算；consume/cancel 若授权已过期则按既有 authorization_expired 规则收敛；validate 按 marker/coordination_abort 规则收敛。恢复 worker 在租约内仍不能提交时必须续租或让下一 worker 接管，processing 不得因没有客户端请求而永久存在。投影和 committed 恢复数据在 committedAt+24 小时一并删除；自 createdAt 超过 2 分钟未 committed 告警，自 createdAt 超过 10 分钟仍未 committed 升级为 P0，但不得绕过 operation key 手工重做。
- 每个 telemetry lineage 必须持有严格递增且不复用的 `lineageConcurrencyVersion`；任何事件接受、决定刷新/替换、validate marker 预留/收敛、取消、到期和 consume 都以旧值 CAS 并在提交时加一。服务端前置读取必须保存 expectedLineageConcurrencyVersion、decisionRevision、当前 telemetry credential jti/replaced 标志、lastAcknowledgedSequence 和规范状态；后续写事务必须逐项复核，不能只复核状态枚举。activeValidateOperation 存在时，除持有 marker 的 validate owner/后台恢复器外，所有 lineage 写事务还必须要求 marker 不存在；前读后 marker 抢先创建会使其 CAS 失败并按 requested 状态重新判定。该并发版本是服务端协调字段，不由客户端选择，也不能代替凭据、sequence 或业务 revision。
- V1 JSON Schema 和 OpenAPI 文档是可执行合约，字段增补只能为可忽略的非关键扩展；必需能力通过 `requiredCapabilities` 协商。
- 客户端必须忽略未知的非关键字段。未知的增量候选格式或算法属于候选级可选扩展：只忽略该候选并继续选择受支持的完整包，不得因此拒绝整个 Release Manifest。只有未知语义位于顶层策略、已选择制品、签名/身份字段，或出现在 `requiredCapabilities` 中且客户端不支持时，才返回 `updater_protocol_upgrade_required`。所有关键扩展必须以 `requiredCapabilities` 的稳定标识显式声明；服务端不得在不提升协议/Schema 版本的情况下改变既有字段、枚举和错误码语义。
- 控制面请求体默认上限 256 KiB，`Content-Type` 必须为 `application/json`；重复 JSON key、非法 UTF-8、超深嵌套和 Schema 未允许的关键字段必须拒绝。所有签名元数据必须在 JSON 解析前按原始字节硬限额：Timestamp 64 KiB，Root/Target 各 256 KiB，Release/Package Manifest 各 1 MiB，Snapshot 8 MiB；最大嵌套深度 16、单对象成员 256、签名 16 个、Target `hostConstraints` 64 项、Release 增量候选和 Package 增量包各 128 项、每个包的 helperArtifacts 8 项。超过任一限制即拒绝，不得先分配声明大小的内存。OpenAPI 必须明确每个字段的 required/nullable、长度、数值范围及未知字段策略，不能只依赖本文示例推断。

可信元数据通过下列控制面接口获取：

```text
GET /api/v1/update/metadata/root/{version}.json
GET /api/v1/update/metadata/timestamp.json
GET /api/v1/update/metadata/snapshot/{version}.json
GET /api/v1/update/metadata/targets/{productId}/{componentId}/{channel}/{targetKey}/{version}.json
GET /api/v1/update/metadata/release-manifests/{deploymentId}/{version}.json
GET /api/v1/update/metadata/package-manifests/{sha256}.json
POST /api/v1/update/metadata/time-challenge
```

Root、版本化 Snapshot/Target/Release Metadata 和按内容哈希寻址的 Package Manifest 使用不可变响应与 `Cache-Control: public, immutable`；Timestamp 按有效期刷新。验证顺序不得循环：Root 按 8.5 由已信任 Root 的双阈值顺序更新；Timestamp 直接使用当前 Root 授权的 Timestamp 角色验签；Snapshot 先按 Timestamp 中的版本/长度/哈希验证 7.2 所定义的完整封装字节，再按 Root 的 Snapshot 角色验签；Target/Release/Package Manifest 先按已验证 Snapshot 中的路径、版本、长度和哈希验证完整封装字节，再验证 `_type`、作用域、引用链和角色签名。任一重复 JSON key、路径、长度、哈希或 Manifest 引用不一致均返回 `metadata_mismatch` 并拒绝制品。Package Manifest 可长期缓存且自身不因时间失效，但从当前 Snapshot 消失、构建密钥被 revoked 或当前 Release Target/授权不再允许时不得用于新安装。检查响应必须给出当前 Target/Release Manifest 的不可变引用；即使内嵌 Release Manifest 副本，客户端也只能把通过上述端点验证的对象用于安全决策，并须确认内嵌副本的完整封装字节身份与其一致。

`time-challenge` 只用于 8.2 的 bootId/时钟异常恢复，不签发更新资格，并严格受 8.5 的 Root 有效性/受限例外约束。请求只含至少 128 bit 随机 `nonce`；响应使用当前 Root 授权的 Timestamp 角色签署封闭对象 `{_type:"time-attestation",schemaVersion:1,aud:"pinvou-time-recovery",nonce,issuedAt,expiresAt}`，其中 `0 < expiresAt-issuedAt <= 10min`，服务端成功响应的 `issuedAt` 与服务端可信当前时间差不得超过 2 分钟，并返回 `Cache-Control: private, no-store`。客户端须精确校验类型、aud、nonce、签名、kid 未 revoked、时间区间合法且同一 nonce/证明未使用；因为进入该流程时本地墙钟不可信，不用旧 trustedNow 判断证明到期，而要求请求和响应处于同一次存活进程的单调计时往返且总耗时不超过 2 分钟，响应一经验证即消费 nonce。证明只能原子建立本 bootId 的时间锚点，不能替代 Timestamp/Snapshot 或单独解除异常。端点可用性纳入控制面 SLO；服务端时间源异常时失败关闭并告警，绝不返回缓存证明。

`decisionToken`、`installAuthorization`、`telemetrySessionCredential` 和 `installTransactionCredential` 均使用 Compact JWS，固定 `alg=EdDSA`，`kid` 必须属于当前 Root 授权且处于可验证状态的 Online Authorization 角色，禁止 `alg=none` 和算法降级。四类令牌必须分别精确校验固定类型，不允许跨接口代用：decision 使用 `aud=pinvou-update-decision, tokenUse=decision`；安装授权使用 `aud=pinvou-install-authorization, tokenUse=install-authorization`；遥测会话使用 `aud=pinvou-telemetry-session, tokenUse=telemetry-session`；安装事务使用 `aud=pinvou-install-transaction, tokenUse=install-transaction`。每个端点必须在业务处理前同时校验 aud、tokenUse、必需字段集合、禁止字段集合和字段类型；合法签名但类型错误也返回 `token_type_invalid`。

决策载荷至少绑定 `iss`、上述固定类型、`jti`、`iat`、`nbf`、`exp`、`decisionId`、`decisionRevision`、`telemetrySessionId`、`updateAvailable`、`targetKey`、`hostArch`、规范化 `osVersion` 的 RFC 8785 SHA-256、`currentVersion`、`installId`、`installationScopeId`、`channel`、受保护渠道记录的 `channelRevision`、`selectionGeneration`、`sourceProfileId`、`sourceFactsRegistryVersion`、`protocolVersion`、`capabilitiesSha256`、`currentUpdaterFactsSha256` 和 `requestNonce`；`capabilitiesSha256` 是 Schema 校验后的 `updaterCapabilities` 原始语义对象按 RFC 8785 规范化所得 SHA-256，包含受保护 updater 完整事实、备份 scope 注册表版本/哈希、backupHashAlgorithms 和 backupContainerFormats，集合型能力数组先拒绝重复项并按 ASCII 排序；`currentUpdaterFactsSha256` 按 11.2 的封闭投影单独计算，便于跨阶段明确复核。有更新时必须另绑定 `selectionMode`、`deploymentId`、`releaseId`、`releaseTargetId`、Target/Release Manifest 的版本与完整封装哈希、`installIntentSha256`、`helperArtifactsIndexSha256`、允许的 `packageIds`，以及按 packageId 排序的 `helperExecutionPlans`；每个 plan 固定包含 `transactionHelperId`、`helperBytesInstallRequired`、`helperHandoffRequired` 和派生的 `helperUpdateRequired`，并禁止 `reason`。无更新时必须绑定 `reason`，且禁止 `selectionMode` 及所有 Deployment/Release/Package/Manifest/助手字段。若本次命中候选 Rollout（包括 `candidate_prerequisite`），还必须绑定 `rolloutId`、`rolloutDecisionKind=candidate|baseline-prerequisite` 和不含 SN 的分桶结果快照。安装授权另绑定上述决定 revision/世代、渠道 revision、sourceProfileId/sourceFactsRegistryVersion、`installIntentSha256`、`currentUpdaterFactsSha256`、所选包的 `transactionHelperId`、`helperArtifactsSha256`、助手本地复核所得两个原始布尔值及其派生汇总值、包容器大小/SHA-256、最终安装器大小/SHA-256、本地事务 ID 和 `backupFactsSha256`。事件凭据绑定 telemetrySessionId、decisionRevision、事件状态机版本和允许阶段，不能用于下载或安装授权。SN 不进入令牌绑定字段，改变或缺失 SN 本身不能使已签发令牌失效。

有更新 HTTP 响应体中的 `helperExecutionPlans` 必须与 decisionToken 同名字段逐字相同，按 packageId ASCII 升序且与 token.packageIds 一一对应，不得缺项、额外或重复；客户端必须同时验证数组本身、所选 packageId 的 plan 及三个布尔值逻辑关系。响应体、令牌和按当前受保护事实本地复算结果任一不一致，均拒绝整个决定。

令牌时间偏差只适用于未来签发边界：接受条件为 `iat <= trustedNow + 2min` 且 `nbf <= trustedNow + 2min`；到期严格要求 `trustedNow < exp`，不得对 exp 增加容差，`installStartNotAfter` 与 `mainInstallerStartNotAfter` 也都严格按 `trustedNow < 对应边界` 判断。2 分钟不得延长任何声明有效期。

错误响应统一为 `{"requestId":"...","error":{"code":"stable_machine_code","message":"localized safe message","retryable":false,"supportGuideCode":"optional_fixed_code","details":{}}}`；`supportGuideCode` 只能取客户端内置允许列表的机器码，不是 URL。`details` 只允许字段级诊断，不暴露灰度规则、SN、令牌或内部栈。

### 11.2 检查更新

```text
POST /api/v1/update/check
```

后续启用增量能力后的请求示例（首期正式客户端按下文要求发送空能力数组；`hardwareSn` 为可选字段）：

```json
{
  "productId": "pinvou-agent",
  "componentId": "pinvou-desktop",
  "currentVersion": "0.10.5",
  "currentPackageId": "full-0.10.5-windows-x86_64",
  "channel": "stable",
  "channelRevision": 7,
  "target": {
    "os": "windows",
    "arch": "x86_64",
    "packageFormat": "nsis"
  },
  "hostArch": "x86_64",
  "osVersion": {
    "family": "windows",
    "ntVersion": "10.0",
    "build": 22631,
    "productType": "workstation"
  },
  "hardwareSn": "<raw-device-sn-over-tls>",
  "installId": "550e8400-e29b-41d4-a716-446655440000",
  "installationScopeId": "4f8a60ac-c3d8-48c8-a678-ec4c475b6771",
  "locale": "zh-CN",
  "requestNonce": "<128-bit-random-base64url>",
  "updaterCapabilities": {
    "protocolVersion": 1,
    "helperProtocolVersion": 1,
    "launcherProtocolVersion": 1,
    "protectedUpdaterFactsSchemaVersion": 1,
    "protectedUpdaterFactsRevision": 12,
    "activeHelperRef": {
      "helperId": "pinvou-updater-helper",
      "version": "1.3.0",
      "installTargetId": "pinvou-updater-helper-versioned",
      "sha256": "<current-helper-sha256>"
    },
    "currentHelperFacts": [
      {
        "helperId": "pinvou-updater-helper",
        "version": "1.3.0",
        "protocolVersion": 1,
        "installTargetId": "pinvou-updater-helper-versioned",
        "size": 8200000,
        "sha256": "<current-helper-sha256>",
        "authenticitySha256": "<jcs-helper-authenticity-sha256>"
      }
    ],
    "currentLauncherFact": {
      "version": "1.2.0",
      "protocolVersion": 1,
      "installTargetId": "pinvou-updater-launcher-stable",
      "size": 2100000,
      "sha256": "<launcher-sha256>",
      "authenticitySha256": "<jcs-launcher-authenticity-sha256>"
    },
    "backupScopeRegistryVersion": 1,
    "backupScopeRegistrySha256": "<64-lowercase-hex-sha256>",
    "backupHashAlgorithms": ["pinvou-backup-manifest-v1"],
    "backupContainerFormats": ["pinvou-backup-container-v1"],
    "fullPackage": true,
    "incrementalFormats": ["artifact-delta-v1"],
    "incrementalAlgorithms": ["bsdiff-v1"]
  },
  "cachedArtifacts": [
    {
      "packageId": "full-0.10.5-windows-x86_64",
      "resultArtifactSha256": "<base-installer-sha256>"
    }
  ]
}
```

`updaterCapabilities` 中的 updater 事实是封闭安全字段，不是 UI 自报能力。V1 `protectedUpdaterFactsSchemaVersion` 固定为 1，`protectedUpdaterFactsRevision` 是同一 installationScopeId 下每次 helper/launcher 事实提交后严格递增且不复用的正整数。`currentHelperFacts` 必须有 1～8 项；允许同一 helperId 的多个版本在受保护版本目录并存，数组按 helperId、version 原始 UTF-8、sha256 的 ASCII 字节序逐级升序，完整 `(helperId,version,installTargetId,sha256)` 及解析后的规范化实际文件路径均不得重复。每项恰含示例中的七个字段，version 使用 6.2 SemVer，protocolVersion 为正整数，size 为非负 JSON 安全整数，sha256/authenticitySha256 为 64 位小写十六进制。`activeHelperRef` 恰含 helperId、version、installTargetId、sha256，必须唯一且逐字段命中 currentHelperFacts 的一项；外层 helperProtocolVersion 必须等于该活动项 protocolVersion。由此活动旧版本与已安装未激活的新版本可同时被准确表达，不能只靠 helperId 推断活动字节。`currentLauncherFact` 恰含示例中的六个字段，规则相同，外层 launcherProtocolVersion 必须与其相等。`authenticitySha256` 分别对 7.3 已规范化的完整 helperAuthenticity/launcherAuthenticity tagged-union 对象求 `SHA256(RFC8785(object))`，不得只哈希 type 或证书显示名。

`currentUpdaterFactsSha256` 固定为以下对象的 RFC 8785 SHA-256：`{protectedUpdaterFactsSchemaVersion,protectedUpdaterFactsRevision,activeHelperRef,currentHelperFacts,currentLauncherFact}`。受控助手必须从系统级受保护安装记录读取这些值，同时重新验证当前文件的版本、协议、规范路径、大小、内容哈希和平台真实性；记录与文件不一致时在发出 check 前进入人工修复。服务端把完整事实与 `currentVersion + targetKey` 对应的 Updater Source Facts Registry 复核；不属于任何已批准来源事实时返回 426 和固定修复指引，不得猜测。对每个目标 packageId，服务端生成唯一助手执行计划：`helperBytesInstallRequired=true` 当且仅当目标 helperArtifacts 中至少一项无法在 currentHelperFacts 找到 helperId、version、protocolVersion、installTargetId、size、sha256、authenticitySha256 全等的条目；目标数组为空时该值为 false。`helperHandoffRequired=false` 当且仅当 activeHelperRef 唯一命中的活动事实之 helperId 等于 transactionHelperId，且数组非空时该活动事实还与数组中 transactionHelperId 的目标条目七字段全等，数组为空时该活动事实满足 minHelperProtocolVersion；其他情况一律为 true。`helperUpdateRequired` 必须严格等于两者逻辑或，只作兼容汇总，客户端不得提交或选择不同值。由此，“目标 transaction helper 的相同字节已安装但当前未激活”固定得到 bytes=false、handoff=true、update=true，仍须走受保护接管；多 helper 数组也只能由 transactionHelperId 指定的唯一助手执行事务。服务端据此执行 10.5 的允许集合/桥接选择；客户端、validate 和 consume 都必须复算并与 decision 的 currentUpdaterFactsSha256、helperExecutionPlan 及授权字段逐项一致。

首期正式客户端的两个增量能力数组均为 `[]`。每次新的普通检查生成至少 128 bit `requestNonce`，并以同值发送 `Idempotency-Key`；同一 `requestNonce`/`Idempotency-Key` 不得用于另一业务检查，持久 `installId` 则正常跨周期检查和升级复用。响应未知时，客户端必须在 24 小时内以相同 nonce/key 和相同规范语义请求对象重试。服务端按 `installationScopeId + requestNonce` 保存 11.1 定义的隐私保护请求摘要和幂等记录；同摘要 processing 按 11.1 判断返回 in-progress 或由过期租约 CAS 胜者恢复，同摘要 committed 重试在确认原结果的 decision/telemetry 签名 kid 均未 revoked 后返回原凭据与业务字段并生成新 requestId，任一 kid 已 revoked 则返回 401 `credential_revoked`、不创建第二 session，客户端须改用新 nonce/key 建立独立检查；摘要变化返回 `idempotency_conflict`，committed 结果从 committedAt 保留 24 小时。`channelRevision` 和 updater 完整事实由受控助手读取，不信任 UI 自报。服务端把 nonce、revision、capabilitiesSha256、currentUpdaterFactsSha256、sourceProfileId 和 sourceFactsRegistryVersion 绑定进决策令牌以阻止不同检查响应互换。客户端收到有更新响应后，必须先验证 `decisionToken` 的 JWS 和有效期，再逐项比对本次请求保存的 requestNonce、installId、installationScopeId、currentVersion、channel、channelRevision、targetKey、hostArch、osVersion 摘要、protocolVersion、capabilitiesSha256、currentUpdaterFactsSha256，以及响应中的 decisionId、selectionGeneration、sourceProfileId、sourceFactsRegistryVersion、deploymentId、releaseId、releaseTargetId、Target/Release Manifest 引用、允许的 packageId 和 helperExecutionPlans；任一不一致都拒绝整个响应，不能只依赖后续服务端复核。`currentPackageId` 为可选优化提示：旧安装、迁移安装或无法证明来源时必须省略，服务端不得用它代替 `currentVersion` 或作为准入条件。`cachedArtifacts` 最多上报 20 条，仅包含客户端已重新校验的基础安装器身份。Release Manifest 是按 Deployment 签名的不可变内容，不按客户端能力临时裁剪；响应可以携带完整不可变 Manifest 及全部已批准候选，但 decisionToken.packageIds 是本设备本次唯一允许集合，客户端只能在该集合内按自身能力与本地缓存选择，服务端不得假设客户端必然持有基础制品。首期能力数组为空，因此客户端安全忽略全部增量候选。检查请求中的版本、缓存、updater 事实和 SN 都不是安全授权边界，真正安装仍以签名清单、受控助手实际事实复核和安装授权为边界。

普通 check 开始选择时必须保存不可变 `selectionReadSet`：当前 Snapshot 版本/完整封装哈希及选择指针、Target/Release/Package 引用、sourceFactsRegistryVersion/sourceProfileId/edgeId、channel selectionGeneration、即时 deny revision、Deployment/Rollout/Release Target/Artifact 状态版本。签发 decision/telemetry JWS 后，最终 `processing → committed` 事务除 11.1 live-owner/live-lease 外，还必须逐项 CAS 该 read set 仍等于当前值并重验签名 kid 可签；任何变化都必须丢弃预计算凭据，在当前租约剩余时间内读取全新一致性快照并从选择步骤重算。重算结果可以是新 generation 的有更新、无更新或封闭错误，但绝不能提交旧 read set 的 session/decision；剩余租约不足则让后继 owner 用 operationRecoveryProjection 恢复。即时 deny 的写事务与其状态版本/generation 提升先于新 check 提交栅栏生效，因而 deny 提交后不得再产生引用旧资格的新决定。

本文所称“来源构建矩阵”统一实现为仅供升级服务选择器使用的版本化 `Updater Source Facts Registry`。它**不是**客户端可信元数据角色，不进入 Root/Snapshot、不由客户端获取或缓存、没有公开元数据端点，也不改变 8.1 的角色/阈值/到期模型。注册表匹配投影固定为 `{canonicalAppVersion,targetKey,activeHelperRef,currentHelperFacts,currentLauncherFact}`，不含运行时 protectedUpdaterFactsRevision；每个 `profileId=SHA256(RFC8785(投影))`。条目必须记录不可变来源 `build-native|helper-plan|launcher-boundary|cleanup`、引用的已签名 Package/Release 完整封装哈希、introducedAt、状态与审计主体；只能由发布闸门从已验证签名元数据和固定转换算法生成，普通配置、客户端请求和运维手工 SQL 都不能新增或改写 profile。

该注册表是控制面 RPO=0 不可变账本，版本作用域固定为 `productId + componentId + targetKey`，`sourceFactsRegistryVersion` 从 1 严格递增且不复用，同版本不同内容为 P0 冲突。新增、状态变更和当前版本指针必须经双人发布审批，在一个控制面事务中写入新账本版本、提升受影响作用域的 selectionGeneration 并切换选择指针；check 在同一一致性快照读取 registryVersion+selectionGeneration，不能混读。账本复制、备份和恢复遵守 17.2 的 RPO=0 高水位；恢复到较低版本、缺失当前版本或跨区域内容哈希不等时该作用域失败关闭并告警。profile/edge 的最低留存终点使用可审计控制面事件计算：取所有引用渠道已签名 Target 的 supportFloorVersion 首次严格高于该 profile.canonicalAppVersion 的发布时间最大值，再加 2 年；若仍有当前 Target 覆盖该版本、活跃事务/幂等恢复引用或法定冻结，则继续保留。长期离线设备不被推断为“已退出”，留存期满后的请求只能得到固定人工修复指引。安全撤回只改变 profile 的授权资格/修复结果，不删除审计链。注册表无 expiresAt；其输入 Release/Target 的当前安装资格仍按各自有效期和状态判断。

发布闸门必须从 supportFloorVersion 内所有实际来源 profile 枚举直接边、桥接边、bytes-only、handoff-only、字节加接管、接管后主安装器未启动、以及安装器允许的 launcher 边界，生成有限闭包并验证每个中间 profile 都能继续检查或明确进入人工修复。单一 `canonicalAppVersion + targetKey` 最多保留 64 个 active/intermediate profile；超过时发布失败，必须用应用桥接版本收敛，不得运行时模糊匹配。只要来源版本仍高于 support floor，已授权事务可能留下的 intermediate profile 即使原 Deployment 后来 superseded/withdrawn，也须在服务端账本保留为“可识别来源”；安全撤回可禁止其取得新授权，但必须返回明确修复路径，不能把精确合法中间态误报为未知构建。主安装器尚未启动而事务进入 abandoned_before_install 时，新普通 check 必须接受“旧 canonicalAppVersion + 已提交 post-helper profile”，重新选择后对相同目标得到 bytes=false/handoff=false；主安装器已启动后则以实际探测 app/launcher/helper 事实匹配注册表，失败事务可以只允许人工修复或更高版本前向修复，但不得因该组合已由协议产生而固定返回未知事实 426。

Registry V1 Schema 是封闭合约。顶层恰含 `schemaVersion=1`、productId、componentId、targetKey、sourceFactsRegistryVersion、createdAt、`profiles[]`、`edges[]` 和前一版本内容哈希。每个 profile 恰含 profileId、上述完整匹配投影、`originType=build-native|helper-plan|launcher-boundary|cleanup`、originRef、`status=selectable|forward-only|repair-only|revoked`、statusReasonCode、supportGuideCode、introducedAt；每条 edge 恰含 edgeId、fromProfileId（build-native 为 null）、toProfileId、originType、releaseTargetId/packageId（不适用时为 null）、有序 removeHelperFacts（仅 cleanup 非空）、`edgeStatus=enabled|disabled|revoked` 和 `remediationEdge`。数组按 ID ASCII 升序且拒绝重复；全部 ID 均为对应 RFC 8785 投影的 SHA-256。profile 投影不可变，变更只能新增 profile/edge 或在更高账本版本写状态事件。profile 状态只允许 `selectable → forward-only|repair-only|revoked`、`forward-only → repair-only|revoked`、`repair-only → revoked`，revoked 为终态；edge 只允许 enabled→disabled|revoked，revoked 为终态，恢复资格必须创建新 edgeId 并完成新一轮审批，不能原地回退状态。

四类投影函数必须由发布闸门和服务端使用同一参考实现及固定向量，定义如下，其中 helper fact 由 Manifest 七字段去掉 filePath/installMode 并加入其 authenticitySha256 后形成，数组均按 11.2 顺序规范化：① `build-native(Package)` 只用于全新安装或迁移引导安装的原生落点，要求所选完整包 helperArtifacts 非空且 transactionHelperId 精确命中一项；结果为 `{canonicalAppVersion=targetVersion,targetKey,activeHelperRef=命中项的四字段引用,currentHelperFacts=该包 helperArtifacts 的事实集合,currentLauncherFact=postInstallLauncher事实}`。不满足该条件的包可用于升级，但不能产生 build-native profile。② `helper-plan(F,Package,bytes,handoff)` 保持 F 的 app/target/launcher；bytes=true 时把 Package.helperArtifacts 的事实逐项并入 currentHelperFacts，bytes=false 时要求这些事实已全部存在；任何同 `(helperId,version,installTargetId)` 不同哈希/身份冲突都使发布失败。handoff=true 时 activeHelperRef 改为 transactionHelperId 在合并后集合中的唯一精确事实，false 时保持 F.activeHelperRef；结果必须与三个计划布尔值的算法一致。③ `launcher-boundary(F,Release)` 表示主安装器已启动并提交其 app/launcher 事实：canonicalAppVersion 改为 targetVersion、currentLauncherFact 改为 postInstallLauncher，activeHelperRef/currentHelperFacts 保持 F 的 post-helper 值；V1 主安装器不得在该边隐式删除助手。④ `cleanup(F,R)` 要求 R 是 F.currentHelperFacts 的精确子集、不得包含 activeHelperRef 命中项且没有活跃事务引用；结果仅从 currentHelperFacts 删除 R，其他字段逐字不变。每个计算结果必须已经以 toProfileId 存在，运行时不允许临时生成 profile 或模糊合并。

状态到响应的映射固定如下：selectable profile 可走 enabled 普通边；forward-only 只可走 `enabled && remediationEdge=true && targetVersion>canonicalAppVersion` 的前向修复边，不得选择普通候选，若无此边返回 426 `updater_protocol_upgrade_required`、supportGuideCode=`source_profile_forward_repair_required`；repair-only 固定返回同一 426、supportGuideCode=`source_profile_manual_repair_required`；revoked 固定返回同一 426、supportGuideCode=`source_profile_security_repair_required`，并禁止 download-info/validate/consume；未注册 profile 固定返回同一 426、supportGuideCode=`source_profile_unknown`。只有 profile 处于 selectable/forward-only 且按其允许边完成结构路径计算后，才可能返回 `no_compatible_update`；不得把 repair-only/revoked/unknown 映射成该 reason。intermediate profile 即使来源 Deployment 失效仍按自身状态识别，是否可前向修复只看当前 enabled remediation edge；因此响应唯一，不由运行节点猜测。

Registry 不含 channel，但 selectionGeneration 含 channel。任何 profile/edge 新增或资格状态变化都必须在同一 RPO=0 控制面事务中：枚举该 product/component/targetKey 下当前存在的 internal、beta、stable 及未来新增渠道作用域，分别严格提升 generation，写入各渠道新的 Registry 指针，再切换全局 registryVersion；任一渠道无法提交则全部回滚。新建渠道必须从当前 registryVersion 初始化，禁止引用旧版本。check 的一致性快照及其决定必须绑定 `sourceProfileId + sourceFactsRegistryVersion + channel selectionGeneration`；validate/consume 重新匹配实际 facts 得到同一 sourceProfileId，并复核该 profile/所选 edge 仍有资格及 Registry 指针、generation 均未变化。任何不一致按固定发布资格优先级返回 selection_generation_changed；不能让某渠道继续使用被另一渠道撤回的 profile。

客户端不直接读取 Registry。服务端可在普通 check 响应中附带短期 `helperCleanupAuthorization`，但只有当不存在 activeValidateOperation、未消费安装事务或其他活跃本地事务，且服务端已有一条 enabled cleanup edge 时才可签发。该 Compact JWS 固定 `aud=pinvou-helper-cleanup,tokenUse=helper-cleanup`，绑定 installationScopeId、targetKey、beforeProfileId、精确有序 removeHelperFacts、afterProfileId、sourceFactsRegistryVersion、channelRevision、selectionGeneration、iat、`exp<=iat+2min` 和唯一 jti。launcher 必须用当前 Root 授权的 Online Authorization 公钥验签，重新计算本机 beforeProfileId、确认无活跃事务、逐项验证删除目标不是 activeHelperRef 且未被事务引用，再以 12.8 的事实 intent 原子删除；完成结果必须精确等于 afterProfileId，授权在本地受保护日志中只消费一次。过期、账本/generation 与最新 check 不同、before 不同、离线无法取得当前 check、断电后已提交或任一条件不符都不得继续删除；完成或恢复完成后必须丢弃原决定并以新 facts 发起普通 check。服务端不得仅返回无签名删除建议，客户端也不得自行猜测 Registry 允许集合。

`protectedUpdaterFactsRevision` 是 launcher 在本机受保护记录中的防崩溃提交序号，不是设备认证凭据或服务端跨请求防重放高水位。事实未变化的任意多次普通 check 必须允许重复同一 revision+currentUpdaterFactsSha256；事实事务每成功一次，launcher 在本地只接受严格大于其已提交高水位的下一 revision，并拒绝本地记录/日志回退。服务端不得从普通 `/check` 创建或推进 installationScopeId 级 revision 高水位，也不得因为新请求 revision 小于、等于或远大于历史请求就改变后续请求资格；它只校验正整数、当前请求内字段/摘要一致、语义事实命中 Source Facts Registry，并把精确 revision+摘要绑定到本次 decision/validate/consume。由此伪造极大 revision 最多使伪造请求自身失败或得到仅对其摘要有效的决定，不能持久阻断真实安装。全新安装/迁移初始化 revision=1；完整卸载生成新 installationScopeId 后重新从 1 开始，旧 scope 的服务端关联按 15.3 清理，不跨 scope 比较 revision。

有更新响应示例：

```json
{
  "requestId": "req_check_xxx",
  "decisionId": "dec_xxx",
  "decisionRevision": 1,
  "updateAvailable": true,
  "telemetrySessionCredential": "<compact-jws>",
  "decisionExpiresAt": "2026-09-30T10:15:00Z",
  "decisionToken": "<short-lived-signed-token>",
  "selectionMode": "direct",
  "selectionGeneration": 44,
  "sourceProfileId": "<source-profile-sha256>",
  "sourceFactsRegistryVersion": 18,
  "installIntentSha256": "<jcs-install-intent-sha256>",
  "selectionPolicy": "client-verified",
  "helperExecutionPlans": [
    {
      "packageId": "full-0.11.0-windows-x86_64",
      "transactionHelperId": "pinvou-updater-helper",
      "helperBytesInstallRequired": true,
      "helperHandoffRequired": true,
      "helperUpdateRequired": true
    }
  ],
  "targetMetadataRef": {
    "targetKey": "windows-x86_64-nsis",
    "version": 12,
    "length": 4096,
    "sha256": "<target-metadata-response-sha256>"
  },
  "releaseManifestRef": {
    "deploymentId": "dep_stable_windows_xxx",
    "version": 31,
    "length": 8192,
    "sha256": "<release-manifest-response-sha256>"
  },
  "releaseManifest": {
    "signed": {
      "_type": "release-manifest",
      "schemaVersion": 1,
      "releaseManifestVersion": 31,
      "releaseSequence": 42,
      "deploymentId": "dep_stable_windows_xxx",
      "releaseId": "rel_xxx",
      "releaseTargetId": "rt_windows_xxx",
      "releaseTargetApprovalClass": "stable",
      "productId": "pinvou-agent",
      "componentId": "pinvou-desktop",
      "targetVersion": "0.11.0",
      "minHelperProtocolVersion": 1,
      "minimumSourceVersion": "0.10.0",
      "channel": "stable",
      "targetKey": "windows-x86_64-nsis",
      "targetMetadataVersion": 12,
      "targetMetadataSha256": "<target-metadata-sha256>",
      "releaseNotes": {
        "zh-CN": "本次更新内容",
        "en-US": "Release notes",
        "ja-JP": "更新内容"
      },
      "platformIdentity": {
        "type": "windows-authenticode",
        "publisherSubject": "<approved-publisher-subject>",
        "leafSpkiSha256": ["<approved-spki-sha256>"],
        "rfc3161TimestampRequired": true
      },
      "postInstallLauncher": {
        "version": "1.2.0",
        "protocolVersion": 1,
        "installTargetId": "pinvou-updater-launcher-stable",
        "size": 2100000,
        "sha256": "<launcher-sha256>",
        "launcherAuthenticity": {
          "type": "windows-authenticode-helper-v1",
          "publisherSubject": "<approved-publisher-subject>",
          "leafSpkiSha256": ["<approved-spki-sha256>"],
          "rfc3161TimestampRequired": true
        }
      },
      "issuedAt": "2026-09-30T10:00:00Z",
      "expiresAt": "2026-12-01T00:00:00Z",
      "policy": {
        "enforcementPolicy": "optional",
        "downloadPolicy": "manual",
        "installPolicy": "interactive",
        "enforcementEffectiveAt": null,
        "deadlineAt": null,
        "gracePeriodSeconds": 0
      },
      "fullPackage": {
        "type": "full",
        "packageId": "full-0.11.0-windows-x86_64",
        "transactionHelperId": "pinvou-updater-helper",
        "format": "full-installer-v1",
        "size": 170000000,
        "sha256": "<full-package-zip-sha256>",
        "resultArtifactSize": 169000000,
        "resultArtifactSha256": "<installer-sha256>",
        "helperArtifacts": [
          {
            "helperId": "pinvou-updater-helper",
            "version": "1.4.0",
            "protocolVersion": 2,
            "filePath": "Files/Updater/pinvou-updater-helper.exe",
            "installTargetId": "pinvou-updater-helper-versioned",
            "size": 8400000,
            "sha256": "<helper-sha256>",
            "helperAuthenticity": {
              "type": "windows-authenticode-helper-v1",
              "publisherSubject": "<approved-publisher-subject>",
              "leafSpkiSha256": ["<approved-spki-sha256>"],
              "rfc3161TimestampRequired": true
            },
            "installMode": "versioned-handoff-v1"
          }
        ]
      },
      "incrementalCandidates": [
        {
          "type": "incremental",
          "packageId": "delta-0.10.5-to-0.11.0-windows-x86_64",
          "transactionHelperId": "pinvou-updater-helper",
          "format": "artifact-delta-v1",
          "algorithm": "bsdiff-v1",
          "baseVersion": "0.10.5",
          "targetVersion": "0.11.0",
          "basePackageId": "full-0.10.5-windows-x86_64",
          "baseArtifactSize": 160000000,
          "baseArtifactSha256": "<base-installer-sha256>",
          "resultArtifactSize": 169000000,
          "resultArtifactSha256": "<target-installer-sha256>",
          "size": 32000000,
          "sha256": "<delta-package-sha256>",
          "helperArtifacts": [],
          "minUpdaterProtocolVersion": 1
        }
      ],
      "packageManifestSha256": "<package-manifest-sha256>",
      "requiredCapabilities": [],
      "dataMigration": {
        "mode": "backwardCompatible",
        "sourceAppVersionRange": {
          "minInclusive": "0.10.0",
          "maxExclusive": "0.11.0"
        },
        "backupPolicy": "none",
        "backupScopeIds": [],
        "backupScopeRegistryVersion": null,
        "backupScopeRegistrySha256": null,
        "backupHashAlgorithm": "none",
        "backupContainerFormat": "none",
        "maxBackupBytes": 0,
        "maxBackupDurationSeconds": 0,
        "snapshotVerification": "none"
      },
      "restartRequirement": "application"
    },
    "signatures": [
      {
        "keyId": "release-2026-a",
        "algorithm": "ed25519",
        "value": "<base64url-no-padding>"
      },
      {
        "keyId": "release-2026-b",
        "algorithm": "ed25519",
        "value": "<base64url-no-padding>"
      }
    ]
  },
  "nextCheckAfterSeconds": 21600
}
```

无更新响应示例：

```json
{
  "requestId": "req_check_xxx",
  "decisionId": "dec_xxx",
  "decisionRevision": 1,
  "updateAvailable": false,
  "telemetrySessionCredential": "<compact-jws>",
  "decisionExpiresAt": "2026-09-30T10:15:00Z",
  "decisionToken": "<short-lived-signed-no-update-token>",
  "selectionGeneration": 44,
  "sourceProfileId": "<source-profile-sha256>",
  "sourceFactsRegistryVersion": 18,
  "reason": "already_latest",
  "supportGuideCode": null,
  "nextCheckAfterSeconds": 21600
}
```

`reason` 仅使用 `already_latest`、`current_version_ahead_of_channel`、`channel_has_no_baseline`、`no_compatible_update` 等不泄露内部规则的稳定枚举，不单独返回“未命中灰度”，也不返回不兼容版本、其他平台或候选详情。所有成功检查结果都必须签发 decisionToken；无更新令牌绑定通用字段中的 requestNonce、两类安装 ID、currentVersion、channel/channelRevision、targetKey、hostArch、osVersion 摘要、selectionGeneration、sourceProfileId、sourceFactsRegistryVersion、protocolVersion、capabilitiesSha256、`currentUpdaterFactsSha256`、decisionId、`updateAvailable=false` 和 reason，禁止 selectionMode 及 Deployment/Release/Package/Manifest/助手字段。客户端必须先验签，并逐项比对本次请求保存的上述字段（包括 currentUpdaterFactsSha256 和响应中的两个 Source Registry 字段）后，才接受无更新结论或上报事件。SN 缺失或无效不是错误码；若因此未命中候选但存在可升级基线或桥接版本，仍应返回该版本。`decisionToken` 不包含 SN，有效期最多 15 分钟，只用于文件信息和安装复核；有更新时其 `packageIds` 只包含本次 Release Manifest 允许选择的目标完整包和增量候选。同一令牌只允许在绑定相同的 `currentVersion`、`installId`、`installationScopeId`、channelRevision 和请求随机数时重复查询文件信息。

服务端先按 10.5 选择单个目标发布，客户端不得自行改选另一个发布版本。客户端先将缺省 `minimumSourceVersion` 解析为 `0.0.0`，再只在该 Release Manifest **且 decisionToken.packageIds 允许集合**内按以下顺序选择制品：验证 `currentVersion >= resolvedMinimumSourceVersion` 且 `currentVersion < targetVersion` → 确认目标/能力 → 查找完全匹配且再次校验通过的本地基础制品 → 在满足节省率和空间条件时选最小增量候选 → 否则选允许集合中的 `fullPackage`。若允许集合不含可用完整包且没有满足条件的允许增量，必须重新检查/走兼容桥接，不能退回 Manifest 中被过滤的 packageId。首期客户端因能力数组为空而只会选择允许的完整包。`selectionMode=compatibility_bridge` 的版本安装并通过健康检查后，客户端应按正常退避规则重新检查后续版本，不在同一安装事务中连续升级。

`supportGuideCode` 仅在需要人工处理时返回允许列表中的机器码，其他情况为 null，不允许携带 URL。

#### 11.2.1 长流程中的决定刷新与本地成果复用

decisionToken 的 15 分钟是授权前的新鲜度窗口，不要求 4 GiB 下载、用户等待或最长 24 小时备份在窗口内完成。客户端在 token 过期、距离过期不足 2 分钟，或完成系统权限获取及必要的最终备份、准备调用 validate 前，必须使用当前真实版本、受保护 channelRevision、全新 requestNonce 和当前 OS/能力重新调用普通 `/check`，不得自行延长旧 token。权限对话或最终备份后若新 token 又过期，必须再次刷新；validate 只接受最后一次新鲜决定。

刷新请求携带 `refreshOfDecisionId`、旧 telemetrySessionCredential 和服务端最后确认的 lineage sequence，不使用已过期 decisionToken 取得安装权。只有旧遥测凭据签名真实、`aud/tokenUse` 与 Schema 正确、尚未 replaced/revoked，且其中 installId、installationScopeId、decisionId、telemetrySessionId、decisionRevision 与服务端记录和本次请求完全匹配时，才有权读取或修改旧 lineage。若该凭据仍在 `exp` 内、会话处于 11.5 列出的可刷新状态、安装意图等价且服务端仍允许，服务端保留同一 `decisionId`/`telemetrySessionId`、严格提升 `decisionRevision`，签发绑定新 nonce/generation 的 decisionToken 与替代遥测凭据，并原子把旧凭据置为 replaced；服务端已持久化的规范状态和 telemetrySessionId lineage sequence 延续，新凭据不能重置 sequence 或重放早期阶段。替换前客户端先用旧凭据上传仍在 occurredAt 窗口内的队列；替换后旧凭据只返回 `credential_replaced`。若凭据真实且绑定完全匹配，但意图不等价，服务端仅可从允许刷新状态原子把旧会话收敛为 `decision_superseded` 并创建新 decisionId/会话，客户端从展示/确认重新开始。凭据过期后仅在 `exp + 24h` 内允许幂等的“关闭旧 lineage 并新建独立会话”，不得等价续接旧 sequence、确认、备份或权限上下文；超过该窗口旧会话不再可由客户端修改。坏签名、错误类型、跨安装/决定绑定、revoked/replaced 凭据或旧 sequence 不匹配必须返回稳定错误，且不得读取、关闭或以任何方式修改旧会话；客户端只能去掉 refresh 参数发起全新普通 `/check`，新会话也不得声明与旧 lineage 连续。`decisionRevision` 和 telemetrySessionId 必须进入两类凭据、validate 及事件绑定。

决定刷新属于有副作用的 `/check`：客户端必须发送随机 128-bit `Idempotency-Key`，并在响应未知时复用完全相同的 key、refreshOfDecisionId、旧凭据字符串、lastAcknowledgedSequence、requestNonce 及规范语义对象。服务端以 `installationScopeId + Idempotency-Key` 保存 11.1 的规范请求摘要和幂等记录；先完成旧凭据的纯密码学/固定类型/Schema/不可变绑定校验并确认 kid 未 revoked，再在检查 `exp`、replaced 或当前 lineage 状态前查询该记录。同摘要 processing 按 11.1 判断返回 in-progress 或由过期租约 CAS 胜者恢复；同摘要 committed 重试即使旧凭据或原 decisionToken 已过期，也返回原 decisionId/revision、decisionToken、telemetrySessionCredential 和全部业务字段，但生成新的 HTTP requestId；这只恢复原提交结果，不延长其中任何时间窗口。变更 nonce、sequence、旧凭据字符串或规范摘要返回 `idempotency_conflict`，不得再次替换或新建会话。无记录时才执行上一段首次刷新逻辑并严格检查新鲜度；committed 结果从 committedAt 保留 24 小时。客户端恢复原结果后若其中 decisionToken 已过期，必须按正常规则再次刷新，不能把它用于 validate；revoked kid 始终拒绝。

refresh 选择新决定时必须生成与普通 check 相同的 selectionReadSet，并额外保存旧 lineageConcurrencyVersion、decisionRevision、credential jti/replaced 标志与 sequence。最终替换旧凭据、提升 revision、创建新会话或提交 superseded 的事务必须同时满足 11.1 live-owner/live-lease、lineage CAS、marker 不存在以及完整 selectionReadSet 未变化；即时 deny、Registry/Snapshot 指针、generation 或任一发布资格在读取后改变时，预计算替代凭据全部丢弃，并在当前租约内按最新一致性快照重新判断“等价刷新、superseded/新会话或封闭错误”。不得在安全撤回提交后签出仍引用旧资格的新 refresh 凭据。

服务端为有更新决定计算 `installIntentSha256`：对安装意图投影对象按 RFC 8785 求 SHA-256。投影固定包含 product/component、实际来源 canonicalAppVersion、sourceProfileId、sourceFactsRegistryVersion、installationScopeId、channel/channelRevision、targetKey/hostArch/osVersion 摘要、目标版本、resolvedMinimumSourceVersion、`selectionMode`、对应 `rolloutDecisionKind`，以及 `candidate_prerequisite`/直接候选语义下的 rolloutId；还包含 Deployment/Release Target、Target 业务策略摘要（support floor、installScope、hostConstraints）、Release 业务摘要（策略、结构化 sourceAppVersionRange、backupPolicy、backupScopeIds、backupScopeRegistryVersion、backupScopeRegistrySha256、backupHashAlgorithm、backupContainerFormat、maxBackupBytes、maxBackupDurationSeconds、snapshotVerification 等静态 dataMigration 全量字段、发布说明、平台身份、postInstallLauncher、restartRequirement）、Package Manifest 身份、全部允许 packageId 的容器/基础/结果制品身份、helperArtifactsIndexSha256、按 packageId 排序的 transactionHelperId 与三个助手执行计划字段、`currentUpdaterFactsSha256` 和所需能力。运行时 `backupFacts` 不进入该决定期投影，而在 validate 和安装授权中单独绑定。投影排除 decisionId、requestNonce、selectionGeneration、Manifest 机械续签版本、issuedAt/expiresAt、签名、下载 URL、原始 SN 和分桶 HMAC 值。相同业务语义的纯时间续签得到相同摘要；selectionMode、候选前置语义或任何会改变用户确认、兼容性、制品、助手、launcher、备份或安装行为的字段变化都必须改变摘要。

只有按上一段通过未过期 lineage 凭据完成的等价刷新，且刷新后的 `installIntentSha256`、currentVersion、channelRevision、installationScopeId、targetKey、hostArch/osVersion 摘要和所选 packageId/大小/哈希全部与旧决定一致时，客户端才可复用已校验的 `.partial`/完整 staging；备份还必须在受保护记录中绑定该摘要、数据源 revision/哈希、backupScopeIds、scope 注册表版本、maxBackupBytes、实际字节数、snapshotVerification 和完成时间，且源数据未发生变化，才可复用；用户确认也必须绑定同一摘要、selectionMode 与确认文案版本。即使可复用，本地版本、磁盘、包锁、助手、OS、渠道和发布状态预检仍须重跑。任一等价条件不满足，或旧凭据过期/无效时，旧确认、备份和权限上下文全部失效，流程回到新决定的展示/确认阶段；旧 staging 不得直接用于 validate，仅可在新决定下按完整大小、哈希和真实性重新验证后作为普通内容缓存使用。刷新结果为无更新、其他目标或错误时停止原事务。

### 11.3 更新文件信息

```text
POST /api/v1/update/packages/{packageId}/download-info
```

请求携带 `decisionToken`、`deploymentId`、`releaseId`、`releaseTargetId`、当前版本、安装实例 ID 和安装范围 ID，不重复提交硬件 SN。服务端验证令牌绑定、Deployment/发布目标/制品当前状态及 packageId 是否属于令牌允许范围，不以 SN 重新执行准入判断。请求示例：

```json
{
  "deploymentId": "dep_stable_windows_xxx",
  "releaseId": "rel_xxx",
  "releaseTargetId": "rt_windows_xxx",
  "decisionToken": "<compact-jws>",
  "currentVersion": "0.10.5",
  "installId": "550e8400-e29b-41d4-a716-446655440000",
  "installationScopeId": "4f8a60ac-c3d8-48c8-a678-ec4c475b6771"
}
```

```json
{
  "requestId": "req_download_xxx",
  "decisionId": "dec_xxx",
  "deploymentId": "dep_stable_windows_xxx",
  "releaseId": "rel_xxx",
  "releaseTargetId": "rt_windows_xxx",
  "packageId": "full-0.11.0-windows-x86_64",
  "package": {
    "size": 170000000,
    "sha256": "<full-package-zip-sha256>"
  },
  "url": "https://third-party-files.example.com/path/package.zip?signature=...",
  "expiresAt": "2026-09-30T10:15:00Z",
  "supportsRange": true,
  "etag": "<optional-strong-etag>",
  "objectVersion": "<optional-immutable-provider-version>"
}
```

- URL 必须为 HTTPS、有效期不超过 15 分钟，不得含明文用户名或密码。
- 服务端只能从后台已审核并版本化的文件服务配置签发下载域名，配置变更须审计；客户端不硬编码具体供应商域名，但必须拒绝 IP 字面量、localhost、环回/链路本地/私有地址、非 HTTPS 降级和 URL 中的 userinfo，并对连接时 DNS 解析得到的每个地址执行同样的网段检查。默认不跟随重定向；确需重定向时，每一跳都重新执行相同校验，最多 3 跳且不得把控制面凭据或原 URL 查询参数转发到新主机，以防 SSRF、DNS 重绑定和凭据泄漏。
- 文件服务签名 URL 必须绑定不可变对象键、允许的 `GET/HEAD` 方法和过期时间；禁止授予列目录、写入或删除权限。若供应商无法限制 HTTP Range，请求仍只能读取同一不可变对象。
- 客户端不得长期缓存域名、URL 或完整查询参数，也不得把它们写入日志/遥测。
- 访问数据面时不得附带 SN、installId、decisionToken 或控制面凭据。
- URL 过期后重新请求；packageId、大小和 SHA-256 必须一致，并且至少存在一个可跨 URL 比较的强不可变验证器（强 ETag 或对象版本）才可续传；若响应同时提供两者，则两者都必须与原请求一致。供应商两者均没有、只提供弱 ETag 或任一已提供验证器变化时，URL 换新后必须从头下载。
- 文件服务适配器必须把统一 `objectVersion` 映射到供应商的强对象世代/版本字段，并声明用于首请求、续传请求和条件读取的确切请求参数/Header；响应必须回显可验证的同一版本，否则按验证器变化处理。适配器合约至少固定输入 `objectKey + objectVersion + rangeStart + strongEtag`，输出 HTTP 状态、Content-Range、总长、响应对象版本和强 ETag，并用同一组供应商固定测试向量验证首个 200、正确 206、版本不匹配/412、对象被覆盖和换签 URL；供应商仅把版本放在不回显且不能条件读取的查询参数中，不视为强 objectVersion。
- 生产文件服务必须为所有包提供不可变对象版本或强 ETag；包大于 100 MiB 时还必须支持 Range，因此 4 GiB 上限不会依赖单个 15 分钟连接完成。已在 URL 到期前建立的连接能否继续由供应商明确承诺并纳入接入测试，但客户端始终必须支持到期后按强验证器申请新 URL 续传；不满足这些能力的供应商不得承载生产升级包。
- `decisionToken` 已过期时，客户端必须重新执行检查更新取得新决定；只有新决定仍指向同一 `releaseId + packageId + packageSha256` 且发布状态允许时才能申请新 URL 并按上一条规则续传。重新检查未命中、发布暂停/撤回或制品身份变化时停止下载并清理不可信临时文件。
- 暂停后通常拒绝新地址；唯一例外是 8.1 已登记允许的旧 `rolloutDecisionKind=baseline-prerequisite`，若候选暂停是 generation 变化的唯一原因，且其基线 packageId、Target/Release 业务策略、渠道 revision、Manifest 与制品身份逐字未变，则仍可为该**基线包**签发 download-info，并继续按同一例外接受 validate/consume，绝不为候选包签发地址。安全撤回、基线暂停/撤回、Target 或制品变化没有此例外；安全撤回后在 5 分钟内同时停止签发、吊销安装授权并请求文件服务/CDN 失效已有 URL。无论 URL 是否仍可下载，客户端均须通过安装前复核。

### 11.4 安装前复核

```text
POST /api/v1/update/deployments/{deploymentId}/validate
```

调用 validate 前，客户端必须先用当前 telemetrySessionCredential 上传并获得服务端逐条确认，使该 telemetrySessionId lineage 的规范状态确切到达本次发布的 `authorizationReadyState`：`backupPolicy=none` 时为 `permission_granted`，`backupPolicy=required` 时为 `backup_succeeded`。网络不可用、队列仍有未确认的安全关键前置状态，或 required 流程的强制写冻结/quiesceFencingToken 已不连续有效时不得调用 validate。请求包含 decisionToken、当前 telemetrySessionCredential、telemetrySessionId、服务端最后确认的 `lastAcknowledgedSequence`、目标 packageId、已下载包容器大小/SHA-256、最终安装器大小/SHA-256、`transactionHelperId`、`helperArtifactsSha256`、三个助手执行计划字段、受控助手重新计算的 `currentUpdaterFactsSha256`、decision 绑定的 sourceProfileId/sourceFactsRegistryVersion、installId、installationScopeId、channelRevision、当前规范化 OS 摘要、助手复核的当前版本、完整 `backupFacts` 和本地状态事务 ID，不包含 hardwareSn。`transactionId` 由客户端以 UUIDv4 为每次安装尝试生成，在同一 installationScopeId 下永不复用并在请求前写入系统级受保护事务存储。完整包流程从 `FullPack.zip` 安全解压得到最终安装器；增量流程从补丁包和可信基础安装器重建得到最终安装器。两条路径都必须在本地重新验证包容器大小/哈希、最终安装器大小/哈希、助手数组及平台真实性。外部升级助手必须重新读取实际安装版本、安装范围、渠道 revision、OS、备份注册表身份、完整 helper/launcher 事实与 revision，要求 currentUpdaterFactsSha256 等于 decision/installIntent 绑定，并复算 transactionHelperId 与三个执行计划字段；服务端以实际 facts 重新计算 sourceProfileId，要求 Registry 当前指针/版本、profile 状态及所选 edge 资格仍与决定一致。同时验证 required 流程的原数据源句柄身份、冻结世代及 quiesceFencingToken 仍等于该备份事务的受保护记录，确认实际版本严格低于 Release Manifest 的 `targetVersion`，否则返回稳定不匹配错误并拒绝安装：

validate 必须使用 `Idempotency-Key=transactionId`。服务端以 `installationScopeId + transactionId` 保存 11.1 的规范请求摘要及成功/业务失败幂等结果。处理顺序固定为：先对 decisionToken 与 telemetrySessionCredential 做纯密码学/结构校验（签名、固定 aud/tokenUse/Schema、不可变安装/决定绑定）并确认 kid 未 revoked；随后在检查两类凭据 exp、decision 新鲜度、lineage 当前状态或发布窗口之前查询该 transactionId 的幂等记录。相同 transactionId/key 下规范摘要不同返回 `idempotency_conflict`；摘要相同的 processing 按 11.1 判断未到期返回 in-progress、到期则由 CAS 胜者接管恢复；摘要相同且 committed 的命中返回原 valid、原 installAuthorization/expiresAt 或原稳定失败并生成新 requestId，即使凭据此时已过期或 lineage 已终态，也不重新授权、不延长窗口，committed 结果从 committedAt 保留 24 小时。使用不同 transactionId 不会命中旧记录：无本事务记录但同一 lineage 已有活动的 `activeValidateOperation` 时返回 `authorization_in_progress`；首个操作已提交为 install_authorized/authorization_failed/authorization_coordination_aborted 时返回 `lineage_state_conflict`，绝不返回 idempotency_conflict 或创建第二授权。无记录、无活动 marker 且仍为该发布对应的 authorizationReadyState 时，才严格检查凭据新鲜度和发布资格并创建本事务 processing reservation；revoked kid 无论是否命中记录都立即拒绝。lastAcknowledgedSequence、decision revision、制品/backupFacts 或其他语义字段只有在同一 transactionId/key 下变化时才属于幂等冲突。

```json
{
  "decisionToken": "<compact-jws>",
  "telemetrySessionCredential": "<compact-jws>",
  "telemetrySessionId": "ts_xxx",
  "lastAcknowledgedSequence": 12,
  "packageId": "full-0.11.0-windows-x86_64",
  "packageSize": 170000000,
  "packageSha256": "<full-package-zip-sha256>",
  "resultArtifactSize": 169000000,
  "resultArtifactSha256": "<installer-sha256>",
  "transactionHelperId": "pinvou-updater-helper",
  "helperArtifactsSha256": "<jcs-helper-array-sha256>",
  "helperBytesInstallRequired": true,
  "helperHandoffRequired": true,
  "helperUpdateRequired": true,
  "currentUpdaterFactsSha256": "<current-updater-facts-sha256>",
  "sourceProfileId": "<source-profile-sha256>",
  "sourceFactsRegistryVersion": 18,
  "installId": "550e8400-e29b-41d4-a716-446655440000",
  "installationScopeId": "4f8a60ac-c3d8-48c8-a678-ec4c475b6771",
  "channelRevision": 7,
  "osVersionSha256": "<jcs-os-version-sha256>",
  "currentVersion": "0.10.5",
  "backupFacts": {
    "sourceAppVersion": "0.10.5",
    "hashAlgorithm": "none",
    "containerFormat": "none",
    "scopeRegistryVersion": null,
    "scopeRegistrySha256": null,
    "dataSourceRevisionSha256": null,
    "actualBytes": 0,
    "durationMilliseconds": 0,
    "snapshotSha256": null,
    "completedAt": null
  },
  "transactionId": "9d40d20e-3944-4f6b-8b0c-b71ef768f1aa"
}
```

成功响应示例：

```json
{
  "requestId": "req_validate_xxx",
  "decisionId": "dec_xxx",
  "valid": true,
  "installAuthorization": "<compact-jws>",
  "expiresAt": "2026-09-30T10:05:00Z"
}
```

validate 只接受服务端已持久化状态等于发布策略对应 authorizationReadyState 且 credential/revision/sequence 完全匹配的 lineage；policy=required 还要求 backupFacts 非空语义、备份能力与决定绑定一致，并强制 `helperBytesInstallRequired=false` 且 `helperHandoffRequired=false`，否则以 `helper_identity_mismatch` 失败关闭，客户端随后在 consume 前继续证明本地冻结连续性。令牌、幂等、sequence 和状态等前置校验通过后，当前 processing owner 必须先以 lineage 唯一索引创建并独立提交可观察的 `activeValidateOperation`。该 marker 固定包含 telemetrySessionId、transactionId、幂等记录作用域/键/摘要、ownerEpoch、ownerStartedAt、leaseExpiresAt、创建前读取的 expectedLineageConcurrencyVersion、decisionRevision、telemetry credential jti、lastAcknowledgedSequence、状态 `processing`，以及足以独立恢复的不可变 `recoveryProjection`。该投影包含已通过密码学校验的两类 token kid/jti/必要 claims、Schema 后 validate 语义请求、Manifest/发布引用及其规范摘要，不含私钥或 hardwareSn，并按 15.3 服务端敏感授权数据要求加密、限权和随 marker/tombstone 清理；后台恢复不得依赖客户端再次提交请求体。创建 marker 的数据库事务必须同时要求：幂等记录仍为相同 endpoint/scope/key/digest 的 processing、仍归当前 ownerEpoch 且 `trustedNow < leaseExpiresAt`；lineageConcurrencyVersion、decisionRevision、当前 credential jti/replaced 标志、最后 sequence 和 authorizationReadyState 仍与前置快照逐项相等；marker 不存在。成功时插入 marker 并把 lineageConcurrencyVersion 加一，把新值保存为 `reservedLineageConcurrencyVersion`。任一条件失败都不得创建 marker；尤其 `trustedNow == leaseExpiresAt` 的旧 owner 已无权提交。

该 marker 只是授权协调记录，不是安装授权或可消费结果；它存在时派生服务端可观察状态 `install_authorization_requested`，因此 refresh 和其他 transactionId 的 validate 能稳定返回 `authorization_in_progress`。刷新最终事务必须 CAS 自己读取的 lineageConcurrencyVersion 并要求 marker 不存在；若 marker 抢先创建，刷新 owner 必须确认自己的 operation 尚无领域副作用，删除自己的 processing reservation，回到固定管线返回 authorization_in_progress，绝不能替换 credential 或提升 decisionRevision。反之，刷新先提交时会提升 lineageConcurrencyVersion/替换 credential，使旧 validate 的 marker 创建 CAS 失败。若另一 validate 抢先创建 marker，失败方只可在仍拥有自己 processing ownerEpoch 且确认零领域副作用时删除自己的 reservation；若本操作的幂等 ownerEpoch 已被接管，旧 owner 不得删除、创建 marker 或返回成功。

marker 存在期间，每次续租必须在一个数据库事务内把幂等记录和 marker 的同一 ownerEpoch、ownerStartedAt、leaseExpiresAt 一起更新；任一不一致立即停止处理，不能形成分裂 owner。接管也必须在一个事务内同时 CAS 两条记录，并确认 lineageConcurrencyVersion 仍等于 reservedLineageConcurrencyVersion；后台恢复器使用相同规则，不允许只取得其中一条后继续。若在 marker 提交后崩溃，同 transactionId 重试或后台恢复器只能在租约到期后取得两条记录的同一新 owner；其他请求继续返回 in-progress，绝不能并行复核或签发第二授权。

最终授权事务必须按 11.1 再次校验幂等记录与 marker 仍归当前 ownerEpoch、两者的 ownerStartedAt/leaseExpiresAt 一致且当前租约未到期，并要求 lineageConcurrencyVersion 等于 reservedLineageConcurrencyVersion；随后重验两类凭据 kid 未 revoked、凭据 exp、decision/credential 未替换、sequence、channelRevision、selectionGeneration、sourceFactsRegistryVersion/sourceProfileId/所选 edge 当前资格、Deployment/Release Target/Artifact/Manifest、Rollout 与安全 deny。全部动态条件仍有效时，业务复核成功才写 `install_authorized` 和一次性授权；制品、helper/launcher、实际版本或 backupFacts 等首次业务复核失败才写终态 `authorization_failed`。两类结果都必须在一个事务中提交幂等结果、删除 marker 并提升 lineageConcurrencyVersion；租约或 owner 栅栏失效时必须丢弃已生成的授权/响应，由当前 owner 或后继 owner 恢复，不能提交。

若 marker 已存在后，kid 吊销、凭据过期、发布暂停/撤回、generation/channel 变化、安全 deny 或其他发布前置条件失效，当前 owner 或后台恢复器必须走唯一 `coordination_abort`：按 11.1 在一个事务中校验 live owner/live lease、marker 和 lineage 栅栏，提交不含 installAuthorization 的 committed 幂等 tombstone（保存当时对应的稳定外层 HTTP/code），把 lineage 写为持久终态 `authorization_coordination_aborted`，记录 `coordinationAbortedAt`、稳定 reason/error code 与 `authorizationIssued=false`，删除 marker、关闭并删除该 lineage 的可写凭据索引并提升 lineageConcurrencyVersion。该状态不是 `authorization_failed`、安装失败或质量阈值样本；旧 lineage 永远不能回到 authorizationReadyState、刷新或再次 validate，客户端如需继续只能发起不继承旧 lineage 的全新普通 `/check`。公开请求仍遵守步骤②的 revoked 优先级，因此 revoked 客户端可只看到 credential_revoked；但后台恢复器不依赖再次接受该客户端凭据，看到 revoked 后只能关闭、绝不能授权。恢复扫描至少每 30 秒处理已过 leaseExpiresAt 的 marker；即使原客户端不再请求，也必须先接管有效租约，再收敛为 authorized、authorization_failed 或 authorization_coordination_aborted。客户端不得自行上报 requested、authorized、failed 或 coordination-aborted，也不能用本地积压事件跨越它们。`installAuthorization` 为一次性令牌，绑定上述字段和该 lineage 的授权状态版本，有效期最多 5 分钟；其中 `packageSize + packageSha256` 证明所选下载包，`resultArtifactSize + resultArtifactSha256` 证明实际将执行的安装器，四者均必须匹配 Release/Package Manifest。外部升级助手在调用平台安装器前必须调用：

validate 的业务拒绝统一返回 HTTP 422 和封闭响应 `{"requestId":"...","decisionId":"...","valid":false,"error":{"code":"<11.6-code>","message":"...","retryable":false,"supportGuideCode":null,"details":{}}}`，禁止出现 installAuthorization/expiresAt；纯认证、幂等、并发或发布状态前置错误仍使用 11.6 对应 HTTP 外层错误结构且不得伪装成 valid=false。成功授权必须在 JWS 中额外绑定 `backupFactsSha256=SHA256(RFC8785(backupFacts))`；policy=required 时还绑定 snapshotSha256 与 dataSourceRevisionSha256，consume 前受控助手再次对本地受保护记录计算同一摘要并逐字比对。

```text
POST /api/v1/update/install-authorizations/consume
```

validate 成功后若用户仍在 consume 前明确取消，客户端调用幂等端点：

```text
POST /api/v1/update/install-authorizations/cancel
```

请求携带原 installAuthorization 与 transactionId，使用 `Idempotency-Key=cancel:<transactionId>`。服务端以 `installationScopeId + transactionId + cancel` 保存规范请求摘要和取消幂等结果。判定优先级固定为：① 校验签名、固定类型/Schema 和 kid revoked；② 校验 request transactionId 与 `cancel:<transactionId>` 是否匹配 JWS claim，及 claim 的 installId/installationScopeId 是否匹配服务端授权记录，不符唯一返回 `install_not_authorized`；③ 以服务端授权 scope + transactionId + cancel 定位记录，同一 key 已有记录但规范摘要不同返回 `idempotency_conflict`；④ 摘要相同的 processing 按 11.1 判断返回 in-progress 或由过期租约 CAS 胜者恢复；⑤ 摘要相同的 committed 命中即使授权已过期也返回原取消结果和新 requestId，不再次 CAS；⑥ 无记录才检查 exp/状态并首次取消。首次取消要求 `trustedNow < exp` 且 lineage 仍为 install_authorized、授权尚未消费，并 CAS 为 cancelled_before_install；committed 结果从 committedAt 保留 24 小时。服务端定时任务在 trustedNow>=exp 时把仍为 install_authorized 的 lineage CAS 为 authorization_expired；若任务尚未运行，无记录的到期 consume/cancel 请求也先尝试同一过期 CAS。consume、cancel 和 expiry 共用授权状态版本，只能一个成功：consume 成功后 cancel 返回 authorization_already_consumed，cancel 成功后 consume 返回 authorization_cancelled。revoked kid 优先于全部记录；使用另一事务、安装实例、scope 或错误 cancel key 不能查询原取消结果。

请求携带 `installAuthorization` 和其中已绑定的 `transactionId`，并以该事务 ID 作为 `Idempotency-Key`。**首次消费**只允许从服务端已持久化的 `install_authorized` 状态发生，且必须在 JWS `exp` 前到达；所有流程的受控助手都必须在发出请求的最后一刻重新读取受保护 updater 事实并要求 currentUpdaterFactsSha256 与授权完全相等，同时复算 transactionHelperId、helperBytesInstallRequired、helperHandoffRequired 及派生 helperUpdateRequired，漂移时先 cancel/等到期而不得 consume。required 流程还必须复核同一进程/boot 身份、全部强制锁、原数据源句柄身份和 quiesceFencingToken 连续有效，且授权中的两个原始助手布尔值均为 false，任一不符先 cancel/等到期而不得 consume。服务端在一次事务中检查签名/类型/到期时间、授权状态版本、当前 `selectionGeneration`、channelRevision、sourceFactsRegistryVersion/sourceProfileId/所选 edge 当前资格、Deployment/Release Target/Artifact 状态，以及决定绑定 Rollout 的授权资格，再把授权 `jti`、transactionId、幂等键和请求绑定摘要标记为已消费，原子把遥测 lineage 置为 `handed_off_to_install_transaction` 并创建安装事务 `authorization_consumed`；成功返回 200 及服务端签发的 `installTransactionCredential`。实际候选决定只在 Rollout 仍为 running 且 generation 精确匹配时允许；Rollout paused、aborted、completed 或已晋升为基线后，旧候选决定都必须重新检查，不能因制品相同跳过新 generation。`candidate_prerequisite` 只授权其安全基线包，候选 Rollout 后续暂停不阻止符合 8.1 唯一兼容例外的基线安装该基线，但该事务绝不授权候选包。

为处理“服务端已消费但响应丢失”，consume 的判定优先级固定为：① 校验签名、固定类型/Schema 和 kid revoked；② 校验请求 transactionId/Idempotency-Key 是否分别等于 JWS claim transactionId，及令牌中的 installId/installationScopeId 是否与其服务端授权记录一致，不一致返回 403 `install_not_authorized`；③ 以 `jti + transactionId + Idempotency-Key` 定位已有记录，若存在但 11.1 规范请求摘要不同，返回 409 `idempotency_conflict`；④ 摘要相同的 processing 按 11.1 判断返回 in-progress 或由租约 CAS 胜者接管；⑤ 摘要相同的 committed 命中在 24 小时内返回逐字段相同的业务结果、原 installTransactionCredential 和原启动窗口，并生成新 requestId；⑥ 无记录才走首次消费。步骤⑤即使原授权已过 exp 仍可查询，但不重新执行发布资格检查、不创建新授权、不延长任何启动窗口。首次消费严格要求 `trustedNow < exp`；过期后首次到达时，把仍处于 install_authorized 的 lineage 原子置为 authorization_expired。kid revoked 优先于所有记录；使用另一授权 jti、事务、安装实例或安装范围不能查询原结果。幂等记录保留期从首次成功提交起精确 24 小时，`trustedNow == committedAt + 24h` 时不再返回凭据。该凭据至少绑定 `transactionId`、`installId`、`installationScopeId`、channelRevision、selectionGeneration、sourceProfileId、sourceFactsRegistryVersion、`decisionId`、`releaseId`、`releaseTargetId`、可选 rolloutId/decisionKind、`packageId`、目标版本、`currentUpdaterFactsSha256`、`transactionHelperId`、`helperArtifactsSha256`、`helperBytesInstallRequired`、`helperHandoffRequired`、派生的 `helperUpdateRequired`、包容器与最终安装器各自的大小/SHA-256、允许的事件阶段、事件状态机版本、签发时间、`installStartNotAfter`、`mainInstallerStartNotAfter` 和唯一 `jti`，只用于该安装事务的事件上报。`installStartNotAfter` 最迟为消费成功后 2 分钟，表示首次受保护动作：需要助手字节安装时为开始写入已验证助手；仅需所有权接管时为开始受保护 handoff；两者均不需要时为 transactionHelperId 指定的当前活动助手启动主安装器。`mainInstallerStartNotAfter` 最迟为消费成功后 5 分钟。幂等重放可以在其后返回原结果用于对账，但不得据此迟延启动。若任一窗口错过且相应动作未开始，原事务进入 `abandoned_before_install`，后续必须重新检查并申请新事务。凭据不得用于另一助手、制品或实例。

上述安装授权与 `installTransactionCredential` 还必须绑定当前 `deploymentId`，消费时重新检查的暂停/撤回/吊销状态必须分别对应该 Deployment、Release Target 和 Artifact，不得仅用 `releaseId` 粗粒度判断。

请求示例：

```json
{
  "installAuthorization": "<compact-jws>",
  "transactionId": "9d40d20e-3944-4f6b-8b0c-b71ef768f1aa"
}
```

成功响应示例：

```json
{
  "requestId": "req_consume_xxx",
  "decisionId": "dec_xxx",
  "transactionId": "9d40d20e-3944-4f6b-8b0c-b71ef768f1aa",
  "installTransactionCredential": "<compact-jws>",
  "installStartNotAfter": "2026-09-30T10:07:00Z",
  "mainInstallerStartNotAfter": "2026-09-30T10:10:00Z"
}
```

首期安装必须在线复核并消费授权。网络不可用、令牌过期、发布暂停/撤回、令牌绑定不一致或哈希不一致时失败关闭，不得离线开始安装；SN 缺失、变化或不可读取不得单独导致已签发的有效授权失败。授权成功消费后，即使网络中断，升级助手仍须观察安装器并记录最终状态；安装或健康检查失败时不自动安装旧版本，而是进入 `failed_manual_repair_required` 并展示人工修复入口。安装已经交给 OS 安装器后无法绝对远程撤销，此边界必须在安全响应界面明确；平台应尽快终止尚未提交的事务。

### 11.5 事件上报

```text
POST /api/v1/update/events
```

单批最多 100 条、请求体不超过 256 KiB。事件示例：

```json
{
  "credential": "<telemetry-session-or-install-transaction-compact-jws>",
  "events": [
    {
      "eventId": "550e8400-e29b-41d4-a716-446655440001",
      "sequence": 4,
      "occurredAt": "2026-09-30T10:20:00Z",
      "installId": "550e8400-e29b-41d4-a716-446655440000",
      "installationScopeId": "4f8a60ac-c3d8-48c8-a678-ec4c475b6771",
      "decisionId": "dec_xxx",
      "deploymentId": "dep_stable_windows_xxx",
      "releaseId": "rel_xxx",
      "releaseTargetId": "rt_windows_xxx",
      "packageId": "full-0.11.0-windows-x86_64",
      "targetKey": "windows-x86_64-nsis",
      "state": "failed_manual_repair_required",
      "errorCode": "engine_start_timeout",
      "retryable": false,
      "diagnostic": "required process did not become ready"
    }
  ]
}
```

检查更新成功响应必须签发 `telemetrySessionCredential`，至少绑定 `installId`、`installationScopeId`、`decisionId`、decisionRevision、telemetrySessionId、`targetKey`、`requestNonce`、`currentVersion`、`channel`、channelRevision、selectionGeneration、`hostArch`、`protocolVersion`、`capabilitiesSha256`、事件状态机版本、签发时间、`eventNotAfter` 和唯一 `jti`。其状态集合必须显式覆盖 consume 前的完整集合：检查/展示、下载/校验、`local_preflight_*`、`backup_started|backup_succeeded|backup_failed|backup_invalidated`、`permission_*`、`install_authorization_requested|install_authorized|authorization_failed|authorization_coordination_aborted|authorization_expired|cancelled_before_install|handed_off_to_install_transaction|decision_superseded`；凭据中逐项列出允许枚举，未列状态拒绝。其中 requested、authorized、authorization_failed、authorization_coordination_aborted 和 handed-off 是仅允许服务端/API 写入的状态，列入集合不授予客户端事件写入权。有更新时还必须绑定 `deploymentId`、`releaseTargetId` 及允许的 packageId，无更新时必须绑定 reason 且禁止伪造这些投放字段。安装授权消费后，安装、助手接管及健康阶段只能使用该事务的 `installTransactionCredential`，遥测凭据不能继续推进事务。

遥测凭据的 `eventNotAfter` 最长为签发后 24 小时、JWS `exp` 最长为 7 天；安装事务凭据的 `eventNotAfter` 最长为消费后 30 天、`exp` 最长为 37 天，用于覆盖 30 天内发生的断电/恢复事件及随后 7 天补报。事件时间必须满足 `credential.iat - 2min <= occurredAt <= eventNotAfter` 且 `occurredAt <= serverNow + 2min`；越界事件拒绝且不进入质量阈值统计。第 30 天事件发生窗口结束但不得合成不可变协议终态；服务端只标记可被迟到合法事件校正的派生运营状态 `overdue_reconciliation`，并可按 10.3 作为逾期失败样本。直到凭据 `exp` 前，仍接受 `occurredAt <= eventNotAfter` 且状态边合法的队列事件，包括第 30 天前已发生的成功终态。

第 37 天上传窗口结束仍无终态时，服务端才按最后可信阶段合成不可变超时终态：尚无 `helper_update_started` 或 `installer_started` 证据的 `authorization_consumed` 事务进入 `abandoned_before_install`；已经出现 `helper_update_started`、`helper_plan_succeeded`、`installer_started`、`reconciling`、`installation_verified` 或 `health_check_started` 的事务进入 `failed_manual_repair_required`。唯一例外是该事务任一有效期内事务凭据的 kid 在上传窗口结束前被安全吊销：它保持非协议派生状态 `credential_revoked_telemetry_unavailable`，不得根据缺失事件猜测本地阶段、不得合成上述终态或计入普通失败率；安全审计记录 revokedAt、kid、transactionId 和最后可信状态，并按事件留存期限清理。其他事务的合成事件记录 `reason=server_timeout_after_upload_window` 并遵守终态不可变规则。事件批次必须在 exp 前携带对应服务端签发凭据，服务端验证签名、固定 aud/tokenUse、绑定字段、允许阶段及事件发生时间，不接受无凭据、自签凭据、跨实例/决定/事务复用或用遥测会话冒充安装事务的事件。

sequence 不以凭据 jti 为作用域。consume 前的遥测事件以 `telemetrySessionId lineage` 为唯一序列作用域，从正整数开始且每个新事件必须严格大于服务端最后接受值；等价刷新替换凭据时延续服务端最后确认 sequence，绝不重置。consume 后的安装事件以 `transactionId` 为全新序列作用域，首个**客户端**事件使用 sequence=1 并贯穿该事务凭据的整个有效期；consume 在服务端创建的 `authorization_consumed` 是安装事务初始状态，不占用客户端 sequence。服务端分别以 `作用域 ID + sequence` 检查单调性，以 `作用域 ID + eventId` 幂等，并依据已持久化状态验证合法阶段迁移：不得阶段倒退、跳过状态机要求的客户端安全关键状态、从终态继续迁移，或把失败终态改写为成功；允许因离线批量上报一次提交连续多个合法阶段。逐条返回 `accepted`、`duplicate` 或 `rejected` 及稳定原因码，非法迁移使用 `invalid_event_transition`，但事件拒绝不得反向改变客户端已经确认的本地安装事实。事件不得携带明文 SN、文件路径、业务内容或下载 URL。

事件的规范状态只使用 `state` 字段；服务端按版本化映射表从已接受的 state 派生指标 `phase + status`，V1 客户端不得提交这两个字段。若兼容导入器收到旧客户端的 phase/status，只能在隔离字段中保存用于诊断，任何与 state 派生值冲突的输入均拒绝并记 `event_metric_conflict`，不得污染统计或改变状态。同一 sequence 作用域内，完全相同的 `eventId + sequence + payload` 可幂等重放，旧序号或相同序号不同内容必须拒绝并产生安全告警。允许 sequence 数值有空洞以容忍**未提交**的非关键进度事件丢失；空洞本身不代表状态被跳过，是否接受完全由“sequence 严格增大 + 当前服务端状态存在表中合法下一边”共同决定。`authorization_consumed` 是服务端初始状态而非客户端事件；`helper_update_started`（需要助手时）、`installer_started` 和终态等客户端负责的安全关键状态不得从本地队列丢弃，缺失时后续状态因非法迁移而拒绝。自由文本 `diagnostic` 最长 256 字节且必须经过客户端允许列表映射和路径/令牌脱敏；原始安装日志不得放入普通事件，只有用户明确同意的支持流程才能按单独上传协议提交。

有更新决定时，`telemetrySessionCredential` 除上述字段外还必须绑定 `deploymentId`、`releaseTargetId` 及允许的 packageId；无更新决定时这些字段必须省略，不得伪造一个投放。

V1 遥测会话的合法转换使用下表，不依赖图形缩进解释：

| 当前状态 | 下一状态 | 触发条件 | 是否终态 |
|---|---|---|---:|
| `decision_received` | `no_update` | 服务端无更新决定 | 是 |
| `decision_received` | `update_offered` | 更新已展示或等待展示 | 否 |
| `decision_received` | `download_started` | 用户已预先同意且策略允许自动下载 | 否 |
| `update_offered` | `deferred`、`download_started`、`user_cancelled` | 用户延期、开始下载或取消 | `user_cancelled` 是 |
| `deferred` | `update_offered`、`download_started`、`user_cancelled` | 到达重提示条件、开始下载或取消 | `user_cancelled` 是 |
| `download_started` | `download_succeeded`、`download_failed`、`user_cancelled` | 下载结果或下载阶段取消 | 后两者中仅 `user_cancelled` 是 |
| `download_failed` | `download_started`、`user_cancelled` | 允许重试或用户取消 | `user_cancelled` 是 |
| `download_succeeded` | `verification_succeeded`、`verification_failed` | 包容器及最终安装器校验结果 | 否 |
| `verification_failed` | `download_started`、`user_cancelled` | 清理不可信数据后重新下载或取消 | `user_cancelled` 是 |
| `verification_succeeded` | `awaiting_user_confirmation`、`local_preflight_started` | 交互安装先等待确认；已获确认/策略允许则开始本地预检 | 否 |
| `awaiting_user_confirmation` | `local_preflight_started`、`user_cancelled` | 用户确认或取消 | `user_cancelled` 是 |
| `local_preflight_started` | `local_preflight_succeeded`、`preflight_failed`、`user_cancelled` | 磁盘、任务、包锁、数据迁移和助手兼容检查 | `user_cancelled` 是 |
| `preflight_failed` | `local_preflight_started`、`user_cancelled` | 可重试问题消除后重检，或取消 | `user_cancelled` 是 |
| `local_preflight_succeeded` | `permission_requested`、`user_cancelled` | 所有流程先进入系统权限阶段，或取消 | `user_cancelled` 是 |
| `permission_requested` | `permission_granted`、`permission_denied`、`user_cancelled` | UAC/macOS/管理员授权结果 | `user_cancelled` 是 |
| `permission_denied` | `permission_requested`、`user_cancelled` | 用户重新发起权限交互，或取消 | `user_cancelled` 是 |
| `permission_granted` | `backup_started`、`install_authorization_requested`、`user_cancelled` | required 先由客户端取得强制冻结并开始最终备份；none 由 validate 提交 activeValidateOperation 后派生 requested；或取消 | `user_cancelled` 是 |
| `backup_started` | `backup_succeeded`、`backup_failed`、`user_cancelled` | 冻结下的可验证数据快照结果，或取消 | `user_cancelled` 是 |
| `backup_failed` | `backup_started`、`permission_requested`、`user_cancelled` | 权限上下文仍有效时重试；权限丢失则重新授权；或取消 | `user_cancelled` 是 |
| `backup_succeeded` | `install_authorization_requested`、`backup_invalidated`、`user_cancelled` | validate 提交 activeValidateOperation 后派生 requested；消费前冻结/权限/时限失效；或取消 | `user_cancelled` 是 |
| `backup_invalidated` | `permission_requested`、`user_cancelled` | 删除旧快照/keyRef、释放锁后重新取得权限和备份，或取消 | `user_cancelled` 是 |
| `install_authorization_requested` | `install_authorized`、`authorization_failed` | activeValidateOperation 存在时的服务端可观察派生态；最终事务提交授权或业务复核失败 | `authorization_failed` 是 |
| `install_authorization_requested` | `authorization_coordination_aborted` | marker 后凭据、发布资格或安全条件失效时，服务端 coordination_abort 关闭 lineage，不签发授权 | 是 |
| `install_authorized` | `cancelled_before_install`、`authorization_expired` | cancel API 的消费前 CAS，或服务端到期任务/到期请求 CAS | 是 |
| `install_authorized` | `handed_off_to_install_transaction` | consume 接口原子消费成功 | 是 |

validate/consume 服务端状态只能由对应 API 写入；policy=none 的授权入口状态是 `permission_granted`，policy=required 的授权入口状态是 `backup_succeeded`，二者分别是该发布的 authorizationReadyState。`install_authorization_requested` 不作为与最终结果同事务内不可见的普通 lineage 行值，而由已独立提交的 activeValidateOperation 派生；最终 validate 事务原子写 authorized/failed，或通过 coordination_abort 写终态 `authorization_coordination_aborted`，同时提交幂等结果、清除 marker 并提升 lineageConcurrencyVersion。consume 必须只从 `install_authorized` 原子地把遥测会话置为 `handed_off_to_install_transaction`，并创建以下安装事务初始状态 `authorization_consumed`。客户端不能用遥测凭据自行伪造 requested、authorized、authorization_failed、authorization_coordination_aborted 或该跨凭据迁移。`backup_invalidated` 只允许在 activeValidateOperation 创建之前由客户端上报；进入 requested 后若冻结失效，客户端不得 consume，须由原 transactionId 恢复 validate 结果后 cancel 或等待授权到期。发布状态或凭据在 marker 前失效使用既有外层错误；marker 后失效走 coordination_abort，绝不冒充 authorization_failed，也不重新暴露旧 ready state。UAC/系统权限拒绝和用户取消仍落入上表对应稳定状态/原因码，而不是共用含糊的“失败”。

11.2.1 的决定刷新只有在旧 lineage 凭据通过签名、类型、完整绑定和 sequence 校验后才可由服务端原子操作会话。允许等价刷新的状态封闭为：`decision_received|update_offered|deferred|download_started|download_failed|download_succeeded|verification_succeeded|verification_failed|awaiting_user_confirmation|local_preflight_started|preflight_failed|local_preflight_succeeded|permission_requested|permission_denied|permission_granted|backup_started|backup_failed|backup_succeeded`；未过期凭据在这些状态刷新不产生状态边，只提升 decisionRevision 并让替代凭据沿同一 telemetrySessionId lineage 和 sequence 继续。required 流程在 `backup_succeeded` 刷新时必须继续持有强制冻结，刷新不延长 postBackupQuiesceDeadline；刷新不等价、超时或失败使客户端删除快照并释放锁。`backup_invalidated` 只能转入重新授权或取消，不允许复用旧决定/备份等价刷新。`install_authorization_requested` 表示 validate 正在提交，刷新返回 `authorization_in_progress`；`install_authorized` 只能继续原授权的 consume/取消/到期流程；`authorization_coordination_aborted` 及其他所有终态都禁止等价刷新或再次 validate，只能由客户端发起无 lineage 继承的全新普通检查。真实且绑定匹配的凭据在允许刷新状态下若意图不等价，可原子进入 `decision_superseded` 并创建独立新会话。过期但仍可验证的凭据仅能在 `exp + 24h` 内幂等关闭旧 lineage，不能续接或复用成果；超过该窗口只允许全新普通检查且不修改旧状态。坏签名、错误类型、revoked/replaced、跨实例/决定绑定或旧 sequence 不得改变旧状态。`decision_superseded` 只表示旧决定被替代，不是安装失败，不得由客户端任意上报，也不能再进入 handed-off 状态。

V1 安装事务的合法转换如下：

| 当前状态 | 下一状态 | 触发条件 | 是否终态 |
|---|---|---|---:|
| `authorization_consumed` | `helper_update_started` | 凭据绑定 `helperUpdateRequired=true`，且在 `installStartNotAfter` 前开始助手字节安装或受保护所有权接管 | 否 |
| `authorization_consumed` | `installer_started` | 两个原始助手布尔值均为 false，且 transactionHelperId 等于当前活动助手并在 `installStartNotAfter` 前启动平台安装器 | 否 |
| `authorization_consumed` | `abandoned_before_install` | 超时、助手崩溃或 OS 拒绝启动，且确认安装器未启动；此前已完成提权 | 是 |
| `helper_update_started` | `helper_plan_succeeded`、`failed_manual_repair_required` | 按计划安装缺失助手字节（如需）、完成 transactionHelperId 的唯一事务接管（如需），或失败 | 后者是 |
| `helper_plan_succeeded` | `installer_started`、`abandoned_before_install` | 字节安装/所有权接管计划已逐项完成；在 mainInstallerStartNotAfter 前启动主安装器，或超时且确认未启动 | 后者是 |
| `installer_started` | `reconciling` | 安装器退出、助手恢复、用户切换或系统重启后开始核对实际状态 | 否 |
| `reconciling` | `installation_verified` | 实际版本、安装范围和签名身份均为目标值 | 否 |
| `reconciling` | `failed_manual_repair_required` | 已能确认未正确安装或状态无法在 30 分钟内确定 | 是 |
| `installation_verified` | `health_check_started` | 启动目标应用并建立挑战式健康观察 | 否 |
| `health_check_started` | `succeeded`、`failed_manual_repair_required` | 健康观察通过或失败/超时 | 是 |

用户取消只允许发生在安装授权消费前；`authorization_consumed` 或 `installer_started` 后关闭窗口仅隐藏进度，不得声明取消或终止 OS 安装器。`succeeded`、`failed_manual_repair_required` 和 `abandoned_before_install` 均为不可变终态；需要重试时创建新的安装事务。事件状态机版本写入凭据，后续扩展必须新增版本并保持服务端兼容窗口，不能原地改变既有迁移语义。

上表之外只有上一段定义的“上传窗口结束”服务端合成边合法；`overdue_reconciliation` 是派生运营标签而非客户端可上报的规范状态，不占用事件 sequence，也不阻止迟到事件。任何更早的定时任务不得直接写不可变终态。

事件凭据只能证明事件属于服务端签发的会话或事务并满足协议顺序，不构成硬件证明、用户身份认证、独立物理设备证明或客户端诚实性证明。服务端须按凭据 `jti`、installId、installationScopeId 和网络簇异常模式限流去重；客户端事件只能触发自动冻结扩量与告警，人工暂停复核还须参考安装授权消费记录、平台/版本分布和异常流量检测。安全撤回、密钥吊销等安全决策不得仅依据客户端自报事件自动执行。

`events_dropped` 是不推进上述规范状态机的聚合指标类型，不使用 `state`/事务 sequence。下一次合法 telemetrySessionCredential 可上报 `{metricId, metricType:"events_dropped", windowStart, windowEnd, droppedByClass:{progress:n,securityCritical:n}, reason, installId}`；凭据只绑定当前 installId，指标不得携带或冒充已丢弃事件的旧 decisionId/transactionId。服务端按 metricId 幂等、单独限流存储，不把计数转换为安装成功/失败或安全状态。客户端必须优先保证 securityCritical=0；若资源耗尽仍丢失关键事件，计数用于数据质量告警而非重建原事件。

制品选择、未选增量原因、重建资源/结果和人工修复入口展示使用另一类不推进状态机的 `client_observation`，不使用规范 `state` 或事务 sequence。Schema 固定为 `{observationId,observationType,occurredAt,decisionId,optionalTransactionId,packageId,code,measurements}`：observationType 仅允许 `artifact_selected|incremental_not_selected|delta_rebuild_result|manual_repair_entry_shown`；code 使用版本化枚举，measurements 仅允许字节数、毫秒数、算法 ID 和布尔结果，禁止自由路径、URL、SN 和业务内容。它必须使用当前合法会话/事务凭据并与其中 ID/包绑定，服务端按 `credential scope + observationId` 幂等、单独限流；观察值不得伪造状态边、授权消费或直接计入成功/失败率。

### 11.6 错误、重试与旧协议

下表是 V1 稳定错误码的**封闭注册表**；OpenAPI 必须逐项复用，未列代码不得返回给 V1 客户端，新增/改义须提升协议版本。`retryable=true` 只表示可按同一语义重试，不允许改变幂等键或绑定字段；“重查”表示终止本次决定后重新 `/check`。

| code | 端点 | HTTP | retryable | 重查/lineage | supportGuideCode |
|---|---|---:|---:|---|---|
| `invalid_request` | 全部 | 400 | 否 | 不改变 | 禁止 |
| `decision_invalid`、`token_type_invalid` | 需令牌端点 | 401 | 否 | 重查；不得修改旧 lineage | 禁止 |
| `credential_revoked` | 需令牌端点，以及 check 的已提交幂等结果包含 revoked kid 时 | 401 | 否 | 不返回原凭据、不修改旧 lineage；普通 check 使用新 nonce/key 建立独立会话 | 禁止 |
| `decision_expired`、`credential_expired` | 需令牌端点 | 401 | 否 | 无已提交幂等命中时重查；不得推进旧 lineage | 禁止 |
| `install_not_authorized` | consume/cancel | 403 | 否 | 重查；当前尝试终止 | 禁止 |
| `package_not_allowed` | download-info/validate | 403 | 否 | 重查；不得修改 lineage | 禁止 |
| `credential_replaced` | refresh/validate/events | 409 | 否 | 仅 refresh 的原已提交幂等键可恢复替代结果；validate/events 必须改用最新凭据或重查 | 禁止 |
| `idempotency_conflict` | check/refresh/validate/consume/cancel | 409 | 否 | 当前请求终止，不改变已提交结果 | 禁止 |
| `idempotency_in_progress` | check/refresh/validate/consume/cancel | 409 | 是 | 固定 Retry-After: 2；以完全相同请求重试，不执行第二次副作用 | 禁止 |
| `lineage_sequence_mismatch`、`lineage_state_conflict` | refresh/validate/events | 409 | 否 | 拒绝本次请求，不改变 lineage | 禁止 |
| `authorization_in_progress` | refresh/validate | 409 | 是 | 固定 Retry-After: 2；重试同一语义请求 | 禁止 |
| `authorization_expired`、`authorization_cancelled` | consume/cancel | 409 | 否 | lineage 已终态，重查 | 禁止 |
| `authorization_already_consumed` | cancel | 409 | 否 | 不改变安装事务；使用原 consume 幂等键查询 | 禁止 |
| `deployment_paused`、`deployment_inactive`、`rollout_paused`、`rollout_inactive`、`release_target_revoked`、`artifact_quarantined`、`artifact_changed`、`channel_revision_changed`、`selection_generation_changed` | download-info/validate/consume | 409 | 否 | 重查；安全撤回时清理未信任数据 | 禁止 |
| `invalid_event_transition` | events | 409 | 否 | 拒绝该事件，不反改本地事实 | 禁止 |
| `event_metric_conflict` | events | 422 | 否 | 拒绝该事件/观察，不推进状态 | 禁止 |
| `metadata_mismatch` | 元数据/检查链 | 422 | 否 | 停止升级并安全告警 | 禁止 |
| `unsupported_target` | check/validate | 422 | 否 | 当前目标终止 | 允许固定码 |
| `clock_invalid` | 元数据/check/validate | 422 | 否 | 修复时钟后重查 | 允许固定码 |
| `installed_version_mismatch`、`artifact_verification_failed`、`helper_identity_mismatch`、`launcher_identity_mismatch`、`backup_facts_mismatch` | validate | 422 | 否 | 首次 validate 写 authorization_failed；修复后新建事务并重查 | 后四项禁止；版本项允许固定码 |
| `updater_protocol_upgrade_required` | check | 426 | 否 | 当前协议终止 | 必须固定码 |
| `rate_limited` | 动态 API | 429 | 是 | 遵从 Retry-After，不改变 lineage | 禁止 |
| `service_unavailable` | 动态 API | 503 | 是 | 指数退避，不改变 lineage | 禁止 |

`updateAvailable=false` 的 `reason` 也是封闭枚举：`already_latest|current_version_ahead_of_channel|no_compatible_update|channel_has_no_baseline`；它们不是错误码，不能放进 error.code。成功的幂等结果查询始终为 200 并带新的 requestId。所有 error 的默认 `supportGuideCode` 为 null，只有表中“允许/必须固定码”的行可按客户端签名允许列表返回；不得在 details 中暗藏 URL 或新机器码。

以下条件映射是 OpenAPI/状态机的规范补充，优先于实现中的通用异常捕获。表格纵向顺序**不是**多条件命中时的优先级；所有实现必须使用以下固定判定管线，不能由异常抛出顺序决定结果：① 解析 JSON 并校验请求 Schema；② 对需令牌端点依次完成 JWS 结构/算法/kid/签名、固定 aud/tokenUse、令牌 Schema 与不可变身份绑定、kid revoked 校验，前一子步骤失败即停止；③ consume/cancel 在读取任何其他事务幂等详情前，校验 installAuthorization claim 与 request transactionId/Idempotency-Key、服务端授权记录 installId/scope 的绑定；④ 按各端点定义的作用域定位幂等键，存在记录时先比较 11.1 规范请求摘要，不同返回 `idempotency_conflict`；相同且为 committed 才恢复已提交结果，普通 check 的 committed 精确命中在返回前另检查结果内两个 kid 是否 revoked；相同且为 processing 时，租约未到期返回 `idempotency_in_progress`，租约恰到或已过期则尝试 ownerEpoch CAS，败者返回 in-progress，胜者进入同 operation key 的恢复流程；⑤ 仅在**不存在该端点对应幂等记录**时，按 `credential_replaced → credential_expired → decision_expired → lineage_sequence_mismatch → activeValidateOperation 特判 authorization_in_progress → 其他 lineage_state_conflict → 发布业务资格` 的适用子集顺序完成无副作用前置判断，consume/cancel 的授权到期也在本步骤、状态冲突之前处理；⑥ 需要创建/迁移状态时按 11.1 原子创建 processing reservation，唯一索引竞争失败回到步骤④，取得 ownerEpoch 后才允许领域 CAS 与 committed 原子提交；validate 还须按 11.4 独立提交与该 processing owner 绑定的 activeValidateOperation，再以最终事务提交授权结果和幂等结果并清除 marker。events 不使用这组请求幂等键，只在步骤②之后按 eventId/sequence 规则处理。故 refresh 的同 key 改 nonce/sequence、validate 的同 transactionId 改 sequence 均由步骤④唯一返回 idempotency_conflict，即使旧凭据已 replaced 或当前 sequence 已变化；同 key/摘要并发且租约有效时返回 idempotency_in_progress，租约过期时只允许 CAS 胜者恢复；consume/cancel 的 claim 绑定错误则由步骤③唯一返回 install_not_authorized，即使同 key 摘要也变化。kid revoked 在所有需令牌端点始终于步骤②拒绝，不能恢复旧结果。

上一步“发布业务资格”内部也使用封闭全序，所有适用端点及后台恢复从同一个一致性快照按第一命中停止：① Artifact quarantined/即时制品 deny → `artifact_quarantined`；② Release Target revoked/即时目标 deny → `release_target_revoked`；③ Deployment withdrawn 或 superseded → `deployment_inactive`；④ Deployment paused → `deployment_paused`；⑤ 决定绑定的 Manifest/制品身份与当前发布集合不等 → `artifact_changed`；⑥ channelRevision 不等 → `channel_revision_changed`；⑦ sourceProfile/edge 不再有资格、sourceFactsRegistryVersion/Registry 指针或 selectionGeneration 不等 → `selection_generation_changed`；⑧ 候选 Rollout aborted/completed/不再属于该 generation → `rollout_inactive`；⑨ 候选 Rollout paused → `rollout_paused`。download-info、首次 validate、consume、check/refresh 的最终重验以及 marker 后 coordination_abort 都必须使用该全序；同一快照多条件命中只能返回最靠前的 code，coordination_abort tombstone 也保存该唯一 code。baseline-prerequisite 的唯一兼容例外只在到达 Rollout 两项时判断，不能越过前七项。条件表的书写顺序不覆盖本全序。

| 条件 | 端点 | HTTP/code | lineage/幂等结果 |
|---|---|---|---|
| JSON/Schema/字段非法 | 任一动态 API | 400 `invalid_request` | 不创建或修改 lineage/幂等记录 |
| 签名错误、固定 token 类型错误、decision/telemetry 的不可变 ID 跨绑、kid revoked | 任一需令牌端点 | 401 `decision_invalid\|token_type_invalid\|credential_revoked` | 不读取业务详情、不修改 lineage；revoked 优先于已有幂等结果。installAuthorization 正文绑定冲突按下方专行 |
| consume/cancel 的 installAuthorization transactionId/Idempotency-Key 与 claim 不等，或 claim 中 installId/scope 与服务端授权记录不等 | consume/cancel | 403 `install_not_authorized` | 步骤③终止；不查询其他事务的幂等详情，不修改 lineage |
| 完成需令牌端点步骤②及 consume/cancel 步骤③后，幂等键已存在但规范请求摘要不同 | check/refresh/validate/consume/cancel | 409 `idempotency_conflict` | 步骤④终止；不修改原结果 |
| 幂等键和规范请求摘要相同，记录为 processing 且租约仍有效，或过期租约 CAS 接管失败 | check/refresh/validate/consume/cancel | 409 `idempotency_in_progress` | Retry-After: 2；不重复执行；trustedNow>=leaseExpiresAt 时 CAS 胜者进入恢复而不返回本行 |
| 普通 check 的 committed 精确幂等记录命中，但原结果任一 decision/telemetry kid 已 revoked | check | 401 `credential_revoked` | 不返回原结果、不创建第二 session；客户端改用新 nonce/key 建立独立检查 |
| 不存在本端点幂等记录，且 telemetry credential 已被等价刷新替代 | refresh/validate/events | 409 `credential_replaced` | 不修改 lineage；refresh 可用原幂等键查替代结果，validate/events 改用最新凭据 |
| 不存在本端点幂等记录，且 decision/credential 到期 | download-info/validate/events/refresh | 401 `decision_expired\|credential_expired` | 不修改 lineage；refresh 仅按 11.2.1 的 exp+24h 独立关闭例外处理 |
| 不存在本端点幂等记录，且 refresh/validate 的 `lastAcknowledgedSequence` 不等于服务端最后确认值 | refresh/validate | 409 `lineage_sequence_mismatch` | 不修改 lineage |
| events 的 sequence 小于等于最后接受值且不是同 eventId/sequence/payload 的精确重放 | events | 409 `lineage_sequence_mismatch` | 不修改 lineage；严格增大的空洞允许，但仍须通过状态迁移 |
| 不存在本端点幂等记录，且同一 lineage 存在活动 `activeValidateOperation` | refresh/validate | 409 `authorization_in_progress` | 派生 requested，优先于通用状态冲突；不修改 marker/lineage，按 Retry-After 重试原请求 |
| 不存在本端点幂等记录，且 lineage 不处于端点要求状态或已终态 | refresh/validate/events | 409 `lineage_state_conflict` | 不修改 lineage |
| packageId 不在 decision 允许集合 | download-info/validate | 403 `package_not_allowed` | 不修改 lineage |
| Deployment paused | download-info/validate/consume | 409 `deployment_paused` | 不签地址/授权、不消费，客户端重查 |
| Deployment withdrawn/superseded | download-info/validate/consume | 409 `deployment_inactive` | 同上；不可恢复原投放 |
| Release Target revoked | download-info/validate/consume | 409 `release_target_revoked` | 同上并清理未信任数据 |
| Artifact quarantined | download-info/validate/consume | 409 `artifact_quarantined` | 同上并清理未信任数据 |
| Manifest/制品身份相对决定变化但未进入隔离 | download-info/validate/consume | 409 `artifact_changed` | 不继续旧决定，客户端重查 |
| 候选 Rollout paused | download-info/validate/consume | 409 `rollout_paused` | 不继续候选；仅 8.1 的 baseline-prerequisite 例外可继续基线 |
| 候选 Rollout aborted/completed 或已不再是该 generation 候选 | download-info/validate/consume | 409 `rollout_inactive` | 不继续旧候选，客户端重查 |
| channelRevision/selectionGeneration 变化 | validate/consume | 409 对应 `channel_revision_changed\|selection_generation_changed` | 不授权/消费；重查 |
| activeValidateOperation 创建后，kid/到期/发布资格/安全状态在最终提交前失效 | validate/后台恢复 | 对应既有 `credential_revoked\|credential_expired\|decision_expired\|deployment_paused\|deployment_inactive\|release_target_revoked\|artifact_quarantined\|artifact_changed\|rollout_paused\|rollout_inactive\|channel_revision_changed\|selection_generation_changed` | coordination_abort：提交相同外层 HTTP/code 的幂等 tombstone、清除 marker、lineage→`authorization_coordination_aborted` 并关闭可写凭据索引；不写 authorization_failed |
| 包容器或最终安装器大小/哈希/平台真实性不符 | 首次 validate | 422 `artifact_verification_failed` 且 `valid=false` | authorizationReadyState→install_authorization_requested→authorization_failed，并保存 24h 幂等失败 |
| helper 或 launcher 版本、协议、哈希、身份、路径不符 | 首次 validate | 422 `helper_identity_mismatch\|launcher_identity_mismatch` 且 `valid=false` | 同上 |
| 实际 canonicalAppVersion 缺失、超范围或不等于决定来源 | 首次 validate | 422 `installed_version_mismatch` 且 `valid=false` | 同上 |
| backupFacts 缺字段，注册表版本/哈希、source version、revision、字节、快照或时间不符 | 首次 validate | 422 `backup_facts_mismatch` 且 `valid=false` | 同上；不得安装 |
| 不同 transactionId 的 validate 与首个 validate 并发，且首个 activeValidateOperation 仍在处理或等待接管 | validate | 409 `authorization_in_progress` | 不命中另一事务幂等记录、不改变 marker/lineage；稍后查询首个事务或重新检查 |
| 不同 transactionId 的 validate 到达时 lineage 已 authorized/authorization_failed/authorization_coordination_aborted | validate | 409 `lineage_state_conflict` | 不创建第二授权，也不返回 `idempotency_conflict` |
| lineage 已 install_authorized 且无相同 validate 幂等结果 | validate | 409 `lineage_state_conflict` | 不创建第二授权 |
| 首次 consume/cancel 到达时授权已过期 | consume/cancel | 409 `authorization_expired` | install_authorized→authorization_expired；已有相同提交结果查询优先 |

validate 只有在令牌密码学/Schema/绑定/新鲜度、幂等冲突、sequence 和发布对应 authorizationReadyState 检查均通过后，才可创建 activeValidateOperation 并派生 `install_authorization_requested`；表中四类首次本地/业务复核失败随后在最终事务写 `authorization_failed`、valid=false、committed 幂等结果并清除 marker。所有更早的前置失败使用外层 error 结构且不写 authorization_failed。download-info/consume/cancel/events 从不返回 valid 字段。

尚未迁移的存量客户端继续访问原有升级系统，并由原有方式获取迁移引导版本；新平台不提供旧请求协议兼容端点。已经切换到新平台但 Updater 协议版本过低时返回 426 和固定站点上的人工升级说明。旧服务正式停止后，其旧端点返回 410 并提供人工升级入口。不得为了兼容旧客户端降低签名、平台匹配或版本兼容要求；SN 不属于这些安全要求。

“受信人工升级/修复指引”不从错误响应接受任意 URL。客户端只接受稳定机器码 `supportGuideCode` 和可选 locale，并将其映射到随应用签名发布的固定 `https://www.pinvou.com/support/update/<code>` 路径或本地多语资源；响应中的 URL、主机名、Markdown 或重定向都不得改变入口。未知 code 回退到通用本地指引。

## 12. 客户端流程、缓存与安装结果确认

### 12.1 持久状态机

```text
空闲 → 检查 → 发现更新 → 用户/策略确认 → 选择制品
     → 下载 → 校验 → [增量重建] → 本地预检
     → 获取系统权限 → [最终数据备份并持续写冻结] → 助手复核实际状态
     → validate → consume → 2 分钟内开始[助手更新与接管]或主安装
     → 最迟 5 分钟内启动主平台安装器
     → 外部健康观察 → 成功提交 → 清理

安装授权消费前 → 用户取消 / 可重试失败 / 不可重试失败
安装授权消费后 → 不允许取消；仅可隐藏进度并由外部助手确认终态
增量失败 → 完整包兜底
安装或健康失败 → 失败终态 → 人工修复
```

`installId` 仍是用户级遥测标识；系统级安装的并发、安全和事务归属只使用安装器生成的 `installationScopeId`。状态、事务 ID、制品身份、安装授权消费状态和安装日志必须保存在所有本机用户共享但仅受控助手/管理员可写的系统级受保护位置（Windows 使用 ProgramData 与全局命名互斥体、Linux 使用 `/var/lib` 与系统锁、macOS 使用 `/Library/Application Support` 与特权助手锁的等价机制），不得放在单个用户应用数据目录作为唯一事实来源。事务状态和凭据文件使用原子替换、严格 ACL/权限并校验 Schema；凭据不得进入普通日志，终态补报完成或安装事务 37 天上传窗口结束后立即删除。应用崩溃、用户切换或系统重启后由外部升级助手按 installationScopeId 核对未完成事务、安装器退出状态和实际安装版本，不能永久停留在“更新中”。首期不下载、缓存或执行恢复包，也不自动恢复升级前版本；无法确认成功的事务进入 `failed_manual_repair_required`。OS 全局产品锁保证同一 installationScopeId 只运行一个升级事务，所有用户会话、窗口和进程共用该锁；锁必须具备进程存活检测和租约，不能因进程崩溃永久阻塞升级。

全新 per-machine 安装与 10.6 的迁移安装共享同一版本化初始化器，但来源标记不同。全新安装器必须在应用首次可运行前，以系统权限取得产品级全局初始化锁，幂等生成唯一随机 `installationScopeId`，并在一个原子日志事务中写入 `channel=stable`、`channelRevision=1`、canonicalAppVersion、安装范围/规范路径、平台真实性、内置初始 Root 及其高水位、稳定 launcher/helper 的完整事实、protectedUpdaterFactsSchemaVersion=1/protectedUpdaterFactsRevision=1、helperOwnershipFencingToken 初值、scope 注册表版本、受保护记录 Schema 和 `upgradeProtocol=new-v1,migrationSource=fresh-install`。所有用户会话只读同一记录；重复运行、修复安装、多用户并发或断电恢复必须复用已提交 ID，未完成事务只能回放或清理后重新执行且不得留下两个有效记录。只有 Root、launcher/helper、权限和安装事实自检全部通过才提交初始化并允许首次 `/check`；提交后缺失或篡改遵守人工修复规则，不得静默补生成。初始 Root 只能随签名安装器交付，不能在首次启动时从网络建立信任。

`installationScopeId` 不信任普通应用进程上报的任意字符串。检查前由受控助手从系统安装记录读取，经认证本地 IPC 交给客户端；validate 和 consume 前助手再次读取并与决策/授权绑定比对。OS 全局锁的键只能使用该受保护记录中的值，不能使用请求体构造的值。除 10.6 明确定义的迁移引导安装器在 `transitioning` 阶段首次初始化外，已存在安装尤其是已提交 `new-v1` 的安装，其记录缺失、Schema/权限异常或值被替换时必须停止自动升级并进入人工修复，不得临时生成新 ID 绕过锁。

### 12.2 下载和校验

在应用数据目录使用 `.partial` 文件，支持 Range、取消、指数退避和校验后原子重命名。首请求、HEAD、206 与所有续传请求都必须发送 `Accept-Encoding: identity`，并只接受无 Content-Encoding 或明确 `identity`；gzip/br 等任何传输内容编码一律拒绝，避免字节偏移与 Manifest 大小语义变化。续传必须携带 `Range` 及绑定同一不可变对象的 `If-Range`（强 ETag）或等价对象版本条件；收到 206 时逐项校验 `Content-Range` 的起止位置和总大小与已验证 Manifest 一致后才可追加。服务器返回 200、412、验证器变化、区间错位、总大小变化或非 identity 编码时必须丢弃 partial 并从 0 重新下载（编码不合规时改用其他合规文件源），绝不把完整响应追加到旧文件；416 仅在本地长度恰等于声明总大小且最终 SHA-256 通过时可视为下载完成，否则从头开始。下载前按“下载文件 + 解压/重建 staging + 可选数据备份 + 20% 安全余量”检查空间。校验失败删除不可信文件；真实性失败不可自动重试同一内容。

### 12.3 安装前保护

检查未保存内容、运行任务、Engine/连接器子进程、包管理器锁、权限、发布状态和数据迁移。普通更新可延期；到期更新仍必须允许保存、导出和正常退出。升级助手必须独立于待替换的主应用，并按 7.3 独立 `helperAuthenticity` 校验自身身份、哈希与受保护安装位置；助手与主程序可属于同一逻辑发布，但不得假设两者 Bundle ID、包身份或签名字段相同。主应用和助手之间使用仅当前用户/受控系统身份可访问的本地 IPC，所有路径使用已经打开并校验的文件句柄或受控目录中的规范化路径，禁止把下载内容拼接为命令行或 Shell。提权进程仅执行已签名事务中列出的安装动作，不负责网络下载、解析任意脚本或接受其他本地用户发来的安装请求。升级日志使用原子追加、限制大小并脱敏。安装确认界面必须明确提示：首期不提供失败自动回滚，安装失败可能需要人工重新安装或修复。

“人工修复入口”只展示脱敏诊断、导出日志和打开固定 HTTPS 官方支持/下载页面，不签发降级或同版本自动安装授权，也不由升级助手自动执行修复命令。支持人员可指导用户交互式重新运行官方签名安装器，但该人工操作属于产品安装/支持流程；若未来要由升级平台自动执行同版本 repair，必须另行定义授权目的、状态机和验收，不能复用 V1 向上升级令牌。

安装授权是最后一步的短期安装权，不是长耗时预检。未保存内容、任务、磁盘、包管理器锁、用户确认及 UAC/macOS/管理员授权必须先完成；权限拒绝或用户取消属于消费前的 `permission_denied`/`user_cancelled`，`authorization_failed` 仅用于后续服务端 validate 拒绝。`backupPolicy=none` 在 `permission_granted` 后即可进入最后复核；`backupPolicy=required` 则必须在 permission_granted 后由受控助手按 10.7 的 quiesceProfile 取得全部 scope 强制写冻结，使用该冻结下的最终 revision 生成并验证新快照，成功后保持同一锁、原数据源句柄和 quiesceFencingToken，不得在权限获取前生成一个快照再靠事后 rehash 补强。决定过期或不足 2 分钟时，助手应在开始长备份前先刷新一次；备份完成后仍不新鲜则在持续冻结下再次等价刷新，刷新不能延长 10 分钟 postBackupQuiesceDeadline。随后助手重新读取实际版本、安装范围、渠道/OS、制品与助手身份和发布状态，并连续执行“上传确认 authorizationReadyState → validate → 本地冻结复核 → consume → 安装器接管”。consume 前若冻结、权限、boot/process 身份、句柄或 quiesceFencingToken 失效，或到达 postBackupQuiesceDeadline，禁止 consume：validate 尚未 requested 时上报 `backup_invalidated`；validate 已提交时先以原 transactionId 恢复结果，并在授权成功时 cancel，响应无法恢复时最多等待原 5 分钟授权自然到期，再删除快照/keyRef、释放锁并重新走权限与备份。用户在 backup_succeeded 后取消也必须先删除快照/keyRef并释放锁。若需要更早的服务端可行性查询，只能使用不签发安装权的 preflight，不能提前创建 installAuthorization。

### 12.4 Windows 安装与结果确认合约

- 首期仅支持 per-machine NSIS，并使用 Authenticode 签名的完整 EXE。
- 上线前必须确认全部受支持 Windows 来源版本的实际安装范围均可迁移到 per-machine；检测到 per-user、安装目录/Publisher 不匹配或未知安装范围时不得覆盖安装，转入经过审核的人工迁移指引。
- 实际版本以受信安装目录内通过 Authenticode 身份校验的主可执行文件版本与安装注册信息共同判定；两者不一致、路径可由普通用户替换或签名身份不符时返回 `installed_version_mismatch`，不得只读取用户可写注册表或配置文件。
- 外部升级助手在主应用退出前启动并写入事务日志，只缓存目标安装器和发布声明要求的可选数据备份。
- NSIS 必须提供稳定的静默/交互参数、退出码、安装路径验证和版本探测；安装器自身可提供供支持流程使用的同版本交互式 repair 模式，但升级助手首期不自动调用。UAC 取消映射为明确结果。
- 目标安装失败、进程意外退出或 120 秒外部健康观察未通过时，升级助手记录实际版本、安装器退出码和诊断信息，进入人工修复状态，不启动上一版本安装器。
- 断电、强制结束主应用/安装器/升级助手后，下次登录或启动时从事务日志核对结果；不得在运行进程内覆盖 Updater 自身，也不得把未知状态自动判定为成功。

### 12.5 Linux DEB 安装与结果确认合约

- 仅支持 Ubuntu 22.04/24.04 LTS Desktop 的已声明架构，通过无网络的 `dpkg` 事务和系统授权安装；V1 安装阶段禁止 `apt` 从仓库下载依赖。
- 实际版本和架构以 `dpkg-query` 的已安装包数据库为主，并与受信可执行文件/DEB 元数据映射核对；数据库处于半配置状态时先进入事务核对，不得仅用应用自报版本申请授权。
- 目标 DEB 必须对受支持的基础系统自包含，或其声明依赖已由该系统默认预装；构建闸门与真实机矩阵必须在无网络环境证明依赖闭包。安装前缓存目标 DEB、使用 `dpkg --audit`/等价机制完成依赖与半配置预检，并完成发布声明要求的可选数据备份；依赖不满足时在 validate 前失败，不得临时联网补齐。包管理器锁默认等待 120 秒后可重试失败。
- 升级助手记录 `dpkg` 事务阶段。安装失败时可以执行不改变目标版本方向的包管理器一致性修复，例如完成未配置事务，但不得安装上一版本 DEB。
- DEB 维护脚本的哈希已在 Release/Package Manifest 中绑定，还必须通过静态策略检查与无网络隔离安装测试，不得执行下载器或动态远程脚本。必须在真实系统测试安装脚本失败、依赖失败、包管理器锁、断电和半配置状态；无法完成目标版本安装时保留日志并给出经过审核的人工修复命令。
- 非 Ubuntu、无桌面环境、容器或不匹配发行版不得执行这些命令。

### 12.6 macOS 安装与结果确认合约

- DMG 内 App 必须具备 Developer ID 签名及公证，Team ID、Bundle ID 和版本与 Release Manifest 一致。
- 实际版本从预期安装位置的已签名 App Bundle 读取，并同时核对 Team ID、Bundle ID、`CFBundleShortVersionString` 和可执行文件身份；用户可写副本、其他安装位置或仅修改 Info.plist 的版本不得作为授权来源。
- 外部升级助手只读挂载 DMG，在同一文件系统 staging，验证后执行原子替换；切换成功后不把上一 App Bundle 作为自动恢复资源保留。
- 权限不足时使用系统授权交互；不得移除 quarantine、修改签名或绕过 Gatekeeper。
- 新 App 在 120 秒观察期内未通过健康检查时，升级助手记录失败并提供人工重新安装入口，不自动恢复上一 Bundle。
- Intel 与 Apple Silicon 均须验证原子切换、崩溃、断电和失败状态确认；ad-hoc 签名不得进入 stable。

### 12.7 外部健康观察

健康判断由不随主应用同时退出的升级助手监督，而不是只由新版本自报。升级助手启动目标版本时生成至少 128 bit 随机挑战值，并将其绑定到事务 ID、预期可执行文件身份和本次启动的进程身份；本地 IPC 端点只允许该用户及预期进程访问，挑战值一次性使用且不得写入日志。新版本需在 120 秒内回报挑战响应，并通过：主进程连续存活至少 30 秒、实际版本与签名身份正确、配置/迁移成功、Engine 和必要后台进程已启动、基础 Tauri IPC 可用。旧版本进程、同机其他用户或仅知道事务 ID 的进程不能代为确认。120 秒总超时和 30 秒连续存活窗口属于 V1 合约；变更须提升健康检查协议版本，不能由单个 Release 任意放宽。成功后升级助手才提交事务并上报；超时、崩溃或版本不符均进入 `failed_manual_repair_required`。

首期不自动恢复应用二进制或数据。若 `dataMigration.mode=backupRequired`，备份仅供人工修复使用；若为 `irreversible`，界面必须预先说明升级失败后无法自动回退程序或数据，且不得强制或静默安装。

V1 Release Manifest 的 `restartRequirement` 固定为 `application`。任何平台安装器返回“需要系统重启”、实际文件处于待重启替换状态或无法在当前启动周期确认目标版本时，不得进入 `succeeded`，而是进入 `failed_manual_repair_required` 并提示人工完成系统重启/修复；V1 不定义 `pending_system_restart`，后续支持必须提升协议和状态机版本。

### 12.8 升级助手自更新与事务接管

升级助手不得作为主应用目录中会被安装器原地覆盖的单一可执行文件。三平台都必须使用受保护的稳定 bootstrap/launcher 与按版本并存的助手目录：Windows 使用受 ACL 保护的 Program Files/ProgramData 组件及系统级 launcher；Linux 使用 root-owned `/usr/lib/pinvou/updater/<version>`（或等价发行目录）和稳定 launcher；macOS 使用 App Bundle 外受保护的 `/Library/Application Support/Pinvou/Updater/<version>` 与已注册特权助手。稳定入口不是 helperArtifact，V1 只能由通过完整校验的主平台安装器在同一 OS 安装事务中更新；目标 `postInstallLauncher` 的版本、协议、大小、哈希、独立真实性和 installTargetId 必须在 Package/Ota/Release 三层一致并进入安装授权。安装后由仍可信的入口或 OS 安装机制先核对新入口并原子提升 launcher 高水位，下次启动再次验证；不能由待替换主应用直接覆盖，也不能接受比高水位低的 launcher。断电恢复只能选择事务日志绑定且符合 postInstallLauncher 的旧/新入口，二者都无法验证时进入人工修复。

本节的助手所有权令牌统一命名为 `helperOwnershipFencingToken`，只用于决定哪个助手可以写事务或启动安装器；它与 10.7 数据写冻结使用的 `quiesceFencingToken` 相互独立。V1 通过 10.5 的边约束保证两种接管不会在同一事务中发生：`backupPolicy=required` 必须同时满足 `helperBytesInstallRequired=false` 与 `helperHandoffRequired=false`，发生助手字节安装或所有权接管的事务必为 `backupPolicy=none`。未来若要在同一事务内同时换助手和保持数据冻结，必须提升安装协议与状态机版本并另行定义由稳定锁代理持有数据句柄的无空窗转移协议，不能直接放宽本约束。

当目标发布包含助手时，普通权限阶段只能把需要的助手解压到不可执行 staging，并由当前活动助手验证其大小、SHA-256、三层 Manifest 完全一致的 `helperAuthenticity` 及对应平台真实性；Windows 校验 Authenticode Publisher/SPKI/时间戳，macOS 校验助手独立 Team ID、identifier、designated requirement 与公证，Linux 校验签名元数据链/哈希/ELF 类型架构并预先核对目标路径和 root-owned 权限模板。此时不得写入受保护助手目录或启动目标助手。validate/consume 把精确 transactionHelperId、helperArtifactsSha256 和两个原始执行布尔值纳入一次性授权后，当前活动助手在 `installStartNotAfter` 前执行计划：`helperBytesInstallRequired=true` 时才把缺失的已验证条目写入受保护版本目录，false 时不得重复写入；`helperHandoffRequired=true` 时必须通过认证本地 IPC 与受保护事务文件把事务所有权切换到 transactionHelperId 对应的目标事实，false 时不得切换且 activeHelperRef 唯一命中的事实必须已等于目标执行事实。即使目标字节已安装但未激活，也必须执行后一接管步骤。稳定 launcher 在整个事务期间持有 OS 全局互斥体，互斥体所有权不跨进程转移；launcher 为每次助手所有权授予严格递增的 helperOwnershipFencingToken，所有事务状态写入、受保护目录变更和安装器启动都必须携带并由 launcher 校验当前 token。当前助手创建绑定 transactionId、transactionHelperId、目标七字段事实、随机挑战、helperOwnershipFencingToken 和有效期的一次性 handoff；目标助手证明自己是授权列出的预期已验证进程，launcher 在同一受保护事务中原子撤销旧 token、签发更高 token、把 activeHelperRef 精确更新为目标事实的 helperId/version/installTargetId/sha256、按实际安装结果更新 currentHelperFacts 并严格递增 protectedUpdaterFactsRevision、记录新所有者后，目标助手才可回报 ready。持旧 token 的复活进程所有写入均拒绝。旧助手确认后退出，并由 activeHelperRef 对应的当前 token 所有者在 `mainInstallerStartNotAfter` 前启动主应用安装器。主安装器若更新 launcher，必须在健康提交前以原子日志把 currentLauncherFact 更新为 Manifest 目标并再次严格递增 protectedUpdaterFactsRevision；任何一步失败均不得留下“文件已换但受保护事实未换”或相反状态。助手协议只保证兼容当前和前一个协议版本；不能协商说明发布闸门或现场事实异常，必须在 consume 前失败并进入重新检查或人工修复，V1 不存在可绕过 Release 的另行助手引导授权。

所有受保护助手字节的增加、替换或删除都必须通过稳定 launcher 的防崩溃事实事务，不能只改文件系统。launcher 先持久化绑定 transactionId、授权 helperArtifactsSha256、操作集合和前后事实投影的 intent，再写入/核验版本目录，最后以原子提交记录更新 currentHelperFacts 并严格递增 protectedUpdaterFactsRevision；只有提交后这些字节才算已安装并可进入 helper_plan_succeeded。`helperBytesInstallRequired=true && helperHandoffRequired=false` 时必须把全部新增目标条目加入 currentHelperFacts、保持 activeHelperRef 和当前 helperOwnershipFencingToken 不变并递增 revision；handoff=true 时，字节事实、activeHelperRef 和新 fencing token 必须在同一 launcher 事实事务提交，不得先暴露一半。断电恢复只能完成已验证 intent 或删除未登记字节，绝不能留下文件存在但事实缺失、事实存在但文件缺失或 revision 未递增。

接管 ready 前的崩溃不算应用回滚：launcher 撤销未完成 handoff，旧助手保留可执行性并中止本次安装；ready 后旧助手不得再次处理该事务。断电或双进程恢复时，稳定 launcher 保持/重建互斥所有权，只选择受签名事务记录引用且通过验证的最高助手版本，并签发高于持久化高水位的新 helperOwnershipFencingToken；状态无法唯一确认时不得启动安装器，进入人工修复。主安装器不得删除当前事务使用的助手版本，直到事务终态和事件持久化完成；旧助手清理只能执行 11.2 由普通 check 返回且已验签、未过期、beforeProfileId/删除集合/afterProfileId 精确匹配的 helperCleanupAuthorization，离线、自行推断或仅凭“未被当前事务引用”均不得删除。每次清理同样必须通过 launcher intent，在一个事实事务中删除字节、从 currentHelperFacts 删除对应项并递增 protectedUpdaterFactsRevision；断电恢复完成授权指定的删除或恢复字节与旧事实，不能产生记录/文件漂移，完成后废弃原决定并重新检查。该机制不保存或恢复旧版品悟应用，也不是恢复包。

## 13. 客户端界面

- 更新提示展示目标版本、三个策略维度的用户含义、截止时间、下载大小、说明、预计时间和是否需要数据备份。
- 渠道选择器始终公开展示 internal、beta、stable 的稳定性说明；首次进入预览渠道要求显式确认，退出时若不能立即切回较低 stable 版本，应明确提示“保持当前版本，等待更高 stable 版本”，不能暗示会降级。
- `selectionMode=compatibility_bridge|candidate_prerequisite` 时明确显示“当前版本需要分步升级，本次将先升级到 X”，不得把中间版本描述为最终最新版本或保证下一次必然获得灰度候选；该跳成功后重新检查并按当时发布/灰度规则处理下一跳。
- 设置页展示当前版本/渠道、手动检查、后台下载开关、下载/校验/安装/健康确认状态及人工修复入口。
- 明确区分检查失败、SN 灰度信息不可用（仅提示已按非定向分组处理）、下载失败、校验失败、权限拒绝、安装器启动失败、安装进行中、等待健康确认、安装失败和需人工修复。
- 不得把“文件下载完成”“安装器已启动”或“主应用重新出现”直接显示为成功。
- 所有文案复用品悟 i18n，并提供简体中文、英文、日文；缺少翻译在发布闸门失败，不依赖运行时跨语言兜底掩盖。

## 14. 新平台接入

```text
PlatformUpdateAdapter
├── target()
├── capabilities()
├── optional_hardware_sn()
├── validate_asset()
├── prepare_data_backup()
├── stage()
├── request_permission()
├── install()
├── verify_installation()
└── cleanup()
```

新平台必须：注册目标及已认证系统版本范围；在平台可提供且用户未关闭发送时实现硬件 SN 的稳定读取与规范化，读取不支持或失败时按“无 SN 灰度输入”继续；实现签名验证、权限、安装、实际版本探测和外部健康观察；接入构建/签名流水线；通过适配器合约、真实系统故障注入和失败状态确认测试；经后台审核后启用。`prepare_data_backup()` 在无需备份时返回 `not_required`，不支持必需备份时返回 `unsupported` 并阻止相应发布安装；任何不可用能力都不得静默套用其他平台实现。

在品悟代码中，业务逻辑位于 `features/updater/`，平台实现位于其 `platform/` 子目录；跨 feature 的 OS 原语才进入全局 `platform/`。React 只消费语义能力，不检查 User-Agent 或 Tauri 全局变量。

## 15. 存储、隐私和留存

### 15.1 文件服务

原始复合包、Package Manifest、完整包和增量包均以内容哈希不可变存储。发布前至少复制到两个故障域并校验哈希。切换第三方文件服务只能改变短期 URL，不能改变 packageId、大小、SHA-256 或签名。后台记录服务标识、健康状态、切换和失效操作。BridgeEligibility 为 enabled 或仍被可达性矩阵引用的历史发布及其目标制品必须维持在线可用，不能因 superseded 状态被生命周期任务删除。

### 15.2 灰度与实例标识

- 硬件 SN 是可选灰度分组数据，不是认证标识或安全授权边界；仅在 TLS 检查请求内传给控制面，不进入文件信息、安装复核、事件或文件服务请求。服务端规范化后使用独立 KMS 密钥计算 HMAC，灰度分桶和包含/排除组只保存 HMAC 值；原始 SN 在请求处理完成后丢弃，不进入访问日志或普通事件。
- 批量灰度包含/排除组导入在受控服务中立即规范化和 HMAC 化，原文件在 24 小时内删除。确需明文排障必须单独审批、加密、脱敏且有访问审计。此类分组不得复用为封禁名单、许可证名单或安全访问控制表。
- `installId` 为首次启动生成并存于应用数据目录的 UUIDv4，更新时保持，卸载清数据或用户执行“重置设备标识”后重建。它是伪匿名持久标识，不得宣传为匿名。这里“活跃 lineage/事务”只指本机受保护存储持有尚未过期的服务端凭据，或持有尚未达到协议终态的本地/服务端已确认事务；客户端从未收到响应、因而没有凭据或本地决定记录的服务端孤儿检查会话，不得永久阻止本机重置。存在上述任一活跃记录时，界面重置操作必须拒绝并说明待事务结束；若用户从系统外部清除应用数据导致新 ID 生成，系统级受保护事务仍保存创建时的旧 installId，受控助手必须继续用该快照和原凭据补报至终态，新 installId 只用于之后新建的检查，二者不得互换或改绑。
- 普通 `/check` 已提交但响应丢失时，客户端先以原 requestNonce/Idempotency-Key 和相同规范语义对象恢复原响应；记录已过 24 小时或客户端也丢失本地请求快照时，才可用新 nonce 发起新检查。服务端不得把无法由客户端证明持有的旧 session 视为设备仍在升级。若旧 session 自 `decisionToken.iat` 起 24 小时内没有任何凭据认证的事件、refresh、download-info 或 validate，服务端将其标记为只读派生状态 `orphaned_check_expired`，关闭后续状态推进，并在原 telemetry 凭据 eventNotAfter 到达时删除可写凭据索引。若会话曾有活动但始终未进入 `install_authorization_requested|install_authorized|handed_off_to_install_transaction`，则以该 lineage 最后签发的 telemetrySessionCredential 为准，在其 `exp+24h` 关闭窗口结束仍非终态时标记只读派生状态 `session_stale_expired`，删除可写/幂等关闭索引并禁止后续推进；已进入授权/安装的会话改由授权到期和安装事务收敛规则处理。`install_authorization_requested` 必须由 marker 恢复规则收敛；收敛为 `authorization_coordination_aborted` 后已经是持久终态并已关闭可写凭据索引，不再进入 stale 判定，也不依赖客户端补发事件。两个派生状态都不是客户端事件、协议终态、失败安装或质量阈值样本，不得伪造 user_cancelled/失败结果；审计/聚合记录仍按 15.3 留存。
- `installationScopeId` 存于系统级受保护安装元数据，per-machine 安装下由所有用户会话共享，只用于产品安装互斥、事务恢复和授权绑定；不得展示为用户身份、用于广告画像或替代 SN 分组。普通用户重置 installId 不改变 installationScopeId，只有完整卸载清理系统安装元数据后才重建。
- 用户可在隐私设置中查看标识用途、重置 installId，并关闭硬件 SN 灰度数据发送；关闭后请求省略 `hardwareSn`，服务端按 SN 缺失规则处理，不能因此降低基线、桥接版本或 100% Rollout 的可用性。界面必须明确说明硬件 SN 只用于灰度分组。客户端不得宣称 SN 可证明设备身份，无法读取时继续检查并回退到非 SN 定向的基线、桥接版本或 100% Rollout。

### 15.3 留存和访问

升级事件保留 180 天，SN 灰度 HMAC 与最后一次分组使用关系在活跃期及其后 90 天保留，审计和审批记录保留 2 年；到期自动删除或聚合匿名化。安全事件可依法冻结留存并记录原因。SN 灰度映射仅升级服务受限角色可访问，运维看板默认只展示聚合数据。

`installationScopeId` 在服务端作为伪匿名事务关联键，明细访问仅限升级服务、受控故障处置和审计角色，不进入面向业务/运营的逐设备报表或通用导出；导出默认聚合并进行最小样本抑制。完整卸载时客户端删除本机受保护记录；服务端无法信任客户端卸载声明，因此在最后一次合法事件后 180 天删除该 ID 与决定/事务的在线关联，仅按不可逆聚合保留指标，法定冻结除外。隐私说明须披露该期限、用途和访问角色。

五端点 operationRecoveryProjection 与 activeValidateOperation.recoveryProjection 仅供幂等恢复及 coordination_abort 使用，统一使用服务端授权恢复专用密钥 AEAD 加密，访问限当前 owner、升级服务恢复 worker 和安全审计角色，不进入日志、报表或导出；marker 副本与幂等主记录的投影摘要必须相同。operation 收敛后只保留恢复 committed 结果所需的最小 claims/kid/请求投影，committedAt+24 小时与对应幂等记录同时删除，禁止按 180 天事件期限延长；密钥轮换必须保证所有未到期 processing/committed 记录可解密。`authorization_coordination_aborted` 的 lineage 终态审计只保留 coordinationAbortedAt、稳定原因码、authorizationIssued=false 及必要发布/决定标识，按升级事件 180 天期限留存；可写凭据索引在终态提交时立即删除，不能因审计留存恢复写权限。

## 16. 事件、指标与告警

客户端至少上报检查、展示、延期、制品选择与未选增量原因、下载、校验、重建、安装授权、权限、安装、健康确认和人工修复入口展示等阶段结果。离线事件队列默认上限 10 MiB；遥测会话事件最长保留 7 天，安装事务的安全关键迁移和终态最长保留 37 天。恢复网络后按原凭据和 `sequence` 分批上报；超限优先删除最旧的非关键进度事件，不得删除尚在保留期内的授权消费、安装开始、安装结果、健康结果和失败终态。确需丢弃时必须保留本地计数并在下一次合法会话上报 `events_dropped` 聚合指标，不得把事件改绑到新的凭据。

后台按版本、平台、架构、来源版本、阶段和稳定错误码统计检查命中、转化、安装失败、健康失败、人工修复状态、全量兜底、下载速度及文件源可用性。检查、展示和下载阶段使用 `decisionId + deploymentId + releaseTargetId + installId` 关联；安装授权消费后的安装/健康阶段以 `transactionId`（或安装事务凭据 jti）为唯一主键，并反向关联 decisionId、deploymentId 和 installationScopeId，防止同一决定下的多次安装尝试被合并。`requestId` 仅定位单次 HTTP 交换；不在普通事件中复制硬件 SN。

P0 告警包括错误制品/签名、撤回仍可授权、安装后大面积不可用和密钥异常；P1 包括接口错误率、文件源故障、安装/健康失败率、自动冻结扩量和人工暂停待复核。告警必须关联处置手册、值班人和处置确认。

## 17. 非功能要求

### 17.1 容量和性能

- 检查服务月可用性不低于 99.9%，按预计峰值 3 倍且至少 1,000 RPS 完成压测。
- 正常负载下检查接口服务端 P95 ≤ 500 ms、P99 ≤ 1 s，不含客户端网络时间。
- 文件信息和安装复核 P95 ≤ 500 ms；事件接口持续处理能力至少 2,000 events/s。
- 客户端定时检查使用服务端间隔、指数退避和 0～10% 随机抖动，避免启动风暴。

### 17.2 灾备

- 已发布元数据的版本分配器、签名字节哈希账本、Updater Source Facts Registry 全量版本/状态/当前指针、各作用域 `selectionGeneration` 及安全兼容允许表、当前 Snapshot/Timestamp 发布世代和安全 deny 状态必须跨故障域同步，RPO = 0、RTO ≤ 30 分钟；不得依赖可能回退 5 分钟的数据库副本重新分配版本。其他尚未发布的控制面草稿和运营配置 RPO ≤ 5 分钟、RTO ≤ 30 分钟；审计日志 RPO = 0。
- 故障切换时先扫描双故障域不可变对象与哈希账本，恢复各作用域已发布最大版本和当前世代，再从严格更高版本继续分配；任何已使用版本永不重用。无法证明最大值时停止发布但可继续提供已验证且未过期的当前世代，不能猜测版本号。
- 已发布不可变制品完成双故障域复制后才允许激活，因此已发布制品 RPO = 0；单一文件服务故障时 10 分钟内可签发备用源。
- 发布暂停对新检查和安装复核 60 秒内生效；安全撤回及 CDN 失效目标为 5 分钟内。

### 17.3 安全和可维护性

- 服务端、控制面和发布系统的全部长期秘密必须存于集中式密钥管理系统；客户端本地快照数据密钥及其包装主密钥不适用该集中式要求，只能按 10.7 的三种 keyProtectionProfile 存放和销毁，不得降级为普通应用可读配置。日志禁止令牌、URL 查询参数、明文 SN、用户路径和业务内容。
- 后台无状态水平扩容，状态变更使用事务、幂等键和并发版本。
- 生产发布前完成威胁建模、第三方文件服务评估、依赖扫描、安装失败处置演练及密钥轮换演练。

## 18. 验收标准

### 18.1 协议与签名

- 服务端、Windows、Linux、macOS 对 RFC 8785/Ed25519 固定测试向量结果一致。
- 元数据固定向量覆盖完整封装 JCS 字节、签名排序、UTF-8/无 BOM/identity 传输、长度与哈希；有任何空白/编码/压缩/重复 key 差异时拒绝。测试顺序必须是 Root 顺序更新 → Root 验 Timestamp 签名 → Timestamp 验 Snapshot 封装 → Snapshot 验三类 Manifest，不允许以 Snapshot 反向验证 Root/Timestamp/Snapshot。
- 修改任一 `signed` 字段、签名、Manifest 哈希或下载制品后均被拒绝。
- 客户端能通过不可变元数据端点获取 Target、Release 和 Package Manifest，先按 Snapshot 中的路径/版本/长度/哈希验证原始字节，再验证引用链与签名；缺失、截断、路径哈希不符、内嵌 Release 副本不同或从 Snapshot 移除时均拒绝。
- 同一不可变元数据对象跨多次请求的响应体字节、长度和 SHA-256 完全一致；不同 `X-Request-Id` 只存在于 Header，不进入签名内容或内容哈希。
- Package Manifest Schema 不包含 `expiresAt`，跨越任意墙钟时间仍可验证其不可变构建事实；没有当前有效 Target/Release/Snapshot 授权时，即使 Package Manifest 签名有效也不得下载或安装。
- 构建密钥正常 retired 不影响既有 Package Manifest；构建密钥 revoked 时，受其影响的 Package Manifest 和 Release Target 必须被拒绝。
- Online Authorization key revoked 后不补发旧 lineage/事务凭据；已开始事务仅本地收敛并产生不可冒充协议终态的遥测缺失标签，新检查必须以新 key 建立独立会话，不能继承旧 sequence、确认或授权。
- Target/Release/Snapshot/Timestamp 密钥 retired 后，仍可达元数据在公钥移除前完成阈值重签和原子级联；提前移除被发布闸门拒绝，revoked 则立即进入安全撤回/重签。
- 能验证 2-of-3 Root、2-of-3 stable 发布、双签轮换、单一发布密钥不可独立签 stable、撤销、过期、防回滚和同版本不同哈希；同一 keyId 重复签名不能满足阈值。
- Root 更新固定测试覆盖顺序 `n+1`、旧/新双阈值、原子持久化、跳号、中间版本缺失、过期 Root 仅允许恢复根更新及 32 个版本的单轮上限。
- `n+1` Root 不存在时的 404 不得被客户端或 CDN 负缓存，下一轮能及时发现新 Root。Snapshot 只含当前可达集合，并在 8 MiB/10,000 条硬上限和 70% 告警线内发布。
- Release Manifest 防回滚按 `deploymentId` 隔离：历史桥接 Deployment 续签为修订 100 后，客户端仍能接受 Snapshot 同时允许的另一 Deployment 修订 99；同一 Deployment 回退到修订 98 则必须拒绝。同一 Release Target 在 stable/beta 的两个 Deployment 也不互相污染防回滚状态。
- 演练 Target Metadata 纯时间续签：所有当前基线/候选/桥接 Deployment 的 Release Manifest 都同时生成新修订并指向新 Target，Snapshot 原子切换；任一新旧交叉引用或过期 Target 引用均被拒绝。
- 分别修改 `supportFloorVersion`、Windows/Linux/macOS 宿主约束和 `installScope`，验证所有继续有效 Deployment 的 Release Manifest 级联修订，不兼容投放先暂停/撤回，Target/Release/Snapshot/Timestamp 成套原子切换；任一旧 Target 引用或遗漏的有效 Deployment 均阻止发布。
- 检查响应的 decisionToken 必须先由客户端验签，并对 requestNonce、安装两类 ID、版本、渠道、目标、hostArch、协议、能力摘要和响应标识逐项比对；重放另一次检查的合法响应也必须拒绝。
- 下载 URL 域名和查询参数变化不影响制品身份；URL 不能替代哈希/签名判断。

### 18.2 更新包与增量预留

- 四类目标均能上传、解析和校验完整包；危险路径、超过上限、哈希/平台/签名异常均拒绝。
- Schema 覆盖 `incrementalPackages=[]` 和至少两个非空候选。
- 使用 `bsdiff-v1` 参考实现从指定基础制品重建目标制品，并验证结果大小、哈希及平台真实性；错误基础、错误补丁和错误结果均拒绝。
- 完整包和增量包均分别验证包容器大小/`sha256` 与最终安装器 `resultArtifactSize`/`resultArtifactSha256`；交换 ZIP、替换内层安装器、伪造重建大小、把容器哈希冒充安装器哈希或重建出错误安装器时均拒绝安装授权。
- API 返回完整的非空增量候选字段；无基础缓存、缓存损坏或首期空能力时稳定选择完整包。
- Full/Incremental 每个 packageId 的 helperArtifacts 在 Package、Ota 和 Release 三层逐项一致；决定绑定全包索引摘要，安装授权绑定所选包数组摘要。不含助手、同版本助手、助手升级、助手条目被替换/增删、重复 helperId、重复目标路径、乱序和跨一代协议均有固定结果。
- 助手替换攻击分别覆盖：Windows Publisher/SPKI/时间戳任一不符；macOS 复用主 App Bundle ID、错误 helper identifier/designated requirement/Team ID 或未公证；Linux 以 DEB 身份冒充 ELF 助手、错误哈希/ELF 架构、可由非 root 写入或目标路径不符。任一攻击在写入受保护目录和进程启动前拒绝。
- 增量候选的 `baseArtifactSize + baseArtifactSha256` 在 Package、IncrementalOtaInfo、Release 三层完全一致；基础缓存大小或哈希任一不符均不得重建。
- `bsdiff-v1` 在 CPU、内存、30 分钟墙钟、输出大小和中间展开任一预算越界或被取消时安全终止并删除 staging，随后选择完整包；签名补丁不能通过构造异常资源消耗绕过预算。
- Linux `platformIdentity` 三层一致性覆盖 packageName、版本、amd64/arm64 规范架构、必需 Maintainer、可选 Vendor 及其规范化，以及四类维护脚本的 absent/空文件/非空文件；增删或替换任一脚本均拒绝。
- Linux helper/launcher 身份只绑定 `metadataRole=build` 而不绑定具体 keyId；Build A→B 正常轮换后可用 B 重签更高 Package Manifest 并级联退出 A，无需改变 OtaInfo/包哈希。错误角色或未满足当前 Root Build 阈值仍拒绝。
- macOS/Linux helperAuthenticity 与 launcherAuthenticity 的 JSON Schema、NFC/ASCII、路径模板、八进制 mode 和固定测试向量在服务端与三平台得到一致结果；未知字段/type、错误 Team ID/identifier、ELF 类型/架构、metadataRole、uid/gid/mode 均拒绝。
- postInstallLauncher 在 Package、Full/Incremental OtaInfo 和 Release 三层逐项一致；同版、升级、哈希/真实性替换、安装后缺失及降级高水位均有固定结果，launcher 只能由主平台安装器事务更新。
- Release Manifest 含未知的可选增量算法时只忽略该候选并回退完整包；未知能力位于 `requiredCapabilities`、顶层策略或已选制品时才返回 426。
- Release Manifest 和安装授权 Schema 均不包含恢复包字段；客户端不会因为缺少旧版本安装器而拒绝目标版本安装。
- 后续启用增量不改变 V1 字段含义，且失败自动全量兜底。

### 18.3 后台、状态与灰度

- Artifact、Release、Release Target、Deployment、Rollout 状态独立，非法转换和并发冲突被拒绝；单一目标 rejected/revoked 不改变其他目标。Release 首次 closed 时拒绝既有 draft/in_review 目标；关闭后既有 targetKey 的新修订可独立处于 draft/in_review 而 Release 保持 closed，新增 targetKey 仍被拒绝。
- rejected/approved/revoked Release Target 和 rejected/withdrawn/superseded Deployment 的业务字段不可原地修改；改变最低来源版本、制品、迁移或策略时必须创建新 ID 从 draft 重审，旧审批不继承；纯时间续签才允许保留 ID。
- 每个渠道/目标至多一个基线和至多一个活动候选；无基线时返回 `channel_has_no_baseline`，不得回退其他渠道或 superseded 版本；暂停候选回基线、晋升原子完成。
- Deployment/Rollout 可从 paused 经人工复核恢复；Artifact quarantined、Release Target revoked 及终态 Rollout 不可原地恢复。并发恢复、撤回和晋升只能有一个按乐观版本成功。
- `minimumSourceVersion` 可缺省且缺省值为 `0.0.0`；显式值必须是小于目标版本的合法 SemVer。缺省与显式 `0.0.0` 的选择获批后不得原地切换，任何变更必须产生新 Release Target/Deployment ID、新 Manifest，并重新签名和审核。
- 来源兼容范围始终为连续区间 `[resolvedMinimumSourceVersion, targetVersion)`；Schema 不提供 `allowedSourceRanges` 或单版本排除字段，后台和客户端均不能构造离散来源集合。
- 非 none dataMigration 的 sourceAppVersionRange 必须精确等于 `[resolvedMinimumSourceVersion,targetVersion)`；上下界任一不等时在 Release Target 审核、可达性建边和选择三处均失败，不能先返回包再等 validate 拒绝。
- Target Metadata 的 `supportFloorVersion` 按产品/组件/渠道/目标隔离且必须对应真实发布或迁移版本；stable 初始值等于 `migrationBootstrapVersion`。后台能展示从当前已签名 `supportFloorVersion` 到当前发布的逐跳可达性矩阵，并阻止存在未审批路径断点的 stable 发布。低于下界的请求即使存在 `minimumSourceVersion=0.0.0` 的发布也只返回人工指引，不下发制品。
- 已在同一渠道达到 100% 并成为过基线的 superseded 版本可作为桥接版本继续提供；不得跨渠道桥接，paused、withdrawn、revoked、隔离制品和未批准灰度版本不能被选为桥接版本。
- 桥接 Manifest 过期前能按阈值续签且不改变业务字段；过期未续签时停止返回并产生路径断点告警。
- 上传者不能批准本人 stable 发布；稳定、强制和紧急修复发布满足双人审核。
- internal/beta 的 Release Target 和 Deployment 也必须有一名与上传者不同的人员批准，单签密钥不等于免人工审批。
- irreversible 的 backupPolicy 必须显式为 none 或 required；none 只接受空 scope、空注册表身份、backupHashAlgorithm=none、零额度/零时长/verification=none，required 完全满足备份约束，缺失或交叉组合在 Schema 与审核闸门拒绝。
- preview 级 Release Target 不能创建 stable Deployment；从 preview 晋升 stable 必须创建新的 stable 级 Release Target 并完成稳定渠道双人审批，不能复用原 ID 或批准记录。
- BridgeEligibility 的启用/禁用独立审计且不修改 Deployment；禁用后旧 revision 不得恢复资格，paused/withdrawn/revoked/quarantined 始终不能被选中。
- 管理后台验证企业 SSO、MFA、角色越权、会话失效和高风险操作二次认证；共享账号、服务账号代替人员审批、篡改或删除审计记录均被阻止并告警。
- 以 `1.3.0` 为问题版本、`1.3.1` 为修复版本执行演练：激活 `1.3.1` 时原子关闭 `1.3.0` 的桥接资格；`1.3.0` 客户端只获得 `1.3.1`，任何更早来源的路径都不再经过 `1.3.0`。
- 伪造低 `currentVersion` 不能获得降级安装结果；升级助手检测实际安装版本高于或不等于授权来源版本时必须拒绝消费或执行安装。
- 无需登录品悟账号的用户也能查看并自愿申请切换 `internal`、`beta` 和 `stable`；“未登录”不免除 per-machine 安装所需的本机管理员 OS 授权，普通 OS 用户只能发起申请而不能提交受保护 channelRevision。加入 `internal`/`beta` 不要求白名单或 SN，退出预览渠道时若当前版本高于 stable 基线则返回 `current_version_ahead_of_channel`、保持当前版本且不返回降级目标。
- 同一 per-machine 安装的两个用户会话读取同一渠道 revision；普通用户切换、并发相反写入和旧 revision 写入均被拒绝，管理员授权的最后一次 CAS 提交对所有会话生效并使旧渠道决定失效。
- 覆盖 `beta → stable → beta`：channelRevision 每次递增，第一次 beta 的 decision/telemetry/install token 即使渠道名称再次相同也全部失效。
- `internal`/`beta` 拒绝 deadline/mandatory 及远程强制安装；非法策略组合在后台保存、Manifest 签名和客户端执行三处均失败。`autoWhenIdle` 未经用户显式开启、需要临时提权或数据迁移非兼容时不得执行。
- optional/recommended/deadline/mandatory 分别按 10.4 的重提示、宽限期和限制时点执行；拨回系统时钟不解除已生效限制，无可信时间的首次离线评估失败开放，限制后仍可保存、导出、查看修复指引和退出。
- 时钟固定向量覆盖相对 suspend-aware 单调锚点后退 2 分钟/多 1 秒、前跳 10 分钟/多 1 秒、休眠、bootId 改变、异常持久化和 time-challenge 恢复；重启后即使缓存 Timestamp 是数小时前签发且仍未过期也不能清除异常，只有绑定本次 nonce、单调往返不超过 2 分钟且不可重放的签名 time-attestation 建立新锚点，再通过完整元数据链后才能恢复。另覆盖错误前拨使 Root 表面过期：Root 在 lastTrustedTime 尚有效时只可验证 time-attestation，恢复后须顺序更新 Root；Root 在 lastTrustedTime 已过期时挑战也拒绝。异常不能首次启用限制或被本地改标志清除。
- mandatory 在 Manifest 续签前后使用相同 `enforcementEffectiveAt`，限制时点不得被延后；实际版本到达目标、可信元数据确认投放不再有效或 Manifest 离线租期到期时解除该投放的限制，不得将已无法安装的 revoked 版本永久锁定用户。
- 硬件 SN 缺失、非法或读取失败不返回身份错误，也不阻断升级：低于 100% 或 SN 定向候选视为未命中，并继续返回适用的基线或兼容桥接版本；100% Rollout 无 SN 也能命中。
- SN 包含/排除组只影响候选 Rollout，不得阻止基线、兼容桥接或已全量发布的前向安全修复；改变 SN 不能绕过签名、版本兼容、发布状态、令牌和安装授权校验。
- 用至少 10,000 个固定 SN 验证基于 RFC 8785 对象的 HMAC 分桶测试向量、比例误差和扩量集合单调性；覆盖 releaseId 中分隔符、CR/LF、控制字符、Unicode 非法输入和规范化 SN 边界，证明不存在拼接歧义；同 SN 重装后桶不变。另用缺失、格式错误和检查后 SN 变化的用例验证其不构成安全授权边界。
- 普通 check 幂等测试证明同一 installId 可跨任意周期检查复用，而 requestNonce/key 只可恢复同一规范语义请求；JSON 换序/空白不冲突、语义字段变化冲突。服务端持久记录只含用途隔离 HMAC 和 keyId，不含原始 SN 或无密钥可枚举摘要；轮换后仍能以记录 keyId 在 24 小时内恢复，Rollout/名单密钥与两类幂等 HMAC 密钥绝不复用。
- Rollout 扩量前后同时固定 `bucketKeyId + snNormalizationVersion + snNormalizationProfileSha256`；运行中更改任一值被拒绝，三平台执行同一 V1 注册表测试向量，不得因规范化规则或实例配置漂移破坏已命中集合的单调性。
- 演练 SN HMAC 密钥泄露时暂停旧 Rollout，并以新 keyId 和新 Rollout ID 从初始比例开始；不得声称新旧分桶连续，且基线升级不被阻断。
- 通过模拟样本验证质量阈值：达到阈值时自动冻结扩量并告警，但仅凭客户端事件不得自动 paused/deny；经人工复核暂停后不自行恢复。批量伪造 installId/installationScopeId、单代理与代理池攻击只能降低数据可信度并触发告警，不能单独暂停；同一网络簇贡献上限不掩盖经多簇、平台和版本信号确认的合法集中故障。
- 候选决定签发后 Rollout 经人工暂停或安全 deny 时，直接候选的 validate/consume 被 rollout deny 拒绝；`candidate_prerequisite` 在基线及所有策略/制品逐字未变时仍可取得该基线 download-info 并只安装已授权基线，之后必须重新检查且不能沿旧令牌取得候选。

### 18.4A API、选择与服务端安全合约

- 使用模拟客户端完成 OpenAPI/JSON Schema、签名元数据链、检查/无更新 decisionToken、Target/Release/Package 引用、最低来源版本、当前基线前置跳、历史桥接、文件信息、动态地址和 validate/consume 服务端合约测试；不以真实平台安装器、健康观察或存量迁移完成作为阶段一 A 的前置条件。
- 四类 JWS 必须精确校验各自 `aud + tokenUse + Schema`；把任意一种合法令牌投递到另外三类端点均返回 `token_type_invalid`。正常轮换必须先传播含新验证公钥的 Root 后才启用签发，旧密钥随后进入 retired verify-only；其 Root 验证授权必须跨越最后凭据 exp、普通 check 结果及相关 refresh/validate/consume/cancel 幂等记录到期和 `exp+24h` lineage 关闭截止中的最晚者再加 2 分钟，任何一个恢复窗口尚存时删除均被阻止。先签新令牌再发布 Root 的故障注入必须被发布闸门阻止；revoked 密钥及其全部幂等重放立即拒绝，普通 check 已提交结果含 revoked decision/telemetry kid 时返回 `credential_revoked`、不创建第二 session，客户端只有改用新 nonce/key 才能建立独立会话。Build retired key 仅在全部仍可达 Package Manifest 以 active key 重签并完成级联后移除；否则 Root 删除被阻止。
- 固定时间边界覆盖 `iat/nbf=trustedNow+2min` 可接受、再多 1 秒拒绝，`exp==trustedNow`、`installStartNotAfter==trustedNow` 和 `mainInstallerStartNotAfter==trustedNow` 均拒绝；Timestamp 签发 24 小时但尚未过期时不能因“超过 10 分钟”被拒绝。
- 预解析资源上限覆盖每类元数据字节、深度、签名、hostConstraints 和增量候选边界，超限输入在大对象分配前拒绝。
- 激活/暂停/撤回故障注入验证两阶段可见性：检查决策引用的 Manifest 必在其当前 Snapshot 中，安全 deny 生效后即使旧 Snapshot/CDN 尚未完全传播也不能获得新授权。
- Target 业务变更后旧 selectionGeneration 的 validate/consume 全部失效；规范化 osVersion、Target/Release Manifest 引用或 generation 任一不匹配均拒绝。纯时间续签不提升 generation，未过期旧决定仍可按相同业务语义复核。唯一例外测试证明仅因候选暂停产生的旧 `baseline-prerequisite` 可在基线与策略逐字未变时安装该基线，不能安装候选或在任何其他变更后继续。
- 直接候选决定签发后候选晋升基线会因 generation 变化而拒绝旧 validate/consume，客户端重新检查后才能按新基线决定继续；不得套用 baseline-prerequisite 例外。
- updater 能力可达性测试覆盖“当前 backup registry/算法不匹配→安装携带新 updater 的桥接版本→下一次检查直接命中期望终点”，以及“所有安全桥接仍无法获得 required capability→426”；不得在第一步误删可桥接终点、直接返回能力不匹配目标或把无能力路径误报为 no_compatible_update。
- updater 完整事实合约覆盖 currentHelperFacts 为空、完全重复、乱序、同 helperId 多版本并存、activeHelperRef 缺失/不唯一、外层协议不一致、helper/launcher 大小/哈希/安装目标/真实性摘要漂移和受保护记录与实际文件不一致；客户端对本机事实 revision/事务日志回退及后两类本地不一致不发 check，服务端对不属于 Source Facts Registry 的语义事实返回 426。事实未变化的周期检查重复相同 revision+摘要必须接受；向普通 check 注入极大 revision 后，真实客户端原 revision 的新请求仍按自身事实正常选择，证明服务端未被高水位投毒。两台 currentVersion 与 helperProtocolVersion 相同、但活动 helper 版本或 SHA-256 不同的固定客户端必须得到不同且确定的 packageIds/桥接选择；capabilitiesSha256、currentUpdaterFactsSha256、decision、installIntent、validate、授权和 consume 任一阶段篡改均拒绝。
- 无更新响应必须同样验证 `currentUpdaterFactsSha256`：固定请求得到签名的 updateAvailable=false 后，替换响应令牌中的 updater 事实摘要、把另一台设备的无更新令牌互换，或让客户端本地保存摘要与令牌不等，均必须拒绝无更新结论和后续事件上报。
- 助手计划固定矩阵覆盖：活动 A-v1、已安装但未激活且与目标完全相同的 A-v2 时，必须得到 bytes=false、handoff=true、update=true，不能直接进入 installer_started；活动目标助手完全相同时三个值均为 false；目标任一助手字节缺失时 bytes=true。多助手数组必须只由 transactionHelperId 指定执行者，Package/Ota/Release、决定、installIntent、validate、授权或 consume 任一处替换 transactionHelperId/三个布尔值均拒绝。

### 18.4B 迁移、安装事务与端到端撤回

- OpenAPI/JSON Schema 合约测试覆盖成功、无更新、所有稳定错误码、时间格式、429/503 重试和已切换客户端的 426 协议过低场景。
- 从每个受支持的存量来源版本执行旧系统升级，均只能得到同一 `migrationBootstrapVersion`；该版本必须通过对应平台真实性验证并内置预期的新平台初始 Root。
- 模拟迁移包安装失败、首次启动崩溃、Root 自检失败，以及在 installationScopeId/渠道记录/安装事实/助手记录各写入点断电时，不得提交 `upgradeProtocol=new-v1`；安装失败按旧机制重试或进入人工修复，已安装迁移引导版本则保持 `transitioning` 并通过原子日志安全重试初始化或接受同版本修复。
- 迁移引导版本首次健康启动后原子提交切换；此后即使 `update.pinvou.com` 暂时不可用，也不得回退旧协议或旧端点。重复运行、多用户同时首次启动和断电恢复必须复用唯一 installationScopeId、`stable/revision=1` 及同一受保护安装记录，不得产生双重检查、重复安装或分裂状态；提交 new-v1 后删除/篡改记录只能进入人工修复，不得补生成新 ID。
- dataMigration 合约以助手复核的来源 canonicalAppVersion 验证结构化 `sourceAppVersionRange.minInclusive/maxExclusive`；Release/decision/installIntent 只绑定含 scope 注册表版本/哈希、backupHashAlgorithm 和 backupContainerFormat 的静态策略，validate/授权另绑定完整 backupFacts。缺失、损坏、超界、自由格式字符串、注册表/算法/容器格式漂移、实际字节或耗时超限、快照或数据源 revision 变化和 decision/validate/授权间来源版本不一致均拒绝；policy=none 仅接受 `not_applicable`、hashAlgorithm/containerFormat=none、actualBytes/durationMilliseconds=0 且五个可空运行字段全部为 null。`pinvou-backup-manifest-v1` 固定向量逐项覆盖排序、编码、空目录/文件、内容变更但 mtime 不变、路径冲突、链接/特殊文件拒绝；`pinvou-backup-container-v1` 使用固定 key/nonce/keyRef 向量覆盖精确 magic/长度/JCS header/AAD/ciphertext/tag/整容器哈希、三平台 key protection profile、reopen 后清单/内容复核以及 header/AAD/tag/keyRef/密文篡改拒绝。
- 新平台可达性矩阵从 `migrationBootstrapVersion` 起算，首个 stable 基线可由其直接到达或通过完整桥接路径到达；更早版本不能直接调用新平台获得制品。
- 426、低于 support floor、Root 恢复和安装失败只能返回 `supportGuideCode`；伪造响应 URL/主机/重定向不能改变客户端内置的 HTTPS 官方路径，未知 code 显示本地通用指引。
- 兼容选择测试至少覆盖：字段缺省与显式 `0.0.0` 得到相同选择结果；`currentVersion == minimumSourceVersion` 且 updater 边兼容时可直接命中；低于最新发布最低来源版本时返回满足条件的最高 SemVer/能力桥接版本；安装桥接版本后以其签名能力事实重新请求能进入下一跳；仅版本无可用跳点时返回 `no_compatible_update`，仅 required capability 无可达跳点时返回 426。
- 固定灰度向量覆盖：命中候选但只能经当前基线到达时返回 `candidate_prerequisite`；安装基线后重新检查仍命中才返回候选；未命中设备只得到 `rollout_baseline`，不得因中间跳进入候选。
- 合约测试必须拒绝非法 SemVer、`minimumSourceVersion >= targetVersion`、`allowedSourceRanges`、单版本排除配置及任何 `targetVersion <= currentVersion` 的定向降级结果。
- 使用 `1.1.0(min=0.8.0) → 1.2.0(min=1.0.0) → 1.3.0(min=1.2.0)` 固定测试链验证 `0.9.0` 依次获得 `1.1.0`、`1.2.0`、`1.3.0`，且一次响应只包含一个目标发布。
- 修改请求 `currentVersion` 后，原 decisionToken、文件信息资格和安装授权全部失效；客户端不能通过自行选择 Release Manifest 绕过最低来源版本。
- 时序测试证明用户确认和系统提权先于 required 最终备份，备份从全部强制锁取得并首次读取起计时，直到容器 reopen 与冻结下最终 revision 复算完成；恰等于 maxBackupDurationSeconds 成功，多 1 毫秒失败，休眠计入，重启/bootId 或单调钟连续性丢失失败。成功后同一强制锁、句柄和 quiesceFencingToken 连续覆盖决定刷新、validate、consume，直到安装器验证并接管；在 validate 后/consume 前注入写入、锁丢失、权限丢失、进程或 boot 变化均不得启动安装器。postBackupQuiesceDeadline 恰到 10 分钟时失效且刷新不得延长；validate 未提交走 backup_invalidated，已提交则恢复结果后 cancel/等到期。required 路径必须两个原始助手布尔值均为 false，并在 consume 后 2 分钟内由 transactionHelperId 对应活动助手直接启动主安装器；另以 backupPolicy=none 分别验证“需要写字节并接管”和“字节已存在但只需接管”两条路径在 2 分钟内开始受保护计划并在 5 分钟内启动主安装器。UAC/系统权限拒绝不得发生在授权消费之后。
- 用慢速 4 GiB 下载、24 小时备份和超 15 分钟权限等待验证决定刷新：未过期且完整绑定的 lineage 凭据可等价续期同一 decision/session revision，并仅复用哈希、数据源 revision、selectionMode 和确认摘要都匹配的成果；`direct`、`rollout_baseline`、`candidate_prerequisite` 或 rolloutDecisionKind/rolloutId 变化必须改变 installIntent 并使旧确认失效。发布策略、迁移、助手、包、渠道或 OS 任一实质变化时旧确认/备份/staging 均不能直接进入 validate。凭据替换保持唯一状态/sequence；不存在本端点幂等记录时，旧凭据在 refresh、validate、events 均返回 `credential_replaced`，不能以旧凭据推进或读取 lineage。
- 故障注入让等价刷新在服务端提交后丢失响应；完全相同 Idempotency-Key/请求摘要重试先命中 24 小时记录，恢复原 decision/telemetry 凭据且 requestId 更新，不再次提升 revision。通过签名/类型/不可变绑定校验后，改变 nonce、sequence、凭据字符串或其他摘要字段返回 idempotency_conflict；无记录的 replaced 凭据不能修改 lineage。
- 决定刷新必须拒绝坏签名、错误 aud/tokenUse、跨 installId/installationScopeId/decisionId、replaced/revoked 及旧 sequence，并证明旧会话未改变；过期但可验证凭据只能关闭旧 lineage 并新建独立会话，不得续接 sequence 或复用确认/备份；无效凭据只能去掉 refresh 参数另发普通检查。
- 模拟安装授权已消费但响应丢失：5 分钟 `exp` 前首次消费成功；同一 `jti + transactionId + Idempotency-Key + 请求摘要` 在过期前、过期后且首次提交后未满 24 小时均得到同一业务结果、事务凭据与启动窗口，但每次 HTTP requestId 都不同，不重复消费或延长启动窗口；过期后首次消费拒绝并原子写 authorization_expired，恰到 24 小时边界停止返回，revoked key 始终拒绝，任一绑定变化均拒绝。超过 `installStartNotAfter` 未开始首个授权写入，或超过 `mainInstallerStartNotAfter` 仍未启动主安装器时，只能上报 `abandoned_before_install`，不得继续启动。
- 故障注入让 validate 成功/失败提交后丢失响应；同 transactionId 和相同规范语义请求在完成签名/类型/绑定及 revoked 检查后、凭据新鲜度和 lineage 状态检查前命中 24 小时记录，故 decision/telemetry 凭据过期后仍恢复原 installAuthorization 或原 valid=false 失败且 requestId 更新，不再次写状态、不延长授权。同 transactionId 改变 decision revision、sequence、制品或 backupFacts 返回 idempotency_conflict；仅改变 JSON 空白/成员顺序不冲突。不同 transactionId 永不查询旧记录：只有首个操作能创建 lineage 唯一 activeValidateOperation，另一个在 marker 存在时返回 authorization_in_progress，首个最终提交后返回 lineage_state_conflict；无命中记录的过期凭据严格拒绝。
- install_authorized 后分别测试 cancel、自然到期和 consume 三方 CAS 竞争，任何交错只能一个成功：取消/过期后不能消费，已消费后不能取消，到期定时任务最终收敛无客户端请求的授权。cancel 严格按“签名/type/revoked→claim 与请求/服务端授权绑定→同 key 摘要冲突→精确已提交命中→首次到期/状态 CAS”测试；绑定错误只能返回 install_not_authorized，同 key 不同摘要只能返回 idempotency_conflict。取消响应丢失后，同一取消幂等请求在 24 小时内即使授权已过期仍返回原结果；无记录的到期取消先写 authorization_expired，revoked kid 始终拒绝。
- consume 合约按“签名/type/revoked→claim 与请求/服务端授权绑定→同 key 摘要冲突→精确已提交命中→首次消费”的顺序逐项测试；claim 绑定不符唯一返回 install_not_authorized，同 key 不同规范摘要唯一返回 idempotency_conflict，精确命中跨 exp 恢复原结果。
- 多条件优先级固定向量必须覆盖：refresh 已提交且旧凭据 replaced 后，以同 key 改 nonce/sequence 返回 idempotency_conflict；validate 已提交后，以同 transactionId 改 lastAcknowledgedSequence 返回 idempotency_conflict；consume 与 cancel 同时制造 claim/request/server-record 绑定错误和同 key 摘要变化时只返回 install_not_authorized。三者均不得随数据库查询、异常抛出或表格行顺序改变。
- check、refresh、validate、consume、cancel 分别并发提交两份同 key/同摘要请求：先取得 processing reservation 的请求独占副作用，租约有效时另一请求返回 idempotency_in_progress 与 Retry-After: 2；同 key/异摘要仍返回 idempotency_conflict。对每个端点分别在 reservation 后、领域事务提交前、领域事务提交后但 HTTP 响应前终止 owner：trustedNow=leaseExpiresAt-1ms 时只有原 owner 可完成最终提交，恰等于或晚于 leaseExpiresAt 时原 owner 必须失败且只允许一个 ownerEpoch CAS 胜者接管；ownerStartedAt+2min-1ms、恰等于和 +1ms 使用同样边界。故障注入须覆盖旧 owner 先算好签名后新 owner CAS、以及新 owner CAS 后旧 owner 再提交两种交错，旧签名/响应均被丢弃。验证 operation key 恢复只能产生一个 session/revision/授权/消费/取消结果，committedAt 起 24 小时边界精确，processing 永不被 TTL 清除后重做。
- validate/refresh 逐点交错测试至少覆盖：validate 的旧 owner 在 leaseExpiresAt 恰到时与新 owner 竞争创建 marker，只有仍同时拥有 processing ownerEpoch、未到期租约和完整 lineage CAS 的一方能创建；refresh 先提交必须使旧 validate 因 lineageConcurrencyVersion/credential jti/revision 变化失败，marker 先提交必须使 refresh 因 marker/版本 CAS 失败并返回 authorization_in_progress。不得出现 idem owner 与 marker owner 不同、旧凭据进入 requested、decisionRevision 被覆盖或处理永久悬挂。
- validate 独立提交 activeValidateOperation 后、最终事务前终止 owner：无 refresh 幂等记录的 refresh 和另一 transactionId 的 validate 都唯一返回 authorization_in_progress/Retry-After: 2；相同 transactionId 在 marker/processing 租约有效时优先返回 idempotency_in_progress，租约到期后只有在一个事务中同时 CAS 取得幂等记录、marker 和 reservedLineageConcurrencyVersion 的 owner 或后台恢复器能继续。分别在 marker 后注入 decision/telemetry kid revoked、凭据过期、Deployment pause/withdraw、Target/Artifact deny、channel/generation/Rollout 变化和 owner 崩溃，必须重验并唯一收敛为 coordination_abort tombstone：清除 marker、写持久终态 authorization_coordination_aborted、立即关闭可写凭据索引、不写 authorization_failed、不签第二授权；公开 revoked 请求仍只见 credential_revoked，后台恢复器仍能在 30 秒扫描周期后独立关闭 marker。终态后 refresh、events 和另一 transactionId validate 均不能推进旧 lineage，只能全新普通检查。
- 每次动态 JSON API 调用生成不同 `requestId`，同一检查决定的 `decisionId` 在文件信息、安装复核和事件链路保持一致；两者互换、跨决定复用或绑定不一致均被拒绝。不可变元数据仅改变 X-Request-Id Header，响应体身份不变。
- V1 错误码与无更新 reason 逐项匹配 11.6 封闭注册表及条件映射的端点、HTTP、valid=false Schema、retryable、重查/lineage 和 supportGuideCode；逐项覆盖旧 sequence、凭据过期、包/结果哈希、helper/launcher、backupFacts、实际版本、lineage 状态、Deployment withdrawn/superseded、Artifact quarantined、Rollout aborted/completed。任何未注册 code、错误 HTTP/状态迁移、前置失败误写 authorization_failed 或 details 暗藏新机器码均使合约测试失败。
- 事件接口拒绝无服务端凭据、自签凭据、跨 `installId`/`installationScopeId`/决定/事务复用、凭据不允许的阶段、乱序倒退、非法状态跃迁及终态后续写；同一 `eventId` 重放只返回 `duplicate`，同批连续合法迁移可以按 `sequence` 验证并接受。V1 指标 phase/status 只由服务端从 state 派生；事件示例不含禁用字段，客户端提交或旧导入值与派生值冲突时拒绝且不进入统计。
- 遥测 sequence 以 telemetrySessionId lineage 严格增大，等价刷新凭据后不重置；安装事务首个客户端事件以 transactionId/sequence=1 开始，服务端初始 authorization_consumed 不占序号。非关键事件丢失导致的数值空洞允许，但旧号、同号不同 payload、跨 scope 复用或跳过状态机安全关键边均拒绝；refresh/validate 的 lastAcknowledgedSequence 仍须精确等于服务端最后值。
- 断网积压必须在 validate 前全部上传确认；policy=none 的 authorizationReadyState 为 permission_granted，policy=required 为 backup_succeeded。validate 绑定 credential/session/lastAcknowledgedSequence，只从相应 ready state 创建可观察 activeValidateOperation 并派生 requested，再由最终事务写 authorized/failed，或由 coordination_abort 写终态 authorization_coordination_aborted，同时提交幂等结果并清除 marker；consume 只从 install_authorized 原子 handoff。required 的 backup_succeeded 在 marker 创建前可因冻结/时限失效进入 backup_invalidated，marker 创建后不得伪造回退状态。缺事件、旧 sequence、终态、authorization_in_progress 和已 authorized 时刷新均有稳定拒绝/继续结果。
- 状态机合约测试遍历 11.5 的全部合法边和代表性非法边；取消只能在 consume 前发生，consume 原子关闭遥测会话并创建安装事务，消费后关闭 UI 不得伪造取消终态。
- 状态机测试必须逐一观察本地预检、权限请求/拒绝/重试、required 最终备份/失效和 validate 结果；不得从 `verification_succeeded` 跳到 install_authorization，不得在 permission_granted 前生成 required 最终快照，`authorization_failed` 只由服务端 validate 失败产生，权限拒绝或 backup_invalidated 不能冒充该状态。
- 遥测凭据覆盖全部 consume 前合法边但拒绝 `authorization_consumed` 之后状态；安装事务凭据相反。助手更新状态只能使用安装事务凭据，凭据交叉使用被拒绝。
- `events_dropped` 只按独立 metricId 上报聚合计数，不推进或伪造旧事务状态；重复指标幂等，关键事件丢失只触发数据质量告警。
- `client_observation` 四类结构化观察均按 observationId 幂等且不推进状态；路径、URL、SN、自由业务内容、错误凭据绑定和用 observation 冒充授权/成功状态均拒绝。
- 安装后关机 7 天、29 天再启动仍可产生并上报合法核对终态；第 30 天仅标记可校正的 `overdue_reconciliation`，第 29 天发生的成功可在第 36 天补报并取代该标签。第 37 天仍无终态时按最后可信阶段分别合成 `abandoned_before_install` 或 `failed_manual_repair_required`，此后不可倒改；但任一有效期内事务凭据的 kid 在上传窗结束前 revoked 的事务必须保持 `credential_revoked_telemetry_unavailable`，不得合成终态或进入普通失败率，只进入安全/数据缺失统计。
- 个性化响应均为 `private, no-store`，共享代理测试不能把一台设备的决定提供给另一台。
- 下载地址最长 15 分钟，过期可按制品身份安全续传；第三方地址请求不含设备标识。生产文件服务接入测试必须按供应商适配器固定向量证明 objectVersion 的请求条件与响应回显，或证明强 ETag，并在对象覆盖/换签 URL 时拒绝错对象；大于 100 MiB 的包还须支持 Range，用 4 GiB 测试对象验证 URL 多次换新后可安全续传。
- Range 测试覆盖 `Accept-Encoding: identity`、正确 206/Content-Range、意外 gzip/br、200、412、错误起止/总长、强 ETag 变化和 416；除已完整且最终哈希通过外，所有不一致都丢弃 partial 从 0 开始，绝不追加错对象。
- 服务端不会为未审核文件服务签发地址；下载客户端拒绝 localhost、IP 字面量、私有/环回/链路本地地址、HTTPS 降级和不合规重定向。弱 ETag 或无对象版本时换 URL 必须从头下载；decisionToken 过期后的续传必须先重新检查并得到同一制品决定。
- 暂停后 60 秒内停止新资格和安装授权；安全撤回后 5 分钟内停止签发并触发 CDN 失效。
- 已下载制品在没有有效一次性安装授权时不能安装；断网安装失败关闭。

### 18.5 三平台安装与结果确认

在 6.1 的每个系统/架构组合上至少覆盖：正常升级、UAC/授权取消、网络中断、URL 过期、磁盘不足、校验失败、多实例锁、应用/安装器/升级助手被结束、安装阶段断电、健康超时和数据备份失败；并且 6.1 **每个 targetKey** 都必须使用 `pinvou-settings-v1` 在对应真实系统完成至少一次 `backupPolicy=required` 的成功强制冻结、备份、validate、consume 与安装器接管，明确包括 Ubuntu x86_64 和 arm64，不能用一个 Ubuntu 架构代表另一个，也不得只验失败关闭。

- 同一 per-machine 安装在两个操作系统用户会话中共享同一 `installationScopeId`；并发升级时只有一个事务获得系统级锁，用户切换、应用崩溃和系统重启后能从受保护全局存储恢复并收敛到唯一终态。
- 本机持有未过期凭据或非终态受保护事务时 UI 重置 installId 被阻止；外部清数据造成新 ID 时，助手仍用受保护事务快照中的旧 ID 补报原事务，新 ID 仅用于新检查，跨绑必拒绝。普通 `/check` 响应丢失先按原 nonce/key 恢复，无本地凭据的旧 session 在 24 小时无认证活动后只派生 orphaned_check_expired；曾有活动但未进入授权/安装的 session 在最后遥测凭据 exp+24h 后派生 session_stale_expired。二者均不伪造客户端终态、不计失败样本，也不永久阻止标识重置。
- 全新安装在首次检查前原子初始化唯一 installationScopeId、stable/revision=1、初始 Root/高水位、安装事实、launcher/helper 完整事实、protectedUpdaterFactsSchemaVersion=1/revision=1、helperOwnershipFencingToken 初值和 new-v1 标记；安装器重复运行、多用户并发及每个写入点断电均只产生一个已提交记录，Root 或权限自检失败不得发起检查。
- 普通应用进程伪造、删除或替换 `installationScopeId` 时，受控助手在检查/validate/consume 链路中能发现不一致，且全局锁仍以系统受保护记录为键；记录异常进入人工修复，不生成新 ID 解锁。
- 版本映射合约分别验证 Windows ProductVersion/FileVersion、DEB Version、macOS ShortVersion/BundleVersion 及旧四段版本映射；非规范值、字段不一致或无法唯一映射时拒绝自动升级。
- 所有渠道均拒绝含 prerelease/build metadata 的产品版本；internal/beta 仅通过渠道表达成熟度，原生包版本与 `canonicalAppVersion` 不得产生另一排序语义。
- SemVer 边界测试覆盖 `0.0.0`、`65535.65535.65535` 和 Windows `<major>.<minor>.<patch>.0` 唯一映射；任一段 65536、四段输入、前导零、Unicode 数字、符号、空白、prerelease/build metadata 和非零 FileVersion 第四段均拒绝。
- Target Metadata 宿主约束测试覆盖未认证 OS、错误 hostArch，以及 Universal DMG 上 Intel 与 Apple Silicon 不同的已认证系统范围；服务端与客户端任一处不匹配都拒绝。
- Windows build、Ubuntu allowedVersionIds、macOS 三段 productVersion、Rosetta 物理宿主架构和 `installScope=perMachine` 的固定向量在服务端与三平台得到一致结果；缺字段、非法数字、未列出 Ubuntu 版本和 Rosetta 进程架构冒充宿主均拒绝。
- Linux 固定向量接受 `versionId=22.04|24.04` 并拒绝 `22.4`/Unicode 数字；将 uname `x86_64`+DEB `amd64` 归一为 `x86_64`，uname `aarch64|arm64`+DEB `arm64` 归一为 `arm64`，未知或交叉映射拒绝。

- Windows：NSIS 退出码和实际版本探测通过；Windows 10/11 x64 各至少一次断电后的事务状态核对，失败时不得自动调用上一版本安装器。三个公开渠道在 Authenticode 吊销状态既无在线结果又无 nextUpdate 内缓存时都失败关闭，internal/beta 不得放宽。
- Linux：Ubuntu 22.04/24.04 的 x64/arm64 覆盖包锁、依赖失败和 `dpkg` 半配置状态识别；在封禁网络的真实机上验证默认系统的依赖闭包与维护脚本，依赖缺失必须在 validate 前失败且不触发 `apt` 下载；非支持发行版不执行安装，失败时不得安装上一版本 DEB。
- macOS：Intel/Apple Silicon 验证签名、公证、原子替换和失败状态确认；未公证包被拒绝，健康失败时不得自动恢复上一 Bundle。
- 升级助手自更新分别在 Windows/Linux/macOS 覆盖 handoff 前崩溃、ready 后旧助手复活、断电、双进程竞争、主应用覆盖和当前/前一协议兼容；任何时刻只有一个助手拥有事务，接管失败在安装开始前中止且仍可由稳定 launcher 恢复，不依赖旧版应用恢复包。固定测试 `bytes=true,handoff=false`：新增非事务助手后 activeHelperRef/token 不变，currentHelperFacts 增项且 revision 恰加一；在 intent、文件写入、事实提交各点断电只能恢复完成或删除未登记字节。清理旧助手也必须原子删除事实并加 revision，任一时刻重新读取均不出现记录/文件漂移。
- 来源事实注册表验收覆盖构建原生、bytes-only、handoff-only、字节加接管、合法清理和 launcher 原/目标边界 profile；未注册组合固定 426，已注册 intermediate 不因原 Deployment superseded/withdrawn 被误判未知。固定执行 handoff succeeded 后阻止主安装器启动并进入 abandoned_before_install：下一次普通 check 接受旧 app + post-helper profile，对同一目标确定得到 bytes=false/handoff=false 并可创建新事务；主安装器已启动但失败时仍须识别实际 profile并只按声明进入人工修复或更高版本前向修复，不得误报未知构建。
- 发布闸门枚举 support floor 内每个实际来源的 helper/launcher 协议并验证每条直接或桥接边；跨两代助手协议、未知 launcher 协议或受保护事实漂移时不得选择该边。多代升级只能通过应用版本递增的桥接 Release，V1 不签发 helper-only 引导授权。固定组合测试证明 `backupPolicy=required` 时，helperBytesInstallRequired 或 helperHandoffRequired 任一为 true 的 packageId 都不进入决定允许集合：存在 backupPolicy=none 助手桥接时先返回桥接，安装后重新检查才返回 required 目标；无桥接时返回 426，validate/consume 的现场漂移也失败关闭。
- 覆盖不含助手、已安装同版本助手、所选包携带新助手、助手受保护安装失败、handoff 超时、授权过期和主安装器 5 分钟窗口过期；任何未授权助手字节都不能写入受保护目录或接管事务。
- 稳定 launcher 全程持有 OS 互斥体并持久化 helperOwnershipFencingToken 高水位；旧 token 进程复活、双进程写入和断电后低 token 重放均被拒绝，只有当前最高 token 能改事务或启动安装器。另验证 quiesceFencingToken 属于独立命名空间，二者不能比较、继承或互换。
- 新版本必须在 120 秒外部观察内通过挑战式健康检查才成功；超时进入 `failed_manual_repair_required`。
- 使用旧进程、另一用户进程、错误可执行文件或重放挑战值模拟健康回报均被拒绝；只有绑定事务和目标进程连续存活至少 30 秒且完成全部检查才可成功。
- `backupRequired` 仅接受签名注册的 backupScopeIds；路径/glob、链接与挂载逃逸、跨用户、TOCTOU、超 maxBackupBytes/50 GiB、空间不足和无法封装加密密钥均在 validate 前失败并删除半成品。快照不上传，只供授权人工导出；活动授权/安装事务期间普通删除被拒绝，成功终态 7 天、失败终态 30 天内清理，终态后管理员提前删除必须二次确认不可恢复影响并写审计；客户端不自动恢复。`irreversible` 不允许自动、限期或强制安装。
- V1 安装器返回需要系统重启、存在待重启文件替换或无法在本次启动确认目标版本时，一律进入 `failed_manual_repair_required`，不产生未定义的 `pending_system_restart` 或成功终态。
- 任何情况下不得把“安装器启动”报告为成功，崩溃重启后不能永久卡在更新状态。

### 18.6 性能、灾备和隐私

- 1,000 RPS 或预测峰值 3 倍压测达到性能目标，客户端抖动能消除同秒请求尖峰。
- 演练控制面 RTO/RPO、备用文件源、时间戳服务故障和发布签名服务故障；在数据库落后 5 分钟的故障切换中，从不可变对象/哈希账本恢复已发布最大版本并只签发更高版本，Snapshot/Timestamp 版本和当前世代零丢失、零复用。
- 新操作系统版本在目标注册表更新前返回 `unsupported_target`；完成真实机矩阵并更新注册表后才可支持，不能因版本字符串“更高”自动放行。
- 日志、事件、CDN 请求和导出报告扫描不到明文 SN、令牌、完整下载 URL、用户路径或业务内容。
- 留存到期任务、installId 重置和 SN 灰度 HMAC 访问审计均通过。
- 完整卸载删除本机 installationScopeId；服务端关联在 180 天无合法事件后删除或不可逆聚合，运营导出不可获得逐设备 ID，受限故障处置访问均有审计。

## 19. 实施阶段与闸门

### 19.1 阶段一 A：协议、安全与后台

- 冻结 OpenAPI、JSON Schema、JCS 测试向量、Root/Timestamp/Snapshot/Target/Release/Package 元数据、迁移切换状态和密钥运行手册。
- 实现顺序 Root 更新、不可变元数据获取/引用验证、复合包、增量非空解析和参考重建校验、五实体状态机、逐 Release Target 双人审核、SN 灰度、`supportFloorVersion`/最低来源版本/桥接选择、可达性矩阵和动态文件地址。
- 完成四类平台目标的真实性验证、SBOM、不可变存储及管理后台。

退出闸门：18.1～18.3 与 18.4A 的自动化验收通过，密钥轮换和服务端安全撤回演练通过；不要求真实安装器与助手端到端闭环在本阶段完成。

### 19.2 阶段一 B：可信全量升级与结果确认

- 实现完整包选择、断点续传、安装前授权、required 数据备份（含三平台 `pinvou-settings-v1` 成功路径）、三平台适配器、外部升级助手、健康观察、失败诊断和事件闭环。
- 实现基础制品缓存索引，但正式客户端增量能力数组保持为空。
- 构建唯一迁移引导版本；开发阶段可先用代表性版本冒烟，但阶段一 B 退出前必须通过原有升级系统完成清单中**每个受支持存量来源版本**到该版本的单跳升级测试，并验证成功后单向切换、失败不切换及新平台不可用时不回退。

退出闸门：18.4B、18.5、18.6 全部真实系统、迁移、离线补报与故障注入矩阵通过；无法在重启后确认安装结果或进入明确失败终态的目标不得进入 stable。

### 19.3 阶段二：受控启用增量

- 完成三平台真实压缩安装器的节省率、耗时、内存和故障技术验证。
- 分平台灰度开启 `artifact-delta-v1 + bsdiff-v1`，验证全量兜底和质量阈值。
- 只有达到至少 30% 传输节省且质量不低于完整包时才扩大范围。

### 19.4 阶段三：平台和制品扩展

- 增加经审核的其他增量算法、RPM/AppImage 等目标。
- 接入模型、插件、技能和连接器独立制品。
- 增加企业定向发布和另行设计、独立签名的离线升级方案。

## 20. 上线前业务确认项

下列事项不改变本需求契约，但须在进入阶段一 B 前落实责任人和实际资源：

1. Windows Authenticode 证书、Apple Developer ID/公证账号、Linux 签名身份及对应 KMS/HSM。
2. 升级后台、双故障域制品存储、第三方文件服务和 CDN 供应商。
3. 各角色人员、值班表、紧急撤回权限及事后复核流程。
4. 三平台可用的硬件 SN 读取接口、无 SN 时回到基线/桥接选择的行为和经隐私/法务确认的告知文本。
5. `migrationBootstrapVersion` 的具体版本号、各存量平台覆盖范围、原有升级系统迁移观察期、停止条件、目标迁移率、负责人及人工升级页面地址。
6. internal/beta 公开预览渠道的风险说明、用户同意文案、运营负责人及退出渠道提示。
7. 官方人工修复/下载页面、三平台支持手册、日志导出与隐私脱敏流程；首期不得以支持流程名义暗中引入自动回滚或恢复包。
8. 平台目标注册表负责人、操作系统新版本认证时限、停止支持通知机制和真实机资源。
9. 质量阈值与自动冻结扩量口径、人工暂停审批人、网络簇贡献上限、异常事件限流策略及事件凭据/本地队列的数据保护评审。

这些项目未落实时可以开发和测试，但不得进入 stable 生产发布。
