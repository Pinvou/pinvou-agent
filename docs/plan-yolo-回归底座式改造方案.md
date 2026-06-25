# Plan/Yolo 回归底座式 改造方案

> 状态：待评审（2026-06-25）
> 目标：砍掉 app 自建的 `PlanPhase` 四态生命周期，回归底座原生的 Plan/Yolo 二态 + 底座 plan 交接闭环；模式切换入口做成 composer（信息输入栏）chip；plan 决策保留卡片形态。

## 0. 背景与判断

底座（DeepSeek-TUI）**本来就有**完整的 plan→执行交接闭环：

1. 模型在 Plan 模式调 `update_plan` = 交接信号（`prompts/modes/plan.md` 引导）
2. engine 层 `should_stop_after_plan_tool`（`dispatch.rs:385`）强制结束本 turn
3. （TUI）自动弹 Plan Confirmation modal → 四选项 Accept(Agent)/Accept(YOLO)/Revise/Exit
4. Accept(YOLO) → 自动 `set_mode(Yolo)` + 注入 `"Proceed with the accepted plan."` + 共享 `Arc<Mutex<PlanState>>` 不清

pinvou3 是 GUI，用不了 TUI 的 ratatui 弹窗，所以**确认 UI 必须是卡片**——这部分 pinvou3 已经有（`chat:plan_ready` 事件 → PlanCard → `accept_plan`）。pinvou3 真正多包的是 `Planning/Ready/Executing` 四态 enum + M2 自驱 + M3 兜底。

**改造 = 减法**：剥掉 phase 状态机，让卡片直接挂底座的 `update_plan` 交接信号，符合 CLAUDE.md 约束1（不重复造轮子）。

## 1. 目标形态

```
┌─ mode 维度（复用底座）──────────────────────────────┐
│ Plan ⇄ Yolo 二态。切换入口 = composer chip（单击 toggle）│
│ Plan: 底座 ReadOnly sandbox + 只读工具集（不注册写工具）  │
│ Yolo: 底座 DangerFullAccess                          │
└────────────────────────────────────────────────────┘
        │ 进 Plan，AI 调研，调 update_plan
        ▼
┌─ 交接（复用底座 engine 层）─────────────────────────┐
│ should_stop_after_plan_tool 停 turn（底座，mode=Plan 自动生效）│
│ engine forwarder 检测 plan_used → emit chat:plan_ready│
└────────────────────────────────────────────────────┘
        │ 弹 PlanCard（GUI，承接底座 confirmation 语义）
        ▼
┌─ 决策（app 卡片）──────────────────────────────────┐
│ ✅就这么干 → accept_plan: set_mode(Yolo) + 注入执行指令 │
│ ✏️改改   → prefill "修订方案:"（不 invoke）           │
│ 🚪算了   → discard_plan: set_mode(Yolo) + 清 pending 卡│
└────────────────────────────────────────────────────┘
```

驱动状态从「显式 phase enum」改为「`mode(Plan/Yolo)` + 有无 pending plan 卡」。phase 维度全删。

## 2. 关键设计决策

| # | 决策 | 取舍 |
|---|---|---|
| D1 | **chip 带下拉**（默认 Yolo，Plan 手切） | 照 `ComposerKbSelector`（`index.html:3219`）下拉形态：chip 显示当前 mode，点开下拉选 Yolo/Plan（各带一句说明）。比单击 toggle 更易懂两个模式各是什么 |
| D2 | **不暴露 Agent** | 沿用现状（底座三态取其二）。Accept 只有一个目标=Yolo，卡片 3 选项不变 |
| D3 | **accept 保留执行指令注入** | 对齐底座 `"Proceed with the accepted plan."`，切 Yolo 后 AI 靠注入指令 + 共享 PlanState 续做 |
| D4 | **砍 M2 自驱续跑** ⚠️行为变化 | 回到底座行为：执行一个 turn 后停下，用户决定是否继续。原 M2 会自动发"继续执行"循环——砍掉 |
| D5 | **M3 收口兜底重做**（放弃旧判据） | 旧 `text_len>300` 判据是拍的、无测试支撑，放弃。重做依据"现有测试情况"评估：现状 plan 测试全是 L1 真 vLLM 端到端（`plan_ablation`/`plan_write_urge`，`#[ignore]`，靠人看收口率），M3 兜底卡逻辑**零单测覆盖**。阶段3 重做 = 先定可靠的"收口失败检测信号"（不靠字数）+ 配套建测试，触发降到 mode 维度（`mode==Plan && !plan_used`） |
| D6 | **每轮 mode reminder 保留** | 不回退底座"指针+回查"（对 Qwen3.6 弱）。`reminder_for` 从 4 段(mode,phase)降到 2 段(mode)：Plan 一段 / Yolo 一段 |

