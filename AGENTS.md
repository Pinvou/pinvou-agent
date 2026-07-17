# pinvou3 项目规则

## 核心约束

### 0. 本地私人记忆

- 如果仓库根目录存在 `.codex-memory.md`，开始工作前先读取；该文件是本地私人记忆，不提交。

### 1. 开发与 PR 前同步最新主线

- 开始新的开发任务前，先安全同步最新 `origin/main` 和 submodule，不得丢失已有本地改动。
- 创建 PR 前及合并 PR 前，确保分支已同步最新 `origin/main` 并解决全部冲突。

### 2. DeepSeek-TUI 是底座，不重复造轮子

DeepSeek-TUI 已有：Engine / ToolRegistry / 流式 SSE / Session / SkillRegistry / Commands 路由 / MCP client / Hooks / Cycle / Compaction。

**绝不在 pinvou3 重新实现这些**。扩展时按场景选：

| 想做的事 | 用哪个 |
|---|---|
| 加领域 agent / 工具组合 | SKILL.md（复用 SkillRegistry） |
| 接外部 API | 写独立 MCP server |
| 改 LLM 行为引导 | `instructions.md`（见下「主体」bundle 布局） |
| Tauri UI / Rust wrapper / Engine 配置 | pinvou3-app 内 Rust |
| 修上游 bug | DeepSeek-TUI fork(详见 `docs/fork-policy.md`,软上限 1500 行) + 视情况 PR |

> **fork 维护**：策略/sync 流程见 [`docs/fork-policy.md`](docs/fork-policy.md)；fork 现状按 **6 个长期主题**维护，清单与 sync 后验证 checklist 见 [`docs/fork-modifications.md`](docs/fork-modifications.md)。**基线/drift 以 fork-policy §0 为单一真相源**(软上限 1500 行,超过必须记录强制评估)；守护手段 = fork-guard.sh 指纹 + forkguard_ 测试 + dump_system_prompt 前后 diff。
> **fork patch 指纹随 patch 同 PR**：新增/改 fork patch 的 PR 必须**同 PR**带上 fork-guard.sh 指纹 + 更新 fork-modifications.md——指纹随 patch 走,**不拆事后 catch-up PR**(出现 catch-up PR = 原始 PR 漏了指纹)。提 PR 前跑 `./scripts/fork-guard.sh --fast` 自查。**main CI `fast-gate` 已 enforce 此约束**：缺指纹 / 改 gitlink 没登记 fork-modifications / 落后 main 都挡合(`.github/workflows/pr-check.yml`)。
> **fork 改动是否要 PR**：通用优化 / bug 修复才提；pinvou3 专用留 fork。详见 `docs/fork-policy.md` §2 决策树。
> **底座上游PR规范**：https://github.com/Hmbown/CodeWhale/blob/main/CONTRIBUTING.md

### 3. 提交日志

- git log 提交日志统一使用中文，并在标题中注明类型前缀，例如 `fix:` / `feat:` / `docs:`；类型前缀后的描述使用中文。

### 4. GitHub PR 规范

- GitHub PR 的标题和正文统一使用中文（代码标识、命令、路径等保留原文）。
- PR 正文必须明确说明以下内容：
  - **改了什么**：概括本次修改的主要内容。
  - **改动原因**：说明问题背景、修改目的或采用该方案的原因。
  - **影响面**：列出受影响的功能、模块、平台、兼容性及潜在风险；没有影响也要明确说明。
- 提交 PR 前遵循 `CONTRIBUTING.md` 中的流程和 CI 门控要求。

## 主体

- `pinvou3-app/` — 🟢 Tauri 2.0 + EngineHandle wrapper（主线）
- `DeepSeek-TUI/` — submodule（h3c-hexin/DeepSeek-TUI fork），改动遵循约束 2
- 运行时数据在 `~/.pinvou3/`（sessions / settings.json / bundle / knowledge）
- 扩展物（instructions.md / skills / mcp-servers / personas）源码在 `pinvou3-app/.../resources/bundle/`，**编译进 app**，启动释放到 `~/.pinvou3/bundle/`

启动：`./pinvou3-app/run-dev.sh`
