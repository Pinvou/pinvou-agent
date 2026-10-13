# Client upgrade supplied candidate opening relationship

T27 batch 22 implements the three relationship guards from frozen
verifyCandidateRelease. Target and Release are full-Schema canonical envelopes;
opening is an independently bounded Document. Their exact roles are checked
within this defined input domain.

SHA-256 of the factory's complete original Release bytes, including signatures,
must differ from the Target's declared baseline envelope hash. The existing B11
checker then binds Target scope, that exact Release hash and the full closed
seven-field opening/commitment. Release targets must contain the Target key.
The Release hash is constructed internally as a bounded scalar; the Target's
already validated signed Document is borrowed. No large business subtree is
cloned and no generic unchecked public Document constructor is exposed.

This does not perform verifyEnvelope or authenticate independent product,
component, channel, release ID, trusted time, current membership or eligibility.
It adds no deployment/rollout epoch or signature constraint beyond the original
guards. Success cannot select or download a candidate, create an execution
object, update high water, consume permission or certify erasure. Correctly
rebound different-context objects, encoding-valid zero signatures, zero leaf
salt and Schema-valid reversed time are deliberate pure-relationship controls.

Four test groups cover all four keys, baseline refusal, all seven opening fields,
closed opening/duplicates/resources, wrong roles, changed complete Release
signatures, context/time scope and target membership. A missing-target negative
first updates the opening to the new complete Release hash and recomputes the
Target commitment, independently proves B11 opening success and baseline
inequality, then requires the precise missing-target error. This prevents earlier
opening refusal from masking omitted target membership validation.

The actual Node oracle extracts the exact three original statements and invokes
original assertOpening and sha256 after original full parser/signature encoding,
using the real bindings/opening Schema. It never replaces verifyEnvelope or
claims equivalence to the full export. Self-derived contexts isolate these three
relationships; B16 covers independent context separately. App checks a coherent
candidate, its baseline counterpart and a null opening.

No dependency, lock, frozen Schema, CI, version or gitlink changes are required.
Final evidence is recorded in [implementation progress](upgrade-implementation-progress.md).
Complete T27 authenticity/protected persistence/current selection remains
unfinished. Downloads retain no total/cumulative duration or retry cap.
