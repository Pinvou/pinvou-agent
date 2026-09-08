# CodeWhale Fork Modification Register

> This is the current-state register for Pinvou's CodeWhale fork.
> See [`fork-policy.md`](fork-policy.md) for policy and [`codewhale-upgrade-0.9.5-to-0.9.12.md`](codewhale-upgrade-0.9.5-to-0.9.12.md) for upgrade evidence. The Chinese register is authoritative.

## Current state (2026-09-08, v0.9.12 r1 public PR candidate)

| Item | Value |
|---|---|
| Upstream | `v0.9.12`, `dcd4c200f72f0c1ffd60d8e7f6850313db879fc5` |
| Candidate branch | Pushed as `codex/pinvou-v0.9.12-r1` in CodeWhale PR #44, currently at `fe0cd7551175f0f3df2c785b15c6c16e282218f7` |
| Publication | Not yet the protected baseline: public `pinvou3-clean` still points to r13 and the immutable `pinvou-v0.9.12-r1` tag does not exist |
| Rollback | Public immutable tag `pinvou-v0.9.5-r13` at `f853f8f1566c57e6be40d5439a222a932aa79ef5`; the local branch `backup/pre-v0.9.12-sync` at the same SHA is only a convenience ref |
| History | Seven signed commits above upstream, grouped into four long-lived topics; the final two commits close review-confirmed behavior, test, and documentation gaps |
| Drift | 66 files, `+4769/-620` (net +4,149), down from r13's 110 files and `+10895/-1195` |
| Guard | 35 distinct CodeWhale `forkguard_*` behavior tests (29 default plus 6 under `benchmark-eval-controls`) plus parent fingerprints/tests |

## Clean re-fork decision

The old r13 changed 110 files. Upstream v0.9.12 also changed 104 of those files, with 57 projected conflict files. Upstream had already absorbed or redesigned session recovery, edit-last-turn, post-compaction usage, route budgets, provider/model routing, native search, Windows UTF-8 shell handling, JSON schema repair, and much of the task substrate. The r1 branch therefore starts directly at the official v0.9.12 tag and re-expresses only behavior that remains necessary inside foundation lifecycle boundaries.

## Disposition of r13 behavior

| Disposition | Result |
|---|---|
| Drop fork patch, use upstream | Session snapshot/recovery, edit-last-turn, compaction tokens, strict-direct model matching, route budgets, schema normalization, native search, Windows shell decoding, and provider pin improvements |
| Semantic migration | `Yolo`/`Auto` become `Agent` plus approval/trust policy; reasoning is per `SendMessage`; host tools use `ExtraTools`; explicit Skill roots replace global disable shims |
| Rebuild in fork | Host facade and route limits, reliable steer, bulk child cancel, MCP secret resolver, raw worker ledger, per-turn/final-dispatch safety, 64 KiB File cap, prompt ownership, and Automation ownership/schema/lifecycle |
| Preserve, default off | Rebuild r13 benchmark eval controls on the v0.9.12 architecture: final-only-after-tool-budget and unambiguous missing-read-action repair are explicitly feature-gated, off on the desktop default path, and orchestrated/observed through the parent's `benchmark-hooks` surface |
| Do not port | The #35 override that made Bing the direct tail of API search chains. v0.9.12 first uses DuckDuckGo as the configured-API tail, while the standard DuckDuckGo path has a bounded internal Bing fallback for empty/challenge responses. The Pinvou desktop explicitly selects Bing by default, so its default path is unchanged; API-backend users incur the extra DuckDuckGo attempt |
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

All commits carry DCO sign-off. `b4c02616b` contains most of the cross-topic closure and is admittedly coarse for bisecting. Because the candidate is already public and under review, this round does not force-push merely to polish history; review fixes are appended as signed commits, while this table, fingerprints, and behavior tests restore audit granularity. An immutable tag must never be rewritten.

## T1: host embedding and routing

- Exposes the host APIs needed for modes, approval, Automation/Task, route resolution, and worker-ledger projection. The compatibility facade still includes 18 `pub mod` declarations with 272 direct references across 48 parent Rust files; narrowing it is tracked debt and is not attempted as a breaking review hotfix.
- Preserves `resolve_runtime_route_with_limits`, including wire model and embedding aliases.
- Installs `EngineConfig.session_id` before spawn and filters owner-bearing child/workflow events by session.
- Gives each steer an opaque id and exactly one committed/dropped terminal event across withdrawal, interruption, stop, compaction, session sync, and Engine drop.
- Provides idempotent, session-scoped `CancelSubAgents`.
- Keeps v0.9.12's closed role set as the executable posture while allowing only exact config-origin, prompt-only profiles explicitly injected by the host. Ambient personal/workspace/plugin profiles and route/authority pins fail closed.

