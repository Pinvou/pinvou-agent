# Client upgrade exact original metadata reference

T27 batch10 validates a private MetadataReferenceShape against frozen common
metadataReference and checkReferenceRoles rules. The reference is closed to role,
scope, version, expiresAt, size and envelopeSha256. Roles are the six metadata
roles; size is1..1MiB and SHA is lowercase64hex. Scope keys are empty for Root/
Timestamp/Snapshot, channel+targetKey for Target, releaseId for Release and
targetKey for Package. Every scope value is an identifier; no channel/target
enum restriction absent from this reference contract is added. Package version/
expiry must be null; other roles require positive safe version and nonnegative
safe nonnull expiry. No current time or expiry-window check occurs.

derive_metadata_reference first parses the complete original metadata wire bytes
through Document bounds/duplicates. It projects signed role/scope, Release
revision (never fallback version), other metadata version or Package null,
expiresAt using the original missing/null behavior, and actual original byte
length/SHA. Package nonnull expiry is not silently erased; resulting invalid
reference shape is refused. Bytes are hashed without JCS reserialization.
assert_original_envelope derives again from the supplied current original bytes
and compares all six canonical reference fields, never just size/hash.

The result has no Debug/Serialize/public constructor. These functions establish
reference shape and byte association only, not complete Envelope Schema,
signature validity, expected product/component, Root authenticity, time validity,
metadata high-watermark state or current qualification. Minimal unsigned claims,
missing/ill-shaped signatures and unselected extra claims can still derive a
shape-positive reference, exactly as the original projection does. All extras
remain bound by raw SHA and subject to complete raw resource/duplicate checks;
ignored extras are not traversed as metadata references. The owner authenticates
the expected enclosing reference and performs the complete trust/context checks.

Four groups cover all six metadata roles, original public metadata fixtures,
every reference field, original wire versus pretty bytes, same-length order or
opaque/signature-field changes affecting only raw SHA, role/scope/key/value
shapes, Release-only revision/Package-ignored version/expiry preservation,
raw ignored extras and literal/escaped duplicates, safe binary64 and size
ceilings, exact/+1 raw1MiB and original decoded string limits. Actual original
Node/AJV oracle applies the common reference Schema through Timestamp.snapshot
plus checkReferenceRoles, then original envelopeReference/assertReference;
it does not accidentally add Timestamp time/snapshot-role semantics. An actual
App facade checks original wire and rejects changed whitespace or reference
version. No production keys or private fixture materials are stored.

No new dependency, lock, Schema, CI, product version or gitlink is needed. Final
evidence appears in [implementation progress](upgrade-implementation-progress.md).
Complete T27 trust/chain/high-watermark integration remains unfinished. Downloads
retain no total/cumulative duration or retry cap.
