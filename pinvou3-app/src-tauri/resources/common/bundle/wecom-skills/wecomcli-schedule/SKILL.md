---
name: wecomcli-schedule
description: 何时用:仅当用户明确指向企业微信日程(查/建/改企微日程)时使用;泛指记个待办、排个日程默认走本地工具或待办技能,不要误用。企微日程:(1) 查询时间范围内的日程列表或日程详情(标题、时间、地点、参与者) (2) 创建日程并设置提醒、参与人 (3) 修改/取消日程 (4) 添加/移除参与人 (5) 查询多成员闲忙并分析共同空闲时段以安排会议。
metadata:
  requires:
    bins: ["wecom-cli"]
  cliHelp: "wecom-cli schedule --help"
---

# 企业微信日程管理技能

通过 `wecom-cli schedule <接口名> '<json入参>'` 与企业微信日程系统交互。

## 注意事项

- 日程列表查询仅支持**当日前后 30 天**，时间格式 `YYYY-MM-DD` 或 `YYYY-MM-DD HH:mm:ss`
- 涉及参与者 userid 时，需先使用 **wecomcli-contact** 技能获取；存在同名时展示候选让用户选择（禁止暴露 userid）
- 创建/修改/取消前，先确认目标日程和参与者信息
- `errcode != 0` 时展示错误；入参用 `YYYY-MM-DD HH:mm:ss` 字符串，返回的 `start_time`/`end_time` 是 Unix 秒级时间戳，展示前需转换。

---

## 接口列表

### get_schedule_list_by_range — 查询日程 ID 列表

```bash
wecom-cli schedule get_schedule_list_by_range '{"start_time": "YYYY-MM-DD HH:mm:ss", "end_time": "YYYY-MM-DD HH:mm:ss"}'
```

返回 `schedule_id_list` 数组。仅支持当日前后 30 天。

### get_schedule_detail — 获取日程详情

```bash
wecom-cli schedule get_schedule_detail '{"schedule_id_list": ["SCHEDULE_ID_1", "SCHEDULE_ID_2"]}'
```

支持 1~50 个 ID，返回日程标题、时间、地点、参与者等。参见 [API 详情](references/get-schedule-detail.md)。

### create_schedule — 创建日程

```bash
wecom-cli schedule create_schedule '{"schedule": {"start_time": "YYYY-MM-DD HH:mm:ss", "end_time": "YYYY-MM-DD HH:mm:ss", "summary": "日程标题", "attendees": [{"userid": "USER_ID"}], "reminders": {"is_remind": 1, "remind_before_event_secs": 3600, "timezone": 8}}}'
```

参见 [API 详情](references/create-schedule.md) | [reminders 字段](references/ref-reminders.md)。

### update_schedule — 修改日程

只需传入需修改的字段，未传字段保持不变。

```bash
wecom-cli schedule update_schedule '{"schedule": {"schedule_id": "SCHEDULE_ID", "summary": "更新后的标题"}}'
```

参见 [API 详情](references/update-schedule.md)。

### cancel_schedule — 取消日程

```bash
wecom-cli schedule cancel_schedule '{"schedule_id": "SCHEDULE_ID"}'
```

### add_schedule_attendees / del_schedule_attendees — 管理参与人

- 添加参与人：
```bash
wecom-cli schedule add_schedule_attendees '{"schedule_id": "SCHEDULE_ID", "attendees": [{"userid": "USER_ID"}]}'
```
- 移除参与人：
```bash
wecom-cli schedule del_schedule_attendees '{"schedule_id": "SCHEDULE_ID", "attendees": [{"userid": "USER_ID"}]}'
```

### check_availability — 查询闲忙

```bash
wecom-cli schedule check_availability '{"check_user_list": ["USER_ID_1", "USER_ID_2"], "start_time": "YYYY-MM-DD HH:mm:ss", "end_time": "YYYY-MM-DD HH:mm:ss"}'
```

支持 1~10 个用户，返回各用户的忙碌时段列表。参见 [API 详情](references/check-availability.md)。

---

## 典型工作流

### 查询日程

**经典 query 示例：**
- "我今天有哪些日程？"
- "查一下最近有没有关于项目评审的日程"

**流程：**
1. 根据用户意图计算时间范围（如"今天"→当日 00:00:00 至 23:59:59，"这周"→本周一至周日）
2. 调用 `get_schedule_list_by_range` 获取日程 ID 列表
3. 调用 `get_schedule_detail` 批量获取详情，将 Unix 时间戳转为可读时间
4. 若用户提到关键词（如"项目评审"），在 `summary` 中匹配筛选；未找到则逐步扩大范围至前后 30 天上限
5. 展示日程列表时包含标题、时间、地点、参与者等关键信息，方便用户快速了解

### 创建日程

**经典 query 示例：**
- "帮我创建一个明天下午 2 点到 3 点的会议，标题叫需求评审"
- "创建日程：后天上午 10 点和张三、李四开产品方案讨论会，地点在 3 楼会议室"

**流程：**
1. 解析用户意图，提取时间、标题、地点、参与人、提醒设置等信息
2. 涉及参与人：先经 wecomcli-contact 取 userid（规则见「注意事项」）
3. 若用户未指定提醒，默认设置提前 15 分钟提醒（`remind_before_event_secs: 900`）
4. 若用户说"全天"，设置 `is_whole_day: 1`，时间设为当天 00:00:00 至 23:59:59
5. 向用户确认日程信息（标题、时间、地点、参与人等）后调用 `create_schedule`

### 修改日程

**经典 query 示例：**
- "把明天的需求评审改到后天下午 3 点"
- "我今天 14 点的日程地点改成线上腾讯会议"

**流程：**
1. 先通过查询工作流定位目标日程（根据用户提到的时间、标题等关键词匹配）
2. 若匹配到多个日程，展示候选列表让用户确认
3. 向用户确认要修改的字段和目标值
4. 调用 `update_schedule`，只传入需修改的字段

### 取消日程

**经典 query 示例：**
- "取消明天下午的需求评审"
- "我不想开今天 15 点的会了，帮我取消"

**流程：**
1. 先通过查询工作流定位目标日程
2. 向用户确认取消的日程信息（标题、时间等），避免误操作
3. 确认后调用 `cancel_schedule`

### 管理参与人

**经典 query 示例：**
- "把张三加到明天的需求评审会议里"
- "明天下午的会议再邀请一下王伟和赵敏"

**流程：**
1. 涉及参与人：先经 wecomcli-contact 取 userid（规则见「注意事项」）
2. 通过查询工作流定位目标日程
3. 调用 `add_schedule_attendees` 或 `del_schedule_attendees` 完成添加/移除

### 查询闲忙并安排会议

**经典 query 示例：**
- "帮我看看张三和李四明天下午有没有空"
- "找一个明天下午大家都有空的时段，安排一个 1 小时的会议"

**流程：**
1. 通过 **wecomcli-contact** 获取相关人员 userid
2. 调用 `check_availability` 查询指定时间范围内各用户的忙碌时段
3. 分析所有用户的忙碌时段，计算出共同空闲时段并推荐给用户
4. 用户确认时段后，调用 `create_schedule` 创建会议并自动添加参与人
