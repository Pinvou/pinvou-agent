# 阶段 1 CI 与真实 Agent 测试策略

> 状态：草案（draft）。日期：2026-08-19。
> 上游：蓝图 §22（测试策略）、§13.4.1（测量合同）、§23 阶段 1 验收；实施规格 §4（PR 切分）。

## 1. 问题

蓝图 §23 要求"真实 Codex 验收，mock 不能替代"，但没回答：CI 里怎么跑真实 Agent？账号与密钥怎么办？Agent 非确定性导致的 flaky 怎么定门禁？本文档给出分层方案。

## 2. 测试分层

| 层 | 内容 | 触发 | 网络 | Agent |
|---|---|---|---|---|
| **T0 单元/合同** | schema fixture 往返、IPC 帧编解码、退出码、状态机转换、合并规则 | 每 PR | 无 | 无 |
| **T1 确定性集成** | 假 Adapter 驱动全拓扑：`control`/`main` 独立 source/transport/ACK、source span 回收、WAL group commit、BatchAck 独立水位、main 积压时 control 无队头阻塞、断连补发、barrier 前 kill -9 显式 gap、spool 限额与 `stream.aborted` | 每 PR | 本机 IPC | 假 |
| **T1p 吞吐压测** | 确定性事件发生器（蓝图 §23：小事件高频 / 64KB 批量 / 突发积压 10s 恢复） | 每 PR（短档 60s）+ nightly（长档） | 本机 | 假 |
| **T2 真实 codex smoke** | 捕获场景回放断言 + 1 次真实短对话 + 1 次可重复文件任务 | nightly + 手动 + 发布前 | 外网 | 真 |
| **T3 性能基准** | 蓝图 §13.4.1 四类负载全量 + 报告归档 | 自托管 Windows runner：每周 + M4 里程碑 + 发布前 | 外网 | 真 |

**原则：真实 Agent 永不进每次-PR 路径**（成本、速率限制、flaky 三重原因）；PR 路径全部确定性可重放。

## 3. T2：真实 codex 在 CI 的执行方案

### 3.1 认证（按可行性降序，实施时验证第一项）

1. **API key 模式**：codex 支持 `OPENAI_API_KEY` 认证（以 0.139.0 实测为准）。CI secret 注入 key，`codex` 以 key 模式运行。优点：无交互、可轮换。
2. **专用 ChatGPT 账号 + 预登录状态**：自托管 runner 上维持一个登录态（`~/.codex/auth.json`），凭据不进 CI 变量；账号用量可监控。
3. **都不行**：T2 降级为"维护者本机每周定时执行 + 报告上传仓库"（流程同 T3，只是位置在人机上），并在 README 声明。**不允许**为了 CI 方便绕过 codex 登录或共享个人账号。

### 3.2 用例设计与 flaky 政策

Agent 输出非确定性，所以 T2 断言只验**结构与产物**，不验文本：

| 用例 | 断言 |
|---|---|
| 短对话（"用一句话回答 X"） | 事件序列合同：`turn.started → ≥1 text.delta → message.completed → turn.ended(completed)`；seq 连续；零 R0 丢失 |
| 可重复文件任务 | 固定种子提示词（如"在指定目录创建 hello.txt，内容为固定字符串"）；断言：文件存在且内容匹配、`resource.ref_created` 事件、checksum 校验通过 |
| 审批回合 | 预置需要审批的命令；断言 `approval.requested` 到达且 y/n 应答后收到 `approval.resolved` |
| 打断 | 流式中 Ctrl+C；断言 `turn.ended(interrupted)` 且 R0 时延记录 |

