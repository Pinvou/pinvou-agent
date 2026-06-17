# pinvou3 项目规则

## 核心约束

### 1. DeepSeek-TUI 是底座，不重复造轮子

DeepSeek-TUI 已有：Engine / ToolRegistry / 流式 SSE / Session / SkillRegistry / Commands 路由 / MCP client / Hooks / Cycle / Compaction。

**绝不在 pinvou3 重新实现这些**。扩展时按场景选：

| 想做的事 | 用哪个 |
|---|---|
| 加领域 agent / 工具组合 | SKILL.md（复用 SkillRegistry） |
| 加 `/xxx` 命令 | `~/.deepseek/commands/xxx.md` |
| 接外部 API | 写独立 MCP server |
| 改 LLM 行为引导 | `.deepseek/instructions.md` |
| Tauri UI / Rust wrapper / Engine 配置 | pinvou3-app 内 Rust |
| 修上游 bug | DeepSeek-TUI fork ≤50 行 + 视情况 PR（见下） |

> **fork 改动是否要 PR**：不是所有 fork 改动都提 PR。**通用优化 / 通用 bug 修复**（任何用上游的人都受益）才建议提上游 PR（参考 #1511）；**pinvou3 专用**的改动（只为本项目场景、依赖 pinvou3 约定/配置）留在 fork 内，不提 PR。
> **底座上游PR规范**：https://github.com/Hmbown/CodeWhale/blob/main/CONTRIBUTING.md

### 2. 只用本地算力（GB10 + Qwen3.6-35B-A3B-FP8）

- 设计以当前模型能力为基线，未来变强是 bonus

## 主体

- `pinvou3-app/` — 🟢 Tauri 2.0 + EngineHandle wrapper（主线）
- `DeepSeek-TUI/` — submodule（h3c-hexin/DeepSeek-TUI fork），改动遵循约束 1

启动：`./pinvou3-app/run-dev.sh`

## 参考文档

- `docs/验证报告-qwen3.6-deepseek-tui.md` — 阶段 A 实证报告
- `process.md` — 跨阶段待办 / 长期搁置项
- git log + commit message — 决策记录与已知坑修复

<!-- SPECKIT START -->
如需了解当前 Spec Kit feature 使用的技术、项目结构、shell 命令和其他重要信息，请阅读
`specs/008-update-test-docs/plan.md`.
<!-- SPECKIT END -->
