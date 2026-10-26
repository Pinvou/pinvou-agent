# pinvou3 System Prompt 架构

> 创建：2026-05-28
> 范围：从 session 创建到首次发给 LLM，system prompt 是怎么被构造出来的
> 目的：让任何想动 prompt 行为的人能定位到正确的修改点
>
> ⚠️ **2026-07-17 复核**：文中「per-session instructions 写 disk
> （`~/.pinvou3/sessions/<sid>/instructions.md`）」的描述（§1 时序图、§3 骨架、§4 rehydrate
> 语义）已过期。现行为 C 方案 P-no-disk：instructions 走
> `InstructionSource::Inline` 内存注入（`features/assistant/platform/bridge.rs`
> `session_instructions()`），不再写
> disk；`sync_session` 传 `system_prompt: None`（`src-tauri/src/features/assistant/engine.rs`）；`{{PINVOU3_WORKSPACE}}`
> 占位符已移除，workspace 改走 per-turn `<turn_meta>` 注入；用户自定义 instructions 现路径为
> `~/.pinvou3/user/instructions.md`（旧 `.deepseek`/`pinvou3` 盘上 instructions 残留会在 boot
> 时被清理，见 `features/assistant/platform/bridge.rs`
> `cleanup_legacy_pinvou3_disk_files()`）。第 6a 层
> `<instructions>` 注入点的结论不受影响；但**13 层拼装结构本身的结论已过期**：上游
> 2026-08-09 移除 mode 教义（mode 真相移至 per-turn reminder），2026-09-07 起 host 经
> 静态层 composer 接管全部编译期静态文案——两者均晚于本复核时点。§0–§5 保留作
> 2026-05-28 时点记录，**当前树的实际结构见 §0b**。
> 另：各 `file.rs:NNN` 行号随版本漂移，定位以符号名为准；文中
> `src-tauri/src/bridge/mod.rs`、`src-tauri/src/engine.rs` 等旧路径已随
> `features/` 重组迁移到 `features/assistant/` 下，`bundle/instructions.md`
> 已拆分为 `base.md` + `instructions-shared/work/code.md` 等分模式文件，
> 底座 prompts 常量已从 `prompts/*.md` 合并进 `CodeWhale/crates/tui/src/prompts/text.rs`。

---

## 0. TL;DR

> （2026-05-28 时点记录。composer 接管后，本节"pinvou3 只动两个地方"与"其它 12 层
> 不直接改文案"的说法均不再成立——当前树的实际结构见 §0b。）

- 底座 CodeWhale 在 `Engine::new` 里**自动跑 13 层 prompt 拼装**，pinvou3 没有绕过这个流程。
- pinvou3 只动两个地方：
  - **第 6a 层 `<instructions>`**：通过 `EngineConfig.instructions` 把 `bundle/` 分模式
    instructions（`instructions-shared.md` / `instructions-work.md` 及其绑定目录变体
    `instructions-work-bound.md` / `instructions-code.md`，底座文案为 `base.md`）渲染版塞进去。
  - **Per-turn `<system-reminder>`**：贴在每次 user message 头部，按 `(mode, phase)` 选段。
- 其它 12 层（BASE_PROMPT / mode_prompt / project_context / environment / skills / handoff / authority / locale 等）全是底座管，pinvou3 通过 `EngineConfig` 字段（`workspace` / `skills_dir` / `locale_tag` / `mode`）影响渲染参数，不直接改文案。
- 多 session 并发隔离的关键：每个 session 的 instructions 以带 session id 的
  `InstructionSource::Inline` 注入，互不串台（参考 [`多session并发-设计实现.md`](./多session并发-设计实现.md)）。

---

## 0b. 当前树实际结构（2026-10 复核）

定位以符号名为准；以下每一条都可在当前树按符号 grep 验证。

**入口不变**：`Engine::new` 仍强制调用拼装函数（现名
`system_prompt_for_mode_with_context_skills_session_and_approval_for_host`，
`CodeWhale/crates/tui/src/prompts.rs`），pinvou3 没有也无法绕过。变化的是拼装的
**内容来源**：

