//! Multi-conversation management wrapper (facade + submodules).
//!
//! Reuses the upstream deepseek-tui [`SessionManager`](deepseek_tui::session_manager::SessionManager) (which already supports `new(custom_dir)`),
//! directing the sessions directory to `~/.pinvou3/sessions/` (isolated from `~/.deepseek/`).
//!
//! Capabilities exposed to pinvou3-app Tauri commands:
//! - `list` — list all session metadata (frontend history panel)
//! - `create_new` — create a new empty session (before the first message is sent)
//! - `load` — read the full conversation (injected into the engine via `Op::SyncSession` on session switch)
//! - `save` — persist (auto-save after each turn completes)
//! - `delete` — delete session + artifacts directory
//! - `set_title` — rename
//! - `active_id` / `set_active` — track the current active session (chat command surface)
//!
//! **Arc + RwLock wrapping**: every field is an `Arc`, so the whole
//! `SessionStore` can be cheaply Cloned into Tauri State + shared across tasks.
//!
//! Historically this file was a 3700+ line god-module mixing 7 kinds of
//! responsibility. Wave 2 task 2d split it into a facade (this file, keeping
//! the struct definitions + constants) + submodules:
//!
//! - `store` — session storage CRUD / lifecycle / engine-state persistence entry
//! - `scheduled` — scheduled-run profile / engine-state types and registry
//! - `retention` — retention policy and the `persist_then_reconcile` helper family
//! - `transcript` — transcript revision / truncation protection
//! - `mode_state` — per-session mode state machine (mode/plan/persona/skill)
//! - `injections` — one-shot injections and the plan-claim transactional checkout guard
//! - `sidecars` — independent sidecar persistence for skill bindings / model / pin / collapse
//! - `rewind` — conversation truncation for code-mode rewind and the `_rewound_turns.json` backup
//! - `validators` — id / workspace / path validation and small helpers
//! - `workspace_bindings` —— per-session sidecar of plain chat sessions' user
//!   working-directory bindings (`workspace-binding.json` in the session
//!   directory)
//!
//! Submodules continue the methods via `impl SessionStore` (Rust allows impl
//! blocks of the same struct to be scattered across submodules) and read
//! `&self`'s private fields directly — struct fields are visible to descendant
//! modules. The pub surface is re-exported centrally in this file, keeping
//! external call paths unchanged.

pub(crate) mod diagnostics;
mod injections;
pub(crate) mod mode_state;
mod retention;
mod rewind;
mod scheduled;
mod sidecars;
mod store;
mod transcript;
pub(crate) mod validators;
mod workspace_bindings;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

pub use crate::core::mode_state::{ModeLane, SerializableMode};
use crate::platform::paths;
use crate::platform::prefs::{CodePermissionPrefs, ModeDefaultPrefs};
use parking_lot::{Mutex, RwLock};
#[cfg(feature = "benchmark-hooks")]
pub(crate) use retention::RetentionEvictionRecord;

