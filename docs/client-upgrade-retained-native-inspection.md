# Inspect a retained FullPack's native installer

The explicit `upgrade-inspect-retained-native` tool connects an existing download
job's retained complete bytes to actual Windows embedded Authenticode evidence.
It creates one fresh ordinary output file and prints a bounded JSON report. It
does not execute an installer, initialize protected trust or install a service.

```text
cargo build --locked --manifest-path pinvou3-app/src-tauri/src/features/updater/download/native-inspection/Cargo.toml --bin upgrade-inspect-retained-native
upgrade-inspect-retained-native inspect JOB MANIFEST_SHA256 ATTEMPT OUTPUT_DIRECTORY MEMBER LIMITS_JSON
```

Retain the original independent `manifestSha256` from `upgrade-download-cache
create`. ATTEMPT is a positive canonical decimal number. OUTPUT_DIRECTORY must
already exist on the local supported filesystem. MEMBER is the exact ZIP member
whose bytes match the signed Package's `finalInstaller`; member names themselves
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
all declared native byte identities, locate the selected finalInstaller by exact
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
```

Failure to obtain complete chain/timestamp evidence fails that explicit check;
the tool does not fetch material, import certificates or substitute a fixture.
