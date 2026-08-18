# tmeet meeting — 会议管理

> **前置条件：** 先执行 `tmeet auth login` 完成登录授权。

时间参数格式：`2026-03-12T14:00:00+08:00` 或 `2026-03-12T14:00+08:00`（必须包含时区）。

## 目录

- [查询命令选择准则](#查询命令选择准则)
- [create — 创建会议](#create--创建会议)
- [update — 更新会议](#update--更新会议)
- [cancel — 取消会议](#cancel--取消会议)
- [get — 获取会议详情](#get--获取会议详情)
- [list — 获取待开始/进行中的会议列表](#list--获取待开始进行中的会议列表)
- [list-ended — 获取已结束会议列表](#list-ended--获取已结束会议列表)
- [search — 搜索会议](#search--搜索会议)
- [invitees-list — 获取会议受邀者](#invitees-list--获取会议受邀者)
- [invitees-add — 添加受邀成员](#invitees-add--添加受邀成员)
- [invitees-remove — 移除受邀成员](#invitees-remove--移除受邀成员)
- [invitees-replace — 替换受邀成员列表](#invitees-replace--替换受邀成员列表)
- [常见错误](#常见错误)
- [参考](#参考)

---

## 查询命令选择准则

> 📌 **仅时间范围** → `meeting list`（待开始/进行中）/ `meeting list-ended`（已结束）； **已知会议号 / 会议 ID** → `meeting get`； **含关键词**（主题 / 创建人 / 备注）→ `meeting search`（可与时间范围组合）。
>
> 完整规则（含“不得在 list 上硬塞关键词”、“不得把关键词当时间使用”、“歧义时先澄清”等）见 [SKILL.md 「查询命令选择准则」](../SKILL.md#查询命令选择准则list-vs-search)。

---

## create — 创建会议

```bash
# 创建普通会议（必填：主题、开始时间、结束时间）
tmeet meeting create \
  --subject "项目周会" \
  --start "2026-04-10T14:00:00+08:00" \
  --end "2026-04-10T15:00:00+08:00"

# 创建带密码的会议
tmeet meeting create \
  --subject "季度复盘" \
  --start "2026-04-10T14:00:00+08:00" \
  --end "2026-04-10T16:00:00+08:00" \
  --password "123456"

# 创建仅受邀成员可入会的会议，并开启等候室
tmeet meeting create \
  --subject "保密会议" \
  --start "2026-04-10T10:00:00+08:00" \
  --end "2026-04-10T11:00:00+08:00" \
  --join-type 2 \
  --waiting-room

# 创建每周周期性会议（按次数结束，共 10 次）
tmeet meeting create \
  --subject "每周站会" \
  --start "2026-04-10T09:30:00+08:00" \
  --end "2026-04-10T10:00:00+08:00" \
  --meeting-type 1 \
  --recurring-type 2 \
  --until-type 1 \
  --until-count 10

# 创建每天周期性会议（按日期结束）
tmeet meeting create \
  --subject "每日站会" \
  --start "2026-04-10T09:00:00+08:00" \
  --end "2026-04-10T09:30:00+08:00" \
  --meeting-type 1 \
  --recurring-type 0 \
  --until-type 0 \
  --until-date "2026-05-10T00:00:00+08:00"

# 创建会议并邀请成员（最多 100 人，openid 列表）
tmeet meeting create \
  --subject "需求评审" \
  --start "2026-04-10T14:00:00+08:00" \
  --end "2026-04-10T15:00:00+08:00" \
  --invitees "open_id1,open_id2,open_id3"

# 显式关闭音频水印 / 自动文字转写（注意必须使用 = 形式传 false）
tmeet meeting create \
  --subject "无水印会议" \
  --start "2026-04-10T14:00:00+08:00" \
  --end "2026-04-10T15:00:00+08:00" \
  --audio-watermark=false \
  --auto-asr=false
```

### 参数

| 参数 | 必填 | 默认值 | 说明                                               |
|------|------|--------|--------------------------------------------------|
| `--subject <text>` | ✅ | — | 会议主题                                             |
| `--start <time>` | ✅ | — | 开始时间（ISO 8601，含时区）                               |
| `--end <time>` | ✅ | — | 结束时间（ISO 8601，含时区）                               |
| `--password <pwd>` | 否 | — | 会议密码（4~6 位数字）                                    |
| `--timezone <tz>` | 否 | — | 时区，如 `Asia/Shanghai`                             |
| `--meeting-type <n>` | 否 | `0` | 会议类型：`0`-普通，`1`-周期性                              |
| `--join-type <n>` | 否 | `0` | 入会限制：`1`-所有成员，`2`-仅受邀，`3`-仅企业内部                  |
| `--waiting-room` | 否 | `false` | 开启等候室                                            |
| `--recurring-type <n>` | 周期性时使用 | `0` | 重复类型：`0`-每天，`1`-每周一至五，`2`-每周，`3`-每两周，`4`-每月      |
| `--until-type <n>` | 周期性时使用 | `0` | 结束类型：`0`-按日期，`1`-按次数                             |
| `--until-count <n>` | 周期性时使用 | `7` | 重复次数（每天/每个工作日/每周最大 500，每两周/每月最大 50）              |
| `--until-date <date>` | 周期性按日期结束时使用 | — | 结束日期（ISO 8601，含时区，如 `2026-05-10T00:00:00+08:00`） |
| `--invitees <ids>` | 否 | — | 邀请成员的 openid 列表，逗号分隔或重复传参，最多 100 人              |
| `--water-mark-type <n>` | 否 | `2` | 文字水印：`0`-单排，`1`-双排，`2`-关闭<br>● 个人账号：默认为2<br>● 企业/组织账号：<br>  ✧ 企业设置强制态-使用企业设置作为强制态，入参不生效<br>  ✧ 企业未设置强制态-使用企业设置作为默认值，入参覆盖默认值 |
| `--audio-watermark` | 否 | `false` | 音频水印： ✧ 企业设置强制态-使用企业设置作为强制态，入参不生效<br>  ✧ 企业未设置强制态-使用企业设置作为默认值，入参覆盖默认值<br>显式关闭需使用 `--audio-watermark=false` |
| `--auto-record-type <type>` | 否 | `none` | 主持人入会后自动录制会议：`none`-关，`local`-本地，`cloud`-云录制<br>● 个人账号：默认none<br>● 企业/组织账号：<br>  ✧ 企业设置强制态-使用企业设置作为强制态，入参不生效<br>  ✧ 企业未设置强制态-使用企业设置作为默认值，入参覆盖默认值 |
| `--auto-asr` | 否 | `false` | 自动文字转写： ✧ 企业设置强制态-使用企业设置作为强制态，入参不生效<br>  ✧ 企业未设置强制态-使用企业设置作为默认值，入参覆盖默认值<br>显式关闭需使用 `--auto-asr=false` |

---

## update — 更新会议

> ⚠️ **写操作，执行前请确认用户意图。** 会议信息（时间、主题、入会限制、邀请列表、周期规则等）的变更会影响所有参会人，必须先向用户明确列出**将变更的字段及前后值**并获得确认后再执行。

> ⚠️ **周期性会议注意**：修改周期性会议时，如果没有修改会议类型，**必须传 `--meeting-type 1`**，否则系统会将其修改为普通会议，导致周期规则丢失。

> ⚠️ **邀请变更**：`--invitees` 与 `--invitees-type` 必须同时使用。`--invitees` 单次最多 100 人。

```bash
# 修改会议主题
tmeet meeting update --meeting-id "100000000" --subject "新主题"

# 修改时间
tmeet meeting update \
  --meeting-id "100000000" \
  --start "2026-04-10T15:00:00+08:00" \
  --end "2026-04-10T16:00:00+08:00"

# 修改入会限制并开启等候室
tmeet meeting update \
  --meeting-id "100000000" \
  --join-type 3 \
  --waiting-room

# 修改周期性会议（必须传 --meeting-type 1，否则会被当作普通会议处理）
tmeet meeting update \
  --meeting-id "100000000" \
  --meeting-type 1 \
  --subject "每周站会（新主题）" \
  --recurring-type 2 \
  --until-type 1 \
  --until-count 20

# 只修改周期性会议中的某一场子会议时间（不修改周期规则）
tmeet meeting update \
  --meeting-id "100000000" \
  --meeting-type 1 \
  --sub-meeting-id "200000001" \
  --start "2026-04-17T10:00:00+08:00" \
  --end "2026-04-17T11:00:00+08:00"

# 在原邀请列表上追加成员
tmeet meeting update \
  --meeting-id "100000000" \
  --invitees "open_id4,open_id5" \
  --invitees-type add

# 从原邀请列表移除指定成员
tmeet meeting update \
  --meeting-id "100000000" \
  --invitees "open_id1" \
  --invitees-type remove

# 整体覆盖邀请列表
tmeet meeting update \
  --meeting-id "100000000" \
  --invitees "open_id1,open_id2,open_id3" \
  --invitees-type replace

# 显式关闭音频水印 / 自动文字转写（注意必须使用 = 形式传 false）
tmeet meeting update \
  --meeting-id "100000000" \
  --audio-watermark=false \
  --auto-asr=false
```

### 参数

| 参数 | 必填 | 默认值 | 说明                                               |
|------|------|--------|--------------------------------------------------|
| `--meeting-id <id>` | ✅ | — | 会议 ID                                            |
| `--subject <text>` | 否 | — | 新会议主题                                            |
| `--start <time>` | 否 | — | 新开始时间（ISO 8601，含时区）                              |
| `--end <time>` | 否 | — | 新结束时间（ISO 8601，含时区）                              |
| `--password <pwd>` | 否 | — | 新会议密码（4~6 位数字）                                   |
| `--timezone <tz>` | 否 | — | 新时区                                              |
| `--meeting-type <n>` | **周期性会议时必填** | `0` | 会议类型：`0`-普通，`1`-周期性                              |
| `--join-type <n>` | 否 | `0` | 入会限制：`1`-所有成员，`2`-仅受邀，`3`-仅企业内部                  |
| `--waiting-room` | 否 | `false` | 开启等候室                                            |
| `--recurring-type <n>` | 周期性时使用 | `0` | 重复类型：`0`-每天，`1`-每周一至五，`2`-每周，`3`-每两周，`4`-每月      |
| `--until-type <n>` | 周期性时使用 | `0` | 结束类型：`0`-按日期，`1`-按次数                             |
| `--until-count <n>` | 周期性时使用 | `7` | 重复次数（每天/每个工作日/每周最大 500，每两周/每月最大 50）              |
| `--until-date <date>` | 周期性按日期结束时使用 | — | 结束日期（ISO 8601，含时区，如 `2026-05-10T00:00:00+08:00`） |
| `--sub-meeting-id <id>` | 修改单场子会议时使用 | — | 子会议 ID：仅修改该场子会议的时间；**不可与 `--recurring-type` / `--until-type` / `--until-count` / `--until-date` 同时使用**。不填则修改整个周期性会议 |
| `--invitees <ids>` | 与 `--invitees-type` 同时使用 | — | 待变更的邀请成员 openid 列表，逗号分隔或重复传参                |
| `--invitees-type <s>` | 同上 | — | 邀请变更策略：`add` / `remove` / `replace`                |
| `--water-mark-type <n>` | 否 | `2` | 文字水印：`0`-单排，`1`-双排，`2`-关闭<br>● 个人账号：默认为2<br>● 企业/组织账号：<br>  ✧ 企业设置强制态-使用企业设置作为强制态，入参不生效<br>  ✧ 企业未设置强制态-使用企业设置作为默认值，入参覆盖默认值 |
| `--audio-watermark` | 否 | `false` | 音频水印： ✧ 企业设置强制态-使用企业设置作为强制态，入参不生效<br>  ✧ 企业未设置强制态-使用企业设置作为默认值，入参覆盖默认值<br>显式关闭需使用 `--audio-watermark=false` |
| `--auto-record-type <type>` | 否 | `none` | 主持人入会后自动录制会议：`none`-关，`local`-本地，`cloud`-云录制<br>● 个人账号：默认none<br>● 企业/组织账号：<br>  ✧ 企业设置强制态-使用企业设置作为强制态，入参不生效<br>  ✧ 企业未设置强制态-使用企业设置作为默认值，入参覆盖默认值 |
| `--auto-asr` | 否 | `false` | 自动文字转写： ✧ 企业设置强制态-使用企业设置作为强制态，入参不生效<br>  ✧ 企业未设置强制态-使用企业设置作为默认值，入参覆盖默认值<br>显式关闭需使用 `--auto-asr=false` |

---

## cancel — 取消会议

> ⚠️ **写操作，执行前请确认用户意图。**

```bash
# 取消普通会议
tmeet meeting cancel --meeting-id "100000000"

# 取消周期性会议的某个子会议
tmeet meeting cancel \
  --meeting-id "100000000" \
  --sub-meeting-id "200000001"

# 取消整场周期性会议
tmeet meeting cancel \
  --meeting-id "100000000" \
  --meeting-type 1
```

### 参数

| 参数 | 必填 | 默认值 | 说明                                   |
|------|------|--------|--------------------------------------|
| `--meeting-id <id>` | ✅ | — | 会议 ID                                |
| `--sub-meeting-id <id>` | 否 | — | 子会议 ID（取消周期性会议的某场时使用）                |
| `--meeting-type <n>` | 否 | `0` | `0`-普通会议，`1`-周期性会议；取消整场周期性会议时必须传 `1` |

---

## get — 获取会议详情

```bash
# 通过会议 ID 查询（优先级更高）
tmeet meeting get --meeting-id "100000000"

# 通过会议码查询
tmeet meeting get --meeting-code "123456789"
```

### 参数

| 参数 | 必填 | 说明 |
|------|------|------|
| `--meeting-id <id>` | 二选一 | 会议 ID（优先级高于会议码） |
| `--meeting-code <code>` | 二选一 | 会议码 |

> `--meeting-id` 和 `--meeting-code` 必须提供其中一个。

### 响应关键字段

响应 `data.meeting_info_list[]` 中每条会议含录制相关字段：

| 字段路径 | 说明 |
|---------|------|
| `records[]` | 录制列表（⚠️ 仅返回最近若干条，见下方分页限制） |
| `records[].permission_status` | 权限状态（取值见下方决策规则） |
| `records[].state` | 录制状态（如 `转码完成，可根据录制文件权限进行下一步`；录制/转码过程中会有相应中间状态） |
| `records[].meeting_record_id` | 会议录制 ID |
| `records[].record_file_id` | 录制文件 ID |
| `records[].subject` | 录制标题 |
| `records[].duration` | 录制时长 |
| `records[].url` | 录制播放地址 |
| `records[].type` | 录制类型（如 `云录制`） |
| `records[].media_start_time` | 录制开始时间 |
| `records[].record_file_count` | 录制文件数量 |
| `records_total_count` | 录制总数（可能远大于 `records[]` 实际返回条数） |
| `sub_meetings[]` | 子会议列表（仅周期性会议返回） |
| `sub_meetings[].sub_meeting_id` | 子会议 ID |
| `sub_meetings[].start_time` / `end_time` | 子会议起止时间 |
| `sub_meetings[].status` | 子会议状态 |
| `current_sub_meeting_id` | 当前/下一场子会议 ID（周期性会议） |
| `has_more_sub_meeting` | 是否还有更多子会议（0=已全部返回） |

> **注意**：`sub_meetings[]` 仅含 `sub_meeting_id` / `start_time` / `end_time` / `status` 四个基本字段，**不含 `records[]`**。周期性会议的录制统一在顶层 `records[]` 中返回；`records[]` 条目**不含 `sub_meeting_id`**，需按 `media_start_time` 与子会议的起止时间对齐归属到具体子会议。

> **⚠️ records 分页限制**：`meeting get` **不支持分页参数**（无 `--page-token`/`--page-size`）。当 `records_total_count` 大于实际返回条数时（`records[]` 仅返回最近若干条），**无法通过 `meeting get` 翻页获取全部录制**。如需完整录制列表，按时间范围 `meeting list-ended` 分批查询（含 `permission_status`）；`record list` 虽支持分页，但只返回已有权限的录制、无 `permission_status`，仅适用于查自己的录制。

> **`permission_status` 决策规则**：
> - `can_view`：直接展示录制信息（标题、时长、播放地址），可继续获取智能纪要/转写
> - `can_apply`：需申请权限，询问是否发起申请（走 `permission-apply-prepare` → 用户确认 → `permission-apply-commit` 流程）
> - `closed`：告知用户录制已被关闭，无法查看
> - `deleted`：告知用户录制已被删除，无法查看
> - `password_required`：告知用户需要密码，请用户提供密码后重试

> **`--compact` 不影响 `permission_status`**：启用 `--compact` 时 `records[]` 仍保留 `permission_status` 及全部字段，可放心使用。

---

## list — 获取待开始/进行中的会议列表

```bash
# 查询所有待开始/进行中的会议列表（不限时间范围）
tmeet meeting list

# 按时间范围查询
tmeet meeting list \
  --start "2026-04-01T00:00:00+08:00" \
  --end "2026-04-30T23:59:59+08:00"

# 展示所有子会议
tmeet meeting list --show-all-sub 1

# 分页查询（翻下一页）
tmeet meeting list --page-token "<next_page_token>" --page-size 20
```

### 参数

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `--start <time>` | 否 | — | 查询起始时间（ISO 8601，含时区） |
| `--end <time>` | 否 | — | 查询结束时间（ISO 8601，含时区） |
| `--show-all-sub <n>` | 否 | `0` | 展示所有子会议：`0`-不展示，`1`-展示 |
| `--page-token <token>` | 否 | — | 分页游标，首页不传；后续翻页传入上一次响应的 `next_page_token` |
| `--page-size <n>` | 否 | `20` | 每页数量，默认 20，最大 20 |

---

## list-ended — 获取已结束会议列表

```bash
# 查询所有已结束会议
tmeet meeting list-ended

# 按时间范围查询已结束会议
tmeet meeting list-ended \
  --start "2026-04-01T00:00:00+08:00" \
  --end "2026-04-30T23:59:59+08:00"

# 分页查询（使用 page-token）
tmeet meeting list-ended \
  --start "2026-04-01T00:00:00+08:00" \
  --end "2026-04-30T23:59:59+08:00" \
  --page-token "<next_page_token>" \
  --page-size 30
```

### 参数

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `--start <time>` | 否 | — | 查询起始时间（ISO 8601，含时区）。**与 `--end` 的区间不得超过 90 天**，超出返回 `190004` |
| `--end <time>` | 否 | — | 查询结束时间（ISO 8601，含时区）。**与 `--start` 的区间不得超过 90 天**，超出返回 `190004` |
| `--page-token <token>` | 否 | — | 分页游标，首页不传；后续翻页传入上一次响应的 `next_page_token` |
| `--page-size <n>` | 否 | `30` | 每页数量，默认 30，最大 30 |
| `--page <n>` | 否 | — | ⚠️ **已弃用**：页码（从 1 开始），请改用 `--page-token` |

> **⚠️ 90 天区间上限**：`--start` 与 `--end` 的跨度**超过 90 天时返回 `190004`**，需查更长时间范围时，**主动拆成多个 ≤90 天的区间顺序查询**，不要先撞错误再拆。拆段时相邻区间的端点应连续（前段 `--end` = 后段 `--start`），避免漏查。

### 响应关键字段

`meeting list-ended` 的响应结构与 `meeting get` 一致，`meeting_info_list[].records[]` 同样含 `permission_status` / `state` / `record_file_id` / `url` 等字段。**批量查询录制时无需逐场调 `meeting get`，一步获取。**

> **与 `record list` 的区别**：`record list` 返回 `record_meetings[]`，**不含顶层 `permission_status` / `url`**（`record_file_id` 不在顶层，仅在部分条目的 `record_files[]` 内）；`list-ended` 返回 `meeting_info_list[].records[]`，每条**含** `permission_status` / `record_file_id` / `url`。需权限状态时用 `list-ended`，找用户已有权限的录制用 `record list`。

> **分页**：当 `has_more` 为 `true` 或 `next_page_token` 非空时，使用 `--page-token` 翻页获取完整列表。首次查询不传 `--page-token`，翻页时传入上一次响应的 `next_page_token`。`meeting list-ended` 支持 `--page-token` / `--page-size` 分页参数（注意：`meeting get` **不支持**分页参数）。

---

## search — 搜索会议

按关键词、会议号、时间范围等条件搜索会议。所有过滤参数均为可选，可任意组合。**当用户带关键词（会议主题/创建人/备注）查会议时用本命令，而非在 `list` 上硬塞关键词。**

```bash
# 按主题关键词搜索
tmeet meeting search --query "周例会" --query-field subject

# 按创建者昵称搜索
tmeet meeting search --query "张三" --query-field creator

# 按会议号精确搜索
tmeet meeting search --meeting-code "931945029"

# 按时间范围搜索
tmeet meeting search \
  --start "2026-04-01T00:00+08:00" \
  --end "2026-04-30T23:59+08:00"

# 关键词 + 时间范围组合
tmeet meeting search \
  --query "项目评审" --query-field subject \
  --start "2026-04-01T00:00+08:00" \
  --end "2026-04-30T23:59+08:00"

# 翻下一页
tmeet meeting search \
  --query "项目评审" \
  --page-token "<next_page_token>" --page-size 30
```

### 参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `--query <text>` | string | 否 | — | 搜索关键词 |
| `--query-field <f>` | string | 否 | `all` | `--query` 的搜索字段：`subject`-会议主题；`creator`-创建者昵称/备注名；`note`-用户对会议的备注；`all`-搜索所有字段 |
| `--meeting-code <code>` | string | 否 | — | 按会议号过滤（精确匹配，仅数字，无短横线） |
| `--start <time>` | string | 否 | — | 搜索时间窗下限（ISO 8601）。匹配条件：会议预约开始时间、实际开始时间或当前用户加入时间任一落在窗口内 |
| `--end <time>` | string | 否 | — | 搜索时间窗上限（ISO 8601），语义同上 |
| `--page-token <token>` | string | 否 | — | 分页游标，首页不传；翻页时传入上一次响应的 `next_page_token` |
| `--page-size <n>` | int | 否 | `30` | 每页大小，默认 30，最大 30 |

> **`meeting search` vs `record search` 的分流**：按**会议级线索**（会议主题 / 创建人 / 会议号）找会议或其录制 → `meeting search` / `meeting get`；按**录制内容**检索（转写原文 / 纪要关键词）→ `record search`（见 [`tmeet-record.md`](tmeet-record.md)）。

---

## invitees-list — 获取会议受邀者

> ⚠️ **适用场景**：获取会议已邀请用户名单。本命令仅会议创建者可调用，若收到相关提示，无需重试直接告知用户。

> ⚠️ **成员回显格式（强约束）**：向用户展示受邀成员列表时，每一名成员**必须**严格遵循 [SKILL.md 「响应处理规则」](../SKILL.md#响应处理规则)中「成员回显格式」，即按 `姓名（<标识>）` 的格式回显（标识从 `部门` / `职位` / `open_id` 中任选一项，优先级 `部门` > `职位` > `open_id`）；**严禁**用 `open_id` / `ms_open_id` / `userid` / 邮箱前缀 / 花名 替代姓名。

```bash
# 获取会议受邀者列表
tmeet meeting invitees-list --meeting-id "100000000"

# 分页获取（翻下一页）
tmeet meeting invitees-list \
  --meeting-id "100000000" \
  --page-token "<next_page_token>" \
  --page-size 30
```

### 参数

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `--meeting-id <id>` | ✅ | — | 会议 ID |
| `--page-token <token>` | 否 | — | 分页游标，首页不传；后续翻页传入上一次响应的 `next_page_token` |
| `--page-size <n>` | 否 | `30` | 每页数量，默认 30，最大 30 |
| `--pos <n>` | 否 | — | ⚠️ **已弃用**：分页起始位置，请改用 `--page-token` |

---

## invitees-add — 添加受邀成员

> ⚠️ **高风险写操作（被邀请者会收到会议通知）：执行前必须按 [SKILL.md 「响应处理规则」](../SKILL.md#响应处理规则)中「受邀人管理类写操作的二次确认模板」向用户展示并获得明确确认。**

> ⚠️ **适用场景**：本命令仅会议创建者在会议开始前可调用，若收到相关提示，无需重试直接告知用户。

> ⚠️ **成功后按 [SKILL.md 「响应处理规则」](../SKILL.md#响应处理规则)中「会议成员变更操作的回复模板」输出结果**。

向已存在的会议中追加受邀成员。受邀成员通过用户 `open_id` 指定，可通过 `contact search` 命令查询获得。

```bash
# 通过英文逗号分隔传入多个 open_id
tmeet meeting invitees-add \
  --meeting-id "100000000" \
  --invitees "open_id1,open_id2"

# 重复传入 --invitees 参数
tmeet meeting invitees-add \
  --meeting-id "100000000" \
  --invitees "open_id1" \
  --invitees "open_id2"
```

### 参数

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `--meeting-id <id>` | ✅ | — | 会议 ID |
| `--invitees <list>` | ✅ | — | 待添加的受邀成员 `open_id` 列表，支持英文逗号分隔或重复传入该参数，最多 100 个 |

---

## invitees-remove — 移除受邀成员

> ⚠️ **高风险写操作：执行前必须按 [SKILL.md 「响应处理规则」](../SKILL.md#响应处理规则)中「受邀人管理类写操作的二次确认模板」向用户展示并获得明确确认。**

> ⚠️ **适用场景**：本命令仅会议创建者在会议开始前可调用，若收到相关提示，无需重试直接告知用户。

> ⚠️ **成功后按 [SKILL.md 「响应处理规则」](../SKILL.md#响应处理规则)中「会议成员变更操作的回复模板」输出结果**。

从已存在的会议中移除指定的受邀成员。

```bash
tmeet meeting invitees-remove \
  --meeting-id "100000000" \
  --invitees "open_id1,open_id2"
```

### 参数

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `--meeting-id <id>` | ✅ | — | 会议 ID |
| `--invitees <list>` | ✅ | — | 待移除的受邀成员 `open_id` 列表，支持英文逗号分隔或重复传入该参数，最多 100 个 |

---

## invitees-replace — 替换受邀成员列表

> ⚠️ **高风险写操作（以传入的列表整体覆盖当前受邀成员列表，未在 `--invitees` 中的成员会被移除）：执行前必须按 [SKILL.md 「响应处理规则」](../SKILL.md#响应处理规则)中「受邀人管理类写操作的二次确认模板」向用户展示最终完整列表并获得明确确认。**

> ⚠️ **适用场景**：本命令仅会议创建者在会议开始前可调用，若收到相关提示，无需重试直接告知用户。

> ⚠️ **成功后按 [SKILL.md 「响应处理规则」](../SKILL.md#响应处理规则)中「会议成员变更操作的回复模板」输出结果**。

```bash
tmeet meeting invitees-replace \
  --meeting-id "100000000" \
  --invitees "open_id1,open_id2,open_id3"
```

### 参数

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `--meeting-id <id>` | ✅ | — | 会议 ID |
| `--invitees <list>` | ✅ | — | 替换后的受邀成员 `open_id` 完整列表，支持英文逗号分隔或重复传入该参数，最多 100 个 |

---

## 常见错误

| 错误现象 | 原因 | 解决方案 |
|---------|------|---------|
| `--subject is required` | 缺少必填参数 | 补充 `--subject` |
| `--start format error` | 时间格式不合法（如缺少时区） | 改用 `2026-03-12T14:00:00+08:00` 格式 |
| `--meeting-id is required` | 缺少必填参数 | 补充 `--meeting-id` |
| 执行`invitees-list / invitees-add / invitees-remove / invitees-replace`命令报`1000009042 无权限操作` | 操作者非会议创建者 | 仅会议创建者可调用 |
| 执行`invitees-list / invitees-add / invitees-remove / invitees-replace`命令报`1000190004 meeting_id必须是整型` | meeting-id参数非法 | 参数非法，请检查meeting-id是否正确 |
| 执行`invitees-list / invitees-add / invitees-remove / invitees-replace`命令报`1000190457 会议不存在, 请核对meetingId` | meeting-id没有对应会议 | 会议不存在，请检查meeting-id是否正确 |
| 执行`invitees-add / invitees-replace`报会议室已满 | 会议人数已达上限 | 请前往腾讯会议客户端/APP进行扩容 |
| 执行`invitees-remove`报人员不在"已邀请名单中" | 操作对象不在已邀请名单中 | 无需继续操作 |
| 执行`invitees-add / invitees-remove / invitees-replace`命令报open_id非法 | 参数非法 | 请检查openid是否正确 |
| `500273 会议已开始，无法修改受邀人` | 会议不存在，无法修改受邀人 | 会议不存在，无法修改受邀人 |
| `500274 会议已取消，无法修改受邀人` | 会议已取消，无法修改受邀人 | 会议已取消，无法修改受邀人 |
| `500275 会议不存在，无法修改受邀人` | 会议已取消，无法修改受邀人 | 会议已取消，无法修改受邀人 |

## 参考

- [tmeet](../SKILL.md) — 全部命令概览
- [tmeet-record](tmeet-record.md) — 录制管理
- [tmeet-report](tmeet-report.md) — 会议报告