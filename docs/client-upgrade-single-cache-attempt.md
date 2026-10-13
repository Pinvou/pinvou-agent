# Client upgrade single restricted GET/cache attempt

T29 batch 9 combines one original RequestPlan, destination policy, resolver and
URL with actual cache reception. CacheFetchRequest keeps these inputs immutable,
with mandatory independent ReplacementPermission. Calling the attempt requests
append for a legal original 206 range; replacing bytes for a legal 200 additionally
requires Allow. Refuse preserves old bytes even if the provider ignores Range.
The sole target comes from the original request and actual response plan; no
second target, arbitrary headers, custom Client or production CA is exposed.

On an owner's independent blocking worker with an actively driven runtime, the
entry point pins the same actual fetch future and polls cancellation while DNS,
TLS/connection or response headers are pending. False polls do not restart DNS,
GET or existing single-operation timers. Cancellation is checked again after a
ready result before writing or returning a future-action projection. Data reception
then delegates to B8: only actual transport EOF can complete actual-file writes.
No retry, qualification check/refresh, URL renewal, session replacement or new
deadline origin is performed here. Downloads gain no total/cumulative duration
or retry cap; existing SDK operation/idle limits remain distinct.

The outcome is an observed Segment, a Restart reason for 412/416, or VerifyExisting
for B1's complete-cache plan. The last two leave bytes, length and cursor untouched.
VerifyExisting may bypass even URL parsing/DNS because it requests later local
whole-file verification; it does not establish that current file bytes are complete
or trusted. Association loss does not force a new download. The owner later runs
actual B3 verification under its original T26 validation budget and independently
satisfies current qualification, source continuity and physical protection. None
of the outcomes is immutable cache evidence or installation permission.

Tests use the real isolated HTTPS peer and actual protected-owner fixture files:
replacement refusal/approval, short range success, 412/416 preservation, mismatched
validator refusal, DNS-await cancellation and a positive completed resolver. The
DNS callback waits for the resolver's actual call marker; its cancellation leaves
old bytes/cursor intact and the call count stays one. The header-wait peer records
the exact request but sends no headers or body; independent wire verification
requires that request to reach it, excluding cancellation before DNS/TLS/request
arrival. A standalone complete-cache test uses denied policy, empty resolver and
invalid unused URL, verifies zero resolver calls and unchanged cursor, then performs
actual B3 verification. It also tests cancellation after an immediate local result.

The shared pinned-future helper continues B8's pending-body cancellation controls.
Runtime restrictions, blocking OS calls, safe-stop responsibility and failure
isolation from B8 remain. Private fixture CA/profile injection is crate-internal
only; no test root or trust bypass is exported. The App facade exposes this one
attempt to later owner orchestration. This batch changes no dependency, lock,
product version or gitlink. Real HTTPS-to-file tests are isolated interoperability,
not a full App workflow, protected cache/staging or production provider approval.

Final checks and independent review are recorded in
[implementation progress](upgrade-implementation-progress.md). Current qualification,
URL renewal/retry coordination, protection, native authenticity/rights and full T29
remain unfinished. Downloads retain no hard total/cumulative limit or retry cap.
