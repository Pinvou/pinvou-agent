# Client upgrade observed cache segment writes

T29 batch 7 writes actual response chunks into the cache owner's already open,
independently protected regular File. FileSegmentWriter holds that mutable handle
and the original validated BodyPlan. A separate mandatory CacheWritePermission
must match the response mode: ReplaceExisting explicitly authorizes set_len(0)
for a 200 response; AppendExisting for 206 requires the current actual length to
equal the original range offset, without truncating it. Type, permission and
cancellation checks precede truncation. A failed start can still leave untrusted
truncated bytes if a later OS seek or cancellation fails. No path is opened,
deleted, promoted or treated as protected through regular metadata alone.

Each write accepts at most 64 KiB, checks cancellation and B1's count before any
actual output, then handles partial writes, Interrupted retries and zero-write
failure. Every write failure latches separately from B1's pre-write count; a
partially written chunk cannot be retried or completed into success by this writer.
The per-call bound imposes no total artifact capacity or elapsed/retry limit.

The owner calls finish_after_network_eof only after the restricted HTTP adapter
reports FileChunk::SegmentFinished from actual EOF. Network failures require
dropping the writer, not completing it merely because accumulated count matches.
The supplied transport receipt must match the original target and segment end.
Finish consumes the writer, requires exact received count and actual file length,
syncs the same handle, then rechecks actual length and cancellation. Short 206
segments report offset plus their own span, not the complete artifact size.
The returned SegmentReceipt is only an observed count; it is not complete hash,
source continuity, immutable cache identity, VerifiedExecutionObject or rights.

All IO is synchronous and belongs on an owner's blocking worker. The owner keeps
physical exclusion and source-continuity evidence across these calls, confirms
safe stop before releasing protection, and later runs the complete B3 hash scan
under the original T26 validation budget. Polling cannot interrupt an active OS
operation. Prefix length does not prove the prefix's authenticity. Equal-length
pollution and changes at already observed positions require that final protected
whole-file verification; this module supplies no universal race defense.

Actual File tests cover explicit replacement permission, original-byte preservation,
short range continuation, mismatched offsets, read-only and append-mode handles,
foreign receipts, visible length changes, every observed cancellation poll,
64 KiB boundaries and a real 33 MiB transfer followed by B3 verification. Private
writer and sync hooks distinguish partial/Interrupted/zero-write, failure after a
real partial output and sync-time mutation from physical fault certification.
Transport receipts in these tests and the App control are structural fixtures;
no real HTTPS-to-cache E2E or complete download workflow is claimed in this batch.
No dependency, lock, CI, product version or gitlink changes are needed.

Final checks and scoped independent review are recorded in
[implementation progress](upgrade-implementation-progress.md). URL renewal,
qualification, protected cache/staging, native authenticity and full T29 remain
unfinished. Downloads have no hard total/cumulative deadline or retry cap.
