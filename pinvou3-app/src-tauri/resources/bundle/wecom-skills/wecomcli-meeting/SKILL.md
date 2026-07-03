---
name: wecomcli-meeting
description: 企业微信会议技能，支持创建预约会议、查询会议列表、获取会议详情、取消会议、更新会议成员。当用户需要"创建会议"、"预约会议"、"约会议"、"安排会议"、"查看会议"、"查询会议列表"、"会议详情"、"什么时候开会"、"有哪些会议"、"查找会议"、"取消会议"、"删除会议"、"修改会议成员"、"添加会议参与人"、"移除会议成员"时触发。
metadata:
  requires:
    bins: ["wecom-cli"]
  cliHelp: "wecom-cli meeting --help"
---
# 企业微信会议技能

> `wecom-cli` 是企业微信提供的命令行程序，所有操作通过执行 `wecom-cli` 命令完成。

## 概述

wecomcli-meeting 提供企业微信会议的完整管理能力，包含以下功能：

1. **创建预约会议** - 创建会议，支持设置会议参数，邀请参与人等
2. **查询会议列表** - 按用户和时间范围查询会议 ID 列表 (限制: 当日及前后 30 天，上限 100 个)
3. **获取会议详情** - 通过会议 ID 查询完整会议信息
4. **取消会议** - 取消指定的预约会议
5. **更新会议受邀成员** - 修改会议的参与人列表

## 命令调用方式

执行指定命令：
```bash
wecom-cli meeting <tool_name> '<json_params>'
```

---

## 命令详细说明

### 1. 创建预约会议 (create_meeting)

创建一个预约会议，支持设置会议参数配置等。

#### 执行命令

```bash
wecom-cli meeting create_meeting '{"title": "<会议标题>", "meeting_start_datetime": "<会议开始时间>", "meeting_duration": <会议持续时长(秒)>}'
```

#### 入参说明

| 参数                       | 类型    | 必填 | 说明                                              |
| -------------------------- | ------- | ---- | ------------------------------------------------- |
| `title`                  | string  | 是   | 会议标题                                          |
| `meeting_start_datetime` | string  | 是   | 会议开始时间，格式:`YYYY-MM-DD HH:mm`           |
| `meeting_duration`       | integer | 是   | 会议持续时长 (秒)，例如 3600 = 1 小时             |
| `description`            | string  | 否   | 会议描述                                          |
| `location`               | string  | 否   | 会议地点                                          |
| `invitees`               | object  | 否   | 被邀请人，格式:`{"userid": ["lisi", "wangwu"]}` |
| `settings`               | object  | 否   | 会议设置 (详见下方)                               |

> 被邀请人 userid 通过 `wecomcli-contact` 技能获取

**settings 字段:**

| 参数                        | 类型    | 说明                                          |
| --------------------------- | ------- | --------------------------------------------- |
| `password`                | string  | 会议密码                                      |
| `enable_waiting_room`     | boolean | 是否启用等候室                                |
| `allow_enter_before_host` | boolean | 是否允许成员在主持人进入前加入                |
| `enable_enter_mute`       | integer | 入会时静音设置 (枚举: 0: 关闭，1: 开启)       |
| `allow_external_user`     | boolean | 是否允许外部用户入会                          |
| `enable_screen_watermark` | boolean | 是否开启屏幕水印                              |
| `remind_scope`            | integer | 提醒范围 (1: 不提醒，2: 仅提醒主持人，3: 提醒所有成员，4: 指定部分人响铃，默认仅提醒主持人) |
| `ring_users`              | object  | 响铃用户，格式:`{"userid": ["lisi"]}`   |

> 响铃用户 userid 通过 `wecomcli-contact` 技能获取

#### 返回参数

```json
{
  "errcode": 0,
  "errmsg": "ok",
  "meetingid": "会议ID字符串",
  "meeting_code": "会议号码字符串",
  "meeting_link": "会议链接URL",
  "excess_users": ["无效会议账号的userid"]
}
```

| 字段             | 类型   | 说明                                                                                                               |
| ---------------- | ------ | ------------------------------------------------------------------------------------------------------------------ |
| `meetingid`    | string | 会议 ID                                                                                                            |
| `meeting_code` | string | 会议号码，向用户展示时需在回复**开头**单独一行纯文字展示，格式 `#会议号: xxx-xxx-xxx` (每3位用 `-` 分隔) |
| `meeting_link` | string | 会议链接                                                                                                           |
| `excess_users` | array  | 参会人中包含无效会议账号的 userid，仅在购买会议专业版企业由于部分参会人无有效会议账号时返回                        |

