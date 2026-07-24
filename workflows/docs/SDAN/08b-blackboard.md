# 08b · 动态产出账本 Blackboard

> 架构层文档 · 通用 · 资源模块三类之一（总览见 `08-resources`）
> Blackboard = 角色运行时产出的可变账本（brief/outline/page_layout/slides）。**某角色产，会变，会被回滚覆盖。**

## 职责

各 SubAgent 产出的**全局账本**：登记每个节点的 `produces`，供 adapter 取数据、翻译成下游 `accepts` 打包进 Task。

## 存储：只存指针，大产物留磁盘

- 大产物（研究 `.md` / HTML / 图片等）**本就在磁盘**（`_research/`、`HTML_Deck/` 等）。
- Blackboard 只存**指针**：`node → 产出文件路径 + 治理元数据`（见下）。
- 指针表跟 State 一起放 `_state/`、**一起原子写**（同一份或 `blackboard.json`），崩溃恢复同源、一致。

## 指针 entry 结构（含治理元数据）

每个指针不只是路径，带产出溯源 + 裁决状态（防下游"把没过 Gate 的草稿当圣旨"）：

```jsonc
"page_layout": {
  "ref": "_state/page_layout.json",
  "produced_by": "designer",      // 谁产（调试/回滚追责）
  "produced_at": <step_index>,    // 何时
  "gate_status": "passed",        // hard 裁决: pending|passed|failed（只看 hard,与 03/05/06 一致,不引新阻断点）
  "kind": "intermediate"          // intermediate(仅下游用) | final(人类可见,对应 09 卡片流)
}
```

- `gate_status`：adapter 打包 inputs 前可校验上游是否已过 hard，防错误沿 DAG 放大。**注意**：SDAN 已定 soft 不阻断（`05`），这里只看 hard 的 PASS/WARN/FAIL，不引入新阻断点。
- `kind`：harness 决策依据——intermediate 自动传下游，final 触发用户可见（对应 `09` 卡片流）。

## run_id 隔离

指针表 namespace = `(run_id, node)`，跨 run 物理隔离。**当前 run 的 SubAgent 永远读不到上次 run 的产出**（防跨 run 槽污染）。

## 写入：append-once（约束在 Router）

- **正常推进时 append-once**：同一 run 内某 slot 只由其 `produced_by` 角色写；过 Gate 后标 sealed，防弱模型迭代里反复覆写已过 Gate 的产出。
- **回滚时**：由 Router 显式清指针后才允许重写。回滚是 Router 的权（`04` SubAgent 不发起跨节点回滚 + `06` 重跑覆盖），与本条自洽。

## 回滚

- 回滚清产出 = **清对应节点的指针**；磁盘真身由重跑**直接覆盖**（不版本/stale）。
- 与 State 的回滚计数/游标在同一次原子写里更新，不会半新半旧。

## 与 State 的关系

- **State = 进度**（轻）、**Blackboard = 产出指针**（也轻，真身在磁盘）。
- 两者一起原子写、一起恢复 → 进度与产出账本始终一致。

## 剩余细节（实现时定，非阻塞）

- 大产物引用的粒度（按文件 / 按 glob）。
- 指针与磁盘真身的一致性校验（如重跑前真身已被外部删除的兜底）。

## 不做（避免过度设计，记录推迟项）

- **不引 Cleaner agent / TTL**：SDAN 是串行 DAG + Router 完成度驱动调度，stale 由"回滚清指针 + run_id 隔离"已解决，加 TTL 是给单机串行工作流上分布式系统的药。
- **不引向量库 / 跨 run 语义记忆层**：现阶段 bonus，标未来。
