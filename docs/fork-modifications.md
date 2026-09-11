# CodeWhale Fork 修改清单

> 本文是 Pinvou 对 CodeWhale fork 的单一现状清单。
> 维护策略见 [`fork-policy.md`](fork-policy.md)，升级证据见 [`codewhale-upgrade-0.9.5-to-0.9.12.md`](codewhale-upgrade-0.9.5-to-0.9.12.md)。
> English: [`fork-modifications.en.md`](fork-modifications.en.md)

## 0. 当前状态（2026-09-09 · v0.9.12 r1 已发布基线）

| 项 | 当前值 |
|---|---|
| 上游基线 | tag `v0.9.12`，commit `dcd4c200f72f0c1ffd60d8e7f6850313db879fc5` |
| 维护分支 | CodeWhale PR #44 与 fast-follow PR #46 将 `pinvou3-clean` 对齐到 `1fafee7e26b60a59457a43bce50c63aa2ad9dbaf` |
| 发布状态 | 公开 `pinvou3-clean`、不可变 tag `pinvou-v0.9.12-r1` 与父仓 gitlink 均指向同一 head |
| 升级前回退点 | 公开不可变 tag `pinvou-v0.9.5-r13` → `f853f8f1566c57e6be40d5439a222a932aa79ef5`；同 SHA 的本地 `backup/pre-v0.9.12-sync` 仅作便利引用 |
| 历史组织 | 上游之上 15 个带 DCO sign-off 的提交，仍归属 4 个长期主题；最后十个提交收口评审确认的行为、测试、文档与精确 SHA 发布门禁缺口 |
| drift | `94 files, +5022/-944`，净增 4078 行；旧 r13 为 `110 files, +10895/-1195` |
| 守护 | 37 条独立 CodeWhale `forkguard_*` 行为测试（31 条默认 + 6 条 `benchmark-eval-controls`）+ 父仓指纹与行为测试 |
| 父仓适配 | v0.9.12 EngineConfig、Agent/Plan 模式、逐轮 reasoning/安全、ExtraTools、owner 事件隔离、Automation v3/v4 数据兼容、rusqlite 0.40.2 |

### 多根工作区 workspace_roots + 指令 source 相对化（2026-09-11，本分支未推送）

- CodeWhale 分支 `pinvou3/workspace-roots-v12`（v0.9.12 r1 之上 9 个提交，head `a78c223de`，自 v0.9.5 线的 `pinvou3/workspace-roots` @d9431a28f 逐提交移植，层次 1:1）：线程从单根 `cwd` 扩展为 **cwd（主根）+ workspace_roots（全量可访问根集合）**，是"单入口工作区"（项目 = 主文件夹 + 一组钥匙）的底座前提。四层落地：
  1. **协议与会话模型**：`ThreadStartParams`/`ThreadResumeParams`/`ThreadForkParams` 与 `Thread` DTO 增加 `workspace_roots`（serde default，旧载荷读入为空）；SQLite `threads` 表 v5 迁移加 `workspace_roots TEXT`（缺省 `'[]'`，旧库零迁移退化）；TUI 两侧 JSON（`ThreadRecord`/`SessionMetadata`）additive serde default 字段。
  2. **回合环境**：根集合经 `Op::SyncSession` / Runtime API `UpdateThreadRequest` 运行中替换（活动回合拒绝 + 驱逐缓存引擎），每回合读取——快照语义，下回合生效；`normalize_workspace_roots` 保证 cwd 居首去重，空集合 ≡ `[cwd]`。resume 三态：带 roots 整体替换；只带 cwd 替换主根槽位、附加根保留；都不带沿用持久化值（顺带修复 cwd 被 fallback 覆盖的缺陷）。v12 适配：`SyncSession` wire 镜像、exec_agent 装配、acp_server 接缝均已接线。
  3. **权限沙箱**：`:workspace_roots` 符号常量（`WORKSPACE_ROOTS_SYMBOL`）在每回合策略构造点物化（注：暂无配置面消费该字面值）；`workspace_write_policy(workspace, roots, network_access)` 的 writable_roots = 归一化全集合（空集合逐字节等于旧值 `[workspace]`）；写豁免 carve-out 逐根判定（排除名仍拒）；`ToolContext::resolve_path` 跨根放行、真越界仍 `PathEscape`；execpolicy 规则跨根匹配。v12 的 `SandboxNetworkAccess` 类型化参数与 fork 反转点全部保留。
  4. **提示词/项目指令**：AGENTS.md 发现仅主根（v12 的仓库边界回溯语义保留），附加根只给访问权不注入指令；`<project_instructions source="…">` 标签由绝对路径改为仅文件名（统一 helper `project_instructions_source_label`，位于 `project_context/types.rs`），目录搬移/换主根且指令内容相同者不再击破 KV 前缀缓存。v12 差异：compaction 逐字重注入点已在上游重构中消失，旧线对应测试未移植。
