# DOC 编辑引擎 API 参考

本文件包含腾讯文档 DOC 编辑引擎（docengine）的所有工具 API 说明。这些工具专用于 Word 文档的编辑操作，包括插入 Markdown、文本插入、替换、查找、段落设置、文本属性修改、任务插入、图片插入、分页符和表格插入等。

> ⚠️ **注意**：本文档中的工具仅适用于 **Word 文档（doc_type: word）** 类型，不适用于智能文档（smartcanvas）等其他类型。

---

## 服务信息

| 项目     | 说明                                                                          |
| -------- | ----------------------------------------------------------------------------- |
| 所属服务 | `tdoc-doc`                                                                     |
| 服务地址 | `https://docs.qq.com/api/v6/doc/mcp`                                          |
| 工具前缀 | `doc.*`（如 `doc.insert_markdown`、`doc.get_outline`、`doc.find` 等）         |
| 调用方式 | 通过 MCP 协议调用 tdoc-doc 服务，使用与 tencent-docs 共享的 Authorization Token |
| Token    | 与 tencent-docs / tdoc-slide / tdoc-sheet 共用同一 Token，完成授权（`references/auth.md`）后自动配置 |
| 文档类型 | 仅支持 Word 文档类型（`doc_type: word`）                                      |

> ⚠️ **所有 `doc.*` 工具均通过 `tdoc-doc` 独立 MCP 服务调用**（`mcp_tdoc-doc_doc.<工具名>`），与 `tencent-docs` 主服务平级。Token 共用，但工具调用走独立 endpoint。

> ⚠️ **所有 `doc.*` 工具均支持 `file_id` / `file_url` 二选一**。若用户提供的是文档链接（形如 `https://docs.qq.com/doc/<file_id>`），可直接传 `file_url`，或先从链接末尾解析出 `file_id` 再调用。
>
> 编辑前推荐先调用 `doc.get_outline` 获取文档大纲结构，了解各标题和正文的可操作位置。
>
> 当用户要求「在文档开头插入」时，需向用户确认是在「文档标题之前」（使用 `HEADING_LEVEL_TITLE` 的 `title_start`）还是「正文开头/标题之后」（使用 `HEADING_LEVEL_TITLE` 的 `content_start`）插入，未明确时应主动询问。
> 
> 当用户要求将结果写入 Word 文档时，推荐组合使用：1. 用 `manage.create_file`（`file_type=doc`）创建一个空白 Word 文档 2. 调用 `doc.get_last_operable_pos` 获取可操作位置 3. 调用 `doc.insert_markdown` 将 Markdown 内容写入文档。

---

## 通用说明

### 文档标识

所有 docengine 工具都支持 `file_id` 与 `file_url` 二选一：
- `file_id` (string, 可选): 文档唯一标识符
- `file_url` (string, 可选): 腾讯文档链接（形如 `https://docs.qq.com/doc/<file_id>`）

> 💡 推荐优先沿用用户输入：用户给了链接就传 `file_url`，给了 ID 就传 `file_id`。

### 版本参数

部分 docengine 工具支持可选的 `version_info` 参数，用于指定基于哪个版本进行编辑（不传时默认基于最新版本操作）；**是否支持以各工具参数说明为准**：
- `version_info` (object, 可选):
  - `base_version` (int64, 可选): 基准版本号，通常使用上一步查询类接口（`doc.get_last_operable_pos`、`doc.get_outline`、`doc.resolve_document_structure`、`doc.find` 等）返回的 `version` 值，基于该版本继续编辑，确保编辑操作的连续性。值为 0 表示不指定。
  - `is_latest` (bool, 可选): 是否基于最新版本操作。设为 `true` 时忽略 `base_version`，直接在文档最新版本上编辑。

> 💡 连续多步编辑时，若目标工具支持 `version_info`，建议将上一步查询接口返回的 `version` 传入下一步的 `version_info.base_version`，以避免并发冲突。

### 响应结构

编辑类 API 返回：
- `base_version` (int64): 文档的基准版本号
- `new_version` (int64): 编辑后的文档新版本号
- `err_msg` (string): 错误信息（成功时为空）
- `trace_id` (string): 调用链追踪 ID

查询类 API（如 find）返回：
- `read_result.version` (int64): 文档当前版本号
- `read_result.trace_id` (string): 调用链追踪 ID

---

## 工具列表

| 工具名称 | 功能说明 |
|---------|---------|
| doc.find | 查找文本所在位置，返回匹配位置和上下文 |
| doc.insert_text | 在指定位置插入文本 |
| doc.insert_paragraph | 在指定位置插入段落，支持设置标题级别、编号类别和编号级别 |
| doc.insert_paragraph_with_text | 一步插入带文本的段落（原子操作），避免两步调用的位置漂移 |
| doc.replace_text | 替换指定范围内的文本 |
| doc.find_and_replace | 查找并替换文档中所有匹配的文本 |
| doc.update_text_property | 更新指定范围内文本的属性（加粗、斜体、下划线、删除线、颜色等） |
| doc.modify_paragraph | 修改已有段落的属性（对齐方式、行间距、段前段后间距、段落样式） |
| doc.insert_task | 在指定位置插入一个或多个任务，支持设置任务状态和内容文本 |
| doc.insert_image | 在指定位置插入图片 |
| doc.insert_page_break | 在指定位置插入分页符 |
| doc.insert_table | 在指定位置插入表格 |
| doc.delete_table | 删除整张表格 |
| doc.insert_rows | 在指定表格中批量插入多行 |
| doc.insert_cols | 在指定表格中批量插入多列 |
| doc.insert_comment | 在指定范围插入批注 |
| doc.replace_image | 替换文档中的图片 |
| doc.insert_markdown | 在指定位置插入 Markdown 格式内容，引擎自动转换为富文本 |
| doc.insert_normal_link | 在指定位置插入普通超链接 |
| doc.insert_header | 设置页眉文本内容 |
| doc.insert_footer | 设置页脚文本内容 |
| doc.insert_code_block | 在指定位置插入代码块 |
| doc.insert_footnote | 在指定位置插入脚注或尾注 |
| doc.insert_border | 在指定位置插入分隔符（水平分隔线） |
| doc.insert_numbering | 在指定范围插入项目列表（编号/项目符号） |
| doc.insert_html_content | 在指定位置插入 HTML 内容，引擎自动转换为富文本 |
| doc.insert_attachment | 在指定位置插入附件（需先调用 pre_insert_attachment） |
| doc.pre_insert_attachment | 预插入附件，获取上传链接和 object_key |
| doc.get_images | 获取文档中所有图片的信息 |
| doc.get_last_operable_pos | 获取文档末尾最后一个可操作位置的索引及前面内容 |
| doc.get_outline | 获取文档大纲结构（标题层级树） |
| doc.get_text_property | 读取指定位置处生效的文本属性 |
| doc.get_paragraph_property | 读取指定位置所在段落的属性 |
| doc.get_comments | 获取文档中所有批注 |
| doc.resolve_document_structure | 获取文档完整结构树 |
| doc.get_table_info | 获取指定位置所在表格的整体信息（行高/列宽/合并/各单元格文本+属性），表格写类工具的前置查询 |
| doc.set_table_properties | 修改表格属性（边框、对齐、宽度、单元格内边距、染色） |
| doc.set_page_number | 设置页码（在页脚中插入 PAGE 字段） |
| doc.set_table_layout | 设置表格行高/列宽（支持 auto/manual 两种模式） |
| doc.copy_format | 格式刷：将源范围的格式复制到目标范围 |
| doc.compare_documents | 对比两个文档的内容和格式差异 |
| doc.create_with_markdown | 创建新 Word 文档并写入 Markdown 内容 |
| doc.replace_bookmarks | 替换文档中书签标记范围的内容 |
| doc.accept_all_revisions | 接受文档中的所有修订 |
| doc.revert_revision | 反向指定版本号的 revision 改动（git revert 风格） |
| doc.list_recent_ai_edits | 列出文档最近的 AI MCP 编辑记录，辅助按版本回退 |
| doc.insert_math | 在指定位置插入由 LaTeX 渲染出的数学公式 |

---

## 工具详细说明

## 1. doc.find

### 功能说明
在 Word 文档中查找指定文本，返回所有匹配位置及其上下文。若用于替换文本，推荐优先通过 `doc.resolve_document_structure` 定位并确认目标范围；若根据 `text_preview` 无法可靠定位，再使用 `doc.find` 进行补充查找。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "text": "要查找的文本"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `text` (string, 必填): 要查找的文本内容
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

### 返回值说明
```json
{
  "text_and_locations": [
    {
      "range": { "begin": 10, "end": 15 },
      "related_text": "...上下文文本..."
    }
  ],
  "read_result": {
    "version": 1,
    "trace_id": "trace_1234567890"
  }
}
```
- `text_and_locations` (array): 匹配到的文本位置列表
  - `range.begin` (uint32): 匹配文本的起始位置
  - `range.end` (uint32): 匹配文本的结束位置
  - `related_text` (string): 匹配位置的上下文文本
- `read_result.version` (int64): 当前文档版本号
- `read_result.trace_id` (string): 调用相关的可追踪链路id

### 推荐使用流程
1. 优先调用 `doc.resolve_document_structure` 获取结构节点与 `text_preview`，定位目标文本所在范围
2. 若通过 `text_preview` 无法可靠定位，再调用 `doc.find` 查找目标文本并补充定位
3. 将候选位置展示给用户确认后，调用 `doc.replace_text` 传入对应的 `ranges` 进行精确替换

