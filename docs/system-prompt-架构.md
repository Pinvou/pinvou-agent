# pinvou3 System Prompt 架构

> 创建：2026-05-28
> 范围：从 session 创建到首次发给 LLM，system prompt 是怎么被构造出来的
> 目的：让任何想动 prompt 行为的人能定位到正确的修改点
>
> ⚠️ **2026-07-17 复核**：文中「per-session instructions 写 disk
> （`~/.pinvou3/sessions/<sid>/instructions.md`）」的描述（§1 时序图、§3 骨架、§4 rehydrate
> 语义、§5 速查表相关行、§7 验证清单）已过期。现行为 C 方案 P-no-disk：instructions 走
> `InstructionSource::Inline` 内存注入（`bridge/mod.rs` `session_instructions()`），不再写
> disk；`sync_session` 传 `system_prompt: None`（`src-tauri/src/engine.rs`）；`{{PINVOU3_WORKSPACE}}`
> 占位符已移除，workspace 改走 per-turn `<turn_meta>` 注入；用户自定义 instructions 现路径为
> `~/.pinvou3/user/instructions.md`（`~/.deepseek/instructions.md` 会在 boot 时被清理，见
> `bridge/mod.rs` `cleanup_legacy_pinvou3_disk_files()`）。底座 13 层拼装与第 6a 层
> `<instructions>` 注入点的结论不受影响；下文相关段落保留作 2026-05-28 时点记录。
> 另：各 `file.rs:NNN` 行号随版本漂移，定位以符号名为准。

---

## 0. TL;DR

- 底座 DeepSeek-TUI 在 `Engine::new` 里**自动跑 13 层 prompt 拼装**，pinvou3 没有绕过这个流程。
- pinvou3 只动两个地方：
  - **第 6a 层 `<instructions>`**：通过 `EngineConfig.instructions` 把 `bundle/instructions.md` 渲染版塞进去。
  - **Per-turn `<system-reminder>`**：贴在每次 user message 头部，按 `(mode, phase)` 选段。
- 其它 12 层（BASE_PROMPT / mode_prompt / project_context / environment / skills / handoff / authority / locale 等）全是底座管，pinvou3 通过 `EngineConfig` 字段（`workspace` / `skills_dir` / `locale_tag` / `mode`）影响渲染参数，不直接改文案。
- 多 session 并发隔离的关键：每个 session 写自己的 `~/.pinvou3/sessions/<sid>/instructions.md`，互不串台（参考 [`多session并发-设计实现.md`](./多session并发-设计实现.md)）。

---

## 1. Session 启动时序

```
用户 GUI 点新会话
  ↓
pinvou3-app/src-tauri/src/engine.rs:56  AppEngine::spawn_for_session()
  │
  ├─ bridge.write_session_instructions(sid)                  # bridge/mod.rs:172
  │    └─ INSTRUCTIONS_MD                                    # bundle/instructions.md 编译期 include
  │         .replace("{{PINVOU3_WORKSPACE}}", ws)
  │         .replace("{{PINVOU3_SUDO_INSTRUCTION}}", ...)
  │       → 写到 ~/.pinvou3/sessions/<sid>/instructions.md
  │
  ├─ bridge.build_engine_config_for_session(sid)             # bridge/mod.rs:422
  │    workspace    = ~/.pinvou3/sessions/<sid>/workspace
  │    instructions = [上面那个文件, ~/.deepseek/instructions.md(可选)]
  │    skills_dir   = ~/.pinvou3/bundle/skills
  │    locale_tag   = "zh-Hans"
  │
  └─ spawn_engine(engine_config, dt_config)
       └─ Engine::new(...)                                   # DeepSeek-TUI core/engine.rs:443
            └─ prompts::system_prompt_for_mode_with_context_skills_session_and_approval(
                  AppMode::Agent,
                  &config.workspace,
                  None,
                  Some(&config.skills_dir),
                  Some(&config.instructions),                ← pinvou3 注入点
                  PromptSessionContext { locale_tag, ... },
                  session.approval_mode,
              )                                              # core/engine.rs:458-475
            → session.system_prompt = SystemPrompt::Text(13 层拼装结果)
```

