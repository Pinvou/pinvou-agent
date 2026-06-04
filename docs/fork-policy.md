# pinvou3 对 DeepSeek-TUI 底座的 fork 维护策略

> 创建：2026-05-28
> 适用：每次新增 fork patch + 每次跟进上游 sync
> 配套工具：`scripts/fork-guard.sh`、`pinvou3-app/src-tauri/src/bin/dump_system_prompt.rs`、`docs/fork-modifications.md`

## 0. 现状

- DeepSeek-TUI 是 `h3c-hexin/DeepSeek-TUI` fork（submodule，**`pinvou3-clean` 分支**,`.gitmodules` 追踪;旧 `pinvou3-patches` 留 fork 上当备份)
- 当前 fork drift 约 **+1258 / -315 行,34 文件**（**clean re-fork 后,2026-06-04;`pinvou3-clean` ← v0.8.53,6 主题 commit**;含 prompt override 已移 app 层不计入此数)。clean re-fork 砍掉 subagent 全套(不可达)/ phase-demo workflow(跨仓)/ qwen-128K 死码 / fetch_url 残留测试 + 撤除已 harvest 指纹,drift 从 +1844 降到 +1258。**fork 结构 = 6 主题 commit,详见 `docs/fork-modifications.md` §1**
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
| 1 | `docs/fork-modifications.md` §1 对应主题 commit 小节补 entry（文件 + 改动 + 理由 + 上游 PR 可行性） | 手动 |
| 2 | `scripts/fork-guard.sh` 加指纹（grep 固定字串抓静默丢失） | 手动 |
| 3 | 写 `forkguard_*` 测试（断言 fork 后行为，反向防回归） | `cargo test forkguard_` |
| 4 | 上游原回归测试因这次 fork 必然 fail → 加 `#[ignore = "pinvou3 fork(<主题>): ..."]` | 手动 |
| 5 | 跑 `bash scripts/fork-guard.sh --fast` 确认全过 | 脚本 |

### 测试命名规范

- 新加防回归测试 → `forkguard_<assertion>`（前缀让 fork-guard.sh 自动 cargo test）
- 上游原测试因 fork 失效 → `#[ignore = "pinvou3 fork(<主题/原因>): <一句解释>"]`

## 4. 上游 sync 流程

> 触发：上游 DeepSeek-TUI 有新版本要 pull 进来
> 估时：30-60 分钟（取决于 conflict 数量）
>
> 💡 **团队成员同步别人已合好的改动**(非自己做 upstream sync):用 Claude Code 项目命令 **`/sync-fork`**(`.claude/commands/sync-fork.md`)—— 自动把 submodule 同步到父仓钉的 commit + 追踪分支、处理脏树/孤儿分支、重编 + 跑 fork-guard。脏工作树/分歧会先停下问。

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
# remote: origin = Hmbown(上游),fork = h3c-hexin(我们)
git fetch origin --tags
git checkout pinvou3-clean
# ⚠️ release 是独立 tag(v0.8.5x),不在 origin/main 上(main 常停在上个版本+CI),
#    要对 release tag 合,别合 origin/main
git merge v0.8.XX
```

**Conflict 处理优先级**：

1. **核心 fork 文件**（必须最先 review,对应 fork-modifications §1 的 6 commit）：
   - `crates/tui/src/project_context.rs`（C5:砍空 + constitution 短路）
   - `crates/tui/src/skills/mod.rs` + `prompts.rs`（C5:skills union / 路径砍 / embedder-agnostic）
   - `crates/tui/src/tools/pinvou3_blocklist.rs` + `core/engine/tool_catalog.rs`（C2:工具门控)
   - `crates/tui/src/tools/file.rs` + `core/engine/{dispatch,turn_loop}.rs`（C3:append_file/大产物)
   - `crates/tui/src/command_safety.rs` + `tools/shell.rs`（C4:careful)
   - `crates/tui/src/lib.rs`（C1:上游加/删模块要手动同步 `pub mod`)
2. 其它 fork 触及文件按 `docs/fork-modifications.md` §1 逐个对
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
# 1. fork-modifications.md §4「Sync 历史」加一条本次 sync 章节
#    - 上游变化对 pinvou3 的影响 / 被 harvest 可作废的 patch / 新暴露点
#    若有 patch 被 harvest:撤 fork-guard 指纹 + 更新 §1 commit 描述

# 2. 更新本文件 §0 "当前 fork drift" 行数

# 3. submodule 提交在 pinvou3-clean 分支 + push fork + 主 repo 更新 submodule ref
```

