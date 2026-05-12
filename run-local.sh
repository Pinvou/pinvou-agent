#!/bin/bash
# pinvou3 启动脚本 — 测试机本地模型（Qwen3.6-35B-A3B-FP8 on 10.214.74.113）
#
# 测试机用 vLLM/OpenAI 兼容协议（/v1/chat/completions），DEEPSEEK_BASE_URL
# 不含 /chat/completions 后缀，由 deepseek-tui 自己拼接。
# 本地推理不校验 API key，传任意非空值即可。

# vLLM 走 OpenAI 兼容协议（/v1/chat/completions）。
# 切 provider 到 vllm：v0.8.30 起，custom base_url 时 vllm provider 也 pass-through
# 自定义 model 字符串（不会被 normalize）。
# 同时 vllm 分支的 apply_reasoning_effort 知道用 chat_template_kwargs.enable_thinking
# 关闭 Qwen3 的 thinking 模式（避免 10+ 秒 reasoning 段把前端"卡死"）。
export DEEPSEEK_PROVIDER=vllm
export DEEPSEEK_API_KEY="local-no-auth"
export DEEPSEEK_BASE_URL="http://10.214.74.113:8000/v1"
export DEEPSEEK_MODEL="/model"
# 关 Qwen3 thinking 模式：reasoning_effort=off 触发
# body["chat_template_kwargs"] = {"enable_thinking": false}
export DEEPSEEK_REASONING_EFFORT=off
# 测试机是内网 HTTP 推理服务，deepseek-tui 默认拒绝非 HTTPS URL。
# 在受信内网用 HTTP 是合理的，显式 override 这个安全检查。
export DEEPSEEK_ALLOW_INSECURE_HTTP=1
# 强制 HTTP/1.1：reqwest 默认会试 HTTP/2 upgrade，但 vLLM 内网通过某些
# 代理时 HTTP/2 ALPN 协商会卡死（SSE 不返回 response headers，45s 超时）。
# HTTP/1.1 兼容性最广，SSE 也完全支持。
export DEEPSEEK_FORCE_HTTP1=1
# EngineHandle 重构暂时回退到 Legacy 路径（自写 tool loop），等架构梳理
# 后再启动。详见 engine-refactor-status.md。
# 想试 Engine 路径：取消下面这行注释。
# export PINVOU_USE_ENGINE_HARNESS=1
# vLLM max-model-len=65536，engine 默认 max_output_tokens=64000 会撞顶。
# 切 Engine 路径时需要这个 env；Legacy 路径不依赖。
export DEEPSEEK_MAX_OUTPUT_TOKENS=16384

cd "$(dirname "$0")"
exec cargo run --manifest-path pinvou-platform/Cargo.toml -- --prompts-dir prompts/ "$@"
