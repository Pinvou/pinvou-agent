#!/usr/bin/env bash
# 企微 live 冒烟测试 —— 打真企微云,**只读为主、安全默认**(不创建文档、不发消息)。
#
# 用法:
#   bash scripts/wecom-smoke.sh
#   WECOM_TEST_DOCID=<docid> bash scripts/wecom-smoke.sh   # 额外真实读一篇已有文档
#
# 退出码:有硬性检查(版本/连接)失败则非 0,可挂 CI(但需先扫码授权,故默认手动跑);
# 全部域探测被跳过(瞬态首调)时 exit 3 —— 「没验证到」不等于「验证通过」。
set -u

pass=0; fail=0; skip=0; dom_ok=0
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
# 与 wecom.rs parse_wecom_version 同口径:取输出前三个数字段逐段数值比较、
# 不足三段补 0(两段 `2.0` → 2.0.0,与 parse_semver3 一致);
# 不用 sort -V——它对 prerelease/两段版本的判定与 Rust 侧三段解析不一致。
TRI="$(echo "$VER" | grep -oE '[0-9]+' | head -3 | paste -sd. -)"
GE="$(printf '%s\n' "$TRI" | awk -F. '($1*10000+$2*100+$3 >= 10100) {print "yes"}')"
if [ -n "$VER" ]; then
  ok "--version 可执行: $(echo "$VER" | head -1)"
  if [ "$GE" = yes ]; then
    ok "版本 ≥1.1.0: $VNUM"
  else
    no "版本低于 1.1.0(命令模型不匹配): $VNUM"
  fi
else
  no "--version 失败"
fi

echo "[2] 连接状态(auth show --status 输出 authorized = 已授权)"
# 与 wecom.rs status_is_authorized 同口径:整行、大小写不敏感;保留 stderr、
# 剥 \r(Windows npm shim 的 CRLF 输出会让大小写敏感的 grep -qx 误报未连接)。
AUTH="$("$CLI" auth show --status 2>&1 || true)"
if echo "$AUTH" | tr -d '\r' | grep -xiq 'authorized'; then
  ok "已连接(auth show --status 返回 authorized)"
else
  no "未连接/未授权(先扫码): $(echo "$AUTH" | tr -d '\n' | head -c 120)"
fi

echo "[3] 各域授权探测(只读;授权域应响应,未授权域报权限错)"
# 域=CLI 顶层子命令:doc-manage 技能的命令实跑在 doc 域下,chat 有域无技能目录。
# 用退出码判定可用性——错误输出也带 Usage 行,grep Usage 会把不存在的域误报可用。
for d in contact doc chat mail disk media message meeting sheet smartpage smartsheet calendar todo; do
  OUT="$("$CLI" "$d" --help 2>&1)"; d_rc=$?
  if echo "$OUT" | grep -Eq '权限|暂不支持|未授权'; then sk "$d 域未授权"
  elif echo "$OUT" | grep -q 'unrecognized subcommand'; then no "$d 域不存在(CLI 无此子命令)"
  elif [ "$d_rc" -eq 0 ]; then ok "$d 域可用"; dom_ok=$((dom_ok+1))
  else sk "$d 域结果未知(exit $d_rc): $(echo "$OUT" | tr -d '\n' | head -c 60)"; fi
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
if [ "$fail" -eq 0 ] && [ "$dom_ok" -eq 0 ]; then
  echo "警告: 所有域探测均被跳过,本次未验证任何命令面(多为 service discovery 瞬态,重跑一次)"
  exit 3
fi
[ "$fail" -eq 0 ]
