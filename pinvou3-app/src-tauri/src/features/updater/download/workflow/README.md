# Explicit download-cache jobs

Private reusable Rust T29 job library and the `upgrade-download-cache` QA/support
adapter. It composes the existing restricted network and actual byte libraries.
There is no application auto-updater, install command, CA override or execution
object constructor. The CLI's `help` lists the closed command surface.

See [inputs, workflow, recovery and limits](../../../../../../../docs/client-upgrade-download-jobs.md).

```text
cargo test --locked -- --test-threads=1
cargo fmt -- --check
cargo clippy --locked --all-targets -- -D warnings
```

Set `CARGO_BUILD_JOBS=2`. Node >=22 and the frozen reference's existing npm
dependencies (`npm --prefix specs/upgrade/v1 ci --ignore-scripts`) are needed for
signature fixtures. Windows runs the real custody/process controls; other hosts
run pure signature controls and the explicit unsupported adapter test.

The normal tests are isolated: local peers only hold real TLS handshakes for
cancellation/death controls. They do not serve successful HTTPS. To explicitly
allow a separate public-network smoke, build the production binary, then run:

```text
python tests/public_https.py --allow-public-https --exe target/debug/upgrade-download-cache.exe
```

This retrieves one fixed public Rust license file with independent Python TLS,
signs ephemeral QA metadata for its exact bytes, and downloads it using that
unchanged production executable and its normal TLS trust. No CA is installed or
overridden. It is not an installer/ZIP/current-qualification acceptance. Owned
fixture files are retained; failed network runs fail rather than becoming skips.

The relative-handle filesystem adapter is shared through updater-local
`platform/custody`. The download compatibility adapter retains the same public
APIs, fixed error codes and Identity JSON; it adds no native authentication,
trusted journal ownership, installation permission or protected staging.

The extraction compatibility control accepts an independently retained old
production executable; it creates an old job, compares new status/cache scans,
and preserves a damaged journal until exact source restoration:

```text
python tests/platform/legacy_compat.py --old-exe OLD_EXE --old-sha256 OLD_SHA256 --new-exe NEW_EXE --proof PROOF_JSON
```

This explicit local control performs no download and retains owned fixtures.
