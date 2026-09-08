# CodeWhale Fork 修改清单

> 本文是 Pinvou 对 CodeWhale fork 的单一现状清单。
> 维护策略见 [`fork-policy.md`](fork-policy.md)，升级证据见 [`codewhale-upgrade-0.9.5-to-0.9.12.md`](codewhale-upgrade-0.9.5-to-0.9.12.md)。
> English: [`fork-modifications.en.md`](fork-modifications.en.md)

## 0. 当前状态（2026-09-08 · v0.9.12 r1 公开 PR 候选）

| 项 | 当前值 |
|---|---|
| 上游基线 | tag `v0.9.12`，commit `dcd4c200f72f0c1ffd60d8e7f6850313db879fc5` |
| 候选分支 | 已推送 `codex/pinvou-v0.9.12-r1`（CodeWhale PR #44），当前 head `ff299f94b0795180c76d0336152385dbd02dfa05` |
| 发布状态 | 未成为受保护基线；公开 `pinvou3-clean` 仍是 r13，`pinvou-v0.9.12-r1` 不可变 tag 尚未创建 |
| 升级前回退点 | 公开不可变 tag `pinvou-v0.9.5-r13` → `f853f8f1566c57e6be40d5439a222a932aa79ef5`；同 SHA 的本地 `backup/pre-v0.9.12-sync` 仅作便利引用 |
| 历史组织 | 上游之上 8 个签署提交，仍归属 4 个长期主题；最后三个提交收口评审确认的行为、测试与文档缺口 |
| drift | `72 files, +4848/-637`，净增 4211 行；旧 r13 为 `110 files, +10895/-1195` |
| 守护 | 36 条独立 CodeWhale `forkguard_*` 行为测试（30 条默认 + 6 条 `benchmark-eval-controls`）+ 父仓指纹与行为测试 |
| 父仓适配 | v0.9.12 EngineConfig、Agent/Plan 模式、逐轮 reasoning/安全、ExtraTools、owner 事件隔离、Automation v3/v4 数据兼容、rusqlite 0.40.2 |

## 1. 为什么本次使用 clean re-fork

旧 r13 相对 v0.9.5 修改 110 个文件。与 v0.9.12 对照时，104 个旧修改文件也被上游改动，直接三方移植预计产生 57 个冲突文件。与此同时，上游已经吸收或重构了大量旧 patch，包括会话快照/恢复、编辑上一轮、压缩后 token、provider/model 路由、原生搜索、Windows UTF-8 Shell、JSON schema 修复与任务基础设施。

因此本次从官方 `v0.9.12` tag 直接建立新分支，只重表达仍然缺失且必须位于底座生命周期内的语义；没有 merge 或 cherry-pick 旧 r13 冲突树。

## 2. 旧 r13 逐项处置

| 处置 | 能力 | v0.9.12 r1 结果 |
|---|---|---|
| 上游已有，删除 fork | 会话 snapshot/recovery、edit-last-turn、post-compaction tokens、route budget、严格直连模型大小写、JSON 容器修复、厂商原生搜索、Windows UTF-8、provider pin 等 | 采用上游实现和测试，不复制旧代码 |
| 语义迁移 | `Yolo`/`Auto` 模式、全局 reasoning、旧 custom-tools/disabled-skills 接口 | 映射到 `Agent` + approval/trust、逐轮 `Op::SendMessage` reasoning、`ExtraTools` 与显式 Skills 根 |
| 继续保留 | 宿主 facade/route limits、可靠 steer 与批量取消、MCP secret resolver、raw worker ledger、逐轮最终分发安全、64 KiB File 上限、prompt ownership、Automation conversation/schema/lifecycle | 重写到 v0.9.12 当前 Engine/Task/Prompt 结构并增加结果式测试 |
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

所有提交都含 DCO `Signed-off-by`。`b4c02616b` 包含大部分跨主题收口，历史粒度确实不利于 bisect；但候选已公开进入评审，本轮不为历史美化 force-push，而是以新的签署提交修复评审问题，并用本表、指纹和行为测试弥补审计粒度。一旦创建不可变 tag，不得重写。

## 4. T1 — 宿主嵌入与路由边界

### 保留内容

- 对宿主公开 `AppMode`、`ApprovalMode`、Automation、Task、route 和 worker ledger API。当前兼容 facade 还包含 18 个 `pub mod`，父仓 48 个 Rust 文件存在 272 处直接引用；这是已登记的收窄债务，不在本次热修中做破坏性改名。
- `resolve_runtime_route_with_limits` 保留 wire model、context/output 上限与 embedding alias。
- `EngineConfig.session_id` 在 Engine spawn 前绑定，所有子智能体/工作流事件带 owner session 并由宿主按 owner 过滤。
- `EngineHandle::steer` 返回 opaque id；`withdraw_steer` 区分已撤回与非 pending；中断、停止、压缩、换会话和 Engine drop 都使每个 id 恰好进入 committed/dropped 终态。
- `CancelSubAgents` 对当前 session 的子智能体做幂等批量取消。
- 模型侧仍以 v0.9.12 的封闭角色集为执行姿态；仅允许宿主在当前 route Config 中显式注入的 prompt-only profile 贡献身份、描述和人设。Personal/Workspace/Plugin profile 及 model/provider/reasoning/permission/delegation pin 均 fail closed。

