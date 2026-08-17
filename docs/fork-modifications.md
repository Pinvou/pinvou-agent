# CodeWhale Fork 修改清单

> 本文是 Pinvou 对 CodeWhale fork 的单一现状清单。
> 基线、主题边界、守护指纹和同步结论以本文与 `docs/fork-policy.md` 为准。
> English: [`docs/fork-modifications.en.md`](fork-modifications.en.md)

## 0. 当前状态（2026-08-17 · v0.9.5 r7 公开基线）

| 项 | 当前值 |
|---|---|
| 上游基线 | tag `v0.9.5`，commit `853cb707bbcf4f7dc4268fba6d811e0d04083f9c` |
| 公开维护分支 | `Pinvou/CodeWhale:pinvou3-clean`，head `a36e6cd533024cfe5724bae21875aea42b2ed87a` |
| 已合并修复 | `Pinvou/CodeWhale#9`、`#11`、`#12`、`#13` 已合并 |
| 公开状态 | `pinvou3-clean` 与固定标签 `pinvou-v0.9.5-r7` 均指向公开维护分支 head；旧标签保留为不可变历史 |
| 旧基线备份 | tag `pinvou-v0.9.0-r4` + branch `backup/pinvou3-clean-v0.9.0-r4`，均指向 `03e9e1027c03ce1e4b35ab9e3ccce751b65b9624` |
| 组织方式 | 从 `v0.9.5` clean re-fork 的 4 个长期主题；公开 9 个线性提交 |
| drift | r7 公开基线 `46 files changed, +1852/-269` |
| 守护 | r7 保留 CodeWhale `forkguard_*` 行为测试 + Tauri/Web 展示回归；完整 guard 结果见第 4 节 |
| 父仓适配 | gitlink、`Cargo.lock`、`EngineConfig` v0.9.5 字段适配 |

### r7 逐轮评测工具安全扩展（PR 候选）

> CodeWhale PR #15 候选提交 `1eca6103a`：为嵌入宿主增加进程内逐轮工具安全策略、可信外部路径完全覆盖和最终执行前精确白名单门禁。快速 guard 仅额外接受这个直接位于 r7 head 之上的单提交拓扑；公开标签校验仍固定为 `pinvou-v0.9.5-r7`。

- 部分 OpenAI 兼容后端会返回结构上合法、但把 schema 声明的嵌套 object/array 再编码成 JSON 字符串的工具参数；这会让 `request_user_input` 等强类型工具在进入业务校验前失败。
- T2 只在 schema 明确声明 object/array、字符串为不超过 64 KiB 的严格 JSON 且解码类型一致时修复该容器；普通文本和数字/布尔字符串不做宽松转换，修复后仍执行原工具校验。
- 重复工具调用触发 `stuck_guard`，或连续工具错误触发 degradation hint 时，内部策略提示折叠进对应 `tool_result`，不再为这两条路径追加独立 runtime `user` 消息，避免严格 OpenAI 兼容后端因角色序列拒绝下一轮。
- Tauri/Web bridge 只在工具卡展示投影中剥离 `stuck_guard` / `tool_error_degradation` 两种已知内部 suffix；持久化消息和送模上下文保持原样。
- 本次不覆盖真实用户 `pending_steers`，也不覆盖循环入口 steer、LSP diagnostics 或 subagent handoff 等其他 runtime 注入路径；这些路径涉及用户权限和上下文语义，不在本 PR 中引入全局角色 normalizer 或虚构 assistant 消息，后续单独设计处理。
- 底座修复已通过 `Pinvou/CodeWhale#12` squash 合入，公开 commit 为 `3bbf8421ebdb16bff71f83dac4d42c8fb65f0f02`，并由固定标签 `pinvou-v0.9.5-r6` 发布。
- 通用 schema 参数修复已通过 `Hmbown/CodeWhale#5348` 合入官方上游；最新上游已移除 `stuck_guard` 和本 fork 的连续错误 degradation 路径，因此角色续轮兼容只保留在当前 v0.9.5 fork 生命周期内。

### r5 会话恢复修复（已验证并发布）

