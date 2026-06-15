# pinvou3 对 DeepSeek-TUI 底座的 fork 维护策略

> 创建 2026-05-28 · 最后更新 2026-06-15(v0.8.60 sync)
> 适用:每次新增 fork patch + 每次跟进上游 sync
> 配套:`scripts/fork-guard.sh`、`bin/dump_system_prompt.rs`、`docs/fork-modifications.md`(现状清单 + 验证 checklist)

## 0. 现状

- DeepSeek-TUI 是 `h3c-hexin/DeepSeek-TUI` fork(submodule,**`pinvou3-clean` 分支** ← upstream **v0.8.60**;HEAD `1161bc78` = v0.8.60 + 8 主题 commit(2026-06-15 第 2 次 clean re-fork);`.gitmodules` 追踪;备份 `backup/pre-reclean-v0.8.60`)
- fork drift **+2335 / −360 行,43 文件**(vs v0.8.60;主体是工作流层 W1–W12,**已超 1500 软上限**——撤回评估见 fork-modifications §4;app 层 prompt override 不计入)
- fork 结构 = **C1–C7 + W 逻辑主题**(W = 三省六部工作流层),详见 `docs/fork-modifications.md` §1
- 路线:接受"重 fork",靠工程化(指纹 + 测试 + dump diff + 文档)控制维护成本。当前 drift 2335 已超软上限,2026-06-15 评估=主体必需、保留(见 fork-modifications §4)

## 1. 总则

### 1.1 软上限
- fork drift **不超过 1500 行**。超过强制评估"撤回 / 提上游 PR"
- 单文件被 fork 改 **不超过 200 行**。超过强制评估"是否能换层解决"

### 1.2 优先顺序(修 bug / 加功能)
```
1. 应用层(pinvou3-app/)解决     ─┐ 优先
2. SKILL.md / instructions.md     │
3. 通用 bug/API 缺口 → 上游 PR    │  上游 PR 路径
─────────────────────────────────
4. pinvou3 私有 fork patch          ↓ fork 路径
5. 删上游主推功能(慎)
```

## 2. 新增 fork patch 决策树

| # | 问题 | 是 | 否 |
|---|---|---|---|
| Q1 | 能在 `pinvou3-app/` 应用层(bridge / EngineConfig 字段 / instructions.md 反指引 / composer override)解决? | **走应用层** | ↓ Q2 |
| Q2 | 是底座通用 bug 或 API 缺口?任何用底座的人都受益? | **写好后单独提上游 PR,不进 fork** | ↓ Q3 |
| Q3 | 是 pinvou3 GUI 场景特有(Qwen3.6 / Tauri / 多 session / GB10)? | **走 fork patch**(按 §3 配套) | ↓ Q4 |
| Q4 | 是删上游主推功能(删段 / 改 API 语义 / 不向后兼容)? | **慎重**:先评估能否用 instructions.md `<system-reminder>` 反指引代替 | — |

## 3. 每个 fork patch 配套清单(5 项必做,缺一易在 sync 时静默丢失)

| # | 必做项 | 工具 |
|---|---|---|
| 1 | `fork-modifications.md` §1 对应主题补 entry(文件 + 改动 + 理由 + 上游 PR 可行性) | 手动 |
| 2 | `fork-guard.sh` 加指纹(grep 固定字串抓静默丢失) | 手动 |
| 3 | 写 `forkguard_*` 测试(断言 fork 后行为,反向防回归) | `cargo test forkguard_` |
| 4 | 上游原回归测试因 fork 必然 fail → 加 `#[ignore = "pinvou3 fork(<主题>): ..."]` | 手动 |
| 5 | 跑 `bash scripts/fork-guard.sh --fast` 确认全过 | 脚本 |

**测试命名**:新加防回归 → `forkguard_<assertion>`(前缀让 fork-guard 自动 cargo test);上游原测试失效 → `#[ignore = "pinvou3 fork(<主题/原因>): <一句解释>"]`。

## 4. 上游 sync 流程(小 sync 30–60 min;大版本如 v0.8.57/v0.8.60 约半天)

> ⚠️ **验收硬 gate**:sync 完成的判据是 `docs/底座升级验收清单.md`(L0 编译 / L1 自动化测试 / L2 六大功能 / L3 回归专项 + 签收表)全过——本节是流程,验收单是 pass/fail。

### 4.1 sync 前
```bash
git -C DeepSeek-TUI branch -f backup/pre-vX-sync pinvou3-clean   # 安全备份,出错可 reset --hard 回退
git -C DeepSeek-TUI diff v0.8.XX..HEAD --stat | tail -1          # 记录 sync 前 drift
cargo run --manifest-path pinvou3-app/src-tauri/Cargo.toml \
  --bin dump_system_prompt 2>/dev/null > /tmp/pre-sync-prompt.txt # baseline(关键)
bash scripts/fork-guard.sh --fast                                # 起点指纹 clean
```

