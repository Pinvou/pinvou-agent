//! 契约:编译器栈覆盖不得泄漏到运行时进程。
//!
//! 背景:macOS 构建 SIGBUS 的规避通过 `RUSTC_WRAPPER` 环境变量注入
//! `scripts/rustc-stack-wrapper`(正式 Cargo 入口按平台注入,见 run-dev.sh
//! 与 CI job env),把 `RUST_MIN_STACK=16MiB` 只注入到编译期 rustc 进程。
//! cargo test / cargo run 启动的目标进程不经过 wrapper,不得继承该变量,
//! 默认线程栈语义(约 2 MiB)不变。
//!
//! 若本测试失败,说明有人在运行时环境里设置了 `RUST_MIN_STACK`
//! (或改回了 `.cargo/config.toml [env]` 注入方案),需要修回 wrapper
//! 作用域——`[env]` 会把变量传给 cargo run / cargo test 的目标进程,
//! 造成开发态与发布产物的线程栈语义漂移。

#[test]
fn runtime_process_does_not_inherit_compiler_stack_override() {
    assert!(
        std::env::var_os("RUST_MIN_STACK").is_none(),
        "RUST_MIN_STACK 必须只作用于编译期 rustc 进程;运行时进程继承了该变量, \
         说明作用域泄漏(曾用 .cargo/config.toml [env] 注入,已被 rustc-wrapper 替代)"
    );
}