1. **静态层被 host composer 全量接管**。Bridge boot 调用
   `install_prompt_overrides()`（`features/runtime_bundle/platform/mod.rs`，由
   `features/assistant/platform/bridge.rs` 的 boot 路径触发；`OnceLock` 首设生效、
   幂等，必须早于任何 engine spawn）：
   - `set_base_prompt_override`：base 槽被 `BASE_PROMPT_MD` 占位——纯注释空壳，正文
     永不进 prompt，仅用于压制 bundled-headless 的 compact 宪法分支；
   - locale 前奏/收尾换成 pinvou3 瘦身版（zh-Hans/ja；产品决策不教"用某语言思考"）；
   - **`set_static_prompt_composer_override(compose_static_layers)`**：底座
     Personality/Mode/Approval/ContextMgmt/COMPACT_TEMPLATE/taxonomy 常量全部不再进
     prompt。composer 只输出 `MODE_EXECUTE_MD`（"## Mode: Execute"），不按
     `ctx.mode` 选块——底座 v0.8.57 起 mode/approval 移到 per-turn，Plan/Agent 的
     真相全在 per-turn reminder，静态层恒为 Execute 块。
2. **静态（可缓存）前缀的实际内容** = 瘦身 locale 前奏 + `## Mode: Execute` +
   skills 块（只列名字+一句描述+SKILL.md 路径，仍属稳定前缀）。project context 块
   在 composer 生效时**不再追加**（`load_project_context_with_parents` 的结果只用
   于无 composer 分支）；底座 `## Core Execution`（`CORE_EXECUTION_PROFILE_PROMPT`）
   在 composer 生效时跳过；`COMPACT_TEMPLATE` 不进任何新会话（自动压缩走
   `canonical_prompt()` 代码拼装，手动压缩走 `create_summary()` 独立 LLM 调用）。
3. **Volatile 半 = WorldState fragments**（`WorldStateSnapshot` →
   `to_system_blocks`）：
   - Workspace fragment：**environment 块挪到了 volatile 半**（外加 memory 块与
     `MEMORY_GUIDANCE`、goal，均按需出现）；
   - Permissions fragment：pinvou3 instructions（`render_instructions_block`，
     `<instructions source="pinvou3:instructions">`——label 固定，不再带 session
     id，以免破坏跨会话 prefix cache）；
   - Route fragment：verbosity/translation 姿态；AgentTopology/SkillsTools 留空
     （skills 留在稳定前缀，AgentTopology 由运行时调用方更新）；
   - TokenBudget fragment：handoff——pinvou3 无 `.codewhale/handoff.md` 写入通路，
     恒为 None；
   - ProjectInstructions fragment（`.cursorrules` 等）：composer 生效时不加载。
4. **Trailer**：`AUTHORITY_RECAP` 在 composer 生效时跳过（其指向的宪法小节不再保证
   存在，追加会留悬空交叉引用）；locale 收尾用 pinvou3 瘦身 override。
5. **Per-turn `<system-reminder>`**：PlanPhase 删除后只剩 mode 维度——Plan 的提醒由
   session policy 生成，Yolo 的大产物分块提醒已裁撤（只余动态 sudo 状态），Agent
   未对 pinvou3 暴露；选取逻辑在 `build_send_message_op` 发送链内
   （`features/assistant/platform/bridge.rs`）。
6. **instructions 内容来源不变**：`session_instructions()` 把 `bundle/` 分模式文件
   渲染成 `InstructionSource::Inline` 注入 `EngineConfig.instructions`（按会话隔
   离；`sync_session` 传 `system_prompt: None`，rehydrate 重新拼装）。
7. **Skills**：bundle 启动解包到 `~/.pinvou3/bundle/skills`
   （`paths::bundle_skills_dir()`），EnginePool 每会话把启用技能物化到
   `~/.pinvou3/sessions/<sid>/skills`（`features/assistant/skill_materialization`），
   引擎的发现根是物化目录（`explicit_skills_root_only`），不是 bundle 目录本身。

