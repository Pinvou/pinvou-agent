# Experimental upgrade platform lab v0

This standalone Node 26 program implements the first experimental measurement
slice of T03. It uses built-in SQLite, child processes, AES-256-GCM, SHA-256 and
file fsync; it adds no runtime or package dependency to Pinvou Agent. Its container
is an experimental stream, not the final backup format or a protocol version.

The owner approved the existing candidate platform/data/measurement proposal for
isolated experiments on 2026-10-19. That does not approve concrete preparation
budgets or certify a platform. See [the proposal](../../docs/upgrade-implementation-decisions.md).

Use an existing dedicated scratch parent with sufficient space. Each run creates
a new exclusive `pinvou-lab-<random UUID>` directory. It does not enumerate user
profiles, boot Pinvou/CodeWhale, read credentials, change system time, power off a
machine, install a service or delete files. Outputs remain for diagnosis. Remove
only explicitly verified owned run directories when no longer needed. Avoid a
shared or untrusted scratch parent; this prototype has no privileged, handle-based
directory custody or machine-wide ACL/admission guarantee.

```sh
node --test tools/upgrade-platform-lab/test/lab.test.mjs
node tools/upgrade-platform-lab/lab.mjs run /dedicated/scratch small many-small
node tools/upgrade-platform-lab/lab.mjs run /dedicated/scratch medium few-large
node tools/upgrade-platform-lab/lab.mjs run /dedicated/scratch large many-small
node tools/upgrade-platform-lab/lab.mjs summarize /owned/run-one/report.json /owned/run-two/report.json
```

Profiles have exactly 1/10/50 GiB of synthetic payload. The many-small variants
contain 10,000/100,000/500,000 payload files; few-large contains 12 files. Database,
freeze content bytes are additional and included in `backup.bytes`; framing,
header and authentication-tag overhead is additional and is not separately counted.
The scratch space check reserves source plus encrypted output, a per-file
allowance and 2 GiB headroom. It is a preflight observation, not a disk reservation;
low-space/write errors fail the run. There is no automatic cleanup or truncation
to fit a profile. No generated or reported value limits product data or downloads.

Three independent owned child processes write real SQLite WAL databases, labeled
active-user, disconnected-user and service. These labels represent synthetic
writers under the current account, not actual distinct OS accounts or services.
Freeze sends the original generation once, stops new writes, checks the actual
checkpoint result, changes its exclusively owned synthetic database to DELETE
journal mode, closes the database and persists each writer's sequence. The
parent checks the persisted generation, row count and SQLite integrity before
persisting complete freeze evidence. Any missing/unsavable writer refuses the
whole measurement; it never emits a successful backup/report. Children are joined
on success and failure. The program does not claim that real app writers already
cooperate, or that a new unregistered process is fenced by the OS.

Backup timing includes inventory, streamed encryption, file fsync, complete
decryption, exact ordered name/size/byte hashes and GCM authentication. No verified
result returns before authentication finishes. It does not restore files or test
production backup migration. Keys live only in memory and are wiped after the
attempt; parent crash key recovery is explicitly unsupported. Do not use this
container to protect valuable data. Partial failed output is retained and has no
successful report. The final report name is published exclusively only after its
pending file has been synced/closed and every child has actually closed. The
pending file remains for diagnosis and is refused by aggregation. A filesystem
without hard-link support refuses report publication. Windows Node directory
durability is explicitly unsupported;
Unix directory fsync is reported only when actually observed. A file fsync or
same-process readback never certifies machine-crash or power-loss durability.

Measurements use a same-boot monotonic clock. Wall time only labels a report.
The owner superseded strict time option A on 2026-10-25. This program does not
implement the new authenticated-time/cross-boot-unknown/cleanup-recovery path; no
original deadline, retained secret or product budget is changed by its measurements.
Reports identify the exact runtime module bytes by SHA-256. Report aggregation
rejects duplicate run identities and mixed prototypes, workloads or machine
environments. It reports sample count, nearest-rank p95, lower median and maximum;
it never derives or approves a preparation budget. Cache state is uncontrolled.

An optional fourth run argument supplies a local JSON context file with exactly
`labHostId`, `storageId` and `storageClass`, each a non-secret ASCII label. Use a
stable dedicated-lab host/storage identity and describe the observed storage
class truthfully; these operator labels are not hardware attestation. The report
also records actual scratch device and filesystem-type numbers. Aggregation
requires exact matching context as well as prototype/workload/environment.
Without a context file, each run has a unique unidentified context and cannot be
pooled with other runs. Unknown filesystem type or storage class stays unknown.

Run at least 30 independent successes for each approved baseline and cold/warm
storage class on the dedicated laboratory matrix, then adverse cases and real
application/user/service integration as described by the proposal. A development
volume smoke is useful primitive evidence only. Unavailable cold-cache control,
distinct-user admission, protected key custody, restart/power-loss durability and
real authenticated-time/unknown/recovery behavior remain unproven capabilities
for final T03/T46 qualification. Universal offline powered-off elapsed-time proof
is no longer a prerequisite; see the approved implementation decision register.

