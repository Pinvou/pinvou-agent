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

## Signed Package and supplied Release binding

`verify_release_package` implements the complete frozen Package operation using
supplied full-Schema canonical Release material, original Package bytes, supplied
existing Root/time/global denial and independent bounded product/component/
targetKey context. It finds the Release target and checks all six fields of its
reference against the exact original Package bytes before Package authentication.
The unchanged ordinary verifier checks fresh Root, exact Package policy, all
actual signatures, independent context and Production. The unchanged codec then
binds appVersion, launcher, target and activation-mode subset relationships.

This original operation authenticates only Package. It does not authenticate
supplied Release or check that object's product/component/context/time. Callers
must independently authenticate Release before any qualification. Accordingly,
the immutable `SignatureCheckedPackageBinding` owns `checked_package`, borrows
the exact `supplied_release` and supplied Root, and records supplied time. The
supplied Release getter remains Schema material, not a signature proof. No
partial proof, persistent acceptance, protected custody, current online rights or
execution authority is returned. Package here is signed metadata rather than
proof of downloaded artifact bytes. No high-water, role-denial or Development
option is introduced.

Five tests execute thirty-five complete frozen Node cases: references/business
binding (13), supplied Release boundaries (4), independent Package context (3),
actual signatures/thresholds/denial (8), and windows/policy/wire/Production (7).
Ten positive controls include mode ordering/subsets, foreign product/component,
expired or invalid-signature supplied Release, context extras, actual two-signer
Package and irrelevant denial. All supplied Release inputs independently pass
the full Schema factory. These positives establish only Package authentication
and supplied-value relationships, never Release authenticity or current rights.
All reference fields and actual Package signatures have negative controls.
Existing closed-reference/Schema/policy error labels are mapped explicitly;
fresh private keys remain in Node. One App test exercises ten genuine controls.
Codec, ordinary verification, manifests and all three locks are unchanged, so
prior unaffected regressions retain acceptance. App/CLI checks are not workflow
E2E; protected acceptance and full T27 remain open. No artifact download size,
retry or total/cumulative duration cap is added.

## Authenticated successful Check/Refresh response correlation

`verify_check_response` combines unchanged complete frozen request/response
contracts and actual ordinary verification of both new Decision and Telemetry
credentials with normative request-fact comparisons. Its operation selects only
a successful Check or Refresh body. Error/unactivated transport classification
remains separate. Original response bytes use the 1 MiB metadata profile;
embedded objects receive canonical identities and retain the original 64 KiB
credential limit. Signature arrays retain their original order and uniqueness.
Transport whitespace/key order is allowed; no raw-envelope substring is invented.

The independent original bounded request supplies product/component and complete
installation/channel/revision/target scope, nonce, installId, host, four fact
digests, currentVersion and the sourceProfileId derived by the unchanged stable
facts projection. Stable product/component/target must agree with request.scope.
Both new credentials require fresh supplied Root, exact role policies, all actual
signatures, global denial and current supplied time. Complete existing response
pair/session bindings also apply. These are supplied facts, not native probes.

The old inline Refresh request Decision keeps its original contract semantics;
it is not newly authenticated or checked against current time/deny/Root policy.
A valid expired prior decision can accompany a newly authenticated response.
This preserves long downloads without extending old credential windows. When the
new refreshed Decision offers an update, its workflowId must match the request.
Response workflowRevision is not forced to old+1; writer registration, immutable
workflow continuation and CAS remain later owner checks.

The immutable `SignatureCheckedCheckResponse` owns canonical checked credentials
and exact original transport bytes, borrows the exact original request/Root and
records operation/time. It exposes no partial proof, protected custody, metadata
acceptance, Registry/generation qualification, current session acceptance,
selection or execution rights. Authenticated metadata, protected facts and owner
state must still be checked before any usable client projection.

Seven tests run sixty-two composed Node controls (12 positives): successful/
long-download boundaries (8), independent facts/workflow (15), complete expected
context (6), credential pair/session (6), actual signatures/thresholds/deny (12),
current windows (8), and contracts/wire/resource bounds (7). The oracle uses
complete frozen `assertApiContract`, actual `verifyEnvelope` twice and original
`sourceProfileId`, plus explicitly new normative request comparisons; it is not
an unchanged frozen full-response verifier. Tests cover both credentials at exp
and one millisecond before, bad last signatures after a valid threshold, expired
or invalid-signature old Refresh inputs, independent workflowRevision, and exact
original bytes/canonical credential identities. Private keys stay in Node. One
App facade test exercises twelve genuine controls. Codec, ordinary verifier,
manifests and three locks remain unchanged. App/CLI compilation is not workflow
E2E; full T27 and protected acceptance remain open. No artifact download size,
retry or total/cumulative duration limit is introduced.

