# Pinvou 对 CodeWhale 底座的 fork 维护策略

> 最后更新：2026-09-09（上游 `v0.9.12` r1；受保护维护分支、不可变标签与父仓 gitlink 已发布并对齐）
> 配套：`docs/fork-modifications.md`、`scripts/fork-guard.sh`、`docs/底座升级验收清单.md`
> English: [`docs/fork-policy.en.md`](fork-policy.en.md)

## 0. 当前基线

- 上游：`Hmbown/CodeWhale` tag `v0.9.12`，commit `dcd4c200f72f0c1ffd60d8e7f6850313db879fc5`。
- 当前 fork 基线：`Pinvou/CodeWhale:pinvou3-clean` 与不可变 tag `pinvou-v0.9.12-r1`，head `1fafee7e26b60a59457a43bce50c63aa2ad9dbaf`，共 15 个带 DCO sign-off 的提交；由 CodeWhale PR #44 与 fast-follow PR #46 形成。
- 升级前公开回退点是不可变 tag `pinvou-v0.9.5-r13`，head `f853f8f1566c57e6be40d5439a222a932aa79ef5`；同 SHA 的本地 branch `backup/pre-v0.9.12-sync` 只作便利引用。
- r1 已成为可消费的受保护基线；父仓 gitlink、维护分支和不可变 tag 必须持续指向同一 commit。
- `.gitmodules` 不配置浮动 `branch`；发布后父仓 gitlink、维护分支和不可变标签必须指向同一 commit。
- 当前只维护 4 个长期主题：

  1. 宿主嵌入与路由边界
  2. 工具兼容与命令执行安全
  3. 嵌入上下文与技能来源
  4. 定时任务与运行生命周期

精确 commit、文件、理由和验证见 `docs/fork-modifications.md`。新增需求优先归入这 4 个主题；只有形成新的稳定状态、验证和回退边界时才增加主题。

## 1. 核心原则

### 1.1 最小 fork

CodeWhale 提供 Engine、工具循环、Session、Skills、Commands、MCP、Hooks、Compaction、Fleet 和 Automation。扩展按以下顺序落位：

1. `pinvou3-app` bridge / `EngineConfig` / Tauri wrapper
2. bundle `instructions.md` / `SKILL.md`
3. MCP server / connector / plugin
4. 通用缺口提交上游
5. 只有必须进入底座生命周期、且不能由以上层完成的 Pinvou 语义才留 fork

Pinvou 的产品工具白名单、UI、工作区选择和业务策略留在 app；底座只提供通用配置入口和执行期硬约束。

### 1.2 规模软上限

- 总 drift 软上限：净增 1500 行（净增 = 新增 − 删除行数，与下方基线表述同口径）。
- 单文件 fork-distinct 改动软上限：200 行。
- 超过不是自动拒绝，但必须记录保留原因和减量顺序。
- v0.9.12 r1 相对上游为 `94 files, +5022/-944`，净增 4078 行；相对 v0.9.5 r13 的 `110 files, +10895/-1195` 已收敛。新增触达文件包含当前 Rust/rustdoc 发布 lint 的等价调整、评审要求的生命周期与评测结果式回归、API 搜索后备链可达性与错误提示修复、无调用上游比较 helper 的删除、精确登记官方 v0.9.12 与 Pinvou r1 模型可见工具契约的预算收口、有界的 macOS 冷启动构建超时和 overdue one-shot 投递，不增加 fork 行为主题。基线仍超总量与个别文件软上限，因为可靠 steer、受限轮最终分发、宿主 prompt/profile/Skills 所有权、Automation 生命周期和对应安全回归必须在 Engine/Task 原子边界内实现。减量顺序是：先上游化通用 steer 与逐轮安全，再上游化 Automation 生命周期，父仓迁移到窄 re-export API 后分批收窄 18 个 `pub mod` 兼容 facade，最后评估 prompt/profile/Skills ownership 是否能由稳定 host API 完全替代。

### 1.3 主题提交

- 一个主题只包含共享状态、验证和回退边界的改动。
- 小修 fixup/squash 回所属主题，不维护 catch-up 提交串。
- 每次升级从上游 release tag 直接阅读 4 个线性主题，不复用冲突批次作为长期历史。

## 2. 新 fork patch 决策

