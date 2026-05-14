# Plan / YOLO 双模式 — 设计决策

> 创建：2026-05-14
> 状态：V1 已实施（commit 待定）；**V2 弱模型加固设计已拍板，待实施**（见末尾 §12-13）
> 适用阶段：阶段 D（紧接 C 之后）

---

## 1. 上下文

pinvou3 目前 `build_engine_config()` 写死 `trust_mode = true`，加上敏感目录 hook 兜底，实质是「YOLO 限于家目录非敏感区」的杂交态。准备引入显式模式切换，**让用户在需要谨慎的场景下能让 AI 先出方案再动手**。

### 关键判断：不照搬底座三 mode

底座（`DeepSeek-TUI/docs/MODES.md`）有 Plan / Agent / YOLO 三 mode，是 coder 工具范式。pinvou3 面向**普通 Ubuntu 工作用户**（写文档、旅游计划、PDF 总结、表格整理、邮件润色），他们的心智是「指令 → 结果」，**Agent mode 每次写文件弹 approval 是噪音**。

| 维度 | Coder（Claude Code 范式） | pinvou3 普通用户 |
|---|---|---|
| 期望交互 | review plan → approve → execute | 指令 → 结果 |
| 对 approval 弹窗 | 是把关，正常 | "AI 不是该自己搞定？怎么这么烦" |
| 错误代价 | 高（破坏 codebase） | 中（文本任务重生成即可） |
| Mode 三选一 | 接受 | "Plan/Agent/YOLO 我该选啥？" |

**简化为 2 mode**：
- **YOLO** = 默认 home 态，「直接干」
- **Plan** = 临时进入的子流程，「先讨论再动手」

砍掉 Agent。

---

## 2. 核心设计决策

| # | 决策 | 备选已淘汰 |
|---|---|---|
| 1 | **双模式**：YOLO（默认）+ Plan（临时子流程） | 三 mode 全保留 / 单 mode 不切换 |
| 2 | **Plan 是会话级状态**（不是消息级标签），有明确入口和**自动退出** | 每条消息独立标签（多轮 Plan 流程无法表达） |
| 3 | **进入 Plan**：composer 旁 [💡] 按钮 + 发消息 | 顶栏持久 toggle / slash 命令 |
| 4 | **Plan 内的 3 个出口**：Plan Ready 卡片三选项 + Planning 态 chip ⚡ 直接动手 | 只有 ✅/🚪 两选项（不够灵活） |
| 5 | **Plan Ready 用气泡内嵌卡片**，不是弹窗 | 弹窗（抢焦点、无历史、移动端差） |
| 6 | **AI 反向触发 Plan**：不做 | 任务复杂时 AI 主动切 Plan（增加复杂度，普通用户不期待） |
| 7 | **mode 通过 `Op::SendMessage { mode }` 注入**（底座原生支持，逐消息携带） | 独立 ChangeMode Op（底座没有） |
| 8 | **底座 `PlanPromptView` 模态**不用，bridge 自实现卡片渲染 | 复刻 TUI 模态（不符合 GUI 心智） |
| 9 | **trust_mode 解耦**：Plan 时 false，YOLO 时 true；敏感目录 hook 永远生效 | 一直 true / 一直 false |
| 10 | **执行 plan 时切回 YOLO** + `plan_executing` 子标记 | 单独的 Executing mode（多此一举） |

---

## 3. 状态机

```
┌─────────────────────────────────────────────────────────────┐
│                                                              │
│   [YOLO 默认]  ←──────────────────────────────────────┐     │
│        │                                                │     │
│        │ 用户点 💡 + 发消息                             │     │
│        ↓                                                │     │
│   [Plan: Planning] ───────────────────────────┐         │     │
│        │                                       │         │     │
│        │ AI 调过 update_plan + turn 结束       │ 用户点  │     │
│        │                                       │ [⚡ 直接 │     │
│        ↓                                       │  动手]  │     │
│   [Plan: Ready]                                │         │     │
│        │ (气泡卡片三选项)                       │         │     │
│        ├─ ✅ 就这么干 → [YOLO 执行 plan] ──────┼─────────┤     │
│        ├─ ✏️ 改改 → 回 [Planning]              │         │     │
│        └─ 🚪 算了 ────────────────────────────┴─────────┤     │
│                                                          │     │
│                              执行 turn 结束 ─────────────┘     │
└─────────────────────────────────────────────────────────────┘
```

