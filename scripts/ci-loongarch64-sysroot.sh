#!/usr/bin/env bash
# Produce a glibc-2.38 LoongArch64 (new-world, dpkg arch: loong64) sysroot
# holding the dev libraries the Pinvou release build links against, and
# generate a CMake toolchain file for it. Consumed by
# scripts/ci-build-linux-loongarch64.sh and by
# scripts/asr/build-sensevoice-runtime.sh (which consumes the generated
# CMAKE_TOOLCHAIN_FILE verbatim).
#
# WHY NOT the Debian trixie loong64 port (ghcr.io/loong64/debian:trixie,
# verified pullable): it ships glibc 2.41. Linking against 2.41 would raise
# the binary's symbol floor above the deb baseline — deepin 23 "beige",
# deepin 25 "crimson" and UOS V25 (deepin-25 same-source) all ship glibc
# 2.38 — so the trixie image is usable as a mechanism test only, never as
# the release sysroot. Both supported sources below come from deepin's own
# beige-family package index and carry glibc 2.38 (libc6-dev
# 2.38-6deepin13, verified in the live index on 2026-10-16).
#
# Sources (method auto = docker first, then direct download):
#   docker: extract --download-only .debs from a loong64 rootfs container
#           (default image linuxdeepin/deepin:beige-loong64; that tag could
#           not be verified from the dev network — Docker Hub unreachable —
#           so auto falls back to the apt method when the image cannot be
#           pulled or its apt cannot run). Needs qemu-loongarch64 binfmt on
#           the docker host; the CI workflow owns that setup (e.g.
#           docker/setup-qemu-action), this script only probes for it.
#   apt:    fetch the dists index, resolve the dependency closure of the
#           dev set, download and unpack every .deb (resolver: embedded
#           python3 for readable Provides/alternatives handling). Same
#           resolution semantics as apt in a single-source repo: first
#           alternative of a group, version constraints ignored (the repo
#           is self-consistent), virtual packages resolved through their
#           first sorted provider (required: libwebkit2gtk-4.1-dev depends
#           on gir1.2-gtk-3.0-dev, which beige ships only as a Provides of
#           gir1.2-gtk-3.0). Arch-qualified interpreter deps (`perl:any`,
#           `python3:any`) are skipped by design: `:any` is satisfied by
#           the build host, and the whitelist below would drop the payload
#           anyway.
#   Mirror overrides:
#     PINVOU_LOONG64_DEEPIN_MIRROR  base URL (default
#                                   https://community-packages.deepin.com/beige)
#     PINVOU_LOONG64_DEEPIN_SUITE   dists suite (default beige; for deepin
#                                   25 crimson use
#                                   PINVOU_LOONG64_DEEPIN_MIRROR=https://community-packages.deepin.com/deepin/beige
#                                   and PINVOU_LOONG64_DEEPIN_SUITE=crimson)
#     PINVOU_LOONG64_SYSROOT_IMAGE  docker image for the docker method
#     PINVOU_LOONG64_SYSROOT_METHOD force docker|apt (default auto)
#
# Dev set: the loong64 counterpart of the arm64 release job's native apt
# install (.github/workflows/release-packages.yml): libwebkit2gtk-4.1-dev
# (+ the gtk/soup/javascriptcore friends it pulls transitively), libssl-dev
# (hf-hub -> native-tls -> openssl-sys is not vendored),
# libayatana-appindicator3-dev (tray), libpipewire-0.3-dev, libdrm-dev,
# libgbm-dev, libegl-dev, libdbus-1-dev. The arm64 job additionally builds
# PipeWire 1.0.7 into /usr/local because jammy ships 0.3.48 without the SPA
# inline helpers pipewire-rs 0.10 needs; beige ships libpipewire 1.2.5, so
# the sysroot needs no equivalent of scripts/ci-libpipewire-build.sh.
#
# Size discipline (the Qomicex.Tauri #215 lesson — a naive sysroot unpacks
# to multiple GB): after unpacking, everything outside a whitelist
# (usr/include, usr/lib, usr/share/pkgconfig, lib) is deleted, plus known
# non-linking bulk under usr/lib (gconv modules, perl/python trees, locale
# archives) and all *.a/*.la (the app links the sysroot dynamically; static
# archives would only bloat it). gir-1.0 typelibs, docs, executables and
# every other non-compile payload fall outside the whitelist.
#
# Verification before the script reports success: host pkg-config must
# resolve every library of the verify set against the sysroot via
# PKG_CONFIG_LIBDIR + PKG_CONFIG_SYSROOT_DIR (the host pkg-config binary
# reads foreign .pc files fine), libc dev must be present, the libc binary
# must be a LoongArch ELF and its glibc version must not be below 2.38
# (a newer version is a loud warning, not an error: only referenced symbols
# matter, and check-linux-glibc-floor.sh judges those on the final deb).
#
# Local static check: bash -n scripts/ci-loongarch64-sysroot.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

