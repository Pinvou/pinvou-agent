# PinvouOS：资源治理与 Host Supervisor

## 状态

已接受；当前工作树已实现 schema v6 HostWork 控制面、首版 Linux
Supervisor、显式 MegaBook profile helper 与固定 E2E harness，但尚未执行 MegaBook
真实安装 / High / OOM / 卸载 E2E，不能把脚本存在写成实机通过。

本文是 PinvouOS 资源治理、Host 工作控制和 Linux Supervisor 的唯一权威。它延续
ADR-0007 的原则：Resource Agent 只观察并提交 Claim，确定性 Governor 决策，
受信 Host 执行控制。ADR-0007 的历史决策保持不变；本文补齐此前没有定义的
“工作如何注册、谁能停止、外部进程由谁持有、失败如何留证与恢复”。

当前工作树中，Resource Agent 仍只观察：它每 5 秒采样资源，在全机压力变化、
30 秒心跳、app cgroup 治理证据变化或一次有资格的 fresh retry evidence 时写入 Runtime Ledger；
Governor 只根据固定规则签发 `Pending`
Directive。Runtime 已以 opaque `work_id + generation` 注册 HostWork，以
`directive_id` 做单飞行与幂等关联，并由各 Adapter 的独立异步 worker 执行、ACK 和
reconcile。生产组合根已装配 6 个静态 Adapter：scheduled、knowledge、编译期
固定 connector、受限 detached sub-agent、ASR cgroup Stop，以及
`essential + non-governable` 的 app cgroup status-only 观测。

这不等于“任何 PinvouOS 工作都可停”。旧 Mission 同步 callback 生产面已移除，
Mission Adapter 数量为 0；前台 turn、任意 managed child 和 app/WebKit 自停不在
当前 Governor 动作面。“已观测 Critical”只是治理输入，不是已停止证据；只有
匹配 Directive 的 Adapter / Supervisor ACK 与后验观测才能改变 observed state。

## 事故边界

本决策直接回应 2026-08-19 MegaBook 的 WebProcess 内存事故：一段 135,770 字符回答被
拆成 63,927 个 delta，旧前端对每个 delta 反复做累计全文 Markdown 和全 state clone；
JavaScriptCore 在 22,213 MB 时杀死 WebProcess，旧开发服务记录的瞬时 MemoryPeak 为
24.9 GiB。这条事故证明应用内观测不能替代有界流式路径与 cgroup 最后防线；它不是
kernel OOM。

2026-08-20 的整机硬冻结是另一条事件：现有日志只有非正常重启，没有 OOM、GPU reset、
panic、hung task 或 lockup 证据。本文不把它归因于内存，也不把 Supervisor 描述成已经
能修复未知的整机冻结；后续仍需外部心跳和独立取证。

## 第一性事实

1. 监控者与被监控应用同进程时，应用卡死或被 OOM 杀死会同时失去监控者；它不能成为
   最后的资源边界。
2. Linux PID、systemd unit 名和 shell 命令都是环境可变字符串。让模型或 Renderer
   直接提交这些字符串，相当于开放任意进程控制。
3. “发出了停止请求”不等于“工作已经停止”。只有受信执行器返回的状态回执和后续
   观测才能改变 observed state。
4. 一个 Linux 后台工作不一定是 Agent。把编译、WebKit、ASR 或文件索引伪装成
   Mission Agent，会污染 Agent 领域模型和能力目录。
5. 暂停、停止和恢复的安全性取决于工作自己的中断语义。`Atomic` 工作不能在任意位置
   暂停；无法恢复的工作不能承诺 Resume。
6. 突发内存膨胀可能快于 5 秒采样，也可能在全机使用率达到 97% 前杀死 WebProcess。
   主动治理必须与 cgroup 硬上限并存，不能只依赖应用内阈值。
7. 同 UID Unix socket、peer credential 和 PID 相互校验能缩小误调用面，但不是对
   已获得同一 Unix 用户权限的恶意 shell 的强隔离或多租户安全边界。

## 决策

### Resource Agent 继续只观察

Resource Agent 只负责：

- 从可信平台探针形成 `ResourceObservation`；
- 标注观测健康、缺失传感器和证据时间；
- 形成可撤回的资源压力 Claim；
- 向 Governor 提交治理候选。

它不持有进程句柄，不调用 systemd，不选择任意 PID，不直接改变 desired/observed state，
也不获得模型可调用的停止工具。其 `pinvou-resource` Skill 保持只读。

