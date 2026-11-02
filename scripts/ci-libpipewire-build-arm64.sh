#!/usr/bin/env bash
# Build PipeWire 1.0.7 (headers + libpipewire-0.3 core) for the aarch64 cross
# target into /usr/local/arm64, for the release build-linux-arm64 job since it
# moved to x86_64 cross-compilation (2026-11).
#
# Why: same jammy SPA-header gap as scripts/ci-libpipewire-build.sh (the
# native/host variant — see that script for the pipewire-rs 0.10 rationale),
# but the release arm64 deb is now cross-built on an x86_64 ubuntu-22.04
# runner, so the 1.0.7 prefix must be an aarch64 one. The multiarch
# libpipewire-0.3-dev:arm64 package is 0.3.48 — identical version gap as
# jammy's amd64 package — hence this pinned cross build.
#
# Mechanism: upstream release tarball (sha256-pinned), meson cross file
# (aarch64-linux-gnu-gcc from the gcc-aarch64-linux-gnu apt package), every
# optional backend/plugin disabled — the same validated flag set as the host
# script, which needs no further system dev packages. Install goes to
# /usr/local/arm64 (never ldconfig'd: these are target-arch objects, not
# host-executable ones); discovery happens through the job env
# PKG_CONFIG_LIBDIR, which lists /usr/local/arm64/lib/pkgconfig first.
#
# Idempotent: skips when pkg-config already resolves the pinned version under
# the arm64 prefix.
set -euo pipefail

PW_VERSION=1.0.7
PW_SHA256=9c45eef65e66224804ae8671849452a7f221e913813072b3aad346f20df666a8
PREFIX=/usr/local/arm64

# 前缀优先,但保留 multiarch 系统目录:setup 探测 glib/libcap 等 :arm64
# 依赖也走同一 pkg-config,只留前缀会让它们全部 not found。
export PKG_CONFIG_LIBDIR="$PREFIX/lib/pkgconfig:/usr/lib/aarch64-linux-gnu/pkgconfig:/usr/share/pkgconfig"

if command -v pkg-config >/dev/null 2>&1 &&
  pkg-config --exists libpipewire-0.3 2>/dev/null &&
  [[ "$(pkg-config --modversion libpipewire-0.3)" == "$PW_VERSION" ]]; then
  echo "libpipewire $PW_VERSION already resolved by pkg-config under $PREFIX; skipping build"
  exit 0
fi

for tool in aarch64-linux-gnu-gcc aarch64-linux-gnu-ar; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "FAIL: $tool not found — install gcc-aarch64-linux-gnu before running this script" >&2
    exit 1
  }
done

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

sudo apt-get update -qq
sudo apt-get install -y --no-install-recommends meson ninja-build ca-certificates curl

# Same curl retry convention as the host variant (gitlab.freedesktop.org
# transients must not hard-fail the release leg).
curl --proto '=https' --tlsv1.2 --location --silent --show-error --fail \
  --retry 3 --retry-all-errors --max-time 300 \
  "https://gitlab.freedesktop.org/pipewire/pipewire/-/archive/${PW_VERSION}/pipewire-${PW_VERSION}.tar.gz" \
  -o "$work/pipewire.tar.gz"
echo "$PW_SHA256  $work/pipewire.tar.gz" | sha256sum --check --status

tar -xzf "$work/pipewire.tar.gz" -C "$work"
cd "$work/pipewire-$PW_VERSION"

cross_file="$work/aarch64-cross.ini"
cat > "$cross_file" <<'EOF'
[binaries]
c = 'aarch64-linux-gnu-gcc'
cpp = 'aarch64-linux-gnu-g++'
ar = 'aarch64-linux-gnu-ar'
strip = 'aarch64-linux-gnu-strip'
# 键名必须是 pkgconfig(一个词):meson 0.61 的 lookup 不做连字符规范化,
# 写 pkg-config 会被当作未声明(交叉模式下又禁用 PATH 默认值),setup 必然
# 报 "Pkg-config binary for machine 1 not found"。值配合导出的
# PKG_CONFIG_LIBDIR(arm64 目录表)决定解析哪一侧。
pkgconfig = 'pkg-config'

[host_machine]
system = 'linux'
cpu_family = 'aarch64'
cpu = 'aarch64'
endian = 'little'
EOF

# Same minimal header+core-lib flag set as the host script (validated on
# jammy's meson 0.61.2, no further system dev packages required); only the
# cross file and the /usr/local/arm64 prefix differ. setup/build run as the
# job user; only the install needs sudo. No ldconfig: target-arch libs must
# stay out of the host loader/linker default path.
meson setup build --prefix="$PREFIX" -Dlibdir=lib --cross-file "$cross_file" \
  -Dtests=disabled -Ddocs=disabled -Dman=disabled -Dexamples=disabled \
  -Dpipewire-alsa=disabled -Dpipewire-jack=disabled '-Dsession-managers=[]' \
  -Dsystemd=disabled -Ddbus=disabled -Dgstreamer=disabled -Dbluez5=disabled \
  -Dvulkan=disabled -Dalsa=disabled -Djack=disabled -Davahi=disabled \
  -Decho-cancel-webrtc=disabled -Dgsettings=disabled -Dsndfile=disabled \
  -Dlibusb=disabled -Dlibcamera=disabled -Dreadline=disabled -Dx11=disabled \
  -Dlibcanberra=disabled -Dudevrulesdir=/tmp/pw-udevrules
ninja -C build
sudo ninja -C build install

found="$(pkg-config --modversion libpipewire-0.3)"
if [[ "$found" != "$PW_VERSION" ]]; then
  echo "FAIL: pkg-config resolves libpipewire-$found, expected $PW_VERSION under $PREFIX" >&2
  exit 1
fi
grep -q "spa_meta_region_is_valid" "$PREFIX/include/spa-0.2/spa/buffer/meta.h" ||
  { echo "FAIL: built SPA headers lack the pipewire-rs 0.10 inline helpers" >&2; exit 1; }
echo "libpipewire $PW_VERSION cross-built into $PREFIX (pkg-config resolves the prefix)"
