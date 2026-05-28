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
fingerprints=(
  "#1  |subagent 步数上限 20            |DeepSeek-TUI/crates/tui/src/tools/subagent/mod.rs|DEFAULT_MAX_STEPS: u32 = 20"
  "#2  |subagent 墙钟上限 300s          |DeepSeek-TUI/crates/tui/src/tools/subagent/mod.rs|DEFAULT_SUBAGENT_ELAPSED_MAX: Duration = Duration::from_secs(300)"
  "#4  |resolve_agent_ref 截断容错      |DeepSeek-TUI/crates/tui/src/tools/subagent/mod.rs|LLM 可能截断"
  "#7  |tool_agent_route 继承父 model   |DeepSeek-TUI/crates/tui/src/tools/subagent/mod.rs|上游硬编码 deepseek-v4-flash"
  "#10 |web_search bing 实体解码        |DeepSeek-TUI/crates/tui/src/tools/web_search.rs|decode_html_entities"
  "#13 |fetch_url fake-ip 放行          |DeepSeek-TUI/crates/tui/src/tools/fetch_url.rs|is_trusted_fakeip_addr"
  "#18a|network_policy fake-ip CIDR     |DeepSeek-TUI/crates/tui/src/network_policy.rs|with_trusted_fakeip_cidrs"
  "#14 |file.rs 64KB content 上限       |DeepSeek-TUI/crates/tui/src/tools/file.rs|WRITE_FILE_MAX_CONTENT_BYTES"
  "#15 |truncated_args_hint 截断提示    |DeepSeek-TUI/crates/tui/src/core/engine/turn_loop.rs|truncated_args_hint"
  "    |tool_catalog blocklist 模型     |DeepSeek-TUI/crates/tui/src/core/engine/tool_catalog.rs|pinvou3_should_defer_native_tool"
  "#18b|bridge 透传 fake-ip 信任段      |pinvou3-app/src-tauri/src/bridge/mod.rs|with_trusted_fakeip_cidrs"
  "#16 |bridge subagent_api_timeout 300 |pinvou3-app/src-tauri/src/bridge/mod.rs|from_secs(300)"
  "#25 |skills union pub API            |DeepSeek-TUI/crates/tui/src/skills/mod.rs|pub fn render_available_skills_context_for_workspace_and_dir"
  "#26 |prompts skills_block union 调用 |DeepSeek-TUI/crates/tui/src/prompts.rs|render_available_skills_context_for_workspace_and_dir(workspace, dir)"
  "#28 |Tier 5 cover EngineConfig.instructions |DeepSeek-TUI/crates/tui/src/prompts/base.md|files configured via \`EngineConfig.instructions\`"
  "#33 |Output Formatting 改 embedder-aware|DeepSeek-TUI/crates/tui/src/prompts/base.md|Match the embedder's render target"
  "#32 |Sub-Agent Strategy embedder-aware  |DeepSeek-TUI/crates/tui/src/prompts/base.md|concurrent cap is embedder-configured"
  "#36 |Constitution 改 PINVOU3 brand      |DeepSeek-TUI/crates/tui/src/prompts/base.md|CONSTITUTION OF PINVOU3"
  "#36 |Brother Whale preamble 已删         |DeepSeek-TUI/crates/tui/src/prompts/base.md|running inside pinvou3"
  "#37 |LOCALE_PREAMBLE_ZH_HANS pinvou3 brand|DeepSeek-TUI/crates/tui/src/prompts.rs|你正在 pinvou3 中运行"
  "#38 |AUTHORITY_RECAP pinvou3 brand       |DeepSeek-TUI/crates/tui/src/prompts.rs|Constitution of pinvou3 (Articles I-VII)"
  "#40 |environment block 移到 volatile 下  |DeepSeek-TUI/crates/tui/src/prompts.rs|6 (was 2.25). Environment block"
  "#41 |skill 路径只剩 ~/.agents/skills    |DeepSeek-TUI/crates/tui/src/skills/mod.rs|patch #41): 砍掉底座的 10 路径扫描清单"
  "    |phase tracking 弱化(dormant)       |DeepSeek-TUI/crates/tui/src/skills/mod.rs|This section is dormant by default"
  "    |phase tracking 反指引(不要声明不适用)|DeepSeek-TUI/crates/tui/src/skills/mod.rs|Don't announce that a phased skill is"
  "    |PROJECT_CONTEXT_FILES 砍到 1 条     |DeepSeek-TUI/crates/tui/src/project_context.rs|PROJECT_CONTEXT_FILES: &[&str] = &[\".pinvou3/workspace_context.md\"]"
  "    |GLOBAL_PATHS 砍空                   |DeepSeek-TUI/crates/tui/src/project_context.rs|const GLOBAL_PATHS: &[&[&str]] = &[]"
  "    |C 方案 InstructionSource enum       |DeepSeek-TUI/crates/tui/src/prompts.rs|pub enum InstructionSource {"
  "    |C 方案 EngineConfig.instructions    |DeepSeek-TUI/crates/tui/src/core/engine.rs|pub instructions: Vec<crate::prompts::InstructionSource>"
  "    |C 方案 pinvou3 注入 Inline          |pinvou3-app/src-tauri/src/bridge/mod.rs|fn session_instructions(&self, session_id: &str) -> Vec<InstructionSource>"
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
    resolve_agent_ref_tolerates_truncated_agent_prefix \
    pinvou3_yolo_offers_nonblocklisted_tools_outside_upstream_default \
    bing_ckurl_with_html_entities_decodes_real_url \
    trusted_fakeip_cidr_allows_placeholder_but_not_real_private \
    truncated_args_hint_fires_for_file_write_missing_field \
    truncated_args_hint_skips_other_tools_and_other_errors \
    test_write_file_rejects_oversized_content \
    test_append_file_rejects_oversized_content ) || fail=1

echo
bold "── 第 2 层:fork 回归测试 (pinvou3-tauri / bridge) ──"
( cd "$APP" && cargo test -p pinvou3-tauri --lib -- \
    forkguard_ \
    engine_config_locks_critical_fields \
    default_model_window_recognized_by_engine ) || fail=1

echo
if [[ $fail -eq 0 ]]; then
  green "✅ fork-guard 全过 — 底座 fork patch 在当前基线上完好。"
else
  red   "❌ fork-guard 有失败 — 见上方。对照 docs/fork-modifications.md 排查被静默丢/改回的 patch。"
fi
exit $fail
