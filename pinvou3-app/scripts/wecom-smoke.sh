#!/usr/bin/env bash
# WeCom live smoke test — hits the real WeCom cloud; read-only-first with safe defaults (creates no
# documents, sends no messages).
#
# Usage:
#   bash scripts/wecom-smoke.sh
#   WECOM_TEST_DOCID=<docid> bash scripts/wecom-smoke.sh   # additionally read one existing document for real
#
# Exit codes: non-zero when any hard check (version/connection/optional doc read) fails, so it can hang off CI
# (but QR-code authorization is needed first, hence manual runs by default);
# exit 3 when every domain probe was skipped (transient first call) — "not verified" is not "verified".
set -u
# Under set -u, an unset HOME would blow up as an unbound variable; give an actionable exit code 2 here instead.
if [ -z "${HOME:-}" ]; then echo "HOME 未设置,无法定位 wecom-cli"; exit 2; fi
# Resolve the data root the same way the app does (paths.rs pinvou3_home, same semantics as dingtalk-smoke.sh):
# PINVOU3_HOME can relocate the data root wholesale; honoring only $HOME/.pinvou3 would miss relocated installs.
PINVOU3_ROOT="${PINVOU3_HOME:-$HOME/.pinvou3}"

pass=0; fail=0; skip=0; dom_ok=0
ok(){ echo "  [PASS] $1"; pass=$((pass+1)); }
no(){ echo "  [FAIL] $1"; fail=$((fail+1)); }
sk(){ echo "  [SKIP] $1"; skip=$((skip+1)); }
# Redact failure echo (same semantics as host-side connector_cli.rs safe_auth_log_line): any line containing
# access_token/refresh_token/authorization/bearer (matched after lowercasing the whole line, case-insensitive)
# is masked in full — equals signs, bare colons, URL query strings, and uppercase scheme forms all covered
# (the host does full-line replacement for exactly these forms); remaining lines then mask token values by
# JSON field shape. Defense in depth.
# Reviewer addition (same semantics as dingtalk-smoke.sh): generalized token assignment shapes (api_token=…,
# bare token: …, token = …) are likewise masked in full — previously only the four host markers were
# recognized, so bare assignment keys containing "token" leaked into failure excerpts.
redact(){ awk '{ l=tolower($0)
    if (l ~ /access_token|refresh_token|authorization|bearer/ || l ~ /[a-z0-9_-]*token[a-z0-9_-]*[[:space:]]*[=:]/) print "[已脱敏:含凭证形状的整行]"
    else print }' | sed -E 's/[A-Za-z0-9_-]*[Tt][Oo][Kk][Ee][Nn][A-Za-z0-9_-]*"?:[[:space:]]*"[^"]*"/token=[REDACTED]/g'; }

# --- Locate wecom-cli (same semantics as dingtalk-smoke.sh) ---
# Resolution order (reviewer correction, app-managed assets first): the smoke must verify the wecom-cli the
# app installed from connectors.lock.json and hash-verified, not a coincidentally same-named binary on PATH —
# a stale/malicious PATH entry would shadow the lock-pinned asset and make the smoke pass "another binary".
# Order: app version directory → legacy layout remnants → npm global shim → PATH only as an explicit dev
# fallback (loudly annotated when selected; the [1] version floor still gates it).
# The app lays wecom-cli down from connectors.lock.json into the version directory <data root>/assets/cli/wecom-cli/<version>/
# (paths.rs assets_cli_dir; the data root supports PINVOU3_HOME relocation; native_installer uses
# connector_lock::executable_name, with .exe on Windows);
# the legacy layout <data root>/connectors/<platform>/bin/ remains only as a remnant fallback.
# Searching PATH only would miss the CLI the app installed itself and steer the user toward a self-installed
# npm -g copy — exactly the path the wecomcli-shared skill forbids (it bypasses lock pinning and triggers the
# hash-mismatch reinstall loop).
CLI=""
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
  # With multiple coexisting versions, take the highest (per-segment numeric sort, no reliance on sort -V);
  # accept only all-numeric directories containing a dot (excluding renamed remnants like 1.3.4.bak and bare
  # date directories), and require both -f and -x (excluding same-named directories). Candidates accumulate in
  # a plain loop and sort afterwards — stock bash 3.2 cannot parse case nested inside $().
  ver=""
  for d in "$PINVOU3_ROOT"/assets/cli/wecom-cli/*/; do
    db="${d%/}"; db="${db##*/}"
    case "$db" in *[!0-9.]*) continue ;; esac
    case "$db" in *.*) : ;; *) continue ;; esac
    [ -f "$d$EXE" ] || continue
    [ -x "$d$EXE" ] || continue
    ver="$ver$db
