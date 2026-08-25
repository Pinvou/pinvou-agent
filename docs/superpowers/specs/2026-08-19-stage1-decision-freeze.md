# 阶段 1 决策冻结清单

> 状态：**已冻结（frozen）**——2026-08-19 完成逐条评审并经用户批准；D-04 按共享 `seglog` crate 定稿。此后变更须走决策变更评审，不得静默修改。
> 日期：2026-08-19（提案）→ 2026-08-19（评审冻结）
> 上游：`2026-08-18-pinvou-distributed-node-runtime-design.md`（下称"蓝图"）§26 把其中约 10 项阶段 1 阻塞参数列为"实施前冻结"。本文档把这些参数从"以后再说"提级为"现在定稿"。
> 依据：本机实测（HP Z1 Tower G5 / i7-8700 / Win10 企业版 / KIOXIA NVMe SSD，见延迟 spike 文档）+ codex-cli 0.139.0 第一方协议 Schema（见 Codex Adapter 合同文档）。

## 决策总表

| # | 决策 | 定稿值 | 状态 |
|---|---|---|---|
| D-01 | 原生平台优先顺序 | Windows 参考平台 → Linux（CI）→ macOS（不承诺）；阶段 2 与阶段 3 插入 Linux 延迟冒烟 | 冻结 |
| D-02 | 数据根目录 | Windows `%LOCALAPPDATA%\pinvou\`（数据+配置）；Linux XDG；与 `~/.pinvou3` 完全隔离 | 冻结 |
| D-03 | 本地 IPC 传输与帧协议 | Named Pipe / UDS + u32 长度前缀 JSON 帧；帧上限 16MB | 冻结 |
| D-04 | 存储技术选型 | 共享 `seglog` crate（段机制）+ spool/WAL 薄封装；元数据与 Session Store 用 SQLite | 冻结 |
| D-05 | durable barrier 策略 | R0 专用 5ms/16 事件 group commit；R1 挂 50ms 合并批 | 冻结 |
| D-06 | 文本合并窗口默认值 | 50ms（非 100ms），spike 后复核 | 冻结 |
| D-07 | Codex 官方接口 | `codex app-server`（stdio JSON-RPC）为主；ACP 为降级备选；experimental 标签显式接受 | 冻结 |
| D-08 | Controller daemon 生命周期 | `pinvou` 按需 detach 拉起；默认常驻；不注册登录自启；崩溃由客户端驱动重生 | 冻结 |
| D-09 | 跨进程单调时钟 | Windows QPC / Linux CLOCK_MONOTONIC；同 boot 内有效；阶段 4 跨机须分段测量 | 冻结 |
| D-10 | 性能门禁参考终端 | Windows Terminal；conhost 尽力支持 | 冻结 |
| D-11 | 阶段 1 Node 网络面 | 无 TCP 监听、无端口、无发现；仅本地 IPC | 冻结 |
| D-12 | 密钥存储 | OS 凭据存储（keyring crate）；headless Linux 兜底登记为阶段 4 前置决策 | 冻结 |
| D-13 | CLI 退出码表 | 0/1/2/3/4/5/6/7/8 九类；因果链最前置者优先 | 冻结 |
| D-14 | 依赖基线 | tokio/serde/clap/tracing/keyring/rusqlite/windows-sys；网络栈推迟阶段 4 | 冻结 |

---

## D-01 原生平台优先顺序

- **候选**：a) Windows 先；b) 三平台同步；c) Linux 先（CI 友好）。
- **决策**：**Windows 为参考平台**——性能门禁（蓝图 §13.4.1）的基准数据在 Windows 上采集即为有效；Linux 第二（CI 容器内跑合同测试与吞吐压测）；macOS 阶段 1 不承诺，只保证代码无 `cfg` 阻塞。
- **理由**：开发主力环境与桌面产品主环境均为 Windows；Windows 恰是 fsync 语义（`FlushFileBuffers`，无 `fdatasync`）与终端（conhost/WT 双形态）差异最大、最需要先证伪的平台。
- **影响**：阶段 1 验收报告只需 Windows 基准；CI 的 Linux job 只跑合同/吞吐，不跑延迟门禁。
- **注记（评审新增）**：Windows-only 门禁使 **Linux 延迟特性（ext4 fsync、UDS）到阶段 4 才首次暴露**，而阶段 4 的 Remote Node 主场景恰含 Linux/WSL/Docker。缓解：实施规格 M3 出口增加 Linux 延迟冒烟（不设门禁数值，只验证"无数量级意外"）。

## D-02 数据根目录

- **候选**：a) `~/.pinvou`（跨平台统一）；b) 各 OS 惯例目录；c) 复用 `~/.pinvou3`。
- **决策**：**b**，且与 Desktop 数据物理隔离（蓝图 §6.5 硬约束）。**修订（评审定稿）：配置与数据统一放机器本地目录**，避免"配置漫游、身份留原地"的半机语义：

```text
Windows:
  数据 + 配置               %LOCALAPPDATA%\pinvou\
  运行时 socket/pipe 名     \\.\pipe\pinvou-controller-<logon-sid-hash>
