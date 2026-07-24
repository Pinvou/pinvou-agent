# CodeWhale Fork Inventory

> This is the public English summary of Pinvou Agent's CodeWhale delta.
> The detailed Chinese inventory and [`fork-policy.md`](fork-policy.md) remain the maintainer source of truth.

## Current baseline

| Item | Value |
|---|---|
| Upstream | `Hmbown/CodeWhale` tag `v0.9.0`, commit `d167c07c96282411956ea7f35ddb8227afa1402f` |
| Public release | `Pinvou/CodeWhale` tag `pinvou-v0.9.0-r1` |
| Pinned commit | `070f4413eeb0e0c4e6f2634f1ada13c60fd2e86e` |
| Maintenance branch | `pinvou3-clean` |
| Organization | 6 long-lived product themes, 3 maintenance/fix commits, and 3 public baseline/security commits |
| Diff from upstream tag | 3,878 insertions, 550 deletions, 57 files |

The delta exceeds the 1,500-line soft limit and has therefore received a mandatory boundary review. The retained code owns behavior that cannot be reconstructed safely in the desktop wrapper: task persistence, workflow completion, prompt-source sealing, and engine-level safety. Shell rendering and platform-specific desktop behavior remain in the parent repository and do not add fork drift.

## Long-lived themes

### T1 — Host library facade

Exposes the upstream bin-first crate as a library for the desktop host. It exports existing engine modules and does not reimplement them.

### T2 — Tool surface and execution safety

Defines the Pinvou tool surface, write-size limits, truncated-argument guidance, wildcard tool restrictions, and fail-closed handling for dangerous commands and required approvals. Result-oriented golden and safety tests protect the behavior.

### T3 — Sealed prompts and one context source

Lets the app own the static prompt composer, accepts only explicitly injected project instructions, loads Skills from the Pinvou runtime bundle, filters disabled Skills, and keeps internal reminders out of the working set.

### T4 — Scheduled execution and history lifecycle

Keeps a stable conversation identity for each automation, persists run links, anchors hourly schedules, skips offline misfires, prevents overlapping runs, and deletes only terminal task history and its artifacts.

### T5 — Host orchestration and workflow completion

Adds host-provided tools and hard allowlists across execution modes, structured subagent outputs, safe declared file persistence, authoritative completion/failure envelopes, bulk cancellation, and cancellable OAuth login.

### T6 — Host routing and shared automation APIs

Exposes an opaque runtime route receipt, explicit route limits, and shared reconciliation APIs so the host can use one consistent routing and automation model without duplicating engine internals.

## Public baseline maintenance

The three commits above the product baseline add:

- a full-history Gitleaks workflow and two exact allowlist entries for public upstream test fixtures;
- `PINVOU_FORK.md` and the README fork notice;
- removal of one internal project-name comment without changing behavior.

They do not introduce a seventh product theme.

## Verification

The released CodeWhale baseline passed:

- full-history Gitleaks across 5,448 commits with zero reported leaks;
- `cargo check --workspace --all-targets --locked`;
- `forkguard_` regression tests;
- formatting, DCO, required gate, and CodeQL checks.

The parent repository additionally verifies that:

- `.gitmodules` uses `https://github.com/Pinvou/CodeWhale.git`;
- no floating submodule branch is configured;
- the gitlink commit is publicly reachable;
- `pinvou-v0.9.0-r1^{}` equals the pinned gitlink.

For any future gitlink or fork-behavior change, update this inventory, the Chinese source-of-truth inventory, fingerprints, and result-oriented tests in the same PR.
