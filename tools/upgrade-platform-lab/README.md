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
Trusted elapsed shutdown time remains unproven under approved option A; no
original deadline, retained secret or product budget is changed by this program.
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
distinct-user admission, protected key custody, restart/power-loss recovery and
trusted power-off time remain explicit blockers for final T03/T46 qualification.