Linux:
  数据                      ~/.local/share/pinvou/
  配置                      ~/.local/share/pinvou/config/     # 同根，保持机器语义一致
  运行时 socket             $XDG_RUNTIME_DIR/pinvou/controller.sock
macOS:
  数据                      ~/Library/Application Support/pinvou/
```

- **理由**：spool/WAL 是机器本地、可再生的数据；Controller/Node 身份密钥在 OS 凭据存储（机器本地），配置若走 `%APPDATA%` 漫游会产生"配置跟人走、身份留原地"的不一致——干脆全机器本地，语义自洽。复用 `~/.pinvou3` 直接违反蓝图 §6.5。
- **注记（评审新增）**：隔离不仅靠约定——增加一条 T0 运行时断言测试：新子图代码运行时的全部文件访问路径不包含 `.pinvou3`。

## D-03 本地 IPC 传输与帧协议

- **候选**：a) Named Pipe/UDS + 长度前缀 JSON；b) gRPC/tonic；c) JSON Lines（行分隔）。
- **决策**：**a**。
  - 传输：Windows Named Pipe 使用当前 logon session 的自定义 DACL、`PIPE_REJECT_REMOTE_CLIENTS`、`FILE_FLAG_FIRST_PIPE_INSTANCE`，连接后校验客户端身份；名称含 logon SID 或等价会话标识。普通 user SID 只能区分用户，不能防同一用户跨登录会话碰撞。Unix 使用 pathname UDS，父目录 0700、socket 0600，连接后校验 `SO_PEERCRED` 或平台等价 peer credential；禁止 Linux abstract namespace。
  - 帧协议：`u32 LE 长度前缀 + UTF-8 JSON`。行分隔（c）被否决：事件 payload 含换行时脆弱，且无法做二进制预留。gRPC（b）被否决：引入代码生成链与第二套类型系统，违反"薄 IPC"原则。
  - **帧上限（评审新增）：单帧 16MB**。声明长度超限 → 返回协议错误并**断连该客户端**，daemon 不尝试分配缓冲。16MB 覆盖事件合并批与 ResourceRef 元数据场景（大二进制本就不进事件流，蓝图 §3）。超限行为进 T0 合同测试（畸形帧不 panic、不 OOM）。
  - 消息信封：`{v:1, id?, kind: req|rsp|evt|ack|err, method|topic, payload}`；请求/响应/事件订阅在同一连接多路复用，订阅带 cursor（蓝图 §13.1 `subscribe(cursor, filter)`）。
  - 实例挑战：客户端连接后首条消息为 `{kind:"hello", protocol_version, client_info}`；daemon 回 `{instance_id, protocol_version}`；版本不匹配返回稳定错误码（见 D-13）。
- **影响**：`protocol` crate 拥有帧编解码与信封类型的黄金合同测试。

## D-04 存储技术选型

- **候选**：a) 全自研两套（controller WAL、node spool 各自实现）；b) SQLite 全包；c) redb；d) 共享 `seglog` crate + 薄封装。
- **决策**：**d**，并配 SQLite：
  - **`seglog` 共享 crate（新增于 workspace）**：提供"单流 append-only 段日志"原语——记录级 CRC、批量 fsync durable barrier、崩溃恢复（读到第一个坏记录为止）、游标推进、按确认区间截断回收。
  - **Node spool** = 阶段 1 至少为 `control`（R0）和 `main`（R1–R3）各建一个 seglog 实例；每流独立 raw `source_seq`/durable/transport-sent/ACK 水位、持久 `transport seq -> source_span` 映射与配额，R0 紧急段只属于 control。传输序号在所属流内于合并/降级后连续分配，详见事件 schema v1 §2。
  - **Controller WAL** = 单个 seglog 实例 + group commit（5ms/16）+ 携带每流独立水位的 BatchAck + `(node,att,stream,seq)` 去重（策略薄封装，controller crate 内）。单一 WAL 物理日志不等于单一传输序列。
  - **Controller/Node 元数据与 Session Store：SQLite（rusqlite bundled）**，WAL 模式、单写连接。理由：蓝图 §13.1 要求 single-writer-multi-reader 快照读——SQLite WAL 模式是该并发模型的成熟实现；可查询性对调试与未来 `pinvou data` 工具链有实际价值。redb 被否决：查询能力弱、快照读语义不如 SQLite WAL 明确。
- **理由（共享 vs 各自）**：spool 与 WAL 是同一抽象的两个真实调用方（蓝图 §6 提升规则正好满足）；持久化（半条记录、fsync 语义、恢复）是最易出隐蔽 bug 的领域，共享实现使"修一次 bug 两处受益"，迁移策略文档的 L2 段格式与合同测试只维护一份。分叉点（raw/transport 水位与 source span 映射 vs BatchAck）天然属于上层策略，不会倒灌共享层。
- **红线**：`seglog` 只做"追加 + CRC + barrier + 游标 + 恢复"，**不做索引、不做查询、不做页管理**。出现向 seglog 添加"智能"功能的提议即视为抽象癌变信号，须重新评审。若未来证明抽象错误，拆散路径明确：上层薄封装接口不动，seglog 复制两份分别演化。

## D-05 durable barrier 策略

- **候选**：每事件独立 fsync / 纯批量 / 分级批量。
- **决策**：**分级批量**（与蓝图 §13.4 一致，数值定稿）：
  - R0：独立 `control` stream + 专用 group commit 窗口 **5ms / 16 事件上限**（蓝图默认值确认）；`main` 的积压、缺口或 ACK 停滞不得阻止 control 发送与 ACK 推进。
  - R1：同一个 **50ms batch 窗口**收集并追加原始记录，窗口到期/批上限时先完成 spool durable barrier，再立即对该 durable source span 合并并分配传输序号；不是“先等 50ms durable、再额外等 50ms 合并”。barrier 前崩溃造成的内存尾部必须通过 active Turn journal 暴露为显式 gap，不能宣称历史完整。
  - R2/R3：独立有界队列，允许丢弃/截断（R3）。
  - Windows 用 `File::sync_all()`（= FlushFileBuffers）；无 `fdatasync` 的差异记录进基准报告。
- **依据**（本机实测，详见 spike 文档）：逐事件 fsync p50≈0.8–0.9ms、p95≈1.3–1.8ms；16 事件批量后**每事件成本降至 ~57–60µs（p95 69–87µs），降为 1/14**；双屏障串行（spool+WAL 同盘）p95≈2.7–2.8ms。批量化是吞吐与延迟双赢的前提。

## D-06 文本合并窗口默认值

- **候选**：25ms / 50ms / 100ms。
- **决策**：**50ms**，配置项 `events.text_merge_window_ms`。
- **理由**：门禁是 `event-to-screen p95 ≤ 100ms`，合并窗口是预算中最大的单一分项，取 100ms 会让 p95 必然贴边；取 50ms 留出双 fsync（~3ms）+ IPC（~0.1ms）+ 投影批 + 终端 flush 的余量。首 token 最多延迟 50ms，在人类"即时感"阈值（~100ms）内。spike S2（真实 codex 事件形态）完成后复核。
- **影响**：蓝图 §26"文本合并窗口准确默认值"项就此关闭。50ms 是传输合并窗口，不授权在可恢复 spool 之前保留不可观测的静默丢失窗口。

## D-07 Codex 官方接口

- **候选**：a) `codex app-server`（stdio JSON-RPC）；b) ACP（`@agentclientprotocol/codex-acp` npm 适配器，桌面 codex_acp 现役方案）。
- **决策**：**a 为主，b 为记录在案的降级备选**。
- **依据**（第一方事实，非推测）：本机 codex-cli 0.139.0 提供 `codex app-server generate-json-schema`，已生成包含顶层聚合定义与 v1/v2 版本目录的官方协议 Schema；文件总数会随生成器扩展，不作为合同。Schema 证实：
  - 生命周期完整：`initialize`、`thread/start|resume|fork|read|list|inject_items|compact/start`、`turn/start|steer|interrupt`；
  - 审批为 server→client 请求（`item/commandExecution/requestApproval`、`item/fileChange/requestApproval`、`item/permissions/requestApproval`、`item/tool/requestUserInput` 等 10 个）——可编程应答，比 ACP 审批语义更完整；
  - 存在流式通知（`item/agentMessage/delta`、`item/reasoning/textDelta`、`turn/diff/updated`、`thread/tokenUsage/updated` 等）；通知总数不作为固定兼容假设；
  - `account/login/start|cancel|logout`、`account/read` 存在——登录态可经协议查询；
  - steer（`turn/steer`）原生存在。
- **显式接受的风险（评审新增）**：`codex app-server` 在上游标注 **[experimental]**（`--help` 原文）。为产品功能钉住 experimental 接口意味着破坏性变更概率高于稳定接口——该风险**显式接受**，缓解为版本钉住（兼容矩阵 `>=0.139.0, <0.150.0`）+ ACP 降级路径。
- **ACP 备选的成本注记（评审新增）**：ACP 路线依赖 npm 适配器（`@agentclientprotocol/codex-acp`），意味着 Node 侧需要 node 运行时——对阶段 4 远端 Node 部署是实际成本，也是坚持 app-server（自包含二进制）为主的又一层理由。
- **影响**：蓝图 §10.3"Codex 可优先使用 app-server"从倾向变为定稿；兼容版本策略见 Codex Adapter 合同文档。

## D-08 Controller daemon 生命周期

- **候选**：a) 登录自启常驻；b) CLI 按需拉起 + 常驻；c) 每命令即启即停。
- **决策**：**b**：
  - 首次需要主控能力的 `pinvou` 命令时，CLI 以 detach 方式拉起 `pinvou-controller`（Windows：`CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS`；Unix：`fork + setsid`），daemon 持 OS 级单实例锁（蓝图 §13.1）。
  - 默认常驻不退出；`controller.idle_exit_secs`（默认 0=不退出）可配置。
  - **不注册登录自启**（蓝图 §26 该项推迟到阶段 4 前评估）；阶段 1 也不注册 Node 后台服务——私有 Node 由 Controller 进程内监督（LocalNodeSupervisor）。
  - TUI/CLI 退出不影响 daemon（蓝图 §6.4）。
- **注记（评审新增）**：
  - **崩溃重生归属 = 客户端驱动**：Controller 崩溃后由下一个 `pinvou` 命令/TUI 重连失败时重新拉起；单实例锁消解多客户端并发重生的竞态。不存在系统级自愈，文档与实现不得假设。
  - **存活判断用 OS 锁句柄，不用 pid 文件数值比较**（pid 会被系统复用产生误判）；pid 文件仅作诊断信息保留。

## D-09 跨进程单调时钟

- **候选**：a) `std::time::Instant`；b) 平台 API（QPC / CLOCK_MONOTONIC）。
- **决策**：**b**。Rust `Instant` 不保证跨进程可比；Windows `QueryPerformanceCounter` 系统级单调、同 boot 跨进程可比，Linux `CLOCK_MONOTONIC` 同理。
- **规则**（蓝图 §13.4.1 的落地）：t0–t4 打点全部经 `HostMonotonicClock`；仅同次运行、同 boot 内有效；不持久化、不跨重启比较、不作业务时间。
- **注记（评审新增）**：本规则在阶段 4 **天然失效**——t0/t1 在远端 Node、t4 在主控，跨机时钟不可比。阶段 4 的延迟测量必须改为分段模型：每段在单机内用本时钟计算，跨机段（t1→t2）用传输层 RTT 表达，或显式声明 NTP 同步假设并标注误差界。禁止任何跨机时间戳直接相减。
- **影响**：`protocol` crate 提供 `HostMonotonicClock` 封装（Windows 用 `windows-sys`，Linux 直接 `clock_gettime`）。

## D-10 性能门禁参考终端

- **决策**：延迟门禁（p95≤100ms）的基准数据在 **Windows Terminal** 中采集；报告记录终端版本。conhost 尽力支持（中文宽字符路径重点测试），但不作为门禁环境。
- **理由**：t4 是 write+flush 返回——写向真实控制台句柄时 flush 耗时取决于终端消费速率，conhost 与 WT 差异可达数量级；门禁必须固定终端形态，否则 t4 段噪声淹没架构信号。

## D-11 阶段 1 Node 网络面

- **决策**：阶段 1 私有 Node **无任何 TCP 监听、无 mDNS、无端口分配**，仅经本地 IPC 供 Controller 使用。蓝图 §26"Node 默认端口与端口冲突处理"整体推迟到阶段 4 冻结。
- **理由**：阶段 1 无远程场景；零网络面同时简化威胁模型（见威胁模型文档"阶段 1 范围"）。

## D-12 密钥存储

- **决策**：`keyring` crate（Windows Credential Manager / macOS Keychain / Linux libsecret）。阶段 1 唯一密钥是 Controller/Node 自身身份私钥；凭据文件兜底方案不做（失败即显式错误）。
- **理由**：威胁模型要求身份私钥不落明文文件；阶段 1 密钥种类少，keyring 足够。
- **前置警告（评审新增）**：**headless Linux（服务器/Docker Node）没有 secret service，keyring 不可用**。阶段 4 引入 Linux/容器 Node 前必须补一条兜底决策（候选：`0600` 权限文件 + 显式风险声明，或环境注入）。登记为阶段 4 前置决策，不阻塞阶段 1（Windows 参考平台）。

## D-13 CLI 退出码表

| 码 | 含义 |
|---|---|
| 0 | 成功 |
| 1 | **未分类内部错误（兜底——保证任何错误路径都非零退出）** |
| 2 | usage 错误（未知子命令、缺参数、非 TTY 误入 TUI 路径） |
| 3 | Controller 不可达 / IPC 版本不匹配 / 单实例冲突 |
| 4 | Runtime `blocked_auth`（第三方 Agent 登录失效） |
| 5 | Runtime 执行失败（进程退出、协议错误） |
| 6 | 用户取消 / 超时 |
| 7 | spool/资源耗尽（R0/R1 无法保全） |
| 8 | 数据损坏（store/spool/WAL 校验失败） |

- **优先级规则（评审新增）**：多条件共存时**报因果链最前置的错误**——Controller 不可达（3）优先于 auth 失效（4），因为前者存在时后者不可知。
- **影响**：进入合同测试（含"未映射错误必须落 1"的兜底断言）；蓝图 §6.1"稳定退出码"项就此落稿。

## D-14 依赖基线

```text
tokio（rt-multi-thread, process, io-util, net, sync, time, macros）
serde / serde_json
thiserror / anyhow（边界内）
clap v4（derive）
tracing + tracing-appender（daemon 文件日志，滚动默认 50MB×5 份）
keyring
rusqlite（bundled, WAL 模式）
windows-sys（Named Pipe / QPC / DACL；仅 cfg(windows)）
```

- JSON-RPC 由 Adapter 手写实现（帧格式简单且有捕获 fixture 锁定，不引 jsonrpc 库）。
- 网络栈（rustls/quinn 等）**推迟到阶段 4**（D-11 无网络面）。全部依赖为 MIT/Apache-2.0/ISC 系许可，满足 §24.4 SBOM 要求。
- CI 依赖图守卫（蓝图 §6.5）禁止：`tauri*`、`pinvou3-app`、`codewhale-*`、`pinvou-product-backend` 出现在新 crate（含 `seglog`）依赖中。

---

## 与蓝图 §26 的勾稽

本表关闭 §26 中以下条目：平台优先顺序、本地 IPC 细节（部分——peer credential 精确校验仍待实现前冻结）、文本合并窗口、Codex 官方接口（合同细节见 Adapter 合同文档）、Windows 后台启动机制（阶段 1 形态）、Controller daemon 自启动策略、Node 默认端口（推迟阶段 4）、新 CLI 数据根位置。**spool 初始限额与 WAL 分段大小在有效 S2 给出事件形态后、PR#5 前采用保守值冻结；S3 使用真实实现校准这些值，并可在 M4 前走显式决策变更**。不能要求先完成依赖 spool 实现的 S3 再开始实现 spool。

## 变更记录

| 日期 | 变更 |
|---|---|
| 2026-08-19 | 初版提案（D-01..D-14） |
| 2026-08-19 | 评审冻结：D-02 配置改机器本地目录；D-03 增帧上限 16MB；D-04 定稿共享 seglog crate（含红线）；D-07 增 experimental 风险显式接受与 ACP 成本注记；D-08 增客户端驱动重生与 OS 锁句柄规则；D-09 增阶段 4 跨机分段测量约束；D-12 增 headless Linux 前置警告；D-13 增退出码 1 与优先级规则；D-01/D-02 增注记 |
| 2026-08-19 | 合同一致性修约：不改变 D-04/D-05/D-06 数值，明确 raw spool durable 先于传输合并、传输 seq 与 source span 分离，避免“合并前静默丢失”与累计 ACK 缺口 |
| 2026-08-19 | 外部验证修约：阶段 1 拆分 control/main 独立 seq/ACK；D-03 精确冻结 UDS 权限/peer credential 与 Named Pipe first-instance/logon SID；不修改主工程 |