- v0.9.5 的 `load_session` 会把无配对 `tool_use` 视为进程崩溃并立即补写失败结果；Pinvou 运行中持久化工具调用后再次读取同一会话时，这一假设并不成立。
- 底座修复已通过 `Pinvou/CodeWhale#11` 合入，公开 commit 为 `2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd`。
- T1 新增无修复副作用的 `load_session_snapshot` 与显式 `recover_session_for_resume`。Pinvou 的运行时读改写统一使用前者，仅在应用进程启动、任何 Engine 接管会话前执行后者，并把恢复结果原子落盘。
- 前端仅对真正的跨端回合保留 revision 对账门禁；本地 `chat:done` 直接释放下一轮发送，落盘读回异常不得阻塞普通本地对话，跨端未收敛提示按会话去重。
- 本次新增 2 条 CodeWhale `forkguard_*`、2 条父仓 `forkguard_*` 和 Tauri/Web 前端行为回归，分别锁定运行时无副作用读取、显式恢复可观测与幂等、二次 Store 打开安全、启动恢复落盘以及本地完成后连续发送。
- 本节改动已计入上方公开维护分支 head、drift 和固定标签 `pinvou-v0.9.5-r5`；CodeWhale required checks 与父仓自动测试均已通过。

### 软上限评估

r7 已移除三省六部专用编排协议，公开 drift 收敛为 `46 files changed, +1852/-269`。当前保留量集中在宿主嵌入、工具安全、Skill 来源与 Automation 生命周期；逐轮评测权限作为 T2 的单提交候选，不恢复已删除的产品专用协议。

## 1. 五个长期 fork 主题

### T1：宿主嵌入与路由边界

- **公开 commits**：`331cb1594688c723d98499d9ca11f05af291b599`、`2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd`（`Pinvou/CodeWhale#11`）。
- **公开规模**：10 文件，`+394/-31`；仓库级 CI 恢复不计入 T1 主题规模。
- **核心文件**：`crates/tui/src/lib.rs`、`core/engine.rs`、`route_runtime.rs`、`runtime_threads.rs`、`automation_manager.rs`、`session_manager.rs`。
- **内容**：
  - 在 v0.9.5 原生 library target 上只公开 Pinvou 实际使用的模块和宿主类型，不恢复旧的全量 bin facade。
  - 以根级窄重导出公开 `FleetRoster` 与工作区角色目录常量，供嵌入宿主在写入角色文件后装配和热刷新名册；不公开整个 `fleet` 模块。
  - 提供只读持久化 worker 投影，供 live 宿主结合自身进程纪元判断状态；恢复入口仍按 v0.9.5 原语把孤儿 worker 收敛为 interrupted。
  - 提供 opaque resolved route、显式 route limits 和 embedding host route override。
  - 保留宿主需要的 runtime thread / Automation 接口和 `EngineConfig` 注入边界。
  - 将无副作用的运行时 session snapshot 与已知进程重启后的显式 tool history recovery 分开，避免嵌入宿主把仍在执行的工具调用误判为崩溃。
- **边界**：不实现 Pinvou 产品工具策略，不包含三省六部完成语义。
- **守护**：`forkguard_embedding_route_limits_preserve_wire_alias`、`forkguard_runtime_session_snapshot_preserves_in_flight_tool_call`、`forkguard_explicit_session_recovery_is_reported_and_idempotent_after_save`，以及父仓启动恢复、resolved-route 和 compaction 合约测试。

### T2：工具兼容与命令执行安全

- **commits**：`595adce47e2d1bcf895d7bfd6426c074eb969324`、`3bbf8421ebdb16bff71f83dac4d42c8fb65f0f02`（`Pinvou/CodeWhale#12`）。
- **规模**：初始主题 15 文件，`+181/-98`；r6 兼容修复另改 5 文件，`+463/-30`。
- **核心文件**：`core/engine.rs`、`core/engine/{dispatch,turn_loop,tool_setup}.rs`、`core/ops.rs`、`tools/file.rs`、`command_safety.rs`、`tools/shell.rs`、`docs/TOOL_SURFACE.md`。
- **内容**：
  - `EngineConfig.extra_tools` 让宿主工具在 Plan、Agent、Yolo 等 turn registry 中一致注册。
  - `SetDisallowedTools` 支持工具商店、知识库和会话策略在不重建 Engine 的情况下动态收窄工具面。
  - 复用 v0.9.5 原生 `allowed_tools` 作为硬白名单入口；Pinvou 名单由 app 构造，底座不维护产品 blocklist。
  - `File` 写入保持 64 KiB 单次内容上限，并在落盘前拒绝超限输入。
  - 多行 Shell 按 segment 检查；破坏性命令在自动批准模式下仍被阻断。
  - schema 明确要求 object/array 时，窄修复模型输出的严格 JSON 字符串容器；不做 primitive coercion，业务工具仍执行自身校验。
  - `stuck_guard` 与连续工具错误 degradation 提示折叠进对应 `tool_result`，保持这两条续轮路径的 provider 角色序列合法；应用 bridge 只从工具卡展示值剥离这两种已知内部 suffix。
  - 当前工具面不恢复已退役的独立追加文件工具，也不放宽 `request_user_input` 的问题数量、字段和选项校验。
  - 本地未发布的逐轮安全策略在 catalog 与最终 dispatch 共用 exact allowlist；显式受限轮次清空 trusted roots 时不再追加持久信任目录或剪贴板目录，并禁止 MCP 初始化、控制面 shell、动态工具和子 Agent。`None` 保持现有 GUI 行为。
