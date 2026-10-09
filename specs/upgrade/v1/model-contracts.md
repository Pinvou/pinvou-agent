# Upgrade V1 control and model contract

This is T02's executable specification. Normative inputs remain R/D/S V1.0 and
C/F/M/K in this directory, with the ownership graph in the V1.0 development task
document. These models freeze guard composition and independently testable
interfaces. They do not implement the owning production services, physical
helpers, database isolation, authentication or compatibility certification.

## Control transport

`openapi.yaml` is OpenAPI 3.1 using JSON syntax (YAML 1.2 compatible). Every Schema
reference is bundled locally. `schemas/api/` contains deterministic closed
artifacts; regenerate using `npm --prefix specs/upgrade/v1 run generate:api`.
All routes use POST with closed request/response bodies and stable error codes.
The relative server URL requires the later service to configure its real HTTPS
endpoint. Management authorization is an opaque header; T05 will provide actual
authentication, MFA and RBAC, without this task choosing a provider or token type.

The API validator checks resource bounds, role, purpose, audience and structural
bindings. It does not authenticate a submitted signature or authorize an action.
Intrinsic credential validation uses its own declared issuance/not-before time
only to check declaration consistency. Current rights require the separate
verifier and current protected qualification at the real commit instant.
Cancelled/expired original authorizations may be submitted as exact identity
snapshots for cancellation and an already-consumed transaction's execution
review. They create no new consume rights; T19 compares the protected originally
accepted envelope and separately rechecks current execution eligibility.

Unactivated check returns a closed 409 `SCOPE_UNACTIVATED` with `checkAfterMs` and
no credentials, session or workflow. Consume-status uniformly returns unknown
for invalid secret/binding, expired query window or absent outcome. Denied event
keys return `EVENT_KEY_DENIED` before looking at successful deduplication results.
Stable errors never include incoming claims, identifiers from hidden objects,
raw exception messages or secrets.

Check/refresh may carry `grayTargeting`: disabled/unavailable with null SN, or
enabled with the locally authorized serial number. T24 defaults reporting off.
This is transient TLS control input, not device authentication. Before any
durable idempotency row, request log, audit, export or ordinary business storage,
T17 must pass it to the private bucketing port and persist only the keyed HMAC
reference. T24/T15 own approved protected inclusion/exclusion sets. Raw SN is
forbidden in credentials, events and file URLs. No desktop settings or network
behavior is changed here.

## Atomic plan boundary

Every snapshot is a complete, already-authorized owner snapshot, not an HTTP
client dictionary. Each record is protected by its owning service and a revision
plus canonical complete-body hash. `recordKind` and `projectionOwner` describe
that protected provenance; writing those strings in a client object cannot make
it trusted. Required owner ports are closed typed projections with their complete
dependency keys, revisions and hashes. Mutable keys join the same final read set;
new keys join the same absent-key checks. Scope/domain inventories and indices
are maintained under the same serialization domain as every creator.

Models return a plan, not a committed result. `applyAtomically` accepts only the
original unmodified plan identity, checks all expected revisions/hashes and
absences before writing anything, then returns records and committed facts as
one result. Plan commit time must equal the actual supplied commit instant. A
worker delayed even one millisecond must re-run the entire model against the
current snapshot and trusted time. This prevents precomputed lease/expiry guards
from surviving to a later commit. T06 must implement those checks with real
RPO=0 transactional storage and durable audit/outbox/result rows.

Operations retain a 30-second lease, renew no sooner than ten seconds and have a
two-minute maximum continuous owner period. Takeover increases ownerEpoch. Old
owners cannot finish. First validate operation/owner/activeValidate registration
is one unit, with same-request recovery using the same lineage. Original results
remain immutable; 24-hour full replay followed by minimal recovery never refreshes
credentials or reopens a terminal.

`command-contracts.mjs` freezes mandatory read/write families and field owners.
Families such as entities/currentMetadata expand to every key in the concrete
owner projections. They are conditional unions, not optional hints or flat
database table lists. Every production composition must include its full expanded
read set. Model facts correspond to future durable audit/outbox writes.

## Metadata and publication

T07 owns current Root and the independently provisioned initial anchor. Revision
zero with `published=false` is not an established current Root. Genesis requires
the separate protected anchor's exact body hash, initial version one and only
unpublished revision-zero component heads; incoming keys cannot establish their
own anchor. Component publication/qualification cannot precede Root establishment.

Root rotation checks consecutive versions, both signature thresholds and all
public/private current envelopes across the complete active component inventory.
That operation preserves scoped verification keys even if existing metadata has
expired. It does not extend expiry or restore eligibility. Recovery first rotates
to an unexpired final Root, then publishes a fresh fully verified chain under the
two-head CAS. A current qualification always requires unexpired complete metadata.