- 行为变化边界：未配置多根（空集合）时协议帧、策略值、路径判定逐字节等价单根现状；TUI 交互端无多根 UI，根集合只能经 Runtime API/headless 进入；fork 暂不继承父线程附加根。
- drift：v0.9.12 r1 → 本主题为 `40 files, +1635/-142` 的移植等价物；新增 6 条 forkguard（5 条 `forkguard_workspace_roots_*` + 1 条 source 相对化），默认组 31→37 条运行通过。
- 指纹锚点：`pub workspace_roots: Vec<PathBuf>,`（protocol）、`pub fn normalize_workspace_roots(`（core）、`ADD COLUMN workspace_roots TEXT NOT NULL DEFAULT '[]';`（state）、`WORKSPACE_ROOTS_SYMBOL: &str = ":workspace_roots"`（sandbox/policy）、`fn project_instructions_source_label(`（project_context/types.rs）及 6 条 `fn forkguard_*` 测试名。

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
| `54819b0d6` | 发布门禁 fast-follow | 为手动精确 SHA CI 补齐 migration manifest 比较基线；修正 rustdoc 私有链接与过时步数说明；将 18 个仅供下游宿主兼容的宽 facade 从生成 API 文档中隐藏，不改变编译 API 或运行行为 |
| `ff9959bfc` | 发布门禁跟进 | 删除已由当前阻断结果式回归替代、且产品语义已明确反转的上游 Full Access 比较 helper；不抬高 dead-code 预算 |
| `6615af7ca` | 文档复审收口 | 删除 4 处指向不存在的 fork-policy 小节引用，直接说明测试所锁定的产品反转与显式宿主边界 |
| `409138dbe` | 运行时契约发布门禁 | 精确登记官方 v0.9.12 `automation`/`tasks` 路由字段与 Agent 精简的净增长，以及 Pinvou r1 写入上限和 host prompt-only profile 的模型可见增长；只抬高 Act/Operate full 的真实上限，同时收紧 active 与 Plan full，工具 identity 不变 |
| `881cf4444` | 精确 SHA CI 收口 | macOS npm wrapper 在可选 sccache 丢失后的冷启动 release build 可使用 60 分钟上限，并以 wiring 回归锁定例外只作用于该 job |
| `baa87f4de` | T4 复审修复 | offline-misfire skip 只作用于 recurring，使过期一次性任务仍精确持久入队一次后暂停 |
| `1fafee7e2` | T2 复审修复 | 所有搜索后端不可用时恢复可操作且脱敏的 provider/config 配置提示 |

所有提交都含 DCO `Signed-off-by`。`b4c02616b` 包含大部分跨主题收口，历史粒度确实不利于 bisect；分支公开进入评审后没有为历史美化 force-push，而是追加带 sign-off 的提交修复评审和发布门禁问题，并用本表、指纹和行为测试弥补审计粒度。不可变 tag 已创建，后续不得重写。

## 4. T1 — 宿主嵌入与路由边界

### 保留内容

- 对宿主公开 `AppMode`、`ApprovalMode`、Automation、Task、route 和 worker ledger API。当前兼容 facade 还包含 18 个 `pub mod`，父仓全部 Rust 源码中有 61 个文件、340 处直接引用；这些模块作为不稳定下游兼容桥刻意不进入生成 API 文档。这是已登记的收窄债务，不在本次热修中做破坏性改名。
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

## 8. 父仓适配边界

- `pinvou3-app` 负责产品工具白名单、AppMode 到 approval/trust 的映射、reasoning effort、会话 owner 过滤和定时会话创建。
- bridge 保留 v0.9.12 的有限轮次/工具预算、read denylist、bubblewrap、MCP OAuth、goal loop 与 telemetry 安全默认值。
- `session_id` 必须在 `Engine::spawn` 前进入 `EngineConfig`；不得事后依赖事件猜归属。
- 旧的全局 disabled-skills 调用已删除；包开关通过显式 bundle/registry 和每会话 disallowed tools 生效。

## 9. 软上限评估与后续减量

当前净增 4078 行，超过 1500 行软上限；`engine.rs`、`turn_loop.rs` 和 `engine/tests.rs` 也超过单文件 200 行提示线。原因是状态机、最终分发安全、“完整专家人设仅进入被选子智能体”的 spawn 边界、bundle Skills 排除 ambient 文件源的权限边界，以及评审要求的结果式生命周期/评测回归必须和 v0.9.12 原生 Engine 同步，拆成 app 侧镜像会形成更危险的双状态源。当前 Rust/Clippy/rustdoc 兼容只包含等价重写、`Default` 补全、窄 lint 说明、文档可达性修复和无调用测试 helper 清理，不改变公开函数签名或新增运行语义。

后续减量顺序：

1. 将可靠 steer lifecycle 与逐轮 exact/read-only dispatch 作为通用 host API 上游化。
2. 将 Automation misfire/no-overlap/conversation ownership 上游化。
3. 将 `FragmentId::Permissions` 的显式 100 KiB 宿主预算做成通用的可配置 fragment limit，上游化后删除固定例外。
4. 父仓先迁移到明确 re-export API，再分批收窄当前 18 个 `pub mod` 兼容 facade。
5. 上游提供完整 static-composer/explicit-skills-root 契约后删除 T3 patch。
6. 每次上游 release 重新核对已吸收项，不保留兼容壳。

## 10. 发布与回退

- 公开回退点是不可变 tag `pinvou-v0.9.5-r13`；本地 `backup/pre-v0.9.12-sync` 不是发布前提。
- `pinvou3-clean` 与不可变 `pinvou-v0.9.12-r1` 已发布到 `1fafee7e26b60a59457a43bce50c63aa2ad9dbaf`；父仓 gitlink 对齐同一 SHA 后以 `scripts/verify-public-submodule.sh` 验证公开可达性。
- 发布过程中只为精确 head 的受保护分支更新临时移除无法在该维护分支触发的 required status contexts，完成快进后立即恢复原保护配置；未关闭 force-push 防护，也未重写已发布 tag。
- 后续发布仍不得降低公开校验或把本地 object 当成发布成功。
