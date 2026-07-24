# 06 · 路由表

> 架构层文档 · 通用 · 路由表是 Router 加载·解析·执行的**数据**

## 定位

**Router 引擎不变，调度逻辑全在路由表（数据）。换 workflow 只换表。** 这张表可由上层（如 Pinvou3）生成——这是 SDAN "可复制 / 可生成"的落点。

路由表**引用** registry（`tools` / `outputs` / `output_schema` / `max_steps`），不重复定义（registry 是唯一真像）。

## 顶层结构

```
route_table = {
  nodes:          { <node_id>: NodeSpec },   // 每个节点的裁决/契约/出口
  edges:          [ (from, to) ],            // 转发拓扑（DAG）
  adapters:       { (from,to): AdapterSpec },// 每条边的格式翻译
  joins:          { <node>: [硬上游...] },    // 谁要等齐谁（现阶段全硬）
  rollback_rules: { <violation_type>: { rollback_to } },  // 只声明起点
  scenarios:      { <scenario>: { 激活子集, 依赖重写 } },
}
```

## NodeSpec（节点 schema）

```jsonc
{
  "accepts":  { "<字段>": "← <上游>.produces 的某部分" },
  "produces": "← registry.outputs + output_schema + signals",
  "dispatch_mode": "single | per_page",   // 可选，缺省 single；见下「派发模式」
  "hard":     [ "<代码规则脚本>" ],
  "soft":     { "criteria": "<裁决标准>" } | null,
  "header_signals": [ "verdict", "violation_type?" ],
  "outcomes": {
    "pass":            { "unlock": [ "<下游>" ] },
    "warn":            "同 pass + 记账",
    "fail_local":      { "respawn": "self", "max_retries": N },
    "fail_structural": { "rollback_to": "<起点节点>" }    // cascade 不写，Router 自动算
  }
}
```

## 派发模式（dispatch_mode）：纵向 fan-out

SDAN 既有的并行是**横向 fan-out**——多个无依赖节点可同时 dispatched（见 `02` 并发节、`11` 不变量①）。`dispatch_mode` 增补**纵向 fan-out**：**一个节点按一个列表拆成 N 个「自己」的实例并发执行**。

- `single`（缺省）：节点派 1 个 SubAgent，行为不变。
- `per_page`：派发该节点时，Router 据 registry 的 `dispatch` 块（`over` 指向的列表）把它拆成 **N 个 per-page SubAgent 并发**（受 `MAX_SUBAGENTS` 并发上界节流）；每个实例只产列表第 i 项对应的产物（如 `slides/p{page:02d}.html`）。能力细节（拆什么列表、产物模板）在 **registry.<agent>.dispatch**；路由开关在此。
- `over` 来源类型：`outline.slides`＝大纲页清单（slide_writer）；`html_image_slots`＝扫 `slides/*.html` 的 data-image 声明、**按含图位的页分组每页一项**（illustrator，一页一个写者、规避同页 CSS 并发互踩），**0 项＝空批次＝节点直接 completed**。
- 实例「完成即真」校验由 `registry.<agent>.dispatch.realness` 声明（`html_page`／`image_file`），Router 按声明校验，不在 runtime 写角色知识；未过即单实例重试。批次 total ＝任务枚举结果数（**不是** outline 页数）。
- **不变量（不能弄错）**：fan-out 节点在 `edges`/`joins`/`scenarios`/`rollback` 里**仍是单一逻辑节点**，这些拓扑引用**一字不改**。只有两件运行时行为变：①派发 1→N；②节点 status 只在 **N 个实例全 done** 时才置 `completed`。于是下游 `join`（检查节点 status）天然等齐全部实例；回滚到本节点=重派整批；scenario 操作单节点（per_page 展开在 apply_scenario 之后、dispatch 时发生）。
- **重试粒度**：local fail（单实例失败/单页 gate 不过）只重派**那一个实例**；structural 回滚才整节点重来。

## 回滚：只声明起点 + 上界，cascade 自动算闭包

- `rollback_rules` / `fail_structural` **只声明 `rollback_to`（起点）+ `max_rollback`（上界）**。
- **cascade 由 Router 按 `edges` 自动算传递闭包**：起点 + DAG 上所有向下可达节点，全部重置重跑。**不手写 cascade 列表**（手写必漏，是 bug 温床）。
- **结构回滚有上界（①，否则会死循环）**：Router 维护 per-`(rollback_to, violation_type)` 回滚计数；同一回滚超 `max_rollback` 仍 FAIL → **不再回滚，转 `blocked` + 系统消息**（对称 local retry 的 `max_retries → blocked` 逃生；这是 walking skeleton 真跑暴露的设计缺口，见 `11`）。
- 重跑**直接覆盖**旧产出（不搞版本/stale）。
- **attempts 语义**：`attempts = 1（初始 dispatch）+ 已重试次数`；`attempts > max_retries` 即 local 耗尽 → blocked。

**回滚的三个来源**（执行权全在 Router）：
1. **SubAgent 自身失败**（交 failed Result）→ Router `local` 重派它自己（≤max_retries）→ 耗尽 `blocked`。
2. **检查类节点 hard 出 `violation_type`** → `structural` 回滚到对应起点 → 自动闭包。
3. **用户干预**（改控制平面：卡牌 + route_table，无自然语言入口，**现阶段推迟**，见 `09` 用户界面平面 / `05` soft 不阻断）。

## join（现阶段全硬）

`joins` 声明节点的硬上游列表；Router 等齐所有硬上游 `done` 才派该节点。现阶段**全硬**，无软依赖（推迟）。

## scenarios：场景裁剪

不同场景激活不同节点子集 + 重写某些节点的依赖来源。Router 在 `on_start` 时 `apply_scenario`：先剪枝 + 重写依赖，再开跑。

**`apply_scenario` 产出"一份有效依赖图"（③ 同源，不留两套）**：剪掉非激活节点 + 重写依赖边，得到本场景唯一的有效 DAG；之后 root（无上游者）和 join（多上游者）**都基于这一份算**，不分别维护两套拓扑。被剪掉的节点，其下游的 join 自动不再等它（它已不在有效依赖图里，不计入硬上游列表）。

## 加载执行流

```
load(route_table) → on_start(scenario): apply_scenario → 派 roots
                  → on_result 循环（见 02-router）
```

> 本篇是**通用结构**。一张真实的路由表（PPT 工作流）见 `10-ppt-instance` 与 `../../route_table.json`。