---

### 2. 查询会议列表 (list_user_meetings)

查询指定用户在时间范围内的会议 ID 列表。

#### 执行命令

```bash
wecom-cli meeting list_user_meetings '{"begin_datetime": "2026-03-01 00:00", "end_datetime": "2026-03-31 23:59", "limit": 100}'
```

#### 入参说明

| 参数               | 类型    | 必填 | 说明                                    |
| ------------------ | ------- | ---- | --------------------------------------- |
| `begin_datetime` | string  | 否   | 查询起始时间，格式:`YYYY-MM-DD HH:mm` |
| `end_datetime`   | string  | 否   | 查询结束时间，格式:`YYYY-MM-DD HH:mm` |
| `cursor`         | string  | 否   | 分页游标，用于获取下一页数据            |
| `limit`          | integer | 否   | 每页返回条数，最大 100                  |

> **限制**: 时间范围仅支持当日及前后 30 天。

#### 返回参数

```json
{
  "errcode": 0,
  "errmsg": "ok",
  "next_cursor": "分页游标字符串，为空表示无更多",
  "meetingid_list": ["会议ID_1", "会议ID_2"]
}
```

| 字段               | 类型   | 说明                           |
| ------------------ | ------ | ------------------------------ |
| `meetingid_list` | array  | 会议 ID 列表                   |
| `next_cursor`    | string | 下一页游标，为空表示无更多数据 |

---

### 3. 获取会议详情 (get_meeting_info)

通过会议 ID 查询会议的完整详情。

#### 执行命令

```bash
wecom-cli meeting get_meeting_info '{"meetingid": "<会议id>"}'
```

#### 入参说明

| 参数              | 类型   | 必填 | 说明            |
| ----------------- | ------ | ---- | --------------- |
| `meetingid`     | string | 是   | 会议 ID，通过 `list_user_meetings` 获取 |
| `meeting_code`  | string | 否   | 会议号码        |
| `sub_meetingid` | string | 否   | 子会议 ID       |

#### 返回参数

> 完整的返回参数结构和字段说明详见 [references/response-get-meeting-info.md](references/response-get-meeting-info.md)

**核心字段速览:**

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `title` | string | 会议标题 |
| `meeting_start_datetime` | string | 会议开始时间 |
| `meeting_duration` | integer | 会议时长 (秒) |
| `status` | integer | 会议状态 (1: 待开始，2: 会议中，3: 已结束，4: 已取消，5: 已过期) |
| `meeting_type` | integer | 会议类型 (0: 一次性，1: 周期性，2: 微信专属，3: Rooms 投屏，5: 个人会议号，6: 网络研讨会) |
| `meeting_code` | string | 会议号码 |
| `meeting_link` | string | 会议链接 |
| `description` | string | 会议描述 |
| `location` | string | 会议地点 |
| `attendees.member[].status` | integer | 与会状态 (1: 已参与，2: 未参与) |

---

### 4. 取消会议 (cancel_meeting)

取消指定的预约会议。

#### 执行命令

```bash
wecom-cli meeting cancel_meeting '{"meetingid": "<会议id>"}'
```

#### 入参说明

| 参数              | 类型   | 必填 | 说明                               |
| ----------------- | ------ | ---- | ---------------------------------- |
| `meetingid`     | string | 是   | 会议 ID，通过 `list_user_meetings` + `get_meeting_info` 获取 |

#### 返回参数

```json
{
  "errcode": 0,
  "errmsg": "ok"
}
```

---

### 5. 更新会议受邀成员 (set_invite_meeting_members)

更新会议的受邀成员列表（全量覆盖）。

#### 执行命令

```bash
wecom-cli meeting set_invite_meeting_members '{"meetingid": "<会议id>", "invitees": [{"userid": "lisi"}, {"userid": "wangwu"}]}'
```

#### 入参说明

| 参数          | 类型   | 必填 | 说明                                   |
| ------------- | ------ | ---- | -------------------------------------- |
| `meetingid` | string | 是   | 会议 ID，通过 `list_user_meetings` + `get_meeting_info` 获取 |
| `invitees`  | array | 是   | 受邀成员列表，每项包含 `userid` 字段 |

