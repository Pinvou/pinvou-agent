# 阶段 1 延迟 Spike：实验设计与首轮实测

> 状态：**S1 已完成（本机实测）**；首次 S2 运行因额度错误且门禁脚本 fail-open，结论无效；S2/S3 待有效执行。S2 只作为开工前协议/事件形态可行性门禁，产品全路径性能门禁在阶段 1 M4 执行。
> 日期：2026-08-19
> 上游：蓝图 §13.4.1（测量合同与晋级门禁）；决策冻结 D-05/D-06/D-09/D-10。

## 0. 本 spike 要证伪的三个假设

1. **H1**：单机"Node spool durable → Controller WAL durable"双屏障路径的延迟地板，足以支撑 `event-to-screen p95 ≤ 100ms`。
2. **H2**：批量 group commit 下，WAL/spool 的持续吞吐能达到真实 Codex 峰值事件速率的 10 倍。
3. **H3**：真实 codex 流式事件形态（速率、事件大小分布）与我们对 50ms 合并窗口的假设一致。

## 1. 测量合同修正（t0 定义）

蓝图 §13.4.1 原来的 `t0 = adapter_normalized` 边界不够明确：实现若把“normalized”记录在合并完成后，50ms batch 窗口的等待时间会被排除在 `event-to-screen` 之外，而用户感知延迟包含它。故合同改为从 stdout `read()` 返回原始字节起计，并另设 `t0a` 表示未合并事件完成解析。

**修正后定义（本文档起生效，需回写蓝图 §13.4.1）：**

```text
t0  raw_cli_output_arrived   # Adapter 从 codex stdout read() 返回原始字节
t0a raw_event_normalized     # 解析为未合并的原始 RuntimeEvent
t1  raw_spool_durable        # 50ms raw batch 达到 durable barrier
t1' transport_event_ready    # 对已 durable source span 合并并分配传输 seq
t2  controller_ingested / t3a wal_durable / t3b projected / t4 terminal_flushed
event-to-screen = t4 - t0    # 用户视角；包含同一个 50ms raw group-commit/合并窗口
```

## 2. S1：fsync / IPC 微基准（已完成）

### 环境

```text
主机: HP Z1 Entry Tower G5 | CPU: Intel i7-8700 @ 3.20GHz
OS:   Windows 10 企业版 | 磁盘: KIOXIA KXG60ZNV512G (NVMe SSD, 512GB)
Rust: rustc 1.95.0, -O, std-only | C/D 同一块物理盘
```

### 结果（n=300，各盘取第二轮；D 盘=C 盘差异 <10%，取 D 盘列示）

| 测项 | p50 | p95 | p99 | max |
|---|---|---|---|---|
| 纯写 256B（无 sync） | 4.9µs | 7.8µs | 47µs | 68µs |
| 逐事件 fsync（write+sync_all） | 794µs | 1.26ms | 3.27ms | 4.4ms |
| 16 事件批量 fsync（每事件摊销） | **56.9µs** | **69.1µs** | 122.7µs | 122.7µs |
| 双屏障串行（spool durable→WAL durable） | **1.61ms** | **2.70ms** | 3.20ms | 6.3ms |
| TCP 回环 RTT（IPC 上界近似） | 36.5µs | 45.8µs | 69.5µs | 16.4ms* |

\* max 尾部是首连接/调度噪声，named pipe 预计更低。

**结论 1（H1 部分证实）**：双屏障 p95≈2.7ms，仅占 100ms 预算的 ~3%。**结构上"显示前双 fsync"在本机（NVMe SSD）不构成门禁风险**；风险转移到吞吐路径（见结论 2）。注意：此结论仅对 NVMe SSD 成立，HDD（p95 预计 >20ms）不在支持范围，安装文档需声明 SSD 为推荐配置。

