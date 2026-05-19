# Auto Compact 调参记录 (Qwen3.6 + 256K context)

日期：2026-05-19
背景：用户问"我现在 context 是 256K，auto compact 应该如何触发，不需要手动触发"。

## TL;DR

底座 DeepSeek-TUI **已经在 turn_loop preflight 内置了 should_compact 触发**（turn_loop.rs:90），按 `token_threshold` 直接调 `compact_messages_safe` 走 LLM 摘要。但对 pinvou3 + Qwen 原生配置仍不工作，根因有二：

1. 模型名 `qwen36_35b` 没 `_Nk` 后缀，`context_window_for_model` 返回 `None` → preflight 二级保险 (`recover_context_overflow`) 静默禁用。
2. 即便加了后缀让底座识别 256K，`context_input_budget` 仍按 `TURN_MAX_OUTPUT_TOKENS = 262K` 算 input budget → `256K - 262K - 1K = 负数` → 同样返回 `None`，emergency 路径静默禁用。

会话只会一路涨到 vLLM `max_model_len` 撞墙报 400。

## 全套修复

bridge 2 处 + fork 2 处 + 前端 1 处 + L1 harness + 回归测试：

### A. pinvou3-app/src-tauri/src/bridge/mod.rs

- **compaction**：`token_threshold = 200_000`（256K × ~78%）、`auto_floor_tokens = 60_000`（默认 500K 地板撞不到，必须降到 should_compact 能放行的水位）。turn_loop:90 的 preflight `should_compact` 直接吃这俩参数。
  - ⚠️ `auto_floor_tokens` 只是 should_compact 的"低于则拒绝"下限，**不是"超过启动 prune"的上限**（codex round 3 finding 2）。`prune_tool_results` 是 `compact_messages_safe` 内部的第一步，必须 should_compact 先返回 true（≥200K）才会跑。60K–200K 区间什么都不做。
- **capacity controller**：保持上游 default = off（**不**打开）。
  - 第一版误开 capacity 是被 codex round 1 误导 + 没看到 turn_loop:90 的 preflight。round 2 codex 抓出 capacity 的 `low_risk_max` / `medium_risk_max` 是 **p_fail 风险阈值**而非 `context_used_ratio`，且 `p_fail` 公式里 context 只占 15% 权重（35% action + 30% tool + 20% ref + 15% context），打开后会让"复杂工具轮"在 context 远低于 200K 时触发 `VerifyAndReplan` / `VerifyWithToolReplay` 改写会话——这不是 auto compact 想要的。
  - turn_loop:90 的 preflight should_compact 已经是干净的 context-pressure 触发，根本不需要 capacity。
- **cycle 子系统**：覆写 `enabled = false`。
  - 上游默认 true，但 `cycle_manager.rs:184` 算 `trigger_floor = window saturating_sub reserved_response_headroom_tokens` 用的 reserved 是 `TURN_MAX_OUTPUT_TOKENS + 1024 = 263_168`，对 256K 窗口减出 0；接着 `threshold.min(0) = 0`（per-model override 也救不回来，因为是 `min`），`active_input_tokens >= 0` 永远成立 → **每轮 turn 结束都做 briefing + 归档 + 重置 messages**（codex round 3 finding 1）。
  - pinvou3 用 compaction 路径管 context，cycle 是另一套（checkpoint-restart），关掉避免双系统冲突 + 上游 saturating_sub bug。理论上 fork B4（让 cycle 也按窗口分级算 reserved）能根治，但 cycle 对 pinvou3 是冗余子系统，关掉够用。
- **LOCAL_VLLM_MODEL**：改 `qwen36_35b_256k`（后缀让 fork B1 派生 256K 窗口）。
- 加回归测试 `default_model_window_recognized_by_engine` 锁住"默认模型必须被底座识别窗口"。

### B1. DeepSeek-TUI fork（已 commit 7e5288e3）

`context_window_for_model`（models.rs:212）把 `_Nk` hint 检查从 deepseek 分支内部**移到所有 vendor 分支前面**。让任意 vendor 模型名都能通过 `_Nk` 后缀声明窗口。hint 用 kilo × 1000 算，256k → 256_000，与实际 262_144 差 6K（2%），在压缩计算精度内可接受。

### B2. DeepSeek-TUI fork（同上 commit）

`context_input_budget`（engine/context.rs:366）的"reserved output"项按窗口分级：

- `window ≥ 500K`（V4-class）：仍用 `TURN_MAX_OUTPUT_TOKENS` —— 保持上游 V4 interleaved thinking 的保守预算契约（被 `internal_context_budget_unaffected_by_api_request_cap` 测试锁定）
- `window < 500K`（自托管/小窗口）：用 `effective_max_output_tokens(model)` —— 即 API 实际发的 `max_tokens`（受 `DEEPSEEK_MAX_OUTPUT_TOKENS` env 控制，pinvou3 设 16384）

