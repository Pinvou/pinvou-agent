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
# 失败回显脱敏(host 侧 connector_cli.rs safe_auth_log_line 同口径):凡出现
# access_token/refresh_token/authorization/bearer(整行小写后匹配,大小写不敏感)
# 的行整行打码——等号、裸冒号、URL 查询串、大写 scheme 形态一并覆盖(host 对
# 这些形态就是整行替换);其余行再按 JSON 字段形态打码 token 值。纵深防御。
redact(){ awk '{ l=tolower($0)
    if (l ~ /access_token|refresh_token|authorization|bearer/) print "[已脱敏:含凭证形状的整行]"
    else print }' | sed -E 's/[A-Za-z_-]*[Tt][Oo][Kk][Ee][Nn][A-Za-z_-]*"?:[[:space:]]*"[^"]*"/token=[REDACTED]/g'; }

# --- 定位 wecom-cli(Windows npm 全局 shim) ---
CLI="$(command -v wecom-cli 2>/dev/null || true)"
[ -z "$CLI" ] && [ -n "${APPDATA:-}" ] && [ -f "$APPDATA/npm/wecom-cli.cmd" ] && CLI="$APPDATA/npm/wecom-cli.cmd"
[ -z "$CLI" ] && [ -f "$HOME/AppData/Roaming/npm/wecom-cli" ] && CLI="$HOME/AppData/Roaming/npm/wecom-cli"
if [ -z "$CLI" ]; then echo "找不到 wecom-cli(先 npm i -g @wecom/cli 并扫码授权)"; exit 2; fi

# 探测必须限时(与 dingtalk-smoke.sh 同口径):这一族 npm-shim CLI 曾实测卡在
# 网络/代理/无 TTY 提示上无限挂起,冒烟挂着不退出比红更糟。有 coreutils timeout
# 用之;否则 bash 看门狗——子壳必须重定向到 /dev/null,否则它继承 $( ) 管道写端,
# 快探针也要被孤儿 sleep 拖满时限(实测健康路径 9×30s;重定向后即刻返回)。
# kill 只打直接子进程:native 二进制 / exec node 的 npm shim 即全杀。
WECOM_TIMEOUT=30
if command -v timeout >/dev/null 2>&1; then
  wecom_run() { timeout "$WECOM_TIMEOUT" "$CLI" "$@"; }
else
  wecom_run() {
    "$CLI" "$@" &
    local pid=$!
    ( sleep "$WECOM_TIMEOUT" && kill "$pid" 2>/dev/null ) >/dev/null 2>&1 &
    local wpid=$!
    wait "$pid" 2>/dev/null; local rc=$?
    kill "$wpid" 2>/dev/null; wait "$wpid" 2>/dev/null
    [ "$rc" -eq 143 ] && rc=124
    return "$rc"
  }
fi
echo "wecom-cli = $CLI"
echo

MIN_WECOM="1.3.4"  # 与 wecom.rs WECOM_MIN_VERSION 一致;connector_online_install_contract.test.js 钉 literal+比较形态,漂移即红
echo "[1] --version runs and is >= ${MIN_WECOM} (matches WECOM_MIN_VERSION in wecom.rs)"
# 只认点分版本号(比 wecom.rs parse_wecom_version 更严:裸数字串如构建日期会被
# 逐段比较误放行);不足三段补 0(两段 `2.0` → 2.0.0)。不用 sort -V。
VER="$(wecom_run --version 2>&1 || true)"
TRI="$(printf '%s\n' "$VER" | grep -oE '[0-9]+(\.[0-9]+)+' | head -1 | awk -F. '{printf "%d.%d.%d", $1, $2+0, $3+0}')"
# 逐段数值比较(与 dingtalk-smoke.sh 同算法;加权 $1*10000+... 会把 1.2.9999
# 误判 >= 1.3.4):TRI >= MIN_WECOM 当且仅当两行升序排序后的首行就是 MIN_WECOM。
GE="$(printf '%s\n%s\n' "$TRI" "$MIN_WECOM" | sort -t. -k1,1n -k2,2n -k3,3n | head -1 | grep -qxF "$MIN_WECOM" && echo yes || echo no)"
if [ -n "$TRI" ]; then
  ok "--version 可执行: $(echo "$VER" | tr -d '\r' | head -1)"
  if [ "$GE" = yes ]; then
    ok "version >= ${MIN_WECOM}: $TRI"
  else
    no "version below ${MIN_WECOM} (below the minimum accepted version): $TRI"
  fi
else
  no "--version 失败或无法解析: $(printf '%s' "$VER" | redact | tr -d '\n' | head -c 120)"
fi

echo "[2] 连接状态(auth show --status 输出 authorized = 已授权)"
# 与 wecom.rs status_is_authorized 同口径:整行、大小写不敏感;保留 stderr、
# 剥 \r(Windows npm shim 的 CRLF 输出会让大小写敏感的 grep -qx 误报未连接)。
AUTH="$(wecom_run auth show --status 2>&1 || true)"
if echo "$AUTH" | tr -d '\r' | grep -xiq 'authorized'; then
  ok "已连接(auth show --status 返回 authorized)"
else
  no "未连接/未授权(先扫码): $(printf '%s' "$AUTH" | redact | tr -d '\n' | head -c 120)"
fi

echo "[3] 各域授权探测(只读;授权域应响应,未授权域报权限错)"
# 域=CLI 顶层子命令:doc-manage 技能的命令实跑在 doc 域下,chat 有域无技能目录。
# 用退出码判定可用性——错误输出也带 Usage 行,grep Usage 会把不存在的域误报可用。
for d in contact doc chat mail disk media message meeting sheet smartpage smartsheet calendar todo slide; do
  OUT="$(wecom_run "$d" --help 2>&1)"; d_rc=$?
  if echo "$OUT" | grep -Eq '权限|暂不支持|未授权'; then sk "$d 域未授权"
  elif echo "$OUT" | grep -q 'unrecognized subcommand'; then no "$d 域不存在(CLI 无此子命令)"
  elif [ "$d_rc" -eq 0 ]; then ok "$d 域可用"; dom_ok=$((dom_ok+1))
  else sk "$d 域结果未知(exit $d_rc): $(echo "$OUT" | tr -d '\n' | head -c 60)"; fi
done

echo "[4] 真实读文档(可选,需 WECOM_TEST_DOCID)"
if [ -n "${WECOM_TEST_DOCID:-}" ]; then
  R="$(wecom_run doc contents get --docid "$WECOM_TEST_DOCID" 2>&1)"; r_rc=$?
  # PASS 以退出码 0 + 有内容为前提:flag/参数漂移时 CLI 打 usage/错误走非零退出,
  # 不能因输出里没有错误字样就把失败输出误判成读成功(内容可能是纯文本,不要求 JSON)。
  if [ "$r_rc" -eq 0 ] && [ -n "$R" ] && ! echo "$R" | grep -Eqi '"error"|错误|失败|权限'; then
    ok "读到文档内容($(printf '%s' "$R" | wc -c | tr -d ' ') 字节)"
  else
    no "读文档失败(exit $r_rc): $(printf '%s' "$R" | redact | tr -d '\n' | head -c 160)"
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
