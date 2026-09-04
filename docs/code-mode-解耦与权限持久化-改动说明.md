# code 模式解耦与权限持久化 — 改动说明（合并留档）

> 本文件于 2026-08-12 由《code-plain-decoupling-改动说明.md》（第一篇）与
> 《code-mode-permission-改动说明.md》（第二篇）合并留档——两篇功能均已审议
> 合入主线，内容原样保留，仅标题层级统一下移。
> **注意**：mode 持久化的现行语义以第二篇「六、语义修订（2026-08-12，三分
> lane + 会话独立持久化）」为准；其前文（三/四/五节）描述的两层持久化语义
> 已被该节取代，仅作历史记录。

---

## 第一篇：code 与 plain 模式解耦及能力回补（PR #182）

> 关联：`.luzeyang/code-plain-decoupling/code-plain-decoupling-improvements.md`（设计方案，下称「设计文档」，已归档）、`.luzeyang/code-plain-decoupling/code-mode-review-improvements.md`（问题来源，已归档）、`.luzeyang/code-plain-decoupling/code-native-agent-会话能力档案设计.md`（策略对象方向，已归档）。
> 分支：`feat/code-plain-decoupling`（自 `fix/code-native-agent-review-issues` HEAD 拉出，未同步主线）。
> 本篇登记设计文档 D-1~D-3、R-1~R-3 六项的实施结果，作为 PR 的验收依据。X-1/X-2（Skills 底座改动）维持标记、不在本次改动内；S-1 安全护栏按决策挂起。

### 一、为什么需要这些改动

code 模式（真实项目目录绑定）与 plain 模式（沙箱会话目录）共享同一引擎、同一 `chat` 命令、同一发送 op，但两者的模式差异没有一等表达，而是以 if 分支堆在共享链路里，造成两类实际问题：

1. **耦合本身**：`code_session: bool` 把"产品模式"（plain/code）与"运行时"（native/ACP）两条正交轴压扁在一个布尔值上，互斥靠调用顺序约定，非法组合可构造且不报错；判断散布 8 个文件，新增代码很容易漏判、误判。
2. **耦合导致的能力缺失**：共享 `build_send_message_op` 不感知会话类型，Plan reminder 写死 work"方案卡片"语义而 code 页没有卡片管线（模型被引导把方案藏进用户看不见的通道）；code 前端不接 plan 事件，Plan 模式成了"假安全"（用户以为有审批保护，实际没有）；逐轮工具白名单、审批参数对 code 无从表达；`compact_now`、token 用量、memory 事件在 code 页未接线。

思路（经 VS Code agent 模式源码调研验证）：模式是**配方卡（类型 + 策略数据），不是新机器（管线）**。先用枚举 + 策略对象拆掉共享链路的 if，再逐项接回被 if 砍掉的能力。底座（CodeWhale）零改动。

### 二、改动总览

| 项 | 内容 | 性质 |
|---|---|---|
| D-1 | `SessionMode::{Plain, Code}` 枚举替代 `code_session: bool` | 行为不变重构 |
| D-2+D-3 | `SessionPolicy` 策略对象 + `session_mode`/`session_policy` 统一查询入口 | 行为不变重构 |
| R-1 | code 页 Plan 审批闭环（方案审批卡 + `accept_plan`/`discard_plan`） | 功能修复（消"假安全"） |
| R-2 | 审批参数收编策略；逐轮白名单链路核实贯通 + 测试锁定 | 行为不变重构 + 测试 |
| R-3 | code 页接线 compact 入口 / token 用量 chip / memory 徽标 | 前端接线 |

13 个文件修改 + 4 个文件新增（约 +900/-130）。work 主聊天与 ACP 会话行为零变化。

### 三、逐项改动说明

#### D-1：`SessionMode` 枚举替代 `code_session: bool`

- **什么**：新增 `core/session_mode.rs` 定义 `SessionMode::{Plain(默认), Code}`（`is_code()`/`is_plain()`）；`SessionAgentRecord.code_session: bool` → `mode: SessionMode`。运行时轴不新建枚举——既有 `AgentBackend`（Deepseek=原生、其余=ACP）即该轴，两轴从此各自类型化、不再压扁。
- **兼容性（关键设计）**：磁盘 `session-agents.json` 保持原 `code_session` 键与**布尔格式**——自研 serde 适配模块 `session_mode_serde`（store.rs）序列化写布尔、反序列化兼容旧布尔与未来 kebab-case 字符串，新旧版本应用互读文件均不误判，零迁移。
- **为什么**：布尔约定下"绑 ACP 手动清标志、绑 code 靠 bail 间接互斥"非对称，新增会话类型或新路径漏清除即出非法状态且静默；枚举使 `matches!(mode, Code)` 成为唯一判定，新增取值时编译器强制审查所有分支。
- 改动面：`store.rs`、`codex_acp/mod.rs` 共 24 处（其他 6 个文件全走 `is_code_session()`/`code_project_workspace()` 方法，签名不变零改动）。新增测试 `session_mode_deserializes_legacy_bool_and_serializes_as_bool`（旧 JSON true/false/缺字段/未来字符串四种情形）。

