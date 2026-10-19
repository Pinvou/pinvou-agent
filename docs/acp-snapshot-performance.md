# ACP snapshot reconciliation performance

## Behavior

`mergeAcpTimelineSnapshot` reconciles an authoritative ACP snapshot with local events retained during loading, sequence-gap recovery and remote reconnection. Previously it replayed every local event through `appendAcpEvent`: each event searched the growing snapshot for a duplicate, and every new event copied and sorted the full array.

For serialized envelopes with own data properties, string session IDs and nonnegative safe-integer sequence numbers, reconciliation now builds one sequence index, collects unseen local events and sorts once when needed. Duplicate-only overlap is linear; adding events still requires one final sort. The index exists only for the synchronous merge call, with no persistent cache.

The original contract remains:

- Only local events matching the selected session participate. Snapshot events from other sessions remain present.
- Snapshot entries win conflicts. Existing duplicate snapshot entries are retained. The first novel local occurrence wins later local duplicates.
- Duplicate matching remains strict on session and sequence identity; no string/number normalization is introduced.
- A no-op returns the original snapshot with its original order. A merge with new events preserves the existing stable numeric sequence ordering.
- Arrays, envelopes and payloads are not mutated, and selected event object references are retained.
- Irregular sequences, sparse/invalid snapshot entries, accessor/inherited keys and failed descriptor inspection use the original replay path. Existing replay errors remain visible.
- An empty local working set returns immediately without indexing the snapshot.

Transport, journal reads, permissions, elicitation, terminal-event handling, session-load/gap guards, model execution and CodeWhale are unchanged. This App helper is shared by desktop and Web ACP views. Web loads pages of 128 events but accumulates the full history before reconciliation, so pagination does not cap the final merge at 128 events.

## Reproduction and method

From `pinvou3-app/`, with locked development dependencies installed:

```powershell
npm run perf:acp-snapshot -- --baseline-ref fcb700f853a32b2900af73451814589af8e9fe79 --output tmp/acp-snapshot-final-performance.json
```

The script loads the immutable baseline state/model modules with `git show` and compares the baseline merge with the actual working-tree merge. Each completed synthetic turn has seven events, including a tool call/result with about 1.5KB of output. Snapshot objects are separately cloned to model transport delivery. Scenarios cover empty current history, full overlap, overlap plus twenty newly arrived events, and only twenty local tail events.

Each configuration uses five warm-ups and thirty alternating before/after samples. Each sample times **one merge**; P95 uses nearest rank over those samples. Setup, assertions and downstream projection are outside timing. Complete values, selected event references and no-op snapshot identity are checked; downstream projections are compared per scenario. The report retains source/script/fixture hashes, environment and all samples, and rejects source changes during measurement. Run it separately from builds and tests. `--samples` accepts 5 through 200 (default 30).

## Before/after results

Measured on 2026-10-19 on Windows, Intel Core i7-8700 @ 3.20GHz, Node v24.13.0. Baseline: `fcb700f853a32b2900af73451814589af8e9fe79`. This table uses full local overlap plus twenty new events received after the snapshot boundary.

| Completed history turns | Snapshot events | Merge P50 before | P50 after | P50 saved | P50 reduction | Merge P95 before | P95 after |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 200 | 1400 | 8.39ms | 0.32ms | 8.08ms | 96.2% | 16.44ms | 0.52ms |
| 1000 | 7000 | 301.02ms | 2.07ms | 298.95ms | 99.3% | 457.59ms | 4.37ms |
| 2000 | 14000 | 1306.94ms | 4.57ms | 1302.36ms | 99.7% | 1503.01ms | 6.28ms |

Rounded entries are from the same run; saved times and percentages use unrounded values. Absolute times vary with workload, JIT, GC and machine load; this is not a device-independent SLA.

Other scenarios confirm the scope and avoid claiming a universal first-open gain:

| Completed history turns | Full overlap, no additions: P50 before → after | Only twenty local tail events: P50 before → after |
| --- | --- | --- |
| 200 | 7.27ms → 0.29ms | 0.76ms → 0.16ms |
| 1000 | 190.22ms → 1.38ms | 3.68ms → 0.83ms |
| 2000 | 824.06ms → 2.91ms | 19.29ms → 3.06ms |

Empty-current reconciliation remains below 0.01ms in both variants at every size and returns the snapshot directly. This does not establish faster empty-session creation or cold history loading. Complete raw samples are in `tmp/acp-snapshot-final-performance.json`, excluded from the change; the committed script and fixture reproduce the experiment.

## Verification and limits

- Before implementation, 16 semantic tests passed while two deterministic work budgets failed: 140 overlapping events caused 9870 snapshot element reads, and twenty additions caused twenty complete sorts.
- The final snapshot suite passes 20/20 tests, including linear-read and one-sort budgets, conflict precedence, original duplicates, no-op identity/order, stable ordering, other sessions, frozen inputs, live tails, permission/elicitation/tool/terminal events, irregular sequences, sparse inputs, accessor read behavior, descriptor failures and 300 deterministic differential cases against legacy replay.
- ACP/conversation area tests pass 45/45.
- Real frontend browser smoke with mocked Tauri services passes 55/55. The new recovery case creates a live sequence gap, holds the snapshot request pending, delivers another live event, then resolves a partial authoritative snapshot. It verifies missing history, snapshot-wins conflicts, one copy of the new live tail, prior history and the prior completed tool summary.
- Desktop and Web UI builds, Safari 14 compatibility audit, architecture guard, changed-file ESLint and all five frontend lint commands pass. Existing unrelated warnings remain.
- Independent review found no remaining blockers after reviewing merge/fallback semantics, real load/gap guards, the benchmark and the browser recovery case.

These are synthetic local merge-function measurements, not network/disk time, projection/React rendering, first visible paint, native WebView2 latency, model throughput, task completion time, process CPU utilization or RSS. Full repository tests and native device resource/latency measurements were not rerun for this change. Recovery trigger frequency in real sessions is unmeasured. Native work-mode chat and empty/new sessions do not receive the demonstrated overlapping-history benefit.

No UI copy, configuration format, runtime dependency, network endpoint, permission policy, Rust or CodeWhale behavior changes. Background-preview and incremental-projection improvements are independent PRs; their percentages must not be added to these merge-function percentages.