副作用：`context_input_budget` 签名简化为 `fn(model: &str) -> Option<usize>`，三处调用点（turn_loop / capacity_flow / engine）同步删第二参数；`recover_context_overflow` 删 `requested_output_tokens` 参数。

### C. 前端 token-bar 隐藏

`pinvou3-app/src/index.html` 给 `.token-bar` 加 `hidden`，`pinvou3-app/src/styles/chat.css` 加 `display: none` 双保险。auto compact 不需要用户看 context 压力指示。

### D. L1 harness 默认模型同步

`pinvou3-app/src-tauri/tests/l1_dialog_harness.rs:445` 默认 `DEEPSEEK_MODEL=qwen36_35b_256k`，与 bridge / run-dev 一致。

### E. ops（用户做一次）

vLLM 启动加 `--served-model-name qwen36_35b_256k`。OpenAI-compat API 协议要求 `model` 字段匹配 served name，不改的话启动后立刻报 `model_not_found`（显式失败，不会再有静默退化）。

## 触发线全景（最终版）

```
0K ──────────────────── 200K ───────── ~239K ──── 256K
                       │                │          │
                       │                │          └─ vLLM max_model_len 硬墙
                       │                └─ B2 preflight emergency 兜底
                       │                   (recover_context_overflow,
                       │                    估算 > 239K 强制 compact)
                       └─ turn_loop:90 preflight should_compact
                          触发 compact_messages_safe (内部先 prune_tool_result
                          再决定 LLM 摘要)。auto_floor=60K 只是允许触发的下限
```

稳态：

- **0–200K**：完全不动（auto_floor 是下限，60K 那条线在当前实现下不做任何事）
- **≥200K**：turn_loop:90 调 `compact_messages_safe`
  - 内部先 `prune_tool_results`（机械去重旧工具结果），如果剪完已经回到 200K 以下直接返回，**省一次 LLM**
  - 否则做 LLM 摘要：末尾 4 条 + 工作集相关 pin 保留 + 旧消息总结成 system block
- **≥239K**：B2 preflight emergency 兜底（最多重试 2 次）
- **≥256K**：vLLM 硬拒（理论上 emergency 已救回）

> 如果将来发现 200K 才 LLM 太贵，可以做 fork B5：加独立 preflight prune-only 分支让 60K 起就剪老的重复工具结果，不进 LLM。当前实现没有这条路径。

## fork PR roadmap

B1 + B2 都是上游友好的改动，可独立 PR：

| Patch | 文件 | 行数级 | PR 卖点 |
|---|---|---|---|
| B1 | models.rs:212 | ~5 行 | 让 `_Nk` hint 适用于任何 vendor，自托管 / 第三方部署友好 |
| B2 | engine/context.rs:366 + 调用方 | ~25 行 | 修 < 500K 窗口模型 input budget 为负 → None 的静默禁用 bug，含双路径回归测试 |

按 CLAUDE.md "修上游 bug ≤50 行 + 立即 PR" 节奏。fork 总 patch 量 ~30 行，已 commit 在 DeepSeek-TUI submodule（`pinvou3-patches` branch tip = `7e5288e3`），父仓库 gitlink 已 stage。

## Codex adversarial review 反馈历程

**Round 1（先关 capacity / 后开 capacity 的来回过程）**

我最初担心 200K token_threshold 没生效，因为只看到 capacity_flow 里的 `apply_targeted_context_refresh` 调 should_compact，没看到 turn_loop:90 那条更直接的路径。于是错误地打开 capacity controller 想"激活"200K 阈值。codex round 1 也没指出这个误判，反而抓了"默认模型没带 _256k 后缀"作为高优 finding。

**Round 2（codex 抓出 capacity 危险 + 我自己发现 turn_loop:90）**

codex 抓出 capacity 的 ratio 阈值是 p_fail 不是 context_used_ratio，且 enable 后 VerifyWithToolReplay / VerifyAndReplan 会改写会话。verify 时翻 turn_loop.rs 才发现 :90 早就有 should_compact 触发。回到最简设计：capacity off，靠 turn_loop:90 内置 preflight。

**Round 3（codex 抓出 cycle 每轮触发 + docs prune 假承诺）**