#### D-2+D-3：`SessionPolicy` 策略对象 + 统一查询入口

- **什么**：新增 `features/assistant/session_policy.rs`——`SessionPolicy::for_mode(mode)` 提供四个取值：`connector_scope()`（连接器禁用集 plain/code scope）、`extra_hidden_tools()`（code 追加 `present_artifact`/`load_skill`，过渡方案注释原意保留）、`plan_reminder()`、`approval_params()`。bridge 新增 `session_policy(session_id)`、store 新增 `session_mode(session_id)` 作为统一入口。
- **收编的 if**：`shape_disallowed_tools` 改为策略驱动（plain 早退路径与旧实现逐字节等价）；`build_send_message_op` 增加 `session_id` 首位参数，Plan reminder 与审批参数改经策略产出；bridge 内 L327/L535 两处裸判断改走策略。`reminder_for()` 函数与 `PLAN_REMINDER` 常量迁入策略模块。
- **配套**：`AppEngine` 补 `session_id` 字段（engine wrapper 本就 per-session）；`chat.rs` 附件 `reference_absolute` 经调查为 roots 比较通用逻辑，核实后不动；`sessions.rs`/`codex.rs` 的 `is_code_session` 调用已是统一 store 方法，不改。
- **为什么**：共享链路每加一个模式行为就堆一个 if，双向误伤风险随热度累积；策略对象把差异收敛为数据，S-1 安全分化（挂起项）与未来第三条会话类型的挂载点就此就位。
- 新增测试：策略三取值断言、plain/code 整形等价（含幂等）、两模式 Plan op 逐字节相等（本期行为不变断言）。

#### R-1：code 页 Plan 审批闭环（功能修复）

- **问题**：后端 Plan 能力对 code 完备（只读工具面、`plan_snapshot`/`plan_ready` 事件、`accept_plan`/`PendingPlanClaim` 通用），但 code 前端不消费 plan 事件——用户切 Plan 看到的只是文本方案，无批准/拒绝按钮，"以为有审批保护，实际没有"。
- **什么**：
  - `code-native-lane.js`：lane 新增 `planSnapshot` 状态与两个 plan 事件消费（增量快照、plan_id 幂等、新方案冻结旧卡为 superseded）；`chat:user_message` 的 `accept_plan` 回声置卡片已批准；hydrate 还原只读历史卡（镜像 work 冷启动取舍：待批方案跨 remount 降级不可批准）。
  - `CodexAcpView.jsx`：`NATIVE_CHAT_EVENTS` 注册两个 plan 事件；新增 `NativePlanCard` 组件（复用 `PlanLayer`，与 work 方案卡同视觉语言）；`acceptNativePlan` 调 `accept_plan`（planMarkdown 逐字镜像 work 的 `composePlanMarkdown`）、`discardNativePlan` 调 `discard_plan`（失败按 `plan_not_active` 分流，与 work 同构）；删除"本期不接"注释。仅 native 生效，ACP 分支零改动。
  - 按钮在非 Plan / busy 时前置禁用（比 work 的错误收口更诚实）。
- **reminder 诚实化核对**：审批卡落地后，同文 reminder（"方案卡片由系统在你调 update_plan 后自动展示"）对 code 变为真实描述，**保持同文不改**；卡片批准按钮文案与 reminder 的【就这么干】一致。
- i18n：`uiCodex` 新增 6 key（批准/放弃/覆盖/历史/两类失败提示），卡片按钮复用既有 `planReady/planGo/planDrop` 等顶层 key，zh/en/ja 三语齐全。
- 测试：`code_native_lane.test.mjs` 新增 Plan 审批与 hydrate 两组用例（快照增量、出卡、markdown 拼装、幂等、覆盖冻结、回声批卡、只读还原等约 +128 行）。

#### R-2：审批参数收编策略 + 白名单链路锁定

