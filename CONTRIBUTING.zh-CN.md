# 贡献指南

[English](CONTRIBUTING.md) | 简体中文

> 面向 pinvou3 协作者。配套:`CLAUDE.md`(AI agent 规则 + 核心约束)、`docs/fork-policy.md`(fork 策略)、`docs/fork-modifications.md`(fork 现状清单)。

## DCO 签署

本项目使用 [Developer Certificate of Origin 1.1](https://developercertificate.org/)。
每个提交都必须包含与提交作者一致的 `Signed-off-by`：

```bash
git commit -s
```

详情见 [DCO.md](DCO.md)。PR 中任一提交缺少签署都会被 CI 拦截。

## 提 PR 前(自查,秒级)

1. **跑一遍 fork 守卫**(不编译,几秒):
   ```bash
   ./scripts/fork-guard.sh --fast
   ```
   CI 的 `fast-gate` 跑的就是它——本地先跑,别等 CI 红。
2. 确认创建 PR 时已基于最新 `main`，并解决全部冲突。

## main 受 CI 门控保护

每个 PR 按改动范围自动运行以下检查（`.github/workflows/pr-check.yml`），红的合不了：

| 检查 | 在哪 | 红了怎么办 |
|---|---|---|
| **fork-guard 指纹** | fast-gate | sync/merge 静默丢了 fork patch → 对照 `docs/fork-policy.md` 找回 |
| **fork 合规联动** | fast-gate | 改了 `CodeWhale` gitlink 没更新 `docs/fork-modifications.md` → 补登记 + 指纹(脚本报错有指引) |
| **Session replay auditor 单测** | fast-gate | session 筛选、工具配对或产物识别回归 → 本地跑 `python3 -m unittest discover -s scripts/tests -p 'test_*.py'` |
| **Python MCP 测试** | fast-gate | `mcp-servers/*/test_*.py` 挂 → 看 test 输出 |
| **内置 MCP 协议与产物契约** | fast-gate | 本地跑 `python3 scripts/mcp-server-contract-smoke.py`；检查 5 个本地 server 与 QCC 清单 |
| **前端逻辑 + Mock GUI + 完整 WebUI smoke** | frontend-test（按前端/Relay 路径触发） | 本地跑 `./scripts/run-user-journey-tests.sh`；WebUI 相关改动另跑 `npm --prefix pinvou3-app run test:webui` |
| **cargo test --lib** | rust-test | Rust 单测挂 → 本地 `cargo test --lib -- --test-threads=1` 复现(见下) |

> **新单测自动进 CI**:`cargo test --lib`(全跑 `src/` 下所有 `#[test]`)+ `mcp-servers/*/test_*.py`(glob)都是**通配**,新加的测试无需改 workflow 自动被跑。但有两条铁律,否则你的新测试会让 CI 红:
> 1. **依赖外部资源**(网络/真 bge-m3/vLLM/真模型)的测试**必须标 `#[ignore]`** —— CI 的 bge-m3 是空占位、无网络。参照现有 `e2e_test` / `l1_harness`。
> 2. 本地复现 CI 用 **`cargo test --lib -- --test-threads=1`** —— bridge 等测试读写全局 env,并行会竞争 flaky(CI 已锁单线程)。

main 使用 **Merge Queue**：PR 需要 CI 通过、获得 1 个 review approval 并解决全部对话，然后加入队列。队列会基于最新 `main` 运行完整检查并自动合入；评审和修复期间不要仅因 `main` 更新而反复 rebase，出现真实冲突时再处理。

> 注:**fmt / clippy 暂未进 gate**(现有代码各 329/75 处不符合,要先清理才能加 `-D` gate),后续再说。

## 改 fork(CodeWhale submodule)的铁律

fork patch 指纹**随 patch 同 PR**——新增 / 改 fork patch 必须**同一个 PR**带上:
- `docs/fork-modifications.md` 登记条目
- `scripts/fork-guard.sh` 指纹(+ L2 回归测试,如适用)

**不拆事后 catch-up PR**(出现 catch-up PR = 原始 PR 漏了指纹)。submodule gitlink 要焊到 fork 跟踪分支(`pinvou3-clean`)上、不是游离的 PR 分支 commit。细节见 `AGENTS.md` 约束 2 + `docs/fork-policy.md`。

## commit message（强制）

本项目强制遵守 [`docs/Git Commit 信息规范文档.md`](docs/Git%20Commit%20信息规范文档.md)。任何方式发起的 commit 都必须使用规范格式：

```text
<type>[可选作用域]: <中文简短描述>
```

允许的 `type` 仅包括：`feat` / `fix` / `refactor` / `perf` / `docs` / `style` / `test` / `build` / `ci` / `chore` / `revert`。

本仓库提供 `.githooks/commit-msg` 强制校验本地提交，并由 CI 校验 PR/push 提交信息。首次克隆后执行一次：

```bash
git config core.hooksPath .githooks
```

> 注：本地 `git commit --no-verify` 可绕过客户端 hook，但 PR/主分支 CI 仍会拦截不合规提交；需要绝对强制时，应配合远端分支保护把该 CI 设为 required check。

## (可选)本地 pre-push hook

想让 push 前自动跑 fork-guard、根本不让红 PR 出门,自己装一个(git hook 不随仓库走,每人本地装一次):
```bash
cat > .git/hooks/pre-push <<'EOF'
#!/usr/bin/env bash
./scripts/fork-guard.sh --fast || { echo "fork-guard 失败,push 取消"; exit 1; }
EOF
chmod +x .git/hooks/pre-push
```
