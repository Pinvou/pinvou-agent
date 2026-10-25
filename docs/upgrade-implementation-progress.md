# Upgrade implementation progress

Task source: [V1.0 development tasks](pinvou-upgrade-platform-development-tasks.zh-CN.md).
Checkpoint: **2026-10-25, Windows fixed-host final regression passed**.

There are 47 tasks: eight completed, eighteen in progress, twenty not completed,
and T42 explicitly deferred. The strict completion ratio is **8/47 (17.0%)**.
The equal-task engineering estimate is **37.0%**. T07, T08, T09, T10, T29 and T31 reflect their current delivery;
other estimates retain their 2026-10-22 assessment. These estimates are not
measured hours or release readiness.

## Delivery rules and active work

**T31 Windows helper/launcher host is implemented and pushed** in `18a241e1a`.
The current main task is **T09 Artifact-valid admission and quarantine**.
T10 channel risk review, exceptions and current-risk reads are delivered; its
remaining actual selection-widening composition is parked until T14 supplies the
real qualification producer. The fixed T08 adapter is delivered. T09 extends its
verified upload lifecycle and validation components with the delivered
admission/quarantine increment. Independent FullPack custody and generic
fail-closed refusal passed their checks. The Linux physical-main/XZ follow-up is
committed and pushed as service `e7e58fa`, with client progress `145540ee`.
The current direct T04 increment adds fixed Windows host FullPack tooling and
protected main-image checks. Its scoped built-CLI workflow and five owner/config
checks passed. Final common regression and the separately rerun final CLI also
passed, as recorded below.
T07 signing-key lifecycle is implemented, independently reviewed and pushed. T29 signed-source
continuation is delivered; its remaining production current-metadata/time owners
remain parked with T26/T27.
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

## Verified T10 original scanning and coverage increment

Service commit `e29b77f18d0c0aae53a70d45e703b4ee95ca8cb8` is pushed.
The fixed scan owner now retains original actor/scope/material/template and an
at-most-once native attempt, with explicit installation, durable recovery,
six immutable originals including actual stderr, and three-language management.
HTTP registration does not launch the scanner. A protected operator supplies
the actual SBOM/profile to the single-job CLI; no automatic materialization
daemon or formal risk approval is claimed.

The strict generic CycloneDX/PURL evaluator supports fifteen verified language
ecosystems using fixed Trivy parser revisions. Unsupported OS/package inputs,
missing versions, skipped components and unconfirmed diagnostics remain
incomplete. A complete coverage result still cannot approve risk, validate an
Artifact or publish a channel. The formal risk component below now consumes
these originals; capture alone remains separate from risk acceptance.

Independent source and repair reviews passed. Scoped domain/PostgreSQL/signed
HTTPS checks, the admin build, ten pure admin cases and three real browser cases
passed; supported coverage passed 20 top-level / 140 total records. Final default
service tests produced **4554 passed, 1762 skipped records, zero failures**.
Vet, module verification, all command builds and client contract pin checks
passed. Linux binaries cross-compiled; Linux/systemd native E2E remains
unexecuted on this Windows host. Original browser-locator and invalid PURL
fixture/qualifier failures remain recorded separately from repaired passing runs.

## Verified T10 formal risk component

Service commit `97324779984f7e4067829a3af0f717e0ebdcd201` is pushed.
The formal owner now provides exact Target/channel risk proposals, independent
review, approval/revocation, bounded dual-responsibility exceptions, authenticated
directories, private expiry/recovery and three-language administration. Final
native writes bind actual session/MFA deadlines to the original epoch. The
same-transaction current-risk reader authenticates highest risk/exception facts,
checks exact evidence bindings and actual SQL time, and returns bounded read
inventory. It does not qualify an Artifact or Target.

The fixed T08 adapter uses the actual complete reference census for atomic
deny/pending/job tightening. Old exception revocation, replacement or rollback
cannot be treated as a still-current basis for a replacement approval. Genuine
restoration/widening still requires T14's concrete selection proposal and source;
that producer/composition is not delivered and remains unsupported. T10 is
therefore **90%**, not complete. This is remaining business integration, distinct
from the non-blocking environment E2E gap.

