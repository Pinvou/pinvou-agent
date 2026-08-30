#!/usr/bin/env bash
# 诊断专用(不合并到 main):rust-test 测试腿在 hosted runner 上反复
# "lost communication",一旦失联整份 job 日志被 GitHub 删除,无法取证。
#
# v1 结论(26b359c60 的 run 33255201715):20s 采样间隔内死亡,最后快照
# 内存/磁盘/负载全部健康(MemAvailable=14.6GB),20s 粒度不足以区分
# 「瞬时内存炸弹/写回风暴把 VM 打到假死」与「Runner.Worker 被杀/断网」。
#
# v2 改进:
#   1. 1s 分辨率本地采样 MemAvailable/Swap/Dirty/Writeback/load/线程数/
#      PSI(memory+io)/top 进程 → /tmp/ci-observe/mem-1s.log(纯本地,不依赖网络)
#   2. sudo dmesg 环形缓冲 tail → /tmp/ci-observe/dmesg.tail(OOM killer 证据)
#   3. cargo 输出经 ci-tee-unbuffered.py chunk 级无缓冲落盘(消除 tee 的
#      4KB 块缓冲,精确到「进行中」的测试)
#   4. 每 10s 把以上三类尾部 PATCH 到临时 Check Run;runner 死亡后最后
#      一次 PATCH 仍保留在 GitHub 侧,可还原死亡前逐秒现场。
#
# 任何一步失败都只影响诊断本身,绝不影响被观测的测试腿。
set -u

: "${GITHUB_REPOSITORY:?}" "${GITHUB_TOKEN:?}" "${TELEMETRY_SHA:?}"

CHECK_NAME="rust-test-live-telemetry"
OBSERVE_DIR="/tmp/ci-observe"
MEM_LOG="$OBSERVE_DIR/mem-1s.log"
DMESG_TAIL="$OBSERVE_DIR/dmesg.tail"
CARGO_LOG="$OBSERVE_DIR/cargo.log"

mkdir -p "$OBSERVE_DIR"

# 机器身份(失联后日志被删,只能靠 PATCH 留证):runner 镜像版本/内核/boot_id。
# 用于判别死亡是否与特定 runner 镜像 rollout 相关。
{
  echo "ImageOS=${ImageOS:-?} ImageVersion=${ImageVersion:-?}"
  echo "uname: $(uname -a)"
  echo "boot_id: $(cat /proc/sys/kernel/random/boot_id 2>/dev/null)"
  echo "nproc: $(nproc) MemTotal: $(awk '/MemTotal/ {print int($2/1024)"MB"}' /proc/meminfo)"
} > "$OBSERVE_DIR/machine.log"

# 幂等:探针 job 跨 step 重复启动时只保留一个实例(每个实例会新建 Check Run)。
if [ -f "$OBSERVE_DIR/telemetry.pid" ] && kill -0 "$(cat "$OBSERVE_DIR/telemetry.pid")" 2>/dev/null; then
  exit 0
fi
echo $$ >"$OBSERVE_DIR/telemetry.pid"

: >"$MEM_LOG"

# --- 1s 本地采样器(无网络依赖,死亡前最后一秒也有数据) ---
(
  while :; do
    ts=$(date -u +%H:%M:%S)
    read -r avail swapf dirty wb < <(awk '
      /^MemAvailable:/ {a=int($2/1024)}
      /^SwapFree:/     {s=int($2/1024)}
      /^Dirty:/        {d=int($2/1024)}
      /^Writeback:/    {w=int($2/1024)}
      END {print a, s, d, w}' /proc/meminfo)
    load=$(cut -d' ' -f1 /proc/loadavg)
    thr=$(ps -eL --no-headers 2>/dev/null | wc -l)
    psi_mem=$(awk '/^some/ {print $2}' /proc/pressure/memory 2>/dev/null)
    psi_io=$(awk '/^some/ {print $2}' /proc/pressure/io 2>/dev/null)
    top=$(ps -eo rss=,comm= --sort=-rss 2>/dev/null | head -3 | awk '{printf "%s=%dMB ", $2, $1/1024}')
    printf '%s avail=%sMB swap=%sMB dirty=%sMB wb=%sMB load=%s thr=%s %s %s | %s\n' \
      "$ts" "$avail" "$swapf" "$dirty" "$wb" "$load" "$thr" "$psi_mem" "$psi_io" "$top" \
      >>"$MEM_LOG"
    sleep 1
  done
) &
sampler_pid=$!

# --- dmesg tail(OOM killer / 内核异常证据;sudo 不可用时静默降级) ---
sudo sh -c "while :; do dmesg 2>/dev/null | tail -6 > '$DMESG_TAIL.tmp' && mv '$DMESG_TAIL.tmp' '$DMESG_TAIL'; sleep 2; done" &
dmesg_pid=$!

cleanup() {
  kill "$sampler_pid" "$dmesg_pid" 2>/dev/null || true
}
trap cleanup EXIT

# --- Check Run PATCH 循环 ---
check_id=$(gh api "repos/$GITHUB_REPOSITORY/check-runs" \
  -X POST \
  -f name="$CHECK_NAME" \
  -f head_sha="$TELEMETRY_SHA" \
  -f status="in_progress" \
  --jq '.id' 2>/dev/null) || exit 0
[ -n "${check_id:-}" ] && [ "$check_id" != "null" ] || exit 0

while :; do
  last_mem=$(tail -1 "$MEM_LOG" 2>/dev/null)
  summary="$(date -u +%H:%M:%S) PATCH
--- machine ---
$(cat "$OBSERVE_DIR/machine.log" 2>/dev/null)
--- 1s sampler (last 12) ---
$(tail -12 "$MEM_LOG" 2>/dev/null)
--- cargo tail ---
$(tail -c 1000 "$CARGO_LOG" 2>/dev/null | sed 's/\x1b\[[0-9;]*m//g' | tail -6)
--- dmesg tail ---
$(cat "$DMESG_TAIL" 2>/dev/null)
--- ab probe ---
$(tail -25 "$OBSERVE_DIR/ab.log" 2>/dev/null)
--- strace tail ---
$(tail -15 "$OBSERVE_DIR/strace.log" 2>/dev/null)"
  gh api "repos/$GITHUB_REPOSITORY/check-runs/$check_id" \
    -X PATCH \
    -f "output[title]=${last_mem:-starting}" \
    -f "output[summary]=$summary" \
    >/dev/null 2>&1 || true
  sleep 10
done