**5 个状态转移触发点**：

| 触发 | 转移 |
|---|---|
| 用户点 💡 + 发消息 | `mode = Plan`, `plan_phase = Planning` |
| Planning 态用户点 chip [⚡ 直接动手] | `mode = Yolo`, `plan_phase = None`（对话历史保留作 context） |
| AI turn 结束 + `plan_tool_used_in_turn` + plan 非空 | `plan_phase = Ready`，emit `plan_card` 消息 |
| Ready 卡片选 ✅ 就这么干 | `mode = Yolo`, `plan_phase = Executing`，重发 plan markdown 作为执行指令 |
| Ready 卡片选 ✏️ 改改 | `plan_phase = Planning`，下条用户消息走 Plan 修订 |
| Ready 卡片选 🚪 算了 / Executing 完成 | `plan_phase = None`, `mode = Yolo` |

`plan_tool_used_in_turn` 判据从底座抄：`crates/tui/src/tui/ui.rs:1072-1085` 同款条件。

---

## 4. 复用 vs 自建边界（**遵循 CLAUDE.md 约束**）

### 4.1 底座 DeepSeek-TUI 已有，直接复用，**不动**

| 能力 | 底座位置 | 复用方式 |
|---|---|---|
| `AppMode` enum (Plan/Agent/Yolo) | `tui/app.rs:90-94` | 直接当协议字段类型用 |
| Plan mode 工具白名单切换 | `core/engine/tool_setup.rs:42-59` | 通过 `Op::SendMessage { mode }` 触发 |
| Plan mode Shell sandbox（ReadOnly） | `core/engine/tool_setup.rs:22-33` | 同上 |
| `update_plan` 工具 + `PlanState` | `tools/plan.rs` | 底座注册到 ToolRegistry，bridge 通过 plan 工具结果反序列化拿 snapshot |
| Plan system prompt | `prompts/modes/plan.md` | 底座自动注入 |
| Approval policy 切换（Plan→Suggest / YOLO→Auto） | `Op::SendMessage` 携带 `approval_mode` | bridge 按 mode 决定字段值 |
| YOLO trust_mode = true 自动联动 | bridge 按 mode 决定 `trust_mode` 字段值 | |
| `Op::SendMessage { mode, allow_shell, trust_mode, auto_approve, approval_mode }` | `core/ops.rs:14` | bridge 直接构造 |

### 4.2 pinvou3 增量（**bridge + 前端，零侵入底座**）

| 增量 | 文件 | 工作量 |
|---|---|---|
| bridge 状态机：`mode + plan_phase` 持久化到 SessionStore | `src-tauri/src/bridge/sessions.rs` | 0.3 天 |
| bridge 命令：`set_mode_for_next_send` / `accept_plan` / `revise_plan` / `discard_plan` / `exit_plan_to_yolo` | `src-tauri/src/commands.rs` | 0.4 天 |
| bridge 监听：turn 结束 + plan_tool_used_in_turn + plan 非空 → emit `plan:ready` event | `src-tauri/src/engine.rs` | 0.3 天 |
| 前端 [💡] 按钮 + 4 状态（默认/高亮/disabled/二次确认） | `src/main.js` + `src/index.html` | 0.3 天 |
| 前端 composer 边框 + chip 提示行 + [⚡ 直接动手] 按钮 | `src/styles/chat.css` + `src/main.js` | 0.3 天 |
| 前端 message kind `plan_card` 渲染 + 三按钮 + 状态机（active/approved/exited/frozen） | `src/main.js` | 0.6 天 |
| `instructions.md` Plan section 引导（主动给选项缩小歧义） | `resources/bundle/instructions.md` | 0.2 天 |
| Footer mode chip（一直在） | `src/main.js` + `src/styles/chat.css` | 0.2 天 |

