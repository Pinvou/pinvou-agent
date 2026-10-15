#!/usr/bin/env bash
# Extract a release deb and validate the target architecture plus the
# dynamic-symbol floors of every ELF inside; the policy itself
# lives in check-linux-elf-policy.sh.
set -euo pipefail

usage() {
  echo "usage: $0 <deb> <amd64|arm64|loongarch64> [glibc-floor]" >&2
  exit 2
}

deb_path="${1:-}"
expected_arch="${2:-}"
glibc_floor="${3:-}"
[ -n "$deb_path" ] && [ -f "$deb_path" ] && [ -n "$expected_arch" ] || usage

# Per-arch default floor (same map as check-linux-elf-policy.sh): jammy
# glibc 2.35 for amd64/arm64, 2.38 for loongarch64 (deepin 23/25 + UOS V25).
# Validated here too, so a wrong arch name fails before the deb extraction.
case "$expected_arch" in
  amd64|arm64) glibc_floor="${glibc_floor:-2.35}" ;;
  loongarch64) glibc_floor="${glibc_floor:-2.38}" ;;
  *) usage ;;
esac

command -v dpkg-deb >/dev/null || { echo "missing required command: dpkg-deb" >&2; exit 1; }

extract_dir="$(mktemp -d)"
trap 'rm -rf -- "$extract_dir"' EXIT
dpkg-deb -x "$deb_path" "$extract_dir"
"$(cd "$(dirname "$0")" && pwd)/check-linux-elf-policy.sh" \
  "$extract_dir" "$expected_arch" "$glibc_floor"
