//! Tauri 命令实现。前端通过 `invoke(name, args)` 调到这里。
//!
//! 本模块按域拆分为数十个子模块、注册约四百条命令，逐条清单不在这里维护：
//! 权威命令面以 `lib.rs` 的 `generate_handler!` 为准，`protocol_tests.rs`
//! 把该清单钉为本模块的协议契约，新增命令时两边同步。

pub(crate) mod acp_providers;
pub(crate) mod artifacts;
pub(crate) mod assistant_response;
pub(crate) mod attachments;
pub(crate) mod browser;
pub(crate) mod builtin;
pub(crate) mod chat;
pub(crate) mod checkpoints;
pub(crate) mod codex;
pub(crate) mod computer_use;
pub(crate) mod connectors;
pub(crate) mod dependencies;
pub(crate) mod diagnostics;
pub(crate) mod files;
pub(crate) mod interaction;
pub(crate) mod knowledge;
pub(crate) mod marketplace;
pub(crate) mod memory;
pub(crate) mod monitor;
pub(crate) mod multiagent;
pub(crate) mod personas;
pub(crate) mod pet;
mod prelude;
pub(crate) mod projects;
pub(crate) mod remote_control;
pub(crate) mod remote_knowledge;
pub(crate) mod runtime;
pub(crate) mod scheduled;
pub(crate) mod sessions;
pub(crate) mod settings;
pub(crate) mod shared_knowledge_host;
pub(crate) mod startup;
pub(crate) mod timeline;
pub(crate) mod updater;
pub(crate) mod voice;

#[cfg(test)]
mod protocol_tests;
#[cfg(test)]
mod tests;
