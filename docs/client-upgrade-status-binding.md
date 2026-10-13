# Client upgrade consume-status binding math

T27 batch 4 computes the frozen consume-status binding and recovery-handle hashes.
The status method accepts only an already bounded duplicate-free Document whose
complete root has exactly nine required fields: recoveryHandleHash, product,
component, installationScopeId, purpose, authorizationJti, consumeKey,
transactionId and consumeRequestDigest. All six identifiers use the original
ASCII 1..128 pattern; both hashes are exactly 64 lowercase hex characters and
purpose is precisely preinstall/install/activate. Extra transport/self fields,
missing/null/wrong types and other purposes fail. Unlike source projection, no
outer field is silently excluded and no missing value is defaulted.

After shape validation, the existing bounded JCS writer serializes the entire
object. The preimage is fixed ASCII pinvou-consume-status-v1 followed by one actual
NUL byte and those complete canonical bytes; SHA256 of this input is the binding
hash. No caller-controlled domain or generic Serialize entrance exists. The
private StatusBinding constructor exposes exact input/hash without automatic
Debug/serialization. Source and status share only private predicates from the
frozen common Schema; source projection behavior remains independently regressed.

recovery_handle_hash borrows exactly 32 provided raw bytes and computes SHA256
without a domain, base64 interpretation or wrapper. The result stores only the
hex hash, not the raw secret, and has no Debug/Serialize. Wrong length returns a
fixed error without retaining the input. The function does not generate secrets,
prove entropy/possession, provide memory protection or attest secret custody.
The owner remains responsible for unpredictable generation, isolation, original
consume/query association and lifecycle handling. No secret is persisted, logged,
transmitted or used to issue a request by this module.

Tests match the frozen input hex/hash and recovery vector, distinguish actual NUL
from escaped text, preserve reordered/pretty JSON, bind every field and allow all
three purposes. Root extras/absence/null/types, opaque identifier boundaries and
line endings, hash shape, decoded escaped duplicate keys, secret lengths 0/1/31/
32/33/64 and base64-text refusal are covered. The original Node/Ajv reference
independently compares acceptance, exact preimage and hashes across the matrix.
The all-zero 32-byte control is length-valid, explicitly showing that hashing
alone asserts no entropy. Earlier source, signing-input and byte-codec matrices
remain regression controls, including the actual original Node oracles.

The App facade checks the fixed status/recovery vectors and rejects a transport
extra through its actual dependency graph. Final checks and scoped review are
recorded in [implementation progress](upgrade-implementation-progress.md).
This batch changes no dependency, lock, protocol Schema, version or gitlink.
It performs no network query, consumption, server ledger lookup, signature
verification, trustworthy timing or installation action. Full T27/T32 remain
unfinished and the unresolved Ed25519 point policy is not bypassed. Downloads
retain no total/cumulative duration or retry cap.
