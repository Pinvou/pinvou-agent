# CodeWhale Fork Modification Register

> This is the current-state register for Pinvou's CodeWhale fork.
> See [`fork-policy.md`](fork-policy.md) for policy and [`codewhale-upgrade-0.9.5-to-0.9.12.md`](codewhale-upgrade-0.9.5-to-0.9.12.md) for upgrade evidence. The Chinese register is authoritative.

## Current state (2026-09-09, v0.9.12 r1 published baseline)

| Item | Value |
|---|---|
| Upstream | `v0.9.12`, `dcd4c200f72f0c1ffd60d8e7f6850313db879fc5` |
| Maintenance branch | CodeWhale PR #44 and fast-follow PR #46 align `pinvou3-clean` to `1fafee7e26b60a59457a43bce50c63aa2ad9dbaf` |
| Publication | Public `pinvou3-clean`, immutable tag `pinvou-v0.9.12-r1`, and the parent gitlink all point to the same head |
| Rollback | Public immutable tag `pinvou-v0.9.5-r13` at `f853f8f1566c57e6be40d5439a222a932aa79ef5`; the local branch `backup/pre-v0.9.12-sync` at the same SHA is only a convenience ref |
| History | Fifteen DCO-signed-off commits above upstream, grouped into four long-lived topics; the final ten close review-confirmed behavior, test, documentation, and exact-SHA release-gate gaps |
| Drift | 94 files, `+5022/-944` (net +4,078), down from r13's 110 files and `+10895/-1195` |
| Guard | 37 distinct CodeWhale `forkguard_*` behavior tests (31 default plus 6 under `benchmark-eval-controls`) plus parent fingerprints/tests |

### Multi-root workspace_roots + relative instruction source label (2026-09-11, unpushed on this branch)

- CodeWhale branch `pinvou3/workspace-roots-v12` (9 commits above v0.9.12 r1, head `a78c223de`, ported 1:1 from the v0.9.5-line `pinvou3/workspace-roots` @d9431a28f): threads grow from a single `cwd` into **cwd (primary root) + workspace_roots (full accessible root set)** — the foundation prerequisite for the single-entry workspace (project = primary folder + a set of keys). Four layers: protocol/session model (`workspace_roots` on start/resume/fork params and the `Thread` DTO, serde-defaulted; SQLite `threads` v5 column; both TUI JSON stores additive), per-turn environment (runtime root replacement via `Op::SyncSession`/Runtime API with an active-turn fence, effective next turn; `normalize_workspace_roots` keeps cwd first, deduped; resume three-state semantics including the cwd fallback fix), permission sandbox (the `:workspace_roots` symbol materialized at per-turn policy construction; `workspace_write_policy` carries the normalized set and an empty set is byte-identical to the legacy single root; carve-out, `ToolContext::resolve_path`, and execpolicy rule matching all span attached roots), and prompt/instructions (AGENTS.md discovery stays primary-root-only; the `<project_instructions source="…">` tag becomes file-name-only via the shared `project_instructions_source_label` helper so moving a directory or switching primary root with identical instructions no longer busts the KV prefix cache).
- Behavior boundary: with no roots configured, wire frames, policy values, and path decisions are byte-identical to single-root behavior; there is no multi-root UI in the TUI — roots enter only via the Runtime API/headless; fork does not inherit the parent's attached roots. v0.9.12 note: the compaction reinjection point disappeared in the upstream rework, so the old-line test was not ported.
- Guard: 6 new `forkguard_*` behavior tests (5 `forkguard_workspace_roots_*` + 1 source-label), default group 31→37. Fingerprints: see `scripts/fork-guard.sh`.

## Clean re-fork decision

The old r13 changed 110 files. Upstream v0.9.12 also changed 104 of those files, with 57 projected conflict files. Upstream had already absorbed or redesigned session recovery, edit-last-turn, post-compaction usage, route budgets, provider/model routing, native search, Windows UTF-8 shell handling, JSON schema repair, and much of the task substrate. The r1 branch therefore starts directly at the official v0.9.12 tag and re-expresses only behavior that remains necessary inside foundation lifecycle boundaries.

## Disposition of r13 behavior

