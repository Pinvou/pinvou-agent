#!/bin/bash
# pinvou3-app dev 启动脚本
#
# 集中处理 Linux 下 Tauri / webkit2gtk 跟输入法、显示协议的兼容性。

set -euo pipefail

# ── 1. 后端 env（vLLM + Qwen3.6） ───────────────────────────────
# vLLM provider (OpenAI 兼容 /v1/chat/completions)
export DEEPSEEK_PROVIDER=vllm
export DEEPSEEK_API_KEY="local-no-auth"
export DEEPSEEK_BASE_URL="http://10.214.74.113:8000/v1"
# 2026-05-18 起 vLLM served-model-name 从 /model 改成 qwen36_35b
# (跟 main 上 commit ad95782 model 同步保持一致)
export DEEPSEEK_MODEL="qwen36_35b"

# 关 Qwen3 thinking 模式 (reasoning_effort=off 触发
# chat_template_kwargs.enable_thinking=false,避免 10+ 秒 reasoning 段卡死)
export DEEPSEEK_REASONING_EFFORT=off

# 内网 HTTP 推理服务,绕过 deepseek-tui 默认 HTTPS 强制
export DEEPSEEK_ALLOW_INSECURE_HTTP=1

# 内网代理 HTTP/2 ALPN 协商有时卡死,强制 HTTP/1.1
export DEEPSEEK_FORCE_HTTP1=1

# vLLM max-model-len=262144 (256K,2026-05 升级,旧值 65536 已过期),
# engine 默认 max_output_tokens=64000 适配 DeepSeek 1M API,本地 Qwen 自托管按需调。
# 16384 是 65K context 时代的安全边界,256K context 下严重保守(只用 6%)。
# 65536 给单次输出 64K budget,本地 Qwen 一次写 600-1000 行 HTML/代码不撞顶;
# 留 200K context 给 input + history + tool schema (30K) 仍宽松。
# 实测来源:嘴替 v2 测试 tetris,16K 撞顶导致 write_file 输出空 content。
export DEEPSEEK_MAX_OUTPUT_TOKENS=65536

# SSE idle timeout:
# 90s 原值只够 prefill 一次大 prompt,但 decode 长 HTML/代码块时 token 间静默可超 90s
# (实测:嘴替 review 后让 AI 一次性写完整 tetris HTML 文件 → 91s 卡断)。
# 240s 给 4 分钟容忍 decode 间隔,vLLM idle 超过这个就真挂了。
# 用户感知:turn 真 hang 时 240s 后报错,正常 decode 间隔不打扰。
export DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS="${DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS:-240}"

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
