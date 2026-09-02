# 按时间范围查询待办 — `wecom-cli todo list`

按创建时间或截止时间范围拉取当前用户创建和参与的待办列表，支持按状态过滤。返回含 `title` / `description` / `followers` / `deadline` 等完整字段，多数场景无需再走本技能的「批量查询待办详情」。

本接口用于直接查看待办列表、确认待办状态、查询特定待办，或在修改、完成、删除前定位目标待办。

## 命令

```bash
wecom-cli todo list --json '<JSON 参数>' [--page-count N]
```

`--page-count N` 自动翻页并最多拉取 N 页的内容（默认 1，即只拉首页）。不传则只拉首页。注意 `--page-count` 是命令行参数，写在 `--json '...'` 之外，不要塞进 JSON 体里。

## 参数

查询接口不进 `items` 壳，参数直接平铺：

| 字段 | 类型 | 必填 | 默认值 | 语义 |
|---|---|---|---|---|
| `create_begin_time` | string | 否 | — | 创建时间起始，格式 `YYYY-MM-DD HH:mm:ss` |
| `create_end_time` | string | 否 | — | 创建时间截止，格式 `YYYY-MM-DD HH:mm:ss` |
| `deadline_begin_time` | string | 否 | — | 截止时间起始，格式 `YYYY-MM-DD HH:mm:ss` |
| `deadline_end_time` | string | 否 | — | 截止时间截止，格式 `YYYY-MM-DD HH:mm:ss` |
| `status_filter` | string[] | 否 | — | 状态过滤，合法枚举值只有 `finished`（已完成）、`proceed`（进行中），可多选；不传时默认只返回 `proceed`（进行中）的待办 |
| `keywords` | string[] | 否 | — | 关键词过滤，对待办文本（标题/描述）做命中匹配。数组元素之间是 **OR**，单个元素内空格分隔的词是 **AND**。语义与构造方式详见下方「keywords 语义」 |
| `limit` | integer | 否 | 10 | 单次返回数量，不传为10，最大只能传20，如果需要大量查询，应该使用自动翻页 |
| `cursor` | string | 否 | — | 分页游标，首次请求不传 |

示例入参（按时间范围 + 状态过滤 + 关键词 + 可选的翻页参数）：

```json
{
  "create_begin_time": "2026-05-01 00:00:00",
  "create_end_time": "2026-05-09 23:59:59",
  "status_filter": ["proceed"],
  "keywords": ["报销"],
  "limit": 20,
  "cursor": "<上次返回的 next_cursor>"
}
```

> 各顶层过滤条件之间是 **AND** 关系：一条待办需同时满足时间范围、状态、关键词表达式才会被返回。`keywords` 内部再按下方规则展开自己的 OR/AND 逻辑。

## keywords 语义

`keywords` 用两层结构表达"或"与"且"：

- **数组多个元素之间 = OR**：命中任意一个元素即召回。
- **单个元素内空格分隔 = AND**：该元素里的每个词都命中，才算命中这个元素。

例：`["service ai", "claw"]` 等价于布尔表达式 `("service" AND "ai") OR "claw"`——"同时包含 service 和 ai"或"包含 claw"的待办都会被召回。

从用户表达构造 `keywords`：

| 用户说 | keywords | 含义 |
|---|---|---|
| "包含报销的待办" | `["报销"]` | 命中"报销" |
| "同时提到项目和评审的待办" | `["项目 评审"]` | 一个元素、空格分隔 = "项目" AND "评审" |
| "提到报销，或者同时提到项目和评审的待办" | `["项目 评审", "报销"]` | `("项目" AND "评审") OR "报销"` |

## 返回

| 字段 | 类型 | 语义 |
|---|---|---|
| `items` | array | 待办列表，每项结构见下 |
| `next_cursor` | string | 下一页游标，配合 `has_more=true` 使用 |
| `has_more` | boolean | 是否还有更多数据 |

`items[]` 元素结构：

