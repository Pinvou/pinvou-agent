# Excel 文档（sheet）品类操作指引

本目录提供 Excel 文档（sheet）品类的专业操作能力，包括计算、筛选、统计、Excel操作相关场景。sheet 工具通过独立的 `tdoc-sheet` MCP 服务调用，工具名以 `sheet.` 为前缀。

## 使用场景

对于以下操作，**必须**使用 `sheet.*` 系列工具处理：
插入图片、设置/批量设置单元格值、CSV 批量写入、设置/获取单元格样式、合并/取消合并单元格、插入/删除/移动行列、插入/删除区域、设置行高列宽、冻结/取消冻结行列、筛选（设置/移除/更新）、查找文本、设置/清除边框、排序、超链接、清除内容/样式、保护区域（创建/更新/删除/查询）、获取子表信息、获取单元格数据、获取合并单元格信息、获取子表对象列表、添加/删除/重命名/移动子表、图表（添加/删除/更新/查询）、透视表（创建/删除/读取详情/更新）、文件数据校验等。

---

## 服务信息

| 项目     | 说明                                                                             |
| -------- | -------------------------------------------------------------------------------- |
| 所属服务 | `tdoc-sheet`                                                                      |
| 服务地址 | `https://docs.qq.com/api/v6/sheet/mcp`                                           |
| 工具前缀 | `sheet.*`（如 `sheet.get_cell_data`、`sheet.set_cell_value`）                    |
| 调用方式 | 通过 MCP 协议调用 tdoc-sheet 服务（``mcp_tdoc-sheet_sheet.`<工具名>"`） |
| Token    | 与 tencent-docs / tdoc-slide / tdoc-doc 共用同一 Token，完成授权后自动配置         |
| 文档类型 | 仅支持 Sheet 文档类型                                                            |

> ⚠️ **所有 `sheet.*` 工具均通过 `tdoc-sheet` 独立 MCP 服务调用**，与 `tencent-docs` 主服务平级。Token 共用，但工具调用走独立 endpoint。

---

## 文档标识

sheet 工具优先使用 `file_id` 标识文档；支持 `file_url` 的工具也可直接传在线表格链接：
- `file_id` (string): 在线表格的唯一标识符
- `file_url` (string): 在线表格链接，支持时可与 `file_id` 二选一

> 💡 **获取 file_id**：可通过 `manage.search_file` 搜索文档获取，或从文档链接中解析。

---

## 工具列表

| 工具名称                     | 功能说明                                                         |
| ---------------------------- | ---------------------------------------------------------------- |
| sheet.insert_image           | 在指定单元格插入图片                                             |
| sheet.set_cell_value         | 设置单个单元格的值                                               |
| sheet.set_range_value        | 批量设置单元格的值                                               |
| sheet.set_range_value_by_csv | 以 CSV 格式批量插入数据                                          |
| sheet.set_cell_style         | 设置单元格的样式（也可清除样式）                                 |
| sheet.get_cell_style         | 获取区域单元格的样式（背景色、字体、字号等）                     |
| sheet.merge_cell             | 合并单元格                                                       |
| sheet.unmerge_cell           | 取消合并单元格                                                   |
| sheet.insert_dimension       | 插入行或列                                                       |
| sheet.delete_dimension       | 删除行或列                                                       |
| sheet.move_dimension         | 移动一段连续的行或列到新的位置                                   |
| sheet.set_dimension_size     | 设置行高或列宽（支持批量、清除）                                 |
| sheet.insert_range           | 在指定区域插入空白单元格（按行下移 / 按列右移）                  |
| sheet.delete_range           | 在指定区域删除单元格（按行上移 / 按列左移）                      |
| sheet.set_freeze             | 设置冻结行列                                                     |
| sheet.unset_freeze           | 删除所有冻结行列                                                 |
| sheet.set_filter             | 设置筛选                                                         |
| sheet.remove_filter          | 移除筛选                                                         |
| sheet.update_filter          | 更新已有筛选的范围和列筛选项                                     |
| sheet.find                   | 在表格中搜索指定文本（支持大小写敏感、整单元格、正则、公式搜索） |
| sheet.sort_range             | 对指定区域按列排序（支持多列、表头识别、筛选区域内排序）         |
| sheet.set_link               | 设置单元格超链接                                                 |
| sheet.clear_link             | 清除单元格超链接                                                 |
| sheet.set_border             | 设置区域单元格的边框（位置、线型、颜色）                         |
| sheet.clear_border           | 清除区域单元格的边框                                             |
| sheet.clear_range_cells      | 清除区域单元格内容                                               |
| sheet.clear_range_style      | 清除区域单元格样式                                               |
| sheet.clear_range_all        | 清空区域内容和样式                                               |
| sheet.get_sheet_info         | 获取子表信息（ID、名称、类型、行列数）                           |
| sheet.get_cell_data          | 获取区域单元格数据（支持 CSV 格式、公式）                        |
| sheet.get_merged_cells       | 获取区域内的合并单元格信息                                       |
| sheet.get_sheet_object_list  | 获取子表上的对象列表（图表、透视表、筛选表、浮动图片）           |
| sheet.add_sheet              | 增加子表                                                         |
| sheet.delete_sheet           | 删除子表                                                         |
| sheet.rename_sheet           | 重命名子表                                                       |
| sheet.move_sheet             | 移动子表的位置（按 0-based 源位置和目标位置）                    |
| sheet.add_chart              | 添加图表（指定类型、数据范围、位置大小、标题）                   |
| sheet.delete_chart           | 删除图表                                                         |
| sheet.get_charts             | 获取子表下所有图表信息                                           |
| sheet.update_chart           | 更新图表的类型、数据范围、位置尺寸、标题等                       |
| sheet.add_pivot_table        | 创建透视表                                                       |
| sheet.remove_pivot_table     | 删除透视表                                                       |
| sheet.get_pivot_table_detail | 读取透视表详细配置                                               |
| sheet.update_pivot_table     | 更新透视表的字段配置（行/列/值/筛选/计算字段）                   |
| sheet.add_protect_range      | 创建保护区域，支持指定矩形区域或整张子表                         |
| sheet.update_protect_range   | 按 protect_range_id 更新保护区域范围                             |
| sheet.delete_protect_range   | 按 protect_range_id 删除保护区域，并同步清理区域权限记录         |
| sheet.get_protect_ranges     | 获取指定子表下所有保护区域，返回 protect_range_id 和 range        |
| sheet.validate_file_data     | 校验在线表格指定版本的数据是否正常（排障专用）                   |

