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

NODE_VERSION="24.20.0"
CODEX_ACP_VERSION="1.6.2"
CODEX_ACP_PACKAGE="@agentclientprotocol/codex-acp"
CLAUDE_ACP_VERSION="0.70.0"
CLAUDE_ACP_PACKAGE="@agentclientprotocol/claude-agent-acp"
CLAUDE_SDK_VERSION="0.3.232"
BRIDGE_PACKAGE_DIR="$SCRIPT_DIR/codex-bridge-runtime"

# Print one string field of manifest.json on stdout.
#
# The exit status only distinguishes "file missing or empty" (returns 1, prints
# nothing). A present file without the key still returns 0 with empty output,
# because sed simply finds no match, so callers always compare the output
# (`[ "$(manifest_field ...)" = "$X" ]`) instead of relying on the status.
#
# Two passes and no back-reference: strip everything up to `"<key>": "`, then
# everything from the closing quote. Back-references through nested quotes are
# easy to get wrong, and a wrong one silently yields an empty string - every
# comparison would then fail and the bridge would be rebuilt on every run
# without anyone noticing.
manifest_field() {
  local manifest="$1"
  local key="$2"
  [ -s "$manifest" ] || return 1
  sed -n "s/.*\"$key\"[[:space:]]*:[[:space:]]*\"//p" "$manifest" \
    | head -n 1 \
    | sed 's/".*//'
}

bridge_runtime_valid() {
  local root="$1"
  local node
  # 本 PR 前生成的旧 staging 可能仍残留 Claude 平台原生二进制；发现即视为无效，
  # 强制重打包，避免本地复用旧产物时把约 245MB 的二进制重新打进安装包。
  if find "$root/acp/node_modules/@anthropic-ai" -maxdepth 1 -mindepth 1 \
    -type d -name 'claude-agent-sdk-*' -print -quit 2>/dev/null | grep -q .; then
    return 1
  fi
  local entry="$root/acp/node_modules/@agentclientprotocol/codex-acp/dist/index.js"
  local claude_entry="$root/acp/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js"
  local package_json="$root/acp/node_modules/@agentclientprotocol/codex-acp/package.json"
  local claude_package_json="$root/acp/node_modules/@agentclientprotocol/claude-agent-acp/package.json"
  local npm_cli="$root/node/lib/node_modules/npm/bin/npm-cli.js"
  local version_output

  # An existing runtime must match the target architecture.
  #
  # Without this, a host build (x64 node in staging) followed by a cross build
  # passes the execution checks below (x64 node runs on an x64 host), prints
  # "already ready" and ships an x64 node inside the arm64 package - the same
  # bug through another path.
  #
  # The manifest already records the arch (see the manifest generation at the
  # end of this file), so no separate marker file is added. Darwin packages a
  # universal runtime (both architectures) and is not compared. An unreadable
  # manifest or mismatching field counts as invalid: rebuilding once more beats
  # trusting a runtime of unknown origin.
  if [ -n "${NODE_CPU:-}" ] && [ "$OS_NAME" != "Darwin" ]; then
    [ "$(manifest_field "$root/manifest.json" arch)" = "$NODE_CPU" ] || return 1
  fi
  case "$OS_NAME-$(uname -m)" in
    Linux-x86_64|Linux-aarch64|Linux-arm64)
      node="$root/node/bin/node"
      ;;
    Darwin-arm64)
      node="$root/node/darwin-arm64/bin/node"
      [ -x "$root/node/darwin-x64/bin/node" ] \
        && /usr/bin/lipo "$node" -verify_arch arm64 \
        && /usr/bin/lipo "$root/node/darwin-x64/bin/node" -verify_arch x86_64 || return 1
      ;;
    Darwin-x86_64)
      node="$root/node/darwin-x64/bin/node"
      [ -x "$root/node/darwin-arm64/bin/node" ] \
        && /usr/bin/lipo "$root/node/darwin-arm64/bin/node" -verify_arch arm64 \
        && /usr/bin/lipo "$node" -verify_arch x86_64 || return 1
      ;;
    *)
      return 1
      ;;
  esac

  [ -x "$node" ] && [ -s "$npm_cli" ] && [ -s "$entry" ] && [ -s "$package_json" ] \
    && [ -s "$claude_entry" ] && [ -s "$claude_package_json" ] || return 1

  # The two checks below run plain JavaScript (the ACP package's --version and
  # reading package.json), so any interpreter works, but it must execute on
  # this machine:
  # - the host Node is already unpacked (the two calls after the download):
  #   use it;
  # - not unpacked yet but not a cross build: the packaged Node is the host
  #   architecture and runs (on macOS the universal case above already picked
  #   the host slice);
  # - not unpacked yet and cross (the fast-path call at the start): the
  #   packaged Node targets another architecture and would fail with ENOEXEC,
  #   so leave the runner empty and take the non-executing path below.
  local runner=""
  if [ -n "${NPM_NODE_ROOT:-}" ] && [ -x "$NPM_NODE_ROOT/bin/node" ]; then
    runner="$NPM_NODE_ROOT/bin/node"
  elif [ "$OS_NAME" = "Darwin" ] || [ "${NODE_TARGET:-}" = "${HOST_NODE_TARGET:-}" ]; then
    runner="$node"
  fi

  if [ -z "$runner" ]; then
    # Cross-build fast path: compare the versions recorded in the manifest with
    # the script constants instead of executing anything. Previously this fell
    # back to the target-architecture node, `--version` failed with ENOEXEC,
    # and "already ready" was dead code for cross builds, so every repeated
    # local cross build paid for two tarballs, sha256 checks and `npm ci`.
    #
    # This cannot mistake a broken runtime for a ready one: manifest.json is
    # written last - after `npm ci` and pruning, then a full executing
    # validation, then node / acp / manifest are moved into OUT_DIR, with the
    # manifest as the last of the three moves. A manifest with matching
    # versions in OUT_DIR therefore proves that the runtime passed the
    # executing validation and that all three moves completed.
    #
    # All three versions (Node and both ACP packages) are compared, so bumping
    # any single one invalidates the old staging. The proof only covers the
    # build sequence: files changed in OUT_DIR between two builds (corrupt but
    # still non-empty) are not detected; the executing validation runs fully
    # only right after preparation.
    [ "$(manifest_field "$root/manifest.json" node_version)" = "$NODE_VERSION" ] \
      || return 1
    [ "$(manifest_field "$root/manifest.json" codex_acp_version)" = "$CODEX_ACP_VERSION" ] \
      || return 1
    [ "$(manifest_field "$root/manifest.json" claude_acp_version)" = "$CLAUDE_ACP_VERSION" ] \
      || return 1
    return 0
  fi

  version_output="$(
    env CODEX_PATH="$(command -v codex || true)" \
      "$runner" "$entry" --version 2>/dev/null
  )" || return 1
  [ "$version_output" = "$CODEX_ACP_PACKAGE $CODEX_ACP_VERSION" ] || return 1
  local claude_version
  claude_version="$(
    "$runner" -e 'process.stdout.write(require(process.argv[1]).version)' "$claude_package_json"
  )" || return 1
  [ "$claude_version" = "$CLAUDE_ACP_VERSION" ]
}

