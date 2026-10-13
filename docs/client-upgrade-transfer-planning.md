# Client upgrade transfer planning

T29 batch 1 provides a private, dependency-free transfer crate under
`pinvou3-app/src-tauri/src/features/updater/download/transfer` and an App facade.
It closes request/response byte-range planning and exact body counting. It supplies
no network client, clock, file writer, cache protection or installation rights.
Full T29 remains unfinished.

## Input ownership

The protected owner verifies the signed packageId/positive safe-integer size/hash,
current qualification and actual retained cache bytes/length under exclusive
ownership. Structural constructors validate field shape, not authenticity.
`SourceContinuity::SameImmutableEntity` is independently proven provider/owner
evidence; equal ETags cannot create it. The response planner captures the original
target and request kind instead of accepting a second target that could replace it.

An unproven, unbound or changed partial cache cannot produce an append request.
An exact-length complete cache can produce `VerifyExisting` even when its earlier
association is lost; the next owner action is actual full-size/hash verification,
which may establish safe reuse. The candidate does not claim that check passed.
A complete cache requires that local action before any new HTTP request. This
batch supports strong ETag resume only; providers using object-version evidence
without an ETag require a later adapter, not a fabricated strong validator.

## Response and byte boundaries

- A 200 response is always a full replacement from zero, including an ignored
  Range request. It cannot carry Content-Range or append to the retained prefix.
- A 206 response must match an original Range request, exact strong ETag, offset,
  signed total size and valid inclusive range. A shorter valid segment is allowed;
  its receipt preserves the next offset below the total. Content-Length, when
  present, must equal that response's exact span.
- A 412 plans restart. A 416 only offers local verification for an already complete
  cache; otherwise it plans restart. Neither status establishes download completion.
  Other statuses are explicitly refused for this byte planner; URL renewal and
  retry behavior remain the future workflow's responsibility.
- Only absent or identity Content-Encoding is accepted. Raw ETag bytes preserve
  RFC 9110 opaque text, including obs-text and backslashes. Weak tags, controls,
  embedded quotes and DEL are refused. OWS means SP/HTAB, not general whitespace.
  The adapter supplies every original field occurrence before duplicate folding.

The local input profile permits at most 4,096 raw header fields, 1 MiB aggregate
name/value bytes and 64 KiB per name/value or standalone ETag. Every occurrence,
including ignored headers, counts. These are implementation resource bounds for
future provider qualification; they do not change the frozen wire Schema or cap
artifact size at 32 MiB. Borrowed fields allocate no header copies. The real HTTP
adapter must bound its own parsing before allocation, validate transfer framing
and Content-Length/Transfer-Encoding conflicts, and disable transparent content
decompression. Counters accept entity body bytes after transfer framing decoding.

Body counters are constructed only from a validated plan. Exact EOF produces a
segment receipt; byte excess, arithmetic overflow, truncation or abort permanently
fails that counter. Failure and finished state refuse later chunks and another
completion. A receipt with the full byte count still proves no hash, signature,
platform authenticity, protected staging or execution permission. A plan is not
a cache ownership token; the future owner binds it to the actual request/body and
unique writer, persists partial state and verifies real bytes before any promotion.

There is no elapsed-duration field, total deadline or retry count limit. The
existing global full-download helper and its other callers are unchanged.

## Verification and outstanding integration

Eleven focused groups cover identity and safe-size boundaries, raw ETags, target
and cache mismatches, unknown continuity, replacement/partial/recovery decisions,
range arithmetic, duplicate/encoding headers, input limits, exact EOF and terminal
failure latches, plus the safe-integer maximum. The App graph control checks ignored
Range replacement and truncated-body refusal. CI runs standalone fmt, locked
Clippy and tests without requiring the heavy App gate.

Actual HTTPS/redirect/DNS destination policy, credential isolation, URL renewal,
retained-file tamper proof, immutable provider evidence, actual final hashing,
exclusive staging and physical-platform verification remain subsequent work.
This batch supplies no file/provider E2E or target certification. Deployment is
deferred; downloads retain no hard total/cumulative time limit.

All eleven groups, standalone fmt and Clippy with warnings denied passed. App
formatting, both complete locked App/CLI graphs, 46 CI policies, architecture,
version consistency and diff checks passed. Each parent lock adds only the new
local dependency-free package and its App edge. The initial Clippy findings were
two collapsible conditions, repaired without behavior changes. Actual Windows
App integration passed all nine updater tests, including the transfer facade
control (0.00s after a 2m35s build with the existing compiler stack wrapper).
No App-wide pass is claimed; earlier unrelated Windows limitations remain in
[the byte codec evidence](client-upgrade-byte-codec.md).
Both independent roles passed all eleven groups and gave final B1 scoped PASS
after reviewing the frozen implementation, scope and actual App evidence. No
mandatory finding remains.
