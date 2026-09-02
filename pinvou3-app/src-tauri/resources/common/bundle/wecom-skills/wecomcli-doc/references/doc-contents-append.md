# 追加内容到在线文档 — `wecom-cli doc contents append`

在**doc文档**的末尾追加新的文本内容，不影响已有内容。

## 命令

```bash
wecom-cli doc contents append --json '<JSON 参数>'
```

## 参数

| 字段 | 类型 | 必填 | 默认值 | 语义                        |
|---|---|---|---|---------------------------|
| `docid` | string | 是 | — | doc文档 ID                  |
| `content` | string | 是 | — | 要追加的文本内容；支持格式：`text`（纯文本） |

## 返回

追加成功返回空对象。