TRIPLE="loongarch64-unknown-linux-gnu"
DPKG_ARCH="loong64"
MULTIARCH_DIR="loongarch64-linux-gnu"
GLIBC_FLOOR="2.38"   # deepin 23/25 + UOS V25; keep in sync with the elf policy

DEFAULT_MIRROR="https://community-packages.deepin.com/beige"
DEFAULT_SUITE="beige"
DEFAULT_DOCKER_IMAGE="linuxdeepin/deepin:beige-loong64"
DEFAULT_OUTPUT="$REPO_ROOT/pinvou3-app/src-tauri/target/loongarch64-sysroot"
DEFAULT_TOOLCHAIN_PREFIX="$REPO_ROOT/pinvou3-app/src-tauri/target/loongarch64-cross-tools/loongarch64-unknown-linux-gnu"
CMAKE_TOOLCHAIN_NAME="pinvou-loongarch64-toolchain.cmake"
MANIFEST_NAME="pinvou-sysroot-manifest.txt"

# Same package list as the arm64 release job installs natively (host tools
# build-essential/pkg-config/cmake/lld excluded: they are the runner's).
DEV_PACKAGES=(
  libwebkit2gtk-4.1-dev
  libssl-dev
  libayatana-appindicator3-dev
  libpipewire-0.3-dev
  libdrm-dev
  libgbm-dev
  libegl-dev
  libdbus-1-dev
)

# .pc modules that must resolve against the finished sysroot: one per dev
# package of the seed set, plus the transitive pkg-config deps the build
# actually probes (glib/gio/gobject, gtk, soup, javascriptcore).
VERIFY_PC_MODULES=(
  webkit2gtk-4.1
  javascriptcoregtk-4.1
  libsoup-3.0
  gtk+-3.0
  gdk-3.0
  glib-2.0
  gio-2.0
  gobject-2.0
  ayatana-appindicator3-0.1
  openssl
  libssl
  libcrypto
  libpipewire-0.3
  libdrm
  gbm
  egl
  dbus-1
)

help() {
  cat <<EOF
usage: $0 [--output <dir>] [--method auto|docker|apt] [--toolchain-prefix <dir>]
          [--packages <comma,list>] [--force] [--help]

Build a glibc ${GLIBC_FLOOR} loong64 sysroot with the dev set the release build links.

  --output <dir>          sysroot directory (default: $DEFAULT_OUTPUT)
  --method <m>            auto (default: try docker, fall back to apt),
                          docker, or apt
  --toolchain-prefix <d>  cross toolchain root (the directory containing bin/)
                          used for the generated CMake toolchain file
                          (default: $DEFAULT_TOOLCHAIN_PREFIX)
  --packages <list>       override the dev package seed set (comma-separated;
                          testing/debugging escape hatch — the default is the
                          audited release set)
  --force                 rebuild even if a manifest for the same source exists
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
  echo "[loong64-sysroot] $*"
}

output_dir=""
method="${PINVOU_LOONG64_SYSROOT_METHOD:-auto}"
toolchain_prefix="${PINVOU_LOONG64_TOOLCHAIN_PREFIX:-}"
packages_csv=""
force=false
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output)           [ "$#" -ge 2 ] || usage; output_dir="$2"; shift 2 ;;
    --method)           [ "$#" -ge 2 ] || usage; method="$2"; shift 2 ;;
    --toolchain-prefix) [ "$#" -ge 2 ] || usage; toolchain_prefix="$2"; shift 2 ;;
    --packages)         [ "$#" -ge 2 ] || usage; packages_csv="$2"; shift 2 ;;
    --force)            force=true; shift ;;
    --help)             help; exit 0 ;;
    *)                  usage ;;
  esac
done
[ -n "$output_dir" ] || output_dir="$DEFAULT_OUTPUT"
case "$method" in
  auto|docker|apt) ;;
  *) fail "unknown --method '$method' (auto|docker|apt)" ;;
