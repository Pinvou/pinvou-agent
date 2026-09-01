# 代码模式「改动随对话回退」设计方案

> 状态：设计提案，待评审。
> 范围：**仅品悟原生代码会话（Native code lane）**；ACP 会话（Codex/Claude 外部进程）明确不做。
> 关联：`docs/adr/0006-多智能体收缩为会话内主动委派模式.md`（需修订，见 §8）、`docs/code-native-agent-完全体架构设计.md`（qiuYliangM/feat-full-code-mode 分支，本方案移植其 checkpoint 机制）。

## 1. 目标与语义定义

在原生代码会话中，用户可以把会话回退到任意历史 turn 边界节点，**代码状态与对话状态一起回退**：

> 「回退到第 N 轮」= 恢复第 N+1 轮 checkpoint（第 N+1 轮写入之前的工作区状态）+ 对话截断到第 N 轮末尾。

语义细则：

- 粒度为 **turn（一轮用户消息）**，不做 turn 内中间状态、不做单文件 keep/undo。
- 「回退到第一轮之前」（清空全部）= 恢复第 1 轮 checkpoint + 对话截断到 0，UI 文案写明。
- 恢复单位是**执行根**，不是会话（共享执行根的语义见 §6）。
- 代码侧可反悔（恢复前强制打 PreRestore 快照）；对话侧 v1 不可前进（被截段落存 sidecar 留恢复可能，UI 不承诺 redo）。

## 2. 方案选型结论

### 2.1 不启用底座快照（CodeWhale `crates/tui/src/snapshot/`）

底座已有完整 shadow-git 快照体系（pre/post-turn + per-tool 快照、`restore` 带 pre-restore 反悔、`/undo`/`revert_turn`），但不适用于 pinvou 场景：

- `mod snapshot;` 非 pub（`CodeWhale/crates/tui/src/lib.rs:134`），app 使用必须改 fork；
- 标签用进程内 `turn_counter`（`engine.rs`），重启后 `pre-turn:1` 重复，多进程下锚定失效；
- 存储按工作区哈希隔离（`~/.codewhale/snapshots/`），同工作区多会话交错，保留策略 7 天/50 个/500MB 与会话生命周期脱节；
- pinvou 当前显式关闭：`snapshots_enabled: false`（`pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs:1315`），两套体系不并存。

复用底座需 4 处 fork 改动（pub/facade、锚定、会话命名空间、保留策略），按项目公约应优先回馈上游，周期不可控。

### 2.2 移植 feat 分支 app 侧 checkpoint（选定）

以 `qiuYliangM/feat-full-code-mode` 提交 `32b5fdf9e`（`code_sessions/checkpoints.rs`，714 行，含 6 个 Rust 测试 + 前端逻辑测试）为基底移植，**砍掉 ACP 钩子**。机制层（shadow git 操作，约 200 行）刻意照搬底座已验证的安全语义；策略层（按会话存储、turn 锚定、LRU、commands/UI）为 app 新增，是底座没有也不该有的部分。

**去重留痕与后续窗口**：机制层与底座的重复在此留档说明；若上游将来提供稳定锚定 + 会话命名空间的快照 API，机制层可整体替换为调底座，策略层原样保留。评审时需确认上游 roadmap 后再决定机制层是否预留可替换接口。

### 2.3 fork 边界

本方案**零 fork 改动**：checkpoint 在 app 命令层钩子，对话截断复用既有截断链路 + `Op::SyncSession` 重注水，不需要新 engine Op。无需 fork-guard 指纹变更。

## 3. 代码侧：checkpoint 机制（移植）

数据布局（账本根下，随会话删除整体清理）：

```text
<ledger>/checkpoints/
  repo/        # 影子 git-dir（--work-tree=执行根，从不触碰用户 .git）
  index.json   # CheckpointIndex { version, entries }，上限 20 LRU
```

核心语义（移植自 feat 分支，均已有测试）：

- 快照 = `add -A` + `write-tree` + orphan `commit-tree` + `refs/checkpoints/<id>` 保持可达；`autocrlf=false` 字节保真；内容与上一条相同则复用 commit（空 turn 无冗余对象）。
- 恢复 = `read-tree` + `checkout-index -f -a` + `clean -fd`（不用 `-x`，不误删 node_modules 等 ignored 目录）；**恢复前强制打 PreRestore 快照，失败则中止回滚**。
- 影子 exclude 列表（.git/node_modules/target/dist 等）+ checkpoint 目录自身排除（临时会话两根相同时防自递归）。
- 安全闸：session_id validate、checkpoint_id 字符集校验、写操作 work-tree 恒为执行根。
- diff 预览：changes 清单 + unified patch（512KB 截断）。

**移植时的修正**：

