#!/usr/bin/env bash
# Download the exact, reviewed connector CLI binaries used by Pinvou for one
# platform. Each platform's connectors.lock.json is the pinned manifest; this
# script mirrors it so builds work without a JSON parser.
# The binaries are intentionally excluded from Git and are materialized only
# for development, verification, and release builds.
#
# usage: fetch-connectors.sh [--platform <id>] [--check]
#   <id>: linux-arm64 | linux-x64 | darwin-arm64 | darwin-x64 | windows-x64
#   不传 --platform 时按宿主平台探测(Windows 宿主无 uname 映射,必须显式传)。
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
cache_root="${XDG_CACHE_HOME:-${HOME}/.cache}/pinvou/connectors"
check_only=false
platform=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --platform)
      platform="${2:?--platform 需要参数}"
      shift 2
      ;;
    --check)
      check_only=true
      shift
      ;;
    *)
      echo "usage: $0 [--platform <id>] [--check]" >&2
      exit 2
      ;;
  esac
done

if [[ -z "$platform" ]]; then
  case "$(uname -s)-$(uname -m)" in
    Linux-aarch64 | Linux-arm64) platform="linux-arm64" ;;
    Linux-x86_64) platform="linux-x64" ;;
    Darwin-arm64) platform="darwin-arm64" ;;
    Darwin-x86_64) platform="darwin-x64" ;;
    *)
      echo "无法从宿主探测连接器平台($(uname -s)-$(uname -m)),请显式传 --platform" >&2
      exit 2
      ;;
  esac
fi

