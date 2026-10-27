# Upgrade implementation progress

Task source: [V1.0 development tasks](pinvou-upgrade-platform-development-tasks.zh-CN.md).
Checkpoint: **2026-10-27, Windows writer admission delivered; read-only journal diagnostics verified locally; full T28 preparation remains in progress**.

There are 47 tasks: eight completed, nineteen in progress, nineteen not completed,
and T42 explicitly deferred. The strict completion ratio is **8/47 (17.0%)**.
The equal-task engineering estimate is **38.7%** (1820/47). T07, T08, T09, T10, T13, T25, T28, T29 and T31 reflect their current delivery;
other estimates retain their 2026-10-22 assessment. These estimates are not
measured hours or release readiness.

## Delivery rules and active work

The current main task is **T28 Windows preparation**, with T26 original-budget
composition as its direct prerequisite. Other unfinished tasks are parked.
The current component connects an early GUI App startup barrier, the actual
authenticated service, protected writer journal, native Job custody and the
owner-approved restricted keeper. A bounded pending-release index preserves
retirement work across service restart. Scoped V3 review covers this component;
the strict completion count below is unchanged. T28 is now estimated at **10%**
for its delivered admission, journal and native custody foundations. This is a
component-based engineering estimate, not measured acceptance coverage.
CLI/headless admission, real Session preparation, Saved acknowledgements, complete
writer freezing and other platforms remain outside this App admission component
and are still required by T28. No new E2E tests are being run.

The delivered admission component's local verification passed the host's **188 tests, zero failed,
one existing ignored**, including ordinary owned native Job and Event fixtures.
Both normal-CRT library tests and static-CRT standalone all-target linked tests
closed naturally; helper/launcher harnesses contain no additional test cases.
All three real App library compositions compiled: default without the host,
default with `upgrade-host-production`, and no-default with `upgrade-host-lab`.
Ownership tests, CI-selection policy, architecture, scoped formatting, UI lint,
language tests, UI/Web builds and the Safari 14 compatibility audit passed;
unchanged-source evidence is inherited only for its original scope. A minimal
Event-ordering negative control failed the intended assertion; exact restoration
passed. Earlier runs requiring forced compiler-helper cleanup remain excluded.

The diagnostic increment adds the launcher's fixed `--writer-status`
command. It reads the authenticated service's protected journal in one
transaction and reports reserved, admitted, draining and pending-release counts.
It neither launches the GUI nor repairs state. Missing or corrupt state and
changed service identity return errors; zero counts are only journal data,
never proof of stopped writers, saved data or installation permission.
Its five independent source first reviews are frozen. Current library
compilation, architecture and scoped formatting passed. Exact-source cached
normal-CRT library and static-CRT all-target linked tests each passed **193 tests,
zero failed, one existing ignored**, with natural process cleanup; the two
configurations execute the same cases. Earlier cold runs requiring forced
compiler-helper cleanup remain excluded. Their normal/static cold builds
took about 15/17 minutes, while test execution took 19.6/15.7 seconds;
qualified cached runs took 22.7/19.8 seconds. Prior App assembly evidence is
inherited after independently checking unchanged callers, manifests, locks,
features and build inputs; it was not rerun.
No new E2E tests are scheduled.

Five independent first reports were frozen before findings were exchanged;
original-finder repair reviews and separate non-author adjudication apply to the
fixed component. Local compilation and owned native fixtures do not verify an
installed LocalSystem/restricted-keeper journey, multi-user recovery, current CI,
merge or release readiness. The next T28 slice must connect the genuine original
upgrade Session to all affected writers, durable saving, physical drain and
safe release. In particular, an admitted writer whose reply was lost must enter
the same preparation inventory before a later admission retry can proceed;
the present component has no native preparation producer proving that case.

The scoped T03 increment adds the versioned
[platform/backup observation contracts](../specs/upgrade/v1/platforms/README.md)
and [independent conformance program](../tools/upgrade-platform-conformance/README.md).
It defines **32** callable observation-audit operations and fixed acceptance /
counterexample pairs. The consumer invokes existing small synthetic SQLite,
writer-freeze and AEAD primitives while all production ports explicitly return
`unsupported`; a real synthetic freeze without an OS-wide admission fence is
rejected. A passing DATA audit cannot create native rights or prove its supplied
facts. No production Rust ABI, backup byte format, stage caller, real protected
backup/key owner or platform qualification is supplied.

Fixed-source Windows/Node26 validation passed **13 focused contract-module tests**,
the model report/CLI, architecture and whitespace checks. Two conformance-module
tests included real64KiB synthetic primitives with natural child close. Repairs
separate original/effective budget limits, allow only
tighter current Session cutoffs and preserve truthful overspent high-water;
health observations cover every attempt and required migrations. Three inherited
negative controls remove each-attempt, Session-no-extension and original-health-
start guards separately; one fresh control removes the canonical colon alphabet.
Each weakened copy fails its assertion and the restored copy passes. These are
four distinct controls, not additional end-to-end scenarios. The corrected
health-start negative keeps elapsed time valid.
Final cross-contract checks align canonical identifier punctuation with T01. A
direct committed-Schema regression covers this seam and rejection of invalid
ID/hash/version input, including trailing line terminators. The original
non-multiline patterns already reject these terminators; this is additional
regression coverage, not a separate lexical-defect repair or native trust proof.

Five independent V3 first reports were frozen before exchange. Original-finder
repair review and separate non-author adjudication govern the scoped delivery;
local verification is distinct from CI, merge and release qualification. Node22,
Linux/macOS runtime, full reference suite and current CI were not run locally.
No E2E, App/Rust build or dependency/lock change was added. T03 remains **35%**,
T26 **20%**, strict completion **8/47** and engineering **38.5%**; this audit
component does not complete either task or its production/platform milestones.

**T31 Windows helper/launcher host is implemented and pushed** in `18a241e1a`.
T25 network privacy is parked at **85%** after its complete public read/export
increment. The task graph also retains the
T33/T34/T35 platform-adapter dependencies; they are not completed by this Windows
increment. The previously verified
quality-purpose source, restricted Stage reader and three-language disclosure
batch is service `5d76` / client `aaf5cec7`; its historical verification remains
below. This increment adds the complete platform network-week public read
and export composition. Both approved policies remain decided: explicit
all-product/component/channel/target `operations.read` is required without
partial-scope filtering, and the Release report week is the protected original
workflow FirstStartedAt. Late events remain in that workflow/week; source and key
weeks remain original acceptance weeks.

This public increment passed the scoped and final checks recorded below.
T25 is **85%** (still in progress), increasing the overall engineering estimate
from 1800 to **1810/47 = 38.5%**. These estimates do not complete T25. The remaining
Release-purpose restricted owner needs a genuine T18 source selector/issuance
producer and authenticated source class; it cannot accept self-reported facts,
assume a default primary source or manufacture authority. T21's atomic quality
projection/cursor consumption and T24's report owner are their separate tasks,
not additional prerequisites for implementing the T25 ports.
The dependency audit found no production download-info issuance/selection source
behind T18; that path first needs T17 current qualification and T14 formal
baseline activation. T11/T12/T13 still lack their approved active sources, and
T11 needs genuine T46 local qualification. These other tasks remain parked.

### Initial engineering preparation policy (2026-10-26)

The owner approved configurable coordination/freeze **120 seconds** and backup
preparation **30 minutes** as initial engineering values for all targets, replacing
the measurement-before-initial-values development prerequisite. This is not
platform performance certification. The Windows host embeds the public build
inputs `PINVOU_HOST_PREP_FREEZE_MS` and `PINVOU_HOST_BACKUP_PREPARATION_MS`;
missing build inputs select these approved initial values, while invalid explicit
inputs fail the build. Runtime environment does not change protected policy.

