# Upgrade implementation progress

Task source: [V1.0 development tasks](pinvou-upgrade-platform-development-tasks.zh-CN.md).
Checkpoint: **2026-10-24 05:56 Asia/Shanghai**.

There are 47 tasks: seven completed, nineteen in progress, twenty not completed,
and T42 explicitly deferred. The strict completion ratio is **7/47 (14.9%)**.
The equal-task engineering estimate is **35.0%**. T07, T08 and T31 reflect their current delivery;
other estimates retain their 2026-10-22 assessment. These estimates are not
measured hours or release readiness.

## Delivery rules and active work

**T31 Windows helper/launcher host is implemented and pushed** in `18a241e1a`.
The current main task is **T29 download continuation and immutable-source binding**.
T08's internal atomic publication, renewal, deny/pending and durable worker
module is complete. Trusted host composition must explicitly start, cancel and
join the callable worker; actual first activation requires T14's business owner. T29's
reviewed byte-custody and installed-image components are delivered, and the
owner has approved mandatory signed source binding without legacy compatibility.
Work on one main task
and at most one direct prerequisite; other incomplete tasks are parked. Parallel roles
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
The 32 MiB limit applies to the joined new publication-signing batch; stored
graphs and package downloads have separate bounds.

## Completed T08 internal publication delivery

The complete internal metadata publication owner is committed and pushed in the
separate service repository as `316090d0801b10c6fe288511e13437dee0f51626`. T08 is complete within its
publication/fencing boundary. The sole component head remains authoritative;
one original T06 transaction commits business writes, affected generations,
the complete Release/Target/Snapshot/Timestamp graph, current pointers, issuance,
result, audit and outbox. Private renewal preserves immutable Package bytes and
archives retired Release/Package closures with their actual historical Root.

Active signing-key deny atomically records deny, affected pending generations
and one durable reconciliation job. Missing business ownership keeps selections
unavailable while the worker records persistent backoff and alerts. The callable
worker scans independently of signing every 30 seconds, caps retry backoff at
five minutes and emits durable alerts after 15 minutes. Trusted host composition
must start, cancel and join it explicitly; the current server executable does
not automatically start this worker.

The native business-source registration binds actual fixed SQL functions and
physical records. A schema-owner fixture, matching interface or qualification
boolean cannot provide T14 eligibility. Real first activation and the later
dynamic endpoint composition remain with their owning tasks.

Independent deep source, repair and completion-boundary reviews passed. Native
PostgreSQL tests verify original-transaction publication, full graph/archive
readback, multi-channel all-or-nothing changes, private renewal, fresh opening
entropy, stale input/owner refusal, permissions, split-commit refusal, lease
takeover and exact replay. Real TLS/OIDC/MFA/dual-review tests cover active deny,
pending jobs, missing-owner retries/alerts and late authorization rollback.
Pure tests cover graph planning, historical key dependencies, independent bounded
copies and worker scanning during blocked signing.

The final migration regression verifies the exact genesis-v2/active-v3 chain,
reinitialization without downgrade, native body/default/volatility/digest
tampering, unknown migrations and published-head refusal without active support.
A half-upgraded original profile fails runtime readiness until explicit schema
initialization; its migration preserves actual native identity and business
state. The original HTTPS/issuer/ACK regression and real Chrome workflow passed,
including three languages, dual review, damaged saved input refusal and original
lost-ACK recovery. Admin build and three response-profile parser contracts passed.

Final full default Go tests passed **4038 / skipped 1736 / failed 0** test
records (1264 passing and 946 skipped top-level tests). Dependency verification,
full static analysis, all command builds and the pinned contract check passed.
All qualifying invocations closed naturally without resource-floor breaches.

Failed development runs remain recorded. This includes the discovered legacy
SQL schema mismatch that allowed preparation but refused commit; the additive
migration repairs it without changing original schema bytes or granting active
ports to a genesis-only runtime. Functional failures are not environment skips.

Earlier delivered milestones remain part of T08: complete current reads and
the approved Windows fixture repair (`9fca399`), finite three-role preparation
(`4888054`), bounded published-only refresh (`dbf22f7`) and complete finite
publication signing (`5c2dc4a`). The related reference-model repair is committed
in the client repository as `eed20df6d`.

## Completed T31 delivery

