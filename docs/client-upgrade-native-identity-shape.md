# Client upgrade native identity shape and comparison

T27 batch 7 validates each supplied native identity as a bounded duplicate-free
Document with exactly five required fields: size, sha256, identity, publisher and
format. Size is a positive safe binary64 integer. Hash is lowercase hex64;
identity/publisher use the original ASCII 1..128 identifier pattern. Format is
exactly nsis/deb/dmg/helper/launcher. Missing/null/wrong type, extra fields,
unsafe/fractional size and malformed identifiers/hash are refused. There is no
platform, role or filename cross-rule beyond the frozen common Schema.

The privately constructed immutable NativeIdentityShape contains only bounded
canonical bytes and has no Debug/Serialize. Both operands must pass the complete
shape before assert_native_identity compares their whole JCS. Every field matters,
including identity/publisher/format even when size/hash match. Reordered/pretty
input and equivalent binary64 numeric spellings compare equal. Comparison does
not introduce a domain or digest. Fixed diagnostics contain no submitted identity.
Source-profile projection uses the same private native predicate, moved without
changing behavior; its original actual Node/Ajv matrix remains a regression.

These are supplied facts, including the operand named actual. Equality proves no
OS probe, Authenticode/native signature, package authenticity, object protection,
current Registry match, VerifiedExecutionObject or installation rights. The owner
must independently establish actual native facts on the same protected object and
validate the current authenticated expected identity before any privileged action.
Callers must protect explicit canonical-byte uses; omission of automatic logging
is not secure memory or erasure. Full T27/T29 remain unfinished.

Tests compare exact plain native JCS, reordered/pretty JSON and equivalent numeric
spellings; each of the five changed fields mismatches in both directions while
self-comparison succeeds. The original Node assertNativeIdentity/common Schema
oracle compares both operands' acceptance, equality and exact canonical bytes,
covering every original format, missing/null/types/extras, identifier/hash/line
ending limits, escaped duplicate keys, fractions, safe boundaries and rounding.
The App facade checks equality, publisher mismatch with identical package bytes
and root-extra refusal through its actual dependency graph. Final checks and
scoped review are recorded in
[implementation progress](upgrade-implementation-progress.md).

No dependency, lock, protocol Schema, CI, product version or gitlink changes.
The unresolved Ed25519 point policy is not bypassed. Downloads retain no total/
cumulative duration or retry cap.