/// Re-export the session-domain mode-state types so they are owned by the
/// sessions feature. These were historically re-exported through a `core`
/// shim (`core::mode_state`), but they are feature-domain aggregates
/// (persona/review/knowledge) and must not form a `core → features`
/// reverse dependency. Consumers should import from
/// `crate::features::sessions::{...}`.
pub use self::mode_state::{
    MountedCollection, MountedCollectionsSnapshot, MountedRemoteCollection, SessionModeState,
};
/// Re-export scheduled-run types so the historical
/// `crate::features::sessions::X` paths stay stable.
pub use self::scheduled::{
    ChatEngineState, ScheduledEngineState, ScheduledRunMode, ScheduledRunProfile,
    ScheduledTokenAccounting,
};
// Only the benchmark-gated headless runner (agentic_task) mints headless ids
// and warns about retention eviction, so both re-exports follow its gate.
// Retention itself reaches `store` directly and needs neither.
/// Re-export the headless session id prefix: the runner mints ids from it and
/// retention keys the separate headless eviction budget on it, so the two must
/// not drift into separate literals.
#[cfg(feature = "benchmark-hooks")]
pub(crate) use self::store::HEADLESS_SESSION_PREFIX;
/// Re-export the headless retention cap: it is the number the runner's
/// eviction warning quotes, and quoting the chat cap there would name a budget
/// headless runs no longer touch.
#[cfg(feature = "benchmark-hooks")]
pub(crate) use self::store::MAX_HEADLESS_SESSIONS;
/// Re-exported for the observer-filter test: the same sweep can also evict
/// from the chat budget, and the test must pin that such evictions are NOT
/// reported as headless ones. Gated to the same cfg as its only consumer
/// (the `agentic_task` tests module) — a plain `#[cfg(test)]` re-export is
/// unused, and therefore a `[lints]`-denied error, in every test build that
/// compiles without `benchmark-hooks` (plain `cargo test`, rust-lint).
#[cfg(all(test, feature = "benchmark-hooks"))]
pub(crate) use self::store::MAX_SESSIONS_PER_KIND;
/// Re-export the new-chat placeholder sentinel: the auto-rename trigger in
/// the command layer must compare against the same value GUI-created and kept
/// headless sessions carry, or a renamed constant silently breaks auto-rename.
pub(crate) use self::store::NEW_CHAT_TITLE;
/// Re-export transcript helpers (consumed across engine / remote-control).
pub use self::transcript::transcript_revision;
/// Re-export the crate-visible sched-id predicate: the send gates
/// (alias-defeating, round-34 minor 4) and the tests that pin the
/// predicate itself.
pub(crate) use self::validators::is_sched_session_id;
/// Re-export the crate-visible session-id validator (used by commands). It is
/// `pub(crate)` so it stays out of the crate's public API surface.
pub(crate) use self::validators::{
    aux_side_effect_exclusion, is_aux_session_id, validate_scheduled_task_id, validate_session_id,
    validate_user_workspace_path,
};
/// Re-export the rebind outcome (public rebind docs link into it; the module
/// itself stays private).
pub use self::workspace_bindings::RebindBindingsOutcome;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Chat,
    ScheduledRun,
}

