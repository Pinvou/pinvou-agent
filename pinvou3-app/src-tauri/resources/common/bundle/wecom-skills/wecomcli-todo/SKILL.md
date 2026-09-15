---
name: wecomcli-todo
description: 何时用:仅当用户明确指向企业微信待办(建/查/改企微待办)时使用;泛指记个待办/提醒默认走本地工具,不要误用。企微待办:创建、删除或退出、完成、查询和筛选,以及修改标题、描述、参与人名单和截止时间。

metadata:
  requires:
    bins: ["wecom-cli"]
---

# 企业微信待办管理

> 执行任何 `wecom-cli` 命令前，必须先读取并完成 `wecomcli-shared` 技能的公共前置检查。

使用 `wecom-cli` 管理企业微信待办。

## 查询与定位

- 查询范围仅限企业微信待办系统中已经存在的记录。
- 可按创建时间、截止时间、完成状态和标题/描述关键词查询；关键词是字面匹配，不是语义搜索。
- 用户问"我有哪些待办""未完成待办有哪些"或"接下来有哪些待办"时，使用 `todo list` 查询待办系统中的记录。
- 删除、完成或更新时，若上下文没有 `todo_id`，先用 `todo list` 定位；已有 `todo_id` 且需要确认最新详情或状态时，使用 `todo get`。

## 接口路由表

**[重要事项]** 执行任何操作前，必须先定位「接口路由表」指向的参考文档并完整读取，再执行命令，避免出现参数错误。严禁凭路由表描述或自身记忆猜测拼参数。

| 用户意图 | 参考位置 |
|---|---|
| 创建待办（可选分派） | references/todo-create.md |
| 删除待办 / 退出待办 / 从我的待办中移除 | references/todo-delete.md |
| 完成当前用户自己的部分 / 将整条待办全部完成 | references/todo-finish.md |
| 已有 `todo_id` 时确认待办详情和最新状态 | references/todo-get.md |
| 查看待办列表；按创建时间、截止时间、完成状态或关键词筛选；为后续操作定位待办 | references/todo-list.md |
| 修改待办内容 / 分派人名单 / 截止时间（不含参与人状态） | references/todo-update.md |

## `deadline` 对象规范

待办的截止时间统一以 `deadline` 对象表达。涉及"设置截止时间"、"修改截止时间"、"清空截止时间"或读取待办的截止信息时，按本节规范处理。

### 结构

| 字段 | 类型 | 必填 | 语义 |
|---|---|---|---|
| `type` | string | 是 | 枚举：`date`（仅日期，如果用户没有提及具体时分秒，则一定选择`date`） / `datetime`（用户提及了具体时刻） |
| `value` | string | 是 | `type=date` 时格式 `YYYY-MM-DD`；`type=datetime` 时格式 `YYYY-MM-DD HH:mm:ss` |

### 在 deadline / remind_at_deadline 字段上的语义

- **设置或修改 `deadline`**：整体可选；若提供则其内部 `type` 与 `value` 必填。
- **清空已设置的截止时间**：将 `deadline` 字段更新为空对象 `{}`；不更新该字段则保持原值不变。
- **作为返回字段**：未设置截止时间的待办，`deadline` 字段不返回或为 `null`。
- **提醒时机（`remind_at_deadline`）**：`remind_at_deadline` 与 `deadline` 是一对，必须一起出现——脱离 `deadline` 单独传 `remind_at_deadline` 不会生效，不要这么传。`remind_at_deadline` 只决定提醒**时机**，入参层面**没有"关闭提醒"这一档**（是否真正提醒由后台判断，可能因不满足条件而不提醒，以返回的 `extra_info` 为准）：
  - `remind_at_deadline=true`（仅 `deadline.type=datetime` 可传）→ 在**截止时刻**提醒。
  - `remind_at_deadline=false` 或不传 → 按**后台默认的提前时间**提醒（**不是关闭提醒**）。
  - `deadline.type=date` 或未传 `deadline` 时不要传 `true`。

### 从用户输入推断 `deadline`

日期/星期直接限定待办中的任务或事件时，也视为截止日期。例如"周三开会要带笔记本"应将周三写入 `deadline`。

1. **待办提醒时间 = 截止时间**：明确要"定时提醒的待办 / 到某时提醒的待办 / 待办提醒"且给出具体时刻时，用户预期提醒时间落为 `deadline.type=datetime`，并传 `remind_at_deadline=true`；只给日期或未给提醒/截止时间时不追问，不传 `remind_at_deadline=true`。
2. **普通截止时间**：只说截止/到期时间，或给出任务发生日期时，仅填写 `deadline`、不传 `remind_at_deadline`；此时按后台默认提前时间提醒。
3. **时间格式**：具体截止/提醒时刻 → `deadline.type=datetime`、`value="YYYY-MM-DD HH:mm:ss"`；只有截止日期 → `type=date`、`value="YYYY-MM-DD"`。
4. **未提截止/提醒或任务发生时间**：`deadline` 整体不传，`remind_at_deadline` 也不传，不追问。
5. **xx 时间截止，并提前 yy 提醒**：`deadline` 永远填用户说的 xx 截止时间，不要填提前后的提醒时间。当前入参不能直接设置"提前 yy"；创建/更新后用返回的 `extra_info` 判断系统提醒时间是否刚好满足 yy，不满足或无 `extra_info` 时回复：`目前不支持直接创建您需要的提醒时间，已为您设置截止时间为 XX，请到企业微信待办功能中手动修改提醒时间。`（XX 填本次 `deadline.value`）


### 示例

```json
{ "type": "date",     "value": "2026-05-08" }
{ "type": "datetime", "value": "2026-05-08 09:00:00" }
```

### 特别注意
- 禁止将 `todo_id`（待办 ID）展示给用户。