**关键**：`Engine::new` 行 458 强制走 13 层拼装函数，**pinvou3 无法跳过**。所有改 prompt 文案的诉求要么通过 `EngineConfig` 字段影响，要么往 `instructions.md` 加内容。

---

## 2. 13 层拼装顺序

入口：`DeepSeek-TUI/crates/tui/src/prompts.rs:673` 的 `system_prompt_for_mode_with_context_skills_session_and_approval()`。

### 静态前缀（cacheable，session 内不变，命中 KV prefix cache）

| 层 | 文件位置 | 来源 | 内容 |
|---|---|---|---|
| **0** | `prompts.rs:696` | `locale_reinforcement_preamble(locale_tag)` | zh-Hans/ja/pt-BR 时，最前面贴"用该语言思考"前奏 |
| **1a** | `prompts.rs:683` | `compose_prompt_with_approval_model_and_shell()` | BASE_PROMPT (include `prompts/constitution.md`，v0.9 前为 base.md) + mode 子段 (plan/yolo/agent) + ApprovalMode 段 |
| **1b** | `prompts.rs:686` | `load_project_context_with_parents(workspace)` | workspace 内自动扫的 `.codewhale/instructions.md` 块 |
| **2a** | `prompts.rs:731` | `render_environment_block()` | `## Environment`：locale / platform / shell / pwd |
| **2b** | `prompts.rs:741` | `translation_output_instruction()` | 仅当 `translation_enabled=true`（pinvou3 关闭） |
| **3** | `prompts.rs:757` | `render_available_skills_context_for_workspace()` | `## Skills`：只列名字+一句描述+SKILL.md 路径，body 不内联 |
| **4** | `prompts.rs:764-781` | 编译期常量 | `## Context Management` + `## Prompt-cache awareness`（仅 Yolo/Agent） |
| **5** | `prompts.rs:785` | `COMPACT_TEMPLATE` | handoff.md 格式说明（用于 `/compact` 跨会话 relay） |

### ⎯⎯⎯ Volatile Boundary（`prompts.rs:785` 那条分界注释）⎯⎯⎯

下方每段都可能 turn-to-turn 漂移，不进 KV prefix cache。

| 层 | 文件位置 | 来源 | 内容 |
|---|---|---|---|
| **6a** | `prompts.rs:800-804` | `render_instructions_block(cfg.instructions)` | **pinvou3 注入点**：每个 instructions 文件包成 `<instructions source="...">...</instructions>` |
| **6b** | `prompts.rs:810` | `session_context.user_memory_block` | 用户长期 memory 笔记 + MEMORY_GUIDANCE（pinvou3 关了 memory，此层空） |
| **6c** | `prompts.rs:819` | `session_context.goal_objective` | `## Current Session Goal`（pinvou3 没用到，此层空） |
| **7** | `prompts.rs:829` | `load_handoff_block(workspace)` | 上一会话 compact 出来的 `.codewhale/handoff.md` |
| **8a** | `prompts.rs:836` | `AUTHORITY_RECAP` 常量 | 权威优先级提醒（system > instructions > user） |
| **8b** | `prompts.rs:849-860` | `locale_reinforcement_closer()` 或 `hidden_thinking_language_instruction()` | 中文输出收尾 |

### 然后是 user message —— pinvou3 在这里贴第二个 prompt 层

**文件**：`pinvou3-app/src-tauri/src/bridge/mod.rs:489 build_send_message_op()`

```rust
let full_content = match reminder_for(mode, phase) {
    Some(r) => format!("<system-reminder>\n{r}\n</system-reminder>\n\n{content}"),
    None    => content,
};
```

提醒文本是 `bridge/mod.rs:530 reminder_for()` 的 hardcode 表：

