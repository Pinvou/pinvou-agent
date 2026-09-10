#!/usr/bin/env bash
# Build a shippable ASR engine (sense-voice-main) for the current Linux host
# from the pinned SenseVoice.cpp commit. Must run on Ubuntu 22.04 or older:
# the output links the build host's glibc, and the post-build validation
# rejects symbols above the 22.04 (glibc 2.35) baseline; release CI builds
# on ubuntu-22.04 runners.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=sensevoice-source.env
source "$SCRIPT_DIR/sensevoice-source.env"

usage() {
  echo "usage: $0 --output <path> --arch <x86_64|aarch64>" >&2
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
  x86_64|amd64) host_arch="x86_64"; policy_arch="amd64" ;;
  aarch64|arm64) host_arch="aarch64"; policy_arch="arm64" ;;
  *) echo "unsupported Linux build architecture: $(uname -m)" >&2; exit 1 ;;
esac
[ "$requested_arch" = "$host_arch" ] || {
  echo "requested SenseVoice architecture $requested_arch does not match build host $host_arch" >&2
  exit 1
}

output_path="$(realpath -m "$output_path")"
output_dir="$(dirname "$output_path")"
build_root="${PINVOU3_SENSEVOICE_BUILD_ROOT:-$REPO_ROOT/pinvou3-app/src-tauri/target/sensevoice-build/$host_arch}"
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
  && grep -Fxq "architecture=$host_arch" "$build_info" \
  && cached_runtime_matches \
  && validate_runtime; then
  echo "[sensevoice] 复用 $host_arch 运行时: $output_path"
  exit 0
fi

# Pre-check the validation dependencies as well, so a host that cannot run
# check-linux-elf-policy.sh fails here instead of after a full compile.
for command_name in cmake g++ git make sha256sum strip dpkg file objdump readelf; do
  command -v "$command_name" >/dev/null || {
    echo "missing SenseVoice build dependency: $command_name" >&2
    exit 1
  }
done

source_tmp="$build_root/source.tmp.$$"
rm -rf -- "$source_tmp"
trap 'rm -rf -- "${source_tmp:-}" "${output_tmp:-}"' EXIT
git init --quiet "$source_tmp"
git -C "$source_tmp" remote add origin "$SENSEVOICE_SOURCE_URL"
git -C "$source_tmp" fetch --quiet --depth 1 origin "$SENSEVOICE_SOURCE_COMMIT"
git -C "$source_tmp" checkout --quiet --detach FETCH_HEAD
git -C "$source_tmp" submodule update --init --recursive --depth 1
rm -rf -- "$source_dir" "$build_dir"
mv "$source_tmp" "$source_dir"

# With GGML_NATIVE=OFF the pinned ggml still defaults AVX2/FMA to ON, so the
# x86_64 engine carries a Haswell+ (AVX2) floor: pre-Haswell CPUs would
# SIGILL at inference time. Accepted as the release baseline.
cmake -S "$source_dir" -B "$build_dir" \
  -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_SHARED_LIBS=OFF \
  -DSENSE_VOICE_BUILD_EXAMPLES=OFF \
  -DSENSE_VOICE_BUILD_TESTS=OFF \
  -DSENSE_VOICE_CCACHE=OFF \
  -DGGML_NATIVE=OFF \
  -DGGML_CUDA=OFF \
  -DGGML_VULKAN=OFF \
  -DGGML_KOMPUTE=OFF
cmake --build "$build_dir" --target sense-voice-main \
  --parallel "${PINVOU3_SENSEVOICE_BUILD_JOBS:-4}"

output_tmp="$output_path.tmp.$$"
cp "$build_dir/bin/sense-voice-main" "$output_tmp"
strip --strip-unneeded "$output_tmp"
chmod 0755 "$output_tmp"
mv "$output_tmp" "$output_path"
validate_runtime

sha256="$(sha256sum "$output_path" | awk '{print $1}')"
printf 'source=%s\ncommit=%s\narchitecture=%s\nsha256=%s\n' \
  "$SENSEVOICE_SOURCE_URL" "$SENSEVOICE_SOURCE_COMMIT" "$host_arch" "$sha256" \
  > "$build_info"
echo "[sensevoice] 已构建 $host_arch 运行时: $output_path ($sha256)"
