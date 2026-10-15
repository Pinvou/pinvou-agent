#!/usr/bin/env bash
# Download, verify and unpack the pinned LoongArch64 (new-world) cross
# toolchain used by the loong64 release build, then print the environment a
# CI step must export for cargo/cc/cmake to drive it.
#
# Source of the pin (release page + per-tag version table):
#   https://github.com/loong64/cross-tools/releases/tag/20260912
# The tag ships four glibc variants for loongarch64-unknown-linux-gnu:
#   baseline  gcc 12.3.0, binutils 2.41, glibc 2.38, kernel 6.6
#   stable    gcc 14.4.0, binutils 2.41, glibc 2.38, kernel 6.6   <- pinned
#   mainline  gcc 14.4.0, binutils 2.41, glibc 2.41, kernel 6.12
#   latest    gcc 16.2.0, binutils 2.47, glibc 2.44, kernel 6.18
# "stable" is the only variant whose bundled glibc (2.38) matches the
# deb's floor (deepin 23 beige / deepin 25 crimson / UOS V25 all ship
# glibc 2.38) while carrying a current GCC. mainline (2.41) and latest
# (2.44) would link binaries that refuse to start on a 2.38 target — the
# same reason the Debian trixie loong64 port (glibc 2.41) is unusable as
# a linking baseline.
#
# Layout of the unpacked tarball: a single top-level directory
# loongarch64-unknown-linux-gnu/ containing bin/ (crosstool-NG prefix),
# lib/ and a bundled target sysroot at
# loongarch64-unknown-linux-gnu/sysroot (glibc 2.38 + kernel 6.6 headers).
# The prefix is relocatable: gcc derives its sysroot from its own argv[0]
# (verified with `gcc -print-sysroot` after moving the tree), so no
# absolute-path fixups are required after unpacking.
#
# NOTE for the release job: this toolchain ships NO lld (only ld/ld.bfd).
# The arm64 release job's RUSTFLAGS "-C link-arg=-fuse-ld=lld" must NOT be
# copied to the loong64 job; linking goes through the default bfd ld.
#
# Env overrides:
#   PINVOU_LOONG64_CROSS_TOOLS_URL    full asset URL override (mirror).
#                                     The sha256 pin still applies, so a
#                                     mirror can only serve the identical
#                                     bytes.
#   PINVOU_LOONG64_CROSS_TOOLS_SHA256 expected hash override. Only needed
#                                     to bootstrap an unpinned host/variant
#                                     combination (see the pin table below);
#                                     never weaken the x86_64 pin.
#
# Idempotent: a verified tree (marker file with tag/variant/host/sha) is
# reused; --force rebuilds it.
#
# Host support: x86_64 hosts (the release runner) carry a pinned sha256.
# aarch64 hosts exist as upstream assets too, but only an explicit
# PINVOU_LOONG64_CROSS_TOOLS_SHA256 unlocks them, so a hash can never be
# used before a human recorded it here or passed it in.
#
# Local static check: bash -n scripts/ci-loongarch64-toolchain.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

TRIPLE="loongarch64-unknown-linux-gnu"
CROSS_TOOLS_TAG="20260912"
VARIANT="stable"
GCC_VERSION="14.4.0"          # release-notes GCC column for stable@20260912
GLIBC_VERSION="2.38"          # release-notes Libc(glibc) column: the deb floor

# Pinned asset: x86_64 host (GitHub release runner). Source page:
# https://github.com/loong64/cross-tools/releases/tag/20260912
# sha256 verified by downloading the asset and running sha256sum on
# 2026-10-16 (size 76443160, matching the release API listing).
CROSS_TOOLS_ASSET_X86_64="x86_64-cross-tools-${TRIPLE}-${VARIANT}.tar.xz"
CROSS_TOOLS_URL_X86_64="https://github.com/loong64/cross-tools/releases/download/${CROSS_TOOLS_TAG}/${CROSS_TOOLS_ASSET_X86_64}"
CROSS_TOOLS_SHA256_X86_64="cf68c8c59249fa6707d88b0380f8eb2e3598749958deac4d2149e149b0ccc51b"
# aarch64-hosted build of the same tag/variant exists upstream
# (aarch64-cross-tools-loongarch64-unknown-linux-gnu-stable.tar.xz) but is
# intentionally not pinned: no sha256 was recorded for it.
CROSS_TOOLS_ASSET_AARCH64="aarch64-cross-tools-${TRIPLE}-${VARIANT}.tar.xz"
CROSS_TOOLS_URL_AARCH64="https://github.com/loong64/cross-tools/releases/download/${CROSS_TOOLS_TAG}/${CROSS_TOOLS_ASSET_AARCH64}"

