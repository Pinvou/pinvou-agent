#!/usr/bin/env bash
# fork-guard.sh — 上游 sync 后自动校验 pinvou3 对 DeepSeek-TUI 底座的 fork patch。
#
# 用法:  ./scripts/fork-guard.sh            # 全量(指纹 + 编译跑测试)
#        ./scripts/fork-guard.sh --fast      # 只跑指纹层(秒级,不编译)
#
# 两层防护:
#   1. 指纹层 — grep 每个 fork 标记是否还在(抓 merge 静默丢整段 patch),秒级。
#   2. 行为层 — cargo test 跑精选 fork 回归测试(抓「值/行为被改回上游」),需编译。
#
# 注:L1 vLLM dialog harness 慢且需后端,不在此脚本内;按需单独跑。
# 维护:新增 fork patch 时,在此同步加一条指纹 + 一个 forkguard_ 前缀测试,
#       并更新 docs/fork-modifications.md。
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

# ---------- 第 1 层:指纹(代码是否还在) ----------
bold "── 第 1 层:fork patch 指纹校验 ──"
# 格式: "编号|说明|文件(相对 REPO)|grep -F 固定串"
# 2026-06-04 clean re-fork(pinvou3-clean ← v0.8.53):已删 subagent 全套 / phase-demo /
# qwen-128K / fetch_url 残留测试;已 harvest 的(bing decode / network_policy fake-ip /
# InstructionSource / override hook / EngineConfig.instructions / 256K compact)指纹一并撤除
# —— 它们已是上游自带,不再是 fork-distinct patch。下面只保留仍 fork-distinct 的 patch。
fingerprints=(
  # —— submodule fork patch ——
  "#14 |file.rs 64KB content 上限       |DeepSeek-TUI/crates/tui/src/tools/file.rs|WRITE_FILE_MAX_CONTENT_BYTES"
  "#15 |truncated_args_hint 截断提示    |DeepSeek-TUI/crates/tui/src/core/engine/dispatch.rs|truncated_args_hint"
  "    |tool_catalog blocklist 模型     |DeepSeek-TUI/crates/tui/src/core/engine/tool_catalog.rs|pinvou3_should_defer_native_tool"
  "    |pinvou3_blocklist 工具表        |DeepSeek-TUI/crates/tui/src/tools/pinvou3_blocklist.rs|fn is_pinvou3_hidden"
  "    |tool_search 注入受 blocklist gate|DeepSeek-TUI/crates/tui/src/core/engine/tool_catalog.rs|is_pinvou3_hidden(TOOL_SEARCH_NAME)"
  # 2026-07-03:v0.8.65 上游把 tool_search 折叠成**单名**,门控 TOOL_SEARCH_NAME=\"tool_search\" 依赖
  # blocklist 含**裸单名**。原指纹查废弃双旧名 tool_search_tool_regex(空防、恒在),bug 时照样命中→
  # 没抓住漏注入。改查裸单名;真正行为守护靠 forkguard_tool_search_not_injected 测试(已修断言)。
  "    |tool_search 裸单名进 blocklist   |DeepSeek-TUI/crates/tui/src/tools/pinvou3_blocklist.rs|\"tool_search\","
  # 2026-07-03 工具表 golden 守护(结果式,堵 sync 改名/新增/折叠漂移;验收清单 3.2/3.4)
  "    |golden:blocklist 精确名单        |DeepSeek-TUI/crates/tui/src/tools/pinvou3_blocklist.rs|fn forkguard_blocklist_golden"
  "    |golden:注入层 active snapshot    |DeepSeek-TUI/crates/tui/src/core/engine/tests.rs|fn forkguard_yolo_no_deferred_activator_first_class"
  # 2026-07-03 auto-compact 根治:pinvou3 直接调底座 budget(不再镜像 500K/262144/公式→静默倒置)
  "    |compact:底座 budget 函数 depub   |DeepSeek-TUI/crates/tui/src/core/engine.rs|pub use context::context_input_budget_for_route"
  "    |compact:derive 调底座 budget     |pinvou3-app/src-tauri/src/bridge/mod.rs|context_input_budget_for_route("
  "    |compact:跨仓不倒置守护测试       |pinvou3-app/src-tauri/src/bridge/mod.rs|fn forkguard_compaction_threshold_below_emergency_all_windows"
  # C4-a「多行逐行取最严」指纹于 v0.8.57 撤除:上游 18df8db0 extract neutral command support
  # 自带 split_command_segments+analyze_destructive_patterns,已取代,fork 块已删。
  "    |careful shell YOLO 也 BLOCK     |DeepSeek-TUI/crates/tui/src/tools/shell.rs|Dangerous commands are BLOCKED in ALL modes"
  "#25 |skills union pub API            |DeepSeek-TUI/crates/tui/src/skills/mod.rs|pub fn render_available_skills_context_for_workspace_and_dir"
  # v0.8.65 集成:#41 收窄到只 ~/.pinvou3/bundle/skills(2026-06-29 决策,去 .agents/skills);
  # skill 市场停用开关(MKT)取 origin/main 更全的 3 条指纹。
  "#26 |prompts skills_block union 调用 |DeepSeek-TUI/crates/tui/src/prompts.rs|render_available_skills_context_for_workspace_and_dir_with_mode("
  "#41 |skill 路径只剩 ~/.pinvou3/bundle/skills|DeepSeek-TUI/crates/tui/src/skills/mod.rs|home.join(\".pinvou3\").join(\"bundle\").join(\"skills\")"
  "MKT |skill 停用过滤器 setter         |DeepSeek-TUI/crates/tui/src/skills/mod.rs|pub fn set_disabled_skills"
  "MKT |render 跳过停用 skill           |DeepSeek-TUI/crates/tui/src/skills/mod.rs|if is_skill_disabled(&skill.name)"
  "MKT |load_skill 停用即 not-found     |DeepSeek-TUI/crates/tui/src/tools/skill.rs|crate::skills::is_skill_disabled(name)"
  "    |PROJECT_CONTEXT_FILES 砍空(C 终态)  |DeepSeek-TUI/crates/tui/src/project_context.rs|PROJECT_CONTEXT_FILES: &[&str] = &[]"
  "    |GLOBAL_PATHS 砍空                   |DeepSeek-TUI/crates/tui/src/project_context.rs|const GLOBAL_PATHS: &[&[&str]] = &[]"
  "53  |constitution.json loader 短路       |DeepSeek-TUI/crates/tui/src/project_context.rs|v0.8.53 上游引入 \`.codewhale/constitution.json\`"
  "    |generate_ephemeral_context 砍空(C5) |DeepSeek-TUI/crates/tui/src/project_context.rs|[pinvou3-fork C5] 砍空返 None"
  "#42 |static composer hook(密封静态层)   |DeepSeek-TUI/crates/tui/src/prompts.rs|pub fn set_static_prompt_composer_override"
  "#42 |ContextMgmt/COMPACT 受 composer gate|DeepSeek-TUI/crates/tui/src/prompts.rs|static_prompt_composer().is_none()"
  "#42 |Runtime Policy Ref 受 composer gate |DeepSeek-TUI/crates/tui/src/prompts.rs|Policy Reference(agent/plan/yolo"
  "#42 |runtime_prompt tag 受 composer gate |DeepSeek-TUI/crates/tui/src/core/engine/turn_loop.rs|static_prompt_composer_installed()"
  # —— P(pwd/workspace 移出静态 system → per-turn turn_meta)指纹已撤(2026-06-29 v0.8.65)——
  #    上游 v0.8.65 已 harvest 该优化(render_environment_block 不再输出 pwd + turn_meta 带
  #    workspace),不再 fork-distinct;详见 fork-modifications §2.2。
  # —— 会话工具开关(2026-06-23):pinvou3 connector 开关广播 disallowed_tools 给引擎(fork #4)——
  # 引擎加 Op::SetDisallowedTools → 写 config.disallowed_tools → 下一轮 filter_tool_catalog_for_gates 隐藏。
  "C8  |SetDisallowedTools op 定义       |DeepSeek-TUI/crates/tui/src/core/ops.rs|SetDisallowedTools { tools: Vec<String> }"
  "C8  |SetDisallowedTools 写 disallowed |DeepSeek-TUI/crates/tui/src/core/engine.rs|Op::SetDisallowedTools { tools }"
  # —— C9(2026-06-30,fork #5):disallowed_tools 规则支持 `*` 后缀前缀通配,禁掉远程 MCP 动态工具 ——
  "C9  |command_denies_tool 前缀通配    |DeepSeek-TUI/crates/tui/src/core/engine/turn_loop.rs|rule.strip_suffix('*')"
  # —— C10(2026-06-30,fork #6):MCP env placeholder + Windows 子进程后台控制台抑制 ——
  "C10 |MCP env placeholder 解析        |DeepSeek-TUI/crates/tui/src/mcp.rs|fn expand_env_placeholders(value: &str) -> Result<String>"
  "C10 |MCP env placeholder 回归        |DeepSeek-TUI/crates/tui/src/mcp/tests.rs|PINVOU3_MCP_SECRET_QCC_API_KEY"
  "C10 |Windows 子进程无控制台 helper   |DeepSeek-TUI/crates/tui/src/utils.rs|pub(crate) fn suppress_tokio_console_window"
  "C10 |MCP 启动应用无控制台 helper     |DeepSeek-TUI/crates/tui/src/mcp.rs|suppress_tokio_console_window(&mut cmd)"
  # —— C11(2026-07-07,fork #7):Windows killed background shell 不 join 可能阻塞的 reader ——
  "C11 |Windows killed shell reader 不阻塞|DeepSeek-TUI/crates/tui/src/tools/shell.rs|if matches!(self.status, ShellStatus::Killed)"
  "C12 |shell 实时输出事件定义          |DeepSeek-TUI/crates/tui/src/core/events.rs|ToolCallOutput {"
  "C12 |shell reader 输出回调           |DeepSeek-TUI/crates/tui/src/tools/spec.rs|pub tool_output_sink: Option<ToolOutputSink>"
  "C12 |shell reader 流式 UTF-8 解码    |DeepSeek-TUI/crates/tui/src/tools/shell.rs|struct StreamingUtf8Decoder"
  "C12 |shell 中文跨分片行为测试        |DeepSeek-TUI/crates/tui/src/tools/shell/tests.rs|fn forkguard_shell_live_output_preserves_utf8_across_read_boundaries"
  "C12 |shell 完成前输出行为测试         |DeepSeek-TUI/crates/tui/src/tools/shell/tests.rs|fn forkguard_exec_shell_streams_output_before_completion"
  "C12 |后台启动返回后输出行为测试       |DeepSeek-TUI/crates/tui/src/tools/shell/tests.rs|fn forkguard_exec_shell_background_streams_after_start_returns"
  "C12 |后台 wait 完成前输出行为测试     |DeepSeek-TUI/crates/tui/src/tools/shell/tests.rs|fn forkguard_exec_shell_wait_streams_background_output_before_completion"
  "C12 |Engine 完成前输出事件测试        |DeepSeek-TUI/crates/tui/src/core/engine/tests.rs|fn forkguard_engine_emits_shell_output_before_tool_completion"
  "C12 |Engine 后台启动返回后输出测试    |DeepSeek-TUI/crates/tui/src/core/engine/tests.rs|fn forkguard_engine_keeps_background_shell_output_after_tool_completion"
  "C12 |Engine 后台 wait 输出事件测试    |DeepSeek-TUI/crates/tui/src/core/engine/tests.rs|fn forkguard_engine_emits_background_wait_output_before_tool_completion"
  "C12 |Engine 可控实时输出测试工具      |DeepSeek-TUI/crates/tui/src/core/engine/tests.rs|struct ControlledStreamingTool"
  # —— P1(2026-07-03):list_mcp_resources/templates 按对应集合非空 gate(上游原为 servers 非空即注入)——
  # pinvou3 MCP server 全 tools-only,原条件下这两个元工具永久空转;改按 resources/templates 非空注入。可上游。
  "P1  |list_mcp_resources 按 resources 非空 gate|DeepSeek-TUI/crates/tui/src/mcp.rs|if !self.all_resource_templates().is_empty()"
  # AUTO(2026-07-09):automation MINUTELY RRULE support for PINVOU scheduled tasks.
  "AUTO|automation MINUTELY schedule variant |DeepSeek-TUI/crates/tui/src/automation_manager.rs|Minutely {"
  "AUTO|automation MINUTELY forkguard test  |DeepSeek-TUI/crates/tui/src/automation_manager.rs|fn forkguard_parses_minutely_rrule"
  "AUTO|automation tool advertises MINUTELY |DeepSeek-TUI/crates/tui/src/tools/automation.rs|FREQ=MINUTELY;INTERVAL=N"
  "AUTO|host executor immutable task getters |DeepSeek-TUI/crates/tui/src/task_manager.rs|pub fn workspace(&self) -> &Path"
  "AUTO|host executor pre-turn thread link   |DeepSeek-TUI/crates/tui/src/task_manager.rs|ThreadCreated {"
  "AUTO|automation propagates selected model |DeepSeek-TUI/crates/tui/src/automation_manager.rs|model: automation.model.clone()"
  "AUTO|automation skips stale slot backlog  |DeepSeek-TUI/crates/tui/src/automation_manager.rs|latest_due_at_or_before"
  "AUTO|MINUTELY normalizes legacy cursor     |DeepSeek-TUI/crates/tui/src/automation_manager.rs|fn normalize_due_cursor"
  "AUTO|task prune protects run/pending owners|DeepSeek-TUI/crates/tui/src/automation_manager.rs|pub fn protected_task_ids"
  "AUTO|running run link triggers persistence|DeepSeek-TUI/crates/tui/src/automation_manager.rs|run.thread_id != task.thread_id"
  "AUTO|run index avoids retained-history scan|DeepSeek-TUI/crates/tui/src/automation_manager.rs|fn retention_guard_does_not_parse_retained_history_on_nonterminal_save"
  "AUTO|retention reads only prune candidates |DeepSeek-TUI/crates/tui/src/automation_manager.rs|fn terminal_retention_reads_only_prune_candidates"
  "AUTO|journaled enqueue recovery failpoint  |DeepSeek-TUI/crates/tui/src/automation_manager.rs|fn manual_run_recovers_journaled_enqueue"
  "AUTO|Running state durable before execute  |DeepSeek-TUI/crates/tui/src/task_manager.rs|fn executor_never_starts_before_running_record_is_durable"
  "AUTO|terminal artifact retry is durable    |DeepSeek-TUI/crates/tui/src/task_manager.rs|fn terminal_artifact_write_failure_retries_without_publishing_terminal_state"
  "AUTO|report failure cancels executor token |DeepSeek-TUI/crates/tui/src/task_manager.rs|fn reporter_failure_cancels_token"
  "AUTO|bad persisted mode isolated           |DeepSeek-TUI/crates/tui/src/task_manager.rs|fn invalid_mode_isolated_retaining_idempotency"
  "AUTO|terminal task prune is crash durable  |DeepSeek-TUI/crates/tui/src/task_manager.rs|pub async fn prune_terminal_tasks"
  "AUTO|persisted task id matches safe stem   |DeepSeek-TUI/crates/tui/src/task_manager.rs|fn load_state_rejects_unsafe_mismatched_and_duplicate_task_ids"
  "AUTO|non-bypassable approval carries force |DeepSeek-TUI/crates/tui/src/core/engine/tests.rs|fn yolo_hook_ask_emits_non_bypassable_force_prompt"
  # —— 工作流 fork 基座层(三省六部;feat/sansheng-workflow 随附,2026-06-12 补)——
  # 行为层已有 engine_config_locks_critical_fields(W10 reasoning_effort);其余 W* 暂只 L1。
  "W1  |SpawnSubAgent 扩展字段          |DeepSeek-TUI/crates/tui/src/core/ops.rs|expects_file_output: bool"
  "W2  |StructuredOutput 催交重试上限   |DeepSeek-TUI/crates/tui/src/tools/subagent/mod.rs|const MAX_STRUCTURED_OUTPUT_RETRIES: u32 = 3;"
  "W3  |submit_output 合成工具名        |DeepSeek-TUI/crates/tui/src/tools/subagent/mod.rs|const SUBMIT_OUTPUT_TOOL: &str = \"submit_output\";"
  "W4  |request_user_input 路由通道     |DeepSeek-TUI/crates/tui/src/tools/subagent/mod.rs|pub user_input_tx: Option<broadcast::Sender<UserInputDecision>>"
  "W5  |AgentComplete failed 信封       |DeepSeek-TUI/crates/tui/src/core/events.rs|[pinvou3-fork] True when sub-agent execution ended via error rather"
  "W6  |SubAgent Mailbox 信封           |DeepSeek-TUI/crates/tui/src/tools/subagent/mailbox.rs|pub struct MailboxEnvelope {"
  "W7  |SubAgent 贪心解码 temp=0        |DeepSeek-TUI/crates/tui/src/tools/subagent/mod.rs|const SUBAGENT_TEMPERATURE: f32 = 0.0;"
  "W8  |SubAgent web/custom 工具面      |DeepSeek-TUI/crates/tui/src/tools/subagent/mod.rs|with_full_agent_surface("
  # W9 read_pdf catch_unwind 于 v0.8.60 sync 被上游 harvest(guard_pdf_extract,
  # file.rs:447 同语义 catch_unwind 辅助函数;char-boundary 部分也已是上游自带)→ 撤指纹。
  "W10 |reasoning_effort 会话初始注入   |DeepSeek-TUI/crates/tui/src/core/engine.rs|session.reasoning_effort = config.reasoning_effort"
  "W11 |submit_output 成功即 break 收工 |DeepSeek-TUI/crates/tui/src/tools/subagent/mod.rs|if output_schema.is_some() && output_submitted.is_some()"
  "W12 |registry max_steps per-spawn 生效|DeepSeek-TUI/crates/tui/src/tools/subagent/mod.rs|options.max_steps.unwrap_or(self.max_steps)"
  # —— Agentic RAG: EngineConfig.extra_tools 应用层工具注入口(2026-06-24)——
  # 通用扩展点(可上游 PR):app 注入 kb_search 等 ToolSpec,无需 fork 工具表。丢了 → app
  # 的 kb_search 工具静默不进 registry,Agentic RAG 整条失效却不报错。
  "RAG1|EngineConfig.extra_tools 字段     |DeepSeek-TUI/crates/tui/src/core/engine.rs|pub extra_tools: ExtraTools"
  "RAG2|tool_setup 注册 extra_tools       |DeepSeek-TUI/crates/tui/src/core/engine/tool_setup.rs|&self.config.extra_tools.0"
  # —— app 层 fork(pinvou3-app)——
  "#18b|bridge 透传 fake-ip 信任段      |pinvou3-app/src-tauri/src/bridge/mod.rs|with_trusted_fakeip_cidrs"
  "#16 |bridge subagent_api_timeout 300 |pinvou3-app/src-tauri/src/bridge/mod.rs|from_secs(300)"
  # main #14 prompt 单一来源重构:宪法/裁决/Authority 从 base.md 折叠进 instructions.md,
  # compose 丢弃 base(静态层只剩 Mode)→ 原 base.md 的 3 个指纹失效,改指向新落点。
  "#36 |宪法核心折叠进 instructions(单一来源)|pinvou3-app/src-tauri/resources/bundle/instructions.md|权威顺序"
  "#43 |compose 丢弃 base,静态层只剩 Mode   |pinvou3-app/src-tauri/src/bridge/bundle.rs|静态层只剩 Mode"
  "    |敏感目录 deny hook hard-deny exit 2 |pinvou3-app/src-tauri/resources/bundle/deny_sensitive_paths.sh|hard-deny 必须 **exit 2**"
  "#37 |LOCALE_PREAMBLE_ZH_HANS 短版        |pinvou3-app/src-tauri/src/bridge/bundle.rs|pinvou3 界面语言为简体中文"
  "#38 |AUTHORITY_RECAP 清空(已折叠 instr)  |pinvou3-app/src-tauri/src/bridge/bundle.rs|Authority Recap（Final Reminder）清空"
  "#45 |instructions 动态注入 model 名      |pinvou3-app/src-tauri/src/bridge/mod.rs|{{PINVOU3_MODEL}}"
  "    |C 方案 pinvou3 注入 Inline          |pinvou3-app/src-tauri/src/bridge/mod.rs|fn session_instructions(&self, session_id: &str) -> Vec<InstructionSource>"
  "#42 |app 装静态层 composer               |pinvou3-app/src-tauri/src/bridge/bundle.rs|set_static_prompt_composer_override"
  "#42 |pinvou3 Mode/compact 静态层文案     |pinvou3-app/src-tauri/src/bridge/bundle.rs|pub fn compose_static_layers"
  "#42 |LOCALE_PREAMBLE_JA 短版             |pinvou3-app/src-tauri/src/bridge/bundle.rs|pinvou3 の UI 言語は日本語です"
)
for fp in "${fingerprints[@]}"; do
  IFS='|' read -r id desc file pat <<<"$fp"
  if grep -qF -- "$pat" "$REPO/$file" 2>/dev/null; then
    green "  ✓ ${id}${desc}"
  else
    red   "  ✗ ${id}${desc}  — 指纹消失于 $file (疑似 merge 静默丢失)"
    fail=1
  fi
