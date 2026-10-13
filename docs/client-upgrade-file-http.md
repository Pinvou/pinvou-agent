# Client upgrade restricted HTTPS transport

T29 batch 2 adds a private GET transport under
`pinvou3-app/src-tauri/src/features/updater/download/http` and a narrow App facade.
It binds the original B1 request plan to the actual response and streams entity
bytes. Full T29 remains unfinished; no workflow command is registered yet.

## Destination and credential boundary

The protected owner supplies a current immutable approval snapshot and source
continuity evidence. The adapter approves the canonical HTTPS host and effective
port before DNS and approves every resolved socket before connecting. Literal IPs
use their actual URI address, never a substituted resolver result. Empty, excessive,
unspecified, multicast or wrong-port results fail. A deny-only fallback resolver
prevents unapproved DNS fallback. Approval establishes no signed qualification,
artifact identity or publication right. Existing callers are unchanged.

Each request uses a new private client: system proxies, redirects and all transparent
content decoding are disabled. Only GET, identity Accept-Encoding and original
Range/If-Range headers are supplied. Cookies and control-plane authorization have
no injection surface. Userinfo (including empty userinfo), fragments, backslashes
and raw control/space characters are refused before parsing. URLs use the frozen
8,192-character bound with an additional 64 KiB byte bound. Errors retain fixed
codes and discard SDK errors, including their URL/query and source chain.

The initial provider profile requires a directly reachable approved HTTPS URL;
all redirects are refused, including redirects to the same origin. Providers
requiring redirects need a later separately qualified adapter. DNS and connection
waits are bounded at 15 seconds per operation; read idle is 30 seconds. There is
no request-wide, download-wide or cumulative deadline and no retry-count limit.
Retry/URL renewal and the meaning of credible progress belong to the workflow.

## Streaming boundary

All response header occurrences are supplied to the B1 bounded planner without
duplicate folding. Header lifetimes are separate from the original target lifetime.
The returned body exposes its validated original target, write mode and exact
span before any body delivery; an ignored Range with 200 selects replacement.
When Transfer-Encoding is present, only one chunked coding is permitted and
Content-Length must be absent;
other codings, duplicate transfer fields and conflicting framing are refused.
An optional valid strong ETag can be returned as opaque bytes; equal tags establish
no immutable-source evidence. Only exact entity EOF produces a segment receipt.
Excess, truncation, SDK failures and consumer abort latch failure and release the
response; subsequent reads cannot produce a successful receipt. Delivered chunks
are at most 64 KiB, without collecting the whole body before splitting.

This output bound is not an SDK allocation bound. The existing pinned SDK owns
TLS, HTTP parsing, framing and network chunk buffers. B1's count/per-value/aggregate
limits apply after SDK parsing; they cannot establish equivalent preallocation
limits. Provider qualification must account for the actual SDK parser behavior.
The module supplies no retained-file ownership, writer, durable partial ledger,
exclusive staging, final hash/signature, platform authenticity or activation right.

## Verification

The isolated HTTPS harness builds a standard-library Go peer with a randomly
generated in-memory CA and leaf key. Only the public test CA is written to the
owned temporary directory. Test roots and short idle settings exist solely under
Rust `cfg(test)`; production accepts no root/client override. A child-only synthetic
proxy trap checks system-proxy isolation without changing user configuration.
The harness enables every otherwise ignored HTTPS test and asserts wire GET,
identity encoding, original Range/If-Range, no control/cookie/proxy credentials,
zero proxy or redirect-target hits, and zero denied/wrong-pin endpoint hits.
CI routes Go/Python helper-only changes into the mandatory standalone gate.

Nine groups cover URL normalization, origin/address/DNS refusal, TLS trust/name,
resume/replacement/short segments, redirects/encodings/duplicate headers, recovery,
exact EOF/error/abort, continuous progress beyond an idle period, idle gaps and
bounded delivered chunks. Validation results and independent review are recorded
in [implementation progress](upgrade-implementation-progress.md).
Deployment and full T29 integration remain deferred.

The final primary run passed all nine groups (2.38s) and 34 real request assertions,
including raw TLS duplicate lengths/ranges/encodings and framing conflicts. Two
independent reviewers each passed an earlier nine-group/30-request tree and then
reviewed the final getter/duplicate-header delta separately; their earlier runs
are not presented as final-tree execution. Final standalone fmt/Clippy, all eleven
B1 transfer groups, 47 CI policies, architecture, version, diff and YAML checks
passed. Both complete locked App/CLI graphs passed; each parent lock adds only the
local package and App edge (11 lines), with no external version changes. Actual
final Windows App integration passed all ten updater tests (0.01s after a 1m38s
incremental build with the existing compiler stack wrapper). No App-wide pass is
claimed; previous unrelated Windows limitations remain documented in
[byte codec evidence](client-upgrade-byte-codec.md).

Initial findings were an oversized enum and an overly specific immediate-connect
error expectation. The primary role boxed the response state and accepted bounded
connect timeout for the unavailable approved pin, retaining zero actual peer hits.
Self-review added strict framing and pre-delivery plan visibility. Independent
review added raw duplicate Content-Length/Content-Range/Content-Encoding controls.
Both independent roles gave final B2 scoped PASS after reviewing final App/B1
evidence and documentation. No mandatory finding remains.
