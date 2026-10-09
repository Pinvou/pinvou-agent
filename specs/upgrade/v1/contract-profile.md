# Upgrade protocol V1 byte and trust profile

Applies to T01 artifacts. These choices were confirmed for implementation on
2026-10-09: Ed25519, initial Root 2-of-3 threshold, other roles initially one valid
signature, and the resource limits below. The service/backend lives in a separate
repository whose location and technology have not yet been selected. This package
does not select that backend or introduce a production key service.

## Closed JSON and resource limits

Input is UTF-8 JSON without BOM, duplicate object keys, unpaired UTF-16 surrogates,
non-finite numbers or non-JSON whitespace. Escaped spellings of the same key count
as duplicates. Unknown fields are rejected by the applicable closed Schema.
Numbers follow RFC 8785's ECMAScript representation; protocol counters, revisions,
sizes and times are bounded safe integers. Versions are three canonical decimal
components without leading zeros. String data is not normalized with NFC/NFD.

| Limit | Bound |
|---|---:|
| Complete metadata envelope | 1,048,576 bytes |
| Complete credential envelope | 65,536 bytes |
| Nested values below the root | 32 levels |
| Decoded individual string, including an object key | 65,536 UTF-8 bytes |
| Members in any object or array | 4,096 |

Schema artifacts and local CLI input are also read within the metadata bound.
Their limits do not limit downloads or decoded installer payloads. Archive/extract
resource policy is a separate T03/T04 contract. Limit changes require contract and
compatibility review; remote objects cannot advertise larger parsing limits.

## Canonical envelope and signing input

Each envelope is exactly `{"signed":<closed role body>,"signatures":[...]}`.
The wire bytes must equal its RFC 8785 JCS encoding, with no trailing newline.
Signature entries contain `keyId` and an unpadded canonical base64url 64-byte
Ed25519 signature; entries are unique and sorted by keyID. Public Ed25519 keys
are unpadded canonical base64url encodings of 32 raw bytes.

`keyId = lowercaseHex(SHA-256(JCS({algorithm:"ed25519", publicKey})))`.
The key descriptor and role grant must be present in the caller's current trusted
Root. An envelope cannot introduce its own trust anchor or choose an algorithm.

Signing input is the ASCII string `pinvou-upgrade-signed-v1`, one **NUL byte**
`0x00`, then JCS of:

```json
{
  "protocolVersion": 1,
  "role": "<role>",
  "product": "<product>",
  "component": "<component or null for Root>",
  "scope": "<the actual scope object>",
  "signed": "<the complete signed object>"
}
```

The display strings above denote objects/values, not literal wire placeholders.
Metadata roles are `root`, `timestamp`, `snapshot`, `target`, `release`, `package`.
Credential roles/types, `aud`, `purpose`, `credentialPurpose` and event models are
closed by their individual Schemas. Domain separation includes the full signed
body, so identities, roles, lifetimes and conditional fields cannot be swapped.

Root is product-global (`component=null`, empty scope). Timestamp/Snapshot have
product/component scope. Target adds channel/targetKey; Release adds releaseId;
Package adds targetKey. Credentials add installationScopeId/channelRevision.
Role grants match component/channel/targetKey exactly; `null` means that dimension
does not apply, not a wildcard. Release-ID and installation-scope identity are
bound by the signed context and compared with trusted expected values.

Initial Root grants use three distinct keyIDs and threshold two. Root grants may
be strengthened by a reviewed successor but cannot contain fewer than three keys
or a threshold below two. Other role thresholds are positive and configurable.
High-risk management approval is additional business authorization, not a way to
replace a cryptographic threshold.

`envelopeSha256` is SHA-256 of the **complete canonical envelope bytes**, including
signatures. References bind role, scope, metadata version/revision, expiration,
size and that digest. Package references have `version=null, expiresAt=null`.
Package bodies contain neither their own envelope digest nor the outer upload
ZIP digest. `manifestId`, if present, is a logical label without security meaning.

Target separately lists approved ordinary paths and enabled historical bridge
paths. Bridge entries bind superseded Deployment ID/revision, BridgeEligibility
ID/revision and a Release reference. Every listed Release must contain that
Target's targetKey. Public membership follows only baseline/ordinary/bridge
references and their Packages; unrelated candidate catalogs remain forbidden.
Snapshot Target/Release entries are unique by role/scope, while Packages are
unique by complete envelope digest, allowing different Packages for one target.

Package binds a nonempty helper array (independent version, targetKey, native
identity and protected relative installation target) and executionHelperId.
Its `helper` identity must equal the selected array entry. SBOM/provenance bind
relative file name, schema version, format, size and hash; source revision is an
explicit git-sha1/40-hex or git-sha256/64-hex identity. Incremental fixtures also
declare baseVersion. Path declarations are normalized ASCII relative paths;
mapping them to protected OS locations and inspecting artifacts belong to T03/T04.

## Commitments and binding digests

The empty rollout commitment is:

`SHA-256(ASCII("pinvou-rollout-set-v1") || 0x00 || JCS([]))`.

