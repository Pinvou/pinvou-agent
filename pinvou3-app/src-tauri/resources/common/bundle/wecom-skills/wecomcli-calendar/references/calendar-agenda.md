# calendar schedules list / get — 查看日程安排

查看近期日程安排或获取日程详情。只读操作，不修改任何日程。

> **只给时间/日期时必须用 `list` [REQUIRED]**：用户只提供了时间/日期（如"19号那条"）而没有日程主题关键词时，必须走本文档的 `list` 按时间浏览，禁止把日期当关键词喂给 `search`。

> **模糊查询同时拉会议 [REQUIRED]**：若本次是"会 / xx会 / 有什么会 / 最近有哪些会"等模糊查询（见 [SKILL.md 查询消歧](../SKILL.md)），除拉日程 `list` 外，必须同时 `读取 wecomcli-meeting 技能` 用相同时间范围拉会议 `list`，把两边结果合并、分「（会议）」「（日程）」两部分汇总展示（同一场会议按主题 + 时间去重）——不论日程是否查到都要查会议。明确是日程 / 安排（不带在线会议特征）时只查日程。

## 命令

### list — 读取日程列表

```bash
# 查看今天日程
wecom-cli calendar schedules list --json '{"begin_time": "2026-04-07 00:00:00", "end_time": "2026-04-07 23:59:59"}'

# 查看本周日程
wecom-cli calendar schedules list --json '{"begin_time": "2026-04-06 00:00:00", "end_time": "2026-04-12 23:59:59"}'
```

**参数：**

| 参数 | 类型 | 必填 | 说明 |
|------|------|:----:|------|
| `begin_time` | string | 否 | 查询开始时间（格式 YYYY-MM-DD HH:mm:ss）。必须与 `end_time` 同时传入或同时省略，禁止单独传入其中一个。 |
| `end_time` | string | 否 | 查询结束时间（格式 YYYY-MM-DD HH:mm:ss）。必须与 `begin_time` 同时传入或同时省略，禁止单独传入其中一个；同时传入时，`end_time` 必须晚于 `begin_time`。 |

> **时间参数约束**：`begin_time` 和 `end_time` 必须**同时存在**或**同时为空**，禁止只传其中一个。两者同时传入时，`end_time` 必须严格晚于 `begin_time`，否则视为非法参数。
>
> **查询窗口上限：当前时刻前后 30 天 [REQUIRED]**：`schedules list` 仅支持查询**当前时刻前后 30 天以内**的日程，超出范围的部分服务端不返回。
> - 用户给的时间范围部分或完全超出窗口（`begin_time` 早于「今天 - 30 天」或 `end_time` 晚于「今天 + 30 天」）时，**直接告知用户「日程查询仅支持当前时刻前后 30 天范围内，请重新给一个更短的时间范围」**，等用户重新提供时间后再调用。
>
> **未指定时间时的默认范围策略 [REQUIRED]**：调用前先显式计算好时间范围再传入，不依赖服务端默认值——
> - 用户已明确时间（如"今天"、"本周"、"4月15日到4月20日"）→ 直接映射为 `begin_time`/`end_time`。
> - 用户未明确时间（如"查一下我的日程"、"看看我的安排"）→ **默认策略：今天起未来 7 天**（`begin_time = 今天 00:00:00`，`end_time = 7 天后 23:59:59`），无需追问。
> - 用户说"最近"或"近期" → 使用"过去 3 天到未来 7 天"（`begin_time = 3 天前 00:00:00`，`end_time = 7 天后 23:59:59`）。
> - 用户只提供了模糊但有意义的范围（如"上个月"）→ 解析为对应日期范围。

**返回**：`schedule_list[]` 数组，每项字段如下：

