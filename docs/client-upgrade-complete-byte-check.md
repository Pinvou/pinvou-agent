# Client upgrade complete-byte check

T29 batch 3 adds a private `download/verify` crate and App facade for complete
size/SHA-256 comparison of an already open file. The owner supplies the original
authenticated artifact and keeps the actual object protected. Artifact construction
checks shape only; this module establishes no metadata signature or qualification.

The synchronous function seeks the same mutable file handle to byte zero and reads
to actual EOF using a fixed 64 KiB heap buffer. Each read is checked against the
original signed size before hashing, so excess fails immediately. Exact size alone
does not finish: a following real EOF and matching SHA-256 are both required.
It never reopens a path, trusts a prior stat length or skips the hash. A private
constructor supplies `ByteMatch` only after success and keeps the original target.
The observed match is not a protected file token, durable cache trust,
VerifiedExecutionObject, platform authenticity or installation right. The file
can change later unless its owner maintains independent protection; no snapshot
or concurrent replacement defense is claimed by this scanner.

Cancellation is checked before seeking, each read (including Interrupted retries)
and once again after EOF. IO/seek, short, excess, hash and cancellation failures
produce no match. Fixed errors discard original IO errors and their private path/
source details. The caller runs this synchronous scanner on a blocking worker;
a predicate cannot interrupt a currently executing OS seek/read. The owner must
connect its original T26 validation budget and unknown/fault safe-stop behavior
to cancellation and must confirm safe stop before releasing any protection.
This scanner introduces no elapsed deadline, download duration or retry cap and
does not reset an owner's existing budget. Success leaves the file cursor at EOF.

Tests use actual owned temporary files for cursor reset, size 1 and 64 KiB boundary
neighbors, a 33 MiB file, empty/short/excess input, pre-scan truncate/append and
same-length first/middle/tail pollution, write-only read errors and initial,
post-first-chunk and post-EOF cancellation. Expected hashes are independent Python
hashlib constants, including the published `abc` SHA-256 vector. A crate-private
reader/seek adapter injects seek failure, Interrupted and later IO failure for
scenarios unavailable reliably on regular files; these are unit controls, not
claimed physical E2E or public substitute-file APIs. Cleanup removes only each
test's exclusively created file, without recursive directory deletion.

The existing path-based hashing and legacy full-download helpers remain unchanged.
The private crate reuses existing pinned sha2 0.11.0. CI runs standalone formatting,
locked Clippy and all tests. Actual verification evidence and independent review
are recorded in [implementation progress](upgrade-implementation-progress.md).
Protected cache/staging, safe unpacking, installer/platform authenticity and
current installation rights remain later integration. Full T29 is unfinished.

Final primary validation passed all eight groups (1.89s), standalone fmt/Clippy,
48 CI policies, architecture, version, diff, YAML and complete locked App/CLI
graphs. Actual Windows App integration passed all eleven updater tests (0.01s
after a 3m33s build with the existing compiler stack wrapper). The initial test
assertion incorrectly required Debug on a deliberately opaque Artifact; it was
repaired without changing that public type. An initial isolated lock seed selected
newer transitive versions; self-review replaced it with the complete App lock and
pruned offline. All 12 isolated packages now exactly match App versions/sources/
checksums. Each parent lock adds only nine lines for the local package and App
edge; no existing external package changes. The eight groups and Clippy passed
again after the lock correction. No App-wide, race-defense or physical-platform
certification pass is claimed.

Both independent roles gave final B3 scoped PASS after reviewing the final locked
graph/App evidence and documentation; no mandatory finding remains. Their actual
eight-group runs preceded the isolated-lock correction; both separately verified
the final lock, and the primary role reran the tests/Clippy on that lock.
Full T29, protected
staging and deployment remain unfinished.