GPU 是可选证据，不能成为 RAM / cgroup 治理采样的串行前置条件。GPU cache
只启动一个异步刷新，任何 GPU I/O 都不持 cache 锁；采样调用立即返回上一张
有效值或 `None`。NVIDIA `nvidia-smi` 的硬超时是 2 秒，macOS `ioreg` 是
5 秒，Windows 平台探针是 15 秒；某个 GPU 探针卡死不再阻塞 RAM 或之后的
cgroup `try_read`，5 秒采样节奏保持不变。

### Governor 只签确定性 Directive

Governor 根据固定 Policy、工作优先级、中断级别和最新可信观测签发 Directive。第一版
保持以下策略：

- `Normal`：不限制；仅恢复此前由同一 Governor 暂停且声明可恢复的工作；
- `Warm`：拒绝新的可选 Heavy 工作，不中断已运行工作；
- `Hot`：暂停低优先级、可中断且声明可恢复的工作；不能暂停时保持运行并记录原因；
- `Critical`：停止已注册、非必要、可治理的后台工作；不得把未知工作当成已停止。

当前生产 HostWork spec 只有 Stop 和 status-only，没有任何 Pause / Resume Adapter；
因此 Warm / Hot / Normal 的上述策略不会虚构当前 Adapter 不声明的动作。

Critical 对明确标记为 `governable + nonessential` 的工作是预授权安全动作，不等待模型
确认。对必要工作、系统级服务、未知来源或超出白名单的目标，必须拒绝或进入独立的
Host/用户授权流程。模型不能覆盖拒绝。

### HostWork 是独立领域实体

新增 `HostWork`，不复用 `AgentManifest`：

```text
HostWork {
  work_id, generation, owner, kind,
  resource_class, priority, interruptibility,
  essential, governable,
  supported_actions,
  desired_state, observed_state,
  registered_at, last_observed_at
}
```

- `work_id` 由可信宿主生成，是不透明标识；调用方不能自选 PID、unit 或命令作为 id；
- `generation` 防止旧 Directive 控制同名工作的新实例；
- `kind` 是编译期封闭枚举，包含 Engine turn、detached sub-agent、scheduled run、
  knowledge job、connector job、managed child process、app cgroup 与 ASR cgroup；枚举成员
  存在不代表生产组合根已为它注册 Adapter；
- `supported_actions` 由 Adapter 声明，不能从 UI 或模型输入推导；
- PID、cgroup 路径、systemd unit 和子进程句柄只存在于受信 Adapter 内部，不进入
  Renderer、模型参数或长期 Capability Catalog。

HostWork Registry 只接受组合根和受信 feature 代码注册。远控、Web、MCP、A2UI 和模型
工具没有注册任意 HostWork 的写入口。

### 控制由窄 Adapter 执行

每个 HostWork Adapter 只实现其声明的动作。概念接口为：

```text
status(work_id, generation)
pause(work_id, generation, directive_id)
stop(work_id, generation, directive_id)
resume(work_id, generation, directive_id)
```

`directive_id` 同时是幂等 request id。Adapter 不接受额外命令字符串。Governor 在
Runtime 的串行 append 路径上只签发 `Pending`，绝不在该锁内等待 systemd 或进程。
每个生产 Adapter 拥有独立异步 worker；一个 worker 慢或超时不阻塞 Resource Agent
采样、Runtime append 或其他 Adapter。worker 在任何 Adapter 副作用前先 append
`HostWorkDirectiveDispatchRecorded`，并完成 flush + `sync_data`；该 marker 只证明同一
directive 已进入“可能执行”的 attempt window，不证明动作成功。已有 marker 的 Pending，
或 prior-boot 遗留且没有 marker、因而仍无法证明未执行的 Pending，都只以同一
`directive_id` 进入 `OutcomeUnknown` / status-only 对账，绝不重放副作用。结构化 ACK
之后仍必须做 status reconcile。

当前生产组合根的 6 个静态 Adapter 为：

- Scheduled aggregate：只声明 `Stop`，取消 Scheduler 自有的 queued / running run；
- Detached sub-agent aggregate：只声明 `Stop`，且只对 mailbox 仍存活、root turn
  已终态、session 空闲的 detached sub-agent 发 `CancelSubAgents`。共享同一
  Engine 的前台 turn 存活时返回 Unknown / deferred，不取消 root 或前台 turn；
