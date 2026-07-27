#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"
OUT_DIR="$APP_DIR/src-tauri/resources/platforms/linux/codex-bridge"

NODE_VERSION="22.22.0"
CODEX_ACP_VERSION="1.1.5"
CODEX_ACP_PACKAGE="@agentclientprotocol/codex-acp"
BRIDGE_PACKAGE_DIR="$SCRIPT_DIR/codex-bridge-runtime"

bridge_runtime_valid() {
  local root="$1"
  local node="$root/node/bin/node"
  local entry="$root/acp/node_modules/@agentclientprotocol/codex-acp/dist/index.js"
  local package_json="$root/acp/node_modules/@agentclientprotocol/codex-acp/package.json"
  local version_output
  [ -x "$node" ] && [ -s "$entry" ] && [ -s "$package_json" ] || return 1
  version_output="$(
    env CODEX_PATH="$(command -v codex || true)" \
      "$node" "$entry" --version 2>/dev/null
  )" || return 1
  [ "$version_output" = "$CODEX_ACP_PACKAGE $CODEX_ACP_VERSION" ]
}

if bridge_runtime_valid "$OUT_DIR"; then
  echo "Codex ACP Bridge already ready: $OUT_DIR"
  exit 0
fi

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)
    NODE_TARGET="linux-x64"
    NODE_SHA256="9aa8e9d2298ab68c600bd6fb86a6c13bce11a4eca1ba9b39d79fa021755d7c37"
    ;;
  Linux-aarch64|Linux-arm64)
    NODE_TARGET="linux-arm64"
    NODE_SHA256="1bf1eb9ee63ffc4e5d324c0b9b62cf4a289f44332dfef9607cea1a0d9596ba6f"
    ;;
  *)
    echo "当前 Bridge 构建脚本仅支持 Linux x64/arm64" >&2
    exit 1
    ;;
esac

for command_name in curl sha256sum tar; do
  command -v "$command_name" >/dev/null 2>&1 || {
    echo "缺少构建命令: $command_name" >&2
    exit 1
  }
done

# Node 解压与 npm 安装需要数百 MB。把 staging 放到资源目录同一文件系统，
# 避免容量较小的 /tmp 留下半成品，也让最终目录切换只做同盘 rename。
RESOURCE_PARENT="$(dirname "$OUT_DIR")"
mkdir -p "$RESOURCE_PARENT"
BUILD_DIR="$(mktemp -d "$RESOURCE_PARENT/.codex-bridge-build.XXXXXX")"
trap 'rm -rf -- "$BUILD_DIR"' EXIT

NODE_ARCHIVE="node-v${NODE_VERSION}-${NODE_TARGET}.tar.xz"
NODE_ARCHIVE_PATH="$BUILD_DIR/$NODE_ARCHIVE"
NODE_DIST_ROOT="$BUILD_DIR/node-v${NODE_VERSION}-${NODE_TARGET}"

downloaded=false
for base_url in "https://nodejs.org/dist/v${NODE_VERSION}" "https://npmmirror.com/mirrors/node/v${NODE_VERSION}"; do
  if curl --fail --location --retry 2 --connect-timeout 15 \
    "$base_url/$NODE_ARCHIVE" --output "$NODE_ARCHIVE_PATH"; then
    downloaded=true
    break
  fi
done
if [ "$downloaded" != true ]; then
  echo "下载 Node.js Runtime 失败" >&2
  exit 1
fi

printf '%s  %s\n' "$NODE_SHA256" "$NODE_ARCHIVE_PATH" | sha256sum --check -
tar -xJf "$NODE_ARCHIVE_PATH" -C "$BUILD_DIR"

ACP_ROOT="$BUILD_DIR/acp"
mkdir -p "$ACP_ROOT"
cp -- "$BRIDGE_PACKAGE_DIR/package.json" "$BRIDGE_PACKAGE_DIR/package-lock.json" "$ACP_ROOT/"
PATH="$NODE_DIST_ROOT/bin:$PATH" "$NODE_DIST_ROOT/bin/npm" ci \
  --prefix "$ACP_ROOT" \
  --no-audit \
  --no-fund \
  --omit=dev

# Bridge 总是通过 CODEX_PATH 启动系统或托管 Codex，不需要随包携带平台 Codex。
rm -rf -- "$ACP_ROOT/node_modules/@openai"/codex-*
if find "$ACP_ROOT/node_modules/@openai" -maxdepth 1 -mindepth 1 \
  -type d -name 'codex-*' -print -quit | grep -q .; then
  echo "Bridge 中仍残留 Codex 平台二进制，拒绝打包" >&2
  exit 1
fi

READY_DIR="$BUILD_DIR/ready"
mkdir -p "$READY_DIR/node/bin" "$READY_DIR/acp"
install -m 0755 "$NODE_DIST_ROOT/bin/node" "$READY_DIR/node/bin/node"
install -m 0644 "$NODE_DIST_ROOT/LICENSE" "$READY_DIR/node/LICENSE"
mv -- "$ACP_ROOT/node_modules" "$READY_DIR/acp/node_modules"

"$READY_DIR/node/bin/node" -e '
const fs = require("fs");
const path = require("path");
const out = process.argv[1];
const manifest = {
  schema_version: 1,
  node_version: process.argv[2],
  codex_acp_version: process.argv[3],
  platform: process.platform,
  arch: process.arch,
  node: "node/bin/node",
  entrypoint: "acp/node_modules/@agentclientprotocol/codex-acp/dist/index.js",
  requires_codex_path: true
};
fs.writeFileSync(path.join(out, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
' "$READY_DIR" "$NODE_VERSION" "$CODEX_ACP_VERSION"

bridge_runtime_valid "$READY_DIR" || {
  echo "生成的 Codex ACP Bridge 未通过完整性检查" >&2
  exit 1
}

mkdir -p "$OUT_DIR"
rm -rf -- "$OUT_DIR/node.next" "$OUT_DIR/acp.next"
rm -f -- "$OUT_DIR/manifest.json.next"
mv -- "$READY_DIR/node" "$OUT_DIR/node.next"
mv -- "$READY_DIR/acp" "$OUT_DIR/acp.next"
mv -- "$READY_DIR/manifest.json" "$OUT_DIR/manifest.json.next"
rm -rf -- "$OUT_DIR/node" "$OUT_DIR/acp"
rm -f -- "$OUT_DIR/manifest.json"
mv -- "$OUT_DIR/node.next" "$OUT_DIR/node"
mv -- "$OUT_DIR/acp.next" "$OUT_DIR/acp"
mv -- "$OUT_DIR/manifest.json.next" "$OUT_DIR/manifest.json"

bridge_runtime_valid "$OUT_DIR" || {
  echo "Codex ACP Bridge 安装后完整性检查失败" >&2
  exit 1
}
echo "Codex ACP Bridge ready: $OUT_DIR"
