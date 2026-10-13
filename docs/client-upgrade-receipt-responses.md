# Supplied event and task-action receipt contracts

T27 batch 30 implements the complete original response contracts for six
event-report operations and two reconciliation/recovery-review action operations.
A fixed eight-operation enum selects closed frozen Schemas with exactly the
embedded common dependency. All other schema resources fail local resolution.
Whole metadata input is canonicalized and strictly reparsed before validation.
These receipts contain no credential and have no additional original API
response semantic guards.

Event acknowledgement disposition is committed/replayed/late-diagnostic; action
disposition is recorded/protection-retained/reconciled. Event responses require
1..4096 acknowledgement objects but do not specify uniqueItems: duplicates or
one event with different dispositions remain Schema-valid. Same-family Schemas
have identical acceptance and are not given invented purpose distinctions.
Request/event/task IDs and task revisions are supplied structural facts.

Success returns unit. It does not prove request correlation, actual server
durability, committed event lineage/quality admission, task completion,
reconciliation occupancy release or permission to revive an executor. Those
facts require real transport and owner protocols in T19/T20/T22/T32.
Structural duplicates must not become multiple quality contributions.

Four integration groups cover all eight responses, disposition domains,
same-family acceptance, explicit duplicate positives, nonempty/member and raw
byte/duplicate-key boundaries. The actual complete original Node API oracle
provides 358 required/scalar/explicit boundary cases with intended outcomes
asserted for named controls. A local-resource unit and real App facade cover
resolution and exact invalid-contract negatives/restored positives.

Verification and independent review evidence are recorded in
[progress](upgrade-implementation-progress.md). No Schema/OpenAPI/profile/vectors,
dependencies/locks/CI, crypto/trust, product version or gitlinks change.
Downloads retain no total/cumulative duration or retry cap. Complete
T27/T20/T22/T32 workflows remain unfinished.
