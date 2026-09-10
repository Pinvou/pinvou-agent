#!/usr/bin/env bash
# Validate the target architecture and the Ubuntu 22.04 dynamic-symbol
# floors of every ELF under a file or directory:
#   GLIBC_   <= 2.35   (jammy glibc 2.35; overridable via the third argument)
#   GLIBCXX_ <= 3.4.30 (jammy libstdc++, gcc 12)
#   CXXABI_  <= 1.3.13 (jammy libstdc++, gcc 12)
# Release binaries link the build host's glibc, so a runner upgrade silently
# raises the baseline; any ELF above the floor fails here in CI instead of
# crashing on launch for 22.04 users. Runs on the release runner (see the
# glibc floor guard steps in release-packages.yml) and at the end of
# build-sensevoice-runtime.sh. Depends on dpkg/file/find/objdump/readelf,
# all preinstalled on ubuntu runners; meaningful only on Linux, use bash -n
# for a local syntax check.
set -euo pipefail

usage() {
  echo "usage: $0 <file-or-directory> <amd64|arm64> [glibc-floor]" >&2
  exit 2
}

scan_root="${1:-}"
expected_arch="${2:-}"
glibc_floor="${3:-2.35}"
glibcxx_floor="3.4.30"
cxxabi_floor="1.3.13"

[ -n "$scan_root" ] && [ -e "$scan_root" ] || usage
case "$expected_arch" in
  amd64) expected_machine="Advanced Micro Devices X86-64" ;;
  arm64) expected_machine="AArch64" ;;
  *) usage ;;
esac

for command_name in dpkg file find objdump readelf; do
  command -v "$command_name" >/dev/null || {
    echo "missing required command: $command_name" >&2
    exit 1
  }
done

display_path() {
  local elf="$1"
  if [ -d "$scan_root" ]; then
    printf '/%s\n' "${elf#"$scan_root"/}"
  else
    printf '%s\n' "$elf"
  fi
}

dump_dynamic_symbols() {
  local elf="$1" out
  out="$(LC_ALL=C objdump -T "$elf" 2>&1)" || {
    echo "FAIL: $(display_path "$elf") objdump 无法解析(文件损坏或 binutils 不支持?):" >&2
    printf '%s\n' "$out" >&2
    return 1
  }
  printf '%s\n' "$out"
}

max_symbol_version() {
  local dynsyms="$1" prefix="$2"
  printf '%s\n' "$dynsyms" | grep -o "${prefix}[0-9][0-9.]*" | sort -Vu | tail -n 1 || true
}

check_prefix() {
  local dynsyms="$1" elf="$2" prefix="$3" floor="$4" highest version shown
  highest="$(max_symbol_version "$dynsyms" "$prefix")"
  [ -n "$highest" ] || return 0
  version="${highest#"$prefix"}"
  shown="$(display_path "$elf")"
  if dpkg --compare-versions "$version" gt "$floor"; then
    echo "FAIL: $shown requires $highest > baseline $prefix$floor" >&2
    return 1
  fi
  echo "ok: $shown $highest ≤ $prefix$floor"
}

check_architecture() {
  local elf="$1" machine shown
  # LC_ALL=C: readelf localizes header field names (e.g. under zh_CN the
  # "Machine:" header is translated), which would break parsing below.
  machine="$(LC_ALL=C readelf -h "$elf" | sed -n 's/^[[:space:]]*Machine:[[:space:]]*//p')"
  shown="$(display_path "$elf")"
  if [ "$machine" != "$expected_machine" ]; then
    echo "FAIL: $shown architecture $machine != expected $expected_machine ($expected_arch)" >&2
    return 1
  fi
  echo "ok: $shown architecture $expected_arch"
}

elf_count=0
failed=0
while IFS= read -r -d '' elf; do
  file_out="$(file -b "$elf")"
  case "$file_out" in
    ELF*) ;;
    *) continue ;;
  esac
  elf_count=$((elf_count + 1))
  ok=0
  case "$file_out" in
    *executable*)
      # A lost exec bit would otherwise surface late as "Permission denied"
      # when the engine or host binary is launched.
      [ -x "$elf" ] || {
        echo "FAIL: $(display_path "$elf") is an ELF executable without the execute bit" >&2
        ok=1
      }
      ;;
  esac
  check_architecture "$elf" || ok=1
  dynsyms="$(dump_dynamic_symbols "$elf")" || { failed=1; continue; }
  check_prefix "$dynsyms" "$elf" 'GLIBC_' "$glibc_floor" || ok=1
  check_prefix "$dynsyms" "$elf" 'GLIBCXX_' "$glibcxx_floor" || ok=1
  check_prefix "$dynsyms" "$elf" 'CXXABI_' "$cxxabi_floor" || ok=1
  [ "$ok" -eq 0 ] || failed=1
done < <(
  if [ -d "$scan_root" ]; then
    find "$scan_root" -type f -print0
  else
    printf '%s\0' "$scan_root"
  fi
)

[ "$elf_count" -gt 0 ] || {
  echo "FAIL: 未发现任何 ELF: $scan_root" >&2
  exit 1
}
[ "$failed" -eq 0 ] || {
  echo "FAIL: 存在架构错误或超出 Ubuntu 22.04 基线的 ELF(见上)" >&2
  exit 1
}
echo "PASS: $elf_count 个 ELF 为 $expected_arch，且满足 GLIBC_ ≤ $glibc_floor / GLIBCXX_ ≤ $glibcxx_floor / CXXABI_ ≤ $cxxabi_floor"
