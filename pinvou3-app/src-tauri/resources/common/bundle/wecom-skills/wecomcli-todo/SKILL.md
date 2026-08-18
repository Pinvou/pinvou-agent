---
name: wecomcli-todo
description: 何时用:仅当用户明确指向企业微信待办(建企微待办、查企微待办)时使用;泛指记个待办/提醒默认走本地工具,不要误用。企微待办:创建、更新、更改参与人状态、获取待办列表、批量获取待办详情、删除。用户说"帮我创建一个待办"、"把这个任务分派给张三"、"标记待办完成"、"接受/拒绝这个待办"、"删掉那个待办"、"看看我有哪些待办"等需要对待办进行读写操作的场景时使用。
metadata:
  requires:
    bins: ["wecom-cli"]
  cliHelp: "wecom-cli todo --help"
---

# 企业微信待办事项管理技能

> `wecom-cli` 是企业微信提供的命令行程序，所有操作通过执行 `wecom-cli` 命令完成。

## 概述

wecomcli-todo 提供企业微信待办事项的管理能力，包含以下功能：

1. **搜索 userid** - 根据姓名或别名搜索用户的 userid，用于在创建/更新待办、更改用户状态、查询待办列表时把姓名解析为 userid
2. **创建待办** - 创建新的待办事项，可指定内容、参与人、截止时间和提醒方式
3. **更新待办** - 修改已有待办的内容、参与人、待办状态、截止时间和提醒方式
4. **更改用户状态** - 更改某位参与人在某个待办中的状态（拒绝/接受/已完成）
5. **获取待办列表** - 获取指定用户的待办列表，支持按创建时间和提醒时间过滤
6. **获取待办详情** - 根据待办 ID 列表批量获取完整信息
7. **删除待办** - 删除指定的待办事项

## 命令调用方式

执行指定命令：
```bash
wecom-cli todo <tool_name> '<json_params>'
```

---

## 命令详细说明

### 1. 搜索 userid (search_todo_userid)

基础接口，根据姓名或别名搜索用户的 userid，用于在创建待办、更新待办时把用户姓名解析为 userid 进行传参。

#### 执行命令

```bash
wecom-cli todo search_todo_userid '<json格式的入参>'
```

#### 入参说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `keyword` | string | 是 | 姓名或别名 |

#### 调用示例

```bash
wecom-cli todo search_todo_userid '{"keyword": "张三"}'
```

#### 返回字段说明

- 返回与关键字匹配的用户的 `userid`（可能为多个候选）。
- 若有多个同名/相似用户，展示候选列表让用户确认后再使用；展示时只用姓名/别名/部门等可读信息区分，**禁止向用户暴露 `userid`**。
- 拿到的 `userid` 用于 `create_todo` / `update_todo` 的 `follower_list`，仅供内部传参，不在对话中展示。
- 调用前可先检查对话上下文是否已有该用户的 `userid`，若有则直接复用、无需再次查询；若本接口返回错误（查询失败、无结果等），则改用通讯录 `wecomcli-contact` 技能搜索（详见注意事项第 2 条）。

---

### 2. 创建待办 (create_todo)

创建一个新的待办事项，可指定内容、参与人、截止时间和提醒方式。

#### 执行命令

```bash
wecom-cli todo create_todo '<json格式的入参>'
```

#### 入参说明

| 参数 | 类型 | 必填 | 说明                                                              |
|------|------|----|-----------------------------------------------------------------|
| `content` | string | 是  | 待办内容                                                            |
| `follower_list` | object | 是  | 参与人信息，对象中包含 `followers` 数组，格式见注意事项第 3 条 |
| `end_time` | string | 条件必填  | 截止时间（到期时间），格式：`YYYY-MM-DD HH:mm:ss`；提醒方式基于此时间计算偏移。当 `remind_type_list` 含非 `0`（即设置了实际提醒）时**必填**          |
| `remind_type_list` | uint32[] | 否  | 提醒方式，相对 `end_time` 计算，可传入多个。取值：`0`-不提醒，`1`-到期时，`3`-提前 15 分钟，`5`-提前 1 小时，`6`-提前 2 小时，`7`-提前 1 天，`8`-提前 2 天，`9`-提前 1 周（详见注意事项第 4 条）                       |

#### 调用示例

```bash
wecom-cli todo create_todo '{"content": "<待办的内容>", "follower_list": {"followers": [{"follower_id": "FOLLOWER_ID", "follower_status": 1}]}, "end_time": "2025-06-01 09:00:00", "remind_type_list": [3, 7]}'
```

#### 返回字段说明

| 字段 | 类型 | 说明 |
|------|------|------|
| `todo_id` | string | 新创建的待办唯一 ID |

---

### 3. 更新待办 (update_todo)

修改已有待办事项的内容、参与人、待办状态、截止时间或提醒方式。

> 说明：本接口可更改待办状态（`todo_status`），但**不能更改参与人状态（`follower_status`）**。如需更改当前用户在某个待办中的状态，请使用 `change_todo_user_status`。

