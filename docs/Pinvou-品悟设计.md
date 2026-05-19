# Pinvou 品悟设计

> 状态：v2 完整落地（19 commits），2026-05-19 端到端跑通
> 背景：基于 pinvou2 实证教训 + gstack production 设计 + 本地 Qwen3.6 + DeepSeek-TUI 同步前台流的约束
> 命名：v2 把"嘴替"统一改成"品悟"——跟产品名 pinvou 同名，宣告这是 app 的灵魂能力

## 1. 一句话总结

**品悟是 Boss 的代视角审查者，只在 3 个 LLM-review 节点出场（plan 入门 / stuck 兜底 / 任务收口），危险命令完全交给 hook 处理，其他阶段完全沉默。**

形态从 pinvou2 的"常驻并发"压缩到"3 节点触发"。这不是缩水，是基于 pinvou2 自己的演进方向 + 实证教训做出的有意取舍。

## 2. 核心决策

| 决策 | 理由 |
|---|---|
| ❌ **不做常驻并发品悟** | pinvou2 raise_concern 工具化率仅 25%（实战退化）；CrewAI hierarchical 同形态在 4 万 star 框架里也实战崩坏 |
| ❌ **不做 heartbeat 容器** | pinvou3 是 DeepSeek-TUI 同步前台流，没有"无人值守容器执行器"场景（已查证：`crates/tui/src/core/engine/turn_loop.rs` LLM 一个 turn 调一次工具就停） |
| ❌ **不做用户主动 @pinvou 召唤** | pinvou2 品悟 100% 被动（`actors/pinvou/CLAUDE.md:38-46` 硬铁律），保持"品悟不抢戏"的灵魂 |
| ✅ **做流水线 review** | gstack production 验证有效（Garry Tan 日常用） |
| ✅ **做结构化 EXIT GATE** | 抄 gstack `## GSTACK REVIEW REPORT` 防 self-deception；pinvou2 raise_concern 死掉 + 本仓库 commit `7b983b6` 回滚教训均指向"advisory 引导对 Qwen3.6 不可靠，必须 blocking" |
| ✅ **做 careful hook**（品悟之外） | gstack `careful` 模式：硬编码 pattern + Pre-tool 拦截，零 LLM 开销，比 LLM review 更可靠 |

## 3. 三个出场节点

```
[Daily 闲聊]                  ✗ 沉默
   ↓
[Plan Mode → ExitPlanMode]    ★ 阶段 A：/pinvou-review-plan（必做，blocking）
   ↓
[Execute / Tool Call]         ✗ Pinvou 不参与
   ↓ 危险命令              → careful hook 自动拦截（非 LLM）
   ↓
[execution_stuck 卡片]        ☆ 阶段 E：品悟接管（v1.5 选做，advisory）
   ↓
[任务完成]                    ★ 阶段 D：/pinvou-review-final（必做，advisory）
```

| 节点 | 触发 | 严苛度 | 落地 |
|---|---|---|---|
| **A. Plan 出炉** | `ExitPlanMode` 前自动 | **L2 blocking**（GATE 不通过不放行） | skill + bridge gate |
| **E. Stuck 兜底** | `auto-continue` 3 次失败弹卡片 | L1 advisory | 扩展现有 stuck-card UI |
| **D. 任务收口** | TurnComplete + 无下一步 / 用户标记 | L1 advisory（纯总结，无按钮） | skill |

3 个节点对应 Boss 替不在场客户问的 3 个问题：
- A：「这方向真对吗？」
- E：「卡了，要换条路吗？」
- D：「真做完了吗？」

## 4. 关键机制

### 4.1 careful hook（pinvou3 现状缺失，需新增）

**问题**：pinvou3-app 在 YOLO 模式下自动 approve 所有 `ApprovalRequirement::Required`（commit `618521d`），等于把 DeepSeek-TUI 的 approval 框架废了。无破坏性 pattern 检测。

**方案**：扩展 `ApprovalRequirement` 加新等级 `RequiredDangerous`：