| 字段 | 类型 | 说明 |
|------|------|------|
| `schedule_id` | string | 日程 ID |
| `subject` | string | 日程主题 |
| `begin_time` | string | 开始时间（YYYY-MM-DD HH:mm:ss） |
| `end_time` | string | 结束时间（YYYY-MM-DD HH:mm:ss） |
| `attendees` | object[] | 参与人列表，格式 `[{"userid": "USERID", "name": "englishname(name)"}]` |
| `meeting_room` | object | 会议室信息，含 `meeting_room_id` + `meeting_room_name` |
| `location` | string | 日程地点 |
| `meeting` | object | 在线会议信息（关联了会议时才有），含 `meeting_id`/`meeting_code` |
| `description` | string | 日程描述 |
| `creator_name` | string | 日程创建者名字 |
| `allow_self_join` | bool | 是否允许非参与人主动加入日程 |
| `is_all_day` | bool | 是否全天事件（`true` 是 / `false` 否） |
| `repeat_rule` | object | 重复规则（`is_repeat=false` 时无此字段或为空），见下表 |
| `reminders` | object | 提醒设置，含 `is_remind`（是否开启，bool，`true` 是 / `false` 否）和 `reminder_time`（int[]，与开始时间的差值秒数，负数为提前提醒） |
| `timezone` | object | 时区信息，含 `timezone_id`（IANA 标识，如 `Asia/Shanghai`，优先使用）和 `timezone_offset`（UTC 偏移量秒数，`timezone_id` 为空时使用） |

**`repeat_rule` 子字段（list 返回）：**

| 字段 | 类型 | 说明 |
|------|------|------|
| `is_repeat` | bool | 是否重复日程 |
| `repeat_type` | string | 重复类型：`daily`/`weekly`/`monthly`/`monthly_on_the_nth_day`/`yearly`/`yearly_on_the_nth_day`/`work_day` |
| `repeat_flag` | string[] | 重复标记，数组，可选值：`leap_month`（闰月）、`never_ends`（永不结束） |
| `repeat_time` | int | 重复次数，`0` 表示无限 |
| `repeat_interval` | int | 重复间隔 |
| `repeat_until` | string | 重复截止时间（格式 YYYY-MM-DD HH:mm:ss） |
| `repeat_week_of_month` | string[] | 每月第几周，数组，可选值：`first`/`second`/`third`/`fourth`/`last` |
| `repeat_day_of_week` | string[] | 每周周几，数组，可选值：`MO`/`TU`/`WE`/`TH`/`FR`/`SA`/`SU` |
| `repeat_month_of_year` | int[] | 每年哪几个月，数组，取值范围：1~12 |
| `repeat_day_of_month` | int[] | 每月哪几天，数组，取值范围：1~31 |
| `is_custom` | bool | 是否自定义重复 |
| `exception` | object[] | 例外日程列表，每项含 `begin_time`/`end_time`/`flag`/`except_schedule_id` |

### get — 读取日程详情

```bash
wecom-cli calendar schedules get --json '{"schedule_ids": ["<schedule_id1>", "<schedule_id2>"]}'
```

**参数：**

| 参数 | 类型 | 必填 | 说明 |
|------|------|:----:|------|
| `schedule_ids` | string[] | 是 | 日程 ID 列表，支持传入一个或多个 |

**返回**：`schedule_list[]` 数组，每项字段如下：

| 字段 | 类型 | 说明 |
|------|------|------|
| `schedule_id` | string | 日程 ID |
| `subject` | string | 日程主题 |
| `begin_time` | string | 开始时间（YYYY-MM-DD HH:mm:ss） |
| `end_time` | string | 结束时间（YYYY-MM-DD HH:mm:ss） |
| `attendees` | object[] | 参与人列表，格式 `[{"userid": "USERID", "name": "englishname(name)"}]`，直接取 `name` 展示，禁止展示 userid |
| `meeting_room` | object | 会议室信息，含 `meeting_room_id` + `meeting_room_name` |
| `location` | string | 日程地点 |
| `meeting` | object | 在线会议信息（关联了会议时才有），含 `meeting_id`/`meeting_code` |
| `description` | string | 日程描述 |
| `creator_name` | string | 日程创建者名字 |
| `allow_self_join` | bool | 是否允许非参与人主动加入日程 |
| `is_all_day` | bool | 是否全天事件（`true` 是 / `false` 否） |
| `repeat_rule` | object | 重复规则（`is_repeat=false` 时无此字段或为空），见下表 |
| `reminders` | object | 提醒设置，含 `is_remind`（是否开启，bool，`true` 是 / `false` 否）和 `reminder_time`（int[]，与开始时间的差值秒数，负数为提前提醒） |
| `timezone` | object | 时区信息，含 `timezone_id`（IANA 标识，如 `Asia/Shanghai`，优先使用）和 `timezone_offset`（UTC 偏移量秒数，`timezone_id` 为空时使用） |

