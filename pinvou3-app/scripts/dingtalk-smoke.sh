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
# set -u 下 HOME 未设置会直接炸成 unbound variable;这里给可行动的退出码 2。
if [ -z "${HOME:-}" ]; then echo "HOME 未设置,无法定位 dws"; exit 2; fi

pass=0; fail=0; skip=0; dom_ok=0
ok(){ echo "  [PASS] $1"; pass=$((pass+1)); }
no(){ echo "  [FAIL] $1"; fail=$((fail+1)); }
sk(){ echo "  [SKIP] $1"; skip=$((skip+1)); }
# 失败回显脱敏(host 侧 connector_cli.rs safe_auth_log_line 同口径):凡出现
# access_token/refresh_token/authorization/bearer(整行小写后匹配,大小写不敏感)
# 的行整行打码——等号、裸冒号、URL 查询串、大写 scheme 形态一并覆盖(host 对
# 这些形态就是整行替换);其余行再按 JSON 字段形态打码 token 值。dws auth status
# 的正常输出不含凭证,这是纵深防御——防未来 CLI 输出形状变化把密钥带进 FAIL 行。
redact(){ awk '{ l=tolower($0)
    if (l ~ /access_token|refresh_token|authorization|bearer/) print "[已脱敏:含凭证形状的整行]"
    else print }' | sed -E 's/[A-Za-z0-9_-]*[Tt][Oo][Kk][Ee][Nn][A-Za-z0-9_-]*"?:[[:space:]]*"[^"]*"/token=[REDACTED]/g'; }

# --- 定位 dws ---
# 应用按 connectors.lock.json 把 dws 落到版本目录 ~/.pinvou3/assets/cli/dws/<version>/
# (paths.rs assets_cli_dir);旧布局 ~/.pinvou3/connectors/<platform>/bin/ 会被
# native_installer 迁移并清理,只作存量回退;再退到 npm 全局 shim。
CLI="$(command -v dws 2>/dev/null || true)"
# 本机平台 → connectors.lock 的平台目录名与可执行名(connector_lock executable_name);
# 只探本机平台,避免共享 HOME 时误选别的平台的二进制。
case "$(uname -s 2>/dev/null)" in
  MINGW*|MSYS*|CYGWIN*) PLAT=windows-x64; EXE=dws.exe ;;
  Linux)
    case "$(uname -m 2>/dev/null)" in
      aarch64|arm64) PLAT=linux-arm64 ;;
      *) PLAT=linux-x64 ;;
    esac
    EXE=dws ;;
  Darwin)
    case "$(uname -m 2>/dev/null)" in
      arm64) PLAT=darwin-arm64 ;;
      *) PLAT=darwin-x64 ;;
    esac
    EXE=dws ;;
  *) PLAT=""; EXE=dws ;;
esac
if [ -z "$CLI" ]; then
  # 多版本并存时取最高版本(按数字段排序,不依赖 sort -V);只认纯数字点分目录,
  # 排除 1.0.99.bak 这类改名残留目录冒充高版本。候选在普通循环里累积后再排序:
  # macOS 自带 bash 3.2 在执行期才解析 $() 内文,解析不了「$() 内嵌 case」
  # (pattern 的 `)` 会终止命令替换)——文档化用法是 bash scripts/dingtalk-smoke.sh,
  # 必须兼容 3.2(bash -n 两版都过,只有真跑 3.2 才暴露,已实测)。
  ver=""
  for d in "$HOME"/.pinvou3/assets/cli/dws/*/; do
    db="${d%/}"; db="${db##*/}"
    case "$db" in *[!0-9.]*) continue ;; esac
    # 还须含点:裸日期目录(如 20261027)不是版本号,选它会遮住旁边的健康安装
    # (下游版本探测虽会 FAIL,但报错指向错误目录,排查费劲)。
    case "$db" in *.*) : ;; *) continue ;; esac
    # -f 排除「名字恰好叫 dws 的目录」:[ -x ] 对目录也为真,选中后 [1] 会把
    # shell 报错文本里的路径当版本号误报 PASS。
    [ -f "$d$EXE" ] || continue
    [ -x "$d$EXE" ] || continue
    ver="$ver$db
"
  done
  ver="$(printf '%s' "$ver" | sort -t. -k1,1n -k2,2n -k3,3n | tail -1)"
  [ -n "$ver" ] && CLI="$HOME/.pinvou3/assets/cli/dws/$ver/$EXE"
