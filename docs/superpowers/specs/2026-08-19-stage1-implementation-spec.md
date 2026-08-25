# 阶段 1 实施规格（Walking Skeleton）

> 状态：**草案（draft）**——阶段 1 主体实现前置条件：协议捕获 harness/fixture 完成，且延迟 spike S2 以有效样本通过 F1–F3；2026-08-19 首次 S2 运行无效，不满足此前置条件。决策冻结文档已经用户批准。
> 日期：2026-08-19
> 上游：蓝图 §6（crate 拓扑）、§23 阶段 1（验收标准）；本文档是其唯一实施分解，蓝图其余阶段的合同不在本文档范围。

## 1. 主体实现前置条件（M0 顺序）

```text
P0  决策冻结文档批准（D-01..D-14）
P1  执行 T1：协议捕获 harness 完成（Codex Adapter 合同 §6），零成本握手 fixture 入库
P2  基于 P1 harness 执行 spike S2，并以有效样本通过 F1–F3（协议成功终态、所需事件齐全、峰值样本有效；无效运行不得计算 PASS）

P0 通过后允许执行 M0 的 T1 和 S2；P1、P2 全部通过后，才允许从 T2 开始阶段 1 主体实现。T1 是前置产出任务，不得又把“P1 已完成”列为自身依赖。
```

## 2. 目录与 workspace 落位

现有 `pinvou-cli/` workspace（GAIA benchmark，6 crate）**原样保留**，新增 6 个 crate：

```text
pinvou-cli/crates/
  cli                      # 现有 benchmark 入口不变；新增 daemon 拉起 + distributed 命令模块（feature 隔离）
  benchmark-core 等        # 不动
  controller               # lib + pinvou-controller bin（IPC、Store、WAL、projector、NodeClient、Supervisor）
  node                     # lib + pinvou-node bin（EventSpool、RuntimeHost、resource）
  protocol                 # 事件 schema v1、IPC 帧与信封（含 16MB 帧上限）、HostMonotonicClock
  seglog                   # 共享段日志原语（append-only + CRC + barrier + 游标 + 恢复；决策冻结 D-04）
  runtime-api              # AgentRuntimeAdapter trait + RuntimeCapabilities + AdapterError
  agent-adapter-codex      # codex app-server JSON-RPC 适配
```

- `cli` 的 distributed 命令放 `cli/src/distributed/` 独立模块，与 benchmark 代码零交叉引用；编译不启用 `product-backend` feature 时必须完整可用（蓝图 §6.5）。正式 distributed/release binary 的 resolved dependency graph 不得包含 `pinvou-product-backend`、`pinvou3-app` 或任何 `codewhale-*` crate。
- 依赖图守卫 CI（禁止 tauri/pinvou3-app/codewhale*/product-backend）随第一个 crate PR 建立。
- 阶段 1 验收第 6 条（无 Tauri/CodeWhale/pinvou3-app 构建通过）在 CI 以独立 job 固定。
- 阶段 1 不修改 `pinvou3-app/`、`CodeWhale/`、主工程 lockfile/打包清单/默认构建输入，也不读写现有 Desktop 数据。legacy benchmark 保持现有行为；若 feature 隔离不能在仅修改 `pinvou-cli/` 的前提下完成，则停线并缩小 CLI 发布范围，不以修改主工程解耦。

## 3. 任务分解、构建顺序与估算

依赖串行主线为 T2→T5，T6/T8 可与主线并行（2 人并行点）。估算按 1 名熟练 Rust 工程师毛人日计。

