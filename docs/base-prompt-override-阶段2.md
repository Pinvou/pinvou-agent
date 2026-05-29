# base.md / 品牌串 override 化迁移 — 阶段 2 规格

> 创建 2026-05-29。目标：把 pinvou3 对底座 prompt 文案的 fork 改动搬到 pinvou3-app，
> DeepSeek-TUI submodule 对这些 prompt 的 fork drift → 0（hook 除外，hook 是上游 PR 候选）。
> 方案="务实大头"：只迁 `base.md` + `LOCALE_PREAMBLE_ZH_HANS` + `AUTHORITY_RECAP` 三处；
> mode/approval/compact 小文案留 fork（边际收益低于改造成本）。

## 执行进度 — 阶段2 已完成（2026-05-29）

全部落地并验证通过：

**submodule（commit `af64e9f7`，在阶段1 `5f847284` 之上）**
- `base.md` 回退上游原文（`git diff 54151a4b` = 0，字节一致）
- `prompts.rs` 品牌词回退 codewhale（locale / authority / doc 注释 3 处）
- 删除 6 个内容 forkguard 测试（职责移到 pinvou3-app），authority 测试断言回退 CodeWhale
- 全量测试 **3349 passed / 0 failed**；hook（3 个 `set_*_override`）完好

**pinvou3-app（working tree，待父仓 commit）**
- `resources/bundle/base.md` = pinvou3 版 base.md（249 行）
- `bridge/bundle.rs` 3 常量（`BASE_PROMPT_MD` + `LOCALE_PREAMBLE_ZH_HANS` + `AUTHORITY_RECAP`）+ `install_prompt_overrides()` + 3 个新 forkguard 测试（内容锚点 + override 端到端生效）
- `bridge/mod.rs` `boot()` 调 `install_prompt_overrides()`（早于 ensure_dirs；dump_system_prompt bin 同经此 boot）
- forkguard 测试 **4 passed / 0 failed**

**fork-guard.sh + 文档**
- 7 个品牌/内容指纹 retarget 到 pinvou3-app 文件 + 新增 #42 hook 存活指纹；`--fast` 指纹层全过
- `fork-modifications.md` prompts 段更新为 override 架构

**端到端**：`dump_system_prompt` = 38746B，pinvou3 内容全在（CONSTITUTION OF PINVOU3 / running inside pinvou3 / 你正在 pinvou3 / authority pinvou3），上游内容全无（Brother Whale / RLM / CODEWHALE = 0）。与阶段1 渲染字节一致 → 行为零回退。

> 下方为阶段2 原始规格（执行前所写），保留作设计记录。

---

## 阶段 1 已完成（commit `5f847284`，submodule pinvou3-patches 分支）

`crates/tui/src/prompts.rs` 加了 3 个 `OnceLock` override + setter + `effective_*()` getter，
三处使用点改用 getter。**向后兼容**：override 未 set → 返回原 const，行为完全不变。
验证：`cargo check` 0 error；36 个 prompt/forkguard 测试全过。

| override const | setter（pub） | effective getter | 使用点 |
|---|---|---|---|
| `BASE_PROMPT` (L273) | `set_base_prompt_override` | `effective_base_prompt()` | `compose_prompt_with_approval_and_model` parts[0] |
| `LOCALE_PREAMBLE_ZH_HANS` (L372) | `set_locale_preamble_zh_hans_override` | `effective_locale_preamble_zh_hans()` | `locale_reinforcement_preamble` 的 zh-Hans 分支 |
| `AUTHORITY_RECAP` (L558) | `set_authority_recap_override` | `effective_authority_recap()` | 末尾 append（已改为 `let authority_recap = ...` 绑定） |

（行号是 commit 前的近似，加 hook 后整体下移 ~45 行；定位靠 `grep`，不靠行号。）

## 阶段 2 剩余步骤

### A. DeepSeek-TUI submodule 侧（回退 fork 内容到上游）

1. **base.md 回退上游原文**：
   `git show 54151a4b:crates/tui/src/prompts/base.md > crates/tui/src/prompts/base.md`
   （`54151a4b` = merge-base(origin/main, HEAD)，即 fork 改动前的上游 base.md，297 行。
   也可用 `origin/main:` 版，内容一致。回退后 submodule base.md fork drift = 0。）

2. **LOCALE_PREAMBLE_ZH_HANS 回退品牌词**：把常量里 `你正在 pinvou3 中运行` 改回
   `你正在 codewhale 中运行`（仅此 1 词差，其余文本两版相同）。

3. **AUTHORITY_RECAP 回退品牌词**：把常量里 `The Constitution of pinvou3 (Articles I-VII)`
   改回 `The Constitution of CodeWhale (Articles I-VII)`（仅此 1 词差）。