- Knowledge aggregate：只声明 `Stop`，取消自有 scan 与 index；磁盘上保留
  resumable checkpoint 表示可以续做，不表示当前还有执行体；
- Connector aggregate：只声明 `Stop`，仅包含编译期固定的 `feishu / wecom /
  dingtalk / tmeet`。所有 connect-flow 阶段都绑定同一精确 lease；短命令
  使用 30 秒 bounded runner，auth URL discovery 使用 40 / 60 秒显式
  deadline。出码后等待扫码没有固定总时限，但整个 flow 仍受 lease 跟踪并可按
  PGID 取消。`connector_id + monotonic generation` 的 active lease
  登记全部 owned process group，而不是单 PID 槽；旧 generation cleanup 不能
  清除新 PID。Unix 子进程以独立进程组启动，Adapter 只使用 Pinvou 登记的
  root PID，拒绝 PID 0，通过直接
  `libc::kill(-pgid, SIGKILL)` 停止进程组。Cancel / HostWork Stop 会对该
  connector 所有活跃 generation 的全部 owned PGID 逐一尝试后再汇总；
  每个确认成功的 generation / PGID 立即退休 ownership，只保留失败项供后续
  retry / reconcile，避免对可复用旧 PGID 重复杀伤。任一失败仍使该批次整体为
  `outcome_unknown`，不假报 `Stopped`，也不扫描全机进程猜目标；
- ASR cgroup：只声明 `Stop`；Adapter 先从 Supervisor 取得通过固定身份和有效
  policy 核验的 `Status + Reconciled`，再把该实例的 systemd `InvocationID` 原样
  作为 Stop 前置条件；
- App cgroup：`essential=true`、`governable=false`、不声明控制动作；它只通过
  Supervisor Status 提供受信状态和资源观测，不是 Governor 自停 PinvouOS /
  WebKit 的通道。

当前没有 Engine turn 或通用 managed-child Adapter。旧 Mission 同步 callback 已从
生产面移除，不得把 Mission Ledger 中仍可见的 Directive 骨架写成可执行控制。

四个进程内 Adapter 是“聚合 HostWork”：同一 HostWork generation 期间成员集合
可以变化。一次 status / stop / reconcile 只证明当次 poll 看到的成员；新成员可以在
“Stopped”观测之后立即出现，因此它不是原子 admission fence。后续 poll 若再看到
live state，Runtime 会续租为新 generation 并重新对当前压力做治理判断；这里
存在一个有界 poll 窗口，不得宣称同 generation 下永久无新工作。

### 独立 Host Supervisor 持有固定用户级动作

当前 Linux 工作树已包含与 PinvouOS app service 进程分离的同 UID
`pinvou-supervisor` user service。首版 descriptor schema 为 v1，wire protocol 为 v2，
请求的动作矩阵只有：

| 固定目标 | 允许动作 | 当前 HostWork 用法 |
|---|---|---|
| `pinvou-app` | `Status`、`Launch` | App cgroup Adapter 只调 `Status`；`Launch` 只由固定专用 launcher 使用 |
| `pinvou-asr` | `Status`、`Stop` | ASR Adapter 先取受信 `Status`，再以精确 `InvocationID` 请求 `Stop` |

协议拒绝未知字段，没有 PID、unit、cgroup path、property、command、shell 或任意
`systemctl` 参数。Daemon 还会核对固定 unit 的 effective `FragmentPath / ExecStart /
resource policy`；未通过身份和保护策略核验的 Status 只能返回
`outcome_unknown`，不能成为控制 authority。

effective policy 核验还包含 `Restart / RestartUSec / StartLimitIntervalUSec /
StartLimitBurst`。App base unit 固定为 `on-failure / 15s / 300s / 3`，其
4 GiB / 8 GiB / 2 GiB 内存线由显式 MegaBook canary drop-in 提供；ASR base
unit 固定为 `on-failure / 3s / 60s / 3`，20% / 35% / 2 GiB 由 Supervisor
package drop-in 提供。任一字段缺失、无效或被其他 drop-in 改写都使
Status fail closed 为 `outcome_unknown`。ASR 百分比不只校验 `high < max`：
Daemon 会从有界 `/proc/meminfo` 读取物理内存，按系统页向上取整后校验
effective 字节值不宽于 20% / 35%，允许页舍入或更严格的上限，并要求
systemd property 与实际 cgroup 文件一致。

