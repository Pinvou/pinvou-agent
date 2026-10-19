# Background Stream Performance Implementation Plan

**Goal:** Reduce background Markdown work while preserving immediate foreground catch-up, semantic events and terminal correctness, and report comparable before/after measurements.

**Architecture:** Keep authoritative text and persistence unchanged. The existing Tauri stream adapter owns preview timers; background previews use a longer budget. Before publishing a newly active workspace, the host synchronously refreshes its pending preview. Stream publications can share a frame across sessions; semantic notifications stay synchronous. No CodeWhale, Web transport, model or permission policy change.

**Tech stack:** Existing JavaScript bridge, Node test runner, deterministic VM replay and marked. No new runtime dependency.

## Implementation

- [x] Capture the synchronized main baseline and source hashes; preserve the previous experiment separately.
- [x] Add a reusable test harness executing actual native stream handlers, the shared event router and the host notification function with isolated session buffers and virtual timers.
- [x] Add failing tests for background parse budgets, foreground activation including draft transitions, immediate input/error events, completion/failure/interruption, tool boundaries, eviction, frame fallback and cross-session publications.
- [x] Add a background-only preview budget and synchronous activation refresh before subscriber snapshots. Coalesce stream publications without changing semantic event timing.
- [x] Run focused native bridge regression and architecture/protocol guards; fix any discovered in-scope gap.
- [x] Measure synchronized baseline and final production source with the same parser, notifications, input, warm-up and alternating order; retain raw samples and assert full content and zero activation catch-up delay.
- [x] Self-review behavior, exceptional states, scope and evidence. Run UI lint/build and the relevant browser smoke. Document limitations explicitly.

## Files

- `pinvou3-app/src/platform/tauri/bridge/chat-events.js`: preview and stream publication scheduling, activation refresh.
- `pinvou3-app/src/platform/tauri/bridge.js`: activation refresh before observable publication.
- `pinvou3-app/tests/helpers/native-stream-harness.mjs`: real bridge function execution with substituted transport/storage.
- `pinvou3-app/tests/native_stream_scheduling.test.mjs`: scheduling and lifecycle regressions.
- `pinvou3-app/scripts/native-stream-performance.mjs`: deterministic before/after replay using the same harness.
- `docs/background-stream-performance.md`: design, measurements, reproduction and verification boundaries.

## Acceptance

Background parse work decreases at 5/10 sessions; visible first output and activation remain synchronous; input/error/tool/terminal events remain complete and timely. A single-session slowdown must be disclosed. Measurements describe local handler work, not model/task acceleration. Tests do not substitute for native Tauri/WebView2 end-to-end confirmation.
