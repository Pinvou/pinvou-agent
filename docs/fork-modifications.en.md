# CodeWhale Fork Modification Register

> Updated: 2026-08-25. Public maintenance baseline: upstream `v0.9.5` r10. Canonical Chinese register: [`docs/fork-modifications.md`](fork-modifications.md). This English page is a condensed summary; the Chinese version is the complete, authoritative register.
>
> 2026-08-22 corrections: (1) the parent gitlink bump from r6 (`3bbf8421`) to r7 happened in parent PR #285 (`95502ac8`), not in PR #302 — PR #302 started from a pre-#285 main and merged without touching the gitlink; PR #305 later advanced the published baseline to r8. (2) PR #302 (capability-bundle unification, parent commit `c75f2fb2`) updated the parent-side scope model — the single `disabled_bundles.json` (package id × mode, plus `hidden_scopes`) replaced the separate `disabled_connectors.json` / `disabled_skills.json` files.

## Current baseline

| Item | Value |
|---|---|
| Upstream | `v0.9.5` at `853cb707bbcf4f7dc4268fba6d811e0d04083f9c` |
| Public maintenance branch | `Pinvou/CodeWhale:pinvou3-clean` at `feb8761ae` (`pinvou-v0.9.5-r10`) |
| Merged fixes | `Pinvou/CodeWhale#9`, `#11`, `#12`, `#13`, `#15`, `#16`, `#17`, and `#19` are merged |
| Published status | `pinvou3-clean`, `pinvou-v0.9.5-r10`, and the parent gitlink resolve to `feb8761aeda31749f3d54c6e1f8ef460540567a1`; `r1` through `r10` remain immutable historical tags |
| Previous baseline backup | Tag `pinvou-v0.9.0-r4` and branch `backup/pinvou3-clean-v0.9.0-r4`, both at `03e9e1027c03ce1e4b35ab9e3ccce751b65b9624` |
| Drift | Published r10 baseline: 64 files, `+5612/-702` |
| Organization | Four current long-lived topics; PR #13 removes the product-specific orchestration topic |
| Guard inventory | Published r10 has 41 CodeWhale `forkguard_*` tests, plus two generic tool-compatibility regressions and parent fingerprints/behavior tests |

### r10 fixed-sampling and compaction-usage boundaries (published)

- CodeWhale PR #19 was published as `feb8761aeda31749f3d54c6e1f8ef460540567a1`. The Kimi Code membership route strips non-default sampling only for the exact membership roster (`k3`, `k3-256k`, `kimi-for-coding`, and `kimi-for-coding-highspeed`). DeepSeek keeps a compatibility shim only for exact `deepseek-v4-flash` Responses calls, while the Chat dialect preserves the documented 0..=2 sampling contract.
- K3 and K3-256K reuse the existing membership route, reasoning dialect, and model metadata paths. `k3-256k` is fixed at 262,144 context tokens so the generic name hint cannot reinterpret it as 256,000; bare `k3` alone retains the existing 1M entitlement path.
- `CompactionCompleted.post_input_tokens` uses the engine's canonical estimate for the complete compacted request, including the system prompt and merged summary. The parent updates the usage chip immediately and persists a non-turn `context_snapshot`, so remounting does not restore the pre-compaction value.
- r10 adds 17 files and `+370/-31` over r9, including four new `forkguard_*` regressions. The change stays within the existing T1 routing and host-event boundary and adds no long-lived fork topic.

### r9 conversation insertion and edit boundaries (published)

- CodeWhale PR #16 was published as `8aa5f77d35ac1d00d1f444193543307a7e9b391c`. Steer now returns an opaque id, reports `SteerCommitted` / `SteerDropped`, preserves or retires uncommitted input according to the explicit cancel mode, and deterministically terminates foreground Shell work owned by the cancelled turn.
- CodeWhale PR #17 was published as `07d183e350ce4a1ed4f91bdfa1875c996e710d2b`. `EditLastTurnTarget` distinguishes editable text, unsupported latest user content, and a missing target. Tool results, internal runtime envelopes, and non-authoritative provenance cannot become edit points; genuine unsupported latest user content also cannot be skipped in favor of older text.
- Edit preflight rejection uses stable nonrecoverable `edit_last_turn_*` codes and one authoritative `TurnComplete(Failed)`. The parent suppresses optimistic fallback persistence and uses `chat:done.operation_rejected` to hydrate the unchanged durable transcript in both Tauri and Web clients.
- r9 adds 18 files and `+1998/-178` over r8. The retained volume is the concurrency/state coverage for cross-interrupt steer ownership, Shell termination, and provenance-aware history classification; these invariants belong in the Engine lifecycle and are prioritized for upstreaming as generic host APIs.

### r8 per-turn evaluation security extension (published)

CodeWhale PR #15 combined candidate `1eca6103a` with security follow-ups `169c24cc5`, `21e5f661a`, and `a647ed866`, then squash-merged them as `d127aed113529dc93754d044b9f352e9746f6b83`. The merge commit has the same tree as the verified candidate head and is published as immutable tag `pinvou-v0.9.5-r8`. It adds a process-local per-turn tool policy, complete trusted-path replacement, an exact final dispatch gate, read-only `File` action schema projection with a repeated read-only check before final execution, and denial of queued goal continuation, edit replay, and MCP reload while restricted. Restricted turns also block queued control-plane operations, hooks, MCP initialization, dynamic tools, and child agents. After a restricted turn, idle child-agent completion and background-Shell wake remain deferred until an explicit message installs replacement authority; read-only `Bash` uses the hardened `ShellPolicy::ReadOnly` direct-argv path. Tool logs and audits retain only non-private identity fields. At r8 publication, the parent gitlink and verifier aligned strictly to that immutable tag; r9 is now the active public baseline.

