# 创建 docx 文件

模型只需写一份 JSONL 描述文件（**不需要写 Python 脚本**），由分发器 `build_docx.py` 把每条命令派发到对应函数完成 `.docx` 生成

# 整体工作流

| 步骤         | 命令                                                                               | 说明 |
|------------|----------------------------------------------------------------------------------|---|
| 1. 写 jsonl | `File(action="write")` 工具                                                                       | 输出一个 `*.jsonl` 文件 |
| 2. 生成 docx | `python3 build_docx.py <*.jsonl>` | 自动应用默认样式 → 按 `action` 派发 → 输出 `.docx` |

> **`build_docx.py` 位置**：（即与 当前 `references/` 同级的 `scripts/` 目录下）

**用法约定**：

- JSONL 须写入宿主可读目录（`WECOMAGENT_READABLE_DIRS`），否则报路径越权
- 产物固定写入可写目录的 `docx/` 子目录，实际路径以脚本 stdout 的 `Successfully built` 行为准
- spec 文件名建议用 ASCII 字符并与目标文档标题一致（非 ASCII 词干会被替换为 `document`）

# JSONL 书写规范

## 格式

- 文件后缀：`.jsonl`
- 每行一个 JSON 对象，结构固定为：
  ```json
  {"action": "<函数名>", "params": {<入参对象>}}
  ```
- 每个 JSON 对象建议压缩到单行
- 解析器也容忍空行、注释行与多行 pretty-print

## 所有 action 一览

| Action | 用途                               |
|---|----------------------------------|
| `add_paragraph` | 段落（纯文本 / 列表样式 / Subtitle / 多 run 混排格式） |
| `add_heading` | **所有标题**：封面主标题（level=0，即 Title） + 章节标题（level=1~4） |
| `add_table` | 固定布局表格                           |
| `add_page_break` | 分页                               |

> **硬性规则**：任何"标题"性质的文本（封面主标题、章节/小节标题）一律使用 `add_heading`，**禁止**用 `add_paragraph` + `style: "Title"` 的写法。仅当确实需要"副标题段落"时才使用 `add_paragraph` + `style: "Subtitle"`。

## action详解

### `add_paragraph` — 段落

可写纯文本、套用列表样式、套用 Subtitle、或多 run 混排格式。

> 注意：本 action **不**用于生成"标题"。封面主标题、章节标题一律使用 `add_heading`。

| 参数 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `text` | string | — | 单 run 纯文本（与 `runs` 二选一；同时传以 `runs` 为准） |
| `runs` | array | — | 多 run 混排，元素见下表 |
| `style` | string | — | 内置样式名：`List Bullet` / `List Number` / `Subtitle` |
| `alignment` | string | — | 段落级对齐：`left` / `center` / `right` / `justify` |

`runs` 元素字段（仅字符级格式，无段落级字段）：

| 字段 | 类型 | 说明 |
|---|---|---|
| `text` | string | 文本 |
| `bold` / `italic` / `underline` | bool | 粗体 / 斜体 / 下划线 |
| `color_hex` | string | 字色（6 位 hex，可选前导 `#`，解析器两者都接受） |
| `size_pt` | number | 字号 |
| `font` | string | 西文字体 |
| `east_asia_font` | string | 中文字体 |

纯文本：
```jsonl
{"action": "add_paragraph", "params": {"text": "这是一段正文。"}}
```

Title / Subtitle（封面级）：
- **Title 必须用 `add_heading` + `level: 0`**，**禁止**写成 `add_paragraph` + `style: "Title"`
- Subtitle 才使用 `add_paragraph` + `style: "Subtitle"`

```jsonl
{"action": "add_heading", "params": {"text": "项目周报", "level": 0}}
{"action": "add_paragraph", "params": {"text": "2026 年第 22 周", "style": "Subtitle"}}
```

列表（必须用内置样式，绝不手写 `•` 或 `1.`）：

| 级别 | Bullet 样式 | Number 样式 |
|---|---|---|
| 0 | `List Bullet` | `List Number` |
| 1 | `List Bullet 2` | `List Number 2` |
| 2 | `List Bullet 3` | `List Number 3` |

> 内置最深 3 级；如需更深嵌套，应重组内容结构而非手写 `List Bullet 4`（不存在该样式，运行会报错）

