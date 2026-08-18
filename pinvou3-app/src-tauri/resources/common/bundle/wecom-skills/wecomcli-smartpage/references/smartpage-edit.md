# 智能文档编辑 API 参考

针对特定智能文档的读写操作，包括读取页面内容、修改页面结构、追加内容、覆盖页面内容、获取关联智能表信息，以及两个典型的内容级工作流。

---

## 读取所有页面内容 (smartpage pages get)

根据智能文档的 docid 或 url，读取智能文档的完整页面树结构，包括页面名称、层级关系（通过 `parent_id` 字段表示父子关系）以及页面内容。不包含智能表格信息，智能文档包含的智能表格信息要通过 `smartpage databases get` 获取。

> `docid` 和 `url` 二选一传入即可，优先使用 `docid`。

```bash
wecom-cli smartpage pages get --json '<JSON参数>'
```

**请求参数 (JSON 格式传入):**

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `docid` | string | 否 | 文档 ID，与 `url` 二选一 |
| `url` | string | 否 | 文档 URL，与 `docid` 二选一 |
| `content_type` | string | 否 | 返回内容格式，可选 `markdown`（裸 Markdown 文本）/ `text`（纯文本页面内容）/ `block`（block 级 JSON，页面的 block 树）；其中 `block` 仅在编辑组件的场景下传入(用于获取组件 ID) |
| `page_id` | string | 否 | 指定页面 ID，传入则只返回该页面数据（`pages` 数组长度为 1）；不传则返回文档页面结构(标题、层级、page_id) |

**返回字段:**

| 字段 | 说明 |
| --- | --- |
| `doc_title` | 文档标题 |
| `pages` | 页面数组 |
| `pages[].page_title` | 页面标题 |
| `pages[].page_id` | 页面 ID，后续所有编辑操作必须取自此字段 |
| `pages[].parent_id` | 父页面 ID（可选），无此字段或为空表示该页面是根页面; 有值则表示该页面是 `parent_id` 对应页面的子页面 |
| `pages[].content_type` | 内容格式，回显请求中的 `content_type`（`markdown` / `text` / `block`） |
| `pages[].content_file_inner` | 页面内容，页面内容 ≤ 48KB 时直接返回文本内容于此字段中；文本格式由 `content_type` 决定：`markdown` → 裸 Markdown 文本，`text` → 纯文本页面内容，`block` → block 级 JSON（页面 block 树） |
| `pages[].file_path` | 页面内容 > 48KB 时返回，页面内容写入本地文件、回包本地文件路径；文件内容格式与 `content_file_inner` 相同，由 `content_type` 决定（`markdown` / `text` / `block`） |

> **提示**：传入 `page_id` 时，回包的 `file_path` 指向一个本地文件，文件内容根据 `content_type` 不同而不同。不带 `page_id` 时不返回 `file_path`。
> **读取文件**：拿到 `file_path` 后，使用 `read` 工具读取该路径下的文件内容，获取页面的完整数据。
> **页面层级**：`pages` 数组是扁平列表，通过 `parent_id` 字段表达树形结构。没有 `parent_id`（或为空）的页面是根页面; 有 `parent_id` 的页面是对应父页面的子页面。梳理页面树时，以 `page_id` 为节点、`parent_id` 为边构建层级关系。
> **注意**：`file_path` 中的文件编号仅用于保证文件名唯一，不代表任何业务 ID。所有 ID(如 `page_id`、`parent_id` 等)必须从实际回包字段中获取，禁止从文件名中提取。
> **读取页面结构**：在不知道 `page_id` 的情况下，不传 `page_id` 直接调用 `smartpage pages get`，可以获取文档的页面结构。如需查询页面详细内容，在下一次请求中指定page_id。

### 正文图片解析（markdown 内容作答类任务必做）

`content_type=markdown` 读回的页面正文里，原文中的图片会以 `![](<图片URL>)` 形式返回，`<图片URL>` 是**外部可直接访问的 CDN 链接**（通常形如 `https://w...qpic.cn/...`）。

