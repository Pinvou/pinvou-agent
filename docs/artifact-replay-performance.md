# Artifact lookup misses during conversation replay

Conversation replay can attempt an artifact lookup for every distinct file
mutation, including source files that never create an artifact card. Previously,
each miss scanned the growing chat timeline backward.

The replay core now records the basenames of artifact cards it actually appends.
A missing basename proves that the existing lookup would return no card, so that
scan can be skipped. Possible matches still use the original lookup and update
helpers, including exact-path preference, basename fallback, absolute-path
collision handling, stable IDs and user/mutation boundaries. All five artifact
append sites register membership. Live artifact handling is unchanged.

Membership belongs to one synchronous replay invocation. Its Set is allocated
only after the first supported artifact card is appended. Any non-string query
or appended path disables filtering for the remainder of that invocation,
preserving legacy coercion and exception behavior even when an object changes
its string representation during an update. Failed replays retain no index for
the next replay. There is no new persisted state or public interface.

## Measurement scope

The baseline is `0baf7fa6c83c6d91f7959b564b386bddab3780b5`. The A/B harness
loads the actual desktop or Web bridge, shared replay factory, real Markdown
renderer and DOMPurify in Chrome. Desktop IPC and Web session-chunk responses
use synthetic in-memory fixtures. Each measured restoration must execute exactly
one durable load and one replay. Every A/B pair compares complete published
messages, chat items and busy state.

Measurements include replay, lazy membership construction and state publication.
They exclude real disk I/O, Engine execution, packaged WebView2, React rendering
and application startup. They establish a reduction in bridge restoration work;
they do not establish an equivalent reduction in user-visible loading time.

Final measurements use alternating variant order, three warmup restores and
11 paired restores per large-history case. First-restoration measurements use
11 new browser pages per variant, empty session caches and the public session
switch entry point; page/script startup is outside the timer. Small-history
controls use 31 pairs. Timing values are workload-specific medians, not universal
speedups. Source snapshots, hashes and raw samples were retained with the local
validation report.

### Large-history restore medians

Each 2,000-turn fixture has five tools per turn. Mixed fixtures write a
deliverable every 100 or 10 turns; the other writes are source files. The
pressure fixture writes a distinct deliverable on every turn.

| Fixture | Desktop before → after | Web before → after |
| --- | ---: | ---: |
| Distinct source-file writes, no artifact cards | 700.1 → 583.2 ms (−16.7%) | 586.8 → 512.5 ms (−12.7%) |
| Mixed, 20 artifact cards | 574.6 → 468.4 ms (−18.5%) | 599.7 → 501.2 ms (−16.4%) |
| Mixed, 200 artifact cards | 585.4 → 468.2 ms (−20.0%) | 616.6 → 496.5 ms (−19.5%) |
| Pressure: 2,000 artifact cards | 954.0 → 467.9 ms (−51.0%) | 976.2 → 502.5 ms (−48.5%) |
| Repeated edits to five deliverable paths | 490.8 → 503.9 ms (+2.7%) | 525.1 → 513.1 ms (−2.3%) |

Repeated-path controls show no consistent improvement across hosts. A separate
200-turn same-basename collision control was also effectively flat:
55.6 → 56.3 ms on desktop and 58.4 → 58.6 ms on Web. Positive-match semantics
are preserved rather than optimized away.

### First restore and small-history controls

| First restore fixture | Desktop before → after | Web before → after |
| --- | ---: | ---: |
| 50 turns, source writes | 38.9 → 37.0 ms | 43.0 → 41.8 ms |
| 2,000 turns, source writes | 617.3 → 561.9 ms (−9.0%) | 716.9 → 669.0 ms (−6.7%) |
| 2,000 turns, 20 artifact cards | 654.1 → 570.7 ms (−12.8%) | 727.7 → 625.9 ms (−14.0%) |

Empty-history medians remain 0.4 ms on desktop and 0.5 ms on Web. Warm 50-turn
controls varied from −0.5 to +1.6 ms across hosts and fixtures; a small-session
speedup is not established. No large first-restore median regression appeared
in these fixtures. Short runs do not establish a universal tail-latency bound.

Six forced-GC restoration observations had similar heap trends across variants;
they do not establish a memory reduction or prove long-term leak absence.

## Compatibility checks

Regression tests exercise the actual shared factory and extracted desktop/Web
host hooks. They cover all reachable append paths, exact/basename collisions,
relative/absolute upgrades, stable IDs, user and mutation boundaries, explicit
temporary/source-file presentations, missing/failed results, multipath patches,
persona interleaving, unusual coercions and replay after renderer failure.
Deterministic getter counts ensure that known misses really avoid old scans;
allocation counts ensure ordinary histories do not allocate an artifact Set.

Full-host checks additionally cover stale-load rejection, failed-load retry,
cached A/B/A navigation, renderer exception recovery, live hydration, failed
tools, background shell state, transient errors, cancellation, Interrupted
completion and a subsequent retry. These use synthetic host events, rather than
a real model or packaged application.

The final validation run passed 140 related Node tests, 486 full-host complete
output comparisons, and 240 generated-transcript comparisons (including input
immutability). UI/test lint, the architecture guard and both UI/Web builds
passed. Independent specification and code reviews also passed after adding the
unsupported-update regressions.

The optimization does not promise a gain for repeated edits to a few paths or
for basename collisions. Existing replay preprocessing already collapses
repeated paths, while possible basename matches deliberately retain old scans.