Socket 由 systemd 在 `%t/pinvou-supervisor/control.sock` 激活，目录模式 0700、socket
模式 0600。Daemon 以 `SO_PEERCRED` 校验同 UID；ASR Stop 还要求 peer PID 精确
匹配当前受信 `pinvou3-app.service` `MainPID`。Client 通过 `SO_PASSCRED /
SCM_CREDENTIALS` 反向校验回包 UID 与 `pinvou3-supervisor.service` `MainPID`。这种相互
身份校验缩小了误调用面，但同 UID shell 仍可以获得同级用户权限，因此
不得宣称这是强 sandbox 或恶意同 UID 代码的安全边界。

Supervisor 在受监督 app service cgroup 之外，因此该 app/WebKit 整组终止时 Supervisor
可以继续保存证据。控制账本 `control-v1.jsonl` 在副作用前持久化 `Pending`
并 fsync，再写终态 tombstone；崩溃恢复不重放未知副作用，只能用同一
request id 对账。独立滚动 `observations-v1.jsonl` 保留状态、cgroup events、unit
result 与心跳证据。ASR Stop 在写入 Pending 后、副作用前再次读取 unit；
`InvocationID` 已变化就拒绝旧实例控制。

App Launch 同样 fail closed：初始 Status 与紧邻副作用前的第二次 preflight
都必须受信，且均明确为 `Inactive | Failed` 且 `MainPID=None`，才可以
start。`Active` 直接视为已满足；`Activating / Deactivating / Unknown`、任何带
MainPID 的状态、Status 读取失败或 policy 不可信都在无 start 副作用时拒绝。
若 start 后的 Status 不可用或不可信，还只有上述两次 preflight 都可归因且该次
start 返回成功，才把新 app 视为本请求所有并执行 rollback stop；未知或既有
实例、不允许的过渡状态与 start 失败都不得误停 app。

这个边界只在显式受监督部署中成立。普通 `pinvou3.desktop` 仍直接执行
`/usr/bin/pinvou3-tauri`，不在 app service cgroup 中，也不因“安装包内有
Supervisor”就获得 app 内存硬上限。受审的 MegaBook 资源只作为 inert profile
和专用 launcher 随包提供，需显式激活。

旧 f24 direct transient canary 曾运行 15 分 50 秒、峰值 306 MiB、memory events
为 0，但它使用旧直接临时 unit。它不是当前 Supervisor / profile / HostWork
新链路的实机验收，也不能证明真实 deb 安装、mutual credential、ASR Stop 或
app `MemoryMax` 留证已成立。

第一版只治理用户级工作。不得复用任何 `NOPASSWD:ALL` 或通用 sudo 开关。未来若确需
控制系统级服务，应增加独立 polkit/helper，只允许编译期白名单中的固定
`action + descriptor`，并单独评审。

### 账本先记意图，再记回执

Runtime schema v6 已增加以下事件；旧账本保持原字节，只在 replay 时 upcast：

- `HostWorkRegistered / HostWorkObserved`；
- `HostWorkDirectiveIssued`；
- `HostWorkDirectiveDispatchRecorded`；
- `HostWorkDirectiveAcknowledged`；
- `HostWorkDirectiveReconciled`；
- `HostWorkUnregistered`。

签发 Directive 只把 desired state 与 Directive 状态记为 `Pending`。`Applied` ACK 也不直接
修改 observed state；只有后验 status 观测才能 reconcile 到真实 observed state。超时、
连接断开或 RPC 响应丢失统一进入 `outcome_unknown`，保留同一 `directive_id`
对账，不因响应丢失换 id 重放副作用。Runtime 对每个 `work_id + generation`
强制单飞行；generation 不匹配时拒绝旧控制。

### Capability 与 Host 控制严格分层

模型可见 Capability 回答“Pinvou 能做什么”；Host Control 回答“内核能停止哪个已登记
工作”。两者不共享用户开关，也不因为某工具在 catalog 可见就获得进程控制权。

Front 只能解释资源事实和已确认回执；`system/*` 投影由 Kernel/Host 生成。Renderer
可以显示“正在停止 / 已停止 / 拒绝 / 状态未知”，不能构造目标或伪造成功。

## 当前阈值与部署边界