| # | 任务 | 内容 | 估算 | 依赖 |
|---|---|---|---|---|
| T1 | 协议捕获 harness | Codex Adapter 合同 §6；JSONL fixture；完成后满足 P1，并供 S2 使用 | 1d | P0 |
| T2 | workspace 脚手架 | 仅在 `pinvou-cli/` 修改现有 cli 组合入口并创建 6 个新 crate（含 seglog）+ 依赖图守卫 CI + 主工程零 diff 门禁 | 1d | P1、P2 |
| T3 | protocol crate | 事件 schema v1（含 fixture 往返测试）、IPC 帧/信封（16MB 上限）、HostMonotonicClock、退出码常量 | 3d | T2 |
| T4 | IPC + Controller 骨架 | Named Pipe/UDS 监听、DACL、单实例锁（OS 锁句柄，D-08）、实例挑战、daemon detach 拉起、健康查询、日志滚动（50MB×5） | 3d | T3 |
| T5 | Node 骨架 + 监督 | node daemon、runtime-api trait、Controller 内 LocalNodeSupervisor（启动/崩溃重启/版本校验） | 3d | T4 |
| T6 | seglog + Node Event Spool | seglog 共享 crate（append-only 段 + CRC + barrier + 游标 + 恢复，红线见 D-04，含迁移文档 §5 合同测试）；至少 `control`/`main` 每流一实例并各自维护 raw `source_seq`/durable/transport-sent/ACK 水位，R0 紧急段只服务 control；合并只消费已 durable 原始记录 | 5d | T3（可与 T4/T5 并行） |
| T7 | Controller WAL + ACK | 基于 seglog 的薄封装 + group commit（5ms/16）+ BatchAck；`control`/`main` 各自连续传输 `seq`、ACK、去重与重放，Node 持久化每流 `seq -> source_span` 映射并据 ACK 回收 raw spool | 2.5d | T6 |
| T8 | Codex Adapter | 捕获确认帧格式→initialize/thread/turn 驱动→通知映射→审批应答→interrupt→auth_status | 5d | T5、T1（可与 T6/T7 并行启动） |
| T9 | CLI 终端投影 | 流式渲染（50ms 帧节流）、审批 y/n 交互、Ctrl+C 打断、退出码、`pinvou chat` 子命令 | 3d | T4 |
| T10 | Session Store + projector | SQLite schema、事件投影、回合末合并落盘（借鉴桌面 timeline 经验）、ResourceRef 登记 | 3d | T7 |
| T11 | 测量 harness | 在真实产品链路实现 t0/t0a/t1/t1'/t2/t3a/t3b/t4 打点（HostMonotonicClock）、事件发生器（S3）、基准报告生成；不得复用 S2 抛弃式文件 flush 冒充终端数据 | 3d | T7、T8、T9、T10 |
| T12 | 验收收尾 | 蓝图 §23 阶段 1 验收 1–9 逐项跑、报告、文档回写 | 2d | 全部 |

**合计：约 34.5 毛人日**（单人 ≈7 周；双人并行 T6/T8 错开后 ≈4.5–5 周；seglog 抽取多投 1d、T7 因复用省 0.5d）。估算含 15% 未知缓冲的口径是 **40 人日封顶**；超出即触发范围重审而不是加班。

## 4. PR 切分（对应仓库公约）

每个 PR 独立可评审、可合并；提交信息 `<type>(<scope>): <中文描述>` + DCO。scope 建议用 crate 名。

| PR | 内容 | 验收测试 |
|---|---|---|
| #1 | T2 脚手架 + 依赖图守卫 | 守卫 job 红绿测试（引入违禁依赖时必须失败）；`pinvou3-app/`、`CodeWhale/` 与主工程 lockfile/打包清单零 diff |
| #2 | T3 protocol | schema fixture 往返、帧编解码 fuzz 一轮 |
| #3 | T4 IPC + controller 骨架 | 双客户端并发连接、第二实例拒绝、版本不匹配错误码 |
| #4 | T5 node + supervisor | node 崩溃自动重启、协议版本拒绝 |
| #5 | T6 seglog + spool | seglog 合同测试（迁移文档 §5）、raw/transport 游标推进、合并 source span、barrier 前崩溃显式 gap、区间回收后重放正确性 |
| #6 | T7 WAL + ACK | 重复事件去重、BatchAck 携带独立水位、每流 seq 连续性、ACK 到 raw source span 的回收映射、崩溃恢复补拉、main 积压时 control 无队头阻塞 |
| #7 | T8 adapter | 捕获 fixture 回放合同测试 + 真实 codex smoke（手动跑，CI 标记 ignored） |
| #8 | T9 CLI 投影 | 退出码表、非 TTY 拒启、审批流 |
| #9 | T10 store/projector | 投影幂等（同事件重放两次结果一致） |
| #10 | T11+T12 测量与验收 | S3 压测 + 基准报告归档 |

## 5. 里程碑与出口标准

```text
M0（≈3d）   先完成 T1 捕获 harness/fixture，再执行 spike：S2 F1–F3 以有效样本通过；本文档从 draft 转 active
M1（≈10d）  IPC echo 端到端：cli → controller → node → echo 事件流回显
            出口：三个进程真实拓扑跑通，spool/WAL 可为 stub（事件走内存直通）
M2（≈17d）  可靠性合同：spool 断连补发、WAL 崩溃恢复、R0/R1 零静默丢失（不可恢复尾部显式 gap）合同测试全绿
            出口：蓝图 §23 阶段 1 验收 3、4 通过（确定性测试形态）
M3（≈24d）  真实 codex E2E：多轮流式对话 + 可重复文件任务 + 审批 + 打断
            出口：蓝图 §23 阶段 1 验收 2、5 通过
            附加（D-01 注记）：Linux 容器内延迟冒烟——fsync/UDS 路径无数量级意外（不设数值门禁）
M4（≈34d）  产品全路径性能门禁 + 报告：蓝图 G1–G3 + S3 + 验收第 7、8 项（p95≤100ms、10x、零静默丢失）
            出口：验收 1–9 全绿 → 晋级阶段 2 评审（蓝图 §23 门禁条款）
```

