//! Registry and decision caches (per session) for the in-process interactive approval surface (tri-mode safety).
//!
//! The base engine emits `Event::ApprovalRequired` for promptable tools while `ask_on_guardrail` is armed
//! (or in the Auto stance). The forwarder no longer auto-approves unconditionally; dispatch goes through
//! [`approval_surface_decision`] in a fixed order: session denial cache → Plan defensive hard-deny →
//! session grant cache → tri-mode stance. Raised cards are registered in this module for the `get_pending_approvals`
//! command, so the frontend can restore approval cards after a remount (same restore idea as `pending_user_input`).
//!
//! The grant/denial caches mirror the scoping semantics of the TUI `resolve_approval_request_disposition`
//! (crates/tui/src/core/authority.rs): **denials are scoped to the exact approval_key**
//! (#1617, changed arguments mean asking again), **grants to the lossy grouping_key**
//! (v0.8.37, "allow for this session" covers later flag variants). A force_prompt hold is never
//! let through by a cache — only the card a human has clicked face-to-face can pass. Everything is memory-resident:
//! after a process restart the engine lazy-spawns and the caches die with the process; no persistence needed.
//! Pending cards are swept as a fallback at the turn terminal state via
//! `clear_pending` (a cancel/interrupt may leave no per-item decision); grants/denials are
//! session-level memory that **survives across turns** (matching the TUI per-session approval cache: the TUI only
//! clears on session/workspace switch); the host-side cleanup point is session deletion (`clear_session` in the lib.rs session-cleanup handler). The bind-commit point of a directory rebind (projects.rs via the
//! [`commit_rebind_publication`]
//! publication fence) advances, for every moved session, the
//! **workspace generation** ([`advance_workspace_generation`]) and runs preserve-generation cleanup
//! ([`clear_session_preserving_generation`], review JensenChen28 5764675531
//! item 1, mechanism re-landed on this line from the round-2/3 shape): request/decision writes carry the generation
//! captured at emission time and are rejected inside the lock on mismatch with the committed one — beyond "clear on workspace switch" this adds a version boundary, so late old-directory decisions never reach the new
//! directory's memory. The bind write and the advance/cleanup share one critical section
//! (zhuowp 5780927743 P2 single identity boundary); for spawn-side identity capture see
//! [`capture_workspace_identity`].

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::core::mode_state::SerializableMode;

/// A pending approval card. Fields map one-to-one to the base `Event::ApprovalRequired`.
/// serde camelCase matches the frontend get_pending_* read style.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingApproval {
    /// Engine approval id (`Event::ApprovalRequired.id`), echoed back by decide_approval.
    pub approval_id: String,
    pub tool_name: String,
    pub description: String,
    /// Tool arguments, for approval display. Passed through as-is, no reprocessing.
    pub input: serde_json::Value,
    /// The model's explanation of a write tool's intent (#2381); None = a read-only tool has no summary.
    pub intent_summary: Option<String>,
    /// exact-args fingerprint: the denial scope (#1617).
    pub approval_key: String,
    /// lossy / arity-aware fingerprint: the session-grant scope (v0.8.37).
    pub approval_grouping_key: String,
    /// Base hold forcing a card (CW-7495): a session grant still requires asking face-to-face.
    pub force_prompt: bool,
    pub created_at_ms: u64,
    /// The **emitting instance's** workspace generation: the forwarder snapshots it when its engine instance
    /// spawns (an instance's identity belongs to its spawn moment) and [`record`] stamps it under the lock
    /// (review 5764675531, sourcing semantics per round-4 P2 sub-gap a). Decision writes
    /// ([`remember_grant`] / [`remember_denial`]) carry it for an atomic invalidation comparison:
    /// generation at write time ≠ currently committed generation (the rebind bind-commit point has advanced)
    /// drops the whole write — late old-scope decisions must not stamp the new generation. Only `Serialize`
    /// is derived: the registry is purely memory-resident, no on-disk state; the failure-arm restore
    /// ([`restore_if_generation_is`]) brings the stamped generation back as-is, so a restored card's next decision is still validated against the same era.
    pub request_workspace_generation: Option<u64>,
}

/// Millisecond UNIX timestamp; a clock rollback would only jitter ordering in extreme cases — not worth a dependency.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Default)]
struct ApprovalStore {
    by_session: HashMap<String, Vec<PendingApproval>>,
    /// grouping_key → set of sessions that granted it. Set-valued so "the same key granted by several sessions"
    /// stays independent: `clear_session` removes only its own share, never revoking another session's grant.
    grants: HashMap<String, HashSet<String>>,
    /// approval_key → set of sessions that denied it (same shape as grants, different scope:
    /// exact key here, grouping key there).
    denials: HashMap<String, HashSet<String>>,
    /// Incremented on every mutation. Intended for a future staleness probe ("has the registry changed since the
    /// list was read"); today it only guarantees that every observable mutation advances it.
    generation: u64,
    /// Session → committed workspace generation (advanced at the rebind bind-commit point). A missing entry = never
    /// rebound (None); [`remember_grant`] / [`remember_denial`] compare the write's carried generation against it
    /// inside the same lock critical section, refusing the write on mismatch (including None vs Some(1)).
    workspace_generations: HashMap<String, u64>,
}

static PENDING: Mutex<Option<ApprovalStore>> = Mutex::new(None);

fn with_store<T>(f: impl FnOnce(&mut ApprovalStore) -> T) -> T {
    let mut guard = PENDING.lock().unwrap_or_else(|poison| poison.into_inner());
    f(guard.get_or_insert_with(ApprovalStore::default))
}

/// Registered when the forwarder raises a card. `instance_generation` is the **emitting instance's** workspace
/// generation — the forwarder (one task per engine incarnation) snapshots [`workspace_generation`] when that
/// instance spawns and must not pass "the current generation at card-receive time" (review
/// 5764675531, round-4 P2 sub-gap a sourcing): a turn started past the entry busy-gate during a rebind still runs
/// on the old cwd runtime, and if a card popped by the old instance after the bind-commit-point generation
/// advance were stamped with the current generation, the old directory's request would be booked under the new
/// generation, and a user approval would write new-generation memory. Deduplicated by approval_id (in-place
/// replace): an engine replaying the same approval must not raise a second card, but fields (such as
/// intent_summary) follow the latest occurrence — the workspace generation likewise takes the latest instance
/// generation (same-instance replay = same generation, still new-request semantics; engine approval ids are minted per instance, so a rebuilt instance never replays old ids).
pub fn record(session_id: &str, mut approval: PendingApproval, instance_generation: Option<u64>) {
    with_store(|store| {
        store.generation = store.generation.wrapping_add(1);
        // Stamp the emitting instance's spawn generation at request creation: decision writes compare against it for atomic invalidation.
        approval.request_workspace_generation = instance_generation;
        let entries = store.by_session.entry(session_id.to_string()).or_default();
        if let Some(existing) = entries
            .iter_mut()
            .find(|entry| entry.approval_id == approval.approval_id)
        {
            *existing = approval;
            return;
        }
        entries.push(approval);
    });
}

/// The session's currently committed workspace generation (read-only): [`capture_workspace_identity`] uses it
/// for the before/after generation reads inside the same synchronous section of `AppEngine::spawn_for_session`
/// that pins `engine_config.workspace` (the await-free span after roots resolution), capturing a
/// consistent instance identity generation that is passed to the forwarder at spawn for [`record`] to stamp —
/// it must not be read live on the forwarder async body's first execution (reviews 5766477612/5766212059/
/// 5766526920: that execution may already be past the rebind bind-commit-point generation advance), nor may
/// the spawn side bypass [`capture_workspace_identity`] with a single live read (review
/// 5768924092/5768948485 P2-a round 2: a single read shares no lock with roots resolution, and a
/// rebind commit interleaved from another thread tears the snapshot). Sessions never rebound return None —
/// consistent with the decision-write comparison semantics of new instances (None == None).
pub fn workspace_generation(session_id: &str) -> Option<u64> {
    with_store(|store| store.workspace_generations.get(session_id).copied())
}

/// Advances the session's committed workspace generation, returning the advanced value. The caller is the
/// rebind bind-commit point (same critical-section logic inside the publication fence [`commit_rebind_publication`],
/// adjacent to the preserve-generation cleanup): a bind commit is an identity switch closing the old-directory
/// era — request cards and decision writes carrying the old generation are rejected in-lock on mismatch. A missing entry counts as 0 (first rebind → 1).
pub fn advance_workspace_generation(session_id: &str) -> u64 {
    with_store(|store| {
        store.generation = store.generation.wrapping_add(1);
        let next = store
            .workspace_generations
            .entry(session_id.to_string())
            .or_insert(0);
        *next = next.wrapping_add(1);
        *next
    })
}

/// Retry cap after a failed identity-capture round. All 4 attempts torn → `Err` (fail-closed: refusing
/// this spawn), no unbounded spinning.
const CAPTURE_IDENTITY_ATTEMPTS: usize = 4;