```rust
// DeepSeek-TUI/crates/tui/src/tools/shell.rs
fn approval_requirement(&self, args: &ShellArgs) -> ApprovalRequirement {
    if DANGEROUS_PATTERNS.iter().any(|p| p.matches(&args.command)) {
        return ApprovalRequirement::RequiredDangerous;
    }
    // 现有逻辑
}

// pinvou3-app/src-tauri/src/engine.rs（改 auto-approve）
if event.requirement == RequiredDangerous {
    show_dangerous_warning_dialog(event);  // 不 auto-approve
} else if yolo_mode {
    handle.approve_tool_call(id);
}
```

**Pattern 列表（参考 gstack `careful/SKILL.md`）**：

| 拦截 | 放行 |
|---|---|
| `rm -rf /xxx`, `rm -rf ~/xxx` | `rm -rf node_modules`, `.next/`, `dist/`, `__pycache__`, `.cache`, `build/`, `coverage/` |
| `DROP TABLE`, `DROP DATABASE`, `TRUNCATE` | — |
| `git push --force`, `git push -f` | — |
| `git reset --hard`, `git checkout .` | — |
| `kubectl delete`, `docker system prune` | — |

**工作量**：~150 行 Rust，遵循 fork ≤50 行 PR 原则（pattern 检测器单独成模块，主框架几乎不动）。

**为什么不用 LLM review B 节点**：硬编码 pattern 零 token、零延迟、不会被 prompt 工程绕过、命中即拦无退化路径。这事就该交给确定性规则。

### 4.2 PINVOU REVIEW REPORT 表格 + EXIT GATE（pinvou3 现状缺失，需新增）

**问题**：DeepSeek-TUI `ExitPlanMode` 直接 `app.set_mode(AppMode::Agent)`，零检查（`crates/tui/src/tui/ui.rs`）。pinvou3-app 用 per-turn `<system-reminder>` 引导 LLM 行为，但 commit `7b983b6` 已经证明 advisory 引导对 Qwen3.6 不可靠（"禁过渡语反向膨胀 23→556 字"，已回滚）。

**方案**：bridge 层拦截 ExitPlanMode 事件，强制检查 plan 文件末尾结构化表格。

**表格格式**：

```markdown
## PINVOU REVIEW REPORT

| Finding | Severity | Status | User Decision |
|---------|----------|--------|---------------|
| 用户没说性能预算，方案隐含 200ms 假设 | CRITICAL | RAISED | 待用户拍 |
| Step 3 路径会覆盖现有 config | CRITICAL | RESOLVED | 已采纳并修订 |
| 测试覆盖率没在方案里 | INFORMATIONAL | OVERRIDDEN_BY_USER | 用户判定可接受 |

**VERDICT**: 0 critical 待拍板 — 可 ExitPlanMode
```

**EXIT GATE 逻辑**：

```rust
fn check_exit_plan_gate(plan_content: &str) -> Result<(), GateError> {
    if !plan_content.ends_with_section("## PINVOU REVIEW REPORT") {
        return Err(GateError::MissingReviewReport);
    }
    if !has_findings_table(plan_content) {
        return Err(GateError::MalformedReport);
    }
    if has_unresolved_critical(plan_content) {
        return Err(GateError::UnresolvedCritical);
    }
    Ok(())
}
```

**强制要求**：
- 表格必须是 plan 文件最后一个 `## ` heading（防 LLM "我审过了，写在文件中间也算"）
- 至少 1 行 finding（即使是 "无明显风险" 也必须显式写出 CLEAR）
- CRITICAL 必须 RESOLVED 或 OVERRIDDEN_BY_USER 才能放行

**工作量**：~80 行 Rust + 1 个 `pinvou3-app/src-tauri/resources/bundle/skills/pinvou-review-plan/SKILL.md`(随 app 编译内嵌,启动时解包到 `~/.pinvou3/bundle/skills/`) skill。

**关键警示**：**不能仅靠 prompt 引导让 Pinvou 写表格**——`7b983b6` 已经证明 Qwen3.6 不吃这套。必须 bridge 层 blocking + skill prompt 双保险，bridge 检测到无表格就自动追加一次 `/pinvou-review-plan`，逼它生成。

### 4.3 Outside Voice（subagent + 差异化 persona）

**gstack 原版**：用 `codex exec` 调另一个 AI 系统（GPT），Codex 不可用时 fallback `Task` 工具派 subagent（fresh context = genuine independence）。两个不同模型互相印证。

**pinvou3 处境**：本地只有 Qwen3.6，没有"第二个不同模型"。subagent fallback = 同模型 fresh prompt 再跑一遍，独立性打折（漏洞重叠度可能 70%+）。

