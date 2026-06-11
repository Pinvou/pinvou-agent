# 07 · 记忆模块 State

> v1 定稿 · 载体/时机/恢复/状态机已定（经第 4 层真跑验证字段需求）

## 职责

工作流的**进度记忆**——Router 无状态，"现在到哪了"都在这：

- 每个节点 `status`：`pending / dispatched / running / done / warn / failed / blocked / blocked-upstream`（**无 `waiting`**——等用户归 SubAgent 内部，见 `04`）
- `join` 等齐情况（哪些硬上游已 done）
- `attempts`（retry 计数）、`last_verdict`
- **回滚计数 per-`(rollback_to, violation_type)`**（配 `max_rollback`，整轮累计，超限 → blocked）
- 回滚游标（结构回滚闭包重跑进度）
- **in-flight 账本 `(node → task_id)`**（Result 来源校验用，见 `03`）
- `scenario`

## 存储载体：落盘 JSON + 原子写

- 沿用 `_state/` JSON（跟现状一致、人可读、可直接 diff 排查）。
- **每次决策**（处理完一个 Result）后，把 State 快照**原子写**（写 temp + rename）。决策串行 → 一次写完整、不撕裂。

## 写盘时机

决策完成即原子写一次。State 很轻（几个节点的状态/计数），秒级决策频率，JSON 写开销可忽略；将来若变重再议增量。

## 崩溃恢复：至少一次 + 幂等

- 崩溃后**读最新快照恢复**。
- 决策原子性靠次序：**先写 State（标 `dispatched` + 记 in-flight `task_id`）→ 再派 Task**。
- 崩在"写了 State 没派出 Task"之间 → 恢复时对所有 `dispatched` 节点**重发** Task。
- **不漏**（重发）、**不重**（`task_id` 去重 + SubAgent 重跑覆盖幂等）。
- 即「**至少一次投递 + 幂等执行**」——不追求"恰好一次"（分布式难题），用幂等兜底。

## 已定约束

- Router 无状态 → 进度全在 State → 崩了重启接着跑（不丢）。
- 决策串行 → 读写无竞争（也是 join 不死锁的前提）。
- 回滚重跑直接覆盖（不版本/stale）。

## 剩余细节（实现时定，非阻塞）

- `status` 枚举的完整转移图（各状态进入/离开条件）——实现时画全。
- 与 Blackboard 同库还是分文件（见 `08`，建议同 `_state/` 下、一起原子写）。
