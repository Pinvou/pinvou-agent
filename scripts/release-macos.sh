#!/usr/bin/env bash
# Build the unsigned/ad-hoc-signed community macOS package and upload it to an
# existing GitHub Release. Official Developer ID builds use the private release
# pipeline and may be uploaded to the same release with an -official suffix.
set -euo pipefail

if [ "$(uname -s)" != "Darwin" ]; then
  echo "This script must run on macOS." >&2
  exit 1
fi

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
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"

for target in aarch64-apple-darwin x86_64-apple-darwin; do
  rustup target list --installed | grep -q "^${target}$" || rustup target add "$target"
done

gh release view "$TAG" >/dev/null
(cd "$APP_DIR" && npm ci --prefer-offline --no-audit)
(cd "$APP_DIR" && node scripts/tauri/build.js build --target universal-apple-darwin)

APP_BIN="$APP_DIR/src-tauri/target/universal-apple-darwin/release/bundle/macos/pinvou3.app/Contents/MacOS/pinvou3-tauri"
if [ ! -f "$APP_BIN" ]; then
  echo "Community app binary not found: $APP_BIN" >&2
  exit 1
fi
lipo "$APP_BIN" -verify_arch arm64 x86_64

SOURCE="$APP_DIR/src-tauri/target/universal-apple-darwin/release/bundle/dmg/pinvou3_${VERSION}_universal.dmg"
ASSET="$APP_DIR/src-tauri/target/universal-apple-darwin/release/bundle/dmg/pinvou-agent_${VERSION}_macos-universal-community.dmg"
if [ ! -f "$SOURCE" ]; then
  echo "Community dmg not found: $SOURCE" >&2
  exit 1
fi

# Same compression upgrade as release-packages.yml: tauri emits UDZO without
# zlib-level (hdiutil default level 1); converting to ULMO (LZMA) typically
# saves another 20-40%. Mounting ULMO needs macOS 10.15+. Under an ad-hoc
# signature the dmg itself is not signed, so a container conversion breaks
# no signature.
DMG_FORMAT="$(hdiutil imageinfo -format "$SOURCE" 2>/dev/null || echo UNKNOWN)"
if [ "$DMG_FORMAT" != "ULMO" ]; then
  # hdiutil convert appends .dmg to the -o target name, hence the fixed
  # mv source "$SOURCE.ulmo.dmg".
  rm -f "$SOURCE.ulmo.dmg"
  if hdiutil convert "$SOURCE" -format ULMO -o "$SOURCE.ulmo" >/dev/null; then
    mv -f "$SOURCE.ulmo.dmg" "$SOURCE"
    echo "dmg compression upgraded ${DMG_FORMAT}→ULMO"
  else
    rm -f "$SOURCE.ulmo.dmg"
    echo "⚠️ ULMO conversion failed, keeping the original ${DMG_FORMAT} dmg"
  fi
fi

cp "$SOURCE" "$ASSET"
shasum -a 256 "$ASSET" > "$ASSET.sha256"
gh release upload "$TAG" "$ASSET" "$ASSET.sha256" --clobber
echo "Uploaded community macOS assets to GitHub Release $TAG"