> **注意**: invitees 为全量覆盖，传入的列表将替换现有成员列表。
> invitees 的 userid 通过 `wecomcli-contact` 技能获取

#### 返回参数

```json
{
  "errcode": 0,
  "errmsg": "ok"
}
```

---

## 典型工作流

### 工作流 1: 最简创建 (无邀请人)

**用户意图**: "帮我约一个明天下午3点的会议，主题是周例会，时长1小时"

**步骤:**

1. **解析用户意图**: 时间 + 主题已有，邀请人未提及则默认留空，直接创建。
2. **调用创建命令**:

```bash
wecom-cli meeting create_meeting '{"title": "周例会", "meeting_start_datetime": "2026-03-18 15:00", "meeting_duration": 3600}'
```

3. **展示结果**:

#会议号: <会议号>

```
✅ 会议创建成功!

📅 <会议标题>
🕐 时间: <开始时间>，时长 <时长>
🔗 会议链接: <会议链接>
```

### 工作流 2: 带邀请人 + 地点 + 描述创建

**用户意图**: "帮我约一个明天下午3点的会议，主题是技术方案评审，邀请张三和李四，地点在3楼会议室，时长1小时"

**步骤:**

1. **解析用户意图**: 有邀请人，需先查询通讯录获取 userid。
2. **通讯录查询**: 调用 `wecomcli-contact` 技能获取通讯录成员，按姓名筛选出参与者的 userid。

```bash
wecom-cli contact get_userlist '{}'
```

在返回的 `userlist` 中筛选 `name` 包含 "张三" 和 "李四" 的成员，获取其 `userid`。

3. **信息已充分，直接调用创建命令** (禁止暴露内部 ID):

```bash
wecom-cli meeting create_meeting '{"title": "技术方案评审", "meeting_start_datetime": "2026-03-18 15:00", "meeting_duration": 3600, "location": "3楼会议室", "invitees": {"userid": ["zhangsan", "lisi"]}}'
```

4. **展示结果**:

#会议号: <会议号>

```
✅ 会议创建成功!

📅 <会议标题>
🕐 时间: <开始时间>，时长 <时长>
👥 参与人: <参与者姓名列表>
🔗 会议链接: <会议链接>
```

---

### 工作流 3: 查询会议列表

**示例**: 用户说 "帮我查一下本周有哪些会议"

**步骤:**

1. **确定时间范围**: 根据当前日期计算本周的起止时间。
2. **查询会议 ID 列表**:

```bash
wecom-cli meeting list_user_meetings '{"begin_datetime": "2026-03-16 00:00", "end_datetime": "2026-03-22 23:59", "limit": 100}'
```

3. **逐个查询会议详情** (对返回的每个 meetingid):

```bash
wecom-cli meeting get_meeting_info '{"meetingid": "<会议id1>"}'
```
```bash
wecom-cli meeting get_meeting_info '{"meetingid": "<会议id2>"}'
```

4. **汇总展示**:

```
📋 本周会议列表 (共 3 场):

1. 📅 技术方案评审
   🕐 2026-03-17 10:00 - 11:00
   👥 张三，李四，王五

2. 📅 产品需求沟通
   🕐 2026-03-18 14:00 - 15:00
   👥 赵六，钱七

3. 📅 周五周会
   🕐 2026-03-21 09:00 - 10:00
   👥 全组成员
```

> **分页处理**: 如果 `next_cursor` 不为空，使用 `cursor` 参数继续拉取下一页。

---

### 工作流 4: 获取会议详情

**示例**: 用户说 "帮我看下技术方案评审会议的详情"

**步骤:**

1. **定位会议**: 先通过会议列表查询找到目标会议的 meetingid (按关键词匹配)。
2. **查询详情**:

```bash
wecom-cli meeting get_meeting_info '{"meetingid": "<target_meetingid>"}'
```

3. **展示结果**:

#会议号: <会议号>

```
📅 <会议标题>

🕐 时间: <开始时间>，时长 <时长>
📍 地点: <会议地点>
📝 描述: <会议描述>
👤 创建者: <创建者姓名>
👥 参与者: <参与者姓名列表>
🔗 会议链接: <会议链接>
```

---

### 工作流 5: 根据关键词查找会议

**示例**: 用户说 "技术评审会议是什么时候?"

**查询策略:**

