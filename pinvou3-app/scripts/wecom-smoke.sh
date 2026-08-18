#!/usr/bin/env bash
# 企微 live 冒烟测试 —— 打真企微云,**只读为主、安全默认**(不创建文档、不发消息)。
#
# 用法:
#   bash scripts/wecom-smoke.sh
#   WECOM_TEST_DOCID=<docid> bash scripts/wecom-smoke.sh   # 额外真实读一篇已有文档
#
# 退出码:有硬性检查(版本/连接)失败则非 0,可挂 CI(但需先扫码授权,故默认手动跑)。
set -u

pass=0; fail=0; skip=0
ok(){ echo "  [PASS] $1"; pass=$((pass+1)); }
no(){ echo "  [FAIL] $1"; fail=$((fail+1)); }
sk(){ echo "  [SKIP] $1"; skip=$((skip+1)); }

# --- 定位 wecom-cli(Windows npm 全局 shim) ---
CLI="$(command -v wecom-cli 2>/dev/null || true)"
[ -z "$CLI" ] && [ -n "${APPDATA:-}" ] && [ -f "$APPDATA/npm/wecom-cli.cmd" ] && CLI="$APPDATA/npm/wecom-cli.cmd"
[ -z "$CLI" ] && [ -f "$HOME/AppData/Roaming/npm/wecom-cli" ] && CLI="$HOME/AppData/Roaming/npm/wecom-cli"
if [ -z "$CLI" ]; then echo "找不到 wecom-cli(先 npm i -g @wecom/cli 并扫码授权)"; exit 2; fi
echo "wecom-cli = $CLI"
echo

echo "[1] 版本可执行且 ≥1.1.0(命令模型基线)"
VER="$("$CLI" --version 2>/dev/null || true)"
VNUM="$(echo "$VER" | awk '{print $2}')"
if [ -n "$VER" ]; then
  ok "--version 退出 0: $(echo "$VER" | head -1)"
  if [ "$(printf '%s\n' "1.1.0" "$VNUM" | sort -V | head -1)" = "1.1.0" ]; then
    ok "版本 ≥1.1.0"
  else
    no "版本低于 1.1.0(命令模型不匹配): $VNUM"
  fi
else
  no "--version 失败"
fi

echo "[2] 连接状态(auth show --status 输出 authorized = 已授权)"
AUTH="$("$CLI" auth show --status 2>/dev/null || true)"
if echo "$AUTH" | grep -qx 'authorized'; then
  ok "已连接(auth show --status 返回 authorized)"
else
  no "未连接/未授权(先扫码): $(echo "$AUTH" | tr -d '\n' | head -c 120)"
fi

echo "[3] 各域授权探测(只读;授权域应响应,未授权域报权限错)"
for d in contact doc doc-manage mail disk media message meeting sheet smartpage smartsheet calendar todo; do
  OUT="$("$CLI" "$d" --help 2>&1 || true)"
  if echo "$OUT" | grep -Eq '权限|暂不支持|未授权'; then sk "$d 域未授权"
  elif echo "$OUT" | grep -qi 'Usage'; then ok "$d 域可用"
  else sk "$d 域结果未知: $(echo "$OUT" | tr -d '\n' | head -c 60)"; fi
done

echo "[4] 真实读文档(可选,需 WECOM_TEST_DOCID)"
if [ -n "${WECOM_TEST_DOCID:-}" ]; then
  R="$("$CLI" doc contents get --docid "$WECOM_TEST_DOCID" 2>&1 || true)"
  if [ -n "$R" ] && ! echo "$R" | grep -Eqi '"error"|错误|失败|权限'; then
    ok "读到文档内容(${#R} 字节)"
  else
    no "读文档失败: $(echo "$R" | tr -d '\n' | head -c 160)"
  fi
else
  sk "未设 WECOM_TEST_DOCID,跳过真实读文档(避免无谓副作用)"
fi

echo
echo "结果: PASS=$pass  FAIL=$fail  SKIP=$skip"
[ "$fail" -eq 0 ]
