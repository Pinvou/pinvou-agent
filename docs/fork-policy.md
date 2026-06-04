# pinvou3 对 DeepSeek-TUI 底座的 fork 维护策略

> 创建：2026-05-28
> 适用：每次新增 fork patch + 每次跟进上游 sync
> 配套工具：`scripts/fork-guard.sh`、`pinvou3-app/src-tauri/src/bin/dump_system_prompt.rs`、`docs/fork-modifications.md`

## 0. 现状

- DeepSeek-TUI 是 `h3c-hexin/DeepSeek-TUI` fork（submodule，`pinvou3-patches` 分支）
- 当前 fork drift 约 **+1844 / -333 行,41 文件**（**v0.8.53 基线,2026-06-04 sync 后**;含 prompt override 已移 app 层不计入此数）。v0.8.51 时 +1811,本次 +33 来自 constitution.json loader 短路新 patch。v0.8.49 时 +1796/40 文件。上一基线 v0.8.47 时为 +2127
- 已超出原 CLAUDE.md "≤50 行" 约定 — **本文件正式修订该约束**
- 接受"重 fork"路线，靠工程化（指纹 + 测试 + 文档）控制维护成本

## 1. 总则

### 1.1 软上限

- fork drift **不超过 1500 行**。超过强制评估"撤回 / 提上游 PR"
- 单个文件被 fork 改 **不超过 200 行**。超过强制评估"是否能换层解决"

### 1.2 优先顺序（修 bug / 加功能时）

```
1. 应用层（pinvou3-app/）解决     ─┐ 优先
2. SKILL.md / instructions.md     │
3. 通用 bug/API 缺口 → 上游 PR    │
─────────────────────────────────  上游 PR 路径
4. pinvou3 私有 fork patch          ↓ fork 路径
5. 删上游主推功能(慎)
```

## 2. 新增 fork patch 决策树

新增改动前，按顺序自问：

| # | 问题 | 是 | 否 |
|---|---|---|---|
| Q1 | 能在 `pinvou3-app/` 应用层（bridge / EngineConfig 字段 / instructions.md 反指引）解决吗？ | **走应用层** | ↓ Q2 |
| Q2 | 是底座通用 bug 或 API 缺口？任何用底座的人都受益？ | **写好后单独提上游 PR，不进 fork** | ↓ Q3 |
| Q3 | 是 pinvou3 GUI 场景特有（Qwen3.6 / Tauri / 多 session / GB10）？ | **走 fork patch**（按 §3 配套） | ↓ Q4 |
| Q4 | 是删上游主推功能（删段 / 改 API 语义 / 不向后兼容）？ | **慎重**：先评估能否用 instructions.md `<system-reminder>` 反指引代替 | — |

## 3. 每个 fork patch 配套清单

新增 fork patch 后，**5 项必做**（缺一不可，否则上游 sync 时容易静默丢失）：

| # | 必做项 | 工具 |
|---|---|---|
| 1 | `docs/fork-modifications.md` 加 entry（# 编号 + 文件:行 + 改动 + 理由 + 上游 PR 可行性） | 手动 |
| 2 | `scripts/fork-guard.sh` 加指纹（grep 固定字串抓静默丢失） | 手动 |
| 3 | 写 `forkguard_*` 测试（断言 fork 后行为，反向防回归） | `cargo test forkguard_` |
| 4 | 上游原回归测试因这次 fork 必然 fail → 加 `#[ignore = "pinvou3 fork patch #X: ..."]` | 手动 |
| 5 | 跑 `bash scripts/fork-guard.sh --fast` 确认全过 | 脚本 |

### 测试命名规范

- 新加防回归测试 → `forkguard_<assertion>`（前缀让 fork-guard.sh 自动 cargo test）
- 上游原测试因 fork 失效 → `#[ignore = "pinvou3 fork patch #X: <一句解释>"]`

## 4. 上游 sync 流程

> 触发：上游 DeepSeek-TUI 有新版本要 pull 进来
> 估时：30-60 分钟（取决于 conflict 数量）

