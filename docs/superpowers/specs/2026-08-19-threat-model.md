# 威胁模型与安全决策

> 状态：草案（draft）——两个显式决策（§5）需用户确认。日期：2026-08-19。
> 上游：蓝图 §7（身份与配对）、§11（凭据）、§13.1（IPC 单写）、§18（权限与安全）。
> 定位：蓝图 §18 是规则清单；本文档补齐威胁建模（谁攻击什么、现有控制、残余风险），并显式声明 accepted risks。

## 1. 阶段 1 范围声明

阶段 1 **无网络面**（决策冻结 D-11）：无 TCP 监听、无配对、无远程身份。本文档对阶段 1 只约束本地 IPC 硬化（§4）；§2/§3 的远程威胁模型为阶段 4 预冻结**不变量**——阶段 4 实施时不得推翻本文档已声明的 accepted risks，若需推翻必须重新评审。

## 2. 资产与攻击者

### 资产

| # | 资产 | 权威位置 |
|---|---|---|
| A1 | Controller Owner 授权根 + 可轮换 device/transport credential | OS 凭据存储（D-12） |
| A2 | Node 身份私钥 / OwnerBinding | Node 端 OS 凭据存储 + Node Store |
| A3 | Logical Session / 事件历史 | Controller Store |
| A4 | 第三方 Agent 凭据（codex 等） | Agent CLI 官方存储（Node 本机） |
| A5 | 工作目录文件 | Node 文件系统 |
| A6 | spool/WAL 残留数据 | Node/Controller 数据根 |

### 攻击者（能力从弱到强）

| # | 攻击者 | 能力 |
|---|---|---|
| T1 | 同网段被动观察者 | 抓包 |
| T2 | 同网段主动攻击者 | 抓包、注入、伪造 mDNS 广播、轰炸配对请求、DoS |
| T3 | Node 上的同机低权限用户 | 读文件系统可读部分、尝试连接 IPC |
| T4 | Node 上的同用户恶意进程 | 以运行 Pinvou/Agent CLI 的同一用户身份执行任意代码；视为该用户会话已失陷 |
| T5 | **Controller 机器失窃/被入侵** | 持有 A1，可远程连接已配对 Node |
| T6 | 配对前的中间人 | 篡改首次连接握手 |
| T7 | 物理接触 Node 的人 | Node 本机任意操作 |

## 3. 威胁矩阵（阶段 4 不变量）

| 威胁 | 攻击 | 现有控制（蓝图条款） | 残余风险 |
|---|---|---|---|
| W1 | T1/T2 窃听流量 | TLS 1.3 + 配对后公钥固定（§8） | 仍存在流量分析与 DoS；不泄露明文内容 |
| W2 | T2 伪造 Node 诱骗连接 | Node 身份密钥签名 + fingerprint 双侧展示（§7.3） | 人工核对错误与实现缺陷仍是残余风险 |
| W3 | T2 轰炸配对请求 | TTL、尝试上限、限速、指数退避（§7.3）；逐请求审批（§7.3） | mDNS 垃圾广播占带宽（接受，阶段 4 评估） |
| W4 | T2 冒充已配对 Controller | OwnerBinding 授权根 + 有 generation 的可轮换 transport credential（§7.4） | transport credential 失窃可吊销；Owner 授权根失窃转入 W5 |
| W5 | **T5 持被盗 Owner 授权根永久控制 Node** | transport credential 短期化/轮换不能撤销授权根 | **见 §5 决策 A（显式接受）** |
| W6 | T6 首次配对 MITM | 验证码 + Node 本机审批 + fingerprint 人工比对（§7.3） | fingerprint 被人肉看错（低，接受） |
| W7 | T3 读 spool/Store/日志 | 数据根目录 ACL 当前用户（D-02）；日志脱敏（§18） | ACL 不抵御 T4；备份、崩溃转储和临时文件仍需审计 |
| W8 | T3/T4 连接本地 IPC | Named Pipe 自定义 DACL + `PIPE_REJECT_REMOTE_CLIENTS` + `FILE_FLAG_FIRST_PIPE_INSTANCE` + logon SID/客户端身份校验；Unix pathname UDS 位于 0700 目录、socket 0600 并校验 peer credentials | 可阻止其他低权限用户 T3，但不能阻止同用户 T4；T4 视为用户会话完全失陷 |
| W9 | T4 读取 A4（codex 凭据） | Codex 官方存储自身 ACL；Pinvou 不复制凭据 | 同用户 T4 通常可借用户权限访问或调用凭据，接受为会话失陷后果 |
| W10 | T4 伪造 node/controller 进程 | 单实例锁 + instance_id 可发现误连、旧实例和并发启动 | instance_id 不是秘密，不能认证同用户进程；T4 可抢占名称或模拟协议，接受为会话失陷后果 |
| W11 | T7 在 Node 本机作恶 | 超出威胁模型——本机物理接触即完全失守 | 明确声明：物理接触 Node = 游戏结束（所有本地系统同此假设） |

