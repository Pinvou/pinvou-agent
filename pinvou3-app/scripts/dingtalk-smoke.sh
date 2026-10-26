#!/usr/bin/env bash
# 钉钉 dws live 冒烟测试 —— 打真钉钉云,**只读为主、安全默认**(不创建文档、不发消息)。
#
# 用法:
#   bash scripts/dingtalk-smoke.sh
#   DINGTALK_TEST_DOC_ID=<doc_id> bash scripts/dingtalk-smoke.sh   # 额外真实读一篇已有文档
#
# 退出码:有硬性检查(版本/连接/可选读文档)失败则非 0,可挂 CI(但需先扫码授权,故默认手动跑);
# 全部域探测被跳过时 exit 3 —— 「没验证到」不等于「验证通过」。
set -u

pass=0; fail=0; skip=0; dom_ok=0
ok(){ echo "  [PASS] $1"; pass=$((pass+1)); }
no(){ echo "  [FAIL] $1"; fail=$((fail+1)); }
sk(){ echo "  [SKIP] $1"; skip=$((skip+1)); }

# --- 定位 dws ---
# 应用按 connectors.lock.json 把 dws 落到版本目录 ~/.pinvou3/assets/cli/dws/<version>/
# (paths.rs assets_cli_dir);旧布局 ~/.pinvou3/connectors/<platform>/bin/ 会被
# native_installer 迁移并清理,只作存量回退;再退到 npm 全局 shim。
CLI="$(command -v dws 2>/dev/null || true)"
if [ -z "$CLI" ]; then
  # 多版本并存时取最高版本(按数字段排序,不依赖 sort -V)
  for exe in dws dws.exe; do
    [ -n "$CLI" ] && break
    ver="$(for d in "$HOME"/.pinvou3/assets/cli/dws/*/; do
             [ -x "$d$exe" ] && basename "$d"; done 2>/dev/null | sort -t. -k1,1n -k2,2n -k3,3n | tail -1)"
    [ -n "$ver" ] && CLI="$HOME/.pinvou3/assets/cli/dws/$ver/$exe"
  done
fi
for plat in linux-arm64 linux-x64 darwin-arm64 darwin-x64; do
  [ -z "$CLI" ] && [ -x "$HOME/.pinvou3/connectors/$plat/bin/dws" ] && CLI="$HOME/.pinvou3/connectors/$plat/bin/dws"
done
[ -z "$CLI" ] && [ -f "$HOME/.pinvou3/connectors/windows-x64/bin/dws.exe" ] && CLI="$HOME/.pinvou3/connectors/windows-x64/bin/dws.exe"
[ -z "$CLI" ] && [ -n "${APPDATA:-}" ] && [ -f "$APPDATA/npm/dws.cmd" ] && CLI="$APPDATA/npm/dws.cmd"
[ -z "$CLI" ] && [ -f "$HOME/AppData/Roaming/npm/dws.cmd" ] && CLI="$HOME/AppData/Roaming/npm/dws.cmd"
if [ -z "$CLI" ]; then echo "找不到 dws(先在 Pinvou Agent 工具面板连接钉钉,应用会在线下载并校验)"; exit 2; fi
echo "dws = $CLI"
echo

echo "[1] --version runs and is >= 1.0.64 (dws SKILL.md cli_version floor; lock pins 1.0.65)"
VER="$("$CLI" --version 2>&1 || true)"
# 与 wecom-smoke 同口径:取输出前三个数字段逐段数值比较、不足三段补 0;不用 sort -V。
# The floor value (10064 = 1.0.64) is pinned to the SKILL.md cli_version by
# connector_online_install_contract.test.js, so drift on either side fails the test.
TRI="$(echo "$VER" | tr -d '\r' | grep -oE '[0-9]+' | head -3 | paste -sd. -)"
GE="$(printf '%s\n' "$TRI" | awk -F. '($1*10000+$2*100+$3 >= 10064) {print "yes"}')"
if [ -n "$TRI" ]; then
  ok "--version 可执行: $(echo "$VER" | head -1)"
  if [ "$GE" = yes ]; then
    ok "version >= 1.0.64: $TRI"
  else
    no "version below 1.0.64 (below the SKILL.md cli_version floor): $TRI"
  fi
else
  no "--version 失败或无法解析: $(echo "$VER" | tr -d '\n' | head -c 120)"
fi

echo "[2] 连接状态(auth status --format json 的 authenticated=true = 已授权)"
# 与 dingtalk.rs 同口径:只认官方 JSON 的 authenticated:true,stdout/stderr 都看;剥 \r。
AUTH="$("$CLI" auth status --format json 2>&1 || true)"
if echo "$AUTH" | tr -d '\r' | grep -Eq '"authenticated"[[:space:]]*:[[:space:]]*true'; then
  ok "已连接(auth status authenticated=true)"
else
  no "未连接/未授权(先扫码): $(echo "$AUTH" | tr -d '\n' | head -c 160)"
fi

echo "[3] 各服务域探测(只读 --help;存在的服务 exit 0,不存在则 exit 1)"
# 域=dws 顶层服务,--help 不触网、不需授权。用退出码判定——Cobra 对未知命令也会打
# "Run '… --help' for usage." 行,grep Usage 会把不存在的域误报可用。只探顶层服务:
# Cobra 对组命令下的未知叶子会直接打印组帮助并 exit 0,叶子级探测分不出真假。
for d in doc drive calendar todo chat sheet aitable; do
  OUT="$("$CLI" "$d" --help 2>&1)"; d_rc=$?
  if [ "$d_rc" -eq 0 ]; then ok "$d 域可用"; dom_ok=$((dom_ok+1))
  elif echo "$OUT" | grep -Eqi 'unknown command|not found|不存在|未知'; then no "$d 域不存在(CLI 无此服务)"
  else sk "$d 域结果未知(exit $d_rc): $(echo "$OUT" | tr -d '\n' | head -c 60)"; fi
done

echo "[4] 真实读文档(可选,需 DINGTALK_TEST_DOC_ID)"
if [ -n "${DINGTALK_TEST_DOC_ID:-}" ]; then
  R="$("$CLI" doc read --node "$DINGTALK_TEST_DOC_ID" --format json 2>&1 || true)"
  if [ -n "$R" ] && ! echo "$R" | grep -Eqi '"error"|"success"[[:space:]]*:[[:space:]]*false|错误|失败|权限'; then
    ok "读到文档内容(${#R} 字节)"
  else
    no "读文档失败: $(echo "$R" | tr -d '\n' | head -c 160)"
  fi
else
  sk "未设 DINGTALK_TEST_DOC_ID,跳过真实读文档(避免无谓副作用)"
fi

echo
echo "结果: PASS=$pass  FAIL=$fail  SKIP=$skip"
if [ "$fail" -eq 0 ] && [ "$dom_ok" -eq 0 ]; then
  echo "警告: 所有域探测均被跳过,本次未验证任何命令面(重跑一次)"
  exit 3
fi
[ "$fail" -eq 0 ]