### 4.1 sync 前

```bash
# 1. 记录 sync 前 fork drift 行数
git -C DeepSeek-TUI diff <upstream-base>..HEAD --stat | tail -1

# 2. dump 当前 prompt 作 baseline
cargo run --manifest-path pinvou3-app/src-tauri/Cargo.toml \
  --bin dump_system_prompt 2>/dev/null > /tmp/pre-sync-prompt.txt

# 3. 跑 fork-guard 确认起点 clean
bash scripts/fork-guard.sh
```

### 4.2 sync 过程

```bash
cd DeepSeek-TUI
git fetch upstream
git checkout pinvou3-patches
git merge upstream/main  # 或 git rebase,看团队习惯
```

**Conflict 处理优先级**：

1. **核心文件 5 个**（必须最先 review）：
   - `crates/tui/src/prompts/base.md`
   - `crates/tui/src/prompts.rs`
   - `crates/tui/src/project_context.rs`
   - `crates/tui/src/skills/mod.rs`
   - `crates/tui/src/core/engine.rs`
2. 其它 fork 触及文件按 `docs/fork-modifications.md` 列表逐个对
3. Conflict 时**保留 pinvou3 行为**（fork-modifications.md 记的就是不要回退的内容）
4. 上游新加内容 review：是否对 pinvou3 有用？无用就丢、有用就保留

### 4.3 sync 后验证

```bash
# 1. 跑 fork-guard 全套（指纹 + 测试）
bash scripts/fork-guard.sh

# 2. 跑底座所有测试,看新加测试是否需要 #[ignore]
cd DeepSeek-TUI && cargo test -p codewhale-tui --lib 2>&1 | grep FAILED

# 3. 跑 pinvou3-tauri 测试
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib 2>&1 | grep FAILED

# 4. dump 新 prompt 跟 baseline 对比
cargo run --manifest-path pinvou3-app/src-tauri/Cargo.toml \
  --bin dump_system_prompt 2>/dev/null > /tmp/post-sync-prompt.txt
diff /tmp/pre-sync-prompt.txt /tmp/post-sync-prompt.txt
```

**期望的 diff**：
- 上游新加段落（如新 Statute / Article）— 评估是否对 pinvou3 有意义
- 工具描述微调（如 web_search 字段加 / 删）— OK
- pinvou3 私有改动应**字节稳定**（如果变了，说明 sync conflict 处理出错）

### 4.4 收尾

```bash
# 1. 更新 fork-modifications.md 顶部"v0.8.X 同步后整理"章节
#    - 列出本次 sync 上游变化对 pinvou3 的影响
#    - 标注哪些 fork patch 被上游 harvest（可作废）
#    - 标注新增的暴露点

# 2. 更新本文件 §0 "当前 fork drift" 行数

# 3. 提交 submodule commit + 主 repo submodule ref update
```

## 5. 撤回评估时机

满足**任一**触发条件，停下来审视是否能撤掉某些 fork patch：

| 触发条件 | 评估方向 |
|---|---|
| fork drift > 1500 行 | 哪些 patch 上游已 harvest 可作废？哪些可以换 instructions.md 反指引？ |
| 单次 sync conflict > 10 处 | 哪些 patch 触及的核心文件可以避开？ |
| 任一 `forkguard_*` 测试 fail 但原因不明 | 看 fork-modifications.md 对应 entry,确认改动是否还必要 |
| 上游加新 API 跟 pinvou3 fork 重叠 | 撤回 pinvou3 私有版本,改用上游 API |

## 6. fork patch 编号规则

- **全局递增**：`#1, #2, ..., #41` 用 fork-modifications.md 内顺序
- **新加 patch**：取 fork-modifications.md 当前最大编号 +1
- **删除 patch**：编号**不复用**，在 fork-modifications.md 标 "❌ 已删除（上游 harvest）"

## 7. 上游 PR 提交流程

提通用 fork patch 上游 PR 时：

