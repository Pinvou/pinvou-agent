# Upgrade V1 executable contracts

This package implements development tasks **T01** and **T02**: the metadata and
credential byte/trust contract, control APIs and executable transition models.
It is a reference tool and test oracle, not an update service
or a production client. Successful verification proves the stated contract
properties; it does not grant current download, preparation or execution rights.

The input requirements remain the three V1.0 project documents and the four
invariant specifications in this directory. The archived comprehensive design is
not normative. The [contract profile](contract-profile.md) freezes the concrete
encoding choices made here. The [model contract](model-contracts.md) defines T02's
atomic boundaries and read-only owner ports. T03 defines actual platform,
protected storage and backup interfaces; it is not implemented by these models.

The owner-approved 2026-10-25 time policy is in R section17.1 and D section13:
cross-boot time uncertainty pauses dependent actions, preserves original
deadlines/budgets and clears query secrets conservatively; fresh authenticated
server time recovery must finish original-deadline cleanup before resuming.
Existing numeric trusted-now models do not implement that local clock/persistent
recovery barrier. The earlier policy-only documentation update changed no wire
bytes. The subsequent owner-approved dedicated time role now adds the explicit
[time request/response profile](contract-profile.md#dedicated-fresh-request-time-response)
and fixed T26 API; client/service contracts must update together without a legacy
clock fallback. This does not implement the protected owner or recovery barrier.

## Run

Requires Node.js 22 or newer. Ajv 8.20.0 is the only direct dependency, isolated
from the application's dependencies. Schema lookup is local and performs no
network requests. Installation uses the committed npm lockfile and no lifecycle
scripts:

```powershell
npm ci --prefix specs/upgrade/v1 --ignore-scripts
npm --prefix specs/upgrade/v1 test
```

Local commands:

```powershell
node specs/upgrade/v1/reference/cli.mjs canonicalize input.json
node specs/upgrade/v1/reference/cli.mjs schema metadata/package package-envelope.json
node specs/upgrade/v1/reference/cli.mjs verify envelope.json trusted-root-body.json context.json
```

The verification context contains `now` (trusted UTC epoch milliseconds) and
`expected` with `role`, `product`, `component` and the complete expected `scope`.
The root file must contain a root body already established through an independent
trust anchor. Do not extract an incoming envelope's keys and feed them back as
trusted keys. CLI errors contain stable codes, never submitted claims or secrets.

The JSON Schemas are generated, committed artifacts. Regenerate them with:

```powershell
node specs/upgrade/v1/reference/build-schemas.mjs
node specs/upgrade/v1/reference/build-api.mjs
```

The separate public time vector generator is
`node specs/upgrade/v1/reference/generate-time-vectors.mjs`. It writes
`vectors/time/anchor.json` with direct and continuous-Root-successor cases; fresh
private keys stay in generator memory. Ordinary tests read the committed vector.
Do not regenerate the original `vectors/crypto` files for this extension.

Tests compare artifacts with their source, compile every Schema, test RFC 8785,
verify independently checked fixed signature/hash vectors, exercise required
fields and nested field tampering, and run the CLI end to end. Fixed vectors
contain public test verification material; their ephemeral private signing keys
were discarded. They are not production trust anchors or credentials.

## Reference boundaries

| Module | Responsibility |
|---|---|
| `canonical-json.mjs` | Bounded strict parsing and RFC 8785 canonical bytes |
| `build-schemas.mjs`, `schema-registry.mjs` | Closed field contracts and local Schema validation |
| `digests.mjs` | Artifact identities, stable facts, consume/status hashes and salted commitments |
| `signatures.mjs` | Scoped Ed25519 thresholds, continuous Root successors, dedicated time interval verification and pure high-water guards |
| `time-contract.mjs` | Closed time syntax and canonical nonce checks; no clock authority |
| `semantics.mjs` | Declared bindings and strict time/purpose constraints |
| `metadata-chain.mjs` | Public membership, candidate opening and Release-to-Package binding |
| `cli.mjs` | Bounded local file input and stable diagnostic output |
| `api-*.mjs`, `build-api.mjs` | Closed control/management Schemas and bundled OpenAPI 3.1 |
| `command-contracts.mjs` | Internal ownership and mandatory read/write families |
| `models/atomic.mjs`, `models/operation.mjs` | All-or-none final-instant CAS, absent-key races and worker fencing |
| `models/` | In-scope state, publication, selection and quality guard compositions |

`verifyEnvelope` checks an authenticated statement against an existing trusted
Root. The caller supplies trusted expected context, current deny information and,
where applicable, accepted high-water state. `assertBindings` compares a complete
expected fact projection with authenticated claims. It does not make client
self-reported facts trustworthy.

### Dedicated T26 time API and reference function

`POST /v1/upgrade/time` takes the exact `time/request` and returns the direct
canonical `time/response` Envelope, with 64 KiB limits on each body. It does not
create a session, decision or install permission. The complete wire and bootstrap
rules are in the [contract profile](contract-profile.md#dedicated-fresh-request-time-response).
The generic CLI `verify`/`verifyEnvelope` path deliberately refuses time; do not
pass serverTime or an old signed iat as its trusted now.

`verifyTimeResponse(bytes, context)` requires `request`, `trustedRootBytes`,
`sentMonotonicMs`, `receivedMonotonicMs`, `maxRoundTripMs`, `maxUncertaintyMs`,
`trustedTimeFloor` and `deniedKeyIds`; `rootSuccessors` is an optional ordered
array of original continuous Root Envelope bytes. It takes no now/wall-clock
argument and returns frozen `{earliest,latest,asOfMonotonicMs,root}` DATA only.
A caller must prove the input provenance and conservative send/receive brackets;
calling this function with a DTO does not establish those facts. API contract
validation checks time syntax separately instead of applying credential iat/nbf
semantics. Missing time policies or explicit limits fail closed.

The delivered Time reference batch's JS/Rust source passed independent source review. The Node reference
suite passed 219 tests; the Rust codec, verifier and HTTP crates passed 160, 75
and 4 tests respectively. One existing opt-in TLS test remained ignored.
Formatting, architecture and whitespace checks passed. Generated schemas,
OpenAPI and the public time vector are current; original crypto vectors are
unchanged.
Neither this result nor the vectors prove an actual issuer's SQL/UTC accuracy,
configured production time keys, protected host continuity, durable budget ledger
or cleanup recovery. T26 remains incomplete and concrete T03 budgets unapproved.

### Fixed Root history GET

`GET /v1/upgrade/roots/{product}/{version}.json` distributes at most 1 MiB of
original canonical T01 Root Envelope bytes. Product is a bounded ASCII
identifier and version an exact positive safe-integer decimal selector. The
OpenAPI `trust-metadata` GET is independent of the existing 27 POST client
operations. Rust `pinvou_upgrade_http::get_root_once` returns original
`Found(ReceivedRoot)` or closed `Missing` DATA; it checks Schema/context and the
exact selector, without requiring recovered time. Expired or time-less Root
bytes can therefore reach the dedicated protected-anchor continuity verifier.

A closed `ROOT_NOT_FOUND` 404 does not prove chain tail, latest/current Root or
absence of a later version. Received Root bytes cannot establish their own
anchor, clock, installation permission or current eligibility. The fixed-origin
reader preserves the existing TLS/no-proxy/no-redirect/no-retry/identity-encoding
and connect/idle-read rules, with no total elapsed download deadline. See the
[Root retrieval profile](contract-profile.md#fixed-root-history-retrieval).

### Metadata and publication helpers

`verifyPublicChain` verifies the complete supplied public component snapshot;
unreferenced Release/Package objects cannot be added as a public candidate catalog.
For a selected candidate, use `verifyCandidateRelease` with an authenticated
current Target and bound opening, then `verifyReleasePackage`, then compare each
actual artifact's bytes and OS-probed identity with its declarations. The OS probe
and same-object execution handoff belong to T03/T29/T33–T36.

`verifyArchivedPackage` deliberately proves historical immutable identity after
the historical Root expires. Its result cannot supply current online eligibility.
`verifyRootSuccessor` verifies old/new root thresholds and every supplied active
envelope's continued authorization. T07 owns obtaining the complete active
component read set and committing it atomically with the root head; this reference
does not discover components, publish metadata or implement CAS.
T02's separate publication model discovers complete component/head inventories
inside an explicitly complete, protected test snapshot and emits a CAS plan. It
does not turn that snapshot into a database implementation.
`verifyRootChain` traverses consecutive expired intermediate Roots for offline
recovery but returns only after an unexpired final Root validates. The ordinary
envelope verifier accepts only the caller's existing Root; new Root acceptance
must use these explicit continuous-transition functions.

The reference code does not install production keys, implement a production
endpoint, or introduce a remote signer, installer, application state, permission
prompt, database, backup, download deadline or runtime Node dependency into the
desktop application.

`openapi.yaml` uses JSON serialization, a valid YAML 1.2 subset. All OpenAPI
references are bundled and local. The service/admin implementation is separate;
this reference package does not implement its current-source transactions or
configure deployment. Actual time issuance, secure physical clock/state facts,
production recovery E2E and OS certification require their real owners.