**触发条件（同时满足才走本流程）**：
1. 用户诉求是**基于文档内容作答**（总结、抽取信息、问答、翻译、复述、依据文档回答问题等），而非纯粹的页面结构调整/重命名/搬运/覆盖写入等不需要理解图片内容的操作；
2. 读回的 markdown 中扫到 ≥1 条 `![...](<图片URL>)` 图片引用。

**处理步骤**：

1. **收集图片 URL**：读完 `content_file_inner` / `file_path` 指向的 markdown 后，扫描 `![...](...)` 语法，收齐所有图片的 URL（保留其在正文中的出现顺序，便于对齐上下文）。
2. **下载到本地**：对每个图片 URL，用**通用网络下载工具**（如 `curl -sSL -o <本地路径> <图片URL>`）落地到本地临时目录，得到本地图片文件路径。
   - 若下载失败（403 / 网络不通 / 链接过期），在最终回答中如实说明"第 N 张图片无法访问，未纳入分析"，继续处理其余图片，**不得**编造图片内容。
3. **交由外部图片解析能力识别**：拿到本地图片路径后，尝试使用外部能力解析每张图片的内容，把每张图片的识别结果与其在正文中出现的位置对齐。
   - 本 skill 不提供图片内容解析接口，也不代为 OCR；纯文本编辑类任务无需此步。
4. **合并作答**：把 markdown 正文文本 + 每张图片的识别结果作为整体上下文进行作答，必要时在回答中标注"图 N：<简述>"以便用户溯源。

**跳过条件**：以下场景**无需**下载和解析图片，直接按原始 markdown 处理即可：
- 用户仅要求调整页面树、重命名、移动、删除页面等**结构级**操作；
- 用户明确说"不用看图片"、"只根据文字回答"；
- 目标是把原页面内容整体搬运/覆盖到另一处（图片 URL 原样保留即可）。

---

## 上传附件到文档空间

将本地图片或其他文件（PDF、Office文档、`.zip` 压缩包等）上传到企业微信文档空间，返回文件对应的 URL。

根据文件类型选择上传命令：

- **图片**使用 `wecom-cli smartpage images upload`。
- **PDF、Office 文件、`.zip` 压缩包等非图片文件**使用 `wecom-cli smartpage files upload`。

两个命令的参数完全相同，文件内容支持两种传入方式（`file_path` / `media_id` 二选一，**优先使用 `file_path`**）：

```bash
# 图片 — 传本地文件路径（推荐）
wecom-cli smartpage images upload --json '{"file_path": "<本地文件路径>", "docid": "<文档ID>"}'

# 图片 — 传已上传的 media_id
wecom-cli smartpage images upload --json '{"media_id": "<media_id>", "docid": "<文档ID>"}'

# 非图片文件 — 传本地文件路径（推荐）
wecom-cli smartpage files upload --json '{"file_path": "<本地文件路径>", "docid": "<文档ID>"}'

# 非图片文件 — 传已上传的 media_id
wecom-cli smartpage files upload --json '{"media_id": "<media_id>", "docid": "<文档ID>"}'
```

**入参：**

| 参数 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `file_path` | string | 否 | 待上传文件的本地路径。与 `media_id` **二选一**，优先使用 |
| `media_id` | string | 否 | 已通过 `wecomcli-media` 的 `media upload` 获取到的媒体文件 ID。与 `file_path` **二选一**，仅当只能拿到 `media_id`（例如由其他 skill 转交）时使用 |
| `docid` | string | 是 | 目标文档的 ID |

**出参：**

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `url` | string | 上传后的文件访问 URL。图片返回直接图片资源 URL，通常形如 `https://w...qpic.cn/...`；非图片文件返回文件分享链接，通常形如 `https://d...qq.com/...?k=...` |

**调用示例：**

```bash
# 上传图片（本地路径）
wecom-cli smartpage images upload --json '{"file_path": "/path/to/image.png", "docid": "a1_xxx"}'

# 上传图片（已有 media_id）
wecom-cli smartpage images upload --json '{"media_id": "mcabc123...", "docid": "a1_xxx"}'

# 上传非图片文件（本地路径）
wecom-cli smartpage files upload --json '{"file_path": "/path/to/report.pdf", "docid": "a1_xxx"}'

# 上传非图片文件（已有 media_id）
wecom-cli smartpage files upload --json '{"media_id": "mcabc123...", "docid": "a1_xxx"}'
```

