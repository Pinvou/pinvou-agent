#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

OS_NAME="$(uname -s)"

# Linux/macOS 开发环境自动准备与正式包一致的应用隔离 Node + 精简 ACP Bridge。生成物被
# gitignore；完整性判断统一交给准备脚本，避免新增 Agent 后开发入口仍把旧 Runtime 误判为可用。
if [ "$OS_NAME" = "Linux" ] || [ "$OS_NAME" = "Darwin" ]; then
  ./scripts/prepare-codex-bridge-runtime.sh
fi

# 注:源 workflows/ → bundle 嵌入快照的同步已移入 build.rs(任何 cargo build/打包都同步,
# 不再只覆盖 dev 启动,改完直接 build 也不漂移)。

# ── 工作流预检开关 ───────────────────────────────────────────────
# warmup_check 已对齐 app 配置(endpoint 由 harness 注入 / REQUIRED_ENVS 精简为 base_url),
# 默认启用预检、与客户机一致,尽早暴露 warmup 类问题。要跳过(省 vLLM 冷启动探活 ~30s)
# export PINVOU3_SKIP_WARMUP=1。
export PINVOU3_SKIP_WARMUP="${PINVOU3_SKIP_WARMUP:-0}"

# ── 自托管 vLLM 连接(明文 HTTP,仅可信内网)────────────────────────
# 底座默认拒绝对非 loopback 的明文 http:// 发请求(client.rs validate_base_url_security),
# 且 reqwest 默认协议协商在某些网关下会 502。连接可信内网端点时
# 必须显式放行明文 HTTP + 钉死 HTTP/1.1。可在外部 export 覆盖。
# macOS 走远程 HTTPS API,不需要这两项,跳过。
if [ "$OS_NAME" = "Linux" ]; then
  export DEEPSEEK_ALLOW_INSECURE_HTTP="${DEEPSEEK_ALLOW_INSECURE_HTTP:-1}"
  export DEEPSEEK_FORCE_HTTP1="${DEEPSEEK_FORCE_HTTP1:-1}"
fi

# ── L1 知识库语义检索：本地 embedding 模型目录 ──────────────────────
# 应用默认使用 ~/.pinvou3/knowledge/models/bge-m3 托管目录。只有调用方已经显式设置
# PINVOU3_KB_EMBED_MODEL_DIR 时才覆盖，用于开发者测试自备模型；应用不会下载或覆盖外部目录。
# 配了就启用 fastembed 进程内向量化(bge-m3 int8 单文件
# onnx/model_int8.onnx 或 model.onnx),知识库检索
# 升级为 fts+向量 RRF 混合;不配/加载失败则降级为纯全文 fts。模型目录需含
# 单文件 ONNX + tokenizer.json/config.json/special_tokens_map.json/tokenizer_config.json。
# (生产包和普通 dev 的模型下载/配置入口均为设置页"知识库模型"卡。)
# 三平台共用(bge-m3 是工具模型非 LLM,Mac/Win/Linux 完全等效)。

# ── 三省六部「网页类」预置模板 seed 源(dev)──────────────────────────
# 工部角色 `cp -r ~/.pinvou3/web-template ...` 的母版,首次启动从此处复制(prod 走随 deb 的
# resource_dir)。目录需含 package.json + 预装 node_modules(离线可 npm run build 出单文件)。
export PINVOU3_WEB_TEMPLATE_DIR="${PINVOU3_WEB_TEMPLATE_DIR:-$HOME/models/web-template}"

# ── 完整 WebUI v2 relay ──────────────────────────────────────────
# 社区版默认连接本机自托管 Relay；跨设备测试时同时覆盖 public 与 WebSocket 地址。
export PINVOU_REMOTE_PUBLIC_URL="${PINVOU_REMOTE_PUBLIC_URL:-http://127.0.0.1:8787/pinvou3/remote}"
export PINVOU_REMOTE_RELAY_WS_URL="${PINVOU_REMOTE_RELAY_WS_URL:-ws://127.0.0.1:8787/pinvou3/remote/ws}"

# ── macOS 提示 ───────────────────────────────────────────────────
# Mac 不需要 webkit/fcitx/X11 相关 env(那些在 lib.rs RELEASE_ENV_DEFAULTS Linux 段)。
# 此处无需额外 Mac 专属 export,直接落到 tauri dev 即可。
# macOS dev 同样套用平台 overlay(原生红绿灯顶栏 titleBarStyle=Overlay),
# 与打包产物保持一致;build.js 的自动 overlay 只覆盖 build/bundle,dev 在此显式带上。
if [ "$OS_NAME" = "Darwin" ]; then
  echo "✓ macOS dev 模式(跳过 Linux 内网 vLLM/WebKit env)"
  exec npx tauri dev --config src-tauri/config/platforms/macos/tauri.conf.json "$@"
fi

exec npx tauri dev "$@"
