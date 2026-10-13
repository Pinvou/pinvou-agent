# Client upgrade canonical envelope shape

T27 batch13 decodes the same complete original wire bytes with the caller-selected
metadata (1 MiB) or credential (64 KiB) profile and requires exact equality with
bounded JCS bytes. It never rewrites noncanonical input into a positive result.
Closed outer signed/signatures structure and the five common signed context fields
are checked: protocolVersion=1, one of20 frozen roles, product identifier, explicit
root-null or other-role identifier component, object scope. Six metadata roles
and14 credential roles must match InputKind; calling the larger profile cannot
admit a credential. ROLES was moved unchanged into the shared scalar predicate
module; original Root validation uses the same unchanged set.

This is explicitly a partial shape, not complete role-specific Schema. Minimal
bodies or signed extras can pass after whole-wire resource/duplicate checks.
Expected product/component/scope, role-specific fields, time/semantics, key
authorization, cryptographic verification, thresholds, Root trust/lineage and
current execution rights are not established.

Signatures require1..4096 closed two-field records, lowercase64-hex keyId and
exact86-character unpadded base64url signature. Decode must produce exactly64
bytes and reencode identically, rejecting nonzero pad bits. Original IDs must be
strictly increasing and unique; no sorting occurs. Raw64-byte encoding facts do
not attest curve points or valid signatures. Zero/weak bytes are shape-positive
test examples only. No public constructor, Debug or Serialize is provided.
After all checks the original signed subtree is moved into a bounded Document,
avoiding a whole-envelope clone. The private shape retains original raw bytes and
typed records with immutable getters. Original/caller data are not securely erased.

Four groups cover all20 original public fixtures and getters, opposite profiles,
missing/invalid common fields, minimal bodies, noncanonical whitespace/order/
escape/number spellings/newline, escaped duplicates and invalid UTF8, depth/string
limits, exact/+1 wire limits for both profiles,4096 signatures/+1 and2080 actual
Node differential cases. The oracle executes unchanged extracted parseEnvelope,
decodeBase64Url, isMetadata and schemaName with actual frozen imports. It compares
the original parser plus encoding helper combination, not parseEnvelope alone.
Every differential case retains an original full-Schema-valid signed body;
arbitrary minimal-body/full-Schema equivalence is not claimed. Extraction failure
has no copied-function fallback. Frozen reference files are unchanged.
The App facade checks original metadata and credential fixtures, wrong profiles,
noncanonical trailing space and noncanonical signature pad bits.

No new dependency, lock, Schema, CI, product version or gitlink is needed.
Final validation evidence is recorded in
[implementation progress](upgrade-implementation-progress.md).
Complete T27 signature/trust integration remains unfinished. Downloads retain no
total/cumulative duration or retry cap.