### 4.2 sync 过程
```bash
cd DeepSeek-TUI
git remote get-url upstream >/dev/null 2>&1 || git remote add upstream https://github.com/Hmbown/CodeWhale.git
git fetch upstream --tags      # upstream=Hmbown/CodeWhale(上游);origin=h3c-hexin(我们的 fork)
git checkout pinvou3-clean
git merge vX.Y.Z               # ⚠️ 合 release tag,不合 main(main 常停在上版本+CI)
```
**冲突处理优先级**(最先 review 这些核心 fork 文件,对应 fork-modifications §1):

| 文件 | 主题 | 看点 |
|---|---|---|
| `prompts.rs` | C5+C7 | **最易出血**:composer 机制 + ContextMgmt/COMPACT/Runtime-Policy gate;上游若动 prompt 合成结构要逐字段比对 |
| `core/engine/turn_loop.rs` | C7+C3 | per-turn 消息构造(`messages_with_turn_metadata` tag gate)+ append_file SSE 遥测 |
| `project_context.rs` | C5 | 砍空 + constitution 短路(上游 constitution 层在演进) |
| `skills/mod.rs` | C5 | #41 路径收窄(union 接线已 harvest,只守收窄) |
| `tools/pinvou3_blocklist.rs` + `tool_catalog.rs` | C2 | 工具门控 mode-aware;上游改 defer 签名要跟 |
| `tools/file.rs` + `dispatch.rs` | C3 | append_file / 64KB / truncated_args_hint |
| `tools/shell.rs` + `command_safety.rs` | C4 | YOLO 也拦 Dangerous(rm -rf / ~,fork bomb;C4-a 已撤) |
| `tools/subagent/mod.rs` 等 | W | 三省六部工作流层(SpawnSubAgent 五字段/结构化产出/max_steps);上游大改 subagent 时 union 不冲掉 W |
| `lib.rs` | C1 | **上游加/删模块必手动同步 `pub mod`**(删模块→删孤儿 `pub mod`) |

原则:**冲突时保留 pinvou3 行为**(fork-modifications 记的就是不要回退的内容);上游新内容评估对 pinvou3 是否有用,无用就丢。

### 4.3 sync 后验证(fork-guard **不够**,以下每条都踩过坑 —— 依据见 fork-modifications §3)
```bash
# 1. fork-guard 全套
bash scripts/fork-guard.sh
# 2. 全量 lib 测试(抓非 forkguard_ 前缀的上游测试 fail)
cd DeepSeek-TUI && cargo test -p codewhale-tui --lib 2>&1 | grep -E "FAILED|test result"
# 3. app 测试(单线程,bridge env 测试并行会 flake)
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib -- --test-threads=1 2>&1 | grep -E "FAILED|test result"
# 4. dump 前后 diff —— 期望字节稳定(非 0 就逐块查谁漏进静态 prompt)
cargo run --manifest-path pinvou3-app/src-tauri/Cargo.toml \
  --bin dump_system_prompt 2>/dev/null > /tmp/post-sync-prompt.txt
diff /tmp/pre-sync-prompt.txt /tmp/post-sync-prompt.txt
# 5. 扫 per-turn 注入(dump 抓不到)
grep -rn "messages.push\|runtime_prompt" DeepSeek-TUI/crates/tui/src/core/engine/turn_loop.rs DeepSeek-TUI/crates/tui/src/core/engine.rs
# 6. 工具集合盘点(blocklist 模型下上游新工具默认可见)
#    对比两版 ToolSpec::name() 字面量集合,确认无新工具漏入需补黑名单
#    更要查上游有没有新增能激活 deferred 工具的机制(tool_search/ensure_advanced_tooling 类)
# 7. hook 决策协议(v0.8.60 Hooks v2 教训):上游可能改 hook 退出码/JSON 契约
#    读 fold_tool_call_before_results 确认 deny 脚本(deny_sensitive_paths.sh)退出码契约
#    (v0.8.60 把 hard-deny 从「非零」改成只认 exit_code==2,旧 exit 1 静默失效)
```
**期望 diff**:pinvou3 私有改动**字节稳定**(变了=冲突处理出错);上游新静态段落要评估是否对 pinvou3 有意义、是否该 gate。

### 4.4 收尾
1. `fork-modifications.md` §4 加本次 sync 章节(上游变化影响 / 被 harvest 的 patch / 新暴露点);harvest 的 patch 撤 fork-guard 指纹 + 更新 §1。
2. 更新两文档 §0 drift / 指纹 / 工具数。
3. submodule 在 `pinvou3-clean` 提交 + push fork + 主 repo 更新 submodule ref。

## 5. 撤回评估时机(满足任一即停下审视)

