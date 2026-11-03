#!/usr/bin/env bash
# DingTalk dws live smoke test — hits the real DingTalk cloud; read-only-first with safe defaults (creates
# no documents, sends no messages).
#
# Usage:
#   bash scripts/dingtalk-smoke.sh
#   DINGTALK_TEST_DOC_ID=<doc_id> bash scripts/dingtalk-smoke.sh   # additionally read one existing document for real
#
# Exit codes: non-zero when any hard check (version/connection/optional doc read) fails, so it can hang off CI
# (but QR-code authorization is needed first, hence manual runs by default);
# exit 3 when every domain probe was skipped — "not verified" is not "verified".
set -u
# Under set -u, an unset HOME would blow up as an unbound variable; give an actionable exit code 2 here instead.
if [ -z "${HOME:-}" ]; then echo "HOME 未设置,无法定位 dws"; exit 2; fi
# Resolve the data root the same way the app does (paths.rs pinvou3_home): PINVOU3_HOME can relocate the data
# root wholesale; honoring only $HOME/.pinvou3 would miss relocated installs — the app is connected while the
# smoke reports "CLI not found".
PINVOU3_ROOT="${PINVOU3_HOME:-$HOME/.pinvou3}"

pass=0; fail=0; skip=0; dom_ok=0
ok(){ echo "  [PASS] $1"; pass=$((pass+1)); }
no(){ echo "  [FAIL] $1"; fail=$((fail+1)); }
sk(){ echo "  [SKIP] $1"; skip=$((skip+1)); }
# Redact failure echo (same semantics as host-side connector_cli.rs safe_auth_log_line): any line containing
# access_token/refresh_token/authorization/bearer (matched after lowercasing the whole line, case-insensitive)
# is masked in full — equals signs, bare colons, URL query strings, and uppercase scheme forms all covered
# (the host does full-line replacement for exactly these forms); remaining lines then mask token values by
# JSON field shape. Normal dws auth status output contains no credentials; this is defense in depth — guarding
# against a future CLI output-shape change smuggling secrets into FAIL lines.
# Reviewer addition: generalized token assignment shapes (api_token=…, bare token: …, token = …) are likewise
# masked in full — previously only the four host markers were recognized, so bare assignment keys containing
# "token" leaked into failure excerpts.
redact(){ awk '{ l=tolower($0)
    if (l ~ /access_token|refresh_token|authorization|bearer/ || l ~ /[a-z0-9_-]*token[a-z0-9_-]*[[:space:]]*[=:]/) print "[已脱敏:含凭证形状的整行]"
    else print }' | sed -E 's/[A-Za-z0-9_-]*[Tt][Oo][Kk][Ee][Nn][A-Za-z0-9_-]*"?:[[:space:]]*"[^"]*"/token=[REDACTED]/g'; }

# --- Locate dws ---
# Resolution order (reviewer correction, app-managed assets first): the smoke must verify the dws the app
# installed from connectors.lock.json and hash-verified, not a coincidentally same-named binary on PATH —
# a stale/malicious PATH entry could shadow the lock-pinned asset and make the smoke pass "another binary".
# Order: app version directory → legacy layout remnants → npm global shim → PATH only as an explicit dev
# fallback (loudly annotated when selected; the [1] version floor still gates it).
# The app lays dws down from connectors.lock.json into the version directory <data root>/assets/cli/dws/<version>/
# (paths.rs assets_cli_dir; the data root supports PINVOU3_HOME relocation); the legacy layout
# <data root>/connectors/<platform>/bin/ is migrated and cleaned up by native_installer,
# kept only as a remnant fallback.
CLI=""
# Native platform → platform directory name and executable name in connectors.lock (connector_lock
# executable_name); probe only the native platform so a shared HOME never picks another platform's binary.
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
  # With multiple coexisting versions, take the highest (numeric per-segment sort, no reliance on sort -V);
  # accept only all-numeric dotted directories, so renamed remnant directories like 1.0.99.bak cannot
  # impersonate a higher version. Candidates accumulate in a plain loop and sort afterwards: macOS's bundled
  # bash 3.2 parses $() bodies only at execution time and cannot parse "case nested inside $()" (the pattern's
  # `)` would terminate the command substitution) — the documented usage is bash scripts/dingtalk-smoke.sh,
  # which must work on 3.2 (bash -n passes on both versions; only actually running 3.2 exposes it, verified in practice).
  ver=""
  for d in "$PINVOU3_ROOT"/assets/cli/dws/*/; do
    db="${d%/}"; db="${db##*/}"
    case "$db" in *[!0-9.]*) continue ;; esac
    # Must also contain a dot: bare date directories (e.g. 20261027) are not versions, and picking one would
    # shadow a healthy install next to it (the downstream version probe would FAIL, but point at the wrong
    # directory, making diagnosis painful).
    case "$db" in *.*) : ;; *) continue ;; esac
    # -f excludes "a directory that happens to be named dws": [ -x ] is also true for directories, and after
    # picking one, [1] would misreport PASS by reading the path inside the shell error text as a version number.
    [ -f "$d$EXE" ] || continue
    [ -x "$d$EXE" ] || continue
    ver="$ver$db
