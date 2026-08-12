#!/bin/sh
:; export RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}"; exec "$@"
@echo off
if not defined RUST_MIN_STACK set RUST_MIN_STACK=16777216
%*
rem ===== 以下注释对 sh 与 cmd 均不可达,仅作说明 =====
rem rustc-stack-wrapper:三端统一为编译期 rustc 进程注入更大的默认线程栈,
rem 规避 rustc/LLVM 编译 codewhale-tui 时的栈溢出(MachineLateinstrsCleanup
rem 递归,rustc 1.96/1.97 稳定复现;std 线程默认栈三端均为 2 MiB,rust-lang/rust
rem #160535 已把默认值提升到 16 MiB,此处为当前 stable 提前注入同值)。
rem 通过 .cargo/config.toml [build] rustc-wrapper 接入,只包装编译期 rustc
rem 调用;cargo run / cargo test 目标进程不经过本文件,运行时线程栈语义不变。
rem 本文件是 sh/cmd 双分支 polyglot:Unix 经 shebang 走 sh 分支,Windows 经
rem Rust std 对 .cmd 程序名的 cmd /C 处理走 cmd 分支;同一配置三端可执行。
