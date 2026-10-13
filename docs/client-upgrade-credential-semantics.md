# Client upgrade fourteen-role credential semantics

T27 batch18 implements the complete14-credential subset of frozen checkSemantics
using a SchemaCheckedEnvelope and existing SuppliedProtocolTime. The modules own
entry/lifetime/metadata-set rules, update-chain relationships, authorization
relationships, and event/task relationships respectively. Metadata references
reuse the unchanged private marker checker through pub(super), with no new public
generic validator. All provided semantics remain pure and have no state mutation,
authorization consumption, authoritative read, actual clock observation or execution.

Credential iat/exp intervals are positive and half-open with original role TTLs:
15min Decision/Download,5min three Authorizations and two task Actions,2min cleanup,
7days telemetry/task Events,37days transaction Events. Require iat<=nbf<exp and
now>=nbf. Decision availability agrees with both update!=null and reason==null.
MetadataSet checks actual host compatibility, four reference roles/current expiry,
entire Target channel/target scope, Release roles/expiry and complete reference
membership. Supply expiry is checked only in the original metadataSet+update branch.

Update binds mode, baseline, full same-ID entity chains, nullable migration/backup
plans, exact higher decimal versions, entity states, rollout/bridge presence and
path alternatives, asymmetric irreversible backup, package ID, both Release and
Package refs/target, visibility/end windows, full candidate opening fields and
locator bounds relative to iat/exp. Version comparison reuses the prior arbitrary-
magnitude decimal comparator; private Documents clone only already Schema/resource-
checked version strings. No new segment magnitude or digit cap is introduced.

Authorization retains exact JTI, freeze/backup/preparation epoch, staged/slot,
upgrade mode and activation relationships. Preinstall may precede installNotBefore
but still respects visibility/end; install/activate require installNotBefore.
Staged transactions cannot reuse the preinstall ID; stagedAt<=iat, stagedAt+30days
must be SAFE, and validUntil must equal the exact minimum with installNotAfter/
MAX_SAFE and remain open. Only this original staged addition receives a SAFE guard.
Other nonstaged additions retain original binary64 behavior even near MAX_SAFE.

Download binds whole package identity JCS. Event anchors retain transaction30/37
versus nontransaction1/7day limits. No new now<eventNotAfter or occurrence validation
is added here. Every task-bearing role retains task purpose/window ordering,
full Package/ref/target/version/mode relationships and reconcile-only retirement
proof. Only reconciliation/recovery-review Events bind nbf/eventNotAfter to task
window; Actions receive no new now-within-task-window rule. Locator checks also
retain the original iat-relative rule without a new now<locatorExpiresAt check.
All whole-object comparisons use bounded JCS and propagate failures.

Success proves only provided-claim semantics after full Schema/canonical encoding;
it does not verify signatures/key thresholds/Root lineage, authenticate supplied
time/expected source, establish current qualification/consume, persist high water,
alter ownership, execute or certify secure erasure. Six metadata roles are explicitly
unsupported by this entry point. Credential TTL is never a total download limit.

Four groups cover14 original credentials and6 unsupported metadata, exact expiry/
nbf/maxTTL/+1, every original scalar leaf, coherent candidate/bridge/ordinary-path
branches, full entity identity, preinstall-before-install, Action outside taskwindow/
Event at eventNotAfter, nonstaged nearSAFE positive, staged overflow refusal and
4096-digit version comparison. The7287-case actual Node oracle composes original
parser/encoding/full Schema/checkSemantics production:true; self-derived expected
context isolates semantics, not production context authority. Acceptance comparison
does not promise identical error-stage categories. App checks original Decision,
expiry, availability inconsistency and invalid supplied time.

No dependency/lock/Schema/CI/version/gitlink changes are needed. Applicable
regression filters only the unchanged31556-case Schema mutation test with B15 final
evidence; Schema golden and every other control run, and CI remains complete.
Final evidence is in [implementation progress](upgrade-implementation-progress.md).
Full T27 authenticity/persistence remains unfinished. Downloads retain no
total/cumulative duration or retry cap.
