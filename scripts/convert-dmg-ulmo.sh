#!/usr/bin/env bash
# Convert a tauri dmg from UDZO to ULMO (LZMA) in place.
#
# Background: the tauri dmg (bundler's bundle_dmg) is UDZO/zlib and passes no
# -imagekey zlib-level, so hdiutil uses its default compression level 1
# (speed-oriented). Converting to ULMO (LZMA) usually saves another 20-40% on
# the app payload; mounting ULMO requires macOS 10.15+ (our floor is 11.0).
# Under an ad-hoc signature (signingIdentity="-") the dmg itself is not
# signed, so a container conversion breaks no signature; the inner .app
# signature is unaffected by the outer container format.
#
# Degradation: a failed conversion keeps the original dmg and exits 0, so a
# compression upgrade can never fail the release packaging.
#
# macOS-only by nature (needs hdiutil); shared by the macOS job in
# release-packages.yml and by scripts/release-macos.sh so the two release
# paths cannot drift.
# Local static check: bash -n scripts/convert-dmg-ulmo.sh.
set -euo pipefail

usage() {
  echo "usage: $0 <dmg>" >&2
  exit 2
}

dmg="${1:-}"
[ -n "$dmg" ] && [ -f "$dmg" ] || usage
command -v hdiutil >/dev/null 2>&1 || {
  echo "missing required command: hdiutil (macOS only)" >&2
  exit 1
}

format="$(hdiutil imageinfo -format "$dmg" 2>/dev/null || echo UNKNOWN)"
if [ "$format" = "ULMO" ]; then
  exit 0
fi

before="$(stat -f%z "$dmg")"
# hdiutil convert appends .dmg to the -o target name (out.ulmo ->
# out.ulmo.dmg), so the mv source is fixed as "$dmg.ulmo.dmg".
rm -f "$dmg.ulmo.dmg"
if hdiutil convert "$dmg" -format ULMO -o "$dmg.ulmo" >/dev/null; then
  mv -f "$dmg.ulmo.dmg" "$dmg"
  echo "dmg compression upgraded ${format}→ULMO: ${before} -> $(stat -f%z "$dmg") bytes"
else
  rm -f "$dmg.ulmo.dmg"
  echo "⚠️ ULMO conversion failed, keeping the original ${format} dmg"
fi
