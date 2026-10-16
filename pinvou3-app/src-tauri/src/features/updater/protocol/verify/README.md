# Supplied-Root envelope verification

`verify_with_supplied_root` performs the ordinary frozen V1 single-envelope
verification operation. It checks original bounded canonical bytes, the complete
embedded role Schema, independent complete context, the supplied existing Root's
fresh window and exact role policy, every submitted Ed25519 signature, the unique
configured threshold, credential `signingKeyId`, supplied denied keys, body time
and metadata high-water rules. It reuses the codec's fixed signing domain and the
App/CLI graph's exact `ring` 0.17.14 provider.

The caller must supply the existing Root from its trust state. Constructing
`RootPolicyShape` checks structure; it does not establish a bootstrap anchor.
`SignatureCheckedEnvelope` preserves original bytes and borrows that same Root.
It grants no protected persistent acceptance, trusted clock, current online
qualification, download/install rights or durable acknowledgement. A changed
Root returns `RootTransitionRequired`; a successor uses the separate operation
below. Complete chains and historical identities use their own operations.

`VerificationInputs::new` defaults to production semantics. Explicit trusted
callers may select `PackageSemanticsMode::Development`, matching the original
reference's `production=false` package rule. It permits otherwise-valid signed
incremental descriptors without relaxing any signature, context or time check.
The application facade fixes production semantics and exposes no remote mode
selection. Existing codec semantics APIs also retain their production default.
A package supplied with high-water state refuses; credentials ignore that
argument, preserving the reference's original applicability.

Run `cargo test --locked --manifest-path <this-directory>/Cargo.toml` with Node
available and the pinned `specs/upgrade/v1` npm dependencies installed. Tests use
all twenty original public envelopes and 111 complete frozen Node-reference
cases. Fresh test private keys remain in the Node process; returned fixtures
contain public Roots, claims and signatures only. The A/R/S encoding matrix
records actual provider compatibility, including accepted small-order controls;
it is sampled evidence, not a new key blacklist or custody qualification.
The input limits apply to protocol objects, not artifact downloads. This module
does not impose an artifact size, retry or total/cumulative duration cap.

## Single Root successor

`verify_root_successor` verifies exactly one continuous successor against the
supplied old Root, independently supplied complete Root context and safe time.
The expected context contains role `root`, independent product, component `null`
and empty scope; its bounded `Document` avoids a raw unbounded product serializer.
The original old Root window and issued time still apply, while old expiry alone
is allowed. The next Root must be fresh and have exactly old version plus one;
there is no successor after the maximum safe integer. All submitted signatures
must belong to the union of the two exact Root-role policies and verify actually.
Both policies must separately meet their thresholds; a shared actual signer may
count in both. Descriptor lookup gives the next Root precedence for shared IDs.

The mandatory `ActiveMetadataInput` slice holds each original envelope and its
independently expected complete context. The caller must supply its complete,
consistent active snapshot; this pure operation cannot attest completeness. Each
entry must be non-Root metadata and pass canonical encoding, full Schema, exact
context and every next-Root signature/policy check. This is key preservation:
expired or otherwise ordinary-semantic-invalid but Schema-valid active metadata
still needs its key. Active body time, Production restrictions, high-water and
denial options are not added. Empty and repeated entries are explicit and all
entries are checked, including a bad last entry. Only original per-object
resource limits apply; no aggregate 4096-entry or 32 MiB policy is invented.

`SignatureCheckedRootSuccessor` preserves original bytes, borrows the exact old
Root and owns the validated next Root. It installs/persists nothing, attests no
active-set completeness, acquires no bootstrap/clock/custody and grants no current
qualification or execution rights. Full T27 and protected Root storage/installation remain open.

Fifty-five fresh full frozen Node successor cases cover dual thresholds and extras,
continuity and safe-integer edges, both windows, active key retention, original
input limits and Schema-valid descriptor/policy inconsistencies. A separate API
test checks mandatory independent complete Root context. The 4097 repeated-entry case verifies absence of an invented set
cap. Private test keys never leave the Node process. Existing ordinary 111-case
oracles remain separate regressions. The App facade exercises both Root policies
and active key retention; actual CLI graph compilation is not workflow E2E.

