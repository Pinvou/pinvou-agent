#!/usr/bin/env bash
# CI 门:改了 submodule(CodeWhale)的 gitlink,就必须**同 PR**更新 docs/fork-modifications.md。
#
# 背景:fork patch 指纹随 patch 走、不拆事后 catch-up PR(见 docs/fork-policy.md)。
# PR #70 漏掉过这一步(gitlink 焊到新 commit 却没登记 fork-modifications + fork-guard 指纹),
# 当时靠人工 review 才补上。这个检查把它自动化:从此 gitlink 一动、忘了登记就直接 fail。
#
# 用法:  ./scripts/ci-fork-link-check.sh [base-ref]    # base 默认 origin/main
# 退出码: 0=通过 / 1=违规(改了 gitlink 没更新 fork-modifications)或基线 diff 失败
set -uo pipefail

BASE="${1:-origin/main}"

# Which files this PR changed vs base (three-dot = the PR's real changes
# from the merge-base). A failed diff (missing base ref, corrupt objects)
# must fail explicitly: this gate is a reverse grep, and an empty diff
# would let both greps miss and pass vacuously — a compliance gate must
# not spin idle.
if ! changed="$(git diff --name-only "${BASE}...HEAD")"; then
  echo "::error::git diff ${BASE}...HEAD failed (base ref missing or objects corrupt); cannot verify the fork compliance linkage."
  exit 1
fi

gl=no; fm=no
printf '%s\n' "$changed" | grep -qx 'CodeWhale'                  && gl=yes
printf '%s\n' "$changed" | grep -qx 'docs/fork-modifications.md'    && fm=yes

if [ "$gl" = yes ] && [ "$fm" = no ]; then
  echo "::error::改了 submodule CodeWhale 的 gitlink,但 docs/fork-modifications.md 未更新。"
  echo "fork patch 指纹必须随 patch 同 PR(见 docs/fork-policy.md)。"
  echo "请在本 PR 内补:fork-modifications.md 条目 + scripts/fork-guard.sh 指纹(+ 跑 ./scripts/fork-guard.sh --fast 自查)。"
  exit 1
fi

echo "fork 合规联动检查通过 (gitlink_changed=${gl}, fork_modifications_changed=${fm})"
