---
name: wecomcli-todo
description: 企业微信待办事项管理技能，支持查询待办列表、获取待办详情、创建待办、更新待办、删除待办及变更用户处理进度状态。在用户说"看看我的待办列表"、"我有哪些待办"、"帮我创建一个待办"、"把这个任务分派给张三"、"标记待办完成"、"删掉那个待办"、"帮我建个提醒"、"更新一下待办内容"、"把提醒时间改到下周"、"接受这个待办"、"拒绝这个待办"、"这个待办的详情"、"待办分派给谁了"等需要对待办进行读写操作的场景时使用。
metadata:
  requires:
    bins: ["wecom-cli"]
  cliHelp: "wecom-cli todo --help"
---

# 企业微信待办事项管理技能

> `wecom-cli` 是企业微信提供的命令行程序，所有操作通过执行 `wecom-cli` 命令完成。

## 概述

wecomcli-todo 提供企业微信待办事项的完整管理能力，包含以下功能：

1. **查询待办列表** - 按创建时间和提醒时间过滤，支持分页，返回待办概要信息
2. **获取待办详情** - 根据待办 ID 列表批量获取完整信息（含内容和分派人）
3. **创建待办** - 创建新的待办事项，可指定内容、分派人和提醒时间
4. **更新待办** - 修改已有待办的内容、分派人、状态或提醒时间
5. **删除待办** - 删除指定的待办事项
6. **变更用户待办状态** - 更改当前用户在某个待办中的状态（拒绝/接受/已完成）

## 命令调用方式

执行指定命令：
```bash
wecom-cli todo <tool_name> '<json_params>'
```

## 行为策略

**查完列表必须查详情**: `get_todo_list` 只返回待办 ID 和状态等概要信息，不包含待办的实际内容和分派人。对用户来说，没有内容的待办列表毫无用处——他们想知道的是"要做什么"，而不是一串 ID。因此，每次调用 `get_todo_list` 拿到结果后，都要紧接着用返回的 todo_id 列表调用 `get_todo_detail` 获取完整详情（内容、分派人等），然后再向用户展示。这不是可选步骤，而是完成用户请求的必要环节。

**人员 ID 转姓名（关键步骤）**: `get_todo_detail` 返回结果中的 `follower_id` 和 `creator_id` 都是系统内部 ID，直接展示给用户毫无意义——用户不认识这些 ID，只认识姓名。因此在向用户展示待办详情之前，必须先调用 `wecomcli-contact` 技能获取通讯录，将所有 `follower_id` 和 `creator_id` 匹配为真实姓名。具体做法：
```bash
wecom-cli contact get_userlist '{}'
```
如果通讯录中找不到某个 ID，展示时标注"未知用户(ID：xxx)"即可。

**分页未拉完时必须提醒用户**: `get_todo_list` 接口是分页的，不要求一次性拉完所有数据。但如果响应中 `has_more` 为 `true`，说明后面还有待办没有返回——这时你在展示当前结果的同时，必须明确告诉用户"还有更多待办未显示，是否需要继续查看？"。用户可能不知道后面还有数据，如果你不说，他们会以为看到的就是全部，这会导致遗漏重要待办。

**重试策略**: 遭遇"返回 HTTP 错误"或"HTTP 请求失败"时，主动重试，最多重试三次。

---

## 命令详细说明

### 1. 查询待办列表 (get_todo_list)

查询当前用户的待办列表，支持按时间过滤和分页。返回的是待办概要信息（不含内容和分派人）。

#### 执行命令

```bash
wecom-cli todo get_todo_list '<json格式的入参>'
```

#### 入参说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `create_begin_time` | string | ❌ | 创建开始时间，格式：`YYYY-MM-DD HH:mm:ss` |
| `create_end_time` | string | ❌ | 创建结束时间，格式：`YYYY-MM-DD HH:mm:ss` |
| `remind_begin_time` | string | ❌ | 提醒开始时间，格式：`YYYY-MM-DD HH:mm:ss` |
| `remind_end_time` | string | ❌ | 提醒结束时间，格式：`YYYY-MM-DD HH:mm:ss` |
| `limit` | number | ❌ | 最大返回数量，默认 10，最大 20 |
| `cursor` | string | ❌ | 分页游标，首次请求不传，后续传入上次响应的 `next_cursor` |

