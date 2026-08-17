# 幻灯片（Slide / PPT）品类入口

本文件是腾讯文档在线 PPT / 幻灯片 / 演示文稿任务的统一工作流入口。处理以下场景必须使用本入口：

- 从零生成一份 PPT（基于主题、材料、用户需求）。
- 续写已有 PPT（新增一页 / 加一页 / 在第 N 页后补一页 / 末尾追加一页）。
- 修改某一页（改文字、换图、调色、调整布局、重排版、整页重做）。
- 删除页 / 调整页面顺序。
- 检查或抽查 PPT 内容（页数、关键文本、空白页残留等）。

> 用户提到 "PPT"、"幻灯片"、"slide"、"演示文稿"、"这份 PPT"、"这页"、"下一页"，或在已有 PPT 上下文中说 "加一页"、"新增一页"、"补一页"、"再来一页"、"换成 xxx"、"改一下"，都使用本工作流。

## 0. 核心原则

- 在线腾讯文档 PPT 全部走 `tdoc-slide` 服务的 `mcp_tdoc-slide_slide_*` 工具。
- 任何写类 MCP 操作前必须有 DESIGN；没有 DESIGN 时通过 `slide_set_design` 的 `design_md` 参数直接持久化（禁止写本地 .md 文件再传参，造成重复）。
- 0-1 构建整份 PPT、增加一页、整页重排，统一用 JSX + `slide_add_page_with_jsx`。
- 修改某一页时，先生成脚本或批量调用方案，在脚本里调 MCP 工具，避免反复多轮调用。
- 只保留必要结构化结果，不保存 MCP 的大段原始返回。
- ⭐ **风格参考（重点约束）**：当用户指定一份 PPT 作为模板、或要求“参考这份 PPT 的风格 / 排版 / 配色”时，**必须先用 `slide_export_pages_to_image_urls` 把参考 PPT 的关键页渲染为图片**，再以这些图片为视觉锚点驱动 DESIGN 与 JSX 生成（详见 3.5 节）。禁止凭空臆测参考 PPT 的视觉风格。**注意**：风格参考只复用配色 / 字体 / 母版 / 版式 / 视觉锚点等"视觉风格"维度，**幻灯片尺寸（`w_pt` / `h_pt` / 宽高比）一律以目标 PPT 当前尺寸为准**，禁止用参考 PPT 的尺寸覆盖目标 PPT，也禁止主动调用 `slide_set_slide_size`（除非用户显式要求改尺寸）。
- ⭐ **页面顺序（重点约束）**：所有写类操作（0-1 构建 / 加页 / 整页重排）都必须以“最终成品页面顺序与大纲完全一致”为唯一验收标准。**`slide_add_page_with_jsx` 严禁并发调用，必须严格串行**（一次只发起一个 `slide_add_page_with_jsx` 调用，等其返回后再发起下一个）；同时每次调用都必须**显式传 `page_index`**，禁止依赖 MCP 默认追加行为，也禁止依赖“调用顺序决定页面顺序”；完成后走 6.3 节的顶层顺序校验。

---

## 1. 获取当前 PPT 基本信息

任何 Slide 工作流的第一步都必须归并 PPT 状态。分两步：

1. 用 MCP 工具取数（`slide_get_info` → `slide_get_page_info`（每页）→ `slide_get_design`；slide_count=0 且尺寸有效时再加 `check_access` 验权限）。
2. 把各工具的 JSON 响应按顺序（有 check_access 则其响应放第一行，`--with-access`）通过 stdin 喂给状态脚本归并：

```bash
printf '%s\n' "$INFO_JSON" "$DESIGN_JSON" ${PAGE_JSONS[@]} \
  | bash sidebar-pptx-generator/scripts/get_slide_info.sh
```

脚本要求：

- 只消费 stdin 上预先取好的 `slide_get_info` / `slide_get_design` / 每页 `slide_get_page_info` 响应（第一行 info、第二行 design、后续每页一行），不做网络请求、不经手 Token。
- 不把 MCP 原始大 JSON 写入上下文。

