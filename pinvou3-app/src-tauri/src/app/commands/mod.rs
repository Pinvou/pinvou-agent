//! Tauri 命令实现。前端通过 `invoke(name, args)` 调到这里。
//!
//! This module is split into dozens of domain submodules registering ~400
//! commands; the itemized list is not maintained here. The authoritative
//! command surface is `lib.rs`'s `generate_handler!`, and `protocol_tests.rs`
//! pins each domain module's source-side command list as its protocol
//! contract — keep the two sides in sync when adding commands.

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
pub(crate) mod global_shortcut;
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