## 3. 分阶段实施

每阶段独立可验证、可单独 commit。

### 阶段 1：composer mode chip（纯前端，风险前置）

**为什么先做**：Plan 模式在 pinvou3 GUI **从未真跑过**（入口一直注释下线）。先用现有后端（PlanPhase 仍在）把 chip 接通，端到端验证「底座 Plan 交接在 GUI 能跑」这个最大未知，再做减法。若此阶段暴露底座交接在 GUI 的问题，整个方案需重评——越早越好。

**动的文件**：`src/index.html`（+ i18n），`src/tauri-bridge.js` 零改动（函数已导出）。

- 新建 `ComposerModeChip` 组件，照 `ComposerKbSelector`（`index.html:3219-3287`）形态：读 `bs.modeState`，chip 样式用 `rounded-xl px-3 py-1.5 border` 那套，图标 `Zap`，激活态(plan)蓝 pill
- 切换逻辑搬 `ModeHeader.toggleMode`（`index.html:5477-5486`）：busy 先 `bridge.cancelGeneration()`，再 `exitPlanToYolo()` / `setPlanModeNext()`
- 挂载：`index.html:3522` 后插 `<ComposerModeChip t={t} bs={bs} />`
- i18n：补三语 key（参考 `kbMount` 体例 `index.html:217/430/643`），替换 ModeHeader 的硬编码中文
- `ModeHeader` 组件保留但不挂（或删，阶段4 收尾）

**验证**：
- headless UI 冒烟（`headless_ui_smoke_check`）：chip 渲染、点击 Yolo⇄Plan、`modeState` 刷新、激活态样式
- 真机端到端（需 vLLM）：进 Plan → 发需求 → AI 调 update_plan → 卡片弹出 → ✅ → 切 Yolo 执行。确认底座 `should_stop_after_plan_tool` + ReadOnly 在 GUI 生效

### 阶段 2：后端砍 PlanPhase（一次内聚减法）

**为什么一次性**：`phase` 参数贯穿 `send_user_message → build_send_message_op → reminder_for` 和多个命令，去参数必须一起改才编译通过。内部按文件分组，末尾统一验证。

**动的文件**：`mode_state.rs`、`sessions.rs`、`commands.rs`、`engine.rs`、`engine_pool.rs`、`bridge/mod.rs`（详见 §4 清单）

核心改动：
- `PlanPhase` enum + `plan_phase` 字段 + 所有 import 删除
- `set_plan_mode_next`→`set_mode(Plan)`、`exit_plan_to_yolo`/`discard_plan`→`set_mode(Yolo)`、`accept_plan`→`set_mode(Yolo)`+保留执行指令注入
- `set_mode`（`sessions.rs:206`）删 phase 自动转移，成为主力 setter；删 `set_plan_phase`
- `send_user_message` / `build_send_message_op` / `reminder_for` 去 phase 参数

**验证**：`cargo check` + `cargo test`（含改后的 mode_state/sessions/mod 测试）

### 阶段 3：engine.rs plan 逻辑收敛

**动的文件**：`engine.rs`

- **改并留** plan_ready 检测（`879-898`）：去 phase 条件 → `plan_used && mode==Plan` 即 emit `chat:plan_ready`，删 `set_plan_phase(Ready)`
- **删** M2 自驱续跑整块（`944-990`）+ `plan_has_pending`（`250-265`）+ auto-continue 计数器（`sessions.rs:411-433`）
- **改造** M3（`992-1008`）：`mode==Plan && !plan_used && text_len>阈值` → emit `plan_text_fallback`（收口兜底，D5）
- `TurnPlanTracker` 删 `write_tool_used`/`assistant_text_len` 字段（仅 M2/M3 旧逻辑用），留 `plan_tool_used`+snapshots

**验证**：`cargo check` + `cargo test`；真机验证 plan_ready 仍弹卡、M3 兜底卡在「写了方案没调工具」时出现

### 阶段 4：reminder 降维 + 前端联动清理 + 回归

**动的文件**：`bridge/mod.rs`、`src/tauri-bridge.js`、`src/index.html`

- `reminder_for(mode)`：删 Ready/Executing 分支，留 Plan 一段（去"Planning 阶段"措辞）+ Yolo 大产物分块段
- 删消融脚手架：`PLAN_PLANNING_NO_COLLECT`/`NO_BLOCK` 常量 + `plan_planning_reminder` 的 `PINVOU3_ABLATION` env 分发 + `ablation-clean` worktree
- 前端清理：`chat:execution_stuck` 监听（M2 删后不再发）、`chat:plan_text_fallback` 保留（M3 改造后仍发）；切会话 `wasExecuting`（`tauri-bridge.js:1341`）重定义为「有 pending plan 卡」；`ModeHeader` 组件删除
- `PlanStuckCard`（execution_stuck 分支）按 D4 处理

