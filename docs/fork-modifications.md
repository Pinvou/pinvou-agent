# CodeWhale Fork 修改清单

> 本文是 Pinvou 对 CodeWhale fork 的单一现状清单。
> 基线、主题边界、守护指纹和同步结论以本文与 `docs/fork-policy.md` 为准。
> English: [`docs/fork-modifications.en.md`](fork-modifications.en.md)

## 0. 当前状态（2026-08-20 · v0.9.5 r7 + PinvouOS feature checkpoints）

| 项 | 当前值 |
|---|---|
| 上游基线 | tag `v0.9.5`，commit `853cb707bbcf4f7dc4268fba6d811e0d04083f9c` |
| 公开维护分支 | `Pinvou/CodeWhale:pinvou3-clean`，head `a36e6cd53` |
| 已合并修复 | `Pinvou/CodeWhale#9`、`#11`、`#12`、`#13` 已合并；公开维护分支固定于 `pinvou-v0.9.5-r7` |
| 发布状态 | `pinvou3-clean` 与 `pinvou-v0.9.5-r7` 仍指向 `a36e6cd533024cfe5724bae21875aea42b2ed87a`；`feat/pinvouos-front-round-policy` 与 PinvouOS feature 父仓 gitlink 精确指向 `2f1f851ed038ffa161b42404bf478b1d9d4aeff2` |
| 旧基线备份 | tag `pinvou-v0.9.0-r4` + branch `backup/pinvou3-clean-v0.9.0-r4`，均指向 `03e9e1027c03ce1e4b35ab9e3ccce751b65b9624` |
| 组织方式 | 从 `v0.9.5` clean re-fork 的 4 个当前长期主题；专用编排主题由 PR #13 整体撤销 |
| drift | r7 公开基线 `46 files, +1852/-269`；checkpoint B 相对 A `15 files, +1547/-129`；feature 累计相对 r7 `15 files, +1849/-135`；累计相对 upstream `50 files, +3678/-381` |
| 守护 | r7 公开基线 23 条 + PinvouOS feature 15 条（3 条 direct-round、12 条 completion-boundary），CodeWhale `forkguard_*` 共 38 条；另有通用工具兼容与父仓指纹/行为测试 |
| 父仓适配 | gitlink、`Cargo.lock`、`EngineConfig` v0.9.5 字段适配 |

### PinvouOS feature checkpoint A：Direct 工具轮次边界

- 为 PinvouOS Front 增加通用、默认关闭的 `DirectToolRoundPolicy` Engine 配置。底座只按“一个 assistant 工具批次算一轮”计数，在宿主设定的轮次耗尽后把工具面收窄到指定 handoff 工具，并在 handoff 执行后关闭本轮工具面。
- Pinvou app 将该策略配置为最多 3 轮 direct 工具批次、溢出时仅保留 `agent`；普通 CodeWhale 与其他 Pinvou Engine 保持 `None`，行为不变。产品如何判断完成、何时编排仍由 app instructions 定义。
- `forkguard_direct_tool_round_budget_narrows_to_one_handoff_then_closes` 锁定模型可见目录；另外两条真实 Engine 回归锁定执行门：旧/幻觉 direct 工具不能绕过收窄，handoff 后任何工具都不能继续执行。
- CodeWhale feature 分支为 `feat/pinvouos-front-round-policy`，commit `3f64e41e971167aede9390dbecc0a307224562ba`。它不移动 r7 公开 head 与不可变标签；父仓 feature 通过 gitlink 精确固定该提交。

### PinvouOS feature checkpoint B：Front 插话与后台完成回流边界

