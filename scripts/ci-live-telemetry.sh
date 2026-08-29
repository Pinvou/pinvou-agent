#!/usr/bin/env bash
# 诊断专用(不合并到 main):rust-test 测试腿在 hosted runner 上反复
# "lost communication",一旦失联整份 job 日志被 GitHub 删除,无法取证。
# 本脚本在测试腿后台运行,每 20s 把内存/磁盘/负载/top 进程以及
# /tmp/rust-test-output.log(cargo 输出 tee)的尾部 PATCH 到一个临时
# Check Run。runner 死亡后,最后一次 PATCH 仍保留在 GitHub 侧,
# 可据此还原死亡瞬间的机器状态与正在执行的测试。
#
# 任何一步失败都只影响诊断本身,绝不影响被观测的测试腿。
set -u

: "${GITHUB_REPOSITORY:?}" "${GITHUB_TOKEN:?}" "${TELEMETRY_SHA:?}"

CHECK_NAME="rust-test-live-telemetry"
LOG_FILE="${TELEMETRY_LOG:-/tmp/rust-test-output.log}"

check_id=$(gh api "repos/$GITHUB_REPOSITORY/check-runs" \
  -X POST \
  -f name="$CHECK_NAME" \
  -f head_sha="$TELEMETRY_SHA" \
  -f status="in_progress" \
  --jq '.id' 2>/dev/null) || exit 0
[ -n "${check_id:-}" ] && [ "$check_id" != "null" ] || exit 0

while :; do
  avail_kb=$(awk '/MemAvailable/ {print $2}' /proc/meminfo)
  swap_free_kb=$(awk '/SwapFree/ {print $2}' /proc/meminfo)
  disk_free=$(df -h / | awk 'NR==2 {print $4}')
  loadavg=$(cat /proc/loadavg)
  top=$(ps -eo rss=,comm= --sort=-rss | head -5 | awk '{printf "%s=%dMB ", $2, $1/1024}')
  log_tail=$(tail -c 1200 "$LOG_FILE" 2>/dev/null | sed 's/\x1b\[[0-9;]*m//g' | tail -8)
  summary="$(date -u +%H:%M:%S) MemAvailable=$((avail_kb / 1024))MB SwapFree=$((swap_free_kb / 1024))MB disk_free=${disk_free} load=${loadavg}
top: ${top}
--- cargo tail ---
${log_tail}"
  gh api "repos/$GITHUB_REPOSITORY/check-runs/$check_id" \
    -X PATCH \
    -f "output[title]=$(date -u +%H:%M:%S) MemAvailable=$((avail_kb / 1024))MB disk=${disk_free}" \
    -f "output[summary]=$summary" \
    >/dev/null 2>&1 || true
  sleep 20
done
