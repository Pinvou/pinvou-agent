# 修改文档名 — `wecom-cli doc names update`

修改指定文档的名称，通过 `docid` 统一操作。

## 命令

```bash
wecom-cli doc names update --json '<JSON 参数>'
```

## 参数

| 字段 | 类型 | 必填 | 默认值 | 语义 |
|---|---|---|---|---|
| `docid` | string | 是 | — | 要改名的文档 ID |
| `new_name` | string | 是 | — | 新的文档名称 |

## 返回

改名成功返回空对象。
