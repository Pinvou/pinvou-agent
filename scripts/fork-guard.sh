#!/usr/bin/env bash
# CodeWhale v0.9.12 clean re-fork guard: 92 review-candidate commits, ten maintained themes (r3 closed at pinvou-v0.9.12-r3; inside a transition window the head may lead the tag).
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CODEWHALE="$REPO/CodeWhale"
APP="$REPO/pinvou3-app/src-tauri"
EXPECTED_UPSTREAM="dcd4c200f72f0c1ffd60d8e7f6850313db879fc5"
# Review candidate above the published #115/#116/#117 maintenance base.
EXPECTED_HEAD="2a0221f22705c04d6cdac6db29b62b56d4d6738e"
EXPECTED_COMMITS=92
# Published maintenance base of this integration.
MAINTENANCE_BASE="ea9fdc90f1c90335554833195eeeb847c83d82dd"
# r1 收口锚点：不可变 r1 tag 的收口 commit。层 0 断言它是当前 head 的祖先，
# 即维护分支自 r1 收口线性前进而非另起分叉（2026-10-02 起处于过渡期：gitlink =
# 维护分支头，领先钉在 r3 收口 commit 的不可变 tag；tag 钉点与领先关系由
# scripts/verify-public-submodule.sh 在过渡期断言）。
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

bold "── Layer 0: v0.9.12 registered maintenance topology ──"
actual_head="$(git -C "$CODEWHALE" rev-parse HEAD 2>/dev/null || true)"
if [[ "$actual_head" == "$EXPECTED_HEAD" ]]; then
  green "  ✓ CodeWhale matches registered review head ${EXPECTED_HEAD}; public maintenance alignment is checked separately"
else
  red "  ✗ CodeWhale HEAD ${actual_head:-<unreadable>} differs from registered head $EXPECTED_HEAD"
  fail=1
fi

if git -C "$CODEWHALE" merge-base --is-ancestor "$EXPECTED_UPSTREAM" HEAD 2>/dev/null; then
  green "  ✓ Registered head descends from official v0.9.12"
else
  red "  ✗ Registered head does not descend from official v0.9.12 $EXPECTED_UPSTREAM"
  fail=1
fi

if git -C "$CODEWHALE" merge-base --is-ancestor "$R1_CLOSURE" HEAD 2>/dev/null; then
  green "  ✓ Registered head descends from the immutable r1 closure; public maintenance alignment is checked separately"
else
  red "  ✗ Immutable r1 closure $R1_CLOSURE is not an ancestor of the registered head"

  fail=1
fi

if git -C "$CODEWHALE" merge-base --is-ancestor "$MAINTENANCE_BASE" HEAD 2>/dev/null; then
  green "  ✓ Registered head contains public maintenance base $MAINTENANCE_BASE"
else
  red "  ✗ Registered head does not contain public maintenance base $MAINTENANCE_BASE"
  fail=1
fi

commit_count="$(git -C "$CODEWHALE" rev-list --count "$EXPECTED_UPSTREAM..HEAD" 2>/dev/null || true)"
if [[ "$commit_count" == "$EXPECTED_COMMITS" ]]; then
  green "  ✓ $EXPECTED_COMMITS registered commits above v0.9.12"
else
  red "  ✗ ${commit_count:-<unreadable>} commits above v0.9.12; expected $EXPECTED_COMMITS"
  fail=1
fi

