#!/usr/bin/env bash
# Extract a release deb and validate the target architecture plus the
# Ubuntu 22.04 dynamic-symbol floors of every ELF inside; the policy itself
# lives in check-linux-elf-policy.sh.
set -euo pipefail

usage() {
  echo "usage: $0 <deb> <amd64|arm64> [glibc-floor]" >&2
  exit 2
}

deb_path="${1:-}"
expected_arch="${2:-}"
glibc_floor="${3:-2.35}"
[ -n "$deb_path" ] && [ -f "$deb_path" ] && [ -n "$expected_arch" ] || usage

command -v dpkg-deb >/dev/null || { echo "missing required command: dpkg-deb" >&2; exit 1; }

extract_dir="$(mktemp -d)"
trap 'rm -rf -- "$extract_dir"' EXIT
dpkg-deb -x "$deb_path" "$extract_dir"
"$(cd "$(dirname "$0")" && pwd)/check-linux-elf-policy.sh" \
  "$extract_dir" "$expected_arch" "$glibc_floor"