Root genesis/rotation reads the immediate Root-role deny, a T05 protected scoped
management authorization (operator, MFA, permission and expiry) and two distinct
non-author/operator approvals of the exact Root envelope, base head and initial
anchor. Signing-key EmergencyDeny has the same exact high-risk approval boundary.
Every permission/approval record joins CAS; signatures do not replace approvals.

T07's complete key-use index and each protected last-use projection also join
Root CAS. Active issuers retain their authorized keys. Stopped issuers retain
the exact key material, role/component/channel/target authorization and original
threshold capability until the latest object/upload/idempotency/other recovery
deadline plus at least two minutes. The margin grants no additional rights.
Issuance owners must update these protected facts atomically before exposing a
new signature; the reference projection does not implement a signing service.

T08's private `metadataHead.releaseEnvelopes` map owns current signed Release
envelopes referenced by current public/private selections; it is not a public
candidate catalog or the historical archive. T11 holds only the immutable
Release business hash, state and a metadata-head reference. Renewal atomically
updates the T08 map and public references without changing T11 business revision.
Chain reads use this current map and verify its immutable business hash. Identical
metadata versions require identical complete envelope bytes, including signatures.
Higher Release metadata revisions may change only revision/issuance/expiry.

Private Release publication inputs bind the actual T11 record and frozen
business hash. The complete future public references and private openings define
the current map, with one exact envelope per Release across all scopes. Hidden
renewal changes the opening hash/salt, Target, generation and head atomically;
plain refresh cannot change a private opening. It keeps the actual Deployment,
Rollout, percentage and original quality window. Rollout publication can compose
the same renewal with its own legal stage transition and prospective path guard.

Removed or replaced current envelopes enter a separate T07-owned verification
archive in that same unit. It retains the exact trusted Root body/head hash,
protected lineage, historical Release and every precisely referenced Package
envelope. The guard verifies historical signatures/roles/thresholds and full
Release/Package references. Lineage binds the independent provisioned anchor,
Root version/body hash and immutable chain material identity/hash; all protected
dependencies join CAS. Root publication links an exact prepared successor lineage
without resetting the anchor. T07 later implements complete chain storage,
verification, access control and indefinite recoverability. Archives establish
historical identity only and never feed current selection/credentials/consume.

Publication compares Root/head/inventory and every required entity, verifies the
complete signed snapshot and atomically publishes all affected generations.
Refresh preserves selection, generation and baseline/ordinary/bridge business
references while refreshing full signed Release references. First activation uses
a complete signed staged component set, two base heads and a 15-minute bound.
Replacement fences the old set; old complete bytes are removed within 24 hours
of its original terminal/expiry. A second scope must retain every other current
component Target. Standalone publication cannot change a baseline without its
domain composition.

Single-scope lifecycle and quality commands require the exact affected scope,
not membership in a larger caller-supplied set. Every Target matches the protected
scope's support floor; these commands preserve its OS/host policy and do not own
configuration edits. Pause/resume and ordinary stage transitions preserve path
business identities. Reconciliation preserves the current baseline and exact
candidate identity/revisions, permits only immutable Release renewal and removes
a prior path only with protected current safety-loss facts in the final CAS.
Each affected scope's resolved opening is stored with the published Target/head
and pending/job completion, including multi-scope private Release renewal.
Every affected nonempty candidate opening uses a salt different from its actual
previous salt; a changed Release envelope hash or commitment cannot substitute.

Immediate deny is one T08-owned typed record, read by all relevant compositions.
Signing-key entries enumerate exact T01 roles; object entries enumerate exact
subject kinds/identities. Tightening commits deny, all affected generations,
pending flags and a unique durable reconciliation job together. Reconciliation
clears only the matching latest pending generations; it does not erase deny.
Each path/chain still rejects denied subjects after successful publication.

## Paths and final guards

T13's approved read-only path projection covers exactly the current selectable
and forward-only sources above supportFloor and below the endpoint. This set
may be empty. Every step binds exact from/to source profile IDs, Package/native
identity, mode and T46 certification. Adjacent steps must join the exact same
profile, not merely the same version. Forward-only steps also match exactly one
approved immutable policy edge and transform, including both fact profiles,
Package, mode, migration and backup policy.

Ordinary/bridge authorization must match the applicable signed Target and same
Snapshot: actual Deployment/revision, independent approval or eligibility ID and
revision, and full Release envelope reference. An ordinary intermediate cannot
borrow a paused/aborted own Rollout's eligibility. Forced paths cover the whole
endpoint installation window and use certified direct-install steps.

