# tmeet meeting — 会议管理

> **前置条件：** 先执行 `tmeet auth login` 完成登录授权。

时间参数格式：`2026-03-12T14:00:00+08:00` 或 `2026-03-12T14:00+08:00`（必须包含时区）。

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

> ⚠️ **周期性会议注意**：修改周期性会议时，如果没有修改会议类型，**必须传 `--meeting-type 1`**，否则系统会将其修改为普通会议，导致周期规则丢失。

> ⚠️ **邀请变更**：`--invitees` 与 `--invitees-type` 必须同时使用。`--invitees` 单次最多 100 人。

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
| `--start <time>` | 否 | — | 查询起始时间（ISO 8601，含时区） |
| `--end <time>` | 否 | — | 查询结束时间（ISO 8601，含时区） |
| `--page-token <token>` | 否 | — | 分页游标，首页不传；后续翻页传入上一次响应的 `next_page_token` |
| `--page-size <n>` | 否 | `30` | 每页数量，默认 30，最大 30 |
| `--page <n>` | 否 | — | ⚠️ **已弃用**：页码（从 1 开始），请改用 `--page-token` |

---

## invitees-list — 获取会议受邀者

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

> ⚠️ **写操作，执行前请确认用户意图。被邀请者会收到会议通知。**
> ⚠️ **成功后必须按本文档末尾《成员变更后的回复模板》输出结果；「已邀请成员」**仅限展示通讯录姓名**，严禁直接展示 `open_id` / `userid` / `ms_open_id` / 花名 / 邮箱前缀等任何内部标识。

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

> ⚠️ **写操作，执行前请确认用户意图。**
> ⚠️ **成功后必须按本文档末尾《成员变更后的回复模板》输出结果；「已邀请成员」**仅限展示通讯录姓名**，严禁直接展示 `open_id` / `userid` / `ms_open_id` / 花名 / 邮箱前缀等任何内部标识。

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

> ⚠️ **高风险写操作：会以传入的列表整体覆盖当前受邀成员列表，未在 `--invitees` 中的成员会被移除。执行前必须向用户明确列出最终列表并获得确认。**
> ⚠️ **成功后必须按本文档末尾《成员变更后的回复模板》输出结果；「已邀请成员」**仅限展示通讯录姓名**，严禁直接展示 `open_id` / `userid` / `ms_open_id` / 花名 / 邮箱前缀等任何内部标识。

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

## 成员变更后的回复模板（适用于 invitees-add / invitees-remove / invitees-replace）

以上三个命令成功执行后，**必须**按下列模板回复用户，**字段顺序固定、不得增删**：

- **会议主题**：<subject>
- **会议时间**：<start> ~ <end>（含时区）
- **会议号**：<meeting_code>（严禁展示 meeting_id）
- **入会链接**：<join_url>
- **已邀请成员**：<姓名1>、<姓名2>、…

### 「已邀请成员」展示规则（强约束）

1. **必须以通讯录中的姓名展示**（如 `张三`）；**严禁**直接展示 `open_id` / `userid` / `ms_open_id` / 花名（如 `zhangsan`） / 邮箱前缀等任何内部标识。
2. **信息不足时的兑底动作**：若手头没有 `open_id → 姓名` 映射，先调用 `meeting invitees-list --meeting-id <id>` 拉取变更后的完整受邀列表，再依次使用 `contact search`（仅在该场景下允许，遵循 SKILL.md 中的「通讯录搜索仅限特定场景使用」规则）将每个 `open_id` 解析为姓名后再回复。
3. **解析失败的兑底**：某个 `open_id` 无法解析为姓名时，标注为 `未知成员`，**禁止**回退到打印 `open_id` 本身。
4. **仅在用户明确要求**“展示 ID / 原始字段”时，才可附带展示 `open_id`。
5. **基础字段补齐**：会议主题 / 会议号 / 入会链接 若变更接口未返回，使用 `meeting get --meeting-id <id>` 补齐，不得遗漏字段或用 `-` / `N/A` 占位。

---

## 常见错误

| 错误现象 | 原因 | 解决方案 |
|---------|------|---------|
| `--subject is required` | 缺少必填参数 | 补充 `--subject` |
| `--start format error` | 时间格式不合法（如缺少时区） | 改用 `2026-03-12T14:00:00+08:00` 格式 |
| `--meeting-id is required` | 缺少必填参数 | 补充 `--meeting-id` |

## 参考

- [tmeet](../SKILL.md) — 全部命令概览
- [tmeet-record](tmeet-record.md) — 录制管理
- [tmeet-report](tmeet-report.md) — 会议报告