Independent source/repair and documentation reviews passed. The final focused
invocation passed **11 top-level / 84 total records**, zero skips/failures; host
pool/classification/shutdown controls also passed. The **13 pure admin cases**
passed once without skips or retries. Final common checks passed **4682 records,
1768 skipped records, zero failures**, plus vet, module integrity, all command
builds and client contract pins. Original functional failures and repaired runs
remain separately recorded in service verification documentation.

Four formal real-browser cases, a protected Linux scan-to-approved lifecycle and
positive highest-approved/actual-exception-revocation lifecycle coverage remain
unexecuted in this environment. Rolled-back structural SQL controls do not
substitute for them. These environment gaps do not block this component's
delivery under the agreed policy. No download elapsed deadline or mandatory
native signing-certificate requirement is added.

## Verified T29 main-integration prerequisite

The approved alignment updates 32 exact requirements in fifteen updater
manifests to seven libraries selected by the incoming main branch. Cargo
normally regenerated the application and fifteen standalone lockfiles. The
application retains all 1065 incoming registry packages and their checksums,
with the original updater path packages and two canonical-JSON dependencies
restored. Independent source and lock review passed.

Seventeen native Windows module/doc/fault checks passed: **483 passing test
records, 28 existing ignored records, zero failures** in the qualifying runs.
The complete application MSVC all-targets check with `dev-tools`, version
consistency, dependency advisories/licenses/bans/sources, architecture guard,
fast fork guard and whitespace check passed. Application compilation is not
GUI runtime or installation acceptance.

The stale management-operation test inventory was repaired to require the exact
33 frozen operations, reject duplicates, and keep every management operation
unknown to the client profile. Prior auxiliary-process cleanup failures and an
intentionally interrupted cold application check remain unsuccessful diagnostic
runs. Final qualifying invocations closed naturally without resource-floor
breaches. Future affected checks separate compilation from test execution to
avoid replaying long tests after compiler auxiliary-process cleanup failures.

The approved main-integration prerequisite is committed and pushed as
`09c3ad32550a156bdea6a72a4e7d0cfdb169f043`. The following continuation
increment is integrated and verified independently from that prerequisite.

## Verified T29 signed-source continuation increment

The Download credential now requires a closed signed `sourceBinding`: logical
source, immutable object and object version, plus SHA-256 of the exact outer URL
UTF-8 bytes. The verifier returns a privately constructed source identity only
after the existing complete signature, qualification and expiry checks. A renewed
signed URL can retain the same immutable identity; a changed source tuple starts
a fresh exclusive file even when its ETag matches. Legacy unbound credentials
are refused, as approved; the original twenty cryptographic vectors are unchanged.

Windows cache jobs now publish independently retainable checkpoint digests only
after flushing and rereading the actual held prefix. Resumption requires that
external pin, closed checkpoint, exact held file identity/length/hash, strong
ETag and the same original Package and signed source. The checked prefix is
copied into a new exclusive attempt; interruption, replacement and HTTP restart
preserve the original bytes. Cancellation remains latched. Full credential
validation runs again after local preparation and before the first GET; there
is no total or cumulative download deadline.

Independent deep source and repair reviews passed. Qualifying Windows results:
**157 codec, 71 verifier, 3 control HTTP, 45 workflow and 1 actual signed-package
consumer passes**, with one existing control-HTTP opt-in ignored. The workflow
includes all 21 new portable/Windows continuation and time-boundary tests. The
codec total combines 34 unchanged successful targets from the complete run with
the final three-target repair run; the earlier complete run remains failed.
The unchanged reference suite's previous **205 passes** remain applicable.
The complete application Windows all-targets check, architecture guard and
whitespace checks passed. Earlier stale-fixture, missing-dependency and MSVC
auxiliary-process cleanup failures remain recorded as unsuccessful runs.
Final qualifying commands closed naturally without resource-floor breaches.

The separate service pins the committed client contract
`5122bc7fd0e28f80b35fa0c54394ba1f97823ef1`; its contract and test adaptations
are committed and pushed as `378af2ea06b4666187d4ba842a89de05cd817640`. The normal export also includes
earlier committed scan-capture routes and Rollout abort-proof variants; copying
these descriptors introduces no new business owner. Fixed cryptographic vectors
remain unchanged. The old inventory assertions now require 59 routes and 84
floor/Rollout schemas, with exact checks for the four scan-capture operations.

