# Client upgrade Windows fresh cache object primitive

T29 batch 12 atomically creates one new Windows cache file with read/write access,
create_new, share mode zero and OPEN_REPARSE_POINT. It never first creates then
reopens a path. Before any OS call it rejects colon-bearing components except
parsed disk/verbatim-disk prefixes, preventing NTFS alternate data streams from
being treated as fresh physical files. Existing final entries fail without truncation/deletion. The same
actual handle supplies file attributes, size and link count through one narrow
GetFileInformationByHandle FFI call. Directory/reparse/nonempty/multiple-link
observations fail; no result is constructed. Failure after creation closes that
handle but may leave an untrusted new entry for owner cleanup. No unprotected path
is automatically deleted. Other OS adapters explicitly return Unsupported without
creating a file; this is not a downgrade in approved platform scope or certification.

The private ExclusiveCacheFile owns that File, has no Debug/Serialize/Clone or
ordinary-File constructor, and exposes a mutable borrow for trusted transfer code.
Windows sharing rejects later ordinary data read/write opens and delete/rename
access to that object. It is no ACL, metadata-access denial, administrator barrier,
parent directory/namespace protection or permanent single-link guarantee. The owner
still protects path roots/ancestors/ACL, lifecycle and all trusted handle uses.
Explicit try_clone/handle duplication can retain access and extend sharing
restrictions after the wrapper drops; all such handles must close for release.
A trusted mutable File borrower can also explicitly replace the handle. The owner
must preserve the original handle and account for all duplicates; the wrapper is
not an unforgeable protection certificate for arbitrary caller misuse.
The handle remains writable, not sealed/read-only. This is no native authenticity,
VerifiedExecutionObject, current qualification or installation-rights claim.

Default actual Windows tests exercise second data read/write open, delete/rename
refusal and same-operation positive controls after close; explicit duplicate
lifetime; alternate-stream refusal with unchanged base bytes and no created stream;
valid drive-prefix and invalid stream-syntax controls; unchanged existing regular/
hardlink/directory entries; and same-handle
checker rejection of an actual multiple-link empty file, nonempty file and directory.
These checker tests directly use ordinary test handles to reach branches that
create_new itself refuses before observation. A separate ignored opt-in test
requires Windows symlink privilege and checks final symlink creation refusal and
same-handle reparse rejection. The current host denied setup with OS1314; this
physical symlink case remains unverified, not a production failure or silent pass.
It must run on an eligible certification host. No elevation or deployment is added.

The isolated package pins existing windows-sys0.61.2/windows-link0.2.1; its three-
package graph exactly matches the parent lock. App/CLI locks add only this private
package/edge, preserving all prior versions/sources/checksums. Linux CI tests the
explicit unsupported adapter; native Windows regression runs the real default
primitive tests. The opt-in symlink test is never silently counted as executed.
An App facade checks actual sharing exclusion and release, or explicit unsupported.
Final tests/reviews are recorded in
[implementation progress](upgrade-implementation-progress.md).

Full T29, protected namespace/staging/readonly handoff, native identity, original
trusted clock/budgets and platform certification remain unfinished. No Schema,
product version or gitlink changes. Downloads retain no total/cumulative duration
or retry cap.
