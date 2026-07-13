# pinvou3 项目规则

## 核心约束

### 0. 本地私人记忆

- 如果仓库根目录存在 `.codex-memory.md`，开始工作前先读取；该文件是本地私人记忆，不提交。

### 1. DeepSeek-TUI 是底座，不重复造轮子

DeepSeek-TUI 已有：Engine / ToolRegistry / 流式 SSE / Session / SkillRegistry / Commands 路由 / MCP client / Hooks / Cycle / Compaction。

**绝不在 pinvou3 重新实现这些**。扩展时按场景选：

| 想做的事 | 用哪个 |
|---|---|
| 加领域 agent / 工具组合 | SKILL.md（复用 SkillRegistry） |
| 接外部 API | 写独立 MCP server |
| 改 LLM 行为引导 | `instructions.md`（见下「主体」bundle 布局） |
| Tauri UI / Rust wrapper / Engine 配置 | pinvou3-app 内 Rust |
| 修上游 bug | DeepSeek-TUI fork(详见 `docs/fork-policy.md`,软上限 1500 行) + 视情况 PR |

> **fork 维护**：策略/sync 流程/PR 状态见 [`docs/fork-policy.md`](docs/fork-policy.md)；fork 现状清单(C1–C7)+ sync 后验证 checklist 见 [`docs/fork-modifications.md`](docs/fork-modifications.md)。**基线/drift 以 fork-policy §0 为单一真相源**(软上限 1500 行,原"≤50 行"已修订)；守护手段 = fork-guard.sh 指纹 + forkguard_ 测试 + dump_system_prompt 前后 diff。
> **fork patch 指纹随 patch 同 PR**：新增/改 fork patch 的 PR 必须**同 PR**带上 fork-guard.sh 指纹 + 更新 fork-modifications.md——指纹随 patch 走,**不拆事后 catch-up PR**(出现 catch-up PR = 原始 PR 漏了指纹)。提 PR 前跑 `./scripts/fork-guard.sh --fast` 自查。**main CI `fast-gate` 已 enforce 此约束**：缺指纹 / 改 gitlink 没登记 fork-modifications / 落后 main 都挡合(`.github/workflows/pr-check.yml`)。
> **fork 改动是否要 PR**：通用优化 / bug 修复才提；pinvou3 专用留 fork。详见 `docs/fork-policy.md` §2 决策树。
> **底座上游PR规范**：https://github.com/Hmbown/CodeWhale/blob/main/CONTRIBUTING.md

### 2. 只用本地算力（GB10 + Qwen3.6-35B-A3B-FP8）

- 设计以当前模型能力为基线，未来变强是 bonus

### 3. 提交日志

- git log 提交日志标题需要注明类型，例如 `fix` / `feat` / `doc`。

### 4. 合并冲突保全

- merge、rebase、cherry-pick、跨仓迁移或手工移植时，必须把当前仓库已有功能和用户改动视为受保护基线。
- 能够共存的双方功能必须合并保留，不得为了消除文本冲突而整文件选择一侧或用来源分支覆盖本地行为。
- 只有不改变行为的机械性冲突、明确重复或可证明等价的实现可以独立处理。
- 遇到互斥方案、产品行为、安全/兼容性取舍或无法证明等价的实现时，必须保持未决，向用户说明选项和影响并等待明确决策，不得猜测。

## 主体

- `pinvou3-app/` — 🟢 Tauri 2.0 + EngineHandle wrapper（主线）
- `DeepSeek-TUI/` — submodule（h3c-hexin/DeepSeek-TUI fork），改动遵循约束 1
- 运行时数据在 `~/.pinvou3/`（sessions / settings.json / bundle / knowledge）
- 扩展物（instructions.md / skills / mcp-servers / personas）源码在 `pinvou3-app/.../resources/bundle/`，**编译进 app**，启动释放到 `~/.pinvou3/bundle/`

启动：`./pinvou3-app/run-dev.sh`

## 参考文档

- `CONTRIBUTING.md` — 贡献 / PR 流程 + CI 门控(人类协作者入口)
- `docs/验证报告-qwen3.6-deepseek-tui.md` — 阶段 A 实证报告
- git log + commit message — 决策记录与已知坑修复
- `docs/Git Commit 信息规范文档.md` — **强制** Git commit 信息规范。任何方式发起的 commit 都必须符合该规范；本仓库通过 `.githooks/commit-msg` 与 CI 校验执行。

<!-- SPECKIT START -->
如需了解当前 Spec Kit feature 使用的技术、项目结构、shell 命令和其他重要信息，请阅读
`specs/024-llmapi-hub-integration/plan.md`.
<!-- SPECKIT END -->