- Standalone helper and stable launcher, authenticated fixed-action IPC, native
  scope/identity checks, registration CAS, takeover/fencing primitives, controlled
  restart handoff, app wiring and explicit lab/production packaging are integrated.
- The no-argument `helper --install-local` entry requires an already elevated local
  administrator and the complete local package. It validates held file bytes and
  PE resources, creates a fixed protected source with native creation ACLs, then
  starts only its retained installer. Partial/different source is refused and
  preserved. Existing installation repair requires the original exact source.
  It does not authorize a metadata upgrade or prompt for elevation.
- First intent, immutable metadata/preparation genesis and original Root bytes
  commit in the same native registry transaction. Readback verifies the complete
  scope-bound state; retries never reset epochs, Root or missing records. Root
  signatures at signed issuance prove anchor integrity only, not current metadata
  qualification or trusted current time. Unavailable T32/T36 handlers refuse.
- Scoped independent source review, repair review and T31 task-coverage review
  passed. Current Rust tests: **host 74 passed / one existing ignored; metadata
  owner 6 passed; ownership 4 passed; custody 2 passed**, zero failures. All owned
  processes closed naturally and resource guards remained satisfied.
- Packaging/startup contracts: **114 tests; 111 passed, zero failed, three native
  opt-in tests unexecuted**. Normal and embedded production profile checks passed.
  A fresh current-source dev build produced both actual standalone roles; PE
  gates verified System32-only dependent-load policy and absence of dynamic CRT
  imports. The real helper public-profile command passed. Two actual CLI controls
  refused invalid local arguments and unavailable metadata authorization with
  their exact expected exit codes. These are lab artifacts, not a shipping release.
- The three additional approved app lock edges passed an isolated Windows compile
  probe. OAuth2 and serde-saphyr used the app's resolved Windows feature sets;
  iana-time-zone was checked explicitly although absent from that active graph.
  The probe verified all three exact selected dependency versions. This is not a
  whole-application build. Existing package versions/checksums were preserved;
  the app added only the two internal owner packages.
- Rust formatting, locked dependency resolution, whitespace and architecture
  checks passed. The first SHA formatting compile error was fixed and the failed
  run retained. Earlier Node summary-parser, MSVC teardown, low-RAM refusal,
  ambiguous dependency selection, Cargo subset-selection panic and UTF-8 setup
  failures remain diagnostic history, not successful tests.

Environmental E2E gaps: isolated guest SCM install/repair/removal, ordinary-user
and multi-user entry, recovery after process death/revival and reboot remain
unverified. Native compiler/signature opt-ins lack directly reusable qualified
inputs. Record these as follow-up scenarios, not passed tests. Under the owner's
explicit policy these environment gaps do not block T31 completion or delivery.
The implementation does not complete T26/T27/T28/T32/T36 or system acceptance T43.

Cleanup pending: the successful isolated dependency probe's disposable build
target and temporary directories are no longer needed. Automatic approval review
blocked their recursive deletion with no more specific reason. They are retained;
essential logs, probe inputs and reusable host artifacts remain available.

File distribution separates policy/control from immutable package data. T09/T18
still need complete OSS/COS adapters and switching acceptance: preserve package
identity, size and SHA-256, verify migrated bytes, and retain old paths for active
or resumable downloads. This remains queued, not a second active development task.

## Current T29 increment

The explicit QA/support cache tool now obtains DownloadInfo through the fixed
control POST. It verifies the original Decision before sending, then correlates
both response credentials and the exact Package before opening a new payload.
One real job writer stays held across POST and GET. Supplied time remains a QA
observation; no current qualification or immutable-source continuity is invented.

Independent scoped source review passed. Windows regression: download workflow
22 library and two process tests passed; control transport three passed / one
opt-in ignored; native inspection two library and thirteen process tests passed /
three existing opt-ins ignored. Final invocations all closed naturally with no
resource-floor failure. Earlier compilation invocations with a leftover MSVC
telemetry process failed the whole-process gate and remain failed history; a
cache-hit rerun established natural closure without discarding compiler outputs.
No registry package version/checksum changed in the two affected locks.

Protected byte staging is now implemented in the Windows host. Ordinary borrowed
file/metadata inputs grant no trust: compiled Seed/Root establish the exact
Package context, and only that verified Package supplies artifact/native claims.
The host creates a fresh fixed protected leaf, copies and completely verifies
the package and selected native member, closes writers and reopens the same
objects through held ancestry. The resulting capability offers bounded reads
and cancellable full rechecks, with no path, handle, launch or install interface.
The byte capability alone is not a VerifiedExecutionObject or current
installation qualification.

