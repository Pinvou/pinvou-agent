# CodeWhale Fork 修改清单

> 本文是 Pinvou 对 CodeWhale fork 的单一现状清单。
> 维护策略见 [`fork-policy.md`](fork-policy.md)，升级证据见 [`codewhale-upgrade-0.9.5-to-0.9.12.md`](codewhale-upgrade-0.9.5-to-0.9.12.md)。
> English: [`fork-modifications.en.md`](fork-modifications.en.md)

## 0. 当前状态（2026-09-11 · r1 基线 + 13 个登记提交，r2 收口未切 tag）

| 项 | 当前值 |
|---|---|
| 上游基线 | tag `v0.9.12`，commit `dcd4c200f72f0c1ffd60d8e7f6850313db879fc5` |
| 维护分支 | `pinvou3-clean` = `ae7e3fb36f89486f30d41b28ae0eaaa516ae4740`（r1 基线 `1fafee7e2` 之上 13 个 squash 合入：#41/#47/#49 与 2026-09-10/11 遗留 PR 清理批次 #31/#37/#38/#39/#43/#48/#50/#51/#52/#53） |
| 发布状态 | 过渡期（fork-policy 第 0 节豁免）：不可变 tag `pinvou-v0.9.12-r1` 保持 r1 收口状态（15 个提交），父仓 gitlink 指向 `ae7e3fb36`、领先 tag 13 个提交，直至下一次 r2 发布收口对齐 |
| 升级前回退点 | 公开不可变 tag `pinvou-v0.9.5-r13` → `f853f8f1566c57e6be40d5439a222a932aa79ef5`；同 SHA 的本地 `backup/pre-v0.9.12-sync` 仅作便利引用 |
| 历史组织 | 上游之上 28 个带 DCO sign-off 的提交，归属 4 个长期主题（T1–T4）+ 2 个追加减量主题（T5 会话归档导出、T6 蜂群限流治理）；r1 之后 13 个提交全部经 PR squash 合入并过五项必需门禁 |
| drift | `139 files, +9953/-1217`，净增 8736 行；r1 为 `94 files, +5022/-944`，旧 r13 为 `110 files, +10895/-1195` |
| 守护 | 54 条独立 CodeWhale `forkguard_*` 行为测试（48 条默认 + 6 条 `benchmark-eval-controls`）+ 父仓指纹与行为测试 |
| 父仓适配 | v0.9.12 EngineConfig、Agent/Plan 模式、逐轮 reasoning/安全、ExtraTools、owner 事件隔离、Automation v3/v4 数据兼容、rusqlite 0.40.2；消费方 PR #396（execpolicy）、#408（轮次取消）、#444（蜂群）、#468（computer-use）、#472（一键导出）依赖本批底座能力 |

## 1. 为什么本次使用 clean re-fork

旧 r13 相对 v0.9.5 修改 110 个文件。与 v0.9.12 对照时，104 个旧修改文件也被上游改动，直接三方移植预计产生 57 个冲突文件。与此同时，上游已经吸收或重构了大量旧 patch，包括会话快照/恢复、编辑上一轮、压缩后 token、provider/model 路由、原生搜索、Windows UTF-8 Shell、JSON schema 修复与任务基础设施。

因此本次从官方 `v0.9.12` tag 直接建立新分支，只重表达仍然缺失且必须位于底座生命周期内的语义；没有 merge 或 cherry-pick 旧 r13 冲突树。

## 2. 旧 r13 逐项处置

