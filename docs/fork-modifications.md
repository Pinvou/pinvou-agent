# CodeWhale Fork 修改清单

## 已合入维护分支：压缩检查点角色兼容性

- T7：将带类型的 Agent 拓扑检查点放到最近一次真实用户输入之后、该轮 assistant/tool 链之前，包括压缩摘要位于链尾的情况。Chat Completions 发送前把结构化识别的当前格式摘要移到其保留轮次之前，并把该摘要与拓扑检查点合并进这条 user 出站消息；普通用户引用摘要标记不会触发重排。修复范围是压缩流程自身产生的尾部 `tool -> user`，客户报错原文为 `SSE stream request failed: HTTP 400 Bad Request: response_role: "user"`（SSE 载体）；存储历史及工具调用/结果 ID 不变，会话固定的系统前缀也不变。
- 未覆盖的相邻真实 user：多次压缩后保留的**真实**用户消息之间因中间轮次被裁掉而在出站线上相邻（`retained_user_messages` 默认保留 20k token 的用户消息，测试 `compaction_does_not_merge_unrelated_adjacent_user_messages` 固定 `[user, user(summary+prompt), assistant, tool]`）。本修复刻意不合并真实用户轮次——那会改变模型看到的轮次边界——因此若某路由的模板同样拒绝相邻 `user -> user`，该路由在多次压缩后仍会 400。现有证据只有上面那条尾部形态的报错原文，未做客户服务重放，无法判定模板对相邻**真实** user 的容忍度；该场景需要路由相关的出站测试与修复，属本 PR 范围外。
- 父仓配套：`pinvou3-app` 的模型服务错误分类把两种载体的 role/模板 400 都归为请求格式卡片且不提示重试——SSE 包装的 `HTTP 400 ... response_role: "user"` 与流式之外的 `Invalid request (400): ...`（`LlmError::InvalidRequest` 的 Display 文本）。卡片文案只说明请求格式被拒，不声称具体线形已修复，因此上述未覆盖场景不会误导用户。分类器按文本形态匹配、刻意保持宽松：含 `server error` 字样的 400 体仍归可重试的 `server` 类，带引号的 JSON-KV 形态 `{"response_role":"user"}` 不命中 `response_role\s*:` 分支而落回未分类。这两处宽度已登记，如需收紧另开改动。
- 这是可复用的 CodeWhale 修复，已在 `Pinvou/CodeWhale#62` 通过审核并 squash 合入 `pinvou3-clean`（`2ab5e64b5`）；审核证据取自候选分支 head `abadae45d`（其已把维护分支 `92427bd8d` 并入），候选树与 squash 提交树同为 `32880a95`，内容等价。修复前持久化的会话由该修复在会话载入/恢复时自动修正，不再要求新建会话，也没有独立的迁移工具。父仓 gitlink（`7fc36e587`）已包含该提交，公开可达性门禁按过渡期口径验证通过。
- 回归测试 `forkguard_compaction_topology_preserves_tool_round_boundary` 覆盖空/活跃 Agent 拓扑及重复压缩；`forkguard_compaction_tool_round_has_valid_chat_wire_roles` 校验完整出站角色序列和工具 ID；`compaction_marker_quoted_by_user_keeps_wire_order` 覆盖普通用户引用标记；`forkguard_restored_pre_fix_session_has_valid_chat_wire_roles`、`restore_replaces_a_pre_provenance_carrier`、`replay_replaces_a_restored_topology_carrier_instead_of_stacking` 覆盖修复前会话的恢复修复，`restore_keeps_a_user_turn_that_quotes_the_summary_header` 与 `compaction_checkpoint_is_never_the_edit_target` 覆盖恢复层删除边界与 `/edit` 归属。既有拓扑测试继续覆盖终态和用户仿冒消息。
- 审核跟进 `0d679a5e8`：检查点 carrier 改按结构识别（首块以摘要头开头、其余块为引擎写入的来源块），恢复层把遗留 carrier 移到放置锚点、再次压缩丢弃被取代的已恢复 carrier 而不是叠加第二份；保存期放置、恢复修复与请求期重排现在共用同一套锚点定义（此前三份实现已在「哪些消息算 prompt」上漂移）；0.9.6 之前的旧 header carrier 同样被识别与重排；普通用户粘贴完整摘要头不再在恢复时被静默删除，且在被覆盖统计豁免的路径上也不丢；程序生成的检查点不再被 `/edit` 当成可编辑轮次。
- 范围边界：本 PR 不额外提供历史迁移入口；修复前持久化的会话在会话载入/恢复时按上述结构识别与放置锚点自动修复。手动 `/compact` 的旧风险（恢复后的拓扑检查点仍留在工具结果之后）随之消失；本 PR 不覆盖已载入内存副本的在线迁移，也不做客户服务重放验证。压缩流程之外，后台子 Agent 的完成、失败或等待事件，以及 LSP 诊断、步骤预算、子 Agent 协调、输出截断和基准预算提示，都可能在工具结果后产生 `tool -> user`。真实用户的中途补充和中断恢复也可能形成相同序列，但不能在不改变用户意图的前提下重排。这些独立场景需另行设计路由相关的出站测试与修复；CodeWhale 仓库未启用 Issues，暂记录在本清单与 PR 审核讨论中。
- 已知后续项（CodeWhale 侧，非本 PR 阻塞，同样记录于此）：`is_compaction_summary_text` 在 `0d679a5e8` 之后失去最后调用者成为死代码（`pub`，无编译告警）且其文档注释仍描述为在用；`chat.rs` 中退化孤儿 tool-result 分支的注释仍称该请求保留严格模板拒绝的线形，实际会丢弃孤儿并只发一条 `user`；以精确 header 开头的真实轮次在「无系统提示检查点」的恢复路径、以及 `retained_user_messages` 生成的单块保留副本在下一次压缩时被丢弃这条链路，仍缺回归测试。

> 本文是 Pinvou 对 CodeWhale fork 的单一现状清单。
> 维护策略见 [`fork-policy.md`](fork-policy.md)，升级证据见 [`codewhale-upgrade-0.9.5-to-0.9.12.md`](codewhale-upgrade-0.9.5-to-0.9.12.md)。
> English: [`fork-modifications.en.md`](fork-modifications.en.md)

## T2: shell environment guidance (integrated upstream as CodeWhale #50)

