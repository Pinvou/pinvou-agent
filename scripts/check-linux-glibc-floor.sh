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

# 每个 ELF 的动态符号表只解析一次。objdump 自身失败(异架构 ELF/文件损坏/
# 未来 binutils 不识别的新特性)必须硬失败——静默当成「无符号引用」会把该
# ELF 的三个前缀全部放行,只有 grep 无匹配才是合法的「无引用」。
dump_dynamic_symbols() {
  local elf="$1" out
  out="$(objdump -T "$elf" 2>&1)" || {
    echo "FAIL: objdump 无法解析 ${elf#"$extract_dir"}(异架构或损坏?):" >&2
    printf '%s\n' "$out" >&2
    return 1
  }
  printf '%s\n' "$out"
}

# 从动态符号表文本提取某前缀的最大符号版本(如 GLIBC_2.38);无引用输出空。
max_symbol_version() {
  local dynsyms="$1" prefix="$2"
  printf '%s\n' "$dynsyms" | grep -o "${prefix}[0-9][0-9.]*" | sort -Vu | tail -n 1 || true
}

check_prefix() {
  local dynsyms="$1" elf="$2" prefix="$3" floor="$4" highest version
  highest="$(max_symbol_version "$dynsyms" "$prefix")"
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
  dynsyms="$(dump_dynamic_symbols "$elf")" || { failed=1; continue; }
  ok=0
  check_prefix "$dynsyms" "$elf" 'GLIBC_' "$glibc_floor" || ok=1
  check_prefix "$dynsyms" "$elf" 'GLIBCXX_' "$glibcxx_floor" || ok=1
  check_prefix "$dynsyms" "$elf" 'CXXABI_' "$cxxabi_floor" || ok=1
  [ "$ok" -eq 0 ] || failed=1
done < <(find "$extract_dir" -type f -print0)

# deb 至少含主二进制、codex-bridge 的 node、knowledge-host 的
# pinvou-knowledge-server;一个 ELF 都没有说明解包或打包异常。
[ "$elf_count" -gt 0 ] || {
  echo "FAIL: deb 内未发现任何 ELF(解包或打包异常): $deb_path" >&2
  exit 1
}
[ "$failed" -eq 0 ] || {
  echo "FAIL: 存在超出 Ubuntu 22.04 基线的符号版本引用(见上)" >&2
  exit 1
}
echo "PASS: $elf_count 个 ELF 满足 GLIBC_ ≤ $glibc_floor / GLIBCXX_ ≤ $glibcxx_floor / CXXABI_ ≤ $cxxabi_floor(基线 Ubuntu 22.04)"
