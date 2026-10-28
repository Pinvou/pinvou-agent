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
# 失败回显脱敏:任何 *token* 字段值 / authorization 头 / bearer 方案先打码再截断
# (与 host 侧 connector_cli.rs safe_auth_log_line 同族口径)。dws auth status 的
# 正常输出不含凭证,这是纵深防御——防未来 CLI 输出形状变化把密钥带进 FAIL 行。
redact(){ sed -E 's/[A-Za-z_-]*[Tt][Oo][Kk][Ee][Nn][A-Za-z_-]*"?:[[:space:]]*"[^"]*"/token=[REDACTED]/g
s/[Aa]uthorization"?:[[:space:]]*"?[^",}]+/authorization=[REDACTED]/g
s/[Bb]earer[[:space:]][A-Za-z0-9._~+/=-]+/bearer [REDACTED]/g'; }

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
    [ -x "$d$EXE" ] || continue
    ver="$ver$db
"
  done
  ver="$(printf '%s' "$ver" | sort -t. -k1,1n -k2,2n -k3,3n | tail -1)"
  [ -n "$ver" ] && CLI="$HOME/.pinvou3/assets/cli/dws/$ver/$EXE"
fi
[ -z "$CLI" ] && [ -n "$PLAT" ] && [ -x "$HOME/.pinvou3/connectors/$PLAT/bin/$EXE" ] && CLI="$HOME/.pinvou3/connectors/$PLAT/bin/$EXE"
[ -z "$CLI" ] && [ -n "${APPDATA:-}" ] && [ -f "$APPDATA/npm/dws.cmd" ] && CLI="$APPDATA/npm/dws.cmd"
[ -z "$CLI" ] && [ -f "$HOME/AppData/Roaming/npm/dws.cmd" ] && CLI="$HOME/AppData/Roaming/npm/dws.cmd"
if [ -z "$CLI" ]; then echo "找不到 dws(先在 Pinvou Agent 工具面板连接钉钉,应用会在线下载并校验)"; exit 2; fi

# 探测必须限时:connector_cli.rs run_probe 对同一族命令用 30s kill-tree——npm-shim
# CLI 曾实测卡在网络/代理/无 TTY 提示上无限挂起,冒烟脚本挂着不退出比红更糟。
# 有 coreutils timeout 用之;macOS 裸机退化为 bash 看门狗。看门格子壳必须重定向
# 到 /dev/null:本函数总在 $( ) 里被调用,子壳及其内层 sleep 会继承命令替换的
# 管道写端,快探针结束后孤儿 sleep 仍握着管道,$() 要等它睡满 30s 才见到 EOF
# (实测健康路径被拖满 9×30s);指向 /dev/null 后快探针即刻返回(孤儿 sleep
# ≤30s 自行退出)。kill 只打直接子进程:对「native 二进制 / exec node 的 npm
# shim」即全杀;若 CLI 是 fork 孙进程再 wait 的 wrapper,孙进程可能存活到自行
# 退出(本脚本现有发现形态均非该类;coreutils timeout 分支无此问题)。
DWS_TIMEOUT=30
if command -v timeout >/dev/null 2>&1; then
  dws_run() { timeout "$DWS_TIMEOUT" "$CLI" "$@"; }
else
  dws_run() {
    "$CLI" "$@" &
    local pid=$!
    ( sleep "$DWS_TIMEOUT" && kill "$pid" 2>/dev/null ) >/dev/null 2>&1 &
    local wpid=$!
    wait "$pid" 2>/dev/null; local rc=$?
    kill "$wpid" 2>/dev/null; wait "$wpid" 2>/dev/null
    [ "$rc" -eq 143 ] && rc=124
    return "$rc"
  }
fi
echo "dws = $CLI"
echo

MIN_TRI="1.0.64"  # dws SKILL.md cli_version floor;connector_online_install_contract.test.js 与 SKILL.md、connectors.lock.json 三方钉扎,漂移即红
echo "[1] --version runs and is >= ${MIN_TRI} (dws SKILL.md cli_version floor; lock pins 1.0.65)"
# stdout/stderr 都收(与 [2] auth 同口径,个别发行把版本打在 stderr)。只认点分版本号
# (比 connector_cli.rs parse_semver3 更严:那边按任意非数字切分,裸数字串如构建日期
# 20261027 会读成 20261027.0.0 放行;此处点分要求天然排除,不足三段补 0,不用 sort -V)。
VER="$(dws_run --version 2>&1 || true)"
TRI="$(printf '%s\n' "$VER" | grep -oE '[0-9]+(\.[0-9]+)+' | head -1 | awk -F. '{printf "%d.%d.%d", $1, $2+0, $3+0}')"
# 逐段数值比较(与上面目录排序同键;加权 1*10000+... 会把 0.99.164 误判 >= 1.0.64):
# TRI >= MIN_TRI 当且仅当两行升序排序后的首行就是 MIN_TRI。
GE="$(printf '%s\n%s\n' "$TRI" "$MIN_TRI" | sort -t. -k1,1n -k2,2n -k3,3n | head -1 | grep -qxF "$MIN_TRI" && echo yes || echo no)"
if [ -n "$TRI" ]; then
  ok "--version 可执行: $(echo "$VER" | tr -d '\r' | head -1)"
  if [ "$GE" = yes ]; then
    ok "version >= ${MIN_TRI}: $TRI"
  else
    no "version below ${MIN_TRI} (below the SKILL.md cli_version floor): $TRI"
  fi
else
  no "--version 失败或无法解析: $(printf '%s' "$VER" | redact | tr -d '\n' | head -c 120)"
fi

echo "[2] 连接状态(auth status --format json 的 authenticated=true = 已授权)"
# 与 dingtalk.rs 同口径:只认官方 JSON 的 authenticated:true,stdout/stderr 都看;剥 \r。
AUTH="$(dws_run auth status --format json 2>&1 || true)"
# dingtalk.rs 同口径是「顶层 authenticated == true」;grep 没有树的概念,加一条
# 「不得同时出现 authenticated:false」排除嵌套假真(如 {"authenticated":false,
# "detail":{"authenticated":true}})。
if echo "$AUTH" | tr -d '\r' | grep -Eq '"authenticated"[[:space:]]*:[[:space:]]*true' \
  && ! echo "$AUTH" | tr -d '\r' | grep -Eq '"authenticated"[[:space:]]*:[[:space:]]*false'; then
  ok "已连接(auth status authenticated=true)"
else
  no "未连接/未授权(先扫码): $(printf '%s' "$AUTH" | redact | tr -d '\n' | head -c 160)"
fi

echo "[3] 服务域抽样探测(代表性子集,只读 --help;存在的服务 exit 0,不存在则 exit 1)"
# 域=dws 顶层服务,--help 不触网、不需授权。用退出码判定——Cobra 对未知命令也会打
# "Run '… --help' for usage." 行,grep Usage 会把不存在的域误报可用。只探顶层服务:
# Cobra 对组命令下的未知叶子会直接打印组帮助并 exit 0,叶子级探测分不出真假。
for d in doc drive calendar todo chat sheet aitable; do
  OUT="$(dws_run "$d" --help 2>&1)"; d_rc=$?
  if [ "$d_rc" -eq 0 ]; then ok "$d 域可用"; dom_ok=$((dom_ok+1))
  elif echo "$OUT" | grep -Eqi 'unknown command|not found|不存在|未知'; then no "$d 域不存在(CLI 无此服务)"
  else sk "$d 域结果未知(exit $d_rc): $(echo "$OUT" | tr -d '\n' | head -c 60)"; fi
done

echo "[4] 真实读文档(可选,需 DINGTALK_TEST_DOC_ID)"
if [ -n "${DINGTALK_TEST_DOC_ID:-}" ]; then
  R="$(dws_run doc read --node "$DINGTALK_TEST_DOC_ID" --format json 2>&1)"; r_rc=$?
  # PASS 以退出码 0 为前提:flag 漂移时 Cobra 打 usage 走非零退出,
  # 不能因输出里没有错误字样就把 usage 误判成读成功。--format json 下成功读取
  # 必含 JSON 对象(dingtalk.rs 取首个 { 到末个 } 同口径),exit 0 的 usage/help
  # 倾倒不含 {,同样不判读成功。
  if [ "$r_rc" -eq 0 ] && [ -n "$R" ] && echo "$R" | grep -q '{' \
    && ! echo "$R" | grep -Eqi '"error"|"success"[[:space:]]*:[[:space:]]*false|错误|失败|权限'; then
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
