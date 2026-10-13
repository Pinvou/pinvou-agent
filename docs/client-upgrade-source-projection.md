# Client upgrade stable source-fact projection

T27 batch 3 derives only the frozen sourceProfileId from already bounded,
duplicate-free supplied JSON. It selects exactly product, component, targetKey,
canonicalAppVersion, application, helper, launcher, installation and dataMigration.
All outer extras are excluded as in the original reference function, including
unrecognized extras; this does not validate the complete dynamic facts document.
The three native identities, installation and dataMigration are strictly closed,
fully required objects. Nested epochs, missing/null fields and wrong types fail.

Shape checks preserve the committed stableFacts/nativeIdentity Schemas: exact
ASCII identifier and lowercase hash patterns, all four target enums, positive
binary64 safe integer sizes, native/installation format enums and perMachine scope.
Canonical version is a decimal triple without leading zeros, but no integer
conversion or additional component-length bound rejects a valid long component.
Original strings are neither normalized nor trimmed. This shape operation adds
no role/format/target cross rules absent from Schema. Actual Node/Ajv is the
independent oracle for pattern endings, safe-number rounding and enum behavior.

Only the fixed validated nine-field projection may use the internal Document
constructor; its canonical bytes reuse the existing bounded JCS writer. The ID
is SHA256(JCS(projection)), without a domain prefix, NUL or signing wrapper.
SourceProfile's private constructor and no Debug/Serialize keep it separate from
arbitrary raw fact serialization. It exposes the complete canonical projection
and hex identity, not a claim that a real native probe verified those facts.
Complete original input limits/duplicates still apply even to excluded fields.

Tests retain the frozen sourceProfileId and canonical bytes, reordered/whitespace
invariance, dynamic/self/unknown outer exclusion and changes to every variable
stable field and native subfield. Closed nested shapes, missing/null/type cases,
all frozen target/format enums, unsafe/fractional/rounded binary64 sizes,
identifier/hash endings and length boundaries, valid very long versions, decoded
64 KiB strings, 32-level input depth, 4,096 members, 1 MiB wire and escaped duplicate
keys are covered. Original Node/Ajv compares acceptance, exact canonical bytes
and hashes across the generated matrix; no copied approximation is the oracle.
The initial test adapter treated the reference Uint8Array as a Buffer; that test
encoding was corrected before final verification, without changing production
hash behavior or frozen vectors. Actual non-ASCII controls use Rust Unicode
escapes so Windows shell encoding cannot replace the test characters.

The App facade derives the frozen fixture ID, keeps outer ownerEpoch excluded and
refuses a nested helper epoch through its actual dependency graph. Final checks
and scoped independent review are recorded in
[implementation progress](upgrade-implementation-progress.md). This batch changes
no dependency, lock, product version, gitlink or protocol Schema. Existing CI
installs the frozen Node reference dependencies before the isolated codec gate.

Native probing/protection, Registry uniqueness/current eligibility, signatures,
metadata high-watermark persistence and full T27 remain unfinished. The unresolved
Ed25519 point policy is not bypassed through a new verifier. Stable ID does not
replace clientFactsDigest, actual runtime epochs or installation rights. Downloads
remain outside total/cumulative duration or retry caps.