DEFAULT_PREFIX="$REPO_ROOT/pinvou3-app/src-tauri/target/loongarch64-cross-tools"
MARKER_NAME=".pinvou-loong64-toolchain"

help() {
  cat <<EOF
usage: $0 [--prefix <dir>] [--force] [--help]

Download, verify (sha256) and unpack the pinned loong64/cross-tools
${CROSS_TOOLS_TAG} (${VARIANT}, gcc ${GCC_VERSION}, glibc ${GLIBC_VERSION}) toolchain.

  --prefix <dir>  directory that receives the toolchain root
                  (default: $DEFAULT_PREFIX)
                  the tree itself lands at <dir>/${TRIPLE}
  --force         re-download and re-unpack even if a verified tree exists

After setup the script prints the environment (KEY=value lines) a CI step
must export to build with this toolchain.
EOF
}

usage() {
  help >&2
  exit 2
}

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

log() {
  echo "[loong64-toolchain] $*"
}

prefix=""
force=false
while [ "$#" -gt 0 ]; do
  case "$1" in
    --prefix) [ "$#" -ge 2 ] || usage; prefix="$2"; shift 2 ;;
    --force)  force=true; shift ;;
    --help)   help; exit 0 ;;
    *) usage ;;
  esac
done
[ -n "$prefix" ] || prefix="$DEFAULT_PREFIX"
mkdir -p "$prefix"
prefix="$(cd "$prefix" && pwd)"

# The tarball's top-level directory becomes the toolchain root.
toolchain_root="$prefix/$TRIPLE"
toolchain_bin="$toolchain_root/bin"
marker="$toolchain_root/$MARKER_NAME"

for command_name in curl tar sha256sum; do
  command -v "$command_name" >/dev/null || fail "missing required command: $command_name"
done

host_arch="$(uname -m)"

already_verified() {
  [ -f "$marker" ] || return 1
  # The marker is only written after a sha256-verified download+unpack (see
  # the id string built below), so trusting it here is the same trust model
  # as the smoke test that follows; re-pinning is only needed to re-download.
  [ -x "$toolchain_bin/$TRIPLE-gcc" ] || return 1
}

if [ "$force" = false ] && already_verified; then
  log "reusing verified toolchain (marker: $(cat "$marker")): $toolchain_root"
else
  case "$host_arch" in
    x86_64|amd64)
      asset_name="$CROSS_TOOLS_ASSET_X86_64"
      default_url="$CROSS_TOOLS_URL_X86_64"
      pinned_sha256="$CROSS_TOOLS_SHA256_X86_64"
      ;;
    aarch64|arm64)
      asset_name="$CROSS_TOOLS_ASSET_AARCH64"
      default_url="$CROSS_TOOLS_URL_AARCH64"
      pinned_sha256="${PINVOU_LOONG64_CROSS_TOOLS_SHA256:-}"
      [ -n "$pinned_sha256" ] || fail "no pinned sha256 for the $host_arch-hosted toolchain asset; download it once, record 'sha256sum' in this script (or pass PINVOU_LOONG64_CROSS_TOOLS_SHA256), and re-run"
      ;;
    *) fail "unsupported host architecture: $host_arch (the release runner is x86_64)" ;;
  esac

  asset_url="${PINVOU_LOONG64_CROSS_TOOLS_URL:-$default_url}"
  expected_id="${CROSS_TOOLS_TAG}/${VARIANT}/${host_arch}/${pinned_sha256}"

  if [ "$force" = false ] && [ -e "$toolchain_root" ]; then
    log "existing tree at $toolchain_root does not match the pin (or is incomplete); re-fetching"
    rm -rf -- "$toolchain_root"
  fi

  work_dir="$(mktemp -d)"
  trap 'rm -rf -- "$work_dir"' EXIT

  log "downloading $asset_url"
  curl --proto '=https' --tlsv1.2 --location --silent --show-error --fail \
    --retry 3 --retry-delay 2 \
    -o "$work_dir/$asset_name" "$asset_url" \
    || fail "download failed: $asset_url"

  echo "$pinned_sha256  $work_dir/$asset_name" | sha256sum --check --status \
    || fail "sha256 mismatch for $asset_name (expected $pinned_sha256); refusing to use the download"

  rm -rf -- "$toolchain_root"
  tar -xJf "$work_dir/$asset_name" -C "$prefix" \
    || fail "unpacking failed into $prefix"
  [ -x "$toolchain_bin/$TRIPLE-gcc" ] || fail "toolchain root incomplete: $toolchain_bin/$TRIPLE-gcc missing after unpack"

  # Tripwire: the bundled sysroot must stay on the deb's glibc floor. A
  # variant mix-up (mainline/latest instead of stable) would link binaries
  # against a newer libc and fail check-linux-glibc-floor.sh — better to
  # catch it here with a precise message.
  bundled_libc="$toolchain_root/$TRIPLE/sysroot/lib64/libc.so.6"
  [ -f "$bundled_libc" ] || fail "bundled sysroot libc missing: $bundled_libc"
  libc_version="$(strings "$bundled_libc" | sed -n 's/^GNU C Library.*version \([0-9][0-9.]*[0-9]\)\.*$/\1/p' | head -n 1)"
  [ "$libc_version" = "$GLIBC_VERSION" ] \
    || fail "bundled sysroot glibc is '${libc_version:-<unknown>}', expected $GLIBC_VERSION (wrong variant?)"

  printf '%s\n' "$expected_id" > "$marker"
  log "verified and unpacked: $toolchain_root"