/// pinvou3 session store: wraps SessionManager + active-id tracking +
/// per-session mode state.
///
/// Mode state lives on two layers (two-lane semantics, settled in review;
/// the design lane has been merged into work):
/// - per-session: any session's explicit mode switch (`set_mode`) persists to
///   `~/.pinvou3/sessions/_session_mode_states.json` (reopening a session
///   restores its own last mode); a switch inside an already-materialized
///   session **never** touches any global default.
/// - global defaults split per lane: work → settings.json
///   `mode_defaults.work` (a legacy `mode_defaults.design` value is folded
///   into the work mirror at startup),
///   code → `code_permission.last_mode` (never used code mode → Plan
///   read-only). Written only by an explicit **draft-state** switch in the
///   matching lane (`set_mode_default`); a new session's default
///   mode = its lane's global default (code resolved by
///   `resolved_default_mode`, work applied by the frontend at session
///   materialization). The one-shot yolo confirmation flag
///   `code_permission.yolo_confirmed` also lives in settings.json, written by
///   the confirmation command.
/// Other runtime interaction state (pending_plan, persona, knowledge-base
/// mounts, etc.) stays in-memory only.
///
/// `auto_continue_count`: M2 weak-model hardening — when an Executing-state
/// LLM stops after a single tool call, the bridge automatically sends a
/// "continue" message to drive the agent loop. Reset to 0 on every
/// user-initiated message.
#[derive(Clone)]
pub struct SessionStore {
    /// Underlying upstream session manager (durable JSON store).
    pub(crate) manager: Arc<deepseek_tui::session_manager::SessionManager>,
    /// Scheduled-run profiles keyed by session id (`sched-*`).
    pub(crate) scheduled_profiles: Arc<RwLock<HashMap<String, ScheduledRunProfile>>>,
    /// Path to the scheduled-profile registry JSON.
    pub(crate) scheduled_profiles_path: Arc<PathBuf>,
    /// Root for scheduled-task workspaces (`<root>/<task_id>/workspace`).
    pub(crate) scheduled_root: Arc<PathBuf>,
    /// Coarse mutation lock for scheduled create / delete / reconcile.
    pub(crate) scheduled_mutation: Arc<Mutex<()>>,
    /// Currently active session id (chat command surface).
    pub(crate) active: Arc<RwLock<Option<String>>>,
    /// Per-session runtime mode state (mode, plan, persona, ...).
    pub(crate) mode_states: Arc<RwLock<HashMap<String, SessionModeState>>>,
    /// Per-session model binding: session_id → SavedModel.id. An entry exists
    /// only for a session that has explicitly chosen a model; the rest fall
    /// back to the global active_model_id. Persisted to
    /// `_session_models.json`; the upstream SavedSession cannot gain fields, so
    /// this lives in an independent sidecar.
    pub(crate) session_models: Arc<RwLock<HashMap<String, String>>>,
    /// Pin table for conversation history: session_id -> pinned_at. Persisted
    /// independently to `_pinned_sessions.json` without changing the
    /// SavedSession structure.
    pub(crate) pinned_sessions: Arc<RwLock<HashMap<String, String>>>,
    /// Whether the boot-time load of `_pinned_sessions.json` actually produced
    /// the file's contents.
    ///
    /// `durable_pinned_sessions` falls back to the in-memory map when the file
    /// cannot be read, on the reasoning that the boot map can only be a subset
    /// and so can never widen the eviction set. That reasoning fails when the
    /// file was ALREADY corrupt at boot: the load is a no-op on failure, so the
    /// map is empty for the same reason the file is unusable, and the first
    /// sweep — which `SessionStore::boot` runs immediately afterwards — would
    /// exempt nothing and delete every pinned session. This flag lets the sweep
    /// tell "no pins" from "pins unknown" and refuse to evict in the latter.
    pub(crate) pinned_sessions_loaded: Arc<std::sync::atomic::AtomicBool>,
    /// Sessions collapsed from the left task list: session_id -> hidden_at.
    /// Persisted independently to `_hidden_sessions.json` without changing the
    /// SavedSession structure.
    pub(crate) hidden_sessions: Arc<RwLock<HashMap<String, String>>>,
    /// Session execution-root resolver, injected by the app composition root
    /// (lib.rs) once the AcpPool is ready — the production implementation covers
    /// both sources: codex_acp native code sessions' project bindings plus plain
    /// chat sessions' user working-directory bindings (sidecar, see the read-cache
    /// field below). None (not yet injected in tests / early startup) = no external
    /// binding, every session's execution root is the session-private directory;
    /// the or_else fallback in `store.rs` is only a defense for that case. The
    /// ledger root (attachments/audits/artifacts/remote grants) is unaffected and
    /// is always the session-private directory.
    pub(crate) execution_root_resolver: Arc<RwLock<Option<ExecutionRootResolver>>>,
    /// Read cache of plain chat sessions' user working-directory bindings:
    /// session_id → user-selected directory. The authoritative store is the
    /// per-session sidecar `workspace-binding.json` inside the session-private
    /// directory (see workspace_bindings.rs; SavedSession is unchanged); the cache
    /// is written on bind and backfilled from the sidecar on a read miss.
    /// `session_roots` falls back to this map when the resolver misses: on a hit,
    /// execution = bound directory and ledger = session-private directory (the
    /// same dual-root semantics as native code session bindings).
    pub(crate) session_workspaces: Arc<RwLock<HashMap<String, PathBuf>>>,
    /// Pinvou-native code-session predicate (ACP sessions are always plain; see the
    /// codex_acp store). Shares the same `SessionAgentStore` closure with the Engine
    /// bridge / remote side, injected by the app composition root (lib.rs);
    /// None = no code-session predicate (tests / early startup), and every session
    /// follows plain semantics.
    code_session_predicate: Arc<RwLock<Option<CodeSessionPredicate>>>,
    /// In-memory source of truth for `_session_mode_states.json`: every
    /// session's explicit mode (under two-lane semantics plain sessions
    /// persist too). Loaded and merged into `mode_states` at startup;
    /// maintained and flushed by set_mode / session deletion / retention
    /// policy cleanup.
    session_mode_states: Arc<RwLock<HashMap<String, SerializableMode>>>,
    /// In-process mirror of settings.json `code_permission`. `mode_state` is
    /// called on every turn along the chat send path; default resolution only
    /// reads this memory (read under lock, never touching disk); writes are
    /// persisted via `UserPrefs::update_transaction` and then synced into this
    /// mirror.
    code_permission: Arc<RwLock<CodePermissionPrefs>>,
    /// In-process mirror of settings.json `mode_defaults` (the work lane's
    /// global default), mirroring `code_permission` semantics. At startup the
    /// legacy `mode_defaults.design` is folded into work (see
    /// `store.rs::from_paths`); afterwards the design field participates in
    /// no semantic writes (the original value is preserved verbatim by
    /// whole-preferences writes so the legacy fold source stays available).
    mode_defaults: Arc<RwLock<ModeDefaultPrefs>>,
    /// Persistence mutex for `_multi_agent.json`: the in-memory snapshot and
    /// the tmp+rename must complete inside the same critical section. Without
    /// it, two concurrent saves each read snapshots from different moments and
    /// the **older snapshot that finishes writing last** overwrites the newer
    /// one — after a restart, the flag state of some sessions is gone.
    multi_agent_flags_io: Arc<Mutex<()>>,
    /// Persistence mutex for `_pinned_sessions.json` (same contract as
    /// `multi_agent_flags_io`): the id-level RMWs of `set_pinned` and the
    /// retention sweep must be serialized, or a lost update erases a freshly
    /// written pin and the sweep then turns that session evictable.
    pinned_sessions_io: Arc<Mutex<()>>,
    /// Persistence mutex for `_hidden_sessions.json` (same contract as
    /// `pinned_sessions_io`).
    hidden_sessions_io: Arc<Mutex<()>>,
    /// Persistence mutex for `_session_models.json` (same contract as
    /// `pinned_sessions_io`): `set_session_model_id` is self-consistent across
    /// the RMW under the cache's write lock, but the sweep side's batch removal
    /// was previously lock-free — interleaving loses updates.
    session_models_io: Arc<Mutex<()>>,
    /// Persistence mutex for `_session_mode_states.json` (same contract as
    /// `pinned_sessions_io`): shared by set_mode / set_mode_and_persist / the
    /// retention sweep.
    session_mode_states_io: Arc<Mutex<()>>,
    /// In-process snapshot cache of `manager.list_sessions()`. Every upstream
    /// call does a full-directory read_dir + per-file prefix parsing, and the
    /// startup paths (boot restore / retention policy / AcpPool metadata) plus
    /// every list command call it — the same-generation metadata gets rescanned
    /// 3+ times. The cache is invalidated by `save_session_atomic`/`delete` and
    /// the other App-side exclusive write paths.
    ///
    /// Entry = (generation, sessions-dir change token, snapshot). The
    /// generation covers this process's writes; the token covers a peer
    /// process's creates and deletes, which a headless `agent run` sharing
    /// `PINVOU3_HOME` now produces under a live GUI. See
    /// `SessionStore::sessions_dir_change_token` for what the token does and
    /// does not observe.
    pub(crate) list_cache: Arc<
        RwLock<
            Option<(
                u64,
                Option<Vec<std::ffi::OsString>>,
                Arc<Vec<deepseek_tui::session_manager::SessionMetadata>>,
            )>,
        >,
    >,
    /// Generation counter for `list_cache`: incremented on every invalidation
    /// and compared before backfill — a scan result produced across a write
    /// during the miss must not be backfilled (a resurrected stale snapshot
    /// would linger until the next write). See store.rs.
    pub(crate) list_cache_generation: Arc<AtomicU64>,
    /// Session-purged hook (dependency inversion): see
    /// [`SessionPurgedHook`]. None = nobody registered (tests/early
    /// startup); deletion proceeds and only the process-level state goes
    /// unnotified.
    session_purged_hooks: Arc<RwLock<Vec<SessionPurgedHook>>>,
    /// Durable-session-deleted hook registry. This lifecycle point is separate
    /// from `session_purged_hooks`: the durable JSON can be committed as absent
    /// before workspace/side-map cleanup succeeds, while a side-map purge does
    /// not itself prove that the durable session record is absent.
    session_deleted_hooks: Arc<RwLock<Vec<SessionDeletedHook>>>,
    /// Headless retention-eviction receiver: the `agent run` runner installs
    /// one around the turn so its stderr warning keys on the sweep's real
    /// deletions (see `product_runtime::agentic_task`). `None` everywhere
    /// else — the GUI never installs one and the sweep pays nothing.
    #[cfg(feature = "benchmark-hooks")]
    retention_eviction_observer: Arc<Mutex<Option<Arc<Mutex<RetentionEvictionRecord>>>>>,
    /// Evictions recorded before any observer was installed (the headless
    /// host boots — and sweeps — before the runner arms). Flushed into the
    /// observer when one is installed; the record windows its ids, since a
    /// process that never arms must not grow it without bound. Gated like
    /// the observer.
    #[cfg(feature = "benchmark-hooks")]
    pending_retention_evictions: Arc<Mutex<RetentionEvictionRecord>>,
}