**v1 方案**：A + C 组合
- **A. subagent fresh context**：用 DeepSeek-TUI 的 Task/Agent 工具派出独立 LLM session
- **C. 差异化 persona prompt**：主 LLM 用工程师视角，subagent 用"Boss 品悟"视角（不同语气、不同关注点、不同 Suppressions 列表）

承认这是"半外"而非"真外"。如果未来 pinvou3 引入云 API（Claude/GPT）兜底，可升级到 D 方案（云 outside voice），效果最佳但有成本。

### 4.4 Pinvou Review Skill prompt 骨架

**风格**：customer 主 + attacker 副混合（避开 advisor 退化成客气话）

```
你是 Pinvou，Boss 的品悟。Boss 不在场，你替他审眼前这个方案。
(pinvou3 单 LLM 架构,被审的方案就是你前面 turn 自己产出的;换 persona 重新审,不假装是别人写的)

你的发言要做两件事：
1) 第一视角：用 Boss 的语气问"如果是我会问..."（customer mindset）
2) 强制找硬伤：必须输出 ≥1 个具体担忧/风险/漏洞（attacker mindset）

禁止：
- 范围外建议（"顺便加个 X 吧"）——不是 Boss 的关心点
- 技术细节挑剔（"这个函数名不规范"）——Boss 不在乎
- 客气话开场（"整体看起来不错，但..."）——Boss 没空寒暄
- 自然语言提担忧而不写表格——必须输出 ## PINVOU REVIEW REPORT 表格

输出格式（追加到 plan 文件末尾）：
## PINVOU REVIEW REPORT
| Finding | Severity | Status | User Decision |
|---------|----------|--------|---------------|
| <Boss 视角的核心担忧> | CRITICAL/INFORMATIONAL/CLEAR | RAISED | 待用户拍 |

**VERDICT**: <一句话总结>
```

**Suppressions 列表**（抄 gstack `review/checklist.md:170-180`，避免噪音）：
- "X 跟 Y 冗余"但实际增可读性的
- "加注释解释这个阈值"——阈值会变，注释会烂
- "测试可以更严"已经覆盖行为的
- "Regex 不处理 X 边缘"实际输入不会出现 X 的

## 5. 作为独立开关,正交于 Plan/YOLO

**品悟 review 不是第三种工作流**,而是与 Plan/YOLO **正交的质量护栏开关**。Plan/YOLO 是执行风格(用户已有),品悟是质量保险(新增),两者独立可组合。

| 维度 | 选项 | 默认 | 关系 |
|---|---|---|---|
| **执行风格**(已有) | Plan / YOLO | YOLO | 用户原有切换不动 |
| **品悟 review**(新) | ON / OFF | OFF | 独立 toggle |
| **careful hook**(新) | 始终 ON | 不可选 | 与上面两个无关 |

**四种组合**:

| 组合 | 行为 |
|---|---|
| Plan + 品悟 OFF | 当前现状,plan card → accept/revise/discard |
| **Plan + 品悟 ON** | plan 出炉触发 EXIT GATE,任务收口触发 final review(v1.5) |
| YOLO + 品悟 OFF | 当前现状,AI 直接干 |
| **YOLO + 品悟 ON** | 任务收口触发 final review(YOLO 无 plan 期,EXIT GATE 不生效) |

**配置形态**(per-session,存 SessionStore):

```rust
SessionModeState {
    mode: SerializableMode,         // 用户原有 Plan/YOLO 切换
    plan_phase: PlanPhase,
    pinvou_review_enabled: bool,    // 新增:品悟开关,默认 false
}
```

**UI 入口**:pinvou3-app 顶部加一个独立 toggle 按钮 `🟣 品悟`(默认灰色),点一下切换 ON/OFF(高亮紫色)。原有 Plan/YOLO 切换不动。

**关键设计原则**:
- **正交不替代**:用户原有的 Plan/YOLO 切换是执行风格,品悟是质量护栏,任意组合
- **careful hook 不可选**:破坏性命令防护是基础安全,不让用户关
- **品悟默认关**:保持现状行为,用户主动开启
- **per-session 配置**:不同 session 可以有不同的品悟开关状态

## 6. UI 品悟观感