- **什么**：`SessionPolicy::approval_params()` 收编共享 op 写死的 `auto_approve: true` + `ApprovalMode::Auto`（本期两模式同值，行为不变），`build_send_message_op` 策略取数——S-1 安全分化的挂载点。前端 `restrictTools: false` 调用点补注释标注入口与承接关系。
- **链路核实结论**：逐轮白名单（`restrict_tools` → `allowed_tools: Some(空表)`）对 code 会话本就无阻断——缺的不是链路而是驱动源；故本期不改前端传参，只锁定行为并留策略挂载点（设计文档 R-2 方案已按此修订）。
- 新增测试：`approval_params_are_full_auto_for_both_modes_for_now`（行为不变断言）、`build_send_message_op_restrict_tools_also_applies_to_code_sessions`（code 会话 `restrict=true` 得空白名单、`false` 不限制）。

#### R-3：code 页接线 compact / 用量 / memory

- **什么**（仅 native，底栏 kb 选择器后两元素）：
  - **用量 chip 兼 compact 入口**：lane 已消费的 `chat:usage` 渲染"上下文 Nk"（`tokens.max` 恒 0 为已知限制，按只显已用降级，格式与 work `fmtCtxTok` 同款）；点击调 `compact_now`（参数与 work 侧封装同款），busy/compacting 置灰，压缩过程由既有 `chat:compaction` 系统项呈现。
  - **memory 徽标**：lane 新增 `chat:memory` 监听（事件本就对全部会话发射），底栏 Brain 徽标 + 条数，点击只读弹层列条目；无条目不占位。不照搬 work 完整记忆面板（取舍：轻量展示，code 页信息架构克制）。
- i18n：`uiCodex` 新增 4 key，三语齐全；反馈文案复用既有 `compactStart/Done/Fail`。
- 测试：lane 新增 memory 快照归一化/过滤/hydration 保留、`compacting` 置位复位断言。

#### 文档同步

- 设计方案与剩余项评估（含 S-1 挂起项、X-1/X-2 标记项）已归档至 `.luzeyang/code-plain-decoupling/`；R-2 方案实施修订归档于该处设计文档内。
- `docs/code-native-agent.md` §9：Plan 降级条目更新为审批闭环已落地；用量条目更新为现行降级方案（R-3 顺带）。

### 四、验证结果

- `cargo check --tests` ✅、`cargo clippy --tests` ✅（零新增 warning，项目 deny 防回流下通过）。
- 前端：`test:codex-acp`（6 套件）✅、`test:ui-language` ✅、`lint:ui` ✅、`check:architecture` ✅。
- **Rust 单测未实际运行**：本机测试 exe 启动即 `0xc0000139`（DLL 加载期失败，既有机型问题），已用 stash 对照实验证明改动前后同样失败、与本次改动无关。新增 Rust 单测经编译与静态自查，实际运行以 CI `cargo test --lib` 为准。
- 人工走查待做（需启动应用）：Plan 卡片批准/放弃/覆盖/remount 路径、chip 压缩与记忆弹层、work 侧 Plan 回归（预期零变化）。

### 五、行为兼容与遗留

- **行为不变面**：D-1/D-2/D-3/R-2 为纯重构，两模式发送语义、工具整形结果、Plan reminder 文本、磁盘数据格式均与改动前一致（各有等价性测试锁定）。
- **行为变化面（刻意）**：code 页 Plan 模式从"无审批假安全"变为真实审批闭环（R-1）；code 页新增用量 chip/压缩入口与记忆徽标（R-3）。
- **遗留**：X-1/X-2（Skills 按会话化、项目级 skills）维持标记待底座评估，挂载点已预留在 `SessionPolicy`；S-1 安全护栏挂起项后续单独分析，审批参数挂载点已就位（R-2）；`tokens.max` 上限、用量/记忆快照不持久化（重启后等下轮事件）为既有数据限制，未做补偿。

---

## 第二篇：code 会话权限默认值与 mode 持久化（PR #190）

> 关联：`docs/code-native-agent.md` §8.7（历史语义；现行三分 lane 语义以本篇第六节为准）、第一篇（前序解耦）、`.luzeyang/code-plain-decoupling/code-plain-decoupling-剩余待改动项评估.md`（S-1 背景，已归档）。
> 分支：`feat/code-mode-permission`（基于最新 main，含 #182 解耦成果）。
> 本篇登记 S-1 权限决策的实施结果，作为 PR 的验收依据。

### 一、为什么需要这个改动