示例输出：

```json
{"action":"write_design_md","reason":"design_is_empty","slide_count":1,"w_pt":960,"h_pt":540,"content_page_count":1,"design_exists":false}
```

根据 `reason` 决策：

| reason | 含义 | 下一步 |
|---|---|---|
| `permission_denied` | 当前账号无 VIEW 权限 | 提示用户分享文档权限或下载到本地后重试 |
| `ppt_is_empty` | 页数为 0 或尺寸为 0 | 按 0-1 构建整份 PPT |
| `ppt_content_is_empty` | 有页面但全是空占位符 | 按 0-1 构建；新页完成后删除空白页 |
| `design_is_empty` | PPT 有内容但没有 DESIGN | 先写 DESIGN，再继续目标任务 |
| `design_exists` | 已有 DESIGN | 直接进入 0-1 续写、增加页或修改页 |

> **创建空白 PPT 的入口**：如果用户希望从一份不存在的 PPT 开始，先用 `manage.create_file`（doc_type=`slide`）创建空白 PPT，拿到 `file_id` / `file_url` 后再走本工作流。创建出来的 `file_url` 在本次任务完成时必须按第 6.1 节展示给用户。

---

## 2. 写 DESIGN.md（写类操作前置）

当状态脚本返回 `action=write_design_md`，或任何写类 MCP 工具需要 DESIGN 时，先完成本节。后续流程默认已经满足这个前置条件，不再重复声明。

执行步骤：

1. 读取 `sidebar-pptx-generator/references/design-principle.md`。
2. 结合用户需求、当前 PPT 状态、已有内容、材料摘要和搜索事实，在上下文中构思 DESIGN 内容（视觉与落地契约，不含大段叙事）。
3. **禁止写本地 .md 文件**。直接调用 MCP 工具 `slide_set_design`，将完整内容通过 `design_md` 参数传入即可持久化；只有当内容过长可能导致 MCP 截断时，才使用脚本方式（见 `slide_set_design` 工具描述中的 set_design.py）。
4. DESIGN 已在当前会话中拿到或刚写入后，不要重复调用 `slide_get_design`。

DESIGN.md 的结构、字段和质量门禁以 `sidebar-pptx-generator/references/design-principle.md` 为准；本文件不重复展开。

---

## 3. 网络搜索与信息收集

不是所有 PPT 都需要搜索。只在信息不足、需要事实支撑或主题有时效性时搜索。

必须搜索的情况：

- 市场规模、行业趋势、公司动态、政策法规、新闻事件、竞品对比、投资/财务数据。
- 用户要求"最新""有数据""有案例""更有说服力"。
- 出现陌生术语、缩写、产品名、公司名、行业名。
- 页面需要年份、增长率、排名、价格、时间线、政策名称等具体事实。

可以不搜索的情况：

- 用户明确要求严格基于上传材料。
- 材料已经给出完整事实和结论。
- 任务只是排版、改色、增加结构页或局部编辑。
- 纯内部经验、个人计划、模板型 PPT。

---

## 3.5 模板 / 风格参考（参考 PPT 截图驱动）

当用户指定一份 PPT 作为**模板 / 风格参考**时，必须按本节流程把参考 PPT 转成截图后再开始创作。典型触发说法：

- “以这份 PPT 为模板”、“按这个 PPT 的风格做”、“参考这份 PPT 的排版 / 配色 / 字体 / 母版”；
- 同时给出一份参考 PPT 的 `file_id` / `file_url` 和一份目标 PPT（待创作 / 待修改）的 `file_id` / `file_url`；
- 要求“做得跟这份一样好看”、“沿用这份的视觉感觉”、“延续这份的封面 / 章节页样式”。

### 3.5.1 截图采样流程

