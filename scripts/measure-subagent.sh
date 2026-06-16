#!/usr/bin/env bash
# 可靠地测量 L1 subagent 场景成败 —— 焊死两个 cargo test 判据坑。
#
# 背景(2026-06-16 血泪,见 memory cargo_test_judge_pitfall):
#   用 shell 循环跑 cargo test + grep 判 subagent 成败时,两个坑会让你
#   把"明明 PASS"读成"躺平 0/10",追大半天不存在的 bug:
#     1. cwd 坑   : 从 repo 根/worktree 根直接 `cargo test` 会 "could not
#                   find Cargo.toml"(manifest 在 pinvou3-app/src-tauri/)。
#                   → 必须 --manifest-path 指过去。
#     2. capture 坑: cargo test 默认捕获 test 内 eprintln(L1 harness 打的
#                   `[scenario] elapsed=Xs tools={...}`),只在 FAIL 时显示,
#                   PASS 时根本不输出 → grep 'tools=' 抓不到、误判躺平。
#                   → 必须加 --nocapture。
#
# 用法: scripts/measure-subagent.sh [scenario] [N]
#   scenario 默认 subagent_single_simple;N 默认 10
#   例: scripts/measure-subagent.sh subagent_compare_3_libs 5
#
# 判定: tools 含 agent_open/agent_eval = 成功; tools={} = 躺平;
#       vLLM 不可达 = SKIP(多半是 GB10 持续大负载卡死,非躺平); 其它 = 异常
# 前置: vLLM 端点可达(DEEPSEEK_BASE_URL 或默认 10.214.74.113:8000)。
set -uo pipefail
SCENARIO="${1:-subagent_single_simple}"
N="${2:-10}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MANIFEST="$REPO_ROOT/pinvou3-app/src-tauri/Cargo.toml"
[ -f "$MANIFEST" ] || { echo "找不到 $MANIFEST(本脚本须放在 repo 的 scripts/ 下)"; exit 1; }
cd "$REPO_ROOT" || exit 1

PASS=0; LAY=0; SKIP=0; ERR=0
echo "=== $SCENARIO N=$N @ $(date +%H:%M:%S) ==="
for i in $(seq 1 "$N"); do
  OUT=$(cargo test --manifest-path "$MANIFEST" --test l1_dialog_harness "$SCENARIO" \
        -- --ignored --test-threads=1 --nocapture 2>&1)
  TLINE=$(echo "$OUT" | grep -oE "\[$SCENARIO\] elapsed=[0-9.]+s tools=\{[^}]*\} text_len=[0-9]+" | head -1)
  if echo "$OUT" | grep -q 'could not find.*Cargo.toml'; then
    ERR=$((ERR+1)); echo "run $i: 命令错(Cargo.toml — 不该出现,脚本已带 --manifest-path)"; continue
  fi
  if [ -z "$TLINE" ]; then
    if echo "$OUT" | grep -qi 'SKIP'; then SKIP=$((SKIP+1)); echo "run $i: SKIP(vLLM 不可达?)"
    else ERR=$((ERR+1)); echo "run $i: 异常(无 tools 行) [$(echo "$OUT" | grep -oE 'test result:.*' | head -1)]"; fi
    continue
  fi
  if echo "$TLINE" | grep -qE 'agent_open|agent_eval'; then
    PASS=$((PASS+1)); echo "run $i: ✓成功  $TLINE"
  else
    LAY=$((LAY+1)); echo "run $i: 躺平✗  $TLINE"
  fi
done
echo "=== 结果 $SCENARIO: 成功 $PASS / 躺平 $LAY / SKIP $SKIP / 异常 $ERR (共 $N) ==="