## Signed DownloadInfo request/response inheritance

`verify_download_info_response` composes the unchanged complete DownloadInfo
request/response contracts and actual ordinary authentication of both the current
request Decision and new Download credential. Independent original request.scope
supplies the complete expected product/component/installation/channel/revision/
target identity. Each role independently requires the supplied fresh Root, exact
policy, every actual signature, global denial and current supplied time. Inline
envelopes receive canonical identities with existing bounds and signature array
order preserved; exact original response bytes remain available separately.

Request Decision must offer an update for the requested actual-hop package. All
nineteen shared qualification fields, including complete metadataSet, forwardPath
and update, must match the new Download claims. The credential's package must
match the request and its own signed actual-hop package. Role-specific type,
audience, purpose, jti, signing key and windows deliberately remain independent.
The existing response contract binds packageId/size/hash and URL window to the
credential; this verifier additionally requires the URL be current at supplied
now. It does not invent signed downloadKey or HTTP requestId correlation.

Unlike Refresh's historical request input, a file eligibility request requires a
currently valid Decision. Expired Decision or URL requires rechecking/refreshing
and a fresh short-lived URL, never terminating a logical download, clearing its
cache or imposing a total/retry/cumulative duration limit. The immutable proof
owns both canonical checked envelopes and original response bytes, borrows exact
request/Root and records time. Native facts, metadata/Registry/generation
acceptance, active workflow/session, destination approval and downloaded bytes
still require their respective protected owners and T29 checks. No selection,
fetch, installation or execution rights follow merely from this proof.

Seven test groups cover sixty-one composed Node cases (nine positives): general
success five, inherited qualification sixteen, independent context seven,
package/outer binding seven, signatures twelve, windows eight and wire six.
They compose complete frozen assertApiContract, actual verifyEnvelope
twice and explicitly normative inheritance/URL-now comparisons. This is not an
unchanged frozen full-operation oracle. Tests include every shared qualification
field, complete scopes and update, current Decision/no-update/wrong package,
distinct role keys/jti, both actual thresholds, bad last signatures/duplicates,
key denial/exact role policy, URL and credential expiry/last valid millisecond,
closed contracts and resource/wire limits. Original response bytes, canonical
envelope identities and exact borrowed objects are checked. Private keys stay in
Node. One narrow App facade test reuses twelve genuine controls. Codec/ordinary/manifests/locks
remain unchanged; prior unaffected regressions retain acceptance. Actual App/CLI
builds are not workflow E2E; full T27 and physical qualification remain open.

## Authenticated immutable Release renewal

`verify_release_renewal` composes three unchanged complete operations:
`verify_archived_release`, ordinary next Release verification, and codec
`assert_supplied_release_renewal`. Both original envelope inputs use fixed
Release role and independent bounded product/component/releaseId identity.
Historical and current trust are separately supplied Root objects.

Previous is a historical identity: each original Root/Release window is checked
at that object's own issuedAt, with full Schema, context, all actual signatures
and exact archived policy. No current-time or retroactive current-deny check is
added. Next requires fresh supplied current Root/time, actual ordinary signatures
and global denial. Complete business equality excludes exactly revision,
issuedAt and expiresAt. Revision strictly increases; gaps are allowed, and no
cross-object issuance ordering or signature-array equality is invented.

The immutable result retains distinct `ArchivedEnvelopeIdentity` and
`SignatureCheckedEnvelope` wrappers, original bytes, signer identities and exact
separately borrowed Roots. The current wrapper's owned parts are safely rebound
to the exact input Root; a temporary expected-context borrow cannot escape.
No partial proof, archive custody/completeness, Root rotation lineage,
high-water/cache replacement, complete public/private publication, generation
acceptance, selection or execution permission is established. Those remain
protected owner responsibilities. This does not choose archival retention policy.

Six tests cover forty-seven composed controls (nine positives): renewal/revision
eight, immutable business seven, context six, signatures twelve, windows seven
and wire seven. They compose the three unchanged original Node functions, rather than
claiming a nonexistent frozen full-renewal helper. Genuine controls cover expired
history, unchanged Root/key or separately supplied current Release signing policy, revision gaps/safe bound,
immutable business/context/scope changes, both actual thresholds/bad extras/
duplicates/exact role policies, current denial without retroactively denying
history, original own-issued-time behavior without invented cross ordering,
current windows, canonical/full Schema/resource bounds, original proof bytes and
distinct Root pointers. Private keys remain in Node. One App facade test reuses twelve
genuine controls. Ordinary/archive/codec implementations, manifests and locks
remain unchanged. Actual App/CLI compilation is not workflow E2E; full T27 and
physical/deployment qualification remain open. No artifact size, retry or total/
cumulative download duration cap is introduced.