全机内存比例仍来自 `total - MemAvailable`，默认阈值为 85% / 92% / 97%。
这些阈值只形成全机压力 Claim，不是 app 自身 cgroup 的保护线。当前工作树
已从 app Supervisor 的受信 `Status + Reconciled` 回执导入 cgroup memory 观测。
Supervisor worker 单写进程内 cache，Resource sampler 只用非阻塞 `try_read`；等锁、超过
15 秒、`OutcomeUnknown`、无效 `InvocationID` 或未通过固定身份/策略核验时，
本次 5 秒采样不注入 cgroup 数据，也不阻塞采样循环。

cgroup Critical 的确定性语义如下：

- 第一张受信样本对 cumulative `memory.events` 只建 baseline，但同一张样本若
  `memory.current >= memory.high > 0` 会立即进入 Critical；
- 同一 `InvocationID` 的后续严格更新样本中，`memory.events` 的 `high /
  oom / oom_kill` 任一正 delta，或 `memory.current >= memory.high > 0`，都是新鲜
  Critical 证据；
- 一旦 cgroup Critical 成立，缺失、过期、`OutcomeUnknown`、实例切换、倒序样本或
  counter reset 只能 sticky hold，不能降压；
- 解除要求同一 `InvocationID` 的新样本，三个 counter delta 都明确为 0，且
  `memory.current < memory.high`；其他任何“没看到”都不是 relief 证据。

`pressure_epoch` 只在 `Normal / Warm / Hot / Critical` 等级变化时递增。同一等级
下新 cgroup counter / crossing edge 可以刷新 Claim 与落账证据，但不制造新 epoch。
为补偿一次真实的 Adapter `Rejected`，同一 `work_id + generation + action +
pressure_epoch + policy_revision` 最多签发“首次 + 1 次 retry”。第二张 Directive
必须由首张 Directive 之后、样本年龄小于等于 15 秒、时间严格递增且已经落账的
独立 fresh Critical 证据授权；证据可来自当前全机 Critical 阈值或受信 cgroup
Critical 事实，sticky hold 本身不算新证据。这张证据可以先于首次 Rejected ACK 到达，其
credit 随 Ledger projection 跨重启保留；第二次 Rejected 后，同 generation / epoch
不再重试。这个有界规则避免 5 秒采样变成无限副作用重放器；对聚合工作，
如果第二次仍失败且没有新 generation 或新 pressure episode，当前不再尝试，这是
明确的已知边界。

基础 app unit 故意不包含绝对 `MemoryHigh / MemoryMax / MemorySwapMax`。每台设备的
绝对值只能由受审查、显式激活的部署 profile 决定，不由模型或 Renderer
动态改写。当前唯一的 app 绝对 profile 是 MegaBook canary：4 GiB
`MemoryHigh`、8 GiB `MemoryMax`、2 GiB `MemorySwapMax`。它与专用 launcher 均是
inert 安装资源，需受审流程显式激活，不是普通 desktop 默认，也不是其他设备
的默认配置。

ASR 的完整 policy owner 是 ASR base unit 加 Supervisor package drop-in：base unit
持有 restart / StartLimit，drop-in 声明 20% `MemoryHigh`、35% `MemoryMax`、2 GiB
`MemorySwapMax` 及其余 cgroup 保护。安装脚本只对当时在线 user manager 做
daemon-reload 并启动 Supervisor socket，不停止或重启已运行 ASR；已运行实例
要到之后的有效重启才会应用这组新 cgroup property。

## 当前实现锚点

本决策中的“已实现 / 未部署 / 未验证”以这些生产入口为准：

- `pinvou3-app/src-tauri/src/features/pinvou_os/resource_agent.rs`：5 秒采样；除全机
  压力变化 / 30 秒心跳外，app cgroup baseline、实例/策略/counter 变化、越线或
  relief 证据以及有界 fresh retry evidence 也可触发入账；
- `pinvou3-app/src-tauri/src/features/monitor/mod.rs` 与 `features/monitor/platform/`：
  GPU cache 异步单飞刷新、旧值 / `None` 立即返回，NVIDIA 2 秒、macOS 5 秒、
  Windows 15 秒外部探针硬超时；GPU I/O 不持 cache 锁也不阻塞 RAM / cgroup 采样；
- `pinvou3-app/src-tauri/src/features/pinvou_os/governor.rs`：85 / 92 / 97%
  默认全机压力阈值、HostWork 确定性候选与 app cgroup 压力评估；
