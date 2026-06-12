# 03 · SDAN 协议

> 架构层文档 · 通用 · 这是 Router 与 SubAgent 之间的契约规范

## 铁律：只通过两种信封通信

agent 与 Router 之间**只有两种报文**：Task（下发）/ Result（上交）。SubAgent **只认信封**——看不到路由表、看不到别的 agent、看不到全局状态。这是 SubAgent 与 Router 解耦的根。

## Task Envelope（Router → Agent，下发任务）

```jsonc
{
  "task_id": "<run>/<node>/attempt-<n>",
  "to":      "<node_id>",
  "attempt": 1,
  "reason":  "start | dispatch | local_retry | rollback",   // 为什么派给你
  "inputs":  { /* 已按 to 的 accepts 打包好的输入（adapter 产出） */ },
  "feedback": null,            // local_retry / rollback 时填具体修改意见
  "constraints": { "max_steps": …, "timeout_secs": …, "workspace_rule": … },  // ← registry
  "allowed_tools": [ … ]       // ← registry，决定它能干什么
}
```

| 字段 | 含义 | 来源 |
|---|---|---|
| `to/attempt/reason` | 派给谁、第几次、为什么 | Router（读 State） |
| `inputs` | **已翻译成本节点标准的输入** | adapter 从 Blackboard 抽 |
| `feedback` | 打回/回滚时的具体意见 | 上轮 hard findings（soft 只产建议卡片、不进 feedback） |
| `constraints/allowed_tools` | 执行上界 + 工具白名单 | **registry（唯一真像）** |

## Result Envelope（Agent → Router，上交产出）

```jsonc
{
  "task_id": "…",
  "from":    "<node_id>",
  "status":  "completed | failed",     // 只有两种，没有第三种（SubAgent 契约）
  "outputs": [ { "ref": "…", "kind": "…" } ],   // 产物引用（文件路径等）
  "produces": { /* 结构化产出 + 暴露给下游的协议数据（signals） */ },
  "reason":  null              // status=failed 时填失败原因（含"需要用户提供X"这类）
}
```

> 注意：**没有 `need_input` 这种状态**。SubAgent 缺信息干不下去，就是 `failed + reason="需要X"`；而"主动跟用户要信息"是收集类 SubAgent 用自己的卡片通道做的（见 `04-subagent`），不靠新增 Result 类型。

## Router 只认 in-flight Result（②，来源校验）

Router 处理 Result 前**先验合法性**，只认"在飞"的报文：

- 派发时记 `(node → task_id)`，并把该节点标 `dispatched`。
- 收 Result 时校验：**`task_id` 匹配 + sender 当前是 `dispatched`**。
- 不匹配——伪造（冒充别人解锁下游、越过 join）、重复、过期、未知 sender、未注册节点——**一律拒收丢弃 + 记日志**，不进处理、不抛异常。

这保证了不变量 ①控制核唯一 / ③报文有归宿 / ⑥SubAgent封闭 的边界：外部塞不进假报文来篡改调度。

## 信封头 vs 信纸（协议的命门）

照搬网络协议的智慧——**协议只规定"信封头"，不规定"信纸里装什么"**：

- **信封头 = 标准路由元数据**：`to/from/reason/status`，以及 Router 做转发决策要用的信号（`verdict`、检查类节点的 `violation_type` 等）。**Router 只看头转发。**
- **信纸 = 不透明载荷**：`inputs/produces` 的实际内容（JSON / HTML / 图片 / 长文本……任意）。**Router 从不解析信纸。**

收益：**数据内容差异再大，Router 一视同仁地转发**——内容的理解是上下游 agent（端到端）的事，不是 Router 的事。

**推论（设计时最要下功夫的地方）**：凡 Router 做转发决策要用的信息，**必须上浮到信封头**；大块内容留信纸。

## `header_signals`：哪些上浮到头

每个节点在路由表里声明它的 Result 要把哪些信号**上浮到信封头**，使 Router 不拆信纸就能定下一跳。典型：

- `verdict`（PASS/WARN/FAIL）——每个节点都要。
- `violation_type`——检查类节点（裁决出多种结构性问题、需分流回滚的）要。

## adapter：格式适配（"对面是谁用谁的标准"）

每条边 `from → to` 挂一个 adapter，把 `from.produces` 翻译成 `to.accepts`：

- **声明式字段映射**：抽某几个字段、改名、过滤——写在路由表里（数据）。
- **旁挂函数**：复杂转换（如从某产出里扫描提取一份结构化清单）——路由表只声明"调函数 X"，**Router 调用、不内置逻辑**（与裁决旁挂同模式）。
- 同一份上游产出，转发给不同下游可走不同 adapter。

## accepts / produces 契约

- `produces` = 节点产出什么 = 产物引用 + 结构化 schema + 暴露的 signals。
- `accepts` = 节点要什么输入 = 字段清单 + 每个字段来自哪个上游的 `produces`。
- 每条边的 adapter 让 `produces → accepts` **闭合**（对账见 `11-validation`）。

## 依赖：现阶段全硬

**现阶段所有上游依赖按硬 `join`**（等齐所有上游 done 才派下游）。"软依赖（用但不等）"推迟（TODO），见 `11-validation` 走查 #2。
