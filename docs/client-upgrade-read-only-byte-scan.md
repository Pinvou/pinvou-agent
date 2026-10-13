# Client upgrade actual read-only cache byte scan

T29 batch 14 scans a typed ReadOnlyCacheHandle by privately try_clone copying its
current actual File, then runs the existing B3 scan on that clone. It never reopens
a path, takes an arbitrary mutable caller File, accepts an old ByteMatch or changes
the original Artifact. Cancellation is checked before duplication, at every B3
seek/read/retry/EOF boundary and once after final hash computation. Clone failures
map to the fixed IO error without a path/provider diagnostic. The local File closes
through RAII on every success/error/cancel exit; the external view and its aliases
retain their own independent lifetime.

The clone inherits read-only permission and original sharing but shares the file
pointer; the owner must coordinate all aliases. B3 starts at zero and reads to real
EOF, with exact original size/SHA and bounded memory. No current metadata length,
response count or selected-prefix claim substitutes for actual bytes. The result
is the existing ByteMatch observation only. It proves no content seal, writer
retirement, namespace/ACL protection, native identity, current qualification or
VerifiedExecutionObject. The owner preserves the original T26 validation budget
through cancellation and runs synchronous IO on a blocking worker; this module
creates no clock, deadline, network request or download cap.

Three new actual Windows groups cover whole original byte match at actual EOF,
shared cursor, wrong original size/hash, cancellation before clone/seek through
after final hash, unchanged bytes and no leftover internal handle. While the
external view lives, a new writer is denied; after view/clone cleanup the same
path opens/writes again, including error/cancel paths. Other OSes explicitly
observe the cache factory's Unsupported and report no physical scan controls
executed; there is no normal File fallback. Existing 26 byte-check/copy/segment
groups and 8 Windows cache groups remain regressions, with the symlink privilege
case still ignored/unverified. An actual App facade matches the readonly file and
rejects a wrong original hash. Native Windows CI now runs the byte crate's real
controls in addition to its existing Linux gate.

Verify adds only a private dependency on the lower cache-object interface, with
no reverse cycle. Both parent locks and the Verify/HTTP/Archive isolated locks
track the direct/transitive local edge; all prior external versions/sources/
checksums/edges and archive backend remain unchanged, and each isolated graph
matches the parent. Final evidence and scoped reviews appear in
[implementation progress](upgrade-implementation-progress.md).

Full T29, protected staging/sealing and actual original clock/budget/qualification
integration remain unfinished. Downloads retain no total/cumulative duration or
retry cap.