fi

# Smoke test: the compiler must run and report the pinned GCC version. On the
# x86_64 release runner it runs natively and can never be skipped. On a
# foreign-arch dev machine it needs a matching-userland container (binfmt
# alone is not enough: the x86_64 loader must exist to start the binary), so
# a non-x86_64 host may set PINVOU_LOONG64_TOOLCHAIN_SKIP_SMOKE=1 to verify
# everything else; the x86_64 runner ignores the flag.
if [ "$host_arch" = "x86_64" ] || [ "${PINVOU_LOONG64_TOOLCHAIN_SKIP_SMOKE:-0}" != "1" ]; then
  gcc_version="$("$toolchain_bin/$TRIPLE-gcc" --version 2>&1 | head -n 1)" \
    || fail "$TRIPLE-gcc --version failed (x86_64 binary on a foreign-arch host: run it inside a matching-userland container, or export PINVOU_LOONG64_TOOLCHAIN_SKIP_SMOKE=1): $gcc_version"
  case "$gcc_version" in
    *" $GCC_VERSION"*) ;;
    *) fail "$TRIPLE-gcc reports '$gcc_version', expected version $GCC_VERSION (pin drift)" ;;
  esac
  "$toolchain_bin/$TRIPLE-gcc" -print-sysroot >/dev/null \
    || fail "$TRIPLE-gcc -print-sysroot failed"
fi

print_env() {
  # Both spellings: cc-rs/cmake-rs accept the underscore and the verbatim
  # triple form; cargo only knows the underscore form for the linker.
  cat <<EOF
---- ci-loongarch64-toolchain: export in the CI step ----
PINVOU_LOONG64_TOOLCHAIN_PREFIX=$toolchain_root
CC_loongarch64_unknown_linux_gnu=$toolchain_bin/$TRIPLE-gcc
CC_loongarch64-unknown-linux-gnu=$toolchain_bin/$TRIPLE-gcc
CXX_loongarch64_unknown_linux_gnu=$toolchain_bin/$TRIPLE-g++
CXX_loongarch64-unknown-linux-gnu=$toolchain_bin/$TRIPLE-g++
AR_loongarch64_unknown_linux_gnu=$toolchain_bin/$TRIPLE-gcc-ar
AR_loongarch64-unknown-linux-gnu=$toolchain_bin/$TRIPLE-gcc-ar
RANLIB_loongarch64_unknown_linux_gnu=$toolchain_bin/$TRIPLE-gcc-ranlib
RANLIB_loongarch64-unknown-linux-gnu=$toolchain_bin/$TRIPLE-gcc-ranlib
STRIP=$toolchain_bin/$TRIPLE-strip
CARGO_TARGET_LOONGARCH64_UNKNOWN_LINUX_GNU_LINKER=$toolchain_bin/$TRIPLE-gcc
---- notes ----
# gcc resolves its bundled glibc $GLIBC_VERSION sysroot from its own path
# (relocatable); no SYSROOT variable is needed for plain rust/cargo builds.
# The toolchain ships NO lld (binutils ld/ld.bfd only): do NOT copy the
# arm64 job's RUSTFLAGS "-C link-arg=-fuse-ld=lld" into the loong64 job.
EOF
}

print_env
log "toolchain ready: $toolchain_root (gcc $GCC_VERSION, glibc $GLIBC_VERSION)"
