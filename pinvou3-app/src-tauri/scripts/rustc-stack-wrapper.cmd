@echo off
rem rustc-stack-wrapper(Windows 版):仅为编译期 rustc 进程注入更大的默认
rem 线程栈,规避 rustc/LLVM 编译 codewhale-tui 时的栈溢出(MachineLateinstrs
rem Cleanup 递归,rustc 1.96/1.97 稳定复现;std 线程默认栈三端均为 2 MiB)。
rem 接入方式:CI 的 Windows job 通过 RUSTC_WRAPPER 环境变量注入本文件
rem (见 pr-check.yml windows-rust-test / rustc-wrapper-smoke.yml);
rem Windows 本地无 SIGBUS 触发(仅 macOS 实测),默认透传即可。
rem cargo run / cargo test 目标进程不经过本文件,运行时线程栈语义不变。
rem Unix 请用无扩展名版本 scripts/rustc-stack-wrapper(sh,带 shebang)。
rem 本文件全部行以 @ 开头,cmd 全程无回显,rustc stdout 纯净。
if not defined RUST_MIN_STACK set RUST_MIN_STACK=16777216
%*