**结论 2（批量是硬前提）**：逐事件 fsync 每事件 ~0.8–1.8ms → 单 stream 上限 ~600–1200 events/s；16 事件批量后每事件摊销 ~57µs → ~17k events/s，**相差 14 倍**。蓝图 §13.4 的 group commit 设计被证实为必要条件而非优化项；D-05 数值（R0 5ms/16 上限、R1 挂 50ms 批）与实测匹配。

**结论 3（IPC 可忽略）**：双跳 IPC（cli↔controller↔node）按 TCP 上界估计 <0.1ms，不进入预算主项。

### 预算分解表（p95 目标 ≤100ms）

| 分项 | 预算 | 实测/假设 |
|---|---|---|
| t0→t0a 原始解析 | ≤1ms | S2 记录，M4 校准 |
| t0a→t1 raw batch 等待 + spool durable | ≤50ms | D-06 的同一 50ms 窗口；fsync 批内摊销实测 69µs/事件 |
| t1→t1' durable source span 合并 + 分配传输 seq | ≤1ms | 假设，M4 校准 |
| t1'→t2 传输 + Controller ingress | ≤1ms | IPC 实测 <0.1ms |
| t2→t3a WAL durable（批量摊销 + R0 窗口） | ≤6ms | 双屏障实测 2.7ms（含 spool） |
| t3a→t3b projector 批 | ≤20ms | 假设，阶段 1 M4 产品链路校准 |
| t3b→t4 终端 flush | ≤20ms | 假设，阶段 1 M4 在 Windows Terminal 校准 |
| **名义分项预算和** | **≤99ms** | 仅用于定位超支；各分项 p95 相加不能数学上证明端到端 p95，G1 必须直接测量 `t4-t0` |

> 分解表的意义：S2 只能回填事件形态、合并和抛弃式持久化估算；IPC、projector 与终端行必须到阶段 1 M4 产品链路回填。超支行即为优化对象，不再笼统"调性能"。

### 复现

```bash
cd tmp/spike-fsync && rustc -O main.rs -o spike.exe && ./spike.exe .   # 跑两轮取第二轮
```

代码：`tmp/spike-fsync/main.rs`（std-only，无依赖）。

## 3. S2：真实 codex 协议与事件形态探针（开工前门禁，剩余 ~1 人日）

### 3.0 已完成：零成本协议探针（2026-08-19，不消耗配额）

不跑 turn、只做方法握手，已确认（详见 Codex Adapter 合同 §1.1）：

- 帧格式 = **行分隔 JSON**；响应不含 `jsonrpc` 字段；stderr 必须与 JSON-RPC 流分离。
- `initialize` / `account/read`（登录态可查）/ `model/list` / `thread/start` / `thread/list` 全部实机可用。
- thread 创建触发 MCP startup 通知串（含失败项）；响应与通知内容重复 → 投影幂等去重是真实需求。
- 登录态：ChatGPT（本机 `codex login status` 确认）。

**剩余需要消耗配额的部分**是下面 A–D 四个 turn 场景；执行方式见 §3.1 分层。

### 3.0.1 2026-08-19 首次运行判定：无效

仓库 `tmp/spike-fsync/s2-report.txt` 与 `s2-raw.jsonl` 只保留为失败证据，不得作为门禁报告：

- 四个 turn 均收到 `usageLimitExceeded`；`turn/completed` 的实际状态为 `failed` 或 `interrupted`，没有成功内容回合。
- A/B/C 三个场景均为 `R1=0`、`first_delta=None`；C 未出现审批请求，却被旧脚本判定 G1/G2/G3 PASS。
- 旧脚本把任意 `turn/completed` 视为 `completed=true`，且 PASS 条件没有校验成功终态、最小样本数、R1 内容或 `approval_seen`，属于 fail-open。
- t4 是重定向文件句柄 flush，模拟链路没有真实 Controller IPC、projector 或 Windows Terminal，不能证明产品 `event-to-screen`。

在不修改探针代码的本轮文档修订中，S2 保持 pending；原 `s2.rs` 不得原样重跑并用于门禁。后续修复探针时必须先补齐下述 fail-closed 判据。