The genuine original-install transaction writes the exact scope-bound preparation
policy and authenticates its precommit and committed readback. Existing/legacy
missing policy is never healed by load, update or a retry. New preparation windows
read actual protected values; existing windows retain their original first start,
spent high-water and the earlier Session cutoff. Four validation/execution
qualified durations remain absent; their maxima are not substituted. No actual
stage owner, backup producer or other-OS adapter is supplied by this increment.
Download has no total deadline.

Current fixed-source verification passed **163 host tests, zero failed, one
existing ignored**, including four new tests for default/override validation,
invalid inputs, actual owned-HKCU policy write/read/no-heal and missing execution
qualification. Natural helper/launcher builds, the non-static-CRT host library,
architecture, formatting and tracked/new-file whitespace passed. Exit-zero first
attempts requiring auxiliary-process termination remain excluded; only natural
retries are accepted. No UI or full application build was newly run: public host
API, App wiring, dependencies and UI are unchanged. E independently assessed the
scoped build evidence as proportionate; the previous default-App CI follow-up
remains outstanding. Actual Cargo override/invalid/non-Unicode/rerun invocations
are unrun; those branches have direct static wiring plus parser/default-build
evidence. The original protected SCM/TxR/stage journey remains static-only.

Five independent V3 first reports are frozen. A found stale current prose claiming
all six durations were mandatory; that paragraph is corrected below. Original
finder/delivery delta review and separate non-author final adjudication qualify
the final object and actual evidence. No E2E was run; no final commit, CI, merge,
release, platform certification or whole-task completion is inferred here.
T03 stays **35%**, T26 **20%**, strict completion **8/47** and engineering **38.5%**.

### Current T26 protected anchor workflow (2026-10-26)

The approved cross-boot policy pauses time-dependent actions when continuity
cannot be proved, preserves original deadlines/spent budgets, conservatively
clears recovery-query secrets and requires authenticated fresh server time plus
successful original-deadline cleanup before resuming. The dedicated Root time
role and coordinated unreleased V1 update are approved; no legacy clock fallback
is provided. The owner approved configurable initial engineering preparation
budgets on2026-10-26:120seconds coordination/freeze and30minutes backup preparation
for all four targetKeys. Full platform measurements no longer precede initial
value development; platform qualification remains separate.

The delivered foundations are:

| Component | Delivery | Historical verification |
|---|---|---|
| Closed time contract and no-now verifier | Client `37ca93504` | Node 219; codec 160/verifier 75/HTTP 4, with one existing TLS opt-in ignored |
| Fixed original-Root transport | Client `9b2d182a5` | Node 11; affected codec 18; HTTP 9 including both real TLS tests, 71 POSTs across 27 operations and 17 Root GETs |
| Actual service time issuer and original Root reader | Service `d0e7fee94` | Scoped 100 including real PostgreSQL/OpenBao 1; default 5762 PASS/1828 SKIP, vet/build/contract checks |
| Actual Windows counters bound to the committed SCM incarnation | Client `de0fa18d2` | Host 103 PASS/one existing ignored, including four new math/raw-API tests; role builds and guards |
| Protected exact original Root custody and consumption recheck | Client `49e266a3e` | Host 103 PASS/one existing ignored; metadata-owner 7 PASS including one new binding-refusal test; role builds and guards |
| Startup acquisition and atomic protected anchor/deny/barrier | Client `434484d2` | Host 115 PASS/one existing ignored; role builds, non-static-CRT library and guards |
| Native interval and original-budget kernel | Client `f640ea03` | Host 132 PASS/one existing ignored, role builds, non-static-CRT library and guards |
| Scoped diagnostics, explicit refresh and fresh zero-retention custody | Client `dac926c46` | Five independent first reviews plus non-author adjudication; host 152 PASS/one existing ignored, Node 12, production-host application library and guards; default application check unverified with owner-approved CI follow-up |

These are separate historical runs with overlapping tests, not additive totals.

Durable policy-fault recovery was delivered in client `87698938f`.
Initial protected time policy was delivered in client `d4dbb1f3` after the final
independent V3 adjudication and exact reviewed-tree/remote/DCO checks. The current
scope records positive corruption of bounded native binary Time Policy bytes
while the original Time ledger, budget floor and never-enabled retention custody
remain valid. Its incident and Unknown/barrier successor share one native
transaction. Reopening, restoring Policy bytes and changing the SCM incarnation
do not erase an active incident. Only a fresh signed request-bound anchor and
the actual zero-custody completion transaction mark it recovered; the incident
remains in protected Time history. Ordinary reads remain side-effect-free.

This policy-fault increment has five independent V3 first reports. C found a
late-observation gap: policy damage during network/publication/cleanup could
escape the initial-only incident path. The repair surrounds the complete
attempt with the same authenticated positive-byte re-observer, after failed
native transactions unwind, and uses the latest verified owner snapshot.
Commit-unknown errors remain separate. C and B statically rechecked this fix;
the separate non-author final adjudication checks the exact final object and
closed receipts rather than counting reviewer votes.

The repaired fixed-source host suite passed **159 tests, zero failed, one
existing ignored**, with normal helper/launcher builds, non-static-CRT host
library, architecture, formatting and tracked/new-file whitespace checks.
The earlier version's **12 Node module tests**, project UI lint and UI build
remain applicable: UI product sources, dependencies, configuration and callers
did not change in the repair. They are inherited execution, not new runs or
additional test totals. The affected browser smoke assertion was corrected for
the recovery action and only syntax/lint checked; no browser/E2E ran. Initial
forced-auxiliary exit-zero host attempts remain excluded in favor of natural
retries. Native installed-SCM/TxR/crash/reboot recovery remains static-only.

Missing/wrong-type/unreadable policy, OS query failures and damaged
Time/history/retention/budget records do not masquerade as this positive-byte
fault observation. Other integrity failures remain diagnosed/refused; their
complete repair, nonzero secret/backup actions and other OS adapters are outside
this slice. The existing three-language action explicitly requests time after
repair; its ACK never proves recovery. No new E2E was run. No CI, merge, release,
whole-task acceptance or delivery commit is inferred here; T26 stays **20%**.

The preceding initial-policy increment provisioned the approved **30,000 ms RTT /
1,000 ms uncertainty** policy only inside the genuine original-install
transaction, with exact protected readback. Loads and refreshes retain mandatory
actual-policy reads and cannot initialize or heal existing/legacy scopes. Its
five independent V3 first reports, final non-author adjudication and closed
execution receipts qualified that exact delivery. Its host run passed **155 tests,
zero failed, one existing ignored**; normal helper/launcher builds and the
non-static-CRT host library check also passed. Architecture, formatting and
whitespace checks passed. These results supersede two resource-refused or
interrupted attempts, which remain failures. Initial exit-zero attempts that
required auxiliary-process termination are excluded; their natural-exit retries
provide the accepted test/build evidence. No new E2E was run. This private host
increment changed no application wiring, dependency or public API, so it did
not require another complete application build; the earlier diagnostic increment's
default-application CI follow-up remains outstanding. This is scoped local
validation, not CI, merge, release or whole-task acceptance; T26 remains **20%**.

The Root reader returns exact original canonical DATA, including expired/time-less
history; Missing is not chain-tail evidence. The fixed control origin, TLS,
no-proxy/no-redirect, canonical framing and per-object limits remain. There is no
total download deadline. Earlier invocation and functional failures were repaired;
they were not relabeled as environment passes. The original crypto vectors remain
unchanged.

The current Windows increment connects real BCrypt 32-byte nonces, fixed Root GET
and time POST, actual incarnation-bound counter observations and the dedicated
verifier to one native registry transaction. Fresh verification starts at the
protected accepted current Root and checks only new successors against current
deny; retained historical crypto is not retroactively invalidated by new deny.
Captured protected-policy denials are durably accumulated before network I/O.
Final publication rechecks actual host state, original Root custody, exact policy
bytes and prior time Head, then atomically saves accepted Root originals,
nondecreasing time floor, request/response/counter/incarnation evidence and
`cleanup_pending`. Post-COMMIT ambiguity poisons the in-memory publisher until a
genuine protected reopen; no guessed no-write retry or rollback is provided.

