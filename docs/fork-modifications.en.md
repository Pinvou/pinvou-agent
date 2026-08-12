# CodeWhale Fork Modification Register

> Updated: 2026-08-13. Canonical Chinese register: [`docs/fork-modifications.md`](fork-modifications.md).

## Current baseline

| Item | Value |
|---|---|
| Upstream | `v0.9.5` at `853cb707bbcf4f7dc4268fba6d811e0d04083f9c` |
| Public maintenance branch | `Pinvou/CodeWhale:pinvou3-clean` at `3bbf8421ebdb16bff71f83dac4d42c8fb65f0f02` |
| Merged fixes | `Pinvou/CodeWhale#9`, `Pinvou/CodeWhale#11`, and `Pinvou/CodeWhale#12` are merged |
| Public status | `pinvou3-clean` and immutable tag `pinvou-v0.9.5-r6` both resolve to the public maintenance head; `r1` through `r5` remain immutable historical tags |
| Previous baseline backup | Tag `pinvou-v0.9.0-r4` and branch `backup/pinvou3-clean-v0.9.0-r4`, both at `03e9e1027c03ce1e4b35ab9e3ccce751b65b9624` |
| Drift | r6 public baseline: 52 files, `+2640/-299` |
| Organization | Five long-lived topics and eight published linear commits replayed from `v0.9.5` |

### r6 local-model tool-call compatibility fix (verified and published)

- Some OpenAI-compatible backends return tool parameters that are structurally valid but re-encode schema-declared nested object/array values as JSON strings; this makes strongly typed tools such as `request_user_input` fail before reaching business validation.
- T2 repairs such a container only when the schema explicitly declares `object`/`array`, the string is strict JSON no larger than 64 KiB, and the decoded type matches; plain text and numeric/boolean strings are not loosely converted, and the original tool validation still runs afterward.
- When repeated tool calls trigger `stuck_guard`, or consecutive tool errors trigger a degradation hint, the internal policy notice is folded into the corresponding `tool_result` instead of appending a standalone runtime `user` message for these two paths, so strict OpenAI-compatible backends no longer reject the next turn over the role sequence.
- The Tauri/Web bridge strips only the `stuck_guard` / `tool_error_degradation` known internal suffixes from the tool-card presentation projection; persisted messages and the model-facing context stay unchanged.
- This change intentionally does not cover real-user `pending_steers`, loop-entry steers, LSP diagnostics, or subagent handoff, among other runtime injection paths; these paths involve user authority and context semantics, so this PR introduces neither a global role normalizer nor synthetic assistant messages, and they will be designed separately later.
- The base fix was squash-merged through `Pinvou/CodeWhale#12`; its public commit is `3bbf8421ebdb16bff71f83dac4d42c8fb65f0f02`, published by the immutable tag `pinvou-v0.9.5-r6`.
- The generic schema-parameter repair merged upstream through `Hmbown/CodeWhale#5348`; the latest upstream has removed `stuck_guard` and this fork's consecutive-error degradation path, so the role-continuation compatibility remains scoped to the current v0.9.5 fork lifecycle.

### Published session fix

- v0.9.5 `load_session` treats an unmatched `tool_use` as evidence of a crashed process. That assumption is invalid when Pinvou persists a live tool call and reads the same session again during the turn.
- The engine fix was merged through `Pinvou/CodeWhale#11`; its public commit is `2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd`.
- T1 now separates side-effect-free `load_session_snapshot` from explicit `recover_session_for_resume`. Pinvou uses snapshots for all runtime read-modify-write paths and performs durable recovery only during app process startup, before any Engine can own a session.
- Revision reconciliation remains fail-closed only for genuine cross-client turns. A local `chat:done` immediately releases the next send, readback failures cannot block ordinary local chat, and cross-client pending notices are deduplicated per session.
- Two CodeWhale tests, two parent `forkguard_*` tests, and Tauri/Web frontend behavior coverage protect side-effect-free runtime reads, observable and idempotent explicit recovery, safe secondary Store opening, durable startup recovery, and consecutive sends after local completion.
- The fix is included in the published head, drift figures, and immutable tag `pinvou-v0.9.5-r5`; CodeWhale required checks and parent automation pass.

