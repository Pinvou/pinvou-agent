#!/usr/bin/env bash
# Build a shippable ASR engine (sense-voice-main) for the current Linux host
# from the pinned SenseVoice.cpp commit. Must run on Ubuntu 22.04 or older:
# the output links the build host's glibc, and the post-build validation
# rejects symbols above the 22.04 (glibc 2.35) baseline; release CI builds
# on ubuntu-22.04 runners.
#
# --arch loongarch64 (deepin 23/25 + UOS V25, glibc 2.38 baseline) is an
# env-driven cross build: no host==requested check, CMake driven by CC/CXX
# pointing at loongarch64 cross compilers or by CMAKE_TOOLCHAIN_FILE, and
# STRIP naming the target strip (default loongarch64-linux-gnu-strip) since
# the host strip cannot handle foreign-arch ELFs. The engine links
# libstdc++/libgcc statically (the deb's only C++ artifact, so no
# GLIBCXX_/CXXABI_ requirements) and the result is validated against the
# loongarch64 policy (GLIBC_ <= 2.38) on the build host.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=sensevoice-source.env
source "$SCRIPT_DIR/sensevoice-source.env"

usage() {
  echo "usage: $0 --output <path> --arch <x86_64|aarch64|loongarch64>" >&2
  exit 2
}

output_path=""
requested_arch=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output)
      [ "$#" -ge 2 ] || usage
      output_path="$2"
      shift 2
      ;;
    --arch)
      [ "$#" -ge 2 ] || usage
      requested_arch="$2"
      shift 2
      ;;
    *) usage ;;
  esac
done
[ -n "$output_path" ] && [ -n "$requested_arch" ] || usage

case "$(uname -m)" in
  x86_64|amd64) host_arch="x86_64" ;;
  aarch64|arm64) host_arch="aarch64" ;;
  loongarch64) host_arch="loongarch64" ;;
  *) echo "unsupported Linux build architecture: $(uname -m)" >&2; exit 1 ;;
esac

case "$requested_arch" in
  x86_64) target_arch="x86_64"; policy_arch="amd64"; cross_mode=0 ;;
  aarch64) target_arch="aarch64"; policy_arch="arm64"; cross_mode=0 ;;
  # loongarch64: release CI cross-compiles on an x86_64/arm64 runner, so the
  # host==requested assertion below is skipped and the cross toolchain comes
  # from the environment instead (see the dependency pre-check and cmake
  # invocation).
  loongarch64) target_arch="loongarch64"; policy_arch="loongarch64"; cross_mode=1 ;;
  *) echo "unsupported requested SenseVoice architecture: $requested_arch" >&2; exit 1 ;;
esac

if [ "$cross_mode" -eq 0 ]; then
  [ "$requested_arch" = "$host_arch" ] || {
    echo "requested SenseVoice architecture $requested_arch does not match build host $host_arch" >&2
    exit 1
  }
fi

output_path="$(realpath -m "$output_path")"
output_dir="$(dirname "$output_path")"
# Build tree per target architecture: a cross loongarch64 build must not
# clobber or reuse a native x86_64/aarch64 tree (the values coincide for
# native builds).
build_root="${PINVOU3_SENSEVOICE_BUILD_ROOT:-$REPO_ROOT/pinvou3-app/src-tauri/target/sensevoice-build/$target_arch}"
source_dir="$build_root/source"
build_dir="$build_root/build"
build_info="$output_path.build-info"
mkdir -p "$output_dir" "$build_root"

validate_runtime() {
  "$REPO_ROOT/scripts/check-linux-elf-policy.sh" "$output_path" "$policy_arch"
}

cached_runtime_matches() {
  local expected_sha actual_sha
  expected_sha="$(sed -n 's/^sha256=//p' "$build_info")"
  [ -n "$expected_sha" ] || return 1
  actual_sha="$(sha256sum "$output_path" | awk '{print $1}')"
  [ "$actual_sha" = "$expected_sha" ]
}

if [ -x "$output_path" ] && [ -f "$build_info" ] \
  && grep -Fxq "commit=$SENSEVOICE_SOURCE_COMMIT" "$build_info" \
  && grep -Fxq "architecture=$target_arch" "$build_info" \
  && cached_runtime_matches \
  && validate_runtime; then
  echo "[sensevoice] 复用 $target_arch 运行时: $output_path"
  exit 0
fi

# Pre-check the validation dependencies as well, so a host that cannot run
# check-linux-elf-policy.sh fails here instead of after a full compile.
# Native builds need the host g++; cross builds get their compilers from the
# environment instead, and the host `strip` cannot handle foreign-arch ELFs,
# so STRIP names the target strip (Debian cross binutils name by default).
if [ "$cross_mode" -eq 1 ]; then
  for command_name in cmake git make sha256sum dpkg file objdump readelf; do
    command -v "$command_name" >/dev/null || {
      echo "missing SenseVoice build dependency: $command_name" >&2
      exit 1
    }
  done
  # The env-driven toolchain is only mandatory when actually cross-building;
  # on a loongarch64 host the native compiler toolchain is enough.
  if [ "$requested_arch" != "$host_arch" ] \
    && [ -z "${CMAKE_TOOLCHAIN_FILE:-}" ] && [ -z "${CC:-}" ] && [ -z "${CXX:-}" ]; then
    echo "loongarch64 cross build requires CC/CXX (loongarch64 cross compilers) or CMAKE_TOOLCHAIN_FILE in the environment" >&2
    exit 1
  fi
  # A native loongarch64 host uses its own strip; a real cross build needs
  # the target's strip (host strip cannot handle foreign-arch ELFs).
  # Default spelling matches the pinned cross toolchain's binutils
  # (loongarch64-unknown-linux-gnu-*); the Debian cross-binutils spelling
  # keeps working via STRIP override.
  if [ "$requested_arch" = "$host_arch" ]; then
    strip_bin="${STRIP:-strip}"
  else
    strip_bin="${STRIP:-loongarch64-unknown-linux-gnu-strip}"
  fi
