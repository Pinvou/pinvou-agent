# tmeet minutes — 元宝纪要管理

## 目录

- [元宝纪要查询路由总则](#元宝纪要查询路由总则)
- [`search` — 搜索元宝纪要](#search--搜索元宝纪要)
- [`get` — 查询元宝纪要详情](#get--查询元宝纪要详情)
  - [模式一：获取稳态纪要](#模式一获取稳态纪要)
  - [模式二：获取滚动（瞬态）纪要](#模式二获取滚动瞬态纪要)
- [典型工作流](#典型工作流) · [参考](#参考)

> **前置条件：** 先执行 `tmeet auth login` 完成登录授权。

> **领域边界与链路选择：** 元宝纪要与录制纪要（`record smart-minutes`）是两条独立链路。
> **判据只有一条 —— 先看录制权限**：
> **有录制查看权限（`permission_status=can_view`）→ 取录制纪要 `record smart-minutes`（内容更全，含逐字稿）；
> 无权限 / 无录制（`can_apply` / `closed` / 无录制文件）→ 取元宝纪要 `minutes get`。**
> 用户明确指定纪要类型时以用户为准。**不得仅凭命令名字面匹配。**

> **参数以本文档为准**：下列 `search` / `get` 的参数表持续更新。
> **执行前无需再跑 `tmeet minutes search --help` / `minutes get --help` 确认参数**。
> 仅当命令返回 `unknown flag` 类报错时，才需 `--help` 复核。

时间参数格式：`2026-03-12T14:00:00+08:00` 或 `2026-03-12T14:00+08:00`（必须包含时区）。

---

## 元宝纪要查询路由总则

用户说"查纪要/会议总结/会议纪要/会议要点"时，**先判录制权限决定走哪条链路**（有权限走录制、无权限走元宝，见上方「领域边界与链路选择」）。确定走元宝后，按用户线索选择命令：

| 用户线索 | 入口命令 | 说明 |
|---------|---------|------|
| 会议号 / 会议 ID | `minutes get --meeting-code` / `--meeting-id` | 取该会议的元宝纪要 |
| 已有 `minute_id` | `minutes get --minute-id` | 取单份完整纪要（含实时滚动总结） |
| 纪要内容关键词（无会议号、记得会上说过什么） | `minutes search --query` | 跨会议搜纪要文本 |
| 主题 / 创建人 / 时间范围（无会议号） | 先 `meeting search` / `list-ended` 定位会议，再 `minutes get --meeting-id` | 元宝无按主题/创建人直接搜纪要的命令 |
| 要"原话/逐字稿/谁说了什么" | 元宝无逐字稿，走录制链路 `record transcript-*`；无录制权限时降级取元宝 `short_summaries` 标注"非原话" | 降级时必须标注「非原话/AI 加工版」 |

> **元宝纪要 vs 录制纪要**：元宝基于会中 ASR、参会者人人可取、因人而异、无链接、无逐字稿；录制基于录制文件、创建者所有、需权限、多人共享、有播放地址、有逐字稿。
> **选择时不必逐项比对上述差异 —— 只看录制权限**：有 `can_view` 取录制，否则取元宝。

> **当前命令仅支持会后获取**：元宝纪要在会中生成，但 `minutes` 命令**仅支持取已结束会议**的纪要。
> 会议进行中或未开始时，**直接告知用户会后重试，不要重复拉取**（重复拉取只会连续返回空），
> 也不得臆造尚未生成的纪要内容。

> **术语纪律**：对用户一律称「元宝纪要」或「会议纪要」，**严禁称为「智能纪要」**
> ——「智能纪要」在本体系中特指录制链路产物（`record smart-minutes`），混用会让用户误判数据来源与权限要求。
> **来源与创建人不可混填**：需标注数据来源（元宝纪要 / 录制转写）时须单独设「来源」列，
> **不得填入「创建人」列** —— 创建人只能是真实用户名。

---

## search — 搜索元宝纪要

按关键词、时间范围搜索元宝纪要。所有过滤参数均为可选，可任意组合。

```bash
# 按关键词搜索
tmeet minutes search --query "季度目标"

# 按时间范围搜索
tmeet minutes search \
  --start "2026-04-01T00:00+08:00" \
  --end "2026-04-30T23:59+08:00"

# 关键词 + 时间范围组合搜索
tmeet minutes search \
  --query "项目评审" \
  --start "2026-04-01T00:00+08:00" \
  --end "2026-04-30T23:59+08:00"

# 翻下一页
tmeet minutes search \
  --query "项目评审" \
  --page-token "<next_page_token>" --page-size 20
```

### 参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `--query <text>` | string | 否 | — | 搜索关键词，最多 50 字 |
| `--start <time>` | string | 否 | — | 搜索时间下界（ISO 8601，如 `2026-03-12T14:00+08:00`） |
| `--end <time>` | string | 否 | — | 搜索时间上界（ISO 8601，如 `2026-03-12T14:00+08:00`） |
| `--page-token <token>` | string | 否 | — | 分页游标，首页不传；翻页时传入上一次响应的 `next_page_token` |
| `--page-size <n>` | int | 否 | `20` | 每页大小，默认 20，最大 50 |

> **时间匹配为 OR 宽容策略**：纪要开启时间 / 会议预约开始时间 / 会议实际开始时间
> **任一** ∈ `[start, end]` 即命中。因此可能召回「预约时间在区间外、但实际开始在区间内」的会议，属正常，不必反复缩放时间窗。
> 约束：`start < end`；跨度 ≤ 1 年；`start` 距当前 ≤ 1 年。违反会直接报错。

> **支持按发言人检索**：`--query` 会匹配 `short_summaries` 的 `speakers[]` 字段，
> 因此「张三在会上提过什么」「@小楠说了什么」可直接用 `minutes search --query "张三"`。
> ⚠️ 命中的是 AI 总结片段而非发言原话，回复时须标注「非原话」；用户要逐字原话仍须走 `record transcript-*`。

### 响应字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `total_count` | int | 命中总数 |
| `has_more` | bool | 是否有下一页 |
| `next_page_token` | string | 下一页游标（`has_more=false` 时不返回） |
| `minutes[]` | array | 元宝纪要列表，按纪要创建时间倒排 |
| `minutes[].minute_id` | string | 纪要唯一标识（**供 `get --minute-id` 使用**） |
| `minutes[].meeting_id` | string | 会议 ID（周期级）⚠️ **内部标识，严禁向用户展示，对用户一律用 meeting_code** |
| `minutes[].subject` | string | 所属会议主题 |
| `minutes[].minute_start_time` | string | 纪要开始时间（ISO 8601） |
| `minutes[].q_fields` | array? | 命中字段：overview/summary_points/todos/short_summaries |
| `minutes[].snippets[]` | array? | 命中片段：`source` / `timestamp` / `text`（关键词用 `<mark>` 包裹，前后文约 150 字符，每条结果最多 3 个） |

> **与 `record search` 的区别**：`minutes search` 搜元宝纪要文本（overview/summary_points/todos/short_summaries）；`record search --query-field transcript_content` 搜录制转写原文。两者检索范围不同，按用户要的纪要类型选择。

---

## get — 查询元宝纪要详情

支持两种模式：

### 模式一：获取稳态纪要

通过会议 ID、会议号或纪要 ID 查询稳态（完整）纪要。`short-summary` 不传或为 `false` 时走此模式。

> **meeting-code 与 meeting-id 的区别**：`meeting-code` 是会议号（通常为 9~12 位数字），`meeting-id` 是会议唯一标识（通常为 13 位以上的数字字符串）。

```bash
# 按纪要 ID 查询稳态纪要
tmeet minutes get --minute-id "minute_abc123"

# 按会议 ID 查询稳态纪要
tmeet minutes get --meeting-id "6953553464429888300"

# 按会议号查询稳态纪要（自动解析为 meeting-id）
tmeet minutes get --meeting-code "295150176"

# 按会议 ID + 子会议 ID 查询（周期性会议）
tmeet minutes get \
  --meeting-id "6953553464429888300" \
  --sub-meeting-id "100001"

# 仅获取概览和待办，不获取要点
tmeet minutes get \
  --meeting-id "6953553464429888300" \
  --summary-points=false

# 翻下一页（当一个会议有多份纪要时）
tmeet minutes get \
  --meeting-id "6953553464429888300" \
  --page-token "<next_page_token>" --page-size 10
```

#### 稳态纪要参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `--minute-id <id>` | string | 三选一 | — | 纪要唯一标识；来自 `minutes search` 或上次 `get` 响应的 `minutes[].minute_id`。**已有时直接用，最省调用** |
| `--meeting-id <id>` | string | 三选一 | — | 会议 ID（13 位以上） |
| `--meeting-code <code>` | string | 三选一 | — | 会议号（9~12 位） |
| `--sub-meeting-id <id>` | string | **周期会必填** | — | 子会议 ID。非周期会议不传；**周期性会议必须传入以定位具体子实例**，否则返回该周期会下全部纪要（可能上百条），无法满足「其中一场」类诉求 |
| `--overview` | bool | 否 | `true` | 是否获取会议概览 |
| `--summary-points` | bool | 否 | `true` | 是否获取要点 |
| `--todos` | bool | 否 | `true` | 是否获取待办 |
| `--page-token <token>` | string | 否 | — | 分页游标，首页不传；翻页时传入上一次响应的 `next_page_token` |
| `--page-size <n>` | int | 否 | `10` | 每页大小，默认 10，最大 30 |

#### 三选一速查表

| 用户给的 / 手头已有的 | 选用参数 | 判断依据 |
|---------|---------|---------|
| **已有 `minute_id`**（来自 `search` 或上次 `get`） | `--minute-id` | **优先用，直接取单份，最省调用** |
| 9~12 位纯数字的会议号 | `--meeting-code` | 位数 9~12 |
| 13 位以上数字字符串的会议唯一标识 | `--meeting-id` | 位数 ≥13 |
| 用户说「其中一场 / 这周那场 / 上周那场」+ 周期会 | `--meeting-id` + **`--sub-meeting-id`** | **必须定位到子实例**，否则会返回全部子会议纪要 |

> **位数不符时不要猜**：如果用户给的数字位数不符合上述规则（如只给了 5 位），
> **不要自行选用参数、不要补位、不要拿它依次试三种参数**，应先问用户澄清完整的会议号或会议 ID。

#### 稳态纪要响应字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `meeting_id` | string | 会议 ID ⚠️ **内部标识，严禁向用户展示，对用户一律用 meeting_code** |
| `subject` | string | 会议主题 |
| `minutes[]` | array | 纪要列表，按纪要创建时间倒排；周期性会议或同一会议多次入会时，会有多个纪要 |
| `minutes[].minute_id` | string | 纪要唯一标识 |
| `minutes[].created_at` | string | 纪要创建时间（ISO 8601） |
| `minutes[].overview` | string | 概览，≤100 字，1 句话，主题 + 议题 + 结论 |
| `minutes[].summary_points` | string | 议题总结（全文） |
| `minutes[].todos[]` | array | 待办事项，会后需要执行的项 |
| `has_more` | bool | 是否有下一页 |
| `next_page_token` | string | 下一页游标（末页时为空） |

---

### 模式二：获取滚动（瞬态）纪要

通过 `--short-summary` 标志获取滚动纪要，**必须提供 `--minute-id`**（稳态纪要唯一标识）。命令会自动循环分页拉取（使用返回的 `next_page_token` 不断请求下一页，直到 `has_more=false`），最终合并所有 items 一次性输出。

> ⚠️ **滚动总结最多返回 100 条**：超出部分不会返回。当 `total_count > 100` 或已取满 100 条时，
> **必须告知用户**「本次会议滚动纪要较长，已展示前 100 条，完整内容请前往腾讯会议客户端查看」，
> **不得默认已取全**、不得在未告知的情况下基于残缺内容做总结或写周报。

```bash
# 获取全量滚动纪要（自动循环翻页）
tmeet minutes get --short-summary --minute-id "minute_abc123"

# 指定每页大小
tmeet minutes get --short-summary --minute-id "minute_abc123" --page-size 200

# 从指定 page-token 开始拉取全量
tmeet minutes get --short-summary --minute-id "minute_abc123" \
  --page-token "<next_page_token>"

# 两步链：只有会议号时取滚动纪要
# ① 先用会议号取稳态纪要，拿到 minute_id
tmeet minutes get --meeting-code "295150176"
# ② 再用 minute_id 取滚动纪要
tmeet minutes get --short-summary --minute-id "<①返回的 minutes[].minute_id>"
```

#### 滚动纪要参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `--short-summary` | bool | 是 | `false` | 设为 `true` 表示获取滚动（瞬态）纪要 |
| `--minute-id <id>` | string | 是 | — | 纪要唯一标识（来自 `minutes search` 响应的 `minutes[].minute_id`，或 `minutes get` 稳态响应的 `minutes[].minute_id` 字段） |
| `--page-token <token>` | string | 否 | — | 起始分页游标，不传则从第一页开始 |
| `--page-size <n>` | int | 否 | `100` | 每页大小，默认 100，最大 300 |

#### 滚动纪要响应字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `minute_id` | string | 纪要 ID |
| `items` | array | 滚动纪要记录列表 |
| `items[].timestamp` | string | 时间戳（MM:SS，相对会议开始时间的偏移） |
| `items[].speakers` | array | 发言人展示名列表 |
| `items[].content` | string | 总结内容 |
| `total_count` | int | 总记录数 |
| `has_more` | boolean | 是否有更多记录 |
| `next_page_token` | string | 下一页游标 |

---

## 批量查询（多场会议）

用户说「上周所有会的纪要」「这个月的会议总结」「昨天所有会议的纪要」时：

1. **优先用 `minutes search --start --end` 一次性检索** —— search 支持时间范围，
   一次可返回多场纪要（含 `subject` / `minute_id` / 命中片段），**避免逐场 `get` 造成 N+1 次调用**
2. 若需按会议维度核对，再用 `meeting list-ended --start --end` 补齐会议清单
3. **仅当用户明确要某场完整正文时**，才用该场 `minute_id` 单独 `get`

> **⚠️ 会议数 > 10 场时，先问再取**：先向用户展示会议清单（主题 / 时间 / 有无纪要），
> 询问「需要展开哪几场的完整内容」，**不要默认逐场拉全文**。
> 30 场会逐场 `get` ≈ 30 次调用，成本极高且大部分正文用户并不需要。

> **不要自行写脚本绕过 CLI**：分页与聚合用 `--page-token` + `--page-size` 完成，
> 不可控的自写脚本会导致结果不可复现。

> **响应字段类型需做兼容**：`todos` / `summary_points` 在部分会议返回**字符串**而非数组，
> 解析时不要假设固定为 array，需做类型判断后再取值。

---

## 典型工作流

```
1. 判断纪要类型（元宝 vs 录制）
   先用 meeting get 拿 permission_status：can_view → 走录制；can_apply/closed/无录制 → 走元宝
   确定走元宝 → 继续；走录制 → 见 tmeet-record.md

2. 定位会议（若用户给会议号/会议 ID 可跳过）
   - 会议号 → minutes get --meeting-code
   - 会议 ID → minutes get --meeting-id
   - 主题/创建人 → meeting search 拿 meeting_id
   - 时间范围 → meeting list-ended 拿 meeting_id
   - 纪要内容关键词（无会议号）→ minutes search --query

3. 取纪要
   - 取一场会议的所有完整总结 → minutes get --meeting-code / --meeting-id
   - 取单份完整纪要（含实时总结）→ minutes get --minute-id
   - 单独取实时总结 → minutes get --minute-id --short-summary

4. 原话/逐字稿需求
   - 有录制权限 → 走 record transcript-*（见 tmeet-record.md）
   - 无录制权限 → 降级取 minutes get --minute-id --short-summary，标注"非原话/AI 加工版"
```

---

## 参考

- [tmeet](../SKILL.md) — 全部命令概览
- [tmeet-record](tmeet-record.md) — 录制管理（`record smart-minutes` / `record transcript-*` / `record address`）
- [tmeet-meeting](tmeet-meeting.md) — 会议管理（`meeting search` / `get` / `list-ended`）