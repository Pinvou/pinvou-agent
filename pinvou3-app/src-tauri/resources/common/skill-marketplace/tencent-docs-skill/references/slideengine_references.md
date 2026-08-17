# SLIDE 编辑引擎 API 参考

本文件包含腾讯文档 SLIDE 编辑引擎（slideengine）的所有工具 API 说明。这些工具专用于演示文稿（PPT）的精细化编辑操作，包括幻灯片管理、形状插入、文本编辑、表格操作、图表管理、批注、节管理、分组、动画、主题设置、演讲者备注等。

> ⚠️ **注意**：本文档中的工具仅适用于 **演示文稿（幻灯片/PPT）** 类型，不适用于 Word 文档或智能文档等其他类型。如需了解 PPT 的完整工作流（0-1 生成、续写、改页、检查），请参考 `slide/entry.md`。

---

## 服务信息

| 项目     | 说明                                                                                    |
| -------- | --------------------------------------------------------------------------------------- |
| 所属服务 | `tdoc-slide`                                                                             |
| 服务地址 | `https://docs.qq.com/api/v6/slide/mcp`                                                  |
| 工具前缀 | `slide_*`（如 `slide_add_shape`、`slide_find_text`、`slide_set_notes_text` 等）          |
| 调用方式 | 通过 MCP 协议调用，使用 tdoc-slide 服务的 Authorization Token                              |
| 文档类型 | 仅支持演示文稿（幻灯片/PPT）类型                                                         |
| 工具总数 | **78**                                                                                  |

> ⚠️ **所有 `slide_*` 工具均使用 `file_id` 或 `file_url` 标识文档**（二选一）。若用户提供的是文档链接（形如 `https://docs.qq.com/slide/<id>`），可直接传入 `file_url`，服务端自动解析。

---

## 通用说明

### 文档标识

所有 slideengine 工具都支持通过 `file_id` 或 `file_url` 标识文档（二选一）：

- `file_id` (string): 文档唯一标识符，形如 `300000000$WKxxx`
- `file_url` (string): 腾讯文档的分享链接（形如 `https://docs.qq.com/slide/...`），服务端自动解析为 file_id

### 版本参数

所有 slideengine 工具都支持可选的 `version_info` 参数，用于指定基于哪个版本进行操作（不传时默认基于最新版本）：

- `version_info` (object, 可选):
  - `base_version` (int64, 可选): 基准版本号，通常使用上一步读类接口返回的 `version` 值。值为 0 表示不指定。
  - `is_latest` (bool, 可选): 是否基于最新版本操作。设为 `true` 时忽略 `base_version`。

> 💡 连续多步编辑时，建议将上一步接口返回的 `version` / `new_version` 传入下一步的 `version_info.base_version`，以避免并发冲突。

### 页面索引

- `page_index` (integer): 幻灯片页索引，**从 0 开始计数**。第 1 页对应 `page_index = 0`。

### 形状标识

- `shape_id` (string): 形状的唯一 ID，通过 `slide_get_shape_info` / `slide_get_page_info` 等读类接口获取。

### 坐标与尺寸单位

- **坐标（x, y）和尺寸（w, h）**：单位为 **磅（pt）**，1pt = 12700 EMU。
- **边框宽度（border_width）**：单位为 **pt**（磅）
- **旋转角度（rotation）**：单位为 **度**
- **EMU 换算**：`1 pt = 12700 EMU`（用于直接传 EMU 的接口，如 `slide_set_slide_size`）
- **透明度（fill_alpha, border_alpha）**：取值 0~100000（与 OOXML 对齐，100000 表示完全不透明）；部分简化接口取值 0~100

### 颜色

颜色值统一使用 6 位十六进制 RRGGBB 格式（**不含** `#` 前缀），如 `FF0000` 表示纯红。

### 响应结构

编辑类（写类）API：

- 大部分写类工具返回空 Rsp 消息（实际编辑结果通过 MCP TextContent 中的 JSON 透出）
- 部分工具（如表格类）返回结构化响应，包含 `new_version` / `base_version` 等

读类 API 返回结构化数据，包含：

- `version` (int64): 文档当前版本号
- 业务数据字段

---

## 工具列表

### 页面工具组（Page）

| 工具名称 | 类型 | 功能说明 |
|---------|------|---------|
| slide_add_slide | 写 | 在演示文稿中插入一张新幻灯片 |
| slide_add_slides | 写 | 在同一位置批量插入多张幻灯片，所有页共用同一布局模板 |
| slide_remove_slide | 写 | 删除指定位置的幻灯片 |
| slide_duplicate_slide | 写 | 深拷贝一张或多张幻灯片，副本插入在各自原始页的紧后方（或 target_page_index 指定的位置），保留所有形状、文本、动画和样式 |
| slide_move_slide | 写 | 移动一张或多张幻灯片到指定位置 |
| slide_get_info | 读 | 获取演示文稿元数据：幻灯片总数、有序 slide_ids、幻灯片尺寸（EMU 和磅 pt） |
| slide_get_page_info | 读 | 获取指定幻灯片上所有形状的摘要信息，包含每个形状的 id / 类型 / 位置 / 尺寸 / 文本 / 填充色 / 边框等 |
| slide_get_master_info | 读 | 获取演示文稿中母版页的详细信息，包含母版 ID、页眉页脚状态、布局列表及母版上的形状 |
| slide_set_page_properties | 写 | 设置幻灯片页面级属性，包括背景填充（纯色/图片/渐变）和可见性 |
| slide_add_page_number | 写 | 在指定幻灯片插入页码占位符，位置和样式从布局/母版继承 |
| slide_add_datetime | 写 | 在指定幻灯片插入日期时间占位符 |
| slide_add_notes | 写 | 为指定幻灯片创建演讲者备注页并写入文本内容 |

### 形状工具组（Shape）

| 工具名称 | 类型 | 功能说明 |
|---------|------|---------|
| slide_add_shape | 写 | 在指定幻灯片插入一个普通形状（rect / ellipse / triangle 等图形预设块） |
| slide_add_shapes | 写 | 批量在同一幻灯片插入多个形状 |
| slide_add_line_shape | 写 | 在指定幻灯片插入一根线形 / 方向性箭头线条（从 (x1,y1) 起点到 (x2,y2) 终点） |
| slide_add_line_shapes | 写 | 批量在同一幻灯片插入多根线形 / 方向性箭头线条 |
| slide_add_image | 写 | 在指定幻灯片插入一张图片。 |
| slide_add_text | 写 | 在指定幻灯片插入一个文本框 |
| slide_add_texts | 写 | 批量在同一张幻灯片插入多个文本框，单次产生一个 SlideCommand（一次广播），比循环调用 slide_add_text 更高效 |
| slide_reorder_shape | 写 | 调整指定形状的 z-order 层级 |
| slide_remove_shapes | 写 | 从指定幻灯片删除一个或多个形状 |
| slide_get_shape_info | 读 | 查询指定幻灯片中某个形状的详细信息（包含 type / preset_geom / bounds / text / fill / border / 字体属性 等 16 个字段） |
| slide_set_shape_properties | 写 | 修改幻灯片中一个或多个形状的属性 |

### 文本工具组（Text）

| 工具名称 | 类型 | 功能说明 |
|---------|------|---------|
| slide_append_text | 写 | 追加文本 |
| slide_insert_text | 写 | 插入文本 |
| slide_delete_text | 写 | 删除文本 |
| slide_find_text | 读 | 查找文本 |
| slide_find_replace_text | 写 | 查找替换文本 |
| slide_set_text_property | 写 | 设置文本格式 |
| slide_get_text | 读 | 读取文本 |

### 表格工具组（Table）

| 工具名称 | 类型 | 功能说明 |
|---------|------|---------|
| slide_add_table | 写 | 在指定幻灯片页创建空表格，page_index 从 0 开始计数 |
| slide_insert_table_rows | 写 | 在表格 shape 中插入一行或多行 |
| slide_insert_table_cols | 写 | 在表格 shape 中插入一列或多列 |
| slide_delete_table_rows | 写 | 从表格 shape 中删除一行或多行，从 index 指定的行开始 |
| slide_delete_table_cols | 写 | 从表格 shape 中删除一列或多列，从 index 指定的列开始 |
| slide_merge_table_cells | 写 | 合并表格 shape 中的矩形单元格区域，区域由 start_row/start_col 和 row_span/col_span 指定 |
| slide_unmerge_table_cells | 写 | 取消表格 shape 中指定矩形区域的单元格合并，区域由 start_row/start_col 和 row_span/col_span 指定 |
| slide_set_cell_text | 写 | 向表格 shape 的单个单元格写入纯文本 |

### 图表工具组（Chart）

| 工具名称 | 类型 | 功能说明 |
|---------|------|---------|
| slide_add_chart | 写 | 创建图表 |
| slide_change_chart_type | 写 | 切换图表类型 |
| slide_update_chart_data | 写 | 更新图表数据 |
| slide_get_chart_info | 读 | 查询图表信息 |
| slide_update_chart_title | 写 | 更新图表标题 |
| slide_update_chart_legend | 写 | 更新图表图例 |
| slide_update_chart_axis | 写 | 更新图表坐标轴 |
| slide_update_chart_gridlines | 写 | 更新主网格线 |
| slide_update_chart_series_style | 写 | 更新单条系列样式 |
| slide_update_chart_data_labels | 写 | 更新数据标签 |

### 分组工具组（Group）

| 工具名称 | 类型 | 功能说明 |
|---------|------|---------|
| slide_get_group_info | 读 | 查询组合信息 |
| slide_group_shapes | 写 | 组合形状 |
| slide_ungroup_shapes | 写 | 取消组合 |
| slide_reorder_shapes_in_group | 写 | 重排组合内形状 |
| slide_update_group_shape_properties | 写 | 修改组合内形状属性 |

### 动画工具组（Animation）

