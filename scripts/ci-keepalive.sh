#!/usr/bin/env bash
# rust-test CI 保命采样循环(诊断分支 v18 修复候选,勿直接合入 main)。
#
# 背景:restore 暖 rust-cache(target/)后,ubuntu-latest hosted runner 会在
# 测试二进制起跑瞬间被宿主层蒸发("lost communication",日志全删)。经 v12-v17
# 单变量实验定位:在编译腿开头启动「每秒一次的本地采样循环」可 100% 避免失联
# (有则 18/18 活,无则 14/14 死);必要件 = 每秒 ps 全进程表遍历 + loadavg/PSI
# 读取的组合,网络流量(POST/PATCH)已被证明无关(probe-localonly 零网络存活)。
# 机理不明(宿主层行为,仓库内不可证实),本脚本把已验证的最小保护集固化下来。
set -u

OBSERVE_DIR="/tmp/ci-observe"
LOG="$OBSERVE_DIR/keepalive.log"
mkdir -p "$OBSERVE_DIR"

# 幂等:跨 step 重复启动时只保留一个实例(实例会跨 step 存活到 job 结束)。
if [ -f "$OBSERVE_DIR/keepalive.pid" ] && kill -0 "$(cat "$OBSERVE_DIR/keepalive.pid")" 2>/dev/null; then
  exit 0
fi
echo $$ >"$OBSERVE_DIR/keepalive.pid"

while :; do
  ts=$(date -u +%H:%M:%S)
  load=$(cut -d' ' -f1 /proc/loadavg)
  psi_mem=$(awk '/^some/ {print $2}' /proc/pressure/memory 2>/dev/null)
  psi_io=$(awk '/^some/ {print $2}' /proc/pressure/io 2>/dev/null)
  thr=$(ps -eL --no-headers 2>/dev/null | wc -l)
  top=$(ps -eo rss=,comm= --sort=-rss 2>/dev/null | head -3 | awk '{printf "%s=%dMB ", $2, $1/1024}')
  printf '%s load=%s thr=%s %s %s | %s\n' \
    "$ts" "$load" "$thr" "$psi_mem" "$psi_io" "$top" >>"$LOG"
  sleep 1
done