esac
if [ -n "$toolchain_prefix" ] && [ ! -d "$toolchain_prefix" ]; then
  toolchain_prefix=""
fi
mkdir -p "$output_dir"
output_dir="$(cd "$output_dir" && pwd)"

mirror="${PINVOU_LOONG64_DEEPIN_MIRROR:-$DEFAULT_MIRROR}"
suite="${PINVOU_LOONG64_DEEPIN_SUITE:-$DEFAULT_SUITE}"
docker_image="${PINVOU_LOONG64_SYSROOT_IMAGE:-$DEFAULT_DOCKER_IMAGE}"

if [ -n "$packages_csv" ]; then
  IFS=, read -r -a DEV_PACKAGES <<< "$packages_csv"
fi

for command_name in curl gzip dpkg dpkg-deb readelf find pkg-config strings sha256sum python3; do
  command -v "$command_name" >/dev/null || fail "missing required command: $command_name"
done

sysroot_glibc_version() {
  strings "$1" | sed -n 's/^GNU C Library.*version \([0-9][0-9.]*[0-9]\)\.*$/\1/p' | head -n 1
}

# Point the CURRENT shell's pkg-config at the sysroot (exported, so the
# verification calls below and any later consumer in this process see it).
pkg_config_env() {
  export PKG_CONFIG_LIBDIR="$1/usr/lib/$MULTIARCH_DIR/pkgconfig:$1/usr/share/pkgconfig:$1/usr/lib/pkgconfig"
  export PKG_CONFIG_SYSROOT_DIR="$1"
}

# ---- shared post-extraction: whitelist prune (Qomicex #215 size discipline) --
prune_sysroot() {
  local root="$1"
  # Whitelist: usr/include, usr/lib, usr/share/pkgconfig, lib. Everything
  # else (etc, bin, sbin, var, usr/bin, docs, man, locales, gir typelibs,
  # gtk-doc, dbus system configs, udev rules, ...) is irrelevant to
  # compiling/linking and is removed.
  find "$root" -mindepth 1 -maxdepth 1 ! -name usr ! -name lib -exec rm -rf -- {} +
  if [ -d "$root/usr" ]; then
    (cd "$root/usr" && find . -mindepth 1 -maxdepth 1 \
      ! -name include ! -name lib ! -name share -exec rm -rf -- {} +)
  fi
  if [ -d "$root/usr/share" ]; then
    (cd "$root/usr/share" && find . -mindepth 1 -maxdepth 1 \
      ! -name pkgconfig -exec rm -rf -- {} +)
  fi
  # Tighten usr/lib to what a cross build can reference: the multiarch tree,
  # cmake package configs and pkgconfig. systemd/aspell/python/emacsen/
  # tmpfiles payload under usr/lib is unreachable from any -l or -I flag.
  if [ -d "$root/usr/lib" ]; then
    (cd "$root/usr/lib" && find . -mindepth 1 -maxdepth 1 \
      ! -name "$MULTIARCH_DIR" ! -name cmake ! -name pkgconfig \
      -exec rm -rf -- {} +)
  fi
  # Runtime-only bulk inside the multiarch tree: dlopened/runtime files no
  # -l flag references. mesa driver internals (libLLVM/libgallium/libz3 are
  # pulled as libEGL/libgbm's runtime Depends; nothing in the build links
  # them — verified: no dev .so symlink points at them), gstreamer plugins
  # (core lib .so stays), gir typelibs, glibc gconv modules, perl trees,
  # systemd units. Every versioned .so that a dev symlink points at is kept.
  if [ -d "$root/usr/lib/$MULTIARCH_DIR" ]; then
    (cd "$root/usr/lib/$MULTIARCH_DIR" && rm -rf -- \
      libLLVM.so.* libLLVM-*.so libgallium*.so libz3.so.* \
      gconv perl perl-base gir-1.0 girepository-1.0 gstreamer-1.0 systemd)
  fi
  # Static archives and libtool files: the app links the sysroot
  # dynamically; keeping .a would only bloat it.
  find "$root" \( -name '*.a' -o -name '*.la' \) -type f -delete
}

