# CodeWhale Fork Maintenance Policy

> Last updated: 2026-07-30
> Released baseline: `pinvou-v0.9.0-r2@cb93e0f4`
> Detailed inventory: [`fork-modifications.en.md`](fork-modifications.en.md)

## 1. Baseline and ownership

- Upstream: [`Hmbown/CodeWhale`](https://github.com/Hmbown/CodeWhale), tag `v0.9.0`, commit `d167c07c96282411956ea7f35ddb8227afa1402f`.
- Public fork: [`Pinvou/CodeWhale`](https://github.com/Pinvou/CodeWhale).
- Maintenance branch: `pinvou3-clean`, currently at `cb93e0f4466d60e306252ed08bbbe214f2def752`.
- Pinvou Agent pins the immutable tag `pinvou-v0.9.0-r2`, which dereferences to that commit.
- `.gitmodules` intentionally has no `branch` entry. A parent-repository checkout must never move merely because the maintenance branch moved.

The fork is maintained as six long-lived themes:

1. host library facade;
2. tool surface, file-write limits, and execution safety;
3. sealed prompts and a single context/Skill source;
4. scheduled execution and task-history lifecycle;
5. host orchestration, workflow completion gates, and cancellable login;
6. host routing, budgets, and shared automation APIs.

The exact files, commits, rationale, drift, and tests are recorded in [`fork-modifications.en.md`](fork-modifications.en.md).

## 2. Where a change belongs

Use the narrowest layer that can own the behavior:

| Need | Location |
|---|---|
| Desktop UI, Tauri bridge, or runtime configuration | `pinvou3-app/` |
| Model guidance or a domain workflow | bundle instructions or `SKILL.md` |
| Independent external integration | MCP server or connector |
| Reusable CodeWhale bug fix or API | a clean branch from the latest upstream `main`, then an upstream PR |
| Pinvou-specific behavior that must be atomic with engine, subagent, or task lifecycle | the nearest existing fork theme |

Create a new fork theme only when the change has a genuinely independent state, verification, and rollback boundary.

## 3. Same-PR requirements

A parent-repository PR that changes fork-specific behavior or the CodeWhale gitlink must also update:

1. `docs/fork-modifications.md` and its English counterpart when public guidance changes;
2. the relevant fixed-string fingerprint in `scripts/fork-guard.sh`;
3. at least one result-oriented `forkguard_*` test, unless the PR documents a platform-only substitute;
4. any intentionally invalidated upstream test with an explicit `#[ignore = "pinvou3 fork(...)"]`.

Before opening the PR, run:

```bash
./scripts/verify-public-submodule.sh
./scripts/fork-guard.sh --fast
```

Documentation, fingerprints, tests, and the patch must travel in the same PR.

## 4. Upstream synchronization

Before syncing:

```bash
git -C CodeWhale fetch upstream --tags
git -C CodeWhale branch backup/pre-vX-sync <current-fork-head>
git -C CodeWhale diff --shortstat <current-release-tag>..<current-fork-head>
./scripts/fork-guard.sh --fast
```

Prefer a clean re-fork from an upstream release tag when crossing a major version, when core engine/prompt/automation code was reorganized, when conflict volume is high, or when old drift has already exceeded the soft limit.

Classify every old patch as one of:

- already provided upstream;
- movable to the app, a Skill, or an MCP server;
- still required in one of the six fork themes.

After the sync, run the fork guard, CodeWhale library and `forkguard_` tests, parent app checks, and a before/after static system-prompt diff. The full commands remain in the Chinese policy, which is the maintainer source of truth.

## 5. Release and integrity rules

- Preserve upstream MIT licensing and author attribution.
- Create a new immutable Pinvou tag for every reviewed public baseline; never move or reuse a released tag.
- The tag target, public maintenance branch, and parent gitlink must identify the same reachable commit at release time.
- CI verifies the public URL, tag target, gitlink reachability, and absence of a floating `.gitmodules` branch.
- Never force-push a shared release baseline without explicit authorization.
