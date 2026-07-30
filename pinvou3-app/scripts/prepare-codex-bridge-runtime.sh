#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"

OS_NAME="$(uname -s)"
case "$OS_NAME" in
  Linux)
    PLATFORM_DIR="linux"
    ;;
  Darwin)
    PLATFORM_DIR="macos"
    ;;
  *)
    echo "当前 Bridge 构建脚本仅支持 Linux/macOS" >&2
    exit 1
    ;;
esac
OUT_DIR="$APP_DIR/src-tauri/resources/platforms/$PLATFORM_DIR/codex-bridge"

# BSD 工具(macOS)不支持 GNU 风格的 `--` 参数分隔符;脚本内路径无空格风险可控,
# 按 OS 决定是否携带。
if [ "$OS_NAME" = "Darwin" ]; then
  DD=""
else
  DD="--"
fi

NODE_VERSION="22.22.0"
CODEX_ACP_VERSION="1.1.5"
CODEX_ACP_PACKAGE="@agentclientprotocol/codex-acp"
CLAUDE_ACP_VERSION="0.62.0"
CLAUDE_ACP_PACKAGE="@agentclientprotocol/claude-agent-acp"
CLAUDE_SDK_VERSION="0.3.219"
BRIDGE_PACKAGE_DIR="$SCRIPT_DIR/codex-bridge-runtime"

bridge_runtime_valid() {
  local root="$1"
  local node
  local claude_native
  local entry="$root/acp/node_modules/@agentclientprotocol/codex-acp/dist/index.js"
  local claude_entry="$root/acp/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js"
  local package_json="$root/acp/node_modules/@agentclientprotocol/codex-acp/package.json"
  local claude_package_json="$root/acp/node_modules/@agentclientprotocol/claude-agent-acp/package.json"
  local version_output
  case "$OS_NAME-$(uname -m)" in
    Linux-x86_64)
      node="$root/node/bin/node"
      claude_native="$root/acp/node_modules/@anthropic-ai/claude-agent-sdk-linux-x64/claude"
      ;;
    Linux-aarch64|Linux-arm64)
      node="$root/node/bin/node"
      claude_native="$root/acp/node_modules/@anthropic-ai/claude-agent-sdk-linux-arm64/claude"
      ;;
    Darwin-arm64)
      node="$root/node/darwin-arm64/bin/node"
      claude_native="$root/acp/node_modules/@anthropic-ai/claude-agent-sdk-darwin-arm64/claude"
      [ -x "$root/node/darwin-x64/bin/node" ] \
        && [ -x "$root/acp/node_modules/@anthropic-ai/claude-agent-sdk-darwin-x64/claude" ] \
        && /usr/bin/lipo "$node" -verify_arch arm64 \
        && /usr/bin/lipo "$claude_native" -verify_arch arm64 \
        && /usr/bin/lipo "$root/node/darwin-x64/bin/node" -verify_arch x86_64 \
        && /usr/bin/lipo \
          "$root/acp/node_modules/@anthropic-ai/claude-agent-sdk-darwin-x64/claude" \
          -verify_arch x86_64 || return 1
      ;;
    Darwin-x86_64)
      node="$root/node/darwin-x64/bin/node"
      claude_native="$root/acp/node_modules/@anthropic-ai/claude-agent-sdk-darwin-x64/claude"
      [ -x "$root/node/darwin-arm64/bin/node" ] \
        && [ -x "$root/acp/node_modules/@anthropic-ai/claude-agent-sdk-darwin-arm64/claude" ] \
        && /usr/bin/lipo "$root/node/darwin-arm64/bin/node" -verify_arch arm64 \
        && /usr/bin/lipo \
          "$root/acp/node_modules/@anthropic-ai/claude-agent-sdk-darwin-arm64/claude" \
          -verify_arch arm64 \
        && /usr/bin/lipo "$node" -verify_arch x86_64 \
        && /usr/bin/lipo "$claude_native" -verify_arch x86_64 || return 1
      ;;
    *)
      return 1
      ;;
  esac
  [ -x "$node" ] && [ -s "$entry" ] && [ -s "$package_json" ] \
    && [ -s "$claude_entry" ] && [ -s "$claude_package_json" ] \
    && [ -x "$claude_native" ] || return 1
  version_output="$(
    env CODEX_PATH="$(command -v codex || true)" \
      "$node" "$entry" --version 2>/dev/null
  )" || return 1
  [ "$version_output" = "$CODEX_ACP_PACKAGE $CODEX_ACP_VERSION" ] || return 1
  local claude_version
  claude_version="$(
    "$node" -e 'process.stdout.write(require(process.argv[1]).version)' "$claude_package_json"
  )" || return 1
  [ "$claude_version" = "$CLAUDE_ACP_VERSION" ] \
    && "$claude_native" --version >/dev/null 2>&1
}

