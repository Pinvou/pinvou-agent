# CodeWhale Fork Modification Register

> Updated: 2026-08-17. Canonical Chinese register: [`docs/fork-modifications.md`](fork-modifications.md).

## Current baseline

| Item | Value |
|---|---|
| Upstream | `v0.9.5` at `853cb707bbcf4f7dc4268fba6d811e0d04083f9c` |
| Public maintenance branch | `Pinvou/CodeWhale:pinvou3-clean` at `a36e6cd53` (`pinvou-v0.9.5-r7`) |
| Merged fixes | `Pinvou/CodeWhale#9`, `#11`, `#12`, and `#13` are merged |
| Published status | `pinvou3-clean`, `pinvou-v0.9.5-r7`, and the parent gitlink resolve to `a36e6cd533024cfe5724bae21875aea42b2ed87a`; `r1` through `r7` remain immutable historical tags |
| Previous baseline backup | Tag `pinvou-v0.9.0-r4` and branch `backup/pinvou3-clean-v0.9.0-r4`, both at `03e9e1027c03ce1e4b35ab9e3ccce751b65b9624` |
| Drift | Published r7 baseline: 46 files, `+1852/-269` |
| Organization | Four current long-lived topics; PR #13 removes the product-specific orchestration topic |

### Published session fix

- v0.9.5 `load_session` treats an unmatched `tool_use` as evidence of a crashed process. That assumption is invalid when Pinvou persists a live tool call and reads the same session again during the turn.
- The engine fix was merged through `Pinvou/CodeWhale#11`; its public commit is `2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd`.
- T1 now separates side-effect-free `load_session_snapshot` from explicit `recover_session_for_resume`. Pinvou uses snapshots for all runtime read-modify-write paths and performs durable recovery only during app process startup, before any Engine can own a session.
- Revision reconciliation remains fail-closed only for genuine cross-client turns. A local `chat:done` immediately releases the next send, readback failures cannot block ordinary local chat, and cross-client pending notices are deduplicated per session.
- Two CodeWhale tests, two parent `forkguard_*` tests, and Tauri/Web frontend behavior coverage protect side-effect-free runtime reads, observable and idempotent explicit recovery, safe secondary Store opening, durable startup recovery, and consecutive sends after local completion.
- The fix is included in the published head, drift figures, and immutable tag `pinvou-v0.9.5-r5`; CodeWhale required checks and parent automation pass.

## In flight: session steer and deterministic cancel (CodeWhale#16 / pinvou-agent#308)

- **Status**: review fixes (opaque steer id, stop semantics, teardown coverage, scoped kill) are complete, pending the CodeWhale#16 merge and release. Once published, this section folds into topics T1/T2, the public baseline head, and the drift figures.
- **T1 (host embedding and routing boundary) additions**:
  - Session steer (mid-turn injection) primitives: `SteerMessage { id, content }` travels the steer channel; `EngineHandle::steer` assigns and returns an opaque `steer_id` at enqueue time; `Event::SteerCommitted` / `Event::SteerDropped` carry `steer_id` so the host can correlate its queued placeholder message — no content hash (a cross-language hash over non-ASCII content cannot be made consistent).
  - `steer_keep_inbox` switch (set by the host before cancel): interrupt (true) parks unconsumed steers for the next turn's step boundary; stop (false) settles at every Interrupted exit — both `pending_steers` and steer-channel residue emit one `SteerDropped` each.
  - `Op::SyncSession` and `Op::Shutdown` drain queued steers with per-message `SteerDropped` events, preventing cross-session injection; `Drop for Engine` is a best-effort `try_send` backstop for host evict/reclaim paths that drop the engine directly.
  - Steer withdrawal: `EngineHandle::withdraw_steer` (fire-and-forget) records the id in a shared withdrawn set; every collection/injection point filters withdrawn ids and emits exactly one `SteerDropped`, so a withdrawn steer is never injected into the transcript. Marks survive across turns and are cleared on `SyncSession`/`Shutdown`. This makes the host UI's queued-placeholder ✕ effective until the moment of injection.
- **T2 (tool compatibility and command-execution safety) addition**:
  - Cancel-time process kills are scoped: `ShellManager::kill_running_turn_foreground` kills only this turn's foreground (`spawned_as_foreground`, no `owner_agent`) shell process groups; user-backgrounded tasks and sub-agent background shells survive any cancel source.