Time genesis is created only in the same actual transaction as a genuinely new
original installation intent. Existing/partial/legacy absence never initializes
itself or implies empty budgets, backups or secrets. The previous Host Head,
three bootstrap names, Seed and original CAS rules remain. Policy is an explicit
scope/genesis-bound protected registry value with no RTT/uncertainty defaults;
absence keeps time unavailable. Root bodies and request/response/policy evidence
use separate bounded native blobs rather than expanded strings in Head. Retained
successor originals have a checked aggregate 1 MiB implementation capacity,
excluding the independent original Root; exceeding it refuses acquisition and
preserves prior state. This capacity is unrelated to the server 32 MiB batch limit.

A single SCM-owned worker performs startup acquisition and retains the actual
Core context. Stop cancels its HTTP future and joins the owned runtime/thread.
There is no detached worker, periodic retry or fixed DNS stop-latency claim.
Network/policy failure preserves existing setup-only IPC while leaving time
unavailable. The saved interval is historical at its recorded native sample,
not commit-time UTC or a durable Known-clock/installation capability.

The current Windows companion implements a transaction-borrowed native interval
source. It advances the original signed interval with actual same-incarnation
suspend-inclusive counters, keeps enclosing floor/ceil bounds and refuses an
expired Root window, changed policy/deny, different incarnation, missing source
or a backward interval. No point-valued ordinary wall clock supplies authority.

Its protected budget kernel records ten fixed window kinds: coordination/freeze,
backup preparation, full validation, three execution purposes, the health total
and two checks, and consume waiting. The scope, original operation and kind select
one original window; reopening preserves its first interval, original cutoff and
spent high-water. Preparation keeps the earlier session restriction; explicit
qualified durations cannot exceed the validation/execution caps. Health retains
the original total/check lineage, at most two check identities and related
historical spending. Before requires the entire interval and retained spending
to be below the cutoff; spanning is Unknown and confirmed exhaustion is sticky.
There is no download budget.

Only actual protected same-transaction records supply health parents/siblings.
A separate protected, scope/genesis-bound policy supplies the two approved
engineering preparation values from genuine original registration. The four
validation/execution durations remain optional and unavailable until separately
qualified; their maxima are not defaults. Ordinary reads have no fallback and
this policy is not T03 certification. The budget namespace and initial policy are
created in the original fresh registration transaction. Missing existing/partial
namespaces are refused. Starting a new configured-duration window or tightening
one of those kinds requires the actual Budget Policy; absence cannot select a
default, and captured bytes are rechecked before publication. Existing Begin/Assess
instead retains the authenticated original recorded duration without reading that
Budget Policy. Native host, previous Head and retained history are checked for
atomic publication; post-COMMIT ambiguity remains outcome-unknown.

Every successful native sample is retained, including an initial sample when
the final sample becomes unavailable. Its lower bound advances the protected
global watermark and its upper bound/spending cannot be refunded. The final
missing sample still yields Unknown, or retains previously confirmed exhaustion.
Fresh signed time must include this watermark, with the maximum rechecked in
the final native transaction. Related health-child knowledge is read from actual
protected siblings when the total/next check reopens. These two cases were found
during independent review/self-review, repaired and independently re-reviewed.

These are budget intentions and as-of assessments. They are not proof that a
T28/T32/T36 stage started, stopped or succeeded. Those actual owners must
authenticate their original operation/session and compose first actions with
the budget port; no public IPC command, installer action or execution permission
is added. This kernel itself never releases the recovery barrier or infers a
backup/secret census. The separately verified fresh-install zero-custody path
below has its own actual native census and completion transaction.

Original-budget increment verification: 132 host tests passed, with one existing test ignored. The 17 new tests are included in 132: five live-interval/Root-window DATA controls, seven budget-model tests, three native index/policy DATA controls and two observation/health-family regressions. Helper/launcher builds, the non-static-CRT library check, formatting, architecture and whitespace passed. The unchanged service, public
contract, Node and dedicated-verifier suites were not repeated. The library
check remains distinct from a full desktop application build.

Actual installed-SCM, TxR crash/power-loss, suspend/reboot and protected-policy
provisioning E2E remain unexecuted in the available environment. These gaps are
non-blocking under the owner rule and are not passes. Production UTC accuracy,
a complete recovery cleanup and whole-platform qualification are not established.

### Trusted-time diagnostics and explicit refresh (2026-10-26)

The current increment adds a complete local diagnostic and explicit refresh
workflow. A separate native semantic capability exposes the Settings entry even
when the legacy application updater is disabled. The App command adapter uses
the real registered App-role client, authenticated pipe/session sequence and
protected native store. Web and unsupported builds remain disabled. Reading
status does not send a time HTTP request. Explicit refresh reserves one slot in
the existing owned worker; startup and concurrent clicks cannot create a retry
queue. Accepted is an activity acknowledgement, not Known time, cleanup success
or installation permission. The old application-update and trusted-deadline
capabilities remain disabled.

The protected read distinguishes ordinary source/read failure, a changed
incarnation, missing setup/policy and positively confirmed invalid records.
Transient Win32 query failures and legal size races cannot label component
damage. Each status read reopens actual protected sources; cached worker success
cannot manufacture a time result. Chinese, English and Japanese explain all
eight closed states (including the native zero-custody completion below),
explicit network access, accepted-only results and stable
Busy/unavailable errors. No raw private error details enter the view.

Independent review found and closed pre-ACK shared-read reuse, an older promise
clearing a newer request, synchronous effect state reset, and adapter A-B-A
observation reuse. The repaired bridge starts an actual post-ACK observation;
the component owns a separate source lifetime and rejects late unmounted results.
The exact changes and subsequent test-only DOM-scheduling corrections passed
non-author review.

Historical combined diagnostics and fresh-zero verification: host **149 PASS, one existing
ignored**. This includes the earlier 140 tests and nine new retention/history
tests, rather than adding overlapping run totals. Helper/launcher builds, the
compatible non-static-CRT host library and formatting passed. The affected Node
suite passed 12 tests: ten diagnostic tests, the bridge protocol test and language
coverage. Six real React/browser groups passed, covering all eight states in
three languages, explicit refresh/invalid ACK, adapter A-B-A/unmount races and
unsupported zero-call behavior. These use synthetic semantic adapters; they do
not certify the installed SCM service or actual control-plane time. Owned browser,
Vite server and temporary profile were closed and cleaned.

The scoped source/test lint, UI production build, architecture and tracked/new-file
whitespace checks passed. The fixture JSX has no matching ESLint configuration;
its actual Vite/React compilation and browser execution passed instead. Earlier
full UI lint found a new conditional-style error which was repaired; existing
unrelated chat JSDoc warnings remain outside this batch. No full-repository UI
lint pass is claimed.

The owner approved an application-compilation-only runtime reserve of **4 GiB**,
with the **7 GiB** start threshold unchanged; other guarded checks retain their
existing reserve. Earlier 6 GiB interruptions are historical failures, not passes.
A subsequent production-host application library check passed in 316.75 seconds;
the corresponding default check stopped at the guard after 181.8 seconds.

This review also checked a fixed Git source snapshot with exact submodule commits.
The production-host application library check passed naturally in 14.91 seconds.
Its preceding exit-zero attempt needed forced cleanup of a residual compiler
auxiliary and is not accepted evidence. The default application library check
again stopped at the approved 4 GiB guard after 221.02 seconds. A separate stable
Cargo profile experiment disabling codewhale-tui optimization/debug data also
stopped at that guard after 123.53 seconds. Both default checks remain
**unverified**, with no reported compiler error and all owned processes joined.
Static review of the default unsupported adapter supports the scoped code
judgment; it does not establish default application compilation or merge/release
qualification. No dependency or lockfile changed. The unchanged service and
public time-contract suites were not repeated.

