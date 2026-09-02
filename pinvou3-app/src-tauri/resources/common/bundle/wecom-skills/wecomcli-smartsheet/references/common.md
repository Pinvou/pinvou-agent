# 智能表格文件级公共接口参考

本文件聚焦智能表格（`smartsheet`）的**文件级创建与导入接口**；重命名、搜索、成员管理、加入规则与未读管理请使用 `wecomcli-doc-manage` 技能。

---

## 场景导航

| 用户场景/意图 | 对应命令 |
| --- | --- |
| 新建智能表格 | `smartsheet create` |
| 导入本地/已上传文件（.csv/.xls/.xlsx）为智能表格 | `smartsheet import` |
| 删除智能表格文件 | 不支持删除智能表格文件，请直接告知用户 |
| 修改智能表格名称 | 使用 `wecomcli-doc-manage` 技能 |
| 搜索智能表格 / 按名称查找 / 查看最近浏览或创建的智能表格 | 使用 `wecomcli-doc-manage` 技能 |
| 向智能表格添加管理员、可编辑或仅浏览成员 | 使用 `wecomcli-doc-manage` 技能 |
| 设置通过链接加入智能表格时的权限规则 | 使用 `wecomcli-doc-manage` 技能 |
| 读取与我相关中未读的文档列表 | 使用 `wecomcli-doc-manage` 技能 |
| 将与我相关中的文档标记为已读或未读 | 使用 `wecomcli-doc-manage` 技能 |

---

## 接口说明

### 一、新建智能表格（smartsheet create）

新建一个智能表格文档，可选择传入初始表结构定义。

> **新建文档 vs 导入文档——如何正确选择：**
> - **`smartsheet create`（本接口）**：用于**从零新建**一个企微智能表格。推荐在创建时通过 `sheet_title` + `fields` 一次性初始化子表字段；也支持创建空白智能表格后再调整。**本接口不支持文件导入类参数**（如 `media_id`），所有涉及文件导入的场景请使用 `smartsheet import`。
> - **`smartsheet import`（导入接口）**：用于将一个**已上传并获得 `media_id` 的文件**（如 `.csv`、`.xlsx` 等）导入为智能表格。适用于用户已经提供文件 `media_id`，或用户明确表达「导入」文件意图的场景。若用户只提供本地文件路径，必须先使用 `wecomcli-media` 技能上传文件获取 `media_id`，再调用本接口。

**命令示例：**

创建智能表格并一次性初始化子表字段（推荐）：

```bash
wecom-cli smartsheet create --json '{"name": "任务跟踪表", "sheet_title": "任务列表", "fields": [{"field_title": "任务名称", "field_type": "text"}, {"field_title": "优先级", "field_type": "single_select", "property_single_select": {"is_quick_add": true, "options": [{"text": "高", "style": 18}, {"text": "中", "style": 20}, {"text": "低", "style": 16}]}}, {"field_title": "负责人", "field_type": "user", "property_user": {"is_multiple": false, "is_notified": true}}]}'
```

**（强制）传入 `fields` 创建字段后，须根据传入的 `field_title`，按 `references/smart-sheet-view-types.md` 中「新建字段时的列宽判断规则」和「列宽调整接口调用方式」完成列宽写入。**

创建空白智能表格：

```bash
wecom-cli smartsheet create --json '{"name": "任务跟踪表"}'
```

**通用参数：**

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `name` | string | 是 | 文档标题 |

**智能表格专用参数：**

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `sheet_title` | string | 否 | 子表名称 |
| `fields` | array | 否 | 初始化字段列表。创建空智能表格时无需填写 |
| `fields[].field_title` | string | 否 | 字段标题 |
| `fields[].field_type` | string | 否 | 字段类型，完整枚举详见 `references/smart-sheet-field-types.md` |

**返回值：**

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `errcode` | int | 错误码，0 表示成功 |
| `errmsg` | string | 错误信息 |
| `url` | string | 新建文档的访问链接 |
| `docid` | string | 新建文档的 ID |
| `doc_name` | string | 新建文档的名称 |

