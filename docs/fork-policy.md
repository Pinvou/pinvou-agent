# pinvou3 对 DeepSeek-TUI 底座的 fork 维护策略

> 最后更新：2026-07-17（上游基线 `v0.9.0@d167c07c9`）
> 配套：`docs/fork-modifications.md`、`scripts/fork-guard.sh`、`docs/底座升级验收清单.md`

## 0. 当前基线与组织方式

- 上游：`Hmbown/CodeWhale` tag `v0.9.0`，commit `d167c07c96282411956ea7f35ddb8227afa1402f`。
- clean re-fork 工作分支：`codex/sync-v0.9.0`，当前 head `4cff0b9e6e1d`。
- fork 不再按历史 C1–C12 / W1–W13 批量编号维护，当前只保留 **6 个长期主题**：

  1. 宿主 library facade
  2. 工具面、文件写入与执行安全
  3. 提示词密封与 context / skill 单一来源
  4. 定时任务执行与历史生命周期
  5. 宿主编排、工作流完成闸与可取消登录
  6. 宿主路由、预算与 shared automation 接口

- 精确文件、commit、理由与测试见 `docs/fork-modifications.md`。
- 新需求优先并入已有主题；只有形成新的稳定边界时才增加主题，禁止按 PR 批次、冲突批次或日期堆小 fork。

## 1. 核心原则

### 1.1 不重复造底座

Engine、ToolRegistry、SSE、Session、SkillRegistry、Commands、MCP client、Hooks、Compaction 等能力直接复用上游。扩展顺序：

1. `pinvou3-app` bridge / EngineConfig / Tauri wrapper
2. bundle `instructions.md` / `SKILL.md`
3. 独立 MCP server
4. 通用 bug/API 缺口提上游 PR
5. 只有 pinvou3 私有且必须进入底座生命周期的语义才留 fork

### 1.2 软上限

- 总 drift 软上限：1500 行。
- 单文件 fork-distinct 改动软上限：200 行。
- 超过不是自动拒绝，但必须在 `fork-modifications.md` 记录：为什么不能放 app/skill/MCP、哪些已 harvest、后续减量顺序。
- 当前 v0.9.0 re-fork 为 `+3260/-539，53 文件`，已经完成强制评估；结论见修改清单 §0。

### 1.3 主题提交

- 一个长期主题可以跨多个强耦合文件；以“必须一起验证和回退”为合并标准。
- 不按 cherry-pick 批次、冲突批次、旧 PR 编号拆 commit。
- 小修必须 fixup/squash 回所属主题，避免在主题后维护一串 catch-up commit。
- clean re-fork 后的线性主题历史应能从上游 tag 直接阅读。

## 2. 新增 fork patch 决策树

| 判断 | 处理 |
|---|---|
| app bridge / EngineConfig / instructions 能解决 | 放 app 或 bundle，不改 submodule |
| 独立外部能力 | 写 MCP server |
| 所有 CodeWhale embedder 都受益 | 从最新 upstream main 切净分支提上游 PR |
| pinvou3 私有，且必须在 engine/subagent/task 生命周期内原子完成 | 放入最接近的现有 fork 主题 |
| 与任何现有主题都不共享状态、验证或回退边界 | 才考虑新主题 |

## 3. 同 PR 配套要求

任何新增或修改 fork-distinct 代码的父仓 PR 必须同时包含：

1. `docs/fork-modifications.md` 对应主题更新。
2. `scripts/fork-guard.sh` 固定字符串指纹。
3. 至少一条 `forkguard_*` 结果式行为测试；纯平台行为需说明替代验证。
4. 上游原测试因产品语义必然不成立时，明确 `#[ignore = "pinvou3 fork(...)"]`，不能静默删测试。
5. `./scripts/fork-guard.sh --fast` 通过。

指纹、文档和 patch 必须同 PR；出现事后 catch-up PR 视为原 PR 漏项。

## 4. 上游 sync 流程

