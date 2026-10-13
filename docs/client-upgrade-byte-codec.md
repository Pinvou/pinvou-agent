# Client Upgrade V1 bounded byte codec

This first T27 client batch implements bounded JSON and RFC 8785 canonical bytes
in `features/updater/protocol/codec`. The application explicitly consumes this
private path dependency through its updater feature facade. The standalone crate
has no Tauri, OS, networking, persistence or CodeWhale dependency. Its unpublished
0.0.0 package is an internal codec, not another product version; the application
continues to use root VERSION 0.10.8. Old unsupported update behavior is unchanged.

`Document::parse` has fixed metadata (1 MiB) and credential (64 KiB) wire profiles.
Remote inputs cannot supply a larger profile. Strict serde seeds reject decoded
duplicate keys while reading each object, before insertion loses that information.
Depth is counted per value from root zero, with a maximum of 32. Member limits
are per object/array, each at most 4096, checked before decoding an excess member.
Decoded string/key length is measured in UTF-8 bytes, at most 64 KiB. Invalid
UTF-8, BOM/non-JSON whitespace, trailing input, malformed JSON, unpaired Unicode
surrogates and non-finite numbers refuse. Fixed errors never return input fragments
or parser-driver diagnostics. Some malformed-number/Unicode parser failures share
the fixed invalid-JSON category; no public protocol error mapping is introduced.

All parsed numbers use finite binary64, including integer spelling. This matches
the frozen Node parser's Number semantics. Later closed-schema checks must enforce
integral/safe-range requirements on values rather than Rust number variants.
`serde_jcs =0.2.0` uses `ryu-js 0.2.2` formatting and UTF-16 key sorting; exact
serde 1.0.229 and serde_json 1.0.151 preserve the synced application's existing
versions. `float_roundtrip` is unified for the same serde_json version in the App.
The crate and App/CLI lockfiles record the actual graph.
The final App/CLI lock delta contains only the three new packages and their
App dependency edge. Cargo initially reselected tempfile's permitted getrandom
edge; that unrelated choice was restored, and both complete graphs passed locked
metadata resolution with the original edge. Pure test dev-features explicitly
exercise preserve_order/raw_value alongside float_roundtrip, matching the actual
App/CLI runtime graph; arbitrary_precision is absent from those graphs.
No generic Serialize,
RawValue, mutable JSON accessor or unbounded Document constructor is public.

Canonical output is at most 1 MiB, as in the reference for both input profiles.
The credential wire limit still remains 64 KiB. A bounded writer never returns
partial bytes when numeric expansion exceeds the output bound. Canonicalization
does not prove that incoming wire bytes were already canonical. That exact-wire
comparison, closed envelope/schema, Ed25519/Root-role threshold/deny verification,
trusted time, metadata anti-rollback storage and upgrade rights are later owners;
none are supplied by a parsed Document or its canonical bytes. Full T27 remains
unfinished, and no new Tauri endpoint/startup update behavior is enabled.

## Verification

Initial seven groups had six passes and one failed frozen signing-input assertion
(1.71s). The test incorrectly omitted the normative wrapper binding protocol,
role, product, component and scope alongside signed; the codec itself required
no change for that finding. Corrected fixture assembly passed all 20 original
frozen envelope hashes and complete signing-input bytes. These are public test
verification fixtures, not production trust anchors or proof of signatures.

Seven focused groups then passed with the actual original Node reference opt-in
(0.32s). On the final synced serde version, seven groups passed again (1.70s).
With the final App-matching preserve_order/raw_value features and indexmap version,
all seven groups passed (6.85s); independent reruns passed (6.79s and 7.20s).
The same seven groups also passed with actual Node 22.23.3 (6.42s), matching
the CI major version; no new Node runtime dependency is added to the application.
Standalone all-target Clippy with warnings denied passed. Original Node
parseJson/canonicalize were actually invoked for 19 number/Unicode/rejection/depth
cases; tests do not generate their expected oracle bytes with Rust. Controls also
cover 2^53/u64 rounding, negative zero, finite/subnormal/underflow/overflow,
ECMAScript exponential boundaries, UTF-16 ordering and no Unicode normalization,
escaped duplicates, UTF-8/string/key bounds, per-container 4096/4097, depth
including deepest empty containers, metadata/credential wire exact/+1, and
canonical numeric expansion to exactly 1 MiB/+1 with no partial output.

CI explicitly runs the standalone manifest's fmt, all-target Clippy and tests with
Node 22 and the original reference. No oracle verification is claimed by a local
default run that omits `PINVOU_UPGRADE_NODE_REFERENCE`. Current worktree safely
merged then-current origin/main once, without conflicts, and aligned both parent
gitlinks before implementation. Neither submodule source/gitlink was changed by
this codec. Independent review found that changes to only the test oracle or its
three original reference/vector inputs could bypass the Rust job. Those direct
paths now trigger the codec gate, with an executable routing regression. All 44
CI policy tests passed (0.101s), with independent reruns also passing. The codec step runs in its own directory to
discover the same application toolchain configuration. Actual Windows App
integration passed all four updater tests, including both new facade controls
(0.00s test execution after a 47m08s cold build). Compilation used the project's
existing Windows rustc stack wrapper through an invocation-only environment
variable. Both independent roles found no remaining mandatory B1 source finding.
The full App library run completed with 2967 passed, 11 failed and 10 ignored
(1659.42s); it did not pass. Five failures were existing include_str source pins
that expected LF in a CRLF Windows checkout. Restoring only three unmodified
source files to exact HEAD LF bytes and recompiling passed those five controls
(0.01s after a 1m22s rebuild), without a logic/assertion change or committed diff.
Both independent roles traced the six other failures to existing Windows fixture
or dependency limits: four reconciliation fixtures use Unix-rooted paths that
Windows Path::is_absolute rejects, one asserts slash separators on a Windows
PathBuf, and one lacks bundled Python before its intended rollback failpoint.
Their strings/booleans/ordinary integers and those path/runtime checks have no
identified dependency on float_roundtrip. The rollback path is unverified by that
failed case. These six failures remain disclosed; no all-App pass is claimed and
marketplace functionality is outside this byte-codec batch.
Both independent roles gave final scoped B1 PASS after the evidence and documents
were frozen. This result does not certify the full App suite or full T27.
No physical platform, metadata trust, provider or service E2E is claimed. Deployment
is still deferred, and downloads have no hard total/cumulative time limit.