1. 在 fork 主分支基础上 cherry-pick 出独立 commit（每个 PR 一个 commit）
2. Commit message 用英文，引用上游 issue # 如果有
3. PR title 用 conventional commit 格式（`feat:` / `fix:` / `docs:`）
4. PR body 引用 pinvou3 实测 case + before/after dump 对比
5. Accept 后等下一次上游 sync 自动 harvest（不要手动从 fork 删除 — 等 sync 自然走掉）

参考：https://github.com/Hmbown/CodeWhale/blob/main/CONTRIBUTING.md（如有）

## 8. 上游 PR 状态（实时，2026-05-31 核对）

> 全部 PR 提到 `Hmbown/CodeWhale`，head 走 `h3c-hexin/DeepSeek-TUI` 跨 fork。状态用 `gh pr list --repo Hmbown/CodeWhale --author h3c-hexin --state all` 核。
>
> **2026-05-31 大批合入**：owner 一次性 merge 了 #2245/#2311/#2313/#2314/#2354/#2355。**OPEN 仅剩 #2356**。这批合入的 fork patch 下次 sync 会被上游 harvest，届时按文件级 diff 确认 fork 版消失（漂移归零）。

**✅ 已 MERGED（下次 sync 随上游归零，别重复提）**

| PR | 内容 | merge |
|---|---|---|
| #1511 | exec reasoning_effort | 05-12 |
| #1686 | OpenAI streaming batch tool_calls 累积 | 05-15 |
| #2057 | subagent completion role system→user | 05-25 |
| #2060 | 256K 自托管窗口 auto-compact 生效 | 05-25 |
| #2146 | grep_files spawn_blocking + 30s timeout | 05-26 |
| #2147 | max_output_tokens env override | 05-26 |
| #2245 | web_search bing /ck/a HTML 实体解码 | 05-31 |
| #2311 | InstructionSource enum (File/Inline) | 05-31 |
| #2313 | Tier 5 覆盖 EngineConfig.instructions | 05-31 |
| #2314 | environment block 移 volatile 区 | 05-31 |
| **#2354** | subagent stop-on-failure + bounded-effort（doc #3） | 05-31 |
| **#2355** | fetch_url 可配置信任 fake-ip 段防 SSRF（doc #13） | 05-31 |

**🟡 OPEN（等上游 review/merge）**

- (无)

> **#2356** prompt override OnceLock hook:v0.8.49 上游**已自行实现并扩展**(10 个 `set_*_override`,签名 `Result<(), String>`)。我们 PR 本身或被采纳或被独立实现,无论哪种 fork 侧已取上游版,app 层 install_prompt_overrides 适配新签名。可关闭跟进。

**❌ CLOSED（不再跟进）**

| PR | 内容 | 处置 |
|---|---|---|
| #2312 | skills_dir union | 自己关的（diff 泄漏 pinvou3 字样）；功能仍是 pinvou3 必需活 patch，留 fork |
| #2044 / #1790 | file_search cancel/timeout | 功能已被上游 **#2035**(merge `d22da53e`) 用自家实现覆盖；fork 侧 file_search timeout patch 与上游重叠，**下次 sync 撤回留上游版** |
| #1480 | vLLM chat_template_kwargs | 留 fork |

> **提 PR 防泄漏铁律**（#2312 教训）：从 `upstream/main` 切干净分支，cherry-pick 后必跑 `git diff upstream/main <br> | grep -i 'pinvou\|qwen\|vllm\|gb10\|brother whale'` 自查；cherry-pick 常把源 commit 里的中文/品牌注释带进来，逐行剔除。

## 9. 相关文档

- `docs/fork-modifications.md` — 所有 fork patch 列表（按逻辑组分类）
- `docs/system-prompt-架构.md` — system prompt 全链路梳理
- `scripts/fork-guard.sh` — 指纹 + 回归测试守卫
- `pinvou3-app/src-tauri/src/bin/dump_system_prompt.rs` — prompt dump 工具（debug 必备）
