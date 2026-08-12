@echo off
rem Windows 透传 shim:cargo 按 PATHEXT 把无扩展名 wrapper 解析到本文件。
rem macOS 构建 SIGBUS 是 Darwin 特有,Windows 无需注入 RUST_MIN_STACK,
rem 原样转发 rustc 调用,构建行为不变。
%*
