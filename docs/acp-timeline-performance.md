# ACP timeline projection performance

## Behavior and scope

The ACP view owns one latest timeline projector. The event log remains the source of truth, and `projectAcpTimeline` remains the authoritative full projection.

For immutable, strictly ordered, single-session logs whose existing event references remain an exact prefix, only turns targeted by appended events are projected again. Unchanged turn, item and tool objects retain their references. Previously published results are never modified. An update targeting an older turn still rebuilds that turn; completion is not assumed to make it immutable.

Replacement snapshots, truncation, reordered or duplicate events, mixed sessions, unsupported sequence/turn IDs, language changes and error-redactor changes retain full replay. Session changes naturally replace the latest cache. There is no global cache or historical snapshot pool. React render retries/interleavings can cause a full replay but do not change the result.

This reduces repeated derived-object creation. Prefix validation and output-array assembly still scan history; appending is not constant time. A large active turn still replays its own events. Eligibility validation and grouping are deferred to the first append, keeping initial hydration on the original full-projection path. The appender, transport, raw events, permissions, elicitation, terminal events, persistence, model execution and native lane are unchanged. The view is shared by desktop and Web ACP clients.

## Measurement method

Synthetic data only; no model, credentials or user history. The reproducible command, from `pinvou3-app/`, is:

```powershell
npm run perf:acp-timeline -- --baseline-ref fcb700f853a32b2900af73451814589af8e9fe79 --output tmp/acp-timeline-final-performance.json
```

The script loads immutable baseline projection/model modules with `git show` and compares them with the actual working-tree projector. Each completed history turn contains seven events, including a tool call with about 1.5KB of output. Each sample appends twenty text chunks targeting a new turn and projects after each append. Event appending/copying/sorting is included. Setup and correctness assertions are excluded. Final complete output is checked against full replay; unchanged historical references are asserted for the candidate.

Each configuration receives five warm-up runs and thirty alternating before/after samples. Update P50/P95 are percentiles of **twenty-update sample means**, not individual update tail latency. Initial full projection is timed separately in each fresh projector; these are warm-process function measurements, not process/disk/Engine cold-start timings. The report retains every sample, source hashes, environment and a source-stability assertion. Run measurements separately from builds and tests.

## Before/after results

Measured on 2026-10-19, Windows, Intel Core i7-8700 @ 3.20GHz, Node v24.13.0. Baseline: `fcb700f853a32b2900af73451814589af8e9fe79`. Values are local milliseconds per update, averaged within each twenty-update sample.

| Completed history turns | Update P50 before | P50 after | P50 saved | P50 reduction | Update P95 before | P95 after |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 0.012ms | 0.007ms | 0.005ms | no meaningful user-facing claim | 0.021ms | 0.015ms |
| 200 | 1.40ms | 0.08ms | 1.32ms | 94.5% | 2.22ms | 0.14ms |
| 1000 | 8.05ms | 0.38ms | 7.67ms | 95.3% | 9.41ms | 0.52ms |
| 2000 | 18.18ms | 1.08ms | 17.10ms | 94.1% | 23.40ms | 1.74ms |

Each history has seven events per completed turn (14000 events at 2000 turns). Once the appended turn exists, all completed history turns retain their references rather than being rebuilt on each chunk. This is reference/allocation-work evidence, not a measured process-memory reduction.

| Completed history turns | Initial projection P50 before → after | Initial projection P95 before → after |
| --- | --- | --- |
| 1 | 0.010ms → 0.014ms | 0.024ms → 0.044ms |
| 200 | 1.30ms → 1.49ms | 2.33ms → 2.18ms |
| 1000 | 7.61ms → 7.35ms | 10.21ms → 10.86ms |
| 2000 | 16.51ms → 16.50ms | 24.88ms → 24.54ms |

There is no consistent initial-load improvement: the 200-turn initial P50 is 0.19ms slower and the 1000-turn initial P95 is 0.65ms slower in this run. Initial projection uses the unchanged full function; eligibility/grouping is paid on the first append and included in update sample means. An earlier eager-cache candidate added about 1.5ms to initial P50 at 2000 turns and was replaced by the deferred setup before final measurement. Tiny single-turn differences are not a responsiveness claim. Rounded entries and percentages use the same run; arithmetic uses unrounded samples.

Complete local samples are in `tmp/acp-timeline-final-performance.json`, excluded from the change. The committed script and fixture reproduce the experiment without that generated file.

## Verification

- Red/green tests establish unchanged-turn reuse and whole-output reuse. A separate failing regression caught prototype-inherited turn IDs; these now use the authoritative global-event fallback.
- All 17 projector tests pass, including every semantic event, late historical updates, immutable inputs/outputs, gaps, snapshots, duplicates, out-of-order events, session changes, interleaved render branches, redaction changes and retry after a projection error. A deterministic 300-event interleaving equals full replay after every append.
- ACP/conversation area tests: 42/42 pass.
- Real frontend browser smoke with mocked Tauri services: 54/54 pass, including ACP streaming/tool cards and completion.
- Desktop and Web UI builds, Safari 14 compatibility audit, architecture guard, changed Node-file lint and all five frontend lint commands pass. Existing unrelated warnings remain.
- Independent review found no remaining blocker after the prototype-ID fix. Full repository tests and native device latency/resource measurements were not rerun for this change; no claim of a green full suite or end-to-end performance gain is made.

These measurements concern local JavaScript append/projection processing. They do not measure React rendering, WebView2 input latency, disk loading, provider throughput, task completion time, total process memory or durable persistence. Single-turn histories and first-load overhead must be assessed separately from long-history streaming gains. The earlier native background-stream optimization is in a separate branch/PR.
