# Windows embedded-signature evidence

Reusable read-only Rust observations and the explicit `upgrade-native-evidence`
CLI. This is one T33 primitive, not a platform certification or an app updater.
See [workflow, evidence semantics and limitations](../../../../../../docs/client-upgrade-native-evidence.md).

```text
cargo build --locked --bin upgrade-native-evidence
upgrade-native-evidence help
upgrade-native-evidence inspect FILE EXPECTED_SIZE EXPECTED_SHA256 [EXPECTED_IDENTITY_SHA256]
```

The path entry uses the updater-local relative-handle custody library. The
library's `observe_file` reopens the caller's actual File with stricter sharing;
it does not re-resolve a name or claim that `try_clone` changes access/sharing.
An `Observation` holds that handle until dropped. The CLI prints only public,
bounded evidence; fixed errors contain no path, certificate body or credential.

Exit 0 means an observation was collected. Its per-signature status may be
`unsigned`, `invalidSignature`, `refused` or `unknown`. `verified` means that one
embedded signature's OS/provider check and collected chains passed the fixed
cache-only policy. It does not mean valid/approved Pinvou Artifact, publisher
qualification, current online qualification or installation rights.

Default checks require no certificate installation or network service:

```text
cargo fmt -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets -- --test-threads=1
```

Use `CARGO_BUILD_JOBS=2`. Windows tests copy real system signed PE bytes and use
the actual API; no fixture image is executed. They verify opposite path/hFile
inputs, unsigned bytes, rehashed tampering, live writable sections, identity and
namespace refusals, native cancellation cleanup and two CLI process-test groups.
The default hFile matrix compares the actual NULL-path baseline, including
missing signer evidence: an offline/cold revocation cache may correctly yield
`unknown`. Chain-completeness rule controls use a separate pure data fixture;
they do not claim native verification succeeded. Other platforms return explicit
unsupported. No Windows API result is simulated.

One additional strict positive deliberately requires locally cached complete
signature **and** timestamp chains on the real dual-signed kernel32 fixture:

```text
cargo test --locked --lib cache_only_strict_signature_and_timestamp_positive -- --ignored --test-threads=1
```

Run it explicitly on the target Windows evidence environment. Missing cached
revocation/chain evidence fails this opt-in check, without relaxing verification,
installing a CA, fetching missing material or turning the failure into a skip.
