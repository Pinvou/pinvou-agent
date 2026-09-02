//! rustc-stack-wrapper 的 Windows 可执行版(CI 编译为 .exe)。
//!
//! 背景:Windows 上 RUSTC_WRAPPER 指向 .cmd 时,Cargo 经 cmd /C 执行,cmd.exe
//! 命令行上限 8191 字符;windows-sys/windows 等大型 crate 的 rustc 命令行
//! (数千个 --cfg feature=...)会超限,报 "The command line is too long"。
//! 截断发生在 cmd /C 入口,.cmd 无法修复;本 .exe 经 CreateProcess 直启
//! (上限 32767 字符),只在编译期 rustc 进程注入 RUST_MIN_STACK=16MiB。
//!
//! 语义与 scripts/rustc-stack-wrapper(sh)一致:
//!   - 未显式设置 RUST_MIN_STACK 时注入 16777216;
//!   - 设置 RUSTC_WRAPPER_CHAIN 时把整条命令行(含 rustc 首参)转交给链上命令
//!     (如 sccache),形成 cargo → 本 wrapper → sccache → rustc;否则直接转发
//!     首参(编译器)。
//!   - 只作用于编译期 rustc;cargo run / cargo test 目标进程不经过本 wrapper,
//!     运行时默认线程栈语义(约 2 MiB)不变。

use std::env;
use std::ffi::OsString;
use std::process::Command;

fn main() {
    let mut args: Vec<OsString> = env::args_os().skip(1).collect();
    if args.is_empty() {
        eprintln!("rustc-stack-wrapper: missing compiler command");
        std::process::exit(2);
    }

    // 链式 wrapper(如 sccache):设置 RUSTC_WRAPPER_CHAIN 时,首参仍是编译器,
    // 整条命令行转交给链上命令;否则首参即编译器程序本身。
    let chain = env::var_os("RUSTC_WRAPPER_CHAIN");
    let has_chain = chain.as_ref().is_some_and(|c| !c.is_empty());

    let program: OsString;
    let rest: Vec<OsString>;
    if has_chain {
        program = chain.unwrap();
        rest = args;
    } else {
        program = args.remove(0);
        rest = args;
    }

    let mut cmd = Command::new(&program);
    cmd.args(&rest);
    // 仅在未显式设置时注入 16 MiB(与 sh 版一致)。
    if env::var_os("RUST_MIN_STACK").is_none() {
        cmd.env("RUST_MIN_STACK", "16777216");
    }

    match cmd.status() {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => {
            eprintln!(
                "rustc-stack-wrapper: failed to run {}: {e}",
                program.to_string_lossy()
            );
            std::process::exit(2);
        }
    }
}
