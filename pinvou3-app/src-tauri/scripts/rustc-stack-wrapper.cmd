@echo off
rem rustc-stack-wrapper(Windows 版):仅为编译期 rustc 进程注入更大的默认
rem 线程栈,规避 rustc/LLVM 编译 codewhale-tui 时的栈溢出(MachineLateinstrs
rem Cleanup 递归,rustc 1.96/1.97 稳定复现;std 线程默认栈三端均为 2 MiB)。
rem 接入方式:Windows 本地 dev 经 run-dev.sh → selector 注入本文件;CI 的
rem Windows job(发布 build-windows-x64 与 windows-rust-test)通过
rem RUSTC_WRAPPER 环境变量注入本文件;
rem 平台选择统一走 rustc-stack-wrapper-select.sh(输出空 = 不注入):
rem 栈溢出根因三端同源(macOS 实测 SIGBUS、Windows 实测栈溢出),Windows
rem 本地 dev 经 selector 注入本文件(无扩展名 sh 无法被 Windows 原生
rem Cargo 执行 → os error 193,故用 .cmd 版经 cmd /C 执行)。
rem cargo run / cargo test 目标进程不经过本文件,运行时线程栈语义不变。
rem Unix 请用无扩展名版本 scripts/rustc-stack-wrapper(sh,带 shebang)。
rem 本文件全部行以 @ 开头,cmd 全程无回显,rustc stdout 纯净。
if not defined RUST_MIN_STACK set RUST_MIN_STACK=16777216
rem 链式 wrapper(如 sccache):设置了 RUSTC_WRAPPER_CHAIN 时,在本文件之后
rem 再调用它,形成 cargo → 本文件 → RUSTC_WRAPPER_CHAIN → rustc。这样既能
rem 保留 sccache 缓存,又把 RUST_MIN_STACK 只注入编译期 rustc 进程,
rem 不泄漏到 cargo run / cargo test 目标。
if not "%RUSTC_WRAPPER_CHAIN%"=="" (
  %RUSTC_WRAPPER_CHAIN% %*
) else (
  %*
)
