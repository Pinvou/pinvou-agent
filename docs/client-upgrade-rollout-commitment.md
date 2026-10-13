# Client upgrade rollout commitment math

T27 batch 5 computes the frozen rollout commitment from bounded duplicate-free
JSON. Literal JSON null denotes the empty set. Every other input must be exactly
one closed seven-field opening: targetKey, deploymentId, deploymentRevision,
rolloutId, rolloutRevision, releaseEnvelopeSha256 and leafSalt. Arrays, empty
objects, absence, null field values, wrong types and extra fields are refused.
The four target enums, two ASCII identifiers (1..128), two positive binary64 safe
integer revisions and two lowercase 64-character hex values match common Schema.
Numeric spelling follows the original finite binary64 parser and Node/Ajv
semantics, including rounding before safe-integer validation.

The module constructs [] for null or [opening] for one valid opening. It uses
the existing bounded JCS writer on this fixed array, then prefixes exactly ASCII
pinvou-rollout-set-v1 and one actual NUL before SHA256. Callers cannot supply an
arbitrary domain, prewrapped array or generic Serialize input. The private result
exposes exact input/hash with no Debug/Serialize; the input includes the supplied
leaf salt and candidate identifiers, so the owner must protect any explicit use.
Shared private target/revision predicates preserve source-profile validation.

The result establishes only shape and mathematical byte binding. Salt shape,
including an all-zero value, proves no entropy or freshness. The owner must
supply unpredictable salt, authenticate the opening, verify current signed Target
membership and bind qualification to the original transaction. This module does
not query, enumerate candidates, validate signatures, persist a catalogue, obtain
current authorization or grant installation rights. A null commitment does not
attest that a trusted server currently has no eligible candidate.

Tests compare both frozen commitment hashes and exact fixed array preimages,
reordered/pretty JSON, all seven field bindings, null versus array/object input,
escaped duplicate keys and the original Node/Ajv shape/preimage/hash matrix.
The matrix covers all original target enums, identifier/hash boundaries and line
endings, missing/null/types/extras, safe integer limits, fractions and binary64
rounding. Earlier signing, byte, source and status tests remain controls. The
actual App facade checks both frozen commitments and refuses prewrapped arrays.
Final verification and scoped reviews are recorded in
[implementation progress](upgrade-implementation-progress.md).

No dependency, lock, protocol Schema, CI, product version or gitlink changes.
Full T27 remains unfinished; unresolved Ed25519 point policy is not bypassed.
Downloads retain no total/cumulative duration or retry cap.