## 5. 撤回评估时机

满足**任一**触发条件，停下来审视是否能撤掉某些 fork patch：

| 触发条件 | 评估方向 |
|---|---|
| fork drift > 1500 行 | 哪些 patch 上游已 harvest 可作废？哪些可以换 instructions.md 反指引？ |
| 单次 sync conflict > 10 处 | 哪些 patch 触及的核心文件可以避开？ |
| 任一 `forkguard_*` 测试 fail 但原因不明 | 看 fork-modifications.md 对应 entry,确认改动是否还必要 |
| 上游加新 API 跟 pinvou3 fork 重叠 | 撤回 pinvou3 私有版本,改用上游 API |

## 6. fork patch 组织规则（clean re-fork 后）

- 不再用旧的 `#1..#42` 全局编号(clean re-fork 已废弃)。fork 按 **6 个主题 commit**(C1-C6)组织,见 `docs/fork-modifications.md` §1。
- **新加 patch**:归入对应主题 commit 的范畴(工具/prompt/safety/lib/…),在 fork-modifications §1 对应小节补文件 + 改动 + 上游 PR 可行性 + 测试。
- **删/harvest patch**:从 fork-guard 指纹撤除,在 §2「移除清单」记一条 + §1 对应小节更新。
- drift 涨太多或主题混杂时,可再做一次 clean re-fork(从最新 release 重建主题 commit)—— 本次(2026-06-04)即范例。

## 7. 上游 PR 提交流程

提通用 fork patch 上游 PR 时：

1. 在 fork 主分支基础上 cherry-pick 出独立 commit（每个 PR 一个 commit）
2. Commit message 用英文，引用上游 issue # 如果有
3. PR title 用 conventional commit 格式（`feat:` / `fix:` / `docs:`）
4. PR body 引用 pinvou3 实测 case + before/after dump 对比
5. Accept 后等下一次上游 sync 自动 harvest（不要手动从 fork 删除 — 等 sync 自然走掉）

参考：https://github.com/Hmbown/CodeWhale/blob/main/CONTRIBUTING.md（如有）

## 8. 上游 PR 状态（2026-06-04 clean re-fork 后核对）

> 全部 PR 提到 `Hmbown/CodeWhale`，head 走 `h3c-hexin/DeepSeek-TUI` 跨 fork。状态用 `gh pr list --repo Hmbown/CodeWhale --author h3c-hexin --state all` 核。
>
> **下方 MERGED 的 PR 已在 v0.8.53 + clean re-fork 全部确认 harvest 归零**(fork 侧取上游版,对应指纹已从 fork-guard 撤除)。

**🟡 OPEN(2026-06-04 提,clean re-fork 派生,等上游 review)**

| PR | 内容 | head 分支 |
|---|---|---|
| [#2736](https://github.com/Hmbown/CodeWhale/pull/2736) | `tool_agent_route` 硬编码 `deepseek-v4-flash` → 继承父 session model(非 DeepSeek 后端 404) | `fix/tool-agent-inherit-parent-model` |
| [#2737](https://github.com/Hmbown/CodeWhale/pull/2737) | skills `skills_dir` 被 workspace skills 用 `or_else` 遮蔽 → 改 union | `fix/skills-dir-union-not-shadowed` |

> 均从 `origin/main`(=v0.8.53)切净分支,leak-check(grep pinvou/qwen/vllm/中文)零泄漏,英文 commit/PR body。Accept 后下次 sync 随上游 harvest。
>
> **C3(64KB cap / truncated_args_hint)评估后不提**:与 pinvou3 专属的 `append_file` 工具深度耦合(cap 同时管 append_file、hint 文案引导 "build up with append_file"、`APPEND_FILE_MAX_CONTENT_BYTES`);上游无 append_file,去耦后引导无落点("多次 write_file" 会覆盖)。留 fork。

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

- `docs/fork-modifications.md` — fork patch 清单（§1 = 6 主题 commit 结构 / §2 移除清单 / §3 fork-guard / §4 sync 历史）
- `docs/system-prompt-架构.md` — system prompt 全链路梳理
- `scripts/fork-guard.sh` — 指纹 + 回归测试守卫
- `pinvou3-app/src-tauri/src/bin/dump_system_prompt.rs` — prompt dump 工具（debug 必备）
