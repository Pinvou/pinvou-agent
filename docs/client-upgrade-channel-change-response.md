# Supplied channel-change response contract

T27 batch 32 implements the complete original channel-change response API
contract. The fixed closed response Schema uses only the embedded common
resource; all other schemas fail local retrieval. Whole metadata input is
canonicalized and strictly reparsed before validation. This response contains
no credential and has no extra original response semantic guard.

The Schema permits stable/beta/internal with positive safe channelRevision
1..9007199254740991. Request ID and revision are independent supplied structural
facts. Success returns unit and does not prove request correlation, a persisted
channel/settings write, current selection generation or permission to carry
old qualifications into a new channel. T17/T19/T28/T40 and real transport/owner
state must separately establish those facts. No settings are changed here.

Three integration groups exercise original channel/revision/closed-field
boundaries, whole1MiB raw profile/duplicates and40 cases from actual complete
original Node assertApiContract. Named controls assert intended original
outcomes. A local-resource unit and real App facade exercise resolution and
precise invalid-contract failure/restored positive for revision0/1.

Verification and independent review evidence are recorded in
[progress](upgrade-implementation-progress.md). No Schema/OpenAPI/profile/vectors,
dependencies/locks/CI, crypto/trust, product version or gitlinks change.
Downloads retain no total/cumulative duration or retry cap. Complete
T27/T28/T40 workflows remain unfinished.