# ---- Target architecture vs host architecture ----
#
# They differ in cross builds, and the bridge must package the target
# architecture's Node. The script used to look only at `uname -m`, so an arm64
# package cross-built on an x64 host carried an x86_64 node that failed with
# ENOEXEC on first use, with no warning during the build.
#
# Unlike SenseVoice, the bridge can be prepared for another architecture: it
# compiles nothing, it only downloads, verifies and unpacks an official Node
# tarball. The one obstacle is that `npm ci` must run on a Node that executes
# on the host, so cross builds download two: the host one runs npm, the target
# one is packaged. npm already receives `--os/--cpu/--libc` (npm_ci_for_target
# below), so resolving dependencies for the target platform is built in.
#
# The macOS branch already downloads two Node builds (darwin-arm64 and
# darwin-x64) and packages both, so "download several, run one" is an existing
# pattern in this script.
HOST_MACHINE="$(uname -m)"
TARGET_MACHINE="${PINVOU3_BRIDGE_TARGET_ARCH:-$HOST_MACHINE}"

# Machine name -> official Node distribution target name.
node_target_for() {
  case "$OS_NAME-$1" in
    Linux-x86_64) echo "linux-x64" ;;
    Linux-aarch64|Linux-arm64) echo "linux-arm64" ;;
    Darwin-x86_64) echo "darwin-x64" ;;
    Darwin-arm64) echo "darwin-arm64" ;;
    *) return 1 ;;
  esac
}

HOST_NODE_TARGET="$(node_target_for "$HOST_MACHINE")" || {
  echo "Unsupported Codex ACP Bridge host: $OS_NAME-$HOST_MACHINE (only x86_64 and aarch64/arm64)" >&2
  exit 1
}

# The target vocabulary is wider than `uname -m`: PINVOU3_BRIDGE_TARGET_ARCH is
# injected from the Rust target triple on the JS side, where macOS arm64 is
# spelled aarch64 (aarch64-apple-darwin), while macOS `uname -m` reports arm64.
# Both words must be accepted, or a manual single-architecture build on a darwin
# host falls into `*)` - the same reason the Linux entry lists both.
case "$OS_NAME-$TARGET_MACHINE" in
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
  Darwin-aarch64|Darwin-arm64)
    NODE_OS="darwin"
    NODE_CPU="arm64"
    NODE_TARGET="darwin-arm64"
    NODE_TARGETS=("darwin-arm64" "darwin-x64")
    ;;
  *)
    echo "Unsupported Codex ACP Bridge target: $OS_NAME-$TARGET_MACHINE (only x86_64 and aarch64/arm64)" >&2
    exit 1
    ;;
esac