## 4. 阶段 1 本地 IPC 硬化清单（实现检查表）

- [ ] Windows Named Pipe：自定义 DACL 限当前 logon session；`PIPE_REJECT_REMOTE_CLIENTS`；`FILE_FLAG_FIRST_PIPE_INSTANCE`；连接后校验客户端身份。普通 user SID 只能区分用户，不能防同一用户跨登录会话碰撞，需要 logon SID 或等价会话标识。
- [ ] Unix pathname UDS：父目录 0700、socket 0600，连接后使用 `SO_PEERCRED` 或平台等价机制校验 peer；不使用缺少 pathname 权限语义的 Linux abstract namespace。
- [ ] 实例挑战：客户端首条 hello → daemon 返回 instance_id；后续命令校验；用于发现旧实例、误连和并发启动，不宣称防御同用户 T4。
- [ ] 单实例锁：`controller.pid` + OS 级锁（蓝图 §13.1）；第二实例必须连接或失败，不并行打开 Store。
- [ ] 日志脱敏过滤器：Token / Cookie / Authorization / api_key / `chatgpt_access_token` 模式（§18）；违规进 T0 合同测试。
- [ ] 数据根 ACL：`%LOCALAPPDATA%\pinvou\` 显式设置当前用户（不依赖继承）。
- [ ] A1/A2 私钥只进 OS 凭据存储（D-12），任何文件落盘路径在代码评审中视为缺陷；OS 凭据存储降低静态文件泄露，不构成同用户恶意代码隔离边界。

阶段 1 的本地安全边界明确到“不同 OS 用户/远程客户端”。若攻击者已能以 Pinvou 所在用户执行任意代码，Controller Identity、Node 管理 IPC、Agent 凭据和 Workspace 均按该用户会话已失陷处理；文档、UI 和测试不得把 DACL、instance_id 或 keyring 描述为对此场景的完整防御。

## 5. 两个显式决策（需确认）

### 决策 A：Owner 授权根失窃 = 永久远程全控，接受；普通传输凭据可轮换

- **事实**：蓝图 §7.4 规定 Owner 绑定不自动过期；Node 侧撤销 Owner 授权根的最终手段是本机 `pinvou node release`。普通 device/transport credential 与 OwnerBinding 分离，允许在已认证状态下按 generation 吊销和轮换；只有授权根失窃才形成 T5 的永久控制。
- **选项**：a) 接受（蓝图现状）；b) 定期 presence 确认（软 lease）；c) 双因素 release。
- **推荐：a）接受授权根风险，同时强制传输凭据轮换**。自动过期 OwnerBinding 与“永久独占、断网不释放”冲突，但这不要求长期复用同一个网络认证凭据。缓解措施：授权根进入 OS 凭据存储；传输凭据短期化或带 generation；支持吊销旧设备凭据；文档向用户明示 Controller 授权根丢失时应尽快在各 Node 本机执行 release。
- **影响**：写入用户文档安全章节；阶段 4 配对 UI 展示此风险提示。

### 决策 B：v1 单 Controller 身份，不支持导出/同步

- **事实**：蓝图全文假设单一 Controller；第二台管理机无法共享 Owner 身份——需先在 Node 本机 release 再与新机器重新配对。
- **选项**：a) v1 单机身份；b) 身份可导出导入；c) 多 Controller 白名单。
- **推荐：a）**，理由：b 让 A1 离开硬件保护边界、直接放大 W5；c 引入分权与冲突语义（谁写 Store），是一套新的分布式问题。当前无多管理机需求证据；出现真实需求（且决策 A 的痛点被证实）时再设计 c。
- **影响**：蓝图 §26 增补"多管理机：v1 明确不支持"；release/重配对流程必须在用户文档写清。

## 6. 审计与可验证性

- 配对审计（蓝图 §7.3）落 Node Store：request_id、fingerprint、结果、时间、本地确认主体；不含验证码与密钥材料。
- 阶段 4 前补：威胁矩阵评审会（本 §3 表逐行过）、W3 的 mDNS 垃圾广播实测、Windows 管道 DACL 的渗透用例进 T1 测试。