/// **Consistent capture** of the spawn-side workspace identity (single identity boundary, review zhuowp
/// 5780927743 P2; the detect-retry comparison is retained from review 5768924092 / 5768948485 P2-a round 2
/// as defense in depth). Generation reads and roots resolution sit under two different locks (the approval
/// store lock and the `SessionStore` read path); without a shared critical section, a rebind commit interleaved
/// between "read generation → resolve roots" yields a torn snapshot: the directory runtime and the generation stamp
/// land in different eras — a card carrying generation B passes the decision comparison and directory A's
/// grants/denials are written into B; worse, in the "new root published, generation not yet advanced" window
/// the generation-blind session-grant lookup can still reuse old-directory grants. Detect-retry only narrows the tear window and cannot close the swap→advance two-step commit's own window.
///
/// This protocol adds the missing critical section itself: the `gen_a` read, `resolve()` (the SessionStore read
/// path, lock order approval store → SessionStore), and the `gen_b` read share **one store-lock critical section**,
/// mutually exclusive with the rebind publication fence ([`commit_rebind_publication`], same lock order) — a capture
/// either sees the complete pre-commit state (old root + old generation + old caches) or the complete post-commit
/// state (new root + new generation + cleared caches); the two-step commit's intermediate state is unobservable,
/// and the "new root published, approval memory not yet invalidated" window disappears.
///
/// The in-window comparison is kept as defense in depth: `gen_a != gen_b` or `roots != expected`
/// (the same roots the engine side resolved **earlier** and already fed into `engine_config.workspace` — if a
/// rebind lands entirely between the engine-side resolution and this capture, expected is the old value while
/// the capture can only read the new state) invalidates the whole round as a torn window and retries; all attempts exhausted →
/// `Err`, and the call site refuses this spawn (`engine_config.workspace` is still the old root then; letting it
/// through is the defect itself — fail-closed re-asking beats writing memory across eras).
///
/// The `resolve` closure runs under the store lock: its body may only walk SessionStore / bridge
/// read paths (downstream of the lock order) and must not call back into any lock-taking function of this
/// module (std Mutex is not reentrant; nesting deadlocks on itself).
///
/// Returns `(roots, gen_b)`: roots are only for protocol comparison (accepted only if equal to `expected`); the
/// call site's actual run directory still comes from its previously constructed workspace; `gen_b` is the
/// instance identity generation, passed to the forwarder at spawn for stamping ([`record`]).
pub fn capture_workspace_identity<T: PartialEq>(
    session_id: &str,
    expected: &T,
    resolve: impl Fn() -> T,
) -> Result<(T, Option<u64>), String> {
    for _ in 0..CAPTURE_IDENTITY_ATTEMPTS {
        let (generation_before, roots, generation_after) = with_store(|store| {
            let generation_before = store.workspace_generations.get(session_id).copied();
            let roots = resolve();
            let generation_after = store.workspace_generations.get(session_id).copied();
            (generation_before, roots, generation_after)
        });
        if generation_before != generation_after {
            // Defense in depth: within one critical section the generation cannot change (the rebind publication
            // fence holds the same lock); this arm only guards a future move of the reads outside the lock — retry.
            continue;
        }
        if roots != *expected {
            // Generation matches but the roots drifted from the engine-side resolved value: the rebind commit
            // landed entirely between the engine-side resolution and this capture (expected old, capture new) —
            // invalidate and retry as a torn window (exhaustion is fail-closed).
            continue;
        }
        return Ok((roots, generation_after));
    }
    Err(format!(
        "workspace rebind is in flight for session {session_id}: no \
         consistent workspace identity could be captured across \
         {CAPTURE_IDENTITY_ATTEMPTS} attempts; refusing this spawn \
         (fail-closed) — resend the request once the rebind settles"
    ))
}

/// The rebind publication fence (single identity boundary, review zhuowp 5780927743 P2): it puts the **bind
/// write** (the root-switch publication in SessionStore / the codex index) and the **same sessions' identity advance + approval-memory
/// cleanup** (advance + preserve-generation clear) into **one critical section** under the approval store lock. The
/// previous two-step commit (publish the new root first, then advance+clear) left a "new root published, approval memory not yet
/// invalidated" window: instances started inside it see two consistent generation reads and expected/resolved roots both
/// pointing at the new directory, the detect-retry protocol lets them through, and the generation-blind session-grant lookup
/// can still reuse the old directory's grants — cached decisions escape the invalidation window.
///
/// Once mutually exclusive with [`capture_workspace_identity`] (same lock, same order), the window is unobservable:
/// a capture sees either the complete pre-commit state or the complete post-commit state (new root + new generation +
/// grants cleared, no old-directory grant left to reuse). Lock order is throughout
/// approval store → downstream (SessionStore / codex section / fs). Lock-order audit (this round, precise scope): across codex_acp and bridge
/// there is no path that, holding a downstream lock, calls back into this module; the sessions side nests **one-way** —
/// `SessionStore::delete` holds `scheduled_mutation` throughout, and its purge hook takes this module's store lock
/// inside that lock (lib.rs's `clear_session`).
/// The safety invariant is therefore "inside the publish/resolve closures, never take `scheduled_mutation`
/// or any sessions lock again" (all three closures verified to walk only `session_workspaces` /
/// codex section / fs / read-only paths); keeping that direction one-way means no AB-BA and no deadlock.
/// While the fence holds the lock the approval surface briefly pauses = the duration of the rebind's own critical
/// section (consistent with the publication/admission coordination the reviews prescribed); the cost is explicit,
/// and the payoff is the atomicity of the identity switch.
///
/// `publish` runs under the lock (downstream reads/writes only, no callbacks into this module); `affected_ids`
/// extracts the moved sessions from the publish result and runs advance + preserve-generation cleanup on each (same
/// order as the existing contract: advance first so the descendant era lives, then clean up, late old-generation writes always mismatch). Returns the `publish`
/// result for the caller to continue with (reporting, metadata loops, etc.).
pub fn commit_rebind_publication<R>(
    publish: impl FnOnce() -> R,
    affected_ids: impl FnOnce(&R) -> Vec<String>,
) -> R {
    with_store(|store| {
        let result = publish();
        for session_id in affected_ids(&result) {
            // Same shape as the existing projects.rs contract: advance first (the version boundary of the B-directory
            // era), then the preserve-generation cleanup (reclaiming pending cards / grant / denial shares). Within
            // one critical section the two steps are order-equivalent — the real load-bearing invariant is that the
            // cleanup must be the **preserve-generation** variant, not the wholesale `clear_session` (falling back to
            // no generation would re-equalize pre-advance old decisions). That invariant is pinned by
            // `forkguard_identity_switch_cleanup_preserves_workspace_generation`
            // ; see the docs of [`advance_workspace_generation`] and
            // [`clear_session_preserving_generation`].
            store.generation = store.generation.wrapping_add(1);
            let next = store
                .workspace_generations
                .entry(session_id.clone())
                .or_insert(0);
            *next = next.wrapping_add(1);
            store.by_session.remove(&session_id);
            store.grants.retain(|_, sessions| {
                sessions.remove(&session_id);
                !sessions.is_empty()
            });
            store.denials.retain(|_, sessions| {
                sessions.remove(&session_id);
                !sessions.is_empty()
            });
        }
        result
    })
}

/// Unfenced removal helper (since round-8 `decide_approval` goes through [`remove_fenced`], this function has no
/// production caller anymore — same category as [`clear_pending`], kept as test/probe glue; new callers must use
/// the fenced variant, otherwise the failure arm's restore protocol loses its generation precondition). Returns the
/// removed entry (decisions need its approval_key / grouping_key).
pub fn remove(session_id: &str, approval_id: &str) -> Option<PendingApproval> {
    with_store(|store| {
        store.generation = store.generation.wrapping_add(1);
        let entries = store.by_session.get_mut(session_id)?;
        let position = entries
            .iter()
            .position(|entry| entry.approval_id == approval_id)?;
        let removed = entries.remove(position);
        if entries.is_empty() {
            store.by_session.remove(session_id);
        }
        Some(removed)
    })
}

/// Fenced removal for the decide-approval failure arm: like [`remove`], but
/// also returns the store generation captured atomically under the same lock
/// as the removal (the removal itself bumps it). The command layer re-reads
/// [`generation`] before restoring a removed entry, so any registry mutation
/// that raced the await — the turn-terminal sweep, a session purge, a
/// concurrent decision or a newly raised card — cancels the restore instead
/// of recreating an entry the registry lifecycle already closed (reviewer
/// P2: restore only within the same still-live approval lifecycle).
pub fn remove_fenced(session_id: &str, approval_id: &str) -> Option<(PendingApproval, u64)> {
    with_store(|store| {
        store.generation = store.generation.wrapping_add(1);
        let entries = store.by_session.get_mut(session_id)?;
        let position = entries
            .iter()
            .position(|entry| entry.approval_id == approval_id)?;
        let removed = entries.remove(position);
        if entries.is_empty() {
            store.by_session.remove(session_id);
        }
        Some((removed, store.generation))
    })
}

/// Fenced restore for the decide-approval failure arm: re-inserts the entry
/// removed by [`remove_fenced`] only while the registry generation still
/// equals the snapshot captured atomically with that removal — the
/// comparison and the insertion share one registry critical section.
/// `decide_approval` previously read [`generation`] and then called
/// [`record`] as two separate lock acquisitions, so a terminal sweep could
/// run between the comparison and the insertion and the stale restore
/// resurrected an entry whose engine-side lifecycle had already closed
/// (reviewer P2). Returns whether the restore happened. A `false` return
/// drops the registry entry for a still-open engine approval: retries and
/// remounts report "approval not found" until the turn's deadline or an
/// explicit cancel — there is no automatic re-trigger path (the generation
/// is process-global, so any unrelated card or session can advance it
/// during the await, round-19 review minor 5). That is still the safe
/// direction: a wrongly restored entry would recreate a dead decision
/// surface.
pub fn restore_if_generation_is(
    session_id: &str,
    approval: PendingApproval,
    expected_generation: u64,
) -> bool {
    with_store(|store| {
        if store.generation != expected_generation {
            return false;
        }
        store.generation = store.generation.wrapping_add(1);
        let entries = store.by_session.entry(session_id.to_string()).or_default();
        if let Some(existing) = entries
            .iter_mut()
            .find(|entry| entry.approval_id == approval.approval_id)
        {
            *existing = approval;
            return true;
        }
        entries.push(approval);
        true
    })
}

/// Test-only: the registry + `generation` are process-global singletons, so
/// ALL tests that touch them must serialize on ONE shared lock. This
/// crate-visible accessor is that lock; this module's `test_registry_lock`
/// delegates to it, and sibling-module tests (projects.rs's rebind fences)
/// take it directly — two separate statics would not exclude each other
/// (round-13 review flagged the residual cross-module race window).
#[cfg(test)]
pub(crate) fn test_registry_lock_for_sibling_tests() -> std::sync::MutexGuard<'static, ()> {
    static REGISTRY_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    REGISTRY_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

/// Current store generation (see [`ApprovalStore::generation`]). Read-side
/// helper; production fencing goes through [`remove_fenced`]'s atomic
/// snapshot — today's direct callers are tests/probes. A global
/// counter by design: the restore fence is deliberately conservative — any
/// observable registry change during the await window skips the restore.
/// A skipped restore for a still-open engine approval strands that approval
/// (retries/remounts report "approval not found" until the turn deadline or
/// an explicit cancel — no automatic re-trigger exists, round-19 review
/// minor 5); it is still the safe direction, since a wrongly restored entry
/// recreates a decision surface whose engine-side approval no longer
/// exists.
pub fn generation() -> u64 {
    with_store(|store| store.generation)
}

/// Fallback sweep of the session's pending cards at the turn terminal state (a cancel/interrupt may leave no
/// per-item decision). Grants/denials are session-level memory and **do not die with the turn terminal state** —
/// "allow for this session" must survive across turns (TUI semantics: cleared only on session/workspace switch), otherwise the button is really "allow for this turn". The cleanup point for session-level memory is
/// [`clear_session`] (session deletion).
///
/// Production terminal paths should use [`clear_pending_returning_child_ids`] instead: detached-worker approvals
/// must not be silently dropped here (dropping = stranding the worker); this function remains for
/// tests and future non-terminal cleanup points.
pub fn clear_pending(session_id: &str) {
    with_store(|store| {
        store.generation = store.generation.wrapping_add(1);
        store.by_session.remove(session_id);
    });
}

