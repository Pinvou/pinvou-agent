# 读取子表数据 — `wecom-cli sheet ranges get`

根据 `docid`、`sheet_id` 读取**在线表格**指定子表的全部数据。可通过 `mode` 参数选择返回结构化的表格数据（含格式信息），或返回 CSV（内容或文件路径）。

## 命令

```bash
wecom-cli sheet ranges get --json '<JSON 参数>'
```

## 参数

| 字段 | 类型 | 必填 | 默认值 | 语义 |
|---|---|---|---|---|
| `docid` | string | 是 | — | 在线表格 ID |
| `sheet_id` | string | 是 | — | 工作表 ID；通过 `sheet get` 获取 |
| `mode` | string | 否 | `"default"` | 返回格式选择：`"default"` 返回结构化 `grid_data`（含单元格格式）；填 `"csv"` 返回 CSV（内容或文件路径） |
| `range` | string | 条件必填 | — | 当 `mode="default"` 时**必填**，表示要读取的区域，形如 `"A1:A100"`；取值可从 `sheet get` 返回的 `sheets[].data_range` 拿到。`mode="csv"` 时忽略此字段 |

### `mode` 如何选择

默认一律使用 `"default"`，包括普通的读取、查看、展示数据等场景。此时必须同时传 `range`，返回 `grid_data`（含单元格值、格式、数据类型等完整信息）。
仅当用户明确表达需要对数据做统计、计算、聚合分析（例如"求和/平均/分组统计/透视/跑数据分析"等）时，才填 `"csv"`，便于直接把数据交给计算流程。

## 返回

### `mode` 为 `"default"`（默认）

| 字段 | 类型 | 说明 |
|---|---|---|
| `grid_data` | object | 结构化表格数据，包含每个单元格的值（`cell_value`）、格式（`cell_format`，如字体、字号、颜色、对齐方式）和数据类型（`data_type`） |

### `mode` 为 `"csv"`

回包可能是以下两种形式之一（取决于数据大小）：

| 字段 | 类型 | 说明 |
|---|---|---|
| `content` | string | CSV 内容（直接返回） |
| `file_path` | string | CSV 文件落盘后的绝对路径 |

## 使用规则

- `mode="csv"` 且返回 `file_path` 时：必须再用 `read` 工具读取该文件内容，才能展示给用户或继续做分析。
- `mode="csv"` 且返回 `content` 时：可直接消费，无需再次读取文件。
