---
name: pptx
description: 本地直出可编辑 PowerPoint（.pptx）：用户要做 PPT / 幻灯片 / 演示文稿 / 汇报 / 方案时使用。先列大纲确认，再产结构化 deck 并按内容自动选主题，调 mcp_pptx_make_pptx 生成，产物必须上产物卡。
---

# PPT 生成

本地生成可编辑 .pptx：模型产结构化大纲，渲染器套主题模板生成，真·可编辑图表，自带封面缩略图，数据不出机。

## 流程（严格按序）

1. **先列一版大纲**（章节 + 每页要点）给用户确认/修改
2. 确认后产结构化 deck：封面 / 目录 / 章节 / 要点 / 双栏 / KPI / 图表 / 表格 / 配图 / 结尾
3. **按 PPT 内容自动选一套主题**（business-blue / tech-dark / gov-red / creative-purple / fresh-green / warm-orange / navy-gold / minimal-mono / midnight），并一句话说明理由，用户可改
4. 调 `mcp_pptx_make_pptx` 生成 .pptx（连接器工具默认延迟加载：工具列表里没有它时先 `tool_search` 激活；`tool_search` 也搜不到，说明『PPT 生成』连接器未安装——按下方「严格禁止」处理，不要硬拼成品）
5. 拿到 path 后**必须再调 `mcp_pinvou3_present_artifact(path, title)`** 上产物卡（若你的工具列表里没有这个工具，说明产物卡后端本轮不可用——直接交付并在回复里说明产物卡不可用）

## 严格禁止

- 全程**不要用 HTML 代替** .pptx
- 没点名平台时默认本地生成，**不要用飞书/在线文档代替**——只有用户明确说"做成飞书文档 / 发到飞书"才走飞书技能

## 环境说明

首次安装 MCP 部分会自动下载 python-pptx 依赖（需联网）。