fi
[ -z "$CLI" ] && [ -n "$PLAT" ] && [ -f "$HOME/.pinvou3/connectors/$PLAT/bin/$EXE" ] && [ -x "$HOME/.pinvou3/connectors/$PLAT/bin/$EXE" ] && CLI="$HOME/.pinvou3/connectors/$PLAT/bin/$EXE"
[ -z "$CLI" ] && [ -n "${APPDATA:-}" ] && [ -f "$APPDATA/npm/dws.cmd" ] && CLI="$APPDATA/npm/dws.cmd"
[ -z "$CLI" ] && [ -f "$HOME/AppData/Roaming/npm/dws.cmd" ] && CLI="$HOME/AppData/Roaming/npm/dws.cmd"
if [ -z "$CLI" ]; then echo "找不到 dws(先在 Pinvou Agent 工具面板连接钉钉,应用会在线下载并校验)"; exit 2; fi

# 探测必须限时:connector_cli.rs run_probe 对同一族命令用 30s kill-tree——npm-shim
# CLI 曾实测卡在网络/代理/无 TTY 提示上无限挂起,冒烟脚本挂着不退出比红更糟。
# 有 coreutils timeout 用之;macOS 裸机退化为 bash 看门狗。看门狗子壳必须重定向
# 到 /dev/null:本函数总在 $( ) 里被调用,子壳及其内层 sleep 会继承命令替换的
# 管道写端,快探针结束后孤儿 sleep 仍握着管道,$() 要等它睡满 30s 才见到 EOF
# (实测健康路径被拖满 9×30s);指向 /dev/null 后快探针即刻返回。
# CLI 输出先落临时文件再回放,不直接写 $( ) 管道:两个分支的 kill 都只打得到
# 直接子进程(GNU timeout 默认也只杀子进程),若 CLI 是 spawn 孙进程再 wait 的
# wrapper(@wecom/cli 的 npm 包装器就是 execFileSync 形态),孙进程会握着管道
# 写端把整段脚本拖到它自行退出(实测 45s+ 乃至 >3min);落盘后孙进程只握临时
# 文件,管道随 cat 结束即 EOF,脚本在时限内返回。
DWS_TIMEOUT=30
# TERM 不是终点:吞掉 TERM 的 CLI(清理例程卡住、无 TTY 提示器拦信号)不能把探测
# 变成永久等待。GNU timeout 不带 -k 会在子进程无视 TERM 后一直等;看门狗只发一次
# TERM 同样让 wait 永久阻塞。两级都在 KILL_GRACE 后升级 SIGKILL,并把 137(KILL 死)
# 归一为 124——超时诊断分支照常生效,不能把超时伪装成「结果未知」。
DWS_KILL_GRACE=5
TIMEOUT_BIN="$(command -v timeout 2>/dev/null || true)"
# Ctrl-C 中断探测时兜底清理当前临时文件(dws_run 尾部自清正常路径;裸 CLI 输出
# 未过 redact,不应留在 $TMPDIR 供人翻看)。
DWS_TMP=""
trap 'rm -f "$DWS_TMP" "${DWS_TMP}.deadline" 2>/dev/null' EXIT
dws_run() {
  local t rc tflag
  t="$(mktemp "${TMPDIR:-/tmp}/dws-smoke.XXXXXX")"
  tflag="${t}.deadline"
  DWS_TMP="$t"
  if [ -n "$TIMEOUT_BIN" ]; then
    "$TIMEOUT_BIN" -k "$DWS_KILL_GRACE" "$DWS_TIMEOUT" "$CLI" "$@" >"$t" 2>&1; rc=$?
  else
    "$CLI" "$@" >"$t" 2>&1 &
    local pid=$!
    # 发出 TERM 即落「到点」标记:子进程收到 TERM 后优雅清理并以 0 退出时,wait
    # 的 0 会把超时伪装成通过(GNU timeout 同形态本就返回 124)——到点事实必须
    # 独立于子进程最终退出码,标记存在则一律按 124 上报。标记先于 TERM 落盘:
    # 子进程恰在到点与 TERM 之间的微窗内自行退出的形态不再漏判超时。
    ( sleep "$DWS_TIMEOUT" && : >"$tflag" && kill "$pid" 2>/dev/null && sleep "$DWS_KILL_GRACE" && kill -9 "$pid" 2>/dev/null ) >/dev/null 2>&1 &
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
echo "dws = $CLI"
echo

MIN_TRI="1.0.64"  # dws SKILL.md cli_version floor;connector_online_install_contract.test.js 与 SKILL.md、connectors.lock.json 三方钉扎,漂移即红
echo "[1] --version runs and is >= ${MIN_TRI} (dws SKILL.md cli_version floor; lock pins 1.0.65)"
# stdout/stderr 都收(与 [2] auth 同口径,个别发行把版本打在 stderr)。版本号只从
# 含 CLI 名字的那一行、且位于名字之后取:同族 lark-cli 实测会在版本行*之前*打
# 「a new version 1.0.96 is available」(feishu.rs 钉有该夹具),dws 的自动更新检查
# 也写 stderr——「全文首个点分数字」会把更新号/代理 URL 里的 IP 读成本机版本,
# 让低于下限的 CLI 误 PASS。DWS_NO_UPDATE_CHECK=1 若 CLI 认该变量则顺手关掉
# 更新检查(不认该变量时无副作用)。只认点分版本号(比 connector_cli.rs parse_semver3
# 更严:那边按任意非数字切分,裸数字串如构建日期 20261027 会读成 20261027.0.0
# 放行;此处点分要求天然排除),不足三段补 0,不用 sort -V。残余已知误读形状:
# 名字在数字之前的行(如 `dws mirror 2.5.1 …`)仍会被当成版本行——真实 CLI 未观测
# 到该形状,锚定它的代价(逐词位置语义)高于收益。
export DWS_NO_UPDATE_CHECK=1
VER="$(dws_run --version 2>&1)"; v_rc=$?
TRI="$(printf '%s\n' "$VER" | awk 'tolower($0) ~ /dws/ { for (i = 1; i <= NF; i++) { if (seen && $i ~ /^[0-9]+(\.[0-9]+)+$/) { print $i; exit } if (tolower($i) ~ /dws/) seen = 1 } }' | awk -F. '{printf "%d.%d.%d", $1, $2+0, $3+0}')"
# 逐段数值比较(与上面目录排序同键;加权 1*10000+... 会把 0.99.164 误判 >= 1.0.64):
# TRI >= MIN_TRI 当且仅当两行升序排序后的首行就是 MIN_TRI。
GE="$(printf '%s\n%s\n' "$TRI" "$MIN_TRI" | sort -t. -k1,1n -k2,2n -k3,3n | head -1 | grep -qxF "$MIN_TRI" && echo yes || echo no)"
# PASS 前提是探测本身成功(v_rc=0):健康形状的输出 + 非零退出(含超时 124)是
# 「CLI 跑了但失败」,不能因输出像样就发 PASS——实测 exit 7 + 正常版本行曾全绿。
if [ "$v_rc" -eq 0 ] && [ -n "$TRI" ]; then
  ok "--version 可执行: $(printf '%s\n' "$VER" | redact | tr -d '\r' | head -1 | cut -c 1-200)"
  if [ "$GE" = yes ]; then
    ok "version >= ${MIN_TRI}: $TRI"
  else
    no "version below ${MIN_TRI} (below the SKILL.md cli_version floor): $TRI"
  fi
else
  # 超时(124)与「跑不动/解析不了」的处置不同:前者是这一族 CLI 已知的挂起形态
  # (见下方 DWS_TIMEOUT 注释),提示重跑/查代理,不是「先扫码」。
  if [ "$v_rc" -eq 124 ]; then
    no "--version 探测超时(${DWS_TIMEOUT}s+${DWS_KILL_GRACE}s 宽限;该 CLI 族已知会挂网络/代理/无 TTY 提示): $(printf '%s' "$VER" | redact | tr -d '\n' | head -c 120)"
  else
    no "--version 失败或无法解析: $(printf '%s' "$VER" | redact | tr -d '\n' | head -c 120)"
  fi
fi

echo "[2] 连接状态(auth status --format json 的 authenticated=true = 已授权)"
# 与 dingtalk.rs 同口径:只认官方 JSON 的 authenticated:true,stdout/stderr 都看;剥 \r。
AUTH="$(dws_run auth status --format json 2>&1)"; a_rc=$?
# dingtalk.rs 同口径是「顶层 authenticated == true」;grep 没有树的概念,加一条
# 「不得同时出现 authenticated:false」排除嵌套假真(如 {"authenticated":false,
# "detail":{"authenticated":true}})。与 [1] 同口径:先要求探测退出 0,输出形状
# 不为失败背书(exit 7 + authenticated:true 是失败,不是已连接)。
if [ "$a_rc" -eq 0 ] && echo "$AUTH" | tr -d '\r' | grep -Eq '"authenticated"[[:space:]]*:[[:space:]]*true' \
  && ! echo "$AUTH" | tr -d '\r' | grep -Eq '"authenticated"[[:space:]]*:[[:space:]]*false'; then
  ok "已连接(auth status authenticated=true)"
else
  if [ "$a_rc" -eq 124 ]; then
    no "auth 探测超时(${DWS_TIMEOUT}s+${DWS_KILL_GRACE}s 宽限;该 CLI 族已知会挂网络/代理/无 TTY 提示,不是未授权): $(printf '%s' "$AUTH" | redact | tr -d '\n' | head -c 160)"
  elif [ "$a_rc" -ne 0 ]; then
    no "auth 探测失败(exit $a_rc;CLI 自身报错,不是未授权): $(printf '%s' "$AUTH" | redact | tr -d '\n' | head -c 160)"
  else
    no "未连接/未授权(先扫码): $(printf '%s' "$AUTH" | redact | tr -d '\n' | head -c 160)"
  fi
fi

echo "[3] 服务域抽样探测(代表性子集,只读 --help;存在的服务 exit 0,不存在则 exit 1)"
# 域=dws 顶层服务,--help 不触网、不需授权。用退出码判定——Cobra 对未知命令也会打
# "Run '… --help' for usage." 行,grep Usage 会把不存在的域误报可用。只探顶层服务:
# Cobra 对组命令下的未知叶子会直接打印组帮助并 exit 0,叶子级探测分不出真假。
# 超时(124)检查先于错误词(与 wecom-smoke [3] 同款,那边有分支序钉):超时探测的
# 部分输出可能恰含 not found/未知——先查词会把超时误标成「域不存在」硬失败;
# 两者处置不同,超时要求的是重跑,标注不能误导。
for d in doc drive calendar todo chat sheet aitable; do
  OUT="$(dws_run "$d" --help 2>&1)"; d_rc=$?
  if [ "$d_rc" -eq 0 ]; then ok "$d 域可用"; dom_ok=$((dom_ok+1))
  elif [ "$d_rc" -eq 124 ]; then sk "$d 域探测超时(${DWS_TIMEOUT}s+${DWS_KILL_GRACE}s 宽限;该 CLI 族已知会挂网络/代理/无 TTY 提示)"
  elif echo "$OUT" | grep -Eqi 'unknown command|not found|不存在|未知'; then no "$d 域不存在(CLI 无此服务)"
  # SKIP 回显同样过 redact:未知退出码正是 CLI 重试/报错时可能把凭证带进输出的路径。
  else sk "$d 域结果未知(exit $d_rc): $(printf '%s' "$OUT" | redact | tr -d '\n' | head -c 60)"; fi
done

echo "[4] 真实读文档(可选,需 DINGTALK_TEST_DOC_ID)"
if [ -n "${DINGTALK_TEST_DOC_ID:-}" ]; then
  R="$(dws_run doc read --node "$DINGTALK_TEST_DOC_ID" --format json 2>&1)"; r_rc=$?
  # PASS 以退出码 0 为前提:flag 漂移时 Cobra 打 usage 走非零退出,
  # 不能因输出里没有错误字样就把 usage 误判成读成功。--format json 下成功读取
  # 必含 JSON 对象(dingtalk.rs 取首个 { 到末个 } 同口径),exit 0 的 usage/help
  # 倾倒不含 {,同样不判读成功。只查结构化错误键("error"/"success":false),
  # 不再扫正文错误词——正文是用户文档,主题恰好是「错误/权限」的合法文档会误红。
  # 错误键只查 envelope、不查正文值:正文是用户文档,值为恰为 error 的合法文档会
  # 被裸 grep 误红。屏蔽键取包内文档实际钉到的正文载体键:global-reference.md 的
  # --format json 节钉 envelope 为 {"success": true, "body": {...}}(body 为正文
  # 载体),error-codes.md 钉错误形态 {"success": false, "code", "message"},
  # attendance_report_common.py 钉 success/result/error 拆包契约,--fields 文档钉
  # 顶层信封键 data/result/success/status,doc-read.md 钉 --content-format jsonml
  # 载荷 {revision, jsonml}——各文档间并不一致,故正文类键一并屏蔽:
  # content(与 wecom-smoke [4] 同族同键)+body+result+data+jsonml。字符串值按
  # JSON 语义(\" 不终止值)整体屏蔽后再查错误键;屏蔽不碰键名与布尔值,envelope
  # 层的错误键("error" 键/"success":false/"status":"error" 值形态)照常生效。
  # 残余(保守向):jsonml 数组内的裸字符串元素不呈 key:value 形态,屏蔽不到——
  # 数组元素恰为 error 时仍会误红(裸 grep 的 "error" 由定界引号拼出)。
  R_ENVELOPE="$(printf '%s' "$R" | sed -E 's/"(content|body|result|data|jsonml)"[[:space:]]*:[[:space:]]*"(\\.|[^"\\])*"/"\1":"[正文已屏蔽]"/g')"
  if [ "$r_rc" -eq 0 ] && [ -n "$R" ] && echo "$R" | grep -q '{' \
    && ! printf '%s\n' "$R_ENVELOPE" | grep -Eqi '"error"|"success"[[:space:]]*:[[:space:]]*false'; then
    ok "读到文档内容($(printf '%s' "$R" | wc -c | tr -d ' ') 字节)"
  else
    no "读文档失败(exit $r_rc): $(printf '%s' "$R" | redact | tr -d '\n' | head -c 160)"
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
