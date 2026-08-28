# Pinvou Agent 贡献指南

[English](CONTRIBUTING.md) | [简体中文](CONTRIBUTING.zh-CN.md)

感谢你帮助改进 Pinvou Agent。我们欢迎问题修复、文档、连接器、Skills、工作流、平台支持和边界清晰的产品改进。

本文是 [CONTRIBUTING.md](CONTRIBUTING.md) 的中文参考版；两者内容冲突时以英文版为准。
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

创建 PR 前同步官方最新 `main`；评审期间不要仅因 `main`
日常更新而反复 rebase。PR Ready 后由 Merge Queue 在最新 `main` 组合
提交上验证；仅真实冲突，或队列集成失败需要修改分支时手工 rebase。

解决冲突时保留双方兼容的功能与用户改动；不得在没有向用户说明各选项
及其影响的情况下，在行为不同的方案之间擅自二选一。

## DCO

每个人工提交都必须包含有效的 `Signed-off-by`：

```bash
git commit -s
```

修改或 rebase 已有提交时使用 `--signoff`。详情见 [DCO.md](DCO.md)。未签署的人工提交会被 CI 拦截；受信任的 Dependabot 和 GitHub Actions bot 提交，以及合并提交（多于一个父提交）除外。

## 改动应放在哪里

Pinvou Agent 使用 [CodeWhale](https://github.com/Pinvou/CodeWhale) 作为 Agent 底座，不在桌面层重复实现底座能力。扩展落位边界表单点维护在 [AGENTS.md](AGENTS.md) 第 2 节；CodeWhale 改动必须遵循该边界和 [`docs/fork-policy.md`](docs/fork-policy.md)，包括同 PR 配套的文档、指纹和测试要求。

## Commit 信息

使用以下格式：

```text
<type>: <English description>
<type>(<scope>)!: <English description>
```

`scope` 和 `!` 可省略。允许的类型为 `feat`、`fix`、`refactor`、`perf`、`docs`、`style`、`test`、`build`、`ci`、`chore` 和 `revert`。使用不超过 50 个字符且结尾无标点的简洁英文描述。CI 只校验格式，不校验语言。

分支、Issue、PR、Commit、代码注释、开发文档和诊断信息使用英文。既有历史和本地化资源除外；UI 文案遵循 [AGENTS.md](AGENTS.md)。

完整提交规则见 [`docs/commit-message-convention.md`](docs/commit-message-convention.md)。

## 本地检查

可选：启用本地 commit-msg 钩子，在推送前提前拦截提交信息格式问题：

```bash
git config core.hooksPath .githooks
```

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

PR 使用分层、按路径选择的门禁。Draft 跑 lint、build、确定性逻辑测试和
适用的 Rust fmt 快检；Ready PR 追加由实际 diff 选定的浏览器 smoke、平台
Runtime 契约和其他受影响检查。Ready Rust PR 默认运行快速的格式、lint、
依赖策略和编译反馈；未知 Rust 路径默认进入完整回归，只有明确登记的孤立叶子
功能边界走轻量路径。依赖、CodeWhale gitlink、应用命令、平台、权限、凭据、
Session、远控及其他高爆炸半径路径会在入队前自动追加完整 Linux/Windows Rust 回归。
其他 Ready Rust PR 可用 `ci:full-rust` 强制追加同款全量回归；Draft 不响应该
标签。目前轻量 Rust 边界仅限 `feedback`、`personas`、`pet` 模块内部改动；它们的
注册、应用命令、共享平台接口、Cargo 元数据或任何未分类 Rust 路径仍进入完整回归。
发布链路改动只跑轻量契约测试；完整
deb、dmg、nsis 安装包仅在 `VERSION` 改动进入 `main` 后，或人工明确触发
`workflow_dispatch` 时构建。

Merge Queue 在入队 PR 与最新 `main` 的实际组合树上运行适用门禁：Rust 改动
运行格式、Clippy、编译和依赖策略检查；高爆炸半径 Rust 改动还会在组合树上执行
完整 Linux 行为回归，Windows 已在 Ready PR 验证，不在队列重复。前端改动按
merge group 的真实 base/head diff 选择浏览器 smoke，共享、未知或测试设施路径
fail-closed 回退全套。每个保留下来的 `main` push 都执行完整 Linux / Windows
Rust 回归并持续写暖缓存；main 回归变红后应停止继续入队，直至修复或回滚。
评审阶段只查看 required checks：

```bash
gh pr checks <编号> --required
```

main 绿色时不要等待非 required 的合入后平台构建或发布构建。无真实冲突的独立 Ready PR
可直接入队，由队列验证新鲜度。每个 merge group 最多放两个低风险、相互独立的
PR；依赖锁、CI、发布、权限、Session、CodeWhale gitlink 及其他高风险改动单独入队。

## Pull Request

提交前检查目标分支的实际差异，并完成 [AGENTS.md](AGENTS.md) 要求的质量自检。

PR 标题和正文使用英文；标题必须符合 Squash Merge 所需的 Commit 标题规范。

PR 应说明：

- 改了什么以及为什么；
- 受影响的功能、平台、兼容性和已知风险；
- 实际执行的测试；
- 未验证场景或环境限制。

改动应保持聚焦，行为变化应同时更新文档和回归测试。合并前基于官方最新 `main` 解决冲突。本项目使用 CI 门控 PR，并默认 Squash Merge。

参与社区协作即表示同意遵守 [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)。支持范围见 [SUPPORT.md](SUPPORT.md)。

## 安全

禁止提交凭据、Token、密码、客户或私人数据及仅限内部使用的地址。未修复的安全漏洞应按 [SECURITY.md](SECURITY.md) 或通过 `security@pinvou.com` 私密报告。