**成功示例：**

```json
{
  "url": "https://example.com/xxx/xxx"
}
```

---

## 修改页面结构 (smartpage pages update)

根据智能文档的 docid 或 url，执行页面级别的结构操作: 新建页面、删除页面、重命名页面、移动页面层级、修改页面布局。

> 每次调用传入一种操作类型。需要批量操作时，多次调用即可。例如新建页面：

```bash
wecom-cli smartpage pages update --json '{"docid": "<docid>", "create_page": {"page_name": "新页面标题", "parent_page_id": "<父页面ID>", "index": 0}}'
```

**请求参数 (JSON 格式传入):**

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `docid` | string | 否 | 文档 ID，与 `url` 二选一，只能使用编辑态的 ID（a1_xxx） |
| `url` | string | 否 | 文档 URL，与 `docid` 二选一 |
| `create_page` | object | 否 | 新建页面参数(五种操作互斥，每次传一种) |
| `create_page.page_name` | string | 是 | 新页面名称 |
| `create_page.parent_page_id` | string | 否 | 父页面 ID，为空则创建在根级别 |
| `create_page.index` | integer | 否 | 子页面目标位置索引 |
| `delete_page` | object | 否 | 删除页面参数 |
| `delete_page.page_id` | string | 是 | 要删除的页面 ID |
| `rename_page` | object | 否 | 重命名页面参数 |
| `rename_page.page_id` | string | 是 | 要重命名的页面 ID |
| `rename_page.new_name` | string | 是 | 新名称 |
| `move_page` | object | 否 | 移动页面参数 |
| `move_page.page_id` | string | 是 | 要移动的页面 ID |
| `move_page.new_parent_page_id` | string | 否 | 目标父页面 ID，为空则移动到根级别 |
| `move_page.index` | integer | 否 | 子页面目标位置索引 |
| `update_page_layout` | object | 否 | 修改布局参数 |
| `update_page_layout.page_id` | string | 是 | 要修改布局的页面 ID |
| `update_page_layout.layout` | string | 是 | 布局类型: `default`/`full_width`/`paper` |

> **注意**：通过 `delete_page` 删除某个页面时，其**所有子页面也会被一并删除**（级联删除），且**无法通过接口恢复**。**调用前必须向用户复述"将删除页面 `<页面名>` 及其所有子页面"并取得明确确认**，不得凭 plan 直接执行。删除后可重新调用 `smartpage pages get` 重新获取页面结构。

**返回字段:**

| 字段 | 说明 |
| --- | --- |
| `page_url` | 页面 URL |
| `page_title` | 页面标题 |
| `page_id` | 页面ID |

---

## 追加内容到页面 (smartpage pages append)

根据智能文档的 docid 或 url 及 page_id，在当前页面 block 序列的末尾插入单个或批量 block。

> 支持 markdown内容格式，通过 `content_type` 声明。
>
```bash
wecom-cli smartpage pages append --json '{"docid": "<docid>", "page_id": "<page_id>", "content_type": "markdown", "file_path": "{产出目录}/smartpage/<文件名>"}'
```

**请求参数 (JSON 格式传入):**

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `docid` | string | 否 | 文档 ID，与 `url` 二选一，只能使用编辑态的 ID（a1_xxx） |
| `url` | string | 否 | 文档 URL，与 `docid` 二选一 |
| `page_id` | string | 是 | 目标页面 ID，**必须来自 `smartpage pages get` 回包的 `pages[].page_id` 字段，禁止自行编造或从文件名推断** |
| `content_type` | string | 是 | 内容格式: `markdown` |
| `file_path` | string | 是 | 传参读取的本地文件路径，通过文件传递内容不受命令行长度限制，能避免内容被截断。|

**使用 `file_path` 传入文件的策略**:
- **已有现成文件时**：无需读写文件，直接将原始文件路径传入 `file_path`，原始文件可直接使用，不要求文件格式
- **内容需要现场构造时**：用 `write` 工具写入 `{产出目录}/smartpage/` 下，传入路径

**content_file 文件内容:**

| `content_type` | 文件内容格式 |
| --- | --- |
| `markdown` | 裸 Markdown 文本内容 |