else
  for command_name in cmake g++ git make sha256sum dpkg file objdump readelf; do
    command -v "$command_name" >/dev/null || {
      echo "missing SenseVoice build dependency: $command_name" >&2
      exit 1
    }
  done
  strip_bin="strip"
fi
command -v "$strip_bin" >/dev/null || {
  echo "missing SenseVoice build dependency: $strip_bin" >&2
  exit 1
}

source_tmp="$build_root/source.tmp.$$"
rm -rf -- "$source_tmp"
trap 'rm -rf -- "${source_tmp:-}" "${output_tmp:-}"' EXIT
git init --quiet "$source_tmp"
git -C "$source_tmp" remote add origin "$SENSEVOICE_SOURCE_URL"
git -C "$source_tmp" fetch --quiet --depth 1 origin "$SENSEVOICE_SOURCE_COMMIT"
git -C "$source_tmp" checkout --quiet --detach FETCH_HEAD
# Verify the shallow checkout is exactly the pinned commit (the same
# tripwire as setup-sensevoice.sh), so a broken refspec or env value cannot
# silently swap the release source.
test "$(git -C "$source_tmp" rev-parse HEAD)" = "$SENSEVOICE_SOURCE_COMMIT" \
  || { echo "pinned commit checkout mismatch: expected $SENSEVOICE_SOURCE_COMMIT, got $(git -C "$source_tmp" rev-parse HEAD)" >&2; exit 1; }
git -C "$source_tmp" submodule update --init --recursive --depth 1
rm -rf -- "$source_dir" "$build_dir"
mv "$source_tmp" "$source_dir"

# With GGML_NATIVE=OFF the pinned ggml still defaults AVX2/FMA to ON, so the
# x86_64 engine carries a Haswell+ (AVX2) floor: pre-Haswell CPUs would
# SIGILL at inference time. Accepted as the release baseline.
#
# loongarch64: the pinned ggml (SenseVoice.cpp submodule, b2a092a7) supports
# loongarch64 natively — ggml_get_system_arch derives GGML_SYSTEM_ARCH from
# CMAKE_SYSTEM_PROCESSOR matching "loongarch64", which adds
# ggml-cpu/arch/loongarch/quants.c, -march=loongarch64, and -mlasx/-mlsx when
# GGML_LASX/GGML_LSX are on (both default ON; LoongArch64 new-world hardware
# is 3A5000+ and implements LSX/LASX). A plain CC/CXX cross configure leaves
# CMAKE_SYSTEM_PROCESSOR at the build host's value, which would silently drop
# the loongarch backend, so the cross mode passes it (plus
# CMAKE_SYSTEM_NAME=Linux) explicitly; a caller-provided toolchain file is
# trusted to set both itself. There is no separate GGML_LOONGARCH64-style
# toggle to disable.
cmake_args=(
  -S "$source_dir" -B "$build_dir"
  -DCMAKE_BUILD_TYPE=Release
  -DBUILD_SHARED_LIBS=OFF
  -DSENSE_VOICE_BUILD_EXAMPLES=OFF
  -DSENSE_VOICE_BUILD_TESTS=OFF
  -DSENSE_VOICE_CCACHE=OFF
  -DGGML_NATIVE=OFF
  -DGGML_CUDA=OFF
  -DGGML_VULKAN=OFF
  -DGGML_KOMPUTE=OFF
)
if [ "$cross_mode" -eq 1 ]; then
  # The engine is the deb's only C++ artifact: link libstdc++/libgcc
  # statically so it carries no GLIBCXX_/CXXABI_ version requirements and the
  # jammy GLIBCXX_/CXXABI_ floors hold for every architecture.
  cmake_args+=(-DCMAKE_EXE_LINKER_FLAGS="-static-libstdc++ -static-libgcc")
  if [ -n "${CMAKE_TOOLCHAIN_FILE:-}" ]; then
    cmake_args+=(-DCMAKE_TOOLCHAIN_FILE="$CMAKE_TOOLCHAIN_FILE")
  else
    cmake_args+=(
      -DCMAKE_SYSTEM_NAME=Linux
      -DCMAKE_SYSTEM_PROCESSOR="$requested_arch"
    )
  fi
fi
cmake "${cmake_args[@]}"
cmake --build "$build_dir" --target sense-voice-main \
  --parallel "${PINVOU3_SENSEVOICE_BUILD_JOBS:-4}"

output_tmp="$output_path.tmp.$$"
cp "$build_dir/bin/sense-voice-main" "$output_tmp"
"$strip_bin" --strip-unneeded "$output_tmp"
chmod 0755 "$output_tmp"
mv "$output_tmp" "$output_path"
validate_runtime

sha256="$(sha256sum "$output_path" | awk '{print $1}')"
printf 'source=%s\ncommit=%s\narchitecture=%s\nsha256=%s\n' \
  "$SENSEVOICE_SOURCE_URL" "$SENSEVOICE_SOURCE_COMMIT" "$target_arch" "$sha256" \
  > "$build_info"
echo "[sensevoice] 已构建 $target_arch 运行时: $output_path ($sha256)"