- 新增通用、默认保持 `Eager` 的 `SubAgentCompletionDeliveryPolicy`。PinvouOS Front 选择 `BoundaryOnly` 后，后台子 Agent 完成不会再注入当前用户 turn，而是在完整 turn 边界后以 typed `SubAgentHandoff` provenance 开启独立回流 turn；普通 CodeWhale 与 host-managed 显式 claim 行为不变。
- `HoldSubAgentCompletions` / `ReleaseSubAgentCompletions` 提供快速的 mailbox admission 与精确释放；`Acquire → Applied → Confirm → Confirmed(active)` 两阶段 barrier 另外保证 Host forwarder 已串行处理此前的 turn 终态，普通 chat 只有在 barrier 确认后才 reserve、认领本地 turn 或创建乐观回答。Pinvou app 仅为普通 chat 建立 renderer/session-local turn lease；同一 Host 的 FIFO 可以跨多个用户 turn 保持 lease，但不同 Host 之间不宣称全局 FIFO。
- holder id 最多 128 bytes、重复 Hold 幂等、Release 只影响同 id。Engine 只在 idle 边界为每个 holder 独立计算 30 秒 fail-open；活跃 turn 不消耗期限，一个存活 Host 的心跳也不会延长另一个已崩 Host 的 holder。
- BoundaryOnly 在 channel frame 丢失时会从 manager 的当前 boot/root 未交付终态恢复一次；live holder 与已进入 mailbox 的用户操作也先于 Engine 自有 goal continuation。Web Host 对 RPC timeout / outcome-unknown 使用稳定 request id，并把该 FIFO 队首停在 `uncertain`：只有精确权威 user/terminal 事件或用户取消才能收敛，绝不自动换 id 重发。
- 12 条新增 `forkguard_*` 锁定 active-turn 隔离、独立 handoff、已入 mailbox 用户操作优先、跨 FIFO turn lease、精确释放与控制前进、idle-only 过期、per-holder 心跳隔离、manager-only terminal once、goal continuation 排序、两阶段匹配确认、非 BoundaryOnly fail-closed，以及默认 Eager / host-managed 兼容。
- CodeWhale feature 分支为 `feat/pinvouos-front-round-policy`，checkpoint commit `2f1f851ed038ffa161b42404bf478b1d9d4aeff2`；父仓 gitlink 与公开 feature ref 精确指向该提交。它不移动 `pinvou3-clean` 或不可变标签 `pinvou-v0.9.5-r7`。

### 本次会话修复（已验证并发布）

- v0.9.5 的 `load_session` 会把无配对 `tool_use` 视为进程崩溃并立即补写失败结果；Pinvou 运行中持久化工具调用后再次读取同一会话时，这一假设并不成立。
- 底座修复已通过 `Pinvou/CodeWhale#11` 合入，公开 commit 为 `2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd`。
- T1 新增无修复副作用的 `load_session_snapshot` 与显式 `recover_session_for_resume`。Pinvou 的运行时读改写统一使用前者，仅在应用进程启动、任何 Engine 接管会话前执行后者，并把恢复结果原子落盘。
- 前端仅对真正的跨端回合保留 revision 对账门禁；本地 `chat:done` 直接释放下一轮发送，落盘读回异常不得阻塞普通本地对话，跨端未收敛提示按会话去重。
- 本次新增 2 条 CodeWhale `forkguard_*`、2 条父仓 `forkguard_*` 和 Tauri/Web 前端行为回归，分别锁定运行时无副作用读取、显式恢复可观测与幂等、二次 Store 打开安全、启动恢复落盘以及本地完成后连续发送。
- 本节改动已计入上方公开维护分支 head、drift 和固定标签 `pinvou-v0.9.5-r5`；CodeWhale required checks 与父仓自动测试均已通过。

### PR #13 退役发布

- **合并 commit**：`a36e6cd533024cfe5724bae21875aea42b2ed87a`；已通过 `Pinvou/CodeWhale#13` squash 合并并发布为 `pinvou-v0.9.5-r7`。
- 删除专用角色派发字段、结构化提交入口、文件完成闸和对应 TUI 投影，不再让产品协议进入通用 SubAgent 生命周期。
- 保留宿主取消所有运行中子智能体的窄操作，以及通用完成事件的 `failed` 终态；桌面停止/回收仍不会遗留后台子任务。
- 新增 `forkguard_host_bulk_cancel_stops_all_running_children_idempotently`，锁定批量取消和重复取消行为。
- 修复退役后两处通用兼容回归：MCP registry 提示恢复 canonical `Bash(action="run")` / `Web(action="fetch")`，Custom SubAgent allowlist 的旧 action alias 继续解析到已注册的 canonical family。

### 软上限评估

净增量高于 1500 行软线，主要保留量来自 Automation 持久化、会话恢复、工具兼容和嵌入上下文密封：

- T4 `+373/-24`：稳定 conversation/thread 关联、Pinvou 历史 schema 兼容、misfire/no-overlap 和终态级联清理必须与 Task/Automation 持久化原子完成。
- T3 `+253/-71`：嵌入宿主的静态指令、ambient context 和 Skill 单根来源必须在模型上下文生成前密封。