1. **决定要采哪些页**。优先采**有代表性**的页面，而不是全量采：
   - 必采：封面、目录 / 章节页、典型内容页、数据 / 图表页、结束页；
   - 用户明确点名某几页（如“看第 3、5、12 页”），则只采这几页；
   - 用户没说明、且参考 PPT 总页数 ≤ 12 时，可以全量采（`page_indices` 留空）；
   - 总页数较多且没有明确指向时，按上面“必采”清单挑出 4 ~ 8 页即可，避免上下文膨胀。
2. **调 `slide_export_pages_to_image_urls`**：
   ```json
   {
     "file_id": "<参考 PPT 的 file_id 或 file_url>",
     "page_indices": [0, 2, 4, 6, 9],
     "long_edge": 1600
   }
   ```
   - `page_indices` 是 **0-based**；不传则导出全部页（仅推荐在小 PPT 上使用）。
   - 像素优先用 `long_edge`（1280 ~ 1920 之间，足够看清版式即可），不必同时传 `width` / `height`。
   - 工具返回每页对应的图片 URL；把 URL 列表与对应原始 `page_index` 一一对齐保存。
3. **读取截图作为视觉输入**。把返回的图片 URL 直接作为多模态输入读图，提取以下要素并写入上下文（不要把图片 URL 长串原样塞进最终 PPT）：
   - 配色：主色 / 辅助色 / 强调色（HEX）、深浅背景、中性灰阶；
   - 字体：标题 / 正文 / 数字字体族，是否使用衬线；
   - 母版结构：标题位置、页码 / 页脚位置、装饰线 / 角标 / 视觉锚点；
   - 版式节奏：封面 / 章节页 / 内容页 / 数据页 / 结束页的典型布局；
   - 信息密度：每页平均字数、是否大量图表 / 图片、是否常用分栏。
   - ⚠️ **不要**提取 / 抄录参考 PPT 的画布尺寸（`w_pt` / `h_pt` / 宽高比）。目标 PPT 的尺寸以第 1 节 `get_slide_info.sh` 返回的 `w_pt` / `h_pt` 为唯一基准，全部 JSX 都按这套尺寸排版。
4. **把视觉要素落到 DESIGN**。在执行第 2 节 `slide_set_design` 之前，先把上面提炼出的色板、字体、母版、视觉锚点、密度约束写进 DESIGN，并在 DESIGN 顶部注明“风格参考自 file_id=<...> 第 [a, b, c] 页”，方便后续核对。**画布尺寸**（DESIGN 中如有 `slide_size` / `canvas` / `w_pt` / `h_pt` 之类字段）必须使用**目标 PPT 当前的实际尺寸**（即第 1 节 `get_slide_info.sh` 返回的 `w_pt` / `h_pt`），**不得**抄录参考 PPT 的尺寸；如果参考 PPT 与目标 PPT 宽高比不一致，所有版式 / 留白 / 字号要按目标 PPT 的画布做等比适配，而不是反过来调整目标 PPT 的尺寸。
5. **JSX 生成阶段持续对标**。生成每页 JSX 前，对比同类型参考页的截图（封面对封面、内容页对内容页），确保版式、字号、留白节奏接近；不一致时优先调 JSX 而不是改 DESIGN。

### 3.5.2 适用场景与禁止行为

| 场景 | 是否走本节流程 |
|---|---|
| 用户指定参考 PPT 做 0-1 创作 | ✅ 必须（在 4.1 流程开始前完成） |
| 用户指定参考 PPT 给已有 PPT 做整页重排 / 改风格 | ✅ 必须（在 4.3 整页重排前完成） |
| 用户只让加 / 改一页，但要求“跟前面几页风格一致” | ✅ 推荐（采样目标 PPT 自身相邻页即可，不需要外部参考） |
| 用户没给参考 PPT，也没要求模仿任何风格 | ❌ 不调用，避免无谓的图片生成开销 |
| 仅做文字替换 / 局部改色等不涉及版式判断的局部编辑 | ❌ 不调用 |