| 判断 | 处理 |
|---|---|
| app bridge / EngineConfig / instructions 能解决 | 放 app 或 bundle |
| 独立外部能力 | MCP server / connector / plugin |
| 所有 CodeWhale embedder 都受益 | 从最新 upstream main 提上游 PR |
| Pinvou 私有且必须在 Engine、SubAgent、Task 生命周期中原子完成 | 并入最接近的既有主题 |
| 与 4 个主题都不共享状态、验证或回退边界 | 评审后才新增主题 |

## 3. 同 PR 配套要求

新增或修改 fork-distinct 行为时，同一父仓 PR 必须包含：

1. `docs/fork-modifications.md` 对应主题更新。
2. `scripts/fork-guard.sh` 固定指纹更新。
3. 至少一条结果式 `forkguard_*` 行为测试；纯平台行为说明替代验证。
4. 上游测试因产品语义不再成立时明确标注原因，不静默删除。
5. `./scripts/fork-guard.sh --fast` 通过。

只更新 gitlink 且行为不变时，仍需更新基线、commit 和指纹；现有行为测试已覆盖时不强制新增测试。

## 4. 上游同步流程

### 4.1 同步前

```bash
git -C CodeWhale fetch upstream --tags
git -C CodeWhale branch backup/pre-vX-sync <current-fork-head>
git -C CodeWhale diff --shortstat <current-release-tag>..<current-fork-head>
./scripts/fork-guard.sh --fast
```

先核对父仓、submodule 和 worktree 状态。备份只建 branch，不删除用户 worktree 或未跟踪文件。

### 4.2 选择 merge 或 clean re-fork

以下任一成立时优先 clean re-fork：

- 上游重构 Engine、SubAgent、Prompt、Automation 或 crate 边界。
- 预计冲突超过 10 处。
- 旧 drift 超过软上限。
- 多个旧 patch 已被上游吸收。

clean re-fork 从 release tag 新建隔离分支，逐主题重表达仍必要的语义；不得把旧 fork 整包 merge 后直接把冲突结果当作新基线。

### 4.3 逐项判定

每个旧 patch 归入：上游已有、迁到 app/Skill/MCP、仍需 fork。重点检查：

| 面 | 必查内容 |
|---|---|
| embed/route | library API、`EngineConfig` 新字段、resolved route、事件结构 |
| tools/safety | canonical catalog、allowed/disallowed、宿主工具、文件上限、命令安全 |
| prompt/skills | static composer、ambient context、Skill 根、disabled 语义、fragment 上限 |
| automation | schema、conversation key、misfire、no-overlap、终态清理 |

### 4.4 同步后 gate

```bash
./scripts/fork-guard.sh --fast

cargo check --manifest-path CodeWhale/Cargo.toml -p codewhale-tui --lib --locked
cargo test --manifest-path CodeWhale/Cargo.toml -p codewhale-tui --lib --locked \
  forkguard_ -- --test-threads=1
cargo test --manifest-path CodeWhale/Cargo.toml -p codewhale-tui --lib --locked \
  --features benchmark-eval-controls forkguard_benchmark_ -- --test-threads=1

cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml --locked
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml --all-targets \
  --features benchmark-hooks --locked
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib --locked \
  -- --test-threads=1
python3 scripts/architecture-guard.py
```

`fork-guard` 和自动测试不替代真实模型、GUI、MCP/OAuth 和定时任务端到端签收。

## 5. 上游贡献策略

- 从最新 upstream main 建净分支，一项通用语义一个 PR。
- 提交前扫描 `pinvou|qwen|vllm|gb10`，不得携带产品 fixture、私有注释或内部地址。
- 优先候选：通用 embedding route API、命令安全修复、Automation 生命周期修复。
- Pinvou 专用的提示词来源密封不直接推上游。

## 6. 发布边界

1. CodeWhale 先形成可按 4 个长期主题审阅的干净提交序列并完成底座测试；跨主题的生命周期安全收口可保留独立提交，但不得演变为未说明的 catch-up 串。
2. 父仓更新 gitlink、app 适配、`Cargo.lock`、fork 文档、guard 和升级报告。
3. 候选 review 分支可以为 PR 推送；只有获得明确授权后，才更新受保护维护分支 `pinvou3-clean` 并创建固定标签。发布前不得放宽“公开可达”验证来伪造完成。
4. 发布后复核远端 commit、不可变标签和父仓 gitlink 三者一致。
5. 清理临时 worktree/branch 是独立动作，不与升级默认捆绑。
