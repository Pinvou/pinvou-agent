# Client upgrade supplied Release and Package relationship

T27 batch 21 implements the reference and post-verification relationship subset
of frozen verifyReleasePackage. Inputs are two full-Schema canonical envelopes
and a resource-bounded known target-key Document. Both roles must be the declared
Release/Package roles; four frozen keys define this input domain.

The first matching Release target is selected exactly as in the original find
operation. Its six-field Package reference is checked against the factory's
complete original Package bytes using the existing B10 checker. Signature-record
changes invalidate an old reference even when the signed business body is equal.
A correctly recomputed reference can restore this pure relationship; that does
not make its signature authentic.

After the reference passes, Package target scope and appVersion must equal the
Release target and Release version. The complete launcher identity compares as
bounded JCS, including size, SHA, identity, publisher and format. Every target
activation mode must occur in Package activation modes; an allowed superset or
different order is retained. JCS failures propagate rather than comparing absent
error results. Only the small, already full-Schema checked reference is cloned;
launcher values are borrowed.

This does not perform verifyEnvelope or authenticate the supplied product,
component, target, time or selection authority. Product/component equality is an
independent expected-context obligation, not an added private-helper rule.
Relationship-positive alternate components/products and encoding-valid zero
signatures are explicit controls. The relationship can be tested for all four
keys without certifying actual OS native formats, launchers or installers.
No execution object, current qualification, protected persistence, ownership,
consume outcome or erasure certificate is produced.

Four groups cover all keys, target modes as subsets/supersets and both orders,
each Package binding field with the full reference successfully rebuilt first,
launcher fields, exact complete raw identity, first-target behavior, wrong roles,
target/resource boundaries and context/authenticity scope. The actual Node oracle
runs original assertReference and the exactly extracted private
assertReleasePackageBinding after original complete parser/encoding. It never
substitutes verifyEnvelope or claims equivalence to the whole export.
Self-derived contexts isolate this relationship; B16 independently checks context.
App exercises a legal pair, publisher mismatch after correct reference rebinding,
and an invalid supplied target.

No dependency, lock, frozen Schema, CI, version or gitlink change is required.
Final evidence is recorded in [implementation progress](upgrade-implementation-progress.md).
Complete T27 authenticity, protected persistence and selection/execution composition
remain unfinished. Downloads retain no total/cumulative duration or retry cap.