| Disposition | Result |
|---|---|
| Drop fork patch, use upstream | Session snapshot/recovery, edit-last-turn, compaction tokens, strict-direct model matching, route budgets, schema normalization, native search, Windows shell decoding, and provider pin improvements |
| Semantic migration | `Yolo`/`Auto` become `Agent` plus approval/trust policy; reasoning is per `SendMessage`; host tools use `ExtraTools`; explicit Skill roots replace global disable shims |
| Rebuild in fork | Host facade and route limits, reliable steer, bulk child cancel, MCP secret resolver, raw worker ledger, per-turn/final-dispatch safety, 64 KiB File cap, prompt ownership, and Automation ownership/schema/lifecycle |
| Preserve, default off | Rebuild r13 benchmark eval controls on the v0.9.12 architecture: final-only-after-tool-budget and unambiguous missing-read-action repair are explicitly feature-gated, off on the desktop default path, and orchestrated/observed through the parent's `benchmark-hooks` surface |
| Preserve | The #35 direct Bing tail for configured API search chains. DuckDuckGo's internal Bing fallback only covers empty/challenge responses, not connection failures; using DuckDuckGo as the outer tail therefore returns early on networks where it is unreachable. API providers now fall directly through to keyless Bing, locked by a result-level test |
| Use upstream removal | Stuck/read-repeat/coaching guards removed by upstream `b39cf5650`: the old stuck fingerprint omitted result state and could kill live-job polling. Do not restore the old guard or its environment knobs; finite `max_steps`, tool budgets, and cancellation remain the limits |

### Upstream test disposition notes

These upstream tests were replaced, not silently deleted, because registered Pinvou product semantics intentionally differ from the upstream defaults:

| Upstream test | v0.9.12 r1 disposition and reason |
|---|---|
| `full_access_auto_approves_non_bypassable_registered_tools` | The high-level execution test is replaced by `full_access_blocks_non_bypassable_registered_tools_without_prompting`; Full Access cannot bypass a non-bypassable approval. The upstream resolver comparison remains covered, with the reversal applied only at Pinvou's final Engine dispatch boundary |
| `discover_for_workspace_and_dir_merges_workspace_and_configured_sources` | The upstream default-merge test is restored; `forkguard_explicit_skills_dir_excludes_ambient_workspace_sources` separately covers the explicit single-root path selected by the Pinvou host |
| `system_prompt_merges_workspace_and_configured_skills_dir` | The upstream default composer test is restored; `forkguard_system_prompt_uses_only_explicit_configured_skills_dir` separately locks ambient-workspace isolation after the host composer is installed |

## Commit sequence

| Commit | Topic |
|---|---|
| `38dd961ea` | T1 host facade, routing, and embedding boundary |
| `a5c12e203` | T2 tool compatibility, MCP secrets, and execution safety |
| `7dc1a429a` | T3 host-owned static prompt composition |
| `02c0faa27` | T4 Pinvou Automation and Task ownership |
| `b4c02616b` | Cross-topic lifecycle, safety, host prompt-only profile, explicit-Skill-root closure, and current Rust release-lint compatibility |
| `dbd1b7cb3` | Review fixes restoring feature-gated benchmark eval controls and result-level guards for 64 KiB writes, session cancellation, terminal deletion, restricted-turn idle deferral, and MCP hiding/denial |
| `fe0cd7551` | Review closure restoring upstream comparison tests, annotating intentional reversals, adding live steer channel/turn-loop coverage, and registering Permissions-fragment upstreaming debt |
| `ff299f94b` | Rereview fix restoring the reachable Bing tail for API search providers, removing the empty benchmark-observability feature, and restoring rationale comments around benchmark compatibility paths |
| `54819b0d6` | Release-gate fast-follow providing a migration-manifest baseline for manual exact-SHA CI, repairing private rustdoc links and stale model-step documentation, and hiding the 18 broad downstream-only compatibility facades from generated API docs without changing compile-time APIs or runtime behavior |
| `ff9959bfc` | Release-gate follow-up removing an unused upstream-comparison Full Access helper after its product semantics had already been deliberately reversed and covered by the active blocked-result regression; restores the dead-code budget without raising it |
| `6615af7ca` | Documentation rereview closure removing four references to a nonexistent fork-policy subsection and describing the registered product reversals and explicit-host boundaries directly |
| `409138dbe` | Runtime-contract release gate recording the official v0.9.12 automation/tasks route fields plus Agent-copy reduction and Pinvou r1 write-limit/host-profile growth; raises only the measured Act/Operate full ceiling while tightening active and Plan full with unchanged tool identities |
| `881cf4444` | Exact-head CI closure allowing the macOS npm wrapper smoke up to 60 minutes for a cold release build after optional sccache loss, with a wiring regression that locks the exception to this job only |
| `baa87f4de` | T4 rereview fix limiting offline-misfire skipping to recurring schedules, so an overdue one-shot still durably enqueues exactly once before pausing |
| `1fafee7e2` | T2 rereview fix restoring actionable, redacted configuration guidance when every web-search backend is unavailable |

