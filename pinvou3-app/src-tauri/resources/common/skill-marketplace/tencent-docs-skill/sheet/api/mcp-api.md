# 腾讯文档 Sheet MCP 工具完整参考

本文件包含腾讯文档 Sheet MCP 所有工具的通用 API 说明、详细调用示例、参数说明和返回值说明。

---

## 通用说明

### 公共参数

所有工具都包含以下公共参数：
- `file_id` (string, 通常必填): 文档唯一标识符；支持 `file_url` 的工具可与 `file_url` 二选一
- `sheet_id` (string, 必填): 子表 ID（部分工具不需要或可选，详见各工具说明）

部分工具支持 `file_url` 与 `file_id` 二选一，具体以各工具参数说明为准。

### 响应结构

除单个工具另有返回值说明外，API 成功时返回空对象 `{}`，失败时会抛出对应错误信息。

## 工具调用示例

## 1. sheet.set_cell_value

### 功能说明
设置在线表格指定单元格的值，支持文本、数字、布尔、公式等类型（SHEET）。

> 💡 **建议**：单次写入操作的请求体内容尽量不超过 **1MB**，超大内容请拆分为多次写入。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "cell": {
    "row": 0,
    "col": 0,
    "value_type": "STRING",
    "string_value": "Hello World"
  }
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `cell` (object, 必填): 单元格值参数
  - `row` (int64, 必填): 行索引（0-based）
  - `col` (int64, 必填): 列索引（0-based）
  - `value_type` (string, 必填): 值类型，可选值：`STRING`、`NUMBER`、`BOOL`、`FORMULA`
  - `number_value` (double, 可选): 数值，`value_type` 为 `NUMBER` 时使用
  - `string_value` (string, 可选): 字符串值，`value_type` 为 `STRING` 时使用
  - `bool_value` (bool, 可选): 布尔值，`value_type` 为 `BOOL` 时使用
  - `formula` (string, 可选): 公式，`value_type` 为 `FORMULA` 时使用，例如 `"=SUM(A1:A10)"`

### 返回值说明
```json
{}
```

---

## 2. sheet.set_range_value

### 功能说明
批量设置在线表格多个单元格的值（SHEET）。

> 💡 **建议**：单次写入操作的请求体内容尽量不超过 **1MB**（大约几千个单元格，视单元格内容长度而定），超大批量请拆分为多次写入。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "values": [
    {
      "row": 0,
      "col": 0,
      "value_type": "STRING",
      "string_value": "Name"
    },
    {
      "row": 0,
      "col": 1,
      "value_type": "STRING",
      "string_value": "Score"
    },
    {
      "row": 1,
      "col": 0,
      "value_type": "STRING",
      "string_value": "Alice"
    },
    {
      "row": 1,
      "col": 1,
      "value_type": "NUMBER",
      "number_value": 95.5
    }
  ]
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `values` (array, 必填): 单元格值列表，每个元素与 `set_cell_value` 的 `cell` 参数结构相同

### 返回值说明
```json
{}
```

---

## 3. sheet.set_cell_style

### 功能说明
设置在线表格指定范围单元格的样式，包括字体、颜色、对齐等（SHEET）。同时支持三种清除场景：
1. 顶层 `is_clear=true`：清除区域内所有样式（字体/对齐/背景/数字格式等），此时 `format` 字段被忽略。
2. `format` 内字段级清除：`bool` 字段传 `false`、`string` 字段传空字符串 `""`、`int32` 字段传 `0`，均表示清除该单个属性。
3. 未传字段：保持原值不变。

### 调用示例

#### 示例 1：设置样式
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "start_row": 0,
  "start_col": 0,
  "end_row": 5,
  "end_col": 3,
  "format": {
    "bold": true,
    "italic": false,
    "font_size": 12,
    "font_color": "FF000000",
    "bg_color": "FFFFFF00",
    "horizontal_align": "center",
    "vertical_align": "center",
    "wrap_text": true
  }
}
```

#### 示例 2：整体清除区域样式
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "start_row": 0,
  "start_col": 0,
  "end_row": 5,
  "end_col": 3,
  "is_clear": true
}
```

#### 示例 3：只清除背景色 + 取消加粗
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "start_row": 0,
  "start_col": 0,
  "end_row": 5,
  "end_col": 3,
  "format": {
    "bold": false,
    "bg_color": ""
  }
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `start_row` (int64, 必填): 起始行索引（0-based）
- `start_col` (int64, 必填): 起始列索引（0-based）
- `end_row` (int64, 必填): 结束行索引
- `end_col` (int64, 必填): 结束列索引
- `is_clear` (bool, 可选): 若为 true 则清除区域内单元格的所有样式，同时忽略 `format` 字段
- `format` (object, 可选): 样式参数对象（`is_clear=true` 时被忽略）
  - `bold` (bool, 可选): 是否粗体；传 `false` 表示清除粗体
  - `italic` (bool, 可选): 是否斜体；传 `false` 表示清除斜体
  - `font_family` (string, 可选): 字体名称；传 `""` 表示清除字体
  - `font_size` (int32, 可选): 字号（pt）；传 `0` 表示清除字号
  - `font_color` (string, 可选): 字体颜色，ARGB hex，如 `"FF000000"`；传 `""` 表示清除字体颜色
  - `bg_color` (string, 可选): 背景色，ARGB hex，如 `"FFFFFFFF"`；传 `""` 表示清除背景色
  - `horizontal_align` (string, 可选): 水平对齐：`general` / `left` / `center` / `right` / `fill` / `justify`；传 `""` 表示清除水平对齐
  - `vertical_align` (string, 可选): 垂直对齐：`top` / `center` / `bottom` / `justify`；传 `""` 表示清除垂直对齐
  - `wrap_text` (bool, 可选): 是否自动换行；传 `false` 表示清除自动换行
  - `strike_through` (bool, 可选): 是否删除线；传 `false` 表示清除删除线
  - `underline` (string, 可选): 下划线类型：`none` / `single` / `double` / `single_accounting` / `double_accounting`；传 `""` 表示清除下划线
  - `number_format_pattern` (string, 可选): 数字格式，如 `"0.00%"`；传 `""` 表示清除数字格式

### 返回值说明
```json
{}
```

---

## 4. sheet.merge_cell

### 功能说明
合并在线表格指定范围的单元格，支持全部合并、按行合并、按列合并（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "start_row": 0,
  "start_col": 0,
  "end_row": 3,
  "end_col": 3,
  "merge_type": "all"
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `start_row` (int64, 必填): 起始行索引（0-based）
- `start_col` (int64, 必填): 起始列索引（0-based）
- `end_row` (int64, 必填): 结束行索引
- `end_col` (int64, 必填): 结束列索引
- `merge_type` (string, 必填): 合并类型
  - `"all"`: 全部合并（默认）
  - `"columns"`: 按列合并
  - `"rows"`: 按行合并

