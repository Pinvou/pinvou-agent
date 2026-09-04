#!/usr/bin/env bash
# Repack the tauri-bundler deb with maximum compression.
#
# Background: the tauri v2 bundler hardcodes gzip level 6 for data.tar
# (tauri-cli bundle/linux/debian.rs, Compression::default(); tauri.conf has no
# compression knob — its schema only exposes nsis.compression and
# rpm.compression). The deb bulk is the three ELFs inside data.tar (main
# binary + node + knowledge-server); gzip-6 → xz -9 typically saves another
# 20-30%.
#
# Approach: unpack with dpkg-deb -R, then rebuild in place with
# --root-owner-group -Zxz -z9. The content is untouched — only the data.tar
# compression container changes — so md5sums, maintainer scripts, the
# packaged ELFs and their glibc symbol versions are all preserved;
# --root-owner-group keeps entries root:root on non-root build machines
# (e.g. GitHub runners); control.tar comes out as xz as well (dpkg uniform
# compression is the default).
#
# Linux-only by nature (needs dpkg-deb); shared by the two deb jobs in
# release-packages.yml and by scripts/release-deb.sh. The glibc floor guard
# that runs afterwards unpacks with dpkg-deb -x, which reads xz natively.
# Local static check: bash -n scripts/repack-deb-xz.sh.
set -euo pipefail

usage() {
  echo "usage: $0 <deb>" >&2
  exit 2
}

deb="${1:-}"
[ -n "$deb" ] && [ -f "$deb" ] || usage
command -v dpkg-deb >/dev/null 2>&1 || {
  echo "missing required command: dpkg-deb (Linux only)" >&2
  exit 1
}

before=$(stat -c%s "$deb")
work="$(mktemp -d)"
trap 'rm -rf -- "$work"' EXIT

dpkg-deb -R "$deb" "$work/pkg"
dpkg-deb --root-owner-group -Zxz -z9 --build "$work/pkg" "$deb"

# Repack self-check: the control metadata must parse and the Package field
# must be non-empty, otherwise treat the artifact as corrupted and fail.
[ -n "$(dpkg-deb -f "$deb" Package)" ] || {
  echo "FAIL: repacked deb has empty Package field: $deb" >&2
  exit 1
}

after=$(stat -c%s "$deb")
echo "repack-deb-xz: $(numfmt --to=iec-i --suffix=B "$before") -> $(numfmt --to=iec-i --suffix=B "$after") ($deb)"
