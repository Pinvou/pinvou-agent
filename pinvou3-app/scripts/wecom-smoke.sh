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
# set -u 下 HOME 未设置会直接炸成 unbound variable;这里给可行动的退出码 2。
if [ -z "${HOME:-}" ]; then echo "HOME 未设置,无法定位 wecom-cli"; exit 2; fi

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
    else print }' | sed -E 's/[A-Za-z0-9_-]*[Tt][Oo][Kk][Ee][Nn][A-Za-z0-9_-]*"?:[[:space:]]*"[^"]*"/token=[REDACTED]/g'; }

# --- 定位 wecom-cli(与 dingtalk-smoke.sh 同口径) ---
# 应用按 connectors.lock.json 把 wecom-cli 落到版本目录 ~/.pinvou3/assets/cli/wecom-cli/<version>/
# (paths.rs assets_cli_dir;native_installer 用 connector_lock::executable_name,Windows 带 .exe);
# 旧布局 ~/.pinvou3/connectors/<platform>/bin/ 只作存量回退;再退到 npm 全局 shim。
# 只搜 PATH 会漏掉应用自己装的 CLI,并把用户引向 npm -g 自装——那正是
# wecomcli-shared 技能禁止的路径(绕过 lock 钉扎,触发哈希不符重装循环)。
CLI="$(command -v wecom-cli 2>/dev/null || true)"
case "$(uname -s 2>/dev/null)" in
  MINGW*|MSYS*|CYGWIN*) PLAT=windows-x64; EXE=wecom-cli.exe ;;
  Linux)
    case "$(uname -m 2>/dev/null)" in
      aarch64|arm64) PLAT=linux-arm64 ;;
      *) PLAT=linux-x64 ;;
    esac
    EXE=wecom-cli ;;
  Darwin)
    case "$(uname -m 2>/dev/null)" in
      arm64) PLAT=darwin-arm64 ;;
      *) PLAT=darwin-x64 ;;
    esac
    EXE=wecom-cli ;;
  *) PLAT=""; EXE=wecom-cli ;;
esac
if [ -z "$CLI" ]; then
  # 多版本并存时取最高版本(逐段数值排序,不依赖 sort -V);只认含点的纯数字目录
  # (排除 1.3.4.bak 改名残留与裸日期目录),且 -f -x 并举(排除同名目录)。候选在
  # 普通循环里累积再排序——stock bash 3.2 解析不了 $() 内嵌 case。
  ver=""
  for d in "$HOME"/.pinvou3/assets/cli/wecom-cli/*/; do
    db="${d%/}"; db="${db##*/}"
    case "$db" in *[!0-9.]*) continue ;; esac
    case "$db" in *.*) : ;; *) continue ;; esac
    [ -f "$d$EXE" ] || continue
    [ -x "$d$EXE" ] || continue
    ver="$ver$db
"
  done
  ver="$(printf '%s' "$ver" | sort -t. -k1,1n -k2,2n -k3,3n | tail -1)"
  [ -n "$ver" ] && CLI="$HOME/.pinvou3/assets/cli/wecom-cli/$ver/$EXE"
fi
[ -z "$CLI" ] && [ -n "$PLAT" ] && [ -f "$HOME/.pinvou3/connectors/$PLAT/bin/$EXE" ] && [ -x "$HOME/.pinvou3/connectors/$PLAT/bin/$EXE" ] && CLI="$HOME/.pinvou3/connectors/$PLAT/bin/$EXE"
[ -z "$CLI" ] && [ -n "${APPDATA:-}" ] && [ -f "$APPDATA/npm/wecom-cli.cmd" ] && CLI="$APPDATA/npm/wecom-cli.cmd"
[ -z "$CLI" ] && [ -f "$HOME/AppData/Roaming/npm/wecom-cli" ] && CLI="$HOME/AppData/Roaming/npm/wecom-cli"
if [ -z "$CLI" ]; then echo "找不到 wecom-cli(先在 Pinvou Agent 工具面板连接企业微信,应用会在线下载并校验;开发机自装时放进 PATH)"; exit 2; fi