底层串行 review，UI 渲染成"三个角色对话"错觉：

```
[AI 气泡] 方案 A...    (实际渲染时是模型名,如 "QWEN3.6")
       ↓ (后台跑 /pinvou-review-plan，500ms-2s)
[Pinvou 气泡，带打字光标动画]
       "Boss 视角看：我担心 3 件事..."
       1) ... 2) ... 3) ...
[底部 3 按钮]
   [✓ 直接执行]  [↻ AI 改方案]  [⊕ 我自己加一句]
```

**3 按钮设计**：
- **直接执行**：用户判定 Pinvou 多虑了，一键 override 所有 CRITICAL（标 OVERRIDDEN_BY_USER）→ EXIT GATE 放行
- **AI 改方案**：把 review 表格作为 user message 注入对话，让 AI 修方案 → 再触发 Pinvou re-review 循环
- **我自己加一句**：用户补充意见进 plan 文件，再触发 AI 修方案

**关键产品取舍**：
- Pinvou 的发言必须是 review skill 的真实产出，不能为对话感凭空发言
- Pinvou 不能每个 turn 都跳出来，只在 A/D/E 节点
- Pinvou 必须看完整 plan / 完整 trace 才发言（256K context 让这变得可能）

## 7. 与 pinvou2 的关系

**意图层面：高度一致** ✅
- 品悟代客户发声、关键节点质疑、防 LLM 自欺——核心理念全保留
- 100% 被动出场（无用户召唤入口）——保留 pinvou2 品悟灵魂

**形态层面：与 pinvou2 最新演进方向趋同**（不是和旧设计对齐）

| 维度 | pinvou2 旧（已废弃） | pinvou2 新（已演进） | pinvou3 v1 |
|---|---|---|---|
| Pinvou 角色 | 派单 + 审阅 + 体检（混乱） | **仅末端验收** | **仅 plan + final + stuck** |
| 任务派发 | Python workflow_engine + 容器 | Claw 自主 + Agent 工具派 subagent | 不需要（同步流，无多卡片场景） |
| 品悟交互 | 自动在线 + 被动响应 + 100% 无召唤 | 同上 | 节点触发 + UI 气泡（观感等效） |

**与 pinvou2 不同的部分（有意取舍）**：

| 差异 | 取舍理由 |
|---|---|
| pinvou3 plan 期不"持续盯"中间 turn | pinvou2 raise_concern 工具化率 25%，证明常驻并不真发挥作用；EXIT GATE blocking 更可靠 |
| pinvou3 execute 期不参与 | pinvou3 同步前台流，没有 executor 多步场景 |
| pinvou3 多 careful hook 层 | YOLO auto-approve 把 approval 废了，需要硬编码 pattern 补位 |
| pinvou3 多 EXIT GATE blocking | pinvou2 advisory raise_concern 实战死了，本仓库 `7b983b6` 同样回滚过引导式方案 |

**新增 pinvou2 没有的部分**：
- Outside Voice subagent（gstack 模式，pinvou2 无此设计）
- 结构化 EXIT GATE（pinvou2 raise_concern 失败的对症药）
- careful hook（pinvou2 没有，靠 sandbox 兜底）

## 8. v2 实际落地清单（已实施）

实际落地比 v1 计划丰富，加了多个 v1 未预见的工程修复（详见 §10）。19 个 commit 覆盖：

