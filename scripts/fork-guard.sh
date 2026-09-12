#!/usr/bin/env bash
# CodeWhale v0.9.12 clean re-fork guard: 28 commits, six maintained themes (r2 tag pending).
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CODEWHALE="$REPO/CodeWhale"
APP="$REPO/pinvou3-app/src-tauri"
EXPECTED_UPSTREAM="dcd4c200f72f0c1ffd60d8e7f6850313db879fc5"
EXPECTED_HEAD="ae7e3fb36f89486f30d41b28ae0eaaa516ae4740"
EXPECTED_COMMITS=28
# 过渡期锚点：不可变 r1 tag 的收口 commit。层 0 断言它是当前 head 的祖先，
# 即 gitlink 沿维护分支领先 tag 而非另起分叉；r2 收口后随 TAG 常量一起退役。
R1_CLOSURE="1fafee7e26b60a59457a43bce50c63aa2ad9dbaf"
FAST_ONLY=0

case "${1:-}" in
  "") ;;
  --fast) FAST_ONLY=1 ;;
  *) echo "unknown argument: $1" >&2; exit 2 ;;
esac

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
bold()  { printf '\033[1m%s\033[0m\n' "$*"; }

fail=0

bold "── 第 0 层：v0.9.12 clean re-fork 拓扑（r1 tag 之后 13 个登记提交，r2 收口未切 tag）──"
actual_head="$(git -C "$CODEWHALE" rev-parse HEAD 2>/dev/null || true)"
if [[ "$actual_head" == "$EXPECTED_HEAD" ]]; then
  green "  ✓ CodeWhale gitlink 指向登记 head ${EXPECTED_HEAD}（过渡期：gitlink 领先 r1 tag，见 fork-policy 第 0 节豁免）"
else
  red "  ✗ CodeWhale HEAD 为 ${actual_head:-<unreadable>}，登记 head 为 $EXPECTED_HEAD"
  fail=1
fi

if git -C "$CODEWHALE" merge-base --is-ancestor "$EXPECTED_UPSTREAM" HEAD 2>/dev/null; then
  green "  ✓ 当前 gitlink 继承官方 v0.9.12"
else
  red "  ✗ 当前 gitlink 未继承官方 v0.9.12 $EXPECTED_UPSTREAM"
  fail=1
fi

if git -C "$CODEWHALE" merge-base --is-ancestor "$R1_CLOSURE" HEAD 2>/dev/null; then
  green "  ✓ 过渡期领先成立：r1 收口是当前 head 的祖先（gitlink 沿维护分支领先 tag）"
else
  red "  ✗ r1 收口 $R1_CLOSURE 不是当前 head 的祖先，过渡期领先关系断裂"
  fail=1
fi

commit_count="$(git -C "$CODEWHALE" rev-list --count "$EXPECTED_UPSTREAM..HEAD" 2>/dev/null || true)"
if [[ "$commit_count" == "$EXPECTED_COMMITS" ]]; then
  green "  ✓ v0.9.12 之上 $EXPECTED_COMMITS 个登记提交"
else
  red "  ✗ v0.9.12 之上有 ${commit_count:-<unreadable>} 个 commit，登记值为 $EXPECTED_COMMITS"
  fail=1
fi