# 探测必须限时(与 dingtalk-smoke.sh 同口径):这一族 CLI 曾实测卡在网络/代理/
# 无 TTY 提示上无限挂起,冒烟挂着不退出比红更糟。有 coreutils timeout 用之;否则
# bash 看门狗——子壳必须重定向到 /dev/null,否则它继承 $( ) 管道写端,快探针也要
# 被孤儿 sleep 拖满时限(实测健康路径 9×30s;重定向后即刻返回)。CLI 输出先落临时
# 文件再回放:两个分支的 kill 都只打得到直接子进程,@wecom/cli 的 npm 包装器是
# execFileSync(spawn+wait)形态,被杀后 native 孙进程会握着管道写端拖住整段脚本;
# 落盘后孙进程只握临时文件,管道随 cat 结束即 EOF。
WECOM_TIMEOUT=30
# 与 dingtalk-smoke.sh 同口径:TERM 不是终点,吞 TERM 的 CLI 不能把探测变成永久
# 等待。timeout 加 -k、看门狗 KILL_GRACE 后升级 SIGKILL;137(KILL 死)与 143
# (TERM 死)都归一为 124,超时诊断分支照常生效。
WECOM_KILL_GRACE=5
TIMEOUT_BIN="$(command -v timeout 2>/dev/null || true)"
wecom_run() {
  local t rc tflag
  t="$(mktemp "${TMPDIR:-/tmp}/wecom-smoke.XXXXXX")"
  tflag="${t}.deadline"
  if [ -n "$TIMEOUT_BIN" ]; then
    "$TIMEOUT_BIN" -k "$WECOM_KILL_GRACE" "$WECOM_TIMEOUT" "$CLI" "$@" >"$t" 2>&1; rc=$?
  else
    "$CLI" "$@" >"$t" 2>&1 &
    local pid=$!
    # 发出 TERM 即落「到点」标记(与 dingtalk-smoke.sh 同口径):子进程收到 TERM
    # 后优雅清理并以 0 退出时,wait 的 0 会把超时伪装成通过——到点事实必须独立
    # 于子进程最终退出码,标记存在则一律按 124 上报。
    ( sleep "$WECOM_TIMEOUT" && kill "$pid" 2>/dev/null && : >"$tflag" && sleep "$WECOM_KILL_GRACE" && kill -9 "$pid" 2>/dev/null ) >/dev/null 2>&1 &
    local wpid=$!
    wait "$pid" 2>/dev/null; rc=$?
    kill "$wpid" 2>/dev/null; wait "$wpid" 2>/dev/null
    [ -e "$tflag" ] && rc=124
  fi
  # 超时形态归一:TERM 死(143)与 KILL 死(137)都按 124 上报。
  [ "$rc" -eq 143 ] && rc=124
  [ "$rc" -eq 137 ] && rc=124
  cat "$t"; rm -f "$t" "$tflag"
  return "$rc"
}
echo "wecom-cli = $CLI"
echo

MIN_WECOM="1.3.4"  # 与 wecom.rs WECOM_MIN_VERSION 一致;connector_online_install_contract.test.js 钉 literal+比较形态,漂移即红
echo "[1] --version runs and is >= ${MIN_WECOM} (matches WECOM_MIN_VERSION in wecom.rs)"
# 版本号只从含 CLI 名字的那一行、且位于名字之后取(与 dingtalk-smoke.sh 同口径;
# 实测 1.3.4 banner 为单行 `wecom-cli 1.3.4 (wecom <build-time> <sha>)`):同族
# lark-cli 会在版本行之前打「a new version … is available」,「全文首个点分数字」
# 会把更新号读成本机版本。只认点分版本号(比 wecom.rs parse_wecom_version 更严:
# 裸数字串如构建日期会被逐段比较误放行);不足三段补 0(两段 `2.0` → 2.0.0)。不用 sort -V。
VER="$(wecom_run --version 2>&1)"; v_rc=$?
TRI="$(printf '%s\n' "$VER" | awk 'tolower($0) ~ /wecom-cli/ { for (i = 1; i <= NF; i++) { if (seen && $i ~ /^[0-9]+(\.[0-9]+)+$/) { print $i; exit } if (tolower($i) ~ /wecom-cli/) seen = 1 } }' | awk -F. '{printf "%d.%d.%d", $1, $2+0, $3+0}')"
# 逐段数值比较(与 dingtalk-smoke.sh 同算法;加权 $1*10000+... 会把 1.2.9999
# 误判 >= 1.3.4):TRI >= MIN_WECOM 当且仅当两行升序排序后的首行就是 MIN_WECOM。
GE="$(printf '%s\n%s\n' "$TRI" "$MIN_WECOM" | sort -t. -k1,1n -k2,2n -k3,3n | head -1 | grep -qxF "$MIN_WECOM" && echo yes || echo no)"
# PASS 前提是探测本身成功(v_rc=0,与 dingtalk-smoke.sh 同口径):健康形状的输出 +
# 非零退出(含超时 124)是「CLI 跑了但失败」,不能因输出像样就发 PASS。
if [ "$v_rc" -eq 0 ] && [ -n "$TRI" ]; then
  ok "--version 可执行: $(printf '%s\n' "$VER" | redact | tr -d '\r' | head -1)"
  if [ "$GE" = yes ]; then
    ok "version >= ${MIN_WECOM}: $TRI"
  else
    no "version below ${MIN_WECOM} (below the minimum accepted version): $TRI"
  fi
else
  # 超时(124)与「跑不动/解析不了」的处置不同(与 dingtalk-smoke.sh 同口径)。
  if [ "$v_rc" -eq 124 ]; then
    no "--version 探测超时(${WECOM_TIMEOUT}s;该 CLI 族已知会挂网络/代理/无 TTY 提示): $(printf '%s' "$VER" | redact | tr -d '\n' | head -c 120)"
  else
    no "--version 失败或无法解析: $(printf '%s' "$VER" | redact | tr -d '\n' | head -c 120)"
  fi