**`repeat_rule` 子字段：**

| 字段 | 类型 | 说明 |
|------|------|------|
| `is_repeat` | bool | 是否重复日程 |
| `repeat_type` | string | 重复类型：`daily`/`weekly`/`monthly`/`monthly_on_the_nth_day`/`yearly`/`yearly_on_the_nth_day`/`work_day` |
| `repeat_flag` | string[] | 重复标记，数组，可选值：`leap_month`（闰月）、`never_ends`（永不结束） |
| `repeat_time` | int | 重复次数，`0` 表示无限 |
| `repeat_interval` | int | 重复间隔 |
| `repeat_until` | string | 重复截止时间（格式 YYYY-MM-DD HH:mm:ss） |
| `repeat_week_of_month` | string[] | 每月第几周，数组，可选值：`first`/`second`/`third`/`fourth`/`last` |
| `repeat_day_of_week` | string[] | 每周周几，数组，可选值：`MO`/`TU`/`WE`/`TH`/`FR`/`SA`/`SU` |
| `repeat_month_of_year` | int[] | 每年哪几个月，数组，取值范围：1~12 |
| `repeat_day_of_month` | int[] | 每月哪几天，数组，取值范围：1~31 |
| `is_custom` | bool | 是否自定义重复 |
| `exception` | object[] | 例外日程列表，每项含 `begin_time`/`end_time`/`flag`/`except_schedule_id` |

## 输出格式

将结果整理为顺序的日程列表（**禁止使用 markdown 表格**，每条日程作为独立条目顺序输出，按开始时间升序排序）：

```
（会议）
1. 产品评审
   时间：4月7日 10:00-11:00
   参与人：王五、赵六、钱七

（日程）
1. 站会
   时间：4月7日 09:30-09:45
   参与人：张三、李四

共 2 场，其中会议 1 场、日程 1 场
```

> 上例为"模糊会议查询"（日程 + 会议都查）且**两类同时存在**时的呈现：合并日程 `list` 与会议 `list` 的结果，按是否含在线会议链接分成「（会议）」「（日程）」两个部分（来自会议 `list` 或 `meeting.meeting_code` 非空者归会议），同一场会议两边都出现时按"主题 + 时间"去重，末尾给汇总；若本次结果只有单一类别（全是会议或全是日程），则不分部分、不加「（会议）」/「（日程）」标题，按普通列表直接展示；普通"看日程"查询也可不分部分、省略汇总行。

**展示规则：**
- 每个条目 **只展示三项：主题、时间、参与人**（不展示地点、会议室等其他字段）。
- **时间默认省略年份**（只到月日）；仅当日程年份与当前年份不同（跨年）时，才在月日前带上年份。
- **昨天 / 今天 / 明天**的日程，时间行在月日前加相对词（如 `明天 6月11日 14:00-15:00`）；其余日期按月日展示。
- **超过 10 条时只展示前 10 条**，并在末尾告知"还有 N 条，需要查看更多吗？"。
- 参与人原样取接口返回的 `attendees[].name` 展示（完全与接口返回的格式保持一致，如返回 `zhangsan(张三)` 就展示 `zhangsan(张三)`），禁止展示 userid、schedule_id。
- **会议 / 日程 分两部分展示**：判断依据是该日程是否带有会议链接——`meeting.meeting_code` 有值（非空）归为「会议」，为空 / 不存在归为「日程」。**仅当本次结果中同时存在「会议」和「日程」两类时**，才把结果分成「（会议）」和「（日程）」两个部分分别展示（每部分内按开始时间升序、逐条只列主题/时间/参与人）；**当结果只有单一类别时**（全是会议或全是日程），不分部分、不展示「（会议）」/「（日程）」标题，按普通列表直接展示即可。`search`/`list`/`get` 返回均含 `meeting` 字段，可直接判断，无需额外调用其它接口补 `get`。