bold "── Layer 1: registered themes and parent-adaptation fingerprints ──"
# Format: theme|description|file relative to parent root|grep -F literal
fingerprints=(
  "T2|Test metadata summary loss requests recovery|CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_run_tests_metadata_summary_cut_requests_recovery"
  "T2|Child report artifact retains exact bytes and ids|CodeWhale/crates/tui/src/tools/subagent/tests.rs|fn forkguard_subagent_report_artifact_keeps_exact_bytes"
  "T2|Deferred subagent report names explicit activation fallback|CodeWhale/crates/tui/src/tools/subagent/tests.rs|fn stamp_subagent_summary_truncates_when_over_budget"
  "T2|Legacy spillover names explicit activation fallback|CodeWhale/crates/tui/src/tools/truncate.rs|fn truncation_footer_without_an_artifact_names_the_legacy_retrieval_route"
  "T2|Preserved output activates only allowed retrieval schema|CodeWhale/crates/tui/src/core/engine/tool_catalog/tests.rs|fn forkguard_preserved_output_activates_only_available_retrieval_schema"
  "T2|Fetch full envelope survives context cut and one-hop recovery|CodeWhale/crates/tui/src/tools/fetch_url/tests.rs|fn forkguard_fetch_url_full_envelope_round_trips_before_context_cut"
  "T2|Small fetch JSON stays artifact-free|CodeWhale/crates/tui/src/tools/fetch_url/tests.rs|fn forkguard_fetch_url_small_envelope_stays_parseable_and_artifact_free"
  "T2|Immutable artifacts are private before publication|CodeWhale/crates/tui/src/artifacts.rs|fn forkguard_immutable_artifacts_are_private_and_replay_safe"
  "T2|Aggregate envelope preserves original before fan-out|CodeWhale/crates/tui/src/core/engine/output_recovery_tests.rs|fn forkguard_turn_loop_envelope_preserves_output_before_fanout"
  "T2|serial complete output stays artifact-free|CodeWhale/crates/tui/src/core/engine/output_recovery_tests.rs|fn forkguard_turn_loop_serial_keeps_complete_output_artifact_free"
  "T2|parallel complete output stays artifact-free|CodeWhale/crates/tui/src/core/engine/output_recovery_tests.rs|fn forkguard_turn_loop_parallel_keeps_complete_output_artifact_free"
  "T2|envelope complete output stays artifact-free|CodeWhale/crates/tui/src/core/engine/output_recovery_tests.rs|fn forkguard_turn_loop_envelope_keeps_complete_output_artifact_free"
  "T2|Serial dispatch preserves original before fan-out|CodeWhale/crates/tui/src/core/engine/output_recovery_tests.rs|fn forkguard_turn_loop_serial_preserves_output_before_fanout"
  "T2|Parallel dispatch preserves original before fan-out|CodeWhale/crates/tui/src/core/engine/output_recovery_tests.rs|fn forkguard_turn_loop_parallel_preserves_output_before_fanout"
  "T2|Gate stream DTO omissions are recoverable|CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_task_stream_summaries_are_inline_or_recoverable"
  "T2|Interrupted coordination field omissions are recoverable|CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_short_coordination_fields_are_inline_or_recoverable"
  "T2|Real taskless gate retains both output streams|CodeWhale/crates/tui/src/tools/tasks.rs|fn forkguard_gate_without_active_task_keeps_both_streams_recoverable"
  "T2|Taskless gate recovery declares producer boundary|CodeWhale/crates/tui/src/tools/tasks.rs|fn forkguard_taskless_gate_recovery_names_returned_summaries"
  "T2|Engine recovery names the complete returned result|CodeWhale/crates/tui/src/core/engine/context.rs|complete returned tool result at"
  "T2|Real interrupted child retains checkpoint recovery|CodeWhale/crates/tui/src/tools/subagent/tests.rs|fn forkguard_interrupted_snapshot_preserves_checkpoint_recovery"
  "T2|Complete structured output has no false recovery|CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_complete_structured_results_do_not_request_recovery"
  "T2|Structured field omissions retain recovery|CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_structured_field_cuts_still_request_recovery"
  "T2|Adaptive evidence keeps original line totals|CodeWhale/crates/tui/src/tools/truncate.rs|fn adaptive_evidence_footer_names_artifact_path_and_recovery"
  "T2|Preview metadata overflow fails closed|CodeWhale/crates/tui/src/tools/truncate.rs|fn refit_spilled_preview_rejects_overflowing_metadata"
  "T2|Unsaved wire previews are not full-content refs|CodeWhale/crates/tui/src/client/chat/tests/stream_decoder.rs|fn request_builder_never_deduplicates_unsaved_output_previews"
  "T2|Header-padded Web body and JSON fields survive|CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_web_body_and_json_fields_survive_header_padding"
  "T2|Web JSONPath schema agrees with parser results|CodeWhale/crates/tui/src/tools/web/contract.rs|fn forkguard_web_fields_description_matches_parser"
  "T2|Document the Web JSONPath subset|CodeWhale/crates/tui/src/tools/web_tool.rs|Recursive descent (..) and filters are not supported"
  "T2|Preserve failed tool-result flags|CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_tool_failure_flag_survives_recording_and_round_trip"
  "T2|Unix shell guidance preservation test|CodeWhale/crates/tui/src/tools/shell/guidance.rs|fn shell_guidance_preserves_unix_shell_contracts"
  "T2|Windows PowerShell policy bypass flags|CodeWhale/crates/tui/src/shell_dispatcher.rs|fn powershell_build_command_includes_no_profile_and_command_flags"
  "T2|Windows PowerShell encoded fallback builder|CodeWhale/crates/tui/src/shell_dispatcher.rs|fn forkguard_powershell_encoded_fallback_avoids_the_script_file"
  "T2|Windows PowerShell encoded form off-limit payload|CodeWhale/crates/tui/src/shell_dispatcher.rs|fn forkguard_powershell_encoded_fallback_skips_a_payload_past_the_command_line_limit"
  "T2|Windows PowerShell encoded form non-PowerShell shells|CodeWhale/crates/tui/src/shell_dispatcher.rs|fn forkguard_powershell_encoded_fallback_is_none_for_other_shells"
  "T2|Windows execution-policy retry wiring|CodeWhale/crates/tui/src/tools/shell.rs|fn retry_powershell_without_script_file"
  "T2|Windows execution-policy retry gate helper|CodeWhale/crates/tui/src/tools/shell.rs|fn powershell_refusal_needs_inline_retry"
  "T2|Windows execution-policy refusal ownership|CodeWhale/crates/tui/src/tools/shell.rs|fn powershell_temp_script"
  "T2|Windows execution-policy retry startup failure|CodeWhale/crates/tui/src/tools/shell.rs|fn keep_first_result_after_a_failed_retry"
  "T2|Windows execution-policy rejection predicate|CodeWhale/crates/tui/src/tools/shell/tests.rs|fn forkguard_powershell_execution_policy_rejection_is_locale_independent"
  "T2|Windows execution-policy retry gate|CodeWhale/crates/tui/src/tools/shell/tests.rs|fn forkguard_powershell_inline_retry_only_for_a_failed_file_refusal"
  "T2|Windows execution-policy retry glue|CodeWhale/crates/tui/src/tools/shell/tests.rs|fn forkguard_powershell_inline_retry_runs_the_command_and_notes_the_fallback"
  "T2|Windows execution-policy retry keeps first evidence|CodeWhale/crates/tui/src/tools/shell/tests.rs|fn forkguard_powershell_retry_keeps_the_first_result_when_it_cannot_start"
  "T2|Windows shell UTF-8 prefix family rule|CodeWhale/crates/tui/src/sandbox/mod.rs|fn forkguard_windows_shell_text_matches_the_shell_family"
  "T2|Shell environment overlays preserve defaults|CodeWhale/crates/tui/src/sandbox/mod.rs|fn command_spec_env_overlays_preserve_existing_values"
  "T2|Windows Python stdio environment end-to-end|CodeWhale/crates/tui/src/sandbox/mod.rs|fn forkguard_windows_shell_env_overlays_keep_python_stdio_utf8"
  "T2|Windows explicit environment override casing|CodeWhale/crates/tui/src/sandbox/mod.rs|fn windows_shell_env_explicit_override_is_case_insensitive"
  "T2|Windows native Python lowercase bash output|CodeWhale/crates/tui/src/tools/shell/tests.rs|fn forkguard_windows_python_stdio_lowercase_bash"
  "T2|Windows native Python background output|CodeWhale/crates/tui/src/tools/shell/tests.rs|fn forkguard_windows_python_stdio_background_shell"
  "T2|Windows PowerShell encoded form end-to-end|CodeWhale/crates/tui/src/tools/shell/tests.rs|fn forkguard_powershell_encoded_spec_runs_a_non_ascii_payload"
  "T2|Windows PowerShell temp-script form end-to-end|CodeWhale/crates/tui/src/tools/shell/tests.rs|fn forkguard_powershell_temp_script_form_runs_a_non_ascii_payload"
  "T1|automation manager|CodeWhale/crates/tui/src/lib.rs|pub mod automation_manager;"
  "T1|resolve runtime route with limits|CodeWhale/crates/tui/src/route_runtime.rs|pub fn resolve_runtime_route_with_limits("
  "T1|read persisted agent worker records|CodeWhale/crates/tui/src/tools/subagent/mod.rs|pub fn read_persisted_agent_worker_records("
  "T1|steer|CodeWhale/crates/tui/src/core/engine/handle.rs|pub async fn steer(&self, content: impl Into<String>) -> Result<String>"
  "T1|steer lifecycle withdrawal is bounded and prevents commit|CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_steer_lifecycle_withdrawal_is_bounded_and_prevents_commit"
  "T1|steer channel commits live and drops withdrawn input in turn loop|CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_steer_channel_commits_live_and_drops_withdrawn_input_in_turn_loop"
  "T1|Host operation cancels subagents|CodeWhale/crates/tui/src/core/ops.rs|CancelSubAgents"
  "T1|cancel all running is session scoped and idempotent|CodeWhale/crates/tui/src/tools/subagent/tests.rs|forkguard_cancel_all_running_is_session_scoped_and_idempotent"
  "T1|host profile overlay is config only and prompt only|CodeWhale/crates/tui/src/tools/subagent/tests.rs|forkguard_host_profile_overlay_is_config_only_and_prompt_only"

  "T1|TurnCancelSlot|CodeWhale/crates/tui/src/core/engine.rs|pub struct TurnCancelSlot"
  "T1|cancel turn|CodeWhale/crates/tui/src/core/engine/handle.rs|pub fn cancel_turn(&self, turn_id: &str, reason: CancelReason, mode: CancelMode) -> bool"
  "T1|publish stop disposition|CodeWhale/crates/tui/src/core/engine/handle.rs|pub fn publish_stop_disposition(&self, reason: CancelReason, mode: CancelMode)"
  "T1|cancel turn binding spares unnamed turns and hits the observed turn|CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_cancel_turn_binding_spares_unnamed_turns_and_hits_the_observed_turn"
  "T1|idle subagent completion self start ignores a stale previous turn cancel|CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_idle_subagent_completion_self_start_ignores_a_stale_previous_turn_cancel"
  "T1|Stop disposition publishes without firing tokens|CodeWhale/crates/tui/src/core/engine/tests.rs|engine_handle_stop_disposition_publishes_without_firing_any_token"
  "T1|turn started echoes submission id self starts stay none|CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_turn_started_echoes_submission_id_self_starts_stay_none"

  "T1|apply zai forced thinking effort|CodeWhale/crates/tui/src/client/chat.rs|fn apply_zai_forced_thinking_effort"
  "T1|BigModel host uses first-party Chat routing|CodeWhale/crates/config/src/provider.rs|is_exact_https_route(base_url, \"open.bigmodel.cn\", \"api/paas/v4\")"

  "T2|ExtraTools|CodeWhale/crates/tui/src/core/engine.rs|pub struct ExtraTools("
  "T2|install mcp secret resolver|CodeWhale/crates/tui/src/mcp.rs|pub fn install_mcp_secret_resolver("
  "T2|WRITE FILE MAX CONTENT BYTES|CodeWhale/crates/tui/src/tools/file.rs|const WRITE_FILE_MAX_CONTENT_BYTES: usize = 64 * 1024;"
  "T2|write primitive enforces the 64kib boundary|CodeWhale/crates/tui/src/tools/file/tests.rs|forkguard_write_primitive_enforces_the_64kib_boundary"
  "T2|write file enforces the 64kib boundary|CodeWhale/crates/tui/src/tools/file/tests.rs|forkguard_write_file_enforces_the_64kib_boundary"
  "T2|TurnToolSecurityPolicy|CodeWhale/crates/tui/src/core/ops.rs|pub struct TurnToolSecurityPolicy"
  "T2|exact dispatch rejects forged backends|CodeWhale/crates/tui/src/core/engine/tool_execution.rs|forkguard_exact_dispatch_rejects_forged_backends"
  "T2|read only turn rejects write at final dispatch|CodeWhale/crates/tui/src/core/engine/tool_execution.rs|forkguard_read_only_turn_rejects_write_at_final_dispatch"
  "T2|restricted tool audit redacts private payload|CodeWhale/crates/tui/src/core/engine/tool_execution.rs|forkguard_restricted_tool_audit_redacts_private_payload"
  "T2|queued control op keeps restricted turn authority|CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_queued_control_op_keeps_restricted_turn_authority"
  "T2|restricted turn defers idle subagent completion until new message|CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_restricted_turn_defers_idle_subagent_completion_until_new_message"
  "T2|restricted turn defers idle shell wake until new message|CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_restricted_turn_defers_idle_shell_wake_until_new_message"
  "T2|denied mcp is absent from catalog and blocked at execution|CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_denied_mcp_is_absent_from_catalog_and_blocked_at_execution"
  "T2|denied mcp tool error matches the unknown tool error|CodeWhale/crates/tui/src/core/engine/turn_loop.rs|forkguard_denied_mcp_tool_error_matches_the_unknown_tool_error"
  "T2|api provider chain tail is bing|CodeWhale/crates/tui/src/tools/web/backend.rs|forkguard_api_provider_chain_tail_is_bing"
  "T2|Unavailable search gives actionable configuration guidance|CodeWhale/crates/tui/src/tools/web/backend.rs|all_unavailable_returns_actionable_error_without_private_details"
  "T2|execute with options env for owner and session|CodeWhale/crates/tui/src/tools/shell.rs|pub fn execute_with_options_env_for_owner_and_session("
  "T2|benchmark controls are explicit and default off|CodeWhale/crates/tui/src/core/ops.rs|forkguard_benchmark_controls_are_explicit_and_default_off"
  "T2|benchmark repairs only unambiguous read actions|CodeWhale/crates/tui/src/core/engine/turn_loop.rs|forkguard_benchmark_repairs_only_unambiguous_read_actions"
  "T2|benchmark repairs read schema and attachments|CodeWhale/crates/tui/src/core/engine/turn_loop.rs|forkguard_benchmark_repairs_read_schema_and_attachments"
  "T2|benchmark budget truncates batch and clears followup tool surface|CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_benchmark_budget_truncates_batch_and_clears_followup_tool_surface"
  "T2|benchmark final only rejects repeated tool only responses|CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_benchmark_final_only_rejects_repeated_tool_only_responses"
  "T2|benchmark turn repairs file aliases before execution|CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_benchmark_turn_repairs_file_aliases_before_execution"

  "T2|is single letter slash flag|CodeWhale/crates/execpolicy/src/lib.rs|fn is_single_letter_slash_flag"
  "T2|Deny rule supports a bounded middle wildcard|CodeWhale/crates/execpolicy/src/lib.rs|if rule_tokens[j] == \"*\" {"
  "T2|Deny command folds one exe suffix|CodeWhale/crates/execpolicy/src/lib.rs|basename.strip_suffix(\".exe\")"
  "T2|Rule sets remain shared across executor clones|CodeWhale/crates/execpolicy/src/lib.rs|rulesets: Arc<RwLock<Vec<Ruleset>>>"
  "T2|absolute path rule matches|CodeWhale/crates/execpolicy/src/lib.rs|fn absolute_path_rule_matches"
  "T2|exec shell ask rule decision for policy|CodeWhale/crates/tui/src/core/engine.rs|pub(crate) fn exec_shell_ask_rule_decision_for_policy"
  "T2|forkguard subagent execpolicy deny matches main line|CodeWhale/crates/tui/src/tools/subagent/tests.rs|async fn forkguard_subagent_execpolicy_deny_matches_main_line"

  "T2|tool result message content|CodeWhale/crates/tui/src/core/engine/turn_loop.rs|fn tool_result_message_content("
  "T2|forkguard tool result images reach model as image blocks|CodeWhale/crates/tui/src/core/engine/turn_loop.rs|fn forkguard_tool_result_images_reach_model_as_image_blocks"
  "T2|forkguard tool result without images key is unchanged|CodeWhale/crates/tui/src/core/engine/turn_loop.rs|fn forkguard_tool_result_without_images_key_is_unchanged"
  "T2|forkguard tool result bad images degrade to text only result|CodeWhale/crates/tui/src/core/engine/turn_loop.rs|fn forkguard_tool_result_bad_images_degrade_to_text_only_result"
  "T2|forkguard tool result images are capped per result|CodeWhale/crates/tui/src/core/engine/turn_loop.rs|fn forkguard_tool_result_images_are_capped_per_result"
  "T2|ToolResult image artifact contract|CodeWhale/crates/tools/src/lib.rs|a tool that produced image artifacts"

  "T2|check network policy|CodeWhale/crates/tui/src/tools/finance.rs|fn check_network_policy("
  "T2|Verifier names the current shell polling tool|CodeWhale/crates/tui/src/tools/verifier.rs|\"poll_with\": [\"task_shell_wait\"]"
  "T2|settings installs configured method from config|CodeWhale/crates/tui/src/tui/notifications.rs|fn settings_installs_configured_method_from_config"
  "T2|runtime command guidance|CodeWhale/crates/tui/src/tools/shell/guidance.rs|pub(super) fn runtime_command_guidance()"
  "T2|forkguard shell catalog guidance matches execution|CodeWhale/crates/tui/src/tools/shell/tests.rs|fn forkguard_shell_catalog_guidance_matches_execution"
  "T2|Computer-use zoom preserves raster ancestry|CodeWhale/plugins/computer-use/src/raster.mjs|export function zoomChildRaster(prev, region)"

  "T3|set static prompt composer override|CodeWhale/crates/tui/src/prompts.rs|pub fn set_static_prompt_composer_override("
  "T3|runtime loader ignores ambient project authority|CodeWhale/crates/tui/src/project_context.rs|forkguard_runtime_loader_ignores_ambient_project_authority"
  "T3|explicit skills dir excludes ambient workspace sources|CodeWhale/crates/tui/src/skills/tests.rs|forkguard_explicit_skills_dir_excludes_ambient_workspace_sources"
  "T3|Skill index discovers deferred tools through tool_search|CodeWhale/crates/tui/src/skills/mod.rs|activate it via \`tool_search\`; if it is still missing"
  "T3|forkguard skill index usage names tool search activation|CodeWhale/crates/tui/src/skills/tests.rs|fn forkguard_skill_index_usage_names_tool_search_activation"
  "T3|Subagent skill catalog discovers deferred tools|CodeWhale/crates/tui/src/tools/subagent/mod.rs|activating it via \`tool_search\` first if it is not in your tool list"
  "T3|forkguard subagent skill catalog uses tool search discovery|CodeWhale/crates/tui/src/tools/subagent/tests.rs|fn forkguard_subagent_skill_catalog_uses_tool_search_discovery"
  "T3|MCP discovery names registry unavailability honestly|CodeWhale/crates/tui/assets/skills/mcp-discovery/SKILL.md|MCP Registry access is unavailable in"
  "T3|forkguard registry first instruction names registered tool specs|CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_registry_first_instruction_names_registered_tool_specs"
  "T3|forkguard mcp discovery skill conditions registry commands|CodeWhale/crates/tui/src/skills/tests.rs|fn forkguard_mcp_discovery_skill_conditions_registry_commands"
  "T3|Skill discovery attempts the direct fallback before giving up|CodeWhale/crates/tui/src/skills/mod.rs|continue without skills only if that call also fails"
  "T3|forkguard goal continuation names tool search activation|CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_goal_continuation_names_tool_search_activation"
  "T3|forkguard omitted skills line stays short|CodeWhale/crates/tui/src/skills/tests.rs|fn forkguard_omitted_skills_line_stays_short"
  "T3|forkguard bundled skills cite no hidden or retired tool names|CodeWhale/crates/tui/src/skills/system/tests.rs|fn forkguard_bundled_skills_cite_no_hidden_or_retired_tool_names"
  "T3|forkguard best of n goal tool is availability gated|CodeWhale/crates/tui/src/skills/system/tests.rs|fn forkguard_best_of_n_goal_tool_is_availability_gated"
  # Batch #112 (2026-10-20) removed the PDF skill's false built-in read claim.
  # Pin its actual bash-side pdftotext/pdfinfo/qpdf/mutool guidance; the
  # bundled-skills regression still rejects hidden or retired tool names.
  "T3|PDF guidance uses actual bash extraction tools|CodeWhale/crates/tui/assets/skills/pdf/SKILL.md|\`pdftotext\` for text extraction"
  "T3|Best-of-n gates goal creation on availability|CodeWhale/crates/tui/assets/skills/best-of-n/SKILL.md|\`create_goal\` is in your tool list"
  "T3|MCP discovery waits for pool initialization|CodeWhale/crates/tui/assets/skills/mcp-discovery/SKILL.md|the start tool once the host's MCP pool is initialized"
  "T3|MCP discovery activates the registry start tool|CodeWhale/crates/tui/assets/skills/mcp-discovery/SKILL.md|\`tool_search\` as in step 1; if that fails, registry starts"
  "T3|Best-of-n activates deferred goals through tool_search|CodeWhale/crates/tui/assets/skills/best-of-n/SKILL.md|if it is not, run \`tool_search\` first"
  "T3|Subagent result verifies side effects through visible tools|CodeWhale/crates/tui/src/core/engine/context.rs|verify side effects with \`read\` or \`bash\`"
  "T3|instruction fragment preserves explicit host budget|CodeWhale/crates/tui/src/prompts.rs|forkguard_instruction_fragment_preserves_explicit_host_budget"
  "T3|working set ignores leading system reminder paths|CodeWhale/crates/tui/src/working_set.rs|forkguard_working_set_ignores_leading_system_reminder_paths"
  "T3|Registry-first guidance activates deferred tools|CodeWhale/crates/tui/src/core/engine.rs|run \`tool_search\` first to activate it; if it cannot be surfaced"
  "T3|Registry start activation is distinct from registry sync|CodeWhale/crates/tui/src/core/engine.rs|activate it via \`tool_search\` as well, since activating \`registry_sync\` does not activate it"
  "T3|Registry sync and start name explicit discovery|CodeWhale/crates/tui/src/tools/mcp_registry.rs|run tool_search first to activate it"
  "T3|forkguard registry first prompt teaches start tool activation|CodeWhale/crates/tui/src/tools/mcp_registry.rs|fn forkguard_registry_first_prompt_teaches_start_tool_activation"
  "T3|Worker transcript retrieval names explicit discovery|CodeWhale/crates/tui/src/tools/subagent/mod.rs|if \`handle_read\` is not in your tool list, activate it via \`tool_search\` first"
  "T3|forkguard worker record hints teach handle read activation|CodeWhale/crates/tui/src/tools/subagent/tests.rs|fn forkguard_worker_record_hints_teach_handle_read_activation"
  "T3|Goal guidance attempts the direct fallback|CodeWhale/crates/tui/src/prompts/text.rs|call \`update_goal\` directly anyway"
  "T3|Context guidance names bounded transcript retrieval|CodeWhale/crates/tui/src/core/engine/context.rs|Use \`handle_read\` on \`transcript_handle\` for bounded transcript slices"
  "T3|Fetch results publish available evidence metadata|CodeWhale/crates/tui/src/tools/fetch_url.rs|\"evidence_available\": true,"
  "T3|Web results publish available evidence metadata|CodeWhale/crates/tui/src/tools/web_run.rs|\"evidence_available\": true,"
  "T3|forkguard slash agent dispatch teaches handle read activation|CodeWhale/crates/tui/src/commands/groups/core/agent.rs|fn forkguard_slash_agent_dispatch_teaches_handle_read_activation"
  "T3|Goal command discovers deferred goal creation|CodeWhale/crates/tui/src/commands/groups/project/goal.rs|if \`create_goal\` is not in your tool list"
  "T3|Goal command retains the direct creation fallback|CodeWhale/crates/tui/src/commands/groups/project/goal.rs|call \`create_goal\` directly anyway"
  "T3|Agent command names bounded transcript retrieval|CodeWhale/crates/tui/src/commands/groups/core/agent.rs|Use \`handle_read\` on a sub-agent transcript handle if you need more detail"
  "T3|forkguard phantom denylist covers canonical lists|CodeWhale/crates/tui/src/skills/system/tests.rs|fn forkguard_phantom_denylist_covers_canonical_lists"

  "T4|Automation retains a stable conversation key|CodeWhale/crates/tui/src/automation_manager.rs|add_task_with_conversation_key(new_task, Some(automation.id.clone()))"
  "T4|scheduler skips offline backfill and overlapping runs|CodeWhale/crates/tui/src/automation_manager.rs|forkguard_scheduler_skips_offline_backfill_and_overlapping_runs"
  "T4|once schedule missed while offline enqueues exactly one run|CodeWhale/crates/tui/src/automation_manager.rs|forkguard_once_schedule_missed_while_offline_enqueues_exactly_one_run"
  "T4|PINVOU LEGACY TASK SCHEMA VERSION|CodeWhale/crates/tui/src/task_manager.rs|const PINVOU_LEGACY_TASK_SCHEMA_VERSION: u32 = 4;"
  "T4|automation enqueue preserves settings and conversation owner|CodeWhale/crates/tui/src/automation_manager.rs|forkguard_automation_enqueue_preserves_settings_and_conversation_owner"
  "T4|delete terminal task|CodeWhale/crates/tui/src/task_manager.rs|pub async fn delete_terminal_task("
  "T4|terminal task delete refuses active and is idempotent|CodeWhale/crates/tui/src/task_manager.rs|forkguard_terminal_task_delete_refuses_active_and_is_idempotent"
  "T4|terminal automation run delete refuses active and is idempotent|CodeWhale/crates/tui/src/automation_manager.rs|forkguard_terminal_automation_run_delete_refuses_active_and_is_idempotent"
  "T4|Task manager emits worker thread creation|CodeWhale/crates/tui/src/task_manager.rs|ThreadCreated {"

  "T5|write session archive|CodeWhale/crates/tui/src/session_export.rs|pub fn write_session_archive("
  "T5|XZ dependency uses static liblzma|CodeWhale/crates/tui/Cargo.toml|liblzma = { version = \"0.4\", features = [\"static\"] }"
  "T5|CLI exposes session export|CodeWhale/crates/tui/src/lib.rs|Some(SessionsCommand::Export {"
  "T5|session archive export roundtrips full context|CodeWhale/crates/tui/src/session_export.rs|forkguard_session_archive_export_roundtrips_full_context"
  "T5|session archive includes artifacts and respects skip|CodeWhale/crates/tui/src/session_export.rs|forkguard_session_archive_includes_artifacts_and_respects_skip"
  "T5|session archive rejects artifact shorter than recorded size|CodeWhale/crates/tui/src/session_export.rs|forkguard_session_archive_rejects_artifact_shorter_than_recorded_size"

  "T6|DynamicGate|CodeWhale/crates/tui/src/tools/subagent/governor.rs|pub(crate) struct DynamicGate"
  "T6|RateLimitGovernor|CodeWhale/crates/tui/src/tools/subagent/governor.rs|pub(crate) struct RateLimitGovernor"
  "T6|Fleet spawn inherits the runtime governor|CodeWhale/crates/tui/src/tools/subagent/mod.rs|runtime.governor = Some(Arc::clone(&self.governor));"
  "T6|forkguard rate limit governor pauses and time recovers after window drains|CodeWhale/crates/tui/src/tools/subagent/governor.rs|fn forkguard_rate_limit_governor_pauses_and_time_recovers_after_window_drains"
  "T6|forkguard dynamic gate redispatches grant of cancelled waiter|CodeWhale/crates/tui/src/tools/subagent/governor.rs|fn forkguard_dynamic_gate_redispatches_grant_of_cancelled_waiter"

  "T7|Compaction tool boundary regression|CodeWhale/crates/tui/src/runtime_handoff.rs|fn forkguard_compaction_topology_preserves_tool_round_boundary"
  "T7|Compaction paired chat wire regression|CodeWhale/crates/tui/src/client/chat.rs|fn forkguard_compaction_tool_round_has_valid_chat_wire_roles"
  "T7|Compaction topology round regression|CodeWhale/crates/tui/src/compaction/last_round.rs|fn forkguard_mid_round_topology_is_not_a_user_turn_on_recompaction"
  "T7|Compaction later-turn survival regression|CodeWhale/crates/tui/src/compaction/last_round.rs|fn forkguard_recompaction_keeps_turns_after_the_previous_summary"
  "T7|Compaction pasted-header provenance regression|CodeWhale/crates/tui/src/client/chat.rs|fn forkguard_full_summary_header_pasted_after_tool_result_is_not_relocated"
  "T7|Restored completion boundary regression|CodeWhale/crates/tui/src/client/chat.rs|fn restored_completion_does_not_join_an_ordinary_user_turn"
  "T7|Restored pre-fix session wire regression|CodeWhale/crates/tui/src/client/chat.rs|fn forkguard_restored_pre_fix_session_has_valid_chat_wire_roles"
  "T7|Restore-tier quoted-header deletion regression|CodeWhale/crates/tui/src/compaction.rs|fn restore_keeps_a_user_turn_that_quotes_the_summary_header"
  "T7|Pre-provenance carrier restore regression|CodeWhale/crates/tui/src/compaction.rs|fn restore_replaces_a_pre_provenance_carrier"
  "T7|Checkpoint edit-target exclusion regression|CodeWhale/crates/tui/src/runtime_handoff.rs|fn compaction_checkpoint_is_never_the_edit_target"

  "T8|ROSTER HOST PROFILE LIMIT|CodeWhale/crates/tui/src/tools/subagent/mod.rs|const ROSTER_HOST_PROFILE_LIMIT: usize = 48"
  "T8|Roster discloses host profile truncation|CodeWhale/crates/tui/src/tools/subagent/mod.rs|\"host_profiles_truncated\": host_profiles_truncated,"
  "T8|roster profile query|CodeWhale/crates/tui/src/tools/subagent/mod.rs|fn roster_profile_query(input: &Value) -> Option<String>"

  "APP|Engine spawn retains the host session identity|pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|cfg.session_id = Some(session_id.to_string());"
  "APP|Host installs the exact allowed tool set|pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|allowed_tools: Some(crate::features::assistant::tool_policy::allowed_tool_names())"
  "APP|host declared output fact reaches the wire for both wire policies|pinvou3-app/src-tauri/src/features/assistant/route_wire_cap_tests.rs|fn host_declared_output_fact_reaches_the_wire_for_both_wire_policies"
  "APP|shape disallowed tools|pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|pub fn shape_disallowed_tools("
  "APP|Subagent ledger uses the host state root|pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|cfg.subagent_state_root = Some(roots.ledger);"
  "APP|Host installs per-turn tool security|pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|turn_tool_security: Some(Arc::new(turn_tool_security))"
  "APP|Host starts without unrestricted dynamic tools|pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|dynamic_tools: Vec::new()"
  "APP|Engine pool cancels subagents on session changes|pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs|Op::CancelSubAgents"
  "APP|dispatch turn bound cancel|pinvou3-app/src-tauri/src/features/assistant/engine.rs|fn dispatch_turn_bound_cancel"
  "APP|Host cancellation binds the target turn|pinvou3-app/src-tauri/src/features/assistant/engine.rs|self.cancel_turn_with_mode(turn_id, mode)"
  "APP|Unbound host stop publishes disposition only|pinvou3-app/src-tauri/src/features/assistant/engine.rs|engine.publish_stop_disposition_only(mode)"
  "APP|Engine pool dispatches cancellation from turn identity|pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs|dispatch_turn_bound_cancel(engine, identity.as_ref(), steer_mode)"
  "APP|Forwarder binds pending cancellation replay|pinvou3-app/src-tauri/src/features/assistant/forwarder.rs|approve_handle.cancel_turn("
  "APP|Overtaking self-start cannot consume pending replay|pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs|overtaking_self_started_turn_started_cannot_consume_the_replay"
  "APP|bounded while holding turn gate|pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs|async fn bounded_while_holding_turn_gate<F>"
  "APP|forkguard cancel holds turn lock boundedly|pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs|fn forkguard_cancel_holds_turn_lock_boundedly"
  "APP|bounded join while holding turn gate|pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs|async fn bounded_join_while_holding_turn_gate<T>"
  "APP|forkguard bounded join reports panicked task as not settled|pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs|fn forkguard_bounded_join_reports_panicked_task_as_not_settled"
  "APP|forkguard bounded join detaches task that outlives budget|pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs|fn forkguard_bounded_join_detaches_task_that_outlives_budget"
  "APP|forkguard reclaim shutdown sends bounded and retried|pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs|fn forkguard_reclaim_shutdown_sends_bounded_and_retried"
  "APP|forkguard reclaim shutdown retry gives up within patience|pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs|fn forkguard_reclaim_shutdown_retry_gives_up_within_patience"
  "APP|forkguard reclaim cleanup failed preset clears on success|pinvou3-app/src-tauri/src/features/assistant/turn_shell_tasks.rs|fn forkguard_reclaim_cleanup_failed_preset_clears_on_success"
  "APP|forkguard background cleanup sweep takes cleanup gate|pinvou3-app/src-tauri/src/features/assistant/turn_shell_tasks.rs|fn forkguard_background_cleanup_sweep_takes_cleanup_gate"
  "APP|bounded reserved submit send|pinvou3-app/src-tauri/src/features/assistant/engine.rs|async fn bounded_reserved_submit_send<Fut>"
  "APP|bounded join detached on timeout|pinvou3-app/src-tauri/src/features/assistant/engine.rs|async fn bounded_join_detached_on_timeout<T>"
  "APP|forkguard reserved submit send fails cleanly on wedged engine|pinvou3-app/src-tauri/src/features/assistant/engine.rs|fn forkguard_reserved_submit_send_fails_cleanly_on_wedged_engine"
  "APP|forkguard reclaimed persistence join detaches task that outlives budget|pinvou3-app/src-tauri/src/features/assistant/engine.rs|fn forkguard_reclaimed_persistence_join_detaches_task_that_outlives_budget"
  "APP|bounded teardown shutdown|pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs|async fn bounded_teardown_shutdown<S, Fut>"
  "APP|forkguard spawn failure teardown shutdown bounded and retried|pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs|fn forkguard_spawn_failure_teardown_shutdown_bounded_and_retried"
  "APP|resolve runtime route for model|pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|pub fn resolve_runtime_route_for_model("
  "APP|forkguard zai direct route survives model casing mismatch|pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|fn forkguard_zai_direct_route_survives_model_casing_mismatch"
  "APP|forkguard compaction 128k scenarios|pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|fn forkguard_compaction_128k_scenarios"
  "APP|Forwarder isolates events by owner session|pinvou3-app/src-tauri/src/features/assistant/forwarder.rs|owner_session_id == session_id"
  "APP|Scheduled executor emits thread creation|pinvou3-app/src-tauri/src/features/scheduled/executor.rs|TaskExecutionEvent::ThreadCreated"
  "APP|Scheduled tasks reuse the shared run API|pinvou3-app/src-tauri/src/features/scheduled/tasks.rs|run_now_shared(&self.automations"
  "APP|Multiagent surface reads live worker records|pinvou3-app/src-tauri/src/features/multiagent/transcripts.rs|read_persisted_agent_worker_records(workspace)"
  "APP|Runtime bundle installs the static prompt composer|pinvou3-app/src-tauri/src/features/runtime_bundle/platform/mod.rs|set_static_prompt_composer_override"
  "APP|locale bookend slim wiring, reply-language only |pinvou3-app/src-tauri/src/features/runtime_bundle/platform/mod.rs|fn forkguard_locale_bookends_never_mandate_thinking_language"
  "APP|forkguard runtime snapshot load does not repair in flight tool call|pinvou3-app/src-tauri/src/features/sessions/tests.rs|fn forkguard_runtime_snapshot_load_does_not_repair_in_flight_tool_call"
  "APP|forkguard boot repairs interrupted tool call once|pinvou3-app/src-tauri/src/features/sessions/tests.rs|fn forkguard_boot_repairs_interrupted_tool_call_once"
  "APP|History repair runs only at process startup|pinvou3-app/src-tauri/src/lib.rs|SessionStore::boot_for_process_startup()"
  "APP|install mcp secret resolver|pinvou3-app/src-tauri/src/features/marketplace/mod.rs|pub fn install_mcp_secret_resolver()"
  "APP|startup process env|pinvou3-app/src-tauri/src/lib.rs|pub(crate) fn startup_process_env()"
  "APP|Tool cards strip internal runtime suffixes|pinvou3-app/src/shared/bridge-shared-helpers.js|function stripInternalToolRuntimeSuffix("
  "APP|forkguard admitted display fallback edit cuts before trailing tool result|pinvou3-app/src-tauri/src/features/sessions/tests.rs|fn forkguard_admitted_display_fallback_edit_cuts_before_trailing_tool_result"
  "APP|forkguard admitted display fallback does not skip unsupported user turn|pinvou3-app/src-tauri/src/features/sessions/tests.rs|fn forkguard_admitted_display_fallback_does_not_skip_unsupported_user_turn"
  "APP|forkguard shell monitor assigns identical commands by stable origin|pinvou3-app/src-tauri/src/features/assistant/shell_output.rs|fn forkguard_shell_monitor_assigns_identical_commands_by_stable_origin"
  "APP|GUI export reuses base session_export |pinvou3-app/src-tauri/src/features/sessions/store.rs|deepseek_tui::session_export::write_session_archive("
  "APP|GUI export store contract regression  |pinvou3-app/src-tauri/src/features/sessions/tests.rs|fn forkguard_session_archive_export_via_store_keeps_full_context"
  # --fast runs only the fingerprint layer (CI's sole entry point), but
  # renaming a pinned test is exactly the shape it cannot see: this
  # source-level fingerprint pulls the existence of the "no tool-call cap
  # pinned test" into the sub-second layer — a rename or deletion turns red
  # immediately; whether it actually executes is still verified at compile
  # level by the layer-3 run_pinned_cap_test.
  "APP|engine config no-tool-call-cap pinned test |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|fn engine_config_has_no_tool_call_cap"
  # Cross-feature / cross-language literal contracts: the two sides share no
  # type-system link, so renaming either side alone fails silently — a code
  # probe that misses the marker would treat a native code session as a plain
  # chat and walk the wrong consent domain; a title-sentinel mismatch breaks
  # sidebar localization and first-message auto-rename. One sub-second
  # fingerprint pinned on each side.
  "APP|code-session marker, probe-side literal    |pinvou3-app/src-tauri/src/features/sessions/store.rs|const CODE_SESSION_MARKER_FILE: &str = \"code-session.json\""
  "APP|code-session marker, writer-side literal    |pinvou3-app/src-tauri/src/features/codex_acp/store.rs|join(\"code-session.json\")"
  "APP|title sentinel, Rust-side literal          |pinvou3-app/src-tauri/src/features/sessions/store.rs|const NEW_CHAT_TITLE: &str = \"新对话\""
  "APP|title sentinel, frontend dict literal      |pinvou3-app/src/shared/i18n.js|DEFAULT_CHAT_TITLES = new Set(['新对话', 'New chat', '新しいチャット'])"

  "T9|Custom route honors the config wire channel |CodeWhale/crates/config/src/route/resolver.rs|pub wire_override: Option<RequestProtocol>"
  "T9|Custom wire dialect parsed at a single source |CodeWhale/crates/config/src/provider.rs|pub fn wire_dialect_override"
  "T9|Custom wire dialect read at a single source   |CodeWhale/crates/tui/src/route_runtime.rs|pub(crate) fn custom_wire_override_for"
  "T9|Runtime candidate reads the dialect by identity |CodeWhale/crates/tui/src/route_runtime.rs|custom_wire_override_for(&route_config)"
  "T9|Runtime candidate carries the true wire         |CodeWhale/crates/tui/src/route_runtime.rs|fn forkguard_named_table_wire_responses_reaches_the_runtime_candidate"
  "T9|Unrecognized dialect degrades to Chat           |CodeWhale/crates/tui/src/route_runtime.rs|fn forkguard_named_table_unrecognized_wire_keeps_the_chat_default"
  "T9|Per-turn client pins /responses                 |CodeWhale/crates/tui/src/client.rs|fn forkguard_custom_responses_route_turn_client_posts_to_the_responses_endpoint"
  "T9|Replay tag scoped to provider identity          |CodeWhale/crates/tui/src/client.rs|fn reasoning_provider_tag"
  "T9|Ingress fails closed on non-Chat custom wire    |CodeWhale/crates/app-server/src/chat_completions.rs|wire_dialect_override(\"custom\", provider_cfg.wire.as_deref())"
  "T9|Preflight validates the table's own dialect     |CodeWhale/crates/tui/src/provider_readiness.rs|codewhale_config::provider::wire_dialect_override("
  "T9|Custom Responses encrypted-reasoning capture    |CodeWhale/crates/tui/src/client/responses/tests.rs|fn forkguard_custom_responses_stream_captures_encrypted_reasoning_as_opaque_state"
  "T9|Custom Responses exact-model replay             |CodeWhale/crates/tui/src/client/responses/tests.rs|fn forkguard_custom_responses_replays_only_exact_model_opaque_reasoning_state"
  "T9|Chat wire never captures encrypted reasoning    |CodeWhale/crates/tui/src/client/responses/tests.rs|fn forkguard_custom_chat_stream_does_not_capture_encrypted_reasoning"
  "T9|Empty encrypted content skips without breaking the stream |CodeWhale/crates/tui/src/client/responses/tests.rs|fn forkguard_custom_responses_capture_tolerates_missing_or_empty_encrypted_content"

  "T10|Tachyon effort ladder never emits high   |CodeWhale/crates/tui/src/client.rs|fn forkguard_tachyon_effort_ladder_never_emits_high"
  "T10|registered-only Tachyon table survives save |CodeWhale/crates/config/src/tests.rs|fn forkguard_tachyon_only_provider_table_survives_config_store_save"

  "APP|unified 2000-step budget default           |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|const HOST_STEP_BUDGET: u32 = 2_000;"
  "APP|2000-step budget behavior regression       |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|fn engine_config_defaults_to_unified_step_budget_and_respects_override"
)

