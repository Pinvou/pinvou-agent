# PinvouOS：资源治理与 Host Supervisor

## 状态

已接受设计，尚未实施。

本文是 PinvouOS 资源治理、Host 工作控制和 Linux Supervisor 的唯一权威。它延续
ADR-0007 的原则：Resource Agent 只观察并提交 Claim，确定性 Governor 决策，
受信 Host 执行控制。ADR-0007 的历史决策保持不变；本文补齐此前没有定义的
“工作如何注册、谁能停止、外部进程由谁持有、失败如何留证与恢复”。

当前生产实现仍只有观测和控制账本骨架，没有有效执行控制：Resource Agent 每 5 秒
采样全机资源，压力变化或 30 秒心跳时写入账本；Governor 能为 Runtime Mission Agent
生成 `Pause / Stop / Resume` Directive，但生产装配没有注册任何 Control Adapter。
CodeWhale 子 Agent、WebKit WebProcess、PinvouOS 常驻后台、定时任务、知识任务、
连接器任务和 Linux 服务都还不属于可治理工作。因此“已监测到 Critical”目前不等于
“已停止任何工作”。

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

## 决策

### Resource Agent 继续只观察

Resource Agent 只负责：

- 从可信平台探针形成 `ResourceObservation`；
- 标注观测健康、缺失传感器和证据时间；
- 形成可撤回的资源压力 Claim；
- 向 Governor 提交治理候选。

它不持有进程句柄，不调用 systemd，不选择任意 PID，不直接改变 desired/observed state，
也不获得模型可调用的停止工具。其 `pinvou-resource` Skill 保持只读。

### Governor 只签确定性 Directive

Governor 根据固定 Policy、工作优先级、中断级别和最新可信观测签发 Directive。第一版
保持以下策略：

- `Normal`：不限制；仅恢复此前由同一 Governor 暂停且声明可恢复的工作；
- `Warm`：拒绝新的可选 Heavy 工作，不中断已运行工作；
- `Hot`：暂停低优先级、可中断且声明可恢复的工作；不能暂停时保持运行并记录原因；
- `Critical`：停止已注册、非必要、可治理的后台工作；不得把未知工作当成已停止。

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
- `kind` 是编译期封闭枚举，例如 Engine turn、detached sub-agent、scheduled run、
  knowledge job、connector job、managed child process、app cgroup、ASR cgroup；
- `supported_actions` 由 Adapter 声明，不能从 UI 或模型输入推导；
- PID、cgroup 路径、systemd unit 和子进程句柄只存在于受信 Adapter 内部，不进入
  Renderer、模型参数或长期 Capability Catalog。

HostWork Registry 只接受组合根和受信 feature 代码注册。远控、Web、MCP、A2UI 和模型
工具没有注册任意 HostWork 的写入口。

### 控制由窄 Adapter 执行

每个 HostWork Adapter 只实现其声明的动作：

```text
status(work_id, generation)
pause(work_id, generation, directive_id)
stop(work_id, generation, directive_id)
resume(work_id, generation, directive_id)
```

`directive_id` 同时是幂等 request id。Adapter 不接受额外命令字符串。执行必须离开
Runtime 的串行 append 路径，避免慢 systemd/进程等待阻塞资源采样和账本；完成后把
结构化回执交回 Runtime。

第一批 Adapter 复用已有所有权边界：

- EnginePool / TurnShellTaskRegistry：取消指定 turn 归属的 sub-agent 与后台 shell；
- Scheduled、Knowledge、Connector：调用各自已有取消句柄；
- Managed child：只控制由 Pinvou 创建并登记的子进程/cgroup；
- App / WebKit 与 ASR：只通过外部 Supervisor 持有的固定 descriptor 控制。

任何 Adapter 都不得扫描全机进程后“猜目标”。

### 独立 Host Supervisor 持有最后防线

Linux 上新增与 PinvouOS 应用进程分离的同 UID `pinvou-supervisor` 用户服务。它通过
cgroup v2 / user systemd 持有应用及受控后台工作，至少设置：

- 独立 app cgroup；
- `MemoryHigh` 软压力阈值；
- `MemoryMax` 硬上限；
- `OOMPolicy=kill`；
- `KillMode=control-group`；
- 有界 `TasksMax`、重启退避和日志保留。

Supervisor 位于 app cgroup 外，因此 WebKit/App 整组被杀后仍能保存 cgroup events、
unit result 和最后日志，并按策略恢复。应用内受信 Host Adapter / Supervisor client
只能在收到 Governor Directive 后请求 Supervisor 执行固定动作；Resource Agent 仍只提交
观测与治理候选。任何应用内调用方都不能修改 unit、写任意 property 或执行任意
`systemctl`。

第一版只治理用户级工作。不得复用任何 `NOPASSWD:ALL` 或通用 sudo 开关。未来若确需
控制系统级服务，应增加独立 polkit/helper，只允许编译期白名单中的固定
`action + descriptor`，并单独评审。

### 账本先记意图，再记回执

Runtime schema 升级时增加以下事件；旧账本保持原字节，只在 replay 时 upcast：

- `HostWorkRegistered / HostWorkObserved`；
- `HostWorkDirectiveIssued`；
- `HostWorkDirectiveAcknowledged`；
- `HostWorkDirectiveReconciled`；
- `HostWorkUnregistered`。

