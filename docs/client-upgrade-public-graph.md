# Client upgrade supplied public metadata graph

T27 batch 23 checks the supplied context, complete references and reachability
relationships from frozen verifyPublicChain. Inputs are full-Schema canonical
Timestamp/Snapshot/member factories and a separately bounded product/component
context. Context extras are ignored after complete raw resource/duplicate checks.
Existing B16 enforces full expected role/scope/product/component relationships.

Timestamp binds the complete original Snapshot reference. Each member's exact
six-field original-byte reference is derived once using B10; immutable factories
permit this local calculation cache without a time-of-check/use byte change.
References retain full scopes, versions, expiry, size and original raw SHA.
Duplicate actual member hashes fail. Repeated identical Snapshot references keep
the original Map overwrite effect, with no added duplicate-reference rejection.
Actual and referenced member cardinalities must agree.

All public Targets contribute baseline, ordinary and bridge Release references.
Each must name an actual Release that contains that Target key and matches the
full reference. Every concrete target of each reachable Release must name its
actual Package and pass the original four Package relationships. B21's unchanged
target-specific private helper is shared; the graph never replaces this loop
with a first-by-key search. Every member must be reachable through these public
paths and at least one Target must exist. Unreachable candidate objects fail.

Members cannot outnumber Snapshot entries in any successful original graph.
This necessary Snapshot-derived bound is applied before index allocation, and
the byte facade applies it before decoding member factories. Indexes, permitted
sets and unique reachable Release sets therefore stay within the frozen 4096
entry limit. References are cached once, so repeated paths do not rehash entire
Release/Package envelopes or create a growing duplicate Release vector. Immutable
pure repeated checks have the same acceptance result when each unique reachable
Release is checked once. No independent aggregate byte or duration cap is added.

Success returns unit relationships only, not a Trusted graph or execution object.
This does not perform verifyEnvelope, authenticate supplied context/Root/time,
validate metadata semantics, grant current eligibility, persist high water,
publish, consume or certify erasure. Fixed errors do not expose untrusted data;
equivalence concerns acceptance, not identical failure ordering.

Four groups cover complete references/context, every concrete target including
Schema-only duplicate-key targets, repeated-reference Map behavior and the exact
4096/4097 boundary. These Schema-only controls are separate from B17 metadata
semantics, which rejects duplicate reference/target identities where required.
The actual Node oracle generates strong fresh signatures in memory and calls the
unchanged verifyPublicChain export. Before any graph outcome, each exact input
independently passes real verifyEnvelope with actual Root role/scope policy and
fixture time. Rust comparison composes full Schema, B17 semantics and this graph
checker only within that freshly authorized fixture domain. It implements no Rust
crypto and claims no general full-export equivalence. Private signing keys are
never serialized, logged or committed. App checks complete graph, missing member,
wrong supplied component and Snapshot-derived excess member count.

No dependency, lock, frozen Schema, CI, version or gitlink changes are required.
Final evidence is recorded in [implementation progress](upgrade-implementation-progress.md).
Complete T27 authenticity/protected persistence/current selection stays unfinished.
Downloads retain no total/cumulative duration or retry cap.