1. **turn 计数口径**（必须改）：feat 分支用 `messages.iter().filter(|m| m.role == "user").count() + 1`，会把 tool_result（同样以 `role="user"` 落盘）计入，导致 turn 号错位。改用 `is_user_turn_prompt` 同口径谓词（`CodeWhale/crates/tui/src/runtime_handoff.rs:583`，`deepseek_tui` 已 re-export），与 `8d4d2a991`「编辑重发三层同口径」保持一致。
2. **模块落位**：feat 分支依赖 `code_sessions` 模块拆分，本方案不引入该重构；移植到现有 `codex_acp` 相邻位置或新建 `features/code_checkpoints/`，按架构守卫边界落位。
3. **快照钩子**：只保留原生车道 `chat_with_reservation`（`app/commands/chat.rs`）一处，删除 ACP `codex_acp_prompt` 钩子。

## 4. 对话侧：截断到任意 turn + engine 重注水（新增）

feat 分支没有的部分，本方案新增：

1. **磁盘截断**：`SessionStore` 新增「截断到第 N 轮」方法，复用 `8d4d2a991` 的同口径定位逻辑（`is_user_turn_prompt` rposition）；被截段落写入 sidecar `_rewound_turns.json`（留恢复可能，UI 不暴露 redo）；放行 `looks_like_truncating_overwrite` 守卫（`sessions/transcript.rs:27`）的显式回退路径。
2. **engine 内存态**：不新增 fork Op。回退后**回收该会话 engine 实例**，下次发送时走既有 `Op::SyncSession` 用截断后的 messages 重新注水（注水链路 `features/assistant/engine.rs:1602` 现成）。回退只发生在空闲态，无在途状态可丢。
3. **前端**：每个 turn 边界渲染回退入口（移植 `CheckpointChip.jsx` + `checkpoints.js` 的 `checkpointMapByTurn` 对齐与兜底规则），点击后懒加载 diff 预览（`checkpoint_diff`）+ 二次确认；确认后串行执行：恢复代码 → 截断对话 → 刷新时间线。

**回退命令（新 Tauri command，如 `rewind_to_turn`）编排**：

```text
1. 忙碌门：本会话 reserve_turn；同执行根其他会话忙碌 → 拒绝（§6 防线二）
2. 定位 checkpoint：turn N+1 的 Turn 快照；不存在（LRU 淘汰/快照失败）→ 降级为「仅回退对话」，文案明示
3. restore_checkpoint（内部强制 PreRestore）
4. SessionStore 截断到第 N 轮 + sidecar 备份
5. 作废旧分支快照：index 中 turn > N 的 Turn 条目移除并删 ref（清理性质，失败只 warn；
   被截分支的代码状态已由步骤 3 的 PreRestore 兜底）。conversation_only 降级同样作废——
   对话已截断，turn 复用冲突与是否恢复代码无关。restore_checkpoint 命令不做此作废
   （它不动对话，turn 编号继续有效）
6. 回收 engine 实例
7. 返回 { restoredCheckpoint, rewoundTurns } 供前端刷新与提示
```

> **turn 序号是消息序列的相对位置，不是稳定锚**。回退后重新创作会复用 turn 编号，
> 若不作废旧分支快照，「同 turn 取先创建者」的对齐规则（Rust `entries.find` / 前端
> first-wins）会把后续回退锚到被遗弃分支的旧快照（2026-08-21 设计审阅发现的 P0，
> 已由步骤 5 修复并有回归测试）。截断之外的序号漂移来源见 §12。

## 5. 边界与降级（诚实语义）

| 场景 | 行为 |
|---|---|
| 系统无 git 二进制 | 快照失败 warn 不阻断 turn；该会话无回退入口（ADR-0006 修订后合法的降级） |
| 工作区快照失败（磁盘满等） | 同上，该轮无入口，如实记日志 |
| 目标 turn 的 checkpoint 已被 LRU（上限 20）淘汰 | 入口按 checkpoint 可用性渲染；老节点仅允许「仅回退对话」，文案明示代码不回退 |
| 会话运行中 | 忙碌门拒绝回退 |
| 临时会话（两根相同） | exclude 自递归规则已覆盖（移植自带测试） |
| 空目录创建 | git 只跟踪文件内容，空目录不进快照、不出现在 diff 预览（固有语义，非 bug）；目录内一旦有文件即正常覆盖，回退时 `clean -fd` 连空目录一并清除，恢复语义完整。不做占位文件 hack（污染用户项目） |
| shell 后台任务跨 turn 写入 | turn 边界快照只覆盖快照时刻的工作区；exec_shell 后台任务在 turn 结束后的迟到写入不归属任何 turn，回退可能带上它们（best-effort，不承诺） |
| LRU 预算共享 | PreRestore 条目与 Turn 快照共享 index 上限 20：频繁回退会加速淘汰可用 Turn 快照，老节点更早退化为「仅回退对话」。v1 接受，不分开计价 |