本轮不为压数字复制底座状态机到 app。后续减量顺序：T1 通用 embedding route API、T2 通用命令安全、T4 通用 Automation 生命周期；T3 的 Pinvou 产品语义继续留 fork。

## 1. 四个长期 fork 主题

### T1：宿主嵌入与路由边界

- **commits**：`331cb1594688c723d98499d9ca11f05af291b599`、`2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd`（`Pinvou/CodeWhale#11`）、`a36e6cd533024cfe5724bae21875aea42b2ed87a`（`Pinvou/CodeWhale#13`）、`2f1f851ed038ffa161b42404bf478b1d9d4aeff2`。
- **公开规模**：10 文件，`+394/-31`；仓库级 CI 恢复不计入 T1 主题规模。
- **核心文件**：`crates/tui/src/lib.rs`、`core/engine.rs`、`core/engine/turn_loop.rs`、`core/events.rs`、`core/ops.rs`、`route_runtime.rs`、`runtime_threads.rs`、`automation_manager.rs`、`session_manager.rs`。
- **内容**：
  - 在 v0.9.5 原生 library target 上只公开 Pinvou 实际使用的模块和宿主类型，不恢复旧的全量 bin facade。
  - 以根级窄重导出公开 `FleetRoster` 与工作区角色目录常量，供嵌入宿主在写入角色文件后装配和热刷新名册；不公开整个 `fleet` 模块。
  - 提供只读持久化 worker 投影，供 live 宿主结合自身进程纪元判断状态；恢复入口仍按 v0.9.5 原语把孤儿 worker 收敛为 interrupted。
  - 提供 opaque resolved route、显式 route limits 和 embedding host route override。
  - 保留宿主需要的 runtime thread / Automation 接口和 `EngineConfig` 注入边界。
  - 将无副作用的运行时 session snapshot 与已知进程重启后的显式 tool history recovery 分开，避免嵌入宿主把仍在执行的工具调用误判为崩溃。
  - 提供通用的宿主批量取消操作和失败终态标记，供会话停止与 Engine 回收安全收敛后台子智能体。
  - 提供默认关闭的后台完成边界交付策略、typed turn provenance、快速 Hold/Release 与经过 forwarder 水位确认的两阶段 barrier；opaque holder id 有 128-byte 上限并由 per-holder idle watchdog 回收，产品 FIFO、显示与 heartbeat 仍由嵌入宿主负责。
- **边界**：不实现 Pinvou 产品工具策略、跨 Host 全局 FIFO 或耐久消息队列；30 秒仅是 renderer 消失后的 idle fail-open，不是任务 SLA。
- **守护**：除既有 embedding/session/cancel 守护外，本轮 12 条 completion-boundary 守护覆盖 active-turn 隔离、queued-op 优先、跨 FIFO turn lease、matching release、idle fail-open、per-holder heartbeat、默认 Eager、host-managed explicit claim、manager-only terminal once、goal continuation 排序、两阶段 matching confirm 和非 BoundaryOnly fail-closed；父仓另锁定 reserve/ownership 前 barrier、Web outcome-unknown 与语音/队列行为。

### T2：工具兼容与命令执行安全

- **commits**：`595adce47e2d1bcf895d7bfd6426c074eb969324`、`3bbf8421ebdb16bff71f83dac4d42c8fb65f0f02`（`Pinvou/CodeWhale#12`）、`a36e6cd533024cfe5724bae21875aea42b2ed87a`（`Pinvou/CodeWhale#13`）、`3f64e41e971167aede9390dbecc0a307224562ba`。
- **核心文件**：`core/engine.rs`、`core/engine/tool_setup.rs`、`core/ops.rs`、`tools/file.rs`、`command_safety.rs`、`tools/shell.rs`、`docs/TOOL_SURFACE.md`。
- **内容**：
  - `EngineConfig.extra_tools` 让宿主工具在 Plan、Agent、Yolo 等 turn registry 中一致注册。
  - `SetDisallowedTools` 支持工具商店、知识库和会话策略在不重建 Engine 的情况下动态收窄工具面。
  - 可选 `DirectToolRoundPolicy` 在单个 turn 内按 assistant 工具批次计数；达到宿主上限后只保留显式 handoff 工具，handoff 完成后关闭该 turn 工具面。默认 `None`，不改变上游/通用 Engine 行为。
  - 复用 v0.9.5 原生 `allowed_tools` 作为硬白名单入口；Pinvou 名单由 app 构造，底座不维护产品 blocklist。
  - `File` 写入保持 64 KiB 单次内容上限，并在落盘前拒绝超限输入。
  - 多行 Shell 按 segment 检查；破坏性命令在自动批准模式下仍被阻断。
  - schema 约束的 JSON 容器兼容、工具续轮 provider 角色顺序和已知内部 runtime suffix 展示清理继续沿用 r6 行为。
  - registry-first 提示只引用 canonical action；Custom SubAgent 的显式旧 action allowlist 通过 alias 映射解析到 canonical family，不扩大实际工具权限。
  - 当前工具面不恢复已退役的独立追加文件工具，也没有改动 `request_user_input`。
