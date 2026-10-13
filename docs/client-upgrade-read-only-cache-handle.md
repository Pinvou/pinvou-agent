# Client upgrade read-only cache handle duplicate

T29 batch 13 borrows the current actual File of the trusted ExclusiveCacheFile
and calls Windows DuplicateHandle from the current process to itself. Desired
access is exactly FILE_GENERIC_READ, inheritance false and flags zero, without
DUPLICATE_SAME_ACCESS or CLOSE_SOURCE. No path is reopened, no original handle
closed/replaced and no second file created. A successful new handle is taken into
one File RAII owner exactly once; failures expose only a fixed error. The same-
handle observation FFI is factored without changing the fresh-object validator.
Other OS implementations explicitly return Unsupported.

ReadOnlyCacheHandle has private construction, no Debug/Serialize/Clone or ordinary
File constructor, and exposes only &File. This protects its own data/length/time
write permissions, not content immutability or retirement of other writers. The
original writer and prior trusted writer duplicates can still change bytes; owners
must separately retire/account for all such handles before any sealing assertion.
Explicit try_clone preserves these read-only rights and can extend object sharing
lifetime. Raw handles remain trusted caller responsibility. DuplicateHandle shares
file pointer with the source and other duplicates; Read/Seek through &File changes
that shared cursor. Owners must serialize all uses. This is not an independent
snapshot, immutable cursor, ACL/namespace protection, native identity, current
qualification, VerifiedExecutionObject or installation rights.

Actual Windows tests compare original/duplicate volume and file index, read/seek
positive controls and shared cursor, data write/set_len/set_times refusal with
unchanged bytes, then prove the original writer still succeeds and its changes
are visible through the reader. A separate positive/control sequence proves read-
only duplicates retain original share0 rules after the source closes, an explicit
reader clone retains them after its wrapper closes, and same-path read/write/
rename/delete succeed after every related handle closes. Existing six fresh-file/
ADS controls remain regressions. The privileged symlink test remains explicitly
ignored/unverified; no permission escalation or deployment is attempted.

The App facade reads the actual duplicate, rejects write/truncate and checks
retained sharing/release through the real dependency graph. Only the already-used
windows-sys Threading feature is enabled in the isolated package; all dependency
versions/sources/checksums and locks remain unchanged. Existing Linux unsupported
and native Windows default CI gates include the new code/tests. Final evidence
and scoped reviews appear in
[implementation progress](upgrade-implementation-progress.md).

Full T29, sealed content/retired-writer proof, protected staging/namespace, native
identity and actual original clock/budget integration remain unfinished. Downloads
retain no total/cumulative duration or retry cap.