| mode × phase | 提醒大意 |
|---|---|
| Plan × Planning | 在 Plan 模式，只读+规划 |
| Plan × Ready    | Plan + Ready，等用户确认 |
| Yolo × Executing | 执行阶段，禁止只调 update_plan |
| Yolo × None      | Yolo 自由动 |

每 turn 重选，不进 system 块。

---

## 3. 最终骨架（伪结构）

```text
┌─ SYSTEM ─────────────────────────────────────────────
│ <locale 中文思考预备>                       ← 底座 const (prompts.rs:696)
│ <BASE_PROMPT + mode + approval>             ← 底座 prompts/constitution.md
│ <project context (auto-scanned)>            ← 底座扫 ws
│ <Environment block>                          ← 底座生成
│ ## Skills (name + desc + path 列表)         ← 底座扫 skills_dir
│ ## Context Management / Cache awareness     ← 底座 const (Yolo/Agent)
│ <COMPACT_TEMPLATE>                          ← 底座 const
│ ══════════════ volatile boundary ══════════════
│ <instructions source=".../sessions/<sid>/instructions.md">
│   ← pinvou3 bundle/instructions.md 渲染版（占位符已替换）
│   ← 这里是 pinvou3 塞 SKILL 路由 / Plan 规则 / sudo 块 / 执行纪律 的唯一槽位
│ </instructions>
│ <instructions source="~/.deepseek/instructions.md">  ← 可选，用户自建
│ <handoff block>                             ← 上一会话 compact 产物（若有）
│ <AUTHORITY_RECAP>                           ← 底座 const
│ <locale closer / hidden-thinking 语言指令>   ← 底座 const
└──────────────────────────────────────────────────────

┌─ USER (每 turn 重组) ────────────────────────────────
│ <system-reminder>                           ← pinvou3 注入
│   按 (mode, phase) 选的提醒文本
│ </system-reminder>
│
│ <用户实际输入 + 附件预解析后的 file_ingest 块>
└──────────────────────────────────────────────────────
```

---

## 4. Rehydrate / 切 session 时的覆盖语义

### 4.1 `Op::SyncSession`（切 session）

`pinvou3-app/src-tauri/src/engine.rs:167 sync_session()` 会发：

```rust
Op::SyncSession {
    messages,
    system_prompt: Some(SystemPrompt::Text(bridge.build_session_system_prompt(sid))),
    system_prompt_override: false,   // ← 关键：不 override
    ...
}
```

`system_prompt_override: false` 表示**底座可以拿这个值当 hint，但 rehydrate 时会重新跑 13 层拼装覆盖**（见下文）。

### 4.2 `rehydrate_latest_canonical_state` 行为

`DeepSeek-TUI/crates/tui/src/core/engine.rs:1878 refresh_system_prompt()` 会**重新调用 13 层拼装函数**，参数照样从 `EngineConfig.instructions` 读 disk 文件。

**所以**：sync_session 传进去的 `SystemPrompt::Text` 拼装结果会被覆盖。**真正能让 AI 看到 session-specific 内容的，是改 disk 上的 instructions.md 文件本身**。

这就是 `bridge/mod.rs:172 write_session_instructions()` 必须先写 disk、再 send `Op::SyncSession` 的原因——双保险。

### 4.3 多 session 并发隔离

每个 session 写到独立路径 `~/.pinvou3/sessions/<sid>/instructions.md`，每个 engine 的 `EngineConfig.instructions` 指向自己 session 的文件。engine rehydrate 时各读各的，互不干扰。

测试覆盖：`bridge/mod.rs:865 engine_config_for_session_paths_are_isolated`。

---

## 5. 关键代码地址速查