4. **改造受影响的 forkguard 测试**（base.md/locale/authority 回退后，这些断言 pinvou3
   内容的测试会失败 —— 它们测的是 `BASE_PROMPT`/裸 `compose_prompt`，不经过 override）：
   - `forkguard_constitutional_preamble_uses_pinvou3_branding`
   - `forkguard_rlm_section_removed_by_pinvou3`
   - `forkguard_tool_selection_guide_is_embedder_aware`
   - `forkguard_pinvou3_omitted_upstream_specific_tool_names_from_base_prompt`
   - `forkguard_no_deepseek_specific_fork_context_prose_in_base_prompt`
   - `forkguard_local_law_tier_covers_engine_config_instructions`
   - authority recap 测试（断言 "Constitution of pinvou3"）
   - `system_prompt_prepends_locale_preamble_for_zh_hans`（若断言含 "pinvou3" 则需调整）

   处理：在 submodule 侧删除这些"断言 pinvou3 内容"的测试（内容已搬走），
   **改为测 override 机制本身**：set 一个 marker override → `compose_prompt` 后应含 marker；
   未 set → 应为上游 const。这层"内容正确性"测试搬到 pinvou3-app（见 B4）。

### B. pinvou3-app 侧（承载 pinvou3 prompt 内容 + 启动注入）

1. **新建 `pinvou3-app/src-tauri/resources/bundle/base.md`** = 当前 pinvou3 版 base.md（249 行，
   含 CONSTITUTION OF PINVOU3 / running inside pinvou3 / embedder-aware 改写 / 删 RLM·Toolbox·V4 段）。
   取自 commit `5f847284^` 的 `crates/tui/src/prompts/base.md`（即回退前的版本）：
   `git show 5f847284:crates/tui/src/prompts/base.md`（阶段1未动 base.md，所以 5f847284 的 base.md = pinvou3 版）。

2. **locale/authority 的 pinvou3 文本**：各放一个 Rust 常量或 bundle 小文件。
   就是 submodule 回退前的 pinvou3 版（"你正在 pinvou3 中运行" / "Constitution of pinvou3"）。
   注意 base.md 里若含 `{model_id}` 占位符要保留（`apply_model_template` 会替换）。

3. **启动入口调 3 个 setter**：在 app 最早处、**engine 池初始化前**（多 session 并发：
   OnceLock 全局只 set 一次，base 对所有 session 一致，无竞态）。候选位置见
   `bridge/mod.rs` boot 或 app 入口。调用：
   ```rust
   deepseek_tui::prompts::set_base_prompt_override(include_str!(".../bundle/base.md").to_string());
   deepseek_tui::prompts::set_locale_preamble_zh_hans_override(PINVOU3_LOCALE_ZH.to_string());
   deepseek_tui::prompts::set_authority_recap_override(PINVOU3_AUTHORITY.to_string());
   ```

4. **新增 forkguard 测试在 pinvou3-app**：测 override 内容正确（base 含 "CONSTITUTION OF PINVOU3"、
   不含 Brother Whale/RLM/Toolbox）+ override 已生效（spawn 的 prompt 含 pinvou3 内容）。
   = 把 A4 删掉的内容断言搬来这里。

### C. fork-guard.sh + 文档

1. **fork-guard.sh 指纹**：base.md/prompts.rs 品牌指纹（`#28/#32/#33/#36/#37/#38`，
   grep "CONSTITUTION OF PINVOU3" / "running inside pinvou3" / "Match the embedder's render target" /
   "concurrent cap is embedder-configured" / "你正在 pinvou3 中运行" / "Constitution of pinvou3 (Articles I-VII)"）
   **目标文件改** `DeepSeek-TUI/crates/tui/src/prompts/base.md` → `pinvou3-app/src-tauri/resources/bundle/base.md`
   （locale/authority 改到其 override 内容所在文件）。
   **新增 1 条**：grep `set_base_prompt_override` 在 `prompts.rs` —— 防 sync 时 hook 被冲掉、
   override 静默失效退回上游 base（这正是 fork-modifications.md 记录过的"merge 静默丢失"坑）。

2. **fork-modifications.md**：更新 `#28/#32/#33/#36/#37/#38` 形态描述（从"改 base.md/prompts.rs"
   → "override hook + 内容在 pinvou3-app"），并登记 hook（阶段1 commit `5f847284`）。

## 验证

- submodule：`cargo check -p codewhale-tui --lib`（0 error）+ 改造后的 prompt 测试全过
- pinvou3-app：`cargo check -p pinvou3-tauri` + 新 forkguard 测试过
- 端到端：`cargo run --bin dump_system_prompt`，确认渲染出的 prompt 含 pinvou3 brand（override 生效）
- `./scripts/fork-guard.sh --fast`（指纹层）全绿
- 预期 submodule fork drift 净降 ~160 行（base.md 全部 + 2 品牌词）

## 上游 PR

hook（`set_*_override` + `effective_*`）是通用 embedder 能力 → 按 fork-policy §2 Q2 提上游 PR。
merge 后连 hook 那 ~45 行 fork drift 也归零 = prompt 终态零 fork。

## 注意（本次环境）

阶段 1 执行时工具结果通道间歇性串台（Read 错位、Bash 输出混入幻觉文本）。
阶段 2 涉及删改测试 + 跨 crate，务必：单工具调用串行、大块用 git、每步靠
`cargo check`/`cargo test` 的 exit code 与 `test result:` 行兜底，不依赖单次读取的文本。