| 工具名称 | 类型 | 功能说明 |
|---------|------|---------|
| slide_add_anim | 写 | 为指定幻灯片中的某个形状添加动画效果 |
| slide_list_anim_types | 读 | 列出所有支持的动画类型，包含类型名称、分类（entrance / exit / emphasis）以及是否支持方向参数 |
| slide_remove_anim | 写 | 移除指定形状的某个动画 |
| slide_move_anim | 写 | 移动指定形状的动画在序列中的位置（从 from_index 移到 to_index） |
| slide_set_anim_properties | 写 | 修改指定形状某个动画的类型和方向 |
| slide_set_anim_trigger | 写 | 修改指定形状某个动画的触发方式 |

### 主题工具组（Theme）

| 工具名称 | 类型 | 功能说明 |
|---------|------|---------|
| slide_get_themes | 读 | 获取当前演示文稿中嵌入的所有主题列表，返回每个主题的 theme_id 和 theme_name |
| slide_list_builtin_themes | 读 | 列出服务端所有内置（预置）主题，返回每个主题的 theme_id 和 theme_name |
| slide_set_theme | 写 | 设置演示文稿的主题 |

### 批注工具组（Comment）

| 工具名称 | 类型 | 功能说明 |
|---------|------|---------|
| slide_get_comments | 读 | 查询批注 |
| slide_add_comment | 写 | 添加批注 |
| slide_remove_comment | 写 | 删除批注 |
| slide_modify_comment | 写 | 修改批注 |

### 节工具组（Section）

| 工具名称 | 类型 | 功能说明 |
|---------|------|---------|
| slide_get_sections | 读 | 查询节列表 |
| slide_add_section | 写 | 在指定位置添加新节 |
| slide_remove_sections | 写 | 删除节（保留幻灯片） |
| slide_remove_section_with_slides | 写 | 删除节及其幻灯片 |
| slide_move_section | 写 | 移动节 |
| slide_rename_section | 写 | 重命名节 |

### 演示文稿级工具组（Presentation）

| 工具名称 | 类型 | 功能说明 |
|---------|------|---------|
| slide_set_slide_size | 写 | 设置幻灯片尺寸 |
| slide_set_default_font | 写 | 设置默认字体 |
| slide_export_pages_to_image_urls | 写 | 将指定幻灯片页面导出为图片，返回可访问的图片 URL 列表 |

### 备注工具组（Notes）

| 工具名称 | 类型 | 功能说明 |
|---------|------|---------|
| slide_set_notes_text | 写 | 设置或覆盖指定幻灯片的备注页（演讲者备注）文本，page_index 从 0 开始计数 |

### 其他

| 工具名称 | 类型 | 功能说明 |
|---------|------|---------|
| slide_add_footer | 写 | 在指定幻灯片添加或移除页脚占位符 |
| slide_move_slides_to_section | 写 | 移动幻灯片到节 |
| slide_reply_comment | 写 | 回复批注 |

---

## 工具详细说明

## 页面工具组（Page）

### slide_add_slide

#### 功能说明

在演示文稿中插入一张新幻灯片。index 为 0-based 插入位置，必须落在 [0, 当前页数] 区间（等于当前页数即追加到末尾，不支持 -1，越界会报 page insert index out of range）；不确定当前页数时先调 slide_get_info 拿 slide_count；layout_index 为首个母版下的布局序号（0=标题，1=标题+内容 等），默认 0

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `index` | number | 否 | 0-based 插入位置，必须 ∈ [0, 当前页数]（=当前页数即追加到末尾），不支持 -1。不确定时先调 slide_get_info 拿 slide_count。 |
| `layout_index` | number | 否 | 布局序号（0=标题，1=标题+内容 等），默认 0 |

---

### slide_add_slides

#### 功能说明

在同一位置批量插入多张幻灯片，所有页共用同一布局模板。index 为 0-based 插入位置，必须落在 [0, 当前页数] 区间（=当前页数即追加到末尾，不支持 -1）；不确定当前页数时先调 slide_get_info 拿 slide_count。count 必须 >= 1

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `index` | number | 否 | 0-based 插入位置，必须 ∈ [0, 当前页数]（=当前页数即追加到末尾），不支持 -1。不确定时先调 slide_get_info 拿 slide_count。 |
| `count` | number | 否 | 要插入的页数，必须 >= 1 |
| `layout_index` | number | 否 | 布局序号（0=标题，1=标题+内容 等），默认 0 |

---

### slide_remove_slide

#### 功能说明

删除指定位置的幻灯片。page_index 为 0-based 页索引

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 待删除幻灯片的 0-based 页索引 |

---

### slide_duplicate_slide

#### 功能说明

深拷贝一张或多张幻灯片，副本插入在各自原始页的紧后方（或 target_page_index 指定的位置），保留所有形状、文本、动画和样式。page_indexes 和 target_page_index 均为 0-based 索引

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_indexes` | array | 否 | 待复制幻灯片的 0-based 页索引列表（整数数组），长度 >= 1 |
| `target_page_index` | number | 否 | 副本插入位置（0-based 索引），不传或 -1 表示默认（紧跟在被复制页后方） |

---

### slide_move_slide

#### 功能说明

移动一张或多张幻灯片到指定位置。移动后第一张被移动的幻灯片将出现在 to_index 处

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_indices` | array | 否 | 待移动幻灯片的 0-based 页索引列表（整数数组），长度 >= 1 |
| `to_index` | number | 否 | 目标位置（0-based），第一张被移动页将出现在此位置 |

---

### slide_get_info

#### 功能说明

获取演示文稿元数据：幻灯片总数、有序 slide_ids、幻灯片尺寸（EMU 和磅 pt）。在添加或编辑幻灯片前使用此工具了解结构

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |

---

### slide_get_page_info

#### 功能说明

获取指定幻灯片上所有形状的摘要信息和动画列表。
返回数据包含：
- `shapes`: 每个形状的 id / 类型 / 位置 / 尺寸 / 文本 / 填充色 / 边框等 16 个字段
- `animations`: 每页的动画序列，每个动画含 shape_id / index / seq_type / category / preset_id / preset_subtype

#### 批量获取全部页的 shape / animation

要一次性获取全部页（或指定页列表）信息，请使用本地脚本 `batch_get_page_info.py`（完整脚本代码已内嵌在 `slide_get_page_info` 工具描述中，直接从工具描述复制落盘即可执行）。

用法：`python3 batch_get_page_info.py --mcp-url "<endpoint>" --file-id "300000000$WKxxx"`
指定页：追加 `--pages 0,2,5,6`。认证：优先 `--auth` → WorkBuddy 连接器自动发现。

如果不需要批量、只需要单页信息，直接传 `page_index` 调用本工具即可；如果只需要总页数/页列表，先调 `slide_get_info` 即可。

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始 |

---

### slide_get_master_info

#### 功能说明

获取演示文稿中母版页的详细信息，包含母版 ID、页眉页脚状态、布局列表及母版上的形状。可选传 page_index 仅返回该页关联的母版

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 可选：指定幻灯片 0-based 页索引，仅返回该页关联的母版；省略时返回所有母版 |

---

### slide_set_page_properties

#### 功能说明

设置幻灯片页面级属性，包括背景填充（纯色/图片/渐变）和可见性。仅修改传入的属性，未传的保持不变。

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片 0-based 页索引 |
| `visible` | bool | 否 | 可见性：true=显示，false=隐藏；省略不修改 |
| `fill_type` | string | 否 | 背景填充类型：solid / image / gradient；省略不修改背景 |
| `fill_color` | string | 否 | fill_type=solid 时的背景色（RRGGBB）；空串清除背景 |
| `fill_alpha` | number | 否 | 背景透明度 0~100，0=完全不透明（默认），100=完全透明；solid/gradient 路径写入 srgb alpha，image 路径写入 blip.alphaModFix.amt |
| `image` | string | 否 | fill_type=image 时的图片（data URI / 本地绝对路径，不接受远程 URL） |
| `stretch` | bool | 否 | fill_type=image 时是否拉伸铺满整页；默认 true。设为 false 时图片按 tile_* 系列参数平铺（仅此时 tile_alignment / tile_flip / tile_tx / tile_ty / tile_sx / tile_sy 才生效） |
| `tile_alignment` | string | 否 | 平铺对齐锚点（仅 fill_type=image && stretch=false 时生效）；常用取值：tl / t / tr / l / ctr / r / bl / b / br（左上 / 上 / 右上 / 左 / 居中 / 右 / 左下 / 下 / 右下） |
| `tile_flip` | string | 否 | 平铺翻转模式（仅 fill_type=image && stretch=false 时生效）；OOXML 取值：none / x / y / xy |
| `tile_tx` | number | 否 | 平铺水平偏移（EMU；仅 fill_type=image && stretch=false 时生效） |
| `tile_ty` | number | 否 | 平铺垂直偏移（EMU；仅 fill_type=image && stretch=false 时生效） |
| `tile_sx` | number | 否 | 平铺水平缩放（1/100000；仅 fill_type=image && stretch=false 时生效） |
| `tile_sy` | number | 否 | 平铺垂直缩放（1/100000；仅 fill_type=image && stretch=false 时生效） |
| `gradient_stops` | array | 否 | fill_type=gradient 时的色标列表（至少 2 项），每项含 color(RRGGBB) 和 pos(0~100000) |
| `gradient_angle` | number | 否 | 渐变角度（1/60000 度） |

---

### slide_add_page_number

#### 功能说明

在指定幻灯片插入页码占位符，位置和样式从布局/母版继承

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片 0-based 页索引 |

---

### slide_add_datetime

#### 功能说明

在指定幻灯片插入日期时间占位符。display_text 为可选的显示文本，如 2026/05/11；为空时引擎保留空内容

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片 0-based 页索引 |
| `display_text` | string | 否 | 可选的显示文本，如 "2026/05/11"；空串时引擎保留空内容 |

---

### slide_add_notes

#### 功能说明