- **边界**：不包含 Skill 来源、Automation 或三省六部角色协议。
- **守护**：`forkguard_host_extra_tools_register_in_all_modes`、`forkguard_file_content_caps_reject_before_writing`、`forkguard_multiline_still_blocks_destructive_segments`、`forkguard_schema_bound_json_container_repair_accepts_nested_payload`、`forkguard_schema_bound_json_container_repair_rejects_wrong_or_unbounded_values`、`forkguard_stuck_guard_warning_is_embedded_in_tool_result_content`、`forkguard_stuck_guard_tool_warning_preserves_provider_role_sequence`、`forkguard_tool_error_degradation_preserves_provider_role_sequence`、`forkguard_session_trusted_roots_override_persisted_workspace_trust`、`forkguard_dispatch_allowlist_rejects_forged_calls_before_all_dispatch_backends`，以及 Tauri/Web 工具卡展示回归。

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
- 定时任务页面、通知、三省六部页面和业务日志展示。

CodeWhale fork 只提供这些产品能力不可缺少的底座生命周期入口和原子不变量。

## 3. v0.9.5 同步结论

### 上游已有，不再维护

- v0.9.5 原生 library/runtime crate 边界：T1 只保留必要公开面。
- 原生 `allowed_tools`：Pinvou 白名单直接复用，不恢复 fork-only 第二套白名单字段。
- 通用 OAuth 取消、Fleet roster、Runtime API、MCP registry 和 session-tree：直接使用上游。
- canonical action 工具面：不恢复旧独立工具名。

### v0.9.5 新增适配

- `EngineConfig` 新增 `subagent_state_root`；父仓按 `SessionRoots` 显式设置：执行根保持任务目录，delegated-agent 状态根使用会话 ledger。
- Pinvou 全局专家池通过原生 `fleet.profiles` 配置提供给 Engine；不再借 `subagent_state_root` 或每会话角色文件承载专家定义，个人/项目 profile 的原生覆盖优先级保持不变。
- 已删除的旧 `hidden_tools` 字段不再恢复；Pinvou 原有动态隐藏行为本就通过 `disallowed_tools` 完成。
- v0.9.5 WorldState 40 KiB fragment cap 只对 Permissions 做 100 KiB 窄例外，其他 fragment 不变。
- v0.9.5 workspace crate 拆分引起父仓 `Cargo.lock` 重算，未增加 Pinvou 直接依赖。

## 4. 验证

r6 当前已通过：

```text
cargo fmt --all -- --check
cargo test -p codewhale-tui --lib --locked forkguard_ -- --test-threads=1
29 passed / 0 failed
node pinvou3-app/tests/scheduled_tasks_unit.test.js
PASS scheduled tasks unit
npm run lint:ui
npm run build:ui
npm test
122 passed / 0 failed；pet asset validation passed
./scripts/fork-guard.sh
CodeWhale 29 passed；pinvou3-app 20 passed
python3 scripts/architecture-guard.py
architecture guard passed
```

以下为 r5 公开基线发布时通过的完整父仓 Rust 构建矩阵；r6 本轮重复了上方行为门禁、Node/UI 和架构检查，未重复以下全部 Rust 构建矩阵：

```text
cargo fmt --all -- --check
cargo check --locked
cargo test --locked --lib -- --test-threads=1
1077 passed / 0 failed / 12 ignored
python3 scripts/architecture-guard.py
npm test
npm run lint:ui
npm run build:ui
npm run build:web
cargo build --locked --no-default-features --features local-embed --bin pinvou3-tauri
```

完整结果见 `docs/codewhale-upgrade-0.9.0-to-0.9.5.md`。12 个 ignored 测试依赖真实模型、外部工具或专用 fixture；公开标签一致性由 `scripts/verify-public-submodule.sh` 校验，不得以修改脚本方式绕过。

## 5. 后续修改规则

- 修改任一主题时，同步更新本文、`scripts/fork-guard.sh` 和对应 `forkguard_*` 行为测试。
- 通用修复从 upstream main 建净分支贡献；不得把整个 Pinvou 主题直接提交上游。
- 发布后把本节状态更新为远端维护分支、不可变标签和实际 commit，并验证父仓 gitlink 一致。
