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
#   - Windows (MINGW*/MSYS*/CYGWIN*):透传不注入。Windows 无 SIGBUS
#     触发(仅 macOS 实测);且无扩展名 Unix shell 文件无法被 Windows
#     原生 Cargo 执行(CreateProcess → os error 193)。
#
# 本脚本是"平台选择"的单一真相源:run-dev.sh 与 CI smoke
# (rustc-wrapper-smoke.yml)都执行它,保证正式入口与实际验证一致。
# 输出空时调用方不得设置 RUSTC_WRAPPER。
#
# 注意:本脚本不做路径转换(cygpath 等)。Windows 分支刻意输出空,
# 避免 MSYS 风格路径(C:\ vs /c/)与空格转义在 CreateProcess 下出错。
case "$(uname -s)" in
  Darwin|Linux)
    # 与本脚本同目录的 sh wrapper(绝对路径,不依赖调用方 cwd)
    echo "$(cd "$(dirname "$0")" && pwd)/rustc-stack-wrapper"
    ;;
  MINGW*|MSYS*|CYGWIN*)
    # Windows:透传,不注入
    ;;
  *)
    # 未知平台:透传,不注入
    ;;
esac