禁止行为：

- 禁止在没有调用 `slide_export_pages_to_image_urls` 的情况下，凭主观印象描述参考 PPT 的“风格”；
- 禁止把返回的图片 URL 直接写进目标 PPT 的 JSX（这些 URL 是临时签名 URL，会过期）；如确需在目标 PPT 中嵌入参考截图，应另行通过图片上传 / 资源接口落库；
- 禁止把整批截图原样塞回上下文反复传递，只保留提炼后的视觉要素摘要 + 必要的少量缩略图引用。
- 禁止用参考 PPT 的画布尺寸 / 宽高比覆盖目标 PPT；不得为了“和参考 PPT 一致”而调用 `slide_set_slide_size` 修改目标 PPT 尺寸（除非用户显式要求改尺寸）。目标 PPT 的尺寸是创作的硬约束，参考 PPT 只贡献“视觉风格”，不贡献“画布形状”。

---

## 4. 意图分类处理

### 4.1 0-1 构建整个 PPT

适用：用户要求"生成一份 PPT / 从零做 / 基于主题做完整汇报 / 当前 PPT 是空白页"。

流程：

0. **如有参考 PPT，先按 3.5 节完成风格参考采样**（`slide_export_pages_to_image_urls` 渲染 + 视觉要素提炼）。提炼出的色板 / 字体 / 母版要在第 3 步写 DESIGN 时一并落地。
1. 基于用户需求 + 材料摘要 + 搜索事实生成大纲。
2. 大纲至少包含：页码、标题、类型、角色、核心观点、证据/素材、版式建议、视觉角色、禁止项。**页码必须连续递增（1, 2, 3, ... N）**，作为后续生成与校验的唯一顺序基准。
3. 按第 2 节完成 DESIGN 前置。
4. 写 JSX 前读取组件语法规范 `sidebar-pptx-generator/references/component-*.md`。
5. 每页用到什么组件，再读取对应 `sidebar-pptx-generator/references/component-*.md`，不要一次读全。
6. **严格串行生成 N 页 JSX 并调用 `slide_add_page_with_jsx` 写入（严禁并发）**：
   - **一次只发起一个 `slide_add_page_with_jsx` 调用，等该次调用返回成功后再发起下一个**。严禁在同一轮里并发发起多个 `slide_add_page_with_jsx`（会造成页面顺序错乱 / 版本冲突 / 底层锁争抢）。
   - 每次调用都必须**显式传 `page_index`**；绝不依赖 MCP 默认追加行为，也不依赖“调用顺序决定页顺序”。
   - **推荐范式——末尾追加策略**：先记录当前总页数 `base = slide_count`（包含原始空白占位页），然后串行依次插入大纲第 k 页（1 ≤ k ≤ N），统一使用 `page_index = base + k - 1`；待 N 页全部串行成功后再统一删除原始占位空白页（第 7 步）。
   - 任一页插入失败不要重试到不同 `page_index`；在原 `page_index` 上重试或记录失败页后统一补发（同样串行）。
7. 如果原 PPT 是空白占位页，先确认 N 页都成功插入且顺序正确，再删除原空白页。
8. 抽查封面、内容页、结束页的 page_info，确认关键文本和页数。
9. **顺序校验**：调用 `slide_get_info` 拿到当前页面列表，按 `page_index` 升序与大纲逐页核对（封面 / 关键内容页 / 结束页的标题文字至少抽 3 处），不一致则用 `slide_move_slide` / `slide_move_section` 等顺序调整工具修正，**严禁**通过"删 + 重插"绕过。

大纲基本格式：

```markdown
| # | 标题 | 类型 | 角色 | 核心观点 | 证据/素材 | 版式建议 | 视觉角色 | 禁止项 |
|---|---|---|---|---|---|---|---|---|
```

JSX 基本约束：

