---
name: lark-task
version: 1.0.0
description: "【何时用:仅当用户明确指向飞书/Lark(发到飞书、飞书文档等)时使用;泛指做个文档或PPT或表格或方案默认走本地工具,不要误用飞书】飞书任务：管理任务、清单和任务智能体。创建待办任务、查看和更新任务状态、拆分子任务、组织任务清单、分配协作成员、上传任务附件、注册或注销任务智能体、更新任务智能体的主页数据、写入智能体任务记录。当用户需要创建待办事项、查看任务列表、跟踪任务进度、管理项目清单或给他人分配任务、为任务上传附件文件、注册注销任务智能体、更新智能体主页数据、写入任务记录时使用。"
metadata:
  requires:
    bins: ["lark-cli"]
  cliHelp: "lark-cli task --help"
---

# task (v2)

**CRITICAL — 开始前 MUST 先用 `File(action="read")` 读取 [`../lark-shared/SKILL.md`](../lark-shared/SKILL.md)，其中包含认证、权限处理**

> **搜索 vs 列表**：有真实查询关键字（任务名/清单名/片段）才用 `+search` / `+tasklist-search`；只有范围条件（“今年以来”“已完成”“我关注的”“由我创建”）时用列表型——任务用 `+get-related-tasks`（与我相关/我关注/我创建）或 `+get-my-tasks`（分配给我），清单用原生 `tasklists.list` 后本地筛选。**不要把时间范围词当 query**。
> **用户身份识别**：在用户身份（user identity）场景下，如果用户提到了“我”（例如“分配给我”、“由我创建”），请默认获取当前登录用户的 `open_id` 作为对应的参数值（可用 `lark-cli whoami` 获取）。
> **术语理解 — 待办 disambiguation（必读）**：
> - 用户提到「待办 / todo / 任务」时，**先判断归属**，不要默认走本 skill。
> - **走妙记（禁止本 skill）**：上下文含妙记/会议纪要/minute_token/`/minutes/` URL 时，直接用 `lark-cli minutes +todo` 系列命令，**不要**调任何 `task` 命令去“找清单再放任务”。
> - **走本 skill**：任务清单、分配给我、截止日期/提醒、子任务；applink 含 `client/todo/task?guid=`；或明确说“飞书任务/任务中心/我的任务清单”。
> **友好输出**：在输出任务（或清单）的执行结果给用户时，建议同时提取并输出命令返回结果中的 `url` 字段（任务链接），以便用户可以直接点击跳转查看详情。

> **创建/更新注意**：
> 1. 只有在设置了 `due`（截止时间）的情况下，才能设置 `repeat_rule`（重复规则）和 `reminder`（提醒时间）。
> 2. 若同时设置了 `start`（开始时间）和 `due`（截止时间），开始时间必须小于或等于截止时间。
> 3. 使用 tenant_access_token（应用身份）时，无法跨租户添加任务成员。

> **查询注意**：
> 1. 在输出任务详情时，如果需要渲染负责人、创建人等人员字段，除了展示 `id` (例如 open_id) 外，还必须通过其他方式（例如调用通讯录技能）尝试获取并展示这个人的真实名字，以便用户更容易识别。
> 2. 在输出清单详情时，如果需要渲染 owner、member、角色成员等人员字段，也必须像任务成员展示一样，除了展示 `id` 外，尽量解析并展示对应人员的真实名字。
> 3. 在输出任务或清单详情时，如果需要渲染创建时间、截止时间等字段，需要使用本地时区来渲染（格式 YYYY-MM-DD HH:MM:SS）。

> **Task GUID 定义**：
> Task OpenAPI 中用于更新/操作任务的 `guid` 是任务的全局唯一标识（GUID），不是客户端展示的任务编号（例如 `t104121` / `suite_entity_num`）。
> 对于 Feishu 的任务 applink（例如 `.../client/todo/task?guid=...`），必须使用 URL query 里的 `guid` 参数作为 task guid。

