# Upgrade implementation progress

Task source: [V1.0 development tasks](pinvou-upgrade-platform-development-tasks.zh-CN.md).
Checkpoint: **2026-10-23 21:04 Asia/Shanghai**.

There are 47 tasks: five completed, twenty-one in progress, twenty not completed,
and T42 explicitly deferred. The strict completion ratio is **5/47 (10.6%)**.
The existing equal-task engineering estimate remains **32.9%**; estimates were
assessed on 2026-10-22 and are not measured hours or release readiness.

## Delivery rules and active work

The active task is **T31 Windows helper/launcher host**. Work on one main task and
at most one direct prerequisite; other incomplete tasks are parked. Parallel roles
implement, independently review, test and integrate this same task. Reuse reviewed
foundations and test tools; review changed behavior and its impact after repairs.
Do not start a fresh runner or historical evidence-review chain for a small patch.

Environment-only E2E failures or unavailable environments do not block task
completion, commits or pushes. Record the cause, unexecuted scenarios and follow-up;
never label them passed. Functional failures must be repaired and verified.
Clean task-owned environments promptly after successful tests only when they are
no longer needed, all owned processes/handles have closed, and exact paths have
been verified. Preserve essential results and shared or unrelated resources.

Independently usable components may be committed after their own review and
applicable checks. A component, source review, build or single-platform test does
not complete its enclosing task. T42 deployment and T03's 50 GiB / 500,000-file
measurement remain explicitly deferred. Downloads have no fixed total deadline.
The 32 MiB limit applies to joined business metadata, not package downloads.

## Current T31 batch

- Integrated previously reviewed standalone helper/launcher and protected custody
  code into the main working tree, app restart ordering, public build inputs,
  independent NSIS packaging and package inventory.
- Added explicit production opt-in with stable profile identity. Production and
  lab have separate service, pipe, directory, registry and mutex namespaces.
  Production refuses fault profiles and native acceptance hooks. Independent
  scoped review passed after repairing namespace isolation. Root/metadata checks
  remain mandatory; native code-signing certificates are optional.
- Packaging/startup regression: **114 tests; 111 passed, zero failed, three native
  opt-in cases skipped**. Exit zero and natural owned-process closure confirmed.
  The original wrapper could not parse Node's newer summary format; readback
  confirmed these counts without repeating the tests.
- Baseline host unit assertions: **58 passed, zero failed, one existing ignored**.
  Cargo exited zero, but MSVC auxiliary-process cleanup failed; the complete run
  is not passed and its build outputs are quarantined from execution reuse.
- The first new host unit attempt was refused before compilation because free
  RAM was below the existing 7 GiB startup guard. The other build was not
  interrupted. After RAM recovered, a fresh run passed **65 tests, zero failures,
  one existing ignored test**. Cargo exited zero; all 214 owned processes closed
  naturally and the resource guard did not trip. This run took 294.45 seconds
  overall; Cargo reported 4 minutes 18 seconds to compile and link.
- App/host Rust formatting, locked offline Cargo metadata, whitespace and
  architecture checks passed. Final independent scoped source review passed.
  The reviewed integration batch is ready for commit; T31 remains incomplete.
- The successful Node run's unused empty temporary directory was removed after
  natural process closure; command, logs and essential results were retained.
  The fresh Rust target is retained for the next T31 delta tests.

Remaining T31 implementation includes protected source deployment and binding
first-install T27/T28 ownership initialization. The metadata upgrade entry refuses
safely until a genuine producer exists. Physical service, IPC, multi-user and
recovery scenarios remain unverified; environment-only gaps are non-blocking.

File distribution separates policy/control from immutable package data. T09/T18
still need complete OSS/COS adapters and switching acceptance: preserve package
identity, size and SHA-256, verify migrated bytes, and retain old paths for active
or resumable downloads. This remains queued, not a second active development task.

## Per-task status

Estimates are preserved. Outstanding environment-only E2E qualification is not a
completion blocker; outstanding business implementation is.