# ---- shared extraction -------------------------------------------------------
extract_debs_into() {
  local debs_dir="$1" root="$2" deb name
  [ -d "$root" ] || fail "sysroot root missing: $root"
  # With nullglob off, a missing glob falls through as the literal '*.deb'
  # and the -e guard below reports it.
  for deb in "$debs_dir"/*.deb; do
    [ -e "$deb" ] || fail "no .deb files found in $debs_dir"
    name="$(dpkg-deb -f "$deb" Package 2>/dev/null || true)"
    [ -n "$name" ] || fail "unreadable .deb (corrupt download?): $deb"
    dpkg-deb -x "$deb" "$root" || fail "dpkg-deb -x failed for $name ($deb)"
  done
}

# ---- verification ------------------------------------------------------------
verify_sysroot() {
  local root="$1" pcmod machine libc_version
  [ -f "$root/usr/include/stdio.h" ] || fail "sysroot lacks libc headers: $root/usr/include/stdio.h"
  [ -f "$root/usr/lib/$MULTIARCH_DIR/libc.so" ] \
    || fail "sysroot lacks the linker's libc.so: $root/usr/lib/$MULTIARCH_DIR/libc.so"

  local libc_so="$root/usr/lib/$MULTIARCH_DIR/libc.so.6"
  [ -f "$libc_so" ] || fail "sysroot lacks libc.so.6: $libc_so"
  machine="$(LC_ALL=C readelf -h "$libc_so" 2>/dev/null | sed -n 's/^[[:space:]]*Machine:[[:space:]]*//p')"
  [ "$machine" = "LoongArch" ] \
    || fail "sysroot libc is not a LoongArch ELF (readelf Machine: '${machine:-<none>}')"
  libc_version="$(sysroot_glibc_version "$libc_so")"
  [ -n "$libc_version" ] || fail "cannot determine the sysroot glibc version of $libc_so"
  if dpkg --compare-versions "$libc_version" lt "$GLIBC_FLOOR"; then
    fail "sysroot glibc $libc_version is below the $GLIBC_FLOOR floor of deepin 23/25 + UOS V25"
  fi
  if dpkg --compare-versions "$libc_version" gt "$GLIBC_FLOOR"; then
    echo "WARNING: sysroot glibc is $libc_version > floor $GLIBC_FLOOR; binaries only inherit" >&2
    echo "         the symbols they reference, but the final deb MUST pass" >&2
    echo "         scripts/check-linux-glibc-floor.sh" >&2
  fi

  # Host pkg-config reads the foreign .pc files through the sysroot env.
  pkg_config_env "$root"
  for pcmod in "${VERIFY_PC_MODULES[@]}"; do
    if ! pkg-config --exists "$pcmod" 2>/dev/null; then
      fail "pkg-config cannot resolve '$pcmod' against the sysroot (see PKG_CONFIG_LIBDIR/SYSROOT_DIR printed below)"
    fi
    pkg-config --silence-errors --cflags --libs "$pcmod" >/dev/null \
      || fail "pkg-config --cflags --libs failed for '$pcmod'"
  done
}

write_cmake_toolchain_file() {
  local root="$1" prefix="$2" file
  if [ -z "$prefix" ]; then
    prefix="$DEFAULT_TOOLCHAIN_PREFIX"
  fi
  [ -x "$prefix/bin/$TRIPLE-gcc" ] \
    || fail "cannot write $CMAKE_TOOLCHAIN_NAME: no bin/$TRIPLE-gcc under '$prefix' (run scripts/ci-loongarch64-toolchain.sh first or pass --toolchain-prefix)"
  file="$root/$CMAKE_TOOLCHAIN_NAME"
  {
    echo "# Generated by scripts/ci-loongarch64-sysroot.sh — do not edit."
    echo "# Consumed for the loong64 cross build (build-sensevoice-runtime.sh and"
    echo "# any cmake-rs build script); mirrors the interface of the toolchain.cmake"
    echo "# shipped in the loong64/cross-tools tarball, with CMAKE_SYSROOT pointed"
    echo "# at this deepin glibc ${GLIBC_FLOOR} tree."
    echo "set(CMAKE_SYSTEM_NAME Linux)"
    echo "set(CMAKE_SYSTEM_PROCESSOR loongarch64)"
    echo "set(CMAKE_SYSROOT \"$root\")"
    echo "set(CMAKE_C_COMPILER \"$prefix/bin/$TRIPLE-gcc\")"
    echo "set(CMAKE_CXX_COMPILER \"$prefix/bin/$TRIPLE-g++\")"
    echo "set(CMAKE_AR \"$prefix/bin/$TRIPLE-gcc-ar\")"
    echo "set(CMAKE_RANLIB \"$prefix/bin/$TRIPLE-gcc-ranlib\")"
    echo "set(CMAKE_STRIP \"$prefix/bin/$TRIPLE-strip\")"
    echo "set(CMAKE_FIND_ROOT_PATH \"$root\")"
    echo "set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)"
    echo "set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)"
    echo "set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)"
    echo "set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)"
  } > "$file"
  printf '%s\n' "$file"
}