bold "── 第 1 层：四主题与父仓适配指纹 ──"
# 格式：主题|说明|文件（相对父仓根）|grep -F 固定串
fingerprints=(
  "T1|宿主 facade 公开 Automation        |CodeWhale/crates/tui/src/lib.rs|pub mod automation_manager;"
  "T1|宿主显式 route limits              |CodeWhale/crates/tui/src/route_runtime.rs|pub fn resolve_runtime_route_with_limits("
  "T1|只读 worker ledger                |CodeWhale/crates/tui/src/tools/subagent/mod.rs|pub fn read_persisted_agent_worker_records("
  "T1|可靠 steer 返回关联 id             |CodeWhale/crates/tui/src/core/engine/handle.rs|pub async fn steer(&self, content: impl Into<String>) -> Result<String>"
  "T1|steer 撤回有界回归                 |CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_steer_lifecycle_withdrawal_is_bounded_and_prevents_commit"
  "T1|steer 真实 channel/turn-loop 回归       |CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_steer_channel_commits_live_and_drops_withdrawn_input_in_turn_loop"
  "T1|换会话批量取消子智能体              |CodeWhale/crates/tui/src/core/ops.rs|CancelSubAgents"
  "T1|批量取消按 session 隔离且幂等      |CodeWhale/crates/tui/src/tools/subagent/tests.rs|forkguard_cancel_all_running_is_session_scoped_and_idempotent"
  "T1|宿主 prompt-only profile 边界       |CodeWhale/crates/tui/src/tools/subagent/tests.rs|forkguard_host_profile_overlay_is_config_only_and_prompt_only"

  "T1|轮次绑定取消槽与身份契约            |CodeWhale/crates/tui/src/core/engine.rs|pub struct TurnCancelSlot"
  "T1|宿主轮次绑定取消入口                |CodeWhale/crates/tui/src/core/engine/handle.rs|pub fn cancel_turn(&self, turn_id: &str, reason: CancelReason, mode: CancelMode) -> bool"
  "T1|收口期 stop 处置入口不开火          |CodeWhale/crates/tui/src/core/engine/handle.rs|pub fn publish_stop_disposition(&self, reason: CancelReason, mode: CancelMode)"
  "T1|陈旧取消不误杀自主续跑轮回归        |CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_cancel_turn_binding_spares_unnamed_turns_and_hits_the_observed_turn"
  "T1|自启续轮陈旧 stop 端到端回归        |CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_idle_subagent_completion_self_start_ignores_a_stale_previous_turn_cancel"
  "T1|处置入口不触发任何 token 回归      |CodeWhale/crates/tui/src/core/engine/tests.rs|engine_handle_stop_disposition_publishes_without_firing_any_token"

  "T1|GLM-5.3 强制思考改写禁用 payload    |CodeWhale/crates/tui/src/client/chat.rs|fn apply_zai_forced_thinking_effort"
  "T1|BigModel host 纳入第一方 Chat 路由  |CodeWhale/crates/config/src/provider.rs|is_exact_https_route(base_url, \"open.bigmodel.cn\", \"api/paas/v4\")"

  "T2|宿主额外工具入口                  |CodeWhale/crates/tui/src/core/engine.rs|pub struct ExtraTools("
  "T2|宿主 MCP secret resolver          |CodeWhale/crates/tui/src/mcp.rs|pub fn install_mcp_secret_resolver("
  "T2|File 写入 64 KiB 硬上限           |CodeWhale/crates/tui/src/tools/file.rs|const WRITE_FILE_MAX_CONTENT_BYTES: usize = 64 * 1024;"
  "T2|write primitive 64 KiB 边界回归   |CodeWhale/crates/tui/src/tools/file/tests.rs|forkguard_write_primitive_enforces_the_64kib_boundary"
  "T2|write_file 64 KiB 边界回归        |CodeWhale/crates/tui/src/tools/file/tests.rs|forkguard_write_file_enforces_the_64kib_boundary"
  "T2|逐轮安全策略                      |CodeWhale/crates/tui/src/core/ops.rs|pub struct TurnToolSecurityPolicy"
  "T2|精确最终分发 fail-closed          |CodeWhale/crates/tui/src/core/engine/tool_execution.rs|forkguard_exact_dispatch_rejects_forged_backends"
  "T2|只读最终分发 fail-closed          |CodeWhale/crates/tui/src/core/engine/tool_execution.rs|forkguard_read_only_turn_rejects_write_at_final_dispatch"
  "T2|受限审计固定脱敏                  |CodeWhale/crates/tui/src/core/engine/tool_execution.rs|forkguard_restricted_tool_audit_redacts_private_payload"
  "T2|排队控制面不绕过逐轮权限            |CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_queued_control_op_keeps_restricted_turn_authority"
  "T2|受限轮推迟 idle 子智能体唤醒         |CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_restricted_turn_defers_idle_subagent_completion_until_new_message"
  "T2|受限轮推迟 idle Shell 唤醒           |CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_restricted_turn_defers_idle_shell_wake_until_new_message"
  "T2|禁用 MCP 不进入 catalog 或执行       |CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_denied_mcp_is_absent_from_catalog_and_blocked_at_execution"
  "T2|禁用 MCP 与未知工具错误不可区分       |CodeWhale/crates/tui/src/core/engine/turn_loop.rs|forkguard_denied_mcp_tool_error_matches_the_unknown_tool_error"
  "T2|API 搜索后备链直接落到 Bing          |CodeWhale/crates/tui/src/tools/web/backend.rs|forkguard_api_provider_chain_tail_is_bing"
  "T2|搜索全失败提供可操作配置提示          |CodeWhale/crates/tui/src/tools/web/backend.rs|all_unavailable_returns_actionable_error_without_private_details"
  "T2|宿主 Shell owner+session 入口      |CodeWhale/crates/tui/src/tools/shell.rs|pub fn execute_with_options_env_for_owner_and_session("
  "T2|评测控制默认关闭且显式启用          |CodeWhale/crates/tui/src/core/ops.rs|forkguard_benchmark_controls_are_explicit_and_default_off"
  "T2|评测只修复无歧义只读调用            |CodeWhale/crates/tui/src/core/engine/turn_loop.rs|forkguard_benchmark_repairs_only_unambiguous_read_actions"
  "T2|评测 read schema/附件修复受约束     |CodeWhale/crates/tui/src/core/engine/turn_loop.rs|forkguard_benchmark_repairs_read_schema_and_attachments"
  "T2|评测预算截断并清空后续工具面         |CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_benchmark_budget_truncates_batch_and_clears_followup_tool_surface"
  "T2|评测 final-only 熔断有界            |CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_benchmark_final_only_rejects_repeated_tool_only_responses"
  "T2|评测轮次执行前修复 File aliases     |CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_benchmark_turn_repairs_file_aliases_before_execution"

  "T2|cmd.exe 单字母斜杠旗标可跳过        |CodeWhale/crates/execpolicy/src/lib.rs|fn is_single_letter_slash_flag"
  "T2|deny 规则中段通配符跨令牌          |CodeWhale/crates/execpolicy/src/lib.rs|if rule_tokens[j] == \"*\" {"
  "T2|deny 命令词折叠 .exe 后缀           |CodeWhale/crates/execpolicy/src/lib.rs|basename.strip_suffix(\".exe\")"
  "T2|引擎克隆共享活规则集                |CodeWhale/crates/execpolicy/src/lib.rs|rulesets: Arc<RwLock<Vec<Ruleset>>>"
  "T2|File 绝对路径规则精确匹配          |CodeWhale/crates/execpolicy/src/lib.rs|fn absolute_path_rule_matches"
  "T2|subagent 工具调用过 execpolicy      |CodeWhale/crates/tui/src/core/engine.rs|pub(crate) fn exec_shell_ask_rule_decision_for_policy"
  "T2|subagent execpolicy 拒绝面回归      |CodeWhale/crates/tui/src/tools/subagent/tests.rs|async fn forkguard_subagent_execpolicy_deny_matches_main_line"

  "T2|工具结果 metadata.images 图片注入   |CodeWhale/crates/tui/src/core/engine/turn_loop.rs|fn tool_result_message_content("
  "T2|工具结果图片到达模型行为回归        |CodeWhale/crates/tui/src/core/engine/turn_loop.rs|fn forkguard_tool_result_images_reach_model_as_image_blocks"
  "T2|无 images 键结果保持不变回归        |CodeWhale/crates/tui/src/core/engine/turn_loop.rs|fn forkguard_tool_result_without_images_key_is_unchanged"
  "T2|坏图片降级不中断轮次回归            |CodeWhale/crates/tui/src/core/engine/turn_loop.rs|fn forkguard_tool_result_bad_images_degrade_to_text_only_result"
  "T2|单结果图片数量上限回归              |CodeWhale/crates/tui/src/core/engine/turn_loop.rs|fn forkguard_tool_result_images_are_capped_per_result"
  "T2|ToolResult images 元数据约定文档    |CodeWhale/crates/tools/src/lib.rs|a tool that produced image artifacts"

  "T2|finance 工具受网络策略闸门          |CodeWhale/crates/tui/src/tools/finance.rs|fn check_network_policy("
  "T2|verifier 不再教退役工具名          |CodeWhale/crates/tui/src/tools/verifier.rs|\"poll_with\": [\"task_shell_wait\"]"
  "T2|notify 配置方法合同钉               |CodeWhale/crates/tui/src/tui/notifications.rs|fn settings_installs_configured_method_from_config"
  "T2|shell 指引与执行同一 dispatcher    |CodeWhale/crates/tui/src/tools/shell/guidance.rs|pub(super) fn runtime_command_guidance()"
  "T2|指引对齐 catalog 一致性回归        |CodeWhale/crates/tui/src/tools/shell/tests.rs|fn forkguard_shell_catalog_guidance_matches_execution"

  "T3|静态 prompt composer              |CodeWhale/crates/tui/src/prompts.rs|pub fn set_static_prompt_composer_override("
  "T3|ambient project authority 密封     |CodeWhale/crates/tui/src/project_context.rs|forkguard_runtime_loader_ignores_ambient_project_authority"
  "T3|显式 Skills 根排除 ambient 来源    |CodeWhale/crates/tui/src/skills/tests.rs|forkguard_explicit_skills_dir_excludes_ambient_workspace_sources"
  "T3|Permissions 窄 100 KiB 预算       |CodeWhale/crates/tui/src/prompts.rs|forkguard_instruction_fragment_preserves_explicit_host_budget"
  "T3|内部 reminder 不污染 working set  |CodeWhale/crates/tui/src/working_set.rs|forkguard_working_set_ignores_leading_system_reminder_paths"

  "T4|Automation 稳定 conversation key |CodeWhale/crates/tui/src/automation_manager.rs|add_task_with_conversation_key(new_task, Some(automation.id.clone()))"
  "T4|离线不补跑且同一任务不重叠          |CodeWhale/crates/tui/src/automation_manager.rs|forkguard_scheduler_skips_offline_backfill_and_overlapping_runs"
  "T4|过期一次性任务精确入队一次            |CodeWhale/crates/tui/src/automation_manager.rs|forkguard_once_schedule_missed_while_offline_enqueues_exactly_one_run"
  "T4|Pinvou 历史 v4 schema 窄兼容       |CodeWhale/crates/tui/src/task_manager.rs|const PINVOU_LEGACY_TASK_SCHEMA_VERSION: u32 = 4;"
  "T4|conversation owner 与任务参数持久  |CodeWhale/crates/tui/src/automation_manager.rs|forkguard_automation_enqueue_preserves_settings_and_conversation_owner"
  "T4|终态任务可显式清理                  |CodeWhale/crates/tui/src/task_manager.rs|pub async fn delete_terminal_task("
  "T4|Task 删除拒绝 active 且幂等        |CodeWhale/crates/tui/src/task_manager.rs|forkguard_terminal_task_delete_refuses_active_and_is_idempotent"
  "T4|Automation run 删除拒绝 active 且幂等|CodeWhale/crates/tui/src/automation_manager.rs|forkguard_terminal_automation_run_delete_refuses_active_and_is_idempotent"
  "T4|worker 建线程边界事件              |CodeWhale/crates/tui/src/task_manager.rs|ThreadCreated {"

  "T5|全保真归档导出入口                |CodeWhale/crates/tui/src/session_export.rs|pub fn write_session_archive("
  "T5|liblzma 静态压缩依赖              |CodeWhale/crates/tui/Cargo.toml|liblzma = { version = \"0.4\", features = [\"static\"] }"
  "T5|CLI sessions export 子命令        |CodeWhale/crates/tui/src/lib.rs|Some(SessionsCommand::Export {"
  "T5|归档全上下文 roundtrip 回归        |CodeWhale/crates/tui/src/session_export.rs|forkguard_session_archive_export_roundtrips_full_context"
  "T5|artifacts 包含/排除回归            |CodeWhale/crates/tui/src/session_export.rs|forkguard_session_archive_includes_artifacts_and_respects_skip"
  "T5|短读 member fail-closed 回归      |CodeWhale/crates/tui/src/session_export.rs|forkguard_session_archive_rejects_artifact_shorter_than_recorded_size"

  "T6|可缩容 launch gate 模块            |CodeWhale/crates/tui/src/tools/subagent/governor.rs|pub(crate) struct DynamicGate"
  "T6|限流 AIMD 治理器                   |CodeWhale/crates/tui/src/tools/subagent/governor.rs|pub(crate) struct RateLimitGovernor"
  "T6|fleet 治理器接线全部 spawn 路径    |CodeWhale/crates/tui/src/tools/subagent/mod.rs|runtime.governor = Some(Arc::clone(&self.governor));"
  "T6|限流时间自愈行为回归               |CodeWhale/crates/tui/src/tools/subagent/governor.rs|fn forkguard_rate_limit_governor_pauses_and_time_recovers_after_window_drains"

  "APP|spawn 前安装 Engine session id   |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|cfg.session_id = Some(session_id.to_string());"
  "APP|产品白名单复用原生 allowed_tools |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|allowed_tools: Some(crate::features::assistant::tool_policy::allowed_tool_names())"
  "APP|会话工具开关走动态禁用整形        |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|pub fn shape_disallowed_tools("
  "APP|subagent ledger root 透传       |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|cfg.subagent_state_root = Some(roots.ledger);"
  "APP|逐轮精确安全策略下发               |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|turn_tool_security: Some(Arc::new(turn_tool_security))"
  "APP|受限操作动态工具清空               |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|dynamic_tools: Vec::new()"
  "APP|停止与回收级联取消子智能体          |pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs|Op::CancelSubAgents"
  "APP|resolved route 由宿主统一解析     |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|pub fn resolve_runtime_route_for_model("
  "APP|GLM 小写存量配置解析到规范模型    |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|fn forkguard_zai_direct_route_survives_model_casing_mismatch"
  "APP|128K/256K compaction 合约        |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|fn forkguard_compaction_128k_scenarios"
  "APP|事件按 owner session 隔离         |pinvou3-app/src-tauri/src/features/assistant/forwarder.rs|owner_session_id == session_id"
  "APP|定时任务报告 ThreadCreated        |pinvou3-app/src-tauri/src/features/scheduled/executor.rs|TaskExecutionEvent::ThreadCreated"
  "APP|定时任务复用 shared run API      |pinvou3-app/src-tauri/src/features/scheduled/tasks.rs|run_now_shared(&self.automations"
  "APP|多智能体面板只读 live worker     |pinvou3-app/src-tauri/src/features/multiagent/transcripts.rs|read_persisted_agent_worker_records(workspace)"
  "APP|静态 prompt composer 由应用安装   |pinvou3-app/src-tauri/src/features/runtime_bundle/platform/mod.rs|set_static_prompt_composer_override"
  "APP|运行时读取不修复在途工具调用       |pinvou3-app/src-tauri/src/features/sessions/tests.rs|fn forkguard_runtime_snapshot_load_does_not_repair_in_flight_tool_call"
  "APP|进程启动恢复中断调用且幂等         |pinvou3-app/src-tauri/src/features/sessions/tests.rs|fn forkguard_boot_repairs_interrupted_tool_call_once"
  "APP|仅进程启动入口触发历史恢复         |pinvou3-app/src-tauri/src/lib.rs|SessionStore::boot_for_process_startup()"
  "APP|MCP secret 经 resolver 钩子下发  |pinvou3-app/src-tauri/src/features/marketplace/mod.rs|pub fn install_mcp_secret_resolver()"
  "APP|进程 env 写收口到启动窗口         |pinvou3-app/src-tauri/src/lib.rs|pub(crate) fn startup_process_env()"
  "APP|工具卡隐藏内部 runtime suffix    |pinvou3-app/src/platform/tauri/bridge.js|function stripInternalToolRuntimeSuffix("
  "APP|落盘编辑截断与底座同口径           |pinvou3-app/src-tauri/src/features/sessions/tests.rs|fn forkguard_admitted_display_fallback_edit_cuts_before_trailing_tool_result"
  "APP|不支持的新用户内容不回退旧轮       |pinvou3-app/src-tauri/src/features/sessions/tests.rs|fn forkguard_admitted_display_fallback_does_not_skip_unsupported_user_turn"
  "APP|GUI 导出复用底座 session_export    |pinvou3-app/src-tauri/src/features/sessions/store.rs|deepseek_tui::session_export::write_session_archive("
  "APP|GUI 导出 store 契约回归            |pinvou3-app/src-tauri/src/features/sessions/tests.rs|fn forkguard_session_archive_export_via_store_keeps_full_context"
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

forkguard_count="$(grep -Rho --include='*.rs' 'forkguard_[A-Za-z0-9_]*' "$CODEWHALE/crates" 2>/dev/null | sort -u | wc -l | tr -d ' ')"
if [[ "$forkguard_count" -ge 54 ]]; then
  green "  ✓ CodeWhale 至少保留 54 条独立 forkguard 行为名（实际 ${forkguard_count}）"
else
  red "  ✗ CodeWhale forkguard 行为名仅 ${forkguard_count:-0}，登记下限为 54"
  fail=1
fi

if [[ $FAST_ONLY -eq 1 ]]; then
  echo
  [[ $fail -eq 0 ]] && green "指纹层全过 (--fast)" || red "指纹层有缺失"
  exit $fail
fi

echo
bold "── 第 2 层：CodeWhale forkguard 回归 ──"
( cd "$CODEWHALE" && cargo test -p codewhale-tui --lib --locked forkguard_ -- --test-threads=1 ) || fail=1
( cd "$CODEWHALE" && cargo test -p codewhale-tui --lib --locked --features benchmark-eval-controls forkguard_benchmark_ -- --test-threads=1 ) || fail=1

echo
bold "── 第 3 层：pinvou3-app forkguard 回归 ──"
( cd "$APP" && cargo test --lib --locked forkguard_ -- --test-threads=1 ) || fail=1
( cd "$APP" && cargo check --all-targets --features benchmark-hooks --locked ) || fail=1
( cd "$APP" && cargo test --lib --locked --features benchmark-hooks eval_send_message_op_isolated_from_gui_authority_and_installs_exact_policy -- --test-threads=1 ) || fail=1
( cd "$APP" && cargo test --lib --locked --features benchmark-hooks features::assistant::product_runtime::headless_bridge::tests -- --test-threads=1 ) || fail=1
( cd "$APP" && cargo test --lib --locked --features benchmark-hooks features::assistant::product_runtime::agentic_task::tests -- --test-threads=1 ) || fail=1
( cd "$APP" && cargo test --lib --locked --features benchmark-hooks engine_config_tool_call_cap_respects_env_override -- --test-threads=1 ) || fail=1
( cd "$APP" && cargo test --lib --locked --features benchmark-hooks headless_bridge_contract_tests:: -- --test-threads=1 ) || fail=1

echo
if [[ $fail -eq 0 ]]; then
  green "✅ fork-guard 全过：CodeWhale v0.9.12 r1 的 4 个 Pinvou 主题完好。"
else
  red "❌ fork-guard 失败：请对照 docs/fork-modifications.md 排查。"
fi
exit $fail