### 返回值说明
```json
{}
```

---

## 5. sheet.insert_dimension

### 功能说明
在在线表格指定位置插入行或列（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "dimension_type": "row",
  "index": 2,
  "count": 3,
  "direction": "before"
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `dimension_type` (string, 必填): 行列类型：`"row"` | `"col"`
- `index` (int64, 必填): 起始索引（0-based）
- `count` (int64, 必填): 插入数量
- `direction` (string, 可选): 插入方向：`"before"`（默认）| `"after"`

### 返回值说明
```json
{}
```

---

## 6. sheet.delete_dimension

### 功能说明
删除在线表格指定位置的行或列（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "dimension_type": "col",
  "index": 3,
  "count": 2
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `dimension_type` (string, 必填): 行列类型：`"row"` | `"col"`
- `index` (int64, 必填): 起始索引（0-based）
- `count` (int64, 必填): 删除数量

### 返回值说明
```json
{}
```

---

## 7. sheet.set_freeze

### 功能说明
设置在线表格的冻结行列数，传 0 可取消冻结（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "row_count": 1,
  "col_count": 2
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `row_count` (int64, 必填): 冻结行数（0 = 取消冻结行）
- `col_count` (int64, 必填): 冻结列数（0 = 取消冻结列）

### 返回值说明
```json
{}
```

---

## 8. sheet.set_filter

### 功能说明
为在线表格指定数据区域设置筛选（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "start_row": 0,
  "start_col": 0,
  "end_row": 100,
  "end_col": 5
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `start_row` (int64, 必填): 数据区域起始行（0-based）
- `start_col` (int64, 必填): 数据区域起始列（0-based）
- `end_row` (int64, 必填): 数据区域结束行
- `end_col` (int64, 必填): 数据区域结束列
- `filter_id` (string, 可选): 筛选 ID（不传则自动生成）

### 返回值说明
```json
{}
```

---

## 9. sheet.remove_filter

### 功能说明
移除在线表格的筛选，可按筛选 ID 精确移除或移除全部（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "filter_id": "filter_001"
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `filter_id` (string, 可选): 筛选 ID（不传则移除该子表所有筛选）

### 返回值说明
```json
{}
```

---

## 10. sheet.set_link

### 功能说明
为在线表格指定单元格设置超链接，可指定链接 URL 和显示文本（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "row": 0,
  "col": 0,
  "url": "https://docs.qq.com",
  "display_text": "腾讯文档"
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `row` (int64, 必填): 单元格行（0-based）
- `col` (int64, 必填): 单元格列（0-based）
- `url` (string, 必填): 超链接 URL
- `display_text` (string, 可选): 单元格显示文本

### 返回值说明
```json
{}
```

---

## 11. sheet.clear_link

### 功能说明
清除在线表格指定单元格的超链接，可按链接 ID 精确清除或清除全部超链接（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "row": 0,
  "col": 0,
  "link_id": "link_001"
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `row` (int64, 必填): 单元格行（0-based）
- `col` (int64, 必填): 单元格列（0-based）
- `link_id` (string, 可选): 链接 ID（不传则按位置清除）

### 返回值说明
```json
{}
```

---

## 12. sheet.unmerge_cell

### 功能说明
取消在线表格指定区域的单元格合并（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "start_row": 0,
  "start_col": 0,
  "end_row": 3,
  "end_col": 3
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `start_row` (int64, 必填): 起始行索引（0-based）
- `start_col` (int64, 必填): 起始列索引（0-based）
- `end_row` (int64, 必填): 结束行索引
- `end_col` (int64, 必填): 结束列索引

### 返回值说明
```json
{}
```

---

## 13. sheet.clear_range_cells

### 功能说明
清除在线表格指定区域内所有单元格的内容，不影响样式（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "start_row": 0,
  "start_col": 0,
  "end_row": 9,
  "end_col": 4
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `start_row` (int64, 必填): 起始行索引（0-based）
- `start_col` (int64, 必填): 起始列索引（0-based）
- `end_row` (int64, 必填): 结束行索引
- `end_col` (int64, 必填): 结束列索引

### 返回值说明
```json
{}
```

---

## 14. sheet.clear_range_style

### 功能说明
清除在线表格指定区域内所有单元格的样式，不影响内容（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "start_row": 0,
  "start_col": 0,
  "end_row": 9,
  "end_col": 4
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `start_row` (int64, 必填): 起始行索引（0-based）
- `start_col` (int64, 必填): 起始列索引（0-based）
- `end_row` (int64, 必填): 结束行索引
- `end_col` (int64, 必填): 结束列索引

### 返回值说明
```json
{}
```

---

## 15. sheet.clear_range_all

### 功能说明
清空在线表格指定区域内所有单元格的内容和样式（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "start_row": 0,
  "start_col": 0,
  "end_row": 9,
  "end_col": 4
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `start_row` (int64, 必填): 起始行索引（0-based）
- `start_col` (int64, 必填): 起始列索引（0-based）
- `end_row` (int64, 必填): 结束行索引
- `end_col` (int64, 必填): 结束列索引

### 返回值说明
```json
{}
```

---

## 16. sheet.unset_freeze

### 功能说明
删除在线表格指定子表的所有冻结行列（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001"
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID

### 返回值说明
```json
{}
```

---

## 17. sheet.get_sheet_info

### 功能说明
获取在线表格的子表信息，包括子表 ID、名称、类型、行列数量（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890"
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID

> 注意：此工具不需要 `sheet_id` 参数，返回文档下所有子表的信息。

### 返回值说明
```json
{
  "sheets": [
    {
      "sheet_id": "sub_sheet_001",
      "sheet_name": "Sheet1",
      "sheet_type": "worksheet",
      "row_count": 100,
      "col_count": 26
    }
  ]
}
```
- `sheets` (array): 子表信息列表
  - `sheet_id` (string): 子表 ID
  - `sheet_name` (string): 子表名称
  - `sheet_type` (string): 子表类型：`worksheet` / `smartsheet` / `smartcanvas`
  - `row_count` (int32): 行数
  - `col_count` (int32): 列数

---

## 18. sheet.get_cell_data

### 功能说明
获取在线表格指定区域的单元格数据，支持返回 CSV 格式或结构化单元格数据（SHEET）。