---

## 注意事项

- 工具名带 `sheet.` 前缀（如 `sheet.get_cell_data`、`sheet.set_cell_value` 等）
- 操作前需确保拥有文档的写入权限（查询类工具需读取权限）
- 所有行、列索引均从 0 开始计数（0-based）
- 颜色一般使用 ARGB hex 格式（如 `"FF000000"` 表示黑色）
- 详细 API 参数和调用示例请参考 `api/mcp-api.md`

---

## 按场景工作流

### 设置单元格内容和样式

```
1. 按需调用 sheet.* 工具更新单元格内容或者样式
    - 更新单个单元格内容：sheet.set_cell_value
    - 更新多个单元格内容：sheet.set_range_value
    - 以 CSV 文本批量写入（数据源即为表格 / CSV 格式时推荐）：sheet.set_range_value_by_csv
    - 更新单元格样式：sheet.set_cell_style
    - 查询单元格样式（背景色、字体、字号等）：sheet.get_cell_style
```

### 插入图片

```
1. 调用 sheet.insert_image，在指定单元格插入图片
2. 小图可以直接传 base64 编码后的图片内容 content
3. 若图片过大导致 base64 内容超出传输限制，应先调用 upload_image 工具获取 image_id，再调用 sheet.insert_image 传入 image_id
4. 需要提供目标 sheet_id、row_index、col_index，以及 content 或 image_id
```

### 清除单元格内容和样式

```
1. 按需调用 sheet.* 工具清除单元格内容或者样式
    - 清除单元格内容：sheet.clear_range_cells
    - 清除单元格样式：sheet.clear_range_style
    - 同时清除内容和样式：sheet.clear_range_all
    - 也可使用 sheet.set_cell_style 的 is_clear=true 或字段级清除（bool 传 false / string 传 ""）
```

### 设置和取消合并单元格

```
1. 调用 sheet.merge_cell，可以生成合并单元格（支持全部合并、按行合并、按列合并）
2. 调用 sheet.unmerge_cell，可以取消合并单元格
3. 调用 sheet.get_merged_cells 查询区域内的合并单元格分布
```

### 设置和取消筛选

```
1. 调用 sheet.set_filter，可以设置筛选
2. 调用 sheet.update_filter，可以更新筛选范围和/或列筛选项（仅支持值筛选）
3. 调用 sheet.remove_filter，可以取消筛选
```

### 设置和取消冻结

```
1. 调用 sheet.set_freeze，可以设置冻结区域
2. 调用 sheet.unset_freeze，可以一次性取消所有冻结区域
```

### 添加和删除链接

```
1. 调用 sheet.set_link，可以设置链接
2. 调用 sheet.clear_link，可以删除链接
```

### 增删行列与区域

```
1. 调用 sheet.insert_dimension，可以增加行或者列
2. 调用 sheet.delete_dimension，可以删除行或者列
3. 调用 sheet.move_dimension，可以移动一段连续的行或列到新的位置
4. 调用 sheet.insert_range，可以在指定区域插入空白单元格（按行下移 / 按列右移）
5. 调用 sheet.delete_range，可以在指定区域删除单元格（按行上移 / 按列左移）
```

### 设置行高列宽