- **Guards**: thirteen new behavior tests — `engine_handle_steer_returns_unique_incrementing_ids`, `keep_inbox_true_cancel_parks_steer_and_next_turn_commits_it`, `keep_inbox_false_cancel_drops_parked_and_channel_steers`, `steer_enqueued_before_turn_is_committed_at_step_boundary`, `pending_steer_is_committed_at_no_tool_turn_end`, `pending_steer_is_committed_after_tool_execution`, `sync_session_drops_all_queued_steers_with_events`, `engine_drop_reports_unconsumed_steers_best_effort`, `withdrawn_channel_steer_is_dropped_not_injected_on_next_turn`, `withdrawn_pending_steer_is_dropped_not_injected`, `withdrawing_committed_steer_is_a_noop`, `withdraw_after_interrupt_park_prevents_reinjection`, `withdraw_leaves_other_steers_unaffected` (`core/engine/tests.rs`), and `kill_running_turn_foreground_scopes_to_this_turns_unowned_foreground_shells` (`tools/shell/tests.rs`); fingerprints are the T1/T2 steer entries in `scripts/fork-guard.sh`.
- **Upstream policy**: the capability is implemented upstream-neutrally (English comments, no Pinvou-private context) and should later be contributed from a clean branch off upstream main per the standing rules; this register is authoritative until that lands.

## Topics

PR #13 was squash-merged as `a36e6cd533024cfe5724bae21875aea42b2ed87a` and published as immutable tag `pinvou-v0.9.5-r7`. It removes product-specific orchestration while preserving canonical registry prompt text and alias-aware Custom SubAgent allowlist resolution.

1. **Host embedding and routing boundary** — `331cb1594688c723d98499d9ca11f05af291b599`, `2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd` (`Pinvou/CodeWhale#11`), and `a36e6cd533024cfe5724bae21875aea42b2ed87a` (`Pinvou/CodeWhale#13`). Exposes only the library modules, narrow root-level Fleet roster API, read-only live-worker projection, opaque resolved-route interfaces, distinct runtime-snapshot versus process-resume session APIs, generic host bulk cancellation, and terminal failure facts required by the host; the full `fleet` module remains private.
2. **Tool compatibility and command-execution safety** — `595adce47e2d1bcf895d7bfd6426c074eb969324`, `3bbf8421ebdb16bff71f83dac4d42c8fb65f0f02` (`Pinvou/CodeWhale#12`), and `a36e6cd533024cfe5724bae21875aea42b2ed87a` (`Pinvou/CodeWhale#13`). Adds host `extra_tools`, dynamic `SetDisallowedTools`, file-size enforcement, fail-closed multiline command safety, schema-bound JSON-container repair, provider-role-safe continuations, canonical registry instructions, and alias-aware Custom SubAgent allowlists while reusing upstream `allowed_tools`.
3. **Embedded context and Skill sources** — `5a9f52941b83452c1e8b76c2d679bac315edcf70`. Seals ambient project authority, scans only the explicit Skill root, filters disabled Skills, preserves up to 100 KiB only for the Permissions fragment, and excludes internal reminders from Working Set extraction.
4. **Automation and runtime lifecycle** — `fc84f7d3e5dca0e3db404d43e218597764129f9b`. Preserves stable conversation/thread identity, v4 task compatibility, anchored schedules, no-backfill/no-overlap behavior, and terminal-only cleanup.

Pinvou's product tool allowlist, connector state, UI, workspace selection, bundle instructions, session Skill materialization, and presentation remain in `pinvou3-app`.

## v0.9.5 migration notes

- The parent passes through the new `EngineConfig.subagent_state_root` field.
- The removed legacy `hidden_tools` field is not restored; session-level hiding already uses dynamic `disallowed_tools` shaping.
- The upstream 40 KiB WorldState cap is retained globally. Only `FragmentId::Permissions` uses the existing 100 KiB instruction limit.
- The parent lockfile reflects the v0.9.5 workspace-crate split without adding a new direct Pinvou dependency.

## Verification

- CodeWhale format and locked library check pass.
- All 23 CodeWhale `forkguard_*` tests pass for the published r7 baseline.
- Parent locked Rust check and desktop binary link pass.
- Parent library tests pass: 1220 passed, 0 failed, and 12 environment-dependent tests ignored.
- Parent fork guard, architecture guard, npm tests, UI lint, desktop UI build, and web UI build pass.
- Full product results are recorded in `docs/codewhale-upgrade-0.9.0-to-0.9.5.md`.

Any fork-distinct change must update this register, guard fingerprints, and a result-oriented behavior test, then pass `./scripts/fork-guard.sh --fast`.