The owner explicitly accepted this default-configuration validation gap for this
increment's commit and push after final independent review; applicable CI must
still verify that configuration. This acceptance does not label the check passed.

The V3 review fixed five independent first reports before sharing findings, then
used a non-author adjudicator to check findings, omissions and full-chain seams.
Its confirmed native-read issue is repaired: failed or bounded registry queries
remain unavailable, while positively observed invalid state remains corrupt.
The original finder and the state/reliability role rechecked the repair. Added
controls call the real native adapters, including invalid-handle failures and an
owned HKCU census with checked cleanup; no installed service is involved.

The final repaired fixed snapshot passed **152 host tests, zero failures, one
existing ignored test**, helper/launcher builds and the non-static-CRT host library
check. Exit-zero attempts requiring forced auxiliary-process cleanup are excluded;
only subsequent natural completions qualify. An earlier fixed-export run had nine
signing-fixture startup failures because its locked Node dependencies were absent;
after locked installation, the full suite passed. This is not a business-assertion
waiver. The current affected Node suite passed **12 tests**.
The final repaired production-host application library check also passed naturally
in **11.13 seconds**; its preceding forced-auxiliary-cleanup attempt is excluded.
Unchanged UI build, lint and browser results above are inherited historical
evidence, not new runs.
No additional E2E ran under the latest instruction. Actual changed-hunk and
complete-chain evidence is recorded separately from defects and remaining review.
These conclusions cover this diagnostic/fresh-zero increment, not the cumulative
branch, all of T26, CI, merge or release qualification.

### Fresh-original zero-retention custody and recovery (2026-10-26)

A genuinely new original-install transaction now also establishes fixed protected
retention custody. Its three producer families are explicitly NeverEnabled.
There is no producer activation, nonzero-object insertion or generic empty-census
callback. Existing/legacy/partial custody is never initialized or healed on read.

After persisting a fresh signed anchor with the barrier set, the actual native
publisher checks the fixed secret/copy, backup, timed-backup-staging and
decryption-key namespaces. It authenticates the real SCM incarnation, protected
policy, original anchor/history and live counter samples under the existing
guard and transaction. Only a genuine never-enabled zero-obligation inventory
can produce an atomic completion record and time-state successor. Ordinary
downloaded packages and download cache/staging are excluded; no download deadline
is introduced.

The completion binds inventory, policy, incarnation, both counters and the exact
preceding anchor revision/hash. Actual retained Time references and completion
records must match in both directions, including older completions after Unknown
or a new anchor. Older history is checked for canonical headers, hash-chain and
completion-reference integrity; this does not certify every old detached signed
payload. Current and immediately previous states load and validate their full
signed evidence. An old completion grants no current UTC or cleanup authority.
The bounded adapter refuses overflow without truncating or
deleting evidence. Independent review found and repaired the missing historical
reverse check; cross-version missing-record/hash/duplicate/chain controls passed.
Successful counter observations retain the global watermark even if a later
sample fails or the first scan exceeds its original-response-based ten-minute
ceiling. This does not qualify nonzero backup/secret cleanup.

A verified zero completion can clear this inventory's barrier and expose
cleanup_complete diagnostic DATA. Unknown or a new anchor restores the barrier.
The result is neither durable Known UTC nor preparation/installation permission.
Ambiguous post-commit reads remain outcome-unknown until a genuine reopen.
The host build first exposed a missing local Seed binding, and a new DATA test
selected the wrong member of its existing Root tuple; both were repaired without
weakening assertions. The independent review also closed the post-commit error
classification repair. These actual compile/test failures remain recorded.

Still required for T26 are integration with actual stage/record owners, approved
target budgets, actual nonzero backup/secret producers and cleanup, backup-specific
three-language retention/recovery notifications, remaining platform adapters and
the unverified default application check. Real installed-SCM/TxR crash/reboot/
protected-policy E2E remains unexecuted; the owner's latest rule stops further E2E
execution, without turning it into a pass. The zero-only workflow does not replace
these remaining owners.
T26 stays **20%**, the engineering estimate **1810/47 = 38.5%**, and strict
completion **8/47 (17.0%)**. T03 remains the only prerequisite currently being worked; the original
T33/T34/T35 platform dependencies remain pending, and other main tasks stay parked. No deployment or whole-T26 completion is claimed.

T09's approved controlled macOS measurement component remains
delivered; genuine downstream T46 production/consumption is still its parked gap.
T13's protected planning workflow is delivered in service `51630257` and client
progress `1805fcc`; its remaining active-floor/path/bridge work is parked.
The domain, protected persistence/private owner, current HTTP/host composition and
three-language administration form an independently reviewed and verified
planning workflow. Scoped PostgreSQL/HTTPS, pure and synthetic UI, all six genuine
three-language browser cases and final common checks passed as recorded below.
The dedicated macOS path preserves the actual whole paired provenance, separate
measurement authority and final current-policy/Root/deny guards. Real controlled
macOS/provider E2E is unexecuted; genuine downstream T46 producer/consumption
remains unimplemented. T09's finite Mach-O
physical-entry increment is committed and pushed in service `c609f303`, with client
progress `2426003d`; that parser does not complete DMG/App admission. Arbitrary NSIS
layouts are outside the delivered fixed first-party Windows host profile, rather
than an additional mandatory T09 implementation.
T10 channel risk review, exceptions and current-risk reads are delivered; its
remaining selection-widening composition stays parked until T14 supplies the real
qualification producer. The fixed T08 adapter is delivered. T09's independent
FullPack custody, fail-closed admission/quarantine machinery, Linux physical-main/XZ
(service `e7e58fa`, client `145540ee`) and finite Windows host FullPack/tooling
(service `66b1e7d5`, client `74d2cba0`) retain their previously recorded verification.
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

The owner's latest instruction (2026-10-26) stops all further E2E execution.
Do not launch browser journeys, installed-service or VM E2E for later development.
Retain earlier E2E results as historical evidence only, stating their version and
limits; unexecuted scenarios are not passes. Continue applicable module tests,
compilation and static checks. Functional failures must be repaired and verified.
Apply the supplied V3 review method: five independent first reports, fixed before
sharing findings, followed by a non-author independent adjudicator. Maintain
actual hunk and complete-chain coverage plus separate defect and remaining-review
ledgers. Scope code, functional evidence, merge and release conclusions separately;
this instruction does not bypass applicable CI, external review or branch rules.
Clean task-owned environments promptly after successful tests only when they are
no longer needed, all owned processes/handles have closed, and exact paths have
been verified. Preserve essential results and shared or unrelated resources.

Independently usable components may be committed after their own review and
applicable checks. A component, source review, build or single-platform test does
not complete its enclosing task. T42 deployment and T03's 50 GiB / 500,000-file
measurement remain explicitly deferred. Downloads have no fixed total deadline.
The 32 MiB limit applies to the joined new publication-signing batch; stored
graphs and package downloads have separate bounds.

## Current T25 complete public network-week read/export delivery

The public composition reuses the installed network-week owner, configured realm,
actual pool and T05 source factory. Fixed POST `/v1/admin/network-weeks/read` and
`/export` accept only the existing closed protocol-version/week request. Current
OIDC/session activity, Origin/CSRF and safe denial audit remain in place; callers
cannot supply a realm, product list, leaf filter, cursor or replacement body.

One same-pool serializable transaction authenticates the persisted snapshot and
its T05 MAC witness, joins the actual current protected product inventory with
the frozen record's original products, and authenticates current principal and
provision facts. Each product needs one explicit `operations.read` grant with all
component/channel/target breadth flags true. Null dimensions are exact, partial
grants are not combined into a wide grant, and empty inventory cannot bypass
authorization. The same transaction holds the current scope relation lock and
rejects filtered/replaced relation kinds, RLS/policies, inheritance/rules,
unexpected triggers, owner drift and runtime write privileges before this
census. The complete joined metadata/read costs keep the existing
4096-point / 32 MiB limit; errors do not return a partial or fabricated report.

