---
name: wecomcli-meeting
description: 何时用:仅当用户明确指向企业微信会议(用企微开会、约企微会议)时使用;泛指约会议/查日程默认走日程技能或本地工具,不要误用。企微会议:创建预约会议、查询会议列表、获取会议详情、取消会议、更新会议成员。用户需要"创建/预约/安排会议"、"查看会议列表"、"会议详情"、"什么时候开会"、"查找/取消会议"、"修改会议成员"、"添加/移除会议参与人"时使用。
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

## 更多工作流

按场景按需加载，避免一次性引入过多示例:

| 文件 | 适用场景 |
| ---- | -------- |
| [references/workflows.md](references/workflows.md) | 查询会议列表、获取详情、按关键词查找会议、取消会议、更新会议成员(含展示格式模板) |

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

- **信息追问**: 缺少时间或主题时简洁追问;未提及邀请人则默认留空。时间 + 主题已知即可直接创建,非必要不请求确认
- **通讯录查询**: 涉及参与人时,先用 `wecomcli-contact` 的 `get_userlist`(无入参)获取可见范围成员(含 `userid`、`name`、`alias`),按姓名/别名本地筛选匹配 userid
- **定位会议**: 取消/更新成员前,先 `list_user_meetings` + `get_meeting_info` 定位 meetingid(查询详情本就需这两步)
- **成员更新为全量覆盖**: `set_invite_meeting_members` 替换现有成员列表,需先取当前成员再合并
- **参与人仅支持企业内成员**