**返回字段:**

| 字段 | 说明 |
| --- | --- |
| `status` | 操作状态 |

---

## 覆盖页面内容 (smartpage pages overwrite)

根据智能文档的 docid 或 url 及 page_id，**全量覆盖**页面内容——将原有 block 全部删除后重新创建。与 `smartpage pages append`（追加到末尾）互为对照，适用于整页重写的场景。

```bash
wecom-cli smartpage pages overwrite --json '{"docid": "<docid>", "page_id": "<page_id>", "content_type": "markdown", "file_path": "{产出目录}/smartpage/<文件名>"}'
```

**请求参数 (JSON 格式传入):**

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `docid` | string | 否 | 文档 ID，与 `url` 二选一，只能使用编辑态的 ID（a1_xxx） |
| `url` | string | 否 | 文档 URL，与 `docid` 二选一 |
| `page_id` | string | 是 | 目标页面 ID，**必须来自 `smartpage pages get` 回包的 `pages[].page_id` 字段，禁止自行编造或从文件名推断** |
| `content_type` | string | 是 | 内容格式: `markdown` |
| `file_path` | string | 是 | 传参读取的本地文件路径，通过文件传递内容不受命令行长度限制，能避免内容被截断。|

**使用 `file_path` 传入文件的策略**:
- **已有现成文件时**：无需读写文件，直接将原始文件路径传入 `file_path`，原始文件可直接使用，不要求文件格式
- **内容需要现场构造时**：用 `write` 工具写入 `{产出目录}/smartpage/` 下，传入路径

**content_file 文件内容:**

| `content_type` | 文件内容格式 |
| --- | --- |
| `markdown` | 裸 Markdown 文本内容 |

**返回字段:**

| 字段 | 说明 |
| --- | --- |
| `status` | 操作状态 |

---

## 获取关联的数据表信息 (smartpage databases get)

根据智能文档的 docid 或 url，获取智能文档关联的数据表 ID 及其子表列表。

**后续操作数据表**：拿到数据表 ID 后，委托 `wecomcli-smartsheet` 技能进行记录查询、编辑等操作。

> `docid` 和 `url` 二选一传入即可，优先使用 `docid`。

```bash
wecom-cli smartpage databases get --json '<JSON参数>'
```

**请求参数 (JSON 格式传入):**

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `docid` | string | 否 | 文档 ID，与 `url` 二选一，只能使用编辑态的 ID（a1_xxx） |
| `url` | string | 否 | 文档 URL，与 `docid` 二选一 |
| `table_name` | string | 否 | 指定子表名称，传入则只返回该子表信息（`tables` 数组长度为 1）；不传则返回所有子表信息 |

**返回字段:**

| 字段 | 说明 |
| --- | --- |
| `database_info.id` | 智能表 ID |
| `database_info.tables` | 子表数组 |
| `database_info.tables[].id` | 子表 ID |
| `database_info.tables[].name` | 子表名称 |

---


## 编辑页面 Block (smartpage blocks update)

根据智能文档的 docid 或 url 及 page_id，对页面内的指定 block 执行插入/替换/删除等细粒度编辑操作。通过 `method` 字段切换具体操作类型，单次调用仅支持一种 `method`，需要批量操作时多次调用即可。

> `docid` 和 `url` 二选一传入即可，优先使用 `docid`。

```bash
wecom-cli smartpage blocks update --json '<JSON参数>'
```

**请求参数 (JSON 格式传入):**

> `docid` 与 `url` **至少传一个**，两者均为空时校验不通过。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `docid` | string | 否 | 智能文档 ID (类型必须为 `smartpage`)，与 `url` 二选一 |
| `url` | string | 否 | 智能文档 URL，与 `docid` 二选一 |
| `page_id` | string | 是 | 目标页面 ID，必须来自 `smartpage pages get` 回包的 `pages[].page_id` 字段，自行推断或从文件名构造会失效 |
| `method` | string | 是 | 操作类型，枚举值: `insertBefore` / `insertAfter` / `prepend` / `append` / `replace` / `delete` |
| `mdx` | string | 条件 | MDX 内容片段，当 `method` 为 `insertBefore` / `insertAfter` / `prepend` / `append` / `replace` 时必传; 仅传入局部内容，无需外层 `<smartpage>` / `<page>` 标签 |
| `block_id` | string | 条件 | 参考目标块 ID。当 `method` 为 `insertBefore` / `insertAfter` / `replace` 时**必传**，用于定位单个目标 block; `prepend` / `append` / `delete` 不使用此字段 |
| `block_ids` | string[] | 条件 | 批量目标块 ID 列表。仅当 `method` 为 `delete` 时使用，支持传 1 个或多个 block ID; 其他 `method` 不使用此字段 |

