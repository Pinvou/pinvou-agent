# session-reader builtin tool (session creation) — design and acceptance

> Status: **implemented** (2026-09-30; translated to English 2026-10-06 per the developer-documentation language rule). Companion contract: `docs/builtin-toolset-contract.md` §9 registry row `create_session`.
> Baseline: split from `feat/scheduled-messages` (which carries app-automations CRUD and scheduled messages, PRs #628/#629) into its own PR per the round-3 review. The implementation fully reuses the family's verified "MCP tool + spool + app-side watcher + short result-marker wait" skeleton, with **zero CodeWhale changes**.
> Usage: after implementation, accept item by item against the §5 matrix; the matrix ids map one-to-one onto the tests.

---

## 1. Background and goal

The session model previously could only message **existing** sessions (`send_message_to_session`); it could not create new ones. Goal: from **any session**, the model can create a new session through a builtin tool (optionally attaching a first message that starts it immediately), completing the delegation chain "open a session to do X and report back".

## 2. Design decision record

| Decision | Outcome | Rationale |
|---|---|---|
| Ownership | session-reader family (contract §2 "one family = one server") | Session-domain writes belong to the session family; `send_message_to_session` is the precedent; app-automations is the scheduling-automation family |
| Tool shape | A single L1 tool `create_session`, `first_message` as an optional parameter | Contract §4.2 prefers one tool + mode; no need to split create / create_and_chat |
| Focus semantics | **Never `set_active`** | A tool-created session appears silently in the list (`session:list_changed` refreshes both ends automatically) and the user opens it themselves; this is the one deliberate divergence from the Tauri command's default |
| Inheritance semantics | Always "inherit app defaults, not the caller's state" | Model = the app's new-session default (`default_model_for_new_session`), overridable by an explicit `model_id` (must name a saved model; unknown ids are rejected); workspace = app default (`bridge.workspace`), bindable by an explicit `workspace_path` (absolute + existing directory, canonicalized before spooling — review round-3 M2); mode/persona/knowledge sets take fresh defaults. Reusing `create_session_record` semantics comes for free — zero new semantics |
| First message | Becomes the new session's opening **plain-text** user turn (`deliver_messaging_turn`), with no cross-session header block | It is the opening instruction, not a relayed message; isomorphic to `web_access_create_session_and_chat` |
| Explicit title | `set_title` after `create_new` | Auto-naming only triggers while the title is still the default "New chat", so an explicit title is naturally never overwritten by the first turn |
| Approval | L1 typed Ask registered (latent pin); panel-parity, unbound only (round-7 D): the Plan mirror applies to UNBOUND sessions; workspace-bound sessions resolve via the code lane's last mode, exactly like the panel; the unattended deny channel is TURN-LOCAL — an unattended run can relay through send_message_to_session into a normal session whose unshielded turn can call create_session (the #627 relay property; depth/rate caps are the future-work closure) | The Ask rule never prompts for MCP tools in any current mode; the review surfaces are the audit trail and the session list. Unattended `sched-`/`eval_`/`aux-` requesters are denied by validation, and unattended turns deny the tool via the engine channel — the attended→created recursion (A creates B whose opening message creates C…) is closed AT THE TOOL-NAME CHANNEL for the watcher-delivered opening turn (round-9 M6: the delivery applies the engine-side `unattended_disallowed_tools` shield before the turn, so B's automated first turn cannot CALL create_session/create_goal/the scheduled-task writes — round-10 M5(f): the shield is a tool-name deny list, not a sandbox; under default Yolo the shielded turn's exec_shell can still write the user-writable spool directly and reach the same watcher, so the closure holds at the named-tool channel only); the shield persists on that engine until recycle — a user opening the fresh session inside the idle window inherits the list, fail-closed and disclosed); the #627 relay property (an unattended turn relaying through send_message_to_session into a normal session) and depth/rate caps beyond it remain family-pass future work |
| Failure semantics | At-least-once, window stated honestly (round-1 M-B, rounds 2-7 wording) | The marker is written AFTER the post-create steps (binding → title → first-message delivery, up to the 30s bound). RETRYABLE errors get 3 attempts per attempt-EPISODE — the budget clears on every terminal disposition and a terminal failure marker re-arms the key on the next call (the pending note invites the same-key re-poll), so one key can see multiple 3-attempt episodes in one process; only the restart-rebuy is separate; permanent validation classes (unknown model, invalid workspace, isolated requester) poison on the first failure — so a crash anywhere in that span, or a persistent marker-write failure, can duplicate the session (up to 3 per episode). (Round-10 M4 reword: the round-1..9 "3 TOTAL per key per process" claim was false as written.) Every created session that reaches the Ok arm is audited and announced before the marker write (the audit floor — the non-UTF-8-name quarantine carries no workspace audit (the log line is the trace); the ceiling arm audits parseable records with a usable sender); a workspace-binding failure rolls back and deletes the whole empty session (mirroring the `create_session` command), making the request retryable as a whole |

## 3. Components and data flow

| Component | Location | Responsibility |
|---|---|---|
| MCP server | `pinvou3-app/resources/mcp-servers/session-reader/server.py` (modified) | Tool surface: validation (caps / absolute path / isdir probe / isolation prefixes / **required sender**), atomic spool write `~/.pinvou3/session-requests/spool/<sha256(from\|create\|key)|uuid>.json`, short result-marker wait ≤5s (hit → sessionId returned; timeout → `delivery:"pending"`) |
| manifest | `manifest.json` in the same directory (modified) | `mcp_tools` + `tool_features` register `mcp_session-reader_create_session` → new feature `session-creation` (on by default); version 1.1.0 → 1.2.0 |
| Creation watcher | `pinvou3-app/src-tauri/src/features/session_creation/mod.rs` (new) | 1s poll drain, re-validation (the spool directory is user-writable; server-side checks are not trusted), poison-file quarantine `spool/failed/`, ≤3 retries, 14-day retention pruning of terminal state |
| Domain entry | `PoolCreator` (same file) | Validate/resolve (workspace canonicalization, model_id resolved against saved models — extracted into `resolve_creation_model`, unit-tested) → `SessionStore::create_new` (the panel's own pipeline) → `bind_session_workspace` (failure rolls back) → `set_title` (failure is warn-only — the title stays default) → audit → `session:list_changed` → result marker (audit/notify BEFORE the marker — §2's floor) |
| Approval rule | `features/assistant/platform/bridge.rs` (modified) | `SESSION_CREATE_TOOL` constant + ToolAskRule registered in `scope_deny_ruleset_with` |
| Mount | `lib.rs` (modified) | The same app-lifetime spawn next to the messaging watcher |
| Contract doc | `docs/builtin-toolset-contract.md` (modified) | §9 registry row |

Audit: success → `session_create` in the requester's execution root (tool/session_id/title/workspace_bound/model_id/first_message_delivered); failure/quarantine → `session_create_failed`. Event: `session:list_changed {id, action:"created"}` (same payload shape as the `create_session` command; already inside the web access-policy allowlist and both ends' listeners — zero frontend changes).

