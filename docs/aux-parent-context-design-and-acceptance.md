# aux parent-context (辅助侧聊受限读取主任务) — design and acceptance

> Status: **PR 1 landed, implementation pending** (PR 1's decision + registration landed on `docs/aux-scoped-read-adr`; the §3–§8 implementation lands in this stacked PR — the remaining ○ rows are the live-model manual QA halves that need a dev build). Companion decision record: **ADR-0024**. Companion contract: `docs/builtin-toolset-contract.md` §5 isolation precedent (amendment landed in PR 1).
> Baseline: branch `feat/aux-parent-context` from `origin/main` @ `582e917c6` (post-#640); worktree `pinvou-agent-wt-aux-context`. All file:line anchors below are at that commit.
> Coordination disclosure: the `feat/session-creation-tool` chain (messaging + scheduled + creation, round 7+) rewrites `server.py` heavily and touches `manifest.json` / `marketplace/builtin.rs`. This feature's server edits are **additive and localized** (a new `--only-session` arg + scoped-mode branches); merge order is independent either way — when that chain lands, merge `origin/main` in-branch and resolve (expect conflicts only in `server.py` test groups and the registry-aggregate test).

## 1. Background and goal

The auxiliary side chat (PR #433; ADR-0006 supplements 2026-09-15+) is deliberately **blind**: its engine is isolated (empty tool table, minimal `AUX_SESSION_INSTRUCTIONS` at `bridge.rs:2218`, no MCP / subagents / memory / vision — the `is_aux` branch at `bridge.rs:2302`), and the instructions explicitly promise "不访问主任务的执行与上下文". The user must paste or quote context manually. That zero-knowledge stance was a deliberate #433 design residual, not an oversight.

The session-reader builtin plugin (PR #585) is now in main: read-only L0 `read_session` (cursor-paginated, completed-turns-only, per-item truncation, untrusted-content contract) + `list_sessions`, rejecting the isolated prefixes `sched-`/`eval_`/`aux-` (`server.py:322`).

**Goal**: give the aux engine exactly **one** scoped tool — a session-reader instance pinned at spawn time to the parent task's session id — so the side chat can answer "这个任务进行到哪了 / 刚才的报错什么意思 / 总结一下结论" from the parent's real history. The zero-tool invariant is upgraded to a **scoped-read-only invariant**; every other isolation property is preserved byte-for-byte. Simultaneously: land the decision record and the read-surface registration (PR 1), and reserve explicit interfaces for future aux capabilities.

Not in scope (v1): any aux-panel UI affordance (chips, buttons, indicators — the capability is engine-side and model-driven), the session-mention UI reuse (that machinery lives on the unmerged `wt-pr586` branch and is NOT a dependency), any write-family aux ownership, an independent feature switch.

## 2. Delivery plan

| PR | Branch | Contents | Depends on |
|---|---|---|---|
| PR 1 — decision + registration | `docs/aux-scoped-read-adr` (may be delivered from this same branch first) | ADR-0024 + ADR-0006 dated errata; read-surface registration (doc comments + pins) for `export_session` / `get_session_timeline`; the two stale "no producer" comments in `server.py`; the dropped remote-control denylist comment reword (commit `ef48f0701` was dropped by a branch rewrite before it could merge — it never reached main in any form; the hash is unrecoverable in-repo, restorable only from GitHub); contract §5 amendment | — |
| PR 2 — implementation | `feat/aux-parent-context` (this branch) | Everything in §3–§8 below | PR 1 (ADR reference) |

One-PR fallback: if review load prefers it, PR 1's files ride the same PR with an explicit section split — the matrix ids stay identical.

## 3. Design decision record

| Decision | Outcome | Rationale |
|---|---|---|
| Capability shape | Exactly one model-visible tool: `mcp_session-reader_read_session`. `list_sessions` stays server-side guarded but is NOT in the aux allowlist | The parent's sessionId rides the instructions; a listing adds surface without adding information. Minimal catalog = minimal trained-tool wandering |
| Scope anchor | Spawn-time `--only-session <parent_id>` arg on a per-aux MCP config file; the server enforces the scope process-side | Identity is app-anchored (the bridge derives the parent from the aux id), never model-claimed. Matches the browser wrapper's per-session-config precedent (`write_work_mode_mcp_config`, `extraction.rs:1181`) and the server's existing argparse pattern (`--sessions-dir`, `server.py:228`) |
| Boot gating | `Feature::Mcp` **enabled** for aux + `mcp_config_path` → per-aux file containing ONLY the scoped session-reader entry | `Feature::Mcp` gates boot; `allowed_tools` only filters the catalog (foundation `engine.rs` `start_mcp_session_boot`). A dedicated one-entry file means the boot starts exactly one local python subprocess — no user MCP servers, no network |
| Per-turn enforcement | The chokepoint at `bridge.rs:3199` becomes three-way: `is_aux(engine's own id)` → scoped list; `restrict` → empty; else full. The aux test keys on the **engine's own session id**, preserving the `TurnToolRestrict` "a bogus token cannot un-restrict aux" property (`engine_pool.rs:376-423`) | Same server-side pinning stance #433 established; `Op::SendMessage.allowed_tools` accepts exact lists (foundation `project_exact_allowed_tools`) so no foundation change is needed |
| Parent derivation | Case-insensitive strip of the `aux-` prefix, parent part kept verbatim; isolated-prefix parents (`aux-sched-x`, `aux-aux-x`, `aux-eval_x`) → `ZeroTool` fallback (legacy branch: `Feature::Mcp` disabled, empty tools) | Fail-closed: by construction aux parents are normal sessions (store rejects sched/aux parents at creation), but the config builder must not trust construction. The fallback is byte-identical to today's behavior |
| Decision center | One function, `aux_tool_surface(session_id) -> AuxToolSurface { ZeroTool, ParentScopedRead { parent_id } }`, consumed by both the spawn-config branch and the per-turn chokepoint | Every future capability change (multi-session scope, write family, indicator) funnels through this one seam — the core reserved interface (§5 R1) |
| Instructions language | `AUX_SESSION_INSTRUCTIONS` stays zh; rewritten to name the tool + parent id + untrusted stance | #433's disclosed stance (translating the model-visible stack is an untested behavior change); the rewrite itself is a behavior change — recorded in ADR-0024, verified by manual QA (A1) |
| Reminder | `AUX_ZERO_TOOL_REMINDER` (`engine_pool.rs:359`) renamed `AUX_SCOPED_TOOL_REMINDER`, reworded: exactly one tool, its scope, no other markup | Markup-emission is trained behavior (the round-31 M8 lesson); the reminder restates the boundary next to the user message |
| Feature gating (v1) | No independent switch; the scoped instance rides `read_session`'s existing `session-mention`+`long-memory` union (both off → structured `feature_disabled`) | Union semantics are instance-global in the manifest; an independent `aux-context` switch needs an instance-scoped mapping override — reserved (§5 R3), not built |
| Scoped `list_sessions` | Returns exactly the one scoped session's metadata entry (empty list if the record is absent) | Graceful if the allowlist ever widens; a cheap parent-exists probe; zero secret surface |
| Server version | `manifest.json` 1.0.0 → 1.1.0 | Behavior addition; builtin packages re-release by byte comparison (`ensure_package_released`) |

## 4. Components and data flow

| Component | Location | Responsibility |
|---|---|---|
| Scope arg + scoped mode | `pinvou3-app/resources/mcp-servers/session-reader/server.py` (modified) | `--only-session <id>`: validated at startup (charset, ≤128, isolated prefixes case-insensitively rejected → refuse to start). Scoped `read_session` accepts only the scoped id (structured error otherwise); scoped `list_sessions` returns the single entry. `tools/list` unchanged — the engine allowlist filters |
| Manifest | same dir, `manifest.json` (modified) | version bump only; `mcp_tools` / `tool_features` unchanged (the aux instance is spawned from an app-written config, not the marketplace install) |
| Per-aux config writer | `pinvou3-app/src-tauri/src/features/runtime_bundle/platform/extraction.rs` (new fn next to `write_work_mode_mcp_config`) | Build the one-server entry reusing the marketplace resolution (`connectors.rs` `local_server_command` / `local_server_args`: python command + released `~/.pinvou3/bundles/session-reader/mcp/server.py`), append the pin as `--only-session=<parent>` (the `=` form is dash-immune: the charset admits leading `-`, and a two-token pin would make argparse kill the server at boot — review round 3); write `{"servers":{"session-reader":…}}` to a token-named path; `atomic_write_private`, idempotent |
| Token path | `pinvou3-app/src-tauri/src/platform/paths.rs` (new `aux_session_mcp_json`) | Mirrors `browser_session_mcp_json` (fnv1a digest filename — no raw session id in the path), under a NEW top-level `~/.pinvou3/mcp-sessions/` dir (not shared with the browser layout at `~/.pinvou3/browser/mcp-sessions/`) |
| Decision center | `features/assistant/engine_pool.rs` (or bridge) — `AuxToolSurface` + `aux_tool_surface` | The single seam both consumers call (§3) |
| Spawn config | `features/assistant/platform/bridge.rs` aux branch (~2302, rewritten) | `allowed_tools = Some([read_session])`; keep minimal instructions (rewritten text), subagents/memory/vision/tools off; **enable** `Feature::Mcp`; `mcp_config_path` → the per-aux file (written here, like the work-mode path at ~2312) |
| Per-turn chokepoint | `bridge.rs:3199` three-way branch | §3; `EditLastTurn` resends inherit the engine config's scoped list automatically (no per-turn surface on that op) |
| Reminder | `engine_pool.rs` rename + reword; merge sites unchanged | §3 |
| ADR-0024 + registers | `docs/adr/0024-…md` (new), ADR-0006 errata, contract §5 row, registration doc-comments | PR 1 (§2) |

Data flow: aux panel send (unchanged wire) → pool `send_reserved_user_message` → chokepoint composes scoped allowlist → engine boots (first spawn) with the per-aux config → one python `server.py --only-session=<parent>` process → model calls `read_session(parent)` → paginated untrusted turns → answer rendered in the aux panel.

## 5. Reserved interfaces (预留接口)

- **R1 — `AuxToolSurface` decision center**: every future aux capability question (multi-session scope, write-family admission, model indicator, scheduled delivery) starts as a new variant/consumer at this one seam, not a scattered `is_aux` check.
- **R2 — `--only-session` takes one id now; documented to widen to a list** (comma-separated or repeated flag) when aux-side @-references become possible (post-`wt-pr586` merge). The server's scope check is written set-ready.
- **R3 — `--gate-feature <id>` slot**: instance-scoped feature-mapping override (aux instance gates on `aux-context` alone), documented in ADR-0024 as the v2 kill-switch design; not implemented.
- **R4 — the per-aux config writer is generic over server entries** (a map, not a hardcoded singleton) so a future aux-scoped server joins by data, not by a new writer.
- **R5 — ADR-0024's deferred-decisions register**: write-family aux ownership (contract §5 "explicit ownership design"), S1 model propagation + panel indicator, scheduled delivery into aux. Decisions are recorded, not silently open.

## 6. Disclosed limitations and boundaries

- **No UI in v1**: no panel change, no i18n keys. The model decides when to consult the parent; a visible affordance is follow-up work.
- **Prompt-injection surface widens**: instructions inside the parent transcript can now reach the aux model's answers. Mitigated by the untrusted contract (tool description + instructions wording) — the same stance as session-mention; disclosed in ADR-0024.
- **One python subprocess per live aux engine** (idle-reclaimed with the engine). Bounded; disclosed.
- **Union-gated kill switch**: turning off aux context requires disabling both `session-mention` and `long-memory` (v1); the scoped instance then returns `feature_disabled`. Not independent — R3.
- **Rust shape pins on instruction/reminder text are `contains()`-vulnerable** (house-known gap from #433's register); accepted with disclosure.
- **The aux transcript now contains tool_use/tool_result blocks** — the aux panel's chat-item projection has never rendered a tool call (the zero-tool era had none). Verified under A5; a minimal neutral rendering is acceptable v1 (the generic MCP tool card if it fits, else a plain system-style item). No first-party breakage is acceptable.
- **Per-aux config files are not reclaimed on aux deletion** (review round-1 registered follow-up): `~/.pinvou3/mcp-sessions/<token>.json`, one tiny secret-free file (resolved paths + parent id) per task, persists after the aux session is deleted/reset — same retention stance as the browser per-session configs. Follow-up: delete the token file on the aux delete/reset paths.
- Zero CodeWhale changes (`fork-modifications.md` untouched); no new Tauri commands; no new events.

## 7. Acceptance matrix

● = landed (automated). ○ = pending (the remaining ○ rows are the live-model / dev-build manual halves). Priorities: P0 blocking, P1 should, P2 nice.

### A — end-to-end capability (manual QA on the dev build)

| # | Scenario | Expected | Verification | Pri |
|---|---|---|---|---|
| A1 | Ask about the parent in the side chat ("这个任务进行到哪了") | The aux model calls the scoped `read_session` and answers from the parent's actual turns — no hallucinated history; answer notes it consulted the task | ○ Manual QA (live model) — the only verification for the rewritten zh instructions (F1's disclosed gap) | P0 |
| A2 | Parent with >20 turns | The model paginates via `nextCursor` instead of stopping at page 1 | ○ Manual (server-side paging is pinned: `test_pagination_and_cursor_work_inside_the_scope`) | P1 |
| A3 | Parent deleted while the panel is open | `read_session` → structured `not_found`; the aux answer degrades to "无法读取" (the existing out-of-band-deletion UX applies) | ● python `scoped_read_returns_structured_not_found` (`test_deleted_parent_returns_structured_not_found`) + ○ manual UX half | P1 |
| A4 | Side chat on a task with no aux-relevant question | The model does NOT call the tool gratuitously (instructions say when-not-to) | ○ Manual | P1 |
| A5 | Aux transcript with a tool call | The aux panel renders the `read_session` card (generic MCP card or neutral item) without breakage | ● executing `projectAuxChatTurns … generic tool item (A5)` (`aux_chat_state.test.mjs`) + contract pin (`aux_chat_panel_contract.test.mjs`) + ○ manual render check | P0 |

### B — spawn-config invariants (rewrite of `bridge.rs:6439` `aux_session_is_tool_free_on_spawn_config_and_send_op`)

| # | Scenario | Expected | Verification | Pri |
|---|---|---|---|---|
| B1 | Aux engine config | `allowed_tools == ["mcp_session-reader_read_session"]` exactly; `Feature::Mcp` **enabled**; `mcp_config_path` == the per-aux token file; instructions still exactly one inline aux persona (rewritten text); subagents/memory/vision off; `tools == None` | ● Rust `aux_session_tools_are_scoped_read_only_on_spawn_and_send` | P0 |
| B2 | Per-aux config file content | Exactly one server `session-reader`; args end `--only-session=<parent>` (= form, round-3 fix); resolved python + released server.py path; **no user servers, no browser entry, no timeouts drift** | ● Rust writer test (`aux_mcp_config_writes_exactly_one_scoped_session_reader_entry`) + the bridge leg inside B1's test | P0 |
| B3 | Writer hygiene | Token filename (fnv1a — raw session id absent from the path); file 0600 / dir 0700; identical content → rewrite skipped | ● Rust (same test; the skip branch is proven by a read-only-dir probe on unix) | P0 |
| B4 | Case-variant aux ids | `AUX-1` / `Aux-1` / `aUx-1`: scoped surface applies, parent part case preserved | ● Rust truth table (`aux_tool_surface_truth_table`) | P0 |

### C — per-turn enforcement

| # | Scenario | Expected | Verification | Pri |
|---|---|---|---|---|
| C1 | Aux send with caller `restrict=false` | `Op::SendMessage.allowed_tools == scoped list` (chokepoint keys on the engine's own id); demotion-aware — empty when the per-aux config is absent (review round-1 fix) | ● Rust (legs ②/②-b of B1's test) + `aux_turn_allowed_tools_is_demotion_aware` | P0 |
| C2 | Bogus token / unrelated-id token | Cannot hand an aux engine a full-tool turn (`restricts_tools_for` re-check intact); a non-aux restricted turn still gets the empty list | ● `engine_pool.rs` token tests (`send_dispatch_forwards_forced_restrict_to_engine_entry` incl. the mismatched-token leg) | P0 |
| C3 | `EditLastTurn` resend | Inherits the engine config's scoped allowlist (no per-turn surface on that op) | ● `edit_resend_dispatch_merges_aux_scoped_tool_reminder_into_outgoing_message` (+ the spawn-config source pin in B1) | P1 |

### D — decision center

| # | Scenario | Expected | Verification | Pri |
|---|---|---|---|---|
| D1 | `aux_tool_surface` truth table | `aux-{normal}` → `ParentScopedRead`; isolated-prefix parents (`aux-sched-x` / `aux-aux-x` / `aux-eval_x`, case variants) → `ZeroTool`; multibyte ids never panic | ● Rust unit test (`aux_tool_surface_truth_table`; charset-invalid parents also fail closed) | P0 |
| D2 | ZeroTool fallback config | Byte-identical to today's legacy aux branch (`Feature::Mcp` disabled, empty tools, global mcp path) | ● Rust (leg ①-c of B1's test, incl. the verbatim `AUX_ZERO_TOOL_INSTRUCTIONS` pin) | P0 |

### E — server scoped mode (python)

| # | Scenario | Expected | Verification | Pri |
|---|---|---|---|---|
| E1 | `--only-session` validation | Charset/≤128 ok; isolated prefixes (case-insensitive) → refuse to start with a clear message | ● python `OnlySessionArgTests` (incl. a real-stdio refusal-to-start case) | P0 |
| E2 | Scoped `read_session` | Accepts only the scoped id (exact); sibling/other ids → structured "not readable" error | ● python `ScopedReadTests` (+ `ScopedStdioTests` end-to-end) | P0 |
| E3 | Scoped `list_sessions` | Exactly one entry (the scoped session); empty list when the record is absent | ● python `ScopedListTests` | P1 |
| E4 | Feature gate in scoped mode | `session-mention`+`long-memory` both off → `feature_disabled` (union, per-call re-read) | ● python `ScopedFeatureGateTests` | P0 |
| E5 | Untrusted envelope | Injection payloads inside the parent transcript pass through inside the untrusted framing (existing pattern, scoped variant) | ● python `ScopedUntrustedEnvelopeTests` | P0 |
| E6 | Package re-release | manifest 1.1.0; `ensure_package_released` upgrades an existing install | ● smoke (`version` pin + the scoped journey) — the byte-compare upgrade path itself rides the existing `ensure_package_released` mismatch-rewrite tests; ○ manual dev-build half | P1 |

### F — instructions & reminder

| # | Scenario | Expected | Verification | Pri |
|---|---|---|---|---|
| F1 | `AUX_SESSION_INSTRUCTIONS` (rewritten, zh) | Names the single scoped tool + the parent sessionId slot + untrusted stance; still no file/command/network/delegation claims | ● source-shape pin in B1's test (contains() — disclosed vulnerable) | P0 |
| F2 | `AUX_SCOPED_TOOL_REMINDER` | Exactly-one-tool + scope + no-other-markup; merged per-turn for aux ids incl. case variants; persona anchors keep precedence | ● `aux_turn_carries_scoped_tool_boundary_reminder` (content pins + merge matrix) | P0 |

### G — isolation regression pins (existing tests stay green; updated where wording moved)

| # | Scenario | Expected | Verification | Pri |
|---|---|---|---|---|
| G1 | Aux invisibility | Still excluded: global `list_sessions`, sidebar/archived lists, deliverables index, ACP classification | ● existing python + Rust suites stay green (incl. `test_list_excludes_aux_sessions_case_insensitive`) | P0 |
| G2 | Delete cascade | main→aux cascade and the aux reset gate unchanged | ● existing `sessions` tests | P0 |
| G3 | Aux turn dressing | No sudo-status / MCP-inventory reminder in aux turns (persona channel only) | ● updated pin (leg ② of B1's test: bare `content == "hi"` + persona-channel-only wrap) | P0 |
| G4 | Workspace merge surface | The aux execution root's `.codewhale/mcp.json` merge path is private/empty (no user file can ride in) | ● disclosure landed (ADR-0024 costs; the execution root is app-owned and `allowed_tools` filters any merged catalog regardless); the optional pin deliberately not added — it would be vacuous (nothing creates that file, so absence proves nothing about future writers) | P1 |

### H — PR 1 registration items (docs + hygiene) — delivered on `docs/aux-scoped-read-adr`

| # | Item | Expected | Verification | Pri |
|---|---|---|---|---|
| H1 | ADR-0024 + ADR-0006 errata | Decision clauses, read-surface register, reserved interfaces, costs; errata points from 0006's aux supplements (09-15/09-18/09-19/09-25) | ● review (PR 1) | P0 |
| H2 | Stale comments | Both "no producer" claims in `server.py` (module docstring ~L17-19; above `ISOLATED_SESSION_PREFIXES` ~L318-322) replaced with the real producer (`get_or_create_aux_session`, derived id) | ● review (PR 1; grep-clean pin deliberately not added — the suite already pins the isolation behavior) | P0 |
| H3 | `export_session` registration | Doc comment names aux explicitly (deliberate any-id read stance); store test pins aux-id export works | ● landed (PR 1; Rust `aux_session_archive_export_reads_any_id_class`, mutation-checked by review) | P0 |
| H4 | `get_session_timeline` registration | Doc comment names the aux exemption (the read is exercised by id, not sidebar-listed; the normative point is "grow no id-class rejection here"); timing test pins aux-id reads | ● landed (PR 1; Rust `read_timeline_reads_aux_ids_for_panel_hydration`) | P0 |
| H5 | Contract §5 row | Aux isolation precedent amended: scoped read exception (app-anchored, planned-marker + implementation PR reference (the marker flips to landed in this PR)); the model-facing write family admits no aux requester/target, the auxChat relay is the sanctioned channel, deletion interacts only via the designed cascade/reset | ● review (PR 1) | P0 |
| H6 | Denylist comment reword | `remote_control/manager/mod.rs:116-127` re-keyed on the mechanism (execution-surface scope), restoring the `ef48f0701` reword (dropped by a branch rewrite before it could merge) | review; existing denylist test green | P1 |
| H7 | Durable user-facing zero-tool claims | `README.md` / `README.zh-CN.md` ("server-enforced zero-tool / 服务端强制零工具") amended to the scoped-read stance when the implementation lands | ● executed in this PR (README reworded to the scoped-read stance) | P1 |
| H8 | e2e zero-tool assertion | `pinvou3-app/tests/e2e/aux-quote.cdp.mjs` scenario 9 ("aux zero-tools … stays a pure Q&A turn", incl. the zero-tool marker-leak assertion) updated to the scoped surface when the implementation lands — otherwise it asserts the superseded invariant | ● executed in this PR (scenario re-keyed to the scoped surface) | P0 |

### J — gates

| # | Gate | Expected |
|---|---|---|
| J1 | `cargo test --lib` + `fmt --check` + clippy leg1 (`-D warnings`) / leg2 (no `-D`) | green |
| J2 | `npm run test:node` + lint suite (eslint/ox/biome) | green |
| J3 | python session-reader suite + `scripts/mcp-server-contract-smoke.py` (incl. a scoped-instance journey) | green |
| J4 | `python3 scripts/architecture-guard.py`; `./scripts/fork-guard.sh --fast` | pass |

## 8. Test map

| Test (file) | Covers |
|---|---|
| `scripts/tests/test_session_reader_server.py` (new `Scoped*` + validation groups) | E1 E2 E3 E4 E5 (+A3 not_found) |
| `scripts/mcp-server-contract-smoke.py` (scoped journey) | E6 J3 |
| `features/assistant/platform/bridge.rs` inline tests (renamed spawn+send test; writer tests if co-located) | B1 B2 B3 C1 F1 G3 |
| `features/assistant/engine_pool.rs` tests (truth table, reminder, token) | B4 C2 C3 D1 F2 |
| `features/runtime_bundle/platform/` tests (next to the browser-wrapper pins) | B2 B3 D2 G4 |
| `features/sessions/tests.rs` / `features/assistant/timing.rs` | H3 H4 |
| `pinvou3-app/tests/aux_chat_panel_contract.test.mjs` | A5 |
| Manual QA (dev build) | A1 A2 A4 (+A3 UX half) |

## 9. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Aux panel projection meets its first tool_use blocks | A5 pin + minimal neutral rendering fallback; no first-party breakage accepted |
| Parent-transcript prompt injection reaching aux answers | Untrusted contract (description + instructions + reminder); disclosed in ADR-0024; E5 pins the envelope |
| Conflicts with the `feat/session-creation-tool` chain | Server edits additive/localized; merge `origin/main` in-branch when it lands; cargo suites re-run after any merge (the #433 deadlock lesson: always run cargo lib tests after merging main) |
| Scope escape via a second config source | Per-aux file contains one entry; global mcp.json never copied; aux workspace merge path private (B2/G4) |
| Trained markup emission beyond the one tool | `AUX_SCOPED_TOOL_REMINDER` restates the boundary per turn (round-31 M8 lesson) |
| `AUX_SESSION_INSTRUCTIONS` rewrite is an untested behavior change | The #433 disclosure stance: recorded in ADR-0024, zh kept, verified by A1 manual QA |

## 10. Local gate recipe (this ARM64 box)

One cargo suite at a time (shared target-dir fixtures + ENV_LOCK — two concurrent runs SIGTERM/hang). Env:

```bash
export PKG_CONFIG_PATH="$HOME/.local/pipewire-dev/usr/lib/aarch64-linux-gnu/pkgconfig"
export BINDGEN_EXTRA_CLANG_ARGS="-I$HOME/.local/llvm18/usr/lib/llvm-18/lib/clang/18/include"
export RUST_MIN_STACK=16777216
```

Order: cargo lib → fmt → clippy legs → npm test:node + lint → python suite → smoke → arch-guard → fork-guard --fast. DCO sign every commit; PR-language and self-review per AGENTS.md §1 / CONTRIBUTING.md.

Gate status (2026-10-08, this ARM64 box, after the final fmt pass): J1 cargo lib 3042 passed / 0 failed + fmt clean + clippy `-D warnings` clean + clippy all-targets baseline-only; J2 `npm run test:node` 978 passed / 0 failed (after `npm install` in the fresh worktree — the initial 13 file-level failures were missing node_modules, not code) + eslint/ox/biome exit 0; J3 python discover 241 passed + smoke ALL PASS (incl. the scoped journey); J4 arch-guard passed + fork-guard `--fast` 指纹层全过.
