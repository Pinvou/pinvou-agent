#!/usr/bin/env bash
# CodeWhale v0.9.5 clean re-fork guard: published four-theme baseline.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TUI="$REPO/CodeWhale"
APP="$REPO/pinvou3-app/src-tauri"
EXPECTED_UPSTREAM="853cb707bbcf4f7dc4268fba6d811e0d04083f9c"
EXPECTED_HEAD="5a5bf363ebeb720410f30e400c3de44abab71de6"
EXPECTED_COMMITS=38
FAST_ONLY=0
[[ "${1:-}" == "--fast" ]] && FAST_ONLY=1

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
bold()  { printf '\033[1m%s\033[0m\n' "$*"; }

fail=0

bold "── 第 0 层：v0.9.5 r14 候选四主题基线拓扑 ──"
actual_head="$(git -C "$TUI" rev-parse HEAD 2>/dev/null || true)"
if [[ "$actual_head" == "$EXPECTED_HEAD" ]]; then
  green "  ✓ CodeWhale gitlink 指向登记的 r14 未发布候选 $EXPECTED_HEAD"
else
  red "  ✗ CodeWhale HEAD 为 ${actual_head:-<unreadable>}，应为 r14 候选 $EXPECTED_HEAD"
  fail=1
fi

if git -C "$TUI" merge-base --is-ancestor "$EXPECTED_UPSTREAM" HEAD 2>/dev/null; then
  green "  ✓ 当前公开 gitlink 继承官方 v0.9.5"
else
  red "  ✗ 当前 gitlink 未继承官方 v0.9.5 $EXPECTED_UPSTREAM"
  fail=1
fi

commit_count="$(git -C "$TUI" rev-list --count "$EXPECTED_UPSTREAM..HEAD" 2>/dev/null || true)"
if [[ "$commit_count" == "$EXPECTED_COMMITS" ]]; then
  green "  ✓ v0.9.5 之上 $EXPECTED_COMMITS 个登记提交"
else
  red "  ✗ v0.9.5 之上有 ${commit_count:-<unreadable>} 个 commit，登记拓扑应为 $EXPECTED_COMMITS"
  fail=1
fi