### 4.1 sync 前

```bash
git -C DeepSeek-TUI fetch upstream --tags
git -C DeepSeek-TUI branch backup/pre-vX-sync <current-fork-head>
git -C DeepSeek-TUI diff --shortstat <current-release-tag>..<current-fork-head>
./scripts/fork-guard.sh --fast

cargo run --manifest-path pinvou3-app/src-tauri/Cargo.toml \
  --bin dump_system_prompt > /tmp/pre-sync-prompt.txt
```

先核对工作树、worktree 和 submodule branch 占用；备份只建 branch，不删除 worktree、不 `git clean`。

### 4.2 选择 merge 还是 clean re-fork

满足任一条件，优先 clean re-fork：

- 跨大版本或上游重构了 engine/subagent/prompt/automation 主干。
- 预计冲突超过 10 处。
- 旧 fork drift 已超过软上限。
- 多个旧 patch 已被上游 harvest，merge 会保留大量无意义历史。

clean re-fork 做法：从 release tag 新建隔离分支，逐主题重表达仍必要的语义；不要先把旧 fork 整包 merge 再按冲突结果提交。

### 4.3 逐项判定

每个旧 patch 必须归入三类之一：

- **上游已有**：逐字段确认等价，撤 fork 代码、指纹和说明。
- **可移到 app/skill/MCP**：迁出底座并补应用层测试。
- **仍需 fork**：按当前主题重打，并立即补新基线测试/指纹。

最易静默漂移的面：

| 面 | 必查内容 |
|---|---|
| prompt/context | composer 调用点、静态/运行时分层、project context 扫描源、Working Set |
| tools/safety | active catalog 结果、deferred activator、写入上限、审批 fail-closed、Dangerous 命令 |
| subagent/workflow | assignment、tool surface、完成判定、结构化落盘、取消与 terminal event |
| automation/task | schema 兼容、conversation key、运行链接、锁跨 await、终态清理 |
| embed API | lib module、opaque route、EngineConfig 新字段、Event/Op 结构变化 |

### 4.4 sync 后硬 gate

```bash
# 指纹
./scripts/fork-guard.sh --fast

# submodule
cargo check -p codewhale-tui --lib
cargo test -p codewhale-tui forkguard_ --lib -- --test-threads=1
cargo test -p codewhale-tui automation_manager::tests --lib -- --test-threads=1

# app
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib --no-run

# 静态 prompt
cargo run --manifest-path pinvou3-app/src-tauri/Cargo.toml \
  --bin dump_system_prompt > /tmp/post-sync-prompt.txt
diff /tmp/pre-sync-prompt.txt /tmp/post-sync-prompt.txt
```

完整产品签收仍以 `docs/底座升级验收清单.md` 为准。`fork-guard` 通过不等于 L1 真模型、GUI、MCP、OAuth、定时任务端到端已签收。

## 5. 上游 PR 策略

- 从最新 upstream main 切净分支，一项通用语义一个 PR。
- 提交前扫描 `pinvou|qwen|vllm|gb10|brother whale`，禁止产品 fixture 和中文私有注释泄漏。
- PR 标题/body 按上游 `CONTRIBUTING.md`；父仓自己的 commit/PR 仍遵守中文规范。
- CLOSED 不等于未 harvest；每次 sync 都按代码语义复核。
- 当前优先候选：T6 opaque route/shared reconcile、T2 通用安全修复、T4 通用 automation 生命周期。

## 6. 收尾与发布边界

1. submodule 先形成干净的主题 commit，并完成底座测试。
2. 父仓同一 PR 更新 gitlink、app 适配、Cargo.lock、两份文档和 guard。
3. fork 远端分支与父仓 gitlink 必须指向同一个可达 commit。
4. 未明确授权时，不直接 force-push 共享 main；先在同步分支交付验证结果。
5. 合入后删除临时 worktree/branch 属单独清理动作，不与 sync 默认捆绑。
