# Client upgrade supplied Release renewal

T27 batch 20 implements frozen assertReleaseRenewal over two private-factory
SchemaCheckedEnvelope values. Both signed roles must be release, and the next
positive SAFE revision must be strictly higher. A jump is allowed; this helper
adds no requirement for consecutive revisions.

A private borrowed serializer excludes exactly the three root signed properties
revision, issuedAt and expiresAt. Every other signed property, including product,
component, complete scope, version, migration/backup policy and every target or
nested reference, compares as whole bounded JCS. Nested properties named expiresAt
are retained. Object member order and equivalent JSON number spelling follow JCS;
array order remains significant. Canonicalization errors propagate.

The outer signature records are not Release business properties. An encoding-valid
signature change does not affect this comparison, while full envelope encoding
and frozen Schema still precede both inputs. This operation adds no credential
or metadata clock check. Schema-valid reversed issuedAt/expiresAt therefore stays
a positive relation control; independent metadata semantics must reject it where
appropriate. Callers still establish signature/Root/history/context/time and
current publication qualification outside this helper.

Success is a provided-input relationship only. It neither advances protected
high water nor replaces, publishes or renews stored metadata. Publication still
must atomically renew references and rebind affected openings/generations through
the responsible service owners. No new trust, ownership, execution, consumption
or secure-erasure certificate is created.

Four test groups cover revision jumps, equality/regression, SAFE endpoints,
every signed business leaf, nested reference changes, two-target array ordering,
both wrong-role directions, complete Schema/encoding, escaped duplicates,
changed signatures and the three excluded fields. The actual Node oracle invokes
the unchanged original assertReleaseRenewal export, with original full parser
and signature encoding checks before it. Self-derived expected contexts isolate
this relationship only, without a signature-verification substitute. App checks
a revision jump, reversed-time relation, changed appVersion and equal revision.

No dependency, lock, frozen Schema, CI, version or gitlink changes are required.
Final evidence is recorded in [implementation progress](upgrade-implementation-progress.md).
Complete T27 authenticity/protected persistence and publication composition remain
unfinished. Downloads retain no total/cumulative duration or retry cap.