# Cross builds also download the host Node; it only runs npm and is not packaged.
if [ "$HOST_NODE_TARGET" != "$NODE_TARGET" ]; then
  case " ${NODE_TARGETS[*]} " in
    *" $HOST_NODE_TARGET "*) ;;
    *) NODE_TARGETS+=("$HOST_NODE_TARGET") ;;
  esac
  echo "[codex-bridge] cross build: target $NODE_TARGET, running npm with the host $HOST_NODE_TARGET node"
fi

if bridge_runtime_valid "$OUT_DIR"; then
  echo "Codex ACP Bridge already ready: $OUT_DIR"
  exit 0
fi

node_archive_ext() {
  case "$1" in
    linux-*) echo "tar.xz" ;;
    darwin-*) echo "tar.gz" ;;
    *) return 1 ;;
  esac
}

node_sha256() {
  case "$1" in
    linux-x64) echo "2f2c0da162318f0de47665410c7c8c2ed3d36c8f3105de4bbc61176c70a7cbf2" ;;
    linux-arm64) echo "5f4ddab610c1ab2016b3c227cebdbf6d9495161487e4739c7b90090595f465f7" ;;
    darwin-x64) echo "9e5b2644cf107befb6aefca676b96d3296bc10138096f022ed378d6233ed81f4" ;;
    darwin-arm64) echo "40e5607e5ecb3db9192723776da2d75d966260fc74a7a9e731c1bd67dda96bc8" ;;
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

# The Node that is packaged (target architecture).
NODE_DIST_ROOT="$BUILD_DIR/node-v${NODE_VERSION}-${NODE_TARGET}"
# The Node that runs npm (host architecture); the same directory unless cross building.
NPM_NODE_ROOT="$BUILD_DIR/node-v${NODE_VERSION}-${HOST_NODE_TARGET}"
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
  # Run npm with the host-architecture Node: the target one cannot execute
  # here during a cross build (ENOEXEC). Dependency resolution is governed by
  # --os/--cpu/--libc above, not by the interpreter. Outside cross builds
  # NPM_NODE_ROOT equals NODE_DIST_ROOT, so nothing changes.
  PATH="$NPM_NODE_ROOT/bin:$PATH" "$NPM_NODE_ROOT/bin/npm" "${npm_args[@]}"
}

npm_ci_for_target "$ACP_ROOT" "$NODE_OS" "$NODE_CPU"

# Bridge 通过 CODEX_PATH 启动系统 Codex，通过 CLAUDE_CODE_EXECUTABLE / PATH
# 中的 claude 启动系统 Claude Code（与 Kimi 一致），均不随包携带平台原生二进制
#（单个 claude 二进制解压后约 245MB，universal 双架构会让 dmg 多出约 140MB）。
rm -rf $DD "$ACP_ROOT/node_modules/@openai"/codex-*
rm -rf $DD "$ACP_ROOT"/node_modules/@anthropic-ai/claude-agent-sdk-{darwin,linux,win32}-*
if find "$ACP_ROOT/node_modules/@openai" -maxdepth 1 -mindepth 1 \
  -type d -name 'codex-*' -print -quit | grep -q .; then
  echo "Bridge 中仍残留 Codex 平台二进制，拒绝打包" >&2
  exit 1
fi
if find "$ACP_ROOT/node_modules/@anthropic-ai" -maxdepth 1 -mindepth 1 \
  -type d -name 'claude-agent-sdk-*' -print -quit | grep -q .; then
  echo "Bridge 中仍残留 Claude 平台二进制，拒绝打包" >&2
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
else
  mkdir -p "$READY_DIR/node/bin"
  install -m 0755 "$NODE_DIST_ROOT/bin/node" "$READY_DIR/node/bin/node"
  install -m 0644 "$NODE_DIST_ROOT/LICENSE" "$READY_DIR/node/LICENSE"
fi
# 连接器首次使用时由随包 Node 运行 npm；保留 Node 官方发行包自带的纯 JS npm，
# Linux/macOS 均不再依赖用户机器预装 npm。
mkdir -p "$READY_DIR/node/lib/node_modules"
cp -R $DD "$NODE_DIST_ROOT/lib/node_modules/npm" "$READY_DIR/node/lib/node_modules/npm"
mv $DD "$ACP_ROOT/node_modules" "$READY_DIR/acp/node_modules"

# Generate the manifest with the host Node (`$NPM_NODE_ROOT`), not the one just
# installed into `$READY_DIR/node`: that is the packaged target binary and
# cannot execute here during a cross build. The generator is plain JavaScript.
"${NPM_NODE_ROOT}/bin/node" -e '
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
  schema_version: 3,
  node_version: process.argv[2],
  codex_acp_version: process.argv[3],
  claude_acp_version: process.argv[4],
  claude_sdk_version: process.argv[7],
  platform,
  arch: platform === "darwin" ? "universal" : runtimeArch,
  node: nodes[runtimeArch],
  nodes,
  npm: "node/lib/node_modules/npm/bin/npm-cli.js",
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
