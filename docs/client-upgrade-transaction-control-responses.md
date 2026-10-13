# Supplied transaction-control response contracts

T27 batch 28 implements the complete original response API contracts for
consume-status, three cancellation purposes and three execution-review purposes.
A fixed seven-operation enum selects frozen closed Schemas with exactly four
embedded common/status-response/read-set/revision-ref resources. Unregistered
resources fail locally even when parent networking features are enabled.

The whole input is bounded metadata JSON, canonicalized and strictly reparsed
before Schema validation. These seven response shapes contain no credential
and have no additional reference response semantic guards. Successful checking
returns unit; it does not correlate a live request, authenticate the server,
prove a durable consumed ACK, establish current/complete read-set membership,
confirm the supplied verified-object hash or revive a retired executor.
Transport/owner/T19/T32/T36 must establish those facts separately.

consume-status consumed requires a nonempty transactionId; cancelled/expired
require an explicit null transactionId. Cancellation responses permit consumed,
cancelled or expired independently of authorizationRevision. Review response
revision, reviewedAt and verifiedObjectSha256 follow their frozen field Schemas.
Request identity and supplied times remain structural facts.

Read sets contain 1..4096 unique whole revision references. Object property order
and zero/negative-zero do not distinguish duplicates. A record with a different
revision or hash is a different whole reference and remains Schema-valid; no
logical record-key uniqueness or actual complete-read-set rule is invented here.
Cancellation purposes share the same response shape, as do review purposes.

Four Rust integration groups exercise seven operations/status nullability,
cross-family distinction, same-family compatibility, duplicate/distinct read
sets, raw byte/member limits and duplicates, plus 434 cases from actual complete
original assertApiContract. The Node oracle asserts intended outcomes for named
positive/negative controls and serializes invalid cases without first applying
a positive-only canonical helper. The real App facade covers all seven with
precise invalid-contract negatives and restored positives. A resource test
verifies local-only resolution.

Verification and independent review evidence are recorded in
[progress](upgrade-implementation-progress.md). No Schema/OpenAPI/profile/vectors,
dependencies/locks/CI, crypto/trust, product version or gitlinks change.
Downloads retain no total/cumulative duration or retry cap. Full T27/T32 remain
unfinished until real trust, transport, storage and ownership workflows exist.