/// Split variant of the terminal sweep: removes all of the session's pending cards and returns the approval_ids
/// of the **detached worker approvals** among them. Base subagent approval ids look like
/// `agent:<agent_id>:approval:<boot_id>:<n>` (minted via
/// `SubAgentManager::register_child_approval`) and are the key to the subagent oneshot: silently
/// dropping them permanently strands a worker still waiting in `WaitingForUser` (detached workers deliberately
/// outlive the parent turn). The caller must route each returned id through an engine deny
/// (`route_child_approval_decision` routes to the subagent), turning "the parent turn has ended" into a
/// deterministic denial visible to the worker; if the engine has already been reaped, the oneshot settles as
/// `Unavailable` when the receiver drops — also a deterministic terminal state. The parent session's own
/// approval ids (engine per-turn approvals die with the turn) can simply be dropped.
pub fn clear_pending_returning_child_ids(session_id: &str) -> Vec<String> {
    with_store(|store| {
        store.generation = store.generation.wrapping_add(1);
        let entries = store.by_session.remove(session_id).unwrap_or_default();
        entries
            .into_iter()
            .filter(|entry| {
                deepseek_tui::tools::subagent::SubAgentManager::is_child_approval_id(
                    &entry.approval_id,
                )
            })
            .map(|entry| entry.approval_id)
            .collect()
    })
}

/// On session deletion/cleanup, removes all of the session's shares: pending cards, grants, denials, and the
/// workspace generation (a full lifecycle reset: a same-id session recreated after deletion starts again from "no
/// generation"). Grants/denials are stored as per-session sets, so only the session's own share is removed, never
/// another session's grant. Identity-switch cleanup (the rebind bind-commit point) **must not** use this function —
/// clearing the generation would re-equalize old-generation decisions (review 5764675531); that path uses [`clear_session_preserving_generation`].
pub fn clear_session(session_id: &str) {
    with_store(|store| {
        store.generation = store.generation.wrapping_add(1);
        store.by_session.remove(session_id);
        store.workspace_generations.remove(session_id);
        store.grants.retain(|_, sessions| {
            sessions.remove(session_id);
            !sessions.is_empty()
        });
        store.denials.retain(|_, sessions| {
            sessions.remove(session_id);
            !sessions.is_empty()
        });
    });
}

/// Identity-switch cleanup (the `commit_rebind_publication` publication fence of the rebind bind-commit
/// point): reclaims the stored shares of pending cards, grants, and denials, but **preserves the workspace
/// generation**. This is the only difference from [`clear_session`] and the fix point of review
/// 5764675531 item 1: if the generation were cleared too, the session would fall back to "no generation", and
/// old decisions stamped None before the bind-commit-point advance would re-equal the new state — old grants
/// revive and late old-scope writes get accepted again. Preserving the generation keeps old generations missing
/// forever (re-asking is cheap; a stale silent allow is unacceptable); decisions under the new directory land
/// normally against the surviving generation. The session-deletion path (lib.rs) still does the full [`clear_session`] reset.
pub fn clear_session_preserving_generation(session_id: &str) {
    with_store(|store| {
        store.generation = store.generation.wrapping_add(1);
        store.by_session.remove(session_id);
        store.grants.retain(|_, sessions| {
            sessions.remove(session_id);
            !sessions.is_empty()
        });
        store.denials.retain(|_, sessions| {
            sessions.remove(session_id);
            !sessions.is_empty()
        });
    });
}

/// Process-wide full snapshot (for tests and future staleness probes, no production caller): the real frontend
/// remount restore path is `get_pending_approvals` → [`list`] (filtered by session). Sorting by
/// (session, created_at) keeps the output stable.
pub fn list_all_sessions() -> Vec<(String, PendingApproval)> {
    let mut all = with_store(|store| {
        store
            .by_session
            .iter()
            .flat_map(|(session_id, entries)| {
                entries
                    .iter()
                    .map(|entry| (session_id.clone(), entry.clone()))
            })
            .collect::<Vec<_>>()
    });
    all.sort_by(|(left_session, left), (right_session, right)| {
        left_session
            .cmp(right_session)
            .then(left.created_at_ms.cmp(&right.created_at_ms))
            .then(left.approval_id.cmp(&right.approval_id))
    });
    all
}

pub fn list(session_id: &str) -> Vec<PendingApproval> {
    with_store(|store| {
        store
            .by_session
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    })
}

/// Whether this session previously granted this lossy operation (a grouping_key hit allows it). The read path
/// must carry the session dimension: grants are **session-level** memory, and session A's grant must not allow
/// session B's same-family calls (cross-session allow = one click authorizing the whole process, breaking the "allow for this session" semantics).
pub fn session_granted(session_id: &str, grouping_key: &str) -> bool {
    with_store(|store| {
        store
            .grants
            .get(grouping_key)
            .is_some_and(|sessions| sessions.contains(session_id))
    })
}

/// "Allow for this session": the grant is recorded on the grouping_key (covering later flag variants, v0.8.37).
/// `origin_generation` is the originating request's workspace generation (decide_approval writes with the
/// registered `pending.request_workspace_generation`): compared against the currently committed generation under
/// the same store lock; on mismatch (including None vs Some(1)) the whole write is refused and no memory is
/// written (review 5764675531 item 1: after the rebind bind-commit point advances the generation, late writes from
/// a resumed old success arm must not stamp the new generation; comparison and insertion share one critical section, no check-then-write window). Returns whether the write
/// landed; the cost of `false` is one re-ask — silently writing the wrong generation is unacceptable.
pub fn remember_grant(
    session_id: &str,
    grouping_key: &str,
    origin_generation: Option<u64>,
) -> bool {
    with_store(|store| {
        if store.workspace_generations.get(session_id).copied() != origin_generation {
            return false;
        }
        store.generation = store.generation.wrapping_add(1);
        store
            .grants
            .entry(grouping_key.to_string())
            .or_default()
            .insert(session_id.to_string());
        true
    })
}

/// Whether this session previously denied this exact operation (an exact approval_key hit denies it).
/// Same as [`session_granted`]: denials are also session-level memory, never guilt-by-association across sessions.
pub fn session_denied(session_id: &str, approval_key: &str) -> bool {
    with_store(|store| {
        store
            .denials
            .get(approval_key)
            .is_some_and(|sessions| sessions.contains(session_id))
    })
}

/// The denial is recorded on the exact key (#1617: changed arguments require asking again, no guilt by association).
/// Generation validation matches [`remember_grant`]: the origin generation is compared inside the same lock
/// critical section, and a mismatch refuses the write (a late old-directory denial must not spill into the new directory).
pub fn remember_denial(
    session_id: &str,
    approval_key: &str,
    origin_generation: Option<u64>,
) -> bool {
    with_store(|store| {
        if store.workspace_generations.get(session_id).copied() != origin_generation {
            return false;
        }
        store.generation = store.generation.wrapping_add(1);
        store
            .denials
            .entry(approval_key.to_string())
            .or_default()
            .insert(session_id.to_string());
        true
    })
}

// ===================== Pure decision functions (tri-mode approval-surface ordering) =====================

/// Hard-deny reason. `reason_key` is the stable English key in the `chat:guardrail_denied` event; the
/// frontend/i18n maps the copy from it. Human-readable wording never enters the protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDenyReason {
    /// The session previously chose to deny this exact operation (exact approval_key).
    SessionDenied,
    /// In the Plan stance the base should block tool execution itself; receiving an approval event is anomalous — defensive hard-deny.
    PlanMode,
    /// Yolo full-auto with ask-on-guardrail off: a guardrail hit hard-denies per the stance.
    GuardrailAskOff,
}

impl ApprovalDenyReason {
    pub fn reason_key(self) -> &'static str {
        match self {
            Self::SessionDenied => "session_denied",
            Self::PlanMode => "plan_mode",
            Self::GuardrailAskOff => "guardrail_ask_off",
        }
    }
}

/// The approval surface's disposition. The scheduled unattended path never reaches here (short-circuited earlier in the forwarder).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalSurfaceDecision {
    /// Silent approval (a previous grant hit): no event — ToolCallStarted already showed the call.
    ApproveSilently,
    /// Silent hard-deny plus a `chat:guardrail_denied` notification, no card.
    Deny(ApprovalDenyReason),
    /// Register as pending and raise a `chat:approval_required` card.
    ShowCard,
}

/// The branch order builds on the TUI `resolve_approval_request_disposition` (authority.rs), with the Plan
/// defensive arm deliberately stricter: **session denial > Plan hard-deny > session grant > force_prompt card >
/// stance decision**. Scheduled unattended runs are short-circuited by the caller and never reach this function;
/// scheduled/multi-agent sessions other than unattended ones follow the same order as normal chats.
///
/// - Denial cache hit: hard-deny regardless of stance (TUI legacy order: session denial first).
/// - Plan: the base Plan flow should have blocked tool execution; an arriving approval event is anomalous, so
///   hard-deny defensively — **ahead of the grant cache**. TUI legacy order puts the grant first, but the TUI's
///   Plan flow never produces approval events; a host-side turn may switch to Plan mid-flight (defensive
///   semantics), and a still-silently-allowed old grant would puncture "Plan = read-only" — hard-deny is fail-safe and zero-cost.
/// - Grant cache hit and not force_prompt: silent approval (a force_prompt hold only passes the card clicked
///   face-to-face; the cache must not let it through).
/// - force_prompt: always raise a card (an armed host guarantees a responder). Note: on an unarmed host with
///   Yolo + toggle off, the base faces deny inside the engine gate and emit no force events (see the #131 wiring),
///   so this arm is in practice only reachable on armed hosts and forms no reachable divergence with the base
///   `AutoDenyFullAccessPolicyHold`.
/// - Yolo + toggle off: a guardrail hit hard-denies per stance; Yolo + toggle on, and Auto: raise a card (in the
///   Auto stance every ordinary promptable tool asks each time; the toggle only governs guardrail hits).
#[must_use]
pub fn approval_surface_decision(
    session_mode: SerializableMode,
    ask_on_guardrail: bool,
    force_prompt: bool,
    session_granted: bool,
    session_denied: bool,
) -> ApprovalSurfaceDecision {
    if session_denied {
        return ApprovalSurfaceDecision::Deny(ApprovalDenyReason::SessionDenied);
    }
    if session_mode == SerializableMode::Plan {
        return ApprovalSurfaceDecision::Deny(ApprovalDenyReason::PlanMode);
    }
    if session_granted && !force_prompt {
        return ApprovalSurfaceDecision::ApproveSilently;
    }
    if force_prompt {
        return ApprovalSurfaceDecision::ShowCard;
    }
    match session_mode {
        SerializableMode::Yolo if !ask_on_guardrail => {
            ApprovalSurfaceDecision::Deny(ApprovalDenyReason::GuardrailAskOff)
        }
        _ => ApprovalSurfaceDecision::ShowCard,
    }
}

