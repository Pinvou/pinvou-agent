#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

# 注:源 workflows/ → bundle 嵌入快照的同步已移入 build.rs(任何 cargo build/打包都同步,
# 不再只覆盖 dev 启动,改完直接 build 也不漂移)。

# ── 工作流预检开关 ───────────────────────────────────────────────
# warmup_check 已对齐 app 配置(endpoint 由 harness 注入 / REQUIRED_ENVS 精简为 base_url),
# 默认启用预检、与客户机一致,尽早暴露 warmup 类问题。要跳过(省 vLLM 冷启动探活 ~30s)
# export PINVOU3_SKIP_WARMUP=1。
export PINVOU3_SKIP_WARMUP="${PINVOU3_SKIP_WARMUP:-0}"

exec npx tauri dev "$@"