#### 执行命令

```bash
wecom-cli todo update_todo '<json格式的入参>'
```

#### 入参说明

| 参数 | 类型 | 必填 | 说明                                                       |
|------|------|------|----------------------------------------------------------|
| `todo_id` | string | 是 | 待办 ID                                                    |
| `content` | string | 否 | 新的待办内容                                                   |
| `follower_list` | object | 否 | 新的参与人信息（全量替换，非追加），对象中包含 `followers` 数组，格式见注意事项第 3 条。若要新增参与人，需先查出现有参与人，合并后一起提交。**本接口不会更改参与人状态（`follower_status`），如需更改用户状态请使用 `change_todo_user_status`** |
| `todo_status` | uint32 | 否 | 新的待办状态：`0`-已完成，`1`-进行中       |
| `end_time` | string | 否 | 新的截止时间（到期时间），格式：`YYYY-MM-DD HH:mm:ss`；提醒方式基于此时间计算偏移   |
| `remind_type_list` | uint32[] | 否 | 新的提醒方式，相对 `end_time` 计算，可传入多个。取值：`0`-不提醒，`1`-到期时，`3`-提前 15 分钟，`5`-提前 1 小时，`6`-提前 2 小时，`7`-提前 1 天，`8`-提前 2 天，`9`-提前 1 周（详见注意事项第 4 条）                                |

#### 调用示例

```bash
wecom-cli todo update_todo '{"todo_id": "TODO_ID", "content": "<待办的内容>", "end_time": "2025-07-01 09:00:00", "remind_type_list": [1]}'
```

---

### 4. 更改用户在某个待办的状态 (change_todo_user_status)

更改某位参与人在某个待办中的状态（拒绝/接受/已完成）。

#### 执行命令

```bash
wecom-cli todo change_todo_user_status '<json格式的入参>'
```

#### 入参说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `todo_id` | string | 是 | 待办 ID |
| `follower_id` | string | 是 | 参与人 `userid`，指定要修改哪一位参与人的状态。来源遵循注意事项第 2 条的 userid 获取规则（上下文已有 → `search_todo_userid` → `wecomcli-contact` 技能兜底），禁止自行猜测或构造 |
| `user_status` | uint32 | 是 | 用户状态：`0`-拒绝，`1`-接受，`2`-已完成 |

#### 调用示例

```bash
wecom-cli todo change_todo_user_status '{"todo_id": "TODO_ID", "follower_id": "FOLLOWER_ID", "user_status": 2}'
```

---

### 5. 获取待办列表 (get_todo_list)

获取指定用户（由 `follower_id` 指定，可为任意用户）通过机器人的待办列表，支持当天前后一个月的待办，可按创建时间、提醒时间、截止时间和待办状态过滤。

#### 执行命令

```bash
wecom-cli todo get_todo_list '<json格式的入参>'
```

#### 入参说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `follower_id` | string | 是 | 参与人 `userid`，指定要查询哪一位用户的待办列表（可为任意用户），来源遵循注意事项第 2 条的 userid 获取规则，禁止自行猜测或构造 |
| `create_begin_time` | string | 否 | 创建开始时间 |
| `create_end_time` | string | 否 | 创建结束时间 |
| `remind_begin_time` | string | 否 | 提醒开始时间 |
| `remind_end_time` | string | 否 | 提醒结束时间 |
| `deadline_begin_time` | string | 否 | 截止开始时间 |
| `deadline_end_time` | string | 否 | 截止结束时间 |
| `todo_status` | uint32 | 否 | 待办状态：`0`-已完成，`1`-进行中 |
| `limit` | uint32 | 否 | 最大返回数量，默认为 10，最大为 20 |
| `cursor` | string | 否 | 游标，用于分页 |

> **查询时间范围限制**：
> - 用于筛选的时间（创建时间、提醒时间、截止时间）必须落在当天前后 30 天以内。
> - 若仅按 `todo_status` 筛选而未传任何时间范围，则默认按「创建时间在当天前后 30 天内」查询。

#### 调用示例

```bash
wecom-cli todo get_todo_list '{"follower_id": "FOLLOWER_ID", "create_begin_time": "2025-06-01 00:00:00", "create_end_time": "2025-06-30 23:59:59", "todo_status": 1, "limit": 20}'
```

#### 返回字段说明

- 返回指定用户（`follower_id`）的待办列表，以及用于分页的游标。
- 拿到的 `todo_id` 可用于 `get_todo_detail` / `update_todo` / `delete_todo`，仅供内部传参，**禁止向用户暴露**。

---

### 6. 获取待办详情 (get_todo_detail)

根据待办 ID 列表批量查询完整详情，包含待办内容和参与人信息。

#### 执行命令

```bash
wecom-cli todo get_todo_detail '<json格式的入参>'
```

#### 入参说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `todo_id_list` | string[] | 是 | 待办 ID 列表，至少 1 个，最多 20 个 |

#### 调用示例

