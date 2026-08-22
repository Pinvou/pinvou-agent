# CodeWhale Fork Modification Register

> Updated: 2026-08-23. Canonical Chinese register: [`docs/fork-modifications.md`](fork-modifications.md).

## Current baseline

| Item | Value |
|---|---|
| Upstream | `v0.9.5` at `853cb707bbcf4f7dc4268fba6d811e0d04083f9c` |
| Public maintenance branch | `Pinvou/CodeWhale:pinvou3-clean` at `d127aed11` (`pinvou-v0.9.5-r8`) |
| Merged fixes | `Pinvou/CodeWhale#9`, `#11`, `#12`, `#13`, and `#15` are merged; the public maintenance branch is pinned at `pinvou-v0.9.5-r8`. The upstream #5461 backport (CodeWhale PR #21, carrying parent PR #216) awaits maintainer merge and the `pinvou-v0.9.5-r9` tag |
| Published status | `pinvou3-clean` and `pinvou-v0.9.5-r8` resolve to `d127aed113529dc93754d044b9f352e9746f6b83`; the parent gitlink resolves to `2645c6c63` (= r8 + the #5461 backport). `verify-public-submodule.sh` validates against `pinvou-v0.9.5-r9` — before that tag is published the script fails by design (the gitlink must not ride a floating branch/PR ref); once r9 ships, tag, branch, and gitlink align on the same commit |
| Previous baseline backup | Tag `pinvou-v0.9.0-r4` and branch `backup/pinvou3-clean-v0.9.0-r4`, both at `03e9e1027c03ce1e4b35ab9e3ccce751b65b9624`; `r1`–`r8` remain immutable |
| Drift | r8 baseline plus the backport: 49 files, `+3303/-476` (net +2827; r8's `#15` itself contributes +1449 and is maintainer-published baseline) |
| Organization | Four current long-lived topics (PR #13 removes the product-specific orchestration topic) plus one upstream-#5461 backport commit |

### Published session fix

- v0.9.5 `load_session` treats an unmatched `tool_use` as evidence of a crashed process. That assumption is invalid when Pinvou persists a live tool call and reads the same session again during the turn.
- The engine fix was merged through `Pinvou/CodeWhale#11`; its public commit is `2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd`.
- T1 now separates side-effect-free `load_session_snapshot` from explicit `recover_session_for_resume`. Pinvou uses snapshots for all runtime read-modify-write paths and performs durable recovery only during app process startup, before any Engine can own a session.
- Revision reconciliation remains fail-closed only for genuine cross-client turns. A local `chat:done` immediately releases the next send, readback failures cannot block ordinary local chat, and cross-client pending notices are deduplicated per session.
- Two CodeWhale tests, two parent `forkguard_*` tests, and Tauri/Web frontend behavior coverage protect side-effect-free runtime reads, observable and idempotent explicit recovery, safe secondary Store opening, durable startup recovery, and consecutive sends after local completion.
- The fix is included in the published head, drift figures, and immutable tag `pinvou-v0.9.5-r5`; CodeWhale required checks and parent automation pass.

### Output-cap fix: upstream #5461 backport (parent PR #216)

> **2026-08-23 (rewritten approach)**: CodeWhale PR #8 (the
> `ApiProvider::Openai` allowlist) was closed by the CodeWhale maintainer, who
> upstreamed the fix with narrower semantics — issue
> [Hmbown/CodeWhale#5460](https://github.com/Hmbown/CodeWhale/issues/5460),
> PR [Hmbown/CodeWhale#5461](https://github.com/Hmbown/CodeWhale/pull/5461)
> (merged 2026-08-17 as `d03260ec`). Upstream semantics: **a route that
> explicitly declares `output_tokens` replaces the conservative 8192 guess for
> an uncatalogued model with that concrete fact**; routes publishing no limit
> stay fail-closed, documented ceilings and the caller's requested cap remain
> authoritative, and a route fact can never raise any cap.
>
> The fork no longer carries the allowlist drift. Instead, the upstream
> `d03260ec` net change to `route_budget.rs` is backported on top of r8
> (CodeWhale PR #21, commit `2645c6c63`; the diff matches upstream line for
> line modulo line offsets; CHANGELOG entries are omitted per fork convention).
> The operator-owned decision stays in the app's `route_limits_for_model`:
> user-custom OpenAI-compatible endpoints (the `OpenAI compatible` preset or
> `provider_kind=custom`) declare an `output_tokens` route fact via the base
> window heuristic (≥500K→65536 / else min(window/2, 65536) / no window
> fact→64000); official endpoints and unregistered cloud models declare
> nothing and stay fail-closed in the base. The guard test
> `forkguard_cloud_models_defer_output_cap_to_base` pins this semantics with
> four assertions.

## Topics

PR #13 was squash-merged as `a36e6cd533024cfe5724bae21875aea42b2ed87a` and published as immutable tag `pinvou-v0.9.5-r7`. It removes product-specific orchestration while preserving canonical registry prompt text and alias-aware Custom SubAgent allowlist resolution.

1. **Host embedding and routing boundary** — `331cb1594688c723d98499d9ca11f05af291b599`, `2eceab4e19cb0b15576c09d5b89e0d8bc42e11fd` (`Pinvou/CodeWhale#11`), and `a36e6cd533024cfe5724bae21875aea42b2ed87a` (`Pinvou/CodeWhale#13`). Exposes only the library modules, narrow root-level Fleet roster API, read-only live-worker projection, opaque resolved-route interfaces, distinct runtime-snapshot versus process-resume session APIs, generic host bulk cancellation, and terminal failure facts required by the host; the full `fleet` module remains private.
2. **Tool compatibility and command-execution safety** — `595adce47e2d1bcf895d7bfd6426c074eb969324`, `3bbf8421ebdb16bff71f83dac4d42c8fb65f0f02` (`Pinvou/CodeWhale#12`), and `a36e6cd533024cfe5724bae21875aea42b2ed87a` (`Pinvou/CodeWhale#13`). Adds host `extra_tools`, dynamic `SetDisallowedTools`, file-size enforcement, fail-closed multiline command safety, schema-bound JSON-container repair, provider-role-safe continuations, canonical registry instructions, and alias-aware Custom SubAgent allowlists while reusing upstream `allowed_tools`.
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
- All 23 CodeWhale `forkguard_*` tests pass for the published r7 baseline.
- Parent locked Rust check and desktop binary link pass.
- Parent library tests pass: 1220 passed, 0 failed, and 12 environment-dependent tests ignored.
- Parent fork guard, architecture guard, npm tests, UI lint, desktop UI build, and web UI build pass.
- Full product results are recorded in `docs/codewhale-upgrade-0.9.0-to-0.9.5.md`.

Any fork-distinct change must update this register, guard fingerprints, and a result-oriented behavior test, then pass `./scripts/fork-guard.sh --fast`.
