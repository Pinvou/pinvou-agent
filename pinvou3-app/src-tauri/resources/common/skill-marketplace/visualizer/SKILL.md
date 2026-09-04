---
name: visualizer
description: 当用户要求数据可视化、做图表、生成看板、数据仪表盘、可视化报告、Excel/CSV 转图表、预算/销售/运营等指标分析页面、Chart.js 可视化时使用。只处理数据可视化任务；纯网页、banner、海报、简历等非数据图表设计任务应交给 visual-design。数据报告页以图表为主体归本技能；以文案排版为主体的静态报告页归 visual-design。
---

# 数据分析可视化

把结构化数据、表格汇总、业务指标或用户描述的数据口径转成可交付的 HTML 数据可视化仪表盘。

## 成功判定
一次合格交付必须同时满足：
- 已读取 `references/visualizer-design-system.md`。
- 已用 `write_file` 写出 `.html` 文件。
- 已用本技能目录下的 `scripts/validate_visualizer_html.py` 校验最终 `.html`，且结果为通过。
- 已调用 `present_artifact(path, title)` 展示产物卡。
- HTML 使用 Chart.js UMD：`https://cdnjs.cloudflare.com/ajax/libs/Chart.js/4.4.1/chart.umd.js`。
- 每个 `<canvas>` 都有 `role="img"`、描述性 `aria-label` 和 fallback text。
- Chart.js 默认 legend 关闭，并使用自定义 HTML legend。
- 数据先聚合再入图，展示数字经过合理四舍五入。
- 图表关键数值常驻显示，不只依赖 hover tooltip；数据密集时显示首尾、峰值、谷值或 Top N 等关键标签。

## 失败判定
如果发生以下任一情况，必须重写产物再交付：
- 使用 ECharts、Plotly、Vega、D3 伪地图或任何非 Chart.js 图表库，除非用户明确点名要求。
- HTML 中出现 `echarts`、`Plotly`、`cdn.plot.ly`、`cdn.jsdelivr.net/npm/echarts`。
- 直接在聊天正文粘贴完整 HTML，而没有写 `.html` 文件和展示 artifact。
- `<canvas>` 缺少 `role="img"`、`aria-label` 或 fallback text。
- 使用默认 Chart.js legend，而没有自定义 HTML legend。
- 图表数值只出现在 hover tooltip 中，画布上没有常驻数值标签。
- 使用彩虹渐变 KPI、重阴影、发光、深色 hero、emoji 或营销页式大标题。
- HTML/CSS/JS 中出现 `<!-- comments -->`、`/* comments */` 或独立行 `// comments`。
- `scripts/validate_visualizer_html.py` 返回失败。

## 必须先读
在执行任务前读取 `references/visualizer-design-system.md`，并遵守其中的 Chart.js、布局、配色、无障碍和流式输出规则。若任务很小，也至少遵守本文件的硬性规则。

## 交付方式
Pinvou 的聊天正文会转义或清理 `<script>`，所以不要把带 Chart.js 的 HTML 直接贴在普通回复正文里当最终成品。

必须按以下流程交付：
1. 用 `write_file` 写出一个 `.html` 文件，文件内容可以是完整可打开 HTML，必须包含可执行的 Chart.js 脚本。
2. 用 `exec_shell` 运行本技能目录下的校验器：
   `python <visualizer-skill-dir>/scripts/validate_visualizer_html.py <artifact.html> --json`
   其中 `<visualizer-skill-dir>` 是 `load_skill` 返回的 Source 所在目录。
3. 如果校验失败，读取错误列表，重写 `.html`，再次运行校验器；不要展示失败产物。
4. 只有校验通过后，才能调用 `present_artifact(path, title)` 展示产物卡。
5. 普通回复只保留简短说明，不重复粘贴整段 HTML。

## 触发边界
使用本技能：
- Excel/CSV/JSON/表格数据转图表、转看板、转可视化报告。
- 用户说“做数据可视化”“做图表”“生成看板”“数据仪表盘”“可视化图表”。
- 数据分析仪表盘、指标看板、可视化报告。
- 柱状图、折线图、组合图、散点图、热力图、图表卡片。
- 用户明确提到 Chart.js、canvas、数据可视化。

不使用本技能：
- 落地页、品牌页、banner、海报、简历、作品集等以视觉表达为主、没有数据图表核心诉求的任务。
- 需要真实地图但没有真实拓扑数据的任务；不要手绘伪地图。
- 需要在线查询最新数据但用户没有提供数据时，先说明需要数据源或使用可用查询工具获取数据。

## 数据纪律
- 不要编造真实业务数据。缺数据时先询问，或明确生成空模板/示例模板。
- 用户给出 Excel、CSV、JSON、表格或明细数据时，先做必要聚合，再写入图表。
- 所有展示数字都要四舍五入到合理精度。
- 图表解释写在普通回复中；HTML 产物内部只放视觉元素、必要标题、图例和简短标签。

## HTML 产物硬规则
- 输出 `.html` 文件，不输出 Markdown 包裹的 HTML。
- 使用 Chart.js UMD：`https://cdnjs.cloudflare.com/ajax/libs/Chart.js/4.4.1/chart.umd.js`。
- 每个 `<canvas>` 必须有 `role="img"`、描述性 `aria-label` 和 fallback text。
- 默认 legend 必须关闭，使用自定义 HTML legend。
- 默认在图表关键数据点上常驻显示数值标签，不只依赖 hover tooltip；柱状图显示在柱体末端或顶部，折线图显示在关键节点附近，饼图/环图显示分类占比。数据密集时只显示首尾、峰值、谷值或 Top N 等关键标签，避免重叠；完整数值保留在图例或 KPI 卡中。
- canvas 外层 wrapper 设置高度，canvas 本身不直接设置高度。
- Chart.js 配置里使用硬编码 hex，不使用 CSS 变量。
- 页面视觉要扁平、紧凑、无渐变背景、无阴影、无深色外层容器。
- 不写 HTML 注释、CSS/JS 块注释或行内叙事注释。
- 不写独立行 `//` 注释；生成脚本内也不要把解释性注释复制进最终 HTML。
- 不使用 emoji；需要图形标识时用 CSS 小色块或简洁 SVG。
- 字号保持紧凑：h1 15px、h2 14px、h3 13px、正文 13px；只使用 400 和 500 字重。
- 推荐结构：2-4 个 KPI 卡片、1 个宽趋势图、1-2 个辅助对比图、每个图表上方放自定义 legend。
- HTML 内只放视觉元素、标题、图例和必要标签；详细分析写在普通回复中。
- 若引用完整规范与本文件冲突，以本文件的交付方式为准。

## 交付前机器校验
本技能自带校验器 `scripts/validate_visualizer_html.py`，用于拦截常见违规项，包括注释残留、ECharts/Plotly、缺失 Chart.js UMD、canvas 无障碍缺失、默认 legend 未关闭、缺少自定义 legend、渐变/阴影/模糊/发光、异常字重、过小字号和 emoji。

校验器失败时必须按错误逐项修复并重跑，直到输出 `ok: true` 或文本 `OK visualizer artifact`。不要把“校验失败但看起来可用”的 HTML 交付给用户。
