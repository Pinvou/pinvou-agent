#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

# ── 内置 MCP 共享 key(dev) ─────────────────────────────────
# 与 release-deb.sh 使用同一个 gitignored 密钥文件。option_env! 在编译时读取，
# 因此必须在 `tauri dev` 启动 Cargo 之前 export。未配置时保持普通开发模式可用，
# 仅需要高德天气/同花顺问财/企查查共享额度时才必须填写。
SECRETS_ENV="../scripts/.builtin-secrets.env"
if [ -f "$SECRETS_ENV" ]; then
  set -a
  . "$SECRETS_ENV"
  set +a
  echo "✓ 已加载内置 MCP 共享 key(dev)"
fi

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
# 配了就启用 fastembed 进程内向量化(bge-m3 int8 单文件
# onnx/model_int8.onnx 或 model.onnx),知识库检索
# 升级为 fts+向量 RRF 混合;不配/加载失败则降级为纯全文 fts。模型目录需含
# 单文件 ONNX + tokenizer.json/config.json/special_tokens_map.json/tokenizer_config.json。
# (生产 deb 的模型下载/配置入口=设置页"知识库模型"卡,Phase 3 收尾待做。)
export PINVOU3_KB_EMBED_MODEL_DIR="${PINVOU3_KB_EMBED_MODEL_DIR:-$HOME/models/bge-m3}"

# ── 三省六部「网页类」预置模板 seed 源(dev)──────────────────────────
# 工部角色 `cp -r ~/.pinvou3/web-template ...` 的母版,首次启动从此处复制(prod 走随 deb 的
# resource_dir)。目录需含 package.json + 预装 node_modules(离线可 npm run build 出单文件)。
export PINVOU3_WEB_TEMPLATE_DIR="${PINVOU3_WEB_TEMPLATE_DIR:-$HOME/models/web-template}"

# ── 完整 WebUI v2 relay ──────────────────────────────────────────
# dev 默认走公网域名中继，电脑/手机浏览器均可直接粘贴完整链接。外部覆盖时
# public 页面和 WebSocket 都必须保留 Relay 的公开 base path：
#   PINVOU_REMOTE_PUBLIC_URL=http://10.x.x.x:8787/pinvou3/remote
#   PINVOU_REMOTE_RELAY_WS_URL=ws://10.x.x.x:8787/pinvou3/remote/ws
export PINVOU_REMOTE_PUBLIC_URL="${PINVOU_REMOTE_PUBLIC_URL:-https://pinvou.com/pinvou3/remote}"
export PINVOU_REMOTE_RELAY_WS_URL="${PINVOU_REMOTE_RELAY_WS_URL:-wss://pinvou.com/pinvou3/remote/ws}"

exec npx tauri dev "$@"
