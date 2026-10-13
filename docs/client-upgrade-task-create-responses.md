# Supplied task-create response contracts

T27 batch 29 implements the complete original reconciliation-create and
recovery-review-create response contracts. A fixed two-operation enum selects
the frozen closed response Schemas with exactly five embedded common and four
action/event credential dependencies. All other resources fail local retrieval.

Whole metadata input is canonicalized and strictly reparsed before Schema
validation. Each original full credential independently retains its canonical
64KiB cap and uses unchanged supplied credential semantics at max(iat,nbf).
Both credentials bind the complete declared task; their entire scopes and
product/component/installId must match as in the original API contract.
Binary64-normalized complete JSON value equality preserves original JCS equality,
including object order independence, zero and array order. No field projection,
credential renewal, runtime task state or lifecycle transition is introduced.

Checking returns unit. Signatures are Schema data here: zero and duplicate
Schema-valid signatures remain compatible with the original contract.
Supplied times never become current trusted time. Contract acceptance does not
authenticate a task, prove a current task/retirement/evidence state, grant action
rights, release reconciliation occupancy or revive an executor.
T19/T22/T32 and the real transport/owner workflow must establish those facts.

Four Rust integration groups cover both operations, credential role isolation,
task/identity/scope negative bindings with independent credential Schema and
semantic preflight, restored positives, supplied-time/signature compatibility,
both credential limits and whole input limits/duplicates. An actual complete
original Node oracle covers 5694 nested required/scalar and explicit relation
cases. Named controls assert intended original outcomes; relation negatives
prove full response Schema and independent original credential semantics pass
before requiring exact API_RESPONSE_MISMATCH. No-op named mutations fail.
The real App facade covers both operations and exact binding failure/restoration.
A resource unit proves local-only resolution.

Verification and independent review evidence are recorded in
[progress](upgrade-implementation-progress.md). No Schema/OpenAPI/profile/vectors,
dependencies/locks/CI, crypto/trust, product version or gitlinks change.
Downloads retain no total/cumulative duration or retry cap. T27/T32 remain
unfinished until actual trust, transport, storage and ownership workflows exist.