/// The single construction point of the `chat:approval_required` event payload. Fields mirror
/// [`PendingApproval`] one-to-one plus `session_id` (serde camelCase exists only for the
/// remount read path; the event surface here keeps the base event's snake_case keys, consumed by both
/// frontend lanes and the web announcer). The payload used to be hand-built as `json!` in the forwarder,
/// forming a two-place hand-synced shape with this registry — review noted that the deny/resolved/mode_changed
/// payloads all have key-set pins while this one had none (round-8 L3-F2), so it was folded into the same
/// pattern: one construction point + one key-set pin; any field drift turns red.
pub fn approval_required_payload(
    session_id: &str,
    approval: &PendingApproval,
) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "approval_id": approval.approval_id,
        "tool_name": approval.tool_name,
        "description": approval.description,
        "input": approval.input,
        "intent_summary": approval.intent_summary,
        "approval_key": approval.approval_key,
        "approval_grouping_key": approval.approval_grouping_key,
        "force_prompt": approval.force_prompt,
    })
}

/// The single construction point of the `chat:guardrail_denied` event payload. `reason` is the stable English
/// key ([`ApprovalDenyReason::reason_key`]); frontend i18n maps the trilingual copy from it.
/// Deliberately **excludes the raw tool input** — sensitive arguments must not enter the event stream (the
/// desktop emit and the remote forward share this payload); on the card path the input travels only via `chat:approval_required`
/// (the user has already been shown it there).
pub fn guardrail_denied_payload(
    session_id: &str,
    tool_name: &str,
    reason_key: &str,
) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "tool_name": tool_name,
        "reason": reason_key,
    })
}

/// The "unknown outcome" classification for engine approval acks (review P2). Since CodeWhale fb4f5e63f the
/// approve/deny handles carry an ack + settle dual-window confirmation contract (see `EngineHandle::
/// approve_tool_call` docs): a determinate refusal (stale id / failed durable commit / never consumed
/// by the engine / decision channel closed) and an unknown outcome (both windows timed out, the decision may
/// still commit and execute late on the engine side — durable commit has no deadline) are two different `Err`s.
/// The host distinguishes them: a determinately refused card is still a live pending request (fenced restore may retry),
/// while an unknown outcome must not restore active decision controls (a late commit has no host
/// reconciliation event; restoring would promise that a possibly already consumed decision can be redone).
/// The error types are pub(crate) inside the base crate, so the host cannot downcast by type and must classify
/// by Display wording; base wording drift would drop unknown outcomes back into the determinate-refusal arm —
/// restoring a retryable pending card is the old (dishonest) behavior (the direction: restore more, never
/// wrongly allow; both fences remain). Host tests pin the "classifier ↔ wording literals" side; detecting drift
/// in the base Display itself needs the base to export marker constants/types (a CodeWhale#131 follow-up; round-8 L3-F1/L6-F2 honestly narrowed this claim).
pub(crate) fn approval_outcome_is_unknown(error: &anyhow::Error) -> bool {
    error
        .to_string()
        .contains("approval decision outcome is unknown")
}

