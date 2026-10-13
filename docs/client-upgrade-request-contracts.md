# Complete supplied client request contracts

T27 batch 36 implements complete original assertApiContract request acceptance
for all26 non-management client operations. ClientRequestOperation aliases the
existing exact operation catalog, preserving its enum/name parsing and response
interfaces. Frozen request Schemas use exactly18 local dependencies; unknown
resources fail even under parent networking features.

Responsibility boundaries separate fixed Schema resources/cache, embedded
credential semantics/scope binding and operation-specific request guards.
Whole metadata input is canonicalized and strictly reparsed before Schema
validation. Every original embedded credential independently retains its64KiB
canonical cap and unchanged supplied semantics at max(iat,nbf), then binds full
request product/component/installId and installation/channel/revision/target
scope. This never verifies signatures or trusted current time.

The exact original guards cover disabled incremental arrays; consume
purpose/digest/scope; canonical32-byte recovery secret; validate purpose/null
fields; cancel authorizationJti/purpose; execution-review complete prepare
fields/transactionId; event purpose/lineage/facts digest/occurrence window/
allowed kind; task action revision/evidence digest. Existing consume digest
and credential semantic primitives are reused without changes.
Request contract does not add the later post-auth consume envelope-hash binding
or current request/read-set/ownership/eligibility proof. The original API accepts
a different authorizationEnvelopeSha256 when its supplied semantic digest is
rebuilt; the later consume-binding phase independently rejects that relation.

Four integration groups exercise all26, every attached credential cap and raw
metadata limits/duplicates, precise purpose/scope/recovery negatives/restores,
and7313 actual complete original Node cases. The oracle covers26x26 cross
operations, required/scalar fields surrounding the existing credential envelopes,
all scope leaves, specific API guards, supplied-time/signature/cap controls and
the later-binding independence positive. Existing complete nested credential
Schema/semantic suites remain their own proof. Isolated guard negatives first
pass original full request Schema and independent credential semantics before
requiring exact API error code. Consume identity controls change each of product,
component and installationScopeId and rebuild the digest, keeping the credential
and outer scope unchanged; every control restores the baseline. Non-telemetry
event executionPurpose is fixed by each original request Schema, so switching
to another vocabulary purpose precisely rejects with API_CONTRACT_INVALID
before the redundant relationship guard; these controls also restore the
baseline. The independent review initially requested an impossible Schema-valid
event purpose mismatch. Actual original preflight rejected that test draft;
the reviewer rechecked all five const fields and withdrew that part of the
finding without changing the contract. Preparation controls use actual ownerEpoch;
a draft initially used a nonexistent field and was corrected before shipment.
One local-resource/inventory unit and the real App request entry cover closure,
all26 fixed mappings and canonical recovery-secret rejection/restoration.

Checking returns unit, does not send a request, expose a trusted authorization,
establish current state/owned object, consume/cancel/review/execute, record an
event, erase a secret or release protection. Those actions remain real transport,
signature, owner and T19/T20/T28/T32 workflows.

Verification and independent review evidence are recorded in
[progress](upgrade-implementation-progress.md). No Schema/OpenAPI/profile/vectors,
dependencies/locks/CI, crypto/trust, product version or gitlinks change.
Downloads retain no total/cumulative duration or retry cap. Complete T27 and
real transport/ownership workflows remain unfinished.
