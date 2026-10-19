# Upgrade implementation progress

Task source: [V1.0 development tasks](pinvou-upgrade-platform-development-tasks.zh-CN.md).
Tasks are completed individually only after in-scope independent code review,
repairs, applicable tests and commit/push. Contract completion is not production
client, service, physical platform certification or rollout readiness.

Current task totals: 47 tasks; five implemented, fourteen in progress and
twenty-eight not completed or deferred. Detailed chronological evidence follows
the current overview; a completed bounded batch does not complete its whole task.

| Task | Status | Delivery / outstanding dependency |
|---|---|---|
| T01 | Implemented; independently reviewed and verified | Executable metadata/credential Schemas, byte/trust profile, reference verifier, independently verified fixed vectors and local CLI E2E |
| T02 | Implemented; independently reviewed and verified | Closed control APIs, publication/lifecycle/selection/quality reference models and regression tests; no production service implementation |
| T03 | Not completed | Platform execution/protected-storage/backup contracts; platform/data scope and measured qualification remain pending. |
| T04 | Not completed | Independent composite packaging and verification toolchain; depends on T03 platform contracts. |
| T05 | Implemented; independently reviewed and verified | Separate service management identity, MFA, scoped permissions, dual approval, protected audit and three-language admin; isolated actual Keycloak password/OTP/provider flows verified; deployment realm acceptance remains pending |
| T06 | Implemented; independently reviewed and verified | Durable atomic executor, encrypted recovery projections, SQL lease fencing and background recovery; later business commands and production HA acceptance remain pending |
| T07 | In progress | Reviewed trust initialization, key material, scoped issuer enable/stop and actual Root retention; complete issuance and emergency deny composition remain pending |
| T08 | In progress | Reviewed genesis catalog/component management and protected atomic publication primitives; complete business publication workflows remain pending |
| T09 | In progress | Bounded package/native-byte inspection and immutable local storage; complete upload, quarantine and platform qualification remain pending |
| T10 | Not completed | Approved supply-chain policies and bounded security exceptions; complete T08/T09 composition remains pending. |
| T11 | In progress | Protected multi-platform drafts and reviewed content freezing with actual UI/API/history/recovery; package qualification and Release/Target lifecycle approval remain pending. |
| T12 | In progress | Original stable-facts identity, exact profile matching and immutable forward-policy intersection; protected Registry/version/approval and production qualification remain pending |
| T13 | Not completed | Supported floor, prerequisite paths and historical bridge approvals. |
| T14 | In progress | Protected configuration drafts with actual UI/API/recovery; qualified parents, publication, scheduling and first-baseline activation remain pending. |
| T15 | Not completed | Approved automatic rollout scheduling, quality guards and controlled completion. |
| T16 | Not completed | Explicit Stable-to-Beta/Internal atomic draft synchronization. |
| T17 | In progress | Bounded original initial selection, protected logical workflow/initial installation, atomic handoff/live resume and private activation Session; qualified protected check/refresh composition remains pending |
| T18 | Not completed | Approved HTTPS file-source management and current download-info issuance. |
| T19 | Not completed | Current qualification and three-purpose authorize/consume/cancel/status API composition over T44. |
| T20 | In progress | Actual no-update/transaction and bounded Session/download/resume/activation acceptance, four HTTP batch routes, client request/ACK correlation and restricted task observation calculation; actual task acceptance, durable client queue and quality contribution remain pending |
| T21 | In progress | Private sample/receipt persistence authenticates actual frozen SN sources and preserves original-stage history; contribution, aggregate evaluation and business composition remain pending |
| T22 | Not completed | Independent restricted unknown-consume reconciliation and protected original-request disposition. |
| T23 | Not completed | Complete grouped quality review and controlled unfreeze. |
| T24 | Not completed | Fixed weekly release-quality snapshots and restricted export. |
| T25 | In progress | Privacy/deduplication primitives; weekly provider configuration and complete business composition remain pending |
| T26 | In progress | Trusted-time/budget primitives; measured platform evidence and complete host composition remain pending |
| T27 | In progress | Authenticated Root/public/candidate/Package/Check/Refresh/DownloadInfo/Release-renewal verification and fixed control POST; protected workflow and high-water/cache integration remain pending |
| T28 | Not completed | Machine-wide preparation ownership and multi-user writer freezing. |
| T29 | In progress | Download/cache/byte-verification primitives; complete host download/continuation workflow remains pending |
| T30 | Not completed | Complete encrypted backup, manual export and original-deadline local cleanup. |
| T31 | Not completed | Trusted helper/launcher build, installation and host protection. |
| T32 | Not completed | Client consume, original-result recovery and permanent old-execution fencing. |
| T33 | Not completed | Actual Windows upgrade execution and qualification. |
| T34 | Not completed | Actual Linux x64/arm64 upgrade execution and qualification. |
| T35 | Not completed | Actual macOS Intel/Apple Silicon execution and qualification. |
| T36 | Not completed | Protected single-hop execution, active-identity verification and local health. |
| T37 | Not completed | Complete normal upgrade interaction across hops. |
| T38 | Not completed | Complete silent preinstallation, restart activation and slot management. |
| T39 | Not completed | Forced gate and related higher-version forward repair. |
| T40 | Not completed | Machine-wide channel settings and three-language privacy/consent workflow. |
| T41 | Not completed | Atomic legacy bootstrap migration to the new protocol. |
| T42 | Explicitly deferred | Production deployment, operations, alerts and disaster-recovery acceptance are deferred at the owner's request |
| T43 | Not completed | All-target real-system combined qualification and release gates. |
| T44 | Implemented; independently reviewed and verified | Actual protected lifecycle kernel, complete atomic commands, original retention/backup deadlines and worker/access-driven timeout convergence; T17/T19/T20 composition and deployment remain separate |
| T45 | Not completed | Approved helper/launcher evolution and restricted cleanup execution. |
| T46 | Not completed | Actual local-mode prerequisite certification and exact evidence integration. |
| T47 | In progress | Protected frozen SN lifecycle persistence, operation-bound child write sets and original outer T06 recovery; fixed production parent/admin/quality/publication composition remains pending |

Confirmed implementation decisions (2026-10-09):

- Ed25519; initial Root 2-of-3, other roles initially one valid signature; scoped configurable roles and separate high-risk dual approval.
- Metadata 1 MiB, credentials 64 KiB, depth 32, decoded string 64 KiB, per-container 4,096-member bounds.
- Service/admin engineering is in the separately selected **UpdateServer repository**. Go + Gin + pgx + PostgreSQL 17 and React + TypeScript + Vite are approved; PostgreSQL durable jobs/transactional outbox, OIDC with community self-hosted Keycloak, and separate signing/file-storage ports follow that choice. Its private address stays in local Git configuration. Production provider/deployment details remain pending. No server project is created here.

T01 adds Ajv 8.20.0 only to the isolated contract package; dependency audit reported
no vulnerabilities at installation. Desktop behavior and runtime dependencies are
unchanged. Future service tasks cannot be marked complete through client fixtures.

The first implementation batch safely merged the then-current origin/main and
aligned CodeWhale with the parent gitlink. Continue task/review work on that base
without repeatedly syncing merely because main advances.

T01 validation: 41 tests passed on Node 22 and the current local Node release;
the architecture guard passed. Both independent reviewers rejected the first
implementation, the primary role repaired every finding, and both passed the
second review. Python independently verified all 20 fixed envelopes, signing
inputs, hashes and Ed25519 signatures. No production service, desktop update
workflow, installer or physical platform certification is claimed.

A parallel review run exposed a 15-second CLI subprocess test timeout; isolated
and serial reruns passed. The test-only subprocess allowance was raised to 60
seconds and rechecked. It does not set a network or download time limit. Remaining
tasks retain their dependencies, including production infrastructure decisions
that will be supplied later. The separate backend repository is now selected.

T02 validation: 167 tests passed on Node 22 and Node 26, including local CLI E2E,
legal transitions, binding failures, expiry equality and atomic race regressions.
Both independent reviewers passed the fifth frozen code review after the primary
role repaired all findings. Each independently ran the Node 26 suite and the
architecture guard. Generated OpenAPI 3.1 and closed Schemas match their sources;
an independent YAML parser resolved all 1,425 local references across 37 paths
and 146 bundled Schemas. Diff whitespace checks passed.

T02 freezes API contracts, owner projections, final guards and atomic reference
models. It does not implement a database, production signing or authentication,
real helper/launcher, desktop upgrade workflow or physical OS certification.
Download handoff and credible progress/waiting remain independent of session
expiry; there is no total or cumulative download deadline.

T05 is committed and pushed in the separate UpdateServer repository as
`bf5f89c6b6a55c0c71cf2e17c0d385eff7d708c3` on
`feat/t05-management-security`. Explicit owner provisioning initializes an
immutable OIDC identity in a protected server ACL; request/JWT roles never grant
authority. Dedicated signed password-plus-OTP ACR evidence is fresh for five
minutes. Current scope, MFA, independent reviewers and exact command/revision
bindings are rechecked at the owner boundary. The admin implements real draft,
review, audit and response-loss recovery workflows in Chinese, English and Japanese.

Both independent reviewers passed the second frozen T05 review after the primary
role repaired all findings. Each independently ran the complete Go suite with
real PostgreSQL 17.11 and Chrome E2E, without skipping those integration cases.
Go vet, module verification, server/admin builds, TypeScript checks, pinned
contract checks and diff checks passed. Client architecture checks passed.
Regressions cover current-actor binding, OIDC authorized party, per-attempt
request IDs, terminal revision refusals, preserved unknown requests, strict
24-hour complete response replay and protected minimal committed-object recovery.

The signed test IdP does not certify an actual Keycloak password/OTP realm. Run
the deployment acceptance checks in the service's `docs/management.md` before
relying on that realm. T05 does not implement T06's general executor, business
publication/consumption, artifact downloads, production HA/RPO or signing custody.
No total or cumulative download deadline has been introduced.

T06 is committed and pushed in the separate UpdateServer repository as
`0e0c85458013e5f0307b9871ebf438b71bc3a5b2` on
`feat/t06-atomic-executor` (2026-10-10). The executor atomically reserves original
business identities, persists encrypted minimal recovery projections and commits
domain/result/audit/outbox write sets through a trusted owner interface. Current
authority and complete read sets are checked again before final writes. SQL time,
owner epochs, 30-second leases, ten-second renewals and the original two-minute
owner deadline fence late work. Background recovery does not require client retry.
Standard 24-hour response retention does not truncate owner-managed long-lived
event, consume or synchronization ledgers. The production registry awaits those
later owners; no sample command or generic HTTP dispatcher is exposed.

Both independent reviewers passed the third frozen T06 review after the primary
role repaired every finding. Initial Read/checkpoint and final transactions no
longer contend with their own heartbeat; a legal renewal after a slow initial read
preserves the original owner deadline. Standard replay checks its strict window
again after current-authority and replay work. Unknown commit acknowledgements
return no speculative result and recover through the original persisted identity.

Independent real PostgreSQL 17.11 tests and Chrome management regressions passed.
Evidence includes two real 22-second transaction regressions, a real ten-second
heartbeat, natural lease expiry and a 30-second periodic recovery scan, an abrupt
subprocess exit after checkpoint, all executor commit-ACK boundaries, partial-write
rollback, concurrency, current denial, read-set changes, ciphertext tampering and
restricted runtime privileges. The parent subprocess helper is intentionally
skipped; the actual crash E2E runs it explicitly. Final Go vet/builds, module
verification, pinned contract checks, formatting and diff checks passed.

Serial independent reruns resolved a browser test-budget failure observed under
concurrent local test load; these allowances do not limit product downloads.
T06 does not certify later business workflows, an actual Keycloak OTP realm,
production signing custody, PostgreSQL HA or acknowledged-state RPO=0. No race
detector execution is claimed. Downloads retain no total or cumulative deadline.

T07 preflight (2026-10-10): the separate service's
`docs/signing-implementation-plan.md` at
`a181b2eb352b064be3ad45fae1ee457498a4562c` on `feat/t07-signing-root` defines
the implementation and real-provider acceptance boundaries. Both independent
reviewers passed this plan; they did not certify T07 implementation. The product
owner approved OpenBao Transit evaluation and implementation on 2026-10-10.
Independent Root custody, provider lifecycle recovery, concrete-client trust,
complete historical Release/Package archives and T08's same-transaction deny
composition remain required. Protected append-only PostgreSQL archives are a
proposal; production protection and restoration evidence remain outstanding.
T07 is not completed and no production key or anchor has been provisioned.

T07's protected OpenBao Transit adapter is committed and pushed as
`e0dc83cb9be4fc6ee3614dc00ebec6f26ebc3e54` on `feat/t07-signing-root`.
Both independent reviewers passed this adapter batch after the primary role
repaired the real-provider test issues. Each ran OpenBao 2.7.1 against isolated
synthetic keys and restricted identities. Evidence covers nonexportability,
forbidden management operations, pinned versions, signing revocation, unsafe
existing keys and a lost successful create ACK reconciled to the original key.
The official release asset SHA-256 matched; release-signature certification and
production seal/custody/backup acceptance are not claimed.

The separate Root byte/trust batch is committed and pushed as
`aa3cf27bb86793e50f378a48ebaa95652c1546dd` on `feat/t07-signing-root` and has
passed both independent deep reviews.
It preserves all 20 frozen signing vectors, closed Schemas, canonical bytes,
exact context/scope, unique signers and old/new Root thresholds. All frozen
regular expressions were compared with actual Node ECMAScript, including path
traversal and terminal line separators. The existing transitive `regexp2`
dependency is now directly required for negative lookahead; matching is bounded
and fails closed without changing the protocol Schema. This budget applies to
control validation, never downloads.

The service regression run exercised real PostgreSQL, Chrome and OpenBao.
All Go packages passed except a first-run management browser interception timing
failure (`Route is already handled`); that E2E passed on an isolated serial rerun
without a code change. Build, full Go vet, module verification, pinned contract
checks and diff checks passed. The client architecture guard passed. No race
detector or production custody acceptance is claimed.

The protected storage batch is committed as
`92bafad645056b805df775222fdc88a059a26544` on `feat/t07-signing-root`.
It implements independently provisioned anchors, authenticated consecutive Root
history, monotonic online key-use obligations and exact Release/Package archives.
Historical verification survives expiry and normal key retirement without
creating current qualification. Failed archive writes roll back together.
Both independent reviewers passed after the primary role repaired column-level
writes, reachable SET-role permissions, installed triggers after grant revocation
and masked login identities. Actual PostgreSQL regressions cover these paths.

The full Go regression passed with real PostgreSQL, Chrome E2E and OpenBao.
The final storage package was rerun after the permission repairs and passed;
module verification, full vet, build, pinned artifact and diff checks passed.
These checks do not certify production custody or acknowledged-state RPO=0.

These batches do not complete T07. The Root publication owner composes T05
current MFA and exact dual approvals with T06 durable recovery and requires a
registered authoritative T08 source for complete public/private metadata and
current Root-role deny. Purpose-scoped issuers, provider lifecycle recovery and
actual T08 composition remain. Storage functions are not application publication
endpoints; no missing-owner fallback or production signing is wired.
On 2026-10-10 the product owner deferred deployment work. Production hosting,
seal, independent custodians and protected backup/restore acceptance remain
deferred without blocking code development. Local provider experiments do not
establish three independent production custodians.

T03 still requires approved OS certification coverage, all-user/service data
scope and finite preparation budgets. The product owner approved local health
criteria, strict trustworthy time option A and the Go backend stack on 2026-10-09,
and requested the platform/data inventory and measured-budget test proposal in
[implementation decisions](upgrade-implementation-decisions.md).
That document distinguishes approved policy from proposals and physical evidence.
Normal offline retention across shutdown also needs a proven trustworthy elapsed-time mechanism under
the current strict requirement. Ordinary OS clocks or powered-on TPM clocks
alone do not demonstrate elapsed shutdown duration. OS/data scope and concrete
budget approvals, and time-implementation evidence, remain pending. No support
restriction or retention-policy change is assumed approved.


T07 authorized Root publication owner (2026-10-10): implementation and both
independent deep reviews passed. It is committed and pushed as
`46cf03a814226c28686bd792a72544559f33c28a` on `feat/t07-signing-root`.
The owner binds exact actor, current head, initial anchor and public successor
bytes to protected T05 dual approvals, compares the complete current component
inventory and issuer obligations, and commits original Root/result/audit/outbox
through T06. Owner replay preserves the original result beyond the standard
24-hour response window while rechecking current authority and Root-role deny.

The primary role repaired the missing Package role, fresh-MFA evidence loss after
re-entry and crash, and SQL NULL authorization-window bypass. Separate protected
latest authentication facts retain the original verified actor and times; current
T05 permissions, exact approvals, session expiry and five-minute MFA are checked
on every recovery. They cannot rewrite the immutable business receipt or prolong
MFA. Eight actual PostgreSQL owner regressions passed (195.032s), including
natural original-MFA expiry, fresh re-entry, a second cancellation, natural lease
expiry and recovery through a new command/engine without caller context. The
SQL NULL cases passed in the final complete signing PostgreSQL rerun (197.360s).

All Go packages passed across the full real PostgreSQL/Chrome/OpenBao regression
and the affected-package rerun. The first full run found an existing column-grant
test's synthetic migration-version collision with real migration two; the primary
role replaced its fixed synthetic version with the next unused version and both
independent reviewers confirmed the original permission test remained intact.
The final database package and real PostgreSQL authority/readonly health probe
passed. Full vet, module verification, build, pinned contract checks and diff
checks passed. The client architecture guard passed. No race detector, actual
Keycloak OTP realm or production deployment/custody acceptance is claimed.

The production T08 source is required and absent until its own implementation;
integration fixtures do not replace it. This batch adds no generic transport
signing/SQL dispatcher and does not complete T07. Deployment work remains deferred
and downloads have no total or cumulative time limit.

T07 intrinsic signed-claim validation (2026-10-10) is committed and pushed as
`6f2e4ae25cf07c319aa45f133b1c690eb746a035` on `feat/t07-signing-root`.
The Go validator implements the frozen client reference semantics for all six
metadata and fourteen credential roles, including current windows, precise
reference scope, update dual chains, authorization preparation/backup/staged
bindings and original event/task anchors. It grants no signing, qualification,
client trust or execution rights; historical expiry remains a separate path.

Both independent reviewers passed the original batch and the expanded coverage.
All twenty frozen vectors, no-update, all four baseline/candidate by
ordinary/bridge combinations, required irreversible backup, Windows/macOS native
identities and original event-anchor boundary mutations agree with the actual
Node reference module. The final expanded scoped run passed (2.326s).
The complete serial Go regression passed with real PostgreSQL, Chrome E2E,
OpenBao and the Node comparison; signing PostgreSQL passed (149.051s), operations
PostgreSQL (163.202s), management browser/database (26.765s) and OpenBao (5.442s).
Full vet, module verification, build, pinned artifact and diff checks passed.
Production code was unchanged by the later test-only coverage additions.
Purpose-scoped issuance, provider lifecycle and actual T08/T17 composition remain
required; this batch does not complete T07. Deployment acceptance remains deferred.

T07 purpose-bound pre-signing (2026-10-10) is committed and pushed as
`b9e99eaffc16d9ea4d5ca26ba052d377b43b0966` on `feat/t07-signing-root`.
The component fixes complete role/product/component/scope and protected provider
mapping, refuses online Root signing, checks intrinsic claims and exact current
Root role policy/deny/threshold/designated key before provider calls, and verifies
every provider response locally. Immutable candidates retain their original bytes;
final revalidation refuses changed Root, deny, expiry and another signer instance.

Both independent line-by-line reviewers passed this batch. Pure tests cover all
nineteen online roles, two-signature thresholds, wrong purpose/context, current
fences, provider substitution/mutation, cancellation and copy isolation. Real
OpenBao 2.7.1 exercised all nineteen roles with an isolated nonexportable key and
restricted identity. The complete serial regression passed: management
browser/database (21.452s), operations PostgreSQL (213.994s), signing PostgreSQL
(139.718s), OpenBao (6.331s), pre-signing (5.693s) and Node semantics comparison.
Full vet, module verification, build, pinned artifact and diff checks passed.

This batch does not issue credentials, add a transport endpoint or implement the
future owners' business/client trust guards. Atomic issuance facts/last-use and
provider lifecycle remain required. Two independent contract checks confirmed
that frozen V1 does not yet define a trustworthy confirmation of a specific
client's accepted new Root; this protocol-extension decision was returned to the
product owner. No caller-declared trust, device authentication or hardware proof
is substituted. T07 remains incomplete; independent code development continues.

T07 protected Transit retirement (2026-10-10) is committed and pushed as
`ff74abecf0e6094a08c8f9d2781caa36b5948296` on `feat/t07-signing-root`.
The protected tooling adapter pins the original name/version-one/public identity,
reserves an unused provider successor when required by actual Transit behavior,
raises the signing floor and inspects the final retained material. Unknown ACKs
remain unknown until a new instance reconciles the same original identity.
The runtime provider has no lifecycle method; historical public verification
material remains available. Every physical name permanently belongs to one
protocol key; reserved successor versions never become registered issuers.

Both independent line-by-line reviewers passed the five-file adapter batch.
Real isolated OpenBao tests covered the version prerequisite, forbidden runtime
mutation, both lost-ACK boundaries, recovery, idempotence, pinned signing refusal
and historical verification (12.641s affected run; 13.630s final full-suite package).
The complete Go suite, full vet, module verification, build, pinned artifacts and
diff checks passed. PostgreSQL and Chrome opt-ins were skipped in this adapter-only
full run: database/browser code was unchanged and the preceding pre-signing batch
had passed their actual physical regressions. No race or production custody
acceptance is claimed.

This low-level adapter requires a previously committed approved durable intent,
domain issuance fence and serialized controller. It introduces no durable jobs,
registry, authoritative deny, HTTP dispatch or completed T07 acceptance. The main
role continues those code paths; production deployment remains deferred.

T07 permanent provisioning ledger (2026-10-10) is committed and pushed as
`9a4f654c6893bad582aca38d2de0b1f00cfa2c54` on `feat/t07-signing-root`.
The protected internal intent binds the exact actor, full non-Root role/scope,
permanent provider reference, complete approval reference and command hash. One
T06 result permanently owns one intent and version-one provider name. SQL-clock
claims use monotonic epochs and an original two-minute window; exact canonical
public material and completion time become immutable ready records. This storage
port grants no execution, publication, enablement, client trust or issuance rights.

Both independent line-by-line reviews passed after the main role repaired test
map contamination. Each closed-shape rejection now starts from independent valid
input. The main role also added raw SQL descriptor counterexamples in a running
claim, V2-to-V3 upgrade preserving an actual signed Root, column-write and retained
trigger startup refusal, and corrected the old V1 upgrade fixture to remove newer
migration objects first. Earlier SQL migration bytes/digests remain unchanged.

All Go packages passed across the full serial regression and affected reruns.
Actual signing PostgreSQL passed (276.494s), including natural job-lease expiry,
takeover, old-owner refusal, concurrency and rollback; operations PostgreSQL
passed (181.046s) and real OpenBao (8.217s). The initial untouched management
browser test failed with a route.fetch/route.abort already-handled timing error;
its original unchanged real browser rerun passed (12.090s). This is recorded as a
test timing issue, not a claimed repair or proof of test stability. Final pure
contracts/lifecycle runs passed (2.130s/1.708s), and the three final affected actual
PostgreSQL tests passed (14.207s). Full vet, module verification, build, pinned
artifact and diff checks passed. No production logic changed in the test-only
repair; no skipped database/browser opt-in is claimed as physical evidence.

Actual T05/T06 command composition, authenticated recoverable provider controller,
configuration ownership, OpenBao invocation/reconciliation, enabled key registry,
atomic issuance/key-use and T08 stop/deny integration remain required. Permanent
ledger fixtures do not complete those owners or T07. Main development continues;
production deployment remains deferred and downloads retain no cumulative limit.

T07 approved role-key provisioning (2026-10-10) is committed and pushed as
`ea9fd560a52403309a16471b950a6e5d1e3ed50c` on `feat/t07-signing-root`.
The actual internal T05/T06 owner generates the permanent identity before exact
actor/role/scope/provider approval, requires current permissions, two distinct
reviewers and fresh MFA, and atomically commits the durable intent, immutable
queued result/outbox, authorization audit and T06 state. Compute has no external
provider effect. The authenticated controller commits its independent SQL claim
before RPC and checks fresh current T05 authority and SQL time at claim, just
before RPC and at completion; no SQL transaction spans the external call.

Latest verified same-actor authentication remains separate from the immutable
receipt and can be renewed after queue commitment without moving backwards.
Unknown create ACKs retain the same physical name/version and original job fence;
fresh instances recover the actual existing key. Provider references bind trusted
connection configuration and exclude renewable tokens; this is association, not
remote namespace attestation. Ready replay performs no RPC and preserves public
material/time. The permanent T06 result stays queued after readiness and without
response retention. None of this publishes Root, enables a key or issues a token.

Both independent line-by-line reviewers passed the complete 25-file batch after
the main role repaired one ineffective completion-window test. Its final negative
cases use a valid running claim, epoch and descriptor, with successful rollback
controls, so only the invalid authorization window causes refusal. Actual
PostgreSQL also covers final rollback, unaccepted storage intents, actor/body/scope
substitution, permission revocation before/during RPC, natural MFA expiry,
monotonic fresh authentication, immutable replay and additive migration/ACL checks.
Migration four leaves the first three SQL migrations and frozen contracts intact.

The full serial Go suite passed with all configured PostgreSQL, Chrome, OpenBao
and reference-semantics opt-ins. Actual management/browser passed (18.853s),
operations PostgreSQL (154.516s), signing PostgreSQL (576.442s), OpenBao (6.846s),
claims/reference semantics (2.052s), and purpose-bound pre-signing (10.229s).
The integrated PostgreSQL/OpenBao test deliberately loses a successful create ACK,
waits for the original two-minute SQL fence, and recovers the same public key with
fresh repository, command, executor and provider instances. Earlier affected
integration passed (136.853s); the final repaired PostgreSQL guard tests passed
(26.392s). Full vet, module verification, build, pinned-artifact and diff checks
passed. No production Keycloak OTP, custody, deployment or HA acceptance is claimed.

T07 remains in progress. Protected key registration/enablement, actual issuer
final transactions/key-use writes and immediate-deny/T08 integration remain.
Specific-client trust confirmation still awaits the pending protocol decision;
dependent issuance stays closed. Deployment remains deferred by the owner's
instruction and downloads have no hard total/cumulative elapsed limit.

T07 inactive role-key material registration (2026-10-10) is committed and pushed
as `d103754972aa9e497875ad8391627cbb81130229` on `feat/t07-signing-root`.
The actual internal owner requires a new exact actor/source/scope-bound T05
approval, two distinct reviewers, fresh MFA and current permissions. Its source
comes from the accepted original provisioning receipt/outbox and immutable ready
record, including canonical descriptor, permanent provider/name/version, epoch
and original completion time. The T06 final transaction rechecks the source and
complete authorization read set, then atomically stores the registered material,
immutable result/outbox, audit and operation completion. No provider RPC occurs.

Unique intent, key identity and physical provider tuple enforce one permanent
binding. Failed pending reservations do not grant perpetual ownership: another
authorized actor can obtain a new exact approval if no binding committed.
Fresh composition replays the original result independently of T06 retention;
protected same-actor authentication also enables durable worker recovery.
Registration publishes no Root, enables no signer and creates no issuance/key-use.
Migration five preserves the bytes and digests of migrations one through four.

Both independent line-by-line reviewers passed production code and documentation
after the main role repaired canonical source/result/receipt byte checks, strict
integer/string and safe revision checks, the actual approval `changeId` field,
and exact original intent-to-scope association. Restricted-runtime SQL rejection
tests include successful canonical writes followed by rollback, so malformed
bytes/types or scope cannot be masked by an unrelated failed prerequisite.

The final affected real PostgreSQL/OpenBao suite passed (66.981s), including fresh
worker recovery after the natural lease (33.85s) and actual Transit generation to
registration (5.15s). The frozen serial integration run passed signing PostgreSQL
(531.506s), operations PostgreSQL (152.494s), OpenBao (7.473s), reference semantics
(3.622s) and all other packages except the unchanged management browser timing
test. That run remains recorded as failed: `route.fetch/route.abort` reported an
already-handled route. Its unchanged isolated real browser rerun passed (13.594s);
this is neither a repair nor evidence of test stability. The subsequent default
`go test ./...`, full vet, module verification, build, pinned-contract and diff
checks passed; skipped default opt-ins are not claimed as physical evidence.

T07 remains in progress. Actual published Root membership and immediate deny must
precede controlled enablement; final issuance/key-use composition and the pending
specific-client trust-confirmation decision remain required. Next development
addresses the real T08 protected inventory/genesis/deny source needed by Root
publication, without treating empty fixtures as owner state or marking T08
complete. Deployment remains deferred and downloads retain no hard elapsed limit.

T08 protected metadata genesis (2026-10-10) is committed and pushed as
`f1992b07eb2c088be24f1cd422afa4cea3db82fa` on `feat/t08-metadata-genesis`.
The actual internal owner initializes a protected empty product catalog and typed
deny, then registers unpublished components after actual approved Root publication.
Separate exact actor/product/component/base-bound policies require current T05
authority, two distinct reviewers and fresh MFA. Final SQL rechecks canonical
receipts, current Root/catalog, complete independent inventory and authorization
time before atomic head/catalog, original result/outbox, audit and T06 completion.
The original ACK survives later mutable state changes without granting new rights.

The actual Root source enumerates protected heads independently, rejects missing
catalog/deny or published heads, and resolves each retention reference only through
its actual registered owner in the same transaction. There is no empty-source or
requested-hash fallback. This independent additive metadata schema leaves T07 SQL
migrations one through five and frozen V1 contract bytes intact. Runtime receives
only SELECT, three typed mutations and a read-only resource-budget helper.

Both independent line-by-line reviewers passed the frozen code and documentation.
The main role repaired two prospective Root-maintenance capacity findings: all
actual key-use and distinct retention references join the full count budget and
protected key-use records/index join the registration's final read-set CAS. Their
actual protected encoded sizes also join the conservative 1MiB projection budget.
Root publication separately verifies and captures actual retention records through
their registered owners. Count and byte limits are repeated by final SQL. Tests
verify actual Root rotation at 4096 reads and reject the next registration with no
business writes; long retention references can exhaust bytes before the count.
Bulk capacity fixtures are not claimed as thousands of actual approvals/issuances.

All eleven actual isolated metadata PostgreSQL tests passed (92.908s), including
natural lease-expiry worker recovery without the caller (33.89s), concurrent
registration, stale Root capture, immutable replay, current/final authority refusal,
independent new approval after failure, SQL positive/negative canonical controls,
rollback, runtime ACLs and migration digest/missing-schema checks. The affected
actual T05 PostgreSQL regression also passed (4.054s). Default full Go tests, full
vet, module verification, build, pinned-contract and diff checks passed. External
opt-ins in the default suite were skipped; no new browser, physical provider,
production Keycloak OTP or custody acceptance is claimed for this metadata batch.

T08 remains in progress: full signed selection/refresh publication, public/private
current metadata, first activation, actual immediate deny plus pending/job generation
and durable reconciliation are still required. T07 controlled enablement/issuance
and the pending specific-client trust decision remain separate. Development
continues without deployment work; downloads have no hard total/cumulative limit.

T07 scoped online issuer epochs (2026-10-10) is committed and pushed as
`7c65b30676cc9ccead3b304bf0c21ccc3517d543` on `feat/t07-online-issuer-epochs`.
This internal batch completes non-Root scoped signer-set enablement and ordinary
domain stop, with actual current-registry and immutable-retention read ports.
Enable approval binds the full role/product/component/channel/target scope,
original registered material, threshold, Root/deny/control revisions and actual
actor. Stop approval binds the actual actor, complete original group scope and
threshold, current control, immutable epoch and active KeyUse; its Root/deny
references are absent and its material list is empty.
Both commands require current scoped authority, two distinct non-author reviews
and fresh MFA. No public V1 endpoint or frozen contract is changed.

Enable uses actual published unexpired Root, typed role deny and the actual T08
genesis inventory. Each material undergoes an outside-SQL private-capability
probe with a separate internal domain, original fixed provider name/version,
exact key/capture/result binding and fresh server nonce. Real Transit GET alone
does not qualify. Probe signatures are discarded; no credential or fabricated
last-issued/object/upload/recovery fact is stored. Final T06/SQL rechecks complete
current dependencies and time before atomic epoch/control/active-use/index,
original result/outbox, audit and executor completion.

Ordinary stop works without provider configuration, current Root validity or
current key eligibility. It fences only the exact current epoch and preserves
all existing threshold, keys, immutable references and retention deadlines.
A stopped epoch never resumes; a new independent approval allocates a new epoch.
Actual same-transaction retention verifies the original protected epoch/receipt
and every original material source. Current registry reads require actual current
Root/deny/control/use/materials; a historical epoch or earlier read grants no
new signing permission. Immutable ACK recovery rechecks original actor/approval
and current T05 authority/MFA without probes or mutable qualification.

Both independent line-by-line reviewers passed the frozen code, SQL, tests and
documentation. The main role repaired empty-provider Stop construction, complete
command capacity prechecks, final SQL Root-expiry checks and nondeterministic
dependency ordering. Canonical byte/type and actual SQL material-association
errors found by initial positive controls were fixed before acceptance. Both
prospective Root and complete command count/byte budgets are checked before
approval and repeated in SQL; duplicate read identities fail closed. Existing
signing migrations one through five and V1 artifacts retain their original bytes.

All 22 actual isolated PostgreSQL/OpenBao top-level tests passed (308.044s): eleven
new issuer tests plus all eleven existing metadata regressions. Cases include
legal 4096-reference enable and 4097-reference approval/SQL refusal, retained Root
rotation, concurrent enable, final Root/permission/deny changes, rollback,
canonical SQL, runtime ACL/schema/digest refusal, four successive epochs with
all historical uses retained, provider-free stop/replay and fresh-worker natural
lease recovery (37.50s). The real Transit pipeline passed (9.86s), including
GET-only failure, actual fixed-version Sign success, forbidden runtime rotation
and current-registry refusal after domain stop. Nonzero retention and deny test
fixtures do not claim implemented issuance or emergency-deny reconciliation.

The strengthened final-SQL expiry counterexample separately passed (25.480s
package, 18.77s case). Its nontransactional sequence proves execution reached the
owner's isolated delay trigger inside final SQL; only Root expiry changes and
all epoch/control/outbox/use writes roll back. The affected actual T05 PostgreSQL
regression passed (4.087s). Default full Go tests, full vet, module verification,
three command builds, pinned-contract and diff checks passed. Independent pure
tests and scoped vet also passed. Default external opt-ins were skipped; no new
browser, live Keycloak OTP, production custody, HA or physical-client acceptance
is inferred. The additive issuer schema uses owner initialization and restricted
runtime SELECT/typed mutations only.

T07 remains in progress. Final credential/metadata issuing owners and actual
atomic last-use updates, the pending specific-client trust-confirmation decision,
durable physical stop and complete immediate deny remain required. T08 remains
in progress: the actual adapter supports genesis only and rejects published heads
until complete publication/current-private/activation/pending-job reconciliation
owners exist. Development continues with deployment deferred; downloads still
have no hard total or cumulative elapsed limit.

T07 scoped durable ordinary material retirement (2026-10-10) is committed and
pushed as `36e7f223df7593b114dd8ec328f325b40b3e7e55` on
`feat/t07-material-retirement`. This internal batch completes permanent non-Root
physical retirement under an actor-bound `StopRoleKeyMaterial` approval, with
current scoped authority, two distinct non-author reviews and fresh MFA.
The approval binds original accepted provisioning/registration, fixed provider
name/version one, full material identity, expected permanent-fence absence and
complete actual global Root/anchor and affected-group inventories.

Final T06/SQL acceptance atomically fences the material permanently, stops each
affected current whole group/control/use and stores the original result, task,
audit and outbox. All keys, thresholds, historical epochs and original object/
upload/recovery windows remain intact. The accepted result is explicitly
`fenced-and-queued`, not physical completion. Missing provider configuration or
failed RPC leaves the durable task and permanent fence. Current Enabled reads
and new Enable preparation/final SQL reject retired material; no new approval,
epoch or registration restores it. This ordinary retirement preserves valid old
objects; it does not substitute for emergency key deny and T08 reconciliation.

Root exclusion checks actual public identity and Root-role membership in every
product, including a different product's anchor/current Root. Every legal Root
publication, independent anchor ingress and retirement acceptance shares one
protected MVCC gate after its own product/anchor lock, without locking other
products afterward. Real row writes reject stale SERIALIZABLE snapshots even
after multiple token flips. Root ingresses reject permanently fenced material,
closing promotion races throughout the outside-SQL provider call.

Durable provider work commits its independent two-minute SQL lease and owner
epoch before fixed-version RPC. Current original authority/approval/MFA is checked
at claim, immediately before RPC and completion. Actual SQL time fences completion
again after mutation. Unknown RPC/completion acknowledgements retain the original
identity; natural lease expiry permits a fresh worker's higher epoch while old
owners cannot complete. Verified replay can refresh pending authentication
without changing the original result or lease. Fair rotating durable scans avoid
starvation by an expired first task. Actual Transit retirement requires retained
version-one public identity and an observed minimum signing floor above one;
network errors, failed Sign and ACK alone are not completion evidence. No public
verification trim/delete or raw successor protocol registration is introduced.

Both independent roles passed line-by-line review of frozen code, SQL, eight new
test files, related fixture and three server documents. The primary role repaired
global Root membership races, final lease expiry, byte-budget consistency,
composed schema/ACL readiness and worker fairness before final PASS. Additive
signing guard six and issuer guard two retain original signing migrations one
through five, issuer/metadata schema one and all frozen V1 bytes.

The complete actual serial metadata PostgreSQL/OpenBao package passed 39 top-level
cases in 566.982s: 17 retirement, 11 issuer and 11 metadata regressions. These
include competing cross-product Root/retirement commit orders, stale snapshots,
unchanged two-member thresholds/history, final authority/group changes, natural
lease recovery (126.12s), lost completion ACK, fair worker/authentication refresh
(44.18s), runtime inherited/SET-role refusal, additive upgrades and actual Transit
fixed-version Sign denial with historical local signature verification. The common
capacity reserve permits 1740 valid owner fixtures (3487 reads, 1,048,148 bytes)
and rejects 1741 before approval and in final SQL; bulk fixtures certify bounds,
not thousands of deployed catalog approvals.

The preceding full signing PostgreSQL invocation remains recorded as FAIL
(589.755s): four old migration fixtures were compiled before their dependency-order
repair; all other cases passed. After removing the additive guard before migration
five, all four affected actual upgrade cases passed separately (15.675s package).
The affected actual T05 approval/audit regression also passed (8.610s package,
1.54s case). Full default Go tests/vet, module verification, three command builds,
pinned-contract and diff checks passed; repaired-fixture default scoped tests and
the client architecture guard passed. Default external opt-ins were skipped and
are distinguished from explicit PostgreSQL/OpenBao runs. No new browser, live
Keycloak OTP, production custody, HA or physical-client certification is claimed.

T07 remains in progress: actual final issuing owners/atomic last-use updates,
specific-client trust confirmation and complete emergency deny are still required.
Public lifecycle transport/management workflows remain separate from these
internal owner components. T08 still requires complete publication/current private
metadata, first activation and pending/job generation/reconciliation owners.
Development continues with deployment deferred. Downloads have no hard total
or cumulative elapsed limit.

T07 scoped finite current-metadata presigning (2026-10-11) is committed as
`5fe6ca5144078081d68366d4caedd732a53a7e76` on
`feat/t07-issuance-write-set` in the separate server repository. The owner approved
a 32MiB (33,554,432-byte) aggregate cap for complete signed metadata envelopes in
one internal atomic issuance transaction. Each envelope remains limited to 1MiB.
This new parameter does not restrict installer size, download volume or duration.

The preparation component checks all exact contexts, intrinsic claims, current
Root/role deny, unique role/full scope and exact signature overhead before any
provider callback. It keeps private claim/Root/deny snapshots, locally verifies
each fixed-context signature and returns no partial batch on failure. Final
revalidation repeats size, Root, role deny and original time fences without a
provider call; returned copies cannot change candidates. The actual publisher
still owns metadata reference graphs and current publication/business eligibility.

Frozen V1 offers at most fourteen unique current slots in one component: twelve
Target scopes, one Snapshot and one Timestamp. Its natural envelope bound is
therefore at most 14MiB, below the approved ceiling. The 32MiB/4096-count boundary
tests are explicitly synthetic arithmetic evidence, not legal publication cases.
Actual schema/semantics-valid synthetic Target claims exercise exact 1MiB signed
envelopes and one-byte signature-overhead refusal with zero provider calls.

Both independent roles passed line-by-line code, test and documentation review.
All six new pure batch cases and existing presign regressions passed (37.046s);
independent package runs also passed (57.282s and 70.350s). The explicit isolated
Transit all-online-role case passed (40.96s; package 49.000s), including the new
three-role batch, duplicate preflight, copy/deny/no-resign guards and revoked
provider failure without partial material. Full default Go tests/vet, module
verification, three command builds, pinned-contract and diff checks passed, as
did the client architecture guard. Default database/browser/external opt-ins are
distinguished from the explicit Transit run. No new actual PostgreSQL issuance,
browser, Keycloak OTP, production custody, HA or physical-client acceptance is
inferred; no database mutation was changed in this preparation batch.

T07/T08 remain in progress. The actual atomic SQL issuance ledger, original
last-use window updates, durable response recovery, publication/authority guards
and issuer/material/read-set final composition remain unfinished. Local
owner/window helpers and the future write-set plan are not committed as completed
functionality. Deployment remains deferred; downloads have no hard total or
cumulative elapsed limit.

T07 internal finite atomic metadata recording (2026-10-11) is committed as
`ba104e4cd294ce4f2d1eac5b92c9d752901f2d59` in the separate server repository on
`feat/t07-issuance-write-set`.
It captures actual T05 enabled epochs, original accepted provider sources and
the T06 owner, prepares signatures outside SQL, then records exact whole-envelope
bytes and monotone original KeyUse obligations in one private SQL transaction.
The new issuance namespace leaves every existing SQL migration and frozen V1
artifact unchanged. Runtime has SELECT and no EXECUTE on its private write port;
a command name or prepared signature does not establish publisher authority.

Before writing, the port rechecks the complete original Root, typed deny,
metadata inventory, epoch/control/use, material/fence and owner read set. After
writing each distinct epoch once, it checks precisely derived final uses/index,
unchanged dependencies, the actual lease and final object/Root expiry. Original
keys, threshold and provenance remain immutable. Retention preserves previous
deadlines, uses the original signed expiry and the current SQL lease rounded up
plus the standard 24-hour reply window, with a two-minute margin. These windows
create no fresh object validity, event-upload right or permanent replay right.
Recovery returns original stored bytes within the actual original T06 window;
it calls no signer and still requires current publisher authorization and T06's
final replay fence. Same-version exact bytes may be reused by a later result;
different bytes at the same full role/scope/version conflict.

The corrected production tree passed all ten new real PostgreSQL/OpenBao cases
and the affected existing issuer Transit regression in one serial run (222.567s).
They cover multi-role/threshold and multi-Target exact-scope issuance, original
replay without signing, business/lease rollback, version/byte/canonical/owner/read
and policy counterexamples, final object/Root expiry, Stop during presigning,
private helper/column ACL and digest refusal, and additive initialization.
The actual complete-source boundary records 4096 references and refuses 4097;
its additional canonical genesis heads are isolated owner fixtures, not thousands
of accepted T05 registrations. The 32MiB aggregate-array tests are synthetic
arithmetic controls; frozen V1 still has at most fourteen legal slots per component.

Actual Transit uses the approved original T05 source, fixed name/version/public
descriptor and local Ed25519 verification. Revoking the isolated signing token
does not prevent original-byte replay; new issuance fails without extra records.
The collation regression checks actual accepted test descriptors whose ICU
numeric and byte ordering differ: explicit `COLLATE "C"` accepts the legal batch,
while the old comparator refuses it with no partial writes. Deterministic public
test labels and full pair selection remove random fixture failures; the corrected
test-only fixture passed separately (16.398s). Final
post-write verification also binds itself back to the original complete candidate.

Both independent roles passed the complete scoped code, SQL, tests and design
record after the primary role repaired their findings. Full default Go tests/vet,
module verification, three command builds and pinned-contract checks passed.
Default external opt-ins remain distinct from the explicit local integration
runs; no browser, live Keycloak OTP, production custody, HA, concurrent-load or
physical-client certification is inferred.

This completes the internal recording port, not all T07/T08 functionality.
The actual source currently accepts complete genesis and rejects published heads.
A production publisher must supply current business/authority guards, metadata
reference graph and all current/generation/pending publication writes in the same
T06 transaction. The isolated test publisher does not provide those owners.
Specific-client trust proof, complete emergency deny/reconciliation and permanent
Manifest archives remain separate work. Deployment remains deferred; downloads
have no hard total or cumulative elapsed limit.


T44 first lifecycle storage batch (2026-10-12) is committed as
`1e9321b4b73559843cf9ed229a9e8abf60feddba` in the separate server repository on
`feat/t44-lifecycle-kernel`. It introduces protected Session/Authorization,
minimal consume reservation, initial Transaction and Outcome storage. Private
SQL composes session fencing, unique active transaction, original cancellation/
expiry outcomes and the actual T06 ledger result in one final owner transaction.
Ordinary mutations require the current original StandardReplay owner/epoch/lease;
OwnerReplay cannot replace the original 24-hour complete-response obligation.
There is no runtime/PUBLIC first-consumption or new-authorization entry point.
Future T17/T19 owners still need their actual eligibility/preparation/trust guards.

Recovery persists only semantic digests and minimal bindings, never the full
request, recovery secret or signed credentials. Internal status verifies all V1
identity fields and the canonical 256-bit secret, returns state/transaction ID,
closes at original authorizationExp+61d and cleans at +62d. Late terminal
observation derives original stored state and cannot reconstruct a cleaned result.
The additive schema preserves every previous migration and pinned V1 artifact.
Explicit owner initialization and a restricted maintenance command scan original
due authorization/cleanup state immediately and every 30 seconds. A session's
future-action lifetime never ends an ongoing logical download.

The final explicit real PostgreSQL suite passed (42.043s), with pure lifecycle
tests (2.124s). Both independent roles passed the complete scoped production code,
SQL, tests, CLI and design record after repairs. Tests cover consume/cancel races,
zero-partial rollback, original result recovery, 61/62d and post-write crossing,
lease/epoch/NULL fences, JCS integer equivalence, private/PUBLIC/SET-column rights,
complete read sets and the actual restricted worker. The replay-policy control
removes only the target SQL guard in an isolated database and then completes a
legal OwnerReplay NULL result, so downstream rejection cannot mask the regression.
The reference-byte test refuses 4006 refs below the item cap, independently of
4097-item refusal. Original auth/session and temporal/mutation controls are
explicit isolated fixtures, not production T17/T19 or physical trust evidence.
Synthetic ciphertexts prove SQL atomicity, not future signed-response acceptance.

Full default Go tests/vet, module verification, all four command builds, pinned
artifact checks and client architecture guard passed. Default external opt-ins
remain distinct from the explicit PostgreSQL run. No HTTP/browser, live OTP,
physical platform, custody or deployment certification is inferred.
This completes the scoped first storage/maintenance batch; full T44 remains in
progress. ActiveValidate, 24-hour pending session/30-day transaction settlement,
execution edges, staged/waiting ownership, safe activation successors and channel
composition remain next. Activation stays closed without the actual waiting
source. Undecidable client trust proof, platform/data/preparation-budget and
migration bootstrap items remain deferred as requested. Deployment is deferred;
downloads have no hard total or cumulative elapsed limit.


### T44 batch 2: validation coordination and original time windows (2026-10-12)

The separate service now has additive lifecycle migration 2. Migration 1 and the
pinned V1 wire schemas remain unchanged. Private activeValidate registration and
rebind repeat the actual T06 operation identity/owner, full scope, original
preparation binding, session CAS and safe prospective fence. Failed coordination
closes atomically; successful new authorization remains unavailable until actual
credential, preparation, workflow and qualification owners exist.

An optional trusted T06 reservation stage joins initial validation to ledger
insertion/owner acquisition before their shared commit, and rebind to Execute or
background recovery claim. It uses the earlier original lease/owner deadline and
a final checkpoint fence. The actual-engine fixture verifies rollback of both
sides, lost reservation acknowledgement, original-result takeover, fresh worker
recovery, final owner fencing and bounded SQL wait cancellation. It uses real
projection/result encryption; its fixed publisher is isolated and never registered
in production. It cannot certify client trust or successful authorization.

Fixed maintenance closes pending sessions/active validation at original start+24h
and transaction reporting at original start+30d. Persistent reporting_timeout
distinguishes missing reports from confirmed local failure; existing terminal
records remain immutable. Session expiry fences future actions without modifying
logical workflow download state or creating a synthetic committed T06 response.
Complete capture refuses un-settled due scope projections before background scans.
No retry resets the original deadline, and no hard download deadline is introduced.

Independent review found and repaired first-registration atomicity, persistent
reporting reason, due-read refusal, safe-integer fencing and reservation-stage
lease bounds. Real PostgreSQL tests exercise additive v1-to-v2 upgrade/repeat,
digest mismatch, closed private/PUBLIC privileges, null CAS, unique active races,
original identity rebind, exact deadlines and post-write crossing, safe highest
epoch, terminal immutability and zero partial writes. The full real T06/T44
regression passed (314.536s/302.025s); the final reservation context test passed
(24.534s) after the last guard delta. Final current-code verification is recorded
in the service lifecycle plan. Default external opt-ins remain separate from
explicit integration evidence.

Full T44 still requires execution edges, staged completion/waiting slots, safe
activation cancellation and channel invalidation, followed by integration with
actual T17/T19/T20 owners. Platform/data certification, preparation measurement
approval, specific-client current Root proof and deferred deployment are not
silently approved through fixtures. Continue unblocked work rather than reopening
owner decisions or claiming full-task acceptance.


The final current-code full lifecycle PostgreSQL suite passed (167.554s); both
independent read-only roles passed the complete scoped final delta. Full default
Go tests/vet, module verification, all command builds, pinned artifact checks,
formatting/diff checks and the client architecture guard passed. No unfinished
T44 work or deferred production decision is counted as accepted.


### T44 batch 3: protected execution events (2026-10-12)

The service now has additive migration 3, leaving committed migrations 1/2 and
pinned wire contracts unchanged. Purpose-specific execution paths, initial
consumed acknowledgement, sequence/revision, occurrence/reception windows,
protected safe-stop evidence, earliest active boundary and original health anchor
are repeated by private SQL. Actual T06 OwnerReplay fences both sides of the
writes, and complete reads retain historical/terminal transaction membership.

Both independent roles passed the complete repaired delta. Review corrected
JSON integer spelling parity and the V1 upgrade fixture. Explicit real SQL tests
cover Go/SQL body parity, atomic rollback, purpose/terminal closure, wrong event
replay policy with an independent legal completion control, incomplete safety and
boundary facts, health/fraction/null/unsafe failures with legal decimal/exponent
controls, old-data V1/V2 upgrade/repeat, private grants and digest mismatch.
Final whole-suite results are recorded in the service lifecycle plan.

This scoped batch does not certify full T44 or absent T20 credential/deny/dedup
owners. Single-record staging completion and activation terminal writes remain
closed until staged/preparation composition exists. Continue session events,
waiting slots/safe successors and channel invalidation without reopening deferred
owner decisions. Downloads retain no hard total or cumulative elapsed limit;
client trust proof, physical evidence/budgets and deployment remain deferred.


### T44 batch 4: protected ordinary session events (2026-10-12)

Additive migration 4 implements actual ordinary session-state composition with
purpose, sequence, CAS, original occurrence/reception window and OwnerReplay
fences. Verification requires the original object and verified result; preflight
requires support and the exact scope; silent permission requires the existing
helper. Freeze and backup bind exact owner/epochs/data scope; readiness binds the
original preparation owner. Terminal events retain prior state and fence future
actions. No download total or cumulative hard deadline is added.

Both independent roles passed the repaired complete scoped delta. Review closed
missing intrinsic evidence, readiness epoch and ordinary-cancellation defects.
Actual restricted PostgreSQL cases verify all purpose/backup paths, Go/SQL parity,
rollback and wrong/missing evidence with legal controls, safe numeric spellings,
private rights and V3 upgrade preserving active validation, final 24-hour crossing,
double-owner CAS race, terminal preservation, null/stale CAS and no-update.
The final current-code entire lifecycle regression and baseline evidence are
recorded in the service lifecycle plan.

The ordinary event writer cannot bypass validation, authorization, workflow
resume, staged verification or lifecycle cancellation. Actual T17/T20 owners
still need credential, deny, workflow and atomic event-ledger/outbox composition.
This is scoped kernel progress, not full T44 acceptance or physical certification.
Continue pending lifecycle cancellation, staged/waiting composition and channel
invalidation while deferred client trust/platform/budget/deployment decisions
remain recorded and skipped.


### T44 batch 5: atomic pending lifecycle cancellation (2026-10-12)

Private additive migration 5 cancels a writable update session and fences its
original activeValidate in one actual StandardReplay/scope transaction. Original
sequence, start and immutable identity remain unchanged. Already issued rights
must use the shared authorization-end path. Ordinary telemetry cannot cancel;
no local stop, cleanup or logical-download termination is inferred. The original
validation's T06 response is not fabricated; the future T19 owner must use its
own operation and compose current authority/cleanup/workflow effects atomically.

Both independent roles passed the production delta and final test additions.
Explicit PostgreSQL cases passed for original identity and Go/SQL parity, full
rollback, rejection of old validation completion, null/stale CAS and wrong replay
policy with legal control, competing registration, issued-authority closure with
shared-end control, V4 data preservation/private grants/digest, final 24-hour
crossing with the same owner/CAS positive control and safe highest terminal epoch.
The final entire lifecycle integration and baseline results are recorded in the
service lifecycle plan. No production credential/API, physical or deployment
acceptance is implied. Continue the remaining kernel and independent task work;
undecidable owner choices remain recorded and skipped as requested.


### T44 batch 6: original session creation (2026-10-12)

Private additive migration 6 creates install/preinstall sessions under an actual
StandardReplay owner, exact complete-scope CAS and SQL-derived original time.
Revision-zero absence is a real read fact; concurrent creation yields one winner.
Current decision/qualification/workflow writes remain the future T17 owner's same
transaction responsibility; regular activation prototypes stay closed. No new
client credential or physical preparation certification is inferred.

Review repaired prospective membership capacity and safe JSON integer spelling.
Actual PostgreSQL repair cases passed at the exact 4096-ref, 1 MiB reference and
32 MiB member aggregate bounds, with over-capacity rollback and complete capture.
Decimal original preparation fields now traverse initialization and ordinary
preinstall events into existing validation registration. Resource fixtures prove
internal byte limits only; downloads have no hard total/cumulative elapsed limit.
Final full integration and independent review evidence is in the service plan.
Continue remaining lifecycle work and independent tasks while owner decisions
and deployment remain recorded and skipped.


### T44 batch 7: atomic waiting content (2026-10-12)

Private additive migration 7 completes verified preinstall and creates one
waiting slot with exact original bindings, safe-stop/slot evidence and actual
OwnerReplay/scope CAS. SQL supplies original staged time; content expires at
min(completion + 30 days, original hop end). Completion immediately releases
active transaction occupancy; fixed expiry invalidates waiting content without
inventing physical deletion or local failure. Complete scope reads retain staged
history. Current T20 qualification/preparation guards and release still belong
to its final same-transaction composition.

Independent review repaired mandatory convergence reserves across older scope
writers. Fixed additive overlays change only 13 revision writes in 12 old
functions, with exact-equivalence tests and immutable migrations 1-6. Waiting
reserves future invalidation, live terminal growth, missing outcomes, ref widths
and terminal/cleanup revisions. Older ordinary events cannot consume the reserve;
cancellation and expiry can use their own reserved writes. Targeted actual
PostgreSQL tests passed (48.526s plus 13.430s); both independent roles passed,
and the final full lifecycle PostgreSQL regression passed (404.978s). Baseline
checks and detailed evidence are recorded in the service plan. No hard total/cumulative
download timeout, physical/qualification certification or deployment is inferred.
Continue activation/staged successors, channel invalidation and remaining tasks;
undecidable owner choices remain recorded and skipped as requested.


### T44 batch 8: activation session and restart confirmation (2026-10-12)

Private additive migration 8 binds a new activation Session to actual waiting,
original completed preinstall, consumed authorization/Session, exact scope,
channel, slot, object, context/hop and workflow. StandardReplay initialization
uses a distinct Session ID and new SQL 24h action-window start. The private
OwnerReplay restart command alone advances update_offered -> staged_verified,
requiring exact slot/object/hop and actual waiting/source re-reads. It leaves
waiting state/revision, original staged time/deadline and preinstall terminal
untouched. Ordinary events remain closed to this edge; no execution rights or
waiting consumption are issued. Current T17/T20 qualification, workflow,
preparation and authenticated local restart evidence remain their same final
transaction responsibilities.

Both independent roles passed the production/test delta. Actual PostgreSQL
controls passed for original identity/rollback, wrong bindings and owner/CAS,
legal numeric spelling, exact 4096 reads, migration/private grants/digest,
original content and independent Session final deadlines/clock rollback with
same-owner positive controls, concurrent creation/restart and expiry competition.
A two-day-later activation retains its own new 24h Session window without
extending original waiting content. The frozen full lifecycle PostgreSQL
regression passed (411.633s); baseline checks passed and their evidence is
recorded in the service plan. Full T44 and production/physical acceptance
remain unfinished; continue remaining private lifecycle commands and unblocked
tasks while existing owner decisions/deployment stay recorded and skipped.

### T44 batch 9: atomic activation consumption (2026-10-12)

Private additive migration 9 consumes only actual available activation
authorization, authorized Session, original minimal attempt and waiting/
completed-preinstall lineage. Scope/Auth/Session CAS, request/status bindings,
five staged claims and complete frozen selection identity are re-read. Metadata
references can refresh while original Package/object/mode/plans remain fixed.
Auth consumed, Session fenced, new Transaction, minimal Outcome and staged
activating commit together under StandardReplay, retaining original staged
times and repeating owner/authorization/Session/staged/clock guards at the end.
Activating preserves full member/byte/ref/counter and terminal convergence
reserves; waiting expiry cannot interrupt an already consumed activation.

Targeted actual PostgreSQL checks passed: basic/recovery/migration (9.660s),
legal numeric spellings and exact complete member/32 MiB terminal-growth/scope
revision controls (20.103s), full staged/selection bindings, fresh metadata and
consume/cancel/expire races (11.453s). Available authorization creation remains
an explicit isolated identity fixture; original waiting, new Session/restart,
attempt registration, consumption and T06 commits are real private ports.
Production issuance remains closed pending existing dependencies. T44 owns
authorization storage/lifecycle, with actual T19 signature/current/preparation/
freeze/backup guards in its same final transaction. Compound activation terminal,
reporting timeout and safe successor remain subsequent work. Final complete
regression and independent review evidence follows in the service plan; no
physical, credential, current qualification or deferred deployment acceptance
is inferred. Continue remaining unblocked tasks and record/skip owner decisions.

Both independent roles passed this complete code/test/documentation batch. The
frozen full actual lifecycle PostgreSQL regression passed (460.289s), including
initial/final deadlines and races. Baseline tests/vet/build/module/pinned-contract
and client architecture checks passed; detailed evidence is in the service plan.
This closes B9 only and does not mark full T44 or dependent production complete.

### T44 batch 10: consumed activation terminals (2026-10-12)

Private additive migration 10 atomically binds actual consumed activation to
its original completed preinstall and activating staged content. OwnerReplay
success/activated or proven failure/failed shares Scope/Transaction/Staged CAS,
lawful sequence and existing health/safety facts, final owner/time checks and
full capture bounds. Original staged times, preinstall and consume outcome stay.
Events and reporting use the activation Transaction's original 30-day window;
expiry of earlier action credentials, Session or waiting content cannot undo
consumption. Fixed reporting maintenance produces no client event or local
damage fact. Exact old B9 reporting terminals can converge only their original
still-activating content, preserving the old Transaction; unknown history is
refused. Safe cancellation/successor remains subsequent work.

Actual PostgreSQL tests passed full healthy/failure paths, atomic rollback,
same-owner retry, original deadline and old B9 compensation, execution after
old action/content expiry, clock NULL/unsafe/rollback (31.787s), terminal and
maintenance races (15.262s), exact 4096 reads and 32 MiB aggregate with one-byte
rejection, migration/private grants/digest and unchanged old maintenance
(16.674s). Resource/time injections are explicit adversarial fixtures, not
credential, current qualification, physical preparation or deployment evidence.
Full regression and final independent review evidence follow in the service
plan. Full T44 and production composition remain in progress; continue remaining
unblocked tasks while recorded owner decisions/deployment stay deferred.

Both independent roles passed the frozen complete B10 code/test/documentation
delta. Full actual lifecycle PostgreSQL regression passed (553.199s), along with
default Go tests/vet, module verification, all commands, pinned contract checks,
formatting/diff and client architecture guard. This closes B10 only; full T44
and production composition stay open.

### T44 batch 11: safe activation cancellation and waiting successor (2026-10-12)

Private additive migration 11 confirms actual safe cancelled_before_install
under OwnerReplay, preserving original activating content until separate
successor/cleanup. StandardReplay rebuild repeats actual confirmed cancellation,
original lineage, full stopped-owner/old identity/data/intact slot proof and
the full immutable update. Fresh qualified metadata can consume/cancel, but
selection-only equality cannot rebuild an old slot. Unique old cancelled/new
waiting records commit together, with monotone revision and explicit successor/
cancelled-transaction links, preserving original staged times and preinstall.
New membership retains complete capture and mandatory expiry/cleanup reserves.

Actual PostgreSQL tests passed atomic rollback and same-owner retry, original
expiry, fresh metadata/full safety bindings, exact membership and scope reserve,
private migration rights/digest, clock faults, concurrent rebuild/cancel-boundary
winners, exact 32 MiB expiry projection and real T06 encrypted-result replay
after engine restart. Later activation requires a new unprivileged Session and
a newly verified restart edge; no old execution, preparation or backup revives.
Current-channel/qualification/preparation source and final owner composition
remain future work; private kernel fixtures do not certify those or deployment.
Full regression and final independent evidence follow in the service plan.
This batch does not complete full T44; continue remaining unblocked tasks.

Both independent roles passed the complete frozen B11 code/test/documentation
delta. Full actual lifecycle PostgreSQL regression passed (659.411s); default
Go tests/vet, module verification, all commands, pinned contract inventory,
formatting/diff and client architecture guard passed. Detailed evidence remains
in the service plan. This closes B11 only; full T44 stays in progress.

### T44 batch 12: protected channel storage (2026-10-12)

Private additive migration 12 initializes an explicit stable/beta/internal
channel only with actual complete fresh scope absence and StandardReplay
fencing. Existing empty/populated legacy scopes are refused; no old binding or
default supplies current configuration. The channel participates in every full
capture, hash/revision CAS and 4096-member/1 MiB refs/32 MiB aggregate bound.
Actual PostgreSQL controls passed atomic rollback, fresh-scope competition,
legacy-preserving migration, input/clock/owner/permission/digest refusals and
exact channel-inclusive capacity with one-byte negative controls. Real T06
restart replay returns the original encrypted result without another write.

This storage batch opens no channel-change or qualification workflow. Existing
historical private action ports have not yet adopted current-channel guards;
the next fixed batch must compose all new-action guards and full invalidation
before any production registration. Shared machine settings are T40's same
final transaction; legacy migration requires real T17 configuration. Protected
presence does not certify current qualification or deployment. Full regression
and independent review evidence follow in the service plan. Full T44 remains
open; continue other unblocked tasks and skip recorded owner decisions.

Both independent roles passed the frozen complete B12 code/test/docs delta.
Full actual lifecycle PostgreSQL regression passed (729.182s); default tests,
vet, module verification, all commands, pinned contract and client architecture
checks passed. This closes B12 storage only, leaving B13/full T44 open.

### T44 batch 13: current guards and atomic channel change (2026-10-12)

Private additive migration 13 adds actual current-channel guards to every new
Session/validate/consume/activation-restart/staging-complete/waiting-rebuild port.
It fails closed on missing legacy configuration. StandardReplay bulk change
compares actual Scope/channel CAS and complete mutable membership, atomically
fences pending Sessions, aborts Validate epochs, ends available authorizations
and original Outcomes, invalidates waiting, and only requests safe cancellation
of consumed preinstall. Already-consumed install/activation and old original
result/reporting paths retain frozen lineage and can safely converge. Returning
to the same channel name never resets channelRevision.

Protected initial/final SQL clocks, parent membership, original deadlines and
full capacity preserve terminal/cleanup convergence. Every later write in an
actually changed-channel scope retains those reserves; ACK/new Session growth
cannot consume them, while actual terminal and cleanup release their respective
obligations before the guard. Due inventory may be
captured only for this invalidation operation; ordinary qualification/execution
still refuse unsettled due state. No hard total/cumulative download deadline is
introduced. Explicit current and legacy fixtures remain separate; only channels
removed by declared reverse-schema simulation may be restored from an independent
known test source after checking production migration did not backfill them.

Targeted real PostgreSQL controls cover current-source guard failures/positive
retry, atomic rollback, Validate/Outcome, three consumed purposes, ABA/races,
clock boundaries, member/raw/ref and convergence reserves. Real T06 engine
restart replays the original encrypted result without another channel mutation.
Both independent roles passed the frozen B13 code/test/docs delta after repairs.
Final full actual lifecycle PostgreSQL regression passed (826.957s); default
tests/vet, module verification, all command builds, pinned contracts, formatting,
diff and client architecture checks passed. This closes B13 only.
This batch opens no production configuration or client-credential owner. T40
shared-machine settings and affected scopes, actual T20/T45 preparation release,
legacy T17 configuration and recorded owner/deployment decisions remain separate.
Full T44 stays in progress; continue other unblocked tasks.

### T47 first batch: protected draft and frozen policy (2026-10-12)

An isolated service worktree now implements original-schema plan conversion,
protected full-scope draft revisions, exact T05 ownerBound plan approval and a
fixed T06 freeze/replay owner interface. Twelve normal/forced/silent vectors
match original reference canonical bytes. Draft edits invalidate old approval;
frozen policy is immutable. Stable keeps actual dual review and fresh MFA.

Private SQL captures actual T06 identity/epoch at insert/claim, verifies original
lookup/request digests and canonical receipt/proof bytes, and repeats actual
owner/time/CAS/source/approval guards before committing plan/result/audit/outbox.
Fresh same-actor evidenced reentry can update protected authentication for crash
recovery. Runtime receives no private mutator rights; isolated fixed publishers
exercise the real kernel and do not certify production composition.

Non-null SN references remain closed until their actual approved group source
exists; null is the independently supported path. Lifecycle/StageFacts and actual
T14/T15/T08 selection-changing composition remain subsequent T47 work. No
download deadline or caller-supplied qualification flag is introduced. Both
independent roles passed the frozen complete batch after repairs. Actual
PostgreSQL regression passed (62.200s), strengthened post-write clock controls
passed (30.678s) and exact original-request digest controls passed (3.532s).
All default tests/vet, module verification, command builds, pinned contracts,
formatting, diff and client architecture checks passed.
Full T47 remains in progress; continue other unblocked tasks and skip recorded
owner/deployment decisions.

### T09 first batch: bounded ZIP inspection (2026-10-12)

The service artifact feature now inspects complete ZIP physical layout, portable
relative paths and regular object types before decoding. It independently checks
each actual expanded size, CRC and SHA-256, including descriptor-free zero CRC,
and refuses unconsumed compressed tails. Nested ZIP layers share explicit input,
file, total expansion, entry, ratio, depth, metadata and live-buffer capacities.
The 32 MiB business-batch limit is not used as a package size policy, and no
elapsed download cap is introduced.

An isolated inspection command captures an exact bounded memory snapshot through
explicit OS source adapters. Its report grants no Artifact validity, native
authenticity, protected executable handoff, SBOM/pipeline approval or publication
authority. ZIP64, split/prefixed archives and unsupported extensions/hosts fail
explicitly. T09 remains in progress pending the remaining actual validation,
immutable storage, upload workflow and T08 quarantine composition. Both
independent roles passed the frozen code/test/docs after repairs. Actual rebuilt
CLI E2E and Windows source controls passed; archive/source/CLI packages passed
(2.137s/5.782s/8.853s). Final 30-second mutation fuzz passed 1,938,358 executions
(31.232s). Default tests/vet, module verification, command builds, pinned
contracts, formatting, diff, client architecture and version checks passed.
Linux/macOS source and inspector compile checks passed; actual OS runs remain
unverified. Host symlink and existing mapped-remote-source tests explicitly
skipped for missing privileges/source. Production source certification and
unaffected external database/provider/browser suites are not claimed here.


### T09 second batch: signed Manifest and inventory binding (2026-10-12)

The artifact feature captures the exact fixed root Manifest under the approved
1 MiB bound while completing the whole ZIP inspection. Original closed Schema,
canonical envelope signatures, complete scope, intrinsic Package semantics and
current Root policy are reused. Root expiry, package-role denied keys and exact
package subject denies refuse verification. Packages acquire no invented expiry.
Outer ZIP, Manifest envelope and FullPack remain independent byte identities;
SBOM/provenance exact signed names bind only to distinct outer files. Every signed
native identity binds to direct FullPack member size/hash, without inventing
source paths from installationTarget or authenticating publisher/format claims.

An actual PostgreSQL source composes T07 continuous protected Root history and
T08 complete deny in the same SERIALIZABLE transaction. It binds the actual SQL
connection/database/login/current identity, committed schema digests and frozen
shared namespace/table owner to the checked runtime source. A token marker or
constructor check on another pool alone cannot grant source trust. SQL time is
read at both boundaries; missing/corrupt source returns no facts.

Real T05 dual approval, T06 owner, genesis and Root v1/v2 publication prepare an
actual package policy before signed byte binding. Integrity/isolation controls
use real PostgreSQL, including another complete protected database with the same
runtime login. They do not manufacture Artifact validity or approved deny
commands. Both independent roles passed the frozen complete code/test/docs after
source boundary repairs. Final actual PostgreSQL regression passed (11.168s;
source 5.35s, Manifest 3.85s). Affected archive/source/verification passed
(6.265s/6.178s/3.902s), along with final default Go tests/vet, module verification,
all command builds, pinned contracts, formatting/diff and client architecture/version
checks. Unchanged provider/browser/deployment opt-ins are not claimed. Actual
native/pipeline/SBOM content verification, storage, upload/error/recovery workflow,
T46 evidence and T08 quarantine are subsequent
work. Full T09 stays in progress; no download deadline is introduced.


### T47 second batch: local lifecycle and original StageFacts (2026-10-12)

The bounded current-instance reducer binds the full protected frozen plan revision,
canonical policy hash and complete scope. It derives first/strictly adjacent/final
100% stages, preserves percentage and quality freezing across pause/resume, refuses
terminal resurrection and retains distinct operator/parent abort causes. Original
closed StageFact bytes preserve old window/watermark on pause/complete/abort and
open new windows on start/advance/resume. Outputs own their pointers; final clock
or source regression is refused and the last revision remains for termination.

Ten actual Node signed-publication/quality fixture vectors compare canonical local
instance and StageFact bytes against the original rollout model. The reducer has
no SQL writer, production command or HTTP registration. Actual T14/T15/T21/T08
read sets, baseline switch, publication, scope indexes and permanent observation
window/history storage remain subsequent work. T23 release of quality freezing
and historical recovery reasons cannot be replaced by ordinary resume. No new
download deadline or full-task/deployment acceptance is introduced. Full T47
remains in progress; continue the unblocked database/history batch.


Both independent roles passed the second-batch frozen code/test/documentation
review. Original Node/domain tests passed (4.690s; independent 7.557s/3.292s),
as did default Go tests, full vet, module verification, all command builds,
pinned artifacts and client architecture/version/diff checks. No SQL/provider
behavior changed; actual database/provider opt-ins were not rerun for this batch.
Service commit: `04e248a`.


### T47 third batch: private persistence and permanent history (2026-10-12)

An additive state namespace persists bounded current instances, immutable original
StageFact revisions and permanent full-scope windows. Private owner writes bind
actual frozen policies, head CAS, source facts, exact local edges and final SQL
time. Runtime receives read access only; no production lifecycle command, parent
composition, quality qualification or publisher is opened by this batch. Paused
instances retain their scope slot; terminated instances retain window attribution.
History reads verify actual plans, first revisions, chronology and source watermarks.

Both independent roles passed the frozen scoped code/test/documentation review.
Complete actual PostgreSQL regression passed (142.931s), and the final retired-window
watermark control passed separately (3.618s). Real history exceeds 4,096 facts and
1 MiB without enlarging current JSON. Concurrent CAS, rollback, final-clock faults,
terminal revision reserve, runtime denial and schema/ACL drift have real controls.
Domain/original Node tests passed (4.178s); final default tests, vet, module
verification, all command builds, pinned contracts and diff checks passed.

Full T47 remains in progress pending actual T14/T15/T21/T08 final read sets and
controlled operation/recovery composition. This closes only the private storage
batch; production commands and deployment acceptance remain closed. Continue
unblocked T21 quality calculation/projection work; recorded owner decisions stay
deferred. Downloads have no hard total/cumulative time limit.

Service commit: `f12f3e5`. Client architecture/version checks passed.


### T21 first batch: exact conservative assessment (2026-10-12)

The service quality feature now evaluates all original required metrics/groups
against a complete frozen normal/forced/silent plan and bounded original-schema
observation. Exact arbitrary-precision rational arithmetic preserves threshold
equality and category sums beyond safe integers. Confirmed failures and unknown
upper bounds reach freezing; incomplete alone waits. Every group/metric needs
its own approved confirmed sample denominator and both strictly-below ratios.
The full observation period remains required, without a total download limit.

Both independent roles passed the complete frozen code/test/documentation delta.
All 36 actual original Node canonical vectors and domain controls passed (2.051s;
independent 4.419s/8.401s). This calculation has no production command, permission
or write registration. Actual T25 purpose-bound eligibility, original T17/T20
lineage, permanent evidence/source processing, watermarks and freeze-reason
latching remain subsequent T21 work. A supplied group set, time or local pass
cannot authorize expansion/completion. Full T21 remains in progress; unchanged
database/provider/browser/deployment opt-ins are not applicable to this batch.

Final default tests/vet, module verification, all command builds, pinned contracts
and client architecture/version/diff checks passed. Service commit: `b85476b`.


### T21 second batch: original evidence outcome calculation (2026-10-12)

The bounded sample reducer preserves exact original workflow, source/hop group,
target, installation scope, metric and window. Legal late original evidence may
resolve unknown; confirmed failure stays latched even when older than the source
watermark. Higher-version repair and auxiliary health cannot replace original
candidate outcomes or increase its success denominator. Successful original
outcomes do not regress through incomplete/unknown progress. Deduplication uses
an independent immutable point receipt; exact replay changes nothing and a
different digest conflicts. No growing event array is embedded in the sample.

Both independent roles passed the complete frozen code/test/docs delta. All 148
actual original projectEvidence comparisons, 36 assessment vectors and domain
controls passed (2.359s; independent 4.634s/27.621s). Final default Go tests/vet,
module verification, command builds, pinned contracts and diff checks passed.
No SQL/provider behavior changed. Actual accepted-event sources, atomic ledger/
sample/source cursor, contribution eligibility/limits and freeze reasons remain
subsequent work; a caller-provided receipt lookup does not grant evidence rights.
Full T21 remains in progress. Continue the protected persistence batch without
introducing any download time cap.

Service commit: `d2ac3ae`. Client architecture/version checks passed.


### T21 third batch: private sample and permanent receipt storage (2026-10-12)

The service stores bounded sample heads and immutable point receipts in a separate
protected namespace. A sample consumes an actual T47 first window fact and frozen
full-scope plan. Its unique scope/installation/window/group/metric slot excludes
workflow so retries cannot add samples or rebind the original target. Registration
starts incomplete; rows grant no contribution or production assessment authority.

Actual configured SQL database/login/current identity, SERIALIZABLE isolation,
pinned shared owner and actual T47 sources guard every access. Runtime receives
read rights only. New receipt/sample writes share one exact-head transaction, with
first/final SQL clocks bounded by real window and predecessor commit. Bounded
readback checks the exact original projection, initial/later snapshot relations,
one immutable predecessor, chronological binding and sticky history. Point history
remains separate from current JSON and outlives the 24-hour response replay limit.

Real PostgreSQL controls passed: complete seven-test regression (36.527s), exact
post-write clock fault with same-event legal positive (7.597s), and complete foreign
database with the same runtime login plus SET-reachable writer refusal (7.992s).
Original credentials remain process-local. Sixty-four real history rows preserve
current bounds and the first receipt; no larger quality-history run is claimed.
Controls also cover real T05/T06-approved policies, replay/conflict, workflow slot,
paused origin, rollback/CAS, raw/runtime denial and schema/ACL/source/token drift.
Final default Go tests/vet, module verification, command builds, pinned contracts
and scoped formatting/diff checks passed.

T21 remains in progress. Actual T20 event acceptance/deny/source processing cursor,
T25 purpose-bound eligibility/limits, aggregate evaluation and freeze reasons must
join the future fixed owner command. Private attributed digests are not accepted
original event authentication. No production ingestion, quality permission or
deployment acceptance is opened; downloads retain no hard time cap. Continue T20
trusted event reception and other unblocked tasks.

Final catalog relations explicitly use pg_catalog. Actual temporary catalog
shadowing of the owner OID with real runtime column write permission refuses
both point reads and private writes; restored normal controls pass. The targeted
control passed (3.809s), and final complete nine-test real PostgreSQL regression
passed (111.344s). Final default checks passed after the production fix.

Both independent scoped roles gave final PASS after the catalog fix. Service
commit: `780bdea`. Client architecture/version/diff checks passed. Full T21
remains open at the production event/eligibility/evaluation boundaries above.


### T20 first batch: original six-role event input verification (2026-10-12)

The service compiles the six original event request schemas from pinned OpenAPI.
Bounded closed JSON, canonical nested credential bytes, scoped Ed25519 under
already-protected Root/role-deny inputs and intrinsic occurrence/upload windows
precede checked values. Request scope/installId and each event's purpose, lineage,
allowed kind and facts hash must bind exactly. The original canonical event bytes
and full-event SHA are retained without aliasing caller input. Current claimed
key deny gives fixed EVENT_KEY_DENIED before any event existence lookup.

Actual original Node API/signature/occurrence verification agrees on 168 cases
including canonical bytes/hashes (4.225s). Fixed signatures for six roles, binding
mutations, parser/credential bounds, actual fresh synthetic signatures and lawful
nbf/event-end-minus-one/upload-end-minus-one controls passed. Final default Go
tests (events: 8.651s), full vet, module verification, all command builds, pinned
artifacts and diff checks passed. No SQL/provider/browser integration changed.

Checked observations do not accept events, acknowledge persistence, authorize
state changes or contribute quality. Complete production T07/T08 final sources,
actual lineage/sequence, exp+two-minute durable ledger, T44 atomic normal/late
composition, contribution cursor and protected client outbox remain subsequent
work. Full T20 remains in progress. Downloads retain no hard total duration limit.

Both independent scoped roles gave final PASS; their actual original Node
runs passed (4.335s/4.149s). Service commit: `5d47583`. Client architecture,
version and diff checks passed. Continue actual protected source/ledger work.


### T20 second batch: actual current protected event sources (2026-10-12)

The service joins six-role original input verification to actual published T07
Root, complete T08 role-specific deny, SQL time and exact same-transaction source
revision/hash references. A shared signing read adapter preserves T09's existing
API and fixed errors; its previously reviewed archive/Manifest batches are
integrated without modifying frozen SQL or normative schemas.

Configured endpoint/database/login, current identity, SERIALIZABLE isolation,
pinned source owner and actual read-only rights guard each read. Catalog relations
and functions are qualified; effective function/operator resolution must put
pg_catalog before all other schemas, including temporary schemas, before any reused query. User triggers
remain forbidden after their original creation grant is revoked. Unsafe paths,
real column-write permission hidden behind a temporary catalog, source/owner
changes and foreign/weak tokens produce no partial snapshots.

Actual PostgreSQL fixtures publish both original Root generations with real
protected genesis and dual approvals, then validate six process-local synthetic
signed input families. They establish current role deny and real catalog,
function, isolation, clock, comparison-operator, path-changing temporary-domain and persistent-trigger faults
with restored positive controls. Targeted final search-path/operator controls
passed (15.406s), and temporary-domain resolution controls passed (4.793s). Final
combined eight-test actual PostgreSQL regression passed (46.714s), including
catalog-first actual column-writer refusal independent of the path guard. Final
default Go tests, full vet, module verification, all command builds, pinned
contracts and diff checks passed. Client architecture/version checks passed.

These checked inputs are not accepted events, production-issued lineage or
execution/quality qualification. Durable ledger, real lineage/sequence, atomic
T44 normal/late diagnostic/outbox/contribution composition and protected client
outbox remain subsequent work. Full T20 stays open. Downloads have no hard total
or cumulative duration limit; Previously recorded pending policy decisions and deployment remain deferred.


Both independent scoped roles gave final PASS after the resolution, actual ACL
and trigger fixes. Service commit: `cf73e49`. Integrated T09 prerequisite commits
are `c34fc4a` and `f3a7529`. Continue durable event receipt/lineage composition;
full T20 remains open at the explicitly listed boundaries.


### T20 third batch: immutable receipt/replay calculation (2026-10-12)

The service domain calculates bounded original event receipts with immutable
credential/event/full-digest binding, original disposition/commit time and exact
exp plus two-minute retention. Canonical tuple lookup excludes ambiguous string
concatenation and event-only keys. Exact replay changes no original receipt,
sequence, terminal outcome or retention, including beyond T06's 24-hour response.
Upload expiry still refuses and impossible/corrupt values return no partial plan.

All six original signed families cover seven/37-day replay and exact expiry.
All 24 actual original planEvent receipt projections agree for three transaction
purposes, fresh/late, 24-hour and post-timeout replay, changed event/credential and
expired upload (complete receipt run: 4.383s). Key binding/ambiguity, safe-integer,
input mutation and stored corruption controls pass. No SQL/provider behavior
changes; calculation grants no persisted acceptance, ACK, execution or quality.

Actual current Root/deny/input and lineage binding precede every future lookup,
including replay. The owner must still join protected receipts with actual
normal/diagnostic cursors, T44 state writes, outbox and contribution watermark.
SQL ownership, key usage and protected client transport remain subsequent work;
full T20 stays open. Downloads retain no hard total/cumulative duration limit.


Final default Go tests passed (events: 7.564s) with the actual original reference
enabled. Full vet, module verification, all command builds, pinned artifacts and
diff checks passed. Both independent scoped roles gave final PASS; their events
runs with 24 receipt and 168 input comparisons passed (12.009s/13.425s). Service
commit: `45e73ee`. Client architecture/version checks passed. Continue actual
protected historical lineage and durable composition without pausing the task.


### T20 fourth batch: actual protected historical lifecycle binding (2026-10-12)

The service joins current Root/deny/input verification to actual same-transaction
T44 Session and consumed Authorization/immutable Update/Transaction facts and
complete scope references. All thirteen source migrations, shared provisioning
owner, actual database/login/identity/isolation, catalog parsing, source relation
ownership/ACLs/column rights/triggers and private function rights are checked.
Originating decision ID/revision is required for each transaction purpose in
addition to original ID/start/auth JTI/install/scope/channel/purpose/final identity.

Narrow historical ports can read original due or terminal records without future
execution rights. Ordinary Session capture preserves its due rejection. Combined
Root/deny/lifecycle dependencies independently enforce 4096 references and 1 MiB
canonical bytes; exact ceilings pass and +1 refuses without partial snapshots.
Current key deny wins over absent/unreadable lineage; T22 task credentials cannot
borrow execution lineage. No SQL migration bytes or original API schema change.

Real PostgreSQL storage/signature controls cover actual historical Session and
all three consumed transaction relations, original-decision mismatch/restoration,
terminal preservation, migration/actual column-write failures and deny ordering.
Fixtures establish actual reads, not issuance, T17 eligibility or accepted writes.
Durable ledger, normal/diagnostic cursor, workflow/outbox/contribution composition,
fixed owner command and protected client transport remain subsequent work. Full
T20 stays open; download duration remains unlimited and pending decisions remain
skipped as requested.


The complete ten-test actual PostgreSQL source regression passed (156.513s).
The strengthened three-purpose 31-day original occurrence/37-day upload control
then passed (23.213s): ordinary due transaction capture refuses while historical
binding preserves the original active/terminal record, without executing it.
Exact combined read-set bounds passed (1.306s). Final default Go tests passed
(events: 16.022s; adapter: 3.575s), with both original Node reference suites.
Full vet, module verification, all command builds, pinned contract and diff
checks passed. Both independent scoped roles gave final PASS for the complete code, tests and
documentation after all findings were repaired. This closes the historical read
batch only; actual atomic event reception remains subsequent work.


Service commit: `28659ce`. Both independent roles passed the final complete
historical-source code/tests/documentation. Client architecture/version/diff
checks passed. Continue protected durable reception and final trust locking;
source snapshots remain observations rather than acceptance authority.

### T20 fifth batch: private no-update durable receipt kernel (2026-10-12)

The service prepares one original signed no-update telemetry event from current
protected Root/deny and actual historical Session. A new explicitly provisioned
private reception schema joins real T06 fixed-command ownership, final Root/deny
row locking, live Session CAS, immutable receipt and original-byte outbox in one
transaction. It creates no quality contribution or runtime writer grant.

The private ABI requires canonical original event/binding bytes and facts digest;
public integral JSON spellings are normalized by original validation/JCS first.
Shorter lawful occurrence windows and an end equal to upload expiry are preserved.
Existing receipts retain all original fields and are checked for canonical shape,
digest, row binding, original retention and commit chronology. Fresh and replay
both recheck owner and SQL time at return, including NULL/unsafe/backward time,
credential expiry and Root expiry. Actual inherited/SET/default ACLs and catalog
ownership/function configuration/trigger drift refuse unsafe provisioning.

This is a private live no-update/replay kernel. Its real PostgreSQL fixtures use
actual protected Root and T06 reservations with test-only opaque projections;
they do not certify credential issuance, an actual T17 decision or a registered
processable owner. Initial late reporting, diagnostic cursors, workflow/download
handoff, transaction/task events, contribution projection, resumable protected
input storage, runtime readiness and production HTTP/client transport remain
subsequent work. No legacy SQL bytes or normative schema change. Full T20 stays
open; download duration is unlimited and undecidable items remain skipped.

Final actual PostgreSQL event regression: fourteen tests PASS (97.135s); isolated
reception suite: six tests PASS (40.885s). It covers canonical/short-window controls,
receipt corruption/restoration, fresh/replay final clocks, stale Root/deny snapshots,
deny-before-receipt-existence, default inherited/SET ACLs and real wrong-command/
epoch refusal. Full default Go tests, vet, module verification, command builds,
pinned contract, client architecture/version and diff checks passed. Other opt-in
database/provider tests are skipped by the default run; this batch's PostgreSQL
tests were explicitly enabled. Production/cross-repository E2E is not certified.
Both independent scoped roles gave final PASS for the complete fifth-batch code,
tests and documents. Continue protected recovery and actual owner composition.
Service commit: `fff5e1c` (pushed). This freezes the new reception migration;
subsequent SQL changes must use additive migrations.

### T20 sixth batch: no-update diagnostic calculation (2026-10-12)

The service domain planner distinguishes live state writes from lawful late
diagnostic cursor writes and existing receipt replay, using actual original
Session state and trusted time. Replay precedes new fences/cursor/sequence work.
T44 validates a historical edge at original occurrence using an ephemeral
projection; the original Session is preserved and no future action is renewed.
No-update records cannot invent freezing/preparation or quality contribution.
Malformed/mismatched/exhausted cursors, sequence/edge errors, channel cutoffs
and upload expiry refuse; output cursor retains exact original diagnostic fields
and original credential retention. Calculations do not alias input records.

The unmodified original Node event model supplies 51 comparisons over all three
execution purposes and live/late/reserved/full-cursor/replay/conflict/expiry cases.
Exact receipt/cursor bytes and write separation match. Additional domain tests
cover corrupted/exhausted cursors, fence equality and original revision exhaustion.
Targeted tests passed (3.077s). No SQL or normative contract changes; actual late
cursor/receipt/outbox transactions, recoverable registered owner and HTTP/client
transport remain later work. Full T20 is open; downloads have no hard time cap.

Final default full Go tests passed (events: 23.468s) with the original input,
receipt and 51 no-update Node comparisons enabled. Module verification, vet,
all command builds, pinned contract and client architecture/version/diff checks
passed. No SQL change requires a new database test in this pure batch; default
database/provider opt-ins remain skipped. Actual late reception/E2E remains open.
Both independent scoped roles passed the complete frozen code/tests/documents;
their independent targeted Node/domain runs passed (7.384s and 3.490s).
Service commit: `a92fbf4` (pushed). Continue actual diagnostic SQL composition
without rewriting the frozen reception migration or pausing later work.

### T20 seventh batch: actual no-update diagnostic transactions (2026-10-12)

Additive reception migration two supports private live/late/replay composition
while preserving migration one's exact bytes, original receipt and outbox.
Explicit initialization records/verifies the new digest and grants runtime only
reads. Shared helpers recheck actual T06 owner, locked current Root/deny bytes,
canonical input/facts and original scope/Session binding before receipt lookup.
Existing committed/diagnostic receipts replay unchanged; malformed/null/unsupported
dispositions refuse. No production command or HTTP is enabled.

Late observations call T44's historical edge calculation and write only a protected
diagnostic cursor, immutable receipt and original-byte outbox together. Original
Session/Scope revision and terminal state are preserved; sequence/edge and channel
cutoffs remain strict. Final NULL/unsafe/backward/expired clock faults roll back
all late writes. Cursor compatibility covers the original reservation and optional
null/absent fields, closing the independent review finding with six lawful
profiles and twelve actual corruption/restoration controls.
Explicit string guards prevent JSON number/bool identity coercion. A real signed
Session "123" has numeric cursor/event lineage single-field counterexamples with
zero-write checks and same-owner original-string restoration.

Real PostgreSQL tests include unswept two-day history, actual T44 expire_pending
versus stale preparation/rebinding, exact domain receipt/cursor comparison,
rollback/replay/sequence/cutoff/deny ordering and predecessor upgrade/digest drift.
Final twenty-test actual event regression passed (123.749s); the new migration is
LF-only and predecessor upgrade/digest checks passed independently (18.616s).
Default full Go tests, vet, module verification, all command builds, pinned
contract and client architecture/version/diff checks passed. Other database/
provider opt-ins remain skipped by default; this scope's PostgreSQL cases were
enabled explicitly. Production/cross-repository E2E remains unverified. The frozen
first migration was untouched. Fixtures establish private
storage behavior, not credential issuance, T17 decisions or registered recovery.
Runtime readiness, recoverable owner, transport, other event kinds and quality
composition remain later work. Full T20 stays open; downloads have no time cap.
Both independent scoped roles passed the final complete code, tests and documents
after all compatibility/type findings were repaired. Continue runtime verification
and actual recoverable owner composition without pausing the remaining tasks.
Service commit: `49eaf63` (pushed). Reception migration two is now frozen;
subsequent SQL behavior changes require additive migrations.

### T20 eighth batch: protected reception point observations (2026-10-12)

The service composes original credential/current Root/deny and actual lifecycle
checks with read-only receipt/diagnostic-cursor observations. Startup and each
read authenticate actual configured DB/login/isolation/catalog resolution,
protected common owner, frozen migration digests and read-only privileges.
Independent review found and closed case-insensitive cursor parsing and missing
actual uniqueness checks. All four tables now have actual column checks, five
enforced immediate primary/unique keys and three validated FK mappings; recorded
migration hashes alone cannot certify present structure.

Receipt replay does not inspect a later diagnostic cursor. Fresh observations
retain complete point references including canonical null absence hashes; final
original verification supplies current SQL time, rejects changed trust/backward
time, and bounds complete combined reads. Invalid storage returns no partial
snapshot. Exact canonical receipt bytes/native hashes and case-sensitive cursor
fields are mandatory; lawful absent/null cursor profiles remain accepted.
SQL bounds body transfer to 1..64 KiB before allocation; removing storage size
checks cannot make an oversized body cross into the runtime process.

Actual PostgreSQL reception tests passed (five tests, 38.361s), covering live/late/
replay, corruption/restoration, denied-key ordering, dangerous permissions,
unchanged-digest structural faults and actual duplicate cursor refusal at startup
and reads. Original Node/domain tests passed (17.708s), adapter tests (5.463s).
Final three source tests after the resource-bound fix passed (21.941s), including
dropped size checks and oversized cursor/receipt refusal with restoration controls.
Full default Go tests, vet, module verification, command builds, pinned contract
and client architecture/version/diff checks passed. Database/provider opt-ins
remain skipped by default; this batch's actual PostgreSQL cases were explicitly
enabled. Production/cross-repository E2E remains unverified.
No frozen SQL or normative contract changed, writer granted, command registered
or HTTP enabled. Observations are not ACKs. Actual recoverable owner/completion,
transport, remaining event kinds and quality composition continue as later work.
Full T20 is open; downloads have no hard total/cumulative elapsed limit.
Both independent scoped roles passed the complete eighth-batch code, tests and
documents after all findings were repaired. Continue actual recovery/owner
composition; this completes only the protected point-observation batch.
Service commit: `143c943` (pushed). Continue without rewriting frozen SQL.

### T20 ninth batch: minimal recovery observation context (2026-10-12)

The service derives a minimal T06 projection only after original signed input,
current trust, actual lineage and reception validation. It stores derived fixed
bindings and one canonical event, without the credential envelope/raw request.
Closed canonical decoding and original scope/pair/lineage/facts/time checks reject
invalid contexts. The complete frame, including every read, obeys the original
1 MiB bound and rejects overflow without truncation.

The internal recovery read port is for the future fixed owner's authenticated
engine projection; parsing does not authenticate a proof or accept client input.
Current actual Root/deny must match the original proof, with denied signing keys
refused first. Independent review found and repaired the historical Root expiry
exception leaking into current recovery observations: both trust reads now reuse
the original RootBody validator at returned SQL time with expiry acceptance off.
Changed proof refuses instead of renewal. Actual lifecycle timeouts refresh only
mutable CAS observations, preserve original event/proof/windows and terminal
Session, and calculate diagnostic output. Existing receipt replay omits cursor
work; all final references/time remain actual complete protected observations.

Actual PostgreSQL recovery tests passed (three tests, 61.986s), covering minimal
context exclusion, real timeout/reconstructed adapter/private binding, replay,
malformed context and deny/proof drift with restoration. Unchanged short-lived
Root tests delay the actual deny read across expiry for fresh and existing receipts
while credential remains valid: pre-expiry succeeds, post-expiry yields no partial
snapshot. Pure Root issuedAt/expiry-1/equality/after and whole-frame exact/+1 tests
passed (1.573s); independent targeted run passed (1.709s).

These are recovery observations, not a registered/sealed production owner or
AES/restart/crash certification. No writer, SQL migration or HTTP is enabled.
Actual T06 completion, transport, other event kinds and contribution composition
continue as later work. Full T20 is open; downloads have no time cap.

Final default full Go tests, vet, module verification, command builds, pinned
contracts and client architecture/version/diff checks passed. Database/provider
opt-ins remain skipped by default; this scope's actual PostgreSQL cases were
explicitly enabled above. Production/cross-repository E2E remains unverified.
Both independent scoped roles passed the frozen ninth-batch code, tests and
documents after all findings were repaired. Continue actual owner composition.
Service commit: `b1feb11` (pushed). Frozen reception SQL remains unchanged.

### T20 tenth batch: registered fixed no-update owner (2026-10-12)

The service now registers the single-event no-update owner with actual T06,
including sealed checkpoint, fenced SQL reservation, recovery and completion.
Original credentials/request bytes remain ephemeral; protected intent storage
holds exact bounded event bytes, while encrypted recovery stores only derived
binding and opaque references. Migration three is additive; migrations one and
two remain byte-identical. Explicit initialization grants only two audited fixed
ports. Actual native function-chain/structure/privilege checks reject drift,
direct writes, public execution and delegation; standalone readers remain read-only.

Permanent event identity binds original scope/lineage/windows and exact event
bytes before acceptance. Processing attempts separately bind request bytes and
current proof generation: lawful signature variants or freshly verified trust
changes share the original intent/receipt without renewing windows. Real SQL
operation lookup/digest, result ID and epoch checks prevent cross-event borrowing.
Current denial precedes storage availability. Prospective computing frames include
both new point references before reservation and enforce complete 1 MiB/4096
limits. Apply joins Session/cursor, receipt/outbox and T06 completion atomically.

Targeted PostgreSQL tests passed for live/late/replay and additive upgrade
(15.498s), cross-intent/function/default drift (29.618s), actual 4096/4097 capacity
(19.559s), natural lease recovery without an original request (34.62s), real child
process exit and encrypted-frame tamper/restoration (37.25s), committed ACK loss
(4.56s), write-after-Apply rollback/recovery and first-intent/signature controls
(48.991s combined), and authority revocation/restoration/deny priority (26.497s).
Exact whole-frame 1 MiB/+1 tests passed (1.564s; independent 1.771s).
Independent review additionally required an explicit real-Apply-success marker
before injecting the rollback failure; the strengthened actual test passed
(40.16s). Both independent scoped roles passed the final code/tests/documents.
Default full Go tests, vet, module verification, command builds, pinned contracts
and client architecture/version checks passed. Default database/provider opt-ins
remain skipped; complete actual PostgreSQL event regression runs separately.
The complete actual PostgreSQL event regression passed (38 tests, 481.977s),
including earlier reception/trust/ACL/clock cases. It compiled before the last
test-only Apply marker change; the separate final 40.16s run verifies that change.
Service commit: `6d3bf2a`. Continue the explicit runnable owner entry points;
the fixed single-event owner does not close full T20.

This completes no production HTTP or batch endpoint. Full T20 stays open until
retention maintenance, transport, other event roles, quality composition and
client outbox are implemented. Deployment/HA and cross-repository E2E remain
postponed. Downloads have no hard total/cumulative time limit.

### T20 eleventh batch: explicit owner commands (2026-10-12)

The service adds three closed internal commands: initialize, enable-no-update,
recover-no-update. Provisioning requires the protected admin connection and an
explicit runtime role; recovery uses only runtime authority and the original
independent T06 key. No automatic schema/permission setup, client input, HTTP or
generic dispatch is added. Actual current deny/owner checks and the single fixed
OwnerReplay registry are reused. Worker cancellation returns before the pool is
closed, and all diagnostics remain fixed secret-safe categories.

Built-binary actual PostgreSQL E2E passed (83.85s); the expanded final run includes
the real run/worker/pool context-cancellation helper (108.76s). Repeated read-only
initialization and explicit enablement, forbidden read-only/admin runtime, missing
and wrong keys, natural SQL lease expiry and original sealed pending-result
recovery are verified. The new process receives no original request and uses a
new epoch while preserving one intent/receipt/outbox. Actual fixture connection
identities are serialized and parsed back; pgx's original ConnString alone cannot
describe fields modified for the isolated database/runtime login.

Default full Go tests, vet, module verification, command builds and pinned checks
passed. Database/provider opt-ins stay skipped by default; command E2E was
explicitly enabled. The final test-only DSN query/Unix socket and single-Wait
cleanup delta changes no product behavior and is checked separately.
The final DSN unit check passed (7.022s package), scoped vet and client
architecture/version/diff checks passed. Both independent roles passed the final
code, tests and three documents without remaining required findings. Frozen
reception SQL is unchanged.
Context cancellation is tested at startup without active work. Harness kill is
not native Windows/POSIX signal or active-work signal certification. Deployment
remains postponed; T06 cleanup does not complete T20 event/intent maintenance.
Full T20, public batches/remaining roles/contribution/client outbox remain open.
Downloads have no hard total or cumulative time limit.
Service command batch commit: `bdd306a`.

### T20 twelfth batch: expired unaccepted intent maintenance (2026-10-12)

The service adds a protected-owner `clean-unaccepted` command. Each serializable
page retires only expired, never-accepted no-update input intents and their entire
still-processing T06 attempt closure. Original immutable expiry plus two minutes,
native canonical identities/hashes, original operation binding, safe ownership
windows and monotonic native SQL time are required. A single active attempt keeps
the whole closure; accepted receipt/outbox/cursor and T44 state remain intact.

Pages are bounded at 32 candidates, 4096 linked attempts and 32 MiB of conservative
logical native-row charges. Exact payload lengths plus bounded fixed-member/header
allowances are counted; this is not a PostgreSQL index/MVCC/peak-memory guarantee.
Oversize closures stay complete while a private cursor permits later work. Each
page commits independently; a failed command explicitly says earlier pages may
have committed and emits no speculative per-page success or event identifiers.

The directly used function chains also receive module-owned path-only upgrades:
operations migration two, lifecycle fourteen and reception four. Explicit catalog
lookup includes temporary schema last; all earlier migration bytes and native
business bodies/ACLs remain unchanged. Initialization resolves legacy DDL safely.
Native body, argument, return, strictness/support and effective path checks prevent
markers alone from authorizing drift or undeclared functions. Existing sealed
frames, keys, result identities, leases and business records are preserved.

Targeted real PostgreSQL evidence has passed for complete oversize/cursor controls
(19.846s), exact logical 32 MiB/+1 (7.385s), sealed operations upgrade and original
null-result OwnerReplay completion (3.887s), lifecycle safety/native-path and V1
upgrade controls (24.103s), original reception upgrade/immutable receipt preservation
(11.602s), and isolated temporary-type boundary regression (28.601s).

All 52 selected actual event cases are verified across the initial regression and
serial rechecks. The initial concurrent invocation failed four database/command
cases (1363.975s); the same executable bytes passed isolated verification, then
all four serial rechecks passed (269.705s). These include genuine original expiry
plus two minutes (155.57s) and built-command recovery (89.80s). The full operations
PostgreSQL package passed (546.695s). The lifecycle invocation exhausted its
25-minute test allowance during fixture creation and found two old modified-clock
upgrade fixtures requiring source restoration. Remaining 74 cases plus three
rechecks passed serially (441.252s), including the final explicit drift-refusal
and clock-restoration controls. All 154 lifecycle cases are verified across runs;
neither initial failed invocation is described as a full pass. Product budgets
were not relaxed. All 19 prior SQL blobs compare byte-identical to the baseline.

Default full Go tests, vet/builds, module/pinned checks and client architecture/
version checks passed. Database/provider opt-ins are skipped by default; the
actual PostgreSQL cases above were explicitly enabled. Both independent roles
passed the final frozen B12 code/tests/documents with no remaining required
findings. This completes this batch; full T20 remains open.

This scope does not complete accepted-data retention, full T20, multi-event public
transport, remaining event roles, quality composition or client outbox. Copied
capacity fixtures do not certify authentication/recovery of new attempts. Actual
deployment, HA and cross-repository/platform E2E remain deferred. Downloads retain
no hard total or cumulative elapsed deadline.
Service maintenance batch commit: `083d404`.

### T20 thirteenth batch: read-only accepted retention inspection (2026-10-12)

The service adds an explicit protected-owner `inspect-no-update-retention`
command, with no role/key/request arguments. Serializable READ ONLY pages
authenticate native receipt/outbox bytes, canonical shape, original JTI/event
identity, disposition and SQL acceptance time. The actual scope and original
Session must match the no-update event and explicitly contain updateAvailable
false; null contribution watermark is insufficient. Native local-state facts
are checked through the existing pinned private helper.

The pure time floor covers original Session start plus seven days and two
minutes, including still-unseen original credentials. Original receipt retention
must fit that window. Expiry, terminal/fence state, Root or channel changes do
not create deletion or replay authority. Every accepted receipt, outbox, intent,
attempt, shared cursor and T44 row remains intact. Legacy acceptance without
later intent scaffolding stays visible. Actual cursor retention and complete
operation closure are not inspected or claimed as proved.

Each page has at most 32 points and 32 MiB conservative logical payload charges.
Native scalar size preflight precedes payload transfer/decoding; an over-budget
point is uninspected and counted while the private cursor advances. The output
reports only elapsed/pending protocol floors, outside-scope kinds and oversize
counts. A kind does not identify its signing role. Pages are separate consistent
observations, not one global inventory snapshot under concurrent acceptance.

Current V1 does not specify accepted outbox consumption/archive ACK or the final
diagnostic/audit retention policy. That decision and real narrow interfaces stay
deferred; minimum exp+two-minute dedup retention cannot authorize deleting the
outbox. The archived historical record supplies no active retention rule.
Independent implementation work continues without expanding the delete surface.

Final serial actual PostgreSQL verification passed all four cases (179.353s):
capacity preflight (7.67s), accepted/legacy/built command (8.21s), original-source
and native-constraint controls (5.83s), and natural shared-window boundary
(154.95s). The original actual
run passed accepted/legacy and genuine expiry/grace cases, but failed two test
fixture expectations; it is not called a full pass. Native Session ID/revision
constraints are now tested as database refusals. The capacity fix moves preflight
before transfer and has a separate synthetic outside-scope payload sentinel;
these copied native rows do not certify signing/acceptance of another role.
The aged Session is seeded explicitly; its genuine signed event's unchanged
upload expiry plus two minutes elapses naturally without rewriting clocks,
credentials, leases or Session timestamps after acceptance.

Full default Go tests with original Node references, vet, module verification,
all command builds, pinned comparison and client architecture/version/diff checks
passed. Database/providers stay skipped by default; actual PostgreSQL/command
evidence above was explicitly enabled. No SQL migration bytes changed. Final
independent review of this frozen code, tests and evidence passed both roles
without remaining required findings. This closes the read-only inspection
capability, not accepted-data deletion or full T20.

This batch does not close accepted cleanup, full T20, other event roles, quality,
public batch transport or client outbox. Deployment and cross-platform/repository
acceptance remain deferred. Downloads retain no hard cumulative duration limit.
Service inspection batch commit: `582f325`.

### T20 fourteenth batch: three-purpose transaction event calculation (2026-10-12)

The service adds a complete pure single-event calculator for preinstall/install/
activate transactions. Original verified request and actual immutable Session/
transaction binding precede receipt evaluation. Replay returns only the original
receipt, without re-running an edge, extending retention or inspecting a damaged
cursor. Original 30-day occurrence and 37-day upload windows remain unchanged.

Fresh live candidates reuse T44's transaction transitions. Purpose terminal state
or original start+30d selects diagnostics evaluated at original occurrence. Known
cursor history takes priority over a missing previous-state field in an older
terminal row; without either source the calculation refuses. Current timeout
reason/cancellation markers are removed only from the ephemeral historical view.
Original terminal/state/start/revision/credential and all protected records remain
unchanged; cursor/sequence growth stays bounded. Returned anchors/update bytes
are independently owned.

The exact original shared facts union is compiled from the pinned API. The local-
state kind relation, live telemetry-only progress branch, lawful other shared
facts and historical transaction branch are preserved. Explicit JSON-null
boundary presence is required for safe termination; missing is not null proof.
Initial consumption self-observation also requires its original kind. No second
caller facts projection or fabricated quality watermark is accepted.

The initial test fixture
primitive-JCS snapshot mistake was repaired; the initial 198 original Node model
cases then passed, including an independent role's own run (5.391s). The cursor
priority finding adds six original positive/negative controls, for 204 cases;
the root's expanded targeted run passed (5.557s package). Default pure tests
cover all three original signed consumption profiles, original revision MAX,
timeout replay, unknown/known historical cursor, parser shape and output ownership.

Final full Go tests with original Node reference opt-ins passed (events package
22.852s), including the final cursor output-alias controls. Vet, module checks,
all command builds, pinned comparison and client architecture/version/diff checks
passed. Database/provider opt-ins stay skipped by default; this pure batch claims
no real-database or physical E2E.
Both independent roles passed the final frozen code/tests/documents without
required findings; final independent pure tests including 204 reference cases
and the last alias controls passed (5.292s). This closes only this calculation.

Outputs are candidates rather than event ACK or database commit permissions.
Stage, activation terminal/cancellation, occupancy and quality contribution still
require real final T44/T25 owner composition with receipt/outbox/T06. No native
SQL migration changes, actual writer registration, public atomic batches or
physical/database E2E are claimed. Full T20 remains open. Deployment stays
deferred; downloads have no hard total/cumulative duration limit.
Service transaction calculation commit: `b159d18`.

### T20 fifteenth batch: protected transaction reception observation (2026-10-12)

The service binds the three-purpose calculator to actual current Root/role deny,
original T44 Session/authorization/transaction and complete scope inventories.
Receipt and independent transaction cursor point reads precede final same-token
original trust/input/SQL-time consistency checks. The returned complete bounded
read set and live/late/replay plan are observations, never event ACK or commit
permission. The source reads no outbox and proves no quality contribution.

Reception migration five adds a distinct transaction cursor table with composite
scope/transaction PK and actual T44 transaction FK. The old Session cursor table,
including its Session FK, stays unchanged. Equal textual IDs cannot collide in
storage or reference kind. Original reception SQL one through four remains byte-
identical; the new migration is LF-only and creates no function or writer grant.
Structure, digest/count, read-only role and protected maintenance/owner guards
include the new warehouse. Explicit initialization verifies old native structure
before adding the table and full current structure afterward.

Runtime remains without direct writes/new EXECUTE. Actual symmetric existing
reserve_owner/apply_owner privileges can coexist with the reader; singleton or
unsafe PUBLIC/named/inherited/SET grants refuse. The request cannot choose a
schema privilege mode. Repeated initialization still revokes the enabled owner
ports and requires explicit enablement. Current null-only no-update outbox policy
is unchanged; a null watermark is not transaction quality completion.

Targeted actual PostgreSQL live and day-31 original late observations passed for
all three purposes (36.54s). The namespace fixture's first assertion expected
eight references after adding another real Session; nine is the correct complete
inventory, and both Session references are now required. Fixed namespace/role
control passed (3.92s). Cursor native identity/revision/shape, named/PUBLIC column
ACL and FK fault/restoration controls passed (4.50s). All three immutable receipt
candidate/digest conflict/current-deny before unavailable existence controls
passed (17.89s). Storage samples are explicitly protected fixtures, not T19
issued authorization/eligibility, accepted T20 transaction events or quality.
Receipt-only samples deliberately contain no outbox/contribution evidence.

The final source additionally requires ordered safe initial/native/final SQL
timestamps; checking only initial/final cannot hide an intermediate regression.
Its targeted missing-dependency/time-pair unit controls passed (1.549s).
Default full Go tests/vet, module verification, all command builds and pinned
comparison passed on that final source guard; client architecture/version/diff
checks passed without changing the application version.

The first complete actual event regression (1163.502s) had one failed protected
retention CLI assertion after the natural shared window; all five new transaction
and additive-upgrade cases passed. Fixed-only command failure categories were
added without copying private subprocess output or relaxing production guards.
With rebuilt commands on the final source guard, the natural boundary and all
five new transaction cases passed (235.652s), including native sealed identity
preservation. The final complete actual event regression passed (1254.003s):
61 applicable named cases passed and only the subprocess parent helper skipped
by design; actual child-crash recovery passed. The natural retention/CLI boundary
passed (154.72s). All five new transaction controls, additive sealed upgrade and
prior owner/recovery/maintenance/native guards passed. This positive complete
run does not prove the first CLI failure's cause; that observation remains
disclosed. Both independent roles gave final scoped PASS; the independent pure
dependency/time-order control passed (1.614s). No mandatory observation finding
remains. Deployment,
physical platform acceptance, transaction writers/public batches/T25 composition
and full T20 remain unfinished. Downloads retain no hard duration cap.
Service protected transaction observation commit: `5f7ffb6`.

### T25 first batch: fixed network weekly calculation (2026-10-12)

A separate `networkprivacy` feature calculates bounded fixed five-dimensional
ISO UTC weekly leaf candidates. It applies scope/week/full-leaf once, prefix/
week/full-leaf/day once and a shared prefix/week maximum seven together only
on actual selection. Rejected prefix candidates do not burn scope allowance;
already counted scopes do not steal new prefix/day buckets. Network scope limits
are per complete leaf, without T24's first-workflow/one-leaf rule. Original point
identity duplicates are idempotent; changed projection conflicts.

The result exposes only week and complete leaf fields. k>=20 exposes exact count;
smaller leaves expose only suppressed without count/approximation/bounds. No
event/scope/prefix/HMAC/key binding, aggregate or cross-plane join key is returned.
Typed internal projections prove no actual authentication, KMS derivation or ASN
provenance; production report ports remain closed. Final full-source ownership,
persistent freeze, privacy permissions/exports, expiry cleanup and three-language
integration remain unfinished. T21/T24 cannot use this calculation as eligibility.

Independent review caught and repaired the invented network purpose label: the
actual normative label is `quality-prefix` (security section 15.1), with literal
positive and invented/other-purpose negative controls. It also found refusal of
the final ISO week containing safe integer MAX; internal full-week bounds are
now preserved and event instants separately limited to MAX. Verification was
strengthened for same-instant eventId ascending competition and the exact shared
weekly cap seven; caps six/eight and a per-leaf seven cannot satisfy the controls.
The expanded seven focused unit groups passed (3.674s). Final full default Go
tests passed (networkprivacy 5.966s), with vet/all command builds/diff; module
verification and pinned comparison passed without dependency/artifact changes.
Both independent roles gave final scoped PASS after the four repairs; their
final seven-group pure tests passed (1.258s and 5.986s). No mandatory calculation
finding remains. External provider/database opt-ins
remain skipped; these pure tests claim no actual event acceptance or E2E.

Distinct original identities may tie on occurrence and eventId. The tie rule is
awaiting the owner's explicit decision; until then the complete ambiguous input
returns no candidate/counts. Arrival/map order or an unapproved third key cannot
choose winners. No T24 week-end+seven-days rule is imported into network freezing.
This batch is pure calculation, not full T25 or physical/provider/database E2E.
Deployment remains deferred; downloads have no hard elapsed-duration cap.
Service fixed network calculation commit: `0ba848f`.

### T25 second batch: in-memory edge prefix normalization (2026-10-12)

The edge adapter normalizes an actual trusted `netip.Addr` peer into IPv4 /24 or
IPv6 /48, discards host bits and canonicalizes compressed lowercase IPv6.
IPv4-mapped IPv6 uses the same IPv4 prefix. Invalid/zoned peers produce no
prefix; local interface names cannot enter the KMS input. It reads no forwarding
headers, writes no raw IP/prefix and makes no network-based update/identity
decision. The protected plaintext object refuses ordinary JSON/text encoding
and redacts conventional formatting; only independently copied explicit Input
bytes may enter the KMS adapter in edge memory. This does not prevent explicit
misuse of those bytes or arbitrary reflection.

Three focused groups passed (1.309s), covering prefix boundaries/mapped families,
host-bit removal, canonical spelling, owned input copies, direct/nested encoding
refusal, ordinary format redaction and invalid/zone/zero-object refusal. Independent
review found a normal fmt recursion leak through unexported wrappers despite
direct String/GoString methods. Private plaintext now uses an indirect payload;
direct/pointer/exported/unexported wrapper/slice/map controls passed on the repair
(0.897s package). Final full default Go tests passed (edge 1.764s), with
vet/all command builds/diff checks. Module verification and pinned comparison
passed without dependency/artifact changes. Both independent roles gave final
scoped PASS after the formatting repair; independent pure three-group tests
passed (1.468s and 5.809s). No mandatory prefix finding remains.

An isolated actual OpenBao 2.7.1 capability probe found that hmac derived=true
was unavailable (HTTP 500); an AES-derived Transit key accepted HMAC and remained
stable for the same context, but changing the ISO week context produced the same
digest. The initial isolation expectation failed (6.39s probe; 10.094s package).
The later purpose-change case was not reached. Keys/tokens/responses/digest bytes
stayed private and no deployed provider was touched. No adapter is registered
on that behavior and no secret-bearing probe is committed. Request context alone
cannot prove a derived week key. Selecting a compliant KMS-side derivation or
changing approved key semantics is an unresolved decision, skipped while this
independent prefix primitive proceeds. The normative HMAC/weekly isolation,
deferred deployment and no-hard-duration download rule remain unchanged.
Service protected edge prefix commit: `f2af566`.

### T27 first batch: bounded client JSON byte codec (2026-10-13)

The updater feature consumes a private standalone Rust byte codec. Fixed metadata
and credential wire limits, strict duplicate-key parsing, depth/string/member
bounds, finite binary64 semantics, UTF-16 key order and bounded RFC 8785 output
match the frozen reference. Canonicalization does not prove canonical incoming
wire, closed schema, signature/Root trust, expiry, high-water acceptance or current
execution rights. No endpoint or startup update behavior is enabled. Application
VERSION remains 0.10.8; the unpublished codec's 0.0.0 is not a product version.

Seven focused groups passed with the actual original Node reference and final
App-matching JSON features (6.85s); independent reruns passed (6.79s and 7.20s).
The seven groups also passed with actual Node 22.23.3 (6.42s), matching CI.
All 20 original envelope hashes/signing-input bytes and 19 actual Node differential
cases passed. Independent review repaired oracle/reference-only CI routing and an
unrelated Cargo-selected dependency edge. App/CLI locks add only three required
packages and their App edge; both locked complete metadata graphs resolve. All
44 CI policy tests, standalone fmt/Clippy, architecture and version checks passed.

Actual Windows App integration passed four updater tests, including both new
facade controls (0.00s, after a 47m08s cold build using the project's existing
compiler stack wrapper). The full App library run completed with 2967 passed,
11 failed and 10 ignored (1659.42s); it did not pass. Five existing source-text
pins failed on CRLF checkout. Three unchanged source files were locally restored
to exact HEAD LF bytes; recompilation passed all five pins (0.01s, 1m22s rebuild)
without logic/assertion changes or a committed diff. Independent roles traced the
six remaining failures to four Unix-rooted path fixtures on Windows, one fixed
slash-separator assertion and one missing bundled Python before the intended
rollback failpoint. No float_roundtrip link was found in their string/boolean/
ordinary-integer/path branches; that rollback path remains unverified by the
failed case. These existing limitations remain disclosed, with no all-App pass or
out-of-scope marketplace repair claimed. Both independent roles gave final B1
scoped PASS; no mandatory byte-codec finding remains.

No physical platform, trusted time, signature/provider or service E2E is claimed.
Full T27 remains unfinished, deployment remains deferred, and downloads retain no
hard total/cumulative duration limit.
Client bounded byte codec commit: `dc6dbdeb2`.

### T27 second batch: fixed signing bytes and descriptor identities (2026-10-13)

The private codec constructs the complete normative signed wrapper from the
original five required properties and the entire signed object, preserves explicit
null, refuses missing fields and rechecks the extra wrapper level/resource bounds.
Direct bounded JCS avoids false refusal from an oversized noncanonical intermediate.
The 1 MiB limit applies to the wrapper payload; fixed domain plus one NUL adds
25 bytes. Canonical 32-byte public-key encoding produces the fixed two-field
descriptor and lowercase SHA-256 keyId. These bytes establish no point validity,
signature validity, authorized key, trusted Root or installation eligibility.

The final repository run passed six new groups and seven existing groups with
actual Node 22.23.3 (5.65s/0.36s), including all 20 original inputs, four descriptor
IDs and 44 actual signing/descriptor oracle cases. The isolated draft also passed
Node 26/22 and independent runs. Initial compile issues were repaired; an initial
oracle failure came from duplicating a public fixture Root keyId and was corrected
without changing production binding behavior. Base64, depth, complete-field and
exact 1 MiB/+1 boundaries are covered. Standalone Clippy, App formatting, locked
App/CLI metadata and 44 CI routing tests passed. Only two existing codec dependency
edges change in the App/CLI locks; versions and JSON features remain unchanged.

Actual Windows App integration passed all five updater tests, including the new
binding/descriptor facade control (0.00s, 4m02s build). Independent actual-repo
reruns passed six new groups (17.25s), seven original groups (15.77s) and 44 CI
policy tests (0.277s); the second role passed six/seven groups (6.74s/0.49s) and
44 policy tests (0.079s). Architecture/version checks passed. Both independent
roles gave final B2 scoped PASS, with no mandatory binding/descriptor finding.
Strict Ed25519 point/
signature policy remains an explicit pending protocol decision; this batch has no
verifier or bootstrap path. Full T27, time/platform/provider certification and
deployment remain unfinished. Downloads retain no hard elapsed-duration cap.
Client signing-byte binding commit: `bf956c8aa`.

### T26 first batch: original deadline and budget arithmetic (2026-10-13)

The private std-only timing crate implements safe whole milliseconds, checked
original deadlines and separate preparation, validation, execution and consume-
recovery budgets. It captures the original preparation session cutoff and only
permits tightening; three execution purposes use actual qualified durations within
their individual bounds. Consume recovery starts at the first transmission and
cannot be refreshed by a status query. Backwards known observations are unproven;
unknown clocks and confirmed faults carry distinct reasons without a guessed
remaining duration. No download budget is introduced.

Eight focused groups, standalone fmt/Clippy, 45 CI policy tests, complete locked
App/CLI metadata, architecture and product-version checks passed. Each integration
lock adds only the local dependency-free package and App edge. Actual Windows App
integration passed all seven updater tests, including two new timing controls
(0.00s after a 31m59s build with the existing compiler stack wrapper). Workflow
YAML parsing and the fast fork guard passed. Both independent roles found no
mandatory source finding and independently passed eight groups and 45 CI policies.
Both gave final B1 scoped PASS after reviewing the frozen App evidence and staged
scope; no mandatory timing-arithmetic finding remains.

The current 0.10.9 product version and CodeWhale gitlink came from the initial
safe main synchronization for this task. This batch does not certify a target,
clock source, cross-power continuity, durable ledger or preparation/execution
budget. Health timing is the next separate batch; full T26, cleanup and gate
integration remain unfinished. Deployment stays deferred. Details:
[client timing arithmetic](client-upgrade-timing-math.md).
Client original-budget arithmetic commit: `4969b08d0`.

### T26 second batch: original health timing windows (2026-10-13)

The timing crate now records the first launch's fixed five-minute total window,
at most two nonoverlapping checks and each check's 120-second limit clipped to
the original total window. Private check handles reject old callbacks before they
can change the watermark. Check and total exhaustion latch separately; waiting
at the bound exhausts, while immediate confirmed completion at the bound can be
timing-eligible only before exhaustion was observed. Unknown/fault/regression
cannot clear a latch or release a check. Independent safe-stop closure produces
no timing-eligible completion, and each finished check has its own saved result.

The isolated draft and actual repository passed nine health groups plus the
repository's eight original budget groups. A pre-review found an untrimmed check
remaining value; the primary role clipped it to the total remainder and added
late-check/SAFE_MAX controls. A subsequent review added the decisive boundary
regression: latch at D, then unknown/fault/backwards observations, then completion
at the same D cannot revive it. Both independent roles passed the final isolated
draft. Actual Windows App integration passed all eight updater tests, including
the new health timing graph control (0.00s after a 2m12s build with the existing
compiler stack wrapper). Actual repository fmt/Clippy, 45 CI policies,
architecture, version consistency and diff checks passed. Both independent roles
passed the 17 groups and gave final B2 scoped PASS after reviewing the frozen App
evidence. No mandatory finding remains. This batch changes no dependency, lock or CI
configuration and does not claim an App-wide pass.

The record proves timing only. The protected owner must bind actual scope,
transaction, launch/check facts, safe stop and eligible new-version restart, and
handle immediate identity/data/launch failures. No health probe, health success,
gate release, protected persistence or cross-power clock certification is claimed.
Full T26 and deployment remain unfinished, and downloads have no duration cap.
Details: [client health timing](client-upgrade-health-timing.md).
Client health-timing arithmetic commit: `dd59630d6`.

### T29 first batch: bound byte-range transfer planning (2026-10-13)

The private std-only transfer crate captures the original structural artifact,
cache binding and request plan. It validates raw strong ETags, known inclusive
byte ranges, exact signed total/span, content lengths and identity encoding.
Unproven continuity or a changed partial cache cannot append. An ignored Range
with 200 always replaces from zero; 206 requires the original resume conditions.
Complete caches can request actual local verification even after association loss.
412/416 plan recovery without declaring completion. Body excess, truncation,
abort and finished state latch, preventing later input from producing a receipt.

Eleven focused groups, standalone fmt/Clippy, App formatting, complete locked
App/CLI metadata, 46 CI policies, architecture, version and diff checks passed.
Each parent lock adds only the new dependency-free local package and App edge.
The initial Clippy-only findings were two collapsible conditions, now repaired.
Actual Windows App integration passed all nine updater tests, including the new
transfer facade control (0.00s after a 2m35s build with the existing compiler stack
wrapper). Both independent roles gave final B1 scoped PASS after reviewing the
frozen source and App evidence; no mandatory finding remains. No App-wide pass is
claimed; earlier unrelated Windows limitations stay documented.

The local header input profile has explicit count/per-value/aggregate resource
bounds; it does not impose a 32 MiB artifact limit, elapsed deadline or retry cap.
Headers and segment receipts establish no source trust, final hash/signature,
platform authenticity, exclusive file ownership or installation rights. Actual
HTTPS/DNS/redirect policy, credentials/URL renewal, protected cache and staging,
provider continuity and physical byte verification remain subsequent work.
Full T29 and deployment remain unfinished. Details:
[client transfer planning](client-upgrade-transfer-planning.md).

### T29 second batch: restricted HTTPS file transfer (2026-10-13)

The new private transport binds each actual HTTPS GET to the original byte plan,
approves canonical origin and every exact-port socket before connecting, handles
literal IPs without resolver substitution and refuses DNS fallback. Fresh clients
disable system proxies, redirects and transparent decoding; no control credential
or cookie injection is accepted. Fixed errors discard SDK URL/source chains.
When Transfer-Encoding is present, only one chunked coding is permitted and
Content-Length must be absent. A validated
plan is visible before byte delivery so 200 replacement cannot be mistaken for
206 append. Exact EOF supplies only a byte receipt; failure/abort/finish latch.
Output fragments are bounded to 64 KiB, separately from SDK parser allocation.

Final real HTTPS tests passed all nine groups (2.38s) and 34 wire requests, including
actual raw duplicate CL/CR/CE and TE conflict controls, CA/name and wrong pin,
short ranges, partial recovery, stream excess/truncation/abort and continuous
progress exceeding an idle interval. Proxy/redirect/denied endpoint hits were zero;
GET identity and Range/If-Range were observed with no control/cookie/proxy credentials.
Standalone fmt/Clippy and all eleven B1 groups, 47 CI policies, architecture,
version, diff, YAML and complete locked App/CLI graphs passed. Each parent lock
adds only the local package/App edge with no external version change. Final actual
Windows App integration passed all ten updater tests (0.01s after a 1m38s incremental
build); no App-wide pass is claimed. Two independent roles each passed an earlier
nine-group/30-wire version, then separately reviewed the final plan getter and raw
duplicate-header additions, without conflating earlier execution with the final tree.

Qualification, URL renewal, current rights, immutable provider evidence, protected
file writes/cache/staging and final physical hashing/authenticity remain subsequent
work. SDK parser bounds are not claimed to equal post-parsed B1 bounds. There is
no total/cumulative download deadline or retry cap. Full T29 and deployment remain
unfinished. Details: [restricted HTTPS transport](client-upgrade-file-http.md).
Both independent roles gave final B2 scoped PASS after the final App/B1 evidence
and documentation review; no mandatory finding remains.

### T29 third batch: complete observed file-byte comparison (2026-10-13)

The private scanner seeks the same already open File handle to zero, reads through
actual EOF with a fixed 64 KiB buffer and compares the complete length/SHA-256 to
the original structural Artifact. Excess fails before hashing that chunk; EOF and
hash are required after exact count. Cancellation is checked before seek/reads,
every Interrupted retry and after EOF. IO failures retain no underlying path/source.
ByteMatch's private constructor proves only the observed byte string matched;
it supplies no durable cache trust, physical object protection or execution right.

Final eight groups passed (1.89s), including real 33 MiB/buffer-boundary files,
nonzero initial cursor, pre-scan truncate/append, same-length first/middle/tail
pollution, actual write-only read failure and three cancellation positions.
Crate-private Seek/Read fault controls are separately identified as unit tests.
Five independent Python hash constants matched. Standalone fmt/Clippy, 48 CI
policies, architecture, version, diff, YAML and both complete locked App/CLI
graphs passed. Final actual Windows App integration passed all eleven updater
tests (0.01s after a 3m33s build). No App-wide pass is claimed. Each parent lock
adds nine lines for the local package and App edge without external version changes.
Self-review corrected an isolated lock seeded from an earlier codec graph;
all 12 final isolated packages match App versions/sources/checksums and eight
groups/Clippy passed again after correction.

The owner must connect the original T26 validation budget and unknown/fault
safe-stop rules to cancellation; a running OS seek/read cannot be interrupted
by this predicate. Qualification, protected cache/staging, safe unpacking,
installer/platform authenticity and current execution rights remain subsequent
work. Full T29 and deployment remain unfinished, with no download elapsed cap.
Details: [complete byte check](client-upgrade-complete-byte-check.md).
Both independent roles gave final B3 scoped PASS after reviewing the final
App/locked-graph evidence and documentation. No mandatory finding remains.
Their independent eight-group runs preceded the lock correction; the final
lock was independently checked and the primary role reran its tests/Clippy.

### T29 fourth batch: copy and rescan the actual destination (2026-10-13)

The verify crate now copies current source bytes from the original open handle
into an independently protected fresh destination. It rejects nonregular handles
or a nonempty destination without truncation/deletion. B3's bounded scanner checks
actual source count before each chunk write and requires full source EOF/hash.
Partial/EINTR/zero-write and cancellation paths cannot produce success. After
sync_all, B3 rescans that same destination handle from zero through actual EOF
and compares the whole size/hash again. Old ByteMatch facts are not copy inputs.
Failure keeps untrusted bytes for owner isolation/cleanup; a new match remains
only observed byte evidence, without durable protection or execution rights.

Seven copy groups plus eight B3 groups passed (3.61s). Real 65,537-byte/33 MiB
copy/rescan, cursor reset, nonempty/alias preservation, source changes before copy,
read-only/write-only errors and all copy/sync/EOF cancellation boundaries were
tested. Private partial-write/EINTR/zero-write and sync-failure/pre-rescan pollution
controls are identified as unit evidence. Standalone fmt/Clippy, 48 CI policies,
architecture, version and diff checks passed. Actual Windows App integration
passed all eleven updater tests (0.01s after a 1m37s build), including actual copy
through the App facade; no App-wide pass is claimed. This batch changes no
dependency, lock, workflow, product version or submodule.

Original T26 validation timing spans copy/sync/rescan without a new origin.
OS operations cannot be interrupted mid-call by the predicate; the owner must
confirm safe stop before releasing physical protection. Empty length and regular
metadata do not establish ownership. Already scanned positions changing later
are not universally detected. Physical cache/staging protection, safe unpacking,
installer authenticity and current rights remain subsequent work. Full T29 and
deployment remain unfinished, and downloads retain no elapsed cap. Details:
[copy and rescan](client-upgrade-byte-copy.md).
Both independent roles gave final B4 scoped PASS after reviewing the frozen
implementation, final App evidence and documentation. Each independently passed
all fifteen groups (3.47s and 3.52s), plus fmt/Clippy. No mandatory finding remains.

### T29 fifth batch: read-only ZIP32 byte inventory (2026-10-13)

The private inspector validates the original package bytes on the same File before
and after scanning raw central/local records and actual expanded members. Unique
EOCD/physical layout, header/descriptor bindings, portable paths/types, explicit
metadata/path-state/expansion/count/ratio capacities, actual CRC/size/hash and exact
Deflate input consumption are checked. Raw reads and bounded 64 KiB codec steps
check cancellation. No partial inventory escapes a later failure. Internal ZIPs
remain opaque data; the module never writes or promotes extraction paths.

The Manifest has no complete member-name allowlist, so none is invented. Safe
extra members are covered by the complete signed-container identity; native claim
membership, OTA interpretation and FullPack selection remain later owner duties.
ZIP64/split/prefix and unsupported extra/encoding/type formats fail explicitly.
The 4,096-entry and separate 8 MiB metadata/path-state parser ceilings do not cap
artifact size at 32 MiB or certify production resource budgets.

Final three groups covering 48 independent Python wire vectors passed (7.28s),
including real 33 MiB Store and Deflate archives, exact compressed tail refusal,
late corrupt entry and Store/Deflate cancellation boundaries. Python checked all
outer hashes and every positive inventory with its standard ZIP SDK. Fmt/Clippy,
49 CI policies, architecture, version, diff/YAML and complete locked App/CLI graphs
passed. Each parent lock adds twelve lines for the local package/App edge without
external version change; all 30 isolated packages match parent versions/sources/
checksums. Final actual Windows App integration passed all twelve updater tests
(0.02s after a 2m10s incremental build), including Store/Deflate success and
CompressedTail refusal; no App-wide pass is claimed.

Independent review found that initial standalone miniz checks did not exercise
the App's existing zlib-rs decoder. The primary role aligned that backend, repaired
the verification gap and added actual App Deflate controls. Both independent
roles passed the repaired backend's three groups (8.10s and 12.40s); earlier miniz
runs are not treated as App backend evidence. Baseline dependency graphs for
Windows, Linux x86_64/arm64 and macOS arm64 already selected zlib-rs; graph checks
are not native certification. Physical source protection, safe extraction,
installer authenticity, current rights and full T29 remain unfinished. The same
original validation budget spans all scans, without a new download duration cap.
Details: [ZIP inventory](client-upgrade-zip-inventory.md).
Both independent roles gave final B5 scoped PASS after reviewing the repaired
backend, actual App evidence and final documentation. No mandatory finding remains.

### T29 sixth batch: materialize and rescan an observed ZIP member (2026-10-13)

The owner supplies an independently authenticated name/size/hash expectation and
already open protected source/destination handles. The fresh regular destination
must be empty. Whole-package inspection precedes writes; current raw records and
selected member are read again, with actual size checked before output writes and
full member EOF/CRC/SHA/compressed-span checks. The original whole package is
hashed again after copying. After sync_all, the same destination is read from zero
to ordinary real EOF for size/CRC/SHA comparison. ZIP offsets or compressed lengths
never truncate that final scan. Empty members use their actual empty-file identity,
without a fabricated positive-size Artifact. MaterializedMember is only observed
byte evidence; it confers no protection, native authenticity or execution rights.

Final ten groups (seven new plus three inventory groups) passed (8.75s), including
actual Store/Deflate/descriptors, 33 MiB output, empty files, zero-write mismatch and
nonempty/alias refusals, real handle permissions, every observed cancellation poll,
late corruption and visible source changes after inspection. Private writer/sync
faults are separately identified as unit controls. Python checked all 49 container
hashes and positive inventories. Fmt/Clippy, 49 CI policies, architecture, version
and diff checks passed. Actual Windows App integration passed all twelve updater
tests (0.04s after a 2m17s build), including real Store/Deflate/empty-file copying;
no App-wide pass is claimed. No dependency, lock, CI, version or gitlink changed.

Both independent roles passed the frozen ten groups (12.83s and 9.35s), Python
49-vector checks, fmt/Clippy and diff checks; one additionally reran all 49 CI
policies. Failure leaves untrusted destination bytes for owner isolation/cleanup.
The original T26 validation budget covers every scan/copy/sync; cancellation
cannot interrupt a running OS operation. Physical protection, path creation,
native claim selection/authenticity and current rights remain subsequent work.
Full T29 and deployment remain unfinished, with no download duration or retry cap.
Details: [member materialization](client-upgrade-member-materialization.md).
Both independent roles gave final B6 scoped PASS after reviewing the frozen
implementation, actual App result and final evidence documents. No mandatory
finding remains.

### T29 seventh batch: actual cache segment writes (2026-10-13)

The private verify crate now exposes a writer borrowing the actual cache File and
original validated response plan. Separate explicit owner permission authorizes
200 replacement; permission/type/cancellation checks precede set_len(0). A 206
plan only appends at its original offset when the actual current length matches.
Every write checks the 64 KiB per-call bound and response count before output,
handles partial/Interrupted/zero writes, and latches every failure independently.
Finish consumes the writer only after actual network EOF, binds the supplied
transport receipt to the same target/end, checks exact count and actual length,
syncs, and rechecks length/cancellation. A short segment's observed next offset
is not the complete artifact size or a whole-file hash match.

Final 26 groups (eleven new plus fifteen existing) passed (3.85s); Clippy passed
(2.03s). Actual file evidence covers replacement authorization and preservation,
short range continuation, both offset mismatches, read-only/append-mode handles,
foreign receipts, visible length changes, all observed cancellation polls and a
real 33 MiB write followed by the independent B3 scan. Private writer/sync hooks
are identified separately. Standalone/App fmt, 49 CI policies (0.048s), architecture,
version and diff checks passed. Actual Windows App integration passed all thirteen
updater tests (0.03s after a 3m02s build), including the new actual cache writer;
no App-wide pass is claimed. No dependency, lock, CI, version or gitlink changed.

The transport receipts in these tests are structural fixtures, so this batch does
not claim real HTTPS-to-cache E2E. The owner supplies current physical protection,
prefix continuity and actual EOF, preserves untrusted bytes on failure and later
runs complete protected validation under the original T26 budget. Blocking IO
cannot be interrupted mid-call by cancellation. Qualification, URL renewal,
protected cache/staging, native authenticity and full T29 remain unfinished.
Downloads retain no total/cumulative deadline or retry cap. Details:
[cache segment writes](client-upgrade-cache-segment-writes.md).
Both independent roles passed the frozen 26 groups (4.56s and 4.40s), fmt and
Clippy; one additionally reran all 49 CI policies (0.045s).
Both independent roles gave final B7 scoped PASS after reviewing the frozen
implementation, actual App result and final documents. No mandatory finding remains.

### T29 eighth batch: actual HTTPS-to-cache composition (2026-10-13)

The receiver consumes the restricted actual FileBody and derives its sole plan
from it. It feeds real Data chunks to the actual-file writer and completes only
from the transport's actual SegmentFinished EOF receipt. Every network/file/
permission/cancellation failure drops that body and returns no receipt, preserving
untrusted cache bytes. It runs on an owner's independent blocking worker with a
live actively driven runtime; current-thread IO requires the owner to keep
Runtime::block_on running, as the actual spawn_blocking E2E demonstrates.
A pinned next_chunk future survives false cancellation polls while a 100 ms timer
allows cancellation during pending reads. This is no total/idle download deadline;
active OS calls or future polls cannot be interrupted mid-call.

Final isolated real HTTPS verification passed all thirteen groups (5.60s) and
47 wire requests, including four new actual-file groups. These cover 200 explicit
replacement/refusal, short 206 followed by continuation and B3 verification,
truncation/excess/disconnect, real read-only output and wrong prefix length,
pending-read cancellation, ongoing progress and bounded writes of 131,073 bytes.
The synthetic large-response digest intentionally mismatches, so its successful
count is followed by B3 HashMismatch rather than trusted cache evidence.
Self-review and independent review found that a 500 ms stall plus a two-second
test allowance could mask await cancellation through a later write poll. The
primary role replaced it with a peer that sends headers but never Data/EOF,
recorded the first true poll after all writer-start polls and added the same
exchange's always-false control ending only at the fixture's read-idle limit.
The repaired proof passes; test allowances never impose product download limits.

Actual Windows App integration passed all thirteen updater tests (0.05s after a
4m05s build). It compiles the new receiver facade and retains existing integration
controls; the real HTTPS-to-file execution is the isolated fixture, not a full App
workflow or App-wide pass. Standalone/App fmt, 49 CI policies (0.068s), architecture,
version and diff checks passed. Parent App/CLI locks each add only one local HTTP
to verify edge, with no external version change. All 178 isolated HTTP packages
match the parent versions/sources/checksums. No product version or gitlink changed.

Physical cache protection, immutable provider evidence, qualification/URL renewal,
protected staging, native authenticity/current rights and full T29 remain open.
The owner later performs complete protected validation under the original T26
budget and confirms safe worker stop before releasing protection. There is no
total/cumulative download deadline or retry cap. Details:
[HTTPS-to-cache receive](client-upgrade-http-cache-receive.md).
Both independent roles gave final B8 scoped PASS after the repaired cancellation
test and final evidence documents. Each independently passed all thirteen actual
HTTPS groups (5.64s and 5.72s), 47 wire assertions, fmt/Clippy and diff checks; one
additionally reran 49 CI policies (0.053s). No mandatory finding remains.

### T29 ninth batch: one restricted GET/cache attempt (2026-10-13)

The immutable request combines the original RequestPlan, URL, current owner policy
and resolver with mandatory separate replacement approval. A legal original 206
appends through B8; a legal 200 requires Allow before touching old cache bytes.
The same actual fetch future remains pinned over cancellation polls during DNS/
connection/header waits, preserving the original operation timers. A ready result
observes cancellation again before a write or projection. Restart for 412/416 and
VerifyExisting leave file bytes/length/cursor unchanged. A complete cache without
association requests later local B3 verification without requiring URL/DNS, rather
than pretending that planned length already proves the current file. No retry,
qualification refresh, URL renewal, new session or deadline reset is performed.

Final real HTTPS regression passed sixteen groups (6.73s) and 55 wire assertions;
Clippy passed (4.19s). Three new groups cover replacement approval/refusal, short
range output, 412/416 and mismatched validator preservation, complete-cache local
verification and immediate-result cancellation, DNS cancellation after an actual
resolver call marker with one call and positive completed-resolver control, and
header cancellation against a peer sending no headers/body. The independent wire
verifier requires that exact header-wait request to reach the peer, preventing a
pre-DNS/TLS cancellation from masquerading as the pending-header case. Existing
B8 no-data/EOF cancellation and false-cancel controls passed through the shared
pinned-future helper. Actual Windows App integration passed all thirteen updater
tests (0.03s after a 2m45s build). It compiles the new facade; actual HTTPS-to-file
execution is the isolated fixture, not a full App workflow or App-wide pass.

Standalone/App fmt, 49 CI policies (0.060s), architecture, version and diff checks
passed. This batch changes no dependency, lock, CI, product version or gitlink.
Private fixture profiles remain crate-internal; no production CA bypass is exposed.
Current qualification, immutable-source evidence, protected cache/staging, URL
renewal/retry coordination, native authenticity/rights and full T29 remain open.
The owner preserves protection and original T26 validation timing and confirms
safe worker stop. No total/cumulative download duration or retry cap is imposed.
Details: [single cache attempt](client-upgrade-single-cache-attempt.md).
Both independent roles gave final B9 scoped PASS after final App evidence and
documentation review. Each independently passed sixteen actual HTTPS groups
(6.25s and 6.44s), 55 wire assertions, fmt/Clippy and diff checks; one additionally
reran 49 CI policies (0.066s). No mandatory finding remains.

### T29 tenth batch: transient response/write/wait observations (2026-10-13)

The owner can observe actual validated response shape (original target/mode/span
and optional opaque strong tag), successful whole-write counts and pinned-future
Fetch/Body waiting. Private constructors, temporary borrowed fields and no automatic
Debug/serialization prevent these facts from becoming public trust wrappers. Tags
do not establish immutable-source continuity or retained prefix identity. Checked
Progress counts only completed actual writes; a failed partial write may leave
bytes without Progress. Waiting comes only from the timer branch and does not
infer DNS phase, trustworthy time, contact loss, quality or signed event authority.

Stop returns fixed OwnerStopped, consumes the actual body/future and yields no
success receipt. Response Stop precedes writer start and preserves old bytes/
length/cursor; Progress/Waiting Stop can leave untrusted output. Continue cannot
authorize replacement or infer EOF from count. Noop wrappers preserve the existing
entry points and original cancellation-poll sequence without new filesystem
preconditions. Callbacks must promptly return, cannot reenter or nest block_on,
and Waiting may run inside a future poll. No timer interrupts an active callback,
OS operation or future poll; no total/cumulative deadline or retry cap is added.

Final isolated actual HTTPS regression passed twenty-two groups (10.16s) and
65 wire assertions; six new groups exercise Response preservation, complete-count
Progress Stop without a receipt, original strong tag and two actual appended
bytes before disconnect, short-range progress, Fetch/Body/DNS waiting Stop,
Continue with denied replacement and existing read-idle timeout, continuing
progress and VerifyExisting with no synthetic notices/DNS. A private write hook
writes one real byte then injects IO failure, testing absence of completed-block
Progress; this is unit injection, not physical disk-failure certification.
Clippy passed (18.87s). Standalone/App fmt, 49 CI policies (0.063s), architecture,
version and diff checks passed. Actual Windows App integration passed all thirteen
updater tests (0.03s after a 3m37s build). It compiles the observer facade and keeps
existing controls; actual HTTPS execution is the isolated fixture, not a full App
workflow or App-wide pass. No dependency, lock, CI, version or gitlink changed.

Physical cache/staging protection, persisted resume state, current qualification,
URL renewal, signed progress ingestion, native authenticity/rights and full T29
remain open. The owner later validates complete bytes under original T26 timing
and confirms safe stop before releasing protection. Downloads retain no hard
elapsed or retry cap. Details: [cache observations](client-upgrade-cache-observations.md).
Both independent roles gave final B10 scoped PASS after final App evidence and
documentation review. Each independently passed twenty-two actual HTTPS groups
(10.57s and 10.16s), 65 wire assertions, fmt/Clippy and diff checks; one additionally
reran 49 CI policies (0.055s). No mandatory finding remains.

### T27 third batch: closed stable source-fact projection (2026-10-13)

The byte codec derives SHA256(JCS) of exactly the nine frozen stable fields from
an already bounded duplicate-free Document. Any outer extra is excluded, while
native identities, installation and data migration remain closed required shapes.
ASCII patterns, all frozen enums and positive binary64 safe sizes are checked
without inventing role/format/target cross rules. Decimal-triple versions retain
original bytes and allow long components without integer conversion. Only this
fixed validated projection may use the internal Document constructor and existing
bounded JCS writer. SourceProfile is supplied-fact shape/digest evidence only,
without a native probe, current Registry match, signature or installation right.

Final eighteen groups passed under Node 22.23.3: existing signing six (13.47s),
byte seven (1.38s), and five new source groups (2.55s). Both existing Node oracle
switches were configured and actually executed; the new source oracle additionally
compared 218 acceptance/canonical-byte/hash cases with the original Node/Ajv
reference. These cover every variable stable/native field, outer dynamic/self
exclusion, closed nested/missing/null/type refusal, all frozen target/format enums,
line endings/identifier/hash/string/version boundaries, safe-number rounding and
original input limits/escaped duplicate keys even in excluded facts. Self-review
corrected a Uint8Array-to-Buffer test encoding error and replaced shell-sensitive
non-ASCII literals with Rust Unicode escapes before final verification. Production
hash behavior and frozen vectors did not change to accommodate that test adapter.
Clippy passed (14.99s); standalone/App fmt, 49 CI policies (0.130s), architecture,
version and diff checks passed. Existing CI installs the frozen Node dependencies
before the isolated codec gate. No dependency, lock, Schema, CI, version or gitlink
changed.

Actual Windows App integration passed all fourteen updater tests (0.04s after a
3m56s build), including the new actual source-projection facade control: frozen
ID match, outer ownerEpoch exclusion and nested helper epoch refusal. No App-wide
pass or actual platform source probe is claimed. Native facts/protection, Registry
uniqueness/current eligibility, signature policy and metadata high-watermarks
remain subsequent work; full T27 stays unfinished. The unresolved Ed25519 point
policy is not bypassed by a new verifier. Downloads remain outside total/cumulative
duration and retry caps. Details: [source projection](client-upgrade-source-projection.md).
Both independent roles gave final B3 scoped PASS after final App evidence and
documentation review. Each independently ran Node 22.23.3 with both existing
oracle switches enabled: eighteen groups passed, including 218 actual source
Node/Ajv comparisons. Independent source groups took 1.04s and 0.90s; fmt/Clippy
and diff checks passed, and one role additionally reran 49 CI policies (0.129s).
No mandatory finding remains.

### T27 fourth batch: closed consume-status and raw-secret hash bindings (2026-10-13)

The status method validates the complete nine-field root object with no exclusions
or defaults: six identifiers, two lower-hex SHA256 fields and exactly the three
preinstall/install/activate purposes. Existing bounded JCS serializes the whole
binding; fixed pinvou-consume-status-v1 plus one actual NUL prefixes it before
SHA256. The raw-secret function borrows exactly 32 bytes and hashes those bytes
without a domain/base64/wrapper. Private results store no raw secret and have no
Debug/Serialize. Neither computation attests entropy, possession, a persisted
server record, consume result, signature, time or installation rights.

Private common Schema predicates were moved from source to a shared shape module
because status reuses them; source projection keeps its original root-extra-ignore
behavior and independently passes all 218 reference cases. Status root extras,
including transport/self fields, are strictly refused. Final Node 22.23.3 tests
passed twenty-three groups: existing signing six (5.46s), byte seven (0.44s), source
five (1.04s/218 cases) and five new status groups (5.87s/144 original Node/Ajv
acceptance/preimage/hash/secret cases). Both existing oracle switches were enabled
and executed. New controls match frozen input hex/hash, distinguish NUL from
escaped text, bind every field, allow all three purposes, refuse extras/missing/
null/types/bad shapes/escaped duplicates, preserve reordered/pretty JSON and check
secret lengths plus base64-text refusal. A zero 32-byte control explicitly proves
no entropy is asserted. Clippy passed (12.17s); standalone/App fmt, 49 CI policies
(0.042s), architecture, version and diff checks passed. No dependency, lock, Schema,
CI, product version or gitlink changed.

Actual Windows App integration passed all fifteen updater tests (0.03s after a
2m50s build), including frozen status/secret vectors and a transport-extra refusal
through the new facade. No App-wide pass is claimed. The module sends no query,
stores/generates no secret, creates no consumption and grants no rights. Secret
entropy/protection/original association, actual query recovery, current trust and
full T27/T32 remain later owner work. The unresolved Ed25519 point policy is not
bypassed. Downloads have no total/cumulative duration or retry cap. Details:
[consume-status binding](client-upgrade-status-binding.md).

Two independent reviewers returned final scoped PASS after source and final
evidence review. Their actual Node 22.23.3 runs passed all 23 groups, including
218 source and 144 status/secret cases: dependency review signing/byte/source/
status 3.90s/0.38s/1.00s/0.80s; compatibility review 3.29s/0.35s/1.69s/0.97s.
Independent fmt, Clippy and diff checks passed; compatibility also passed all
49 CI policies (0.079s). Final documentation/App evidence was reviewed without
rebuilding. Root self-review found no remaining in-scope gap.

## T27 fifth batch - rollout commitment math (2026-10-13)

This continued batch accepts bounded duplicate-free JSON null or one exact
seven-field opening. Only null constructs the empty array; a closed valid object
constructs a singleton array. Caller arrays, missing/null fields and extras fail.
Original four targets, two ASCII identifiers, two positive binary64 safe integer
revisions and two lower-hex hashes are validated. Fixed pinvou-rollout-set-v1 plus
one actual NUL precedes bounded JCS of the fixed array; SHA256 computes the hash.
The result has private construction and no Debug/Serialize. It proves no salt
entropy/freshness, current membership, signature, qualification or rights. Shared
private target/revision predicates moved equivalently from source-profile code.

Root actual Node 22.23.3 run passed 27 groups: signing six (8.62s), byte seven
(6.39s), rollout four (0.72s/132 original Node/Ajv acceptance/preimage/hash cases),
source five (1.04s/218) and status five (0.94s/144). Both earlier oracle switches
were enabled and executed. Controls include both frozen hashes/exact array bytes,
pretty/reordered input, all seven field bindings, null/array/object distinction,
escaped duplicates, all original target enums, closed fields/type/null rules,
identifier/hash boundaries and line endings, safe integer limits/fractions and
binary64 rounding. All-zero salt is shape-valid, making no entropy assertion.
Clippy passed (5.33s); standalone/App formatting, 49 CI policies (0.321s),
architecture, version and diff checks passed. No dependency, lock, Schema, CI,
product version or gitlink changed. Actual Windows App passed all sixteen updater
tests (0.15s after a 5m32s build), including both frozen commitments and caller-array
refusal through the actual facade. No App-wide pass is claimed.

Two independent source reviewers returned scoped PASS. Compatibility's actual
Node22 run passed signing/byte/rollout/source/status in 18.16s/0.66s/1.05s/1.18s/
0.85s, Clippy 3.39s and 49 CI policies 0.085s. Dependency's actual Node22 run passed
all 27 groups, including rollout 132 cases in 1.51s and source/status regressions.
Both independently passed fmt, Clippy and diff checks. Both reviewers returned final scoped PASS after reviewing the frozen source and
final App/document evidence without rebuilding. Root self-review found no remaining
in-scope gap; no App-wide or full T27 claim follows.

Details: [rollout commitment](client-upgrade-rollout-commitment.md). Full T27,
actual authenticated opening/membership and high-watermark integration remain
unfinished. The unresolved Ed25519 point policy is not bypassed. Downloads have
no total/cumulative duration or retry cap.

## T27 sixth batch - semantic consume-request digest (2026-10-13)

This continued batch derives SHA256 of plain semantic JCS, with no domain/NUL/
wrapper. The complete original Document is bounded and duplicate-checked first,
including fields later excluded. Only four exact top-level fields are excluded:
consumeRequestDigest, recoverySecret, requestId, headers. Every other field is
retained and validated against closed17 consumeRequest and the original closed
nested preparation/backup/staged/plans/preinstallSlot rules. Nullable required
fields are never defaulted; installation plan is mandatory non-null; five other
required plans may be null. Finite binary64 const1, positive epochs/revisions and
nonnegative staged times follow original Schema without added temporal/purpose
relationships. Private results contain only semantic JCS/hash without Debug/
Serialize; original Document/caller input may still contain excluded secret. No
secure memory, automatic erasure, actual consumption or rights is attested.

Root actual Node22.23.3 passed all 31 groups: signing6 6.41s, byte7 0.37s, consume4
1.98s/1093 original Node/Ajv acceptance/JCS/hash cases, rollout4 0.81s/132, source5
2.52s/218, status5 0.83s/144. Both old oracle switches were enabled and executed.
Controls cover the frozen digest/plain JCS, pretty/reordered objects, every
variable semantic leaf, nullable branches, all three purposes, zero/descending
staged times, four root exclusions with arbitrary values, missing/null/types/
extras at every nested object, numeric boundaries/rounding, identifiers/hashes/
line endings and escaped duplicates. Excluded fields still reject original
duplicate/input/depth/string/member excess. Clippy passed 5.60s; standalone/App
formatting, 49 CI policies (0.050s), architecture, version and diff passed. No
dependency, lock, Schema, CI, product version or gitlink changed.

Two independent source reviews returned scoped PASS. Dependency independently
passed all31 actual Node22 groups including1093 consume cases in7.86s and fmt/
Clippy/diff. Compatibility passed sign/byte/consume/rollout/source/status in
6.24s/8.55s/2.70s/6.25s/1.26s/0.90s, Clippy1.61s, CI49 0.093s and fmt/diff.
Actual Windows App passed all17 updater tests (0.09s after3m22s build), including
frozen digest, exact excluded root fields and refusal of a missing required nullable
field through the actual facade. No App-wide pass is claimed. Both reviewers returned final scoped PASS after frozen-source and final App/
document evidence review, without rebuilding. Root self-review found no remaining
in-scope gap.

Details: [semantic consume digest](client-upgrade-consume-digest.md). Full T27/T32,
actual secret custody, ConsumeAttempt, current trust and backup/slot authenticity
remain later work. Unresolved Ed25519 policy is not bypassed. Downloads retain no
total/cumulative duration or retry cap.

## T27 seventh batch - native identity shape/comparison (2026-10-13)

This continued batch validates bounded duplicate-free native JSON as a closed
five-field identity: positive safe binary64 size, lowercase hex64 sha256, original
ASCII identity/publisher and all five formats. The immutable privately constructed
NativeIdentityShape retains bounded JCS without Debug/Serialize. Both operands
must pass the complete Schema shape before whole canonical bytes are compared.
All fields participate, including publisher/identity/format with identical size/
hash. There is no role/platform cross-rule, domain or digest. The private native
predicate moved equivalently from Source to shared shape, preserving its behavior.
Supplied actual facts do not attest OS identity, signature, protected object,
Registry match, VerifiedExecutionObject or rights.

Root actual Node22.23.3 run passed34 groups: signing6 7.31s, consume4 2.01s/1093
original cases, native3 1.15s/324 original Node/Ajv operand pairs, rollout4 1.27s/132,
source5 1.49s/218, status5 1.11s/144. The collected first byte-suite output was not
visible, so only that unchanged suite was rerun for concrete evidence: byte7
passed0.68s with its actual original Node oracle enabled. Both old oracle switches
were enabled throughout. Native controls check exact JCS, reordered/pretty input,
equivalent numeric spellings, every field mismatch in both directions, self-equality
and invalid either operand. The original reference validates both operands and
compares actual complete canonical bytes; all original formats, missing/null/types/
extras, identifier/hash endings, escaped duplicates, safe limits and rounding are
covered. Clippy passed29.52s; standalone/App formatting, CI49 (0.064s), architecture,
version and diff passed. No dependency, lock, Schema, CI, version or gitlink changed.
Actual Windows App passed all18 updater tests (0.17s after3m36s build), including
same native identity, publisher mismatch with identical size/hash and root-extra
refusal through the actual facade. No App-wide pass is claimed.

Both independent source reviews returned scoped PASS. Dependency passed all34
actual Node22 groups, including324 native operand pairs in1.19s and source regression;
fmt/Clippy/diff passed. Compatibility passed sign/byte/consume/native/rollout/source/
status in6.15s/0.69s/2.35s/4.50s/2.31s/1.69s/7.05s; Clippy0.22s, CI49 0.062s and
fmt/diff passed. Both reviewers returned final scoped PASS after reviewing the
frozen source and final App/document evidence without rebuilding. Root self-review
found no remaining in-scope gap.

Details: [native identity shape](client-upgrade-native-identity-shape.md). Full
T27/T29 and actual native probes/authoritative identity checks remain unfinished.
Unresolved Ed25519 policy is not bypassed. Downloads retain no total/cumulative
duration or retry cap.

## T29 eleventh batch - single attempt plus whole-file scan (2026-10-13)

This continued task reuses the warm worktree on a scoped branch from the reviewed
T27 commit; it does not resync main merely for the next continued task batch. The
actual B9/B10 GET/cache attempt now composes with B3's same-File whole-byte scan.
Partial Segment/Restart starts no validation and returns no match. Only original
complete count or local VerifyExisting enters a fresh real-EOF/size/SHA scan on the
same supplied File. Original target comes only from the actual attempt outcome.
There is no path reopen, alternate target, retry, URL renewal or promotion.

Mandatory CacheValidationOwner begins/resumes the original workflow/transaction/
target record and polls under its original T26 budget. It supplies no actual clock/
protected storage proof; its trusted integration contract must preserve original
deadlines and stop Expired/Unproven/TimeFault/OwnerStopped. Begin refusal precedes
seek/read, scan stops latch the first reason, and final owner/cancel checks gate
returning B3 ByteMatch. Validation is separate from downloading. ByteMatch attests
observed bytes only, not native identity/protection/current trust/VMO/rights.

Both independent reviewers found a final owner callback cancellation gap: owner
poll could return Continue while publishing cancellation. Root added cancellation
recheck after that callback and expanded related scan controls to all five owner
poll positions, including no seek when the first callback cancels. Each scan owner
Continue also rechecks cancellation before proceeding. Fixed stop reasons remain
prioritized and never revive. Initial/transition 27-test/79-request passes are not
used as final-tree evidence. A latest-tree link attempt hit Windows LNK1104 while
an earlier independent test executable was active; participants then stopped
overlapping runs and final root verification ran serially. No process was killed
or unrelated output removed. Final frozen-tree root harness passed all27 actual
HTTPS tests (9.68s) and79 wire assertions; Clippy2.45s/fmt/diff passed. Five new
groups exercise actual200/206 complete scans, short/restart no-validation paths,
wrong original hash/equal-length pollution, network/truncation/observer Stop and
complete local cache with actual size/hash mismatch and zero DNS. Scripted begin
and each of five poll stops/cancellations preserve the original deadline; before-
seek callback cancellation preserves cursor7. This is no physical clock evidence.

Dependency reviewer independently compiled/ran this final1..5 frozen tree: all27
actual HTTPS tests passed11.03s with79 wire assertions; Clippy6.87s/fmt passed.
The original final-callback finding and related scan callback cases are closed.
Compatibility's first corrected4/5 tree independently passed27/79 in9.70s,
Clippy3.87s/CI49 0.064s/fmt/diff, then reviewed latest1..5 delta separately. Its
latest targeted actual-file controls and final doc review are recorded below.

Actual final Windows App passed all19 updater tests (0.04s after1m39s build),
including local complete projection -> actual File match and actual truncation
refusal through the new facade. The earlier pre-fix App pass is not substituted
for this final-tree build. No App-wide pass is claimed. Real HTTPS execution is
provided by the isolated actual-peer harness, not by this App-local facade test.

Existing checks passed CI49 0.056s, architecture, version and formatting/diff.
No dependency, lock, Schema, CI, product version or gitlink changes. Full T29 and
protected cache/staging/native authenticity/current qualification/URL renewal,
durable resume and actual original-budget/clock integration remain unfinished.
Downloads retain no total/cumulative duration or retry cap. Details:
[single attempt plus complete scan](client-upgrade-checked-cache-attempt.md).

Compatibility independently ran the exact latest actual-file local test:1 PASS
0.04s,26 filtered, with zero HTTPS/proxy/redirect requests. It covers all1..5
callback cancellations and first-callback cursor preservation. Its first target
run also passed0.04s, but the temporary read-only adapter incorrectly rejected
Go's zero Requests slice JSON null instead of allowing null/[]; the adapter was
corrected and rerun exit0. This was a verification-script issue, not a production
failure, and no repository file changed. Both reviewers returned latest-source
scoped PASS with no remaining mandatory. Both reviewers returned final scoped
PASS after final doc/evidence review without rebuilding. Root self-review found
no remaining in-scope gap.

## T29 twelfth batch - Windows fresh exclusive cache object (2026-10-13)

A separate private cache-object package owns physical fresh-file opening, distinct
from byte comparison/network streaming. The Windows adapter atomically create_new
opens read/write with share0 and OPEN_REPARSE_POINT, then observes only that live
handle's attributes/link count/empty size through a narrow initialized-struct FFI.
Directory/reparse/nonempty/multiple-link observations fail without a minted object.
Existing entries are never truncated/deleted; post-create failure closes its handle
but may leave an untrusted entry for owner cleanup. Other OS adapters explicitly
return Unsupported. ExclusiveCacheFile has private File/no Debug/Serialize/Clone
and no ordinary-File constructor; trusted borrowers may explicitly duplicate it.
All duplicated handles must close for sharing restrictions to release. This is
no ACL/metadata-access/admin/parent-namespace/forever-single-link protection, read-
only seal, native identity, VMO or rights. Owners retain those responsibilities.

Initial actual Windows default tests passed4 (0.05s),1 explicit privilege-dependent
symlink test ignored; Clippy6.06s/fmt passed. Tests exercise actual second read/write
opens, delete/rename denial with same-operation success after closure; retained
sharing from an explicit duplicate; unchanged existing regular/hardlink/directory
paths; direct same-handle observation of multiple-link empty/nonempty/directory
negative controls. Public create_new refusal is not misreported as a link checker
branch. The opt-in physical symlink case was explicitly attempted: fixture creation
failed OS1314 before production calls because this host lacks symlink privilege.
No symlink behavior pass or certification is claimed, and no elevation/deployment
was requested. The case stays an explicit eligible-host certification dependency.
An initial internal visibility compilation error was fixed before these results.

The isolated exact three-package graph pins existing windows-sys0.61.2 and windows-
link0.2.1 and is an exact parent graph subset. Offline parent/CLI resolution added
only the private cache-object package and its local edge (8 lines each lock),
retaining every prior version/source/checksum. App manifest adds one private path
dependency. CI routes all cache sources/manifests/lock to Rust gates and runs Linux
unsupported-adapter checks plus native Windows default tests during regression;
privilege-dependent ignored tests are not silently counted. CI policies passed50
(0.081s), architecture/version/diff passed. No Schema, product version or gitlink
changes. Independent review found that NTFS create_new can create an ADS on an
existing physical file. Root added an actual existing:cache negative control that
failed before repair, then rejects colons in every path component except parsed
disk/verbatim-disk prefixes before any OS call. A new fixed InvalidPath error
retains no path; normal/relative/verbatim drives and UNC paths remain accepted,
while named/type streams, nested stream components and ambiguous verbatim/device
prefixes fail. Final actual6 tests passed0.04s with the privileged symlink case
still ignored; base bytes remain unchanged and no stream exists after refusal.
The earlier4-pass/tree App pass is not substituted for final-tree evidence.
Final Clippy passed16.61s. Actual final Windows App passed all20 updater tests
(0.06s after1m42s build), including new-facade sharing denial and same-path access
release. The earlier pre-ADS App pass does not substitute for this final build;
no App-wide pass is claimed. Trusted mutable File borrowers must preserve the
original handle and account for duplication/replacement; the wrapper is not an
unforgeable certificate for caller misuse.

Both independent reviewers closed ADS and returned latest-source scoped PASS.
Dependency actual6 Windows tests passed0.01s with1 privilege test ignored, Clippy
7.34s/fmt, CI50 0.080s/diff. Compatibility actual6 passed0.11s with1 ignored,
Clippy0.06s/fmt, CI50 0.082s/diff. Both independently checked exact lock compatibility
and source/CI/App-facade/document boundaries. Both returned final scoped PASS
on frozen source and final documentation/evidence without rebuilding. Root self-
review found no remaining in-scope gap. The symlink privilege/physical certification
dependency stays disclosed and deferred under the instruction to continue other work.

Details: [exclusive cache object](client-upgrade-exclusive-cache-object.md). Full
T29, namespace/ACL/staging/readonly handoff and physical platform certification
remain unfinished. Downloads retain no total/cumulative duration or retry cap.

## T29 thirteenth batch - read-only cache handle duplicate (2026-10-13)

A new method borrows the trusted current ExclusiveCacheFile File and duplicates
its Windows handle into private ReadOnlyCacheHandle with immutable &File access.
DuplicateHandle uses current process twice, exact FILE_GENERIC_READ, inheritance
false and flags0: no SAME_ACCESS/CLOSE_SOURCE/path reopen. The new valid handle
gets one File RAII owner; source remains unchanged/open. Factoring original live-
handle observation preserves B12 validation. Other OS adapters fail Unsupported.
Only the existing parent windows-sys Threading feature is enabled; all locks/
versions/sources/checksums remain unchanged and the isolated3 graph matches parent.

This narrows only that handle's write rights. Original/prior trusted writers may
still modify bytes; no sealed content or writer retirement is attested. Explicit
read-only clones extend original sharing rules. Duplicates share a file pointer,
so Read/Seek through &File moves the source/alias cursor and owners must serialize
uses. It proves no ACL/namespace/native identity/current eligibility/VMO/rights.
Trusted borrower handle replacement/duplication remains the owner's duty.

Root actual Windows8 tests passed0.07s,1 privileged symlink test remains ignored/
unverified. Two new groups show source/duplicate volume/file index equality, real
read/seek/shared cursor, write/set_len/set_times refusal with unchanged bytes, then
original-writer success and visible changed bytes to disprove content sealing.
Read-only wrapper/clone keep original share0 after the source closes; same-path
read/write/rename/delete succeed only after all copies close. Prior six fresh/ADS
controls remain regressions. Clippy4.22s/fmt passed; CI50 0.072s, architecture/
version/diff passed. No Schema, product version or gitlink changes.

Both independent source reviewers returned scoped PASS without mandatory.
Dependency Windows8 passed0.02s/1ignored, Clippy1.80s/fmt, CI50 0.066s/diff; its
first metadata validation script used default GBK and failed decoding, then the
same read-only graph check passed with explicit UTF8. No repository file changed.
Compatibility Windows8 passed0.04s/1ignored, Clippy0.05s/fmt, CI50 0.068s/diff.
Both checked existing parent feature/unchanged locks and exact graph compatibility.
Actual final Windows App passed all21 updater tests (0.06s after3m00s build),
including real read-only duplicate read/write/truncate and retained sharing through
the facade. No App-wide pass is claimed. Both reviewers returned final scoped
PASS after frozen-source/final doc/evidence review without rebuilding. Root self-
review found no remaining in-scope gap.

Details: [read-only duplicate](client-upgrade-read-only-cache-handle.md). Full T29,
sealed content/retired-writer proof, protected staging/namespace/native identity
and actual original-clock/budget integration remain unfinished. Downloads retain
no total/cumulative duration or retry cap.


## T29 fourteenth batch - actual read-only cache scan (2026-10-13)

Typed ReadOnlyCacheHandle now feeds B3 through a privately cloned current actual
File, with no path reopening, arbitrary mutable File, old match or original-target
replacement. Cancellation precedes cloning, covers every original scan boundary
and follows final hashing. All exits close the internal clone through RAII. Aliases
share a cursor and require owner coordination; writer retirement/content sealing,
namespace protection and original T26 clock/budget integration remain external.
The result is byte-match observation only, never VMO or execution authority.

Three new Windows groups cover actual EOF/full original size and SHA, shared cursor,
wrong size/hash, all six cancellation boundaries and clone cleanup on each exit.
Unsupported platforms report no physical controls executed and use no ordinary
File fallback. The Windows CI regression gate now executes the Verify crate in
addition to its existing Linux gate; CI policy coverage increased to51.

Root Verify29 passed8.90s, Clippy5.89s/fmt; CI51 passed0.053s, architecture/version/
diff checks passed. Changed-transitive regressions passed: Archive10 in15.21s with
Clippy17.98s; actual HTTPS27 and79 wire assertions in10.02s with Clippy7.78s.
Final actual Windows App passed all22 updater tests in0.12s after3m39s build,
including readonly-facade match and wrong-original-hash refusal. No App-wide or
other-platform physical pass is claimed. The prior privileged symlink scenario
remains ignored/unverified, not certified by these tests.

Verify adds a private lower cache-object dependency. Five lock graphs were checked
against their prior state: all previous external versions/sources/checksums/edges
and the Archive backend are unchanged; isolated Verify15, Archive33 and HTTP179
are exact parent-version/source/checksum subsets. Parent changes are local edges
only. No Schema, product version or gitlink changes.

Both independent source reviewers returned scoped PASS without mandatory fixes.
Dependency Verify29 passed4.09s/Clippy1.48s, Cache8 passed0.02s/one privilege ignore/
Clippy0.92s, CI51 passed0.127s; compatibility Verify29 passed10.83s/Clippy2.40s,
Cache8 passed0.09s/one ignore, CI51 passed0.073s. Both ran fmt/diff and independently
checked lock compatibility, actual Windows controls and feature boundaries.
Both returned final documentation/evidence scoped PASS without rebuilding or
changing the frozen source. Root self-review found no remaining in-scope gap.

Details: [read-only byte scan](client-upgrade-read-only-byte-scan.md). Full T29,
protected staging/sealing, actual native identity/qualification and original clock/
budget integration remain unfinished. Downloads retain no total/cumulative
duration or retry cap.


## T29 fifteenth batch - actual read-only cache copy (2026-10-13)

A narrow typed adapter privately clones the current ReadOnlyCacheHandle and runs
B4 on a supplied fresh ExclusiveCacheFile with unchanged original Artifact and
cancellation callback. Cancellation precedes clone and follows final destination
SHA; every exit closes the internal clone. No path/old match is accepted. B4
retains preseek nonempty/regular rejection, real source EOF/size/SHA, sync and same
actual destination EOF/size/SHA rescan. Result is ByteMatch only, never sealed
staging/namespace protection/current qualification/VMO. Trusted alias/borrower
and original clock/budget preservation remain owner duties.

Three new real Windows groups cover10-byte/65,537-byte copies, wrong size/SHA,
nonempty/same-object refusal, source/destination cursor preservation, all13
single-chunk cancellation points and both-path RW/rename release controls.
Short/hash failure may leave full untrusted destination bytes; tests inspect
these rather than claiming zero writes, and no automatic truncate/delete occurs.
Unsupported factories explicitly report no physical controls executed without
ordinary File fallback. No dependency/lock/CI/Schema/version/gitlink changes.

Root first Verify32 passed8.69s; after adding nonempty cursor and two-path rename
controls, final Verify32 passed7.56s with Clippy1.88s/fmt. CI51 passed0.068s,
architecture/version/diff passed. Final actual Windows App passed all23 updater
tests in0.12s after1m30s build, including same-target actual destination copy and
nonempty retry refusal. No App-wide or other-platform physical pass is claimed.
The existing privileged symlink scenario remains ignored/unverified.

Dependency independent frozen-source scoped PASS: Verify32 in3.94s/Clippy1.49s,
Cache8 in0.02s/one privilege ignore/Clippy0.86s, fmt/diff. Compatibility final
scoped PASS: Verify32 in7.57s/Clippy1.29s/fmt, Cache8 in0.04s/one privilege ignore,
CI51 in0.073s/diff and independent65,537-byte SHA confirmation. Both returned
final documentation/evidence scoped PASS on frozen source. No unchanged HTTP/archive
rerun is required; they were verified in B14 and their dependency graphs did not
change. Root self-review found no remaining in-scope gap.

Details: [read-only cache copy](client-upgrade-read-only-cache-copy.md). Full T29,
protected staging/sealing and actual native identity/qualification remain open.
Downloads retain no total/cumulative duration or retry cap.


## T27 eighth batch - supplied Root policy consistency (2026-10-13)

RootPolicyShape now validates the plain supplied signed Root10-field body,
closed key3/rolePolicy6 objects, canonical raw public-key descriptor identity,
unique declared keys and exact nullable four-tuple policies, unique existing
members and bounded positive thresholds. Exactly one Root policy has null
component/channel/target, threshold at least2 and at least3 distinct members;
nonroot policies require component. All20 frozen roles and original nullable
channel/target combinations are preserved without extra cross-role rules.
Ordered sets avoid repeated key scans. Canonical supplied-body bytes preserve
array order and unused keys; result is private/no Debug/Serialize/constructor.

This exactly scopes frozen validateTrustedRoot structural checking. Time fields
are safe nonnegative integer shapes only: equal/reversed times remain legal.
No anchor, signature verification/threshold satisfaction, point validity, current
time/Root trust, rotation lineage or high-watermark fact follows. Strict Ed25519
point/signature policy remains undecided and deferred. No new dependency/lock/
Schema/CI/product version/gitlink changes.

Four new groups include639 actual original Node/AJV cases across20 roles,
nullable scopes, wrong shape/encoding/ID, duplicate keys/tuples/members, absent
members, thresholds, unused keys/array order, literal/escaped duplicate names,
finite binary64 rounding and1MiB/4096 exact/+1 wire/key/policy/member ceilings.
Root final all38 codec tests passed with actual Node22 and both existing oracle
flags: signing6 in5.55s, bytes7 in0.77s, consume4 in1.47s, native3 in0.67s,
rollout4 in0.59s, Root4 in5.41s, Source5 in0.79s and status5 in0.63s. Clippy1.88s/
fmt passed, CI51 in0.047s, architecture/version/diff passed. The earlier37-test
pre-resource tree is not substituted for final evidence. Final actual Windows
App passed all24 updater tests in0.05s after2m29s build, including Root structure
acceptance and weakened-threshold refusal. No App-wide pass is claimed.

Dependency independent frozen-source and final documentation scoped PASS: all38
actual Node22 tests with both old oracle flags passed, signing6 in3.46s, bytes7
in0.38s, consume4 in1.53s, native3 in0.77s, rollout4 in0.68s, Root4 in5.95s
(with639 original cases), Source5 in1.30s and status5 in0.92s. Clippy1.91s/fmt/diff
passed. Compatibility final source/tests/docs scoped PASS: all38 actual Node22
tests with both old oracle flags passed, signing6 in3.67s, bytes7 in0.45s,
consume4 in1.48s, native3 in0.69s, rollout4 in0.58s, Root4 in4.17s (639 cases),
Source5 in0.83s and status5 in0.79s. Clippy1.75s/fmt, CI51 in0.071s and diff
passed. Both final reviews used unchanged frozen source; root self-review found
no remaining in-scope gap.

Details: [Root policy shape](client-upgrade-root-policy-shape.md). Full T27
Root authenticity/chain/high-watermark/native-fact integration remains open.
Downloads retain no total/cumulative duration or retry cap.


## T27 ninth batch - canonical decimal version ordering (2026-10-13)

Private CanonicalAppVersion accepts only an original bounded decoded JSON string
with exactly three canonical ASCII decimal components. Source's existing version
predicate moves unchanged into the shared private shape helper. Numeric ordering
uses component length then ASCII lexicographic order, stopping at the first
difference, exactly equivalent to frozen three-BigInt comparison. No u64/f64
conversion, new magnitude/component-length cap, trim or semver extension occurs.
Only supplied-value ordering follows, not actual source/currentness/eligibility.

Four groups cover each component,9/10/99/100, beyondSAFE/u64 magnitudes, equality/
reverse/transitivity, escaped semantic digits, invalid grammar/types/UTF8,
escaped duplicate properties before shape, major precedence over giant later
components and decoded64KiB exact/+1 in all three components. Metadata and
Credential wire limits remain distinct and unchanged. Final348 actual original
Node BigInt operand pairs match; Source218 original cases remain unchanged.

Root final all42 codec tests with actual Node22 and both old oracle flags passed:
signing6 in5.25s, bytes7 in6.07s, consume4 in2.70s, native3 in7.87s, rollout4
in0.89s, Root4 in6.05s (639 cases), Source5 in0.87s (218), status5 in0.78s and
version4 in1.09s (348). Clippy2.02s/fmt passed; CI51 in0.089s, architecture/
version/diff passed. First336-pair test tree does not substitute for final348
evidence. No dependency/lock/Schema/CI/product version/gitlink changes.
Final actual Windows App passed all25 updater tests in0.07s after2m31s build,
including component precedence, above-u64 values, escaped equality and leading-
zero refusal. No App-wide pass is claimed.

Both independent frozen-source and final documentation reviews returned scoped
PASS. Dependency all42 actual Node22 tests passed with both old oracle flags:
signing6 in3.57s, bytes7 in0.36s, consume4 in1.53s, native3 in0.62s, rollout4
in0.59s, Root4 in4.53s (639), Source5 in0.87s (218), status5 in0.74s and version4
in1.22s (348); Clippy1.53s/fmt/diff passed. Compatibility all42 actual Node22
tests passed with both flags: signing6 in4.78s, bytes7 in0.61s, consume4 in1.99s,
native3 in0.99s, rollout4 in0.86s, Root4 in4.90s (639), Source5 in0.86s (218),
status5 in0.67s and version4 in0.88s (348); Clippy0.09s/fmt, CI51 in0.058s/diff
passed. Frozen source remained unchanged; root self-review found no remaining
in-scope gap.

Details: [version ordering](client-upgrade-version-ordering.md). Full T27 remains
unfinished. Downloads retain no total/cumulative duration or retry cap.


## T27 tenth batch - exact original metadata reference (2026-10-13)

Private MetadataReferenceShape validates frozen closed six-field reference rules
and role-specific scope keys, preserving generic identifier values rather than
adding channel/target enums. Package version/expiry must be null; other metadata
uses positive safe version and nonnegative safe nonnull expiry. Size is1..1MiB.
Raw derivation first applies full original Document bounds/duplicates, then
projects role/scope, Release revision only, other version or Package null and
the original expiresAt missing/null behavior. Nonnull Package expiry is refused,
never erased. Length and SHA bind the current original wire bytes without JCS
reserialization; matching compares all six reference fields.

This is byte/reference association only. Minimal unsigned claims and unselected
extras/signature shapes can derive a positive reference, while all raw bytes
remain hash-bound and resource/duplicate checked. Ignored extras are not treated
as nested metadata references. No full Envelope Schema/signature/expected product
or component/time/current trust/qualification/high-watermark proof follows.
No dependency/lock/Schema/CI/product version/gitlink changes.

Root all46 codec regressions passed with actual Node22 and both old oracle flags:
signing6 in7.24s, bytes7 in0.65s, consume4 in1.75s, native3 in0.76s, reference4
in0.88s (initial721 cases), rollout4 in0.62s, Root4 in6.36s (639), Source5
in0.80s (218), status5 in0.69s and version4 in2.25s (348); Clippy4.91s passed.
After adding only standalone same-length raw-order/opaque/signature-field controls,
latest reference4 passed0.98s with730 actual original Node/AJV cases and final
Clippy2.86s/fmt passed. Initial721 evidence does not substitute for final730;
the other42 tests and source remained unchanged. Common reference Schema is
exercised through original Timestamp.snapshot plus checkReferenceRoles, followed
by original envelopeReference/assertReference, without unrelated time semantics.

CI51 passed0.064s, architecture/version/diff passed. Final actual Windows App
passed all26 updater tests in0.13s after2m22s build, including matching original
bytes and rejecting raw whitespace or declared-version changes. No App-wide pass
is claimed. Dependency final frozen-source/tests/docs scoped PASS: all46 actual
Node22 tests with both old oracle flags passed, signing6 in3.94s, bytes7 in1.17s,
consume4 in3.33s, native3 in0.67s, reference4 in1.08s (730), rollout4 in0.66s,
Root4 in4.16s (639), Source5 in0.74s (218), status5 in0.60s and version4 in0.93s
(348); Clippy1.60s/fmt/diff passed. Compatibility final source/tests/docs scoped
PASS: all46 actual Node22 tests with both flags passed, signing6 in3.16s, bytes7
in0.32s, consume4 in1.42s, native3 in0.64s, reference4 in0.95s (730), rollout4
in0.60s, Root4 in3.74s (639), Source5 in0.75s (218), status5 in0.63s and version4
in0.87s (348); Clippy1.18s/fmt, CI51 in0.046s and diff passed. Both final reviews
used unchanged frozen source; root self-review found no remaining in-scope gap.

Details: [metadata reference](client-upgrade-metadata-reference.md). Complete T27
trust/chain/high-watermark integration remains unfinished. Downloads retain no
total/cumulative duration or retry cap.


## T27 eleventh batch - supplied opening consistency (2026-10-13)

A supplied signed-body Target, JSON Release hash string and nonnull opening are
checked through original complete bounded/duplicate-aware Documents. Nonnull
opening reuses closed7 validation and fixed22-byte domain/fixed-array commitment.
Three strict string comparisons bind targetKey, Release-envelope hash and computed
commitment; safe field access never unwraps a Target envelope or coerces/trims/
casefolds. Null always fails, even against a correct empty commitment. Ignored
Target/scope extras still undergo complete raw resource and duplicate checks.
Only supplied-value consistency follows, not FullTargetSchema/signatures/current
membership/context/selection/entropy/freshness/qualification or execution rights.
Fixed safe errors do not promise original short-circuit error-class equivalence.

Four groups cover every7 opening leaf with old and recomputed commitment, three
independent bindings, minimal/non-Target positive bodies, null/empty commitment,
wrong scope/primitive/fields/types/case/line endings, JSON order/escaped key
equivalence and all three raw inputs' exact/+1 wire/decoded-string/duplicate/
invalidUTF8 controls. Final152 actual original Node/AJV cases include separate
missing Target scope/commitment, actual targetKey/commitment uppercasing and hash
padding. Recomputed valid revisions/IDs/salt remain math-positive without invented
selection or entropy guards. No dependency/lock/Schema/CI/version/gitlink changes.

Root initial all50 codec regressions passed with Node22 and both old oracle flags:
signing6 in5.19s, bytes7 in0.65s, consume4 in1.38s, native3 in0.63s, opening4
in0.95s (147), reference4 in2.16s (730), rollout4 in0.60s, Root4 in5.36s (639),
Source5 in0.79s (218), status5 in0.65s and version4 in0.94s (348). After adding
the dedicated safe JSON error variant for facade decoding and final five negative
controls, latest opening4 passed2.32s (152) and Clippy8.27s/fmt passed. Earlier147
evidence is not substituted for final152; the other46 tests remained unchanged.
CI51 passed0.212s, architecture/version/diff passed. Final actual Windows App
passed all27 updater tests in0.11s after3m00s build, including matching the supplied
binding and rejecting null/wrong Release hash. No App-wide pass is claimed.

Both independent latest frozen-source scoped reviews passed without mandatory.
Dependency actual Node22 and both old oracle flags all50 PASS: signing6 in9.59s,
bytes7 in6.27s, consume4 in4.19s, native3 in1.73s, opening4 in1.07s (152),
reference4 in1.13s (730), rollout4 in1.73s, Root4 in4.96s (639), Source5 in1.43s
(218), status5 in0.71s and version4 in1.01s (348); Clippy0.11s/fmt/diff passed.
Compatibility actual Node22 and both flags all50 PASS: signing6 in4.74s, bytes7
in0.66s, consume4 in1.83s, native3 in0.84s, opening4 in1.00s (152), reference4
in1.04s (730), rollout4 in6.82s, Root4 in10.01s (639), Source5 in2.01s (218),
status5 in1.68s and version4 in8.75s (348); Clippy2.86s/fmt, CI51 in0.067s/diff
passed. Both final documentation/evidence reviews returned scoped PASS on
unchanged frozen source without rebuilding. Root self-review found no remaining
in-scope gap.

Details: [opening binding](client-upgrade-opening-binding.md). Full T27 trust/
selection/high-watermark integration remains unfinished. Downloads retain no
total/cumulative duration or retry cap.


## T27 twelfth batch - exact declared role-policy lookup (2026-10-13)

RootPolicyShape now owns a private immutable typed product/policy cache built in
the same previously validated pass. Original Root canonical bytes and member
order remain unchanged; positive integral threshold<=members<=4096 precedes
bounded and checked u16 conversion. No whole Root JSON clone occurs. Private
RoleContextShape requires string role/product, explicitly present null-or-string
component and object scope. Only missing/null channel/targetKey become None;
other strings remain exact. Scope/body extras still undergo complete raw bounds/
duplicates. Object scope is the stated existing V1 claims precondition; invalid
primitive scope outside that domain is not claimed equivalent to the private
helper. No complete claims Schema is asserted.

Lookup first checks exact product then exact role/component/channel/targetKey,
with no wildcard, fallback, union or default. Root validation already proves
unique tuples. The returned private DeclaredRolePolicy borrows the originating
Root and exposes only threshold, original member IDs and bounded canonical policy.
It grants no Root/key trust, signature satisfaction, role authorization/currentness
or execution authority. No dependency/lock/Schema/CI/version/gitlink changes.

Four new groups cover20 roles, same-role multi-scope/component, required versus
nullable fields, independent mismatches,4096-policy last-item lookup,4096 reverse-
order members and threshold4096, complete ignored-extra bounds/escaped duplicates
and exact/+1 context wire limits. Final966 actual Node cases execute the original
private policyFor body extracted unchanged from frozen signatures.mjs using
vm.compileFunction and the original requireCondition helper. Original
validateTrustedRoot and the explicitly stated context subset precede the call.
Extraction failure has no alternate algorithm or fallback; reference files remain
unchanged. All639 old Root shape/JCS cases remain regressions.

Root all54 codec tests passed with actual Node22 and both old oracle flags:
signing6 in7.93s, bytes7 in0.88s, consume4 in1.50s, native3 in0.87s, opening4
in1.19s (152), policy4 in5.37s (966), reference4 in0.97s (730), rollout4 in0.63s,
Root4 in6.80s (639), Source5 in1.60s (218), status5 in0.70s and version4 in0.97s
(348); Clippy4.99s/fmt passed. CI51 passed0.117s, architecture/version/diff passed.
Final actual Windows App passed all28 updater tests in0.11s after3m13s build,
including exact declared selection and changed-product/undeclared-channel refusal.
No App-wide pass is claimed. Both final documentation/evidence reviews returned
scoped PASS on unchanged frozen source without rebuilding; root self-review found
no remaining in-scope gap.

Both independent frozen-source scoped reviews returned PASS without mandatory.
Dependency actual Node22 and both old flags all54 PASS: signing6 in6.30s, bytes7
in0.69s, consume4 in2.88s, native3 in1.02s, opening4 in1.48s (152), policy4
in5.55s (966), reference4 in2.94s (730), rollout4 in0.74s, Root4 in4.57s (639),
Source5 in0.86s (218), status5 in0.77s and version4 in1.02s (348); Clippy0.17s/
fmt/diff passed. Compatibility actual Node22 and both flags all54 PASS: signing6
in3.70s, bytes7 in0.39s, consume4 in1.71s, native3 in0.75s, opening4 in1.09s
(152), policy4 in4.92s (966), reference4 in1.03s (730), rollout4 in0.70s,
Root4 in4.96s (639), Source5 in0.91s (218), status5 in1.48s and version4 in1.17s
(348); Clippy2.19s/fmt, CI51 in0.063s/diff passed. Frozen source remains unchanged.

Details: [declared role policy](client-upgrade-declared-role-policy.md). Complete
T27 signature/trust/lineage/high-watermark integration remains unfinished.
Downloads retain no total/cumulative duration or retry cap.


## T27 thirteenth batch - original canonical envelope shape (2026-10-13)

This batch checks the same original wire bytes against bounded JCS and the closed
outer envelope/signature shapes; verifies the five common signed context fields
and matching6-metadata/14-credential InputKind; and requires original strictly
ascending unique lowercase key IDs and canonical86-character base64url64-byte
signatures. Nonzero pad bits are refused through decode/reencode equality.
The same frozen20-role set is moved unchanged to shared scalar predicates.
After whole checks, the signed subtree is moved into a Document; private immutable
original bytes and typed records have no public constructor/Debug/Serialize.
This is partial context/encoding only, not full role Schema, expected-context,
curve/signature validity, key authority, threshold, Root trust/time or execution
rights. Minimal bodies and zero/weak signature bytes remain shape-positive only.

Four groups include all20 frozen public vectors, both opposite role profiles,
common-context invalid/missing fields, original spelling/duplicates/UTF8, depth/
string and both exact/+1 wire limits,4096 signatures/+1. Final2080 Node cases
execute unchanged extracted parseEnvelope+decodeBase64Url and original private
isMetadata/schemaName with actual imports, restricted to original full-Schema-valid
signed bodies unchanged. This combination, not parseEnvelope alone or arbitrary
partial bodies, is the stated equivalence domain. A test fixture size-estimation
error failed once and was corrected by measuring exact wire punctuation; the
final four groups passed6.63s. No production failure or reference change occurred.

Root all58 codec tests passed with actual Node22 and both old oracle flags:
signing6 in3.54s, bytes7 in0.46s, consume4 in1.57s, envelope4 in8.05s (2080),
native3 in0.65s, opening4 in0.98s (152), policy4 in4.42s (966), reference4
in2.30s (730), rollout4 in0.63s, Root4 in4.26s (639), Source5 in0.84s (218),
status5 in5.88s and version4 in0.92s (348). Clippy7.54s/fmt passed. CI51 passed
0.128s; architecture/version/diff and frozen boundary checks passed. Final actual
Windows App passed all29 updater tests0.07s after2m36s build, including both
role profiles, original-byte getters, noncanonical wire and pad-bit refusal.
No App-wide pass is claimed. Root self-review found no remaining in-scope gap.
Both independent frozen-source scoped reviews returned PASS without mandatory.
Dependency actualNode22+both old flags all58 PASS: signing6 in3.19s, bytes7
in0.35s, consume4 in1.39s, envelope4 in7.42s (2080), native3 in0.76s,
opening4 in1.00s (152), policy4 in4.07s (966), reference4 in1.06s (730),
rollout4 in0.65s, Root4 in4.11s (639), Source5 in0.83s (218), status5 in0.64s
and version4 in0.96s (348); Clippy1.08s/fmt/diff passed. Compatibility actual
Node22+both old flags all58 PASS: signing6 in3.06s, bytes7 in0.37s, consume4
in1.56s, envelope4 in7.83s (2080), native3 in0.79s, opening4 in0.96s (152),
policy4 in4.17s (966), reference4 in0.94s (730), rollout4 in0.64s, Root4
in4.12s (639), Source5 in0.78s (218), status5 in0.67s and version4 in0.93s
(348); Clippy0.10s/fmt, CI51 in0.058s/diff passed. Frozen code is unchanged.
Both final documentation/evidence reviews returned scoped PASS on unchanged source.
No remaining in-scope issue was found in root self-review.
Details: [canonical envelope shape](client-upgrade-canonical-envelope.md).
No dependency/lock/Schema/CI/version/gitlink changes. Full T27 trust integration
remains unfinished. Downloads retain no total/cumulative duration or retry cap.


## T27 fourteenth batch - supplied high-water comparison (2026-10-13)

One required supplied bounded record is compared with the original canonical
envelope shape; no alternate bytes/hash parameter or absent-record success exists.
Five versioned metadata roles are supported, with positive SAFE current version
and Release revision only. Package/credential roles refuse. Complete four-field
context JCS must match, including all scope fields/extra context keys; positive
SAFE record.version then rejects rollback or same-version original full-envelope
SHA equivocation. The higher-version branch intentionally ignores the old hash,
including missing/null/number/wrong/uppercase variants, as the original helper
does. Whole raw resources/duplicates still cover ignored record fields. Borrowed
context fields avoid a large clone; fixed errors include facade encoding errors.
No record trust, authenticated acceptance, persistence, advancement, default after
loss, Root lineage or current execution rights is granted. This is the stated
role/version precondition subset, not arbitrary malformed-body equivalence.

Four groups cover five supported/all15 unsupported roles, full context/scope,
Release no-fallback, positive current versions, required records, original full
signature-byte binding, ignored-extra resources/duplicates/UTF8 and1935 actual
Node assertHighWater comparisons including all version/hash branches and SAFE
bounds. Initial compilation failed because this isolated Serde graph has no
derive macro; explicit existing Serialize implementation fixed it with no new
dependency. Initial tests incorrectly selected fixtures by position (Package is
second); explicit role filtering corrected coverage. The subsequent four groups
passed2.42s before the fixed facade-envelope-error variant was added. No frozen
reference change occurred.

Root final all62 codec tests passed with actualNode22 and both old oracle flags:
signing6 in3.99s, bytes7 in0.44s, consume4 in2.42s, envelope4 in7.45s (2080),
high-water4 in2.83s (1935), native3 in0.68s, opening4 in0.99s (152), policy4
in4.37s (966), reference4 in0.91s (730), rollout4 in0.67s, Root4 in4.66s (639),
Source5 in0.91s (218), status5 in0.66s and version4 in0.95s (348).
Clippy12.00s/fmt passed; CI51 passed0.131s, architecture/version/diff passed.
Root self-review found no remaining in-scope gap. Final actual Windows App passed all30 updater tests in0.07s after2m46s build,
including Release original-envelope match, equal-version hash mismatch and
rollback refusal. No App-wide pass is claimed.
Both independent frozen-source scoped reviews returned PASS without mandatory.
Dependency actualNode22+both old flags all62 PASS: signing6 in3.32s, bytes7
in0.40s, consume4 in1.70s, envelope4 in20.41s (2080), high-water4 in2.66s
(1935), native3 in0.71s, opening4 in0.96s (152), policy4 in4.16s (966),
reference4 in6.05s (730), rollout4 in0.59s, Root4 in3.96s (639), Source5
in0.78s (218), status5 in0.62s and version4 in0.92s (348); Clippy0.13s/fmt/
diff passed. Compatibility actualNode22+both flags all62 PASS: signing6 in3.00s,
bytes7 in0.34s, consume4 in1.35s, envelope4 in6.78s (2080), high-water4 in2.51s
(1935), native3 in0.66s, opening4 in0.91s (152), policy4 in4.65s (966),
reference4 in0.87s (730), rollout4 in0.66s, Root4 in3.92s (639), Source5 in0.80s
(218), status5 in0.66s and version4 in6.51s (348); Clippy1.04s/fmt, CI51 in0.049s/
diff passed. Frozen code is unchanged. Both final documentation/evidence scoped reviews returned PASS on unchanged source.
Details: [supplied high-water](client-upgrade-supplied-high-water.md).
No dependency/lock/Schema/CI/version/gitlink changes. Full T27 trust/persistence
integration remains unfinished. Downloads retain no total/cumulative duration or
retry cap.


## T27 fifteenth batch - complete frozen envelope Schema (2026-10-13)

A private SchemaCheckedEnvelope consumes one original CanonicalEnvelopeShape
after full frozen Draft2020-12 role Schema validation; only immutable original
shape access remains. Compile-time embedded original20schemas/common use no
duplicated normative files or dynamic schema/path/payload$ref interpretation.
A private OnceLock caches20 validators or a fixed failure; explicit exact-common-
URN retriever denies every other network/file/resource request independently of
parent feature unification. Existing FancyRegex/ECMA backend retains negative
lookahead relative-path support; format validation remains disabled as in Ajv.
Original own raw bytes are reparsed, not reconstructed from partial signed facts.
Complete Schema does not prove crypto, expected context, semantics/time, key/
Root consistency, protected acceptance/high-water persistence or execution rights.

The exact existing parent jsonschema0.46.10/defaultfalse closure is reused;
App/CLI lock nodes/checksums/versions/edges are unchanged except the new codec
dependency edge. Isolated lock has114 exact-parent-version/checksum packages.
Six formerly different isolated transitive versions align to parent: cfg-if
1.0.5->1.0.4, cpufeatures0.3.1->0.3.0, hybrid-array0.4.15->0.4.14, libc
0.2.190->0.2.189, syn3.0.8->3.0.5 and unicode-ident1.0.27->1.0.24.
Initial hand-selected125-package closure was refused by --locked; offline
resolution pruned unneeded/unified dependencies into the114-package subset.
This is not a claim that every old isolated dependency version stayed unchanged.
No parent external upgrade or frozen Schema/CI/version/gitlink change occurred.

Two integration groups test all20 original vectors, full-schema-invalid minimal
bodies, zero/weak signatures and reversed time semantics remaining Schema-only
positive, and31556 actual Node cases generated from committed schema paths and
real nested instances. Required/closed/ref/type/array/numericSAFE/Unicode/regex/
relative-path cases compare the original parseEnvelope+decodeBase64Url composition
with actual AJV, not AJV alone on arbitrary raw bytes. Canonical and encoding
preconditions are part of the claimed domain. Test expected context comes from
each case's own signed fields to isolate Schema, never as production trusted
context; actual expected-context validation remains external. Initial two groups passed149.35s;
the separate explicit resource-refusal unit passed0.03s. A subsequent assertion-
message-only edit removed whole input printing before the final all65 run.

Root all65 tests passed with actualNode22 and both old oracle flags: resource
unit1 in0.03s; signing6 in4.48s, bytes7 in0.44s, consume4 in1.44s, envelope4
in9.31s (2080), high-water4 in2.53s (1935), native3 in0.63s, opening4 in0.92s
(152), policy4 in4.09s (966), reference4 in0.90s (730), rollout4 in0.62s, Root4
in4.08s (639), Schema2 in174.48s (31556), Source5 in0.76s (218), status5 in0.63s
and version4 in0.87s (348); Clippy12.42s/fmt passed. CI51 passed0.176s, architecture/
version/diff passed. Three isolated/App/CLI locked offline feature graphs passed
with the same jsonschema/referencing/jsonschema-regex0.46.10 and FancyRegex0.18.0.
Parent node/checksum/edge comparison proved only the new codec dependency edge.
Final actual Windows App passed all31 updater tests0.17s after3m27s build; no
App-wide pass is claimed. Root self-review found no remaining in-scope gap.
After this run, shared oracle comments were clarified to distinguish B13 unchanged-
body partial encoding from B15 mutated-body full Schema composition. Only comments
changed; independent final-source runs verify the resulting test harness without
another unchanged App rebuild. Compatibility final-source scoped review returned PASS without mandatory:
actualNode22+both old flags all65 PASS: resource unit1 in0.03s, signing6 in5.99s,
bytes7 in0.34s, consume4 in1.36s, envelope4 in7.33s (2080), high-water4 in2.43s
(1935), native3 in0.62s, opening4 in0.91s (152), policy4 in3.81s (966), reference4
in0.80s (730), rollout4 in0.62s, Root4 in3.94s (639), Schema2 in154.44s (31556),
Source5 in0.79s (218), status5 in0.61s and version4 in0.91s (348); Clippy3.24s/
fmt, CI51 in0.061s/diff passed. Its independent lock audit proved the114-package
exact parent subset/checksums and only the new codec edge in the two parent graphs.
Dependency final-source scoped review also returned PASS without mandatory:
actualNode22.23.3+both old flags all65 PASS: resource unit1 in0.03s, signing6
in3.28s, bytes7 in0.34s, consume4 in1.48s, envelope4 in10.21s (2080), high-water4
in2.94s (1935), native3 in0.62s, opening4 in0.95s (152), policy4 in4.17s (966),
reference4 in1.00s (730), rollout4 in0.61s, Root4 in4.03s (639), Schema2 in150.60s
(31556), Source5 in0.73s (218), status5 in0.61s and version4 in0.91s (348);
Clippy0.33s/fmt/diff passed. Its114-package exact-parent audit and both complete
App/CLI actual locked offline metadata graphs passed, with only codec->jsonschema
as a new parent edge. The run had already completed when a later request sought
to avoid a duplicate long test; no running test was interrupted or new repetition
started. Both final documentation/evidence scoped reviews returned PASS on unchanged source.
Details: [complete envelope Schema](client-upgrade-envelope-schema.md).
Full T27 authenticity/persistence integration remains unfinished. Downloads retain
no total/cumulative duration or retry cap.


## T27 sixteenth batch - independent expected-context binding (2026-10-13)

A complete SchemaCheckedEnvelope is compared to a separate required bounded
expected-context Document with own role/product/component/scope fields. No
candidate-derived expected facts, wrong-role repair, default null, fallback,
coercion or trimming occurs. Full candidate Schema makes exact string/null Value
comparison of the first three fields match original strict equality. Entire scope
is compared through borrowed bounded JCS, binding installationScopeId, numeric
channelRevision, target/channel and every original scope field. Extra expected
top-level fields retain raw resources/decoded duplicates. Root missing component
remains distinct from explicit null. Fixed facade errors retain stage without
payload. Source authenticity, crypto/time/Root trust, protected acceptance/high
water/current rights and secure erasure remain external.

Four groups cover all20 independent original expectedContext fixtures, missing/
null/scalar fields, all scope leaves/extras, reordering/escaping/numeric equivalence,
decoded64 KiB and exact/+1 1 MiB expected input, UTF8/depth/escaped duplicates.
First1694 Node cases passed6.12s before facade error variants; final1716 (including
20 reorders and two1MiB boundaries) passed5.66s. The actual original
parseEnvelope+decodeBase64Url bodies use a separately parsed bounded expectedRaw,
making the expected-document input constraint explicit; no expectations come from
the candidate. Acceptance is compared, not exact stage/error categories.

No dependency/lock/Schema/CI/version/gitlink change occurred. Final regression runs
select all68 applicable tests and explicitly filter the one unchanged31556-case
Schema mutation test, already passed by root and both final-source independent
B15 runs. The20-role Schema golden/minimal-negative group still runs. The CI suite
remains complete; no gate is weakened.
Root applicable68 PASS with actualNode22 and both old oracle flags: resource unit1
in0.02s, signing6 in7.20s, bytes7 in0.44s, consume4 in1.57s, context4 in6.04s
(1716), envelope4 in7.02s (2080), high-water4 in2.48s (1935), native3 in0.63s,
opening4 in1.00s (152), policy4 in4.29s (966), reference4 in0.83s (730), rollout4
in0.62s, Root4 in4.04s (639), Schema golden1 in0.29s (one31556 test filtered),
Source5 in0.81s (218), status5 in0.65s and version4 in0.94s (348). Clippy20.17s/
fmt passed; CI51 passed0.055s, architecture/version/diff passed. Actual Windows
App all32 updater tests passed0.17s after2m44s build. No App-wide pass is claimed.
Root self-review found no remaining in-scope gap.
Both independent frozen-source scoped reviews returned PASS without mandatory.
Dependency actualNode22.23.3+both old flags applicable68 PASS: unit1 in0.03s,
signing6 in3.28s, bytes7 in0.36s, consume4 in1.45s, context4 in6.11s (1716),
envelope4 in7.14s (2080), high-water4 in2.46s (1935), native3 in0.65s, opening4
in0.91s (152), policy4 in3.93s (966), reference4 in0.87s (730), rollout4 in0.64s,
Root4 in4.19s (639), Schema golden1 in0.33s (one31556 test filtered), Source5
in0.78s (218), status5 in0.65s and version4 in0.96s (348); Clippy1.76s/fmt/diff
passed. Compatibility actualNode22+both flags applicable68 PASS: unit1 in0.03s,
signing6 in3.39s, bytes7 in0.36s, consume4 in1.45s, context4 in6.11s (1716),
envelope4 in7.03s (2080), high-water4 in2.84s (1935), native3 in0.68s, opening4
in1.01s (152), policy4 in4.32s (966), reference4 in0.90s (730), rollout4 in1.17s,
Root4 in4.04s (639), Schema golden1 in0.35s (one31556 test filtered), Source5
in0.82s (218), status5 in0.64s and version4 in0.99s (348); Clippy0.29s/fmt,
CI51 in0.047s/diff passed. No unchanged graph/App/download repetition occurred.
Both final documentation/evidence scoped reviews returned PASS on unchanged source.
Details: [independent expected context](client-upgrade-expected-context.md).
Complete T27 authenticity/persistence remains unfinished. Downloads retain no
total/cumulative duration or retry cap.


## T27 seventeenth batch - six-role metadata semantics (2026-10-13)

The six metadata role subset of original checkSemantics now consumes a complete
SchemaCheckedEnvelope and private supplied protocol time shape. Time accepts only
nonnegative SAFE finite integral binary64, including -0; it does not attest a
trusted physical clock. Package also requires supplied time but has no own expiry.
Five metadata windows remain half-open, positive and<=365/1/7/30/90 days. Default
Current refuses exact expiry. Explicit AllowExpired affects Root only and still
requires original bounded positive window/now>=issuedAt; it works before expiry
and is ignored for other metadata, never proving lineage or custody.

Recursive reference scope/version/expiry marker checks retain original generic
identifier and historical reference acceptance. Snapshot identity is role+whole
scope JCS for Target/Release, Package hash only across scopes. Timestamp/Target
reference roles, sorted exact host architectures, Release unique targets/Package
scope/one-way irreversible backup, and Package unique helper IDs/target/full-five-
field helper JCS/revision40or64/strict production delta/provenanceSHA/native formats
follow the reference exactly. Canonicalization failures propagate. No Root key/
policy consistency, crypto, actual native probe, supplied time authority, expected
context, protected acceptance/persistence/current rights or secure erasure is
claimed. Credential semantics are explicitly unsupported.

Four groups include all six metadata/all14 unsupported credential roles and4854
actual original Node composed parser/encoding/Schema/checkSemantics cases. The
oracle self-derived context isolates this semantic subset, not independent trust.
Initial4884 cases passed10.17s but one standalone test used invalid backupPolicy
"none"; a replacement attempt failed to alter that line, so the same standalone
failure recurred while4854 valid-enum cases passed8.89s. Correcting the actual
fixture to frozen "notRequired" produced final four groups PASS8.51s. No observed
production/reference divergence occurred. A preliminary review also replaced
canonical errors converted to Option equality with explicit propagation before
testing; no None==None failure acceptance remains.

No dependency/lock/Schema/CI/version/gitlink change. Applicable final regression
selects72 tests, filtering only the unchanged31556-case Schema mutation group
with B15 final evidence; CI retains the complete73-test suite. Root applicable72 PASS with actualNode22 and both old oracle flags: unit1
in0.04s, signing6 in4.03s, bytes7 in0.46s, consume4 in1.66s, context4 in6.01s
(1716), envelope4 in6.98s (2080), high-water4 in2.44s (1935), metadata4 in9.20s
(4854), native3 in0.63s, opening4 in1.05s (152), policy4 in4.05s (966), reference4
in0.84s (730), rollout4 in0.63s, Root4 in4.63s (639), Schema golden1 in0.30s
(one31556 test filtered), Source5 in0.97s (218), status5 in2.87s and version4
in0.96s (348); Clippy19.20s/fmt passed. CI51 passed0.200s, architecture/version/
diff passed. Root self-review found no remaining in-scope gap.
Both independent frozen-source scoped reviews returned PASS without mandatory.
Dependency actualNode22.23.3+both old flags applicable72 PASS: unit1 in0.03s,
signing6 in3.09s, bytes7 in0.34s, consume4 in1.39s, context4 in5.61s (1716),
envelope4 in7.14s (2080), high-water4 in2.42s (1935), metadata4 in9.53s (4854),
native3 in0.67s, opening4 in0.98s (152), policy4 in4.29s (966), reference4 in0.89s
(730), rollout4 in0.68s, Root4 in4.03s (639), Schema golden1 in0.27s (one31556
test filtered), Source5 in0.78s (218), status5 in0.69s and version4 in0.94s (348);
Clippy1.77s/fmt/diff passed. Compatibility actualNode22+both flags applicable72
PASS: unit1 in0.03s, signing6 in3.17s, bytes7 in0.35s, consume4 in1.45s, context4
in6.42s (1716), envelope4 in7.12s (2080), high-water4 in2.56s (1935), metadata4
in9.35s (4854), native3 in0.70s, opening4 in0.94s (152), policy4 in4.32s (966),
reference4 in0.79s (730), rollout4 in0.59s, Root4 in4.14s (639), Schema golden1
in0.28s (one31556 test filtered), Source5 in0.75s (218), status5 in0.66s and
version4 in0.93s (348); Clippy0.29s/fmt, CI51 in0.092s/diff passed.
Final actual Windows App passed all33 updater tests0.13s after2m53s build,
including current Root supplied-time success, exact-expiry refusal, explicit
expired-Root calculation and invalid-time refusal. No App-wide pass is claimed.
Both final documentation/evidence scoped reviews returned PASS on unchanged source.
Details: [metadata semantics](client-upgrade-metadata-semantics.md).
Complete T27 authenticity/persistence remains unfinished. Downloads retain no
total/cumulative duration or retry cap.


## T27 eighteenth batch - fourteen-role credential semantics (2026-10-14)

Complete14-credential checkSemantics subset follows full Schema and existing safe
supplied-time shape. Modules separate entry/metadata-set, update, authorization
and event/task rules. The existing recursive metadata reference checker gains
only private pub(super) visibility, with no algorithm or public entrance change.
Original role lifetimes/nbf, Decision two-way availability, complete metadata set,
entity/path/plan/version/state/rollout/bridge/package/ref/window/opening/locator,
authorization JTI/freeze/backup/preparation/slot/staged/mode, Download package JCS,
event anchors and task bindings are all implemented. Version strings reuse the
prior exact decimal comparator without magnitude caps. Bounded whole-JCS errors
propagate. Six metadata roles explicitly refuse in this entry point.

Preinstall can precede installNotBefore; Actions need not be now-inside-taskwindow
and eventNotAfter/locator expiry are not new now cutoffs. Only original staged
30day addition checks SAFE; nonstaged nearSAFE behavior remains binary64. No
signature/Root/expected source/trusted clock/current qualification/consume/
ownership/execution/persistence/secure-erasure guarantee is introduced. Credential
TTLs are separate from download duration, with no total download cap.

Four groups include all14 fixtures/all6 unsupported metadata, maxTTL/+1, exact
expiry/nbf, original scalar leaves, coherent candidate/bridge/approved ordinary-path,
preinstall/window/task/event exceptions, nearSAFE nonstaged positive, staged
overflow negative and4096-digit version controls. First5820 original Node composed
parser/encoding/Schema/checkSemantics cases passed34.47s, but two standalone fixtures
failed: opening used "salt" instead of frozen "leafSalt", and an assumed fixture
now had not crossed eventNotAfter. A diagnostic single-branch run confirmed the
Schema fixture failure; correcting the field and choosing explicit later supplied
time yielded all four groups PASS32.19s. Extending coherent SAFE/version/preinstall
scenarios produced final7287 Node cases/allfour PASS43.12s before facade staged
error variants were added. No observed production/reference acceptance divergence.
Oracle self-derived context isolates semantics; independent context is B16.

No dependency/lock/Schema/CI/version/gitlink changes. Applicable final regression
selects76 tests and filters only the unchanged31556-case Schema mutation test with
B15 final evidence; CI retains the complete77-test suite. Initial final regression
passed all76 tests but Clippy rejected eight nonminimal boolean expressions.
Changing a == !is_null to equivalent a != is_null preserved all four truth-table
cases and original short-circuit order; both independent roles checked the saved
changes. Post-fix root applicable76 passed with actual Node22.23.3 and both legacy
oracle flags; the7287 credential comparisons passed in48.18s. The unchanged Schema
golden and all other applicable tests passed, with the one31556 mutation test
explicitly filtered. Final root Clippy passed in8.92s and fmt passed.
Actual Windows App passed all34 updater tests in0.16s after1m12s build on the
post-fix production source; no App-wide pass is claimed. Architecture/version/diff
checks passed; CI gate policy passed all51 tests in0.046s.
Independent dependency review returned scoped PASS with no mandatory findings;
its actualNode22.23.3 and both legacy flags applicable76 passed, including7287
credential cases in45.71s. Clippy all-targets -D warnings passed in2.32s; fmt/diff
passed. Independent compatibility review also returned scoped PASS without
mandatory findings; actualNode22.23.3+both legacy flags applicable76 passed,
including7287 credential cases in55.33s. Clippy passed in2.25s with fmt/diff;
CI51 passed in0.048s. Both reviews explicitly filtered only the unchanged31556
Schema mutation test and did not repeat App/graph/download tests. Root self-review
confirmed requirement coverage, exact original branches and exceptional-state
boundaries with no remaining in-scope gap. Final evidence/document reviews are
recorded before commit on unchanged production source.
Details: [credential semantics](client-upgrade-credential-semantics.md).
Complete T27 authenticity/persistence remains unfinished. Downloads retain no
total/cumulative duration or retry cap.


## T27 nineteenth batch - supplied consume binding (2026-10-14)

A borrowed private closed17 ConsumeRequestShape reuses the unchanged B6 request
predicate through parent-only visibility. Digest projection exclusions are not
allowed in this exact semantic request. Full Schema/canonical envelope remains
a separate private-factory input. Three purposes bind exact authorization roles,
the complete original envelope SHA including signatures, eleven whole-JCS fields
and installation scope/plans. Bounded JCS failures propagate without cloning
production subtrees or exposing an unchecked value entrance.

This is only the original assertConsumeBinding post-verification fragment.
The caller must separately establish signature/Root/expected context/time and
current qualification, then perform atomic consumption. consumeKey and
recoveryHandleHash remain Schema-valid independent owner inputs here. No
persistence/ownership/execution/erasure right is established. Download duration
remains unlimited and is unrelated to credential TTL.

Four groups include all three purposes, full nullable/plan records, bound leaf
mutations, raw signature changes, signed-body hash refusal, wrong roles, exact
closed request shape, reordered JSON and duplicate/resource boundaries. Actual
Node runs the precisely extracted original post-verification body, preceded
by original request Schema and complete envelope parser/encoding. There is no
verifyEnvelope substitute or whole-export equivalence claim. App checks all
three purposes, stale hash and forbidden transport fields.

Initial four groups passed in16.72s, including2752 actual original Node
post-verification binding cases. No observed acceptance divergence. Architecture/
version/diff checks passed; CI gate policy passed all51 tests in0.090s.

No dependency/lock/Schema/CI/version/gitlink changes. Final regression selects
80 applicable tests, filtering only the unchanged B15 31556-case Schema mutation
test; CI retains the complete81-test suite. Final root applicable80 passed with
actualNode22.23.3 and both legacy oracle flags, including2752 consume-binding
cases in17.81s. Schema golden and every other applicable test passed, including
B18 credential7287 in78.16s. Final actual Windows App passed all35 updater tests
in0.18s after4m03s build; no App-wide pass is claimed. Root Clippy passed
in12.82s and fmt passed. Independent dependency review returned scoped PASS
without mandatory findings; actualNode22.23.3+both legacy flags applicable80
passed, including2752 consume-binding cases in15.34s and7287 credential cases
in52.42s. Its Clippy all-targets -D warnings passed in2.90s; fmt/diff passed.
Independent compatibility review also returned scoped PASS without mandatory
findings; actualNode22.23.3+both legacy flags applicable80 passed, including2752
consume-binding cases in12.70s and7287 credential cases in45.85s. Its Clippy
passed in2.30s with fmt/diff, and CI51 passed in0.048s. Both reviews explicitly
filtered only the unchanged31556 Schema mutation test and did not repeat App,
graphs or downloads. Root self-review found no remaining in-scope requirement,
exceptional-state, dependency-direction or regression-evidence gap.
Final scoped evidence/document reviews precede commit on unchanged production source. Details: [consume binding](client-upgrade-consume-binding.md).
Complete T27 authenticity/persistence/consumption remains unfinished. Downloads
retain no total/cumulative duration or retry cap.


## T27 twentieth batch - supplied Release renewal (2026-10-14)

The original assertReleaseRenewal relationship now consumes two complete
SchemaCheckedEnvelope factories. Both roles must be release and revision must
strictly increase, allowing jumps. A private borrowed serializer excludes only
root revision/issuedAt/expiresAt and binds all remaining signed properties using
bounded JCS with propagated errors. Nested expiresAt remains included and arrays
retain order. Outer signature records are not business properties.

No time, signature/Root/context/history/qualification, publication, high-water
persistence, ownership or execution certificate is provided. Schema-valid
reversed time and encoding-valid changed signatures are explicit positive relation
controls; separate caller operations must establish their respective obligations.

Four groups cover jump/equal/lower revisions and SAFE boundaries, all business
leaves, nested refs, two-target order, both wrong-role directions, complete
Schema/encoding, duplicates and signature/time exclusions. Actual Node runs
the unchanged original export after original complete parser/encoding with
self-derived expected context for relationship isolation only. App checks jump,
reversed time, business change and equality. No crypto substitute is used.

Initial four groups passed in8.33s, including1256 actual original Node
Release renewal pairs. No observed acceptance divergence.

No dependency/lock/Schema/CI/version/gitlink changes. Final regression selects
84 applicable tests, filtering only the unchanged B15 31556-case Schema mutation
test; CI retains complete85. Root final applicable84 passed with actual
Node22.23.3 and both legacy oracle flags, including1256 renewal pairs in8.77s
and7287 credential cases in76.24s. Schema golden and all other applicable tests
passed. Clippy all-targets -D warnings passed in11.10s on this same frozen
production source, with codec fmt and App facade rustfmt checks.
Architecture/version/diff checks passed; CI51 passed in0.064s. Actual Windows
App passed all36 updater tests in0.24s after4m07s build; no App-wide pass is
claimed. Root self-review found no remaining in-scope gap. Independent dependency
review returned scoped PASS without mandatory findings. ActualNode22.23.3 and
both legacy oracle flags applicable84 passed, including1256 renewal pairs in7.89s
and7287 credential cases in68.16s. Clippy all-targets -D warnings passed in2.63s
with fmt/diff. Independent compatibility review also returned scoped PASS without
mandatory findings. ActualNode22.23.3+both legacy flags applicable84 passed,
including1256 renewal pairs in8.01s and7287 credential cases in59.83s. Clippy
passed in2.45s with fmt/diff; CI51 passed in0.090s. Both reviews filtered only
the unchanged31556 Schema mutation test and did not repeat App/graphs/downloads.
Final scoped evidence/document reviews precede commit on unchanged production source. Details: [Release renewal](client-upgrade-release-renewal.md).
Complete T27 authenticity/protected persistence/publication remains unfinished.
Downloads retain no total/cumulative duration or retry cap.


## T27 twenty-first batch - supplied Release and Package binding (2026-10-14)

The original verifyReleasePackage reference and post-verification relationship
subset consumes two full-Schema canonical factories and a bounded known-target
Document. Role/domain checks precede exact first matching Release target. The
existing unchanged B10 six-field checker binds its Package reference to the
complete original Package bytes; then target scope/appVersion/full launcher JCS
and target activation-mode subset bind exactly. Canonical failures propagate.
Only a small checked reference subtree is cloned, not launcher subtrees.

This does not perform verifyEnvelope or authenticate product/component/target/
time/current selection authority. Cross product/component and encoding-valid
zero signature pure-relationship positives are explicit; callers still enforce
independent expected context and crypto/current qualification. Four target-key
relation tests do not certify native OS formats or real installers. No execution
object, ownership, consume result, high-water persistence or erasure is proved.

Four groups cover all keys, mode subsets/supersets and orders, raw signature
changes/old ref rejection/full rebinding positive, first matching target, roles,
target/resource boundaries and all four Package relationships. Every Package
binding negative rebuilds its complete six-field reference and independently
proves the reference passes, then asserts BindingMismatch, followed by a legal
restore positive. Actual Node uses original assertReference plus exact original
private assertReleasePackageBinding with complete original parser/encoding and
self-derived contexts for relationship isolation, without a crypto substitute
or full-export equivalence claim. App also binds a legal pair and isolates a
publisher mismatch after correct reference rebinding.

Initial four groups passed in18.19s, including2944 actual original Node
Release/Package cases. Root self-review found that known alternate-target and
valid launcher-format negatives were isolated in standalone tests but not
explicitly in the Node corpus. The actual Node set now also receives all eight
successfully rebound relationship negatives and restored positives for each key,
plus first-target controls; this strengthens evidence without changing production.
All four strengthened groups passed in14.58s, including3012 actual original Node
cases; both independent roles checked the saved test delta as strictly in scope.
Architecture/version/diff passed; CI gate policy passed all51 tests in0.100s.

No dependency/lock/Schema/CI/version/gitlink changes. Final regression selects
88 applicable tests, filtering only the unchanged B15 31556-case Schema mutation
test; CI retains complete89. Root applicable88 passed with actualNode22.23.3
and both legacy oracle flags, including3012 Release/Package cases in13.63s and
7287 credential cases in54.18s. Schema golden and every other applicable group
passed; Clippy all-targets -D warnings passed in11.68s with codec fmt and App
facade rustfmt. Actual Windows App passed all37 updater tests in0.16s after
3m05s build; no App-wide pass is claimed. Root self-review found no remaining
in-scope requirement or evidence gap. Independent dependency review returned
scoped PASS without mandatory findings. ActualNode22.23.3+both legacy flags
applicable88 passed, including3012 Release/Package cases in14.72s and7287
credential cases in44.77s. Clippy all-targets -D warnings passed in2.34s,
with fmt/diff. Independent compatibility review also returned scoped PASS without
mandatory findings. ActualNode22.23.3+both legacy flags applicable88 passed,
including3012 Release/Package cases in14.23s and7287 credential cases in46.42s.
Clippy passed in1.73s with fmt/diff; CI51 passed in0.063s. Both reviews filtered
only the unchanged31556 Schema mutation test and did not repeat App/graphs/
downloads. Final scoped evidence/document reviews precede commit on unchanged
production source. Details:
[Release/Package relationship](client-upgrade-release-package-binding.md).
Complete T27 authenticity/protected persistence/selection remains unfinished.
Downloads retain no total/cumulative duration or retry cap.


## T27 twenty-second batch - supplied candidate opening relationship (2026-10-14)

The three original verifyCandidateRelease guards follow two full-Schema canonical
Target/Release factories and a bounded opening Document. Complete original
Release SHA must differ from the declared baseline SHA; unchanged B11 checks
the exact seven-field opening/Target commitment with that same hash; Release
targets must include the Target key. Signed Target Document is borrowed and
only an internally computed64hex scalar gets a private Document construction.

No verifyEnvelope or independent product/component/channel/release-ID context,
trusted time/current membership/eligibility/selection, high-water persistence,
ownership/execution/consumption/erasure guarantee is introduced. Cross context,
zero signature/salt and Schema-valid reversed-time relationship positives remain
explicit. This does not provide package or actual native file authentication.

Four groups include all keys, baseline refusal, seven opening fields, exact raw
signature changes/old opening refusal/rebinding positive, closed shape/duplicates/
resources and wrong roles. The missing-target negative first rebuilds the opening
with the actual changed Release SHA and Target commitment and independently
proves B11 success plus baseline inequality before requiring TargetMissing.
Original Node executes the precisely extracted original three statements and
original assertOpening/sha256 after full original parser/encoding and the actual
bindings/opening Schema. No fake verifier or whole-export equivalence is claimed.
App checks coherent candidate, baseline counterpart and null opening.

Initial four groups passed in5.27s, including608 actual original Node candidate
guard cases, with no observed acceptance divergence.

Both independent static reviews identified a test-only P2: the App null-opening
negative used the already baseline-equal Target and stopped at IsBaseline.
Root restored the nonbaseline hash, re-canonicalized that same Target, retained
a legal opening positive and now requires exact Opening(Mismatch) for null.
No production relationship change or acceptance divergence was found.

No dependency/lock/Schema/CI/version/gitlink changes. Final regression selects
92 applicable tests, filtering only the unchanged B15 31556-case Schema mutation
test; CI retains complete93. Root final applicable92 passed with actual
Node22.23.3 and both legacy oracle flags, including608 candidate cases in4.06s
and7287 credential cases in52.32s. Schema golden and every other applicable
group passed. Clippy all-targets -D warnings passed in28.29s with codec fmt
and App facade rustfmt. Architecture/version/diff passed; CI51 passed in0.080s.
Actual Windows App passed all38 updater tests in0.24s after2m55s build,
including the repaired exact null-opening control; no App-wide pass is claimed.
Root self-review found no remaining in-scope gap. Independent dependency review
returned scoped PASS without remaining mandatory findings. ActualNode22.23.3+
both legacy flags applicable92 passed, including608 candidate cases in4.08s and
7287 credential cases in44.10s. Clippy all-targets -D warnings passed in2.38s,
with fmt/diff. Independent compatibility review also returned scoped PASS without
remaining mandatory findings. ActualNode22.23.3+both legacy flags applicable92
passed, including608 candidate cases in4.11s and7287 credential cases in50.88s.
Clippy passed in4.54s with fmt/diff; CI51 passed in0.052s. Both reviews explicitly
filtered only the unchanged31556 Schema mutation test and did not repeat App/
graphs/downloads. The App P2 is closed. Final scoped evidence/document reviews
precede commit on unchanged production source. Details: [candidate relationship](client-upgrade-candidate-binding.md).
Complete T27 authenticity/protected persistence/current selection stays unfinished.
Downloads retain no total/cumulative duration or retry cap.


## T27 twenty-third batch - supplied public graph (2026-10-14)

The complete supplied-context/reference/reachability subset of original
verifyPublicChain follows full Schema/canonical factories. B10 exact original
six-field references are cached once per immutable member; B16 binds separate
provided product/component plus each declared role/scope. Actual SHA uniqueness,
Snapshot Map overwrite/cardinality, Target baseline/ordinary/bridge reachability,
every concrete Release target Package relationship and no candidate extras all
follow original acceptance. B21 gains only private target-specific helper
extraction without algorithm change; first-key selection remains in B21, never
replacing the public graph's per-target loop.

Indexes and permitted/unique reachable Release sets use the necessary bound
members<=Snapshot.entries<=4096, with no independent aggregate byte cap.
Cached references avoid repeated whole raw hashing; unique immutable Release
checks avoid duplicate path vector growth. Metadata semantics/crypto/Root/time/
context authenticity/current eligibility/persistence/selection/execution/consume/
erasure guarantees remain separate. No Trusted graph object is returned.

Four groups include Schema-only duplicate target positive/second-target launcher
negative and restore, repeated-reference Map positive with4096 entries, direct
4097 parser refusal and actual complete Node export on strong fresh authorized
signed graphs. Every exact Node input first passes real verifyEnvelope against
actual fixture Root role/scope/time, then graph outcome compares Rust full Schema+
B17 semantics+new pure graph in that explicit signed-fixture domain. No private
keys are serialized/logged/committed and no fake verifier/general full-export
equivalence is claimed. App validates full graph, missing member, wrong supplied
component and excess count before member decoding.

Initial four-group run passed three groups, including28 actual signed/
preflighted Node cases, in3.85s, but the4097 negative fixture used the positive
wire helper and unwrapped its correct MemberLimit refusal before the assertion.
Root changed that test to assert direct bounded parser MemberLimit on original
serde bytes. No production acceptance divergence was observed.
Corrected allfour groups passed in3.90s, including28 actual signed and
independently preflighted Node public graph cases.

No dependency/lock/Schema/CI/version/gitlink changes. Final regression selects
96 applicable tests, filtering only the unchanged B15 31556-case Schema mutation
test; CI retains complete97. Root final96 passed with actual Node22 and both
original optional oracle environments enabled. Clippy passed in25.32s; codec
format and App facade format passed. Actual Windows App39 passed in0.20s after
3m05s compilation. Architecture guard/version consistency passed; CI policy51
passed in0.075s. Root self-review confirms complete reference/context/reachability,
bounded ownership and every concrete target, with no new trust or download cap.
Both independent scoped reviews found no mandatory issue. Dependency review's
frozen96 passed (graph28 in3.81s, credential7287 in47.39s), Clippy2.27s/fmt/diff
passed. Compatibility review's frozen96 passed (graph28 in4.50s,
credential7287 in44.60s), CI51 in0.076s/diff passed. Its first format invocation
briefly discovered a future B24 unformatted test draft, unrelated to B23 source.
Root moved that draft outside test discovery; the reviewer reran format and
Clippy on the unchanged B23 source, both passed (Clippy2.20s). No production
behavior failure occurred. All test executables closed before source development
resumed. Both independent final documentation reviews passed with no mandatory
issue; the B23 source and evidence are approved for this scoped commit.
Details: [public graph](client-upgrade-public-graph.md).
Complete T27 authenticity/protected persistence/current selection stays unfinished.
Downloads retain no total/cumulative duration or retry cap.

## T27 twenty-fourth batch - supplied check/refresh responses (2026-10-14)

Scope: complete original assertApiContract check/refresh response acceptance:
closed frozen Schemas with exact embedded dependency retrieval; whole canonical
reparse; independent embedded credential64KiB bounds; unchanged B18 claims
semantics evaluated at max(iat,nbf); seven identity fields and full scope;
refresh writableSessionId binding. Response requestId/workflowRevision stay
independent schema facts. No signature sorting/verification or current-time
authority is added to API Schema data. Unit-only result, not a trusted check
projection, writable session, persisted chain or execution permission.
B18 claims helper and JCS equality are extracted privately without algorithm
change; its original envelope API remains unchanged.

Initial four integration groups passed in26.39s, including3446 actual complete
original Node API contract cases. Root self-review strengthened standalone
whole-scope negatives to require precise BindingMismatch with restoration, and
added the greatest within64KiB signature count positive before the next-count
over-limit negative. These are test-only additions; production remains unchanged.
Dependency independent review found test-only P2: Node targetKey mutation used
the default fixture key (no-op). Root's just-added standalone scope control
also reused that key; the first final run consequently stopped with3/4 in
check_response (Node3446 passed in22.32s, but its target mutant was ineffective).
Root changed both controls to another legal ARM key, added explicit mutation
inequality, and Node now preflights complete Schema and both independent
credential semantics before requiring exact API_RESPONSE_MISMATCH for all
identity/scope/session negatives. Production algorithm is unchanged. Corrected
final regression passed all101 applicable tests, filtering only unchanged
B15's31556-case Schema mutation test (CI complete102 retained). Actual Node22
and both original optional oracle environments were enabled. Clippy3.31s,
codec/App facade formatting passed. Actual Windows App40 passed in0.25s after
2m43s compilation; architecture/version/diff passed, CI policy51 passed in0.047s.
Root self-review confirms closed-response coverage, unchanged original helpers,
raw and canonical limits, error isolation and absence of authority/total-download
caps. Both independent scoped reviews close the test P2 with no residual
finding. Dependency frozen101 passed: new3446 in21.82s, credential7287 in46.02s,
Clippy2.22s/fmt passed. Compatibility frozen101 passed: new3446 in21.66s,
credential7287 in44.25s, Clippy2.92s/fmt, CI51 in0.050s/diff passed. Both use
actual Node22 with both legacy environments and only the unchanged31556 filtered.
All test executables closed. Both independent final documentation reviews
passed with no mandatory finding before this scoped commit.
No partial first101 PASS is claimed. The Node oracle calls complete original assertApiContract
with nested required/scalar mutations, isolated cross-identity controls and
future-nbf/duplicate-signature/zero-signature contract positives; no fake crypto
or private-key output. Root self-review and static P2 closure are complete;
final independent evidence is recorded above.
No dependencies/locks/frozenSchema/CI/VERSION/gitlinks change. Details:
[check responses](client-upgrade-check-response.md). Complete T27 still unfinished.
Downloads retain no total/cumulative duration or retry cap.

## T27 twenty-fifth batch - supplied download-info response (2026-10-14)

Scope: complete original download-info API response contract, fixed closed Schema/
common+Download embedded resources, whole canonical bounded reparse, credential
64KiB, unchanged private claims semantics at max(iat,nbf), exact declared package
ID/size/SHA and nbf<locator expiry<=exp. No signature/current-time/eligibility/
ApprovedDestination or total download deadline is produced. Source class/request
ID/URL stay original Schema facts. Unicode scalar URL length is preserved.
T29 separately checks usable origin/DNS/socket before actual network transport.

Private next-batch preparation outside repository discovery generated1517 actual
original complete API cases; no current frozen review was affected. Initial Rust four-group comparison passed in11.14s, including all1517 actual
original Node cases. Both independent scoped static reviews found no mandatory
issue. Final Clippy passed in23.72s; codec/App facade format passed.
Actual Windows App41 passed in0.18s after3m03s compilation. Architecture/version/
diff passed; CI policy51 passed in0.059s. Root self-review verifies closed single
credential coverage, original binary64 primitive equality, Unicode scalar URL
bounds and no authority/total-download budget. Root final106 passed with actual
Node22 and both legacy oracle environments (new1517 in9.31s, credential7287
in49.55s), filtering only unchanged31556 Schema mutations; CI complete107
retained. Dependency independent106 passed: new1517 in9.42s, credential7287
in45.76s, Clippy2.21s/fmt/diff passed. Compatibility independent106 passed:
new1517 in10.26s, credential7287 in44.33s, Clippy2.29s/fmt, CI51 in0.046s/diff
passed. Both use actual Node22 and both legacy environments, with only the same
unchanged31556 filtered. Both scoped reviews report no mandatory issue.
All test executables closed. Both independent final documentation reviews
passed with no mandatory finding before this scoped commit. Original Node binding negatives
preflight full API Schema and independent original credential semantics before
exact API_RESPONSE_MISMATCH. No fake cryptography/private-key output.
No dependencies/locks/Schema/CI/VERSION/gitlinks change. Details:
[download-info](client-upgrade-download-info.md). Complete T27/T29 remain unfinished.
Downloads retain no total/cumulative duration or retry cap.

## T27 twenty-sixth batch - supplied three-purpose responses (2026-10-14)

Scope: complete original validate/consume response API contracts for preinstall,
install and activate. Fixed six-operation enum/closed Schemas and exact seven
embedded common/credential dependencies; whole bounded canonical reparse;
independent credential64KiB and unchanged supplied semantics at max(iat,nbf).
Validate keeps original purpose binding; Consume binds declared transactionId.
AuthorizationRevision/requestId remain independent Schema facts. Unit-only
contract success never becomes execution authorization, durable consumed ACK,
current qualification or permission to revive a retired executor.

Outside-repository preparation ran the actual complete original API oracle
and generated5453 cases, with all explicitly named positive/negative controls
checked against their intended original outcomes. No frozen B25 discovery was
affected. Initial four-group Rust comparison passed in31.63s, including all5453
actual original Node cases. Both scoped static reviews found no mandatory issue.
Root final111 passed with actual Node22/both legacy environments, filtering only
unchanged31556 Schema mutations (CI complete112 retained): new5453 in28.53s,
credential7287 in52.43s. Clippy29.15s/codec+App facade format passed.
Actual Windows App42 passed in0.25s after3m17s compilation; architecture/version/
diff passed, CI policy51 passed in0.047s. Root self-review confirms complete six
closed responses, field/purpose mapping, original helper use and no authority
or download cap. Dependency independent111 passed (5453 in29.22s, credential7287
in46.05s, Clippy3.11s/fmt/diff); compatibility independent111 passed (5453
in28.90s, credential7287 in46.35s, Clippy2.47s/fmt, CI51 in0.060s/diff).
Both actual Node22/both legacy environments, same sole31556 filter; no mandatory
findings and all executables closed. Both independent final documentation
reviews passed with no mandatory issue before this scoped commit. Node transaction negatives
preflight full Schema and original independent credential semantics before exact
API_RESPONSE_MISMATCH. No fake crypto/private-key output or production writes.
No dependencies/locks/Schema/CI/VERSION/gitlinks change. Details:
[purpose responses](client-upgrade-purpose-responses.md). T27/T32 remain unfinished.
Downloads retain no total/cumulative duration or retry cap.

## T27 twenty-seventh batch - reference API read-set correction (2026-10-14)

Future transaction-control response work exposed an actual reference dependency
bug: valid multi-object currentReadSet passes direct ordinary-object Schema but
assertApiContract strict reparse produces null prototypes, causing Ajv
uniqueItems/fast-deep-equal TypeError (valueOf). Independent read-only review
confirmed the root cause and correction as a technical bug, not a new policy
decision. No Schema/wire/interface changes are needed.

Five regression groups were written against the actual reference first:3 failed
with the original TypeError and2 strict/prototype guards passed. Root corrected
only the API adapter: canonicalize once, unchanged strictparse of identical bytes,
ordinary JSON.parse copy only for Schema, original value for semantics/return.
All5 then passed (Node runner859.73ms). Null-prototype parser, resource checks,
whole-object uniqueness, no-hook/no-pollution and return identity remain intact.

Root full Node22 reference172 passed in14.65s; affected Rust12 passed:
check3446 in23.39s, download1517 in10.81s and purpose5453 in31.31s.
Root architecture/version/diff passed; CI policy51 passed in0.052s.
All five service pinned committed-source and bundled artifact bytes exactly
match their manifest SHA256; reference code is not part of that bundle.
Root self-review confirms unchanged strict checks, original semantic input/
return identity, closed Schema rejection, whole-value uniqueness and no hooks/
prototype assignment. Both independent static reviews found no mandatory issue.
Dependency independent full Node172 passed in19.04s and affected Rust12 passed
(check24.10s/download10.11s/purpose29.69s), actual Node22/both legacy environments,
all executables closed. Compatibility independent full Node172 passed in14.32s
and affected Rust12 passed (check23.15s/download9.52s/purpose30.08s), actual
Node22/both legacy environments; no mandatory finding and all executables
closed. Both independent final documentation reviews passed after clarifying
that only the affected Rust groups were repeated for this reference correction.
The full Rust suite, App tests and heavy31556 mutation test were not rerun;
affected Rust12 were rerun as recorded above.
No Schema/OpenAPI/profile/vectors/locks/crypto/trust/limits/VERSION/gitlinks
changes. Details: [read-set compatibility](upgrade-api-read-set-compatibility.md).
Complete client authority/storage/execution tasks remain unfinished.
Downloads retain no total/cumulative duration or retry cap.

## T27 twenty-eighth batch - supplied transaction-control responses (2026-10-14)

Scope: complete original seven response contracts for consume-status, cancellation
and execution review. Fixed operation enum, closed frozen Schemas/exact four
local-only dependencies, whole bounded canonical reparse. Unit success is not
current read-set proof, verified-object identity, request correlation, durable
ACK, owner revival or execution authorization. Whole-object uniqueness retains
different revisions/hashes as Schema-valid; complete current review remains
owner responsibility. Outside-repository actual original Node generated434
cases after B27 dependency correction, with all named intended outcomes checked.
Initial four Rust integration groups passed in10.54s, including all434 actual
complete original Node contract cases. Both independent static reviews found
no mandatory issue. Actual Windows updater App43 passed in0.31s after3m27s compilation;
architecture/version/diff passed and CI policy51 passed in0.075s. Root self-review
confirms full fixed seven-operation mappings, exact resource closure, status
nullability, whole-value uniqueness and no added rights/current-time proof.
Root applicable codec116 passed with actual Node22/both legacy environments,
filtering only unchanged31556 (complete CI117 retained): new434 in7.18s and
credential7287 in52.86s. Clippy passed in23.61s; codec and App facade format passed.
Dependency independent task-scoped5 passed: integration4/actual Node434 in7.41s,
resourceunit1 in0.03s (four unrelated units filtered), Clippy1.96s/fmt/App facade
format/diff passed. Both legacy environments enabled; no old full111/7287 or
App repeat. Compatibility independent task-scoped5 passed: integration4/actual
Node434 in7.32s, resourceunit1 in0.02s (four unrelated units filtered),
Clippy1.69s/codec+App facade format/diff passed; CI policy51 in0.053s.
Both actual Node22/both legacy environments, no repeat of old111/7287/App or
31556. Both scoped reviews passed without mandatory findings and all processes
closed. Both independent final documentation reviews passed before commit.
No Schema/OpenAPI/profile/vectors/dependencies/locks/CI/crypto/trust/limits/
VERSION/gitlinks changes. Details:
[transaction responses](client-upgrade-transaction-control-responses.md).
Complete T27/T32 remain unfinished. Downloads retain no total/cumulative
duration or retry cap.

## T27 twenty-ninth batch - supplied task-create responses (2026-10-14)

Scope: complete original reconciliation-create/recovery-review-create response
API contracts. Fixed two-operation enum/closed Schemas and exactly five local
resources; whole canonical bounded reparse; both credentials independently
retain64KiB and unchanged supplied semantics at max(iat,nbf). Complete declared
task equals both credential tasks; full credential scopes and three identities
match. Unit success is not authenticated task/action rights, current read set,
durable outcome, occupancy release or owner revival.
Outside-repository actual original Node generated5694 nested/control cases.
Preparing the oracle caught a positive-control helper incorrectly classifying
restored bindings as negatives, then a no-op guard incorrectly covering raw
scalar enumeration; both test-only preparation issues were fixed before source
entered the worktree. Named mutations now require a genuine difference and
binding negatives preflight original full Schema/credential semantics before
exact API_RESPONSE_MISMATCH. All named intended outcomes then passed.
Root task-scoped5 passed: integration4/actual original5694 in51.69s,
resourceunit1 in0.03s (five unrelated units filtered). Clippy alltargets passed
in14.24s; codec+App facade format passed. Architecture/version/diff passed and
CI policy51 in0.102s. No production source/helper change beyond this independent
module/export and App facade; unchanged full116/31556 were not repeated.
Root self-review confirms full bounded inputs, both original credential
semantic gates/caps, full task/scope and three identity bindings, meaningful
precise negative/restore controls and no elevation to task/action authority.
Both independent static reviews found no mandatory issue. Actual Windows
updater App44 passed in0.26s after2m50s compilation. Dependency independent5
passed: integration4/actual original5694 in55.73s, resourceunit1 in0.02s
(five unrelated units filtered), Clippy3.33s/fmt/App facade format/diff passed.
Actual Node22/both legacy environments; no repeat of old116/31556 or App.
Compatibility independent5 passed: integration4/actual original5694 in46.63s,
resourceunit1 in0.02s (five unrelated units filtered), Clippy2.42s/codec+App facade
format/diff passed; CI policy51 in0.074s. Actual Node22/both legacy environments,
no repeat of old116/31556 or App. Both scoped reviews passed with no mandatory
issue and all processes closed. Both independent final documentation reviews
passed before commit.
No Schema/OpenAPI/profile/vectors/dependencies/locks/CI/crypto/trust/limits/
VERSION/gitlinks changes. Details:
[task-create responses](client-upgrade-task-create-responses.md).
Complete T27/T32 remain unfinished. Downloads retain no total/cumulative
duration or retry cap.

## T27 thirtieth batch - supplied event/task-action receipts (2026-10-14)

Scope: complete original six event-report and two task-action response contracts.
Fixed eight-operation enum/closed frozen Schemas with only common resource;
whole bounded canonical reparse. Original receipts contain no credential or
additional response semantic guards. Event receipts allow duplicate complete
objects and same event different dispositions (no uniqueItems), with1..4096
bounds. Same family remains Schema-compatible; request/event/task identities
are supplied structural facts, not correlation or durable commitment.
Unit success never releases protection/occupancy, admits quality contribution,
establishes task completion or revives an executor.
Outside-repository actual original complete Node generated358 required/scalar/
boundary cases, all named intended outcomes checked. Root task-scoped5 passed:
integration4/actual original358 in2.28s, resourceunit1 in0.02s (six unrelated
units filtered); Clippy alltargets23.58s/codec+App facade format passed.
Architecture/version/diff passed; CI policy51 in0.051s. Actual Windows updater
App45 passed in0.36s after2m51s compilation. Root self-review confirms full fixed
eight-operation mapping/common-only resource closure, original duplicate/
same-family acceptance, limits and no authority upgrade. No existing helper
changed, and unchanged old116/31556 were not repeated.
Both independent static reviews found no mandatory issue. Dependency
independent5 passed (integration4/actual358 in3.02s/resourceunit1 in0.02s/six
unrelated units filtered, Clippy1.93s/fmt/App facade format/diff passed).
Compatibility independent5 passed (integration4/actual358 in2.21s/resourceunit1
in0.02s/six unrelated units filtered, Clippy2.13s/fmt/App facade format/diff;
CI policy51 in0.075s). Both actual Node22/both legacy environments, no repeat
of old116/31556/App. Both scoped reviews passed without mandatory finding and
all processes closed. Both independent final documentation reviews passed
before commit. No Schema/OpenAPI/profile/vectors/
dependencies/locks/CI/crypto/trust/limits/VERSION/gitlinks changes. Details:
[receipts](client-upgrade-receipt-responses.md).
Complete T27/T20/T22/T32 remain unfinished. Downloads retain no total/cumulative
duration or retry cap.

## T27 thirty-first batch - supplied helper-cleanup response (2026-10-14)

Scope: complete original helper-cleanup response API contract. Fixed frozen
closed Schema/exact two common+helper-cleanup resources; whole bounded canonical
reparse and independent credential64KiB/unchanged supplied semantics max(iat,nbf).
Original response has no additional binding guards. Unit success never grants
protected-object identity/current eligibility/deletion rights or executor revival.
Outside-repository actual original complete Node generated288 required/scalar/
boundary cases with all named intended outcomes checked. Root task-scoped5
passed: integration4/actual original288 in2.61s, resourceunit1 in0.03s (seven
unrelated units filtered); Clippy alltargets22.79s/codec+App facade format passed.
Architecture/version/diff passed; CI policy51 in0.096s. Root self-review confirms
fixed closed response/resource closure, unchanged original semantic gate and
caps, meaningful negatives/restores, no request-correlation invention or
protected-object/deletion authority upgrade. No existing helper changed and
unchanged full116/31556 were not repeated. Both independent static reviews
found no mandatory issue. Actual Windows updater App46 passed in0.27s after
2m47s compilation. Dependency independent5 passed (integration4/actual original
288 in2.97s, resourceunit1 in0.02s/seven unrelated units filtered, Clippy2.19s/
codec+App facade format/diff passed). Actual Node22/both legacy environments;
no old116/31556/App repeat. Compatibility independent5 passed (integration4/
actual original288 in3.73s, resourceunit1 in0.04s/seven unrelated units filtered,
Clippy1.88s/codec+App facade format/diff passed; CI policy51 in0.057s).
Actual Node22/both legacy environments, no old116/31556/App repeat. Both scoped
reviews passed without mandatory finding and all processes closed. Both
independent final documentation reviews passed before commit. No Schema/OpenAPI/profile/
vectors/dependencies/locks/CI/crypto/trust/limits/VERSION/gitlinks changes. Details:
[helper-cleanup response](client-upgrade-helper-cleanup-response.md).
Complete T27/T28/T31/T32 remain unfinished. Downloads retain no total/cumulative
duration or retry cap.

## T27 thirty-second batch - supplied channel-change response (2026-10-14)

Scope: complete original channel-change response API contract. Fixed frozen
closed Schema/common-only dependency, whole bounded canonical reparse. Original
response contains no credential or extra semantic binding guard; three channel
names and positive safe revision boundaries remain fixed. Unit success is not
persisted settings/current selection/correlation or cross-channel permission.
Outside-repository actual original complete Node generated40 controls with all
named intended outcomes checked. Root task-scoped4 passed: integration3/actual
original40 in0.95s, resourceunit1 in0.02s (eight unrelated units filtered);
Clippy alltargets22.93s/codec+App facade format passed. Architecture/version/diff
passed; CI policy51 in0.179s. Actual Windows updater App47 passed in0.30s after
2m57s compilation. Root self-review confirms exact common-only/fixed closed
contract, original channel/revision/required boundaries and no invented current
revision/settings-write/request-correlation or permission carry-over rule.
No existing helper changed and unchanged full116/31556 were not repeated.
Both independent static reviews found no mandatory issue. Dependency
independent4 passed (integration3/actual40 in1.09s/resourceunit1 in0.03s/eight
unrelated units filtered, Clippy2.03s/fmt/App facade format/diff passed).
Compatibility independent4 passed (integration3/actual40 in1.34s/resourceunit1
in0.05s/eight unrelated units filtered, Clippy1.81s/fmt/App facade format/diff;
CI policy51 in0.093s). Both actual Node22/both legacy environments, no old116/
31556/App repeat. Both scoped reviews passed without mandatory findings and
all processes closed. Both independent final documentation reviews passed
before commit. No Schema/OpenAPI/profile/
vectors/dependencies/locks/CI/crypto/trust/limits/VERSION/gitlinks changes. Details:
[channel-change response](client-upgrade-channel-change-response.md).
Complete T27/T28/T40 remain unfinished. Downloads retain no total/cumulative
duration or retry cap.

## T27 thirty-third batch - fixed client response entry (2026-10-14)

Scope: exhaustive fixed26 non-management client response enum and one supplied
contract entry, delegating unchanged family validators. Original actual registry
contains26 client/11 management operations; all exact client names roundtrip,
management/unknown/aliases fail. Typed wrapped errors keep original fixed codes.
Unit success never proves transport/current state/Root/signature/ACK or rights.
No lifecycle/settings writes or owner revival. Separate task-create contracts
remain outside the client entry as their original operations are management.
Outside-repository actual original complete Node generated773 cases, including
all26 positive fixtures/26x26 cross-operation pairs and semantic/relation errors.
Relation negatives preflight original full Schema/independent credential
semantics before exact API_RESPONSE_MISMATCH. Root task-scoped3 passed:
all773 actual original cases in2.91s, including negative fixed-code comparisons.
Clippy alltargets36.44s/codec+App facade format passed; architecture/version/diff
passed and CI policy51 in0.141s. Root self-review confirms exhaustive26 exact
mappings and11 management exclusions, no changed family validator/interface,
original error-code preservation, metadata byte profile and no authority
upgrade. Unchanged full135/31556 were not repeated. Both independent static
reviews found no mandatory issue. Actual Windows updater App48 passed
in0.25s after2m44s compilation. Dependency independent3 passed (actual original
773 in3.15s, Clippy2.08s/codec+App facade format/diff passed). Compatibility
independent3 passed (actual original773 in3.16s, Clippy1.78s/codec+App facade
format/diff passed; CI policy51 in0.073s). Both actual Node22/both legacy
environments, no new resource unit and no old135/31556/App repeat.
Both scoped reviews passed without mandatory finding and all processes closed.
Both independent final documentation reviews passed before commit. No Schema/OpenAPI/profile/
vectors/dependencies/locks/CI/crypto/trust/limits/VERSION/gitlinks changes. Details:
[client response entry](client-upgrade-response-entry.md).
Full T27/real transport/ownership remain unfinished. Downloads retain no
total/cumulative duration or retry cap.

## T27 thirty-fourth batch - supplied client HTTP errors (2026-10-14)

Scope: frozen closed error/unactivated body Schemas with exact client operation/
HTTP-status error alternatives from original OpenAPI, no duplicated handwritten
code/status table. Fixed26 client inventory complete/unique, common-only resource,
whole bounded canonical reparse. Original retryable flags/optional retryAfterMs
and special checkAfterMs remain hints, not total download/retry budgets.
Unit success is not trusted transport/correlation/confirmed absence/ACK,
occupancy release/execution right/owner revival.
Outside-repository actual original OpenAPI Ajv generated14464 cases over all
26 client operations/all21 error codes/status pairs plus required/scalar/delay
boundaries; all named intended outcomes checked. Root task-scoped5 passed:
integration3/actual original14464 in3.05s/resourceunits2 in0.07s (nine unrelated
units filtered), Clippy alltargets22.83s/codec+App facade format passed.
Architecture/version/diff passed; CI policy51 in0.109s. Root self-review confirms
fixed local resources, complete unique client inventory, original operation/
status/code and retry-field rules, bounded input/non-object handling and no
authority/absence/retry-budget inference. No existing validator/helper changed,
and unchanged full138/31556 were not repeated. Both independent static reviews
found no mandatory issue; compatibility independently checked every actual
OpenAPI error alternative equals the generic error branch or exact unactivated
reference (zero divergent), confirming the membership/body combination has no
missing variant guard in the frozen artifact. Actual Windows updater App49
passed in0.28s after2m46s compilation. Dependency independent5 passed
(integration3/actual14464 in2.99s/resourceunits2 in0.08s/nine unrelated units
filtered, Clippy1.78s/fmt/App facade format/diff passed). Compatibility
independent5 passed (integration3/actual14464 in2.75s/resourceunits2 in0.07s/nine
unrelated units filtered, Clippy1.77s/fmt/App facade format/diff passed;
CI policy51 in0.070s). Both actual Node22/both legacy environments, no old138/
31556/App repeat. Both scoped reviews passed without mandatory findings and
all processes closed. Both independent final documentation reviews passed
before commit. No Schema/OpenAPI/profile/vectors/
dependencies/locks/CI/crypto/trust/limits/VERSION/gitlinks changes. Details:
[client error responses](client-upgrade-error-responses.md).
Full T27/real transport/ownership remain unfinished. Downloads retain no
total/cumulative duration or retry cap.

## T27 thirty-fifth batch - supplied client HTTP entry (2026-10-14)

Scope: complete fixed client HTTP contract entry. HTTP200 delegates unchanged
complete operation response;4xx/5xx delegates unchanged original OpenAPI-bound
error response; all other statuses reject. Typed wrapper errors retain original
fixed codes, App raw byte entry keeps metadata profile/duplicate rejection.
Unit success is structural for declared errors too, not business success,
trusted transport/ACK/absence/current rights or owner revival.
Outside-repository actual original oracles generated15497 cases: complete
operation and direct OpenAPI/Ajv error controls plus undeclared HTTP statuses
and wrong-branch operation bodies. Three integration groups and actual App
facade were checked; no new resource unit. Root task-scoped3 passed with all
15497 original cases in5.97s, Clippy alltargets41.65s/codec+App facade format
passed. Architecture/version/diff passed; CI policy51 in0.058s. Actual Windows
updater App50 passed in0.31s after3m11s compilation. Root self-review confirms
fixed200/error/undeclared-status branches, unchanged validator delegation,
typed/fixed errors and metadata profile, structural-only unit success for
declared errors, and no authority or business-state inference. Unchanged full
143/31556 were not repeated. Both independent static reviews found no mandatory
issue. Dependency independent3 passed (actual original15497 in7.57s,
Clippy2.17s/codec+App facade format/diff passed), actual Node22/both legacy
environments, no new resource unit/no old143/31556/App repeat. Compatibility
independent3 passed (actual original15497 in6.44s, Clippy1.89s/codec+App facade
format/diff passed; CI policy51 in0.093s), actual Node22/both legacy environments,
no new resource unit/no old143/31556/App repeat. Both scoped reviews passed
without mandatory finding and all processes closed. Both final documentation
reviews passed before commit. No Schema/OpenAPI/profile/vectors/
dependencies/locks/CI/crypto/trust/limits/VERSION/gitlinks changes. Details:
[complete HTTP entry](client-upgrade-http-response-entry.md).
Full T27/real transport/ownership remain unfinished. Downloads retain no
total/cumulative duration or retry cap.

## T27 thirty-sixth batch - supplied client request contracts (2026-10-14)

Scope: complete original26 client request contracts. Shared exact operation
catalog alias, fixed frozen request Schemas/exact18 dependencies, whole bounded
canonical reparse. Module boundaries separate resource/cache, credential/scope
gates and operation guards. Original supplied semantics max(iat,nbf)/64KiB each,
full scope and operation-specific digest/purpose/event/evidence rules, reusing
existing credential semantics and consume digest without alteration.
Unit success never verifies signatures/current time/read set/owner/object,
sends a request, consumes/cancels/executes, records quality or erases a secret.
Later post-auth consume envelope hash binding is deliberately a separate phase.
Outside-repository actual original Node generated7285 cases over all26/cross
pairs/surrounding required/scalar/explicit preflighted guard/cap cases. A draft
preparation negative used a nonexistent epoch field; reading frozen Schema
corrected it to ownerEpoch before shipment and all guard negatives now preflight
full Schema/independent credential semantics before exact expected API code.
Initial integration4/7285 passed in72.72s; test-only signature capacity arithmetic
replaced iterative appends while retaining actual canonical <=64KiB/next>64KiB
assertions (production unchanged), then integration4/7285 passed in32.31s,
resourceunit1 in0.44s (11 unrelated units filtered), Clippy21.91s/fmt passed.
Independent compatibility review found missing isolated consume identity
controls, now all3 purposes x3 legal identifiers rebuild the digest, pass
original Schema/credential semantics and precisely reject consume mismatch,
then restore. Its requested isolated non-telemetry purpose control was
impossible: all five original request fields are const. That test draft failed
original Schema preflight (integration4 failed in47.10s); reading each frozen
Schema confirmed the boundary and reviewer withdrew this part of the finding.
Final controls precisely require Schema rejection plus restore, without changing
production. Root final scope5 passed: integration4/actual original7313 in33.32s,
resourceunit1 in0.40s (11 unrelated units filtered), Clippy4.06s/fmt passed.
Actual Windows updater App51 passed in0.37s after2m37s compilation; subsequent
test-oracle-only correction did not change App production/facade/test source.
Architecture/version/diff passed; CI policy51 in0.060s.
Root self-review confirms original26 request acceptance, closed18 resources,
whole bounded input/each64KiB credential, exact supplied semantics/full scope
and every operation guard, preflighted isolated error controls and the actual
Schema-first redundant event-purpose boundary. Existing B6/B18 helpers remain
unchanged; no later B19 authentication binding or request sending/authority
was inferred. Unchanged full146/31556 were not repeated. Dependency independent scope5 passed:
integration4/actual7313 in33.10s, registryunit1 in0.37s/11 filtered,
Clippy1.72s/codec+App facade format/diff passed. Compatibility independent
scope5 passed: integration4/actual7313 in32.64s, registryunit1 in0.38s/11 filtered,
Clippy1.69s/codec+App facade format/diff passed; CI51 in0.044s. Both actual
Node22/two legacy environments, no old146/31556/App repeat. Both scoped reviews
passed with no remaining mandatory findings and all processes closed.
Both independent final documentation reviews passed before commit.
No Schema/OpenAPI/profile/vectors/dependencies/locks/CI/crypto/trust/limits/
VERSION/gitlinks changes. Details:
[client requests](client-upgrade-request-contracts.md).
Full T27/real transport/ownership remain unfinished. Downloads retain no
total/cumulative duration or retry cap.

## T27 thirty-seventh batch - actual fixed control POST (2026-10-14)

Scope: actual26-route one-POST transport and App feature port, frozen OpenAPI
routing, original complete request/HTTP-response contracts, actual canonical
request and preserved full response wire bytes. Fixed production control origin/
TLS minimum, no redirects/proxies/cookies/decompression/automatic retries,
bounded streamed metadata response and fixed non-sensitive SDK error mapping.
No signature/current time/read-set/owner/ACK/right or remote absence is inferred.
Initial independent resolution selected52 external versions outside the parent;
it was replaced with the actual parent lock before final validation, with zero
external package-version drift. Parent App/CLI lock updates add only the local
crate. Initial compile/default tests3 passed (4.10s after3m37s compilation;
actual TLS test ignored until explicit harness). Parent-aligned default3 passed
in3.94s after2m32s compilation. First actual TLS run failed (3 passed/1 failed
in2.96s): peer and Rust had separately generated fresh signing fixtures, so
correct received bytes differed from the separately generated expectation.
Harness now passes one actual-original fixture snapshot to both sides;
assertions avoid printing entire synthetic credentials.
Independent static review found permissive charset quote stripping and
first-only/non-digit Content-Length parsing. Root repaired both with exact
unquoted/one-pair UTF-8, ASCII OWS and single ASCII-digit length; unit negative/
restore and actual raw TLS malformed charset/duplicate/conflicting length/
transfer framing/truncation controls were added. Root actual HTTPS4 passed
in2.48s with67 wire requests over all26 routes after8.59s compilation.
First final Clippy rejected two collapsible-if styles; equivalent MSRV-compatible
let-chain guards fixed them. Final original Node22 default3 passed in12.45s
(one actual TLS test ignored only in the default command) after10.62s compilation.
Actual HTTPS4 passed in3.86s with70 observed requests/all26 routes, including
one received-then-dropped connection submitted exactly once and a progressing
response spanning multiple read-idle periods. Clippy alltargets8.37s/codec
transport format/App facade format passed. Actual Windows updater App52 passed
in0.91s after4m compilation, then final source App52 passed in0.34s after1m06s
compilation. Architecture/version/diff passed; CI policy52 in0.131s (initial
0.087s also passed). Root self-review confirms fixed route/domain/ordinary
certificate identity, canonical original request, whole streamed wire-byte
cap/EOF and unchanged response contract, ambiguous framing/charset refusal,
fixed non-sensitive errors and test-only injection boundaries. Real proxy/
redirect/cookie/canonical-body/no-retry/progress/cancellation controls support
the scoped claims. This bounds accumulated payload, not SDK peak memory,
does not retry or persist a business result and cannot grant current authority
or prove remote absence. Parent/CLI external versions remain unchanged.
Dependency independent scoped PASS: actual Node22 default3 in2.55s/1TLS ignored,
isolated HTTPS4 in3.80s/70wire/all26routes, Clippy2.47s/HTTP+App facade
format/diff passed. Compatibility independent scoped PASS: actual Node22
default3 in2.73s/1TLS ignored, isolated HTTPS4 in4.11s/70wire/all26routes,
Clippy2.37s/HTTP+App facade format/diff passed; CI52 in0.053s. Both found no
remaining mandatory issue, independently confirmed the final header repairs,
one-submit/progress/peer-seen cancellation proof and scoped lock/CI boundaries.
All processes and isolated peers closed. Old codec146/31556 and full unrelated
App were not repeated. Both independent final documentation reviews passed before commit. A draft progress write used an invalid newline
argument and truncated this local file; root restored the exact committed
history and appended only this batch before further review.
Production deployment and prior undecided policies remain deferred; no Schema/
OpenAPI/profile/vectors, crypto/trust policy, product VERSION or gitlinks change.
Details: [actual control transport](client-upgrade-control-http.md).
Full T27/real authority/protected persistence/workflows remain unfinished.
Downloads retain no total/cumulative duration or retry cap.

## T29 sixteenth batch - same-file FullPack/Package binding (2026-10-14)

Scope: complete read-only metadata-to-actual-FullPack relationship; separate
feature crate composes unchanged canonical/full Schema/expected-context/full
raw-reference/Package semantics/canonical-version guards with actual archive
inspection. Independent caller Artifact matches all FullPack fields; each
declared native/helper byte identity must exist as a direct regular member.
A bounded identity set avoids helper x member scans or repeated name copies.
No source path is invented from installationTarget, and identical claims/members
may share bytes. Nested native files alone cannot satisfy direct membership.
No signature/native publisher/current qualification/immutable token or install
right is inferred; outer-wrapper SBOM/provenance admission stays independent.
Initial actual-file4/14 generated ZIP variants passed in5.87s after1m01s
compilation. Original Node strong-signature/Schema/semantics preflight was real;
native payloads are synthetic and not certified. Final wrong-role-before-I/O
and final-return cancellation controls passed: root scoped actual4/14 in4.28s
after5.66s compilation, Clippy alltargets10.04s/package+App download facade
format passed. Actual Windows updater App53 passed in0.36s after3m10s compilation.
Architecture/version/diff passed; CI policy53 in0.061s. Root self-review confirms
mandatory Package role and complete independent expectation/reference/version/
Artifact preconditions, same actual source scan, bounded direct-member index
and native/helper completeness, non-path semantics and final cancellation with
no partial return. No native-signature/current-rights inference was introduced.
Independent dependency scoped actual4/14 passed in4.20s, Clippy1.81s/package+
App facade format/diff passed, actual Node22/two legacy environments. It reviewed
all new code/tests/CI/App/docs without mandatory findings and closed all
processes. Compatibility independent scoped actual4/14 passed in4.17s, Clippy1.98s/
package+App facade format/diff passed; CI53 in0.074s, actual Node22. It found
no mandatory issue and closed all processes. Both reviews confirmed original
outer-upload versus direct FullPack boundaries and supplied-only signature/
native proof limits. Existing full archive/codec suites and unrelated full
App were not repeated. Both independent final documentation reviews passed before commit.
App/CLI only add the local crate; standalone lock inherits parent external
versions. Original crypto policy/clock/platform/production capacities decisions
and deployment remain deferred. No Schema/OpenAPI/profile/fixed vectors,
product VERSION or gitlinks change. Details:
[same-file Package binding](client-upgrade-package-binding.md).
Full T29/protected staging/native probes and T27 trust remain unfinished.
Downloads retain no total/cumulative duration or retry cap.

## T29 seventeenth batch - actual declared native materialization (2026-10-14)

Scope: supplied Package native role/helper ID plus independent safe ZIP locator
to current same readonly source/native membership and actual new exclusive
destination copy/sync/full rescan, retaining complete supplied five-field native
identity and current readonly destination duplicate. Private shared metadata/
member helpers preserve the preceding inspect_full_pack public behavior and
all original4 actual-file tests. No source path from installationTarget, path
reopen/delete, command, native signature/current authority/sealing/ACL/writer
retirement/protected-stage or execution token is introduced.
Initial compile caught a moved mutable callback in the helper extraction;
an explicit reborrow preserves the original callback lifetime/order.
Actual Windows initial8 passed in22.78s after7.17s compilation: original4 plus
new4 real handle groups/all4 native selections/Store/Deflate/33MiB, field/EOF/
readonly/sharing proofs, nonempty/mismatched/cancel controls. Final actual
partial-size assertion and App real typed copy were added. Root final scoped8
passed in23.47s after4.83s compilation, Clippy alltargets3.38s/package format
passed. Architecture/version/diff passed; CI53 in0.048s. Root self-review confirms
private helper extraction preserves the old gates/callbacks/public API, all
metadata/selection/locator gates precede source duplication, complete current
membership and physical materialization reuse proven scan/rescan/sync/EOF,
original/destination readonly duplicates do not imply sealing/other-writer
retirement, fixed errors and final cancellation release local views without
returning facts. Genuine partial-size and observed typed-handle tests support
the scoped claims. Independent dependency scoped8 passed in30.86s, Clippy2.43s/
package+App facade format/diff passed, actual Windows/Node22/two legacy
environments; source/test/architecture review found no mandatory issue and
all processes closed. Actual Windows updater App53 passed in0.61s after3m12s compilation, including
the added real typed native copying/readback/readonly sharing stage in the
existing Package App test; no new App test count is invented. Compatibility
independent scoped8 passed in28.22s, Clippy2.04s/package+App facade format/diff
passed; CI53 in0.061s, actual Windows/Node22/two legacy environments. Both scoped
reviews found no mandatory issue, confirmed helper extraction compatibility and
returned facts/readonly permission limits, and all processes closed.
Existing complete archive/codec and unrelated full App were not repeated.
Both independent final documentation reviews passed before commit. Parent/CLI locks only add the
direct existing cache edge; external versions remain unchanged.
No Schema/OpenAPI/profile/fixed vectors/product VERSION/gitlinks changes.
Previously undecided time/platform/native/crypto policies and deployment remain
deferred. Details: [native bytes materialization](client-upgrade-native-materialization.md).
Full T29/native authenticity/protected staging and T27 trust remain unfinished.
Downloads retain no total/cumulative duration or retry cap.


## T09 third batch - owned server artifact capture (2026-10-14)

The separate service's artifacts/capture module now captures an exact-length
source into private process memory under explicit source/memory capacities.
It requires complete EOF and computes the observed SHA-256 before exposing only
independent ReaderAt reads and a copied identity. The isolated artifact inspector
uses that capture through its existing safe source adapter; the additional
Manifest InspectSnapshot entry delegates all existing current Root/current deny
and complete byte-binding checks. Existing CLI flags/output/error categories and
the old immutable ReaderAt inspection API remain compatible.

The root affected regression includes a real 33 MiB-plus source, independent
hash and concurrent read controls, source/read-buffer mutation refusal, capacity
boundaries and fixed incomplete/extra/invalid/cancelled reads. Actual local HTTP
body streaming/owner cancellation and a signed upload with current package deny
and restored positive passed. Both independent static roles found that the first
HTTP cancellation marker could run before Capture; the primary role moved it
into the actual body Reader and required no result before owner cancellation.
After that test-only repair, capture5 groups passed in6.752s, verification4 in
4.669s and default CLI2 in2.157s. The earlier rebuilt actual CLI3 including
child-process E2E passed in2.857s. Full default Go tests, vet, module verification,
server/CLI builds, original pinned contracts, formatting and diff checks passed.
Both independent implementation reviews passed after that repair. Dependency
actual Windows Go1.27.2 passed archive5.968s/capture5=6.879s/sourcefile2.962s/
verification4=4.330s/actualCLI3+E2E6.638s, affected vet/format/diff. Compatibility
independently passed archive3.374s/capture5=7.351s/sourcefile2.547s/
verification4=5.084s/actualCLI3+E2E3.055s and affected vet/format/diff.
Test processes are closed. Client
architecture/version/diff checks passed. Both independent final documentation reviews passed before commit.

This is process-memory byte ownership, not durable content-addressed storage,
native authentication, supply-chain approval, Artifact admission or a production
upload HTTP workflow. Existing unchanged database/provider/browser/deployment
opt-ins are not claimed; no SQL/provider/contract/dependency/version changes.
Full T09 remains unfinished and deployment remains deferred. Download has no
total/cumulative elapsed deadline or retry budget.


## T09 fourth batch - independent observed FullPack (2026-10-14)

The separate service now captures a safe exact root ZIP through its existing
complete parser while retaining the selected root buffer in the live nested
memory budget through later outer entries. Capturing FullPack and parsing it
share one buffer; capture/nested bounds are checked before allocation. The old
non-container root-file selector and uncaptured sibling accounting remain
compatible. The new extraction port requires an owned upload Snapshot and
returns a separate observed FullPack Snapshot, comparing actual parent and child
identities. The additional Manifest verifier entry binds that child to the exact
signed FullPack after the same current Root/deny checks; every failure returns
no earlier Facts or child.

Actual Store and Deflate ZIPs with a 33 MiB-plus expanded fixture passed child
capture and independent inspection. Exact/below child capacities, held-root plus
later-sibling memory limits and restored controls, root-only selection, later
corruption, invalid/cancelled parent, signed child/outer separation and current
package deny/restored positive passed. These are byte/format/signature tests,
not native platform certification.

Final root full default Go tests passed (archive3.870s/capture7.437s/
extraction6.863s/sourcefile5.812s/verification9.347s/defaultCLI4.828s),
as did full vet/module verification/server+CLI builds/pinned contracts/
format/diff. Rebuilt actual CLI3 including child E2E passed in2.864s.
Both independent static reviews passed without mandatory findings; independent
frozen tests passed: dependency actual Windows Go1.27.2 archive3.859s/
capture3.804s/extraction7.499s/sourcefile5.965s/verification9.316s/
actualCLI3+childE2E4.440s plus affected vet/format/diff. Compatibility independently
passed archive4.240s/capture3.535s/extraction4.330s/sourcefile6.015s/
verification6.018s/actualCLI3+childE2E8.415s and affected vet/format/diff.
Test processes are closed.
The final return cancellation poll has static/full-regression evidence; no dedicated
final-poll injection test is claimed. Both independent final documentation reviews passed before commit.

This batch selects no storage provider or deployment policy, writes no paths,
and grants no durable storage, supply-chain/native approval or Artifact admission.
Source snapshot, aggregate live nested buffers and the returned child's memory
capacity remain distinct finite allocations. The final business owner still
rereads current trust/time/deny and protected object identity at admission.
No SQL/dependency/contract changes; unchanged database/provider/deployment opt-ins
are not claimed. Full T09 remains unfinished, and downloads retain no cumulative
elapsed deadline or retry budget.


## T09 fifth batch - exact observed evidence source bytes (2026-10-14)

The separate service now captures safe exact outer logical filenames, including
directory prefixes, without choosing host paths or inner ZIP decoys. Extraction
privately reuses its existing FullPack capture work; old FullPack API, selector
and fixed invalid error and old bare-root/root-ZIP selectors remain unchanged.
The additive Manifest evidence entry applies the same current signed trust and
byte-binding inspection, extracts exactly the declared SBOM/provenance and checks
each independent owned child's observed size/hash. Both return together; any
error, including failed provenance after successful SBOM, returns no earlier
Facts or child.

Root actual logical-name/inner-decoy/capacity/cancel/later-corrupt/source/restore,
bare+prefixed signed evidence, current deny/restore and actual hash/context
controls passed. Deliberately incomplete SBOM/provenance content tests
prove exact declared bytes, not CycloneDX or provenance content approval.
Full default Go suite passed (archive6.292s/capture14.949s/extraction7.083s/
sourcefile5.863s/verification9.597s/defaultCLI16.399s), as did full vet/
module verification/server+CLI builds/pinned/format/diff. Rebuilt actual CLI3
including child-process E2E passed in2.816s.
Compatibility audit found the original hash negative also changed size; the
primary role changed it to equal-length single-byte mutation with an explicit
different SHA, preserved signed claims and correct regenerated ZIP CRC.
The repaired three evidence groups passed in1.703s; production code stayed
unchanged after full checks. Both independent implementation/test reviews passed
after repair. Dependency actual Windows Go1.27.2 archive2.037s/capture2.025s/
extraction3.708s/sourcefile5.736s/verification8=5.970s/actualCLI3+E2E7.888s
and affected vet/format/diff passed. Compatibility independently passed
archive6.216s/capture3.552s/extraction4.372s/sourcefile3.272s/
verification8=9.424s/actualCLI3+E2E4.188s and affected vet/format/diff.
Test processes are closed; client
architecture/version/diff passed. Both independent final documentation reviews passed before commit.

Memory capacities bound parent, aggregate nested buffers, ordinary evidence
capture, independent child buffers and the retained first evidence separately;
the owner must account for the aggregate. No production capacity, schema
acceptance, scanner, pipeline allowlist, native authentication, durable storage,
admission or deployment policy is selected by this batch. No SQL/contract/
dependency/version changes; unchanged database/provider/deployment opt-ins
are not claimed. Full T09 remains incomplete; download has no total/cumulative
elapsed deadline or retry budget.


## T09 sixth batch - explicit offline CycloneDX Schema (2026-10-14)

The separate service now supports explicit CycloneDX1.6 structural validation
using the complete original versioned Draft7 BOM Schema, SPDX/JSF closure and
Apache License2.0 from fixed upstream commit55343ba19dee1785acf1ce9191540d5fd7b590db.
All four raw digests/inventory are checked before compilation and Git preserves
the bytes. A closed loader refuses runtime Schema retrieval; an instance's
schema annotation cannot select resources. Unknown factory versions refuse
without fallback, actual specVersion must match1.6, and original format/pattern
checks apply. The independent dependency role compared all four original upstream
resources byte-for-byte with the local copies and confirmed equality.

The new strict JSON explicit-capacity parser retains all Unicode/duplicate/
depth32/string64KiB/member4096/binary64 limits; legacy metadata/JCS entry points
still enforce1MiB. The isolated SBOM inspector requires explicit version and
source/snapshot/JSON capacities, reads through the existing supported safe
source adapter into an owned Snapshot, then prints only supported schemaVersion
and the original observed source size/hash after success. Fixed diagnostics
expose no input path, body or provider details.

Initial core3/Schema4 groups passed in1.053s/1.407s, including actual SPDX/JSF
references, noncanonical raw JSON above1MiB under explicit capacity, independent
concurrent reads, schema/format/version negatives and restored positives,
ambiguous strict parser input, exact capacity/cancel, unknown version, resource
drift/missing/extra and closed-reference loader. Later core/Schema/defaultCLI
passed in5.862s/1.636s/1.505s; built CLI was explicitly skipped pending actual
rebuild/E2E. Source/parsed/validator copies have separate explicit finite memory
boundaries; synchronous JSON/Schema calls cannot be interrupted internally.
The root full default Go suite passed (core3=6.160s/Schema4=6.794s/
defaultSBOMCLI1=11.472s) along with all existing metadata/signing/claims/trust/
archive/evidence regressions. Full vet/module verification/server+CLI builds/
pinned contracts/format/diff and client architecture/version/diff passed.
Rebuilt actual SBOMCLI2 including positive and mismatched-version child E2E
passed in7.669s. Both independent implementation/test reviews passed.
Dependency actual Windows Go1.27.2 core3=1.688s/archive1.565s/capture3.963s/
extraction7.103s/Schema4=3.780s/sourcefile3.675s/verification8=9.189s/
actualSBOMCLI2+childE2E4.446s and affected vet/format/diff passed.
Compatibility independently passed core3=1.647s/archive6.058s/capture4.037s/
extraction4.712s/Schema4=3.784s/sourcefile5.969s/verification9.364s/
actualSBOMCLI2+childE2E4.457s and affected vet/format/diff passed.
Test processes are closed.
Both independent final documentation reviews passed before commit.

This adapter does not approve the owner's accepted-version policy, bind SBOM to
the final installer, authenticate a fake JSF structure, validate provenance/
pipeline or scan risks, or grant Artifact admission/storage/deployment. No SQL,
protocol, external dependency or application version changes. Full T09 remains
unfinished; downloads retain no total/cumulative elapsed deadline or retry budget.


## T09 seventh batch - Manifest-bound SBOM Schema composition (2026-10-14)

The separate service's additional SBOM Schema inspection entry joins the same
current signed Manifest/Root/deny and exact paired evidence inspection to its
supported offline Schema adapter. Schema version comes only from the validated
original Manifest, actual specVersion must match the supported declaration and
the Schema result must retain the exact original signed SBOM size/hash.
Explicit evidence and JSON capacities remain separate; unknown version/Schema/
format/capacity/cancel/deny failures return no earlier Facts or paired bytes.
Old verification APIs are unchanged; the private decoder additionally retains
the Manifest's SBOM schemaVersion.

Root signed pretty raw CycloneDX1.6 with prefixed logical filename/current reads,
byte-only stage positive before each precise unsupported/mismatched-version/
schema/format negative and restored positives, explicit JSON capacity, current
deny/restore, independent target, cancellation and caller-buffer original-identity
controls passed. The final full default Go suite passed (verification11=12.703s/
Schema4=6.073s/core3=5.893s/defaultSBOMCLI1=4.042s) plus all existing regressions;
full vet/modverify/server+CLI builds/pinned contracts/format/diff passed.
Actual rebuilt SBOMCLI2 including positive/wrong-version child E2E passed in
3.015s. Dependency independently passed actual Windows Go1.27.2
verification11=10.893s/Schema4=2.845s/actualSBOMCLI2+childE2E3.686s and affected
vet/format/diff; frozen code/test review found no mandatory issue.
Independent compatibility actual Windows Go1.27.2 passed verification11=6.483s/
Schema4=1.563s/actualSBOMCLI2+childE2E2.379s and affected vet/format/diff.
Both frozen independent code/test reviews passed without mandatory findings;
all processes are closed. Client architecture/version/diff passed.
Both independent final documentation reviews passed before commit.

This is structural observation, not accepted-version policy, final-installer SBOM
binding, provenance signatures/source/pipeline, vulnerability/scan, native
authentication, durable storage or Artifact admission. Final admission rereads
current time/trust/deny/immutable identities. No SQL/provider/protocol/dependency/
application version changes; unchanged DB/provider/browser/deployment opt-ins
are not claimed. Full T09 remains incomplete and downloads retain no cumulative
elapsed deadline or retry budget.


## T09 eighth batch - actual protected-source composition (2026-10-14)

The separate service's existing real PostgreSQL artifact integration now also
runs owned upload capture, exact paired evidence and offline SBOM Schema against
the actual protected runtime/SERIALIZABLE source. Existing protected genesis,
T05 dual review, T06 Root publication and package-role rotation stay unchanged;
the original byte-only fixture remains unchanged. Added strong signed pretty
CycloneDX1.6 at safe outer evidence prefixes retains actual raw identity and
current read-set facts. Schema/version/format negatives first pass the same
actual byte-only observer before precise Schema refusal/zeroFacts/nilpair;
each restores a supported positive.

A valid canonical indexed package deny in the actual isolated table refuses the
same previously accepted owned upload; restoring exact original genesis body/
digest/index restores the positive. These owner SQL changes are adversarial
current-read/integrity controls, not approved deny commands or publication writes.

The root actual baseline passed in13.794s (7.49s/4.54s); after the test-only delta
the two actual PostgreSQL groups passed in14.052s (6.70s/5.14s). Full default Go
tests/vet/modverify/server build/pinned contracts/format/diff also passed.
Dependency independently passed the same actual PostgreSQL2 groups in13.788s
(6.83s/4.71s), affected vet/format/diff and frozen code/test scoped review.
Compatibility independently passed the same actual PostgreSQL2 groups in14.901s
(7.34s/4.79s), affected vet/format/diff and frozen scoped review.
All independent/root processes are closed; private connection values stayed only
in process environment and were not printed. Client architecture/version/diff
passed. Both independent final documentation reviews passed before commit.

No production code, SQL schema, provider/dependency/protocol/application version
or deployment changes. This proof concerns protected reading and structural
observation, not HA/native publisher, final-installer SBOM binding, provenance/
pipeline/scan or Artifact admission. Unrelated provider/browser/deployment opt-ins
are not claimed; full T09 remains incomplete. Download has no total/cumulative
elapsed deadline or retry budget.


## Ninth batch: declared primary SBOM installer byte binding

InspectSBOMBinding extends the existing current signed Manifest, full archive,
paired exact evidence and offline CycloneDX1.6 Schema observer. It requires at
least one SHA-256 declaration on metadata.component, the primary described
component. Every primary SHA-256 declaration must be exactly 64 hexadecimal
characters and match the signed finalInstaller.sha256. Equivalent upper/lower
hexadecimal forms retain the original raw SBOM identity. Dependency components
cannot provide this identity; other hash algorithms do not replace SHA-256.
This is a declared byte relationship, not publisher, name/version, pipeline,
provenance-signature, vulnerability-scan or Artifact admission approval.

Only the already Schema-validated immutable original SBOM is decoded. The
additional read remains bounded by explicit JSONBytes, uses bounded chunk reads
and checks cancellation. Every failure returns zero Facts and no paired evidence.
Existing byte-only and Schema-only APIs retain their original acceptance.

Root new binding tests passed all three groups in3.980s: correct and upper-case
hex, missing primary/hash, wrong/short digest, other-algorithm-only,
dependency-only decoy and contradictory primary SHA-256. Each Schema-valid
negative first passes the same Schema observer, then returns precise binding
failure; supported positives are restored. Current deny/restore, cancellation
and independent JSON capacity controls also passed.

The actual protected PostgreSQL composition additionally verifies the same
binding through the existing SERIALIZABLE runtime token. A Schema-valid wrong
primary digest first passes actual Schema observation, then binding refuses
without Facts/evidence. Restoring the same supported owned upload succeeds.
The current canonical indexed package deny refuses both Schema and binding
observers on that previously accepted upload; exact genesis restoration restores
both. These owner SQL mutations remain adversarial read-current controls, not
approved deny commands or admission writes. Root actual PostgreSQL2 groups
passed in18.686s (11.31s/4.87s), without printing private connection values.

Root full default Go tests passed, including verification14=14.016s and all
existing regressions. Full vet/module verification/server build/pinned contracts/
changed-source format/diff passed. Client architecture/version/diff passed.
Dependency independently passed verification14=9.736s and actual PostgreSQL2
groups16.590s (7.80s/6.25s), affected vet/format/diff and complete scoped review.
Compatibility independently passed verification14=10.051s and actual PostgreSQL2
groups15.579s (7.79s/5.42s), affected vet/format/diff and complete scoped review.
Both frozen independent code/test reviews found no mandatory issue. All test
and tool processes are closed; private credentials were not printed.
Both independent final documentation reviews passed before commit. No production SQL, provider, dependency,
protocol or application version changes. Full T09 is incomplete; unrelated
provider/browser/deployment opt-ins are not claimed. Download has no
total/cumulative elapsed deadline or retry budget.


## T09 tenth batch - bounded original provenance JSONL syntax (2026-10-14)

The service adds a strict JSON object-per-line observer on an owned provenance
snapshot, with explicit positive finite original byte, raw line byte (including
LF/CRLF terminator) and record capacities. It accepts LF/CRLF and an unterminated
final object. Empty input, blank lines, BOM, duplicate keys, ambiguous Unicode,
non-object roots and malformed later records return no report. The fixed32KiB
reader and one preallocated bounded raw line retain only one parsed record at a
time; existing per-object strict JSON depth/string/member/binary64 rules remain.
Limits describe raw capacities, not an exact total heap/preparation budget.
Original whole-source size/SHA and count are returned without normalization.
Cancellation is checked between bounded reads and before returning; a bounded
strict parse is synchronous and cannot be interrupted inside its library call.

InspectProvenanceJSONL composes this syntax with existing current signed package,
actual full archive/native byte presence, exact paired evidence, supported SBOM
Schema and primary installer SHA-256 binding. Every failure returns zero package,
no evidence pair and zero JSONL report. Existing APIs retain their acceptance.
No in-toto/DSSE/SLSA version/profile, provenance signature, subject/source/pipeline
or allow-list validation is implied. Synthetic object payloads deliberately prove
syntax only; the unresolved statement/attestation profile remains deferred.

Root grammar3=1.653s and complete verification17=11.813s passed. Grammar controls
include raw CRLF/LF/no-finalLF identities, a fragmented line over1MiB, malformed
later records, duplicate keys, Unicode/shape/depth/string rejection, independent
exact/below capacities, empty/invalid snapshot, cancellation and restored positives.
Signed-composition negatives first pass the same complete SBOM binding observer,
then fail precise JSONL syntax with zero earlier facts; capacities/current
deny/restore also pass. The actual protected PostgreSQL2 groups passed13.857s
(6.31s/5.26s), including original raw/count/current-read retention, signed-binding
positive before duplicate-key syntax refusal, restore and current deny/restore.
The fixture helper takes explicit provenance bytes and signs their actual size/
SHA; old cases pass the original unchanged bytes. Owner SQL mutations remain
adversarial read-current controls, not approved deny or admission commands.
Private connections were kept in process environment and were not printed.

Root full default Go suite passed, including grammar3=5.928s/
verification17=16.754s and all existing regressions. Full vet/module verification/
server build/pinned contracts/changed-source format/diff passed. Client
architecture/version/diff passed. Dependency independently passed grammar3=1.647s/
verification17=10.949s and actual PostgreSQL2 groups14.187s (6.90s/5.11s),
affected vet/format/diff and complete scoped review. Compatibility independently
passed grammar3=1.680s/verification17=14.914s and actual PostgreSQL2 groups13.407s
(6.25s/4.85s), affected vet/format/diff and complete scoped review.
Both frozen independent code/test reviews found no mandatory issue; all processes
are closed and credentials were not printed. Both independent final documentation reviews passed before commit.
No production SQL/provider/dependency/protocol/application version or deployment
changes. Full T09 remains incomplete; unrelated provider/browser/deployment
opt-ins are not claimed. Download has no total/cumulative elapsed deadline or
retry budget.


## T09 eleventh batch - executable provenance JSONL observation (2026-10-14)

The isolated cmd/upgrade-provenance-inspect accepts one actual source file and
requires explicit positive source bytes, owned snapshot memory, raw line bytes
(including terminator) and record capacities. It opens through the existing OS
source adapter, captures exact bytes with EOF confirmation and runs the unchanged
bounded object-per-line observer. Success emits only original source size/SHA
and record count; fixed failures emit no successful report or argument/provider/
payload data. Interrupt cancellation is passed to capture and syntax inspection.
The command writes no file and performs no network/provider/database operation.

This is a complete isolated syntax workflow, not Manifest/Root verification,
in-toto/DSSE/SLSA schema/signature/source/pipeline, immutable storage or Artifact
admission. Windows uses the existing actual local handle checks. Unix regular
descriptors do not attest filesystem locality; unchanged source-provider limits
remain. Unavailable symlink privileges and an actual mapped-remote-source opt-in
remain explicitly unverified; no elevation or external share is created.

Root built the actual command and passed command2+childE2E7.542s (child6.32s),
grammar3=1.463s and source adapter1.468s with the two documented unavailable
source scenarios skipped. Actual source original CRLF/no-finalLF raw identity
and two-record count, all missing/zero/insufficient independent capacities,
invalid flag, missing path, directory, cancellation, nil context, duplicate-key
later record, fixed redacted refusals and restored positives passed. The actual
child executes positive, duplicate-key refusal and restored original positive.
Its30s test-only process allowance is not a product/download deadline.

Root full default Go tests passed, including default command1=12.212s,
grammar3=5.996s/source5.961s/verification17=16.346s and all existing regressions;
full vet/module verification/server build/pinned contracts/changed-source format/
diff passed. Client architecture/version/diff passed.
Dependency independently rebuilt a private unique binary and passed command2+
actual childE2E3.790s (child1.61s), grammar3=2.260s/source2.260s and affected
vet/format/diff. Compatibility independently rebuilt a private unique binary and
passed command2+actual childE2E7.256s (child1.53s), grammar3=1.667s/source1.666s
and affected vet/format/diff. Both retained the two explicitly unverified source
scenarios. A multiline documentation example was repaired to a complete single
command and both reviewers confirmed that fix. Both independent frozen scoped
code/test reviews passed; all processes are closed. Both independent final documentation reviews passed before commit. No SQL/provider/dependency/protocol/application version or deployment
changes. Full T09 remains incomplete; unchanged DB/provider/browser/deployment
opt-ins are not claimed. Download retains no total/cumulative elapsed deadline
or retry budget.


## T09 twelfth batch - exact owned native member candidates (2026-10-14)

InspectNativeBytes accepts a closed signed role selection: finalInstaller,
helper, launcher or helperArtifact with its exact helper ID. Helper IDs on other
roles and caller path/unknown role selectors fail. Existing current signed
Manifest verification and independently owned exact FullPack run first. Only
exact size/SHA direct FullPack inventory members can supply the chosen signed
five-field identity. The returned owned snapshot has independent reads and no
host path; candidate byte aliases retain the same identity. All resource checks
remain applicable. Any failure returns zero Facts and no candidate.

The initial four native groups passed4.084s, including all roles/five fields,
caller/read-buffer mutation isolation, independent exact/below FullPack/member
capture capacities, cancellation/current deny/restore and actual33MiB native
members under Store/Deflate. This file size is not the32MiB atomic business limit.
Initial real PostgreSQL2 groups passed20.032s (12.15s/5.69s), adding all four
selected byte candidates against the actual protected SERIALIZABLE source, raw
size/SHA and signed/current facts, plus current deny/restore. Owner SQL changes
remain adversarial controls, not approved deny/admission commands.

Both independent static reviews found the same P2: the initial byte selector
called the existing non-container OuterFile on the first matching identity, so
a valid ZIP direct candidate or ZIP-first equal alias was wrongly refused.
A new regression first passed old InspectFullPack on that same actual signed
upload, then reproduced capture-input-invalid (1.491s). The root repaired this
with a narrow InspectOuterMember/OuterMember port accepting any safe direct
logical member, including prefixed ZIP. Complete recursive scanning and retained
ZIP capture live-nested-memory accounting remain; existing non-container and
bare-root selector APIs keep their acceptance.

ZIP-only and both ZIP/non-ZIP alias orders now retain exact raw ZIP bytes and
all five signed fields. Nested-only identity remains refused by old FullPack and
the new candidate port. Archive controls first pass uncaptured sibling inspection
under lower live memory, then refuse retained capture and restore exact aggregate
capacity; corrupt later/selected ZIP, nested decoy, unsafe name, cancellation and
old selector compatibility pass. Owned prefixed ZIP Store/Deflate, exact raw
identity, old OuterFile refusal and independent memory/ctx controls pass.
After repair, complete archive=1.235s/extraction=2.858s/verification22=17.656s
passed. Repaired actual PostgreSQL2 groups passed16.361s (8.21s/5.83s).
Root full default Go suite then passed, including archive5.142s/extraction6.130s/
verification22=22.457s and all existing regressions. Full vet/module verification/
server build/pinned contracts/changed-source format/diff passed; client
architecture/version/diff passed. Both independent second static scoped reviews
closed the P2 and found no additional mandatory issue. Independent frozen
execution also passed: dependency archive2.109s/extraction2.990s/
verification22=14.251s and actual PostgreSQL2 groups15.412s (7.44s/5.69s);
compatibility archive1.350s/extraction2.827s/verification22=14.888s and actual
PostgreSQL2 groups15.523s (7.83s/4.67s). Both passed affected vet/format/diff
and the complete repaired scoped code/test review. All processes are closed
and private credentials were not printed. Both independent final documentation reviews passed before commit.

These are signed byte candidates, including intentionally non-native ZIP
fixtures, not OS format/identity/publisher authenticity, protected staging,
writer retirement, execution authority or Artifact admission. FullPack/member/
source/inventory/live nested buffers have separate finite capacities; callers
must establish aggregate memory/preparation policy. No production defaults,
SQL/provider/dependency/protocol/application version or deployment changes.
Full T09 remains incomplete; download has no total/cumulative elapsed deadline
or retry budget.


## T09 thirteenth batch - bounded native AR physical spans (2026-10-14)

The isolated artifacts/platform/ar adapter observes the physical short-name
SysV/GNU AR container used by Debian packaging. It validates original magic,
fixed header/trailer and decimal/octal fields, every size/offset and odd-length
newline padding, then hashes each exact member with bounded32KiB reads from an
owned snapshot. Explicit positive original byte and member-count capacities are
required. Header names/modes remain declarations, never host paths/permissions.
Duplicate names retain distinct spans; empty physical AR and empty ordinary
members are valid syntax. Long names, symbol/name tables, thin archives and path
name variants return explicit unsupported, without fetching external data.
Every error/cancellation returns zero source and no partial member facts.

This is container observation, not Debian member order/version/control/data
validation, package/app version/architecture, helper/launcher format, publisher,
native authentication or Artifact admission. The returned whole original source
identity remains exact; no member or header is reencoded. It writes no file and
performs no network/provider/database operations. No existing API changes.

Root initial three grammar groups passed1.740s. After narrowing BSD symbol-table
recognition to the explicit reserved names, the same grammar groups passed0.923s.
Controls include independent raw source/member identities and actual spans,
ordinary GNU trailing slash, duplicate and zero members, >32KiB body, odd/even
padding, corrupt later/truncated/oversized frame, numeric field/trailer/name/pad
errors, unsupported variants, exact/below capacities, nil/unowned/context/invalid
limits and restored positives. Cancellation is polled between bounded memory
reads and before success; no synchronous finite header operation is claimed to
be individually interruptible.

An opt-in real LLVM/GNU AR subprocess test is provided, using only isolated
short-name test files and explicitly selected tool. No such executable was found
in PATH or the checked existing toolchain/install locations; actual tool-produced
E2E is explicitly skipped and no real Debian/ELF/native package certification is
claimed. Its30s test allowance is not a product/download deadline.

The first full default Go/vet/module verification/server build/pinned contracts/
format/diff passed (AR3=6.384s), before the scoped review found a P2: checking BSD
reserved symbol names before removing the optional GNU trailing slash treated
__.SYMDEF/ and __.SYMDEF_64/ as ordinary members. A single-variable regression
reproduced success where Unsupported was required (1.192s). The root moved the
exact reserved-name check after normalization; ordinary similar prefixes stay
supported. The four repaired grammar groups passed0.969s with real-tool opt-in
still explicitly skipped. Repaired full default Go suite passed (AR4=6.032s/
verification22=21.121s plus all existing regressions), full vet/module verification/
server build/pinned contracts/format/diff and client architecture/version/diff
passed. Both second static scoped reviews closed the P2. Dependency independently
passed AR4=1.652s and affected vet/format/diff; compatibility independently passed
AR4=1.129s and affected vet/format/diff. Both explicitly skipped real-tool opt-in
without installing tools and completed repaired scoped code/test review with no
remaining mandatory finding. All processes are closed. Both independent final documentation reviews passed before commit; unchanged DB/provider/browser/CLI opt-ins are not claimed.

No SQL/provider/dependency/protocol/application version or deployment changes.
Full T09 remains incomplete. Download retains no total/cumulative elapsed
deadline or retry budget.


## T09 fourteenth batch - Debian outer required-member framing (2026-10-14)

The isolated artifacts/platform/debian adapter follows the public deb(5)
outer-format contract:
https://manpages.debian.org/bookworm/dpkg-dev/deb.5.en.html.
Existing complete physical AR validation runs first, including every extension.
The first member must be debian-binary with a bounded newline-terminated ASCII
major.minor first record; major2 is supported, minor revisions and additional
version-record lines are ignored. Required control then data members accept the
documented compression labels (control plain/gz/xz/zst; data additionally
bz2/lzma). Underscore extensions before required members and arbitrary members
after required data are retained separately. Exact selected control/data spans
cannot be replaced by later same-name decoys. Every failure returns zero report.
No compression/tar/control metadata, app version, architecture, publisher or
native authenticity is verified by this observer.

AR source/count limits and separate positive version-record raw-memory capacity
are explicit. Version reads use bounded32KiB chunks with cancellation polling
and final polling. The complete original source and selected member identities
are retained, with no reencoding or file writes. Limits do not prescribe
aggregate heap/preparation or production policy.

Root initial synthetic grammar3 passed1.208s with real-package opt-in skipped.
All24 control/data compression-label combinations, future minor/extra lines,
before/between/after extensions, post-data control decoy, malformed/unsupported
version/order/unknown required member, missing/later-corrupt physical frames,
exact/below version capacity and nil/context/source controls passed.

For actual format evidence, a public hello2.10-3 amd64 package was downloaded
from the public Debian HTTPS repository into a private temporary fixture; it was
only read, with no package installation or execution. Size53080 and observed
SHA-2562e6e2f1a0007dc43bc91c273fd36e91e40a4f1c2765a03eca68b70a42103878a
were retained independently. Real-package opt-in plus three grammar groups passed
1.052s; physical AR regression4=1.220s passed with real-AR-tool opt-in still
explicitly skipped. The actual-file test was then bounded before allocation
and limited during reading; rebuilt root actual Debian4 passed1.086s.
Root full default Go tests passed (Debian3=5.989s/AR4=6.050s/
verification22=20.422s and all existing regressions), full vet/module verification/
server build/pinned contracts/changed-source format/diff and client
architecture/version/diff passed. Dependency independently passed Debian4 with
actual existing fixture1.166s/AR4=1.116s and affected vet/format/diff; compatibility
independently passed Debian4 with actual existing fixture1.140s/AR4=1.081s and
affected vet/format/diff. Both only read the existing public fixture and separately
confirmed exact size/observed SHA; real-AR-tool opt-in remains explicitly skipped.
Both frozen scoped code/test reviews found no mandatory issue and all processes
are closed. Both independent final documentation reviews passed before commit.
This is actual outer-framing evidence, not repository release-signature,
package publisher, full tar/control/native installation or Pinvou certification.

No SQL/provider/dependency/protocol/application version or deployment changes.
Existing APIs are unchanged; full T09 remains incomplete. Downloads retain no
total/cumulative elapsed deadline or retry budget.


## T09 fifteenth batch - current signed Debian installer composition (2026-10-14)

InspectDebianInstaller combines the existing current signed Manifest, fully
inspected owned FullPack and exact final-installer member snapshot with observed
physical AR/Debian outer framing. It requires the signed installer format deb
and confirms the framing source is the same observed native byte identity.
Non-deb declarations return explicit unsupported; every failure returns zero
package/native/framing result, even after earlier byte inspection succeeded.
Existing APIs and their byte-only acceptance stay unchanged.

Root new four groups including actual public-file composition passed2.862s.
Strong signed synthetic framed bytes preserve original raw identity/current reads,
major2 minor compatibility and exact selected bytes. Non-AR, unsupported major
and malformed version negatives first pass the same old signed-native byte stage,
then fail precise framing without earlier results and restore supported positives.
Independent version capacity, cancellation/current deny/restore pass. A properly
scoped strong Windows declaration first passes its independent byte observer
before this Debian observer returns unsupported; no OS execution is involved.
The real public Debian fixture is bounded before/during test reading and wrapped
in a synthetic signed Package with exact native/full-package hashes. The
synthetic Pinvou app/version/publisher claims deliberately do not certify that
public package's app identity/version/architecture/publisher.

Actual protected PostgreSQL2 groups passed12.333s (6.35s/4.46s). Additional strong
signed owned upload contains observed framed native bytes through the actual
SERIALIZABLE runtime source. Its exact original native size/SHA and current
Root read-set facts are retained. The original byte-only non-AR installer first
passes its existing observer, then framing refuses; a supported framed positive
is restored. Current canonical indexed package deny refuses the same previously
accepted framed source and exact genesis restoration restores it. Owner SQL
changes remain adversarial read-current controls, not approved commands/admission;
private connection values were kept only in child process environment.

Root complete default Go tests passed (verification25 groups26.290s, public
fixture opt-in skipped); module verification, whole-repository vet, server build,
pinned contract check, changed-file formatting and diff checks passed. Client
architecture and version guards passed; no client application code changed.
Dependency independent verification26 including the public fixture passed13.948s
and actual PostgreSQL2 passed14.240s (7.18s/4.78s). Compatibility independent
verification26 passed18.858s and actual PostgreSQL2 passed12.965s (6.29s/4.49s).
Both scoped reviews found no required code/test repair; both final documentation
reviews passed. No compression/tar/control
metadata, app identity/version/architecture, publisher/native trust, protected
storage, execution rights or Artifact admission are implied. Full T09 is
incomplete; no SQL/provider/dependency/protocol/application version/deployment
changes. Download has no total/cumulative elapsed deadline or retry budget.


## T09 sixteenth batch: exact stored Debian member snapshots

Added a separate Debian platform API that validates complete package framing,
then captures the selected control/data stored sequences from the same immutable
source. Each owned child has its original size/SHA-256 checked against the
selected AR member. Independent caller-supplied source and memory capacities
bound both captures. Any failure returns a zero report and no partial pair;
existing framing and signed-installer observers retain their acceptance.

Root seven Debian groups, including both opt-in observations of the existing
public package, passed1.025s. Synthetic stored bytes verify independent child
identity, later duplicate-member isolation, caller buffer isolation, all four
one-below capture capacities, missing limits, cancellation and restored positive.
The public fixture verifies exact original selected stored bytes without
installation or execution. Root full default Go tests passed (Debian5 groups,
both public-fixture opt-ins skipped,6.038s; verification25 groups21.695s).
Module verification, whole-repository vet, server build, pinned contract check,
changed-file formatting and diff checks passed. Client architecture and version
guards passed. Dependency independent Debian7 including both public-file cases
passed1.493s; affected vet, formatting and diff passed. Compatibility independent
Debian7 passed1.735s; affected vet, formatting and diff passed. All root and
independent test/tool processes closed. Both final documentation reviews passed.

This API does not decompress members, inspect tar/control metadata or scripts,
certify package app/version/architecture/publisher, grant execution, admit an
Artifact or define production storage/aggregate preparation policy. Full T09
remains incomplete. No dependency/provider/SQL/protocol/application version or
deployment changes. Download has no cumulative deadline or retry budget.


## T09 seventeenth batch: bounded gzip decoding of owned bytes

Added an explicit gzip platform adapter for immutable stored snapshots. It checks
stored-source capacity, validates all concatenated members/checksums/trailers and
complete EOF in a fixed-buffer first pass, then captures the exact expanded length
from the same source in a second pass. Caller-supplied expanded source and memory
capacities bound snapshot bytes; fixed decoder workspace is separate. No duplicate
expanded buffer is accumulated, no path is reopened, and every failure returns no
snapshot. Snapshot byte capacity is not a total heap budget. Cancellation is
checked through bounded source reads and both passes; synchronous decoder work
between these checks cannot be interrupted inside the SDK.
No cumulative download deadline or retry budget is introduced.

Root four groups including the independently generated Python gzip fixture
passed1.307s. Tests cover concatenation/empty members, exact raw identity and
owned-buffer isolation, independent capacity failures, checksum/size trailer/
truncation/later-member/trailing corruption, cancellation and restored positive.
An actual33 MiB expanded result passes with its explicit artifact capacity;
business batch32 MiB is not an artifact limit. The frozen independent fixture is
278 stored bytes (SHA-256 809c54939b6324cda731e7e9a3eff6c402e25c90b878ba46fe9720a423d48b48)
and65580 expanded bytes (SHA-256 e64dac4b42e2d73604d318be7ca45eb0f4d0a15e2daaefba00d7fc156ff6ae28).
The fixture was generated separately by Python, not by this Go decoder.
Initial complete Go checks passed (default compression3 groups6.800s,
Python opt-in skipped; verification25 groups17.407s), module verification, vet,
server build, pinned contracts, formatting/diff and client guards passed.
Dependency static review found that Go gzip accepts RFC1952 reserved header bits.
A single-bit root negative reproduced the issue (FAIL0.895s). The repair keeps a
shared ByteReader and explicitly validates each member header before a
non-multistream SDK reader checks its body/trailer; no following header is lost
to SDK read-ahead. Root repaired five groups including Python passed3.429s,
with all three reserved bits refused in first/later members and lawful
concatenation restored. Root final six groups including Python passed5.987s:
all legal FEXTRA/FNAME/FCOMMENT/FHCRC bits stay compatible; single-variable first/
later header CRC corruption refuses and exact lawful concatenation restores.
Final full default Go tests passed (compression5 groups, Python opt-in skipped,
6.627s; verification25 groups33.169s). Module verification, whole-repository vet,
server build, pinned contracts, formatting/diff and client architecture/version
guards passed. Dependency independent repaired six groups including Python
passed1.950s (33 MiB .34s); affected vet, formatting and diff passed, all processes
closed. Compatibility independent six groups passed1.685s (33 MiB .33s); final
checks (affected vet, formatting and diff) passed; all processes closed.
Both final documentation reviews passed.

This adapter does not interpret tar/control metadata, names or scripts, certify
app/version/architecture/publisher, provide native authentication, admit Artifacts
or select a production preparation policy. Debian member dispatch and other codecs
remain separate work. Full T09 remains incomplete. No dependency/provider/SQL/
protocol/application version/deployment changes or package execution.


## T09 eighteenth batch: complete Debian plain/gzip member decoding

Added a Debian platform composition that first observes complete framing and
captures both exact selected stored byte sequences, then dispatches plain/gzip
formats to independent expanded snapshots. Stored and expanded capacities stay
separate for control/data. Plain members reuse already immutable owned snapshots
while checking expanded capacity; gzip uses the complete per-member decoder.
Stored AR identities remain distinct from expanded snapshot identities. Other
recognized compression labels return explicit unsupported, never a guessed
fallback. Any failure returns zero framing facts and neither decoded member.

Root ten Debian groups including two existing public-package byte observations
and the new independently generated Python gzip-package observation passed1.459s.
Tests cover four plain/gzip combinations, expanded/stored identity separation,
later decoy isolation, empty streams, caller buffer isolation, four one-below
expanded limits, missing limits, cancellation and exact supported restoration.
A corrupted data checksum first passes the existing stored-member observer then
fails decoding without earlier control; xz framing first passes the old observer
then returns unsupported at the explicit decoder boundary. Plain data also
refuses insufficient expanded memory. Root full default Go tests passed:
Debian7 groups (all three actual-fixture opt-ins skipped)3.342s, verification25
groups32.190s. Module verification, whole-repository vet, server build, pinned
contracts, changed-file formatting and diff passed. Client architecture and
version guards passed. Dependency independent Debian10 including all three
actual cases passed1.418s; compatibility independent Debian10 passed1.513s.
Both affected vet, formatting and diff checks passed; all test/tool processes
closed. Both final documentation reviews passed.

The independent Python USTAR/gzip/AR fixture has520 stored bytes with SHA-256
0c7bfea20dbd6983f7b142f3f693ca028095264ba1e8a93ec2e64f2545cfc7c7.
Its control/data expanded sequences are10240 bytes each; their original SHA-256
values are checked without tar interpretation or package execution. This is an
isolated format fixture, not authenticated application provenance or identity.

No tar/control metadata/script execution, app/version/architecture/publisher
certification, native trust, Artifact admission, production storage/heap/preparation
policy or support for other codecs is implied. Full T09 remains incomplete;
no dependency/provider/SQL/protocol/application version/deployment changes.
Download has no cumulative deadline or retry budget.


## T09 nineteenth batch: bounded bzip2 decoding and shared snapshot capture

Moved common immutable-source, exact-length/EOF, explicit-capacity, cancellation
and owned capture logic to a private compression helper. Codec factories remain
closed implementation details, not caller-supplied validation. The gzip public
API and complete per-member reserved-flag/CRC/trailer acceptance remain intact.
Added a bzip2 adapter using the Go SDK ByteReader path to validate concatenated
streams, block/file CRC and complete source EOF. Bzip2 header levels bound SDK
block workspace to the format maximum (900000 uint32 entries plus decoder
bookkeeping); this workspace is
separate from caller-selected expanded snapshot capacities and total heap policy.
All failures return no expanded snapshot; no cumulative time/retry limit is used.

Root initial compression9 groups, including the existing Python gzip opt-in, passed3.194s
(actual33 MiB bzip2 .48s and gzip .25s). Root Debian default7 groups passed2.360s
with three actual fixture opt-ins skipped, then actual Debian10 groups passed1.545s.
Three new bzip2 groups use frozen independently Python-generated sequences:
concatenated level1/empty/level9 streams, original expanded identity/buffer
isolation, three independent one-below capacities, invalid header level, block
CRC, truncation, later-stream and trailing corruption, cancellation and exact
supported restoration. A separately frozen50-byte sequence expands to33 MiB with
its explicit artifact capacity; business batch32 MiB is not a file limit.
The main195-byte stored vector has SHA-256
c1bf0d06f82ef209efe1059248fd8fde3ef7fd9e7cdcaa62743475560e276e3f;
its67629 expanded bytes have SHA-256
9c9e65f9bd6f3adea2404464bfc79f5ce667607bf87d07a01f5ee907b22541fb.
Initial complete default Go tests passed (compression8 groups, Python opt-in
skipped,6.976s; verification25 groups27.044s), module verification, whole-repository
vet, server build, pinned contracts, formatting/diff and client guards passed.
Compatibility static review identified a critical verification gap: block CRC
negatives did not independently exercise terminal file/combined CRC failure.
The root added first/later single-bit file CRC controls after complete payload.
Each original stream first passes; the test verifies its exact end marker and
independently observes all expected SDK output followed specifically by file
checksum mismatch. The adapter returns Bzip2Invalid and no snapshot, then the
same original owned source restores success. Root final compression10 including
Python passed1.855s (33 MiB bzip2 .50s/gzip .25s). Final complete default Go
tests passed (compression9 groups, Python opt-in skipped,7.574s; verification25
groups18.945s). Module verification, whole-repository vet, server build, pinned
contracts, formatting/diff and client architecture/version guards passed.
Dependency independent compression10 including Python passed5.331s (33 MiB
bzip2 .80s/gzip .30s), Debian10 including all three actual cases passed3.348s;
affected vet, formatting and diff passed, all processes closed. Compatibility
independent compression10 passed7.605s (33 MiB bzip2 .58s/gzip .35s), actual
Debian10 passed2.123s; affected vet, formatting and diff passed, all processes
closed. Both final documentation reviews passed.

The adapter does not yet add Debian bzip2 dispatch, interpret tar/control data,
certify app/version/architecture/publisher/native trust, grant execution or admit
Artifacts. Synchronous SDK work between read polls is not internally interruptible;
snapshot byte limits are not a total heap or preparation policy. No dependencies,
provider/SQL/protocol/application version/deployment changes. Full T09 remains
incomplete; real application/native certification is not supplied by these vectors.


## T09 twentieth batch: Debian bzip2 data-member decoding

Extended the explicit Debian member dispatcher to data.tar.bz2, using the
complete bounded SDK bzip2 adapter. Control decoding retains its existing plain/
gzip paths; other recognized codecs still return unsupported. Stored and
expanded control/data identity/capacity boundaries and every failed zero-report/
nil-pair result remain intact; no caller-selected path or guessed encoding.

Root Debian13 groups including all four actual-fixture opt-ins passed1.432s.
New controls exercise both plain/gzip control with independent concatenated
Python bzip2 data, later same-name decoys, owned buffer isolation, separate data
source/memory capacities, and post-payload final file CRC failure. The corrupted
stored sequence first passes the old byte observer; decoded data then refuses
with exact Bzip2Invalid and neither earlier control nor framing. The same original
supported source restores exact expanded identity.

An independent Python USTAR/AR package with plain control and bzip2 data is only
read, never installed/executed. Its10570 original bytes have SHA-256
f54ccbd8c327a98960af0562ba465c25a184c4a27dd06c42409ae7570fac78a3;
control/data expand to10240 bytes each, with the same frozen independent tar byte
identities as the previous Python gzip fixture. No tar or control interpretation
is claimed. Root complete default Go tests passed with package parallelism2:
Debian9 groups (four actual opt-ins skipped)2.063s, compression9 groups4.383s,
verification25 groups18.547s. Module verification, whole-repository vet, server
build, pinned contracts, changed-file formatting/diff and client architecture/
version guards passed. Dependency independent actual Debian13 passed3.778s,
compression10 including Python passed7.765s (33 MiB bzip2 .61s/gzip .41s);
affected vet, formatting and diff passed, all processes closed. Compatibility
independent actual Debian13 passed3.007s, compression10 including Python
passed4.151s (33 MiB bzip2 .73s/gzip .40s); affected vet, formatting and diff
passed, all processes closed. Both final documentation reviews passed.

No app/version/architecture/publisher/native authenticity, execution rights or
Artifact admission is supplied. Other codecs, tar/control validation, protected
storage and aggregate preparation policy remain separate unfinished work.
No dependency/provider/SQL/protocol/application version/deployment changes.
Full T09 stays incomplete; download has no cumulative deadline or retry budget.


## T09 twenty-first batch: current signed Debian member decoding

Added a separate signed-inspection composition that retains current Manifest,
FullPack, native selected bytes and Debian framing, then decodes control/data
from that same owned native snapshot. The framing source identity is rechecked
against the earlier native observation, and original signed/read-set facts remain
paired with independent expanded snapshots. Any later refusal returns zero
installer/package/framing facts and no partially decoded members. Existing
byte-only and outer-framing observers keep their acceptance.

Root new three groups including the existing independent Python bzip2 fixture
passed1.756s. Strong signed synthetic gzip data preserves exact native and expanded
identity/current reads. A checksum-negative first passes the old signed framing
observer, then refuses complete decoding without earlier signed facts; independent
expanded memory refusal, restored positive, current package deny and cancellation
with original-source restoration pass. The real independent Python format fixture
is wrapped with exact strong synthetic signed package hashes; its control/data
expanded10240-byte identities match their frozen independent originals. Synthetic
Pinvou claims do not authenticate the fixture's native app/version/publisher.

Actual protected PostgreSQL2 groups passed13.202s (6.70s/4.35s). The added plain
member composition retains exact native/control/data bytes and Root read revision2
through the actual SERIALIZABLE runtime source. A one-below expanded data-memory
limit refuses after earlier signed/frame success with no earlier facts, then the
same original source restores. The actual canonical indexed package deny refuses
the previously accepted source; exact original genesis restoration restores it.
Owner SQL mutations are adversarial current-read controls, not approved commands
or Artifact admission. Private connection values stayed only in child environment.
Root actual verification29 including both existing fixture opt-ins passed14.403s.
Complete default Go tests passed with package parallelism2 (verification27 groups,
both fixture opt-ins skipped,14.404s); module verification, whole-repository vet,
server build, pinned contracts, changed-file formatting/diff and client
architecture/version guards passed. Dependency independent verification29
including both fixtures passed14.048s; actual PostgreSQL2 passed14.359s
(6.05s/6.20s). Affected vet, formatting and diff passed, all processes closed.
Compatibility independent verification29 including both fixtures passed14.095s;
actual PostgreSQL2 passed12.566s (5.94s/4.45s). Affected vet, formatting and diff
passed, all processes closed. Both final documentation reviews passed.

This composition does not interpret tar/control metadata, authenticate native app/
architecture/publisher, grant execution or admit Artifacts. Other codec support,
protected storage and aggregate preparation policy remain unfinished. No provider/
dependency/SQL migration/protocol/application version/deployment changes. Full T09
remains incomplete; downloads have no cumulative elapsed or retry limit.


## T09 twenty-second batch: explicit bounded LZMA-alone decoding

Added a separate single-stream LZMA-alone adapter. It requires positive original/
expanded capacities and an explicit dictionary bound (at least4096 bytes, within
host-int/format limits); no large library default is used. Immutable13-byte header
preflight refuses excessive dictionaries and known expanded lengths before decoder
allocation. Fixed properties are validated. Complete decoded EOF must also prove
original stored EOF through the same buffered ByteReader; trailing or concatenated
input returns no snapshot. Shared private two-pass capture keeps exact expanded
identity and no duplicate expanded buffer. Cancellation is checked between bounded
reads; synchronous decoder work itself is not internally interruptible.

Adopted pure-Go github.com/ulikunitz/xz v0.5.17 only for its classic LZMA reader,
whose configured dictionary capacity is a hard maximum. The XZ reader's dictionary
parameter can be increased by its header and is not used. Only one direct dependency
and its two module checksums changed; existing versions stayed fixed. The original
BSD license is retained in compression/third_party/ulikunitz-xz.LICENSE for binary
redistribution materials. No private executable or external decoder is launched.

Private independent Python/LZMA evaluation confirmed unknown and known lengths,
full source EOF, dictionary refusal, truncated refusal and observable unread tail.
Root compression14 groups including existing Python gzip passed7.541s (33 MiB
LZMA .68s/bzip2 .45s/gzip .24s). Four new groups use frozen independent Python
vectors to check known/unknown length, empty stream, owned-buffer isolation,
separate source/expanded/dictionary limits, property/truncation/tail/concatenation
refusal, context failure and restored originals. A4970-byte vector expands to33 MiB
under explicit artifact capacity, unrelated to business-batch32 MiB.
The normal138-byte vector has SHA-256
08ad9a7f38d1a361460f846d644aaaa2aa7ef73b45b665711ea5a6be9ed17932;
its77862 expanded bytes have SHA-256
438a9f2d9e86aa2530b3f1bbaf231ddd76bc1a58537436621193d11371dcf9b2.
Both independent static reviews found a32-bit allocation boundary defect: the
library allocates dictionary capacity plus one sentinel byte. The extracted
nonallocating32/64-bit boundary regression first failed2.274s against the original
predicate. The repaired host bound reserves that byte while keeping the64-bit
format maximum unchanged. Root compression15 groups including the existing
Python gzip fixture passed8.941s (33 MiB LZMA1.18s/bzip2 .70s/gzip .35s).
Complete default Go tests passed with package parallelism2 (compression14
mandatory groups plus one fixture skip,7.924s; verification27 groups plus two
fixture skips,37.216s). Module verification, whole-repository vet, server build,
pinned contract bytes, changed-file formatting/diff and client architecture/version
guards passed; all root processes closed. Dependency independent compression15
including Python gzip passed8.314s (33 MiB LZMA 1.51s/bzip2 1.44s/gzip .56s).
Affected vet, formatting/diff, module verification and retained upstream license
byte comparison passed; all processes closed. Compatibility independent
compression15 including Python gzip passed6.136s (33 MiB LZMA 1.95s/bzip2 1.10s/
gzip .45s); affected vet, formatting/diff, module verification and license byte
comparison passed, all processes closed. Both independent scoped audits passed;
both final documentation consistency reviews passed.

LZMA-alone has no checksum or magic signature: observed bytes are not native
identity or authentication. Probability-model/fixed workspace is separate from
snapshot/dictionary bounds and is not a total heap/preparation certification.
No Debian LZMA dispatch, tar/control interpretation, publisher/app certification,
execution rights, Artifact admission, production storage/provider/protocol/SQL
migration/application version/deployment change is implied. Full T09 remains
incomplete; downloads retain no cumulative elapsed or retry limit.


## T09 twenty-third batch: Debian LZMA data-member dispatch

Debian decoded-member inspection now dispatches only data.tar.lzma to the
previous bounded classic LZMA-alone adapter. DecodeLimits adds an explicit
LZMADictionaryBytes field required only for that codec; old plain/gzip/bzip2
callers retain their existing zero-value behavior. Stored and expanded capacities
remain independent; selected original member identities remain in Report.
Failures after earlier control success return neither report nor decoded pair.

The frozen independent Python package has10588 bytes, SHA-256
2aa4a71a063e4e826aedb0fd71f668738e41914ccdc33816ec7edbac35b743d8.
Its data LZMA stream uses a4096-byte dictionary and155 stored bytes; control/data
expanded10240-byte identities match the independently generated prior tar sources.
No package was installed or executed. Inline mandatory controls exercise mixed
plain/gzip control, exact owned bytes, expanded source/memory limits, missing or
excessive explicit dictionary, raw tail rejection after old stored observation
succeeds, and restoration of the original immutable package.

Root Debian16 groups including five existing/new independent package opt-ins
passed2.970s. A new actual current signed composition case wraps the same frozen
LZMA installer with exact strong synthetic hashes. It checks original/expanded
identities, missing dictionary refusal with zero earlier facts, and restoration.
Synthetic Pinvou claims do not attest its native app/version/architecture/publisher.
Root actual verification30 including all three existing/new package fixture
opt-ins passed50.996s. Complete default Go tests passed with package parallelism2
(Debian11 mandatory groups plus five fixture skips,1.167s; compression14 groups plus
one skip,3.360s; verification27 groups plus three skips,16.311s). Module verification,
whole-repository vet, server build, pinned contract bytes, changed-file formatting/
diff and client architecture/version guards passed; all root processes closed.
Both independent static audits passed. Dependency independent Debian16 including
all five package opt-ins passed1.254s; verification30 including all three package
fixture opt-ins passed15.274s. Affected vet, formatting and diff passed; all
processes closed. Compatibility independent Debian16 including all five package
opt-ins passed1.549s; verification30 including all three package fixture opt-ins
passed84.945s. Affected vet, formatting and diff passed; all processes closed.
Both independent scoped audits and final documentation reviews passed.

This observes expanded byte sequences only; tar/control interpretation, native identity/publisher certification,
XZ/Zstandard, Artifact admission and protected storage remain unfinished. No new
dependency, SQL/protocol/application version or deployment change. Full T09 remains
incomplete; downloads have no cumulative elapsed or retry limit.


## T09 twenty-fourth batch: bounded XZ container and LZMA2 chunk inspection

Added a complete-stream XZ adapter with explicit stored/expanded byte capacities,
dictionary capacity and total block/stream counts. It supports a single LZMA2
filter per block and None/CRC32/CRC64/SHA256 checks, multiple blocks/streams, empty
streams and aligned zero padding. Each actually reached block header is checked
before fixed dictionary allocation through the existing pinned Reader2 adapter;
the top-level library XZ dictionary parameter is a minimum and is not used.
Header/block CRC, reserved flags, canonical VLI, dictionary properties, optional
declared versus observed sizes, actual EOS, block checks, ordered Index tuples,
Index CRC, footer flags/backward size/magic and original EOF all remain binding.
Unknown filter/check families return explicit Unsupported. Index tuples use a
constant-workspace SHA-256 commitment to their canonical ordered encodings,
rather than trusting Index offsets or allocating a caller-sized record list.
Shared closed codec error classification preserves Limit/Unsupported while old
codec structural errors retain their existing fixed invalid classifications.

Independent review found two real internal chunk-length defects in the initial
adapter: a single compressed-size increment was accepted despite liblzma refusal
(Go regression FAIL 1.276s), and a two-chunk compensating realignment defeated the
first raw-EOS observer (Go regression FAIL 1.340s). The repair independently tracks
raw payload spans and limits each SDK output Read to its current chunk's declared
expanded remainder. At each exact output boundary, declared raw bytes must already
be fully consumed before the SDK may read another chunk header. A one-byte first
read handles transitions, including a next chunk of length one, while preserving
the SDK's dictionary/properties/state. Complete raw EOS and SDK EOS must both hold.
LZMA2's lc+lp<=4 rule is checked before property-model creation; classic LZMA's
separate property rules are unchanged.

Frozen Python/liblzma vectors independently verify all four checks, multiple
blocks, aligned concatenated streams, declared sizes, uncompressed/mixed chunks,
one-byte next chunks and lc+lp4. The lc+lp5 vector and both malformed chunk-length
vectors are rejected by independent liblzma while the old SDK accepts their decoded
bytes, isolating the new guards. Complete-payload terminal checks, repaired-CRC
header/Index/footer negatives, canonical VLI, capacity/context failures and original
restoration are exercised. A5164-byte vector expands to33 MiB under explicit
artifact capacity, separate from business-batch32 MiB. Root final compression25
including existing Python gzip passed3.598s (33 MiB XZ .81s/LZMA .80s/bzip2 .54s/
gzip .29s). Complete default Go tests passed with package parallelism2
(compression24 mandatory groups plus one fixture skip,8.336s; Debian11 groups plus
five fixture skips,5.997s; verification27 groups plus three fixture skips,14.256s).
Module verification, whole-repository vet, server build, pinned contracts,
changed-file formatting/diff and client architecture/version guards passed; all
root processes closed. Both repaired independent static reviews passed.
Dependency independent compression25 including Python gzip passed3.741s (33 MiB
XZ .86s/LZMA .77s/bzip2 .49s/gzip .27s); actual Debian16 consumer regression with
all five fixture opt-ins passed1.251s. Affected vet, formatting and diff passed;
all processes closed. Compatibility independent compression25 including Python
gzip passed3.187s (33 MiB XZ .69s/LZMA .65s/bzip2 .46s/gzip .25s); actual Debian16
consumer regression with all five fixture opt-ins passed1.089s. Affected vet,
formatting and diff passed; all processes closed. Both repaired independent scoped
audits and both final documentation reviews passed.

No new dependency, executable decoder, SDK fork, native installer authentication,
Debian XZ dispatch, tar/control interpretation, Artifact admission or deployment.
Decoder/model workspace is separate from snapshot/dictionary/count limits and
does not certify total heap or aggregate preparation budgets. Full T09 remains
incomplete; downloads have no cumulative elapsed or retry limit.


## T09 twenty-fifth batch: Debian control/data XZ member dispatch

Debian decoded-member inspection now dispatches control.tar.xz and data.tar.xz to
the previously reviewed bounded XZ adapter. Dictionary/block/stream capacities
are explicit and apply independently to each member invocation, rather than a
control+data aggregate or total heap/preparation budget. Old plain/gzip/bzip2/LZMA
callers retain their keyed zero-field behavior. Stored/expanded capacities and
original framing identities stay independent; failures return neither report nor
decoded pair. The former unsupported-XZ regression now uses recognized unimplemented
Zstandard, preserving its no-fallback refusal after XZ support was added.

Root Debian20 groups including seven fixture checks passed1.179s. Mandatory cases
cover plain/gzip/XZ control mixed with XZ data, owned bytes and later decoys,
expanded source/memory caps, absent explicit dictionary/counts, excessive dictionary
with repaired header CRC, and raw tail refusal after old stored observation
succeeds, followed by restoration. The independent Python/liblzma both-XZ package
has640 bytes, SHA-256
677478ae83412ace2f2a880e4b441d9656888b03fd87a7278235d65b498a78bb;
its248/200 stored control/data bytes expand to the existing10240-byte tar sources.
The frozen public hello package's1868/51020 stored control/data bytes expand to
10240 bytes SHA-256
32ceb51ab23c8e75cf90b441d7f4c1ae164883ea4f4fa06603a72ca86eb948d5,
and256000 bytes SHA-256
f0c28e66b1a4d548ff77e392ae277fbba70683818a19ae97c51fbdd6ba46c1b5.
All expanded identities were independently frozen with liblzma; no package was
installed or executed.

Root actual verification31 with all four package fixture opt-ins passed13.991s.
The new signed both-XZ composition retains original/expanded identities and
refuses missing dictionary capacity with zero earlier facts, then restores the
same original source. Synthetic signed Pinvou claims bind bytes and do not attest
native app/version/architecture/publisher. Both independent static audits passed;
complete default Go tests passed with package parallelism2 (Debian13 mandatory
plus seven fixture skips,5.702s; compression24 plus one skip,3.796s; verification27
plus four fixture skips,18.644s). Module verification, whole-repository vet, server
build, pinned contracts, changed-file formatting/diff and client architecture/
version guards passed; all root processes closed. Dependency independent Debian20
with all seven fixture checks passed1.441s; verification31 with all four package
fixture opt-ins passed14.419s. Affected vet, formatting and diff passed; all
processes closed. Compatibility independent Debian20 with all seven fixture checks
passed1.077s; verification31 with all four package fixture opt-ins passed14.060s.
Affected vet, formatting and diff passed; all processes closed. Both independent
scoped audits and both final documentation reviews passed.

Tar/control interpretation, native installer certification, Zstandard, Artifact
admission, production storage and aggregate preparation evidence remain unfinished.
No new dependency/SQL/protocol/application version/deployment change. Full T09
remains incomplete; downloads have no cumulative elapsed or retry limit.


## T09 twenty-sixth batch: bounded Zstandard frame decoding

Added a separate complete-stream Zstandard adapter with explicit stored/expanded
capacities and window size. Both SDK MaxWindow and streaming MaxMemory use the
same explicit bound, including the SingleSegment content-size path. No large SDK
default is invoked. The host-int guard checks the actual pinned history-ring span
window+2*(min(window,128KiB)+16) before conversion/allocation. Block/model workspace
is separate and these limits do not certify total heap or preparation budgets.

The adapter uses synchronous concurrency1, low-memory mode, disabled whole-reader
DecodeAll shortcuts and enabled checksums. True Decoder Close runs on both passes
and all failure paths. Empty raw sources are refused, while complete empty/skippable
frames and concatenations are observed through original EOF. Known window/decoder
size errors conservatively return Limit; that SDK error can also indicate invalid
internal references/block sizes, so the classification does not prove caller
capacity alone was inadequate. Missing external dictionary returns Unsupported;
other errors return a fixed invalid classification. No external dictionary is
registered. Cancellation is polled around bounded reads. The SDK has some
diagnostic printing paths; this adapter does not claim complete stderr silence.

Adopted github.com/klauspost/compress v1.20.3 only for its Go Zstandard decoder;
one direct module and two sums changed, with no existing version drift. Its Go1.25
minimum is compatible with the pinned Go1.27.2 SDK. The full upstream module LICENSE
and actual zstd/internal/xxhash MIT license are retained byte-for-byte. No SDK fork
or executable decoder is used.

Private independent Python3.14/libzstd vectors verified small unknown-size windows,
SingleSegment refusal/restoration, empty/skippable/concatenated source EOF, truncated
refusal and checksum errors. Five new mandatory groups cover owned-buffer isolation,
stored/expanded/window caps including a later SingleSegment, raw-empty refusal,
tail lengths1/2/3/4, context failure/restoration and eight nonallocating32/64-bit
allocation boundaries. An isolated nonzero Dictionary_ID first proves the SDK's
exact UnknownDictionary refusal, then adapter Unsupported/nil and original restore.
The SDK WriterTo path emits full decoded payload before a terminal CRC error; the
adapter's Read path retains no snapshot on that error. The initial test assumed
Read emitted the final block before CRC; its proof was corrected to WriterTo
after observing the actual SDK contracts. A1075-byte unknown-size
vector expands to33 MiB with a1 MiB window and explicit artifact capacities,
separate from business-batch32 MiB. Root compression30 including existing actual
Python gzip passed3.623s (33 MiB Zstandard .24s/XZ .70s/LZMA .66s/bzip2 .46s/
gzip .24s). Complete default Go tests passed with package parallelism2
(compression29 mandatory groups plus one fixture skip,4.903s; Debian13 groups plus
seven fixture skips,1.195s; verification27 groups plus four fixture skips,14.063s).
Module verification, whole-repository vet, server build, pinned contracts,
changed-file formatting/diff and client architecture/version guards passed; all
root processes closed. Both final independent scoped audits passed with no
remaining required fixes. Dependency audit ran compression30 including actual
Python gzip in3.637s and actual Debian20 with all seven fixture opt-ins in1.417s;
compatibility audit ran the same groups in3.713s and1.352s. Each independently
passed affected vet, formatting/diff, module verification, exact dependency delta
and byte-for-byte retained-license checks. All independent processes closed.
Unchanged database/provider/browser integration was not repeated.

No Debian Zstandard dispatch, tar/control interpretation, native identity/publisher
certification, Artifact admission, production storage, SQL/protocol/application
version/deployment change. Full T09 remains incomplete; downloads have no
cumulative elapsed or retry limit.

## T09 twenty-seventh batch: Debian control/data Zstandard member dispatch

DecodeMembers dispatches control.tar.zst and data.tar.zst to the reviewed
Zstandard adapter. Caller-explicit ZstandardWindowBytes applies independently
to each member; existing stored and expanded control/data capacities remain
independent. No implicit window, format guessing or aggregate heap/preparation
policy is introduced. Non-Zstandard callers retain their previous zero-field
behavior. Every failure returns zero Report and nil decoded pair, including
failure after successful control decoding. The existing signed composition uses
the same owned source and current authorization checks.

Three new Debian groups cover mixed plain/gzip/Zstandard control, exact stored
and expanded identities, independent buffer ownership, both expanded capacities,
missing/excessive advertised window, isolated stored-valid unread tail and same
original restoration. The previous unsupported-format negative becomes valid
window plus gzip bytes labeled Zstandard: old stored capture succeeds, exact
ZstandardInvalid refuses codec fallback. Existing format tests remain intact.
An initial private draft substituted package size640 to548 throughout encoded
fixture text, corrupting the normal frame; root actual Debian testing failed two
inline cases (1.715s), while the actual package passed. Both independent static
audits identified the same fixture defect. Root restored the exact independent
945-byte Python/libzstd normal vector and added its frozen stored SHA guard;
production code and expected decoded bytes were unchanged.

The independent Python/libzstd Debian package is548 bytes, SHA256
5956a14a81aeb0e58bfbfd2ccdb4da06c60e8ed112ae0eccd12265311e6834ba.
Control/data stored spans are197/157 bytes; each decoded tar byte stream is10240
bytes with the independently frozen control/data hashes. Actual tests opt in
through UPGRADE_TEST_DEBIAN_ZSTANDARD_PACKAGE and never install or execute the
package. A new signed-composition group binds the same exact package, expanded
identities, missing-window refusal with no earlier facts and same-source restore.
Its synthetic Pinvou claims do not certify native name/version/architecture or
publisher. Root corrected actual Debian23 groups passed1.358s and actual signed
verification32 groups passed14.888s, with all eight/five fixture opt-in groups executed.
Complete default Go tests passed with package parallelism2: compression29 groups
plus one fixture skip,5.213s; Debian15 groups plus eight fixture skips,1.160s;
verification27 groups plus five fixture skips,30.500s. Module verification, whole
vet, server build, pinned contracts, changed-file formatting/diff and client
architecture/version guards passed. All root processes closed. Dependency audit
independently ran actual Debian23 in1.508s and verification32 in14.415s with no
fixture skips, plus affected vet and formatting/diff checks; all processes closed.
Compatibility audit independently ran actual Debian23 in1.188s and
verification32 in15.296s with no fixture skips, plus affected vet and formatting/
diff checks; all processes closed. Both final independent scoped code/test audits
passed with no remaining required fixes. Unchanged database/provider/browser
integration was not repeated.

No new dependency, SQL/protocol/application version or deployment change.
Tar/control interpretation, native identity/publisher certification, Artifact
admission and production storage remain separate work. Full T09 is incomplete.
Downloads retain no total/cumulative elapsed or retry-count limit.

## T09 twenty-eighth batch: bounded ordinary native TAR physical inspection

A new feature-specific platform/tar adapter inspects immutable owned snapshots
without extracting, following links or executing anything. Explicit Bytes,
MemberBytes and Members limits bound source, individual payload and report count.
It parses ordinary unextended V7/USTAR/GNU headers through the Go standard parser,
checks complete physical spans and hashes each exact payload. Two end blocks and
all trailing complete zero blocks through original source EOF are mandatory;
checksum, truncation, incomplete terminators and nonzero/unread tail refuse with
zero Report. Payload loops use fixed32 KiB scratch and cancellation checks.
No header-controlled expanded allocation or implicit preparation policy is added.

Names, links and modes are declarations, not safe host paths or permission/native
identity evidence. Regular files, directories, links, device and FIFO headers are
observed; non-file payload declarations refuse. Duplicate names retain distinct
physical spans. The original zero/V7 type byte stays in Member.Type while the
standard parser's normalized type is used for payload rules. PAX/GNU long-name
extensions, sparse maps, STAR and unknown types return explicit Unsupported;
this batch never guesses their logical member spans.

Both independent static audits identified two SDK integration P2 issues: valid
headers accompanied by ErrInsecurePath could depend on ambient GODEBUG and host
filepath rules; STAR was silently accepted beyond the declared subset. Root
allows only this specific error with a valid header because no declared name is
used as a host path, and explicitly refuses the USTAR-magic/STAR-trailer layout.
Tests independently isolate real SDK ErrInsecurePath plus a valid header from
adapter acceptance and invalid checksum refusal, and SDK-valid STAR from
Unsupported plus same USTAR restoration. Genuine V7 regular/directory zero type,
GNU and device/FIFO positives cover the accepted ordinary declarations. The first
new V7-directory test incorrectly asked the standard writer to encode a regular
file ending in slash (1.321s failure); its setup now constructs a directory header
before setting the legacy raw type. Production behavior was not altered for that
test-only setup correction.

Nine root actual groups passed1.582s (33 MiB payload .36s), including the one
fixture opt-in group. GNU base-256 MaxInt64 is a valid SDK numeric declaration
but exceeds the actual source span and returns Invalid without arithmetic
wraparound; negative/non-numeric sizes and non-file bodies also refuse. Source,
payload and count boundaries, context/nil handling, later checksum failure,
footer/tail failure and exact same-source restore are covered. Independent Python
USTAR/GNU fixtures each10240 bytes have frozen identities, as do actual public
Debian control/data tar streams10240/256000 bytes with3/143 members. Actual PAX
and GNU-long-name fixtures prove unsupported refusal; no package was installed,
executed or natively authenticated. The private fixture directory opt-in is
UPGRADE_TEST_TAR_FIXTURE_DIRECTORY; files remain outside repositories.

Complete default Go tests passed with package parallelism2: TAR8 groups plus one
fixture skip,6.308s; compression29 groups plus one fixture skip,6.411s; Debian15
groups plus eight fixture skips,5.761s; verification27 groups plus five fixture
skips,15.982s. Module verification, whole vet, server build, pinned contracts,
changed-file formatting/diff and client architecture/version guards passed.
All root processes closed. Both final independent scoped code/test audits passed
with no remaining required fixes. Dependency audit independently ran actual TAR9
in1.449s (33 MiB payload .29s); compatibility audit ran actual TAR9 in1.462s
(payload .26s). Both executed all six fixtures in the actual fixture group with
no skips, and each passed affected vet, two-file formatting and diff checks.
All independent processes closed. Unchanged database/provider/browser integration
was not repeated.
No Debian TAR composition, logical contents/control interpretation, native
name/version/architecture/publisher certification, Artifact admission, production
storage, SQL/protocol/application version/deployment or dependency change.
Full T09 remains incomplete. Downloads have no total/cumulative elapsed or
retry-count limit.

## T09 twenty-ninth batch: current signed Debian TAR pair composition

Debian InspectTars uses one owned installer for complete framing, stored-member
capture, decoding, then complete control/data TAR physical inspection. Caller
TarLimits are independent for each expanded member and separate from stored/
expanded decoding limits. TarMembers retains the same owned decoded sequences
plus their complete physical reports; no pathname reopen or extraction occurs.
Every failure returns zero framing and nil members, including data failure after
successful control inspection. The original inspection/decoding APIs are intact.

Verifier InspectTarDebianInstaller first applies the existing current signed
Manifest, FullPack/native-byte and Debian framing guards. It inspects the same
owned Native.Bytes and compares the resulting whole source identity with the
previous framing. Any later failure returns zero earlier package/native/framing
facts and no expanded sequences; cancellation is checked before successful return.
The observations grant no safe logical paths, app identity, publisher or Artifact
admission. Current authorization and original byte/source binding are preserved.

Three new Debian groups cover complete plain/gzip control/data pairs, exact
stored/expanded/report identities, caller-buffer isolation, independent control/
data TAR capacities, data payload/count caps, a late second-header checksum
negative after successful old decoding, and exact same-source restoration. The
actual opt-in group requires all six existing frozen packages: public hello plus
independent Python gzip, bzip2, LZMA, XZ and Zstandard. Whole package identities
and decoded stream identities are frozen; public TAR counts are3/143 and the
independent packages have1/1 ordinary members. No installation or script execution
occurs. These fixtures do not certify native app identity or publisher.

Four new signed-composition groups cover exact same source/expanded sequences,
current package denial and restoration, old signed-decoding positive followed by
isolated TAR failure, per-data capacity refusal, cancellation and same original
restore. Actual public hello and Python Zstandard compositions preserve complete
signed byte/framing facts and frozen decoded TAR identities/counts. Their Pinvou
signed claims are synthetic test byte bindings, not native platform certification.
Root actual Debian26 groups passed1.265s and verification36 groups passed15.280s,
with all nine/seven fixture opt-in groups executed. Both initial independent
static audits passed without required fixes. Complete default Go tests passed
with package parallelism2: Debian17 groups plus nine fixture skips,1.364s;
verification29 groups plus seven fixture skips,14.990s; TAR8 groups plus one
fixture skip,1.828s; compression29 groups plus one fixture skip,4.108s. Module
verification, whole vet, server build, pinned contracts, changed-file formatting/
diff and client architecture/version guards passed. All root processes closed.
Both final independent scoped code/test audits passed with no remaining required
fixes. Dependency audit independently ran actual Debian26 in1.377s and
verification36 in15.733s; compatibility audit ran them in1.418s and15.039s.
Each ran all nine/seven fixture opt-in groups without skips and passed affected
vet, four-file formatting and diff checks. All independent processes closed.
Unchanged database/provider/browser integration was not repeated.

No existing codec, dependency, SQL/protocol/application version or deployment
change. Logical TAR contents/control interpretation, native identity/publisher
certification, Artifact admission and production storage remain separate work.
Full T09 remains incomplete. Downloads retain no total/cumulative elapsed or
retry-count limit.

## T09 thirtieth batch: bounded original Debian control-record lexical parsing

ParseControl observes one immutable UTF-8 binary-control stanza. Explicit
ControlLimits.SourceBytes and MemoryBytes bound the logical copied source;
Fields bounds report cardinality. These capacities do not bound the combined
raw/string/report heap or aggregate preparation policy. Copy loops use 32 KiB
chunks, host-int checks before allocation and context checks. All failures return
zero source identity and nil fields, including late duplicate/second-stanza errors.

The lexical profile follows [Debian Policy section 5.1](https://www.debian.org/doc/debian-policy/ch-controlfields.html#syntax-of-control-files):
ASCII field names use the documented printable ranges and cannot start with # or -;
names are case-insensitive and duplicate identities refuse. One stanza supports
LF/CRLF, final EOF without newline and horizontal-whitespace stanza separators.
Orphan continuations, source-template comments, second stanzas, wholly empty
field values, invalid UTF-8, ASCII C0 bytes except HT/LF/CR framing, DEL and stray
CR refuse. Other valid UTF-8 value text is opaque; Unicode control categories are
not claimed to be prohibited. Blank separators never become guessed field values.

ControlField preserves original field-name spelling and value/continuation line
content, including leading/trailing horizontal whitespace and dot lines. CRLF/LF
terminators are parsed while Source binds the exact original bytes. Field-specific
folding, multiline unescaping, required binary-field semantics, package/version/
architecture grammar, safe display/path policy, approved app mapping and publisher
trust are separate work. A Maintainer text field never proves signing custody.
Existing Debian framing/codec/TAR/signed-composition APIs are unchanged; no new
inspection composition or Artifact validity is introduced.

Five new groups cover UTF-8, case folding, raw line preservation, line endings,
late duplicate and second-stanza rejection, malformed names/value/control bytes,
source/memory/field capacities, context/nil and same-source restoration. A 33 MiB
opaque extension value succeeds under caller-selected artifact capacities; one-
byte lower logical memory refuses. The business atomic 32 MiB limit is unrelated.
Two actual frozen records opt in via UPGRADE_TEST_DEBIAN_CONTROL_FIXTURE_DIRECTORY:
public hello control: 757 bytes; SHA-256
27ee01d2de09a1a678763c41013d4d1aa47e6985230ca08f414e903a237fd163,
13 fields; independent Python control: 109 bytes; SHA-256
d557a48d2711f6984cb7b3c4bfa4d67e95db4da68e14fd967d0199d7a6253d79,
4 fields. Their observed package/version/architecture remain declarations;
fixtures and public format-research captures stay outside repositories.

Root selected control 5 passed 1.735s (.30s large value). Root complete actual
Debian 31 passed 1.785s (.24s large value), with all ten fixture opt-in groups
executed. Both initial independent scoped static audits passed without required
fixes. Complete default Go tests passed with package parallelism 2: Debian 21
groups plus ten fixture skips, 5.892s; verification 29 groups plus seven fixture
skips, 19.639s; TAR 8 groups plus one fixture skip, 1.618s; compression 29 groups
plus one fixture skip, 3.816s. Module verification, whole vet, server build, pinned
contracts, changed-file formatting/diff and client architecture/version guards
passed. All root processes closed. Dependency audit independently ran actual
Debian 31 in 2.440s (large value .27s), all ten fixture groups without skips, and
passed affected vet, two-file formatting and diff checks; all processes closed.
Compatibility audit independently ran actual Debian 31 in 1.713s (large value
.24s), all ten fixture groups without skips, and passed affected vet, two-file
formatting and diff checks; all processes closed. Both final independent scoped
code/test audits passed with no remaining required fixes.
Unchanged database/provider/browser integration was not repeated.
No dependency, SQL/protocol/application version/deployment or existing behavior
change. Full T09 remains incomplete. Downloads retain no total/cumulative elapsed
or retry-count limit.

## T09 thirty-first batch: current signed exact root control-file composition

Debian InspectControl uses the same immutable installer for complete framing,
decoding and both physical TAR inspections before locating the control record.
It selects exactly one root declaration named control or ./control, accepting
ordinary regular and legacy raw-zero regular types. Repeated aliases or identical
names refuse even when their bytes match; links, directories, FIFO and other
special declarations cannot supply the record. Nested/absolute paths do not
become guessed root aliases. No host path is opened, followed or extracted.

The exact observed member offset/size is captured into an owned snapshot and its
identity compared with the physical TAR member before ParseControl. Positive
source/memory/field capacities are required before inspection. Capture and parser
copies each have their explicit logical capacity; this does not certify combined
heap or aggregate preparation budgets. ControlMembers retains both complete TARs,
owned selected bytes and lexical observations. Every failure returns zero framing
and nil members, including later field-count, duplicate or lexical errors.

Verifier InspectControlDebianInstaller first uses the existing current signed
Manifest, FullPack/native-byte and framing guards, then the same Native.Bytes for
the complete control inspection, comparing the whole source identity and checking
context before successful return. Any later failure returns zero earlier package/
native/framing facts and no captured control/TAR/decoded bytes. Old APIs and their
behavior are intact; parsed declarations grant no approved app identity, publisher,
safe installation namespace or Artifact admission.

Three new Debian groups cover root control/./control with legacy-zero/ordinary
regular declarations, exact file/report identities, projection and caller-source
mutation isolation, alias/same-name duplicates, missing root, links/hardlinks/
directories/FIFO and late duplicate-field negatives. Each isolated negative first
passes the prior complete TAR observer, then returns exact ControlInvalid with no
previous facts. Separate capture source/memory, lexical field-count and data TAR
caps refuse, followed by exact original restoration. An actual six-package group
uses all existing frozen public/Python codec fixtures: selected public control is
757 bytes, 13 fields; all independent controls are109 bytes, 4 fields, with the
previously frozen original record identities.

Four new signed groups cover current package denial/restoration, exact original
installer/control bytes, old signed TAR success before isolated lexical refusal,
capture/field capacity, cancellation and same-source restore. Actual public hello
and Python Zstandard compositions bind the same record identities and counts.
Their signed Pinvou claims are synthetic test byte bindings; no native app/version/
architecture/publisher certification is implied. Root actual Debian34 passed1.632s
and verification40 passed16.032s, with all eleven/nine fixture opt-in groups
executed. Both initial independent scoped static audits passed without required
fixes. Complete default Go tests passed with package parallelism 2: Debian 23
groups plus eleven fixture skips, 1.586s; verification 31 groups plus nine fixture
skips, 19.614s; TAR 8 groups plus one fixture skip, 1.216s; compression 29 groups
plus one fixture skip, 3.587s. Module verification, whole vet, server build, pinned
contracts, changed-file formatting/diff and client architecture/version guards
passed. All root processes closed. Dependency audit independently ran actual
Debian 34 in 2.142s and verification 40 in 15.601s, with all eleven/nine fixture
groups without skips, plus affected vet, four-file formatting and diff checks;
all processes closed. Compatibility audit independently ran actual Debian 34 in
1.621s and verification 40 in 19.826s, all eleven/nine fixture groups without skips,
plus affected vet, four-file formatting and diff checks; all processes closed.
Both final independent scoped code/test audits passed with no remaining required
fixes. Unchanged database/provider/browser integration was not repeated.

No dependency, SQL/protocol/application version/deployment change. Required-field
semantics, native matching, publisher certification, Artifact admission and
production storage remain separate work. Full T09 remains incomplete. Downloads
retain no total/cumulative elapsed or retry-count limit.

## T09 thirty-second batch: required identity declarations from original control bytes

ReadIdentity accepts an immutable owned control-file snapshot and explicit
ControlLimits, reparses it through ParseControl, and requires Package, Version
and Architecture to each exist as exactly one nonempty logical line. Case-folded
field lookup and horizontal-whitespace trimming derive three declaration strings;
the original record, field spelling, lines and complete original byte identity
remain available. No caller-supplied record projection or field default is used.

Missing/folded required fields, base lexical errors, capacity or cancellation
failures return zero IdentityDeclarations, nil record fields and no earlier
partially selected value. Context is checked before successful return. Values
remain opaque declarations: this batch does not validate field-specific native
package/version/architecture grammar, compare versions, certify a machine,
apply an approved app/publisher mapping or grant Artifact validity. Existing
parser, framing, codecs, TAR and signed-composition APIs are unchanged.

Three new groups cover original case/raw-line preservation, leading-zero version
text retained without normalization, caller-source/projection mutation isolation,
each missing/folded field and same-source restoration. Each required-field
negative first succeeds in the base lexical observer, then returns the precise
IdentityInvalid result. Late duplicate-field, field capacity, context/nil and
underlying-error propagation retain no record or value. The actual opt-in group
uses both previously frozen public/Python control records and independently
checks their original SHA/size plus all three exact declaration strings. No
package is installed or executed; those declarations prove no native identity
or publisher approval.

Root selected identity 3 groups passed 1.260s. Root actual Debian 37 groups passed
1.627s, with all twelve fixture opt-in groups executed. Both initial independent
scoped static audits passed without required fixes. Complete default Go tests
passed with package parallelism 2: Debian 25 groups plus twelve fixture skips,
1.599s; verification 31 groups plus nine fixture skips, 15.759s; TAR 8 groups plus
one fixture skip, 1.504s; compression 29 groups plus one fixture skip, 4.273s.
Module verification, whole vet, server build, pinned contracts, changed-file
formatting/diff and client architecture/version guards passed; all root processes
closed. Both final independent scoped code/test audits passed without remaining
required fixes. Dependency audit independently ran actual Debian 37 in 1.698s;
compatibility audit ran it in 1.603s. Both executed all twelve fixture groups
without skips and passed affected vet, two-file formatting and diff checks;
all independent processes closed. Unchanged database/provider/browser integration
was not repeated. The copied-source capacities keep
the previous logical, non-aggregate-heap meaning; no production preparation policy
is inferred. No dependency, SQL/protocol/application version/deployment change.
No new complete-installer/signed identity composition or native matching yet.
Full T09 remains incomplete. Downloads have no total/cumulative elapsed or
retry-count limit.

## T09 thirty-third batch: current signed complete required-identity composition

Debian InspectIdentity performs complete same-owned-installer framing, decoding,
both TAR inspections and unique regular root control capture before ReadIdentity
reparses the exact captured ControlFile bytes for all three required declarations.
It compares the new identity record Source with the prior control record Source
and checks context before returning. No caller-supplied field projection is
consumed. Every later error returns zero framing and nil IdentityMembers, without
earlier TAR/decoded/control bytes, parsed fields or a partial identity string.

Verifier InspectIdentityDebianInstaller first preserves existing current signed
Manifest, FullPack/native-byte and framing guards, then uses the same Native.Bytes
for complete identity inspection, comparing the whole source identity and checking
context. Later failure returns zero previous package/native/framing facts and no
member sequences. Original APIs are unchanged. The repeated original-byte parsing
is deliberate; each capture/parser copy has its own logical byte capacity, not a
combined heap or preparation budget. Declaration values remain opaque; no native
package/version grammar, version ordering, approved architecture/app mapping,
publisher certification or Artifact admission is introduced.

Three new Debian groups cover exact installer/control/identity-record identities,
all three values, independent returned-record/source mutation isolation and same
source restoration. Missing/folded required-field cases first pass the previous
complete control observer, then return exact IdentityInvalid with no prior facts.
Field capacity refusal also returns no captured control bytes. The actual group
uses all six frozen public/Python codec packages, retaining their whole-package
identities and original selected control identities plus exact three declaration
strings. No installation or script execution occurs.

Four new signed groups cover current package denial and restoration, original
installer/control bytes, old signed complete-control positives before isolated
required-field failure, field capacity, cancellation and same-source restore.
Actual public hello and Python Zstandard retain both control-record byte identities
and all three declaration strings. Their signed Pinvou claims are synthetic test
byte bindings, not approved native app/publisher mappings or platform certificates.
Root actual Debian 40 groups passed 1.662s and verification 44 groups passed
15.847s, with all thirteen/eleven fixture opt-in groups executed. Both initial
independent scoped static audits passed without required fixes. Complete default
Go tests passed with package parallelism 2: Debian 27 groups plus thirteen fixture
skips, 1.789s; verification 33 groups plus eleven fixture skips, 15.897s; TAR 8
groups plus one fixture skip, 1.584s; compression 29 groups plus one fixture skip,
4.071s. Module verification, whole vet, server build, pinned contracts, changed-
file formatting/diff and client architecture/version guards passed. All root
processes closed. Both final independent scoped code/test audits passed without
remaining required fixes. Dependency audit independently ran actual Debian 40 in
1.641s and verification 44 in 16.682s; compatibility audit ran them in 1.742s and
16.564s. Each executed all thirteen/eleven fixture groups without skips and passed
affected vet, four-file formatting and diff checks; all independent processes
closed. Unchanged database/provider/browser integration was not repeated.

No dependency, SQL/protocol/application version/deployment change. Native matching,
field-specific grammar, publisher certification, Artifact admission and production
storage remain separate work. Full T09 remains incomplete. Downloads retain no
total/cumulative elapsed or retry-count limit.

## T09 thirty-fourth batch: isolated complete Debian inspection command

`cmd/upgrade-debian-inspect` opens a supported regular descriptor through the
existing explicit OS source adapter, captures an exact bounded immutable process
snapshot, then performs complete Debian framing, codec, both TAR, unique regular
root control and required-declaration inspection. It outputs one JSON summary
only after all checks succeed. The summary includes whole installer, stored
control/data, expanded TAR and original control-record byte identities, exact
TAR member counts, format version and Package/Version/Architecture declarations.
It omits archive paths and unrelated control fields. These are unsigned byte and
declaration observations, not approved native identity or publisher claims.

All 26 capacity arguments are required positive values; no production capacity
defaults are inferred. Each used codec enforces its existing specific limits.
Snapshot/record capacities are logical bytes, not an aggregate process heap or
preparation budget. The 32 MiB atomic business-batch limit does not cap files.
For the small frozen inspection fixtures only, a complete invocation is:

```sh
go run ./cmd/upgrade-debian-inspect \
  --package-bytes 2097152 --package-memory-bytes 2097152 --version-bytes 16 \
  --stored-control-bytes 2097152 --stored-control-memory-bytes 2097152 \
  --stored-data-bytes 2097152 --stored-data-memory-bytes 2097152 \
  --expanded-control-bytes 2097152 --expanded-control-memory-bytes 2097152 \
  --expanded-data-bytes 2097152 --expanded-data-memory-bytes 2097152 \
  --lzma-dictionary-bytes 4096 --xz-dictionary-bytes 8388608 \
  --xz-blocks 8 --xz-streams 8 --zstandard-window-bytes 1024 \
  --control-tar-bytes 2097152 --control-tar-member-bytes 2097152 \
  --data-tar-bytes 2097152 --data-tar-member-bytes 2097152 \
  --control-record-bytes 1024 --control-record-memory-bytes 1024 \
  --ar-members 10 --control-tar-members 1000 --data-tar-members 1000 \
  --control-record-fields 20 fixture.deb
```

Exit codes are 0 for a complete report/help, 2 for invalid arguments, and 1 for
source, inspection, cancellation or output failure. Argument/source/provider
details are omitted from fixed diagnostics. Short writes, including an initial
short help write followed by a successful writer, cannot report success.
An output-device failure can physically leave a truncated stream, but returns
failure; callers must require exit 0 and complete JSON. Underlying codec numeric
diagnostic limitations remain as documented in the standalone codec batch.

Existing source-profile boundaries remain: Windows checks the final disk handle
and supported local volume without following a final reparse point; Unix refuses
a final symlink and blocking FIFO but does not attest remote-mount locality.
Unsupported hosts refuse. Owned captured bytes resist later path replacement;
capture does not attest that an arbitrary concurrent same-size source write
never occurred. This is not protected staging, privileged final handoff or an
installation command. No files are extracted and no package scripts are executed.

Six command groups cover all 26 missing/zero/negative arguments, help, actual
regular source and replacement isolation, source/copy/field capacities, late
missing identity fields, cancellation and same-source restoration, fixed errors
and short-output refusal. Actual opt-in cases use all six frozen public/Python
packages, independently checking every stored and expanded byte identity, original
control identity, exact member counts and all three declarations. The built CLI
E2E runs five child invocations: synthetic success, invalid arguments, late
required-field failure, actual public hello and actual Python Zstandard success.
The 60-second child allowance is test-only and sets no production download limit.

Root default command tests passed four mandatory groups in 1.286s with two
explicit opt-in skips. Initial actual command/E2E passed all six groups in 3.772s.
Root self-review then strengthened every reported stored/expanded byte identity
and exact count assertion; the frozen actual six-group rerun passed in 2.538s
without skips. Both independent static audits passed that final code/test delta.
Complete default Go tests passed with package parallelism 2: CLI 6.151s, Debian
1.734s, verification 20.217s, compression 4.197s and TAR 1.755s; fixture/provider/
database/browser opt-ins were not enabled in that default run. Module verification,
whole vet, server build, private inspector build, contract pins and changed-file
formatting/diff passed. All root build/test processes closed.

No dependency, SQL, protocol, application version or deployment change. Current
Root/signed Manifest inspection remains in the existing verifier API and is not
granted by this standalone command. Field-specific grammar, native app/architecture
mapping, publisher authentication, Artifact admission and production storage remain
subsequent work. Full T09 remains incomplete. Downloads have no total/cumulative
elapsed or retry-count limit.

Both final independent scoped code/test audits passed without required fixes.
Dependency audit ran all six actual CLI groups in 6.598s (built-child group 3.93s);
compatibility audit ran them in 3.416s (built-child group 1.86s). Each enabled all
six native fixtures and all five actual child invocations without skips, and
passed affected vet, two-file formatting and diff checks. All independent
processes closed. Client architecture and synchronized-version guards passed.
Unchanged database/provider/browser integration was not repeated.

## T09 thirty-fifth batch: PE image physical-span inspection

The new explicit `artifacts/platform/pe` adapter observes PE32 and PE32+ headers,
raw section spans and embedded attribute-certificate table spans on an owned
immutable snapshot. Source bytes, optional-header bytes, section count and
certificate count are separately required positive capacities before applicable
allocation. uint16/uint32 declarations are widened to int64 before offset/length
arithmetic; subtraction bounds guard every actual read. The directory count must
fit the actual optional-header size. Its Security directory address is interpreted
as a file offset, not an RVA.

The section table must fit declared SizeOfHeaders within the source. Every
nonempty raw span lies after headers and within the source. Nonempty spans are
sorted only for non-overlap validation before hashing, preventing repeated
overlapping source reads. Returned order remains declared section order, with
raw eight-byte names and virtual/characteristic declarations. Empty sections
have an empty-byte identity and retain their declared pointer without inventing
a physical read. Machine values remain observed declarations, not a supported
architecture or signed app mapping.

A present certificate table must be eight-byte aligned, fully within the file
and after all nonempty section spans. Every entry has at least its eight-byte
header; rounded dwLength values must exactly exhaust the table. Extra alignment
bytes outside dwLength must be zero. Each identity covers exactly dwLength,
including any padding already counted inside that declaration; only additional
external alignment bytes are excluded. Revision, type and opaque certificate
payloads are observations. DER structure, unique semantic certificate types,
Authenticode image digests/signatures and publisher authentication are not
established. Missing embedded tables do not establish absence or validity of
catalog authentication.

Header, section and certificate parsing are separate narrow adapter functions.
Every failure returns a literal zero report without prior source, sections or
certificate facts. Context is checked between bounded reads/hashes and before
success. Source and returned projection mutation cannot change the owned input.
Uninterpreted overlays are retained in the complete Source identity. No bytes
are extracted, executed, installed or handed to a privileged helper.

This physical observer does not validate Windows virtual loading, FileAlignment,
imports, resources, file/product version, NSIS package contents or installation
support. It grants no native format certification, publisher approval, protected
execution-object handoff or Artifact admission. Existing Debian and signed
verifier APIs are unchanged; no signed PE composition is introduced in this batch.
The Microsoft [PE format reference](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format)
was read privately to check optional-header/directory widths and the certificate
table file-pointer/rounding rules. No reference or binary fixture is committed.

Seven groups cover PE32/PE32+ original source/section/certificate identities,
caller-source/projection isolation and restoration, malformed/truncated headers,
unbounded-looking uint32 offsets/sizes, overlap, late certificate/padding/tail
errors, missing/exceeded capacities, invalid sources/context and pre-cancellation.
Separate controls retain original section order, ignore undeclared directory
bytes and preserve a zero-size section's pointer only as a declaration. A file
with more than 33 MiB of opaque overlay succeeds under explicit test capacities;
the 32 MiB atomic business-batch limit remains unrelated to file size.

Actual opt-in inspection reads the previously frozen built Go inspector image
(5,933,568 bytes, 16 sections) and a frozen actual Windows system image (360,448
bytes, 7 sections). Original SHA/size are fixed; both have no embedded certificate
table. Go's independent debug/pe parser and direct original-byte SHA checks verify
all raw section spans and virtual/characteristic declarations. Neither image is
executed in this test and no actual signed-certificate/WinVerifyTrust or Windows
installation certification is claimed.

Initial default selected tests passed six groups in 1.641s with one explicit
fixture skip; initial actual seven groups passed in 1.566s. Dependency review found
a test-only unbounded fixture-read gap. Root repaired it to require a regular file
with exact expected size before reading at most size+1, then close and check exact
read length and original SHA. Both independent roles passed the static repair.
Root frozen actual seven-group rerun passed in 1.572s without skips.

Complete default Go tests passed with package parallelism 2: PE six mandatory
groups plus one fixture skip, 6.066s; Debian 2.903s, verification 16.022s,
compression 4.504s and TAR 2.225s. Fixture/provider/database/browser opt-ins were
not enabled in that default run. Module verification, whole vet, server build,
contract pins, changed-file formatting and diff checks passed; all root
build/test processes closed.

No dependency, SQL, protocol, application version or deployment change. Approved
native app/publisher mappings, signed PE composition, full native authenticity,
Artifact admission and production storage remain later work. Full T09 remains
incomplete. Downloads have no total/cumulative elapsed or retry-count limit.

Both final independent scoped code/test audits passed without remaining fixes.
Dependency audit ran all seven actual PE groups in 1.455s; compatibility audit ran
them in 1.583s. Both enabled the frozen Go/Windows image group without skips and
passed affected vet, four-file formatting and diff checks; all independent
processes closed. Client architecture and synchronized-version guards passed.
Unchanged database/provider/browser integration was not repeated.

## T09 thirty-sixth batch: current signed PE installer byte composition

`Verifier.InspectPEInstaller` preserves the existing current signed Manifest,
published Root/deny, complete FullPack and exact owned finalInstaller byte guards
before requiring the signed format declaration to be `nsis`. It then inspects
physical PE framing on the same Native.Bytes, compares the complete source byte
identity and checks context before success. Any failure returns a literal zero
PEInstallerFacts with no earlier package, selected native or physical PE facts.
Original APIs and the standalone PE observer are unchanged.

The nsis declaration selects this observer; neither a file name nor MZ/PE bytes
can override a different correctly signed format. This composition does not
certify actual NSIS package contents, Windows loading, app/file version,
architecture matching, Authenticode, native publisher or approved Pinvou identity.
Current signature/byte claims plus physical PE observations remain inputs to
later native authenticity and admission guards. The final Artifact owner must
reread current authority and protected immutable identities before granting
qualification. No published Artifact state or executable handoff is introduced.

Three mandatory groups cover original signed owned installer and section
identities, source/returned-projection isolation, current deny and restoration,
cancellation, declared-format refusal and late header/optional-variant/certificate/
capacity errors. Each isolated malformed-PE case first passes the previous current
signed native-byte observer, then fails the new framing step with no prior facts.
A correctly signed Linux/deb claim over physically PE bytes first passes its byte
guard, then is refused as Unsupported by this nsis-declaration observer.

Two actual opt-in groups bind the same frozen built Go and Windows system PE
images used in the preceding batch. Reading is bounded by regular-file/exact-size
prechecks, size+1 reads, close, original SHA and complete byte comparison. Their
synthetic strong Pinvou signatures declare nsis solely to test current signed
byte continuity. Neither actual file is certified as an NSIS installer or Pinvou
application, and no file is executed or installed. Explicit larger test ZIP
capacities accommodate the 5,933,568-byte image; they are not production upload,
aggregate heap or preparation policy. The business-batch 32 MiB limit remains
unrelated to file size.

Root actual selected five-group run passed in 8.896s with both actual fixture
groups executed. Both independent initial scoped static audits passed without
required fixes.

Complete default Go tests passed with package parallelism 2: verification
17.339s, PE 6.177s, Debian 1.741s, compression 3.888s, TAR 1.690s and Debian CLI
2.366s. Fixture/provider/database/browser opt-ins were not enabled in that
default run. Module verification, whole vet, server build, pinned contracts,
changed-file formatting and diff checks passed; all root processes closed.

No dependency, SQL, protocol, application version or deployment change. Actual
NSIS parsing, approved native identity/version/architecture/publisher matching,
full authenticity, Artifact admission and production storage remain subsequent
work. Full T09 remains incomplete. Downloads have no total/cumulative elapsed
or retry-count limit.

Both final independent scoped code/test audits passed without required fixes.
Dependency audit independently ran all five selected groups in 3.456s;
compatibility audit ran them in 3.805s. Both executed the two actual fixture
groups without skips and passed affected vet, two-file formatting and diff
checks; all independent processes closed. Client architecture and synchronized-
version guards passed. Unchanged database/provider/browser integration was not
repeated.

## T09 thirty-seventh batch: isolated PE inspection command

`cmd/upgrade-pe-inspect` uses the existing explicit OS regular-source adapter to
capture an exact bounded immutable snapshot before the unchanged physical PE
observer runs. All five positive capacities are mandatory: complete source bytes,
owned snapshot bytes, optional-header bytes, sections and certificates. They have
no production defaults and do not establish aggregate heap or preparation budgets.
For the isolated frozen inspection fixtures only:

```sh
go run ./cmd/upgrade-pe-inspect \
  --source-bytes 16777216 --snapshot-memory-bytes 16777216 \
  --optional-header-bytes 4096 --sections 96 --certificates 8 fixture.exe
```

The complete JSON report is written only after successful observation. It retains
source, section and certificate byte identities, raw eight-byte section-name
declarations, header size, machine/optional magic and other observed fields.
It does not resolve COFF string-table section names into logical names. No Root,
signed Manifest, NSIS contents, app version, architecture approval, Authenticode
or publisher authentication is performed by the command. Current signed package
composition remains in the preceding verifier API. There is no Artifact admission,
installation, extraction, external script execution or protected handoff.

Exit 0 requires complete report/help; invalid arguments return 2; source,
inspection, cancellation or output failure returns 1. Diagnostics omit raw
arguments, provider failures and source paths. Short writes, negative/overlong
write counts and initial help failure remain sticky and cannot report success.
A failing output device may physically leave a truncated stream; consumers must
require exit 0 plus complete JSON. The source adapter keeps its existing Windows
final-handle/local-volume/final-reparse and Unix no-final-symlink/nonblocking-FIFO
boundaries. Unix remote-mount locality and arbitrary concurrent same-size source
mutation are not attested. Unsupported hosts refuse.

Six groups cover every missing/zero/negative capacity and unknown/extra arguments,
actual regular input and post-capture replacement isolation, source/copy/header
capacities, late certificate failure, nil/pre-cancelled capture and same-source
restoration. Synthetic opaque certificate output independently checks complete
offset, original identity, Revision and Type; it is not a PKCS#7 signature proof.
Report/help output checks include short, negative and overlong writer returns.

Two fixture opt-ins use the same frozen Go/Windows PE files from the prior batches,
with regular/exact-size prechecks, bounded reads, close and original SHA guards.
Go debug/pe independently checks raw section spans, virtual/characteristic
declarations and SizeOfHeaders. Original eight-byte name declarations are compared
directly with their original physical headers; SDK-resolved long names are separate.
The actual built command E2E runs six children: synthetic success, invalid argument,
invalid framing, same-path original restoration, actual Go image and actual Windows
image. The 60-second child allowance is test-only and creates no download deadline.
Only the inspection command is executed; input PE images are never launched.

Initial default tests passed four mandatory groups in 1.702s with two explicit
fixture/binary opt-in skips. Initial actual run failed in 4.202s because the test
incorrectly compared original eight-byte names with SDK-resolved COFF long names.
Root corrected the test to compare original physical names; production behavior
was unchanged. Root frozen actual six-group rerun passed in 3.729s, with both
fixture groups and all six actual child invocations executed without skips.

Complete default Go tests passed with package parallelism 2: PE CLI four
mandatory groups plus two opt-in skips, 1.383s; verification 20.959s, PE 1.484s,
Debian 1.990s, compression 8.455s and TAR 1.631s. Fixture/provider/database/browser
opt-ins were not enabled in that default run. Module verification, whole vet,
server build, private inspector build, contract pins, changed-file formatting
and diff checks passed; all root build/test processes closed.

No dependency, SQL, protocol, application version or deployment change. Full
native authenticity/version/app/publisher mapping, Artifact admission and
production storage remain later work. Full T09 remains incomplete. Downloads
have no total/cumulative elapsed or retry-count limit.

Both final independent scoped code/test audits passed without required fixes.
Dependency audit independently ran all six actual CLI groups in 10.362s (built
child group 8.63s); compatibility audit ran them in 3.259s (built child group
1.74s). Both enabled both frozen PE fixtures and all six actual child invocations
without skips, and passed affected vet, two-file formatting and diff checks.
All independent processes closed. Client architecture and synchronized-version
guards passed. Unchanged database/provider/browser integration was not repeated.

## T09 thirty-eighth batch: original Windows fixed-version value observation

The new `artifacts/platform/windowsversion.ObserveFixed` adapter accepts exactly
one complete owned VS_VERSION_INFO record with explicit positive source-byte
capacity. It validates the uint16 root length against the whole original source,
binary type, 52-byte fixed value, complete UTF-16 VS_VERSION_INFO key, zero
alignment, fixed signature and supported VS_FIXEDFILEINFO structure version 1.0.
It uses a fixed 92-byte buffer; no caller record projection or default field
is accepted. Unknown fixed layouts return Unsupported. Any failure returns a
literal zero FixedValue, and context is checked before/after reads and at return.

The complete original record and exact 52-byte fixed value retain independent
SHA/size identities. The checked signature constant and remaining words retain
structure version, all four file/product version components, flags/mask, file OS,
type/subtype and both file-date words. Nonzero fourth components and uint16 maxima
are preserved. No product-version normalization, comparison or approved Windows
application mapping is introduced. FileDate is opaque declaration data and must
never establish trustworthy time or preparation/retention deadlines.

Children remain uninterpreted original bytes inside Source's complete identity.
This is a fixed-value observer, not a complete StringFileInfo/VarFileInfo parser,
translation/company validation, PE resource-tree locator, executable association,
NSIS or publisher authentication. No raw company names, extracted files,
executable handoff, Artifact state or admission result is produced. Existing PE,
signed byte and standalone CLI APIs remain unchanged.

Four mandatory groups cover exact original identities/all fields, complete
four-part preservation and caller-source/projection isolation; missing/truncated
root/key/type/value/alignment/signature negatives; unsupported structure, extra
source bytes, capacities, nil source/context, cancellation and same-original
restoration. Maximum version components and opaque child bytes are independent
positive controls. A record's uint16 root-length constraint is intrinsic format
framing, unrelated to the 32 MiB atomic business-batch capacity.

The actual opt-in fixture is an 880-byte original version record privately read
from the same frozen Windows system image used earlier. Its original SHA is
fixed, as is the exact fixed value's SHA. Root independently called actual Windows
GetFileVersionInfoW and VerQueryValueW against that system image: all original
52 fixed bytes matched, and numeric evidence was saved privately. This does not
authenticate the extraction location/resource tree or a Pinvou installer/publisher.
The image is never executed or installed and no binary/native dump is committed.

Microsoft [VS_VERSIONINFO](https://learn.microsoft.com/en-us/windows/win32/menurc/vs-versioninfo)
and [VS_FIXEDFILEINFO](https://learn.microsoft.com/en-us/windows/win32/api/verrsrc/ns-verrsrc-vs_fixedfileinfo)
were read privately to check root framing, binary widths and version word order.
The initial actual run failed in 1.101s because a manually written expected
low-order version component was incorrect. Original fixed bytes and the actual
native API agreed on 5415; root corrected the test literal without changing
production behavior. The frozen actual five-group rerun passed in 1.194s with
the fixture group executed. Both initial independent scoped static audits passed
without required implementation fixes.

Complete default Go tests passed with package parallelism 2: windowsversion four
mandatory groups plus one fixture skip, 5.911s; verification 24.081s, PE 1.768s,
Debian 1.753s, compression 9.102s and TAR 2.054s. Fixture/provider/database/browser
opt-ins were not enabled in that default run. Module verification, whole vet,
server build, contract pins, changed-file formatting and diff checks passed;
all root processes closed. After closure, root clarified the signature-checked/
remaining-words-retained comment only; formatting/diff remained clean.

No dependency, SQL, protocol, application version or deployment change. Complete
version-child parsing, original PE resource association, signed package composition,
approved native version/app/publisher mapping, Artifact admission and production
storage remain later work. Full T09 remains incomplete. Downloads have no total/
cumulative elapsed or retry-count limit.

Both final independent scoped code/test audits passed without required fixes.
Dependency audit independently ran all five actual groups in 1.353s;
compatibility audit ran them in 1.177s. Both enabled the original Windows-record
fixture group without skips and passed affected vet, two-file formatting and
diff checks; all independent processes closed. Client architecture and
synchronized-version guards passed. Unchanged database/provider/browser
integration and the previously recorded native API probe were not repeated.

## T09 thirty-ninth batch: complete physical Windows version-block tree

`windowsversion.InspectBlocks` first reparses the same exact owned record through
ObserveFixed, then copies it under separate explicit positive source/copy limits
and parses every nested block's physical framing. Block count, nesting depth and
each original UTF-16 key size including its terminator also require positive
caller capacities. The original uint16 root length bounds the copy before host-int
allocation. These are logical copy/structure capacities, not aggregate heap or
installation/preparation policy.

Every block has positive in-parent length, aligned physical start, a complete
nonempty UTF-16 key with valid surrogate pairing and zero alignment bytes.
Type 0 value lengths are bytes; type 1 lengths are UTF-16 words. Declared values
must fit their original parent and preserve original size/SHA/offset. Unknown
value-unit types return Unsupported. Child cursors advance strictly within parent
bounds, with total-node/depth limits including the root. Exact duplicate sibling
keys are refused. Case variants remain original declarations and do not select
or certify Windows native key-lookup equivalence.

The report preserves complete block and value identities, original keys/order/
type/offsets and the complete tree. Root Source/Value are compared with the earlier
fixed observer's identities. Each bounded subtree receives its own Source hash;
nested bytes may therefore be hashed at more than one depth. No one-scan or
aggregate-heap guarantee is claimed. Any later malformed block, capacity or
cancellation failure returns literal zero BlockRecord with no earlier fixed value
or partial child tree. Orchestration/copy and block/UTF-16 parsing are separate
narrow functions/modules; no input paths or extracted objects are accepted.

Values remain opaque bytes. This is complete physical block framing, not semantic
StringFileInfo/VarFileInfo hierarchy validation, UTF-16 text-value decoding,
translation relationships, company/publisher authentication, native case policy
or PE resource association. It grants no NSIS validation, approved application/
version identity, privileged handoff or Artifact admission. The original fixed
observer retains its intentionally opaque-child behavior and is unchanged.

Five new groups cover binary/text length units, all original node/value/parent
identities, supplementary keys, caller-source/projection isolation, empty values
and preserved case variants. Late malformed length/value/key/zero-padding/
surrogate cases first pass the previous fixed observer, then fail complete physical
block inspection without retaining fixed facts. Exact duplicate siblings and
unknown value-unit types refuse; copy/key/node/depth/source capacities, nil context,
cancellation and same-source restoration have independent controls.

The new actual opt-in group uses the same frozen original 880-byte Windows record.
It checks StringFileInfo/VarFileInfo physical keys, the original language/code-page
table spelling, eight child fields and Translation. Every reported node and value
is independently checked against its exact original parent span, raw header length/
type and original SHA. These keys/counts prove no translation policy or publisher
approval. The previous actual fixed-word evidence also executes; no image is
launched, no native API probe is repeated and no fixture is committed.

Root initial actual windowsversion ten-group run passed in 1.195s, with both
fixture groups executed. Root self-review strengthened recursive original node/
value span and SHA assertions; the frozen ten-group rerun passed in 0.865s without
skips. Both initial independent scoped static audits passed without required fixes.

Complete default Go tests passed with package parallelism 2: windowsversion eight
mandatory groups plus two fixture skips, 1.098s; verification 16.570s, PE 1.736s,
Debian 2.133s, compression 3.896s and TAR 1.429s. Fixture/provider/database/browser
opt-ins were not enabled in that default run. Module verification, whole vet,
server build, contract pins, changed-file formatting and diff checks passed;
all root processes closed. After closure, root grouped standard/internal imports
for readability only; formatting/diff stayed clean.

No dependency, SQL, protocol, application version or deployment change. Semantic
version-tree validation and text/translation policy, original PE resource binding,
current signed version composition, approved native app/version/publisher mapping,
Artifact admission and production storage remain later work. Full T09 remains
incomplete. Downloads have no total/cumulative elapsed or retry-count limit.

Both final independent scoped code/test audits passed without required fixes.
Dependency audit independently ran all ten actual windowsversion groups in 0.981s;
compatibility audit ran them in 1.215s. Both enabled the two original-record fixture
groups without skips and passed affected vet, three-file formatting and diff
checks; all independent processes closed. Client architecture and synchronized-
version guards passed. Unchanged database/provider/browser integration and the
previously recorded native API probe were not repeated.

## T09 fortieth batch: exact declared-path value capture

`windowsversion.CaptureValue` accepts the original owned version record, explicit
physical-block and value-capture capacities, and an exact declaration-key path
below the root. It does not accept a caller-supplied parsed tree. An intrinsically
impossible selector is refused before copying: the source must fit the format's
uint16 root length, and each nested block requires physical bytes. The selector
is copied before context/source callbacks can change caller storage. No path is
a host path; case folding, native Windows key-lookup and field defaults are absent.

The complete same-owned record is freshly checked by InspectBlocks before exact
sibling keys are selected. Zero/multiple matches refuse. The selected original
value span is captured into a separate immutable snapshot under explicit positive
source/memory capacities. Its identity must equal the freshly observed Value ID,
and context is checked before success. Later failure returns literal zero
CapturedValue without earlier fixed values, record/tree, selector or value bytes.
An explicitly empty value retains a real owned empty-byte snapshot; no text or
identity default is inserted.

The result binds complete original record, exact declaration path, selected block
and original raw value bytes. These are byte observations, not decoded CompanyName/
FileVersion/Translation semantics, native case equivalence, approved app/version/
publisher identity, PE resource association or authenticated executable handoff.
Existing fixed/block observers and signed package/PE/CLI APIs are unchanged.
Copy/structure/value capacities retain their logical meaning, not aggregate heap
or preparation budgets, and the 32 MiB business-batch limit does not cap artifacts.

Three mandatory groups cover exact selected identities/raw bytes, independent
caller-source/tree/path mutation, synchronous context-callback selector mutation,
intrinsically impossible long-path refusal before inspection, missing/case-variant/
deep paths, exact duplicate original keys, value source/memory capacities,
cancellation and same-source restoration. Each late capture-capacity negative
first succeeds in complete physical block inspection, then returns no earlier
record. An explicitly empty value is captured without defaults.

The new actual opt-in uses the same fixed 880-byte original Windows version record
and captures FileVersion and Translation as raw bytes. Regular/exact-size checks,
bounded read, close and original whole-record SHA precede inspection. Independent
audit found that the initial expected raw span came from the returned offset,
which could miss an incorrect field selection with the same value type. Root
replaced it with independently measured fixed Path/Key, block offset/size/identity,
value offset/size/SHA and type assertions; expected bytes now use only those frozen
original spans. FileVersion is independently fixed at block 284/110 and value
316/78; Translation at block 844/36 and value 876/4. No text/native lookup or
publisher approval is inferred from these spans.

Root initial actual fourteen-group run passed in 1.217s. Root self-review froze
the selector before callbacks and added its mutation control (1.181s), then bounded
intrinsically impossible selector copies (1.161s). After the audit's actual-selection
P2 repair, final actual fourteen groups passed in 1.237s; all three original-record
fixture groups executed without skips. Both independent static audits passed that
frozen repair with no remaining required fixes.

Complete default Go tests passed with package parallelism 2: windowsversion eleven
mandatory groups plus three fixture skips, 1.749s; verification 16.991s, PE 1.589s,
Debian 2.143s, compression 3.848s and TAR 1.988s. Fixture/provider/database/browser
opt-ins were not enabled in that default run. Module verification, whole vet,
server build, contract pins, changed-file formatting and diff checks passed;
all root processes closed. The later actual-selection test-only repair received
the actual fourteen-group rerun plus affected vet/formatting/diff; production
behavior remained unchanged and no redundant whole-suite rerun was performed.

No dependency, SQL, protocol, application version or deployment change. Semantic
text/translation/native lookup policy, original PE resource association, approved
app/version/publisher mapping, Artifact admission and production storage remain
later work. Full T09 remains incomplete. Downloads have no total/cumulative
elapsed or retry-count limit.

Both final independent scoped code/test audits passed without required fixes.
Dependency audit independently ran all fourteen actual windowsversion groups in
1.245s; compatibility audit ran them in 1.396s. Both enabled all three original-
record fixture groups without skips and passed affected vet, two-file formatting
and diff checks; all independent processes closed. Client architecture and
synchronized-version guards passed. Unchanged database/provider/browser integration
and the previously recorded native API probe were not repeated.


## T09 forty-first batch: original PE Resource-directory byte capture

`pe.CaptureResources` first completely inspects the same owned PE32/PE32+ image
before rereading its original optional-header Resource data-directory declaration.
The declared address is an RVA, not a raw file offset. Directory count and actual
optional-header size are checked before the index is read. Missing declarations
return ResourceUnsupported; half-empty declarations refuse. Original APIs remain
unchanged, and no caller-supplied PE report is accepted.

The supported profile maps the whole nonempty declared interval to headers or one
section's positive VirtualSize/raw-size intersection. Every competing virtual
claim that intersects even part of the interval causes ResourceInvalid, including
zero-fill/BSS claims. Cross-section stitching is never performed: multiple
intersecting claims return ResourceInvalid. Absent backing, zero VirtualSize or
one incomplete raw/virtual backing return ResourceUnsupported; loader defaults
and zero-filled bytes are not invented. Widened arithmetic avoids integer overflow,
and the resource interval must remain in the 32-bit RVA domain: an exclusive end
of 2^32 is valid, while any included byte beyond 0xffffffff refuses.

The entire exact mapped span is captured into a separate immutable snapshot under
explicit source/copy capacities. It retains the complete original image report,
declared RVA and exact file offset. Context is checked before success. Any later
declaration/mapping/capture/cancellation failure returns literal zero Resources
without earlier image or resource bytes. Capacities retain logical copy meaning,
not aggregate heap or preparation/installation policy.

This proves physical original byte association, not a parsed Resource tree,
version-record origin, Windows virtual loading, NSIS format, app/version/architecture
approval, Authenticode or publisher trust. Header-backed declarations and unknown
machine values do not become native qualification. No file extraction, installation,
executable handoff or Artifact admission is introduced.

Four new groups cover PE32/PE32+ original RVA/file-offset/bytes, explicit complete
header backing, caller-source/report isolation, missing and partial declarations,
directory-count omission, zero-fill/incomplete virtual backing, partial competing
virtual claims, capture capacities, cancellation and same-owned restoration.
Each isolated mapping negative first succeeds in the old physical image observer.
The actual opt-in uses the fixed Windows system image, independently measured
Resource RVA 237568, file offset 229376, size 123344 and exact region SHA plus the
original fixed image identity. Expected bytes use that independent fixed range.
No image is executed and no native API probe or binary fixture is committed.

Initial actual PE eleven-group run failed in 1.592s because the test expected
Unsupported for a huge declared interval that also intersects a second virtual
BSS claim; root corrected the expected ambiguity result to Invalid without
changing production behavior. The actual eleven-group rerun passed in 1.566s.
Both earlier image and new resource fixture groups executed.

Independent audits then found a production RVA-domain boundary gap and a zero-fill
test-isolation gap. Root added exact end<=2^32 checking, with old-image positives
before a three-byte interval at 0xfffffffe refuses and its two-byte boundary succeeds.
Independent controls now separately test larger virtual/smaller raw backing and
smaller virtual/larger raw backing; both refuse without image facts. The identical
owned good snapshot succeeds before and after the negative matrix. Root actual
eleven-group repaired rerun passed in 1.543s without skips; complete checks were
rerun because production changed.

Complete default Go tests passed with package parallelism 2: PE nine mandatory
groups plus two fixture skips, 1.679s; verification 15.721s, Debian 1.764s,
windowsversion 1.770s, compression 3.964s and TAR 1.574s. Module verification,
whole vet, server build, unchanged contract pins, two-file formatting and diff
checks passed; every root process closed. Actual PE eleven groups had already
passed in 1.543s with both original-image fixture groups enabled.

Dependency audit independently ran all eleven PE groups in 1.446s; compatibility
audit ran them in 1.588s, both without skips. Both rechecked the frozen repairs
and closed the RVA-boundary and isolated-zero-fill findings. Affected vet,
formatting and diff checks passed; all independent processes closed.

No dependency, SQL, protocol, app-version, deployment or download-budget change.
Full T09 remains incomplete. Resource-tree parsing, version origin, approved
native app/version/publisher mapping, Artifact admission and durable production
storage remain later work. Downloads have no hard total/cumulative elapsed or
retry-count limit. Unchanged database/provider/browser integration and the earlier
native API probe were not repeated.

## T09 forty-second batch: complete original PE resource-tree observation

`pe.InspectResourceTree` freshly captures the same owned image's Resource
directory through CaptureResources, then traverses every declared directory and
leaf using an explicit work stack. It takes no caller-supplied image/resource/tree
projection. Directory and entry offsets are relative to the original Resource
directory; leaf addresses are image RVAs. Leaf data may be outside the metadata
region, but every nonempty leaf must have the same unique complete original
file backing as the earlier resource observer. Empty data retains its declared
RVA plus the empty-byte identity; its observed file offset is zero.

Three required positive capacities bound total entries, directory depth with
root at one, and UTF-16 code units per individual name. Physical span bounds and
entry counts precede child allocation; names have intrinsic uint16 lengths and
are bounded before copying. Iterative traversal does not assume the customary
Type/Name/Language depth and does not grow the call stack on nested input. These
are logical structure capacities: copied paths and repeated shared-leaf hashing
do not prove aggregate heap or one-scan work bounds. No artifact size policy is
inherited from the 32 MiB atomic business-batch limit.

Names precede numeric IDs as declared. Name strings require original word
alignment and valid UTF-16 surrogate pairs; comparison uses case-sensitive
original UTF-16 code-unit order. Numeric IDs preserve the full non-name value.
Each partition is strictly increasing, rejecting duplicate/descending siblings.
An explicit named flag distinguishes empty names from numeric IDs. No case
folding or filesystem path interpretation occurs. Directory reserved flags and
data-entry reserved words must be zero. Other timestamp/version/code-page
declarations remain opaque; timestamps are not trusted time.

Each directory retains its exact original header-plus-entry-table identity.
Every leaf retains its exact declaration path, data-entry identity, original RVA,
image file offset and complete data identity. Shared names/data entries/payloads
are allowed; repeated directory nodes, including cycles, return Unsupported in
this tree-only profile. Unknown payloads remain opaque. Any later malformed
table/name/data, backing, capacity or cancellation error returns literal zero
ResourceTree, including all earlier image/resource/directory/leaf facts.

Four new top-level groups cover exact synthetic named and three-level numeric
paths, independent metadata/data identities, opaque declarations, data outside
the Resource metadata region, empty/shared data, caller mutation and same-owned
restoration, UTF-16 order across supplementary characters, distinct case/empty
names, duplicate/descending IDs/names, partition mistakes, unpaired surrogates,
alignment/bounds/reserved failures, shared/cyclic directories, late unbacked/RVA
domain failures, per-name/entry/depth capacities, pre-cancellation and a 1500-
directory iterative positive with a one-short depth refusal. Framing negatives
first succeed through the unchanged original Resource capture.

The actual opt-in uses the same fixed original Windows image, bounded regular-
file reading and independently measured complete counts: 39 directories, 69
entries and 31 leaves. The unique [16,1,1033] resource has data-entry offset 1640,
image RVA 359704, original file offset 351512 and declared size 884; original
entry/data SHA assertions are independent of returned offsets. The contained
version root declares 880 bytes followed by four original zero bytes. This
observer retains the entire 884-byte resource and does not silently treat it as
the standalone 880-byte version-record observer input. No image is executed.

Root actual fifteen-group run passed in 1.667s. After adding original UTF-16
ascending/case/empty positives plus shared-leaf and shared-directory controls,
all fifteen passed in 1.636s. All three actual fixture groups executed without
skips. Complete default Go tests passed at package parallelism 2: PE twelve
mandatory groups and three fixture skips, 1.675s; verification 15.857s, Debian
6.155s, windowsversion 0.888s, compression 3.997s and TAR 1.156s. Module
verification, whole vet, server build, unchanged contract pins, two-file
formatting and diff checks passed; every root process closed.

No dependency, SQL, protocol, application-version, deployment or download-budget
change. This is original physical resource association, not selected resource
capture, semantic version parsing, Windows loading, NSIS installation format,
approved app/version/architecture, Authenticode/publisher trust or Artifact
admission. Existing APIs remain unchanged and full T09 remains incomplete.
Downloads retain no total/cumulative elapsed or retry-count cap. Unchanged
database/provider/browser integration and the earlier native API probe were
not repeated.

Both final independent scoped code/test audits passed without required fixes.
Dependency audit ran all fifteen actual PE groups in 2.159s; compatibility audit
ran them in 1.544s. Both enabled all three original-image fixture groups without
skips and passed affected vet, two-file formatting and diff checks; all independent
processes closed.

## T09 forty-third batch: exact declared resource-path byte capture

`pe.CaptureResource` accepts the original owned image, explicit image/resource/
tree/value capacities and an exact declared ResourceKey path. Source identity,
positive image byte capacity, selector entry/depth and intrinsic source-size
bounds are checked before cloning. An over-capacity image refuses before selector
copy. Named keys cannot contain numeric IDs, and numeric keys cannot contain
names. The selector is cloned before context/source callbacks can change caller
storage. It is a declaration path, not a filesystem path, native Windows lookup
or a default Type/Name/Language selection.

The complete same-owned Resource tree is freshly checked before any selected
leaf is used. Exact zero/multiple matches refuse; case variants and numeric/name
coercion do not select another declaration. The entire leaf span is captured into
a new immutable snapshot under positive value source/memory capacities. Its
complete identity must equal the freshly checked leaf's identity, and context is
checked after capture and identity comparison. The returned selected leaf path
also owns a separate slice from the returned full tree. Explicit empty resources
retain an actual owned empty-byte snapshot. Every later error returns literal
zero CapturedResource, without earlier PE, Resource tree, leaf or value bytes.

Three new groups cover exact numeric and named selection, selector mutation in a
synchronous context callback, independent source/tree/selected-path mutation,
same-owned restoration, empty values, missing/directory-only/case-variant and
mixed-key selectors, isolated intrinsic source-size and image-capacity negatives
before context callbacks, capture source/memory limits, pre-cancellation and a
valid early selection followed by a malformed later unselected resource. That
late tree failure cannot be hidden by selecting the earlier good leaf.

Independent compatibility audit found a test-proof gap: final cancellation was
originally calibrated using the wrapper's own successful call count, which could
move to an earlier stage if the wrapper's last check were removed. Root changed
only the test. The complete independent InspectResourceTree and capture.Capture
APIs now count their same-source prefix checks using the independently fixed
original span 768/3. The wrapper positive must reach exactly prefix+1, and the
negative cancels at that extra post-copy check with literal zero output. Removing
the wrapper's final poll now breaks the control rather than recalibrating it.
Both independent static audits closed the finding.

The actual opt-in bounded-reads the same frozen original Windows image. The exact
[16,1,1033] leaf is independently fixed at entry offset 1640, image RVA 359704,
file offset 351512 and size 884 with its original full-resource SHA. Expected
bytes use the independently fixed original range. All 884 bytes are retained;
the inner version root's 880-byte declaration is not silently substituted.
No image is executed or native lookup/publisher probe repeated.

Root initial eighteen-group run passed in 1.581s. Isolated source-size and
post-copy cancellation controls passed in 1.617s; the self-review's early image
capacity guard plus its isolated control passed in 1.577s. After the independent
cancellation-proof repair, all eighteen passed in 1.627s with all four original-
image fixture groups enabled and no skips. Affected vet, formatting and diff
also passed; all root processes closed.

Complete default Go tests passed at package parallelism 2 before that later
test-only repair: PE fourteen mandatory groups with four fixture skips, 1.842s;
verification 16.923s, Debian 2.024s, windowsversion 0.976s, compression 3.518s
and TAR 1.177s. Module verification, whole vet, server build, unchanged contract
pins and formatting/diff checks passed. Production remained unchanged after
that full pass, so the later test-only repair received actual eighteen-group and
affected checks without a redundant whole-suite rerun.

No dependency, SQL, protocol, application-version, deployment or download-budget
change. Copy capacities remain logical limits, not aggregate heap/preparation
policy or the 32 MiB atomic business-batch cap. This provides original selected
resource bytes, not semantic version parsing, Windows lookup/loading, NSIS
contents, approved app/version/architecture, Authenticode/publisher trust or
Artifact admission. Existing APIs remain unchanged; full T09 stays incomplete.
Downloads have no hard total/cumulative elapsed or retry-count limit. Unchanged
database/provider/browser integration was not repeated.

Both final independent scoped code/test audits passed without required fixes.
Dependency audit ran all eighteen actual PE groups in 1.439s; compatibility audit
ran them in 1.616s. All four original-image fixture groups executed without skips.
Both passed affected vet, two-file formatting and diff checks; every independent
process closed. The independent-prefix post-copy cancellation finding is closed.

## T09 forty-fourth batch: original version record in current signed PE bytes

`windowsversion.CaptureResourceRecord` reads the original uint16 root length at
the beginning of an owned resource. It captures exactly that inner record into
another immutable snapshot and freshly checks every physical nested block.
The complete resource identity and all bytes after the declared record retain
separate identities. The tail is opaque data, not approved padding, extension or
native validity. A 33 MiB opaque-tail positive confirms that the atomic 32 MiB
business-batch limit does not become an artifact/resource limit. Whole-resource,
record source/copy and block capacities remain explicit, separate logical limits.

The known root framing and 52-byte fixed structure are checked by existing
observers. No CompanyName, translation, native resource lookup or app-version
normalization is inferred. The tail is hashed from the same original snapshot in
bounded 32 KiB reads without copying it into the inner record. Later boundary,
capture, physical-block, tail-read or cancellation failure returns literal zero
ResourceRecord, discarding earlier record/tree/fixed facts.

`Verifier.InspectPEVersionInstaller` composes the existing current signed package/
Root/deny/FullPack/native-byte/PE checks with exact Resource-path capture and that
inner-record observer. Selector count/depth and minimum image/member source/copy
capacity bounds precede path cloning and all context/current-source callbacks.
Original resource-tree image identity must equal the signed installer framing
identity; the version observer's complete resource identity must equal the
freshly selected leaf identity. Final context is checked before success. Every
later failure returns literal zero PEVersionFacts, without earlier signed,
native, PE, resource or version facts.

These observations bind current signed bytes to their original physical version
declarations. They do not map four-part versions to Manifest appVersion, approve
resource types/languages/tails, prove NSIS contents, Windows loading, publisher/
Authenticode identity or grant Artifact admission. No defaults or production
routing are added; existing observers remain unchanged.

Three new resource-record groups cover exact inner/no-tail/zero-tail/opaque-tail/
33 MiB-tail identities, independent original block/value spans, owned-source/
projection isolation and restoration, too-short/overlong roots, invalid fixed
signature, resource/record source/copy/block/key capacities and cancellation.
Three new signed composition groups cover whole-image/resource/record/tail
identity equality, four-part declarations, outer selector freezing under context
mutation, current read references, independent returned projections, later tree/
record/capacity refusals, current deny and same-owned restoration.

Independent audit found missing final-check proof for both new wrappers: initial
controls could not detect deleting their final context checks. Root added test-
only controls. The resource-record prefix independently performs the original
boundary polls and copy/block/tail stages on fixed spans; the signed-composition
prefix uses the existing signed-PE, Resource-capture and record APIs. Each wrapper
positive must reach exactly the independently measured prefix plus one. Each
negative cancels at that extra final poll and returns literal zero. Both restore
the same original input. Five additional isolated outer-selector controls cover
Entries, Depth, image bytes, member source bytes and member copy bytes before
any current-source/context callback. Production behavior remained unchanged.

Actual resource-record coverage uses the independently frozen complete 884-byte
Windows resource, the 880-byte inner record and its original 52-byte fixed-value
identities. Actual signed composition wraps the same fixed original system PE
in isolated strong synthetic package byte claims, then binds the original
[16,1,1033] resource at file offset 351512 to those record/tail identities and
the declared [10,0,22621,5415] words. Synthetic nsis/app/version claims prove byte
continuity only; they do not qualify that image as an installer or publisher.
No image is executed or native API probe repeated.

The initial resource-record test build failed because an int length was passed
to an existing int64 span helper; root corrected only that test argument. Actual
windowsversion seventeen groups then passed in 1.686s with all four fixture groups.
The first signed-composition selector ran two mandatory groups (1.971s) and omitted
the actual group; the corrected exact selector ran all three in 7.557s without
skips. After audit controls, final root actual seventeen passed in 1.773s and all
three composition groups in 2.421s, without fixture skips. Affected two-package
vet, four-file formatting and diff checks passed; all root processes closed.

Complete default Go tests passed at package parallelism 2 before the later
test-only controls: windowsversion thirteen mandatory groups with four fixture
skips, 1.654s; verification 17.342s (its new composition fixture skipped), PE
1.878s, Debian 1.462s, compression 3.527s and TAR 1.432s. Module verification,
whole vet, server build, unchanged contract pins and formatting/diff passed.
No production delta followed that full pass, so repaired actual affected tests
and checks were run without a redundant whole-suite rerun.

No dependency, SQL, protocol, application-version, deployment or download-budget
change. Semantic Windows version/text/translation and approved native identity/
publisher mapping, Artifact admission and production storage remain later work.
Full T09 remains incomplete. Downloads have no hard total/cumulative elapsed or
retry-count cap. Unchanged database/provider/browser integration was not repeated.

Both final independent scoped code/test audits passed without required fixes.
Dependency audit ran actual windowsversion seventeen in 1.552s and the exact new
three signed-PE version groups in 2.257s; compatibility audit ran them in 1.729s
and 1.989s. All four original-record/resource fixture groups and the new actual
composition group executed without skips. Both passed affected two-package vet,
four-file formatting and diff checks; all independent processes closed. Both
independent-prefix final-cancellation findings and early-selector controls are
verified without native qualification claims.

## T09 forty-fifth batch: original ELF64 physical framing

The new `platform/elf` observer reads original owned ELF identification before
decoding a known ELF64 little-endian current-version header. Other classes and
encodings return Unsupported, including a real 52-byte ELF32-sized input; they
are not guessed as ELF64. Ordinary known 64/56/64-byte header/program/section
entry sizes are supported. Extended numbering/reserved index forms return
Unsupported without reading guessed counters or allocating declared extensions.

Three required positive capacities bound original source bytes, program count
and section count. Original table intervals are subtraction-bounded before table
arrays are allocated and table offsets become int64. Each associated nonempty
data span is uint64 subtraction-bounded before its read offset becomes int64;
all such spans are checked before raw hashes begin. NULL program/section entries and NOBITS
sections have no associated file bytes: their other fields remain uninterpreted
declarations, with an actual empty-byte identity. Section index zero must be NULL.
Empty table offsets remain declarations rather than invented pointers.
Legitimate shared program/section file spans are allowed and independently hashed;
this is not a one-scan or aggregate heap guarantee.

The result preserves original header, complete program/section-table identities,
every individual entry identity and backed data identity, plus all observed
type/machine/ABI/entry/flags/name-index/link/info/virtual-size/alignment declarations.
Unknown machine/ABI and unused NULL fields do not become platform certification.
Section names, string lookup, virtual loading/alignment, imports/relocations,
compressed-section expansion, app identity, publisher trust and execution are
outside this physical profile. Overlays remain in the complete source identity.
Any late failure returns literal zero Report without earlier headers or spans.

Four groups cover original declarations and exact original metadata/payload
identities, source/projection isolation and same-owned restoration, shared backed
aliases, NULL/NOBITS maximal uint64 declarations without raw reads, absent tables,
unknown machine/ABI, class/encoding/version/header widths, extended counts,
invalid table/string indices, maximal/late truncated spans, explicit capacities,
pre-cancellation and a 33 MiB overlay positive. A final-cancellation negative uses
the independently fixed original read/hash prefix plus one; the successful
observer must reach that exact extra poll, and cancellation there returns literal
zero. Removing its final poll cannot recalibrate the test to an earlier stage.

The actual opt-in comes from the previously fixed public Debian hello data TAR.
Independent Python reads only its original regular binary member and writes a
private fixture; no package extraction into a host namespace or execution occurs.
The original ELF is fixed at 31448 bytes and SHA 1aab5d66fba9313733ca534dc9693f262532ab696eb9d29cc70978c5e1c7078c.
Bounded Open/Stat/LimitReader/Close and exact original size/SHA precede inspection.
Go debug/elf independently compares every declared program/section field and raw
payload interval; fixed original offsets supply header/table/entry/name-index
identities. It contains machine 62, thirteen program headers and thirty sections.
No fixture binary, tool output or private host path is committed.

Root actual four groups first passed in 1.448s. Self-review added index-zero NULL,
independent final-cancellation and complete actual entry/table/name-index identity
controls (1.083s), then identified ELF32 using its original 16-byte identification
before requiring an ELF64-sized header. Final actual four passed in 1.491s with
the original ELF fixture enabled and no skips.

Complete default Go tests passed at package parallelism 2: ELF three mandatory
groups with one fixture skip, 1.976s; verification 18.589s, PE 1.841s,
windowsversion 6.825s, Debian 1.589s, compression 4.219s and TAR 2.031s. Module
verification, whole vet, server build, unchanged contract pins, two-file
formatting and diff checks passed; all root processes closed.

No dependency, SQL, protocol, application-version, deployment or download-budget
change. Current signed Debian-to-original-ELF selection and approved native app/
architecture/publisher identity remain later work. Full T09 stays incomplete.
The 32 MiB business-batch limit does not constrain artifact bytes. Downloads have
no hard total/cumulative elapsed or retry-count cap. Unchanged database/provider/
browser integration was not repeated.

Independent compatibility audit found that the original short Class32 control
was a truncated ELF64 fixture with its class tag changed, not a complete ELF32
header. Root changed only that test to a genuine 52-byte ELF32 layout, independently
accepted by Go debug/elf as Class32 with no program/section tables before our
observer returns Unsupported and literal zero. Root repaired actual four groups
passed in 1.684s without skips, with affected vet, formatting and diff checks;
all processes closed. Production was unchanged after the complete default pass,
so no redundant whole-suite rerun was needed for this test-only repair.

Both final independent scoped code/test audits passed without required fixes.
Dependency audit initially ran actual four in 1.722s before the ELF32 evidence
repair, then reran the final repaired four in 1.418s. Compatibility audit ran the
final four in 1.570s. All four executed, including the original public ELF fixture,
without skips. Both passed affected ELF vet, two-file formatting and diff checks;
all independent processes closed. The genuine ELF32 control finding is closed.

## T09 forty-sixth batch: exact original TAR regular-member capture

`tar.CaptureRegular` freshly inspects the complete same owned TAR, including every
later member, both end blocks and the full zero trailer, before selecting exact
declaration bytes. The name is an immutable string value, not a host path.
Ordinary USTAR's actual maximum 155-byte prefix plus slash plus 100-byte name
allows 256 bytes; longer selectors cannot match this unextended profile.
No prefix stripping, case folding, normalization, host extraction or link
following is introduced, and no approved application path is assumed.

Zero or multiple exact name matches refuse. Uniqueness covers all declaration
types, so a regular file competing with a directory/link cannot be selected.
Only an ordinary raw regular type is copied. The legacy zero type remains
accepted as a file, but its trailing-slash directory form refuses rather than
becoming an empty executable. A selected empty regular file still receives a
real owned empty-byte snapshot.

The original bounded member span is copied directly from the same immutable TAR
into another owned snapshot under explicit source/memory capacities. Its complete
identity must match the freshly observed member identity, followed by a final
context check. Every later archive/name/type/copy/identity/cancellation failure
returns literal zero CapturedRegular, without earlier archive/member/value facts.
Copy capacities remain logical limits, not aggregate heap/preparation policy.

Three new top-level groups cover independent exact ./ versus plain declarations,
case/missing/impossible selectors, source/archive/member projection isolation and
same-owned restoration, regular/legacy/empty values, duplicate regular and
regular/directory competitors, symbolic/hard-link refusal, legacy-directory
refusal, later unselected damaged headers, source/copy capacities and final
cancellation. That negative independently completes the original Inspect and
fixed 512/3 capture prefix, then cancels at exactly the wrapper's extra final
poll; removing that poll breaks the positive/negative controls. The full 256-byte
ordinary USTAR name is a positive, and a 33 MiB captured payload does not inherit
the 32 MiB atomic business-batch limit.

The actual opt-in uses the independently frozen 256000-byte public hello data
TAR, with bounded regular-file reading and exact complete SHA before inspection.
Independent Python measurements fix 143 ordinary members and the exact
./usr/bin/hello declaration at payload offset 2048, size 31448, raw regular type,
mode 493 and SHA 1aab5d66fba9313733ca534dc9693f262532ab696eb9d29cc70978c5e1c7078c.
Expected payload bytes use that independent original range, not a returned
offset. No file is extracted into an installation namespace or executed.

Root actual twelve TAR groups passed in 1.654s, then in 1.666s after maximum-name
and 33 MiB owned-capture positives. Both original fixture groups executed without
skips. Complete default Go tests passed at package parallelism 2: TAR ten mandatory
groups with two fixture skips, 3.987s; verification 18.040s, ELF 1.478s, PE 1.616s,
windowsversion 2.386s, Debian 2.266s and compression 3.922s. Module verification,
whole vet, server build, unchanged contract pins, two-file formatting and diff
checks passed; all root processes closed.

No dependency, SQL, protocol, application-version, deployment or download-budget
change. This is original ordinary-file byte selection, not installation path
safety, executable/native identity, approved app mapping or Artifact admission.
Current signed Debian-to-original-ELF composition and approved platform/app/
publisher mappings remain later work; full T09 stays incomplete. Downloads have
no hard total/cumulative elapsed or retry-count cap. Unchanged database/provider/
browser integration was not repeated.

Both final independent scoped code/test audits passed without required fixes.
Dependency audit ran all twelve actual TAR groups in 2.082s; compatibility audit
ran them in 6.599s. Both enabled both original fixture groups without skips and
passed affected TAR vet, two-file formatting and diff checks; every independent
process closed. No production or test changes followed the frozen audit runs.

## T09 forty-seventh batch: original ELF member in current signed DEB

`InspectELFDebianInstaller` first completes the existing current signed
FullPack/native-DEB, required control identity and both complete TAR inspections.
It then freshly captures one exact unique ordinary data-TAR member from the same
owned decoded data, followed by ELF inspection on that same owned member.
Complete TAR and member identities are compared across those boundaries; the
final context check follows all observations. Every later error returns literal
zero composite facts. Existing APIs remain unchanged.

The immutable name must be nonempty and at most 256 bytes. No default binary
location, normalization, case folding, prefix stripping, extraction or link
following is introduced. All framing, decompression, TAR, control, member-copy
and ELF capacities are explicit. Raw control/ELF declarations are not approved
architecture, app, version or publisher mappings and do not grant admission.

Two mandatory groups cover byte continuity and owned/report isolation, prior
signed identity positives before exact ELF framing/class/table negatives, exact
name refusal, independent member source/memory and ELF byte limits, current
deny/restore, and final cancellation. That final negative completes the three
original APIs independently, then cancels at exactly the wrapper's extra poll;
it does not calibrate against a successful wrapper invocation.

The actual opt-in reads the frozen 53080-byte public hello DEB with a bounded
regular-file read and independent complete SHA. Its original 256000-byte data
TAR contains ./usr/bin/hello at offset 2048, size 31448, SHA
1aab5d66fba9313733ca534dc9693f262532ab696eb9d29cc70978c5e1c7078c.
The same member yields machine 62, thirteen programs and thirty sections.
Synthetic strong package claims demonstrate continuity only, not Pinvou identity.
No original program is executed.

The initial actual test refused XZ because its test budget omitted explicit XZ
capacities. The test now specifies the previously measured 8 MiB dictionary and
eight block/stream bounds; production refusal/default behavior was unchanged.
Root final three actual groups passed in 2.519s without skips. Complete default
Go tests passed at package parallelism 2; verification 18.362s, ELF 2.597s,
TAR 3.217s, Debian 1.697s, PE 6.211s and compression 4.935s. The new actual opt-in
skips in default runs and was executed separately. Module verification, whole
vet, server build, unchanged contract pins, two-file formatting and diff checks
passed; all root processes closed.

No dependency, SQL, protocol, application-version, deployment or download-budget
change. Full T09 remains incomplete: approved native identity, publisher trust,
capability evidence, immutable admission/storage and upload workflows remain
required. Downloads have no hard total/cumulative elapsed or retry-count cap.
Unchanged database/provider/browser integration was not repeated.

Both independent scoped code/test audits passed without required fixes.
Dependency audit ran only the new three actual groups in 7.253s; compatibility
audit ran them in 2.058s. All three, including the original hello DEB, executed
without skips. Both passed affected verification vet, two-file formatting and
diff checks; every independent process closed. No source/test changes followed.

## T09 forty-eighth batch: standalone original ELF inspection command

`cmd/upgrade-elf-inspect` is a complete read-only command for the existing
ordinary ELF64 little-endian physical observer. Application adaptation remains
in the command; original parsing remains in the feature's ELF adapter.
It opens a supported descriptor through the existing OS sourcefile adapter,
captures complete owned bytes and observes that snapshot, followed by an extra
final context check before JSON output. The input program is never executed.

All four capacities are required positive arguments: source-bytes,
snapshot-memory-bytes, programs and sections. There are no implicit production
capacities. Source capture retains the existing Windows actual-volume/reparse/
device restrictions and Unix last-symlink/nonblocking rules; this is not a
protected execution-object handoff or filesystem-locality proof on Unix.

Argument errors exit 2; source, inspection, cancellation and output failures
exit 1; a complete successful report or help exits 0. Diagnostics are fixed
and do not echo input paths, command arguments or underlying I/O/provider errors.
Inspection/cancellation refusals emit no JSON. A failing output device may have
accepted partial bytes, but short/error/negative/overlong write results are
sticky and can never report success; help uses the same checked writer.

Five mandatory groups independently compare the complete decoded report with
Go's ELF reader and original raw header/table/program/section byte identities,
including NULL/NOBITS absence of file data. They cover owned snapshot path
replacement, every missing/nonpositive/overflow capacity, invalid arguments/help,
missing/directory input, each source/memory/program/section bound, invalid/class/
late section-span refusal and original restoration. Final cancellation completes
ownedSource plus the original ELF API independently, then targets only the CLI's
extra poll with exact positive/negative counts. Failed writers and absent writers
also refuse; no self-calibration or approval policy is added.

Two separate opt-ins verify the independently frozen 31448-byte public hello ELF
and a privately built command through actual child processes. The original file
is bounded-read and checked against its complete known SHA. Child E2E covers
synthetic success, invalid arguments, invalid framing, same-path restoration and
the original hello file, comparing all complete report fields and raw spans.
Its 60-second child test timeout is a local test bound, not a download deadline.

Root default five mandatory groups passed in 1.419s with both opt-ins skipped.
The freshly built command then passed all seven in 3.071s, child E2E 1.82s,
with both opt-ins executed and no skips. Complete default Go tests passed at
package parallelism 2: the new command 1.980s (five mandatory, two skips),
verification 17.763s, ELF 1.407s and sourcefile 1.467s. Module verification,
whole vet, server build, unchanged contract pins, two-file formatting and diff
checks passed; all root Go processes closed.

Dependency audit passed all seven actual groups in 3.099s, child E2E 1.67s;
compatibility audit passed them in 8.054s, child E2E 6.67s. Both enabled both
opt-ins without skips and reused the frozen already built read-only command.
Both passed affected command vet, two-file formatting and diff checks; all
independent processes closed. Both scoped code/test audits found no required
fixes and no source/test changes followed their frozen runs.

No dependency, SQL, protocol, app VERSION, deployment or download-budget change.
This command grants no native loading, app/version/architecture/publisher
qualification, installation permission or Artifact admission. Full T09 remains
incomplete. Downloads retain no total/cumulative duration or retry-count cap;
unchanged database/provider/browser integration was not repeated.

## T20 explicit original no-update acceptance command (2026-10-15)

The separate service adds `upgrade-events accept-no-update` to its existing
fixed owner commands. It requires the enabled runtime connection, original
independent T06 encryption key and explicit UPGRADE_EVENT_PRODUCT. It takes one
original signed no-update request from standard input, never a caller-supplied
projection, event disposition, SQL command or recovery frame. Existing
provisioning, recovery and maintenance actions retain their behavior.

Before database work, the owned input is read through an explicit 1 MiB-plus-one
bound and existing strict canonicalization, rejecting incomplete/duplicate/
multiple/invalid JSON and over-cap input without exposing contents. Cancellation
closes the owned descriptor exactly once and joins the close callback. Both an
in-process pipe and a real native descriptor have read-start cancellation
controls; they do not certify console/POSIX signal behavior.

Actual acceptance reuses the unchanged NoUpdateOwner and T06 registration.
Current signed Root/deny, original contract/lineage/sequence/windows and final
private guards remain mandatory before one atomic lifecycle/receipt/outbox and
operation completion. Only a real success or currently lawful replay prints the
fixed acknowledgement. Short, invalid-count or failed output remains a failure
and explicitly says acceptance may have committed. No event/credential/product/
provider text or private connection configuration is printed.

Root unit checks passed five command groups in 0.788s with two actual-helper
opt-ins skipped; configuration's five groups passed in 0.536s. The command and
its test helper were built privately. The first actual database invocation
refused both cases in 2.157s because the original private local test database was
stopped; it is not called a pass. The same local test instance was restored,
without production deployment or policy changes. The repaired actual two cases
then passed in 27.007s, without skips.

An independent P2 finding identified an administrator negative before writer
enablement, which could be masked by missing capability. The primary role moved
it after enablement, changing only the actual login with the original good
request/product/key, asserting exact readiness refusal and zero acceptance
before restoring runtime. Actual E2E also checks read-only authority, product/
key/input/signature refusal, same-original replay, immutable receipt, joined
no_update Session revision two, current key-deny refusal and restored replay.
Receipt, outbox and committed operation counts remain one. The second actual
case injects stdout failure after real owner completion, verifies all three
committed facts, then uses a fresh normal command to replay the original request
without another accepted event or receipt rewrite.

No SQL migration, dependency, protocol, application VERSION, provider or
download-budget change. This is a fixed internal no-update reception workflow,
not HTTP batching, another event role, protected client outbox, accepted-history
deletion, quality qualification or full T20 completion. Accepted-data deletion
remains deferred until its consumer/archive and diagnostic retention policy is
defined. Deployment remains deferred. Downloads retain no total/cumulative
duration or retry-count cap.

Complete default Go tests passed at package parallelism 2: command 1.299s,
configuration 1.367s, events 4.374s, event PostgreSQL adapter 2.138s, metadata
PostgreSQL adapter 2.550s and HTTP 7.185s. Real database/provider opt-ins skip
by default; the two new actual cases and committed-output helper ran separately.
Module verification, whole vet, server build, unchanged contract inventory,
six-file formatting and diff checks passed; all root Go processes closed.

Dependency audit passed command five in 1.845s (two helper skips), configuration
five in 6.000s and actual PostgreSQL two in 19.250s (11.08s / 6.51s), without actual
skips. Compatibility audit passed command five in 2.053s (two helper skips),
configuration five in 0.619s and actual PostgreSQL two in 19.992s (11.65s / 6.76s),
without actual skips. Both reused the frozen privately built command and helper,
passed affected three-package vet, six-file formatting and diff checks, and
closed every process. Both final scoped code/test audits passed after the
primary role's P2 test isolation repair; production owner/SQL behavior was unchanged.

## T05 isolated actual Keycloak acceptance (2026-10-15)

The separate service adds an explicit opt-in real-provider harness and one Go
acceptance test. Production authentication, permissions, sessions and dependency
versions are unchanged. Official Keycloak 26.8.1 and Temurin Java 21.0.12.1+1 run
in a fresh owned temporary runtime with an in-memory realm and synthetic
identities. Only the official library tree is copied; source configuration,
data and custom providers are excluded. The harness downloads nothing, uses
explicit runtime paths and never prints provider logs, tokens or credentials.
It provisions no service management principal and performs no deployment.

The actual flow uses S256 PKCE and the unchanged production OIDC adapter.
Password-only authentication cannot satisfy the dedicated fresh-MFA guard.
Empty and wrong OTP cannot complete authorization; correct OTP produces the
verified dedicated ACR and current signed authentication time for the same actor.
Actual introspection must pass before logout. Authorization-code replay must
be refused; successful actual provider user logout must make subsequent session
introspection refuse. Five-minute equality tests
calculate from the actual signed timestamp, without claiming five minutes of
wall-clock waiting or production-realm certification.

Early real runs exposed fixture mistakes: the launch path/relaunch contract,
local HTTP Secure-cookie handling, missing standard basic scope and missing
exact client audience. All were fixed within the isolated harness. The standard
basic/acr scopes and precise access-token/introspection audience mapper retain
Keycloak's current checks; no compatibility bypass or backend relaxation exists.
A test-only Secure-cookie exception is confined to the exact private HTTP
127.0.0.1 origin. Actual CookieJar controls preserve port/host, domain, path,
expiry and normal HTTPS behavior.

Independent review found two P2 issues, repaired by the primary role. The helper
now owns its bound socket before realm configuration, including configuration
failure. Its context waits for real active handlers before returning. The
handler regression holds that handler blocked through a controlled wait;
a single-variable block_on_close=false negative implementation produced exactly
the intended failure, while the normal four local groups passed in 0.851s.
SDK-log-derived diagnostics were also removed instead of claiming general
secret-safe redaction.

Root final actual acceptance passed in 2.892s (case 1.27s), without skips.
Complete default Go tests passed at package parallelism two; OIDC 2.007s,
management PostgreSQL 2.818s and HTTP 2.154s. The new actual opt-in skips by
default and was enabled separately. Module verification, whole vet, server
build, unchanged contract pins, formatting and diff checks passed; root's
processes closed before independent executable reviews.

Dependency review passed all four local groups in 0.844s and actual Keycloak
in 3.146s (case 1.30s), without skips. Compatibility review passed all four local
groups in 0.841s and actual Keycloak in 3.582s (case 1.81s), without skips.
Both also passed the existing two default OIDC groups with the actual opt-in
explicitly skipped there (1.742s / 1.785s), affected vet, formatting and diff
checks. Both independent final code/test reviews passed after the primary role's
repairs. All helper, Python, Go and Java processes closed and owned temporary
runtime cleanup completed before each role released execution.

This closes the local actual-provider evidence gap for these exercised T05
flows. It does not approve the deployment's realm, enrollment, TLS ingress,
administration, HA or broader T05 certification. Deployment remains deferred.
No database migration, protocol, app VERSION or runtime dependency changes.
Downloads retain no hard total/cumulative elapsed or retry-count cap; local
acceptance timeouts do not apply to product downloads.

## T20 fixed late transaction acceptance and recovery (2026-10-15)

The separate service adds one bounded internal workflow for genuine original
`preinstall-transaction-event`, `install-transaction-event` and
`activate-transaction-event` requests. It accepts only late diagnostics; live
transaction candidates are refused before a reservation. Actual original
Session, authorization and transaction lineage, full original allowed events,
current signed Root/role-specific deny and original upload windows remain
mandatory. Neither terminal transactions nor elapsed report windows are revived.

An independently initialized `event_transaction_reception` namespace owns only
this command's immutable intents and attempt bindings. The existing five event
reception migrations, their digests and native function definitions remain
unchanged. Receipts, original-event outbox and transaction diagnostic cursors
remain in the existing event ledger. Initialization is read-only; explicit
enablement grants only the two fixed owner ports. Existing no-update rights are
preserved. Every reachable runtime role must have the whole pair or neither.

One registered T06 OwnerReplay command seals compact original claims, hashes and
complete actual read references. Original credentials and request packets never
enter durable recovery projections. Recovery rechecks original immutable points,
current trust and complete actual lineage; it refreshes mutable CAS values without
renewing proof or deadlines. Go and native SQL both calculate the historical edge
using T44's pure planner. Receipt, original outbox, cursor and T06 completion
commit together. Original Session/authorization/transaction/staged/active state
and quality watermarks remain unchanged. Native epoch zero can return only an
existing receipt for the exact already committed operation; it cannot reserve
or recreate missing accepted data.

The explicit `upgrade-transaction-events` command supplies initialize, enable,
single original-input acceptance and original-request-free recovery. Only a
complete committed acceptance/current lawful replay prints the fixed ACK.
Failed or lost output can follow a real commit and must reuse the original
identity. Cancellation owns and closes stdin; recovery joins work before pool
closure. Windows test process termination proves isolation/joining, not native
console interrupt or POSIX signal certification. Deployment remains deferred.

Independent runtime review reproduced two P2 issues in actual PostgreSQL:
changed native CHECK constraints retained readiness and partial enablement
silently repaired a missing port. The fixes verify all ten exact native CHECK
definitions/attributes, complete constraint counts, column defaults/identity/
generated attributes and rewrite rules, and reject a half-pair in any current or
SET-reachable role. Actual 25 catalog/permission controls and restoration passed;
the other reviewer independently checked the fixes. Two P2 test defects were
also repaired: final-clock controls now observe all three exact candidate rows
inside the still-open transaction, and replay-only refusals assert their exact
reason so another historical-edge failure cannot mask a missing guard.

Root's complete new actual PostgreSQL suite passed all 18 top-level groups in
276.957s, without skips. It includes all three roles, terminal/report-window late
input, live refusal, current deny before unavailable receipt, ephemeral owner
isolation, exact native catalog/ACL drift, concurrent identity/conflicting bytes,
corrupt cursor, receipt preservation, positive-epoch and replay final-clock
faults, atomic Apply-before-Complete rollback, natural-lease sealed recovery,
actual CLI initialization/enablement/acceptance/replay, real committed-output
loss and abrupt-process exit followed by a fresh normal recovery command.
That recovery process receives only the runtime connection and original T06 key,
not the original packet, credential, role or product. It commits the original
result under a higher fenced epoch without duplicating the accepted write set.

Root also passed 14 existing actual regression groups in 216.033s, without skips:
original no-update command/lost output, explicit provisioning and authenticated
recovery, natural-lease recovery, lost commit ACK, legal signature variants,
live/late owner behavior, unaccepted maintenance protection, accepted retention
inspection, and existing transaction observation/cursor upgrades and guards.
These runs used freshly built current command/test executables. Full default Go
tests passed with package parallelism two (new command 1.602s, events 3.935s,
events PostgreSQL 1.985s, metadata PostgreSQL 2.173s). Actual database/provider
opt-ins skip by default and were run separately where described above.
Module verification, whole vet, server/both command builds, unchanged contract
inventory, formatting and diff checks passed. All root Go processes closed
before independent executable review.

Both final independent scoped code/test audits passed without new mandatory
findings. Dependency review independently reran all 18 actual groups in 274.482s
with zero skips; compatibility review reran all 18 in 271.857s with zero skips.
Each also passed all five affected default packages, affected vet, all 27 changed
Go files' formatting and diff checks. Their default runs explicitly clear actual
opt-ins and do not replace the separate actual runs. The catalog/permission
repairs and both fault-test repairs passed the other role's independent review.
No reviewer edited the frozen result during final validation. All root/reviewer
Go, helper and recovery processes closed before commit preparation. Client
architecture, unchanged application version 0.10.9 and diff checks passed.

The fixture uses actual protected storage and genuinely signed event credentials;
it does not certify T19 authorization issuance or physical client execution.
Public atomic batching, live event transitions, task-owned events, protected
client outbox, quality contribution and full T20 remain unfinished. Accepted
history deletion awaits its consumer/archive and diagnostic retention policy.
No app VERSION, protocol contract, dependency or download-budget change.
Downloads retain no hard total/cumulative elapsed or retry-count cap.


## T20 fixed live transaction acceptance and recovery (2026-10-15)

This continuation extends the separate service's committed fixed late receiver
with one complete internal single-event transaction workflow. The new explicit
`enable-transaction`, `accept-transaction <role>` and `recover-transaction` actions
use `upgrade.events.transaction.v1`; the original four late-only actions and
`upgrade.events.late-transaction.v1` retain their scope and operation identity.
All three transaction roles support live local-state events, permanent receipt
replay and original-request-free sealed recovery. This does not complete T20.

The original signed input, current Root/role denial, full allowed events and
actual original Session/consumed authorization/transaction remain mandatory.
T44 owns every lifecycle write: ordinary advance, preinstall staged completion,
activation terminal/staged composition and safe activation cancellation. The
staged completion identity comes from the server-owned T06 result. Safe cancel
does not automatically rebuild waiting or grant fresh qualification. Original
activation association remains verifiable after completed/cancelled content or
the exact old reporting timeout, using the original consumption time. Actual
staged/transaction terminal pairing is checked before any local projection.

Version-two migration adds fixed guards/ports only in the independent transaction
namespace. Existing five reception migrations and transaction version-one SQL
bytes/digest remain unchanged. Original intents/attempts and permanent accepted
data are shared. Explicit initialization upgrades the namespace and removes
both transaction writer pairs for the named runtime; each is explicitly enabled
again. Startup/enable/recovery never implicitly upgrade. Every reachable role
has each known pair in full or neither; OID checks are search-path independent.
No runtime receives direct lifecycle writes or private T44 helper privileges.

Live T44 changes, immutable receipt/original outbox and T06 completion share one
transaction. Late diagnostics use the existing pure planner/cursor and do not
change original lifecycle state. After complete live or diagnostic readback,
the same fixed native port rechecks the existing receipt/outbox, original trust,
association and windows using the original positive processing epoch. Only live
lifecycle writes advance the CAS revisions. Returned receipt bytes must match.
Pure time crossing can change live/late classification without changing read
references; the candidate/native disposition mismatch rolls back as Changed,
requiring a fresh Read/Compute attempt rather than silent reinterpretation.

Different fixed commands may own distinct T06 results/attempts but converge on
one permanent receipt/outbox/cursor, preserving original disposition and time.
Retries of the same authenticated command/proof/request attempt preserve its
result; fresh proof/request attempts can have distinct T06 results while retaining
the permanent receipt. Native epoch zero remains exact
committed-operation read-only replay and cannot reconstruct missing acceptance.
Value-copied facades have distinct ephemeral original-input context identities.

Independent static review found and the primary repaired core-copy identity,
historical terminal pairing, search-path-dependent function-name matching, and
post-readback original-window gaps. The terminal-pair negative independently
proves StagedValid remains true, exact Go/native rejection, unchanged protected
fingerprints and restored original replay. Removing the Go pair check gives the
required red (7.61s). Window faults are independently triggered after actual
cursor Scan or complete live inventory readback. Both missing final seal (5.87s)
and seal-before-readback (5.79s) wrongly accept and are detected as red; the
restored four live/late original-expiry/short-Root cases pass (25.71s). This avoids
a fault triggered by the tested guard itself masking incorrect guard placement.

Complete default Go tests, module verification, vet, server/both event-command
and helper builds, unchanged contract pins, formatting and whitespace passed.
Affected default package times: transaction command 6.681s, existing event command
1.071s, events domain 3.884s, events PostgreSQL 2.162s, metadata PostgreSQL 2.293s,
lifecycle domain 2.578s and lifecycle PostgreSQL 1.793s. Actual provider/database
opt-ins remain skipped by default and are executed separately below.

Root's frozen new actual suite passed all 17 top-level groups in 363.770s,
without skips. It includes all three live state sequences with per-edge replay,
real CLI role/product/refusal boundaries, safe cancellation, completed original
history and exact legacy timeout, time-only classification drift, copied owner
context isolation, fixed-pair/search-path checks, native epoch-zero refusals,
shared permanent receipts across command identities, concurrent same-event
acceptance/conflicting bytes, Apply-before-Complete rollback, sealed fresh-owner
recovery, lost committed output and abrupt process loss with a fresh normal
recovery command. Actual original-expiry and independently shorter-Root faults
cover live and diagnostic post-readback rollback. A real version-one pending
operation survives explicit version-two installation and recovers from its
original sealed frame without new original input or proof.

The initial 32-group existing actual regression invocation passed 31 groups and
exposed one generated-column test fixture that incorrectly reused the sole v1
migration digest for the new v2 row. The primary repaired only that test: preserve
and independently verify each original digest during the generated-column
mutation, then restore the ordinary non-defaulted column and each version's
exact original value. Its complete ten-case catalog group passed separately in
16.49s (package 17.497s), including final runtime and lifecycle restoration.
The other 31 unchanged groups retain their passing evidence from that invocation
(543.924s); a passing combined rerun is not claimed. They cover the old late
receiver/CLI/epochs/catalog/identity/current-deny and original no-update command,
atomicity, sealed recovery, lost ACK/signature variants, maintenance and existing
transaction observation/cursor guards. No production change followed the frozen
17-group run. Full default checks passed again after the test-only repair.

Both final independent scoped deep reviews passed, including the test restoration
repair, without remaining required fixes. Each independently executed all 18
actual top-level groups (the 17 new groups and repaired complete catalog group)
without skips: dependency review 453.359s, compatibility review 410.525s.
Each also passed the five affected default packages with actual opt-ins cleared,
affected vet including the lifecycle adapter, unchanged pinned contract bytes and
inventory, all 29 changed Go files' formatting and diff checks. No reviewer
edited the frozen source/tests. All root and reviewer test, helper and recovery
processes closed. Client architecture, unchanged version 0.10.9 and diff checks
passed. These results accept this bounded transaction workflow, not full T20.

Actual fixtures use protected storage and genuinely signed event credentials
with synthetic original consumed records. They do not certify T19 issuance,
physical client execution, qualified successor reconstruction or deployment.
Public atomic multi-event HTTP, remaining live progress/task profiles, protected
client outbox, quality contribution, accepted-history deletion policy and full
T20 remain unfinished. Deployment remains deferred. No application version,
protocol contract, dependency or download-budget change is introduced. Downloads
retain no hard total/cumulative duration or retry-count cap.

## T17 protected logical workflow prerequisite (2026-10-15)

Independent prerequisite review found that general update-Session event
acceptance cannot use T44 Session edges alone. The original model also requires
the actual T17 workflow and current writable Session, atomic original-hop
confirmation, permanent original download-failure latch and ended/handoff
cutoffs. The service has no production qualified workflow creation/handoff
owner yet. General Session reception remains refused rather than using an
invented workflow projection. Both independent roles passed the bounded kernel
plan after the primary clarified complete resume preservation and distinct
logical versus existing opaque lifecycle context identities.

The new separate service feature `upgradeworkflow` derives exact original
workflow context, complete selection identity and four-field hop identity from
already authenticated decision claims. Closed pinned Schema and intrinsic
semantics are repeated; signature/current eligibility are not inferred. The
protected record supports original ongoing/failed facts, complete optional
downloadResumeBinding, first failure occurrence/server end/event/Session, original
stage/group/content, confirmation and explicit positive loss interval. Unknown
observation never creates a failure or refreshes progress. Logical downloads
have no total/cumulative duration or retry-count cap.

An independent namespace provides protected canonical workflow bytes and T17
original Session associations. Each association preserves the actual T44 opaque
context digest and separate logical context digest, scope/workflow/Session/install,
original start/channel/type/purpose/hop/final identity. Reads require actual
Session plus complete T44 scope inventory and full read references; missing or
altered associations fail closed. Explicit provisioning grants runtime reads
only, with no private execution or direct/SET-reachable write authority. Startup
checks exact native definitions/attributes, table shape/defaults/constraints,
keys/indexes/owners/rules/triggers, migration and actual connection/isolation.
Existing T44, T06 and event migration bytes remain unchanged.

Private SECURITY INVOKER confirmation/failure primitives require real T06 owner
epoch/lease/deadline and actual scope/Session/workflow CAS. Confirmation follows
the actual T44 confirmed edge and current writable Session. A lawful old Session
failure can permanently end its ongoing workflow after handoff without changing
that Session or granting new rights. First failure bytes/time/revision remain
unchanged for later lawful observations; original event receipt replay still
belongs to the future event owner. Actual tests compose T44/workflow/T06 through
a clearly isolated test definer, not a production event or decision command.

The primary repaired a native optional-array append error exposed by actual
handoff tests. Subsequent self-review added original handoff/acceptance chronology:
pure negative controls originally accepted both confirmation/failure (1.111s),
and the real native confirmation returned a candidate while failure refused only
later at the wrong guard (package 8.325s, group 6.03s). Both sides now refuse the
original pre-handoff acceptance explicitly. Compatibility review found SQL NULL
and out-of-range final clock gaps. All initial/final clocks now require an
explicit safe non-null value and nondecreasing time. Removing only the final
confirmation NULL guard wrongly returns a real candidate; the independent
post-write fault detects that red (package 4.701s, case 2.52s), then the original
source is restored byte-for-byte. Tests cover both initial NULL/unsafe and actual
post-write NULL/unsafe/backward clocks, complete rollback and restored positives.

Independent Node evaluation rejected five terminal line-separator publisher
counterexamples in the original Schema while intrinsic semantics stayed valid.
The new Go original-claims/context and stored-record paths reject all five too;
no Schema or regular-expression behavior change was needed. Domain context,
selection and hop goldens were independently computed with the pinned original
JavaScript model. Actual Go/native agreement includes the same counterexamples.

Root's frozen actual suite passed all 12 top-level groups without skips in
80.831s. The original handoff guard group passed in 6.02s, and the ten independent
bounded-clock cases passed in 30.84s. Complete default Go tests passed with all
actual opt-ins cleared; workflow domain's eight groups passed in 1.645s, contracts
2.266s and metadata PostgreSQL 7.260s. Module verification, whole vet, server
build, unchanged pinned contract bytes/inventory, formatting and diff checks
passed. Client architecture and unchanged application version 0.10.9 passed.
Both independent static implementation reviews passed after the repairs.

Both final independent scoped implementation audits passed without remaining
required fixes. Each independently ran all 12 actual top-level groups with zero
skips: dependency 74.664s and compatibility 74.800s. Both also passed all eight
domain groups and the affected default packages, affected vet, exact contract
bytes/inventory, all 13 new Go files' formatting and diff checks. Default runs
explicitly clear actual opt-ins: integration child cases skip, including those
inside passing parent containers, and provide no actual database evidence.
No reviewer edited source/tests. All root/reviewer sessions and test processes
closed before commit preparation. This accepts the bounded workflow kernel.
T17 is in progress; no further whole task is promoted to completed. The current
47-task totals are four implemented, twelve in progress and thirty-one not
completed or deferred.

The kernel is a T17
prerequisite, not full T17, qualified decision creation/handoff, general Session
event acceptance, production signing, quality contribution or deployment. No
application version, protocol contract, dependency or download-budget change.

## T17 private atomic initial installation (2026-10-15)

The next bounded prerequisite adds a pure initial-installation calculation and
private native atomic creation of the actual T44 Session, logical workflow and
independent original association. Both independent roles passed its scope plan
after clarifying two boundaries: the original channel owner must already have
initialized the scope/current channel, and time guarantees cover the actual
returned Session start and wrapper-observed clocks, not unexposed inner reads.

Only available normal/forced install or silent preinstall is supported. Exact
canonical zero-start prototypes retain original scope/workflow/Session/install,
channel/type/purpose/hop/final identity. The actual returned T44 Session supplies
workflow and association start; opaque lifecycle and logical context digests
remain distinct. The private installer requires a processing standard24h T06
owner with positive epoch and active original lease/deadline, original scope CAS
and exact current channel. It neither repairs partial originals nor guesses a
channel. Duplicate identities and partial sets refuse without writes. T44 remains
the sole Session writer. Actual protected readback and stored hashes must agree
before final clock/fence checks and the future owner's same-transaction completion.

Explicit workflow migration two preserves v1 and every T44/T06/event migration
byte-for-byte. Updated readers recognize exact v1 or v1+v2; old v1 binaries are
not claimed to support v2. Startup never migrates. Runtime retains reads only,
without private EXECUTE, DML or SET-reachable authority. Complete post-write
inventory includes every T44 member, scope and both workflow references; exact
4096-reference and 1 MiB encoded bounds join the original T44 source/reserve
checks. They do not limit artifact downloads or introduce a duration/retry cap.

Actual tests use the original initialize_channel/create_session functions and
a clearly isolated test definer, not a production qualification/credential owner.
All three supported modes agree with the pure model and read back through the
restricted actual reader. Tests cover original owner/canonical/scope refusals,
whole rollback, duplicate/partial originals, same/different-identity concurrency,
missing scope/channel, v1 reads/explicit upgrade with preserved bytes/references
and confirmation, and restored v2 native catalog/privilege drift controls.
Resource-only terminal inventory fixtures prove exact 4096 and 1 MiB positives;
one extra reference or encoded byte refuses and rolls back while T44's own
inventory remains within its original bounds.

Compatibility review requested independent returned-start and action-window
controls. The primary added two actual T44 returned-start faults and exact final
time equality at Session start plus one day. These supplement six wrapper-clock
cases: initial NULL/unsafe and actual post-write final NULL/unsafe/backward/window
equality. Every fault checks the precise native guard, full state rollback, T06
processing and restored positive; no Session helper is replaced with a fake.
The original helper definition is restored after isolated clock faults.

Root's complete 24-group actual run passed without skips in 156.890s. After the
review additions and test-only concurrency encoding cleanup, the three affected
groups passed without skips in 36.686s: six wrapper clocks 17.88s, two returned
starts 10.54s and concurrent same/different identities 5.74s. These are separate
runs, not a claimed root single-run 25-group result. Client architecture and
unchanged version 0.10.9 checks passed.

Root's complete default Go tests passed with actual opt-ins cleared (workflow's
ten groups 1.736s; metadata PostgreSQL 1.606s). Module verification, full vet,
server build, pinned contract bytes/inventory and final empty formatting/diff
checks passed. Default integration skips are not actual database evidence.

The dependency role's final scoped audit passed without required findings. Its
complete 25-group actual run had zero skips/failures; 224.221s is the private
runner's elapsed time, not a preserved Go package duration. Its cleared-opt-in
default run passed all ten workflow domain groups (7.059s) and affected packages;
the metadata package's actual parents include sixteen skips and nine containers
whose children skip, providing no separate database evidence. Affected vet,
exact pins, empty formatting for all ten Go files and diff checks passed. Both
execution sessions and all child processes closed before the compatibility role
received the exclusive test slot. No reviewer modified source or tests.

The compatibility role's final scoped audit also passed without required
findings. Its complete actual 25-group run had zero skips and a Go package
duration of 172.767s. Its cleared-opt-in default contracts (1.576s), ten workflow
domain groups (1.928s) and metadata package (2.185s), affected vet, original pins,
all ten Go files' empty formatting and diff checks passed. Default actual cases
again consist of sixteen top-level skips and nine passing parent containers with
skipped children; they are not actual database evidence. All three compatibility
sessions and child processes closed before commit preparation. Both independent
roles accept this bounded private primitive after the test repairs/additions.

Qualified decision creation still needs current metadata/Root/deny, complete
qualification/contribution and signing composition, including original
decision.iat/create-time consistency. This private primitive does not establish
those absent inputs or make a presigned credential valid for a different start.
Handoff, general Session reception, production HTTP/signing, quality contribution
and deployment remain outside this batch. T17 remains in progress; the 47-task
totals remain four implemented, twelve in progress and thirty-one not completed
or deferred. No application version, protocol contract or dependency change.

## T20 workflow-aware Session observation (2026-10-15)

This bounded prerequisite adds pure Session event calculation and an actual
protected PostgreSQL read source for available normal/forced install and silent
preinstall. It returns candidates only. The fixed atomic acceptance owner,
sealed recovery, production endpoint and client upload queue remain pending;
neither T17 nor T20 is promoted to completed.

Original signature, current published Root, role deny and immutable actual T44
Session lineage precede permanent receipt lookup. Exact original receipt replay
repeats current proof without reading workflow or cursor, including when the
workflow catalog is unavailable. Current denial and original upload expiry still
apply. New events require the actual protected workflow, original association,
same actual Session and exact original diagnostic cursor. Final proof and safe,
nondecreasing SQL time follow the reads. Complete joined references deduplicate
only identical points and enforce 4096 references and 1 MiB canonical bytes.

An inert workflow reader constructor avoids both premature workflow checks on
replay and a second transaction while an owner holds the sole pool connection.
The original constructor preserves eager readiness checks, and every actual
Read/Capture preserves original catalog, role and transaction guards. No SQL,
migration, writer grant, protocol, dependency or command registration changes.

Live events calculate original T44 edges, normal confirmation and the first
workflow failure. Lawful late events calculate diagnostics without reviving the
old Session. Cursor state/sequence precede absent pre-terminal history; shared
freeze, boundary, health and byte anchors retain their original meaning. Unused
zero/preinstall freeze anchors do not become Session execution authority. Only
the original backup edges consume the freeze guard. Already ended workflows
retain their first failure. New progress/task, activation and protected
resume/staged transitions are explicitly outside this local-state profile.

Root compared 182 cases against the pinned original full Node planEvent with
genuine signed original credentials and schemas; all passed together with eight
pure domain groups in 42.098s. Comparison covers complete receipt/workflow/cursor
candidates and the Session revision/state/sequence/freeze projection, not the
original model's additional ledger/outbox/contribution atomic write set.

Root's combined actual PostgreSQL run passed all twelve top-level groups with
zero skips in 105.352s: nine new Session observation groups and three related
workflow constructor/migration regressions. Coverage includes all three modes,
late facts, receipt-before-workflow, current deny, cutoff equality, missing
association, sole-connection operation, catalog/privilege restoration, continuous
late cursors and preserved shared anchors. Actual joined count/byte overflow
tests first prove both source subsets independently readable, then refuse only
the oversized union and restore exact-limit positives. A point-read delay lets
an actually published Root expire before final proof; the actual clock and
source are not replaced, and refusal leaves no writes.

Protected historical receipt/cursor and terminal resource fixtures certify
observation, not production qualification, credential issuance or acceptance.
Default integration skips do not provide actual database evidence. Download
continuation retains no total/cumulative duration or retry cap. The 47-task
totals remain four implemented, twelve in progress and thirty-one not completed
or deferred.

Root's complete default Go suite passed with all actual/reference opt-ins
cleared (events 9.433s, event PostgreSQL adapter 2.431s, metadata PostgreSQL
2.329s and workflow 2.313s). Module verification, complete vet, server build,
original pinned bytes/inventory, all eleven Go files' empty formatting and diff
checks passed. Client architecture and unchanged version 0.10.9 checks passed.

The dependency role's final scoped audit passed without required findings or
source/test edits. Its actual twelve-group run passed without skips in 89.839s;
its explicitly enabled original Node 182 cases and eight pure groups passed in
43.689s. Cleared-opt-in affected default packages passed (events 9.975s, event
PostgreSQL adapter 2.027s, workflow 2.043s and metadata PostgreSQL 2.119s), as did
affected vet, exact pins, all eleven Go files' empty formatting and diff checks.
Default actual parent containers with skipped children provide no database
evidence. All dependency sessions and child processes closed before the second
independent role received the exclusive execution slot.

The compatibility role's final scoped audit also passed without required
findings or edits. Its actual twelve-group run passed without skips in 94.812s;
the explicit original Node 182 cases and eight pure groups passed in 41.979s.
Cleared-opt-in defaults passed (events 9.432s, event PostgreSQL adapter 1.638s,
workflow 1.908s and metadata PostgreSQL 2.192s), along with affected vet, exact
pins, all eleven Go files' empty formatting and diff checks. Within the nine new
actual groups, the default run skips eight top-level groups and all five children
across the suite; the ninth parent passes with both capacity children skipped.
It is not actual integration evidence. All root and both reviewers' execution
sessions/child processes closed before commit
preparation. Both independent roles accept this observation/calculation batch.

## T20 fixed original Session event acceptance (2026-10-15)

This bounded batch implements the internal original-input Session acceptance,
sealed recovery and command acknowledgement workflow for available normal/forced
install and silent preinstall. It follows the previously committed Session
observation prerequisite and composes actual T06/T44/T17 commands. Full T20
remains in progress: activation, resume/staged transitions, progress/task facts,
public batching, client persistent upload queue and quality contribution are
separate work. Deployment and physical-client qualification are not claimed.

The additive protected event_session_reception namespace owns immutable intent
and operation-attempt rows only. Original event_reception receipt/outbox/cursor
identities and all existing lifecycle/workflow/reception/transaction SQL bytes
remain unchanged. Explicit initialization leaves runtime read-only; explicit
enable grants only the fixed reserve/apply pair. Startup checks exact catalogs,
permanent table persistence and complete direct/inherited/SET-reachable capability
pairs; it neither migrates nor repairs sibling permissions.

Original signed claims/allowedEvents, current published Root, role deny and
immutable Session lineage precede receipt. All workflow/cursor and mutable CAS
checks for new acceptance follow absent receipt. Existing original receipts can
therefore replay after terminal/workflow changes or workflow unavailability,
while current deny, Root and upload windows still apply. Permanent JTI/event and
signed-claims identity remain independent of lawful envelope variants and proof
generation. Generic decoding or a copied facade cannot borrow initial authority.
Sealed frames retain bounded claim bindings/hashes/refs, never raw credentials
or original packets; original event bytes live in the protected immutable intent.

Live facts call actual T44 transitions; normal confirmation and first download
failure compose actual T17 ports. First failure's returned endedAt is also the
receipt/outbox time, and the complete Go plan is recomputed at that actual time.
Already failed workflows retain their first latch, while new lawful late events
receive their own acceptance time. Late facts advance only the original cursor
and preserve shared anchors without reviving Session rights.

Actual domain readback, complete joined references and whole encoded frame
capacity precede a final same-port receipt-only time/proof seal. Safe observation
lower bounds include protected-intent Current, each recovery refresh and the
final actual workflow read. All applicable Session/workflow/cursor writes,
receipt, outbox and T06 completion commit together. Committed epoch zero cannot
insert missing acceptance. Unknown commit/output acknowledgement never returns
speculative success; a fresh original retry or restricted recovery reconciles it.

Independent review identified and root repaired permanent-table persistence,
last-readback native time propagation and protected-intent recovery time
propagation. Tests independently capture actual workflow clock/readback completion
and use exact rejection reasons. Three controlled private red runs prove that
removing the native clock comparison, omitting the last actual readback watermark
or omitting the protected-intent lower bound permits real acknowledgement and
commit; every modified source is restored byte-for-byte afterward. An initial
recovery-clock control accidentally exercised Current's own internal guard; the
corrected control holds that internal pair coherent and isolates the cross-read
lower bound. Compiler errors or a different refusal never count as red evidence.

Root's actual PostgreSQL run passed all 22 top-level groups with zero skips in
636.877s. The subsequently tightened native epoch-zero error-reason assertions
passed in 10.492s (case 8.20s). T06 intentionally hides native SQL errors behind
Unavailable; this test captures the actual native reason before that mapping
without changing execution or emitting private diagnostics.

Root's complete cleared-opt-in default Go suite passed (events 15.699s, events
PostgreSQL 2.627s, metadata PostgreSQL 2.607s, Session command 7.009s, workflow
2.179s), as did module verification, complete vet, original pins, all 31 Go
sources' empty formatting, server build and diff checks. Client architecture
and unchanged version 0.10.9 checks passed. Default integration skips are not
actual execution evidence.

The independent dependency audit passed without remaining findings or edits.
Its nine core actual groups all executed with zero skips in 300.134s. Cleared
opt-in defaults passed (Session command 2.124s, config 1.330s, events 10.042s,
events PostgreSQL 2.401s, workflow 2.159s and metadata PostgreSQL 2.481s), along
with affected vet, exact original pins/inventory, all 31 Go sources' empty
formatting, both SQL files' UTF-8/LF checks and diff checks. All its sessions and
child processes closed before the compatibility role received the sole execution
slot.

The independent compatibility audit also passed without remaining findings or
edits. Its other thirteen actual groups all executed with zero skips in
303.740s, including real CLI output loss, abrupt exit/fresh restricted recovery,
three unknown-commit boundaries, eight capability combinations, original owner
acceptance/replay, concurrency, single connection and historical/signature cases.
Cleared-opt-in defaults passed (Session command 2.546s, config 1.509s, events
9.318s, events PostgreSQL 2.275s, workflow 2.057s and metadata PostgreSQL 2.299s),
as did affected vet, original pins/inventory, all 31 Go sources' empty formatting,
both SQL UTF-8/LF checks and diff checks. Default skipped helpers/references and
actual tests provide no integration evidence. All root and both reviewers'
sessions, command/helper children and test processes closed before commit
preparation. Both independent roles accept this scoped implementation batch.

The final staged-file check additionally found an extra trailing LF in the new
owner SQL; previous unstaged checks did not include that untracked file. Root
removed exactly one final LF outside the unchanged SQL definitions/statements.
Both independent roles rechecked the final single-LF staged bytes and unchanged
old SQL. The original 22/9/13 runs precede this nonsemantic cleanup. Root rebuilt
the command/helper for the resulting new namespace migration digest and passed
three related actual groups with zero skips in 43.720s: command provisioning/
output loss, catalog/role restoration and confirmation/receipt-before-workflow
replay. These post-cleanup runs are root evidence, not another reviewer runtime
run.
Post-cleanup cleared-opt-in affected defaults (Session command 2.089s, config
1.217s, events PostgreSQL 2.748s, metadata PostgreSQL 6.949s), affected vet,
server build, original pins, 31-Go empty formatting, both SQL single-final-LF,
all 33 frozen fingerprints and staged/unstaged diff checks also passed. All
post-cleanup execution processes closed before commit preparation.

The 47-task totals remain four implemented, twelve in progress and thirty-one
not completed or deferred. Downloads retain no hard total/cumulative duration
or retry cap; credible progress or confirmable waiting remains incomplete.

## T20 original event HTTP batching and recovery (2026-10-15)

This bounded service increment exposes only the original telemetry-event and
three purpose-specific transaction-event POST operations. Exact pinned request,
response and status-specific error Schemas remain unchanged. Task paths remain
absent. No-update, available normal/forced install and silent preinstall Session
facts, and three transaction purposes use their existing actual owners. Full T20
remains in progress: progress/resume/staged/activation-Session and task facts,
protected client outbox/upload lifecycle and quality contribution remain separate.

The events feature validates the complete original batch, then the actual current
Root/role deny/signature/windows and event bindings precede lineage dispatch,
unsupported-profile refusal and receipt availability. Each ordered original unit
repeats owner checks and commits independently. Success contains every matching
confirmed acknowledgement, retaining duplicates; a later failure emits a closed
error with no successful-prefix list. Earlier accepted units remain durable and
replay on original retry. Every HTTP response, including replay, receives a fresh
server requestId without exposing the T06 result identity or provider diagnostics.

Default-off explicit composition requires the configured expected product,
independent operations key and all explicitly provisioned owner capabilities.
Startup never migrates, grants or borrows administrative configuration. One
joined recovery worker registers no-update, Session, live/late TransactionOwner
and the unchanged legacy late-only command. Recovery uses sealed original
bindings without an original request or an HTTP product override. Listener
failure and shutdown join the worker before closing its pool. Existing native
SQL, dependency versions and pinned contracts remain unchanged.

The per-request event control-body read deadline is 30 seconds, with server-side
cancellation, joined callback, owned body close and final deadline clear. Even
invalid Content-Type owns cleanup so net/http cannot subsequently drain an open
incomplete input indefinitely. Global read/write timeouts remain unset; this is
not a download-duration limit. Legal undelivered profiles return retryable 503,
without acknowledgement or permanent-invalid classification.

Independent review found and root repaired the early Content-Type cleanup gap
and tests that originally relied on client disconnection. Revised real HTTP
tests keep the peer open, cancel only server context, observe server-side closed
connections for all four rejected routes, and isolate callback join and deadline
clear ordering. Four private controlled removals independently fail their target
tests: missing cancellation deadline, missing callback join, missing final clear
and restored early media-type refusal. Compiler failures do not count as red
evidence, and every altered production source is restored byte-for-byte.

Actual integration fixtures contain genuinely signed original inputs, published
Root/deny and protected original lineage records. Those fixture records do not
certify qualified Session/Transaction creation. The actual server lifecycle helper
runs production composition and cancels its context rather than killing the
process, so worker joining is exercised. Expired four-role credentials are
rejected before absent lineage; transaction late facts occur after the 30-day
execution window but within the original 37-day upload horizon. No expired
credential is extended to make a late positive pass.

Root's frozen-code PostgreSQL run passed all nine new top-level groups with zero
skips in 441.845s. It covers seven delivered profiles and permanent replay,
whole-batch Schema and duplicate ordering, prefix commit/error/original retry,
deny before unavailable lineage/receipt across supported and undelivered legal
profiles, original expired credentials, valid late facts, actual built-process
startup/runtime permission refusal and joined shutdown, lost HTTP output and all
four versioned sealed recovery commands. Whole business-state readback compares
Session, Transaction, workflow, receipt and outbox records on replay. Additional
current-authority observations may legitimately commit another T06 frame for the
same permanent acceptance, and fixtures also contain unrelated Root/component
operations; total operation-row count alone is not an acceptance duplication
oracle.

Root's complete cleared-opt-in Go suite passed, including server 3.944s,
contracts 3.268s, events 10.791s, event PostgreSQL 3.977s, metadata PostgreSQL
5.303s and HTTP 6.711s. Module verification, complete vet, original pinned
bytes/inventory, all eighteen Go sources' empty formatting, actual server build,
frozen fingerprints and staged/unstaged whitespace checks passed. Client
architecture and unchanged version 0.10.9 checks passed. Default integration
skips are not actual database, physical platform or provider evidence.

The independent dependency role passed its final scoped static audit and actual
four-group run with zero top-level or child skips in 222.626s (runner elapsed
236.419s). Its groups cover actual process startup/joined shutdown, expired
credentials before absent lineage, denial before unavailable lineage/receipts
and undelivered profiles, and all four sealed recovery commands. It reused the
frozen actual server/helper binaries, without rebuilding or editing sources.
Cleared-opt-in affected defaults passed, along with affected vet, module
verification, original pins/inventory, eighteen-source empty formatting, frozen
fingerprints and staged/unstaged whitespace checks. Default skips and parents
with skipped actual children are not integration evidence. Every dependency-role
session and child process closed before the compatibility role received the
exclusive execution slot.

The independent compatibility role's actual other five-group run passed with
zero top-level or child skips in 158.284s. It covers seven original profiles and
permanent replay, partial-batch commit/error/retry, complete Schema and duplicate
order, four late profiles and lost HTTP output followed by original retry.
These five groups and the dependency role's four are complementary independent
runtime coverage; neither role's subset is reported as its own full nine-group
execution.

The compatibility role's final scoped audit passed without required findings or
edits. Its cleared-opt-in affected defaults passed (server 3.060s, contracts
2.199s, events 9.170s, event PostgreSQL 2.479s, HTTP 5.850s and metadata
PostgreSQL 3.189s), as did affected vet, module verification, original pins and
inventory, eighteen-source empty formatting, both working/staged frozen hashes
and staged/unstaged whitespace checks. In the default suite, this batch's nine
actual groups are exactly five top-level skips plus four passing parents whose
eighteen children all skip; neither the parent passes nor skipped helpers/Node
references count as actual execution. Every compatibility-role session and child
process closed before returning the exclusive execution slot to root.

Both independent roles accept this bounded HTTP/recovery implementation. The
47-task totals remain four implemented, twelve in progress and thirty-one not
completed or deferred. There is no hard total/cumulative download duration or
retry cap, and this increment makes no deployment or physical-client claim.

## T20 original download progress reception (2026-10-15)

This bounded increment accepts the original `download_progress` telemetry for
available normal/forced install and silent preinstall Sessions. It composes the
T44 protected observation and T17 logical-observation write with the actual T06
owner, original receipt/outbox, explicit CLI provisioning/recovery and the existing
HTTP batch routes. Activation-Session, resume/staged and task facts, client
outbox, qualified Session creation and quality evaluation remain separate work.

The four original progress labels retain their reference semantics. Confirmable
waiting can keep the byte count; total bytes may change and source observations
may arrive out of order. Live acceptance advances only Session sequence/revision
and scope revision, the protected progress record and workflow revision/maximum
original observation time. It preserves execution rights, deadlines and other workflow
fields. Late acceptance advances the shared diagnostic cursor and receipt only.
Ordinary late events preserve the actual progress byte floor. Session expiry
does not stop a download; no total/cumulative duration or retry cap is introduced.

Additive lifecycle/workflow observation and fixed event namespaces authenticate
exact native definitions, ownership, migration hashes, constraints, indexes and
direct/inherited/SET-reachable rights. The runtime has readonly native state and
only the complete fixed reserve/apply pair. Trusted initialization installs the
ordinary-event progress-aware overlay atomically; original SQL migration bytes
and digests remain unchanged. Older binaries with the original exact-definition
guard fail closed on that overlay. No mixed-version deployment is qualified.
Repeated initialization verifies existing state and preserves an enabled pair;
its neutral acknowledgement does not imply that existing rights were revoked.

Current published Root and role deny precede global receipt replay. Existing
receipts do not require a mutable original workflow or progress row. New events
authenticate their complete actual association/Session/progress/cursor. A present
inaccessible namespace cannot become an empty installation. Old sealed frames
refresh complete progress references. Incremental reference union deduplicates
each read set before applying the original count and whole-frame limits.

The fixed owner compares complete actual state after native application and
seals against the latest actual SQL observation, including progress readback.
Concurrent ordinary/progress events share one sequence CAS. Partial native
writes, stale leases, regressed/unsafe clocks and changed readback roll back
without acknowledgement. Original retry and sealed recovery preserve permanent
acceptance identity. The default-off HTTP admission option still allows existing
receipt replay and registers sealed progress recovery whenever its complete
capability is ready. A missing capability cannot strand pending progress.

Protected retention uses the original maximum Session upload horizon, saturated
at the protocol maximum safe integer. Genuine original signatures/API/model
verification confirmed that shorter lawful credentials at the upper Session
timestamp boundary remain valid. Trusted due-only cleanup is independent of a
shorter current credential and cannot remove a usable byte anchor.

Root's actual PostgreSQL run passed all nineteen new top-level groups with zero
skips in 464.145s. Coverage includes six live/late mode profiles, all four HTTP
labels, unchanged-byte waiting/source order, shared ordinary/progress sequence
competition, both timeout orders, nineteen native catalog fault/restore controls,
five fixed capability controls, both native post-write fences, eight final-clock
guards, two write/readback mismatches, latest progress SQL time regression,
permanent replay before mutable workflow, closed owner authority, admission-off
recovery, CLI output loss, fresh-process recovery and due-only cleanup. Native
primitive and future safe-time calculations are distinguished from authenticated
acceptance and actual current-clock execution. Original Node differential checks
cover thirty-four signed API/model vectors; they do not replace database tests.

That nineteen-group run follows the initialization-message repair and precedes
removing one extra final LF outside the new reception schema's definitions. The
cleanup changes its new installation digest while preserving SQL behavior and
all original SQL files. Final-digest checks and independent runtime evidence are
recorded below; the earlier run is not described as a final-digest execution.

On the final digest, root passed the complete cleared-opt-in Go suite, module
verification, complete vet, original pinned bytes/inventory, all fifty Go/SQL
frozen hashes, Go formatting, actual server build and staged/unstaged whitespace
checks. The actual original Node/API differential group passed in 8.60s, within
the four-group pure download run of 11.724s. Actual CLI/helper executables were
rebuilt from these bytes. Client architecture and unchanged version 0.10.9 checks
passed. Default skipped integrations and helpers provide no actual database,
provider, physical-platform or deployment evidence.

Root's final-digest PostgreSQL rerun passed the three relevant actual groups in
50.849s with zero skips: native catalog fault/restoration, actual command
provisioning/reinitialization/output loss/cleanup, and ordinary/progress receipt
replay before an unavailable original workflow. All root sessions, helpers and
workers closed before the dependency reviewer received the exclusive execution
slot. The independently executed subsets below must not be described as either
reviewer's own full nineteen-group run.

The independent dependency role passed its final scoped code/test/doc audit
without required findings or edits. Its final-digest actual PostgreSQL run
passed thirteen top-level groups (nine new and four old Session regressions),
with twenty-nine named child cases and zero skips in 371.028s. It covers native
progress/workflow parity, six live/late mode profiles, complete prospective
capacity, final native clocks/fences, actual readback mismatch, latest progress
seal time, shared sequence CAS, both timeout orders and original Session
rollback/sealed recovery. Its independent original Node comparison passed all
thirty-four vectors with zero skips in 12.289s (runner elapsed 18.835s).

The dependency role also passed cleared-opt-in affected defaults, affected vet,
module verification, original pins/inventory, all forty-five Go sources' empty
formatting, five new SQL files' single-final-LF/no-CR checks, fifty frozen
Go/SQL hashes and both staged/unstaged whitespace checks. Original SQL remained
unchanged. Default integration/reference/helper skips are not actual evidence.
Every dependency-role session, child process and worker closed before the
compatibility role received the exclusive execution slot.

The compatibility role's complementary ten new actual PostgreSQL groups passed
in 208.116s with zero top-level or child skips. They cover fixed capability and
native catalog fault/restoration, actual CLI initialization/reinitialization and
output loss, process death/fresh recovery, four-label HTTP/disabled-admission
replay, trusted due cleanup, safe upper retention, copied-owner/generic-input
refusal, sealed recovery with admission off and ordinary/progress receipt replay
before a missing original workflow. These ten and the dependency role's nine
new groups jointly cover the nineteen-group profile on the final digest;
neither role executed all nineteen as its own actual run.

The compatibility role passed its final scoped code/test/doc audit with no
remaining required findings, source edits or executable rebuilds. Its actual
Node 26 original comparison passed thirty-four vectors in 8.79s, within four
pure download groups totaling 11.807s with zero skips. Cleared-opt-in affected
defaults, affected vet, module verification, original pins/inventory,
forty-five-source empty Go formatting, fifty working/staged frozen hashes,
five SQL UTF-8/single-final-LF/no-CR checks and both whitespace checks passed.
The default new PostgreSQL groups comprise thirteen top-level skips and six
passing parents whose seventeen children all skip; none is actual evidence.
Original SQL stayed unchanged. Every compatibility-role runtime/tool session,
test worker and actual command/helper/Node child closed before returning the
exclusive execution slot to root. Unrelated existing local services were not
stopped or changed.

Both independent roles accept this bounded download-progress increment. Root
also updated the existing HTTP and ordinary-Session operator documents to
describe the optional progress owner, fifth recovery registration, byte-floor
compatibility and protected overlay. No wire contract, dependency, application
version or download-budget change is introduced. The forty-seven-task totals
remain four implemented, twelve in progress and thirty-one not completed or
deferred. Full T17/T20/T44, physical client qualification and deployment remain
uncompleted; no hard total/cumulative download duration or retry cap is added.

## T17 private atomic download handoff (2026-10-16)

This bounded T17/T44 increment creates a new actual ordinary Session and fences
the downloading source in the same T06 transaction as the workflow update,
original association and immutable per-destination resume registration. It is
a private prerequisite. Qualified check/refresh, current decision issue-time
and signing composition, preparation acquisition, original resume-event
acceptance, client continuation and quality contribution remain pending.

Available normal/forced install and silent preinstall sources can hand off from
download_started/deferred or their original expired states. Actual create_session
provides the common new startedAt, old fencedAt and binding handedOffAt. Scope
revision advances once. The source state, sequence, original start and complete
logical context, hop, original stage/group/window and source observation times
are preserved. New opaque lifecycle context/current preparation may differ,
while each actual association independently binds its own Session. There is no
total/cumulative download duration, retry or handoff-count cap; revision/resource
bounds do not become artifact budgets.

Readonly registration capture validates the actual destination and source
Session/association, including install identity and source fence. Repeated
handoff authenticates its prior immutable binding before replacing latest;
historical bindings survive later handoffs and legitimate channel invalidation.
Missing expected current registrations, fabricated sources and inaccessible or
partial native catalogs fail closed. Observation clocks retain all actual
source watermarks, including a later channel fence. These reads grant no resume
event or execution permission.

Explicit paired initialization holds shared locks, creates only from complete
absence, verifies an existing pair without repair and refuses either one-sided
installation without DDL/grants. New handoff capability requires lifecycle
migration 15. Its additive overlay preserves the original channel signature and
all existing invalidation logic, adding only valid authority-free fenced download
sources to the complete inventory. Existing migration SQL bytes remain unchanged;
runtime receives no new native write/execute rights and startup never initializes.
Older exact-definition binaries fail closed; mixed-version operation is not
qualified. Deployment remains deferred.

Complete first-handoff joined references include the old-binding absence point
(native inventory plus seven); repeated handoff additionally consumes the earlier
source association (plus eight). Actual old/new capture union is bounded at
4096 refs and 1 MiB encoded refs. Each lifecycle record retains 1 MiB, original
association/binding 64 KiB and aggregate sources 32 MiB. Final capacity includes
the old fenced body's growth and the actual new Session. The native result
reserves maximum safe observation-clock width before its final owner/window
guard. All compound state and T06 completion roll back together on refusal.

Independent review identified and root repaired incomplete historical source
authentication, previous-binding validation before replacement, one-sided
initialization repair, post-handoff channel incompatibility, source-clock
watermark loss and incomplete safe fence validation. Final test review also
replaced a boolean-only competition result with exact SQL conflict checks and
standalone full-write/rollback positives. A final source-channel-fence time
lower bound was added after the first complete runtime run and independently
reviewed. The bounded production/code/test audits have no remaining required
static findings; independent runtime acceptance is recorded below.

Root's first frozen-code actual PostgreSQL run passed eighteen new groups and
three existing initial-creation/download-owner/HTTP groups, with seventy-seven
child passes, zero skips and zero failures in 361.897s. After the final source
fence clock guard, root passed all four directly affected groups in 31.237s,
including exact source-only time regression and restored positives, current and
historical binding tamper/restore, and direct/expired/repeated channel changes.
The earlier full run is not described as an execution of the later guard.

Actual resource checks use complete old/new capture unions: exact4096 refs,
exact1MiB encoded refs and exact32MiB aggregate pass; one-item/one-byte excess
rolls back. The aggregate negative isolates growth of the old fenced body after
the actual initializer succeeds. Sixteen observed clock faults, two post-write
lease losses, seventeen identity/CAS/epoch controls plus the separate event-owner
policy refusal, both one-sided catalogs, rights/catalog drift and actual
same/different-destination/progress/failure/timeout competition retain restored
positives. Historical timestamps and unrelated resource rows do not establish
qualified online decisions or physical multi-day downloads.

On the final frozen code root passed the complete cleared-opt-in Go suite,
module verification, complete vet, original pinned contract bytes/inventory,
empty formatting for all twenty-eight changed Go sources and an actual service
build. Thirty-two working/staged Go/SQL/reference hashes, three new SQL
UTF-8/single-final-LF/no-CR checks and both whitespace checks passed. Client
architecture and unchanged version 0.10.9 checks passed. Default opt-in skips
are not actual database/provider/platform evidence.

The final original Node comparison passed twelve genuinely signed mode/source
profiles in 3.900s (the actual reference subtest took 2.63s), comparing complete
Go handoff candidates, preserved source observations and original channel
invalidation. It calls the normative signer/verifier and both original plans,
and does not substitute a local model or claim current production qualification.

The independent dependency role passed its final scoped code/test/doc audit and
actual nine-group subset, with thirty child passes and zero skips in 106.605s.
Its actual migration/safe-path regression passed two groups with zero skips in
6.643s. Its independent original Node comparison passed twelve child cases in
3.971s. Cleared affected defaults, affected vet, module verification, original
pins/inventory, all twenty-eight Go sources' empty formatting, thirty-two
working/staged frozen hashes, thirteen production hashes, new SQL byte format
and both whitespace checks passed. Parent default passes containing skipped
actual children are not database evidence. Every test/tool session and child
process closed before the compatibility role received the exclusive runtime
slot. Neither role's nine-group subset is reported as its own eighteen-group run.

The independent compatibility role's actual run passed the complementary nine
new groups and three existing initial-creation/download-owner/HTTP groups,
with forty-seven child passes, zero skips and zero failures in 241.866s. Its
independent original Node comparison passed twelve child cases in 4.121s.
Together the two roles cover all eighteen new actual groups on the final
frozen code, including the last source-channel-fence watermark guard.

The compatibility role also passed cleared affected defaults, affected vet,
module verification, original pinned bytes/inventory, twenty-eight-source empty
Go formatting, thirty-two working/staged code hashes, thirteen production
hashes, the three SQL byte-format checks and both whitespace checks. Its actual
workers, fixture cleanups and all tool/Go/Node child processes closed before
formally returning the exclusive runtime slot. It made no source edits.

Both independent roles accept this bounded increment with no remaining required
code/test findings. Their complementary actual subsets jointly cover the full
new profile; skipped default children are not counted as integration execution.
No wire contract, dependency or application version changes. The forty-seven
task totals remain four implemented, twelve in progress and thirty-one not
completed or deferred. Full T17/T20/T44, physical client qualification and
deployment remain uncompleted; no hard total/cumulative download duration or
retry cap is introduced. Continue the missing original resume edge and remaining
task graph after committing this prerequisite.


## T17/T44 private live download resume edge (2026-10-16)

This bounded prerequisite adds the original update_offered/deferred to
download_resume_context edge after an actual handoff. The destination first
receives the existing ordinary update_offered event. The next download_started
edge uses the existing ordinary command. Authenticated original T20 receipt,
outbox, late-cursor, sealed recovery and HTTP/CLI acceptance follow separately.
No complete T17/T20/T44 or physical client acceptance is claimed.

T17 authenticates both real Sessions and original associations, immutable
per-destination registration and unchanged logical context/hop. Live resume
requires ongoing/current/latest equality. Original event facts only compare
protected workflow/Session/hop identities; normal mode reuses the actual
confirmed hop. Opaque lifecycle context remains independently bound to each
Session. T44 advances Session revision/sequence/state and scope exactly once;
the workflow revision, original attribution, source observation, preparation
and Session start/action window remain unchanged. Activation is excluded.

Two new private namespaces require the existing actual handoff pair and
lifecycle migration 15. Explicit initialization creates a wholly absent pair,
verifies an existing pair and refuses partial/corrupt state without repair.
Runtime has no generic native EXECUTE/DML or PUBLIC authority. Event OwnerReplay
requires a positive live owner epoch, exact complete original CAS and scope
lock. Full native readback, source time watermarks, joined references and
aggregate/result growth precede final owner/clock/window fences. T06 completion
shares the transaction; refusal rolls back every business write.

CaptureDownloadResume authenticates the complete native inventory plus seven
joined refs, including actual source association, immutable binding and the
new installation identity. Original limits remain 4096 refs, 1 MiB encoded
refs and 32 MiB aggregate source, including final body growth. They impose no
artifact size, total/cumulative download duration, retry or handoff-count cap.
Source timeout does not end the ongoing logical workflow or erase original
intent. The private native port acknowledges no authentic client event.

The root's final frozen actual PostgreSQL run passed fifteen groups and
sixty-eight child cases with zero skips/failures in 370.003s: twelve new resume
groups plus three existing handoff/download-owner/HTTP regressions. Controls
cover genuine initial creation and first handoff, three modes/two source
states, full Go/native readback, original source tampering, exact CAS/owner
refusal, four observed clocks, postwrite lease, catalog/role/partial capability,
4096 and 1 MiB exact joined refs, final32MiB aggregate, three lawful-owner
races and original source timeout with fresh-capture convergence. Historical
failure latches only its workflow; a winning resume advances scope once.

The final cleared-opt-in complete Go tests, module verification, complete vet,
original contract bytes/inventory, fifteen-source empty Go formatting, server
build, eighteen working/staged source hashes and both whitespace checks passed.
Both new SQL files are UTF-8 with single final LF; all previously committed
SQL bytes remain unchanged. The original genuinely signed Node reference
passed six mode/state pairs in 5.054s (actual subtest 3.74s), compares complete
Session candidates, and independently asserts unchanged workflow/source.
Default skipped cases are not actual integration evidence. Client architecture
guard and version consistency passed; application version remains 0.10.9 and
normative document versions remain 1.0.


The independent dependency role's final scoped code/test/doc audit passed with
no remaining required finding. Its assigned six new actual groups passed
twenty-nine child cases with zero skips/failures in 94.849s. Its original signed
Node 26 reference comparison passed six cases in 5.784s (actual subtest 4.89s).
Cleared affected defaults/vet, module verification, original pinned bytes and
inventory, fifteen-source empty formatting, eighteen working/staged frozen
hashes, two new SQL byte formats, unchanged old SQL and both whitespace checks
passed. All sessions, fixtures and Go/Node child processes closed before the
compatibility role received the exclusive runtime slot. Its six-group subset
is not described as a twelve-group run.


The independent compatibility role's final scoped code/test audit passed with
no remaining required finding. Its complementary six new groups plus three
existing handoff/download-owner/HTTP regressions passed nine groups and
thirty-nine child cases, zero skips/failures in 274.270s. Its original signed
Node comparison passed six cases in 2.445s (actual subtest 1.17s). Cleared
affected defaults/vet, module verification, pinned original bytes/inventory,
fifteen-source empty formatting, eighteen frozen working/staged hashes, both
new SQL byte formats, unchanged old SQL and both whitespace checks passed.
All fixture cleanups, worker/Go/Node child processes and tool sessions closed
before the exclusive runtime slot returned to the primary role.

Both independent scoped code/test audits pass with no remaining required
findings. Their complementary actual subsets jointly cover all twelve new
resume groups; neither role's six-group subset is called its own twelve-group
run. Default parent passes with skipped children are not actual PG evidence.
No wire contract, dependency or application version changes. The forty-seven
task totals remain four implemented, twelve in progress and thirty-one not
completed or deferred. Continue authenticated original resume reception,
shared late cursors, receipt/outbox/sealed recovery and HTTP/CLI composition,
then the remaining task graph. Deployment remains explicitly deferred.

## T20 authenticated original download resume reception (2026-10-16)

This bounded increment composes the actual T17/T44 resume prerequisite with
authenticated original event reception. It accepts the original offered/deferred
resume event through a fixed owner, immutable receipt/outbox, shared late cursor,
sealed recovery, explicit CLI and the existing ordered HTTP batch route. Ordinary
download-start and progress events retain their existing owners. Original Root,
credential and current deny checks precede receipt replay; genuinely committed
receipts remain replayable after mutable workflow/resume capabilities disappear.

Live acceptance observes the actual source Session, handoff association and
immutable destination registration. Final readback and time/capacity checks join
native state, receipt, outbox and T06 completion in one transaction. A private
transaction marker preserves the live action window across savepoints and
same-transaction owner completion, while later committed replay retains original
permanent-receipt semantics. Historical events update only their diagnostic
cursor. New admission may be disabled without stranding sealed recovery or
original receipt retries. Repeated CLI initialization verifies existing state
without changing enabled permissions.

The primary passed all thirteen new actual PostgreSQL groups and three existing
download HTTP/download owner/native resume regressions in two serial segments:
three groups/four children in 216.952s and thirteen groups/thirty-one children in
551.512s, totaling sixteen groups/thirty-five children with zero skips/failures.
The four final clock cases cover top-level, open-savepoint, released-savepoint
and same-transaction completed-owner epoch zero. Each rejects the actual final
SQL clock change atomically, accepts the restored case and permits genuine
next-transaction permanent replay. Capacity controls reject 4097 joined refs
before reservation and accept the restored exact 4096 boundary. CLI crash/output
loss and disabled-admission recovery use actual isolated database state.

Complete cleared-opt-in Go tests/vet, module verification, original pins,
formatting, server build, thirty-six working/staged source hashes, both whitespace
checks and two new SQL byte-format checks passed. All old SQL remains unchanged.
After a final pure test refinement, event defaults and affected vet passed again;
production and actual PostgreSQL-test bytes remained identical. The final
original signed Node comparison passed twenty-four mode/state/history cases in
8.532s (actual reference subtest 7.22s).

The original model's full quality head/outbox is independently checked by the
comparison; the physical owner retains its previous bounded null contribution
watermark. Full T21 quality writing, protected durable client queues and physical
host qualification remain pending. Default skipped children are not integration
evidence. No wire contract, dependency or application version changes and no
artifact size, total/cumulative download duration or retry cap is introduced.
The forty-seven-task totals remain four implemented, twelve in progress and
thirty-one not completed or deferred. Full T17/T20/T44 remains open. Continue
client request-to-ACK correlation and the remaining task graph after this batch;
deployment remains explicitly deferred.

The independent dependency role's final scoped code/test/doc audit passed with
no remaining required findings. Its six new actual groups passed seventeen
child cases with zero skips/failures in 370.917s. Its original signed Node 26
comparison passed twenty-four cases in 8.799s (actual reference subtest 7.43s).
Cleared affected defaults/vet, module verification, original pins, thirty-four
Go files' empty formatting, thirty-six working/staged hashes, both new SQL byte
formats, unchanged old SQL and both whitespace checks passed. All sessions,
fixtures and worker/Go/Node processes closed before the compatibility role
received the exclusive runtime slot. Its six-group subset is not described as
its own thirteen-new-group run.

The independent compatibility role passed its complementary seven new groups
and three existing download HTTP/download owner/native resume regressions:
ten actual groups, eighteen child cases, zero skips/failures in 392.186s. Its
independently built CLI/helper exercised actual command and crash recovery.
Its original signed Node comparison passed twenty-four cases in 8.510s (actual
reference subtest 7.13s).

Its cleared affected defaults/vet, module verification, original pins,
thirty-four Go files' empty formatting, thirty-six frozen working/staged hashes,
two new SQL byte formats and both whitespace checks passed. Old committed SQL
blobs remained identical; the observed Windows checkout CRLF difference was
verified without rewriting that file. Default skipped children are not actual
integration evidence. Both independent scoped code/test audits pass with no
remaining required findings. Their complementary actual subsets jointly cover
all thirteen new groups, plus three existing regressions in the compatibility
subset.
All compatibility workers, fixtures, owned runtime processes and tool sessions
closed before the exclusive runtime slot returned to the primary role.

The service increment is committed and pushed as
`100b22ea855e52ee9b7a074e4e448bb616cd0d70` on
`codex/t20-authenticated-resume`. Both final independent documentation reviews
passed. The client architecture guard and version consistency check passed;
application version remains 0.10.9 and normative document versions remain 1.0.

## T20 supplied original client event acknowledgement correlation (2026-10-16)

This bounded client prerequisite adds one dedicated event request/receipt entry
in the existing Rust codec and a raw-byte application facade. Only the six
original event operations and supplied HTTP 200 responses can return a binding.
Complete original request/response validators retain all metadata and nested
credential bounds. Exact count and per-position event IDs preserve identical
duplicate events, all three original dispositions and the server-generated
response requestId. A batch-local credential JTI/event-ID map compares full
canonical event bytes and refuses immutable conflicts beyond the facts hash.
Existing contract-only APIs, including structural acceptance of declared HTTP
errors, retain their behavior and error codes. No request is sent, queue written
or pending event cleared by this supplied correspondence calculation.

The primary's final scoped five groups passed with zero skips/failures in 3.50s,
after 4.46s compilation. Six genuinely signed original credential templates are
generated in memory and preflighted through the original complete Node API
contracts. Controls cover mixed dispositions, duplicates, cardinality, distinct-ID
reorder/foreign IDs, immutable sequence/time/from-state/facts conflicts with
independent old-contract positives, canonical key-order equivalence, wrong
operation/purpose, status separation, unchanged inputs, malformed/duplicate JSON,
exact 1 MiB metadata and nested 64 KiB credential boundaries and restored
positives. The facts-conflict control recomputes its digest before preflight.

The actual Windows application passed all fifty-four updater tests in 1.26s
after 3m24s compilation, including the new real application facade across all six
original fixtures. Architecture guard, version consistency, all-target codec
Clippy (final 3.18s), formatting and five working/staged source hashes/whitespace
checks passed. The final pure-test-only refinement explicitly permits swapping
two identical events' valid mixed dispositions, which the wire cannot
distinguish. Only that integration test's hash changed after the application run;
all production, application/test and Node fixture bytes remained identical.

Both independent static audits found no required code defect. One documentation
phrase about reordered acknowledgements was narrowed to event-ID positions and
the duplicate-disposition positive was added. Actual independent verification
is recorded separately below. Unchanged full codec/original
contract suites and unrelated application tests were not repeated.

Supplied correspondence is not authenticated transport, current Root/deny/time,
remote commit evidence or durable queue deletion authority. A batch without a
complete exact receipt cannot clear a remotely committed prefix. Protected
durable original event storage, retry/recovery ownership, authenticated receipt
handling and physical host qualification remain pending; full T20 is open.
No Schema, wire contract, dependency, configured origin/root, UI, application
version or gitlink changes. No total/cumulative artifact download duration or
retry cap is introduced. The forty-seven-task totals remain four implemented,
twelve in progress and thirty-one not completed or deferred. Deployment remains
explicitly deferred. Details: [supplied event acknowledgements](client-upgrade-event-acknowledgements.md).

The independent dependency role's final scoped code/test/doc audit passed with
no remaining required findings. Its five codec groups passed with zero
skips/failures in 2.86s; the actual application's new exact facade test passed
in 6.46s, with 3066 unrelated tests filtered. This is not another full
fifty-four-updater run. All-target Clippy (2.600s), codec/application formatting,
five frozen working/staged source hashes and both whitespace checks passed.
All Cargo/rustc/wrapper/application/fixture Node children and tool sessions
closed before the exclusive runtime slot transferred to the compatibility role.

The independent compatibility role's final scoped code/test audit passed with
no remaining required findings. Its five codec groups passed with zero
skips/failures in 3.01s; the actual application's new exact facade test passed in
2.78s, with 3066 unrelated tests filtered. All-target Clippy (2.448s), codec and
application formatting, five frozen working/staged source hashes and both
whitespace checks passed. It ran only the new application test, not another full
fifty-four-updater run. All owned Cargo/rustc/application/fixture Node processes
and tool sessions closed before the exclusive runtime slot returned to the
primary role. Both independent scoped code/test audits pass; final documentation
reviews precede the DCO commit and push of this bounded increment.

## T17/T44 private activation Session registration (2026-10-16)

The bounded private prerequisite now joins an actual original preinstall Session,
association, consumed authorization, completed preinstall transaction and waiting
content to a new native activation Session. Complete original update/workflow,
scope, current channel, selection/content, owner and opaque/logical context
identities are checked independently. Only workflow revision and writable
Session ID change; original window/group/observation, confirmation/resume and
waiting deadline remain unchanged. Another lawful activation may become
writable, while the previous immutable registration stays historically readable.

The explicit private initializer validates exact catalogs, migration digests,
native dependencies and all SET-reachable runtime permissions without repairing
corruption. Runtime gets readonly observations and no generic native execution
or DML. Full actual readback, current channel/CAS, SQL returned-start, source and
destination fence watermarks, final clocks, joined capacities and a live standard
T06 owner bound the compound port. Completion belongs in the same transaction.
Event-only owner replay and epoch zero cannot initialize an activation Session.

The primary's final original signed Node comparison passed three groups and
thirty-three child cases in 8.135s with zero skips/failures. It compares the
complete workflow and explicitly mapped Session fields. The final nine new real
PostgreSQL groups passed forty-eight child cases in 69.691s, with zero
skips/failures; complete native Session/association/binding are separately
compared with the private pure candidate. Controls include actual source flow,
rollback/replacement/history, tampered original update, strict owner/CAS and
clock/returned-value fences, exact 4096/1 MiB references, exact 32 MiB/+1 joined
source and postwrite growth. Both concurrency candidates prove full native+T06
positives before rollback; their race only accepts actual CAS/serialization or
deadlock refusals as losers.

Cleared defaults, full vet, module verification, original contract pins, server
build, sixteen Go files' formatting, eighteen working/staged source hashes,
fifty-seven unchanged old SQL raw bytes/index blobs and whitespace checks passed.
The default suite's 918 skipped opt-in/default cases are not actual integration
evidence. The new actual source fixture uses native initial creation,
consumption and staging completion; successful qualified authorization and
physical preparation/verification facts remain an explicit isolated bridge.

There is no public qualified decision/authorization issuer, authenticated
activation/restart event acceptance, new credential/Schema/wire field or physical
client qualification in this increment. The original task totals remain four
implemented, twelve in progress and thirty-one not completed or deferred out of
forty-seven. Full T17/T20/T44 stays open. Deployment remains deferred. Downloads
have no total/cumulative time or retry cap; credible progress or confirmable
waiting remains incomplete and only lost/unconfirmable progress becomes unknown.

The independent dependency role's final scoped code/test/doc audit passed with
no remaining required findings. Its original signed Node/pure comparison passed
three groups and thirty-three child cases in 7.258s; its actual PostgreSQL run
passed nine new groups plus three existing initial/handoff/resume regressions,
twelve groups and sixty child cases in 136.683s. Both actual selections had zero
skips/failures. Full vet, modules, pins, server build, formatting, eighteen frozen
working/index source identities, all fifty-seven unchanged old SQL raw bytes/index
blobs and whitespace checks passed. Defaults included forty-two cached packages
and 918 skipped opt-in/default cases, which are not fresh actual evidence. All
owned runtime processes, fixtures and tool sessions closed before the primary
received the exclusive runtime slot and transferred it to the compatibility role.

The independent compatibility role's final scoped code/test audit passed with no
remaining required findings. Its original signed Node/pure comparison passed
three groups and thirty-three child cases in 8.216s. Its actual PostgreSQL run
passed the same nine new and three existing regression groups: twelve groups,
sixty child cases, zero skips/failures, 130.726s. Cleared defaults passed with
forty-two cached packages and 918 skipped opt-in/default cases; these are not
fresh actual evidence. Full vet, modules, original pins, server build, formatting,
eighteen frozen source identities, fifty-seven unchanged old SQL raw bytes/index
blobs, the new SQL byte format and both whitespace checks passed. All owned
workers, fixtures, runtime processes and tool sessions closed before the primary
received the exclusive runtime slot. Both independent scoped code/test audits
pass; final documentation verification precedes DCO commit and push.


## T20/T44 original activation Session event reception (2026-10-16)

The separate service now has a fixed private activation owner for all eighteen
original client preparation edges over the protected T17 registration. It checks
original signed telemetry, current Root/role denial, immutable Session identity,
full historical source and the applicable actual native transition. Activation
failures preserve the complete workflow. Only staged verification requires
current waiting eligibility; later ordinary preparation uses the original
activation action window. Late events update only diagnostics, receipt and
outbox. Permanent replay precedes mutable registration/waiting access, while
current trust and original proof remain mandatory.

Native transition/cursor, immutable receipt/outbox, complete readback, joined
capacity and T06 completion share one transaction. Independent source/native
clocks and final waiting/action/proof windows fence acceptance. CLI output loss,
crash recovery and HTTP ordered partial commit/replay use original identities.
An HTTP error contains no successful-prefix acknowledgements; replay reconciles
already committed units without extending the original wire Schema. Admission
is explicitly disabled by default; disabling it preserves sealed recovery and
exact receipt replay.

The primary's final forty-two-source service run passed seven original signed
Node/pure groups with fifty-seven child cases (package times 8.917s/50.587s),
including ninety-one signed activation cases. The actual PostgreSQL selection
passed thirteen new and three existing regression groups: sixteen groups,
thirty-four child cases, zero skips/failures, 709.019s. Cleared defaults, full
vet, modules, pinned original contracts, server build, formatting, frozen source
identities, all fifty-eight unchanged old SQL raw bytes/index blobs and whitespace
passed. Defaults included 937 opt-in/default skips, which are not actual evidence.
A further independent review gap is covered by a forty-third, test-only source:
the corrected actual final-readback/seal test passed in 44.830s. The earlier
forty-two production/test sources remain byte-identical; this additional test is
not represented as a repeated full primary run.

Original Node terminal recovery fields are explicitly checked against native
semantics before comparing all remaining Session fields. The actual outbox's
contribution marker stays null. Original model outbox/head comparisons do not
claim the unfinished T21 owner. Qualified authorization/preparation remains an
explicit isolated fixture bridge; source creation/consumption/staging/waiting
and registration use actual native storage and ports. No production qualified
issuer, physical restart/host certification or protected durable client outbox
is claimed. Full T17/T20/T44 remains open. Totals remain four implemented,
twelve in progress and thirty-one incomplete/deferred out of forty-seven.
Deployment remains deferred. Downloads have no total/cumulative duration or
retry cap; credible progress/confirmable waiting remains incomplete and only
lost/unconfirmable progress becomes unknown.


Both controlled working-tree mutations were decisively rejected by the new
actual test: removing the final Go seal and moving it before the final activation
readback each exposed incorrect successful acceptance (`errNil=true` and an
acknowledgement). Neither red result was a compile failure or unrelated refusal.
The exact original production bytes and staged blob were restored after each
control. The restored actual test passed again, package 48.395s, with zero skips
or failures. The native observation probe uses existing SELECT-only capabilities;
it grants no extra execution privilege and leaves the clock/lease/catalog intact.


On the final forty-three-source freeze, the primary repeated cleared defaults
(387 groups, 783 child cases, 938 skipped opt-in/default cases, zero failures),
full vet and both source/old-SQL identity and whitespace checks after restoring
the two mutations. The additional skipped case is the explicitly opt-in actual
readback test. These defaults do not represent actual integration execution.


The independent dependency role's frozen forty-three-source original Node/pure
selection passed seven groups and fifty-seven child cases with zero skips or
failures (package times 9.185s/50.280s). Independently built CLI/helper binaries
were used for all seventeen actual PostgreSQL groups: fourteen new plus three
shared regressions, thirty-four child cases, zero skips/failures, 728.269s.
Cleared defaults passed 387 groups and 783 child cases with 938 opt-in/default
skips and zero failures; cached/default results are not fresh actual evidence.


The dependency role's remaining full vet, module verification, original pins,
server build, all forty-one Go files' empty formatting, forty-three frozen
working/index source identities, fifty-eight previous SQL raw/index identities
and both whitespace checks passed. Its scoped code/test audit has no remaining
required finding. All owned fixtures, workers, Go/compiler/Node/CLI/helper
processes and tool sessions closed before the exclusive runtime slot returned
to the primary and then transferred to the compatibility role.


The independent compatibility role's fresh original Node/pure selection passed
seven groups and fifty-seven child cases with zero skips/failures (workflow
7.453s, events 48.809s, including the ninety-one original signed cases). Its
complementary actual selection passed ten groups: seven new focused groups and
three existing shared regressions, nineteen child cases, zero skips/failures,
376.561s. This is a targeted ten-group run, not another full seventeen-group run.
It independently exercises the final-readback seal, current denial/permanent
replay, native historical channel fence, ordinary preparation after the original
waiting deadline, complete staged facts/fixed profile, and freshly built actual
CLI/HTTP output-loss and partial-commit paths. Cleared defaults passed 387 groups
and 783 child cases with 938 skips and zero failures; cached/default results are
not fresh actual integration evidence.


The compatibility role's full vet, module verification, original pins, server
build, forty-one Go files' empty formatting, forty-three frozen working/index
source identities, fifty-eight previous SQL raw/index identities, the two new
SQL files' UTF-8/LF/single-final-LF format and both whitespace checks passed.
Its full scoped code/test audit passes with no remaining required finding.
All owned workers, fixtures, Go/compiler/Node/CLI/helper processes and tool
sessions closed before the exclusive runtime slot returned to the primary.
Both independent code/test audits pass on the same final forty-three-source
freeze. Final documentation review precedes the DCO commit and push.

## Original unaccepted Session intake maintenance

The separate service adds a protected-owner cleanup command for four existing
fixed profiles: ordinary Session, download progress, download resume and
activation Session. It removes only complete never-accepted intake closures
after original signed credential `exp + 120000`, with no current Root/workflow
requalification or renewed upload/action authority. Shared receipts, outbox,
cursors, lifecycle/workflow/registration state and other commands stay intact.
Both resume and activation acceptance markers prevent deletion; any native live
lease retains its whole closure with full fractional timestamp precision.

Serializable pages inspect at most 32 intents, 4096 operation/sidecar pairs and
32 MiB of conservatively charged native logical row data including original
source Session bytes. Oversized closures stay whole and cannot starve later
keys. Safe nondecreasing SQL time is checked after the last catalog read before
commit. Unknown commits return no speculative page result; output-loss retry
does not create a duplicate transition or acknowledgement. Protected provenance
hashes/ciphertext lengths are not claimed as fresh signature/AEAD verification.

Eighteen new Go files and no new SQL are involved. All sixty previously delivered
SQL files retain their working bytes and index blobs. Independent static review
found the resume marker omission and a final-clock negative-control gap; both
were repaired and re-reviewed before fresh actual validation. Accepted-data
archival policy and production scheduling remain outside scope. Full T20/T44
remains in progress; totals remain four implemented, twelve in progress and
thirty-one incomplete/deferred out of forty-seven. Deployment remains deferred.
No artifact size, total/cumulative download duration or retry cap is introduced.

The primary's final eighteen-file actual PostgreSQL run passed one parent and
twenty-two child cases with zero skips/failures, 295.730s (package 299.442s).
It covers genuine signed sealed intake, natural original expiry/native leases,
both acceptance markers, post-delete/post-catalog backward/null/unsafe time,
ACL/binding/identity controls, exact FK full rollback, cross-profile accepted
evidence, real command output-loss/retry/runtime refusal, and whole-closure
4097/4096 plus source-inclusive 32 MiB/+1-byte limits and cursor progress.
Capacity padding is explicit synthetic diagnostic data, not fresh signature or
AEAD evidence. Activation retains the existing qualified-authorization and
supplied physical-fact fixture bridge. Cleared defaults passed 393 groups and
789 child cases, with 940 skipped test cases and zero failures; defaults and
cached packages are not actual integration evidence. Full vet, modules, pins,
server/fresh command builds, eighteen-file formatting/source identity and sixty
unchanged old SQL raw/index checks passed. Client architecture/version checks
also passed, keeping product version 0.10.9 and normative documents 1.0.

The independent dependency role rebuilt its own maintenance command/helper and
the original no-update command. Its actual selection passed the complete new
natural-expiry parent and three existing maintenance regressions: four groups,
twenty-seven child cases, zero skips/failures (package 305.630s), on the same
eighteen-source freeze.

Its cleared-opt-in fresh default run (`-count=1`) passed 393 groups and 789
child cases with 940 skipped test cases and zero failures. The primary's raw
952 skip records included twelve packages without tests; its skipped test case
count is also 940. Neither default run represents actual integration execution.

The dependency role's full vet, module verification, original pins, server
build, eighteen-file formatting/frozen identities, all sixty old SQL raw/index
identities and whitespace checks passed. Its scoped code/test audit passes
with no remaining required finding; all owned workers, fixtures, processes and
tool sessions closed before it returned the exclusive runtime slot.

The compatibility role independently passed four complementary actual groups
and ten child cases with zero skips/failures (package 126.798s), covering
original maintenance, ordinary Session preservation, resume acceptance/replay
and activation profile/sibling privileges. This focused regression run is not
another complete natural-expiry parent. Its fresh command/config/event-PG
defaults passed forty groups and twenty-four child cases with one opt-in helper
skip; default evidence remains separate from actual integration execution.

Its fresh full default run (`-count=1`) passed 393 groups and 789 child cases,
940 skipped tests, twelve packages without tests and zero failures. Full vet,
modules, original pins, server build, eighteen-file formatting/frozen identities,
all sixty previous SQL raw/index identities and whitespace checks passed.
Its scoped code/test audit passes with no remaining required finding. All owned
fixtures, workers, processes and tool sessions closed before the runtime slot
returned. Both independent audits pass on the same eighteen-source freeze;
final documentation review precedes DCO commit/push. Full task counts and the
download behavior stated above remain unchanged.

## Original unaccepted transaction intake maintenance

The separate service adds fixed protected-owner transaction intake cleanup for
the existing shared namespace: v1 late-only and v2 late/live command attempts
over one permanent original event identity. Narrow internal sizing/locking/
wire/deletion/clock mechanisms are shared with the four existing Session
profiles; their public API and native acceptance-marker behavior remain intact.
The original exact Session, consumed authorization and transaction must prove
their complete consumption association. Current Root, workflow, waiting or
qualification cannot shorten or renew original `exp + 120000` retention.

Twenty-seven Go sources are frozen, with no SQL changes. All sixty previously
delivered SQL files retain their working bytes and index blobs. Each attempt
uses its own actual fixed command's digest domain. Any live lease or shared
accepted evidence retains the complete closure; only complete never-accepted
processing intake is deleted.
The source-inclusive 32 MiB logical budget counts all three original bodies and
headers, with 32 intents/4096 attempts per page. Final safe nondecreasing SQL time
follows all catalog checks; an error or unknown commit returns no speculative
counts or cursor. Output-loss retry never creates another event acknowledgement.

Both independent static reviews passed the implementation and tests after the
primary repaired logged/rewrite source checks, complete original consumption
associations and independent whole-command ledger counting. Real three-purpose
native smoke passed one parent and three children, zero skips/failures (package
28.806s). Qualified issuance and supplied physical preparation remain explicit
fixture bridges. The first exploratory natural-expiry run failed only an
incorrect v1 initialization fixture; it was repaired to use the existing frozen
v1 DDL. That failed run is not final acceptance evidence. Final frozen primary
and independent runtime evidence follows.

The primary's final actual PostgreSQL selection passed three groups and sixty-
three child cases with zero skips/failures (package 640.859s), using freshly
rebuilt transaction and Session command/output-loss helpers. The shared original
four-profile Session natural-expiry parent passed in 294.010s; transaction
natural expiry passed in 294.720s with thirty-eight children; the three-purpose
real native smoke passed in 48.850s. It proves original signed windows, natural
native leases, otherwise-valid source faults/restoration, same-identity mixed
command domains, exact native FK full rollback, final catalog followed by three
faulted clocks, unchanged v1 structure, UNLOGGED persistence fault bracketed by
actual cleanup positives, actual cross-command accepted evidence, fresh command
output-loss/retry/runtime refusal and whole count/source-byte/cursor limits.
Synthetic capacity proof/ciphertext padding is not signature or AEAD evidence.
Independent whole-command ledger counts reject orphaned selected operations.
The twenty-seven Go and sixty old SQL raw/index freeze checks remain intact.

The primary's cleared-opt-in fresh default run (`-count=1`) passed 399 groups
and 802 child cases, with 945 skipped test cases, twelve packages without tests
and zero failures. Passed group/child counts exclude skipped cases; defaults
are not actual integration evidence. Full vet, module verification, private
server build, original pinned contracts, twenty-seven-file formatting/freeze,
all sixty old SQL identities and whitespace checks passed. Both client
workspaces passed architecture/version checks; product 0.10.9 and normative
documents 1.0 remain unchanged. All primary runtime processes and tool sessions
closed before independent runtime review.

The independent dependency role rebuilt its own transaction maintenance command,
output-loss helper and original no-update command. Its separate actual selection
passed four groups and forty-three child cases with zero skips/failures (package
305.294s): the complete transaction natural-expiry parent in 284.960s with thirty-
eight children, plus three original native ACL/protected-owner/final page-clock
maintenance regressions with five children. This is independent actual evidence
on the same twenty-seven-source freeze.

Its cleared-opt-in fresh full defaults (`-count=1`, package parallelism two)
passed 399 groups and 802 children, 945 skipped tests, twelve packages without
tests and zero failures. Full vet, modules, original pins, private server build,
twenty-seven-file formatting/frozen working/index identities, removed old path,
all sixty old SQL raw/index identities and whitespace checks passed. Its scoped
code/test audit passes without remaining required findings. All owned fixtures,
workers, compiler/test/Node/CLI/helper/runner processes and tool sessions closed
before returning the exclusive runtime slot. The compatibility role's runtime
verification follows separately.

The independent compatibility role rebuilt its own Session maintenance command
and output-loss helper. Its complementary actual run passed two groups and
twenty-five children with zero skips/failures (package 324.741s): the complete
original four-profile Session natural-expiry parent in 285.620s with twenty-two
children, plus the new three-purpose native transaction smoke in 35.620s with
three children. This is not another full transaction natural-expiry parent;
the primary and dependency roles independently executed that complete parent.

Its cleared-opt-in fresh defaults (`-count=1`) passed 399 groups and 802 children,
945 skipped tests, twelve no-test package records and zero failures. Full vet,
modules, original pins, private server build, twenty-seven Go sources' empty
formatting and frozen working/index identities, the removed old source path,
all sixty previous SQL raw/index identities and whitespace checks passed. The
before/after freeze digest is identical. Its scoped code/test audit passes
without required findings. All owned fixtures, workers, compiler/test/CLI/helper/
runner processes and tool sessions closed before returning the exclusive runtime
slot. Both independent code/test audits pass on the same twenty-seven-source
freeze; final documentation review precedes DCO commit/push.

Accepted-data archival, production scheduling, durable protected client queues,
independent task-event owners and full T17/T20/T44 completion remain open. Totals
remain four implemented, twelve in progress and thirty-one incomplete/deferred
out of forty-seven. Deployment remains deferred. No artifact download size,
retry or total/cumulative duration cap is introduced.

## Accepted transaction minimum retention inspection

The separate service adds a fixed protected-owner READ ONLY inspection API and
`inspect-transactions` command for accepted transaction events. Elapsed is only
the original minimum time floor, never archive/deletion authority or consumer
completion. Original receipt/outbox, immutable intake and exact native Session,
consumed authorization and transaction must agree. Missing located evidence
refuses the entire page; no transaction intake means Unclassified without a
retention conclusion. Lookup uses both permanent key and original JTI/event tuple.

The floor remains immutable exp + 120000, equal to the original receipt retention.
All observations follow actual receipt/native time watermarks and earlier clocks;
the final safe clock follows the last catalogue/ACL/READ ONLY check. Unknown
commit/error returns no speculative page result. Existing v1 late-only and v2
late/live dispositions remain original under permanent replay. Current channel,
qualification, waiting and workflow authority do not renew original evidence.

Fourteen Go sources are frozen. All sixty previous SQL raw/index identities and
the original cleanup/Session public APIs remain intact. Scalar lengths precede
payload transfer; at most 32 points share the source-inclusive 32 MiB logical
resource budget. Whole oversize points retain an advancing private cursor.
Fixed CLI output is aggregate only after the complete scan; pages are separate
observations and output loss requires a read-only retry.

Both independent static reviews passed the production and test repairs. The
primary moved the fixed transaction NULL quality restriction after Unclassified
classification. Current schema does not permit an otherwise-valid actual
non-NULL watermark; pure binding negatives and lawful NULL Unclassified actual
controls are reported separately. Exploratory test failures exposed an incorrect
replay ACK expectation, private canonical SQL use and an FK-unsafe test restore;
all were repaired, with in-place original-key restoration and cleanup safeguards.
Those failed runs are not final acceptance evidence.

The repaired primary actual PostgreSQL selection passed three groups and twenty-
six children, zero skips/failures (package 304.682s): capacity 35.490s, genuine
three-purpose late/live acceptance and replay 55.470s, original natural expiry
205.220s. Fresh CLI and two helpers verify receiptAt-1 at/above all native times,
restored receiptAt, final-only backward/unsafe/NULL-Scan observations after full
native/catalogue reads, unknown commit, output loss/retry/runtime refusal,
original evidence faults/restored positives and v1 unchanged. All full native,
accepted, intake and operation fingerprints remain unchanged by inspection.
Synthetic exact-byte padding/source sentinels prove budget/cursor ordering,
not signatures, AEAD or qualified business facts.

Accepted archival policy, full T17/T20/T44, durable protected client queues, T21
contribution, qualified issuers/physical hosts and production deployment remain
open. Totals remain four implemented, twelve in progress and thirty-one
incomplete/deferred of forty-seven. No artifact size, retry or total/cumulative
download duration cap is introduced.

The primary's cleared-opt-in fresh full defaults passed 402 groups and 807
children, with 955 skipped tests, twelve packages without tests and zero
failures. Skipped cases are excluded from passed counts; defaults are separate
from actual integration evidence. Full vet, modules, private server build,
original pinned contracts, fourteen-source formatting/frozen working/index
identities, all sixty prior SQL raw/index identities and whitespace checks
passed. Both client architecture/version checks passed, preserving product
0.10.9 and normative documents 1.0.

The independent dependency role built its own command and both helpers and
passed the complete new natural-expiry parent: one group, twenty children, zero
skips/failures (parent 203.250s, package 206.643s). This is independent actual
evidence for that parent, not another full three-group combined selection.
Its fresh defaults passed 402 groups/807 children, 955 test skips plus twelve
no-test package records, zero failures. Vet, modules, original pins, private
server build, fourteen-source format/frozen working/index identities, all sixty
old SQL raw/index identities and both whitespace checks passed. The freeze
manifest digest is unchanged. Its scoped code/test audit has no required finding;
all owned fixtures, workers, compiler/test/CLI/helper/Node/runner processes and
sessions closed before returning the exclusive runtime slot. Compatibility
verification follows separately.

The independent compatibility role's complementary actual selection passed
three groups/nine children, zero skips/failures (package 129.163s): new six-case
late/live native acceptance/replay, capacity and the original three-purpose
maintenance native parent, preserving old fixture defaults. It did not repeat
the complete natural-expiry parent (executed by primary and dependency) and did
not build/run fresh CLI/helpers; no extra CLI E2E evidence is claimed here.

The complementary parents passed in 57.000s (new six-case retention), 33.610s
(capacity) and 35.480s (old three-purpose maintenance). Its fresh defaults passed
402 groups/807 children, 955 test skips plus twelve no-test package records,
zero failures. Full vet, modules, pins, private server build, fourteen-source
format/frozen working/index identities, all sixty previous SQL raw/index
identities and whitespace checks passed. The freeze digest is unchanged. Its
scoped code/test audit has no required finding. All owned fixtures, workers,
Go/compiler/test/server/CLI/helper/runner processes and tool sessions closed
before returning the exclusive runtime slot. Both independent code/test audits
pass on the same fourteen-source freeze; final documentation review precedes
DCO commit/push. Task totals and deferred boundaries stated above are unchanged.

Both final independent documentation reviews passed. Service delivery is DCO
committed and pushed as 60e6d17a0d2346d29add7728e9c761e8db44ab13 on
`codex/t20-transaction-retention`. The accepted-data and full-task boundaries
above remain open; both client progress records are synchronized separately.

## Supplied existing-Root envelope signature verification

The client adds a feature-owned pure verifier for the ordinary frozen V1
single-envelope operation. It verifies original bounded canonical bytes, the
complete embedded role Schema, independent full context, fresh supplied Root,
exact declared policy, every actual Ed25519 signature and configured unique
threshold, credential signingKeyId, supplied deny facts, body time and original
metadata high-water rules. Bad/unknown/denied extra signatures still refuse
after a valid prefix meets the threshold. Root claims must equal the existing
Root; a genuine successor requires the separate transition operation.

The provider reuses exact ring 0.17.14 already in the App/CLI graph. Production
remains the default; explicit trusted-caller Development matches the reference's
production=false incremental descriptor rule. The App facade fixes Production.
The same genuinely signed incremental package succeeds in explicit Development
and refuses through that facade. Packages with high-water refuse; credentials
ignore that argument, preserving the original applicability. Existing codec APIs
keep their production behavior, with narrow immutable Root key/window reuse.

This proves signature checks only under independently supplied existing Root,
context, safe time and deny/high-water facts. No protected bootstrap, trusted
clock acquisition, atomic accepted-state persistence, complete Root-chain
installation, archive mode, current qualification, execution permission or
durable queue acknowledgement is added. Full T27 remains open. Deployment and
platform/custody qualification remain deferred. Task totals remain four
implemented, twelve in progress and thirty-one incomplete/deferred of forty-seven.
No artifact size, retry or total/cumulative download duration cap is introduced.
The primary passed thirteen verifier tests: three original-vector tests and ten
groups containing exactly 111 full frozen Node acceptance/error oracle cases.
They cover all twenty original envelopes, complete signature processing, role
and product scopes, time/high-water applicability, Production/Development,
resource limits and malformed Ed25519 A/R points and S scalars. The sampled
provider preflight also agreed across Node, ring and Go on sixty controls; this
is compatibility evidence, not universal cryptographic certification or a new
weak-point blacklist. Strict all-target verifier Clippy passed. All 157 codec
tests passed with zero failures/ignored; the two opt-in original Node oracles
were additionally executed genuinely, within those same 157 cases.

The actual default App facade passed forty tests, zero failures/ignored. The
actual CLI-workspace backend and its App/verifier dependencies built with the
locked offline graph and passed its library harness (zero unit tests, 22m56s).
An additional App benchmark-hooks/local-embed facade run passed forty tests,
zero failures/ignored (5.05s after compilation). Neither backend compilation
nor App harnesses constitute CLI workflow E2E. Earlier external-package Cargo
selection attempts failed at harness selection and are excluded from acceptance.

Selected-root App and CLI dependency trees each contain 104 dependency packages
and 109 feature-variant nodes. The nine verification-critical packages have
identical versions, sources and features. The complete closures are not identical:
original locks already differ in icu_provider 2.3.0/2.3.1 and zerovec-derive
0.11.5/0.11.6. This batch changes no existing package version/source. Embedded
Schema validation explicitly disables format validation and retrieves only the
fixed common URN; both independent reviewers confirmed these baseline patch
differences do not affect this operation. This is Windows graph evidence;
physical platform qualification and release packaging remain open.

The independent dependency role's final scoped code/test audit passed with no
required finding. Its own verifier run passed thirteen tests/111 full Node cases,
zero failures/ignored (10.056s wall); strict all-target Clippy passed. Its actual
default App facade passed forty tests (2.960s harness), and its actual CLI backend
library harness passed with zero unit tests (78.612s wall), without an additional
benchmark run. Scoped formatting, architecture, version, working/index whitespace
and before/after sixteen-source freeze checks passed. All its owned compiler,
runner, Node and tool sessions closed before returning the exclusive runtime slot.

The independent compatibility role's final scoped code/test audit also passed
with no required finding. Its own verifier run passed thirteen tests/111 fresh
full Node cases, zero failures/ignored (9.928s wall); strict all-target Clippy
passed. Its actual default App facade passed forty tests (2.640s harness), and
its actual CLI backend passed its zero-test library harness (9.022s wall).
Its own selected-root graph reconstruction reproduced the nine critical package
identities and the two disclosed baseline patch differences. Formatting,
architecture, version, narrow lock changes, before/after sixteen-source freeze
and working/index whitespace checks passed. All its owned runtime processes and
sessions closed before returning the exclusive slot. No CLI E2E is claimed.

Both final code/test audits apply to the same sixteen-source freeze, manifest
SHA256 8a8b54ae5e42bc4fec91fc9e56894a047c19915b9376f3e04c0217ecdb4ca738.
The primary's final scoped format, strict codec library Clippy, architecture,
version, narrow parent locks, selected dependency graphs and whitespace checks
also passed. Product 0.10.9, normative documents 1.0, original pinned V1 contracts
and CodeWhale gitlink remain unchanged. Final progress documentation review
precedes DCO commit/push; all task and authority boundaries above remain open.

## Single supplied-Root successor verification

The client now verifies exactly one genuine Root successor in the feature-owned
pure verifier. The mandatory independent complete Root context binds product,
role, null component and empty scope without serializing an unbounded product
string. Original canonical bytes, full Schema and Root descriptor/policy
consistency are checked. The old Root permits expiry only, retaining its
original window and issued-time constraints; the next Root must be fresh and
exactly version + 1, within the original maximum safe integer.

Every submitted actual Ed25519 signature must belong to the union of the exact
old/new Root-role policies. Each policy separately meets its verified-member
threshold, shared signers may count in both, and next descriptors take precedence
for repeated IDs. Invalid or unauthorized extras still refuse after sufficient
valid signatures. The ordinary verifier reuses the private wire/context and
whole-Root signature workers, retaining its fresh-Root production behavior.

The caller explicitly supplies the complete active metadata read set. Each
original entry binds its full independent expected context and non-Root metadata
Schema, then every actual next-Root signature and exact role policy. This checks
key preservation rather than current body eligibility: expired or otherwise
ordinary-semantic-invalid but Schema-valid metadata, including incremental
Package descriptors, still needs its original key. Empty/repeated entries are
allowed and every entry is checked; no aggregate 4096-entry or 32 MiB policy,
body-time/Production/high-water/deny option is added. Per-object original limits
remain. The result cannot attest caller snapshot/read-set completeness.

The immutable result preserves original successor bytes, borrows the exact old
Root and owns the validated next Root. The App exposes the same narrow operation.
It does not install/persist a Root, reset high-water, establish protected lineage,
bootstrap, trusted time or current rights. Complete chains, archive identities,
protected OS custody and full T27 remain open. Totals remain four implemented,
twelve in progress and thirty-one incomplete/deferred of forty-seven. Deployment
and physical platform qualification remain deferred. No artifact size, retry or
total/cumulative download duration cap is introduced.

Twenty verifier tests passed: the original thirteen tests/111 full Node cases,
six groups with fifty-five fresh full frozen Node successor cases, and a separate
mandatory independent Root-context API test with five negative controls. New
cases include distinct dual 2-of-3 and overlapping policies, all extras,
consecutive/max-safe versions, old/next windows, all five metadata roles,
4097 actually checked repeated active entries, multiple components/bad last,
expired and invalid-window/incremental active bodies, original raw/Schema limits,
and five Schema-valid descriptor/policy inconsistencies. Strict all-target
verifier Clippy passed. Keys are generated only inside Node and never exported.
Earlier exploratory failures were a missing new cache-field initializer and a
wrong future-old test error label; both were repaired before final acceptance.
The frozen reference and Rust both require TIME_WINDOW_INVALID for future old
Roots in the expired-history branch; no production relaxation was needed.

The primary's complete codec regression passed all 157 tests, zero failures or
ignored (549.809s wall), including both explicitly enabled original Node oracles.
Those two are within the same 157 tests, not additional unique tests. Strict
codec library Clippy passed. The actual default App facade passed forty-one
tests, zero failures/ignored (168.276s including compilation). The new App test
uses six genuine full-reference controls for both Root policies and active key
retention, alongside the previous forty facade tests.

The actual CLI-workspace backend and its production App/verifier dependencies
built and passed the zero-test library harness (150.204s wall). This is Windows
dependency-graph evidence, not CLI workflow E2E. The selected App and CLI trees
retain 104 dependency packages/109 feature nodes each, identical verification-
critical nine package versions/sources/features, and only the previously disclosed
icu_provider and zerovec-derive baseline patch differences. None of the three
App, CLI or verifier lockfiles changes in this batch. Architecture, version,
scoped formatting, standard repository whitespace and final frozen-source checks
passed; an initial custom whitespace command misclassified valid Windows CRLF,
so the repository's normal check was used without changing source bytes.

The independent dependency role's final scoped code/test audit passed without a
required finding. Its own twenty verifier tests/111 ordinary and fifty-five
successor Node cases plus five API negative controls passed, zero failures or
ignored (17.903s wall). Strict all-target Clippy passed. Its actual default App
passed forty-one tests (4.840s harness, 25.078s wall); the actual CLI backend
zero-test harness passed (8.996s wall). Formatting, architecture, version,
before/after/final ten-source freeze and working/index whitespace checks passed.
Its private runner initially combined harmless line-ending stderr warnings with
empty stdout; a separate corrected final check passed without source changes or
functional reruns. All owned runtime processes and sessions closed before return.

The independent compatibility role's final scoped code/test audit also passed
without a required finding. Its own twenty verifier tests/111 ordinary and
fifty-five successor Node cases plus five API negative controls passed, zero
failures/ignored (13.843s wall); strict all-target Clippy passed. Its actual App
facade passed forty-one tests (8.090s harness, 17.042s wall); its actual CLI backend
zero-test library harness passed (8.627s wall). Formatting, architecture, version,
all three unchanged locks, before/after ten-source freeze and standard working/
index whitespace checks passed. It reused the unchanged selected graph evidence
and did not rerun the codec suite or extra benchmark. All its owned processes and
sessions closed before returning the exclusive runtime slot.

Both independent final code/test audits apply to the same ten-source freeze,
manifest SHA256 3f83c848e9c4d94f773a8b9281863ae75758652fba4051da21d5d7e3d91f43bf.
Product 0.10.9, normative documents 1.0, pinned V1 contracts and CodeWhale gitlink
remain unchanged. Final documentation review precedes DCO commit/push. Complete
chain and archive plans reviewed separately are future scopes, not implemented
by this single-successor batch; all task/authority boundaries above stay open.

## Complete supplied-Root chain verification

The feature-owned pure verifier now checks one through 4096 supplied original
Root successors. Each step retains the original full canonical/Schema/context,
descriptor/policy, product, exact consecutive safe version, all actual signatures
and separate old/new Root-role threshold checks. Only a nonfinal next Root may
be expired, retaining original window and issued-time constraints; the final Root
must be fresh. A later error returns no valid prefix. Public ordinary and single-
successor operations retain their fresh-Root behavior and original defaults.

A private owned transition-parts type and private next-window selection avoid
constructing a public fresh-successor proof for expired intermediates. Temporary
previous envelope/cache state is released each step. The immutable chain result
borrows the exact supplied initial Root and owns only final original envelope,
validated final Root, final verified signer IDs, checked step count and supplied
time. It does not retain cloned intermediate envelopes or persist/install Roots.

Caller-owned active metadata key preservation is checked only under the final
Root; temporary intermediate key retirement followed by final restoration is
allowed. All supplied final active entries still bind full independent context
and verify actual exact-policy signatures, without ordinary body-time/Production
eligibility, high-water, deny or new aggregate set/byte rules. Caller snapshot
consistency/completeness and protected original chain storage are not attested.
The original 4096 chain count rule is not an artifact download limitation.

The primary's twenty-eight verifier tests passed, zero failures/ignored: the
previous twenty tests/111 ordinary and fifty-five successor reference cases plus
eight new chain tests. Thirty new full frozen Node chain cases are separate from
the fifty-five reused successor fixtures that additionally execute both complete
frozen operations and both Rust APIs as length-one chains. API negative controls
also require complete independent Root context. Chain bounds include an actual
4096-step positive, every distinct consecutive body genuinely re-signed with a
valid unchanged Root policy/key set, and a 4097 invalid-byte list rejected before
parsing. Other controls use distinct real rotations, old/intermediate/final
windows, both thresholds/bad middle extras, final key retention/bad last, exact
product/version and every step's original raw/full-Schema boundaries. Private
keys remain inside Node and never enter emitted fixtures. Strict all-target
Clippy passed. Both frozen static deep audits found no required issue.

The codec source subtree, all three locks and dependency manifests are unchanged.
Its previous 157 actual regression tests, including two explicitly enabled Node
oracles, retain acceptance and are not rerun without a new relevant change.
The App facade adds the same narrow complete-chain operation and six genuinely
signed positive/negative controls. No workflow E2E, protected bootstrap/lineage
custody, trusted clock acquisition, atomic high-water acceptance, Root storage/
installation, current qualification or execution rights are claimed. Full T27 and
historical archive identities remain open. Totals remain four implemented,
twelve in progress and thirty-one incomplete/deferred of forty-seven. Physical
platform qualification and deployment remain deferred. No artifact size, retry
or total/cumulative download duration cap is introduced.

The primary's actual default App facade passed forty-two tests, zero failures/
ignored (11.390s harness, 111.556s wall). The actual CLI-workspace backend and
production App/verifier graph built and passed the zero-test library harness
(140.173s wall); this is compilation/graph evidence, not CLI workflow E2E.
Scoped formatting, architecture, version, standard whitespace and final frozen
source checks passed. The unchanged codec subtree, all three lockfiles and
manifests retain the prior selected App/CLI graph evidence: 104 dependency
packages/109 feature nodes each, identical nine verification-critical package
versions/sources/features, and only the disclosed preexisting icu_provider and
zerovec-derive patch differences. Full closure identity is not claimed.

The independent dependency role's final scoped code/test audit passed without a
required finding. Its own twenty-eight verifier tests passed with zero failures/
ignored (47.936s wall): fixed three, ordinary ten/111 cases, chain eight tests
(30 new cases plus 55 reused parity fixtures and five API controls), and successor
seven/fifty-five cases plus five API controls. The reused parity fixtures are not
counted as fifty-five new distinct chain scenarios. Strict all-target Clippy
passed. Its actual App facade passed forty-two tests (14.640s harness, 24.246s
wall); its actual CLI backend passed the zero-test library harness (8.952s wall).
Formatting, architecture, version, unchanged codec/locks, before/after nine-source
freeze and standard working/index whitespace checks passed. All owned runtime
processes and sessions closed before returning the exclusive slot.

The independent compatibility role's final scoped code/test audit also passed
without a required finding. Its own twenty-eight verifier tests passed with zero
failures/ignored (30.863s wall), keeping 30 new chain cases and 55 reused one-step
parity fixtures distinct. Strict all-target Clippy passed. Its actual App facade
passed forty-two tests (14.690s harness, 23.580s wall); its actual CLI backend
zero-test library harness passed (8.731s wall). Formatting, architecture, version,
unchanged codec/three locks, before/after nine-source freeze and standard working/
index whitespace checks passed. It reused the unchanged selected graph evidence,
without codec/metadata/extra-benchmark reruns. All owned runtime processes and
sessions closed before returning the exclusive slot.

Both independent final code/test audits apply to the same nine-source freeze,
manifest SHA256 04f17c11d6a9be6485672a9eb2a18414f71d9df04c588a91c7cc273e24869e2f.
Product 0.10.9, normative documents 1.0, pinned V1 contracts and CodeWhale gitlink
remain unchanged. Final documentation review precedes DCO commit/push. Historical
archive identity plans are a later separately reviewed scope, not implemented
by this chain batch; protected custody/acceptance and full-task boundaries above
remain open.

## Historical Package and Release signature identities

The feature-owned verifier now provides separate original Package and Release
identity operations under caller-supplied archived Root and independently expected
complete context. Each checks original bounded canonical bytes, full Schema,
exact role policy, all actual Ed25519 signatures and configured threshold. The
immutable historical proof preserves original bytes, borrows the exact archived
Root and records verified signer IDs; it is distinct from ordinary fresh proof.

Package uses supplied safe time: Root expiry alone is allowed, retaining original
positive bounded window and issued-at checks. Production body semantics remain
fixed, including incremental-package refusal. Release takes no current time: its
Root original window is checked at that Root's issuedAt and its Release original
window at that Release's issuedAt. No cross-object ordering is added; valid
Root-after-Release and Release-after-Root-expiry controls establish identity only.
No denial/high-water/development switch is added to these archive operations.

Five new verifier tests execute thirty-six full frozen Node cases: Package six,
Release eight, signatures/thresholds eight, independent complete context eight,
and wire-limit/closed-Schema six. Historical positives additionally assert
ordinary verification refusal where applicable. Private keys remain inside Node.
Two narrow App facades add one test exercising eleven genuine positive/negative
cases for both operations. No production codec source, manifests or locks change.

Both final independent static deep audits passed the same seven-source freeze,
manifest SHA256 78ce97c26f3530e94bfddd6bb740cd926df5203cdb63d58fbf161a67ab441a29.
The primary's full thirty-three verifier tests passed, zero failures/ignored:
previous twenty-eight tests and five new archive tests (1.060s archive harness).
Strict all-target Clippy passed (5.343s wall). Existing 111 ordinary, 55 successor,
30 new chain and 55 reused length-one parity fixtures remain distinct regressions.
The unchanged codec's prior 157 actual tests, including two enabled Node oracles,
retain acceptance without rerunning an unaffected exhaustive suite.

Protected archive lineage/custody and snapshot consistency remain caller-owned.
Historical identity grants no current qualification, download, activation,
installation or acknowledgement rights, and specifies no service accepted-data
archival, retention or deletion policy. Protected trust/high-water persistence,
clock acquisition and complete T27 workflow remain open. Totals remain forty-seven:
four implemented, twelve in progress, thirty-one incomplete/deferred. Product
0.10.9, normative documents 1.0, pinned V1 and CodeWhale remain unchanged. Physical
platform qualification and deployment remain deferred. No artifact download size,
retry or total/cumulative duration cap is introduced.

The primary default App facade passed forty-three tests, zero failures/ignored
(12.300s harness, 111.679s wall). The actual CLI-workspace backend built the
production verifier/App graph and passed its zero-test library harness (140.999s
wall), which is compilation evidence rather than workflow E2E. Formatting,
architecture, version, normal whitespace and final seven-source freeze passed.
The unchanged selected dependency graph retains 104 packages/109 feature nodes
per App/CLI root and identical nine verification-critical packages/features;
previously disclosed ICU/zerovec patch differences remain, not full closure identity.

The independent dependency role's own thirty-three verifier tests passed with
zero failures/ignored (58.070s wall), including five archive tests/36 cases
(1.860s harness). Strict all-target Clippy passed (7.799s wall). Its default App
facade passed forty-three tests (16.780s harness, 55.094s wall); its actual CLI
backend zero-test library harness passed (19.03s wall). Formatting,
architecture, version, unchanged source/dependency baseline, before/after freeze
and normal working/index whitespace checks passed. Runtime closure is confirmed
before the next independent role receives the exclusive slot.

The independent compatibility role's own thirty-three verifier tests passed with
zero failures/ignored (47.521s wall), including all five archive
tests/36 full Node cases. Strict all-target Clippy passed
(3.188s wall). Its actual default App
facade passed forty-three tests (15.040s harness,
23.789s wall); its actual CLI backend zero-test library harness
passed (8.744s wall). Five-source Rust formatting, architecture,
version, unchanged codec/three locks, reused actual selected graph evidence,
before/after seven-source freeze and normal working/index whitespace all passed.
No E2E or unrelated benchmark/codec-suite rerun is claimed. Both independent
roles closed every owned runtime process/session before returning the slot.

Both final code/test audits apply to the same seven-source freeze above, with no
required finding. Final documentation review precedes DCO commit/push. The
separately reviewed authenticated-public-graph plan is future work, not delivered
by this archive batch; the remaining T27/task and authority boundaries stay open.

## Complete authenticated public metadata chain

The feature-owned verifier now composes the original complete public-chain
operation under caller-supplied existing Root, independent product/component,
safe time and global/exact-role denial facts. Each Timestamp, Snapshot and
referenced member passes unchanged ordinary fresh-Root, full Schema/context,
all-signature, exact-policy and Production semantics checks. Role-specific deny
is consulted only for that role; unrelated entries have no effect. No high-water
or Development argument is introduced into this operation.

The authenticated Timestamp binds original Snapshot bytes. That authenticated
Snapshot supplies each member's role and complete scope, while product/component
always come from independent bounded context. Escaped bounded JSON creates each
complete expected Document. Exact original references and complete membership
are checked, then the unchanged codec validates Target baseline/ordinary/bridge
reachability, Release targets/Package identities/modes and candidate-leak refusal.
Repeated Snapshot references fail original ordinary metadata semantics. The
Snapshot-derived necessary actual-member bound is applied before allocating member collections,
without inventing an independent aggregate byte/member or artifact policy.

The immutable public-chain proof owns checked original Timestamp/Snapshot/member
envelopes, preserves member input order and borrows the exact supplied Root with
supplied time. It exposes authenticated original objects, not Node's derived
result arrays. Successful private ordinary proofs are consumed into owned data;
temporary expected/deny facts do not escape. Failure returns no valid prefix.
No protected trust/clock custody, durable atomic high-water acceptance, current
online qualification, candidate selection or execution rights are established.

Five new tests execute fifty-six full frozen Node public-chain controls: thirty-
four relationship cases, nine denial/isolation, five actual signatures/thresholds,
three windows and five exact-policy/wire/Production. Twenty-eight relationship
scenarios reuse the existing complete Node graph fixture logic; six additional
membership/multi-Target scenarios and twenty-two authentication controls are
separate additions, not fifty-six previously unseen protocol scenarios. Private
keys stay inside Node. Genuine two-Target/two-Package graphs use independent exact
Root policies and shuffled five-member order, with second-Target policy/channel
scope negative controls. Within-count duplicate members, oversized lists,
repeated Snapshot references, ordinary/bridge reachability, bad extra after a
valid threshold, unsigned body changes and role-denial isolation are covered.

Independent static review found a missing multi-Target test, which the primary
repaired with the complete signed positive/negative controls above. A test-only
mapping now preserves the existing Rust ROLE_POLICY_NOT_DECLARED label against
Node ROLE_NOT_AUTHORIZED; reference/Schema labels are similarly mapped. Test
fixture preflight corrections retain ARM hostArchitectures and original duplicate
reference semantics. No production algorithm was changed to satisfy the tests.
Both final frozen static deep audits passed without a required finding.

The primary's thirty-eight verifier tests passed, zero failures/ignored (75.304s
wall), including five new public-chain tests/56 controls (2.070s harness). Strict
all-target Clippy passed (14.089s wall). The prior ordinary111, successor55,
archive36, thirty new chain and fifty-five reused length-one parity cases remain
separate regressions. One narrow typed App facade adds one test with nine genuine
full-operation positive/negative controls. The ordinary verifier, codec source,
manifests and all three locks are unchanged; prior codec157 actual tests,
including two enabled Node oracles, retain acceptance without an unaffected rerun.

Both final source/test audits apply to the six-source freeze, manifest SHA256
5721005371b5c4e2800ff87b6de6e6f69c943c44be0ab9851526f653113ad07c.
Full T27 remains in progress. Overall totals remain forty-seven: four implemented,
twelve in progress, thirty-one incomplete/deferred. Product 0.10.9, normative
document versions 1.0, pinned V1 contracts and CodeWhale remain unchanged. Physical platform
qualification and deployment remain deferred. No artifact download size, retry
or total/cumulative duration cap is added.

The primary default App facade passed forty-four tests, zero failures/ignored
(13.850s harness, 197.728s wall). The actual CLI-workspace backend built the
production App/verifier graph and passed the zero-test library harness (200.228s
wall); it is compilation/graph evidence rather than workflow E2E. Four-source
Rust formatting, architecture, version, before/after six-source freeze and normal
working/index whitespace checks passed. No codec, manifest or dependency lock
changed; prior selected App/CLI graph evidence retains 104 dependency packages/
109 feature nodes each, identical nine verification-critical packages/features
and disclosed preexisting ICU/zerovec patch differences, not full closure identity.

The independent dependency role's final scoped code/test audit passed without a
required finding. Its own thirty-eight verifier tests passed, zero failures/
ignored (51.453s wall); public-chain five tests/56 cases took 1.970s harness.
Strict all-target Clippy passed (3.820s wall). Its actual default App facade
passed forty-four tests (15.250s harness, 25.335s wall); its actual CLI backend
zero-test library harness passed (8.766s wall), not workflow E2E. Four-source
Rust formatting, architecture, version, before/after six-source freeze and normal
working/index whitespace passed. Unchanged codec/three-lock/core graph acceptance
was reused. All its owned runtime processes/sessions closed before handoff.

The independent compatibility role's final scoped code/test audit also passed
without a required finding. Its own thirty-eight verifier tests passed, zero
failures/ignored (47.541s wall), including five public-chain tests/
56 complete Node controls. Strict all-target Clippy passed
(3.474s wall). Its actual default App facade
passed forty-four tests (17.010s harness,
25.823s wall); actual CLI backend zero-test library harness passed
(8.884s wall). Four-source Rust formatting, architecture, version,
unchanged codec/three-lock baseline, before/after six-source freeze and standard
working/index whitespace passed. No workflow E2E or unrelated codec/benchmark
rerun is claimed. All owned runtime processes/sessions closed before handoff.

Both independent final audits apply to the same six-source freeze above. Final
documentation review precedes DCO commit/push. The separately approved candidate-
opening authentication plan remains future work, not delivered by this public-
chain batch; protected acceptance and complete T27 boundaries remain open.

## Authenticated candidate Release and complete opening

The feature-owned verifier now composes the complete frozen candidate operation
using original Target/Release bytes, bounded opening, supplied existing Root/safe
time/global denial and independent bounded product/component/channel/targetKey/
expectedReleaseId context. Fixed-role expected Documents share independent product
and component; incoming claims never supply expected Release identity. Escaped
bounded values construct the complete Target and Release scopes, ignoring only
extra caller context after original whole-input limits.

Both envelopes pass unchanged ordinary fresh-Root, full Schema/context, actual
all-signature/exact-policy and body checks. The unchanged codec then verifies the
exact original Release hash differs from Target baseline, all seven nonnull
opening commitment fields and Release target membership. Both authentications
precede final relationships; only multi-fault diagnostic precedence may differ
from Node. Existing Rust opening/policy/Schema labels are explicitly mapped,
without changing acceptance or original ordinary/codec behavior.

The immutable candidate proof owns checked original Target/Release, borrows exact
supplied Root and records supplied time. Temporary expected Documents do not
escape consumed private ordinary proofs. Failure returns no partial pair. It
proves supplied signatures and commitment, not candidate selection, protected
trust/clock custody, atomic acceptance, current online qualification or execution.
No high-water, role-denial or Development option is added to this operation.

Five new tests execute thirty-eight full frozen Node cases: thirteen opening/
baseline/membership, seven independent shared context/Release identity, eight
actual signatures/thresholds/denial, four windows and six policy/wire/Schema.
Five genuine positives include empty ordinary/bridge catalogs, restored opening,
context extras, actual two-signer Target and irrelevant denied key. Negative
controls mutate every opening field, null/unknown fields, both actual signature
paths, independent expectedReleaseId/component, original windows and exact policy.
The missing-target control uses a genuine separately signed ARM Package reference.
Fresh private keys remain in Node. One narrow App facade test exercises ten
actual positive/negative controls. Both final scoped static deep audits passed
without a required finding on the same six-source freeze, manifest SHA256
46d9dbfb89f6dff8149b7e4054ee12411cbe09ef9cf8a555b33ffae76c84770d.

The primary's forty-three verifier tests passed, zero failures/ignored (66.621s
wall), including new candidate five tests/38 cases (1.180s harness). Strict
all-target Clippy passed (10.384s wall). Prior ordinary111, successor55, archive36,
public-chain56, thirty new chain and fifty-five reused length-one parity cases
remain separate regressions. Ordinary verification, codec source, manifests and
all three locks are unchanged; prior actual codec157 tests including two enabled
Node oracles retain acceptance without an unaffected exhaustive rerun.

Full T27 remains in progress. Overall totals remain forty-seven: four implemented,
twelve in progress, thirty-one incomplete/deferred. Product 0.10.9, normative
document versions 1.0, pinned V1 and CodeWhale remain unchanged. Physical platform
qualification and deployment remain deferred. No artifact download size, retry
or total/cumulative duration cap is introduced.

The primary default App facade passed forty-five tests, zero failures/ignored
(14.160s harness, 167.052s wall). The actual CLI-workspace backend built the
production App/verifier graph and passed its zero-test library harness (187.413s
wall); this is compilation/graph evidence, not workflow E2E. Four-source Rust
formatting, architecture, version, before/after six-source freeze and standard
working/index whitespace checks passed. The unchanged actual selected App/CLI
graph retains 104 packages/109 feature nodes each and identical nine critical
verification packages/features; preexisting ICU/zerovec patch differences stay
disclosed, not a claim of full closure identity.

The independent dependency role's final scoped code/test audit passed without a
required finding. Its own forty-three verifier tests passed, zero failures/
ignored (53.970s wall); candidate five tests/38 full Node controls took 1.080s
harness. Strict all-target Clippy passed (3.447s wall). Its default App facade
passed forty-five tests (15.760s harness, 26.199s wall); its actual CLI backend
zero-test library harness passed (8.818s wall), not workflow E2E. Four-source
formatting, architecture, version, unchanged codec/manifests/three-lock baseline,
before/after six-source freeze and standard working/index whitespace passed.
It reused prior selected graph evidence. All owned processes/sessions closed
before returning the sole runtime slot.

The independent compatibility role's final scoped code/test audit also passed
without a required finding. Its own forty-three verifier tests passed, zero
failures/ignored (48.064s wall), including all five candidate tests/
38 full Node controls. Strict all-target Clippy passed
(3.358s wall). Its default App facade passed
forty-five tests (17.980s harness,
27.032s wall); its actual CLI backend zero-test library harness
passed (8.589s wall), not workflow E2E. Four-source Rust formatting,
architecture, version, unchanged codec/manifests/three-lock baseline, before/
after six-source freeze and standard working/index whitespace passed. Prior
selected graph evidence is reused without unrelated codec/benchmark reruns.
All owned runtime processes/sessions closed before returning the exclusive slot.

Both final independent audits apply to the same six-source freeze above. Final
documentation review precedes DCO commit/push. The separately approved signed-
Package/supplied-Release binding plan is future work, not implemented by this
candidate batch; full T27 and protected acceptance/authority boundaries stay open.

## Signed Package authentication and supplied Release binding

The feature-owned verifier now implements the complete frozen Package operation
with original Package bytes, caller-supplied canonical full-Schema Release,
supplied existing Root/time/global denial and independent bounded product/
component/targetKey. It finds the supplied Release target and checks every original
reference field before authenticating Package with unchanged ordinary fresh-Root,
full Schema/context, actual all-signature/exact-policy and Production checks.
The unchanged codec binds target, appVersion, launcher and activation-mode subset.
Existing closed-reference/Schema/policy labels remain explicitly mapped in tests.

The original operation authenticates only Package metadata. It neither verifies
supplied Release signatures nor checks that object's product/component/context/
original window. Callers must independently authenticate Release before any
qualification. The immutable SignatureCheckedPackageBinding owns checked_package,
borrows the exact supplied_release Schema object and supplied Root, and records
time; getters preserve those distinct proof levels. This is not a downloaded
artifact-byte proof. No partial proof, metadata acceptance, protected custody,
current online qualification, selection or execution permission is returned.
There is no high-water, role-denial or Development option in this operation.

Five new tests execute thirty-five complete frozen Node cases: thirteen exact
reference/business, four supplied Release boundaries, three independent context,
eight actual signatures/thresholds/denial and seven windows/policy/wire/Production.
Ten positives include mode ordering/subsets, foreign product/component, expired
or invalid-signature supplied Release, context extras, actual two-signer Package
and irrelevant denial. All supplied Release inputs independently pass the full
Schema/canonical factory. These positives assert only Package authenticity plus
supplied-value binding, never authenticity of Release or current rights. Every
reference field and Package signature path has a negative control; private keys
stay in Node. A misleading fixture comment was clarified to describe reusing the
original caller-owned envelope wrapper rather than re-signing mutated claims.
One narrow App facade test exercises ten genuine positive/negative controls.

Both independent final scoped static deep audits passed the same six-source
freeze without a required finding, manifest SHA256
e8294f10bf6e6e98d5aadf81eaddf88fdf2397cec8be734419462cc6312eb6c6.
The primary's forty-eight verifier tests passed, zero failures/ignored (73.960s
wall); new Package five tests/35 cases took 1.040s harness. Strict all-target
Clippy passed (8.535s wall). Prior ordinary111, successor55, archive36, public-
chain56, candidate38, thirty new chain and fifty-five reused length-one parity
controls remain separate regressions. Ordinary/codec source, manifests and all
three locks are unchanged; prior actual codec157 tests including two enabled
Node oracles retain acceptance without rerunning an unaffected exhaustive suite.

The primary default App facade passed forty-six tests (15.390s harness,
164.679s wall), zero failures/ignored. The actual CLI-workspace backend built the
production App/verifier graph and passed its zero-test library harness (177.813s
wall), which is compilation/graph evidence, not workflow E2E. Four-source Rust
formatting, architecture, version, before/after six-source freeze and standard
working/index whitespace passed. The unchanged selected graph retains prior
104 packages/109 feature nodes per App/CLI root and nine identical verification-
critical packages/features; preexisting ICU/zerovec patch differences remain
disclosed rather than a full-closure equivalence claim.

The dependency auditor independently ran the same frozen six-source tree:
forty-eight verifier tests passed (57.278s wall), including new five/35 controls
in 1.210s harness; strict Clippy passed (3.693s). Default App forty-six passed
(16.430s harness / 26.751s wall); actual CLI backend zero-test build/harness
passed (9.010s). All formatting, architecture, version, standard whitespace and
before/after freeze checks passed. Session 37308 closed exit zero; all owned
runner/compiler/runtime processes were zero. Codec, manifests and three locks
were explicitly unchanged; selected-graph evidence remains reused.

The compatibility auditor independently repeated all gates on the same freeze:
forty-eight verifier tests passed (53.623s wall), new five/35 controls 1.200s
harness; strict all-target Clippy 3.249s; default App forty-six 18.440s harness /
27.332s wall; CLI backend zero-test compilation/harness 8.542s wall. Four Rust
format checks, architecture, version, normal working/index whitespace and both
six-source freeze checks passed. Session 41690 closed exit zero; all auditor-owned
runners/compiler/runtime and newly spawned Node processes were zero. Codec,
manifests and three locks were independently confirmed unchanged; prior selected
graph and codec evidence remains reused, not a newly run exhaustive test claim.

Both auditors also passed the next bounded successful check/refresh-response
correlation plan: complete contracts, actual new Decision/Telemetry verification
and independently supplied request-fact binding. Its old Refresh request decision
retains contract-only time semantics; metadata/Registry/generation acceptance and
workflow writer CAS remain separate owners. This plan is not delivered behavior.

Full T27 remains in progress. Overall forty-seven tasks still comprise four
implemented, twelve in progress and thirty-one incomplete/deferred. Product
0.10.9, normative document versions 1.0, pinned V1 and CodeWhale are unchanged.
Physical platform qualification and deployment remain deferred. No artifact
download size, retry or total/cumulative duration cap is introduced.

## Authenticated successful Check/Refresh response correlation

The feature-owned verifier now composes complete original request/response
contracts, actual ordinary authentication of both new Decision and Telemetry
credentials, and explicitly normative correlation to the caller's independent
original request. Product/component and complete scope come from that request,
not incoming response claims. Nonce, installId, host, four fact digests, current
version and derived sourceProfileId must match; stable product/component/target
must agree with request.scope. Existing complete response pair/session checks
remain unchanged. Both new credentials require supplied fresh Root, exact roles,
all actual signatures/global denial and actual supplied current time.

Inline response credentials have canonical identities, preserve signature array
order and original credential bounds. Original transport whitespace/key order is
allowed and exact original response bytes are retained. No fabricated original
envelope substring, trust bootstrap, protected fact probe, persistent metadata
acceptance or current workflow writer permission is claimed. The proof owns
canonical checked Decision/Telemetry envelopes, borrows the exact request/Root
and records operation/time. Metadata/Registry/generation qualification and
protected workflow CAS remain later owner checks.

The old Refresh request Decision deliberately has original contract semantics
only. It may be expired or have an unverified actual signature; it does not gain
new rights through this correlation proof. New response credentials still require
actual current verification. New update.workflowId must equal the Refresh request
workflowId, while response workflowRevision is not forced to old+1. This keeps
long download continuation possible without extending any credential window or
introducing a total/cumulative download-duration or retry limit.

Seven tests execute sixty-two composed Node controls, with twelve positives:
successful/continuation boundaries eight, request facts/workflow fifteen,
independent complete context six, credential pair/session six, actual signatures/
thresholds/denial twelve, windows eight and contract/wire/resource seven. Both
credentials are checked exactly at expiration and one millisecond before. Bad
last signatures after an otherwise valid threshold, duplicates, wrong exact role policy,
expired prior Refresh Decision, unverified old actual signature, independent
revision and original-response/canonical-credential identity controls are present.
The oracle invokes complete frozen assertApiContract, actual verifyEnvelope twice
and original sourceProfileId, then explicitly new normative comparisons. It is
not claimed as an unchanged frozen full-response operation. Private keys stay in
Node. One App facade test exercises twelve genuine controls.

Both independent scoped final static deep audits passed the six-source freeze,
manifest SHA256 f2075ac161619d0332926f42b99366b7ad1971e6606cd0eba827ed5e8e03b188,
without a required finding. The primary's fifty-five verifier tests passed with
zero failures/ignored (87.653s wall); new seven/62 controls took 4.070s harness.
Strict all-target Clippy passed (9.018s wall). Original codec/ordinary verifier,
manifests and three locks remain unchanged. Prior actual codec157 including two
enabled Node oracle tests retains acceptance; the unchanged selected graphs retain
104 dependency packages/109 feature nodes per App/CLI root and nine identical
verification-critical packages/features. Previously disclosed ICU/zerovec patch
differences remain; no full-closure equivalence or newly rerun exhaustive codec
suite is claimed.

The primary actual default App facade passed forty-seven tests with zero failures/
ignored (17.030s harness / 172.097s wall). The actual CLI-workspace backend built
the production App/verifier graph and passed its zero-test library harness
(210.125s wall); this is compilation/graph evidence rather than workflow E2E.
Four Rust formatting checks, architecture (16.415s wall), version, before/after
six-source freeze and normal working/index whitespace checks passed. Root session
37400 closed exit zero; all known primary-owned compiler/runtime/runner/Node
processes were zero before granting the dependency auditor exclusive execution.

The dependency auditor independently repeated all gates on the same frozen tree:
fifty-five verifier tests passed (67.537s wall), new seven/62 controls 3.490s
harness; strict Clippy 8.267s; default App forty-seven 18.210s harness / 54.666s
wall; CLI backend zero-test build/harness 19.080s wall. Four Rust format checks,
architecture, version, normal working/index whitespace and before/after freeze
all passed. Session 25087 closed exit zero; all owned runners/Cargo/compiler/
Node/test processes were zero. Codec, manifests and three locks were explicitly
unchanged; prior selected-graph evidence remains reused.

The compatibility auditor independently repeated all gates on the same freeze:
fifty-five verifier tests passed (60.016s wall), new seven/62 controls 3.530s
harness; strict all-target Clippy 3.314s; default App forty-seven 20.920s harness /
29.835s wall; CLI backend zero-test compilation/harness 8.920s wall. Four Rust
format checks, architecture, version, standard working/index whitespace and both
six-source freeze checks passed. Session 52453 closed exit zero; all owned
runner/compiler/runtime/test processes and newly spawned Node processes were
zero. Codec, manifests and three locks were explicitly confirmed unchanged;
prior selected-graph and actual codec acceptance remain reused.

Both independent reviewers passed the next bounded signed DownloadInfo plan:
complete frozen contracts, current Decision plus new Download authentication,
complete shared qualification/update inheritance and current URL window. Refresh
historical-input semantics remain separate. URL/Decision expiry requires fresh
qualification and a new short-lived URL; it does not terminate a logical download
or erase cache. Destination/file/native/metadata/Registry/workflow owner checks
remain distinct. This plan is not delivered behavior in the current batch.

Full T27 remains in progress. Overall forty-seven tasks remain four implemented,
twelve in progress and thirty-one incomplete/deferred. Product 0.10.9, normative
versions 1.0, pinned V1 and CodeWhale remain unchanged. Physical platform
qualification and deployment remain deferred; no artifact download size, retry
or total/cumulative duration cap is introduced.

## Signed DownloadInfo request/response inheritance

The feature verifier now composes unchanged complete DownloadInfo request and
response contracts with actual ordinary verification of both the current request
Decision and new Download credential. Independent original request.scope supplies
full product/component/installation/channel/revision/target expectations. Both
roles require fresh supplied Root/current time, all actual signatures/exact
policies and global denial. This differs deliberately from Refresh historical
inputs: current file eligibility requires a currently valid Decision.

Decision must offer an update for the requested actual-hop package. All nineteen
shared qualification fields, including full metadataSet, forwardPath and update,
match the new Download credential. Role-specific jti/key/type/audience/purpose and
windows remain independent; no signed downloadKey or HTTP requestId is invented.
The unchanged response contract binds packageId/size/SHA and nbf<URLexpiry<=exp;
this verifier additionally requires supplied now<URLexpiry. Expired Decision/URL
requires rechecking/refreshing and a fresh short-lived URL; it never terminates a
logical download, clears cache or adds a total/cumulative/retry duration cap.

The immutable SignatureCheckedDownloadResponse owns both canonical checked
envelopes and exact original response bytes, borrows exact request/current Root
and records supplied time. Signature-array order remains unchanged. This is no
native-fact, protected custody, complete metadata/Registry/generation acceptance,
active workflow/writable-session, destination approval, artifact-byte or execution
proof. Existing network destination and complete-byte checks plus protected owners
remain necessary before useful operations. No partial authenticated result escapes.

Seven test groups execute sixty-one composed Node controls, nine positives:
general success five, inherited qualification sixteen, independent context seven,
package/outer binding seven, signatures twelve, windows eight and wire six.
All nineteen common fields are covered by inheritance/context controls, including
complete update and forwardPath; current Decision expiry is isolated with a fresh
Download credential. Both actual thresholds, bad extras/duplicates/denial/exact
policy, distinct role key/jti, no-update/wrong package, URL and credential expiration
and one millisecond before, full contracts, original bytes/canonical identities,
closed Schema and resource bounds are exercised. The oracle composes complete
frozen assertApiContract and actual verifyEnvelope twice plus explicitly new
normative inheritance/URL-now comparisons; it is not an unchanged frozen full
operation. Private keys stay in Node. One App facade test exercises twelve genuine
controls. Initial seven/61 passed (4.200s harness); naming-only clarification then
changed old/new locals to decision_claims/download_claims before the final freeze.

Both independent scoped final static audits passed the six-source freeze without
a required finding, manifest SHA256
eb77ed89e15abd3e2179199fbb7ad21f6fb573912834e6a38ebd948b451ff029.
The primary's sixty-two verifier tests passed with zero failures/ignored (93.793s
wall); new seven/61 controls took 4.160s harness. Strict all-target Clippy passed
(9.802s wall). The default App protocol facade passed forty-eight tests, zero
failures/ignored (19.120s harness / 190.503s wall).

Codec, ordinary verifier, manifests and three locks remain unchanged. Prior
actual codec157 including two enabled Node oracle tests retains acceptance;
unchanged selected App/CLI graphs retain 104 packages/109 feature nodes each and
nine identical verification-critical packages/features, with preexisting two
ICU/zerovec patch differences already disclosed. No full-closure equivalence or
new exhaustive codec execution is claimed. Actual CLI/App builds are not workflow
E2E. Full protected workflow and physical qualification remain later work.

The primary actual CLI-workspace backend built the production App/verifier graph
and passed its zero-test library harness (197.329s wall), solely compilation/graph
evidence. Four Rust format checks, architecture (21.497s), version, before/after
six-source freeze and standard working/index whitespace all passed. Session 26960
closed exit zero; all known primary-owned compiler/runtime/runner/Node processes
were zero before granting the dependency auditor exclusive execution.

The dependency auditor independently repeated all gates on the same freeze:
sixty-two verifier tests passed (76.088s wall), new seven/61 controls 4.700s
harness; strict all-target Clippy 3.455s; default App forty-eight 25.560s harness /
35.234s wall; actual CLI backend zero-test compilation/harness 9.337s wall.
Four Rust format checks, architecture, version, standard working/index whitespace
and before/after six-source freeze all passed. Session 91476 closed exit zero;
all owned runners/Cargo/compiler/Node/test processes were zero. Codec, manifests
and three locks were explicitly unchanged; prior selected-graph evidence is
reused rather than newly claiming equivalent entire dependency closures.

The compatibility auditor independently repeated all gates on the same freeze:
sixty-two verifier tests passed (59.893s wall), new seven/61 controls 4.810s
harness; strict all-target Clippy 3.783s; default App forty-eight 24.410s harness /
33.269s wall; actual CLI backend zero-test compilation/harness 8.651s wall.
Four Rust format checks, architecture, version, normal working/index whitespace
and before/after six-source freeze all passed. Session 11428 closed exit zero;
all owned runner/compiler/runtime/test processes and newly spawned Node processes
were zero. Codec, manifests and three locks were independently confirmed unchanged;
prior actual codec and selected-graph evidence remains reused.

Both reviewers passed the next bounded authenticated Release renewal plan:
compose unchanged archived identity, ordinary current verification and complete
immutable-business renewal binding. Historical/current Roots and proof levels
remain distinct; strict higher revision need not be consecutive. Archive custody,
Root lineage, full chain publication and atomic high-water replacement remain
later protected-owner responsibilities. This plan is not delivered behavior here.

Full T27 remains in progress. Forty-seven tasks remain four implemented, twelve
in progress and thirty-one incomplete/deferred. Product 0.10.9, normative versions
1.0, pinned V1 and CodeWhale remain unchanged. Physical platform qualification and
deployment remain deferred. No artifact download size, retry or total/cumulative
duration cap is introduced.

## Authenticated immutable Release renewal

The feature-owned verifier composes unchanged archived Release identity, ordinary
current Release verification and complete codec renewal binding. Fixed role and
independent bounded product/component/releaseId apply to both original envelopes.
Previous and next use separately supplied archived/current Roots. Each historical
object's original window is checked at its own issuedAt; no retroactive current
deny/window or cross-object issuance ordering is added. Next requires supplied
fresh Root/current time/global denial and all actual exact-role signatures.

Renewal requires strictly greater revision and exact signed business equality,
excluding only revision/issuedAt/expiresAt. Consecutive revisions or matching
signature arrays are not required. The immutable result keeps the distinct
ArchivedEnvelopeIdentity and SignatureCheckedEnvelope wrappers, original bytes,
signer identities and exact Root pointers. The current wrapper safely retains
owned checked parts and rebinds the exact input current Root lifetime; no temporary
expected-context reference escapes. No unsafe construction or partial proof is
introduced. Archive custody/completeness, Root rotation lineage, atomic metadata
high-water/cache replacement, complete publication/generation/current rights and
candidate renewal opening remain their protected owners' responsibilities.

Six tests run forty-seven composed Node controls, nine positives: renewal/revision
eight, business seven, independent context six, actual signatures twelve,
windows seven and wire seven. They compose the three unchanged original full
functions rather than claiming a nonexistent full renewal helper. Coverage
includes expired history, separately supplied current Release policy/key,
unchanged Root/key with the SAME Root object passed in both standalone input
roles, revision gaps/safe maximum, immutable content changes, full expected
scope, both actual thresholds/bad extras/duplicates/exact policies, current denial
without retroactive archive denial, historical future-own-issued-time/no cross
ordering, next expiry/future window and last valid millisecond, canonical/full
Schema/size limits and original bytes/Root/time/signer getters. Private keys stay
in Node. One App facade test exercises twelve genuine controls. Root-policy
fixtures never attest actual Root rotation lineage.

Initial six/46 passed (1.390s harness); self-review added the no-rotation/same-Root
positive before the final freeze. One private generator preflight initially tried
to sign an unsafe revision that the frozen signing helper correctly refuses;
the malformed negative now mutates the valid original envelope before full
verification. This changed only negative-fixture preparation, no protocol rules.

Both independent final scoped static deep audits passed the same six-source
manifest SHA256 ddd3ac060e7342243390b733b4e4bf8c5a60995cf56dbd76b03be038b036895e,
without a required finding. The primary's sixty-eight verifier tests passed,
zero failures/ignored (96.207s wall); new six/47 controls took 1.620s harness.
Strict all-target Clippy passed (9.240s wall). Default App protocol facade
forty-nine passed, zero failures/ignored (22.980s harness / 170.357s wall).
Actual CLI-workspace backend built the production App/verifier graph and passed
its zero-test library harness (204.074s wall), solely compilation/graph evidence,
not workflow E2E.

Ordinary/archive/codec implementations, manifests and three locks are unchanged.
Prior actual codec157 including two enabled Node oracle tests retains acceptance;
unchanged selected App/CLI graphs retain 104 packages/109 feature nodes each and
nine identical verification-critical packages/features, with preexisting two
ICU/zerovec patch differences already disclosed. No newly run exhaustive codec
or full-closure equivalence claim is made.

Four Rust format checks, architecture (16.943s wall), version, before/after
six-source freeze and standard working/index whitespace all passed. Primary
session 34845 closed exit zero; all known owned compiler/runtime/runner/Node
processes were zero before granting the dependency auditor exclusive execution.

The dependency auditor independently repeated all gates on the same freeze:
sixty-eight verifier tests passed (68.239s wall), new six/47 controls 1.310s
harness; strict all-target Clippy 3.378s; default App forty-nine 20.750s harness /
32.748s wall; actual CLI backend zero-test compilation/harness 8.810s wall.
Four Rust format checks, architecture, version, standard working/index whitespace
and before/after six-source freeze all passed. Session 4458 closed exit zero;
all owned runners/Cargo/compiler/Node/test processes were zero. Ordinary/archive/
codec source, manifests and three locks were explicitly unchanged; prior selected
graph evidence remains reused.

The compatibility auditor independently repeated all gates on the same freeze:
sixty-eight verifier tests passed (61.522s wall), new six/47 controls 1.240s
harness; strict all-target Clippy 3.318s; default App forty-nine 24.730s harness /
33.449s wall; actual CLI backend zero-test compilation/harness 8.497s wall.
Four Rust format checks, architecture, version, standard working/index whitespace
and before/after six-source freeze all passed. Session 36886 closed exit zero;
all owned runner/compiler/runtime/test processes and newly spawned Node processes
were zero. Codec/archive/manifests/three locks were explicitly unchanged; ordinary
verification's lib delta adds only registration/exports, no behavior change.
Prior actual codec and selected-graph evidence remains reused.

A separate independent read-only task inventory confirmed concrete unfinished
T44/T47 functionality; neither is merely waiting for a final test run. T44 lacks
production successful validation -> available Authorization + authorized Session
installation (existing tests seed that record). T47 still lacks executable
lifecycle owner composition, non-null approved SN-set sources and full recovery
reason/frozen-state carrier integration. Their T19/T14/T15/T21 business owners
remain separate; these findings do not expand the current renewal code audit.

The next concrete increment is T44's private successful-validation installation
kernel, same real T06 transaction/activeValidate/owner lease/scope/session CAS,
with actual Authorization creation, final fences/clock and downstream recovery/
consume/cancel/expiry tests. The plan was independently reviewed and repaired to
include all existing capacity dimensions (canonical refs 1MiB, member 1MiB,
4096/32MiB plus terminal/cleanup growth and revision reserve). Actual full Schema/
consume-purpose binding and historical-success-marker semantics remain mandatory;
no runtime/HTTP writer or fabricated T19 qualification is permitted. This is an
approved next implementation plan, not delivered functionality or task completion.

Full T27 remains in progress. Forty-seven tasks remain four implemented, twelve
in progress and thirty-one incomplete/deferred. Product 0.10.9, normative versions
1.0, pinned V1 and CodeWhale remain unchanged. Physical platform qualification and
deployment remain deferred. No artifact size, retry or total/cumulative download
duration cap is introduced.

## Private successful-validation installation

The T44 lifecycle kernel now has a private actual processing Validate -> available
Authorization + authorized Session installation port. It starts with real
begin_validate and commits the success marker, authorization, parent and scope
revision in one actual T06 transaction. Expected scope/Session/Validate revisions,
original owner/epoch/lease, unchanged preparation, current channel, actual waiting
activation lineage and final safe/nondecreasing SQL time are repeated. Success
history remains readable after consume/cancel/expiry/channel changes and cannot
reopen validation. No production T19 command, runtime writer or HTTP endpoint is
registered; qualification, signature/custody and complete final read-set guards
remain actual T19 owner composition responsibilities.

The Go proposal check applies the unchanged full update and consume Schemas,
including actual workflow, installer/type/backup, context/plans, preparation,
freeze and backup data bindings. Its discarded synthetic consume shape allocates
no recovery identity or execution right. Exact hashes alone remain insufficient
proof of signatures/current eligibility. SQL repeats actual lifecycle bindings;
activation re-reads the stored waiting slot and completed preinstall transaction.

Additive migration 16 joins installation/capacity/recovery SQL in one digest.
Sixty already tracked SQL files remain raw/index byte identical. The protected
capacity projection reserves missing Attempt/Outcome/Transaction/preinstall Staged
and a durable recovery-closure fact, plus terminal growth/cleanup/counter reserve;
actual records are not counted twice. Exact/+1 tests cover 4096 members including
scope, 32 MiB logical data, 1 MiB canonical refs, 1 MiB member and finite revisions.
These internal limits do not cap artifact size, retry count or download duration.

Original 62d cleanup now records an immutable recovery closure for new terminal
authorizations, including identities with no Attempt. Cleanup and closure share
scope locking and final nonrollback time. Repeated/concurrent scans close once;
late recovery reconstruction remains refused even after clock rollback. Original
Authorization/Session/Validate/Transaction/accepted-event history is retained;
no additional archival/deletion authority is inferred. Shared native source checks
authenticate permanent heap/columns/constraints/sole PK index/view and readonly ACL;
optional V14/V15 source compatibility remains, and exact optional 16 is recognized.

Independent reviews found and repaired native workflow binding, state-constraint
catalogue, complete future capacity, clock-based premature reserve release,
concurrent cleanup and extra-index cases. The final competing-outcomes test first
proves each branch fully writes and completes T06 then rolls back; the same code
races with original PgError identity and exact permitted conflict causes. Final
history assertions read the actual marker independently of activeValidateId.

Primary session 26972 closed with exit zero on the unchanged 26-file manifest
810ca8701bf1566d4fead79500a59febc3db0e956d9c897aaf4eb7de22e110bd.
Module verification, default full service tests (46 tested packages; 413 top-level
and 830 child passes, 997 skipped tests), vet, server build and pinned V1 checks
passed. Separate affected lifecycle tests used actual PostgreSQL and passed
222 top-level and 359 child tests, with zero skips or failures; the native package
took 844.944s. Go formatting, standard whitespace and all 60 existing SQL raw/index
checks passed. All owned runtimes were zero before exclusive reviewer execution.
An unnecessarily wide additional actual-PostgreSQL run was intentionally stopped;
its partial results do not count as acceptance. Its only leftover isolated test
database and associated role were verified and removed.

The dependency auditor independently passed pure 42 top-level/127 child tests
(2.701s package; 8.689s wall) and actual PostgreSQL 33 top-level/71 child tests
(151.117s package; 158.840s wall), with zero skips/failures in those scoped runs.
Default service tests passed the same 413 top-level/830 child tests, with 997
skipped tests and 12 no-test packages separately recorded (177.538s wall).
Vet (5.011s), module verification (9.294s), server build (31.369s), corrected
pinned-artifact check (2.481s), format, whitespace and before/after source/SQL
freezes passed. Initial runner 35294 closed nonzero only because its private
artifact-check command omitted the required source argument; corrected remaining
gates closed zero in 63330 without repeating passed tests. Owned runtimes were
zero before explicit compatibility-auditor handoff. No repository source changed.

The compatibility auditor independently passed pure 42 top-level/127 child tests
(2.859s package; 9.014s wall) and the same actual PostgreSQL 33 top-level/71 child
tests (146.603s package; 159.103s wall), with zero skips/failures in those scoped
runs. Fresh cleared-default service tests passed 46 tested packages, 413 top-level
and 830 child tests; 997 test skips and 12 no-test packages remain separately
recorded (198.942s wall). Vet (9.182s), module verification (5.069s), private server
build (31.315s), pinned V1 check with explicit source (3.147s), all 22 changed Go
format checks, standard differences and before/after source/SQL freezes passed.
Session 61102 closed exit zero; all owned runner, compiler, test/server and new
Node processes were zero before returning the slot. No repository source changed.

Both independent scoped final audits passed the same frozen implementation after
the primary repaired every required finding. The service implementation is DCO
committed and pushed as `c6b09bc7c3304a8523be6aedda154b5182ed3065` on
`codex/t44-validation-success`; its worktree is clean and matches upstream.
This delivers the bounded installation increment, with whole T44 still in progress.

A separate independent T44 whole-task inventory identified two remaining actual
features: durable successful-backup evidence and same-transaction original
backupCleanupAt=terminalAt+24h for an unconsumed terminal Session; controlled
unexpired waiting invalidation/replacement for revoked/reselected content. Both
next scopes passed independent planning review. The backup scope uses one private
per-Session fact with terminal growth reserve; legacy missing evidence/time remains
explicit Unknown without a guessed deadline, placeholder member or blocked old
cancellation/timeout. Waiting uses separate fixed control/event replay-policy
wrappers over a private core, actual distinct original/new preinstall sources,
atomic replacement and a final cross-step time/readback fence. These are approved
next scopes, not implemented functionality, local deletion or whole T44 acceptance.

Forty-seven tasks remain four implemented, twelve in progress and thirty-one
incomplete/deferred. Product 0.10.9, normative 1.0, pinned V1 and CodeWhale are unchanged.
Deployment and physical platform qualifications remain separate. Downloads retain
no hard total/cumulative duration limit.

## T44 durable unconsumed-backup deadlines (2026-10-16)

Additive lifecycle migration 17 records one permanent fact only when the actual
private session event owner accepts backup_succeeded under its T06 OwnerReplay
fence. It binds the original preparation, freeze, data scope and SQL completion
time. Every first unconsumed Session terminal atomically records its original
terminalAt and exact backupCleanupAt=terminalAt+24h. Cancellation, validation
failure, authorization end/expiry, due compensation and channel invalidation
share this rule. An actual consumed Transaction excludes it. Repeated observations
never rebase either time; missing legacy evidence remains explicit unknown.
The private complete-read-set observation does not grant local deletion authority.

Admission reserves evidence growth and remaining terminal writes against the
existing member, logical aggregate, encoded-reference and revision bounds. Both
independent reviews and actual regressions found compatibility gaps and the
primary repaired them: new backup admission now preserves all existing legacy
credentials' missing late original Attempt/Outcome and actual future recovery
closure. Only real cleanup at original authorization expiry+62d creates that
closure, deletes original recovery material and advances the scope in one
transaction with a final clock fence. Migration, reads and admission never invent
historical facts. Actual terminal parent and consumed Transaction bindings remain
required; scopes without backup evidence or successful validation keep their old
cleanup behavior. A recorded closure prevents reopening after clock rollback.

The primary passed the final affected real PostgreSQL regression: 31 top-level
and 56 child tests, zero skips/failures (178.184s wall). Default full-service tests
passed 46 tested packages, 420 top-level and 830 child tests (64.562s wall);
1028 explicit test skips and 12 no-test packages are recorded separately. Module
verification, vet, private server build, pinned V1, Go formatting and both working
and cached whitespace checks passed. An earlier broader lifecycle regression
passed 236 top-level and 387 child tests before the final legacy-capacity delta;
that broader result is not represented as a rerun of the final implementation.

Both independent auditors passed the frozen scoped source and performed separate
fresh pure-backup and actual PostgreSQL runs. The dependency auditor passed pure 1 top-level test (0.763s package,
7.354s wall) and actual PostgreSQL 20 top-level/28 child tests (92.442s package,
100.414s wall). The compatibility auditor independently passed pure 1 top-level
test (1.026s package, 6.135s wall) and the same actual PostgreSQL 20 top-level/28
child tests (105.625s package, 113.539s wall). These scoped runs had zero skips or
failures. Sessions 77307 and 64800 closed exit zero, joined all owned children and
returned the runtime slot with zero owned Go/compiler/test processes. Before/after
21-file raw source, 63 existing SQL raw/index and complete status/index checks
passed without source edits. ROOT's nine final gates closed exit zero in 39231.

The bounded service increment is DCO committed and pushed as
`4d1389cd7b3ab44f672cf9f8d62804210beb95f0` on `codex/t44-backup-cleanup`;
the service checkout is clean and matches its upstream.

This bounded increment does not complete T44. Controlled unexpired waiting
invalidation/replacement and separate whole-task acceptance remain. T19 HTTP and
qualification composition, T30 local backup/deletion, physical certification and
deployment are not delivered by this increment. Downloads retain no hard total
or cumulative duration limit. Product 0.10.9, normative 1.0, pinned V1 and CodeWhale
remain unchanged. Totals remain four implemented, twelve in progress and
thirty-one incomplete/deferred across all 47 tasks.

## T44 controlled waiting invalidation and replacement (2026-10-16)

Additive lifecycle migration 18 supplies fixed private control and event-owner
ports for retiring unconsumed, unexpired waiting content and atomically replacing
it with a different actual completed preinstall source. Retirement preserves all
original identities, preparation/slot history and deadlines. Replacement binds
the current channel and actual Session/Authorization/Transaction lineage; the
original staging-completion signature and result remain compatible. Neither port
grants current qualification, physical slot writes or local deletion authority.

Independent reviews found and the primary repaired original-channel migration
compatibility, last-obligation revision capacity, helper ACL authentication and
final-readback test isolation. Cross-step SQL watermarks include retirement's
final observation, the new completion's first observation and the outer fence
after all actual readbacks. NULL, unsafe or regressing clocks and either original
deadline reject the whole write set. Complete member, aggregate, encoded-reference
and convergence reserves remain enforced; no earlier SQL file was rewritten.

The primary passed the complete lifecycle feature with actual PostgreSQL enabled:
pure domain 44 top-level/127 child tests and PostgreSQL 212 top-level/286 child
tests, zero skips/failures (1037.867s combined wall). Default full-service
tests passed 46 tested packages, 426 top-level and 830 child tests (70.296s wall).
Its 1055 explicit test skips and 12 no-test packages are separate from that native
feature result. Modules, vet, private server build, pinned V1, Go formatting and
both working/cached whitespace checks passed. All 18 changed source files and
64 existing SQL raw/index entries stayed frozen throughout these nine gates.

Both independent auditors passed the frozen scoped code and performed separate
fresh runs. The dependency auditor passed pure 1 top-level test (0.663s package,
7.267s wall) and actual PostgreSQL 15 top-level/26 child tests (87.311s package,
95.026s wall). The compatibility auditor independently passed pure 1 top-level
test (1.178s package, 7.637s wall) and the same actual PostgreSQL 15 top-level/26
child tests (79.835s package, 91.910s wall). Both scoped runs had zero skips or
failures. Sessions 72489 and 17974 closed exit zero, joined owned children and
returned the runtime slot with zero owned processes. All 18 frozen source paths,
64 existing SQL raw/index entries and full index/status checks remained unchanged.
ROOT's complete nine-gate session 25814 also closed exit zero. Final scoped review
status is PASS; these results do not complete whole T44.

The bounded service increment is DCO committed and pushed as
`879eca5ee43aa8fe805ce3477da37950dfd293db` on `codex/t44-waiting-invalidation`; the checkout is clean and
matches upstream. Native regressions cover full T06 completion/rollback, actual
activation/channel/expiry competitors, exact resource boundaries and committed
original-result recovery after discarding the reply. This is actual database
evidence; network ACK loss or HTTP E2E is not claimed for this private increment.

Whole-task independent inventory confirmed another T44 requirement still pending:
C section 8 requires an access to persist selected-scope timeout compensation
before reading again. Existing captures reject unsettled due rows while the
bounded global worker performs persistence. The next independently scoped
increment will commit selected-scope settlement before a fresh protected typed
observation, retaining historical read-only ports and final business-owner CAS.
That implementation and separate whole-task acceptance remain outstanding.

Totals remain four implemented, twelve in progress and thirty-one incomplete or
deferred across 47 tasks. Product 0.10.9, normative 1.0, pinned V1 and CodeWhale
remain unchanged. T19/qualification composition, physical certification and
deployment remain separate. Downloads have no hard total/cumulative deadline.

## T44 access-driven convergence and whole-task acceptance (2026-10-16)

Additive migration 19 commits selected-scope timeout compensation before a fresh
protected typed observation. Session, Authorization, Transaction and Staged initial
ports release the first connection before opening a new serializable transaction,
including with MaxConns=1. Final business owners still recapture and CAS. Existing
historical event observations remain read-only; this does not claim T19 HTTP wiring.
All prior committed SQL sources remain unchanged.

Independent reviews found and the primary repaired current original-result source
compatibility, pending-phase final clock regression, typed fresh-capture SQL time
propagation and over-reservation before terminal writes. Current complete inventory
is bounded separately from future admission; real advance_scope writers and final
remaining-obligation capacity stay enforced. Same original MAX-1 terminal writes
can commit at MAX; exhausted revisions fail with whole-write rollback. Actual
4097/4096 member controls, selected scope behind a 1000-scope global prefix, all
three purposes, original short-expiry readback, full T06 competitors, active Validate
fencing and precise additive catalogue/ACL guards passed.

The primary complete lifecycle feature passed pure domain 44
top-level/127 child and actual PostgreSQL 230
top-level/312 child tests, zero skips/failures
(1206.87s combined wall). Default service passed
46 tested packages, 433 top-level/
830 child tests (70.524s wall). Its explicit
517 top-level/575 child skips and 12 no-test packages are separate
from the zero-skip native feature result. Modules, vet, private build, pinned V1,
Go formatting and working/cached whitespace passed. All 16 candidate paths, full
source/index/status and 65 prior SQL raw/index entries stayed frozen through all
nine gates. ROOT session 27674 closed exit zero with owned children joined.

Final staging found two spaces on otherwise empty lines in the new SQL source.
After the complete and both independent runs, the primary removed only those two
spaces; SQL executable tokens and the other 15 files stayed identical. The final
actual migration/catalogue tests passed again (31.587s wall), followed
by cached whitespace checks and both independent read-only final delta reviews.
Earlier frozen full-suite evidence therefore precedes this exact formatting-only
adjustment; it is not represented as another complete rerun.

Actual isolated PostgreSQL wire tests lose real COMMIT acknowledgements in both
settlement and fresh-read phases, independently confirm committed original facts,
then reconnect and recover the same identities without a second terminal write.
This is database integration evidence; product HTTP, installer and deployment E2E
are not claimed. Controlled SQL-clock tests are private fault controls, not public
fault-injection ports or physical certification.

Both independent auditors reviewed the complete frozen candidate line by line.
The dependency auditor independently ran actual PostgreSQL C19: 18 top-level and
26 child tests, zero skips/failures (113.861s package, 122.814s wall). Session 6706
closed exit zero, joined its children and returned the runtime slot with zero
owned processes. It independently checked ROOT's original complete-feature logs
and all nine gate results; that full run is ROOT evidence, not an auditor rerun.

The compatibility auditor separately ran actual PostgreSQL C19: 18 top-level
and 26 child tests, zero skips/failures (118.984s package, 125.851s test-command
wall). Session 42060 closed exit zero, joined its children and returned the slot
with zero owned processes. Its separate outer verification took 129.797s wall.
It also directly checked ROOT's complete-feature and nine-gate logs; these are
ROOT execution evidence, not an independent whole-feature rerun. Both auditors
passed the complete candidate and the final whole-task functional mapping.

Both final inventories distinguish the independently testable T44 kernel from
T17/T19/T20 business composition, physical certification and deployment. The full
16 candidate hashes, source/index/status and 65 prior SQL raw/index entries remain
unchanged within every actual run. The two-byte final SQL whitespace change is
listed separately above. C19 final scoped and whole-task review: PASS.

Both independent whole-task inventories mapped the T44 task and C sections 5-9,
M sections 2-5, K sections 3.2/4 and D sections 10-11 to actual implementation and
native evidence. Session initialization, unique validation, successful authorization,
consume/cancel/expire CAS, three-purpose execution, safe new-waiting revisions,
controlled invalidation/replacement, channel invalidation, original 24h/30d
boundaries, 61d status closure/62d cleanup, backup deadlines and startup/periodic
plus initial-access compensation have no remaining confirmed T44 feature gap.
The final independent scoped and whole-task conclusions are PASS.

T44 is implemented within its independently testable protected-kernel boundary.
T17/T19/T20 must use initial settlement/observation and their own final T06 guards;
selection, issuance, network ingress, client outbox and physical helper/slot/backup
deletion remain those tasks. This acceptance grants no execution qualification,
production HA acceptance or deployment authority.

The service increment is DCO committed and pushed as `8908d94205bf6939701cf358c18007b9590fa2bc` on
`codex/t44-access-convergence`. Current totals become five implemented, eleven in
progress and thirty-one incomplete or deferred across 47 tasks. T47 continues
next; product 0.10.9, normative 1.0, pinned V1 and CodeWhale remain unchanged.
Downloads have no hard total or cumulative elapsed deadline.

### T47 private SN sources and same-plan freeze

This bounded increment adds real private inclusion/exclusion groups and complete
same-plan T05 approval/T06 freeze bindings. Exact decoded UTF-8 SN is transient;
canonical durable groups contain purpose-isolated keyed digests, full scope,
plan/policy identity, role and exact retained provider material. Nil and explicit
empty input remain distinct. Edits require the exact old plan and new approval;
frozen source bodies cannot change. A generic approved change is not a group.

A dedicated restricted OpenBao Transit HMAC adapter pins an explicit version
and actual material commitment. The trusted registry keeps explicitly retained
versions; latest rotation, recreated names and missing/unsafe provider material
cannot substitute for an original reference or produce a successful negative
membership result. This does not provision production keys or settle destructive
retirement, cross-plan reuse, visibility or export policy.

Additive rollout_sn storage preserves all 66 prior SQL raw/index entries and
original rollout native executable functions. Actual complete source readback
joins the original receipt, Base, approval, result/outbox and T06 identity/epoch.
Its frozen history survives ordinary response blob expiry. Explicit provisioning
refuses structural/executable/permission drift before grants; private execution,
write/CREATE/PUBLIC and non-owner grant options remain closed. Unknown commit
reconciles the original result, and late clock/lease failure rolls back complete
plan, source binding, outbox and audit writes.

Both independent auditors reviewed all 24 production/test/documentation paths.
Primary repairs restored native upgrade-type guards, complete original source
index/Base binding and grant-option refusal. Stronger tests use individually
complete rollback positives, canonical single-field controls, actual server race
causes and authenticated in-memory decryption of actual processing projection
and committed result; ciphertext scanning is not treated as plaintext evidence.

ROOT earlier ran the entire old/new rollout feature: 62 top-level/55 child tests
passed, zero failures (190.002s wall); its two opt-in original Node references
were initially skipped. The final candidate then ran every changed SN/domain/
provider test plus both actual original references: 22 top-level/34 child tests,
zero skips/failures (54.873s wall). Default service passed 48 tested packages,
440 top-level/830 child tests (63.276s wall), with 530 top-level/580 child explicit
integration skips recorded separately. Modules, vet, private build, pinned V1,
Go formatting and working/cached whitespace checks passed. The complete source,
index/status and 66 original SQL raw/index entries remained frozen in every run.
One intermediate reference invocation used the wrong model entry point and
failed; correcting only private test configuration made the actual reference
run pass. No repository implementation was changed for that environment error.

After both initial independent actual runs passed, final primary self-review
identified a narrower historical provenance gap: comparing stored digest columns
alone did not authenticate current receipt actor/request bytes. Both auditors
agreed with the in-scope repair. New Go/native frozen-source readers recompute
the original T06 lookup/request digest and native canonical receipt bytes; they
do not change old SQL or reauthorize historical actors. Canonical actor-only and
operation-lookup controls reject and restore. The initial independent 22/32
runs precede this exact revision; final independent results below supersede them.

The dependency auditor independently reran the final provenance candidate:
22 top-level/34 child tests, zero skips/failures, 57.431s wall. Actual PostgreSQL
package took 48.885s; actual OpenBao and both original Node references passed.
Session 40478 closed exit zero, joined its children and returned owned processes
to zero. It also checked ROOT's nine final gate logs without claiming that full
default service execution as its own.

The compatibility auditor separately reran the same final frozen candidate:
22 top-level/34 child tests, zero skips/failures, 60.609s wall. Actual PostgreSQL
package took 47.142s; actual OpenBao and both original Node references passed.
Session 80651 closed exit zero, joined its children and returned owned processes
to zero. It independently checked ROOT's nine final gate logs, not a second
whole-default execution. Both auditors passed the complete final candidate.

The service increment is DCO committed and pushed as
`99393e031019b2794b01b6cd3243c940f5a26e58` on `codex/t47-sn-groups`.
The service developer boundary is recorded in `docs/rollout-sn-sources.md`.

This increments T47 without marking the whole task complete. Actual lifecycle
commands/admin workflow, T14/T15/T21 qualification and quality, T08 atomic
publication, physical platform/data qualification and measured budgets remain
pending. T42 deployment stays explicitly deferred. Totals remain 47 tasks:
five implemented, eleven in progress, thirty-one incomplete or deferred.
Product 0.10.9, normative 1.0, pinned V1 and CodeWhale are unchanged. Downloads
retain no hard total or cumulative duration limit.

### T47 frozen SN lifecycle storage

Actual same-plan T05-approved/T06-frozen SN plans now use original shared private
lifecycle storage. Additive rollout_sn_state installs two fixed private invokers
and one migration table; all 67 prior SQL raw/index entries and original native
executable owners stay unchanged. Original null-only ports still reject real SN
plans. Valid null-plan operation needs no optional SN namespace/provider/registry.

Draft and transition authenticate complete original source/approval/receipt/
outbox/T06 provenance before and after shared writes. Typed head/fact/window reads
repeat current schema, storage, ACL and source integrity checks in one transaction.
Effective search paths are checked before catalog/data access; shared tables must
be permanent ordinary tables without RLS/inheritance. PUBLIC, column writes,
private EXECUTE and non-owner grant options remain closed. Frozen reads take no
plan advisory lock before a later writer's scope lock. Historical authentication
does not reauthorize an old actor or grant execution/publication qualification.

Real private transitions retain exact percentages, adjacent stages, actor/abort
purposes, terminal revision reserve and permanent full-scope windows. Actual
exclude-only and explicit empty groups complete approval/freeze/lifecycle flows.
Same-base and shared-scope competitors each first complete their own write/readback
and roll back. Races preserve original PostgreSQL causes, admit exactly one
winner and verify exact durable head/fact/window and unchanged loser/source rows.
Source faults occur after real draft/fact insertion; late clock faults require
both candidate fact and updated head. Independent nontransactional markers prove
the final guards were reached; all writes roll back, with complete positives
before/after restoration. Real native history creates 4,101 facts and 2,051
windows, exceeds 4,096 rows and 1 MiB, and preserves bounded old/current/source
reads after ordinary encrypted response expiry.

Both auditors independently reviewed all 13 production/test/documentation paths.
Primary repairs closed current shared ACL/storage and typed-read/path gaps;
stronger controls distinguish actual post-write guards from early failures.
Initial test compilation and history-capacity controls were corrected before the
final frozen runs. No production authority was inferred from those controls.

ROOT ran the complete old/new rollout feature with actual PostgreSQL, OpenBao
and both original Node references: 78 top-level/82 child tests, zero skips or
failures (335.675s wall). Default service passed 48 tested packages,
443 top-level/830 child tests (61.024s wall), with 541 top-level/588 child
explicit integration skips recorded separately. Modules, vet, private server
build, pinned V1, Go formatting and working/cached whitespace checks passed.
The full source/index/status and prior 67 SQL raw/index entries stayed frozen
through every actual run. Browser/physical platform/deployment E2E is not claimed.

The dependency auditor separately reran 47 top-level/59 child tests, zero skips
or failures (250.509s wall). The compatibility auditor separately reran
47 top-level/59 child tests, zero skips or failures (249.933s wall).
Each actual run included PostgreSQL, OpenBao and both original Node references,
closed its process session and joined owned children. Each checked ROOT's gate
logs without counting the whole-default run as its own. Both final independent
scoped conclusions are PASS.

The service increment is DCO committed and pushed as
`b178b65e84be0f39c392a19ff4dd6355fb8c7238` on `codex/t47-sn-state`.
The service developer boundary is recorded in `docs/rollout-sn-state.md`.

T47 remains in progress. Next is a private operation-bound child write set and
recovery in the same original outer T06 transaction/result, without an independent
child Engine or commit. Fixed actual T14 parent/baseline, T15 approval/scheduling,
T21 quality/contribution/freeze and T08 publication composition remain required;
production business ports stay closed while these final guards are missing.
Administrative workflow, measured budgets and physical qualification remain
pending. T42 deployment remains explicitly deferred. Totals remain 47 tasks:
five implemented, eleven in progress, thirty-one incomplete or deferred.
Product 0.10.9, normative 1.0, pinned V1 and CodeWhale are unchanged. Downloads
retain no hard total or cumulative elapsed limit.

### T47 operation-bound child composition

A closed private kernel now binds original lifecycle intents to the actual outer
T06 request digest, command purpose, operation/result, epoch and lease. Original
reservation attribution and child receipts remain protected for recovery; the
same supplied final transaction atomically writes child lifecycle/facts/windows,
outbox and original outer result. No child Engine or independent completion is
introduced. Current operator commands run actual scoped T05 authorization,
two independent reviews and dedicated fresh MFA on every channel. Recovery uses
the protected original login metadata and actual current T05 owner, with no new
caller identity or minted authentication time.

The complete inventory measures actual source bodies and canonical references,
including both original/current windows and their first facts. Shared immutable
plans and SN sources are charged once; conflicting identities refuse. The same
operation-wide 32 MiB logical bound remains separate from installer size and
download duration. Fixed outer owners must merge their own actual source costs.
An opaque sequential write set retains its original transaction; copied/closed
handles cannot repeat or move execution. Only immutable plan mathematics is
cached. Native provenance/CAS/current authority/time/owner guards remain per
child, and full Seal rechecks actual catalog/source/head/fact/window/outbox and
the final authorization/owner fences after every outer write.

Actual controls cover coordinated wire/receipt tampering, current permission or
approval drift, wrong purpose/epoch/lease, post-write source/index/catalog drift,
late backward/null/unsafe clock and exact authorization expiry. Two competing
outer operations each first reach real T06 Complete SQL and roll back, then race
final writes against the same unchanged lifecycle: one complete durable winner,
precise read-set or actual unique-slot refusal, no partial loser result and
unchanged original frozen source. Identical original requests return Busy and
later replay the same result. Worker expiry negatives require the actual
recovered-identity Denied result; a restored original proof recovers through a
fresh worker without caller identity. Unknown-result testing returns Unavailable
after an actual SQL commit; it is not physical socket/HTTP ACK-loss E2E.

ROOT's complete four-package actual PostgreSQL/OpenBao/two-original-Node run
passed 103 top-level/128 child tests, zero skips/failures (668.583s wall).
Default service passed 48 tested packages, 454 top-level/840 child tests
(76.145s wall), with 556 top-level/601 child explicit opt-in skips and 12 no-test
packages separately recorded. Modules, full vet, private server build, pinned V1,
Go formatting and both whitespace gates passed: nine gates total.

The dependency auditor independently ran 73 top-level/105 child tests, zero
skips/failures (599.678s wall); the compatibility auditor separately ran the same
selection, 73 top-level/105 child tests, zero skips/failures (599.909s wall).
Each used actual PostgreSQL, OpenBao and both original Node references, verified
frozen full source/index/status and all 68 previous SQL raw/index identities,
closed its tool session and joined owned processes. Each checked ROOT's complete
gate logs without claiming the default service run as its own. Both final scoped
conclusions and final evidence-document reviews are PASS.

The actual 64-plan SN source-union capacity cases passed in
197.64s/199.03s/200.24s (ROOT/dependency/compatibility). A shared 100-stage plan
with 162 actual child drafts passed full Seal, after-Seal rollback and original
request recovery/commit in 52.71s/53.64s/55.59s. Earlier repeated plan-validation
cost exhausted existing control leases; same-transaction immutable mathematics
reuse repaired the cost without changing leases or removing authorization,
provenance, source, CAS, final readback or clock checks. Early failed runs are
not acceptance evidence. Only reviewed documentation clarifications followed
ROOT's frozen source run; no production/test code changed afterward.

The service increment is DCO committed and pushed as
`f1efe76aad1914597b59f4dbf33fe05c4603132b` on `codex/t47-operation-kernel`.
Its developer boundary and verification are in
`docs/rollout-operation-kernel-plan.md` in the service repository.

T47 remains in progress. Production business routes stay unavailable until fixed
actual T14 parent/baseline, T15 approval/scheduling, T21 contribution/quality/freeze
and T08 publication guards/writes are composed. The test-only fixed outer composer
does not qualify those missing owners. Admin workflow, measured platform/data
budgets and physical qualification remain pending; T42 deployment remains
explicitly deferred. Totals remain 47 tasks: five implemented, eleven in progress,
thirty-one incomplete or deferred. Product 0.10.9, normative 1.0, pinned V1 and
CodeWhale stay unchanged. Downloads have no hard total/cumulative duration limit.

### T21 explicit frozen-SN quality source integration

The private quality owner and runtime now explicitly opt into the original T47
frozen-SN source/lifecycle adapters. Existing null-only setup remains independent
of optional SN schemas and providers. Owner preflight checks existing quality
role rights without rerunning grant normalization; drift refuses without healing.
Readiness and actual reads retain current protected catalog, connection, owner,
scope and original T05/T06 approval binding checks. Partial staged provisioning
is not readiness. No runtime writer, private EXECUTE or production route is added.

New sample and complete receipt/head writes recheck their actual original source
after writing and before the existing final SQL clock. Original two-clock fences,
CAS, replay, sticky outcomes and permanent history are unchanged. Actual
include/exclude, empty and exclude-only plans preserve the first stage/window
through pause/resume/advance. Faults change actual retained binding metadata only
after the complete candidate write set is observable; the actual subsequent bad
binding read refuses with no object and one clock. Rollback restores every row;
identical original inputs pass before rollback and after a real two-clock commit.
Exact Invalid/42501 runtime refusal and a single-connection unsafe session path
control have restored positive checks. All 69 existing SQL files remain unchanged.

ROOT's complete quality run enabled actual PostgreSQL and both original Node
models: 23 top-level/11 child tests, zero skips/failures (150.263s wall).
Related actual SN source/lifecycle/OpenBao regression passed 7 top-level/18 child
tests with zero skips/failures (42.098s wall). Default service passed 48 tested
packages, 455 top-level/840 child tests (67.241s wall); 560 top-level/606 child
explicit external opt-in skips and 12 no-test packages are separately recorded.
All nine service gates passed, including modules, vet, private build, pinned V1,
formatting and both whitespace checks.

Both independent roles gave final in-scope code/test/documentation PASS. Their
separate actual quality/Node runs passed 23 top-level/11 child tests each, zero
skips/failures (162.383s/156.692s wall, dependency/compatibility). Each verified
full source/index/status and all 69 previous SQL raw/index identities before and
after, closed its session and joined owned processes. They checked ROOT's full
gate evidence without claiming those gates as their own execution.

This service increment is DCO committed and pushed as `1f854602624f7c6f50b289ab288ced1452be1bd9` on
`codex/t21-frozen-sn-samples`. Its boundary and evidence are in the service's
`docs/quality-kernel-plan.md`, fourth batch.

The isolated MAC and fixed publisher fixtures certify storage integration only.
Actual T20 accepted bytes and processing cursors, T25 purpose-bound eligibility
and limits, complete groups/aggregates, freeze reasons and fixed T15/T14/T08
business composition remain pending. No device membership, qualified sample or
production assessment is inferred. T21 remains in progress; totals remain five
implemented, eleven in progress and thirty-one incomplete/deferred out of 47.
T42 deployment stays deferred; physical platform/data certification and measured
budgets remain pending. Product 0.10.9, normative 1.0, pinned V1 and CodeWhale stay
unchanged. Downloads have no hard total/cumulative duration or retry ceiling.

### T12 stable installed-facts identity and exact profile matching

The service now computes the original sourceProfileId from the complete nine
stable fields using the actual pinned Schema and exact JCS/SHA-256. Closed facts
parsing and richer-request projection are separate: top-level runtime/self/policy
fields never enter stable identity, nested unknown fields still refuse, and the
actual request owner must separately authenticate the complete DTO and bind its
full digest. No version normalization or target/format certification is inferred.

Every internal candidate must have complete facts, its recomputed actual ID and
the exact product/component/target scope. Matching compares full canonical facts,
never version or array order. Zero/duplicate matches return the original unknown
or ambiguous reasons with no partial identity. Invalid nonmatching entries refuse
even after earlier matches. Returned snapshots have no mutable input/result alias.
An independent 1 MiB raw payload preflight prevents whitespace parsing amplification;
the full canonical array, framing and all facts have a separate 1 MiB limit, with
4,096 entries as an upper bound. These are internal metadata capacities.

Full original-field and single-variable malformed-JSON controls, every leaf's
match/scope refusal, empty/duplicate/order cases and exact/excess parser and raw/
canonical inventory byte controls pass. Original Node sourceProfileId actually
executes 47 positive/refusal cases and compares both canonical bytes and IDs.
The final primary contracts/registry run passed 13 top-level/51 child tests,
zero skips/failures (9.205s wall). Default service passed 49 tested packages,
460 top-level/891 child tests (63.978s wall), separately recording 561 top-level/
606 child external opt-in skips and 12 no-test packages. All nine service gates
passed, including full vet, modules, build, pinned contracts and whitespace.

Both independent code/test/documentation reviews gave final scoped PASS. Their
own separate contracts/registry/original-Node runs passed 13 top-level/51 child
tests each with zero skips/failures (13.281s/9.548s wall, dependency/compatibility).
Each checked full source/index/status and all 69 existing SQL raw/index identities,
closed its session and joined owned processes. Unchanged database/provider/browser
integrations were not rerun for this domain-only delta.

This service increment is DCO committed and pushed as `7ef4c0983a9ad580bc1f902bdc393cdb8e77b30f` on
`codex/t12-stable-source-facts`; its boundary is in `docs/registry-facts.md` in
the service repository. The actual protected Registry/version/state/approval,
ForwardPathPolicy/transforms, T46 evidence, helper cleanup and all-channel T08
publication remain pending. A match is neither host evidence nor production
qualification. T12 is now in progress, not completed. Totals are 47 tasks:
five implemented, twelve in progress and thirty incomplete/deferred. T42 deployment
and physical platform/data/budget certification remain deferred or pending.
Product 0.10.9, normative 1.0, pinned V1 and CodeWhale are unchanged; downloads
have no hard total/cumulative duration or retry cap.

### T12 immutable forward-policy content and exact selected-hop intersection

The service now validates the original complete policy/edge Schemas and exact
original objectHash(edges), preserving edge/channel order and duplicate edges.
Every edge is checked, including unused sources. The owned parsed policy is a
content calculation, not a protected current Registry record or approval.

The matcher rechecks the actual stable-facts identity and policy bytes, then
intersects an already selected hop with source ID, all seven hop dimensions and
current channel. A target must be strictly higher than the source using exact
arbitrary-length decimal components. Zero matches returns the original unavailable
reason; duplicates or overlapping channel lists refuse as ambiguity. Neither
failure exposes a partial edge. Unique results own exact canonical edge bytes
and hashes. Raw/canonical 1 MiB boundaries are independently tested.

The actual original Node executes 29 policy/selected-hop comparisons and all
47 existing facts comparisons. Non-higher target refusal is independently tested
as a Registry invariant, without claiming whole-selector parity for its earlier
no_higher_version exit. The final primary contracts/registry run passed 19
top-level/81 child tests with zero skips/failures (11.957s wall). Default service
passed 49 tested packages and 465 top-level/921 child tests (85.954s wall), with
562 top-level/606 child external opt-in skips and 12 no-test packages separately
recorded. All nine service gates passed, including modules, full vet, build,
pinned V1, formatting and both whitespace checks.

Both independent code/test/documentation reviews gave final scoped PASS. Their
separate contracts/registry/original-Node runs each passed 19 top-level/81 child
tests, zero skips/failures (15.457s/14.144s wall, dependency/compatibility).
Both froze source/index/status and all 69 existing SQL raw/index identities,
closed sessions and joined their owned processes. Unchanged database/provider/
browser integrations were not rerun for this calculation-only delta.

The increment is DCO committed and pushed as `fe2ed92c74ab06d14f76c54fa29998f203c6604c` on
`codex/t12-forward-policy`; service boundary/evidence is in
`docs/registry-forward-policy.md`. Actual Registry registration must still bind
all referenced sources, higher targets, transform/capability evidence, immutable
ID/revision, current approval and publication. Full T12 and complete T17 path
selection remain in progress; this result grants no host or business authority.
No SQL, grants, route, provider or UI changes are introduced. Totals remain 47:
five implemented, twelve in progress and thirty incomplete/deferred. Product
0.10.9, normative 1.0, pinned V1 and CodeWhale are unchanged. Downloads have no
hard total/cumulative duration or retry cap.

### T17 bounded initial upgrade selection calculation

The service now computes the original initial baseline/candidate and highest
reachable next hop from the complete closed pinned selection projection and
actual parsed stable facts. Extra source/current-version/hash consistency checks
are explicitly separate from original model parity. Calculation flags, buckets
and an owned facts identity do not authenticate current sources or a host.

Original floor/profile/state/endpoint/path/policy precedence is preserved. SN
absence, exclusion, inclusion and bucket order match the frozen model; bucket
upper bounds are consulted only in the actual bucket branch. Exact eligibility,
ordinary/bridge rules, forced full-window coverage and activation-mode equality
feed an iterative descending equal-version-group search. Equal versions cannot
enable one another. Forward-policy hash/intersection is checked only for the
unique winning hop, with no lower-hop fallback. Refusals return no partial IDs.

A shared checked decimal-version primitive preserves exact arbitrary-length
three-component order without conversion or normalization; the accepted Registry
comparison now uses it with unchanged API. Tests include complete malformed
JSON, source consistency, independent raw/canonical 1 MiB exact/excess controls,
oversized components and a 1100-node path within input bytes. Actual original
Node runs generated selection comparisons and all 19 public path plus 7 public
bucket vectors, checking their fixed expectations first. Both forced visibility
window boundaries have otherwise-valid positive/negative controls and original
Node comparisons. Existing 47 facts and 29 policy comparisons also regress.

The final primary affected run passed 4 packages,
27 top-level/256 child tests with zero skips/failures
(17.862s wall). Default service passed 51 tested packages,
472 top-level/996 child tests (64.506s wall), with
563 top-level/606 child external opt-in skips and 12 no-test packages
separately recorded. All nine service gates passed: affected/default tests,
modules, full vet, build, pinned V1, formatting and both whitespace checks.

Both independent code/test/documentation reviews gave final scoped PASS after
the primary repaired the independent forced-visibility test gap. Their own
separate affected/original-Node runs each passed 27 top-level/256 child
tests, zero skips/failures (11.234s/20.179s wall, dependency/compatibility).
Source/index/status and all 69 existing SQL raw/index identities remained fixed;
all sessions and owned children closed. Unchanged PG/Bao/browser integrations
were not rerun for this calculation delta; no real qualification is claimed.

This increment is DCO committed and pushed as `73498aef89795bac6790fced3fdff4580e00873d` on
`codex/t17-selection-calculation`; service boundaries/evidence are in
`docs/initial-selection-calculation.md`. Current Registry/approvals, T13 paths,
T46 capability, T47 membership/private bucket, protected chain projection and
atomic final rechecks remain with the future business composition. No route,
SQL, Session writer, signer, provider or UI is added. T17 remains in progress;
totals stay five implemented, twelve in progress and thirty incomplete/deferred
out of 47. Product 0.10.9, normative 1.0, pinned V1 and CodeWhale are unchanged.
Downloads have no hard total/cumulative duration or retry cap.

### T20 restricted task observation calculation

The service now plans both reconciliation-event and recovery-review-event
observations beside existing Session/transaction planners. The complete original
task binding, full scope, role/purpose/install/start and target bind before exact
receipt replay. The task binding revision remains independent from its mutable
observation record revision. Internal DTOs and signed fixtures are not protected
T22/T23 task sources or actual admission authority.

Fresh live pending/observing tasks can only advance observing state and sequence.
At exact windowEnd or a server terminal, only the historical diagnostic cursor
advances; current task state and execution rights remain unchanged. Complete
shared anchors, including zero, are preserved with owned copies. Existing exact
receipts replay before fresh facts/cursor/increment checks, including maximum
current revisions/sequences; changed immutable bindings still refuse first.
Clients cannot complete/fail/cancel tasks or submit an execution transition.

Both actual original public signed task-role fixtures pass the real verifier.
Independent binding/scope negatives still fit the complete original Schema;
fresh facts/window/target, sequence/overflow, terminal/history, replay/conflict,
cursor and ownership controls pass. Strict snapshot validity is separately
labelled. The actual frozen signed planEvent executes 76 task/cursor/receipt
comparisons with in-memory test keys. Its contribution/outbox/outer atomic writes
are outside the compared projection and are not implemented by this batch.

The final primary contracts/complete-events run passed 57 top-level/
846 child tests, zero skips/failures (141.898s wall), executing all
existing original event reference groups as well as the 76 new comparisons.
Default service passed 51 tested packages, 478 top-level/
1120 child tests (97.624s wall), separately recording
564 top-level/606 child external opt-in skips and 12 no-test packages.
All nine service gates passed, including modules, full vet, build, pins,
formatting and both whitespace checks. Both independent code/test/documentation
reviews gave final scoped PASS. Their own contracts/complete-events/original-Node
runs each passed 57 top-level/846 child tests, zero skips/failures
(142.643s/136.425s wall, dependency/compatibility). Source/index/status and all
69 existing SQL raw/index identities stayed fixed; all sessions/children closed.
Unchanged PG/Bao/browser integrations were not rerun for this calculation delta.

The increment is DCO committed and pushed as `6c0a5503f38fad3b6963f15e9ae37b1632934f94` on
`codex/t20-task-observation`; service boundary/evidence is in
`docs/task-observation-calculation.md`. Actual T22/T23 lineage, current trust/deny
and final atomic receipt/task/cursor/outbox/contribution composition remain
pending. No SQL, grants, public task HTTP roles, production writer, client queue
or qualification are added. T20 remains in progress and totals stay 5/12/30
out of 47. Product 0.10.9, normative 1.0, pinned V1 and CodeWhale are unchanged.
Downloads have no hard total/cumulative duration or retry cap.

### T25 reviewed network privacy modules on the combined service line

The current service development line now includes the accepted fixed network
weekly calculator and protected in-memory IPv4/IPv6 prefix normalization. All
seven code/test/source-document files are exact original Git blobs from
`0ba848f77042339e6f5afe3073e817e9d416ab13` and
`f2af566bde47bb1547c8aec8c0e81bb9cb5e6aed`; unrelated branch history and another
worktree's mutable files were not imported. Only an additional integration
evidence document is new. Original historical source-batch checks remain separate.

Network leaf versions retain their own two-component semantics. Full-leaf scope
and prefix/day quotas, shared cross-leaf prefix/week cap seven, k20 suppression,
identity duplicate/conflict and owned output rules remain unchanged. Prefix
normalization retains /24,/48,mapped unwrapping/zone refusal and direct/nested
formatting/encoding protection with an explicit owned in-memory KMS-input copy.
The network result does not qualify stage/release-quality contribution. Unresolved
same-time/eventId distinct identity ties still refuse the whole calculation; the
OpenBao probe's unproven week isolation is not enabled as an adapter or changed
key-custody policy. No owner decision is inferred by this integration.

Both independent code/test/documentation reviews gave scoped PASS and separately
checked all seven source blobs/raw digests. The primary final privacy/edge run
passed 10 top-level tests, zero child tests/skips/failures (15.419s wall).
Default service passed 53 tested packages, 488 top-level/
1120 child tests (73.934s wall), with 564 top-level/
606 child external opt-in skips and 12 no-test packages separately recorded.
All nine service gates passed: affected/default tests, modules, full vet, build,
pinned V1, formatting and both whitespace checks. The independent reviewers'
own separate privacy/edge runs each passed 10 top-level tests, zero child tests/
skips/failures (5.522s/6.277s wall, dependency/compatibility). Full source/index/
status and all 69 existing SQL raw/index identities stayed fixed; all sessions/
owned children closed. No Node/PG/Bao/browser or physical E2E is claimed here.

This combined-tree integration is DCO committed and pushed as `e374493d28a22d59270947c2a146afa0bab26bba` on
`codex/t25-network-integration`; its boundary/evidence is in
`docs/network-privacy-integration.md`. Actual KMS/ASN/event-source composition,
purpose-specific quality policy, permanent frozen snapshots, restricted reports/
exports, retention cleanup and three-language privacy UI remain pending. No SQL,
grants, dependency, artifact, signing, Root or public route changes are introduced.
T25 stays in progress and totals remain 5/12/30 out of47. Product0.10.9,
normative1.0, pinned V1 and CodeWhale are unchanged. Downloads have no hard total/
cumulative duration or retry cap; 32MiB is not an artifact/download size limit.

### T05 actual Keycloak acceptance on the combined service line

Accepted source `f123be0b49f7a5aa9fd3ef638e1bbef04659c0b1` is integrated on
the combined service line: the Go acceptance test remains its exact original
blob, and the two Python scripts have a narrow post-import temporary-lifetime
repair. The original management document delta is exactly 116 additive lines;
all current content and historical evidence are preserved. Source provenance
and repaired-file digests were verified separately.

The first actual authentication case passed, but its surrounding ownership gate
failed because Java left three empty scratch directories outside the run root.
That attempt is not a complete acceptance pass. The repair binds Java native
TEMP/TMP/TMPDIR and java.io.tmpdir to the owned root on initial and code10 restart.
The fifth local regression exercises both failure paths, checks every child wait,
and verifies scratch material and the isolated parent are empty after cleanup.
The initial private empty directories are retained; the revised runs use a fresh
isolated parent and do not clean or depend on shared temporary directories.

Fresh root verification passed all 12 gates: verified official runtime cache,
five Python control groups, default OIDC, actual provider, full default service,
modules, full vet, private server build, pinned V1, formatting and both whitespace
checks. Default OIDC passed two tests and separately skipped the opt-in provider
case; the actual run enabled that one case with zero skips/failures
(90.175s full harness wall, 1.46s Go case).
Full default service passed 53 tested packages,
488 top-level/1120 child tests
(67.041s wall), with 565 top-level/
606 child external opt-in skips and 12 no-test packages
recorded separately. No opt-in skip is represented as actual provider coverage.

Both independent roles gave scoped code/test review PASS and separately reran
all six relevant gates: runtime provenance, five Python groups, default OIDC,
actual provider and both whitespace checks. Their actual provider runs each
passed the one real case with zero skips/failures
(80.885s/73.568s full harness wall,
1.21s/1.33s Go case;
dependency/compatibility). Full source/index/status and all 69 existing SQL
raw/index identities stayed fixed during every run. Each run closed its owned
children and left its new temporary parent empty. Historical source-batch figures
are separate from these combined-tree measurements.

The isolated official Keycloak26.8.1/Temurin21.0.12.1+1 provider exercises actual
password/OTP forms, signed adapter validation, single-use code and provider logout;
the five-minute boundary is calculated from signed auth_time, not five elapsed
minutes. Verified cache provenance covers both archive SHA256 digests, all486
Keycloak library files and all490 Java distribution files. No credentials,
provider logs or production realm are retained in the repository. Production
authentication/permissions/session/SQL/dependencies/routes/UI are unchanged.
Deployment realm/proxy/enrollment/HA acceptance remains pending. T05's previously
implemented scope and totals5/12/30 out of47 stay unchanged. Product0.10.9,
normative1.0, pinnedV1 and CodeWhale are unchanged. Downloads have no hard total/
cumulative duration or retry cap; 32MiB is not an artifact/download size limit.

DCO committed and pushed as `d16cd77e0558ba603201820a5a5c75306a84f192` on
`codex/t05-keycloak-integration`; see service
`docs/keycloak-acceptance-integration.md` for boundaries and evidence.

### T09 owned byte capture on the combined service line

Accepted source `191c9e8148f90032f878e1d337a544f83a8c007f` is integrated on
the current service line as seven exact Git blobs. The existing command/verifier
files and source document matched its parent, and only that source document's
70 original additive lines were applied. Historical source-batch acceptance
remains distinct from fresh combined-tree evidence.

An owned process-memory Snapshot confirms exact source length, complete EOF and
observed SHA256 before returning private bytes through independent ReaderAt.
Neither source/read-buffer mutation nor concurrent reads can modify it. The
transport owner retains blocking-read cancellation and lifetime; no detached
reader, elapsed/retry budget or implicit production capacity is introduced.
The isolated CLI shares the capture while retaining its flags/errors/output;
InspectSnapshot reuses current Root/deny/Manifest checks on every inspection
and confirms the complete observed upload identity. Old Inspect remains compatible.

Both independent scoped code/test reviews passed with all seven source blobs
and raw digests independently verified. The primary frozen affected run passed
four packages, 28 top-level/75 child tests, zero skips/failures
(18.099s wall), with an explicitly rebuilt actual CLI case enabled.
Coverage includes a real33MiB-plus source, four concurrent hash readers, malformed
Reader/EOF/cancellation controls, actual streamed HTTP body cancellation,
signed Manifest/current-deny recheck and actual CLI subprocess output/error checks.
This does not qualify native identity, protected storage or production HTTP upload.

The first broader test invocation passed all actual Go tests but its outer helper
rejected two unchanged sourcefile fixture skips. The helper's affected package
selection was corrected to the agreed capture/archive/verification/CLI scope;
no product or test source was changed. The final required affected run has zero
skips. The full default service continues to include sourcefile: actual symlink
creation permission and an authorized mapped remote source are unavailable, so
those two host checks remain explicitly unverified among the default skips.
Neither is represented as a qualification pass.

All ten primary gates passed: private CLI build, affected tests with actual CLI,
full default service, modules, full vet, private server build, pinnedV1, formatting
and both whitespace checks. Default service passed 54 tested packages,
494 top-level/1128 child tests
(66.664s wall), with 565 top-level/
606 child opt-in/unavailable-host skips and12 no-test packages
separately recorded. Each reviewer separately rebuilt its own CLI and passed
all affected tests (28/75, zero skips/failures, actual CLI enabled), affected vet,
formatting and both whitespace checks: six gates each
(15.655s/12.666s affected-test wall,
dependency/compatibility). They did not repeat the primary full default gate.

Full source/index/status and all69 existing SQL raw/index identities stayed fixed
during every run; sessions and owned children closed. All seven imports remain
exact source blobs. No Node/PG/Bao, production upload/admission or physical E2E
is claimed. Persistent upload, quarantine, complete supply-chain/native inspection
and physical platform qualification remain pending. No SQL, route, dependency,
permission, provider, Root, contract, Session or UI changed. T09 stays in progress;
totals remain5/12/30 out of47. Product0.10.9, normative1.0, pinnedV1 and CodeWhale
remain unchanged; deployment stays deferred. Downloads have no hard total/
cumulative duration or retry cap;32MiB is not an artifact/download size limit.

DCO committed and pushed as `b859fb0e685cb9958a3ccf4b052f2eee9a881fd7` on
`codex/t09-capture-integration`; service boundaries and fresh evidence are in
`docs/artifact-capture-integration.md`.

### T09 Manifest-bound independent child snapshots

Accepted consecutive sources `334eaec82a4460e5190c0c39b8bb85126d14b64f` and
`5208ab5733fa5697a45e96e9d785787a309c620b` are integrated as nine exact final
Git blobs. Five existing paths matched the first parent at both raw/Git levels;
four new paths were absent. The source document retains its text and the original
124 additive lines. Historical source-batch evidence is separate from these fresh
combined-tree checks; no divergent branch history or later policy was imported.

FullPack/SBOM/provenance extraction returns independent owned process-memory
bytes only after the entire same immutable upload is inspected. Root ZIP capture
shares the nested buffer and retains its live-memory charge until the outer scan
ends. OuterFile accepts exact safe relative names in the outer ZIP, including
directory prefixes, without nested traversal or host path selection; FullPack
alone uses the bare root ZIP. Child source/memory caps, complete EOF/hash and
parent/member identity are checked. Current Manifest/Root/deny are rechecked by
InspectFullPack/InspectEvidence, and the actual child identities must match their
signed claims. A failed second evidence file returns neither the first file nor
earlier Facts. Old inspection APIs remain compatible and no authority is cached.

Both independent code/test reviews passed after verifying all nine source blobs
and raw digests. The primary frozen affected suite passed five packages,
38 top-level/79 child tests, zero skips/failures (21.931s wall),
including actual Store/Deflate ZIP separation with a 33MiB-plus expanded native
payload, retained-buffer
capacity and restored positives, outer/nested decoys, later corruption, exact
directory-prefixed evidence, second-file failure, same-size hash changes, wrong
target/current-deny refusal and signed-byte recovery. Original HTTP cancellation
and explicitly rebuilt actual CLI subprocess controls also passed. Synthetic
signed-byte payloads do not certify SBOM/SLSA content or native authenticity.

All ten primary gates passed: rebuilt private CLI, affected tests with actual CLI,
full default service, modules, full vet, private server build, pinnedV1, formatting
and both whitespace checks. Default service passed 55 tested packages,
504 top-level/1132 child tests
(70.93s wall), with 565 top-level/
606 child opt-in/unavailable-host skips and12 no-test packages
separately recorded. The unchanged sourcefile symlink/mapped-source fixtures
remain unverified on this host and are not represented as qualification passes.
Each independent role separately rebuilt its own CLI and passed all38/79 affected
tests, zero skips/failures, actual CLI enabled, affected vet, formatting and both
whitespace checks: six gates each (16.625s/19.568s
affected-test wall, dependency/compatibility). They did not repeat whole-default.

Full source/index/status and all69 existing SQL raw/index identities stayed fixed
during every run; sessions and owned children closed. All nine imports remain
exact accepted source blobs. No Node/PG/Bao, durable upload/quarantine/Artifact
admission, supply-chain approval, native authentication or physical E2E is claimed.
No SQL, route, dependency, permission, provider, Root, contract, Session or UI
changed. Those full T09 workflows remain pending, so T09 stays in progress and
totals remain5/12/30 out of47. Product0.10.9, normative1.0, pinnedV1 and CodeWhale
are unchanged; deployment remains deferred. Downloads have no hard total/
cumulative duration or retry cap;32MiB is not an artifact/download size limit.

DCO committed and pushed as `75deeb65729e251963fed8e4a26b24ea73022a7e` on
`codex/t09-owned-children`; service scope/evidence is in
`docs/artifact-owned-children-integration.md`.

### T09 offline SBOM structure and signed installer digest

The accepted consecutive range `eb440ad772f6cf448976edade90ce402d24fb9d3`
through `820df61edcd7d5d10fc3cb8a81be6aed603b1e3b` is integrated as18 exact
final Git blobs. Six existing paths matched the first parent at raw/Git levels;
12 new paths were absent. The existing PostgreSQL artifact trust test is retained
with accepted additions. Initial preflight caught its mistaken new-file count
before any repository write; the corrected6/12 inventory was independently
reviewed. The original214 additive source-document lines remain historical
source-batch evidence, separate from these fresh combined-tree checks.

All four embedded official resources/license match fixed upstream revision
`55343ba19dee1785acf1ce9191540d5fd7b590db`, fresh public GitHub tree/blob
identities, SHA256, accepted bytes and runtime-pinned digests. Runtime is offline
with a closed loader and explicit1.6 support. ParseBounded adds finite caller
capacity for non-envelope documents; metadata/JCS1MiB and other frozen grammar
limits stay unchanged. InspectSBOMSchema uses current signed paired evidence and
the declared version; primary binding additionally requires at least one main
component SHA-256 and every such declaration matching signed finalInstaller.
Dependency components cannot substitute. Equal repeated declarations remain
supported by code/Schema; no dedicated identical-repeat fixture is claimed here.
Hex case preserves raw identity. Failure exposes no earlier Facts or evidence.

Both scoped independent code/test reviews passed with all18 source blobs and
four official resources verified. The primary frozen affected run passed eight
packages,53 top-level/79 child tests, zero skips/failures
(28.075s wall), including both freshly rebuilt actual CLI tests,
strict original envelope-cap controls, offline references/resource drift,
version/schema refusals with earlier byte-stage positives, primary hash case/
missing/dependency-decoy/conflict controls with Schema positives and current-deny
rechecks. These are structure and declared byte relationships, not content trust.

Actual isolated PostgreSQL17.11 passed one top-level/four child tests, zero
skips/failures (47.644s wall), covering published Root/current deny,
same signed upload/Schema/primary binding and adversarial storage refusal with
restored positives. Before/after fixture database and role inventories matched:
0 pre-existing fixture databases and
1 pre-existing fixture role were retained. No
new fixture database or role remained. Owner storage controls do not represent
approved business deny commands; no production database/realm was used.

Recorded PG intervals run from the pre-inventory check through Go completion;
the verified post-inventory and frozen-source checks are outside those intervals.

All12 root gates passed: two CLI builds, affected tests with both actual CLIs,
actual PG, full default service, modules, full vet, private server build, pinnedV1,
formatting and both whitespace checks. Default service passed
57 tested packages,516 top-level/
1132 child tests (93.425s wall), with
566 top-level/606 child opt-in/unavailable-host
skips and12 no-test packages recorded separately. Unavailable sourcefile host
fixtures remain unverified. Each reviewer separately rebuilt both CLIs and
passed all53/79 affected tests and actual PG1/4, zero skips/failures, plus affected
vet, formatting and both whitespace checks: eight gates each. Their affected
walls were 24.784s/21.597s; actual PG walls were
19.919s/20.077s (dependency/compatibility).
Each PG inventory was restored to the same pre-existing baseline. Independent
runs did not repeat the root whole-default gate. All sessions/owned children
closed; full source/index/status and all69 old SQL raw/index identities stayed
fixed during every run. All18 imports remain exact accepted source blobs.

No Node/Bao, production upload/quarantine/admission, SBOM completeness/license/
vulnerability/SLSA/native approval or physical/deployment qualification is claimed.
No production SQL, grant, route, dependency version, provider, Root, Session, UI,
contract or product/normative version changed. Those full T09 workflows remain
pending; T09 stays in progress and totals remain5/12/30 out of47. Product0.10.9,
normative1.0, pinnedV1 and CodeWhale are unchanged; deployment stays deferred.
Downloads have no hard total/cumulative duration or retry cap;32MiB is not an
artifact/download-size limit.

DCO committed and pushed as `f607656cf837b23806e1e19aa722dcf26ce758b2` on
`codex/t09-sbom-integration`; service scope/evidence is in
`docs/artifact-sbom-integration.md`.

### T09 bounded provenance object syntax

Consecutive accepted sources `b9e69b7c8c14b05ff34dcd0b8ce75fd4ac2fc548`
and `ee94807fbea900c6df01aff83e78336e779e8af6` are integrated as eight exact
final raw/Git blobs. Two existing document/PG-test paths matched the first parent;
six new paths were absent. The original103 additive document lines and accepted
PG52 additions/seven deletions remain source-batch history. Fresh combined-tree
evidence is recorded separately. Prior SBOM/current-authority cases are retained.

Owned Snapshot inspection independently bounds whole bytes, raw line bytes
including LF/CRLF and object records. Bounded fragments fill one capped buffer;
strict ParseBounded preserves original grammar rules. A final unterminated object
is supported; blank/BOM/non-object/duplicate/later-malformed/capacity/cancel
failures expose zero Report. Composition first repeats current Manifest/Root/deny,
paired evidence and primary SBOM/installer binding, then checks JSONL syntax and
the same provenance identity; failure returns no earlier Facts, evidence or count.
The explicit-capacity standalone CLI reports only record count/raw identity and
uses fixed diagnostics. Old APIs and offline SBOM resources remain unchanged.

Both independent line-level code/test reviews passed. Primary affected testing
passed ten packages,61 top-level/79 child tests, zero skips/failures
(23.711s wall), including three freshly built actual CLI cases,
real HTTP capture and malformed/capacity/identity/current-deny controls with
earlier-stage positives and restored valid observations. Actual isolated
PostgreSQL17.11 passed one top-level/four child tests, zero skips/failures
(22.439s recorded interval), with valid two-object provenance,
duplicate-key refusal after signed size/hash and SBOM checks, restored positive,
and existing published Root/current-deny cases. Before/after fixture inventory
matched: 0 pre-existing fixture databases and
1 pre-existing fixture role were retained; no
new fixture database/role remained. These private adversarial storage controls
do not represent production data or approved business deny commands.

All13 root gates passed: three actual CLI builds, affected tests, actual PG,
full default service, modules, full vet, server build, pinnedV1, formatting and
two whitespace checks. Default service passed59 tested
packages,523 top-level/1132 child tests
(61.059s wall); 567 top-level/
606 child opt-in/unavailable-host skips and12 no-test
packages were recorded separately. Sourcefile symlink/mapped-source host
fixtures remain unverified. Each reviewer independently rebuilt all three CLIs,
passed61/79 affected tests and actual PG1/4 with zero skips/failures, and passed
affected vet, formatting and both whitespace checks: nine gates each.
Dependency/compatibility affected walls were25.075s/
24.688s; PG recorded intervals were21.108s/
26.056s. PG intervals include pre-inventory and Go completion;
post-inventory and frozen-source checks are outside those intervals. Other
command walls exclude frozen-source checks. Reviewers did not repeat the root
whole-default suite. Every PG inventory matched its pre-existing baseline.
All sessions/owned children closed; full source/index/status and all69 old SQL
raw/index identities stayed fixed during every run. Eight imports remain exact.

Object syntax does not interpret/approve declared provenance schemaVersion,
in-toto/DSSE/SLSA profiles, signatures, subjects, source revisions or pipeline
identity. Synthetic objects are not qualified provenance and supply no trusted
statement or Artifact admission. Unresolved profile/signature policy stays
closed. No production HTTP upload/quarantine/admission, native/physical or
deployment qualification is claimed. No SQL, grant, route, dependency, provider,
Root, Session, UI, contract or version changed. Full T09 remains in progress;
totals stay5/12/30 out of47, product0.10.9/normative1.0/pinnedV1/CodeWhale unchanged,
deployment deferred. Downloads have no hard total/cumulative duration or retry
cap;32MiB is not an artifact/download-size limit.

DCO committed and pushed as `175ff0c7b80dcb43ffe4acdb26699cbfba4f4ca8` on
`codex/t09-provenance-integration`; scope/evidence is in
`docs/artifact-provenance-integration.md`.

### T09 owned native byte candidates and Debian framing

Four accepted consecutive sources from
`eb93f9738c57a4b7209ce066fdce88dd6a6bebe6` through
`2defedfbfe3591a61da9596094dd29a46dbcdc25` are integrated as14 exact final
raw/Git blobs. Six existing paths matched the first parent; eight new paths were
absent. Original219 additive document and134 additive PG-test lines remain
historical source-batch evidence, separate from fresh combined-tree checks.

Closed signed role/helper-ID selection follows current Manifest/Root/deny and
complete owned FullPack inspection. Exact size/SHA direct membership yields
the signed five-field declaration and owned native bytes. Safe direct ZIP and
ordinary members share complete scanning/retained live nested-memory accounting;
old selectors stay compatible. ZIP-only/equal aliases preserve raw identity;
nested-only matches cannot supply direct candidates. Short-name SysV/GNU AR
inspection checks all physical headers/fields/spans/padding and member hashes
with bounded reads. Debian framing observes major2, control/data order, supported
labels, extensions and exact raw spans; signed composition requires format deb
and the same owned byte identity. Failures return zero earlier results.

Both independent scoped line-level reviews passed. Primary affected testing
passed12 packages,81 top-level/79 child tests (38.334s wall),
zero failures, with exactly one explicit unavailable actual-AR-tool skip.
Three freshly rebuilt actual CLIs, real HTTP capture, actual public Debian
outer framing and actual-file/synthetic-signed composition all passed. Fixture
original size53080 and SHA256
`2e6e2f1a0007dc43bc91c273fd36e91e40a4f1c2765a03eca68b70a42103878a`
were checked throughout. Public bytes were only read; no installation/execution
or repository/publisher authenticity was tested. AR synthetic controls and this
public package do not substitute actual AR-tool-produced E2E. Controls include
all four signed roles, exact/below capacities,33MiB expanded native payloads
under Store/Deflate (not a claim about Deflate container size), aliases,
late-corrupt/unsupported physical frames, zero-result errors and restored positives.

Actual isolated PostgreSQL17.11 passed one top-level/four child tests,
zero skips/failures (28.726s recorded interval), retaining all
earlier SBOM/JSONL/current-authority coverage and adding owned four-role bytes,
signed framing positive, non-AR refusal after old byte-stage positive and
current-deny/restores. Before/after inventory matched:
0 pre-existing fixture databases and
1 pre-existing fixture role retained; no new
fixture database/role remained. Owner mutations are adversarial storage controls,
not approved business deny commands or production data.

All13 root gates passed: three CLI builds, affected tests, actual PG, full
default service, modules, full vet, server build, pinnedV1, formatting and both
whitespace checks. Default passed61 tested packages,
541 top-level/1132 child tests
(69.491s wall), with570 top-level/
606 child opt-in/unavailable-host skips and12 no-test
packages separately recorded. Unavailable sourcefile host fixtures remain
unverified. Each independent reviewer passed nine gates: three own CLI builds,
affected81/79 tests plus the same single actual-AR-tool skip, actual PG1/4 with
zero skips/failures, affected vet, format and two whitespace checks. Their
affected walls were26.509s/27.927s;
PG intervals were19.72s/20.582s
(dependency/compatibility). PG intervals include pre-inventory through Go
completion, excluding verified post-inventory and frozen-source checks. Other
command walls exclude frozen-source checks. Independent runs did not repeat
the root whole-default suite. All sessions/owned children closed, PG inventories
restored and full source/index/status,69 old SQL identities,four official SBOM
resources and public fixture stayed fixed. All14 imports remain exact.

These are candidate bytes and physical framing, not decompression/TAR/control,
app identity/version/architecture, publisher/native authentication, protected
staging, execution or Artifact admission. Synthetic declarations do not certify
the public package. No production SQL/grant/route/dependency/provider/Root/
Session/UI/contract/version change or physical/deployment qualification is
claimed. Full T09 stays in progress; totals remain5/12/30 out of47. Product0.10.9,
normative1.0,pinnedV1,CodeWhale and deployment deferral remain unchanged.
Downloads have no hard total/cumulative duration or retry cap;32MiB is not an
artifact/download-size limit.

DCO committed and pushed as `023218a899a54284eb6fdb169ae46fe8decd3c37` on
`codex/t09-native-framing`; scope/evidence is in
`docs/artifact-native-framing-integration.md`.

### T09 bounded stored and expanded Debian bytes

Six consecutive accepted commits from
`2818db15d7ce20a79ba68ae34f88feb694b1fabc` through
`b6c6e9f69f8f8d2900aa9ae7099857be368e4c0b` were initially imported as 14 exact
raw/Git blobs. Two existing document/PG-test paths matched the first parent;
12 new paths were absent. Original 261 additive document and 47 additive PG-test
lines remain historical source evidence, separate from fresh combined-tree checks.

Independent review found the SDK's hidden 512-byte text buffer refused lawful
gzip FNAME/FCOMMENT within explicit stored capacity. A primary red regression
passed eight 511-byte children but failed 32 first/later 512/513/32769/131073-byte
children plus the independent Python long-header case: two failed top-level,
32 failed child cases, eight passing children, 10.022s recorded interval including
post-run source-freeze checks. The initial original 13-gate passing run is retained
as unrepaired evidence and does not represent final acceptance. The primary
repaired gzip.go and added gzip_header.go/test: original header/XLEN/opaque Latin-1
NUL fields/FHCRC are verified in bounded fragments without retaining text. A
fixed private header excludes already checked optional metadata before SDK
Deflate/trailer processing; Reader and ByteReader share original source position.
The stored Snapshot/identity stays unchanged. No filename/path/UTF-8 or 512-byte
policy is added. Thirteen accepted imports remain exact; one is repaired and two
new files have separate primary repair raw/Git identities. Original proof is kept.

Complete immutable Debian framing precedes exact stored control/data captures.
Shared closed codec adapters first validate full EOF/CRC and count expanded
length within a fixed buffer, then capture the same input's exact expanded bytes.
Gzip validates every concatenated header, reserved flag, lawful optional header
CRC and trailer; bzip2 validates concatenation, block and terminal file CRC.
Stored/expanded control/data capacities are independent; SDK workspace and total
heap/preparation policy remain separate. Plain reuse preserves immutable owned
bytes while checking expanded capacity. Only control plain/gzip and data
plain/gzip/bzip2 are supported here; other recognized labels are unsupported.
Signed composition repeats current Manifest/Root/deny/native/framing then decodes
the same native source. Every failure returns no earlier facts or partial pair.

Both independent scoped line-level reviews passed. Primary affected testing
passed 13 packages, 107 top-level/121 child tests (34.036s wall),
zero failures and exactly one explicit unavailable actual-AR-tool skip. Eleven
required actual cases passed: three freshly rebuilt CLIs, public Debian framing,
stored pair and synthetic-signed framing, independent gzip stream, independent
Debian gzip/bzip2 decoding, synthetic-signed bzip2 composition and independent
long gzip headers. Python standard library independently read/decompressed four
fixed generated fixtures; per-member zlib also verified long headers/FHCRC before
Go. Five original fixture identities were checked throughout. Fixture original
sizes/SHA256 were gzip 278/809c54939b6324cda731e7e9a3eff6c402e25c90b878ba46fe9720a423d48b48,
Debian gzip 520/0c7bfea20dbd6983f7b142f3f693ca028095264ba1e8a93ec2e64f2545cfc7c7,
Debian bzip2 10570/f54ccbd8c327a98960af0562ba465c25a184c4a27dd06c42409ae7570fac78a3,
plus the unchanged 53080-byte public hello fixture. Expanded identities match
their frozen independent originals. The new long-header sequence is 230517 bytes,
SHA256 81b886e0e96a97379cf9f86f96f7cba0fdc00c08a9268c372fb317191d181b60,
expanded 420/SHA256 1782c4d4d140688dae63f07542d98971c45ea68f89cf92bd722edfd3dc694e83.
Nothing was installed/executed or certified
as an application, publisher or repository release. Actual AR-tool output remains
unverified. Controls retain exact/below capacities, empty/concatenated streams,
post-payload file CRC errors after SDK output, first/later header faults, restored
positives, owned buffer isolation and 33 MiB expanded gzip/bzip2 capacities.
All 40 first/later long-field/CRC combinations pass, as do Latin-1, maximum XLEN,
empty fields, isolated header/trailer corruption and same-original restoration.

Actual isolated PG17.11 passed one top-level/four child tests, zero skips/failures
(33.76s recorded interval). Existing coverage remains; additions
observe plain opaque control/data, original/expanded identities and Root revision2,
one-below expanded data memory after signed framing success, zero earlier facts,
current deny and exact restoration. PG gzip/bzip2 CRC scenarios are not claimed.
Owner storage mutations are adversarial controls, not approved business commands.
Before/after inventories matched: 0 pre-existing
fixture databases and 1 pre-existing fixture role
retained; no new fixture database/role remained.

All 13 root gates passed: three CLI builds, affected tests, actual PG, full default
service, modules, full vet, server build, pinned V1, formatting and two whitespace
checks. Default passed 62 tested packages,
561 top-level/1174 child tests
(132.67s wall); 576 top-level/
606 child opt-in/unavailable-host skips and 12 no-test packages
were recorded separately. Sourcefile host fixtures remain unverified. Each
reviewer passed nine gates with its own three builds, 107/121 affected tests plus
the same single AR-tool skip, actual PG1/4 zero skips/failures, affected vet,
formatting and both whitespace checks. Dependency/compatibility affected walls
were 30.52s/31.233s; PG intervals were
24.627s/19.925s. PG intervals include
pre-inventory through Go completion, excluding post-inventory and frozen-source
checks. Other command walls exclude frozen-source checks. Independent runs did
not repeat the root whole-default suite. All sessions/owned children closed,
PG inventories matched their baseline, and full source/index/status, 69 old SQL
identities, four official SBOM resources and all five fixture files stayed fixed.
Thirteen imports remain exact; three primary repair paths match separate proof.

No TAR/control/path/script interpretation, app/version/architecture/publisher or
native authentication, protected staging, execution, Artifact admission or
physical/deployment qualification is supplied. No SQL/grant/route/dependency/
provider/Root/Session/UI/contract/version change. Full T09 stays in progress;
totals remain 5/12/30 out of 47. Product 0.10.9, normative 1.0, pinned V1,
CodeWhale and deployment deferral remain unchanged. Downloads have no hard
total/cumulative duration or retry cap; 32 MiB is not an artifact/download-size limit.

DCO committed and pushed as `de7c555d3a0d61cbae5bb5530f55005ac48d8129` on
`codex/t09-debian-decode`; scope/evidence is in
`docs/artifact-debian-decode-integration.md`.

### T09 explicit LZMA-alone and Debian data decoding

Two consecutive accepted commits b4b9ee693468eed1862fcc30813521be497801b7
and b963a3dfc9229b865cad972e893d5b2c2f436015 were initially imported as nine
exact raw/Git blobs: five existing paths matched their first parent, four paths
were absent. The source delta adds 102 historical documentation lines, one
go.mod line, two go.sum lines, 11 added/5 removed Debian dispatch lines and 46
added signed-composition test lines. Historical source evidence is separate from
fresh combined-tree runs.
Eight imported paths remain exact. The repaired lzma.go and two new EOF
implementation/test paths have separate primary repair proof. All three earlier
gzip repair paths stayed exact. No whole-branch overwrite was performed.

The only added module is github.com/ulikunitz/xz v0.5.17, its pure-Go classic
LZMA Reader. Official proxy download and public sum.golang.org lookup matched
both pinned go.sum entries. Its go.mod has no additional module requirements.
Actual cache source and all file identities were checked; the BSD license is
retained byte-exact under compression/third_party/ulikunitz-xz.LICENSE and must
accompany binary redistribution materials. Every root/independent run checked
the pinned cache, public-checksum proof and license; modules verified separately.

Explicit stored, expanded source/memory and dictionary capacities are distinct.
Properties, original header dictionary and known expanded size are checked before
SDK allocation. Dictionary has a minimum of 4096 bytes and reserves its sentinel
byte before host-int conversion. SDK probability/fixed workspace and total
preparation heap remain separate. The pinned SDK supports known-size headers
through 2^50 bytes (1 PiB); above that returns LZMAUnsupported after caller/host
capacity checks. Unknown-size
sentinel does not have this SDK header boundary. This is an implementation support
boundary, not an artifact/download policy, timeout or 32 MiB limit.

Independent pre-import SDK/liblzma evaluation covered 167 synthetic/specification
profiles. It exposed two bugs: canonical known0/no-EOS was refused, and 22 corrupt
known+EOS finalization variants were accepted. Duplicate empty-writer and
canonical vectors represent one bug. A red original-adapter regression passed 143
children and failed 24 children plus its parent (7.53s
command wall); that expected failing run is retained, not final acceptance.
Python ALONE independently checks true known/no-EOS. Its wrapper refuses even
the lawful specification known+EOS positive, so independent RAW LZMA1 verifies
full EOS/no unused bytes plus exact header count and output SHA for that profile.
That reference limitation is not classified as a malformed artifact.

Primary repair recognizes only exact 18-byte known0 with all five range-init
bytes zero, after all ordinary guards, and returns normal owned empty Capture.
Other zero-size forms use the full decoder. Known+EOS is first decoded normally,
then replayed through the SDK's strict unknown-size EOS path using the same
immutable source and only a private size-header substitution. Complete EOS,
exact stored EOF, original known count, expanded SHA and context are checked with
fixed scratch, no second expanded capture. Known/no-EOS is not required to have
an EOS marker. Original decoder references are dropped before replay; GC timing
does not guarantee total heap. Replay may use full header dictionary instead of
known-size shrinking, always within explicit dictionary capacity. Any failure
returns no Snapshot or earlier Debian/signed facts. No dependency fork is used.
LZMA-alone has no checksum/magic authentication; identity is an observation only.

Debian dispatch adds only data.tar.lzma with explicit LZMADictionaryBytes.
Existing keyed zero-field plain/gzip/bzip2 behavior remains compatible; no control
LZMA, XZ or Zstd dispatch is added. Signed composition repeats current Manifest,
Root and deny checks over the same original native-byte source.

Both scoped line-level independent reviews passed after primary repairs.
Root passed 14 gates: three own CLI builds, affected tests in 13 packages, actual
isolated PG, whole default suite, modules, full vet, server build, pinned V1,
license/cache proof, formatting and two whitespace checks. Affected passed
119 top-level/295 child tests, zero failures,
one explicit unavailable actual-AR-tool skip (37.252s wall).
Thirteen required actual cases passed, including the prior 11 actual cases plus
independent Python-produced Debian LZMA data and synthetic-signed composition.
All 167 EOF children, canonical empty guards/restoration, nonallocating known-size
and 32-bit dictionary boundaries, capacities, truncation/tail/concatenation,
cancellation, owned isolation and 33 MiB expanded LZMA passed.

Six fixed original fixtures stayed exact. The new Debian LZMA fixture is 10588
bytes, SHA256 2aa4a71a063e4e826aedb0fd71f668738e41914ccdc33816ec7edbac35b743d8;
Python independently decoded its control/data to 10240-byte originals with frozen
identities. Prior gzip/bzip2/long-header independent checks remain enabled.
Nothing was installed/executed, or certified as an app/publisher/repository release.

Actual PG17.11 passed 1 top-level/4 child tests with zero skips/failures
(27.927s recorded interval). This is unchanged plain-member stage
regression, not PG LZMA or CRC qualification. Before/after inventories matched:
0 pre-existing fixture databases and
1 pre-existing fixture role retained; no new
fixture resources remained. Whole default passed 62 tested
packages, 571 top-level/1348 child tests
(141.064s wall); 578 top-level/606 child
opt-in/unavailable-host skips and 12 no-test packages are separate. Sourcefile
symlink/mapped-drive host fixtures remain unverified, as does actual AR-tool output.

The initial whole-default run failed one unchanged upgrade-events native-pipe
cancellation test at its five-second observation allowance (144.745s
whole-run wall; 570 top-level/1348 child passes, 578/606 skips, one top-level
failure). Source stayed fixed. Isolated original-test repetition passed five of
five (11.802s wall). The complete default suite was then rerun
and passed with the counts above; the isolated positives did not replace that
gate. Accepted root evidence combines the first five passed stages and nine
subsequent retry/remaining stages, preserving the initial failure separately.
No event code or test allowance changed.

Each reviewer passed 11 gates with own three CLI builds, identical affected
counts/13 actual cases/one AR skip, PG 1/4 zero skips/failures, modules, affected
vet, license/cache proof, formatting and both whitespace checks. Dependency and
compatibility affected walls were 35.649s/
34.062s; PG intervals were
20.621s/19.287s.
PG intervals include pre-inventory through Go completion and exclude post-inventory
and frozen-source checks; other command walls exclude frozen-source checks.
Independent runs do not repeat the root whole-default suite. All sessions/owned
children closed. Source/index/status, 69 prior SQL identities, four official SBOM
resources, six fixtures, prior gzip repairs and new import/repair identities stayed
fixed. Official module/retained license matched in all three runs.

No TAR/control interpretation, app/version/architecture/publisher or native
authentication, protected staging, execution, Artifact admission or physical/
deployment qualification is supplied. No SQL/grant/route/provider/Root/Session/
UI/contract/version change. Full T09 stays in progress; totals 5 implemented,
12 in progress, 30 incomplete/deferred out of 47. Product 0.10.9, normative 1.0,
pinned V1, CodeWhale and deployment deferral stay unchanged. Downloads have no
hard total/cumulative duration or retry cap; 32 MiB bounds only the joined atomic
business inventory.

DCO committed and pushed as `e1bba99a91b0033d3630e70a379e2e2f75f2b44c` on
`codex/t09-lzma`; scope/evidence is in `docs/artifact-lzma-integration.md`.

### T09 ordinary TAR and current signed Debian pair inspection

Six new code/test paths were selected from consecutive source commits
e41c587b62cd9f7e2be0384b2dd853144ee25c35 and
e152ab2b3e26006ce20ce46030e370c6c9814045. Their first parent contains codec
work that has not been integrated. The seventh changed path, the historical
artifact-verification-plan.md with 119 added documentation lines, was deliberately
omitted; its earlier combined-tree results and codec scope are not current evidence.
All six new paths were initially imported as exact raw/Git blobs after absence
checks. Three production files and the standalone TAR test remain exact; two
consumer test files have separately recorded primary adaptations to supported
codecs. No whole-branch merge or historical-plan overwrite was performed.

The ordinary TAR adapter observes immutable owned bytes without extraction,
link following, permission changes or execution. Explicit source/payload/member
capacities are distinct from aggregate preparation policy. Ordinary V7/USTAR/GNU
headers use the actual Go 1.27.2 standard parser for checksum and numeric fields.
PAX, GNU long names/links, sparse formats, STAR and unknown types are Unsupported
before hidden extension parsing. A valid header accompanied by ErrInsecurePath
remains observable because declarations are never host paths; ambient GODEBUG and
host filepath rules do not change these byte observations. Original legacy type
is retained, while normalized SDK type controls body rules; non-file bodies fail.
Physical spans use remaining-length checks before arithmetic, each payload is
hashed with fixed 32 KiB scratch and context checks, and complete source EOF
requires two terminal blocks plus only whole zero blocks thereafter. Duplicate
names retain distinct physical spans. Failures return zero Report, not partial
earlier members. Names, links and modes remain untrusted declarations.

Debian InspectTars preserves the same owned original installer through framing,
stored capture, decoding and both complete TAR reports. Control/data TAR limits
are independent; late data failure discards earlier control and framing facts.
The verifier first applies current signed Manifest/Root/deny/native-byte guards,
then inspects the same Native.Bytes and checks the whole-source identity again.
Later failure or cancellation returns no earlier signed facts or expanded pair.
Existing inspection/decoding APIs are unchanged; no new authority is granted.

Primary test adaptations remove references to unintegrated XZ/Zstandard fields
and success cases. The actual whole-DEB TAR matrix requires gzip, bzip2 and LZMA,
with three separately identified child cases. Actual signed positives use frozen
bzip2 and LZMA originals. The public XZ package is a precise negative: original
signed/framing verification first succeeds with its XZ member labels, followed
by exact Unsupported and zero earlier facts from the TAR composition. Standalone
inspection of its independently decoded raw TAR files does not mean that whole
XZ decoding is supported. XZ integration remains deferred pending an explicit
dependency-maintenance decision; no candidate XZ code was imported.

Both independent scoped line-level code/test reviews passed. Primary passed
14 gates: three own CLI builds, affected tests, actual isolated PG, whole default,
modules, full vet, server build, pinned V1, license/cache proof, formatting and
both whitespace checks. Affected tests passed 14 packages, 136 top-level/
298 child cases, zero failures (46.756s wall), with exactly one explicit
unavailable actual-AR-tool skip. Eighteen required actual cases passed: the prior
13 plus standalone TAR fixtures, the supported DEB TAR matrix, signed bzip2/LZMA
TAR compositions and the precise public-XZ refusal. Original 167 LZMA EOF and
40 long-gzip cases remain enabled and passed. Ordinary header types, V7/GNU,
ambient host path policy, STAR refusal, numeric MaxInt64/negative/invalid values,
header-only bodies, late checksum/tail/terminator errors, all explicit capacities,
cancel/nil handling, owned isolation and restoration passed. A real 33 MiB TAR
payload passed its caller-selected capacity and failed one-below; it does not
inherit the unrelated atomic business inventory bound.

Twelve original fixture files stayed fixed: the prior six whole/codec files and
six raw TAR files. Python tarfile independently observed 152 ordinary members in
four supported raw files and accepted the two extension references which this
adapter explicitly refuses. Public control/data TAR bytes were rederived with
independent liblzma from the exact frozen original public DEB and matched the
10240/256000-byte raw sources (3/143 members). USTAR/GNU each have 10240 bytes
and 3 members. No package or archive was extracted, installed or executed; no
publisher, native identity or deployment permission is attested by these fixtures.

The initial root runner completed three builds and affected tests, then stopped
before running a PG test because the pre-inventory connection was refused. The
existing isolated local PG 17.11 test cluster had stopped; its owned test-data
path, version and loopback port were verified and the fixture was restarted.
Source and index were unchanged. Accepted root evidence combines those first
four passed stages with ten resumed stages, retaining the original environment
failure diagnostics separately. Production deployment was not changed.
Actual PG passed 1 top-level/4 child tests, zero skips/failures
(28.287s recorded interval). This is unchanged plain-member/current-trust
regression, not new PG TAR qualification. Before/after inventories matched:
0 pre-existing fixture databases and
1 pre-existing fixture role preserved, with no new
fixture resources left. Whole default passed 63 tested packages,
583 top-level/1348 child cases (172.426s wall).
Its 583 top-level/606 child opt-in/unavailable-host
skips and 12 no-test packages are separate. Sourcefile symlink/mapped-drive host
fixtures and actual AR-tool output remain unverified.

Each reviewer passed 11 gates with own three CLI builds, the same affected counts,
18 required actual cases/one AR skip, PG 1/4 zero skips/failures, modules, affected
vet, license/cache proof, formatting and two whitespace checks. Dependency and
compatibility affected walls were 43.297s/
43.162s; PG intervals were
24.459s/20.739s. PG intervals
include pre-inventory through Go completion, excluding post-inventory and frozen
checks; other command walls exclude frozen checks. Reviewers did not repeat the
root whole-default suite. All sessions and owned stage children closed, with PG
inventories restored. Full source/index/status, all 69 prior SQL identities,
four official SBOM resources, 12 fixtures, go.mod/go.sum, the historical plan,
the prior six gzip/LZMA repair paths and new source/repair identities stayed fixed.
The pinned module cache, official-checksum proof and retained license matched.

No control text/path/script interpretation, extraction, native app/version/
architecture/publisher authentication, protected staging, execution, Artifact
admission or physical/deployment qualification is supplied. No existing codec,
dependency, SQL/grant/route/provider/Root/Session/UI/contract/version change.
Full T09 stays in progress; totals remain 5 implemented, 12 in progress and 30
incomplete/deferred of 47. Product 0.10.9, normative 1.0, pinned V1 and CodeWhale
stay unchanged; deployment remains deferred. Downloads have no hard total or
cumulative duration or retry cap. The 32 MiB limit applies only to joined atomic
business inventory, not artifacts or downloads.

DCO committed and pushed as `7105e81b4cf07a9c19edb2700f5f10a66a22d5db` on
`codex/t09-tar`; scope/evidence is in `docs/artifact-tar-integration.md`.

### T09 Debian control and required identity declarations

Twelve new code/test paths were selected from consecutive source commits
6ef51a5c653e9b7f626026d9899be9f2e5ec4da2,
4815e0b8bba5dda3a8bb0f36fc67bca07aa3cc7d,
087fbe1b95b645fa736ddf563035d868ba77e34c and
ea0121b53847779d4b4c51d875016d45923a591f. Their first parent is e152ab2b;
the integrated baseline preserves the separately accepted TAR and codec repairs.
The historical artifact-verification-plan.md and its 227 added lines were omitted:
its combined-tree codec assumptions/results are not current evidence. All twelve
paths were initially exact raw/Git imports after absence checks. Six production
files and two standalone tests stay exact; four consumer tests have separately
recorded primary adaptations to the supported codecs. No whole-tree merge.

ParseControl observes one owned UTF-8 binary-control stanza, retaining original
field spelling and value/continuation lines. ASCII field names follow Debian
Policy 5.1, with case-folded uniqueness; LF/CRLF, final EOF without newline and
horizontal-whitespace separators are explicit. Orphan continuations, comments,
second stanzas, wholly empty values, invalid UTF-8, disallowed ASCII controls,
DEL and stray CR refuse without earlier fields. Other valid UTF-8 values stay
opaque. Source/memory capacities bound each logical copied source, field count
bounds cardinality; none is a combined heap or aggregate preparation budget.
Chunked source copy and successful return check context. A real 33 MiB opaque
value passed caller-selected capacity and failed one-byte lower logical memory.

InspectControl first observes both complete TARs, then selects exactly one root
control or ./control physical regular/legacy-zero member. Duplicate names/aliases,
missing roots and links/special types refuse. Nested/absolute paths are not root
aliases. Exact member offset/size are captured from the owned expanded bytes,
their identity is compared and lexical parsing uses that captured snapshot.
ReadIdentity reparses owned bytes and requires Package, Version and Architecture
each as one nonempty line, trimming only horizontal whitespace. It does not
consume mutable returned maps or guess defaults. InspectIdentity preserves the
complete control composition and checks both record identities. All late errors
and cancellation return zero earlier framing/parsed fields and no captured bytes.

The two verifier compositions apply existing current signed Manifest/Root/deny/
native-byte/framing guards first, then observe the same Native.Bytes and check
whole-source identity again. Signed claims in tests are synthetic byte bindings.
Package/version/architecture remain declarations: no field-specific grammar,
version ordering, approved app/architecture mapping or publisher trust is granted.
No extraction, path resolution, link following, script execution or installation.
Existing APIs, codecs, routes, SQL, dependencies, grants and providers are unchanged.

All previous twelve whole/codec/TAR fixtures and two new raw control records
remained fixed. Independent Python tarfile read control payloads without filesystem
extraction; email.parser independently checked field counts and three declarations.
Public control: 757 bytes, SHA-256
27ee01d2de09a1a678763c41013d4d1aa47e6985230ca08f414e903a237fd163,
13 fields, hello/2.10-3/amd64. Synthetic control: 109 bytes, SHA-256
d557a48d2711f6984cb7b3c4bfa4d67e95db4da68e14fd967d0199d7a6253d79,
4 fields, independent-fixture/0.0.1/all; independently decoded gzip/bzip2/LZMA
originals yield identical raw control bytes. Public raw-control positives rely
on independently decoded raw TAR bytes, not whole-XZ support.

Both named whole-DEB matrices require gzip, bzip2 and LZMA children. Four signed
positive groups use frozen bzip2/LZMA packages. Two public-XZ negatives first pass
current signed/framing verification, then require exact Unsupported and zero
earlier control/identity facts. XZ remains deferred pending its explicit owner
maintenance decision; no candidate XZ or Zstandard integration was imported.

Both independent scoped line-level code/test reviews and primary self-review
passed. Root passed 14 gates: three own CLI builds, affected native tests, actual
isolated PG regression, whole default, modules, full vet, server build, V1 pins,
cache/license proof, changed formatting and two whitespace checks. Affected tests
passed 14 packages, 160 top-level/304
child cases, zero failures (53.905s wall), with exactly one unavailable
actual-AR-tool skip. All 28 required actual groups passed: the prior 18, eight new
positive control/identity groups and two public-XZ refusals. The previous 167
LZMA EOF and 40 long-gzip cases remain enabled and passed. Lexical, malformed/
duplicate/second-stanza, alias/type/capture, missing/folded required fields,
current deny, capacity, cancellation/nil, projection isolation and restoration
cases passed. Whole default passed 63 tested packages,
597 top-level/1348 child cases (252.123s);
593 top-level/606 child opt-in/unavailable-host skips
and 12 no-test packages are separate. Host sourcefile symlink/mapped-drive
fixtures and actual AR-tool output remain unverified.

Actual unchanged plain-member/current-trust PG regression passed 1 top-level/
4 child tests, zero skips/failures (34.008s recorded interval), with
0 pre-existing fixture databases and
1 pre-existing fixture role unchanged before/after.
This is not new PG control/identity qualification. Each reviewer separately
passed 11 gates: own three CLI builds, affected tests with the same counts,
28 actual groups/one AR skip, PG 1/4 zero skips/failures, modules, affected vet,
cache/license proof, fmt and diffs. Dependency/compatibility affected walls were
50.385s/48.505s;
PG intervals 22.205s/25.226s.
PG intervals run from pre-inventory through Go completion, excluding post-inventory
and freeze checks; other command walls exclude freeze checks. Reviewers did not
repeat root whole-default. All owned stage children/sessions closed; PG inventory
was restored. Full source/index/status, 69 prior SQL raw/index identities,
16 protected paths (including prior six codec repairs, six TAR paths and its doc),
four official SBOM resources, 14 fixtures and module/license/checksum proofs stayed
fixed during each actual run.

No Artifact admission, protected staging, native qualification, physical platform
or deployment acceptance is supplied. Full T09 remains in progress; totals stay
5 implemented, 12 in progress and 30 incomplete/deferred of 47. Product 0.10.9,
normative 1.0, pinned V1 and CodeWhale stay unchanged; deployment remains deferred.
Downloads have no hard total/cumulative duration or retry cap. The 32 MiB bound
applies only to joined atomic business inventory, not artifacts or downloads.

DCO committed and pushed as `7ff8a99611ea04ca6ed5ae9614a98d8c0a029394` on
`codex/t09-control`; scope/evidence is in `docs/artifact-control-integration.md`.

### T09 standalone Debian byte/declaration inspection command

Two new command code/test paths were initially imported exactly from
14b71f40af4fa792ee34e704f770a70899bcb5ab (parent ea0121b5). Its historical
artifact-verification-plan.md with 86 added lines was omitted because its
XZ/Zstandard combined-tree evidence is not current acceptance. Both imported
files have separate primary adaptation proofs; neither is claimed source-exact
after adaptation. Production removes the four unavailable codec flags and their
capacity checks; tests use codec-keyed expectations and current supported inputs.
Prior accepted control/TAR/codec code and documentation remain unchanged.

upgrade-debian-inspect opens a supported ordinary descriptor through sourcefile,
captures bounded owned bytes, then performs complete framing/decoding/two TARs/
unique regular root control/required declaration inspection. Success emits one
JSON report with installer, stored/expanded/control identities, exact TAR member
counts, format and three declaration values. No archive paths or other fields.
This standalone command supplies unsigned byte/declaration observations; it
does not invoke current signed Manifest verification or grant native app/version/
architecture/publisher approval, Artifact admission or installation authority.

All 22 registered capacity arguments must be positive. Their logical copy,
dictionary and report meanings follow the accepted adapters, not aggregate heap
or production preparation budgets. For the small frozen supported fixtures only:

```sh
go run ./cmd/upgrade-debian-inspect \
  --package-bytes 2097152 --package-memory-bytes 2097152 --version-bytes 16 \
  --stored-control-bytes 2097152 --stored-control-memory-bytes 2097152 \
  --stored-data-bytes 2097152 --stored-data-memory-bytes 2097152 \
  --expanded-control-bytes 2097152 --expanded-control-memory-bytes 2097152 \
  --expanded-data-bytes 2097152 --expanded-data-memory-bytes 2097152 \
  --lzma-dictionary-bytes 4096 \
  --control-tar-bytes 2097152 --control-tar-member-bytes 2097152 \
  --data-tar-bytes 2097152 --data-tar-member-bytes 2097152 \
  --control-record-bytes 1024 --control-record-memory-bytes 1024 \
  --ar-members 10 --control-tar-members 1000 --data-tar-members 1000 \
  --control-record-fields 20 fixture.deb
```

Exit 0 means complete JSON/help, 2 means invalid arguments, 1 means source,
inspection, cancellation or output failure. Fixed diagnostics omit private
arguments/source/writer details. Short/failed writes fail even if a later writer
call would succeed. Output-device failure can leave truncated bytes; callers
must require exit 0 and complete JSON. Owned snapshots resist later path
replacement, but do not certify arbitrary concurrent same-size writes. Existing
sourcefile OS support/remote mount/symlink limitations remain; no protected staging,
link following, extraction or execution is introduced.

The initial root runner passed the artifact and SBOM builds, then stopped at
provenance build because a helper edit mistakenly supplied two packages to one
non-directory output. No feature or PG test had started. Source/index stayed
unchanged; the command was corrected to its one intended target. The original
failure is retained separately. Accepted root evidence combines those first two
passed stages with thirteen resumed stages, reusing the two hash-checked original
build outputs for actual tests. Each reviewer uses the corrected normal helper
and builds all four of its own binaries.

Both independent scoped line-level code/test reviews and primary self-review
passed. Root passed 15 gates: four own inspector builds, affected tests, isolated
actual PG regression, whole default, modules/full vet/server build/V1 pins,
cache-license, formatting and two diffs. Affected tests passed
15 packages, 167 top-level/314 child
cases, zero failures (62.409s), with exactly one unavailable actual-AR
tool skip. All 31 required actual groups passed: prior 28 plus frozen supported
CLI matrix, public-XZ exact refusal and built E2E. The supported matrix checks
every reported byte identity, count and declaration for gzip/bzip2/LZMA.
Built E2E passed seven named invocations: synthetic success, invalid arguments,
late missing required field, the three frozen supported originals and public-XZ
refusal. Both run() and binary require exact Unsupported and zero stdout for XZ.
All missing/zero/negative capacities, help, source/copy/field limits, cancellation,
same-source restoration, replacement isolation and broken/short writer cases
passed. The 60-second test-child allowance is not a production download limit.

Root whole default passed 64 tested packages,
601 top-level/1348 child cases (208.318s);
596 top-level/606 child opt-in/unavailable-host skips
and 12 no-test packages are separate. Root unchanged plain-member/current-trust
PG regression passed 1/4 with zero skips/failures (23.628s interval),
not new CLI/database qualification. Each reviewer passed 12 own gates with four
builds, the same affected counts/31 actual groups/one AR skip, PG 1/4 zero skips/
failures, modules, affected vet, cache-license, fmt and diffs. Dependency and
compatibility affected walls were 42.798s/
45.742s, PG intervals
29.845s/21.0s.
PG intervals include pre-inventory through Go completion, excluding post-inventory
and freeze checks; other command walls exclude freeze checks. Inventories kept
0 pre-existing fixture databases and
1 existing role unchanged with no new
resources left. Reviewers did not repeat root whole default. All stage children/
sessions closed. Full source/index/status, 29 protected prior paths, 69 SQL raw/
index identities, four official SBOM resources, fourteen fixed fixture files and
module/license proofs stayed fixed; independent Python codec/TAR/control audits
passed without filesystem extraction. Actual AR and sourcefile symlink/mapped-drive
host fixtures remain unverified.

No dependency/SQL/grant/route/provider/UI/contract/product-version change. XZ
maintenance remains owner-pending; no candidate XZ/Zstandard code was imported.
Full T09 remains in progress; totals stay 5 implemented, 12 in progress and
30 incomplete/deferred of 47. Product 0.10.9, normative 1.0, pinned V1 and CodeWhale
stay unchanged; deployment deferred. Downloads have no hard total/cumulative
duration or retry cap. The 32 MiB bound is joined atomic inventory only.

DCO committed and pushed as `76519974e414745b32c00f62c937a30dee00bcf7` on
`codex/t09-debian-cli`; scope/evidence is in `docs/artifact-debian-cli-integration.md`.

### T09 original ELF physical observations and same-TAR selection

Eight new code/test paths were initially imported exactly from four consecutive
source commits f009438ef3895524b52aa99f7fbe7141a0281b69,
25efea0a9ba8360f00708bc1b159cfa2d67c2c9c,
04a5cbc8a0f300cd870f5934507710877673d2f7 and
310bb926d080266592007bcf9a168d58e2903e15. Their historical combined-tree
artifact-verification-plan.md changes (262 added lines) were omitted. The first
parent's PE functionality is not imported. Six paths remain source-exact; only
the signed-composition and CLI consumers' tests were adapted, with separate
initial/final proofs. All four production files remain source-exact. Existing
TAR/control/codecs and current signed verification remain unchanged.

elf.Inspect observes complete owned ELF64 little-endian current-version bytes.
Known 64/56/64-byte header/table widths, ordinary numbering, explicit positive
source/program/section limits and every nonempty physical backed span are
checked before hashing. Overflow-safe span checks precede integer conversion.
Reports retain raw header/table/entry fields and original SHA-256 identities;
aliases remain separate declarations. NULL programs/sections and NOBITS sections
have empty Data identities while their unused raw fields remain declarations.
ELF32, big-endian and extended/reserved numbering return Unsupported. No loader,
name resolution, relocation, decompression, alignment or virtual-memory policy,
machine/app/version/publisher approval, signature authentication or admission is
inferred. The bounds describe logical source/counts and fixed hash scratch, not
aggregate heap/work or a production preparation budget. Repeated aliased spans
may be hashed separately. A 33 MiB overlay positive is not an artifact size cap.

tar.CaptureRegular freshly inspects the entire same immutable TAR, including
later entries, terminators and the full trailer, before selecting exactly one
1..256-byte declaration name. Every same-name competitor counts regardless of
type. Only ordinary regular declarations are captured; the legacy zero-type
trailing-slash directory form refuses. No prefix/case/path normalization, host
path, link following, extraction or execution is involved. Captured original
payload and member identity must agree, including empty-file and 33 MiB positive
controls. InspectELFDebianInstaller first preserves current signed installer,
complete control/data TAR and required control guards, then reselects one member
from those same owned data bytes and binds its ELF identity. No default binary
location is supplied. Every late error/cancellation returns the literal zero
composite, discarding all earlier signed/native/member facts.

upgrade-elf-inspect is a standalone unsigned physical-observation command. It
uses the existing ordinary-descriptor sourcefile adapter and owned capture,
requires four positive capacities, and emits one complete JSON on success:

```sh
go run ./cmd/upgrade-elf-inspect \
  --source-bytes 67108864 --snapshot-memory-bytes 67108864 \
  --programs 128 --sections 128 fixture.elf
```

These example values are test capacities, not selected production defaults.
Exit 0 means complete JSON/help, 2 invalid arguments, 1 source/inspection/context/
output failure. Fixed diagnostics do not expose private arguments/source/writer
details. Sticky short/error/negative/overlong writes fail; a failed device may
leave truncated bytes, so callers must require exit 0 and complete JSON. Existing
sourcefile host/remote/symlink limitations and same-size concurrent-write caveat
remain. No protected staging or execution is added. The CLI does not invoke the
signed-composition API or confer Artifact admission/installation authority.

Two new fixed private fixture files extend the prior fourteen to sixteen:
the original public hello ELF is 31448 bytes, SHA-256:
1aab5d66fba9313733ca534dc9693f262532ab696eb9d29cc70978c5e1c7078c;
its unchanged public data TAR is 256000 bytes, SHA-256:
f0c28e66b1a4d548ff77e392ae277fbba70683818a19ae97c51fbdd6ba46c1b5,
with the exact ./usr/bin/hello payload at offset 2048 among 143 members.
A new synthetic gzip DEB repack is 61722 bytes, SHA-256:
c5890508f2e3989fd8fdd0d326bedc48762645011653581e48128b4a0a79ae20.
It carries unchanged public raw control/data TARs; isolated synthetic signed
claims bind the new container bytes. It is not the original public DEB or a
publisher signature. The original public 53080-byte whole XZ DEB remains unchanged
and is an exact Unsupported/literal-zero negative after current signed/framing
positive controls. No XZ support is claimed. Python tarfile/struct/hashlib/gzip
independently derives/verifies the raw spans, all 13 program/30 section fields
and hashes and deterministic gzip repack, without filesystem archive extraction
or execution. Go debug/elf independently checks all reported raw projections.
A genuine 52-byte ELF32 reference-positive is explicitly unsupported by this API.

Primary self-review and both independent scoped line-level code/test audits
passed. Root passed 16 gates: five own inspector builds, affected tests, isolated
actual PG regression, whole default, modules/full vet/server build/V1 pins,
cache-license, formatting and two diffs. Affected tests passed
17 packages, 185 top-level/319 child
cases, zero failures (62.864s), with exactly one unavailable actual-AR
tool skip. All 37 required actual groups passed, including the prior 31 and six
ELF groups. Actual ELF binary E2E passed five mandatory named children: synthetic
success, invalid arguments, invalid framing, same-path restoration and frozen
original. Prior Debian seven built children and three supported-codec projections
also passed. Missing/zero/negative/overflow capacities, physical truncation/
overflow, unsupported profiles, duplicated/nonregular names, identity binding,
replacement isolation, final cancellation and failed writers passed. Test-child
allowances are not production download limits.

Root whole default passed 66 tested packages,
613 top-level/1348 child cases (200.366s);
602 top-level/606 child opt-in/unavailable-host skips
and 12 no-test packages are separate. The unchanged plain-member/current-trust
PG regression passed 1/4, zero skips/failures (32.84s interval), not
new ELF/database qualification. Each independent reviewer passed 13 own gates
with five builds, the same affected counts/37 required groups/one AR skip, PG 1/4
zero skips/failures, modules, affected vet, license, formatting and diffs.
Dependency/compatibility affected walls were
94.045s/121.508s;
PG intervals 30.638s/26.833s.
PG intervals run from pre-inventory through Go completion, excluding post-inventory
and freeze checks; other command walls exclude freeze checks. Inventories kept
0 pre-existing fixture databases and
1 existing role unchanged, leaving no new resources.
Reviewers did not repeat root whole default. All actual children/sessions closed.
Full source/index/status, 32 protected prior paths, 69 SQL raw/index identities,
four official SBOM resources, sixteen fixtures and module/license proofs stayed
fixed during actuals. Actual AR and sourcefile symlink/mapped-drive host fixtures
remain unverified.

No dependency/SQL/grant/route/provider/UI/contract/product-version change. XZ
maintenance remains owner-pending, and candidate XZ/Zstandard/PE code is absent.
Full T09 remains in progress; totals stay 5 implemented, 12 in progress and 30
incomplete/deferred of 47. Product 0.10.9, normative 1.0, pinned V1 and CodeWhale
stay unchanged; deployment deferred. Downloads have no hard total/cumulative
duration or retry cap. The 32 MiB bound is joined atomic inventory only.

DCO committed and pushed as `e50ad4b39538fc32856a08e9f7277baafe787fbf` on
`codex/t09-elf`; scope/evidence is in `docs/artifact-elf-integration.md`.

### T09 original PE, resource and version-byte observations

Twenty-five new code/test paths integrate the connected original PE/resource/
Windows-version observation capability from ten consecutive source commits,
38934c7dfb2b537c0f0ed0b3cffb8694a570f834 through
93312cd7c1b519beb7666df5819feca8886afb34. Historical combined-tree
artifact-verification-plan.md changes (785 added lines) were omitted.
Separate original/final proofs retain 22 source-exact
paths, three primary adaptations and 0 formatting-only
paths. The two CLI files add the final pre-output context check, independent
completed-prefix cancellation proof, overflow arguments and six mandatory named
built children. The signed-PE test alone adds an independent native-bytes/PE-prefix
control proving its existing final poll; production wrapper behavior is unchanged.
Formatting-only paths: none. Existing accepted ELF/TAR/control/codecs remain fixed.

pe.Inspect bounds ordinary PE32/PE32+ DOS/COFF/optional headers, all raw sections
and embedded WIN_CERTIFICATE spans on complete owned bytes. Sections preserve
declaration order; nonempty physical overlaps refuse before hashing. Certificate
identities include each complete declared dwLength, separately checking external
eight-byte zero alignment. Embedded bytes are opaque, with no DER, Authenticode,
catalog/publisher, loader or NSIS-content authentication. Complete overlays stay
in the original source identity. Unknown machines remain declarations.

Resource RVA backing must be a unique complete original interval in headers or
one section's positive VirtualSize/raw-size intersection, within the uint32 RVA
domain. Every partial competing virtual claim also refuses; no BSS zero-fill,
cross-section stitching or guessed loading is introduced. The complete declared
resource tree is traversed iteratively with entry/depth/UTF-16-name capacities,
strict exact sibling order and checked physical metadata/data spans. Cyclic or
shared directories are outside the supported tree profile; shared names/data
remain observations. No assumed type/name/language lookup depth or defaults.
Caller selectors freeze before callbacks; complete trees precede exact selection
and owned-byte capture. Empty resources retain owned empty snapshots.

Windows version APIs separately observe known VS_VERSION_INFO framing and its
52-byte fixed words, a complete bounded physical block tree, exact opaque value
selection, and the explicit uint16 inner-record boundary with separately hashed
opaque tail. Four-part version fields, text values, translations and timestamps
are declarations without normalization, native key authority or trustworthy time.
Current signed Manifest/Root/deny/FullPack/native-byte guards precede signed PE
and selected version-resource composition, using the same owned bytes and exact
identity equality. Every late error/cancellation returns literal zero, discarding
earlier signed/native/resource/record facts. No approved app/version/architecture/
publisher mapping, NSIS installer validity, Artifact admission or installation
authority is added. Logical copy/count limits and repeated tree/hash work are
not aggregate heap/work or selected production preparation budgets.

upgrade-pe-inspect is a standalone unsigned physical-observation command with
five explicitly positive capacities and one JSON result:

```sh
go run ./cmd/upgrade-pe-inspect \
  --source-bytes 16777216 --snapshot-memory-bytes 16777216 \
  --optional-header-bytes 4096 --sections 96 --certificates 8 fixture.exe
```

These are fixture example capacities, not production defaults. Exit 0 means
complete JSON/help, 2 invalid arguments, 1 source/inspection/context/output
failure. Fixed diagnostics avoid private argument/source/writer details; sticky
short/invalid writes fail. A failed output device may leave truncated bytes, so
callers must require exit 0 and complete JSON. Existing sourcefile host/symlink/
remote-mount limitations and arbitrary concurrent same-size-write caveat remain.
The command does not invoke current signed composition or execute its input.

Five new fixed private files extend sixteen fixtures to twenty-one. Original
Go-built PE bytes are 5933568 bytes, SHA-256:
4312ec15cca353f54c259ffdad7c015ca6be56826e632c96a86cc852fe34faab;
the original system PE is 360448 bytes, SHA-256:
49f096cbf9337b0a80bde835d29be41bc9371057c4ff6c72f8a36158c29cfa3a.
Its original Resource directory is 123344 bytes; the complete version resource
is 884 bytes, SHA-256:
3f16cbab55f8bb20ceadf8fd3802ae7ea05a5866176c65e11ecb835ab1e406a6;
the inner record is 880 bytes, SHA-256:
d1543f915d5efbd6e0728c74abe97db54f3151e7a2210f1fd86d005679ed0362,
followed by four opaque original zero bytes. Independent Python struct/hashlib
checks all original PE spans, 39 resource directories/69 entries/31 leaves,
exact [16,1,1033] resource at file offset 351512 and all 13 version blocks.
Windows version.dll data APIs independently compare only the 52-byte fixed
value; MUI/native data behavior is not whole-record/raw-byte qualification.
Go debug/pe independently checks physical section and complete CLI projections.
This batch reads both original images as data without executing them. Signed nsis/app/version
claims around them are isolated synthetic byte-binding fixtures, not publisher
signatures or proof that either is an approved NSIS/Pinvou installer.

Primary self-review and both independent scoped line-level code/test audits
passed. Root passed 17 gates: six own inspector builds, affected tests, isolated
actual PG regression, whole default, modules/full vet/server build/V1 pins,
license, formatting and two diffs. Affected tests passed 20
packages, 236 top-level/341 child cases, zero failures
(90.334s), with exactly one unavailable actual-AR tool skip.
All 51 required named groups passed: prior 37, thirteen new fixture/binary opt-in
groups and one mandatory original-source PE CLI control. PE built E2E passed
six mandatory children: synthetic success, invalid arguments/framing, same-path
restoration and both fixed originals. Prior ELF five, Debian seven and supported
codec three projections also passed. Physical/late certificate/resource/block/
capacity/ordering/ambiguous backing/RVA-boundary refusals, frozen selector/source
isolation, independently calibrated final cancellation, failed writers and
33 MiB positives passed. Test time allowances do not limit production downloads.

Root whole default passed 69 packages,
651 top-level/1364 child cases (195.639s),
with 615 top-level/606 child opt-in/unavailable-host
skips and 12 no-test packages separately disclosed. The unchanged plain-member/
current-trust PG regression passed 1/4, zero skips/failures (27.92s),
not new PE/database qualification. Each independent reviewer passed 14 own gates,
six builds and the same affected counts/51 required groups/one AR skip, PG 1/4
zero skips/failures, modules, affected vet, license, fmt and diffs. Dependency/
compatibility affected walls were 64.199s/
72.053s; PG intervals
27.435s/30.292s.
PG intervals include pre-inventory through Go completion, excluding post-inventory
and freeze checks; other command walls exclude freeze checks. Inventory retained
0 existing fixture databases and
1 existing role with no new resources left.
Reviewers did not repeat root whole default. All stage children/sessions closed.
Full source/index/status, 41 protected prior paths, 69 SQL raw/index identities,
four official SBOM resources, twenty-one fixtures and prior module/license/repair
proofs stayed fixed during actuals. Actual AR and sourcefile symlink/mapped-drive
host fixtures remain unverified.

No dependency/SQL/grant/route/provider/UI/contract/product-version change. XZ
maintenance remains owner-pending; XZ/Zstandard candidates are absent. Full T09
remains in progress; totals stay 5 implemented/12 in progress/30 incomplete or
deferred of 47. Product 0.10.9, normative 1.0, pinned V1 and CodeWhale remain
unchanged; deployment deferred. Downloads have no hard total/cumulative duration
or retry cap. The 32 MiB bound is joined atomic inventory only.

DCO committed and pushed as `1adce90a4f144fe67385058e724a3a2c6b772060` on
`codex/t09-pe`; scope/evidence is in `docs/artifact-pe-integration.md`.

### T09 bounded Zstandard and Debian stored-member integration

Five new adapter/test/license paths integrate source commits
78d1eac1f2308957efcbb8dc051a5d402d832e3d and
a7c5844656f5f80df5bc243791b6a7459418dd48, with six narrow current-tree changes:
go.mod/go.sum, generic compression error classification, Debian window/dispatch,
isolated mislabeled-codec refusal and the appended current signed composition.
Separate original/final proofs retain 5 source-exact new paths,
0 formatting-only new paths and all six existing-path adaptations.
Historical combined-tree documentation (111 added lines) and all XZ code remain
omitted. Existing XZ Unsupported negatives and all earlier implementation remain.

The only new direct dependency is github.com/klauspost/compress v1.20.3,
the official latest stable when checked, requiring Go1.25 and no transitive
modules. Official proxy/checksum-database receipts, complete 474-file cached
module versus official ZIP/Go dirhash, both sums and complete upstream LICENSE
and internal xxhash MIT license were verified and retained. No SDK patch/fork.
The existing github.com/ulikunitz/xz v0.5.17 remains LZMA-only here.

DecodeZstandard consumes complete original frames/concatenations/skippable frames
through EOF, enabling checksums, concurrency1, low-memory mode and disabling
DecodeAll shortcut. No external dictionary is supplied. Empty raw input refuses;
valid empty/skippable frames remain supported. Stored, expanded source/copy and
explicit window limits are separate per member. The full pinned history ring
window+2*(min(window,128KiB)+16) must fit host int before conversion. Streaming
MaxWindow and MaxMemory both bound the window, including SingleSegment content
size; they do not bound all heap/work or choose production preparation budgets.
Known SDK window/decoder-size errors conservatively classify as Limit, not proof
that caller capacity alone caused failure; missing dictionaries are Unsupported,
other codec errors fixed invalid. The shared private helper preserves only exact
Limit/Unsupported classifications. Every failure returns nil output; Debian and
current signed composition discard all earlier framing/control/signed facts.

Before repository mutation, a neutral offline private module compared 2,372
cases against independent Python3.14/libzstd: all four fixed originals, every
truncated prefix, sixteen skippable magics, concatenations, header/block/checksum
mutations, trailing bytes, dictionaries and explicit window/copy/cancellation
controls. Of 31 independently legal inputs, 24 matched adapter hashes/sizes and
seven explicit policy refusals retained nil output. All 2,341 independently
invalid inputs refused. The Go child exited 0; the recorded 70.345s
evaluation interval includes Go execution, result comparison and frozen-source
verification.
Its initial harness exited 1 because four zero-length prefix aliases were
misclassified as SDK overacceptance. The complete red record is retained. A
report-only correction applied the already-known empty-input SDK exception
by actual input length; all five empty aliases were refused by the adapter.
No runtime result changed, no rerun was substituted, and no nonempty SDK
overacceptance or adapter mismatch remained. Both independent reviews accepted
this correction before import.

Five additional private fixed files extend twenty-one to twenty-six fixtures:
945/63/13/1075-byte independent Zstandard frames and a synthetic 548-byte Debian
package (SHA-256 5956a14a81aeb0e58bfbfd2ccdb4da06c60e8ed112ae0eccd12265311e6834ba).
Its exact control.tar.zst/data.tar.zst are 197/157 bytes, each expanding to the
same previously independent 10,240-byte TAR identity. Python/libzstd/hashlib and
AR arithmetic rechecked all bytes without extraction or execution. The 33 MiB
expanded positive is independent of the 32 MiB joined atomic inventory limit.
Synthetic signed claims bind bytes, not native app/version/architecture/publisher
qualification, Artifact admission or installation authority.

Primary self-review and both independent scoped line-level audits passed.
ROOT passed 17 gates: six own inspector builds, affected tests, unchanged actual
PG regression, whole default, modules/full vet/server build/V1 pins, both module
license checks, formatting and two diffs. Affected tests passed 20
packages, 245 top-level/341 child cases (75.54s),
zero failures and exactly one unavailable actual-AR tool skip. All 53 required
named groups passed, including the two new actual Debian/Zstandard API and
current signed compositions; PE six/ELF five/Debian seven built E2E children and
the existing three-codec CLI report matrix passed. Whole default passed
69 packages, 658 top-level/1364 child cases
(196.142s), zero failures, 617 top-level/606 child
opt-in/unavailable-host skips and 12 no-test packages separately disclosed.
PG passed 1/4, zero skips/failures (32.13s), preserving
0 existing fixture databases and
1 existing role. This is the unchanged plain-member
current-trust PG regression, not new Zstandard/database qualification.

Each reviewer passed 14 own serial gates, six builds, the same affected counts/
53 named groups/one AR skip, PG1/4 with zero skips/failures, modules, affected vet,
both license checks, format and diffs. Dependency/compatibility affected times
were 66.773s/67.447s;
PG intervals 25.851s/25.576s.
PG intervals include pre-inventory through Go completion, excluding post-inventory
and freeze checks. Reviewers did not repeat whole default. All children/sessions
closed. Source/index/status, 65 unchanged protected paths, six changed-path
original/final proofs and 69 existing SQL raw/index identities,
four official SBOM resources, twenty-six fixtures and earlier repair/module
proofs stayed fixed during actuals. Actual AR and sourcefile symlink/mapped-drive
host fixtures remain unverified.

The pinned SDK has generic diagnostics on some malformed-input paths, including
truncated frame-content-size fields; complete stderr silence is not claimed,
and global stderr is untouched.
The existing Debian CLI has no explicit Zstandard window option: its 22-capacity
interface and three-codec positive matrix are unchanged. This batch adds internal
API/current signed composition, not positive CLI Zstandard support. Later CLI
integration must address compatibility and diagnostic behavior separately.
No XZ implementation, SQL/grant/route/provider/UI/contract/product-version change.
XZ dependency maintenance remains owner-pending. Full T09 stays in progress;
totals remain 5 implemented/12 in progress/30 incomplete or deferred of 47.
Product 0.10.9, normative 1.0, pinned V1/CodeWhale stay fixed; deployment deferred.
Downloads have no hard total/cumulative duration or retry cap. The 32 MiB bound
applies only to joined atomic inventory, never artifact or expanded-output size.

DCO committed and pushed as `05dd49638f9b09956efc8ca320785fe1b53eeba2` on
`codex/t09-zstandard`; evidence: `docs/artifact-zstandard-integration.md`.

### T09 original Mach-O byte inspection

Eight new primary source/test files add thin Mach-O64 little-endian and ordinary
big-endian FAT_MAGIC (32-bit architecture table) physical observations, a complete
standalone unsigned inspection CLI and current signed helper/launcher composition.
No historical code/documentation import or new dependency is involved. Existing
seventy-seven protected paths and sixty-nine SQL raw/index identities remain fixed.

The parser bounds every fat table/slice, rejects table/slice aliases, slice overlap
and misalignment, and binds original CPU/subtype fields to the exact thin slice.
Commands consume the complete declared table with exact counts/sizes/alignment;
LC_SEGMENT_64 records preserve raw 16-byte names, backed segment/section bytes and
complete 8-byte relocation records. Zero-fill types 1/12/18 have no file payload.
Empty unused offsets remain declarations; valid segment/header/section aliases
remain supported. Unknown commands retain complete opaque identities; external
references, loader/dyld/symbol/encryption/code-signature semantics are unverified.
Whole original source identities include overlays. Other recognized class/endian/
fat64 profiles explicitly refuse Unsupported; empty fat is Invalid. Zero-command
thin images, duplicate CPU slices and mixed declared filetypes are observations,
not architecture/loader qualification policy.

All caller capacities are positive: source bytes, architecture count, per-image
command count and per-image total sections across segments. They do not establish
aggregate work/heap or production preparation budgets. Immutable snapshots, safe
span arithmetic and context-polled hashing retain original bytes; every failure,
including late second-slice and final cancellation, returns a literal zero report.
The CLI requires five capacity flags including snapshot memory, gives fixed
errors, handles short/sticky writers and same-path original restoration, and
executes no inspected image. InspectMachONative accepts only explicit helper,
helperArtifact or launcher roles through existing current package/native checks,
requires the corresponding signed format and exact owned identity, and discards
all earlier facts on failure. Complete package semantics already require role/
format binding; the local guard preserves this boundary explicitly. finalInstaller
refuses before protected source access: the frozen macOS installer enum is DMG,
not a new macho enum. Synthetic claims certify neither helper behavior nor trust.

Before integration, pinned Go1.27.2 cross-built a private minimal Go program for
darwin/amd64 and darwin/arm64 with CGO disabled (21.110s/18.522s). The complete
originals are 1,210,448 and 1,178,770 bytes. A clearly synthetic 2,395,282-byte fat
wrapper contains both byte-exact originals at aligned offsets. Fixed source/tool
hashes and build provenance remain private. Python struct/hashlib and Go debug/macho
independently agreed with every report field and original span hash: Intel 13/15,
ARM 14/15 and combined 27/30 commands/sections. No target image was executed.
The neutral initial run retained one failing test expectation: XOR of the magic
selected a recognized unsupported 32-bit profile. Changing only the negative magic
to an invalid value yielded 7 top-level/31 child passes, no production change.
The complete initial red result remains retained; separate recognized-profile
negatives remain. Both independent prerequisite reviews passed before integration.

Primary self-review and both independent line-level audits passed after adding a
decisive two-segment cumulative section-capacity test: each segment fits alone,
total cap 2 succeeds, cap 1 returns Limit with zero report, and original restoration
matches. Import grouping and explanatory comments were the other primary cleanup.
The first repository actual run retained one failing test expectation: a request
cancelled before package inspection correctly returned the structured archive
cancelled failure, while the new test expected the later capture.Cancelled. Zero
facts were already confirmed. Primary asserted its exact failure type, code and
original entry, retaining zero facts. An intermediate nonexistent-sentinel assertion was caught in static review
and corrected before any compilation or execution. Both independent reviews
passed. All initial logs and the red summary remain retained.
The final acceptance below is a fresh complete run, not mixed initial results.
ROOT passed 18 serial gates including seven own inspector builds, affected actuals,
unchanged actual PG, whole default, modules/full vet/server build/V1 pins, full
licenses for both existing dependency modules, formatting and diffs. Affected tests passed
22 packages, 266 top-level/394 child
cases (66.394s), zero failures and one unavailable actual-AR tool skip.
All 58 required named groups passed, including all five Mach-O groups. Seven
Mach-O built E2E children, prior PE 6/ELF 5/Debian 7 children and the three-codec CLI
matrix passed. Whole default passed 71 packages,
675 top-level/1395 child cases (196.839s),
zero failures, 621 top-level/606 child opt-in or
unavailable-host skips and 12 no-test packages separately disclosed.
PG passed 1/4 with zero skips/failures (26.648s), preserving
0 preexisting fixture databases and
1 role. This unchanged plain-member current-trust
regression is not Mach-O/database qualification.

Each independent reviewer passed 15 own serial gates and seven builds, the same
affected counts/58 groups/one AR skip and PG 1/4 without skips/failures, modules,
affected vet, existing licenses, format and diffs. Dependency/compatibility affected
intervals were 122.752s/65.02s,
PG intervals 26.423s/23.082s.
PG intervals include pre-inventory through Go completion, excluding post-inventory
and freeze checks. Reviewers did not repeat whole default. All stage children and
sessions closed; source/index/status, 77 protected paths, 69 SQL, 4 official SBOM
resources and 29 fixed fixtures stayed frozen throughout actuals. Actual AR and
sourcefile symlink/mapped-drive host fixtures remain unverified.

This batch establishes original physical byte observations, not Apple codesign,
notarization, native loading, approved CPU coverage, Pinvou publisher/helper
qualification, DMG acceptance, Artifact admission or installation authority.
No SQL, grant, route, provider, UI, frozen contract or product-version change.
XZ maintenance remains owner-pending; this batch does not implement XZ.
T09 remains in progress; totals stay 5 implemented/12 in progress/30 incomplete
or deferred of 47. Product 0.10.9, normative 1.0, pinned V1/CodeWhale stay fixed.
Deployment remains deferred. Downloads have no hard total/cumulative duration or
retry cap. The 32 MiB bound applies only to joined atomic business inventory.

DCO committed and pushed as `7602824a794fc3c3557e4fdd204a4950da792809` on
`codex/t09-macho`; evidence: `docs/artifact-macho-integration.md`.

### T09 complete Debian Zstandard inspection CLI

One narrow existing command change and one dedicated new test add explicit
Zstandard window capacity to upgrade-debian-inspect. The existing main_test.go,
all 22 required legacy capacity flags and the prior gzip/bzip2/LZMA CLI matrix
remain byte/behavior compatible. A related correction retracts the earlier SDK
diagnostic claim in its prior integration document. No historical source import
or new module.

--zstandard-window-bytes is optional for legacy input. Explicit occurrence requires
a positive int64 before source access; zero/negative values return fixed capacity
errors, nonnumbers/overflow return fixed argument errors. Omitting it preserves
legacy calls and supplies no implicit Zstandard window. Zstandard input still
requires a supported explicit bound through the existing decoder; positive values
outside that profile are runtime Invalid, not evidence of a capacity violation.
Supported smaller windows can produce exact Limit. The window is independently
applied to each member, not aggregate memory/work or a production default.
The help text describes this conditional argument. All original required flags
stay required, including the legacy LZMA capacity; no defaults were invented.

The complete original stored control/data frames, expanded TARs and exact control
record are observed through the existing immutable pipeline. XZ remains
Unsupported; format names never select a fallback decoder. Invalid framing,
control/data codec faults, missing window and late window limits produce zero
stdout; same-path restoration must match the complete independently frozen JSON.
The command neither installs nor extracts, authenticates a publisher, approves
application/architecture declarations nor admits an Artifact.

Two new fixed synthetic DEBs extend 29 to 31 fixtures. The 548-byte larger-window
input (SHA-256 f26c9d08c0f430cc213832cccf9e5eeedcbb27406b088155b9aa5a7483eb66c8)
differs from the previous 548-byte original at only data's window descriptor:
1024 to 2048. Control remains byte-exact at 1024. The 1024 refusal therefore occurs
after valid control decoding; 2048 gives the complete original projection. The
568-byte skippable input (SHA-256 af1c8e966e0c0392441c512fe012445ad729572038884386976a68528e6ba0c8)
prefixes each member with 11 exact bytes: a skippable header and 3-byte payload;
AR sizes/padding are independently rebuilt. Python/libzstd, struct/hashlib and
in-memory tarfile confirm both original 10240-byte TAR identities and 109-byte
control record. Both prerequisite reviews verified exact bytes before integration.
These are synthetic codec/CLI controls, not original distribution packages.

The first repository actual run retained one failing test expectation: the legal
skippable CLI succeeded, but emitted no process stderr instead of the four lines
expected after static review. Both the initial two-line and later four-line
assumptions incorrectly treated package-scoped println as Go's built-in printer.
The pinned SDK's own println/printf definitions log only when debug flags are
enabled; debug/debugDecoder/debugEncoder are all false in the unchanged module.
A separate probe using the same owned CLI confirmed legal skip exit 0/empty stderr
and malformed truncated-content-size exit 1/zero stdout/only the fixed 34-byte CLI
invalid diagnostic. Both probe children joined; no SDK or production repair.

Primary corrected only test expectations and added truncated-content-size cases
to direct and built refusals. Successful built originals now require exactly empty
process stderr and full independently measured JSON. Malformed fixed controls
require exactly the CLI diagnostic and no stdout. No stderr redirection or relaxed
matching. A narrow correction in the prior Zstandard integration document retracts
its earlier generic-SDK-stderr assertion. This correction supersedes the earlier chronological
progress statement; its historical entry remains intact. These concrete observations
do not promise global silence for every possible failure. Initial actual red logs,
source/guard proofs and the intermediate static assumptions remain retained. Final
acceptance below is a fresh complete run after both reviewers rechecked the actual
SDK function definitions; no initial green results are mixed into it.

Primary self-review and both independent line-level code/test audits passed.
ROOT passed 18 serial gates with seven own inspector builds, affected actuals,
unchanged actual PG, whole default, modules/full vet/server build/V1 pins, full
licenses for both existing dependency modules, formatting and two diffs.
Affected tests passed 22 packages,
270 top-level/407 child cases (58.509s),
zero failures and exactly one unavailable actual-AR tool skip. All 61 required
named groups passed, including three new Zstandard CLI groups. Seven new built
children and three complete new reports passed alongside unchanged Mach-O 7,
PE 6/ELF 5/Debian 7 built children and the prior three-codec report matrix.
Whole default passed 71 packages,
676 top-level/1395 child cases (180.798s),
zero failures, 624 top-level/606 child opt-in or
unavailable-host skips and 12 no-test packages separately disclosed.
PG passed 1/4 with zero skips/failures (22.938s), preserving
0 preexisting fixture databases and
1 role. This is the unchanged plain-member current
trust regression, not Zstandard/database or publication qualification.

Each independent reviewer passed 15 own serial gates and seven builds, the same
affected counts/61 groups/one AR skip, new and old E2E matrices, PG 1/4 without
skips/failures, modules, affected vet, existing licenses, format and diffs.
Dependency/compatibility affected intervals were
53.492s/56.151s,
PG intervals 21.741s/21.375s.
PG intervals include pre-inventory through Go completion, excluding post-inventory
and freeze checks. Reviewers did not repeat whole default. All sessions/stage
children closed; source/index/status, 84 protected paths, 69 SQL raw/index,
both changed-path original index/raw and current code/document proofs,
4 official SBOM resources and 31 fixtures stayed fixed throughout actuals.
Actual AR and sourcefile symlink/mapped-drive host fixtures remain unverified.

No SQL, grant, route, provider, UI, frozen contract or product-version change.
XZ dependency maintenance remains owner-pending and unimplemented. T09 remains
in progress; totals stay 5 implemented/12 in progress/30 incomplete or deferred
of 47. Product 0.10.9, normative 1.0, pinned V1/CodeWhale stay fixed; deployment
remains deferred. Downloads have no hard total/cumulative duration or retry cap.
The 32 MiB bound applies only to joined atomic business inventory, never artifacts,
expanded output or these explicit inspection capacities.

DCO committed and pushed as `41c920166ef251838c4b49cd43dce8cdda733a5b` on
`codex/t09-zstandard-cli`; evidence: `docs/artifact-zstandard-cli-integration.md`.

### T47 refuse lifecycle writes without final business owners

An independent whole-card audit identified an incomplete T47 guard obligation.
The original canonical intent, T05 approval, T06 owner epoch and caller watermark
do not authenticate purpose-specific actual parent/baseline, quality, freeze,
observation or atomic publication state. Those fixed final owners are not yet
installed. Runtime SQL permissions and absent business routes remain valuable,
but cannot substitute for the kernel's own missing-owner refusal.

All seven existing exported lifecycle writer methods now return the existing
Unavailable error and zero result before receiver/transaction access. Exported
OperationWriteSet Apply/Seal do the same, including nil, zero, nonzero and copied
handles. Existing signatures/type remain available. No public dispatcher,
always-refusing operations.Command, enabling flag, callback registry, fake parent
or blanket approval was added. Plan/SN draft/freeze and read workflows stay usable.

The implementations in four existing production files are retained as private
storage primitives. Mechanical comparison after the planned name/type redirects
confirms unchanged body logic. Existing storage, recovery, clock, source, CAS,
capacity and concurrency controls still exercise these private primitives. Six
cross-feature quality test calls use an explicitly test-only schema-owner fixture
instead of a public writer. They establish local source facts, never actual parent
or business qualification. No production quality logic was changed.

Three new actual PostgreSQL groups collectively cover the missing-owner boundary.
Public-writer and complete-candidate Seal groups each exercise both null-SN and
frozen-SN paths; the uninstalled-purpose routing group uses a null-SN fixture.
Public-writer refusals use real frozen plans, approved original Start intent and
processing T06 root/receipt fixtures. Transactions commit after refusals, with
full protected-owner row hashes before/after and transaction-local write deltas,
so rollback cannot hide a public write. Closed uninstalled business purposes
leave no operation; this checks routing only, not final-owner authentication.
For Seal, a complete private child/outbox candidate first seals successfully and
rolls back as a positive control. Rebuilding the same complete candidate and
calling public Seal/nonzero/copied handles refuses without row/write changes.
Diagnostic raw candidates are rolled back and the original owner hashes restored.

The first focused run retained a failing test assertion: an absolute transaction
write counter was not zero after private fixture preparation. Public calls still
returned exact Unavailable/zero. Primary corrected the assertion to compare the
counter delta after that preparation, retained committed full-owner row checks,
and added the complete-candidate Seal positive/refusal control requested during
independent review. No production behavior was loosened. Initial red logs and
source/inventory proofs remain private. Revised focused actuals passed 3 top-level
and 34 child cases with zero skips/failures (36.213s).

Primary self-review and both independent scoped line-level audits passed. ROOT
then passed nine serial gates against the frozen revised tree. Full rollout and
quality actuals passed six packages, 129 top-level/173 child,
zero skips/failures (879.322s), using actual PostgreSQL/OpenBao
and the four original Node reference projections. Full default service regression
passed 71 tested packages, 678 top-level/1395 child,
zero failures (174.683s). Its 625 top-level/
610 child opt-in or host skips and 12 no-test packages are
separately disclosed; default skips do not stand in for the actual feature run.
Modules, full vet, private server build, original V1 pins, formatting and both
whitespace checks also passed.

Each independent reviewer then owned a separate serial six-gate run: actual
affected feature paths, modules, affected vet, format and both whitespace checks.
Dependency passed 84 top-level/106 child (438.643s);
compatibility passed 84 top-level/106 child (452.106s).
Both passed six packages with zero skips/failures, all three new groups and the
selected unchanged original positive/fault/recovery/reference/quality controls.
Their selectors exclude unchanged bulk capacity/fanout/permanent-history stress;
ROOT's full feature actual includes it. Reviewers did not repeat whole default.
Every stage/session and owned native child closed before slot handoff. Source,
index and status stayed frozen throughout actuals; all 69 committed SQL files
retain exact raw and index bytes. PostgreSQL inventories before/after each actual
were identical, including 1 preexisting fixture database(s)
and 2 role(s). These are preserved baselines, not newly leaked
fixtures or a claim that the baseline inventory was empty.

Two prior kernel documents carry explicit current-entry-point notes; their earlier
private mechanism acceptance does not imply complete final business guards.
No SQL, grant, route, provider, UI, dependency, frozen contract or version change.
T47 remains in progress: the next integration must install real fixed final
owners and authenticate their actual state in the original atomic operation.
This repair does not complete parent publication or authorize a running Rollout.
Totals remain 5 implemented/12 in progress/30 incomplete or deferred of 47.
Product 0.10.9, normative 1.0 and pinned V1/CodeWhale remain fixed. Deployment stays
deferred. Downloads have no hard total/cumulative duration or retry cap; the
32 MiB bound applies only to joined atomic business inventory.

DCO committed and pushed as `1cbf50297b4b57e517159c782fdd416f7672058b` on
`codex/t47-missing-owner-fences`; evidence: `docs/rollout-missing-owner-hardening.md`.

## 2026-10-18: T14 protected configuration-draft authoring

Delivered the separate service's complete bounded configuration-authoring workflow:
original V1 create, exact current-head edit, protected point/list/history queries,
permanent minimal original-result locator, and Chinese/English/Japanese UI.
T14 is now in progress; this increment does not qualify declared Release/Target
references, publish parents, submit, schedule, activate, switch the first baseline
or create runnable Rollout children. These remain dependencies of the full task.
Totals are five implemented, thirteen in progress and twenty-nine incomplete or
deferred out of 47. No other task is promoted by this batch.

Every mutation uses actual T05 current authority and the original T06 owner,
closed native intent/authentication and exact source CAS. The atomic write set
includes the owned head/history/result, protected audit/outbox and T06 result.
Unattended recovery never renews its retained proof; the active composition merges actual
event and draft registrations into one worker, detects missing processing owners,
and joins owned work before pool shutdown. Disabling draft admission retains
queries and recovery. Schema provisioning is an explicit owner-only utility.

The browser retains exact original wire/key/public SHA before sending, separates
fresh credentials, preserves uncertain results, and keeps the latest explicit
refusal diagnostic before releasing its exact key. Corrupt or SHA-invalid local
diagnostics refuse new work; late responses across actor/scope changes cannot
release a later request. Current/historical references stay distinct, and current
grants can refresh without reloading the page. Local session storage is plaintext,
not cross-user confidentiality. No generic command dispatch or default account.

The first complete focused run passed both actual Go groups (browser and CLI),
zero skips/failures, in 46.282s; inner Playwright passed both tests in 23.9s.
After a final frontend self-review unified same-actor identity refresh in App,
the new build and all three actual groups (new draft browser, existing management
browser and CLI) passed again, zero skips/failures, in 61.842s. The browser also
checks the access page after live permission refresh; all Go/SQL/protocol bytes
remain identical to the full-regression backend.
It covers actual HTTPS, signed generated OIDC, protected provisioning, UI edit
content, original-response loss/reload recovery, read-only/actor/scope isolation,
live grant refresh, bounded list/history, malformed and SHA-corrupt storage,
and settlement of a delayed actual fetched Response before comparing a new
pending request and retrying it. This adapter seam is not physical network ACK
qualification. CLI acceptance proves exact idempotent initialize/runtime check;
fresh DDL is tested through the native initializer. A separate actual sealed
event/interrupted-draft shared worker test retains original proofs, advances
consistent epochs and writes each original result once.

Final affected actual regression: 12 tested packages, 125 top-level/436 child passes, 2 top-level/0 child skips, 0/0 failures (363.854s).
Final cleared-opt-in default regression: 74 tested packages, 721 top-level/1701 child passes, 654 top-level/651 child skips, 0/0 failures (329.783s);
13 no-test packages. The two affected-suite skips are the deployed Keycloak acceptance opt-in and
parent-only crash-process helper. Default opt-in/host skips do not stand in for
real feature acceptance. Module verification, complete vet, private
server build, original V1 pin, formatting and both whitespace gates passed.
All source/index/status and 69 previously committed SQL files stayed frozen
through native runs; the preexisting generated PostgreSQL inventory remained
1 database/2 roles before and after. Earlier red, timed-out and interrupted
harness evidence was retained privately and is not counted as accepted work.

Both independent roles performed scoped line-by-line static code/test/document
reviews; the primary repaired findings and the roles rechecked to PASS.
Evidence: UpdateServer `docs/deployment-drafts.md`.
Service DCO commit `bcb1cc8de90ce39d74684ef24d0b337e5c37efb9` pushed on `codex/t14-deployment-drafts`.
Product 0.10.12, normative 1.0 and pinned V1 source revision remain unchanged.
CodeWhale stays `c08c8c022a31aa4bd4a5a146887690de99495786`; no fork behavior change.
Deployment remains deferred. Downloads retain no hard total/cumulative duration
or retry cap; 32 MiB is only the joined atomic business-inventory bound.


### T11: protected multi-platform Release drafts (2026-10-18)

Service commit `7c49fac1b88550c449a420623548490ae5f01131` is committed and pushed on
`codex/t11-release-drafts`. Actual multi-target create/edit, authenticated
current/history queries, original-result reconciliation, explicit restricted
initialization and the three-language management workflow are delivered. Only
draft state exists; Package/certification references remain declarations.
Assembly, qualification, target review/approval/revocation, Release closure and
cancellation, and publication remain open. T11 remains in progress.

Current protected T05 rights cover the product/component parent and every target
in the original old/new union. Native audit authentication, original CAS/proof,
session and owner fencing guard the atomic history/result/audit/outbox/T06 commit.
The permanent minimal locator does not reopen expired full acknowledgements.
Original browser wire/key/digest survive unknown responses and scope changes;
unavailable storage blocks transmission. Shared utility extraction preserves the
existing Deployment namespaces and behavior.

Broad actual native coverage passed 42 groups/137 children; later runtime and
HTTP/CLI composition passed 77 groups/147 children with three opt-in skips.
Subsequent actual HTTPS, CLI, admission-off recovery, prior Deployment browser/CLI
and the strengthened write-only original-CAS native case passed. The first new
browser run failed on a nested label. After primary repairs and stronger actual
JSON-settlement observation, all four Release browser cases passed (37.785-second
targeted Go invocation), followed by the existing management browser (30.188
seconds). Both independent scoped code reviews passed the repaired sources.

Final defaults with integration opt-ins cleared passed 751 groups and
1730 children, with 1333 skipped cases and zero failures. Module verification,
full vet, private server build, original pinned artifact comparison, affected-source
formatting and whitespace checks passed. An untouched historical Rollout
boundaries test retains its original formatting issue; whole-tree formatting is
not claimed clean. Defaults are separate from actual integration acceptance.
All 76 previously committed SQL files stayed unchanged;
owned native fixtures restored their original inventory. Generated signed OIDC
and response-adapter faults do not certify deployed IdP ceremonies, physical
networks, hosts or deployment.

Documents 1.0, product 0.10.12 and the CodeWhale gitlink remain unchanged. Downloads
have no hard total/cumulative duration or retry cap; 32 MiB still bounds only
joined atomic business inventory. No artifact/download size cap is introduced.


### T11: authenticated target-record migration (2026-10-19)

Service `37572f4970c60198f28805d4853f768c106b3b22` is DCO committed and pushed on
`codex/t11-release-drafts`. Explicit schema-owner migration authenticates original
V1 complete parent bytes and audit MACs before atomic V2 target backfill. Original
SQL/ABI, history, pending proofs and acknowledgements remain compatible. Runtime
startup never migrates; obsolete heads, wrong keys, coherent replacement sources
and missing installed children refuse without healing or partial migration writes.

Two independent scoped code reviews passed after primary repairs. Actual isolated
PostgreSQL tests cover the original V1 pending owner's epoch-2 recovery without
proof refresh, old committed replay, current editing, source rewind/rollback and
installed missing-child refusal/restoration. Actual CLI subprocess acceptance
covers initialize/check/migrate/check/idempotent migrate. Default regression
passed 763 top-level/1781 child tests, 1335 opt-in skips and zero failures; defaults
are separate from the actual acceptance. Modules, full vet, server build, pins,
affected formatting and whitespace passed. The unrelated unchanged rollout
formatting baseline remains out of scope. All 85 prior SQL files stayed intact;
owned database fixtures restored their original inventory.

The assembled-record model and logical approval binding are groundwork, not an
installed Assemble owner, approval preparation, HTTP route or UI. T11 remains in
progress; totals remain five implemented/fourteen in progress/twenty-eight
incomplete or deferred of 47. Product 0.10.12, documents 1.0 and CodeWhale remain
fixed. Deployment is deferred; downloads retain no hard total/cumulative duration
or retry cap, and 32 MiB applies only to joined atomic business inventory.

### T11: independently reviewed content freezing (2026-10-19)

Service `d5ac0200d64eb3e5ff7c0daddb08ca24a100cda1` is DCO committed and pushed
on `codex/t11-release-drafts`. The complete operator workflow prepares an exact
parent/complete-target content change, obtains one real independent T05 review,
and executes the frozen AssembleRelease command through authenticated HTTP and
the three-language admin. Reviewers cannot execute their own reviewed command,
even with full write permissions. Assembly freezes the original source refs and
content, advances the parent once and blocks subsequent editing. It does not
qualify declared packages/certificates or approve/publish Release Targets.

Explicit owner migration adds V3 after the authenticated V2 migration. All 88
prior committed SQL sources remain intact; startup does not migrate or heal data.
V3 keeps both assembly recovery owners registered when admission is disabled.
T06 optionally obtains the native final result and seals/completes it in the same
transaction as domain, audit and outbox writes, preserving legacy Apply owners.
Replay authenticates the original actual prepared change or assembled history
and keyed MAC. Permanent original commit locators remain available under current
read-only authority after encrypted response cleanup, without reopening an ACK.

Two independent scoped code reviews passed after primary repairs. Targeted
actual PostgreSQL coverage verifies final-result rollback/retry/fencing/lost ACK,
V3 migration, source-fault refusal/exact restoration, stale original CAS proof,
independent approval and read-only lookup after 24-hour cleanup. Subsequent V3,
HTTPS and actual CLI acceptance passed five groups with no skips or failures.
The new actual browser case passed all three languages, corrupt refusal storage
with zero writes, confirmed committed-response loss, exact original replay,
actual JSON settlement after a scope unmount, independent review and frozen
editing refusal. All four original Release draft browser cases also passed.
The first new browser run exposed a test locator mismatch; only that locator was
repaired and the new case rerun. Failed attempts are not counted as passing.

Final defaults passed 768 top-level/1781 child tests, with 1344 explicit opt-in
skips and zero failures; these are separate from actual acceptance. Modules,
full vet, private server build, pinned contracts, affected Go formatting and
whitespace passed. The unrelated existing rollout formatting baseline stayed
unchanged, and generated database fixtures restored their original inventory.

T11 remains in progress pending qualification and the remaining Release/Target
approval/lifecycle work. Totals remain five implemented/fourteen in progress/
twenty-eight incomplete or deferred of 47. Documents 1.0, product 0.10.12 and
CodeWhale stay fixed. Deployment is deferred. Downloads have no hard total or
cumulative duration/retry cap; 32 MiB still bounds only joined atomic inventory.

T07/T08 explicit public trust bootstrap tool (2026-10-19):

- Service commit `8c85d71` adds four fixed CLI actions: independently initialize
  the existing signing and metadata schemas, check the actual runtime login, and
  provision an independently obtained public initial Root body. Owner and runtime
  connections remain separate; no HTTP process provisions its own trust.
- Strict bounded public-body parsing rejects envelopes, unknown fields, duplicate
  keys and oversized input. The existing owner validates policy and validity with
  PostgreSQL time. Exact retries preserve trust; conflicting bodies or IDs and
  runtime provisioning are refused. Initialization does not create product
  catalogs, components, deny state or published Root material. All 93 previously
  committed SQL files are unchanged. No private key or deployment is involved.
- Two independent reviewers passed after the primary role repaired two negative
  test controls: expiry is tested before the first anchor with zero rows before
  and after refusal; duplicate keys are tested on an otherwise complete valid
  Root. The selected run passed two unit tests and one actual PostgreSQL 17/separate
  CLI process acceptance test, with no skip/failure; fixture database/role
  inventory was restored.
- Final repository checks passed: module verification, normal all-package tests
  (770 top-level and 1,781 nested passes; 681 top-level and 664 nested opt-in skips;
  zero failures), vet, service build, pinned-contract verification, affected-file
  formatting and whitespace checks. The opt-in skips are not actual acceptance.
  Applicable CLI acceptance was run separately; no browser behavior changed.
- T07/T08 remain in progress. Reviewed catalog initialization, external-public-Root
  genesis publication and component-registration HTTP/UI workflows are next;
  published metadata sources, independent private custody, complete issuance and
  deployment acceptance remain separate pending work. Totals remain 5/14/28 of 47.


T07/T08 reviewed trust initialization management (2026-10-19):

- Service commit `3a602c3` adds fixed private preparation, original-result lookup
  and three execution routes for catalog initialization, externally signed public
  Root publication and component registration. It composes the original T05/T06
  owners and the shared recovery worker, preserving their wire and SQL contracts.
  Admission defaults off; closing admission preserves existing recovery/replay.
- Preparation validates current actor, exact scope, MFA and keyed audit before and
  after actual source reads. Its permanent result binding authenticates the exact
  original request digest and change ID; coherent substitution by another genuine
  same-content change and orphan audit/result state fail closed without healing.
  Expired 24-hour full responses retain only the original minimal locator.
- The three-language admin retains separate unresolved action slots per tab and
  restores original public Root bytes. It requires two actual independent reviews,
  binds execution to the visible approved input, and preserves exact pending bytes
  across lost responses, reloads, scope changes and delayed response settlement.
  A pre-Root component preparation can retry its original wire after Root exists;
  a temporary source failure is not treated as a durable terminal refusal.
- Two independent scoped static reviews passed after primary repairs. Actual
  PostgreSQL 17/TLS/signed-OIDC acceptance passed all three final selected groups:
  HTTP workflow, coherent-cache/orphan fault refusal and full browser workflow.
  These groups had zero skips/failures. The browser verifies three languages,
  independent reviews, lost preparation/execution ACKs, wrong-slot zero-write
  refusal, edited-Root execution refusal and actual late JSON settlement after
  scope unmount. Existing draft/assembly browser regression also passed earlier
  in this batch. Initial new-browser locator failures were repaired and rerun;
  unsuccessful attempts are not counted as passing.
- The dedicated signed MFA fixture is explicit test composition, not certification
  of a deployed password/OTP identity provider. No production identity provider,
  private Root custody, metadata publisher or deployment was introduced. All 93
  prior SQL files are unchanged and owned database/role inventory was restored.
- Final checks passed module verification, default all-package tests (774 top-level
  and 1,781 nested passes; 684 top-level and 664 nested opt-in skips; zero failures),
  vet, server build, pinned contracts, affected formatting and whitespace. Default
  opt-in skips are distinct from the separately run actual acceptance.
- T07/T08 remain in progress; totals remain 5 implemented / 14 in progress / 28
  incomplete or deferred of 47. Next is reviewed non-Root online key management.
  Documents 1.0, product 0.10.12 and CodeWhale remain unchanged. Deployment is
  deferred. Downloads have no hard total/cumulative duration or retry cap; 32 MiB
  continues to bound only joined atomic inventory, never artifacts/downloads.


T07 approved online key-material management (2026-10-19):

- Service commit `d65992d` supplies the complete reviewed creation, actual provider
  readiness and separately reviewed inactive-registration workflow for timestamp,
  snapshot and target roles. Fixed private HTTP routes and the Chinese/English/
  Japanese admin compose the original owners, shared T06 recovery and a separate
  joined provider runner. No issuer is enabled and no metadata is published.
- Bounded keyset discovery never grants authority. Original committed T06 identity,
  receipt/outbox, purpose/provider, current T05/MFA and original SQL claim are
  checked. Unsupported genuine old-purpose work is retained/refused with zero
  provider RPC, without starving supported work. Schema/privilege dependencies
  are checked at readiness, before candidates and around RPC outside SQL.
- The actual provider token is protected local configuration, reread per RPC,
  separate from future signing credentials. The isolated dedicated-mount fixture
  uses a single-segment key policy; the same actual token is verified to refuse
  rotate, config, sign, export, backup and delete, while create/inspect succeeds.
  RAM-only development Transit is not production custody, HA or deployment proof.
- Public status requires both current read/key-management permissions and a live
  session, without the creator's original proof or fresh MFA. Another basic-login
  registrar can read ready material after creator revocation; registration still
  needs a separate exact approval with two independent reviewers and fresh MFA.
  Permission conjunction and exact post-principal session expiry are also covered
  by narrow T05 port tests, distinct from actual database acceptance.
- Admin recovery retains per-action original pending/prepared/accepted bytes and
  replays the original key with current verified authentication. Before execute
  POST it closes the complete source and verifies all hashes plus the original
  logical approval hash, including pending-only recovery. Three actual corruption
  controls retain the damaged bytes with zero execution POSTs, then restore the
  original wire and obtain the original registered acknowledgement.
- Read-capable operators preflight a new registration's actual ready selector
  before allocating a preparation key or pending request. Actual absent-ID status
  failure produces zero Prepare POSTs/no pending and keeps the form editable;
  restoring the real ID completes the same workflow. Core write permissions and
  existing unknown-request recovery remain unchanged.
- Two independent scoped static reviews passed after primary repairs. Three actual
  PostgreSQL 17/TLS/signed-OIDC/OpenBao HTTP/worker groups passed separately from
  the final full browser workflow, all with zero skips. They verify coherent
  accepted-ledger fault refusal/exact restoration, the original natural two-minute
  provider lease, same-name/version recovery after confirmed provider response
  loss, restart/admission-off replay and genuine unsupported-work refusal.
  The final browser also verifies three languages, independent reviews, lost
  execution responses, reload/original wire and the new registration preflight.
- All four existing deployment/release/assembly/trust browser groups passed.
  Initial test scope/approval/locator controls and a test-only declaration were
  repaired. One original Release-browser invocation was blocked before navigation
  by Chromium's restricted random port; its four cases passed on the subsequent
  isolated allocation. Failed attempts are not counted as passing acceptance.
- Final Go checks passed modules, defaults (777 top-level/1,781 nested passes;
  688 top-level/664 nested explicit opt-in skips; zero failures), vet, build,
  pinned contracts, affected formatting and whitespace. The last UI-only preflight
  change preserved all Go/SQL hashes and passed a fresh UI build/actual browser
  run. All 93 prior SQL files stayed intact; actual fixtures restored their original
  database/role inventory. Normal opt-in skips are not actual acceptance.
- T07 remains in progress; totals remain 5 implemented / 14 in progress / 28
  incomplete or deferred of 47. Online issuer enable/stop and Root retention
  composition follow. Documents 1.0, product 0.10.12 and CodeWhale are unchanged.
  Deployment stays deferred. Downloads retain no hard total/cumulative duration
  or retry cap; 32 MiB bounds only joined atomic inventory, never downloads.


T07 explicit issuer storage tooling (2026-10-19):

- Service commit `a93514f` adds fixed `upgrade-trust initialize-issuers` and
  `check-issuers` actions. Installation uses the explicit owner login/runtime
  role; checking uses only the runtime login and authenticates the actual
  metadata-source and issuer storage protections. It never repairs storage.
- Initialization reuses the original schema owner, including its explicit ACL
  normalization. Neither action registers providers, enables groups, installs
  issuance storage or asserts complete production signing readiness.
- Independent scoped static review passed. Separate-process CLI acceptance on
  actual PostgreSQL passed with zero skips: missing-schema check leaves no DDL,
  repeated install/check works, owner/runtime separation and migration/privilege
  drift refuse, exact restoration passes, and business/issuance state stays absent.
- Final modules/default tests (777 top-level and 1,781 nested passes; 688 top-level
  and 664 nested explicit opt-in skips; zero failures), vet, build and pinned
  contracts passed. A final formatting failure in one new file was normalized;
  affected formatting and whitespace then passed. All 93 prior SQL files stayed
  unchanged, and the actual fixture restored its database/role inventory.
- Totals remain 5 implemented / 14 in progress / 28 incomplete or deferred of 47.
  Online issuer controls and Root retention composition are still in progress.
  Documents 1.0, product 0.10.12 and CodeWhale are unchanged. Deployment remains
  deferred; downloads have no hard total/cumulative duration or retry cap.


T07 reviewed online signer controls (2026-10-19):

- Service commit `7024d26` supplies fixed private enable/stop management and
  Chinese/English/Japanese admin for timestamp, snapshot and target on exact
  product/component scopes. It reuses the original issuer command, approval
  Capture, permanent OwnerReplay and one shared T06 recovery worker. Registered
  material must have actual published Root authorization and threshold, actual
  current deny/fence/source facts and successful private signing probes.
- Sign credentials are separate protected renewable files, read lazily per RPC.
  Stop-only composition needs no provider. Current read status and ordinary stop
  require neither a valid Root nor an available provider; stop preserves the
  complete prior retention body except its state/revision. The same actual Store
  authenticates immutable retention facts in Root publication and its recovery.
- Fixed-purpose checks repeat at actual protected Receipt reads because worker
  recovery bypasses ingress Decode. The actual broader original owner creates a
  genuine release-role processing record. Its same-source Read, actual private
  probe and complete Apply succeed under an explicit rollback-only positive
  control; the new profile then refuses recovery with zero additional probes or
  issuer business writes while retaining original work. Claim advancement is
  permitted and explicitly observed, not described as rollback.
- Admin closes complete stored Capture/approval structures, verifies native
  actor/request size before retaining new work and verifies prior accepted
  records before allocating a new request. Independent enable/stop slots preserve
  original pending/prepared/accepted bytes. The native 64 KiB positive and one-byte
  overflow negative pass separately; damaged nested records remain blocked.
- Read-capable operators preflight actual Enable selectors and current Stop epoch
  before allocating a mutation key. The new fixed Enable preview has no command
  key, approval or supplied epoch; it reads actual PlanEnable facts under current
  read/key-management/session checks, makes no RPC and writes no approval, ledger
  or issuer state. Ordinary denial audit remains allowed. Its ephemeral candidate
  is neither returned nor saved. Formal preparation/execution always recheck.
- Actual preview acceptance preserves the threshold-two cardinality while
  replacing only one confirmed-absent ID, then restores both actual IDs. It proves
  zero new approval/operation/provider work. After a successful preview, actual
  Root policy removal makes the same formal preparation key refuse; restoring
  authorized policy lets that original key succeed. Preview is not a permit.
- Two scoped independent static reviews passed after primary repairs, including
  recovery purpose gating, nested-record rendering, native size check ordering
  and accepted-record checks. The actual PostgreSQL/TLS/signed-OIDC/Transit
  workflow observes a real signing-endpoint 403 for GET-only credentials, then
  two successful private probes through natural original-claim/shared-worker
  recovery. The same signing token refuses six lifecycle/export operations.
  Actual retained Root publication and expired-Root/unavailable-provider stopping
  pass; selection, stop and original ACK replay add zero provider requests.
- Final browser acceptance passes three languages, two actual independent
  reviewers, confirmed enable ACK followed by browser response loss, reload and
  original wire replay, malformed saved records without blank rendering, damaged
  accepted-record refusal with zero Prepare/new pending, absent material preview
  and wrong stop selector with zero Prepare/new pending, and actual stopped state
  matching the unique original database epoch and exact null dimensions.
  Both affected original key-material and trust-initialization browsers also pass.
- Earlier test attempts exposed an unused import, a nonexistent-column final
  browser assertion and insufficiently isolated negative controls. They were
  repaired and rerun; unsuccessful attempts are not counted as passing. Final
  default checks pass 779 top-level/1,781 nested tests, with 692 top-level/664
  nested explicit opt-in skips and zero failures. Modules, vet, build, pinned
  contracts, affected formatting and whitespace pass. The separate actual cases
  above pass with zero skips; all 93 prior SQL files are unchanged and fixtures
  restore their database/role inventory.
- T07/T08 remain in progress; totals remain 5 implemented / 14 in progress / 28
  incomplete or deferred of 47. Complete metadata/credential issuing owners and
  emergency business reconciliation remain pending. No production custody, live
  realm, HA, physical-client qualification or deployment is certified. Documents
  1.0, product 0.10.12 and CodeWhale are unchanged. Downloads have no hard total/
  cumulative duration or retry cap; 32 MiB bounds only joined atomic inventory.

T09 immutable local byte storage (2026-10-19):

- Service commit `c9040b8` adds an immutable blob port, actual local-filesystem
  adapter and explicit standalone `upgrade-artifacts` initialize/store/verify
  workflow. The absolute container must already exist. Initialization creates a
  fixed private child; runtime never initializes or normalizes permissions.
- Put consumes owned captured bytes, checks full size/hash, flushes file data,
  atomically creates the digest address without replacement and flushes directory
  publication/owned staging cleanup. Existing addresses are verified and their
  actual file data and directory are synchronized. Corrupt addresses are refused,
  never overwritten. Original-identity retry recovers an uncertain publication.
- Actual directory and file descriptors independently authenticate private
  ownership/permissions. Windows uses protected directory ACL creation, actual
  local NTFS/ReFS volume checks and same-object writable flush handles. Linux
  uses owner/private mode checks and file/directory fsync. Other OS storage is
  explicitly unsupported. Operators supply local Linux storage; the adapter does
  not attest remote-mount locality or certify physical durability.
- The read port exposes neither a path nor writing/deletion capability and stays
  usable after Store.Close. A returned reader cannot expose a later privileged
  file tail beyond its exact original identity. Memory capture and per-blob byte
  capacities are explicit and separate; actual 33 MiB storage passes without
  inheriting the 32 MiB atomic metadata inventory bound. No download duration or
  cumulative retry cap is introduced.
- Independent core and CLI/test reviews passed after primary repairs: authenticate
  each blob's own permissions, sync existing file data before durable Put success,
  and inspect the post-kill original object before retry could recreate it.
  Competing EEXIST publication shares final staging-cleanup/directory barriers.
- Final actual Windows acceptance passes 18 top-level and two nested cases with
  zero failures. Three top-level and one nested tests explicitly skip only the
  child fixture's direct invocation and unavailable source symlink/mapped-remote
  fixtures. Mandatory storage/CLI paths pass, including eight competing writers,
  directory/file ACL drift without repair, original-address corruption refusal,
  post-Link sync uncertainty, independent command restart/replay and short/lost
  command output. Owned processes are killed after actual Link and after complete
  Put before receipt; fresh owners first read the original bytes, then retry.
  All child processes are joined. This is process-death evidence, not power loss.
- Final default checks pass 793 top-level/1,783 nested tests with 694 top-level/665
  nested explicit opt-in/fixture skips and zero failures. Modules, vet, server and
  artifact-command builds, pinned contracts, affected formatting and whitespace
  pass. Linux x86_64/arm64 and macOS arm64 cross-compilation passes for affected
  packages; compilation is not native OS execution or certification. All 93
  existing SQL files are unchanged; no database/provider fixture is used here.
- T09 remains in progress. Stored bytes confer no Artifact validity, native or
  channel qualification, local-mode certification or publication right. Actual
  upload/admission/quarantine, protected published-reference retention, orphan
  collection, S3 durability proof and physical certification remain pending.
  Totals remain 5 implemented / 14 in progress / 28 incomplete or deferred of 47.
  Documents 1.0, product 0.10.12 and CodeWhale are unchanged; deployment is deferred.
