#!/bin/sh
# 输出当前平台应注入的 rustc-stack-wrapper 路径;无需注入时输出空。
#
# 背景:macOS 构建 SIGBUS 的规避通过 RUSTC_WRAPPER 注入
# scripts/rustc-stack-wrapper(带 shebang 的 sh,Unix 可执行),只在编译期
# rustc 进程注入 RUST_MIN_STACK=16MiB。RUSTC_WRAPPER 是环境变量,
# 不能在 .cargo/config.toml 里按平台条件化(全局键,Windows 上指向
# 无扩展名 sh 会 os error 193,阻断所有 Cargo 命令),因此由正式
# Cargo 入口按平台决定是否注入:
#   - Darwin/Linux:注入 sh 版(编译 codewhale-tui 有 SIGBUS 实测风险);
#   - Windows (MINGW*/MSYS*/CYGWIN*):编译并注入 .exe 版。栈溢出根因三端
#     同源(Windows 无 SIGBUS 信号、表现为栈溢出,已由 windows-rust-test
#     实测),本地 dev 同样需要 16 MiB 栈;.cmd 经 cmd /C 受 8191 字符
#     命令行上限限制(大型 crate 的 rustc 命令行超限),故 Windows 用
#     .exe 版(经 CreateProcess 直启,上限 32767 字符)。
#
# 本脚本是"平台选择"的单一真相源:run-dev.sh 与 CI smoke
# (rustc-wrapper-smoke.yml)都执行它,保证正式入口与实际验证一致。
# 输出空时调用方不得设置 RUSTC_WRAPPER。
#
# 注意:Windows 分支用 cygpath -m 把 MSYS 风格路径(/c/...)转成 Windows
# 原生路径(C:/...)。原生 cargo.exe 的 CreateProcess 无法解析 /c/...,
# MSYS 对 env 变量的自动路径转换也不可靠,显式转换更稳妥。用 -m(正斜杠)
# 而非 -w(反斜杠):反斜杠经 shell echo 二次解析可能被吃掉(\a/\r/\c)。
case "$(uname -s)" in
  Darwin|Linux)
    # 与本脚本同目录的 sh wrapper(绝对路径,不依赖调用方 cwd)
    echo "$(cd "$(dirname "$0")" && pwd)/rustc-stack-wrapper"
    ;;
  MINGW*|MSYS*|CYGWIN*)
    # 与本脚本同目录的 .exe wrapper:先幂等编译(直接调 rustc、不经 cargo,
    # 无自举依赖),再输出 Windows 原生路径;存在且源码未更新时不重编。
    dir="$(cd "$(dirname "$0")" && pwd)"
    src="$dir/rustc-stack-wrapper.rs"
    exe="$dir/rustc-stack-wrapper.exe"
    if [ ! -f "$exe" ] || [ "$src" -nt "$exe" ]; then
      # 显式 cygpath 转 Windows 原生路径,避免 MSYS 对 /c/... 的自动转换
      # 在 rustc 参数位不可靠。
      # 编译失败即报错终止:Windows 栈溢出已实证,静默退回"不注入"会把 wrapper
      # 构建失败重新表现为难诊断的 rustc 栈溢出。
      if ! rustc -O "$(cygpath -m "$src")" -o "$(cygpath -m "$exe")"; then
        echo "rustc-stack-wrapper-select: 编译 .exe wrapper 失败,无法注入 16 MiB 栈;请检查 rustc 工具链与 wrapper 源码" >&2
        exit 1
      fi
    fi
    cygpath -m "$exe"
    ;;
  *)
    # 未知平台:透传,不注入
    ;;
esac
