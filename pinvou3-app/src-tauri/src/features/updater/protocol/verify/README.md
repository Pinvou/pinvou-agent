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
below. Complete chains and archive operations remain unimplemented.

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
qualification or execution rights. Full T27 and protected Root chains remain open.

Fifty-five fresh full frozen Node successor cases cover dual thresholds and extras,
continuity and safe-integer edges, both windows, active key retention, original
input limits and Schema-valid descriptor/policy inconsistencies. A separate API
test checks mandatory independent complete Root context. The 4097 repeated-entry case verifies absence of an invented set
cap. Private test keys never leave the Node process. Existing ordinary 111-case
oracles remain separate regressions. The App facade exercises both Root policies
and active key retention; actual CLI graph compilation is not workflow E2E.
