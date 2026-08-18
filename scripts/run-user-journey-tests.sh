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

run_optional_skip2() {
  echo "== $* =="
  set +e
  (cd "$ROOT" && "$@")
  rc=$?
  set -e
  if [ "$rc" -eq 2 ]; then
    echo "SKIP: optional dependency missing for: $*"
    return 0
  fi
  return "$rc"
}

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

echo "ALL USER JOURNEY SMOKES PASSED"