for fp in "${fingerprints[@]}"; do
  IFS='|' read -r theme desc file pat <<<"$fp"
  if grep -qF -- "$pat" "$REPO/$file" 2>/dev/null; then
    green "  ✓ ${theme} ${desc}"
  else
    red "  ✗ ${theme} ${desc} — fingerprint missing from $file"
    fail=1
  fi
done

forkguard_count="$(grep -Rho --include='*.rs' 'forkguard_[A-Za-z0-9_]*' "$CODEWHALE/crates" 2>/dev/null | sort -u | wc -l | tr -d ' ')"
if [[ "$forkguard_count" -ge 64 ]]; then
  green "  ✓ CodeWhale retains at least 64 distinct forkguard behavior names (actual ${forkguard_count})"
else
  red "  ✗ CodeWhale has only ${forkguard_count:-0} forkguard behavior names; registered floor is 64"
  fail=1
fi

if [[ $FAST_ONLY -eq 1 ]]; then
  echo
  [[ $fail -eq 0 ]] && green "Fingerprint layer passed (--fast)" || red "Fingerprint layer failed"
  exit $fail
fi

echo
bold "── Layer 2: CodeWhale forkguard regressions ──"
( cd "$CODEWHALE" && cargo test -p codewhale-tui --lib --locked forkguard_ -- --test-threads=1 ) || fail=1
( cd "$CODEWHALE" && cargo test -p codewhale-tui --lib --locked --features benchmark-eval-controls forkguard_benchmark_ -- --test-threads=1 ) || fail=1

