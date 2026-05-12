# pinvou3 项目规则

## 两个核心约束

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
| 修上游 bug | DeepSeek-TUI fork ≤50 行 + 立即 PR（参考 #1511） |

### 2. 只用本地算力（GB10 + Qwen3.6-35B-A3B-FP8）

- 设计以当前模型能力为基线，未来变强是 bonus
- 内网受限：CDN 大概率不可达，所有前端资源必须 vendor 化 + 系统字体
- 部署形态：Tauri 单机桌面，不做 Web 双轨

## 主体

- `pinvou3-app/` — 🟢 Tauri 2.0 + EngineHandle wrapper（当前主线）
- `pinvou-platform/` — ⚠️ 已冻结（旧编排层，等 pinvou3-app 稳定后删）
- `DeepSeek-TUI/` — 库依赖，改动遵循约束 1

启动：`./pinvou3-app/run-dev.sh`

## 参考文档

- `docs/DeepSeek-TUI-架构详解.md` — 底座详尽解析
- `docs/验证报告-qwen3.6-deepseek-tui.md` — 阶段 A 实证报告
- git log + commit message — 决策记录与已知坑修复