Completion verifies current paths and a separate T13 projection of the resulting
publication/domain view. The projected write hash binds the exact future scope,
head and superseded old baseline, with every current dependency still in CAS.
An old baseline is not an ordinary hop after supersession. Where needed, an exact
T13 prepared bridge unit validates the current disabled eligibility, resulting
parent/eligibility revisions and scoped T05 approval, then jointly commits the
bridge and baseline switch. No bridge is enabled early. A complete alternative
path needs no new bridge. These are finite reference guards, not T13 path search,
physical certification or a production T47/T14 implementation.

## Lifecycle, downloads and evidence

T44 models keep workflow, session, authorization, transaction, staged slot,
operation and independent task identities distinct. Validate binds the original
decision, stable source, actual endpoint/hop, Package/native helpers, type/mode,
migration/backup and approved plans. Dynamic clientFactsDigest is rechecked by
T19 against protected current preparation/facts; it is not the stable profile ID.
Consume includes continuous ownership, original staging completion, exact waiting
slot/revision, minimal outcome, timeout, scope index and quality watermark in one
unit. Terminal outcomes never reverse. Safe cancellation requires stopped exact
owners, zero writers and zero protected writes plus intact old application/data.
Activation cancellation may create a higher waiting revision with original expiry.
Consumption checks that the complete scope has no other nonterminal transaction;
waiting creation/rebuilding checks that no other waiting record exists. Index and
every member join CAS, so a stale state cannot bypass uniqueness. Activate validate
and consume both verify the exact current waiting tuple, original completed
preinstall, preparation pointer, quality attribution and immutable target.

24-hour sessions and 30-day server transaction reporting have independent clocks.
Neither imposes a total or cumulative download deadline. A logical download may
span arbitrary qualified sessions; handoff fences old future actions and preserves
original quality group/window/content. Trusted progress or confirmable waiting,
backoff or pause remains incomplete. Only loss beyond the approved finite reporting
interval becomes unknown. Sample insufficiency holds the current gray stage.
Handoff constructs a new session from the current signed Decision and original
workflow instead of copying a supplied record. It captures idle preparation facts,
resets sequence/authorization/activeValidate/freeze, reserves its own diagnostic
cursor and timeout, and requires new verification/permission/preparation events.
It also registers the exact from/to session, workflow, immutable context, hop
and original confirmation in both protected records. A general new session has
no such registration and cannot use the resume edge. Event acceptance rereads
the current workflow/intent and includes it in the final CAS.

Live progress advances workflow observability. Historical events must occur before
the original activity deadline/fence and follow legal sequence/edges. A separate
diagnostic cursor retains historical boundary, freeze and first health-start
anchors; it never revives server state. Events, dedup ledger, outbox and T20
contribution watermark commit together. Health success cannot reset the first
five-minute budget. A transaction observation cannot precede its protected boundary.
An accepted, legally sequenced download failure also locks the logical workflow
as failed in that same event/ledger/outbox commit, even when the event is lawful
historical evidence from a fenced session. It preserves the old Session and
failure quality evidence; future current-session actions and handoff are refused.
Unaccepted/denied/expired evidence cannot create this latch. Session expiry,
authorization consumption and completed preinstallation do not themselves end
the workflow or prevent a lawful fresh activation.
The record separates failure occurrence (`failedAt`) from the trusted server
confirmation/commit instant (`endedAt`). An unfenced current Session's lawful
events that occurred before that confirmation still enter diagnostics only;
events at or after it are refused. End record, cursor, ledger and outbox join CAS.
BeginValidate (including recovery), successful FinishValidate and first Consume
share a mandatory actual-workflow guard: ongoing, exact current writable Session,
immutable context/hash/hop and the original normal confirmation. Qualification
snapshots cannot replace that read. Failure/abort cleanup, authorization cancel,
expiry/status and already-consumed transaction convergence remain available.

T20 registers original window/group obligations at creation, consume and event
commit. T21 projections must catch up to all current watermarks and cover all
registered groups/required metrics under the frozen plan. Comparisons use exact
integer ratios; equality fails, zero samples fail, incomplete alone never freezes.
Confirmed failures remain latched even when older evidence arrives. Current
selectable paths and historical workflow quality groups are separate obligations.
Recovery must address every original cause and preserve original failed/unknown
outcomes, then collect a new full independent observation before advancing.

Baseline/parent resume reads the original pause disposition and new exact scoped
approval (two reviewers for Stable), keeps percentage/plan/quality freeze and
does not restart a completed child. Stable synchronization reads each selected
Stable source's current scope/full chain/generation/deny and public or private
membership, and creates independent Beta/Internal drafts atomically. Permanent
batch replay rechecks the actual operator's current target permissions without
creating duplicate drafts or acquiring new source eligibility.

Trusted time faults, protected OS facts, physical shutdown/offline timers, backup
budgets and actual certification are T03 and later adapters. The models require
trusted time; they do not substitute wall-clock guesses or claim that those
pending platform decisions have been solved.