Read and export share the same path and exact canonical public bytes. Export
adds only a fixed filename header. A stored unavailable result stays unavailable;
not-frozen absence does not create an operation or run a freeze. Admission-off
keeps authorized historical reads and the original private recovery owner. No
new Engine, schema, migration, permission, MFA rule or three-language T40 page is
introduced. Product/source/basis/HMAC details remain private. Cross-leaf prefix7,
k=20 and independent purposes are unchanged.

The scoped increment passed 51 test records (10 top-level tests), with no
failures or skips, plus one genuine PostgreSQL/T05/source/OpenBao-fixture
read/export acceptance test. The same native test was rerun after the Root
scope-inventory repair and passed: an actual RLS policy hiding a newly added
product makes both read/export methods return unavailable without a body; after exact policy
restoration the missing grant is denied, and real reprovisioning restores the
original frozen bytes. These repeated executions count as one native scenario.

Final required default Go regression passed 5674 test records and
skipped 1816 records, with zero failures (1693 top-level
passes and 1019 top-level skips). Formatting, go vet, go mod verify,
Windows and Linux server builds, the pinned client contract check and service
whitespace checks passed. All recorded command trees exited naturally with no
resource-floor trip. No frontend source changed, so the previous batch's UI and
TypeScript results are retained without claiming a new UI/typecheck pass.

The test source distinguishes real PostgreSQL/T05 publication and permission
checks from transport-only business stubs and schema-owner inventory/corruption
controls. A complete available historical-week calendar E2E remains unexecuted;
no synthetic accepted history is promoted into that result. The prior source and
Stage batch's qualified counts below are retained, not added again as new public
read/export acceptance.

T25's component estimate is **85%**, overall **1810/47 = 38.5%**;
strict **8/47 (17.0%)** and all task counts remain unchanged. Release-purpose
restricted contribution ownership remains unfinished until the real T18
selector/issuance path supplies the authenticated source class. It must refuse
self-reported source facts, a guessed primary source or fake current authority.
T24 remains a separate downstream report owner, not a T25 prerequisite. T25 is
not complete.

## Delivered T25 quality-purpose sources and Stage contribution increment

Independent `stage-quality` and `release-quality` accepted-event sources use
actual first-peer prefix material, separate original purpose/week OpenBao keys,
and mandatory same-original-COMMIT persistence. They do not require ASN or a
ready network-quality source and do not reuse `quality-prefix` HMACs. Missing
peer/material and safely classified source unavailability preserve a legal ACK
with explicit missingness; SQL, integrity, cancellation and incomplete costs
remain errors. Legacy receipts never acquire new source facts on replay.

The Stage DATA calculation fixes 1/1/7 quotas and the original window/group's
cross-week installation limit without metric/outcome overrides. It authenticates
no source by itself. The restricted Stage reader derives its real
workflow/T47/window/plan/group and complete related source closure from actual
protected rows and the original live pool token. Its transitive week/window
closure includes other groups' older allowances. Existing dangling window/fact
references are errors; missing current lineage never silently disappears.
Current verification requires the same actual reader, pool and live token.

Explicit owner pruning keeps only purpose, original accepted time and
irreversible record/source/window bindings needed to identify lost coverage.
It keeps no prefix HMAC, ASN, installation identity, outcome or source body.
Authenticated non-Stage markers and disconnected history do not block a current
Stage component; connected expired Stage detail remains unknown. The 30-day
retention rule still requires timely owner maintenance; no automatic schedule
or retained eligibility decision is supplied.

The T40-facing callable contract has complete Simplified Chinese/English/Japanese
purpose, data-form and retention copy through the real lazy i18n loader. Its
implemented-purpose field describes capability, never whether a deployment has
enabled collection. It adds no T40 settings UI, network request, native right or
collection/export authorization. SN selection remains separately disclosed.

The scoped selection passed 61 records (26 top-level tests), with three existing
PostgreSQL controls skipped because their separate opt-ins were absent. Four
actual source/replay PostgreSQL/OpenBao cases passed. The actual Stage/window
case then passed, including foreign/ended-token refusal, provider-independent
historical reading, dangling-window error, genuine SN-state constructor mode,
and pruning/coverage mechanics. Its first failure was a test intent passing SQL
NULL where the unchanged non-abort API requires an empty string; the exact
fixture repair was independently reviewed and verified.

Final default regression passed **5,624 records**, skipped **1,815**, and failed
zero (1,684 passed and 1,018 skipped top-level tests). Formatting, all-package vet,
module integrity, Windows and Linux service builds, client contract pin, UI
build, six disclosure tests, scoped and full UI lint, architecture and whitespace
checks passed. All cited successful executions closed naturally without a
resource-floor trip. Local dependency restoration followed the existing lock
with scripts disabled; package and lock files were unchanged. Two earlier full
UI lint attempts hit the memory floor; a 1,024 MiB Node heap completed the same
full lint without changing checks or guard limits.

Full TypeScript checking remains **failed with 11,876 diagnostics**. A read-only
comparison with the original three dictionaries and without the new module found
the same 11,876 diagnostics: 11,822 primary tuples matched exactly; 54 existing
missing-member errors retained their file, mapped line, column, code, member and
suggestion, with only inferred dictionary type previews changed. There were no
new primary diagnostic sites or missing members in this comparison. The strict
exact-message comparison still failed with 54 replacements; it is not a full
typecheck pass, and span lengths/related-information ranges were not compared.

The previous week-owner batch counts below remain historical. Genuine complete
available historical-week calendar E2E remains NOT RUN; no backdated accepted
source is introduced. The genuine SN-state check is constructor/mode coverage
on an ordinary plan, not an SN-selected-plan positive or T14/T17 qualification.

The verified source/Stage-port/disclosure coverage increases T25's component
estimate from 65% to **75%**, overall **1800/47 = 38.3%**. Strict completion remains
**8/47 (17.0%)**, with task status counts unchanged. This is an engineering
estimate, not measured hours or whole-task acceptance. At that checkpoint, T25 still owed its release-purpose restricted contribution
owner and the decided full-scope public report/read/export implementation. T21 owns
atomic quality projection/cursor consumption; T24 owns release-quality report
calculation/freeze. Their incompleteness is not a prerequisite excuse for the
T25 ports. T25 is not complete.

## Delivered T25 protected internal network-week ownership increment

The source-reviewed component owns one immutable result for each complete ISO
UTC week in its explicitly installed realm. It authenticates the original legal
event, receipt, source-v2 and historical ASN/week-material associations, rather
than accepting caller-provided evidence or consulting today's source state as a
substitute for historical authority. Original server acceptance selects the week
and day; event occurrence and ID order the fixed five-dimensional contributions.
Scope/leaf/week and prefix/leaf/day limits, the cross-leaf prefix cap of seven,
and k=20 suppression are applied to the complete input set. The freeze gate uses
actual SQL time after week-end plus seven days. Missing source, a midweek cutover,
retention loss or unresolved ordering yields an immutable unavailable whole week,
never partial counts; integrity, SQL, cancellation and capacity errors remain errors.

A fixed T05 machine source port and exact native/MAC reverse census authenticate
the installation and frozen record. Initial/final source and audit costs join
under the 32 MiB/4096-point metadata bound. The same original T06 transaction
rechecks source basis, products, parent/lease/epoch and public body, then inserts
the frozen snapshot and calls original Complete last. Failure rolls the audit,
snapshot and parent back together. Frozen read/replay returns the same stored
canonical bytes; it does not reopen sources or recompute a report.

