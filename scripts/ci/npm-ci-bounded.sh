#!/usr/bin/env bash
# Bounded `npm ci` for runners where GNU coreutils `timeout` is not on PATH.
#
# The Ubuntu lanes keep the plain `timeout --kill-after=15 <secs> npm ci`
# prefix. macOS runner images ship no coreutils `timeout` (Homebrew exposes
# only `gtimeout`), and on the Windows PowerShell lanes `timeout` resolves to
# System32's unrelated sleep utility, which rejects the GNU flags with
# "Invalid syntax" and fails the leg before npm even starts. Both host classes
# call this helper under `bash` instead: where `timeout` exists it is used with
# the same --kill-after discipline as the Ubuntu lanes; where it does not, the
# step degrades to the job's timeout-minutes cap with a loud annotation rather
# than dying with exit 127 (or 1) at the first build step.
#
# One bound per invocation line, mirroring the NpmCiBoundsPolicyTests contract:
# every npm ci lane in CI carries a per-leg bound so a wedged registry fails
# the leg, not the job window.
set -euo pipefail
secs="${1:?usage: npm-ci-bounded.sh <seconds> [extra npm ci args...]}"
shift
if command -v timeout >/dev/null 2>&1; then
  exec timeout --kill-after=15 "$secs" npm ci --prefer-offline --no-audit "$@"
fi
echo "::warning::GNU timeout unavailable on $(uname -s); npm ci runs under this job's timeout-minutes cap only"
exec npm ci --prefer-offline --no-audit "$@"
