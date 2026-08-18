# calendar 会议室查询 — buildings list / rooms search

查询办公楼清单（`buildings list`）和会议室可订性（`rooms search`），用于日程/会议创建或更新时选会议室。两者均为**只读查询**，真正的占用在 [calendar-create](calendar-create.md) 创建时传 `meeting_room_id`、或在 update（[日程](calendar-update.md) / [会议](../../wecomcli-meeting/references/meeting-update.md)）改订时传 `meeting_room_id` 完成。

> 本文档是会议室查询的唯一信息源，[wecomcli-meeting 技能](../../wecomcli-meeting/SKILL.md) 创建会议时也引用此处。

## 命令

```bash
# 列出我可访问的办公楼
wecom-cli meeting rooms buildings list --json '{}'

# 查会议室可订性（单时段）
wecom-cli meeting rooms search --json '{
  "begin_time": "<日期> 14:00:00",
  "end_time": "<日期> 15:00:00",
  "room_keyword": "1605",
  "floor_name": "16",
  "min_capacity": 4
}'
```

---

## buildings list — 办公楼清单

返回用户可访问的办公楼全量列表，无入参（传 `{}`）。

### 返回结构

```json
{
  "total_count": 3,
  "buildings": [
    { "name": "创新大厦A座", "city": "北京", "is_current": true },
    { "name": "创新大厦B座", "city": "北京", "is_current": false },
    { "name": "滨海科技园",  "city": "上海", "is_current": false }
  ]
}
```

| 字段 | 说明 |
|------|------|
| `total_count` | `buildings` 数组长度 |
| `buildings[].name` | 建筑本名，不含城市前缀 |
| `buildings[].city` | 城市，展示时拼 `${city} ${name}` |
| `buildings[].is_current` | 当前所在楼标记；无法判断时全为 `false` |

> 无内部 building_id；下游 `rooms search` 引用某栋楼时传 `building_city` + `building_name`。

### 用法

- **仅当用户提到楼名时调用**；没提楼则不调用，让 `rooms search` 用当前所在楼兜底。
- 把用户口语楼名（如"北京创新A"）匹配到列表条目，得到 `city` + `name`。
- 多候选 → 用文字让用户选（展示用 `${city} ${name}`）；无匹配 → 告知不在可访问列表并列出可选项。
- `buildings: []` → 提示"暂无可预订办公地点"。

> **楼栋识别靠模糊匹配 + 确认，不要苛求字面一致，也不要罗列充数：**
> - 用户说的楼名往往与 `buildings list` 的标准名**写法不同**（使用简称、漏字、少写 A/B 座、带或不带城市前缀等）。应把用户表述与返回列表做**模糊匹配**，而不是要求逐字相同。
> - 命中**唯一最接近**的条目 → 用文字确认一句"你是指【${city} ${name}】吗？"，确认后用该条目的 `city`+`name` 调 `rooms search`。
> - 命中**多个相近**条目 → 用文字只列这几个（展示用 `${city} ${name}`）让用户选。
> - **确实匹配不到**（用户没给楼线索，或列表里没有相近项）→ 才让用户补充 / 自由输入楼名；**禁止从全量列表里随机挑几个充数，也禁止凭记忆编造列表里没有的楼名**。
> - 展示给用户的楼名、以及最终喂给 `rooms search` 的 `building_name` / `building_city`，都必须**逐字取自 `buildings list` 的返回条目**。

---

## rooms search — 会议室可订性查询

给定单时段 + 可选会议室提示 + 容量需求，返回目标会议室能否预订及同楼候选。

### 参数