## 6. 共享执行根：三道防线

> v1 实现状态（2026-08-20）：防线二（跨会话忙碌门，含对方会话标题的错误文案）已在
> `rewind_to_turn` 落地；防线三退化为「diff 预览如实展示全部将撤销变更 + 忙碌冲突错误
> 原样上屏」（前端暂无同根会话数据源，条件警示文案未做）；防线一（绑定感知提示）未做，
> 后续迭代补。

回退的恢复单位是执行根；多会话绑定同一项目目录时，回退会撤销该根上的全部变更（含其他会话和用户手动改动）。v1 不做按会话归因（shell 改动无法可靠归因），不做 worktree 隔离（改变产品形态，v2 独立评估），只做知情与防竞态：

1. **绑定感知**：绑定项目目录时检测是否已被其他活跃会话绑定，提示「此目录正被会话 X 使用，改动与回退会互相影响」，不禁止。
2. **跨会话忙碌门**：回退前按执行根查询所有会话的 turn 预约状态，任一在跑即拒绝（EnginePool 预约机制扩展为按根查询）。
3. **如实预览 + 确认文案**：diff 预览展示将撤销的全部变更；同根存在其他会话时确认弹窗追加明确文案。

## 7. UI 与文案

- 回退入口只在代码会话页（聊天页不做，与 feat 分支一致）。
- 入口按 turn 边界渲染；无 checkpoint 的 turn 渲染「仅对话回退」变体或不渲染。
- 确认弹窗三要素：将撤销的变更摘要（added/modified/deleted 计数）、对话将截断到的位置、共享执行根警示（条件出现）。
- i18n：全部文案走 `pinvou3-app/src/shared/i18n.js`，中英日三语（移植 feat 分支已有文案并补齐对话截断部分）。

## 8. 需要同步的文档与决策

- **修订 ADR-0006**：将「不恢复每轮快照、不强制 git 依赖」修订为「原生代码会话可选 checkpoint；git 不可用时诚实降级，不阻断对话」。
- `docs/fork-modifications.md`：无需变更（零 fork 改动），但在本设计文档留档「未复用底座 snapshot 的理由」（§2.1）。

## 9. 测试计划

- Rust：移植 feat 分支 6 个 checkpoint 测试（往返/LRU/反悔/越界/嵌套两根/忽略表）；新增 turn 计数同口径测试（tool_result 不计入）、截断+sidecar 备份测试、跨会话忙碌门测试。
- 前端：移植 `codex_checkpoints_logic.test.mjs`；新增回退编排逻辑测试（降级路径、确认文案条件）。
- 集成：原生代码会话多轮写入 → 回退中间节点 → 验证工作区字节级一致 + 对话截断位置正确 + PreRestore 可反悔；回退后重发消息验证 engine 重注水正常。
- 门禁：`architecture-guard.py`、npm 测试链、`cargo check` 零新增警告。

## 10. 实施步骤与工作量

| 步骤 | 内容 | 估计 |
|---|---|---|
| 1 | 移植 checkpoint 机制 + turn 口径修正 + 模块落位 | 2~3 天 |
| 2 | 对话截断到任意 turn + sidecar + 守卫放行 + engine 回收重注水 | 2~3 天 |
| 3 | `rewind_to_turn` 编排 + 跨会话忙碌门 + 绑定感知提示 | 1~2 天 |
| 4 | UI（Chip 移植 + 确认弹窗 + 三语文案） | 1~2 天 |
| 5 | 测试链 + ADR-0006 修订 | 1~2 天 |

总计约一到两周。风险集中在截断口径（有 `8d4d2a991` 经验直接套用）和守卫放行两处。

## 11. 明确不做（v1）

- ACP 会话回退（外部 agent 文件改动无记录）。
- turn 内/单文件粒度回退、逐 edit keep/undo。
- 对话 redo（sidecar 仅留数据）。
- 按会话归因回退、worktree 会话隔离（v2 独立评估）。
- 启用底座 journal 分支（物理截断 + sidecar 已满足 v1 语义）。
- **编辑重发与回退的整合**（2026-08-21 审阅补充）：语义上「编辑第 N 轮重发」= 回退到第 N-1 轮 + 发送，但代码会话里既有的 `Op::EditLastTurn` 只截对话、不动代码，会制造本功能要消灭的「对话没了代码还在」。在 turn 锚定的漂移问题（§12）有解之前不做整合；v1 期间代码会话的编辑重发入口如需暴露，必须同样走 checkpoint 恢复，否则禁用并引导到「回退后重发」。