The explicit local operator installs or freezes using its closed seven-field
configuration and emits only state. Host configuration keeps realm selection and
new-admission separate: admission-off retains installed recovery/internal reads;
startup never migrates. The actual private recovery/due loops cancel and join,
and the fixed week command joins the existing nineteen private names for unknown
classification only. Common worker Keys remain unchanged. No public HTTP
read/export, wildcard grant, partial-scope report or cross-purpose qualification
was introduced in that internal-only batch; the subsequently approved public
authorization policy and current implementation candidate are recorded above.

Current scoped verification: **82 passed, zero skipped and zero failed**, across
39 top-level cases. This combines the public/domain and T05 port controls, source
reader, owner/CLI and host tests with two genuine PostgreSQL native cases; it
does not add failed attempts, duplicate reruns or older component results.

The native cases verified an actual due historical coverage-gap freeze, original
T05/parent closure, exact replay with admission disabled and runtime parent UPDATE
rights absent; missing or contradictory frozen rows were rejected and restored
exactly. A real direct-TCP accepted event exercised original receipt/outbox/family,
historical ASN and material source costs, same-callback and foreign-token guards,
and stable historical rereads after closing the provider and actually disabling
the current ASN source. These controls did not fabricate an available old week.

Final common checks passed: default tests **5,585 passed, 1,822 skipped, zero
failed** (1,659 passing and 1,022 skipped top-level cases), all-package vet,
module verification, Linux server build, Windows and Linux network-week CLI
builds, client contract-pin verification and whitespace. Default-suite skips
include platform, capability and explicit opt-in cases; the two new native
cases were separately executed and passed. All command trees closed naturally
and the existing resource floor did not trip.

A genuine complete available historical week does not yet exist in this freshly
installed test environment. That calendar-dependent E2E is **NOT RUN**; complete
calculation and k=20 boundaries have pure controls. No backdated source or accepted
event is used to claim that E2E passed. This environment limitation follows the
owner's rule allowing unavailable E2E to remain nonblocking, without waiving
functional failures.

The initial runtime permission refusal was repaired by reusing the original
T06 parent-row lock rather than requiring direct UPDATE rights. The repaired
native test proves those rights remain absent. Failed invocations remain failed;
their repaired controls were rerun, and older delivered component results below
are not counted again as this increment's acceptance.

This component estimate changes T25 from 55% to **65%**, increasing the equal-task
estimate from 1780 to 1790 points: **1790/47 = 38.1%**. Strict completion remains
**8/47 (17.0%)**; eighteen tasks remain in progress and all other task estimates
and counts are unchanged. Protected internal freeze is not ordinary public
report delivery or completion of T25. At that checkpoint, full-scope public
permission/report wiring, stage/release-purpose restricted ports and the
three-language disclosure contract remained T25 work; the current batch status
is recorded above. The T21/T24 quality/report consumers are separate tasks
and do not become additional prerequisites for completing the T25 ports.

## Delivered T25 accepted-event network-source increment

The actual event path uses the immediate TCP connection capability, protected
offline ASN selection and ready per-purpose/per-week OpenBao HMAC material.
Collection occurs outside the final SQL transaction; the same original final
transaction rechecks current owners and writes its version-2 source record.
The original acceptance time, installed CurrentVersion, receipt, ACK and
version-1 attribution remain authoritative and unchanged.

A separate authenticated namespace requires the source/missingness stage and
revokes the four old runtime publishers. Capture defaults off; an installed
receiver still records unavailable facts for new legal events. Missing peer or
ASN coverage preserves a legal ACK; original replay and pre-cutover receipts
cannot acquire later source facts. Actual accounting remains part of the joined
32 MiB/4096-point metadata bound. Raw IP/prefix values are not persisted.
Explicit weekly-material CLI provisioning and source installation/pruning are
documented in the service's network-event-sources.md.

Independent reviews found and Root repaired array encoding in the new SQL
profile, retained cost accounting for known missing ASN coverage, week-boundary
comparison and configured-vs-installed realm binding. The source-private error
wrapper preserves actual internal causes while HTTP keeps its closed response
codes; diagnostics expose no provider messages or source data.

Scoped verification passed 108 test records across 34 top-level tests,
with zero skips or failures. This combines attribution, current source/CLI
foundations, private collector/native definitions and six real source-integration
controls. The latter cover actual TCP/ASN/week-HMAC acceptance and exact replay,
missing peer, real ASN coverage gaps with retained costs, pre-cutover history,
default-off capture with old-publisher refusal, and protected retention.
The separate ASN foundation control also exercises real PostgreSQL. Go/SQL
canonical byte equality and resource-limit refusal were checked on the actual
new namespace. Earlier failed invocations remain failed history; the initial
legal-event 503 was repaired rather than treated as an environment exemption.

The required default regression passed 5,505 records, skipped 1,820 and failed
zero (1,622 passed and 1,020 skipped top-level tests). Skips include existing
opt-in/platform/capability entries; they were not executed passes. All-package
vet, module integrity, Linux service build, both operator CLI builds, exact
client contract pin and whitespace checks passed. All successful invocations
closed naturally without a resource-floor trip before result inspection.

The owner-only expired-row forms exercise the real retention transaction,
rollback and minimal tombstones; they are explicitly synthetic storage data,
not historical accepted events or production source evidence. No proxy topology,
production deployment or real user traffic has been qualified. Ready source
rows do not authorize publishing partial statistics. At that checkpoint,
contribution limits, cross-purpose qualification ports, completeness/k-suppression,
seven-day freeze and fixed privacy-report integration remained unfinished.

At that checkpoint, the component estimate changed T25 from 45% to **55%**, and
the equal-task estimate from 1770 to 1780 points: **1780/47 = 37.9%**. Strict completion remains
**8/47 (17.0%)**; all other task estimates and counts remain unchanged.

## Verified T09 dedicated macOS measurement increment

The finite macOS observer checks the actual built FullPack and its sole matching
final DMG, then measures the actual read-only mounted app rather than a neighboring
build output. Fixed ULMO/UDZO and HFS+/APFS, original XML Info.plist and the finite
universal2 main-entry profile are supported. Binary plists and unhandled variants
remain explicitly Unsupported, not environmental exemptions. Source cleanup and
input/tool rechecks must finish before the same-process producer can sign.

The dedicated DSSE purpose/key/observer revision is independently pinned; the
pipeline identity stays the original build pipeline. Actual whole provenance
JSONL contains the original SLSA record first and dedicated measurement second.
The service authenticates both exact records and their real Package/FullPack/DMG
bindings, then rechecks current policy/Root/deny in the original final transaction
without native/provider/file I/O there. T10's first-statement provenance observation
and second-record syntax checks confer no native authority. Ordinary provenance,
local JSON or a controlled CI assertion alone is not independent proof that an
authorized producer cannot lie.

The two non-overlapping scoped groups passed 545 and 30 test records:
575 passed, two skipped, zero failed (159 passed top-level tests).
The two skips are the existing opt-in actual NSIS and actual Windows-host CLI
fixtures; those scenarios were not executed by this invocation. The initial
57-record codec scope is superseded, not added. Independent source review also
found and repaired the Package ID identifier-vs-digest mismatch, preserved
capacity/cancellation error classes, and removed raw destructive process-group
signals across the Wait/reap boundary; their regression controls passed.

Both Intel/amd64 and Apple Silicon/arm64 macOS CLI builds and native-adapter
test-binary compilation passed. These are compilation checks, not actual macOS
mounting or process-cleanup execution.

The required full default regression passed 5,428 test records, skipped 1,813,
and failed zero (1,601 passed and 1,013 skipped top-level tests). Skips cover
existing opt-in/platform/capability cases and are not executed passes.
All-package vet, module integrity, Linux service build, exact client contract
pin and whitespace checks passed. All invocations closed naturally without
a resource-floor trip.

No real controlled macOS environment or protected measurement key is configured;
actual mount/provider-positive and scan-to-valid E2E remain unexecuted. The
project's non-blocking environment policy permits delivery with that limitation
recorded, never passed. Genuine downstream T46 producer and reference
consumption/display remain a business implementation gap, not an Artifact.valid
prerequisite. Native certificates remain optional.

