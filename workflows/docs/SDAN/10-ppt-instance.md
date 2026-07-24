# 10 · PPT 工作流实例

> **实例层** · PPT 工作流是 SDAN 的**第一个实例**——把角色/依赖/回滚/场景写成一张路由表喂给同一个 Router。
> 配套路由表：`../../route_table.json`。本篇用 SDAN 术语重述。

## 10 个 SubAgent（节点）

| # | role_id | 中文 | produces | hard | soft（裁决标准） | 备注 |
|---|---|---|---|---|---|---|
| 1 | requirements_analyst | 需求分析 | `_state/brief.json` | validate_deliverable | brief 完整无歧义 | **收集类**（5 问，`request_user_input`） |
| 2 | materials_auditor | 素材审计 | inventory + gaps | warn_only（标缺口） | — | **收集类**；WARN-pass 不卡 |
| 3 | researcher | 调研 | `_research/*.md` | 存在非空 | —（交 architect 拍） | |
| 4 | product_manager | 产品分析 | `product_brief.md` | 存在非空 | — | |
| 5 | solution_architect | 方案架构 | `solution_design.md` | 存在非空 | 痛点-能力一致 | **join**[researcher, product_manager] |
| 6 | content_planner | 内容策划 | `outline.json`（submit_output） | title+slides 结构合法 | ghost deck 节奏 | 只产 outline.json；**框架不在此产**——框架/slides 骨架的实例化是 designer 的业务，经 compose_deck tool 出（见行 7 + `02-router` Router 四不：不碰内容/不内置逻辑，业务脚本不归 Router）；base.css 是静态资产（08a）。density 回滚**落点** |
| 7 | designer | 视觉设计 | `_state/page_layout.json` + slides 框架（经 compose_deck） | 存在 | 设计 token/版式 | 框架/slides 骨架实例化由 designer **经 compose_deck tool** 完成（SubAgent 业务，**非 Router**——`02-router`：Router 不跑业务脚本）；base.css/design_tokens 是静态资产，只读不产（08a） |
| 8 | slide_writer | 页面撰写 | `slides/*.html`（成型页上填槽**直出本页 HTML**） | 纯校验：逐槽字数/必填非空/框架一致性（零副作用） | 内容与大纲一致 | narrative 回滚落点；**`dispatch_mode=per_page`** 单一逻辑节点（见 `06`）；**先框后字（白浪 2026-06-05 拍板，见 reference/text_slot_protocol）**：版式/文本框/图层由模板实例化定死，**框架由 designer 经 compose_deck 实例化定死（SubAgent 业务，非 Router——`02-router` Router 四不）**，writer 读成型页只往 data-slot 节点填字、槽外一字不动、完成标 data-filled |
| 9 | illustrator | 配图 | `scenes/*.png` | 逐槽：声明槽全有真图＋图层契约（VLM 四维待 `vlm_page_check` 工具落地） | — | **join**[slide_writer]（design_tokens 走 reads_static，不依赖 designer）；**`dispatch_mode=per_page`**：按含 data-image 声明的页拆 N 个单页 SubAgent、每实例处理该页全部槽位（0 页=空批次即过），DAG 里仍是单一逻辑节点（见 `06` 派发模式） |
| 10 | qa_inspector | 质检 | `gate_report.json`（submit_output） | 结构(audit_format tool) | — | **join**[slide_writer, illustrator]；**唯一 structural 发起点**；审计经 tool（`audit_format` 结构/字号/渲染 + `vlm_page_check` 视觉），不在 subagent 跑 shell。详见 specs/2026-06-02-qa-audit-tools |

## DAG（edges + join）

```
requirements_analyst → { materials_auditor, researcher, product_manager }
{ researcher, product_manager } → solution_architect            (join)
solution_architect → content_planner → designer
designer → slide_writer
slide_writer → illustrator                                       (illustrator join: slide_writer；design_tokens 走 reads_static 静态资产，illustrator 不再依赖 designer)
{ slide_writer, illustrator } → qa_inspector                     (join)
```

## 回滚（qa 的 `violation_type` 分流 → Router 自动闭包）

qa_inspector hard FAIL → 信封头 `violation_type`，Router 查 `rollback_to` 后**自动算传递闭包**：

| violation_type | rollback_to | 自动闭包（Router 按 DAG 算，无需手写） |
|---|---|---|
| `density_violation` | content_planner | content_planner → designer → slide_writer → **illustrator** → qa |
| `narrative_flow_broken` | slide_writer | slide_writer → illustrator → qa |
| `image_quality_failure` | illustrator | illustrator → qa |

> **注**：旧 `workflow.json` 手写的 `cascade` 列表**废弃**——它漏了 illustrator（density 回滚后 slide_writer 重写、图位变，但旧 cascade 没重跑 illustrator）。改自动闭包后修复，见 `11` 走查 #4。
> **推迟**：`user_feedback` / `design_direction_rejected`（用户主动变更类）——见 `09` 推迟项。

## header_signals

- `qa_inspector`：上浮 `verdict` + **`violation_type`**（Router 据此分流三条 structural 回滚）。
- 其他节点：上浮 `verdict`。

## 收集类节点

`requirements_analyst`（向用户 5 问澄清需求）、`materials_auditor`（清点素材、标缺口）——`allowed_tools` 含 `request_user_input`，走**卡片通道**跟用户问答（**不经品悟**，见 `04`/`09`）。

## 场景裁剪（scenarios）

| scenario | 激活节点 | content_planner 上游重写 |
|---|---|---|
| solution_deck | 全 10 节点 | solution_architect |
| research_report | 去 product_manager / solution_architect / illustrator | researcher |
| product_intro | 去 researcher / solution_architect | product_manager |
| internal_quick | 仅 req / content_planner / designer / slide_writer / qa | requirements_analyst |

## 配套

完整节点 schema、adapter、joins 见 `../../route_table.json`。
