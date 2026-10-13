# Client upgrade supplied high-water comparison

T27 batch14 compares a required supplied record Document with one original
CanonicalEnvelopeShape. There is no second byte/digest parameter to substitute
for the envelope factory's original bytes. Only Root/Timestamp/Snapshot/Target/
Release are supported; current version must be positive SAFE, with Release using
revision only. Package and credential roles are explicitly unsupported.
This role/version subset states the original full-Schema precondition and does
not claim arbitrary malformed-body equivalence to the private/reference helper.

The entire context object {role,product,component,scope} is compared by bounded
JCS against supplied record.scope, including all scope fields and extra context
keys. Both subtrees passed their original whole-input resource/duplicate checks;
the borrowed four-field wrapper is shallower than the original envelope and
introduces no large Value clone. A required positive SAFE record.version is then
compared as the existing finite binary64 semantics permit exact SAFE integers.
Lower candidate version refuses rollback. Equal version requires lowercase64-hex
envelopeSha256 matching SHA256 of the original complete raw envelope, including
signatures. Higher version intentionally does not consult the previous hash:
missing, null, number, uppercase or wrong hash remains comparison-positive,
matching the original function. Ignored record fields retain raw bounds/duplicates.
Error diagnostics contain fixed codes only, including facade envelope errors.

The supplied record is not attested trusted, durable or authoritative; the
envelope remains partial context/encoding without full role Schema or cryptographic
verification. This check neither persists nor advances the high-water record,
establishes first acceptance, forgives lost records, resets Root lineage nor
grants execution rights. No absent/undefined/null record becomes automatic success.
Protected authenticated acceptance and atomic metadata/byte persistence remain
subsequent T27/T28 work. No secure-erasure guarantee is claimed.

Four groups cover five supported and all15 unsupported roles, every context
field and complete scope, Release revision without fallback, missing/invalid
positive current versions and records, ignored-extra resources/UTF8/escaped
duplicates, equal-version signature-byte-only changes, and1935 actual Node
assertHighWater comparisons over the supported valid-body/version subset.
Cases cover SAFE upper boundaries, every ordering branch, all hash variants for
equal and newer versions and original numeric spelling equivalence.
The oracle imports the actual frozen exported function, parses the same raw
envelope bytes and bounded record, and never copies its algorithm or changes
frozen reference files. The actual App facade uses the same original bytes and
checks Release equal/hash mismatch/rollback with explicit encoding errors.

No dependency, lock, Schema, CI, version or gitlink change is needed. Final
evidence is in [implementation progress](upgrade-implementation-progress.md).
Complete T27 trust and persistence integration remains unfinished. Downloads
retain no total/cumulative duration or retry cap.