为指定幻灯片创建演讲者备注页并写入文本内容。text 为空串时创建空白备注页

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片 0-based 页索引 |
| `text` | string | 否 | 备注页文本；空字符串时创建空白备注页 |

---

## 形状工具组（Shape）

### slide_add_shape

#### 功能说明

在指定幻灯片插入一个普通形状（rect / ellipse / triangle 等图形预设块）。【重要】必须设置 fill_color 或 border_color 中的至少一个，否则形状对用户不可见，调用会被拒绝。【重要 — 箭头消歧】本工具不支持画两点之间的方向性箭头线条，仅支持 rightArrow / leftArrow 这两种固定方向的图形预设箭头块；如需画"从起点到终点的箭头线"，请改用 slide_add_line_shape 并设置 line_type=arrow / doubleArrow。当用户说"加箭头"时，请先询问要的是"线条箭头"（带起点终点）还是"图形箭头"（固定方向的图形块）再决定调哪个工具。page_index 从 0 开始；x/y/w/h 单位为磅（pt，1pt = 12700 EMU）；fill_alpha / border_alpha 取值 0~100；border_width 单位为 pt

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始 |
| `x` | number | 否 | 形状左上角横坐标（磅 pt，1pt = 12700 EMU） |
| `y` | number | 否 | 形状左上角纵坐标（磅 pt，1pt = 12700 EMU） |
| `w` | number | 否 | 形状宽度（磅 pt，1pt = 12700 EMU），必须 > 0 |
| `h` | number | 否 | 形状高度（磅 pt，1pt = 12700 EMU），必须 > 0 |
| `shape_type` | string | 否 | 形状预设类型，如 rect / ellipse / triangle / rightArrow / leftArrow 等；空串默认 rect。注意：rightArrow / leftArrow 是固定方向的图形预设箭头块，不能指定起点终点；如需画两点之间的方向性箭头线条，请改用 slide_add_line_shape |
| `fill_color` | string | 否 | 填充颜色（RRGGBB，无 # 前缀）；空串表示无填充 |
| `fill_alpha` | number | 否 | 填充透明度 0~100，0=完全不透明（默认），100=完全透明（整个填充被去掉）；仅 (0, 100) 开区间才显式写入 alpha |
| `border_color` | string | 否 | 边框颜色（RRGGBB）；空串表示无边框；传 "none" 移除边框） |
| `border_dash` | string | 否 | 边框线型，如 solid / dash / dot；空串默认 solid |
| `border_width` | number | 否 | 边框宽度（pt）；<= 0 取默认 1.0 |
| `border_alpha` | number | 否 | 边框透明度 0~100，0=完全不透明（默认），100=完全透明（去掉边框）；仅 (0, 100) 开区间才显式写入 alpha |

---

### slide_add_shapes

#### 功能说明

批量在同一幻灯片插入多个形状。shapes 为对象数组，单个对象的字段含义同 slide_add_shape（不再接受 file_id 等顶层字段）。长度必须 >= 1。【重要】每个形状必须设置 fill_color 或 border_color 中的至少一个，否则形状对用户不可见，调用会被拒绝。【重要 — 箭头消歧，与 slide_add_shape 完全一致】本工具仅支持 rightArrow / leftArrow 这两种图形预设箭头块；批量画"从起点到终点的箭头线"请改用 slide_add_line_shapes 并设置 line_type=arrow / doubleArrow。用户说"加箭头"时请先确认要"线条箭头"还是"图形箭头"

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始 |
| `shapes` | array | 否 | 待插入的形状列表，单元素字段：x / y / w / h / shape_type / fill_color / fill_alpha / border_color / border_dash / border_width / border_alpha。shape_type 可选 rect / ellipse / triangle / rightArrow / leftArrow 等；方向性箭头线条请改用 slide_add_line_shapes |

---

### slide_add_line_shape

#### 功能说明

在指定幻灯片插入一根线形 / 方向性箭头线条（从 (x1,y1) 起点到 (x2,y2) 终点）。这是绘制"两点之间的箭头线"的正确工具：line_type=arrow 单端箭头、doubleArrow 双端箭头、line 普通直线。slide_add_shape 不能画这种线条箭头，它只支持 rightArrow / leftArrow 两种固定方向的图形块预设。x1/y1 / x2/y2 单位为磅（pt，1pt = 12700 EMU）；width 单位为 pt

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始 |
| `x1` | number | 否 | 起点横坐标（磅 pt，1pt = 12700 EMU） |
| `y1` | number | 否 | 起点纵坐标（磅 pt，1pt = 12700 EMU） |
| `x2` | number | 否 | 终点横坐标（磅 pt，1pt = 12700 EMU） |
| `y2` | number | 否 | 终点纵坐标（磅 pt，1pt = 12700 EMU） |
| `line_type` | string | 否 | 线形类型：line / arrow / doubleArrow；空串默认 line |
| `color` | string | 否 | 颜色（RRGGBB）；空串默认 000000 |
| `dash` | string | 否 | 线型：solid / dash / dot；空串默认 solid |
| `width` | number | 否 | 线宽（pt）；<= 0 取默认 1.0 |

---

### slide_add_line_shapes

#### 功能说明

批量在同一幻灯片插入多根线形 / 方向性箭头线条。lines 为对象数组，单元素字段含义同 slide_add_line_shape。长度必须 >= 1。这是批量绘制"两点之间的方向性箭头线"的正确工具（line_type=arrow / doubleArrow）；slide_add_shapes 不能批量画线条箭头，它只支持 rightArrow / leftArrow 两种图形预设块

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始 |
| `lines` | array | 否 | 待插入的线形列表，单元素字段：x1 / y1 / x2 / y2 / line_type / color / dash / width |

---

### slide_add_image

#### 功能说明

在指定幻灯片插入一张图片。

#### 图片来源选择优先级（按推荐顺序）

**方式一（⭐首选）：本地脚本（`insert_image.py`）通过 HTTP 直接调用 MCP endpoint 插入图片。**
无需把图片 base64 通过 MCP 通道透传，避免大图触发上下文/传输限制，速度也更快。

脚本不允许硬编码任何凭证，支持两种认证模式：
1. **直连模式（默认）**：自动从 WorkBuddy 连接器配置读取 headers，直连腾讯文档 API
2. **手动模式（`--auth`）**：手动传入 Authorization token（与调用方当前连接 MCP server 使用的同一 token）

`--mcp-url` 由调用方（agent）根据实际使用的端点传入：
- SaaS 端: `https://saas.docs.qq.com/api/v6/open/agent/mcp`
- C 端: `https://docs.qq.com/api/v6/slide/mcp`

调用示例：
```bash
# 默认直连 (零配置，自动从 WorkBuddy 读取 token)
python3 insert_image.py \
    --mcp-url "https://saas.docs.qq.com/api/v6/open/agent/mcp" \
    --image-path /tmp/chart.png \
    --tool-args '{"file_url":"https://docs.qq.com/slide/XXX","page_index":0,"x":50,"y":50,"w":400,"h":300}'

# 手动 auth (兼容 C 端原始方式)
python3 insert_image.py \
    --mcp-url "https://docs.qq.com/api/v6/slide/mcp" \
    --auth "<your_mcp_authorization_token>" \
    --image-path /tmp/chart.png \
    --tool-args '{"file_url":"https://docs.qq.com/slide/XXX","page_index":0,"x":50,"y":50,"w":400,"h":300}'
```

