# Experimental platform lab: development observations

Date: 2026-10-19. This records three isolated synthetic development smokes of
[platform lab v0](../tools/upgrade-platform-lab/README.md), after both scoped
independent static reviews and primary repairs. It does not approve a preparation
budget, establish a 30-run baseline, certify a target or complete T03/T46.

The owner approved the existing candidate platform/data/measurement proposal for
isolated experiments. The program uses actual child-process SQLite/WAL writers,
generation-bound saved/frozen evidence, AES-256-GCM, file fsync and complete
decryption/ordered name-size-hash/authentication checks. No real app/user data,
credentials, keyring entries, system date or power state were accessed or changed.

Runtime module identity: `1d23325351cbfa57b80dd064f5fdc4eea8ff92915d169491c0e7ce1294deca15` (the lab's framed
runtime-module SHA-256). The full reviewed ten-file source snapshot also remained
unchanged before, between and after all three runs. Private raw reports/logs and
synthetic outputs remain local; no environment paths or generated context IDs
are committed.

Observed environment: Windows x64 release `10.0.22631`, Node `v26.0.0`, Intel
Core i7-8700 at 3.20 GHz, 12 logical CPUs, 34,123,063,296 bytes reported physical
memory. The development volume was observed as fixed NTFS/healthy. Its device and
filesystem-type observations plus explicit local host/storage context stay with
the raw reports. Storage hardware/class is unqualified. Cache state and ordinary
development contention were not controlled. No other OS or storage class was
measured, and these shapes must not be pooled or linearly extrapolated.

| Profile | Exact synthetic payload | Shape | Samples | Coordination/freeze | Backup plus verification | Whole run including generation |
|---|---|---|---|---|---|---|---|
| small | 1 GiB / 10,000 files | many-small | 1 | 19.8375 ms | 20.770 s | 36.559 s |
| medium | 10 GiB / 12 files | few-large | 1 | 19.0251 ms | 68.646 s | 79.741 s |
| large | 50 GiB / 12 files | few-large | 1 | 23.9801 ms | 603.389 s | 926.886 s |

Database and freeze bytes are additional to the stated payload and are included
in `backup.bytes`. Stream framing/header/tag overhead is not separately counted.
Coordination timing starts with the actual freeze requests and ends after all
writer sequences, SQLite integrity and complete freeze evidence are persisted.
Backup timing includes source inventory, encryption, file fsync, decryption and
complete ordered content/authentication checks. Final report publication occurs
only after all owned children close successfully; that closing/publication work
is outside the two phase measurements. Wall time also includes generation and
process/report work. Same-boot monotonic measurements do not prove shutdown time.

The eleven focused tests pass with zero skips/failures. Actual controls cover
unsavable SQLite writer refusal; original generation non-restart; wrong key/tag,
truncation, manifest mismatch; real source/archive append during stat/open and
same-size overwrite; invalid paths/hardlinks; actual OS spawn failure and abnormal
child termination without a success report; context/prototype separation; and
complete report writes followed by real sync/close fault injection with no final
success name. Early failed development iterations were repaired before these
accepted tests and smokes; they are not counted as passing evidence.

The following gaps remain explicit: Node Windows directory durability is
unsupported; distinct OS accounts/services, machine-wide writer admission,
protected key custody and parent-crash key recovery are not exercised. The lab
uses an ephemeral in-memory key and an experimental container, not the production
backup format or restoration workflow. Trusted elapsed power-off time under
option A remains unproven. File fsync/readback and process close do not prove
machine-crash or power-loss durability.

Next qualification work must use the approved dedicated platform/storage matrix,
at least 30 independent successes per baseline/cache class, required adverse
cases, actual protected-time/ownership/key adapters and later real app/user/service
integration. Concrete budgets return for owner approval only after that evidence.
No product preparation limit is inferred here; downloads retain no hard total or
cumulative duration or retry cap. The 32 MiB server atomic metadata limit is
unrelated to these payload sizes.