| 参数 | 类型 | 必填 | 默认 | 说明 |
|------|------|:----:|------|------|
| `begin_time` | string | 是 | — | `YYYY-MM-DD HH:mm:ss`，必须晚于当前时刻 |
| `end_time` | string | 是 | — | 晚于 `begin_time`，间隔 ≤ 24h |
| `building_city` | string | 否 | 当前所在楼城市 | 与 `building_name` 同传或同省略 |
| `building_name` | string | 否 | 当前所在楼楼名 | 同上 |
| `room_keyword` | string | 否 | — | 会议室名/号关键词（如 `"1605"`、`"创新室"`） |
| `floor_name` | string | 否 | — | 楼层过滤，按楼层名匹配（如 `"16"`、`"3 楼"`），仅返回该楼层的会议室；用户明确指定楼层时传入，**直接使用用户的原始表述传入，不做归一化/转换**（用户说"16 楼"就传 `"16 楼"`，说"16F"就传 `"16F"`） |
| `min_capacity` | int | 否 | `2` | 容量下限，传 `len(attendees) + 1`（含组织者） |
| `expand_to_other_buildings` | bool | 否 | `false` | `true` 时同城跨楼推荐，仅用户明确要求才传 |
| `limit` | int | 否 | `20` | `recommendations` 上限（最大 100） |

> `building_city/name` 均不传时用当前所在楼兜底；兜底失败返回 `current_building_unknown`。

### 返回结构

传了 `room_keyword` 时 `target` 为命中的目标会议室列表（数组，每项含 `status`：`bookable` / `unavailable` / `not_found`；同一关键词或叠加 `floor_name` 楼层过滤可能命中多间），未传 `room_keyword` 时 `target` 为空数组 `[]`。`recommendations` 为同楼候选。

```json
{
  "inferred_building": { "name": "创新大厦A座", "city": "北京", "source": "user_current" },
  "target": [
    {
      "status": "unavailable",
      "room": { "meeting_room_id": "mrmaaa", "name": "1605", "capacity": 6, "floor": "16F" }
    }
  ],
  "recommendations": [
    { "meeting_room_id": "mrmbbb", "name": "1607", "capacity": 6, "floor": "16F" },
    { "meeting_room_id": "mrmccc", "name": "1608", "capacity": 8, "floor": "16F" }
  ]
}
```

| 字段 | 说明 |
|------|------|
| `inferred_building.name/city` | 实际查询的办公楼，可展示给用户确认 |
| `inferred_building.source` | `user_current`（兜底）或 `from_input`（来自入参） |
| `target` | 目标会议室列表（数组）；传 `room_keyword` 时为命中项（可能多间），未传为空数组 `[]` |
| `target[].status` | `bookable` / `unavailable` / `not_found` |
| `target[].room` | `not_found` 时为 `null`，否则为房间元数据 |
| `recommendations[]` | 同楼候选，已按"同楼层优先 → 容量恰好够用"排序 |
| `recommendations[].meeting_room_id` | 会议室 ID，仅工具链使用，禁止出现在用户回复正文 |

### 边界

- `target[].status = unavailable` 时不返回占用方信息。
- 同楼无可用时 `recommendations: []`，由 Agent 决定是否开 `expand_to_other_buildings`。
- `meeting_room_id` 仅在工具调用间流转，对用户只展示会议室 name。

### 错误码

| code | 触发场景 | 处理 |
|------|---------|------|
| `current_building_unknown` | 未传楼且无法兜底 | 调 `buildings list` 让用户选楼后重试 |
| `building_not_found` | 入参楼名查无匹配 | 提示该楼无权限，列出可选项 |
| `time_in_past` | `begin_time` ≤ 当前时刻 | 提示用户改未来时间 |

---

## Agent 侧编排

