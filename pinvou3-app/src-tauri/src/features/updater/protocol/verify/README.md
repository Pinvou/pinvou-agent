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
below. Complete chains use their own operation; archive identities remain
unimplemented.

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
