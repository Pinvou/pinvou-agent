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
