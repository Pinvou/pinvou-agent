# Fixed supplied client response entry point

T27 batch 33 combines the established complete supplied response contracts
behind a fixed client-only operation enum and one Document/App byte entry point.
The original operation registry contains26 non-management and11 management
operations. The enum has exact names/roundtrips for all26 client operations;
unknown names, aliases/whitespace and all management names are rejected.
The existing independent family contracts and their interfaces remain intact.

An exhaustive typed match delegates to the existing check/refresh, download,
three-purpose, transaction-control, receipt, helper-cleanup and channel response
contracts. Existing original acceptance rules, canonical limits, credential
semantics and full relation gates are reused. Typed wrapped errors preserve
their original fixed display code; the App entry separately wraps raw JSON
errors and keeps the metadata input profile.

This is a contract routing entry for future real transport/owner integration.
Operation selection must correspond to the outstanding request there. Unit
success never authenticates the endpoint/Root or signatures, proves current
read-set/generation/owned object or durable ACK, writes settings, grants execution
rights or revives a retired executor. Management operations are not client
operations; the two task-create contracts previously implemented remain separate.

Three integration groups exercise exact operation roundtrips/rejection, typed
error/restored-positive behavior and773 cases from the actual original complete
API across all26 response fixtures and every26x26 cross-operation pair.
The original operation inventory and11 management exclusions are compared to
the fixed enum. All26 baseline positives are required; explicit relation
negatives preflight original full Schema/credential semantics before exact
API_RESPONSE_MISMATCH. Supplied credential-expiry negatives and all unsuccessful
results compare fixed original error codes. The real App entry exercises
metadata profile, raw duplicate error, typed channel rejection/restoration,
supplied consume-status and receipt routing.

Verification and independent review evidence are recorded in
[progress](upgrade-implementation-progress.md). No Schema/OpenAPI/profile/vectors,
dependencies/locks/CI, crypto/trust, product version or gitlinks change.
Downloads retain no total/cumulative duration or retry cap. Full T27 and
actual transport/ownership workflows remain unfinished.
