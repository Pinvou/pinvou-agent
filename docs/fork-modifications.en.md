# CodeWhale Fork Modification Register

> Updated: 2026-08-21. Canonical Chinese register: [`docs/fork-modifications.md`](fork-modifications.md).

## Current baseline

| Item | Value |
|---|---|
| Upstream | `v0.9.5` at `853cb707bbcf4f7dc4268fba6d811e0d04083f9c` |
| Public maintenance branch | `Pinvou/CodeWhale:pinvou3-clean` at `d127aed11` (r7 plus #15); the r8 casing fix is pending through `Pinvou/CodeWhale#18` |
| Merged fixes | `Pinvou/CodeWhale#9`, `#11`, `#12`, `#13`, and `#15` are merged |
| Published status | `pinvou-v0.9.5-r7` and the r7 parent gitlink resolve to `a36e6cd533024cfe5724bae21875aea42b2ed87a`; `pinvou3-clean` has advanced to `d127aed11`; `r1` through `r7` remain immutable historical tags |
| Previous baseline backup | Tag `pinvou-v0.9.0-r4` and branch `backup/pinvou3-clean-v0.9.0-r4`, both at `03e9e1027c03ce1e4b35ab9e3ccce751b65b9624` |
| Drift | Published r7 baseline: 46 files, `+1852/-269`; the r8 fix adds 2 files, `+95` |
| Organization | Four current long-lived topics; PR #13 removes the product-specific orchestration topic |

### r8 strict-direct model-casing route fix (in flight)

- Selecting `glm-5.2` on the GLM Coding Plan global endpoint in the 0.8.1 stable build failed `send_user_message` with `model "glm-5.2" is not served by direct provider zai`: the zai catalog row uses the marketing casing `GLM-5.2`, so the app's lowercase saved selector missed the owning row under exact comparison and then collided with the bare modelstudio wire id `glm-5.2` in the foreign-selector check; a custom `glm-5.3` does not collide and passed through.
- The original fix in `Pinvou/CodeWhale#14` generalized by provider class; the fork maintainer converged it with review feedback and landed an equivalent implementation upstream through `Hmbown/CodeWhale#5475` (commit `c0f749731`, co-authored with the original author). r8 cherry-picks that upstream commit verbatim onto `pinvou3-clean` (#18 is restacked onto the current maintenance head `125e3fdcd`, which includes #15), keeping zero drift from upstream.
- Upstream semantics: the case-fold fallback applies only to official Deepseek/Zai strict-direct endpoints, and only after exact matching is exhausted accepts a unique provider-owned fold hit; case ambiguity falls back to unknown-model pass-through instead of borrowing metadata by catalog order; custom endpoints keep passing the original model string through; a hit goes on the wire with the row's documented casing and catalog limits, and the fallback runs before the foreign-selector check.
- Audit of analogous paths: the `opencode_go` allowlist, the zai/deepseek/minimax/mimo alias tables, and tui `validate_route` all normalize case already.
- Locked by CodeWhale `resolver_direct_owned_row_match_survives_casing_mismatch` and `resolver_direct_casefold_match_requires_one_owned_row` (upstream-named regressions, no longer under the `forkguard_` prefix); the parent-side bridge regression ships in PR #295.
- Verified: full `codewhale-config` 544 passing; fmt and clippy clean.

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
