# Client upgrade actual read-only cache copy

T29 batch15 connects a typed ReadOnlyCacheHandle to a supplied fresh
ExclusiveCacheFile. The wrapper polls cancellation before privately cloning the
current actual source File, passes the unchanged original Artifact and the same
cancellation callback through B4 copy_and_check, then polls again after final
destination hashing. No path is reopened and no prior ByteMatch is accepted.
The internal clone closes through RAII on every return path. IO errors retain
neither paths nor provider diagnostics.

B4 rejects nonregular or nonempty destinations before seeking or writing. It
starts both handles at zero, bounds each source read and checks original size
before writing, hashes to actual source EOF, syncs the same destination and
rescans that actual destination to real EOF and original size/SHA. The result
is the existing ByteMatch observation of destination bytes, never content
sealing, protected staging, native identity, current qualification or VMO.
Wrong source SHA/short source and later cancellation may leave partial or all
untrusted bytes in the destination; failure neither truncates nor deletes them.
The owner isolates and cleans them. Retrying into a nonempty target is refused.

The typed handles preserve their existing trust boundary: original writers and
explicit aliases may remain, aliases share the source cursor, and trusted mutable
File borrowers can replace or duplicate a handle. Owners must preserve original
objects, coordinate aliases and maintain physical/namespace/ACL protection. This
wrapper does not certify those duties. A blocking worker preserves the original
T26 validation budget through cancellation; no new clock or download cap exists.

Three new actual Windows groups cover ten-byte and64KiB-plus-one-byte success,
wrong original size/SHA, nonempty target bytes/cursors, same-object nonempty
refusal, all13 single-chunk cancellation points through final destination hash,
and both-path open/rename controls after external handles close. Failed output
is inspected as untrusted, including full bytes left by short/hash failure.
Unsupported platforms explicitly report no physical controls executed and use
no ordinary File fallback. An actual App facade tests successful destination
copy and nonempty retry refusal. All B3/B4/readonly regressions remain covered.

No dependency, lock, Schema, product version or gitlink change is needed. Final
evidence appears in [implementation progress](upgrade-implementation-progress.md).
Full T29 and actual protected staging/native identity/qualification integration
remain unfinished. Downloads retain no total/cumulative duration or retry cap.
