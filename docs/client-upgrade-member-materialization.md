# Client upgrade observed member materialization

T29 batch 6 streams one current regular ZIP member into an already open fresh
destination supplied and independently protected by its owner. The owner supplies
the original authenticated package and an independent name/size/hash expectation.
The ExpectedMember constructor checks portable locator shape, safe size and a
32-byte digest only; it does not authenticate native claims. Empty data files are
supported without fabricating a positive-size download Artifact. The API does not
accept an old inventory or reopen/create/delete/truncate/promote any path.

Before any write, it rejects nonregular or nonempty destinations, completes B5's
whole-package inventory scan, and compares the exact selected regular member to
the expectation. It reparses the current raw physical records, then reads the
current selected member again through the bounded Store/Deflate scanner. Actual
size is checked before each output write; partial/Interrupted/zero-write and
cancellation handling preserve exact output. Source entry EOF, CRC, SHA-256 and
full compressed-span consumption are required again; the old observation cannot
replace that second byte scan. The entire original package is hashed once more
after copying to catch visible container changes.

After cancellation and sync_all checks, it seeks the same destination handle to
zero and reads the ordinary decoded file to real EOF, comparing its entire size,
hash and CRC. Source ZIP offsets or compressed lengths never limit that scan.
An empty destination still requires real EOF and SHA-256 of the empty string.
Only the final successful scan/cancellation check produces MaterializedMember's
private constructor, which retains the original package and observed member.

This result is not a file lock, durable cache/staging trust, native signature,
VerifiedExecutionObject or execution permission. Regular metadata and zero length
do not establish exclusive ownership. The owner protects both actual objects
throughout and afterward, preserves its original T26 validation budget over all
scans/copy/sync, and confirms safe stop before releasing protection. Cancellation
cannot interrupt a running OS operation or bounded codec step. No total/cumulative
download deadline or retry cap is introduced.

Failure can leave untrusted partial or complete destination bytes for owner
isolation/cleanup. Visible source changes after the initial scan and destination
pollution before final rescan are refused when read facts deviate; no universal
transient or already-scanned-position race defense is claimed. Fixed errors retain
no raw member names or underlying IO/codec sources. This module does not construct
paths or directories, select native claims from unsigned data, mount/execute an
installer or certify protected extraction. Those integrations remain unfinished.

Seven new groups plus B5's three groups exercise real Store/Deflate/descriptors,
33 MiB output and empty files, zero-write binding/directory/nonempty/alias refusals,
actual read/write permission failures, every observed Store/Deflate cancellation
poll, late entry corruption and visible post-inspection source changes. Private
sync callbacks and writer controls inject sync failure, pre-rescan destination
pollution, partial/Interrupted/zero-write and between-write cancellation; these
are unit fault controls, not physical failure certification. The additional empty
file vector brings the independent Python ZIP oracle to 49 inputs. No dependency,
lock or CI policy changes are needed; the existing gate covers this delta.

Actual checks and independent review are recorded in
[implementation progress](upgrade-implementation-progress.md). Full T29, native
authenticity, physical protection, execution rights and deployment remain open.