The qualifying service default result combines **120 unchanged successful
packages** with the final repaired contracts package: **4,054 passing test
records**. The original complete run failed its two inventory assertions and
remains failed. Its **1,736 skipped records** are not passes. Two subsequent
targeted opt-ins each passed without skips: actual Node claims/Schema parity and
real isolated OpenBao Transit purpose-bound signing. Module integrity, all-package
`go vet`, all command builds and changed-file Go formatting passed. Final owned
command trees closed naturally without resource-floor breaches.
Test-owned compiled outputs and temporary directories remain locally because
automatic approval review rejected cleanup; no deletion was executed.

This increment does not supply production current qualification, protected checkpoint
storage or the authorized executor. T27's protected current acceptance and T26's
proven time source remain real production prerequisites; ordinary QA clock inputs
and cache observations do not replace them. Existing host staging and same-process
VerifiedExecutionObject custody are reused. T29 remains in progress at **75%**;
real remote file-service and installed-host E2E gaps remain explicitly unexecuted.

## Current T09 upload lifecycle, validation and storage increment

The durable validation owner imports an authenticated original received receipt,
creates one Artifact/job under the original T06 command, and retains exact
acknowledgment recovery. Physical upload uniqueness is enforced before a new
processing request can survive. Status/history keep their scoped read rights;
write-only uploaders can recover their own exact request. Jobs retain the same
native attempt across genuine lease expiry and renew while work is confirmable.
There is no total validation or download deadline.

Actual malformed ZIP bytes now commit a rejected Artifact, terminal job, result,
history and outbox together. Rootless products can reject objectively corrupt
archives; a sound archive without current trust remains blocked. Missing complete
admission evidence also remains blocked. That earlier increment exposed no Artifact-valid writer.
The original received acknowledgment and receipt are unchanged.

Independent source/repair reviews passed. An actual PostgreSQL failure exposed a
PL/pgSQL variable/column name collision in the rejection commit; the three-reference
rename was independently reviewed and the four affected native scenarios passed.
Together with the four unaffected successful native scenarios, this covers eight
top-level native cases plus one subcase. The earlier failed invocation remains
failed. A real TLS/OIDC/T05/T06/PostgreSQL browser workflow passed all five cases
without skips or retries and checked the exact original business inventory.
The subsequent diagnostic-directory and fail-fast test-only changes preserve the
same strict five-case success condition.

Separate focused verification passed: replica proof 8 top-level / 36 total test
records; provenance/scanner admission 22 / 96; 11 pure admin contracts plus
TypeScript and the admin build. Three Linux packages cross-compiled successfully;
no native Linux execution is claimed. Related domain/configuration/management
checks passed 136 top-level / 360 total records before the final SQL repair;
those pure checks do not certify native database behavior.

The official AWS S3 adapter passed independent source review, compilation and
7 top-level / 36 total unit records. It verifies actual owner, region and enabled
versioning, performs conditional writes and exact-version full readback, and
rechecks current object identity. The normal Go tools added eleven AWS/Smithy
modules without changing existing dependency versions/checksums. Its 5 GiB
single-PUT capacity is an adapter capability, never permanent package invalidity.
Actual AWS permissions/topology/persistence tests remain unexecuted. Aliyun OSS,
Tencent COS, WORM and production topology are not claimed by this adapter.