## Complete Root chain

`verify_root_chain` accepts one through 4096 original successor byte slices,
the supplied initial Root, independently expected complete Root context, supplied
safe time and mandatory caller-owned active read set. Every step performs the
same canonical/full-Schema/context/descriptor/policy, consecutive-version and
actual dual-threshold checks. Every submitted signature is verified; no valid
prefix is returned after a later error. Only nonfinal next-Root expiry is allowed,
with original window bounds and issued time still checked. The final Root must
be fresh. Public single-successor verification remains strictly fresh.

Active metadata key preservation is checked only against that final Root. A key
may be absent from an intermediate policy and restored in the final policy.
No active body eligibility or new aggregate byte/active-set cap is added. The
4096 chain count limit is the original protocol rule, not a download limitation.
The caller owns active snapshot consistency/completeness and protected original
chain storage; the operation cannot attest these facts or persist/install Roots.

Private owned transition parts avoid constructing a public fresh-successor proof
for an expired intermediate. Each step releases the previous temporary state.
`SignatureCheckedRootChain` borrows the exact initial Root and retains only final
original envelope bytes, validated final Root, final verified signer IDs, checked
step count and supplied time. It grants no bootstrap, protected custody, high-
water reset, current qualification or execution permission. Full T27 stays open.

Thirty full frozen Node chain cases cover real distinct rotations, original count
bounds including 4096 genuinely consecutive re-signed bodies, expired intermediate
versus final Root, bad middle signatures, final active key retention and original
per-object input/Schema limits. All fifty-five single-successor cases additionally
run both complete frozen operations and both Rust APIs as length-one chains.
Separate API controls retain independent full Root context. Test private keys
remain inside Node; the App facade uses genuine signed positive/negative controls.
The unchanged codec regression and dependency locks retain prior acceptance;
App/CLI graph execution does not constitute a full upgrade workflow E2E.

## Historical Package and Release identities

`verify_archived_package` and `verify_archived_release` verify historical signed
identities using the caller's archived Root and independently expected complete
context. Both require original bounded canonical bytes, complete Schema, the
exact role policy, every actual Ed25519 signature and the configured threshold.
Their separate immutable `ArchivedEnvelopeIdentity` preserves original bytes,
borrows the exact archived Root and exposes verified signer IDs. It is distinct
from ordinary fresh-envelope proofs and grants no current download, activation,
installation, qualification or acknowledgement rights.

Package verification takes supplied safe time. Archived Root expiry alone is
allowed; the original positive bounded window and issued-at check still apply.
Package body semantics always use Production, including the incremental-package
restriction. No development, high-water or denial option is introduced.

Release verification takes no current-time argument. It checks the Root's
original window at that Root's own `issuedAt`, then the Release's original window
at the Release's own `issuedAt`. It adds no cross-object issuance ordering:
an otherwise valid Root issued after the Release or Release issued after Root
expiry can establish historical identity. These are frozen reference semantics,
not current eligibility. Claim-derived times are private bounded values rather
than trusted current clock observations.

Thirty-six complete frozen Node cases cover Package windows and Production (6),
Release original time points (8), actual signatures and thresholds (8), complete
independent context (8) and original wire limits/closed Schema (6). Historical
positive controls additionally check ordinary verification refusal where
applicable. Fresh private keys stay in Node. The App facade exercises eleven
genuine positive/negative cases across both operations; CLI compilation remains
dependency-graph evidence, not workflow E2E. Codec source and all three dependency
locks remain unchanged, retaining their prior regression acceptance.

The caller still owns protected historical Root lineage and archive consistency.
This module neither attests custody/completeness nor specifies service accepted-
data archival, retention or deletion policy. Protected storage and complete T27
integration remain open. No artifact download size, retry or total/cumulative
duration cap is added.

## Complete public metadata chain

`verify_public_chain` composes the frozen complete public-chain operation under
caller-supplied existing Root, independent product/component context, safe time
and global/exact-role denied keys. Every Timestamp, Snapshot and referenced
member uses the unchanged ordinary verifier with fresh Root, Production body
semantics, exact role policy and every submitted signature. Role-specific denial
applies only to that role; unrelated role entries have no effect. There is no
high-water or Development option in this operation.

