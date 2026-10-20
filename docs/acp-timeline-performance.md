# ACP timeline projection performance

## Behavior and scope

The ACP view owns one latest timeline projector. The event log remains the source of truth, and `projectAcpTimeline` remains the authoritative full projection.

For immutable, strictly ordered, single-session logs whose existing event references remain an exact prefix, only turns targeted by appended events are projected again. Unchanged turn, item and tool objects retain their references. Previously published results are never modified. An update targeting an older turn still rebuilds that turn; completion is not assumed to make it immutable.

Replacement snapshots, truncation, reordered or duplicate events, mixed sessions, unsupported sequence/turn IDs, language changes and error-redactor changes retain full replay. Session changes naturally replace the latest cache. There is no global cache or historical snapshot pool. React render retries/interleavings can cause a full replay but do not change the result.

This reduces repeated derived-object creation. Prefix validation and output-array assembly still scan history; appending is not constant time. A large active turn still replays its own events. Eligibility validation and grouping are deferred to the first append, keeping initial hydration on the original full-projection path. The appender, transport, raw events, permissions, elicitation, terminal events, persistence, model execution and native lane are unchanged. The view is shared by desktop and Web ACP clients.

## Measurement method

Synthetic data only; no model, credentials or user history. The reproducible command, from `pinvou3-app/`, is:

```sh
npm run perf:acp-timeline -- --baseline-ref fcb700f853a32b2900af73451814589af8e9fe79 --output tmp/acp-timeline-final-performance.json
```

The script loads immutable baseline projection/model modules with `git show` and compares them with the actual working-tree projector. Each completed history turn contains seven events, including a tool call with about 1.5KB of output. Each sample appends twenty text chunks targeting a new turn and projects after each append. Event appending/copying/sorting is included. Setup and correctness assertions are excluded. Final complete output is checked against full replay; unchanged historical references are asserted for the candidate.

Each configuration receives five warm-up runs and thirty alternating before/after samples. Update P50/P95 are percentiles of **twenty-update sample means**, not individual update tail latency. Initial full projection is timed separately in each fresh projector; these are warm-process function measurements, not process/disk/Engine cold-start timings. The report retains every sample, source hashes, environment and a source-stability assertion. Run measurements separately from builds and tests. Without `--output`, the generated report is written to `pinvou-agent/acp-timeline-performance.json` under the OS temporary directory, outside the repository. Explicit relative output paths remain relative to the repository root.

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

There is no consistent initial-load improvement: the 200-turn initial P50 is 0.19ms slower and the 1000-turn initial P95 is 0.65ms slower in this run. Initial projection uses the unchanged full function; eligibility/grouping is paid on the first append and included in update sample means. An earlier eager-cache candidate added about 1.5ms to initial P50 at 2000 turns and was replaced by the deferred setup before final measurement. Tiny single-turn differences are not a responsiveness claim. Rounded entries and percentages use the same run; arithmetic uses unrounded samples. For example, the rounded 200-turn values yield 94.3%; its reported 94.5% comes from the unrounded values in the same report.

Complete local samples are in `tmp/acp-timeline-final-performance.json`, excluded from the change. The committed script and fixture reproduce the experiment without that generated file.

## Verification

Reproduce the area check from `pinvou3-app/` (the command runs the 20 explicitly listed files, including legacy file-level invariant scripts):

```sh
node --test tests/acp_attachment_lifecycle.test.mjs tests/acp_error_message.test.mjs tests/acp_native_attachment_client.test.mjs tests/acp_platform_client.test.mjs tests/acp_session_operation_race.test.mjs tests/acp_timeline_projector.test.mjs tests/chat_turn_error_isolation.test.mjs tests/code_native_lane.test.mjs tests/codex_acp_platform_contract.test.js tests/codex_acp_runtime_notice.test.mjs tests/codex_acp_timeline.test.mjs tests/codex_acp_windows_contract.test.js tests/codex_checkpoints_logic.test.mjs tests/codex_workspace_tree_logic.test.mjs tests/conversation_code_block_copy.test.mjs tests/conversation_scroll_follow.test.mjs tests/conversation_virtualization_logic.test.mjs tests/deepseek_conversation_timeline.test.mjs tests/foundation_event_projection.test.mjs tests/model_reasoning_render.test.mjs
```

- Red/green tests establish unchanged-turn reuse and whole-output reuse. A separate failing regression caught prototype-inherited turn IDs; these now use the authoritative global-event fallback.
- All 19 projector tests pass, including every semantic event, late historical updates, immutable inputs/outputs, gaps, snapshots, duplicates, out-of-order events, session changes, interleaved render branches, redaction changes and retry after a projection error. Redactors are assumed to be pure functions of `(text, language)`; replacing the helper or function invalidates the cache, but hidden state changes inside an otherwise identical function cannot be detected. A deterministic 300-event interleaving equals full replay after every append.
- ACP/conversation area tests: 44/44 pass (the original 42 plus two review regressions). The new sequence-value and non-array recovery tests each fail when their corresponding guard is removed. The exact command is listed above.
- Real frontend browser smoke with mocked Tauri services: 54/54 pass, including ACP streaming/tool cards and completion.
- Desktop and Web UI builds, Safari 14 compatibility audit, architecture guard, changed Node-file lint and `lint:ui`, `lint:ox`, `lint:biome`, `lint:knip` and `lint:cycles` pass. Existing unrelated warnings remain.
- Independent review approved the implementation; this follow-up adds malformed-input coverage and documentation without changing the projector or app behavior. Changed-file lint and the 44 area tests were rerun for the follow-up; builds and browser smoke describe the original implementation validation. Full repository tests and native device latency/resource measurements were not rerun for this change; no claim of a green full suite or end-to-end performance gain is made.

These measurements concern local JavaScript append/projection processing. They do not measure React rendering, WebView2 input latency, disk loading, provider throughput, task completion time, total process memory or durable persistence. Single-turn histories and first-load overhead must be assessed separately from long-history streaming gains. The earlier native background-stream optimization is in a separate branch/PR.
