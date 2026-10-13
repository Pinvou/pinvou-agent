# Complete supplied client HTTP response entry

T27 batch 35 adds one fixed client HTTP response entry to compose the established
operation and error contracts. HTTP200 delegates unchanged complete operation
response validation;4xx/5xx delegates unchanged original OpenAPI-bound error
validation. All other statuses fail the fixed contract. Typed wrapper errors
preserve fixed original codes. No Schema/validator/membership changes occur.

The App byte entry first uses the metadata raw profile, including duplicate
rejection. Unit success means the HTTP status/body combination fits its supplied
contract even for a declared error. Callers must still interpret the HTTP status
and obtain real transport/request correlation, Root/signature/current-state,
durability and owner facts. Checking does not perform a request or retry, write
state/settings, release occupancy, grant execution rights or revive an executor.

Three integration groups cover exact branch selection, typed errors/restored
positives, undeclared statuses, metadata raw bounds/duplicates and15497 actual
original cases. The oracle reuses the existing complete API operation and
direct frozen OpenAPI/Ajv oracles, then adds undeclared-status and wrong-branch
controls. Successful operation and valid error contracts must cover all26
client operations; existing operation failure codes remain exact. The real
App facade checks both response kinds, wrong branches,204 rejection, raw
duplicate errors and restored operation success. There is no new resource unit.

Verification and independent review evidence are recorded in
[progress](upgrade-implementation-progress.md). No Schema/OpenAPI/profile/vectors,
dependencies/locks/CI, crypto/trust, product version or gitlinks change.
Downloads retain no total/cumulative duration or retry cap. Full T27 and
actual transport/ownership workflows remain unfinished.