| Task | Estimate | Status | Delivery / outstanding dependency |
|---|---|---|---|
| T01 | 100% | Implemented; independently reviewed and verified | Executable metadata/credential Schemas, byte/trust profile, reference verifier, independently verified fixed vectors and local CLI E2E |
| T02 | 100% | Implemented; independently reviewed and verified | Closed control APIs, publication/lifecycle/selection/quality reference models and regression tests; no production service implementation |
| T03 | 35% | In progress | Reviewed synthetic freeze/encrypted backup lab, thirty retained 1 GiB development samples, two individual 10 GiB many-file development samples and three earlier payload smokes; the full four-target matrix, final contracts, exact production inventory, approved finite budgets and strict time qualification remain pending. |
| T04 | 65% | In progress | Offline tools, Windows post-build npm adapter and role-specific NSIS inspection are delivered; the integrated source passed default regression and current-CLI normal/wrong-Root cases. Complete provenance, product build integration and exact platform qualification remain pending. Native code-signing certificates are optional. |
| T05 | 100% | Implemented; independently reviewed and verified | Separate service management identity, MFA, scoped permissions, dual approval, protected audit and three-language admin; isolated actual Keycloak password/OTP/provider flows verified; deployment realm acceptance remains pending |
| T06 | 100% | Implemented; independently reviewed and verified | Durable atomic executor, encrypted recovery projections, SQL lease fencing and background recovery; later business commands and production HA acceptance remain pending |
| T07 | 75% | In progress | Reviewed trust initialization, key material, scoped issuer enable/stop, permanent material fencing, OpenBao physical-stop recovery, authenticated Root/history/original-document catalog and genesis-only role-scoped key deny; active issuance and emergency publication composition remain pending |
| T08 | 60% | In progress | Reviewed genesis catalog/component management, protected atomic publication primitives and actual genesis-only append-only signing-key deny; active metadata and other business publication workflows remain pending |
| T09 | 55% | In progress | Bounded package inspection, immutable intake/recovery and current-trust Package/evidence observation; authenticated final-installer and exact helper/launcher/helperArtifact structure queries and three-language admin; formal admission, quarantine and platform qualification remain pending. Native certificate observations do not grant or gate admission. |
| T10 | 60% | In progress | Actual Manifest-bound materials, managed provenance profile preparation, independent dual approval and enable/disable, DSSE observations, pending dossier bindings/history, original-request recovery and three-language administration; formal channel supply-chain approval/revocation, scanner execution/coverage and bounded exceptions remain pending. |
| T11 | 65% | In progress | Protected multi-platform drafts, reviewed content freezing, individual Target cancellation, atomic mixed parent closure, authenticated history and original-request recovery with three-language administration; package qualification and Release/Target lifecycle approval/publication remain pending. |
| T12 | 55% | In progress | Reviewed stable-facts matching and forward-policy primitives plus protected immutable candidate history, preparation, independent review and original-key recovery with three-language administration; formal Registry publication, capability admission and cleanup permissions remain pending |
| T13 | 10% | In progress | Delivered planning domain and authority port; protected persistence, private recovery, HTTP/admin and genuine E2E candidates passed scoped source review. Contract routes and service sync are pushed; complete workflow tests/commit remain pending, as do active-floor approvals/publication, ordinary prerequisite paths and historical bridge eligibility. |
| T14 | 40% | In progress | Protected configuration drafts with actual UI/API/recovery; qualified parents, publication, scheduling and first-baseline activation remain pending. |
| T15 | 0% | Not completed | Approved automatic rollout scheduling, quality guards and controlled completion. |
| T16 | 0% | Not completed | Explicit Stable-to-Beta/Internal atomic draft synchronization. |
| T17 | 55% | In progress | Bounded original initial selection, protected logical workflow/initial installation, atomic handoff/live resume and private activation Session; qualified protected check/refresh composition remains pending |
| T18 | 70% | In progress | Reviewed production HTTPS primary/backup/redirect policy administration, independent approval, replacement/disable, original-key recovery and transient current-policy DNS diagnostics are delivered with three-language admin; qualified download-info issuance and client distribution remain pending. |
| T19 | 0% | Not completed | Current qualification and three-purpose authorize/consume/cancel/status API composition over T44. |
| T20 | 50% | In progress | Actual no-update/transaction and bounded Session/download/resume/activation acceptance, four HTTP batch routes, client request/ACK correlation, restricted task observation and delivered original Session-retention read-only inspection; actual task acceptance, durable client queue, quality contribution and complete archival/cleanup remain pending |
| T21 | 35% | In progress | Private sample/receipt persistence authenticates actual frozen SN sources and preserves original-stage history; contribution, aggregate evaluation and business composition remain pending |
| T22 | 0% | Not completed | Independent restricted unknown-consume reconciliation and protected original-request disposition. |
| T23 | 0% | Not completed | Complete grouped quality review and controlled unfreeze. |
| T24 | 0% | Not completed | Fixed weekly release-quality snapshots and restricted export. |
| T25 | 35% | In progress | Independent purpose/week OpenBao material ownership and the bounded offline ASN import/source-owner component are delivered. The latter passed pure, real PostgreSQL and real-data tests. Accepted-event attribution, retention and shared recovery candidates passed source review; their complete native acceptance/commit remain pending. First-acceptance model repair is reviewed, offline ASN grouping approved; authenticated edge inputs, purpose-qualified consumption and frozen privacy snapshots remain pending. |
| T26 | 20% | In progress | Trusted-time/budget primitives; measured platform evidence and complete host composition remain pending |
| T27 | 55% | In progress | Authenticated public/control verification and fixed POST; reviewed ordinary SQLite acceptance, complete original history and high-water recovery; protected host/time, workflow and current qualification integration remain pending |
| T28 | 0% | Not completed | Machine-wide preparation ownership and multi-user writer freezing. |
| T29 | 65% | In progress | Reviewed restricted transfer/cache primitives, retained Windows QA jobs and original production-job HTTPS-to-FullPack/native observation with exact final-installer/helper/launcher declaration and byte binding; production host/source/continuation and protected staging remain pending. |
| T30 | 0% | Not completed | Complete encrypted backup, manual export and original-deadline local cleanup. |
| T31 | 45% | In progress | Main-tree integration includes standalone host, app restart ordering, explicit lab/production packaging and separate protected namespaces. Independent source review passed. Packaging regression: 111 passed, 3 native opt-in tests skipped. Host units: 65 passed, one existing ignored; protected source deployment and T27/T28 ownership initialization require implementation. Physical SCM/IPC/multi-user/recovery scenarios remain unverified. |
| T32 | 0% | Not completed | Client consume, original-result recovery and permanent old-execution fencing. |
| T33 | 35% | In progress | Reviewed optional Windows same-object Authenticode observations, shared relative custody and retained FullPack final-installer/helper/launcher composition with original-job compatibility; protected host, installation/slot execution and platform qualification remain pending. Native certificate qualification is not a prerequisite. |
| T34 | 0% | Not completed | Actual Linux x64/arm64 upgrade execution and qualification. |
| T35 | 0% | Not completed | Actual macOS Intel/Apple Silicon execution and qualification. |
| T36 | 0% | Not completed | Protected single-hop execution, active-identity verification and local health. |
| T37 | 0% | Not completed | Complete normal upgrade interaction across hops. |
| T38 | 0% | Not completed | Complete silent preinstallation, restart activation and slot management. |
| T39 | 0% | Not completed | Forced gate and related higher-version forward repair. |
| T40 | 0% | Not completed | Machine-wide channel settings and three-language privacy/consent workflow. |
| T41 | 0% | Not completed | Atomic legacy bootstrap migration to the new protocol. |
| T42 | Deferred | Explicitly deferred | Production deployment, operations, alerts and disaster-recovery acceptance are deferred at the owner's request |
| T43 | 0% | Not completed | All-target real-system combined qualification and release gates. |
| T44 | 100% | Implemented; independently reviewed and verified | Actual protected lifecycle kernel, complete atomic commands, original retention/backup deadlines and worker/access-driven timeout convergence; T17/T19/T20 composition and deployment remain separate |
| T45 | 0% | Not completed | Approved helper/launcher evolution and restricted cleanup execution. |
| T46 | 0% | Not completed | Actual local-mode prerequisite certification and exact evidence integration. |
| T47 | 60% | In progress | Protected frozen SN lifecycle, operation-bound child write sets and complete scope occupancy/absence with original T06 atomic composition and permanent-source recovery; running Rollout, parent, quality and active publication composition remain pending. |

## Historical evidence

The full earlier progress record is preserved byte for byte in
[history through 2026-10-23](upgrade-implementation-progress-history-20261023.md).
Its old blocker wording reflects the rules at those checkpoints; the current E2E
policy takes precedence. Detailed evidence remains there so routine progress
updates do not grow the active summary again.