| 处置 | 能力 | v0.9.12 r1 结果 |
|---|---|---|
| 上游已有，删除 fork | 会话 snapshot/recovery、edit-last-turn、post-compaction tokens、route budget、严格直连模型大小写、JSON 容器修复、厂商原生搜索、Windows UTF-8、provider pin 等 | 采用上游实现和测试，不复制旧代码 |
| 语义迁移 | `Yolo`/`Auto` 模式、全局 reasoning、旧 custom-tools/disabled-skills 接口 | 映射到 `Agent` + approval/trust、逐轮 `Op::SendMessage` reasoning、`ExtraTools` 与显式 Skills 根 |
| 继续保留 | 宿主 facade/route limits、可靠 steer 与批量取消、MCP secret resolver、raw worker ledger、逐轮最终分发安全、64 KiB File 上限、prompt ownership、Automation conversation/schema/lifecycle | 重写到 v0.9.12 当前 Engine/Task/Prompt 结构并增加结果式测试 |
| 继续保留但默认关闭 | r13 benchmark eval controls | 在 v0.9.12 架构上恢复无歧义 missing-read-action repair；仅由 benchmark feature 显式启用，桌面默认路径关闭。final-only-after-tool-budget 自 2026-09-11 起宿主不再 arm（工具调用轮数上限已整体移除，budget 永不耗尽，arm 是死代码），底座侧机制与其 forkguard 测试保留，作为未来上游化/删除决策的候选 |
| 继续保留 | #35 API 搜索链直接以 Bing 收尾的覆盖 | 配置型 API backend 失败后直接落到免密 Bing。DuckDuckGo 的内部 Bing fallback 只覆盖空结果/challenge，不覆盖连接失败；在 DuckDuckGo 不可达的网络中把它作为外层尾部会提前返回错误，因此恢复直接 Bing 尾部并以结果式测试锁定全部 API provider |
| 采用上游删除 | stuck/read-repeat/coaching guards | 接受上游 `b39cf5650` 的处置：旧 stuck 指纹不含结果摘要，会把活跃 job 的重复 poll 误判为无进展；不恢复旧 guard 或相关环境开关，继续依靠有限 `max_steps`、每轮墙钟和取消边界（宿主自 2026-09-11 起不设工具调用轮数预算） |

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
- bridge 保留 v0.9.12 的 read denylist、bubblewrap、MCP OAuth、goal loop 与 telemetry 安全默认值；产品构建不再叠加基准专用的每轮工具上限（见 §2 的 2026-09-11 行）。
- `session_id` 必须在 `Engine::spawn` 前进入 `EngineConfig`；不得事后依赖事件猜归属。
- 旧的全局 disabled-skills 调用已删除；包开关通过显式 bundle/registry 和每会话 disallowed tools 生效。

## 11. 软上限评估与后续减量

当前净增 8736 行，超过 1500 行软上限；`engine.rs`、`turn_loop.rs` 和 `engine/tests.rs` 也超过单文件 200 行提示线。原因是状态机、最终分发安全、“完整专家人设仅进入被选子智能体”的 spawn 边界、bundle Skills 排除 ambient 文件源的权限边界，以及评审要求的结果式生命周期/评测回归必须和 v0.9.12 原生 Engine 同步，拆成 app 侧镜像会形成更危险的双状态源。当前 Rust/Clippy/rustdoc 兼容只包含等价重写、`Default` 补全、窄 lint 说明、文档可达性修复和无调用测试 helper 清理，不改变公开函数签名或新增运行语义。

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
- 不可变 `pinvou-v0.9.12-r1` 停在 r1 收口 `1fafee7e26b60a59457a43bce50c63aa2ad9dbaf`；当前处于过渡期：`pinvou3-clean` 与父仓 gitlink 指向 `ae7e3fb36`（领先 tag 13 个 squash 提交），下一次 r2 发布收口时在合并头切不可变 tag 并对齐三方（分支/tag/gitlink），以 `scripts/verify-public-submodule.sh` 验证公开可达性（过渡期断言 gitlink=分支头、tag 钉在 r1 收口，r2 收口后恢复三方相等；依据 `docs/fork-policy.md` 第 0 节过渡期豁免）。
- 发布过程中只为精确 head 的受保护分支更新临时移除无法在该维护分支触发的 required status contexts，完成快进后立即恢复原保护配置；未关闭 force-push 防护，也未重写已发布 tag。
- 后续发布仍不得降低公开校验或把本地 object 当成发布成功。
