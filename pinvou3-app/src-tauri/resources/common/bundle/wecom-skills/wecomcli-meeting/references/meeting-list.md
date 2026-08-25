# 操作参考：查询会议列表

> [!CAUTION]
> **`meeting get` 单次最多查询 10 个会议**：`meeting_ids` 数组长度上限为 10。超过 10 个 meeting_id 时必须分批多次调用（每批 ≤ 10），分别拿到结果后在 Agent 侧合并；禁止一次性传入 > 10 个 ID（会被服务端拒绝）。例如：拉取了 25 个 meeting_id，需拆成 10 + 10 + 5 三批。
>
> **`meeting list` 必须翻页到底**：`meeting list` 返回中只要 `has_more == true`，就必须携带 `next_cursor` 再次调用 list，循环直到 `has_more == false`，否则会漏数据。

## 命令

```bash
wecom-cli meeting list --json '{...}'
wecom-cli meeting get --json '{...}'
```

## 请求参数（list）

| 字段         | 类型    | 必填 | 说明                                                                                           |
| ------------ | ------- | ---- | ---------------------------------------------------------------------------------------------- |
| `begin_time` | string  | 否   | 查询区间开始时间, 格式 `YYYY-MM-DD HH:mm:ss`, 与 `end_time` 必须同时提供或同时不提供, 不可只传其一 |
| `end_time`   | string  | 否   | 查询区间结束时间, 格式 `YYYY-MM-DD HH:mm:ss`, 与 `begin_time` 必须同时提供或同时不提供, 不可只传其一 |
| `cursor`     | string  | 否   | 分页游标, 首次请求不传                                                                         |
| `limit`      | integer | 否   | 单次返回数量, 默认 20                                                                          |

## 返回字段（list）

> 返回结果分为两个列表: `created_meetings`（当前用户创建的会议）和 `attended_meetings`（当前用户参加但非创建的会议），两个列表结构相同。

| 字段                                           | 说明                                     |
| ---------------------------------------------- | ---------------------------------------- |
| `created_meetings[].meeting_id`                | 会议唯一标识                             |
| `created_meetings[].sub_meeting_id`            | 子会议 ID, 周期会议涉及                  |
| `created_meetings[].subject`                   | 会议主题                                 |
| `created_meetings[].begin_time`                | 会议开始时间, 格式 `YYYY-MM-DD HH:mm:ss` |
| `created_meetings[].end_time`                  | 会议结束时间, 格式 `YYYY-MM-DD HH:mm:ss` |
| `created_meetings[].attendee_count`            | 参会人数                                 |
| `created_meetings[].meeting_room`              | 会议室名称                               |
| `created_meetings[].location`                  | 会议地点                                 |
| `created_meetings[].is_repeat_meeting`         | 是否为周期性会议                         |
| `created_meetings[].timezone.timezone_id`      | 时区 ID, 如 `"Asia/Shanghai"`            |
| `created_meetings[].timezone.timezone_offset`  | 时区偏移量（秒）, 如 28800               |
| `attended_meetings[].meeting_id`               | 会议唯一标识                             |
| `attended_meetings[].sub_meeting_id`           | 子会议 ID, 周期会议涉及                  |
| `attended_meetings[].subject`                  | 会议主题                                 |
| `attended_meetings[].begin_time`               | 会议开始时间, 格式 `YYYY-MM-DD HH:mm:ss` |
| `attended_meetings[].end_time`                 | 会议结束时间, 格式 `YYYY-MM-DD HH:mm:ss` |
| `attended_meetings[].attendee_count`           | 参会人数                                 |
| `attended_meetings[].meeting_room`             | 会议室名称                               |
| `attended_meetings[].location`                 | 会议地点                                 |
| `attended_meetings[].is_repeat_meeting`        | 是否为周期性会议                         |
| `attended_meetings[].timezone.timezone_id`     | 时区 ID, 如 `"Asia/Shanghai"`            |
| `attended_meetings[].timezone.timezone_offset` | 时区偏移量（秒）, 如 28800               |
| `attended_meetings[].creator_name`             | 会议创建人名称                           |
| `created_meetings_count`    | `created_meetings` 数组元素数量  |
| `attended_meetings_count`           | `attended_meetings` 数组元素数量 |
| `next_cursor`                                  | 下一页游标, `has_more` 为 true 时有效    |
| `has_more`                                     | 是否还有更多数据                         |

## 请求参数（get）

> **输入格式强制要求**：`meeting_ids` 必须使用以下嵌套对象数组结构传入，不可简化为字符串数组：
> ```json
> {
>   "meeting_ids": [
>     {
>       "meeting_id": "会议ID",
>       "sub_meeting_id": "子会议ID"
>     }
>   ]
> }
> ```
> 每个元素必须是包含 `meeting_id`（必填）和可选 `sub_meeting_id` 的对象，**不得直接传字符串**。

