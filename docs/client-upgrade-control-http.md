# Fixed-origin client control POST

T27 batch 37 adds an actual one-request control transport and an App protocol
adapter. All26 client POST paths come from the frozen original OpenAPI; the
complete unique typed inventory and closed path grammar are checked once.
Production always uses https://update.pinvou.com, TLS1.2 minimum and ordinary
certificate/hostname verification. No custom endpoint or extra authentication,
cookie, proxy, redirect or retry API is exposed. Fixture certificate/address
injection exists only under cfg(test); no production bypass is compiled.

Before networking, the complete bounded raw request profile and original request
contract are checked. The existing codec canonical bytes are the actual POST
body. Request media type/accept are application/json, encoding identity and
cache control no-store. A fresh client disables system proxies, automatic
redirects, cookie storage, automatic retries and content decompression; it has
15s connection and30s read-idle detection with no total/cumulative limit.
Stall/error/drop never proves an absent remote commit. Operation owners still
control retries with the same business key, unknown-consume recovery and ACK
persistence. There is no download duration or cumulative retry cap.

Response bodies are streamed under the existing1MiB metadata limit, checked
before accumulation, with early declared-length refusal. JSON media type,
UTF-8 charset, identity encoding and unambiguous transfer framing are checked.
This bounds accumulated payload bytes, not the SDK/TLS/header buffers or total peak memory.
Strict duplicate-aware metadata parsing and the unchanged complete operation/
HTTP error contract follow. The result retains the original complete bounded
wire bytes plus parsed Document, operation and actual HTTP status. Declared
4xx/5xx bodies can be returned: receiving a valid error is not business success.
SDK errors are discarded into fixed codes, with no URL/body/header/error-source
retention. Response/body types deliberately have no Debug implementation.

Three default tests cover all26 original request/response fixtures and original
route mapping, pre-network invalid/duplicate rejection, and header bounds.
The opt-in real HTTPS harness uses an ephemeral in-memory CA/TLS key and actual
loopback sockets while retaining the fixed hostname/SNI. It sends all26 routes
and checks actual canonical POST bytes, exact headers, no proxy hits, no
redirect hits and no Cookie/Authorization/Proxy-Authorization propagation.
Additional actual responses cover same/foreign redirects, undeclared status,
media/charset/encoding/size/chunk-size/duplicate input, duplicate/conflicting
Content-Length/transfer framing/truncated bodies, header/body stalls, declared
errors, exact1MiB body and untrusted TLS. Each negative restores a valid request, including untrusted TLS. A valid
chunked response restores the transfer framing controls.
Cancellation waits for the peer's public observation marker before dropping the
actual pending task, so a pre-connect timeout cannot impersonate sent-request
cancellation. A connection dropped after the peer received the full request produces exactly
one observed submission, followed by an explicit owner-requested restore.
A progressing response outlives multiple read-idle periods while each fragment
arrives promptly, proving idle detection is not a total deadline in this path.
Private keys never leave peer memory; fixture data is synthetic.

The App graph exposes the same one-POST port and checks pre-network failures.
CI runs default/Clippy/format plus the actual HTTPS harness, including changes to
its Python/Go/Node fixtures. Parent App/CLI locks add only the local crate;
independent lock resolution is aligned to parent external package versions.

Transport is usable as a bounded client port. Full T27 still requires actual
signature/trusted Root, source probes, protected high-water persistence and
workflow/owner integration; this module provides none of those permissions.
No production endpoint was contacted in these tests. Deployment remains deferred.
Verification and independent review facts are recorded in
[progress](upgrade-implementation-progress.md).