The one-candidate commitment uses the same prefix and `JCS([opening])`.
Opening contains exactly targetKey, deploymentId/deploymentRevision,
rolloutId/rolloutRevision, releaseEnvelopeSha256 and leafSalt. Salt is 32 bytes
encoded as 64 lowercase hexadecimal digits; T08 must generate fresh random salt
for every applicable published change. Neither salt, candidate locator, identity,
version nor digest belongs in public Target/Snapshot catalogs. Locators are
opaque IDs, never arbitrary URLs, and expire no later than the decision/15 minutes.

sourceProfileId hashes only the closed stable-fact projection: product/component,
targetKey/canonicalAppVersion, application/helper/launcher native identities,
installation format/scope/ownership protocol and data migration facts. Registry,
approval/policy references, permission state and running owner/freeze epochs are
not part of this projection. Full current facts are independently bound through
clientFactsDigest; stable identity cannot substitute for actual runtime facts.

The consume semantic request includes the exact authorization envelope digest,
all purpose/scope/transaction identifiers, recoveryHandleHash, current facts and
current preparation/freeze/backup/staged facts. The immutable authorization digest
binds its complete metadata, chains, Registry, channel/generation and package
claims. `consumeRequestDigest=SHA-256(JCS(semanticRequest))`; only
consumeRequestDigest, recoverySecret, requestId and headers are excluded before
closed semantic-request validation. Secret is not transmitted in consume.

Update claims additionally bind `plans`: immutable planId/revision/sha256
references for installation and applicable migration, backup, helper migration,
launcher migration and takeover. Migration is nonnull exactly when migrationMode
is not none; backup is nonnull exactly when backupPolicy is required. A null
helper/launcher/takeover reference declares no such operation. Complete plan
bodies, approved transforms, exact source/target facts and execution prerequisites
are T03/T09 contracts; an ID alone cannot approve an operation. The Registry
version and all plan references must match protected approved facts. Plans stay
outside sourceProfileId and are carried exactly into the consume request.
`assertConsumeBinding` verifies the authorization then compares these plans,
purpose/scope, transaction, current facts and preparation/freeze/backup/slot claims.
This pure equality guard performs no consumption or remote qualification CAS.

Normal/forced use directInstall and install authorization; silent uses
stagedRestart with separate preinstall/activate authorizations. Host OS/arch must
match targetKey. A baseline endpoint must match the bound baseline ID/revision;
same-Deployment endpoint/hop chains must have identical complete facts.
SupplyChain effectiveExpiresAt may be null for an approval without an expiry.
Preinstall binds a dedicated inactive slotIdentity/revision; staged is null until
the later activation authorization. Activation binds a past stagedAt, the original
exact min(stagedAt+30d, installNotAfter) deadline and a new transaction identity.
Comparing this record with protected original waiting facts remains mandatory.

Task credentials include original purpose/request digest, workflow/stage/group,
both entity chains, source/target versions, upgrade type, signed Package reference,
download bytes, final installer and activation mode. Reconciliation also requires
a retirement proof digest. The caller must compare the full immutable task target
with its protected original record via `assertBindings`; valid signatures cannot
prove that an independently coherent statement is the caller's original task.
Task credentials create no installation, download or old-event rights.

`recoveryHandleHash=SHA-256(raw 32-byte secret)`.
`statusBindingHash=SHA-256(ASCII("pinvou-consume-status-v1") || 0x00 || JCS(binding))`.
The binding contains exactly recoveryHandleHash, product, component,
installationScopeId, purpose, authorizationJti, consumeKey, transactionId and
consumeRequestDigest. All digests are lowercase hexadecimal. A literal backslash
and `0` never substitute for the NUL byte.

## Time and lifecycle separation

All times are safe-integer UTC epoch milliseconds. Current time is trusted input,
not inferred from an incoming envelope. Valid windows are half-open; equality at
expiration rejects. Metadata uses issuedAt/expiresAt; credentials use iat/nbf/exp.
Occurrence is `[nbf,eventNotAfter)` and upload is `[nbf,exp)`.

The reference enforces the limits in S §9.4/9.5: 365d/24h/7d/30d/90d metadata,
15min decision/file/locator, 5min authorization/task action, 2min cleanup,
24h/7d telemetry or task event and 30d/37d transaction event. Event windows retain
their original session/task/transaction anchor, not each refreshed credential's
iat. Task observation must also fit its bound review window. No handling or clock
retention margin extends eligibility.

Consume query closes at authorizationExp+61d and minimal outcome retention ends
at +62d. The reference provides pure boundary predicates, not lifecycle storage,
secret cleanup or CAS. Similarly, high-water and immutable Release renewal checks
are pure guards; durable accepted bytes belong to T27. Root continuity is checked
with old and new thresholds, but publication/active read-set completeness is T07.

There is no total/cumulative download timeout field. Session expiration cannot
act as a download deadline. Workflow/session handoff, events deduplication,
late-diagnostic routing and original-stage quality are T02/T17/T20/T21/T44 duties.
Only explicit isolated contract fixtures may contain incrementalPackages; normal
production verification rejects them, and empty capabilities select FullPack.
