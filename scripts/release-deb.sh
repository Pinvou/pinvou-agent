#!/usr/bin/env bash
# Build the community Linux package and upload it to an existing GitHub Release.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_DIR="$REPO_ROOT/pinvou3-app"

node "$REPO_ROOT/scripts/sync-version.mjs" --check
VERSION="$(tr -d '[:space:]' < "$REPO_ROOT/VERSION")"
TAG="${1:-v$VERSION}"
ARCH="$(dpkg --print-architecture)"

gh release view "$TAG" >/dev/null
(cd "$APP_DIR" && npm ci --prefer-offline --no-audit && npm run build)

SOURCE="$APP_DIR/src-tauri/target/release/bundle/deb/pinvou3_${VERSION}_${ARCH}.deb"
ASSET="$APP_DIR/src-tauri/target/release/bundle/deb/pinvou-agent_${VERSION}_linux-${ARCH}-community.deb"
if [ ! -f "$SOURCE" ]; then
  echo "Community deb not found: $SOURCE" >&2
  exit 1
fi

cp "$SOURCE" "$ASSET"
sha256sum "$ASSET" > "$ASSET.sha256"
gh release upload "$TAG" "$ASSET" "$ASSET.sha256" --clobber
echo "Uploaded community Linux assets to GitHub Release $TAG"