**任何 M 出口未达标即停线**（蓝图阶段 1 验收第 9 条），修正数据路径后重测，不带病进入下一里程碑。

## 6. 范围红线（再次显式）

- 不做：TUI、远程网络、pairing、第二个 Agent Adapter、mDNS、端口监听（D-11）、插件、Scheduler、`~/.pinvou3` 任何读写。
- `pinvou` 无参数行为 = 输出帮助（蓝图 §6.1 阶段 1 合同）；TUI 到阶段 3。
- benchmark 路径回归：每 PR 必须保证现有 benchmark crate 编译与测试及输出合同不受影响（CI 双 job）。
- 主工程零影响：每 PR 拒绝 `pinvou3-app/`、`CodeWhale/`、现有 Desktop 资源/配置/lockfile/打包脚本 diff；不得启动、迁移或清理现有 Desktop 数据与进程。

## 7. 未尽事项移交

- spool soft/hard 限额具体数值：S2 取得有效事件形态后在 PR#5 实现时定稿；2026-08-19 无效 S2 报告不得作为输入。
- `pinvou runtime detect` 子命令的最小实现（验收 2 需要）含在 T8/T9 交界，估算已摊入。
- AGENTS.md 修约（资产盘点文档 §4）：随 PR#1 一并提交。

## 8. 多 Runtime 会话连续性后续设计约束

本节记录来自“多 Agent Runtime 统一控制平面”讨论中适合 Pinvou 后续阶段吸收的部分；不改变阶段 1
既有出口标准。阶段 1 当前只落地 runtime list/detect/switch 与 turn 边界切换；跨 Runtime
连续会话、上下文压缩和统一工具平面进入后续任务。

### 8.1 Runtime 切换边界

- Runtime 只能在 Turn 边界切换。Node/Controller 必须在 active turn 存在时拒绝
  `runtime.switch`，提示用户等待当前 turn 结束或先 interrupt。
- 后续 Session Store 引入后，`active_runtime`、`turn_runtime` 与 `installed_runtimes` 必须分开：
  - `installed_runtimes`：本机支持的 Runtime profile；
  - `active_runtime`：下一轮默认使用的 Runtime；
  - `turn_runtime`：当前 turn 已绑定的 Runtime，turn 开始后不可变。
- Runtime 切换成功时应生成新的 Runtime Epoch。所有 Runtime 事件必须带 Epoch/fencing token；旧
  Runtime 的迟到事件不得写入新 Epoch 的 session state。

### 8.2 切换时的会话压缩与 Context Compiler

跨 Runtime 切换不应重放厂商私有原始消息作为唯一上下文来源。后续 T10/T11 或阶段 2 需要补齐：

- Canonical Event Log：完整保留用户输入、Runtime 事件、工具调用、审批、文件/资源引用和错误；
- Portable Checkpoint：从事件账本确定性提取事实状态，再由模型做语义压缩；模型不得生成系统 ID、
  文件哈希、Evidence ID 或工作区事实；
- Context Compiler：针对目标 Runtime/模型窗口编译上下文，按预算分层：
  - L0：尽可能完整的近期上下文；
  - L1：大型输出和文件内容外置为 ResourceRef/Artifact；
  - L2：Portable Checkpoint + 近期用户原文；
  - L3：当前 Task Slice。
- 安全规则、当前用户请求、活动计划、硬约束、工作区状态和未完成审批不得静默裁剪；目标窗口仍不足时
  明确阻止切换或要求切成更小任务。
- 切换流程采用 prepare/commit 两阶段：flush 当前 Runtime → durable barrier → 工作区扫描 →
  Checkpoint → 目标 Runtime detect → Context compile → 原子提交新 Epoch。

### 8.3 统一工具调用平面

后续不能只统一文本上下文，还必须统一工具契约，否则会话恢复和 Runtime 切换会在工具层断裂。
Pinvou 的统一工具平面应遵循：

- Tool Registry 记录 tool name、schema version、副作用等级、幂等语义、所需能力和 Runtime 支持矩阵；
- Portable Tools（工作区、Shell、Git、Artifact、Evidence、Plan、user ask）由 Control Plane 接管；
- Observed Native Tools 可被 Adapter 标准化但不一定能完全接管，必须标记 `enforced|observed|unknown`；
- Runtime Private Tools（厂商私有压缩、隐藏推理、私有子 Agent）不得伪装成 Portable Tools；
- 已完成工具调用只恢复结果，不重新执行；未完成非幂等调用标记 `uncertain` 并进入恢复核验；
- 目标 Runtime 缺少关键工具或工具策略降级不可接受时，阻止切换而不是静默降级。
