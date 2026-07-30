# Pinvou Agent 贡献指南

[English](CONTRIBUTING.md) | [简体中文](CONTRIBUTING.zh-CN.md)

感谢你帮助改进 Pinvou Agent。我们欢迎问题修复、文档、连接器、Skills、工作流、平台支持和边界清晰的产品改进。

本文只说明贡献流程；项目级实现与质量边界以 [AGENTS.md](AGENTS.md) 为准。

## 开始之前

1. 先搜索现有 Issue 和 PR，避免重复建设。
2. 大型功能、架构调整和破坏性变更应先通过 Issue 讨论。
3. 按 [README.zh-CN.md](README.zh-CN.md) 准备开发环境。
4. 从官方仓库最新 `main` 开始开发。

维护者可从 `origin/main` 创建分支。外部贡献者应先配置一次官方仓库，并从 `upstream/main` 创建分支：

```bash
git remote add upstream https://github.com/Pinvou/pinvou-agent.git
git fetch upstream
git switch -c feat/short-description upstream/main
git submodule update --init --recursive
```

创建 PR 前和准备合并前同步官方最新 `main`；评审期间不要仅因 `main` 日常更新而反复 rebase。

## DCO

每个人工提交都必须包含有效的 `Signed-off-by`：

```bash
git commit -s
```

修改或 rebase 已有提交时使用 `--signoff`。详情见 [DCO.md](DCO.md)。未签署的人工提交会被 CI 拦截；受信任的 Dependabot 和 GitHub Actions bot 提交除外。

## 改动应放在哪里

Pinvou Agent 使用 [CodeWhale](https://github.com/Pinvou/CodeWhale) 作为 Agent 底座，不在桌面层重复实现底座能力。

| 目标 | 位置 |
|---|---|
| 增加领域 Agent 或工具组合 | `SKILL.md` 包 |
| 连接外部 API | 独立 MCP server 或 connector |
| 调整模型行为引导 | bundle `instructions.md` |
| 修改桌面 UI、Tauri 集成或运行时配置 | `pinvou3-app/` |
| 修复可复用的底座问题 | CodeWhale，上游优先 |

CodeWhale 改动必须遵循 [AGENTS.md](AGENTS.md) 和 [`docs/fork-policy.md`](docs/fork-policy.md) 的 fork 边界，包括同 PR 配套的文档、指纹和测试要求。

## Commit 信息

使用以下格式：

```text
<type>: <中文描述>
<type>(<scope>)!: <中文描述>
```

`scope` 和 `!` 可省略。允许的类型为 `feat`、`fix`、`refactor`、`perf`、`docs`、`style`、`test`、`build`、`ci`、`chore` 和 `revert`。描述必须包含中文、内容明确、不超过 50 字且结尾不加标点。

Issue 和 PR 可使用中文或英文。完整提交规则见 [`docs/Git Commit 信息规范文档.md`](docs/Git%20Commit%20信息规范文档.md)。

## 本地检查

按实际影响范围执行检查，常用基线为：

```bash
./scripts/fork-guard.sh --fast
python3 scripts/architecture-guard.py
npm --prefix pinvou3-app run lint:ui
npm --prefix pinvou3-app run build:ui
npm --prefix pinvou3-app test
cargo fmt --manifest-path pinvou3-app/src-tauri/Cargo.toml -- --check
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib -- --test-threads=1
```

涉及前端、Relay、Rust、CodeWhale 或特定平台时执行相应补充检查。[`.github/workflows/`](.github/workflows/) 是自动化门禁的单一真相源；无法在本地执行的检查必须如实说明。

依赖真实模型、网络服务、凭据或大型模型资源的测试必须默认忽略，并提供显式启用命令。

## CI 与合并队列

PR 使用按路径选择的快速门禁。发布链路改动只跑轻量契约测试；完整 deb、dmg、nsis 安装包仅在 `VERSION` 改动进入 `main` 后，或人工明确触发 `workflow_dispatch` 时构建。

完整 Rust 测试在 Merge Queue 中基于最新 `main` 执行。高风险 Rust PR 如需提前验证，可添加 `ci:full-rust` 标签。评审阶段只查看 required checks：

```bash
gh pr checks <编号> --required
```

不要等待非 required 的合入后平台构建或发布构建；评审期间也不要因 `main` 更新反复 rebase，只在准备进入 Merge Queue 时同步一次最新主线。

## Pull Request

提交前检查目标分支的实际差异，并完成 [AGENTS.md](AGENTS.md) 要求的质量自检。

PR 应说明：

- 改了什么以及为什么；
- 受影响的功能、平台、兼容性和已知风险；
- 实际执行的测试；
- 未验证场景或环境限制。

改动应保持聚焦，行为变化应同时更新文档和回归测试。合并前基于官方最新 `main` 解决冲突。本项目使用 CI 门控 PR，并默认 Squash Merge。

参与社区协作即表示同意遵守 [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)。支持范围见 [SUPPORT.md](SUPPORT.md)。

## 安全

禁止提交凭据、Token、密码、客户或私人数据及仅限内部使用的地址。未修复的安全漏洞应按 [SECURITY.md](SECURITY.md) 或通过 `security@pinvou.com` 私密报告。
