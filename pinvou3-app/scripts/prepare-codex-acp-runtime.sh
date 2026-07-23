#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
echo "prepare-codex-acp-runtime.sh 已迁移为精简 Bridge 构建，继续执行兼容入口。"
exec "$SCRIPT_DIR/prepare-codex-bridge-runtime.sh" "$@"