| 字段                           | 类型   | 必填 | 说明                                                 |
| ------------------------------ | ------ | ---- | ---------------------------------------------------- |
| `meeting_ids`                  | array  | 是   | 会议 ID 列表，最少 1 个，最多 10 个。超过 10 个时必须分批请求，每批不超过 10 个。**每个元素必须是对象（含 `meeting_id` 字段），不可传字符串** |
| `meeting_ids[].meeting_id`     | string | 是   | 会议 ID（长字符串, 如 `mtkSFfCg...`）, 非 9 位会议号 |
| `meeting_ids[].sub_meeting_id` | string | 否   | 子会议 ID, 周期会议需指定                            |

## 返回字段（get）

| 字段                                                            | 说明                                                                                                             |
| --------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| `meetings[].meeting_id`                                         | 会议 ID                                                                                                          |
| `meetings[].sub_meeting_id`                                     | 子会议 ID, 周期会议当前子会议 ID                                                                                 |
| `meetings[].subject`                                            | 会议主题                                                                                                         |
| `meetings[].begin_time`                                         | 开始时间, 格式 `YYYY-MM-DD HH:mm:ss`                                                                             |
| `meetings[].end_time`                                           | 结束时间, 格式 `YYYY-MM-DD HH:mm:ss`                                                                             |
| `meetings[].current_user_enter_time`   | 当前调用用户的入会时间, 格式 `YYYY-MM-DD HH:mm:ss`, 取最早一次入会时间; **仅会议结束后返回, 未入会时为空** |
| `meetings[].current_user_quit_time`        | 当前调用用户的离会时间, 格式 `YYYY-MM-DD HH:mm:ss`, 取最晚一次离会时间; **仅会议结束后返回, 未入会时为空** |
| `meetings[].timezone.timezone_id`                               | 时区 ID, 如 `"Asia/Shanghai"`                                                                                    |
| `meetings[].timezone.timezone_offset`                           | 时区偏移量（秒）, 如 28800                                                                                       |
| `meetings[].meeting_room`                                       | 会议室名称                                                                                                       |
| `meetings[].location`                                           | 会议地点                                                                                                         |
| `meetings[].description`                                        | 会议备注描述                                                                                                     |
| `meetings[].repeat_rule`                                        | 周期规则, 非周期会议为空                                                                                         |
| `meetings[].repeat_rule.repeat_type`                            | 周期类型: `"daily"`-每天, `"weekday"`-每个工作日, `"weekly"`-每周, `"biweekly"`-每两周, `"monthly"`-每月         |
| `meetings[].repeat_rule.repeat_days`                            | 重复天数                                                                                                         |
| `meetings[].repeat_rule.until_type`                             | 结束方式: `"by_date"`-按日期结束, `"by_times"`-按次数结束                                                        |
| `meetings[].repeat_rule.until_date`                             | 周期结束日期, 格式 `YYYY-MM-DD HH:mm:ss`（until_type=`"by_date"` 时有效）                                        |
| `meetings[].repeat_rule.until_times`                            | 结束次数（until_type=`"by_times"` 时有效）                                                                       |
| `meetings[].repeat_rule.version`                                | 重复规则版本, 默认 0                                                                                             |
| `meetings[].repeat_rule.first_begin_time`                       | 第一次开始时间, 格式 `YYYY-MM-DD HH:mm:ss`                                                                       |
| `meetings[].repeat_rule.first_end_time`                         | 第一次结束时间, 格式 `YYYY-MM-DD HH:mm:ss`                                                                       |
| `meetings[].repeat_rule.repeat_step`                            | 每 n（天/周/月）重复一次, 与 repeat_type 配合使用; 例如 repeat_step=3, repeat_type=`"daily"` 表示每 3 天重复一次 |
| `meetings[].meeting_status`                                     | 会议状态: `"init"`-未开始, `"started"`-进行中, `"end"`-已结束（终止态, 不回退）                                  |
| `meetings[].attendees`                                          | 参会人列表，扁平对象数组，企业内部成员与外部成员混在同一数组，通过 `is_external` 区分                             |
| `meetings[].attendees[].userid`                                 | 成员 userid，内部成员与外部联系人统一用此字段，由 `is_external` 区分（内部 `wo` 前缀、外部 `wm` 前缀） |
| `meetings[].attendees[].name`                                   | 参会人名称（如 `"zhangsan(张三)"`），展示时**原样取此字段**（完全与接口返回的格式保持一致），禁止展示 userid                                       |
| `meetings[].attendees[].is_external`                            | 是否为外部联系人（bool）                                                                                         |
| `meetings[].attendees[].is_attended`                            | 是否已入会（bool）                                                                                               |
| `meetings[].attendees[].duration`                               | 参会时长（秒）                                                                                                   |
| `meetings[].notes[].note_content`                               | 智能纪要文字内容（每个媒体房间一条，最多 10 条）                                                                 |
| `meetings[].notes[].todo_content`                               | 智能纪要待办内容                                                                                                 |
| `meetings[].note_url`                                           | 会议智能纪要 URL（如 `"https://xxx"`）。**仅当用户明确询问会议链接 / 纪要链接时才展示**，其余情况不主动输出；**只要展示链接，就必须用 markdown 跳转链接格式 `[会议主题](链接)`**，`[]` 内放该会议主题（`subject`，如 `[产品评审周会](https://xxx)`），禁止裸贴 URL、禁止用固定文案 |
| `meetings[].has_note_permission`                                | 是否有会议纪要权限                                                                                               |
| `meetings[].record_url`                                         | 会议录制地址 URL（如 `"https://xxx"`）。**仅当用户明确询问录制链接 / 回放链接时才展示**，其余情况不主动输出；**只要展示链接，就必须用 markdown 跳转链接格式 `[会议主题](链接)`**，`[]` 内放该会议主题（`subject`，如 `[产品评审周会](https://xxx)`），禁止裸贴 URL、禁止用固定文案 |
| `meetings[].is_except_meet`                                     | 是否是例外（周期会议中被单独修改的子会议）                                                                       |
| `meetings_count`         | `meetings` 数组元素数量            |

