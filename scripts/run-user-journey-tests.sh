#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PYTHONIOENCODING="${PYTHONIOENCODING:-utf-8}"
export PYTHONUTF8="${PYTHONUTF8:-1}"
if [ -z "${CHROME:-}" ]; then
  CHROME="$(node -e "const fs=require('fs');const p=['C:\\\\Program Files\\\\Google\\\\Chrome\\\\Application\\\\chrome.exe','C:\\\\Program Files (x86)\\\\Google\\\\Chrome\\\\Application\\\\chrome.exe','C:\\\\Program Files\\\\Microsoft\\\\Edge\\\\Application\\\\msedge.exe','C:\\\\Program Files (x86)\\\\Microsoft\\\\Edge\\\\Application\\\\msedge.exe','/snap/bin/chromium','/usr/bin/chromium','/usr/bin/chromium-browser','/usr/bin/google-chrome','/usr/bin/google-chrome-stable'].find(x=>fs.existsSync(x));if(p)process.stdout.write(p);" 2>/dev/null || true)"
  if [ -n "$CHROME" ]; then export CHROME; fi
fi

run_required() {
  echo "== $* =="
  (cd "$ROOT" && "$@")
}

# The optional runners are multi-minute browser suites, so their output stays on
# the terminal and is tee'd to a scratch file for the marker check below, rather
# than captured into a variable that would show nothing until the runner exits.
# Cleanup runs from a trap because an interrupted suite is both the case this
# helper exists for and the one path that never reaches the end of the function.
journey_log=""
cleanup_journey_log() {
  if [ -n "$journey_log" ]; then
    rm -f -- "$journey_log"
    journey_log=""
  fi
}
trap cleanup_journey_log EXIT
trap 'cleanup_journey_log; trap - INT; kill -INT $$' INT
trap 'cleanup_journey_log; exit 143' TERM

# Exit code 2 counts as "skipped" only when the runner also printed its explicit
# SKIP: marker, so a real exit-2 failure is no longer swallowed. Both PIPESTATUS
# entries are needed, which forces a single-assignment snapshot: any simple
# command in between, an assignment included, already replaces the array.
run_optional_skip2() {
  echo "== $* =="
  local status=(0 0) rc tee_rc
  journey_log="$(mktemp)"
  (cd "$ROOT" && "$@" 2>&1) | tee "$journey_log" || status=("${PIPESTATUS[@]}")
  rc=${status[0]}
  tee_rc=${status[1]}
  if [ "$tee_rc" -ne 0 ]; then
    echo "warning: output capture failed, cannot honour a SKIP: marker for: $*" >&2
  elif [ "$rc" -eq 2 ] && grep -q '^SKIP:' "$journey_log"; then
    cleanup_journey_log
    echo "SKIP: optional dependency missing for: $*"
    return 0
  fi
  cleanup_journey_log
  return "$rc"
}

# Sourcing this file yields the helpers above without running the suite, so the
# marker-gated skip contract can be exercised directly by scripts/tests.
if [ "${BASH_SOURCE[0]}" != "$0" ]; then
  return 0
fi

run_required node pinvou3-app/tests/markdown_syntax_highlight.test.mjs
run_required node pinvou3-app/tests/windows_runtime_packaging_contract.test.js
run_required python3 -m unittest discover -s scripts/tests -p 'test_*.py'
if ! python3 -c 'import pptx, docx' >/dev/null 2>&1; then
  run_required python3 -m pip install --quiet python-pptx python-docx
fi
run_required python3 scripts/mcp-server-contract-smoke.py
if [ ! -x "$ROOT/pinvou3-app/node_modules/.bin/vite" ]; then
  run_required npm --prefix pinvou3-app ci --prefer-offline --no-audit
fi
if [ ! -d "$ROOT/remote-control-relay/node_modules/ws" ]; then
  run_required npm --prefix remote-control-relay ci --prefer-offline --no-audit
fi
run_required npm --prefix pinvou3-app run build:ui
run_required npm --prefix remote-control-relay test
run_optional_skip2 node pinvou3-app/tests/ui_smoke.js
run_optional_skip2 node pinvou3-app/tests/settings_ui_smoke.js
run_optional_skip2 node pinvou3-app/tests/kb_smoke.js
run_optional_skip2 node pinvou3-app/tests/tool_store_smoke.js
run_optional_skip2 npm --prefix pinvou3-app run test:webui

if [ "${PINVOU3_AUDIT_LATEST_SESSIONS:-0}" != "0" ]; then
  run_required python3 scripts/session-replay-audit.py --latest "$PINVOU3_AUDIT_LATEST_SESSIONS"
fi

if [ -n "${PINVOU3_AUDIT_SESSION:-}" ]; then
  run_required python3 scripts/session-replay-audit.py "$PINVOU3_AUDIT_SESSION"
fi

if [ "${PINVOU3_RUN_L1:-0}" = "1" ]; then
  run_required env PINVOU3_L1_REQUIRE_VLLM=1 \
    cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml \
    --test l1_dialog_harness -- --ignored --test-threads=1
fi

# memory_e2e(集成测试文件,默认 #[ignore]):env 隔离域测试全量 + 真机 vLLM 记忆
# 行为,与 L1 同为手动验收路径;此前无任何 runner 入口(审计孤儿),在此接线。
if [ "${PINVOU3_RUN_MEMORY_E2E:-0}" = "1" ]; then
  run_required cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml \
    --test memory_e2e -- --ignored --test-threads=1
fi

echo "ALL USER JOURNEY SMOKES PASSED"
