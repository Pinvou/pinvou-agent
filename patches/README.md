# Patches for h3c-hexin/DeepSeek-TUI fork

这个目录存放 pinvou3 v2 Pinvou Review 工作期间产出的 DeepSeek-TUI fork patch。
按 `CLAUDE.md` 约束「fork ≤50 行 + 立即 PR」处理:每个 patch 独立 PR 推到
h3c-hexin/DeepSeek-TUI,主 repo 等 fork merge 后 bump submodule pointer。

## v2 patch 清单

### `dtui-careful-yolo-block-dangerous.patch`

**目的**: pinvou Review v1 careful hook 的核心修复 —— 让 YOLO 模式下 `SafetyLevel::Dangerous`
命令也 BLOCKED(原 `if !context.auto_approve` 守卫让 YOLO 跳过了 Dangerous 检查,
等于把 careful 防护关了)。

**影响**: ~15 行,只改 `crates/tui/src/tools/shell.rs` 的 `analyze_command` 守卫逻辑。
现有 `analyze_command()` 已覆盖 `rm -rf /`、`git push --force`、`DROP TABLE`、`kubectl delete`
等所有破坏性 pattern,无需新增 pattern 检测。

**apply 方法**:
```bash
cd DeepSeek-TUI  # fork 仓库
git apply ../patches/dtui-careful-yolo-block-dangerous.patch
git commit -am "shell: 让 YOLO 模式下 Dangerous 命令也 BLOCKED (pinvou careful hook)"
gh pr create
```

**设计依据**: `docs/Pinvou-嘴替设计.md` §4.1
