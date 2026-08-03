#!/usr/bin/env bash
# 钉钉 dws live 冒烟测试 —— 打真钉钉云,只读为主、安全默认。
#
# 用法:
#   bash scripts/dingtalk-smoke.sh
#   DINGTALK_TEST_DOC_ID=<doc_id> bash scripts/dingtalk-smoke.sh   # 额外真实读一篇已有文档
#
# 退出码:版本/连接硬性检查失败则非 0。需先在工具面板扫码授权或手动 dws auth login --device。
set -u

pass=0; fail=0; skip=0
ok(){ echo "  [PASS] $1"; pass=$((pass+1)); }
no(){ echo "  [FAIL] $1"; fail=$((fail+1)); }
sk(){ echo "  [SKIP] $1"; skip=$((skip+1)); }

# --- 定位 dws(Windows npm 全局 shim / 各平台内置 bundle bin / PATH) ---
CLI="$(command -v dws 2>/dev/null || true)"
[ -z "$CLI" ] && [ -n "${APPDATA:-}" ] && [ -f "$APPDATA/npm/dws.cmd" ] && CLI="$APPDATA/npm/dws.cmd"
[ -z "$CLI" ] && [ -f "$HOME/AppData/Roaming/npm/dws.cmd" ] && CLI="$HOME/AppData/Roaming/npm/dws.cmd"
for plat in linux-arm64 linux-x64 darwin-arm64 darwin-x64; do
  [ -z "$CLI" ] && [ -f "$HOME/.pinvou3/bundle/connectors/$plat/bin/dws" ] && CLI="$HOME/.pinvou3/bundle/connectors/$plat/bin/dws"
done
[ -z "$CLI" ] && [ -f "$HOME/.pinvou3/bundle/connectors/windows-x64/bin/dws.exe" ] && CLI="$HOME/.pinvou3/bundle/connectors/windows-x64/bin/dws.exe"
if [ -z "$CLI" ]; then echo "找不到 dws(先 npm i -g dingtalk-workspace-cli 或用 Pinvou3 工具面板连接)"; exit 2; fi
echo "dws = $CLI"
echo

echo "[1] 版本可执行"
if "$CLI" --version >/dev/null 2>&1; then ok "--version 退出 0"; else no "--version 失败"; fi

echo "[2] 连接状态(auth status authenticated=true = 已授权)"
AUTH="$("$CLI" auth status --format json 2>/dev/null || true)"
if echo "$AUTH" | grep -Eq '"authenticated"[[:space:]]*:[[:space:]]*true'; then
  ok "已连接(auth status authenticated=true)"
else
  no "未连接/未授权(先扫码): $(echo "$AUTH" | tr -d '\n' | head -c 160)"
fi

echo "[3] 常用域命令探测(只读 help)"
for cmd in \
  "doc search --help" \
  "calendar event list --help" \
  "todo list --help" \
  "chat message list --help" \
  "sheet --help" \
  "aitable --help"; do
  OUT="$("$CLI" $cmd 2>&1 || true)"
  if echo "$OUT" | grep -Eqi 'Usage|用法|帮助|Commands|Options'; then ok "$cmd"
  elif echo "$OUT" | grep -Eqi 'not found|unknown|不存在|未知'; then sk "$cmd 不支持或命令名变化"
  else sk "$cmd 结果未知: $(echo "$OUT" | tr -d '\n' | head -c 80)"; fi
done

echo "[4] 真实读文档(可选,需 DINGTALK_TEST_DOC_ID)"
if [ -n "${DINGTALK_TEST_DOC_ID:-}" ]; then
  R="$("$CLI" doc read --node "$DINGTALK_TEST_DOC_ID" 2>&1 || true)"
  if [ -n "$R" ] && ! echo "$R" | grep -Eqi 'error|错误|失败|权限|unauthorized'; then
    ok "读到文档内容(${#R} 字节)"
  else
    no "读文档失败: $(echo "$R" | tr -d '\n' | head -c 160)"
  fi
else
  sk "未设 DINGTALK_TEST_DOC_ID,跳过真实读文档(避免无谓副作用)"
fi

echo
echo "结果: PASS=$pass  FAIL=$fail  SKIP=$skip"
[ "$fail" -eq 0 ]
