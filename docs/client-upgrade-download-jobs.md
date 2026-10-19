# Retained QA download jobs

This T29 batch composes the existing Rust transfer planner, restricted HTTPS
adapter, actual file writer, complete byte scanner and signature verifier into a
usable explicit cache tool. Its callers are release QA, integration and support
operators. Future helper composition may reuse the same feature library. This is
not the application's automatic updater, a privileged helper or an installation
interface. The existing application update/install stubs remain unchanged.

The private crate lives under
`pinvou3-app/src-tauri/src/features/updater/download/workflow`. Build its thin
`upgrade-download-cache` binary with the existing locked Rust graph:

```text
cargo build --locked --manifest-path pinvou3-app/src-tauri/src/features/updater/download/workflow/Cargo.toml --bin upgrade-download-cache -j 2
```

## Explicit workflow

The commands are:

```text
upgrade-download-cache create JOB ROOT_JSON ROOT_SHA256 PACKAGE_JSON EXPECTED_JSON IMPORT_JSON
upgrade-download-cache status JOB MANIFEST_SHA256
upgrade-download-cache run JOB MANIFEST_SHA256 IMPORT_JSON NETWORK_JSON
upgrade-download-cache pause JOB MANIFEST_SHA256 RUN_ID
upgrade-download-cache cancel JOB MANIFEST_SHA256 RUN_ID
upgrade-download-cache check JOB MANIFEST_SHA256
upgrade-download-cache restart JOB MANIFEST_SHA256 IMPORT_JSON NETWORK_JSON
upgrade-download-cache check-retained JOB MANIFEST_SHA256 ATTEMPT_NUMBER
```

`create` verifies the original production-profile Package signatures against the
operator's independently pinned canonical public Root body and the supplied
expected product/component/target context. Root input is public body JSON, not an
envelope or private key. `EXPECTED_JSON` contains the original verifier's exact
`role: package`, product, component and `scope.targetKey` expectations. The full
package ID, safe positive size and SHA come from the checked Package; callers
cannot substitute another unsigned target or select incremental reconstruction.

`IMPORT_JSON` is a closed object with three fields: `request` and `response` hold
the **original JSON strings** for DownloadInfo, and `suppliedTimeMillis` is the
operator's explicitly supplied protocol-time observation. Original strings reach
the existing bounded codec before duplicate keys could be erased. The verifier
checks both actual Decision/Download signatures, original request correlation,
scope, package bytes and the exact hop Package reference. Expired credentials or
URL windows at that supplied time are refused. The tool supplies no trusted clock,
current denial inventory, protected bootstrap, high-water store or current online
qualification. An offline import does not itself authenticate the origin of the
outer control response or approve its URL.

The job retains a fixed qualification fingerprint including the complete
`update.workflowId`, `update.originalStage`, hop/endpoint and metadata projection.
A genuinely signed future import may change credential, decision and telemetry
session IDs/windows while retaining that original projection. It cannot change
the original workflow or stage. Import does not rewrite old records, events or
server outcomes. This local binding is not proof of server-side continuation or
present eligibility. Production T18/current-owner integration remains unavailable.
Root rotation requires separate future owner integration; a new Root is not
silently adopted from a download response.

The operator must retain the returned `manifestSha256` separately and supply it
again. It authenticates the static public inputs and original writer-file identity,
not dynamic journal statements. Input credentials and URLs remain only in the
operator-supplied input and process memory: neither goes into job records, tool
diagnostics or file-plane headers. Operators must separately protect/remove their
input files; the cache directory is not secret storage.

Network access occurs only on explicit `run`/`restart`. `NETWORK_JSON` is a closed
operator-consent object, for example:

```json
{"host":"files.example.invalid","port":443,"addresses":["192.0.2.10"]}
```

Consent names an exact normalized HTTPS host/port and resolved addresses. It is
not production provider approval or immutable-source evidence. The existing GET
adapter independently checks DNS and every destination address, denies redirects,
disables system proxies and transparent decompression, uses system TLS trust and
exposes no caller headers/client/CA override. No control Authorization, Cookie,
SN, install ID or environment proxy authentication is forwarded to the file plane.

