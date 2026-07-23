#!/usr/bin/env bash
# v0.9.0 clean re-fork guard:按 6 个长期主题守指纹与行为。
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TUI="$REPO/DeepSeek-TUI"
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
  "T1|library facade 存在               |DeepSeek-TUI/crates/tui/src/lib.rs|pub mod core;"
  "T1|宿主额外工具入口                   |DeepSeek-TUI/crates/tui/src/core/engine.rs|pub extra_tools: ExtraTools"

  "T2|写文件 64KB 上限                  |DeepSeek-TUI/crates/tui/src/tools/file.rs|WRITE_FILE_MAX_CONTENT_BYTES"
  "T2|截断参数修复提示                   |DeepSeek-TUI/crates/tui/src/core/engine/dispatch.rs|truncated_args_hint"
  "T2|工具黑名单结果 golden              |DeepSeek-TUI/crates/tui/src/tools/pinvou3_blocklist.rs|fn forkguard_blocklist_golden"
  "T2|deferred 激活面 golden             |DeepSeek-TUI/crates/tui/src/core/engine/tests.rs|fn forkguard_yolo_no_deferred_activator_first_class"
  "T2|disallowed_tools 前缀规则          |DeepSeek-TUI/crates/tui/src/core/engine/turn_loop.rs|rule.strip_suffix('*')"
  "T2|Dangerous 命令全模式阻断           |DeepSeek-TUI/crates/tui/src/tools/shell.rs|Dangerous commands are BLOCKED in ALL modes"
  "T2|shell 实时输出事件定义             |DeepSeek-TUI/crates/tui/src/core/events.rs|ToolCallOutput {"
  "T2|shell reader 输出回调              |DeepSeek-TUI/crates/tui/src/tools/spec.rs|pub tool_output_sink: Option<ToolOutputSink>"
  "T2|shell reader 流式 UTF-8 解码       |DeepSeek-TUI/crates/tui/src/tools/shell.rs|struct StreamingUtf8Decoder"
  "T2|shell 中文跨分片行为测试           |DeepSeek-TUI/crates/tui/src/tools/shell/tests.rs|fn forkguard_shell_live_output_preserves_utf8_across_read_boundaries"
  "T2|shell 完成前输出行为测试           |DeepSeek-TUI/crates/tui/src/tools/shell/tests.rs|fn forkguard_exec_shell_streams_output_before_completion"
  "T2|后台启动返回后输出行为测试         |DeepSeek-TUI/crates/tui/src/tools/shell/tests.rs|fn forkguard_exec_shell_background_streams_after_start_returns"
  "T2|后台 wait 完成前输出行为测试       |DeepSeek-TUI/crates/tui/src/tools/shell/tests.rs|fn forkguard_exec_shell_wait_streams_background_output_before_completion"
  "T2|Engine 完成前输出事件测试          |DeepSeek-TUI/crates/tui/src/core/engine/tests.rs|fn forkguard_engine_emits_shell_output_before_tool_completion"
  "T2|Engine 后台启动返回后输出测试      |DeepSeek-TUI/crates/tui/src/core/engine/tests.rs|fn forkguard_engine_keeps_background_shell_output_after_tool_completion"
  "T2|Engine 后台 wait 输出事件测试      |DeepSeek-TUI/crates/tui/src/core/engine/tests.rs|fn forkguard_engine_emits_background_wait_output_before_tool_completion"
  "T2|Engine 输出合并异步转发器          |DeepSeek-TUI/crates/tui/src/core/engine/tool_execution.rs|struct ToolOutputEventForwarder"
  "T2|Engine 输出拥塞无丢失测试          |DeepSeek-TUI/crates/tui/src/core/engine/tool_execution.rs|fn forkguard_tool_output_forwarder_coalesces_without_dropping_on_backpressure"

  "T3|项目上下文仅走 inline              |DeepSeek-TUI/crates/tui/src/project_context.rs|fn forkguard_pinvou3_uses_only_inline_project_context"
  "T3|密封静态 prompt composer           |DeepSeek-TUI/crates/tui/src/prompts.rs|pub fn set_static_prompt_composer_override"
  "T3|instructions 不受 4KB fragment 截断|DeepSeek-TUI/crates/tui/src/prompts.rs|fn forkguard_permissions_fragment_preserves_instructions_beyond_default_fragment_cap"
  "T3|skill 来源收敛到 bundle            |DeepSeek-TUI/crates/tui/src/skills/mod.rs|home.join(\".pinvou3\").join(\"bundle\").join(\"skills\")"
  "T3|停用 skill 不进入目录              |DeepSeek-TUI/crates/tui/src/skills/mod.rs|if is_skill_disabled(&skill.name)"
  "T3|内部提醒不污染 Working Set         |DeepSeek-TUI/crates/tui/src/working_set.rs|fn strip_leading_system_reminder(text: &str) -> &str"

  "T4|automation 透传模型                |DeepSeek-TUI/crates/tui/src/automation_manager.rs|model: automation.model.clone()"
  "T4|稳定 conversation key              |DeepSeek-TUI/crates/tui/src/task_manager.rs|pub conversation_key: Option<String>"
  "T4|task schema v4                     |DeepSeek-TUI/crates/tui/src/task_manager.rs|const CURRENT_TASK_SCHEMA_VERSION: u32 = 4;"
  "T4|小时调度稳定锚点                   |DeepSeek-TUI/crates/tui/src/automation_manager.rs|fn forkguard_hourly_rrule_without_explicit_time_keeps_creation_phase"
  "T4|漏跑跳过且同任务不重叠             |DeepSeek-TUI/crates/tui/src/automation_manager.rs|fn forkguard_scheduler_skips_offline_misfires_without_backfill"
  "T4|终态运行保留                       |DeepSeek-TUI/crates/tui/src/automation_manager.rs|terminal_run_prune_candidates"
  "T4|终态 task 级联删除                 |DeepSeek-TUI/crates/tui/src/task_manager.rs|delete_terminal_task"
  "T4|强制审批不可被 auto approve 绕过   |DeepSeek-TUI/crates/tui/src/core/engine/turn_loop.rs|registered_tool_approval_force_prompt"

  "T5|宿主工具硬白名单                   |DeepSeek-TUI/crates/tui/src/core/engine.rs|pub tool_whitelist: Option<HashSet<String>>"
  "T5|宿主额外工具覆盖全部模式           |DeepSeek-TUI/crates/tui/src/core/engine/tool_setup.rs|fn append_host_extra_tools"
  "T5|宿主额外工具全模式回归             |DeepSeek-TUI/crates/tui/src/core/engine/tests.rs|fn forkguard_host_extra_tools_register_in_all_modes"
  "T5|SpawnSubAgent 工作流契约            |DeepSeek-TUI/crates/tui/src/core/ops.rs|expects_file_output: bool"
  "T5|结构化产出提交工具                 |DeepSeek-TUI/crates/tui/src/tools/subagent/mod.rs|const SUBMIT_OUTPUT_TOOL: &str = \"submit_output\";"
  "T5|结构化产出安全路径回归             |DeepSeek-TUI/crates/tui/src/tools/subagent/tests.rs|fn forkguard_structured_output_persists_only_declared_safe_paths"
  "T5|Custom 显式写工具真实落盘          |DeepSeek-TUI/crates/tui/src/tools/subagent/tests.rs|fn forkguard_custom_explicit_write_tool_can_persist_file_without_tool_escalation"
  "T5|文件产出失败保留工具错误           |DeepSeek-TUI/crates/tui/src/tools/subagent/tests.rs|fn forkguard_missing_file_output_reports_last_tool_error"
  "T5|宿主取消全部后台 agent             |DeepSeek-TUI/crates/tui/src/core/ops.rs|CancelSubAgents"
  "T5|批量取消行为回归                   |DeepSeek-TUI/crates/tui/src/tools/subagent/tests.rs|fn forkguard_cancel_all_running_aborts_every_live_agent"
  "T5|OAuth 登录可取消                   |DeepSeek-TUI/crates/tui/src/mcp/oauth.rs|pub async fn perform_oauth_login_for_server_with_cancel"

  "T6|opaque runtime route 对宿主公开    |DeepSeek-TUI/crates/tui/src/route_runtime.rs|pub struct ResolvedRuntimeRoute"
  "T6|宿主路由解析入口                   |DeepSeek-TUI/crates/tui/src/route_runtime.rs|pub fn resolve_runtime_route("
  "T6|宿主显式 route limits 入口         |DeepSeek-TUI/crates/tui/src/route_runtime.rs|pub fn resolve_runtime_route_with_limits("
  "T6|显式 output 覆盖未知模型 4K fallback|DeepSeek-TUI/crates/tui/src/route_budget.rs|fn forkguard_explicit_route_output_limit_beats_unknown_model_name_fallback"
  "T6|automation reconcile shared API    |DeepSeek-TUI/crates/tui/src/automation_manager.rs|pub async fn reconcile_run_statuses_shared("

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
  "APP|turn 权威终态抢占门                 |pinvou3-app/src-tauri/src/features/assistant/engine.rs|pub(crate) fn claim_terminal"
  "APP|Engine 回收终态去重                 |pinvou3-app/src-tauri/src/features/assistant/engine.rs|emit_reclaimed_terminal_once"
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
bold "── 第 2 层:DeepSeek-TUI forkguard 回归 ──"
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