1. **确定查询范围**: 默认查当日前后 30 天 (接口限制范围)。
2. **拉取会议列表**:

```bash
wecom-cli meeting list_user_meetings '{"begin_datetime": "2026-02-15 00:00", "end_datetime": "2026-04-16 23:59", "limit": 100}'
```

3. **逐个查询详情并匹配标题关键词**。
4. **找到匹配后停止查询，展示结果**:

#会议号: <会议号>

```
✅ 找到会议: "<会议标题>"

📅 时间: <开始时间>，时长 <时长>
📍 地点: <会议地点>
👥 参与者: <参与者姓名列表>
🔗 会议链接: <会议链接>
```

5. **未找到处理**: 告知用户在前后 30 天范围内未找到匹配会议，请确认会议名称。

---

### 工作流 6: 取消会议

**示例**: 用户说 "帮我取消明天的技术方案评审会议"

**步骤:**

1. **定位会议**: 通过 `list_user_meetings` + `get_meeting_info` 查询会议列表 + 关键词匹配找到目标会议。
2. **直接执行取消**:

```bash
wecom-cli meeting cancel_meeting '{"meetingid": "<target_meetingid>"}'
```

3. **展示结果**:

```
✅ 会议已取消: 技术方案评审
```

---

### 工作流 7: 更新会议成员

**示例**: 用户说 "把王五加到技术方案评审会议里"

**步骤:**

1. **定位会议**: 通过 `list_user_meetings` + `get_meeting_info` 查询会议列表 + 匹配找到目标会议。
2. **获取当前受邀成员**: `set_invite_meeting_members` 为全量覆盖，必须先通过 `get_meeting_info` 获取会议详情，获取现有成员后再合并。
3. **通讯录查询**: 调用 `wecomcli-contact` 技能获取通讯录成员，按姓名筛选出王五的 userid。

```bash
wecom-cli contact get_userlist '{}'
```

在返回的 `userlist` 中筛选 `name` 包含 "王五" 的成员，获取其 `userid`。

4. **合并成员列表**: 将现有成员 + 新增成员合并 (全量覆盖)。
5. **执行更新**:

```bash
wecom-cli meeting set_invite_meeting_members '{"meetingid": "<target_meetingid>", "invitees": [{"userid": "zhangsan"}, {"userid": "lisi"}, {"userid": "wangwu"}]}'
```

6. **展示结果**:

```
✅ 会议成员已更新: 技术方案评审
👥 当前成员: 张三，李四，王五
```

---

## 复杂场景样例

按场景按需加载，避免一次性引入过多无关示例:

| 文件 | 适用场景 |
| ---- | -------- |
| [references/response-get-meeting-info.md](references/response-get-meeting-info.md) | 获取会议详情完整返回参数结构和字段说明 |
| [references/example-security.md](references/example-security.md) | 会议密码，等候室，外部用户限制 |
| [references/example-reminder.md](references/example-reminder.md) | 响铃提醒，指定部分人响铃 |
| [references/example-full.md](references/example-full.md) | 全参数综合场景 (含静音，屏幕水印，等候室等设置) |

---

## 注意事项

- **信息追问**: 缺少时间或主题时，简洁追问用户；未提及邀请人则默认留空
- **通讯录查询**: 涉及参与人时，需先通过 `wecomcli-contact` 技能的 `get_userlist` 接口获取全量通讯录成员，再按姓名/别名本地筛选匹配出对应的 `userid`。该接口无入参，返回当前用户可见范围内的成员列表 (含 `userid`，`name`，`alias`)
- **直接创建**: 时间 + 主题已知即可直接创建，邀请人有则带上，无则留空；无论信息是一次性提供还是上下文可推断，非必要则均不请求确认，直接创建即可
- **时间格式**: 统一使用 `YYYY-MM-DD HH:mm` 格式
- **会议列表时间范围限制**: 仅支持查询当日及前后 30 天内的会议
- **查询详情需两步**: 先通过 `list_user_meetings` 获取会议 ID 列表，再通过 `get_meeting_info` 逐个获取详情
- **定位会议**: 取消会议和更新成员等管理操作需先通过查询定位到目标会议的 meetingid
- **成员更新为全量覆盖**: `set_invite_meeting_members` 传入的列表将替换现有成员列表，需先获取当前成员再合并
- **参与人仅支持企业内成员**，不支持外部人员