The T09 engineering estimate is **95%**, up from 90% for this independently
usable component: estimate sum 1765 -> 1770, **1770/47 = 37.7%**. This is component coverage, not certification or
whole T09 completion. Strict completion remains **8/47 (17.0%)**; task counts and
all other task estimates are unchanged.

## Delivered T25 immutable accepted-event attribution increment

The protected archive now retains the original legal event's server acceptance
SQL timestamp and the original workflow's installed `CurrentVersion`, with
immutable replay, legacy absence, thirty-day detail retention and irreversible
tombstones. It preserves the original T20 event, signed input, T06 operation
and ACK. Each record explicitly says `networkEvidence: not-captured`; a replay
cannot fill missing peer, prefix, ASN or HMAC facts.

Explicit owner installation/pruning and installed event-seven private recovery
are integrated with the current floor-one, scan-one and formal-ten owners.
The 19 private names affect global unknown-command classification only; the
common worker retains its original due keys. Runtime connections must explicitly
place `pg_temp` after `pg_catalog`; unsafe configuration is refused before capture.

Independent code and repair reviews passed. The original accepted-history
control passed nine Go test records, and the genuine installed HTTP lost-output
and original-retry control passed one. The follow-up scoped invocation passed
174 records across 52 top-level tests, with zero skips and failures. This includes
single-connection reuse, remaining original owners, native cost/permission
controls, pure domain/adapters, current host composition and existing HTTP
regressions. Uninstalled HTTP regressions and classification-only due SQL
specimens do not establish installed private-command business acceptance.

Initial failed runs remain recorded. The SQL parameter-qualification repair,
explicit safe runtime search-path precondition and test-only real-result-ID
repair were independently reviewed and reverified. Original safety checks were
retained; temporary diagnostics were removed.

Final common: 5,286 default Go records passed, 1,803 skipped, zero failed
(2,567 top-level tests); skipped scenarios were not executed and are not passes
(their reasons include opt-in and platform or capability availability). Whole-repository
vet, module integrity, Linux server and attribution CLI builds, client contract
pin and whitespace checks passed. All seven command trees closed naturally
without a memory-floor breach before result inspection.

That independently usable 55-file component was delivered in service
`70da28f3` and client progress `fa009a16`, raising T25 from 35% to **45%**.
At that checkpoint, real first-edge capture, protected ASN/purpose-week HMAC
consumption, contribution limits and frozen privacy snapshots remained
unimplemented. The engineering estimate sum was 1765, or **37.6%** across 47 tasks;
strict completion remained **8/47**.

## Verified T25 protected source-consumption increment

The finite direct-peer adapter is integrated with the actual server connection
context. Acquisition defaults off; explicit `direct-peer.v1` observes only the
immediate TCP peer and never trusts forwarding headers or a client field.
Protected ASN and weekly-material Runtime capture/recheck interfaces now bind
actual authenticated sources to a live original same-pool transaction. Store
Borrow checks actual pool provenance and callback lifetime; detached results and
old Status DTOs cannot mint these captures. Opaque wrappers redact formatting
and cannot be restored from JSON/text.

ASN rechecks selected source/active head and expiry using SQL time without file
I/O in the final transaction. Weekly material rechecks the exact realm/purpose/
ISO week and immutable ready m1, returning semantic reads and both exact cost
phases/framing. Provider HMAC use remains outside the transaction. The final event
must still perform its real joint 32 MiB metadata union; separate source budgets
are not an event qualification or a hard download deadline.

Peer-focused scoped checks recorded 551 passes across 112 top-level tests, zero
skips/failures. Earlier invocations remain failed history; privacy fallback,
pointer-receiver test syntax and extra-format marker assumptions were repaired
and reverified without an environmental exemption. The original six redaction
marker assertions and all directives' no-secret assertions remain.
Scoped source-consumption: 92 passed, one subprocess-helper entry skipped, zero failed
across 48 top-level tests. The real ASN/material/provider packages ran at parallelism
two under the resource guard. Successful unchanged scopes were reused; earlier
functional and test failures remain history, not environmental exemptions.
Final common checks passed: 5,316 default Go records passed, 1,813 skipped,
zero failed (2,587 top-level tests). Skipped records are unexecuted entries,
not passes. Whole-repository vet, module integrity, Linux server and attribution
CLI builds, client contract pin and whitespace checks passed. All seven command
trees closed naturally without a memory-floor breach before result inspection.

This increment does not yet consume these sources in the accepted-event owner.
Persisted attribution still says `networkEvidence: not-captured`; original-ACK
missingness/watermarks, the next attribution migration, contribution rules,
frozen statistics and external queries remain unimplemented. At that delivered
checkpoint T25 stayed **45%**, the engineering estimate **37.6%**, and strict
completion **8/47 (17.0%)**.

## Verified T13 planning-draft workflow increment

The independently reviewed planning domain and wire, T05 admission witness,
private SQL owner/recovery, fixed six-route HTTP API and three-language UI are now
integrated against the current service. Current host wiring keeps scan1, formal10
and floor1 in exact global unknown-command classification only; the common
worker retains its original Keys, and private recovery is canceled and joined
independently. Existing T09/T10 management entries and full-scope callers remain.

This workflow saves declared planning inputs with a deterministic scope head and
immutable revisions, durable saved/stale outcomes and exact-original lookup and
Resume. A proven request-bound no-admit fact permits explicit correction with a
new key; an unknown response or absent lookup never does. None of these facts
establishes an active floor, current path or publication permission.

The existing six-route contract pin is unchanged. No result from an old component
run or another owner is counted as this batch's passing workflow.

Independent source and repair reviews passed. The naturally closed scoped
invocation recorded 155 passed Go test records, one skipped and zero failed,
across 75 top-level tests including skips. It exercised actual PostgreSQL,
original TLS/signed-OIDC HTTPS, concurrent CAS, authority and private recovery.
The new genuine-browser opt-in was skipped there and executed separately below.
All 17 pure floor UI cases passed without skips or retries; the admin build also
passed. An unchanged management browser case was skipped in that pure invocation.

The synthetic UI follow-up passed all 11 controls without skips or retries.
The separate genuine browser invocation passed both fixed cases in English,
Simplified Chinese and Japanese: six browser cases, zero skips/retries/failures.
Its Go caller passed one top-level and three locale subtests, including each
locale's exact original/revision/outcome/MAC census and current readiness.
Synthetic API controls do not establish authentication or database behavior.

Initial synthetic and genuine invocations remain failed history. Their test-only
repairs corrected the expected canonical JSON key order and awaited observable
correction completion before checking pending storage. Original request, new-key,
permission and native-state assertions were retained. A one-locale diagnostic
also failed before the latter repair; no failure is relabeled an environment pass.

The final cleared-opt-in regression recorded 5,207 passed, 1,788 skipped and zero
failed Go test records, across 2,531 top-level tests including skips. All-package
vet, module integrity, Linux server build, native floor CLI build, the unchanged
client contract pin and whitespace checks passed. All original outer command
trees closed naturally without resource-floor breaches before output review.
Cross compilation is not Linux execution; skipped records are not passes.

The completed planning workflow raises the component estimate for T13 from 10%
to **25%**. Active-floor approval/publication, ordinary prerequisite paths and
historical bridge eligibility remain unimplemented. At that checkpoint the estimate sum was 1755,
or **37.3%** across 47 tasks; strict completion stays **8/47 (17.0%)**.

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

## Historical T09 upload, validation and native-profile checkpoints

