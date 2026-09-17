//! Project layer: sessions' **logical grouping** is decoupled from their
//! **physical execution**.
//!
//! - The logical layer (this module) only stores human judgment: project
//!   definitions (id/name/roots/position) and the session-assignment map.
//!   Organizing operations (move assignment / delete project) write only
//!   this layer.
//! - The physical layer (`features/sessions`, `features/codex_acp`) stores
//!   machine facts: a session's working-directory binding is immutable after
//!   creation and is never touched from here.
//! - Dependencies flow one way: this module depends on no other feature
//!   (`app → features → platform/core`); cross-store composition and session
//!   existence checks live in the command layer.
//! - Deleting a project never deletes sessions: all members (explicitly
//!   assigned + auto-grouped) are written as explicit move-outs, stay in
//!   Ungrouped, and do not revive with the folder's next auto-materialization;
//!   sessions created later in that folder have no assignment entry and
//!   auto-group as usual (equivalent to Codex `threads.project_id ...
//!   ON DELETE SET NULL` plus a "move-out entry suppresses revival" semantic).
//!
//! The deterministic order of group resolution is implemented by the
//! frontend; this module only supplies the data:
//! ① explicit assignment (assignments hit; null = explicit move-out, blocks
//!   auto-grouping revival)
//! ② session workspace falls under a project root → auto-group
//! ③ implicit folder grouping / temporary sessions sink to the bottom
//!   (existing fallback)

mod store;
#[cfg(test)]
mod tests;

pub use store::{
    DeleteProjectReport, EnsureFolderOutcome, MoveSessionOutcome, Project, ProjectStore,
    SessionAssignments, removed_roots,
};