| 字段 | 类型 | 语义 |
|---|---|---|
| `todo_id` | string | 待办 ID（前缀 `td`） |
| `title` | string | 待办标题 |
| `description` | string | 详细描述 |
| `status` | string | 待办整体状态：`finished` / `proceed` |
| `user_status` | string | 当前用户在该待办的状态：`accept` / `reject` / `finished` / `removed` / `notshow` |
| `creator` | object | 创建人，含 `userid`（前缀 `wo`） / `user_name`（格式 `英文名(中文名)`） |
| `followers` | array | 分派人列表，每项含 `userid`（前缀 `wo`） / `user_name` / `user_status` / `update_time` |
| `deadline` | object | 截止时间；结构见 SKILL.md `deadline` 对象规范。无截止时间时不返回或为 `null` |
| `extra_info` | string | 提醒时刻只读信息，可能不提醒 |
| `source` | string | 待办来源：`single_chat`（单聊）/ `group_chat`（群聊）/ `doc`（文档）/ `ai_summary`（智能总结）/ `meeting_summary`（会议纪要）/ `face_chat`（「面聊」功能）/ `fused_doc`（融合文档）/ `smart_sheet`（智能表格）/ `smart_doc`（智能文档）/ `JSAPI`（JSAPI） |
| `create_time` | string | 创建时间，格式 `YYYY-MM-DD HH:mm:ss` |
| `update_time` | string | 更新时间，格式 `YYYY-MM-DD HH:mm:ss` |

## 使用规则

- **用户按状态查询待办时，`status_filter` 必须显式传对应状态**：本接口不传 `status_filter` 时只会返回进行中（`proceed`）的待办。用户问"已完成的待办"要传 `["finished"]`，问"所有待办（含已完成）"要传 `["finished","proceed"]`。漏传会导致已完成的待办根本不在结果里，进而把"其实有"误判成"没有"。
- **禁止用 `status_filter` 查询已删除待办**：该字段只接受 `finished` / `proceed`，不得传 `deleted`。用户要求查询已删除待办时，应直接说明当前列表接口不支持按已删除状态查询。
- **时间范围默认归到创建时间**：用户给了"5 月 1 号到 5 月 9 号""上周""本月"这类时间范围、但没点明是"创建"还是"截止"时，默认填 `create_begin_time` / `create_end_time`。一段时间范围最自然的含义是"这段时间内记下/产生的待办"。只有用户明确带"截止 / 到期 / deadline / ddl / 这之前要做完"等字样时，才改用 `deadline_begin_time` / `deadline_end_time`。
- **统计、计数、"有哪些"类需求要基于全量数据**：这类需求必须翻完所有分页（`--page-count` 取足够大，直到某页 `has_more` 为 `false`）。若结果过大被转存到文件，要把整个文件读完整再统计——只读开头几页就下结论会严重少算。
- **如果用户意图是获得所有待办**：应使用 `--page-count N` 快速拉取所有分页，直到 `has_more=false`。
- **已含完整详情**：`followers` / `creator` 已是包含人名的对象，多数场景无需再走本技能的「批量查询待办详情」或使用 `wecomcli-contact` 反查。
- **修改和完成待办时**：`status_filter` 可一次传多个状态。修改通常查进行中即可；完成或确认是否已完成时，应传 `["finished","proceed"]`，避免把已完成误判为未找到或再执行后续操作。
- **删除/退出某个待办时**：`status_filter` 应该传入 `["finished", "proceed"]`，不然可能找不到。删除接口对创建人是删除整条待办，对非创建人是退出/从自己的待办中移除；列表返回的 `creator` / `user_status` 用于判断操作语义和避免重复操作，**不要因为当前用户不是创建人就拒绝删除请求**。
- **默认只返回 10 条**：如需查全部请显式传 `limit` 为更大值，并关注 `has_more` / `next_cursor` 分页；要一次性拉多页可加 `--page-count N`。
- **`keywords` 是对待办系统记录的字面命中过滤，不是语义检索**：它只匹配待办自身的标题/描述文本。

## 返回给用户的格式

> **适用范围**：仅当用户**直接询问待办列表**（如"我今天创建的待办"）时才使用本格式。若 `list` 是被其他操作（修改 / 完成 / 删除待办时为定位 `todo_id`）内部调用，本格式不适用——按对应操作的流程返回，不要把列表展示给用户。

将 `items` **按状态分组**呈现，每个状态分组下用 Markdown 列表展开，每条待办占多行：

```markdown
## 进行中（N 条）

1. <title>
  - 创建人：<creator>
  - 参与人：<followers>
  - 截止时间：<deadline>

## 已完成（M 条）

1. <title>
  - 创建人：<creator>
  - 参与人：<followers>
  - 截止时间：<deadline>
```

字段映射：

- **分组标题**：按 `status` 中文化分组
  - `proceed` → `## 进行中（N 条）`
  - `finished` → `## 已完成（M 条）`
  - 某分组无数据则整个分组省略
- **标题**：`title`
- **创建人**：`creator.user_name`，如果创建人是用户自己，则缺省
- **参与人**：`followers[].user_name` 用 `、` 拼接；无参与人时缺省
- **截止时间**：`deadline.value`；无截止时间时缺省

> 排序：分组内按 `deadline.value` 升序（无截止时间的排在最后）；同一组内截止时间相同时按 `update_time` 倒序。