- 每页是一个 JSX 字符串。
- 最后一个表达式必须是 `<Slide>`。
- 禁止 `import` / `export` / hooks / state / 自定义 React 组件。
- 服从 DESIGN 的颜色、字体、母版、页脚、视觉锚点和密度约束。

### 4.2 增加一页

适用：用户要求"加一页 / 插入一页 / 在第 N 页后补一页 / 增加案例页或数据页"。

流程：

1. 调用 MCP 工具 `slide_get_page_info` 读取插入位置前后页面，提取标题位置、页脚、主色、版式节奏。
2. 为新增页写一个简洁的提纲：标题、核心观点、证据/素材、视觉角色、禁止项。
3. 读取需要使用的组件规范 `sidebar-pptx-generator/references/component-*.md`。
4. 按当前 DESIGN 生成单页 JSX。
5. 调用 MCP 工具 `slide_add_page_with_jsx` **显式传 `page_index`** 插入。
6. 插入后只读取新页一次确认；同时调用 `slide_get_info` 校验目标 `page_index` 处确实是新页，前后页位置未被挤错。

位置规则（page_index 全部为 0-based）：

- "第 N 页后"（用户口径 1-based）：`page_index = N`（即新页落在原第 N+1 页位置，原第 N+1 页及之后整体后移）。
- "作为第 N 页"（用户口径 1-based）：`page_index = N - 1`。
- 用户没说明位置：追加到末尾，`page_index = 当前总页数`。
- **批量加多页（严禁并发）**：必须**严格串行**逐页调用 `slide_add_page_with_jsx`，一次只发一个，等返回成功后再发下一个。为这一批预先计算出一个**互不重叠且连续的 `page_index` 区间**（例如“末尾一口气加 K 页”则为 `[base, base+K-1]`，`base = 开始之前的总页数`），在本批调用期间不要再读取“实时总页数”去动态计算；依次串行逐页插入即可。

### 4.3 修改某一页

适用：用户要求"修改第 N 页 / 替换文字 / 换图 / 调整布局 / 批量改色 / 删除元素"。

流程：

1. 调用 MCP 工具 `slide_get_page_info(file_url/file_id, page_index)` 获取目标页元素。
2. 根据返回中的 shape_id、text、bounds、fill、font 建立修改清单。
3. 生成 sh 脚本或批量调用方案，在脚本中调用 MCP 工具完成修改。
4. 脚本执行后只读取目标页一次确认。

常用工具：

- 改文本：`slide_set_text` / `slide_find_replace_text`
- 改位置、大小、样式：`slide_set_shape_properties`
- 新增元素：`slide_add_text` / `slide_add_shape` / `slide_add_image` / `slide_add_chart`
- 删除元素：`slide_remove_shapes`

如果用户要求接近整页重排，例如"大改布局 / 重新设计这一页"：

0. **如用户同时指定了参考 PPT，先按 3.5 节完成风格参考采样**，提炼版式 / 配色后再开始重排。
1. 读取旧页 page_info，保留必要文本和数据。
2. 根据 DESIGN 生成新 JSX。
3. 在旧页后插入新页。
4. 确认新页成功。
5. 删除旧页。

> 关于 `slide_*` 全量工具的字段、Schema 与边界条件，参见 `references/slideengine_references.md`。

---

## 5. 修改页脚本策略

修改某页时，避免"读一次、改一点、再读、再改"的多轮循环。满足任一条件就写脚本：

- 需要执行 3 个以上写类 MCP 操作。
- 需要遍历多个 shape。
- 需要批量替换文本、颜色、位置或样式。
- 需要根据 page_info 后处理出 shape_id 清单。

脚本本身只做编排与结果归并，MCP 工具调用（`mcp_tdoc-slide_*` 等）由 Pinvou 在对话工具循环中执行，脚本不经手 Token：

脚本要求：

- 输入：file_url/file_id、page_index、必要业务参数。
- 输出：单行 JSON。
- 成功：`{"ok":true,"changed":N,"page_index":N}`。
- 失败：`{"ok":false,"error":"..."}`。
- 不输出大段 MCP 原始响应。

