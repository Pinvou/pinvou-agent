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

## Fixed control acquisition

An existing job can obtain DownloadInfo through the fixed control service before
its next file attempt:

```text
upgrade-download-cache run-control JOB MANIFEST_SHA256 CONTROL_JSON NETWORK_JSON
upgrade-download-cache restart-control JOB MANIFEST_SHA256 CONTROL_JSON NETWORK_JSON
upgrade-download-cache resume-control JOB MANIFEST_SHA256 CONTROL_JSON NETWORK_JSON INDEPENDENT_CHECKPOINT_SHA256
```

`CONTROL_JSON` contains exactly `request` (the original DownloadInfo request JSON
string) and `suppliedTimeMillis` (an explicitly supplied safe integer). The pinned
Root, original Decision signature, Package and retained workflow/stage projection
are checked before the POST. The real response must pass signature, correlation,
Package, time and mandatory signed immutable-source/literal-URL binding checks
before a new payload file is opened. The caller cannot
supply a response, endpoint, CA, headers, retry policy or continuity assertion.

The same writer lock remains held across the POST and file attempt. Elapsed
control-request time is included in the supplied observation; it does not create
a total download deadline. `run-control` starts a new exclusive file from zero.
`resume-control` takes an explicit independently retained checkpoint digest from
a prior actual flush/scan notice; it never discovers that digest in the journal.
It verifies the current signed source tuple and the actual held prefix, then
copies checked bytes to a new exclusive attempt before Range. Before GET it
rechecks the complete import with the supplied time plus local verification and
prefix-preparation elapsed time; expiry or overflow refuses the attempt without
limiting an ongoing download. This is not a trusted clock. Equal ETags alone
cannot authorize continuation. A changed source tuple starts fresh; a forged
checkpoint or damaged prefix is refused. `restart-control` starts fresh even when
an earlier cache was complete. Without restart, a complete cache is scanned again
and reused without a control POST. See the linked workflow document for the
checkpoint's exact bindings and failure behavior.

Dropping the library future requests local cancellation, not a successful close
acknowledgment or proof that the remote service did nothing. Await the result for
local worker closure. Before the file-attempt run ID is emitted, the CLI's
`pause`/`cancel` commands cannot acknowledge stopping a control request.

These remain explicit QA/support operations. Supplied time is not trusted time;
results retain `currentQualificationChecked: false`, `nativeAuthenticated: false`
and `verifiedExecutionObject: false`. This does not provide current eligibility,
protected staging, installation permission or whole-T29 completion.

The extraction compatibility control accepts an independently retained old
production executable; it creates an old job, compares new status/cache scans,
and preserves a damaged journal until exact source restoration:

```text
python tests/platform/legacy_compat.py --old-exe OLD_EXE --old-sha256 OLD_SHA256 --new-exe NEW_EXE --proof PROOF_JSON
```

This explicit local control performs no download and retains owned fixtures.