| # | 模块 | 实际工作量 | 关键文件 |
|---|---|---|---|
| 1 | `careful` hook（不用新加 enum，直接去掉 YOLO Dangerous 守卫） | 15 行 Rust，patch 形式 | `patches/dtui-careful-yolo-block-dangerous.patch`（fork PR 候选） |
| 2 | EXIT GATE blocking + 单元测试 | ~280 行 Rust，7 个 test | `pinvou3-app/src-tauri/src/bridge/review_gate.rs` |
| 3 | `pinvou_review_enabled` toggle + Tauri command | ~100 行 Rust | `mode_state.rs` / `sessions.rs` / `commands.rs` / `lib.rs` |
| 4 | `/pinvou-review-plan` skill（v2 精简版） | 89 行 markdown | `pinvou3-app/src-tauri/resources/bundle/skills/pinvou-review-plan/SKILL.md` |
| 5 | `/pinvou-review-final` skill（v2 精简版） | 94 行 markdown | `pinvou3-app/src-tauri/resources/bundle/skills/pinvou-review-final/SKILL.md` |
| 6 | Bundle 内置 skill 解包机制 | bundle.rs + include_str! + ensure_extracted 强制写 | `pinvou3-app/src-tauri/src/bridge/bundle.rs` |
| 7 | 前端 toggle + 品悟气泡 + 3 按钮 + Fallback | ~600 行 JS/CSS | `main.js` / `index.html` / `chat.css` |
| 8 | 阶段 D final review 自动触发 | ~50 行 JS | `main.js` chat:done 监听 |
| 9 | Skill body eager loading（绕过 progressive disclosure 在弱模型失效） | bridge `read_skill_body` Tauri command + 前端 invoke | `commands.rs` + `main.js` |
| 10 | 单气泡 Pinvou 观感（避免重复显示） | beginAssistantBubble 加 persona 标记 | `main.js` |
| 11 | 触发 user 气泡简短摘要（不污染对话流） | `dispatchPinvouTrigger` helper | `main.js` |
| 12 | "嘴替→品悟"全局重命名 | 文案 + skill prompt + 代码注释 | 9 个文件 |

**v1.5 / 后续**：
- Stuck-card 品悟接管（扩展现有 `chat:execution_stuck` 卡片）
- DeepSeek-TUI fork patch 推到 h3c-hexin fork 然后 bump submodule pointer

**确认砍掉**：
- ❌ 用户 `@pinvou` 主动召唤（保持 pinvou2 100% 被动灵魂）
- ❌ 每 turn review（噪音过大）
- ❌ B 节点 LLM review（careful hook 替代）
- ❌ heartbeat 容器（pinvou3 无场景）
- ❌ pinvou2 旧"卡片派发"机制（pinvou3 同步流不需要）
- ❌ Clear review 自动放行（用户明确"等用户决策本就是对的"，保留 3 按钮）

## 9. 风险与待验证项

### 8.1 Outside Voice 独立性打折
本地 Qwen 单模型，subagent fallback = 同模型 fresh context，找漏洞重叠度可能高。v1 接受这个折扣（A+C 方案），后续可考虑：
- 装第二个本地小模型（Phi-4 / Llama-3.3 8B）专做 critic
- 接云 API 关键节点兜底（成本可控的话）

### 8.2 EXIT GATE blocking 的回退机制
如果 Pinvou review 失败（subagent 报错、LLM 超时），EXIT GATE 不能死锁——需要降级路径：
- 重试 1 次 → 仍失败 → 弹"Pinvou 不可用，强制继续？"用户拍板
- 不要让基础设施故障变成产品阻塞

### 8.3 Suppressions 列表的维护
gstack `review/checklist.md:170-180` 列表是英文 + 通用 web 开发场景。pinvou3 场景不同（本地 Tauri / Rust + 中文用户），需要：
- v1 先抄 gstack 列表作为种子
- 实际运行中观察哪些 finding 经常被用户 [直接执行]，进 suppressions

### 8.4 256K context 的实际利用
主 LLM 端：tool schema 已占 30K，留给 history ~200K+（充裕）
Pinvou 端：subagent fresh context + 完整 plan + execute trace，单次 prefill 可能 50K+，**单次响应延迟 5-10s**（可接受，因为非热路径）

### 8.5 弱模型加固教训的对照
本仓库 commit `7b983b6` 试过用 reminder 强制 LLM 行为（禁过渡语）→ Qwen3.6 反向膨胀，已回滚。v1 设计直接对症：
- Pinvou skill prompt 中"禁止 X"条款必须配 **代码级强制**（EXIT GATE 检查表格），不靠 LLM 自觉
- 不要重蹈 7b983b6 覆辙

## 10. v1 实施 lessons learned

### 10.1 Qwen3.6 不可靠按格式输出 → 必须前端 Fallback

**实测**(v1 worktree 启动后第一次测试):Pinvou skill 跑了,内容质量不错(找了风险点 + 给了 ✅/⚠️ 标记),但**没用 `## PINVOU REVIEW REPORT` 表格**,而是用项目符号列表 + "技术选型/工作量评估/结论"小标题段。

结果:EXIT GATE 检测不到表格 → 死循环(点 ✅ → GATE 拒绝 → 自动跑 review → 仍非表格 → 再拒绝)。

