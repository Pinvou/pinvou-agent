# Client upgrade single attempt and complete cache scan

T29 batch 11 composes the actual restricted single GET/cache attempt with the
existing actual-file whole-byte scan. Original target, explicit replacement
permission, cancellation, response/write/wait observations and blocking-worker/
actively-driven runtime rules remain unchanged. A short segment returns Partial;
412/416 returns Restart. Neither starts validation or yields a byte match. A segment
enters whole-file validation only if its next offset equals the original target
size. B1's complete-cache VerifyExisting branch also requires a fresh actual scan;
no cached receipt or claimed length substitutes for current file bytes.

A mandatory trusted CacheValidationOwner begins or resumes the original workflow/
transaction/target's validation record, then polls under its original T26 budget.
It must not mint another deadline when a session/URL/request changes. The interface
provides no clock, protected record store or budget certification; the actual owner
must stop on expired/unproven/time-fault evidence and preserve its original record.
Begin refusal occurs before scan seek/read. During the same File scan, the first
fixed owner stop reason is latched and cannot be erased by later Continue. Existing
B3 checks before seek/read/retry and after real EOF poll cancellation and owner
state; another check after final size/hash computation prevents a late stop from
returning a byte match. Cancellation is checked again after the final owner
callback, which may itself publish cancellation while returning Continue. The
scan callback also rechecks cancellation after each owner Continue before any
seek/read proceeds. Blocking OS reads/callbacks cannot be interrupted mid-call;
callbacks must return promptly. Validation is separate from ongoing downloads.

Only an actual same-handle scan from zero through real EOF with exact original
size/SHA returns Complete(ByteMatch). Equal response count alone does not succeed.
A failed transfer/validation/hash never falls back to a checked Segment success.
Errors retain only fixed codes, not paths, URLs, raw bodies or provider errors.
Failures may leave untrusted actual output for caller isolation/cleanup. No path
is reopened, promoted or deleted. ByteMatch still attests only observed bytes,
without native identity, exclusive physical protection, current authorization,
VerifiedExecutionObject or installation rights.

Real HTTPS fixture tests cover 200 replacement, explicit refusal, complete 206,
short206, restart preservation, equal-length prefix pollution, wrong original hash,
network/truncation failures and response/progress owner Stop. Complete local cache
checks reject actual truncation/excess/corruption without URL/DNS. Scripted owner
controls cover every fixed begin stop and each scan/final stop point, cancellation
at every scan/final owner callback, including before seek and after EOF,
unchanged original deadline and no revival; these scripts do not certify
actual trustworthy time or protected persistence. The App facade checks an actual
file match and rejects truncation after the complete-length projection. Final
checks and scoped reviews appear in
[implementation progress](upgrade-implementation-progress.md).

No dependency, lock, protocol Schema, CI, product version or gitlink changes.
Full T29, protected cache/staging, native authenticity, URL/qualification renewal,
durable resume and actual original-budget/clock integration remain unfinished.
Downloads retain no total/cumulative duration or retry cap.