code 会话执行根是用户真实项目目录，但此前权限语义有两个商业级缺口：

1. **默认暴露**：新 code 会话默认 Yolo（全自动 + 全工具 + 直写真实仓库），且 mode 不持久化——谨慎的用户切过 Plan，重启后默默回弹 Yolo，以为保护还在。
2. **无放权确认**：从只读到全自动没有任何告知环节，用户对"模型此刻可以无审批改写我的仓库"无感知。

产品决策（用户拍板，VS Code `chat.permissions.default` + 首次选 Bypass 弹警告的同款形态）：

- code 模式**首次使用默认 Plan**（R-1 已让 Plan 有真实审批体验）；
- **切 yolo 时弹一次性警告**，全局记忆，以后不再弹；
- **两层持久化**：每会话恢复各自 mode；新 code 会话默认跟随上次使用的 mode。

该方案吸收原 S-1 档 1（首绑警告）与档 3（默认 Plan）：放权动作只在切 yolo 时发生，警告时机比绑定目录时更精准。

### 二、改动总览

| 层 | 内容 |
|---|---|
| 后端 | `mode_state` 默认值解析（code→全局 last_mode→Plan；plain→Yolo 不变）；per-session mode 持久化（仅 code）；全局 `code_permission` 域；两个新命令 |
| 前端 | code 页 mode 由后端驱动（去三处写死 `'yolo'`）；切 yolo 确认门 + `NativeYoloConfirmCard` |
| 边界 | 仅品悟原生 code 会话；ACP 与 plain/work 行为逐字节不变 |

9 个文件修改 + 3 个文件新增（约 +868/-35）。

### 三、逐项改动说明

> 本节描述的两层持久化语义已被「六、语义修订」取代，留档备查。

#### 后端

- **默认值解析**（`features/sessions/mod.rs`）：`mode_state(id)` 有内存条目原样返回；无记录走 `resolved_default_mode`——code 会话取全局 `last_mode`（None→Plan），plain→Yolo。仅几次 RwLock 读、不触盘、不物化条目（chat.rs 每轮发送路径调用，保持廉价）。code 判定经 lib.rs 注入的 `code_session_predicate`（与 bridge/remote_control 共用同一份 `SessionAgentStore` 闭包，ACP 会话恒判 plain，天然排除）。
- **关键正确性修复**：plan/技能/persona/知识库挂载等 **9 处** `or_default()` 物化站点统一改走 `mode_state_entry`——否则首次默认 Plan 的 code 会话出方案时会被物化成 Yolo，plan 卡静默丢失。单测 `fresh code 会话 register_pending_plan` 钉住。
- **两层持久化**：`set_mode` 仅对 code 会话写 `~/.pinvou3/sessions/_code_mode_states.json`（仿 `_skill_bindings.json` 模式；删除会话/`reset_mode_state` 同步清理）并更新全局 `last_mode`；plain 完全不落盘。启动时加载合并进 mode_states。
- **全局键**（`platform/prefs/mod.rs`）：`UserPrefs` 新增 `code_permission { last_mode, yolo_confirmed }` 域（serde default 兼容旧 settings.json），经 `update_transaction` 字段级事务写盘——已核实所有设置写路径为补丁式，不会被设置页回写冲掉。
- **新命令**（`app/commands/interaction.rs`）：`get_code_permission_prefs() -> { last_mode, yolo_confirmed }`、`confirm_code_yolo()`。`exit_plan_to_yolo` 不做后端门控（确认是 UI 层语义，与 VS Code 同款）。
- **任务级切换不记忆**：`accept_plan` 经 `claim_pending_plan` 切 yolo 属任务级动作，不写两层持久化——重启后会话恢复其持久化 mode（刻意语义，批准方案 ≠ 变更权限偏好）。
- **确认门边界（刻意）**：yolo 一次性确认是 UI 层语义，只挂在 mode chip 的 Plan→Yolo 切换路径（`switchNativeMode`）；`accept_plan`（批准方案卡【就这么干】）同样会把会话切到 Yolo，但**不经确认门**——批准方案本身即用户对执行该方案的显式同意，与 VS Code 首次选 Bypass 弹警告同款取舍。后端 `exit_plan_to_yolo`/`claim_pending_plan` 均不做门控。

#### 前端