```bash
wecom-cli todo get_todo_detail '{"todo_id_list": ["TODO_ID_1", "TODO_ID_2"]}'
```

#### 返回字段说明

| 字段                                                      | 类型     | 说明                                |
|---------------------------------------------------------|--------|-----------------------------------|
| `data_list`                                             | array  | 待办详情列表，最多 20 条                    |
| `data_list[].todo_id`                                   | string | 待办 ID                             |
| `data_list[].todo_status`                               | number | 待办状态：`0`-已完成，`1`-进行中，`2`-已删除      |
| `data_list[].content`                                   | string | 待办内容                              |
| `data_list[].follower_list`                             | object | 参与人列表                             |
| `data_list[].follower_list.followers[].follower_id`     | string | 参与人 ID（即 userid）                  |
| `data_list[].follower_list.followers[].name`            | string | 参与人姓名                             |
| `data_list[].follower_list.followers[].follower_status` | number | 参与人状态：`0`-拒绝，`1`-接受，`2`-已完成       |
| `data_list[].follower_list.followers[].update_time`              | string | 参与人状态更新时间                         |
| `data_list[].creator_id`                                | string | 创建人 ID（即 userid）                  |
| `data_list[].user_status`                               | number | 当前用户在该待办中的状态：`0`-拒绝，`1`-接受，`2`-已完成 |
| `data_list[].end_time`                                  | string | 截止时间（到期时间）                        |
| `data_list[].remind_type_list`                          | array  | 提醒方式列表，取值：`0`-不提醒，`1`-到期时，`3`-提前 15 分钟，`5`-提前 1 小时，`6`-提前 2 小时，`7`-提前 1 天，`8`-提前 2 天，`9`-提前 1 周 |
| `data_list[].create_time`                               | string | 创建时间                              |
| `data_list[].update_time`                               | string | 更新时间                              |

---

### 7. 删除待办 (delete_todo)

删除指定的待办事项。

#### 执行命令

```bash
wecom-cli todo delete_todo '<json格式的入参>'
```

#### 入参说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `todo_id` | string | 是 | 待办 ID |

#### 调用示例

```bash
wecom-cli todo delete_todo '{"todo_id": "TODO_ID"}'
```

---

## 注意事项

1. **todo_id 来源规则**
   - `todo_id` 可来自 `create_todo` 返回的结果，或 `get_todo_list` 查询到的列表，请务必保存好，禁止自行推测或构造

2. **userid 来源与隐私规则**
   - 分派待办时，`follower_list` 中的 `follower_id` 即 `userid`，按以下优先级获取，禁止根据用户姓名自行猜测或构造 userid：
     1. **对话上下文已有**：若对话上下文中已经提到对应用户的 `userid`，直接使用，无需再次查询
     2. **search_todo_userid 查询**：若上下文中没有，则可先询问用户在企业内的用户名称（通常是姓名），再通过 `search_todo_userid` 接口按姓名/别名查询获取
     3. **通讯录技能兜底**：若 `search_todo_userid` 返回错误（如当前企业不适用等），则改用通讯录 `wecomcli-contact` 技能搜索获取 `userid`
   - `userid` 属于敏感标识，仅用于接口传参，**禁止在回复或候选列表中向用户展示**；需要让用户区分人员时，只展示姓名/别名/部门等可读信息

3. **follower_list 格式说明**
   - 作为入参时，`follower_list` 为对象，内含 `followers` 数组，格式如下：
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
   - `follower_id` 即用户的 `userid`，其来源遵循上文「userid 来源与隐私规则」的优先级（上下文已有 → `search_todo_userid` → `wecomcli-contact` 技能兜底），禁止自行猜测或构造
   - `follower_status` 为参与人状态：`0`-拒绝，`1`-接受，`2`-已完成；该字段仅在 `create_todo` 中生效
   - 在 `update_todo` 中为全量替换（非追加）：若要新增参与人，需先用 `get_todo_detail` 查出现有参与人，合并后一并提交；`update_todo` **不会更改参与人状态（`follower_status`）**，如需更改当前用户在某个待办中的状态，请使用 `change_todo_user_status`

4. **remind_type_list 提醒方式取值**

`remind_type_list` 是一个 uint32 数组，可同时传入多个提醒：

| 值 | 含义 | 值 | 含义 |
|----|------|----|------|
| `0` | 不提醒 | `6` | 提前 2 小时 |
| `1` | 到期时 | `7` | 提前 1 天 |
| `3` | 提前 15 分钟 | `8` | 提前 2 天 |
| `5` | 提前 1 小时 | `9` | 提前 1 周 |


## pinvou3 增补行为要求

- `get_todo_list` 只返回概要(无内容),拿到列表后必须紧接着用 `get_todo_detail` 查完整内容再展示,不要只展示 ID 列表
- 响应 `has_more` 为 `true` 时,展示当前结果的同时必须告知"还有更多待办未显示"并询问是否继续
- 删除待办、变更状态为"拒绝"前必须向用户确认
