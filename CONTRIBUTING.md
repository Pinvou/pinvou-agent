# 贡献指南

> 面向 pinvou3 协作者。配套:`CLAUDE.md`(AI agent 规则 + 核心约束)、`docs/fork-policy.md`(fork 策略)、`docs/fork-modifications.md`(fork 现状清单)。

## 提 PR 前(自查,秒级)

1. **跑一遍 fork 守卫**(不编译,几秒):
   ```bash
   ./scripts/fork-guard.sh --fast
   ```
   CI 的 `fast-gate` 跑的就是它——本地先跑,别等 CI 红。
2. 确认 **PR 不落后 main**(落后会被挡合,见下)。

## main 受 CI 门控保护

每个 PR 自动跑 **`fast-gate`**(`.github/workflows/pr-check.yml`),红的合不了。两道检查:

| 检查 | 红的原因 | 怎么修 |
|---|---|---|
| **fork-guard 指纹** | sync / merge 静默丢了某个 fork patch | 对照 `docs/fork-policy.md` 找回被删的 patch |
| **fork 合规联动** | 改了 submodule(`DeepSeek-TUI`)的 gitlink,却没更新 `docs/fork-modifications.md` | 同 PR 补 fork-modifications 条目 + `scripts/fork-guard.sh` 指纹(脚本报错里有指引) |

另外 main 要求 **PR up-to-date**:别人先合了你就得 rebase——GitHub 会显示 "Update branch",点一下或本地 `git rebase origin/main`。合并还需 **1 个 review approval**。

> 注:**fmt 与编译类(clippy / cargo test)暂未进 gate**(fmt 需先全仓格式化、编译类需 cache + bge-m3 占位),后续再加。当前本地编译/测试仍需自行跑。

## 改 fork(DeepSeek-TUI submodule)的铁律

fork patch 指纹**随 patch 同 PR**——新增 / 改 fork patch 必须**同一个 PR**带上:
- `docs/fork-modifications.md` 登记条目
- `scripts/fork-guard.sh` 指纹(+ L2 回归测试,如适用)

**不拆事后 catch-up PR**(出现 catch-up PR = 原始 PR 漏了指纹)。submodule gitlink 要焊到 fork 跟踪分支(`pinvou3-clean`)上、不是游离的 PR 分支 commit。细节见 `CLAUDE.md` 约束 1 + `docs/fork-policy.md`。

## commit message

Conventional Commits:`feat:` / `fix:` / `chore:` / `docs:` / `test:` / `ci:`,**聚焦 why、1–2 句**,不罗列文件。

## (可选)本地 pre-push hook

想让 push 前自动跑 fork-guard、根本不让红 PR 出门,自己装一个(git hook 不随仓库走,每人本地装一次):
```bash
cat > .git/hooks/pre-push <<'EOF'
#!/usr/bin/env bash
./scripts/fork-guard.sh --fast || { echo "fork-guard 失败,push 取消"; exit 1; }
EOF
chmod +x .git/hooks/pre-push
```