**合计 2.6 天**。**不需要 fork 改 DeepSeek-TUI**。

### 4.3 明确不复用底座的部分

| 底座功能 | 不复用原因 |
|---|---|
| `PlanPromptView` 模态（4 选项 TUI 弹窗） | bridge 模式下用户在 webview，气泡内嵌卡片比模态体验好 |
| Agent mode | 普通用户不需要 per-tool approval；pinvou3 砍掉这层中间态 |
| TUI `/mode` slash 命令 + `ModePickerView` | 用 GUI [💡] 按钮替代 |
| Tab 键循环 mode | 不暴露 keyboard shortcut（普通用户不期待） |

---

## 5. UI 设计

### 5.1 四个视觉锚点（用户永远知道在哪个 mode）

1. **composer 边框颜色**
   - YOLO 默认：灰
   - Plan 进行中：蓝 + 左侧细条
   - Plan 执行中：绿

2. **composer 上方 chip 行**（仅非 YOLO 默认显示）
   - Planning：`💡 Plan 模式 · 还在讨论，AI 不会动手  [⚡ 直接动手]`
   - Ready：`✨ AI 给出方案 · 看下面卡片决策`（无按钮，引导看卡片）
   - Executing：`⚡ 执行中 · 「<plan 标题>」  [⏹️ 中断]`

3. **Footer mode chip**（永久显示）
   - `⚡ YOLO` / `💡 PLAN` / `🏃 执行中`

4. **[💡] 按钮自身状态**
   - `phase = None`（YOLO 默认）：可点（亮）
   - `phase = Planning / Ready`：**disabled**（已经在 Plan，用 chip ⚡ 或卡片退出）
   - `phase = Executing`：可点 + 二次确认「中断当前并开启新 Plan？」

### 5.2 Plan Ready 卡片（消息流内嵌，不是弹窗）

```
[AI 气泡] ┌─ ✨ 方案准备好 ─────────────┐
         │  ## 整理 ~/Documents 步骤    │
         │  1. ○ 扫描 /Documents/       │
         │  2. ○ 按类型分组             │
         │  3. ○ 生成 index.md          │
         │  4. ○ 移动到子目录           │
         │  ─────────                  │
         │  下一步：                    │
         │  [✅ 就这么干]  [✏️ 改改]   │
         │  [🚪 算了]                  │
         └─────────────────────────────┘
```

**用户点 ✅ 就这么干**：
- 卡片按钮 disabled + 显示「✅ 已批准」
- 下方追加 user 气泡（内容："✅ 就这么干"）
- bridge `accept_plan` → mode=Yolo, plan_phase=Executing → 重发 plan markdown 作为执行指令

**用户点 ✏️ 改改**：
- 卡片按钮变「等待修改意见…」灰态
- composer focus + placeholder「告诉 AI 你想改什么…」
- 用户发消息 → AI 修订 plan → **新出一张卡片**接在后面
- 旧卡片冻结成只读历史

**用户点 🚪 算了**：
- 卡片显示「🚪 已退出 Plan」
- plan_phase = None, mode = Yolo
- 任务结束

**历史滚回**：所有旧卡片冻结 + 按钮置灰显示「📜 已批准 / 已退出」。

### 5.3 [⚡ 直接动手] 语义（chip 退出按钮）

跟 [✅ 就这么干] 区分：

| 触发 | 语义 |
|---|---|
| Planning 态 [⚡ 直接动手] | "聊够了，凭对话历史自由干，**不需要完整 plan**" |
| Ready 态 [✅ 就这么干] | "按 AI 给的 plan **严格执行**" |
| Ready 态 [🚪 算了] | "**不干了**，plan 丢弃，任务取消" |

⚡ = 提前结束讨论开干；🚪 = 放弃这件事。方向不同。

---

## 6. instructions.md Plan section 引导（关键）

底座 `prompts/modes/plan.md` 是 design-first 引导，**但没有"主动给选项让用户选缩小歧义"**（Claude Code 那种 A/B/C 多选风格）。pinvou3 在 bundle `instructions.md` 加 Plan section 强化：