脚本完整代码（可直接落盘为 `insert_image.py` 后执行）：
```python
#!/usr/bin/env python3
"""通过 MCP HTTP 接口上传本地图片到腾讯文档幻灯片（slide_add_image 兜底方案）。

支持两种认证模式:
  1. 直连模式 (默认): 自动从 WorkBuddy 连接器配置读取 token，直连腾讯文档 API
  2. 手动模式 (--auth): 手动传入 Authorization 头 (原始 C 端方式)

--mcp-url 由调用方根据实际端点传入:
  SaaS 端: https://saas.docs.qq.com/api/v6/open/agent/mcp
  C 端:    https://docs.qq.com/api/v6/slide/mcp

用法:
  # 默认直连 (零配置，自动从 WorkBuddy 读取 token)
  python3 add_image.py --mcp-url "https://saas.docs.qq.com/..." \
      --image-path /tmp/chart.png \
      --tool-args '{"file_id":"DXXXXXX","page_index":0,"x":100,"y":100,"w":500,"h":300}'

  # 手动 auth (兼容 C 端原始方式)
  python3 add_image.py --mcp-url "https://docs.qq.com/..." \
      --auth "Bearer xxx" --image-path /tmp/chart.png \
      --tool-args '{"file_id":"DXXXXXX","page_index":0,"x":100,"y":100,"w":500,"h":300}'
"""

import argparse
import base64
import glob
import json
import os
import sys
import urllib.request


# ─── 默认路径常量 ───
WORKBUDDY_HOME = os.path.expanduser("~/.workbuddy")
CONNECTORS_DIR = os.path.join(WORKBUDDY_HOME, "connectors")

# SaaS 端点特征 (用于判断工具名是否需要 slide. 前缀)
SAAS_HOST = "saas.docs.qq.com"

DEFAULT_COOKIE = (
    "env_name=feature/seperate_from_tencent_docs_skill; "
    "env_id=sit-f93f7f10;"
)


# ─── 配置自动发现 ───

def discover_tdocs_auth():
    """从 WorkBuddy 连接器配置自动读取腾讯文档的认证 headers。"""
    if not os.path.isdir(CONNECTORS_DIR):
        return None

    for instance_dir in glob.glob(os.path.join(CONNECTORS_DIR, "*")):
        if not os.path.isdir(instance_dir):
            continue

        states_file = os.path.join(instance_dir, "connector-states.json")
        mcp_file = os.path.join(instance_dir, "mcp.json")

        if not os.path.isfile(states_file) or not os.path.isfile(mcp_file):
            continue

        try:
            with open(states_file, "r") as f:
                states = json.load(f)
            if "tencent-docs" not in states.get("enabled", []):
                continue

            with open(mcp_file, "r") as f:
                mcp_cfg = json.load(f)

            tdocs = mcp_cfg.get("mcpServers", {}).get("connector:tencent-docs", {})
            header_overrides = states.get("headerOverrides", {}).get("tencent-docs", {})
            static_headers = tdocs.get("headers", {})
            headers = {**static_headers, **header_overrides}
            return headers

        except Exception:
            continue

    return None


# ─── MCP 调用 ───

def call_tool_direct(mcp_url, headers, tool_name, tool_args, cookie=""):
    """直连腾讯文档 MCP (使用从配置读取的认证头)。"""
    payload = {
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {"name": tool_name, "arguments": tool_args},
        "id": 1,
    }
    req_headers = {"Content-Type": "application/json", **headers}
    if cookie:
        req_headers["Cookie"] = cookie
    req = urllib.request.Request(
        mcp_url,
        data=json.dumps(payload).encode("utf-8"),
        headers=req_headers,
    )
    with urllib.request.urlopen(req, timeout=60) as resp:
        body = resp.read().decode("utf-8")
        return json.loads(body)


def call_tool_manual(mcp_url, auth, tool_name, tool_args, cookie=""):
    """手动传入 Authorization 头调用 (C 端原始方式)。"""
    payload = {
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {"name": tool_name, "arguments": tool_args},
        "id": 1,
    }
    req_headers = {
        "Content-Type": "application/json",
        "Authorization": auth,
    }
    if cookie:
        req_headers["Cookie"] = cookie
    req = urllib.request.Request(
        mcp_url,
        data=json.dumps(payload).encode("utf-8"),
        headers=req_headers,
    )
    with urllib.request.urlopen(req, timeout=60) as resp:
        body = resp.read().decode("utf-8")
        return json.loads(body)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--mcp-url", default="",
                        help="MCP endpoint URL (必填，如 https://docs.qq.com/api/v6/slide/mcp)")
    parser.add_argument("--auth", default="",
                        help="手动传入 Authorization 头 (如 'Bearer xxx')，跳过自动发现")
    parser.add_argument("--image-path", required=True, help="本地图片绝对路径")
    parser.add_argument("--tool-name", default="",
                        help="MCP 工具名，默认为空时自动根据 --mcp-url 判断；"
                             "手动指定则直接使用")
    parser.add_argument("--tool-args", default="{}",
                        help="工具入参 JSON（content 字段会被脚本自动覆盖）")
    parser.add_argument("--cookie", default=DEFAULT_COOKIE,
                        help="请求 Cookie 头，默认带 sit 测试环境染色 (env_name + env_id)")
    args = parser.parse_args()

    # ── 确定调用模式 ──
    if not args.mcp_url:
        print("Error: --mcp-url is required", file=sys.stderr)
        sys.exit(1)
    mcp_url = args.mcp_url

    # 根据 URL 判断端点类型：SaaS 端点 (saas.docs.qq.com) 工具名需要 "slide." 前缀
    is_saas = SAAS_HOST in mcp_url
    tool_name = args.tool_name
    if not tool_name:
        tool_name = "slide.slide_add_image" if is_saas else "slide_add_image"

    if args.auth:
        call_fn = call_tool_manual
        call_kwargs = {"mcp_url": mcp_url, "auth": args.auth, "cookie": args.cookie}
        print(f"[*] Mode: manual auth -> {mcp_url}", file=sys.stderr)
    else:
        headers = discover_tdocs_auth()
        if not headers:
            print("Error: 无法自动发现认证信息。请使用 --auth 手动传入，"
                  "或在 WorkBuddy 中连接腾讯文档连接器",
                  file=sys.stderr)
            sys.exit(1)
        call_fn = call_tool_direct
        call_kwargs = {"mcp_url": mcp_url, "headers": headers, "cookie": args.cookie}
        print(f"[*] Mode: direct (auto-auth) -> {mcp_url}", file=sys.stderr)

    with open(args.image_path, "rb") as f:
        b64 = base64.b64encode(f.read()).decode("utf-8")

    tool_args = json.loads(args.tool_args)
    tool_args["content"] = b64

    resp = call_fn(tool_name=tool_name, tool_args=tool_args, **call_kwargs)
    print(json.dumps(resp, ensure_ascii=False))
    try:
        if resp.get("error"):
            sys.exit(1)
    except Exception:
        pass


if __name__ == "__main__":
    main()
```

**方式二（备选）：传 `image_id`** —— 当无法在本地执行脚本，但可以先通过 `upload_image` MCP 工具
或腾讯文档开放平台 OpenAPI 把图片上传后获取 `image_id` 时使用。详见下方 `image_id` 字段说明。

**方式三（兜底）：传 `content`（图片 base64 字符串，不含 data URI 前缀）** —— 仅适合图片体积较小的场景；
若 base64 超出 MCP 单次传输限制会失败，此时请回退到方式一脚本调用。

#### 限制
- 单张图片大小不超过 10MB；
- 支持 PNG / JPG / JPEG / GIF / BMP / WEBP / SVG 格式；

#### ⭐图片等比缩放（强制，禁止违反）
- 入参 `w` / `h` 必须严格保持图片**原始宽高比**（uniform scale only）：即 `w / h == 原图 width / height`；绝对禁止只调宽不调高、或只调高不调宽——任何横向单独缩放 / 纵向单独缩放都会导致图片变形（人物拉宽 / 圆变椭圆 / 文字斜体感），属于硬性禁令；
- 推荐做法：先取得图片原始尺寸（通过 metadata 或 `PIL.Image.size` 等），按目标占位框 `(W, H)` 计算 `s = min(W/origW, H/origH)`，最终传入 `w = origW * s`，`h = origH * s`；剩余空间靠 `x / y` 居中或留白；
- 简化：若仅指定 `w`，则按 `h = w * origH / origW` 推算 `h`；仅指定 `h` 同理；`w / h` 同时 `<= 0` 时使用图片自身原始尺寸；
- **如果图片在目标占位框内放不下（按等比缩放后会留大块空白或溢出画布）**，禁止通过非等比拉伸强行填满——应**重新设计该页面布局**：调整图片占位框的纵横比、改用裁剪式构图、换图、或调用 `slide_get_page_info` 取画布实际尺寸后重排版式；
- 画布默认 16:9 = 960 × 540 pt；调用 `slide_get_page_info` 获取实际尺寸后再决定占位框范围。