**method 对应含义:**

| `method` | 含义 |
| --- | --- |
| `insertBefore` | 在 `block_id` 指向的 block **之前**插入新内容 |
| `insertAfter` | 在 `block_id` 指向的 block **之后**插入新内容 |
| `prepend` | 在页面**开头**插入新内容 |
| `append` | 在页面**末尾**追加新内容 |
| `replace` | 用 `mdx` 内容**替换** `block_id` 指向的 block |
| `delete` | 批量**删除** `block_ids` 列表中的 block |

**返回字段:**

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `status` | string | 操作状态，枚举值: `success` (成功) / `failed` (失败) |
| `block_id` | string | 参考的块 ID (回显请求中的 `block_id`, `method` 为 `insertBefore` / `insertAfter` / `replace` 时返回) |
| `inserted_block_ids` | string[] | 本次插入新生成的块 ID 列表 (`method` 为 `insertBefore` / `insertAfter` / `prepend` / `append` 时返回) |
| `deleted_block_ids` | string[] | 本次删除的块 ID 列表 (`method=delete` 时返回) |
| `new_block_id` | string | 替换后新块的 ID (`method=replace` 时返回) |

## 各操作类型调用示例

以下示例中的 `<docid>` / `<page_id>` / `<block_id>` 等均为占位符，实际调用前请先用 `smartpage pages get` 拉取最新内容，从回包中获取真实值后再替换填入。

### 1) 指定 block 之前插入 (insertBefore)

在 `block_id` 指向的 block 之前插入一段 MDX 内容:

```bash
wecom-cli smartpage blocks update --json '{
  "docid": "<docid>",
  "page_id": "<page_id>",
  "method": "insertBefore",
  "block_id": "<block_id>",
  "mdx": "<mdx>"
}'
```

### 2) 指定 block 之后插入 (insertAfter)

在 `block_id` 指向的 block 之后插入一段 MDX 内容:

```bash
wecom-cli smartpage blocks update --json '{
  "docid": "<docid>",
  "page_id": "<page_id>",
  "method": "insertAfter",
  "block_id": "<block_id>",
  "mdx": "<mdx>"
}'
```

### 3) 页面开头插入 (prepend)

在页面最顶部插入一段 MDX 内容 (无需 `block_id` / `block_ids`):

```bash
wecom-cli smartpage blocks update --json '{
  "docid": "<docid>",
  "page_id": "<page_id>",
  "method": "prepend",
  "mdx": "<mdx>"
}'
```

### 4) 页面末尾追加 (append)

在页面末尾追加一段 MDX 内容 (无需 `block_id` / `block_ids`):

```bash
wecom-cli smartpage blocks update --json '{
  "docid": "<docid>",
  "page_id": "<page_id>",
  "method": "append",
  "mdx": "<mdx>"
}'
```

### 5) 替换指定 block (replace)

把 `block_id` 指向的 block 替换为一段新的 MDX 内容，替换后的新 block ID 由回包 `new_block_id` 返回:

```bash
wecom-cli smartpage blocks update --json '{
  "docid": "<docid>",
  "page_id": "<page_id>",
  "method": "replace",
  "block_id": "<block_id>",
  "mdx": "<mdx>"
}'
```

### 6) 批量删除 block (delete)

一次性删除指定的一个或多个 block (此操作通过 `block_ids` 数组传入)，成功删除的 block ID 由回包 `deleted_block_ids` 返回:

```bash
wecom-cli smartpage blocks update --json '{
  "docid": "<docid>",
  "page_id": "<page_id>",
  "method": "delete",
  "block_ids": ["<block_id_1>", "<block_id_2>"]
}'
```
---