- Integration status: resolved by the 2026-09-11 backlog batch. [CodeWhale PR #50](https://github.com/Pinvou/CodeWhale/pull/50) merged the re-ported candidate as `1d9ee26e6` (a re-port of [fork PR #42](https://github.com/Pinvou/CodeWhale/pull/42) onto v0.9.12 r1), and parent PR #482 advanced the parent gitlink to `ae7e3fb36f89486f30d41b28ae0eaaa516ae4740`, which includes it. The public-submodule gate passes in transition mode: the immutable `pinvou-v0.9.12-r1` tag stays at `1fafee7e26b60a59457a43bce50c63aa2ad9dbaf` while the gitlink rides the public maintenance branch ahead of it, so this PR no longer pins a separate candidate ref.
- Review: [Pinvou/CodeWhale#42](https://github.com/Pinvou/CodeWhale/pull/42), from `zhuowp/CodeWhale:fix/shell-environment-guidance`. The reusable upstream contribution is [Hmbown/CodeWhale#5900](https://github.com/Hmbown/CodeWhale/pull/5900); its source review and compiled verification are separate from the fork integration that has now landed.

- The `Bash` tool description and `command` parameter now share guidance from the existing execution dispatcher. PowerShell, cmd, POSIX sh, Bash, zsh, and other custom shells receive matching syntax guidance. Tool names, permissions, execution, aliases, and read-only argv behavior remain compatible. On the v0.9.12 re-port, the model-visible lowercase `bash` contract surface is unchanged; extending the aligned guidance to that surface is a separate scope decision for review.
- The shell guidance fix is one of the registered post-r1 commits (CodeWhale PR #50, `1d9ee26e6`) inside parent gitlink `ae7e3fb36f89486f30d41b28ae0eaaa516ae4740`; at review time the candidate drifted 3 files, +207/-2 above the r1 head. Upstream main still contained the login-shell-only description when inspected on 2026-09-05.
- Coverage: `shell_guidance_matches_each_interpreter`, `forkguard_shell_catalog_guidance_matches_execution`, and the opt-in `export_shell_guidance_eval_fixture` for live model comparisons. The application removes Unix-specific default examples from shared instructions, browser HTTP verification, and attachment analysis guidance.
- Live-model methodology, measured results, and verification limits: [shell guidance evaluation](shell-guidance-evaluation.md).
- Review follow-up: preserve zsh's `=command` warning and provide Bash/POSIX quoting, pipelines, heredocs or syntax exclusions, and utility-portability guidance. Unix `$SHELL` paths represented as `Custom` receive the same guidance as built-in Bash/sh variants; cmd and fish have their own syntax notes. Covered by `shell_guidance_preserves_unix_shell_contracts`.
- Guidance selection uses one `match`, with a PowerShell-family guard shared with execution and a dedicated text constant. This structural refactor preserves custom PowerShell detection and model-visible wording.
- All shell-specific guidance now lives in named constants, including cmd, fish, and the shared fallback. A before/after comparison across 14 shell cases confirmed identical output after constant extraction.
- Following the 40-call curl ablation, remove the tool-level curl alias reminder only. Preserve the other PowerShell guidance and application instructions used in that experiment; both arms achieved 19/20 correct executions with no shell mismatch errors.

## 0. 当前状态（2026-09-21 · r2 已收口（tag `pinvou-v0.9.12-r2` @ `6f780290f`）+ workspace_roots 主题过渡叠层；上游起 73 个提交）

| 项 | 当前值 |
|---|---|
| 上游基线 | tag `v0.9.12`，commit `dcd4c200f72f0c1ffd60d8e7f6850313db879fc5` |
| 维护分支 | `pinvou3-clean` = `6f780290f1c35e8a3c5dff86b4f76da142744b0c`（r1 基线 `1fafee7e2` 之上 24 个 squash 合入：#41/#47/#49、2026-09-10/11 遗留 PR 清理批次 #31/#37/#38/#39/#43/#48/#50/#51/#52/#53、2026-09-17 批次 #56/#58/#59/#60/#61、2026-09-18 批次 #55/#57/#62、2026-09-20 批次 #66 与 2026-09-21 批次 #64/#67） |
| 发布状态 | r2 已收口（2026-09-21，main 经 #574 完成）：不可变 tag `pinvou-v0.9.12-r2` 切在 `6f780290f`，main 上维护分支/gitlink/tag 三方相等，但 r2 不含 workspace_roots 主题；本 PR 继续过渡期叠层（fork-policy 第 0 节豁免）：父仓 gitlink 钉主题 head `bbc90540a`（已 rebase 到 r2 收口 `6f780290f` 之上，41 个主题提交，领先 r1 tag 65 个提交），CodeWhale #54 squash 合入、`pinvou3-clean` 推进并重钉后回收过渡期叠层；不可变 tag `pinvou-v0.9.12-r1` 保持 r1 收口状态（15 个提交），仍为回退点 |
| 升级前回退点 | 公开不可变 tag `pinvou-v0.9.5-r13` → `f853f8f1566c57e6be40d5439a222a932aa79ef5`；同 SHA 的本地 `backup/pre-v0.9.12-sync` 仅作便利引用 |
| 历史组织 | 上游之上 73 个带 DCO sign-off 的提交（其中 37 个为 workspace_roots 主题、现居 fork 分支 `pinvou3/workspace-roots-v12`，待 CodeWhale #54 squash 合入），归属 4 个长期主题（T1–T4）+ 2 个追加减量主题（T5 会话归档导出、T6 蜂群限流治理）+ 1 个已合入维护分支的主题（T7 压缩检查点角色兼容）+ 1 个过渡期在册主题（workspace_roots，见下节）；r1 之后 58 个提交全部经 PR 审查，合入维护分支者均过五项必需门禁 |
| drift | `190 files, +19135/-2628`，净增 16507 行（维护分支对上游，实测于 r2 收口 `6f780290f`）；过渡期另加本主题 `57 files, +4397/-268`（r2 收口 `6f780290f` → `bbc90540a`，主题分支已 rebase 到 r2 之上）；r1 为 `94 files, +5022/-944`，旧 r13 为 `110 files, +10895/-1195` |
| 守护 | CodeWhale `forkguard_*` 独立行为名实测 **139**（于登记的 gitlink `bbc90540a`，即 `scripts/fork-guard.sh` 实际执行的计数）：r2 收口基线 133 + 本主题新增 6 条 `forkguard_workspace_roots_*`；其中 6 条钉在 `benchmark-eval-controls` 门控面，9 条为 #66 随维护分支在底座新增的 PowerShell 相关行为名，其余为 T1–T7 等存量。登记下限 96 + 父仓指纹与行为测试（workspace_roots 指纹锚点已随本 PR 注册进 `scripts/fork-guard.sh`） |
| 父仓适配 | v0.9.12 EngineConfig、Agent/Plan 模式、逐轮 reasoning/安全、ExtraTools、owner 事件隔离、Automation v3/v4 数据兼容、rusqlite 0.40.2、Shell 任务来源对账；消费方 PR #396（execpolicy）、#408（轮次取消）、#444（蜂群）、#468（computer-use）、#472（一键导出）依赖本批底座能力 |

### 多根工作区 workspace_roots + 指令 source 相对化（CodeWhale PR #54 head `bbc90540a`，已 rebase 到 r2 收口 `6f780290f` 之上）

- CodeWhale 分支 `pinvou3/workspace-roots-v12`（`qiuYliangM/CodeWhale` fork，PR #54 head `bbc90540a`；已 rebase 到 r2 收口 `pinvou3-clean` @`6f780290f` 之上——即 r2 收口维护线头 + 41 个主题提交（第 8/9/10/11 轮修复提交与 r2 语义合并修复；更早的 `88405ffbc`（rebase 到 `92427bd8d` 的 23 提交线）与 10 提交中文信息线 `f2526196c` 均已被本线取代））：线程从单根 `cwd` 扩展为 **cwd（主根）+ workspace_roots（全量可访问根集合）**，是"单入口工作区"（项目 = 主文件夹 + 一组钥匙）的底座前提。四层落地：
  1. **协议与会话模型**：`ThreadStartParams`/`ThreadResumeParams`/`ThreadForkParams` 与 `Thread` DTO 增加 `workspace_roots`（serde default，旧载荷读入为空）；SQLite `threads` 表 v5 迁移加 `workspace_roots TEXT`（缺省 `'[]'`，旧库零迁移退化）；TUI 两侧 JSON（`ThreadRecord`/`SessionMetadata`）additive serde default 字段。
  2. **回合环境**：根集合经 `Op::SyncSession` / Runtime API `UpdateThreadRequest` 运行中替换（活动回合拒绝 + 驱逐缓存引擎），每回合读取——快照语义，下回合生效；`normalize_workspace_roots` 保证 cwd 居首去重，空集合 ≡ `[cwd]`。resume 三态：带 roots 整体替换；只带 cwd 替换主根槽位、附加根保留；都不带沿用持久化值（顺带修复 cwd 被 fallback 覆盖的缺陷）。v12 适配：`SyncSession` wire 镜像、exec_agent 装配、acp_server 接缝均已接线。
  3. **权限沙箱**：每回合策略构造点直接物化全量根集合——`workspace_write_policy(workspace, roots, network_access)` 的 writable_roots = `normalize_workspace_roots(workspace, roots)`（空集合逐字节等于旧值 `[workspace]`；改写线已放弃早先的 `:workspace_roots` 符号常量设计，无配置面消费该字面值）；写豁免 carve-out 逐根判定（排除名仍拒）；`ToolContext::resolve_path` 跨根放行、真越界仍 `PathEscape`；execpolicy 规则跨根匹配（allow 规则保持主根作用域）。v12 的 `SandboxNetworkAccess` 类型化参数与 fork 反转点全部保留。
  4. **提示词/项目指令**：AGENTS.md 发现仅主根（v12 的仓库边界回溯语义保留），附加根只给访问权不注入指令；`<project_instructions source="…">` 标签仅文件名化（统一 helper `project_instructions_source_label`，位于 `project_context/types.rs`）已由 `pinvou3-clean` #59 吸收为共有行为，本主题不再携带该 diff，只保留"指令发现仅主根"的语义与回归测试；目录搬移/换主根且指令内容相同者不再击破 KV 前缀缓存。v12 差异：compaction 逐字重注入点已在上游重构中消失，旧线对应测试未移植。
- 行为变化边界：未配置多根（空集合）时协议帧、策略值、路径判定逐字节等价单根现状；TUI 交互端无多根 UI，根集合只能经 Runtime API/headless 进入。第 8 轮收口：`ThreadForkParams.workspace_roots` 改为 `Option<Vec>`，裸 `thread/fork` 继承父线程集合（cwd 换主根槽位、附加根保留，`Some([])` 为显式清空），不再退化到 `[fallback_cwd]`；app-server 桥的 runtime 线程创建带全量根集合；repo law 逐根判定；`/save`、`/rename`、导出导入、`/fork <id>` 全部与主根成对落盘。
- drift：`pinvou3-clean` @`6f780290f`（r2 收口）→ 本主题 head `bbc90540a` 为 `57 files, +4397/-268`（第 8 轮在上一次登记 `54 files, +3278/-240` 之上补齐 ACP 会话/加载通道、`/title` 写路径、repo law 逐根判定与证据非空洞化等修复，第 9/10/11 轮继续补齐评审修复（hint 记录全域 missing 防护 + 根集 PATCH 驱逐测试），rebase 到 r2 后底座已含的 #66 cherry-pick 等内容退出 diff）；本主题新增 6 条 `forkguard_workspace_roots_*` 行为名（source 相对化那条已被 #59 收进 `pinvou3-clean`）；全仓独立 forkguard 行为名实测 139（r2 线 #55–#67 在底座亦有新增），登记下限随本 PR 的 `scripts/fork-guard.sh` 提到 96。另含 turn_meta 列根：多根会话的每回合元数据新增 `Accessible folders:` 行（紧贴 `Current workspace:`，最多 5 个超出截断，单根逐字节无此行）——模型获知附加根的通道。
- 指纹锚点：`pub workspace_roots: Vec<PathBuf>,`（protocol）、`pub fn normalize_workspace_roots(`（core）、`ADD COLUMN workspace_roots TEXT NOT NULL DEFAULT '[]';`（state）、`writable_roots: codewhale_core::normalize_workspace_roots(workspace, workspace_roots),`（core/authority.rs，每回合策略构造点物化）及 6 条 `fn forkguard_workspace_roots_*` 测试名。

### 轮次绑定取消：宿主 stop 按轮身份分派（父仓适配，本 PR）

- 底座半边已并入登记批次：CodeWhale PR #38（squash `f5c68cab8`，见第 3 节提交序列与 T1 保留内容）把共享 cancel 槽升级为 `TurnCancelSlot { turn_id, token }`，并提供宿主入口 `EngineHandle::cancel_turn(turn_id, reason, mode) -> bool`（身份校验、steer 处置与 token 克隆在同一把槽锁内完成）与收口期 `EngineHandle::publish_stop_disposition(reason, mode)`（只发布 stop 处置、绝不触发任何 token）。
- 底座候选半边（复审轮新增，封闭边界 ③）：`Op::SendMessage` / `Op::EditLastTurn` 增加宿主提交关联字段 `submission_id`，由该轮的 `TurnStarted` 逐字回显；全部运行时自启路径（idle 子代理完成、后台 shell 唤醒、goal 延续、composer shell 命令）恒回显 `None`；线协议 `EventMsg::TurnStarted` 携带同一增量字段（`serde(default)` 缺省缺席，向后兼容）。回归 `forkguard_turn_started_echoes_submission_id_self_starts_stay_none` 钉死回显契约：宿主提交轮的 `TurnStarted` 回显自身令牌、自启轮恒 `None`。该提交已随 #58 以 squash `f05b9acfa` 合入（2026-09-17），并随 2026-09-17 批次把公开 `pinvou3-clean` 推进至 `92427bd8d`。
- 父仓配套（本 PR，与底座共同封闭 pinvou-agent#254）：`EnginePool::cancel` 的取消闭包在 `arm_pending_cancel_and_cancel` 的同一 state 锁临界区内领取 (epoch、已观察 turn_id、收口期) 同源身份快照，经 `turn_bound_cancel_action` 纯函数分派——绑定命中走 `cancel_turn_with_mode`（槽已切换时底座整体跳过、不发布处置）；目标轮尚未被 forwarder 观察到（submit→TurnStarted 窗口）与终态收口期收敛为仅 `publish_stop_disposition`、绝不开火——窗口内引擎槽可能已是延迟观察到的自主续跑轮活 token，无差别开火即 #254，真正在途的目标轮由同临界区内 arm 的 `pending_cancel` 在 TurnStarted 后按 turn 绑定重放取消（槽已切走则底座身份校验把重放整体丢弃）；forwarder 的 `pending_cancel` 重放改为 turn 绑定，且仅当事件回显的提交令牌与武装时记录的一致才消费重放：宿主提交路径（用户发送、多智能体、评测重放、编辑上一轮）经 `Pinvou3Bridge::next_submission_id` 统一铸造 `sub-<uuid>` 令牌并在武装时记入 lifecycle，超越自启轮的 `TurnStarted`（无令牌或异令牌）既不能消费重放、也不能把取消引到自身，宿主提交轮自身的回显到达时重放按 turn 绑定精确落地（app 回归 `overtaking_self_started_turn_started_cannot_consume_the_replay`）。消费闸门仅由提交令牌构成：武装时记录的 epoch 仅作溯源、不参与消费判定——窗口内无关自主轮可以完整跑完 start→terminal 生命周期，使目标轮自身的 `TurnStarted` 以 newly-active 身份推进 epoch，若以 epoch 为消费闸门会在回显精确匹配时误拒、静默丢失用户 stop（app 回归 `pending_stop_replay_survives_an_autonomous_lifecycle_before_the_target_starts` 按 started→terminal→目标 started 的 forwarder 顺序钉死该时序）。武装缺令牌今日不可达（全部宿主提交路径均铸造令牌），arm 站点会发出 `log::warn`，且无令牌重放与任何回显（包括 `None`）都不匹配、永不可消费（回归 `pending_cancel_armed_without_submission_id_is_never_consumed`），消费闸门因此不可能被静默重开。已知边界：① 级联取消（`Op::CancelSubAgents`）未随裁决收敛，绑定跳过时仍取消引擎当前全部子智能体（N 的遗留清理是停止契约，与 N+1 刚派生的子智能体在 app 侧不可区分）；② 入口即 idle 的 backstop（stop=clear 契约）刻意保留无绑定开火——该路径无目标轮可裁决，若引擎已自主续跑，命中其活 token 是 clear 契约的预期语义，瞄准已结束轮的 stop 入口快照为 Some、不会走到该分支；③ 原登记的追击超越窗口（自启后续轮的 `TurnStarted` 抢先消费重放并把取消引到自身、宿主 stop 静默丢失）已由本轮底座提交关联回显 + 父仓回显匹配消费闸门封闭（见上两条），该边界不再保留；④ 计划轮监督器（`wait_for_scheduled_terminal`）保留其先于 turn 绑定契约的无绑定 `cancel_with_reason(External)` 开火（自动化到期/停止权威，语义即整体叫停引擎当前活动）：开火点在观察到计划轮自身 `Started` 信号之后；先于该观察到达的取消请求会先闩锁，待 `Started` 观察到时才开火（等待超 30 秒则按 Timed out 放弃），落在该观察间隙内的自启续跑会承受这次无绑定命中——非本 PR 引入，保留原状；改走 turn 绑定 `cancel_turn` 属自动化契约变更，留作后续工作；⑤ 引擎回收（`reclaim_engine_entry` 经 `engine.cancel_current()`，删除/换模型/闲置收割）保留无绑定开火：回收即引擎拆除——条目已先行摘除、forwarder 随即中止并按需补发 Interrupted 终态、`Op::CancelSubAgents`+`Op::Shutdown` 按同通道 FIFO 收尾，轮次随引擎消亡，不存在「错误轮幸存」，若引擎已自主续跑而 forwarder 未观察，命中其活 token 属回收契约的预期语义。另一处已知有损角落：stop 武装后，若一条超越的自主续跑先跑完 start→terminal 重开 reserve 闸门、用户又在目标轮自身 `TurnStarted` 被处理前发送新消息，`reserve()` 的整体清理会丢弃该武装（重放无回显可匹配、静默失效）；窗口极窄且可恢复——目标轮真正开跑后再次 stop 即按 BoundTurn 精确命中。
- 候选登记已回收：#58 落地后父仓 gitlink 随登记批次重钉（回收时为 `92427bd8d`，`EXPECTED_COMMITS` 相应推进），`CANDIDATE_HEAD`/`CANDIDATE_COMMITS=29` 机制移除；其后 #552 批次已把登记头推进至 `7fc36e587`（`EXPECTED_COMMITS` 36，见第 0 节表格）；指纹层保留底座回显契约回归与父仓超越序回归两条锚点；`verify-public-submodule.sh` 过渡期口径为断言 gitlink = `pinvou3-clean` 分支头：本 PR 叠层期间（gitlink = 主题 head `bbc90540a`）该门禁保持红并在 PR 描述中披露，待 CodeWhale #54 squash 合入、`pinvou3-clean` 推进后重钉恢复绿。

## 1. 为什么本次使用 clean re-fork

旧 r13 相对 v0.9.5 修改 110 个文件。与 v0.9.12 对照时，104 个旧修改文件也被上游改动，直接三方移植预计产生 57 个冲突文件。与此同时，上游已经吸收或重构了大量旧 patch，包括会话快照/恢复、编辑上一轮、压缩后 token、provider/model 路由、原生搜索、Windows UTF-8 Shell、JSON schema 修复与任务基础设施。

因此本次从官方 `v0.9.12` tag 直接建立新分支，只重表达仍然缺失且必须位于底座生命周期内的语义；没有 merge 或 cherry-pick 旧 r13 冲突树。

## 2. 旧 r13 逐项处置

| 处置 | 能力 | v0.9.12 r1 结果 |
|---|---|---|
| 上游已有，删除 fork | 会话 snapshot/recovery、edit-last-turn、post-compaction tokens、route budget、严格直连模型大小写、JSON 容器修复、厂商原生搜索、Windows UTF-8、provider pin 等 | 采用上游实现和测试，不复制旧代码 |
| 语义迁移 | `Yolo`/`Auto` 模式、全局 reasoning、旧 custom-tools/disabled-skills 接口 | 映射到 `Agent` + approval/trust、逐轮 `Op::SendMessage` reasoning、`ExtraTools` 与显式 Skills 根 |
| 继续保留 | 宿主 facade/route limits、可靠 steer 与批量取消、MCP secret resolver、raw worker ledger、逐轮最终分发安全、64 KiB File 上限、prompt ownership（含 locale bookend 瘦身接线，只约束回复语言、不约束思考语言）、Automation conversation/schema/lifecycle | 重写到 v0.9.12 当前 Engine/Task/Prompt 结构并增加结果式测试 |
| 继续保留但默认关闭 | r13 benchmark eval controls | 在 v0.9.12 架构上恢复 final-only-after-tool-budget 与无歧义 missing-read-action repair；仅由 benchmark feature 显式启用，桌面默认路径关闭，父仓 `benchmark-hooks` 负责编排与观测 |
| 继续保留 | #35 API 搜索链直接以 Bing 收尾的覆盖 | 配置型 API backend 失败后直接落到免密 Bing。DuckDuckGo 的内部 Bing fallback 只覆盖空结果/challenge，不覆盖连接失败；在 DuckDuckGo 不可达的网络中把它作为外层尾部会提前返回错误，因此恢复直接 Bing 尾部并以结果式测试锁定全部 API provider |
| 采用上游删除 | stuck/read-repeat/coaching guards | 接受上游 `b39cf5650` 的处置：旧 stuck 指纹不含结果摘要，会把活跃 job 的重复 poll 误判为无进展；不恢复旧 guard 或相关环境开关，继续依靠有限 `max_steps`、工具预算和取消边界 |

### 上游测试处置注记

以下上游测试不是静默删除，而是因为 Pinvou 已登记的产品语义与上游默认语义不同而替换：

| 上游测试 | v0.9.12 r1 处置与原因 |
|---|---|
| `full_access_auto_approves_non_bypassable_registered_tools` | 高层执行测试替换为 `full_access_blocks_non_bypassable_registered_tools_without_prompting`；Full Access 不得绕过注册工具的 non-bypassable approval。上游 resolver 的低层对照测试仍保留，反转只发生在 Pinvou 最终 Engine dispatch 边界 |
| `discover_for_workspace_and_dir_merges_workspace_and_configured_sources` | 恢复上游默认 merge 路径测试；另以 `forkguard_explicit_skills_dir_excludes_ambient_workspace_sources` 覆盖 Pinvou 宿主选用的显式单根路径 |
| `system_prompt_merges_workspace_and_configured_skills_dir` | 恢复上游默认 composer 测试；另以 `forkguard_system_prompt_uses_only_explicit_configured_skills_dir` 锁定安装宿主 composer 后的 ambient workspace 隔离 |

## 3. 当前提交序列

| Commit | 主题 | 说明 |
|---|---|---|
| `38dd961ea` | T1 | 重建 v0.9.12 宿主 facade、路由与嵌入边界 |
| `a5c12e203` | T2 | 保留工具兼容、MCP secret 与执行期安全入口 |
| `7dc1a429a` | T3 | 恢复宿主静态 prompt 所有权 |
| `02c0faa27` | T4 | 保留 Automation 与 Task 的 Pinvou 运行归属 |
| `b4c02616b` | T1–T4 收口 | 可靠 steer、受限控制面、最终分发、ambient 隔离、宿主 prompt-only profile/显式 Skills 根、生命周期回归及当前 Rust 发布 lint 兼容 |
| `dbd1b7cb3` | T1/T2/T4 评审修复 | 恢复 feature-gated benchmark eval controls，并补齐 64 KiB 写入、session cancel、终态删除、受限轮 idle deferral 与 MCP 隐藏/拒绝的结果式守护 |
| `fe0cd7551` | T1/T2/T3 评审收口 | 恢复上游对照测试并注明产品反转；增加 steer 真实 channel/turn-loop 回归；登记 Permissions fragment 上游化债务 |
| `ff299f94b` | T2 复审修复 | 恢复配置型 API 搜索链的可达 Bing 尾部；移除空的 benchmark observability feature；补回评测兼容路径的设计理由注释 |
| `54819b0d6` | 发布门禁 fast-follow | 为手动精确 SHA CI 补齐 migration manifest 比较基线；修正 rustdoc 私有链接与过时步数说明；将 18 个仅供下游宿主兼容的宽 facade 从生成 API 文档中隐藏，不改变编译 API 或运行行为 |
| `ff9959bfc` | 发布门禁跟进 | 删除已由当前阻断结果式回归替代、且产品语义已明确反转的上游 Full Access 比较 helper；不抬高 dead-code 预算 |
| `6615af7ca` | 文档复审收口 | 删除 4 处指向不存在的 fork-policy 小节引用，直接说明测试所锁定的产品反转与显式宿主边界 |
| `409138dbe` | 运行时契约发布门禁 | 精确登记官方 v0.9.12 `automation`/`tasks` 路由字段与 Agent 精简的净增长，以及 Pinvou r1 写入上限和 host prompt-only profile 的模型可见增长；只抬高 Act/Operate full 的真实上限，同时收紧 active 与 Plan full，工具 identity 不变 |
| `881cf4444` | 精确 SHA CI 收口 | macOS npm wrapper 在可选 sccache 丢失后的冷启动 release build 可使用 60 分钟上限，并以 wiring 回归锁定例外只作用于该 job |
| `baa87f4de` | T4 复审修复 | offline-misfire skip 只作用于 recurring，使过期一次性任务仍精确持久入队一次后暂停 |
| `1fafee7e2` | T2 复审修复 | 所有搜索后端不可用时恢复可操作且脱敏的 provider/config 配置提示 |
| `a7215d3c4` | 门禁修复 | 恢复 pinvou3-clean 分支保护的必需合并检查（#49） |
| `bf435e5e8` | T4 修复 | artifact 清理失败时保持终端任务内存一致（#47） |
| `c3a35216e` | T2 文档 | 模型面工具描述与提示词对齐实际行为，36 个工具文件（#41） |
| `a89048184` | T2 修复 | finance 工具接入网络策略闸门（fail-closed）、verifier 不再教退役工具名、notify 配置方法合同钉、fleet-manager 技能补 `resume`/`stop --all`（#31） |
| `ff4add43e` | T2 增强 | execpolicy phase-2：cmd.exe 单字母斜杠旗标跳过、deny 中段通配符、`.exe` 后缀折叠、绝对路径规则精确匹配、规则集跨克隆活共享、subagent 工具调用过同一 execpolicy 门（#37） |
| `f5c68cab8` | T1 修复 | 共享取消槽绑定轮次身份：`TurnCancelSlot` 原子换装、`cancel_turn(turn_id)` 身份校验+处置+令牌同锁、`publish_stop_disposition` 收口期只发处置不开火（#38，修 #254 底座半边） |
| `3f8a25eef` | 文档 | CHANGELOG 补记 API 搜索链直接无 key Bing 尾部与大陆网络成因（#39） |
| `dd7b7785f` | T6 新增 | subagent 自适应限流：`DynamicGate` 可缩容 launch gate + `RateLimitGovernor` 60s 滑窗 AIMD（≥2 事件或 >30% 减半、≥4 暂停、时间驱动探活自愈），429 重试尊重 Retry-After/全抖动退避（#43） |
| `1d9ee26e6` | T2 修复 | Bash 工具指引与执行同一 shell dispatcher 派生（Windows PowerShell 不再收到 login-shell 指引）（#50，取代 r13 线 #42） |
| `68461e84e` | T2 增强 | 工具结果经 `metadata.images` 携带图片：与用户附件同路径的 `<image>` 三元组、单结果上限 2 张、坏图降级不失败轮次（#48，computer-use 截图回传基础） |
| `09ebba3fa` | T5 新增 | 会话全保真 tar.xz 导出：`session_export` 模块 + `codewhale sessions export` CLI，liblzma 静态压缩，刻意不脱敏与 `/export` 互补（#51） |
| `6ae5b1734` | T1 修复 | GLM-5.3/5.3-Flash forced-thinking：`disabled` 改写为 `enabled`+`low`，effort 别名映射到 low/high/max（#52，默认 Z.ai 模型 `off` 档必现报错的修复） |
| `ae7e3fb36` | T1 修复 | BigModel 通用端点 `open.bigmodel.cn/api/paas/v4` 纳入第一方 Chat 路由谓词，推理控制在两 host 统一（#53） |
| `72e98fd89` | T3 修复 | 模型面文本停指不存在的工具：技能索引 Usage 与子代理 `## Skills` 头教 `tool_search` 两段激活与直呼注水兜底、mcp-discovery 注册命令按可用性自条件并停引退役 `exec_shell`、best-of-n `create_goal` 存在性门控、父上下文提示改引首轮必活 `read`/`bash`、捆绑技能退役/隐藏名清单清扫、web/fetch 工件元数据补 `evidence_available: true`（web_run 溢出、fetch_url 全部落盘工件）使 `retrieve_tool_result` 次轮自动激活、`MAX_REGISTRY_MATCHES` 与「eight」文案编译期互钉（#56） |
| `f05b9acfa` | T1 新增 | `TurnStarted` 回显 host 提交的 `submission_id`（SendMessage/EditLastTurn 携带、全部自启续轮路径一律 `None`、wire 事件同步加字段），主机侧可区分在途提交与越前自启（#58，#254 回放越窗收口的底座半边） |
| `102da17a1` | T3 修复 | `<project_instructions>` 与 repo constitution 来源标签相对化：共享渲染助手统一系统提示与上下文报告，未变更内容的大小写重读不再产生伪 `<context_update>` 追加，绝对路径退出 provider 边界标签（#59，自 #54 拆出；祖先链/项目规则标签维持绝对路径，遗留见父仓 #514） |
| `889fc2f99` | 门禁修复 | 基线重同步（自 #56 剥离的搭车修复）：TUI CHANGELOG 切片补 DDG→Bing 记录、v0.9.12 事实文件与生成页刷新、engine/tests.rs clippy 冗余闭包、telemetry 信任文案改钉 0.9.11 历史 ask-first 先例并以契约测试锚定（#60） |
| `92427bd8d` | T3 修复 | stopship 发布验收侦察轮改走两段激活：一次 `tool_search` 激活 deferred `grep_files` 后按原证据契约检索、第三响应出 verdict，简报停引隐藏别名 `File` 与退役 `search_content`，表面/fixture 文本/行为三条 forkguard 互钉（#61） |
| `2ab5e64b5` | T7 修复 | 压缩交接保持工具轮边界：chat wire 角色合法性校验（压缩轮保持合法 assistant/tool 序列）、重压缩保真实用户边界、压缩轮跨恢复保留、生成式压缩摘要识别、restored 拓扑合并限域，7 条 forkguard 互钉（#62） |
| `ce783728c` | T2 修复 | computer-use 插件：zoom 后按裁剪区在父尺度重绑 raster 帧偏移（子栅格坐标不再错配全图）、ssh 下元素状态宿主侧记忆与 `state_wrong_computer` 校验、recording 与 switch_display/left_mouse_down 在 ssh 显式 fail-closed 并给出可操作原因、zoom 在 ssh 可用（远端裁剪源注入+宿主侧几何重绑）（#57） |
| `7fc36e587` | T6 重构 | DynamicGate 重建于 tokio `Semaphore`（取消授权重派、陈旧等待者跳过不漏槽、缩容低于在途后续再准入），抽取 `is_governor_reported_rate_limit` 谓词并以 forkguard 钉 QuotaExhausted 不进治理窗，删除按成功/限流比例缩门的 ratio 启发式（治理窗缩容只认绝对阈值）、清理失实注释与死分支（#55，#43 评审收尾） |
| `c4e6caf94` | T2 修复 | Windows PowerShell 执行策略兼容：所有 dispatcher 构造的 PowerShell 调用统一携带进程级 `-ExecutionPolicy Bypass`，10 条行为回归迁入 `forkguard_` 前缀；`-EncodedCommand` 内联重跑当前仅引擎同步内部分支可达、尚无生产调用方，组策略/AppLocker 场景待重试门接入前台执行车道后覆盖（与插件 `.ps1` 车道同类的后续改动）（#66） |

所有提交都含 DCO `Signed-off-by`。`b4c02616b` 包含大部分跨主题收口，历史粒度确实不利于 bisect；分支公开进入评审后没有为历史美化 force-push，而是追加带 sign-off 的提交修复评审和发布门禁问题，并用本表、指纹和行为测试弥补审计粒度。不可变 tag 已创建，后续不得重写。

## 4. T1 — 宿主嵌入与路由边界

### 保留内容

- 对宿主公开 `AppMode`、`ApprovalMode`、Automation、Task、route 和 worker ledger API。当前兼容 facade 还包含 18 个 `pub mod`，父仓全部 Rust 源码中有 61 个文件、340 处直接引用；这些模块作为不稳定下游兼容桥刻意不进入生成 API 文档。这是已登记的收窄债务，不在本次热修中做破坏性改名。
- `resolve_runtime_route_with_limits` 保留 wire model、context/output 上限与 embedding alias。
- `EngineConfig.session_id` 在 Engine spawn 前绑定，所有子智能体/工作流事件带 owner session 并由宿主按 owner 过滤。
- `EngineHandle::steer` 返回 opaque id；`withdraw_steer` 区分已撤回与非 pending；中断、停止、压缩、换会话和 Engine drop 都使每个 id 恰好进入 committed/dropped 终态。
- `CancelSubAgents` 对当前 session 的子智能体做幂等批量取消。
- 共享取消槽带轮次身份（`TurnCancelSlot`）：`handle_send_message` 与用户 `!` shell 轮先铸 turn id 再原子安装令牌；宿主 `cancel_turn(turn_id)` 在槽锁内做身份校验、steer 处置发布与令牌解析，失配零副作用，陈旧代际的取消不再误杀引擎自启（空闲子代理完成/后台 shell 唤醒/goal 续轮）的新轮。`cancel_with_mode` 保留发射当前令牌语义给单用户前端；`publish_stop_disposition` 供宿主收口期履行 stop=clear 合同而不开火任何令牌（#38，修 pinvou-agent#254 底座半边）。
- Z.ai 第一方 Chat 路由谓词覆盖 `api.z.ai` 两产品与 BigModel 通用端点 `open.bigmodel.cn/api/paas/v4`（tui 侧谓词委托 config crate 单一实现）；GLM-5.3/5.3-Flash 为 forced-thinking：`thinking.type: "disabled"` 改写为 `enabled` + `reasoning_effort: "low"`（厂商迁移建议），effort 别名映射到 low/high/max，未知值省略由 API 默认接管；work_graph 收据约束同路由不得声称 API 未接受的 effective off（#52/#53）。
- 模型侧仍以 v0.9.12 的封闭角色集为执行姿态；仅允许宿主在当前 route Config 中显式注入的 prompt-only profile 贡献身份、描述和人设。Personal/Workspace/Plugin profile 及 model/provider/reasoning/permission/delegation pin 均 fail closed。

### 关键测试

- `forkguard_embedding_route_limits_preserve_wire_alias`
- `forkguard_steer_lifecycle_withdrawal_is_bounded_and_prevents_commit`
- `forkguard_steer_lifecycle_late_withdraw_reconciles_committed_state`
- `forkguard_steer_channel_commits_live_and_drops_withdrawn_input_in_turn_loop`
- `forkguard_cancel_all_running_is_session_scoped_and_idempotent`
- `forkguard_host_profile_overlay_is_config_only_and_prompt_only`
- `forkguard_cancel_turn_binding_spares_unnamed_turns_and_hits_the_observed_turn`
- `forkguard_idle_subagent_completion_self_start_ignores_a_stale_previous_turn_cancel`
- `engine_handle_stop_disposition_publishes_without_firing_any_token`
- `zai_forced_thinking_models_never_send_thinking_disabled`
- `restored_zai_forced_thinking_models_cannot_claim_effective_off`
- `zai_bigmodel_adjacent_routes_stay_fail_closed`
- `zai_chat_route_matching_is_exact`

## 5. T2 — 工具兼容与命令执行安全

### 保留内容

- `ExtraTools` 允许 app 在 Agent/Plan 原生注册宿主工具，不复制底座工具循环。
- MCP secret 只经宿主 resolver 注入；不写进程环境或普通配置文件。
- `SetDisallowedTools` 是逐会话/逐轮的权限塑形：更新后继续在该会话的 catalog 和最终调用边界拒绝匹配工具，但不热断开共享 `McpPool` 中已经建立的 server 连接。全局断连会干扰仍获授权的其他会话；底层连接由正常 pool/session 生命周期回收，安全边界由 catalog + 最终 dispatch 的 fail-closed 双检提供。
- `TurnToolSecurityPolicy` 把精确工具白名单、只读动作与 trusted external paths 下沉到每轮执行。
- 受限轮禁止动态工具、MCP、子智能体和未授权控制操作；新的显式用户消息才可安装替代权限。
- 工具在最终 backend dispatch 前再次做 exact/read-only 校验；Full Access 也不能绕过 non-bypassable approval。
- `File` 的旧/低层写入入口保留 64 KiB 硬上限；受限日志和审计不记录参数、结果或错误私密内容。
- execpolicy deny 表达力 phase-2（#37）：cmd.exe 单字母斜杠旗标（`/f` 形）可跳过而多字符 `/tmp` 类 POSIX 路径保持位置匹配；规则中段 `*` 通配零或多个命令令牌（DFS 有界）；deny 命令词单向折叠一个 `.exe` 后缀；rooted（`/`、`~/`、Windows 盘符）File 路径规则经分隔符/大小写折叠后精确匹配工作区外调用；规则集经 `Arc<RwLock>` 跨克隆活共享（`set_ruleset` 对全部派生执行器生效），`approved_for_session` 刻意克隆私有；subagent 工具调用在执行信封之后过与主线相同的 execpolicy 门（`Block` 镜像主线拒绝文案，`Prompt` 按继承的 auto-approve 姿态裁决），堵住「主线被拒转交子代理」的绕过口。均为嵌入方通用能力，候选上游化。
- 工具结果可经 `ToolResult.metadata["images"]`（文件路径数组）把图片产物交给视觉模型（#48）：引擎回合循环按与用户附件相同的 `<image path>` 三元组追加到工具结果消息，单结果上限 2 张、5 MB 共享上限、坏图/超数降级为纯文本并 `warn`，错误路径结果不附图；wire builder、会话持久化、压缩与非视觉路由剥离因走既有 `ImageUrl` 块路径而零改动。computer-use 截图回传的基础。
- 模型面文档与工具门控对齐（#31/#41/#50）：finance 工具两端点 host 前置过 `NetworkPolicyDecider`（`Deny`/未决 `Prompt` fail-closed，与 web_search/speech 同构错误形状）；verifier 背景 metadata 不再教退役工具名 `exec_shell_wait`；notify 工具对 `[notifications].method = "off"` 的合同有回归钉（配置 off 静默但工具结果仍报成功、settings 安装时机、方法变体 round-trip）；Bash 工具描述与 `command` 字段指引由与执行同一 shell dispatcher 派生（PowerShell/Bash/sh/zsh/cmd/fish 各有具体语法指引，Windows PowerShell 不再收到 login-shell 措辞）。

### 关键测试

- `forkguard_exact_dispatch_rejects_forged_backends`
- `forkguard_read_only_turn_rejects_write_at_final_dispatch`
- `forkguard_restricted_tool_audit_redacts_private_payload`
- `forkguard_restricted_planning_log_redacts_private_input`
- `forkguard_queued_control_op_keeps_restricted_turn_authority`
- `forkguard_queued_goal_edit_and_mcp_keep_restricted_authority`
- `forkguard_restricted_turn_defers_idle_subagent_completion_until_new_message`
- `forkguard_restricted_turn_defers_idle_shell_wake_until_new_message`
- `forkguard_denied_mcp_is_absent_from_catalog_and_blocked_at_execution`
- `forkguard_denied_mcp_tool_error_matches_the_unknown_tool_error`
- `forkguard_api_provider_chain_tail_is_bing`
- `all_unavailable_returns_actionable_error_without_private_details`
- `forkguard_mcp_secret_resolver_supplies_values_without_process_env_writes`
- `forkguard_write_primitive_enforces_the_64kib_boundary`
- `forkguard_write_file_enforces_the_64kib_boundary`
- `forkguard_benchmark_controls_are_explicit_and_default_off`
- `forkguard_benchmark_repairs_only_unambiguous_read_actions`
- `forkguard_benchmark_repairs_read_schema_and_attachments`
- `forkguard_benchmark_budget_truncates_batch_and_clears_followup_tool_surface`
- `forkguard_benchmark_final_only_rejects_repeated_tool_only_responses`
- `forkguard_benchmark_turn_repairs_file_aliases_before_execution`
- `forkguard_subagent_execpolicy_deny_matches_main_line`
- `forkguard_tool_result_images_reach_model_as_image_blocks`
- `forkguard_tool_result_without_images_key_is_unchanged`
- `forkguard_tool_result_bad_images_degrade_to_text_only_result`
- `forkguard_tool_result_images_are_capped_per_result`
- `finance_fails_closed_when_network_policy_denies_endpoint_host`
- `finance_fails_closed_on_prompt_when_default_is_prompt`
- `settings_installs_configured_method_from_config`
- `forkguard_shell_catalog_guidance_matches_execution`

## 6. T3 — 嵌入上下文与 Skills 来源

### 保留内容

- app 安装静态 prompt composer 后，底座不再追加 ambient AGENTS/project context、repo law、用户 constitution、continual harness 或重复 core profile。
- 显式 `skills_dir` 是唯一文件系统 Skills 根；插件 Skills 只能通过显式 registry 合并。
- Permissions/World State 指令片段有独立 100 KiB 窄上限，其他片段仍用 40 KiB 默认上限。
- working-set 路径分析只剥离前导内部 `<system-reminder>`，不改变发送给模型的正文。

### 关键测试

- `forkguard_runtime_loader_ignores_ambient_project_authority`
- `forkguard_explicit_skills_dir_excludes_ambient_workspace_sources`
- `forkguard_instruction_fragment_preserves_explicit_host_budget`
- `forkguard_working_set_ignores_leading_system_reminder_paths`

## 7. T4 — Automation 与运行生命周期

### 保留内容

- 每个 Automation 的 id 作为稳定 `conversation_key`；每次 run 仍是独立 Task。
- Task writer 使用上游 v3；reader 仅额外接受历史 Pinvou v4，v5 及以后 fail closed。
- `ThreadCreated` 在 turn 链接前持久化真实线程；`ExecutionTask` 只公开宿主所需 getters。
- 离线超过 60 秒的 recurring slot 不补跑；存在 queued/running attempt 时不重叠，直接推进到首个未来 slot。调度去重刻意扫描保留期内的完整 run 历史，而非只看最近一页，避免同一 scheduled slot 或较早 active run 被较新的终态记录挤出窗口后重复执行；有限保留策略约束扫描成本。
- 终态 run 清理对应 terminal Task；删除 Automation 不复活记录，也不遗失已经 enqueue 的 run。

### 关键测试

- `forkguard_automation_enqueue_preserves_settings_and_conversation_owner`
- `forkguard_scheduler_skips_offline_backfill_and_overlapping_runs`
- `forkguard_once_schedule_missed_while_offline_enqueues_exactly_one_run`
- `forkguard_accepts_legacy_v4_but_rejects_newer_task_schema`
- `forkguard_terminal_task_delete_refuses_active_and_is_idempotent`
- `forkguard_terminal_automation_run_delete_refuses_active_and_is_idempotent`

## 8. T5 — 会话全保真导出归档（追加减量主题）

### 保留内容

- `session_export` 模块 + `codewhale sessions export <id> [--output] [--compression 0-9] [--skip-artifacts] [--force]` CLI 子命令（#51）：把 `SavedSession` 全量（standalone system prompt、含 thinking/tool_use/tool_result 的全部轮次、branch journal、hydration 后的 approval receipts）流式打包为 `.tar.xz`（liblzma 静态压缩，vendored 镜像 mimalloc 先例）。
- 归档布局（格式版本 1）：`session.json`（`/load` 全保真恢复）、`container.json`（版本容忍 `/resume` 导入对话层）、`artifacts/**`（仅常规文件；符号链接跳过防跨界读取；成员按记录尺寸有界读，导出中途缩水则整次失败而非零填充错位）、`manifest.json` 最后写入。
- 输出经同父目录临时文件 + rename 原子落盘；CLI `--force` 才覆盖已存在目标。内容刻意不脱敏（属主完整日志，与分享向且脱敏的 `/export` Markdown 互补），模块文档与登记写明该区分。
- 上游化意图：归档格式、CLI 子命令与测试为通用基础能力，作为整体推上游后消除 drift。

### 关键测试

- `forkguard_session_archive_export_roundtrips_full_context`
- `forkguard_session_archive_includes_artifacts_and_respects_skip`
- `forkguard_session_archive_rejects_artifact_shorter_than_recorded_size`
- `session_archive_replaces_existing_output_atomically`
- `session_archive_rejects_out_of_range_compression_level`

## 9. T6 — 蜂群限流自适应调度（追加减量主题）

### 保留内容

- `DynamicGate` 取代固定容量 `Semaphore` 作为 launch gate（#43）：容量可运行时缩放（允许低于活跃持有数，超额持有者自然跑完），许可经 oneshot 预计数交接，取消安全的两条路径（未授予陈旧队列条目由授予方跳过、已派发许可由 Drop 再释放）杜绝丢唤醒。
- `RateLimitGovernor` 共享于 engine 并经 `SubAgentRuntime` 派生树继承（manager 单一 spawn 咽喉点盖章）：60s 滑窗内 ≥2 次限流事件或 >30% 比例（带 attempts≥2 体积守卫）容量减半，≥4 次暂停新发射（容量 0，在飞不受影响）；每 3 次连续成功 +1（AIMD）；暂停只能由窗口排空解除，外部限流变更不可静默解除；队列侧 5s 周期 `recover_if_window_drained` 时间驱动探活防「在飞全跑完、无成功信号、冻结到墙钟」。
- 429 重试尊重 `Retry-After`，无则 250ms 起 120s 封顶的全抖动指数退避防同源惊群；`QuotaExhausted` 不当瞬时限流，保留既有致命/checkpoint 路径；治理器只观察不延迟在飞调用。配置的 launch_concurrency 经治理器应用（`update_runtime_limits` 立即生效于活 gate）。
- 配套消费方为父仓蜂群模式（PR #444）；治理器本身为嵌入方通用能力。

### 关键测试

- `forkguard_rate_limit_governor_pauses_and_time_recovers_after_window_drains`
- gate cancel-before-dispatch 与多任务 abort/pause 压力测试（限时排空回满容量，丢唤醒/许可泄漏/陈旧条目吞槽均红）

## 10. 父仓适配边界

- `pinvou3-app` 负责产品工具白名单、AppMode 到 approval/trust 的映射、reasoning effort、会话 owner 过滤和定时会话创建。
- bridge 保留 v0.9.12 的有限轮次/工具预算、read denylist、bubblewrap、MCP OAuth、goal loop 与 telemetry 安全默认值。
- `session_id` 必须在 `Engine::spawn` 前进入 `EngineConfig`；不得事后依赖事件猜归属。
- 旧的全局 disabled-skills 调用已删除；包开关通过显式 bundle/registry 和每会话 disallowed tools 生效。
- Shell 任务对账优先使用快照与完成事件携带的稳定 `origin_tool_call_id`（上游 v0.9.12 行为，Hmbown/CodeWhale #5869）：host monitor 与 Tauri/Web 桥优先回写来源工具卡，仅对无来源旧任务按命令文本回退；来源卡被压缩或重载清除的已识别终态根任务不追加到当前时间线尾部，运行中任务保持合成状态卡可见（`shell_task_projection.test.mjs`、`forkguard_shell_monitor_assigns_identical_commands_by_stable_origin`）。
- 来源范围语义：shell 任务的 `origin_tool_call_id` 是产生它的唯一 root 轮内工具调用；子智能体任务只携带 `owner_agent_id`、来源为空，走无来源对账路径。只读 `multi_tool_use.parallel` 子调用虽共享包装调用的来源，但 shell 工具带 `ExecutesCode`、不能进入只读并行，该共享对 shell 任务不会发生。消费方必须保留 owner 区分，且不得让一个任务抢占已绑定另一任务的卡片。

## 11. 软上限评估与后续减量

当前净增 12759 行，超过 1500 行软上限；`engine.rs`、`turn_loop.rs` 和 `engine/tests.rs` 也超过单文件 200 行提示线。原因是状态机、最终分发安全、“完整专家人设仅进入被选子智能体”的 spawn 边界、bundle Skills 排除 ambient 文件源的权限边界、Windows PowerShell 执行策略兼容及其行为回归，以及评审要求的结果式生命周期/评测回归必须和 v0.9.12 原生 Engine 同步，拆成 app 侧镜像会形成更危险的双状态源。当前 Rust/Clippy/rustdoc 兼容只包含等价重写、`Default` 补全、窄 lint 说明、文档可达性修复和无调用测试 helper 清理，不改变公开函数签名或新增运行语义。

后续减量顺序：

1. 将可靠 steer lifecycle 与逐轮 exact/read-only dispatch 作为通用 host API 上游化。
2. 将 Automation misfire/no-overlap/conversation ownership 上游化。
3. 将 execpolicy phase-2 表达力（斜杠旗标/通配符/`.exe` 折叠/绝对路径）与 subagent 接线作为通用 deny 语义上游化。
4. 将 T5 会话归档导出（格式 + CLI + 测试）作为整体上游化。
5. 将 T6 限流治理器作为通用 fan-out 调度上游化。
6. 将 GLM-5.3 forced-thinking 方言与 BigModel 路由谓词随 provider 方言族上游化。
7. 将 `FragmentId::Permissions` 的显式 100 KiB 宿主预算做成通用的可配置 fragment limit，上游化后删除固定例外。
8. 父仓先迁移到明确 re-export API，再分批收窄当前 18 个 `pub mod` 兼容 facade。
9. 上游提供完整 static-composer/explicit-skills-root 契约后删除 T3 patch。
10. 每次上游 release 重新核对已吸收项，不保留兼容壳。

## 12. 发布与回退

- 公开回退点是不可变 tag `pinvou-v0.9.5-r13`；本地 `backup/pre-v0.9.12-sync` 不是发布前提。
- 不可变 `pinvou-v0.9.12-r1` 停在 r1 收口 `1fafee7e26b60a59457a43bce50c63aa2ad9dbaf`；r2 已收口（2026-09-21，main 经 #574 `1de2a60f0` 完成）：不可变 tag `pinvou-v0.9.12-r2` 切在 `6f780290f`，main 上维护分支/gitlink/tag 三方相等；本 PR 仍处于过渡期叠层：父仓 gitlink 指向 workspace_roots 主题 head `bbc90540a`（已 rebase 到 r2 收口 `6f780290f` 之上），以 `scripts/verify-public-submodule.sh` 验证公开可达性（过渡期断言 gitlink=分支头，本 PR 叠层期间该门禁保持红并在 PR 描述披露；待 #54 squash 合入、`pinvou3-clean` 推进后重钉 gitlink，恢复三方相等；依据 `docs/fork-policy.md` 第 0 节过渡期豁免）。
- 发布过程中只为精确 head 的受保护分支更新临时移除无法在该维护分支触发的 required status contexts，完成快进后立即恢复原保护配置；未关闭 force-push 防护，也未重写已发布 tag。
- 后续发布仍不得降低公开校验或把本地 object 当成发布成功。