"
  done
  ver="$(printf '%s' "$ver" | sort -t. -k1,1n -k2,2n -k3,3n | tail -1)"
  [ -n "$ver" ] && CLI="$PINVOU3_ROOT/assets/cli/wecom-cli/$ver/$EXE"
fi
[ -z "$CLI" ] && [ -n "$PLAT" ] && [ -f "$PINVOU3_ROOT/connectors/$PLAT/bin/$EXE" ] && [ -x "$PINVOU3_ROOT/connectors/$PLAT/bin/$EXE" ] && CLI="$PINVOU3_ROOT/connectors/$PLAT/bin/$EXE"
[ -z "$CLI" ] && [ -n "${APPDATA:-}" ] && [ -f "$APPDATA/npm/wecom-cli.cmd" ] && CLI="$APPDATA/npm/wecom-cli.cmd"
[ -z "$CLI" ] && [ -f "$HOME/AppData/Roaming/npm/wecom-cli" ] && CLI="$HOME/AppData/Roaming/npm/wecom-cli"
if [ -z "$CLI" ]; then
  # PATH only as a dev fallback (reviewer correction, same semantics as dingtalk-smoke.sh): everything above
  # is app-managed on-disk locations; reaching this point means the machine has no app asset, and only a
  # dev-machine direct install should use the same-named binary from PATH — loudly annotated as to its
  # source, since it is not the lock-pinned asset; the [1] version floor check still gates it.
  CLI="$(command -v wecom-cli 2>/dev/null || true)"
  [ -n "$CLI" ] && echo "注意: 使用 PATH 上的 wecom-cli(开发回退,非应用管理的 lock 钉扎资产): $CLI"
fi
if [ -z "$CLI" ]; then echo "找不到 wecom-cli(先在 Pinvou Agent 工具面板连接企业微信,应用会在线下载并校验;开发机自装时放进 PATH)"; exit 2; fi