"
  done
  ver="$(printf '%s' "$ver" | sort -t. -k1,1n -k2,2n -k3,3n | tail -1)"
  [ -n "$ver" ] && CLI="$PINVOU3_ROOT/assets/cli/dws/$ver/$EXE"
fi
[ -z "$CLI" ] && [ -n "$PLAT" ] && [ -f "$PINVOU3_ROOT/connectors/$PLAT/bin/$EXE" ] && [ -x "$PINVOU3_ROOT/connectors/$PLAT/bin/$EXE" ] && CLI="$PINVOU3_ROOT/connectors/$PLAT/bin/$EXE"
[ -z "$CLI" ] && [ -n "${APPDATA:-}" ] && [ -f "$APPDATA/npm/dws.cmd" ] && CLI="$APPDATA/npm/dws.cmd"
[ -z "$CLI" ] && [ -f "$HOME/AppData/Roaming/npm/dws.cmd" ] && CLI="$HOME/AppData/Roaming/npm/dws.cmd"
if [ -z "$CLI" ]; then
  # PATH only as a dev fallback (reviewer correction): everything above is app-managed on-disk locations;
  # reaching this point means the machine has no app asset, and only a dev-machine direct install should use
  # the same-named binary from PATH — loudly annotated as to its source, since it is not the lock-pinned
  # asset; the [1] version floor check still gates it.
  CLI="$(command -v dws 2>/dev/null || true)"
  [ -n "$CLI" ] && echo "注意: 使用 PATH 上的 dws(开发回退,非应用管理的 lock 钉扎资产): $CLI"
fi
if [ -z "$CLI" ]; then echo "找不到 dws(先在 Pinvou Agent 工具面板连接钉钉,应用会在线下载并校验)"; exit 2; fi

