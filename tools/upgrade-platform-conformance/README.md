# Platform contract conformance program

This is T03's independently runnable observation-contract consumer. It imports
the single semantic definition under `specs/upgrade/v1/platforms`, fixed model
vectors and the existing experimental lab primitives. The application does not
depend on the runner, and the contracts do not depend on the runner or host.

```powershell
# Pure contract-model acceptance/counterexamples; Node22+; no external dependency.
node tools/upgrade-platform-conformance/conformance.mjs model
# Focused actual primitive probes; Node26+ (node:sqlite); existing dedicated scratch parent.
node tools/upgrade-platform-conformance/conformance.mjs lab-primitives D:/DedicatedScratch
# Module tests used by the separate Node26 Windows/Linux CI lane.
node --test tools/upgrade-platform-conformance/test/*.test.mjs
```

The parent must already exist, be absolute and free of link/junction ancestors.
The reused lab requires2GiB free headroom. Each invocation creates two uniquely
owned `pinvou-lab-<UUID>` directories containing only small synthetic inputs
(64KiB payload), SQLite files and experimental archive variants. It owns six
short-lived Node writer children in total and waits for their real close/pipes
before reporting success. No app/service, installer, VM, user-data enumeration,
network, wall-clock change or E2E journey occurs. It never terminates unrelated
processes. Keep a failed run for diagnosis; after use, an operator may remove only
the exact owned directories recorded by the two run IDs under the supplied
parent. Keys are wiped in `finally`; ephemeral keys cannot recover after crash.

Reports bind the contract revision, actual per-file source digests, aggregate
source identity, runtime OS/architecture/release/Node, case IDs/provenance and
status. Lab reports additionally bind its existing prototype digest and actual
target context. Contract-model vectors deliberately use their fixed Windows
context on every host; that context is **not** a host capability observation.
Successful primitive probes are labeled `experimental-primitive`. Every current
production port is actually invoked and returns a closed `unsupported` refusal,
reported separately from passing cases. A missing/throwing probe exits nonzero
with the stable diagnostic `CONFORMANCE_FAILED`, never a partial success report.

The primitive probes observe actual same-account writer checkpoint/close and
SQLite readback; reject a duplicate freeze and unsavable writer; authenticate
an exact experimental manifest; reject wrong key, order, tag, truncation, unknown
format, source change and exclusive-output replacement; and check a subsequent
valid archive readback. They also feed the real freeze observation with its
**absent admission fence** to the contract, verifying that this cannot be accepted
as production freeze. No protected registration/key store or production backup
format is claimed. `qualification:false` is unconditional; the report contains
no native installation success, certification, signing or execution authority.

Actual live preparation ownership, protected nonzero backup/cleanup, installed
SCM/TxR/reboot, real execution/slot/health and other-OS adapters remain unattempted.
Five-role code review, local validation, CI/merge qualification and release
qualification are separate conclusions. These module checks are not E2E tests.
