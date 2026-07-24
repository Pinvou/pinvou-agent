# 02 · Router（固定产品）

> 架构层文档 · 通用

## 定位：一个固定的产品 / 模块

Router 是 SDAN 的**标准件**，职责只有两条，再无其他：

1. **加载 · 解析 · 执行路由表**（通用执行器，跟具体是什么 workflow 无关）。
2. **严格按 SDAN 协议收发、处理报文**（报文 = Task / Result 信封，见 `03-protocol`）。

因此 Router 与具体业务彻底解耦：**换 workflow 只换路由表，Router 不变。** 它就像你买来的网络路由器——加载配置就用，不关心网里跑什么业务。

> 这意味着存在一个**协议/实现分层**：SDAN 协议是规范，Router 是它的**参考实现**。协议定死后，谁（包括上层生成器）都能照着写路由表，不用懂 Router 内部。

## 无状态 · 四不

- **不存数据** —— 进度在 🧠State，产出在 📦Blackboard，都在 Router 外面。
- **不碰内容** —— 只看信封头转发，不解析信纸。
- **不内置逻辑** —— 质量判断旁挂给 ⚖️裁决；内容翻译旁挂给 adapter。
- **不持有调度归属之外的东西** —— 三模块只读写/调用，不拥有。

无状态收益：**Router 崩了重启照样接着跑**（进度本就在 State 里）。

## 处理循环

事件驱动的纯逻辑，每来一个 **Result 报文**跑一遍（🧠=State，📦=Blackboard，⚖️=裁决）：

```python
def on_result(report):                       # report = Result 信封（头 + 信纸）
    # 0) 合法性校验 ── 只认 in-flight Result（②）
    if not state.is_inflight(report.from, report.task_id):
        return drop_and_log(report)          # 伪造/重复/过期/未知 sender → 丢弃 + 记日志，不处理

    node = route_table.nodes[report.from]

    # 1) 登记产出 ──────────────── 📦 资源
    blackboard.put(report.from, report.produces)      # 大文件留盘，只登元数据/引用

    # 2) 裁决 ─────────────────── ⚖️（旁挂，不内置）
    verdict = run_hard(node.hard, report.outputs)     # 代码规则，确定性先跑——唯一定路由的裁决
    if verdict == PASS and node.soft:
        advisory = ask_soft(node.soft, report)        # soft 只出建议卡片：不改 verdict、不阻断（见 05/09）
        emit_advisory_card(report.from, advisory)     # → UI 建议卡片
    #  只有 hard 的 verdict 写进信封头：PASS / WARN / FAIL(+local/structural) / violation_type

    # 3) 记进度 ───────────────── 🧠 记忆
    state.update(report.from, status=…, attempts+=1, last_verdict=verdict)

    # 4) 查表定下一跳（只看头，不拆信纸）
    nexts = resolve(node.outcomes, verdict):
        PASS / WARN       → node 解锁的下游
        FAIL/local        → [report.from]                     # 打回自己（attempts > max_retries → blocked）
        FAIL/structural   → 若 (rollback_to, violation) 回滚数 < max_rollback：自动闭包重跑（见 06）
                            否则 → _blocked（①，对称 local 的 retry 耗尽逃生）
        # 任何 verdict 都有归宿；未知 violation / 未注册 → _blocked，不丢不崩（③报文有归宿）

    # 5) 对每个下一跳封 Task 发出
    for Y in nexts:
        if not join_ready(Y, state):  continue            # 🧠 硬上游没齐 → 挂起
        inputs = run_adapter(route_table.adapters[(node, Y)], blackboard)
        state.mark_inflight(Y)                            # 记 (Y → task_id)，供 step0 校验
        send(Task(to=Y, reason=…, inputs=inputs, constraints←registry))
```

**一句话**：先验报文合法 → 查表 → 调旁挂裁决 → 定下一跳 → adapter 打包 → 转发。

## 冷启动：`on_start` 入口（用有效依赖图）

第一棒没有上游 Result，所以有 `on_start` 入口。**③：先把本场景的"有效依赖图"算出来（剪枝+重写依赖），root 与 join 都基于它——同一份关系，不留两套。**

```python
def on_start(scenario, user_request):
    eff = route_table.apply_scenario(scenario)   # ③ 产出有效依赖图（剪掉非激活节点 + 重写依赖）
    for r in eff.roots():                        # root = 有效图里无上游的节点（与 join 同源，见 06）
        send(Task(to=r, reason="start", inputs=run_adapter_for_root(r, user_request), …))
```

由 UI 启动动作触发 `on_start`（见 `09`）。

## blocked 的去向（系统直发）

两条路进 blocked：① 某节点 local 重派耗尽（`attempts > max_retries`）；② **同一 `(rollback_to, violation)` 结构回滚超 `max_rollback`**（①）。

进 blocked 后：Router 直接生成一条**系统通知卡片**上浮到用户界面（卡片流），如「⚙️ 工作流系统：density 类问题回滚 2 次仍未解决，卡在 X」。闭包/下游标 `blocked-upstream`，并行链照跑，等人工决定。详见 `09` 与 `11`。

## 并发

多个 Result 同时到达就**排队，Router 串行处理路由决策**——决策串行保证读写 State 无竞争（也是 join 不死锁的前提）；派发出去的 SubAgent 可并行跑。

## 边界

- **里面只有**：路由表加载/解析/执行引擎 + 协议（信封）处理 + 合法性校验 + `on_result`/`on_start` 入口。
- **外面（可换/可生成）**：路由表、⚖️裁决、🧠State、📦Blackboard、adapter 函数。
- **没有任何具体 workflow 的影子。**