## 约束

- `begin_time` 和 `end_time` 必须同时提供或同时不提供，不可只传其中一个
- `meeting get` 单次传入 1～10 个会议 ID，超出需分批请求
- `meeting_ids` 必须传入对象数组（每个元素含 `meeting_id` 字段），**禁止简化为字符串数组**，如 `["id1","id2"]` 格式是错误的
- `meeting_id` 是长字符串（如 `mtkSFfCg...`）, 不要误传 9 位数字会议号
- 参会人 `name` 由接口直接返回，正常无需通讯录反查；`name` 为空时用 `userid` 通过 `读取 wecomcli-contact 技能` 反查姓名，禁止直接展示 userid
- `notes` 字段包含文字版智能纪要内容，每个媒体房间一条，最多 10 条；`has_note_permission` 为 false 时不展示纪要内容
- **作为「会议总结」用途时 [REQUIRED]**：**只有用户纯粹地说"总结下 / 讲了啥 / 纪要发我 / 看待办"、不带任何自定义描述时，才走本 get 返回现成内容**；取目标字段——要纪要看 `notes[].note_content`、要待办看 `notes[].todo_content`；`has_note_permission == true` 且目标字段有实质内容时**直接返回该现成内容**（无需再调用转写原文接口）；目标字段为空或 `has_note_permission == false` 时，转 [meeting-original-get](meeting-original-get.md) 拉转写原文兜底再总结。**只要用户附带了任何自定义要求/描述**（指定结构/角度/范围/风格/长度等），就跳过本 get、直接走原文加工（详见 [SKILL.md 核心场景 7](../SKILL.md)）。
- **链接展示格式（`note_url` 会议纪要链接、`record_url` 会议录制链接）[CRITICAL]**：
  - **默认不展示**：`note_url` 仅当用户明确询问会议链接 / 纪要链接时才输出；`record_url` 仅当用户明确询问录制链接 / 回放链接时才输出；其余情况一律不主动输出。
  - **展示格式强约束**：**只要要展示这两类链接，就必须用 markdown 跳转链接格式 `[会议主题](链接)`**——`[]` 内放该会议主题（`subject`），`()` 内放对应 URL，如 `[产品评审周会](https://xxx)`。
  - **严禁**：直接裸贴 URL、用「点击查看」等固定文案代替会议主题、或以纯文本形式输出链接。

## 工作流

> **模糊查询前置 [REQUIRED]**：若本次是"会 / xx会 / xx会议 / 有什么会 / 最近有哪些会"等模糊查询（见 [SKILL.md 查询消歧](../SKILL.md)），除按下面拉会议 `list` 外，必须同时 `读取 wecomcli-calendar 技能` 用相同时间范围拉日程 `list`，把两边结果合并、分「（会议）」「（日程）」两部分汇总展示（同一场会议按主题 + 时间去重）——不论会议是否查到都要查日程。仅当用户明确指向在线会议（入会链接 / 会议号 / 视频会议等）时才只查会议。

### 正常路径

1. **确认时间范围**：从用户意图提取时间范围。
   - 用户已明确时间（如"今天"、"本周"、"4月15日到4月20日"）→ 直接映射为 `begin_time`/`end_time`
   - 用户未明确时间（如"查一下我的会议"）→ **使用默认策略：今天起未来 7 天**（无需追问）
   - 用户说"最近"或"近期" → 使用"过去 3 天到未来 7 天"
   - 用户只提供了模糊但有意义的范围（如"上个月"）→ 解析为对应日期范围
