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
| `signatures.mjs` | Scoped Ed25519 thresholds, continuous root successors and pure high-water guards |
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

No production keys, API endpoints, remote signer, installer, application state,
permission prompt, database, backup, download deadline or runtime Node dependency
is introduced into the desktop application by this task.

`openapi.yaml` uses JSON serialization, a valid YAML 1.2 subset. All OpenAPI
references are bundled and local. The service/admin implementation belongs in a
separate repository whose location and technology are still to be provided.
Authentication providers, remote signing, RPO=0 transactions, secure physical
facts, production server E2E and OS certification remain later tasks.
