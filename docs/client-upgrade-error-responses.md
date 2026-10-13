# Supplied client HTTP error response contracts

T27 batch 34 implements the original frozen client error contracts: closed
error/unactivated body Schemas and exact operation/HTTP-status membership.
The26 fixed client operations reuse the original OpenAPI operation error
alternatives, instead of a handwritten duplicated code/status table.
A local registry compiles both body Schemas using only the common resource.
Every client operation must exist exactly once with nonempty error alternatives;
missing/duplicate or invalid static resource shapes fail SchemaUnavailable.

Whole metadata input is canonicalized and strictly reparsed before validation.
Ordinary errors preserve fixed retryable flags and optional retryAfterMs
1..300000 where the original Schema allows it. SCOPE_UNACTIVATED requires
checkAfterMs1..86400000, appears only in check409 and cannot use retryAfterMs.
UNKNOWN_CONSUME_OUTCOME is consume-status404, with retryablefalse. Wrong
operation/status/extra fields fail the original closed contract.

These are supplied structural responses. Unit success does not authenticate
the endpoint, correlate a live request, prove committed/absent consumption or
release reconciliation occupancy, schedule a retry, approve current credentials
or revive an executor. A polling/retry hint is not a total download or retry
budget. Downloads still have no total/cumulative duration or retry cap.

Three integration groups cover operation/status relationships, special delay
and optional retry boundaries, all raw limits/duplicates and14464 cases validated
against actual frozen OpenAPI response Schemas using Ajv. The oracle resolves
original component pointers to an equivalent locally registered defs root;
it tests all21 codes across all26 client operations and status variations plus
every allowed error's required/scalar/flag/delay boundaries. Named controls
assert intended original outcomes. Two resource units prove local-only lookup
and mandatory complete unique client inventory. The actual App facade covers
special check409/refresh rejection and consume-status404 correlation domains.

Verification and independent review evidence are recorded in
[progress](upgrade-implementation-progress.md). No Schema/OpenAPI/profile/vectors,
dependencies/locks/CI, crypto/trust, product version or gitlinks change.
Full T27/real transport/ownership workflows remain unfinished.