| 触发条件 | 评估方向 |
|---|---|
| fork drift > 1500 行 | 哪些 patch 上游已 harvest 可作废?哪些可换 instructions.md 反指引? |
| 单次 sync conflict > 10 处 | 哪些 patch 触及的核心文件可以避开? |
| 任一 `forkguard_*` fail 但原因不明 | 看 fork-modifications 对应 entry,确认改动是否还必要 |
| **上游加新 API 跟 pinvou3 fork 重叠** | 逐字段比对语义:同语义→撤回用上游;**同名不同义→保 fork**(v0.8.57 composer 即此例) |

## 6. fork patch 组织规则

- fork 按 **C1–C7 + W 逻辑主题**组织(不再用旧 `#1..#42` 全局编号),见 fork-modifications §1。
- **新加 patch**:归入对应主题(工具/prompt/safety/lib/workflow/…),按 §3 配套。
- **删/harvest patch**:撤 fork-guard 指纹 + 在 fork-modifications §2 记一条 + §1 对应小节更新。
- 历史乱或主题混杂时,做 **clean re-fork**(`git reset --soft <release>` 保树 → 按 file→theme 重组线性主题 commit,验字节等价)—— 2026-06-04 / 06-15 两次范例,详见 fork-modifications §4。

## 7. 上游 PR 提交流程

1. 在 fork 主分支 cherry-pick 出独立 commit(每 PR 一个),从 `origin/main`(=最新 release)切净分支。
2. **防泄漏铁律**(#2312 教训):cherry-pick 后必跑 `git diff origin/main <br> | grep -i 'pinvou\|qwen\|vllm\|gb10\|brother whale'` 自查——cherry-pick 常把源 commit 的中文/品牌注释带进来,逐行剔除。
3. Commit/PR body 用英文,引用上游 issue # + pinvou3 实测 case + before/after dump 对比;PR title 用 conventional commit。
4. Accept 后等下次 sync 自动 harvest,**不要手动从 fork 删除**。

> ⚠️ **PR 被 CLOSED ≠ 功能没进上游**:上游常**独立重实现**(v0.8.49 override hook #2356、v0.8.57 composer #2786 / skills union #2737 均如此)。sync 时按文件级 diff 逐字段比对,别假设。

## 8. 上游 PR 状态(2026-06-15 v0.8.60 sync 核对)

> `gh pr list --repo Hmbown/CodeWhale --author h3c-hexin --state all` 核。head 走 `h3c-hexin/DeepSeek-TUI` 跨 fork。v0.8.60 sync 无新提 PR。

**🟡 OPEN**:(无)

**⏹️ CLOSED —— 上游独立实现或不跟进**(fork 侧已取上游版 / 保 fork patch)

| PR | 内容 | 处置 |
|---|---|---|
| #2786 | static prompt composer override | 上游 v0.8.57 自行实现**窄版**(同名 API 语义不同)→ pinvou3 宽版保 fork(C7) |
| #2737 | skills_dir union | 上游 v0.8.57 已 harvest union 接线 → pinvou3 #41 路径收窄仍 fork(C5) |
| #2736 | tool_agent_route 继承父 model | subagent 路径生产不可达(blocklist),fork 侧无此 patch,不跟进 |
| #2356 | prompt override OnceLock hook | 上游 v0.8.49 自行实现并扩展(10 个 `set_*_override`)→ fork 取上游版 |
| #2312 | skills_dir union(自关) | diff 泄漏 pinvou3 字样;功能仍 fork 必需(见 #2737) |
| #2044 / #1790 | file_search cancel/timeout | 上游 #2035 自家实现覆盖,fork 撤回留上游版 |
| #1480 | vLLM chat_template_kwargs | 留 fork |

**✅ 已 MERGED(下次 sync 随上游归零,别重复提)**:#1511 exec reasoning_effort · #1686 OpenAI batch tool_calls · #2057 subagent completion role · #2060 256K auto-compact · #2146 grep_files timeout · #2147 max_output_tokens env · #2245 web_search bing 解码 · #2311 InstructionSource · #2313 Tier5 cover · #2314 environment volatile · #2354 subagent stop-on-failure · #2355 fetch_url fake-ip · **#2946 Bocha 响应解析**(v0.8.57)。

**❌ 评估后不提**:C3(64KB cap / truncated_args_hint)—— 与 pinvou3 专属 `append_file` 深度耦合,上游无 append_file 去耦后引导无落点。

## 9. 相关文档
- `docs/底座升级验收清单.md` — **每次升级必过的硬 gate**(L0 编译 / L1 自动化测试 / L2 六大功能验收 / L3 回归专项 + 签收表)
- `docs/fork-modifications.md` — fork 现状清单(§1 C1–C7+W 结构 / §2 移除·harvest / §3 fork-guard + 验证 checklist / §4 sync 历史)
- `docs/auto-compact-256K-tuning.md` — 256K 窗口 compact 阈值调参依据
- `docs/system-prompt-架构.md` — system prompt 全链路梳理
- `scripts/fork-guard.sh` — 指纹 + 回归测试守卫
- `bin/dump_system_prompt.rs` — prompt dump 工具(sync 验证必备)
