#!/usr/bin/env bash
# 校验发布 deb 内全部 ELF 的动态符号版本不超过 Ubuntu 22.04 基线:
#   GLIBC_   ≤ 2.35   (jammy glibc 2.35)
#   GLIBCXX_ ≤ 3.4.30 (jammy libstdc++,gcc 12)
#   CXXABI_  ≤ 1.3.13 (同上)
# 在 release runner 的 deb 产物上运行(见 release-packages.yml 的「glibc 下限守护」
# 步骤);任一 ELF 超线即非零退出,把「22.04 用户启动即崩」提前挡在 CI,防 runner
# 被误升级或依赖引入新符号时静默抬高基线。依赖 dpkg-deb/dpkg/file/objdump,
# 均为 ubuntu runner 自带;脚本本身只在 Linux 上有意义,本地语法检查用 bash -n。
set -euo pipefail

usage() {
  echo "usage: $0 <deb> [glibc-floor]" >&2
  exit 2
}

deb_path="${1:-}"
[ -n "$deb_path" ] && [ -f "$deb_path" ] || usage
glibc_floor="${2:-2.35}"
glibcxx_floor="3.4.30"
cxxabi_floor="1.3.13"

for command_name in dpkg-deb dpkg file objdump; do
  command -v "$command_name" >/dev/null || {
    echo "missing required command: $command_name" >&2
    exit 1
  }
done

extract_dir="$(mktemp -d)"
trap 'rm -rf -- "$extract_dir"' EXIT
dpkg-deb -x "$deb_path" "$extract_dir"

# 从 objdump -T 动态符号表提取某前缀的最大符号版本(如 GLIBC_2.38);无引用输出空。
max_symbol_version() {
  local elf="$1" prefix="$2"
  objdump -T "$elf" 2>/dev/null | grep -o "${prefix}[0-9][0-9.]*" | sort -Vu | tail -n 1 || true
}

check_prefix() {
  local elf="$1" prefix="$2" floor="$3" highest version
  highest="$(max_symbol_version "$elf" "$prefix")"
  [ -n "$highest" ] || return 0
  version="${highest#"$prefix"}"
  if dpkg --compare-versions "$version" gt "$floor"; then
    echo "FAIL: ${elf#"$extract_dir"} requires $highest > baseline $prefix$floor" >&2
    return 1
  fi
  echo "ok: ${elf#"$extract_dir"} $highest ≤ $prefix$floor"
  return 0
}

elf_count=0
failed=0
while IFS= read -r -d '' elf; do
  file -b "$elf" | grep -q '^ELF' || continue
  elf_count=$((elf_count + 1))
  ok=0
  check_prefix "$elf" 'GLIBC_' "$glibc_floor" || ok=1
  check_prefix "$elf" 'GLIBCXX_' "$glibcxx_floor" || ok=1
  check_prefix "$elf" 'CXXABI_' "$cxxabi_floor" || ok=1
  [ "$ok" -eq 0 ] || failed=1
done < <(find "$extract_dir" -type f -print0)

# deb 至少含主二进制与 codex-bridge 的 node;一个 ELF 都没有说明解包或打包异常。
[ "$elf_count" -gt 0 ] || {
  echo "FAIL: deb 内未发现任何 ELF(解包或打包异常): $deb_path" >&2
  exit 1
}
[ "$failed" -eq 0 ] || {
  echo "FAIL: 存在超出 Ubuntu 22.04 基线的符号版本引用(见上)" >&2
  exit 1
}
echo "PASS: $elf_count 个 ELF 满足 GLIBC_ ≤ $glibc_floor / GLIBCXX_ ≤ $glibcxx_floor / CXXABI_ ≤ $cxxabi_floor(基线 Ubuntu 22.04)"
