# Client upgrade transient cache observations

T29 batch 10 gives the protected cache owner temporary response, whole-write and
pending-wait observations from the actual restricted operation. Existing B8/B9
entry points use a Noop observer and retain their behavior; explicit with_observer
entry points expose these callbacks. No response is fabricated for VerifyExisting.
Callbacks receive no URL, body bytes, File, client, timestamp or mutable plan.
Neither notices nor their raw ETag fields have automatic Debug/serialization,
logging, persistence or upload. Their constructors remain internal.

Response facts come from the actual validated body plan and optional usable
strong ETag before writer start. They include only the original Artifact, write
mode and response span. The opaque borrowed tag may contain nontext bytes; it
must never be lossy-normalized into a different validator. The owner may copy it
as provisional resume information, but equal tags or these header facts do not
prove immutable-source continuity, retained prefix identity or protected storage.
Direct reception may report a response before cancellation/permission denies
writer start; a notice's existence proves no file action. The single-attempt
entry rejects unauthorized 200 replacement before handing the body to reception.

Progress is emitted only after the actual writer's entire chunk succeeds. Checked
cumulative response bytes and original start plus those bytes produce next_offset,
bounded by the original response span and Artifact size. It never uses the SDK's
pre-delivery count or B1's pre-write count as observed successful output. A short
206 reports its own next offset, not the whole artifact size. A failed write may
already have left partial bytes without any Progress; absence of Progress does
not establish zero writes. Progress implies neither sync nor complete hash.

Waiting occurs only in the timer branch while the same pinned Fetch or Body
future remains pending. Continue preserves it and the original SDK timers. The
phase does not identify DNS versus TLS/headers, trustworthy time, lost contact,
unknown outcome, stage-quality contribution or a signed telemetry event. Credible
progress or confirmed waiting is still unfinished; no business result is committed.
There is no total/cumulative download duration or retry cap.

Observer Stop yields a separate fixed OwnerStopped error and consumes the actual
body/future without a success receipt. Response Stop precedes writer start and
preserves old bytes, length and cursor. Progress or Waiting Stop can leave
untrusted actual output; even a complete count cannot finish without EOF/sync.
Continue never grants replacement permission, source continuity or execution rights.
The owner decides pause/cancel/recovery semantics later and confirms safe stop
before releasing protection. Callbacks must return promptly, cannot reenter the
operation or nest block_on, and may run inside a future poll for Waiting. Polling
cannot interrupt a running callback, OS operation or active future poll; 100 ms
is a scheduled observation interval, not a hard stopping guarantee.

Real isolated HTTPS-to-file controls cover Response Stop preservation, Progress
Stop at complete count without a receipt, original tag and two actual appended
bytes before a network failure, short range progress, Body/Fetch waiting Stop,
actual DNS call marker with one call, continuing progress, Continue failing to
replace without permission and Continue waiting for the existing fixture idle
limit. A local complete-cache test verifies no synthetic notices or DNS. A private
write hook writes one real byte then injects failure, proving no completed-chunk
Progress is emitted; this is unit fault injection, not physical disk-failure
certification. Earlier Noop transport/cache matrices remain regression controls.

The App facade exposes observation to later owner orchestration. Final checks
and independent review are recorded in
[implementation progress](upgrade-implementation-progress.md). This batch changes
no dependency, lock, product version or gitlink. Physical protection, persisted
resume state, current qualification/URL renewal, signed progress ingestion, native
authenticity/rights and full T29 remain unfinished. Complete B3 validation still
uses the original T26 budget; downloads retain no hard elapsed or retry cap.
