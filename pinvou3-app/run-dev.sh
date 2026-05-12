#!/bin/bash
# pinvou3-app dev 启动脚本
#
# 集中处理 Linux 下 Tauri / webkit2gtk 跟输入法、显示协议的兼容性。

set -euo pipefail

# ── 1. 后端 env（vLLM + Qwen3.6） ───────────────────────────────
# 复用项目根 run-deepseek-tui.sh 的 DEEPSEEK_* 配置
PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source <(grep '^export' "$PROJECT_ROOT/run-deepseek-tui.sh")

# ── 2. 输入法兼容（fcitx5 / ibus 在 Wayland 下跟 webkit 兼容差） ─
# 强制 GTK 走 X11 (XWayland)，webkit 通过 XIM 协议跟 fcitx5 协作
# 比 Wayland 文本输入协议稳定。
export GDK_BACKEND=x11
# webkit2gtk 在某些 Wayland 合成器下渲染异常（DMA-BUF），关掉合成模式
export WEBKIT_DISABLE_COMPOSITING_MODE=1
# 确保 IM 变量传给子进程（默认 inherited，但显式列出方便调试）
export GTK_IM_MODULE="${GTK_IM_MODULE:-fcitx}"
export QT_IM_MODULE="${QT_IM_MODULE:-fcitx}"
export XMODIFIERS="${XMODIFIERS:-@im=fcitx}"

# ── 3. 启动 Tauri dev ─────────────────────────────────────────
cd "$(dirname "$0")"
echo "[run-dev] starting tauri dev (GDK_BACKEND=$GDK_BACKEND)"
exec npx tauri dev "$@"
