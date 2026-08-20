#!/usr/bin/env bash
# CodeWhale v0.9.5 clean re-fork guard: PinvouOS feature checkpoint on r7.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TUI="$REPO/CodeWhale"
APP="$REPO/pinvou3-app/src-tauri"
EXPECTED_UPSTREAM="853cb707bbcf4f7dc4268fba6d811e0d04083f9c"
PUBLISHED_HEAD="a36e6cd533024cfe5724bae21875aea42b2ed87a"
EXPECTED_HEAD="2f1f851ed038ffa161b42404bf478b1d9d4aeff2"
EXPECTED_COMMITS=11
FAST_ONLY=0
[[ "${1:-}" == "--fast" ]] && FAST_ONLY=1

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
bold()  { printf '\033[1m%s\033[0m\n' "$*"; }

fail=0

bold "── 第 0 层：v0.9.5 r7 + PinvouOS feature checkpoint 拓扑 ──"
actual_head="$(git -C "$TUI" rev-parse HEAD 2>/dev/null || true)"
if [[ "$actual_head" == "$EXPECTED_HEAD" ]]; then
  green "  ✓ CodeWhale 工作树 HEAD 指向登记的 PinvouOS feature checkpoint $EXPECTED_HEAD"
else
  red "  ✗ CodeWhale HEAD 为 ${actual_head:-<unreadable>}，feature checkpoint 登记为 $EXPECTED_HEAD"
  fail=1
fi

index_gitlink="$(git -C "$REPO" ls-files --stage -- CodeWhale 2>/dev/null | awk '$1 == "160000" { print $2; exit }')"
if [[ "$index_gitlink" == "$EXPECTED_HEAD" ]]; then
  green "  ✓ 父仓索引 gitlink 精确固定同一 checkpoint"
else
  red "  ✗ 父仓索引 gitlink 为 ${index_gitlink:-<unreadable>}，checkpoint 登记为 $EXPECTED_HEAD"
  fail=1
fi

if [[ -n "$(git -C "$TUI" status --porcelain 2>/dev/null)" ]]; then
  red "  ✗ CodeWhale 工作树含未提交修改，HEAD/gitlink 不能证明当前内容"
  fail=1
else
  green "  ✓ CodeWhale 工作树干净，checkpoint 可由 HEAD 精确复现"
fi

if git -C "$TUI" merge-base --is-ancestor "$EXPECTED_UPSTREAM" HEAD 2>/dev/null \
  && git -C "$TUI" merge-base --is-ancestor "$PUBLISHED_HEAD" HEAD 2>/dev/null; then
  green "  ✓ feature checkpoint 继承官方 v0.9.5 与 r7 公开维护 head"
else
  red "  ✗ feature checkpoint 未同时继承官方 v0.9.5 与 r7 公开维护 head $PUBLISHED_HEAD"
  fail=1
fi

commit_count="$(git -C "$TUI" rev-list --count "$EXPECTED_UPSTREAM..HEAD" 2>/dev/null || true)"
if [[ "$commit_count" == "$EXPECTED_COMMITS" ]]; then
  green "  ✓ v0.9.5 之上 $EXPECTED_COMMITS 个维护/feature 提交"