```markdown
## Plan 模式工作方法

进入 Plan 模式时，你的目标是**先与用户对齐方案，不要急于产出完整 plan**。

1. **任务有歧义时主动给选项**：不要假设用户意图。给 2-4 个候选方案让用户选，
   每个方案标注关键差异（例：「方案 A：按时间分类；方案 B：按主题分类」）。
2. **简短问澄清问题优先于直接出 plan**：用户可能没说清前提（"整理"是删
   重复还是按类目归档？"7 天行程"是商务还是亲子？）。
3. **方案确定后再调 `update_plan`**：每个步骤要可执行、可验证。
4. **plan 出完后明确停止**：在最后说"以上方案如果可以，请点【就这么干】开始
   执行，需要调整请点【改改】"。
```

---

## 7. 实施清单（按依赖顺序）

| 顺序 | 任务 | 工作量 |
|---|---|---|
| 1 | bridge 状态机（mode + plan_phase + persistence）+ 移除写死 `trust_mode = true` | 0.5 天 |
| 2 | bridge 5 个命令（`set_plan_mode_next` / `accept_plan` / `revise_plan` / `discard_plan` / `exit_plan_to_yolo`）+ `Op::SendMessage` mode 注入 | 0.4 天 |
| 3 | bridge turn-end 监听 + plan_state snapshot → emit `plan:ready` event | 0.3 天 |
| 4 | 前端 [💡] 按钮 + 4 状态 + composer 边框 + footer chip | 0.4 天 |
| 5 | 前端 chip 提示行 + [⚡ 直接动手] | 0.3 天 |
| 6 | 前端 message kind `plan_card` 渲染 + 三按钮 + 历史冻结 | 0.6 天 |
| 7 | bundle `instructions.md` Plan section 引导 + 自动 hash bump | 0.2 天 |

**合计 2.7 天**。

---

## 8. 不做的事（明确边界）

| 不做 | 理由 |
|---|---|
| Agent mode | 普通用户不期待 per-tool approval；pinvou3 砍掉 |
| TUI 风格 4 选项 `PlanPromptView` 弹窗 | 气泡卡片更优 |
| AI 反向触发 Plan（YOLO 中 AI 主动切 Plan） | 增加复杂度，AI 在 YOLO 下用对话方式问澄清即可 |
| 顶栏持久 mode toggle | 增加心智负担；[💡] 临时按钮 + chip 已够清楚 |
| Tab 键循环 mode | 不暴露 keyboard shortcut |
| slash 命令 `/plan` `/yolo` | 普通用户不熟悉 slash 范式 |
| 改 DeepSeek-TUI 源码 | 底座完整支持，0 侵入 |
| per-message mode 标签持久化 | mode 是会话级状态，单 message 标签语义不闭合 |

---

## 9. 验证场景（端到端）

### 9.1 基本流程
1. **YOLO 默认**：新对话发"翻译这段：hello" → AI 直接翻译 → 完成
2. **进入 Plan**：点 [💡] → composer 蓝边框 + chip 显示 → 发"帮我整理 ~/Documents" → AI 用 update_plan 出 plan → 弹气泡卡片
3. **接受 plan**：点 ✅ 就这么干 → 卡片 disabled + 显示已批准 → AI 切回 YOLO 开始执行 → 完成后回 home 态
4. **修订 plan**：点 ✏️ 改改 → 用户输入"分类时把代码项目独立分一类" → AI 出新卡片 → 旧卡片冻结
5. **放弃 plan**：点 🚪 算了 → 卡片显示已退出 → 回 home 态

### 9.2 边界场景
6. **Planning 中途退出**：进 Plan 聊一两轮 → 点 chip [⚡ 直接动手] → mode 切回 YOLO + 对话历史保留 → AI 凭 context 直接干
7. **执行中再开 Plan**：执行中点 [💡] → 二次确认弹窗「中断当前并开启新 Plan？」 → 确认后 Cancel + 进入 Planning
8. **多轮修订**：点 ✏️ 改改两次以上 → 历史里有 3 张冻结卡片 + 1 张 active

