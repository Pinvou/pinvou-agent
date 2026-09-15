# tmeet report — 会议报告

> **前置条件：** 先执行 `tmeet auth login` 完成登录授权。

时间参数格式：`2026-03-12T14:00:00+08:00` 或 `2026-03-12T14:00+08:00`（必须包含时区）。

## 目录

- [participants — 获取参会人列表](#participants--获取参会人列表)
- [waiting-room-log — 获取等候室成员列表](#waiting-room-log--获取等候室成员列表)
- [participants-export — 导出参会成员明细](#participants-export--导出参会成员明细)
- [job-result — 获取异步任务结果](#job-result--获取异步任务结果)
- [常见错误](#常见错误)
- [参考](#参考)

---

## participants — 获取参会人列表

> ⚠️ **适用场景**：快速获取参会成员名单、会议角色、是否企业内成员、会中成员的麦克风/摄像头/屏幕共享状态（开/关）。本命令仅会议创建者/主持人/联席主持人可调用，若收到相关提示，无需重试直接告知用户。

> 🚫 **权限被拒后禁止绕行**：收到 `9042`（无权限操作）时，**必须**向用户明确说明「仅会议创建者/主持人/联席主持人或具备会议控制权限的角色可查询参会者」，并**停止获取该会议的参会数据**。**严禁改用 `participants-export`、`job-result` 下载文件**

```bash
# 获取会议参会人列表
tmeet report participants --meeting-id "100000000"

# 分页获取（每页 50 条，翻下一页）
tmeet report participants \
  --meeting-id "100000000" \
  --page-token "<next_page_token>" \
  --page-size 50

# 获取周期性会议某个子会议的参会人
tmeet report participants \
  --meeting-id "100000000" \
  --sub-meeting-id "200000001"

# 按时间范围过滤参会人
tmeet report participants \
  --meeting-id "100000000" \
  --start "2026-04-10T14:00:00+08:00" \
  --end "2026-04-10T15:00:00+08:00"
```

### 返回结果说明

> ⚠️ 返回该会议的**全量**参会成员（含已离会与仍在会中）。成员的 `left_time` 为空表示**仍在会中**，非空表示**已离会**（值即离会时间）。

### 参数

| 参数 | 必填 | 默认值   | 说明                                       |
|------|------|-------|------------------------------------------|
| `--meeting-id <id>` | ✅ | —     | 会议 ID                                    |
| `--sub-meeting-id <id>` | 否 | —     | 子会议 ID（周期性会议的某场）                         |
| `--start <time>` | 否 | —     | 查询起始时间（ISO 8601，含时区）                     |
| `--end <time>` | 否 | —     | 查询结束时间（ISO 8601，含时区）                     |
| `--page-token <token>` | 否 | —     | 分页游标，首页不传；后续翻页传入上一次响应的 `next_page_token` |
| `--page-size <n>` | 否 | `100` | 每页数量，默认 100，最大 100                       |
| `--pos <n>` | 否 | —     | ⚠️ **已弃用**：分页起始位置，请改用 `--page-token`     |
| `--size <n>` | 否 | —     | ⚠️ **已弃用**：每页数量，请改用 `--page-size`        |

---

## waiting-room-log — 获取等候室成员列表

> ⚠️ **适用场景**：获取等候室成员记录。本命令仅会议创建者/主持人/联席主持人可调用，若收到相关提示，无需重试直接告知用户。

```bash
# 获取等候室成员列表
tmeet report waiting-room-log --meeting-id "100000000"

# 分页获取（翻下一页）
tmeet report waiting-room-log \
  --meeting-id "100000000" \
  --page-token "<next_page_token>" \
  --page-size 50
```

### 参数

| 参数 | 必填 | 默认值   | 说明                                       |
|------|------|-------|------------------------------------------|
| `--meeting-id <id>` | ✅ | —     | 会议 ID                                    |
| `--page-token <token>` | 否 | —     | 分页游标，首页不传；后续翻页传入上一次响应的 `next_page_token` |
| `--page-size <n>` | 否 | `100` | 每页数量，默认 100，最大 100                       |
| `--page <n>` | 否 | —     | ⚠️ **已弃用**：页码，请改用 `--page-token`         |

---

## participants-export — 导出参会成员明细

> ⚠️ **适用场景**：获取/导出参会成员明细，用于分析统计：会议总时长、参会总人数，成员参会/开麦/开摄像头/共享屏幕时长、入会/提问/发消息/投票次数、企业内成员组织架构/职位信息。本命令仅会议创建者在会议结束后可调用，会议进行中可使用report participants获取实时名单，严禁为导出明细而修改或结束会议。

> 🚫 **不得用作 `participants` 的权限绕行路径**：若本次会话中 `report participants`已因 `9042` 无权限被拒，**严禁转而调用本命令获取同一会议的参会数据**。

> **异步操作**：本命令仅提交导出任务并返回 `job_id`，获取到 `job_id` 后必须调用 `job-result` 轮询任务状态，直到获取文件下载链接。详见下方[完整工作流](#完整工作流)。

```bash
# 导出会议参会成员明细（默认 xlsx 格式）
tmeet report participants-export --meeting-id "100000000"

# 导出为 json 格式
tmeet report participants-export \
  --meeting-id "100000000" \
  --file-type "json"

# 导出周期性会议某个子会议的参会成员
tmeet report participants-export \
  --meeting-id "100000000" \
  --sub-meeting-id "200000001"

# 按时间范围过滤并指定超时时间
tmeet report participants-export \
  --meeting-id "100000000" \
  --start "2026-04-10T14:00:00+08:00" \
  --end "2026-04-10T15:00:00+08:00"
```

### 参数

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `--meeting-id <id>` | ✅ | — | 会议 ID |
| `--sub-meeting-id <id>` | 周期性会议时必填 | — | 子会议 ID（周期性会议时必填） |
| `--start <time>` | 否 | — | 查询起始时间（ISO 8601，含时区） |
| `--end <time>` | 否 | — | 查询结束时间（ISO 8601，含时区） |
| `--file-type <type>` | 否 | `xlsx` | 导出文件格式：`xlsx` 或 `json` |

### 响应关键字段

| 字段 | 说明 |
|------|------|
| `job_id` | 异步任务 ID（用于轮询任务状态） |

### 完整工作流

`participants-export` 为异步操作，后台生成文件需要一定时间。完整流程如下：

```
1. 提交导出任务，获取 job_id
   tmeet report participants-export --meeting-id "100000000"

2. 每隔 5 秒调用 job-result 轮询任务状态
   tmeet report job-result --job-id <job_id>

3. 根据返回的 status 判断：
   - status = "成功"：返回文件下载链接 url（有效期 2 小时），流程结束
   - status = "处理中"：等待 5 秒后再次调用 job-result 继续轮询
   - status = "失败" 或其他值：终止并返回 error_msg
```

> **注意**：`participants-export` 命令本身只返回 `job_id`，不会自动等待任务完成。agent 必须在获取 `job_id` 后主动调用 `job-result` 进行轮询，直到获取最终结果。

---

## job-result — 获取异步任务结果

查询异步导出任务的执行状态与结果。调用 `participants-export` 获取 `job_id` 后，需每隔 5 秒调用本命令轮询，直到任务完成或失败。

```bash
# 查询异步任务结果
tmeet report job-result --job-id "e1234567-f123-4d12-123a-12346192e332"
```

### 参数

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `--job-id <id>` | ✅ | — | 任务 ID（从 `participants-export` 获取） |

### 返回字段

| 字段 | 说明 |
|------|------|
| `status` | 任务状态："成功"、"失败"、"处理中" |
| `url` | 文件下载链接（状态为 "成功" 时返回，有效期 2 小时） |
| `error_msg` | 错误信息（状态为 "失败" 时返回） |

---

## 常见错误

| 错误现象 | 原因 | 解决方案 |
|---------|------|---------|
| `--meeting-id is required` | 缺少必填参数 | 补充 `--meeting-id` |
| `--start format error` | 时间格式不合法（如缺少时区） | 改用 `2026-03-12T14:00:00+08:00` 格式 |
| `--job-id is required` | 缺少必填参数 | 补充 `--job-id`（从 `participants-export` 获取） |
| `export task failed` | 后台导出任务执行失败（`job-result` 返回 "失败"） | 检查 `error_msg` 中的具体原因，确认参数后重试 |

## 参考

- [tmeet](../SKILL.md) — 全部命令概览
- [tmeet-meeting](tmeet-meeting.md) — 会议管理
- [tmeet-record](tmeet-record.md) — 录制管理
- [tmeet-control](tmeet-control.md) — 会中控制（`participants` / `waiting-room-log` 返回的 `open_id` / `ms_open_id` 是 `control kick` / `control waiting-room` 的必备输入）