bold "── 第 1 层：四主题与父仓指纹 ──"
# 格式：主题|说明|文件（相对父仓根）|grep -F 固定串
fingerprints=(
  "T1|v0.9.5 library 只公开宿主入口       |CodeWhale/crates/tui/src/lib.rs|pub mod automation_manager;"
  "T1|宿主可重载 Fleet roster             |CodeWhale/crates/tui/src/lib.rs|pub use fleet::roster::FleetRoster;"
  "T1|Fleet roster 宿主入口回归           |CodeWhale/crates/tui/src/lib.rs|fn forkguard_host_can_load_workspace_fleet_roster"
  "T1|宿主只读 live worker 投影          |CodeWhale/crates/tui/src/tools/subagent/mod.rs|pub fn read_persisted_agent_worker_records("
  "T1|只读 worker 不触发重启回收回归      |CodeWhale/crates/tui/src/tools/subagent/tests.rs|fn forkguard_host_readonly_worker_projection_preserves_live_status"
  "T1|宿主显式 route limits               |CodeWhale/crates/tui/src/route_runtime.rs|pub fn resolve_runtime_route_with_limits("
  "T1|embedding route wire alias 回归      |CodeWhale/crates/tui/src/route_runtime.rs|fn forkguard_embedding_route_limits_preserve_wire_alias"
  "T1|运行时会话快照不推断工具崩溃        |CodeWhale/crates/tui/src/session_manager.rs|fn forkguard_runtime_session_snapshot_preserves_in_flight_tool_call"
  "T1|显式重启恢复可观测且幂等            |CodeWhale/crates/tui/src/session_manager.rs|fn forkguard_explicit_session_recovery_is_reported_and_idempotent_after_save"
  "T1|宿主批量取消运行中子智能体          |CodeWhale/crates/tui/src/core/ops.rs|CancelSubAgents"
  "T1|批量取消幂等行为回归                |CodeWhale/crates/tui/src/tools/subagent/tests.rs|fn forkguard_host_bulk_cancel_stops_all_running_children_idempotently"
  "T1|通用完成事件携带失败终态            |CodeWhale/crates/tui/src/core/events.rs|failed: bool"
  "T1|可靠插入返回可关联的 steer id       |CodeWhale/crates/tui/src/core/engine/handle.rs|pub async fn steer(&self, content: impl Into<String>) -> Result<String>"
  "T1|steer 消息 opaque id 契约           |CodeWhale/crates/tui/src/core/ops.rs|pub struct SteerMessage"
  "T1|steer 事件携带 steer_id 关联         |CodeWhale/crates/tui/src/core/events.rs|steer_id: String"
  "T1|cancel 原子发布 steer 处置入口       |CodeWhale/crates/tui/src/core/engine/handle.rs|pub fn cancel_with_mode"
  "T1|打断保留未提交 steer 回归           |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn steer_lifecycle_interrupt_keeps_input_for_next_turn"
  "T1|停止丢弃未提交 steer 回归           |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn steer_lifecycle_stop_retires_reserved_late_send_once"
  "T1|换会话拒绝迟到 steer 回归           |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn steer_lifecycle_session_rejects_late_reserved_send"
  "T1|引擎回收 steer 不静默悬挂回归       |CodeWhale/crates/tui/src/core/engine/tests.rs|fn engine_drop_reports_unconsumed_steers_best_effort"
  "T1|steer 撤回宿主入口                  |CodeWhale/crates/tui/src/core/engine/handle.rs|pub fn withdraw_steer"
  "T1|撤回返回明确 outcome 契约           |CodeWhale/crates/tui/src/core/engine/handle.rs|pub fn withdraw_steer(&self, steer_id: &str) -> crate::core::engine::SteerWithdrawal"
  "T1|撤回 outcome 枚举定义               |CodeWhale/crates/tui/src/core/engine.rs|pub enum SteerWithdrawal"
  "T1|撤回有界且阻止注入回归             |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_steer_lifecycle_withdrawal_is_bounded_and_prevents_commit"
  "T1|committed 先到时撤回对账回归        |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_steer_lifecycle_late_withdraw_reconciles_committed_event"
  "T1|编辑目标分类排除工具结果和内部信封  |CodeWhale/crates/tui/src/runtime_handoff.rs|pub fn edit_last_turn_target"
  "T1|宿主复用权威编辑目标分类            |CodeWhale/crates/tui/src/lib.rs|edit_last_turn_target,"
  "T1|编辑上一轮截断在真实用户消息        |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_edit_last_turn_cuts_at_user_prompt_before_tool_results"
  "T1|无用户消息可编辑时报错不发送        |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_edit_last_turn_without_user_prompt_errors_and_sends_nothing"
  "T1|不支持的最新用户内容拒绝编辑        |CodeWhale/crates/tui/src/core/engine.rs|edit_last_turn_unsupported_user_content"
  "T1|kimi-for-coding 固定采样剥离        |CodeWhale/crates/tui/src/client/chat.rs|fn apply_kimi_code_coding_plan_fixed_sampling("
  "T1|kimi-for-coding 固定采样回归        |CodeWhale/crates/tui/src/client/chat.rs|fn forkguard_kimi_code_coding_plan_strips_non_one_temperature"
  "T1|deepseek-v4 Chat 文档采样契约回归   |CodeWhale/crates/tui/src/client/chat.rs|fn forkguard_deepseek_v4_chat_preserves_documented_temperature"
  "T1|deepseek-v4-flash Responses 采样 shim |CodeWhale/crates/tui/src/client/responses.rs|requires_default_temperature"
  "T1|deepseek-v4-flash Responses 采样回归 |CodeWhale/crates/tui/src/client/responses/tests.rs|fn forkguard_deepseek_v4_flash_responses_drops_non_one_temperature"
  "T1|压缩完成事件携带新上下文估算        |CodeWhale/crates/tui/src/core/events.rs|post_input_tokens: Option<u64>"
  "T1|压缩后估算覆盖完整请求输入          |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_compaction_completed_reports_complete_post_input_tokens"
  "T1|显式 route 输出上限参与请求预算      |CodeWhale/crates/tui/src/route_budget.rs|pub(crate) fn effective_max_output_tokens_for_route("
  "T1|steer 撤回结果具有明确状态          |CodeWhale/crates/tui/src/core/engine.rs|pub enum SteerWithdrawal"
  "T1|steer 撤回竞态边界回归              |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_steer_lifecycle_withdrawal_is_bounded_and_prevents_commit"
  "T1|严格直连模型大小写仅限明确自有行    |CodeWhale/crates/config/src/route/resolver.rs|let allow_casefold_wire_match = class == ProviderClass::StrictDirect"
  "T1|严格直连大小写回退行为回归          |CodeWhale/crates/config/src/route/tests.rs|fn resolver_direct_owned_row_match_survives_casing_mismatch"

  "T2|宿主额外工具入口                    |CodeWhale/crates/tui/src/core/engine.rs|pub struct ExtraTools("
  "T2|动态禁用工具操作                    |CodeWhale/crates/tui/src/core/ops.rs|SetDisallowedTools { tools: Vec<String> }"
  "T2|宿主工具覆盖全部运行模式            |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_host_extra_tools_register_in_all_modes"
  "T2|File 写入 64 KiB 上限               |CodeWhale/crates/tui/src/tools/file.rs|const WRITE_FILE_MAX_CONTENT_BYTES: usize = 64 * 1024;"
  "T2|写入上限落盘前拒绝回归              |CodeWhale/crates/tui/src/tools/file/tests/tools.rs|async fn forkguard_file_content_caps_reject_before_writing"
  "T2|多行危险命令分段阻断回归            |CodeWhale/crates/tui/src/command_safety.rs|fn forkguard_multiline_still_blocks_destructive_segments"
  "T2|schema 约束 JSON 容器修复           |CodeWhale/crates/tui/src/core/engine/dispatch.rs|pub(super) fn normalize_schema_json_containers("
  "T2|嵌套容器修复保持 primitive 不变     |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_schema_bound_json_container_repair_accepts_nested_payload"
  "T2|容器修复拒绝越限与类型不匹配        |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_schema_bound_json_container_repair_rejects_wrong_or_unbounded_values"
  "T2|stuck 告警留在 tool result          |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_stuck_guard_warning_is_embedded_in_tool_result_content"
  "T2|stuck 续轮保持 provider 角色合法    |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_stuck_guard_tool_warning_preserves_provider_role_sequence"
  "T2|错误降级提示保持 provider 角色合法  |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_tool_error_degradation_preserves_provider_role_sequence"
  "T2|Registry 提示使用 canonical 工具面 |CodeWhale/crates/tui/src/core/engine/tests.rs|fn registry_first_policy_is_in_the_initial_prompt_only_when_mcp_is_enabled"
  "T2|旧 action alias 解析为 canonical   |CodeWhale/crates/tui/src/tools/subagent/tests.rs|fn custom_child_allowlist_omitting_load_skill_fails_closed"
  "T2|Moonshot 工具降级产生用户可见诊断   |CodeWhale/crates/tui/src/core/events.rs|pub fn tool_projection_warning_message("
  "T2|具名 tool_choice 不得指向省略工具  |CodeWhale/crates/tui/src/client.rs|async fn forkguard_moonshot_rejects_named_choice_for_omitted_tool"
  "T2|Moonshot 每轮只发一次投影诊断       |CodeWhale/crates/tui/src/client.rs|async fn forkguard_moonshot_stream_emits_one_projection_warning"
  "T2|宿主 MCP 密钥解析不写进程环境       |CodeWhale/crates/tui/src/mcp.rs|pub fn install_mcp_secret_resolver("
  "T2|禁用 MCP server 从全部 pool 面消失 |CodeWhale/crates/tui/src/mcp/tests.rs|fn forkguard_mcp_pool_denied_server_disappears_from_every_surface"
  "T2|子智能体不得绕过 MCP 禁用继承       |CodeWhale/crates/tui/src/tools/subagent/tests.rs|fn forkguard_spawn_request_inherit_disallowed_tools_opt_out_not_honored"
  "T2|Shell 跨 poll 保持 UTF-8 解码状态  |CodeWhale/crates/tui/src/tools/shell/output.rs|fn forkguard_shell_output_decoder_preserves_utf8_across_poll_boundaries"
  "T2|API 后端链尾兜底为 Bing            |CodeWhale/crates/tui/src/tools/web/backend.rs|fn forkguard_api_provider_chain_tail_is_bing"
  "T2|厂商原生搜索精确路由门控          |CodeWhale/crates/config/src/route/capabilities.rs|documented_server_side_web_search_for_route"
  "T2|Kimi K3 Formula 搜索适配          |CodeWhale/crates/tui/src/client/provider_native_search/kimi.rs|WEB_SEARCH_FORMULA_URI"
  "T2|全链失败建议配置 API 搜索后端      |CodeWhale/crates/tui/src/tools/web/backend.rs|configure an API-backed [search] provider"
  "T2|cancel 只杀本轮前台 shell          |CodeWhale/crates/tui/src/tools/shell.rs|fn kill_running_turn_foreground"
  "T2|前台范围 kill 不误杀后台回归        |CodeWhale/crates/tui/src/tools/shell/tests.rs|fn kill_running_turn_foreground_scopes_to_this_turns_unowned_foreground_shells"
  "T2|Shell job 保留稳定来源身份            |CodeWhale/crates/tui/src/tools/shell.rs|pub origin_tool_call_id: Option<String>"
  "T2|Engine 分发盖章来源工具调用         |CodeWhale/crates/tui/src/core/engine/turn_loop.rs|fn tool_context_for_call("
  "T2|Shell 来源身份行为回归              |CodeWhale/crates/tui/src/tools/shell/tests.rs|fn forkguard_background_shell_job_preserves_origin_identity"
  "T2|Engine 来源身份行为回归             |CodeWhale/crates/tui/src/core/engine/turn_loop.rs|fn forkguard_tool_context_for_call_preserves_turn_and_sets_call_origin"
  "T3|ambient project authority 密封       |CodeWhale/crates/tui/src/project_context.rs|fn forkguard_runtime_loader_ignores_ambient_project_authority"
  "T3|Permissions 100 KiB 窄例外回归      |CodeWhale/crates/tui/src/prompts.rs|fn forkguard_instruction_fragment_preserves_content_beyond_default_cap"
  "T3|disabled Skill 不可见且不可加载      |CodeWhale/crates/tui/src/skills/tests.rs|fn forkguard_disabled_skill_is_neither_rendered_nor_loadable"
  "T3|内部 reminder 不污染 Working Set    |CodeWhale/crates/tui/src/working_set.rs|fn forkguard_working_set_ignores_leading_system_reminder_paths"

  "T4|Automation 使用稳定 conversation key|CodeWhale/crates/tui/src/automation_manager.rs|add_task_with_conversation_key(new_task, Some(automation.id.clone()))"
  "T4|离线漏跑不补跑                      |CodeWhale/crates/tui/src/automation_manager.rs|fn forkguard_scheduler_skips_offline_misfires_without_backfill"
  "T4|同一 Automation 不重叠              |CodeWhale/crates/tui/src/automation_manager.rs|fn forkguard_scheduler_does_not_overlap_active_automation_run"
  "T4|Pinvou 历史 v3/v4 schema 窄兼容     |CodeWhale/crates/tui/src/task_manager.rs|const PINVOU_LEGACY_TASK_SCHEMA_VERSIONS"
  "T4|conversation/thread 跨 worker 持久化|CodeWhale/crates/tui/src/task_manager.rs|async fn forkguard_conversation_key_and_created_thread_survive_worker_boundary"

  "T2|会话 trusted roots 可完全覆盖       |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_session_trusted_roots_override_persisted_workspace_trust"
  "T2|执行分发白名单 fail-closed          |CodeWhale/crates/tui/src/core/engine/tool_execution.rs|fn forkguard_dispatch_allowlist_rejects_forged_calls_before_all_dispatch_backends"
  "T2|排队控制操作继承受限权限             |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_queued_control_op_keeps_restricted_turn_authority"
  "T2|受限续轮、编辑与 MCP reload 拒绝     |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_queued_goal_continuation_and_mcp_reload_keeps_restricted_turn_authority"
  "T2|受限轮次延迟空闲子代理续轮           |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_restricted_turn_defers_idle_subagent_completion_until_new_message"
  "T2|受限轮次延迟后台 Shell 唤醒          |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_restricted_turn_defers_idle_shell_wake_until_new_message"
  "T2|受限 Bash 使用只读 Shell 加固        |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_restricted_agent_uses_hardened_read_only_shell_context"
  "T2|受限轮次 Hook 默认关闭              |CodeWhale/crates/tui/src/core/ops.rs|fn forkguard_restricted_turn_hooks_require_explicit_host_opt_in"
  "T2|受限工具审计固定脱敏                 |CodeWhale/crates/tui/src/core/engine/tool_execution.rs|fn forkguard_restricted_tool_audit_redacts_private_sentinel"
  "T2|受限轮次 File schema 只读           |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_restricted_agent_uses_read_only_file_schema"
  "T2|只读动作最终分发 fail-closed        |CodeWhale/crates/tui/src/core/engine/tool_execution.rs|fn forkguard_read_only_turn_rejects_write_action_at_final_dispatch"
  "T2|受限轮后子智能体完成等待新消息      |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_restricted_turn_defers_idle_subagent_completion_until_new_message"
  "T2|只读轮次启用 Shell 加固上下文       |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_restricted_agent_uses_hardened_read_only_shell_context"
  "T2|受限轮后 Shell 唤醒等待新消息       |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_restricted_turn_defers_idle_shell_wake_until_new_message"
  "APP|产品白名单复用原生 allowed_tools   |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|allowed_tools: Some(crate::features::assistant::tool_policy::allowed_tool_names())"
  "APP|会话工具开关走动态禁用整形          |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|pub fn shape_disallowed_tools("
  "APP|v0.9.5 subagent state root 透传     |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|cfg.subagent_state_root = Some(roots.ledger);"
  "APP|停止与回收级联取消子智能体          |pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs|Op::CancelSubAgents"
  "APP|resolved route 由宿主统一解析        |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|pub fn resolve_runtime_route_for_model("
  "APP|GLM 小写存量配置解析到规范直连模型  |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|fn forkguard_zai_direct_route_survives_model_casing_mismatch"
  "APP|128K/256K compaction 合约            |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|fn forkguard_compaction_128k_scenarios"
  "APP|定时任务复用 shared run API          |pinvou3-app/src-tauri/src/features/scheduled/tasks.rs|run_now_shared(&self.automations"
  "APP|多智能体面板只读 live worker         |pinvou3-app/src-tauri/src/features/multiagent/transcripts.rs|read_persisted_agent_worker_records(workspace)"
  "APP|静态 prompt composer 由 app 安装     |pinvou3-app/src-tauri/src/features/runtime_bundle/platform/mod.rs|set_static_prompt_composer_override"
  "APP|运行时会话读取不修复在途工具调用      |pinvou3-app/src-tauri/src/features/sessions/tests.rs|fn forkguard_runtime_snapshot_load_does_not_repair_in_flight_tool_call"
  "APP|进程启动显式恢复中断工具调用且幂等    |pinvou3-app/src-tauri/src/features/sessions/tests.rs|fn forkguard_boot_repairs_interrupted_tool_call_once"
  "APP|仅进程启动入口触发工具历史恢复        |pinvou3-app/src-tauri/src/lib.rs|SessionStore::boot_for_process_startup()"
  "APP|MCP secret 经底座 resolver 钩子下发   |pinvou3-app/src-tauri/src/features/marketplace/mod.rs|pub fn install_mcp_secret_resolver()"
  "APP|进程 env 写收口到单线程启动窗口       |pinvou3-app/src-tauri/src/lib.rs|pub(crate) fn startup_process_env()"
  "APP|工具卡隐藏已知内部 runtime suffix    |pinvou3-app/src/platform/tauri/bridge.js|function stripInternalToolRuntimeSuffix("
  "APP|落盘兜底编辑截断与底座同口径        |pinvou3-app/src-tauri/src/features/sessions/tests.rs|fn forkguard_admitted_display_fallback_edit_cuts_before_trailing_tool_result"
  "APP|不支持的最新用户内容不可回退到旧轮  |pinvou3-app/src-tauri/src/features/sessions/tests.rs|fn forkguard_admitted_display_fallback_does_not_skip_unsupported_user_turn"
  "APP|拒绝编辑终态触发权威历史回滚        |pinvou3-app/src-tauri/src/features/assistant/engine.rs|\"operation_rejected\": operation_rejected"
  "APP|Shell 快照按来源工具卡原位回写      |pinvou3-app/src/platform/tauri/bridge/terminal.js|it.toolId === job.origin_tool_call_id"
  "APP|Shell 监控按来源区分同命令任务       |pinvou3-app/src-tauri/src/features/assistant/shell_output.rs|fn forkguard_shell_monitor_assigns_identical_commands_by_stable_origin"
  "APP|历史 Shell 终态不追加到当前时间线   |pinvou3-app/tests/shell_task_projection.test.mjs|completed historical shell jobs are not inserted into the current timeline"
)