# Probes must be time-bounded: connector_cli.rs run_probe uses a 30s kill-tree for this CLI family — the
# npm-shim CLI was observed in practice hanging indefinitely on network/proxy/no-TTY prompts, and a smoke
# script that hangs without exiting is worse than red. Use coreutils timeout when present; on bare macOS
# fall back to a bash watchdog. The watchdog subshell must be redirected to /dev/null: this function is
# always called inside $( ), so the subshell and its inner sleep inherit the command substitution's pipe
# write end; after a fast probe finishes, the orphaned sleep still holds the pipe and $() waits for it to
# sleep out the full 30s before seeing EOF (healthy path measured at a full 9×30s); pointed at /dev/null,
# a fast probe returns at once.
# Non-interactive contract (reviewer addition): the CLI's stdin is /dev/null — probes are all read-only
# --version/auth/--help surfaces needing no input; a CLI inheriting the caller's stdin could consume the
# caller's input, or wait on a no-TTY prompt until timeout (the host process helper applies the same
# null-stdin contract to child processes).
# CLI output goes to a temp file first and is replayed afterwards, never written straight into the $( )
# pipe: some member of a killed process tree may always hold the pipe write end, and a grandchild holding
# the pipe drags the whole script out until it exits on its own (measured 45s+ and even >3min); once on
# disk, the grandchild holds only the temp file, the pipe hits EOF when cat ends, and the script returns
# within the limit.
DWS_TIMEOUT=30
# TERM is not the end: a CLI that swallows TERM (stuck cleanup routine, no-TTY prompter intercepting signals)
# must not turn the probe into a permanent wait. GNU timeout without -k keeps waiting after the child ignores
# TERM; a watchdog that sends TERM only once likewise blocks wait forever. Both tiers escalate to SIGKILL after
# KILL_GRACE and normalize 137 (KILL death) to 124 — the timeout diagnostic branch keeps working; a timeout
# must not be disguised as "outcome unknown".
DWS_KILL_GRACE=5
TIMEOUT_BIN="$(command -v timeout 2>/dev/null || true)"
dws_run() {
  local t rc tflag
  # When mktemp fails (e.g. TMPDIR points at an unwritable/nonexistent directory), t is empty and tflag
  # degrades to the relative path .deadline in the caller's working directory; the plain rm at the end would
  # delete the caller's same-named file (reviewer reproduced in practice); the empty-value guard on the EXIT
  # backstop trap saves only the trap itself, not the plain path. An allocation failure/empty value must
  # early-return before spawning any cleanup path or starting any command — this probe ends as a failure
  # (mktemp's own diagnostics are already on stderr and enter the failure echo via $()'s 2>&1) instead of
  # touching the caller's directory.
  t="$(mktemp "${TMPDIR:-/tmp}/dws-smoke.XXXXXX")" || t=""
  if [ -z "$t" ]; then
    echo "mktemp 失败:探测临时文件分配不出来(检查 TMPDIR)" >&2
    return 1
  fi
  tflag="${t}.deadline"
  # The backstop cleanup hangs on the probe subshell that owns the temp file, never on the outer script: this
  # function is always invoked via $( ), the DWS_TMP assignment lives only inside the subshell, and the outer
  # shell always sees an empty value — cleanup on the outer EXIT would expand to rm -f "" ".deadline" and
  # delete the caller's same-named file (reviewer reproduced in practice; the empty-value guard also blocks
  # the same shape where DWS_TMP is empty after mktemp failure). When SIGINT is delivered to the same process
  # group, the subshell runs its own EXIT before dying (verified the same on bash 3.2.57 and 5.x), taking the
  # real capture file with it; SIGKILL cannot help, so the leak surface is a single file in $TMPDIR. Raw CLI
  # output has not been through redact and must not sit in $TMPDIR for anyone to browse.
  DWS_TMP="$t"
  trap 'if [ -n "$DWS_TMP" ]; then rm -f "$DWS_TMP" "${DWS_TMP}.deadline" 2>/dev/null; fi' EXIT
  if [ -n "$TIMEOUT_BIN" ]; then
    # GNU timeout only monitors its direct child: on deadline it TERMs the CLI,
    # and once that leader exits — compliantly on TERM, or via the -k KILL —
    # timeout returns without following up on the rest of the group, so a
    # TERM-resistant grandchild keeps running past the probe (reviewer
    # reproduced on GNU coreutils 8.32). Give the CLI tree its own process
    # group here too (set -m around the backgrounded timeout: the group id is
    # then the timeout pid, which the CLI and its descendants inherit), and
    # after a deadline-fired 124 re-run the same escalation the watchdog branch
    # owns instead of trusting timeout's narrower contract. Do not pass
    # --foreground: that exempts the child tree from the deadline entirely.
    set -m
    "$TIMEOUT_BIN" -k "$DWS_KILL_GRACE" "$DWS_TIMEOUT" "$CLI" "$@" </dev/null >"$t" 2>&1 &
    local tpid=$!
    set +m
    wait "$tpid" 2>/dev/null; rc=$?
    # Escalation ownership must not die with the leader (reviewer P2): timeout
    # returns as soon as its direct child is gone, so on a 124 sweep the group
    # ourselves. The kill -0 probe on the negative pid gates the sweep: an
    # empty group (compliant TERM death, or -k already killed everything)
    # returns immediately, while a surviving grandchild gets TERM, one
    # KILL_GRACE to exit, then the group KILL — the same TERM->KILL chain as
    # the watchdog branch. Platforms whose kill cannot address negative pids
    # fail the probe and keep plain timeout -k behavior (no weaker than
    # before); the sweep runs while the group demonstrably exists, matching
    # the watchdog's PID-reuse exposure window.
    if [ "$rc" -eq 124 ] && kill -0 "-$tpid" 2>/dev/null; then
      kill -TERM "-$tpid" 2>/dev/null || kill -TERM "$tpid" 2>/dev/null
      sleep "$DWS_KILL_GRACE"
      kill -KILL "-$tpid" 2>/dev/null || kill -KILL "$tpid" 2>/dev/null
    fi
  else
    # Process-group ownership contract (reviewer correction): set -m starts the CLI as leader of its own new
    # process group so the watchdog's group signals pierce the wrapper (@wecom/cli's npm wrapper is an
    # execFileSync shape) and the native grandchild it spawns — killing only the direct child would let the
    # credential-holding grandchild outlive the timeout. set +m restores immediately: this function always
    # runs in a $( ) subshell, so -m only needs to cover this one background spawn; process-group membership
    # is fixed at spawn time, so turning it off afterwards does not affect group signals.
    set -m
    "$CLI" "$@" </dev/null >"$t" 2>&1 &
    local pid=$!
    set +m
    # Drop the "deadline reached" marker as soon as TERM goes out: when the child receives TERM, cleans up
    # gracefully, and exits 0, wait's 0 would disguise the timeout as a pass (GNU timeout returns 124 for the
    # same shape) — the deadline fact must be independent of the child's final exit code, and the marker's
    # presence always reports 124. The marker lands before TERM: the shape where the child exits on its own in
    # the micro-window between deadline and TERM no longer misses the timeout. Before dropping the marker,
    # confirm the capture file still exists: after the parent shell is interrupted by a signal and cleans up,
    # the orphaned watchdog must not conjure a .deadline out of nothing (interrupt remnant flagged by the
    # reviewer); while the parent shell is still in wait, the capture is necessarily there and timing
    # semantics are unchanged.
    # TERM/KILL both go to the whole group (kill -TERM "-$pid": negative pid = process group; the old-style
    # form without --, supported since bash 3.2). When group signals are unavailable (MSYS2 support for
    # negative pids varies, exotic kernels), fall back to a single-process kill — the contract degrades to
    # equivalence with the old behavior, no weaker than before.
    ( sleep "$DWS_TIMEOUT" && [ -e "$t" ] && : >"$tflag" || exit 0
      kill -TERM "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null
      sleep "$DWS_KILL_GRACE"
      kill -KILL "-$pid" 2>/dev/null || kill -KILL "$pid" 2>/dev/null ) >/dev/null 2>&1 &
    local wpid=$!
    wait "$pid" 2>/dev/null; rc=$?
    # Escalation chain must stay alive (reviewer P2): the deadline marker has landed = the watchdog is already
    # on its TERM→KILL escalation chain, and the parent shell must not kill it now — when the leader dies on
    # TERM while a TERM-resistant grandchild survives, wait returns early, and the old shape's kill $wpid
    # would cut down the watchdog just before its group KILL, so the escalation chain's final link never
    # fires and the credential-holding grandchild outlives the timeout. Instead, escort the watchdog until it
    # has sent the group KILL and exits on its own (bounded: at most one more KILL_GRACE of waiting); cancel
    # only on the normal-completion path (marker absent).
    if [ -e "$tflag" ]; then
      wait "$wpid" 2>/dev/null
    else
      kill "$wpid" 2>/dev/null; wait "$wpid" 2>/dev/null
    fi
    [ -e "$tflag" ] && rc=124
  fi
  # Normalize timeout shapes: TERM death (143) and KILL death (137) both report as 124.
  [ "$rc" -eq 143 ] && rc=124
  [ "$rc" -eq 137 ] && rc=124
  cat "$t"; rm -f "$t" "$tflag"
  return "$rc"
}
echo "dws = $CLI"
echo