---

## 6. 完成后的校验与回复

完成后至少确认：

- 页数是否正确。
- 关键文本是否存在。
- 是否残留空白占位页。
- 是否符合 DESIGN 的母版、颜色、字体和视觉锚点。
- **页面顺序与大纲一致**（详见 6.3 节）。

### 6.1 必须给出 PPT 链接（强制）

任何写类工作流完成后（创建空白 PPT、0-1 构建整份、增加页、修改页、删除/重排页等），**必须在回复正文里把当前 PPT 的可点击链接给用户**。用户点击链接即可在腾讯文档中实时看到最新内容。

- 链接来源优先级：
  1. 用户输入里已经带的 `file_url`，直接沿用。
  2. 用户只给了 `file_id`，则拼成 `https://docs.qq.com/slide/<file_id>`。
  3. 通过 `manage.create_file`（doc_type=`slide`）创建出来的新 PPT，直接使用接口返回的 `file_url`。
- 链接展示格式：用 Markdown 链接，例如 `[在腾讯文档中打开](https://docs.qq.com/slide/<file_id>)`，不要只贴裸 URL，也不要藏在折叠区或代码块里。
- 链接放置位置：放在"完成内容总结"之后、"风险点 / 待确认"之前；如果有多次操作合并回复，只展示一次最新链接即可。
- 即使是只读类工作流（如仅做检查、抽查页面），如果用户问"我在哪里看"，也要给出同样格式的链接。
- 禁止编造或修改链接里的 `file_id`；禁止给临时下载链接（如导出产生的带签名 URL）冒充 PPT 在线链接。

### 6.2 回复内容

回复用户时只说：

- 完成了什么。
- 影响了哪些页。
- 当前 PPT 链接（按 6.1 给出）。
- 是否有需要用户确认的风险点。

不要输出内部长流程。

### 6.3 顺序校验（强制）

任何写类工作流完成后，必须做一次"页面顺序对账"，确保成品页面顺序与大纲 / 用户预期一致：

1. 调用 `slide_get_info` 拿到所有页 `page_index` 升序列表。
2. 与本次任务的大纲（0-1 构建）或用户指定的目标位置（加页 / 改页 / 重排）逐页比对。比对粒度：
   - 0-1 构建：每个 `page_index` 上的标题文本必须与大纲第 `page_index + 1` 行一致；
   - 加页：新页应当出现在用户期望的 `page_index` 上，前后相邻页不应被挤错；
   - 重排 / 删页：剩余页的相对顺序与用户指令一致。
3. 发现错位时优先用 `slide_move_slide` / `slide_move_section` 等顺序工具修正，**禁止**通过"删除原页 + 重新插入"的方式绕过——这种做法会丢失页内 shape_id、批注、动画绑定关系。
4. 顺序校验通过前不要回复"完成"；如果修正后仍无法对齐大纲，必须在最终回复的"风险点 / 待确认"里明确列出实际顺序与大纲顺序的差异。

---

## 7. 相关参考文档

| 文档 | 用途 |
|---|---|
| `sidebar-pptx-generator/references/design-principle.md` | DESIGN.md 编写规范（结构、字段、自检门禁） |
| `sidebar-pptx-generator/references/component-*.md` | JSX 组件语法规范（box / chart / codeblock / diagram / faicon / image / math / qrcode / slide / svg / table / text 等） |
| `sidebar-pptx-generator/scripts/get_slide_info.sh` | 状态脚本，工作流入口 |
| `sidebar-pptx-generator/scripts/setup.js` | slidep 工具链安装 / 升级（按需） |
| `sidebar-pptx-generator/scripts/doc_image_extractor.py` | 文档图片素材提取（按需） |
| `references/slideengine_references.md` | `slide_*` 系列工具完整 API Schema（精细编辑工具，含 `slide_export_pages_to_image_urls` 的字段说明） |