codex 抓 cycle_manager.rs:184 在 256K 窗口下 `trigger_floor=0`（saturating_sub reserved 263K 后归零），每轮 turn 后都 briefing/归档/重置 messages —— B1 让 qwen36_35b_256k 派生窗口反而踩了这个二次坑。同时抓出"60K-200K 本地 prune 区间"是 docs 虚构的；prune_tool_results 是 compact_messages_safe 内部的，60K 是 floor 不是启动线。

修复：bridge 关 `cycle.enabled = false`（与 capacity 同理：上游默认 on 但对 pinvou3 + 小窗口模型有 bug），docs 校准触发线。

教训：每次扩展底座识别（B1 让派生路径生效）都要同时审 cycle / capacity / compaction 三个相互独立的"context 管理子系统"是否都按窗口分级算预算。底座是为 V4 1M context 设计的，把它接到 256K 窗口必然踩边缘条件。

**Round 4（codex 抓出生产路径未 wire max_output_tokens）**

codex 抓 `build_dt_config` 没 export `DEEPSEEK_MAX_OUTPUT_TOKENS`。`effective_max_output_tokens` 只读这个 env，clean env 启动时（用户直接双击 Tauri app，不走 `run-dev.sh`）返回 fallback：
- `min(window/2, API_MAX_OUTPUT_TOKENS) = min(128K, 64K) = 64K`

→ B2 算 `context_input_budget = 256K - 64K - 1K = 191K`（而非文档承诺的 ~239K），preflight emergency 提前 48K 触发，长 turn 可能因输出预算太满而被强制压缩或 provider 400。`PINVOU3_MAX_OUTPUT_TOKENS` 和 `prefs.advanced.max_output_tokens` 的接入点（`Pinvou3Bridge::max_output_tokens()`）建好了但**没有 wire 给 engine**。

修复：`boot()` 里 `if env::var_os("DEEPSEEK_MAX_OUTPUT_TOKENS").is_none() { env::set_var("DEEPSEEK_MAX_OUTPUT_TOKENS", self.max_output_tokens().to_string()) }`。已有 env 时不覆盖（允许 run-dev.sh / L1 harness / 用户 override）。回归测试 `boot_wires_deepseek_max_output_tokens_env` 锁住两个语义（clean env set 16384 + 已有 env 不覆盖）。

同时 round 4 把 bridge 自己的 compaction 注释里 "60K-200K 区间纯本地去重" 也删了（round 3 只改了 docs，bridge 注释漏改）。

教训：用户控制的 prefs/method（`max_output_tokens()`）必须真接到 engine 跑的路径上；不接就是死代码。Tauri app 生产路径和 dev 脚本路径要在测试里覆盖到。

**Round 5（codex 抓出 boot 测试隔离）**

codex 抓 `boot_wires_deepseek_max_output_tokens_env` 测试直接调 `Pinvou3Bridge::boot()`，会 ensure ~/.pinvou3 + 解包 bundle + 写 settings.json，没拿 `paths::tests::ENV_LOCK`、没 set 临时 PINVOU3_HOME，跟既有测试隔离约定相冲突。修复：抽出 `wire_max_output_tokens_env()` helper，boot 调它，测试改用 `fixture_bridge().wire_max_output_tokens_env()` 不走 boot，无 disk 副作用。

**Round 6（codex 抓出 submodule push 部署断点）**

codex 抓父仓库 staged gitlink 指向 `7e5288e3`，但 `pinvou3-patches` 分支领先 `fork/pinvou3-patches` 7 commit —— 包含 7e5288e3 在内的所有 [pinvou3-fork] 改动都未 push。如果先 commit/push 父仓库，干净 clone / 部署机做 `git submodule update` 会失败。

修复：`git -C DeepSeek-TUI push fork pinvou3-patches`（fast-forward，7 个 commit 全是 fork 工作流 patches，最新一个是 B1+B2 7e5288e3）。push 完成后远端可达，父仓库可以 commit gitlink。

## 验证

- `cargo check -p pinvou3-tauri` ✅
- `cargo test -p pinvou3-tauri --lib` **51/51** ✅（含 round 4 新增 `wire_max_output_tokens_env_sets_default_then_respects_existing`）
- `cargo test -p deepseek-tui --lib context_budget` 3/3 ✅（V4 路径不变 + 256K 路径）
- `cargo test -p deepseek-tui --lib context_window` 4/4 ✅（B1 兼容性）
- DeepSeek-TUI `pinvou3-patches` 已 push 到 fork remote（HEAD = 7e5288e3）✅

未做：实际 256K 会话端到端验证。需用户：
1. vLLM 重启时带 `--served-model-name qwen36_35b_256k`
2. 重启 pinvou3 跑长对话
3. 观察 ≥200K 时是否自动出现 "Auto-compacting context..." status，以及末尾保留 + summary 拼回的 session_updated