> ⚠️ **限制**：单次请求的单元格范围不得超过 **20000** 个（即 `(end_row - start_row + 1) × (end_col - start_col + 1) ≤ 20000`），超出将返回错误。如需获取更大范围的数据，请分多次请求。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "start_row": 0,
  "start_col": 0,
  "end_row": 9,
  "end_col": 4,
  "return_csv": false
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `start_row` (int64, 必填): 起始行索引（0-based）
- `start_col` (int64, 必填): 起始列索引（0-based）
- `end_row` (int64, 必填): 结束行索引
- `end_col` (int64, 必填): 结束列索引
- `return_csv` (bool, 可选): 是否以 CSV 格式返回数据，`true` 返回 `csv_data`，`false` 返回 `cells` 结构化数据（默认 `false`）

### 返回值说明
```json
{
  "csv_data": "Name,Score\nAlice,95.5\n",
  "cells": [
    {
      "row": 0,
      "col": 0,
      "value_type": "STRING",
      "string_value": "Name"
    },
    {
      "row": 0,
      "col": 1,
      "value_type": "STRING",
      "string_value": "Score"
    }
  ]
}
```
- `csv_data` (string): CSV 格式数据（`return_csv=true` 时返回）
- `cells` (array): 结构化单元格数据（`return_csv=false` 时返回）
  - `row` (int32): 行索引（0-based）
  - `col` (int32): 列索引（0-based）
  - `value_type` (string): 值类型：`NUMBER` / `STRING` / `BOOL` / `FORMULA` / `ERROR` / `TIME_STRING` / `RICH_STRING`
  - `number_value` (double): 数值
  - `string_value` (string): 字符串值
  - `bool_value` (bool): 布尔值
  - `formula` (string): 公式

---

## 19. sheet.get_merged_cells

### 功能说明
获取在线表格指定区域内与该区域相交的合并单元格信息，返回合并单元格范围列表（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "start_row": 0,
  "start_col": 0,
  "end_row": 9,
  "end_col": 9
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `start_row` (int64, 必填): 查询区域起始行索引（0-based）
- `start_col` (int64, 必填): 查询区域起始列索引（0-based）
- `end_row` (int64, 必填): 查询区域结束行索引
- `end_col` (int64, 必填): 查询区域结束列索引

### 返回值说明
```json
{
  "merged_cells": [
    "sub_sheet_001$A1:B2",
    "sub_sheet_001$C3:D5"
  ]
}
```
- `merged_cells` (array): 与查询区域相交的合并单元格范围列表，格式为 `"SheetID$A1:B2"`（列使用字母表示，A=第0列，B=第1列，以此类推）

---

## 20. sheet.set_dimension_size

### 功能说明
设置在线表格指定行的行高或指定列的列宽，支持批量设置和清除自定义尺寸恢复默认值（SHEET）。支持两种模式：单个设置（使用 `index`）和批量设置（使用 `start_index` + `end_index`）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "dimensions": [
    {
      "dimension_type": "row",
      "index": 0,
      "size": 40
    },
    {
      "dimension_type": "col",
      "start_index": 2,
      "end_index": 5,
      "size": 120
    },
    {
      "dimension_type": "row",
      "index": 5,
      "is_clear": true
    }
  ]
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `dimensions` (array, 必填): 行高/列宽参数列表，每个元素包含：
  - `dimension_type` (string, 必填): 行列类型：`"row"` | `"col"`
  - `index` (int64, 可选): 行或列的索引（0-based），设置单个行高/列宽时使用
  - `start_index` (int64, 可选): 起始索引（0-based），批量设置行高/列宽时使用
  - `end_index` (int64, 可选): 结束索引（0-based），批量设置行高/列宽时使用
  - `size` (number, 可选): 行高或列宽的值（行高单位为pt，列宽单位为像素），`is_clear` 为 `true` 时该字段将被忽略
  - `is_clear` (bool, 可选): 是否清除自定义行高/列宽并恢复默认值，为 `true` 时 `size` 字段将被忽略

> 💡 **说明**：`index` 与 `start_index`/`end_index` 二选一。传 `index` 时设置单行/列；传 `start_index` + `end_index` 时批量设置该范围内所有行/列。

### 返回值说明
```json
{}
```

---

## 21. sheet.add_sheet

### 功能说明
在在线表格中添加一个新的子表，支持指定子表名称和位置（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "name": "新子表",
  "index": 0,
  "append_index": false
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `name` (string, 可选): 子表名称，长度限制为 31 个字符，不传则使用默认名称
- `index` (int64, 可选): 子表位置索引（0-based），不传或 `append_index` 为 `true` 时追加到末尾
- `append_index` (bool, 可选): 是否追加到末尾，为 `true` 时 `index` 字段将被忽略

> 注意：此工具不需要 `sheet_id` 参数，用于创建新的子表。

### 返回值说明
```json
{
  "sheet_id": "new_sheet_001"
}
```
- `sheet_id` (string): 新创建的子表 ID

---

## 22. sheet.delete_sheet

### 功能说明
删除在线表格中指定的子表（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001"
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 要删除的子表 ID

### 返回值说明
```json
{}
```

---

## 23. sheet.rename_sheet

### 功能说明
重命名在线表格中指定的子表（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "name": "新名称"
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `name` (string, 必填): 新的子表名称，长度限制为 31 个字符

### 返回值说明
```json
{}
```

---

## 24. sheet.insert_image