### 9.3 视觉一致性
9. **滚动历史**：旧 plan 卡片冻结按钮 + 显示「📜 已批准」/「📜 已退出」状态
10. **mode chip 同步**：所有 mode 变化时 footer chip + composer 边框 + chip 行 **同时更新**，无延迟错位

### 9.4 防御
11. **敏感目录硬底线**：YOLO + Plan 两 mode 下尝试访问 ~/.ssh/ → 都被 hook 拦截

---

## 10. 风险

- **plan_tool_used_in_turn 判据从底座抄但事件不同**：bridge 没有 TUI 的 `app` 状态，要从 engine event 流自己重建该标记（监听 ToolCallCompleted 工具名 == `update_plan` 即可）
- **Plan 修订时新旧卡片冻结状态**：要保证旧卡片按钮真置灰不可触；前端 plan_card 状态机要有完整测试
- **执行中 [💡] 二次确认**：用户误触可能丢失正在执行的任务；UI 必须明确"将中断"
- **mode 切换跟 ongoing turn 的竞态**：用户在 turn 进行中点切 mode 怎么办（建议 disabled，turn 结束才能切）
- **Plan 接受后重发 plan markdown 怎么传**：作为隐式 system 注入 vs user message 头部 — 倾向 user 前缀 + 明确「按以下 plan 步骤执行」开头，避免 long context 下 LLM 偏离

---

## 11. 跟其他阶段的关系

- **阶段 D 候选其他项**：
  - D.D bundle 领域 skill：跟本设计**正交**（SKILL 是 instructions 注入，Plan mode 是 mode 切换；领域 skill 在 YOLO/Plan 两 mode 下都生效）
  - D.E 模型预设切换 GUI：跟本设计**正交**（mode 和 model 是两个独立维度）
  - D.B WorkFlow 视图：**可能融合**——WorkFlow 视图本质是 plan 的可视化，本设计的 plan_card 是其雏形
- **阶段 C AI 加固**：`instructions.md` Plan section 引导跟"中文字号准确性"等加固项并列，可一同纳入下一次 bundle bump

---

## 12. V2 修订：弱模型加固（2026-05-14 拍板）

> V1 实施后测试发现 7 次失败模式，全部靠 prompt 补丁修复治标不治本。
> 经多 agent 并行分析（症状-根因 / 架构 gap / 同类系统借鉴）三方共识：
> **prompt 补丁路线不可持续，必须架构层加固**。

### 12.1 V1 暴露的 7 次失败模式

| # | 现象 | V1 补丁（待替换） |
|---|---|---|
| 1 | Plan 修订时 AI 不调 update_plan，方案塞 text + 假按钮引导 | instructions.md 加引导 |
| 2 | Plan 修订时 AI 调 todo_write 而非 update_plan | plan_ready 触发扩展（保留） |
| 3 | Plan 模式 AI 在 text 贴完整代码 | instructions.md 加 ≤15 行 |
| 4 | Plan 模式 AI 试 write_file 失败 + 给错 `/config` 指引 | plan_stuck 兜底卡片（保留） |
| 5 | Plan 模式 AI 跳过 update_plan 直接试 write_file | 同 #4 |
| 6 | Executing 态 AI 只标 in_progress 就停 | accept_plan 模板 + 执行态行为 |
| 7 | request_user_input 行为不稳定 | instructions.md 加"优先调" |

### 12.2 元根因（三方共识）

**Qwen3.6-35B-A3B 没有"叙述层 vs 动作层"的元认知**——它把"说我开始写"和"真写"在同一语义平面理解。prompt 写"禁止 X"对它的约束力 ≪ Claude 级别（后者 RLHF 内化）。

放大器：
- 工具语义重叠（todo_write / update_plan / checklist_write / request_user_input 多条等价路径）
- 状态机不可观测（mode 状态对 LLM 来说是 system prompt 里的几个字，没有结构化反馈）
- pinvou3 设计依赖 LLM "正确调工具"推进 UI 状态机（**致命架构错误**：把不可靠组件嵌进 critical path）