# Probes must be time-bounded (same semantics as dingtalk-smoke.sh): this CLI family was observed hanging
# indefinitely on network/proxy/no-TTY prompts, and a smoke that hangs without exiting is worse than red.
# Use coreutils timeout when present; otherwise a bash watchdog — the subshell must be redirected to
# /dev/null, else it inherits the $( ) pipe write end and even a fast probe gets dragged out to the full
# limit by the orphaned sleep (healthy path measured at 9×30s; returns at once once redirected).
# Non-interactive contract (reviewer addition, same semantics as dingtalk-smoke.sh): the CLI's stdin is
# /dev/null — probes are all read-only surfaces; a CLI inheriting the caller's stdin could consume the
# caller's input or wait on a no-TTY prompt until timeout.
# CLI output goes to a temp file first and is replayed afterwards: some member of a killed process tree may
# always hold the pipe write end, and @wecom/cli's npm wrapper is an execFileSync (spawn+wait) shape whose
# grandchild holds the pipe write end and stalls the whole script (measured 45s+ and even >3min); once on
# disk, the grandchild holds only the temp file and the pipe hits EOF when cat ends.
WECOM_TIMEOUT=30
# Same semantics as dingtalk-smoke.sh: TERM is not the end, and a TERM-swallowing CLI must not turn the
# probe into a permanent wait. timeout gets -k and the watchdog escalates to SIGKILL after KILL_GRACE;
# 137 (KILL death) and 143 (TERM death) both normalize to 124 so the timeout diagnostic branch keeps working.
WECOM_KILL_GRACE=5
TIMEOUT_BIN="$(command -v timeout 2>/dev/null || true)"
wecom_run() {
  local t rc tflag
  # When mktemp fails (e.g. TMPDIR points at an unwritable/nonexistent directory), t is empty and tflag
  # degrades to the relative path .deadline in the caller's working directory; the plain rm at the end would
  # delete the caller's same-named file (reviewer reproduced in practice); the empty-value guard on the EXIT
  # backstop trap saves only the trap itself, not the plain path. An allocation failure/empty value must
  # early-return before spawning any cleanup path or starting any command — this probe ends as a failure
  # (mktemp's own diagnostics are already on stderr and enter the failure echo via $()'s 2>&1) instead of
  # touching the caller's directory (same semantics as dingtalk-smoke.sh).
  t="$(mktemp "${TMPDIR:-/tmp}/wecom-smoke.XXXXXX")" || t=""
  if [ -z "$t" ]; then
    echo "mktemp 失败:探测临时文件分配不出来(检查 TMPDIR)" >&2
    return 1
  fi
  tflag="${t}.deadline"
  # The backstop cleanup hangs on the probe subshell that owns the temp file, never on the outer script (same
  # semantics as dingtalk-smoke.sh): this function is always invoked via $( ), the DWS_TMP assignment lives
  # only inside the subshell, and the outer shell always sees an empty value — cleanup on the outer EXIT
  # would expand to rm -f "" ".deadline" and delete the caller's same-named file (reviewer reproduced in
  # practice; the empty-value guard also blocks the same shape where DWS_TMP is empty after mktemp failure).
  # When SIGINT is delivered to the same process group, the subshell runs its own EXIT before dying (verified
  # the same on bash 3.2.57 and 5.x), taking the real capture file with it; SIGKILL cannot help, so the leak
  # surface is a single file in $TMPDIR. Raw CLI output has not been through redact and must not sit in
  # $TMPDIR for anyone to browse. The variable name matches dingtalk-smoke.sh on purpose.
  DWS_TMP="$t"
  trap 'if [ -n "$DWS_TMP" ]; then rm -f "$DWS_TMP" "${DWS_TMP}.deadline" 2>/dev/null; fi' EXIT
  if [ -n "$TIMEOUT_BIN" ]; then
    # GNU timeout only monitors its direct child: on deadline it TERMs the CLI,
    # and once that leader exits — compliantly on TERM, or via the -k KILL —
    # timeout returns without following up on the rest of the group, so a
    # TERM-resistant grandchild keeps running past the probe (reviewer
    # reproduced on GNU coreutils 8.32; same shape as dingtalk-smoke.sh).
    # Give the CLI tree its own process group here too (set -m around the
    # backgrounded timeout: the group id is then the timeout pid, which the
    # CLI and its descendants inherit), and after a deadline-fired 124 re-run
    # the same escalation the watchdog branch owns instead of trusting
    # timeout's narrower contract. Do not pass --foreground: that exempts the
    # child tree from the deadline entirely.
    set -m
    "$TIMEOUT_BIN" -k "$WECOM_KILL_GRACE" "$WECOM_TIMEOUT" "$CLI" "$@" </dev/null >"$t" 2>&1 &
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
      sleep "$WECOM_KILL_GRACE"
      kill -KILL "-$tpid" 2>/dev/null || kill -KILL "$tpid" 2>/dev/null
    fi
  else
    # Process-group ownership contract (reviewer correction, same semantics as dingtalk-smoke.sh): set -m
    # starts the CLI as leader of its own new process group so the watchdog's group signals pierce the wrapper
    # (@wecom/cli's npm wrapper is an execFileSync shape) and the native grandchild it spawns — killing only
    # the direct child would let the credential-holding grandchild outlive the timeout. set +m restores
    # immediately: this function always runs in a $( ) subshell, so -m only needs to cover this one background
    # spawn; process-group membership is fixed at spawn time, so turning it off afterwards has no effect.
    set -m
    "$CLI" "$@" </dev/null >"$t" 2>&1 &
    local pid=$!
    set +m
    # Drop the "deadline reached" marker as soon as TERM goes out (same semantics as dingtalk-smoke.sh):
    # when the child receives TERM, cleans up gracefully, and exits 0, wait's 0 would disguise the timeout as
    # a pass — the deadline fact must be independent of the child's final exit code, and the marker's presence
    # always reports 124. The marker lands before TERM: the shape where the child exits on its own in the
    # micro-window between deadline and TERM no longer misses the timeout. Before dropping the marker, confirm
    # the capture file still exists: after the parent shell is interrupted by a signal and cleans up, the
    # orphaned watchdog must not conjure a .deadline out of nothing (interrupt remnant flagged by the
    # reviewer); while the parent shell is still in wait, the capture is necessarily there and timing
    # semantics are unchanged (same semantics).
    # TERM/KILL both go to the whole group (kill -TERM "-$pid": negative pid = process group, the old-style
    # form without --, supported since bash 3.2); when group signals are unavailable, fall back to a
    # single-process kill — no weaker than the old behavior.
    ( sleep "$WECOM_TIMEOUT" && [ -e "$t" ] && : >"$tflag" || exit 0
      kill -TERM "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null
      sleep "$WECOM_KILL_GRACE"
      kill -KILL "-$pid" 2>/dev/null || kill -KILL "$pid" 2>/dev/null ) >/dev/null 2>&1 &
    local wpid=$!
    wait "$pid" 2>/dev/null; rc=$?
    # Escalation chain must stay alive (reviewer P2, same semantics as dingtalk-smoke.sh): the deadline marker
    # has landed = the watchdog is already on its TERM→KILL escalation chain, and the parent shell must not
    # kill it now — when the leader dies on TERM while a TERM-resistant grandchild survives, wait returns
    # early, and the old shape's kill $wpid would cut down the watchdog just before its group KILL, so the
    # escalation chain's final link never fires and the credential-holding grandchild outlives the timeout.
    # Instead, escort the watchdog until it has sent the group KILL and exits on its own (bounded: at most one
    # more KILL_GRACE of waiting); cancel only on the normal-completion path (marker absent).
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
echo "wecom-cli = $CLI"
echo

MIN_WECOM="1.3.4"  # matches wecom.rs WECOM_MIN_VERSION; connector_online_install_contract.test.js pins the literal+comparison shape — any drift goes red
echo "[1] --version runs and is >= ${MIN_WECOM} (matches WECOM_MIN_VERSION in wecom.rs)"
# The version number is taken only from the line containing the CLI name, and only after the name (same
# semantics as dingtalk-smoke.sh; the 1.3.4 banner was measured as the single line
# `wecom-cli 1.3.4 (wecom <build-time> <sha>)`): the same-family lark-cli prints "a new version … is
# available" before the version line, and "first dotted number in the whole text" would read the update
# number as the local version. Accept only dotted version numbers (stricter than wecom.rs
# parse_wecom_version: a bare digit run like a build date would slip through per-segment comparison by
# mistake); pad to three segments with zeros (two segments `2.0` → 2.0.0). No sort -V.
VER="$(wecom_run --version 2>&1)"; v_rc=$?
TRI="$(printf '%s\n' "$VER" | awk 'tolower($0) ~ /wecom-cli/ { for (i = 1; i <= NF; i++) { if (seen && $i ~ /^[0-9]+(\.[0-9]+)+$/) { print $i; exit } if (tolower($i) ~ /wecom-cli/) seen = 1 } }' | awk -F. '{printf "%d.%d.%d", $1, $2+0, $3+0}')"
# Per-segment numeric comparison (same algorithm as dingtalk-smoke.sh; weighted $1*10000+... would misjudge
# 1.2.9999 >= 1.3.4): TRI >= MIN_WECOM iff the first line of the two lines sorted ascending is MIN_WECOM itself.
GE="$(printf '%s\n%s\n' "$TRI" "$MIN_WECOM" | sort -t. -k1,1n -k2,2n -k3,3n | head -1 | grep -qxF "$MIN_WECOM" && echo yes || echo no)"
# PASS presumes the probe itself succeeded (v_rc=0, same semantics as dingtalk-smoke.sh): healthy-looking
# output plus a non-zero exit (including timeout 124) means "the CLI ran but failed"; output alone must not
# earn PASS.
if [ "$v_rc" -eq 0 ] && [ -n "$TRI" ]; then
  ok "--version 可执行: $(printf '%s\n' "$VER" | redact | tr -d '\r' | head -1 | cut -c 1-200)"
  if [ "$GE" = yes ]; then
    ok "version >= ${MIN_WECOM}: $TRI"
  else
    no "version below ${MIN_WECOM} (below the minimum accepted version): $TRI"
  fi
else
  # Timeout (124) and "could not run/could not parse" are handled differently (same semantics as dingtalk-smoke.sh).
  if [ "$v_rc" -eq 124 ]; then
    no "--version 探测超时(${WECOM_TIMEOUT}s+${WECOM_KILL_GRACE}s 宽限;该 CLI 族已知会挂网络/代理/无 TTY 提示): $(printf '%s' "$VER" | redact | tr -d '\n' | head -c 120)"
  else
    no "--version 失败或无法解析: $(printf '%s' "$VER" | redact | tr -d '\n' | head -c 120)"
  fi
fi

echo "[2] 连接状态(auth show --status 输出 authorized = 已授权)"
AUTH="$(wecom_run auth show --status 2>&1)"; a_rc=$?
# Same semantics as wecom.rs status_is_authorized: whole line, case-insensitive; keep stderr and strip \r
# (the Windows npm shim's CRLF output would make a case-sensitive grep -qx misreport "not connected").
# Require the probe to exit 0 first (same semantics as dingtalk-smoke.sh): exit 7 + authorized is a failure,
# not connected.
if [ "$a_rc" -eq 0 ] && echo "$AUTH" | tr -d '\r' | grep -xiq 'authorized'; then
  ok "已连接(auth show --status 返回 authorized)"
else
  if [ "$a_rc" -eq 124 ]; then
    no "auth 探测超时(${WECOM_TIMEOUT}s+${WECOM_KILL_GRACE}s 宽限;该 CLI 族已知会挂网络/代理/无 TTY 提示,不是未授权): $(printf '%s' "$AUTH" | redact | tr -d '\n' | head -c 120)"
  elif [ "$a_rc" -ne 0 ]; then
    no "auth 探测失败(exit $a_rc;CLI 自身报错,不是未授权): $(printf '%s' "$AUTH" | redact | tr -d '\n' | head -c 120)"
  else
    no "未连接/未授权(先扫码): $(printf '%s' "$AUTH" | redact | tr -d '\n' | head -c 120)"
  fi
fi

echo "[3] 各域授权探测(只读 --help;存在的子命令 exit 0,未授权/不存在走非零)"
# Domain = CLI top-level subcommand: the doc-manage skill's commands actually run under the doc domain, and
# chat has a domain but no skill directory. Exit code first: --help touches no network, and exit 0 means the
# subcommand exists and the domain is usable. The timeout (124) check precedes the permission words — a
# timed-out probe's partial output may contain "permissions", and checking words first would mislabel a
# timeout as unauthorized.
# Permission words count as unauthorized only on non-zero exit — the 1.3.4 `doc --help` service descriptions
# were measured to contain "permission management" themselves, so checking words first would forever misreport
# the healthy doc domain (all document skills run on it) as SKIP. Error output also carries a Usage line, and
# grepping Usage would misreport nonexistent domains as available, so the exit code rules here too.
for d in contact doc chat mail disk media message meeting sheet smartpage smartsheet calendar todo slide; do
  OUT="$(wecom_run "$d" --help 2>&1)"; d_rc=$?
  if [ "$d_rc" -eq 0 ]; then ok "$d 域可用"; dom_ok=$((dom_ok+1))
  elif [ "$d_rc" -eq 124 ]; then sk "$d 域探测超时(${WECOM_TIMEOUT}s+${WECOM_KILL_GRACE}s 宽限;该 CLI 族已知会挂网络/代理/无 TTY 提示)"
  elif echo "$OUT" | grep -q 'unrecognized subcommand'; then no "$d 域不存在(CLI 无此子命令)"
  elif echo "$OUT" | grep -Eq '权限|暂不支持|未授权'; then sk "$d 域未授权"
  # The SKIP echo goes through redact too: an unknown exit code is exactly the path where CLI retries/errors
  # may carry credentials into the output.
  else sk "$d 域结果未知(exit $d_rc): $(printf '%s' "$OUT" | redact | tr -d '\n' | head -c 60)"; fi
done

echo "[4] 真实读文档(可选,需 WECOM_TEST_DOCID)"
if [ -n "${WECOM_TEST_DOCID:-}" ]; then
  # --docid is a required flag of 1.3.4 `doc contents get` (listed by --help; --dry-run proves it is the same
  # request body as --json's {"docid"}); the skill docs only demonstrate the --json form.
  R="$(wecom_run doc contents get --docid "$WECOM_TEST_DOCID" 2>&1)"; r_rc=$?
  # PASS presumes exit code 0: on flag/argument drift the CLI prints usage/errors and exits non-zero, and a
  # failure output must not be misjudged as a successful read just because it lacks error words. A successful
  # read returns a JSON envelope (wecomcli-doc SKILL.md return table: url/name/content/file_path/document/
  # version) and must contain {; an exit-0 usage dump contains no { so it likewise does not count as a
  # successful read. Only structured error keys are checked ("error"/"success":false/non-zero errcode), no
  # longer scanning body text for error words — the body is the user's document, and a legitimate document
  # whose topic happens to be "errors/permissions" would go red by mistake (same semantics as dingtalk-smoke.sh [4]).
  # Error keys are checked in the envelope only, never in body values: every string field of the return table
  # (SKILL.md "return" section) carries document data — content is "the original text returned directly when
  # the document is short" (SKILL.md:112), and name/url/file_path are likewise the original document's
  # name/link/on-disk path; a legitimate document whose value is exactly "error" once went red under a bare
  # grep. Body values are masked whole under JSON string semantics (\" does not terminate a value) before
  # checking error keys; masking does not touch key names or booleans, and envelope-level error keys
  # ("error" key / "success":false/non-zero errcode) keep working.
  R_ENVELOPE="$(printf '%s' "$R" | sed -E 's/"(content|name|url|file_path)"[[:space:]]*:[[:space:]]*"(\\.|[^"\\])*"/"\1":"[正文已屏蔽]"/g')"
  if [ "$r_rc" -eq 0 ] && [ -n "$R" ] && echo "$R" | grep -q '{' \
    && ! printf '%s\n' "$R_ENVELOPE" | grep -Eqi '"error"|"success"[[:space:]]*:[[:space:]]*false|"errcode"[[:space:]]*:[[:space:]]*[1-9]'; then
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