# ---- docker method -----------------------------------------------------------
work_dir=""

docker_binfmt_ready() {
  [ -f /proc/sys/fs/binfmt_misc/qemu-loongarch64 ]
}

docker_method() {
  if ! command -v docker >/dev/null; then
    echo "[loong64-sysroot] docker not available" >&2
    return 1
  fi
  if ! docker_binfmt_ready; then
    echo "[loong64-sysroot] qemu-loongarch64 binfmt is not registered on the docker host; the" >&2
    echo "[loong64-sysroot] CI workflow must set it up (e.g. docker/setup-qemu-action, or" >&2
    echo "[loong64-sysroot] docker run --privileged tonistiigi/binfmt --install loongarch64)" >&2
    return 1
  fi
  local container="pinvou-sysroot-extract-$$"
  work_dir="$(mktemp -d)"
  trap 'rm -rf -- "${work_dir:-}"' EXIT

  log "docker method: pulling $docker_image"
  docker pull --platform "linux/loong64" "$docker_image" >&2 || {
    echo "[loong64-sysroot] pulling $docker_image failed" >&2
    return 1
  }
  log "docker method: apt-get --download-only inside the container"
  # %q keeps the seed list safe across whitespace; the container's own
  # sources.list points at the deepin repo the image was built from.
  # The runtime base is requested EXPLICITLY with --reinstall: apt never
  # adds an already-installed Essential package (libc6, libgcc-s1, ...) to
  # a download transaction, so the plain --download-only cache lacks the
  # very libs every ELF needs (observed on a trixie probe: no libc6 in the
  # cache, sysroot without libc.so.6). libc6-dev brings the headers and
  # linker script; the direct apt method does not need this because its
  # resolver downloads every Depends member, including Essential ones.
  local pkg_list
  pkg_list="$(printf '%q ' "${DEV_PACKAGES[@]}" libc6 libgcc-s1 libstdc++6 libc6-dev)"
  docker run --name "$container" --platform "linux/loong64" \
    "$docker_image" \
    bash -c "rm -f /var/lib/apt/lists/lock /var/cache/apt/archives/lock /var/lib/dpkg/lock*; \
             apt-get update -qq && \
             apt-get install --download-only --no-install-recommends --reinstall -y $pkg_list" >&2 || {
    echo "[loong64-sysroot] container apt install failed" >&2
    docker rm -f "$container" >/dev/null 2>&1 || true
    return 1
  }
  mkdir -p "$work_dir/debs"
  docker cp "$container:/var/cache/apt/archives/." "$work_dir/debs/" >&2 || {
    echo "[loong64-sysroot] docker cp of the apt cache failed" >&2
    docker rm -f "$container" >/dev/null 2>&1 || true
    return 1
  }
  docker rm -f "$container" >/dev/null 2>&1 || true
  # Partial downloads are normal in the apt cache; require every seed
  # package to be present so a silently partial resolution cannot pass.
  local pkg deb deb_found=0
  for pkg in "${DEV_PACKAGES[@]}"; do
    deb_found=0
    for deb in "$work_dir/debs"/*.deb; do
      [ -e "$deb" ] || break
      if [ "$(dpkg-deb -f "$deb" Package 2>/dev/null)" = "$pkg" ]; then
        deb_found=1
        break
      fi
    done
    if [ "$deb_found" != 1 ]; then
      echo "[loong64-sysroot] seed package $pkg missing from the container apt cache" >&2
      return 1
    fi
  done
  extract_debs_into "$work_dir/debs" "$output_dir"
  prune_sysroot "$output_dir"
  SOURCE_DESCRIPTION="docker:$docker_image"
}

# ---- apt method (direct download from the deepin index) ----------------------
apt_method() {
  work_dir="$(mktemp -d)"
  trap 'rm -rf -- "${work_dir:-}"' EXIT

  # All components of the suite, from the Release file (beige: main; crimson
  # needs main + community — crimson's libwebkit2gtk-4.1-dev depends on
  # gir1.2-soup-3.0-dev, which its main component does not ship but its
  # community component's newer libsoup-3.0-dev Provides). First-seen
  # package wins, so main keeps priority over the extra components.
  local release_url="$mirror/dists/$suite/Release"
  local components
  components="$(curl --proto '=https' --tlsv1.2 --location --silent --show-error --fail \
    --retry 3 --retry-delay 2 "$release_url" \
    | sed -n 's/^Components:[[:space:]]*//p' | tr -s '[:space:]' ' ' | sed 's/^ //;s/ $//')"
  [ -n "$components" ] || fail "cannot read Components from $release_url (mirror/suite mismatch?)"
  log "apt method: suite $suite components: $components"

  : > "$work_dir/Packages"
  local comp
  for comp in $components; do
    local index_url="$mirror/dists/$suite/$comp/binary-$DPKG_ARCH/Packages.gz"
    log "apt method: fetching $index_url"
    curl --proto '=https' --tlsv1.2 --location --silent --show-error --fail \
      --retry 3 --retry-delay 2 -o "$work_dir/Packages.$comp.gz" "$index_url" \
      || fail "index download failed: $index_url"
    gzip -t "$work_dir/Packages.$comp.gz" || fail "index is not valid gzip: $work_dir/Packages.$comp.gz"
    zcat "$work_dir/Packages.$comp.gz" >> "$work_dir/Packages.raw"
  done

  # Merge components with apt candidate semantics: one stanza per package
  # name, keeping the HIGHEST Debian version (crimson's main ships an older
  # libsoup-3.0-dev without the gir1.2-soup-3.0-dev Provides that its
  # libwebkit2gtk-4.1-dev needs; the newer community stanza wins exactly
  # like apt's candidate selection across components).
  python3 - "$work_dir/Packages.raw" > "$work_dir/Packages" <<'PYEOF'
import sys

# dpkg version comparison (verrevcmp), faithful translation: non-digit
# runs compare by char order where '~' < end-of-string < letters < other;
# digit runs compare numerically.
def order(c):
    if c.isdigit():
        return 0
    if c.isalpha():
        return ord(c)
    if c == "~":
        return -1
    if c:
        return ord(c) + 256
    return 0

def verrevcmp(a, b):
    i = j = 0
    while i < len(a) or j < len(b):
        while (i < len(a) and not a[i].isdigit()) or (j < len(b) and not b[j].isdigit()):
            ac = order(a[i]) if i < len(a) else 0
            bc = order(b[j]) if j < len(b) else 0
            if ac != bc:
                return ac - bc
            i += 1
            j += 1
        da = i
        while da < len(a) and a[da].isdigit():
            da += 1
        db = j
        while db < len(b) and b[db].isdigit():
            db += 1
        va = int(a[i:da]) if da > i else 0
        vb = int(b[j:db]) if db > j else 0
        if va != vb:
            return va - vb
        i, j = da, db
    return 0

def version_gt(a, b):
    # Debian versions compare epoch first ("4:24.2.0" beats "24.x"), then
    # the epoch-stripped rest via verrevcmp. partition() without a match
    # yields (whole, '', '') — the whole string IS the version then.
    ea, sep, ra = a.partition(":")
    if not sep or not ea.isdigit():
        a_epoch, ra = 0, a
    else:
        a_epoch = int(ea)
    eb, sep, rb = b.partition(":")
    if not sep or not eb.isdigit():
        b_epoch, rb = 0, b
    else:
        b_epoch = int(eb)
    if a_epoch != b_epoch:
        return a_epoch > b_epoch
    return verrevcmp(ra, rb) > 0

seen = {}   # name -> (version, stanza lines of the highest version)
stanza = []
name = None
version = None

def flush():
    if not stanza or not name:
        return
    current = seen.get(name)
    if current is None or version_gt(version, current[0]):
        seen[name] = (version, stanza)

for raw in open(sys.argv[1], encoding="utf-8", errors="replace"):
    line = raw.rstrip("\n")
    if not line.strip():
        flush()
        stanza = []
        name = None
        version = None
        continue
    if not line[0] in " \t":
        if line.startswith("Package:"):
            if name is not None:
                # New stanza without a blank separator; flush the previous one.
                flush()
                stanza = []
                version = None
            name = line.partition(":")[2].strip()
        elif line.startswith("Version:"):
            version = line.partition(":")[2].strip()
    stanza.append(line)
flush()

out = []
for name in sorted(seen):
    out.extend(seen[name][1])
sys.stdout.write("\n".join(out) + ("\n" if out else ""))
PYEOF

  local index_sha
  index_sha="$(sha256sum "$work_dir/Packages" | awk '{print $1}')"
  log "apt method: merged index sha256 $index_sha ($(grep -c '^Package:' "$work_dir/Packages") packages)"

  # Resolve the dependency closure with apt's single-repo semantics (see the
  # header) and emit one "name<TAB>Filename<TAB>sha256" line per package.
  python3 - "$work_dir/Packages" "${DEV_PACKAGES[@]}" > "$work_dir/debs.txt" <<'PYEOF' || fail "dependency resolution failed (see the unresolvable list above)"
import sys

index_path, *seeds = sys.argv[1:]

fields = {}
provides_map = {}
order = []
cur = None
key = None
with open(index_path, encoding="utf-8", errors="replace") as fh:
    for raw in fh:
        line = raw.rstrip("\n")
        if not line.strip():
            cur = None
            continue
        if line[0] in " \t" and cur is not None and key is not None:
            # Continuation line of a wrapped field (long Depends, ...).
            fields[cur][key] += " " + line.strip()
            continue
        key, sep, value = line.partition(":")
        if not sep:
            continue
        key = key.strip()
        value = value.strip()
        if key == "Package":
            cur = value
            if cur not in fields:
                fields[cur] = {}
                order.append(cur)
            continue
        if cur is not None:
            fields[cur][key] = value

for name in order:
    for prov in fields[name].get("Provides", "").split(","):
        prov = prov.strip()
        if prov:
            provides_map.setdefault(prov.split()[0], []).append(name)

def dep_groups(pkg):
    text = " ".join(filter(None, (fields[pkg].get("Depends", ""), fields[pkg].get("Pre-Depends", ""))))
    for group in text.split(","):
        group = group.strip()
        if group:
            yield group

def alternatives(group):
    return [alt.strip() for alt in group.split("|") if alt.strip()]

def is_any_qualified(alt):
    # `python3:any` / `perl:any`: satisfiable by the build host's
    # interpreter; the sysroot whitelist drops the payload anyway.
    return ":any" in alt.split()[0]

seen = set()
missing = []
skipped_any = 0
work = list(seeds)
while work:
    want = work.pop()
    if want in seen:
        continue
    seen.add(want)
    if want not in fields:
        providers = provides_map.get(want, [])
        if providers:
            work.append(sorted(providers)[0])
        else:
            missing.append(want)
        continue
    for group in dep_groups(want):
        chosen = None
        had_non_any = False
        for alt in alternatives(group):
            if is_any_qualified(alt):
                skipped_any += 1
                continue
            had_non_any = True
            name = alt.split()[0].split(":")[0]
            if name in fields:
                chosen = name
                break
            providers = provides_map.get(name, [])
            if providers:
                chosen = sorted(providers)[0]
                break
        if chosen is not None:
            work.append(chosen)
        elif had_non_any:
            # Unsatisfiable non-:any deps are hard errors; name the
            # originating package for a precise message.
            missing.append(f"{want} -> {group}")
        # else: the whole group was :any interpreter deps — skip.

if missing:
    print("unresolvable dependencies:", file=sys.stderr)
    for m in sorted(set(missing)):
        print(f"  {m}", file=sys.stderr)
    sys.exit(1)

resolved = sorted(n for n in seen if n in fields)
print(f"resolved {len(resolved)} packages ({skipped_any} :any interpreter deps skipped)", file=sys.stderr)
for name in resolved:
    data = fields[name]
    filename = data.get("Filename")
    sha = data.get("SHA256", "")
    if not filename:
        print(f"package without Filename field: {name}", file=sys.stderr)
        sys.exit(1)
    print(f"{name}\t{filename}\t{sha}")
PYEOF

  local debs_dir="$work_dir/debs"
  mkdir -p "$debs_dir"
  local total downloaded=0 name filename sha url deb_path actual_name
  total="$(wc -l < "$work_dir/debs.txt")"
  while IFS=$'\t' read -r name filename sha; do
    url="$mirror/$filename"
    deb_path="$debs_dir/$(basename "$filename")"
    if [ ! -s "$deb_path" ]; then
      curl --proto '=https' --tlsv1.2 --location --silent --show-error --fail \
        --retry 3 --retry-delay 2 -o "$deb_path" "$url" \
        || fail "download failed: $url"
    fi
    if [ -n "$sha" ]; then
      echo "$sha  $deb_path" | sha256sum --check --status \
        || fail "sha256 mismatch for $name ($deb_path) against the index"
    else
      echo "WARNING: index has no SHA256 for $name; skipping hash check" >&2
    fi
    actual_name="$(dpkg-deb -f "$deb_path" Package 2>/dev/null || true)"
    [ "$actual_name" = "$name" ] \
      || fail "downloaded $url but it is package '${actual_name:-<unreadable>}', expected $name"
    downloaded=$((downloaded + 1))
    if [ $((downloaded % 50)) -eq 0 ]; then
      log "downloaded $downloaded/$total"
    fi
  done < "$work_dir/debs.txt"
  log "downloaded $downloaded debs"

  extract_debs_into "$debs_dir" "$output_dir"
  prune_sysroot "$output_dir"
  SOURCE_DESCRIPTION="apt:$mirror@$suite"
  APT_INDEX_SHA="$index_sha"
}