---

## 2. doc.insert_text

### 功能说明
在 Word 文档的指定位置插入文本。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "text": "要插入的文本内容",
  "idx": 0
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `text` (string, 必填): 要插入的文本内容。注意：如果需要插入换行，应该使用插入段落操作，而不是在文本里插入 '\n' 符号
- `idx` (integer, 必填): 插入位置的索引，从 0 开始，请确认好索引后再操作
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 3. doc.insert_paragraph

### 功能说明
在 Word 文档的指定位置插入段落。支持设置标题级别、编号类别、编号级别和缩进数量，可用于创建标题、有序/无序列表等。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "idx": 0,
  "level": "1",
  "type": "1",
  "numbering_lvl": "1",
  "indent_count": 0
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 插入位置的索引，从 0 开始
- `level` (string, 可选): 标题级别，取值：
  - `"0"`: 未指定（保持原样）
  - `"1"` ~ `"9"`: 一级标题 ~ 九级标题
  - `"10"`: 正文（无标题）
  - `"11"`: 标题
  - `"12"`: 副标题
- `type` (string, 可选): 编号类别，取值：
  - `"0"`: 无编号
  - `"1"`: 圆点列表（无序列表）
  - `"2"`: 数字编号列表（有序列表）
  - 当 `type="1"` 或 `"2"` 时必须同时传 `numbering_lvl`
- `numbering_lvl` (string, 可选): 编号级别，取值 `"1"` ~ `"9"`（当 `type="1"` 或 `"2"` 时必填）
- `indent_count` (integer, 可选): 缩进数量
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 4. doc.replace_text

### 功能说明
替换 Word 文档中指定范围内的文本为新文本。推荐优先使用 `doc.resolve_document_structure` 基于结构和 `text_preview` 定位文本范围；若无法可靠定位，再使用 `doc.find` 补充查找并让用户确认后进行精确替换。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "text": "替换后的文本内容",
  "ranges": [{"begin": 0, "end": 5}]
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `text` (string, 必填): 替换后的文本内容
- `ranges` (array, 必填): 需要替换的文本范围列表，每个范围包含 `begin` 和 `end`
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 5. doc.find_and_replace

### 功能说明
在 Word 文档中查找所有匹配的文本并直接替换为新文本。与 `doc.find` + `doc.replace_text` 的组合不同，此工具会直接替换所有匹配项，用户无法选择性地替换某个特定位置。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "old_text": "要查找的文本",
  "new_text": "替换后的文本"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `old_text` (string, 必填): 要查找的原始文本
- `new_text` (string, 必填): 替换后的新文本
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 6. doc.update_text_property

### 功能说明
更新 Word 文档中指定范围内文本的属性，支持设置加粗、斜体、下划线、删除线、小型大写、字体颜色、背景颜色等。建议先使用 `doc.find` 工具查找文本位置，获取 range 后再调用此工具修改文本属性。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "ranges": [{"begin": 0, "end": 5}],
  "property": {
    "bold": true,
    "color": "FF0000"
  }
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `ranges` (array, 必填): 需要更新属性的文本范围列表，每个范围包含 `begin` 和 `end`
- `property` (object, 必填): 要设置的文本属性，支持以下字段：
  - `bold` (bool, 可选): 是否加粗
  - `italic` (bool, 可选): 是否斜体
  - `underline` (bool, 可选): 是否下划线
  - `strikethrough` (bool, 可选): 是否删除线
  - `small_caps` (bool, 可选): 是否小型大写
  - `color` (string, 可选): 字体颜色，十六进制 RRGGBB 格式，如 "FF0000"
  - `background_color` (string, 可选): 背景颜色，十六进制 RRGGBB 格式，如 "FFFF00"
  - `highlight` (string, 可选): 高亮颜色，如 "yellow"/"cyan"/"none"(清除)
  - `font_size` (number, 可选): 字号，单位磅(pt)，如 12 表示 12pt
  - `font_family` (string, 可选): 字体名，如 'Arial'/'宋体'
  - `vertical_align` (string, 可选): 上下标：baseline/superscript/subscript
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 7. doc.modify_paragraph

### 功能说明
修改 Word 文档中已有段落的属性，支持设置对齐方式、行间距、段前段后间距、段落样式。所有属性均为可选，只传入需要修改的字段即可，未传入的字段保持原样。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "ranges": [{"begin": 0, "end": 15}],
  "jc": "center",
  "line_spacing": 1.5,
  "line_spacing_rule": 1
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `ranges` (array, 必填): 段落范围列表，每项包含 `begin` 和 `end`（字符位置索引）
- `jc` (string, 可选): 段落对齐方式：`left`（左对齐）/ `center`（居中）/ `right`（右对齐）/ `both`（两端对齐）/ `distribute`（分散对齐）
- `spacing_before` (number, 可选): 段前间距，单位磅(pt)
- `spacing_after` (number, 可选): 段后间距，单位磅(pt)
- `line_spacing` (number, 可选): 行间距数值。当 `line_spacing_rule=1`(auto) 时为倍数值（如 1.0=单倍、1.5=1.5倍、2.0=双倍），切勿自行乘以 240；当 `line_spacing_rule=2`(exact) 或 `3`(atLeast) 时为磅值
- `line_spacing_rule` (number, 可选): 行间距规则：`1`=auto（多倍行距）/ `2`=exact（固定值）/ `3`=atLeast（最小值）
- `p_style` (string, 可选): 段落样式名称，如 `Heading1`、`Normal` 等
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 8. doc.insert_task

### 功能说明
在 Word 文档的指定位置插入一个或多个任务（待办事项）。每个任务支持设置任务状态（待办/已完成）和任务内容文本。

### 调用示例

**插入单个任务：**
```json
{
  "file_id": "doc_1234567890",
  "idx": 0,
  "tasks": [
    {
      "state": 1,
      "content": "完成需求文档编写"
    }
  ]
}
```

**插入多个任务：**
```json
{
  "file_id": "doc_1234567890",
  "idx": 5,
  "tasks": [
    {
      "state": 1,
      "content": "完成需求文档编写"
    },
    {
      "state": 2,
      "content": "完成接口设计"
    },
    {
      "state": 1,
      "content": "编写单元测试"
    }
  ]
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 插入位置的索引，从 0 开始
- `tasks` (array, 必填): 任务列表，支持一次插入多个任务，每个任务包含：
  - `state` (integer, 必填): 任务状态枚举值，不允许传递 0 值，取值：
    - `1`: 待办（未完成）
    - `2`: 已完成
  - `content` (string, 必填): 任务内容文本
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》
### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

### doc.insert_image

#### 功能说明
在 Word 文档的指定位置插入图片。使用前请先调用 `doc.resolve_document_structure` 获取文档结构，确定正确的插入位置索引。

#### 图片来源选择优先级（按推荐顺序）

**方式一（⭐首选）：经 MCP 工具通道传 `content`（图片 base64 字符串，不含 data URI 前缀）** —— 本地图片先用 shell 的 `base64` 命令读取编码，再作为 `content` 参数直接调用 `mcp_tdoc-doc_doc.insert_image`。仅适合图片体积较小的场景；若 base64 超出 MCP 单次传输限制会失败，此时请改用方式二。

**方式二（备选）：传 `image_id`** —— 先通过 `upload_image` MCP 工具或腾讯文档开放平台 OpenAPI 把图片上传后获取 `image_id`，再传入本工具。

#### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "content": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
  "idx": 0,
  "width": 400,
  "height": 300
}
```

#### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `content` (string, 可选): 图片的 base64 内容（方式一，本地图片先用 shell `base64` 命令编码，仅适合小图），与 `image_id` 二选一。**大图请改用方式二先上传得到 `image_id` 后再传入**
- `image_id` (string, 可选): 图片的 `image_id`（方式二备选），与 `content` 二选一。获取方式有两种：
  1. 通过 `upload_image` MCP 接口上传图片后获取；
  2. 通过[腾讯文档开放平台 OpenAPI](https://docs.qq.com/open/developers/?nlc=1#/login) 图片上传接口获取（适合图片体积较大的场景；需先完成 OAuth 授权流程获取 `Access-Token`、`Client-Id`、`Open-Id`），示例命令：
  ```bash
  curl --location --request POST 'https://docs.qq.com/openapi/resources/v2/images' \
    --header 'Access-Token: ACCESS_TOKEN' \
    --header 'Client-Id: CLIENT_ID' \
    --header 'Open-Id: OPEN_ID' \
    --form 'image=@"/path/to/your/image.png"'
  ```
  上传成功后，取返回结果中的 `imageID` 字段值传入此参数
- `idx` (integer, 必填): 插入位置的索引，从 0 开始
- `width` (integer, 可选): 图片宽度，单位为像素（px），例如 400 表示 400px；不传时使用图床上传返回的宽度
- `height` (integer, 可选): 图片高度，单位为像素（px），例如 300 表示 300px；不传时使用图床上传返回的高度
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

#### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "",
  "err_msg": ""
}
```

---

## 9. doc.insert_page_break

### 功能说明
在 Word 文档的指定位置插入分页符。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "idx": 10
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 插入位置的索引，从 0 开始
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 10. doc.insert_table

### 功能说明
在 Word 文档的指定位置插入表格。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "idx": 0,
  "rows": 3,
  "cols": 4
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 插入位置的索引，从 0 开始
- `rows` (integer, 必填): 表格行数
- `cols` (integer, 必填): 表格列数
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 11. doc.insert_comment

### 功能说明
在 Word 文档的指定范围内插入批注（评论）。注意：插入批注后文本长度会发生变化，如果需要继续操作应该重新获取位置。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "text": "这里需要修改措辞",
  "range": {"begin": 5, "end": 15}
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `text` (string, 必填): 批注内容
- `range` (object, 必填): 批注关联的文本范围，格式为 `{"begin": 68, "end": 75}`。允许 `begin == end`（点锚批注）
- `ref_id` (string, 可选): 评论ID，用于回复已有批注
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 12. doc.get_images

#### 功能说明
获取 Word 文档中所有图片的信息，包括每张图片的位置索引（`pos`）、来源类型（URL 图片或附件图片）以及对应的 URL 或附件 ID。通常在调用 `doc.replace_image` 前先调用此接口，获取目标图片的 `pos`（即 `idx`）和 `image_url`/`attachment_id`（即 `old_image_url`/`old_attachment_id`）。

### 调用示例
```json
{
  "file_id": "doc_1234567890"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

### 返回值说明
```json
{
  "images": [
    {
      "source": 1,
      "pos": 42,
      "image_url": "https://docimg8.docs.qq.com/image/AgAABsUhABzwC7ScF1dHP4mZWR9jTQ5i.jpeg"
    },
    {
      "source": 2,
      "pos": 88,
      "attachment_id": "AgAABsUhABzwC7ScF1dHP4mZWR9jTQ5i"
    }
  ],
  "version": 1024
}
```
- `images` (array): 文档中所有图片列表，按位置（`pos`）升序排列
  - `source` (int): 图片来源类型，`1` = URL 图片（`FromLink`），`2` = 附件图片（`FromAttachment`）
  - `pos` (int64): 图片在文档中的位置索引，即 `doc.replace_image` 接口的 `idx` 参数
  - `image_url` (string): 当 `source=1` 时有值，图片的内嵌 URL，即 `doc.replace_image` 接口的 `old_image_url` 参数
  - `attachment_id` (string): 当 `source=2` 时有值，附件图片的 object_key，即 `doc.replace_image` 接口的 `old_attachment_id` 参数
- `version` (int64): 当前文档版本号

### 推荐使用流程
1. 调用 `doc.get_images` 获取文档中所有图片信息
2. 根据返回的 `pos`（作为 `idx`）和 `image_url`/`attachment_id`（作为 `old_image_url`/`old_attachment_id`）定位目标图片
3. 调用 `doc.replace_image` 传入对应参数完成图片替换

---

## 12. doc.replace_image

### 功能说明
替换 Word 文档中的图片。**必须同时提供三组参数**：
1. `idx`（图片位置）
2. `old_image_url` 或 `old_attachment_id`（定位旧图片）
3. 新图片来源（按下方优先级三选一）

缺少任何一组都会导致替换失败。建议先调用 `doc.get_images` 获取图片信息，再用返回的 `pos` 和 `image_url`/`attachment_id` 填入对应参数。

> ⚠️ **重要提示**：
> - `old_image_url` 中**不要带查询参数**（如 `?w=300&h=281`），需去掉问号及之后的部分，否则 C++ 层做精确字符串匹配时会匹配失败
> - `get_images` 返回的 `pos` 是 `int64` 类型，经 protobuf JSON 序列化后为字符串（如 `"12"`），传入 `idx` 时请转为整数

#### 新图片来源选择优先级（按推荐顺序）

**方式一（⭐首选）：经 MCP 工具通道传 `content`（图片 base64 字符串，不含 data URI 前缀）** —— 本地图片先用 shell 的 `base64` 命令读取编码，再连同 `idx` / `old_image_url` / `old_attachment_id` / `file_id` / `file_url` 等参数直接调用 `mcp_tdoc-doc_doc.replace_image`。仅适合图片体积较小的场景；若 base64 超出 MCP 单次传输限制会失败，此时请改用方式二。

**方式二（备选）：传 `image_id`** —— 先通过 `upload_image` MCP 工具或腾讯文档开放平台 OpenAPI 把图片上传后获取 `image_id`，再传入本工具。

### 调用示例
```json
{
  "file_url": "https://docs.qq.com/doc/xxxxxxxx",
  "idx": 12,
  "old_image_url": "https://docimg3.docs.qq.com/image/AgAABsUhABzuGm3nPThHvJMLVLu3pZUz.png",
  "image_id": "KlCYcLj1CTUoMfAR9bleB+G+..."
}
```

#### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, **必填**): 图片在文档中的位置索引，对应 `get_images` 返回的 `pos` 字段
- `old_image_url` (string, 条件必填): 旧图片的 URL，与 `old_attachment_id` 二选一（**必须提供其一**），对应 `get_images` 返回的 `image_url` 字段。**注意：URL 中不要带查询参数（如 `?w=300&h=281`），需去掉问号及之后的部分**
- `old_attachment_id` (string, 条件必填): 旧图片的附件 ID，与 `old_image_url` 二选一（**必须提供其一**），对应 `get_images` 返回的 `attachment_id` 字段
- `image_id` (string, 条件必填): 新图片的 `image_id`（方式二备选），与 `content` 二选一（**必须提供其一**）。获取方式有两种：
  1. 通过 `upload_image` MCP 接口上传图片后获取；
  2. 通过[腾讯文档开放平台 OpenAPI](https://docs.qq.com/open/developers/?nlc=1#/login) 图片上传接口获取（适合图片体积较大、base64 内容超出传输限制的场景；需先完成 OAuth 授权流程获取 `Access-Token`、`Client-Id`、`Open-Id`），示例命令：
  ```bash
  curl --location --request POST 'https://docs.qq.com/openapi/resources/v2/images' \
    --header 'Access-Token: ACCESS_TOKEN' \
    --header 'Client-Id: CLIENT_ID' \
    --header 'Open-Id: OPEN_ID' \
    --form 'image=@"/path/to/your/image.png"'
  ```
  上传成功后，取返回结果中的 `imageID` 字段值传入此参数
- `content` (string, 可选): 新图片的 base64 内容（方式一，本地图片先用 shell `base64` 命令编码），与 `image_id` 二选一。**仅适合图片体积较小的场景；若图片过大导致 base64 超出 MCP 单次传输限制，请改用方式二先上传得到 `image_id` 后再传入**
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 13. doc.insert_markdown

### 功能说明
在 Word 文档的指定位置插入 Markdown 格式内容。引擎会自动将 Markdown 转换为文档富文本格式，支持标题、列表、表格、链接、加粗/斜体等常见 Markdown 语法。适合需要批量插入富文本内容的场景，比直接调用多个 `insert_text`/`insert_paragraph` 更高效。

> ⚠️ **推荐使用 `base64_markdown` 参数**：由于 Markdown 内容中可能包含特殊字符（如换行符、引号等），直接传递 `markdown` 参数容易导致 JSON 解析问题。**建议 agent 先将 Markdown 内容进行 base64 编码后，通过 `base64_markdown` 参数传递**。如果填写了 `base64_markdown`，则无需再填写 `markdown`。

### 调用示例

**使用 base64_markdown（推荐）：**
```json
{
  "file_id": "doc_1234567890",
  "idx": 0,
  "base64_markdown": "IyDmoIfpopgKCui/meaYr+S4gOautSoq5Yqg57KXKirmlofmnKzjgIIKCi0g5YiX6KGo6aG5MQotIOWIl+ihqOmhuTIKCnwg5aeT5ZCNIHwg5bm06b6EIHwKfC0tLS0tLXwtLS0tLS18Cnwg5byg5LiJIHwgMjUgfA==",
  "version_info": {
    "base_version": 5,
    "is_latest": false
  }
}
```

**使用 markdown（备选）：**
```json
{
  "file_id": "doc_1234567890",
  "idx": 0,
  "markdown": "# 标题\n\n这是一段**加粗**文本。\n\n- 列表项1\n- 列表项2\n\n| 姓名 | 年龄 |\n|------|------|\n| 张三 | 25 |"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 插入位置的索引，从 0 开始
- `base64_markdown` (string, ⭐ 首选): Markdown 内容的 base64 编码字符串。**推荐优先使用此参数**，agent 需要先将 Markdown 文本进行标准 base64 编码后传入。与 `markdown` 二选一，如果填写了 `base64_markdown` 则无需再填写 `markdown`
- `markdown` (string, 备选): Markdown 格式的原始文本内容，与 `base64_markdown` 二选一。当未提供 `base64_markdown` 时使用此参数。支持以下语法：
  - 标题：`# H1`、`## H2`、`### H3` 等
  - 加粗/斜体：`**加粗**`、`*斜体*`
  - 链接：`[文本](URL)`
  - 无序列表：`- 列表项`
  - 有序列表：`1. 列表项`
  - 表格：使用 `|` 和 `---` 语法
  - 代码块：使用反引号包裹
- `version_info` (object, 可选): 版本控制参数，用于指定基于哪个版本进行编辑。不传时默认基于最新版本操作。包含以下字段：
  - `base_version` (int64, 可选): 基准版本号，通常使用 `doc.get_last_operable_pos`、`doc.get_outline` 或 `doc.resolve_document_structure` 返回的 `version` 值，基于该版本继续编辑，确保编辑操作的连续性。值为 0 表示不指定
  - `is_latest` (bool, 可选): 是否基于最新版本操作。设为 `true` 时忽略 `base_version`，直接在文档最新版本上编辑

> 💡 **version_info 使用场景**：当需要连续执行多步编辑操作时（如先 `doc.get_outline` 获取大纲，再 `doc.insert_markdown` 插入内容），建议将前一步返回的 `version` 传入 `version_info.base_version`，以确保编辑基于同一版本，避免并发冲突。

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```
- `base_version` (int64): 文档的基准版本号
- `new_version` (int64): 命令执行之后的文档版本
- `trace_id` (string): 本次调用的链路追踪 ID
- `err_msg` (string): 失败信息

---

## 14. doc.get_last_operable_pos

### 功能说明
获取 Word 文档正文（main story）最后一个可操作位置的索引，以及该位置前面最多 10 个字符的内容。在需要向文档末尾追加内容时，可先调用此接口获取末尾可操作位置，再使用 `doc.insert_text`/`doc.insert_image` 等接口在该位置插入内容。

### 调用示例
```json
{
  "file_id": "doc_1234567890"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

### 返回值说明
```json
{
  "position": 100,
  "preceding_text": "...前面内容...",
  "version": 1
}
```
- `position` (int64): 最后一个可操作位置的索引
- `preceding_text` (string): 该位置前面最多 10 个字符的内容
- `version` (int64): 当前文档版本号

---

## 15. doc.get_outline

### 功能说明
获取 Word 文档的完整大纲结构（树形），返回文档标题、各级标题及其下正文的可操作位置范围。可用于：
- 了解文档整体结构和层级关系
- 获取指定标题或正文区域的精确位置（`title_start`/`title_end`、`content_start`/`content_end`），以便在对应位置插入或替换内容
- 在操作前先掌握文档大纲，避免盲目使用 `find` 查找

> ⚠️ **关于「在文档开头插入」的位置说明**：文档大纲的根节点通常是 `HEADING_LEVEL_TITLE`（文档标题），其 `title_start` 表示文档标题之前的位置，`content_start` 表示标题之后、正文开头的位置。当用户要求"在文档开头插入内容"时，需要向用户确认具体含义：
> - **在文档标题之前插入**：使用 `HEADING_LEVEL_TITLE` 节点的 `title_start`
> - **在正文开头插入（标题之后）**：使用 `HEADING_LEVEL_TITLE` 节点的 `content_start`
> 
> 如果用户未明确说明，应主动询问用户确认具体插入位置。

### 调用示例
```json
{
  "file_id": "doc_1234567890"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

### 返回值说明
```json
{
  "outlines": [
    {
      "title": "文档标题",
      "level": "HEADING_LEVEL_TITLE",
      "title_start": 0,
      "title_end": 5,
      "content_start": 6,
      "content_end": 100,
      "children": [
        {
          "title": "第一章 概述",
          "level": "HEADING_LEVEL_1",
          "title_start": 6,
          "title_end": 12,
          "content_start": 13,
          "content_end": 50,
          "children": [
            {
              "title": "1.1 背景",
              "level": "HEADING_LEVEL_2",
              "title_start": 13,
              "title_end": 18,
              "content_start": 19,
              "content_end": 50,
              "children": []
            }
          ]
        }
      ]
    }
  ],
  "version": 1
}
```

- `outlines` (array): 大纲根节点列表（树形结构），每个节点包含：
  - `title` (string): 标题文本内容
  - `level` (string): 标题级别，取值说明：
    - `HEADING_LEVEL_TITLE` (11): 文档标题
    - `HEADING_LEVEL_1` ~ `HEADING_LEVEL_9` (1~9): 一级标题 ~ 九级标题
    - `HEADING_LEVEL_BODY` (10): 正文（无标题）
  - `title_start` (int64): 标题可操作的起始位置（可在此位置前插入内容）
  - `title_end` (int64): 标题可操作的结束位置
  - `content_start` (int64): 该标题下正文可操作的起始位置（在标题下方插入内容时使用）
  - `content_end` (int64): 该标题下正文可操作的结束位置（在正文末尾追加内容时使用）
  - `children` (array): 子目录项列表（递归结构，构成树形大纲）
- `version` (int64): 当前文档版本号

---

## 16. doc.resolve_document_structure

### 功能说明
获取 Word 文档的完整结构树（DOC），返回 main story 下所有块级元素的层级结构和位置信息。与 `doc.get_outline` 只返回标题层级不同，此接口返回**所有**块级元素，包括：
- **Paragraph**：普通文本段落
- **Heading**：标题段落（含级别）
- **Table**：表格（含每行每列的起止位置）
- **TextBox**：文本框（含内部段落的起止位置）
- **CodeBlock**：代码块（含内部段落的起止位置）

适用场景：
- 需要在**表格指定行列**插入或修改文本（通过 `table_rows[row].cells[col].end_index` 定位单元格末尾）
- 需要在**文本框内部**插入内容（通过 `children` 中的段落位置定位）
- 需要了解文档完整布局后再决定操作位置
- 需要精确获取某个段落、代码块的起止范围

### 调用示例
```json
{
  "file_id": "doc_1234567890"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `mode` (string, 可选): 信息密度档位，默认 `compact`。取值：`outline`（骨架级）/ `compact`（轻量信号位）/ `full`（详情全回填）
- `text_preview_length` (number, 可选): text_preview 字符上限。不传走 mode 默认值；0=关闭；上限 200
- `include_highlights` (bool, 可选): compact/outline 下回填 highlight_spans 详情
- `include_table_cells` (bool, 可选): compact/outline 下回填 Table.table_rows[].cells[]
- `include_textbox_children` (bool, 可选): compact/outline 下回填 TextBox/CodeBlock/HighlightBlock.children[]
- `idx` (integer, 可选): 字符位置索引（GCP 坐标）。传入则只返回包含该位置的单块，自动按 full 回填
- `offset` (number, 可选): 顶层 nodes 起始下标，默认 0。idx 模式忽略
- `limit` (number, 可选): 顶层 nodes 最大数量，默认 150；0=不分页。idx 模式忽略
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

### 返回值说明
```json
{
  "nodes": [
    {
      "type": "Heading",
      "start_index": 0,
      "end_index": 6,
      "text_preview": "文档标题",
      "heading_level": 1,
      "logical_index": 1,
      "table_rows": [],
      "children": []
    },
    {
      "type": "Paragraph",
      "start_index": 7,
      "end_index": 20,
      "text_preview": "这是第一段正文内容",
      "heading_level": 0,
      "logical_index": 2,
      "table_rows": [],
      "children": []
    },
    {
      "type": "Table",
      "start_index": 21,
      "end_index": 60,
      "text_preview": "",
      "heading_level": 0,
      "logical_index": 3,
      "table_rows": [
        {
          "row": 1,
          "cells": [
            { "row": 1, "col": 1, "start_index": 22, "end_index": 30, "text_preview": "单元格内容" },
            { "row": 1, "col": 2, "start_index": 31, "end_index": 38, "text_preview": "" }
          ]
        },
        {
          "row": 2,
          "cells": [
            { "row": 2, "col": 1, "start_index": 40, "end_index": 48, "text_preview": "" },
            { "row": 2, "col": 2, "start_index": 49, "end_index": 57, "text_preview": "" }
          ]
        }
      ],
      "children": []
    },
    {
      "type": "TextBox",
      "start_index": 61,
      "end_index": 80,
      "text_preview": "文本框内容",
      "heading_level": 0,
      "logical_index": 4,
      "table_rows": [],
      "children": [
        {
          "type": "Paragraph",
          "start_index": 62,
          "end_index": 79,
          "text_preview": "文本框内容",
          "heading_level": 0,
          "logical_index": 1,
          "table_rows": [],
          "children": []
        }
      ]
    },
    {
      "type": "CodeBlock",
      "start_index": 81,
      "end_index": 110,
      "text_preview": "console.log('hello')",
      "heading_level": 0,
      "logical_index": 5,
      "table_rows": [],
      "children": [
        {
          "type": "Paragraph",
          "start_index": 82,
          "end_index": 109,
          "text_preview": "console.log('hello')",
          "heading_level": 0,
          "logical_index": 1,
          "table_rows": [],
          "children": []
        }
      ]
    }
  ],
  "version": 5,
  "total_paragraphs": 3,
  "total_headings": 1,
  "total_tables": 1
}
```

- `nodes` (array): 顶层块级节点列表（main story 直接子节点），按文档顺序排列，每个节点包含：
  - `type` (string): 节点类型，取值：`Paragraph`、`Heading`、`Table`、`TextBox`、`CodeBlock`、`HighlightBlock`
  - `start_index` (uint32): 节点起始位置（inclusive）
  - `end_index` (uint32): 节点结束位置（在此处插入可追加到节点末尾）
  - `text_preview` (string): 文本预览，最多 50 字符，仅 Paragraph/Heading 有值。文本中可能包含以下占位符标记，表示段落内嵌入的非文字元素：
    - `[Image]`：嵌入的图片
    - `[Math]`：数学公式
    - `[TextBox]`：嵌入的文本框/代码块/高亮块锚点（对应的 TextBox/CodeBlock/HighlightBlock 节点会作为独立的顶层节点出现在 `nodes` 中）
    - `[Drawing]`：其他嵌入的图形/形状对象
    - `[Hyperlink]`：超链接（普通链接、文档链接、附件链接等）
    - `[addonHina]`：内嵌插件（流程图、思维导图、白板、内嵌表格等腾讯文档内嵌的第三方插件内容）
  - `heading_level` (int32): 标题级别 1-9，仅 Heading 类型有值，其余为 0
  - `logical_index` (int32): 在同级中的逻辑序号（从 1 开始）
  - `table_rows` (array): 仅 Table 类型有值，包含行列结构：
    - `row` (int32): 行号（从 1 开始）
    - `cells` (array): 该行所有单元格：
      - `row` (int32): 行号（从 1 开始）
      - `col` (int32): 列号（从 1 开始）
      - `start_index` (uint32): 单元格起始位置
      - `end_index` (uint32): 单元格结束位置（在此处插入可追加到单元格末尾）
      - `text_preview` (string): 单元格文本预览，最多 30 字符，可能包含 `[Image]`/`[TextBox]`/`[Drawing]`/`[Hyperlink]`/`[addonHina]` 等占位符标记（含义同上）
  - `children` (array): 子节点列表，TextBox/CodeBlock 内部的段落等
- `version` (int64): 当前文档版本号
- `total_paragraphs` (int32): 正文段落总数（不含标题）
- `total_headings` (int32): 标题总数
- `total_tables` (int32): 表格总数

---

## 17. doc.modify_paragraph

### 功能说明
修改 Word 文档中已有段落的属性，支持设置对齐方式、行间距、段前段后间距、段落样式。所有属性均为可选，只传入需要修改的字段即可，未传入的字段保持原样。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "ranges": [{"begin": 0, "end": 15}],
  "jc": "center",
  "line_spacing": 1.5,
  "line_spacing_rule": 1
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `ranges` (array, 必填): 段落范围列表，每项包含 `begin` 和 `end`（字符位置索引）
- `jc` (string, 可选): 段落对齐方式：`left`（左对齐）/ `center`（居中）/ `right`（右对齐）/ `both`（两端对齐）/ `distribute`（分散对齐）
- `spacing_before` (number, 可选): 段前间距，单位磅(pt)
- `spacing_after` (number, 可选): 段后间距，单位磅(pt)
- `line_spacing` (number, 可选): 行间距数值。当 `line_spacing_rule=1`(auto) 时为倍数值（如 1.0=单倍、1.5=1.5倍、2.0=双倍），切勿自行乘以 240；当 `line_spacing_rule=2`(exact) 或 `3`(atLeast) 时为磅值
- `line_spacing_rule` (number, 可选): 行间距规则：`1`=auto（多倍行距）/ `2`=exact（固定值）/ `3`=atLeast（最小值）
- `p_style` (string, 可选): 段落样式名称，如 `Heading1`、`Normal` 等
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 18. doc.get_text_property

### 功能说明
读取 Word 文档指定位置处生效的文本属性（与 `update_text_property` 字段对称）：加粗/斜体/下划线/删除线/小型大写/颜色/背景色/字号(pt)/字体/上下标等。响应中「字段不存在」表示该属性未在 run 上显式设置，应认为「继承自段落/样式默认值」。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "idx": 10
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 要查询的字符位置（0-based），建议通过 `resolve_document_structure` 获取段落 `start_index`
- `version_info` (object, 可选): 版本参数

### 返回值说明
返回指定位置处生效的文本属性对象，包含 bold、italic、underline、strikethrough、small_caps、color、background_color、font_size、font_name、vertical_align 等字段。

---

## 19. doc.get_paragraph_property

### 功能说明
读取 Word 文档指定位置所在段落的属性（与 `insert_paragraph` 字段对称）：段落样式名、大纲级别、heading_level、是否带编号及编号级别、对齐方式、缩进、段前/后间距与行距。字段不存在表示未显式设置（继承默认值）。主要用于在修改段落属性前先了解当前状态。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "idx": 10
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 要查询的字符位置（0-based），会返回该位置所在段落的属性
- `version_info` (object, 可选): 版本参数

### 返回值说明
返回段落属性对象，包含 p_style、outline_level、heading_level、numbering_type、numbering_lvl、jc、indent、spacing_before、spacing_after、line_spacing、line_spacing_rule 等字段。

---

## 20. doc.get_comments

### 功能说明
获取 Word 文档中所有批注。返回按 `range_begin` 升序排列的批注数组。主要用于对 `insert_comment` 的结果做验证、以及在执行批注相关编辑前枚举现有批注。

### 调用示例
```json
{
  "file_id": "doc_1234567890"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "comments": [
    {
      "id": "comment_001",
      "range_begin": 5,
      "range_end": 15,
      "text": "批注内容",
      "ref_id": "",
      "author": "张三",
      "date": "2024-01-01T12:00:00Z",
      "is_point_anchor": false
    }
  ],
  "read_result": {
    "version": 1,
    "trace_id": "trace_1234567890"
  }
}
```
- `comments` (array): 批注列表
  - `id` (string): 批注 ID
  - `range_begin` / `range_end` (uint32): 批注锚定范围的字符索引
  - `text` (string): 批注正文文本
  - `ref_id` (string): 回复链父批注 ID（仅回复批注才有）
  - `author` (string): 作者展示名
  - `date` (string): ISO 时间字符串
  - `is_point_anchor` (bool): begin == end 时为 true，表示零长度的「点锚」批注

---

## 21. doc.insert_normal_link

### 功能说明
在 Word 文档指定位置插入普通超链接，可指定链接 URL 和显示文本。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "idx": 10,
  "link": "https://docs.qq.com",
  "text": "腾讯文档"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 插入位置索引
- `link` (string, 必填): 链接地址
- `text` (string, 可选): 显示文本
- `file_name` (string, 可选): 文件名
- `layout_type` (string, 可选): 布局类型
- `independent_render` (bool, 可选): 是否独立渲染
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 22. doc.insert_header

### 功能说明
设置页眉文本内容。覆盖式写入：先清空已有页眉内容，再写入新文本。本工具一次只处理一个 section（由 `section_index` 指定）；如需对多个 section 都写入页眉，请按需多次调用。首次插入时会为目标 section 独立创建页眉页脚 substory，打破该 section 与前一节"链接到上一节"的关系。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "text": "公司机密文档",
  "section_index": 0,
  "header_type": "default"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `text` (string, 必填): 要写入的页眉文本内容
- `section_index` (integer, 可选): 节索引（0-based），默认为 0
- `header_type` (string, 可选): 页眉类型：`default`（默认页眉，应用于所有页）、`first`（首页页眉）、`even`（偶数页页眉）
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 23. doc.insert_footer

### 功能说明
设置页脚文本内容。覆盖式写入：先清空已有页脚内容，再写入新文本。本工具一次只处理一个 section（由 `section_index` 指定）。

> ⚠️ 本工具与 `doc.set_page_number` 互斥。调用本工具会清除已有的页码（PAGE 字段）。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "text": "第 X 页",
  "section_index": 0,
  "footer_type": "default"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `text` (string, 必填): 要写入的页脚文本内容
- `section_index` (integer, 可选): 节索引（0-based），默认为 0
- `footer_type` (string, 可选): 页脚类型：`default`（默认页脚，应用于所有页）、`first`（首页页脚）、`even`（偶数页页脚）
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 24. doc.insert_code_block

### 功能说明
在指定位置直接插入一个代码块。代码块会以原生 textbox 形式呈现，支持语法高亮语言标签。自动格式化：会自动去除所有行的公共前导缩进（dedent），无需手动处理缩进对齐。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "idx": 10,
  "code": "def hello():\n    print('Hello, World!')",
  "language": "python"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 插入位置的字符索引
- `code` (string, 必填): 代码内容（纯文本，支持多行）
- `language` (string, 可选): 代码语言标签（如 `python` / `javascript` / `cpp` / `java` / `go` / `mermaid` 等），用于语法高亮
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 25. doc.insert_footnote

### 功能说明
在指定位置插入脚注或尾注。脚注显示在页面底部，尾注显示在文档末尾。

> ⚠️ `idx` 应该使用目标文本的最后一个字符位置（即段落的 `end_index - 1`，段落分隔符 ¶ 之前），这样脚注/尾注引用标记会出现在文本末尾而非中间。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "idx": 7,
  "type": 0,
  "content": "这是一条脚注说明"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 插入脚注/尾注引用标记的位置索引。应使用目标文本最后一个字符的位置
- `type` (integer, 可选): 类型：`0`=脚注（默认），`1`=尾注
- `content` (string, 可选): 脚注/尾注的文本内容（不传则创建空标记）
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 26. doc.insert_paragraph_with_text

### 功能说明
一步插入「带文本的段落」。原子完成「在 idx 处插入 text → 在 idx+len(text) 处插入段落分隔符 → 应用 level/type/numbering_lvl/indent_count」。推荐首选此接口而非「insert_text + insert_paragraph」两步调用，避免两步之间的位置漂移。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "idx": 50,
  "text": "这是新段落的内容",
  "level": "1",
  "type": "0"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 目标段落的 `end_index`。新段落将插入到该段落之后
- `text` (string, 必填): 要写入的段落文本（不包含换行）。允许为空字符串
- `level` (string, 可选): 标题级别：`0`=普通段落（默认），`1`~`9`=Heading1~Heading9
- `type` (string, 可选): 编号类别：`0`=无编号（默认），`1`=圆点项目符号，`2`=数字编号(1、)，`3`=待办事项，`4`=数字编号(1.)
- `numbering_lvl` (string, 可选): 编号级别（1~9），默认为 1。仅在 type=1 或 2 时生效
- `indent_count` (integer, 可选): 编号前缩进空格数，仅在 type=1/2 时生效
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 27. doc.insert_border

### 功能说明
在指定位置插入分隔符（水平分隔线）。通过设置段落底部边框来实现。

> ⚠️ `idx` 必须是「分隔线上方那个段落」的 `end_index`（即该段落的段落分隔符 ¶ 的位置）。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "idx": 20,
  "border_type": "single",
  "color": "000000",
  "sz": 8
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 分隔线上方段落的 `end_index`
- `border_type` (string, 可选): 边框类型：`single`（单线，默认）、`thick`（粗线）、`dotted`（点线）、`dashed`（虚线）、`double`（双线）、`dotDash`（点划线）、`wave`（波浪线）等
- `color` (string, 可选): 边框颜色（十六进制），如 `"000000"` 表示黑色。默认 auto
- `sz` (integer, 可选): 边框粗细（1/8磅为单位），如 8 表示 1 磅。默认 8
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 28. doc.set_table_properties

### 功能说明
修改表格属性。通过表格内任意 cell 的 GCP 位置 `idx` 定位表格，可同时设置边框、对齐、宽度、单元格内边距，以及按 9 种 condition 染色单元格。全部字段可选，未提供的属性沿用原表格设置。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "idx": 25,
  "alignment": "center",
  "borders": {
    "top": {"color": "000000", "sz": 8},
    "bottom": {"color": "000000", "sz": 8}
  }
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 表格内任意 cell 的 GCP 位置（字符索引）
- `alignment` (string, 可选): 表格对齐方式
- `borders` (object, 可选): 表格 6 个方向边框。key 为 `top`/`left`/`bottom`/`right`/`inside_h`/`inside_v`，每项包含 `color`(string: 6位十六进制无#)、`sz`(integer: 1/8磅)、`space`(integer)
- `cell_margin` (object, 可选): 默认单元格内边距（dxa 单位）。含 `top`/`left`/`bottom`/`right`(integer)
- `width` (object, 可选): 表格宽度。含 `type`(string) 和 `value`(integer)
- `cell_fills` (array, 可选): 单元格填充列表。每项含 `condition`(string) 和 `color`(string)。condition 取值：`whole_table` / `first_row` / `last_row` / `first_col` / `last_col` / `band_row_odd` / `band_row_even` / `band_col_odd` / `band_col_even`
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 29. doc.insert_rows

### 功能说明
在指定表格中批量插入多行。通过 `idx` 定位目标表格（表格内任意单元格的 GCP 位置），`rows` 为要插入的多行描述列表，支持一次插入多行，所有插入相对于原表格行号生效（引擎会自动按降序依次应用，避免索引偏移）。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "idx": 25,
  "rows": [
    {"row": 2, "insert_below": true}
  ]
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 表格内任意单元格的 GCP 位置
- `rows` (array, 必填): 要插入的行列表，至少 1 项。每项包含：
  - `row` (integer): 参考行号（0-based，相对于表格内部）
  - `insert_below` (boolean): `true` 表示在参考行下方插入，`false` 表示在参考行上方插入
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 30. doc.insert_cols

### 功能说明
在指定表格中批量插入多列。通过 `idx` 定位目标表格（表格内任意单元格的 GCP 位置），`cols` 为要插入的多列描述列表，支持一次插入多列，所有插入相对于原表格列号生效（引擎会自动按降序依次应用，避免索引偏移）。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "idx": 25,
  "cols": [
    {"col": 1, "insert_right": true}
  ]
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 表格内任意单元格的 GCP 位置
- `cols` (array, 必填): 要插入的列列表，至少 1 项。每项包含：
  - `col` (integer): 参考列号（0-based，相对于表格内部）
  - `insert_right` (boolean): `true` 表示在参考列右侧插入，`false` 表示在参考列左侧插入
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 31. doc.copy_format

### 功能说明
格式刷：将源范围的段落属性和文本属性复制到目标范围。读取 `source_range` 起始位置处生效的段落格式和文本格式，然后应用到 `target_ranges` 指定的所有范围上。适用于快速统一多个段落/文本的格式。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "source_range": {"begin": 0, "end": 10},
  "target_ranges": [{"begin": 20, "end": 30}, {"begin": 40, "end": 50}],
  "copy_paragraph_format": true,
  "copy_text_format": true
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `source_range` (object, 必填): 格式来源范围，包含 `begin` 和 `end`
- `target_ranges` (array, 必填): 目标范围列表，每项包含 `begin` 和 `end`
- `copy_paragraph_format` (bool, 可选): 是否复制段落格式（对齐、缩进、间距、样式等）。默认 true
- `copy_text_format` (bool, 可选): 是否复制文本格式（加粗、斜体、字号、字体、颜色等）。默认 true
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 32. doc.compare_documents

### 功能说明
对比两个 DOC 文档的内容和格式差异。返回段落级别的差异列表，包括新增/删除/内容修改/格式修改。还支持对比页眉页脚、文本框、表格和批注的差异。

### 调用示例
```json
{
  "file_id": "doc_source_123",
  "other_file_id": "doc_target_456",
  "compare_mode": "all"
}
```

### 参数说明
- `file_id` (string, 可选): 基准文档的 file_id，与 `file_url` 二选一
- `file_url` (string, 可选): 基准文档的文档链接，与 `file_id` 二选一
- `other_file_id` (string, 可选): 要对比的另一个文档的 file_id，与 `other_file_url` 二选一
- `other_file_url` (string, 可选): 要对比的另一个文档的文档链接，与 `other_file_id` 二选一
- `compare_mode` (string, 可选): 对比模式：`content`（仅对比文本内容）、`format`（仅对比格式）、`all`（全部对比，默认值）
- `version_info` (object, 可选): 版本参数

### 返回值说明
返回差异列表，包含：
- `diffs` (array): 段落级别差异列表，每项包含 `diff_type`（added/deleted/modified_content/modified_format）、`source_paragraph`、`target_paragraph` 等
- `header_footer_diffs` (array): 页眉页脚差异
- `textbox_diffs` (array): 文本框差异
- `table_diffs` (array): 表格差异
- `comment_diffs` (array): 批注差异

---

## 33. doc.create_with_markdown

### 功能说明
用 Markdown 创建文档。用 Markdown 内容直接创建一篇新的 Word 文档，无需先调用 `manage.create_file` 再 `insert_markdown`，一步完成文档创建和内容写入。

### Markdown 内容传入方式

直接调用 `mcp_tdoc-doc_doc.create_with_markdown`，将 Markdown 内容 base64 编码后传入 `base64_markdown` 字段。长 Markdown 建议先落盘为本地文件，再用 shell 的 `base64` 命令编码后读取传入，避免手工拼接超长字符串出错。

推荐使用流程：
1. 准备好 Markdown 格式的文档内容，将其保存为 `<workspace>/.tmp/tencent_docs/<标题>.md` 文件；
2. 用 shell 的 `base64` 命令编码后作为 `base64_markdown` 传入本工具，从返回值中提取 `file_url` 即可访问文档；
3. 如需继续编辑，使用 `file_id`/`file_url` 和 `last_index` 调用其他 docengine 工具。

手动 base64 编码方法：
```bash
mkdir -p <workspace>/.tmp/tencent_docs
base64 -w 0 <workspace>/.tmp/tencent_docs/<标题>.md > <workspace>/.tmp/tencent_docs/encoded_<标题>.txt
# macOS 无需 -w 0
```
再用 `read_file` 读取 `encoded_<标题>.txt` 获取 base64 内容传入 `base64_markdown`。

### 调用示例
```json
{
  "base64_markdown": "IyDmoIfpopgKCui/meaYr+S4gOautSoq5Yqg57KXKirmlofmnKzjgII=",
  "title": "我的新文档"
}
```

### 参数说明
- `base64_markdown` (string, 必填): base64 编码的 Markdown 内容。长 Markdown 请先落盘为本地文件，再用 shell `base64` 命令编码后传入
- `title` (string, 可选): 文档标题

### 返回值说明
```json
{
  "file_id": "new_doc_file_id",
  "file_url": "https://docs.qq.com/doc/new_doc_file_id",
  "title": "我的新文档",
  "last_index": 100
}
```

---

## 34. doc.pre_insert_attachment

### 功能说明
在 Word 文档中预插入附件，获取上传链接和 object_key。客户端需使用 HTTP PUT 方法将文件上传到返回的 `upload_url`，上传完成后再调用 `insert_attachment` 完成附件插入。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "file_name": "报告.pdf",
  "size": 1048576,
  "ext": "pdf"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `file_name` (string, 必填): 附件文件名，如 "报告.pdf"
- `size` (integer, 必填): 附件大小，单位字节
- `ext` (string, 必填): 附件扩展名，如 "pdf"、"xlsx"，不含点号
- `is_multi_part` (bool, 可选): 是否分片上传，上传 >=5GB 的文件时必须为 true（默认 false）
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "upload_url": "https://upload.example.com/...",
  "object_key": "attachment_object_key_123"
}
```

---

## 35. doc.insert_attachment

### 功能说明
在 Word 文档指定位置插入附件。本接口是 `pre_insert_attachment` 的后续步骤：必须先调用 `pre_insert_attachment` 获取 `upload_url` 和 `object_key`，客户端使用 HTTP PUT 上传文件成功后，再调用本接口完成附件插入。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "idx": 10,
  "object_key": "attachment_object_key_123",
  "upload_success": true,
  "file_name": "报告.pdf",
  "layout_type": 2,
  "classify": 3
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 插入位置索引
- `object_key` (string, 必填): 文件标识 key，从 `pre_insert_attachment` 返回
- `upload_success` (bool, 必填): 文件是否上传成功
- `file_name` (string, 建议必填): 文件名，如 "报告.pdf"
- `size` (integer, 可选): 文件大小，单位字节
- `file_type` (string, 可选): 文件类型，如 "pdf"
- `layout_type` (number, 必填): 附件展示布局类型：`0` 文本态、`1` 图标+文本、`2` 卡片态、`3` 预览态
- `classify` (number, 必填): 附件分类：`0` 未知、`1` 视频、`2` 音频、`3` 文档、`4` 图片
- `text` (string, 可选): 附件显示文本。不传时自动用 `file_name` 回填
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 36. doc.insert_html_content

### 功能说明
在 Word 文档指定位置插入 HTML 内容，引擎自动转换为富文本格式。

> ⚠️ `idx` 必须是有效的段落起始位置，建议使用 `doc.resolve_document_structure` 获取普通段落的 `start_index`，确保 `idx` 落在真正的段落起始位置。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "idx": 10,
  "html_text": "<h1>标题</h1><p>这是<b>加粗</b>文本</p>"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 插入位置索引
- `html_text` (string, 必填): HTML 内容
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 37. doc.insert_numbering

### 功能说明
在 Word 文档中插入项目列表（编号/项目符号），支持设置编号类别和级别。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "start_idx": 5,
  "end_idx": 20,
  "abstract_num_id": "1",
  "type": "2",
  "numbering_lvl": "1"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `start_idx` (integer, 必填): 替换文本的开始位置
- `end_idx` (integer, 必填): 替换文本的结束位置
- `abstract_num_id` (string, 建议必填): 编号模板 ID，一般传 `"1"`
- `p_idx` (integer, 可选): 当前段落的结束位置
- `property` (object, 可选): 段落属性
- `type` (string, 必填): 编号类别
- `numbering_lvl` (string, 必填): 编号级别
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 38. doc.set_page_number

### 功能说明
设置页码（在页脚中插入 PAGE 字段）。默认参数组合：页面底部居中 + 1,2,3 格式 + 续前节 + 应用于整篇文档。调用前会自动切换为分页模式。

> ⚠️ 本工具与 `doc.insert_footer` 互斥，调用后会清空页脚内容再写入页码字段。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "position": "center",
  "format": "decimal",
  "continue_from_prev": true,
  "scope": "whole_doc"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `position` (string, 可选): 页码位置：`center`（默认）/`left`/`right`/`inside`/`outside`
- `format` (string, 可选): 页码格式：`decimal`（默认）/`romanLower`/`romanUpper`/`alphaLower`/`alphaUpper`
- `continue_from_prev` (bool, 可选): 是否续前节，默认 `true`。若为 `false`，使用 `start` 作为起始编号
- `start` (integer, 可选): 起始编号（`continue_from_prev=false` 时生效）
- `scope` (string, 可选): 应用范围：`whole_doc`（默认）/`from_here`/`current_section`
- `section_index` (integer, 可选): 节索引，仅在 `scope=current_section/from_here` 时生效
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 39. doc.set_table_layout

### 功能说明
设置表格的行高/列宽。通过表格内任意 cell 的 GCP 位置 `idx` 定位表格，支持两种模式：
- `mode=auto`：列宽自动按页面可用宽度均分；忽略 `row_heights_dxa/col_widths_dxa`
- `mode=manual`：按 `row_heights_dxa/col_widths_dxa` 逐项设置，值为 `0` 表示保持原值

> ⚠️ `mode=manual` 前建议先调用 `doc.get_table_info` 获取行列数和当前宽高，避免数组长度或单位使用错误。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "idx": 25,
  "mode": "manual",
  "row_heights_dxa": [0, 360, 360],
  "col_widths_dxa": [1800, 1800, 1800]
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 表格内任意 cell 的 GCP 位置（字符索引）
- `mode` (string, 必填): `auto` 或 `manual`
- `row_heights_dxa` (array, 可选): 每行行高数组（dxa）。仅 `manual` 生效，`0` 表示保持原值
- `col_widths_dxa` (array, 可选): 每列列宽数组（dxa）。仅 `manual` 生效，`0` 表示保持原值
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 40. doc.replace_bookmarks

### 功能说明
替换 Word 文档中书签标记范围的内容，支持文本和图片类型替换。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "samples": [
    {
      "bookmark_name": "company_name",
      "type": "text",
      "text": "腾讯科技"
    },
    {
      "bookmark_name": "logo",
      "type": "image",
      "image_info": {
        "content": "base64_encoded_image..."
      }
    }
  ]
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `samples` (array, 必填): 书签替换样本列表，每个元素包含：
  - `bookmark_name` (string): 书签名
  - `type` (string): 替换类型：`text`（文本替换）、`image`（图片替换）、`document`（文档替换）
  - `text` (string): 文本内容（type=text 时使用）
  - `image_info` (object): 图片信息（type=image 时使用），包含 `content`(base64) 或 `image_id`
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 典型工作流示例

### 用 Markdown 创建 Word 文档（推荐）

```
1. 准备好 Markdown 格式的文档内容，将其保存为 <workspace>/.tmp/tencent_docs/<标题>.md 文件（<标题> 为文档标题）
2. 使用系统 base64 命令进行编码，并将结果写入工作区目录下的文件（确保 agent 可通过 read_file 访问）：
   mkdir -p <workspace>/.tmp/tencent_docs
   base64 -w 0 <workspace>/.tmp/tencent_docs/<标题>.md > <workspace>/.tmp/tencent_docs/encoded_<标题>.txt
   或：echo -n "Markdown文本" | base64 -w 0 > <workspace>/.tmp/tencent_docs/encoded_<标题>.txt
   （macOS 上无需 -w 0 参数；<workspace> 为当前项目工作区根目录绝对路径）
3. 调用 manage.create_file 创建一个空 Word 文档（file_type=doc），获取返回的 file_id
4. 调用 doc.get_last_operable_pos（传入 file_id），获取文档末尾可操作的 position 和当前 version
5. 使用 read_file 工具读取步骤 2 生成的 encoded_<标题>.txt，拿到 base64 编码后的 Markdown 内容
6. 调用 doc.insert_markdown，传入 file_id、idx=position、base64_markdown（可选传 version_info.base_version=上一步的 version），将 Markdown 内容写入文档
7. 如需修改文档标题，调用 manage.rename_file_title
```

### 编辑已有 Word 文档

```
1. 调用 doc.get_outline 获取文档大纲结构，了解文档的标题层级和各区域的可操作位置
   （如需精确定位表格行列、文本框内部等，改用 doc.resolve_document_structure）
2. 根据大纲定位目标区域，或调用 doc.find 查找具体文本位置
3. 按需调用工具进行编辑：
   - 插入文本：doc.insert_text
   - 插入段落：doc.insert_paragraph
   - 替换文本：doc.replace_text
   - 全文替换：doc.find_and_replace
   - 修改文本样式：doc.update_text_property
   - 插入任务：doc.insert_task
   - 插入图片：doc.insert_image
   - 替换图片：doc.replace_image
   - 插入分页符：doc.insert_page_break
   - 插入表格：doc.insert_table
   - 插入批注：doc.insert_comment
   - 获取文档大纲：doc.get_outline
   - 获取完整结构树：doc.resolve_document_structure
```

### 查找并替换文本（精确替换）

```
1. 优先调用 doc.resolve_document_structure 获取结构树，并基于 text_preview 定位目标文本所在范围
2. 若通过 text_preview 无法可靠定位，再调用 doc.find 查找目标文本补充定位
3. 将候选位置展示给用户确认
4. 调用 doc.replace_text 传入对应的 ranges 进行精确替换
```

### 查找并替换文本（全部替换）

```
1. 直接调用 doc.find_and_replace，一次性替换所有匹配项
```

### 格式化文本

```
1. 调用 doc.find 查找目标文本，获取文本的 range
2. 调用 doc.update_text_property 设置文本属性（加粗、颜色等）
```

### 向文档末尾追加内容

```
1. 调用 doc.get_last_operable_pos 获取文档末尾可操作位置
2. 使用返回的 position 作为 idx，调用 doc.insert_text / doc.insert_image / doc.insert_table 等工具追加内容
```

### 在指定标题下插入内容

```
1. 调用 doc.get_outline 获取文档大纲，找到目标标题节点
2. 使用节点的 content_start 作为插入位置（在标题下方开头插入）
   或使用 content_end 作为插入位置（在标题下方正文末尾追加）
3. 调用 doc.insert_text / doc.insert_paragraph / doc.insert_image 等工具在对应位置插入内容
```

### 在文档开头插入内容

```
1. 调用 doc.get_outline 获取文档大纲
2. 明确用户意图——是要在「文档标题前」还是「正文开头」插入：
   - 文档标题前：使用 HEADING_LEVEL_TITLE 节点的 title_start 作为插入位置
   - 正文开头（标题之后）：使用 HEADING_LEVEL_TITLE 节点的 content_start 作为插入位置
3. 如果用户未明确说明，应主动询问用户确认具体插入位置
4. 确认位置后，调用 doc.insert_text / doc.insert_paragraph 等工具在对应位置插入内容
```

### 在表格指定行列插入文本

```
1. 调用 doc.resolve_document_structure 获取文档完整结构树
2. 在返回的 nodes 中找到目标 Table 节点
3. 通过 table_rows[row-1].cells[col-1].end_index 获取目标单元格的末尾位置
4. 调用 doc.insert_text，将 idx 设为该 end_index，即可在指定单元格末尾插入文本
```

### 在文本框内部插入内容

```
1. 调用 doc.resolve_document_structure 获取文档完整结构树
2. 在返回的 nodes 中找到目标 TextBox 节点
3. 通过 children 中的段落节点获取内部精确位置
4. 调用 doc.insert_text / doc.insert_paragraph 在对应位置插入内容
```

### 为文本添加批注

```
1. 调用 doc.find 查找目标文本，获取文本的 range（begin/end）
2. 调用 doc.insert_comment 传入 range 和批注内容
```

### 替换文档中的图片

```
1. 调用 doc.get_images 获取文档中所有图片信息，包括图片位置（pos/idx）和 URL/ID
2. 根据返回的 pos（作为 idx）和 url/id（作为 old_url/old_id）定位目标图片
3. 调用 doc.replace_image 传入对应参数完成图片替换
```

---

## 41. doc.get_table_info

### 功能说明
获取指定位置 `idx` 所在表格的整体信息。返回表格行高 / 列宽 / 合并情况 / 各单元格文本 + 属性，是 `doc.set_table_layout` / `doc.set_table_properties` / `doc.insert_rows` / `doc.insert_cols` 等表格写类工具的「前置查询」。

> ⚠️ `idx` 必须落在表格内部；若不确定，先用 `doc.resolve_document_structure` 找到 `type == "Table"` 的节点，再用其 `cells[].start_index` 作为 `idx`。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "idx": 25
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 表格内任意 GCP 索引（cell 内字符位置 / cell 起止 / 表格起止均可）
- `version_info` (object, 可选): 版本参数

### 返回值说明
```json
{
  "table": {
    "found": true,
    "start_index": 20,
    "end_index": 80,
    "row_count": 3,
    "col_count": 4,
    "has_alignment": true,
    "alignment": "left",
    "has_width": true,
    "width_type": "dxa",
    "width_value": 8640,
    "has_style": false,
    "style_name": "",
    "has_cell_margin_top": true,
    "cell_margin_top": 0,
    "has_cell_margin_left": true,
    "cell_margin_left": 108,
    "has_cell_margin_bottom": true,
    "cell_margin_bottom": 0,
    "has_cell_margin_right": true,
    "cell_margin_right": 108,
    "rows": [
      {
        "row": 1,
        "start_index": 21,
        "end_index": 35,
        "has_height": false,
        "height_pt": 0,
        "has_height_rule": false,
        "height_rule": ""
      }
    ],
    "cells": [
      {
        "row": 1,
        "col": 1,
        "start_index": 22,
        "end_index": 28,
        "grid_span": 1,
        "v_merge": "",
        "has_width": true,
        "width_type": "dxa",
        "width_value": 2160,
        "has_background_color": false,
        "background_color": "",
        "text": "标题",
        "text_property": {"bold": true},
        "paragraph_property": {"alignment": "center"}
      }
    ]
  },
  "version": 12,
  "trace_id": "trace_1234567890"
}
```

### 推荐使用流程
1. 调用 `doc.resolve_document_structure` 获取文档结构树，找到目标表格的 `cells[0].start_index`
2. 以该值为 `idx`，调用 `doc.get_table_info` 拿到完整表格信息
3. 根据返回的 `row_count` / `col_count` / 各 `cells[]` 的 `start_index`，再调 `doc.set_table_properties` / `doc.set_table_layout` / `doc.insert_rows` / `doc.insert_cols` 做后续修改

---

## 42. doc.delete_table

### 功能说明
删除 Word 文档中的整张表格。`idx` 只需传表格内任意位置（包括表头/表尾占位、任一单元格位置）。删除后表格后面的内容位置会发生偏移，请谨慎使用，需要先获取用户确认再使用。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "idx": 25
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 表格内任意位置索引，可以是 tblBegin / tblEnd 占位本身、也可以是任一单元格的起始位置；后端会自动反查整张表范围
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 43. doc.accept_all_revisions

### 功能说明
接受 Word 文档中的所有修订。

### 调用示例
```json
{
  "file_id": "doc_1234567890"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 44. doc.revert_revision

### 功能说明
反向指定版本号（`target_version`）的 revision 改动。采用 git revert 风格：取目标 revision 的反向命令，与之后的 revisions 做 OT 冲突处理后，作为新 revision 写入文档；不会真正删除任何历史 revision。

失败原因可能是：
1. 目标 revision 包含暂不支持反向计算的 mutation 类型
2. 反向命令在当前文档上无法合法执行
3. 目标 revision 的全部改动已被后续编辑覆盖

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "target_version": 5
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `target_version` (number, 必填): 待反向的目标 revision 版本号，必须 > 0 且 <= 当前最新版本
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```

---

## 45. doc.list_recent_ai_edits

### 功能说明
列出文档最近的 AI MCP 编辑记录，辅助按版本回退。返回每条记录的 version + tool_name + time（按时间倒序）。

**典型用法**：当用户要求"撤销刚才的 X 操作"（如删图、插表）时：
1. 先调本工具（limit 默认 10），拿到最近若干次 AI 编辑
2. 在返回列表中按 tool_name 与用户描述匹配（replace_image=换图 / insert_text=插文字 ...），找到目标 version
3. 调 `revert_revision`（file_id, target_version=匹配到的 version）完成回退

**限制**：
- 仅记录 AI 通过 MCP 工具产生的编辑，前端用户的手动编辑不在内
- Redis 存储，7 天 TTL；每个文档最多保留最近 200 条，超出会自动淘汰最早的
- limit 缺省 10，上限 50；tool_name 字段为可选过滤（精确匹配，例如 "doc_replace_image"）

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "limit": 10
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `limit` (number, 可选): 返回记录数，缺省 10，最大 50
- `tool_name` (string, 可选): 按工具名精确过滤（例如 "doc_replace_image" 只看换图记录）
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

### 返回值说明
```json
{
  "edits": [
    {
      "version": 5,
      "tool_name": "doc_insert_text",
      "time": "2024-01-01T12:00:00Z"
    }
  ]
}
```

---

## 46. doc.insert_math

### 功能说明
在指定位置插入由 LaTeX 渲染出的数学公式。引擎将 LaTeX 解析为 OOXML OMML 后展开为完整公式。

支持常用 LaTeX 语法：`\frac{1}{2}`（分式）、`x^{2}`（上标）、`\sqrt{x+1}`（根号）、`\sum_{i=1}^{n} i`（求和）、`\begin{matrix}1 & 2\\3 & 4\end{matrix}`（矩阵）等。

**行为**：插入按 OMML 结构展开的占位符序列 + 公式文本字面 + 各层 CTOMathPr 属性（分式 f、根号 rad、上下标 sSup/sSub、N 元运算 nary、矩阵 m、参数 e/num/den/sup/sub 等）。公式总长由实际 OMML 结构决定，下一个可写位置由 `last_index` 字段返回。

**错误处理**：latex 为空 / 语法错误 / OMML 解析失败 ⇒ 返回错误，不会写入文档。

### 调用示例
```json
{
  "file_id": "doc_1234567890",
  "idx": 10,
  "latex": "\\frac{1}{2}"
}
```

### 参数说明
- `file_id` (string, 可选): 文档唯一标识符，与 `file_url` 二选一
- `file_url` (string, 可选): 腾讯文档的文档链接，与 `file_id` 二选一
- `idx` (integer, 必填): 公式插入位置（GCP 字符索引）
- `latex` (string, 必填): LaTeX 数学表达式（非空）。支持常用语法：`\frac{a}{b}`（分式）、`x^{2}`（上标）、`x_{i}`（下标）、`\sqrt{x}`（根号）、`\sum_{i=1}^{n}`（求和）、`\int`（积分）、`\alpha`/`\beta`（希腊字母）、嵌套结构如 `\frac{x^{2}}{\sqrt{y+1}}`
- `version_info` (object, 可选): 版本参数，详见《通用说明 > 版本参数》

### 返回值说明
```json
{
  "base_version": 1,
  "new_version": 2,
  "last_index": 25,
  "trace_id": "trace_1234567890",
  "err_msg": ""
}
```
- `last_index` (int64): 公式插入后的下一个可写位置索引

---

## 注意事项

- 仅支持 Word 文档类型（doc_type: word）
- `idx` 参数表示插入位置，从 0 开始计数
- 操作前需确保拥有文档的写入权限
- `replace_text` 的 `ranges` 参数中 `begin` 和 `end` 必须在文档有效范围内
- 替换文本的推荐流程：优先调用 `doc.resolve_document_structure` 基于 `text_preview` 定位；若无法可靠定位，再调用 `doc.find` 补充查找；确认后再用 `doc.replace_text` 精确替换。如果需要全部替换可直接使用 `doc.find_and_replace`
- **所有 `doc.*` 工具均支持 `file_id` / `file_url` 二选一**；若用户提供的是文档链接（形如 `https://docs.qq.com/doc/<file_id>`），可直接传 `file_url`，也可先解析出 `file_id` 再传入
- `version_info`（`base_version` / `is_latest`）并非所有工具都支持，连续多步编辑时若目标工具支持，建议将上一步查询返回的 `version` 传入下一步的 `version_info.base_version`，避免并发冲突
- `doc.get_last_operable_pos` 返回的 `position` 即为文档末尾可安全插入内容的位置