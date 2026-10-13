# Client upgrade supplied Root policy shape

T27 batch8 checks the plain supplied signed Root body against the frozen
validateTrustedRoot structural contract. Input first passes the original complete
bounded/duplicate-aware Document parser. The Root body is closed to10 fields,
protocolVersion1/role root/product identifier/component null/empty scope,
positive safe version and nonnegative safe issued/expiry numbers. Arrays are
nonempty and retain the original4096-member and1MiB metadata limits.

Each closed three-field descriptor must use ed25519, canonical32-byte unpadded
base64url and the exact SHA256 of JCS algorithm/publicKey; key IDs are unique.
The existing descriptor implementation is reused without point validation.
Each closed six-field role policy uses the frozen20-role enum and nullable
component/channel/target shapes. Its positive safe threshold cannot exceed its
unique member count and every member must be a declared key. A structured
four-tuple detects duplicate role/component/channel/target policies while
preserving null/string distinction. Exactly one Root policy must have all three
scope fields null, threshold at least2 and at least3 distinct members. Nonroot
policies require a nonnull component. No extra role/channel/target combinations
or requirement for all roles/all keys to be used is imposed.

The result is private RootPolicyShape with bounded canonical supplied-body
bytes and no Debug/Serialize or public constructor. Array order and unused keys
are preserved. Like the original structural function, this does not check now,
issued-before-expiry, time-window duration, root signatures, bootstrap anchor,
rotation lineage, high watermarks, valid Ed25519 curve points or current trust.
Zero/identity raw public descriptors and equal/reversed times deliberately remain
shape-positive. The owner must establish all trust/time facts independently.
The unresolved strict Ed25519 point/signature policy remains deferred.

Four new test groups compare against actual original Node/AJV, covering every
role and nullable channel/target combination, closed objects/missing/types/extra,
exact policy scopes, duplicate keys/policies/members, absent members, threshold
boundaries, noncanonical descriptor encoding/wrong identity, safe binary64
rounding, original array order and unused keys, literal/escaped duplicate keys,
and exact/+1 wire/key/policy/member resource ceilings. An actual App facade
checks consistent claims and rejects weakened Root threshold. No production
keys, credentials or private fixture materials are stored.

No new dependency, lock, Schema, CI, product version or gitlink is needed. Final
evidence appears in [implementation progress](upgrade-implementation-progress.md).
Complete T27 Root authenticity/chain/high-watermark/native-fact integration
remains unfinished. Downloads retain no total/cumulative duration or retry cap.
