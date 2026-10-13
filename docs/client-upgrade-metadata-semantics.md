# Client upgrade six-role metadata semantics

T27 batch17 validates the complete six-metadata-role semantic subset from frozen
checkSemantics after canonical encoding and full Schema checking. A privately
constructed SuppliedProtocolTime accepts only a Document numeric literal with
nonnegative SAFE finite integral binary64 milliseconds, including -0. It is a
supplied value, not an attested trusted clock observation. Package also requires
this time shape despite having no own expiry window. No public constructor,
Debug or Serialize is provided for the time shape.

Root/Timestamp/Snapshot/Target/Release require issuedAt<=now<expiresAt, strictly
positive interval and maximum365/1/7/30/90 days respectively, including the exact
maximum. RootWindowMode defaults to Current. Explicit AllowExpired affects only
Root, retains the original positive bounded interval and now>=issuedAt, and works
on unexpired Root too. The option is ignored for other metadata roles as in the
reference; it does not attest historical custody, continuity or trusted lineage.

Recursive reference scope/version/expiry checks follow the original marker rule:
every object owning envelopeSha256 is checked. No nested reference is newly
required unexpired at now, and generic reference identifiers retain their original
acceptance. Snapshot target/release identity is role plus complete scope JCS,
ignoring version/hash; Package identity is hash only even across target scopes.
Timestamp expects Snapshot; Target requires baseline/ordinary/bridge Release refs
and exact sorted host architecture set. Release requires unique targetKey, Package
role/target scope binding and irreversible=>required backup in only that direction.
Package requires unique helperId, target/format, selected helper all-five-field JCS,
algorithm-specific40/64 revision length, empty production incremental packages,
final-installer/provenance SHA binding and native OS/helper/launcher formats.
There is no production:false relaxation switch in this API. Canonical errors
propagate; failure is never converted into an equality of two absent results.

These checks do not duplicate validateTrustedRoot or verify signatures, supplied
time authority, actual native files, Root/key/policy consistency, independent
expected context, persistent high-water acceptance or current execution rights.
Credential semantics are explicitly unsupported and remain a separate module.
Caller buffers/temporary values have no certified secure-erasure guarantee.
No physical deadline/download limit is introduced; these are signed metadata TTLs.

Four groups cover all six metadata and14 unsupported credentials, every interval
boundary, maximum/+1 and Root option, SAFE/-0/invalid supplied time, expired nested
reference positive controls, Snapshot identity rules, helper publisher with identical
bytes, production delta refusal, asymmetric backup rule and4854 original Node
composed-parser/encoding/Schema/checkSemantics cases. Expected context in this
oracle is self-derived only to isolate metadata semantics; independent production
binding remains separately validated in B16. Acceptance, not exact error-stage
equivalence, is claimed. Root inconsistent descriptors/thresholds remain semantic-
only positive, preventing this function from implying validateTrustedRoot.
The App facade checks supplied-time match, Root exact-expiry current refusal,
explicit expired-Root calculation and invalid supplied time.

No dependency, lock, Schema, CI, version or gitlink change is needed. Appropriate
regression includes the20-role Schema golden/minimal-negative group and all other
codec controls; the unchanged31556-case Schema mutation test retains B15 final
evidence and is not rerun. CI remains complete. Final validation is recorded in
[implementation progress](upgrade-implementation-progress.md).
Full T27 authenticity/persistence remains unfinished. Downloads retain no
total/cumulative duration or retry cap.