else
  red "  ✗ v0.9.5 之上有 ${commit_count:-<unreadable>} 个 commit，feature checkpoint 登记为 $EXPECTED_COMMITS"
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
  "T1|后台完成交付策略默认保持 Eager      |CodeWhale/crates/tui/src/core/engine.rs|pub enum SubAgentCompletionDeliveryPolicy"
  "T1|TurnStarted 携带 typed provenance   |CodeWhale/crates/tui/src/core/events.rs|provenance: UserInputProvenance"
  "T1|宿主可临时保持后台完成              |CodeWhale/crates/tui/src/core/ops.rs|HoldSubAgentCompletions { holder_id: String }"
  "T1|宿主按 opaque id 精确释放保持       |CodeWhale/crates/tui/src/core/ops.rs|ReleaseSubAgentCompletions { holder_id: String }"
  "T1|两阶段 barrier 先申请再确认          |CodeWhale/crates/tui/src/core/ops.rs|AcquireSubAgentCompletionHold {"
  "T1|两阶段 barrier 确认不复活过期 holder |CodeWhale/crates/tui/src/core/ops.rs|ConfirmSubAgentCompletionHold {"
  "T1|forwarder 水位事件 Applied          |CodeWhale/crates/tui/src/core/events.rs|SubAgentCompletionHoldApplied {"
  "T1|forwarder 水位事件 Confirmed        |CodeWhale/crates/tui/src/core/events.rs|SubAgentCompletionHoldConfirmed {"
  "T1|后台完成保持仅在 idle 计 30 秒      |CodeWhale/crates/tui/src/core/engine.rs|const SUBAGENT_COMPLETION_HOLD_IDLE_TIMEOUT: Duration = Duration::from_secs(30);"
  "T1|后台完成 holder id 限制 128 bytes  |CodeWhale/crates/tui/src/core/engine.rs|const SUBAGENT_COMPLETION_HOLDER_MAX_BYTES: usize = 128;"
  "T1|BoundaryOnly 隔离当前用户 turn      |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_boundary_only_keeps_active_turn_clean_then_delivers_one_dedicated_handoff"
  "T1|BoundaryOnly 优先已入邮箱用户操作    |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_boundary_only_prefers_an_already_queued_external_op_over_idle_completion"
  "T1|Host lease 跨 FIFO turn 后再回流    |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_host_completion_hold_linearizes_fifo_before_ready_handoff"
  "T1|错误 holder 释放不越权且控制不饥饿  |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_completion_hold_requires_matching_release_and_never_starves_controls"
  "T1|遗弃 holder 只在 idle 边界过期      |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_abandoned_completion_hold_expires_only_at_idle_boundary"
  "T1|存活心跳不延长其他 holder           |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_live_holder_heartbeat_does_not_extend_crashed_holder_deadline"
  "T1|默认 Eager 保持历史回流行为          |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_default_eager_delivery_still_resumes_inside_the_active_turn"
  "T1|Host-managed 显式 claim 行为不变     |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_host_managed_engine_preserves_explicit_claim_delivery_under_boundary_policy"
  "T1|BoundaryOnly 回收 manager-only 终态  |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_boundary_only_recovers_manager_terminal_without_channel_frame_once"
  "T1|用户操作与 holder 先于 goal 续轮     |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_live_holder_and_queued_user_op_precede_goal_continuation"
  "T1|两阶段 barrier 只接受匹配确认        |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_two_phase_hold_requires_matching_confirmed_event"
  "T1|非 BoundaryOnly acquire fail closed |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_acquire_fails_closed_outside_boundary_only"

  "T2|宿主额外工具入口                    |CodeWhale/crates/tui/src/core/engine.rs|pub struct ExtraTools("
  "T2|动态禁用工具操作                    |CodeWhale/crates/tui/src/core/ops.rs|SetDisallowedTools { tools: Vec<String> }"
  "T2|宿主工具覆盖全部运行模式            |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_host_extra_tools_register_in_all_modes"
  "T2|宿主可选 direct 工具轮次预算        |CodeWhale/crates/tui/src/core/engine.rs|pub direct_tool_round_policy: Option<DirectToolRoundPolicy>"
  "T2|轮次耗尽只保留一次 handoff          |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_direct_tool_round_budget_narrows_to_one_handoff_then_closes"
  "T2|轮次耗尽后旧工具执行层拒绝          |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_direct_round_budget_blocks_a_hallucinated_old_tool_at_execution"
  "T2|handoff 后执行层关闭全部工具        |CodeWhale/crates/tui/src/core/engine/tests.rs|async fn forkguard_direct_round_policy_blocks_every_tool_after_handoff_execution"
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

  "T3|ambient project authority 密封       |CodeWhale/crates/tui/src/project_context.rs|fn forkguard_runtime_loader_ignores_ambient_project_authority"
  "T3|Permissions 100 KiB 窄例外回归      |CodeWhale/crates/tui/src/prompts.rs|fn forkguard_instruction_fragment_preserves_content_beyond_default_cap"
  "T3|disabled Skill 不可见且不可加载      |CodeWhale/crates/tui/src/skills/tests.rs|fn forkguard_disabled_skill_is_neither_rendered_nor_loadable"
  "T3|内部 reminder 不污染 Working Set    |CodeWhale/crates/tui/src/working_set.rs|fn forkguard_working_set_ignores_leading_system_reminder_paths"

  "T4|Automation 使用稳定 conversation key|CodeWhale/crates/tui/src/automation_manager.rs|add_task_with_conversation_key(new_task, Some(automation.id.clone()))"
  "T4|离线漏跑不补跑                      |CodeWhale/crates/tui/src/automation_manager.rs|fn forkguard_scheduler_skips_offline_misfires_without_backfill"
  "T4|同一 Automation 不重叠              |CodeWhale/crates/tui/src/automation_manager.rs|fn forkguard_scheduler_does_not_overlap_active_automation_run"
  "T4|Pinvou 历史 v3/v4 schema 窄兼容     |CodeWhale/crates/tui/src/task_manager.rs|const PINVOU_LEGACY_TASK_SCHEMA_VERSIONS"
  "T4|conversation/thread 跨 worker 持久化|CodeWhale/crates/tui/src/task_manager.rs|async fn forkguard_conversation_key_and_created_thread_survive_worker_boundary"

  "APP|产品白名单复用原生 allowed_tools   |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|allowed_tools: Some(crate::features::assistant::tool_policy::allowed_tool_names())"
  "APP|会话工具开关走动态禁用整形          |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|pub fn shape_disallowed_tools("
  "APP|v0.9.5 subagent state root 透传     |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|cfg.subagent_state_root = Some(roots.ledger);"
  "APP|停止与回收级联取消子智能体          |pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs|Op::CancelSubAgents"
  "APP|resolved route 由宿主统一解析        |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|pub fn resolve_runtime_route_for_model("
  "APP|128K/256K compaction 合约            |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|fn forkguard_compaction_128k_scenarios"
  "APP|定时任务复用 shared run API          |pinvou3-app/src-tauri/src/features/scheduled/tasks.rs|run_now_shared(&self.automations"
  "APP|多智能体面板只读 live worker         |pinvou3-app/src-tauri/src/features/multiagent/transcripts.rs|read_persisted_agent_worker_records(workspace)"
  "APP|静态 prompt composer 由 app 安装     |pinvou3-app/src-tauri/src/features/runtime_bundle/platform/mod.rs|set_static_prompt_composer_override"
  "APP|运行时会话读取不修复在途工具调用      |pinvou3-app/src-tauri/src/features/sessions/tests.rs|fn forkguard_runtime_snapshot_load_does_not_repair_in_flight_tool_call"
  "APP|进程启动显式恢复中断工具调用且幂等    |pinvou3-app/src-tauri/src/features/sessions/tests.rs|fn forkguard_boot_repairs_interrupted_tool_call_once"
  "APP|仅进程启动入口触发工具历史恢复        |pinvou3-app/src-tauri/src/lib.rs|SessionStore::boot_for_process_startup()"
  "APP|工具卡隐藏已知内部 runtime suffix    |pinvou3-app/src/platform/tauri/bridge.js|function stripInternalToolRuntimeSuffix("
  "APP|Pinvou Front 选择 BoundaryOnly       |pinvou3-app/src-tauri/src/features/assistant/engine.rs|SubAgentCompletionDeliveryPolicy::BoundaryOnly"
  "APP|普通 chat 原子提交 holder 与消息      |pinvou3-app/src-tauri/src/features/assistant/engine.rs|Op::HoldSubAgentCompletions"
  "APP|EnginePool 暴露窄 completion hold API|pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs|set_subagent_completion_hold"
  "APP|普通 chat reserve 前等待两阶段 barrier|pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs|ensure_subagent_completion_hold_ready"
  "APP|forwarder 串行推进 Applied 水位       |pinvou3-app/src-tauri/src/features/assistant/forwarder.rs|Event::SubAgentCompletionHoldApplied"
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

echo
bold "── 第 3 层：pinvou3-app forkguard 回归 ──"
( cd "$APP" && cargo test --lib --locked forkguard_ -- --test-threads=1 ) || fail=1

echo
if [[ $fail -eq 0 ]]; then
  green "✅ fork-guard 全过：4 个 v0.9.5 fork 主题完好。"
else
  red "❌ fork-guard 失败：请对照 docs/fork-modifications.md 排查。"
fi
exit $fail