2. **拉取会议列表**：调用 `wecom-cli meeting list --json '{...}'`, 获取 `created_meetings` 和 `attended_meetings`。若 `has_more` 为 true, 携带 `next_cursor` 继续翻页, 直至获取全部 `meeting_id`（用于统计总条数 N）。
3. **获取详情**：按开始时间升序排序后，**只对要展示的前 10 条** `meeting_id` 调用 `wecom-cli meeting get --json '{...}'` 反查详情（每批 ≤ 10 个）；其余条数计入"还有 N 条"，不必逐一取详情。
4. **展示参会人名称**：原样使用详情中 `attendees[].name`（完全与接口返回的格式保持一致）；`name` 为空时用该参会人 `userid` 通过 `读取 wecomcli-contact 技能` 反查姓名，禁止直接展示 userid。
5. **顺序输出**：禁止 markdown 表格，每条会议作为独立条目顺序输出，每个条目只含主题/时间/参会人；超过 10 条只展示前 10 条，末尾告知"还有 N 条，需要查看更多吗？"。

### 异常路径

| 异常情况 | 处理方式 |
|---------|---------|
| 列表为空 | 不要直接告知"无会议"——企微里「会」有「含在线会议链接的会议」和「日程」两种载体，团队聚一起的会常落在日程而非会议。先主动 `读取 wecomcli-calendar 技能` 用相同时间范围（及用户提及的关键词/参会人）在日程里查一把：命中则一并呈现并说明「这是一条日程，未关联在线会议链接」；日程也无果，再告知用户该时间段内会议和日程均无安排，并建议扩大时间范围 |
| 翻页过程中出错 | 展示已获取的部分结果, 告知用户可能有更多未加载的数据 |
| 详情获取失败（部分 ID） | 展示成功获取的会议, 标注获取失败的条目 |
| 参会人 `name` 字段为空 | 用该参会人 `userid` 通过 `读取 wecomcli-contact 技能` 反查姓名；反查不到再告知该参会人信息暂时无法获取。禁止直接展示 userid |

## 翻页策略

- `meeting list` 使用 `cursor`/`next_cursor` + `has_more` 分页
- `has_more` 为 true 时必须携带 `next_cursor` 继续翻页, 直至获取全部数据
- 周期会议需同时传入 `sub_meeting_id` 才能获取正确的子会议详情

## 示例请求

**list 请求**：
```json
{
  "begin_time": "2026-04-07 00:00:00",
  "end_time": "2026-04-07 23:59:59",
  "limit": 20
}
```

**get 请求**：
```json
{
  "meeting_ids": [
    { "meeting_id": "<meeting_id_1>" },
    { "meeting_id": "<meeting_id_2>", "sub_meeting_id": "<sub_meeting_id>" }
  ]
}
```

## 典型场景

### 1. 明确指定时间范围

```
用户：帮我看看今天有什么会议
→ 用户已明确"今天"，直接映射：begin_time=今天 00:00:00，end_time=今天 23:59:59
→ 调用 meeting list 获取 created_meetings + attended_meetings 列表
→ 按开始时间升序，对前 10 条调用 meeting get 反查参会人姓名
→ 顺序输出每条会议（主题/时间/参会人，禁止 markdown 表格）：

1. 项目复盘
   时间：4月7日（周二）09:00-10:00
   参会人：赵六、钱七

2. 产品评审
   时间：4月7日（周二）14:00-15:00
   参会人：张三、李四、王五
```

### 2. 未指定时间范围，使用默认策略

```
用户：帮我看看有什么会议
→ 未指定时间范围，直接使用默认策略：今天起未来 7 天
   begin_time = 今天 00:00:00，end_time = 7 天后 23:59:59
→ 调用 meeting list，对前 10 条调用 meeting get 反查参会人姓名
→ 顺序输出每条会议（主题/时间/参会人，禁止 markdown 表格），超过 10 条只展示前 10 条 + "还有 N 条，需要查看更多吗？"
```

### 3. 查询结果为空（用日程兜底）

```
用户：帮我看看这周有什么会
→ 用户已明确"这周"，映射为本周一 00:00:00 ~ 本周日 23:59:59
→ 调用 meeting list → created_meetings 和 attended_meetings 均为空
→ 软性兜底：「会」在企微可能是日程，主动 `读取 wecomcli-calendar 技能` 用同样时间范围在日程里查一把
  - 日程命中 → 一并呈现并说明：在「日程」里找到了本周的安排（这是日程，未关联在线会议链接），随后展示日程列表
  - 日程也无果 → 告知用户：本周（4月7日-4月13日）会议和日程里都没有安排。
```

