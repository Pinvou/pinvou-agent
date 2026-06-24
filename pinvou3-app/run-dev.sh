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

# ── 远程 vLLM 连接(明文 HTTP)─────────────────────────────────────
# 底座默认拒绝对非 loopback 的明文 http:// 发请求(client.rs validate_base_url_security),
# 且 reqwest 默认协议协商在某些网关下会 502。开发机连内网 GB10(http://10.214.74.113:8000)
# 必须显式放行明文 HTTP + 钉死 HTTP/1.1。可在外部 export 覆盖。
export DEEPSEEK_ALLOW_INSECURE_HTTP="${DEEPSEEK_ALLOW_INSECURE_HTTP:-1}"
export DEEPSEEK_FORCE_HTTP1="${DEEPSEEK_FORCE_HTTP1:-1}"

# ── L1 知识库语义检索：本地 embedding 模型目录 ──────────────────────
# 配了就启用 fastembed 进程内向量化(bge-m3 int8 单文件 onnx/model.onnx),知识库检索
# 升级为 fts+向量 RRF 混合;不配/加载失败则降级为纯全文 fts。模型目录需含
# onnx/model.onnx + tokenizer.json/config.json/special_tokens_map.json/tokenizer_config.json。
# (生产 deb 的模型下载/配置入口=设置页"知识库模型"卡,Phase 3 收尾待做。)
export PINVOU3_KB_EMBED_MODEL_DIR="${PINVOU3_KB_EMBED_MODEL_DIR:-$HOME/models/bge-m3}"

exec npx tauri dev "$@"
