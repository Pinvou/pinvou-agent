# 三态安全模型与会话级守卫询问开关（tri-mode safety）

> 本文是应用侧安全姿态从 Plan / Yolo 两态细化为 Plan / Auto / Yolo 三态、
> 并为 Yolo 引入会话级 ask-on-guardrail 开关的设计参考（本 PR 的 Stage 4）。
> 底座行为登记为 [Pinvou/CodeWhale#131](https://github.com/Pinvou/CodeWhale/pull/131)：
> (a) `ask_on_guardrail` 宿主武装的交互式守卫提示；(b) `<turn_meta>` 权威转换通告。
> fork 登记与指纹的单一现状清单见 [`fork-modifications.md`](fork-modifications.md)。
>
> 与 Claude Code 6-mode、Codex 沙箱/审批分离、Gemini CLI plan<default<yolo
> 层级、opencode deny-survives-auto 的对齐研究见父仓 PR 正文，本文只记录落进
> 实现的结论。

## 1. 背景与目标

旧两态下工作车道默认 Yolo（`AppMode::Agent` + 自动批准）：模型可全程免卡执行，
底座守卫面（非旁路 holds、repo-law、typed ask 规则、sandbox 升级、execpolicy
拒绝面）在自主姿态下一律 fail-closed 硬拒。缺口有二：没有中间档（主流 harness
的默认档，如 Claude Code `acceptEdits`/`default`，是「普通操作逐项请批、工作
区内写免卡」）；Yolo 下守卫命中只有硬拒一条路，模型无法就一次受保护的敏感操作
向用户要一个当面决定。

三态改造把**审批姿态从 mode 中拆出**（`SerializableMode::approval_params` 映射，
`pinvou3-app/src-tauri/src/core/mode_state.rs`）：Plan 与 Auto 共用
`(false, Suggest)`，Yolo 为 `(true, Bypass)`，逐轮随 `Op::SendMessage` 下发；
同时在底座引入会话级 `ask_on_guardrail` 武装（经 `Op::ChangeMode`），武装后
上述 fail-closed 守卫面改为升起审批请求——前提是宿主已保证有真人可以应答。

## 2. 模式语义

| 模式 | 引擎姿态（逐轮 op 与同步 op 同一映射） | 守卫命中（开关 ON，默认） | 守卫命中（开关 OFF） | 普通可提示工具 |
|---|---|---|---|---|
| Plan | `AppMode::Plan`（底座只读归一：禁 shell、只读 sandbox、只读工具集） | 只读拦截（既有） | 同左 | 拦截；审批事件到达即异常，防御性硬拒（`plan_mode`） |
| Auto | `Agent` + `(false, Suggest)` | 弹卡 | 硬拒 + `chat:guardrail_denied` | 逐次审批卡；工作区内文件写走底座 #5185 carve-out 免卡，只读命令走 readonly 分类免卡 |
| Yolo | `Agent` + `(true, Bypass)` | 弹卡 | 硬拒 + 通告（`guardrail_ask_off`） | 免卡（历史 Yolo 行为） |

- Auto 是新工作车道（未绑定 plain 会话）的默认；code 会话与绑定工作区的 plain
  会话默认跟随 `code_permission.last_mode`（从未用过 → Plan）。安全姿态跟随
  工作区绑定，不跟随会话名。
- 模式切换（桌面双车道）：任一档位进入 Yolo 都过同一条一次性确认门
  （`chatYoloGateAllows` / 原生车道 `needsYoloConfirmation`）——Plan 退回走
  `exit_plan_to_yolo`，Auto 直达走 `set_session_mode(yolo)`，不存在静默落空的
  切换路径；进入 Auto/Plan 不过门。草稿态（尚无会话）守卫开关禁用。
- Plan 流程交接（出方案 → accept → 回到进入 Plan 前的 mode）复用底座原生闭环，
  app 不自建 phase 状态机；见第 6 节 `pre_plan_mode`。

## 3. 会话级 ask_on_guardrail 开关

### 3.1 语义与持久化

- **默认开**。新会话的开关从全局偏好 `code_permission.ask_on_guardrail_default`
  （settings.json，默认 true）播种；`_ask_on_guardrail.json` 侧车里的显式覆盖
  优先。尚无 mode 条目的会话翻开关：选择落侧车 + 内存覆盖镜像（不借此钉死
  mode），后续物化（模式写入铸条目）与重启经播种点（`ask_on_guardrail_seed`）
  继承该选择——显式覆盖永不静默丢弃。侧车是 `HashMap<String, bool>` 显式覆盖
  表，与 mode 共用同一 io 互斥、原子写（`features/sessions/mode_state.rs`）；
  落盘失败只记日志不打断交互——内存切换已生效；中断写入留不下截断文件，
  损坏文件在装载时静默跳过。会话删除/驱逐时侧车条目随保留策略一并清除。
- 写路径统一过存在性门（id 形状 + 会话存在，`ensure_mode_target_session`），
  防 IPC 直调给任意 id 铸条目/写侧车。

### 3.2 传播与武装

开关与 mode 一起经 `publish_session_mode_change` 发布：会话引擎在位时发
`Op::ChangeMode`（`ask_on_guardrail` 显式携带 `Some(flag)`——该 op 整体替换
姿态；`None` = 不改变，是 wire 层的 serde 缺省缺席语义）+ 规则集热换；引擎不
在位则跳过，lazy spawn 的下一次发送按 store 状态组 op 并在 spawn 期武装
（`engine_pool.rs` 的 `sync_session_safety_posture` /
`arm_spawned_session_safety_posture`，共用同一 `safety_change_mode_op` 纯
映射）。底座侧经 `LiveRuntimeAuthority` 与其他姿态字段同 doctrine 携带；turn
安装重录会话现值，**永不把已发布的武装回滚**（`record_applied_runtime_authority`）。

### 3.3 武装后改变的五个底座守卫面

宿主武装 = 保证有真人可以应答，于是原本在自主姿态下 fail-closed 的面改为审批
（底座 `crates/tui/src/core/engine/turn_loop.rs`，机制登记见底座 PR (a)）：

1. **非旁路 holds**（`rlm`/`rlm_eval`/`start_mcp_server`）：Full Access 下从
   硬拒改为带 `approval_force_prompt` 的审批。必须显式置位——普通 resolver 在
   auto-approve 下会自动放行非旁路工具，落空即静默执行（#3866 反转语义）。
2. **repo-law ask holds**：弹审批而非硬拒。
3. **typed ask-rule Prompt**：被尊重，不再被 auto_approve 静默吞掉（#3790 的
   「mode 是唯一权威」在武装处开例外）。
4. **sandbox 升级**（`sandbox_permissions`）：走 Suggest 路径审批，携带
   justification。
5. **execpolicy 拒绝面**（denied prefixes + typed denies，匹配语义原样）：
   升级为携带匹配证据的审批。

   以上武装豁免只在**可弹卡姿态**（Full Access / auto-approve）生效：Auto-Review
   与 Never 在全部面上无条件保持拒绝——无人能应答的卡不会升起（评审修复
   `04c1f7843` 在 deny face / repo-law / escalation 三处显式排除这两个姿态）；
   武装 escalation 卡携带模型 justification。

### 3.4 不变式（什么不变）

- **未武装逐字节等同**：`ask_on_guardrail=false`（默认）时全部守卫面与旗标前
  doctrine 逐字节一致，既有 fail-closed pin 原样绿。
- **Never 与 Auto-Review 姿态无论武装与否一律拒绝**：deny face、repo-law、
  escalation 的武装豁免都显式排除这两个姿态（评审修复补齐），拒绝只在收据措辞
  上可能有别，从不放行。
- **确定性 allow/deny 永不弹卡**：定时任务无人值守的初始执行在转发器先行短路，
  按任务保存的批准策略确定性 auto-approve/deny（`forwarder.rs` 的
  `scheduled_tool_should_auto_approve` 分支，优先级原样保留）；它不走交互审批
  面，也正因此无人值守引擎的规则档恒为 Full（第 5 节）。无人值守**以外**的
  定时/多智能体会话与普通对话同序。
- **composer `!` 不武装**：用户手敲的命令输入即批准（#5191），typed deny 仍
  硬拒，execpolicy/sandbox 仍是硬边界；武装只作用于模型来源调用。
- **子代理镜像主线**：继承 auto-approve 的子代理对 delegated typed-ask 也
  拒绝——子代理绝不静默执行武装主线要问的操作；held-call 的守卫 hold 路由臂
  为 fail-safe 前瞻（Bypass 下 plan decision 把 built-in gate hold 收敛为
  Block，今天实际可达的武装子代理面是 typed-ask 拒绝；评审修复如实标注）
  (`crates/tui/src/tools/subagent/mod.rs`)。

## 4. 审批面（interactive approval surface）

### 4.1 事件流与处置定序

底座对可提示工具发 `Event::ApprovalRequired` 后，转发器不再无条件自动批准，
而是经纯函数 `approval_surface_decision`
（`pinvou3-app/src-tauri/src/features/assistant/pending_approvals.rs`）定序，
以 TUI `resolve_approval_request_disposition` 为基础、Plan 防御臂有意更严：

```text
会话拒绝缓存命中（exact approval_key）        → Deny(session_denied)
Plan                                          → Deny(plan_mode)
会话授予缓存命中（grouping_key）且非 force_prompt → ApproveSilently（不发事件）
force_prompt（非旁路 hold）                    → ShowCard
Yolo 且开关关                                  → Deny(guardrail_ask_off)
其余（Yolo 开关开 / Auto）                     → ShowCard
```

- 缓存读路径带会话维度：授予/拒绝是**会话级**记忆，A 会话的授予/拒绝不得
  跨会话放行/连坐 B 会话（有跨会话负测试钉）。
- Plan 防御臂先于授予缓存（与 TUI legacy 顺序的有意分歧）：TUI 的 Plan 流
  从不产生审批事件；宿主侧 turn 中途可能切到 Plan，旧授予若仍静默放行，
  「Plan=只读」就被缓存击穿。硬拒方向 fail-safe 且零成本。
- 硬拒调 `deny_tool_call` 并广播 `chat:guardrail_denied`，`reason` 是稳定英文
  键（`session_denied` / `plan_mode` / `guardrail_ask_off`），前端 i18n 按键
  映射三语文案；payload 不带 input 原文，避免敏感参数进事件流。
- 弹卡先登记挂起再广播 `chat:approval_required`（字段与底座事件一一对应：
  `approval_id`、`tool_name`、`description`、`input`、`intent_summary`、
  `approval_key`、`approval_grouping_key`、`force_prompt`）。force_prompt 的
  hold 永不被缓存放行——只有人当面点过的这张卡能过。

### 4.2 卡片决策与作用域

ApprovalCard 三个决策走 `decide_approval` 命令（`app/commands/interaction.rs`）：

| 决策 | 缓存作用域 | 语义 |
|---|---|---|
| 允许（allow） | 无 | 仅此一次 |
| 本会话允许（allow_for_session） | lossy `approval_grouping_key` | 覆盖后续旗标变体（v0.8.37 语义） |
| 拒绝（deny） | exact `approval_key` | 换一个参数要重新请批，不连坐（#1617 语义） |

任何决策都先校验 decision 值（摘除登记不可逆，非法值原样 Err 不留痕迹），再
摘除挂起登记（engine 侧审批 id 随调用消亡，卡不得残留），engine 调用**成功后**
才写缓存——失败的点击不得替用户放行/拒绝后续同族调用。授权/拒绝缓存镜像
TUI 的作用域语义且**按会话隔离**（跨会话不得互相放行/连坐），两类缓存互不串
作用域；`clear_session` 只摘除本会话份额，不误撤销其他会话的授予。

### 4.3 登记簿生命周期

- 全部驻内存：进程重启后 engine lazy spawn，缓存随进程消亡，无需持久化。
  turn 终态 `clear_pending` 兜底清空该会话的**挂起卡**（取消/中断时可能没有
  逐条决策）；授予/拒绝是**会话级记忆，跨 turn 存活**（「本会话允许」的语义，
  与 TUI 一致：TUI 只在换会话/换工作区时清），随会话删除在会话清理处理器经
  `clear_session` 摘除。前端 turn 终态同步把未决策的审批卡就地落为终态
  （cancelled），不再可点。
- 前端 remount / 切回会话时经 `get_pending_approvals` 重放审批卡（与
  `get_pending_user_inputs` 同一还原思路）；事件侧按 `approval_id` 幂等去重，
  后端同 id 重放原位替换、不重复出卡。
- 多端收敛：模式/开关切换统一广播 `chat:mode_changed`（桌面 emit + 远程转发，
  payload 带 snake_case `mode` 与会话解析后的 `ask_on_guardrail`），前端经
  epoch 保护的统一收敛点写回，在途旧读作废。审批/守卫/模式三类事件都进
  remote_control 的事件转发白名单（`RUST_FORWARDED_EVENTS`）；面向 Web 的
  RPC 命令仍受既有多智能体执行 denylist 管辖，本 PR 不改其边界。

## 5. 规则档（SafetyRuleProfile）

execpolicy 硬拒规则集按会话 mode 组装（`features/assistant/safety_deny_rules.rs`）：

| 档 | 服务车道 | 规模 | 组成 |
|---|---|---|---|
| Full | Plan / Auto（及一切无会话信息的调用方默认） | 8,729 条 | never-sensible 底线族 + 敏感族（逐文件凭据销毁、exfil 直传、持久化、sudo 等），与 pre-profile 组成逐字节一致（快照测试钉） |
| Floor | Yolo | 3,521 条 | 只保留 never-sensible 底线族，敏感族缺位 |

- **profile 只选择组装哪些规则族，绝不改写规则串、绝不 Deny→Ask 改标**：每条
  发出的规则保持 typed `Deny`，命中的族在一切审批姿态下照旧短路。武装会话对
  拒绝面的「弹卡还是静默拒」由底座 deny face 的武装映射决定（3.3 第 5 条），
  与 profile 正交。
- **敏感/底线分族按语义不按拼写**（v3.5）：解析 `C:\Users\me\...` 拼写的逐凭据
  **直传**面归敏感族（Full-only），与 `~`/`%userprofile%` 拼写的同族一致——
  此前它随解析宿主家族落在 Floor，使 Yolo 姿态取决于模型恰好写出哪种拼写。
  销毁面与裸根（整目录/整 profile）直传面在一切拼写上保持 Floor（与字面前缀
  族的既定取舍一致，测试钉）。
- Floor 缺敏感族是产品意图：Yolo 用敏感 deny 面换武装卡，只留「永远不合理」
  的底线（mkfs 全盘、sudoers 写等）。Yolo 会话因此确实失去了敏感族的机械门，
  这是有意的取舍而非疏漏。
- **引擎车道**（`SafetyRuleProfile::for_engine_lane`）：无人值守 scheduled
  引擎**恒为 Full**——存储的 Yolo 是交互姿态，无人看卡恰恰是最不能拆机械门的
  场景；交互引擎（含定时会话的交互续段）按 mode 取档。
- 热换纪律：mode 改变触发 `Op::ChangeMode` + `SetPermissionRuleset` 同一发布
  序列；同档重发重建同一规则集，调用方无需追踪旧档。spawn 期武装在引擎条目
  发布前排队，第一轮即按会话车道运行；超级权限/连接器/技能开关的热刷新也按
  各自会话的档组装。

## 6. 模式状态与持久化

- 会话 mode 存内存 map + `_session_mode_states.json` sidecar（所有会话逐会话
  持久；legacy `_code_mode_states.json` 在新文件缺席时迁移并立即落盘）。每次
  会话内切换只写该会话记录；全局车道默认只由草稿态显式切换写（two-lane
  semantics：`set_mode_default` 写 settings.json 的 `mode_defaults.work` /
  `code_permission.last_mode`，会话内切换不回流全局）。
- **`pre_plan_mode`（批准方案 ≠ 批准全自动）**：进入 Plan 时记录来时 mode，
  accept_plan 后回到它（fallback Auto）并经同一发布序列热同步；仅驻内存——
  重启后的 Plan 会话经 `resolved_default_mode` 重新解析，不把进程相对值钉进
  sidecar。headless 路径（`set_mode_and_persist`）要求落盘成功，Plan → Yolo
  的重开漂移按失败 run 处理。首次模式写入即 Plan 的未物化会话，铸条目按
  `resolved_default_mode` 解析（而非历史 Yolo 哨兵），accept 回到 chip 显示
  过的默认（Auto）——有行为测试钉。
- **`SerializableMode::Auto` 向后兼容**：旧侧车里的 `"plan"/"yolo"` 照常反序
  列化；`"auto"` 只由新二进制写入，旧二进制读到会整表回退 boot 缓存
  （`durable_mode_entry` 既定语义：一次性陈旧、无数据丢失）。
- 无持久记录时的默认解析矩阵：

| 会话形态 | 默认 |
|---|---|
| code 会话 / 绑定工作区的 plain 会话 | `code_permission.last_mode`（None → Plan） |
| 未绑定 plain 会话（工作车道） | Auto；显式选择的 work 车道全局默认（plan/yolo/auto）由前端在会话物化时套用为该会话自己的记录 |
| 无人值守定时引擎 | 交互审批面不适用；规则档恒 Full（第 5 节） |

## 7. 模型知情（authority transition）

引擎逐轮记录 `<turn_meta>` 块上报过的姿态（`TurnMetaPosture`：mode、approval、
武装）。宿主在轮间改变 mode/姿态/武装时，下一块渲染且仅渲染一行：

```text
Authority transition (host): agent (Full Access, guardrail prompts armed) -> agent (Ask, guardrail prompts disarmed); reason: host_authority_change
```

轮中任意次 `ChangeMode` 翻转折叠为下一块的一次转换；姿态未变的轮零新增行
（#3947 byte-stable doctrine）。`/preview-request` 读同一引擎计算值，预览与
实线逐字节一致。未改动前，姿态变更只能靠下一轮的常设姿态行被模型「观察到」，
行为突变（突然免批/突然要批）读起来没有解释；这一行让变更自述原因
（`crates/tui/src/core/authority.rs`，机制登记见底座 PR (b)）。

## 8. 测试与登记

- **底座**（CodeWhale#131）：本行为新增 14 条 `forkguard_` pin——提交
  `998ffe27e` 7 条 + 复审加固 6 条 + 评审修复 1 条，逐条钉住上述五个武装面、
  Auto-Review/Never 恒拒、授予缓存不可静默消费武装 hold、composer 不武装、
  子代理两个镜像面、transition 渲染/折叠/预览一致与武装不可回滚。
  forkguard 行为名总数 255 → 269；门禁命令
  `cargo test -p codewhale-tui --lib --locked forkguard_ -- --test-threads=1`
  （评审修复头 `04c1f7843` 实测 254 过 / 0 挂，既有 fail-closed pin 原样绿）。
  14 条逐名：
  `forkguard_armed_full_access_forced_hold_resolves_to_prompt`、
  `forkguard_armed_full_access_hold_prompts_instead_of_blocking`、
  `forkguard_armed_repo_law_hold_prompts_instead_of_blocking`、
  `forkguard_armed_full_access_typed_ask_rule_prompts_instead_of_auto_running`、
  `forkguard_armed_sandbox_escalation_prompts_instead_of_failing_closed`、
  `forkguard_armed_deny_face_becomes_guardrail_prompt_unarmed_stays_block`、
  `forkguard_armed_child_refuses_delegated_typed_ask_even_under_auto_approve`、
  `forkguard_turn_authority_install_never_rolls_back_published_arming`、
  `forkguard_authority_transition_between_renders_the_host_change`、
  `forkguard_turn_meta_authority_transition_renders_only_on_change`、
  `forkguard_live_turn_meta_builder_reads_the_pending_transition`、
  `forkguard_preview_manifest_hashes_the_same_authority_transition_line_the_wire_sends`、
  `forkguard_apply_change_mode_status_suffixes_report_the_arming_change`、
  `forkguard_turn_start_detector_records_the_posture_and_renders_the_transition_once`
  （评审修复新增，逐字节咬住 detector 的记录与一次性消费；deny-face 钉扩展
  Auto-Review 恒拒面）。
- **应用侧**：`forkguard_approval_surface_record_dedupes_by_approval_id` /
  `_remove_returns_entry_and_cleans_empty_session` /
  `_grant_and_denial_scoping` / `_clear_session_wipes_memory` /
  `_decision_truth_table`（处置真值表逐行钉），`forkguard_ruleset_profile_
  full_is_the_pinned_composition` / `_floor_keeps_floor_drops_sensitive` /
  `_sudo_toggle_interplay_unchanged` / `_floor_engine_behavior` /
  `_engine_lane_forces_full_for_unattended_runs` / `_floor_count_is_pinned`
  （Full 8,729 沿用既有 `rule_snapshot_is_stable` 快照，Floor 3,521 新钉；
  车道矩阵另有专测），加 sessions/commands 域的开关持久化与
  默认播种回归；前端 lane 覆盖三态芯片、开关与 ApprovalCard。
- **登记**：fork-guard 的 `EXPECTED_HEAD`/`EXPECTED_COMMITS` 与指纹表
  （`scripts/fork-guard.sh`）、`docs/fork-modifications.md` T2 增补随本 PR
  Stage-4 提交落地；底座候选期登记与 gitlink 重钉按 fork-policy 的过渡期口径。

## 9. 已知边界

- **Web 端收得到、还没渲染**：三类事件已进 Relay 转发白名单
  （`src/platform/web/access-policy.json`），但 Web bridge 尚无审批卡渲染与
  `get_pending_approvals` 回放接线——桌面双车道（原生 + 代码）才是本 PR 的
  产品面。Web 端对审批事件的表现是后续项，不是本 PR 验收范围。
- **审批面跨进程重启不保真**：挂起卡与授予/拒绝缓存是进程内存；重启后 turn
  重新请批（engine 侧审批 id 本就随进程消亡）。这是有意取舍，不是持久化缺陷。
- **多智能体引导期的默认解析先于 code 判定注册**：`_multi_agent.json` 的引导
  装载在 store 构造期用 `resolved_default_mode` 铸条目，而 code-session
  predicate 在 Tauri setup 后段才注册——从未切换过 mode 的多智能体 code 会话
  重启后可能显示 Auto（而非 Plan）。条目一旦物化不再随偏好/predicate 重播种，
  用户显式切换一次即矫正；修复需要引导顺序或解析迟绑定，属后续项。