fi

echo "[2] 连接状态(auth show --status 输出 authorized = 已授权)"
AUTH="$(wecom_run auth show --status 2>&1)"; a_rc=$?
# 与 wecom.rs status_is_authorized 同口径:整行、大小写不敏感;保留 stderr、
# 剥 \r(Windows npm shim 的 CRLF 输出会让大小写敏感的 grep -qx 误报未连接)。
# 先要求探测退出 0(与 dingtalk-smoke.sh 同口径):exit 7 + authorized 是失败,
# 不是已连接。
if [ "$a_rc" -eq 0 ] && echo "$AUTH" | tr -d '\r' | grep -xiq 'authorized'; then
  ok "已连接(auth show --status 返回 authorized)"
else
  if [ "$a_rc" -eq 124 ]; then
    no "auth 探测超时(${WECOM_TIMEOUT}s;该 CLI 族已知会挂网络/代理/无 TTY 提示,不是未授权): $(printf '%s' "$AUTH" | redact | tr -d '\n' | head -c 120)"
  elif [ "$a_rc" -ne 0 ]; then
    no "auth 探测失败(exit $a_rc;CLI 自身报错,不是未授权): $(printf '%s' "$AUTH" | redact | tr -d '\n' | head -c 120)"
  else
    no "未连接/未授权(先扫码): $(printf '%s' "$AUTH" | redact | tr -d '\n' | head -c 120)"
  fi
fi

echo "[3] 各域授权探测(只读 --help;存在的子命令 exit 0,未授权/不存在走非零)"
# 域=CLI 顶层子命令:doc-manage 技能的命令实跑在 doc 域下,chat 有域无技能目录。
# 先看退出码:--help 不触网,exit 0 即子命令存在、域可用。超时(124)检查先于
# 权限词——超时探测的部分输出可能恰含「权限」,先查词会把超时误标成未授权。
# 权限词只在非零退出时才当作未授权——实测 1.3.4 `doc --help` 的服务描述里就有
# 「权限管理」,先查词会把健康的 doc 域(文档技能全跑在它上面)永远误报 SKIP。
# 错误输出也带 Usage 行,grep Usage 会把不存在的域误报可用,故同样以退出码为准。
for d in contact doc chat mail disk media message meeting sheet smartpage smartsheet calendar todo slide; do
  OUT="$(wecom_run "$d" --help 2>&1)"; d_rc=$?
  if [ "$d_rc" -eq 0 ]; then ok "$d 域可用"; dom_ok=$((dom_ok+1))
  elif [ "$d_rc" -eq 124 ]; then sk "$d 域探测超时(${WECOM_TIMEOUT}s;该 CLI 族已知会挂网络/代理/无 TTY 提示)"
  elif echo "$OUT" | grep -q 'unrecognized subcommand'; then no "$d 域不存在(CLI 无此子命令)"
  elif echo "$OUT" | grep -Eq '权限|暂不支持|未授权'; then sk "$d 域未授权"
  # SKIP 回显同样过 redact:未知退出码正是 CLI 重试/报错时可能把凭证带进输出的路径。
  else sk "$d 域结果未知(exit $d_rc): $(printf '%s' "$OUT" | redact | tr -d '\n' | head -c 60)"; fi
done

echo "[4] 真实读文档(可选,需 WECOM_TEST_DOCID)"
if [ -n "${WECOM_TEST_DOCID:-}" ]; then
  # --docid 是 1.3.4 `doc contents get` 的必填 flag(--help 列出;--dry-run 证与 --json
  # 的 {"docid"} 同一请求体),技能文档只示范了 --json 形态。
  R="$(wecom_run doc contents get --docid "$WECOM_TEST_DOCID" 2>&1)"; r_rc=$?
  # PASS 以退出码 0 为前提:flag/参数漂移时 CLI 打 usage/错误走非零退出,不能因输出
  # 里没有错误字样就把失败输出误判成读成功。成功读取返回 JSON envelope(wecomcli-doc
  # SKILL.md 返回表:url/name/content/file_path/document/version),必含 {;exit 0 的
  # usage 倾倒不含 {,同样不判读成功。只查结构化错误键("error"/"success":false/非零
  # errcode),不再扫正文错误词——正文是用户文档,主题恰为「错误/权限」的合法文档会
  # 误红(与 dingtalk-smoke.sh [4] 同口径)。
  if [ "$r_rc" -eq 0 ] && [ -n "$R" ] && echo "$R" | grep -q '{' \
    && ! echo "$R" | grep -Eqi '"error"|"success"[[:space:]]*:[[:space:]]*false|"errcode"[[:space:]]*:[[:space:]]*[1-9]'; then
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