### 关键测试

- `forkguard_embedding_route_limits_preserve_wire_alias`
- `forkguard_steer_lifecycle_withdrawal_is_bounded_and_prevents_commit`
- `forkguard_steer_lifecycle_late_withdraw_reconciles_committed_state`
- `forkguard_steer_channel_commits_live_and_drops_withdrawn_input_in_turn_loop`
- `forkguard_cancel_all_running_is_session_scoped_and_idempotent`
- `forkguard_host_profile_overlay_is_config_only_and_prompt_only`

## 5. T2 — 工具兼容与命令执行安全

### 保留内容

- `ExtraTools` 允许 app 在 Agent/Plan 原生注册宿主工具，不复制底座工具循环。
- MCP secret 只经宿主 resolver 注入；不写进程环境或普通配置文件。
- `SetDisallowedTools` 是逐会话/逐轮的权限塑形：更新后继续在该会话的 catalog 和最终调用边界拒绝匹配工具，但不热断开共享 `McpPool` 中已经建立的 server 连接。全局断连会干扰仍获授权的其他会话；底层连接由正常 pool/session 生命周期回收，安全边界由 catalog + 最终 dispatch 的 fail-closed 双检提供。
- `TurnToolSecurityPolicy` 把精确工具白名单、只读动作与 trusted external paths 下沉到每轮执行。
- 受限轮禁止动态工具、MCP、子智能体和未授权控制操作；新的显式用户消息才可安装替代权限。
- 工具在最终 backend dispatch 前再次做 exact/read-only 校验；Full Access 也不能绕过 non-bypassable approval。
- `File` 的旧/低层写入入口保留 64 KiB 硬上限；受限日志和审计不记录参数、结果或错误私密内容。

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
- `forkguard_mcp_secret_resolver_supplies_values_without_process_env_writes`
- `forkguard_write_primitive_enforces_the_64kib_boundary`
- `forkguard_write_file_enforces_the_64kib_boundary`
- `forkguard_benchmark_controls_are_explicit_and_default_off`
- `forkguard_benchmark_repairs_only_unambiguous_read_actions`
- `forkguard_benchmark_repairs_read_schema_and_attachments`
- `forkguard_benchmark_budget_truncates_batch_and_clears_followup_tool_surface`
- `forkguard_benchmark_final_only_rejects_repeated_tool_only_responses`
- `forkguard_benchmark_turn_repairs_file_aliases_before_execution`

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
- `forkguard_accepts_legacy_v4_but_rejects_newer_task_schema`
- `forkguard_terminal_task_delete_refuses_active_and_is_idempotent`
- `forkguard_terminal_automation_run_delete_refuses_active_and_is_idempotent`

## 8. 父仓适配边界

- `pinvou3-app` 负责产品工具白名单、AppMode 到 approval/trust 的映射、reasoning effort、会话 owner 过滤和定时会话创建。
- bridge 保留 v0.9.12 的有限轮次/工具预算、read denylist、bubblewrap、MCP OAuth、goal loop 与 telemetry 安全默认值。
- `session_id` 必须在 `Engine::spawn` 前进入 `EngineConfig`；不得事后依赖事件猜归属。
- 旧的全局 disabled-skills 调用已删除；包开关通过显式 bundle/registry 和每会话 disallowed tools 生效。

## 9. 软上限评估与后续减量

当前净增 4211 行，超过 1500 行软上限；`engine.rs`、`turn_loop.rs` 和 `engine/tests.rs` 也超过单文件 200 行提示线。原因是状态机、最终分发安全、“完整专家人设仅进入被选子智能体”的 spawn 边界、bundle Skills 排除 ambient 文件源的权限边界，以及评审要求的结果式生命周期/评测回归必须和 v0.9.12 原生 Engine 同步，拆成 app 侧镜像会形成更危险的双状态源。当前 Rust/Clippy 兼容只包含等价重写、`Default` 补全和窄 lint 说明，不改变公开函数签名或新增运行语义。

后续减量顺序：

1. 将可靠 steer lifecycle 与逐轮 exact/read-only dispatch 作为通用 host API 上游化。
2. 将 Automation misfire/no-overlap/conversation ownership 上游化。
3. 将 `FragmentId::Permissions` 的显式 100 KiB 宿主预算做成通用的可配置 fragment limit，上游化后删除固定例外。
4. 父仓先迁移到明确 re-export API，再分批收窄当前 18 个 `pub mod` 兼容 facade。
5. 上游提供完整 static-composer/explicit-skills-root 契约后删除 T3 patch。
6. 每次上游 release 重新核对已吸收项，不保留兼容壳。

## 10. 发布与回退

- 公开回退点是不可变 tag `pinvou-v0.9.5-r13`；本地 `backup/pre-v0.9.12-sync` 不是发布前提。
- 当前候选分支虽已通过 PR #44 公开，但尚未进入 `pinvou3-clean` 且没有不可变 tag，因此仍不满足“公开 submodule 基线可达”。
- 获得明确授权后，先推送 `pinvou3-clean`，再创建不可变 `pinvou-v0.9.12-r1`，最后让父仓 gitlink 指向同一 SHA 并执行 `scripts/verify-public-submodule.sh`。
- 未授权时不得降低公开校验或把本地 object 当成发布成功。