The following T09 Linux/Windows/Mach-O checkpoints retain their delivery-time
states and estimates, including then-open macOS implementation gaps. Those older
estimates and gap descriptions are superseded by the current dedicated macOS
measurement section above and the task table below. Original checks and failures
are retained as historical evidence.

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
adds actual bounded main-image/source checks. Other NSIS scripts/layouts remain
explicitly unsupported, not a separate required extractor. Complete DMG
filesystem/application checks remain an implementation gap. Historical T04 role
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
qualifying invocation, recorded below. Other NSIS scripts/layouts remain explicitly
unsupported outside the fixed host profile. Complete DMG filesystem/application
checks and genuine downstream T46 reference consumption/display remain to implement.
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

## Finite universal2 Mach-O physical-entry increment

The additive owned-byte reader now checks a bounded universal2 MH_EXECUTE profile:
exact x86_64/subtype 3 and arm64/subtype 0 slices, a unique LC_MAIN per slice,
non-overlapping bounded file/VM segments, an executable header-covering `__TEXT`,
and actual file-backed entry bytes. ARM entries require aligned four-byte spans;
the original FAT base is applied once. Other ABI/container/thread-entry variants
are explicitly unsupported. The existing framing API remains unchanged.

The naturally closed, resource-qualified Mach-O/packaging/verification scoped run
recorded **323 passed, 22 skipped and zero failed**, across 116 top-level tests.

The separate final required invocation also closed naturally without resource-floor
breaches: default Go tests recorded **5,084 passed, 1,772 skipped and zero failed**,
across 2,468 top-level tests including skips. All-package vet, module integrity,
server build, the client contract pin and whitespace checks passed. The four new
Mach-O top-level tests account for 40 passed records. Skips are not passes; these
checks do not execute a macOS app or complete DMG admission.
This does not establish compatibility with an authentic Tauri universal2 binary,
a macOS run, a DMG filesystem or AppBundle/Info.plist, a certificate or a positive
macOS Artifact.valid lifecycle. At that checkpoint, complete DMG/App inspection
was an implementation gap. The owner had approved a controlled macOS CI producer
with dedicated signed measurement of the actual DMG/BundleInfo/main Mach-O and
native read-only cleanup. Its implementation, producer and service evidence
consumption were still pending; ordinary provenance could not replace that
evidence. T46 reference consumption/display remained downstream and was not an
Artifact.valid prerequisite. The checkpoint estimate was T09 **90%**, engineering
**37.0%**, and strict completion **8/47 (17.0%)**. The current implemented finite
measurement path and still-unexecuted native E2E are recorded above.

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
| T03 | 35% | In progress | Reviewed synthetic freeze/encrypted backup lab, thirty retained 1 GiB development samples, two individual 10 GiB many-file development samples and three earlier payload smokes; initial engineering budgets are approved. Final contracts, exact production inventory, actual platform measurements across the four targets and real time-source/unknown/recovery qualification remain pending. |
| T04 | 65% | In progress | Offline tools, Windows post-build npm adapter and role-specific NSIS inspection are delivered; the integrated source passed default regression and current-CLI normal/wrong-Root cases. Complete provenance, product build integration and exact platform qualification remain pending. Native code-signing certificates are optional. |
| T05 | 100% | Implemented; independently reviewed and verified | Separate service management identity, MFA, scoped permissions, dual approval, protected audit and three-language admin; isolated actual Keycloak password/OTP/provider flows verified; deployment realm acceptance remains pending |
| T06 | 100% | Implemented; independently reviewed and verified | Durable atomic executor, encrypted recovery projections, SQL lease fencing and background recovery; later business commands and production HA acceptance remain pending |
| T07 | 100% | Implemented; independently reviewed and verified | Nineteen exact managed scopes, original recovery/cleanup, Root/history, mixed-current key rotation, actual retention, Target scope isolation and permanent retirement; native/browser/common checks passed. Old Target cross-version handoff remains unexecuted; production custody/deployment are deferred. |
| T08 | 100% | Implemented; independently reviewed and verified | Internal atomic publication/renewal, original T06 business composition, active deny/pending, durable job storage and callable reconciliation worker; independent review, native regressions, browser and common checks passed. T14 eligibility and host startup composition remain explicit separate boundaries. |
| T09 | 95% | In progress | Component estimate only; independent source review, scoped verification and required common checks passed. Durable upload/validation, FullPack custody, finite Linux/Windows native profiles and admission/quarantine remain delivered. Dedicated macOS readonly observer, protected-purpose same-process signing and exact two-record service consumption are independently reviewed and verified; 575 scoped passes/2 skips plus full 5428 passes/1813 skips and both macOS target compiles are not an authentic macOS/provider or scan-to-valid positive. ULMO/UDZO, HFS+/APFS and original XML plist are finite support; unsupported variants are not environment exemptions. Genuine downstream T46 producer and reference consumption/display remain unimplemented; no complete T09/certification claim. |
| T10 | 90% | In progress | Scan capture/strict coverage plus formal Target/channel risk review, bounded exceptions, revocation/expiry, highest-current risk guards and the fixed T08 tightening adapter are delivered and independently reviewed/verified. Genuine selection restoration/widening still needs T14 actual qualification and publication composition. Linux/systemd and formal complete-lifecycle/browser E2E remain unexecuted and non-blocking. |
| T11 | 65% | In progress | Protected multi-platform drafts, reviewed content freezing, individual Target cancellation, atomic mixed parent closure, authenticated history and original-request recovery with three-language administration; package qualification and Release/Target lifecycle approval/publication remain pending. |
| T12 | 55% | In progress | Reviewed stable-facts matching and forward-policy primitives plus protected immutable candidate history, preparation, independent review and original-key recovery with three-language administration; formal Registry publication, capability admission and cleanup permissions remain pending |
| T13 | 25% | In progress | Protected planning-draft persistence, immutable history, exact-original recovery, fixed HTTP and three-language administration are implemented, independently reviewed and verified with actual PostgreSQL/HTTPS and six genuine browser cases. Active-floor approvals/publication, ordinary prerequisite paths and historical bridge eligibility remain unimplemented. |
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
| T25 | 85% | In progress | Delivered full-platform network-week read/export requires current all-product/component/channel/target operations.read and returns the same authenticated canonical body. Scoped51 plus native1 and the final required Go/build/contract checks passed as recorded above. Source-v2, protected internal freeze/CLI/host, independent Stage/Release capture, restricted Stage reader and callable three-language disclosure remain delivered with their historical verification above. The Release-purpose restricted owner still needs genuine T18 selector/issuance source-class production; self-reported facts/default primary/fake authority cannot replace it. T21 atomic projection/cursor consumption and T24 report ownership remain separate tasks, not T25 prerequisites. |
| T26 | 20% | In progress | Nonce-bound issuer/verifier, fixed Root transport, Windows incarnation/Root custody and protected anchor/deny/barrier are delivered. The current native interval/original-budget kernel preserves scope/operation/kind, original cutoffs, spending and global time watermark; its timing intentions do not prove actual stage execution. Local diagnostics, explicit refresh and actual fresh-install never-enabled zero-custody completion have current host/Node module verification and inherited synthetic React/browser evidence; the production-host application library check passed, while default application compilation remains unverified after resource-guard interruptions, explicitly accepted for this commit/push with CI follow-up. The approved initial engineering preparation policy is provisioned only in genuine Windows original registration; this is not certified platform performance. Actual stage composition, nonzero backup/secret producers and cleanup, backup-specific recovery and other platform adapters remain incomplete. No durable Known clock or installation permission is claimed. |
| T27 | 55% | In progress | Authenticated public/control verification and fixed POST; reviewed ordinary SQLite acceptance, complete original history and high-water recovery; immutable first-install anchor initialization and live read-only installed-image diagnostics are delivered; complete SourceFacts, protected current acceptance/time, workflow and qualification integration remain pending |
| T28 | 10% | In progress | Windows GUI admission, protected writer journal, native Job custody and restricted keeper recovery foundations are delivered. Read-only journal diagnostics passed local checks. Genuine Session preparation, complete writer inventory including CLI/headless, durable save/drain, machine-wide ownership handoff/recovery, multi-user freezing and other platforms remain incomplete. |
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
