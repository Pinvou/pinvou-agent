#!/usr/bin/env bash
# Build PipeWire 1.0.7 (headers + libpipewire-0.3 core) into /usr/local for the
# Linux rust CI jobs.
#
# Why: the computer_use stack needs pipewire-rs 0.10 (direct dependency for the
# Wayland ScreenCast frames in wayland_capture, and transitively via xcap
# 0.9.8). libspa-sys generates its FFI bindings with bindgen from the SYSTEM
# headers at build time, and pipewire-rs 0.10 requires the SPA static inline
# helpers (spa_meta_region_is_valid / spa_meta_first / spa_video_info_raw.flags)
# that only exist in headers newer than the 0.3.48 that ubuntu-22.04 (#511
# release-baseline pin) ships — without this script the crate graph fails with
# `cannot find function spa_meta_region_is_valid in crate spa_sys` on all three
# Linux rust legs. Building the upstream release into /usr/local keeps the
# runner image pinned (no PPA, no image drift) while giving the build the
# headers it needs; /usr/local precedes /usr in pkg-config's default search
# path, so the prefix wins without touching the system install.
#
# Idempotent: skips when pkg-config already resolves the pinned version.
# CI-only: the pr-check rust-lint/rust-test/cli-test legs and the two Linux
# deb jobs in release-packages.yml.
set -euo pipefail

PW_VERSION=1.0.7
PW_SHA256=9c45eef65e66224804ae8671849452a7f221e913813072b3aad346f20df666a8

if command -v pkg-config >/dev/null 2>&1 &&
  pkg-config --exists libpipewire-0.3 2>/dev/null &&
  [[ "$(pkg-config --modversion libpipewire-0.3)" == "$PW_VERSION" ]]; then
  echo "libpipewire $PW_VERSION already resolved by pkg-config; skipping build"
  exit 0
fi

work="$(mktemp -d)"
trap 'sudo rm -rf "$work"' EXIT

sudo apt-get update -qq
sudo apt-get install -y --no-install-recommends meson ninja-build ca-certificates curl

curl --proto '=https' --tlsv1.2 --location --silent --show-error --fail \
  "https://gitlab.freedesktop.org/pipewire/pipewire/-/archive/${PW_VERSION}/pipewire-${PW_VERSION}.tar.gz" \
  -o "$work/pipewire.tar.gz"
echo "$PW_SHA256  $work/pipewire.tar.gz" | sha256sum --check --status

tar -xzf "$work/pipewire.tar.gz" -C "$work"
cd "$work/pipewire-$PW_VERSION"

# Minimal header+core-lib build: every optional backend/plugin disabled; the
# default spa-plugins set stays on (the pw modules reference it), which only
# adds header-free core C plugins. Validated flags, meson >= 0.61.1 (jammy
# ships 0.61.2), no further system dev packages required. setup/build run as
# the job user (a root-owned build dir would break ninja and the cleanup
# trap); only the install into /usr/local and ldconfig need sudo.
meson setup build --prefix=/usr/local -Dlibdir=lib \
  -Dtests=disabled -Ddocs=disabled -Dman=disabled -Dexamples=disabled \
  -Dpipewire-alsa=disabled -Dpipewire-jack=disabled -Dsession-managers=[] \
  -Dsystemd=disabled -Ddbus=disabled -Dgstreamer=disabled -Dbluez5=disabled \
  -Dvulkan=disabled -Dalsa=disabled -Djack=disabled -Davahi=disabled \
  -Decho-cancel-webrtc=disabled -Dgsettings=disabled -Dsndfile=disabled \
  -Dlibusb=disabled -Dlibcamera=disabled -Dreadline=disabled -Dx11=disabled \
  -Dlibcanberra=disabled -Dudevrulesdir=/tmp/pw-udevrules
ninja -C build
sudo ninja -C build install
sudo ldconfig

found="$(pkg-config --modversion libpipewire-0.3)"
if [[ "$found" != "$PW_VERSION" ]]; then
  echo "FAIL: pkg-config resolves libpipewire-$found, expected $PW_VERSION from /usr/local" >&2
  exit 1
fi
grep -q "spa_meta_region_is_valid" /usr/local/include/spa-0.2/spa/buffer/meta.h ||
  { echo "FAIL: built SPA headers lack the pipewire-rs 0.10 inline helpers" >&2; exit 1; }
echo "libpipewire $PW_VERSION built into /usr/local (pkg-config resolves the prefix)"