**对"想动 prompt 文案"的含义**：静态英文文案改 `compose_static_layers` /
`MODE_EXECUTE_MD`（改 `bundle/base.md` 无效果——空壳；改底座常量也进不了 prompt）；
中文/业务文案改 `bundle/` 分模式 instructions（第 6a 层注入点未变）；per-turn 文案
改 `build_send_message_op` 链。

---

## 1. Session 启动时序

```
用户 GUI 点新会话
  ↓
pinvou3-app/src-tauri/src/features/assistant/engine.rs 的 spawn_for_session()
  │
  ├─ bridge.write_session_instructions(sid)                  # 2026-05-28 时点在 bridge/mod.rs；现行为 Inline 注入，不再写盘
  │    └─ INSTRUCTIONS_MD                                    # bundle/instructions.md 编译期 include（时点记录；现拆分为分模式文件）
  │         .replace("{{PINVOU3_WORKSPACE}}", ws)
  │         .replace("{{PINVOU3_SUDO_INSTRUCTION}}", ...)
  │       → 写到 ~/.pinvou3/sessions/<sid>/instructions.md
  │
  ├─ bridge.build_engine_config_for_session(sid)             # features/assistant/platform/bridge.rs
  │    workspace    = ~/.pinvou3/sessions/<sid>/workspace
  │    instructions = [上面那个文件, ~/.deepseek/instructions.md(可选)]
  │    skills_dir   = ~/.pinvou3/bundle/skills                # 时点记录；现发现根为每会话物化目录，见 §0b
  │    locale_tag   = "zh-Hans"
  │
  └─ spawn_engine(engine_config, dt_config)
       └─ Engine::new(...)                                   # CodeWhale core/engine.rs
            └─ prompts::system_prompt_for_mode_with_context_skills_session_and_approval(
                  AppMode::Agent,
                  &config.workspace,
                  None,
                  Some(&config.skills_dir),
                  Some(&config.instructions),                ← pinvou3 注入点
                  PromptSessionContext { locale_tag, ... },
                  session.approval_mode,
              )                                              # core/engine.rs（时点行号，见复核说明）
            → session.system_prompt = SystemPrompt::Text(13 层拼装结果)
```

**关键**：`Engine::new` 行 458 强制走 13 层拼装函数，**pinvou3 无法跳过**。所有改 prompt 文案的诉求要么通过 `EngineConfig` 字段影响，要么往 `instructions.md` 加内容。

---

## 2. 13 层拼装顺序

> （2026-05-28 时点记录：层结构与行号均已过期——上游 2026-08-09 移除 mode 子段、
> 2026-09-07 起 host composer 接管静态层；当前结构见 §0b。）

入口：`CodeWhale/crates/tui/src/prompts.rs:673` 的 `system_prompt_for_mode_with_context_skills_session_and_approval()`。

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

**文件**：`pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs` `build_send_message_op()`

```rust
let full_content = match reminder_for(mode, phase) {
    Some(r) => format!("<system-reminder>\n{r}\n</system-reminder>\n\n{content}"),
    None    => content,
};
```

提醒文本的选取逻辑在 `build_send_message_op` 发送链内（2026-05-28 时点为独立的
`bridge/mod.rs:530 reminder_for()` hardcode 表，现已并入发送构造；定位以
`build_send_message_op` 与 `<system-reminder>` 信封为准）：

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

`pinvou3-app/src-tauri/src/features/assistant/engine.rs` `sync_session()` 会发：

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

`CodeWhale/crates/tui/src/core/engine.rs:1878 refresh_system_prompt()` 会**重新调用 13 层拼装函数**，参数照样从 `EngineConfig.instructions` 读 disk 文件。

**所以**：sync_session 传进去的 `SystemPrompt::Text` 拼装结果会被覆盖。**真正能让 AI 看到 session-specific 内容的，是改 disk 上的 instructions.md 文件本身**。

这就是 `bridge/mod.rs:172 write_session_instructions()` 必须先写 disk、再 send `Op::SyncSession` 的原因——双保险。