All commits carry DCO sign-off. `b4c02616b` contains most of the cross-topic closure and is admittedly coarse for bisecting. After the branch became public for review, history was not force-pushed merely for polish; DCO-signed-off commits were appended for review and release-gate fixes, while this table, fingerprints, and behavior tests restore audit granularity. The immutable tag now exists and must never be rewritten.

## T1: host embedding and routing

- Exposes the host APIs needed for modes, approval, Automation/Task, route resolution, and worker-ledger projection. The compatibility facade still includes 18 `pub mod` declarations with 340 direct references across 61 parent Rust source files; these unstable downstream bridges are intentionally omitted from generated API docs. Narrowing them remains tracked debt and is not attempted as a breaking review hotfix.
- Preserves `resolve_runtime_route_with_limits`, including wire model and embedding aliases.
- Installs `EngineConfig.session_id` before spawn and filters owner-bearing child/workflow events by session.
- Gives each steer an opaque id and exactly one committed/dropped terminal event across withdrawal, interruption, stop, compaction, session sync, and Engine drop.
- Provides idempotent, session-scoped `CancelSubAgents`.
- Keeps v0.9.12's closed role set as the executable posture while allowing only exact config-origin, prompt-only profiles explicitly injected by the host. Ambient personal/workspace/plugin profiles and route/authority pins fail closed.

Tests: `forkguard_embedding_route_limits_preserve_wire_alias`, `forkguard_steer_lifecycle_withdrawal_is_bounded_and_prevents_commit`, `forkguard_steer_lifecycle_late_withdraw_reconciles_committed_state`, `forkguard_steer_channel_commits_live_and_drops_withdrawn_input_in_turn_loop`, `forkguard_cancel_all_running_is_session_scoped_and_idempotent`, and `forkguard_host_profile_overlay_is_config_only_and_prompt_only`.

## T2: tool compatibility and execution safety

- Registers app tools through `ExtraTools` in native Agent and Plan catalogs.
- Resolves MCP secrets through a host callback without process-environment or plain-file writes.
- `SetDisallowedTools` is session/turn-scoped shaping: it still rejects matching tools at that session's catalog and final-call boundaries, but does not hot-disconnect a server already present in the shared `McpPool`. A global disconnect would disrupt other authorized sessions; normal pool/session lifecycle reclaims the connection, while catalog plus final-dispatch checks remain fail closed.
- Applies exact tool names, read-only actions, and trusted external paths per turn.
- Removes dynamic tools, MCP, and subagents from restricted turns and latches queued control operations until a new explicit message installs replacement authority.
- Rechecks exact/read-only policy at final backend dispatch, retains non-bypassable approvals in Full Access, and caps lower-level File writes at 64 KiB.
- Redacts restricted tool/planning audit payloads.

Tests: `forkguard_exact_dispatch_rejects_forged_backends`, `forkguard_read_only_turn_rejects_write_at_final_dispatch`, `forkguard_restricted_tool_audit_redacts_private_payload`, `forkguard_restricted_planning_log_redacts_private_input`, `forkguard_queued_control_op_keeps_restricted_turn_authority`, `forkguard_queued_goal_edit_and_mcp_keep_restricted_authority`, `forkguard_restricted_turn_defers_idle_subagent_completion_until_new_message`, `forkguard_restricted_turn_defers_idle_shell_wake_until_new_message`, `forkguard_denied_mcp_is_absent_from_catalog_and_blocked_at_execution`, `forkguard_denied_mcp_tool_error_matches_the_unknown_tool_error`, `forkguard_api_provider_chain_tail_is_bing`, `all_unavailable_returns_actionable_error_without_private_details`, `forkguard_mcp_secret_resolver_supplies_values_without_process_env_writes`, `forkguard_write_primitive_enforces_the_64kib_boundary`, `forkguard_write_file_enforces_the_64kib_boundary`, `forkguard_benchmark_controls_are_explicit_and_default_off`, `forkguard_benchmark_repairs_only_unambiguous_read_actions`, `forkguard_benchmark_repairs_read_schema_and_attachments`, `forkguard_benchmark_budget_truncates_batch_and_clears_followup_tool_surface`, `forkguard_benchmark_final_only_rejects_repeated_tool_only_responses`, and `forkguard_benchmark_turn_repairs_file_aliases_before_execution`.

