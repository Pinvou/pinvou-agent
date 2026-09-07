# CodeWhale Fork Modification Register

> This is the current-state register for Pinvou's CodeWhale fork.
> See [`fork-policy.md`](fork-policy.md) for policy and [`codewhale-upgrade-0.9.5-to-0.9.12.md`](codewhale-upgrade-0.9.5-to-0.9.12.md) for upgrade evidence. The Chinese register is authoritative.

## Current state (2026-09-07, v0.9.12 r1 local candidate)

| Item | Value |
|---|---|
| Upstream | `v0.9.12`, `dcd4c200f72f0c1ffd60d8e7f6850313db879fc5` |
| Local branch | `codex/pinvou-v0.9.12-r1`, head `09c3b85e366379dbbb69539e4cec8c2dbadd91f4` |
| Publication | Not published; no remote branch or immutable r1 tag is claimed |
| Rollback | `backup/pre-v0.9.12-sync` at v0.9.5 r13 `f853f8f1566c57e6be40d5439a222a932aa79ef5` |
| History | Five signed commits above upstream, grouped into four long-lived topics |
| Drift | 49 files, `+2993/-457` (net +2,536), down from r13's 110 files and `+10895/-1195` |
| Guard | 18 distinct CodeWhale `forkguard_*` behavior tests plus parent fingerprints/tests |

## Clean re-fork decision

The old r13 changed 110 files. Upstream v0.9.12 also changed 104 of those files, with 57 projected conflict files. Upstream had already absorbed or redesigned session recovery, edit-last-turn, post-compaction usage, route budgets, provider/model routing, native search, Windows UTF-8 shell handling, JSON schema repair, and much of the task substrate. The r1 branch therefore starts directly at the official v0.9.12 tag and re-expresses only behavior that remains necessary inside foundation lifecycle boundaries.

## Disposition of r13 behavior

| Disposition | Result |
|---|---|
| Drop fork patch, use upstream | Session snapshot/recovery, edit-last-turn, compaction tokens, strict-direct model matching, route budgets, schema normalization, native search, Windows shell decoding, and provider pin improvements |
| Semantic migration | `Yolo`/`Auto` become `Agent` plus approval/trust policy; reasoning is per `SendMessage`; host tools use `ExtraTools`; explicit Skill roots replace global disable shims |
| Rebuild in fork | Host facade and route limits, reliable steer, bulk child cancel, MCP secret resolver, raw worker ledger, per-turn/final-dispatch safety, 64 KiB File cap, prompt ownership, and Automation ownership/schema/lifecycle |
| Do not port | r13 benchmark-only foundation features and product search overrides; benchmark isolation remains in the parent `benchmark-hooks` surface |

## Commit sequence

| Commit | Topic |
|---|---|
| `38dd961ea` | T1 host facade, routing, and embedding boundary |
| `a5c12e203` | T2 tool compatibility, MCP secrets, and execution safety |
| `7dc1a429a` | T3 host-owned static prompt composition |
| `02c0faa27` | T4 Pinvou Automation and Task ownership |
| `09c3b85e3` | Cross-topic lifecycle, safety, host prompt-only profile, and explicit-Skill-root closure |

All commits carry DCO sign-off. Candidate SHAs may move for review fixes before publication; an immutable tag must never be rewritten.

## T1: host embedding and routing

- Exposes only the host APIs needed for modes, approval, Automation/Task, route resolution, and worker-ledger projection.
- Preserves `resolve_runtime_route_with_limits`, including wire model and embedding aliases.
- Installs `EngineConfig.session_id` before spawn and filters owner-bearing child/workflow events by session.
- Gives each steer an opaque id and exactly one committed/dropped terminal event across withdrawal, interruption, stop, compaction, session sync, and Engine drop.
- Provides idempotent, session-scoped `CancelSubAgents`.
- Keeps v0.9.12's closed role set as the executable posture while allowing only exact config-origin, prompt-only profiles explicitly injected by the host. Ambient personal/workspace/plugin profiles and route/authority pins fail closed.

Tests: `forkguard_embedding_route_limits_preserve_wire_alias`, `forkguard_steer_lifecycle_withdrawal_is_bounded_and_prevents_commit`, `forkguard_steer_lifecycle_late_withdraw_reconciles_committed_state`, and `forkguard_host_profile_overlay_is_config_only_and_prompt_only`.

## T2: tool compatibility and execution safety

- Registers app tools through `ExtraTools` in native Agent and Plan catalogs.
- Resolves MCP secrets through a host callback without process-environment or plain-file writes.
- Applies exact tool names, read-only actions, and trusted external paths per turn.
- Removes dynamic tools, MCP, and subagents from restricted turns and latches queued control operations until a new explicit message installs replacement authority.
- Rechecks exact/read-only policy at final backend dispatch, retains non-bypassable approvals in Full Access, and caps lower-level File writes at 64 KiB.
- Redacts restricted tool/planning audit payloads.

Tests include exact-dispatch forgery, read-only final dispatch, audit/log redaction, and queued control/MCP/goal bypass cases.

## T3: embedded context and Skill sources

- A host static composer suppresses ambient project instructions, repo law, user constitution, continual harness text, and duplicate core-profile text.
- An explicit `skills_dir` is the sole filesystem Skill root; plugin Skills require an explicit registry.
- Permissions/World State receives a narrow 100 KiB fragment allowance; other fragments retain the 40 KiB default.
- Working-set path analysis ignores only a leading internal `<system-reminder>` without altering model-visible content.

Tests cover ambient authority isolation, explicit Skill roots, the host fragment budget, and reminder/path isolation.

## T4: Automation and runtime lifecycle

- Uses the Automation id as a stable `conversation_key` while keeping each run as a distinct Task.
- Writes upstream task schema v3 and accepts exactly legacy Pinvou v4; v5 and newer fail closed.
- Persists `ThreadCreated` before turn linkage and exposes only narrow `ExecutionTask` getters.
- Skips recurring slots missed by more than 60 seconds, prevents overlap with queued/running attempts, and advances to the first future slot.
- Deletes terminal Tasks during run cleanup without resurrecting deleted Automation records or orphaning an already-enqueued run.

Tests: `forkguard_automation_enqueue_preserves_settings_and_conversation_owner`, `forkguard_scheduler_skips_offline_backfill_and_overlapping_runs`, and `forkguard_accepts_legacy_v4_but_rejects_newer_task_schema`.

## Parent boundary and drift reduction

`pinvou3-app` owns product tool policy, AppMode-to-approval/trust mapping, reasoning effort, owner-event filtering, and scheduled-session creation. Its bridge retains v0.9.12 finite turn/tool limits, read denylist, bubblewrap, MCP OAuth, goal-loop, and telemetry-safe defaults.

The candidate's net +2,527 lines exceed the +1,500 soft limit, chiefly in Engine state, final dispatch, and the child-only host profile plus explicit-Skill-root boundaries where an app-side mirror would create two unsafe sources of truth. Reduction order is: upstream generic steer and per-turn dispatch policy; upstream Automation ownership/misfire behavior; then replace T3 once upstream provides complete static-composer, explicit-Skill-root, and host-profile contracts.

## Publication and rollback

The rollback point is `backup/pre-v0.9.12-sync`. This local candidate is not a publicly reachable submodule. After explicit authorization, publish `pinvou3-clean`, create immutable `pinvou-v0.9.12-r1`, align the parent gitlink to the same SHA, and run `scripts/verify-public-submodule.sh`. Never weaken that public check to make an unpublished object appear released.