## Topics

1. **Host embedding and routing boundary** — `331cb1594688c723d98499d9ca11f05af291b599` plus `2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd` (`Pinvou/CodeWhale#11`). Exposes only the library modules, narrow root-level Fleet roster API, read-only live-worker projection, opaque resolved-route interfaces, and distinct runtime-snapshot versus process-resume session APIs required by the host; the full `fleet` module remains private.
2. **Tool compatibility and command-execution safety** — `595adce47e2d1bcf895d7bfd6426c074eb969324` plus `3bbf8421ebdb16bff71f83dac4d42c8fb65f0f02` (`Pinvou/CodeWhale#12`). It adds host `extra_tools`, dynamic `SetDisallowedTools`, file-size enforcement, fail-closed multiline command safety, schema-bound repair of stringified JSON containers, and keeps stuck-guard plus repeated-tool-error degradation guidance inside the corresponding tool result while reusing upstream `allowed_tools`. Primitive strings remain untouched and tool-specific validation still applies. Tool cards remove only these two known internal suffixes at presentation time while durable/model context remains intact. This PR intentionally does not cover real-user pending steers or other runtime injection paths; those require a separate role and authority design. The generic schema repair merged upstream through `Hmbown/CodeWhale#5348`; current upstream has removed `stuck_guard` and the fork's degradation path, so that role-sequence compatibility remains scoped to this v0.9.5 fork lifecycle.
3. **Embedded context and Skill sources** — `5a9f52941b83452c1e8b76c2d679bac315edcf70`. Seals ambient project authority, scans only the explicit Skill root, filters disabled Skills, preserves up to 100 KiB only for the Permissions fragment, and excludes internal reminders from Working Set extraction.
4. **Automation and runtime lifecycle** — `fc84f7d3e5dca0e3db404d43e218597764129f9b`. Preserves stable conversation/thread identity, v4 task compatibility, anchored schedules, no-backfill/no-overlap behavior, and terminal-only cleanup.
5. **Three Departments and Six Ministries orchestration, completion gate, and structured-output safety** — `3782a78d4e11d1fb65042cf9c82231b9d644c20a` plus `d1010aa3bbaf76780e29df4434fd1e03a95b2ca6`. Adds the role/tool/step/output contract, bounded write claims, explicit host-selected output roots, traversal and symlink-escape rejection, safe structured persistence, file-completion gate, cancellation, and authoritative terminal result needed by that workflow.

Pinvou's product tool allowlist, connector state, UI, workspace selection, bundle instructions, session Skill materialization, and presentation remain in `pinvou3-app`.

## v0.9.5 migration notes

- The parent sets the new `EngineConfig.subagent_state_root` from `SessionRoots`: execution stays in the task workspace while delegated-agent state uses the session ledger.
- The removed legacy `hidden_tools` field is not restored; session-level hiding already uses dynamic `disallowed_tools` shaping.
- The upstream 40 KiB WorldState cap is retained globally. Only `FragmentId::Permissions` uses the existing 100 KiB instruction limit.
- The parent lockfile reflects the v0.9.5 workspace-crate split without adding a new direct Pinvou dependency.

## Verification

- The r6 baseline passes CodeWhale format/workspace checks, all 29 CodeWhale `forkguard_*` tests, all 20 parent `forkguard_*` tests, all 122 Node tests, pet asset validation, UI lint/build, and the architecture guard.
- The remaining parent locked Rust build matrix below describes the published r5 baseline and has not been fully rerun for r6.
- Parent locked Rust check and desktop binary link pass.
- Parent library tests pass: 1077 passed, 0 failed, and 12 environment-dependent tests ignored.
- Parent fork guard, architecture guard, npm tests, UI lint, desktop UI build, and web UI build pass.
- Full product results are recorded in `docs/codewhale-upgrade-0.9.0-to-0.9.5.md`.

Any fork-distinct change must update this register, guard fingerprints, and a result-oriented behavior test, then pass `./scripts/fork-guard.sh --fast`.
