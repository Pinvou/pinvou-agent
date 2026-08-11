# CodeWhale Fork Modification Register

> Updated: 2026-08-11. Canonical Chinese register: [`docs/fork-modifications.md`](fork-modifications.md).

## Current baseline

| Item | Value |
|---|---|
| Upstream | `v0.9.5` at `853cb707bbcf4f7dc4268fba6d811e0d04083f9c` |
| Public maintenance branch | `Pinvou/CodeWhale:pinvou3-clean` at `d1010aa3bbaf76780e29df4434fd1e03a95b2ca6` |
| Dependency fix | `Pinvou/CodeWhale#9` is merged; the resulting maintenance head is `d1010aa3bbaf76780e29df4434fd1e03a95b2ca6` |
| Public status | `pinvou3-clean` and immutable tag `pinvou-v0.9.5-r4` both resolve to the public maintenance head; `r1`, `r2`, and `r3` remain immutable historical tags |
| Previous baseline backup | Tag `pinvou-v0.9.0-r4` and branch `backup/pinvou3-clean-v0.9.0-r4`, both at `03e9e1027c03ce1e4b35ab9e3ccce751b65b9624` |
| Drift | 45 files, `+1991/-265` |
| Organization | Five long-lived topics in six linear commits replayed from `v0.9.5` |

## Topics

1. **Host embedding and routing boundary** — `331cb1594688c723d98499d9ca11f05af291b599`. Exposes only the library modules, narrow root-level Fleet roster API, read-only live-worker projection, and opaque resolved-route interfaces required by the host; the full `fleet` module remains private and recovery semantics stay unchanged.
2. **Tool compatibility and command-execution safety** — `595adce47e2d1bcf895d7bfd6426c074eb969324`. Adds host `extra_tools`, dynamic `SetDisallowedTools`, file-size enforcement, and fail-closed multiline command safety while reusing upstream `allowed_tools`.
3. **Embedded context and Skill sources** — `5a9f52941b83452c1e8b76c2d679bac315edcf70`. Seals ambient project authority, scans only the explicit Skill root, filters disabled Skills, preserves up to 100 KiB only for the Permissions fragment, and excludes internal reminders from Working Set extraction.
4. **Automation and runtime lifecycle** — `fc84f7d3e5dca0e3db404d43e218597764129f9b`. Preserves stable conversation/thread identity, v4 task compatibility, anchored schedules, no-backfill/no-overlap behavior, and terminal-only cleanup.
5. **Three Departments and Six Ministries orchestration, completion gate, and structured-output safety** — `3782a78d4e11d1fb65042cf9c82231b9d644c20a` plus `d1010aa3bbaf76780e29df4434fd1e03a95b2ca6`. Adds the role/tool/step/output contract, bounded write claims, explicit host-selected output roots, traversal and symlink-escape rejection, safe structured persistence, file-completion gate, cancellation, and authoritative terminal result needed by that workflow.

Pinvou's product tool allowlist, connector state, UI, workspace selection, bundle instructions, session Skill materialization, and presentation remain in `pinvou3-app`.

## v0.9.5 migration notes

- The parent passes through the new `EngineConfig.subagent_state_root` field.
- The removed legacy `hidden_tools` field is not restored; session-level hiding already uses dynamic `disallowed_tools` shaping.
- The upstream 40 KiB WorldState cap is retained globally. Only `FragmentId::Permissions` uses the existing 100 KiB instruction limit.
- The parent lockfile reflects the v0.9.5 workspace-crate split without adding a new direct Pinvou dependency.

## Verification

- CodeWhale format and locked library check pass.
- All 21 CodeWhale `forkguard_*` tests pass.
- Parent locked Rust check and desktop binary link pass.
- Parent library tests pass: 1077 passed, 0 failed, and 12 environment-dependent tests ignored.
- Parent fork guard, architecture guard, npm tests, UI lint, desktop UI build, and web UI build pass.
- Full product results are recorded in `docs/codewhale-upgrade-0.9.0-to-0.9.5.md`.

Any fork-distinct change must update this register, guard fingerprints, and a result-oriented behavior test, then pass `./scripts/fork-guard.sh --fast`.