签发 Directive 只改变 desired state。只有 Adapter 的成功 ACK 加后验 status 观测才能
改变 observed state。超时、连接断开或 RPC 响应丢失统一进入 `outcome_unknown`：用同一
`directive_id` 查询 status/reconcile，绝不自动换 id 重发。重启后按账本中的 Pending
Directive 和 Registry 当前 generation 补偿；generation 不匹配时拒绝旧控制。

### Capability 与 Host 控制严格分层

模型可见 Capability 回答“Pinvou 能做什么”；Host Control 回答“内核能停止哪个已登记
工作”。两者不共享用户开关，也不因为某工具在 catalog 可见就获得进程控制权。

Front 只能解释资源事实和已确认回执；`system/*` 投影由 Kernel/Host 生成。Renderer
可以显示“正在停止 / 已停止 / 拒绝 / 状态未知”，不能构造目标或伪造成功。

## 当前阈值与部署边界

当前采样是全机视角，内存比例来自 `total - MemAvailable`，默认阈值为 85% / 92% /
97%。这些阈值只适合形成全机压力 Claim，不是 app 自身 cgroup 的保护线。实现 HostWork
后必须同时采集 app cgroup 的 `memory.current / peak / events / pressure / pids.current`，
并为 cgroup 观测使用独立策略。

每台设备的绝对 MemoryHigh/MemoryMax 由受审查的部署 descriptor 决定，不由模型或
Renderer动态改写。MegaBook canary 首先验证 4 GiB / 8 GiB 边界；验证结果不自动成为
所有设备的默认配置。

## 当前实现锚点

本决策中的“已实现 / 未实现”以这些生产入口为准：

- `pinvou3-app/src-tauri/src/features/pinvou_os/resource_agent.rs`：5 秒采样与
  变化 / 30 秒心跳入账；
- `pinvou3-app/src-tauri/src/features/pinvou_os/governor.rs`：85 / 92 / 97%
  默认压力阈值及 Mission Agent Directive 候选；
- `pinvou3-app/src-tauri/src/features/pinvou_os/runtime.rs`：Control Adapter 注册、
  Directive / Ack / replay 骨架；生产组合根目前没有注册调用；
- `pinvou3-app/src-tauri/src/features/monitor/platform/linux_memory.rs`：全机
  `MemTotal - MemAvailable` 观测，不是 cgroup 内存事实；
- `pinvou3-app/src-tauri/src/lib.rs`：当前只暴露只读 Runtime snapshot / projection /
  explain / list 命令，没有模型或 Renderer 控制写入口；
- `pinvou3-app/src-tauri/packaging/linux/deb/`：当前 Linux desktop / deb 安装资源；
  尚无 Supervisor unit、固定 descriptor 或 app cgroup 配置。

实现接线变化后必须同时更新这些锚点、架构索引、三份专题 HTML、Resource Skill、
packaging 说明、schema/replay 测试和 architecture guard；只改图或 Prompt 不算落地。

## 安全与恢复不变量

1. 未注册工作不可控制；注册表没有“任意 PID/unit/command”入口。
2. Resource Agent、Front、模型、Renderer 都不能直接执行控制。
3. Directive 必须绑定 `work_id + generation + action + policy revision`。
4. `Stop` 幂等；重复同一 directive 不产生第二次副作用。
5. outcome unknown 不自动换 id 重发。
6. observed state 只来自 Adapter ACK 和后验 status，不来自 desired state 或 UI。
7. app 崩溃不能杀死 Supervisor；Supervisor 崩溃时应用不得假装仍有硬保护。
8. 恢复只作用于被同一 Governor 暂停且明确支持 Resume 的工作。
9. 每个自动停止都必须留下观测证据、Policy 版本、目标 generation、执行回执与结果。
10. 任何权限扩大都必须新增 ADR、负向测试和部署回滚方案。

## 实施顺序与验收

1. 先落 schema v6、HostWork Registry、只读 status 与 replay 测试；没有执行副作用。
2. 接 Engine/turn、scheduled、knowledge、connector 的窄 Stop Adapter；保持模型工具只读。
3. 增加独立 user Supervisor、固定 descriptor 与 cgroup 观测；先 canary，再设默认值。
4. 接入 Governor 自动 Stop 和 outcome-unknown reconcile。
5. 最后开放受保护的 `system/*` 状态投影与人工恢复入口。

最低验收：

- Critical 只停止已注册 nonessential 工作；未知 PID/unit/command 全部拒绝；
- Stop/ACK/replay 幂等，重启不重复副作用；
- app/WebKit 整组越过 MemoryMax 时 Supervisor 仍存活并保存证据；
- 一个 Adapter 卡住不阻塞 Resource Agent 继续采样；
- ASR、scheduled 和 foreground turn 的 cgroup/所有权互不串线；
- 用户可看到真实结果，但模型无法生成或绕过控制目标；
- 正常降压只恢复由 Governor 暂停且仍匹配 generation 的工作。

## 明确不做

- 不给 Resource Agent、Front 或模型提供任意 `kill` / `pkill` / `systemctl` / PID 工具；
- 不把 Linux 工作伪装成 Mission Agent；
- 不在应用进程内实现唯一 watchdog；
- 不用 97% 全机阈值替代 per-cgroup MemoryMax；
- 不自动重启无限 crash loop；
- 不把 Supervisor 状态、客户端 store 或 UI 提示当成 Runtime Ledger 的替代真相源。