/// The single construction point of the `chat:approval_resolved` event payload (the tri-mode M5 convergence
/// surface): once a decision settles anywhere it broadcasts the terminal state, so second-window /
/// remotely-initiated decisions also settle cards on other surfaces in place. The frontend applies it only to
/// pending cards (idempotent); the payload carries no tool input (same sensitive-surface discipline as [`guardrail_denied_payload`]).
pub fn approval_resolved_payload(
    session_id: &str,
    approval_id: &str,
    decision: &str,
) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "approval_id": approval_id,
        "decision": decision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Session id prefix shared by unit tests; clear_session runs first to guarantee isolation.
    fn isolate(session: &str) -> String {
        clear_session(session);
        session.to_string()
    }

    /// The registry and `generation` are both process-global (the `PENDING` singleton): any registry mutation by
    /// a parallel test thread advances the generation, making tests whose asserts rely on "no concurrent mutation
    /// within the snapshot window" randomly red (round-10 review evidence: the full 84-test forkguard suite ran
    /// 2/4 red concurrently, hitting `restore_if_generation_is` and `pending_card_stamps`). Every test that
    /// touches the registry takes this lock to serialize first; pure payload-construction/source-pin tests do not need it.
    fn test_registry_lock() -> std::sync::MutexGuard<'static, ()> {
        // Same lock as the sibling accessor: one registry, one serialization
        // point (round-13 review).
        super::test_registry_lock_for_sibling_tests()
    }

    fn sample(approval_id: &str, approval_key: &str, grouping_key: &str) -> PendingApproval {
        PendingApproval {
            approval_id: approval_id.to_string(),
            tool_name: "write_file".to_string(),
            description: "write a file".to_string(),
            input: serde_json::json!({ "path": "a.txt" }),
            intent_summary: Some("save notes".to_string()),
            approval_key: approval_key.to_string(),
            approval_grouping_key: grouping_key.to_string(),
            force_prompt: false,
            created_at_ms: now_ms(),
            // record stamps the caller-passed instance generation; None is just a construction placeholder.
            request_workspace_generation: None,
        }
    }

    /// M6b: the hard-deny notification payload carries only stable keys (session_id/tool_name/reason) and
    /// deliberately no raw tool input — sensitive arguments must not enter the event stream (the desktop emit
    /// and remote forward share this construction point). Any field-set growth must change here.
    #[test]
    fn forkguard_guardrail_denied_payload_carries_stable_keys_only() {
        let payload = guardrail_denied_payload("sess-1", "write_file", "guardrail_ask_off");
        let obj = payload.as_object().expect("payload is a JSON object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["reason", "session_id", "tool_name"],
            "deny payload carries exactly the stable keys (no tool input): {obj:?}"
        );
        assert_eq!(obj["reason"], "guardrail_ask_off");
        let serialized = payload.to_string();
        assert!(
            !serialized.contains("input"),
            "deny payload must not carry tool input: {serialized}"
        );
    }

    /// Key-set pin for the `chat:approval_required` payload (round-8 L3-F2): the event-surface snake_case keys
    /// correspond one-to-one with the registry struct; one extra key (leak) or one missing key (frontend gap) turns red.
    #[test]
    fn forkguard_approval_required_payload_carries_the_registry_field_set() {
        let approval = PendingApproval {
            approval_id: "ap-1".to_string(),
            tool_name: "write_file".to_string(),
            description: "describe".to_string(),
            input: serde_json::json!({"path": "/tmp/x"}),
            intent_summary: Some("intent".to_string()),
            approval_key: "key".to_string(),
            approval_grouping_key: "group".to_string(),
            force_prompt: true,
            created_at_ms: 1,
            // The event payload is hand-built via json! (no generation); the generation appears only on the registry list surface.
            request_workspace_generation: None,
        };
        let payload = approval_required_payload("sess-1", &approval);
        let obj = payload.as_object().expect("payload is a JSON object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "approval_grouping_key",
                "approval_id",
                "approval_key",
                "description",
                "force_prompt",
                "input",
                "intent_summary",
                "session_id",
                "tool_name",
            ],
            "approval_required payload carries exactly the registry field set: {obj:?}"
        );
        assert_eq!(obj["session_id"], "sess-1");
        assert_eq!(obj["approval_id"], "ap-1");
        assert_eq!(obj["force_prompt"], true);
    }

    /// M5 convergence surface: the decision-terminal broadcast payload carries only routing keys + the decision
    /// value (idempotent dedup relies on approval_id), and no tool input.
    #[test]
    fn forkguard_approval_resolved_payload_carries_keys_and_decision_only() {
        let payload = approval_resolved_payload("sess-1", "ap-1", "allow_for_session");
        let obj = payload.as_object().expect("payload is a JSON object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["approval_id", "decision", "session_id"],
            "resolved payload carries exactly the routing keys: {obj:?}"
        );
        assert_eq!(obj["decision"], "allow_for_session");
    }

    /// Contract pin for the unknown-outcome classifier (review P2): the types are pub(crate) in the base crate,
    /// so the host classifies by Display wording — drift in the base `ApprovalAckUnknownError` wording
    /// ("approval decision outcome is unknown: …") would silently drop unknown outcomes back into the
    /// determinate-refusal arm (restoring a retryable pending card = the old dishonest behavior), so this pins
    /// the literal wording of both `Err` classes. The determinate-refusal wording comes from the base `ack_answer`
    /// `NotApplied` arms and the closed decision channel; any wording change in an arm turns this test red.
    #[test]
    fn forkguard_approval_outcome_unknown_classifier_separates_the_engine_contracts() {
        // Unknown outcome: ack + settle dual-window timeout (ApprovalAckUnknownError Display).
        let unknown = anyhow::anyhow!(
            "approval decision outcome is unknown: the engine did not \
             acknowledge within 5s ack + 10s settle, and the decision may \
             still be committed and executed"
        );
        assert!(
            approval_outcome_is_unknown(&unknown),
            "the settle-expiry wording must classify as unknown"
        );
        // The three determinate-refusal arms: stale id / never consumed / channel closed — none may classify as unknown.
        let refused_stale = anyhow::anyhow!(
            "approval decision was not applied: the tool call id is stale \
             or its outcome could not be committed"
        );
        let refused_dropped =
            anyhow::anyhow!("approval decision was dropped without being consumed by the engine");
        let channel_closed = anyhow::anyhow!("channel closed");
        for error in [&refused_stale, &refused_dropped, &channel_closed] {
            assert!(
                !approval_outcome_is_unknown(error),
                "a determinate refusal must keep the retryable restore: {error}"
            );
        }
    }

    /// M6a: the forwarder call site is the real wiring beyond the pure function's truth table — swapping the
    /// granted/denied arguments (a recurrence of round-1 B1: cross-session allow) or swapping ask/force would
    /// silently flip the disposition, and truth-table tests cannot see the call site. Pins the argument order and
    /// key dimensions: granted (grouping key, lossy) first and denied (exact approval_key) second,
    /// ask_on_guardrail before force_prompt, and the denial notification goes through the single payload
    /// constructor (no falling back to inline json! that would reintroduce input).
    #[test]
    fn forkguard_forwarder_approval_surface_call_site_pins_argument_order() {
        let src = include_str!("forwarder.rs");
        let production = src.split("#[cfg(test)]").next().expect("non-empty source");
        let call_start = production
            .find("approval_surface_decision(")
            .expect("the forwarder must route ApprovalRequired through the pure decision fn");
        let call = &production[call_start..];
        let call_end = call.find(");").expect("the decision call must terminate");
        let call = &call[..call_end];
        let granted = call
            .find("session_granted(")
            .expect("the grant cache read must feed the granted arm");
        let denied = call
            .find("session_denied(")
            .expect("the denial cache read must feed the denied arm");
        assert!(
            granted < denied,
            "positional argument order: granted (grouping key) must precede denied \
             (exact key) — a swap silently approves denied operations"
        );
        let granted_arm = &call[granted..];
        let denied_arm = &call[denied..];
        assert!(
            granted_arm.contains("&approval_grouping_key"),
            "the granted arm must read the lossy grouping key"
        );
        assert!(
            denied_arm.contains("&approval_key"),
            "the denied arm must read the exact approval key"
        );
        assert!(
            call.find("ask_on_guardrail").is_some(),
            "the ask-on-guardrail arming must be an input to the decision"
        );
        let ask = call.find("ask_on_guardrail").expect("checked above");
        let force = call
            .find("approval_force_prompt")
            .expect("the force flag must be an input to the decision");
        assert!(
            ask < force,
            "positional argument order: ask_on_guardrail must precede approval_force_prompt"
        );
        assert!(
            production.contains("guardrail_denied_payload("),
            "the deny announcement must go through the single payload constructor \
             (no inline json! that could reintroduce tool input)"
        );
    }

    #[test]
    fn forkguard_approval_surface_record_dedupes_by_approval_id() {
        let _registry = test_registry_lock();
        let session = isolate("approval-dedupe-test");
        record(&session, sample("ap-1", "k-1", "g-1"), None);
        record(&session, sample("ap-1", "k-1", "g-1"), None);
        record(&session, sample("ap-2", "k-2", "g-2"), None);
        let listed = list(&session);
        assert_eq!(listed.len(), 2, "同 approval_id 登记去重");
        assert_eq!(listed[0].approval_id, "ap-1", "原位替换不打乱顺序");

        // In-place replacement: a duplicate registration overwrites the fields.
        let mut replaced = sample("ap-1", "k-1", "g-1");
        replaced.intent_summary = None;
        record(&session, replaced, None);
        assert!(list(&session)[0].intent_summary.is_none());
    }

    #[test]
    fn forkguard_approval_surface_remove_returns_entry_and_cleans_empty_session() {
        let _registry = test_registry_lock();
        let session = isolate("approval-remove-test");
        record(&session, sample("ap-1", "k-1", "g-1"), None);
        let removed = remove(&session, "ap-1");
        assert_eq!(
            removed.as_ref().map(|entry| entry.approval_id.as_str()),
            Some("ap-1")
        );
        assert!(remove(&session, "ap-1").is_none(), "重复移除返回 None");
        assert!(list(&session).is_empty());
        assert!(
            !list_all_sessions().iter().any(|(sid, _)| sid == &session),
            "空 session 条目从全局索引摘除"
        );
    }

    #[test]
    fn forkguard_remove_fenced_returns_atomic_generation_snapshot() {
        let _registry = test_registry_lock();
        let session = isolate("approval-remove-fenced-test");
        record(&session, sample("ap-f1", "k-f1", "g-f1"), None);
        let (removed, generation_after_remove) = remove_fenced(&session, "ap-f1")
            .expect("fenced removal must find the recorded approval");
        assert_eq!(removed.approval_id, "ap-f1");
        // The snapshot is captured under the same lock as the removal (the
        // removal itself bumps the generation), so absent further mutations
        // a fresh read equals it — that is the restore fence's "registry
        // unchanged during the await" precondition.
        assert_eq!(generation(), generation_after_remove);
        // Any later observable mutation advances the generation past the
        // snapshot, which is what cancels the decide-approval failure-arm
        // restore (terminal sweep, purge, concurrent decision, new card).
        record(&session, sample("ap-f2", "k-f2", "g-f2"), None);
        assert_ne!(
            generation(),
            generation_after_remove,
            "a registry mutation during the await window must cancel the fenced restore"
        );
        // remove_fenced shares remove's cleanup contract.
        assert!(
            remove_fenced(&session, "ap-f1").is_none(),
            "重复移除返回 None"
        );
    }

    #[test]
    fn forkguard_stale_restore_after_interleaved_mutation_is_refused() {
        let _registry = test_registry_lock();
        let session = isolate("approval-cas-restore-test");
        record(&session, sample("ap-r1", "k-r1", "g-r1"), None);
        let (pending, snapshot) = remove_fenced(&session, "ap-r1")
            .expect("fenced removal must find the recorded approval");
        // Happy path: no mutation since the removal — the compare-and-insert
        // succeeds and the entry is back in the registry.
        assert!(
            restore_if_generation_is(&session, pending.clone(), snapshot),
            "unchanged registry must restore the removed entry"
        );
        assert_eq!(list(&session).len(), 1, "restored entry is pending again");

        // Interleaving (review round 7 P2): a terminal sweep's mutation
        // lands between the remove_fenced snapshot and the restore. The
        // previous read-generation-then-record pair compared the generation
        // in one lock acquisition and inserted in a second, so the sweep
        // could advance it in between and the stale restore resurrected a
        // closed lifecycle's entry. The CAS rejects it under the same lock
        // as the insertion.
        //
        // Naming honesty (round-11 review mutation probe): this sequential
        // test cannot observe a lock split INSIDE restore_if_generation_is —
        // splitting the compare and the insert into two acquisitions yields
        // identical outcomes here, so atomicity itself is not what this test
        // pins. What it pins is the refuse-on-interleaved-mutation CAS
        // contract; the two-acquisition anti-pattern is pinned textually in
        // `decide_approval_side_effect_ordering_is_pinned`'s negative assert.
        let (pending, snapshot) = remove_fenced(&session, "ap-r1").expect("second fenced removal");
        record(&session, sample("ap-r2", "k-r2", "g-r2"), None);
        assert!(
            !restore_if_generation_is(&session, pending, snapshot),
            "a generation advanced after the snapshot must cancel the restore"
        );
        let live = list(&session);
        assert_eq!(live.len(), 1, "only the interleaved card stays");
        assert_eq!(live[0].approval_id, "ap-r2", "no stale resurrection");
    }

    #[test]
    fn forkguard_approval_surface_grant_and_denial_scoping() {
        let _registry = test_registry_lock();
        let session = isolate("approval-scope-test");
        let other = isolate("approval-scope-other");
        // Grants key on grouping_key: same-group variants pass, other groups stay unaffected.
        assert!(!session_granted(&session, "g-write"));
        assert!(remember_grant(&session, "g-write", None));
        assert!(session_granted(&session, "g-write"));
        assert!(
            !session_granted(&session, "g-rm"),
            "未授予的 grouping key 不放行"
        );

        // Session dimension: this session's grants/denials must not take effect across sessions ("allow for this session" ≠ process-wide allow).
        assert!(
            !session_granted(&other, "g-write"),
            "A 会话的授予不得放行 B 会话"
        );
        assert!(
            !session_denied(&other, "k-rm-rf"),
            "A 会话的拒绝不得连坐 B 会话"
        );

        // Denials key on the exact approval_key: the exact arguments hard-deny, other arguments ask again.
        assert!(!session_denied(&session, "k-rm-rf"));
        assert!(remember_denial(&session, "k-rm-rf", None));
        assert!(session_denied(&session, "k-rm-rf"));
        assert!(
            !session_denied(&session, "k-rm"),
            "exact key 之外的调用不受拒绝连坐"
        );

        // Grants and denials never bleed into each other's scope.
        assert!(!session_denied(&session, "g-write"));
        assert!(!session_granted(&session, "k-rm-rf"));
    }

    #[test]
    fn forkguard_approval_surface_clear_pending_keeps_session_memory() {
        let _registry = test_registry_lock();
        let session = isolate("approval-clear-pending-test");
        record(&session, sample("ap-1", "k-1", "g-1"), None);
        remember_grant(&session, "g-1", None);
        remember_denial(&session, "k-1", None);

        // The turn terminal state clears only pending cards; grants/denials are session-level memory that survives across turns.
        clear_pending(&session);
        assert!(list(&session).is_empty(), "clear_pending 清挂起卡");
        assert!(session_granted(&session, "g-1"), "授予跨 turn 存活");
        assert!(session_denied(&session, "k-1"), "拒绝跨 turn 存活");

        // Only session deletion clears session-level memory.
        clear_session(&session);
        assert!(!session_granted(&session, "g-1"), "会话删除清授予");
        assert!(!session_denied(&session, "k-1"), "会话删除清拒绝");
    }

    /// Terminal split sweep: detached-worker approval ids (with the base
    /// `agent:<agent_id>:approval:<boot_id>:<n>` shape) are pulled out and returned to the caller for
    /// deterministic denial, while the parent session's own approval ids are simply dropped; both entry kinds leave the registry.
    /// Silently dropping a worker approval = permanently stranding a `WaitingForUser` worker.
    #[test]
    fn forkguard_approval_surface_terminal_split_returns_child_ids() {
        let _registry = test_registry_lock();
        let session = isolate("approval-terminal-split-test");
        record(&session, sample("ap-parent-1", "k-1", "g-1"), None);
        let mut child = sample("agent:ag-1:approval:boot-1:1", "k-2", "g-2");
        child.intent_summary = None;
        record(&session, child, None);

        let child_ids = clear_pending_returning_child_ids(&session);
        assert_eq!(
            child_ids,
            vec!["agent:ag-1:approval:boot-1:1".to_string()],
            "只有子代理审批 id 被摘出供确定性拒绝"
        );
        assert!(list(&session).is_empty(), "两类条目都离开登记");
        assert!(
            clear_pending_returning_child_ids(&session).is_empty(),
            "重复清空返回空"
        );

        // Session-level grant/denial memory is unaffected by the terminal sweep (same semantics as clear_pending).
        remember_grant(&session, "g-1", None);
        remember_denial(&session, "k-1", None);
        clear_pending_returning_child_ids(&session);
        assert!(session_granted(&session, "g-1"), "授予跨 turn 存活");
        assert!(session_denied(&session, "k-1"), "拒绝跨 turn 存活");
        clear_session(&session);
    }

    /// Terminal wiring pin: `emit_chat_terminal` must sweep the registry with the split variant and route the
    /// pulled-out subagent approvals to deterministic denial — falling back to a silent `clear_pending` would
    /// permanently strand `WaitingForUser` detached workers (worker approvals outlive the parent
    /// turn).
    #[test]
    fn forkguard_emit_chat_terminal_denies_child_approvals_after_terminal() {
        let _registry = test_registry_lock();
        let src = include_str!("engine.rs");
        let production = src.split("#[cfg(test)]").next().expect("non-empty source");
        let sweep = production
            .find("clear_pending_returning_child_ids")
            .expect("the terminal sweep must split out detached-worker approval ids");
        let denial_body_start = production
            .find("fn deny_child_approvals_after_terminal(")
            .expect("the deterministic child denial function must exist");
        // The routing pin must bound the window to before the fn definition: an unbounded tail window would be
        // fed by the definition's own name — after deleting the call arm at line 585 the pin still stayed green (round-15 review I1).
        assert!(
            production[sweep..denial_body_start].contains("deny_child_approvals_after_terminal("),
            "the split ids must be routed to the deterministic child denial"
        );
        let denial_body_end = production[denial_body_start + 1..]
            .find("\nfn ")
            .map(|offset| denial_body_start + 1 + offset)
            .unwrap_or(production.len());
        assert!(
            production[denial_body_start..denial_body_end].contains("deny_tool_call"),
            "the denial task must go through the engine deny route (oneshot resolution)"
        );
        assert!(
            !production.contains("pending_approvals::clear_pending("),
            "the terminal sweep must not drop pending entries silently"
        );
    }

    #[test]
    fn forkguard_approval_surface_clear_session_wipes_memory() {
        let _registry = test_registry_lock();
        let session = isolate("approval-clear-test");
        let other = isolate("approval-clear-other");
        record(&session, sample("ap-1", "k-1", "g-1"), None);
        remember_grant(&session, "g-1", None);
        remember_grant(&other, "g-1", None);
        remember_denial(&session, "k-1", None);

        clear_session(&session);
        assert!(list(&session).is_empty(), "clear_session 清挂起");
        assert!(!session_denied(&session, "k-1"), "clear_session 清拒绝缓存");
        // g-1 was also granted by the other session: only this session's share is removed, never the other's grant.
        assert!(session_granted(&other, "g-1"), "他人授予不受本会话清理影响");
        assert!(!session_granted(&session, "g-1"), "本会话授予被摘除");
        clear_session(&other);
        assert!(
            !session_granted(&other, "g-1"),
            "最后一个授予会话清理后撤销"
        );
    }

    /// Core ordering (named by review JensenChen28 5764675531 item 1; mechanism re-landed on this line from the
    /// reference round-3 P2 shape): decision writes carry the workspace generation captured **at request
    /// creation**, and a write whose generation ≠ the currently committed one is atomically refused in the store
    /// lock and dropped whole. Source-reachable interleave: click in directory A → approve ack committed → IPC
    /// future ready but not yet resumed → rebind gate idle → bind moves to B (the bind-commit point advances the
    /// generation + preserve-generation cleanup) → the old success arm resumes — a write at that moment that
    /// skips the origin-generation check would put directory A's decision into B's memory. Mutation (negative-first): removing the
    /// remember_* comparison arm turns the core assert red; removing advance (generation not advanced) turns the "late write refused" assert red.
    #[test]
    fn forkguard_late_old_scope_decision_is_rejected_after_rebind() {
        let _registry = test_registry_lock();
        let session = isolate("approval-req-gen-late");
        // The card is raised in directory A (no generation): the instance's spawn-generation snapshot is None.
        record(&session, sample("ap-late", "k-late", "g-late"), None);
        let origin = list(&session)[0].request_workspace_generation;
        assert_eq!(origin, None, "A 时代的请求卡携带无代");

        // The rebind bind-commit point: advance the generation + preserve-generation cleanup (the B-directory era begins).
        assert_eq!(advance_workspace_generation(&session), 1);
        clear_session_preserving_generation(&session);

        // The old success callback lands late: carrying the request-time generation, it must be refused whole —
        // it must not stamp the new (B) generation.
        assert!(
            !remember_grant(&session, "g-late", origin),
            "旧范围迟交授予必须被拒（重新请批，不静默记到新代名下）"
        );
        assert!(
            !remember_denial(&session, "k-late", origin),
            "旧范围迟交拒绝必须被拒（不跨目录连坐新目录）"
        );
        // New-scope lookups: B's same-family requests must not hit A-era late decisions.
        assert!(
            !session_granted(&session, "g-late"),
            "B 的同族请求不得命中旧范围的迟交授予"
        );
        assert!(
            !session_denied(&session, "k-late"),
            "B 的同族请求不得命中旧范围的迟交拒绝"
        );

        // Negative control: requests created by a new instance (B, spawned at generation 1) write and hit as usual —
        // record stamps the caller-passed instance generation, and the new instance carries the advanced generation.
        record(&session, sample("ap-b", "k-late", "g-late"), Some(1));
        let origin_b = list(&session)
            .iter()
            .find(|entry| entry.approval_id == "ap-b")
            .map(|entry| entry.request_workspace_generation)
            .flatten();
        assert_eq!(origin_b, Some(1), "record 盖章调用方传入的实例代");
        assert!(remember_grant(&session, "g-late", origin_b));
        assert!(session_granted(&session, "g-late"), "当前代决策照常命中");
        // After another generation advance (the next rebind) + preserve-generation cleanup, that decision is invalid
        // too — validity is always bound to the version; and the generation must be monotonic (no re-arming after a clear, see the identity-switch test).
        assert_eq!(advance_workspace_generation(&session), 2);
        clear_session_preserving_generation(&session);
        assert!(
            !session_granted(&session, "g-late"),
            "版本前进 + 份额回收后决策失效"
        );
        assert!(
            !remember_grant(&session, "g-late", Some(1)),
            "推进后旧代（1）的写入被拒：代单调，不得重臂"
        );
        clear_session(&session);
    }

    /// Pending cards are stamped with the **emitting instance's** spawn generation, not the current generation at
    /// card-receive time (review 5764675531, round-4 P2 sub-gap a sourcing). A turn started past the entry
    /// busy-gate during a rebind still runs on the old cwd runtime: if a card popped by the old instance after the
    /// bind-commit-point generation advance were stamped with the receive-time current generation, a user approval
    /// would write the old directory's decision into new-generation memory. With the registered generation =
    /// the instance spawn generation, decision writes compare against the registered one, so the old instance's
    /// cards are refused whole under the new generation (fail-safe re-ask) while the new instance's (new cwd)
    /// cards land as usual; the decision failure arm's restore (remove_fenced → restore_if_generation_is) brings the registered generation back as-is, so a
    /// restored card's next decision is still validated against the instance generation (covering the same source
    /// as post-partial-failure recovery). Mutation (negative-first): making record ignore instance_generation and
    /// stamp the receive-time current generation instead turns the first pair and the restore-pair asserts red (the forwarder-side snapshot wiring has its own source pin).
    #[test]
    fn forkguard_pending_card_stamps_emitting_instance_generation() {
        let _registry = test_registry_lock();
        let session = isolate("approval-inst-gen");
        // The old instance (spawned in directory A, no generation) raises its card only after the bind-commit-point
        // generation advance: the registration carries the instance's spawn generation (None), not the receive-time current generation.
        assert_eq!(advance_workspace_generation(&session), 1);
        record(&session, sample("ap-old", "k-old", "g-old"), None);
        let stamped = list(&session)[0].request_workspace_generation;
        assert_eq!(
            stamped, None,
            "旧实例的卡必须携带实例的 spawn 代（None），不得按收卡当前代盖章"
        );
        // The user approves the card: the decision write carries the registered generation, unequal to the current
        // one → atomic refusal; the old directory's decision must not be booked under the new generation.
        assert!(
            !remember_grant(&session, "g-old", stamped),
            "旧实例跨推进弹卡的批准必须被拒（重新请批，不静默记到新代名下）"
        );
        assert!(
            !remember_denial(&session, "k-old", stamped),
            "旧实例跨推进弹卡的拒绝同样被拒"
        );
        assert!(
            !session_granted(&session, "g-old") && !session_denied(&session, "k-old"),
            "被拒的旧范围写入不得留下任何记忆"
        );

        // post-partial-failure recovery: the decision's failed engine-call arm puts the removed registration back
        // as-is (remove_fenced → restore_if_generation_is). The restored
        // card must keep the instance generation, and its next decision is still validated against it.
        let (pending, snapshot) = remove_fenced(&session, "ap-old").expect("旧实例的卡仍在登记表");
        assert_eq!(
            pending.request_workspace_generation, None,
            "恢复携带必须原样带回实例代"
        );
        assert!(
            restore_if_generation_is(&session, pending, snapshot),
            "无并发变更时恢复照常落回"
        );
        let restored = list(&session)[0].request_workspace_generation;
        assert_eq!(restored, None, "恢复后的卡保留实例代");
        assert!(
            !remember_grant(&session, "g-old", restored),
            "恢复后的卡再次决策仍按实例代校验（旧实例代仍被拒）"
        );
        clear_session(&session);
    }

    /// A same-approval_id replay (in-place replace) stamps the instance generation of the **latest** occurrence:
    /// an engine replaying the same approval follows new-request semantics and takes the latest instance
    /// generation, never keeping the old value. A same-instance replay = the same generation; engine approval ids
    /// are minted per instance, so a rebuilt instance never replays old ids, and a new instance's replay carries its own spawn generation (consistent with the instance sourcing semantics).
    #[test]
    fn forkguard_record_replace_recaptures_request_generation() {
        let _registry = test_registry_lock();
        let session = isolate("approval-req-gen-recapture");
        record(&session, sample("ap-re", "k-re", "g-re"), None);
        assert_eq!(list(&session)[0].request_workspace_generation, None);
        advance_workspace_generation(&session);
        let mut replay = sample("ap-re", "k-re", "g-re");
        replay.intent_summary = None;
        record(&session, replay, Some(1));
        let entry = &list(&session)[0];
        assert_eq!(
            entry.request_workspace_generation,
            Some(1),
            "重放按最新一次的实例代盖章"
        );
        assert!(entry.intent_summary.is_none(), "其余字段仍以最新一次为准");
        clear_session(&session);
    }

    /// Cold-start identity snapshot vs bind-commit-point advance, a **real interleave** (reviews JensenChen28
    /// 5766477612 and zhuowp 5766212059/5766526920, P2-b): the instance identity must be fixed at
    /// **startup capture** (the AppEngine spawn-chain synchronous section, the same point that pins
    /// roots/workspace — source pins on the engine/forwarder sides hold that wiring). This test drives
    /// "startup capture → rebind bind-commit point `advance_workspace_generation` →
    /// forwarder first execution mints the card": the startup step reads through the same production capture
    /// function `workspace_generation` (no hand-fabricated old generation) and asserts the card carries the
    /// **startup-captured** identity generation — the defective shape (a live read on the async body's first
    /// execution) would stamp the card with the advanced new generation, pass the decision comparison, and write
    /// directory A's grants/denials into B; it then asserts old-generation decision writes are refused. Negative control: a B instance cold-started after the
    /// advance obtains the new generation through the same capture function and its card lands and hits as usual. Mutation (negative-first): moving the capture back to a
    /// forwarder async-body live read turns both source pins red (this test locks the store-side
    /// semantics); making record stamp the receive-time current generation turns the first pair red.
    #[test]
    fn forkguard_coldstart_identity_survives_rebind_before_first_mint() {
        let _registry = test_registry_lock();
        let session = isolate("approval-coldstart-gen");
        // Startup (directory-A cold start): the spawn-chain synchronous section snapshots the identity generation
        // through the production capture function (never rebound → None).
        let startup_captured = workspace_generation(&session);
        assert_eq!(startup_captured, None, "A 纪元冷启动实例身份代为 None");

        // Rebind bind-commit point: advance the generation (the B era begins).
        assert_eq!(advance_workspace_generation(&session), 1);

        // Forwarder first execution mints: the card registration carries the startup identity generation passed at
        // spawn, not the receive-time current generation — the defective live-read shape would carry Some(1) and A's decision would follow into B.
        record(&session, sample("ap-a", "k-a", "g-a"), startup_captured);
        let stamped = list(&session)[0].request_workspace_generation;
        assert_eq!(
            stamped, startup_captured,
            "跨推进首次 mint 的卡必须携带启动捕获的身份代"
        );
        assert_ne!(
            stamped,
            Some(1),
            "A 实例的卡不得带推进后的 B 代：带 B 代即通过决策比较写入 B"
        );

        // User decision on the A-instance card: startup old generation vs committed new generation → atomic
        // refusal; nothing stamps the new generation and nothing lands in memory.
        assert!(
            !remember_grant(&session, "g-a", stamped),
            "A 实例（启动代 None）跨推进的批准必须被拒（重新请批）"
        );
        assert!(
            !remember_denial(&session, "k-a", stamped),
            "A 实例跨推进的拒绝必须被拒（不跨目录连坐）"
        );
        assert!(
            !session_granted(&session, "g-a") && !session_denied(&session, "k-a"),
            "被拒的旧身份写入不得留下任何记忆"
        );

        // Negative control: after the rebind, a new sender cold-starts in directory B and the same capture function
        // reads the advanced generation live; its card lands and hits as usual (the advance does not hurt the new era).
        let startup_captured_b = workspace_generation(&session);
        assert_eq!(
            startup_captured_b,
            Some(1),
            "B 纪元冷启动实例经同一捕获函数取得推进后的代"
        );
        record(&session, sample("ap-b", "k-b", "g-b"), startup_captured_b);
        assert!(
            remember_grant(&session, "g-b", startup_captured_b),
            "B 实例（当前代）的批准照常落笔"
        );
        assert!(session_granted(&session, "g-b"), "当前代决策照常命中");
        clear_session(&session);
    }

    /// Capture protocol (review 5768924092 / 5768948485 P2-a round 2) — no concurrency: one capture round
    /// succeeds, returning (roots == expected, current gen_b). The engine side does not take the return value for
    /// the workspace it actually feeds into config (accepted only when equal to expected);
    /// gen_b goes to the forwarder at spawn for stamping.
    #[test]
    fn forkguard_capture_identity_happy_path_single_round() {
        let _registry = test_registry_lock();
        let session = isolate("capture-happy");
        let expected = "A-root".to_string();
        let resolves = std::cell::Cell::new(0usize);
        let (roots, generation) = capture_workspace_identity(&session, &expected, || {
            resolves.set(resolves.get() + 1);
            expected.clone()
        })
        .expect("无并发时一次捕获必须成功");
        assert_eq!(roots, expected, "接受的 roots 必须与 expected 一致");
        assert_eq!(generation, None, "从未重绑的会话身份代为 None");
        assert_eq!(resolves.get(), 1, "无并发时协议只走一轮");
        clear_session(&session);
    }

    /// Single identity boundary (review zhuowp 5780927743 P2) — the **top regression**: a fresh spawn capture
    /// started while a rebind publication is in flight (the bind write has begun, advance+clear not yet landed)
    /// must not observe the intermediate state. The previous two-step commit let captures through in that window
    /// (new root + old generation consistent, and the generation-blind grant lookup could reuse old-directory
    /// grants); after fencing, capture and publication hold the same store lock, so a capture can only wait while
    /// publication blocks — once publication completes the capture necessarily sees the **full post-commit state**:
    /// new root == expected, generation advanced, old-directory grants cleared. Mutation (red): moving the
    /// advance+clear out of the fence (or deleting the cleanup) turns this test's "capture incomplete while publication blocks" probe or the grants-cleared assert red.
    #[test]
    fn forkguard_capture_during_publication_waits_and_sees_full_post_commit_state() {
        use std::sync::mpsc;
        let _registry = test_registry_lock();
        let session = isolate("capture-fence-post");
        remember_grant(&session, "old-root-grant", None);
        assert!(session_granted(&session, "old-root-grant"));
        let new_root = "B-root".to_string();
        let (publish_started, start_rx) = mpsc::channel::<()>();
        let (release_publish, release_rx) = mpsc::channel::<()>();
        let (fence_done, fence_done_rx) = mpsc::channel::<()>();
        let sid = session.clone();
        let fence = std::thread::spawn(move || {
            commit_rebind_publication(
                || {
                    publish_started.send(()).expect("signal publish start");
                    // Simulating the window where the bind write takes time: advance+clear not yet landed.
                    release_rx.recv().expect("release");
                },
                move |()| vec![sid],
            );
            fence_done.send(()).expect("signal fence done");
        });
        start_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("publish started");
        let (capture_done, capture_done_rx) =
            mpsc::channel::<Result<(String, Option<u64>), String>>();
        let sid = session.clone();
        let expected = new_root.clone();
        let capture = std::thread::spawn(move || {
            let outcome = capture_workspace_identity(&sid, &expected, || expected.clone());
            capture_done.send(outcome).expect("signal capture done");
        });
        // Primary probe: the capture must not complete inside the publication window (pre-fix this let through the
        // "new root + old generation + grants still present" intermediate state; still incomplete after the probe window = held by the fence).
        assert!(
            capture_done_rx
                .recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "capture must not complete while the rebind publication holds the store lock"
        );
        release_publish.send(()).expect("release publish");
        fence.join().expect("fence thread");
        fence_done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("fence completed");
        let (roots, generation) = capture_done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("capture completes after the fence releases")
            .expect("post-commit capture must accept the new roots");
        capture.join().expect("capture thread");
        assert_eq!(
            roots, new_root,
            "post-commit capture resolves the new roots"
        );
        assert_eq!(
            generation,
            Some(1),
            "post-commit capture stamps the advanced generation, not the stale one"
        );
        assert!(
            !session_granted(&session, "old-root-grant"),
            "the old-root grant must be gone once capture can observe the new roots (no stale grant reuse)"
        );
        clear_session(&session);
    }

    /// The publication fence also advances the **global** restore generation (the one counter snapshotted by
    /// `remove_fenced` and compared by `restore_if_generation_is`). round-13 review: the other tests only assert
    /// the per-session `workspace_generations`, so deleting the global bump inside the fence stayed green —
    /// this pin holds "the rebind commit point invalidates in-flight card restore handles":
    /// a restore handle obtained from removing a card must mismatch and be refused after the rebind publication
    /// (a late failure-arm restore must not reinsert an old-directory-era card).
    #[test]
    fn forkguard_publication_bumps_the_global_restore_generation() {
        let _registry = test_registry_lock();
        let session = isolate("rebind-global-gen");
        record(
            &session,
            sample("global-gen-card", "k-global-gen", "g-global-gen"),
            None,
        );
        let (removed, snapshot_generation) = remove_fenced(&session, "global-gen-card")
            .expect("the card must be removed with a restore handle");
        commit_rebind_publication(|| (), |_| vec![session.clone()]);
        assert!(
            !restore_if_generation_is(&session, removed, snapshot_generation),
            "a pre-rebind card-removal restore handle must be refused after the publication \
             bump (the failure arm must not resurrect the old-directory card)"
        );
        clear_session(&session);
    }

    /// The other side of the single identity boundary: the capture takes the lock first (a blocking resolve
    /// simulates a slow SessionStore read path), so the publication fence can only wait — once the capture
    /// completes it necessarily sees the **full pre-commit state** (old root + old generation + grants present),
    /// and the publication lands and cleans up afterwards. Together both directions pin "the intermediate state is unobservable".
    #[test]
    fn forkguard_capture_before_publication_sees_full_pre_commit_state() {
        use std::sync::mpsc;
        let _registry = test_registry_lock();
        let session = isolate("capture-fence-pre");
        remember_grant(&session, "pre-root-grant", None);
        assert!(
            session_granted(&session, "pre-root-grant"),
            "seed grant must be live"
        );
        let old_root = "A-root".to_string();
        let (resolve_started, start_rx) = mpsc::channel::<()>();
        let (release_resolve, release_rx) = mpsc::channel::<()>();
        let (capture_done, capture_done_rx) =
            mpsc::channel::<Result<(String, Option<u64>), String>>();
        let sid = session.clone();
        let expected = old_root.clone();
        let capture = std::thread::spawn(move || {
            let outcome = capture_workspace_identity(&sid, &expected, || {
                resolve_started.send(()).expect("signal resolve start");
                release_rx.recv().expect("release");
                expected.clone()
            });
            capture_done.send(outcome).expect("signal capture done");
        });
        start_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("resolve started");
        let (fence_done, fence_done_rx) = mpsc::channel::<()>();
        let sid = session.clone();
        let fence = std::thread::spawn(move || {
            commit_rebind_publication(|| (), move |()| vec![sid]);
            fence_done.send(()).expect("signal fence done");
        });
        assert!(
            fence_done_rx
                .recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "the publication fence must wait for an in-flight capture to release the store lock"
        );
        release_resolve.send(()).expect("release resolve");
        let (roots, generation) = capture_done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("capture completes")
            .expect("pre-commit capture must accept the old roots");
        capture.join().expect("capture thread");
        assert_eq!(roots, old_root, "pre-commit capture resolves the old roots");
        assert_eq!(
            generation, None,
            "pre-commit capture stamps the pre-commit generation"
        );
        // Note: an inherent race exists between capture completion and the asserts — the fence acquires the lock
        // the instant the capture releases it and clears the grants, so a main-thread re-read of the grants may see
        // the post-cleanup state and cannot seamlessly observe "pre-commit grants present". The pre-state with the
        // grant alive is anchored jointly by the seeded assert above and this test's (old root, None generation)
        // capture result; "grants are cleared in the same critical section as publication" is pinned by the post-commit case's full set of asserts.
        fence.join().expect("fence thread");
        fence_done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("fence completes after capture releases");
        assert!(
            !session_granted(&session, "pre-root-grant"),
            "the fence clears the seeded grant once it proceeds (post-commit state)"
        );
        clear_session(&session);
    }

    /// Capture protocol — **stale expected** (the rebind commit lands entirely between the engine side's early
    /// roots resolution and this capture: expected is already the old value while the capture can only read the
    /// new state — the single critical section leaves no in-window tearing, so this arm refuses the accomplished
    /// drift of "the commit precedes the capture but engine_config.workspace is still the old root"): retries
    /// exhausted → Err, refusing this spawn fail-closed — the error wording explains that a consistent workspace
    /// identity cannot be captured while the rebind is in flight. Mutation: deleting the Err arm of the retry cap
    /// (returning as usual after exhaustion) turns this test red.
    #[test]
    fn forkguard_capture_identity_exhausts_under_persistent_advance() {
        let _registry = test_registry_lock();
        let session = isolate("capture-persistent-advance");
        let expected = "A-root".to_string();
        let error = capture_workspace_identity(&session, &expected, || "B-root".to_string())
            .expect_err("持续漂移必须耗尽重试并 Err（fail-closed 拒绝 spawn）");
        assert!(
            error.contains("rebind is in flight"),
            "报错必须说明重绑进行中无法一致捕获身份：{error}"
        );
        clear_session(&session);
    }

    /// Capture protocol — the **roots-drift retry mechanism** (checking roots against the engine-side resolved
    /// value and invalidating the whole round with a retry): one drifting round then back to expected → detected
    /// + retry succeeds; persistent drift → exhausted → Err. Under a single critical section, in-window tearing is
    /// no longer constructible (fence mutual exclusion; see the two serialization cases above), so this test pins
    /// the comparison arm and the retry mechanism itself. Mutation: deleting the roots comparison arm turns both asserts red.
    #[test]
    fn forkguard_capture_identity_rejects_roots_drift() {
        let _registry = test_registry_lock();
        let session = isolate("capture-roots-drift");
        let expected = "A-root".to_string();
        // One drifting round: resolve returns roots that mismatch expected, the next round returns to expected
        // → the retry succeeds (the retry mechanism neither deadlocks nor gives up early).
        let first_round = std::cell::Cell::new(true);
        let resolves = std::cell::Cell::new(0usize);
        let (roots, generation) = capture_workspace_identity(&session, &expected, || {
            resolves.set(resolves.get() + 1);
            if first_round.get() {
                first_round.set(false);
                "B-root".to_string()
            } else {
                expected.clone()
            }
        })
        .expect("漂移轮检出后，回到 expected 的重试轮必须成功");
        assert_eq!(roots, expected);
        assert_eq!(generation, None, "漂移轮代未推进，接受轮读得的仍是原代");
        assert_eq!(
            resolves.get(),
            2,
            "roots 漂移必须按撕裂窗口整轮作废重试，不得静默接受首轮"
        );

        // Persistent drift: the rebind lands entirely before the capture window, so the new root never equals the
        // engine side's old expected (engine_config.workspace is still the old root then; letting it through would
        // be "directory-A runtime + B era" itself) → exhausted → Err.
        let error = capture_workspace_identity(&session, &expected, || "B-root".to_string())
            .expect_err("持续 roots 漂移必须耗尽重试并 Err");
        assert!(
            error.contains("rebind is in flight"),
            "耗尽报错与代撕裂同口径：{error}"
        );
        clear_session(&session);
    }

    /// The only difference between identity-switch cleanup and deletion cleanup (review 5764675531 item 1):
    /// `clear_session_preserving_generation` reclaims pending cards, grants, and denials but **preserves the
    /// workspace generation** — if the generation were cleared too, the session would fall back to "no generation"
    /// and old decisions stamped None before the bind-commit-point advance would re-equal the new state (old
    /// grants revive, late old-scope writes get accepted again). Preserving the generation keeps old generations
    /// missing forever while decisions under the new directory land and hit as usual; another session's grants are
    /// unaffected; the later real `clear_session` (session deletion) still does the full reset. Mutation
    /// (negative-first): swapping the bind-commit-point cleanup back to
    /// `clear_session` turns the final pair red (projects.rs's command-layer tests reproduce the same shape from the rebind entry).
    #[test]
    fn forkguard_identity_switch_cleanup_preserves_workspace_generation() {
        let _registry = test_registry_lock();
        let session = isolate("approval-ws-gen-preserve");
        let other = isolate("approval-ws-gen-preserve-other");
        // Pre-rebind decisions in directory A (no generation).
        assert!(remember_grant(&session, "g-keep", None));
        assert!(remember_denial(&session, "k-keep", None));
        assert!(remember_grant(&other, "g-keep", None));
        assert!(session_granted(&session, "g-keep"));

        // The bind-commit point advances the generation (directory B) and old decisions miss (write-side defense:
        // late None-generation writes are refused, never stamping the new generation).
        assert_eq!(advance_workspace_generation(&session), 1);
        assert!(!remember_grant(&session, "g-keep", None));
        assert!(!remember_denial(&session, "k-keep", None));

        // Identity-switch cleanup at the bind-commit point: reclaim entries, preserve the generation.
        clear_session_preserving_generation(&session);
        assert!(list(&session).is_empty(), "挂起卡仍被回收");
        assert!(
            !session_granted(&session, "g-keep"),
            "本会话旧授予已被清理摘除，不得复活"
        );
        assert!(
            !session_denied(&session, "k-keep"),
            "本会话旧拒绝已被清理摘除"
        );
        assert!(
            session_granted(&other, "g-keep"),
            "他人授予不受本会话清理影响"
        );
        // The generation survives: late None-generation (A-era) writes are refused, never stamping the new generation.
        assert!(
            !remember_grant(&session, "g-keep", None),
            "迟交的旧范围授予必须被拒（代未回落到无代）"
        );
        assert!(!session_granted(&session, "g-keep"));
        // Decisions under the new directory (generation 1) land and hit as usual.
        assert!(remember_grant(&session, "g-keep", Some(1)));
        assert!(session_granted(&session, "g-keep"));
        // A real session deletion is still the full reset: a same-id session recreated after deletion starts again
        // from no generation (no regression of the deletion semantics).
        clear_session(&session);
        assert!(
            remember_grant(&session, "g-keep", None),
            "删除清理后同 id 会话从无代重新起步"
        );
        assert!(session_granted(&session, "g-keep"));
        clear_session(&session);
        clear_session(&other);
    }

    /// Disposition truth table (the scheduled unattended path is not this function; it short-circuits elsewhere).
    /// Columns: mode | ask | force | granted | denied → disposition.
    #[test]
    fn forkguard_approval_surface_decision_truth_table() {
        let _registry = test_registry_lock();
        use ApprovalDenyReason::*;
        use ApprovalSurfaceDecision::*;
        let table: &[(
            SerializableMode,
            bool,
            bool,
            bool,
            bool,
            ApprovalSurfaceDecision,
        )] = &[
            // The denial cache has top priority, overriding grants and every stance.
            (
                SerializableMode::Auto,
                true,
                false,
                true,
                true,
                Deny(SessionDenied),
            ),
            (
                SerializableMode::Yolo,
                true,
                true,
                false,
                true,
                Deny(SessionDenied),
            ),
            // Grant + non-force: silent approval (see the Plan exceptions in the next two rows).
            (
                SerializableMode::Auto,
                true,
                false,
                true,
                false,
                ApproveSilently,
            ),
            (
                SerializableMode::Yolo,
                false,
                false,
                true,
                false,
                ApproveSilently,
            ),
            // The Plan defensive arm precedes the grant cache (deliberately stricter than the TUI legacy order):
            // after a mid-turn switch to Plan, an old grant must not silently allow.
            (
                SerializableMode::Plan,
                true,
                false,
                true,
                false,
                Deny(PlanMode),
            ),
            (
                SerializableMode::Plan,
                false,
                false,
                true,
                false,
                Deny(PlanMode),
            ),
            // A force_prompt hold is not served by the cache.
            (SerializableMode::Auto, true, true, true, false, ShowCard),
            // Plan defensive hard-deny (the base should have blocked tool execution), force_prompt included.
            (
                SerializableMode::Plan,
                true,
                false,
                false,
                false,
                Deny(PlanMode),
            ),
            (
                SerializableMode::Plan,
                false,
                true,
                false,
                false,
                Deny(PlanMode),
            ),
            // force_prompt always raises a card; Yolo with toggle off still does not hard-deny.
            (SerializableMode::Yolo, false, true, false, false, ShowCard),
            // Yolo + toggle off + non-force: a guardrail hit hard-denies per the stance.
            (
                SerializableMode::Yolo,
                false,
                false,
                false,
                false,
                Deny(GuardrailAskOff),
            ),
            // Yolo + toggle on: raise a card.
            (SerializableMode::Yolo, true, false, false, false, ShowCard),
            // Auto: ordinary promptable tools ask each time, the toggle only governs guardrail hits — always a card.
            (SerializableMode::Auto, true, false, false, false, ShowCard),
            (SerializableMode::Auto, false, false, false, false, ShowCard),
            (SerializableMode::Auto, false, true, false, false, ShowCard),
        ];
        for (mode, ask, force, granted, denied, expected) in table {
            assert_eq!(
                approval_surface_decision(*mode, *ask, *force, *granted, *denied),
                *expected,
                "mode={mode:?} ask_on_guardrail={ask} force_prompt={force} granted={granted} denied={denied}"
            );
        }
    }
}