---

### 二、导入文档为智能表格（smartsheet import）

将已上传的文件（`.csv`、`.xls`、`.xlsx`）导入为企微智能表格。

**命令示例：**

```bash
wecom-cli smartsheet import --json '{"name": "data.xlsx", "media_id": "mcabc123"}'
```

**参数说明：**

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `name` | string | 是 | 二进制文件名（含后缀），用于业务判断文件类型 |
| `passwd` | string | 否 | 若 office 文件有加密，传入用户输入的密码 |
| `media_id` | string | 是 | 已上传文件的媒体 ID，前缀通常为 `mc`。只能来自 `wecomcli-media` 的上传接口或其他上游接口返回，禁止自行构造、猜测或从本地路径推断 |

**返回值：**

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `errcode` | int | 网关错误码；`0` 表示请求成功（任务本身是否成功看 `task_status`） |
| `errmsg` | string | 网关错误信息；失败时用于排查 |
| `task_id` | string | 导入任务 ID；任务异步执行时可用于追踪导入进度 |
| `task_status` | string | 导入任务状态枚举：`succ`（成功）/ `fail`（失败）/ `processing`（处理中） |
| `docid` | string | 导入成功后的文档 ID；`task_status=succ` 时返回 |
| `url` | string | 导入成功后的文档访问链接；`task_status=succ` 时返回 |

---

## 注意事项

- **获取 docid**：统一遵循 `SKILL.md` 的「如何获取文档 ID（docid）」规则
- **创建时一次性初始化字段**：新建智能表格时，优先在 `smartsheet create` 直接传 `sheet_title` + `fields` 完成子表字段初始化，避免拆成"创建后再补字段"两步
- **导入只接受 `media_id`**：执行 `smartsheet import` 前，需先通过 `wecomcli-media` 上传文件并取得 `media_id`，再发起导入
- **参数不全时必须主动补全**：当必填参数缺失时，禁止猜测或使用默认值，必须用简洁自然语言向用户追问缺失的参数

---

## 参数补全（跨技能协作）

当用户提供的信息不足以完成操作时（如缺少必填参数），**应直接用简洁自然语言引导用户补全缺失的信息；有候选项时在文字中列出，不得自行猜测默认值。**

### 何时触发？

当用户发起智能表格文件级操作的意图，但以下任一必填信息缺失时，触发参数补全：

| 缺失信息 | 对应接口/字段 | 示例用户表述 |
| --- | --- | --- |
| 文档名称 | `name` | "帮我建个智能表格"（没说叫什么名字） |
| 新名称 | 查看 `wecomcli-doc-manage` 技能 | "帮我改一下智能表格名"（没说改成什么） |
| 成员信息 | 查看 `wecomcli-doc-manage` 技能 | "帮我给智能表格加个人"（没说加谁、什么权限） |
| 搜索关键词 | 查看 `wecomcli-doc-manage` 技能 | "帮我搜一下智能表格"（没说搜什么关键词） |
| 浏览时间范围 | 查看 `wecomcli-doc-manage` 技能 | "看看我最近浏览了哪些智能表格"（没说时间范围）。需在文字中列出"最近一周"、"最近一个月"、"自定义时间范围"等选项 |
| 创建时间范围 | 查看 `wecomcli-doc-manage` 技能 | "看看我最近创建的智能表格"（没说时间范围）。需在文字中列出"最近一周"、"最近一个月"、"自定义时间范围"等选项 |
| 目标文档和操作类型 | 查看 `wecomcli-doc-manage` 技能 | "帮我标记智能表格已读"（没说哪些文档） |

### 正确做法

1. 分析用户已提供的信息，确定哪些必填参数缺失
2. **用简洁自然语言仅对缺失或有歧义的参数进行提问**（用户已明确的参数不要重复问）；有候选项时在文字中列出供用户选择
3. 收到用户回答后，组装完整的入参，执行操作

### 禁止事项

- ❌ 参数缺失时自行猜测默认值（如随意假设文档名称或目标文档）
- ❌ 用户已明确的参数还重复提问
