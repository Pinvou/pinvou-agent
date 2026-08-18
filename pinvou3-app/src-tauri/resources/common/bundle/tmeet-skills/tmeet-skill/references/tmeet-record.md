# tmeet record — 录制管理

> **前置条件：** 先执行 `tmeet auth login` 完成登录授权。

时间参数格式：`2026-03-12T14:00:00+08:00` 或 `2026-03-12T14:00+08:00`（必须包含时区）。

> 🚦 **本文核心硬约束**：
> 1. **录制状态门禁**：`state` 为「录制中 / 转码中」时**禁止**查看内容或申请权限，需告知用户等待「转码完成」后再操作。
> 2. **路由分流**：查录制前先判用户线索类型（会议级 vs 内容级），错走 `record search` 会漏掉「无权限」录制及 `permission_status`，导致误报「没找到」（详见 [录制查询路由总则](#录制查询路由总则)）。

## 目录

- [录制查询路由总则](#录制查询路由总则)
- [list — 查询录制列表](#list--查询录制列表)
- [search — 搜索录制](#search--搜索录制)
- [address — 获取录制文件播放地址](#address--获取录制文件播放地址)
- [smart-minutes — 获取智能纪要](#smart-minutes--获取智能纪要)
- [transcript-get — 获取转写详情](#transcript-get--获取转写详情)
- [transcript-paragraphs — 获取转写段落列表](#transcript-paragraphs--获取转写段落列表)
- [transcript-search — 搜索转写内容](#transcript-search--搜索转写内容)
- [permission-apply-prepare — 预览录制权限申请](#permission-apply-prepare--预览录制权限申请)
- [permission-apply-commit — 提交录制权限申请](#permission-apply-commit--提交录制权限申请)
- [典型工作流](#典型工作流)
- [常见错误](#常见错误)
- [参考](#参考)

---

## 录制查询路由总则

用户说"查录制/回放/录屏"时，按用户线索的指向层级分流：

| 用户线索 | 入口命令 | 搜索层级 | 返回的录制 |
|---------|---------|---------|----------|
| 会议号 / 会议 ID | `meeting get` | 会议级 | 该会议全部录制（含无权限的，带 `permission_status`） |
| 会议主题 / 创建人 | `meeting search` | 会议级 | 命中会议即得 `records[]` + `permission_status`（缺失时下钻 `meeting get`） |
| 时间范围（查多场已结束会的录制） | `meeting list-ended` | 会议级 | 多场会议的录制（含无权限的，带 `permission_status`） |
| 内容关键词，不确定哪场会（记得会上说过什么） | `record search --query-field transcript_content` | 跨会议内容级 | 仅自己有权限的录制 |
| 不指定会议、只要自己的录制列表 | `record list` | 自己的录制级 | 仅自己有权限的录制 |
| 已有 `record_file_id`，在文件内定位段落 | `record transcript-search --text` | 单文件内级 | 命中的转写段落（带时间戳） |

`record` 命令族（list/search）命中的录制已经有权限，不需要 `permission_status` 判断；
`meeting` 命令族会返回无权限的录制（用户参加过会但没有录制查看权），用 `permission_status` 区分 `can_view` / `can_apply` / `closed` / `deleted` / `password_required`。
`transcript-search` 是最细粒度的搜索，必须在已锁定到某个录制文件后使用，不要在跨会议场景直接用。

> **为什么"会议主题 / 创建人"优先走 `meeting search` 而非 `record search`**：
> `meeting search` 搜的是会议主题字段，命中会议即得 `records[]` + `permission_status` + `url`，无需再下钻。
> `record search --query-field subject` 搜的是录制标题字段（可能是自动生成的"转写_客户销售会"等），不一定匹配用户给的主题词；且 `record search` 命中的只是自己有权限的录制，拿不到无权限录制和 `permission_status`。

> **"会议号 + 内容关键词"组合（如"683-872-007 那场会上说的预算"）**：
> 不要直接走 `record search --meeting-code + --query`（只返回有权限的，没权限时搜不到、无法区分原因）。
> 先走 `meeting get --meeting-code` 拿 `permission_status`，`can_view` 时再用 `record search --meeting-code + --query` 在该会议内有权限的录制中搜内容；
> `can_apply` / `closed` 时直接走权限流程，不搜内容。
> 这是两步下钻逻辑，不是路由入口选择。

> **`meeting get` 报"会议号无效 / 会议信息不存在"**：说明当前账号无该会议的会议级访问权（未参加 / 未获授权——会议级与录制级授权相互独立）。此时降级用 `record list --meeting-code` 查自己已有权限的录制，不要反复重试 `meeting get`。若 `meeting get` 成功但响应无 `records[]` 字段（该字段可能因环境差异缺失，**缺失不代表无录制**），同样降级 `record list --meeting-code`，且不要对用户断言"该会议没有录制"。

> **进行中会议的录制**：`state` 为「录制中 / 转码中」时不可查看也不可申请（见文首「本文核心硬约束」第 2 条「录制状态门禁」），待会议结束转码完成后再查。

---

## list — 查询录制列表

> ⚠️ 路由前置检查 — 以下场景请改用 `meeting` 命令族：
> - 带会议号 / 会议 ID 查某场会议的录制 → `meeting get`
> - 带主题 / 创建人查某场会议的录制 → `meeting search`
> - 按时间窗批量查多场已结束会议的录制 → `meeting list-ended`
>
> `record list` 仅返回用户已有权限的录制，不返回无权限的录制，也不含 `permission_status`。
> 适用于"不指定某场会议、只想找自己的录制"的场景。

```bash
# 按会议 ID 圈定"自己有权限"的录制范围（要查该会议全部录制及权限状态，请改用 meeting get）
tmeet record list --meeting-id "100000000"

# 按会议码圈定（同上）
tmeet record list --meeting-code "123456789"

# 按时间范围查询
tmeet record list \
  --start "2026-04-01T00:00:00+08:00" \
  --end "2026-04-30T23:59:59+08:00"

# 组合使用：会议 ID + 时间范围（进一步缩小结果范围）
tmeet record list \
  --meeting-id "100000000" \
  --start "2026-04-01T00:00:00+08:00" \
  --end "2026-04-30T23:59:59+08:00"

# 分页查询（使用 page-token翻下一页）
tmeet record list \
  --meeting-id "100000000" \
  --page-token "<next_page_token>" \
  --page-size 30
```

### 参数

| 参数 | 必填 | 默认值 | 说明                                             |
|------|------|--------|------------------------------------------------|
| `--meeting-id <id>` | 至少一组 | — | 会议 ID                                          |
| `--meeting-code <code>` | 至少一组 | — | 会议码                                            |
| `--start <time>` + `--end <time>` | 至少一组 | — | 时间范围（ISO 8601，含时区，作为一组时 `--start` 与 `--end` 必须同时提供） |
| `--page-token <token>` | 否 | — | 分页游标，首页不传；后续翻页传入上一次响应的 `next_page_token` |
| `--page-size <n>` | 否 | `30` | 每页数量，默认 30，最大 30 |
| `--page <n>` | 否 | — | ⚠️ **已弃用**：页码（从 1 开始），请改用 `--page-token` |

> `--meeting-id`、`--meeting-code`、`--start + --end` 三组**至少提供一组**，多组可叠加使用以缩小查询范围。

### 响应字段

`record list` 返回 `data.record_meetings[]`，每条含：

| 字段 | 说明 |
|------|------|
| `meeting_record_id` | 会议录制 ID |
| `meeting_id` / `meeting_code` | 会议标识 |
| `subject` | 录制标题 |
| `state` | 录制状态（如 `转码完成，可根据录制文件权限进行下一步`；录制/转码过程中会有相应中间状态） |
| `record_type` | 录制类型（如 `云录制`、`文字转写`） |
| `media_start_time` | 录制开始时间 |
| `host_user_id` | 主持人 ID |
| `record_files[]` | 录制文件列表（含 `record_file_id` / `record_start_time` / `record_end_time`）。⚠️ 同一查询结果中，部分录制条目含 `record_files[]`、部分不含，与查询方式无关；无 `record_files[]` 时通过 `meeting get` / `meeting list-ended` 的 `records[].record_file_id` 获取文件 ID |

> ⚠️ **`record list` 不返回 `permission_status` / `url` 字段**。`record list` 返回的是用户已有权限的录制，无需权限状态字段。如需查询录制权限状态，请使用 `meeting get` 或 `meeting list-ended`。

---

## search — 搜索录制

> ⚠️ 路由前置检查 — 用户给的是**会议级线索**（会议号 / 会议 ID / 会议主题 / 创建人，指向某场会议）时，请改用 `meeting` 命令族：
> - 会议号 / 会议 ID → `meeting get`
> - 会议主题 / 创建人 → `meeting search`
>
> 用户给的是**录制内容线索**（转写原文 / 纪要关键词，"记得会上说过什么"）时，继续用 `record search`。
>
> `record search` 仅返回用户已有权限的录制，不含 `permission_status`。
> 如需查询无权限的录制（`can_apply` / `closed` 等），请改用 `meeting` 命令族。
>
> ⚠️ 注意：`record search` 是跨会议搜索录制文件；如已锁定到某个 `record_file_id` 要在文件内定位段落，
> 请用 `record transcript-search --text`（单文件内搜索），不要用 `record search`。

按关键词、会议号、会议 ID、时间范围、文件类型等条件搜索录制。所有过滤参数均为可选，可任意组合。**这是按录制内容检索的主搜索入口**：能按**录制内容**（转写原文、智能纪要、时间轴）跨会议检索——这是唯一能跨会议按录制内容检索的命令。按会议主题 / 创建人找录制请优先走 `meeting search`（见上方路由前置检查）。

```bash
# 按转写内容关键词搜索（记得会上说过什么、但不确定哪场会）
tmeet record search --query "季度目标" --query-field transcript_content

# 按智能纪要内容搜索
tmeet record search --query "待办" --query-field smart_minutes

# 按录制主题搜索
tmeet record search --query "项目评审" --query-field subject

# 已确认有权限后、在指定会议内搜内容（"会议号+内容关键词"两步下钻的第二步）
tmeet record search --meeting-id "6953553464429888300" --query "预算" --query-field transcript_content

# 按时间范围 + 文件类型搜索
tmeet record search \
  --start "2026-04-01T00:00+08:00" \
  --end "2026-04-30T23:59+08:00" \
  --file-type video

# 翻下一页
tmeet record search \
  --query "项目评审" \
  --page-token "<next_page_token>" --page-size 30
```

### 参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `--query <text>` | string | 否 | — | 搜索关键词 |
| `--query-field <f>` | string | 否 | `all` | `--query` 的搜索字段：`subject`-录制主题；`creator`-会议创建者昵称/备注名；`transcript_content`-文件中的原始转写内容；`smart_minutes`-文件中的智能纪要内容（摘要 + 待办）；`timeline`-文件中的时间轴内容；`all`-搜索所有字段 |
| `--file-type <t>` | string | 否 | `all` | 文件类型：`video`、`audio`、`transcript`、`upload`、`external`、`all` |
| `--meeting-id <id>` | string | 否 | — | 按会议 ID 过滤 |
| `--meeting-code <code>` | string | 否 | — | 按会议号过滤（精确匹配，仅数字，无短横线） |
| `--start <time>` | string | 否 | — | 查询开始时间（ISO 8601，如 `2026-03-12T14:00+08:00`） |
| `--end <time>` | string | 否 | — | 查询结束时间（ISO 8601，如 `2026-03-12T14:00+08:00`） |
| `--page-token <token>` | string | 否 | — | 分页游标，首页不传；翻页时传入上一次响应的 `next_page_token` |
| `--page-size <n>` | int | 否 | `30` | 每页大小，默认 30，最大 30 |

> **与 `transcript-search` 的区别**：`record search --query-field transcript_content` 是**跨录制**按内容检索（无需先知道 `record_file_id`，用于"记得会上说过什么但不确定哪场会"）；`transcript-search` 是**单个录制文件内**的检索（必须先有 `record_file_id`）。找录制先用 `record search` 定位到文件，再用 `transcript-search` 在文件内精确定位。

> **`record search` vs `meeting search` 的分流**：按**录制内容**检索（转写原文 / 纪要关键词，"记得会上说过什么"）→ `record search`；按**会议级线索**（会议号 / 会议主题 / 创建人）找某场会议或其录制 → `meeting get` / `meeting search`（命中即得该会议 `records[]` + `permission_status`；录制标题可能自动生成，用主题词搜录制命中率低）。

---

## address — 获取录制文件播放地址

```bash
# 获取录制文件播放地址
tmeet record address --meeting-record-id "record_abc123"

# 分页获取（翻下一页）
tmeet record address \
  --meeting-record-id "record_abc123" \
  --page-token "<next_page_token>" \
  --page-size 30
```

### 参数

| 参数 | 必填 | 默认值 | 说明                             |
|------|------|--------|--------------------------------|
| `--meeting-record-id <id>` | ✅ | — | 会议录制 ID（从 `meeting get` / `meeting list-ended` 的 `records[].meeting_record_id` 获取，或从 `record list` 获取） |
| `--page-token <token>` | 否 | — | 分页游标，首页不传；后续翻页传入上一次响应的 `next_page_token` |
| `--page-size <n>` | 否 | `30` | 每页数量，默认 30，最大 30 |
| `--page <n>` | 否 | — | ⚠️ **已弃用**：页码（从 1 开始），请改用 `--page-token` |

### 适用场景

`record address` 与 `meeting get` / `meeting list-ended` 返回的 `records[].url` 几乎等价（均为播放地址）。但 `record address` 有以下独特价值：

- **支持非会议场景的录制文件**：当录制文件不属于某场具体会议（如独立上传的录制文件），无法通过 `meeting get` 获取时，使用 `record address` 按 `meeting_record_id` 直接获取播放地址。
- **已知 `meeting_record_id` 的快捷查询**：无需先查会议，直接按录制 ID 获取播放地址。

> 路由指引：查会议录制（含权限状态）用 `meeting get` / `meeting list-ended`；已知 `meeting_record_id` 且只需播放地址用 `record address`。

---

## smart-minutes — 获取智能纪要

> ⚠️ **权限约束**：本命令仅能获取**当前用户有权限**的录制文件的智能纪要。若不确定是否有权限，先走 `meeting get` / `meeting list-ended` 查 `records[].permission_status`：`can_view` 才继续调本命令；`can_apply` 走权限申请流程（见 [permission-apply-prepare](#permission-apply-prepare--预览录制权限申请)）。

```bash
# 获取录制文件的智能纪要（默认原文）
tmeet record smart-minutes --record-file-id "file_abc123"

# 获取中文翻译版纪要
tmeet record smart-minutes \
  --record-file-id "file_abc123" \
  --lang zh

# 带访问密码的录制文件
tmeet record smart-minutes \
  --record-file-id "file_abc123" \
  --pwd "123456"
```

### 参数

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `--record-file-id <id>` | ✅ | — | 录制文件 ID（从 `meeting get` / `meeting list-ended` 的 `records[].record_file_id` 获取，或从 `record address` 结果中获取） |
| `--lang <lang>` | 否 | `default` | 语言：`default`-原文，`zh`-简体中文，`en`-英文，`ja`-日语 |
| `--pwd <pwd>` | 否 | — | 录制文件访问密码 |

---

## transcript-get — 获取转写详情

> ⚠️ **权限约束**：本命令仅能获取**当前用户有权限**的录制文件的转写详情。若不确定是否有权限，先走 `meeting get` / `meeting list-ended` 查 `records[].permission_status`：`can_view` 才继续调本命令；`can_apply` 走权限申请流程（见 [permission-apply-prepare](#permission-apply-prepare--预览录制权限申请)）。

```bash
# 获取转写详情
tmeet record transcript-get --record-file-id "file_abc123"

# 指定起始段落 ID 与查询段落数
tmeet record transcript-get \
  --record-file-id "file_abc123" \
  --meeting-id "100000000" \
  --pid "<paragraph_id>" \
  --limit "30"
```

### 参数

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `--record-file-id <id>` | ✅ | — | 录制文件 ID |
| `--meeting-id <id>` | 否 | — | 会议 ID |
| `--pid <id>` | 否 | — | 查询的起始段落 ID |
| `--limit <n>` | 否 | — | 查询的段落数 |

---

## transcript-paragraphs — 获取转写段落列表

> ⚠️ **权限约束**：本命令仅能获取**当前用户有权限**的录制文件的转写段落。若不确定是否有权限，先走 `meeting get` / `meeting list-ended` 查 `records[].permission_status`：`can_view` 才继续调本命令；`can_apply` 走权限申请流程（见 [permission-apply-prepare](#permission-apply-prepare--预览录制权限申请)）。

```bash
# 获取转写段落列表
tmeet record transcript-paragraphs --record-file-id "file_abc123"

# 指定会议 ID
tmeet record transcript-paragraphs \
  --record-file-id "file_abc123" \
  --meeting-id "100000000"
```

### 参数

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `--record-file-id <id>` | ✅ | — | 录制文件 ID |
| `--meeting-id <id>` | 否 | — | 会议 ID |

---

## transcript-search — 搜索转写内容

> ⚠️ **权限约束**：本命令仅能在**当前用户有权限**的录制文件内搜索转写内容。若不确定是否有权限，先走 `meeting get` / `meeting list-ended` 查 `records[].permission_status`：`can_view` 才继续调本命令；`can_apply` 走权限申请流程（见 [permission-apply-prepare](#permission-apply-prepare--预览录制权限申请)）。

```bash
# 在转写内容中搜索关键词
tmeet record transcript-search \
  --record-file-id "file_abc123" \
  --text "季度目标"

# 指定会议 ID 搜索
tmeet record transcript-search \
  --record-file-id "file_abc123" \
  --meeting-id "100000000" \
  --text "行动项"
```

### 参数

| 参数 | 必填 | 说明 |
|------|------|------|
| `--record-file-id <id>` | ✅ | 录制文件 ID |
| `--text <keyword>` | ✅ | 搜索关键词 |
| `--meeting-id <id>` | 否 | 会议 ID |

---

## permission-apply-prepare — 预览录制权限申请

本命令拉取审批文案、会议主题、录制所有者等预览信息，**展示给用户二次确认后**，再调用 `record permission-apply-commit` 真正提交申请。

### 何时使用（唯一触发路径）

**只有一个入口**：`meeting get` / `meeting search` / `meeting list-ended` 返回的 `records[].permission_status = can_apply` —— 该会议录制存在但当前用户无权限、**允许申请**。命中即走本命令。

> **前置门禁**：
> - `state` 为「录制中 / 转码中」时**禁止**申请，需等「转码完成」（见文首「本文核心硬约束」第 1 条）；
> - `permission_status` 为 `password_required` 时走密码路径（`--pwd`），不是权限申请。

```bash
# 预览录制权限申请信息
tmeet record permission-apply-prepare --meeting-record-id "record_abc123"

# 同时指定会议 ID
tmeet record permission-apply-prepare \
  --meeting-record-id "record_abc123" \
  --meeting-id "100000000"
```

### 参数

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `--meeting-record-id <id>` | ✅ | — | 会议录制 ID |
| `--meeting-id <id>` | 否 | — | 会议 ID |

### 响应关键字段

| 字段 | 说明 |
|------|------|
| `preview.meeting_record_id` | 会议录制 ID |
| `preview.approval_name` | 申请类型文案 |
| `preview.subject` | 会议标题 |
| `preview.file_owner` | 录制所有者名称 |
| `preview.apply_note` | 权限申请备注信息 |
| `preview.applicant` | 申请人名称 |
| `expires_in` | 过期时间（秒），超过后需重新 prepare |

---

## permission-apply-commit — 提交录制权限申请

> ⚠️ **高风险写操作**：本命令会正式发起审批流程。**必须先调用 `permission-apply-prepare` 拉取预览信息**，将申请类型 / 会议标题 / 录制所有者 / 申请备注等关键字段完整展示给用户，**待用户明确同意后再调用本命令**。

```bash
# 提交录制权限申请
tmeet record permission-apply-commit --meeting-record-id "record_abc123"

# 同时指定会议 ID
tmeet record permission-apply-commit \
  --meeting-record-id "record_abc123" \
  --meeting-id "100000000"
```

### 参数

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `--meeting-record-id <id>` | ✅ | — | 会议录制 ID（必须与 `permission-apply-prepare` 一致）|
| `--meeting-id <id>` | 否 | — | 会议 ID |

### 响应关键字段

| 字段 | 说明 |
|------|------|
| `unique_id` | 申请 ID |
| `status` | 审批状态 |
| `message` | 审批状态描述 |
| `approval_url` | 审批链接（可展示给用户跟踪审批进度）|
| `share_text` | 申请说明描述（可分享给审批人）|

---

## 典型工作流

```
1. 获取 meeting_record_id
   → 参见文首 [录制查询路由总则](#录制查询路由总则)，根据用户线索类型选择入口命令

2. 获取录制文件下载地址，获取 record_file_id
   tmeet record address --meeting-record-id <meeting_record_id>

3. 获取智能纪要 / 转写内容
   tmeet record smart-minutes --record-file-id <record_file_id>
   tmeet record transcript-get --record-file-id <record_file_id>
   tmeet record transcript-search --record-file-id <record_file_id> --text "关键词"
```

### 无录制权限时的申请流程

> **触发条件**：`meeting get` / `meeting search` / `meeting list-ended` 返回 `records[].permission_status = can_apply`。详细前置门禁（`state` 状态、`password_required` 分流、以及为何不能靠 `record *` 报错触发）见 [permission-apply-prepare — 何时使用](#permission-apply-prepare--预览录制权限申请)。

命中后按以下流程发起权限申请：

```
1. 调用 prepare 获取预览信息
   tmeet record permission-apply-prepare --meeting-record-id <meeting_record_id>

2. 将 preview 中的「申请类型 / 会议标题 / 录制所有者 / 备注 / 申请人」完整展示给用户，
   并明确询问是否同意发起权限申请；

3. 收到用户明确确认（"确认"/"是"/"yes" 等肯定指令）后，再调用 commit 提交申请：
   tmeet record permission-apply-commit --meeting-record-id <meeting_record_id>

4. 将 commit 响应中的 approval_url 展示给用户跟踪审批进度；
   若用户未明确确认或表示取消，则终止流程，不得调用 commit。
```

> **重要**：`permission-apply-commit` 为写操作，**严禁在未经用户确认时直接执行**。`prepare` 返回的 `expires_in` 过期后，需重新调用 `prepare` 拉取最新预览再确认提交。

## 常见错误

| 错误现象 | 原因 | 解决方案 |
|---------|------|---------|
| `one of the following groups is required` | 缺少必填参数组 | 提供 `--meeting-id`、`--meeting-code` 或 `--start + --end` 其中一组 |
| `--start format error` | 时间格式不合法（如缺少时区） | 改用 `2026-03-12T14:00:00+08:00` 格式 |
| `--record-file-id is required` | 缺少必填参数 | 从 `meeting get` / `meeting list-ended` 的 `records[].record_file_id`，或 `record list` 部分条目的 `record_files[].record_file_id` 获取 |
| `--text is required` | 搜索缺少关键词 | 补充 `--text` |
| `meeting get` / `meeting search` / `meeting list-ended` 返回 `permission_status = can_apply` | 有该会议录制、当前用户无权限但允许申请 | 直接走 `permission-apply-prepare` 预览 → 用户确认 → `permission-apply-commit`（这是权限申请的**唯一**入口） |
| `record list` 静默返回空数据（无 error） | 可能是无权限（不会报错）、也可能录制不存在 —— **无法从此响应本身区分** | **不要**据此直接发起权限申请；回退到 `meeting get` / `meeting list-ended` 查 `permission_status`：`can_apply` 走申请、无该 `records[]` 项则确为不存在 |
| 对 `state=录制中/转码中` 或 `permission_status=closed/deleted` 的录制发起 `permission-apply-*` | 违反录制状态门禁 / 不可申请状态 | 转码中告知用户等待；`closed`/`deleted` 明确告知不可申请，不走 prepare/commit |

## 参考

- [tmeet](../SKILL.md) — 全部命令概览
- [tmeet-meeting](tmeet-meeting.md) — 会议管理
- [tmeet-report](tmeet-report.md) — 会议报告