## 工作流一: 管理智能文档页面结构

**适用场景**：对已有智能文档进行**页面级**结构调整，如新建子页面、重命名页面、移动页面层级、修改页面布局、删除页面等（不涉及页面内部 block 内容的编辑，那类场景见下方工作流二）。

**所需能力**：`smartpage pages get`（获取所有页面数据） → `smartpage pages update`（多次调用，五种操作参数详见上文「修改页面结构」章节）。

- **先读取页面树**：调用 `smartpage pages get`（可省略 `content_type` 以减少返回数据量），从 `pages` 扁平列表中通过 `parent_id` 字段梳理出页面树结构——无 `parent_id` 的为根页面，有 `parent_id` 的为对应父页面的子页面。可传入 `page_id` 只获取指定页面数据，不传则返回所有页面。确认每个页面的 `page_id` 及父子关系后再调 `pages update`。
- **操作顺序建议**：批量调整时，顺序是**先新建 → 再移动/重命名/修改布局 → 最后删除**。这样可避免后续操作引用到已被删除的页面 `page_id`。
- **单次调用仅一种操作**：`smartpage pages update` 每次调用只能传入 `create_page`/`delete_page`/`rename_page`/`move_page`/`update_page_layout` 之一，批量调整需多次调用。
- **`page_id` 必须来自 `pages get` 回包**：禁止自行编造或从 `file_path` 文件名推断。

> **五种操作类型的完整参数表**（`create_page` / `delete_page` / `rename_page` / `move_page` / `update_page_layout` 各自的字段与可选项）详见上文「修改页面结构 (smartpage pages update)」章节，此处不再复述。

---

## 工作流二: 读取并修改已有智能文档内容

**适用场景**：读取当前页面内容并进行**页面内容级**修改——可以是局部修改某个 block，也可以是全量覆盖整个页面，也可以是末尾追加新内容。（若是页面级结构调整，走上方工作流一。）

**涉及接口**：`smartpage pages get`（获取所有页面数据） → `smartpage blocks update`（方案 A） / `smartpage pages append`（方案 B）/ `smartpage pages overwrite`（方案 C）

### 步骤一: 读取智能文档当前内容

在任何修改之前，**必须**先读取当前内容，以:
- 确认目标页面的 `page_id`
- 了解当前页面内容
- 避免覆盖他人的并发修改

**第一步 — 获取页面结构**（不带 `page_id`，仅返回标题、层级、page_id，**不含 `content` / `file_path`**）：

```bash
wecom-cli smartpage pages get --json '{"docid": "<docid>"}'
```

从返回 `pages` 数组中拿到各页面的 `page_id` 与层级关系，确认目标页面。

**第二步 — 获取目标页面内容**（带 `page_id` + `content_type`，此时才会返回 `file_path`）：

```bash
# 查看页面 markdown 内容（整页重写 / 末尾追加 / 查看文字）
wecom-cli smartpage pages get --json '{"docid": "<docid>", "page_id": "<page_id>", "content_type": "markdown"}'
# 查看页面 block 树（block 级局部编辑）
wecom-cli smartpage pages get --json '{"docid": "<docid>", "page_id": "<page_id>", "content_type": "block"}'
```

从返回结果中取得各页面的 `content_file_inner` / `file_path`，获取完整页面内容。

### 步骤二：编辑智能文档内容

| 修改规模 | 推荐方案 | 使用接口 |
| --- | --- | --- |
| 仅调整/修改/替换/删除/插入某个组件/内容，保留页面其他内容不变 | 方案 A（block 级局部编辑，首选） | `smartpage blocks update` |
| 保留原内容，在末尾追加新段落 | 方案 B | `smartpage pages append` |
| 整页重写（仅当用户明确要覆盖整页内容时选用） | 方案 C（markdown 格式） | `smartpage pages overwrite` |

#### 方案 A: block 级局部编辑（首选）