### 4.3 多 session 并发隔离

每个 session 的 `EngineConfig.instructions` 都包含独立的 `InstructionSource::Inline`；workspace 也按 session 分目录。Inline 的 `name` 是固定的 `pinvou3:instructions`——session id 曾用于 label、现已移除，以免破坏跨会话 prefix cache（见 `session_instructions()` 的注释）；隔离靠每会话独立的 `EngineConfig.instructions`，不靠 label。engine rehydrate 复用各自内存中的配置，不依赖 session 目录里的 instructions 文件。

测试覆盖：`features/assistant/platform/bridge.rs` 的 `session_instructions_*` 系列测试（inline 注入与按会话隔离）。

---

## 5. 关键代码地址速查

| 想动什么 | 改哪儿 |
|---|---|
| pinvou3 业务提示词（SKILL 路由、阶段规则、sudo、执行纪律） | `pinvou3-app/src-tauri/resources/common/bundle/` 分模式 instructions（`instructions-shared.md` / `instructions-work.md` 及 `instructions-work-bound.md` / `instructions-code.md`） |
| Plan per-turn 提醒文案 | `pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs` `build_send_message_op()`（发送链内的提醒选取；仅剩 mode 维度，Plan 由 session policy 生成） |
| Skill 列表（要 AI 看见的） | 内置技能改 `pinvou3-app/src-tauri/resources/common/bundle/`（启动解包到 `~/.pinvou3/bundle/skills/`，EnginePool 每会话物化到 `~/.pinvou3/sessions/<sid>/skills/` 供引擎发现，见 `features/assistant/skill_materialization`），每个带 `SKILL.md` |
| 底座静态文案（constitution/BASE_PROMPT/mode/approval/taxonomy 等） | pinvou3 下全部经 `features/runtime_bundle/platform/mod.rs` `compose_static_layers()` 接管（当前只输出 `MODE_EXECUTE_MD` "## Mode: Execute"）；改底座常量（`CodeWhale/crates/tui/src/prompts/text.rs`）或 `bundle/base.md`（纯注释空壳）都进不了 prompt |
| 模式 prompt 子段（plan/yolo/agent） | 已不存在：上游 2026-08-09 移除 mode 教义，静态层恒为 Execute 块，mode 真相在 per-turn reminder（见 §0b） |
| 拼装逻辑（原"13 层"） | `CodeWhale/crates/tui/src/prompts.rs` `system_prompt_for_mode_with_context_skills_session_and_approval_for_host()` |
| 查看 pinvou3 实际最终 prompt | `cargo run --manifest-path pinvou3-app/src-tauri/Cargo.toml --bin dump_system_prompt`（需 `dev-tools` feature） |

---

## 6. 跟开源前评审文档「system-prompt 与底座的差异」的关系

那份文档（2026-05-15，closed 时期 archived，未随开源发布）讲的是"为什么 pinvou3 不完整复制底座 BASE_PROMPT 的所有引导，只在 instructions.md 里加 5 个 XML 执行纪律标签"——**那个方案 B 的决策依然有效**：

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

1. 运行 `cargo run --manifest-path pinvou3-app/src-tauri/Cargo.toml --bin dump_system_prompt > /tmp/pinvou3_system_prompt.txt`。
2. 查看 `features/assistant/platform/bridge.rs` 的 `session_instructions()`，确认 pinvou3 指令以 `InstructionSource::Inline` 进入 `EngineConfig.instructions`。
3. 查看底座 `prompts.rs` 的 `system_prompt_for_mode_with_context_skills_session_and_approval_for_host()` 与 `render_instructions_block()`，确认 inline/file instructions 都进入 `<instructions>` 块。
4. 在 dump 结果中搜索待验证文本；涉及 per-turn `<runtime_prompt>` / `<turn_meta>` 的内容不属于静态 system prompt，需在真实 turn 事件或对应测试中验证。

不要相信"我以为 pinvou3 绕过了底座"这种二手描述，每次都沿实际构造链验证。