| 想动什么 | 改哪儿 |
|---|---|
| pinvou3 业务提示词（SKILL 路由、阶段规则、sudo、执行纪律） | `pinvou3-app/src-tauri/resources/bundle/instructions.md` |
| Plan/Yolo per-turn 提醒文案 | `pinvou3-app/src-tauri/src/bridge/mod.rs:530 reminder_for()` |
| Skill 列表（要 AI 看见的） | 往 `~/.pinvou3/bundle/skills/` 加目录，每个带 `SKILL.md` |
| 底座 BASE_PROMPT 本身 | 上游源文件 `DeepSeek-TUI/crates/tui/src/prompts/constitution.md`（v0.9 前为 base.md）；pinvou3 文案经 override 注入，内容在 `pinvou3-app/src-tauri/resources/bundle/base.md`（见 [`base-prompt-override-阶段2.md`](./base-prompt-override-阶段2.md)） |
| 模式 prompt 子段（plan.md/yolo.md/agent.md） | `DeepSeek-TUI/crates/tui/src/prompts/modes/*.md` |
| 13 层拼装逻辑 | `DeepSeek-TUI/crates/tui/src/prompts.rs:673` |
| 实际看某 session 的最终 prompt | 读 `~/.pinvou3/sessions/<sid>/instructions.md`，再对照 `prompts.rs:673` 走一遍拼装 |

---

## 6. 跟 `docs/archived/system-prompt-与底座的差异.md` 的关系

那份文档（2026-05-15，已 archived）讲的是"为什么 pinvou3 不完整复制底座 BASE_PROMPT 的所有引导，只在 instructions.md 里加 5 个 XML 执行纪律标签"——**那个方案 B 的决策依然有效**：

- ✅ 往 `bundle/instructions.md` 加内容，会被底座渲染到 13 层的第 6a 层。
- ✅ pinvou3 不引入 sidebar / `/compact` slash command / agent_open / rlm_open 这些终端 UX 与高级编排的引导（因为它们对 GUI + Qwen3.6 不适用）。

但该文档**第 52 行**的描述基于一个错误前提：

> 入口：`engine.rs:139` → `SystemPrompt::Text(rendered)` 直接塞进 `EngineHandle.session.system_prompt`，完全绕过 `system_prompt_for_mode_*`。

**实际行为**（2026-05-28 复核）：

- 底座 `Engine::new` (`core/engine.rs:458`) 在 spawn 时强制调用 `system_prompt_for_mode_with_context_skills_session_and_approval`，pinvou3 没有也无法绕过。
- pinvou3 的 `SystemPrompt::Text(...)` 调用只出现在 `Op::SyncSession` 路径（切 session 时），而且 `system_prompt_override: false`，rehydrate 时会被 13 层拼装覆盖。
- BASE_PROMPT 的所有内容（268 行英文）、environment、skills、authority、locale 双书签等**全部**仍在 pinvou3 用户的 system prompt 里。

**所以正确的理解是**：pinvou3 没有"用 ~120 行替换 ~400 行"，而是"在底座 ~400 行的第 6a 层（`<instructions>` 槽位）追加自己的 ~120 行中文规则"。

那份文档关于"token 预算"和"~120 行替换"的描述需要按这个事实重新评估，但**方案 B 的工程结论（在 instructions.md 加 XML 执行纪律段）依然是正确做法**——因为该文件就是 pinvou3 注入 prompt 的唯一槽位。

---

## 7. 验证清单

想确认某段文本是不是真的进了 system prompt：

1. 找一个跑着的 session，看 `~/.pinvou3/sessions/<sid>/instructions.md` 内容。
2. 看 `pinvou3-app/src-tauri/src/bridge/mod.rs:184 session_instruction_paths()`，确认这个文件确实在数组里。
3. 看 `DeepSeek-TUI/crates/tui/src/prompts.rs:800-804`，确认 `instructions` 数组里的所有文件都会被 `render_instructions_block` 包成 `<instructions>` 标签拼进 system prompt。
4. （可选）在 engine 入口加临时 `eprintln!("{}", session.system_prompt)` 打印实际值。

不要相信"我以为 pinvou3 绕过了底座"这种二手描述，每次都按上面 1-3 走一遍（参考 `[[reuse_verify_pinvou3_wiring]]`）。