- **mode 后端驱动**（`CodexAcpView.jsx`）：去掉 useState 初值与两处回落共三处写死 `'yolo'`；切换/新建会话时按 `get_mode_state` + 全局偏好渲染 chip。新增纯逻辑模块 `code-permission-state.js`（fallback 矩阵：无记录→Plan、读取失败按未确认的安全方向）。
- **确认门**：`switchNativeMode` 切 yolo 前查 `yolo_confirmed`——false 弹 `NativeYoloConfirmCard`（复用审批卡风格与既有 backdrop 弹层，三语文案含"以后不再提示"），【确认】调 `confirm_code_yolo` 后继续原切换路径（含 busy 先 cancel），【取消】留在 Plan；true 直接切。草稿态（未建会话）同门，并补齐 yolo 方向暂存应用的原缺口。
- i18n：`uiCodex` 新增 5 key × zh/en/ja。

#### 测试

- Rust 7 个新单测：默认值矩阵、per-session 重启恢复与删除清理、plain 不持久化、确认标志跨重启、fresh code 会话 plan 登记、prefs 旧 JSON 兼容。
- 前端新套件 `code_permission_state.test.mjs`（纯逻辑矩阵 + 源码接线契约），并入 `test:codex-acp`。

### 四、验证结果

- `cargo fmt --check` / `cargo check --tests` / `cargo clippy --tests` ✅（零 error、零新增 warning）。
- 前端：`test:codex-acp` 全链 ✅、`lint:ui` ✅、`test:ui-language` ✅、`check:architecture` ✅。
- **Rust 单测未实际运行**：本机测试 exe 启动即 `0xc0000139`（既有机型 DLL 问题，与改动无关）；新增用例以 CI `cargo test --lib` 为准。
- 人工走查待做：首用 code 默认 Plan 出方案审批卡；首次切 yolo 确认后全局不再弹；重启各会话恢复各自 mode、新会话跟随上次；plain/ACP 回归。

### 五、行为兼容与遗留

- **行为变化面（刻意，仅原生 code 会话）**：新会话默认 Plan（原 Yolo）；mode 持久化（原重启回弹）；切 yolo 一次性确认卡。plain/work 与 ACP 零变化。
- **遗留**：S-1 档 2（危险命令护栏，防注入/防幻觉命令，与本方案互补）、S-3（危险路径拦截 + 非 git 警告）、写绑定目录外确认——见剩余项评估。

### 六、语义修订（2026-08-12，lane 语义 + 会话独立持久化；2026-09 起 design 并入 work 改两分）

真机回归发现两层语义在三种工作区模式（工作/设计/代码）下互相渗透、体验混乱，复审后拍板修订为：

> **2026-09 更新**：设计（design）lane 并入工作（work）lane，lane 语义由三分改为两分（work/code）。`ModeLane::Design` 变体删除；旧 settings.json 的 `mode_defaults.design` 仅在启动加载时一次性折叠进 work 内存镜像（work 为空时回填，不回写磁盘，此后只落 work）。场景标记字符串 `design:poster`/`design:data-visualization` 是历史持久数据，场景白名单保持合法不变。

1. **草稿态切 mode → 刷新本 lane 全局默认**：工作/代码各有独立全局默认（work 存 settings.json `mode_defaults.work`，code 沿用 `code_permission.last_mode`；原 design 默认读取折叠进 work，不再单独写入）。
2. **已生成会话切 mode → 只写会话自己的记录**，不再渗全局（`set_mode` 不再调 `record_code_last_mode`；`accept_plan` commit 也只写 per-session——原「任务级切换不记忆」条目同步废止，任务级 yolo 现纳入 per-session 持久化）。
3. **每个对话保存自己的 mode**：plain 会话废除"恒内存不持久化"，per-session 落盘扩到所有会话；sidecar 文件更名 `_session_mode_states.json`（旧 `_code_mode_states.json` 启动时回退读一次）。
4. **新会话默认 = 本 lane 全局默认**：code 由后端 `resolved_default_mode` 解析；work 由前端在会话物化（`ensureSession`）时应用（后端不再区分 plain 侧 lane）；缺省 code→Plan、work→Yolo 不变。

配套改动：新命令 `get_mode_defaults`/`set_mode_default`（web access-policy 已登记）；草稿态 chip 切换不再物化会话（旧实现 `ensureSession` 会凭空造空会话）；ChatView 经 `bridge.interaction.setModeLane` 显式同步 lane（bridge 不读 localStorage）；CodexAcpView 草稿切换即写 code lane 默认，`applyNativeDraftControls` 末尾补 `refreshNativeControls` 收口（修「发消息后 chip 回旧值」）；plain 会话 mode 重启后恢复。