Timestamp binds the exact original Snapshot reference. That authenticated
Snapshot supplies each member's expected role and complete scope; product and
component always come from the independent caller context. JSON-escaped bounded
values form each complete expected Document. Exact original references and
membership are checked before the unchanged codec's complete public-graph
relationships: Target baseline/ordinary/bridge reachability, Release targets,
Package identity/modes and rejection of unreachable candidate objects. Repeated
Snapshot references still fail the original ordinary metadata semantics. An
oversized actual-member list refuses under the same necessary Snapshot-derived
count bound as the codec, before duplicate scanning. No separate aggregate byte
or member limit is introduced.

The immutable `SignatureCheckedPublicChain` owns checked original Timestamp,
Snapshot and member envelopes, preserving member input order, and borrows the
exact supplied Root with the supplied time. It exposes authenticated original
objects rather than reproducing Node's derived targets/releases/packages arrays.
Private successful ordinary proofs are consumed into owned envelopes; temporary
expected contexts and denial collections do not escape. A failure returns no
partial graph. This neither establishes protected Root/clock custody nor performs
atomic high-water acceptance, current online qualification or execution.

Five tests execute fifty-six full frozen Node cases: complete relationships (34),
denial and isolation (9), actual signatures/thresholds (5), time (3), and exact
policy/wire/Production (5). They include genuinely signed two-Target/two-Package
graphs with independent exact policies and shuffled member order, second-Target
policy/scope negatives, duplicate members/references and ordinary/bridge paths.
Fresh signing keys remain inside Node. One App facade test exercises nine actual
positive/negative controls. Codec, ordinary verification, dependency manifests
and all three lockfiles remain unchanged; prior unaffected regressions retain
acceptance. Actual App/CLI checks remain distinct from workflow E2E. Protected
acceptance and full T27 integration remain open, with no artifact download size,
retry or total/cumulative duration cap.

## Authenticated candidate Release and opening

`verify_candidate_release` composes the complete frozen candidate operation from
original Target/Release bytes, bounded opening, supplied existing Root/safe time,
global denied keys and independent bounded caller context. That context contains
product, component, channel, targetKey and expectedReleaseId. Both fixed-role
expected Documents use the same independent product/component; Target scope uses
channel/targetKey and Release scope uses expectedReleaseId. Incoming claims never
supply the expected Release identity. Context extras are ignored after bounds.

Both envelopes pass the unchanged ordinary verifier with fresh Root, complete
Schema/context, exact policy, every actual signature and body semantics. The
unchanged codec then requires the exact original Release hash to differ from
the Target baseline, checks the complete nonnull seven-field opening commitment
and requires Release target membership. Both authentications precede final
relationship checks; multi-fault diagnostic precedence can differ from Node,
without changing acceptance. Existing Rust opening/policy/Schema labels remain
explicitly mapped in tests. No new high-water, role-denial or Development mode
is added to this original candidate operation.

The immutable `SignatureCheckedCandidateRelease` owns original checked Target
and Release, borrows the exact supplied Root and records supplied time. Temporary
independent expected Documents do not escape consumed private ordinary proofs.
Failure returns no partial pair. This proves supplied signatures/commitment,
not candidate selection, current online eligibility, persistent acceptance,
protected trust/clock custody or execution permission.

Five tests execute thirty-eight full frozen Node cases: opening/baseline/target
membership (13), independent shared context/Release identity (7), actual signature
thresholds/extras/denial (8), windows (4) and exact-policy/wire/Schema (6). Positive
controls include a genuine candidate with empty ordinary/bridge catalogs, restored
opening, context extras, actual two-signer Target and unrelated denied key. Every
opening field, null/extra field, both actual signature paths, independent expected
Release ID and component isolation have negative controls. Membership failure uses
a genuine separately signed ARM Package reference. Private keys remain in Node.
One App test exercises ten genuine positive/negative controls. Codec, ordinary
verification, manifests and all three locks are unchanged; prior unaffected
regressions retain acceptance. App/CLI checks are not workflow E2E. Protected
acceptance and full T27 remain open. No artifact download size, retry or total/
cumulative duration cap is introduced.