#### 返回格式

```json
{
    "errcode": 0,
    "errmsg": "ok",
    "index_list": [
        {
            "todo_id": "TODO_ID",
            "todo_status": 1,
            "user_status": 1,
            "creator_id": "CREATOR_ID",
            "remind_time": "2025-06-01 09:00:00",
            "create_time": "2025-01-15 10:30:00",
            "update_time": "2025-01-16 14:20:00"
        }
    ],
    "next_cursor": "NEXT_CURSOR",
    "has_more": false
}
```

#### 返回字段说明

| 字段 | 类型 | 说明 |
|------|------|------|
| `index_list` | array | 待办列表 |
| `index_list[].todo_id` | string | 待办唯一 ID |
| `index_list[].todo_status` | number | 待办状态：`0`-已完成，`1`-进行中，`2`-已删除 |
| `index_list[].user_status` | number | 用户状态：`0`-拒绝，`1`-接受，`2`-已完成 |
| `index_list[].creator_id` | string | 创建人 ID |
| `index_list[].remind_time` | string | 提醒时间 |
| `index_list[].create_time` | string | 创建时间 |
| `index_list[].update_time` | string | 更新时间 |
| `next_cursor` | string | 下一页游标 |
| `has_more` | boolean | 是否还有更多记录 |

> 列表返回的是待办概要信息（不含内容和分派人）。拿到列表后，必须调用 `get_todo_detail` 获取完整详情再展示给用户。

---

### 2. 获取待办详情 (get_todo_detail)

根据待办 ID 列表批量查询完整详情，包含待办内容和分派人信息。

#### 执行命令

```bash
wecom-cli todo get_todo_detail '<json格式的入参>'
```

#### 入参说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `todo_id_list` | array | ✅ | 待办 ID 列表，最多 20 个 |

#### 调用示例

```bash
wecom-cli todo get_todo_detail '{"todo_id_list": ["TODO_ID_1", "TODO_ID_2"]}'
```

#### 返回格式

```json
{
    "errcode": 0,
    "errmsg": "ok",
    "data_list": [
        {
            "todo_id": "TODO_ID",
            "todo_status": 1,
            "content": "完成Q2规划文档",
            "follower_list": {
                "followers": [
                    {
                        "follower_id": "FOLLOWER_ID",
                        "follower_status": 1,
                        "update_time": "2025-01-16 14:20:00"
                    }
                ]
            },
            "creator_id": "CREATOR_ID",
            "user_status": 1,
            "remind_time": "2025-06-01 09:00:00",
            "create_time": "2025-01-15 10:30:00",
            "update_time": "2025-01-16 14:20:00"
        }
    ]
}
```

#### 返回字段说明

| 字段 | 类型 | 说明 |
|------|------|------|
| `data_list` | array | 待办详情列表，最多 20 条 |
| `data_list[].todo_id` | string | 待办 ID |
| `data_list[].todo_status` | number | 待办状态：`0`-已完成，`1`-进行中，`2`-已删除 |
| `data_list[].content` | string | 待办内容 |
| `data_list[].follower_list.followers` | array | 分派人列表 |
| `data_list[].follower_list.followers[].follower_id` | string | 分派人 ID（即 userid）— **展示前需通过 wecomcli-contact 转为姓名** |
| `data_list[].follower_list.followers[].follower_status` | number | 分派人状态：`0`-拒绝，`1`-接受，`2`-已完成 |
| `data_list[].follower_list.followers[].update_time` | string | 分派人状态更新时间 |
| `data_list[].creator_id` | string | 创建人 ID — **展示前需通过 wecomcli-contact 转为姓名** |
| `data_list[].user_status` | number | 当前用户状态 |
| `data_list[].remind_time` | string | 提醒时间 |
| `data_list[].create_time` | string | 创建时间 |
| `data_list[].update_time` | string | 更新时间 |

