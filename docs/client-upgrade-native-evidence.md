# Read-only Windows native-signature evidence

This bounded T33 workflow collects actual embedded Authenticode observations
from one ordinary local PE object. It complements the existing byte and package
verifiers; it does not execute or install an image, deploy a helper, initialize
protected bootstrap trust, or construct a `VerifiedExecutionObject`.

Build the independent leaf crate:

```text
cargo build --locked --manifest-path pinvou3-app/src-tauri/src/features/updater/native-evidence/Cargo.toml --bin upgrade-native-evidence
upgrade-native-evidence inspect FILE EXPECTED_SIZE EXPECTED_SHA256 [EXPECTED_IDENTITY_SHA256]
```

Size is a positive canonical decimal integer within the JSON safe-integer range.
SHA-256 inputs are exactly 64 lowercase hexadecimal characters. The optional
identity digest binds the observed `fileIdentity` JSON encoding from an earlier
report. It is an ordinary expected object identity, not an owner/publisher token.
Inputs do not authorize file execution or any remote action. The CLI has no
network, certificate, provider, policy or CA override.

## One actual object

The raw-path entry opens each component relative to retained actual directory
handles, using the updater-local custody adapter shared with T29. UNC paths,
alternate streams, reparses, directories and hardlink aliases are refused.
The file entry accepts an actual File rather than a path. `ReOpenFile` obtains a
new read-only reservation on that same object with share-read only; existing
write/delete handles (including a live writable section in the tested Windows
environment) cause refusal. Hashing and all native calls use this actual handle.

The complete file is SHA-256 scanned before and after native collection, with a
64 KiB buffer, expected length/hash and identity checks. Production
`WINTRUST_FILE_INFO` has **no path** and carries the locked `hFile`. An observation
retains the handle and path-entry ancestry until dropped. This is process-local
custody, not a system privilege boundary, persistent anti-rollback protection or
a seal safe for passing a path to a privileged installer. It does not claim
protection against a privileged actor or arbitrary storage/OS compromise.

## Fixed native observation policy

`WINTRUST_ACTION_GENERIC_VERIFY_V2` runs with no UI, cache-only URL retrieval,
whole-chain revocation excluding the root, and MD2/MD4 disabled. There is no
online fallback, test-root installation or modification of certificate stores,
services, system date or trust configuration. The observation reflects the
machine/user's existing Windows trust environment.

The enumeration call supplies only the secondary-signature count. Every index,
including primary 0, is then verified separately with `WSS_VERIFY_SPECIFIC`.
The report preserves the status for each signature rather than choosing a
publisher or applying an invented any/all-signatures approval policy.

`verified` is narrowly defined: the native status, provider errors, trust-step
errors and the collected signer/timestamp-chain error statuses are zero, a
trusted root terminates each collected chain, and non-root certificates have
successful revocation evidence. Missing/offline/unknown evidence cannot become
`verified` merely because `WinVerifyTrust` returned zero. A refusal can lack a
built certificate chain; its actual native failure is still reported.

Provider countersigner chains are included when present. Their verification
as-of FILETIME and certificate validity FILETIMEs are lossless decimal strings.
They are signature-verification observations, **not T26 current trusted time**.
An empty timestamp list is reported as absent and does not prove a verified
timestamp. Native code-signing certificates and timestamps are optional in V1;
these observations do not gate package admission or upgrade activation. This
library does not implement Pinvou Publisher/SPKI/EKU/RFC3161 mapping, current
revocation freshness policy, timestamp allowlists or publisher qualification. A future
actual policy owner must consume the evidence and supply those decisions.

The report always sets `publisherQualification`, `currentQualification`,
`installationRights`, `verifiedExecutionObject` and `trustedDeadlineEvidence`
to false. No report or CLI exit code grants an upgrade permission. Exit 0 means
report collection succeeded, including unsigned/refused/unknown observations;
input, byte/identity mismatch, unsafe object, cancellation and unavailable
collection produce a fixed error with no partial success report.
An output/pipe failure may leave truncated bytes; a complete JSON document and
successful exit are both required. Lost output is recovered by repeating the
same explicit expectations for a new observation. There is no persisted ACK,
business transition, qualification or old result to infer from a partial report.

## Resources, cancellation and unsupported cases

Signature enumeration is bounded to 16 embedded signatures, each with up to 16
provider timestamp signers and 32 certificates per chain. Certificate encodings
are bounded to 1 MiB before hashing; their DER bodies, Subjects and tokens are
not printed. Other evidence is copied from live provider state, never retained
as raw pointers. Over-limit or incomplete collection refuses/returns unknown.

Stable SDK allocations keep every WinTrust pointer alive through state CLOSE.
Successful collection explicitly closes every state; cancellation/errors also
close via RAII before releasing the file. Native verification is synchronous;
cancellation does not abandon a verifier thread or release custody while it is
still using the object. Ctrl-C/Break requests cancellation where the process has
a Windows console. Killing a process relies on OS cleanup, not graceful CLOSE.

Network/mapped remote and special devices are unsupported. Catalog-only OS
images are not resolved through a catalog search and may report unsigned; that
does not claim Windows itself distrusts those files. Non-Windows platforms
return unsupported. These limits do not introduce a download total deadline.

## Verification boundaries

The Windows tests use genuine signed system PE bytes copied to unique owned
fixtures, real unsigned executables, and changed bytes with **recomputed** input
SHA-256 so that native signature failure cannot be masked by the byte gate.
Opposite signed/unsigned paths with actual hFiles prove provider object choice;
the production call supplies NULL path. Tests also exercise writable mappings,
write/delete contention, same-byte different identity, path replacement,
reparse/hardlink refusals and cancellation after a genuine native call.

Actual CLI child-process tests check complete reports/fixed errors and kill a
child only after observing its actual file handle/reservation, then wait for its
exit and verify that writes/rename work again. Test harness observation limits
are not product transfer deadlines. The separate explicit strict-positive test
requires cached complete chains and timestamps; default CI does not silently
obtain that material. Default hFile tests compare against the actual NULL-path
baseline and allow cache-related `unknown`/missing signer evidence. Separate pure
data fixtures test chain-completeness refusal rules without claiming an actual
native positive. None of this is native installer execution, full T33,
T26/T31/T46 certification or production publisher approval.

The custody extraction preserves T29 public APIs, fixed download-job error
mapping and the serialized Identity field profile. T29 jobs remain ordinary
untrusted journals and byte observations; sharing this adapter adds no rights.
