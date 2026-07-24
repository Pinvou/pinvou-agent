# SDAN 设计文档体系

**SDAN = Software Defined Agent Network（软件定义 Agent 网络）**

一个**固定 Router** + 一张**数据路由表** + 三个**旁挂模块**（⚖️裁决 / 🧠State / 📦Blackboard）+ 一群**只认信封的 SubAgent**。换 workflow 只换路由表，不动引擎。对标 SDN 的 control/data plane 分离。

> 本目录是 SDAN 架构的**真相源**。`architecture.html` 是架构总图（**改它必经白浪**）。
> **SDAN 是通用架构；PPT 工作流只是它的第一个实例**（见 `10`）。架构层（01–09、11）不含任何 PPT 专有细节。

## 文档清单与状态

✅ 定稿 · 🟡 部分待细化 · ⬜ 待设计

| # | 文档 | 职责 | 状态 |
|---|---|---|---|
| — | `architecture.html` | 架构总图：三平面 + 信封协议 + 三模块 | ✅ |
| 01 | `01-overview.md` | 总览 / 三平面 / 命名 / 设计原则 / 试金石 / 架构-vs-实例 | ✅ |
| 02 | `02-router.md` | Router 固定产品 / 处理循环 / 无状态四不 / `on_start` | ✅ |
| 03 | `03-protocol.md` | Task-Result 信封 / 头-vs-信纸 / header_signals / adapter / 全硬依赖 | ✅ |
| 04 | `04-subagent.md` | SubAgent 契约（只连 Router / 只交 Result / 不跨节点回滚）/ 收集类 | ✅ |
| 05 | `05-judge.md` | hard + soft 裁决器（固定提示词 / 无状态 / KV-cache） | ✅ |
| 06 | `06-route-table.md` | 路由表结构 + 节点 schema + 自动闭包回滚 | ✅ |
| 07 | `07-state.md` | 记忆模块（落盘JSON原子写 / 至少一次幂等恢复 / 状态机 v1 定稿） | ✅ |
| 08 | `08-resources.md` | 资源模块总览：三类（静态资产/动态产出账本/Runtime Config）+ §0 寻址原则（Router 报文带地址，SubAgent 凭地址访问）+ 物理隔离 | ✅ |
| 08a | `08a-static-assets.md` | 静态资产层：static_assets.json 清单 + reads_static 声明 + Router 信封发地址 + read_file 懒加载 + 反 designer 反模式红线 | ✅ |
| 08b | `08b-blackboard.md` | 动态产出账本：只存指针 + 原子写 + provenance/gate_status/run_id 隔离 + append-once | ✅ |
| 09 | `09-ui-plane.md` | 用户界面平面（卡片流）：DAG/节点卡片(UI 直读 State) + request_user_input 卡片 + 系统通知卡片 + soft 建议卡片，全部不经对话型 LLM；Engine 降级沉默宿主 | ✅ |
| 10 | `10-ppt-instance.md` | PPT 实例：10 agent / DAG / 回滚 / 场景（配 `../../route_table.json`） | ✅ |
| 11 | `11-validation.md` | 走查清单 + 接口对账 + 不变量 + 本次走查 8 条结论 | ✅ |
| 12 | `12-structured-output.md` | SubAgent 结构化产出：output_schema 合成 submit_output 工具 + stop 拦截不交不放行 + 字段级中文打回（≤3 次）+ 代码替它落盘 | ✅ |

## 阅读顺序

`01` → `architecture.html` → `02` → `03` → `04` → `05` → `06` → `09` → `07` → `08` → `08a` → `08b` → `10` → `11` → `12`

## 已钉死的关键决定（速查，详见各篇）

1. **三平面 + 三模块分离**：Router（控制，固定产品无状态）/ SubAgent（数据）/ 裁决·State·Blackboard（旁挂，独立于 Router）。
2. **Router 四不**：不存数据、不碰内容、不内置逻辑、不持有调度归属外的东西。
3. **协议**：信封头标准 + 信纸不透明；Router 只看头转发；翻译靠旁挂 adapter。
4. **SubAgent 契约**：只连 Router、只交 Result（完成/失败+原因）、不发起跨节点回滚。
5. **裁决**：hard（代码，先跑）→ soft（无人格/无状态/固定提示词的模块，非 SubAgent，KV-cache）；soft 不阻断、不打回、不回滚、不发起 structural，只上浮为用户可见的建议卡片。唯一硬门 = hard(代码) + 用户。
6. **回滚**：只声明 `rollback_to`，Router 自动算传递闭包；重跑覆盖。三源 = 自身失败(local) / qa violation(structural) / 用户反馈(推迟)。
7. **用户界面平面**：用户面对 DAG/节点卡片(UI 直读 State) + request_user_input 卡片 + 系统通知卡片 + soft 建议卡片，全部不经对话型 LLM；无聊天窗口、无品悟角色。启动由 UI 触发 Router `on_start`。blocked 由系统直发卡片。用户干预走控制平面(卡牌 + route_table)，无自然语言入口（现阶段推迟）。Engine 降级为沉默宿主（持 client/manager/事件通道/request_user_input→卡片 tx），不可删。
8. **可复制/可生成**：换 workflow 只换路由表。

## 待办

- ✅ **State / Blackboard 持久化** v1 已定（落盘 JSON 原子写 + 至少一次/幂等恢复 + 指针式 Blackboard）→ `07` / `08`
- ✅ **用户界面平面 v1**：`09` 重写为卡片流（DAG/节点卡片 + request_user_input + 系统通知 + soft 建议），取消对话型品悟角色 → `09`
- **推迟**：用户需求变更（Pinvou tools + 运行时改表）/ 软依赖（现全硬）/ 回滚 cascade 细粒度优化
- **实施第一步**：最小可运行骨架真跑（`11` 第 4 层验证）