- `pinvou3-app/src-tauri/src/features/pinvou_os/model.rs` 与 `runtime.rs`：schema v6
  HostWork Registry、opaque handle、generation 续租、单飞行、Directive / durable dispatch
  marker / ACK / reconcile / replay；注册与观测写方法只对 crate 内受信组合面开放；
- `pinvou3-app/src-tauri/src/app/host_work_control.rs`：6 个静态生产 Adapter、独立
  异步 worker、状态轮询与后验 reconcile；
- `pinvou3-app/src-tauri/src/features/monitor/platform/linux_memory.rs`：全机
  `MemTotal - MemAvailable` 观测，不是 cgroup 内存事实；
- `pinvou3-app/src-tauri/src/lib.rs`：组合 HostWork 控制面；对 Renderer / 模型仍只暴露
  Runtime snapshot / projection / explain / list 等只读面，没有 HostWork 注册或
  控制写入口；
- `pinvou3-app/src-tauri/crates/host-supervisor-protocol/`、`src/platform/host_supervisor.rs`
  与 Linux client：封闭 wire v2、固定 target/action、请求响应匹配与回包 peer credential
  校验；
- `pinvou3-app/src-tauri/packaging/linux/supervisor/`：首版 daemon、descriptor / policy
  校验、restart / StartLimit 与 effective memory fail-closed 核对、Launch 所有权
  受限 rollback、`InvocationID` 前置条件、双向 PID 凭据与耐久控制/观测账本；
- `pinvou3-app/src-tauri/packaging/linux/deb/` 与 `packaging/linux/descriptor/`：
  Supervisor socket/service、受监督 app unit、ASR drop-in、descriptor v1、显式 MegaBook
  4 GiB / 8 GiB / 2 GiB profile、专用 launcher，以及只接受
  `activate / deactivate / status` 的 marker-transaction helper。普通 desktop 仍是 direct；
  helper 以 v2 `installing → effective-policy 校验 → applied` 两阶段 marker 表达事实，v1
  profile / desktop 的包内源路径、用户目标路径、字节/hash 与 legacy marker 路径/字节/hash
  共同冻结为 cleanup ABI，并保留 status/deactivate 兼容。每个目标用 no-clobber hardlink、
  inode/mode/hash/nlink 复核及 file/parent fsync 分别原子发布，不宣称两文件整组瞬时原子；
  删除先原子 rename 到同目录固定 quarantine 再复核，避免同 UID 编辑器 atomic-save 的
  TOCTOU。activate/deactivate 都在 app `Inactive | Failed` 且 `MainPID=0` 的停止边界
  fail closed；三个固定私有 `0700`
  staging namespace 只恢复可证明的空/半写单链接残留，或与固定公开目标同 inode 的
  已发布双链接残留；未知内容保留并报出精确恢复路径；
- `pinvou3-app/scripts/megabook-supervisor-e2e.sh` 与其固定 fixtures：真实 deb 基线、
  deb SHA-256、完整 maintainer control 成员与跟踪的安装行为字段、生成 `.list`、control `md5sums`、
  `dpkg --verify` 与 12 条关键安装路径的模式/大小/内容行为等价证据、hardened client /
  mutual credential、same-UID ASR Stop 负测、High / Max 独立 generation、
  ready/go 放量门、精确账本与 journald/oom 证据、trap 回滚、prepare-purge 与
  verify-purged 的自动验收结构。任何已安装 helper / Supervisor 执行之前都先完成精确 deb
  行为等价校验；该证据不冒充为能从 dpkg 状态反推原始压缩 archive 字节的回执。首次
  Launch 前必须证明无任何内存测试资产，退出时恢复并核验 socket / Supervisor /
  ASR 初始活动状态。脚本不调用 sudo；真实安装和 purge 仍各需一次用户授权；
- `pinvou3-app/scripts/tauri/build.js`：Linux deb 以 `umask 0022` 把固定
  `deb.files` allowlist 复制到 `src-tauri/target/` 临时 staging，并在新产物中对每个
  固定目标核对恰好一份、`root/root`、预期 mode 与源字节 SHA-256；symlink、
  staging / 包内 hardlink、路径、mode 或 hash 异常都 fail closed。这是打包门禁，
  不是实机安装证据。

以上新部署资源和验收结构尚未经 MegaBook 真实 deb / systemd / High / OOM / purge E2E。

