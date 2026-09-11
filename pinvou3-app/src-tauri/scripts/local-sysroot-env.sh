#!/bin/sh
# local-sysroot-env.sh — 为本机(无 sudo)的 src-tauri cargo 构建配置本地 GTK sysroot 环境。
#
# 用法(source,不要直接执行):
#   . pinvou3-app/src-tauri/scripts/local-sysroot-env.sh
#   cd pinvou3-app/src-tauri && cargo check --lib
#   cargo test --lib projects -- --test-threads=4
#
# 适用场景:Ubuntu 24.04 (noble) arm64、无 sudo、系统未装 GTK/WebKit -dev 包。
# 系统库全部从 apt 镜像(ports.ubuntu.com)下载 .deb 后解包到
# pinvou3-app/src-tauri/.gtk-sysroot/root(本地目录,不入库),不触碰系统目录。
#
# ============================================================================
# 重建步骤(sysroot 被删除后,在无 sudo 机器上重做;约 470 MB)
# ============================================================================
#   SYSROOT=pinvou3-app/src-tauri/.gtk-sysroot
#   mkdir -p "$SYSROOT/debs/partial" "$SYSROOT/root"
#   cp /var/lib/dpkg/status /tmp/apt-status   # 让 apt 把系统已装包视为已满足
#   apt-get -y --download-only --no-install-recommends \
#     -o Dir::State::status=/tmp/apt-status \
#     -o Dir::Cache::archives="$PWD/$SYSROOT/debs" -o Debug::NoLocking=1 \
#     install libglib2.0-dev libgtk-3-dev libwebkit2gtk-4.1-dev \
#             libjavascriptcoregtk-4.1-dev libsoup-3.0-dev \
#             libayatana-appindicator3-dev libx11-dev libssl-dev
#   # 系统已装、apt 因此跳过的 dev 包,需手工补下(zlib/libffi 等在 .pc
#   # Requires.private 闭包内):
#   (cd "$SYSROOT/debs" && apt-get download zlib1g-dev libffi-dev libexpat1-dev \
#     libzstd-dev libmount-dev libselinux1-dev libpcre2-dev libsepol-dev \
#     libblkid-dev shared-mime-info)
#   # 运行时包(dev 包的 .so 符号链接目标):扫 sysroot 内断裂链接,
#   # 用 dpkg -S /usr/lib/aarch64-linux-gnu/<target> 找系统对应包名,
#   # apt-get download 全部;2026-09-11 实测共 85 个运行时包。
#   for d in "$SYSROOT"/debs/*.deb; do dpkg-deb -x "$d" "$SYSROOT/root"; done
#   # 校验:.pc 无需修改(PKG_CONFIG_SYSROOT_DIR 会自动给 -I/-L 加 sysroot
#   # 前缀);残余断裂链接仅限 usr/share/doc 下,不影响构建。
#
# ============================================================================
# 变量说明
# ============================================================================
# PKG_CONFIG_SYSROOT_DIR  让 pkg-config 把 .pc 输出的 -I/-L 重定位进 sysroot,
#                         .pc 文件本身保持 deb 原样、零修改。
# PKG_CONFIG_LIBDIR       限定 .pc 搜索范围只在 sysroot 内(不含系统默认路径),
#                         避免系统半套 dev 包与 sysroot 混用。
# PKG_CONFIG_PATH         追加 /tmp/dbus-pkgconfig(dbus-1.pc 本机先例,指向
#                         /tmp/dbus-dev 解包的头文件 + 系统运行库;CodeWhale
#                         子模组测试同样依赖它,勿破坏该目录)。
# RUSTC_WRAPPER           经 rustc-stack-wrapper-select.sh(单一真相源)注入
#                         scripts/rustc-stack-wrapper:仅为编译期 rustc 进程
#                         注入 RUST_MIN_STACK=16MiB(规避 codewhale-tui 依赖
#                         编译时的 rustc/LLVM 栈溢出);cargo run/test 目标进程
#                         不经 wrapper,运行时线程栈语义不变。
# RUST_MIN_STACK          不在此导出:编译期由 wrapper 注入 16 MiB;若测试
#                         进程自身需要大栈(如 CodeWhale tui 全量),由调用方
#                         显式设置(如 RUST_MIN_STACK=33554432)。
# LD_LIBRARY_PATH         前缀加入 sysroot 运行库目录,供 cargo test 的测试
#                         二进制优先命中 sysroot 内版本;系统已装同名运行库
#                         (GNOME 桌面),不设也能跑,设了与链接期所见一致。
# CFLAGS                  前缀加入 sysroot 的 multiarch include 目录
#                         (usr/include/aarch64-linux-gnu):opensslconf.h/ffi.h 等
#                         头文件在 Debian/Ubuntu 打包中位于该目录,而 cc 的默认
#                         搜索路径指向真实系统目录(无 sudo 装不了 dev 包,那里
#                         是空的),pkg-config 的 .pc 又不覆盖它——不显式补上
#                         openssl-sys 等 build script 的头文件探测会失败。
# ============================================================================

