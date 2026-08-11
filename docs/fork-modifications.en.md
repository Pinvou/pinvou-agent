# CodeWhale Fork Inventory

> This is the public English summary of Pinvou Agent's CodeWhale delta.
> The detailed Chinese inventory and [`fork-policy.md`](fork-policy.md) remain the maintainer source of truth.

## Current baseline

| Item | Value |
|---|---|
| Upstream | `Hmbown/CodeWhale` tag `v0.9.0`, commit `d167c07c96282411956ea7f35ddb8227afa1402f` |
| Public release | `Pinvou/CodeWhale` tag `pinvou-v0.9.0-r4` |
| Pinned commit | `03e9e1027c03ce1e4b35ab9e3ccce751b65b9624` |
| Maintenance branch | `pinvou3-clean` |
| Organization | 6 long-lived product themes, 11 follow-up behavior/maintenance commits, and 3 public baseline/security commits |
| Diff from upstream tag | 4,839 insertions, 616 deletions, 58 files |

The delta exceeds the 1,500-line soft limit and has therefore received a mandatory boundary review. The retained code owns behavior that cannot be reconstructed safely in the desktop wrapper: task persistence, workflow completion, prompt-source sealing, and engine-level safety. Shell rendering and platform-specific desktop behavior remain in the parent repository and do not add fork drift.

## Long-lived themes

### T1 — Host library facade

Exposes the upstream bin-first crate as a library for the desktop host. It exports existing engine modules and does not reimplement them.

### T2 — Tool surface and execution safety

Defines the Pinvou tool surface, write-size limits, truncated-argument guidance, wildcard tool restrictions, and fail-closed handling for dangerous commands and required approvals. Session-scoped `hidden_tools` may release entries from the fixed Pinvou hidden list but cannot hide tools outside it; the `tool_search` gate remains fixed. `append_file` emits a bounded inline unified diff with byte-summary, oversized-file, and non-UTF-8 fallbacks. Result-oriented golden and safety tests protect the behavior.

### T3 — Sealed prompts and one context source

Lets the app own the static prompt composer, accepts only explicitly injected project instructions, and discovers Skills only from the explicit `EngineConfig.skills_dir` root. The current app injects the Pinvou runtime bundle and filters disabled Skills; CodeWhale no longer adds an implicit bundle fallback. Internal reminders stay out of the working set.

### T4 — Scheduled execution and history lifecycle

Keeps a stable conversation identity for each automation, persists run links, anchors hourly schedules, skips offline misfires, prevents overlapping runs, and deletes only terminal task history and its artifacts.

### T5 — Host orchestration and workflow completion

Adds host-provided tools and hard allowlists across execution modes, structured subagent outputs, safe declared file persistence, authoritative completion/failure envelopes, reliable nested-agent lineage and exactly-once terminal mailbox delivery, bulk cancellation, and cancellable OAuth login. `ChildSpawned` is published when ownership is accepted, while the single `Started` event remains tied to actual execution after the launch gate.

### T6 — Host routing and shared automation APIs

Exposes an opaque runtime route receipt, explicit route limits, and shared reconciliation APIs so the host can use one consistent routing and automation model without duplicating engine internals.

## Public baseline maintenance

The three public baseline/security commits add:

- a full-history Gitleaks workflow and two exact allowlist entries for public upstream test fixtures;
- `PINVOU_FORK.md` and the README fork notice;
- removal of one internal project-name comment without changing behavior.

They do not introduce a seventh product theme. The `cb93e0f44` follow-up extends T2 with `append_file` inline diffs; the four commits ending at `9a31dcdfa` extend T5 with reliable Agent mailbox lifecycle handling. The three commits ending at `03e9e1027` update the existing T2/T3 boundaries with bounded session tool visibility and a single explicit Skill root.

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
- `pinvou-v0.9.0-r4^{}` equals the pinned gitlink.

For any future gitlink or fork-behavior change, update this inventory, the Chinese source-of-truth inventory, fingerprints, and result-oriented tests in the same PR.