## Repeated measurement series

`series.mjs` runs one homogeneous series sequentially through the same real
freeze, encryption, full byte verification and child-close implementation. It
does not run performance samples concurrently: other heavy experiments must be
scheduled separately to keep contention visible. It retains all data and refuses
an insufficient initial estimate for the entire requested retained series. Every
attempt also repeats the original current-space preflight. Neither observation
reserves disk capacity; later IO failures remain failures.

Create a local plan JSON file. The example requests thirty 1 GiB, few-large-file
experiments; it therefore needs space for all thirty sources and encrypted
outputs, plus overhead and headroom. Use truthful non-secret context labels.
`hostKind` is an operator declaration (`developer`, `physical` or `virtual`), not
attestation. `targetKey` must match the actual native host; macOS records the
actual arm64/x64 host separately even though the candidate package is universal.

```json
{
  "repetitions": 30,
  "workload": { "bytes": 1073741824, "files": 12 },
  "shape": "few-large",
  "context": {
    "labHostId": "dedicated-lab-host",
    "storageId": "disposable-volume",
    "storageClass": "unknown"
  },
  "experiment": {
    "targetKey": "windows-x86_64-nsis-perMachine",
    "hostKind": "developer",
    "cacheState": "uncontrolled",
    "mode": "synthetic-freeze-backup-v0"
  }
}
```

```sh
node --test tools/upgrade-platform-lab/test/*.test.mjs
node tools/upgrade-platform-lab/series.mjs run /dedicated/scratch /local/plan.json
node tools/upgrade-platform-lab/series.mjs inspect /owned/pinvou-lab-SERIES-UUID
```

Plans accept 1–1000 repetitions. `many-small` requires the exact existing
1/10/50 GiB and 10,000/100,000/500,000 file pairs; `few-large` uses the same byte
sizes and 12 files. Small actual implementation tests may explicitly use
`custom-smoke` with bounded custom byte/file counts. Such runs do not satisfy the
baseline sample requirement regardless of their repetition count. These
experimental input bounds are never product data or download limits.

Cold/warm cache control and production preinstall/direct-install/activation
adapters are not implemented. Requesting one records a preflight `unsupported`
outcome and stops without executing a substitute. A mismatched or unknown target
does the same. Remaining requested repetitions are `unattempted`; unsupported
does not mean an actual failed freeze or backup.

Each series has an exclusive directory, an immutable plan/source manifest and
separate numbered attempt directories. Before executing an attempt it syncs a
start record. After actual writer close it publishes that attempt's outcome
exclusively, including the full original sample, its original relative report
path and exact report SHA-256. Failures retain their partial data, a safe error
code and the requested repetitions they prevented. A failure stops the series;
the tool never retries until it obtains a convenient set of successes. Source
identity is checked between attempts and before publishing the final report.
The complete original manifest digest, including the requested repetitions,
experiment and recorded native host, is retained in each start/outcome and in
each raw sample. Inspection checks those bindings even before final publication;
it cannot shorten a partially completed plan into a complete smaller series or
relabel actual synthetic samples as a production mode or controlled-cache run.

SIGINT/SIGTERM request a graceful stop: the current attempt completes its owned
IO and joins writers, then no further attempt starts. This may take the full
current attempt; it is not a hard cancellation deadline. The completed attempt
retains its true success/failure status and `stopRequested` records the request.
An abrupt process loss leaves the final series report unpublished. `inspect`
reports a started attempt without a published outcome as `aborted`, with later
slots `unattempted`; this means completion was not confirmed and never proves
zero writes. Missing or invalid manifests/records fail inspection. The tool does
not resume a crashed run or overwrite any report, archive or attempt directory.

Inspection reopens the original samples, verifies their exact bytes and distinct
identities and checks matching source, workload, host environment and observed
volume/filesystem context. It refuses mixed, changed, extra or noncontiguous
attempts. Statistics are present only if every requested attempt succeeded in
that homogeneous series. All raw attempts remain in the report; failures,
unsupported capability and interrupted/unattempted work stay separate. A
complete but not finally published series can be inspected as such; no automatic
publication or recovery claim follows. The command exits unsuccessfully for
incomplete, failed, unsupported or unpublished results.
Recorded host facts, rather than the inspecting computer, determine target
compatibility and unsupported reasons. A present JSON null, boolean, number or
string is an invalid record; only an absent file is treated as unpublished.

For an approved baseline workload, `sampleRequirementMet` only means at least
thirty complete homogeneous primitive samples were retained. Uncontrolled caches,
synthetic same-account writers, production modes, protected time and other
unproven capabilities remain explicit. The output is review evidence: approved
and certified budgets stay null, qualification stays false, and no product
configuration is written. Reports are local observations, not authenticated
hardware attestations. Keep the dedicated scratch parent private; the inherited
prototype's path-custody and power-loss limitations still apply.