# r13 同时包含 r12 搜索边界与正式发布的 GAIA 评测隔离扩展。
fingerprints+=(
  "T2|API 后端链尾兜底为 Bing            |CodeWhale/crates/tui/src/tools/web/backend.rs|fn forkguard_api_provider_chain_tail_is_bing"
  "T2|厂商原生搜索精确路由门控          |CodeWhale/crates/config/src/route/capabilities.rs|documented_server_side_web_search_for_route"
  "T2|Kimi K3 Formula 搜索适配          |CodeWhale/crates/tui/src/client/provider_native_search/kimi.rs|WEB_SEARCH_FORMULA_URI"
  "T2|全链失败建议配置 API 搜索后端      |CodeWhale/crates/tui/src/tools/web/backend.rs|configure an API-backed [search] provider"
  "T2|评测控制默认关闭且显式启用          |CodeWhale/crates/tui/src/core/ops.rs|fn forkguard_benchmark_controls_are_explicit_and_default_off"
  "T2|评测只修复无歧义只读调用            |CodeWhale/crates/tui/src/core/engine/turn_loop.rs|fn forkguard_benchmark_repairs_only_unambiguous_read_actions"
  "T2|评测兼容修复受行为测试约束          |CodeWhale/crates/tui/src/core/engine/turn_loop.rs|fn forkguard_benchmark_repairs_read_schema_and_attachments"
  "T2|评测工具预算截断受轮次测试约束      |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_benchmark_budget_truncates_batch_and_clears_followup_tool_surface"
  "T2|评测 final-only 熔断有界            |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_benchmark_final_only_rejects_repeated_tool_only_responses"
  "T2|评测参数修复经真实轮次执行          |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_benchmark_turn_repairs_file_aliases_before_execution"
)

