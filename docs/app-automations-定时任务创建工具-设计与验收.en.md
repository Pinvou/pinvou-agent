# app-automations Builtin Tool (Session-Created Scheduled Tasks) — Design and Acceptance

> English mirror of [`app-automations-定时任务创建工具-设计与验收.md`](./app-automations-定时任务创建工具-设计与验收.md); the Chinese original is authoritative on any divergence. Added per `CONTRIBUTING.md`'s English requirement for new developer documentation.
> Status: **implemented** (design finalized 2026-09-29, landed with this branch; §9 is the CRUD-stage addendum, also implemented). Companion contract: `docs/builtin-toolset-contract.md` (the §9 registry row has been updated from "not initiated" to registered).
> Baseline: `main` — this design depends on the builtin toolset contract, the `session-reader` package, and `features/messaging` (at writing time all three were reviewed on the luzeyang-dev line, not yet merged to main; all have since landed: messaging via #627, session-reader built-in via #585/#586).
> Implementation pattern: fully reuses the "MCP tool + spool + app-side watcher" skeleton of `send_message_to_session` (`features/messaging`, landed via #627), opens a **new capability family** `app-automations`, zero CodeWhale content changes (the gitlink follows the base main's #721).
> Usage: review section by section before implementation; after implementation, accept item by item against the §5 acceptance matrix — the feature is complete only when every P0 row passes; matrix ids are back-referenced by the §6 test mapping.

---

## 1. Background and Goals

The session model currently has no usable channel for creating scheduled tasks:

- The base `automation` tool (`CodeWhale/crates/tui/src/tools/automation.rs`) is registered in app sessions but its manager is never attached, so every call fails ("AutomationManager is not attached");
- The existing "create via conversation" flow relies on a prompt (`SCHEDULED_TASK_CHAT_PROMPT`, `features/scheduled/tasks.rs::SCHEDULED_TASK_CHAT_PROMPT`) to constrain the model into emitting a `scheduled-task-draft` block, with the frontend calling `create_scheduled_task` on its behalf — available only in the scheduled-tasks panel's dedicated guided session.

Goal: in **desktop Operate/Yolo sessions**, the model can create scheduled tasks directly through a builtin tool, landing in the exact same persistence path as the panel (the same `ScheduledTaskState::create_task` domain function, the same data directory `~/.pinvou3/automations`), while satisfying the L1 safety requirements of contract §5.

## 2. Design Decision Record

| Decision | Conclusion | Rationale |
|---|---|---|
| Placement | **New capability family** with a new MCP server `app-automations` (contract §2/§8) | Scheduling is an independent capability family; future automation tools such as run-now or memory grooming can join it; avoids bloating the session-reader family |
| Interaction confirmation | Creation and metadata updates apply immediately. Prompt/rrule replacement returns `confirmation_required`; the user reviews/submits in the existing panel. ~~Delete also returns `confirmation_required`~~ (round-36 revision: that half was the superseded round-32 proposal — delete of an unambiguous target executes directly per the §5 exception, without `confirmation_required`; see contract §5/§9 and the tool descriptions). | The prompt/rrule panel review was an explicit human decision on 2026-10-21; typed Ask rules are not an authorization mechanism under full-auto. |
| Return semantics | **Short-wait synchronous**: after server.py persists the spool record it polls the result marker for ≤5s; a hit returns the task id, a timeout returns pending | The model can tell the user "created XX" directly and the frontend can react; pure async would leave the model unable to confirm the outcome |
| rrule scope | The tool layer accepts only the product subset (HOURLY/WEEKLY/ONCE), **rejecting CRON and every minute-granular frequency** | Matches the product constraints of `SCHEDULED_TASK_CHAT_PROMPT`; being stricter than the domain layer (`parse_rrule` additionally supports CRON) is deliberate |
| Base dead tools | Also add `automation` and `send_later` to the app disallow list | The app never attaches their manager, so they always fail; leaving them in the catalog only invites trial-and-error; `tasks`/`github` untouched (out of scope) |

## 3. Solution Overview

### 3.1 Tool Surface

| Full tool name | Level | Parameters |
|---|---|---|
| `mcp_app-automations_create_scheduled_task` | L1 | `name` (required ≤200), `prompt` (required ≤32768), `rrule` (required), `model_id` (optional), `paused` (optional, default false), `idempotency_key` (optional ≤128) |
| `mcp_app-automations_list_scheduled_tasks` | L0 | `limit` (1–100, default 20); returns id/name/rrule/status/nextRunAt/model, **no prompt** |

### 3.2 Data Flow

```
Model calls the tool → server.py validates (rrule subset/caps/charset)
  → atomic spool write: ~/.pinvou3/task-requests/spool/<sha256(from_session|kind|task_id|idempotency_key)|uuid>.json (the §9.2 CRUD naming; this section's historical form has been superseded by it)
  → poll spool/.done/<spool_id>.json for ≤5s (0.2s interval)
      hit       → return {ok:true, taskId, taskName, duplicate?}
      timeout   → return {ok:true, taskId:null, delivery:"pending"}

App-side watcher (new module under features/scheduled, 1s poll)
  → re-validate (the spool dir is user-writable; server-side validation is not trusted)
  → .done already exists → skip (idempotent)
  → ScheduledTaskState::create_task (reuses the existing pipeline: forced yolo, workspace creation, model-binding sidecar, failure rollback)
  → success: write .done {ok:true,task_id,task_name,request_digest}, delete the spool record, audit::append(bound sessions → the PRIVATE ledger root, unbound → the execution root; round-24 M4, the messaging r8-M1 rule),
          emit "scheduled_task:run_updated" (both frontends already listen; the panel refreshes live)
  → failure: ≤3 attempts (at most 2 retries) → quarantine into spool/failed/ + write .done {ok:false,error} (the waiting call receives the failure)
```

### 3.3 Components and Responsibilities

| Component | Location | Responsibility |
|---|---|---|
| MCP server | `pinvou3-app/resources/mcp-servers/app-automations/{server.py,manifest.json}` (new) | Model tool surface: validation, spool, result wait, `feature_disabled` fallback |
| Package registration | `src-tauri/src/features/marketplace/mcp_catalog.rs` (edit) | Add one `MCP_PACKAGES` entry; add the id to the default-install list |
| Creation watcher | `src-tauri/src/features/scheduled/creation_requests.rs` (new) | Drain the spool, re-validate, call `create_task`, result markers, audit, events |
| Cloneable state | `src-tauri/src/features/scheduled/tasks.rs` (edit) | `ScheduledTaskState` derives Clone (JoinHandles wrapped in `Arc<Mutex<Option<…>>>`, same as `EnginePool.idle_reaper`) |
| Approval rules | `src-tauri/src/features/assistant/platform/bridge.rs` (edit) | `ToolAskRule::new(create tool full name)`; reserved for approval-mode differentiation (the current product is full-auto in every mode and the rules do not prompt; the deterministic defense against unattended recursion is the engine-side `unattended_disallowed_tools` — the three write tools are listed alongside create_goal/update_goal; the watcher's eval_/aux-/agentic_ sender rejection is only a claimed-string filter; interactive sched- origins are accepted — `from_session` may be omitted and is unauthenticated) |
| Dead-tool cleanup | `src-tauri/src/lib.rs` (edit) | Disallow base tools `automation`/`send_later` |
| Frontend | `src/features/tools/tool-renderers.jsx`, `src/shared/i18n/{en,ja,zh}.js` (edit) | Result card (execution visibility), trilingual copy |
| Contract doc | `docs/builtin-toolset-contract.md` (edit) | §2 registers the new family, §9 flipped to landed (round-8/9 correction: the "§6 rewrite documenting the short-wait synchronous pattern" claimed by earlier versions never landed; the short-wait semantics are recorded in §3.2 of this design doc, not in the contract) |

## 4. Implementation Steps

**Step 0 — Sync**: sync `origin/main` per CONTRIBUTING and stack behind the dependencies (rebase once builtin toolset / messaging merge), align the submodule gitlink.

**Step 1 — MCP package** (details in §3.1/§3.2):
- `server.py`: pure-stdlib NDJSON JSON-RPC over stdio, mirroring the session-reader skeleton section by section (PROTOCOL_VERSION, error sanitization that never leaks host paths, catch-all, `feature_disabled` fallback reading the manifest `tool_features` + `builtin_features.json`).
- rrule validation: only `FREQ=HOURLY` (INTERVAL ≥ 1 hour + BYHOUR/BYMINUTE), `FREQ=WEEKLY` (BYDAY required + BYHOUR 0–23 + BYMINUTE 0–59), `FREQ=ONCE` (AT as local `YYYY-MM-DDTHH:MM`, timezone suffixes rejected, must be in the future); CRON, minute granularity, unknown keys, and out-of-range values are rejected; trim + uppercase.
- Spool record: `{schema_version:1, id, kind, task_id?, name?, prompt?, rrule?, model_id?, paused?, from_session?, created_at, idempotency_key?}` (round-8 correction: `from_title?` as listed in early drafts was never written by either side), written atomically via mkstemp + os.replace.

**Step 2 — Rust registration**: add the `MCP_PACKAGES` entry (include_str! of both files), `len()==12→13`; add the id to `DEFAULT_INSTALLED_MCP_TOOLS` (`marketplace/mod.rs::DEFAULT_INSTALLED_MCP_TOOLS`); `builtin.rs`/`types.rs` mirror session-reader's manifest and feature-registry tests.

**Step 3 — Creation watcher**: make `ScheduledTaskState` cloneable (steps in §3.3); `ScheduledTaskState::start_creation_watcher (creation_requests.rs)` spawned via `tauri::async_runtime::spawn` (not `tokio::spawn` — the reason and the lesson are documented in `creation_requests.rs`'s `start_creation_watcher` doc and the module header), `select!` cancel/sleep(1s), started at the end of `boot_runtime`, handle parked on state, Drop cancels; per-record handling per §3.2.

**Step 4 — Ask rules**: add `pub const SCHEDULED_TASK_CREATE_TOOL` next to `bridge.rs::SCHEDULED_TASK_CREATE_TOOL`; `scope_deny_ruleset_with` pushes `ToolAskRule::new(...)`; the list tool gets none (L0). Note: the current product pins every session to full-auto approval, so the rules do not prompt today (reserved for approval differentiation S-1); unattended recursive self-modification is deterministically rejected by the engine-side `unattended_disallowed_tools` (the watcher's `check_sender_session_id` isolated-prefix rejection is only a claimed-string filter — `from_session` may be omitted and is unauthenticated). Update the bridge.rs ruleset tests (the scope_deny_ruleset_with region).

**Step 5 — Dead-tool cleanup**: the lib.rs `tool_policy` closure appends disallows for `"automation"` and `"send_later"`.

**Step 6 — Frontend and i18n**: trilingual `uiBuiltinPlugins.dataAccess` new scopes + `uiToolDetails.tools["app-automations"]` subtitle; `tool-renderers.jsx` adds the result card (name/rrule/prompt summary). No new Tauri commands → no access-policy/protocol test changes.

**Step 7 — Contract doc**: §2 registers the new family (with the new-family argument), §9's original single row is replaced by six registered rows (round-9 correction: the originally planned §6 rewrite was not executed; the short-wait semantics follow this design doc's §3.2).

**Step 8 — Tests**: added per the §6 checklist.

**Step 9 — Checks and commit**: all §7 gates green; commits follow `docs/commit-message-convention.md` + DCO.

## 5. Acceptance Matrix

Priority: P0 = hard gate, all must pass before merge; P1 = verified as part of feature acceptance; P2 = best effort / later hardening.
Test names in the "Verification" column are marked: ◇ = to be added, ○ = existing.

### A. Core Creation Chain

| # | Scenario | Precondition/Input | Expected result | Verification | Priority |
|---|---|---|---|---|---|
| A1 | Conversational creation end to end | A normal session says "create an 8:30 daily morning-report task" | The model calls the create tool; taskId/taskName returns within ≤5s; the panel shows the new task | Manual QA (real model) | P0 |
| A2 | Short-wait synchronous return | The watcher creates the task and writes `.done` | server.py hits the marker and returns `{ok,taskId,taskName}` | ◇python `test_short_wait_returns_task_ids` | P0 |
| A3 | Wait timeout returns pending | `.done` does not appear within 5s (e.g. watcher paused) | Returns `{ok:true, taskId:null, delivery:"pending"}`, not an error | ◇python `test_wait_timeout_returns_pending_not_error` | P0 |
| A4 | Schedule takes effect | Creation succeeded | The record carries `next_run_at`; the existing 15s scheduling loop picks it up; ONCE runs once at its time then auto-pauses, HOURLY/WEEKLY fire on schedule | Manual (short-interval tasks) + ○existing domain scheduling tests | P0 |
| A5 | Panel refreshes live | Panel open while the watcher creates a task | The `scheduled_task:run_updated` event drives the list refresh, identical on Tauri and web | Manual, both ends | P1 |
| A6 | Model binding | `model_id` passed / not passed | Passed: persisted to `model-bindings.json`, runs use that model; omitted: falls back to the app fallback model | ◇Rust watcher test + manual | P1 |
| A7 | paused semantics | Created with `paused:true` | Task persisted as paused, not scheduled; the panel can resume it manually | ◇Rust test + manual | P1 |

### B. Parameters and Validation (server.py and watcher, two layers)

| # | Scenario | Input | Expected result | Verification | Priority |
|---|---|---|---|---|---|
| B1 | Valid rrule shapes | The three documented samples (HOURLY INTERVAL; WEEKLY BYDAY; ONCE AT) | Accepted and persisted | ◇python parameterized cases | P0 |
| B2 | Minute granularity rejected | "every 5 minutes"-style expressions (MINUTELY/CRON */5 etc.) | Rejected, error guides the user to every-N-hours/daily/weekly | ◇python | P0 |
| B3 | CRON rejected | `FREQ=CRON;...` | Rejected (the tool layer is stricter than the domain layer — a product constraint) | ◇python | P0 |
| B4 | Past/timezone-carrying ONCE | `AT` earlier than now; a `Z`/`+08:00` suffix | Both rejected; the error explains a future local time is required | ◇python | P0 |
| B5 | rrule out-of-range/unknown keys | `BYHOUR=24`, missing `BYDAY`, unknown keys | Rejected | ◇python | P1 |
| B6 | Length caps | name>200 / prompt>32768 / key>128; exactly at the cap | Over-cap rejected; boundary values pass | ◇python boundary cases | P1 |
| B7 | Missing required fields | name/prompt/rrule missing or empty | `invalid` error the model can correct from | ◇python | P1 |
| B8 | Normalization | Lowercase/whitespace-carrying rrule | Persisted after trim+uppercase (same as the domain) | ◇python | P1 |
| B9 | Watcher re-validation | Manually tampered spool record (over-cap prompt/illegal rrule) | Rejected into `failed/`, no task created (the spool dir is user-writable; server-side validation is not trusted) | ◇Rust test | P1 |

### C. Idempotency and Reliability

| # | Scenario | Input | Expected result | Verification | Priority |
|---|---|---|---|---|---|
| C1 | Retry with the same idempotency key | Two consecutive calls with the same key | Same spool file, same task; the second call returns the same taskId with `duplicate:true`, no second creation | ◇smoke journey + ◇python | P0 |
| C2 | No idempotency key | Key omitted | uuid file name, independent creations | ◇python | P1 |
| C3 | Watcher restart recovery | App restarted with unprocessed spool files | Processing resumes after restart; records with an `ok:true` success marker are not re-created (`ok:false` failure markers allow retry). Delivery is at-least-once: a crash window between the app and the marker write can re-create on retry (the same window as features/messaging, recorded) | ◇Rust test + manual | P0 |
| C4 | Failure retry and quarantine | Creation keeps failing (e.g. read-only disk) | ≤3 attempts (at most 2 retries) → moved to `spool/failed/` + `.done {ok:false,error}`; the waiting call receives the failure instead of hanging | ◇Rust test | P1 |
| C5 | Concurrent-write tolerance | The list tool reads while the watcher persists | Bad lines are skipped without crashing | ◇python | P1 |
| C6 | Lazy directory creation | spool dir missing | Created on first call | ◇python | P2 |

### D. Permissions and Safety

| # | Scenario | Input | Expected result | Verification | Priority |
|---|---|---|---|---|---|
| D1 | Ask rule registration | Any session's ruleset | A `ToolAskRule` for the create tool (Ask, no command constraint) | ◇Rust bridge ruleset assertion (mirrors ○`scope_deny_ruleset_asks_for_session_messaging_send`) | P0 |
| D2 | Unattended rejection | Model calls create inside a scheduled-run session | Engine-side rejection via `unattended_disallowed_tools` (this round's review revision: the force-prompt chain cannot trigger under the current full-auto approval) (recursive self-creation blocked) | ○existing semantics tests + ◇new ruleset assertion | P0 |
| D3 | Audit log | Successful creation | A `scheduled_task_create` record appended into the from_session **PRIVATE ledger root for bound sessions** (`SessionRoots.ledger`, the messaging r8-M1 rule; round-24 M4 revision — the old execution-root landing wrote the provenance line inside the audited model's own working tree, where that model could read and rewrite it with ordinary file tools); unbound sessions keep the execution root (both roots coincide there); plus the R4-M3 watcher-side shadow audit (<home>/automations/audit/, keyed by the spool file name, independent of from_session — omitting the field does not suppress this best-effort watcher trace; direct store writes bypass it) | ◇Rust test (incl. shadow_audit_covers_requests_without_from_session + the bound-ledger landing pin) | P1 |
| D4 | Error sanitization | Various constructed failures | Error text contains no host absolute paths | ◇python (mirrors ○`test_error_messages_do_not_leak_absolute_paths`) | P0 |
| D5 | Write scope | Spool, receipt, quarantine and automation/domain state; session-root and shadow audit JSONL are additional writes. No transcript mutation or OS scheduler installation. | Code review + tests | P1 |
| D6 | Protocol robustness | Illegal stdin/unknown tool/deeply nested JSON | Server survives, `-32601/-32602`, catch-all leaks no exception text | ◇python (mirrors ○existing protocol cases) | P1 |

### E. Visibility and UI

| # | Scenario | Expected result | Verification | Priority |
|---|---|---|---|---|
| E1 | Execution visibility | Tool calls appear in the timeline; the result card shows name/rrule/prompt summary | Manual + ◇node renderer test | P1 |
| E2 | Builtin plugin card | The plugin center's builtin section shows app-automations (L1, data scope, version, not uninstallable, no plugin-level switch) | Manual + ○existing builtin list tests | P1 |
| E3 | Trilingual i18n | New scopes and subtitle complete in en/ja/zh | ○`ui_language_coverage.test.mjs` + manual language switching | P1 |
| E4 | Config visibility | The tools do not appear in the session settings' tool-toggle list (builtin convention), only in the timeline | Manual check | P2 |

### F. Feature Switch and Lifecycle

| # | Scenario | Operation | Expected result | Verification | Priority |
|---|---|---|---|---|---|
| F1 | Switch off → family removed | `set_builtin_feature_enabled("scheduled-task-automation", false)` | New sessions' disallowed list contains all five full tool names of the family (lowercase); the family leaves the catalog | ◇Rust (mirrors ○`feature_removal_flows_into_unavailable_tool_names`) | P0 |
| F2 | In-flight calls degrade | Old context still calls after switch-off | Structured `feature_disabled` + alternative-action hint, not a generic not_found | ◇python (mirrors ○feature-gate series) | P1 |
| F3 | Union semantics | Only this family's switch off | Only the family's five tools removed; session-reader tools unaffected | ◇Rust registry test | P1 |
| F4 | State persistence consistency | Restart after toggling | prefs and `builtin_features.json` mirrors agree (○existing replay mechanism regression) | ○existing tests | P1 |

### G. Platform and Compatibility

| # | Scenario | Expected result | Verification | Priority |
|---|---|---|---|---|
| G1 | Default install and upgrade | After first launch on a new or existing `~/.pinvou3`, app-automations is installed and `bundle/mcp.json` carries the server entry | ◇extends ○`ensure_default_installed_mcp_tools_seeds_only_missing_records` + manual | P0 |
| G2 | Service starts | `python3 server.py` stdio initialize/tools/list round-trip works; the tool set matches the manifest | ◇smoke matrix (13 packages) | P0 |
| G3 | Parity across ends | Web end gains no new commands (no access-policy change); the panel-refresh event is already listened to on both ends (○existing) | Code check | P1 |
| G4 | Coexists with conversational creation | The panel's "create via conversation" flow (scheduled-task-draft → frontend create_scheduled_task) regression unaffected | Manual QA | P1 |
| G5 | Multi-agent sessions | Tool surface identical to normal sessions, no extra injection | Manual spot check | P2 |

### H. Regression and Gates (P0 hard gates)

| # | Check | Pass criterion |
|---|---|---|
| H1 | `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib --locked -- --test-threads=1` | 0 failed |
| H2 | `cargo clippy --manifest-path pinvou3-app/src-tauri/Cargo.toml -- -D warnings` + `cargo fmt --check` | 0 warnings |
| H3 | `npm --prefix pinvou3-app run test:node` | 0 failed |
| H4 | `python3 -m unittest discover -s scripts/tests` | 0 failed (incl. no regression of existing session-reader cases) |
| H5 | `python3 scripts/mcp-server-contract-smoke.py` | All green (matrix extended to 13 packages) |
| H6 | `python3 scripts/architecture-guard.py` | Pass |
| H7 | `./scripts/fork-guard.sh --fast` | Pass (zero fork content changes; the branch carries no fork-register/gitlink commits — the gitlink is the base main's `083cebfd9`; corrected in round-27 M2) |
| H8 | Catalog/default-install tests | `len()==13`; the default-install seed includes app-automations |

## 6. Test Files and Matrix Mapping

| Test file (◇ to add / ○ existing) | Matrix coverage |
|---|---|
| ◇`scripts/tests/test_app_automations_server.py` | A2 A3 B1–B8 C1 C2 C5 C6 D4 D6 F2 |
| ◇`scripts/mcp-server-contract-smoke.py` (journey) | A2 A3 C1 G2 H5 |
| ◇inline Rust tests in `features/scheduled/creation_requests.rs` | A6 A7 B9 C3 C4 D3 |
| ◇`features/assistant/platform/bridge.rs` ruleset tests | D1 D2 |
| ◇`features/marketplace/{mcp_catalog,builtin,types,mod}.rs` tests | E2 F1 F3 F4 G1 H8 |
| ◇`pinvou3-app/tests/` (renderer/coverage, where applicable) | E1 E3 |
| Manual QA | A1 A4 A5 E1 E2 E4 G4 G5 |

## 7. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| Prompt injection silently creating tasks (interactive sessions auto-approve) | Audit log + timeline result card showing the draft summary (truncated when long) + a global feature switch + unattended runs unconditionally rejected by the engine-side `unattended_disallowed_tools`; a blocking confirmation card can later reuse this watcher skeleton and forwarder hook (deliberately deferred, per the decision record) |
| Task explosion (the model creating frequently) | The list tool lets the model de-duplicate first; names are non-unique and managed by the user in the panel; `spool/failed` quarantine prevents queue blocking |
| Confusion with the base dead tools | `automation`/`send_later` added to the disallow list (step 5), leaving exactly one working path in the catalog |
| Watcher vs panel/domain concurrent writes | The single creation entry point remains `ScheduledTaskState::create_task` (serialized by the existing operation_locks); the watcher never writes AutomationManager directly |
| Dependencies not yet in main (corrected in round-30 S4) | ~~The builtin toolset/messaging currently live only on `luzeyang-dev`~~ **OBSOLETE**: messaging landed via #627, session-reader via #585/#586, and this branch's base is main — the risk no longer exists |

## 8. Boundaries and Disclosures

- Interactive sessions have no blocking confirmation (decision record §2); the visibility problem of the `tasks`/`github` base tools is out of scope this time.
- **The real boundary of unattended-recursion protection (round-8 M1 revision)**: the deterministic defense is the engine-side `unattended_disallowed_tools` (deny list) +, from round-8, **subagents disabled** only for the unattended run engine; an interactive continuation rebuilds with normal delegation (`agent(inherit_disallowed_tools=false)` can no longer shed the deny list — the sub-runtime keeps MCP+auto-approval, and the posture list does not include this family's three write tools). There are THREE residual channels (corrected in round-27 M1; a third added and closed in round-30 REQUIRED-2). First, **shell is not required**: scheduled tasks pin Yolo (`tasks.rs`) → trust mode accepts absolute paths in `write_file`, `~/.pinvou3` is not in the default denied subtrees, and the unattended deny list is named tools only — so the file tools alone can write valid spool records into `~/.pinvou3/task-requests/` (an existing concession of the claimed-string filter). Second, **the messaging spool is a same-class channel that launders across sessions**: `features/messaging/mod.rs` describes that directory as "an unauthenticated local side door" (no digest binding, optional `from_session`) — an unattended run's file tools can write it, and delivery then injects an ordinary-catalog turn into any plain session whose catalog includes this family's three write tools (the file-write twin of the indirection round-9 M2 fixed at the tool layer). Third, **the registry launchers could widen the same turn's tool surface**: `registry_sync`/`start_registry_mcp_server` ride the app allowlist on sched- engines, auto-approval Allows them, and the foundation's full-access hold list names only `start_mcp_server` — launching a user-cached registry candidate made its `mcp_<name>_*` tools join the SAME turn under fresh identities this name-keyed shield could not see. **This wave adds both launchers to `unattended_disallowed_tools`** (the engine-side named deny list, round-30 REQUIRED-2), closing that escape; adding the launchers to the foundation hold list is the complete fix on the CodeWhale side (tracked upstream per the fork boundary).
- The panel's existing "create via conversation" flow stays untouched; both channels converge on the same `create_task` domain function.
- This branch carries **no** fork-register refreshes or gitlink-chase commits — the gitlink follows the base main (corrected in round-27 M2); `docs/fork-modifications.md` is maintained by each landing PR on its own; this plan document itself lands with this branch as the review baseline for the implementation.
- The domain `parse_rrule` actually supports CRON; the tool layer's tightening to the three-form subset is a deliberate product constraint; opening it up later only touches the server.py + watcher subset validation, not the domain.
- **Store read limits**: single-record reads report an unreadable record when refused by size/type policy; list skips policy-refused/corrupt records. Permission/IO failures return an error, rather than claiming the store is empty. Listing remains incomplete for tasks above the 256 KiB cap.
- **Run outcomes are not observable (round-27 minor 12)**: `list`/`read` expose status/nextRunAt/lastRunAt only — the model cannot read run output (session-reader hard-rejects `sched-` targets; no bypass). The replaced foundation `automation` tool was richer here (its read included run records). Acceptable under the single-user semantics; disclosed.
- **Feature-switch copy ships ahead of a settings page (round-27 minor 13)**: the trilingual copy for the `scheduled-task-automation` switch is in place, but this PR ships no automations settings page — the switch is reachable via command/config today (contract §3.1's "wired into each feature's own settings page as the feature lands" is honored by a follow-up PR).
- **IO scope**: scheduled-request audits use the existing stable-directory platform primitive and the new handle-relative append operation. The earlier global `audit.rs`/`open_private_append_file` changes are removed from this PR; messaging, computer-use and diagnostics retain their prior behavior. Marker publication uses exclusive unique temps rather than the removed truncating temp helper.


---

## 9. Implementation Addendum: Full CRUD (2026-09-29, finalized on the same branch)

The first design (§1–§8) covered only create + list. Per review feedback it is extended to full CRUD; this section is its review baseline; where §1–§8 conflict with this section, this section wins.

### 9.1 Tool Surface (replaces §3.1)

| Full tool name | Level | Parameters |
|---|---|---|
| `mcp_app-automations_create_scheduled_task` | L1 | Same as §3.1 |
| `mcp_app-automations_read_scheduled_task` | L0 | `task_id` (required); returns the single task's full fields, **including prompt** (for inspection before update) |
| `mcp_app-automations_list_scheduled_tasks` | L0 | Same as §3.1 (no prompt) |
| `mcp_app-automations_update_scheduled_task` | L1 metadata / L2 replacement | `task_id` + name/model_id/paused apply immediately; prompt/rrule replacements require review and submission in the panel, with no tool-side mutation |
| `mcp_app-automations_delete_scheduled_task` | L1 scoped exception | `task_id`; execute an explicit request with an unambiguous target under existing permissions. Ask the user to resolve target ambiguity first. Uses normal queued receipts; no definition backup/undo. |

### 9.2 Design Decision Addendum

| Decision | Conclusion | Rationale |
|---|---|---|
| Granularity | Five `verb_noun` tools, not one mode-enum tool | Contract §4.2 prefers one tool + mode, but read and write carry different approval semantics: merging would put L0 reads behind the L1 Ask gate (harassing) or leave writes ungated; same reasoning as the session-reader three-tool precedent |
| Delete authorization | L1 scoped configuration-removal exception, approved by the user on 2026-10-21; no mandatory second confirmation. | Identify the exact target from list/read; ambiguous names/references/details require clarification, never guessing. Lost configuration may need manual recreation. Minimal history is not a definition backup or undo. |
| Spool extension | The record gains `kind` (`create`/`update`/`delete`) + `task_id`; every other field becomes optional | Contract §4.4 allows additive fields only: old create records keep working with the watcher defaulting `kind=create` |
| Idempotency naming | `sha256("<from_session>\|<kind>\|<task_id>\|<key>")`; the key must travel with from_session | The namespace narrows per sender and does not degrade to global when from_session is absent; different operations sharing a key do not stomp each other (create and update can share one key without overwriting each other) |
| Target precheck | Updates probe existence server-side; deletion reaches receipt/domain even when the target is absent. | Ensure-absent deletion retries must not fail before checking an existing receipt. The domain distinguishes typed NotFound from IO/parse errors. |
| Ask rules | update/delete register typed Ask rules like create; read/list get none | Reads have no app confirmation rule, but remain subject to feature/scope and foundation Plan-mode gates; the rules do not prompt under the current full-auto approval and are reserved for differentiation; unattended recursive self-modification is deterministically rejected by the engine-side `unattended_disallowed_tools` (the watcher's eval_/aux-/agentic_ sender rejection is only a claimed-string filter; interactive sched- origins are accepted) |

### 9.3 Acceptance Matrix Addendum (continuing §5 numbering)

| # | Scenario | Expected result | Verification | Priority |
|---|---|---|---|---|
| I1 | Safe update end to end | name/model/paused updates persist and refresh; prompt/rrule replacement returns confirmation_required without mutation | Rust + Python; actual Linux panel submission verified below | P0 |
| I2 | Exact-target deletion and replay | Explicit unambiguous request deletes by ID through the watcher/domain; same-key crash replay and different-key already-absent deletion succeed. Ambiguous targets require model clarification first | Rust + Python; real-model ambiguity/direct-delete acceptance recorded below | P0 |
| I3 | Read details | read returns the full fields including prompt; unknown id is an explicit not found | ◇python + manual | P0 |
| I4 | update validation | Missing task_id / zero fields / CRON rrule all rejected | ◇python + ◇Rust `update_request_validation_requires_target_and_a_field` | P0 |
| I5 | delete validation | Missing task_id and extra fields both rejected | ◇python + ◇Rust `delete_request_validation_rejects_extra_fields` | P1 |
| I6 | Ask-rule extension | The ruleset carries Ask entries for update/delete; read/list carry none | ◇Rust `scope_deny_ruleset_asks_for_scheduled_task_create` extension | P0 |
| I7 | Drain hardening (#680 wave) | When pending files exceed 256, the sorted tail is quarantined and never reaches the domain: each tail record is probed for the REAL cause BEFORE the move (corrupt is "unparseable", oversize/unreadable says so), and only a genuinely-over-ceiling record keeps the ceiling label; the pre-move snapshot re-verify narrows (does not close) the probe→rename window, and a wedged rename re-probes the record per pass; a pre-move snapshot re-verify (the divergence guard) leaves a replacement published between the probe and the rename for its own fresh pass instead of answering it with the old bytes' marker; a digest-bound recorded ok receipt suppresses the failure marker (the receipt answers the waiting call), and a parseable tail disposition leaves an audit line. A stuck episode's (Poison / retry-exhaustion arms and the park gate) audit line and failure marker fire once per episode (per process), keyed by the payload digest (the tail arm's once-ness is structural: its audit and marker fire only inside the successful move): a same-bytes re-spool stays deduped, a corrected one — whose stale ok:false marker the SERVER unlinks — rebinds onto a fresh episode; an unlinked marker is restored while the audit stays deduped (ensure-presence, including the park-gate path's parked-state note). A parked record's un-park is admitted by payload CHANGE, not the clock: a same-bytes mtime bump stays parked with ZERO re-applies, a corrected payload rebinds and applies; every terminal arm and the maintenance path clear the episode identity on the move. Non-UTF-8 names are quarantined on sight BEFORE the count, consuming no batch slots (the enumeration-window cut precedes the quarantine; their window slots free up as the backlog recedes) | ◇Rust `poison_park_corrected_respool_opens_a_fresh_episode` / `poison_park_same_bytes_bump_stays_parked_and_marker_is_restored` / `retry_park_same_bytes_bump_never_reapplies_corrected_payload_rebinds` / `poison_park_episode_clears_when_the_quarantine_finally_succeeds` / `poison_arm_clears_the_episode_entry_when_its_quarantine_succeeds` / `retry_arm_clears_the_episode_entry_when_its_quarantine_succeeds` / `corrupt_ceiling_tail_names_the_real_cause` / `oversize_ceiling_tail_names_the_real_cause` / `ceiling_tail_with_landed_receipt_writes_no_failure_marker` / `non_utf8_squatters_do_not_starve_legitimate_records` / `tail_snapshot_divergence_leaves_the_replacement_for_the_next_pass` / `vanished_ghost_skipped_and_dangling_link_answered_terminally` / `deduped_episode_repairs_a_marker_the_diverged_pass_skipped` | P1 |

The §6 test mapping extends accordingly: the python suite covers I3/I4/I5, the smoke journey covers the read/update/delete validation paths, the result-card tests cover parsing all three operations' results, and the Rust suite covers I7's drain-hardening wave.

## Review follow-up: bounded drain and evidence retention

- Each non-empty drain pass reads only `disabled_builtin_features` from the authoritative settings file. It does not load preferences, probe language, or migrate credentials. Invalid/unreadable settings pause the drain with one diagnostic per failure streak; fixing the file resumes it on the next poll. Empty spools need no settings read.
- Vanished spool names release retry/park entries. Non-UTF-8 filenames are quarantined with their original bytes; they cannot produce a protocol receipt. A pass emits at most one panel refresh for its successful operations.
- Quarantine uses the existing handle-relative cross-platform no-replace move primitive with collision suffixes. The duplicated scheduled-only OS adapter has been removed. Native execution coverage is in the Windows-named shared filesystem tests.
- The 14-day receipt retention does not remove a marker while its spool filename still exists. This keeps an applied request deduplicated across persistent unlink failures. Once the spool name disappears, normal retention resumes. Evidence and stale server temporary files retain the 14-day policy.
- Model-binding updates rely on the store's own snapshot restoration on persistence failure. The redundant second rollback write was removed so it cannot overwrite a concurrent CLI rebind; previously persisted task fields remain applied and retriable.
- **IO scope**: scheduled-request audits use the existing stable-directory platform primitive and the new handle-relative append operation. The earlier global `audit.rs`/`open_private_append_file` changes are removed from this PR; messaging, computer-use and diagnostics retain their prior behavior. Marker publication uses exclusive unique temps rather than the removed truncating temp helper.
- The marketplace copy now gives the actual configuration off-ramp (`disabled_builtin_features` in `settings.json`) and does not imply a settings-page toggle. Duplicate-card copy covers both recorded replay and replacement of a queued request. The first shipped package version is `1.0.0`.

| Acceptance case | Expected result | Verification |
|---|---|---|
| Pending ceiling | At most 256 records attempt apply per pass; excess evidence and failure receipts are preserved without overwriting prior evidence; flood warnings stay streak-latched | `pending_ceiling_quarantines_the_tail_with_failure_markers`, `quarantine_keeps_colliding_evidence` |
| Retention and temporary sweep | Old terminal state and stray `*.tmp` files are pruned after 14 days; receipts for stuck spool names survive | `prune_stale_state_sweeps_old_markers_and_stray_tmp`, `retention_preserves_receipts_for_stuck_spool_records` |
| DST wall time | Both layers reject local ONCE times in DST gaps or overlaps; the Rust watcher remains authoritative if Python tzdata is unavailable | Existing ONCE/DST fixtures in the Rust watcher and Python recurrence suite |
| Broken preferences | Invalid settings pause only this drain; repair resumes it without a restart or repeated credential migration | `a_drain_pass_coalesces_refresh_and_pauses_on_bad_settings` |

## Round-32 follow-up

- **Shared feature gate**: a tool mapped to features A and B remains available if either feature is enabled; it is disabled only when both are off. A two-feature fixture now covers none, either, and both disabled states. The model-facing error describes this same union rule.
- **Lazy export mapping**: the timeline loader must resolve `ScheduledTaskToolResult`, which parses the MCP envelope and supplies the presentation card. Loading `ScheduledTaskToolCard` directly bypasses that adapter and falls back to raw output. The full import/export mapping is now asserted.
- **Parked timestamps**: a replacement un-parks only when `parked_at < mtime < now`; future/restored timestamps do not buy another apply attempt on each poll. Queue/park retry counts remain **filename-scoped**: replacing a keyed request after two failed attempts leaves one of the three attempts; an eligible parked replacement un-parks without resetting its spent budget. Removing a name or reaching a terminal outcome clears it. This deliberately bounds same-key replanting; use a new idempotency key only for an intentionally distinct operation, taking the documented at-least-once duplicate window into account.
- **Shadow audit rotation**: the long-lived `automations/audit/workflow_audit.jsonl` rotates before append once it exceeds 2 MiB, retaining one `workflow_audit.jsonl.old` segment. Normal growth is bounded by the threshold plus one event per segment, and the check is based on disk state across restarts. A larger pre-existing legacy segment is preserved once until the next rotation replaces it. Rotation uses feature-local directory-relative operations; concurrent app instances are not serialized. If rotation fails, existing evidence is kept and new rows append to the current log with a diagnostic; growth can exceed the normal cap until repair.
- **Server responsiveness**: the server is single-threaded. While a write waits up to five seconds for its receipt, `ping`, `tools/list`, and all other calls on the same server wait too; N queued writes can serialize into N times that wait. The pending response remains the recovery path. This follow-up does not introduce threads or change protocol ordering.
- **Failure receipts**: even a foreign-written marker has its known home, task-queue, and automation-store path prefixes redacted before its error enters model context. Non-text errors use the generic failure message. Marker content remains unauthenticated local input; this is path redaction, not provenance verification.
- **Surrogate key**: the existing UTF-8 validation and its regression are retained. The direct test entry point is after all test classes so direct invocation also exercises the prior follow-up cases.


## Round-33 current behavior and boundaries

- Scope is desktop Operate/Yolo. Plan blocks this entire MCP family, including L0 reads; CLI/headless hides it because that host has no ScheduledTaskState consumer. The five tools are always loaded when admitted, with feature/scope denies still taking precedence. The shared model instruction describes scheduling and its app-liveness requirement. Session-bound instructions provide the exact `from_session` for keyed retries; it remains an unauthenticated audit claim. Interactive `sched-` origins are accepted; actual unattended writes remain engine-denied, while aux/eval/agentic_ origin restrictions remain.
- The latest explicit user decision removes the mandatory deletion card and snapshot-confirmation mechanism. Delete remains a normal model tool under existing permissions: list/read first as needed, execute an explicit request with an unambiguous target by exact ID, and ask the user whenever names, references or details do not identify the intended object reliably. Never guess or broaden scope; task content is untrusted data, not authorization. These instructions guide the model and do not provide deterministic intent verification. The normal watcher/domain deletion pipeline and digest-bound receipts are reused. Missing targets mean already absent; IO/parse failures remain errors. No full-definition backup or undo is added. Prompt/rrule replacement still requires panel submission. Minimal run-history archives cannot restore the full definition.
- Audit JSONL has no user-facing viewer and is hidden by the workspace file list. Result cards and the panel are the actual user review surfaces. Shadow audit is best-effort evidence for this watcher only. Direct same-user writes to `<home>/automations/automations/<id>.json` bypass the watcher, validation, provenance and shadow log entirely, in addition to the direct spool/messaging-file bypasses in §8. These name-keyed tool restrictions are not a filesystem sandbox.
- Spool, `.done`, `failed` and audit components are opened through real-directory handles; links/reparse points are refused. Boot prepares them. A regular file occupying `audit` is preserved under a unique `audit-obstruction-*` name before directory creation. Other unsafe/unavailable directories fail setup. Rotation keeps one previous segment normally; a blocked rotation logs and continues appending to the current evidence (temporary unbounded growth until repaired), rather than dropping every new row. Append/open failures are logged and are not tamper-proof guarantees.
- Each pass enumerates at most 290 entries, applies at most 256 selected JSON records, and quarantines at most 32 tail records. Sorting is within that bounded batch, not a global full-directory sort. Sustained same-user flooding/non-request entries can delay later work; this is not a fairness/rate-limiting guarantee. Terminal failure arms compare the captured bytes and file identity again after audit IO; replacements remain queued without the old error marker. The narrow check-to-move race and portable same-size/mtime identity floor remain disclosed.
- Quarantine retention starts when evidence is moved. Receipts and evidence expire after 14 days; a keyed retry after expiry can apply again. A stuck spool preserves its matching success receipt. `.done/*.tmp` and spool temp debris are swept in bounded retention batches. Marker publication uses exclusive unique temps, file sync, and the existing anchored atomic publication primitive. Marker publication failures after apply retry the cached receipt and notify the panel without applying again in that running process. Process exit still loses this cache: crash-before-receipt remains at-least-once and can duplicate creates.
- Disabling via a manual settings.json edit requires app restart so the MCP mirror and drain agree; no settings UI toggle is currently shipped. The manifest and MCP server both report 1.0.0.
- The unattended shield and delegation restriction apply to the run engine only. Refresh broadcasts inspect the active unattended flag. Reusing an engine with a different interactive/unattended posture forces a rebuild. Scheduled scripts relying on delegation during unattended runs remain unsupported; interactive continuation retains normal delegation.
- The deterministic integration test `real_stdio_server_drains_through_real_rust_domain` starts the shipped Python MCP service and drains its real spool through the Rust watcher/domain/storage. Separate live desktop acceptance results are recorded below; this deterministic test is not itself a live-model A1 test.
- An in-process ToolSpec is a viable architectural alternative that would remove this local writable spool protocol. It is not implemented in this repair: migrating the shipped package, pending requests and retry receipts needs a separately reviewed compatibility plan. This PR reuses domain CRUD and existing filesystem primitives; it does not implement a second scheduler or modify CodeWhale.

New-id collision protection refuses to replace an existing custom app-automations package; preserve/rename it before enabling the builtin. Host launch args pass the actual request/store roots explicitly, so filtering PINVOU3_HOME from child environments does not redirect the queue.

### Live desktop acceptance, 2026-10-21

Tested the candidate on Linux arm64 in the actual Tauri/Wry development window with the real Ornith-1.5-35B-A3B-NVFP4 model. An isolated `PINVOU3_HOME` and a separate application identifier kept the user's existing tasks and sessions untouched. A temporary local driver dispatched DOM input/click events in the real renderer; observations included visible panel details, actual IPC DTOs, persisted receipts, run records and session transcripts. The driver is not a shipped product hook.

- **A1 passed:** a normal chat request created a paused daily 08:30 task. The model called the always-loaded list/create tools without `tool_search`; the create call returned its confirmed task ID/name in 604 ms (9.042 s for the full model turn). The actual panel showed its name, full prompt, daily schedule and paused state.
- **A4 passed for the observed fires:** a second normal chat created ONCE, HOURLY and WEEKLY tasks for 17:34 local time (09:34 UTC). The existing scheduler enqueued all three at 09:34:11.919; real model turns completed by 09:34:20.254 and each stored assistant reply was `QA_R33_FIRED`. ONCE became Paused with no next run. HOURLY remained Active with the next run at 10:34 UTC; WEEKLY remained Active with the next run on October 28 at 09:34 UTC. After another 32 seconds, all three still had exactly one run. This verifies their first actual fire and next-run bookkeeping, not a wall-clock wait for another hour/week.
- **Panel submission passed:** a model-generated prompt correction received `confirmation_required` and left the stored prompt unchanged. Editing and saving that prompt through the existing panel persisted the correction. The delete confirmation preserved a paused fixture on Cancel and removed it only after Confirm. Opening the completed ONCE history row showed the real run conversation and response.

This acceptance covers the Linux development desktop. Native Windows/macOS, the packaged release UI and the Web client were not manually exercised in this run. Existing deterministic gates and CI remain separate evidence.

Additional failure boundaries: active retries take precedence within the bounded apply batch; parked records and unpublished receipts use separate maintenance paths, with bounded tracking backpressure. Create persists Paused until workspace, model binding and kind are committed, then resumes only when requested active; workspace failure rolls the inert record back. If evidence timestamps cannot be changed, it is preserved as .untimed-* for manual cleanup instead of being prematurely pruned.

### Deletion product decision and comparison

The user explicitly accepts direct deletion for a clear request and unambiguous target, with conversational clarification when object identity is uncertain, and no additional confirmation card or definition backup. Simple tasks can often be recreated quickly, but complex prompts may be costly to reconstruct; accepted risk is not zero risk or automatic restoration. Deletion uses the existing cancellation/history pipeline and cannot reverse past effects.

[Claude Cowork](https://support.claude.com/en/articles/13854387-schedule-recurring-tasks-in-claude-cowork) documents conversational task setup with review of the proposed parameters and routine edit/pause/delete management. [Cursor Bot routines](https://prod.cursor.com/help/grok-bot/routines) documents deletion with confirmation and no undo. These are user-facing precedents, not evidence of identical internal authorization or undocumented backup policies. These documents show task-management UX precedents only; neither establishes that this product must use an extra mandatory deletion card. The scoped policy above follows the explicit local product decision.

### Ambiguous target and direct deletion: actual model acceptance

In the rebuilt Linux desktop, two paused fixtures had the same name and different hourly intervals. A normal request to delete that name caused the real model to list both and ask which one the user intended; no mutation tool was called and both records remained. After the user specified “delete the every-two-hours task and keep the hourly one,” the model made one delete call with the correct ID, received the app's successful receipt and reported completion. The actual store and IPC list showed exactly that target removed and the hourly fixture plus unrelated decoys preserved. No additional confirmation card appeared. All remaining fixtures were paused. This demonstrates the observed model behavior, not a guarantee that every model always interprets intent correctly.


### Round-36 queue recovery and keyed replacement

Parked terminal records and unpublished receipts do not consume apply or tail-quarantine slots. Each poll retries the identity-checked terminal move for unchanged parked records, without another domain apply; a genuine newer replacement retains its filename-scoped spent retry budget. When the bounded in-memory evidence capacity is exhausted, fresh requests stay queued until storage recovers, with a streak warning, rather than receiving a false ceiling failure. Successful evidence moves automatically release capacity without restart. Directory enumeration remains bounded; flood starvation is not solved by this change.

Replacing a keyed request while the watcher is already applying the previous payload can apply both payloads. This is a separate at-least-once window even without a crash. The retry result describes its own apply and explicitly warns that the earlier request may also have applied; inspect the panel before retrying again. No exactly-once or in-flight cancellation guarantee is made.

---

## 10. Scheduled messages (`session_message`, revised for R19)

A scheduled message reuses the existing automation scheduler and CRUD flow. At
execution it delivers the stored body into an existing ordinary session: steer
when a turn accepts it, otherwise the EnginePool messaging new-turn path. It
creates no separate scheduled-run conversation and no new scheduler or store.

### 10.1 Surfaces and product decisions

| Surface | Supported behavior |
|---|---|
| MCP create | Supplying `target_session` derives `session_message`; it is mutually exclusive with an explicit kind. Ordinary create remains unchanged. |
| MCP read/list | Both expose `kind`; read also exposes `targetSession`. They read the capped kinds registry without rewriting it. Unreadable, corrupt or newer metadata is an error, not proof of an ordinary task. |
| MCP update | Retarget an existing scheduled-message task; an ordinary task cannot be converted. Prompt/rrule replacements require review and submission in the existing panel under #628's product decision. |
| Panel | Displays the target and full body, edits the body/schedule and manages pause/delete/run-now. It cannot create or retarget scheduled messages. |
| Timeline tool card | Shows the requested target for a matching receipt (or a request still marked pending), plus a 120-character body excerpt. A mismatched replay hides the new requested target. This is not a full-body review or deterministic authorization UI. |
| CLI | List/show preserve the raw kind and target. `run` refuses this kind with a product-host remedy; creation and retargeting are not implemented. An update may replace its body only within 32,768 Unicode characters. Unknown kinds and unreadable kind metadata refuse prompt replacement. Other documented CLI mutations retain their existing scope. |

Self-addressing is allowed. Task access follows the existing local-user permission
model; there is no creating-session ownership check for retargeting. Tools must
follow explicit user intent and ask when the task identity is ambiguous, per #628.
This guidance does not prove intent or prevent prompt injection. Delivery injects
the raw body as a target instruction, without a machine-readable origin envelope
or a delivery audit line in the target ledger. CRUD auditing and the run result
provide limited provenance; they do not provide messaging's origin/audit parity.
Prompt/rrule tool replacement remains panel-only. No dedicated confirmation card,
definition backup, undo system, or new authorization protocol is added here.

### 10.2 Validation and persistence

- The body cap is **32,768 Unicode characters** after trimming. The MCP server
  applies it to all task kinds; the domain/panel conservatively enforce it for
  any kind not definitively ordinary chat, and the executor rechecks it for
  scheduled messages. CLI session-message replacements share the app constant.
- Target IDs have an ASCII letters/digits/`-`/`_` charset and a 128-character
  maximum. Leading `_` is reserved; case-insensitive `sched-`, `aux-`, `eval_`
  and `agentic_` targets are refused. Server, watcher and domain/executor apply
  identity checks. Exact directory-entry spelling and a regular non-symlink
  session file are required at server, domain and each delivery.
- **ACP/code classification is checked at domain and executor only**, from
  `session-agents.json`. A missing index is the store's normal empty-index case;
  a present unreadable/corrupt/newer index fails closed. This does not consult
  live AcpPool recovery, and cannot identify code ownership absent from the index.
- Create stages a **Paused** automation, persists workspace/model/kind+target,
  then activates only if requested. Workspace failure rolls back the definition.
  A crash before activation can leave a paused partial record, not an Active
  kind-less chat. Cleanup is best-effort when its own disk write fails.
- Update reads a mandatory original definition and a fresh readable kind registry
  before mutation. Its app AutomationManager mutex spans definition and target
  writes; executor dispatch reads the body and kind/target under that same mutex.
  A dispatch therefore sees an entire old or new pair. The lock is released before
  engine delivery; a later edit cannot recall a pair already captured.
- A target-save or workspace-save error attempts to restore the original mutable
  definition (name/body/schedule/model/status); store-level target failure restores
  its in-memory entry as well. Failure to obtain the original snapshot stops the
  update. These files are **not one crash-atomic transaction**: process death
  between writes, rollback-write failure, and external CLI/file writers outside
  the app mutex remain residual risks. Stop external writers while the app runs,
  inspect and repair the definition/target pair after a failed rollback.
- For scheduled messages, delivery uses the complete configuration current when
  execution starts. A queued TaskRecord's prompt is its enqueue-time snapshot;
  after editing it can differ from the body actually delivered. The result records
  the delivered destination, not an immutable copy of the delivered body.
- Production dispatch refuses unreadable/quarantined whole kinds metadata rather
  than downgrading to chat. An absent entry in a healthy registry still means chat;
  external removal of an individual entry cannot be distinguished from that case.
- `target_session` is additive and old records read normally. **Downgrading is not
  write-safe**: an older typed app can drop target fields when rewriting the
  registry. Stop the app before downgrade; preserve the registry privately and
  restore it before re-upgrade, or recreate affected tasks with their destinations. A
  scheduled-message entry without a target fails closed after re-upgrade.

### 10.3 Execution and history boundaries

- A completed delivery result means the engine accepted steer/new-turn handoff;
  it does not mean the target's model completed its response. The retained linked
  completed TaskRecord result stores the destination using a shared formatter and
  parser. Retargeting A to B leaves old retained rows showing A. Missing/pruned
  results, malformed text or removed kind metadata stay unknown. Deleted archives
  still lose this footer when kind metadata is removed. The panel shows a localized
  missing-target message for identifiable scheduled-message runs; it does not jump
  to the destination session.
- The delivery attempt shares a 30-second bound across steer and new-turn handoff.
  Cancellation is selected before a ready delivery result; after the delivery
  future returns success there is no await that could turn it into Canceled in
  this executor. Cancellation/timeout stops waiting on an in-flight future; it
  cannot promise to retract effects already handed to another task. If delivery
  and cancellation are both ready, the biased cancellation branch can record
  Canceled even after the engine accepted the message. This follows from branch
  ordering; the live acceptance did not exercise that race. No specific
  timeout-after-success window is asserted as verified.
- An accepted steer can still be lost before the target consumes it. This inherited
  EnginePool window permits Completed without consumption. A crash between engine
  acceptance and durable task/run reconciliation is not an exactly-once receipt;
  recovery or manual retry can duplicate delivery. There is no delivery-id dedupe
  journal, and automatic replay of every crash is not guaranteed.
- A recurring slot within the scheduler's 60-second grace is eligible only if
  there is no existing slot record or active run. An active run skips the slot;
  an older missed recurring slot jumps directly to the next future occurrence.
  A missed ONCE can run arbitrarily late. After it is enqueued, ONCE has no future
  slot and pauses even if delivery fails; retry requires an explicit new run or
  task. The next recurring slot is a new attempt, not guaranteed recovery of the
  failed message (the task may have been edited).
- Manual run-now has no in-flight check and can overlap a scheduled run. Pause
  stops future scheduling without canceling an existing delivery. The task's model
  binding is display-only for this kind: delivery uses the target's current live
  turn or its own next-turn model and permission settings.
- Disabling the MCP feature removes its tools without stopping existing scheduled
  messages; manage those in the panel. CRUD spool idempotency does not deduplicate
  delivery. Delete retains #628's ensure-absent success for retry after success.
- Watcher shape validation handles invalid fields before apply. The remaining
  string classifier recognizes narrow domain dispositions (wrong retarget kind,
  explicit missing task/target, ACP/code, expired ONCE). Generic I/O, parse and
  index-read failures use bounded retry. A typed domain-error migration remains
  debt; arbitrary OS text is not a permanent-error contract.

### 10.4 Verification scope and remaining debt

R19 adds CLI kind/target inspection and safe-update regressions, MCP read/list and
invalid-target tests, strict registry/index failure tests, target-store restore,
mandatory pre-update reads, app-lock body/target interleaving, and a real
TaskManager → executor → retained result → target-parser round trip. UI render
fixtures cover both destination rows and unknown-target text. A padded target is
normalized consistently in validation and receipt digests. Parent #628's paused
create/workspace rollback tests remain part of the combined branch.

These are deterministic local regressions. Exact commands, current counts, entry
sizes, live/manual acceptance and platform limitations are recorded in the PR's
latest verification section; historical review measurements are not current
candidate measurements. Parent #628's live A1/A4 ordinary-task acceptance does not
by itself verify this kind's delivery.

R19 Linux Tauri/Wry acceptance also exercised this kind with the configured real
model: a conversation created a self-targeted scheduled message through MCP; the
2026-10-21 11:04 UTC ONCE slot was enqueued at 11:04:06.469 UTC and produced the
expected model response. The actual panel showed the target, full body and
completed destination footer. An IPC/domain retarget followed by manual run-now
reached a second ordinary session and produced the expected real response, while
the old run retained its original destination. A further 30-second observation
left exactly two completed runs, one delivered body/reply per target, no scheduled
run conversation and a paused fixture task. Real stdio read/list also projected
the kind/target correctly. Test app and driver were stopped afterward. Live busy
steer, packaged release, Web UI and native Windows/macOS acceptance were not run.

Remaining debt is explicit: the live EnginePool steer/fallback order still uses
bounded source contracts rather than an extracted behavioral fake; per fire there
is synchronous session directory scanning, a full uncapped ACP index parse,
preferences I/O, and a capped full kinds-registry read on the single worker.
History projection still performs result/metadata lookups per row. No performance
optimization, exact-once receipt, origin envelope or native Windows/macOS manual
acceptance is claimed. The approved entry budget remains 506,000 B on both builds.
