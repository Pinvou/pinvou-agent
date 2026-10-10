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
