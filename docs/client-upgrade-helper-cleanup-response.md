# Supplied helper-cleanup response contract

T27 batch 31 implements the complete original helper-cleanup response API
contract. The fixed closed response Schema uses exactly the embedded common
and helper-cleanup credential resources; all other Schema resources fail locally.
Whole metadata input is canonicalized and strictly reparsed before validation.
The original credential independently retains its canonical64KiB cap and
unchanged supplied semantics at max(iat,nbf). The original response adds no
request-correlation or other binding guard.

Success returns unit. Schema-valid zero/duplicate signatures remain compatible
with the original API; supplied times do not establish trusted current time.
Contract acceptance does not authenticate the credential, approve protected
object identity/current eligibility, permit deletion, bypass stable launcher
qualification or revive an executor. Real T19/T28/T31/T32 ownership and local
protected-object workflows must separately establish those facts.

Four integration groups cover closed/required fields, original role isolation,
independent request IDs, supplied-time/signature compatibility,64KiB credential
and1MiB raw limits/duplicates, and288 cases from actual complete original Node
assertApiContract. Named controls assert intended original outcomes. A
local-resource unit rejects outside resolution and the real App facade covers
exact invalid-contract rejection with restored positive.

Verification and independent review evidence are recorded in
[progress](upgrade-implementation-progress.md). No Schema/OpenAPI/profile/vectors,
dependencies/locks/CI, crypto/trust, product version or gitlinks change.
Downloads retain no total/cumulative duration or retry cap. Complete
T27/T28/T31/T32 workflows remain unfinished.
