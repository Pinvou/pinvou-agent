# Client check and refresh response contracts

T27 batch 24 implements the complete supplied check/refresh response subset of
frozen assertApiContract. The fixed operation selects its original closed Schema.
The registry embeds only those two original response Schemas and their exact
common/Decision/Telemetry dependencies, rejecting every other retrieval URI
even when a parent enables network features. No caller Schema is compiled.

Bounded raw JSON parsing preserves duplicate/resource checks. Wire whitespace and
key order remain allowed; complete canonical expansion is bounded and reparsed
as in the original API contract. Each embedded credential independently retains
its original 64 KiB canonical limit. Existing B18's unchanged claims helper
validates supplied semantics at max(iat,nbf), including legal future nbf. This
is contract validation, not current-time or signature verification. API signature
arrays are Schema data; duplicate or unverified signatures are not promoted to
canonical envelope/verified signature proofs.

Decision and Telemetry bind all seven original identity fields and their full
scope using complete JCS equality. Refresh additionally binds writableSessionId
to the Decision telemetrySessionId. Request ID and workflowRevision are only
Schema-constrained; no additional relationship is invented.

Success returns unit only, never current check result, trusted Decision/session,
new execution rights, protected acceptance or metadata persistence. It does not
make a network request or render an update result. Production trusted verification
and persistence remain unfinished parts of T27.

Four integration groups cover fixed operations, isolated identity negatives and
restored positives, original supplied-time semantics, signature-data positives,
credential and raw byte limits, and the actual complete original Node API
contract across nested required/scalar mutations and coherent relationship
controls. A registry unit test rejects external/unknown resources. Actual App
coverage checks both operations and precise cross-lineage/refreshed-session
refusals. Oracle fixtures use only public envelope output, no private key output
or fake crypto verification. This original API export grants no authentication.

No dependency, lock, frozen Schema, CI, product version or gitlink changes.
Final evidence is in [implementation progress](upgrade-implementation-progress.md).
Downloads retain no total/cumulative duration or retry cap.
