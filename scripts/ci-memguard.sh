#!/usr/bin/env bash
# rust-test CI 内存看门狗。
#
# 背景:ubuntu-latest hosted runner 只有 16GB 内存。rust-test 的测试二进制
# 链接(700+ crate 的 ThinLTO,lld 单线程)是数分钟的内存高峰,历史上多次把
# runner 打到与 GitHub 失联("The hosted runner lost communication with the
# server");一旦失联,该 job 的日志全部丢失,无法取证是编译、链接还是测试
# 执行耗尽了内存。
#
# 本脚本在后台以 5s 间隔输出 MemAvailable / 磁盘剩余 / RSS 前三进程的时间
# 序列;当可用内存跌破阈值时抢先 KILL cargo/rustc/lld/测试二进制,把
# 「runner 失联零日志」转化为「step 失败但日志完整」,保住诊断现场。
#
# 用法:在 cargo step 内后台启动,step 结束时 kill 掉:
#   bash scripts/ci-memguard.sh &
#   guard_pid=$!
#   cargo test ...
#   kill "$guard_pid" 2>/dev/null || true
set -u

THRESHOLD_KB=${MEMGUARD_THRESHOLD_KB:-1572864} # 默认 1.5 GiB

while :; do
  avail_kb=$(awk '/MemAvailable/ {print $2}' /proc/meminfo)
  disk_free=$(df -h / | awk 'NR==2 {print $4}')
  top=$(ps -eo rss=,comm= --sort=-rss | head -3 | awk '{printf "%s=%dMB ", $2, $1/1024}')
  echo "$(date -u +%H:%M:%S) MemAvailable=$((avail_kb / 1024))MB disk_free=${disk_free} top: ${top}"
  if [ "${avail_kb}" -lt "${THRESHOLD_KB}" ]; then
    echo "MEMGUARD: MemAvailable 跌破 $((THRESHOLD_KB / 1024))MB,抢先杀掉 cargo/rustc/lld/测试进程以保住 runner"
    pkill -KILL -f 'ld\.lld' || true
    pkill -KILL -x rustc || true
    pkill -KILL -x cargo || true
    pkill -KILL -f 'pinvou3_lib-' || true
    exit 42
  fi
  sleep 5
done
