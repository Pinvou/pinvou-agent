# CodeWhale Fork Modification Register

> Updated: 2026-08-21. Canonical Chinese register: [`docs/fork-modifications.md`](fork-modifications.md).

## Current baseline

| Item | Value |
|---|---|
| Upstream | `v0.9.5` at `853cb707bbcf4f7dc4268fba6d811e0d04083f9c` |
| Public maintenance branch | `Pinvou/CodeWhale:pinvou3-clean` at `d127aed11` (`pinvou-v0.9.5-r8`) |
| Merged fixes | `Pinvou/CodeWhale#9`, `#11`, `#12`, `#13`, and `#15` are merged |
| Published status | `pinvou3-clean`, `pinvou-v0.9.5-r8`, and the parent gitlink resolve to `d127aed113529dc93754d044b9f352e9746f6b83`; `r1` through `r8` remain immutable historical tags |
| Previous baseline backup | Tag `pinvou-v0.9.0-r4` and branch `backup/pinvou3-clean-v0.9.0-r4`, both at `03e9e1027c03ce1e4b35ab9e3ccce751b65b9624` |
| Drift | Published r8 baseline: 48 files, `+3222/-471` |
| Organization | Four current long-lived topics; PR #13 removes the product-specific orchestration topic |
| Guard inventory | Published r8 has 34 CodeWhale `forkguard_*` tests, plus two generic tool-compatibility regressions and parent fingerprints/behavior tests |

### r8 per-turn evaluation security extension (published)

CodeWhale PR #15 combined candidate `1eca6103a` with security follow-ups `169c24cc5`, `21e5f661a`, and `a647ed866`, then squash-merged them as `d127aed113529dc93754d044b9f352e9746f6b83`. The merge commit has the same tree as the verified candidate head and is published as immutable tag `pinvou-v0.9.5-r8`. It adds a process-local per-turn tool policy, complete trusted-path replacement, an exact final dispatch gate, read-only `File` action schema projection with a repeated read-only check before final execution, and denial of queued goal continuation, edit replay, and MCP reload while restricted. Restricted turns also block queued control-plane operations, hooks, MCP initialization, dynamic tools, and child agents. After a restricted turn, idle child-agent completion and background-Shell wake remain deferred until an explicit message installs replacement authority; read-only `Bash` uses the hardened `ShellPolicy::ReadOnly` direct-argv path. Tool logs and audits retain only non-private identity fields. The parent gitlink and public-submodule verification now align strictly to r8; the temporary PR-candidate allowance has been removed.

### Published session fix

- v0.9.5 `load_session` treats an unmatched `tool_use` as evidence of a crashed process. That assumption is invalid when Pinvou persists a live tool call and reads the same session again during the turn.
- The engine fix was merged through `Pinvou/CodeWhale#11`; its public commit is `2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd`.
- T1 now separates side-effect-free `load_session_snapshot` from explicit `recover_session_for_resume`. Pinvou uses snapshots for all runtime read-modify-write paths and performs durable recovery only during app process startup, before any Engine can own a session.
- Revision reconciliation remains fail-closed only for genuine cross-client turns. A local `chat:done` immediately releases the next send, readback failures cannot block ordinary local chat, and cross-client pending notices are deduplicated per session.
- Two CodeWhale tests, two parent `forkguard_*` tests, and Tauri/Web frontend behavior coverage protect side-effect-free runtime reads, observable and idempotent explicit recovery, safe secondary Store opening, durable startup recovery, and consecutive sends after local completion.
- The fix is included in the published head, drift figures, and immutable tag `pinvou-v0.9.5-r5`; CodeWhale required checks and parent automation pass.

## Topics

PR #13 was squash-merged as `a36e6cd533024cfe5724bae21875aea42b2ed87a` and published as immutable tag `pinvou-v0.9.5-r7`. It removes product-specific orchestration while preserving canonical registry prompt text and alias-aware Custom SubAgent allowlist resolution.

1. **Host embedding and routing boundary** — `331cb1594688c723d98499d9ca11f05af291b599`, `2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd` (`Pinvou/CodeWhale#11`), and `a36e6cd533024cfe5724bae21875aea42b2ed87a` (`Pinvou/CodeWhale#13`). Exposes only the library modules, narrow root-level Fleet roster API, read-only live-worker projection, opaque resolved-route interfaces, distinct runtime-snapshot versus process-resume session APIs, generic host bulk cancellation, and terminal failure facts required by the host; the full `fleet` module remains private.
2. **Tool compatibility and command-execution safety** — `595adce47e2d1bcf895d7bfd6426c074eb969324`, `3bbf8421ebdb16bff71f83dac4d42c8fb65f0f02` (`Pinvou/CodeWhale#12`), `a36e6cd533024cfe5724bae21875aea42b2ed87a` (`Pinvou/CodeWhale#13`), and `d127aed113529dc93754d044b9f352e9746f6b83` (`Pinvou/CodeWhale#15`). Adds host `extra_tools`, dynamic `SetDisallowedTools`, file-size enforcement, fail-closed multiline command safety, schema-bound JSON-container repair, provider-role-safe continuations, canonical registry instructions, and alias-aware Custom SubAgent allowlists while reusing upstream `allowed_tools`. The r8 extension layers an exact catalog/final-dispatch policy, trusted-root replacement, latched control-plane denial, hook opt-in, restricted log/audit redaction, and deferral of idle child/Shell continuations until an explicit replacement-authority message, while preserving the legacy `None` path. The parent GAIA integration explicitly requires read-only dispatch, projects the model-visible `File` schema to read/list/search actions, repeats the read-only check before final execution, and projects `Bash` into the hardened read-only Shell context. This generic seam is now published in the Pinvou fork and will be proposed separately from the latest upstream main; Pinvou's GAIA profiles remain app-owned, and the fork copy will be removed if upstream accepts it.
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
- All 34 CodeWhale `forkguard_*` tests pass for the published r8 baseline.
- Parent locked Rust check and desktop binary link pass.
- Parent library tests pass: 1220 passed, 0 failed, and 12 environment-dependent tests ignored.
- Parent fork guard, architecture guard, npm tests, UI lint, desktop UI build, and web UI build pass.
- Full product results are recorded in `docs/codewhale-upgrade-0.9.0-to-0.9.5.md`.

Any fork-distinct change must update this register, guard fingerprints, and a result-oriented behavior test, then pass `./scripts/fork-guard.sh --fast`.
