---
name: wecomcli-doc
description: 何时用:仅当用户明确指向企业微信文档(写个企微文档、读 `https://doc.weixin.qq.com/` 链接)时使用;泛指做个文档默认走本地工具,不要误用。企微文档/表格/智能表格/智能文档管理:文档创建、Markdown 读取与覆写,智能表格创建,智能文档创建与导出,支持 docid 或 URL 定位;智能表格结构管理见 wecomcli-smartsheet。
metadata:
  requires:
    bins: ["wecom-cli"]
  cliHelp: "wecom-cli doc --help"
---

# 企业微信文档管理

管理企业微信文档和智能文档（原名智能主页）的创建、读取和编辑。文档接口支持通过 `docid` 或 `url` 二选一定位文档。

> ⚠️ **重要触发规则**：只有当用户明确提到「**智能文档**」或「**智能主页**」时，才使用智能文档相关接口（`smartpage_*` 系列）。其他所有涉及「文档」的场景（如"创建文档"、"写个文档"、"帮我建个文档"等），一律使用企微文档接口（`create_doc` / `get_doc_content` / `edit_doc_content`）。

## URL 品类识别与接口路由

企业微信文档有四种品类，**URL 格式不同，读取内容所用的接口也不同**，切勿混用。其中**表格（在线表格）与智能表格是两类不同品类**，请通过 URL 严格区分：

| URL 模式 | 品类 | 读取内容接口 |
|---|---|---|
| `https://doc.weixin.qq.com/doc/*` | **文档**（doc_type=3） | `get_doc_content` |
| `https://doc.weixin.qq.com/sheet/*` | **表格 / 在线表格** | `get_doc_content` |
| `https://doc.weixin.qq.com/smartsheet/*` | **智能表格**（doc_type=10） | 参阅 `wecomcli-smartsheet` skill |
| `https://doc.weixin.qq.com/smartpage/*` | **智能文档**（原名智能主页） | `smartpage_export_task` → `smartpage_get_export_result` |

> ⚠️ `/sheet/`（表格）与 `/smartsheet/`（智能表格）是不同品类；表格读取用 `get_doc_content`，智能表格的读取与结构/记录管理见 `wecomcli-smartsheet` skill。

## 调用方式

通过 `wecom-cli` 调用，品类为 `doc`：

```bash
wecom-cli doc <tool_name> '<json_params>'
```

## 返回格式说明

所有接口返回 JSON 对象，包含以下公共字段：

| 字段 | 类型 | 说明 |
|------|------|------|
| `errcode` | integer | 返回码，`0` 表示成功，非 `0` 表示失败 |
| `errmsg` | string | 错误信息，成功时为 `"ok"` |

当 `errcode` 不为 `0` 时，说明接口调用失败，可重试 1 次；若仍失败，将 `errcode` 和 `errmsg` 展示给用户。

### 特殊错误码

| errcode | errmsg | 含义 | 处理方式 |
|---------|--------|------|----------|
| `851002` | `incompatible doc type` | 文档品类与所调用的接口不匹配 | 根据文档 URL 重新确认品类（参见上方「URL 品类识别与接口路由」表），然后使用该品类对应的正确接口重试 |

---

## 文档

### get_doc_content

获取**文档 / 表格（在线表格）**的完整内容数据，统一以 Markdown 格式返回。采用**异步轮询机制**：首次调用无需传 `task_id`，接口返回 `task_id`；若 `task_done` 为 false，需携带该 `task_id` 再次调用，直到 `task_done` 为 true 时返回完整内容。

> 适用 URL：`/doc/*`、`/sheet/*`。`/smartsheet/*`（智能表格）不适用，请改用 `wecomcli-smartsheet` skill；`/smartpage/*`（智能文档）不适用，请改用 `smartpage_export_task`。

- 首次调用（不传 task_id）：
```bash
wecom-cli doc get_doc_content '{"docid": "DOCID", "type": 2}'
```
- 轮询（携带上次返回的 task_id）：
```bash
wecom-cli doc get_doc_content '{"docid": "DOCID", "type": 2, "task_id": "xxx"}'
```
- 通过 URL 读取文档：
```bash
wecom-cli doc get_doc_content '{"url": "https://doc.weixin.qq.com/doc/xxx", "type": 2}'
```
- 通过 URL 读取表格（在线表格）：
```bash
wecom-cli doc get_doc_content '{"url": "https://doc.weixin.qq.com/sheet/xxx", "type": 2}'
```