MIN_TRI="1.0.64"  # dws SKILL.md cli_version floor; connector_online_install_contract.test.js three-way pins SKILL.md and connectors.lock.json — any drift goes red
echo "[1] --version runs and is >= ${MIN_TRI} (dws SKILL.md cli_version floor; lock pins 1.0.65)"
# Collect both stdout and stderr (same semantics as [2] auth; some builds print the version on stderr). The
# version number is taken only from the line containing the CLI name, and only after the name: the same-family
# lark-cli was observed printing "a new version 1.0.96 is available" *before* the version line (feishu.rs pins
# that fixture), and dws's auto-update check also writes stderr — "first dotted number in the whole text"
# would read the update number or an IP inside a proxy URL as the local version and let a below-floor CLI
# pass by mistake. DWS_NO_UPDATE_CHECK=1 also switches off the update check if the CLI honors that variable
# (no side effect when it does not). Accept only dotted version numbers (stricter than connector_cli.rs
# parse_semver3: that one splits on any non-digit, so a bare digit run like the build date 20261027 reads as
# 20261027.0.0 and passes; the dotted requirement here excludes that naturally), pad to three segments with
# zeros, no sort -V. Known residual misread shape: a line with the name before the number (e.g.
# `dws mirror 2.5.1 …`) would still be treated as the version line — never observed on the real CLI, and the
# cost of anchoring it (word-position semantics) outweighs the benefit.
export DWS_NO_UPDATE_CHECK=1
VER="$(dws_run --version 2>&1)"; v_rc=$?
TRI="$(printf '%s\n' "$VER" | awk 'tolower($0) ~ /dws/ { for (i = 1; i <= NF; i++) { if (seen && $i ~ /^[0-9]+(\.[0-9]+)+$/) { print $i; exit } if (tolower($i) ~ /dws/) seen = 1 } }' | awk -F. '{printf "%d.%d.%d", $1, $2+0, $3+0}')"
# Per-segment numeric comparison (same keys as the directory sort above; weighted 1*10000+... would misjudge
# 0.99.164 >= 1.0.64): TRI >= MIN_TRI iff the first line of the two lines sorted ascending is MIN_TRI itself.
GE="$(printf '%s\n%s\n' "$TRI" "$MIN_TRI" | sort -t. -k1,1n -k2,2n -k3,3n | head -1 | grep -qxF "$MIN_TRI" && echo yes || echo no)"
# PASS presumes the probe itself succeeded (v_rc=0): healthy-looking output plus a non-zero exit (including
# timeout 124) means "the CLI ran but failed"; output alone must not earn PASS — exit 7 plus a normal version
# line was observed going all green in practice.
if [ "$v_rc" -eq 0 ] && [ -n "$TRI" ]; then
  ok "--version 可执行: $(printf '%s\n' "$VER" | redact | tr -d '\r' | head -1 | cut -c 1-200)"
  if [ "$GE" = yes ]; then
    ok "version >= ${MIN_TRI}: $TRI"
  else
    no "version below ${MIN_TRI} (below the SKILL.md cli_version floor): $TRI"
  fi
