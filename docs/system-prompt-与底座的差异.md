# pinvou3 system prompt — 与 DeepSeek-TUI 底座的差异

> 创建：2026-05-15
> 状态：方案 B 已落地（`instructions.md` 加 5 个 XML 执行纪律标签）
> 适用范围：解释为什么 pinvou3 不走底座的 13 层 prompt 链路，以及未来如何与上游 drift 共处

---

## 1. 上下文

DeepSeek-TUI v0.8.34 引入了 `<tool_persistence> / <mandatory_tool_use> / <act_dont_ask> / <verification> / <missing_context>` 五个 XML 标签 + "Tool-use enforcement" 强约束（commit `b023d54c`），落在 `base.md` 末尾，用于防 hallucination 和"嘴炮不干活"。

升级 submodule 后我们发现：**pinvou3 把上游 13 层 prompt 拼装链路完全 bypass 掉了**，所以这些新增的执行纪律一行都没注入到 pinvou3 用户的 system prompt。本文记录这个 bypass 的来龙去脉和我们选的折衷方案。

---

## 2. 现状对比

### 2.1 底座的 13 层 prompt（`prompts.rs::system_prompt_for_mode_with_context_skills_session_and_approval`）

```
1.  locale preamble（zh-Hans 自动注入"请用中文思考"）
─── KV cache 稳定层（编译期常量为主，session 内不变）───
2.  BASE_PROMPT (268 行英文，含目标 XML 标签)
3.  personality (CALM / PLAYFUL)
4.  mode_prompt (plan.md / yolo.md / agent.md)
5.  approval_prompt (auto / suggest / never)
6.  project_context (workspace .deepseek/instructions.md 自动生成)
7.  project_context_pack (可选)
8.  environment block (locale/platform/shell/pwd)
9.  translation_output_instruction (可选)
10. skills block
11. Context Management（Yolo/Agent only，谈 /compact 和 cache hit %）
12. COMPACT_TEMPLATE
─── volatile boundary（下方文件可被 session 改写）───
13. instructions（EngineConfig.instructions 指向的 md 文件们）
14. user_memory_block + MEMORY_GUIDANCE
15. goal_objective
16. handoff_block
17. locale closer（zh-Hans 自动注入"再次提醒"）
```

总长 ~400 行，跨编译期常量 + 运行时拼装。

### 2.2 pinvou3 的单层 prompt（`bridge/mod.rs::build_session_system_prompt`）

```
pinvou3-app/src-tauri/resources/bundle/instructions.md  (~120 行中文)
└── {{PINVOU3_WORKSPACE}} 占位符 replace 为 session-specific 路径
```

入口：`engine.rs:139` → `SystemPrompt::Text(rendered)` 直接塞进 `EngineHandle.session.system_prompt`，完全绕过 `system_prompt_for_mode_*`。

### 2.3 Plan / YOLO 的实现位置不同

| 维度 | 底座 | pinvou3 |
|---|---|---|
| 模式差异写在哪 | system prompt 第 4 层 `mode_prompt`（plan.md/yolo.md） | system prompt **三 mode 完全一样**；模式差异在 `EngineConfig.trust_mode/allow_shell/approval_mode` + per-turn `<system-reminder>`（`bridge/mod.rs::reminder_for`） |
| 工具白名单切换 | 由底座 `tool_setup.rs` 根据 `mode` + `allow_shell` 自动切 | 同上（pinvou3 不动这块） |
| 加固机制 | mode_prompt 文案 | per-turn `<system-reminder>` 包在 user message 顶部，命中 Qwen3.6 短期注意力 |

---

## 3. pinvou3 当初为什么要 bypass？

不是疏忽，是有意决定。理由按重要性排序：

### 3.1 工具 UX 不匹配

底座 prompt 假设的是终端用户：
- 谈 `sidebar` —— pinvou3 GUI 不长那样
- 谈 `/compact` slash command —— pinvou3 用户用不上命令行
- 谈 `cache hit %` 角标 —— pinvou3 没暴露这个 UI
- 谈 `reasoning_content` 内部思考块 —— pinvou3 把 thinking 单独渲染了