# ---- run ----------------------------------------------------------------------
manifest="$output_dir/$MANIFEST_NAME"

already_built() {
  [ -f "$manifest" ] || return 1
  [ -f "$output_dir/$CMAKE_TOOLCHAIN_NAME" ] || return 1
  grep -Fxq "glibc-floor=$GLIBC_FLOOR" "$manifest" || return 1
  grep -Fxq "packages=$(IFS=,; echo "${DEV_PACKAGES[*]}")" "$manifest" || return 1
}

if [ "$force" = false ] && already_built; then
  log "reusing sysroot at $output_dir ($(head -n 2 "$manifest" | tr '\n' ' '))"
else
  SOURCE_DESCRIPTION=""
  APT_INDEX_SHA=""
  case "$method" in
    docker)
      docker_method || fail "docker method failed (see above); fix the docker setup or use --method apt"
      ;;
    apt)
      apt_method
      ;;
    auto)
      if docker_method; then
        log "auto: docker method succeeded"
      else
        log "auto: falling back to the direct apt method"
        rm -rf -- "${work_dir:-}"
        apt_method
      fi
      ;;
  esac

  verify_sysroot "$output_dir"
  cmake_file="$(write_cmake_toolchain_file "$output_dir" "$toolchain_prefix")"

  {
    echo "source=$SOURCE_DESCRIPTION"
    echo "glibc-floor=$GLIBC_FLOOR"
    echo "packages=$(IFS=,; echo "${DEV_PACKAGES[*]}")"
    if [ -n "$APT_INDEX_SHA" ]; then
      echo "index-sha256=$APT_INDEX_SHA"
    fi
    echo "created=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  } > "$manifest"
  log "sysroot ready: $output_dir (glibc $(sysroot_glibc_version "$output_dir/usr/lib/$MULTIARCH_DIR/libc.so.6"))"
  log "cmake toolchain file: $cmake_file"
fi

pkg_config_env "$output_dir"
sysroot_size="$(du -sh "$output_dir" | awk '{print $1}')"
cat <<EOF
---- ci-loongarch64-sysroot: export in the CI step ----
PINVOU_LOONG64_SYSROOT=$output_dir
PKG_CONFIG_LIBDIR=$output_dir/usr/lib/$MULTIARCH_DIR/pkgconfig:$output_dir/usr/share/pkgconfig:$output_dir/usr/lib/pkgconfig
PKG_CONFIG_SYSROOT_DIR=$output_dir
CMAKE_TOOLCHAIN_FILE=$output_dir/$CMAKE_TOOLCHAIN_NAME
---- notes ----
# Do NOT set PKG_CONFIG_PATH (it appends to the host's default dirs); the
# LIBDIR above fully replaces the search path. The host pkg-config binary
# is used as-is (it reads foreign .pc files); no target pkg-config needed.
# The generated $CMAKE_TOOLCHAIN_NAME sets CMAKE_SYSTEM_NAME /
# CMAKE_SYSTEM_PROCESSOR=loongarch64, the cross compilers from the
# toolchain prefix and CMAKE_SYSROOT — exactly the interface
# scripts/asr/build-sensevoice-runtime.sh consumes, and what cmake-rs picks
# up from the env for any cmake-based -sys crate in the cargo graph.
---- size ----
# sysroot size: $sysroot_size
EOF