The next delivered component wraps that capability in a real same-process
`VerifiedExecutionObject` after another complete physical recheck. Its generation
contains both actual file identities, including creation fields, and the entire
authenticated Package/compiled Seed binding. Read-only references do not grant
authority. Handoff consumes the unique owner and rechecks the exact reference
and physical objects while retaining all original handles; an action borrow
likewise keeps the same owner alive. There is no path/handle/launch interface,
cross-process transfer, restart adoption, current qualification or install grant.
Object custody does not require or create a preparation/freeze lock.

Historical caches may now supply a borrowed Root successor chain. Verification
always starts at the exact compiled anchor and checks every consecutive
dual-threshold transition before binding the final Root-claims pin and Package.
Only intermediate expiry is tolerated; the final Root must be valid at the
supplied historical observation. The original empty-chain behavior is retained.
At most 4096 original objects of 1 MiB each are processed one at a time with
latched cancellation; no aggregate allocation, persistent Root adoption,
high-water advance or assertion of current qualification is introduced.

Independent deep and repair reviews passed, including fixes for cooperative
cancellation, genuine threshold/signature controls and a private Artifact type
import. The object-ownership increment also passed independent deep review with
no findings. The host depends on narrow byte/archive libraries rather than the QA
workflow or its HTTP stack. Latest Windows host tests: **94 passed, zero failed,
one existing opt-in ignored**; this includes nine genuine Root/Package/rotation
controls, the Root-input bounds test and five pure object-generation/transition
tests. The unchanged ownership API retains its preceding one passing compile-fail
doctest, proving the moved old owner cannot be reused. These pure tests do
not manufacture a native sealed capability or replace physical E2E.
Native-evidence sources were unchanged during these final increments;
their preceding qualified **9 library + 2 process** passes are reused, with one
existing opt-in ignored. The preceding protected-staging increment built both
standalone binaries; byte-matched independent artifact copies passed PE
loader/static-runtime gates, and the actual helper's public profile command
passed. The subsequent ownership-only API adds no dependency or loader change.
Rust formatting, normal lock resolution,
whitespace and architecture checks passed. Existing registry package identities
and edges are unchanged; the app lock only adds internal host dependency edges.

First compile invocations with lingering MSVC telemetry failed process closure;
unchanged cache-hit retries passed with natural closure. A direct PE check on
Cargo's hardlinked cache output refused as designed; independently copied and
byte-verified artifacts passed the same unchanged gate. These failed invocations
remain diagnostic records. No resource-floor failure occurred.

Actual protected-copy ACL/reparse, replacement and disk-exhaustion E2E remain
unexecuted without a qualified isolated Windows input/environment; they are
environment follow-ups, not passed tests or completion blockers. Current
production qualification, source continuity/continuation and integration with
the later authorized executor remain unfinished business implementation. The
existing DownloadInfo wire lacks an authenticated immutable-source object/version
binding. The owner approved a new signed immutable-source binding without legacy
compatibility; this is now the main implementation work. No ETag or QA
provider assertion is promoted to a production continuity proof. These increments
do not complete T29; its engineering estimate remains unchanged pending closure.

## Installed-image prerequisite delivered with T29

The host now provides the fixed, no-argument `--inspect-installed-images`
diagnostic and an opaque `InstalledImageObservation`. An already elevated local
administrator can inspect only the compiled profile's Ready installation. The
observation retains the real application inventory, its ancestry, all three role
images and the service process. Full hashes, identities, complete namespace,
registration digest/revision and SCM startup incarnation are rechecked before
return and on refresh. Failed refresh invalidates the diagnostic. Directory
construction, enumeration, name matching and byte scans support latched
cancellation. Existing service startup keeps its original ownership and commit
sequence; this diagnostic performs no installation or state write.

The minimal JSON omits private scope, paths, SIDs, native identities and startup
nonces. It labels versions as manifest declarations, grants no permission and is
not a retained lease or complete protocol SourceFacts. Source-semantic detection,
protected current metadata/time and qualification remain outstanding; neither T27
nor T29 is completed by this component, and their estimates are unchanged.