实现接线变化后必须同时更新这些锚点、架构索引、三份专题 HTML、Resource Skill、
packaging 说明、schema/replay 测试和 architecture guard；只改图或 Prompt 不算落地。

## 安全与恢复不变量

1. 未注册工作不可控制；注册表没有“任意 PID/unit/command”入口。
2. Resource Agent、Front、模型、Renderer 都不能直接执行控制。
3. Directive 必须绑定 `work_id + generation + action + policy revision`。
4. `Stop` 幂等；重复同一 directive 不产生第二次副作用。
5. outcome unknown 不自动换 id 重发。
6. `Applied` ACK 只证明 Adapter 接受或发出了动作；observed state 只由后验
   status reconcile 改变，不来自 desired state、UI 或单独 ACK。
7. app 崩溃不能杀死 Supervisor；Supervisor 崩溃时应用不得假装仍有硬保护。
8. 恢复只作用于被同一 Governor 暂停且明确支持 Resume 的工作。
9. 每个自动停止都必须留下观测证据、Policy 版本、目标 generation、执行回执与结果。
10. 聚合 Adapter 的一次 Stopped 只证明该 poll 看到的成员，不是对后来成员的
    原子 admission fence；后续 live 观测必须续租新 generation 并重新治理。
11. 普通 desktop direct 不得宣称受 app cgroup 保护；只有显式 profile + 专用
    launcher 的受监督部署才具有该边界。
12. 同 UID socket 与相互 PID credential 不是恶意同 UID shell 的强隔离。
13. 任何权限扩大都必须新增 ADR、负向测试和部署回滚方案。

## 落地状态与验收

当前工作树已落地的代码阶段：

1. schema v6、HostWork Registry、只读 status、Directive / durable dispatch marker / ACK /
   reconcile 与 replay；
2. scheduled、knowledge、固定 connector、受限 detached sub-agent、ASR Stop 与 app
   status-only 的 6 个生产 Adapter；无 EngineTurn / ManagedChild / app 自停 Adapter；
3. 每个 Adapter 的独立异步 worker、Governor 确定性 Pending、单飞行、后验观测
   和 `outcome_unknown` 对账；
4. 独立 user Supervisor、descriptor v1 / wire v2、固定 action matrix、`InvocationID`
   与双向 PID credential 校验、effective restart / memory policy fail-closed、
   Launch 所有权受限 rollback，以及耐久控制/观测账本；
5. 随包 Supervisor unit、app unit、ASR drop-in、MegaBook inert profile、专用 launcher
   与显式 profile helper；仓库内已有固定、可回滚的 install / High / Max / purge harness。

仍待完成的验收阶段：

1. 在 MegaBook 以真实 x86_64 / Debian `amd64` release deb 完成安装、显式
   profile 激活、专用 launcher、mutual credential、ASR Stop、应用组越过
   `MemoryMax` 以及 Supervisor 存活/留证 E2E；
2. 核对普通 desktop 继续 direct 且不被误设为全局默认；实际执行卸载、回滚和
   已运行 ASR 不被 postinst 意外重启；
3. 再决定是否开放受保护的 `system/*` 状态投影或人工恢复入口；当前模型与
   Renderer 保持只读。

最低验收不变：

- Critical 只向已注册、nonessential、governable 且声明 `Stop` 的 HostWork 签发；
  未知 PID / unit / command 全部拒绝，app cgroup 不能成为自停目标；
- Stop / ACK / replay 幂等，重启不重复未知副作用；
- app/WebKit 受监督组越过 MegaBook profile `MemoryMax` 时 Supervisor 仍存活并留证；
- 一个 Adapter 卡住不阻塞 Resource Agent 继续采样；
- ASR、scheduled、detached sub-agent 与 foreground turn 的所有权互不串线；
- 模型、Renderer 与远程输入无法生成或绕过 HostWork 控制目标；
- 聚合 Adapter 的 Stopped 不冒充永久 admission fence；新 live 成员在下一次
  poll 续租新 generation 并被重新评估。

## 明确不做

- 不给 Resource Agent、Front 或模型提供任意 `kill` / `pkill` / `systemctl` / PID 工具；
- 不把 Linux 工作伪装成 Mission Agent；
- 不在应用进程内实现唯一 watchdog；
- 不用 97% 全机阈值替代 per-cgroup MemoryMax；
- 不自动重启无限 crash loop；
- 不把 Supervisor 状态、客户端 store 或 UI 提示当成 Runtime Ledger 的替代真相源。
