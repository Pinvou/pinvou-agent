# Inspect a retained FullPack's native files

The explicit `upgrade-inspect-retained-native` tool connects an existing download
job's retained complete bytes to actual Windows embedded Authenticode evidence.
It creates one fresh ordinary output file and prints a bounded JSON report. It
does not execute an installer, initialize protected trust or install a service.

```text
cargo build --locked --manifest-path pinvou3-app/src-tauri/src/features/updater/download/native-inspection/Cargo.toml --bin upgrade-inspect-retained-native
upgrade-inspect-retained-native inspect JOB MANIFEST_SHA256 ATTEMPT OUTPUT_DIRECTORY MEMBER LIMITS_JSON
```

The original `inspect` command/library function and its report remain unchanged.
Three additional commands observe exact helper/launcher declarations using the
same source, capacity, copying, identity, cancellation and native policy:

```text
upgrade-inspect-retained-native inspect-execution-helper JOB MANIFEST_SHA256 ATTEMPT OUTPUT_DIRECTORY MEMBER LIMITS_JSON
upgrade-inspect-retained-native inspect-launcher JOB MANIFEST_SHA256 ATTEMPT OUTPUT_DIRECTORY MEMBER LIMITS_JSON
upgrade-inspect-retained-native inspect-helper-artifact JOB MANIFEST_SHA256 ATTEMPT OUTPUT_DIRECTORY HELPER_ID MEMBER LIMITS_JSON
```

`inspect-execution-helper` takes no helper ID. It reads the original signed
`executionHelperId` and the complete declared `helper` identity; existing Package
semantics require that ID to name one unique helper with the same complete native
identity and target. It does not choose the first array entry. The explicit
artifact command accepts only an exact case-sensitive ID already declared in
that Package. Choosing a helper artifact does not change the execution helper.
`installationTarget` is a logical declaration and never a source ZIP locator or
an arbitrary filesystem command/path for this tool.

Role commands produce a separate wrapper with `selection.kind`,
`selection.helperId` (null for finalInstaller/launcher), and `observation` holding
the original full report. `executionHelper` reports the ID from the verified
declaration; `helperArtifact` reports the matching declared ID. These describe
Package roles, not installed roles, approved Registry entries or verified native
versions. All five qualification/permission/time flags remain false in the
wrapper, observation and native report. The library's ordinary `RoleRequest`
combines a closed `Role` and independent member locator; `inspect_role` returns
the same custody through a `RoleObservation` until dropped.

Retain the original independent `manifestSha256` from `upgrade-download-cache
create`. ATTEMPT is a positive canonical decimal number. OUTPUT_DIRECTORY must
already exist on the local supported filesystem. MEMBER is the exact ZIP member
whose bytes match the selected signed Package native declaration (`finalInstaller`
for the original `inspect` command); member names themselves
carry no role or publisher authority. A helper alias with identical bytes is
therefore equivalent as a locator. Only the Windows x86_64 NSIS target is
supported. Actual signature observation covers embedded PE signatures; it does
not establish that an arbitrary PE implements NSIS installation semantics.

The separate capacity file contains exactly these positive numeric limits:

```json
{"fileBytes":134217728,"expandedBytes":134217728,"entries":64,"metadataBytes":1048576,"pathStateBytes":1048576,"ratio":100}
```

These are explicit example archive-processing capacities, not approved platform
budgets or a 32 MiB blob limit. No cumulative download deadline or retry cap is
introduced. The command performs no network transfer or certificate retrieval.

The existing download manifest supplies the exact original Package envelope,
expected context and original supplied protocol time. Its independent digest,
actual original writer.lock identity and Root/Package checks precede retained
whole-file reading. The target is signed `fullPackage`, so bytes are FullPack
ZIP rather than the outer upload ZIP. The dynamic journal is neither consulted
nor repaired: a damaged or absent journal cannot invent success or prevent an
independently pinned complete byte observation.

The existing package/archive implementations check the complete current ZIP and
all declared native byte identities, locate the selected native declaration by exact
size/hash, copy it to an exclusively created single-component cache filename,
synchronize it and rescan its complete actual destination. The tool keeps the
source/job and destination directory handles. All destination writers close
before a relative read-only reopen checks the original identity. The native
verifier then checks the full expected size/hash before and after actual WinTrust
collection using that same file object. No ordinary File-to-cache-capability
constructor or caller-reported sealed flag is introduced.

The close/reopen window can refuse after replacement or conflicting access. It
is **not an atomic protected execution handoff**. Directory custody and sharing
are process-local; they establish neither machine ACL protection nor persistent
anti-rollback, fencing, installation rights or resilience to privileged storage
compromise. Partial failures can retain untrusted ordinary output files. The tool
does not delete by a re-resolved path or replace an existing output name.

The successful report binds original job identity and manifest pin, attempt,
FullPack ID/size/hash, original Package-envelope hash, source physical identity,
member name, supplied complete native identity and actual native observation.
`destinationName` identifies the fresh ordinary file inside OUTPUT_DIRECTORY.
The library retains source/job and native-file custody until Observation drops;
the CLI retains it until output finishes. Afterwards files are ordinary caches.

Exit zero means collection completed, including unsigned, invalid or unknown
native results. Publisher qualification, current qualification, installation
rights, VerifiedExecutionObject and trusted-deadline evidence are always false.
The original supplied time is historical metadata context, not T26 current time.
No Publisher-to-certificate mapping or missing current owner is inferred. Native
policy is exactly the existing cache-only, whole-chain-except-root, no-UI,
no-MD2/MD4 policy; missing revocation evidence cannot become verified. A partial
JSON write or nonzero exit is not a complete successful report; retry with the
original expectations creates another fresh observation without changing a job.

The strict existing-cache positive is run separately on a suitable Windows host:

```text
cargo test --locked --test process cache_only_strict_fullpack_signature_positive -- --ignored --test-threads=1
cargo test --locked --test process roles::cache_only_strict_execution_helper_positive -- --ignored --exact --test-threads=1
```

Failure to obtain complete chain/timestamp evidence fails that explicit check;
the tool does not fetch material, import certificates or substitute a fixture.

An additional explicit compatibility test requires the independently retained
original T29 binary with SHA-256
`52b3b639e9f64a557813b37090255706e375eeb31013c29b57ccadcf684168b3`.
Set the test-only `PINVOU_UPGRADE_LEGACY_DOWNLOAD_CLI` to its local path, then run:

```text
cargo test --locked --test process roles::original_52b3_job_can_observe_exact_execution_helper_and_old_final_installer -- --ignored --exact --test-threads=1
```

The old binary performs actual job creation; tests insert the signed FullPack
bytes as a retained candidate without inventing a successful download journal.
The new role and original inspector both read that original job. No replacement
binary, self-reported version or simulated native API satisfies this opt-in.
