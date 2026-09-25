#!/usr/bin/env bash
# Build the community Linux package and upload it to an existing GitHub Release.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_DIR="$REPO_ROOT/pinvou3-app"

node "$REPO_ROOT/scripts/sync-version.mjs" --check
VERSION="$(tr -d '[:space:]' < "$REPO_ROOT/VERSION")"
TAG="${1:-v$VERSION}"

gh release view "$TAG" >/dev/null
(cd "$APP_DIR" && npm ci --prefer-offline --no-audit && npm run build)

# Take the architecture from the artifact the bundler actually produced. The
# build target comes from the host's node/Rust triple, never from dpkg, so dpkg
# was consulted only to rebuild a filename that is already on disk. Where dpkg
# was absent the old `2>/dev/null || echo amd64` default silently guessed amd64,
# and an arm64 non-Debian host then failed here with a misleading
# "Community deb not found" naming a file the build never intended to produce.
DEB_DIR="$APP_DIR/src-tauri/target/release/bundle/deb"
shopt -s nullglob
BUILT=("$DEB_DIR/pinvou3_${VERSION}_"*.deb)
shopt -u nullglob
if [ "${#BUILT[@]}" -ne 1 ]; then
  echo "Expected exactly one $DEB_DIR/pinvou3_${VERSION}_*.deb, found ${#BUILT[@]}" >&2
  if [ -d "$DEB_DIR" ]; then
    ls -la "$DEB_DIR" >&2
  fi
  exit 1
fi
SOURCE="${BUILT[0]}"
ARCH="$(basename "$SOURCE" .deb)"
ARCH="${ARCH#pinvou3_"${VERSION}"_}"
ASSET="$DEB_DIR/pinvou-agent_${VERSION}_linux-${ARCH}-community.deb"

cp "$SOURCE" "$ASSET"
sha256sum "$ASSET" > "$ASSET.sha256"
gh release upload "$TAG" "$ASSET" "$ASSET.sha256" --clobber
echo "Uploaded community Linux assets to GitHub Release $TAG"