### 功能说明
在在线表格指定单元格插入一张图片，图片内容可通过 base64 或 image_id 传入（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "row_index": 0,
  "col_index": 0,
  "content": "iVBORw0KGgoAAAANSUhEUgAA..."
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `row_index` (int64, 必填): 目标行索引（0-based）
- `col_index` (int64, 必填): 目标列索引（0-based）
- `content` (string, 可选): 图片的 base64 内容，与 `image_id` 二选一，适合图片体积较小的场景；若图片过大导致 base64 内容超出传输限制，请改用 `image_id` 方式
- `image_id` (string, 可选): 图片的 image_id，本质是对图片信息加密后的字符串，与 `content` 二选一，适合图片体积较大的场景。获取方式：
  - 通过 `upload_image` MCP 接口上传图片后获取
  - 通过[腾讯文档开放平台 OpenAPI](https://docs.qq.com/open/developers/?nlc=1#/login) 图片上传接口获取（需先完成 OAuth 授权流程获取 `Access-Token`），示例命令：

```bash
curl --location --request POST 'https://docs.qq.com/openapi/resources/v2/images' \
  --header 'Access-Token: ACCESS_TOKEN' \
  --header 'Client-Id: CLIENT_ID' \
  --header 'Open-Id: OPEN_ID' \
  --form 'image=@"/path/to/your/image.png"'
```

上传成功后，取返回结果中的 `imageID` 字段值传入此参数

### 返回值说明
```json
{}
```

---

## 25. sheet.set_range_value_by_csv

### 功能说明
以 CSV 格式批量插入数据到在线表格（SHEET），指定左上角起始行列和 CSV 内容，自动解析并写入对应单元格。相比 `set_range_value` 需要逐个指定每个单元格的行列和类型，`set_range_value_by_csv` 只需提供一段 CSV 文本即可，更适合批量写入表格数据的场景。

> 💡 **建议**：单次写入请求体内容尽量不超过 **1MB**，超大批量请拆分为多次写入。

> 💡 **类型自动检测**：CSV 中的每个字段会自动检测类型——数字识别为 `NUMBER`，`true`/`false` 识别为 `BOOL`，以 `=` 开头识别为 `FORMULA`，其余为 `STRING`。空字段会被跳过。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "start_row": 0,
  "start_col": 0,
  "csv_data": "Name,Score,Pass\nAlice,95.5,true\nBob,82,true\nCharlie,59,false"
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `start_row` (int64, 可选): 左上角起始行索引（0-based），默认为 0
- `start_col` (int64, 可选): 左上角起始列索引（0-based），默认为 0
- `csv_data` (string, 必填): CSV 格式的数据内容，每行以换行符分隔，每列以逗号分隔，支持标准 CSV 引号转义

### 返回值说明
```json
{}
```

---

## 26. sheet.get_sheet_object_list

### 功能说明
获取在线表格指定子表上的对象列表，包括图表(chart)、透视表(pivot_table)、筛选表(table)、浮动图片(float_image)（SHEET）。支持按对象类型、显示区域、名称模糊匹配过滤。返回每个对象的 ID、类型、名称、显示区域、数据来源区域以及类型相关的简要信息。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "object_types": ["chart", "float_image"],
  "filter_by_range": true,
  "start_row": 0,
  "start_col": 0,
  "end_row": 50,
  "end_col": 10,
  "name_pattern": ""
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `object_types` (array, 可选): 对象类型过滤列表，留空表示返回所有类型；元素取值为字符串：`"chart"`(图表)、`"pivot_table"`(透视表)、`"table"`(筛选表)、`"float_image"`(浮动图片)
- `filter_by_range` (bool, 可选): 是否按区域过滤对象，为 `true` 时才根据 `start_row`/`start_col`/`end_row`/`end_col` 过滤
- `start_row` (int64, 可选): 显示区域过滤起始行索引（0-based），需要 `filter_by_range` 为 `true` 时才生效
- `start_col` (int64, 可选): 显示区域过滤起始列索引（0-based）
- `end_row` (int64, 可选): 显示区域过滤结束行索引
- `end_col` (int64, 可选): 显示区域过滤结束列索引
- `name_pattern` (string, 可选): 对象名称模糊匹配模式，留空表示不按名称过滤

### 返回值说明
```json
{
  "objects": [
    {
      "object_id": "chart_001",
      "object_type": "chart",
      "name": "销售趋势图",
      "display_range": "sub_sheet_001$A1:F15",
      "data_range": "sub_sheet_001$A1:B10",
      "chart_type": 1
    }
  ]
}
```
- `objects` (array): 子表上的对象列表
  - `object_id` (string): 对象唯一 ID
  - `object_type` (string): 对象类型：`"chart"` / `"pivot_table"` / `"table"` / `"float_image"` / `"unknown"`
  - `name` (string): 对象名称
  - `display_range` (string): 显示区域，格式如 `"SheetID$A1:B2"`
  - `data_range` (string): 数据来源区域，格式如 `"SheetID$A1:B2"`
  - `chart_type` (int32): 图表类型枚举值，仅当 `object_type=="chart"` 时有效
  - `pivot_row_group_count` (int32): 透视表行分组数
  - `pivot_column_group_count` (int32): 透视表列分组数
  - `pivot_value_count` (int32): 透视表值字段数
  - `table_has_header` (bool): 筛选表是否含表头
  - `table_column_count` (int32): 筛选表列数
  - `float_image_width` (int32): 浮动图片宽度
  - `float_image_height` (int32): 浮动图片高度

---

## 27. sheet.move_dimension

### 功能说明
在在线表格中移动一段连续的行或列到新的位置（SHEET）。移动语义与 Apps Script `moveRows`/`moveColumns` 一致：`index` 表示要移动的起始行/列索引（0-based），`count` 表示要移动的行/列数量，`to` 表示移动到的目标位置索引（0-based，相对原表）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "dimension_type": "row",
  "index": 0,
  "count": 3,
  "to": 10
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `dimension_type` (string, 必填): 行列类型：`"row"` | `"col"`
- `index` (int64, 必填): 待移动的起始索引（0-based）
- `count` (int64, 必填): 待移动的行/列数量
- `to` (int64, 必填): 目标位置索引（0-based，相对原表）

### 返回值说明
```json
{}
```

---

## 28. sheet.update_filter

### 功能说明
更新在线表格已有筛选的范围和/或列筛选项（SHEET）。当前列筛选项**仅支持值筛选**（`FILTER_CRITERIA_VALUE`）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "is_update_range": true,
  "start_row": 0,
  "start_col": 0,
  "end_row": 100,
  "end_col": 5,
  "columns": [
    { "col": 0, "visible_values": ["北京", "上海"] }
  ]
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `is_update_range` (bool, 可选): 是否更新筛选范围。`true` 时使用下面 4 个字段作为新筛选范围（4 字段必填）；`false`（默认）保留原范围
- `start_row` / `start_col` / `end_row` / `end_col` (int64, 条件必填): 新的数据区域，仅当 `is_update_range=true` 时生效
- `columns` (array, 可选): 列筛选项列表，为空时不修改原列筛选项；每项：
  - `col` (int64, 必填): 列索引（0-based，相对整个子表）
  - `visible_values` (string[], 必填): 该列保留可见的值；不在列表内的值会被隐藏；传空列表代表隐藏该列全部值

### 返回值说明
```json
{}
```

---

## 29. sheet.find

### 功能说明
在表格中搜索指定文本，返回匹配的单元格位置（SHEET）。支持大小写敏感、整单元格匹配、正则表达式、搜索公式等选项；支持基于 `offset` / `max_results` 的分页。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "search_term": "张三",
  "sheet_name": "Sheet1",
  "match_case": false,
  "match_entire_cell": false,
  "use_regex": false,
  "match_formulas": false,
  "offset": 0,
  "max_results": 50
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `search_term` (string, 必填): 要搜索的文本
- `sheet_name` (string, 可选): 限定搜索的子表名称，不指定则全表搜索
- `start_row` / `start_col` (int64, 可选): 搜索起始行/列（0-based，不传从 0 开始）
- `end_row` / `end_col` (int64, 可选): 搜索结束行/列（0-based，不传到最后一行/列）
- `offset` (int64, 可选): 分页偏移量（默认 0）
- `match_case` (bool, 可选): 大小写敏感（默认 `false`）
- `match_entire_cell` (bool, 可选): 整单元格匹配（默认 `false`）
- `use_regex` (bool, 可选): 将搜索文本作为正则表达式（默认 `false`）
- `match_formulas` (bool, 可选): 搜索公式内容（默认 `false`）
- `max_results` (int64, 可选): 每页最大结果数（默认 50）

> 注意：此工具不需要 `sheet_id` 参数，可通过 `sheet_name` 限定子表，未指定则全表搜索。

### 返回值说明
```json
{
  "total_results": 12,
  "results": [
    { "sheet_id": "sub_sheet_001", "row": 2, "col": 0, "address": "A3" }
  ],
  "overflow": false,
  "next_offset": 0
}
```
- `total_results` (int64): 本次查找命中的总结果数
- `results` (array): 当前页命中单元格列表，每项含 `sheet_id`、`row`/`col`（0-based）、`address`（A1 风格 1-based 地址）
- `overflow` (bool): 是否超过引擎底层 / 分页限制仍有更多未返回的结果
- `next_offset` (int64): 当 `overflow=true` 且仅因分页截断时下一页的 offset；后端原生 overflow 场景下为 0

---

## 30. sheet.get_cell_style

### 功能说明
获取在线表格指定区域单元格的样式信息（SHEET），包括背景色、字体颜色、字号、字体、数字格式。返回每个单元格的位置和样式字段，未设置的样式对应字段为空。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "start_row": 0,
  "start_col": 0,
  "end_row": 5,
  "end_col": 5
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `start_row` / `start_col` / `end_row` / `end_col` (int64, 必填): 查询区域（0-based）

### 返回值说明
```json
{
  "cells": [
    {
      "row": 0,
      "col": 0,
      "background_color": "FFFFFF00",
      "font_color": "FF000000",
      "font_size": 12,
      "font_family": "微软雅黑",
      "number_format": ""
    }
  ]
}
```
- `cells` (array): 单元格样式列表，区域内每个单元格一项
  - `row` / `col` (int32): 行/列索引（0-based）
  - `background_color` (string): 背景色 ARGB hex，未设置时为空
  - `font_color` (string): 字体颜色 ARGB hex，未设置时为空
  - `font_size` (int32): 字号 pt，未设置时为 0
  - `font_family` (string): 字体名称，未设置时为空
  - `number_format` (string): 数字格式，未设置时为空

---

## 31. sheet.move_sheet

### 功能说明
移动在线表格中子表的顺序，按源位置和目标位置（均为 0-based 索引）调整子表排列（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "src_index": 2,
  "dest_index": 0
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `src_index` (int64, 必填): 源位置子表索引（0-based）
- `dest_index` (int64, 必填): 目标位置子表索引（0-based）

> 注意：此工具不需要 `sheet_id` 参数，按子表索引位置定位。

### 返回值说明
```json
{}
```

---

## 32. sheet.set_border

### 功能说明
设置在线表格指定区域单元格的边框样式（SHEET），支持设置上下左右及内部横竖边框，可指定边框颜色和线型。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "start_row": 0,
  "start_col": 0,
  "end_row": 5,
  "end_col": 5,
  "border_positions": [0, 1, 2, 3],
  "border_style": 1,
  "border_color": "FF0000"
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `start_row` / `start_col` / `end_row` / `end_col` (int64, 必填): 区域索引（0-based）
- `border_positions` (int[], 必填): 要设置的边框位置列表，至少一个；取值：
  - `0` = 上边框（TOP）
  - `1` = 下边框（BOTTOM）
  - `2` = 左边框（LEFT）
  - `3` = 右边框（RIGHT）
  - `4` = 内部竖线（INNER_VERTICAL）
  - `5` = 内部横线（INNER_HORIZONTAL）
- `border_style` (int, 可选): 边框线型，默认 `1`（细线）；取值：
  - `0` = 无、`1` = 细线、`2` = 中等、`3` = 虚线、`4` = 点线、`5` = 粗线、`6` = 双线、`7` = 极细线
- `border_color` (string, 可选): 边框颜色，RGB hex（不含 `#`），如 `"FF0000"`；不传则使用默认黑色

### 返回值说明
```json
{}
```

---

## 33. sheet.clear_border

### 功能说明
清除在线表格指定区域单元格的边框（SHEET），支持按位置清除指定边框或清除全部边框。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "start_row": 0,
  "start_col": 0,
  "end_row": 5,
  "end_col": 5,
  "border_positions": [0, 1]
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `start_row` / `start_col` / `end_row` / `end_col` (int64, 必填): 区域索引（0-based）
- `border_positions` (int[], 可选): 要清除的边框位置列表（取值同 `set_border.border_positions`）；不传则清除全部边框

### 返回值说明
```json
{}
```

---

## 34. sheet.sort_range

### 功能说明
对在线表格指定区域按列排序（SHEET），支持多列排序、表头识别和筛选区域内排序（仅支持单列）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "start_row": 0,
  "start_col": 0,
  "end_row": 100,
  "end_col": 5,
  "has_header": true,
  "columns": [
    { "col": 2, "order": 1 },
    { "col": 0, "order": 0 }
  ]
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `start_row` / `start_col` / `end_row` / `end_col` (int64, 必填): 排序区域（0-based）
- `has_header` (bool, 可选): 是否含表头（首行不参与排序，默认 `false`）
- `columns` (array, 必填): 排序列列表，至少一个，多列时按顺序依次作为排序键。每项：
  - `col` (int64, 必填): 列索引（0-based，相对于整个子表）
  - `order` (int, 可选): 排序方向：`0` = 降序（默认）、`1` = 升序
- `table_id` (string, 可选): 筛选表 ID。传入时表示在筛选区域内排序，**仅支持单列排序**

### 返回值说明
```json
{}
```

---

## 35. sheet.add_chart

### 功能说明
在在线表格中添加图表（SHEET），支持指定图表类型、数据范围、位置大小等参数，自动从数据范围构建图表配置。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "chart_type": "CHART_CLUSTERED_COLUMN",
  "data_range": {
    "start_row": 0,
    "start_col": 0,
    "end_row": 9,
    "end_col": 1
  },
  "location": {
    "row_index": 1,
    "col_index": 4
  },
  "drawing_id": "chart_001",
  "title": "销售趋势"
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `chart_type` (string, 必填): 图表类型枚举名，常用取值：
  - 面积图：`CHART_AREA` / `CHART_AREA_3D` / `CHART_STACKED_AREA` / `CHART_PERCENT_STACKED_AREA`
  - 折线图：`CHART_LINE` / `CHART_LINE_3D` / `CHART_STACKED_LINE`
  - 股价图：`CHART_HIGH_LOW_CLOSE_STOCK`
  - 雷达图：`CHART_RADAR`
  - 散点图：`CHART_SCATTER`
  - 饼图：`CHART_PIE`
  - 条形图：`CHART_CLUSTERED_BAR`
  - 柱形图：`CHART_CLUSTERED_COLUMN`
  - 气泡图：`CHART_BUBBLE`
  - 组合图：`CHART_CUSTOM_COMBO`
  - 其他：`CHART_REGION_MAP` / `CHART_TREEMAP`
- `data_range` (object, 必填): 数据范围
  - `start_row` / `start_col` / `end_row` / `end_col` (int32, 必填): 数据区域（0-based）
- `drawing_id` (string, 必填): 图表标识ID，由调用方生成（用于后续删除/更新图表）
- `location` (object, 可选): 图表位置和大小，不传则使用默认位置
  - `row_index` (int32): 锚定单元格行索引（0-based）
  - `col_index` (int32): 锚定单元格列索引（0-based）
  - `horizontal_offset` (int32, 可选): 单元格内水平偏移
  - `vertical_offset` (int32, 可选): 单元格内垂直偏移
  - `width` (int32, 可选): 图表宽度
  - `height` (int32, 可选): 图表高度
- `use_column_as_series` (bool, 可选): 列作为系列（不设则自动检测）
- `first_row_as_header` (bool, 可选): 首行作为表头/系列名（不设则自动检测）
- `first_column_as_category` (bool, 可选): 首列作为分类轴（不设则自动检测）
- `title` (string, 可选): 图表显示标题

### 返回值说明
```json
{}
```

---

## 36. sheet.delete_chart

### 功能说明
删除在线表格中指定的图表（SHEET）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "drawing_id": "chart_001"
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `drawing_id` (string, 必填): 要删除的图表标识ID

### 返回值说明
```json
{}
```

---

## 37. sheet.get_charts

### 功能说明
获取在线表格指定子表下的所有图表信息（SHEET），包括图表 ID、类型、标题、位置、数据范围以及完整 ChartOptions JSON。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001"
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID

### 返回值说明
```json
{
  "charts": [
    {
      "drawing_id": "chart_001",
      "chart_type": "CHART_CLUSTERED_COLUMN",
      "title": "销售趋势",
      "location": { "row_index": 1, "col_index": 4 },
      "data_range": { "start_row": 0, "start_col": 0, "end_row": 9, "end_col": 1 },
      "options_json": "{\"title\":...,\"series\":...}"
    }
  ]
}
```
- `charts` (array): 图表信息列表
  - `drawing_id` (string): 图表标识ID
  - `chart_type` (string): 图表类型枚举名
  - `title` (string): 图表标题
  - `location` (object): 图表位置和大小（与 `add_chart.location` 同结构）
  - `data_range` (object): 图表数据范围（与 `add_chart.data_range` 同结构）
  - `options_json` (string): 图表完整 options 配置（JSON 字符串），客户端按需 `JSON.parse`；顶层 key 包含 `title` / `legend` / `xAxis` / `yAxis` / `secondaryYAxis` / `series` 等

---

## 38. sheet.update_chart

### 功能说明
更新在线表格中指定图表的类型、数据范围、位置尺寸、标题等配置（SHEET），未传字段保持原值。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "drawing_id": "chart_001",
  "chart_type": "CHART_LINE",
  "title": "新标题",
  "bool_options": {
    "use_column_as_series": { "value": true }
  }
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `drawing_id` (string, 必填): 图表标识ID
- `chart_type` (string, 可选): 图表类型枚举名（取值同 `add_chart.chart_type`），不传保持原类型
- `location` (object, 可选): 位置和尺寸，结构同 `add_chart.location`
- `data_range` (object, 可选): 数据源区域，结构同 `add_chart.data_range`
- `bool_options` (object, 可选): 布尔选项，使用 `BoolValue` 包装类型区分"未传"和"传了 false"
  - `use_column_as_series` (BoolValue): 列作为系列
  - `first_row_as_header` (BoolValue): 首行作为表头
  - `first_column_as_category` (BoolValue): 首列作为分类轴
- `title` (string, 可选): 图表标题
- `options_json` (string, 可选): 完整 ChartOptions JSON（高级配置）

### 返回值说明
```json
{}
```

---

## 39. sheet.insert_range

### 功能说明
在指定区域插入空白单元格（SHEET）。`dimension_type=col` 时按列插入（选中区域及其右侧单元格右移）；`dimension_type=row` 时按行插入（选中区域及其下方单元格下移）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "start_row": 0,
  "start_col": 0,
  "end_row": 2,
  "end_col": 2,
  "dimension_type": "row"
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `start_row` / `start_col` / `end_row` / `end_col` (int64, 必填): 区域索引（0-based）
- `dimension_type` (string, 必填): 移动方式：`"col"`（右移）| `"row"`（下移）

### 返回值说明
```json
{}
```

---

## 40. sheet.delete_range

### 功能说明
在指定区域删除单元格（SHEET）。`dimension_type=col` 时按列删除（右侧单元格左移）；`dimension_type=row` 时按行删除（下方单元格上移）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "start_row": 0,
  "start_col": 0,
  "end_row": 2,
  "end_col": 2,
  "dimension_type": "row"
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 子表 ID
- `start_row` / `start_col` / `end_row` / `end_col` (int64, 必填): 区域索引（0-based）
- `dimension_type` (string, 必填): 移动方式：`"col"`（左移）| `"row"`（上移）

### 返回值说明
```json
{}
```

---

## 41. sheet.validate_file_data

### 功能说明
校验在线表格指定版本的数据是否正常（**排障专用**，SHEET）。内部走 `GetKeyframe` 并开启 `enable_mutation_check`，若任意一条 mutation 在回放前的校验阶段失败，则返回 `is_valid=false` 以及失败 mutation 的阶段、类型、详细原因。推荐 `is_latest=true` 直接校验最新版本。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "is_latest": true
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `version` (int64, 可选): 要校验的修订版本号；当 `is_latest=true` 时该字段被忽略
- `is_latest` (bool, 可选): 是否校验最新版本。`true` 时忽略 `version`，服务端用 meta 的最新版本号；`false` 时使用 `version` 指定历史版本

> 注意：此工具不需要 `sheet_id` 参数，校验整个文档数据。

### 返回值说明
```json
{
  "is_valid": false,
  "checked_version": 12345,
  "ret_code": 0,
  "stage": "RebuildSheet",
  "failed_mutation_type": 27,
  "err_msg": "stage=RebuildSheet, type=27, detail=..."
}
```
- `is_valid` (bool): 是否通过校验：`true`=数据正常；`false`=存在非法 mutation
- `checked_version` (int64): 本次实际校验的版本号
- `ret_code` (int32): C++ 侧返回的错误码；校验通过时为 0；失败时为 GetKeyframe 底层错误码
- `stage` (string): 首个校验失败所在回放阶段，可能取值：`RebuildWorkbook.workbook_cmds` / `RebuildWorkbook.revisions` / `RebuildSheet` / `RebuildSheetWithTrim` / `ApplyNewRevisions`；校验通过时为空
- `failed_mutation_type` (int32): 首个校验失败的 mutation 类型枚举整数值；校验通过时为 0
- `err_msg` (string): 完整错误信息（含 stage / type / detail 上下文）；校验通过时为空

---

## 42. sheet.add_pivot_table

### 功能说明
在在线表格中创建透视表（SHEET）。`anchor` 指定透视表放置位置（左上角单元格），`source_range` 指定数据源区域，`pivot_table_id` 由调用方生成并保持唯一。`create_new_sheet=true` 时由引擎新建子表放置（忽略 `anchor_sheet_id`）；`create_new_sheet=false`（默认）时 `anchor_sheet_id` 必填，`anchor_row`/`anchor_col` 默认为 0。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "pivot_table_id": "pt_001",
  "name": "销售透视",
  "source_sheet_id": "sub_sheet_001",
  "source_range": {
    "start_row": 0,
    "start_col": 0,
    "end_row": 100,
    "end_col": 5
  },
  "anchor_sheet_id": "sub_sheet_002",
  "anchor_row": 0,
  "anchor_col": 0,
  "create_new_sheet": false
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `pivot_table_id` (string, 必填): 透视表唯一ID（由调用方生成并保持唯一）
- `name` (string, 可选): 透视表显示名称
- `source_sheet_id` (string, 必填): 数据源所在子表ID
- `source_range` (object, 必填): 数据源区域，4 个字段均必填
  - `start_row` / `start_col` / `end_row` / `end_col` (int64, 必填): 数据源区域（0-based）
- `anchor_sheet_id` (string, 条件必填): 锚点（透视表左上角）所在子表ID，`create_new_sheet=false` 时必填，`true` 时忽略
- `anchor_row` (int64, 可选): 锚点行索引（0-based，默认 0）
- `anchor_col` (int64, 可选): 锚点列索引（0-based，默认 0）
- `create_new_sheet` (bool, 可选): 是否新建子表放置透视表，默认 `false`
- `new_sheet_id` (string, 可选): 新建子表的 ID（`create_new_sheet=true` 时使用）

> 注意：此工具不需要顶层 `sheet_id` 参数，使用 `source_sheet_id` 与 `anchor_sheet_id` 分别指定数据源和锚点子表。

### 返回值说明
```json
{}
```

---

## 43. sheet.remove_pivot_table

### 功能说明
删除在线表格中已存在的透视表（SHEET）。通过 `sheet_id` 与 `pivot_table_id` 联合定位目标透视表，删除后其关联的 pivot cache 会被一并清理。`sheet_id` 必须是 worksheet 子表。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_002",
  "pivot_table_id": "pt_001"
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 目标透视表所在子表ID（必须是 worksheet）
- `pivot_table_id` (string, 必填): 目标透视表ID

### 返回值说明
```json
{}
```

---

## 44. sheet.get_pivot_table_detail

### 功能说明
读取指定透视表的详细配置（数据源、行/列/值/筛选、锚点位置、ID 等），用于展示或回显，不会修改文档（SHEET）。`pivot_table_id` 与 `pivot_table_name` 至少传一个，**id 优先**。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "pivot_table_id": "pt_001"
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `pivot_table_id` (string, 二选一): 目标透视表ID（与 `pivot_table_name` 二选一，id 优先）
- `pivot_table_name` (string, 二选一): 目标透视表显示名称

> 注意：此工具不需要 `sheet_id` 参数。

### 返回值说明
```json
{
  "pivot_table": {
    "pivot_table_id": "pt_001",
    "pivot_table_name": "销售透视",
    "anchor_sheet_id": "sub_sheet_002",
    "anchor_row": 0,
    "anchor_col": 0,
    "row_group_columns": [0],
    "column_group_columns": [1],
    "source_sheet_id": "sub_sheet_001",
    "source_data_range": {
      "sheet_id": "sub_sheet_001",
      "start_row_index": 0,
      "start_col_index": 0,
      "end_row_index": 100,
      "end_col_index": 5
    },
    "pivot_values": [
      { "source_data_column": 2, "summarize_function": "SUM" }
    ],
    "filters": [
      { "source_data_column": 3, "visible_values": ["北京"] }
    ]
  }
}
```
- `pivot_table` (object): 透视表详情
  - `pivot_table_id` (string): 透视表ID
  - `pivot_table_name` (string): 透视表显示名称
  - `anchor_sheet_id` (string): 锚点所在子表ID
  - `anchor_row` / `anchor_col` (int32): 锚点行/列索引（0-based）
  - `row_group_columns` (int32[]): 行分组的源数据列索引列表
  - `column_group_columns` (int32[]): 列分组的源数据列索引列表
  - `source_sheet_id` (string): 数据源所在子表ID
  - `source_data_range` (object): 数据源范围（GridRange，含 `sheet_id` / `start_row_index` / `start_col_index` / `end_row_index` / `end_col_index`）
  - `pivot_values` (array): 数据值字段列表
    - `source_data_column` (int32): 源数据列索引（0-based）
    - `summarize_function` (string): 聚合函数大写名（`SUM` / `COUNT` / `AVERAGE` / `MAX` / `MIN` / `PRODUCT` / `COUNTNUMS` / `STDEV` / `STDEVP` / `VAR` / `VARP`）
  - `filters` (array): 页字段筛选器列表
    - `source_data_column` (int32): 源数据列索引（0-based）
    - `visible_values` (string[]): 保留可见的值

---

## 45. sheet.update_pivot_table

### 功能说明
更新已有透视表的字段配置（行分组、列分组、数据值、筛选、计算字段，SHEET）。**替换语义**：每次调用整体替换字段配置，未传入的分类视为清空；推荐先调 `get_pivot_table_detail` 拉取当前配置后再下发。`sheet_id` 必须是 worksheet。

> ⚠️ **校验规则**：
> - `pivot_values[].summarize_function` 必填且不区分大小写，合法值：`SUM` / `COUNT` / `COUNTA` / `AVERAGE` / `MAX` / `MIN` / `PRODUCT` / `COUNTNUMS` / `STDEV` / `STDEVP` / `VAR` / `VARP`（COUNTA 等同于 COUNT）
> - `calculated_values[]` 中 `name` 与 `formula` 均不能为空

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_002",
  "pivot_table_id": "pt_001",
  "row_groups": [{ "source_data_column": 0 }],
  "column_groups": [{ "source_data_column": 1 }],
  "pivot_values": [
    { "source_data_column": 2, "summarize_function": "SUM" }
  ],
  "filters": [
    { "source_data_column": 3, "visible_values": ["北京", "上海"] }
  ],
  "calculated_values": [
    { "name": "毛利率", "formula": "=利润/收入" }
  ]
}
```

### 参数说明
- `file_id` (string, 必填): 文档 ID
- `sheet_id` (string, 必填): 透视表所在子表ID（必须是 worksheet）
- `pivot_table_id` (string, 必填): 目标透视表ID
- `row_groups` (array, 可选): 行分组字段（替换语义），每项 `{ source_data_column: int }`
- `column_groups` (array, 可选): 列分组字段（替换语义），每项 `{ source_data_column: int }`
- `pivot_values` (array, 可选): 数据值字段（替换语义），每项：
  - `source_data_column` (int32, 必填): 源数据列索引（0-based）
  - `summarize_function` (string, 必填): 聚合函数名称，不区分大小写，合法值见上方校验规则
- `filters` (array, 可选): 页字段筛选器（替换语义），每项：
  - `source_data_column` (int32, 必填): 源数据列索引（0-based）
  - `visible_values` (string[], 可选): 保留可见的值列表
- `calculated_values` (array, 可选): 计算字段（替换语义），每项：
  - `name` (string, 必填): 字段名
  - `formula` (string, 必填): 计算公式

### 返回值说明
```json
{}
```

---

## 46. sheet.add_protect_range

### 功能说明
在在线表格指定普通子表（worksheet）的单个矩形区域或整张子表创建保护区域（SHEET）。创建成功会同步在区域权限服务创建默认权限记录，并返回后续更新或删除使用的 `protect_range_id`。

### 调用示例

#### 示例 1：保护指定区域
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "range": {
    "start_row": 0,
    "start_col": 0,
    "end_row": 10,
    "end_col": 5
  }
}
```

