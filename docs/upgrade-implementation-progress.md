# Upgrade implementation progress

Task source: [V1.0 development tasks](pinvou-upgrade-platform-development-tasks.zh-CN.md).
Tasks are completed individually only after in-scope independent code review,
repairs, applicable tests and commit/push. Contract completion is not production
client, service, physical platform certification or rollout readiness.

| Task | Status | Delivery / outstanding dependency |
|---|---|---|
| T01 | Implemented; independently reviewed and verified | Executable metadata/credential Schemas, byte/trust profile, reference verifier, independently verified fixed vectors and local CLI E2E |
| T02 | Implemented; independently reviewed and verified | Closed control APIs, publication/lifecycle/selection/quality reference models and regression tests; no production service implementation |
| T03閳ユ彑04 | Not completed | Platform/data scope, measured preparation budgets and trustworthy time evidence remain pending; follow the task graph |
| T05 | Implemented; independently reviewed and verified | Separate service management identity, MFA, scoped permissions, dual approval, protected audit and three-language admin; actual Keycloak realm acceptance remains pending |
| T06 | Implemented; independently reviewed and verified | Durable atomic executor, encrypted recovery projections, SQL lease fencing and background recovery; later business commands and production HA acceptance remain pending |
| T07閳ユ彑47 (including final T43) | Not completed | Follow the task graph; no runtime or certification completion is claimed |

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
