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
# the leg, not the job window — including the bridge-prep lanes inside
# prepare-codex-bridge-runtime.sh (bash) and codex-bridge.js (spawnSync
# timeout), which the workflow-level pins cannot see (r12 review).
set -euo pipefail
secs="${1:?usage: npm-ci-bounded.sh <seconds> [extra npm ci args...]}"
shift
# Probe GNU-ness, not mere existence (r12 review): Windows images put
# System32's `timeout.exe` on PATH ahead of Git's coreutils, and it rejects
# `--version` — so an existence-only `command -v` probe could silently select
# the exact "Invalid syntax" failure this helper exists to prevent if the
# image PATH order changes. GNU timeout answers `--version`; System32's
# errors, sending us to the degrade branch.
if timeout --version >/dev/null 2>&1; then
  exec timeout --kill-after=15 "$secs" npm ci --prefer-offline --no-audit "$@"
fi
echo "::warning::GNU timeout unavailable on $(uname -s); npm ci runs without a per-leg bound (under CI the job's timeout-minutes cap still applies)"
exec npm ci --prefer-offline --no-audit "$@"