The preceding `5b3d80d` queue/storage increment's final common checks passed:
**4,272 passing / 1,737 skipped / zero failed**
Go test records (1,331 passing and 949 skipped top-level tests), all-package vet,
module integrity, every command build, the client contract pin and whitespace.
All qualifying command trees closed naturally without resource-floor breaches.
Skipped records are not passes. Automatic approval review rejected cleanup of
this increment's task-owned compiled outputs before execution (`blocked by
policy`); no inventory or deletion ran and the outputs remain locally. The
component is committed and pushed in the service repository as
`5b3d80d25f096821977940288dd16036368f0c6b`; the remote tracking revision
matches and the service tree is clean at this delivery boundary.
The upload lifecycle increment now records a committed revision-one uploading
Artifact before the first application file-body read. Actual authenticated
reception joins to the same Artifact at revision two without inventing a version
or job. Explicit first enqueue preserves its birth identity at revision three
and fixes one version/job; historical received imports keep their original IDs
and acknowledgments. Fixed Begin/Locate/Resume routes, accurate Context capability,
mandatory owner readiness and three-language administration form the full path.
Lost confirmations retain exact pending bytes; explicit continuation rechecks the
whole file and owner token. Original received responses retain their 24-hour policy.

Independent source/repair reviews passed. Backend domain verification passed
20 top-level / 27 total records, and 27 admin contracts plus TypeScript/build
passed. Final native regression passed 14 top-level / 17 records; nine actual
browser cases passed once without skips/retries, checking real committed
inventories. Earlier native failures remain failures: a missing receipt field
in the Join time guard was replaced by the actual reception committed time,
and a test-only nil callback was removed. Other reviewed repairs cover the
single-connection startup probe, mandatory lifecycle recovery and pre-Join
registration rollback. Temporary diagnostics were restored before acceptance.

This increment's common checks passed **4,274 default Go records / 1,756 skipped
/ zero failed**, with 1,329 passing and 963 skipped top-level tests; all-package
vet, module integrity, all command builds, client contract pin and whitespace
passed. Default verification did not enable unrelated opt-in integrations; the
actual PostgreSQL and nine browser cases are counted separately above. Final
canonical-profile acceptance also checks the canonical LF database profile;
its original five V1 SQL sources match their unchanged Git contents.

The upload-lifecycle increment is committed and pushed in the service repository
as `c89c35821f45ef5d03f249c42c7e15a07d468bc4`; the remote tracking revision matches
and that service tree is clean at this delivery boundary.

T09 is now estimated at **90%**, not complete. The integrated increment
adds genuine DSSE/SBOM/native/scanner-six-original/replica proof composition,
protected final admission and canonical/alias current sources, plus dedicated
quarantine preparation, two-person review, original-wire recovery and atomic T08
reference tightening. Historical received and enqueue acknowledgments remain
unchanged. No manual success input or old acknowledgment becomes current
qualification.

Current scoped checks passed 34 domain top-level / 121 total records;
valid-profile PostgreSQL controls passed 2 / 7, and quarantine/publication
installation, seven publisher-port and UTF-8 controls passed 1 / 13. These are
schema/codec/protected-source controls, not a complete scan-to-valid/quarantine
business E2E. Administration passed 20 pure cases and its build; HTTP boundary
checks passed 25 records; filesystem checks passed 14 records with one skip.
The four new real browser tests were collected but executed **zero** times;
collection is not acceptance.

The final common invocation closed naturally without resource-floor breaches:
**4,784 passed / 1,771 skipped / zero failed** default Go test records, with
2,416 top-level tests including skips. Linux command builds and Linux scanner
test-binary compilation, all-package vet, module verification, all command builds,
client contract pin and whitespace checks passed. Default skips do not qualify
opt-in/platform integrations, and Linux compilation is not native execution.

Independent FullPack primary Put/Open/full-hash/underlying-EOF/Close and every
replica's separate child readback, with signed identity/typed reference/report
binding, is integrated and covered by this final common run. Its added tests
exercise real signed-child extraction and corrupt/tail/EOF/Close/durability
failures, not a successful native scan or valid Artifact. Without an explicit
protected profile, all four generic target paths still return NativeUnsupported.
The new `pinvou3-desktop-deb.v1` profile, available only for the two exact Linux
DEB targets, adds actual bounded physical main-image checks with full TAR
inventory, fixed ordinary0755 path/ancestors, control identity, ELF62/183,
file-backed executable entry, ARM64 instruction alignment and Linux ABI rules.
Its exact scope/compiled rules and explicit XZ dictionary/block capacities join
the durable original admission binding; no arbitrary path or certificate input
is introduced. Bounded single-stream LZMA2 XZ uses checked original framing and
actual Reader2 consumption before two SDK passes, not a claimed aggregate heap
bound. Debian control is the version source, not an embedded ELF version.

These integrated sources passed the final common checks recorded below.
The first scoped invocation `73097` failed because of an unused verification-test
Debian import; a test-only removal corrected it. That original invocation remains
failed rather than environment-exempt. Actual release DEBs, original
root/systemd scanning and a positive Artifact.valid lifecycle are not verified
by the constructed parser fixtures. The new fixed Windows host increment below
adds actual bounded main-image/source checks; generic NSIS payload extraction
and DMG filesystem/application checks remain business gaps. Historical T04 role
inspection and helper/launcher behavior are unchanged. The **90%** estimate is still component
engineering coverage, not task completion or production admission success.
Actual Linux/root/systemd scan plus live-cloud replica E2E remains unexecuted;
that environment-only gap is distinct from the implementation gaps. Native
certificates remain optional. T10 risk and T46 release/Target qualification are
downstream boundaries, not Artifact.valid prerequisites; the genuine downstream
qualification producer is not supplied or replaced by this increment.
T09 still owes consuming and displaying genuine T46 evidence references bound
to the exact source/target, host platform, Artifact, activation mode, data/backup
range and finite preparation/execution budgets. The genuine producer is not yet
delivered; this integration gap does not make T46 an Artifact.valid prerequisite.
The Linux follow-up passed its final common checks and is committed/pushed in
service `e7e58fa`, with client progress `145540ee`. The current Windows increment
is scoped verification, not another completed task. The estimate does not change
the strict **8/47 (17.0%)** completion ratio or mark T09 complete.

## Linux profile follow-up verification

The first common invocation `22648` also remains failed: its trailing-slash
negative could not be constructed by archive/tar.Writer. The repaired fixture
changes the unique completed physical TAR header and recomputes its checksum,
then retains the original real-parser refusal assertion. No business rule was
weakened, and neither test failure is an environment exemption.

After the test-only repairs, the final common invocation closed naturally
without resource-floor breaches: 4,899 default Go test records passed,
1,771 were skipped and none failed; 2,440 top-level tests include skips.
Linux server and scanner-test binary compilation, all-package vet, module
integrity, all command builds, client contract pins and whitespace passed.
Normal Go test-cache reuse was retained for unchanged packages; the repaired
packaging tests reexecuted, including the physical trailing-slash negative.
These are code/format/configuration regressions, not authentic production-DEB,
Linux/root scanner, live-cloud, Artifact.valid or upgrade-mode acceptance.
The initial unused-import invocation remains failed as recorded above. New
rules do not replace an already-bound original configuration or unknown scan;
apply the compiled profile before a new explicit upload and its unique enqueue.

## Windows fixed-host tooling and admission-profile increment

The explicit `pinvou3-windows-host-source.v1` profile now handles canonical
`installer.exe`, `pinvou-upgrade-helper.exe`, `pinvou-upgrade-launcher.exe` and the
complete embedded-catalog `app/` files. Actual RCDATA 34101..34109 binds production
host Manifest v2 and inventory/shard v1 under the strict ASCII profile. The same
verified FullPack supplies actual x64 main/helper/launcher executable mappings;
main FileVersion/ProductVersion match the application version. Every declared
helper artifact shares the entire same physical helper identity, without duplicate
aliases; distinct alternate helpers remain unsupported. Zero-byte app resources
are allowed, native/evidence remain nonempty and resource ZIPs remain original
recursively inspected bytes.

The existing offline prepare/assemble/verify commands accept the optional local
`sourceProfile` plus relative `sourceDirectory` with `inputs: []`. Collection uses
the real installer's declared inventory, not a caller app list or whole local
directory census. Unrelated local source files are ignored; no protected T31
namespace or all-ancestor no-reparse proof is inferred. The final FullPack itself
still has exact membership. The protected exact-scope/rule reference binds the
admission configuration; Windows-only settings do not require XZ, while mixed
Linux profiles retain their original XZ capacities and canonical references.

Independent scoped source/repair reviews passed. Codec checks recorded
**115 passed / four skipped records**, across **28 top-level tests**. The actual
built CLI invocation closed naturally without a resource-floor breach: formatting,
build and packaging/CLI checks passed **188 records / three skips**, across
**35 top-level tests**. The new fixed-host CLI test and all eight subcases actually
performed prepare -> real external test signature -> assemble -> verify,
byte-identity checks, tamper/restoration, no overwrite and source refusal. This
proves an offline material process with constructed fixtures, not a production
installer, T31 runtime, original scanning, cloud replicas, new real browsers or a
positive Artifact.valid lifecycle. Skips are not passes.

The initial integrated core compile failed on an unused archive import and ran
zero tests; the exact import-only repair passed independent source review. Earlier
integration metadata/formatter preconditions also failed before any test ran.
Those unsuccessful invocations are retained as such. Separate owner/configuration
checks passed five top-level / five records, zero skips/failures, formatting and
whitespace in their qualified closed run. Final common checks and the final built-CLI rerun passed in their separate
qualifying invocation, recorded below. Generic NSIS payload and DMG filesystem/application
business gaps remain, as does genuine downstream T46 evidence consumption.
T09 stays **90%**, equal-task engineering **37.0%**, strict completion **8/47
(17.0%)**. No total download deadline or mandatory native certificate is added.


## Final Windows fixed-host regression

The final common invocation closed naturally without resource-floor breaches.
Default Go tests recorded **5,044 passed, 1,772 skipped and zero failed**, across
2,464 top-level tests including skips. Changed-source formatting, Linux server
and scanner-test binary compilation, all-package vet, module integrity, all
command builds, the client contract pin and whitespace checks passed. Normal
Go test-cache reuse covered 103 unchanged packages; skips are not passes and
Linux cross compilation is not Linux execution.

After those command builds, the final built upgrade-pack executable separately
passed the fixed-host CLI test and all eight subcases: **nine passed records,
zero skips/failures, one top-level test**. The default suite's skip for this
opt-in test is not counted as that separately executed pass. These results
remain constructed offline source/material verification, without a production
installer, T31 runtime, original scanner, live-cloud, browser or Artifact.valid
lifecycle claim. A runner syntax preflight was corrected before any common
stage started; it is not a test pass. Earlier unsuccessful runs remain recorded.

## Completed T07 managed-purpose lifecycle

The management service and three-language admin now cover all nineteen non-Root
roles with exact role-applicable dimensions. Timestamp, Snapshot and Release use
component scopes; Package also requires a target; Target and credentials require
both channel and target. Null means inapplicable, never a wildcard. Fresh
provisioning, registration and enablement are strict at management preparation
and the actual new T06 identity boundary. Retained original requests preserve
their exact scope, wire, hash and recovery identity; fresh legacy cleanup still
captures actual registered material or an existing epoch.

Independent deep source and repair reviews passed for the server, admin,
composed native tests and current documentation. Go compilation, 42 affected
domain test records, TypeScript/build and seven pure admin contracts passed.
Twenty qualifying PostgreSQL records passed: eighteen successful records from
the first invocation plus the repaired mixed lifecycle and new complete
Target-scope tests.
The original command containing the failed single-operation fixture remains
failed; no failed result was relabeled as successful.

The lifecycle test uses real owners for material registration, a Root retaining
old/new metadata-key policies, old stop/new enable, private renewal and final
Root. This new scenario retains the original Root signing members. Only old unissued epochs
use their real initial two-minute retention; signed epochs retain the actual
24-hour replay obligation plus margin. A live renewed graph, original immutable
Package and full historical archive still verify after old metadata keys leave
the current Root. The initial graph is a schema-owner fixture, not T14
activation. RAM signing/stop providers do not certify physical custody. Separate isolated
Transit tests verify the adapter's real RPC, permissions and recovery behavior;
production custody remains deferred.

Ten management-native records and all four real TLS/OIDC/OpenBao/browser
workflows passed. The key-material workflow includes six Playwright cases:
the original reviewed creation/recovery path, three full-scope/remount checks
and two local legacy unknown-wire recovery checks. A test-only JSON field-order
assertion failed in development; the reviewed semantic four-field correction
passed its affected browser group without rerunning the other three successful
workflows. Local legacy data is not evidence of old server acceptance.

Final default Go tests passed 4062 records / skipped 1738 / failed 0
(1274 passing and 948 skipped top-level tests). Contract pin, dependency
verification, full vet, all command builds, changed-Go formatting and whitespace
passed. Every qualifying invocation closed naturally without resource-floor
breaches. Initial fixture and browser assertion failures remain failed history.

Independent task-completeness review found no remaining T07 business code gap.
The service delivery is committed and pushed as
`5f3bab018ed201d6599400548337ff8a966821d7`.
Genuine old Target/null cross-version database handoff has not been run;
production custody/deployment remain deferred. These are disclosed verification
boundaries, not claimed passed scenarios. First activation and each credential's
business qualification remain with their respective downstream owners.

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
production qualification and integration with the later authorized executor
remain unfinished business implementation. The subsequent signed-source
continuation increment above closes the earlier wire-binding and explicit cache
resumption gap. No ETag or QA provider assertion is promoted to production
continuity authority. These components do not complete T29.

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

T29 now reflects the verified continuation increment. Outstanding environment-only E2E qualification is not a
completion blocker; outstanding business implementation is.

| Task | Estimate | Status | Delivery / outstanding dependency |
|---|---|---|---|
| T01 | 100% | Implemented; independently reviewed and verified | Executable metadata/credential Schemas, byte/trust profile, reference verifier, independently verified fixed vectors and local CLI E2E |
| T02 | 100% | Implemented; independently reviewed and verified | Closed control APIs, publication/lifecycle/selection/quality reference models and regression tests; no production service implementation |
| T03 | 35% | In progress | Reviewed synthetic freeze/encrypted backup lab, thirty retained 1 GiB development samples, two individual 10 GiB many-file development samples and three earlier payload smokes; the full four-target matrix, final contracts, exact production inventory, approved finite budgets and strict time qualification remain pending. |
| T04 | 65% | In progress | Offline tools, Windows post-build npm adapter and role-specific NSIS inspection are delivered; the integrated source passed default regression and current-CLI normal/wrong-Root cases. Complete provenance, product build integration and exact platform qualification remain pending. Native code-signing certificates are optional. |
| T05 | 100% | Implemented; independently reviewed and verified | Separate service management identity, MFA, scoped permissions, dual approval, protected audit and three-language admin; isolated actual Keycloak password/OTP/provider flows verified; deployment realm acceptance remains pending |
| T06 | 100% | Implemented; independently reviewed and verified | Durable atomic executor, encrypted recovery projections, SQL lease fencing and background recovery; later business commands and production HA acceptance remain pending |
| T07 | 100% | Implemented; independently reviewed and verified | Nineteen exact managed scopes, original recovery/cleanup, Root/history, mixed-current key rotation, actual retention, Target scope isolation and permanent retirement; native/browser/common checks passed. Old Target cross-version handoff remains unexecuted; production custody/deployment are deferred. |
| T08 | 100% | Implemented; independently reviewed and verified | Internal atomic publication/renewal, original T06 business composition, active deny/pending, durable job storage and callable reconciliation worker; independent review, native regressions, browser and common checks passed. T14 eligibility and host startup composition remain explicit separate boundaries. |
| T09 | 90% | In progress | Component estimate only. Upload/validation, independent FullPack custody and admission/quarantine machinery are delivered; Linux2 physical-main/XZ is committed in e7e58fa. The fixed Windows host source/profile and actual offline built-CLI workflow now have scoped verification; five owner/config checks, final common regression and the separate final built-CLI rerun passed. Generic NSIS payload, DMG filesystem/app and genuine downstream T46 integration remain. No production installer/scan-to-valid claim; four quarantine browsers remain collected, zero executed. |
| T10 | 90% | In progress | Scan capture/strict coverage plus formal Target/channel risk review, bounded exceptions, revocation/expiry, highest-current risk guards and the fixed T08 tightening adapter are delivered and independently reviewed/verified. Genuine selection restoration/widening still needs T14 actual qualification and publication composition. Linux/systemd and formal complete-lifecycle/browser E2E remain unexecuted and non-blocking. |
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
| T29 | 75% | In progress | Signed immutable-source/URL binding and independently pinned Windows cache continuation are verified alongside existing transfer, FullPack/native observation, protected staging, historical Root succession and same-process VerifiedExecutionObject custody; the service pins the committed contract and passes cross-language/real-Transit verification; protected production qualification/checkpoint ownership and authorized-executor composition remain pending. |
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