/// Execution-root resolver for native code sessions (Pinvou Engine): a native
/// code session with a bound project directory returns `Some(project
/// directory)`; all other sessions return `None` and the caller falls back to
/// the session-private directory.
///
/// A closure instead of a direct dependency on `codex_acp::SessionAgentStore`:
/// the `sessions` and `codex_acp` features referencing each other would form a
/// cycle, so the resolver is injected by the app composition root (lib.rs) and
/// shares the same store held by AcpPool (clone shares the Arc, so the latest
/// binding is read at runtime).
pub type ExecutionRootResolver = Arc<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;

/// Pinvou-native code session predicate closure: injected for the same reason
/// as `ExecutionRootResolver` (avoiding a sessions ↔ codex_acp cycle); lib.rs
/// shares the same `SessionAgentStore`. ACP sessions are always plain in their
/// store (explicitly reset on `bind_*`), so a hit from this predicate means
/// "Pinvou-native code session" and never misclassifies an ACP session's own
/// permission mode.
pub type CodeSessionPredicate = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Session-purged hook: fired by [`SessionStore::delete`] and deep deletion
/// paths inside the sessions feature (retention policy/scheduled cleanup),
/// notifying process-level state holders (timing/pending_user_input, etc.)
/// to clear their keys. Same injection reason as `ExecutionRootResolver` —
/// `sessions` must not depend on `assistant` in reverse (the architecture
/// guard's feature dependency direction); the app composition root
/// registers it.
pub type SessionPurgedHook = Arc<dyn Fn(&str) + Send + Sync>;