#### 示例 2：保护整张子表
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001",
  "whole_sheet": true
}
```

### 参数说明
- `file_id` (string, 可选): 文档 ID，与 `file_url` 二选一
- `file_url` (string, 可选): 在线表格的 URL 链接，与 `file_id` 二选一
- `sheet_id` (string, 必填): 子表 ID（仅支持普通子表 worksheet）
- `range` (object, 可选): 保护区域范围，单个矩形区域；与 `whole_sheet` 二选一。未传或传 `false` 的 `whole_sheet` 时，必须传 `range`
  - `start_row` (int64, 必填): 起始行索引（0-based）
  - `start_col` (int64, 必填): 起始列索引（0-based）
  - `end_row` (int64, 必填): 结束行索引（0-based，不包含该行）
  - `end_col` (int64, 必填): 结束列索引（0-based，不包含该列）
- `whole_sheet` (bool, 可选): 是否保护整张子表；传 `true` 时不能同时传 `range`

### 返回值说明
```json
{
  "protect_range_id": "protected_range_001"
}
```

---

## 47. sheet.update_protect_range

### 功能说明
按 `protect_range_id` 修改在线表格已有保护区域的范围（SHEET）。`range` 为替换后的单个矩形区域，仅支持普通子表（worksheet）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "protect_range_id": "protected_range_001",
  "range": {
    "start_row": 2,
    "start_col": 1,
    "end_row": 12,
    "end_col": 6
  }
}
```