> ⚠️ **智能表格（`/smartsheet/*`）不适用此接口**，请使用 `wecomcli-smartsheet` skill 进行操作。

参见 [API 详情](references/get-doc-content.md)。

### create_doc

新建文档（doc_type=3）或智能表格（doc_type=10）。创建成功返回 url 和 docid。

- 创建文档：
```bash
wecom-cli doc create_doc '{"doc_type": 3, "doc_name": "项目周报"}'
```
- 创建智能表格：
```bash
wecom-cli doc create_doc '{"doc_type": 10, "doc_name": "项目任务表"}'
```

**注意**：
- docid 仅在创建时返回，需妥善保存
- 智能表格（doc_type=10）的详细管理功能（子表、字段、数据记录等）已迁移到 `wecomcli-smartsheet` skill，请使用该 skill 进行高级操作
- 普通表格（在线表格，URL 含 `/sheet/`）本 skill **仅支持读取**（通过 `get_doc_content`），不支持创建

参见 [API 详情](references/create-doc.md)。

### edit_doc_content

用 Markdown 内容覆写文档正文。`content_type` 固定为 `1`（Markdown）。

```bash
wecom-cli doc edit_doc_content '{"docid": "DOCID", "content": "# 标题\n\n正文内容", "content_type": 1}'
```

参见 [API 详情](references/edit-doc-content.md)。

---

## 智能文档（原名智能主页）

适用品类：智能文档（用户说「智能文档」或「智能主页」时触发）
适用 URL：`/smartpage/*`

适用场景：
1. 将本地 Markdown 文件创建为智能文档
2. 异步导出智能文档内容为 Markdown

### smartpage_create

创建智能文档（原名智能主页），支持传入标题和多个子页面。每个子页面可指定标题、内容类型和本地文件路径。创建成功返回 docid 和 url。

> ⚠️ **特殊语法**：此命令必须使用 `+smartpage_create`（带 `+` 前缀），加号不可省略；不要自行给其他 `doc` 子命令加 `+`，带 `+` 的命令（如此命令及 smartsheet 的 `+smartsheet_add_records_auto_file` / `+smartsheet_update_records_auto_file`）必须原样保留加号。

```bash
wecom-cli doc +smartpage_create '{"title": "项目概览", "pages": [{"page_title": "需求文档", "content_type": 1, "page_filepath": "/path/to/requirements.md"}]}'
```

**注意**：
- `content_type` **必须与文件实际内容匹配**：`.md` 文件或包含 Markdown 语法的内容必须传 `1`（Markdown），仅纯文本才传 `0`。绝大多数场景应传 `1`
- docid 仅在创建时返回，需妥善保存
- 每个子页面的 Markdown 文件大小不得超过 **10MB**，超过会导致创建失败。如果文件过大，需先拆分为多个子页面再创建

参见 [API 详情](references/smartpage-create.md)。

### smartpage_export_task

发起智能文档内容导出任务（异步）。传入 docid（或 url）和 content_type，返回 task_id。这是异步导出的第一步，需配合 `smartpage_get_export_result` 轮询获取导出结果。

- 通过 docid：
```bash
wecom-cli doc smartpage_export_task '{"docid": "DOCID", "content_type": 1}'
```
- 或通过 URL：
```bash
wecom-cli doc smartpage_export_task '{"url": "https://doc.weixin.qq.com/smartpage/xxx", "content_type": 1}'
```

参见 [API 详情](references/smartpage-export.md)。

### smartpage_get_export_result

查询智能文档导出任务进度。传入 task_id 进行轮询，当 `task_done` 为 `true` 时返回 `content`（导出的完整文档内容）。

```bash
wecom-cli doc smartpage_get_export_result '{"task_id": "TASK_ID"}'
```

当 `task_done` 为 `true` 时，`content` 字段即为导出的 Markdown 内容。

参见 [API 详情](references/smartpage-export.md)。

## 典型工作流
- 读内容：先按 URL 表定品类 → `/doc/`、`/sheet/` 走 `get_doc_content`（轮询至 `task_done`）；`/smartpage/` 走 `smartpage_export_task` → `smartpage_get_export_result`。
- 建文档：`create_doc`（doc_type=3），保存返回的 docid（仅创建时返回）。
- 改文档：先 `get_doc_content` 了解现状，再 `edit_doc_content` 覆写。