## 12. 已知限制（2026-08-21 设计审阅留痕；同日后两项已落地）

- **compaction 摘要残留**：`merge_compaction_summary` 把压缩摘要合进 system_prompt 并持久化；回退只截 messages，system_prompt 不动——经历过压缩的会话回退后，模型上下文可能带着描述「被截掉的未来」的摘要。v1 落地最低限度方案：`rewind_to_turn` 返回 `hadCompaction`（按底座 marker 字面量检测），前端回退后如实提示「该会话经历过对话压缩，回退后上下文可能仍含早期摘要」；system_prompt 同步修正后续迭代评估。
- **compaction 导致 turn 计数漂移**：`count_user_turns` 按当前消息序列计数，compaction 替换序列后 user prompt 数量变化，checkpoint 的 turn 号（压缩前计数）与新序列错位，chip 对齐可能失准。turn 序号漂移有三个来源（截断/压缩/编辑），本设计只处理了截断（§4 步骤 5）。彻底解法是持久化稳定 turn 锚（动存储格式），v2 评估。
- **代码反悔缺 UI 入口**：已落地（2026-08-21）。`rewind_undo_state`（可反悔 ⟺ sidecar 有备份 + 回退后未发新轮次/尾部未被编辑 + 绑定的回滚点仍在）+ `undo_last_rewind`（恢复代码到该次回退绑定的 PreRestore + 对话从 sidecar 还原 + engine 重建；restore 自身会再打 PreRestore，反悔可再反悔）+ 时间线「撤销回退」chip（发过新轮次自动消失）。
- **他会话基线漂移**：共享执行根的其他会话在回退后继续创作时，其下一次快照会以被回退后的工作区为基线，其 checkpoint 序列语义已悄悄改变。无解，与 §6 的「恢复单位是执行根」声明一致。
- **undo 目标绑定（2026-09-01 复审修复）**：早期实现以「最新 PreRestore」为反悔目标，多次回退/反悔重试后会错配到无关快照（降级回退的 undo 甚至会误动代码）。现为精确绑定：`_rewound_turns.json` 记录本次回退强制的 PreRestore id（降级记 None，undo 只还原对话），并要求当前 transcript revision 精确匹配截断时记录（turn 数相等只是弱代理，尾部被编辑即拒绝）。绑定快照被 LRU 淘汰则整体不可反悔，如实不渲染入口。
- **敏感文件不进快照**：影子 exclude 列表含 `.env`/`.env.local`/`.env.*.local`、`*.pem`/`*.key`/私钥本体等模式（非 git 执行根没有 .gitignore 兜底，原文快照进影子 objects 并随 diff 进入 UI 链路不可接受）。模式收窄到秘密实际居住的约定：`.env.example`/`id_rsa.pub` 等常被有意提交的示例/公钥照常进快照；`.env.production` 类约定文件仍会进快照。代价是命中的文件不随回退恢复，属可接受取舍。存量影子仓库由 `ensure_repo` 的 marker 门控一次性迁移（`git rm --cached -f`，任意深度）覆盖；legacy 快照 tree 里的原文条目在 restore 时（read-tree 后）再次清除、在 diff 预览中按同模式过滤（清单与 patch 都不上屏）；历史 commit objects 里的原文随 LRU 淘汰与 gc 回收。
- **临时会话账本暴露在执行根内**：两根相同时 `checkpoints/` 位于 agent 可见的工作目录内，agent 的 shell 可看到甚至误删它（exclude 只保证不进快照/不被 clean）。用户会无感知地失去回退能力；把账本根挪到执行根外属后续迭代，v1 记录为已知风险。
- **undo 后对同一节点再回退退化为仅对话**：回退会作废 `turn > keep` 的全部 Turn 快照（含恢复目标 turn N+1），undo 只还原代码+对话、不重建 Turn 快照；此后再次回退到第 N 轮找不到目标快照，只能仅回退对话。方向保守正确（胜过锚到被遗弃分支），重新创作新轮次后快照自然重建。
- **悬停入口的触屏可达性**：回退入口平时是淡色细线、hover 显形（桌面鼠标语义）；纯触屏无 hover，需首 tap 触发 `:hover` 再点按。鉴于 rewind 命令桌面专属（web 策略锚定测试锁定），v1 不为触屏加交互复杂度。
- **Web 车道不支持**：rewind 直接改写本地文件，`rewind_to_turn`/`undo_last_rewind`/`list_checkpoints`/`checkpoint_diff`/`restore_checkpoint` 均未加入 web access-policy 的 allowed_commands，relay 下 invoke 抛 commandNotAllowed、前端静默不渲染入口。放行需单独评估（桌面执行语义），由 `codex_checkpoints_logic.test.mjs` 的策略断言锚定。
