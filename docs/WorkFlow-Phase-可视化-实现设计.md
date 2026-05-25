# WorkFlow Phase 可视化 · 实现设计（per-session 绑定）

> 愿景/用户旅程见 [`WorkFlow-Phase-可视化-MVP1.md`](./WorkFlow-Phase-可视化-MVP1.md)。
> 本文档记录**已落地的架构**（main `c0273c5` 起，per-session 绑定方案）。
> 文中所有符号/路径可对照代码核实。

## 一句话

「工作流」视图 = **skill 启动器**。点 skill 卡片「启用」→ 新建一个**绑定了该 skill 的独立 session**；该 session 内 chatroom 顶部显示 skill 的 phase chips，随 LLM 输出的 `<phase>` marker 推进。

## 核心设计决策：全局单例 → per-session 绑定

最初是全局 `ActiveSkillStore` 单例（同时只能激活一个 skill，跨 session 串味）。重构为 **per-session 绑定**：

- 每次点「启用」= `start_skill_session` 新建独立 session + 绑定，**可多开、互不干扰**。
- skill 绑定挂在 `SessionModeState` 上，跟 mode/plan_phase 同级、随 session 走。
- 切 session 自动同步 chips 显隐；普通对话 session 不显示 chips。

## 数据模型

`bridge/mode_state.rs`：

```rust
pub struct ActiveSkillBinding {
    pub name: String,
    #[serde(skip)]
    pub pending_instruction: Option<String>, // skill 触发提示，首条 chat prepend 后置空（一次性）
    pub phases: Vec<deepseek_tui::skills::PhaseDef>, // 前端渲染 chips 用
}
// 挂在：
pub struct SessionModeState { mode, plan_phase, pinvou_review_enabled, active_skill: Option<ActiveSkillBinding> }
```

**in-memory only，不持久化**（跟 mode_state 一致）：重启 app 后 session 仍可对话，但 chips 不再驱动——这是合理取舍，用户可重新点卡片新建绑定 session。`PhaseDef` 上游只 derive `Serialize`，故只单向序列化给前端。

## 后端命令（`commands.rs`，注册见 `lib.rs`）

| 命令 | 职责 |
|---|---|
| `start_skill_session(name)` | 「启用」入口：`create_new` 新 session → `set_active` → `sync_session`（切引擎到空 session，避免续到旧上下文）→ `bind_skill`。返回新 session 元数据 + skill 信息 |
| `get_session_active_skill(sid)` | 切 session 后拉绑定信息渲染 chips（无绑定返 None） |
| `unbind_session_skill(sid)` | 点 chips 区 ✕：清绑定，不删 session（chips 隐藏，转普通对话） |
| `list_session_skill_bindings()` | `{session_id: skill_name}` 映射，给左侧历史列表叠 🧭 标签 |

`SessionStore`（`bridge/sessions.rs`）配套：`bind_skill` / `active_skill` / `take_pending_skill_instruction`（一次性消费 prepend）/ `unbind_skill`。

## 端到端数据流

```
工作流视图 → list_skills_v2() → 渲染 skill 卡片（每张是一个"启动入口"）
点「启用」  → start_skill_session(name) → 后端建 session+bind → 前端切到该 session + 渲染 chips
切 session  → get_session_active_skill(sid) → 同步 chips strip 显隐
LLM 输出 <phase id="../> → 底座 engine 抽 marker emit Event::PhaseChanged
            → Tauri event chat:phase_changed → setCurrentPhase(pN, "llm")
用户点 chip → setCurrentPhase(pN, "user") + 发 [USER_NUDGE] 伪消息引导 LLM
用户点 ✕    → unbind_session_skill(sid) → 解绑，chips 隐藏，session 仍可用
```

## 复用底座（不重复造轮子）

- **SKILL.md `phases:` 字段** + `PhaseDef` 解析 + `DemoInfo`：底座 `SkillRegistry`。
- **`<phase id="..."/>` marker 抽取 → `Event::PhaseChanged`**：底座 `turn_loop` 自动从 visible text 抽，前端不需要正则解析文本。
- **`strip_phase_marker_delta`**：底座把 marker 从 `chat:delta` 内容剥掉再发，聊天区无 DOM 污染。

pinvou3 只加了：per-session 绑定状态 + 4 个命令 + 工作流视图/chips 前端 + 每 turn phase reminder。

## 关键约束 / 边界

- **`WORKFLOW_HIDDEN_SKILLS`**（`commands.rs`）= `["pinvou-review-plan", "pinvou-review-final"]`：品悟 review 是系统基础能力，不作为工作流入口暴露。当前软隔离（skiplist），将来物理隔离需上游支持多 `skills_dir`。
- **skill 来源单一**：`list_skills_v2` 只扫 `~/.pinvou3/bundle/skills`（之前是 user/deepseek/bundle 三源合并，已收窄）。
- **每 turn 重申 phase marker 约束**（`commands.rs` chat）：实测 Qwen3.6 长上下文里对顶端 system prompt 的 MANDATORY marker 段遵循率衰减（h3c-ppt 跑到 p5+ 频繁漏标），故每个 user turn 注入一条 `<system-reminder>` 把约束搬到离 LLM 最近处。
- **phase 推进双向**：LLM marker 驱动（主）+ 用户点 chip 手动跳（发 USER_NUDGE 引导 LLM 跟随）。

## 与 MVP1 主分支版的差异（历史）

MVP1 主分支版（`54825bd`/`133ee3f`）chips 在工作流视图内 + 工具调用启发式 phase 推断；本 per-session 版 chips 在 chatroom 顶部 + 显式启用 + 绑定 session。本版已并入 main，是当前实现。