**验证**：headless 冒烟 + 真机全链回归（进 Plan→方案→accept→执行；进 Plan→方案→discard；进 Plan→写长文本不调工具→兜底卡）

## 4. 删除 / 修改清单（精确行号）

### 删除
- `mode_state.rs:34-53` PlanPhase enum+Default；`:83` 字段；`:103` Default 初始化
- `engine_pool.rs:31`、`bridge/mod.rs:35` PlanPhase import
- `bridge/mod.rs:870-872` 消融注释；`:886-892` NO_COLLECT；`:894-903` NO_BLOCK
- `engine.rs:889` set_plan_phase(Ready) 调用；`:944-990` M2 整块；`:250-265` plan_has_pending；`:226` write_tool_used 字段；`:229` assistant_text_len 字段；`:464`/`:533` 两个写点
- `sessions.rs:219-224` set_plan_phase；`:48,65,411-433` auto_continue 计数器；`:214-216` set_mode 里 phase 转移
- `commands.rs:96-97` reset_auto_continue

### 修改
- import 去 PlanPhase：`engine.rs:28`、`commands.rs:22`、`sessions.rs:31`
- `bridge/mod.rs:800-806,823` build_send_message_op 去 phase 参数 + `reminder_for(mode)`；`:905-951` reminder_for 降维；`:873-884` PLAN_PLANNING_FULL 去 Planning 措辞
- `engine.rs:112-124` send_user_message 去 phase；`:863-872`+`:213-230` TurnPlanTracker 去两字段；`:879-898` plan_ready 去 phase 条件；`:992-1008` M3 改 mode 触发
- `engine_pool.rs:178-197` send_user_message 去 phase
- `commands.rs:93-100` chat 去 phase 取值/传参；`:1855-1862`/`:1866-1873`/`:1880-1899`/`:1969-1976` 四命令改 set_mode
- `sessions.rs:206-217` set_mode 删 phase 转移；`:226-234` set_mode_state 去 phase 参数（保留不覆盖 pinvou_review 语义）
- 测试：`mode_state.rs:135-168`、`sessions.rs:553-573,619-651`、`mod.rs:1019-1065,1362-1399,1422-1439` 去 phase 实参/断言

### 前端联动（阶段1+4）
- 新增：`index.html` ComposerModeChip 组件 + 挂载 + i18n
- 清理：`tauri-bridge.js:1446-1450`(execution_stuck 监听)、`:1341`(wasExecuting 重定义)；`index.html:5469-5499`(ModeHeader 删)、`:3736-3737`(stuck 卡渲染按 D4)
- 保留：plan_ready 监听/PlanCard/accept_plan 链（`tauri-bridge.js:1410-1426`、`index.html:5244-5269`）；plan_text_fallback（M3 改造后仍用）

## 5. 风险与回归点

1. **Plan 在 GUI 首次真跑**（阶段1 前置）：底座停 turn/ReadOnly 端到端未验证过，当新功能测
2. **收口 10% 天花板**（D5 应对）：底座式仍依赖模型调 update_plan，~10% 会在 text 写方案——M3 兜底卡是这条防线，别一起砍
3. **砍 M2 是行为变化**（D4）：大任务执行不再自动续跑，需用户驱动；确认产品可接受
4. **去 phase 参数贯穿性**：阶段2 漏一个引用点就编译失败，按 §4 清单逐项核

## 6. 已确认决策（2026-06-25）

1. ✅ **砍 M2 自驱**（D4）：执行不自动续跑、回到底座由用户驱动——可接受。
2. ✅ **chip 带下拉**（D1）：默认 Yolo，Plan 需手切。下拉两项各带说明。
3. ✅ **M3 重做**（D5）：放弃旧 `text_len>300` 判据，依据现有测试情况重评——现状 M3 零单测、plan 测试全是 L1 端到端。阶段3 重做时先定可靠收口失败信号 + 建测试。
4. ✅ **阶段顺序**：先接 chip 验证端到端，再减法。

### 遗留物处理提醒
- `pinvou3-ablation`（`ablation-clean` 分支）worktree + `pinvou3-mode-select` worktree 里未提交的 `l1_dialog_harness.rs`（`plan_ablation`/`plan_write_urge` 消融用例，用 `PlanPhase::Planning`）= 旧消融脚手架。阶段4 删消融时一并清理（这些测试随 PlanPhase 删除会编译失败，要同步处理）。
