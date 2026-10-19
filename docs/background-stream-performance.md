# Native background stream preview performance

## Behavior

The native Tauri chat adapter keeps authoritative messages, raw deltas, tool execution, model requests and persistence unchanged. Only presentation work is scheduled differently:

- Foreground Markdown previews retain their 180ms trailing budget; the first visible bubble is still immediate.
- Background session previews use a 1000ms budget. Raw text continues to accumulate on every event.
- One bounded animation-frame publication covers pending streams across sessions. Each session retains cancellation for deletion/eviction, and timer-only hosts retain the 32ms publication fallback.
- Before subscribers capture a newly activated session, the native host synchronously refreshes its current preview and cancels the older background timer. Same-session notifications do not bypass the foreground budget.
- A WeakMap keyed by the actual message bubble records its last successfully rendered text. Activating a current preview skips parsing, including repeated switches between stalled large streams. Failed renders do not update this record or retain the previous foreground ID.
- Input requests, errors, tool boundaries, completion, failure and interruption retain synchronous semantic handling and final preview flushes.

The preview observer runs only after the native host's background-working-set and startup-publication guards. It does not publish recursively. An activation render failure is logged without rejecting session activation; raw text remains available with the previous HTML until rendering succeeds on a later publication. Returning to the previous session also retries its pending refresh.

This change concerns inactive **sessions**, not OS window visibility. Web transport and ACP projection are outside its scope. Web scheduling parity remains a separate follow-up.

## Before/after measurement

Measured on 2026-10-19 on Windows, Intel Core i7-8700 @ 3.20GHz, Node v24.13.0. Baseline: `fcb700f853a32b2900af73451814589af8e9fe79`, after safely synchronizing main and its submodules.

Each configuration has five warm-ups and thirty measured runs, alternating before/after order. Each session receives the same 200 Markdown chunks at 20ms virtual intervals; exactly one session is visible. Both variants execute the actual event handler, shared router, host notification function and marked parser. Setup and virtual waiting are excluded from timings; trailing preview work is included. Complete final raw text and HTML are asserted for every session.

The timing columns use the emitted report's `cases[].wall.p50Ms` and `cases[].wall.p95Ms`, measuring elapsed local processing time rather than parser-only or CPU time.

| Sessions | Wall P50 before | P50 after | P50 time saved | P50 reduction | Wall P95 before | P95 after | P95 reduction |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 14.81ms | 14.82ms | -0.01ms | -0.1% | 16.19ms | 15.78ms | 2.5% |
| 5 | 79.89ms | 32.14ms | 47.75ms | 59.8% | 91.66ms | 40.77ms | 55.5% |
| 10 | 164.26ms | 53.03ms | 111.23ms | 67.7% | 229.79ms | 119.54ms | 48.0% |

Rounded values come from the same run; saved time and percentages use unrounded samples. Single-session work counts are unchanged and timings vary between runs, so no meaningful single-session improvement is claimed.

| Sessions | Markdown parses before → after | Notification calls before → after |
| --- | --- | --- |
| 1 | 24 → 24 | 223 → 223 |
| 5 | 120 → 44 | 1115 → 225 |
| 10 | 240 → 69 | 2230 → 225 |

An activation diagnostic sends a background chunk at 0ms, another at 40ms, then activates that session at 50ms. The baseline first snapshot has stale HTML and its timer catches up 130ms later. The final implementation's first snapshot already has current HTML: **0ms timer catch-up**. This resolves the 950ms catch-up observed with the earlier timer-only prototype.

These numbers measure local handler/parser/lightweight-subscriber work during a synthetic replay. They do **not** measure Tauri/WebView2 input latency, model throughput, network/tool execution, total task completion time, full-process memory or CPU utilization. Notification calls are not React commits. The isolated session buffers and storage substitutes do not establish durable persistence correctness. Native end-to-end resource measurements remain unverified.

## Reproduction

From `pinvou3-app/`, after installing the locked development dependencies:

```powershell
npm run perf:native-stream -- --baseline-ref fcb700f853a32b2900af73451814589af8e9fe79 --output tmp/native-stream-performance.json
```

`--samples` accepts 5 through 200 samples per configuration (default 30). Output paths are relative to the repository root. The baseline defaults to the pinned commit above, even on a feature checkout; `--baseline-ref` explicitly overrides it. The script obtains immutable baseline sources from that local Git commit and compares them with working-tree sources. It records source hashes, environment, all samples, nearest-rank P50/P95, activation correctness and a staggered-arrival count check. Run performance measurements separately from builds and test suites. Repeat these checks after renderer or parser dependency changes, including the marked upgrade in PR #699; this run uses the current locked marked 14 dependency.

The local run's complete samples are in `tmp/native-stream-review-performance.json`; this generated/private-workspace directory is not part of the change. The committed script makes the comparison reproducible without that file.

## Verification and limits

- Regression tests first failed against the baseline for background parse frequency, activation HTML and cross-session publication counts. The implementation makes these checks pass.
- All 25 native scheduling tests pass, covering activation, unchanged-preview reuse, cross-session render isolation, draft return, rendering failure/retry and return to the previous session, the unchanged immediate foreground publication at 180ms, same-session budget, no-timer fallback, cross-session publication, reentrant and throwing subscribers, input/error handling, tool boundary, completion/failure/interruption, background storage attribution, eviction and frame fallback. Together with one detached-window lifecycle test and one bridge-domain protocol test, the focused run passes all 27 checks.
- Mutation checks confirm that removing background publication fails two tests, changing its budget from 1000ms to 5000ms fails three tests, and removing activation publication cancellation fails one test. Still-background tests assert no new publication through 999ms, current HTML at 1000ms, and publication at 1016ms (animation frame) or 1032ms (timer fallback).
- The real browser UI smoke passes all 55 checks, including a new assertion that the first subscription snapshot after activating a background stream contains current HTML and that the output becomes visible. It exercises the real frontend/bridge with mocked Tauri services, not a native Rust host.
- The detached-window startup fixture models the preview observer as synchronous and non-publishing, preventing its generic mock from recursively calling `notify` for the new `refresh*` method.
- Desktop and Web UI builds, Safari 14 compatibility audit, architecture guard, protocol contracts, affected lifecycle tests, all five frontend lint commands and changed Node-file lint pass. UI lint reports zero errors and pre-existing documentation warnings in unrelated files.
- The original implementation full Node run was not green (the review follow-up reruns the focused checks above): 1172 checks, 1159 passed, 11 failed and 2 skipped. Seven static contract failures (auxiliary chat, browser surface, connector, sidecar and build gates) reproduce in a clean archive of the synchronized baseline with checkout-equivalent line endings. Four failures involve unchanged Windows browser-wrapper tests; three pass on isolated rerun and the cancellation/deadline failure also reproduces on the clean baseline. No new failure attributable to this change was found. This is not a claim of a clean full-suite run or native device validation.
- Independent follow-up review found no remaining blockers after the failed-activation return path was fixed and covered by a red/green test.

No application UI text, configuration format, network endpoint, runtime dependency, permission policy or CodeWhale behavior changes.