done

if [[ $FAST_ONLY -eq 1 ]]; then
  echo
  [[ $fail -eq 0 ]] && green "指纹层全过 (--fast,未跑测试)" || red "指纹层有缺失,见上"
  exit $fail
fi

# ---------- 第 2 层:行为(值/逻辑是否被改回上游) ----------
echo
bold "── 第 2 层:fork 回归测试 (codewhale-tui) ──"
# libtest 多 filter = OR。forkguard_ 前缀网住所有新增守卫;其余按名列出。
( cd "$TUI" && cargo test -p codewhale-tui --lib -- \
    forkguard_ \
    pinvou3_yolo_offers_nonblocklisted_tools_outside_upstream_default \
    truncated_args_hint_fires_for_file_write_missing_field \
    truncated_args_hint_skips_other_tools_and_other_errors \
    test_write_file_rejects_oversized_content \
    test_append_file_rejects_oversized_content \
    disallowed_tools_gate_blocks_prefix_wildcard \
    retention_guard_does_not_parse_retained_history_on_nonterminal_save \
    terminal_retention_reads_only_prune_candidates \
    manual_run_recovers_journaled_enqueue \
    minutely_fast_forward_normalizes_legacy_cursor_at_end_of_minute \
    invalid_pending_journal_blocks_all_task_pruning \
    prune_terminal_tasks_preserves_protected_and_active_tasks \
    startup_finishes_journaled_task_prune \
    load_state_rejects_unsafe_mismatched_and_duplicate_task_ids \
    executor_never_starts_before_running_record_is_durable \
    terminal_artifact_write_failure_retries_without_publishing_terminal_state \
    reporter_failure_cancels_token \
    invalid_mode_isolated_retaining_idempotency \
    yolo_hook_ask_emits_non_bypassable_force_prompt ) || fail=1

echo
bold "── 第 2 层:fork 回归测试 (pinvou3-tauri / bridge) ──"
( cd "$APP" && cargo test -p pinvou3-tauri --lib -- \
    forkguard_ \
    engine_config_locks_critical_fields \
    default_model_window_recognized_by_engine \
    search_prefs_default_is_bing_no_key \
    search_prefs_roundtrip_with_metaso_key \
    search_prefs_partial_json_fills_defaults ) || fail=1

echo
if [[ $fail -eq 0 ]]; then
  green "✅ fork-guard 全过 — 底座 fork patch 在当前基线上完好。"
else
  red   "❌ fork-guard 有失败 — 见上方。对照 docs/fork-modifications.md 排查被静默丢/改回的 patch。"
fi
exit $fail
