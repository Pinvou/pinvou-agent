# Tri-mode safety model and the session-level guardrail-ask switch (tri-mode safety)

> English mirror of [`safety-modes.md`](./safety-modes.md); the Chinese original is authoritative on any divergence. Added per `CONTRIBUTING.md`'s English requirement for new developer documentation.

> This document is the design reference for refining the application-side safety posture from the
> two-mode Plan / Yolo setup into the three-mode Plan / Auto / Yolo, and for introducing a
> session-level ask-on-guardrail switch for Yolo (this PR's Stage 4).
> The foundation behavior is registered as [Pinvou/CodeWhale#131](https://github.com/Pinvou/CodeWhale/pull/131):
> (a) `ask_on_guardrail`, the host-armed interactive guardrail prompt; (b) the `<turn_meta>` authoritative transition notice.
> The single current-state register for fork registrations and fingerprints is [`fork-modifications.md`](fork-modifications.md).
>
> The alignment research against Claude Code 6-mode, the Codex sandbox/approval separation, the Gemini CLI plan<default<yolo
> hierarchy, and opencode's deny-survives-auto is in the parent repo PR body; this document records only the conclusions that landed in
> the implementation.

## 1. Background and goals

Under the old two-mode setup the work lane defaulted to Yolo (`AppMode::Agent` + auto-approval): the model could execute card-free
throughout, and the foundation guardrail faces (non-bypass holds, repo-law, typed ask rules, sandbox escalation, the execpolicy
deny face) all failed closed with hard denies in the autonomous posture. There were two gaps: there was no intermediate mode (mainstream
harnesses' default mode, e.g. Claude Code `acceptEdits`/`default`, is "per-item approval for ordinary operations, card-free writes
inside the workspace"); and under Yolo a guardrail hit had only one path — the hard deny — so the model could not ask the user for an
in-person decision on a single protected sensitive operation.

The tri-mode rework **splits the approval posture out of the mode** (the `SerializableMode::approval_params` mapping,
`pinvou3-app/src-tauri/src/core/mode_state.rs`): Plan and Auto share `(false, Suggest)`, Yolo is `(true, Bypass)`, delivered per turn
with `Op::SendMessage`; at the same time the foundation gains a session-level `ask_on_guardrail` arming (via `Op::ChangeMode`), and
once armed, the fail-closed guardrail faces above instead raise an approval request — provided the host has already guaranteed that a
human is available to answer.

## 2. Mode semantics

| Mode | Engine posture (same mapping for the per-turn op and the sync op) | Guardrail hit (switch ON, default) | Guardrail hit (switch OFF) | Ordinary promptable tools |
|---|---|---|---|---|
| Plan | `AppMode::Plan` (foundation read-only normalization: shell disabled, read-only sandbox, read-only toolset) | Read-only interception (existing) | Same as left | Intercepted; an arriving approval event is an anomaly and is defensively hard-denied (`plan_mode`) |
| Auto | `Agent` + `(false, Suggest)` | Card raised | The autonomous guardrail face silently hard-denies inside the engine gate (switch OFF = not armed; this transition covers only the tri-mode autonomous guardrail face and produces no approval event, so there is **no** `chat:guardrail_denied` — that notice covers only rejections on the host approval surface, see §4.1) | Per-instance approval card; file writes inside the workspace go card-free via the foundation #5185 carve-out, and read-only commands go card-free via the readonly classification |
| Yolo | `Agent` + `(true, Bypass)` | Card raised | Hard deny + notice (`guardrail_ask_off`; this notice goes through the host approval surface — an un-armed autonomous guardrail denial produces no event inside the engine gate, so this cell is reachable only when a denial event still arrives at the host surface, e.g. a card still in flight after an armed→disarmed flip) | Card-free (historical Yolo behavior) |

Note (review P3, surfaced at the opening): switch OFF does not turn Auto into "no prompts" — the **pre-existing**
Suggest per-item Ask face (ordinary promptable tools), repository-law prompts, and sandbox escalation prompts still
pop up as usual when not armed; the only face the switch transitions is the autonomous guardrail face this PR adds
(a baseline guardrail hit changes from "raise a card and wait for a human" to "silently hard-deny"). The "Guardrail
hit (switch OFF)" column in the table above describes this new face's OFF behavior, not Auto mode's overall prompt
surface.

- Auto is the default for the new work lane (unbound plain sessions); code sessions and workspace-bound plain
  sessions follow `code_permission.last_mode` by default (never used → Plan). Safety posture follows workspace
  binding, not the session name.
- Mode switching (desktop dual-lane): entering Yolo from either mode passes the same one-time confirmation gate
  (`chatYoloGateAllows` / native lane `needsYoloConfirmation`) — returning from Plan goes through
  `exit_plan_to_yolo`, going straight from Auto goes through `set_session_mode(yolo)`, and no switch path silently
  lapses; entering Auto/Plan does not pass the gate. In the draft state (no session yet) the guardrail switch is
  disabled.
- The Plan flow handoff (proposal → accept → back to the mode held before entering Plan) reuses the foundation's
  native closed loop; the app does not build its own phase state machine; see section 6's `pre_plan_mode`.

## 3. The session-level ask_on_guardrail switch

### 3.1 Semantics and persistence

- **On by default.** A new session's switch is seeded from the global preference
  `code_permission.ask_on_guardrail_default` (settings.json, default true); an explicit override in the
  `_ask_on_guardrail.json` sidecar takes precedence. Flipping the switch for a session that has no mode entry yet:
  the choice lands in the sidecar plus an in-memory override mirror (without pinning the mode through this), and
  later materialization (a mode write minting the entry) and restarts inherit that choice through the seeding
  point (`ask_on_guardrail_seed`) — an explicit override is never silently dropped. The sidecar is a
  `HashMap<String, bool>` explicit-override table sharing the same io mutex and atomic write as the mode
  (`features/sessions/mode_state.rs`). Disk-write failure semantics split by command: **a mode switch
  (`set_mode`) only logs and does not interrupt the interaction** — the in-memory switch has already taken
  effect, and the disk write is best-effort persistence; **a switch flip (`set_ask_on_guardrail`) persists first
  and publishes after** — until the sidecar RMW succeeds, neither the entry nor the override mirror is written,
  and no read side can see a value that does not exist on disk (fail-consistent; the failure path has no rollback
  action: memory never left its pre-call state). Best-effort persistence fails asymmetrically: **when a
  width-narrowing switch (Yolo→Plan/Auto) fails to persist, the in-process chip and the engine posture remain at
  the narrowed mode for that process, but after a restart they restore to the persisted wider mode** (a failed
  width-widening switch only errs conservative) — this is the established trade-off of the interactive lane;
  the headless planning path takes the strict mode via `set_mode_and_persist`. An interrupted write leaves no
  truncated file, and a corrupted file is skipped with conservative defaults after an error log at load time
  (not silently). On session deletion/eviction the sidecar entry is cleared together under the retention policy.
- Lane global defaults (settings.json `mode_defaults.work` /
  `code_permission.last_mode`) **never write the `"auto"` literal**: an explicit Auto choice persists through a
  dedicated `*_auto` companion flag, and the legacy vocabulary stays Plan/Yolo — pre-tri-mode binaries' whole-file
  parsing is therefore unaffected (an unknown variant would break the whole file and make the next save overwrite
  everything with defaults). The lane-default view/parsing folds the companion flag back into `Auto`. **The truth
  in the downgrade direction**: an old binary reading a new file drops the unknown flag — the code lane's
  `last_mode` stays None → the old default Plan (staleness in the stricter direction); the work lane's `work`
  stays None → an unbound plain session falls to the old default **Yolo full-auto (the wider direction)**: an
  explicit Auto global default is silently widened for the window in which a downgraded binary runs, and
  restoring the new binary restores it, with no persistent loss.
- Write paths uniformly pass an existence gate (id shape + session existence, `ensure_mode_target_session`:
  the four commands `set_plan_mode_next`/`exit_plan_to_yolo`/`set_session_mode`/`set_ask_on_guardrail`),
  preventing direct IPC calls from minting entries/writing sidecars for arbitrary ids; `accept_plan` does not pass
  that gate and is instead constrained by the plan ticket (`claim_pending_plan`) to a genuinely pending proposal,
  likewise minting no arbitrary entry.

### 3.2 Propagation and arming

The switch is published together with the mode through `publish_session_mode_change`: when the session engine is
in place it sends `Op::ChangeMode` (`ask_on_guardrail` explicitly carries `Some(flag)` — this op replaces the
posture wholesale; `None` = no change, which is the wire layer's serde absent-by-default semantics) plus a hot
ruleset swap; when the engine is not in place it is skipped, and the next send after lazy spawn groups the op from
the store state and arms during spawn (`engine_pool.rs`'s `sync_session_safety_posture` /
`arm_spawned_session_safety_posture`, sharing the same `safety_change_mode_op` pure mapping). The foundation side
carries it through `LiveRuntimeAuthority` under the same doctrine as the other posture fields; turn installation
re-records the session's current value and **never rolls back a published arming**
(`record_applied_runtime_authority`).

### 3.3 The five foundation guardrail faces changed by arming

Host arming = guaranteeing a human is available to answer, so faces that previously failed closed in the
autonomous posture switch to approval (foundation `crates/tui/src/core/engine/turn_loop.rs`; mechanism registered
in foundation PR (a)):

1. **Non-bypass holds** (`rlm`/`rlm_eval`/`start_mcp_server`): under Full Access, changed from a hard deny to an
   approval carrying `approval_force_prompt`. The flag must be set explicitly — an ordinary resolver auto-passes
   non-bypass tools under auto-approve, and if the flag fails to land the tool executes silently (the #3866
   inverted semantics).
2. **repo-law ask holds**: raise an approval instead of a hard deny.
3. **typed ask-rule Prompt**: respected, no longer silently swallowed by auto_approve (the #3790 "mode is the
   sole authority" gains an exception at arming).
4. **sandbox escalation** (`sandbox_permissions`): approved through the Suggest path, carrying a justification.
5. **the execpolicy deny face** (denied prefixes + typed denies, match semantics unchanged): upgraded to an
   approval carrying match evidence.

   The arming exemptions above take effect only in **card-raisable postures** (Full Access / auto-approve):
   Auto-Review and Never keep denying unconditionally on all faces — a card no one can answer never rises
   (review fix `04c1f7843` explicitly excludes these two postures at the deny face / repo-law / escalation
   sites); an armed escalation card carries the model's justification.

### 3.4 Invariants (what does not change)

- **Byte-identical when not armed**: with `ask_on_guardrail=false` (default), every guardrail face is
  byte-identical to the pre-flag doctrine, and the existing fail-closed pins stay green as-is.
- **Never and Auto-Review postures deny regardless of arming**: the arming exemptions of the deny face, repo-law,
  and escalation all explicitly exclude these two postures (completed by the review fixes); denials may differ
  only in receipt wording and never let anything through.
- **Deterministic allow/deny never raises a card**: an unattended scheduled task's initial execution
  short-circuits first in the forwarder, auto-approving/denying deterministically per the task-saved approval
  policy (the `scheduled_tool_should_auto_approve` branch in `forwarder.rs`, priority order preserved as-is); it
  does not go through the interactive approval surface, and it is precisely for this reason that an unattended
  engine's rule profile is always Full (section 5). Scheduled/multi-agent sessions **other than** unattended ones
  keep the same ordering as ordinary conversations.
- **Windowless host deterministic decline**: windowless host lanes (`pinvou agent run` / benchmark / eval boot
  lanes, where `run_windowless_host` / `run_bare_host` set the process-level `windowless_host` marker) have no
  approval-decision consumer, while the shared pool's turn posture (auto-approve) + a stored arming would raise
  promptable faces as interactive approvals — a card no one can answer pins the task to its deadline. The
  forwarder gains a windowless arm after the scheduled-unattended arm: interactive approvals are deterministically
  declined (deny, fail-safe; a force_prompt hold is declined the same way — it is asking precisely for a human,
  and there is no human here), with no pending registered and no approval event emitted; the model sees the denial
  and can reroute, so the task can complete. The scheduled-unattended arm stays first (the task-saved policy's
  non-force deterministic pass-through unchanged). **typed ask (`request_user_input`) closes under the same arm
  (round-11 review)**: `Event::UserInputRequired` is likewise deterministically cancelled on a windowless host
  (cancel_user_input, closed out by the foundation as the tool result), with no pending registered and no
  `chat:user_input_required` sent — otherwise an input card no one can answer pins an agentic run to the
  whole-task timeout (reported as timeout rather than an explainable result). If headless is ever explicitly wired
  to a decision workflow (e.g. a runtime decision API), this arm must converge accordingly into an "explicitly
  connected consumer" predicate.
- **composer `!` is not armed**: a user-typed command input is itself approval (#5191); typed denies still
  hard-deny, execpolicy/sandbox remain hard boundaries; arming applies only to model-originated calls.
- **Subagents mirror the main line**: a subagent that inherited auto-approve also refuses a delegated typed-ask —
  a subagent never silently executes an operation the armed main line would ask about; the held-call guardrail-hold
  routing arm is a fail-safe forward-looking measure (under Bypass the plan decision converges a built-in gate
  hold into Block; the armed subagent face actually reachable today is the typed-ask refusal; review fixes label
  this honestly) (`crates/tui/src/tools/subagent/mod.rs`).

## 4. Approval surface (interactive approval surface)

### 4.1 Event flow and disposition ordering

After the foundation emits `Event::ApprovalRequired` for a promptable tool, the forwarder no longer
auto-approves unconditionally; instead it orders through the pure function `approval_surface_decision`
(`pinvou3-app/src-tauri/src/features/assistant/pending_approvals.rs`), based on the TUI's
`resolve_approval_request_disposition` but with the Plan defensive arm deliberately stricter:

```text
session denial cache hit (exact approval_key)          → Deny(session_denied)
Plan                                                   → Deny(plan_mode)
session grant cache hit (grouping_key) and not force_prompt → ApproveSilently (no event emitted)
force_prompt (non-bypass hold)                         → ShowCard
Yolo and the switch off                                → Deny(guardrail_ask_off)
everything else (Yolo switch on / Auto)                → ShowCard
```

- The cache read path is session-scoped: grants/denials are **session-level** memory; a grant/denial from session
  A must not let anything through for, or carry over guilt to, session B (a cross-session negative test pins this).
- The Plan defensive arm precedes the grant cache (a deliberate divergence from the TUI legacy order): the TUI's
  Plan flow never produces approval events; on the host side a turn may switch to Plan midway, and if an old grant
  still passed silently, "Plan = read-only" would be breached by the cache. The hard-deny direction is fail-safe
  and zero-cost.
- A hard deny calls `deny_tool_call` and broadcasts `chat:guardrail_denied`; `reason` is a stable English
  key (`session_denied` / `plan_mode` / `guardrail_ask_off`) that the frontend i18n maps to trilingual copy by
  key; the payload does not carry the raw input, keeping sensitive parameters out of the event stream.
- Raising a card registers the pending first and then broadcasts `chat:approval_required` (fields one-to-one with
  the foundation event: `approval_id`, `tool_name`, `description`, `input`, `intent_summary`,
  `approval_key`, `approval_grouping_key`, `force_prompt`). A force_prompt's hold is never let through by the
  cache — only this card, clicked by a person in person, can pass.

### 4.2 Card decisions and scoping

The three ApprovalCard decisions go through the `decide_approval` command (`app/commands/interaction.rs`):

| Decision | Cache scope | Semantics |
|---|---|---|
| Allow (allow) | none | This time only |
| Allow for session (allow_for_session) | lossy `approval_grouping_key` | Covers subsequent flag variants (v0.8.37 semantics; round-11 review wording precision: wordlist commands group by flag variant, shell commands outside the wordlist group by the base word — **all operand variants** of the same base word are one family, and path-carrying faces group by the narrower key — file writes by full-input hash, the patch family by the hash of the touched path set plus the main root — so a sensitive path is never wrongly let through by the wide family of an out-of-wordlist command) |
| Deny (deny) | exact `approval_key` | A different parameter needs a new approval request, with no guilt-by-association (#1617 semantics) |

Every decision first validates the decision value (removing the registration is irreversible; an illegal value
returns Err as-is and leaves no trace), then removes the pending registration (the engine-side approval id dies
with the call, and no card may linger), and the cache is written only **after** the engine call **succeeds** — a
failed click must not let through or deny later same-family calls on the user's behalf. The grant/denial caches
mirror the TUI's scoping semantics and are **isolated per session** (cross-session pass-through/guilt-by-
association is forbidden); the two caches never leak scope into each other; `clear_session` removes only this
session's share and does not mistakenly revoke other sessions' grants.

### 4.3 Registry lifecycle

- Everything is memory-resident: after a process restart the engine lazy-spawns and the caches die with the
  process; no persistence needed. The turn-terminal `clear_pending_returning_child_ids` backstop clears the
  session's **pending cards** (on cancel/interrupt there may be no per-item decisions); grants/denials are
  **session-level memory that survives across turns** (the semantics of "allow for this session", consistent with
  the TUI: the TUI clears only on session/workspace change), removed via `clear_session` in the session-cleanup
  handler when a session is deleted. At the frontend, the turn terminal state likewise settles undecided approval
  cards in place into a terminal state (cancelled), no longer clickable.
- On frontend remount / switching back to a session, approval cards replay via `get_pending_approvals` (the same
  restore idea as `get_pending_user_inputs`); the event side dedupes idempotently by `approval_id`, and the
  backend replaces in place on same-id replay without re-raising cards.
- Multi-client convergence: mode/switch changes broadcast `chat:mode_changed` uniformly (desktop emit + remote
  forwarding, payload carrying snake_case `mode` and the session-resolved `ask_on_guardrail`); the frontend writes
  back through the epoch-protected unified convergence point, invalidating in-flight stale reads. All three event
  kinds — approval, guardrail, mode — enter the remote_control event-forwarding allowlist
  (`RUST_FORWARDED_EVENTS`); Web-facing RPC commands remain governed by the existing multi-agent-execution
  denylist — this PR registers three new mode/approval change commands in the denylist and registers new surfaces
  in `web_session_scope` and the access policy; the read-only **policy** is unchanged (the multi-agent execution
  face still rejects write operations).

## 5. Rule profiles (SafetyRuleProfile)

The execpolicy hard-deny ruleset is assembled per session mode (`features/assistant/safety_deny_rules.rs`):

| Profile | Serving lane | Size | Composition |
|---|---|---|---|
| Full | Plan / Auto (and the default for every caller without session information) | 8,763 rules | the never-sensible baseline family plus the sensitive family (per-file credential destruction, direct exfil upload, persistence, sudo, etc.), byte-identical to the pre-profile composition (snapshot test pin) |
| Floor | Yolo | 3,555 rules | keeps only the never-sensible baseline family; the sensitive family is absent |

- **The profile only chooses which rule families to assemble; it never rewrites rule strings and never relabels
  Deny→Ask**: every emitted rule stays a typed `Deny`, and a hit family still short-circuits under every approval
  posture. For an armed session, whether the deny face "raises a card or silently denies" is decided by the
  foundation deny face's arming mapping (item 5 in 3.3), orthogonal to the profile.
- **The sensitive/baseline family split is semantic, not spelling-based** (v3.5): the per-credential
  **direct-upload** face written in the `C:\Users\me\...` spelling belongs to the sensitive family (Full-only),
  consistent with the same family in the `~`/`%userprofile%` spellings — previously it landed in Floor with the
  parsing-host family, making the Yolo posture depend on which spelling the model happened to write. The
  destruction face and the bare-root (whole-directory/whole-profile) direct-upload faces stay Floor across all
  spellings (consistent with the established trade-off of the literal prefix family, test pin).
- Floor's missing sensitive family is product intent: Yolo trades the sensitive deny face for the armed card,
  keeping only the "never sensible" baseline (whole-disk mkfs, sudoers writes, etc.). A Yolo session therefore
  genuinely loses the sensitive family's mechanical gate — a deliberate trade-off, not an oversight.
- **The engine lane** (`SafetyRuleProfile::for_engine_lane`): an unattended engine is **always
  Full** — a stored Yolo is the interactive posture, and no one watching the cards is precisely the scenario where
  the mechanical gate must not be dismantled; an interactive engine (including the interactive continuation of a
  scheduled session, desktop host) takes its profile by mode. Both forms of "unattended" land in Full: the
  scheduled-run engine, **and the windowless host** (`run_windowless_host`'s headless/agentic lane — the round-9
  P1 fix: previously only the scheduled flag entered the profile decision, so `pinvou agent run` reusing a GUI
  session saved as Yolo would run unattended at Floor with the credential-destruction/exfil/persistence mechanical
  gates wholly absent and no card to make up for it; spawn config and hot refresh share the same lane decision,
  source-pinned on both sides).
- Hot-swap discipline: a mode change triggers `Op::ChangeMode` + `SetPermissionRuleset` in the same publish
  sequence; re-sending the same profile rebuilds the same ruleset, so callers need not track the old profile.
  Spawn-time arming queues before the engine entry is published, so the first turn already runs on the session's
  lane; hot refreshes of the super-permission/connector/skill switches also assemble at each session's own
  profile.

## 6. Mode state and persistence

- The session mode lives in an in-memory map + the `_session_mode_states.json` sidecar (persisted per session for
  all sessions; the legacy `_code_mode_states.json` migrates when the new file is absent and is persisted to disk
  immediately). Each in-session switch writes only that session's record; lane global defaults are written only by
  an explicit draft-state switch (two-lane semantics: `set_mode_default` writes settings.json's
  `mode_defaults.work` / `code_permission.last_mode`, and in-session switches do not flow back to the global). A
  save failure propagates as `Err` along the command return path (review JensenChen28 5780647695): the
  bridge/frontend keeps the authoritative state and reports the error explicitly, and must not accept the old
  default as success while drawing the chip as the target — otherwise the materialized session would run at the
  old mode while the UI shows it switched.
- **`pre_plan_mode` (approving a proposal is not approving full-auto)**: entering Plan records the mode it came
  from, and after accept_plan it returns to that mode (fallback Auto) with a hot sync through the same publish
  sequence; it is memory-resident only — a Plan session after restart re-resolves through
  `resolved_default_mode`, and the process-relative value is not pinned into the sidecar. The headless path
  (`set_mode_and_persist`) requires the disk write to succeed, and a Plan → Yolo reopen drift is treated as a
  failed run. For an unmaterialized session whose first mode write is Plan, the minted entry resolves by
  `resolved_default_mode` (not the historical Yolo sentinel), and accept returns to the default the chip displayed
  (Auto) — pinned by a behavior test. The Web surface validates **the execution mode that will be restored**
  before admitting accept_plan (`get_mode_state` projects `post_accept_mode`, the mirror of the claim's restored
  row): an accept that would fall back to Auto is explicitly rejected without opening a turn (review zhuowp
  5780927743, item three; a Plan card entered from Auto passed the mode === "plan" gate check yet would execute as
  Auto, while the approval card has no decision surface on the Web side).
- **`SerializableMode::Auto` backward compatibility (downgrade data preservation, review P2)**: `"plan"/"yolo"`
  in old sidecars deserialize as usual; **the shared table never writes the `"auto"` literal** — the same
  discipline as the settings.json lane's `*_auto` companion flag: an explicit Auto choice folds to `"plan"` in
  `_session_mode_states.json`, and the real choice is recorded with a `true` flag in the
  `_session_auto_modes.json` companion table (write order: the shared table first, then the companion table; an
  interruption usually leaves only the conservative `plan` — the sole residual window is the **Plan downgrade** of
  an already-flagged Auto: the shared-table entry writes `plan` anyway, so the whole interruption window lands on
  the companion-flag deletion, and a crash before the deletion leaves the orphaned `plan`+flag combination, which
  a new binary reads back as Auto by the flag — one mode wider than the Plan the user explicitly chose yet still
  below Yolo; the write comment in `sessions/mode_state.rs` registers this as a known bounded residual window).
  The downgrade of "old binary + shared PINVOU3_HOME" therefore no longer pays the cost of whole-table
  quarantine/rebuild from an empty table:
  - **Read path**: a pre-tri-mode binary's whole-table strict deserialization **succeeds**; Auto sessions
    conservatively downgrade to Plan (per-item approval) in its view, while the other sessions' choices are
    preserved as-is.
  - **Write path**: any change by an old binary mutates the whole table as usual without triggering quarantine;
    other sessions' persisted modes never leave the active sidecar.
  - **Upgrade read-back**: the new binary overlays the companion flag at boot load and at `durable_mode_entry`
    reads (`plan` + `true` flag = explicit Auto), so tables an old binary wrote do not lose Auto choices; a
    companion flag never resurrects apart from its map entry — when a shared-table entry is missing, the map is
    authoritative.
  Behavior pin: `session_auto_mode_is_downgrade_parseable_via_companion_flag` (the legacy-vocabulary reader parses
  the whole table + Auto is still recovered by the companion flag after an old binary's rewrite).
- **The old-writer compatibility boundary (an explicit compatibility decision, review P2)**: an old binary does
  not recognize the companion flag, and its writes to the shared table cannot distinguish "a passive rewrite of a
  collapsed Auto" from "a user's explicit re-choice of Plan" — so `plan` + `true` flag is always upgraded to
  explicit Auto at read-back. The supported interoperability boundary is thereby drawn: **an old binary cannot
  re-choose a flagged session to Plan**; falling back to Plan requires the new binary (an explicit Plan write from
  new code also clears the companion flag). No mtime heuristics are deliberately done, and the persistent format
  is not extended for old writers; with maintainer confirmation this trade-off is sealed as-is.
- The default resolution matrix when no persistent record exists:

| Session shape | Default |
|---|---|
| code session / workspace-bound plain session | `code_permission.last_mode` (None → Plan) |
| Unbound plain session (the work lane) | Auto; an explicitly chosen work-lane global default (plan/yolo/auto) is applied by the frontend at session materialization as that session's own record |
| Unattended scheduled engine / windowless host | The interactive approval surface does not apply; the rule profile is always Full (section 5) |

## 7. Model awareness (authority transition)

The engine records per turn the posture reported in the `<turn_meta>` block (`TurnMetaPosture`: mode, approval,
arming). When the host changes mode/posture/arming between turns, the next block renders exactly one line:

```text
Authority transition (host): agent (Full Access, guardrail prompts armed) -> agent (Ask, guardrail prompts disarmed); reason: host_authority_change
```

Any number of `ChangeMode` flips within a turn folds into a single transition in the next block; a turn whose
posture did not change adds zero lines (the #3947 byte-stable doctrine). `/preview-request` reads the same
engine-computed value, so preview and the live wire are byte-identical. This line measures only "standing posture"
changes: provenance-narrowed turns (AgentMail, imported transcripts, and other non-authoritative inputs) run a
per-turn lowered posture, reported faithfully by the narrowing receipt-line family (`Authority narrowing:` /
`Authority transition:`) — they neither trigger the `(host)` line (which would mislabel a per-turn narrowing as a
host flip) nor pollute the standing-posture baseline (otherwise the next authoritative turn would render the
narrowing being lifted as a reverse host flip no one made). The narrowing predicate follows whether the turn
actually produced a narrowing event (`narrowing.is_some()`): when the standing posture has no lowerable auto
authority to begin with (e.g. Ask), the turn produces no narrowing event and is treated as an authoritative turn —
its rendered data is always true; when a genuine host flip lands in the same window, the next authoritative turn
renders it starting from the pre-narrowing standing posture. A narrowing downgrade under Auto-Review
(Auto→Suggest) and a Bypass downgrade likewise carry structured narrowing events, no longer silently. Before the change, a posture change could only be "observed" by the model through the next turn's standing posture line,
and the behavior shift (suddenly approval-free / suddenly demanding approval) read as unexplained; this line lets
the change state its own reason (`crates/tui/src/core/authority.rs`; mechanism registered in foundation PR (b)).

## 8. Tests and registration

- **Foundation** (CodeWhale#131): this behavior adds 24 `forkguard_` pins — commit
  `998ffe27e` 7 pins + re-review hardening 6 pins + review fix 1 pin + re-review fix two `5c13fc561`
  4 pins (arming-publish stickiness, armed composer hard deny, armed Auto-Review escalation always denies,
  preview planned posture hash-identical to the live wire) + re-review fix three `f972884e5`
  2 pins (the runtime pump respects force_prompt; NotStarted does not swallow the transition) + re-review fix four
  `b94cc3175` 2 pins (sync resets the recorded posture; an isolated turn neither records nor consumes) +
  re-review fix five `1785dae15` 2 pins (a provenance-narrowed turn neither falsely emits the host line nor
  pollutes the standing baseline; an Auto-Review narrowing downgrade carries a structured receipt), pinning one by
  one the five armed faces above, Auto-Review/Never always denying, the grant cache never silently consuming an
  armed hold, the composer not being armed, the two subagent mirror faces, transition rendering/folding/preview
  consistency, and that arming cannot be rolled back.
  The forkguard behavior-name total is 255 → 279 (candidate-head basis); a whole-tree census at the re-pinned
  maintenance head `11cba403d` gives **301** (the increment comes from existing pins that landed with
  #123/#124/#129/#133/#134/#135, not from #131 itself); the gate command is
  `cargo test -p codewhale-tui --lib --locked forkguard_ -- --test-threads=1`
  (measured at the re-review fix five head `1785dae15`: 264 passed / 0 failed, with the existing fail-closed pins
  green as-is; measured at the maintenance head: 285 passed / 0 failed;
  the protocol crate 71/0 including a new serde pin for the legacy ChangeMode payload).
  Counting basis (round-8 L10-5): 279 is the candidate-head census of **behavior names** (`forkguard_*` distinct
  test names; repeated occurrences of the same name count once), 264 is the number of **test cases that actually
  passed** at that head (one behavior name can derive multiple cases/parametrizations, and the 24 candidate pins
  are already included at that head); the two numbers need not be equal; 301/285 are the same-basis census and
  measurement at the maintenance head after re-pinning.
  The 24 names, one by one:
  `forkguard_armed_full_access_forced_hold_resolves_to_prompt`,
  `forkguard_armed_full_access_hold_prompts_instead_of_blocking`,
  `forkguard_armed_repo_law_hold_prompts_instead_of_blocking`,
  `forkguard_armed_full_access_typed_ask_rule_prompts_instead_of_auto_running`,
  `forkguard_armed_sandbox_escalation_prompts_instead_of_failing_closed`,
  `forkguard_armed_deny_face_becomes_guardrail_prompt_unarmed_stays_block`,
  `forkguard_armed_child_refuses_delegated_typed_ask_even_under_auto_approve`,
  `forkguard_turn_authority_install_never_rolls_back_published_arming`,
  `forkguard_authority_transition_between_renders_the_host_change`,
  `forkguard_turn_meta_authority_transition_renders_only_on_change`,
  `forkguard_live_turn_meta_builder_reads_the_pending_transition`,
  `forkguard_preview_manifest_hashes_the_same_authority_transition_line_the_wire_sends`,
  `forkguard_apply_change_mode_status_suffixes_report_the_arming_change`,
  `forkguard_turn_start_detector_records_the_posture_and_renders_the_transition_once`
  (added by the review fix, byte-for-byte pinning of the detector's record and its one-shot consumption; the
  deny-face pin extends the Auto-Review always-deny face),
  `forkguard_pending_arming_survives_arming_free_authority_publishes`,
  `forkguard_armed_composer_shell_keeps_the_hard_deny_face`,
  `forkguard_armed_auto_review_escalation_stays_fail_closed`,
  `forkguard_preview_transition_uses_the_planned_posture_not_the_stale_session`
  (added by re-review fix two; all four verified to turn red by mutation),
  `forkguard_pump_never_auto_approves_a_force_prompt_hold`,
  `forkguard_a_not_started_turn_neither_consumes_nor_records_the_transition`
  (added by re-review fix three; both verified to turn red by mutation),
  `forkguard_sync_session_clears_the_recorded_turn_meta_posture`,
  `forkguard_an_isolated_turn_neither_consumes_nor_records_the_transition`
  (added by re-review fix four; both verified to turn red by mutation; this wave's third fix extends the pump
  pin's payload assertions and adds no new pin names),
  `forkguard_a_provenance_narrowed_turn_neither_emits_the_host_line_nor_pollutes_the_baseline`,
  `forkguard_an_auto_review_turn_narrowed_by_provenance_still_records_its_narrowing`
  (added by re-review fix five; both verified to turn red by mutation).
- **Application side**: `forkguard_approval_surface_record_dedupes_by_approval_id` /
  `forkguard_approval_surface_remove_returns_entry_and_cleans_empty_session` /
  `forkguard_approval_surface_grant_and_denial_scoping` /
  `forkguard_approval_surface_clear_session_wipes_memory` /
  `forkguard_approval_surface_decision_truth_table` (the disposition truth table pinned row by row),
  `forkguard_ruleset_profile_full_is_the_pinned_composition` / `_floor_keeps_floor_drops_sensitive` /
  `_sudo_toggle_interplay_unchanged` / `_floor_engine_behavior` /
  `_engine_lane_forces_full_for_unattended_runs` / `_floor_count_is_pinned`
  (Full 8,763 reuses the existing `rule_snapshot_is_stable` snapshot — after round-13's
  resolved-home trailing-slash exfil face added 7 rules it was re-pinned from 8,729, after round-15 review C's
  Windows trailing-backslash tilde-spelling face added 6 it settled at 8,742, after round-19's
  brace-spelling face `${home}\` added 3 it settled at 8,745, and after round-20's
  stock whole-home tokens `${env:userprofile}`/`%homedrive%%homepath%`/`%homepath%` in bare and
  trailing-backslash spellings added 18 it settled at 8,763; Floor 3,528 was re-pinned from
  3,521 in the same batch, settled at 3,534 in round-15, at 3,537 in round-19 and at 3,555 in round-20; the lane matrix has its own dedicated tests), plus
  the switch persistence and default-seeding regressions in the sessions/commands domain; the frontend lane covers
  the tri-mode chip, the switch, and the ApprovalCard.
- **Registration**: fork-guard's `EXPECTED_HEAD`/`EXPECTED_COMMITS` and the fingerprint table
  (`scripts/fork-guard.sh`), plus the `docs/fork-modifications.md` T2 addendum, land with this PR's Stage-4
  commits; CodeWhale#131 has merged into the public maintenance branch as squash `e2a0e14fd`, and the gitlink and
  `EXPECTED_HEAD` have been re-pinned to the maintenance head `11cba403d` (the candidate-period registration basis
  is in the 2026-10-31 consumption-baseline section of `docs/fork-modifications.md`).

## 9. Known boundaries

- **Aux-conversation (aux) mode initialization and Web admission**: the aux lane has no mode controls of its own
  (the panel never activates the session, and the parent session's mode chip never reaches it); its mode record is
  initialized at both entry points — creation and reuse — to the parent session's currently effective mode (Auto
  is substituted with the safe mode Plan — the posture the Web admission gate rejects; an explicit Plan/Yolo is
  inherited as-is, and a Yolo aux passes through the gate — the aux lane itself enforces zero tools). aux records
  materialized in the pre-tri-mode era are back-initialized at first reuse, and this takes effect only for records
  with "confirmed absence": transient read failures, parse corruption, and quarantined states are always skipped
  (unknown is never absent in a write path) and self-heal the next time the panel is opened; an already-persisted
  mode (including a Yolo pin inherited from a parent session that has since switched) is an existing choice, and
  reuse never re-resolves it. Back-initialization goes through `set_mode`'s best-effort persistence variant, and a
  disk-write failure is closed by the next reuse's retry (review JensenChen28 5801173510 + 5802339766).
- **An old writer cannot re-choose a flagged session to Plan**: `plan` + companion flag is always upgraded to
  explicit Auto at read-back (the old format cannot distinguish "a collapsed Auto" from an explicit Plan in the
  old version, see the compatibility decision in section 6); this is a deliberately sealed interoperability
  boundary, not a defect.
- **The Web side explicitly does not support approval cards**: the four event kinds are already in the Relay
  forwarding allowlist (`src/platform/web/access-policy.json`: approval_required /
  approval_resolved / guardrail_denied / mode_changed). The Web bridge renders no approval card:
  `decide_approval` is a forward-compatible registration in the access policy, with no export on the Web bridge
  surface and no caller today (round-13 review correction: `set_session_mode` and
  `get_pending_approvals` have had Web callers since round-9 — the lane-default landing for session
  materialization and bootstrap's backfill read of in-flight approvals; `set_ask_on_guardrail` remains a
  forward-compatible registration) — the desktop dual-lane (native + code) is the product surface for approval
  decisions. **An approval request is not silent on the Web side**: `chat:approval_required` explicitly lands
  a trilingual system notice (an unsupported-capability signal), `chat:guardrail_denied`
  maps to trilingual copy by the stable key, and `chat:mode_changed` goes through the authoritative convergence
  point so the chip follows; a turn waiting for approval shows up on the Web side as "a wait with a visible
  reason" (a desktop decision or the Web user stopping generation), not a spinning indicator without feedback.
  The full Web approval workflow remains follow-up work.
- **The approval surface does not survive a cross-process restart**: pending cards and the grant/denial caches are
  process memory; after a restart the turn requests approval again (engine-side approval ids die with the process
  anyway). This is a deliberate trade-off, not a persistence defect.
- **Multi-agent bootstrap-time default resolution registers before the code-session predicate**: the bootstrap
  load of `_multi_agent.json` mints entries with `resolved_default_mode` during store construction, while the
  code-session predicate registers late in the Tauri setup — a multi-agent code session that never switched modes
  may display Auto (rather than Plan) after a restart. Once an entry is materialized it is never re-seeded from
  preferences/the predicate; one explicit switch by the user corrects it; the fix needs a bootstrap-order change
  or late-bound resolution and belongs to follow-up work.
