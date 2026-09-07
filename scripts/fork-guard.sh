#!/usr/bin/env bash
# CodeWhale v0.9.12 clean re-fork guard: five commits, four maintained themes.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CODEWHALE="$REPO/CodeWhale"
APP="$REPO/pinvou3-app/src-tauri"
EXPECTED_UPSTREAM="dcd4c200f72f0c1ffd60d8e7f6850313db879fc5"
EXPECTED_HEAD="b4c02616b8561dfca43d540fe778bb15287fa719"
EXPECTED_COMMITS=5
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

bold "── 第 0 层：v0.9.12 r1 clean re-fork 拓扑 ──"
actual_head="$(git -C "$CODEWHALE" rev-parse HEAD 2>/dev/null || true)"
if [[ "$actual_head" == "$EXPECTED_HEAD" ]]; then
  green "  ✓ CodeWhale gitlink 指向 v0.9.12 r1 候选 $EXPECTED_HEAD"
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
  "T1|换会话批量取消子智能体              |CodeWhale/crates/tui/src/core/ops.rs|CancelSubAgents"
  "T1|宿主 prompt-only profile 边界       |CodeWhale/crates/tui/src/tools/subagent/tests.rs|forkguard_host_profile_overlay_is_config_only_and_prompt_only"

  "T2|宿主额外工具入口                  |CodeWhale/crates/tui/src/core/engine.rs|pub struct ExtraTools("
  "T2|宿主 MCP secret resolver          |CodeWhale/crates/tui/src/mcp.rs|pub fn install_mcp_secret_resolver("
  "T2|File 写入 64 KiB 硬上限           |CodeWhale/crates/tui/src/tools/file.rs|const WRITE_FILE_MAX_CONTENT_BYTES: usize = 64 * 1024;"
  "T2|逐轮安全策略                      |CodeWhale/crates/tui/src/core/ops.rs|pub struct TurnToolSecurityPolicy"
  "T2|精确最终分发 fail-closed          |CodeWhale/crates/tui/src/core/engine/tool_execution.rs|forkguard_exact_dispatch_rejects_forged_backends"
  "T2|只读最终分发 fail-closed          |CodeWhale/crates/tui/src/core/engine/tool_execution.rs|forkguard_read_only_turn_rejects_write_at_final_dispatch"
  "T2|受限审计固定脱敏                  |CodeWhale/crates/tui/src/core/engine/tool_execution.rs|forkguard_restricted_tool_audit_redacts_private_payload"
  "T2|排队控制面不绕过逐轮权限            |CodeWhale/crates/tui/src/core/engine/tests.rs|forkguard_queued_control_op_keeps_restricted_turn_authority"
  "T2|宿主 Shell owner+session 入口      |CodeWhale/crates/tui/src/tools/shell.rs|pub fn execute_with_options_env_for_owner_and_session("

  "T3|静态 prompt composer              |CodeWhale/crates/tui/src/prompts.rs|pub fn set_static_prompt_composer_override("
  "T3|ambient project authority 密封     |CodeWhale/crates/tui/src/project_context.rs|forkguard_runtime_loader_ignores_ambient_project_authority"
  "T3|显式 Skills 根排除 ambient 来源    |CodeWhale/crates/tui/src/skills/tests.rs|forkguard_explicit_skills_dir_excludes_ambient_workspace_sources"
  "T3|Permissions 窄 100 KiB 预算       |CodeWhale/crates/tui/src/prompts.rs|forkguard_instruction_fragment_preserves_explicit_host_budget"
  "T3|内部 reminder 不污染 working set  |CodeWhale/crates/tui/src/working_set.rs|forkguard_working_set_ignores_leading_system_reminder_paths"

  "T4|Automation 稳定 conversation key |CodeWhale/crates/tui/src/automation_manager.rs|add_task_with_conversation_key(new_task, Some(automation.id.clone()))"
  "T4|离线不补跑且同一任务不重叠          |CodeWhale/crates/tui/src/automation_manager.rs|forkguard_scheduler_skips_offline_backfill_and_overlapping_runs"
  "T4|Pinvou 历史 v4 schema 窄兼容       |CodeWhale/crates/tui/src/task_manager.rs|const PINVOU_LEGACY_TASK_SCHEMA_VERSION: u32 = 4;"
  "T4|conversation owner 与任务参数持久  |CodeWhale/crates/tui/src/automation_manager.rs|forkguard_automation_enqueue_preserves_settings_and_conversation_owner"
  "T4|终态任务可显式清理                  |CodeWhale/crates/tui/src/task_manager.rs|pub async fn delete_terminal_task("
  "T4|worker 建线程边界事件              |CodeWhale/crates/tui/src/task_manager.rs|ThreadCreated {"

  "APP|spawn 前安装 Engine session id   |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|cfg.session_id = Some(session_id.to_string());"
  "APP|逐轮精确安全策略下发               |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|turn_tool_security: Some(Arc::new(turn_tool_security))"
  "APP|受限操作动态工具清空               |pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs|dynamic_tools: Vec::new()"
  "APP|停止与回收级联取消子智能体          |pinvou3-app/src-tauri/src/features/assistant/engine_pool.rs|Op::CancelSubAgents"
  "APP|事件按 owner session 隔离         |pinvou3-app/src-tauri/src/features/assistant/forwarder.rs|owner_session_id == session_id"
  "APP|定时任务报告 ThreadCreated        |pinvou3-app/src-tauri/src/features/scheduled/executor.rs|TaskExecutionEvent::ThreadCreated"
  "APP|静态 prompt composer 由应用安装   |pinvou3-app/src-tauri/src/features/runtime_bundle/platform/mod.rs|set_static_prompt_composer_override"
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
if [[ "$forkguard_count" -ge 18 ]]; then
  green "  ✓ CodeWhale 至少保留 18 条独立 forkguard 行为名（实际 $forkguard_count）"
else
  red "  ✗ CodeWhale forkguard 行为名仅 ${forkguard_count:-0}，登记下限为 18"
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

echo
bold "── 第 3 层：pinvou3-app forkguard 回归 ──"
( cd "$APP" && cargo test --lib --locked forkguard_ -- --test-threads=1 ) || fail=1

echo
if [[ $fail -eq 0 ]]; then
  green "✅ fork-guard 全过：CodeWhale v0.9.12 r1 的 4 个 Pinvou 主题完好。"
else
  red "❌ fork-guard 失败：请对照 docs/fork-modifications.md 排查。"
fi
exit $fail