### 3.2 工具引导过载

底座 prompt 详细描述了 "Decomposition Philosophy: PREVIEW / CHUNK + map-reduce / RECURSIVE"，引导用户用 `agent_open` 起子代理、用 `rlm_open` 跑 Python REPL session、用 `checklist_write` 维护清单。

**问题**：Qwen3.6-35B-A3B 比 DeepSeek-V4 弱一档，这套高级编排撑不住。Qwen3.6 看到工具表会去试，结果是子代理失控、RLM 卡死、checklist 跟实际进度脱节。pinvou3 引导刻意只点 7 个核心工具（read/write/edit/exec_shell/web_search/file_search/code_execution），让 Qwen3.6 在熟悉的小工具集里跑稳。

### 3.3 中文用户语境

底座 base.md 是英文的。中文化通过 `locale_preamble` + `locale_closer` 在头尾加书签解决——但中间 268 行英文 BASE_PROMPT 还是英文。pinvou3 的 INSTRUCTIONS_MD 完全中文，跟 Qwen3.6 中文优势对齐，且包含 pinvou3 独有规则：
- 默认产出目录 `{{PINVOU3_WORKSPACE}}`
- 敏感目录禁令（`~/.ssh/` 等）
- Qwen3.6 没视觉能力的明确提示

### 3.4 Token 预算

底座 baseline ~400 行 prompt + 41 个工具 schema（~28k tokens / 66k 上下文 ≈ 44%）。pinvou3 把 prompt 压到 ~120 行，留出更多窗口给用户实际任务。

---

## 4. Bypass 的代价

| 上游能力 | pinvou3 是否享受 | 影响 |
|---|---|---|
| BASE_PROMPT 的执行纪律（XML 标签） | ❌ 之前没有 | AI 嘴炮多/不验证（已用方案 B 补回） |
| `MEMORY_GUIDANCE` | ❌ | memory 功能本身就关了（`memory_enabled: false`），不影响 |
| `locale_preamble` + `locale_closer`（中文双书签） | ❌ | 长上下文累积英文后可能思考漂移 |
| KV cache 稳定层排版（volatile boundary） | ❌ | INSTRUCTIONS_MD 混杂稳定/易变内容，但 pinvou3 内容总长 ~120 行，cache miss 影响小 |
| `mode_prompt`（plan.md/yolo.md） | ❌ | 由 pinvou3 的 per-turn `<system-reminder>` 补，命中率更高 |
| `Context Management` 章节 | ❌ | 用户用不上 `/compact`，无意义 |
| `COMPACT_TEMPLATE` | ❌ | 不暴露 handoff 机制 |
| 跟上游 prompt 演进自动跟随 | ❌ | 每次上游改 base.md 要人工审 |

---

## 5. 三个方案对比

### A. 完全切回 `system_prompt_for_mode_*` 链路

**改动**：bridge 调上游 13 层函数；把 INSTRUCTIONS_MD 走第 13 层 `instructions` 字段（已 wire 但因 bypass 没生效）

**收益**：XML 标签 + locale 双书签 + 上游 prompt 演进全拿到

**风险**：
- token 膨胀 ~120 → ~400 行
- 引入终端 UX 假设（sidebar/`/compact`）
- 引入 pinvou3 没暴露的高级编排（PREVIEW/CHUNK/RECURSIVE + agent_open + rlm_open）—— Qwen3.6 撑不住
- pinvou3 独有规则（产出目录、敏感目录禁令）被挤到 volatile boundary 下方，优先级降低
- 跟现有 per-turn `<system-reminder>` 形成**两份模式描述**冲突

**结论**：副作用大于收益，不采用。

### B. 把 XML 标签内容复制到 INSTRUCTIONS_MD（已采用）

**改动**：只动一个文件 `pinvou3-app/src-tauri/resources/bundle/instructions.md`，追加 5 个 XML 标签段 + "工具使用强制" 节，做中文化 + 适配 pinvou3 工具名（`code_execution` 而非 `exec_shell python -c`）