#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始 |
| `x` | number | 否 | 图片左上角横坐标（磅 pt，1pt = 12700 EMU） |
| `y` | number | 否 | 图片左上角纵坐标（磅 pt，1pt = 12700 EMU） |
| `w` | number | 否 | 图片宽度（磅 pt，1pt = 12700 EMU），<=0 时使用图片自身原始宽度 |
| `h` | number | 否 | 图片高度（磅 pt，1pt = 12700 EMU），<=0 时使用图片自身原始高度 |
| `image_id` | string | 否 | 已上传图片的加密 ID（有效期一天，方式二备选），与 content 二选一。适合图片体积较大、base64 内容超出 MCP 单次传输限制的场景；如果可以在本地执行脚本，建议优先使用本工具描述中【方式一】的 insert_image.py。  【推荐获取方式】调用 tencent-docs MCP 的 `upload_image` 工具，传入： - `image_base64`：图片文件的实际 base64 编码内容（不含 data URI 前缀，不要传文件路径或 URL），≤10MB； - `file_name`：带扩展名的图片文件名（如 `photo.png`），支持 PNG / JPG / JPEG / GIF / BMP / WEBP / SVG。 返回结构 `{"image_id": "...", "error": "", "trace_id": "..."}`，将 `image_id` 字段值传入此处。  【兜底获取方式】如果图片体积过大，连 `upload_image` 的 `image_base64` 入参也超出 MCP 单次传输限制，请引导用户走腾讯文档开放平台 OpenAPI 上传图片： 1. 提示用户访问 [https://docs.qq.com/open/developers/?nlc=1#/login](https://docs.qq.com/open/developers/?nlc=1#/login) 登录开放平台后台，完成 OAuth 授权流程后获取以下 3 个凭证并交给本工具调用方：`Access-Token`、`Client-Id`、`Open-Id`； 2. 调用方拿到 3 个凭证后，使用 multipart/form-data 形式直接传图片文件（无需 base64 编码，不受 MCP 单次传输限制约束），示例命令： ```bash curl --location --request POST 'https://docs.qq.com/openapi/resources/v2/images' \   --header 'Access-Token: ACCESS_TOKEN' \   --header 'Client-Id: CLIENT_ID' \   --header 'Open-Id: OPEN_ID' \   --form 'image=@"/path/to/your/image.png"' ``` 3. 从返回 JSON 中取 `imageID` 字段值传入此处即可。  【限制】image_id 仅对当前账号有效，自上传起 1 天后过期；过期后需重新上传。 |
| `content` | string | 否 | 图片的 base64 编码内容（不含 data URI 前缀，兜底方式三，仅适合小图）；与 image_id 二选一。大图请优先使用本工具描述中【方式一】的本地脚本（slide_add_image 工具名），或【方式二】通过 upload_image / 开放平台 OpenAPI 上传后传 image_id |

---

### slide_add_text

#### 功能说明

在指定幻灯片插入一个文本框。x/y/w/h 单位为磅（pt，1pt = 12700 EMU），w/h 必须 > 0；text 为空串表示空文本框；font_color / fill_color / border_color 均为 6 位 hex（无 # 前缀）；font_size 单位为 pt，<= 0 取默认字号；border_width 单位为 pt，<= 0 取默认 1.0

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始 |
| `x` | number | 否 | 文本框左上角横坐标（磅 pt，1pt = 12700 EMU） |
| `y` | number | 否 | 文本框左上角纵坐标（磅 pt，1pt = 12700 EMU） |
| `w` | number | 否 | 文本框宽度（磅 pt，1pt = 12700 EMU），必须 > 0 |
| `h` | number | 否 | 文本框高度（磅 pt，1pt = 12700 EMU），必须 > 0 |
| `text` | string | 否 | 文本内容；空串表示空文本框 |
| `font_color` | string | 否 | 字体颜色（RRGGBB，无 # 前缀）；空串使用默认色 |
| `font_name` | string | 否 | 字体名称；空串使用默认字体 |
| `font_size` | number | 否 | 字号（pt）；<= 0 时使用默认字号 |
| `fill_color` | string | 否 | 文本框背景填充色（RRGGBB）；空串表示透明无填充 |
| `border_color` | string | 否 | 文本框边框色（RRGGBB）；空串表示无边框 |
| `border_dash` | string | 否 | 边框线型，如 solid / dash / dot；空串默认 solid |
| `border_width` | number | 否 | 边框宽度（pt）；<= 0 取默认 1.0 |

---

### slide_add_texts

#### 功能说明

批量在同一张幻灯片插入多个文本框，单次产生一个 SlideCommand（一次广播），比循环调用 slide_add_text 更高效。texts 为对象数组，单元素字段含义同 slide_add_text（不再接受 file_id 等顶层字段）。长度必须 >= 1

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始 |
| `texts` | array | 否 | 待插入的文本框列表，单元素字段：x / y / w / h / text / font_color / font_name / font_size / fill_color / border_color / border_dash / border_width |

---

### slide_reorder_shape

#### 功能说明

调整指定形状的 z-order 层级。op 取值：0=置于顶层 / 1=置于底层 / 2=上移一层 / 3=下移一层。shape_ids 为待调整顺序的形状 ID 列表，长度必须 >= 1

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始 |
| `shape_ids` | array | 否 | 待调整 z-order 的形状 ID 字符串数组，长度 >= 1 |
| `op` | number | 否 | z-order 操作类型：0=置顶 / 1=置底 / 2=上移 / 3=下移 |

---

### slide_remove_shapes

#### 功能说明

从指定幻灯片删除一个或多个形状。shape_ids 为待删除的形状 ID 列表，长度必须 >= 1

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始 |
| `shape_ids` | array | 否 | 待删除的形状 ID 字符串数组，长度 >= 1 |

---

### slide_get_shape_info

#### 功能说明

查询指定幻灯片中某个形状的详细信息（包含 type / preset_geom / bounds / text / fill / border / 字体属性 等 16 个字段）。仅支持 SHAPE / CONNECTOR / PICTURE 三种类型；其他类型（如 GROUP / TABLE / CHART）返回的 type 字段会标识为对应类型，但部分字段（fill / border 等）可能为空

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始 |
| `shape_id` | string | 否 | 目标形状的 shape_id |

---

### slide_set_shape_properties

#### 功能说明

修改幻灯片中一个或多个形状的属性。支持三种使用模式：\n  (A) 单形状模式：传入 shape_id + 属性字段（位置/尺寸/旋转/填充/边框）。\n  (B) 批量一对一模式：传入 shapes 数组，每项包含 shape_id 及各自的属性（填充/边框/位置/尺寸/旋转）。视觉属性变更合并为一次撤销单元，transform 变更合并为另一次撤销单元。\n  (C) 批量多对一模式：传入 shape_ids 数组 + 顶层属性，所有形状统一应用相同变更。\n未指定的属性保持不变。\n形状类型支持说明：\n  - 普通形状(shape/connector)：支持 fill_color / fill_alpha / border_color / border_alpha / border_width / border_dash / 位置尺寸旋转。\n  - 图片形状(picture)：支持 fill_alpha（控制整张图片透明度）/ border_color / border_alpha / border_width / border_dash / 位置尺寸旋转；不支持 fill_color（图片颜色由图片本身决定）。\n  - 其他类型（表格、图表等）：仅支持位置/尺寸/旋转，不支持 fill/border 颜色属性。\n坐标 / 尺寸单位为磅（pt，1pt = 12700 EMU）；rotation 单位为度；border_width 单位为磅（pt）；border_dash 为 OOXML STPresetLineDashVal 字符串（如 "solid" / "dash" / "dashDot"）

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始 |
| `shape_id` | string | 否 | 单形状模式 (A)：要修改的形状 ID |
| `shapes` | array | 否 | 批量一对一模式 (B)：对象数组，每项包含 shape_id 及其独立的属性（fill_color/fill_alpha/border_color/border_alpha/border_width/border_dash/x/y/w/h/rotation）。与 shape_id、shape_ids 互斥 |
| `shape_ids` | array | 否 | 批量多对一模式 (C)：形状 ID 数组，所有形状统一应用顶层属性。与 shape_id、shapes 互斥 |
| `fill_color` | string | 否 | 十六进制填充色（如 "FF0000"）。传 "none" 移除填充。不传则保持不变。图片形状不支持此属性 |
| `fill_alpha` | number | 否 | 填充透明度，百分比 0-100（0=完全不透明，100=完全透明）。对普通形状控制 solidFill 透明度；对图片形状控制图片整体透明度（alphaModFix）。设置 fill_color 时若不传则默认 0 |
| `border_color` | string | 否 | 十六进制边框色（如 "000000"）。传 "none" 移除边框。不传则保持不变。普通形状和图片形状均支持 |
| `border_alpha` | number | 否 | 边框透明度，百分比 0-100（0=完全不透明，100=完全透明）。设置 border_color 时若不传则默认 0。普通形状和图片形状均支持 |
| `border_width` | number | 否 | 边框宽度，单位磅（1pt=12700 EMU）。不传则保持不变 |
| `border_dash` | string | 否 | 边框线型：solid/dot/dash/lgDash/dashDot/lgDashDot/lgDashDotDot/sysDash/sysDot/sysDashDot/sysDashDotDot。不传则保持不变 |
| `x` | number | 否 | 新的 X 坐标，单位磅。不传则保持不变 |
| `y` | number | 否 | 新的 Y 坐标，单位磅。不传则保持不变 |
| `w` | number | 否 | 新的宽度，单位磅。不传则保持不变 |
| `h` | number | 否 | 新的高度，单位磅。不传则保持不变 |
| `rotation` | number | 否 | 旋转角度（度），如 90 表示顺时针旋转 90°。不传则保持不变 |

---

## 文本工具组（Text）

### slide_append_text

#### 功能说明

追加文本

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `shape_id` | string | 否 | shape ID |
| `text` | string | 否 | 追加文本 |
| `page_index` | integer | 否 | 幻灯片页索引（0-based） |

---

### slide_insert_text

#### 功能说明

插入文本

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `shape_id` | string | 否 | shape ID |
| `index` | integer | 否 | 插入位置 |
| `text` | string | 否 | 插入文本 |
| `page_index` | integer | 否 | 幻灯片页索引（0-based） |

---

### slide_delete_text

#### 功能说明

删除文本

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `shape_id` | string | 否 | shape ID |
| `index` | integer | 否 | 起始位置 |
| `count` | integer | 否 | 删除长度 |
| `page_index` | integer | 否 | 幻灯片页索引（0-based） |

---

### slide_find_text

#### 功能说明

查找文本

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `search` | string | 否 | 搜索文本 |

---

### slide_find_replace_text

#### 功能说明

查找替换文本

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `search` | string | 否 | 搜索文本 |
| `replace` | string | 否 | 替换文本 |
| `page_index` | integer | 否 | 幻灯片页索引（0-based） |

---

### slide_set_text_property

#### 功能说明

设置文本格式

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `shape_id` | string | 否 | shape ID |
| `index` | integer | 否 | 起始字符偏移（utf-16 code unit） |
| `count` | integer | 否 | 区间字符数 |
| `color` | string | 否 | 文字颜色 RRGGBB |
| `font_size` | number | 否 | 字号 pt；<=0 表示不修改 |
| `font_name` | string | 否 | 字体名 |
| `bold` | bool | 否 | 粗体 |
| `italic` | bool | 否 | 斜体 |
| `underline` | string | 否 | 下划线类型，如 single / double / none |
| `strikethrough` | string | 否 | 删除线类型，如 single / double / none |
| `letter_spacing` | number | 否 | 字符间距 |
| `baseline` | number | 否 | 基线偏移 |

---

### slide_get_text

#### 功能说明

读取文本

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `shape_id` | string | 否 | shape ID |
| `page_index` | integer | 否 | 幻灯片页索引（0-based） |

---

## 表格工具组（Table）

### slide_add_table

#### 功能说明

在指定幻灯片页创建空表格，page_index 从 0 开始计数；坐标与尺寸单位为磅（pt，1pt = 12700 EMU）

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始计数 |
| `x` | number | 否 | 表格左上角 X 坐标，单位为磅（pt，1pt = 12700 EMU） |
| `y` | number | 否 | 表格左上角 Y 坐标，单位为磅（pt，1pt = 12700 EMU） |
| `w` | number | 否 | 表格总宽度，单位为磅（pt），会在列之间均分 |
| `h` | number | 否 | 表格总高度，单位为磅（pt），会在行之间均分 |
| `rows` | number | 否 | 表格行数，必须大于 0 |
| `cols` | number | 否 | 表格列数，必须大于 0 |

---

### slide_insert_table_rows

#### 功能说明

在表格 shape 中插入一行或多行。index 是间隙位置：对 N 行表格，有效范围为 [0, N]；count 未传时默认 1；reference_row_index 指定复制高度的已有行

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始计数 |
| `shape_id` | string | 否 | 目标表格 shape id |
| `index` | number | 否 | 插入间隙位置；0 表示表格顶部，N 表示表格底部 |
| `reference_row_index` | number | 否 | 新行高度参考行索引（0-based）。未传时按 0 处理（=使用第 0 行的高度作为新行高度）；超出已有行数时新行使用默认行高 |
| `count` | number | 否 | 插入行数，未传时默认 1，必须大于 0 |

---

### slide_insert_table_cols

#### 功能说明

在表格 shape 中插入一列或多列。index 是间隙位置：对 M 列表格，有效范围为 [0, M]；count 未传时默认 1；插入后表格总宽度保持不变并重新均分列宽

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始计数 |
| `shape_id` | string | 否 | 目标表格 shape id |
| `index` | number | 否 | 插入间隙位置；0 表示最左侧，M 表示最右侧 |
| `count` | number | 否 | 插入列数，未传时默认 1，必须大于 0 |

---

### slide_delete_table_rows

#### 功能说明

从表格 shape 中删除一行或多行，从 index 指定的行开始；count 未传时默认 1，index + count 不能超过当前行数

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始计数 |
| `shape_id` | string | 否 | 目标表格 shape id |
| `index` | number | 否 | 起始删除行索引，从 0 开始 |
| `count` | number | 否 | 删除行数，未传时默认 1，必须大于 0 |

---

### slide_delete_table_cols

#### 功能说明

从表格 shape 中删除一列或多列，从 index 指定的列开始；count 未传时默认 1，index + count 不能超过当前列数

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始计数 |
| `shape_id` | string | 否 | 目标表格 shape id |
| `index` | number | 否 | 起始删除列索引，从 0 开始 |
| `count` | number | 否 | 删除列数，未传时默认 1，必须大于 0 |

---

### slide_merge_table_cells

#### 功能说明

合并表格 shape 中的矩形单元格区域，区域由 start_row/start_col 和 row_span/col_span 指定；合并后内容来自左上角单元格

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始计数 |
| `shape_id` | string | 否 | 目标表格 shape id |
| `start_row` | number | 否 | 合并区域左上角行索引，从 0 开始 |
| `start_col` | number | 否 | 合并区域左上角列索引，从 0 开始 |
| `row_span` | number | 否 | 合并区域行跨度，必须大于 0 |
| `col_span` | number | 否 | 合并区域列跨度，必须大于 0 |

---

### slide_unmerge_table_cells

#### 功能说明

取消表格 shape 中指定矩形区域的单元格合并，区域由 start_row/start_col 和 row_span/col_span 指定

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始计数 |
| `shape_id` | string | 否 | 目标表格 shape id |
| `start_row` | number | 否 | 区域左上角行索引，从 0 开始 |
| `start_col` | number | 否 | 区域左上角列索引，从 0 开始 |
| `row_span` | number | 否 | 区域行跨度，必须大于 0 |
| `col_span` | number | 否 | 区域列跨度，必须大于 0 |

---

### slide_set_cell_text

#### 功能说明

向表格 shape 的单个单元格写入纯文本。row 和 col 从 0 开始计数；文本可为空，不需要传入段落结束符

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始计数 |
| `shape_id` | string | 否 | 目标表格 shape id |
| `row` | number | 否 | 目标单元格行索引，从 0 开始 |
| `col` | number | 否 | 目标单元格列索引，从 0 开始 |
| `text` | string | 否 | 写入单元格的纯文本，可为空 |

---

## 图表工具组（Chart）

### slide_add_chart

#### 功能说明

创建图表

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `<addChartArgs>` | object | 否 | 更多参数详见 对应 MCP 工具的输入 Schema |

---

### slide_change_chart_type

#### 功能说明

切换图表类型

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `shape_id` | string | 否 | 图表 shape ID |
| `new_chart_type` | string | 否 | 目标图表类型 |
| `page_index` | integer | 否 | 幻灯片页索引（0-based） |

---

### slide_update_chart_data

#### 功能说明

更新图表数据

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `shape_id` | string | 否 | 图表 shape ID |
| `categories` | array | 否 | 分类标签 |
| `series` | array | 否 | 数据系列 |
| `sub_chart_index` | integer | 否 | 子图表索引 |
| `page_index` | integer | 否 | 幻灯片页索引（0-based） |

---

### slide_get_chart_info

#### 功能说明

查询图表信息

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `shape_id` | string | 否 | 图表 shape ID |
| `page_index` | integer | 否 | 幻灯片页索引（0-based） |

---

### slide_update_chart_title

#### 功能说明

更新图表标题

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `<chartTitleArgs>` | object | 否 | 更多参数详见 对应 MCP 工具的输入 Schema |

---

### slide_update_chart_legend

#### 功能说明

更新图表图例

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `<chartLegendArgs>` | object | 否 | 更多参数详见 对应 MCP 工具的输入 Schema |

---

### slide_update_chart_axis

#### 功能说明

更新图表坐标轴

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `<chartAxisArgs>` | object | 否 | 更多参数详见 对应 MCP 工具的输入 Schema |

---

### slide_update_chart_gridlines

#### 功能说明

更新主网格线

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `shape_id` | string | 否 | 图表 shape ID |
| `axis` | string | 否 | 网格线绑定到哪根坐标轴：value（默认；水平横线，值轴主网格线）/ category（垂直竖线，类别轴主网格线） |
| `show_major` | bool | 否 | 是否显示该轴的主网格线（true=显示；false=隐藏） |
| `page_index` | integer | 否 | 幻灯片页索引（0-based） |

---

### slide_update_chart_series_style

#### 功能说明

更新单条系列样式

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `<chartSeriesStyleArgs>` | object | 否 | 更多参数详见 对应 MCP 工具的输入 Schema |

---

### slide_update_chart_data_labels

#### 功能说明

更新数据标签

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `<chartDataLabelsArgs>` | object | 否 | 更多参数详见 对应 MCP 工具的输入 Schema |

---

## 分组工具组（Group）

### slide_get_group_info

#### 功能说明

查询组合信息

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `group_id` | string | 否 | group shape ID |
| `page_index` | integer | 否 | 幻灯片页索引（0-based） |

---

### slide_group_shapes

#### 功能说明

组合形状

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `shape_ids` | array | 否 | shape ID 列表 |
| `page_index` | integer | 否 | 幻灯片页索引（0-based） |

---

### slide_ungroup_shapes

#### 功能说明

取消组合

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `group_id` | string | 否 | group shape ID |
| `page_index` | integer | 否 | 幻灯片页索引（0-based） |

---

### slide_reorder_shapes_in_group

#### 功能说明

重排组合内形状

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `group_id` | string | 否 | group shape ID |
| `shape_ids` | array | 否 | 子 shape ID 列表 |
| `to_index` | integer | 否 | 目标层级序号 |
| `page_index` | integer | 否 | 幻灯片页索引（0-based） |

---

### slide_update_group_shape_properties

#### 功能说明

修改组合内形状属性

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `<groupPropertyArgs>` | object | 否 | 更多参数详见 对应 MCP 工具的输入 Schema |

---

## 动画工具组（Animation）

### slide_add_anim

#### 功能说明

为指定幻灯片中的某个形状添加动画效果。anim_type 为动画类型整数值（通过 slide_list_anim_types 获取可用值）；anim_subtype 为方向/子类型（仅 FLY_IN/FLY_OUT 等需要，0 表示不需要）；index 为动画在该页动画序列中的插入位置（0-based，必须 >= 0；想追加到末尾时填一个 >= 当前动画数量的值即可，例如 9999；不支持 -1）

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始 |
| `shape_id` | string | 否 | 目标形状的 shape_id |
| `anim_type` | number | 否 | 动画类型整数值 |
| `anim_subtype` | number | 否 | 动画方向/子类型整数值，0 表示不需要 |
| `index` | number | 否 | 动画在该页动画序列中的插入位置，0-based，必须 >= 0。想追加到末尾时填一个 >= 当前动画数量的值即可（例如 9999），不支持 -1。 |

---

### slide_list_anim_types

#### 功能说明

列出所有支持的动画类型，包含类型名称、分类（entrance / exit / emphasis）以及是否支持方向参数。返回的 type 整数值可直接用于 slide_add_anim 的 anim_type 参数

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一（仅用于 ticket 校验） |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |

---

### slide_remove_anim

#### 功能说明

移除指定形状的某个动画。index 为动画在形状动画序列中的位置（0-based）

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始 |
| `shape_id` | string | 否 | 目标形状的 shape_id |
| `index` | number | 否 | 要移除的动画索引（0-based） |

---

### slide_move_anim

#### 功能说明

移动指定形状的动画在序列中的位置（从 from_index 移到 to_index）

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始 |
| `shape_id` | string | 否 | 目标形状的 shape_id |
| `from_index` | number | 否 | 动画当前索引（0-based） |
| `to_index` | number | 否 | 动画目标索引（0-based） |

---

### slide_set_anim_properties

#### 功能说明

修改指定形状某个动画的类型和方向。index 为动画在序列中的索引（0-based）；anim_type 为新动画类型；anim_subtype 为新方向（0 表示不需要）

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始 |
| `shape_id` | string | 否 | 目标形状的 shape_id |
| `index` | number | 否 | 要修改的动画索引（0-based） |
| `anim_type` | number | 否 | 新的动画类型整数值 |
| `anim_subtype` | number | 否 | 新的动画方向整数值，0 表示不需要 |

---

### slide_set_anim_trigger

#### 功能说明

修改指定形状某个动画的触发方式。trigger_shape_id 为触发形状 ID，空串表示设为默认触发（即"单击"播放，跟随默认序列）

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片页索引，从 0 开始 |
| `shape_id` | string | 否 | 目标形状的 shape_id |
| `index` | number | 否 | 要修改的动画索引（0-based） |
| `trigger_shape_id` | string | 否 | 触发形状 ID，空串表示设为默认触发 |

---

## 主题工具组（Theme）

### slide_get_themes

#### 功能说明

获取当前演示文稿中嵌入的所有主题列表，返回每个主题的 theme_id 和 theme_name

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |

---

### slide_list_builtin_themes

#### 功能说明

列出服务端所有内置（预置）主题，返回每个主题的 theme_id 和 theme_name；不依赖文档状态，可在调用 slide_set_theme 前用于查询可用的内置主题 ID

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 可选，仅用于 trace/log |

---

### slide_set_theme

#### 功能说明

设置演示文稿的主题。支持两种模式：(1) switch：传入已存在于文档中的 theme_id，切换到该主题；(2) builtin：is_builtin=true，传入内置主题 ID（可通过 slide_list_builtin_themes 获取），由服务端自动加载内置 ThemeElements

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `theme_id` | string | 否 | 目标主题 ID（必填） |
| `is_builtin` | bool | 否 | 是否使用内置主题（true 时 theme_id 须为内置主题 ID） |

---

## 批注工具组（Comment）

### slide_get_comments

#### 功能说明

查询批注

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `page_index` | integer | 否 | 页索引（0-based）；显式传 -1 表示全部页；未传或 0 仅返回第 1 页 |

---

### slide_add_comment

#### 功能说明

添加批注

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `<commentArgs>` | object | 否 | 更多参数详见 对应 MCP 工具的输入 Schema |

---

### slide_remove_comment

#### 功能说明

删除批注

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `<commentKeyArgs>` | object | 否 | 更多参数详见 对应 MCP 工具的输入 Schema |

---

### slide_modify_comment

#### 功能说明

修改批注

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `text` | string | 否 | 批注文本（覆盖式：传空串会清空原文，不修改请省略此字段） |
| `author_name` | string | 否 | 作者名称（覆盖式：传空串会清空原作者名） |
| `<commentKeyArgs>` | object | 否 | 更多参数详见 对应 MCP 工具的输入 Schema |

---

## 节工具组（Section）

### slide_get_sections

#### 功能说明

查询节列表

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |

---

### slide_add_section

#### 功能说明

在指定位置添加新节。提供 before_slide_index 或 after_section_id 之一来指定插入位置。\n- before_slide_index: 新节将拥有目标页及其后续页面（直到下一个节边界）。原来拥有该页的节将从下一页开始。\n- after_section_id: 新节作为空节插入到指定节之后，不会接管任何页面。

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `before_slide_index` | number | 否 | 0-based slide 索引，新节插入到该 slide 之前并拥有它（该 slide 成为新节的第一页）。原来包含该 slide 的节将从下一页开始。与 after_section_id 互斥 |
| `after_section_id` | string | 否 | 已有节的 ID，新节作为空节插入到该节之后（不接管任何页面）；传空字符串表示在最开头插入空节。与 before_slide_index 互斥。使用 slide_get_sections 获取有效的 section ID |
| `name` | string | 否 | 节名称 |

---

### slide_remove_sections

#### 功能说明

删除节（保留幻灯片）

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `section_ids` | array | 否 | 节 ID 列表 |

---

### slide_remove_section_with_slides

#### 功能说明

删除节及其幻灯片

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `section_id` | string | 否 | 节 ID |

---

### slide_move_section

#### 功能说明

移动节

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `section_id` | string | 否 | 节 ID |
| `to_section_index` | integer | 否 | 目标节序号 |

---

### slide_rename_section

#### 功能说明

重命名节

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `section_id` | string | 否 | 节 ID |
| `name` | string | 否 | 节名称 |

---

## 演示文稿级工具组（Presentation）

### slide_set_slide_size

#### 功能说明

设置幻灯片尺寸

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `aspect_ratio` | number | 否 | 页面宽高比：1=4:3, 2=16:9 |
| `scale_mode` | string | 否 | 缩放模式：default（等比缩放）/ no_scale（不缩放，居中）/ enlarge（向放大方向缩放）；空串默认 default |
| `scale_master_layout` | bool | 否 | 是否同步缩放 master / layout 页（默认 false） |

---

### slide_set_default_font

#### 功能说明

设置默认字体

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `latin_font` | string | 否 | 西文字体名 |
| `ea_font` | string | 否 | 东亚字体名 |
| `font_size` | number | 否 | 默认字号 pt |
| `font_color` | string | 否 | 字体颜色 RRGGBB |
| `options` | object | 否 | 额外字体选项（bold / italic / spacing 等） |

---

### slide_export_pages_to_image_urls

#### 功能说明

将指定演示文稿（PPT）页面渲染为图片，返回每页对应的图片 URL 列表。

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文件 ID（与 `file_url` 二选一） |
| `file_url` | string | 否 | 文件 URL（与 `file_id` 二选一） |
| `page_indices` | int[] | 否 | 要导出的页面索引数组（0 起始）；不填则导出全部页面 |
| `width` | int | 否 | 导出图片宽度（像素），默认 1920 |
| `height` | int | 否 | 导出图片高度（像素），默认 1080 |
| `long_edge`| int | 否 | 输出图片的长边像素（优先级最高，与 width/height 互斥）|

---

## 备注工具组（Notes）

### slide_set_notes_text

#### 功能说明

设置或覆盖指定幻灯片的备注页（演讲者备注）文本，page_index 从 0 开始计数；text 为空字符串时表示清空备注页文本

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `page_index` | number | 否 | 待设置备注的幻灯片页索引，从 0 开始计数 |
| `text` | string | 否 | 备注页文本；空字符串表示清空当前备注 |

---

## 其他

### slide_add_footer

#### 功能说明

在指定幻灯片添加或移除页脚占位符。show=true（默认）时插入页脚占位符并写入 display_text；show=false 时移除已有页脚占位符。位置和样式从布局/母版继承

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片URL，与 file_id 二选一 |
| `page_index` | number | 否 | 目标幻灯片 0-based 页索引 |
| `show` | bool | 否 | true（默认）=添加/显示页脚；false=移除/隐藏页脚 |
| `display_text` | string | 否 | 页脚文本内容，如 "Confidential" 或 "© 2026 Acme Corp"；仅 show=true 时生效 |

---

### slide_move_slides_to_section

#### 功能说明

移动幻灯片到节

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `page_indices` | array | 否 | 要移动的幻灯片 0-based 索引列表 |
| `section_id` | string | 否 | 目标节 ID |

---

### slide_reply_comment

#### 功能说明

回复批注

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 在线幻灯片的URL链接，与 file_id 二选一 |
| `page_index` | integer | 否 | 页索引 |
| `group_id` | string | 否 | 批注分组 ID |
| `text` | string | 否 | 回复文本 |
| `author_name` | string | 否 | 作者名称 |

---

## 典型工作流示例

### 在幻灯片中添加带文字的形状

```
1. 调用 slide_add_shape 创建形状，获取 shape_id
2. 调用 slide_append_text 向形状内添加文本
3. 调用 slide_set_text_property 设置文本样式（可选）
```

### 创建带数据的表格

```
1. 调用 slide_add_table 创建空表格，获取 shape_id
2. 循环调用 slide_set_cell_text 填充每个单元格的数据
3. 如需要，调用 slide_merge_table_cells 合并表头单元格
```

### 查找并替换文本

```
1. 调用 slide_find_text 查找目标文本，获取所有匹配位置
2. 确认需要替换的页面后，调用 slide_find_replace_text 在指定页执行替换
```

### 批量添加多个元素到同一页

```
1. 使用 slide_add_shapes 批量插入形状（比循环调用 slide_add_shape 更高效）
2. 使用 slide_add_texts 批量插入文本框
3. 使用 slide_add_line_shapes 批量插入连接线
```

### 管理演示文稿节

```
1. 调用 slide_get_sections 查看现有节结构
2. 调用 slide_add_section 添加新节
3. 调用 slide_move_section 调整节顺序
4. 调用 slide_rename_section 修改节名称
```

### 为形状添加动画

```
1. 调用 slide_list_anim_types 查看支持的动画类型
2. 调用 slide_add_anim 为目标形状添加动画
3. 调用 slide_set_anim_trigger 设置触发方式（可选）
```

### 切换主题

```
1. 调用 slide_list_builtin_themes 查看可用的内置主题
2. 调用 slide_set_theme（设置 is_builtin=true + 内置主题 ID）切换主题
```

### 添加图表

```
1. 调用 slide_add_chart 创建带数据的图表
2. 元素级调整样式（任选其一组合调用，未传字段不影响原值）：
   - slide_update_chart_title       —— 标题文本/字体/可见性
   - slide_update_chart_legend      —— 图例位置/字体
   - slide_update_chart_axis        —— 单根坐标轴（axis=category/value）
   - slide_update_chart_gridlines   —— 主网格线显示
   - slide_update_chart_series_style—— 单条系列填充/线条/标记
   - slide_update_chart_data_labels —— 数据标签（series_index=-1 表示全局）
3. 如需修改数据，调用 slide_update_chart_data
4. 如需更改图表类型，调用 slide_change_chart_type
```

### 插入图片

```
1.（推荐）调用 tencent-docs MCP 的 upload_image 工具上传图片，拿到 image_id
2. 调用 slide_add_image，传入 image_id（与 content 二选一）
3. 如需调整位置 / 大小，调用 slide_set_shape_properties
```

---

---

### slide_set_design

#### 功能说明

持久化 AI 产出的 DESIGN.md 到当前文档（TTL 24h）。

本工具是 slideengine MCP 写类工具的『前置门』——任何 `slide_add_slide` / `slide_add_text` / `slide_add_shape` / `slide_add_image` / `slide_set_*` 等写类工具调用前都会校验本接口是否已写入；未写入则一律拒绝写。AI 必须在拿到 `slide_get_style_design_guide` 输出后【直接】产出 DESIGN.md（无需与用户确认），然后调本工具持久化，才能开始逐页落地。

#### 写入方式选择优先级（按推荐顺序）

**方式一（⭐首选）：本地脚本（`set_design.py`）通过 HTTP 直接调用 MCP endpoint 写入。**
DESIGN.md 通常较大（数千甚至上万字），通过 MCP 通道直接传 `design_md` 字段时容易被中间链路**截断**，导致持久化后内容残缺。脚本从本地 .md 文件读取全文，规避截断风险。

脚本支持两种认证模式：
1. **直连模式（默认）**：自动从 WorkBuddy 连接器配置读取 headers，直连腾讯文档 API
2. **手动模式（`--auth`）**：手动传入 Authorization token

`--mcp-url` 由调用方根据实际端点传入：
- SaaS 端: `https://saas.docs.qq.com/api/v6/open/agent/mcp`
- C 端: `https://docs.qq.com/api/v6/slide/mcp`

调用示例：
```bash
# 默认直连 (零配置，自动从 WorkBuddy 读取 token)
python3 set_design.py \
    --mcp-url "https://saas.docs.qq.com/api/v6/open/agent/mcp" \
    --design-path /tmp/DESIGN.md \
    --tool-args '{"file_url":"https://docs.qq.com/slide/XXX"}'

# 手动 auth (兼容 C 端原始方式)
python3 set_design.py \
    --mcp-url "https://docs.qq.com/api/v6/slide/mcp" \
    --auth "<your_mcp_authorization_token>" \
    --design-path /tmp/DESIGN.md \
    --tool-args '{"file_url":"https://docs.qq.com/slide/XXX"}'
```

脚本完整代码（可直接落盘为 `set_design.py` 后执行）：
```python
#!/usr/bin/env python3
"""通过 MCP HTTP 接口把本地 DESIGN.md 持久化到腾讯文档幻灯片
（slide_set_design 兜底方案，避免大文本通过 MCP 通道截断）。

支持两种认证模式:
  1. 直连模式 (默认): 自动从 WorkBuddy 连接器配置读取 token，直连腾讯文档 API
  2. 手动模式 (--auth): 手动传入 Authorization 头 (原始 C 端方式)

--mcp-url 由调用方根据实际端点传入:
  SaaS 端: https://saas.docs.qq.com/api/v6/open/agent/mcp
  C 端:    https://docs.qq.com/api/v6/slide/mcp

用法:
  # 默认直连 (零配置，自动从 WorkBuddy 读取 token)
  python3 set_design.py --mcp-url "https://saas.docs.qq.com/..." \
      --design-path /tmp/DESIGN.md \
      --tool-args '{"file_id":"DXXXXXX"}'

  # 手动 auth (兼容 C 端原始方式)
  python3 set_design.py --mcp-url "https://docs.qq.com/..." \
      --auth "Bearer xxx" \
      --design-path /tmp/DESIGN.md \
      --tool-args '{"file_id":"DXXXXXX"}'
"""

import argparse
import glob
import json
import os
import sys
import urllib.request


# ─── 默认路径常量 ───
WORKBUDDY_HOME = os.path.expanduser("~/.workbuddy")
CONNECTORS_DIR = os.path.join(WORKBUDDY_HOME, "connectors")

# SaaS 端点特征 (用于判断工具名是否需要 slide. 前缀)
SAAS_HOST = "saas.docs.qq.com"

DEFAULT_COOKIE = (
    "env_name=feature/seperate_from_tencent_docs_skill; "
    "env_id=sit-f93f7f10;"
)


# ─── 配置自动发现 ───

def discover_tdocs_auth():
    """从 WorkBuddy 连接器配置自动读取腾讯文档的认证 headers。"""
    if not os.path.isdir(CONNECTORS_DIR):
        return None

    for instance_dir in glob.glob(os.path.join(CONNECTORS_DIR, "*")):
        if not os.path.isdir(instance_dir):
            continue

        states_file = os.path.join(instance_dir, "connector-states.json")
        mcp_file = os.path.join(instance_dir, "mcp.json")

        if not os.path.isfile(states_file) or not os.path.isfile(mcp_file):
            continue

        try:
            with open(states_file, "r") as f:
                states = json.load(f)
            if "tencent-docs" not in states.get("enabled", []):
                continue

            with open(mcp_file, "r") as f:
                mcp_cfg = json.load(f)

            tdocs = mcp_cfg.get("mcpServers", {}).get("connector:tencent-docs", {})
            header_overrides = states.get("headerOverrides", {}).get("tencent-docs", {})
            static_headers = tdocs.get("headers", {})
            headers = {**static_headers, **header_overrides}
            return headers

        except Exception:
            continue

    return None


# ─── MCP 调用 ───

def call_tool_direct(mcp_url, headers, tool_name, tool_args, cookie=""):
    """直连腾讯文档 MCP (使用从配置读取的认证头)。"""
    payload = {
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {"name": tool_name, "arguments": tool_args},
        "id": 1,
    }
    req_headers = {"Content-Type": "application/json", **headers}
    if cookie:
        req_headers["Cookie"] = cookie
    req = urllib.request.Request(
        mcp_url,
        data=json.dumps(payload).encode("utf-8"),
        headers=req_headers,
    )
    with urllib.request.urlopen(req, timeout=60) as resp:
        body = resp.read().decode("utf-8")
        return json.loads(body)


def call_tool_manual(mcp_url, auth, tool_name, tool_args, cookie=""):
    """手动传入 Authorization 头调用 (C 端原始方式)。"""
    payload = {
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {"name": tool_name, "arguments": tool_args},
        "id": 1,
    }
    req_headers = {
        "Content-Type": "application/json",
        "Authorization": auth,
    }
    if cookie:
        req_headers["Cookie"] = cookie
    req = urllib.request.Request(
        mcp_url,
        data=json.dumps(payload).encode("utf-8"),
        headers=req_headers,
    )
    with urllib.request.urlopen(req, timeout=60) as resp:
        body = resp.read().decode("utf-8")
        return json.loads(body)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--mcp-url", default="",
                        help="MCP endpoint URL (必填，如 https://docs.qq.com/api/v6/slide/mcp)")
    parser.add_argument("--auth", default="",
                        help="手动传入 Authorization 头 (如 'Bearer xxx')，跳过自动发现")
    parser.add_argument("--design-path", required=True, help="本地 DESIGN.md 绝对路径")
    parser.add_argument("--tool-name", default="",
                        help="MCP 工具名，默认为空时自动根据 --mcp-url 判断；"
                             "手动指定则直接使用")
    parser.add_argument("--tool-args", default="{}",
                        help="工具入参 JSON（design_md 字段会被脚本自动覆盖）")
    parser.add_argument("--cookie", default=DEFAULT_COOKIE,
                        help="请求 Cookie 头，默认带 sit 测试环境染色 (env_name + env_id)")
    args = parser.parse_args()

    # ── 确定调用模式 ──
    if not args.mcp_url:
        print("Error: --mcp-url is required", file=sys.stderr)
        sys.exit(1)
    mcp_url = args.mcp_url

    # 根据 URL 判断端点类型：SaaS 端点 (saas.docs.qq.com) 工具名需要 "slide." 前缀
    is_saas = SAAS_HOST in mcp_url
    tool_name = args.tool_name
    if not tool_name:
        tool_name = "slide.slide_set_design" if is_saas else "slide_set_design"

    if args.auth:
        call_fn = call_tool_manual
        call_kwargs = {"mcp_url": mcp_url, "auth": args.auth, "cookie": args.cookie}
        print(f"[*] Mode: manual auth -> {mcp_url}", file=sys.stderr)
    else:
        headers = discover_tdocs_auth()
        if not headers:
            print("Error: 无法自动发现认证信息。请使用 --auth 手动传入，"
                  "或在 WorkBuddy 中连接腾讯文档连接器",
                  file=sys.stderr)
            sys.exit(1)
        call_fn = call_tool_direct
        call_kwargs = {"mcp_url": mcp_url, "headers": headers, "cookie": args.cookie}
        print(f"[*] Mode: direct (auto-auth) -> {mcp_url}", file=sys.stderr)

    with open(args.design_path, "r", encoding="utf-8") as f:
        design_md = f.read()

    tool_args = json.loads(args.tool_args)
    tool_args["design_md"] = design_md

    resp = call_fn(tool_name=tool_name, tool_args=tool_args, **call_kwargs)
    print(json.dumps(resp, ensure_ascii=False))
    try:
        if resp.get("error"):
            sys.exit(1)
    except Exception:
        pass


if __name__ == "__main__":
    main()
```

**方式二（兜底）：直接传 `design_md` 字段** —— 仅适合内容较短的场景；若全文超出 MCP 单次传输限制会被截断，此时请回退到方式一脚本调用。

#### 参数说明

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `file_id` | string | 否 | 文档ID，与 file_url 二选一 |
| `file_url` | string | 否 | 文档URL，与 file_id 二选一 |
| `design_md` | string | 否 | DESIGN.md 全文：基于选定风格的版式分类 / 色板 / 字体 / 装饰 / 自检条件等设计契约。非 Required（推荐方式一脚本注入）；方式二直接调用时必填 |

---

## 注意事项

- 仅支持演示文稿（幻灯片/PPT）类型
- `page_index` 从 0 开始计数，第 1 页 = `page_index: 0`
- 坐标与尺寸单位为 **磅（pt）**，1pt = 12700 EMU
- 边框宽度单位为 **pt**（磅）；EMU 换算：`1 pt = 12700 EMU`
- 操作前需确保拥有文档的编辑权限
- 写类工具大部分返回空消息（实际结果通过 MCP TextContent 透出）
- 批量操作（`slide_add_shapes` / `slide_add_texts` / `slide_add_line_shapes`）比循环单个调用更高效，建议优先使用
- 连续多步编辑时，建议将上一步返回的 `version` / `new_version` 传入下一步的 `version_info.base_version`
- `slide_find_text` 全文搜索，`slide_find_replace_text` 按页替换
- 表格操作需要先通过 `slide_add_table` 获取 `shape_id`，后续所有行列操作都基于该 `shape_id`
- 动画类型的整数值通过 `slide_list_anim_types` 查询，不要硬编码
- 形状的 `shape_id` 通过读类接口（如 `slide_get_shape_info` / `slide_get_page_info`）获取
- 颜色值统一使用 6 位十六进制 RRGGBB 格式（不含 `#` 前缀）
- 图片插入推荐通过 tencent-docs MCP 的 `upload_image` 拿 `image_id`，避免大 base64 撑爆 MCP 单次传输
- 本文档由 `slideengine_mcp_tools.go` 自动派生（参考脚本 `.tmp/gen_slideengine_md.py`）。如与工具输入 Schema 冲突，以 Schema 为准。
