# Same-file FullPack and supplied Package byte binding

T29 batch 16 adds complete read-only binding of one actual downloaded FullPack
ZIP to supplied canonical Package metadata. It composes existing closed
envelope/Schema, independent expected context/full raw reference, Package
semantics and decimal version with the existing complete same-file ZIP scan.
A separate feature crate owns this metadata-to-physical-byte relationship;
the low-level archive library remains independent of protocol metadata.

The caller independently supplies Artifact packageId/size/hash, exact Package
reference, product/component/target context, canonical app version, safe supplied
time, explicit archive capacities and the protected same source handle.
Package role is mandatory. Every metadata/context/reference/version/Artifact
gate precedes source I/O, and the complete original manifest bytes must be
canonical. Supplied time is not attested; V1 Package has no expiration window.
Production incremental arrays remain empty through unchanged Package semantics.

The complete source is scanned to real EOF using the existing archive guard.
Its original target must match all three FullPack identity fields. A single
bounded set indexes direct regular member size/hash facts; finalInstaller,
helper, launcher and every helperArtifact must each occur. Duplicate equivalent
identities may share a member, and multiple same-content files remain valid.
V1 native claims provide no source path: installationTarget is a protected
destination declaration, so this module never interprets it as an archive
locator or chooses an executable. Nested ZIP members remain opaque; a native
file only inside a nested ZIP cannot satisfy direct FullPack membership.
SBOM/provenance files belong to the outer upload wrapper and are not required
as direct FullPack payload members; unchanged Package semantics still binds
the declared provenance final-installer hash. Supply-chain admission remains
the server/owner's independent task.

Cancellation before metadata, during scanning, during native membership or at
the last return gate rejects without a partial inventory. Failure never writes,
extracts or deletes source bytes. Returned PackageInventory contains observed
same-file facts only; no native publisher/format signature, immutable-object
token, qualification, installation permission or protected stage is granted.

Four scoped actual-file groups cover14 independently generated ZIP variants:
Store/Deflate, a33MiB member, same-content aliases, opaque extra nested ZIP,
missing each native role/extra helper, nested-only files and changed member
size/hash. Python hashes actual source/member bytes; actual original Node
Schema/semantics and strong signature verification preflight every generated
manifest. These signatures do not certify the synthetic native payloads.
Precise before-I/O wrong-role/context/full-reference/version/Artifact controls
restore the baseline and check the file cursor. Initial/during/final-poll
cancellation and current-file substitution return no facts. Deliberately invalid
Ed25519 signature with a matching supplied reference passes byte binding,
explicitly proving that this API provides no signature trust.

The actual App graph checks an original Store ZIP with structural claims,
precise native-size mismatch and restoration. It intentionally uses an invalid
signature and repeated abc byte identities; this validates App wiring, not
native certification. CI runs the scoped original/actual-file suite and routes
Python/Node fixture-only changes to it. Parent App/CLI locks add the local crate;
all external versions remain from the existing parent graph.

Full T29/native probes/protected staging/qualification and T27 signature/
Root/high-water ownership workflows remain unfinished. Deployment and existing
undecided platform/clock/crypto policies remain deferred. No Schema, profile,
OpenAPI, fixed vectors, product version or gitlinks change; downloads retain
no total/cumulative duration or retry cap. Verification facts:
[progress](upgrade-implementation-progress.md).
