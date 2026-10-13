# Client upgrade observed byte-copy verification

T29 batch 4 extends the private verify crate with copy-and-rescan support for two
already open regular File handles and a narrow App facade. The owner supplies an
authentic target, independently protected source and exclusively owned fresh
destination. File metadata regularity and empty length are structural filters;
they do not establish that protection, current qualification or immutable identity.

Nonempty destinations are refused before any write. The function never truncates,
deletes, reopens or promotes a path. It seeks both handles to zero, uses B3's
fixed-buffer scanner to count the original source bytes before writing a chunk,
and validates the whole copied string's size/hash at real source EOF. It does not
accept a previous ByteMatch instead of reading the actual source again. Partial
writes are handled exactly, including Interrupted retry, zero-write failure and
cancellation before every write attempt. After source success it checks cancellation,
syncs the actual destination, checks cancellation again and uses B3 to scan that
same destination handle from zero through real EOF. Only the complete destination
size/hash and final cancellation check can produce another observed ByteMatch.

Failure can leave untrusted partial or complete destination bytes; the owner keeps
them isolated and cleans up safely after stopping the writer. No rollback/delete
path is injected here. The result proves only the byte strings observed during
this operation, not unchanged contents afterward. Source changes before copying
or target pollution before its rescan are detected when the actual read bytes
deviate from the target; changes to already scanned positions cannot generally be
detected. Shared clone cursor state and concurrent writes require owner protection.
This function is not protected staging or VerifiedExecutionObject construction.

The synchronous copy/sync/rescan runs on a blocking worker. The original T26
validation budget covers the whole operation; callers must not restart its origin
for a second scanner. The cancellation predicate cannot interrupt an active OS
read/write/seek/sync. Unknown/fault safe stop remains the owner's responsibility.
No download deadline, cumulative duration or retry limit is introduced.

Seven new groups plus the eight B3 groups exercise real 65,537-byte and 33 MiB
copy/rescan, cursor reset, nonempty destination preservation, a clone of the same
nonempty file, source pollution after an old match, actual read-only/write-only
handle errors, and cancellation before writing, after source EOF, before/after
sync and after destination EOF. Crate-private callbacks test sync failure and
visible pre-rescan pollution; private writers test partial/EINTR/zero-write and
between-write cancellation. These unit controls do not certify physical fault or
concurrent replacement defenses. No public test hook exists. The existing CI gate
covers the new Rust modules; dependencies, locks and CI policy do not change.

Actual validation and independent review are recorded in
[implementation progress](upgrade-implementation-progress.md). Physical protection,
safe unpacking, final installer/platform authenticity and current rights remain
subsequent integration; full T29 and deployment remain unfinished.

Primary final validation passed all fifteen groups (3.61s), standalone fmt/Clippy,
48 CI policies, architecture, product-version and diff checks. Actual Windows
App integration passed all eleven updater tests, including the extended actual
copy facade control (0.01s after a 1m37s incremental build with the existing
compiler stack wrapper). No App-wide pass is claimed. No dependency, Cargo lock,
workflow, product version or submodule changes were made in this batch.
Both independent roles gave final B4 scoped PASS after reviewing frozen source,
actual App results and documentation. Each independently passed all fifteen
groups (3.47s and 3.52s respectively), plus fmt/Clippy; no mandatory finding remains.
