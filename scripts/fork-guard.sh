#!/usr/bin/env bash
# CodeWhale v0.9.5 clean re-fork guard: published four-theme baseline.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TUI="$REPO/CodeWhale"
APP="$REPO/pinvou3-app/src-tauri"
EXPECTED_UPSTREAM="853cb707bbcf4f7dc4268fba6d811e0d04083f9c"
PUBLISHED_HEAD="d127aed113529dc93754d044b9f352e9746f6b83"
PUBLISHED_COMMITS=10
FAST_ONLY=0
[[ "${1:-}" == "--fast" ]] && FAST_ONLY=1

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
bold()  { printf '\033[1m%s\033[0m\n' "$*"; }

fail=0

bold "── 第 0 层：v0.9.5 r8 公开四主题基线拓扑 ──"
actual_head="$(git -C "$TUI" rev-parse HEAD 2>/dev/null || true)"
if [[ "$actual_head" == "$PUBLISHED_HEAD" ]]; then
  expected_commits="$PUBLISHED_COMMITS"
  green "  ✓ CodeWhale gitlink 指向 r8 四主题公开基线 $PUBLISHED_HEAD"
else
  expected_commits=""
  red "  ✗ CodeWhale HEAD 为 ${actual_head:-<unreadable>}，应为 r8 公开 head $PUBLISHED_HEAD"
  fail=1
fi

if git -C "$TUI" merge-base --is-ancestor "$EXPECTED_UPSTREAM" HEAD 2>/dev/null; then
  green "  ✓ r8 公开基线继承官方 v0.9.5"
else
  red "  ✗ r8 公开基线未继承官方 v0.9.5 $EXPECTED_UPSTREAM"
  fail=1
fi

commit_count="$(git -C "$TUI" rev-list --count "$EXPECTED_UPSTREAM..HEAD" 2>/dev/null || true)"
if [[ -n "$expected_commits" && "$commit_count" == "$expected_commits" ]]; then
  green "  ✓ v0.9.5 之上 $expected_commits 个登记提交"
else
  red "  ✗ v0.9.5 之上有 ${commit_count:-<unreadable>} 个 commit，r8 合法拓扑应为 ${expected_commits:-10}"
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
  "T1|真实用户回合谓词排除 tool_result/内部信封|CodeWhale/crates/tui/src/runtime_handoff.rs|pub fn is_user_turn_prompt"
  "T1|宿主复用真实用户回合谓词            |CodeWhale/crates/tui/src/lib.rs|pub use runtime_handoff::is_user_turn_prompt;"
  "T1|编辑上一轮截断在真实用户消息        |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_edit_last_turn_cuts_at_user_prompt_before_tool_results"
  "T1|无用户消息可编辑时报错不发送        |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_edit_last_turn_without_user_prompt_errors_and_sends_nothing"

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
  "APP|128K/256K compaction 合约            |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|fn forkguard_compaction_128k_scenarios"
  "APP|定时任务复用 shared run API          |pinvou3-app/src-tauri/src/features/scheduled/tasks.rs|run_now_shared(&self.automations"
  "APP|多智能体面板只读 live worker         |pinvou3-app/src-tauri/src/features/multiagent/transcripts.rs|read_persisted_agent_worker_records(workspace)"
  "APP|静态 prompt composer 由 app 安装     |pinvou3-app/src-tauri/src/features/runtime_bundle/platform/mod.rs|set_static_prompt_composer_override"
  "APP|运行时会话读取不修复在途工具调用      |pinvou3-app/src-tauri/src/features/sessions/tests.rs|fn forkguard_runtime_snapshot_load_does_not_repair_in_flight_tool_call"
  "APP|进程启动显式恢复中断工具调用且幂等    |pinvou3-app/src-tauri/src/features/sessions/tests.rs|fn forkguard_boot_repairs_interrupted_tool_call_once"
  "APP|仅进程启动入口触发工具历史恢复        |pinvou3-app/src-tauri/src/lib.rs|SessionStore::boot_for_process_startup()"
  "APP|工具卡隐藏已知内部 runtime suffix    |pinvou3-app/src/platform/tauri/bridge.js|function stripInternalToolRuntimeSuffix("
  "APP|落盘兜底编辑截断与底座同口径        |pinvou3-app/src-tauri/src/features/sessions/tests.rs|fn forkguard_admitted_display_fallback_edit_cuts_before_trailing_tool_result"
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