---

### 3. 创建待办 (create_todo)

创建一个新的待办事项，可指定内容、分派人和提醒时间。

#### 执行命令

```bash
wecom-cli todo create_todo '<json格式的入参>'
```

#### 入参说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `content` | string | ✅ | 待办内容 |
| `follower_list` | object | ❌ | 分派人列表，格式见注意事项第 6 条 |
| `remind_time` | string | ❌ | 提醒时间，格式：`YYYY-MM-DD HH:mm:ss` |

#### 调用示例

```bash
wecom-cli todo create_todo '{"content": "<待办的内容>", "remind_time": "2025-06-01 09:00:00"}'
```

#### 返回格式

```json
{
    "errcode": 0,
    "errmsg": "ok",
    "todo_id": "TODO_ID"
}
```

#### 返回字段说明

| 字段 | 类型 | 说明 |
|------|------|------|
| `todo_id` | string | 新创建的待办唯一 ID |

---

### 4. 更新待办 (update_todo)

修改已有待办事项的内容、分派人、状态或提醒时间。

#### 执行命令

```bash
wecom-cli todo update_todo '<json格式的入参>'
```

#### 入参说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `todo_id` | string | ✅ | 待办 ID |
| `content` | string | ❌ | 新的待办内容 |
| `follower_list` | object | ❌ | 新的分派人列表（全量替换，非追加），格式见注意事项第 6 条。若要新增分派人，需先查出现有分派人，合并后一起提交 |
| `todo_status` | number | ❌ | 新的待办状态：`0`-已完成，`1`-进行中，`2`-已删除。删除请使用 `delete_todo` |
| `remind_time` | string | ❌ | 新的提醒时间 |

#### 调用示例

```bash
wecom-cli todo update_todo '{"todo_id": "TODO_ID", "content": "<待办的内容>", "remind_time": "2025-07-01 09:00:00"}'
```

#### 返回格式

```json
{
    "errcode": 0,
    "errmsg": "ok"
}
```

---

### 5. 删除待办 (delete_todo)

删除指定的待办事项。

#### 执行命令

```bash
wecom-cli todo delete_todo '<json格式的入参>'
```

#### 入参说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `todo_id` | string | ✅ | 待办 ID |

#### 调用示例

```bash
wecom-cli todo delete_todo '{"todo_id": "TODO_ID"}'
```

#### 返回格式

```json
{
    "errcode": 0,
    "errmsg": "ok"
}
```

> 删除操作不可撤销，执行前应向用户确认。

---

### 6. 变更用户待办状态 (change_todo_user_status)

更改当前用户在某个待办中的状态（拒绝/接受/已完成）。

#### 执行命令

```bash
wecom-cli todo change_todo_user_status '<json格式的入参>'
```

#### 入参说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `todo_id` | string | ✅ | 待办 ID |
| `user_status` | number | ✅ | 用户状态：`0`-拒绝，`1`-接受，`2`-已完成 |

#### 调用示例

```bash
wecom-cli todo change_todo_user_status '{"todo_id": "TODO_ID", "user_status": 2}'
```

#### 返回格式

```json
{
    "errcode": 0,
    "errmsg": "ok"
}
```

#### 返回字段说明

| 字段 | 类型 | 说明 |
|------|------|------|
| `errcode` | number | 返回码，`0` 表示成功 |
| `errmsg` | string | 错误信息，成功时为 `"ok"` |

---

## 典型工作流

> 完整的工作流步骤、命令示例和展示格式详见 [examples/workflows.md](examples/workflows.md)