```
1. 调用 sheet.set_dimension_size，可以设置指定行的行高或指定列的列宽
   - 支持单个设置（index）和批量设置（start_index + end_index）
   - 支持清除自定义尺寸恢复默认值（is_clear=true）
```

### 边框设置

```
1. 调用 sheet.set_border，可以设置区域单元格的边框
   - border_positions：上(0)/下(1)/左(2)/右(3)/内部竖线(4)/内部横线(5)
   - border_style：无(0)/细线(1)/中等(2)/虚线(3)/点线(4)/粗线(5)/双线(6)/极细线(7)
2. 调用 sheet.clear_border，可以清除区域单元格的边框（按位置或全部）
```

### 排序

```
1. 调用 sheet.sort_range，对指定区域按列排序
   - 支持多列排序（按 columns 顺序作为排序键）
   - 支持表头识别（has_header=true 时首行不参与排序）
   - 传 table_id 时表示在筛选区域内排序，仅支持单列
```

### 查找文本

```
1. 调用 sheet.find，在表格中搜索指定文本
   - 支持大小写敏感（match_case）、整单元格匹配（match_entire_cell）、正则（use_regex）、搜索公式（match_formulas）
   - 支持限定子表（sheet_name）和搜索范围（start_row/col、end_row/col）
   - 支持基于 offset / max_results 的分页
```

### 子表管理

```
1. 调用 sheet.add_sheet，可以增加子表，支持指定位置插入和尾部追加两种
2. 调用 sheet.delete_sheet，可以删除指定的子表
3. 调用 sheet.rename_sheet，可以重命名子表
4. 调用 sheet.move_sheet，可以移动子表的顺序（按 src_index / dest_index）
```

### 保护区域

```
1. 调用 sheet.add_protect_range，在普通子表（worksheet）创建保护区域
   - 保护指定矩形区域时传 range；range 包含 start_row / start_col / end_row / end_col，均为 0-based，end_row / end_col 为不包含边界
   - 保护整张子表时传 whole_sheet=true；range 与 whole_sheet 二选一，不能同时传
   - 创建成功会返回 protect_range_id，并同步在区域权限服务创建默认权限记录
2. 调用 sheet.get_protect_ranges，获取指定子表下所有保护区域，返回 protect_range_id 和 range
3. 调用 sheet.update_protect_range，按 protect_range_id 替换保护区域范围
   - sheet_id 可选；protect_range_id 可唯一定位子表，若传 sheet_id 需与保护区域所在子表一致
4. 调用 sheet.delete_protect_range，按 protect_range_id 删除保护区域，并同步清理区域权限服务记录
   - sheet_id 可选；protect_range_id 可唯一定位子表
```

### 图表

```
1. 调用 sheet.add_chart，添加图表，需指定 chart_type、data_range、drawing_id（调用方生成）
   - 支持柱形图、条形图、折线图、饼图、面积图、散点图、雷达图、气泡图、组合图等多种类型
   - 通过 location 指定锚点位置和大小
2. 调用 sheet.get_charts，获取子表下所有图表信息（含完整 ChartOptions JSON）
3. 调用 sheet.update_chart，更新图表的类型、数据范围、位置尺寸、标题等（未传字段保持原值）
4. 调用 sheet.delete_chart，删除指定图表
```

### 透视表

```
1. 调用 sheet.add_pivot_table，创建透视表
   - source_sheet_id + source_range 指定数据源
   - anchor_sheet_id + anchor_row/col 指定放置位置（或 create_new_sheet=true 由引擎新建子表）
   - pivot_table_id 由调用方生成并保持唯一
2. 调用 sheet.get_pivot_table_detail，读取透视表详细配置（数据源、行/列/值/筛选、锚点位置等）
3. 调用 sheet.update_pivot_table，更新透视表字段配置（替换语义，建议先 get_pivot_table_detail 拉取后再下发）
4. 调用 sheet.remove_pivot_table，删除透视表
```

### 查询接口

```
1. 调用 sheet.get_sheet_info，获取在线表格的子表信息，包括子表 ID、名称、类型、行列数量
2. 调用 sheet.get_cell_data，获取在线表格指定区域的单元格数据，支持返回 CSV 格式或结构化单元格数据，可设置 return_formula 只返回公式
3. 调用 sheet.get_merged_cells，获取在线表格指定区域内与该区域相交的合并单元格信息，返回合并单元格范围列表
4. 调用 sheet.get_cell_style，获取区域单元格的样式（背景色、字体颜色、字号、字体、数字格式）
5. 调用 sheet.get_sheet_object_list，获取子表上的对象列表（图表、透视表、筛选表、浮动图片），支持按类型/区域/名称过滤
```

### 数据校验（排障）

```
1. 调用 sheet.validate_file_data，校验在线表格指定版本的数据是否正常
   - 推荐 is_latest=true 直接校验最新版本
   - 也可通过 version 校验历史版本
   - 失败时返回失败 mutation 的阶段、类型、详细原因
```
