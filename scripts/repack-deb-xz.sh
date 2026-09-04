#!/usr/bin/env bash
# 把 tauri-bundler 产出的 deb 重打包为最大压缩。
#
# 背景:tauri v2 bundler 硬编码 data.tar 用 gzip level 6(tauri-cli 的
# bundle/linux/debian.rs,Compression::default();tauri.conf 无压缩配置项,
# schema 里仅 nsis.compression 与 rpm.compression 两个旋钮)。deb 包体大头是
# data.tar 里的主二进制 + node + knowledge-server 三个 ELF,gzip-6 → xz -9
# 通常再省 20-30%。
#
# 做法:dpkg-deb -R 解包后用 --root-owner-group -Zxz -z9 原地重建。内容零改动,
# 只换 data.tar 的压缩容器,因此 md5sums、维护者脚本、包内 ELF 与 glibc 符号
# 版本完全不变;--root-owner-group 保证非 root 构建机(如 GitHub runner)产出
# 的文件归 root:root;control.tar 与 data.tar 同为 xz(dpkg 默认 uniform
# compression)。
#
# 仅 Linux 有意义(依赖 dpkg-deb);release-packages.yml 的两个 deb job 与
# scripts/release-deb.sh 共用。本脚本之后跑的「glibc 下限守护」用 dpkg-deb -x
# 解包,原生支持 xz。
# 本地静态检查:bash -n scripts/repack-deb-xz.sh。
set -euo pipefail

usage() {
  echo "usage: $0 <deb>" >&2
  exit 2
}

deb="${1:-}"
[ -n "$deb" ] && [ -f "$deb" ] || usage
command -v dpkg-deb >/dev/null 2>&1 || {
  echo "missing required command: dpkg-deb (Linux only)" >&2
  exit 1
}

before=$(stat -c%s "$deb")
work="$(mktemp -d)"
trap 'rm -rf -- "$work"' EXIT

dpkg-deb -R "$deb" "$work/pkg"
dpkg-deb --root-owner-group -Zxz -z9 --build "$work/pkg" "$deb"

# 重打包自检:control 可解析、包名字段非空,否则视为产物损坏直接失败。
[ -n "$(dpkg-deb -f "$deb" Package)" ] || {
  echo "FAIL: repacked deb has empty Package field: $deb" >&2
  exit 1
}

after=$(stat -c%s "$deb")
echo "repack-deb-xz: $(numfmt --to=iec-i --suffix=B "$before") -> $(numfmt --to=iec-i --suffix=B "$after") ($deb)"
