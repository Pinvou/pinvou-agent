#!/bin/bash
# DeepSeek-TUI 原生启动 — 测试机本地 vLLM + Qwen3.6-35B-A3B-FP8
#
# 用途：对照实验。同样的本地模型 + 同样的任务，跑 DeepSeek-TUI 原生模式，
# 看 plan/agent mode 在小模型下表现，再跟 pinvou-platform 编排层版本对比。
#
# 进入 TUI 后：
#   /mode plan     — 先 plan 再执行（只读，强制规划）
#   /mode agent    — 自主执行 + 写操作要审批
#   /mode yolo     — 全自动（跟 pinvou-platform 当前默认行为对齐）
#   /help          — 查看所有命令
#
# env 配置跟 run-local.sh 完全一致（都是 deepseek-tui 自己识别的 env）。

# vLLM provider（OpenAI 兼容 /v1/chat/completions）
export DEEPSEEK_PROVIDER=vllm
export DEEPSEEK_API_KEY="local-no-auth"
export DEEPSEEK_BASE_URL="http://10.214.74.113:8000/v1"
export DEEPSEEK_MODEL="/model"

# 关 Qwen3 thinking 模式（reasoning_effort=off 触发
# chat_template_kwargs.enable_thinking=false，避 10+ 秒 reasoning 段卡死）
export DEEPSEEK_REASONING_EFFORT=off

# 内网 HTTP 推理服务，绕过 deepseek-tui 默认 HTTPS 强制
export DEEPSEEK_ALLOW_INSECURE_HTTP=1

# 内网代理 HTTP/2 ALPN 协商有时卡死，强制 HTTP/1.1
export DEEPSEEK_FORCE_HTTP1=1

# vLLM max-model-len=65536，engine 默认 max_output_tokens=64000 会撞顶
export DEEPSEEK_MAX_OUTPUT_TOKENS=16384

# 工作目录设在 pinvou3 项目根，TUI 启动后能直接读取项目里的 markdown 文档
cd "$(dirname "$0")"

# 跑 deepseek-tui binary（不走 cli wrapper，少一层 delegate）
# --yolo 跟 run-local.sh 的 auto_approve=true 行为对齐，方便对比
# 想要审批流：去掉 --yolo，改 --approval-policy on-request
exec cargo run \
  --manifest-path DeepSeek-TUI/Cargo.toml \
  --bin deepseek-tui \
  --release \
  -- \
  --yolo \
  "$@"