| 工作流 | 适用场景 | 关键要点 |
| ------ | -------- | -------- |
| 工作流 1：查看待办列表 | "看看我最近的待办" | 标准三步：get_todo_list → get_todo_detail → contact 转姓名；检查 has_more 分页 |
| 工作流 2：按时间范围查询 | "这个月创建的待办有哪些？" | 传入 create_begin_time / create_end_time 过滤，后续同工作流 1 |
| 工作流 3：创建待办并分派 | "帮我创建一个待办，让张三完成需求文档" | 先通过 wecomcli-contact 查 userid，再 create_todo 带 follower_list |
| 工作流 4：标记待办完成 | "把这个待办标记为完成" | 区分场景 A（改 todo_status=0）和场景 B（改 user_status=2） |
| 工作流 5：更新待办 | "把提醒时间改到下周五" | 先查列表+详情定位目标，再 update_todo |
| 工作流 6：删除待办 | "删掉那个待办" | 删除前必须向用户确认，不可撤销 |
| 工作流 7：分页获取 | 待办数量超过单页上限 | 通过 cursor 循环拉取，未拉完必须提醒用户 |

---

## 注意事项

1. **todo_id 来源规则**
   - `todo_id` 必须来自 `get_todo_list` 返回的结果，禁止自行推测或构造
   - 用户通常提供待办内容描述而非 ID，应先通过 `get_todo_list` 查列表再匹配
   - 若匹配到多个相似待办，展示候选列表让用户确认

2. **follower_id 来源规则**
   - `follower_id` 即 `userid`，必须通过 `wecomcli-contact` 技能的 `get_userlist` 接口获取
   - 禁止根据用户姓名自行猜测 userid
   - 若搜索结果有多个同名人员，展示候选列表让用户选择

3. **人员 ID 必须转姓名**
   - 返回结果中的 `follower_id` 和 `creator_id` 是系统内部标识，用户无法识别
   - 展示待办详情前，先通过 `wecom-cli contact get_userlist '{}'` 获取通讯录
   - 用通讯录的 `userid` 匹配 `follower_id` / `creator_id`，用 `name` 替换展示
   - 如果通讯录中找不到某个 ID，展示时标注"未知用户(ID：xxx)"

4. **时间格式**
   - 所有时间参数使用 `YYYY-MM-DD HH:mm:ss` 格式
   - 用户说"明天"、"下周一"等相对时间时，根据当前日期推算具体日期

5. **状态值含义**
   - 待办状态（`todo_status`）：`0`-已完成，`1`-进行中，`2`-已删除
   - 用户状态（`user_status`）：`0`-拒绝，`1`-接受，`2`-已完成
   - 分派人状态（`follower_status`）：`0`-拒绝，`1`-接受，`2`-已完成

6. **follower_list 的格式**（作为输入参数的时候）

    ```json
    "follower_list": {
        "followers": [
            {
                "follower_id": "FOLLOWER_ID",
                "follower_status": 1
            }
        ]
    }
    ```
    > `follower_id` 即 userid，需要通过 `wecomcli-contact` 查询获取，禁止自行猜测或构造。

7. **破坏性操作确认**
   - 删除待办（`delete_todo`）前必须向用户确认
   - 变更状态为"拒绝"（`user_status=0`）前建议向用户确认

8. **必须查详情**
   - `get_todo_list` 返回的是概要信息（不含内容和分派人），拿到列表后必须紧接着调用 `get_todo_detail` 获取完整内容再展示给用户，不要只展示列表概要

9. **分页未拉完必须提醒**
   - 如果返回的 `has_more` 为 `true`，在向用户展示结果时必须明确说明"还有更多待办未显示"并询问用户是否需要继续查看

10. **单次详情上限**
    - `todo_id_list` 最多传 20 个 ID，超过需要分批请求

11. **错误处理**
    - 若 `errcode` 不为 `0`，说明接口调用失败，告知用户 `errmsg` 中的错误信息

12. **通讯录查询**
    - 涉及分派人时，需先通过 `wecomcli-contact` 技能的 `get_userlist` 接口获取全量通讯录成员，再按姓名/别名本地筛选匹配出对应的 `userid`。该接口无入参，返回当前用户可见范围内的成员列表（含 `userid`, `name`, `alias`）