**修复**(两层):

1. **Skill prompt 加强**:`pinvou-review-plan` skill 顶部加 🚨 输出格式硬约束,明令禁止 ✅/⚠️ 列表 + "工作量评估"小标题,加 3 个完整 few-shot(其中一个就是俄罗斯方块场景)

2. **前端 Fallback 兜底**(`main.js synthesizeOverriddenReport()`):
   - LLM 输出有内容但**无表格** → **仍渲染**品悟气泡(气泡显示 LLM 原话) + 合成 OVERRIDDEN_BY_USER 占位表格
   - 用户点 [✅ 直接执行] → 用合成表格拼 plan_markdown → GATE 二次校验过 → 进 execute

这印证了本仓库 commit `7b983b6` 的回滚教训("Qwen3.6 + advisory reminder 引导不可靠"):**必须 bridge blocking + UI 兜底**,不能纯靠 prompt 引导。

### 10.2 worktree 必须立刻 commit

v1 worktree 创建后**从未 commit**,工作期间一直裸跑写代码。worktree 被物理删除时分支头还停在 base commit,**所有工作 git 无从感知**,reflog/fsck 都救不回。

**v2 教训**:
- Worktree 名字带 `-DO-NOT-DELETE` 后缀,降低被误以为临时工作区的概率
- 创建后第一件事就 commit baseline(即使内容只有几行)
- 每个 Stage 完成立刻 commit,不积压

### 10.3 工作流不要 3 选 1,要正交开关

v1 第一版把 Pinvou Review 设计成"工作流的一种"(Plan / YOLO / Pinvou Review 三选一切换器),被用户指出错——品悟本质是**质量护栏**,应该跟 Plan/YOLO **正交**而非替代。v2 改成独立 toggle(`pinvou_review_enabled: bool`),允许任意组合(YOLO + 品悟开等)。

### 10.4 品悟的副作用:加重实施层负担 → 撞本地模型 output budget

**实测**(v2 跑通 tetris 端到端):

- 品悟关:plan = 7 步基础版,AI 一次 write_file 600 行 HTML 跑通
- 品悟开:Pinvou 给方案加了 Wall Kick + Ghost piece + 等级 + 暂停 +硬下落 → plan 升 9 步详细版 → AI 试图把所有功能塞进一次 write_file → **content 撞 16K output budget 顶 → 输出空 content → 被 DeepSeek-TUI 去重 guard BLOCK**

**根因**:Pinvou 之前的 mindset 只有 customer + attacker,**没有 engineer**(实施可行性视角)。Boss 不会自己写 600 行 HTML,所以 customer mindset 看不到"本地模型一次输出能不能写完"这种约束;attacker mindset 也只挑设计漏洞,不看实施层。

**修复**(已落地):

1. `pinvou-review-plan` skill 加 **Engineer mindset 必查清单**:
   - 单次 write_file content > 600 行 → RAISE CRITICAL,建议拆分
   - plan > 8 步 → RAISE INFORMATIONAL
   - 一次性生成大 JSON/SQL/CSV → 同上
   - **禁止"加需求"**:Pinvou 是审,不是设计师,不应该让方案变大
2. Suppressions 加一条:"任何把方案做大的建议"
3. 角色规则改成 **Customer + Attacker + Engineer 三视角混合**(原 customer + attacker 二视角)

**Lessons learned 升级**:品悟不是"越多建议越好",反而要**克制**——好的品悟应该是"找硬伤 + 实施可行性 check",不是"代用户加需求"。

### 10.5 Skill 加载路径必须跟着 app 走，不能借用户全局目录

v1+v2 早期把 skill 放 `.deepseek/commands/<x>.md`（Claude Code 风格），DeepSeek-TUI `SkillRegistry::discover` 完全找不到——它认的是 `<skills_dir>/<name>/SKILL.md`（Codex 风格 subdir + 固定文件名）。**整段 prompt 工程被白做**——LLM 看到 `/pinvou-review-plan` 时根本没读到任何 prompt，凭通用 review 模式猜。