只动页面里的某个组件，保留其他内容不变。使用 `smartpage blocks update`：
- **前置条件：必须先读取 block tree**——调用 `smartpage pages get` 时**必须同时传入 `page_id` 和 `"content_type": "block"`**，从回包 `file_path` 文件中找到目标 block 的 `id`（即 `block_id`）以及需要定位的相邻 block。
- **选择 method**：插入 → `insertBefore` / `insertAfter` / `prepend` / `append`；替换 → `replace`；删除 → `delete`。各 method 的完整参数与调用示例见上文「编辑页面 Block (smartpage blocks update)」章节。
- **批量修改**：单次调用仅支持一种 `method`，多处修改需多次调用；批量删除可通过 `delete` + `block_ids` 数组一次完成。

#### 方案 B: 末尾追加

在当前页面末尾插入新内容，不影响已有内容。使用 `smartpage pages append`:
- 用 `write` 工具将 Markdown 文本写入 `{产出目录}/smartpage/` 下，通过 `file_path` 传入。

#### 方案 C: 全量覆盖页面

**仅当用户明确要求要覆盖整页内容时选用**。将原有所有内容删除后重新创建，**旧内容无法通过接口恢复，调用前必须向用户复述"将用新内容全量覆盖页面 `<页面名>` 的原有内容"并取得明确确认**。使用 `smartpage pages overwrite`:
- 用 `write` 工具将新的完整页面内容（MDX/Markdown，无需外层 `<smartpage>` 顶层标签）写入 `{产出目录}/smartpage/` 下，通过 `file_path` 传入。

### 步骤三：收尾检查

每次完成文档内容的写入（`pages append` / `pages overwrite` / `blocks update` / `smartpage import`）后，必须执行以下收尾步骤：

**命名一致性审查**：检查当前 **文档标题** 与 **各页面名称**，若名称中包含与内容强相关的信息（如日期、版本号、项目进度阶段等），需判断写入的新内容是否导致名称已过时或不准确：
   - 若名称需要更新（如周报日期已变、进度阶段已推进）→ 委托 `wecomcli-doc-manage` 对文档重命名，或调用 `smartpage pages update`（`rename_page`）对页面重命名。
   - 若名称仍准确 → 跳过，无需操作。

### 关键注意点

- **禁止用 overwrite 做局部替换**：用户要求替换/修改/删除页面中**某部分**内容时，**禁止**使用 `smartpage pages overwrite` 全量覆盖。必须走方案 A 做局部修改。overwrite 仅限用户明确要求覆盖整页内容时使用，不得作为局部编辑的捷径。
- **编辑前必须两阶段读取**：避免覆盖他人并发修改。先不带 `page_id` 调用 `smartpage pages get` 获取页面结构（只有标题、层级、page_id，**无内容**），再带 `page_id` + `content_type` 获取目标页面实际内容。
- **优先使用`file_path`**：无论 append 还是 overwrite，通过文件传递内容不受命令行长度限制，避免截断。
- **写文件用 `write`，读回包文件用 `read`**：为避免跨平台兼容问题，统一使用工具读写文件，不要手动拼接路径或直接操作文件。
- **`page_id` / `block_id` 必须从回包拿，禁止猜测**：不知道 `page_id` 时，先**不传** `page_id` 调 `smartpage pages get`，从回包 `pages[].page_id` 取值。`block_id` 取 `content_type=block` 时回包文件里 block 节点的 `id`。
- 调用 `pages append` / `pages overwrite` / `blocks update` 前，必须先 `smartpage pages get` 拿最新值；禁止使用缓存的旧值、自行编造、或从 `file_path` 文件名推断，否则会报「块不存在」错误。
- **保留原格式**：用户要求保留原格式时，以原文为基准修改，仅改动用户指出的部分，其余格式要素保持与原文一致。
- **只读组件保护**：页面中可能包含只读组件（如 `<flowChart hinaId="..." width="..." height="..." />`），写入时必须原样保留，禁止修改、删除或自行创建。
- **正文图片走通用下载 + 外部图片解析**：`markdown` 正文里的 `![](<URL>)` 是外部 CDN 直链，需要理解图片内容时用通用下载工具（如 `curl`）落地到本地后交给宿主 agent 的多模态图像读取能力解析；**禁止**把 URL 塞给 `wecom-cli media download`（它只吃 `media_id`）。纯结构/搬运/覆盖类任务无需下载图片，URL 原样保留即可。详见上文「正文图片解析」小节。