Independent source, repair and interface/document reviews passed. Final Windows
host regression: **99 passed, zero failed, one existing opt-in ignored**. Both
current standalone roles built; independent byte-matched copies passed PE
loader/static-runtime gates. The actual helper's public-profile command and exact
extra-argument/unelevated diagnostic refusals passed. Three affected packaging
contracts produced **36 passed, zero failed, three existing native opt-ins
unexecuted**. Formatting, whitespace and architecture checks passed. Resource
guards remained satisfied and final owned command trees closed naturally.
Initial compile invocations with leftover MSVC telemetry failed the whole-process
gate and remain failed history; exact cache-hit reruns established natural closure.

A real Ready installation, replacement races and concurrent registration/service
changes were not tested in an isolated installed environment. These environment
E2E gaps are recorded follow-up, not passed tests or delivery blockers. The shared
build target, current role artifacts and essential logs remain needed and retained.

## Per-task status

Only T31 changes from the previous estimates. Outstanding environment-only E2E qualification is not a
completion blocker; outstanding business implementation is.

| Task | Estimate | Status | Delivery / outstanding dependency |
|---|---|---|---|
| T01 | 100% | Implemented; independently reviewed and verified | Executable metadata/credential Schemas, byte/trust profile, reference verifier, independently verified fixed vectors and local CLI E2E |
| T02 | 100% | Implemented; independently reviewed and verified | Closed control APIs, publication/lifecycle/selection/quality reference models and regression tests; no production service implementation |
| T03 | 35% | In progress | Reviewed synthetic freeze/encrypted backup lab, thirty retained 1 GiB development samples, two individual 10 GiB many-file development samples and three earlier payload smokes; the full four-target matrix, final contracts, exact production inventory, approved finite budgets and strict time qualification remain pending. |
| T04 | 65% | In progress | Offline tools, Windows post-build npm adapter and role-specific NSIS inspection are delivered; the integrated source passed default regression and current-CLI normal/wrong-Root cases. Complete provenance, product build integration and exact platform qualification remain pending. Native code-signing certificates are optional. |
| T05 | 100% | Implemented; independently reviewed and verified | Separate service management identity, MFA, scoped permissions, dual approval, protected audit and three-language admin; isolated actual Keycloak password/OTP/provider flows verified; deployment realm acceptance remains pending |
| T06 | 100% | Implemented; independently reviewed and verified | Durable atomic executor, encrypted recovery projections, SQL lease fencing and background recovery; later business commands and production HA acceptance remain pending |
| T07 | 80% | In progress | Reviewed trust initialization, key material, scoped issuer enable/stop, permanent material fencing, OpenBao physical-stop recovery, authenticated Root/history/original-document catalog and finite atomic multi-Release publication signing; T08 now supplies active deny/publication composition; full T07 lifecycle acceptance remains pending |
| T08 | 100% | Implemented; independently reviewed and verified | Internal atomic publication/renewal, original T06 business composition, active deny/pending, durable job storage and callable reconciliation worker; independent review, native regressions, browser and common checks passed. T14 eligibility and host startup composition remain explicit separate boundaries. |
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
| T27 | 55% | In progress | Authenticated public/control verification and fixed POST; reviewed ordinary SQLite acceptance, complete original history and high-water recovery; immutable first-install anchor initialization and live read-only installed-image diagnostics are delivered; complete SourceFacts, protected current acceptance/time, workflow and qualification integration remain pending |
| T28 | 0% | Not completed | Immutable first-install genesis is delivered under T31. Live machine-wide preparation ownership, allocation/handoff/recovery and multi-user writer freezing remain unimplemented; the shared initializer does not count as this workflow. |
| T29 | 65% | In progress | Restricted transfer/cache primitives, retained Windows QA jobs, fixed DownloadInfo caller, original FullPack/native observation, protected staging, authenticated historical Root succession and same-process VerifiedExecutionObject ownership are delivered; production qualification/source continuity/continuation and authorized-executor integration remain pending. |
| T30 | 0% | Not completed | Complete encrypted backup, manual export and original-deadline local cleanup. |
| T31 | 100% | Implemented; independently reviewed and verified | Protected helper/launcher, explicit package source deployment, native identity/IPC/takeover/restart and atomic independent-owner initialization delivered. Current code, modules, role builds and safe refusal paths passed scoped review and checks. Physical SCM/multi-user/crash/reboot E2E remains unverified and is non-blocking under the owner's policy; no deployment or shipping qualification is claimed. |
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