/// Durable-session-deleted hook: fired as soon as `<id>.json` is confirmed
/// absent, including partial commits where later workspace cleanup reports an
/// error. Hooks must be synchronous, non-blocking wakeups; asynchronous cleanup
/// is owned by the composition root.
pub type SessionDeletedHook = Arc<dyn Fn(&str) + Send + Sync>;

/// The two roots of a session:
/// - `execution`: engine cwd / shell execution directory. Native code sessions
///   with a bound project directory, or plain chat sessions with a bound user
///   working directory = the bound directory; other sessions = session-private
///   directory (scheduled sessions = their automation workspace).
/// - `ledger`: application ledger root (attachments/audits/artifacts/remote
///   grants). Sessions with a bound directory always use the session-private
///   directory (the user's directory stays clean); other sessions match `execution`.
///
/// Resolved uniformly by [`SessionStore::session_roots`]; callers explicitly
/// pick the root matching their purpose, avoiding writing to the execution
/// root while intending the ledger root (or vice versa).
///
/// `bound` is the explicit "bound to a real directory" signal (a native code
/// session's project directory, or a plain chat session's user working-directory
/// binding): callers must use it to detect the bound state and must not
/// substitute a `ledger != execution` path comparison — once other dual-root
/// shapes appear, path equality no longer implies binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRoots {
    pub execution: PathBuf,
    pub ledger: PathBuf,
    pub bound: bool,
}

/// Pure resolution of the two roots: given the execution directory bound to the
/// session (a native code session's project directory, or a plain chat session's
/// user working directory; pass `None` when unbound), returns the execution root
/// and ledger root. Unaware of scheduled sessions — both of a scheduled session's
/// roots are its automation workspace, handled upstream by
/// [`SessionStore::session_roots`].
pub fn session_roots_for(session_id: &str, bound_project_root: Option<PathBuf>) -> SessionRoots {
    let private = paths::session_workspace_dir(session_id);
    match bound_project_root {
        Some(project) => SessionRoots {
            execution: project,
            ledger: private,
            bound: true,
        },
        None => SessionRoots {
            execution: private.clone(),
            ledger: private,
            bound: false,
        },
    }
}