### Published session fix

- v0.9.5 `load_session` treats an unmatched `tool_use` as evidence of a crashed process. That assumption is invalid when Pinvou persists a live tool call and reads the same session again during the turn.
- The engine fix was merged through `Pinvou/CodeWhale#11`; its public commit is `2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd`.
- T1 now separates side-effect-free `load_session_snapshot` from explicit `recover_session_for_resume`. Pinvou uses snapshots for all runtime read-modify-write paths and performs durable recovery only during app process startup, before any Engine can own a session.
- Revision reconciliation remains fail-closed only for genuine cross-client turns. A local `chat:done` immediately releases the next send, readback failures cannot block ordinary local chat, and cross-client pending notices are deduplicated per session.
- Two CodeWhale tests, two parent `forkguard_*` tests, and Tauri/Web frontend behavior coverage protect side-effect-free runtime reads, observable and idempotent explicit recovery, safe secondary Store opening, durable startup recovery, and consecutive sends after local completion.
- The fix is included in the published head, drift figures, and immutable tag `pinvou-v0.9.5-r5`; CodeWhale required checks and parent automation pass.

## Topics

PR #13 was squash-merged as `a36e6cd533024cfe5724bae21875aea42b2ed87a` and published as immutable tag `pinvou-v0.9.5-r7`. It removes product-specific orchestration while preserving canonical registry prompt text and alias-aware Custom SubAgent allowlist resolution.

1. **Host embedding and routing boundary** — `331cb1594688c723d98499d9ca11f05af291b599`, `2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd` (`Pinvou/CodeWhale#11`), `a36e6cd533024cfe5724bae21875aea42b2ed87a` (`Pinvou/CodeWhale#13`), `8aa5f77d35ac1d00d1f444193543307a7e9b391c` (`Pinvou/CodeWhale#16`), `07d183e350ce4a1ed4f91bdfa1875c996e710d2b` (`Pinvou/CodeWhale#17`), and `feb8761aeda31749f3d54c6e1f8ef460540567a1` (`Pinvou/CodeWhale#19`). Exposes only the library modules and narrow host seams needed for Fleet roster loading, live-worker projection, resolved routes, runtime snapshots/recovery, bulk cancellation, terminal facts, reliable steer ownership, authoritative edit-target classification, exact provider sampling compatibility, and complete post-compaction usage estimation. Edit rejection cannot call the provider or mutate history, and the app reconciles an optimistic edit from the durable transcript. Fixed-sampling compatibility is limited to the exact Kimi Code membership model roster and exact `deepseek-v4-flash` Responses route; the Chat dialect preserves its documented contract, and gateways plus other models remain untouched. `CompactionCompleted` carries the canonical complete post-compaction input estimate so hosts can refresh and persist usage without duplicating engine token accounting.
2. **Tool compatibility and command-execution safety** — `595adce47e2d1bcf895d7bfd6426c074eb969324`, `3bbf8421ebdb16bff71f83dac4d42c8fb65f0f02` (`Pinvou/CodeWhale#12`), `a36e6cd533024cfe5724bae21875aea42b2ed87a` (`Pinvou/CodeWhale#13`), `d127aed113529dc93754d044b9f352e9746f6b83` (`Pinvou/CodeWhale#15`), and the Shell-cancellation boundary in `8aa5f77d35ac1d00d1f444193543307a7e9b391c` (`Pinvou/CodeWhale#16`). Adds host `extra_tools`, dynamic `SetDisallowedTools`, file-size enforcement, fail-closed multiline command safety, schema-bound JSON-container repair, provider-role-safe continuations, canonical registry instructions, and alias-aware Custom SubAgent allowlists while reusing upstream `allowed_tools`. Cancellation terminates only foreground Shell work owned by the current turn instead of relying on dropped futures. The r8 extension layers an exact catalog/final-dispatch policy, trusted-root replacement, latched control-plane denial, hook opt-in, restricted log/audit redaction, and deferral of idle child/Shell continuations until an explicit replacement-authority message, while preserving the legacy `None` path. The parent GAIA integration explicitly requires read-only dispatch, projects the model-visible `File` schema to read/list/search actions, repeats the read-only check before final execution, and projects `Bash` into the hardened read-only Shell context. These generic seams remain prioritized for upstreaming; Pinvou's GAIA profiles stay app-owned.
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
- All 41 CodeWhale `forkguard_*` tests pass for the published r10 baseline, including the four fixed-sampling and compaction-estimate regressions from `Pinvou/CodeWhale#19`.
- Parent `./scripts/fork-guard.sh` passes with 21 app forkguard tests; both admitted-display edit-target regressions pass separately.
- The Tauri/Web scheduled-task unit harness, architecture guard, version check, CI-policy tests, and strict public-submodule verifier pass.
- Full product results are recorded in `docs/codewhale-upgrade-0.9.0-to-0.9.5.md`.

Any fork-distinct change must update this register, guard fingerprints, and a result-oriented behavior test, then pass `./scripts/fork-guard.sh --fast`.