### 设计

复用 Codex Adapter 合同 §6 的 capture harness，验证真实协议形态并做抛弃式预算探针。该 harness 可以模拟 spool/WAL 延迟，但不能替代阶段 1 产品链路的 IPC、projector 和终端测量：

1. **场景 A（稳定流式）**：新线程 + 一个长输出任务（如"输出一段 2000 字的技术说明"），持续 30s+，记录原始事件速率/大小分布、t0→t0a 解析、t0a→t1 batch 等待及抛弃式持久化估算。
2. **场景 B（真实峰值）**：高输出任务（生成大文件/批量重排），实测单 Adapter 峰值 events/s 与 MB/s → 定 H2 的分母。
3. **场景 C（审批回合）**：触发一次命令审批，确认 request/response schema、真实 `approval.requested` 样本及抛弃式 R0 延迟估算。
4. **场景 D（打断）**：先让 `main` stream 达到 high-water 并保留未 ACK/缺口积压，再触发 `turn/interrupt`；测量独立 `control` stream 的控制事件时延与 ACK 推进，证明 R0 不受 R1 逻辑队头阻塞。

### 开工前通过判据（F1–F3）

```text
F1  协议有效：A/B/C 成功 completed，D 为 interrupted；无 auth/quota/protocol error
F2  事件有效：A/B 各有足量 R1 与 first_delta；C 必须 approval_seen=true；D 必须确认 interrupt 应答和终态
F3  样本有效：取得真实内容峰值 events/s/MB/s、事件大小分布和 50ms 合并率，足以冻结 spool/WAL 尺寸输入

任一前置条件失败 → 本次运行 INVALID，不计算 PASS 分位数、不更新基线、不关闭 S2
```

抛弃式 harness 可以报告模拟的 t0→t4 和 fsync 分段，帮助发现数量级问题，但这些数字只标记为 `feasibility_estimate`。阶段 1 M4 必须在真实 `pinvou CLI → Controller IPC → Node → spool → WAL → projector → Windows Terminal` 路径重新执行蓝图 G1–G3：`event-to-screen p95 ≤100ms`、WAL 吞吐 ≥10×有效 Codex 峰值、R0 审批/打断 p95 ≤30ms。

### 产出物

- `docs/superpowers/specs/` S2 证据报告，显式记录运行有效性、终态、错误计数、事件样本数和硬件/持久化参数；不能命名为产品全路径基准报告。
- H2 的分母数字（真实峰值 events/s）→ 回填实施规格的 10x 门禁具体值。
- D-06 合并窗口复核结论。

## 4. S3：突发与积压压测（阶段 1 后半，随测量 harness 交付）

确定性事件发生器（蓝图 §23 阶段 1 定义）复用同一 spool/IPC/WAL/projector 路径：

1. 小事件高频（1KB × 500–5000 events/s 梯度）持续 60s。
2. 较大事件批量（64KB × 50 events/s）。
3. 突发积压：Controller 暂停消费 10s → 恢复，测重放吞吐与清空时间。
4. spool soft/hard limit 触发与 `stream.aborted` 路径验证（R0/R1 零静默丢失）；`main` 积压时 `control` 独立 seq/ACK 仍满足 R0 门禁。

产出物直接进入阶段 1 验收报告（蓝图 §23 阶段 1 验收第 3、8 项）。

## 5. 对蓝图的回写项

1. §13.4.1：测量顺序已改为 `t0 raw output → t0a raw normalized → t1 raw durable → t1' transport ready`（§1）。
2. §13.4.1：已新增预算分解表与 S2/M4 证据层级要求（§2、§3）。
3. §26：合并窗口默认值已由 D-06 关闭；"Node 默认端口"推迟阶段 4（D-11）。
4. §10.4：默认合并窗口已收敛为 50ms，且合并不得发生在可恢复 raw spool 之前。
