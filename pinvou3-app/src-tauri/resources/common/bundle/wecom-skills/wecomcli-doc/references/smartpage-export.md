# smartpage_export_task / smartpage_get_export_result API

导出智能文档（原智能主页）内容。采用异步两步操作：先通过 `smartpage_export_task` 提交导出任务获取 `task_id`，再通过 `smartpage_get_export_result` 轮询任务状态，直到任务完成后返回完整文档内容。

---

## 第一步：smartpage_export_task — 提交导出任务

发起智能文档内容导出任务（异步）。传入 `docid` 或 `url` 和 `content_type`，返回 `task_id`。

### 技能定义

```json
{
    "name": "smartpage_export_task",
    "description": "发起智能文档（原智能主页）内容导出任务（异步）。传入 docid（或 url）和 content_type，返回 task_id。需配合 smartpage_get_export_result 轮询查询导出进度，直到任务完成后获取文档内容。",
    "inputSchema": {
        "properties": {
            "docid": {
                "description": "智能文档的 docid，与 url 二选一传入",
                "title": "Doc ID",
                "type": "string"
            },
            "url": {
                "description": "智能文档的访问链接，与 docid 二选一传入",
                "title": "URL",
                "type": "string"
            },
            "content_type": {
                "description": "导出内容格式。目前仅支持 1（Markdown 格式）",
                "enum": [1],
                "title": "Content Type",
                "type": "integer"
            }
        },
        "oneOf": [
            { "required": ["docid", "content_type"] },
            { "required": ["url", "content_type"] }
        ],
        "title": "smartpage_export_taskArguments",
        "type": "object"
    }
}
```

### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| docid | string | 与 url 二选一 | 智能文档的 docid |
| url | string | 与 docid 二选一 | 智能文档的访问链接 |
| content_type | integer | 是 | 导出内容格式，目前仅支持 `1`（Markdown 格式） |

### 请求示例

```json
// 通过 docid
{
    "docid": "DOCID",
    "content_type": 1
}

// 通过 url
{
    "url": "https://doc.weixin.qq.com/smartpage/a1_xxxxxx",
    "content_type": 1
}
```

### 响应示例

```json
{
    "errcode": 0,
    "errmsg": "ok",
    "task_id": "TASK_ID"
}
```

---

## 第二步：smartpage_get_export_result — 查询导出结果

查询智能文档导出任务进度。传入 `task_id` 进行轮询，当 `task_done` 为 `true` 时返回完整文档内容。

### 技能定义

```json
{
    "name": "smartpage_get_export_result",
    "description": "查询智能文档（原智能主页）导出任务进度。传入 task_id 轮询，当 task_done 为 true 时返回 content 字段，包含导出的完整文档内容。",
    "inputSchema": {
        "properties": {
            "task_id": {
                "description": "导出任务 ID，由 smartpage_export_task 返回",
                "title": "Task ID",
                "type": "string"
            }
        },
        "required": ["task_id"],
        "title": "smartpage_get_export_resultArguments",
        "type": "object"
    }
}
```

### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| task_id | string | 是 | 导出任务 ID，由 `smartpage_export_task` 返回 |

### 请求示例

```json
{
    "task_id": "TASK_ID"
}
```

### 响应示例

任务未完成：

```json
{
    "errcode": 0,
    "errmsg": "ok",
    "task_done": false
}
```

任务完成：

```json
{
    "errcode": 0,
    "errmsg": "ok",
    "task_done": true,
    "content": "# 项目周报\n\n## 本周进展\n\n1. 完成了用户模块开发\n2. 修复了3个线上Bug"
}
```

---

## 异步轮询机制

1. **调用 smartpage_export_task**：传入 `docid`（或 `url`）和 `content_type: 1`，获取 `task_id`
2. **首次轮询**：传入 `task_id` 调用 `smartpage_get_export_result`
3. **检查响应**：若 `task_done` 为 `false`，继续轮询
4. **获取内容**：当 `task_done` 为 `true` 时，`content` 字段包含完整的 Markdown 内容

## 注意事项

- `smartpage_export_task` 是异步操作的第一步，调用后仅返回 `task_id`
- `content_type` 目前仅支持 `1`（Markdown 格式）
- `docid` 和 `url` 二选一传入即可，无需同时传入
- 任务完成后 `content` 字段直接包含完整文档内容，无需额外读取文件
- 如果轮询多次仍未完成，建议适当增加轮询间隔