```
├─ 用户提了楼名 → buildings list → 匹配 → building_city + building_name
│  用户没提楼   → 跳过（rooms search 用当前所在楼兜底）
│
└─ rooms search（begin/end + 可选楼 + 可选 room_keyword + min_capacity = len(attendees)+1）
    ├─ 用户指定了具体会议室（传了 room_keyword）→ target 为命中列表：
    │    ├─ target 中存在 status = bookable 的会议室：
    │    │     ├─ 仅 1 个 → 唯一确定，拿其 target[].room.meeting_room_id 进 create
    │    │     └─ 多个     → 用文字让用户选（禁止自动取第一个）
    │    ├─ target = []（查无此名 / 无命中）→ 先告知"未查到你指定的『xxx』会议室"，禁止静默替换；
    │    │     再用文字让用户决定改订其他会议室或换时间（候选仅 1 个也须用户确认）；recommendations 为空则告知后问换时间/跨楼
    │    └─ target 中无 bookable、命中项均为 unavailable（被占）→ 先告知"『xxx』该时段已被占用"，
    │          再用文字让用户选替代会议室或换时间（同样禁止静默替换）
    ├─ 用户未指定具体会议室（target = []）：
    │    ├─ recommendations 多个候选 → 必须用文字让用户选（禁止自动取第一个）
    │    └─ recommendations 仅 1 个    → 可直接使用该候选 meeting_room_id
    └─ recommendations = []        → 问是否跨楼（expand_to_other_buildings=true 重试）或换时间
```

> [!CAUTION]
> **五条硬性规则（下游 create 必须遵守）：**
> 1. **先查询、后推荐、后创建**：`meeting_room_id` 必须来自 `rooms search` 的真实返回值，禁止跳过查询直接创建，禁止凭记忆 / 猜测编造。任何向用户展示的候选 / 推荐会议室（含用文字给出的候选、回复正文里提到的会议室名 / 房间号 / 楼层 / 容量）也必须来自本次 `rooms search` 返回的 `target` / `recommendations`——在成功调用 `rooms search` 拿到真实结果之前，禁止凭记忆、上下文、历史会话或想象罗列、推荐、列举任何具体会议室让用户选择。需要让用户选会议室时，先调 `rooms search`，再用其返回的候选组装文字询问。
> 2. **存在多个会议室必须让用户选**：`recommendations` 命中多个候选时，必须用文字让用户选择或指定具体会议室，禁止自动替用户挑选。
> 3. **会议室禁止只写进 `location`**：只要用户提到会议室，就必须经 `rooms search` 查到真实会议室并以 `meeting_room_id` 传入创建。严禁把会议室名 / 房间号仅写进 `location` 字段——那样不会真正占用（预订）会议室。
> 4. **优先先订房、后建程/建会**：用户在创建时就提到会议室的，应先敲定 `meeting_room_id`（含用户确认）再调用 create，会议室查询/选择是 create 的前置阻塞项，避免创建后会议室被抢占。若创建时漏订或事后要换会议室，可通过 `update` 传入新 `meeting_room_id` 改订（须先经 `rooms search` 确认新会议室 `status=bookable`，详见各自的 update 参考），不必取消重建。
> 5. **指定会议室查无/不可用时必须先告知、禁止静默替换**：用户指定的会议室在 `target` 中找不到可订项（`target = []` 查无此名，或命中项均为 `unavailable` 被占）时，必须先告知用户"未查到 / 无法预订你指定的『xxx』会议室"，再用文字让用户决定改订其他会议室或换时间。严禁静默用其他名称的会议室替代——即使 `recommendations` 仅 1 个候选也须经用户确认。"`recommendations` 仅 1 个可直接使用"只适用于用户未指定具体会议室（`target = []`）的情形。

- `rooms search` 需要**确定的起止时间**。用户只给了时间范围（如"明天下午"）时，先用 [calendar-freebusy](calendar-freebusy.md) 查共同空闲、让用户选定一个具体时段，再拿该时段调 `rooms search`；用户已给精确时间（如"明天 3 点"）则直接查。
- 用文字给出的候选必须 2~4 个，展示会议室 `name` + 楼层 + 容量；`meeting_room_id` 仅工具链使用，禁止出现在用户回复正文。
- `meeting_room_taken`（抢订竞态）发生在 create 阶段，处理见 [calendar-create](calendar-create.md)。

## 参考

- [calendar-create](calendar-create.md) — 日程创建（传 `meeting_room_id` 占用会议室）
- [calendar-freebusy](calendar-freebusy.md) — 共同空闲查询
- [wecomcli-calendar.md](../SKILL.md) — 日程技能主文档