for fp in "${fingerprints[@]}"; do
  IFS='|' read -r theme desc file pat <<<"$fp"
  if grep -qF -- "$pat" "$REPO/$file" 2>/dev/null; then
    green "  ✓ ${theme} ${desc}"
  else
    red "  ✗ ${theme} ${desc} — 指纹消失于 $file"
    fail=1
  fi
done

if [[ $FAST_ONLY -eq 1 ]]; then
  echo
  [[ $fail -eq 0 ]] && green "指纹层全过 (--fast)" || red "指纹层有缺失"
  exit $fail
fi

echo
bold "── 第 2 层：CodeWhale forkguard 回归 ──"
( cd "$TUI" && cargo test -p codewhale-tui --lib --locked forkguard_ -- --test-threads=1 ) || fail=1
( cd "$TUI" && cargo test -p codewhale-tui --lib --locked --features benchmark-eval-controls forkguard_benchmark_ -- --test-threads=1 ) || fail=1

echo
bold "── 第 3 层：pinvou3-app forkguard 回归 ──"
( cd "$APP" && cargo test --lib --locked forkguard_ -- --test-threads=1 ) || fail=1
( cd "$APP" && cargo test --lib --locked --features benchmark-hooks eval_send_message_op_isolated_from_gui_authority_and_installs_exact_policy -- --test-threads=1 ) || fail=1
( cd "$APP" && cargo test --lib --locked --features benchmark-hooks features::assistant::product_runtime::headless_bridge::tests -- --test-threads=1 ) || fail=1
( cd "$APP" && cargo test --lib --locked --features benchmark-hooks features::assistant::product_runtime::agentic_task::tests -- --test-threads=1 ) || fail=1
( cd "$APP" && cargo test --lib --locked --features benchmark-hooks engine_config_tool_call_cap_respects_env_override -- --test-threads=1 ) || fail=1

echo
if [[ $fail -eq 0 ]]; then
  green "✅ fork-guard 全过：4 个 v0.9.5 fork 主题完好。"
else
  red "❌ fork-guard 失败：请对照 docs/fork-modifications.md 排查。"
fi
exit $fail
