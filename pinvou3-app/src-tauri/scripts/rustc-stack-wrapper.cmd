@:; export RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}"; exec "$@"
@if not defined RUST_MIN_STACK set RUST_MIN_STACK=16777216
@%*
rem ===== 以下注释对 sh 不可达(exec 已替换进程),对 cmd 仅为注释 =====
rem rustc-stack-wrapper:三端统一为编译期 rustc 进程注入更大的默认线程栈,
rem 规避 rustc/LLVM 编译 codewhale-tui 时的栈溢出(MachineLateinstrsCleanup
rem 递归,rustc 1.96/1.97 稳定复现;std 线程默认栈三端均为 2 MiB,
rem rust-lang/rust #160535 已把默认值提升到 16 MiB,此处提前注入同值)。
rem 通过 .cargo/config.toml [build] rustc-wrapper 接入,只包装编译期 rustc
rem 调用;cargo run / cargo test 目标进程不经过本文件,运行时线程栈语义不变。
rem 本文件为 sh/cmd 双分支 polyglot,无 shebang(避免 cmd 在 echo on 阶段
rem 把 shebang 行回显进 stdout 污染 rustc 输出):
rem - Unix:execvp 对无 shebang 文本 ENOEXEC,按 POSIX 由 /bin/sh 执行第 1 行
rem   sh 分支(@: 占位非命令,产生一条 stderr 噪音但无碍);export 后 exec 转发
rem - Windows:Rust std 对 .cmd 程序名走 cmd /C,每行 @ 前缀消除回显,
rem   保证 rustc stdout 纯净(cargo 解析 --print= 输出不被打断)