The CLI emits its attempt's `runId` before transfer, actual successful-write
progress and actual pending-fetch/body waiting notices. Pause/cancel names exactly
that attempt. The control command acknowledges only the stored request, **not**
that a worker has stopped. The awaited run's final result follows actual body
closure and worker completion; cancellation keeps partial bytes. A dropped library
future requests pause but cannot acknowledge a join: the actual worker keeps its
OS writer handle until it exits. Killing a process leaves an unconfirmed recorded
attempt, not successful download evidence.

There is no download-wide or cumulative deadline or retry-count cap. Existing
individual DNS/connection/read-idle limits remain distinct. A later invocation
continues the local job without resetting its static inputs. This batch has no
production immutable-source proof owner and accepts no `sameSource` flag. Thus
every new network attempt uses a **new exclusive file from zero**, retaining old
attempt bytes. Equal URLs, ETags or a rehashed partial prefix cannot authorize
Range append. The existing transfer planner's proof-required Range capability is
preserved for future actual provider integration.

## Recovery and byte observations

The static manifest is published exclusively only after its inputs and initial
journal were flushed. Publication links the actual flushed pending handle into
the actual retained parent; its pending name intentionally remains. A failed
creation or publication is left intact; `create` refuses to overwrite it. All job
paths are fixed names or a formatted numeric attempt, never paths from a journal.

On supported Windows local-drive paths the adapter retains non-reparse directory
handles for the job and every ancestor without delete sharing. Only the drive
root is opened by a path. Each subsequent single component, directory creation,
writer/journal/payload/input open, and manifest/control link publication uses the
actual retained parent handle. Link publication also uses the actual source file
handle and refuses replacement. Paths are not re-resolved after capture.

Directory sharing alone cannot prohibit in-place junction changes: an ordinary
account can use `FILE_WRITE_ATTRIBUTES` even without write sharing. This batch
therefore does not claim to prevent all such metadata changes. The actual native
relative operation refuses a changed/reparse parent; leaf reparse objects are
never followed and are rejected by the actual handle's attributes. Pre/post path
checks or a sentinel file alone are not its namespace guard.

The actual original `writer.lock` handle uses share mode zero, with physical
identity bound by the external manifest pin. PID files and journal flags are not
locks. Payload handles reject directories, reparses and multiple links, remain
the same objects through write/scan, and are released only by their real owning
worker. Existing partial or complete files are never truncated or overwritten by
a new attempt. Inputs are bounded same-handle regular-file reads; hostile
stream/reparse paths fail. These are process-local ordinary-cache capabilities,
not administrator-resistant custody or execution seals.

The append-only journal streams bounded closed records with a hash chain. A
present malformed, empty, truncated or unknown record is **not missing** and is
not silently skipped or repaired. This detects format/local damage; an ordinary
hash chain cannot prevent coherent whole-history replacement or prefix rollback.
The external static pin does not protect dynamic terminal state, remote workflow,
first-stage time or ownership across a hostile administrator. Recorded phases,
including local cancellation, are untrusted diagnostic history, not irreversible
remote dispositions or permissions. File Sync does not certify Windows directory
durability, power-loss recovery, hardware integrity or a trusted filesystem ACL.

`status` reports recorded state and never declares current bytes matched.
`check` and complete-cache reuse scan the actual same opened file through EOF,
with exact original size and SHA, including after a prior success record. Local
IO uncertainty is distinct from an actual byte mismatch. Network uncertainty,
source refusal, pause and cancellation are distinct local results; no quality or
T20 event is submitted.

If association is missing/damaged, `check-retained` authenticates only the pinned
static inputs, acquires real custody and scans a selected fixed numeric attempt.
It does not read, rewrite or heal the journal. The library's `ObservedCache`
retains the actual read-only file and custody for a caller's subsequent cache use;
the CLI returns only that observation. Corrupt/mismatched bytes still fail and
remain intact. This avoids requiring another download solely because association
was lost. A bare file path, a read-only duplicate or a saved success record is
never a `VerifiedExecutionObject`.

