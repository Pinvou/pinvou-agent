# 08 · 资源模块总览

> 架构层文档 · 通用 · v1 定稿 2026-05-31（白浪拍板）
> 资源模块分三类，本篇是入口。静态资产看 `08a-static-assets`，动态产出账本看 `08b-blackboard`。

## 0. 资源寻址原则（架构定性，最高优先级）

**Router 报文（Task 信封）里带「地址」，不带「内容」；SubAgent 凭地址自己访问。静态资产、动态产出一视同仁。**

资源怎么到 SubAgent——不是 harness 凭空往环境塞，而是 **Router 在 Task 信封里发放资源地址，SubAgent 凭信封里的地址 `read_file` 读取**。

为什么这条同时满足所有 SDAN 约束（不破铁律）：
- **SubAgent 仍只认信封**（`04` 铁律不破）：它拿到的所有地址都来自 Task 信封，没有 Router 没给的地址就读不到。不是"直连文件系统"那种环境访问。
- **控制核仍唯一**（`02` 不变量）：Router 决定每个 SubAgent 信封里放哪些地址（静态资产按 `reads_static` 筛 + 动态产出指针经 adapter 抽），掌握"谁能访问什么"。等于 **Router 做资源寻址授权，SubAgent 做凭授权读取**。
- **信封不臃肿**（弱模型友好）：只带地址不带内容（如 10KB base.css 不打包进信封），大文件不反复塞。
- **本就对齐 SDAN 底层事实**：`08b` Blackboard 本来就"只存指针、大产物留磁盘"；`03` 的 `inputs` 是 adapter 从 Blackboard 抽指针。这条只是把"信封带地址、凭地址读"显式定为统一范式。

> 比 MCP "agent 直连文件系统" 更克制：**地址发放权在 Router**。
> 实现落点：Task 信封带一个地址清单——`[STATIC]` 静态资产地址（按角色 `reads_static` 筛）+ `[BLACKBOARD]` 上游产出指针（adapter 从 Blackboard 抽）。两段都 Router 下发，SubAgent `read_file` 消费。harness spawn prompt 是承载方式，**语义归属 Router 报文**，不是环境注入。

## 1. 三类资源

| | 静态资产 Static Assets | 动态产出账本 Blackboard | 运行时配置 Runtime Config |
|---|---|---|---|
| 是什么 | 工作流自带的固定只读资源 | 角色运行时产出的可变账本 | spawn 时算出的路径/参数 |
| 例子 | `templates/base.css`、`reference/design_tokens.md`、L01-L05 母板、registry/route_table | brief.json、outline.json、page_layout.json、slides/*.html | 资源目录绝对路径、run_id、max_steps |
| 谁产 | **没人产**（工作流版本内置） | 某个 SubAgent 产 | Router/harness 算 |
| 生命周期 | = 工作流版本（打 bundle 时固定） | = 单次 run | = 单次 spawn |
| 持久化 | 否（bundle 内只读） | 是（`_state/` 原子写） | **否**（绝不进 Blackboard/State） |
| 访问 | Router 信封发地址 + `read_file` 懒加载 | Router 经 adapter 打包指针进 Task.inputs | spawn prompt 注入，不可写 |
| 详见 | `08a-static-assets` | `08b-blackboard` | 本篇 §3 |

**业界术语背书**（多份独立调研收敛到同一套，2026-05-31 资源模块调研）：
- arxiv 2603.22386 三层：静态资产 = ACG Template 层；route_table+scenario = Realized Graph 层；Blackboard = Execution Trace 层。
- MCP（Model Context Protocol）原语三分：静态资产 = **Resources**（应用控制、只读、URI 寻址）；Blackboard 写 = **Tools 的副作用**；Runtime Config = context 注入。
- 记忆类型：静态资产 = **程序性记忆**（只读规范）；Blackboard = **情节记忆**（本次产出）。

## 2. 为什么必须物理分开（第一性，非类比）

三类的**写权限 / 失效策略 / 持久化**三个物理属性全不同。把不同生命周期的东西塞进同一容器，必然"该清的没清 / 不该改的被改"。三条业界踩过的坑：

1. **写权限不同** → 经典 Blackboard 架构对所有知识源开放对称 read/write，静态参考数据和角色产出混在同一空间，任何 agent 能覆写固定资产。**这正是历史上 designer 能 write base.css 的根因。**
2. **失效策略不同** → 静态知识不该有 TTL，动态产出不该永不失效。混同导致静态资产被误清，或 stale 产出永久污染。
3. **持久化不同** → 静态路径若进 checkpointer，replay/rollback 时路径失效（LangGraph 已知坑）。所以 Runtime Config 必须独立、不落盘。

> **红线**：三类资源 schema 层物理隔离，不靠命名约定；validator 强制，不靠人工。文档也拆三篇（本篇 + 08a + 08b）用结构体现隔离。

## 3. 运行时配置 Runtime Config（简短）

spawn 时由 Router/harness 算出、注入 SubAgent 的**路径与参数**：资源目录绝对路径、项目目录、run_id、max_steps、timeout、allowed_tools。

- **生命周期 = 单次 spawn**：每次重新算，不复用。
- **铁律：绝不进 State / Blackboard**。这些是"这次怎么跑"的环境参数，不是"跑到哪了"的进度（State）、也不是"产出了什么"的账本（Blackboard）。混进去 → replay/rollback 时路径/参数失效（防 LangGraph 那类坑）。
- 来源：`constraints/allowed_tools` 来自 registry（`03` 已定）；路径来自 harness 解析（bundle 解包位置 + 项目目录）；run_id/max_steps 由 Router 调度算。

## 4. 单一职责（与资源分类配套）

**一个 SubAgent 只产单一语义类型的输出。** 判据：
- **"产出"** = 每次 run 不同的、该角色真正的决策结果（如 designer 的 page_layout = 每页用哪个母板）。
- **"复用静态资产"** = 固定不变、本可复用的资源（base.css/design_tokens.md）——**不该让角色重新生成**。

界定问题就问：「这东西每次 run 会变吗？是这个角色的决策吗？」**不变 + 非决策 = 静态资产，不该作为产出。** 详见 `08a` 的反模式说明。