- 重试：单次失败自动重试 ≤2；连续 2 个 nightly 失败 → 用例标记 `quarantine`（nightly job 可不红）+ 开 issue，不阻塞无关 PR。
- `quarantine` 只隔离日常噪声，不能让里程碑空心化。短对话、文件任务、审批、打断四个关键场景中任一处于 quarantine、缺少有效样本或因 auth/quota 失败时，M3/M4 和发布门禁一律不通过。
- 门禁判定：M3/M4 出口与发布前，T2 四个关键场景各自最多执行 3 次，至少取得 2 次有效通过；有效运行必须满足期望终态、最小事件数和场景特有断言。auth/quota/protocol error 等无效运行不算通过，但仍消耗这 3 次尝试之一，防止靠无限补跑掩盖环境或协议问题。

### 3.3 成本控制

- 每个 nightly run 上限：≤10 turn、≤1 个文件任务；成本以 token 计入报告。
- 开工前 S2 与 T3 之外禁止真实 Agent 长任务；事件发生器承担一切吞吐验证（蓝图 §23"确定性发生器只做压力补充"的边界保持不变——**功能验收**使用真实 Agent，**可重复压力负载**使用发生器，真实峰值只作为吞吐倍率分母）。

## 4. T3：性能基准环境合同

- Runner：自托管 Windows（阶段 1 = 开发机同型配置；记录：CPU/OS/磁盘型号/终端 Windows Terminal/codex 版本/合并窗口/WAL batch/fsync 策略——蓝图 §13.4.1 全字段）。
- 基线对比：同环境历史基线，回归阈值 **吞吐 -10% 或 p95 +20%** 超限即阻断 M4/发布，要求解释或修复（蓝图 §22.4"超过阈值必须解释或阻止发布"）。
- 报告归档：`docs/superpowers/benchmarks/<date>-stage1.md`，含预算分解表回填（spike 文档 §2）。

## 5. CI Job 总表

| Job | 内容 | 门禁 |
|---|---|---|
| `pinvou-cli-build` | workspace 编译 + 现有 benchmark 测试回归 | 每 PR 必须绿 |
| `distributed-guard` | cargo metadata 依赖图守卫（tauri/pinvou3-app/codewhale*/product-backend 禁入）+ 无 product-backend 构建 | 每 PR 必须绿 |
| `main-project-zero-diff` | 拒绝 `pinvou3-app/`、`CodeWhale/`、主工程 lockfile/打包清单 diff；验证既有 benchmark 输出合同未变 | 每 PR 必须绿 |
| `t0-t1` | T0/T1/T1p 短档（Windows + Linux 容器双跑；延迟门禁只在 Windows） | 每 PR 必须绿 |
| `t2-nightly` | 真实 codex smoke | nightly；quarantine 机制 |
| `t3-bench` | 性能基准 + 报告归档 | 周期 + 里程碑 |

## 6. 与仓库现有 CI 的关系

- 现有门禁（fork-guard、architecture-guard、gitlink 校验）面向 pinvou3-app/CodeWhale 路径，不覆盖新 crate；`distributed-guard` 是新增独立 job，不修改现有门禁行为。
- CLI PR 不重新配置、放宽或借用现有主工程门禁；`main-project-zero-diff` 在进入耗时测试前先拒绝越界文件变更。若确需主工程改动，必须移出本路线另行批准。
- 新 crate 的测试不依赖 `~/.pinvou3` 或任何 Desktop 状态（蓝图 §6.5）；T1 使用临时目录 fixture。

## 7. 阶段晋级门禁矩阵

| 晋级 | 必须全绿 |
|---|---|
| M0→M1 | capture harness 自测 + fixture lint/replay + spike S2 F1–F3（有效协议/事件样本）；此时不要求尚未实现的全拓扑 T1 |
| M1→M2 | T0 + 三进程/IPC skeleton 测试；允许 spool/WAL stub，仅验证 M1 出口合同 |
| M2→M3 | T0/T1/T1p 长档 + 完整可靠性合同测试 |
| M3→M4 | + T2 四个关键场景各自最多 3 次、至少 2 次有效通过 |
| 阶段 1→阶段 2 | + T3 报告归档 + 蓝图 §23 验收 1–9 逐项核验记录 |