```jsonl
{"action": "add_paragraph", "params": {"text": "一级要点", "style": "List Bullet"}}
{"action": "add_paragraph", "params": {"text": "二级要点", "style": "List Bullet 2"}}
{"action": "add_paragraph", "params": {"text": "编号 1",   "style": "List Number"}}
```

混排格式（同样要压缩到单行）：
```jsonl
{"action": "add_paragraph", "params": {"runs": [{"text": "重要："}, {"text": "请按时提交", "bold": true, "color_hex": "C00000"}, {"text": "，谢谢配合。"}]}}
```

### `add_heading` — 标题（封面主标题 + 章节标题）

**所有标题统一使用本 action**。封面主标题用 `level: 0`（对应 Word 的 Title 样式），章节标题用 `level: 1~4`。

| 参数 | 类型 | 默认 | 说明        |
|---|---|---|-----------|
| `text` | string | `""` | 标题文本      |
| `level` | int | 1 | 标题级别：`0` = 封面主标题（Title），`1~4` = 一~四级章节标题 |

```jsonl
{"action": "add_heading", "params": {"text": "项目周报", "level": 0}}
{"action": "add_heading", "params": {"text": "第一章 引言", "level": 1}}
{"action": "add_heading", "params": {"text": "1.1 背景", "level": 2}}
```

### `add_table` — 固定布局表格

| 参数 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `data` | array<array> | — | **必填**：二维数组，每个元素是一个 cell |

**Cell 的两种合法形态**（**不支持 `runs` 多 run 混排**）：

| 形态 | 示例 | 说明 |
|---|---|---|
| 字符串 | `"docid"` | 纯文本 cell |
| 单 run 对象 | `{"text": "字段", "bold": true, "color_hex": "FF0000"}` | 整个 cell 共享一组字符格式 |

Cell 对象支持的字段与 `add_paragraph.runs` 的元素字段完全一致：
`text` / `bold` / `italic` / `underline` / `color_hex` / `size_pt` / `font` / `east_asia_font`。

> **重要**：单元格内**无法做"段内局部高亮"**（即一句话里只标红其中几个字）。如有此类需求，请把高亮文本拆出表格，作为表格上方/下方的独立 `add_paragraph + runs` 段落。

写 `data`（二维数组），只描述每行内容，首行加粗即可。**整条 JSON 必须压缩到单行**：

```jsonl
{"action": "add_table", "params": {"data": [[{"text": "字段", "bold": true}, {"text": "类型", "bold": true}, {"text": "说明", "bold": true}], ["docid", "string", "文档 ID"], ["url", "string", "访问链接"]]}}
```

### `add_page_break` — 分页

无任何参数，传空对象即可。

```jsonl
{"action": "add_page_break", "params": {}}
```

## 五、完整示例

> 注意：示例展示的是**最终 jsonl 文件的真实形态**——每行一条 action，表格压缩为单行，行间无空行。

```jsonl
{"action": "add_heading", "params": {"text": "项目周报", "level": 0}}
{"action": "add_paragraph", "params": {"text": "2026 年第 22 周", "style": "Subtitle"}}
{"action": "add_heading", "params": {"text": "一、本周进展", "level": 1}}
{"action": "add_paragraph", "params": {"text": "完成核心模块开发，进入联调阶段。"}}
{"action": "add_paragraph", "params": {"text": "完成 API 设计评审", "style": "List Bullet"}}
{"action": "add_paragraph", "params": {"text": "完成 60% 核心代码", "style": "List Bullet"}}
{"action": "add_paragraph", "params": {"text": "联调开始", "style": "List Bullet 2"}}
{"action": "add_heading", "params": {"text": "二、风险提示", "level": 1}}
{"action": "add_paragraph", "params": {"runs": [{"text": "需重点关注："}, {"text": "依赖方接口延期", "bold": true, "color_hex": "C00000"}, {"text": "，预计影响排期 2 天。"}]}}
{"action": "add_heading", "params": {"text": "三、下周计划", "level": 1}}
{"action": "add_table", "params": {"data": [[{"text": "任务", "bold": true}, {"text": "负责人", "bold": true}, {"text": "DDL", "bold": true}], ["完成联调", "张三", "周三"], ["性能压测", "李四", "周四"], ["发版评审", "王五", "周五"]]}}
```