### 参数说明
- `file_id` (string, 可选): 文档 ID，与 `file_url` 二选一
- `file_url` (string, 可选): 在线表格的 URL 链接，与 `file_id` 二选一
- `sheet_id` (string, 可选): 子表 ID；`protect_range_id` 可唯一定位子表，传入时需与保护区域所在子表一致
- `protect_range_id` (string, 必填): 要修改的保护区域 ID，由 `sheet.add_protect_range` 返回
- `range` (object, 必填): 新保护区域范围，替换语义，单个矩形区域
  - `start_row` (int64, 必填): 起始行索引（0-based）
  - `start_col` (int64, 必填): 起始列索引（0-based）
  - `end_row` (int64, 必填): 结束行索引（0-based，不包含该行）
  - `end_col` (int64, 必填): 结束列索引（0-based，不包含该列）

### 返回值说明
```json
{}
```

---

## 48. sheet.delete_protect_range

### 功能说明
按 `protect_range_id` 删除在线表格已有保护区域（SHEET），并同步清理区域权限服务记录。仅支持普通子表（worksheet）。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "protect_range_id": "protected_range_001"
}
```

### 参数说明
- `file_id` (string, 可选): 文档 ID，与 `file_url` 二选一
- `file_url` (string, 可选): 在线表格的 URL 链接，与 `file_id` 二选一
- `sheet_id` (string, 可选): 子表 ID；`protect_range_id` 可唯一定位子表，传入时需与保护区域所在子表一致
- `protect_range_id` (string, 必填): 要删除的保护区域 ID

### 返回值说明
```json
{}
```

---

## 49. sheet.get_protect_ranges

### 功能说明
获取在线表格指定普通子表（worksheet）的所有保护区域（SHEET），返回每个保护区域的 `protect_range_id` 与 `range`。整张子表保护区域也会以 `range` 形式返回。

### 调用示例
```json
{
  "file_id": "sheet_1234567890",
  "sheet_id": "sub_sheet_001"
}
```

### 参数说明
- `file_id` (string, 可选): 文档 ID，与 `file_url` 二选一
- `file_url` (string, 可选): 在线表格的 URL 链接，与 `file_id` 二选一
- `sheet_id` (string, 必填): 子表 ID（仅支持普通子表 worksheet）

### 返回值说明
```json
{
  "ranges": [
    {
      "protect_range_id": "protected_range_001",
      "range": {
        "start_row": 0,
        "start_col": 0,
        "end_row": 10,
        "end_col": 5
      }
    }
  ]
}
```