| Shortcut | 说明 |
|----------|------|
| [`+create`](references/lark-task-create.md) | create a task |
| [`+update`](references/lark-task-update.md) | update task attributes |
| [`+set-ancestor`](references/lark-task-set-ancestor.md) | set or clear a task ancestor |
| [`+comment`](references/lark-task-comment.md) | add a comment to a task |
| [`+complete`](references/lark-task-complete.md) | mark a task as complete |
| [`+reopen`](references/lark-task-reopen.md) | reopen a completed task |
| [`+assign`](references/lark-task-assign.md) | assign or remove task members |
| [`+followers`](references/lark-task-followers.md) | manage task followers |
| [`+reminder`](references/lark-task-reminder.md) | manage task reminders |
| [`+get-my-tasks`](references/lark-task-get-my-tasks.md) | List tasks assigned to me |
| [`+get-related-tasks`](references/lark-task-get-related-tasks.md) | list tasks related to me |
| [`+search`](references/lark-task-search.md) | search tasks |
| [`+upload-attachment`](references/lark-task-upload-attachment.md) | upload a local file as an attachment to a task |
| [`+tasklist-create`](references/lark-task-tasklist-create.md) | create a tasklist and optionally add tasks |
| [`+tasklist-search`](references/lark-task-tasklist-search.md) | search tasklists |
| [`+tasklist-task-add`](references/lark-task-tasklist-task-add.md) | add tasks to a tasklist |
| [`+tasklist-members`](references/lark-task-tasklist-members.md) | manage tasklist members |

## API Resources

```bash
lark-cli schema task.<resource>.<method>   # 调用 API 前必须先查看参数结构
lark-cli task <resource> <method> [flags] # 调用 API
```

> **重要**：使用原生 API 时，必须先运行 `schema` 查看 `--data` / `--params` 参数结构，不要猜测字段格式。

### tasks

  - `create` — 创建任务
  - `delete` — 删除任务
  - `get` — 获取任务详情
  - `list` — 获取任务列表
  - `patch` — 更新任务

### tasklists

  - `add_members` — 添加清单成员
  - `create` — 创建清单
  - `delete` — 删除清单
  - `get` — 获取清单详情
  - `list` — 获取清单列表
  - `patch` — 更新清单
  - `remove_members` — 移除清单成员
  - `tasks` — 获取清单任务列表

### subtasks

  - `create` — 创建子任务
  - `list` — 获取任务的子任务列表

### members

  - `add` — 添加任务成员
  - `remove` — 移除任务成员

### sections

  - `create` — 创建自定义分组
  - `delete` — 删除自定义分组
  - `get` — 获取自定义分组详情
  - `list` — 获取自定义分组列表
  - `patch` — 更新自定义分组
  - `tasks` — 获取自定义分组任务列表

### custom_fields

  - `create` — 创建自定义字段
  - `get` — 获取自定义字段详情
  - `patch` — 更新自定义字段
  - `list` — 获取自定义字段列表
  - `add` — 将自定义字段加入资源
  - `remove` — 将自定义字段移出资源

### custom_field_options

  - `create` — 创建自定义字段选项
  - `patch` — 更新自定义字段选项

### agent

  - `update_agent_profile` — 更新任务代理的主页内容数据。
  - `register_agent` — 注册AI 智能体

### agent_task_step_info

  - `append_task_steps` — 写入任务记录。

## 权限表

| 方法 | 所需 scope |
|------|-----------|
| `tasks.create` | `task:task:write` |
| `tasks.delete` | `task:task:write` |
| `tasks.get` | `task:task:read` |
| `tasks.list` | `task:task:read` |
| `tasks.patch` | `task:task:write` |
| `tasklists.add_members` | `task:tasklist:write` |
| `tasklists.create` | `task:tasklist:write` |
| `tasklists.delete` | `task:tasklist:write` |
| `tasklists.get` | `task:tasklist:read` |
| `tasklists.list` | `task:tasklist:read` |
| `tasklists.patch` | `task:tasklist:write` |
| `tasklists.remove_members` | `task:tasklist:write` |
| `tasklists.tasks` | `task:tasklist:read` |
| `subtasks.create` | `task:task:write` |
| `subtasks.list` | `task:task:read` |
| `members.add` | `task:task:write` |
| `members.remove` | `task:task:write` |
| `sections.create` | `task:section:write` |
| `sections.delete` | `task:section:write` |
| `sections.get` | `task:section:read` |
| `sections.list` | `task:section:read` |
| `sections.patch` | `task:section:write` |
| `sections.tasks` | `task:section:read` |
| `custom_fields.create` | `task:custom_field:write` |
| `custom_fields.get` | `task:custom_field:read` |
| `custom_fields.patch` | `task:custom_field:write` |
| `custom_fields.list` | `task:custom_field:read` |
| `custom_fields.add` | `task:custom_field:write` |
| `custom_fields.remove` | `task:custom_field:write` |
| `custom_field_options.create` | `task:custom_field:write` |
| `custom_field_options.patch` | `task:custom_field:write` |
| `agent.update_agent_profile` | `task:task:write` |
| `agent.register_agent` | `task:task:write` |
| `agent_task_step_info.append_task_steps` | `task:task:write` |