**Verdict**：instructions.md 已 239 行还在长。继续补只会越补越漏，prompt rot 让旧禁令服从率随之下降。

### 12.3 三个不变量（架构原则）

**不变量 1：UI 状态机由 bridge 拥有**

- LLM 是带噪声的内容生成器，不是状态命令源
- 任何 plan_phase 转移都有 deterministic 兜底
- LLM 调对工具 = fast path；调错 = bridge 容错路径

**不变量 2：关键约束用机械手段强制**

- tool schema 物理剥离（已由底座 Plan 模式 keep list 实现，确认即可）
- 状态相关引导用 per-turn `<system-reminder>` 动态注入，**不放静态 instructions.md**
- 强制 schema / 多级 fuzzy 解析

**不变量 3：Agent loop 自驱**

- Executing 态 LLM 调一次工具就停 → bridge 自动 send "继续"，max 3 次连续
- 不靠用户点"继续"驱动单步

### 12.4 同类系统借鉴（实证）

来源：Claude Code / Codex CLI / Aider / Cursor / Continue / Qwen-Agent / Docker LLM Tool Calling 评测

**通用模式**：
1. 状态用 per-turn `<system-reminder>` 注入 而非一次性 system prompt（抗 long-context 遗忘）
2. 状态出口是专用工具调用（Claude Code 的 ExitPlanMode）—— LLM 显式声明而非应用层猜
3. Agent loop 默认 `while (model_emits_tool_call)`，没人靠用户点"继续"
4. 多级 fuzzy fallback 而非 retry（Aider SEARCH/REPLACE 4 级）
5. Bounded retry budget（Aider 3 次反射 / Cursor 25 tool call）

**弱模型适配（key insight）**：
- Anthropic 的 prompt-only enforcement 假设 Sonnet+ 级别 instruction-following
- 弱模型必须补**外部结构**：tool catalog 真剥离 / schema validate / 多级 fuzzy 解析
- Aider 显式按模型选 edit format（Gemini diff-fenced、弱模型用 whole）——**模型能力适配是 first-class concern**

**反模式**（必须避免）：
- 无界 retry（Cursor warmup 死循环）
- 靠用户点"继续"驱动单步（Cursor 25-tool 限制被骂最多）
- 让 LLM 自己问"是否继续"代替状态转移工具

---

## 13. V2 实施计划：五机制

| # | 机制 | 工作量 | 治哪些 V1 失败模式 |
|---|---|---|---|
| **M1** | per-turn `<system-reminder>` 动态注入（bridge 按 mode/phase 在 user message 前 prepend） | 0.7 天 | 1, 3, 5, 6 |
| **M2** | Executing 态 agent loop 自驱（turn end + plan pending + 没调 write 类 → 自动 send "继续" max 3 次） | 1 天 | 6 |
| **M3** | Planning 态文本兜底卡片（turn end + 没调 plan 工具 + text > 300 字 → 弹"是否直接采纳"卡片提取 text） | 0.5 天 | 1, 5 |
| **M4** | 大幅精简 instructions.md（删 Plan 模式相关 80%，状态相关引导全移到 M1 动态注入） | 0.3 天 | 防 prompt rot |
| **M5** | bridge 检测 + plan_stuck 扩展（已 30% 实现） | 0.5 天 | 4, 5 |

**合计 3 天**。一次性替代当前 7 条补丁 + 防未来同类问题。

### 13.1 M1 详细设计：per-turn `<system-reminder>`

bridge `build_send_message_op` 按当前 mode/phase 在 user message 前 prepend `<system-reminder>...</system-reminder>` 段：

```rust
fn reminder_for(mode: AppMode, phase: PlanPhase) -> Option<&'static str> {
    match (mode, phase) {
        (AppMode::Plan, PlanPhase::Planning) => Some(r#"
你现在在 Plan 模式 + Planning 阶段。本 turn 你必须:
1. 第一动作:用 request_user_input 工具问澄清(如有歧义) 或 update_plan 工具出方案
2. 禁止在 text 里描述方案/贴代码/写"请点【就这么干】"等按钮引导文字
3. 写工具(write_file/exec_shell)在 Plan 模式不可用,调用会失败
"#),
        (AppMode::Yolo, PlanPhase::Executing) => Some(r#"
你现在在执行阶段(用户已批准方案)。本 turn 你必须:
1. 第一动作:用 write_file/edit_file/exec_shell 工具**实际产出文件**
2. 禁止只调 update_plan 标记 in_progress 就结束 turn——那是假执行
3. 一个 turn 内连续调多个工具直到所有步骤完成,不要中途停下来
"#),
        _ => None,
    }
}
```

