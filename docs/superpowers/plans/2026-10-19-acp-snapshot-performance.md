# ACP snapshot reconciliation plan

**Goal:** Remove repeated full-history duplicate scans and sorting during ACP snapshot recovery while preserving existing merge results and event ownership.

**Approved scope:** Optimize only `mergeAcpTimelineSnapshot` in the App state layer. Keep snapshot-wins duplicate resolution, strict session/sequence identity, existing snapshot duplicates, stable numeric ordering, selected-session local filtering, no-op snapshot identity and immutable inputs. Valid serialized envelopes use one sequence index and at most one final sort; irregular inputs retain the legacy replay path. Do not change transport, storage, model execution or CodeWhale.

- [x] Add a legacy replay oracle, semantic regressions and deterministic scan/sort work budgets; observe the work-budget failures on current production code.
- [x] Implement bulk reconciliation with conservative eligibility and explicit legacy fallback.
- [x] Verify recovery-specific semantics and exception behavior, including permissions, elicitation, terminal events, gaps, concurrent live tail and duplicate keys; review the unchanged stale-session guards.
- [x] Add a reproducible immutable-baseline benchmark for empty-current loads, full overlap, overlap with live tail and small local tails at 200/1000/2000 completed turns.
- [x] Run affected ACP/conversation tests, frontend checks, architecture guard, desktop/Web builds and browser smoke; obtain independent code review.
- [x] Document measured results and limits; complete self-review and prepare an independent signed-off PR.

Acceptance: output and selected event references equal legacy replay; no input is mutated; overlapping duplicate search is linear; new additions trigger at most one full sort. Performance claims describe the measured merge function only and distinguish recovery from first-open/model latency. The earlier background-preview and incremental-projection PRs remain independent.