**模糊"会议"查询的汇总 [REQUIRED]**：当本次是"查会议/xx会"等需归类的查询（见 [SKILL.md 查询消歧](../SKILL.md)）时，在列表末尾追加一行汇总：`共 N 场，其中会议 X 场、日程 Y 场`。

**时区标注**：日程 `timezone.timezone_offset != 28800`（非东八区）时，按 [SKILL.md 输出格式规范](../SKILL.md) 的时区标注规则在时间后带上时区，如 `14:00-15:00（纽约时间 UTC-5）`。

### 周期日程标注规则 [REQUIRED]

列表中存在 `repeat_rule.is_repeat=true` 的日程时，必须在该日程的**主题后追加**周期频率标注（不另起新字段，保持每个条目仍只有主题/时间/参与人三项），格式如下：

```
1. 每周站会（每周一次，截止 2026-12-31）
   时间：4月7日（周一）09:00-09:30
   参与人：张三、李四
```

**`repeat_type` 枚举值 → 可读文案映射：**

| `repeat_type` | 含义 | 展示文案示例 |
|:---:|------|------|
| `daily` | 每天 | 每天一次 |
| `weekly` | 每周 | 每周一次 |
| `monthly` | 每月 | 每月一次 |
| `monthly_on_the_nth_day` | 每月第N天 | 每月一次 |
| `yearly` | 每年 | 每年一次 |
| `yearly_on_the_nth_day` | 每年第N天 | 每年一次 |
| `work_day` | 每个工作日 | 每工作日一次 |
| 其他（`is_custom=true`） | 自定义 | 自定义周期 |

**时间范围展示规则：**
- `repeat_until` 非空 → 展示"截止 {repeat_until 的日期部分}"
- `repeat_until` 为空 且 `repeat_time=0` → 展示"无截止"
- `repeat_time > 0` → 展示"共 {repeat_time} 次"

## 典型场景

### 1. 查看今日日程

```
用户：今天有什么安排？
→ 调用 list（today 00:00-23:59）
→ 按开始时间升序，顺序输出每条日程（主题/时间/参与人），超过 10 条只展示前 10 条
```

### 2. 未指定时间范围，使用默认策略

```
用户：帮我看看我的日程安排
→ 未指定时间范围，直接使用默认策略：今天起未来 7 天（无需追问）
   begin_time = 今天 00:00:00，end_time = 7 天后 23:59:59
→ 调用 list，按开始时间升序顺序输出每条日程（主题/时间/参与人）
```

### 3. 查看详情（需要周期规则、会议链接等）

```
用户：这个周会是每周开吗？
→ 先从 list/search 结果中拿到 schedule_id
→ 调用 get 获取详情，展示 repeat_rule
```

## 提示

- 无日程时告知用户"今天日程清空"。
- **查询窗口上限 [REQUIRED]**：`schedules list` 仅覆盖当前时刻前后 30 天以内。用户给的时间范围超出窗口时，直接告知用户超出可查范围、请重新给一个更短的时间范围，等用户重新提供后再调用。
- **顺序列表展示**：每条日程作为独立条目顺序输出，禁止 markdown 表格，每个条目只含主题/时间/参与人。超过 10 条只展示前 10 条，并告知"还有 N 条，需要查看更多吗？"。
- `list` 和 `get` 均返回 `repeat_rule`，可直接判断是否周期日程；`meeting`（含 `meeting_id`/`meeting_code`）在 `search`/`list`/`get` 中均直接返回，判断会议形态无需额外补 `get`。
- **周期日程必须说明 [REQUIRED]**：结果中只要存在 `repeat_rule.is_repeat=true` 的日程，必须在该日程**主题后追加**周期频率（由 `repeat_type` 推导）和时间范围（由 `repeat_until`/`repeat_time` 推导）标注，保持条目仍只含主题/时间/参与人三项。禁止仅展示日程条目而不说明其为周期日程。
- **参与人展示**：`list` 返回的 `attendees` 格式为 `[{"userid": "USERID", "name": "englishname(name)"}]`，直接取 `name` 字段展示，禁止展示 userid，无需反查通讯录。

## 参考

- [wecomcli-calendar](../SKILL.md) — 日程技能主文档
- [calendar-search](calendar-search.md) — 按关键词搜索日程