## T3: embedded context and Skill sources

- A host static composer suppresses ambient project instructions, repo law, user constitution, continual harness text, and duplicate core-profile text.
- An explicit `skills_dir` is the sole filesystem Skill root; plugin Skills require an explicit registry.
- Permissions/World State receives a narrow 100 KiB fragment allowance; other fragments retain the 40 KiB default.
- Working-set path analysis ignores only a leading internal `<system-reminder>` without altering model-visible content.

Tests: `forkguard_runtime_loader_ignores_ambient_project_authority`, `forkguard_explicit_skills_dir_excludes_ambient_workspace_sources`, `forkguard_instruction_fragment_preserves_explicit_host_budget`, and `forkguard_working_set_ignores_leading_system_reminder_paths`.

## T4: Automation and runtime lifecycle

- Uses the Automation id as a stable `conversation_key` while keeping each run as a distinct Task.
- Writes upstream task schema v3 and accepts exactly legacy Pinvou v4; v5 and newer fail closed.
- Persists `ThreadCreated` before turn linkage and exposes only narrow `ExecutionTask` getters.
- Skips recurring slots missed by more than 60 seconds, prevents overlap with queued/running attempts, and advances to the first future slot. Deduplication intentionally scans all retained run history rather than only the newest page, so newer terminal records cannot hide the same scheduled slot or an older active run; bounded retention limits the cost.
- Deletes terminal Tasks during run cleanup without resurrecting deleted Automation records or orphaning an already-enqueued run.

Tests: `forkguard_automation_enqueue_preserves_settings_and_conversation_owner`, `forkguard_scheduler_skips_offline_backfill_and_overlapping_runs`, `forkguard_once_schedule_missed_while_offline_enqueues_exactly_one_run`, `forkguard_accepts_legacy_v4_but_rejects_newer_task_schema`, `forkguard_terminal_task_delete_refuses_active_and_is_idempotent`, and `forkguard_terminal_automation_run_delete_refuses_active_and_is_idempotent`.

## Parent boundary and drift reduction

`pinvou3-app` owns product tool policy, AppMode-to-approval/trust mapping, reasoning effort, owner-event filtering, and scheduled-session creation. Its bridge retains v0.9.12 finite turn/tool limits, read denylist, bubblewrap, MCP OAuth, goal-loop, and telemetry-safe defaults.

The baseline's net +4,078 lines exceed the +1,500 soft limit, chiefly in Engine state, final dispatch, child-only host profiles, explicit-Skill-root boundaries, and review-requested result-level safety coverage where an app-side mirror would create two unsafe sources of truth. Reduction order is: upstream generic steer and per-turn dispatch policy; upstream Automation ownership/misfire behavior; make the 100 KiB `FragmentId::Permissions` host budget a configurable upstream fragment limit; migrate the parent to explicit re-export APIs and then narrow the 18-module compatibility facade; finally replace T3 once upstream provides complete static-composer, explicit-Skill-root, and host-profile contracts.

## Publication and rollback

The public rollback point is the immutable tag `pinvou-v0.9.5-r13`; local `backup/pre-v0.9.12-sync` is only a convenience ref. The protected `pinvou3-clean` branch and immutable `pinvou-v0.9.12-r1` tag are published at `1fafee7e26b60a59457a43bce50c63aa2ad9dbaf`; the parent gitlink is aligned to the same SHA and verified with `scripts/verify-public-submodule.sh`. The release temporarily removed status contexts that cannot trigger on the maintenance branch solely for this exact-head fast-forward, restored the original protection immediately afterward, kept force pushes disabled, and did not rewrite a published tag. Never weaken the public reachability check to make an unpublished object appear released.