# 各平台钉住的厂家 release(与 connectors.lock.json 一一对应;改动必须两边同步)。
case "$platform" in
  linux-arm64)
    platform_resources="linux/aarch64"
    suffix=""
    dws_url="https://github.com/DingTalk-Real-AI/dingtalk-workspace-cli/releases/download/v1.0.51/dws-linux-arm64.tar.gz"
    dws_archive_sha256="1e1a6e3b08adc009950acaa7b1b0a8a3bd327aff7110d1ba6f632a5e76fdfb62"
    dws_binary_sha256="db012e54393ae0d1b78d74d0606e084823ab8e5540991deb6d31e68abd01883b"
    lark_url="https://github.com/larksuite/cli/releases/download/v1.0.65/lark-cli-1.0.65-linux-arm64.tar.gz"
    lark_archive_sha256="f3f11a2e163b2ea9698ae4c5f923a4fbca28274f44cd0a4689bf7588f229242e"
    lark_binary_sha256="a71890afb27405cca77fd7a238ecb5da482bfc5a5c713be718e40ba3d72caf04"
    wecom_url="https://registry.npmjs.org/@wecom/cli-linux-arm64/-/cli-linux-arm64-0.1.9.tgz"
    wecom_archive_sha256="8d41fa973daca2a55b376ecd8849744a681a4a486090c312bf83ff79137f11a0"
    wecom_binary_sha256="5e510f7a7c58ea9c7b62bdbe5d07496a15dfad9010ae579d07057797cfc8d3f4"
    ;;
  linux-x64)
    platform_resources="linux/x86_64"
    suffix=""
    dws_url="https://github.com/DingTalk-Real-AI/dingtalk-workspace-cli/releases/download/v1.0.51/dws-linux-amd64.tar.gz"
    dws_archive_sha256="d7b87fe7b9f7ae48467b776dfc08c72fa5fe6a760ca22484ca14efef5eb3df9a"
    dws_binary_sha256="cf046cb659353c88d2de4829ed65b7d83e9e7c6b42007100ec16d29fe924baaf"
    lark_url="https://github.com/larksuite/cli/releases/download/v1.0.65/lark-cli-1.0.65-linux-amd64.tar.gz"
    lark_archive_sha256="2d8fbd33e79d06efcd7243971d3a4e1a049ad91d04f0ca97214c6730e10c24c8"
    lark_binary_sha256="cd17c09ef6333b521824b9b87bb1e7d78aa020b22fd95ad93d487a9492d1926f"
    wecom_url="https://registry.npmjs.org/@wecom/cli-linux-x64/-/cli-linux-x64-0.1.9.tgz"
    wecom_archive_sha256="c8bfe1d3211b1387e8d9eb1ce6a873551bfe62b6dd6e2706532a4df8934e60b1"
    wecom_binary_sha256="bdd7323e8a4ac9ae36f14cdf663943572c8e2ce4ab5a57c09ba22d2b540d8cd1"
    ;;
  darwin-arm64)
    platform_resources="macos/aarch64"
    suffix=""
    dws_url="https://github.com/DingTalk-Real-AI/dingtalk-workspace-cli/releases/download/v1.0.51/dws-darwin-arm64.tar.gz"
    dws_archive_sha256="025bbb440b9abc099402e8c679a18ff279296aa4d530e43721731f570da0f63d"
    dws_binary_sha256="98b9b2b143f01e85676ceb974ee74afcadeac9dd45244e7ecc5b78422bf611e2"
    lark_url="https://github.com/larksuite/cli/releases/download/v1.0.65/lark-cli-1.0.65-darwin-arm64.tar.gz"
    lark_archive_sha256="9135e0412cf6bcb0ce6e6de3308ba878f6f16a887af46c806bdaa17d7d86e768"
    lark_binary_sha256="0f0cbcc843bb8cf0eff7e9af9bdf96e1c9d8e1f8163513f2eca43b419ea78647"
    wecom_url="https://registry.npmjs.org/@wecom/cli-darwin-arm64/-/cli-darwin-arm64-0.1.9.tgz"
    wecom_archive_sha256="050d251cf0bb55591569af467f9d90292edd5538431b9c5572d55cbc71aa2f33"
    wecom_binary_sha256="560b385bb568ff706ae5919f76aaa044b87c8d9ca4c6f3be1a36f0ec1090fe29"
    ;;
  darwin-x64)
    platform_resources="macos/x86_64"
    suffix=""
    dws_url="https://github.com/DingTalk-Real-AI/dingtalk-workspace-cli/releases/download/v1.0.51/dws-darwin-amd64.tar.gz"
    dws_archive_sha256="5d4a07db95f87ec749a88b7e4598f204fab96102fb50dd31a41fa70c961bf409"
    dws_binary_sha256="4bf25e624ab6c07bc1396e43485a2b73632917a264674b5dfa13510c0edcac31"
    lark_url="https://github.com/larksuite/cli/releases/download/v1.0.65/lark-cli-1.0.65-darwin-amd64.tar.gz"
    lark_archive_sha256="7d8a4539ade2b1bda46936ceae2a73e42a414e444a75b9e2e0f39294b8e61b07"
    lark_binary_sha256="4c112506118b8a5f349a038c7655aa4380eb511569e7e7f54eb908358e81a2fe"
    wecom_url="https://registry.npmjs.org/@wecom/cli-darwin-x64/-/cli-darwin-x64-0.1.9.tgz"
    wecom_archive_sha256="031bab15f91ae19b4e741de1918bd5f166a71397424b2ec9b8d2965e62692b57"
    wecom_binary_sha256="f1e0624c6191b2505b28c0596f9fb53873067273316273aba31bfbb13d8ea72f"
    ;;
  windows-x64)
    platform_resources="windows/x86_64"
    suffix=".exe"
    dws_url="https://github.com/DingTalk-Real-AI/dingtalk-workspace-cli/releases/download/v1.0.51/dws-windows-amd64.zip"
    dws_archive_sha256="85446092b155488f59bfb874a53da5720327d47cbe9ed934b2ce37b498212a4e"
    dws_binary_sha256="cdab71518a3107ebcf1430d704dfd063b104285a4b5f4402dd8eb5c0e6c09797"
    lark_url="https://github.com/larksuite/cli/releases/download/v1.0.65/lark-cli-1.0.65-windows-amd64.zip"
    lark_archive_sha256="6175f8a45fa0039467e785397745665f46a02f6260d36c6cf46f67b597f157d8"
    lark_binary_sha256="2cf0ed5ebd76600dfdc79559ca7b0572771bfe794400df2afa26e6d26398e137"
    wecom_url="https://registry.npmjs.org/@wecom/cli-win32-x64/-/cli-win32-x64-0.1.9.tgz"
    wecom_archive_sha256="9803e8deab1e5ad6877fc21679a07c562fda0d3b389c4839dfdaf0ea49ef549c"
    wecom_binary_sha256="ae74ff825cba1aa198d6e9d8fb2e967c4f67fa38bd7f4ba8b5a34551abb5ed92"
    ;;
  *)
    echo "未知连接器平台: $platform(可选: linux-arm64 linux-x64 darwin-arm64 darwin-x64 windows-x64)" >&2
    exit 2
    ;;
