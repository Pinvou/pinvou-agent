# Client upgrade actual HTTPS-to-cache receive

T29 batch 8 composes the restricted FileBody with B7's actual FileSegmentWriter.
The receiver consumes that actual body, derives its sole plan/target from it and
requires the owner's explicit cache write permission. Data chunks go to the same
borrowed protected file; only FileChunk::SegmentFinished from actual HTTP EOF can
supply the receipt that completes the writer. No second plan or target can replace
the response binding. Any network, file, permission or cancellation failure drops
the consumed body and returns no receipt, retaining untrusted cache bytes for its
owner. Fixed errors expose neither a URL nor an underlying SDK/IO error source.

The synchronous receiver runs on an independent blocking worker with a live
Tokio Handle whose runtime actively drives IO/timers. It must never run within an
async executor poll: nested block_on can panic. For a current-thread runtime the
owner must continue Runtime::block_on, for example by awaiting spawn_blocking;
Handle::block_on alone cannot drive that runtime's IO. The receiver creates no
runtime/thread and establishes no physical owner or safe-stop guarantee. The
owner retains file protection and confirms stopped workers before releasing it.

While awaiting each chunk, it pins the same next_chunk future and selects between
that future and 100 ms cancellation polling. A false cancellation check preserves
the pending operation; a true check drops it and the body. This interval only
supports observed cancellation, subject to runtime scheduling and completion of
an active future poll; it introduces no total or cumulative download deadline,
retry cap or additional read-idle deadline. Actual file OS operations still cannot
be interrupted mid-call by the predicate. Complete protected B3 byte validation
runs later under the original T26 validation budget.

The isolated HTTPS fixture exercises current-thread runtime IO driven while the
receiver runs on a blocking worker. Actual files cover rejected replacement
permission without modifying old cache bytes, 200 replacement, short 206 then
range continuation, complete B3 verification, network truncation/excess/disconnect,
actual read-only output, wrong actual prefix length, cancellation during a pending
read against a peer that never sends data or EOF, a false-cancel control
waiting for the existing fixture read-idle limit, continuing progress and a 131,073-byte response split into bounded writes.
The synthetic large-response request deliberately has a different expected digest:
its count succeeds but B3 rejects the complete hash, proving the distinction.
Fixture TLS keys stay in memory; no test trust root becomes a production API.
This is real local HTTPS-to-file interoperability, not physical cache protection,
file-provider qualification, an actual platform workflow or deployment acceptance.

The HTTP crate gains a local dependency on the existing verify crate; dependency
direction is HTTP composition to verify/transfer, with no network/Tokio dependency
in verify. Parent App/CLI locks add only that local edge, without external version
changes. The isolated graph is seeded from and checked against the parent graph.
The App facade exposes the receiver to later owner orchestration. Final checks
and scoped review are recorded in
[implementation progress](upgrade-implementation-progress.md). URL renewal,
qualification, protected staging, native authenticity/current rights and full T29
remain unfinished. Downloads retain no hard total/cumulative duration or retry cap.