为什么用 `<system-reminder>` 标签：(a) Claude Code 实证有效格式;(b) 比 markdown bold 信号更清晰;(c) Qwen3.6 短期注意力强,放在 user message 顶端命中率高。

### 13.2 M2 详细设计：Executing 自驱

`engine.rs` event forwarder TurnComplete 分支扩展：

```rust
// Executing 态 turn end 检测
if mode == Yolo && plan_phase == Executing {
    let plan_still_pending = last_plan_snapshot.items.iter().any(|i| i.status != "completed");
    let no_write_tool_called = !tracker.write_tool_called;
    let count = store.auto_continue_count(&active_id);

    if plan_still_pending && no_write_tool_called && count < 3 {
        store.bump_auto_continue(&active_id);
        engine.send_user_message(
            "继续执行下一步,记得用 write_file/edit_file 等工具实际产出文件。",
            Yolo,
        ).await;
    } else if count >= 3 {
        emit chat:execution_stuck event;  // 前端弹兜底
    }
}
```

新增 SessionStore 字段 `auto_continue_count: HashMap<sid, u8>`，用户主动发消息时清零。

### 13.3 拍板的设计点

| 设计点 | 选择 |
|---|---|
| M1 reminder 格式 | `<system-reminder>` XML 标签包裹（Claude 风格） |
| M2 auto-continue 触发 | 立即 send（不等 1 秒） |
| M2 上限 | max 3 次连续 auto-continue（用户主动发消息清零） |
| M3 触发阈值 | text > 300 字 **且** 含"方案"/"步骤"/"以下"任一关键词（精准） |
| M3 卡片按钮 | 3 选：✅ 采纳 text 作 plan / ✏️ 让 AI 用工具重出 / 🚪 算了 |

### 13.4 实施顺序

1. **M4 精简 instructions.md**（先精简，给 M1 腾出引导空间）
2. **M1 per-turn reminder**（核心,先做让其他机制补底）
3. **M3 文本兜底卡片**（兜底 1 处场景）
4. **M2 Executing 自驱**（兜底另一处场景）
5. **M5 plan_stuck 扩展**（已 30%）

### 13.5 验证

| 场景 | 期望行为 |
|---|---|
| 用户首条消息进 Plan 态："做俄罗斯方块" | reminder 注入 → AI 优先调 request_user_input 或 update_plan，不会试 write_file |
| Plan 修订："改成深色背景" | reminder 注入 → AI 必调 update_plan，不会只在 text 描述 |
| 用户 ✅ 就这么干 | accept_plan 模板 + reminder 注入 → AI 第一动作必 write_file，不会只标 in_progress |
| AI Executing 态调一次工具就停 | M2 自动 continue 一次让 AI 继续，max 3 次后弹 stuck 卡片 |
| AI 在 Plan 态没调工具但 text 写完整方案 | M3 弹兜底卡片让用户采纳 text 或重出 |

### 13.6 与 V1 已实现部分的关系

| V1 已做 | V2 处理 |
|---|---|
| 状态机（mode + plan_phase） | 保留，M1/M2 依赖它 |
| plan_card 两层渲染（plan + todos） | 保留 |
| plan_stuck 卡片 | M5 扩展（增加 turn-end 检测条件） |
| request_user_input 紫色气泡 | 保留 |
| chip 上 ⚡ 直接动手 | 保留 |
| instructions.md Plan 模式相关 ~150 行 | **删除 80%**（M4），保留通用引导 |
| accept_plan 当前指令模板 | **替换**为简短指令 + M1 reminder（reminder 才是约束主力） |
