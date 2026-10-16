#!/usr/bin/env bash
# Validate the target architecture and the dynamic-symbol floors of every ELF
# under a file or directory. The GLIBC_ floor is per arch (see the floor map
# below); GLIBCXX_/CXXABI_ keep the Ubuntu 22.04 (jammy) values everywhere:
#   GLIBC_   <= 2.35   amd64/arm64: jammy glibc 2.35 (overridable via the
#                      third argument)
#   GLIBC_   <= 2.38   loongarch64: deepin 23/25 and UOS V25 both ship
#                      glibc 2.38; that is the deb's baseline
#   GLIBCXX_ <= 3.4.30 (jammy libstdc++, gcc 12)
#   CXXABI_  <= 1.3.13 (jammy libstdc++, gcc 12)
# Release binaries link the build host's glibc, so a runner upgrade silently
# raises the baseline; any ELF above the floor fails here in CI instead of
# crashing on launch for 22.04 users. loongarch64 keeps the jammy
# GLIBCXX_/CXXABI_ floors: its dynamically-linked C++ artifacts are the
# unofficial Node bridge build and — statically linked — the ASR engine
# (-static-libstdc++ -static-libgcc, so no GLIBCXX_/CXXABI_ requirements at
# all); the unofficial Node build is checked against these same floors like
# every other ELF (the main app binary is Rust).
# Runs on the release runner (see the
# glibc floor guard steps in release-packages.yml) and at the end of
# build-sensevoice-runtime.sh. Depends on dpkg/file/find/objdump/readelf,
# all preinstalled on ubuntu runners; meaningful only on Linux, use bash -n
# for a local syntax check.
set -euo pipefail

usage() {
  echo "usage: $0 <file-or-directory> <amd64|arm64|loongarch64> [glibc-floor]" >&2
  exit 2
}

scan_root="${1:-}"
expected_arch="${2:-}"
glibc_floor_arg="${3:-}"
glibcxx_floor="3.4.30"
cxxabi_floor="1.3.13"

[ -n "$scan_root" ] && [ -e "$scan_root" ] || usage
# readelf Machine: string per arch (binutils maps EM_LOONGARCH to
# "LoongArch", not "LoongArch64" — identical on jammy's binutils 2.38 and
# newer). The glibc floor is part of the same map so a new arch cannot be
# added without choosing its baseline explicitly.
case "$expected_arch" in
  amd64)       expected_machine="Advanced Micro Devices X86-64"; default_glibc_floor="2.35"; baseline_label="Ubuntu 22.04" ;;
  arm64)       expected_machine="AArch64";                       default_glibc_floor="2.35"; baseline_label="Ubuntu 22.04" ;;
  loongarch64) expected_machine="LoongArch";                     default_glibc_floor="2.38"; baseline_label="deepin 23/25 / UOS V25 (glibc 2.38)" ;;
  *) usage ;;
esac
glibc_floor="${glibc_floor_arg:-$default_glibc_floor}"

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
  echo "FAIL: 存在架构错误或超出 ${baseline_label} 基线的 ELF(见上)" >&2
  exit 1
}
echo "PASS: $elf_count 个 ELF 为 $expected_arch，且满足 GLIBC_ ≤ $glibc_floor / GLIBCXX_ ≤ $glibcxx_floor / CXXABI_ ≤ $cxxabi_floor"
