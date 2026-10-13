# Upgrade implementation progress

Task source: [V1.0 development tasks](pinvou-upgrade-platform-development-tasks.zh-CN.md).
Tasks are completed individually only after in-scope independent code review,
repairs, applicable tests and commit/push. Contract completion is not production
client, service, physical platform certification or rollout readiness.

| Task | Status | Delivery / outstanding dependency |
|---|---|---|
| T01 | Implemented; independently reviewed and verified | Executable metadata/credential Schemas, byte/trust profile, reference verifier, independently verified fixed vectors and local CLI E2E |
| T02 | Implemented; independently reviewed and verified | Closed control APIs, publication/lifecycle/selection/quality reference models and regression tests; no production service implementation |
| T03–T04 | Not completed | Platform/data scope, measured preparation budgets and trustworthy time evidence remain pending; follow the task graph |
| T05 | Implemented; independently reviewed and verified | Separate service management identity, MFA, scoped permissions, dual approval, protected audit and three-language admin; actual Keycloak realm acceptance remains pending |
| T06 | Implemented; independently reviewed and verified | Durable atomic executor, encrypted recovery projections, SQL lease fencing and background recovery; later business commands and production HA acceptance remain pending |
| T07–T47 (including final T43) | Not completed | Follow the task graph; no runtime or certification completion is claimed |

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
