# Client upgrade semantic consume-request digest

T27 batch 6 derives SHA256 of the frozen semantic consume-request JCS. The full
original input first passes bounded duplicate-free JSON decoding, including all
fields later excluded. The method requires a root object and excludes only the
exact four top-level fields consumeRequestDigest, recoverySecret, requestId and
headers. It retains every other field, then validates the exact closed 17-field
consumeRequest shape and its closed preparation, backup, staged, plans and
preinstallSlot objects. Unknown fields are refused; nested names matching excluded
root fields are not removed. Missing nullable fields are not defaulted to null.

The predicates follow actual common Schema: protocolVersion is binary64 const 1;
six identifiers use the original ASCII pattern; three hashes are lowercase hex64;
purpose is preinstall/install/activate. Epochs/revisions are positive safe integers;
stagedAt and stagedValidUntil are nonnegative safe integers. plans.installation
must be a planReference; all other five required plan entries may be null. Root
freezeEpoch, backup, staged and preinstallSlot may be null but must exist. Every
nested non-null object is closed and validates each field. Numeric spelling uses
finite binary64, including rounding before integer validation. Pure shape math
adds no temporal ordering, purpose combination or cross-record consistency rule.

The existing bounded JCS writer serializes the complete validated semantic object.
The digest has no domain, NUL, array or signed wrapper. There is no arbitrary
Serialize entrance. The private result stores only semantic JCS/hash, has no
Debug/Serialize, and does not clone excluded fields. Original Document and caller
bytes can still contain the excluded secret: this is no secure-memory, automatic
erasure or custody guarantee. Owners must protect both input and semantic facts.

Tests match the frozen digest, exact plain JCS, reordered/pretty JSON and arbitrary
values under each excluded root field. All variable semantic leaves and nullable
branches bind the digest. The actual original Node/Ajv oracle compares complete
nested acceptance, canonical bytes and SHA across required missing/null/types,
extra fields at every object, identifier/hash/enum limits, binary64 limits and
rounding, all purposes, allowed zero/descending staged times and nested escaped
duplicates. Excluded fields still reject escaped duplicates and input/depth/string/
member resource excess. The App facade checks the frozen digest, excluded fields
and a missing required nullable field. Final evidence and scoped review appear in
[implementation progress](upgrade-implementation-progress.md).

No consumption, attempt persistence, secret generation/transmission/erasure,
backup or slot authenticity, signature, trustworthy time, current eligibility or
installation rights is attested. Full T27/T32 remain unfinished. No dependency,
lock, Schema, CI, product version or gitlink changes. Unresolved Ed25519 policy
remains pending. Downloads retain no total/cumulative duration or retry cap.
