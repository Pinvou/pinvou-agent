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
Root returns `RootTransitionRequired`; successor/chain and archive verification
are separate operations, not implemented by this crate.

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
