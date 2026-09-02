# 修改待办 — `wecom-cli todo update`

批量更新待办的标题、描述、分派人名单或截止时间。

## 命令

```bash
wecom-cli todo update --json '<JSON 参数>'
```

## 参数

外层为对象，待更新的待办放在 `items` 数组中（支持批量）：

| 字段 | 类型 | 必填 | 语义 |
|---|---|---|---|
| `items` | array | 是 | 更新条目数组，每项结构见下，单次最多 20 条；超出需分批 |

`items[]` 元素结构（仅传需修改的字段，未传字段保持不变）：

| 字段 | 类型 | 必填 | 默认值 | 语义 |
|---|---|---|---|---|
| `todo_id` | string | 是 | — | 待办 ID（前缀 `td`） |
| `title` | string | 否 | — | 新的短标题 |
| `description` | string | 否 | — | 新的详细描述 |
| `followers` | array | 否 | — | **全量替换**后的分派人列表，最多 50 人；用户给姓名时使用 `wecomcli-contact` 技能获取 `userid`（前缀 `wo`） |
| `deadline` | object | 否 | — | 新的截止时间；结构见 SKILL.md `deadline` 对象规范。**传空对象 `{}` 表示清空已设置的截止时间**；不传字段则保持原值 |
| `remind_at_deadline` | boolean | 否 | `false` | 提醒时机，须与 `deadline` 同传：`true`=截止时刻提醒（仅 `datetime`）；`false`/不传=按后台默认提前时间提醒（**非关闭提醒**）。脱离 `deadline` 单独传无效 |

`followers` 对象结构：

| 子字段 | 类型 | 必填 | 语义 |
|---|---|---|---|
| `userid` | string | 是 | 分派人 `userid`，前缀 `wo` |

> 入参的 `followers` 子对象**只接收 `userid`**。`wecom-cli todo list` / `wecom-cli todo get` 返回的 `followers` 还含 `user_name` / `user_status` / `update_time`，转入更新入参时全部剥掉，只保留 `userid`。

> `followers` 是全量替换，不是增量添加。只新增或移除部分参与人时，先从 `todo list` / `todo get` 取得现有名单，在本地合并或删减，再把所有应保留的参与人重新传入。

> 用户说"把我也加进去"、"分派给我和某某"时，`followers` 里同样要带上当前用户自己的 `userid`。

### 修改截止时间与提醒

- `remind_at_deadline` 必须与 `deadline` 一起传，语义与 create 完全一致；**只传 `remind_at_deadline`、不带 `deadline` 不会生效**，不要这么做。
- 用户只改**截止时间/到期时间**时，填写新的 `deadline`，不要传 `remind_at_deadline`（即按后台默认提前时间提醒）。
- 用户要求把待办改成"某时间提醒 / 定时提醒 / 截止时提醒"且给出具体时刻时，将该时刻作为新的 `deadline.type=datetime`，并传 `remind_at_deadline=true`；只给日期时不传 `remind_at_deadline=true`。
- **`remind_at_deadline=false` 或不传 ≠ 关闭提醒**，而是按后台默认提前时间提醒。**update 入参没有关闭提醒的开关**（`remind_at_deadline` 只切换提醒时机；是否真正提醒由后台判断）：用户要"取消提醒 / 关掉提醒 / 别提醒了"时，直接告知目前不支持关闭待办提醒；若用户坚持完全不提醒，唯一办法是连同截止时间一起清空（`deadline: {}`，会一并删掉截止时间），须先向用户确认再操作。
- 用户要求"某时间截止，并提前 X 提醒"时，`deadline` 永远填用户说的**截止时间**，不要填提前后的提醒时刻。当前入参不能直接设置"提前 X"；更新后用返回的 `extra_info` 判断系统提醒时间是否刚好满足 X。

### 示例入参

更新标题、截止时间并设置截止时提醒：

```json
{
  "items": [
    {
      "todo_id": "td_xxx",
      "title": "调整后的周会材料",
      "deadline": {
        "type": "datetime",
        "value": "2026-05-13 09:00:00"
      },
      "remind_at_deadline": true
    }
  ]
}
```

清空截止时间、清空分派人：

```json
{
  "items": [
    {
      "todo_id": "td_xxx",
      "deadline": {},
      "followers": []
    }
  ]
}
```

## 返回

外层为对象，结果在 `items` 数组中，与入参 `items` 一一对应：

| 字段 | 类型 | 语义 |
|---|---|---|
| `items` | array | 更新结果数组 |

`items[]` 元素结构：

| 字段 | 类型 | 语义 |
|---|---|---|
| `success` | boolean | 是否更新成功 |
| `todo_id` | string | 待办 ID |
| `extra_info` | string | 提醒时刻只读信息，可能不提醒 |
| `errmsg` | string | 失败原因，仅 `success=false` 时存在 |

## 使用规则

- **如果上下文没有对应待办 ID**：**必须**先阅读 `references/todo-list.md`，在待办列表中找到需要修改的待办。
- **避免冗余更新**：如果用户只是把待办**已经记录过的内容又复述了一遍**（例如标题已经等于用户这次说的内容），这是确认而不是修改，**不要发起 `update`**，直接回复"这条已经记好了"即可。尤其**不要把 `description` 更新成与 `title` 相同的内容**——description 只用于承载标题之外的补充信息，没有新增信息就不要写。
- **补全信息先查上下文**：用户要求"写清楚点"、补充参与人/时间/链接/单号时，先从当前会话、待办详情和可用的聊天/记忆检索结果中找；能确定就更新，找不到或有歧义时再一次性向用户确认，避免直接让用户重发。
- **仅改部分字段**：未传的字段保持原值；若要清空 `followers`，传空数组 `[]`；若要清空 `deadline`，传空对象 `{}`。**没有关闭提醒的入参**：`remind_at_deadline=false`/不传只是改成默认提前提醒，不会关闭提醒（详见「修改截止时间与提醒」）
- **本次更新传了 `remind_at_deadline=true` 或用户提到提醒诉求** 且更新成功时，**必须**在最终回复中附上提醒说明（注意 `remind_at_deadline` 只对 datetime 生效）：
  - 用户要求"提前 X 提醒"时，核对 `extra_info` 是否为用户要求的提前提醒时间（即截止时间提前 X 后的时刻）；匹配则说明已满足，不匹配或无 `extra_info` 则按固定话术说明：`目前不支持直接创建您需要的提醒时间，已为您设置截止时间为 XX，请到企业微信待办功能中手动修改提醒时间。`（XX 填本次 `deadline.value`）。
  - 用户要求"截止时/到点提醒"时，只有 `deadline.type=datetime` 才应传 `remind_at_deadline=true`；若 `extra_info` 不等于 `deadline.value` 或缺失，仍需引导到企业微信待办功能中修改提醒时间。
  - 返回里有 `extra_info`（且非"提前 X 提醒"场景）→ 引用 `extra_info` 里的时刻告诉用户届时会自动提醒。
  - 返回里没有 `extra_info`（且非"提前 X 提醒"场景）→ 说明返回未确认提醒时间，引导用户到企业微信待办应用中检查/修改提醒时间。
  - 不要另建定时任务来模拟待办提醒，避免重复提醒。仅改 `deadline` 但未要求提醒时，无需额外提醒说明。
