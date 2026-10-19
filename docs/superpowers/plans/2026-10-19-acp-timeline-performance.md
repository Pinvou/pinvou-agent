# ACP timeline projection performance plan

**Goal:** Reduce repeated projection of completed ACP history without changing the event log or observable timeline content.

**Architecture:** Keep `projectAcpTimeline` as the authoritative full projection. Add a view-owned projector that recognizes immutable, ordered, single-session append-only logs and reprojects only affected turns. Retain one latest cache per mounted view. Replace snapshots, reordered/duplicate events, session switches and language/redactor changes use the full path. Do not change transport, event accumulation, persistence or model execution.

**Alternatives:** A post-projection deep comparison retains full computation cost. A mutable per-event reducer duplicates the existing merge semantics and increases regression risk. Turn-scoped reuse shares the current projector and preserves unrelated turn references.

## Tasks

- [x] Write failing identity and immutable-output regression tests against the existing full projector.
- [x] Implement the bounded projector and connect it to `CodexAcpView`.
- [x] Verify exact equality with full projection after every event, including permissions, elicitation, tools, terminal statuses, old-turn updates, snapshots, gaps, duplicates, out-of-order events, multiple sessions and language changes.
- [x] Measure 1/200/1000/2000-turn histories with identical append/projection workloads, alternating 30 before/after samples after warm-up. Assert complete output and record baseline/source hashes. Defer cache setup after measuring eager initial-hydration overhead.
- [x] Run relevant ACP/conversation tests, lint, architecture guard, UI builds and affected browser smoke; review regressions and document evidence and limits.

## Acceptance and boundaries

Unchanged turn references survive ordinary append; prior published projections never mutate; output equals the full projector. A malformed or unsupported log uses the existing behavior. Only measured local projection work is reported; no model, disk, native device or total task acceleration is claimed. The previous background-stream PR is separate and is not included in this branch.
