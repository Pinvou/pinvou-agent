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

Goal: in **any session**, the model can create scheduled tasks directly through a builtin tool, landing in the exact same persistence path as the panel (the same `ScheduledTaskState::create_task` domain function, the same data directory `~/.pinvou3/automations`), while satisfying the L1 safety requirements of contract §5.

## 2. Design Decision Record

| Decision | Conclusion | Rationale |
|---|---|---|
| Placement | **New capability family** with a new MCP server `app-automations` (contract §2/§8.1) | Scheduling is an independent capability family; future automation tools such as run-now or memory grooming can join it; avoids bloating the session-reader family |
| Interaction confirmation | **Follows the messaging precedent**: typed Ask rules + audit log + timeline visibility, no blocking confirmation card | Consistent with already-shipped L1 tools; residual risks in §9 |
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
| Approval rules | `src-tauri/src/features/assistant/platform/bridge.rs` (edit) | `ToolAskRule::new(create tool full name)`; reserved for approval-mode differentiation (the current product is full-auto in every mode and the rules do not prompt; the deterministic defense against unattended recursion is the engine-side `unattended_disallowed_tools` — the three write tools are listed alongside create_goal/update_goal; the watcher's sched-/eval_/aux- sender rejection is only a claimed-string filter — `from_session` may be omitted and is unauthenticated) |
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
| D3 | Audit log | Successful creation | A `scheduled_task_create` record appended into the from_session **PRIVATE ledger root for bound sessions** (`SessionRoots.ledger`, the messaging r8-M1 rule; round-24 M4 revision — the old execution-root landing wrote the provenance line inside the audited model's own working tree, where that model could read and rewrite it with ordinary file tools); unbound sessions keep the execution root (both roots coincide there); plus the R4-M3 watcher-side shadow audit (<home>/automations/audit/, keyed by the spool file name, independent of from_session — the requester cannot skip it by omitting the field) | ◇Rust test (incl. shadow_audit_covers_requests_without_from_session + the bound-ledger landing pin) | P1 |
| D4 | Error sanitization | Various constructed failures | Error text contains no host absolute paths | ◇python (mirrors ○`test_error_messages_do_not_leak_absolute_paths`) | P0 |
| D5 | No out-of-scope writes | Whole flow | Never writes `~/.pinvou3/sessions/*.json`, never touches schtasks/cron/systemd (tool and watcher write only `~/.pinvou3/task-requests` and the domain directories) | Code review + grep checks | P1 |
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
- **The real boundary of unattended-recursion protection (round-8 M1 revision)**: the deterministic defense is the engine-side `unattended_disallowed_tools` (deny list) +, from round-8, **subagents disabled** for sched- hidden sessions (`agent(inherit_disallowed_tools=false)` can no longer shed the deny list — the sub-runtime keeps MCP+auto-approval, and the posture list does not include this family's three write tools). There are THREE residual channels (corrected in round-27 M1; a third added and closed in round-30 REQUIRED-2). First, **shell is not required**: scheduled tasks pin Yolo (`tasks.rs`) → trust mode accepts absolute paths in `write_file`, `~/.pinvou3` is not in the default denied subtrees, and the unattended deny list is named tools only — so the file tools alone can write valid spool records into `~/.pinvou3/task-requests/` (an existing concession of the claimed-string filter). Second, **the messaging spool is a same-class channel that launders across sessions**: `features/messaging/mod.rs` describes that directory as "an unauthenticated local side door" (no digest binding, optional `from_session`) — an unattended run's file tools can write it, and delivery then injects an ordinary-catalog turn into any plain session whose catalog includes this family's three write tools (the file-write twin of the indirection round-9 M2 fixed at the tool layer). Third, **the registry launchers could widen the same turn's tool surface**: `registry_sync`/`start_registry_mcp_server` ride the app allowlist on sched- engines, auto-approval Allows them, and the foundation's full-access hold list names only `start_mcp_server` — launching a user-cached registry candidate made its `mcp_<name>_*` tools join the SAME turn under fresh identities this name-keyed shield could not see. **This wave adds both launchers to `unattended_disallowed_tools`** (the engine-side named deny list, round-30 REQUIRED-2), closing that escape; adding the launchers to the foundation hold list is the complete fix on the CodeWhale side (tracked upstream per the fork boundary).
- The panel's existing "create via conversation" flow stays untouched; both channels converge on the same `create_task` domain function.
- This branch carries **no** fork-register refreshes or gitlink-chase commits — the gitlink follows the base main (corrected in round-27 M2); `docs/fork-modifications.md` is maintained by each landing PR on its own; this plan document itself lands with this branch as the review baseline for the implementation.
- The domain `parse_rrule` actually supports CRON; the tool layer's tightening to the three-form subset is a deliberate product constraint; opening it up later only touches the server.py + watcher subset validation, not the domain.
- **An over-limit task is invisible to the model (round-30 S6)**: a panel-created task whose prompt exceeds the 256 KiB store-read cap (the domain imposes no length limit) answers "not found" on `read` and is silently skipped by `list` — the model's mandated pre-create dedupe then duplicates it. Disclosed here and in the read error semantics; a finer "present but unreadable" signal is future work.
- **Run outcomes are not observable (round-27 minor 12)**: `list`/`read` expose status/nextRunAt/lastRunAt only — the model cannot read run output (session-reader hard-rejects `sched-` targets; no bypass). The replaced foundation `automation` tool was richer here (its read included run records). Acceptable under the single-user semantics; disclosed.
- **Feature-switch copy ships ahead of a settings page (round-27 minor 13)**: the trilingual copy for the `scheduled-task-automation` switch is in place, but this PR ships no automations settings page — the switch is reachable via command/config today (contract §3.1's "wired into each feature's own settings page as the feature lands" is honored by a follow-up PR).
- **I/O primitive hardening disclosures (round-24 minor 13)**: the unix write primitives (`open_private_append_file`/`open_private_tmp_write_file`) carry `O_NOFOLLOW|O_NONBLOCK` — a user's legitimately symlinked diagnostics/audit log path now **fails closed** (a deliberate behavior change; before round-17 symlinks were followed); the tmp-write primitive does not re-tighten the mode of a pre-existing leftover tmp (no secrets in the payloads; same private directory); the Windows write primitives have no std-level reparse-point refusal (documented residual of the same-user spool/state tree).


---

## 9. Implementation Addendum: Full CRUD (2026-09-29, finalized on the same branch)

The first design (§1–§8) covered only create + list. Per review feedback it is extended to full CRUD; this section is its review baseline; where §1–§8 conflict with this section, this section wins.

### 9.1 Tool Surface (replaces §3.1)

| Full tool name | Level | Parameters |
|---|---|---|
| `mcp_app-automations_create_scheduled_task` | L1 | Same as §3.1 |
| `mcp_app-automations_read_scheduled_task` | L0 | `task_id` (required); returns the single task's full fields, **including prompt** (for inspection before update) |
| `mcp_app-automations_list_scheduled_tasks` | L0 | Same as §3.1 (no prompt) |
| `mcp_app-automations_update_scheduled_task` | L1 | `task_id` (required) + `name`/`prompt`/`rrule`/`model_id`/`paused` (all optional, at least one); rrule per the same product subset |
| `mcp_app-automations_delete_scheduled_task` | L1 | `task_id` (required); archive-then-delete, destructive |

### 9.2 Design Decision Addendum

| Decision | Conclusion | Rationale |
|---|---|---|
| Granularity | Five `verb_noun` tools, not one mode-enum tool | Contract §4.2 prefers one tool + mode, but read and write carry different approval semantics: merging would put L0 reads behind the L1 Ask gate (harassing) or leave writes ungated; same reasoning as the session-reader three-tool precedent |
| Delete authorization | delete is L1; the typed Ask rule is registered, but the current product is full-auto approval in both modes (session_policy `approval_params` = (true, Auto)), so no per-call prompt appears at runtime | Honest disclosure: writes (including delete) take effect immediately; the actual review entry points are the audit log, the timeline result card, and the scheduled-tasks panel; the Ask rules are reserved as the enforcement point once approval differentiation (S-1) lands. Delete follows the panel's same archive-then-delete pipeline (history stays traceable) |
| Spool extension | The record gains `kind` (`create`/`update`/`delete`) + `task_id`; every other field becomes optional | Contract §4.4 allows additive fields only: old create records keep working with the watcher defaulting `kind=create` |
| Idempotency naming | `sha256("<from_session>\|<kind>\|<task_id>\|<key>")`; the key must travel with from_session | The namespace narrows per sender and does not degrade to global when from_session is absent; different operations sharing a key do not stomp each other (create and update can share one key without overwriting each other) |
| Target precheck | update/delete probe the target's existence server-side first | Fail fast saves a spool round-trip; the watcher and the domain layer still re-check (server side not trusted) |
| Ask rules | update/delete register typed Ask rules like create; read/list get none | Reads stay ungated (no harassment); the rules do not prompt under the current full-auto approval and are reserved for differentiation; unattended recursive self-modification is deterministically rejected by the engine-side `unattended_disallowed_tools` (the watcher's sched-/eval_/aux- sender rejection is only a claimed-string filter) |

### 9.3 Acceptance Matrix Addendum (continuing §5 numbering)

| # | Scenario | Expected result | Verification | Priority |
|---|---|---|---|---|
| I1 | Update end to end | update changing name/rrule/paused persists and refreshes live | ◇Rust `update_request_changes_the_existing_task` + manual | P0 |
| I2 | Delete end to end | delete follows archive-then-delete; the task leaves the panel, history traceable | ◇Rust `delete_request_archives_and_removes_the_task` + manual | P0 |
| I3 | Read details | read returns the full fields including prompt; unknown id is an explicit not found | ◇python + manual | P0 |
| I4 | update validation | Missing task_id / zero fields / CRON rrule all rejected | ◇python + ◇Rust `update_request_validation_requires_target_and_a_field` | P0 |
| I5 | delete validation | Missing task_id and extra fields both rejected | ◇python + ◇Rust `delete_request_validation_rejects_extra_fields` | P1 |
| I6 | Ask-rule extension | The ruleset carries Ask entries for update/delete; read/list carry none | ◇Rust `scope_deny_ruleset_asks_for_scheduled_task_create` extension | P0 |

The §6 test mapping extends accordingly: the python suite covers I3/I4/I5, the smoke journey covers the read/update/delete validation paths, and the result-card tests cover parsing all three operations' results.

## Review follow-up: bounded drain and evidence retention

- Each non-empty drain pass reads only `disabled_builtin_features` from the authoritative settings file. It does not load preferences, probe language, or migrate credentials. Invalid/unreadable settings pause the drain with one diagnostic per failure streak; fixing the file resumes it on the next poll. Empty spools need no settings read.
- Vanished spool names release retry/park entries. Non-UTF-8 filenames are quarantined with their original bytes; they cannot produce a protocol receipt. A pass emits at most one panel refresh for its successful operations.
- Quarantine uses native atomic no-replace rename (Linux `RENAME_NOREPLACE`, macOS `RENAME_EXCL`, Windows `MoveFileExW` without replace). A collision retries a suffix without overwriting existing evidence, including a concurrent writer. Unsupported platforms or filesystems return an explicit error and leave the source queued/parked.
- The 14-day receipt retention does not remove a marker while its spool filename still exists. This keeps an applied request deduplicated across persistent unlink failures. Once the spool name disappears, normal retention resumes. Evidence and stale server temporary files retain the 14-day policy.
- Model-binding updates rely on the store's own snapshot restoration on persistence failure. The redundant second rollback write was removed so it cannot overwrite a concurrent CLI rebind; previously persisted task fields remain applied and retriable.
- Private write helpers reject Unix symlinks and non-regular files. A pre-existing hard-linked regular temporary file is still accepted and can be truncated through its linked inode; the private-directory/same-user authority boundary applies. Windows reparse-point residuals are unchanged.
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
- **Shadow audit rotation**: the long-lived `automations/audit/workflow_audit.jsonl` rotates before append once it exceeds 2 MiB, retaining one `workflow_audit.jsonl.old` segment. Normal growth is bounded by the threshold plus one event per segment, and the check is based on disk state across restarts. A larger pre-existing legacy segment is preserved once until the next rotation replaces it. Rotation uses the shared best-effort log primitive; concurrent app instances are not serialized. If rotation fails, existing evidence is kept and the new shadow append is skipped with a diagnostic.
- **Server responsiveness**: the server is single-threaded. While a write waits up to five seconds for its receipt, `ping`, `tools/list`, and all other calls on the same server wait too; N queued writes can serialize into N times that wait. The pending response remains the recovery path. This follow-up does not introduce threads or change protocol ordering.
- **Failure receipts**: even a foreign-written marker has its known home, task-queue, and automation-store path prefixes redacted before its error enters model context. Non-text errors use the generic failure message. Marker content remains unauthenticated local input; this is path redaction, not provenance verification.
- **Surrogate key**: the existing UTF-8 validation and its regression are retained. The direct test entry point is after all test classes so direct invocation also exercises the prior follow-up cases.