Every observation explicitly reports `currentQualificationChecked: false`,
`nativeAuthenticated: false` and `verifiedExecutionObject: false`. Complete
byte scanning here is a QA observation, not entry into or completion of a trusted
T26 validation-budget stage. Safe unpacking/native publisher identity, protected
staging, machine-wide preparation, current permission/qualification, execution
and platform certification remain separate T27/T28/T29/T31/T36 work. No OS budget,
platform-support approval, deployment or full T29 completion is claimed.

Non-Windows custody adapters return `DOWNLOAD_JOB_UNSUPPORTED` before creating
files. They do not fall back to ordinary files or another platform implementation.
This limitation does not revise the approved four-target protocol scope.

## Focused verification

Run the standalone locked crate tests and Clippy with two build jobs. Tests use
the original frozen reference to produce fresh ephemeral public signed fixtures;
private signing keys stay inside the fixture process. Real file tests cover
physical single-writer exclusion, held-directory replacement, closed/damaged
journals, fixed-name isolation, same-size pollution and exact-source restoration.
They also exercise actual junction rejection, extra payload hard links, replaced
writer identity, failed read-only journal writes and damaged stop controls.
An ordinary-account FSCTL control actually converts a retained empty directory
with minimum `FILE_WRITE_ATTRIBUTES` access. File creation, subdirectory creation
and actual-handle link publication then refuse the changed parent without writing
its redirected target. Clearing the same fixture tag restores successful normal
operations; leaf reparse objects and link-name collisions are also refused. The
first share-only candidate was insufficient and its failed normal-workflow run,
minimum-access bypass proof and earlier red reproduction remain separate evidence.
The native adapter uses additional bindings/features of the already locked
`windows-sys` module; it adds no external package or version.
Independent executable tests terminate and Wait a real child, then reopen its
original bytes before another attempt; new attempts preserve those bytes. Actual
local TCP peers receive real production-client TLS bytes and deliberately leave
TLS pending for pause/cancel controls. They do not supply a test CA or fabricate a
successful HTTP response. A test-only file assembly separately exercises the
executable's complete-orphan scanner; it is not HTTP download evidence.

A separate explicit opt-in `tests/public_https.py` invokes the same production
binary, without a CA override or system trust change. Independent Python/system
TLS retrieves the fixed public Rust 1.89.0 `LICENSE-MIT`; fresh ephemeral QA
signatures bind those actual bytes to the original Package and request. The
production CLI uses its normal restricted HTTPS implementation and explicit
host/port/resolved-IP consent. The first successful run retrieved 1068 bytes,
SHA256 `b71bd43a069ca0641a9ecfe585ca7b3c53b5cc1608f8b68321168698e28b5ea1`,
then re-scanned the complete cache, rejected same-size pollution and scanned the
restored bytes independently of a deliberately damaged journal. This is actual
network/cache evidence for public license bytes, not an installer, native
publisher, ZIP, T18 continuation or production service acceptance. The reference
signatures use their explicit QA protocol-time input, not a certified current
clock. The public URL is fixed by the opt-in script and owned files are retained.

Author verification on Windows passed library groups and two independent
executable groups, with zero skipped/ignored tests. Architecture and the focused
CI-policy checks passed without increasing debt; Clippy treats warnings as
errors. Linux pure tests/unsupported behavior are wired into CI but have not been
run on a Linux host in this author session. New Windows CI prerequisites are
limited to the regression leg's frozen signature oracle; existing check routing,
security gates and unknown-path handling are retained. The leaf lock graph adds
no external package/version/checksum absent from the baseline application lock.

Development failures were retained: missing reference npm prerequisites,
Uint8Array/Buffer fixture conversion, binary64 extraction, nonblocking fixture
socket inheritance, an omitted test import, initial lint warnings and the initial
architecture/CI routing assertions. Their repaired runs do not turn the initial
failures into passes. Final frozen logs are recorded separately. No wall-clock
24-hour transfer, physical power-loss, privileged-administrator, abrupt OS reboot
or production file-service acceptance is inferred from these controls. The new
job always requests full bytes; successful Range/206/416 behavior remains an
existing lower-layer capability and is not claimed as this job's actual coverage.