else
  # Timeout (124) and "could not run/could not parse" are handled differently: the former is this CLI family's
  # known hang shape (see the DWS_TIMEOUT comment below) and calls for rerunning/checking the proxy, not
  # "scan to auth first".
  if [ "$v_rc" -eq 124 ]; then
    no "--version 探测超时(${DWS_TIMEOUT}s+${DWS_KILL_GRACE}s 宽限;该 CLI 族已知会挂网络/代理/无 TTY 提示): $(printf '%s' "$VER" | redact | tr -d '\n' | head -c 120)"
  else
    no "--version 失败或无法解析: $(printf '%s' "$VER" | redact | tr -d '\n' | head -c 120)"
  fi
fi

echo "[2] 连接状态(auth status --format json 的 authenticated=true = 已授权)"
# Same semantics as dingtalk.rs: only the official JSON's authenticated:true counts; both stdout and stderr
# are inspected; strip \r.
AUTH="$(dws_run auth status --format json 2>&1)"; a_rc=$?
# The dingtalk.rs semantics are "top-level authenticated == true"; grep has no notion of trees, so an extra
# "authenticated:false must not also appear" clause excludes nested false truths (e.g. {"authenticated":false,
# "detail":{"authenticated":true}}). Same semantics as [1]: require the probe to exit 0 first — output shape
# does not vouch for a failure (exit 7 + authenticated:true is a failure, not connected).
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
# Domain = dws top-level service; --help touches no network and needs no auth. Decide by exit code — Cobra
# also prints a "Run '… --help' for usage." line for unknown commands, so grepping Usage would misreport
# nonexistent domains as available. Probe only top-level services: for an unknown leaf under a group command
# Cobra prints the group help and exits 0, and leaf-level probing cannot tell true from false.
# The timeout (124) check precedes the error words (same as wecom-smoke [3], which pins the branch order): a
# timed-out probe's partial output may contain not found/unknown — checking words first would mislabel a
# timeout as a hard "domain missing" failure; the two are handled differently, a timeout calls for a rerun,
# and the labeling must not mislead.
for d in doc drive calendar todo chat sheet aitable; do
  OUT="$(dws_run "$d" --help 2>&1)"; d_rc=$?
  if [ "$d_rc" -eq 0 ]; then ok "$d 域可用"; dom_ok=$((dom_ok+1))
  elif [ "$d_rc" -eq 124 ]; then sk "$d 域探测超时(${DWS_TIMEOUT}s+${DWS_KILL_GRACE}s 宽限;该 CLI 族已知会挂网络/代理/无 TTY 提示)"
  elif echo "$OUT" | grep -Eqi 'unknown command|not found|不存在|未知'; then no "$d 域不存在(CLI 无此服务)"
  # The SKIP echo goes through redact too: an unknown exit code is exactly the path where CLI retries/errors
  # may carry credentials into the output.
  else sk "$d 域结果未知(exit $d_rc): $(printf '%s' "$OUT" | redact | tr -d '\n' | head -c 60)"; fi