if bridge_runtime_valid "$OUT_DIR"; then
  echo "Codex ACP Bridge already ready: $OUT_DIR"
  exit 0
fi

case "$OS_NAME-$(uname -m)" in
  Linux-x86_64)
    NODE_OS="linux"
    NODE_CPU="x64"
    NODE_TARGET="linux-x64"
    NODE_TARGETS=("linux-x64")
    ;;
  Linux-aarch64|Linux-arm64)
    NODE_OS="linux"
    NODE_CPU="arm64"
    NODE_TARGET="linux-arm64"
    NODE_TARGETS=("linux-arm64")
    ;;
  Darwin-x86_64)
    NODE_OS="darwin"
    NODE_CPU="x64"
    NODE_TARGET="darwin-x64"
    NODE_TARGETS=("darwin-arm64" "darwin-x64")
    ;;
  Darwin-arm64)
    NODE_OS="darwin"
    NODE_CPU="arm64"
    NODE_TARGET="darwin-arm64"
    NODE_TARGETS=("darwin-arm64" "darwin-x64")
    ;;
  *)
    echo "当前 Bridge 构建脚本仅支持 Linux/macOS x64/arm64" >&2
    exit 1
    ;;
esac

node_archive_ext() {
  case "$1" in
    linux-*) echo "tar.xz" ;;
    darwin-*) echo "tar.gz" ;;
    *) return 1 ;;
  esac
}

node_sha256() {
  case "$1" in
    linux-x64) echo "9aa8e9d2298ab68c600bd6fb86a6c13bce11a4eca1ba9b39d79fa021755d7c37" ;;
    linux-arm64) echo "1bf1eb9ee63ffc4e5d324c0b9b62cf4a289f44332dfef9607cea1a0d9596ba6f" ;;
    darwin-x64) echo "5ea50c9d6dea3dfa3abb66b2656f7a4e1c8cef23432b558d45fb538c7b5dedce" ;;
    darwin-arm64) echo "5ed4db0fcf1eaf84d91ad12462631d73bf4576c1377e192d222e48026a902640" ;;
    *) return 1 ;;
  esac
}

for command_name in curl tar; do
  command -v "$command_name" >/dev/null 2>&1 || {
    echo "缺少构建命令: $command_name" >&2
    exit 1
  }
done

# macOS 没有 sha256sum,回退 shasum;两种工具的 --check 输入格式相同。
if command -v sha256sum >/dev/null 2>&1; then
  SHA256_CHECK=(sha256sum --check -)
elif command -v shasum >/dev/null 2>&1; then
  SHA256_CHECK=(shasum -a 256 --check -)
else
  echo "缺少 SHA256 校验工具(sha256sum 或 shasum)" >&2
  exit 1
fi

# Node 解压与 npm 安装需要数百 MB。把 staging 放到资源目录同一文件系统，
# 避免容量较小的 /tmp 留下半成品，也让最终目录切换只做同盘 rename。
RESOURCE_PARENT="$(dirname "$OUT_DIR")"
mkdir -p "$RESOURCE_PARENT"
BUILD_DIR="$(mktemp -d "$RESOURCE_PARENT/.codex-bridge-build.XXXXXX")"
trap 'rm -rf $DD "$BUILD_DIR"' EXIT

