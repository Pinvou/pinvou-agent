# Client supplied download-info response contract

T27 batch 25 implements the complete original assertApiContract download-info
response subset. It uses the frozen closed response Schema and a closed registry
containing only its common and Download credential dependencies. Caller payload
references are data; only original embedded Schemas are compiled and every other
retrieval URI fails even if a parent enables networking.

Whole raw JSON/resource/duplicate validation precedes bounded JCS expansion and
reparse. Each embedded credential retains its 64 KiB canonical bound. Existing
B18/B24's unchanged private claims helper applies original supplied semantics at
max(iat,nbf). This is not authenticated present time. API outer signature records
are Schema data, without added sorting/deduplication or cryptographic proof.

Declared packageId, size and SHA must match the original credential's package.
The URL expiry must be at most credential exp and strictly greater than nbf.
Expiry at exp is allowed, at nbf is refused. Request ID, source class and URL
carry only original Schema constraints. URL length uses Unicode scalar counts;
a scheme-only string can satisfy the frozen Schema without being a usable URL.
Success returns unit, never ApprovedDestination, download rights, a trusted
credential or a scheduler deadline. T29 separately validates origin/DNS/socket
restrictions before actual transport. No URL is contacted by this checker.

A short-lived locator/credential window is not a total download time budget.
Current eligibility and fresh URL/credential renewal remain outside this pure
contract; ongoing transfers gain no total/cumulative duration or retry cap.

Four integration groups cover isolated package/window negatives and restores,
legal future nbf/Schema-only signatures, Unicode URL lengths, per-credential and
raw input limits, plus complete original Node API contract required/scalar
mutations. Node binding negatives first pass full Schema and original independent
credential semantics, then require exact API_RESPONSE_MISMATCH. A registry unit
tests closed retrieval; App tests the actual facade's package, expiry and Schema
refusals. Oracle output contains only public fixture envelopes, no private keys.
This is cross-language contract evidence, not network or installation E2E.

No dependency/lock/frozenSchema/CI/VERSION/gitlink changes. Evidence is recorded in
[implementation progress](upgrade-implementation-progress.md). Complete T27
trusted validation/persistence and T29 qualification/renewal remain unfinished.
