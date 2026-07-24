#!/usr/bin/env bash
# Download the exact, reviewed Linux ARM64 connector binaries used by Pinvou.
# The binaries are intentionally excluded from Git and are materialized only
# for Linux ARM64 development, verification, and release builds.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
destination="$repo_root/pinvou3-app/src-tauri/resources/platforms/linux/aarch64/bundle/connectors/linux-arm64/bin"
cache_root="${XDG_CACHE_HOME:-${HOME}/.cache}/pinvou/connectors"
check_only=false

if [[ "${1:-}" == "--check" ]]; then
  check_only=true
elif [[ -n "${1:-}" ]]; then
  echo "usage: $0 [--check]" >&2
  exit 2
fi

dws_url="https://github.com/DingTalk-Real-AI/dingtalk-workspace-cli/releases/download/v1.0.51/dws-linux-arm64.tar.gz"
dws_archive_sha256="1e1a6e3b08adc009950acaa7b1b0a8a3bd327aff7110d1ba6f632a5e76fdfb62"
dws_binary_sha256="db012e54393ae0d1b78d74d0606e084823ab8e5540991deb6d31e68abd01883b"

lark_url="https://github.com/larksuite/cli/releases/download/v1.0.65/lark-cli-1.0.65-linux-arm64.tar.gz"
lark_archive_sha256="f3f11a2e163b2ea9698ae4c5f923a4fbca28274f44cd0a4689bf7588f229242e"
lark_binary_sha256="a71890afb27405cca77fd7a238ecb5da482bfc5a5c713be718e40ba3d72caf04"

wecom_url="https://registry.npmjs.org/@wecom/cli-linux-arm64/-/cli-linux-arm64-0.1.9.tgz"
wecom_archive_sha256="8d41fa973daca2a55b376ecd8849744a681a4a486090c312bf83ff79137f11a0"
wecom_binary_sha256="5e510f7a7c58ea9c7b62bdbe5d07496a15dfad9010ae579d07057797cfc8d3f4"

verify_file() {
  local path="$1"
  local expected="$2"
  printf '%s  %s\n' "$expected" "$path" | sha256sum --check --status
}

verify_installed() {
  verify_file "$destination/dws" "$dws_binary_sha256"
  verify_file "$destination/lark-cli" "$lark_binary_sha256"
  verify_file "$destination/wecom-cli" "$wecom_binary_sha256"
}

if "$check_only"; then
  verify_installed
  echo "Linux ARM64 connector binaries match connectors.lock.json"
  exit 0
fi

for command_name in curl tar sha256sum install mktemp; do
  command -v "$command_name" >/dev/null || {
    echo "missing required command: $command_name" >&2
    exit 1
  }
done

mkdir -p "$cache_root" "$destination"
work_dir="$(mktemp -d)"
trap 'rm -rf -- "$work_dir"' EXIT

fetch_archive() {
  local url="$1"
  local archive="$2"
  local expected="$3"
  if ! verify_file "$archive" "$expected" >/dev/null 2>&1; then
    curl --fail --location --retry 4 --retry-all-errors --output "$archive.part" "$url"
    verify_file "$archive.part" "$expected"
    mv "$archive.part" "$archive"
  fi
}

dws_archive="$cache_root/dws-linux-arm64-v1.0.51.tar.gz"
lark_archive="$cache_root/lark-cli-1.0.65-linux-arm64.tar.gz"
wecom_archive="$cache_root/wecom-cli-linux-arm64-0.1.9.tgz"

fetch_archive "$dws_url" "$dws_archive" "$dws_archive_sha256"
fetch_archive "$lark_url" "$lark_archive" "$lark_archive_sha256"
fetch_archive "$wecom_url" "$wecom_archive" "$wecom_archive_sha256"

mkdir "$work_dir/dws" "$work_dir/lark" "$work_dir/wecom"
tar xzf "$dws_archive" -C "$work_dir/dws"
tar xzf "$lark_archive" -C "$work_dir/lark"
tar xzf "$wecom_archive" -C "$work_dir/wecom"

verify_file "$work_dir/dws/dws" "$dws_binary_sha256"
verify_file "$work_dir/lark/lark-cli" "$lark_binary_sha256"
verify_file "$work_dir/wecom/package/bin/wecom-cli" "$wecom_binary_sha256"

install -m 0755 "$work_dir/dws/dws" "$destination/dws"
install -m 0755 "$work_dir/lark/lark-cli" "$destination/lark-cli"
install -m 0755 "$work_dir/wecom/package/bin/wecom-cli" "$destination/wecom-cli"
verify_installed

echo "Prepared pinned Linux ARM64 connector binaries in $destination"
