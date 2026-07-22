#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"
OUT_DIR="$APP_DIR/src-tauri/resources/codex-acp"
VERSION="1.1.5"

command -v npm >/dev/null 2>&1 || { echo "缺少 npm" >&2; exit 1; }

BUILD_DIR="$(mktemp -d "${TMPDIR:-/tmp}/pinvou3-codex-acp.XXXXXX")"
trap 'rm -rf -- "$BUILD_DIR"' EXIT
npm install --prefix "$BUILD_DIR" --no-audit --no-fund \
  "@agentclientprotocol/codex-acp@$VERSION"

mkdir -p "$OUT_DIR"
rm -rf -- "$OUT_DIR/node_modules"
mv -- "$BUILD_DIR/node_modules" "$OUT_DIR/node_modules"

echo "Codex ACP $VERSION runtime ready: $OUT_DIR/node_modules"