- **边界**：不包含 Skill 来源、Automation 或产品角色协议。
- **守护**：`forkguard_host_extra_tools_register_in_all_modes`、`forkguard_direct_tool_round_budget_narrows_to_one_handoff_then_closes`、`forkguard_file_content_caps_reject_before_writing`、`forkguard_multiline_still_blocks_destructive_segments`、registry prompt 和 Custom allowlist alias 回归。

### T3：嵌入上下文与技能来源

- **commit**：`5a9f52941b83452c1e8b76c2d679bac315edcf70`
- **规模**：13 文件，`+253/-71`
- **核心文件**：`prompts.rs`、`project_context.rs`、`repo_law.rs`、`model_context/{fragment,world_state}.rs`、`skills/`、`tools/skill.rs`、`working_set.rs`。
- **内容**：
  - static prompt composer 由 app 接管时，停用 ambient project context 和 repo law，避免用户目录文件隐式进入系统上下文。
  - Skill 只从宿主显式 `skills_dir` 扫描；disabled Skill 同时从目录和 `load_skill` 消失。
  - `FragmentId::Permissions` 单独沿用 100 KiB instruction 上限，其他 WorldState fragment 保持 v0.9.5 的 40 KiB 上限，避免全局放宽。
  - 用户消息前置内部 `<system-reminder>` 不参与 Working Set 路径提取，历史原文保持不变。
- **边界**：app 负责生成和选择 bundle/会话 Skill 根；底座只保证显式来源与上下文不变量。
- **守护**：`forkguard_runtime_loader_ignores_ambient_project_authority`、`forkguard_instruction_fragment_preserves_content_beyond_default_cap`、`forkguard_disabled_skill_is_neither_rendered_nor_loadable`、`forkguard_working_set_ignores_leading_system_reminder_paths`。

### T4：定时任务与运行生命周期

- **commit**：`fc84f7d3e5dca0e3db404d43e218597764129f9b`
- **规模**：4 文件，`+373/-24`
- **核心文件**：`automation_manager.rs`、`task_manager.rs`、`tools/automation.rs`、`tui/automation_routing.rs`。
- **内容**：
  - Automation 透传选定 model，并以 automation id 建立稳定 conversation key。
  - 保持 v0.9.5 当前 task schema v2，同时兼容读取 Pinvou 历史 v3/v4，拒绝未知更新 schema；thread/turn 链接跨 worker 边界及时持久化。
  - HOURLY 调度保持创建时刻锚点；休眠/关机错过时段不补跑，存在 queued/running run 时不重叠执行。
  - 只清理终态 run/task，并级联删除相应 artifact；活动运行保持可恢复。
  - 强制审批不能被通用 auto-approve 绕过。
- **边界**：app 负责展示、通知和业务工作区；底座负责调度与耐久运行事实。
- **守护**：`forkguard_scheduler_skips_offline_misfires_without_backfill`、`forkguard_scheduler_does_not_overlap_active_automation_run`、`forkguard_conversation_key_and_created_thread_survive_worker_boundary`、`forkguard_accepts_pinvou_v4_tasks_but_rejects_unknown_newer_schema`。


## 2. 父仓能力与 fork 的分界

以下能力保留在 `pinvou3-app`，不进入 CodeWhale fork：

