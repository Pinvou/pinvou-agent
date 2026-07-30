#!/usr/bin/env bash
# v0.9.0 clean re-fork guard:按 6 个长期主题守指纹与行为。
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TUI="$REPO/CodeWhale"
APP="$REPO/pinvou3-app/src-tauri"
FAST_ONLY=0
[[ "${1:-}" == "--fast" ]] && FAST_ONLY=1

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
bold()  { printf '\033[1m%s\033[0m\n' "$*"; }

fail=0

bold "── 第 1 层:v0.9.0 主题指纹校验 ──"
# 格式:主题|说明|文件(相对仓库根)|grep -F 固定串
fingerprints=(
  "T1|library facade 存在               |CodeWhale/crates/tui/src/lib.rs|pub mod core;"
  "T1|宿主额外工具入口                   |CodeWhale/crates/tui/src/core/engine.rs|pub extra_tools: ExtraTools"

  "T2|写文件 64KB 上限                  |CodeWhale/crates/tui/src/tools/file.rs|WRITE_FILE_MAX_CONTENT_BYTES"
  "T2|append_file 内联 diff              |CodeWhale/crates/tui/src/tools/file.rs|fn forkguard_append_file_emits_inline_diff"
  "T2|截断参数修复提示                   |CodeWhale/crates/tui/src/core/engine/dispatch.rs|truncated_args_hint"
  "T2|工具黑名单结果 golden              |CodeWhale/crates/tui/src/tools/pinvou3_blocklist.rs|fn forkguard_blocklist_golden"
  "T2|deferred 激活面 golden             |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_yolo_no_deferred_activator_first_class"
  "T2|disallowed_tools 前缀规则          |CodeWhale/crates/tui/src/core/engine/turn_loop.rs|rule.strip_suffix('*')"
  "T2|Dangerous 命令全模式阻断           |CodeWhale/crates/tui/src/tools/shell.rs|Dangerous commands are BLOCKED in ALL modes"
  "T3|项目上下文仅走 inline              |CodeWhale/crates/tui/src/project_context.rs|fn forkguard_pinvou3_uses_only_inline_project_context"
  "T3|密封静态 prompt composer           |CodeWhale/crates/tui/src/prompts.rs|pub fn set_static_prompt_composer_override"
  "T3|instructions 不受 4KB fragment 截断|CodeWhale/crates/tui/src/prompts.rs|fn forkguard_permissions_fragment_preserves_instructions_beyond_default_fragment_cap"
  "T3|skill 来源收敛到 bundle            |CodeWhale/crates/tui/src/skills/mod.rs|home.join(\".pinvou3\").join(\"bundle\").join(\"skills\")"
  "T3|停用 skill 不进入目录              |CodeWhale/crates/tui/src/skills/mod.rs|if is_skill_disabled(&skill.name)"
  "T3|内部提醒不污染 Working Set         |CodeWhale/crates/tui/src/working_set.rs|fn strip_leading_system_reminder(text: &str) -> &str"

  "T4|automation 透传模型                |CodeWhale/crates/tui/src/automation_manager.rs|model: automation.model.clone()"
  "T4|稳定 conversation key              |CodeWhale/crates/tui/src/task_manager.rs|pub conversation_key: Option<String>"
  "T4|task schema v4                     |CodeWhale/crates/tui/src/task_manager.rs|const CURRENT_TASK_SCHEMA_VERSION: u32 = 4;"
  "T4|小时调度稳定锚点                   |CodeWhale/crates/tui/src/automation_manager.rs|fn forkguard_hourly_rrule_without_explicit_time_keeps_creation_phase"
  "T4|漏跑跳过且同任务不重叠             |CodeWhale/crates/tui/src/automation_manager.rs|fn forkguard_scheduler_skips_offline_misfires_without_backfill"
  "T4|终态运行保留                       |CodeWhale/crates/tui/src/automation_manager.rs|terminal_run_prune_candidates"
  "T4|终态 task 级联删除                 |CodeWhale/crates/tui/src/task_manager.rs|delete_terminal_task"
  "T4|强制审批不可被 auto approve 绕过   |CodeWhale/crates/tui/src/core/engine/turn_loop.rs|registered_tool_approval_force_prompt"

  "T5|宿主工具硬白名单                   |CodeWhale/crates/tui/src/core/engine.rs|pub tool_whitelist: Option<HashSet<String>>"
  "T5|standalone exec 配置字段完整        |CodeWhale/crates/tui/src/main.rs|extra_tools: crate::core::engine::ExtraTools::default()"
  "T5|宿主额外工具覆盖全部模式           |CodeWhale/crates/tui/src/core/engine/tool_setup.rs|fn append_host_extra_tools"
  "T5|宿主额外工具全模式回归             |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_host_extra_tools_register_in_all_modes"
  "T5|SpawnSubAgent 工作流契约            |CodeWhale/crates/tui/src/core/ops.rs|expects_file_output: bool"
  "T5|结构化产出提交工具                 |CodeWhale/crates/tui/src/tools/subagent/mod.rs|const SUBMIT_OUTPUT_TOOL: &str = \"submit_output\";"
  "T5|结构化产出安全路径回归             |CodeWhale/crates/tui/src/tools/subagent/tests.rs|fn forkguard_structured_output_persists_only_declared_safe_paths"
  "T5|Custom 显式写工具真实落盘          |CodeWhale/crates/tui/src/tools/subagent/tests.rs|fn forkguard_custom_explicit_write_tool_can_persist_file_without_tool_escalation"
  "T5|文件产出失败保留工具错误           |CodeWhale/crates/tui/src/tools/subagent/tests.rs|fn forkguard_missing_file_output_reports_last_tool_error"
  "T5|宿主取消全部后台 agent             |CodeWhale/crates/tui/src/core/ops.rs|CancelSubAgents"
  "T5|批量取消行为回归                   |CodeWhale/crates/tui/src/tools/subagent/tests.rs|fn forkguard_cancel_all_running_aborts_every_live_agent"
  "T5|OAuth 登录可取消                   |CodeWhale/crates/tui/src/mcp/oauth.rs|pub async fn perform_oauth_login_for_server_with_cancel"
  "T5|结构化用户图片输入                 |CodeWhale/crates/tui/src/core/ops.rs|pub struct UserMessageInput"
  "T5|本地图片引用持久化                 |CodeWhale/crates/tui/src/models.rs|#[serde(rename = \"local_image\")]"
  "T5|请求前物化本地图片                 |CodeWhale/crates/tui/src/vision/image_input.rs|pub async fn materialize_messages_local_images("
  "T5|turn loop 物化本地图片             |CodeWhale/crates/tui/src/core/engine/turn_loop.rs|crate::vision::image_input::materialize_messages_local_images("
  "T5|结构化图片输入端到端回归           |CodeWhale/crates/tui/src/core/engine/tests.rs|fn forkguard_structured_image_input_persists_reference_and_materializes_request"
  "T5|会话落盘无 Base64 回归             |CodeWhale/crates/tui/src/session_manager.rs|fn forkguard_saved_session_with_local_image_persists_reference_without_base64"
  "T5|视觉请求不携带 temperature        |CodeWhale/crates/tui/src/vision/tools.rs|fn forkguard_vision_payload_omits_temperature"

  "T6|opaque runtime route 对宿主公开    |CodeWhale/crates/tui/src/route_runtime.rs|pub struct ResolvedRuntimeRoute"
  "T6|宿主路由解析入口                   |CodeWhale/crates/tui/src/route_runtime.rs|pub fn resolve_runtime_route("
  "T6|宿主显式 route limits 入口         |CodeWhale/crates/tui/src/route_runtime.rs|pub fn resolve_runtime_route_with_limits("
  "T6|显式 output 覆盖未知模型 4K fallback|CodeWhale/crates/tui/src/route_budget.rs|fn forkguard_explicit_route_output_limit_beats_unknown_model_name_fallback"
  "T6|automation reconcile shared API    |CodeWhale/crates/tui/src/automation_manager.rs|pub async fn reconcile_run_statuses_shared("

  "CI|公开 fork 全目标编译门禁           |CodeWhale/.github/workflows/pinvou-fork-ci.yml|cargo check --workspace --all-targets --locked"
  "CI|父仓固定公开 CodeWhale 标签         |scripts/verify-public-submodule.sh|PINVOU_CODEWHALE_TAG=\"pinvou-v0.9.0-r2\""

  "APP|消息携带 resolved route            |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|resolve_runtime_route_for_model"
  "APP|部署级 route profile               |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|fn route_limits_for_model("
  "APP|128K/256K Compact 结果式回归       |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|fn forkguard_compaction_128k_scenarios"
  "APP|兼容引擎显式 limits 结果式回归     |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|fn forkguard_openai_compatible_route_uses_declared_limits"
  "APP|手动压缩携带同源 route             |pinvou3-app/src-tauri/src/features/assistant/engine.rs|send(Op::CompactContext {"
  "APP|定时任务使用 shared run API        |pinvou3-app/src-tauri/src/features/scheduled/tasks.rs|run_now_shared(&self.automations"
  "APP|敏感目录 hard deny 为 exit 2       |pinvou3-app/src-tauri/resources/common/bundle/deny_sensitive_paths.sh|hard-deny 必须 **exit 2**"
  "APP|静态层 composer 仍由 app 安装      |pinvou3-app/src-tauri/src/features/runtime_bundle/platform/mod.rs|set_static_prompt_composer_override"
  "APP|内置技能写入 bundle 单一来源        |pinvou3-app/src-tauri/src/features/runtime_bundle/platform/mod.rs|fn forkguard_builtin_visual_skill_uses_bundle_root_and_safe_name"
  "APP|前端终端跨分片解析状态             |pinvou3-app/src/platform/tauri/bridge/terminal.js|function terminalParserState(item, stream)"
  "APP|前端终端跨分片 UI 回归             |pinvou3-app/tests/ui_smoke.js|terminal parser preserves CRLF and ANSI state across live chunks"
  "APP|后台终态输出 tail 对账             |pinvou3-app/src/platform/tauri/bridge/terminal.js|function reconcileBackgroundTerminalOutput(previous, payload)"
  "APP|后台终态 stdout/stderr UI 回归      |pinvou3-app/tests/ui_smoke.js|background shell terminal event reconciles final stdout and stderr tails"
  "APP|session 级 ShellManager 复用        |pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs|struct SessionShellManagers"
  "APP|ShellManager 非消费式输出观察器     |pinvou3-app/src-tauri/src/features/assistant/shell_output.rs|struct ShellOutputMonitor"
  "APP|Shell 输出拥塞合并无丢失回归        |pinvou3-app/src-tauri/src/features/assistant/shell_output.rs|fn assigns_new_job_by_command_and_coalesces_all_unseen_output"
  "APP|Shell 中文跨快照边界回归            |pinvou3-app/src-tauri/src/features/assistant/shell_output.rs|fn holds_incomplete_utf8_replacement_until_a_stable_snapshot"
  "APP|Shell 后台终态持续观察回归          |pinvou3-app/src-tauri/src/features/assistant/shell_output.rs|fn reports_detached_completion_after_the_engine_tool_has_returned"
  "APP|turn 权威终态抢占门                 |pinvou3-app/src-tauri/src/features/assistant/engine.rs|pub(crate) fn claim_terminal"
  "APP|Engine 回收终态去重                 |pinvou3-app/src-tauri/src/features/assistant/engine.rs|finish_reclaimed_lifecycle_turn"
  "APP|图片输入不支持稳定错误码           |pinvou3-app/src-tauri/src/app/commands/chat.rs|const IMAGE_INPUT_UNSUPPORTED_ERROR"
  "APP|模型图片能力判定                   |pinvou3-app/src-tauri/src/features/assistant/image_capability.rs|pub fn effective_image_capability"
  "APP|ACP 附件按模型能力 gate            |pinvou3-app/src-tauri/src/features/codex_acp/mod.rs|image_capability::acp_model_image_capability("
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
bold "── 第 2 层:CodeWhale forkguard 回归 ──"
( cd "$TUI" && cargo test -p codewhale-tui forkguard_ --lib -- --test-threads=1 ) || fail=1

echo
bold "── 第 2 层:pinvou3-app forkguard 回归 ──"
( cd "$APP" && cargo test -p pinvou3-tauri forkguard_ --lib -- --test-threads=1 ) || fail=1

echo
if [[ $fail -eq 0 ]]; then
  green "✅ fork-guard 全过 — 6 个 v0.9.0 fork 主题完好。"
else
  red "❌ fork-guard 失败 — 对照 docs/fork-modifications.md 排查。"
fi
exit $fail
