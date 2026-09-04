# CodeWhale Fork Modification Register

> This is the current-state register for Pinvou's CodeWhale fork.
> See [`fork-policy.md`](fork-policy.md) for policy and [`codewhale-upgrade-0.9.5-to-0.9.12.md`](codewhale-upgrade-0.9.5-to-0.9.12.md) for upgrade evidence. The Chinese register is authoritative.

## Current state (2026-09-11: r1 baseline plus 13 registered commits, r2 tag pending)

| Item | Value |
|---|---|
| Upstream | `v0.9.12`, `dcd4c200f72f0c1ffd60d8e7f6850313db879fc5` |
| Maintenance branch | `pinvou3-clean` = `ae7e3fb36f89486f30d41b28ae0eaaa516ae4740` (13 squash merges above the r1 baseline `1fafee7e2`: #41/#47/#49 plus the 2026-09-10/11 backlog batch #31/#37/#38/#39/#43/#48/#50/#51/#52/#53) |
| Publication | Transition (fork-policy §0 exemption): the immutable tag `pinvou-v0.9.12-r1` stays at the r1 closure (15 commits) while the parent gitlink points at `ae7e3fb36`, 13 commits ahead, until the next r2 release closure realigns the three |
| Rollback | Public immutable tag `pinvou-v0.9.5-r13` at `f853f8f1566c57e6be40d5439a222a932aa79ef5`; the local branch `backup/pre-v0.9.12-sync` at the same SHA is only a convenience ref |
| History | 28 DCO-signed-off commits above upstream in four long-lived topics (T1–T4) plus two append topics (T5 session archive export, T6 swarm rate-limit governor); all 13 post-r1 commits landed via squash-merged PRs through the five required gates |
| Drift | 139 files, `+9953/-1217` (net +8,736); r1 was 94 files `+5022/-944`, old r13 was 110 files `+10895/-1195` |
| Guard | 54 distinct CodeWhale `forkguard_*` behavior tests (48 default plus 6 under `benchmark-eval-controls`) plus parent fingerprints/tests; consumer PRs #396 (execpolicy), #408 (turn-bound cancel), #444 (swarm), #468 (computer-use), #472 (one-click export) depend on this batch |

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
| `a7215d3c4` | Gate fix restoring the required merge checks for `pinvou3-clean` (#49) |
| `bf435e5e8` | T4 fix keeping terminal-task memory consistent when artifact cleanup fails (#47) |
| `c3a35216e` | T2 documentation aligning model-facing tool descriptions and prompts with behavior across 36 tool files (#41) |
| `a89048184` | T2 fix gating the finance tool through the network policy (fail-closed), un-teaching the retired name in verifier metadata, pinning the notify configured-method contract, and documenting `resume`/`stop --all` in the fleet-manager skill (#31) |
| `ff4add43e` | T2 execpolicy phase-2: cmd.exe single-letter slash-flag skipping, deny mid-rule wildcard, `.exe` suffix folding on the deny command word, exact rooted absolute-path rules, live-shared rulesets across engine clones, and subagent tool calls passing the same execpolicy gate (#37) |
| `f5c68cab8` | T1 fix binding the shared cancel slot to the owning turn: atomic `TurnCancelSlot` installs, identity-checked `cancel_turn`, and a disposition-only `publish_stop_disposition` for the terminal-closing window (#38, foundation half of pinvou-agent#254) |
| `3f8a25eef` | Documentation recording the keyless Bing search tail and its mainland-China rationale in CHANGELOG (#39) |
| `dd7b7785f` | T6 addition: adaptive subagent rate limiting with a shrinkable `DynamicGate` and a 60s sliding-window AIMD `RateLimitGovernor` with time-driven recovery; 429 retries honor `Retry-After` with full-jitter fallback (#43) |
| `1d9ee26e6` | T2 fix deriving Bash tool guidance from the same shell dispatcher as execution, so Windows PowerShell hosts no longer teach login-shell syntax (#50, replacing the retired r13-line #42) |
| `68461e84e` | T2 addition allowing tool results to carry images via `metadata.images`: the same `<image>` triplet path as user attachments, capped at two per result, degrading on bad images without failing the turn (#48, foundation for computer-use screenshots) |
| `09ebba3fa` | T5 addition: full-fidelity session archive export via the `session_export` module and `codewhale sessions export` CLI with static liblzma; deliberately unsanitized counterpart to lossy `/export` (#51) |
| `6ae5b1734` | T1 fix rewriting `disabled` to `enabled`+`low` for forced-thinking GLM-5.3/GLM-5.3-Flash and mapping effort aliases onto low/high/max (#52, fixing the default Z.ai model erroring on `off`) |
| `ae7e3fb36` | T1 fix adding BigModel's general endpoint `open.bigmodel.cn/api/paas/v4` to the first-party Chat route predicate so reasoning controls apply uniformly on both hosts (#53) |

All commits carry DCO sign-off. `b4c02616b` contains most of the cross-topic closure and is admittedly coarse for bisecting. After the branch became public for review, history was not force-pushed merely for polish; DCO-signed-off commits were appended for review and release-gate fixes, while this table, fingerprints, and behavior tests restore audit granularity. The immutable tag now exists and must never be rewritten.

## T1: host embedding and routing

- Exposes the host APIs needed for modes, approval, Automation/Task, route resolution, and worker-ledger projection. The compatibility facade still includes 18 `pub mod` declarations with 340 direct references across 61 parent Rust source files; these unstable downstream bridges are intentionally omitted from generated API docs. Narrowing them remains tracked debt and is not attempted as a breaking review hotfix.
- Preserves `resolve_runtime_route_with_limits`, including wire model and embedding aliases.
- Installs `EngineConfig.session_id` before spawn and filters owner-bearing child/workflow events by session.
- Gives each steer an opaque id and exactly one committed/dropped terminal event across withdrawal, interruption, stop, compaction, session sync, and Engine drop.
- Provides idempotent, session-scoped `CancelSubAgents`.
- Binds the shared cancel slot to the owning turn (#38): `handle_send_message` and the user `!` shell turn mint the turn id before atomically installing the token; the host `cancel_turn(turn_id)` performs the identity check, steer-disposition publication, and token resolution under the slot lock with zero side effects on mismatch, so a stale-generation cancel can no longer kill an engine self-started follow-up turn (idle sub-agent completion, background shell wake, goal continuation). `cancel_with_mode` keeps fire-current-token semantics for single-user frontends; `publish_stop_disposition` serves the terminal-closing stop=clear contract without firing any token.
- Covers both first-party Chat hosts (`api.z.ai` products and BigModel `open.bigmodel.cn/api/paas/v4`; the tui predicate delegates to the single config-crate implementation) and treats GLM-5.3/GLM-5.3-Flash as forced-thinking: `thinking.type: "disabled"` is rewritten to `enabled` + `reasoning_effort: "low"` per the vendor migration note, effort aliases map onto low/high/max, unknown values stay omitted for the API default, and the work-graph receipt constraint refuses an effective `off` the API never accepted (#52/#53).
- Keeps v0.9.12's closed role set as the executable posture while allowing only exact config-origin, prompt-only profiles explicitly injected by the host. Ambient personal/workspace/plugin profiles and route/authority pins fail closed.

Tests: `forkguard_embedding_route_limits_preserve_wire_alias`, `forkguard_steer_lifecycle_withdrawal_is_bounded_and_prevents_commit`, `forkguard_steer_lifecycle_late_withdraw_reconciles_committed_state`, `forkguard_steer_channel_commits_live_and_drops_withdrawn_input_in_turn_loop`, `forkguard_cancel_all_running_is_session_scoped_and_idempotent`, and `forkguard_host_profile_overlay_is_config_only_and_prompt_only`, plus `forkguard_cancel_turn_binding_spares_unnamed_turns_and_hits_the_observed_turn`, `forkguard_idle_subagent_completion_self_start_ignores_a_stale_previous_turn_cancel`, `engine_handle_stop_disposition_publishes_without_firing_any_token`, `zai_forced_thinking_models_never_send_thinking_disabled`, `restored_zai_forced_thinking_models_cannot_claim_effective_off`, `zai_bigmodel_adjacent_routes_stay_fail_closed`, and `zai_chat_route_matching_is_exact`.

## T2: tool compatibility and execution safety

- Registers app tools through `ExtraTools` in native Agent and Plan catalogs.
- Resolves MCP secrets through a host callback without process-environment or plain-file writes.
- `SetDisallowedTools` is session/turn-scoped shaping: it still rejects matching tools at that session's catalog and final-call boundaries, but does not hot-disconnect a server already present in the shared `McpPool`. A global disconnect would disrupt other authorized sessions; normal pool/session lifecycle reclaims the connection, while catalog plus final-dispatch checks remain fail closed.
- Applies exact tool names, read-only actions, and trusted external paths per turn.
- Removes dynamic tools, MCP, and subagents from restricted turns and latches queued control operations until a new explicit message installs replacement authority.
- Rechecks exact/read-only policy at final backend dispatch, retains non-bypassable approvals in Full Access, and caps lower-level File writes at 64 KiB.
- Redacts restricted tool/planning audit payloads.
- Extends execpolicy deny expressiveness and closes the delegation bypass (#37): cmd.exe single-letter slash flags skip while multi-character POSIX paths stay positional; a mid-rule `*` matches zero or more command tokens over a bounded DFS; the deny command word folds one trailing `.exe`; rooted (leading `/`, `~/`, Windows drive) File path rules match calls outside the workspace exactly after separator/case folding; rulesets are shared live across engine clones while session approvals stay clone-private; subagent tool calls pass the same execpolicy gate as the main line (Block mirrors the main-line refusal; Prompt honors the inherited auto-approve posture). Generic embedder capability, upstreaming candidate.
- Lets tool results carry images through `ToolResult.metadata["images"]` (#48): the engine turn loop appends the same `<image path>` triplet blocks user attachments produce to the tool-result message, capped at two images per result with the shared 5 MB limit; missing, oversized, or invalid files degrade to a text-only result with a warning and never fail the turn, and error-path results never attach. Wire builders, session persistence, compaction, and blind-route stripping are unchanged because the images ride the existing `ImageUrl` block path.
- Aligns model-facing docs and gating (#31/#41/#50): the finance tool vets both endpoint hosts up front through `NetworkPolicyDecider` with Deny/undecided-Prompt failing closed in the web_search/speech error shape; verifier background metadata no longer teaches the retired `exec_shell_wait`; the notify configured-method contract is pinned by regression tests; Bash tool descriptions and the `command` field derive from the same shell dispatcher as execution, giving Windows PowerShell hosts concrete PowerShell syntax instead of login-shell guidance.

Tests: `forkguard_exact_dispatch_rejects_forged_backends`, `forkguard_read_only_turn_rejects_write_at_final_dispatch`, `forkguard_restricted_tool_audit_redacts_private_payload`, `forkguard_restricted_planning_log_redacts_private_input`, `forkguard_queued_control_op_keeps_restricted_turn_authority`, `forkguard_queued_goal_edit_and_mcp_keep_restricted_authority`, `forkguard_restricted_turn_defers_idle_subagent_completion_until_new_message`, `forkguard_restricted_turn_defers_idle_shell_wake_until_new_message`, `forkguard_denied_mcp_is_absent_from_catalog_and_blocked_at_execution`, `forkguard_denied_mcp_tool_error_matches_the_unknown_tool_error`, `forkguard_api_provider_chain_tail_is_bing`, `all_unavailable_returns_actionable_error_without_private_details`, `forkguard_mcp_secret_resolver_supplies_values_without_process_env_writes`, `forkguard_write_primitive_enforces_the_64kib_boundary`, `forkguard_write_file_enforces_the_64kib_boundary`, `forkguard_benchmark_controls_are_explicit_and_default_off`, `forkguard_benchmark_repairs_only_unambiguous_read_actions`, `forkguard_benchmark_repairs_read_schema_and_attachments`, `forkguard_benchmark_budget_truncates_batch_and_clears_followup_tool_surface`, `forkguard_benchmark_final_only_rejects_repeated_tool_only_responses`, and `forkguard_benchmark_turn_repairs_file_aliases_before_execution`, plus `forkguard_subagent_execpolicy_deny_matches_main_line`, `forkguard_tool_result_images_reach_model_as_image_blocks`, `forkguard_tool_result_without_images_key_is_unchanged`, `forkguard_tool_result_bad_images_degrade_to_text_only_result`, `forkguard_tool_result_images_are_capped_per_result`, `finance_fails_closed_when_network_policy_denies_endpoint_host`, `finance_fails_closed_on_prompt_when_default_is_prompt`, `settings_installs_configured_method_from_config`, and `forkguard_shell_catalog_guidance_matches_execution`.

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

## T5: full-fidelity session archive export (append topic)

- `session_export` module and `codewhale sessions export <id> [--output] [--compression 0-9] [--skip-artifacts] [--force]` (#51) stream the complete `SavedSession` (standalone system prompt, every turn including thinking/tool_use/tool_result blocks, the branch journal, hydrated approval receipts) into a `.tar.xz` archive with static liblzma (mirroring the bundled mimalloc precedent).
- Archive layout v1: `session.json` (full-fidelity `/load` restore), `container.json` (version-tolerant `/resume` import), `artifacts/**` (regular files only; symlinks skipped so an export cannot read outside the session directory; members size-bounded so a mid-export shrink fails the export instead of corrupting the archive), and `manifest.json` written last.
- Output lands atomically via a sibling temp file and rename; the CLI requires `--force` to overwrite. Content is deliberately unsanitized — the owner's complete log, complementing the redacted, share-facing `/export` markdown — and the distinction is documented in the module docs and this register.
- Upstreaming intent: format, CLI, and tests are generic foundation capability intended to be upstreamed as a unit to remove the drift.

Tests: `forkguard_session_archive_export_roundtrips_full_context`, `forkguard_session_archive_includes_artifacts_and_respects_skip`, `forkguard_session_archive_rejects_artifact_shorter_than_recorded_size`, `session_archive_replaces_existing_output_atomically`, and `session_archive_rejects_out_of_range_compression_level`.

## T6: swarm rate-limit-adaptive scheduling (append topic)

- Replaces the fixed `Semaphore` launch gate with `DynamicGate` (#43): capacity shrinks at runtime (allowed below the active-holder count; surplus holders finish naturally) and permits hand over pre-counted through a oneshot channel, with cancellation-safe handling on both paths so a lost wakeup cannot occur.
- `RateLimitGovernor` is shared per engine and inherited through the `SubAgentRuntime` derive tree (stamped at the manager's single spawn chokepoint): a 60s sliding window halves capacity at ≥2 rate-limit events or >30% ratio (guarded by an attempts≥2 volume floor), pauses new admissions at ≥4 (capacity 0, in-flight unaffected), additively increases one unit per three consecutive successes (AIMD), releases a pause only when the window drains — external limit changes cannot silently lift it — and a 5s queued-side `recover_if_window_drained` probe prevents a frozen queue when the in-flight fleet finishes without any success signal.
- Retries honor `Retry-After` and otherwise use full-jitter exponential backoff (250ms base, 120s cap) to de-synchronize a fan-out thundering herd; `QuotaExhausted` keeps the existing fatal/checkpoint path; the governor only observes and never delays in-flight calls; configured launch concurrency applies to the live gate immediately through the governor.
- Consumed by the parent's swarm mode (PR #444); the governor itself is generic embedder capability.

Tests: `forkguard_rate_limit_governor_pauses_and_time_recovers_after_window_drains` plus the gate cancel-before-dispatch and multi-task abort/pause stress tests that must drain back to full capacity within a time budget.

## Parent boundary and drift reduction

`pinvou3-app` owns product tool policy, AppMode-to-approval/trust mapping, reasoning effort, owner-event filtering, and scheduled-session creation. Its bridge retains v0.9.12 finite turn/tool limits, read denylist, bubblewrap, MCP OAuth, goal-loop, and telemetry-safe defaults.

Shell task reconciliation prefers the stable `origin_tool_call_id` carried by snapshots and completion events (an upstream v0.9.12 behavior, Hmbown/CodeWhale #5869): the host monitor and the Tauri/Web bridges rewrite the originating tool card first and fall back to command-text matching only for legacy origin-less jobs. An identified terminal root job whose origin card was compacted or reloaded away never appends to the current timeline tail, while running jobs keep a visible synthetic status card (`shell_task_projection.test.mjs`, `forkguard_shell_monitor_assigns_identical_commands_by_stable_origin`).

The baseline's net +8,736 lines exceed the +1,500 soft limit, chiefly in Engine state, final dispatch, child-only host profiles, explicit-Skill-root boundaries, and review-requested result-level safety coverage where an app-side mirror would create two unsafe sources of truth. Reduction order is: upstream generic steer and per-turn dispatch policy; upstream Automation ownership/misfire behavior; upstream the execpolicy phase-2 expressiveness and subagent wiring; upstream the T5 archive export as a unit; upstream the T6 rate-limit governor; upstream the GLM-5.3 forced-thinking dialect and BigModel route predicate; make the 100 KiB `FragmentId::Permissions` host budget a configurable upstream fragment limit; migrate the parent to explicit re-export APIs and then narrow the 18-module compatibility facade; finally replace T3 once upstream provides complete static-composer, explicit-Skill-root, and host-profile contracts.

## Publication and rollback

The public rollback point is the immutable tag `pinvou-v0.9.5-r13`; local `backup/pre-v0.9.12-sync` is only a convenience ref. The immutable `pinvou-v0.9.12-r1` tag stays at the r1 closure `1fafee7e26b60a59457a43bce50c63aa2ad9dbaf`; the repository is in a transition where `pinvou3-clean` and the parent gitlink point at `ae7e3fb36` (13 squash commits ahead of the tag), and the next r2 release closure will cut an immutable tag at the merged head and realign branch, tag, and gitlink, verified with `scripts/verify-public-submodule.sh` (during the transition the script asserts gitlink equals the maintenance-branch head and the tag stays pinned at the r1 closure, restoring three-way equality after the r2 closure, per the fork-policy section 0 transition exemption). The release temporarily removed status contexts that cannot trigger on the maintenance branch solely for this exact-head fast-forward, restored the original protection immediately afterward, kept force pushes disabled, and did not rewrite a published tag. Never weaken the public reachability check to make an unpublished object appear released.