NODE_DIST_ROOT="$BUILD_DIR/node-v${NODE_VERSION}-${NODE_TARGET}"
for node_target in "${NODE_TARGETS[@]}"; do
  node_archive_ext="$(node_archive_ext "$node_target")"
  node_archive="node-v${NODE_VERSION}-${node_target}.${node_archive_ext}"
  node_archive_path="$BUILD_DIR/$node_archive"
  downloaded=false
  for base_url in "https://nodejs.org/dist/v${NODE_VERSION}" "https://npmmirror.com/mirrors/node/v${NODE_VERSION}"; do
    if curl --fail --location --retry 2 --connect-timeout 15 \
      "$base_url/$node_archive" --output "$node_archive_path"; then
      downloaded=true
      break
    fi
  done
  if [ "$downloaded" != true ]; then
    echo "下载 Node.js Runtime 失败: $node_target" >&2
    exit 1
  fi
  printf '%s  %s\n' "$(node_sha256 "$node_target")" "$node_archive_path" \
    | "${SHA256_CHECK[@]}"
  case "$node_archive_ext" in
    tar.xz) tar -xJf "$node_archive_path" -C "$BUILD_DIR" ;;
    tar.gz) tar -xzf "$node_archive_path" -C "$BUILD_DIR" ;;
  esac
done

ACP_ROOT="$BUILD_DIR/acp"
mkdir -p "$ACP_ROOT"
cp $DD "$BRIDGE_PACKAGE_DIR/package.json" "$BRIDGE_PACKAGE_DIR/package-lock.json" "$ACP_ROOT/"

npm_ci_for_target() {
  local prefix="$1"
  local target_os="$2"
  local target_cpu="$3"
  local npm_args=(
    ci
    --prefix "$prefix"
    --os="$target_os"
    --cpu="$target_cpu"
    --no-audit
    --no-fund
    --omit=dev
  )
  if [ "$target_os" = "linux" ]; then
    npm_args+=(--libc=glibc)
  fi
  PATH="$NODE_DIST_ROOT/bin:$PATH" "$NODE_DIST_ROOT/bin/npm" "${npm_args[@]}"
}

if [ "$OS_NAME" = "Darwin" ]; then
  # 发布产物是 universal DMG。用同一份 JS 依赖配齐 arm64/x64 两套 Claude
  # 原生程序，并保留两套 Node，运行时由对应架构切片选择，避免 Intel Mac
  # 启动 Apple Silicon Runtime（反之亦然）。
  npm_ci_for_target "$ACP_ROOT" darwin arm64
  ACP_X64_ROOT="$BUILD_DIR/acp-x64"
  mkdir -p "$ACP_X64_ROOT"
  cp $DD "$BRIDGE_PACKAGE_DIR/package.json" "$BRIDGE_PACKAGE_DIR/package-lock.json" "$ACP_X64_ROOT/"
  npm_ci_for_target "$ACP_X64_ROOT" darwin x64
  cp -R $DD \
    "$ACP_X64_ROOT/node_modules/@anthropic-ai/claude-agent-sdk-darwin-x64" \
    "$ACP_ROOT/node_modules/@anthropic-ai/"
else
  npm_ci_for_target "$ACP_ROOT" "$NODE_OS" "$NODE_CPU"
  # Anthropic SDK 同时声明 glibc/musl 包；deb 只保留 glibc 版本。
  rm -rf -- "$ACP_ROOT/node_modules/@anthropic-ai/claude-agent-sdk-linux-${NODE_CPU}-musl"
fi

