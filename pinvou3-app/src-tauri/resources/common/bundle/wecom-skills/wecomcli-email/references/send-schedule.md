# 邮件日程与会议参数说明

**适用场景**：用户需要发送日程邀约或会议邮件时，需要额外组装 `schedule`（以及可选的 `meeting`）对象。普通邮件无需关注本文档。

---

## 日程与会议的关系

- **日程邀约**：只需填 `schedule`，不需要 `meeting`。适用于用户没有明确说要"开会/会议"的场景，如日程提醒、活动通知、约碰头等
- **会议邮件**：必须**同时**填 `schedule` 和 `meeting`。只要用户明确说要"发会议邮件"、"约个会议"等，即视为会议邮件，**不区分线下还是线上**（线下会议也会创建，用户可自行选择是否使用线上会议室，线下地点通过 `location` 字段承载）。单独填 `meeting` 而不填 `schedule` 会导致接口报错
- 判断依据：用户说"开会"、"开个线上会议"、"拉个视频会"、"约腾讯会议"→ 会议邮件（schedule + meeting）；用户说"发个日程"、"约个碰头"、"提醒大家周五有活动"→ 日程邀约（仅 schedule）。不确定时直接问用户"需要创建线上会议室吗？"

## schedule 参数补全

以下参数用户未提供时**必须用自然语言追问用户补全，禁止猜测或使用默认值**：

| 参数 | 格式 | 追问示例 |
|---|---|---|
| 开始时间 (`begin_time`) | `YYYY-MM-DD HH:mm:ss` | "请问日程/会议的开始时间是？" |
| 结束时间 (`end_time`) | `YYYY-MM-DD HH:mm:ss` | "结束时间是几点？"（如果用户只说了"开一小时的会"，可自行推算） |

以下参数有合理默认值，用户未提供时**可使用默认值**，无需追问：

| 参数 | 默认值 | 说明 |
|---|---|---|
| `method` | `"request"` | 固定值，不需要向用户询问 |
| `location` | 不填 | 可选，用户提到地点时才填 |
| `reminders.is_remind` | `true` | 默认开启提醒 |
| `reminders.remind_before_event_mins` | `15` | 默认提前 15 分钟提醒 |
| `reminders.timezone` | `{"timezone_id": "Asia/Shanghai", "timezone_offset": 28800}` | 默认北京时间；`timezone_id` 为 IANA 时区标识，`timezone_offset` 为相对 UTC 的秒数偏移 |
| `reminders.is_repeat` | `false` | 默认不重复 |

---

## 重复规则（用户明确要求时才填）

当用户要求日程重复（如"每周三都开"、"每天提醒我"），需要组装 `reminders` 中的重复相关字段：

- `is_repeat`: `true`
- `is_custom_repeat`: 当用户要求特定日期重复时设为 `true`（如"每周三和周五"）
- `repeat_type`: `daily` / `weekly` / `monthly` / `yearly`
- `repeat_interval`: 重复间隔（如"每两周"则为 2），仅自定义重复时有效
- `repeat_day_of_week`：每周周几重复，取值为英文缩写字符串（`MO`=周一，`TU`=周二，`WE`=周三，`TH`=周四，`FR`=周五，`SA`=周六，`SU`=周日），仅 `repeat_type=weekly` 且自定义重复时有效
- `repeat_day_of_month`: 每月哪几天重复，取值 1~31，仅 `repeat_type=monthly` 或 `yearly` 时有效
- `repeat_month_of_year`: 每年哪几个月重复，取值 1~12，仅 `repeat_type=yearly` 时有效
- `repeat_until`: 重复结束时刻（格式 `YYYY-MM-DD HH:mm:ss`），不填表示一直重复

> **注意**：音视频会议（即同时填了 `meeting` 的场景）对重复规则有限制，某些重复组合不被支持。如果接口拒绝重复规则，按 SKILL.md「接口失败处理规范」展示 `error.message` 和 `error.instruction`，告知用户调整重复规则。

---

## 日程管理员（可选）

`schedule_admins` 最多指定 3 人，且必须是同企业用户且在邮件参与人（收件人/抄送人）中。不填时所有参与人权限相同。当用户说"让张三来管理这个日程"时才填。

---

## meeting 参数补全（仅会议邮件场景）

当判定为会议邮件时，组装 `meeting` 对象。以下参数均有合理默认值，用户未提到时**使用默认值**：

| 参数 | 默认值 | 说明 |
|---|---|---|
| `meeting_admins` | 不填（默认为发件人） | 仅可指定 1 人，用户说"让 xx 管理会议"时才填 |
| `hosts` | 不填 | 会议主持人，最多 10 人，用户说"xx 来主持"时才填 |
| `option.password` | 不填（无密码） | 4~6 位纯数字，用户说"加个会议密码"时才填 |
| `option.auto_record` | `"off"` | 用户说"自动录制"时改为 `"cloud"` 或 `"local"` |
| `option.enable_waiting_room` | `false` | 用户说"开等候室"时设 `true` |
| `option.allow_enter_before_host` | `false` | 用户说"允许提前入会"时设 `true` |
| `option.enable_screen_watermark` | `false` | 用户说"开屏幕水印"时设 `true` |
| `option.enable_enter_mute` | `"auto_over_6"` | 默认超过 6 人自动静音 |
| `option.enter_restraint` | `"all"` | 用户说"只允许企业内部人员"时改为 `"internal_only"` |
| `option.remind_scope` | `"host_only"` | 用户说"提醒所有人入会"时改为 `"all"` |
| `option.water_mark_type` | `"single"` | 默认单排水印 |

---

## 关键注意点

- **会议邮件必须同时带 `schedule`**：`meeting` 对象不能单独使用，必须同时填写 `schedule`。漏掉 `schedule` 会导致接口报错。日程邀约则可以不填 `meeting`
- **`begin_time` 不能小于当前时间**：接口会校验 `begin_time`，过去的时间会被接口拒绝。若用户提供的开始时间早于当前系统时间，必须用自然语言询问用户重新选择时间，禁止自行调整或猜测
- **会议持续时间不超过 24 小时**：`end_time` 减 `begin_time` 超过 24 小时会被接口拒绝
- **会议对重复规则有限制**：音视频会议不是所有重复规则都支持，接口拒绝时按 SKILL.md「接口失败处理规范」展示 `error.message` 和 `error.instruction` 告知用户调整