Tests: `forkguard_embedding_route_limits_preserve_wire_alias`, `forkguard_steer_lifecycle_withdrawal_is_bounded_and_prevents_commit`, `forkguard_steer_lifecycle_late_withdraw_reconciles_committed_state`, `forkguard_steer_channel_commits_live_and_drops_withdrawn_input_in_turn_loop`, `forkguard_cancel_all_running_is_session_scoped_and_idempotent`, and `forkguard_host_profile_overlay_is_config_only_and_prompt_only`.

## T2: tool compatibility and execution safety

- Registers app tools through `ExtraTools` in native Agent and Plan catalogs.
- Resolves MCP secrets through a host callback without process-environment or plain-file writes.
- `SetDisallowedTools` still denies matching tools at catalog/call boundaries, but v0.9.12 no longer hot-disconnects an already connected MCP server; the connection is reclaimed by the normal pool/session lifecycle.
- Applies exact tool names, read-only actions, and trusted external paths per turn.
- Removes dynamic tools, MCP, and subagents from restricted turns and latches queued control operations until a new explicit message installs replacement authority.
- Rechecks exact/read-only policy at final backend dispatch, retains non-bypassable approvals in Full Access, and caps lower-level File writes at 64 KiB.
- Redacts restricted tool/planning audit payloads.

Tests: `forkguard_exact_dispatch_rejects_forged_backends`, `forkguard_read_only_turn_rejects_write_at_final_dispatch`, `forkguard_restricted_tool_audit_redacts_private_payload`, `forkguard_restricted_planning_log_redacts_private_input`, `forkguard_queued_control_op_keeps_restricted_turn_authority`, `forkguard_queued_goal_edit_and_mcp_keep_restricted_authority`, `forkguard_restricted_turn_defers_idle_subagent_completion_until_new_message`, `forkguard_restricted_turn_defers_idle_shell_wake_until_new_message`, `forkguard_denied_mcp_is_absent_from_catalog_and_blocked_at_execution`, `forkguard_denied_mcp_tool_error_matches_the_unknown_tool_error`, `forkguard_mcp_secret_resolver_supplies_values_without_process_env_writes`, `forkguard_write_primitive_enforces_the_64kib_boundary`, `forkguard_write_file_enforces_the_64kib_boundary`, `forkguard_benchmark_controls_are_explicit_and_default_off`, `forkguard_benchmark_repairs_only_unambiguous_read_actions`, `forkguard_benchmark_repairs_read_schema_and_attachments`, `forkguard_benchmark_budget_truncates_batch_and_clears_followup_tool_surface`, `forkguard_benchmark_final_only_rejects_repeated_tool_only_responses`, and `forkguard_benchmark_turn_repairs_file_aliases_before_execution`.

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
- Skips recurring slots missed by more than 60 seconds, prevents overlap with queued/running attempts, and advances to the first future slot.
- Deletes terminal Tasks during run cleanup without resurrecting deleted Automation records or orphaning an already-enqueued run.

Tests: `forkguard_automation_enqueue_preserves_settings_and_conversation_owner`, `forkguard_scheduler_skips_offline_backfill_and_overlapping_runs`, `forkguard_accepts_legacy_v4_but_rejects_newer_task_schema`, `forkguard_terminal_task_delete_refuses_active_and_is_idempotent`, and `forkguard_terminal_automation_run_delete_refuses_active_and_is_idempotent`.

## Parent boundary and drift reduction

`pinvou3-app` owns product tool policy, AppMode-to-approval/trust mapping, reasoning effort, owner-event filtering, and scheduled-session creation. Its bridge retains v0.9.12 finite turn/tool limits, read denylist, bubblewrap, MCP OAuth, goal-loop, and telemetry-safe defaults.

The candidate's net +4,149 lines exceed the +1,500 soft limit, chiefly in Engine state, final dispatch, child-only host profiles, explicit-Skill-root boundaries, and review-requested result-level safety coverage where an app-side mirror would create two unsafe sources of truth. Reduction order is: upstream generic steer and per-turn dispatch policy; upstream Automation ownership/misfire behavior; make the 100 KiB `FragmentId::Permissions` host budget a configurable upstream fragment limit; migrate the parent to explicit re-export APIs and then narrow the 18-module compatibility facade; finally replace T3 once upstream provides complete static-composer, explicit-Skill-root, and host-profile contracts.

## Publication and rollback

The public rollback point is the immutable tag `pinvou-v0.9.5-r13`; local `backup/pre-v0.9.12-sync` is only a convenience ref. The candidate branch is public in PR #44, but it is not yet the protected submodule baseline because `pinvou3-clean` and the immutable r1 tag are not aligned to it. After explicit authorization, publish `pinvou3-clean`, create immutable `pinvou-v0.9.12-r1`, align the parent gitlink to the same SHA, and run `scripts/verify-public-submodule.sh`. Never weaken that public check to make an unpublished object appear released.
