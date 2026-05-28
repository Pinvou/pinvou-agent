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
| 修上游 bug | DeepSeek-TUI fork(详见 `docs/fork-policy.md`,软上限 1500 行) + 视情况 PR |

> **fork 维护策略**：见 [`docs/fork-policy.md`](docs/fork-policy.md) —— 新增 patch 决策树、配套清单（指纹/测试/登记）、上游 sync 流程、撤回评估时机。当前 fork drift 已 ~990 行（2026-05-28 起），原"≤50 行"约束已修订为软上限 1500 行 + 工程化守护（fork-guard.sh + forkguard_ 测试）。
> **fork 改动是否要 PR**：通用优化 / bug 修复才提；pinvou3 专用留 fork。详见 `docs/fork-policy.md` §2 决策树。
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
