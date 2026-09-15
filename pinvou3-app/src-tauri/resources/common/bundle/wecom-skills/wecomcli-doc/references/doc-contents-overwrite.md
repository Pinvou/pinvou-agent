# 覆盖在线文档内容 — `wecom-cli doc contents overwrite`

全量覆盖**doc文档**中的所有内容，原有内容将被完全替换。

## 命令

```bash
wecom-cli doc contents overwrite --json '<JSON 参数>'
```

## 参数

| 字段 | 类型 | 必填 | 默认值 | 语义 |
|---|---|---|---|---|
| `docid` | string | 是 | — | 在线文档 ID|
| `content_type` | string | 否 |`markdown` | 内容格式枚举：`text` 纯文本 / `markdown`；通常传 `text` |
| `content` | string | 否 | — | 覆盖写入的完整文本；与 `file_path` 二选一。|
| `file_path` | string | 否 | — | 本地文件路径；与 `content` 二选一，支持通过文件路径覆盖 |

## 返回

覆盖成功返回空对象。

## 使用规则

- **`content` / `file_path` 二选一**：可以直接传入内容，也可以传入文件路径；两者不可同时省略。
- **清空文档不能传空值**：`content` 若传 `null`、空字符串或不传都会被拒；要清空请传 `" "`（一个空格）。