# Bridge 总是通过 CODEX_PATH 启动系统或托管 Codex，不需要随包携带平台 Codex。
rm -rf $DD "$ACP_ROOT/node_modules/@openai"/codex-*
if find "$ACP_ROOT/node_modules/@openai" -maxdepth 1 -mindepth 1 \
  -type d -name 'codex-*' -print -quit | grep -q .; then
  echo "Bridge 中仍残留 Codex 平台二进制，拒绝打包" >&2
  exit 1
fi

READY_DIR="$BUILD_DIR/ready"
mkdir -p "$READY_DIR/node" "$READY_DIR/acp"
if [ "$OS_NAME" = "Darwin" ]; then
  for node_target in "${NODE_TARGETS[@]}"; do
    mkdir -p "$READY_DIR/node/$node_target/bin"
    install -m 0755 \
      "$BUILD_DIR/node-v${NODE_VERSION}-${node_target}/bin/node" \
      "$READY_DIR/node/$node_target/bin/node"
  done
  install -m 0644 "$NODE_DIST_ROOT/LICENSE" "$READY_DIR/node/LICENSE"
  READY_NODE="$READY_DIR/node/$NODE_TARGET/bin/node"
else
  mkdir -p "$READY_DIR/node/bin"
  install -m 0755 "$NODE_DIST_ROOT/bin/node" "$READY_DIR/node/bin/node"
  install -m 0644 "$NODE_DIST_ROOT/LICENSE" "$READY_DIR/node/LICENSE"
  READY_NODE="$READY_DIR/node/bin/node"
fi
mv $DD "$ACP_ROOT/node_modules" "$READY_DIR/acp/node_modules"

"$READY_NODE" -e '
const fs = require("fs");
const path = require("path");
const out = process.argv[1];
const platform = process.argv[5];
const runtimeArch = process.argv[6];
const nodes = platform === "darwin"
  ? {
      arm64: "node/darwin-arm64/bin/node",
      x64: "node/darwin-x64/bin/node"
    }
  : { [runtimeArch]: "node/bin/node" };
const manifest = {
  schema_version: 2,
  node_version: process.argv[2],
  codex_acp_version: process.argv[3],
  claude_acp_version: process.argv[4],
  claude_sdk_version: process.argv[7],
  platform,
  arch: platform === "darwin" ? "universal" : runtimeArch,
  node: nodes[runtimeArch],
  nodes,
  entrypoints: {
    codex: "acp/node_modules/@agentclientprotocol/codex-acp/dist/index.js",
    claude: "acp/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js"
  },
  requires_codex_path: true
};
fs.writeFileSync(path.join(out, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
' "$READY_DIR" "$NODE_VERSION" "$CODEX_ACP_VERSION" "$CLAUDE_ACP_VERSION" \
  "$NODE_OS" "$NODE_CPU" "$CLAUDE_SDK_VERSION"

bridge_runtime_valid "$READY_DIR" || {
  echo "生成的 Codex ACP Bridge 未通过完整性检查" >&2
  exit 1
}

mkdir -p "$OUT_DIR"
rm -rf $DD "$OUT_DIR/node.next" "$OUT_DIR/acp.next"
rm -f $DD "$OUT_DIR/manifest.json.next"
mv $DD "$READY_DIR/node" "$OUT_DIR/node.next"
mv $DD "$READY_DIR/acp" "$OUT_DIR/acp.next"
mv $DD "$READY_DIR/manifest.json" "$OUT_DIR/manifest.json.next"
rm -rf $DD "$OUT_DIR/node" "$OUT_DIR/acp"
rm -f $DD "$OUT_DIR/manifest.json"
mv $DD "$OUT_DIR/node.next" "$OUT_DIR/node"
mv $DD "$OUT_DIR/acp.next" "$OUT_DIR/acp"
mv $DD "$OUT_DIR/manifest.json.next" "$OUT_DIR/manifest.json"

bridge_runtime_valid "$OUT_DIR" || {
  echo "Codex ACP Bridge 安装后完整性检查失败" >&2
  exit 1
}
echo "Codex ACP Bridge ready: $OUT_DIR"