esac

destination="$repo_root/pinvou3-app/src-tauri/resources/platforms/$platform_resources/bundle/connectors/$platform/bin"

# 各平台的 sha256 工具输出均为 `<hash>  <path>`(GNU coreutils / macOS /sbin /
# perl shasum),但 --check 支持不一(macOS Darwin 版没有),统一自算自比。
compute_sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

verify_file() {
  local path="$1"
  local expected="$2"
  [[ -f "$path" ]] || return 1
  [[ "$(compute_sha256 "$path")" == "$expected" ]]
}

verify_installed() {
  verify_file "$destination/dws$suffix" "$dws_binary_sha256"
  verify_file "$destination/lark-cli$suffix" "$lark_binary_sha256"
  verify_file "$destination/wecom-cli$suffix" "$wecom_binary_sha256"
}

if "$check_only"; then
  verify_installed
  echo "$platform connector binaries match connectors.lock.json"
  exit 0
fi

for command_name in curl tar install mktemp; do
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

extract_archive() {
  local archive="$1"
  local dest="$2"
  mkdir -p "$dest"
  case "$archive" in
    *.zip)
      # Git Bash(Windows CI)不一定有 unzip;bsdtar(macOS/Git Bash 的 tar)
      # 能解 zip,GNU tar 不行,故 unzip 优先、tar 兜底。
      if command -v unzip >/dev/null 2>&1; then
        unzip -q -o "$archive" -d "$dest"
      else
        tar -xf "$archive" -C "$dest"
      fi
      ;;
    *)
      tar xzf "$archive" -C "$dest"
      ;;
  esac
}

dws_ext="tar.gz"
lark_ext="tar.gz"
if [[ "$dws_url" == *.zip ]]; then dws_ext="zip"; fi
if [[ "$lark_url" == *.zip ]]; then lark_ext="zip"; fi

dws_archive="$cache_root/dws-$platform-v1.0.51.$dws_ext"
lark_archive="$cache_root/lark-cli-1.0.65-$platform.$lark_ext"
wecom_archive="$cache_root/wecom-cli-$platform-0.1.9.tgz"

fetch_archive "$dws_url" "$dws_archive" "$dws_archive_sha256"
fetch_archive "$lark_url" "$lark_archive" "$lark_archive_sha256"
fetch_archive "$wecom_url" "$wecom_archive" "$wecom_archive_sha256"

extract_archive "$dws_archive" "$work_dir/dws"
extract_archive "$lark_archive" "$work_dir/lark"
extract_archive "$wecom_archive" "$work_dir/wecom"

verify_file "$work_dir/dws/dws$suffix" "$dws_binary_sha256"
verify_file "$work_dir/lark/lark-cli$suffix" "$lark_binary_sha256"
verify_file "$work_dir/wecom/package/bin/wecom-cli$suffix" "$wecom_binary_sha256"

install -m 0755 "$work_dir/dws/dws$suffix" "$destination/dws$suffix"
install -m 0755 "$work_dir/lark/lark-cli$suffix" "$destination/lark-cli$suffix"
install -m 0755 "$work_dir/wecom/package/bin/wecom-cli$suffix" "$destination/wecom-cli$suffix"
verify_installed

echo "Prepared pinned $platform connector binaries in $destination"