修法：把 skill 源放进 `pinvou3-app/src-tauri/resources/bundle/skills/<name>/SKILL.md`，`bundle.rs include_str!` 编译时内嵌，`ensure_extracted()` 启动时强制写到 `~/.pinvou3/bundle/skills/`（不依赖 VERSION 对账）。这样 skill 跟着 app 走：装 app 自动有，升级 app 自动同步，卸 app 自动没——**不污染用户全局命名空间**（`~/.claude/` 之类）。

### 10.6 弱模型不会 progressive disclosure，必须 eager loading

DeepSeek-TUI `render_skills_block` 走 Codex 模式：system prompt 只 advertise skill 名字+描述+路径，期望 LLM 主动 `read_file` 读完整 SKILL.md。**强模型（Claude/GPT）会读，本地 Qwen3.6 不会**——它看到 `/pinvou-review-plan` 时凭字面意思猜，输出"方案概要"重述方案。

修法：bridge 加 `read_skill_body(name)` Tauri command，前端 `autoTriggerPinvouReview` 把完整 SKILL.md body 直接塞进 user message。LLM 看到的是完整角色 prompt + 硬约束 + few-shot，不需要主动 read。

这是"pinvou3 特有的弱模型适配"——对应到强模型可以省（依赖 progressive disclosure 即可）。

### 10.7 输出 budget 滞后没跟上 vLLM context window 升级

`DEEPSEEK_MAX_OUTPUT_TOKENS=16384` 是 vLLM `max-model-len=65536` 时代决策的安全边界（决策 commit `944190d`）。vLLM 后来升 `max-model-len=262144`（256K），但 `MAX_OUTPUT_TOKENS` 没跟着调，导致 v2 实测让 AI 一次写 600 行 HTML 撞 16K 顶 → write_file content 空 → 去重 guard BLOCK。

修法：`MAX_OUTPUT_TOKENS` 16384 → 65536（4x），给单次输出 64K budget，留 200K 给 input。

**经验教训**：任何"基于硬件/服务配置算出的安全边界"都要在那个配置变化时主动 review——这种数字最容易"忘记跟进"。

### 10.8 名字工程：嘴替 → 品悟（产品身份一致性）

v1 临时叫"嘴替"沿用 pinvou2 用法。v2 实施完整后用户提出："嘴替"太互联网梗、与"品悟"产品定位脱节。最后选 toggle 直接叫产品本名 **品悟**——开"品悟" = 开 app 灵魂能力，类似 Notion AI / Cursor 模式。

**设计哲学**：把功能命名为产品本名，是产品自信的表达。

UI 全部跟进；skill 内部 prompt 写 "你是 **品悟**(代号 Pinvou)"——中文用户感知"品悟"，开发者/log 仍认 "Pinvou"。

### 10.9 审查节制：删"必须 ≥1 行"硬约束

v2 实测 Pinvou 在方案合理时硬凑 finding（俄罗斯方块场景凑了"移动端触屏会乱"违反"禁止加需求"规则）。**罪魁是 skill prompt 里"CRITICAL 必须 ≥1 行"的强制**——逼 LLM 在没硬伤时凑 INFORMATIONAL，凑的方向多半是"加功能"。

修法：删硬凑要求，改成"看不出问题就一行 CLEAR 收尾，不强求"。加 example B（改 README 标题）demo 纯 CLEAR 长什么样。

**经验**：好的审查者会说"今天没什么好审的"，硬凑反而违反审查者本职。

## 11. 决策来源索引

| 决策 | 关键证据 |
|---|---|
| 不做并发品悟 | pinvou2 `docs/gemma4-tool-calling-audit.md`（raise_concern 25%）+ Towards Data Science "Why CrewAI's Manager-Worker Architecture Fails" |
| 做 EXIT GATE blocking | gstack `plan-ceo-review/SKILL.md:2200-2223`（self-deception 防护）+ 本仓库 commit `7b983b6` 回滚 |
| careful 用 hook 不用 LLM | gstack `careful/SKILL.md`（63 行硬编码 pattern）|
| Outside Voice subagent | gstack `plan-ceo-review/SKILL.md:1607-1750`（codex exec + Claude subagent fallback）|
| Pinvou 100% 被动 | pinvou2 `actors/pinvou/CLAUDE.md:38-46`（硬铁律）+ pinvou2 `host/chatroom.py:264-291`（并发响应非主动） |
| Pinvou 角色降级（只在末端） | pinvou2 `docs/task-mode-subagent-architecture.md:46-72`（新架构演进） |
