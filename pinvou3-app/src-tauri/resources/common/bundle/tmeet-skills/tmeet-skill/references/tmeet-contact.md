# tmeet contact — 通讯录

> **前置条件：** 先执行 `tmeet auth login` 完成登录授权。

> 🚫 **适用场景硬约束**：本文档下的所有命令（`search` / `lookup-by-phone` / `lookup-by-email`）返回的 `open_id` **仅可用于「会议邀请」（`meeting invitees-add` / `meeting invitees-replace`）和「呼叫会外成员入会」（`control call`）**两类场景；**严禁作为 `control kick`（会中踢人）的成员来源**——`kick` 的成员必须从 [`tmeet report participants`](tmeet-report.md#participants--获取参会人列表) 获取。

按用户名检索企业通讯录成员，可结合职位或部门进一步过滤搜索结果。

---

## search — 搜索企业通讯录成员

> ⚠️ **`search` 返回的 `openId` 是组织成员名录中的 ID，并不代表该成员已在某场会议中。请勿将本命令的输出作为 `control kick` 的成员来源；踢人场景请使用 [`tmeet report participants`](tmeet-report.md#participants--获取参会人列表) 拉取会中成员。**

```bash
# 按用户名搜索
tmeet contact search --username "张三"

# 用户名 + 职位过滤（用于命中结果较多时进一步缩小范围）
tmeet contact search \
  --username "张三" \
  --job-title "工程师"

# 用户名 + 部门过滤
tmeet contact search \
  --username "张三" \
  --department-name "研发部"

# 同时使用职位 + 部门过滤
tmeet contact search \
  --username "张三" \
  --job-title "工程师" \
  --department-name "研发部"
```

### 参数

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `--username <name>` | ✅ | — | 要搜索的用户名 |
| `--job-title <title>` | 否 | — | 当用户名搜索结果过多时，用于过滤的职位名称 |
| `--department-name <dept>` | 否 | — | 当用户名搜索结果过多时，用于过滤的部门名称 |

### 使用准则

- **`--username` 为必填**：缺失时命令会报错，必须先与用户确认要查找的用户名后再执行。
- **结果较多时建议追加过滤**：当仅按用户名查询返回的成员较多（如同名情况）时，建议结合 `--job-title` 或 `--department-name` 进一步过滤，提升匹配精准度。
- **唯一命中时仅返回 `openId`**：当搜索结果**只有一条**时，命令仅返回该成员的 `openId` 字段，**不会返回部门、职位等其他成员信息**；模型可直接将该 `openId` 用于后续命令（如会议邀请、呼叫入会），无需也无法基于该响应向用户展示部门/职位等字段。
- **多结果必须由用户确认，禁止自行猜测**：当搜索返回 **多个匹配成员** 时（例如同名、同部门多人等），**严禁**模型基于职位、部门、入职时间等任何信息自行选择某一条结果继续后续操作。必须将候选成员的关键信息（如姓名、部门、职位）以清晰列表形式展示给用户，并明确询问"请确认要选择哪一位"，待用户明确指定后再继续执行后续命令（如发起会议邀请、呼叫、踢人等）。即便其中某条结果看起来"明显更匹配"，也必须等待用户确认，不得跳过该步骤。
- **隐私提示**：返回的数据可能包含工号、手机号、邮箱等敏感字段，输出给用户时仅展示与问题直接相关的关键信息（如姓名、部门、职位），不得擅自展示其他敏感字段。

---

## lookup-by-phone — 按手机号查找用户

```bash
# 按单个手机号查找
tmeet contact lookup-by-phone --phones "13800138000"

# 按多个手机号批量查找（逗号分隔，最多50个）
tmeet contact lookup-by-phone --phones "13800138000,13900139000,13700137000"

# 启用精简输出模式
tmeet contact lookup-by-phone --phones "13800138000" --compact

# 美化格式输出
tmeet contact lookup-by-phone --phones "13800138000" --format json-pretty
```

### 参数

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `--phones <numbers>` | ✅ | — | 要查找的手机号列表，用英文逗号分隔，最多支持50个号码 |

### 使用准则

- **手机号格式校验**：系统会自动校验手机号格式，格式不正确时会报错。
- **批量查询效率**：支持最多50个手机号批量查询，适合批量查找多个用户信息。
- **精确匹配**：按手机号精确匹配用户信息，返回结果包含用户的基本信息和联系方式。
- **隐私保护**：返回结果可能包含用户的详细信息，输出时仅展示用户明确需要的字段。

---

## lookup-by-email — 按邮箱查找用户

```bash
# 按单个邮箱查找
tmeet contact lookup-by-email --emails "zhangsan@example.com"

# 按多个邮箱批量查找（逗号分隔，最多50个）
tmeet contact lookup-by-email --emails "zhangsan@example.com,lisi@example.com,wangwu@example.com"

# 启用精简输出模式
tmeet contact lookup-by-email --emails "zhangsan@example.com" --compact

# 美化格式输出
tmeet contact lookup-by-email --emails "zhangsan@example.com" --format json-pretty
```

### 参数

| 参数 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `--emails <addresses>` | ✅ | — | 要查找的邮箱地址列表，用英文逗号分隔，最多支持50个邮箱 |

### 使用准则

- **邮箱格式校验**：系统会自动校验邮箱格式，格式不正确时会报错。
- **批量查询效率**：支持最多50个邮箱批量查询，适合批量查找多个用户信息。
- **精确匹配**：按邮箱地址精确匹配用户信息，返回结果包含用户的基本信息和联系方式。
- **隐私保护**：返回结果可能包含用户的详细信息，输出时仅展示用户明确需要的字段。

---

## 常见错误

| 错误现象 | 原因 | 解决方案 |
|---------|------|---------|
| `--username is required` | 缺少必填参数 | 补充 `--username` |
| `--phones is required` | 缺少必填参数 | 补充 `--phones` |
| `--emails is required` | 缺少必填参数 | 补充 `--emails` |
| `user config is empty` | 未登录 | 执行 `tmeet auth login` |
| `invalid phone format` | 手机号格式不正确 | 检查手机号格式 |
| `invalid email format` | 邮箱格式不正确 | 检查邮箱格式 |

## 参考

- [tmeet](../SKILL.md) — 全部命令概览
- [tmeet-meeting](tmeet-meeting.md) — 会议管理（可使用本文档返回的 `openId` 邀请成员）
- [tmeet-report](tmeet-report.md) — 会议报告（**`control kick` 的成员必须从 `report participants` 获取，而非本文档**）
- [tmeet-control](tmeet-control.md) — 会中控制（`call` 可用本文档 `openId`；`kick` 不可）