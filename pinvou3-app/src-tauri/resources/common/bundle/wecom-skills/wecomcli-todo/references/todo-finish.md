# 完成待办 — `wecom-cli todo finish`

将**当前用户**在该待办中的部分标记为"已完成"。如果当前用户同时是创建人，后台会返回 `ask_finish_all` 提示，可选择把所有参与人一并标记完成。

## 命令

```bash
wecom-cli todo finish --json '<JSON 参数>'
```

## 参数

外层为对象，待办放在 `items` 数组中：

| 字段 | 类型 | 必填 | 语义 |
|---|---|---|---|
| `items` | array | 是 | 待办数组，每项结构见下，单次最多 20 条；超出需分批 |

`items[]` 元素结构：

| 字段 | 类型 | 必填 | 默认值 | 语义 |
|---|---|---|---|---|
| `todo_id` | string | 是 | — | 待办 ID |
| `finished_all` | boolean | 否 | `false` | 创建人可设为 `true` 全部完成该待办。默认 `false` 仅完成自己的部分 |

示例入参：

```json
{
  "items": [
    {
      "todo_id": "td_xxx",
      "finished_all": false
    }
  ]
}
```

## 返回

外层为对象，结果在 `items` 数组中，与入参 `items` 一一对应：

| 字段 | 类型 | 语义 |
|---|---|---|
| `items` | array | 完成结果数组 |

`items[]` 元素结构：

| 字段 | 类型 | 语义 |
|---|---|---|
| `success` | boolean | 是否完成成功 |
| `todo_id` | string | 待办 ID |
| `title` | string | 待办标题 |
| `ask_finish_all` | string | 当后台检测到用户既是创建人又是参与人时返回，提示模型询问用户是否标记为"全部完成" |
| `errmsg` | string | 失败原因，仅 `success=false` 时存在 |

## 使用规则

- **如果上下文没有对应待办 ID**：**必须**先阅读 `references/todo-list.md`，学习如何获取待办列表，在待办列表中找到需要完成的待办；此时应同时查 `finished` 和 `proceed`。如果已有 `todo_id` 但需要确认最新状态，使用 `wecom-cli todo get`。
- **完成操作要幂等**：定位待办时如果发现该待办整体 `status=finished` 或当前用户 `user_status=finished`，说明已完成，直接告知用户"这条待办已完成"，不要再调用 `finish`。只有用户本次或本会话前文明确要求"完成后删除/清掉/自动删除"时，才继续按删除流程处理。
- 调用前先按用户语义决定 `finished_all`：
  - 用户明确表达"仅我完成自己的部分"（"我这边搞完了"、"我自己的部分先完成"、"先把我那块标了"）→ **显式**传 `finished_all: false`。显式 false 才能让后端跳过 `ask_finish_all` 兜底，避免再次询问完成范围。
  - 用户明确表达"全部完成"（"完成了"、"这条结掉"、"都搞完了"），或本会话此前对同一个 `todo_id` 已经调过一次 `finished_all=false`、用户现在又一次说要完成它 → 传 `finished_all: true`。
  - 表达不明确（只说"完成 XX 待办"、"把那条待办完成了"，没有"仅我"或"全部"的语气）→ 不传 `finished_all`，让后端按下方 `ask_finish_all` 流程返回是否需要确认。
- **`ask_finish_all` 处理流程**：如果返回中出现 `ask_finish_all` 字段，说明当前用户是创建人，第一次调用已把当前用户自己的部分标记完成；**必须**用简洁自然语言向用户确认是否把其他参与人也一并标记完成，并在文字中列出「仅我完成」「已完全完成」两个选项。提问中应包含待办标题和 `followers` 中的参与人中文名（用顿号"、"拼接），例如：
  ```
  待办「<待办标题>」中您的部分已完成。参与人：<参与人中文名>。请选择完成范围：仅我完成，还是已完全完成？
  ```
  - 用户选 **「仅我完成」** → 不再调用接口（第一次已经完成了自己的部分），告知用户已标记完成。
  - 用户选 **「已完全完成」** → 用同一个 `todo_id` 再次调用 `wecom-cli todo finish`，并传 `finished_all: true`。
- 结果 `items` 与入参 `items` 一一对应
- 禁止将 `todo_id`（待办 ID）展示给用户。
