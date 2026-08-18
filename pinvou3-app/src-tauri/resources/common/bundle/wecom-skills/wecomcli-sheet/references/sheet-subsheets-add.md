# 添加子工作表 — `wecom-cli sheet subsheets add`

向**在线表格**添加一个新的子工作表。

## 命令

```bash
wecom-cli sheet subsheets add --json '<JSON 参数>'
```

## 参数

| 字段 | 类型 | 必填 | 默认值 | 语义 |
|---|---|---|---|---|
| `docid` | string | 是 | — | 在线表格 ID |
| `sheet` | object | 是 | — | 子表信息 |
| `sheet.title` | string | 是 | — | 工作表名称 |
| `sheet.row_count` | int | 否 | — | 表格总行数 |
| `sheet.column_count` | int | 否 | — | 表格总列数 |
| `index` | int | 否 | — | 插入位置：`0` 表示插入到最后，`1` 表示插入到第一个位置 |

## 返回

| 字段 | 类型 | 说明 |
|---|---|---|
| `sheet` | object | 新增的子表信息；含 `sheet_id`（唯一标识）/ `title` / `row_count` / `column_count` / `data_range`（新建时为空） |
