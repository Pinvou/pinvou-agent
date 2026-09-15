# 批量获取待办详情 — `wecom-cli todo get`

批量查询 1-20 个待办的完整信息。

## 命令

```bash
wecom-cli todo get --json '<JSON 参数>'
```

## 参数

外层为对象，待查待办放在 `items` 数组中：

| 字段 | 类型 | 必填 | 语义 |
|---|---|---|---|
| `items` | array | 是 | 待查待办数组，每项结构见下，单次最多 20 个；超出需分批 |

`items[]` 元素结构：

| 字段 | 类型 | 必填 | 默认值 | 语义 |
|---|---|---|---|---|
| `todo_id` | string | 是 | — | 待办 ID（前缀 `td`） |

### 示例入参

```json
{
  "items": [
    { "todo_id": "td_xxx" },
    { "todo_id": "td_yyy" }
  ]
}
```

## 返回

外层为对象，结果在 `items` 数组中，与入参 `items` 一一对应：

| 字段 | 类型 | 语义 |
|---|---|---|
| `items` | array | 待办详情数组 |

`items[]` 元素结构：

| 字段 | 类型 | 语义 |
|---|---|---|
| `success` | boolean | 此条查询是否成功 |
| `todo_id` | string | 待办 ID（前缀 `td`） |
| `title` | string | 待办标题 |
| `description` | string | 详细描述 |
| `status` | string | 待办整体状态：`proceed` / `finished` |
| `user_status` | string | 当前用户在该待办的状态：`accept` / `reject` / `finished` / `removed` / `notshow` |
| `creator` | object | 创建人，含 `userid`（前缀 `wo`） / `user_name`（格式 `英文名(中文名)`） |
| `followers` | array | 分派人列表，每项含 `userid`（前缀 `wo`） / `user_name` / `user_status` / `update_time` |
| `deadline` | object | 截止时间；结构见 SKILL.md `deadline` 对象规范。无截止时间时不返回或为 `null` |
| `extra_info` | string | 提醒时刻只读信息，可能不提醒 |
| `source` | string | 待办来源：`single_chat`（单聊）/ `group_chat`（群聊）/ `doc`（文档）/ `ai_summary`（智能总结）/ `meeting_summary`（会议纪要）/ `face_chat`（「面聊」功能）/ `fused_doc`（融合文档）/ `smart_sheet`（智能表格）/ `smart_doc`（智能文档）/ `JSAPI`（JSAPI） |
| `create_time` | string | 创建时间，格式 `YYYY-MM-DD HH:mm:ss` |
| `update_time` | string | 更新时间，格式 `YYYY-MM-DD HH:mm:ss` |
| `errmsg` | string | 失败原因，仅 `success=false` 时存在 |

## 使用规则

- **单次上限 20**：超出需分批请求
- **已有 `todo_id` 时确认状态用本接口**：需要核对某条待办的最新 `status` / `user_status` 时，使用 `wecom-cli todo get`。
