#!/usr/bin/env bash
# CI/reviewer helper: materialize one platform's first-use connector artifacts
# directly from connectors.lock.json and verify archive + executable hashes.
# Release builds never call this script and never embed these binaries.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
cache_root="${XDG_CACHE_HOME:-${HOME}/.cache}/pinvou/connectors"
check_only=false
platform=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --platform) platform="${2:?--platform 需要参数}"; shift 2 ;;
    --check) check_only=true; shift ;;
    *) echo "usage: $0 [--platform <id>] [--check]" >&2; exit 2 ;;
  esac
done

if [[ -z "$platform" ]]; then
  case "$(uname -s)-$(uname -m)" in
    Linux-aarch64 | Linux-arm64) platform="linux-arm64" ;;
    Linux-x86_64) platform="linux-x64" ;;
    Darwin-arm64) platform="darwin-arm64" ;;
    Darwin-x86_64) platform="darwin-x64" ;;
    *) echo "无法探测连接器平台，请显式传 --platform" >&2; exit 2 ;;
  esac
fi

case "$platform" in
  linux-arm64) platform_resources="linux/aarch64"; suffix="" ;;
  linux-x64) platform_resources="linux/x86_64"; suffix="" ;;
  darwin-arm64) platform_resources="macos/aarch64"; suffix="" ;;
  darwin-x64) platform_resources="macos/x86_64"; suffix="" ;;
  windows-x64) platform_resources="windows/x86_64"; suffix=".exe" ;;
  *) echo "未知连接器平台: $platform" >&2; exit 2 ;;
esac

lock="$repo_root/pinvou3-app/src-tauri/resources/platforms/$platform_resources/bundle/connectors/connectors.lock.json"
destination="$(dirname "$lock")/$platform/bin"

for command_name in node curl tar install mktemp; do
  command -v "$command_name" >/dev/null || {
    echo "missing required command: $command_name" >&2
    exit 1
  }
done

compute_sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

verify_file() {
  [[ -f "$1" ]] && [[ "$(compute_sha256 "$1")" == "$2" ]]
}

names=()
versions=()
urls=()
archive_hashes=()
binary_hashes=()
while IFS=$'\t' read -r name version url archive_hash binary_hash; do
  names+=("$name")
  versions+=("$version")
  urls+=("$url")
  archive_hashes+=("$archive_hash")
  binary_hashes+=("$binary_hash")
done < <(node -e '
  const fs = require("node:fs");
  const lock = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (lock.schemaVersion !== 1 || lock.platform !== process.argv[2]) process.exit(3);
  for (const item of lock.artifacts) {
    console.log([item.name, item.version, item.url, item.archiveSha256, item.binarySha256].join("\t"));
  }
' "$lock" "$platform")

[[ "${#names[@]}" -eq 3 ]] || { echo "连接器 lock 必须恰含 3 个 artifact" >&2; exit 1; }

if "$check_only"; then
  for index in "${!names[@]}"; do
    verify_file "$destination/${names[$index]}$suffix" "${binary_hashes[$index]}"
  done
  echo "$platform connector binaries match connectors.lock.json"
  exit 0
fi

mkdir -p "$cache_root" "$destination"
work_dir="$(mktemp -d)"
trap 'rm -rf -- "$work_dir"' EXIT

fetch_archive() {
  local url="$1" archive="$2" expected="$3"
  if ! verify_file "$archive" "$expected" >/dev/null 2>&1; then
    curl --fail --location --retry 4 --retry-all-errors --output "$archive.part" "$url"
    verify_file "$archive.part" "$expected"
    mv "$archive.part" "$archive"
  fi
}

extract_archive() {
  local archive="$1" dest="$2"
  mkdir -p "$dest"
  if [[ "$archive" == *.zip ]]; then
    if command -v unzip >/dev/null 2>&1; then
      unzip -q -o "$archive" -d "$dest"
    else
      tar -xf "$archive" -C "$dest"
    fi
  else
    tar xzf "$archive" -C "$dest"
  fi
}

for index in "${!names[@]}"; do
  name="${names[$index]}"
  version="${versions[$index]}"
  url="${urls[$index]}"
  ext="tar.gz"
  [[ "$url" == *.zip ]] && ext="zip"
  archive="$cache_root/$name-$platform-$version.$ext"
  unpacked="$work_dir/$name"
  fetch_archive "$url" "$archive" "${archive_hashes[$index]}"
  extract_archive "$archive" "$unpacked"
  case "$name" in
    dws) source_binary="$unpacked/dws$suffix" ;;
    lark-cli) source_binary="$unpacked/lark-cli$suffix" ;;
    wecom-cli) source_binary="$unpacked/package/bin/wecom-cli$suffix" ;;
    *) echo "未知连接器 artifact: $name" >&2; exit 1 ;;
  esac
  verify_file "$source_binary" "${binary_hashes[$index]}"
  install -m 0755 "$source_binary" "$destination/$name$suffix"
done

"$0" --platform "$platform" --check
echo "Verified pinned $platform connector binaries in $destination"