- `features/assistant/tool_policy.rs`：Pinvou canonical tools 白名单和 MCP namespace 策略。
- `disallowed_tools` 的会话/连接器动态取值与工具商店开关。
- bundle instructions、按会话 Skill 组合目录、用户 AGENTS 注入。
- UI、Tauri IPC、工作区与产物卡、Shell 输出观察和前端终态对账。
- 定时任务页面、通知和业务日志展示。

CodeWhale fork 只提供这些产品能力不可缺少的底座生命周期入口和原子不变量。

## 3. v0.9.5 同步结论

### 上游已有，不再维护

- v0.9.5 原生 library/runtime crate 边界：T1 只保留必要公开面。
- 原生 `allowed_tools`：Pinvou 白名单直接复用，不恢复 fork-only 第二套白名单字段。
- 通用 OAuth 取消、Fleet roster、Runtime API、MCP registry 和 session-tree：直接使用上游。
- canonical action 工具面：不恢复旧独立工具名。

### v0.9.5 新增适配

- `EngineConfig` 新增 `subagent_state_root`，父仓显式透传默认值。
- 已删除的旧 `hidden_tools` 字段不再恢复；Pinvou 原有动态隐藏行为本就通过 `disallowed_tools` 完成。
- v0.9.5 WorldState 40 KiB fragment cap 只对 Permissions 做 100 KiB 窄例外，其他 fragment 不变。
- v0.9.5 workspace crate 拆分引起父仓 `Cargo.lock` 重算，未增加 Pinvou 直接依赖。

## 4. 验证

CodeWhale 当前已通过：

```text
cargo fmt --all -- --check
cargo check -p codewhale-tui --lib --locked
cargo test -p codewhale-tui --lib --locked forkguard_ -- --test-threads=1
cargo test -p codewhale-tui --lib --locked -- --test-threads=1
38 条 forkguard 全部通过；全量 lib：10087 passed / 0 failed / 11 ignored
```

进程级 integration 聚合运行得到 `261 passed / 2 failed`；两条失败都位于既有 telemetry 共享夹具，分别以原命令精确单跑时 `2 / 2` 通过。`cargo clippy --workspace --all-targets --locked -- -D warnings` 仍被 29 条既有基线 lint 阻断（包括已登记的 `private_interfaces`），本 checkpoint 没有把它写成已通过门禁。

父仓本 checkpoint 当前已通过 `cargo fmt`、locked `cargo check`、完整 Rust lib（`1381 passed / 0 failed / 12 ignored`）、完整 fork guard、架构 guard、公开 submodule 校验、UI lint 以及桌面/Web UI build。全量 Node 自动发现为 `151 passed / 1 failed`，唯一失败是本轮 diff 之外的既有 macOS overlay `fullscreen` 配置断言。因此本 checkpoint 不声称全仓 `npm test` 全绿。

r7 发布时的历史产品门禁如下；这组数字不是 2026-08-20 候选的新鲜证据：

```text
cargo fmt --all -- --check
cargo check --locked
cargo test --locked --lib -- --test-threads=1
1220 passed / 0 failed / 12 ignored
./scripts/fork-guard.sh
CodeWhale 23 passed；pinvou3-app 19 passed
python3 scripts/architecture-guard.py
npm test
npm run lint:ui
npm run build:ui
npm run build:web
cargo build --locked --no-default-features --features local-embed --bin pinvou3-tauri
```

上述 r7 历史结果见 `docs/codewhale-upgrade-0.9.0-to-0.9.5.md`。当前 checkpoint 已通过完整 `scripts/fork-guard.sh`（CodeWhale 38 / pinvou3-app 19）、`scripts/architecture-guard.py` 与 `scripts/verify-public-submodule.sh`。`scripts/verify-public-submodule.sh` 分别锁定不可变公开基线标签 `pinvou-v0.9.5-r7` 与已登记 feature ref，并要求当前父仓 gitlink 精确匹配该 feature ref，不再把 r7 基线误当成 feature checkpoint。

## 5. 后续修改规则

- 修改任一主题时，同步更新本文、`scripts/fork-guard.sh` 和对应 `forkguard_*` 行为测试。
- 通用修复从 upstream main 建净分支贡献；不得把整个 Pinvou 主题直接提交上游。
- 发布后把本节状态更新为远端维护分支、不可变标签和实际 commit，并验证父仓 gitlink 一致。