## 4. Disclosed limitations and boundaries

- **Background first turn (revised round-9 M6)**: when the user has not opened the session, the `first_message` first turn runs autonomously in the background under the resolved mode (Yolo by default, or the work-lane Plan default when set — round-4 R2's mirror); NOTHING pauses and no approval prompt exists today (mutating-MCP Ask rules resolve to silent Allow under full-auto). The watcher-delivered opening turn is **unattended-dispatched**: before the turn reaches the new session's engine, the delivery sends `Op::SetDisallowedTools` with the `unattended_disallowed_tools` list (create_session / create_goal / update_goal / the three scheduled-task write tools), so an injected opening instruction cannot recursively create sessions/goals/task-writes through the NAMED tools (the deny list is not a sandbox: exec_shell remains able to write the spool directly under default Yolo — the channel-scope disclosure above); goal-tool continuation turns on the same engine inherit the shield. The shield persists until the engine is recycled OR the user's first attended send — round-11 M1: `send_user_message` detects the mark and restores the ordinary per-session catalog before reserving (the scheduled family's pre/post-turn evict closes the same invariant by respawning; this is the delivery path's equivalent), so a user who takes over the session regains the full catalog instead of silently losing six tools for the engine's lifetime. A connector/skill toggle broadcast in the shield's open window replaces the catalog early (the pre-existing minor-1 race; disclosed). Beyond this turn the exposure matches the existing `send_message_to_session` delivery behavior; the creation is audited — the audit trail and the session list are the review surface.
- Frontend delta: the three uiToolDetails overlay strings (no logic changes): the timeline renders the generic MCP tool card; a dedicated result card and "jump to the new session" are future work (P2).
- No new Tauri commands, no new events → no access-policy or protocol-test changes; web behaves identically to desktop.
- Existing users get the tool automatically on upgrade (builtin packages re-release by byte-for-byte content comparison, `ensure_package_released`).
- Zero fork changes; `docs/fork-modifications.md` needs no update.

## 5. Acceptance matrix

| # | Scenario | Expected result | Verification | Priority |
|---|---|---|---|---|
| A1 | End-to-end creation | The model calls the tool; sessionId/title returns within ≤5s; the new session appears in the list **without stealing focus** | Manual QA (live model) | P0 |
| A2 | Short-wait synchronous return | After the watcher writes `.done`, the server hits the marker and returns `{ok,sessionId,title}` | ● python `test_marker_hit_returns_session_ids` | P0 |
| A3 | Timeout returns pending | With no watcher, returns `{ok:true, sessionId:null, delivery:"pending"}` — not an error | ● python `test_wait_timeout_returns_pending_not_error` | P0 |
| A4 | First-message delivery | The new session's first turn is a plain-text user message (no cross-session header block); the engine starts immediately | Manual + code path (`deliver_messaging_turn`) | P0 |
| A5 | Live list refresh | `session:list_changed` drives both the Tauri and Web ends to refresh | Manual, both ends | P1 |
| B1 | Argument caps | title>200 / first_message>32k / model_id>200 / key>128 rejected | ● python `test_title_and_message_caps` etc. | P0 |
| B2 | workspace validation | Relative / missing / over-long rejected; a valid directory passes (two layers: server isdir probe + watcher canonicalization) | ● python `test_workspace_must_be_absolute_existing_dir` + ● python `test_workspace_path_is_canonicalized_before_spooling` | P0 |
| B3 | Isolation prefixes | sched-/aux-/eval_ (case-insensitive) rejected as from_session at every layer (server + watcher re-check) | ● python + ● Rust `validation_rejects_bad_shapes` (incl. `EVAL_x`) | P0 |
| B3b | Required sender | An omitted from_session is rejected up front (the omission hole of round-3 M1 is closed server-side; the unattended recursion shield is the engine-side deny channel) | ● python `test_from_session_is_required` + ● Rust engine `unattended_turn_denies_scheduled_task_write_family` | P0 |
| B4 | model_id resolution | A non-saved model id → failure marker (the error reaches the model); a valid id → the session's model sidecar binds | ● Rust `resolve_creation_model_rejects_unknown_ids_and_defaults_on_blank` + manual | P1 |
| B5 | Title semantics | An explicit title persists and is never overwritten by first-turn auto-naming; no title + first_message → auto-naming | Manual + ● semantics (`apply_default_session_title` only triggers on the default title) | P1 |
| C1 | Idempotent retry | Retrying the same key reuses one spool file; a completed key returns the recorded result with `duplicate:true` and does not recreate | ● python `test_idempotency_key_reuses_one_spool_file` / `test_preexisting_marker_reports_duplicate_result` | P0 |
| C2 | Watcher idempotency | A `.done` success marker suppresses recreation; a failure marker allows replay | ● Rust `done_marker_suppresses_recreation` / `failure_marker_lets_retry_reapply` | P0 |
| C3 | Poison quarantine | A tampered spool record (isolation prefix / out-of-bounds / schema drift / over-cap) → `failed/` + `{ok:false}` marker, nothing created | ● Rust `tampered_spool_is_quarantined_without_creating` | P0 |
| C4 | Persistent failure | After ≤3 retries, quarantine + failure marker (the waiter receives the failure instead of hanging) | ● Rust `persistent_failure_retries_then_quarantines_with_failure_marker` | P0 |
| C5 | Untrusted id | The spool JSON `id` field never controls watcher paths (the file name is the key) | ● Rust `id_field_is_never_trusted_for_watcher_paths` | P1 |
| C6 | Marker session_id validated | A pre-placed marker's session_id is charset/type-checked before being echoed (a fabricable success does not hand the model an arbitrary string) | ● python `test_result_marker_session_id_is_validated` | P1 |
| D1 | Ask rule | The ruleset carries the `mcp_session-reader_create_session` Ask (no command constraint); read tools stay ungated | ● Rust `scope_deny_ruleset_asks_for_session_create` | P0 |
| D2 | Name drift pin | The Ask tool name is byte-identical to the embedded manifest registration | ● Rust (drift pin inside the same test) | P0 |
| D3 | Audit | Success/failure land `session_create` / `session_create_failed` respectively | ● Rust `creates_session_writes_marker_audits_and_notifies` | P1 |
| D4 | Error sanitization | Failure markers and server errors carry no absolute host paths | ● Rust (marker assertions) + ● python `test_spool_errors_do_not_leak_host_paths` | P0 |
| E1 | Feature switch | `session-creation` off → the server returns a structured `feature_disabled` and the watcher pauses creation; the registry's union semantics keep the other session-reader features intact | ● python FeatureGate suites (the Rust-side watcher-pause test is future work) | P0 |
| E2 | Registry aggregation | The feature registry contains `session-creation` owning exactly this tool | ● Rust `registry_aggregates_session_reader_features` | P0 |
| F1 | Default install/upgrade | After startup on an existing `~/.pinvou3`, server.py/manifest re-release automatically; the tool is available | ● `ensure_package_released` mechanism (pre-existing) + manual | P1 |
| F2 | Smoke matrix | 8 packages' tools/list compare equal; session-reader's four tools; the creation journey (isolation rejection / pending / spool / duplicate) | ● `scripts/mcp-server-contract-smoke.py` | P0 |
| F3 | Gates | cargo test/clippy/fmt, python unittest, smoke, architecture-guard, fork-guard --fast all green | §6 | P0 |

● = landed test.

## 6. Test map

| Test | Covers |
|---|---|
| `scripts/tests/test_session_reader_server.py` (the CreateSession* groups + required-sender/canonicalization/marker suites) | A2 A3 B1 B2 B3 B3b C1 C6 D4 E1 (precondition) |
| `scripts/mcp-server-contract-smoke.py` | F2 (incl. the creation journey) |
| inline tests in `features/session_creation/mod.rs` | B3 B4 (partially) C2 C3 C4 C5 D3 D4 |
| engine.rs `unattended_shield_tests` | B3b (the unattended deny channel) |
| ruleset tests in `features/assistant/platform/bridge.rs` | D1 D2 |
| `features/marketplace/{builtin,types}.rs` | E1 E2 |
| Manual QA | A1 A4 A5 B4 B5 F1 |

## 7. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Prompt-injection mass-creating sessions and flooding the list | Required-sender audit trail + timeline visibility + the feature switch (hides the tool and pauses creation) + the pending-file ceiling; the attended→created chain's first hop is tool-name-blocked at the delivered turn (round-9 M6 — channel scope disclosed), per-hop audits bound the rest (round-2 M2) |
| Duplicate creation (marker window) | The window spans the whole post-create sequence (binding/title/delivery, up to 30s) plus any creator error retried ≤3 times — broader than messaging's marker-gap; bounded by the per-session audit lines and the ≤3 retry cap |
| A post-create step failing leaves a "half-configured" session | All best-effort + failure audits; a binding failure rolls back the creation (retryable); a failed first delivery can be re-sent by the user/model through the messaging channel |
| The background first turn auto-approving ordinary tools under default Yolo | Identical to the existing message-delivery behavior (the precedent already ships); L1 write tools carry latent Ask rules (no prompt today); the opening turn carries the unattended shield (round-9 M6 — recursion via create_session is blocked there); disclosed in §4 |
