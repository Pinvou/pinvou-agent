//! 进程环境变量写入的串行化锁（edition 2024 迁移引入）。
//!
//! ## 背景
//!
//! Rust 2024 起 `std::env::set_var` / `remove_var` 为 unsafe： POSIX 下写 env
//! 可能 realloc `environ`，与任何线程的并发读（含 `std::env::vars_os()` 迭代、
//! libc `getenv`）构成数据竞争（ UB / use-after-free），std 不再提供全局锁。
//!
//! 本 app 的两类并发方：
//!
//! 1. **写者**：运行时安装/卸载/迁移 MCP 工具时写 `PINVOU3_MCP_SECRET_*`
//!    （tokio `spawn_blocking` 线程），boot 阶段同步既有 secret 与会话产物目录。
//! 2. **读者**：底座 CodeWhale 在 spawn MCP/CLI 子进程前用 `std::env::vars_os()`
//!    快照父进程 env（`crates/tui/src/mcp/child_env.rs`），逐项注入子进程
//!    （不靠隐式继承）。
//!
//! ## 本锁的边界（诚实声明）
//!
//! - 消除的是 **pinvou3 自身写者之间** 的竞争，以及（待底座接线后）写者与
//!   底座 env 快照之间的竞争；读者侧快照函数在 CodeWhale 内，需底座暴露
//!   同一把锁才能真正关闭窗口——已按 fork 边界登记为上游议题。
//! - **残余风险**：WKWebView/WebKit2GTK 的 XPC 与 glib 线程可能调 libc
//!   `getenv` 与写并发（ POSIX 形式 UB，无法从 Rust 侧加锁）。运行时写的
//!   key 仅 `PINVOU3_MCP_SECRET_*`，无证据外部运行时会读取它们；作为
//!   已文档化的接受风险保留。
//!
//! ## 用法
//!
//! 所有运行时（多线程阶段）env 写点必须持本锁；`run()` 启动序列内、任何
//! 线程/运行时启动前的写点（`ensure_release_env`、`ui_cache`）已证明单线程，
//! 不需要（取锁只为统一也无害，但当前保持不加）。

use std::sync::{Mutex, MutexGuard};

/// 进程级 env 写锁。测试代码的 env 写另用
/// `platform::paths::tests::ENV_LOCK`（同样串行化全部测试写点）。
static ENV_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// 获取 env 写锁的 guard。持锁期间可安全执行 `set_var`/`remove_var`。
///
/// 短临界区约定：锁内只做 env 写本身，不做 IO / await（std Mutex guard
/// 不可跨 await；调用方都在同步上下文）。
pub(crate) fn lock() -> MutexGuard<'static, ()> {
    match ENV_WRITE_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}