done

echo "[4] 真实读文档(可选,需 DINGTALK_TEST_DOC_ID)"
if [ -n "${DINGTALK_TEST_DOC_ID:-}" ]; then
  R="$(dws_run doc read --node "$DINGTALK_TEST_DOC_ID" --format json 2>&1)"; r_rc=$?
  # PASS presumes exit code 0: on flag drift Cobra prints usage and exits non-zero, and usage must not be
  # misjudged as a successful read just because the output lacks error words. A successful read under
  # --format json must contain a JSON object (same semantics as dingtalk.rs taking the first { through the
  # last }), and an exit-0 usage/help dump contains no { so it likewise does not count as a successful read.
  # Only structured error keys are checked ("error"/"success":false), no longer scanning body text for error
  # words — the body is the user's document, and a legitimate document whose topic happens to be
  # "errors/permissions" would go red by mistake.
  # Error keys are checked in the envelope only, never in body values: the body is the user's document, and a
  # legitimate document whose value is exactly "error" would go red under a bare grep. The masked keys are the
  # body-carrier keys the bundled docs actually pin: global-reference.md's --format json section pins the
  # envelope as {"success": true, "body": {...}} (body is the body carrier), error-codes.md pins the error
  # shape {"success": false, "code", "message"},
  # attendance_report_common.py pins the success/result/error unpacking contract, the --fields doc pins the
  # top-level envelope keys data/result/success/status, and doc-read.md pins the --content-format jsonml
  # payload {revision, jsonml} — the docs do not agree with each other, so the body-like keys are masked
  # together: content (same family and key as wecom-smoke [4]) +body+result+data+jsonml. String values are
  # masked whole under JSON semantics (\" does not terminate a value) before checking error keys; masking
  # does not touch key names or booleans, and envelope-level error keys ("error" key / "success":false /
  # "status":"error" value shape) keep working.
  # Residual (conservative): bare string elements inside a jsonml array are not key:value shaped and cannot
  # be masked — an array element that is exactly "error" still goes red (a bare grep's "error" is assembled
  # from the delimiting quotes).
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
