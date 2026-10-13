# Client Upgrade V1 signing bytes and descriptors

This second T27 batch extends the private updater codec with fixed signing input
and public descriptor identities. It performs no signature verification or trust
acceptance. No new endpoint, bootstrap anchor, high-water write or installation
right is supplied. Application VERSION remains 0.10.8.

`Document::signing_input` expects the document root to be the complete signed
object. A full envelope is not automatically unpacked. The five required fields
protocolVersion, role, product, component and scope are borrowed exactly as parsed;
the sixth wrapper member signed contains the entire original object, including
unknown fields. Missing required fields refuse; explicit null remains distinct.
Closed schema and expected-context validation belong to the later verifier.

The added wrapper level is checked again, together with string/key and container
bounds. Private fixed-shape serializers write JCS directly to the bounded writer;
an ordinary serde JSON intermediate cannot reject otherwise legal canonical bytes.
The canonical wrapper payload has its original 1 MiB bound. The fixed ASCII domain
`pinvou-upgrade-signed-v1` and one actual NUL are appended separately, so the maximum
complete signing input is 1 MiB plus 25 bytes. Failure exposes no partial input or
raw diagnostics. SigningInput has no Debug or Serialize implementation.

`key_descriptor` requires exactly 43 ASCII base64url characters, a 32-byte decode
and identical unpadded re-encoding. The descriptor contains only the fixed algorithm
ed25519 and canonical publicKey; keyId is lowercase SHA-256 of its complete JCS.
No domain prefix, self-referential keyId or caller-selected algorithm is included.
This is a byte identity, not evidence of a valid curve point, authorized key or
signature. Zero/identity point encodings remain encoding-only inputs here. Their
verification acceptance policy is awaiting an explicit protocol decision; no
unapproved strict-point filter or verifier is registered.

base64 0.22.1 and sha2 0.11.0 were already in the application/CLI graph. Their
existing codec test dependencies become runtime dependencies; each App/CLI lock
adds only the two qualified codec dependency edges. Both complete locked graphs
resolve, with no package-version change or JSON feature change in this batch.

## Verification

The isolated draft initially failed to compile because serde derive was not enabled
and the sha2 output type did not implement LowerHex. Fixed private serializers and
explicit lowercase byte formatting closed those implementation issues without a
new macro dependency. An initial actual Node comparison also failed: the test
oracle inserted an existing key again into its public test Root, triggering duplicate
keyId refusal. The fixture now reuses an existing descriptor instead of duplicating
it; that finding required no production binding change.

The final repository run with actual Node 22.23.3 passed six new binding groups
(5.65s) and all seven existing byte groups (0.36s). The draft had also passed six
groups with Node 26 (18.97s), Node 22 (6.79s) and an independent run (6.25s).
The new controls cover all 20 original signingInputHex values, all four original
public descriptor keyIds, complete content/field binding, missing versus null,
full-envelope refusal, wrapper depth and exact 1 MiB payload/+1 with 25-byte domain
separation. 500,000 binary64 zero values exercise acceptance without a larger
ordinary-JSON intermediate. Canonical base64url controls include padding, whitespace,
non-ASCII, wrong alphabet/length and invalid trailing bits. The actual original Node
signing/descriptor reference is invoked for 44 mixed positive/negative cases.
Public verification fixtures contain no retained private signing material.

Standalone Clippy with warnings denied, App formatting and all 44 CI policy tests
passed (0.247s policy tests). CI installs only the existing frozen reference package
and supplies both original oracle paths. All contract-package inputs trigger the
codec gate, covering the signature reference's transitive schemas/modules/lock.
Actual Windows App integration passed all five updater tests, including the new
binding/descriptor facade control (0.00s, after a 4m02s build with the existing
compiler stack wrapper). Independent actual-repository reruns passed the six new
groups (17.25s), seven original groups (15.77s) and 44 policy tests (0.277s).
The second independent role passed six new groups (6.74s), seven original groups
(0.49s) and 44 policy tests (0.079s) with both actual Node oracles configured.
Architecture and product-version checks passed. Both independent roles gave final
B2 scoped PASS; no mandatory binding/descriptor finding remains.

The previous App-wide run and its existing Windows fixture/packaging limitations
remain disclosed in [the byte codec evidence](client-upgrade-byte-codec.md).
No all-App pass, Ed25519 verification, trusted time, physical platform certification
or service/provider E2E is claimed. Full T27 remains unfinished. Deployment stays
deferred, and downloads have no hard total/cumulative duration limit.
