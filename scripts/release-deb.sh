#!/usr/bin/env bash
# Build the community Linux package and upload it to an existing GitHub Release.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_DIR="$REPO_ROOT/pinvou3-app"

V_TAURI="$(jq -r .version "$APP_DIR/src-tauri/tauri.conf.json")"
V_CARGO="$(sed -n 's/^version = "\(.*\)"/\1/p' "$APP_DIR/src-tauri/Cargo.toml" | head -1)"
V_NPM="$(jq -r .version "$APP_DIR/package.json")"
if [ "$V_TAURI" != "$V_CARGO" ] || [ "$V_TAURI" != "$V_NPM" ]; then
  echo "Version mismatch: tauri=$V_TAURI cargo=$V_CARGO npm=$V_NPM" >&2
  exit 1
fi

VERSION="$V_TAURI"
TAG="${1:-v$VERSION}"
ARCH="$(dpkg --print-architecture 2>/dev/null || echo amd64)"

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