**收益**：核心约束（必须调工具/必须验证/不打嘴炮）拿到，零侵入

**风险**：上游 base.md 再改时要人工同步（5 个标签内容稳定，估计 1 年内不会大改）

### C. 编译期 include + slice 上游 base.md 的指定章节

**改动**：bundle.rs 加 `include_str!("../../../DeepSeek-TUI/.../base.md")` 然后字符串 slice 出 "## Execution discipline" 段拼到 INSTRUCTIONS_MD

**收益**：上游改 XML 内容自动同步

**风险**：
- 字符串切割 fragile，上游改个章节标题就崩
- 没法本地化（中文化、改工具名、删 pinvou3 不暴露的工具）

**结论**：fragile + 不可本地化，不采用。

---

## 6. 最终选 B 的理由

1. **改动面最小**：1 个 markdown 文件，0 行 Rust
2. **可控**：内容完全本地化，不引入 pinvou3 没暴露的工具引导
3. **token 友好**：~120 → ~160 行，<1% 上下文增量
4. **跟现有 per-turn `<system-reminder>` 互补**：XML 标签是 session 级稳定约束，reminder 是 turn 级动态加固，两层不冲突
5. **drift 成本低**：XML 标签内容稳定，上游一年内改大概率不超过 5 行；监控办法见 §7

落地位置：`pinvou3-app/src-tauri/resources/bundle/instructions.md` 第 23-66 行（"## 执行纪律" + "## 工具使用强制"）

---

## 7. 如何跟上游 drift 共处

每次 submodule rebase 到新版后，跑一次：

```bash
git -C DeepSeek-TUI diff <old_sha>..HEAD -- crates/tui/src/prompts/base.md
git -C DeepSeek-TUI diff <old_sha>..HEAD -- crates/tui/src/prompts/memory_guidance.md
```

如果 diff 涉及：
- `## Execution discipline` 章节（base.md 第 232-262 行）→ 同步对应中文版到 instructions.md
- `## Tool-use enforcement` 章节（base.md 第 264-268 行）→ 同步"## 工具使用强制"节
- 新增类似 XML 块 → 评估是否摘选

不涉及上述章节的 base.md 改动**不要**同步——pinvou3 故意不要那部分（理由见 §3）。

---

## 8. 未来演进点

按优先级：

1. **观察 XML 标签对 Qwen3.6 的实际效果**：跑一周看 hallucination 率有没有降。XML 标签是 V4 训练的，Qwen3.6 可能"看不见"标签语义，但块内内容是命令式中文 + bullet，单独看也起作用
2. **裁剪 baseline tools schema**（`process.md` 长期搁置项）：61 个工具 → 压到 pinvou3 实际暴露的 ~15 个，可省 ~10k token。等 XML 标签观察期过了再做
3. **per-turn `<system-reminder>` 跟 XML 标签去重**：Yolo Executing 的 reminder 已经说"禁止只调 update_plan"，跟 XML 块内容部分重叠。下次迭代 reminder 文案时精简

---

## 9. 相关代码与决策

- 落地 commit：`pinvou3-app/src-tauri/resources/bundle/instructions.md` 第 23-66 行
- pinvou3 system prompt 入口：`pinvou3-app/src-tauri/src/bridge/mod.rs::build_session_system_prompt`
- 上游 prompt 拼装：`DeepSeek-TUI/crates/tui/src/prompts.rs::system_prompt_for_mode_with_context_skills_session_and_approval`
- 上游 XML 标签源：`DeepSeek-TUI/crates/tui/src/prompts/base.md` 第 232-268 行
- 上游引入 commit：`b023d54c feat(prompts): XML-tagged execution discipline + tool-use enforcement`
- 相关决策：
  - `docs/archived/Plan-YOLO双模式-设计决策.md` §13.1（per-turn `<system-reminder>` 设计）
  - `docs/archived/DeepSeek-TUI-架构详解.md` §5（上游 prompt 拼装结构）