# 由脚本位置推导 src-tauri 根与 sysroot 根(不依赖调用方 cwd;兼容 sh/bash source)
_local_sysroot_script_dir=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE:-$0}")" && pwd)
_local_sysroot_src_tauri=$(dirname -- "$_local_sysroot_script_dir")
_local_sysroot_root="$_local_sysroot_src_tauri/.gtk-sysroot/root"

if [ ! -d "$_local_sysroot_root/usr/lib/aarch64-linux-gnu/pkgconfig" ]; then
  echo "local-sysroot-env: sysroot 不存在: $_local_sysroot_root" >&2
  echo "请按本文件头注释的重建步骤重建 .gtk-sysroot" >&2
  return 1 2>/dev/null || exit 1
fi

export PKG_CONFIG_SYSROOT_DIR="$_local_sysroot_root"
export PKG_CONFIG_LIBDIR="$_local_sysroot_root/usr/lib/aarch64-linux-gnu/pkgconfig:$_local_sysroot_root/usr/share/pkgconfig"
# dbus 先例:/tmp/dbus-pkgconfig 存在时并入搜索路径(不存在不报错)
if [ -d /tmp/dbus-pkgconfig ]; then
  export PKG_CONFIG_PATH="/tmp/dbus-pkgconfig"
fi

# 编译期 rustc 大栈:平台选择唯一真相源 = rustc-stack-wrapper-select.sh
_local_sysroot_wrapper=$("$_local_sysroot_script_dir/rustc-stack-wrapper-select.sh")
if [ -n "$_local_sysroot_wrapper" ]; then
  export RUSTC_WRAPPER="$_local_sysroot_wrapper"
fi

# 测试/运行期动态库:优先 sysroot 内运行库(与链接期所见一致)
case ":${LD_LIBRARY_PATH:-}:" in
  *:"$_local_sysroot_root/usr/lib/aarch64-linux-gnu":*) ;;
  *) export LD_LIBRARY_PATH="$_local_sysroot_root/usr/lib/aarch64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" ;;
esac

# sysroot 的 multiarch include(opensslconf.h/ffi.h 所在)不在 cc 默认搜索路径
case " ${CFLAGS:-} " in
  *" -I$_local_sysroot_root/usr/include/aarch64-linux-gnu "*) ;;
  *) export CFLAGS="-I$_local_sysroot_root/usr/include/aarch64-linux-gnu${CFLAGS:+ $CFLAGS}" ;;
esac

echo "local-sysroot-env: sysroot=$_local_sysroot_root"
echo "local-sysroot-env: RUSTC_WRAPPER=${RUSTC_WRAPPER:-<未注入>}"

unset _local_sysroot_script_dir _local_sysroot_src_tauri _local_sysroot_root _local_sysroot_wrapper