echo
bold "── Layer 3: pinvou3-app forkguard regressions ──"
( cd "$APP" && cargo test --lib --locked forkguard_ -- --test-threads=1 ) || fail=1
( cd "$APP" && cargo check --all-targets --features benchmark-hooks --locked ) || fail=1
( cd "$APP" && cargo test --lib --locked --features benchmark-hooks eval_send_message_op_isolated_from_gui_authority_and_installs_exact_policy -- --test-threads=1 ) || fail=1
( cd "$APP" && cargo test --lib --locked --features benchmark-hooks features::assistant::product_runtime::headless_bridge::tests -- --test-threads=1 ) || fail=1
( cd "$APP" && cargo test --lib --locked --features benchmark-hooks features::assistant::product_runtime::agentic_task::tests -- --test-threads=1 ) || fail=1
# `cargo test <filter>` exits 0 when the filter matches nothing, so a renamed
# or deleted pinned test would pass this gate silently — this very test has
# already been renamed once. `--exact` plus the "1 passed" check makes the
# absence of the test a failure instead of a no-op.
run_pinned_cap_test() {
  local output
  output=$( cd "$APP" && cargo test --lib --locked --features benchmark-hooks \
    features::assistant::platform::bridge::tests::engine_config_has_no_tool_call_cap \
    -- --exact --test-threads=1 2>&1 ) || { printf '%s\n' "$output"; return 1; }
  if ! grep -q '1 passed' <<<"$output"; then
    printf '%s\n' "$output"
    red "❌ pinned test engine_config_has_no_tool_call_cap did not run (renamed or deleted?)"
    return 1
  fi
}
run_pinned_cap_test || fail=1
( cd "$APP" && cargo test --lib --locked --features benchmark-hooks headless_bridge_contract_tests:: -- --test-threads=1 ) || fail=1

echo
if [[ $fail -eq 0 ]]; then
  green "✅ fork-guard passed: registered CodeWhale v0.9.12 r3 themes are intact."
else
  red "❌ fork-guard failed: investigate against docs/fork-modifications.md."
fi
exit $fail
