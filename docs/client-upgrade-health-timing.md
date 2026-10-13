# Client upgrade health timing

T26 batch 2 extends the private timing math crate with a health timing record.
The original total window is 300,000 milliseconds from the first actual new-version
launch attempt. Launching, stopping, restarting and waiting all consume it.
Each actual check has at most 120,000 milliseconds and is cut short by the original
total window. There are at most two checks and only one can be active at a time.

## Owner boundary

The protected owner must bind the record to the original transaction and scope,
provide proven clock projections, establish actual launch/check facts, safely stop
the previous check and prove an eligible new-version restart before retrying.
Explicit identity, data or launch failures remain immediate failures owned by the
health workflow. This arithmetic module launches no process, probes no health,
approves no restart and grants no execution or gate-release rights.

Private check handles distinguish the two callbacks within the record. They are
neither identity authentication nor authority tokens. Wrong/old callbacks are
refused before they can change the known watermark or active check. Unknown,
confirmed fault and backwards observations retain the current check and original
window, and expose no guessed remaining duration.

## Completion and timeout ordering

Waiting at either bound exhausts that window. An immediate owner-confirmed finished
check can be timing-eligible at exactly either bound if exhaustion has not already
been observed. Once observed, it stays exhausted: a late callback cannot undo it,
even after an intervening unknown clock or component fault. Total exhaustion takes
priority when both bounds are reached. Check exhaustion alone still permits the
one retry after owner-confirmed safe stop, if the total window has time remaining.

Completion is independent of the check's health result. `WithinBounds` proves only
timing; it never means local health passed. Safe-stop closure is a separate operation
that produces no timing-eligible completion. Unknown completion or closure does
not release the active check. Known closure saves the check's result in its own
immutable slot; the second check cannot overwrite the first. A duplicate completion
is refused. The owner serializes callbacks and timeout decisions deterministically.

The API accepts immediate completion observations, not historical completion time
replay. Existing finished timing records are queried separately. Cloning preserves
the original origin, known watermark, count, active check, exhaustion and finished
records in memory. Protected persistence/restart recovery are still unimplemented.

## Verification scope

Nine focused groups cover both bounds before/equal/after, shortened second checks,
overlap/third-check/old-handle refusal, safe-stop versus completion, unknown and
backwards observations, timeout latches across unknown/fault at equality, clone
preservation and the safe-integer maximum. An effective one-millisecond window near
the safe maximum is accepted without adding a needless overflowing 120-second
deadline. No download duration limit is added. This batch does not certify real
clocks, launchers, health probes, target budgets or full T26. Deployment is deferred.

The isolated draft and actual repository passed all nine new health groups;
the repository also passed eight existing budget groups. Independent review
identified and closed the untrimmed remaining-check value, then added the decisive
post-exhaustion unknown/fault/equal-completion regression. Both roles passed the
draft and actual-repository source review, and independently ran all 17 groups.
Standalone fmt/Clippy, App formatting, 45 CI policies, architecture, version and
diff checks passed. Actual Windows App integration passed all eight updater tests,
including the new health timing graph control (0.00s after a 2m12s build using the
existing compiler stack wrapper). No Cargo dependency, lock or CI setting changed.
No App-wide pass is claimed; earlier unrelated Windows limitations remain in
[the byte codec evidence](client-upgrade-byte-codec.md).
Both independent roles gave final B2 scoped PASS after reviewing the frozen
repository implementation, scope and actual App evidence. No mandatory finding
remains.
