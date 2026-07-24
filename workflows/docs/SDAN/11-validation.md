# 11 · 验证

> 怎么确认 SDAN 模块互通、无矛盾。本篇固化**本次用 PPT 实例做的纸面走查**成果。

## 验证四层（硬度递增）

1. **端到端场景走查**（纸面）：让报文走过所有模块，看接口对得上、不丢不卡。
2. **接口契约对账**（逐字段）：上游给的 = 下游要的。
3. **不变量检查**：架构必须永真的命题。
4. **最小可运行骨架真跑**（walking skeleton，代码）：1 Router + mock SubAgent + 内存 State/Blackboard 真跑 join/回滚/retry。**第三方硬证据，留实施阶段做。**

> 诚实边界：1–3 是纸面推演（"我说的"）；终极硬证据是 4。**本次完成 1–3，用 PPT 实例压。** 且"对所有 workflow 无矛盾"不可穷举——务实路线：PPT 压通＝必要条件 + 不变量兜性质 + 每加一个实例多一个 conformance 用例。

## 本次走查 8 条结论（拿 PPT 跑出来的）

| # | 问题 | 结论 |
|---|---|---|
| 1 | 冷启动（第一个报文哪来） | Router 加 `on_start` 入口；由 UI 启动动作触发 `on_start` → 派无上游节点 |
| 2 | 软依赖（用但不阻塞） | **现阶段全硬 join**；软依赖推迟 |
| 3 | blocked 逃生 | **系统直发标准消息卡片**给用户（独立样式），不经任何对话型 LLM；下游标 `blocked-upstream`，并行链照跑，人工决定 |
| 4 | 回滚 cascade 漏传递闭包 | **只声明 `rollback_to`，Router 按 DAG 自动算闭包**（修了 density 漏 illustrator 的 bug） |
| 5 | 回滚时旧产出 | 重跑**直接覆盖**（不版本/stale） |
| 6 | need_input 撞"只拉不推" | **收集类 SubAgent 自己走 `request_user_input`+卡片**（不经品悟）→ 推拉矛盾消解；`need_input` 状态不存在 |
| 7 | 暂停粒度 | **不设 Router 级 `waiting` 状态**："等用户"归 SubAgent 内部（节点对 Router 仍是 `running`），SubAgent 自己 `timeout` 兜底；并行其他支天然不受影响 |
| 8 | 回滚深度写死 / 用户变更 | SubAgent 只交 Result、**不发起跨节点回滚**；用户主动变更类**推迟**（Pinvou tools + 运行时改路由表） |

其中 **#4 / #6 / #8 是真矛盾**（设计会跑错），已解 / 推迟；其余 5 条是待定义，已定。

## 接口契约对账表

| 接口 | 上游给 | 下游要 | 闭合？ |
|---|---|---|---|
| Result → Router | `verdict` / `violation_type`（∈ header_signals） | Router `outcomes` 据此定下一跳 | ✓ |
| edge adapter | `from.produces` | `to.accepts` | 逐边核（见 route_table），每条边一个 adapter |
| UI 进度 | `read_full_agent_state` | UI 直读 State 渲染 DAG/卡片 | ✓（零 LLM） |
| SubAgent | `allowed_tools` / `constraints` | ← registry | ✓（唯一真像） |
| 回滚 | `rollback_to` | Router 自动闭包（DAG edges） | ✓（不手写 cascade） |

## 不变量清单（必须永真）

1. **控制核唯一**：只有 Router 定下一跳；SubAgent / soft 裁决 / UI 都不调度。**⚠️ 这是"调度决策权唯一"，不是"同一时刻只能一个节点执行"——SDAN 明确支持并行（fan-out：多个无依赖节点可同时 `dispatched`、并发执行，见 `02` 并发节）。本不变量约束"谁决定下一跳"，不约束执行并发度；多节点同时在跑是特性、不是违反。**
2. **Router 无状态**：进度在 State、产出在 Blackboard，Router 崩了重启不丢。
3. **报文有归宿**：每个 Result 都被处理（放行 / 打回 / 回滚 / blocked 系统通知），不丢消息。
4. **join 不死锁**：DAG 无环 + 决策串行 + 只读 State 判定 → 无循环等待。
5. **回滚收敛**：闭包沿 DAG 向下、有限步；local retry 有上界（`max_retries` → blocked）；**结构回滚总次数有上界**——per-`(起点, violation)` 超 `max_rollback` → `blocked`，与 local 的 `max_retries → blocked` 对称（否则结构回滚死循环）。
6. **SubAgent 封闭**：只连 Router、只交 Result、不发起跨节点回滚。

## 本次纸面验证结论

- 3 个真矛盾（#4/#6/#8）解 / 推迟；5 个待定义（#1/2/3/5/7）已定。
- **已做第 4 层 walking skeleton 真跑** + Code Reviewer / codex 双评：骨架 31/31 过；自动闭包经注入测试（illustrator）证实是真 DAG 计算、非脑补。
- **真跑暴露并已修复 4 点**：
  1. 结构回滚无上界（死循环）→ 加 `max_rollback`（① per-起点/violation 超限 → blocked）；
  2. Result 无来源校验 → 加 in-flight 校验（② 只认在途节点交回的 Result）；
  3. scenario-root 双重身份 → 有效依赖图同源（③ 一份图算 root + join）；
  4. waiting 协议-实现空白 → 归 SubAgent 内部（④ 不设 Router 级 waiting）。
- 真相源已从"**纸面压测**"升级为"**经真跑 + 双评验证**"。
